//! Same-user Conductor transport. Socket identity is checked before any credential.
use crate::prompt::{ForwardedPrompt, PromptCancellation, PromptCompletion, PromptDoor};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
pub fn default_path(
    mut env: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if let Some(path) = env("HYPERMESH_CONDUCTOR_SOCKET") {
        return validate_path(path.into());
    }
    let home = env("HOME").ok_or("Conductor socket needs HOME or an explicit path")?;
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(home).join("Library/Caches");
    #[cfg(not(target_os = "macos"))]
    let base = env("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".cache"));
    validate_path(base.join("hypermesh/conductor/run/conductor.sock"))
}
#[cfg(unix)]
fn validate_path(path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path.as_os_str().len() >= crate::local_peer::SUN_PATH_MAX
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("invalid private Conductor socket path".into());
    }
    Ok(path)
}
#[cfg(unix)]
fn private_socket(path: &Path, uid: u32) -> Result<(), String> {
    let parent = std::fs::symlink_metadata(path.parent().ok_or("invalid Conductor socket")?)
        .map_err(|_| "Conductor is unavailable; configure and start its local service")?;
    let socket = std::fs::symlink_metadata(path)
        .map_err(|_| "Conductor is unavailable; configure and start its local service")?;
    if !parent.is_dir()
        || !socket.file_type().is_socket()
        || parent.uid() != uid
        || socket.uid() != uid
        || parent.mode() & 0o077 != 0
        || socket.mode() & 0o077 != 0
    {
        return Err("Conductor socket is not private to this user".into());
    }
    Ok(())
}
pub(crate) struct SocketDoor {
    path: PathBuf,
    lease: Option<uuid::Uuid>,
}
impl SocketDoor {
    pub(crate) fn connect(path: PathBuf, lease: Option<uuid::Uuid>) -> Result<Self, String> {
        Ok(Self {
            path: validate_path(path)?,
            lease,
        })
    }
    async fn exchange(
        &self,
        call: &ForwardedPrompt,
        cancel: &PromptCancellation,
        attribution: &crate::telemetry::UpstreamScope,
    ) -> Result<PromptCompletion, String> {
        #[cfg(unix)]
        let stream = {
        let uid = unsafe { libc::geteuid() };
        private_socket(&self.path, uid)?;
        let stream = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::UnixStream::connect(&self.path),
        )
        .await
        .map_err(|_| "Conductor connection timed out")?
        .map_err(|_| "Conductor connection failed")?;
        if crate::local_peer::uid(&stream).map_err(|_| "Conductor peer identity unavailable")?
            != uid
        {
            return Err("Conductor peer belongs to another user".into());
        }
        stream
        };
        #[cfg(windows)]
        let stream = hypermesh_local_transport::connect(&hypermesh_local_transport::Endpoint::conductor_path(self.path.clone()).map_err(|_| "invalid Conductor pipe")?).await.map_err(|_| "Conductor connection failed")?;
        if cancel.cancelled() {
            return Err("prompt cancelled".into());
        }
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| "Conductor connection failed")?;
        struct Driver(tokio::task::JoinHandle<()>);
        impl Drop for Driver {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _driver = Driver(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let model = call
            .model
            .as_deref()
            .ok_or("Conductor requires a selected model")?;
        let payload =
            json!({"model":model,"messages":[{"role":"user","content":call.prompt}]}).to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", call.api_key))
            .header("X-Conductor-Expected-Model", model)
            .header(
                "X-Conductor-Task",
                self.lease
                    .or(call.task_id)
                    .unwrap_or_else(uuid::Uuid::new_v4)
                    .to_string(),
            )
            .header("X-Conductor-Step", uuid::Uuid::new_v4().to_string())
            .body(Full::new(Bytes::from(payload)))
            .map_err(|_| "invalid Conductor request")?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| "Conductor request failed")?;
        attribution.response_metadata(
            response
                .headers()
                .get("X-Hypermesh-Version")
                .and_then(|v| v.to_str().ok()),
            response
                .headers()
                .get("X-Hypermesh-Release-Color")
                .and_then(|v| v.to_str().ok()),
            response
                .headers()
                .get("X-Hypermesh-Canary")
                .and_then(|v| v.to_str().ok()),
        );
        if response.status() != StatusCode::OK {
            if response.status().is_server_error() {
                crate::telemetry::report(crate::telemetry::Failure::UpstreamHttp);
            }
            return Err(format!("Conductor HTTP {}", response.status().as_u16()));
        }
        let result = async {
            let raw = Limited::new(response.into_body(), 1 << 20)
                .collect()
                .await
                .map_err(|_| "invalid Conductor response")?
                .to_bytes();
            let text = std::str::from_utf8(&raw).map_err(|_| "invalid Conductor response")?;
            let value = serde_json::from_str(text).map_err(|_| "invalid Conductor response")?;
            Ok(PromptCompletion {
                text: crate::prompt::completion_text(text)?,
                usage: crate::ResponseUsage::from_openai(&value),
            })
        }
        .await;
        if result.is_err() {
            crate::telemetry::report(crate::telemetry::Failure::UpstreamResponse);
        }
        result
    }
}
impl PromptDoor for SocketDoor {
    fn forward(&self, call: &ForwardedPrompt) -> Result<String, String> {
        self.forward_reported(call).map(|r| r.text)
    }
    fn forward_reported(&self, call: &ForwardedPrompt) -> Result<PromptCompletion, String> {
        self.forward_reported_cancelled(call, &PromptCancellation::default())
    }
    fn forward_reported_cancelled(
        &self,
        call: &ForwardedPrompt,
        cancel: &PromptCancellation,
    ) -> Result<PromptCompletion, String> {
        let attribution =
            crate::telemetry::UpstreamScope::enter(crate::telemetry::Service::Conductor);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Conductor client unavailable")?;
        runtime.block_on(async{
            tokio::select! {
                biased;
                _=async {loop {if cancel.cancelled(){break;} tokio::time::sleep(Duration::from_millis(25)).await;}} => Err("prompt cancelled".into()),
                result=tokio::time::timeout(Duration::from_secs(130),self.exchange(call,cancel,&attribution)) => result.map_err(|_|"Conductor request timed out".to_string())?,
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn session_task_survives_multiple_steps_and_lease_takes_precedence() {
        use std::io::Write;
        for lease in [None, Some(uuid::Uuid::new_v4())] {
            let fixture = Fixture::new();
            let listener = fixture.listener.try_clone().unwrap();
            let session = uuid::Uuid::new_v4();
            let expected = lease.unwrap_or(session);
            let server = std::thread::spawn(move || {
                let mut steps = Vec::new();
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream).to_ascii_lowercase();
                    assert!(request.contains(&format!("x-conductor-task: {expected}")));
                    let step = request
                        .lines()
                        .find_map(|line| line.strip_prefix("x-conductor-step: "))
                        .unwrap()
                        .trim()
                        .to_owned();
                    assert!(uuid::Uuid::parse_str(&step).is_ok());
                    steps.push(step);
                    let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
                assert_ne!(steps[0], steps[1]);
            });
            let door = SocketDoor::connect(fixture.path.clone(), lease).unwrap();
            for _ in 0..2 {
                door.forward(&ForwardedPrompt {
                    prompt: "hello".into(),
                    api_key: "org_fixture".into(),
                    model: Some("model".into()),
                    task_id: Some(session),
                })
                .unwrap();
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn socket_failures_keep_safe_conductor_attribution() {
        use std::io::Write;
        for (status, body, expected) in [
            (503, "private upstream body", Some("upstream_http")),
            (200, "private malformed body", Some("upstream_response")),
            (403, "private policy body", None),
        ] {
            let fixture = Fixture::new();
            let listener = fixture.listener.try_clone().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                write!(stream,"HTTP/1.1 {status} Status\r\nX-Hypermesh-Version: 1.2.3\r\nX-Hypermesh-Release-Color: green\r\nX-Hypermesh-Canary: false\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            });
            let reports = crate::telemetry::capture_reports(|| {
                let door = SocketDoor::connect(fixture.path.clone(), None).unwrap();
                assert!(door
                    .forward(&ForwardedPrompt {
                        task_id: None,
                        prompt: "private prompt".into(),
                        api_key: "hm_org_fixture".into(),
                        model: Some("model".into())
                    })
                    .is_err());
            });
            server.join().unwrap();
            if let Some(expected) = expected {
                assert_eq!(reports.len(), 1);
                let text = reports[0].to_string();
                assert!(text.contains(expected));
                let props = &reports[0]["data"]["baseData"]["properties"];
                assert_eq!(props["upstream_service"], "conductor");
                assert_eq!(props["upstream_version"], "1.2.3");
                assert!(!text.contains("private"));
                assert!(!text.contains("hm_org_fixture"));
            } else {
                assert!(reports.is_empty());
            }
        }
    }

    #[test]
    fn explicit_socket_paths_are_bounded_and_absolute() {
        for path in ["relative", "/tmp/../other", ""] {
            assert!(SocketDoor::connect(path.into(), None).is_err());
        }
        assert!(SocketDoor::connect(format!("/{}", "a".repeat(200)).into(), None).is_err());
    }
    #[test]
    fn socket_default_does_not_select_a_network_endpoint() {
        let path = default_path(|name| match name {
            "HOME" => Some("/home/user".into()),
            _ => None,
        })
        .unwrap();
        assert!(path.ends_with("hypermesh/conductor/run/conductor.sock"));
    }
    struct Fixture {
        directory: PathBuf,
        path: PathBuf,
        listener: std::os::unix::net::UnixListener,
    }
    impl Fixture {
        fn new() -> Self {
            #[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            let directory = std::env::temp_dir().join(format!(
                "v{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let path = directory.join("c.sock");
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            Self {
                directory,
                path,
                listener,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn read_request(stream: &mut std::os::unix::net::UnixStream) -> String {
        use std::io::Read;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut raw = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&chunk[..n]);
            if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&raw[..split])
                    .unwrap()
                    .to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if raw.len() >= split + 4 + length {
                    return String::from_utf8(raw).unwrap();
                }
            }
        }
    }
    #[test]
    fn private_socket_preserves_model_lease_and_usage() {
        use std::io::Write;
        let fixture = Fixture::new();
        let listener = fixture.listener.try_clone().unwrap();
        let lease = uuid::Uuid::new_v4();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let lower = request.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer hm_org_fixture"));
            assert!(lower.contains(&format!("x-conductor-task: {lease}")));
            assert!(lower.contains("x-conductor-expected-model: selected-model"));
            let body = r#"{"choices":[{"message":{"content":"result"}}],"usage":{"prompt_tokens":8,"completion_tokens":2}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let door = SocketDoor::connect(fixture.path.clone(), Some(lease)).unwrap();
        let reply = door
            .forward_reported(&ForwardedPrompt {
                task_id: None,
                prompt: "private prompt".into(),
                api_key: "hm_org_fixture".into(),
                model: Some("selected-model".into()),
            })
            .unwrap();
        assert_eq!(reply.text, "result");
        assert_eq!(reply.usage.unwrap().input_tokens, 8);
        server.join().unwrap();
    }
    #[test]
    fn cancellation_closes_the_active_connection_without_retry() {
        use std::io::Read;
        let fixture = Fixture::new();
        let listener = fixture.listener.try_clone().unwrap();
        let (ready, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            ready.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });
        let door = SocketDoor::connect(fixture.path.clone(), None).unwrap();
        let cancel = PromptCancellation::default();
        let flag = cancel.clone();
        let request = std::thread::spawn(move || {
            door.forward_reported_cancelled(
                &ForwardedPrompt {
                    task_id: None,
                    prompt: "private".into(),
                    api_key: "hm_org_fixture".into(),
                    model: Some("model".into()),
                },
                &flag,
            )
        });
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel();
        assert!(request.join().unwrap().err().unwrap().contains("cancelled"));
        server.join().unwrap();
    }
    #[test]
    fn refuses_nonprivate_socket_before_sending_credentials() {
        #[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let door = SocketDoor::connect(fixture.path.clone(), None).unwrap();
        let result = door.forward(&ForwardedPrompt {
            task_id: None,
            prompt: "private".into(),
            api_key: "hm_org_fixture".into(),
            model: Some("model".into()),
        });
        assert!(result.unwrap_err().contains("not private"));
    }
}

#[cfg(windows)]
pub fn default_path(mut env: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    if let Some(path) = env("HYPERMESH_CONDUCTOR_SOCKET") { return validate_path(path.into()); }
    let instance = env("HYPERMESH_CONDUCTOR_INSTANCE").unwrap_or_else(|| "default".into());
    let instance = instance.to_str().ok_or("invalid Conductor instance")?;
    hypermesh_local_transport::Endpoint::conductor(instance).map(|e| e.path().into()).map_err(|_| "invalid Conductor instance".into())
}
#[cfg(windows)]
fn validate_path(path: PathBuf) -> Result<PathBuf, String> {
    hypermesh_local_transport::Endpoint::conductor_path(path).map(|e| e.path().into()).map_err(|_| "invalid Conductor pipe".into())
}
