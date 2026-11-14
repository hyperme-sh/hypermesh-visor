use super::Endpoint;
use std::io;
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use windows_sys::Win32::{
    Foundation::ERROR_PIPE_BUSY, Storage::FileSystem::SECURITY_IDENTIFICATION,
};
pub type Client = tokio::net::windows::named_pipe::NamedPipeClient;
pub type Server = tokio::net::windows::named_pipe::NamedPipeServer;
pub struct Listener {
    endpoint: Endpoint,
    waiting: Server,
}
fn create(endpoint: &Endpoint, first: bool) -> io::Result<Server> {
    hypermesh_local_storage::windows_private_attributes(|attributes| unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(64)
            .create_with_security_attributes_raw(endpoint.path(), attributes)
    })
}
pub async fn connect(endpoint: &Endpoint) -> io::Result<Client> {
    loop {
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(endpoint.path())
        {
            Ok(client) => {
                hypermesh_local_storage::windows_check_private_object(&client)?;
                super::windows_identity::check(&client)?;
                return Ok(client);
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                // Retry only acquisition of an unused pipe, before any request bytes.
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(error) => return Err(error),
        }
    }
}
impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(Self {
            endpoint: endpoint.clone(),
            waiting: create(endpoint, true)?,
        })
    }
    pub async fn accept(&mut self) -> io::Result<Server> {
        self.waiting.connect().await?;
        // Keep an instance registered while replacing the connected listener.
        let next = create(&self.endpoint, false)?;
        let stream = std::mem::replace(&mut self.waiting, next);
        // The OS enforces the private DACL before accepting the client.
        hypermesh_local_storage::windows_check_private_object(&stream)?;
        Ok(stream)
    }
}
