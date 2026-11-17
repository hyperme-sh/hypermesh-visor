//! Phone feedback authority remains native. No JS credential or destination input.
use crate::{
    feedback_intake::{
        Category, ConfirmedReport, Consent, IdentityBinding, IntakeClient, IntakeError,
    },
    MobileMachine,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error, uniffi::Error, PartialEq, Eq)]
pub enum FeedbackError {
    #[error("Feedback unavailable or refused")]
    Refused,
    #[error("Feedback outcome unknown; retry only the unchanged report or discard")]
    Unknown,
    #[error("Another feedback action is in progress")]
    Busy,
}
impl From<IntakeError> for FeedbackError {
    fn from(e: IntakeError) -> Self {
        match e {
            IntakeError::Refused => Self::Refused,
            IntakeError::Unknown => Self::Unknown,
        }
    }
}
#[derive(uniffi::Record)]
pub struct FeedbackPreview {
    pub operation_id: String,
    pub title: String,
    pub description: String,
    pub kind: String,
}
#[derive(Clone, PartialEq, Eq)]
struct MachineBinding {
    tenant: String,
    device: String,
    enrollment: String,
}
trait Backend: Send + Sync {
    fn grant(&self) -> Result<(Zeroizing<String>, MachineBinding), FeedbackError>;
    fn consent(&self, grant: &str, change: Option<(bool, u64)>) -> Result<Consent, FeedbackError>;
    fn confirm(
        &self,
        grant: &str,
        kind: Category,
        title: &str,
        body: &str,
        revision: u64,
    ) -> Result<ConfirmedReport, FeedbackError>;
    fn send(&self, grant: &str, report: &ConfirmedReport) -> Result<(), FeedbackError>;
}
struct Native {
    machine: Arc<MobileMachine>,
    intake: IntakeClient,
}
impl Backend for Native {
    fn grant(&self) -> Result<(Zeroizing<String>, MachineBinding), FeedbackError> {
        let c = self
            .machine
            .phone_credential()
            .map_err(|_| FeedbackError::Refused)?;
        let token = Zeroizing::new(c.token);
        let request_id = uuid::Uuid::new_v4().to_string();
        let response = ureq::AgentBuilder::new()
            .redirects(0)
            .try_proxy_from_env(false)
            .timeout(Duration::from_secs(12))
            .build()
            .post(&format!(
                "{}/api/v1/hypermesh/agent/feedback/grant",
                self.machine.plane_origin()
            ))
            .set("Authorization", &format!("Bearer {}", &*token))
            .set("X-Tenant-ID", &c.tenant)
            .set("Content-Type", "application/json")
            .send_string(
                &json!({"version":1,"device_id":c.device,"request_id":request_id}).to_string(),
            )
            .map_err(|_| FeedbackError::Refused)?;
        if response.status() != 200 {
            return Err(FeedbackError::Refused);
        }
        let mut raw = Zeroizing::new(Vec::new());
        response
            .into_reader()
            .take(8193)
            .read_to_end(&mut raw)
            .map_err(|_| FeedbackError::Refused)?;
        if raw.len() > 8192 {
            return Err(FeedbackError::Refused);
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reply {
            version: u32,
            expires_in: u32,
            grant: String,
        }
        let reply: Reply = serde_json::from_slice(&raw).map_err(|_| FeedbackError::Refused)?;
        let grant = Zeroizing::new(reply.grant);
        if reply.version != 1 || reply.expires_in != 60 {
            return Err(FeedbackError::Refused);
        }
        validate_grant(&grant, &c.tenant, &c.device, &request_id)?;
        Ok((
            grant,
            MachineBinding {
                tenant: c.tenant,
                device: c.device,
                enrollment: c.enrollment,
            },
        ))
    }
    fn consent(&self, grant: &str, change: Option<(bool, u64)>) -> Result<Consent, FeedbackError> {
        Ok(match change {
            None => self.intake.read_consent(grant, true)?,
            Some((enabled, revision)) => self
                .intake
                .change_consent(grant, enabled, revision, enabled)?,
        })
    }
    fn confirm(
        &self,
        g: &str,
        k: Category,
        t: &str,
        b: &str,
        r: u64,
    ) -> Result<ConfirmedReport, FeedbackError> {
        Ok(self.intake.confirm(g, k, t, b, r, true)?)
    }
    fn send(&self, g: &str, r: &ConfirmedReport) -> Result<(), FeedbackError> {
        self.intake.submit(g, r, true)?;
        Ok(())
    }
}
fn validate_grant(
    grant: &str,
    tenant: &str,
    device: &str,
    request: &str,
) -> Result<(), FeedbackError> {
    IdentityBinding::from_grant(grant)?;
    let compact = grant
        .strip_prefix("hm_feedback_")
        .ok_or(FeedbackError::Refused)?;
    let payload = compact.split('.').nth(1).ok_or(FeedbackError::Refused)?;
    let raw = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| FeedbackError::Refused)?,
    );
    #[derive(Deserialize)]
    struct Binding {
        tenant_id: String,
        device_id: String,
        jti: String,
    }
    let b: Binding = serde_json::from_slice(&raw).map_err(|_| FeedbackError::Refused)?;
    if b.tenant_id != tenant || b.device_id != device || b.jti != request {
        return Err(FeedbackError::Refused);
    }
    Ok(()) // Intake verifies signature; this is binding to the fixed authenticated Plane response.
}
struct Session {
    identity: Option<IdentityBinding>,
    machine: Option<MachineBinding>,
    revision: u64,
    pending: Option<(String, ConfirmedReport)>,
}
impl Session {
    fn new() -> Self {
        Self {
            identity: None,
            machine: None,
            revision: 0,
            pending: None,
        }
    }
}
#[derive(uniffi::Object)]
pub struct MobileFeedbackClient {
    backend: Arc<dyn Backend>,
    epoch: AtomicU64,
    state: Mutex<Option<Session>>,
}
impl MobileFeedbackClient {
    fn current(&self, id: u64) -> Result<(), FeedbackError> {
        if id != 0 && self.epoch.load(Ordering::SeqCst) == id {
            Ok(())
        } else {
            Err(FeedbackError::Refused)
        }
    }
    fn with<T>(
        &self,
        session: &str,
        action: impl FnOnce(u64, &mut Session) -> Result<T, FeedbackError>,
    ) -> Result<T, FeedbackError> {
        let id = session.parse::<u64>().map_err(|_| FeedbackError::Refused)?;
        self.current(id)?;
        let mut state = self.state.try_lock().map_err(|_| FeedbackError::Busy)?;
        let result = action(id, state.as_mut().ok_or(FeedbackError::Refused)?);
        if self.current(id).is_err() {
            *state = None;
            return Err(FeedbackError::Refused);
        }
        result
    }
    fn grant(&self, id: u64, s: &mut Session) -> Result<Zeroizing<String>, FeedbackError> {
        self.current(id)?;
        let (g, b) = self.backend.grant()?;
        self.current(id)?;
        if s.machine.as_ref().is_some_and(|old| old != &b) {
            s.pending = None;
            return Err(FeedbackError::Refused);
        }
        if let Some(identity) = &s.identity {
            identity.accepts(&g)?;
        } else {
            s.identity = Some(IdentityBinding::from_grant(&g)?);
            s.machine = Some(b);
        }
        Ok(g)
    }
}
#[uniffi::export]
impl MobileFeedbackClient {
    /// Only native startup supplies the trusted destination. No default configuration.
    #[uniffi::constructor]
    pub fn new(machine: Arc<MobileMachine>, intake_origin: String) -> Result<Self, FeedbackError> {
        Ok(Self {
            backend: Arc::new(Native {
                machine,
                intake: IntakeClient::configured(&intake_origin, "visor-phone")?,
            }),
            epoch: AtomicU64::new(0),
            state: Mutex::new(None),
        })
    }
    /// Synchronous local opt-in; no network. Call before queuing other operations.
    pub fn start(&self, local_opt_in: bool) -> Result<String, FeedbackError> {
        if !local_opt_in {
            return Err(FeedbackError::Refused);
        }
        let mut state = self.state.try_lock().map_err(|_| FeedbackError::Busy)?;
        let id = self
            .epoch
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1))
            .map_err(|_| FeedbackError::Refused)?
            + 1;
        *state = Some(Session::new());
        Ok(id.to_string())
    }
    /// Nonblocking cancellation; in-flight I/O may finish but cannot commit a later step.
    pub fn cancel(&self, session_id: String) {
        if let Ok(id) = session_id.parse::<u64>() {
            if let Some(next) = id.checked_add(1) {
                if self
                    .epoch
                    .compare_exchange(id, next, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    if let Ok(mut s) = self.state.try_lock() {
                        *s = None;
                    }
                }
            }
        }
    }
    pub fn cancel_all(&self) {
        let _ = self
            .epoch
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1));
        if let Ok(mut s) = self.state.try_lock() {
            *s = None;
        }
    }
    pub fn consent(&self, session_id: String) -> Result<bool, FeedbackError> {
        self.with(&session_id, |id, s| {
            let g = self.grant(id, s)?;
            let consent = self.backend.consent(&g, None)?;
            self.current(id)?;
            s.revision = consent.revision;
            Ok(consent.enabled)
        })
    }
    pub fn set_consent(&self, session_id: String, enabled: bool) -> Result<(), FeedbackError> {
        self.with(&session_id, |id, s| {
            s.pending = None;
            let g = self.grant(id, s)?;
            let c = self.backend.consent(&g, None)?;
            self.current(id)?;
            let changed = self.backend.consent(&g, Some((enabled, c.revision)))?;
            self.current(id)?;
            s.revision = changed.revision;
            Ok(())
        })
    }
    pub fn prepare(
        &self,
        session_id: String,
        kind: String,
        title: String,
        description: String,
    ) -> Result<FeedbackPreview, FeedbackError> {
        self.with(&session_id, |id, s| {
            s.pending = None;
            let category = match kind.as_str() {
                "bug" => Category::Bug,
                "idea" => Category::Feature,
                _ => return Err(FeedbackError::Refused),
            };
            if title.len() > 560 || description.len() > 8000 {
                return Err(FeedbackError::Refused);
            }
            let g = self.grant(id, s)?;
            let c = self.backend.consent(&g, None)?;
            self.current(id)?;
            if !c.enabled {
                return Err(FeedbackError::Refused);
            }
            let report = self
                .backend
                .confirm(&g, category, &title, &description, c.revision)?;
            let operation_id = uuid::Uuid::new_v4().to_string();
            s.pending = Some((operation_id.clone(), report));
            s.revision = c.revision;
            Ok(FeedbackPreview {
                operation_id,
                title,
                description,
                kind,
            })
        })
    }
    pub fn submit(&self, session_id: String, operation_id: String) -> Result<(), FeedbackError> {
        self.with(&session_id, |id, s| {
            if !s
                .pending
                .as_ref()
                .is_some_and(|(op, _)| op == &operation_id)
            {
                return Err(FeedbackError::Refused);
            }
            let g = self.grant(id, s)?;
            let c = self.backend.consent(&g, None)?;
            self.current(id)?;
            if !c.enabled || c.revision != s.revision {
                s.pending = None;
                return Err(FeedbackError::Refused);
            }
            let result = self
                .backend
                .send(&g, &s.pending.as_ref().ok_or(FeedbackError::Refused)?.1);
            if result.is_ok() || result == Err(FeedbackError::Refused) {
                s.pending = None;
            }
            result // Unknown keeps the exact body/idempotency for an explicit retry.
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    const TENANT: &str = "11111111-1111-4111-8111-111111111111";
    const DEVICE: &str = "22222222-2222-4222-8222-222222222222";
    fn token(tenant: &str, device: &str, request: &str) -> String {
        let now = crate::machine::now().unwrap();
        let payload = json!({"iss":"https://plane.test","sub":"33333333-3333-4333-8333-333333333333","tenant_id":tenant,"device_id":device,"jti":request,"iat":now,"exp":now+60,"aud":"hypermesh-feedback-v1","token_use":"native-feedback-v1"});
        format!(
            "hm_feedback_e30.{}.c2ln",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }
    struct Fake {
        calls: AtomicU64,
        sends: Mutex<Vec<usize>>,
        unknown: AtomicBool,
        grant_refused: AtomicBool,
        changed: AtomicBool,
        hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
                sends: Mutex::new(vec![]),
                unknown: AtomicBool::new(false),
                grant_refused: AtomicBool::new(false),
                changed: AtomicBool::new(false),
                hook: Mutex::new(None),
            }
        }
    }
    impl Backend for Fake {
        fn grant(&self) -> Result<(Zeroizing<String>, MachineBinding), FeedbackError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.grant_refused.load(Ordering::SeqCst) {
                return Err(FeedbackError::Refused);
            }
            Ok((
                Zeroizing::new(token(TENANT, DEVICE, &uuid::Uuid::new_v4().to_string())),
                MachineBinding {
                    tenant: TENANT.into(),
                    device: DEVICE.into(),
                    enrollment: if self.changed.load(Ordering::SeqCst) {
                        "other"
                    } else {
                        "original"
                    }
                    .into(),
                },
            ))
        }
        fn consent(&self, _: &str, change: Option<(bool, u64)>) -> Result<Consent, FeedbackError> {
            if let Some(h) = self.hook.lock().unwrap().take() {
                h()
            }
            Ok(serde_json::from_value(json!({"enabled":change.map(|x|x.0).unwrap_or(true),"revision":1,"notice":"human-reports-v1","purpose":"product-feedback"})).unwrap())
        }
        fn confirm(
            &self,
            g: &str,
            k: Category,
            t: &str,
            b: &str,
            r: u64,
        ) -> Result<ConfirmedReport, FeedbackError> {
            Ok(
                IntakeClient::configured("https://intake.test", "visor-phone")?
                    .confirm(g, k, t, b, r, true)?,
            )
        }
        fn send(&self, _: &str, report: &ConfirmedReport) -> Result<(), FeedbackError> {
            self.sends.lock().unwrap().push(report as *const _ as usize);
            if self.unknown.load(Ordering::SeqCst) {
                Err(FeedbackError::Unknown)
            } else {
                Ok(())
            }
        }
    }
    fn client() -> (Arc<MobileFeedbackClient>, Arc<Fake>) {
        let backend = Arc::new(Fake::new());
        (
            Arc::new(MobileFeedbackClient {
                backend: backend.clone(),
                epoch: AtomicU64::new(0),
                state: Mutex::new(None),
            }),
            backend,
        )
    }
    fn prepare(c: &MobileFeedbackClient, id: &str) -> FeedbackPreview {
        c.prepare(
            id.into(),
            "bug".into(),
            "Example title".into(),
            "Manually entered report".into(),
        )
        .unwrap()
    }
    #[test]
    fn grant_binding_is_exact_and_not_a_general_credential() {
        let request = uuid::Uuid::new_v4().to_string();
        let g = token(TENANT, DEVICE, &request);
        assert!(validate_grant(&g, TENANT, DEVICE, &request).is_ok());
        for (t, d, r) in [
            (DEVICE, DEVICE, request.as_str()),
            (TENANT, TENANT, request.as_str()),
            (TENANT, DEVICE, TENANT),
        ] {
            assert!(validate_grant(&g, t, d, r).is_err())
        }
        assert!(validate_grant("hm_msi_private", TENANT, DEVICE, &request).is_err());
    }
    #[test]
    fn local_opt_in_and_stale_sessions_cannot_send() {
        let (c, b) = client();
        assert!(c.start(false).is_err());
        assert_eq!(b.calls.load(Ordering::SeqCst), 0);
        let old = c.start(true).unwrap();
        let new = c.start(true).unwrap();
        c.cancel(old.clone());
        assert!(c.consent(old).is_err());
        assert!(c.consent(new.clone()).unwrap());
        let p = prepare(&c, &new);
        c.cancel(new.clone());
        assert!(c.submit(new, p.operation_id).is_err());
        assert!(b.sends.lock().unwrap().is_empty());
    }
    #[test]
    fn uncertain_retry_keeps_same_confirmed_object_and_success_consumes() {
        let (c, b) = client();
        let id = c.start(true).unwrap();
        let p = prepare(&c, &id);
        b.unknown.store(true, Ordering::SeqCst);
        assert_eq!(
            c.submit(id.clone(), p.operation_id.clone()),
            Err(FeedbackError::Unknown)
        );
        b.grant_refused.store(true, Ordering::SeqCst);
        assert_eq!(
            c.submit(id.clone(), p.operation_id.clone()),
            Err(FeedbackError::Refused)
        );
        assert_eq!(b.sends.lock().unwrap().len(), 1);
        b.grant_refused.store(false, Ordering::SeqCst);
        b.unknown.store(false, Ordering::SeqCst);
        c.submit(id.clone(), p.operation_id.clone()).unwrap();
        let sent = b.sends.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
        drop(sent);
        assert!(c.submit(id, p.operation_id).is_err());
    }
    #[test]
    fn edits_withdrawal_and_new_enrollment_invalidate_previews() {
        let (c, b) = client();
        let id = c.start(true).unwrap();
        let old = prepare(&c, &id);
        let new = prepare(&c, &id);
        assert!(c.submit(id.clone(), old.operation_id).is_err());
        c.set_consent(id.clone(), false).unwrap();
        assert!(c.submit(id.clone(), new.operation_id).is_err());
        let p = prepare(&c, &id);
        b.changed.store(true, Ordering::SeqCst);
        assert!(c.submit(id, p.operation_id).is_err());
        assert!(b.sends.lock().unwrap().is_empty());
    }
    #[test]
    fn cancellation_during_consent_prevents_the_next_network_step() {
        let (c, b) = client();
        let id = c.start(true).unwrap();
        let weak = Arc::downgrade(&c);
        let cancel = id.clone();
        *b.hook.lock().unwrap() = Some(Box::new(move || weak.upgrade().unwrap().cancel(cancel)));
        assert!(c
            .prepare(id, "bug".into(), "Title".into(), "Report text here".into())
            .is_err());
        assert!(c.state.lock().unwrap().is_none());
        assert!(b.sends.lock().unwrap().is_empty());
    }
}
