//! Native-only phone credential transport. Prepared prompt bytes are memory-only.
use crate::{
    machine::{identifier, now, origin},
    MachineError, MobileMachine, MobileOwnerSignIn,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(uniffi::Record)]
pub struct PhonePrompt {
    pub bot_id: String,
    pub assignment_id: String,
    pub session_id: String,
    pub task_class: String,
    pub destination: String,
    pub prompt: String,
    pub max_output_tokens: i32,
}
#[derive(Clone, uniffi::Record)]
pub struct PreparedPhonePrompt {
    pub operation_id: String,
    pub task_id: String,
    pub request_sha256: String,
}
#[derive(uniffi::Record)]
pub struct PhoneSession {
    pub bot_id: String,
    pub assignment_id: String,
    pub session_id: String,
    pub expires_at: i64,
}
fn sessions(value: &Value, current: u64) -> Result<Vec<PhoneSession>, MachineError> {
    let bots = value["bots"]
        .as_array()
        .filter(|v| v.len() <= 100)
        .ok_or(MachineError::Invalid)?;
    let mut result = Vec::new();
    for bot in bots {
        if bot["revoked"] != false
            || bot["online"] != true
            || bot["local_consent"] != true
            || bot["mode"] != "interactive"
            || bot["desired"] != "running"
            || !bot["capabilities"]
                .as_array()
                .is_some_and(|v| v.iter().any(|x| x == "prompt"))
        {
            continue;
        }
        let assignment = &bot["assignment"];
        let Some(expiry) = assignment["expires_at"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > current as f64 && *v < i64::MAX as f64)
        else {
            continue;
        };
        let id = |value: &Value| -> Result<String, MachineError> {
            let id = value.as_str().ok_or(MachineError::Invalid)?;
            identifier(id)?;
            if id == "00000000-0000-0000-0000-000000000000" {
                return Err(MachineError::Invalid);
            }
            Ok(id.into())
        };
        result.push(PhoneSession {
            bot_id: id(&bot["bot_id"])?,
            assignment_id: id(&assignment["id"])?,
            session_id: id(&assignment["session_id"])?,
            expires_at: expiry as i64,
        });
    }
    Ok(result)
}
struct Pending {
    raw: String,
    tenant: String,
    device: String,
    enrollment: String,
    expires: u64,
}
trait Sender: Send + Sync {
    fn send(&self, token: &str, raw: &str) -> Result<String, MachineError>;
}
struct HttpsSender {
    origin: String,
}
impl Sender for HttpsSender {
    fn send(&self, token: &str, raw: &str) -> Result<String, MachineError> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(30))
            .build();
        let response = agent
            .post(&format!("{}/v1/phone/prompt-submit", self.origin))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_string(raw)
            .map_err(|_| MachineError::Unavailable)?;
        if response.status() != 200 {
            return Err(MachineError::Unavailable);
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
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| MachineError::Invalid)?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or(MachineError::Invalid)?;
        if content.len() > 262_144 {
            return Err(MachineError::Invalid);
        }
        Ok(content.into())
    }
}
#[derive(uniffi::Object)]
pub struct MobilePhoneClient {
    machine: Arc<MobileMachine>,
    sender: Arc<dyn Sender>,
    pending: Mutex<HashMap<String, Pending>>,
    active: Mutex<()>,
}
#[uniffi::export]
impl MobilePhoneClient {
    /// Native startup supplies the provisioned Conductor origin; never a message URL.
    #[uniffi::constructor]
    pub fn new(
        machine: Arc<MobileMachine>,
        conductor_origin: String,
    ) -> Result<Self, MachineError> {
        Ok(Self {
            machine,
            sender: Arc::new(HttpsSender {
                origin: origin(&conductor_origin)?,
            }),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(()),
        })
    }
    /// Public owned assignments only; no bot or owner credential crosses FFI.
    pub fn owned_sessions(
        &self,
        owner: Arc<MobileOwnerSignIn>,
    ) -> Result<Vec<PhoneSession>, MachineError> {
        let expected = owner.phone_epoch();
        let current = self.machine.phone_credential()?;
        owner.with_session(&current.tenant, expected, |session| {
            sessions(
                &session.request(self.machine.plane_origin(), "", None)?,
                now()?,
            )
        })
    }
    /// One explicit user send grants this phone and admits the exact digest.
    /// Consume before any owner request: uncertain admission is never retried.
    pub fn send_owned(
        &self,
        owner: Arc<MobileOwnerSignIn>,
        prompt: PhonePrompt,
        expires_at: i64,
    ) -> Result<String, MachineError> {
        let _active = self.active.try_lock().map_err(|_| MachineError::Refused)?;
        let expected = owner.phone_epoch();
        let prepared = self.prepare(prompt)?;
        let pending = self
            .pending
            .lock()
            .map_err(|_| MachineError::Unavailable)?
            .remove(&prepared.operation_id)
            .ok_or(MachineError::Refused)?;
        let current = self.machine.phone_credential()?;
        let raw: Value = serde_json::from_str(&pending.raw).map_err(|_| MachineError::Invalid)?;
        let expires = u64::try_from(expires_at)
            .map_err(|_| MachineError::Invalid)?
            .min(now()?.saturating_add(300));
        if pending.tenant != current.tenant
            || pending.device != current.device
            || pending.enrollment != current.enrollment
            || pending.expires <= now()?
        {
            return Err(MachineError::Refused);
        }
        if expires <= now()? {
            return Err(MachineError::Refused);
        }
        owner.with_session(&current.tenant, expected, |session| {
            let prefix = format!("/{}/phone", raw["bot_id"].as_str().ok_or(MachineError::Invalid)?);
            let binding = json!({"version":1,"device_id":current.device,"enrollment_id":current.enrollment,
                "assignment_id":raw["assignment_id"],"session_id":raw["session_id"]});
            let mut grant = binding.clone(); grant["expires_at"] = json!(expires); grant["revoke"] = json!(false);
            let granted = session.request(self.machine.plane_origin(), &format!("{prefix}/grant"), Some(grant))?;
            if granted["version"] != 1 || granted["revoked"] != false { return Err(MachineError::Refused); }
            let nonce = uuid::Uuid::new_v4().to_string();
            let mut admission = binding;
            admission["request_id"] = json!(nonce); admission["task_id"] = json!(prepared.task_id);
            admission["request_sha256"] = json!(prepared.request_sha256);
            let admitted = session.request(self.machine.plane_origin(), &format!("{prefix}/owner-prompt"), Some(admission))?;
            if admitted["version"] != 1 || admitted["request_id"] != nonce || admitted["task_id"] != prepared.task_id
                || admitted["request_sha256"] != prepared.request_sha256 || admitted["assignment_id"] != raw["assignment_id"]
                || admitted["session_id"] != raw["session_id"]
                || !admitted["expires_at"].as_f64().is_some_and(|v| v > now().unwrap_or(u64::MAX) as f64 && v <= expires as f64) {
                return Err(MachineError::Refused);
            }
            session.current()?;
            self.send_pending_checked(pending, || session.current())
        })
    }
    /// Returns only public task binding. The owner must authorize this exact digest
    /// and the assigned host must accept the task before submit can succeed.
    pub fn prepare(&self, prompt: PhonePrompt) -> Result<PreparedPhonePrompt, MachineError> {
        for id in [&prompt.bot_id, &prompt.assignment_id, &prompt.session_id] {
            identifier(id)?;
            if id == "00000000-0000-0000-0000-000000000000" {
                return Err(MachineError::Invalid);
            }
        }
        if prompt.prompt.is_empty()
            || prompt.prompt.len() > 65536
            || prompt.max_output_tokens <= 0
            || prompt.max_output_tokens > 32768
            || prompt.task_class.is_empty()
            || prompt.task_class.len() > 128
            || prompt.task_class.chars().any(char::is_control)
            || prompt.destination.is_empty()
            || prompt.destination.len() > 256
            || prompt.destination.chars().any(char::is_control)
        {
            return Err(MachineError::Invalid);
        }
        let credential = self.machine.phone_credential()?;
        let operation = uuid::Uuid::new_v4().to_string();
        let task = uuid::Uuid::new_v4().to_string();
        let raw = json!({"version":1,"tenant_id":credential.tenant,"device_id":credential.device,
            "enrollment_id":credential.enrollment,"bot_id":prompt.bot_id,"assignment_id":prompt.assignment_id,
            "session_id":prompt.session_id,"task_id":task,"operation_id":operation,"class":prompt.task_class,
            "destination":prompt.destination,"prompt":prompt.prompt,"max_output_tokens":prompt.max_output_tokens}).to_string();
        let digest = format!("{:x}", Sha256::digest(raw.as_bytes()));
        let mut pending = self.pending.lock().map_err(|_| MachineError::Unavailable)?;
        let current = now()?;
        pending.retain(|_, value| value.expires > current);
        if pending.len() >= 8 {
            return Err(MachineError::Refused);
        }
        pending.insert(
            operation.clone(),
            Pending {
                raw,
                tenant: credential.tenant,
                device: credential.device,
                enrollment: credential.enrollment,
                expires: current + 300,
            },
        );
        Ok(PreparedPhonePrompt {
            operation_id: operation,
            task_id: task,
            request_sha256: digest,
        })
    }
    /// Removes the prepared request before I/O. Any failure is uncertain and must
    /// never cause an automatic retry or a new operation to be synthesized.
    pub fn submit(&self, operation_id: String) -> Result<String, MachineError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| MachineError::Unavailable)?
            .remove(&operation_id)
            .ok_or(MachineError::Refused)?;
        self.send_pending(pending)
    }
    /// Cancels only a prepared request. Submitted operations have uncertain outcomes.
    pub fn discard(&self, operation_id: String) -> Result<(), MachineError> {
        self.pending
            .lock()
            .map_err(|_| MachineError::Unavailable)?
            .remove(&operation_id);
        Ok(())
    }
}

impl MobilePhoneClient {
    fn send_pending(&self, pending: Pending) -> Result<String, MachineError> {
        self.send_pending_checked(pending, || Ok(()))
    }
    fn send_pending_checked(
        &self,
        pending: Pending,
        check: impl FnOnce() -> Result<(), MachineError>,
    ) -> Result<String, MachineError> {
        let current = self.machine.phone_credential()?;
        if pending.expires <= now()?
            || pending.tenant != current.tenant
            || pending.device != current.device
            || pending.enrollment != current.enrollment
        {
            return Err(MachineError::Refused);
        }
        check()?;
        self.sender.send(&current.token, &pending.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NativeSessionStore, StoreError};
    use base64::Engine;
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
    struct Capture {
        calls: Mutex<Vec<(String, String)>>,
        fail: bool,
    }
    impl Sender for Capture {
        fn send(&self, token: &str, raw: &str) -> Result<String, MachineError> {
            self.calls.lock().unwrap().push((token.into(), raw.into()));
            if self.fail {
                Err(MachineError::Unavailable)
            } else {
                Ok("Reply".into())
            }
        }
    }
    fn setup(fail: bool) -> (MobilePhoneClient, Arc<Store>, Arc<Capture>) {
        let store = Arc::new(Store::default());
        let device = uuid::Uuid::new_v4().to_string();
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        store.write_refresh_token(json!({"version":1,"origin":"https://plane.test",
            "tenant_id":uuid::Uuid::new_v4().to_string(),"device_id":device,"request_id":uuid::Uuid::new_v4().to_string(),
            "platform":"android","signing_key":key,"wireguard_key":key,"pending_key":null,
            "authority_id":"local","control_plane_id":"local","bound":true,"revoked":false,
            "token":format!("hm_msi_{device}.private-fixture"),"token_expires":now().unwrap()+500}).to_string()).unwrap();
        let machine =
            Arc::new(MobileMachine::new(store.clone(), "https://plane.test".into()).unwrap());
        let capture = Arc::new(Capture {
            calls: Mutex::new(vec![]),
            fail,
        });
        (
            MobilePhoneClient {
                machine,
                sender: capture.clone(),
                pending: Mutex::new(HashMap::new()),
                active: Mutex::new(()),
            },
            store,
            capture,
        )
    }
    fn prompt() -> PhonePrompt {
        PhonePrompt {
            bot_id: uuid::Uuid::new_v4().to_string(),
            assignment_id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            task_class: "coding".into(),
            destination: "model".into(),
            prompt: "Private prompt".into(),
            max_output_tokens: 128,
        }
    }
    #[test]
    fn exact_digest_native_credential_and_no_replay() {
        let (client, store, capture) = setup(false);
        let prepared = client.prepare(prompt()).unwrap();
        assert!(!store
            .read_refresh_token()
            .unwrap()
            .unwrap()
            .contains("Private prompt"));
        assert_eq!(
            client.submit(prepared.operation_id.clone()).unwrap(),
            "Reply"
        );
        assert!(client.submit(prepared.operation_id).is_err());
        let calls = capture.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.starts_with("hm_msi_"));
        assert!(!calls[0].1.contains("private-fixture"));
        assert_eq!(
            prepared.request_sha256,
            format!("{:x}", Sha256::digest(calls[0].1.as_bytes()))
        );
    }
    #[test]
    fn failed_send_is_consumed_and_revoke_prevents_send() {
        let (client, _, capture) = setup(true);
        let prepared = client.prepare(prompt()).unwrap();
        assert!(client.submit(prepared.operation_id.clone()).is_err());
        assert!(client.submit(prepared.operation_id).is_err());
        assert_eq!(capture.calls.lock().unwrap().len(), 1);
        let (client, store, capture) = setup(false);
        let prepared = client.prepare(prompt()).unwrap();
        let mut state: Value =
            serde_json::from_str(&store.read_refresh_token().unwrap().unwrap()).unwrap();
        state["revoked"] = json!(true);
        store.write_refresh_token(state.to_string()).unwrap();
        assert!(client.submit(prepared.operation_id).is_err());
        assert!(capture.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn discard_and_bad_inputs_never_send() {
        let (client, _, capture) = setup(false);
        let prepared = client.prepare(prompt()).unwrap();
        client.discard(prepared.operation_id.clone()).unwrap();
        assert!(client.submit(prepared.operation_id).is_err());
        let mut bad = prompt();
        bad.destination = "model\r\ninjected".into();
        assert!(client.prepare(bad).is_err());
        for value in [
            "http://plain.test",
            "https://user:password@host.test",
            "https://host.test/path",
        ] {
            assert!(MobilePhoneClient::new(client.machine.clone(), value.into()).is_err());
        }
        assert!(capture.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn owner_admission_binds_exact_native_bytes_and_never_sends_owner_token() {
        let (client, store, capture) = setup(false);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let owner = crate::owner::fixture_owner(
            store.clone(),
            Arc::new(move |path, body| {
                let mut body = body.unwrap();
                recorded
                    .lock()
                    .unwrap()
                    .push((path.to_string(), body.clone()));
                if path.ends_with("/grant") {
                    return Ok(json!({"version":1,"revoked":false}));
                }
                body["expires_at"] = json!(now().unwrap() + 5);
                Ok(body)
            }),
        );
        assert_eq!(
            client
                .send_owned(owner, prompt(), (now().unwrap() + 60) as i64)
                .unwrap(),
            "Reply"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.ends_with("/phone/grant"));
        assert!(calls[1].0.ends_with("/phone/owner-prompt"));
        let sent = capture.calls.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].0.starts_with("hm_msi_"));
        assert_eq!(
            calls[1].1["request_sha256"],
            format!("{:x}", Sha256::digest(sent[0].1.as_bytes()))
        );
        assert!(!serde_json::to_string(&*calls)
            .unwrap()
            .contains("Private prompt"));
        assert!(client.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn refused_or_mismatched_admission_and_local_revoke_never_reach_conductor() {
        for mode in 0..3 {
            let (client, store, capture) = setup(false);
            let machine = client.machine.clone();
            let owner = crate::owner::fixture_owner(
                store,
                Arc::new(move |path, body| {
                    if path.ends_with("/grant") {
                        return Ok(json!({"version":1,"revoked":false}));
                    }
                    if mode == 0 {
                        return Err(MachineError::Unavailable);
                    }
                    let mut body = body.unwrap();
                    body["expires_at"] = json!(now().unwrap() + 5);
                    if mode == 1 {
                        body["request_sha256"] = json!("wrong");
                    }
                    if mode == 2 {
                        machine.revoke_local()?;
                    }
                    Ok(body)
                }),
            );
            assert!(client
                .send_owned(owner, prompt(), (now().unwrap() + 60) as i64)
                .is_err());
            assert!(capture.calls.lock().unwrap().is_empty());
            assert!(client.pending.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn owned_session_list_filters_expired_paused_and_unconsented_assignments() {
        let input = prompt();
        let bot = json!({"bot_id":input.bot_id,"revoked":false,"online":true,"local_consent":true,
            "mode":"interactive","desired":"running","capabilities":["prompt"],
            "assignment":{"id":input.assignment_id,"session_id":input.session_id,"expires_at":200}});
        assert_eq!(
            sessions(&json!({"bots":[bot.clone()]}), 100).unwrap().len(),
            1
        );
        for (key, value) in [
            ("local_consent", json!(false)),
            ("online", json!(false)),
            ("revoked", json!(true)),
            ("desired", json!("paused")),
        ] {
            let mut item = bot.clone();
            item[key] = value;
            assert!(sessions(&json!({"bots":[item]}), 100).unwrap().is_empty());
        }
        assert!(sessions(&json!({"bots":[bot]}), 200).unwrap().is_empty());
    }
    #[test]
    fn cancellation_during_grant_fences_admission_without_blocking() {
        let (client, store, capture) = setup(false);
        let owner_ref = Arc::new(Mutex::new(None::<std::sync::Weak<MobileOwnerSignIn>>));
        let reference = owner_ref.clone();
        let owner = crate::owner::fixture_owner(
            store,
            Arc::new(move |path, _| {
                assert!(path.ends_with("/grant"));
                reference
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .upgrade()
                    .unwrap()
                    .cancel_phone_operations();
                Ok(json!({"version":1,"revoked":false}))
            }),
        );
        *owner_ref.lock().unwrap() = Some(Arc::downgrade(&owner));
        assert!(client
            .send_owned(owner, prompt(), (now().unwrap() + 60) as i64)
            .is_err());
        assert!(capture.calls.lock().unwrap().is_empty());
        assert!(client.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn cancellation_during_preparation_does_not_become_a_new_authorization_epoch() {
        let (mut client, store, capture) = setup(false);
        let owner = crate::owner::fixture_owner(
            store.clone(),
            Arc::new(|_, _| panic!("cancelled preparation must not contact Plane")),
        );
        struct CancellingStore {
            inner: Arc<Store>,
            owner: Arc<MobileOwnerSignIn>,
        }
        impl NativeSessionStore for CancellingStore {
            fn read_refresh_token(&self) -> Result<Option<String>, StoreError> {
                self.owner.cancel_phone_operations();
                self.inner.read_refresh_token()
            }
            fn write_refresh_token(&self, token: String) -> Result<(), StoreError> {
                self.inner.write_refresh_token(token)
            }
            fn delete_refresh_token(&self) -> Result<(), StoreError> {
                self.inner.delete_refresh_token()
            }
        }
        client.machine = Arc::new(
            MobileMachine::new(
                Arc::new(CancellingStore {
                    inner: store,
                    owner: owner.clone(),
                }),
                "https://plane.test".into(),
            )
            .unwrap(),
        );
        assert!(client
            .send_owned(owner, prompt(), (now().unwrap() + 60) as i64)
            .is_err());
        assert!(capture.calls.lock().unwrap().is_empty());
        assert!(client.pending.lock().unwrap().is_empty());
    }
}
