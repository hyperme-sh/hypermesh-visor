//! Linux package sidecar with a private inherited Unix listener.
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct ManagedConductor {
    child: std::sync::Mutex<Child>,
    socket: PrivateSocket,
}

impl ManagedConductor {
    pub fn start(config: &std::ffi::OsStr, journal: &std::ffi::OsStr) -> Result<Self, String> {
        let socket = PrivateSocket::bind()?;
        let inherited: OwnedFd = socket
            .listener
            .try_clone()
            .map_err(|e| e.to_string())?
            .into();
        let parent = std::process::id();
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let prefix = executable
            .parent()
            .and_then(|bin| bin.parent())
            .ok_or("invalid package layout")?;
        #[cfg(feature = "desktop")]
        let relative = "lib/hypermesh-visor/conductor";
        #[cfg(not(feature = "desktop"))]
        let relative = "lib/hypermesh-visor-headless/conductor";
        let mut command = Command::new(prefix.join(relative));
        command
            .arg("serve")
            .arg("--stdin-listener")
            .arg("--config")
            .arg(config)
            .arg("--journal")
            .arg(journal)
            .stdin(Stdio::from(inherited));
        // Only async-signal-safe syscalls between fork and exec. Check the parent
        // after installing the signal to cover death between fork and prctl.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|e| format!("starting bundled Conductor: {e}"))?;
        let mut managed = Self {
            child: std::sync::Mutex::new(child),
            socket,
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if managed
                .child
                .get_mut()
                .unwrap()
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err("bundled Conductor exited during startup".into());
            }
            if ready(managed.socket_path()) {
                return Ok(managed);
            }
            if Instant::now() >= deadline {
                return Err("bundled Conductor readiness timed out".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub async fn monitor(&self) -> std::io::Result<()> {
        loop {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                return Err(std::io::Error::other(format!(
                    "bundled Conductor exited: {status}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket.path
    }
}

impl Drop for ManagedConductor {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct PrivateSocket {
    listener: UnixListener,
    directory: PathBuf,
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl PrivateSocket {
    fn bind() -> Result<Self, String> {
        let directory = std::env::temp_dir().join(format!("vc-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "cannot create private Conductor directory")?;
        let path = directory.join("c.sock");
        let result = (|| {
            let listener =
                UnixListener::bind(&path).map_err(|_| "cannot bind private Conductor socket")?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "cannot protect Conductor socket")?;
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|_| "cannot inspect Conductor socket")?;
            Ok(Self {
                listener,
                directory: directory.clone(),
                path: path.clone(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&directory);
        }
        result
    }
}
impl Drop for PrivateSocket {
    fn drop(&mut self) {
        if let Ok(meta) = std::fs::symlink_metadata(&self.path) {
            if meta.dev() == self.device && meta.ino() == self.inode {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}
fn ready(path: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    if stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_millis(300)))
            .is_err()
    {
        return false;
    }
    if stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut status = [0u8; 12];
    stream.read_exact(&mut status).is_ok() && &status == b"HTTP/1.1 200"
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_listener_is_owned_and_removed_on_drop() {
        let socket = PrivateSocket::bind().unwrap();
        let path = socket.path.clone();
        let directory = socket.directory.clone();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(meta.uid(), unsafe { libc::geteuid() });
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::symlink_metadata(&directory).unwrap().mode() & 0o777,
            0o700
        );
        drop(socket);
        assert!(!path.exists());
        assert!(!directory.exists());
    }
    #[test]
    fn cleanup_preserves_replaced_path() {
        let socket = PrivateSocket::bind().unwrap();
        let path = socket.path.clone();
        let directory = socket.directory.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"keep").unwrap();
        drop(socket);
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
