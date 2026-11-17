//! Bounded local challenge/response with kernel-reported server identity.
use crate::{platform, refused};
use std::{
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
const SIZE: usize = 32;
const TIMEOUT: Duration = Duration::from_millis(500);

pub fn request(root: &Path, run: uuid::Uuid, authority: crate::Authority) -> io::Result<()> {
    match authority {
        crate::Authority::Visor => request_with(root, run, platform::verify_service),
        crate::Authority::Host => request_with(root, run, platform::verify_host_service),
    }
}
fn request_with(
    root: &Path,
    run: uuid::Uuid,
    verify: impl Fn(u32) -> io::Result<()>,
) -> io::Result<()> {
    let mut stream = transport::connect(root)?;
    verify(transport::peer(&stream)?)?;
    let mut request = [0; SIZE];
    request[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    request[16..].copy_from_slice(run.as_bytes());
    stream.write_all(&request)?;
    let mut response = [0; SIZE + 1];
    read_bounded(&mut stream, &mut response)?;
    if response[..SIZE] != request || response[SIZE] != 1 {
        return Err(refused());
    }
    stream.write_all(&[1])?;
    Ok(())
}
fn read_bounded(stream: &mut impl Read, mut bytes: &mut [u8]) -> io::Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(refused());
        }
        let size = stream.read(bytes)?;
        if size == 0 {
            return Err(refused());
        }
        bytes = &mut bytes[size..];
    }
    if Instant::now() >= deadline {
        return Err(refused());
    }
    Ok(())
}
pub struct Server(transport::Listener);
impl Server {
    pub fn bind(root: &Path) -> io::Result<Self> {
        Ok(Self(transport::bind(root)?))
    }
    pub fn poll(&self, mut allow: impl FnMut(uuid::Uuid) -> bool) -> io::Result<()> {
        // A client cannot starve cleanup by keeping the accept queue populated.
        if let Some(mut stream) = self.0.accept()? {
            let mut request = [0; SIZE];
            if read_bounded(&mut stream, &mut request).is_ok() {
                let id = uuid::Uuid::from_slice(&request[16..]).map_err(|_| refused())?;
                let mut response = [0; SIZE + 1];
                response[..SIZE].copy_from_slice(&request);
                response[SIZE] = u8::from(allow(id));
                if stream.write_all(&response).is_ok() {
                    let _ = read_bounded(&mut stream, &mut [0]);
                }
            }
        }
        Ok(())
    }
}
#[cfg(unix)]
mod transport {
    use super::*;
    use std::{
        fs,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::PermissionsExt,
                net::{UnixListener, UnixStream},
            },
        },
    };
    pub type Stream = UnixStream;
    pub struct Listener(UnixListener);
    fn setup(s: Stream) -> io::Result<Stream> {
        // macOS accepted sockets inherit the nonblocking listener flag.
        s.set_nonblocking(false)?;
        s.set_read_timeout(Some(TIMEOUT))?;
        s.set_write_timeout(Some(TIMEOUT))?;
        Ok(s)
    }
    pub fn connect(root: &Path) -> io::Result<Stream> {
        let path = root.join("service.sock");
        let bytes = path.as_os_str().as_bytes();
        unsafe {
            let mut address: libc::sockaddr_un = std::mem::zeroed();
            if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
                return Err(refused());
            }
            address.sun_family = libc::AF_UNIX as _;
            for (out, byte) in address.sun_path.iter_mut().zip(bytes) {
                *out = *byte as _;
            }
            let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let stream = UnixStream::from_raw_fd(fd);
            stream.set_nonblocking(true)?;
            let result = libc::connect(
                fd,
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of_val(&address) as _,
            );
            if result != 0 {
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
                    return Err(refused());
                }
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                if libc::poll(&mut poll, 1, TIMEOUT.as_millis() as _) != 1 {
                    return Err(refused());
                }
                let mut error: libc::c_int = 0;
                let mut len = std::mem::size_of_val(&error) as libc::socklen_t;
                if libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut error as *mut libc::c_int).cast(),
                    &mut len,
                ) != 0
                    || error != 0
                {
                    return Err(refused());
                }
            }
            stream.set_nonblocking(false)?;
            setup(stream)
        }
    }
    pub fn bind(root: &Path) -> io::Result<Listener> {
        let path = root.join("service.sock");
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Listener(listener))
    }
    impl Listener {
        pub fn accept(&self) -> io::Result<Option<Stream>> {
            match self.0.accept() {
                Ok((s, _)) => Ok(Some(setup(s)?)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(e),
            }
        }
    }
    pub fn peer(s: &Stream) -> io::Result<u32> {
        #[cfg(target_os = "linux")]
        unsafe {
            let mut cred: libc::ucred = std::mem::zeroed();
            let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
            if libc::getsockopt(
                s.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            ) != 0
                || len as usize != std::mem::size_of_val(&cred)
                || cred.pid <= 0
                || cred.uid != libc::geteuid()
            {
                return Err(refused());
            }
            Ok(cred.pid as u32)
        }
        #[cfg(target_os = "macos")]
        unsafe {
            let mut pid: libc::pid_t = 0;
            let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
            if libc::getsockopt(
                s.as_raw_fd(),
                0,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            ) != 0
                || len as usize != std::mem::size_of_val(&pid)
                || pid <= 0
            {
                return Err(refused());
            }
            Ok(pid as u32)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Err(refused())
    }
}
#[cfg(windows)]
mod transport {
    use super::*;
    use std::{
        cell::RefCell,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::Pipes::*};
    pub struct Stream {
        handle: HANDLE,
        server: bool,
    }
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                if self.server {
                    DisconnectNamedPipe(self.handle);
                }
                CloseHandle(self.handle);
            }
        }
    }
    fn name() -> io::Result<Vec<u16>> {
        Ok(format!(
            r"\\.\pipe\HypermeshCrashRetention-{}",
            hypermesh_local_storage::windows_user_sid()?
        )
        .encode_utf16()
        .chain([0])
        .collect())
    }
    pub struct Listener(RefCell<Option<Stream>>);
    fn create() -> io::Result<Stream> {
        let name = name()?;
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64,
                64,
                0,
                null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Stream {
            handle,
            server: true,
        })
    }
    pub fn bind(_: &Path) -> io::Result<Listener> {
        Ok(Listener(RefCell::new(Some(create()?))))
    }
    impl Listener {
        pub fn accept(&self) -> io::Result<Option<Stream>> {
            let mut slot = self.0.borrow_mut();
            if slot.is_none() {
                *slot = Some(create()?);
            }
            let handle = slot.as_ref().ok_or_else(refused)?.handle;
            let result = unsafe { ConnectNamedPipe(handle, null_mut()) };
            if result != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
                return Ok(slot.take());
            }
            if unsafe { GetLastError() } == ERROR_PIPE_LISTENING {
                return Ok(None);
            }
            Err(io::Error::last_os_error())
        }
    }
    pub fn connect(_: &Path) -> io::Result<Stream> {
        let name = name()?;
        let end = Instant::now() + TIMEOUT;
        loop {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                let stream = Stream {
                    handle,
                    server: false,
                };
                let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
                if unsafe { SetNamedPipeHandleState(handle, &mode, null(), null()) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(stream);
            }
            if Instant::now() >= end {
                return Err(refused());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn peer(s: &Stream) -> io::Result<u32> {
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(s.handle, &mut pid) } == 0 || pid == 0 {
            return Err(refused());
        }
        Ok(pid)
    }
    impl Read for Stream {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let end = Instant::now() + TIMEOUT;
            loop {
                let mut count = 0;
                let ok = unsafe {
                    ReadFile(
                        self.handle,
                        out.as_mut_ptr(),
                        out.len() as u32,
                        &mut count,
                        null_mut(),
                    )
                };
                if ok != 0 && count > 0 {
                    return Ok(count as usize);
                }
                if Instant::now() >= end {
                    return Err(refused());
                }
                if ok == 0 && unsafe { GetLastError() } != ERROR_NO_DATA {
                    return Err(io::Error::last_os_error());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Write for Stream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut count = 0;
            if unsafe {
                WriteFile(
                    self.handle,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut count,
                    null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(count as usize)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_challenge_binds_peer_and_run_decision() {
        let root = std::env::temp_dir().join(format!("ipc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let target = root.clone();
        let allowed = uuid::Uuid::new_v4();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let service = std::thread::spawn(move || {
            let server = Server::bind(&target).unwrap();
            ready_tx.send(()).unwrap();
            while done_rx.try_recv().is_err() {
                server.poll(|id| id == allowed).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let verify = |pid| {
            if pid == std::process::id() {
                Ok(())
            } else {
                Err(refused())
            }
        };
        let accepted = request_with(&root, allowed, verify);
        let rejected = request_with(&root, uuid::Uuid::nil(), verify);
        done_tx.send(()).unwrap();
        service.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        accepted.unwrap();
        assert!(rejected.is_err());
    }
    #[cfg(unix)]
    #[test]
    fn stopped_or_wrong_service_cannot_replay_a_response() {
        let root = std::env::temp_dir().join(format!("ipc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let server = Server::bind(&root).unwrap();
        // Kernel peer PID must match independently verified service ownership.
        assert!(request_with(&root, uuid::Uuid::nil(), |_| Err(refused())).is_err());
        // A real listener that does not answer is refused within a bounded wait.
        let at = Instant::now();
        assert!(request_with(&root, uuid::Uuid::nil(), |_| Ok(())).is_err());
        assert!(at.elapsed() < Duration::from_secs(2));
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }
}
