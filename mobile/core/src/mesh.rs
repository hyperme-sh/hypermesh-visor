//! Native-only short-lived tunnel configuration. Never expose this object to JS.
use crate::machine::{now, origin, PhoneCredential};
use crate::{MachineError, MobileMachine, MobileOwnerSignIn};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(uniffi::Object)]
pub struct MobileMeshClient {
    machine: Arc<MobileMachine>,
    active: Mutex<()>,
    generation: Arc<AtomicU64>,
}

#[derive(uniffi::Object)]
pub struct MobileMeshLease {
    machine: Arc<MobileMachine>,
    owner: Arc<MobileOwnerSignIn>,
    epoch: u64,
    generation: Arc<AtomicU64>,
    stamp: u64,
    credential: PhoneCredential,
    configuration: String,
    conductor: String,
    expires: u64,
    deadline: Instant,
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, MachineError> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 2048 && !s.chars().any(char::is_control))
        .ok_or(MachineError::Invalid)
}
fn public_key(value: &str) -> Result<(), MachineError> {
    let raw = STANDARD.decode(value).map_err(|_| MachineError::Invalid)?;
    if raw.len() != 32 || raw.iter().all(|b| *b == 0) || STANDARD.encode(raw) != value {
        return Err(MachineError::Invalid);
    }
    Ok(())
}
fn address(value: &str) -> Result<Ipv4Addr, MachineError> {
    let ip: Ipv4Addr = value.parse().map_err(|_| MachineError::Invalid)?;
    if !ip.is_private() {
        return Err(MachineError::Invalid);
    }
    Ok(ip)
}
fn parse(
    value: &Value,
    identity: &PhoneCredential,
    own_public: &str,
    private: &str,
    at: u64,
) -> Result<(String, String, u64), MachineError> {
    for (key, expected) in [
        ("tenant_id", &identity.tenant),
        ("device_id", &identity.device),
        ("request_id", &identity.enrollment),
    ] {
        if field(value, key)? != expected {
            return Err(MachineError::Invalid);
        }
    }
    if field(value, "wg_public_key")? != own_public {
        return Err(MachineError::Invalid);
    }
    let server = field(value, "server_public_key")?;
    public_key(server)?;
    if server == own_public {
        return Err(MachineError::Invalid);
    }
    let conductor = origin(field(value, "conductor_origin")?)?;
    let url = url::Url::parse(&conductor).map_err(|_| MachineError::Invalid)?;
    let destination = address(url.host_str().ok_or(MachineError::Invalid)?)?;
    let assigned = field(value, "address")?;
    let ip = address(assigned.strip_suffix("/32").ok_or(MachineError::Invalid)?)?;
    if ip == destination {
        return Err(MachineError::Invalid);
    }
    let allowed = format!("{destination}/32");
    if value["allowed_ips"] != json!([allowed]) {
        return Err(MachineError::Invalid);
    }
    let endpoint = field(value, "endpoint")?;
    let remote =
        url::Url::parse(&format!("udp://{endpoint}")).map_err(|_| MachineError::Invalid)?;
    if remote.host_str().is_none()
        || remote.port().is_none()
        || !remote.username().is_empty()
        || remote.password().is_some()
        || remote.query().is_some()
        || remote.fragment().is_some()
        || !remote.path().is_empty()
        || endpoint.chars().any(char::is_whitespace)
    {
        return Err(MachineError::Invalid);
    }
    let expires = value["expires_at"].as_u64().ok_or(MachineError::Invalid)?;
    if expires <= at.saturating_add(10) || expires > at.saturating_add(60) {
        return Err(MachineError::Refused);
    }
    let configuration = format!("[Interface]\nPrivateKey = {private}\nAddress = {assigned}\n\n[Peer]\nPublicKey = {server}\nEndpoint = {endpoint}\nAllowedIPs = {allowed}\nPersistentKeepalive = 15\n");
    Ok((configuration, conductor, expires))
}

#[uniffi::export]
impl MobileMeshClient {
    #[uniffi::constructor]
    pub fn new(machine: Arc<MobileMachine>) -> Self {
        Self {
            machine,
            active: Mutex::new(()),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    pub fn owned_sessions(
        &self,
        owner: Arc<MobileOwnerSignIn>,
    ) -> Result<Vec<crate::PhoneSession>, MachineError> {
        let epoch = owner.phone_epoch();
        let credential = self.machine.phone_credential()?;
        owner.with_session(&credential.tenant, epoch, |session| {
            crate::phone::sessions(
                &session.request(self.machine.plane_origin(), "", None)?,
                now()?,
            )
        })
    }
    pub fn prepare(
        &self,
        owner: Arc<MobileOwnerSignIn>,
    ) -> Result<Arc<MobileMeshLease>, MachineError> {
        let _guard = self
            .active
            .try_lock()
            .map_err(|_| MachineError::Unavailable)?;
        let epoch = owner.login_epoch();
        let stamp = self.generation.load(Ordering::SeqCst);
        let credential = self.machine.phone_credential()?;
        let (public, private) = self.machine.mesh_key(&credential)?;
        let value = owner.with_mesh_session(&credential.tenant, epoch, |session| {
            session.mesh_request(
                &self.machine,
                &json!({"device_id":credential.device,
                "request_id":credential.enrollment,"access_token":credential.token}),
            )
        })?;
        let at = now()?;
        let (configuration, conductor, expires) =
            parse(&value, &credential, &public, &private, at)?;
        let lease = Arc::new(MobileMeshLease {
            machine: self.machine.clone(),
            owner,
            epoch,
            credential,
            generation: self.generation.clone(),
            stamp,
            configuration,
            conductor,
            expires,
            deadline: Instant::now() + Duration::from_secs(expires - at),
        });
        lease.check()?;
        Ok(lease)
    }
}
impl MobileMeshLease {
    fn check(&self) -> Result<(), MachineError> {
        if self.owner.login_epoch() != self.epoch
            || self.generation.load(Ordering::SeqCst) != self.stamp
            || now()? >= self.expires
            || Instant::now() >= self.deadline
        {
            return Err(MachineError::Refused);
        }
        self.machine.mesh_key(&self.credential)?;
        Ok(())
    }
}
#[uniffi::export]
impl MobileMeshLease {
    /// Native adapters only. Do not put this value in a Capacitor result or VPN profile.
    pub fn wireguard_configuration(&self) -> Result<String, MachineError> {
        self.check()?;
        Ok(self.configuration.clone())
    }
    pub fn conductor_origin(&self) -> Result<String, MachineError> {
        self.check()?;
        Ok(self.conductor.clone())
    }
    pub fn expires_at(&self) -> i64 {
        self.expires as i64
    }
    pub fn valid(&self) -> bool {
        self.check().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, PhoneCredential, String) {
        let own = STANDARD.encode([1u8; 32]);
        let identity = PhoneCredential {
            tenant: "tenant".into(),
            device: "device".into(),
            enrollment: "enrollment".into(),
            token: "native-token".into(),
        };
        (
            json!({"tenant_id":"tenant","device_id":"device","request_id":"enrollment",
            "wg_public_key":own,"server_public_key":STANDARD.encode([2u8;32]),"address":"10.90.0.2/32",
            "endpoint":"gateway.example:51821","allowed_ips":["10.77.0.9/32"],
            "conductor_origin":"https://10.77.0.9:9443","expires_at":1060}),
            identity,
            own,
        )
    }
    #[test]
    fn builds_only_the_bound_native_tunnel() {
        let (value, identity, own) = fixture();
        let (config, origin, expires) = parse(&value, &identity, &own, "private", 1000).unwrap();
        assert!(config.contains("PrivateKey = private\n"));
        assert!(config.contains("AllowedIPs = 10.77.0.9/32\n"));
        assert!(!config.contains("native-token"));
        assert_eq!(origin, "https://10.77.0.9:9443");
        assert_eq!(expires, 1060);
    }
    #[test]
    fn refuses_widening_mismatch_and_expiry() {
        let (value, identity, own) = fixture();
        for (key, bad) in [
            ("tenant_id", json!("other")),
            ("device_id", json!("other")),
            ("request_id", json!("other")),
            ("wg_public_key", json!("other")),
            ("allowed_ips", json!(["0.0.0.0/0"])),
            ("address", json!("10.90.0.2/24")),
            ("endpoint", json!("gateway.example:51821\nDNS = 1.1.1.1")),
            ("conductor_origin", json!("http://10.77.0.9")),
            ("expires_at", json!(1005)),
            ("expires_at", json!(1061)),
        ] {
            let mut changed = value.clone();
            changed[key] = bad;
            assert!(
                parse(&changed, &identity, &own, "private", 1000).is_err(),
                "{key}"
            );
        }
    }
}
