//! Assigned local Visor backend. No token, endpoint or routing input from ACP.
use super::transport::{Backend, BackendFuture};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use uuid::Uuid;

pub struct Machine {
    endpoint: hypermesh_local_transport::Endpoint,
    workspace: String,
    session: Mutex<Option<Uuid>>,
}
impl Machine {
    #[cfg(unix)]
    pub fn new(path: PathBuf, workspace: PathBuf) -> Result<Self, ()> {
        if !workspace.is_absolute() {
            return Err(());
        }
        Ok(Self {
            endpoint: hypermesh_local_transport::Endpoint::unix(path).map_err(|_| ())?,
            workspace: workspace.to_str().ok_or(())?.into(),
            session: Mutex::new(None),
        })
    }
    #[cfg(windows)]
    pub fn new(_: PathBuf, _: PathBuf) -> Result<Self, ()> {
        Err(())
    }
    async fn request(&self, path: &str, body: Option<Value>) -> Result<Value, ()> {
        tokio::time::timeout(Duration::from_secs(350), async {
            let stream = hypermesh_local_transport::connect(&self.endpoint)
                .await
                .map_err(|_| ())?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| ())?;
            struct Driver(tokio::task::JoinHandle<()>);
            impl Drop for Driver {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _driver = Driver(tokio::spawn(async move {
                let _ = connection.await;
            }));
            let method = if body.is_some() { "POST" } else { "GET" };
            let raw = match body {
                Some(value) => serde_json::to_vec(&value).map_err(|_| ())?,
                None => vec![],
            };
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "localhost")
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(raw)))
                .map_err(|_| ())?;
            let response = sender.send_request(request).await.map_err(|_| ())?;
            if response.status() != StatusCode::OK {
                return Err(());
            }
            let bytes = Limited::new(response.into_body(), 524288)
                .collect()
                .await
                .map_err(|_| ())?
                .to_bytes();
            serde_json::from_slice(&bytes).map_err(|_| ())
        })
        .await
        .map_err(|_| ())?
    }
}
impl Backend for Machine {
    fn open<'a>(&'a self, cancel: &'a AtomicBool) -> BackendFuture<'a, Uuid> {
        Box::pin(async move {
            if cancel.load(Ordering::SeqCst) {
                return Err(());
            }
            let value = self.request("/acp/context", None).await?;
            let session =
                Uuid::parse_str(value["session_id"].as_str().ok_or(())?).map_err(|_| ())?;
            if session.is_nil()
                || value["workspace"].as_str() != Some(self.workspace.as_str())
                || cancel.load(Ordering::SeqCst)
            {
                return Err(());
            }
            *self.session.lock().map_err(|_| ())? = Some(session);
            Ok(session)
        })
    }
    fn prompt<'a>(&'a self, text: &'a str, cancel: &'a AtomicBool) -> BackendFuture<'a, String> {
        Box::pin(async move {
            if text.is_empty() || text.len() > 65536 || cancel.load(Ordering::SeqCst) {
                return Err(());
            }
            let session = self.session.lock().map_err(|_| ())?.ok_or(())?;
            let value = self
                .request(
                    "/acp/prompt",
                    Some(json!({"session_id":session,"workspace":self.workspace,"text":text})),
                )
                .await?;
            let text = value["text"].as_str().ok_or(())?;
            if value["session_id"] != session.to_string()
                || text.len() > 65536
                || cancel.load(Ordering::SeqCst)
            {
                return Err(());
            }
            Ok(text.into())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn fixture(
        replies: Vec<Value>,
    ) -> (Machine, tokio::task::JoinHandle<Vec<Value>>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("acp-{}", Uuid::new_v4()));
        hypermesh_local_storage::create_dir(&dir).unwrap();
        let socket = dir.join("v.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = tokio::spawn(async move {
            let mut requests = vec![];
            for reply in replies {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                let end = loop {
                    let mut byte = [0];
                    connection.read_exact(&mut byte).await.unwrap();
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break bytes.len();
                    }
                };
                let headers = String::from_utf8(bytes.clone()).unwrap();
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                bytes.resize(end + len, 0);
                connection.read_exact(&mut bytes[end..]).await.unwrap();
                requests.push(if len == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[end..]).unwrap()
                });
                let body = reply.to_string();
                connection
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
            requests
        });
        (
            Machine::new(socket, PathBuf::from("/project")).unwrap(),
            server,
            dir,
        )
    }
    #[tokio::test]
    async fn assigned_session_routes_turn_without_credentials() {
        let session = Uuid::new_v4();
        let (machine, server, dir) = fixture(vec![
            json!({"session_id":session,"workspace":"/project"}),
            json!({"session_id":session,"text":"answer"}),
        ])
        .await;
        let cancel = AtomicBool::new(false);
        assert_eq!(machine.open(&cancel).await.unwrap(), session);
        assert_eq!(
            machine.prompt("private prompt", &cancel).await.unwrap(),
            "answer"
        );
        let requests = server.await.unwrap();
        assert_eq!(
            requests[1],
            json!({"session_id":session,"workspace":"/project","text":"private prompt"})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn wrong_project_and_unopened_session_cannot_prompt() {
        let (machine, server, dir) = fixture(vec![
            json!({"session_id":Uuid::new_v4(),"workspace":"/other"}),
        ])
        .await;
        let cancel = AtomicBool::new(false);
        assert!(machine.open(&cancel).await.is_err());
        assert!(machine.prompt("prompt", &cancel).await.is_err());
        assert_eq!(server.await.unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
