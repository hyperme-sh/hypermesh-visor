//! Phone enrollment. Native secure storage is the only durable credential owner.
use crate::NativeSessionStore;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MachineError {
    #[error("machine secure storage unavailable")]
    Storage,
    #[error("invalid machine configuration or response")]
    Invalid,
    #[error("machine service unavailable")]
    Unavailable,
    #[error("machine authorization refused")]
    Refused,
    #[error("machine enrollment revoked")]
    Revoked,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    version: u8,
    origin: String,
    tenant_id: String,
    device_id: String,
    request_id: String,
    platform: String,
    signing_key: String,
    wireguard_key: String,
    pending_key: Option<String>,
    authority_id: Option<String>,
    control_plane_id: Option<String>,
    bound: bool,
    revoked: bool,
    token: Option<String>,
    token_expires: u64,
}

#[derive(Debug, uniffi::Record)]
pub struct MachineStatus {
    pub device_id: String,
    pub tenant_id: String,
    pub enrolled: bool,
    pub revoked: bool,
}

trait Transport: Send + Sync {
    fn post(
        &self,
        path: &str,
        tenant: &str,
        owner: Option<&str>,
        body: &Value,
    ) -> Result<Value, MachineError>;
}
struct Https {
    origin: String,
}
impl Transport for Https {
    fn post(
        &self,
        path: &str,
        tenant: &str,
        owner: Option<&str>,
        body: &Value,
    ) -> Result<Value, MachineError> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(Duration::from_secs(30))
            .build();
        let mut request = agent
            .post(&format!(
                "{}/api/v1/hypermesh/client-machines{}",
                self.origin, path
            ))
            .set("X-Tenant-ID", tenant)
            .set("Content-Type", "application/json");
        if let Some(owner) = owner {
            if owner.is_empty() || owner.len() > 65536 || owner.chars().any(char::is_control) {
                return Err(MachineError::Invalid);
            }
            request = request.set("Authorization", &format!("Bearer {owner}"));
        }
        let response = match request.send_string(&body.to_string()) {
            Ok(response) if response.status() == 200 => response,
            Err(ureq::Error::Status(410, _)) => return Err(MachineError::Revoked),
            Err(ureq::Error::Status(400 | 401 | 403 | 404 | 409 | 422, _)) => {
                return Err(MachineError::Refused)
            }
            _ => return Err(MachineError::Unavailable),
        };
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| MachineError::Unavailable)?;
        if bytes.len() > 65536 {
            return Err(MachineError::Invalid);
        }
        serde_json::from_slice(&bytes).map_err(|_| MachineError::Invalid)
    }
}

#[derive(uniffi::Object)]
pub struct MobileMachine {
    store: Arc<dyn NativeSessionStore>,
    transport: Arc<dyn Transport>,
    origin: String,
    lock: Mutex<()>,
}

fn origin(value: &str) -> Result<String, MachineError> {
    let parsed = url::Url::parse(value).map_err(|_| MachineError::Invalid)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(MachineError::Invalid);
    }
    Ok(value.trim_end_matches('/').to_owned())
}
fn key() -> Result<String, MachineError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| MachineError::Unavailable)?;
    Ok(STANDARD.encode(bytes))
}
fn bytes(value: &str) -> Result<[u8; 32], MachineError> {
    STANDARD
        .decode(value)
        .map_err(|_| MachineError::Invalid)?
        .try_into()
        .map_err(|_| MachineError::Invalid)
}
fn public(value: &str) -> Result<String, MachineError> {
    Ok(STANDARD.encode(
        SigningKey::from_bytes(&bytes(value)?)
            .verifying_key()
            .to_bytes(),
    ))
}
fn wg_public(value: &str) -> Result<String, MachineError> {
    Ok(STANDARD.encode(
        x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bytes(value)?)).to_bytes(),
    ))
}
fn now() -> Result<u64, MachineError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| MachineError::Invalid)
}
fn identifier(value: &str) -> Result<(), MachineError> {
    if uuid::Uuid::parse_str(value)
        .map_err(|_| MachineError::Invalid)?
        .to_string()
        != value
    {
        return Err(MachineError::Invalid);
    }
    Ok(())
}
fn authority_name(value: &Value, name: &str) -> Result<String, MachineError> {
    let value = value[name].as_str().ok_or(MachineError::Invalid)?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err(MachineError::Invalid);
    }
    Ok(value.into())
}

impl MobileMachine {
    pub(crate) fn revoke_local(&self) -> Result<(), MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        let mut state = self.read()?.ok_or(MachineError::Refused)?;
        state.revoked = true;
        state.token = None;
        state.token_expires = 0;
        self.save(&state)
    }
    fn read(&self) -> Result<Option<Identity>, MachineError> {
        let Some(raw) = self
            .store
            .read_refresh_token()
            .map_err(|_| MachineError::Storage)?
        else {
            return Ok(None);
        };
        if raw.len() > 65536 {
            return Err(MachineError::Invalid);
        }
        let state: Identity = serde_json::from_str(&raw).map_err(|_| MachineError::Invalid)?;
        if state.version != 1
            || state.origin != self.origin
            || !matches!(state.platform.as_str(), "android" | "ios")
        {
            return Err(MachineError::Invalid);
        }
        for value in [&state.tenant_id, &state.device_id, &state.request_id] {
            identifier(value)?;
        }
        bytes(&state.signing_key)?;
        bytes(&state.wireguard_key)?;
        if let Some(next) = &state.pending_key {
            bytes(next)?;
        }
        Ok(Some(state))
    }
    fn save(&self, state: &Identity) -> Result<(), MachineError> {
        let raw = serde_json::to_string(state).map_err(|_| MachineError::Invalid)?;
        self.store
            .write_refresh_token(raw)
            .map_err(|_| MachineError::Storage)
    }
    fn proof(&self, state: &Identity, use_pending: bool) -> Result<Value, MachineError> {
        let secret = if use_pending {
            state.pending_key.as_ref().ok_or(MachineError::Invalid)?
        } else {
            &state.signing_key
        };
        let public = public(secret)?;
        let next = if use_pending {
            String::new()
        } else {
            state
                .pending_key
                .as_ref()
                .map(|k| public_key(k))
                .transpose()?
                .unwrap_or_default()
        };
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(|_| MachineError::Unavailable)?;
        let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let issued = now()?;
        let authority = state.authority_id.as_deref().ok_or(MachineError::Invalid)?;
        let plane = state
            .control_plane_id
            .as_deref()
            .ok_or(MachineError::Invalid)?;
        let message = format!(
            "hypermesh-machine-identity-v1\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            state.tenant_id,
            state.device_id,
            state.request_id,
            authority,
            plane,
            public,
            issued,
            nonce,
            next
        );
        let signature = STANDARD.encode(
            SigningKey::from_bytes(&bytes(secret)?)
                .sign(message.as_bytes())
                .to_bytes(),
        );
        Ok(
            json!({"authority_id":authority,"control_plane_id":plane,"protocol_version":1,
            "device_id":state.device_id,"request_id":state.request_id,"nonce":nonce,"public_key":public,
            "issued_at":issued,"signature":signature,"next_public_key":next}),
        )
    }
    fn renew_locked(&self, state: &mut Identity) -> Result<(), MachineError> {
        if state.revoked {
            return Err(MachineError::Revoked);
        }
        state.token = None;
        state.token_expires = 0;
        self.save(state)?;
        let started = now()?;
        let path = if state.bound { "/token" } else { "/bind" };
        let mut result =
            self.transport
                .post(path, &state.tenant_id, None, &self.proof(state, false)?);
        if matches!(result, Err(MachineError::Refused))
            && state.bound
            && state.pending_key.is_some()
        {
            result = self
                .transport
                .post(path, &state.tenant_id, None, &self.proof(state, true)?);
        }
        let result = match result {
            Ok(result) => result,
            Err(MachineError::Revoked) => {
                state.revoked = true;
                self.save(state)?;
                return Err(MachineError::Revoked);
            }
            Err(error) => return Err(error),
        };
        let current = public(&state.signing_key)?;
        let next = state.pending_key.as_ref().map(|k| public(k)).transpose()?;
        let returned = result["public_key"].as_str().ok_or(MachineError::Invalid)?;
        if returned != current && next.as_deref() != Some(returned) {
            return Err(MachineError::Invalid);
        }
        if let Some(next) = next {
            if returned != next {
                return Err(MachineError::Invalid);
            }
        }
        let token = result["access_token"]
            .as_str()
            .ok_or(MachineError::Invalid)?;
        let ttl = result["expires_in"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 600)
            .ok_or(MachineError::Invalid)?;
        if result["token_type"] != "Bearer"
            || !token.starts_with(&format!("hm_msi_{}.", state.device_id))
            || token.len() > 512
            || token.chars().any(char::is_control)
            || now()?.saturating_sub(started) >= ttl
        {
            return Err(MachineError::Invalid);
        }
        if let Some(replacement) = state.pending_key.take() {
            state.signing_key = replacement;
        }
        state.bound = true;
        state.token = Some(token.into());
        state.token_expires = started + ttl;
        self.save(state)
    }
}
fn public_key(key: &str) -> Result<String, MachineError> {
    public(key)
}

#[uniffi::export]
impl MobileMachine {
    /// Supply a dedicated machine namespace, never the owner sign-in store.
    #[uniffi::constructor]
    pub fn new(
        store: Arc<dyn NativeSessionStore>,
        plane_origin: String,
    ) -> Result<Self, MachineError> {
        let origin = origin(&plane_origin)?;
        Ok(Self {
            store,
            transport: Arc::new(Https {
                origin: origin.clone(),
            }),
            origin,
            lock: Mutex::new(()),
        })
    }
    pub fn status(&self) -> Result<Option<MachineStatus>, MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        Ok(self.read()?.map(|s| MachineStatus {
            device_id: s.device_id,
            tenant_id: s.tenant_id,
            enrolled: s.bound && !s.revoked,
            revoked: s.revoked,
        }))
    }
    /// The native sign-in flow supplies its current tenant-bound owner session.
    pub fn enroll(
        &self,
        tenant_id: String,
        platform: String,
        owner_session: String,
    ) -> Result<(), MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        identifier(&tenant_id)?;
        if !matches!(platform.as_str(), "android" | "ios") {
            return Err(MachineError::Invalid);
        }
        let mut state = match self.read()? {
            Some(s) if s.tenant_id == tenant_id && s.platform == platform && !s.revoked => s,
            Some(_) => return Err(MachineError::Refused),
            None => {
                let s = Identity {
                    version: 1,
                    origin: self.origin.clone(),
                    tenant_id,
                    platform,
                    device_id: uuid::Uuid::new_v4().to_string(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    signing_key: key()?,
                    wireguard_key: key()?,
                    pending_key: None,
                    authority_id: None,
                    control_plane_id: None,
                    bound: false,
                    revoked: false,
                    token: None,
                    token_expires: 0,
                };
                self.save(&s)?;
                s
            }
        };
        let body = json!({"request_id":state.request_id,"device_id":state.device_id,"platform":state.platform,
            "public_key":public(&state.signing_key)?,"wg_public_key":wg_public(&state.wireguard_key)?});
        let reply = self
            .transport
            .post("", &state.tenant_id, Some(&owner_session), &body)?;
        for name in [
            "request_id",
            "device_id",
            "platform",
            "public_key",
            "wg_public_key",
        ] {
            if reply[name] != body[name] {
                return Err(MachineError::Invalid);
            }
        }
        if reply["tenant_id"] != state.tenant_id
            || reply["client_policy"] != "owner-attested-v1"
            || !matches!(reply["status"].as_str(), Some("approved" | "bound"))
        {
            return Err(MachineError::Invalid);
        }
        let authority = authority_name(&reply, "authority_id")?;
        let plane = authority_name(&reply, "control_plane_id")?;
        if state.authority_id.as_ref().is_some_and(|a| a != &authority)
            || state.control_plane_id.as_ref().is_some_and(|p| p != &plane)
        {
            return Err(MachineError::Invalid);
        }
        state.authority_id = Some(authority);
        state.control_plane_id = Some(plane);
        self.save(&state)?;
        self.renew_locked(&mut state)
    }
    pub fn renew(&self) -> Result<(), MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        let mut state = self.read()?.ok_or(MachineError::Refused)?;
        self.renew_locked(&mut state)
    }
    pub fn rotate_signing_key(&self) -> Result<(), MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        let mut state = self.read()?.ok_or(MachineError::Refused)?;
        if !state.bound || state.revoked {
            return Err(MachineError::Refused);
        }
        if state.pending_key.is_none() {
            state.pending_key = Some(key()?);
            self.save(&state)?;
        }
        self.renew_locked(&mut state)
    }
    pub fn revoke(&self, owner_session: String) -> Result<(), MachineError> {
        let _lock = self.lock.lock().map_err(|_| MachineError::Unavailable)?;
        let mut state = self.read()?.ok_or(MachineError::Refused)?;
        state.revoked = true;
        state.token = None;
        state.token_expires = 0;
        self.save(&state)?;
        let result = self.transport.post(
            &format!("/{}/revoke", state.device_id),
            &state.tenant_id,
            Some(&owner_session),
            &json!({}),
        )?;
        if result["revoked"] != true {
            return Err(MachineError::Invalid);
        }
        self.store
            .delete_refresh_token()
            .map_err(|_| MachineError::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoreError;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Default)]
    struct Store {
        value: Mutex<Option<String>>,
        fail: AtomicBool,
    }
    impl NativeSessionStore for Store {
        fn read_refresh_token(&self) -> Result<Option<String>, StoreError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            Ok(self.value.lock().unwrap().clone())
        }
        fn write_refresh_token(&self, token: String) -> Result<(), StoreError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(StoreError::Unavailable);
            }
            *self.value.lock().unwrap() = Some(token);
            Ok(())
        }
        fn delete_refresh_token(&self) -> Result<(), StoreError> {
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }
    struct Authority {
        store: Arc<Store>,
        calls: Mutex<Vec<Value>>,
        lose: AtomicBool,
        revoked: AtomicBool,
    }
    impl Transport for Authority {
        fn post(
            &self,
            path: &str,
            tenant: &str,
            owner: Option<&str>,
            body: &Value,
        ) -> Result<Value, MachineError> {
            let raw = self.store.read_refresh_token().unwrap().unwrap();
            let saved: Identity = serde_json::from_str(&raw).unwrap();
            assert!(!body.to_string().contains(&saved.signing_key));
            assert!(!body.to_string().contains(&saved.wireguard_key));
            self.calls.lock().unwrap().push(body.clone());
            if path.is_empty() {
                assert_eq!(owner, Some("owner-session"));
                if self.lose.swap(false, Ordering::SeqCst) {
                    return Err(MachineError::Unavailable);
                }
                let mut reply = body.clone();
                reply["tenant_id"] = json!(tenant);
                reply["authority_id"] = json!("local");
                reply["control_plane_id"] = json!("local");
                reply["client_policy"] = json!("owner-attested-v1");
                reply["status"] = json!("approved");
                return Ok(reply);
            }
            if path.ends_with("/revoke") {
                assert!(saved.revoked && saved.token.is_none());
                return Ok(json!({"revoked":true}));
            }
            assert!(owner.is_none());
            if self.revoked.load(Ordering::SeqCst) {
                return Err(MachineError::Revoked);
            }
            let fields = [
                "device_id",
                "request_id",
                "authority_id",
                "control_plane_id",
                "public_key",
            ];
            let mut parts = vec![
                "hypermesh-machine-identity-v1".to_owned(),
                tenant.to_owned(),
            ];
            parts.extend(fields.map(|f| body[f].as_str().unwrap().to_owned()));
            parts.push(body["issued_at"].as_u64().unwrap().to_string());
            parts.push(body["nonce"].as_str().unwrap().into());
            parts.push(body["next_public_key"].as_str().unwrap().into());
            let sig = Signature::from_slice(
                &STANDARD
                    .decode(body["signature"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            VerifyingKey::from_bytes(&bytes(body["public_key"].as_str().unwrap()).unwrap())
                .unwrap()
                .verify(parts.join("\n").as_bytes(), &sig)
                .unwrap();
            let returned = if body["next_public_key"] != "" {
                body["next_public_key"].clone()
            } else {
                body["public_key"].clone()
            };
            Ok(
                json!({"public_key":returned,"access_token":format!("hm_msi_{}.fixture-secret",body["device_id"].as_str().unwrap()),"token_type":"Bearer","expires_in":600}),
            )
        }
    }
    fn setup() -> (MobileMachine, Arc<Store>, Arc<Authority>) {
        let store = Arc::new(Store::default());
        let authority = Arc::new(Authority {
            store: store.clone(),
            calls: Mutex::new(vec![]),
            lose: AtomicBool::new(false),
            revoked: AtomicBool::new(false),
        });
        let client = MobileMachine {
            store: store.clone(),
            transport: authority.clone(),
            origin: "https://plane.test".into(),
            lock: Mutex::new(()),
        };
        (client, store, authority)
    }
    #[test]
    fn lost_response_reuses_persisted_identity_and_signs_without_owner_session() {
        let (client, store, authority) = setup();
        let tenant = uuid::Uuid::new_v4().to_string();
        authority.lose.store(true, Ordering::SeqCst);
        assert!(matches!(
            client.enroll(tenant.clone(), "android".into(), "owner-session".into()),
            Err(MachineError::Unavailable)
        ));
        let first = client.read().unwrap().unwrap();
        client
            .enroll(tenant, "android".into(), "owner-session".into())
            .unwrap();
        let second = client.read().unwrap().unwrap();
        assert_eq!(first.device_id, second.device_id);
        assert_eq!(first.signing_key, second.signing_key);
        assert!(client.status().unwrap().unwrap().enrolled);
        client.renew().unwrap();
        let calls = authority.calls.lock().unwrap();
        assert_ne!(calls[2]["nonce"], calls[3]["nonce"]);
        assert!(!format!("{:?}", client.status().unwrap()).contains("fixture-secret"));
        drop(calls);
        store.fail.store(true, Ordering::SeqCst);
        let count = authority.calls.lock().unwrap().len();
        assert!(matches!(client.renew(), Err(MachineError::Storage)));
        assert_eq!(count, authority.calls.lock().unwrap().len());
    }
    #[test]
    fn rotation_saves_pending_key_before_request_and_revocation_stops_renewal() {
        let (client, _, authority) = setup();
        client
            .enroll(
                uuid::Uuid::new_v4().to_string(),
                "ios".into(),
                "owner-session".into(),
            )
            .unwrap();
        let before = client.read().unwrap().unwrap().signing_key;
        client.rotate_signing_key().unwrap();
        let after = client.read().unwrap().unwrap();
        assert_ne!(before, after.signing_key);
        assert!(after.pending_key.is_none());
        authority.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(client.renew(), Err(MachineError::Revoked)));
        let count = authority.calls.lock().unwrap().len();
        assert!(matches!(client.renew(), Err(MachineError::Revoked)));
        assert_eq!(count, authority.calls.lock().unwrap().len());
        assert!(client.read().unwrap().unwrap().token.is_none());
        client.revoke("owner-session".into()).unwrap();
        assert!(client.read().unwrap().is_none());
    }
    #[test]
    fn invalid_store_and_origins_fail_before_network() {
        let (client, store, authority) = setup();
        store
            .write_refresh_token("owner-refresh-token-is-not-a-machine".into())
            .unwrap();
        assert!(matches!(client.renew(), Err(MachineError::Invalid)));
        assert!(authority.calls.lock().unwrap().is_empty());
        for value in [
            "http://plane.test",
            "https://user@plane.test",
            "https://plane.test/path",
            "https://plane.test/?key=secret",
        ] {
            assert!(MobileMachine::new(store.clone(), value.into()).is_err());
        }
    }
}
