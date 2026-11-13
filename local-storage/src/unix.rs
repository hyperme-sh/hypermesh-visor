use super::{refused, Open};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
};
fn owned(meta: &fs::Metadata, private: bool) -> bool {
    meta.uid() == unsafe { libc::geteuid() } && (!private || meta.mode() & 0o077 == 0)
}
pub fn open(path: &Path, mode: Open) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let private = match mode {
        Open::Read { private } => private,
        Open::Lock => {
            options.write(true).create(true).truncate(false).mode(0o600);
            true
        }
        Open::New => {
            options.write(true).create_new(true).mode(0o600);
            true
        }
    };
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || !owned(&meta, private) {
        return Err(refused());
    }
    Ok(file)
}
pub fn check_dir(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && owned(&meta, true) {
        Ok(())
    } else {
        Err(refused())
    }
}
pub fn create_dir(path: &Path) -> io::Result<bool> {
    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e),
    };
    check_dir(path)?;
    Ok(created)
}
pub fn lock(file: &File) -> io::Result<()> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
pub fn unlock(file: &File) {
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}
pub fn replace(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}
pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
