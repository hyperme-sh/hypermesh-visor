//! Short host-controlled leases for managed guests. A lease is not an enrolled
//! identity or a remote grant; normal session authentication still applies.
use crate::companion_manager::{Error, Scope};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub version: u8,
    pub companion: Uuid,
    pub session: Uuid,
    pub generation: Uuid,
    pub run: Option<Uuid>,
    pub expires_ms: u64,
}
pub fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}
impl Lease {
    pub fn new(scope: Scope, run: Option<Uuid>) -> Result<Self, Error> {
        Ok(Self {
            version: 1,
            companion: scope.companion(),
            session: scope.session(),
            generation: scope.generation(),
            run,
            expires_ms: now_ms()
                .and_then(|n| n.checked_add(1000))
                .ok_or(Error::Backend)?,
        })
    }
    fn validate(&self, scope: Scope, now: u64) -> bool {
        self.version == 1
            && self.companion == scope.companion()
            && self.session == scope.session()
            && self.generation == scope.generation()
            && self.expires_ms > now
            && self.expires_ms - now <= 1000
            && self.run.is_none_or(|run| !run.is_nil())
    }
}
#[derive(Clone)]
pub struct Guest {
    scope: Scope,
    path: PathBuf,
}
impl Guest {
    pub fn from_env() -> Result<Option<Self>, Error> {
        let keys = [
            "HYPERMESH_MANAGED_COMPANION",
            "HYPERMESH_MANAGED_SESSION",
            "HYPERMESH_MANAGED_GENERATION",
        ];
        let values: Vec<_> = keys.iter().map(std::env::var).collect();
        if values
            .iter()
            .all(|v| matches!(v, Err(std::env::VarError::NotPresent)))
        {
            return Ok(None);
        }
        let ids: Result<Vec<_>, _> = values
            .iter()
            .map(|value| {
                value
                    .as_ref()
                    .ok()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .filter(|id| !id.is_nil())
                    .ok_or(Error::Invalid)
            })
            .collect();
        let ids = ids?;
        Ok(Some(Self {
            scope: Scope::recovered(ids[0], ids[1], ids[2]),
            path: PathBuf::from("/run/manager/lease.json"),
        }))
    }
    #[cfg(test)]
    pub(crate) fn fixture(scope: Scope, path: PathBuf) -> Self {
        Self { scope, path }
    }
    pub fn session(&self) -> Uuid {
        self.scope.session()
    }
    pub fn read(&self) -> Result<Lease, Error> {
        read(&self.path, self.scope)
    }
    pub fn active(&self) -> Option<Uuid> {
        self.read().ok()?.run
    }
}
fn read(path: &Path, scope: Scope) -> Result<Lease, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| Error::MonitorUnavailable)?;
    let metadata = file.metadata().map_err(|_| Error::MonitorUnavailable)?;
    if !metadata.is_file() || metadata.len() > 2048 {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(2049)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::MonitorUnavailable)?;
    let lease: Lease = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if !lease.validate(scope, now_ms().ok_or(Error::MonitorUnavailable)?) {
        return Err(Error::MonitorUnavailable);
    }
    Ok(lease)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_binds_scope_and_expires_without_an_observer_tick() {
        let scope = Scope::recovered(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let lease = Lease::new(scope, Some(Uuid::new_v4())).unwrap();
        assert!(lease.validate(scope, lease.expires_ms - 500));
        assert!(!lease.validate(scope, lease.expires_ms));
        assert!(!lease.validate(scope, lease.expires_ms - 1001));
        assert!(!lease.validate(
            Scope::recovered(scope.companion(), scope.session(), Uuid::new_v4()),
            lease.expires_ms - 500
        ));
    }
}
