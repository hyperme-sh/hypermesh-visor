//! Sanitized crash previews and one-report approval. No raw dump or SDK transport.
use crate::health_consent::{Category, Store, Ticket};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const APPROVAL_TTL: Duration = Duration::from_secs(300);
const REPORT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Owned pending report. Not Clone or serializable: approvals never survive restart.
pub struct PendingCrash {
    id: Uuid,
    destination: String,
    body: Vec<u8>,
    digest: String,
    ticket: Ticket,
    approved: Option<Instant>,
    captured: Instant,
    age_at_capture: Duration,
}
pub struct Preview<'a> {
    pub id: Uuid,
    pub destination: &'a str,
    pub sanitized_json: &'a [u8],
    pub sha256: &'a str,
}
fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "crash upload not approved")
}

impl PendingCrash {
    /// `destination` comes from trusted machine configuration, never crash data.
    /// Only an independently consented crash can enter the preview path. The
    /// future capture adapter must also check consent before creating a dump.
    pub fn capture(
        store: &Store,
        destination: &str,
        native_event: &Value,
    ) -> io::Result<Option<Self>> {
        let Some(ticket) = store.capture(Category::Crashes)? else {
            return Ok(None);
        };
        // Never copy SDK messages, stacks, contexts, attachments or breadcrumbs.
        // Only a fixed error-code mapping crosses this boundary.
        let code = match native_event.get("signal").and_then(Value::as_i64) {
            Some(6) => "abort",
            Some(11) => "invalid_memory_access",
            Some(4) => "illegal_instruction",
            Some(8) => "arithmetic_fault",
            _ => "native_crash",
        };
        Self::from_code(ticket, destination, code).map(Some)
    }
    /// Reduce and delete a completed local dump before constructing its preview.
    /// The adapter supplies the ticket captured before starting the dump writer.
    /// Never acquire a new ticket here: re-enabling consent cannot revive a dump.
    pub fn from_dump(
        store: &Store,
        ticket: Ticket,
        destination: &str,
        directory: &std::path::Path,
        id: Uuid,
    ) -> io::Result<Self> {
        let dump = crate::crash_dump::reduce(directory, id)?;
        store.validate_capture(&ticket)?;
        use crate::crash_dump::CrashCode;
        Self::from_code_at(
            ticket,
            destination,
            match dump.code {
                CrashCode::InvalidMemoryAccess => "invalid_memory_access",
                CrashCode::IllegalInstruction => "illegal_instruction",
                CrashCode::ArithmeticFault => "arithmetic_fault",
                CrashCode::NativeCrash => "native_crash",
            },
            dump.captured_at,
        )
    }
    fn from_code(ticket: Ticket, destination: &str, code: &str) -> io::Result<Self> {
        Self::from_code_at(ticket, destination, code, SystemTime::now())
    }
    fn from_code_at(
        ticket: Ticket,
        destination: &str,
        code: &str,
        captured_at: SystemTime,
    ) -> io::Result<Self> {
        let age = SystemTime::now()
            .duration_since(captured_at)
            .map_err(|_| denied())?;
        if age >= REPORT_TTL {
            return Err(denied());
        }
        let destination = destination_url(destination)?;
        let id = Uuid::new_v4();
        let tenant = ticket.crash_tenant().ok_or_else(denied)?;
        let body = serde_json::to_vec(&json!({
            "event_id": id.simple().to_string(),
            "timestamp": captured_at.duration_since(UNIX_EPOCH).map_err(|_| denied())?.as_secs(),
            "platform": "native", "level": "fatal",
            "release": concat!("hypermesh-visor@", env!("CARGO_PKG_VERSION")),
            "message": "Visor terminated unexpectedly",
            "exception": {"values": [{"type": code}]},
            "tags": {"tenant_id": tenant.to_string()}
        }))
        .map_err(|_| denied())?;
        let digest = format!("{:x}", Sha256::digest(&body));
        Ok(Self {
            id,
            destination,
            body,
            digest,
            ticket,
            approved: None,
            captured: Instant::now(),
            age_at_capture: age,
        })
    }
    pub fn preview(&self) -> Preview<'_> {
        Preview {
            id: self.id,
            destination: &self.destination,
            sanitized_json: &self.body,
            sha256: &self.digest,
        }
    }
    /// The visible local dialog calls this only after explicit approval of this
    /// exact preview. Enrollment, category opt-in and server replies cannot call it.
    pub fn approve(&mut self, id: Uuid, sha256: &str, destination: &str) -> io::Result<()> {
        self.approved = None;
        if id != self.id
            || sha256 != self.digest
            || destination != self.destination
            || self.age_at_capture.saturating_add(self.captured.elapsed()) >= REPORT_TTL
        {
            return Err(denied());
        }
        self.approved = Some(Instant::now());
        Ok(())
    }
    pub fn consent_ticket(&self) -> &Ticket {
        &self.ticket
    }
    /// Consumes the report even on failure/unknown outcome: never retry silently.
    /// The adapter must authenticate the machine, refuse redirects and proxies,
    /// and use a short synchronous timeout. It receives only approved JSON.
    pub fn send_once(
        self,
        store: &Store,
        send: impl FnOnce(&str, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.age_at_capture.saturating_add(self.captured.elapsed()) >= REPORT_TTL
            || self.approved.is_none_or(|at| at.elapsed() >= APPROVAL_TTL)
        {
            return Err(denied());
        }
        store.upload_local_crash(&self.ticket, || send(&self.destination, &self.body))
    }
    /// Declining destroys the pending sanitized report and its approval.
    pub fn decline(self) {}
}

pub(crate) fn destination_url(value: &str) -> io::Result<String> {
    if value.len() > 2048 {
        return Err(denied());
    }
    let url = url::Url::parse(value).map_err(|_| denied())?;
    let host = url.host_str().ok_or_else(denied)?.trim_end_matches('.');
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || host == "sentry.io"
        || host.ends_with(".sentry.io")
        || host == "sentry.dev"
        || host.ends_with(".sentry.dev")
    {
        return Err(denied());
    }
    // HTTP is restricted to a numeric local/mesh address. The transport must
    // independently verify it is on the configured WireGuard interface.
    let local_http = url.scheme() == "http"
        && match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
            _ => false,
        };
    if url.scheme() != "https" && !local_http {
        return Err(denied());
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::Scope;
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Temp, Store) {
        let dir = Temp(std::env::temp_dir().join(format!("visor-report-{}", Uuid::new_v4())));
        let store = Store::open(
            &dir.0,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap();
        (dir, store)
    }
    const DEST: &str = "https://crashes.example.invalid/api/1/store/";
    fn pending(store: &Store) -> PendingCrash {
        PendingCrash::capture(store, DEST, &json!({"signal":11}))
            .unwrap()
            .unwrap()
    }
    fn approve(report: &mut PendingCrash) {
        report
            .approve(
                report.id,
                &report.digest.clone(),
                &report.destination.clone(),
            )
            .unwrap();
    }
    #[test]
    fn category_off_discards_and_opt_in_does_not_approve_upload() {
        let (_dir, store) = fixture();
        assert!(PendingCrash::capture(&store, DEST, &json!({}))
            .unwrap()
            .is_none());
        store.decide(Category::Crashes, true).unwrap();
        let report = pending(&store);
        store.synchronized(report.consent_ticket()).unwrap();
        assert!(report
            .send_once(&store, |_, _| panic!("not approved"))
            .is_err());
    }
    #[test]
    fn sanitized_preview_drops_hostile_sdk_fields_and_preserves_tenant() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let report = PendingCrash::capture(
            &store,
            DEST,
            &json!({
                "signal": 11, "prompt":"secret-prompt", "message":"secret-message",
                "file_contents":"secret-file", "disk_serial":"secret-serial",
                "attachments":["raw-dump"], "breadcrumbs":["secret-cookie"],
                "contexts":{"user":"secret-person"}, "exception":{"stacktrace":"secret-path"}
            }),
        )
        .unwrap()
        .unwrap();
        let preview = String::from_utf8(report.body.clone()).unwrap();
        assert!(!preview.contains("secret-"));
        assert!(!preview.contains("raw-dump"));
        let event: Value = serde_json::from_slice(&report.body).unwrap();
        assert_eq!(
            event["tags"]["tenant_id"],
            report.ticket.crash_tenant().unwrap().to_string()
        );
        assert_eq!(event["tags"].as_object().unwrap().len(), 1);
        assert_eq!(
            event["exception"]["values"][0]["type"],
            "invalid_memory_access"
        );
    }
    #[test]
    fn approval_binds_one_report_body_and_destination() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        let other = pending(&store);
        assert!(report
            .approve(other.id, &report.digest.clone(), DEST)
            .is_err());
        assert!(report.approve(report.id, "wrong", DEST).is_err());
        assert!(report
            .approve(report.id, &report.digest.clone(), "https://other.invalid/")
            .is_err());
        approve(&mut report);
        store.synchronized(report.consent_ticket()).unwrap();
        report
            .send_once(&store, |url, bytes| {
                assert_eq!(url, DEST);
                assert!(!bytes.is_empty());
                Ok(())
            })
            .unwrap();
        assert!(other
            .send_once(&store, |_, _| panic!("approval cannot transfer"))
            .is_err());
    }
    #[test]
    fn withdrawal_and_expired_approval_refuse_before_send() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        approve(&mut report);
        store.synchronized(report.consent_ticket()).unwrap();
        store.decide(Category::Crashes, false).unwrap();
        assert!(report
            .send_once(&store, |_, _| panic!("withdrawn"))
            .is_err());
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        approve(&mut report);
        report.approved = Some(Instant::now() - APPROVAL_TTL);
        store.synchronized(report.consent_ticket()).unwrap();
        assert!(report.send_once(&store, |_, _| panic!("expired")).is_err());
    }
    #[test]
    fn stale_reports_cannot_be_newly_approved() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        report.captured = Instant::now() - REPORT_TTL;
        assert!(report
            .approve(report.id, &report.digest.clone(), DEST)
            .is_err());
    }
    #[test]
    fn failed_send_is_attempted_once() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        approve(&mut report);
        store.synchronized(report.consent_ticket()).unwrap();
        let mut calls = 0;
        assert!(report
            .send_once(&store, |_, _| {
                calls += 1;
                Err(io::Error::other("fixture"))
            })
            .is_err());
        assert_eq!(calls, 1);
        pending(&store).decline();
    }
    #[test]
    fn v0_approved_crash_uses_local_consent_without_server_sync() {
        let (_dir, store) = fixture();
        store.decide(Category::Crashes, true).unwrap();
        let mut report = pending(&store);
        approve(&mut report);
        report.send_once(&store, |_, _| Ok(())).unwrap();
        let mut report = pending(&store);
        approve(&mut report);
        store.decide(Category::Crashes, false).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        assert!(report
            .send_once(&store, |_, _| panic!("old revision"))
            .is_err());
    }
    #[test]
    fn hosted_vendor_and_unsafe_destinations_are_refused() {
        for value in [
            "https://o123.ingest.sentry.io/api/1/store/",
            "https://SENTRY.IO/",
            "https://o1.ingest.sentry.io./",
            "https://u:p@host.invalid/",
            "https://host.invalid/?token=x",
            "http://public.invalid/",
            "file:///tmp/dump",
        ] {
            assert!(destination_url(value).is_err(), "{value}");
        }
        assert!(destination_url(DEST).is_ok());
        assert!(destination_url("http://10.1.2.3/api/1/store/").is_ok());
    }
}
