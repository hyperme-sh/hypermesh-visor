//! Native browser/device authorization bridge. No credential is returned over FFI.
use crate::{AppStore, MachineError, MobileMachine, NativeSessionStore};
use hypermesh_session::{Clock, DeviceCodes, Endpoints, Tokens};
use serde_json::{json, Value};
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(uniffi::Record)]
pub struct OwnerSignInPrompt {
    pub verification_url: String,
    pub user_code: String,
}

trait Flow: Send + Sync {
    fn bot_request(
        &self,
        origin: &str,
        path: &str,
        tenant: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, MachineError> {
        bot_request(origin, path, tenant, token, body)
    }
    fn start(&self) -> Result<DeviceCodes, MachineError>;
    fn poll(&self, codes: &DeviceCodes, clock: &mut CancelClock) -> Result<Tokens, MachineError>;
    fn refresh(&self, store: &AppStore) -> Result<Tokens, MachineError>;
    fn tenant_session(&self, token: &str, tenant: &str) -> Result<String, MachineError>;
}
struct NativeFlow {
    endpoints: Endpoints,
    auth_origin: String,
}
impl Flow for NativeFlow {
    fn start(&self) -> Result<DeviceCodes, MachineError> {
        hypermesh_session::start_device(&self.endpoints).map_err(|_| MachineError::Unavailable)
    }
    fn poll(&self, codes: &DeviceCodes, clock: &mut CancelClock) -> Result<Tokens, MachineError> {
        hypermesh_session::poll_device(
            &self.endpoints,
            &codes.device_code,
            codes.interval,
            codes.expires_in.min(Duration::from_secs(180)),
            clock,
        )
        .map_err(|_| MachineError::Refused)
    }
    fn refresh(&self, store: &AppStore) -> Result<Tokens, MachineError> {
        hypermesh_session::refresh_session(store, &self.endpoints)
            .map_err(|_| MachineError::Refused)
    }
    fn tenant_session(&self, token: &str, tenant: &str) -> Result<String, MachineError> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(30))
            .build();
        let response = agent
            .post(&format!("{}/auth/switch-tenant", self.auth_origin))
            .set("Authorization", &format!("Bearer {token}"))
            .set("X-Tenant-ID", tenant)
            .set("Content-Type", "application/json")
            .send_string(&json!({"tenant_id":tenant}).to_string())
            .map_err(|_| MachineError::Refused)?;
        if response.status() != 200 {
            return Err(MachineError::Refused);
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| MachineError::Unavailable)?;
        if bytes.len() > 65536 {
            return Err(MachineError::Invalid);
        }
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| MachineError::Invalid)?;
        let token = body["access_token"]
            .as_str()
            .filter(|s| crate::valid_credential(s))
            .ok_or(MachineError::Invalid)?;
        if body["token_type"] != "Bearer" {
            return Err(MachineError::Invalid);
        }
        Ok(token.into())
    }
}
struct CancelClock {
    generation: Arc<AtomicU64>,
    expected: u64,
    first: AtomicBool,
}
impl Clock for CancelClock {
    fn now(&self) -> Instant {
        let now = Instant::now();
        if !self.first.swap(false, Ordering::SeqCst)
            && self.generation.load(Ordering::SeqCst) != self.expected
        {
            now + Duration::from_secs(86400)
        } else {
            now
        }
    }
    fn wait(&mut self, period: Duration) {
        let end = Instant::now() + period;
        while self.generation.load(Ordering::SeqCst) == self.expected && Instant::now() < end {
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(100)),
            );
        }
    }
}
struct Pending {
    codes: DeviceCodes,
    generation: u64,
    started: Instant,
}
#[derive(uniffi::Object)]
pub struct MobileOwnerSignIn {
    store: Arc<dyn NativeSessionStore>,
    flow: Arc<dyn Flow>,
    issuer_origin: String,
    pending: Mutex<Option<Pending>>,
    generation: Arc<AtomicU64>,
    commit: Mutex<()>,
}
fn https(value: &str, root: bool) -> Result<url::Url, MachineError> {
    let parsed = url::Url::parse(value).map_err(|_| MachineError::Invalid)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || root && parsed.path() != "/"
    {
        return Err(MachineError::Invalid);
    }
    Ok(parsed)
}
#[uniffi::export]
impl MobileOwnerSignIn {
    #[uniffi::constructor]
    pub fn new(
        store: Arc<dyn NativeSessionStore>,
        issuer: String,
        auth_origin: String,
    ) -> Result<Self, MachineError> {
        let issuer_url = https(&issuer, false)?;
        https(&auth_origin, true)?;
        Ok(Self {
            store,
            flow: Arc::new(NativeFlow {
                endpoints: Endpoints::from_issuer(&issuer, hypermesh_session::PUBLIC_CLIENT_ID),
                auth_origin: auth_origin.trim_end_matches('/').into(),
            }),
            issuer_origin: issuer_url.origin().ascii_serialization(),
            pending: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            commit: Mutex::new(()),
        })
    }
    /// Open the returned verified URL in the system browser; show the short user code.
    pub fn begin(&self) -> Result<OwnerSignInPrompt, MachineError> {
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.pending.lock().map_err(|_| MachineError::Unavailable)? = None;
        let codes = self.flow.start()?;
        let verification = https(&codes.verification_uri, false)?;
        if verification.origin().ascii_serialization() != self.issuer_origin
            || codes.user_code.is_empty()
            || codes.user_code.len() > 128
            || codes.user_code.chars().any(char::is_control)
        {
            return Err(MachineError::Invalid);
        }
        let prompt = OwnerSignInPrompt {
            verification_url: codes.verification_uri.clone(),
            user_code: codes.user_code.clone(),
        };
        *self.pending.lock().map_err(|_| MachineError::Unavailable)? = Some(Pending {
            codes,
            generation,
            started: Instant::now(),
        });
        Ok(prompt)
    }
    /// Run off the UI thread. Cancellation cannot commit a late refresh token.
    pub fn finish(&self) -> Result<(), MachineError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| MachineError::Unavailable)?
            .take()
            .ok_or(MachineError::Refused)?;
        let mut codes = pending.codes;
        codes.expires_in = codes
            .expires_in
            .min(Duration::from_secs(180))
            .saturating_sub(pending.started.elapsed());
        if codes.expires_in.is_zero() {
            return Err(MachineError::Refused);
        }
        let tokens = self.flow.poll(
            &codes,
            &mut CancelClock {
                generation: self.generation.clone(),
                expected: pending.generation,
                first: AtomicBool::new(true),
            },
        )?;
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        if self.generation.load(Ordering::SeqCst) != pending.generation {
            return Err(MachineError::Refused);
        }
        let token = tokens
            .refresh_token
            .as_deref()
            .filter(|s| crate::valid_credential(s) && *s != tokens.access_token)
            .ok_or(MachineError::Invalid)?;
        self.store
            .write_refresh_token(token.into())
            .map_err(|_| MachineError::Storage)
    }
    /// Fence phone admission without blocking the UI on a network request.
    pub fn cancel_phone_operations(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    pub fn cancel(&self) -> Result<(), MachineError> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        *self.pending.lock().map_err(|_| MachineError::Unavailable)? = None;
        Ok(())
    }
    /// Uses the stored browser-authorized session; the UI never handles bearer tokens.
    pub fn enroll_machine(
        &self,
        machine: Arc<MobileMachine>,
        tenant_id: String,
        platform: String,
    ) -> Result<(), MachineError> {
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        let tenant = uuid::Uuid::parse_str(&tenant_id)
            .map_err(|_| MachineError::Invalid)?
            .to_string();
        let tokens = self.flow.refresh(&AppStore(self.store.clone()))?;
        let owner = self.flow.tenant_session(&tokens.access_token, &tenant)?;
        machine.enroll(tenant, platform, owner)
    }
    pub fn revoke_machine(
        &self,
        machine: Arc<MobileMachine>,
        tenant_id: String,
    ) -> Result<(), MachineError> {
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        let tenant = uuid::Uuid::parse_str(&tenant_id)
            .map_err(|_| MachineError::Invalid)?
            .to_string();
        if machine.status()?.ok_or(MachineError::Refused)?.tenant_id != tenant {
            return Err(MachineError::Refused);
        }
        machine.revoke_local()?;
        let tokens = self.flow.refresh(&AppStore(self.store.clone()))?;
        machine.revoke(self.flow.tenant_session(&tokens.access_token, &tenant)?)
    }
}

// Internal only: the UI cannot obtain a tenant session or choose an HTTP URL.
impl MobileOwnerSignIn {
    pub(crate) fn phone_epoch(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub(crate) fn with_session<T>(
        &self,
        tenant: &str,
        expected: u64,
        action: impl FnOnce(&OwnerSession<'_>) -> Result<T, MachineError>,
    ) -> Result<T, MachineError> {
        let _guard = self.commit.lock().map_err(|_| MachineError::Unavailable)?;
        if self.phone_epoch() != expected {
            return Err(MachineError::Refused);
        }
        let tokens = self.flow.refresh(&AppStore(self.store.clone()))?;
        let token = self.flow.tenant_session(&tokens.access_token, tenant)?;
        let session = OwnerSession {
            owner: self,
            expected,
            token,
            tenant,
        };
        session.current()?;
        drop(_guard);
        action(&session)
    }
}
pub(crate) struct OwnerSession<'a> {
    owner: &'a MobileOwnerSignIn,
    expected: u64,
    token: String,
    tenant: &'a str,
}
impl OwnerSession<'_> {
    pub(crate) fn current(&self) -> Result<(), MachineError> {
        if self.owner.generation.load(Ordering::SeqCst) != self.expected {
            return Err(MachineError::Refused);
        }
        Ok(())
    }
    pub(crate) fn request(
        &self,
        origin: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, MachineError> {
        self.current()?;
        let result = self
            .owner
            .flow
            .bot_request(origin, path, self.tenant, &self.token, body)?;
        self.current()?;
        Ok(result)
    }
}
fn bot_request(
    origin: &str,
    path: &str,
    tenant: &str,
    token: &str,
    body: Option<Value>,
) -> Result<Value, MachineError> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(30))
        .build();
    let url = format!("{origin}/api/v1/hypermesh/bots{path}");
    let req = agent
        .request(if body.is_some() { "POST" } else { "GET" }, &url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("X-Tenant-ID", tenant)
        .set("Content-Type", "application/json");
    let response = match body {
        Some(body) => req.send_string(&body.to_string()),
        None => req.call(),
    }
    .map_err(|_| MachineError::Unavailable)?;
    if response.status() != 200 {
        return Err(MachineError::Refused);
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| MachineError::Unavailable)?;
    if bytes.len() > 1_048_576 {
        return Err(MachineError::Invalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| MachineError::Invalid)
}

#[cfg(test)]
pub(crate) fn fixture_owner(
    store: Arc<dyn NativeSessionStore>,
    request: Arc<dyn Fn(&str, Option<Value>) -> Result<Value, MachineError> + Send + Sync>,
) -> Arc<MobileOwnerSignIn> {
    struct Requests(Arc<dyn Fn(&str, Option<Value>) -> Result<Value, MachineError> + Send + Sync>);
    impl Flow for Requests {
        fn start(&self) -> Result<DeviceCodes, MachineError> {
            Err(MachineError::Refused)
        }
        fn poll(&self, _: &DeviceCodes, _: &mut CancelClock) -> Result<Tokens, MachineError> {
            Err(MachineError::Refused)
        }
        fn refresh(&self, _: &AppStore) -> Result<Tokens, MachineError> {
            Ok(Tokens {
                access_token: "owner-access".into(),
                refresh_token: None,
                expires_in: Duration::from_secs(60),
            })
        }
        fn tenant_session(&self, _: &str, _: &str) -> Result<String, MachineError> {
            Ok("owner-session".into())
        }
        fn bot_request(
            &self,
            origin: &str,
            path: &str,
            _: &str,
            token: &str,
            body: Option<Value>,
        ) -> Result<Value, MachineError> {
            assert_eq!(origin, "https://plane.test");
            assert_eq!(token, "owner-session");
            (self.0)(path, body)
        }
    }
    Arc::new(MobileOwnerSignIn {
        store,
        flow: Arc::new(Requests(request)),
        issuer_origin: "https://issuer.test".into(),
        pending: Mutex::new(None),
        generation: Arc::new(AtomicU64::new(0)),
        commit: Mutex::new(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoreError;
    #[derive(Default)]
    struct Store(Mutex<Option<String>>);
    impl NativeSessionStore for Store {
        fn read_refresh_token(&self) -> Result<Option<String>, StoreError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn write_refresh_token(&self, token: String) -> Result<(), StoreError> {
            *self.0.lock().unwrap() = Some(token);
            Ok(())
        }
        fn delete_refresh_token(&self) -> Result<(), StoreError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    struct Fixture {
        generation: Arc<AtomicU64>,
        cancel: bool,
        foreign: bool,
    }
    impl Flow for Fixture {
        fn start(&self) -> Result<DeviceCodes, MachineError> {
            Ok(DeviceCodes {
                device_code: "private-device-code".into(),
                user_code: "ABCD-1234".into(),
                verification_uri: if self.foreign {
                    "https://foreign.test/device"
                } else {
                    "https://issuer.test/device"
                }
                .into(),
                verification_uri_complete: None,
                interval: Duration::from_secs(1),
                expires_in: Duration::from_secs(60),
            })
        }
        fn poll(&self, _: &DeviceCodes, _: &mut CancelClock) -> Result<Tokens, MachineError> {
            if self.cancel {
                self.generation.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Tokens {
                access_token: "private-access-token".into(),
                refresh_token: Some("private-refresh-token".into()),
                expires_in: Duration::from_secs(60),
            })
        }
        fn refresh(&self, _: &AppStore) -> Result<Tokens, MachineError> {
            Err(MachineError::Refused)
        }
        fn tenant_session(&self, _: &str, _: &str) -> Result<String, MachineError> {
            Err(MachineError::Refused)
        }
    }
    fn setup(cancel: bool, foreign: bool) -> (MobileOwnerSignIn, Arc<Store>) {
        let generation = Arc::new(AtomicU64::new(0));
        let store = Arc::new(Store::default());
        (
            MobileOwnerSignIn {
                store: store.clone(),
                flow: Arc::new(Fixture {
                    generation: generation.clone(),
                    cancel,
                    foreign,
                }),
                issuer_origin: "https://issuer.test".into(),
                pending: Mutex::new(None),
                generation,
                commit: Mutex::new(()),
            },
            store,
        )
    }
    #[test]
    fn sign_in_retains_only_refresh_and_cancel_discards_late_success() {
        let (login, store) = setup(false, false);
        let prompt = login.begin().unwrap();
        assert_eq!(prompt.user_code, "ABCD-1234");
        assert!(!prompt.verification_url.contains("private-device-code"));
        login.finish().unwrap();
        assert_eq!(
            store.read_refresh_token().unwrap().as_deref(),
            Some("private-refresh-token")
        );
        assert!(login.finish().is_err());
        let (login, store) = setup(true, false);
        login.begin().unwrap();
        assert!(login.finish().is_err());
        assert!(store.read_refresh_token().unwrap().is_none());
        let (login, store) = setup(false, false);
        login.begin().unwrap();
        login.cancel().unwrap();
        assert!(login.finish().is_err());
        assert!(store.read_refresh_token().unwrap().is_none());
    }
    #[test]
    fn expired_owner_session_still_revokes_local_machine_and_retains_retry_identity() {
        use base64::Engine;
        let (login, _) = setup(false, false);
        let store = Arc::new(Store::default());
        let tenant = uuid::Uuid::new_v4().to_string();
        let device = uuid::Uuid::new_v4().to_string();
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        store.write_refresh_token(json!({
            "version":1,"origin":"https://plane.test","tenant_id":tenant,
            "device_id":device,"request_id":uuid::Uuid::new_v4().to_string(),
            "platform":"android","signing_key":key,"wireguard_key":key,
            "pending_key":null,"authority_id":"local","control_plane_id":"local",
            "bound":true,"revoked":false,"token":"cached-machine-credential","token_expires":9999999999u64
        }).to_string()).unwrap();
        let machine =
            Arc::new(MobileMachine::new(store.clone(), "https://plane.test".into()).unwrap());
        assert!(login
            .revoke_machine(machine.clone(), tenant.clone())
            .is_err());
        assert!(machine.status().unwrap().unwrap().revoked);
        assert!(machine.renew().is_err());
        let saved: Value =
            serde_json::from_str(&store.read_refresh_token().unwrap().unwrap()).unwrap();
        assert!(saved["token"].is_null());
        assert_eq!(saved["device_id"], device);
        assert!(login.revoke_machine(machine, tenant).is_err());
    }
    #[test]
    fn issuer_redirects_and_insecure_configuration_are_refused() {
        let (login, store) = setup(false, true);
        assert!(login.begin().is_err());
        assert!(MobileOwnerSignIn::new(
            store.clone(),
            "http://issuer.test".into(),
            "https://api.test".into()
        )
        .is_err());
        assert!(MobileOwnerSignIn::new(
            store,
            "https://issuer.test/realm".into(),
            "https://user@api.test".into()
        )
        .is_err());
    }
    #[test]
    fn cancellation_before_poll_still_expires_its_first_deadline() {
        let clock = CancelClock {
            generation: Arc::new(AtomicU64::new(2)),
            expected: 1,
            first: AtomicBool::new(true),
        };
        let deadline = clock.now() + Duration::from_secs(180);
        assert!(clock.now() > deadline);
    }
}
