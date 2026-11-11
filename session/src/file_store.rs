//! Explicit Unix file backend. No fallback from a failed OS keychain.
use crate::{AuthError, SessionStore};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const LIMIT: usize = 64 * 1024;

pub struct FileStore {
    directory: File,
    name: CString,
}

fn unavailable() -> AuthError {
    AuthError::message("session file store is unavailable or unsafe")
}

#[cfg(not(feature = "os-keyring"))]
pub(crate) fn configured() -> Result<FileStore, AuthError> {
    let path = std::env::var_os("HYPERMESH_SESSION_SECRET_FILE").ok_or_else(|| {
        AuthError::message("headless session storage requires HYPERMESH_SESSION_SECRET_FILE")
    })?;
    FileStore::open(Path::new(&path))
}

impl FileStore {
    /// The operator must select an absolute file under an existing private
    /// directory. Descriptor-relative operations remain inside that directory
    /// even if an ancestor is renamed while a token is written.
    pub fn open(path: &Path) -> Result<Self, AuthError> {
        if !path.is_absolute() {
            return Err(unavailable());
        }
        let parent = path.parent().ok_or_else(unavailable)?;
        let name = CString::new(path.file_name().ok_or_else(unavailable)?.as_bytes())
            .map_err(|_| unavailable())?;
        for ancestor in parent.ancestors() {
            if std::fs::symlink_metadata(ancestor)
                .map_err(|_| unavailable())?
                .file_type()
                .is_symlink()
            {
                return Err(unavailable());
            }
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)
            .map_err(|_| unavailable())?;
        let metadata = directory.metadata().map_err(|_| unavailable())?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o700
        {
            return Err(unavailable());
        }
        Ok(Self { directory, name })
    }

    fn read_file(&self) -> Result<Option<File>, AuthError> {
        let directory = self.directory.metadata().map_err(|_| unavailable())?;
        if directory.uid() != unsafe { libc::geteuid() } || directory.mode() & 0o777 != 0o700 {
            return Err(unavailable());
        }
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(unavailable())
            };
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.nlink() != 1
            || metadata.len() > LIMIT as u64
        {
            return Err(unavailable());
        }
        Ok(Some(file))
    }
}

impl SessionStore for FileStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        if token.is_empty() {
            return Err(AuthError::NoRefreshToken);
        }
        if token.len() > LIMIT {
            return Err(unavailable());
        }
        self.read_file()?;
        let temporary =
            CString::new(format!(".session-{}", crate::new_state()?)).map_err(|_| unavailable())?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(unavailable());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| {
            file.write_all(token.as_bytes())
                .map_err(|_| unavailable())?;
            file.sync_all().map_err(|_| unavailable())?;
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temporary.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.name.as_ptr(),
                )
            } != 0
            {
                return Err(unavailable());
            }
            self.directory.sync_all().map_err(|_| unavailable())
        })();
        if result.is_err() {
            unsafe { libc::unlinkat(self.directory.as_raw_fd(), temporary.as_ptr(), 0) };
        }
        result
    }
    fn refresh_token(&self) -> Result<Option<String>, AuthError> {
        let Some(file) = self.read_file()? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() > LIMIT {
            return Err(unavailable());
        }
        let token = String::from_utf8(bytes).map_err(|_| unavailable())?;
        if token.is_empty() {
            Ok(None)
        } else {
            Ok(Some(token))
        }
    }
    fn delete(&self) -> Result<(), AuthError> {
        if self.read_file()?.is_none() {
            return Ok(());
        }
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) } != 0 {
            return Err(unavailable());
        }
        self.directory.sync_all().map_err(|_| unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("visor-secret-test-{}", crate::new_state().unwrap()));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn explicit_private_file_roundtrips_and_deletes() {
        let dir = Directory::new();
        let path = dir.0.join("token");
        let store = FileStore::open(&path).unwrap();
        assert!(store.refresh_token().unwrap().is_none());
        store.put_refresh_token("first-private-value").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        store.put_refresh_token("replacement").unwrap();
        assert_eq!(
            store.refresh_token().unwrap().as_deref(),
            Some("replacement")
        );
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
        store.delete().unwrap();
        store.delete().unwrap();
        assert!(store.refresh_token().unwrap().is_none());
    }
    #[test]
    fn rejects_public_directories_symlinks_hardlinks_and_public_files() {
        let dir = Directory::new();
        let path = dir.0.join("token");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(FileStore::open(&path).is_err());
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = FileStore::open(&path).unwrap();
        let outside = dir.0.join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(store.put_refresh_token("private").is_err());
        assert!(store.refresh_token().is_err());
        assert!(store.delete().is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "unchanged");
        std::fs::remove_file(&path).unwrap();
        store.put_refresh_token("private").unwrap();
        std::fs::hard_link(&path, dir.0.join("link")).unwrap();
        assert!(store.refresh_token().is_err());
        std::fs::remove_file(dir.0.join("link")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.refresh_token().is_err());
        assert!(store.put_refresh_token("new").is_err());
    }
    #[test]
    fn rejects_relative_and_oversized_tokens() {
        assert!(FileStore::open(Path::new("relative/token")).is_err());
        let dir = Directory::new();
        let store = FileStore::open(&dir.0.join("token")).unwrap();
        assert!(store.put_refresh_token("").is_err());
        assert!(store.put_refresh_token(&"x".repeat(LIMIT + 1)).is_err());
        assert!(store.refresh_token().unwrap().is_none());
    }
}
