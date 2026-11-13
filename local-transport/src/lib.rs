//! Same-user local IPC. A path or SID in a name never substitutes for authentication.
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};
#[cfg(unix)]
#[path = "../../src/local_peer.rs"]
mod peer;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::{Client, Listener, Server};
#[cfg(windows)]
pub use windows::{Client, Listener, Server};

#[derive(Clone, Debug)]
pub struct Endpoint(PathBuf);
impl Endpoint {
    #[cfg(unix)]
    pub fn unix(path: PathBuf) -> io::Result<Self> {
        if !path.is_absolute() || path.as_os_str().len() >= peer::SUN_PATH_MAX {
            return Err(invalid());
        }
        Ok(Self(path))
    }
    #[cfg(windows)]
    pub fn windows(instance: &str, channel: &str) -> io::Result<Self> {
        if instance.is_empty()
            || instance.len() > 64
            || !instance
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !matches!(channel, "control" | "companion")
        {
            return Err(invalid());
        }
        let user = hypermesh_local_storage::windows_user_sid()?;
        Ok(Self(PathBuf::from(format!(
            r"\\.\pipe\hypermesh-visor-{user}-{instance}-{channel}"
        ))))
    }
    #[cfg(windows)]
    pub fn conductor(instance: &str) -> io::Result<Self> {
        if instance.is_empty()
            || instance.len() > 64
            || !instance
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid());
        }
        let user = hypermesh_local_storage::windows_user_sid()?;
        Ok(Self(PathBuf::from(format!(
            r"\\.\pipe\hypermesh-conductor-{user}-{instance}"
        ))))
    }
    #[cfg(windows)]
    pub fn conductor_path(path: PathBuf) -> io::Result<Self> {
        let user = hypermesh_local_storage::windows_user_sid()?;
        let prefix = format!(r"\\.\pipe\hypermesh-conductor-{user}-");
        let instance = path
            .to_str()
            .and_then(|p| p.strip_prefix(&prefix))
            .ok_or_else(invalid)?;
        Self::conductor(instance)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid local IPC endpoint")
}
#[cfg(unix)]
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "local IPC peer or path is not private to this user",
    )
}

pub async fn connect(endpoint: &Endpoint) -> io::Result<Client> {
    tokio::time::timeout(Duration::from_secs(2), async {
        #[cfg(unix)]
        {
            unix::connect(endpoint).await
        }
        #[cfg(windows)]
        {
            windows::connect(endpoint).await
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local IPC connection timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        endpoint: Endpoint,
        #[cfg(unix)]
        directory: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let name = format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            #[cfg(unix)]
            {
                let directory = std::env::temp_dir().join(format!("visor-ipc-{name}"));
                hypermesh_local_storage::create_dir(&directory).unwrap();
                Self {
                    endpoint: Endpoint::unix(directory.join("control.sock")).unwrap(),
                    directory,
                }
            }
            #[cfg(windows)]
            {
                Self {
                    endpoint: Endpoint::windows(&name, "control").unwrap(),
                }
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                let _ = std::fs::remove_dir_all(&self.directory);
            }
        }
    }
    #[tokio::test]
    async fn authenticated_round_trip_and_exclusive_listener() {
        let fixture = Fixture::new();
        let mut listener = Listener::bind(&fixture.endpoint).unwrap();
        assert!(Listener::bind(&fixture.endpoint).is_err());
        let (server, client) = tokio::join!(listener.accept(), connect(&fixture.endpoint));
        let mut server = server.unwrap();
        let mut client = client.unwrap();
        client.write_all(b"request").await.unwrap();
        let mut request = [0; 7];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        server.write_all(b"reply").await.unwrap();
        let mut response = [0; 5];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"reply");
        drop(client);
        drop(server);
        // The listener remains available for a new independent connection.
        let (second, next) = tokio::join!(listener.accept(), connect(&fixture.endpoint));
        assert!(second.is_ok() && next.is_ok());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn public_directory_socket_and_symlink_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let _listener = Listener::bind(&fixture.endpoint).unwrap();
        std::fs::set_permissions(&fixture.directory, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(connect(&fixture.endpoint).await.is_err());
        std::fs::set_permissions(&fixture.directory, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::set_permissions(
            fixture.endpoint.path(),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        assert!(connect(&fixture.endpoint).await.is_err());
        std::fs::set_permissions(
            fixture.endpoint.path(),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let alias = fixture.directory.join("alias.sock");
        std::os::unix::fs::symlink(fixture.endpoint.path(), &alias).unwrap();
        assert!(connect(&Endpoint::unix(alias).unwrap()).await.is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn default_acl_and_invalid_instance_are_refused() {
        use tokio::net::windows::named_pipe::ServerOptions;
        for name in ["", "../remote", "a\\b", "user name"] {
            assert!(Endpoint::windows(name, "control").is_err());
        }
        assert!(Endpoint::windows("valid", "other").is_err());
        let conductor = Endpoint::conductor("unit").unwrap();
        assert_eq!(
            Endpoint::conductor_path(conductor.path().into())
                .unwrap()
                .path(),
            conductor.path()
        );
        assert!(Endpoint::conductor_path(r"\\remote\pipe\conductor".into()).is_err());
        assert!(Endpoint::conductor("../remote").is_err());
        let fixture = Fixture::new();
        // The OS default pipe descriptor is broader than this protocol permits.
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(fixture.endpoint.path())
            .unwrap();
        assert!(connect(&fixture.endpoint).await.is_err());
    }
}
