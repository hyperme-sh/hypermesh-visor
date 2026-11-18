//! Opt-in client for the root-owned machine dispatcher. No controller bypass.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
#[cfg(unix)]
use tokio::net::UnixStream;

const WIRE_MAX: usize = 6 * 1048576 + 4096;
#[derive(Clone)]
pub(crate) struct Bridge {
    socket: PathBuf,
    slots: Arc<Semaphore>,
    uid: u32,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Entry {
    pub definition: Value,
    pub source_hash: String,
    pub graph_hash: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Call {
    pub scope: String,
    pub capability_id: String,
    pub call_id: String,
    pub arguments: Value,
}
impl Bridge {
    pub fn new(socket: PathBuf) -> Result<Self, ()> {
        #[cfg(windows)]
        if socket != std::path::Path::new(hypermesh_local_transport::WINDOWS_CRASH_PIPE) && socket != std::path::Path::new(hypermesh_local_transport::WINDOWS_FEEDBACK_PIPE) { return Err(()); }
        if !socket.is_absolute()
            || socket
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(());
        }
        Ok(Self {
            socket,
            slots: Arc::new(Semaphore::new(4)),
            uid: 0,
        })
    }
    #[cfg(unix)]
    async fn request(&self, method: &str, path: &str, body: Vec<u8>) -> Result<Value, ()> {
        let _permit = self.slots.try_acquire().map_err(|_| ())?;
        tokio::time::timeout(Duration::from_secs(if path == "/coding-run" { 335 } else if path == "/machine-prompt" { 320 } else { 40 }), async {
            let stream = UnixStream::connect(&self.socket).await.map_err(|_| ())?;
            if crate::local_peer::uid(&stream).map_err(|_| ())? != self.uid {
                return Err(());
            }
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| ())?;
            let driver = tokio::spawn(async move {
                let _ = connection.await;
            });
            // Abort the connection driver when the request is dropped or times out.
            struct Driver(tokio::task::JoinHandle<()>);
            impl Drop for Driver {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _driver = Driver(driver);
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "localhost")
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body)))
                .map_err(|_| ())?;
            let response = sender.send_request(request).await.map_err(|_| ())?;
            if response.status() != StatusCode::OK {
                return Err(());
            }
            let raw = Limited::new(response.into_body(), if path == "/feedback-grant" { 4096 } else { WIRE_MAX })
                .collect()
                .await
                .map_err(|_| ())?
                .to_bytes();
            serde_json::from_slice(&raw).map_err(|_| ())
        })
        .await
        .map_err(|_| ())?
    }
    #[cfg(windows)]
    async fn request(&self, method: &str, path: &str, body: Vec<u8>) -> Result<Value, ()> {
        let feedback = path == "/feedback-grant" && self.socket == std::path::Path::new(hypermesh_local_transport::WINDOWS_FEEDBACK_PIPE);
        let crash = path == "/crash-register" && self.socket == std::path::Path::new(hypermesh_local_transport::WINDOWS_CRASH_PIPE);
        if method != "POST" || (!feedback && !crash) || body.len() > if feedback { 256 } else { 2048 } { return Err(()); }
        let _permit = self.slots.try_acquire().map_err(|_| ())?;
        tokio::time::timeout(Duration::from_secs(15), async {
            let stream = if feedback { hypermesh_local_transport::connect_feedback_grant().await } else { hypermesh_local_transport::connect_crash_registration().await }.map_err(|_| ())?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|_| ())?;
            struct Driver(tokio::task::JoinHandle<()>);
            impl Drop for Driver { fn drop(&mut self) { self.0.abort(); } }
            let _driver = Driver(tokio::spawn(async move { let _ = connection.await; }));
            let request = Request::builder().method("POST").uri(path)
                .header("Host", "localhost").header("Content-Type", "application/json")
                .header("Connection", "close").body(Full::new(Bytes::from(body))).map_err(|_| ())?;
            let response = sender.send_request(request).await.map_err(|_| ())?;
            if response.status() != StatusCode::OK { return Err(()); }
            let raw = Limited::new(response.into_body(), 4096).collect().await.map_err(|_| ())?.to_bytes();
            serde_json::from_slice(&raw).map_err(|_| ())
        }).await.map_err(|_| ())?
    }
    pub(crate) async fn feedback_grant(&self, opted_in: bool) -> Result<crate::feedback_grant::FeedbackGrant, ()> {
        if !opted_in { return Err(()); }
        let value = self.request("POST", "/feedback-grant",
            br#"{"version":1,"human_reports_opt_in":true}"#.to_vec()).await?;
        crate::feedback_grant::FeedbackGrant::from_response(value)
    }
    pub async fn crash_register(&self, body: Value) -> Result<Value, ()> {
        let raw = serde_json::to_vec(&body).map_err(|_| ())?;
        if raw.len() > 2048 { return Err(()); }
        self.request("POST", "/crash-register", raw).await
    }
    pub async fn machine_context(&self) -> Result<Value, ()> {
        self.request("GET", "/machine-context", vec![]).await
    }
    pub async fn coding_run(&self) -> Result<Value, ()> {
        self.request("POST", "/coding-run", b"{}".to_vec()).await
    }

    pub async fn machine_prompt(&self, raw: Vec<u8>) -> Result<Value, ()> {
        if raw.len() > 512*1024 { return Err(()); }
        self.request("POST", "/machine-prompt", raw).await
    }
    pub async fn permission_context(&self) -> Result<Value, ()> {
        self.request("GET", "/permission-context", vec![]).await
    }
    pub async fn permission_action(&self, body: Value, admit: bool) -> Result<Value, ()> {
        let raw = serde_json::to_vec(&body).map_err(|_| ())?;
        if raw.len() > 196608 {
            return Err(());
        }
        self.request(
            "POST",
            if admit {
                "/permission-action-admit"
            } else {
                "/permission-action-review"
            },
            raw,
        )
        .await
    }
    pub async fn permission_service_context(&self) -> Result<Value, ()> {
        self.request("GET", "/permission-service-context", vec![])
            .await
    }
    pub async fn permission_service_execute(&self, body: Value) -> Result<Value, ()> {
        let raw = serde_json::to_vec(&body).map_err(|_| ())?;
        if raw.len() > 196608 {
            return Err(());
        }
        self.request("POST", "/permission-service-execute", raw)
            .await
    }
    pub async fn permission_dream(&self,body:Value)->Result<Value,()>{
 let raw=serde_json::to_vec(&body).map_err(|_|())?;if raw.len()>131072{return Err(())}
 self.request("POST","/permission-dream",raw).await
 }
 pub async fn permission_read_admit(&self, body: Value) -> Result<Value, ()> {
        let raw=serde_json::to_vec(&body).map_err(|_|())?;
        if raw.len()>196608 {return Err(())}
        self.request("POST","/permission-read-admit",raw).await
    }
    pub async fn permission_read_review(&self, body: Value) -> Result<Value, ()> {
        let raw=serde_json::to_vec(&body).map_err(|_|())?;
        if raw.len()>196608 {return Err(())}
        self.request("POST","/permission-read-review",raw).await
    }
    pub async fn permission_preview(&self, body: Value) -> Result<Value, ()> {
        let raw = serde_json::to_vec(&body).map_err(|_| ())?;
        if raw.len() > 196608 { return Err(()); }
        self.request("POST", "/permission-preview", raw).await
    }
    pub async fn catalog(&self) -> Result<Vec<Entry>, ()> {
        let value = self.request("GET", "/catalog", vec![]).await?;
        let entries: Vec<Entry> = serde_json::from_value(value).map_err(|_| ())?;
        if entries.len() > 64 || entries.iter().any(|e| e.definition["kind"] != "verb") {
            return Err(());
        }
        Ok(entries)
    }
    pub async fn call(&self, call: Call) -> Result<Value, ()> {
        if uuid::Uuid::parse_str(&call.call_id)
            .map_err(|_| ())?
            .to_string()
            != call.call_id
            || !call.arguments.is_object()
        {
            return Err(());
        }
        let arguments = serde_json::to_string(&call.arguments).map_err(|_| ())?;
        if arguments.len() > 65536 {
            return Err(());
        }
        let entries = self.catalog().await?;
        let entry = entries
            .iter()
            .find(|e| {
                e.definition["scope"] == call.scope && e.definition["id"] == call.capability_id
            })
            .ok_or(())?;
        let raw = serde_json::to_vec(&json!({
            "scope":call.scope, "owner":entry.definition["owner"], "capability_id":call.capability_id,
            "revision":entry.definition["revision"], "source_sha256":entry.source_hash,
            "graph_sha256":entry.graph_hash, "call_id":call.call_id, "arguments_json":arguments
        })).map_err(|_| ())?;
        if raw.len() > 6 * 65536 + 4096 {
            return Err(());
        }
        let result = self.request("POST", "/dispatch", raw).await?;
        if result["protocol_version"] != 1
            || result["call_id"] != call.call_id
            || result["completed"] != true
        {
            return Err(());
        }
        let output = result["output_json"].as_str().ok_or(())?;
        if output.len() > 1048576 {
            return Err(());
        }
        let output: Value = serde_json::from_str(output).map_err(|_| ())?;
        Ok(json!({"call_id":call.call_id,"output":output}))
    }
}
pub(crate) fn tool_name(entry: &Entry) -> Result<String, ()> {
    let scope = entry.definition["scope"].as_str().ok_or(())?;
    let id = entry.definition["id"].as_str().ok_or(())?;
    Ok(format!(
        "host_{:x}",
        Sha256::digest(format!("{scope}\0{id}"))
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn configuration_is_explicit_and_absolute() {
        assert!(Bridge::new("relative.sock".into()).is_err());
        assert!(Bridge::new("/tmp/../socket".into()).is_err());
        assert_eq!(Bridge::new("/run/host.sock".into()).unwrap().uid, 0);
    }
    #[tokio::test]
    async fn refuses_peer_before_sending_http() {
        let path = std::env::temp_dir().join(format!("v-{}.s", uuid::Uuid::new_v4().simple()));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let mut bridge = Bridge::new(path.clone()).unwrap();
        bridge.uid = unsafe { libc::geteuid() } + 1;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            stream.readable().await.unwrap();
            let mut bytes = [0; 16];
            assert_eq!(stream.try_read(&mut bytes).unwrap(), 0);
        });
        assert!(bridge.catalog().await.is_err());
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn binds_dispatch_to_catalog_and_never_retries_refusal() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = std::env::temp_dir().join(format!("v-{}.s", uuid::Uuid::new_v4().simple()));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: Request<hyper::body::Incoming>| {
                            let seen = seen.clone();
                            async move {
                                let (status, value) = if request.uri().path() == "/catalog" {
                                    (
                                        StatusCode::OK,
                                        json!([{"definition":{"kind":"verb","scope":"org/a/site/b","owner":"owner","id":"clock/read","revision":"v1"},"source_hash":"a".repeat(64),"graph_hash":"b".repeat(64)}]),
                                    )
                                } else {
                                    seen.fetch_add(1, Ordering::SeqCst);
                                    let raw =
                                        request.into_body().collect().await.unwrap().to_bytes();
                                    let call: Value = serde_json::from_slice(&raw).unwrap();
                                    assert_eq!(call["owner"], "owner");
                                    assert_eq!(call["source_sha256"], "a".repeat(64));
                                    assert_eq!(call["graph_sha256"], "b".repeat(64));
                                    assert_eq!(call["arguments_json"], r#"{"text":"<private>"}"#);
                                    (StatusCode::FORBIDDEN, json!({"error":"refused"}))
                                };
                                Ok::<_, std::convert::Infallible>(
                                    hyper::Response::builder()
                                        .status(status)
                                        .body(Full::new(Bytes::from(value.to_string())))
                                        .unwrap(),
                                )
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let mut bridge = Bridge::new(path.clone()).unwrap();
        bridge.uid = unsafe { libc::geteuid() };
        let make_call = |id: &str| Call {
            scope: "org/a/site/b".into(),
            capability_id: id.into(),
            call_id: uuid::Uuid::new_v4().to_string(),
            arguments: json!({"text":"<private>"}),
        };
        assert!(bridge.call(make_call("unlisted")).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(bridge.call(make_call("clock/read")).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn feedback_uses_fixed_contract_and_never_retries() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (status, reply, accepted) in [
            (StatusCode::OK, r#"{"version":1,"expires_in":60,"grant":"hm_feedback_header.payload.signature"}"#.to_owned(), true),
            (StatusCode::FORBIDDEN, "private upstream refusal".to_owned(), false),
            (StatusCode::FOUND, "redirect".to_owned(), false),
            (StatusCode::OK, "not json".to_owned(), false),
            (StatusCode::OK, "x".repeat(4097), false),
            (StatusCode::OK, r#"{"version":1,"expires_in":60,"grant":"hm_msi_secret"}"#.to_owned(), false),
        ] {
            let path = std::env::temp_dir().join(format!("v-{}.s", uuid::Uuid::new_v4().simple()));
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let seen = count.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    let seen = seen.clone();
                    let reply = reply.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                            let seen = seen.clone();
                            let reply = reply.clone();
                            async move {
                                seen.fetch_add(1, Ordering::SeqCst);
                                assert_eq!(request.method(), "POST");
                                assert_eq!(request.uri().path(), "/feedback-grant");
                                assert!(request.headers().get("Authorization").is_none());
                                let raw = request.into_body().collect().await.unwrap().to_bytes();
                                assert_eq!(&raw[..], br#"{"version":1,"human_reports_opt_in":true}"#);
                                Ok::<_, std::convert::Infallible>(hyper::Response::builder().status(status)
                                    .body(Full::new(Bytes::from(reply))).unwrap())
                            }
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service).await;
                    });
                }
            });
            let mut bridge = Bridge::new(path.clone()).unwrap();
            bridge.uid = unsafe { libc::geteuid() };
            assert!(bridge.feedback_grant(false).await.is_err());
            assert_eq!(count.load(Ordering::SeqCst), 0);
            let result = bridge.feedback_grant(true).await;
            assert_eq!(result.is_ok(), accepted);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            server.abort();
            let _ = server.await;
            std::fs::remove_file(path).unwrap();
        }
    }

}
