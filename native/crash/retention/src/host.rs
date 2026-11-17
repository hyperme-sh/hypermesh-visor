//! Host-only authority. This never creates or changes Visor consent.
use crate::{refused, Authority, Owner};
use hypermesh_local_storage as storage;
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const TERMS_VERSION: &str = "hypermesh-terms-inline-2026-11-05-r1";
pub const TERMS_SHA256: &str = "46620be8f650c44e8188b0e6143be0193f70f6bd901a47d3d5169785e66c7045";

pub fn require_root() -> io::Result<()> {
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    Err(refused())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    kind: String,
    run_id: uuid::Uuid,
    state_path: PathBuf,
    device: uuid::Uuid,
    terms_version: String,
    terms_sha256: String,
    accepted_at: String,
}
#[derive(Deserialize)]
struct Enrollment {
    device_id: uuid::Uuid,
    terms_version: String,
    terms_sha256: String,
    accepted_at: String,
}
fn read(path: &Path) -> io::Result<Vec<u8>> {
    if !path.is_absolute() || std::fs::canonicalize(path)? != path {
        return Err(refused());
    }
    storage::check_dir(path.parent().ok_or_else(refused)?)?;
    let mut bytes = Vec::new();
    storage::open(path, storage::Open::Read { private: true })?
        .take(16385)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        return Err(refused());
    }
    Ok(bytes)
}
fn matches(ticket: &Ticket, state: &Enrollment, run: &Path) -> bool {
    ticket.kind == "host-terms-v1"
        && !ticket.run_id.is_nil()
        && !ticket.device.is_nil()
        && run.file_name().and_then(|x| x.to_str()) == Some(ticket.run_id.to_string().as_str())
        && ticket.terms_version == TERMS_VERSION
        && ticket.terms_sha256 == TERMS_SHA256
        && ticket.accepted_at.len() <= 64
        && !ticket.accepted_at.trim().is_empty()
        && state.device_id == ticket.device
        && state.terms_version == ticket.terms_version
        && state.terms_sha256 == ticket.terms_sha256
        && state.accepted_at == ticket.accepted_at
}
/// Check current private enrollment on every write, not just startup.
pub fn validate(run: &Path) -> io::Result<()> {
    require_root()?;
    if run.parent().and_then(Path::parent) != Some(crate::root()?.join("config").as_path())
        || Owner::open()?.authority() != Authority::Host
    {
        return Err(refused());
    }
    let ticket: Ticket =
        serde_json::from_slice(&read(&run.join("host-ticket.json"))?).map_err(|_| refused())?;
    if ticket.state_path.file_name().and_then(|x| x.to_str()) != Some("device.json") {
        return Err(refused());
    }
    let state: Enrollment =
        serde_json::from_slice(&read(&ticket.state_path)?).map_err(|_| refused())?;
    if !matches(&ticket, &state, run) {
        return Err(refused());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    #[ignore = "disposable root container in CI only"]
    fn root_enrollment_contract() {
        assert_eq!(std::env::var("HYPERMESH_HOST_ROOT_CONTRACT").unwrap(), "1");
        require_root().unwrap();
        let root = crate::root().unwrap();
        assert!(!root.exists(), "root fixture must be fresh");
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        Owner::register_host().unwrap();
        assert_eq!(Owner::open().unwrap().authority(), Authority::Host);
        assert!(Owner::register().is_err());
        let directory = root.join("config");
        assert!(crate::require_service(&directory).is_err());
        let runs = directory.join("crash-runs");
        storage::create_dir(&runs).unwrap();
        let id = uuid::Uuid::new_v4();
        let device = uuid::Uuid::new_v4();
        let run = runs.join(id.to_string());
        storage::create_dir(&run).unwrap();
        let state_dir = root.join("unit-state");
        storage::create_dir(&state_dir).unwrap();
        let state_path = state_dir.join("device.json");
        let state = serde_json::json!({"device_id":device,"terms_version":TERMS_VERSION,"terms_sha256":TERMS_SHA256,"accepted_at":"2026-11-17T00:00:00Z"});
        crate::write(&state_path, &state).unwrap();
        crate::write(&run.join("host-ticket.json"),&serde_json::json!({"kind":"host-terms-v1","run_id":id,"state_path":state_path,"device":device,"terms_version":TERMS_VERSION,"terms_sha256":TERMS_SHA256,"accepted_at":"2026-11-17T00:00:00Z"})).unwrap();
        validate(&run).unwrap();
        assert!(
            crate::permit(&run).is_err(),
            "no installed service may authorize raw capture"
        );
        let mut changed = state.clone();
        changed["device_id"] = serde_json::json!(uuid::Uuid::new_v4());
        crate::write(&state_path, &changed).unwrap();
        assert!(validate(&run).is_err());
        crate::write(&state_path, &state).unwrap();
        validate(&run).unwrap();
        std::fs::remove_file(&state_path).unwrap();
        assert!(validate(&run).is_err());
    }
    #[test]
    fn host_ticket_cannot_replace_terms_or_enrollment() {
        let id = uuid::Uuid::new_v4();
        let device = uuid::Uuid::new_v4();
        let run = PathBuf::from("/unit").join(id.to_string());
        let mut ticket = Ticket {
            kind: "host-terms-v1".into(),
            run_id: id,
            state_path: "/unit/device.json".into(),
            device,
            terms_version: TERMS_VERSION.into(),
            terms_sha256: TERMS_SHA256.into(),
            accepted_at: "2026-11-17T00:00:00Z".into(),
        };
        let mut state = Enrollment {
            device_id: device,
            terms_version: TERMS_VERSION.into(),
            terms_sha256: TERMS_SHA256.into(),
            accepted_at: ticket.accepted_at.clone(),
        };
        assert!(matches(&ticket, &state, &run));
        ticket.kind = "visor".into();
        assert!(!matches(&ticket, &state, &run));
        ticket.kind = "host-terms-v1".into();
        state.device_id = uuid::Uuid::new_v4();
        assert!(!matches(&ticket, &state, &run));
        state.device_id = device;
        state.terms_sha256 = "0".repeat(64);
        assert!(!matches(&ticket, &state, &run));
        state.terms_sha256 = TERMS_SHA256.into();
        state.accepted_at = "".into();
        assert!(!matches(&ticket, &state, &run));
    }
}
