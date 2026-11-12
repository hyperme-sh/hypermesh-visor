//! Bounded, opt-in first-party crash reports. Never accept diagnostic text.
use std::cell::RefCell;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const QUEUE_LIMIT: usize = 16;
const SEND_TIMEOUT: Duration = Duration::from_secs(2);
static REPORTER: OnceLock<Reporter> = OnceLock::new();
thread_local! {
    static UPSTREAM: RefCell<Option<Upstream>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub enum Component {
    Daemon,
    Companion,
    InverseMcp,
}
impl Component {
    fn name(self) -> &'static str {
        match self {
            Self::Daemon => "visor",
            Self::Companion => "companion",
            Self::InverseMcp => "inverse_mcp",
        }
    }
}
#[derive(Clone, Copy)]
pub enum Failure {
    Panic,
    Startup,
    Serve,
    UpstreamHttp,
    UpstreamResponse,
}
impl Failure {
    fn name(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Startup => "startup",
            Self::Serve => "serve",
            Self::UpstreamHttp => "upstream_http",
            Self::UpstreamResponse => "upstream_response",
        }
    }
}
#[derive(Clone, Copy)]
pub enum Service {
    Supervisor,
    Conductor,
    Visor,
}
impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Conductor => "conductor",
            Self::Visor => "visor",
        }
    }
}
#[derive(Clone)]
struct Upstream {
    service: Service,
    version: Option<String>,
    color: Option<&'static str>,
    canary: Option<bool>,
}
pub struct UpstreamScope(Option<Upstream>);
impl UpstreamScope {
    pub fn enter(service: Service) -> Self {
        Self(UPSTREAM.with(|cell| {
            cell.replace(Some(Upstream {
                service,
                version: None,
                color: None,
                canary: None,
            }))
        }))
    }
    pub fn response(&self, response: &ureq::Response) {
        UPSTREAM.with(|cell| {
            if let Some(upstream) = cell.borrow_mut().as_mut() {
                upstream.version = response
                    .header("X-Hypermesh-Version")
                    .filter(|v| valid_version(v))
                    .map(str::to_owned);
                upstream.color = match response.header("X-Hypermesh-Release-Color") {
                    Some("blue") => Some("blue"),
                    Some("green") => Some("green"),
                    _ => None,
                };
                upstream.canary = match response.header("X-Hypermesh-Canary") {
                    Some("true") => Some(true),
                    Some("false") => Some(false),
                    _ => None,
                };
            }
        });
    }
}
impl Drop for UpstreamScope {
    fn drop(&mut self) {
        UPSTREAM.with(|cell| {
            cell.replace(self.0.take());
        });
    }
}

// Only numeric releases, known prerelease labels, and hexadecimal revisions.
// Opaque/free-form version strings are incomplete metadata, never telemetry text.
fn valid_version(value: &str) -> bool {
    if value.len() > 64 || value.is_empty() {
        return false;
    }
    if (7..=40).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    let (release, suffix) = value
        .split_once('-')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let parts: Vec<_> = release.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    suffix.is_none_or(|s| {
        s.split_once('.').is_some_and(|(name, number)| {
            ["alpha", "beta", "rc"].contains(&name)
                && !number.is_empty()
                && number.bytes().all(|c| c.is_ascii_digit())
        })
    })
}

struct Config {
    endpoint: String,
    key: String,
}
impl Config {
    fn parse(connection: &str) -> Result<Self, &'static str> {
        if connection.len() > 4096 {
            return Err("invalid telemetry configuration");
        }
        let mut key = None;
        let mut endpoint = None;
        for field in connection.split(';').filter(|v| !v.is_empty()) {
            let (name, value) = field
                .split_once('=')
                .ok_or("invalid telemetry configuration")?;
            match name.to_ascii_lowercase().as_str() {
                "instrumentationkey" if key.is_none() => key = Some(value.to_owned()),
                "ingestionendpoint" if endpoint.is_none() => endpoint = Some(value),
                "applicationid" | "liveendpoint" => {}
                _ => return Err("unsupported telemetry configuration"),
            }
        }
        let key = key.ok_or("missing telemetry key")?;
        if key.len() != 36
            || !key.bytes().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
            || key == "00000000-0000-0000-0000-000000000000"
        {
            return Err("invalid telemetry key");
        }
        let mut url = url::Url::parse(endpoint.unwrap_or("https://dc.services.visualstudio.com/"))
            .map_err(|_| "invalid telemetry endpoint")?;
        let host = url.host_str().ok_or("invalid telemetry endpoint")?;
        if url.scheme() != "https"
            || !(host == "dc.services.visualstudio.com"
                || host.ends_with(".in.applicationinsights.azure.com"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some_and(|p| p != 443)
            || url.path() != "/"
        {
            return Err("invalid telemetry endpoint");
        }
        url.set_path("/v2/track");
        Ok(Self {
            endpoint: url.into(),
            key,
        })
    }
}

enum Message {
    Report(Value),
    Flush(mpsc::SyncSender<()>),
}
struct Reporter {
    tx: mpsc::SyncSender<Message>,
    component: Component,
    version: &'static str,
    budget: Mutex<(u64, u32)>,
}
pub struct Guard(bool);
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        if let Some(reporter) = REPORTER.get() {
            let (tx, rx) = mpsc::sync_channel(1);
            if reporter.tx.try_send(Message::Flush(tx)).is_ok() {
                let _ = rx.recv_timeout(Duration::from_secs(3));
            }
        }
    }
}

pub fn init(component: Component, version: &'static str) -> Guard {
    let Ok(connection) = std::env::var("APPLICATIONINSIGHTS_CONNECTION_STRING") else {
        return Guard(false);
    };
    if !valid_version(version) {
        eprintln!("Visor crash reporting disabled: missing or unsupported release version");
        return Guard(false);
    }
    let config = match Config::parse(&connection) {
        Ok(config) => config,
        Err(_) => {
            eprintln!("Visor crash reporting disabled: invalid configuration");
            return Guard(false);
        }
    };
    let (tx, rx) = mpsc::sync_channel(QUEUE_LIMIT);
    if REPORTER
        .set(Reporter {
            tx,
            component,
            version,
            budget: Mutex::new((0, 0)),
        })
        .is_err()
    {
        return Guard(false);
    }
    let started = std::thread::Builder::new()
        .name("visor-crash-report".into())
        .spawn(move || {
            let agent = sender_agent();
            while let Ok(message) = rx.recv() {
                match message {
                    Message::Report(event) => {
                        let _ = send_event(&agent, &config, event);
                    }
                    Message::Flush(done) => {
                        let _ = done.try_send(());
                    }
                }
            }
        });
    if started.is_err() {
        return Guard(false);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(Failure::Panic);
        previous(info);
    }));
    Guard(true)
}

fn sender_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(SEND_TIMEOUT)
        .redirects(0)
        .try_proxy_from_env(false)
        .build()
}

fn send_event(agent: &ureq::Agent, config: &Config, mut event: Value) -> bool {
    event["iKey"] = json!(config.key);
    let Ok(body) = serde_json::to_vec(&vec![event]) else {
        return false;
    };
    // One attempt. No response body, credentials or diagnostics are retained.
    agent
        .post(&config.endpoint)
        .set("Content-Type", "application/json")
        .send_bytes(&body)
        .is_ok_and(|response| (200..300).contains(&response.status()))
}

pub fn report(failure: Failure) {
    let Some(reporter) = REPORTER.get() else {
        return;
    };
    let upstream = UPSTREAM
        .try_with(|cell| cell.try_borrow().ok().and_then(|v| v.clone()))
        .ok()
        .flatten();
    reporter.enqueue(failure, upstream.as_ref(), SystemTime::now());
}

impl Reporter {
    fn enqueue(&self, failure: Failure, upstream: Option<&Upstream>, now: SystemTime) {
        let Ok(elapsed) = now.duration_since(UNIX_EPOCH) else {
            return;
        };
        let Ok(mut budget) = self.budget.try_lock() else {
            return;
        };
        let minute = elapsed.as_secs() / 60;
        if budget.0 != minute {
            *budget = (minute, 0);
        }
        if budget.1 >= 8 {
            return;
        }
        budget.1 += 1;
        drop(budget);
        if let Some(event) = envelope(self.component, self.version, failure, upstream, now) {
            let _ = self.tx.try_send(Message::Report(event));
        }
    }
}

fn timestamp(now: SystemTime) -> Option<String> {
    let seconds: libc::time_t = now
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()?;
    let mut parts = std::mem::MaybeUninit::<libc::tm>::uninit();
    // gmtime_r writes only this local output and does not use shared C time state.
    let parts = unsafe {
        if libc::gmtime_r(&seconds, parts.as_mut_ptr()).is_null() {
            return None;
        }
        parts.assume_init()
    };
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        parts.tm_year + 1900,
        parts.tm_mon + 1,
        parts.tm_mday,
        parts.tm_hour,
        parts.tm_min,
        parts.tm_sec
    ))
}
fn envelope(
    component: Component,
    version: &str,
    failure: Failure,
    upstream: Option<&Upstream>,
    now: SystemTime,
) -> Option<Value> {
    if !valid_version(version) {
        return None;
    }
    let mut properties = json!({"schema_version":"1", "component":component.name(), "visor_version":version,
        "failure_kind":failure.name(), "upstream_attribution":"not_applicable"});
    if let Some(upstream) = upstream {
        properties["upstream_service"] = json!(upstream.service.name());
        properties["upstream_attribution"] =
            json!(if upstream.version.is_some() || upstream.color.is_some() {
                "complete"
            } else {
                "incomplete"
            });
        if let Some(version) = &upstream.version {
            properties["upstream_version"] = json!(version);
        }
        if let Some(color) = upstream.color {
            properties["upstream_color"] = json!(color);
        }
        if let Some(canary) = upstream.canary {
            properties["upstream_canary"] = json!(canary.to_string());
        }
    }
    Some(
        json!({"name":"Microsoft.ApplicationInsights.Exception", "time":timestamp(now)?, "sampleRate":100,
        "tags":{"ai.application.ver":version,"ai.cloud.role":"hypermesh-visor","ai.location.ip":"0.0.0.0"},
        "data":{"baseType":"ExceptionData","baseData":{"ver":2,"severityLevel":3,
            "exceptions":[{"id":1,"outerId":0,"typeName":failure.name(),"message":failure.name(),"hasFullStack":false}],
            "properties":properties}}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = "11111111-2222-4333-8444-555555555555";
    fn at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }
    #[test]
    fn version_is_required_and_free_form_values_are_never_accepted() {
        for version in [
            "",
            "person@example.com",
            "/home/person/private",
            "1.2.3-jane",
            "token-secret",
            "1.2",
            "1.2.3\n",
        ] {
            assert!(!valid_version(version));
            assert!(envelope(Component::Daemon, version, Failure::Panic, None, at()).is_none());
        }
        for version in ["0.1.0", "1.2.3-rc.4", "abcdef0123456789"] {
            assert!(valid_version(version));
        }
    }
    #[test]
    fn reports_have_only_declared_non_personal_fields() {
        let event = envelope(Component::Companion, "0.1.0", Failure::Panic, None, at()).unwrap();
        assert_eq!(event["time"], "2023-11-14T22:13:20Z");
        assert_eq!(
            event["tags"],
            json!({"ai.application.ver":"0.1.0","ai.cloud.role":"hypermesh-visor","ai.location.ip":"0.0.0.0"})
        );
        assert_eq!(event["data"]["baseType"], "ExceptionData");
        assert_eq!(
            event["data"]["baseData"]["exceptions"],
            json!([{"id":1,"outerId":0,"typeName":"panic","message":"panic","hasFullStack":false}])
        );
        assert_eq!(
            event["data"]["baseData"]["properties"],
            json!({"schema_version":"1","component":"companion","visor_version":"0.1.0","failure_kind":"panic","upstream_attribution":"not_applicable"})
        );
        assert!(event.to_string().len() < 1024);
    }
    #[test]
    fn upstream_metadata_is_scoped_and_invalid_metadata_is_incomplete() {
        let outer = UpstreamScope::enter(Service::Supervisor);
        let response: ureq::Response = "HTTP/1.1 500 Error\r\nX-Hypermesh-Version: 1.2.3\r\nX-Hypermesh-Release-Color: green\r\nX-Hypermesh-Canary: true\r\nAuthorization: secret\r\n\r\nprivate body".parse().unwrap();
        outer.response(&response);
        let value = UPSTREAM.with(|cell| {
            envelope(
                Component::Daemon,
                "0.1.0",
                Failure::UpstreamHttp,
                cell.borrow().as_ref(),
                at(),
            )
            .unwrap()
        });
        let props = &value["data"]["baseData"]["properties"];
        assert_eq!(props["upstream_version"], "1.2.3");
        assert_eq!(props["upstream_color"], "green");
        assert_eq!(props["upstream_canary"], "true");
        assert_eq!(props["upstream_attribution"], "complete");
        assert!(!value.to_string().contains("secret"));
        assert!(!value.to_string().contains("private body"));
        {
            let inner = UpstreamScope::enter(Service::Conductor);
            inner.response(&"HTTP/1.1 500 Error\r\nX-Hypermesh-Version: person@example.com\r\nX-Hypermesh-Release-Color: secret\r\n\r\n".parse().unwrap());
            let value = UPSTREAM.with(|cell| {
                envelope(
                    Component::Daemon,
                    "0.1.0",
                    Failure::Panic,
                    cell.borrow().as_ref(),
                    at(),
                )
                .unwrap()
            });
            let props = &value["data"]["baseData"]["properties"];
            assert_eq!(props["upstream_attribution"], "incomplete");
            assert!(props.get("upstream_version").is_none());
            assert!(props.get("upstream_color").is_none());
        }
        assert_eq!(
            UPSTREAM.with(|cell| cell.borrow().as_ref().unwrap().service.name()),
            "supervisor"
        );
        drop(outer);
        assert!(UPSTREAM.with(|cell| cell.borrow().is_none()));
    }
    #[test]
    fn metadata_does_not_cross_request_threads() {
        let _scope = UpstreamScope::enter(Service::Conductor);
        assert!(
            std::thread::spawn(|| UPSTREAM.with(|cell| cell.borrow().is_none()))
                .join()
                .unwrap()
        );
    }
    #[test]
    fn connection_requires_an_azure_https_origin_without_credentials_or_queries() {
        for origin in [
            "http://dc.services.visualstudio.com/",
            "https://dc.services.visualstudio.com.evil.example/",
            "https://user:secret@dc.services.visualstudio.com/",
            "https://dc.services.visualstudio.com/?secret=1",
            "https://dc.services.visualstudio.com/custom",
            "https://127.0.0.1/",
            "https://dc.services.visualstudio.com:444/",
        ] {
            assert!(Config::parse(&format!(
                "InstrumentationKey={KEY};IngestionEndpoint={origin}"
            ))
            .is_err());
        }
        let config = Config::parse(&format!("InstrumentationKey={KEY};IngestionEndpoint=https://westus2-0.in.applicationinsights.azure.com/")).unwrap();
        assert_eq!(
            config.endpoint,
            "https://westus2-0.in.applicationinsights.azure.com/v2/track"
        );
        assert_eq!(config.key, KEY);
        assert!(Config::parse(&format!(
            "InstrumentationKey={KEY};InstrumentationKey={KEY}"
        ))
        .is_err());
        assert!(Config::parse("InstrumentationKey=person@example.com").is_err());
    }
    #[test]
    fn collector_transport_uses_the_exception_envelope_and_never_follows_redirects() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        for status in [202, 307] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}/v2/track", listener.local_addr().unwrap());
            let redirected = TcpListener::bind("127.0.0.1:0").unwrap();
            redirected.set_nonblocking(true).unwrap();
            let destination = redirected.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                assert!(header.starts_with("POST /v2/track HTTP/1.1"));
                let size = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; size];
                socket.read_exact(&mut body).unwrap();
                write!(socket, "HTTP/1.1 {status} Response\r\nLocation: http://{destination}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                serde_json::from_slice::<Value>(&body).unwrap()
            });
            // Only this test injects an HTTP loopback endpoint. Production parses HTTPS Azure origins.
            let config = Config {
                endpoint,
                key: KEY.into(),
            };
            let event = envelope(Component::Daemon, "0.1.0", Failure::Serve, None, at()).unwrap();
            assert_eq!(send_event(&sender_agent(), &config, event), status == 202);
            let sent = server.join().unwrap();
            assert_eq!(sent.as_array().unwrap().len(), 1);
            assert_eq!(sent[0]["iKey"], KEY);
            assert_eq!(sent[0]["data"]["baseType"], "ExceptionData");
            assert_eq!(
                redirected.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn reporter_limits_each_minute_and_drops_when_the_sender_is_stalled() {
        let (tx, rx) = mpsc::sync_channel(QUEUE_LIMIT);
        let reporter = Reporter {
            tx,
            component: Component::Daemon,
            version: "0.1.0",
            budget: Mutex::new((0, 0)),
        };
        for _ in 0..20 {
            reporter.enqueue(Failure::Panic, None, at());
        }
        assert_eq!(rx.try_iter().count(), 8);
        for _ in 0..20 {
            reporter.enqueue(Failure::Panic, None, at());
        }
        assert_eq!(rx.try_iter().count(), 0);
        for minute in 1..=4 {
            for _ in 0..20 {
                reporter.enqueue(
                    Failure::Panic,
                    None,
                    at() + Duration::from_secs(60 * minute),
                );
            }
        }
        assert_eq!(rx.try_iter().count(), QUEUE_LIMIT);
    }
}
