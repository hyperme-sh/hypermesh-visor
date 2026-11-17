//! Verification contract only: no IPC endpoint or native writer is enabled.
//! The future Host transport must derive container identity from OS credentials
//! and mount only PublicConfig read-only. Parsing does not authenticate a key.
use crate::{host, refused};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

const DOMAIN: &[u8; 5] = b"HMCD\x01";
pub const REQUEST_SIZE: usize = 101;
pub const RESPONSE_SIZE: usize = REQUEST_SIZE + 64;
pub const MAX_CONFIG_SIZE: usize = 1024;
pub const CHALLENGE_TIMEOUT: Duration = Duration::from_millis(500);

/// Public data only; authenticity must come from a Host-controlled mount.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicConfig {
    version: u8,
    run: Uuid,
    worker: String,
    session: Uuid,
    public_key: [u8; 32],
}
fn worker_id(value: &str) -> io::Result<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(refused());
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * i..2 * i + 2], 16).map_err(|_| refused())?;
    }
    if out == [0; 32] {
        return Err(refused());
    }
    Ok(out)
}
impl PublicConfig {
    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.run.is_nil()
            || self.session.is_nil()
            || self.public_key == [0; 32]
        {
            return Err(refused());
        }
        worker_id(&self.worker)?;
        Ok(())
    }
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_CONFIG_SIZE {
            return Err(refused());
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|_| refused())?;
        config.validate()?;
        Ok(config)
    }
    pub fn to_bytes(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| refused())?;
        if bytes.len() > MAX_CONFIG_SIZE {
            return Err(refused());
        }
        Ok(bytes)
    }
    fn prefix(&self) -> io::Result<[u8; REQUEST_SIZE]> {
        self.validate()?;
        let mut bytes = [0; REQUEST_SIZE];
        bytes[..5].copy_from_slice(DOMAIN);
        bytes[5..21].copy_from_slice(self.run.as_bytes());
        bytes[21..53].copy_from_slice(&worker_id(&self.worker)?);
        bytes[53..69].copy_from_slice(self.session.as_bytes());
        Ok(bytes)
    }
    /// Start a new per-write challenge before any transport operation.
    pub fn challenge(&self) -> io::Result<Pending> {
        let started = Instant::now();
        let mut request = self.prefix()?;
        SystemRandom::new()
            .fill(&mut request[69..])
            .map_err(|_| refused())?;
        Ok(Pending {
            request,
            public_key: self.public_key,
            started,
        })
    }
}
/// Not Clone: verifying consumes the challenge. Never cache a successful permit.
pub struct Pending {
    request: [u8; REQUEST_SIZE],
    public_key: [u8; 32],
    started: Instant,
}
impl Pending {
    pub fn request(&self) -> &[u8; REQUEST_SIZE] {
        &self.request
    }
    pub fn verify(self, response: &[u8]) -> io::Result<()> {
        let started = self.started;
        self.verify_at(response, Instant::now())?;
        if started.elapsed() >= CHALLENGE_TIMEOUT {
            return Err(refused());
        }
        Ok(())
    }
    fn verify_at(self, response: &[u8], now: Instant) -> io::Result<()> {
        if now
            .checked_duration_since(self.started)
            .is_none_or(|age| age >= CHALLENGE_TIMEOUT)
            || response.len() != RESPONSE_SIZE
            || response[..REQUEST_SIZE] != self.request
        {
            return Err(refused());
        }
        UnparsedPublicKey::new(&ED25519, self.public_key)
            .verify(&response[..REQUEST_SIZE], &response[REQUEST_SIZE..])
            .map_err(|_| refused())
    }
}
/// Ephemeral Host signing authority. No key export, persistence or renewal.
/// Existing OS cleanup admission remains authoritative on every response.
pub struct HostSession {
    config: PublicConfig,
    key: Ed25519KeyPair,
    run_path: PathBuf,
    started: Instant,
    revoked: bool,
}
impl HostSession {
    pub fn open(run: &Path, worker: &str) -> io::Result<Self> {
        host::validate(run)?;
        crate::admit_handler(run)?;
        Self::new(run, worker)
    }
    fn new(run: &Path, worker: &str) -> io::Result<Self> {
        worker_id(worker)?;
        let name = run
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(refused)?;
        let id = Uuid::parse_str(name).map_err(|_| refused())?;
        if id.is_nil() || id.to_string() != name {
            return Err(refused());
        }
        let encoded =
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| refused())?;
        let key = Ed25519KeyPair::from_pkcs8(encoded.as_ref()).map_err(|_| refused())?;
        let config = PublicConfig {
            version: 1,
            run: id,
            worker: worker.into(),
            session: Uuid::new_v4(),
            public_key: key
                .public_key()
                .as_ref()
                .try_into()
                .map_err(|_| refused())?,
        };
        Ok(Self {
            config,
            key,
            run_path: run.to_owned(),
            started: Instant::now(),
            revoked: false,
        })
    }
    pub fn public_config(&self) -> PublicConfig {
        self.config.clone()
    }
    pub fn revoke(&mut self) {
        self.revoked = true;
    }
    /// peer_worker must come from verified OS peer/cgroup identity, never from
    /// caller-supplied fields. The future transport must bound reads first.
    pub fn respond(
        &mut self,
        request: &[u8],
        peer_worker: &str,
    ) -> io::Result<[u8; RESPONSE_SIZE]> {
        let path = self.run_path.clone();
        self.respond_with(request, peer_worker, || {
            host::validate(&path)?;
            crate::permit(&path)
        })
    }
    fn respond_with(
        &mut self,
        request: &[u8],
        peer_worker: &str,
        live: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<[u8; RESPONSE_SIZE]> {
        let at = Instant::now();
        let result = (|| {
            if self.revoked
                || self.started.elapsed().as_secs() >= crate::MAX_AGE - crate::HEARTBEAT_AGE
                || peer_worker != self.config.worker
                || request.len() != REQUEST_SIZE
                || request[..69] != self.config.prefix()?[..69]
                || request[69..] == [0; 32]
            {
                return Err(refused());
            }
            live()?;
            let mut response = [0; RESPONSE_SIZE];
            response[..REQUEST_SIZE].copy_from_slice(request);
            response[REQUEST_SIZE..].copy_from_slice(self.key.sign(request).as_ref());
            if at.elapsed() >= CHALLENGE_TIMEOUT {
                return Err(refused());
            }
            Ok(response)
        })();
        if result.is_err() {
            self.revoke();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> HostSession {
        HostSession::new(
            &PathBuf::from("/unit").join(Uuid::new_v4().to_string()),
            &"a1".repeat(32),
        )
        .unwrap()
    }
    fn sign(session: &mut HostSession, pending: &Pending) -> [u8; RESPONSE_SIZE] {
        session
            .respond_with(pending.request(), &"a1".repeat(32), || Ok(()))
            .unwrap()
    }
    #[test]
    fn fresh_challenge_rejects_replayed_response() {
        let mut s = session();
        let config = PublicConfig::from_bytes(&s.public_config().to_bytes().unwrap()).unwrap();
        let pending = config.challenge().unwrap();
        let response = sign(&mut s, &pending);
        pending.verify(&response).unwrap();
        assert!(config.challenge().unwrap().verify(&response).is_err());
    }
    #[test]
    fn altered_domain_scope_nonce_signature_and_key_are_refused() {
        for index in [0, 5, 21, 53, 69, REQUEST_SIZE, RESPONSE_SIZE - 1] {
            let mut s = session();
            let pending = s.public_config().challenge().unwrap();
            let mut response = sign(&mut s, &pending);
            response[index] ^= 1;
            assert!(pending.verify(&response).is_err());
        }
        let mut s = session();
        let mut pending = s.public_config().challenge().unwrap();
        let response = sign(&mut s, &pending);
        pending.public_key = [7; 32];
        assert!(pending.verify(&response).is_err());
    }
    #[test]
    fn stale_and_future_monotonic_challenges_are_refused() {
        for delta in [CHALLENGE_TIMEOUT, Duration::from_secs(10)] {
            let mut s = session();
            let p = s.public_config().challenge().unwrap();
            let response = sign(&mut s, &p);
            let at = p.started + delta;
            assert!(p.verify_at(&response, at).is_err());
        }
        let mut s = session();
        let p = s.public_config().challenge().unwrap();
        let response = sign(&mut s, &p);
        let at = p.started - Duration::from_secs(1);
        assert!(p.verify_at(&response, at).is_err());
    }
    #[test]
    fn every_response_requires_live_service_and_failure_revokes() {
        let mut s = session();
        let config = s.public_config();
        for _ in 0..2 {
            let p = config.challenge().unwrap();
            let mut called = false;
            let response = s
                .respond_with(p.request(), &"a1".repeat(32), || {
                    called = true;
                    Ok(())
                })
                .unwrap();
            assert!(called);
            p.verify(&response).unwrap();
        }
        let p = config.challenge().unwrap();
        assert!(s
            .respond_with(p.request(), &"a1".repeat(32), || Err(refused()))
            .is_err());
        assert!(s
            .respond_with(p.request(), &"a1".repeat(32), || panic!("must not revive"))
            .is_err());
    }
    #[test]
    fn substitution_and_restart_cannot_reuse_authority() {
        let mut s = session();
        let p = s.public_config().challenge().unwrap();
        assert!(s
            .respond_with(p.request(), &"b2".repeat(32), || panic!("wrong peer"))
            .is_err());
        let mut replacement = HostSession::new(&s.run_path, &"a1".repeat(32)).unwrap();
        assert!(replacement
            .respond_with(p.request(), &"a1".repeat(32), || panic!("old session"))
            .is_err());
    }
    #[test]
    fn revoked_or_expired_session_cannot_sign() {
        for expire in [false, true] {
            let mut s = session();
            let p = s.public_config().challenge().unwrap();
            if expire {
                s.started -= Duration::from_secs(crate::MAX_AGE);
            } else {
                s.revoke();
            }
            assert!(s
                .respond_with(p.request(), &"a1".repeat(32), || panic!("revoked"))
                .is_err());
        }
    }
    #[test]
    fn bounded_frames_and_configuration_refuse_invalid_input() {
        for size in [0, REQUEST_SIZE - 1, REQUEST_SIZE + 1, 4096] {
            assert!(session()
                .respond_with(&vec![0; size], &"a1".repeat(32), || panic!("invalid frame"))
                .is_err());
        }
        for size in [0, RESPONSE_SIZE - 1, RESPONSE_SIZE + 1, 4096] {
            assert!(session()
                .public_config()
                .challenge()
                .unwrap()
                .verify(&vec![0; size])
                .is_err());
        }
        assert!(PublicConfig::from_bytes(&vec![b' '; MAX_CONFIG_SIZE + 1]).is_err());
        for worker in [
            "a".repeat(63),
            "A".repeat(64),
            "0".repeat(64),
            "g".repeat(64),
            "é".repeat(32),
        ] {
            assert!(worker_id(&worker).is_err());
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(&session().public_config().to_bytes().unwrap()).unwrap();
        value["extra"] = serde_json::json!(true);
        assert!(PublicConfig::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[test]
    fn public_open_cannot_bypass_host_authority() {
        assert!(HostSession::open(Path::new("/nonexistent/run"), &"a1".repeat(32)).is_err());
    }
}
