//! GlitchTip DSN configuration and an approval-bound, synchronous Sentry sender.
use crate::{
    crash_report::PendingCrash,
    health_consent::{Scope, Store},
};
use hypermesh_local_storage as storage;
use sentry_core::{types::Dsn, Client, ClientOptions, Envelope, Transport};
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use zeroize::Zeroizing;

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "crash configuration or upload refused",
    )
}

/// Provisioned private config. No Debug implementation: the DSN is a credential.
pub struct Config {
    dsn: Dsn,
    endpoint: String,
    pub scope: Scope,
    owner_socket: Option<std::path::PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    dsn: String,
    scope: Scope,
    #[serde(default)]
    owner_socket: Option<std::path::PathBuf>,
}
impl Config {
    /// Missing config disables crash reporting. Never log the file or parse errors.
    pub fn read(path: &Path) -> io::Result<Option<Self>> {
        storage::check_dir(path.parent().ok_or_else(refused)?)?;
        let file = match storage::open(path, storage::Open::Read { private: true }) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(refused()),
        };
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| refused())?;
        if bytes.len() > 16384 {
            return Err(refused());
        }
        let mut config: FileConfig = serde_json::from_slice(&bytes).map_err(|_| refused())?;
        let secret = Zeroizing::new(std::mem::take(&mut config.dsn));
        let mut parsed = Self::parse(&secret, config.scope)?;
        if let Some(path) = &config.owner_socket {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(refused());
            }
        }
        parsed.owner_socket = config.owner_socket;
        Ok(Some(parsed))
    }
    fn parse(value: &str, scope: Scope) -> io::Result<Self> {
        if value.len() > 2048 {
            return Err(refused());
        }
        let url = url::Url::parse(value).map_err(|_| refused())?;
        if url.query().is_some()
            || url.fragment().is_some()
            || url.password().is_some()
            || url.username().is_empty()
            || !url
                .username()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(refused());
        }
        let dsn: Dsn = value.parse().map_err(|_| refused())?;
        let endpoint = crate::crash_report::destination_url(dsn.envelope_api_url().as_str())?;
        // TLS is required outside loopback. A private IP alone does not prove
        // that a request uses WireGuard, so do not infer transport security.
        if url.scheme() == "http"
            && !match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            }
        {
            return Err(refused());
        }
        Ok(Self {
            dsn,
            endpoint,
            scope,
            owner_socket: None,
        })
    }
    /// Safe for the local preview: contains no DSN key or authentication header.
    pub fn destination(&self) -> &str {
        &self.endpoint
    }

    /// Send only a consumed, individually approved report. The SDK has no
    /// integrations or background transport, and never receives a dump path.
    pub fn send(&self, report: PendingCrash, store: &Store) -> io::Result<()> {
        self.send_registered(report, store, |claim| {
            let bridge =
                crate::host_bridge::Bridge::new(self.owner_socket.clone().ok_or_else(refused)?)
                    .map_err(|_| refused())?;
            // Startup callers may already own a Tokio runtime. The bounded local
            // request uses a separate thread, and credentials never enter Visor.
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| refused())?;
                runtime
                    .block_on(bridge.crash_register(claim))
                    .map_err(|_| refused())
            })
            .join()
            .map_err(|_| refused())?
        })
    }
    fn send_registered(
        &self,
        report: PendingCrash,
        store: &Store,
        register: impl FnOnce(serde_json::Value) -> io::Result<serde_json::Value>,
    ) -> io::Result<()> {
        if !report.consent_ticket().matches_scope(&self.scope) {
            return Err(refused());
        }
        report.send_once(store, |destination, body| {
            if destination != self.endpoint || body.len() > 65536 {
                return Err(refused());
            }
            let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| refused())?;
            let event_id = value
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(refused)?;
            use sha2::{Digest, Sha256};
            let captured = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .ok_or_else(refused)?;
            let hash = format!("{:x}", Sha256::digest(body));
            let fact = register(serde_json::json!({"version":1,"event_id":event_id,
                "captured_at":captured,"payload_sha256":hash}))?;
            let project: u64 = self
                .dsn
                .project_id()
                .to_string()
                .parse()
                .map_err(|_| refused())?;
            if fact["version"] != 1
                || fact["registered"] != true
                || fact["tenant"] != self.scope.tenant.to_string()
                || fact["subject"] != self.scope.subject.to_string()
                || fact["device"] != self.scope.device.to_string()
                || fact["project"].as_u64() != Some(project)
                || fact["event_id"] != event_id
                || fact["captured_at"] != captured
                || fact["payload_sha256"] != hash
            {
                return Err(refused());
            }
            let mut bytes = serde_json::to_vec(&serde_json::json!({"event_id": event_id}))
                .map_err(|_| refused())?;
            bytes.push(b'\n');
            bytes.extend(
                serde_json::to_vec(&serde_json::json!({"type":"event", "length":body.len()}))
                    .map_err(|_| refused())?,
            );
            bytes.push(b'\n');
            bytes.extend_from_slice(body);
            bytes.push(b'\n');
            // Preserve the exact approved JSON; capture_event would enrich it.
            let envelope = Envelope::from_bytes_raw(bytes).map_err(|_| refused())?;
            let transport = Arc::new(OnceTransport {
                endpoint: self.endpoint.clone(),
                auth: self
                    .dsn
                    .to_auth(Some(concat!("hypermesh-visor/", env!("CARGO_PKG_VERSION"))))
                    .to_string(),
                attempted: AtomicBool::new(false),
                result: Mutex::new(None),
            });
            let client = Client::with_options(ClientOptions {
                dsn: Some(self.dsn.clone()),
                transport: Some(Arc::new(transport.clone())),
                default_integrations: false,
                ..Default::default()
            });
            client.send_envelope(envelope);
            let result = transport
                .result
                .lock()
                .map_err(|_| refused())?
                .take()
                .ok_or_else(refused)?;
            result
        })
    }
}
struct OnceTransport {
    endpoint: String,
    auth: String,
    attempted: AtomicBool,
    result: Mutex<Option<io::Result<()>>>,
}
impl Transport for OnceTransport {
    fn send_envelope(&self, envelope: Envelope) {
        if self.attempted.swap(true, Ordering::AcqRel) {
            return;
        }
        let result = (|| {
            let mut bytes = Vec::new();
            envelope.to_writer(&mut bytes).map_err(|_| refused())?;
            let response = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(5))
                .redirects(0)
                .try_proxy_from_env(false)
                .build()
                .post(&self.endpoint)
                .set("X-Sentry-Auth", &self.auth)
                .set("Content-Type", "application/x-sentry-envelope")
                .send_bytes(&bytes)
                .map_err(|_| refused())?;
            if !(200..300).contains(&response.status()) {
                return Err(refused());
            }
            Ok(())
        })();
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Scope {
        Scope {
            subject: uuid::Uuid::new_v4(),
            tenant: uuid::Uuid::new_v4(),
            device: uuid::Uuid::new_v4(),
            notice_version: uuid::Uuid::new_v4(),
            purpose_version: uuid::Uuid::new_v4(),
        }
    }
    #[test]
    fn registration_failure_and_wrong_scope_never_upload() {
        use crate::health_consent::Category;
        let config = Config::parse("https://fixture_key@unreachable.invalid/42", scope()).unwrap();
        let dir = std::env::temp_dir().join(format!("visor-registration-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&dir, config.scope.clone()).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        for bad in ["subject", "device", "project", "payload_sha256"] {
            let mut report =
                PendingCrash::capture(&store, config.destination(), &serde_json::json!({}))
                    .unwrap()
                    .unwrap();
            let preview = report.preview();
            let (id, hash, destination) = (
                preview.id,
                preview.sha256.to_owned(),
                preview.destination.to_owned(),
            );
            report.approve(id, &hash, &destination).unwrap();
            assert!(config
                .send_registered(report, &store, |mut claim| {
                    claim["registered"] = true.into();
                    claim["tenant"] = config.scope.tenant.to_string().into();
                    claim["subject"] = config.scope.subject.to_string().into();
                    claim["device"] = config.scope.device.to_string().into();
                    claim["project"] = 42.into();
                    claim[bad] = "wrong".into();
                    Ok(claim)
                })
                .is_err());
        }
        let mut report =
            PendingCrash::capture(&store, config.destination(), &serde_json::json!({}))
                .unwrap()
                .unwrap();
        let preview = report.preview();
        let (id, hash, destination) = (
            preview.id,
            preview.sha256.to_owned(),
            preview.destination.to_owned(),
        );
        report.approve(id, &hash, &destination).unwrap();
        assert!(config.send(report, &store).is_err()); // Missing owner socket fails closed.
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn dsn_is_ingest_only_and_preview_omits_credentials() {
        let config = Config::parse(
            "https://fixture_key@glitchtip.example.invalid/prefix/42",
            scope(),
        )
        .unwrap();
        assert_eq!(
            config.destination(),
            "https://glitchtip.example.invalid/prefix/api/42/envelope/"
        );
        assert!(!config.destination().contains("fixture_key"));
        for value in [
            "https://fixture_key@sentry.io/42",
            "https://fixture_key@o1.ingest.sentry.io./42",
            "https://fixture_key:secret@host.invalid/42",
            "https://fixture_key@host.invalid/42?secret=x",
            "https://fixture_key@host.invalid/42#x",
            "http://fixture_key@10.0.0.1/42",
            "https://host.invalid/42",
            "https://key%0a@host.invalid/42",
        ] {
            assert!(Config::parse(value, scope()).is_err());
        }
    }
    #[test]
    fn private_config_is_bounded_and_errors_do_not_expose_credentials() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("visor-config-{}", uuid::Uuid::new_v4()));
        storage::create_dir(&dir).unwrap();
        let path = dir.join("crash-reporting.json");
        assert!(Config::read(&path).unwrap().is_none());
        let bytes = serde_json::to_vec(&serde_json::json!({
            "dsn":"https://fixture_key@glitchtip.example.invalid/42", "scope":scope()
        }))
        .unwrap();
        storage::open(&path, storage::Open::New)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(Config::read(&path).unwrap().is_some());
        std::fs::remove_file(&path).unwrap();
        storage::open(&path, storage::Open::New)
            .unwrap()
            .write_all(b"fixture_key invalid config")
            .unwrap();
        let error = Config::read(&path).err().unwrap();
        assert!(!error.to_string().contains("fixture_key"));
        std::fs::remove_file(&path).unwrap();
        storage::open(&path, storage::Open::New)
            .unwrap()
            .set_len(16385)
            .unwrap();
        assert!(Config::read(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    #[ignore = "CI-only fake Sentry transport contract"]
    fn fake_sentry_endpoint_contract() {
        use crate::health_consent::Category;
        use std::{io::Write, net::TcpListener};
        for status in [200, 302, 429, 500] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let config = Config::parse(&format!("http://fixture_key@{addr}/42"), scope()).unwrap();
            let dir = std::env::temp_dir().join(format!("visor-upload-{}", uuid::Uuid::new_v4()));
            let store = Store::open(&dir, config.scope.clone()).unwrap();
            assert!(
                PendingCrash::capture(&store, config.destination(), &serde_json::json!({}))
                    .unwrap()
                    .is_none()
            );
            store.decide(Category::Crashes, true).unwrap();
            let unapproved =
                PendingCrash::capture(&store, config.destination(), &serde_json::json!({}))
                    .unwrap()
                    .unwrap();
            assert!(config.send(unapproved, &store).is_err());
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            let mut report = PendingCrash::capture(
                &store,
                config.destination(),
                &serde_json::json!({"signal":11,"prompt":"excluded-secret"}),
            )
            .unwrap()
            .unwrap();
            let preview = report.preview();
            let expected = preview.sanitized_json.to_vec();
            let (id, hash, destination) = (
                preview.id,
                preview.sha256.to_owned(),
                preview.destination.to_owned(),
            );
            report.approve(id, &hash, &destination).unwrap();
            let server = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => panic!("fake endpoint: {e}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let end = loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break bytes.len();
                    }
                    assert!(bytes.len() < 8192);
                };
                let headers = String::from_utf8(bytes.clone())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(headers.starts_with("post /api/42/envelope/ http/1.1\r\n"));
                assert!(headers.contains("x-sentry-auth: sentry "));
                assert!(headers.contains("sentry_key=fixture_key"));
                let length: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(length < 65536);
                bytes.resize(end + length, 0);
                socket.read_exact(&mut bytes[end..]).unwrap();
                let body = &bytes[end..];
                let prefix_end = body
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'\n')
                    .nth(1)
                    .unwrap()
                    .0
                    + 1;
                assert_eq!(&body[prefix_end..body.len() - 1], expected);
                assert!(!String::from_utf8_lossy(body).contains("excluded-secret"));
                assert!(!String::from_utf8_lossy(body).contains("fixture_key"));
                write!(socket, "HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nLocation: http://127.0.0.1:1/forbidden\r\nConnection: close\r\n\r\n").unwrap();
            });
            assert_eq!(
                config
                    .send_registered(report, &store, |mut claim| {
                        claim["registered"] = true.into();
                        claim["tenant"] = config.scope.tenant.to_string().into();
                        claim["subject"] = config.scope.subject.to_string().into();
                        claim["device"] = config.scope.device.to_string().into();
                        claim["project"] = 42.into();
                        Ok(claim)
                    })
                    .is_ok(),
                status == 200
            );
            server.join().unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
