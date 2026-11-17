//! Account-wide bounded raw storage for isolated Host containers.
use crate::{refused, Authority, Owner};
use hypermesh_local_storage as storage;
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::Path,
};
const OPTIONS: &str = "size=167772160,nr_inodes=4096,mode=0700,uid=0,gid=0";
fn location(run: &Path) -> io::Result<(String, String)> {
    crate::host::require_root()?;
    if Owner::open()?.authority() != Authority::Host
        || run.parent().and_then(Path::parent) != Some(crate::root()?.join("config").as_path())
    {
        return Err(refused());
    }
    storage::check_dir(run)?;
    let text = run
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(refused)?;
    let id = uuid::Uuid::parse_str(text).map_err(|_| refused())?;
    if id.is_nil() || id.to_string() != text {
        return Err(refused());
    }
    let database = run.join("database");
    validate_database(&database)?;
    Ok((
        database.to_str().ok_or_else(refused)?.into(),
        format!("hypermesh-crash-{id}"),
    ))
}
fn validate_database(database: &Path) -> io::Result<()> {
    // A prior purge may already have removed the database directory.
    // Retain strict symlink/path checks whenever it still exists.
    match fs::symlink_metadata(database) {
        Ok(_) if fs::canonicalize(database)? != database => return Err(refused()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}
fn escaped(text: &str) -> String {
    text.replace('\\', "\\134")
        .replace(' ', "\\040")
        .replace('\t', "\\011")
        .replace('\n', "\\012")
}
fn mounted(text: &str, path: &str, source: &str) -> io::Result<bool> {
    let mut found = false;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 10 {
            return Err(refused());
        }
        if fields[4] != escaped(path) {
            continue;
        }
        let split = fields.iter().position(|v| *v == "-").ok_or_else(refused)?;
        if found
            || fields.len() <= split + 3
            || fields[split + 1] != "tmpfs"
            || fields[split + 2] != source
            || !["nosuid", "nodev", "noexec"]
                .iter()
                .all(|flag| fields[5].split(',').any(|v| v == *flag))
        {
            return Err(refused());
        }
        let options: Vec<_> = fields[split + 3].split(',').collect();
        if !options
            .iter()
            .any(|v| *v == "size=163840k" || *v == "size=167772160")
            || !options.contains(&"nr_inodes=4096")
        {
            return Err(refused());
        }
        found = true;
    }
    Ok(found)
}
fn mountinfo() -> io::Result<String> {
    let mut text = String::new();
    File::open("/proc/self/mountinfo")?
        .take(1048577)
        .read_to_string(&mut text)?;
    if text.len() > 1048576 {
        return Err(refused());
    }
    Ok(text)
}
pub fn is_bounded(run: &Path) -> io::Result<bool> {
    let (path, source) = location(run)?;
    mounted(&mountinfo()?, &path, &source)
}
pub fn create(run: &Path) -> io::Result<()> {
    let (path, source) = location(run)?;
    if mounted(&mountinfo()?, &path, &source)? || fs::read_dir(&path)?.next().is_some() {
        return Err(refused());
    }
    let target = CString::new(path.clone()).map_err(|_| refused())?;
    let source_c = CString::new(source.clone()).map_err(|_| refused())?;
    let options = CString::new(OPTIONS).unwrap();
    if unsafe {
        libc::mount(
            source_c.as_ptr(),
            target.as_ptr(),
            c"tmpfs".as_ptr(),
            libc::MS_NODEV | libc::MS_NOSUID | libc::MS_NOEXEC,
            options.as_ptr().cast(),
        )
    } != 0
    {
        return Err(refused());
    }
    let result = (|| {
        if !mounted(&mountinfo()?, &path, &source)? {
            return Err(refused());
        }
        // The independent OS cleanup service must see this exact mount too.
        // Private mount namespaces that hide it cannot authorize capture.
        let pulse: serde_json::Value = crate::read(&crate::root()?.join("heartbeat.json"))?;
        let pid = pulse["pid"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(refused)?;
        crate::platform::verify_host_service(pid)?;
        let own = fs::metadata(&path)?;
        let remote = fs::metadata(format!("/proc/{pid}/root{path}"))?;
        if own.dev() != remote.dev() || own.ino() != remote.ino() {
            return Err(refused());
        }
        Ok(())
    })();
    if result.is_err() {
        unsafe {
            libc::umount2(target.as_ptr(), 0);
        }
    }
    result
}
/// Caller holds the account capture lock. Erase linked raw files before
/// unmounting, so another namespace cannot retain a mounted copy of old data.
pub fn remove(run: &Path) -> io::Result<()> {
    let (path, source) = location(run)?;
    if mounted(&mountinfo()?, &path, &source)? {
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        storage::sync_dir(Path::new(&path))?;
        let target = CString::new(path).map_err(|_| refused())?;
        if unsafe { libc::umount2(target.as_ptr(), 0) } != 0 {
            return Err(refused());
        }
    }
    fs::remove_dir_all(run)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_removal_allows_missing_database_but_refuses_symlinks() {
        let run = std::env::temp_dir().join(format!("raw-mount-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&run).unwrap();
        let database = run.join("database");
        assert!(validate_database(&database).is_ok());
        fs::create_dir(&database).unwrap();
        assert!(validate_database(&database).is_ok());
        fs::remove_dir(&database).unwrap();
        assert!(validate_database(&database).is_ok());
        std::os::unix::fs::symlink(run.join("absent"), &database).unwrap();
        assert!(validate_database(&database).is_err());
        fs::remove_file(&database).unwrap();
        fs::remove_dir(run).unwrap();
    }
    #[test]
    fn only_owned_bounded_mount_is_removable() {
        let valid = "40 20 0:9 / /private/database rw,nosuid,nodev,noexec - tmpfs hypermesh-crash-test rw,size=163840k,nr_inodes=4096,mode=700";
        assert!(mounted(valid, "/private/database", "hypermesh-crash-test").unwrap());
        for bad in [
            valid.replace("163840k", "999999k"),
            valid.replace("nr_inodes=4096", "nr_inodes=9999"),
            valid.replace("hypermesh-crash-test", "unrelated"),
            valid.replace(",noexec", ""),
            format!("{valid}\n{valid}"),
        ] {
            assert!(mounted(&bad, "/private/database", "hypermesh-crash-test").is_err());
        }
        assert!(!mounted(valid, "/other/database", "hypermesh-crash-test").unwrap());
    }
}
