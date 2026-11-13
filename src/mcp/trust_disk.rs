//! Private, atomic trust receipts. Never writes server definitions or secrets.
use super::McpError;
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[cfg(test)]
use std::{
    fs::OpenOptions,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub path: PathBuf,
    pub sha256: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub review_id: Uuid,
    pub revoked: bool,
    pub source_kind: String,
    pub sources: Vec<Fingerprint>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub version: u32,
    pub profiles: BTreeMap<String, Receipt>,
}
const LIMIT: u64 = 128 * 1024;
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn error() -> McpError {
    McpError::message(
        "MCP trust storage unavailable or unsafe; repair its private files before opening sessions",
    )
}
fn directory(base: &Path) -> PathBuf {
    base.join("mcp-trust")
}
fn check_dir(path: &Path) -> Result<(), McpError> {
    storage::check_dir(path).map_err(|_| error())
}
fn read_file(path: &Path) -> Result<Vec<u8>, McpError> {
    let file = storage::open(path, storage::Open::Read { private: true }).map_err(|_| error())?;
    let mut data = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|_| error())?;
    if data.len() as u64 > LIMIT {
        return Err(error());
    }
    Ok(data)
}
pub fn read(base: &Path) -> Result<Option<Vec<u8>>, McpError> {
    let dir = directory(base);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(error()),
        Ok(_) => check_dir(&dir)?,
    }
    // An existing directory with a missing ledger is damage, not a reset to default trust.
    read_file(&dir.join("approvals.json")).map(Some)
}
pub fn decode(base: &Path, bytes: &[u8]) -> Result<Ledger, McpError> {
    let ledger: Ledger = serde_json::from_slice(bytes).map_err(|_| error())?;
    if ledger.version != 1 || ledger.profiles.len() > 8 {
        return Err(error());
    }
    for (name, r) in &ledger.profiles {
        if name.is_empty()
            || name.len() > 512
            || !(2..=3).contains(&r.sources.len())
            || !super::sources::valid_source_kind(&r.source_kind)
        {
            return Err(error());
        }
        if r.sources[0].path != base.join("mcp.json")
            || r.sources[1].path != base.join("mcp-profiles.json")
        {
            return Err(error());
        }
        for source in &r.sources {
            if !source.path.is_absolute()
                || source.path.as_os_str().len() > 4096
                || source
                    .sha256
                    .as_ref()
                    .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(error());
            }
        }
    }
    Ok(ledger)
}
pub(super) use storage::UnlockOnDrop;
pub fn save(base: &Path, expected: Option<&[u8]>, ledger: &Ledger) -> Result<Vec<u8>, McpError> {
    let dir = directory(base);
    fs::create_dir_all(base).map_err(|_| error())?;
    let created = storage::create_dir(&dir).map_err(|_| error())?;
    let lock = storage::open(&dir.join("lock"), storage::Open::Lock).map_err(|_| error())?;
    storage::lock(&lock).map_err(|_| McpError::message("MCP trust is being updated; retry"))?;
    let _unlock = UnlockOnDrop(&lock);
    let current = if created { None } else { read(base)? };
    if current.as_deref() != expected {
        return Err(McpError::message(
            "MCP trust changed in another process; refresh and review again",
        ));
    }
    let data = serde_json::to_vec_pretty(ledger).map_err(|_| error())?;
    if data.len() as u64 > LIMIT {
        return Err(error());
    }
    decode(base, &data)?;
    let path = dir.join(format!(".{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = storage::open(&path, storage::Open::New).map_err(|_| error())?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|_| error())?;
        storage::replace(&path, &dir.join("approvals.json")).map_err(|_| error())?;
        storage::sync_dir(&dir).map_err(|_| error())?;
        Ok(data)
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard_releases_lock_even_while_an_inherited_descriptor_remains_open() {
        let path = std::env::temp_dir().join(format!("visor-unlock-{}", Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        // dup shares the open-file description, just like a forked child.
        let inherited = file.try_clone().unwrap();
        let guard = UnlockOnDrop(&file);
        drop(guard);
        drop(file);
        let contender = OpenOptions::new().write(true).open(&path).unwrap();
        assert_eq!(
            unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let _unlock = UnlockOnDrop(&contender);
        drop(inherited);
        fs::remove_file(path).unwrap();
    }
}
