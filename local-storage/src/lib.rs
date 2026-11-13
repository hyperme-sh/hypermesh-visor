//! Owner-bound local files and locks. Existing unsafe paths are refused, never repaired.
use std::{fs::File, io, path::Path};
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[derive(Clone, Copy)]
pub enum Open {
    Read { private: bool },
    Lock,
    New,
}

pub fn open(path: &Path, mode: Open) -> io::Result<File> {
    platform::open(path, mode)
}
pub fn create_dir(path: &Path) -> io::Result<bool> {
    platform::create_dir(path)
}
pub fn check_dir(path: &Path) -> io::Result<()> {
    platform::check_dir(path)
}
pub fn lock(file: &File) -> io::Result<()> {
    platform::lock(file)
}
pub fn replace(source: &Path, target: &Path) -> io::Result<()> {
    platform::replace(source, target)
}
pub fn sync_dir(path: &Path) -> io::Result<()> {
    platform::sync_dir(path)
}
pub struct UnlockOnDrop<'a>(pub &'a File);
impl Drop for UnlockOnDrop<'_> {
    fn drop(&mut self) {
        platform::unlock(self.0);
    }
}
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "local storage must be an owner-controlled regular path",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(crate) struct Temp(pub(crate) std::path::PathBuf);
    impl Temp {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "visor-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            assert!(create_dir(&path).unwrap());
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn private_create_replace_and_exclusive_new() {
        let dir = Temp::new();
        assert!(!create_dir(&dir.0).unwrap());
        check_dir(&dir.0).unwrap();
        let target = dir.0.join("data");
        let mut file = open(&target, Open::New).unwrap();
        file.write_all(b"first").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(open(&target, Open::New).is_err());
        let source = dir.0.join("next");
        let mut file = open(&source, Open::New).unwrap();
        file.write_all(b"second").unwrap();
        file.sync_all().unwrap();
        drop(file);
        replace(&source, &target).unwrap();
        sync_dir(&dir.0).unwrap();
        let mut bytes = String::new();
        open(&target, Open::Read { private: true })
            .unwrap()
            .read_to_string(&mut bytes)
            .unwrap();
        assert_eq!(bytes, "second");
        assert!(open(&dir.0, Open::Read { private: false }).is_err());
    }
    #[test]
    fn lock_contention_and_explicit_unlock() {
        let dir = Temp::new();
        let path = dir.0.join("lock");
        let first = open(&path, Open::Lock).unwrap();
        lock(&first).unwrap();
        let inherited = first.try_clone().unwrap();
        let second = open(&path, Open::Lock).unwrap();
        assert!(lock(&second).is_err());
        drop(UnlockOnDrop(&first));
        lock(&second).unwrap();
        drop(UnlockOnDrop(&second));
        drop(inherited);
    }
    #[test]
    fn linked_files_are_refused() {
        let dir = Temp::new();
        let path = dir.0.join("data");
        drop(open(&path, Open::New).unwrap());
        let link = dir.0.join("link");
        fs::hard_link(&path, &link).unwrap();
        assert!(open(&path, Open::Read { private: true }).is_err());
        assert!(open(&link, Open::Lock).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn public_and_symlink_paths_are_refused() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = Temp::new();
        let path = dir.0.join("data");
        drop(open(&path, Open::New).unwrap());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(open(&path, Open::Read { private: true }).is_err());
        assert!(open(&path, Open::Read { private: false }).is_ok());
        let link = dir.0.join("link");
        symlink(&path, &link).unwrap();
        assert!(open(&link, Open::Read { private: false }).is_err());
        let directory_link = dir.0.join("directory-link");
        symlink(&dir.0, &directory_link).unwrap();
        assert!(check_dir(&directory_link).is_err());
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_dir(&dir.0).is_err());
        assert!(create_dir(&dir.0).is_err());
    }
}

#[cfg(windows)]
pub use platform::{
    check_private_object as windows_check_private_object,
    private_attributes as windows_private_attributes, user_sid as windows_user_sid,
};
