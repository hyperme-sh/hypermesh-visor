//! Host-only health worker. The ordinary Visor daemon never selects this policy.
use crate::{
    health_consent::Category,
    health_metrics::{self, Metric},
    health_os,
};
use serde::Deserialize;
use std::{
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

fn refused() -> io::Error {
    io::Error::other("host health unavailable")
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    device_id: Uuid,
    tenant_id: Uuid,
    owner_socket: PathBuf,
    owner_token_file: PathBuf,
    collector_url: String,
    collector_authorization_file: PathBuf,
}
#[derive(Deserialize, PartialEq, Eq)]
struct Authority {
    version: u8,
    policy: String,
    device_id: Uuid,
    tenant_id: Uuid,
    terms_version: String,
    terms_sha256: String,
}
impl Authority {
    fn valid(&self) -> bool {
        self.version == 1
            && self.policy == "host-agreement"
            && !self.device_id.is_nil()
            && !self.tenant_id.is_nil()
            && !self.terms_version.is_empty()
            && self.terms_version.len() <= 128
            && self.terms_sha256.len() == 64
            && self.terms_sha256.bytes().all(|c| c.is_ascii_hexdigit())
    }
}

// Root ownership is part of the boundary: an ordinary Visor user's socket or
// config cannot assert host agreement authority, even with the shared library.
fn root_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(refused());
    }
    for ancestor in path.ancestors() {
        let m = std::fs::symlink_metadata(ancestor).map_err(|_| refused())?;
        if m.file_type().is_symlink() || m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(refused());
        }
    }
    Ok(())
}
fn private_bytes(path: &Path, limit: u64) -> io::Result<Zeroizing<Vec<u8>>> {
    root_path(path)?;
    let file =
        hypermesh_local_storage::open(path, hypermesh_local_storage::Open::Read { private: true })
            .map_err(|_| refused())?;
    if !file.metadata()?.is_file() {
        return Err(refused());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| refused())?;
    if bytes.len() as u64 > limit {
        return Err(refused());
    }
    Ok(bytes)
}
fn credential(path: &Path) -> io::Result<Zeroizing<String>> {
    let bytes = private_bytes(path, 8192)?;
    let value = std::str::from_utf8(&bytes).map_err(|_| refused())?.trim();
    if value.is_empty() || value.bytes().any(|b| !(0x20..=0x7e).contains(&b)) {
        return Err(refused());
    }
    Ok(Zeroizing::new(value.to_owned()))
}
fn collector_url(value: &str) -> io::Result<url::Url> {
    let url = url::Url::parse(value).map_err(|_| refused())?;
    let loopback = matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/v1/metrics"
    {
        return Err(refused());
    }
    Ok(url)
}
fn connect_owner(path: &Path, deadline: Duration) -> io::Result<UnixStream> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| refused())?;
    let stream = runtime.block_on(async {
        tokio::time::timeout(deadline, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())
    })?;
    let stream = stream.into_std().map_err(|_| refused())?;
    stream.set_nonblocking(false).map_err(|_| refused())?;
    Ok(stream)
}

fn authorize(config: &Config) -> io::Result<Authority> {
    root_path(&config.owner_socket)?;
    let meta = std::fs::symlink_metadata(&config.owner_socket)?;
    if !meta.file_type().is_socket() || meta.mode() & 0o077 != 0 {
        return Err(refused());
    }
    let mut stream = connect_owner(&config.owner_socket, Duration::from_secs(7))?;
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut peer as *mut _ as *mut _,
            &mut len,
        )
    };
    if rc != 0 || peer.uid != 0 {
        return Err(refused());
    }
    stream.set_read_timeout(Some(Duration::from_secs(7)))?;
    stream.set_write_timeout(Some(Duration::from_secs(7)))?;
    let token = credential(&config.owner_token_file)?;
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(refused());
    }
    let request = Zeroizing::new(format!("GET /v0/health_authorization HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", token.as_str()));
    stream
        .write_all(request.as_bytes())
        .map_err(|_| refused())?;
    let mut response = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(refused)?;
        stream.set_read_timeout(Some(left)).map_err(|_| refused())?;
        let mut chunk = [0u8; 1024];
        let count = stream.read(&mut chunk).map_err(|_| refused())?;
        if count == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..count]);
        if response.len() > 8192 {
            return Err(refused());
        }
    }
    let auth = decode_authority(&response)?;
    if auth.device_id != config.device_id || auth.tenant_id != config.tenant_id {
        return Err(refused());
    }
    Ok(auth)
}
fn decode_authority(bytes: &[u8]) -> io::Result<Authority> {
    if bytes.len() > 8192 {
        return Err(refused());
    }
    let split = bytes
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .ok_or_else(refused)?;
    let headers = std::str::from_utf8(&bytes[..split]).map_err(|_| refused())?;
    if !headers
        .lines()
        .next()
        .is_some_and(|s| s == "HTTP/1.1 200 OK" || s == "HTTP/1.0 200 OK")
        || headers
            .lines()
            .any(|s| s.to_ascii_lowercase().starts_with("transfer-encoding:"))
    {
        return Err(refused());
    }
    let auth: Authority = serde_json::from_slice(&bytes[split + 4..]).map_err(|_| refused())?;
    if !auth.valid() {
        return Err(refused());
    }
    Ok(auth)
}
fn cycle(
    mut authorize: impl FnMut() -> io::Result<Authority>,
    sample: impl FnOnce() -> io::Result<Vec<Metric>>,
    send: impl FnOnce(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let began = Instant::now();
    let first = authorize()?;
    if !first.valid() {
        return Err(refused());
    }
    let body = health_metrics::encode(Category::OsHealth, sample()?, "hypermesh-host")?;
    let second = authorize()?;
    if first != second || began.elapsed() >= Duration::from_secs(30) {
        return Err(refused());
    }
    send(&body)
}
fn send(config: &Config, body: &[u8]) -> io::Result<()> {
    let endpoint = collector_url(&config.collector_url)?;
    let authorization = credential(&config.collector_authorization_file)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(10))
        .build();
    let response = agent
        .post(endpoint.as_str())
        .set("Content-Type", "application/json")
        .set("Authorization", authorization.as_str())
        .send_bytes(body)
        .map_err(|_| refused())?;
    if response.status() != 200 {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| refused())?;
    if bytes.len() > 4096 {
        return Err(refused());
    }
    if !bytes.is_empty() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| refused())?;
        if value.get("partialSuccess").is_some() {
            return Err(refused());
        }
    }
    Ok(())
}
/// Separate headless entry point; never called from the Visor consent flow.
pub fn run(path: &Path, once: bool) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(refused());
    }
    loop {
        // Reload private configuration and credentials for every sample, so
        // deployment rotation needs no cached credential or persisted queue.
        let result = (|| {
            let config: Config =
                serde_json::from_slice(&private_bytes(path, 16384)?).map_err(|_| refused())?;
            collector_url(&config.collector_url)?;
            cycle(
                || authorize(&config),
                health_os::sample,
                |body| send(&config, body),
            )
        })();
        if once {
            return result;
        }
        if result.is_err() {
            eprintln!("host health sample unavailable");
        }
        // A later cycle collects a new sample; failed payloads are never retried.
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authority() -> Authority {
        Authority {
            version: 1,
            policy: "host-agreement".into(),
            device_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            terms_version: "test".into(),
            terms_sha256: "a".repeat(64),
        }
    }
    #[test]
    fn missing_owner_socket_fails_within_connection_deadline() {
        let start = Instant::now();
        let path = std::env::temp_dir().join(format!("absent-host-owner-{}", Uuid::new_v4()));
        assert!(connect_owner(&path, Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn authorization_precedes_sampling_and_revocation_discards_the_sample() {
        assert!(cycle(
            || Err(refused()),
            || panic!("unauthorized source read"),
            |_| panic!("unauthorized send")
        )
        .is_err());
        let mut calls = 0;
        assert!(cycle(
            || {
                calls += 1;
                if calls == 1 {
                    Ok(authority())
                } else {
                    Err(refused())
                }
            },
            || Ok(vec![Metric::MemoryPercent(20)]),
            |_| panic!("revoked send")
        )
        .is_err());
        assert_eq!(calls, 2);
    }
    #[test]
    fn changed_machine_or_terms_cannot_send_old_sample() {
        for changed in 0..3 {
            let mut calls = 0;
            assert!(cycle(
                || {
                    calls += 1;
                    let mut auth = authority();
                    if calls == 2 {
                        match changed {
                            0 => auth.device_id = Uuid::new_v4(),
                            1 => auth.tenant_id = Uuid::new_v4(),
                            _ => auth.terms_sha256 = "b".repeat(64),
                        }
                    }
                    Ok(auth)
                },
                || Ok(vec![Metric::CpuPercent(42)]),
                |_| panic!("changed authority sent")
            )
            .is_err());
        }
    }
    #[test]
    fn authorized_host_uses_shared_projection_without_local_opt_in() {
        let mut calls = 0;
        cycle(
            || {
                calls += 1;
                Ok(authority())
            },
            || Ok(vec![Metric::CpuPercent(42)]),
            |body| {
                let text = std::str::from_utf8(body).unwrap();
                assert!(text.contains("hypermesh-host"));
                assert!(text.contains("visor.os.cpu.utilization"));
                assert!(!text.contains("tenant_id"));
                assert!(!text.contains("device_id"));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
    }
    #[test]
    fn wrong_policy_and_invalid_collector_are_refused() {
        let mut auth = authority();
        auth.policy = "visor-local-consent".into();
        assert!(!auth.valid());
        for url in [
            "http://collector.test/v1/metrics",
            "https://user:secret@collector.test/v1/metrics",
            "https://collector.test/v1/metrics?token=x",
            "https://collector.test/other",
        ] {
            assert!(collector_url(url).is_err());
        }
        assert!(collector_url("https://collector.test/v1/metrics").is_ok());
    }
    #[test]
    fn private_reply_requires_success_and_valid_scoped_authority() {
        let body = format!(
            r#"{{"version":1,"policy":"host-agreement","device_id":"{}","tenant_id":"{}","terms_version":"test","terms_sha256":"{}","checked_at":"now"}}"#,
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            "a".repeat(64)
        );
        assert!(decode_authority(
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}").as_bytes()
        )
        .is_ok());
        assert!(
            decode_authority(format!("HTTP/1.1 403 Forbidden\r\n\r\n{body}").as_bytes()).is_err()
        );
        assert!(decode_authority(
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{body}").as_bytes()
        )
        .is_err());
        assert!(decode_authority(&vec![b'a'; 8193]).is_err());
    }
}
