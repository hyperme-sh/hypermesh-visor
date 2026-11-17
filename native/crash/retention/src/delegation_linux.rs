//! Public-only container consumer. No credentials, consent or local fallback.
use crate::{
    delegation::{PublicConfig, CHALLENGE_TIMEOUT, MAX_CONFIG_SIZE, RESPONSE_SIZE},
    refused,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixStream,
        },
    },
    path::Path,
    time::Instant,
};
const CONTROL: &str = "/run/hypermesh-crash";
const SOCKET: &str = "/run/hypermesh-crash/cleanup.sock";
const DATABASE: &str = "/var/lib/hypermesh-crash/database";
#[derive(Clone)]
pub struct Client {
    config: PublicConfig,
}
fn remaining(start: Instant) -> io::Result<std::time::Duration> {
    CHALLENGE_TIMEOUT
        .checked_sub(start.elapsed())
        .filter(|v| !v.is_zero())
        .ok_or_else(refused)
}
fn protected(path: &Path, directory: bool) -> io::Result<File> {
    if fs::canonicalize(path)? != path {
        return Err(refused());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if meta.uid() != 0
        || meta.mode() & 0o022 != 0
        || meta.is_dir() != directory
        || (!directory && (!meta.is_file() || meta.nlink() != 1))
        || unsafe { libc::fstatvfs(file.as_raw_fd(), &mut stat) } != 0
        || stat.f_flag & libc::ST_RDONLY == 0
    {
        return Err(refused());
    }
    Ok(file)
}
fn connect(start: Instant) -> io::Result<UnixStream> {
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(refused());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    for (a, b) in address.sun_path.iter_mut().zip(SOCKET.bytes()) {
        *a = b as _;
    }
    let result = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as _,
        )
    };
    if result != 0 {
        // AF_UNIX EAGAIN means the backlog is full, not a pending connection.
        if io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(refused());
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let timeout = remaining(start)?.as_millis().min(i32::MAX as u128) as i32;
        if unsafe { libc::poll(&mut poll, 1, timeout) } != 1 || poll.revents != libc::POLLOUT {
            return Err(refused());
        }
        let mut error: i32 = 0;
        let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut len,
            )
        } != 0
            || error != 0
        {
            return Err(refused());
        }
    }
    stream.set_nonblocking(false)?;
    remaining(start)?;
    Ok(stream)
}
impl Client {
    /// Only the fixed read-only Host mount can authenticate the public key.
    /// Worker-writable raw metadata never selects a socket, key or authority.
    pub fn open(database: &Path) -> io::Result<Self> {
        if database != Path::new(DATABASE)
            || fs::canonicalize(database)? != database
            || unsafe { libc::geteuid() } != 0
        {
            return Err(refused());
        }
        hypermesh_local_storage::check_dir(database)?;
        protected(Path::new(CONTROL), true)?;
        let mut bytes = Vec::new();
        protected(&Path::new(CONTROL).join("public.json"), false)?
            .take((MAX_CONFIG_SIZE + 1) as u64)
            .read_to_end(&mut bytes)?;
        Ok(Self {
            config: PublicConfig::from_bytes(&bytes)?,
        })
    }
    /// A fresh challenge is required before every write. Never cache success.
    pub fn permit(&self) -> io::Result<()> {
        let start = Instant::now();
        let pending = self.config.challenge()?;
        let mut stream = connect(start)?;
        stream.set_write_timeout(Some(remaining(start)?))?;
        stream.write_all(pending.request())?;
        stream.shutdown(Shutdown::Write)?;
        let mut bytes = [0; RESPONSE_SIZE];
        let mut offset = 0;
        while offset < bytes.len() {
            stream.set_read_timeout(Some(remaining(start)?))?;
            let n = stream.read(&mut bytes[offset..])?;
            if n == 0 {
                return Err(refused());
            }
            offset += n;
        }
        stream.set_read_timeout(Some(remaining(start)?))?;
        if stream.read(&mut [0; 1])? != 0 {
            return Err(refused());
        }
        remaining(start)?;
        pending.verify(&bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_database_and_unprotected_mount_are_refused() {
        for p in [
            "database",
            "/tmp/database",
            "/run/hypermesh-crash/../database",
        ] {
            assert!(Client::open(Path::new(p)).is_err());
        }
        let path = std::env::temp_dir().join(format!("delegation-public-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"public").unwrap();
        assert!(protected(&path, false).is_err());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn expired_transport_budget_refuses() {
        assert!(remaining(Instant::now() - CHALLENGE_TIMEOUT).is_err());
    }
}
