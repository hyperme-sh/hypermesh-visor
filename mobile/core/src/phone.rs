//! Native-only phone credential transport. Prepared prompt bytes are memory-only.
use crate::{
    machine::{identifier, now, origin},
    MachineError, MobileMachine,
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
        let current = self.machine.phone_credential()?;
        if pending.expires <= now()?
            || pending.tenant != current.tenant
            || pending.device != current.device
            || pending.enrollment != current.enrollment
        {
            return Err(MachineError::Refused);
        }
        self.sender.send(&current.token, &pending.raw)
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
}
