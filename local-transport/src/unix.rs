use super::{refused, Endpoint};
use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
};
pub type Client = tokio::net::UnixStream;
pub type Server = tokio::net::UnixStream;
pub struct Listener(tokio::net::UnixListener);
fn private_socket(endpoint: &Endpoint) -> io::Result<()> {
    hypermesh_local_storage::check_dir(endpoint.path().parent().ok_or_else(refused)?)?;
    let meta = std::fs::symlink_metadata(endpoint.path())?;
    if !meta.file_type().is_socket()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(refused());
    }
    Ok(())
}
fn peer(stream: &tokio::net::UnixStream) -> io::Result<()> {
    if super::peer::uid(stream)? == unsafe { libc::geteuid() } {
        Ok(())
    } else {
        Err(refused())
    }
}
pub async fn connect(endpoint: &Endpoint) -> io::Result<Client> {
    private_socket(endpoint)?;
    let stream = Client::connect(endpoint.path()).await?;
    peer(&stream)?;
    Ok(stream)
}
impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        hypermesh_local_storage::check_dir(endpoint.path().parent().ok_or_else(refused)?)?;
        let listener = tokio::net::UnixListener::bind(endpoint.path())?;
        std::fs::set_permissions(endpoint.path(), std::fs::Permissions::from_mode(0o600))?;
        Ok(Self(listener))
    }
    pub async fn accept(&mut self) -> io::Result<Server> {
        let (stream, _) = self.0.accept().await?;
        peer(&stream)?;
        Ok(stream)
    }
}
