//! Write-ahead ownership records for private companion desktops.
//! Recovery stops old desktops before any replacement is admitted. It never
//! restores a running permit or silently attaches a retained home to a new bot.
use crate::companion_manager::{Allocation, Error, Kind, Resources, Scope, MAX_COMPANIONS};
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 65536;
const MAX_RETAINED: usize = 128;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owned {
    pub companion: Uuid,
    pub session: Uuid,
    pub generation: Uuid,
    pub memory_mib: u32,
    pub cpu_millis: u32,
}
impl Owned {
    pub fn scope(self) -> Scope {
        Scope::recovered(self.companion, self.session, self.generation)
    }
    pub fn container(self) -> String {
        format!("hypermesh-companion-{}", self.generation)
    }
    pub fn home(self) -> String {
        format!("hypermesh-home-{}", self.generation)
    }
    fn valid(self) -> bool {
        !self.companion.is_nil()
            && !self.session.is_nil()
            && !self.generation.is_nil()
            && self.memory_mib > 0
            && self.cpu_millis > 0
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    owner: Uuid,
    active: Vec<Owned>,
    retained: Vec<Owned>,
}
impl Record {
    fn valid(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        self.version == 1
            && !self.owner.is_nil()
            && self.active.len() < MAX_COMPANIONS
            && self.retained.len() <= MAX_RETAINED
            && self.active.iter().chain(&self.retained).all(|a| {
                a.valid()
                    && ids.insert(a.companion)
                    && ids.insert(a.session)
                    && ids.insert(a.generation)
            })
    }
}
/// The engine must check the owner and all three scope labels before stopping
/// or deleting anything. Absence is success; uncertainty and foreign ownership
/// are errors. Removing a desktop must acknowledge that it cannot act again.
pub trait Cleanup {
    fn remove_desktop(&mut self, owner: Uuid, allocation: Owned) -> Result<(), Error>;
    fn remove_home(&mut self, owner: Uuid, allocation: Owned) -> Result<(), Error>;
}

pub struct Journal {
    directory: PathBuf,
    _lock: File,
    record: Record,
    failed: bool,
}
impl Journal {
    pub fn open(directory: &Path) -> Result<Self, Error> {
        storage::create_dir(directory).map_err(|_| Error::Backend)?;
        storage::check_dir(directory).map_err(|_| Error::Backend)?;
        let file = storage::open(&directory.join("manager.lock"), storage::Open::Lock)
            .map_err(|_| Error::Backend)?;
        storage::lock(&file).map_err(|_| Error::Backend)?;
        let path = directory.join("allocations.json");
        let record = match storage::open(&path, storage::Open::Read { private: true }) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Backend)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err(Error::Invalid);
                }
                serde_json::from_slice::<Record>(&bytes).map_err(|_| Error::Invalid)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record {
                version: 1,
                owner: Uuid::new_v4(),
                active: Vec::new(),
                retained: Vec::new(),
            },
            Err(_) => return Err(Error::Backend),
        };
        if !record.valid() {
            return Err(Error::Invalid);
        }
        let mut journal = Self {
            directory: directory.into(),
            _lock: file,
            record,
            failed: false,
        };
        journal.save()?;
        Ok(journal)
    }
    pub fn owner(&self) -> Uuid {
        self.record.owner
    }
    pub fn active(&self) -> &[Owned] {
        &self.record.active
    }
    pub fn retained(&self) -> &[Owned] {
        &self.record.retained
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn save(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Backend);
        }
        let result = (|| {
            let bytes = serde_json::to_vec(&self.record).map_err(|_| Error::Backend)?;
            if bytes.len() as u64 > MAX_BYTES || !self.record.valid() {
                return Err(Error::Invalid);
            }
            let temp = self
                .directory
                .join(format!(".allocations-{}", Uuid::new_v4()));
            let result = (|| -> std::io::Result<()> {
                let mut file = storage::open(&temp, storage::Open::New)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                storage::replace(&temp, &self.directory.join("allocations.json"))?;
                storage::sync_dir(&self.directory)
            })();
            let _ = std::fs::remove_file(temp);
            result.map_err(|_| Error::Backend)
        })();
        // An uncertain commit may have reached disk. This instance cannot
        // perform further side effects; restart reconciles the durable record.
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn reserve(&mut self, allocation: Allocation) -> Result<Owned, Error> {
        if self.failed {
            return Err(Error::Backend);
        }
        if allocation.kind != Kind::Container {
            return Err(Error::Invalid);
        }
        if self.record.active.len() >= MAX_COMPANIONS - 1 {
            return Err(Error::Limit);
        }
        // Always leave enough room to preserve every active home after a crash.
        if self.record.active.len() + self.record.retained.len() >= MAX_RETAINED {
            return Err(Error::Capacity);
        }
        let owned = Owned {
            companion: allocation.scope.companion(),
            session: allocation.scope.session(),
            generation: allocation.scope.generation(),
            memory_mib: allocation.resources.memory_mib,
            cpu_millis: allocation.resources.cpu_millis,
        };
        let mut next = self.record.clone();
        next.active.push(owned);
        if !next.valid() {
            return Err(Error::Invalid);
        }
        self.record = next;
        self.save()?;
        Ok(owned)
    }
    pub fn stop(
        &mut self,
        engine: &mut impl Cleanup,
        scope: Scope,
        retain: bool,
    ) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Backend);
        }
        let index = self
            .record
            .active
            .iter()
            .position(|a| a.scope() == scope)
            .ok_or(Error::Stale)?;
        let owned = self.record.active[index];
        engine.remove_desktop(self.owner(), owned)?;
        if !retain {
            engine.remove_home(self.owner(), owned)?;
        }
        self.record.active.remove(index);
        if retain {
            self.record.retained.push(owned);
        }
        self.save()
    }
    pub fn recover(&mut self, engine: &mut impl Cleanup) -> Result<(), Error> {
        let scopes: Vec<_> = self.record.active.iter().map(|a| a.scope()).collect();
        // Try every desktop even if one engine operation fails; failed records
        // remain reserved and the caller must refuse replacement admission.
        let mut failed = false;
        for scope in scopes {
            failed |= self.stop(engine, scope, true).is_err();
        }
        if failed {
            Err(Error::Backend)
        } else {
            Ok(())
        }
    }
    pub fn delete_retained(
        &mut self,
        engine: &mut impl Cleanup,
        scope: Scope,
    ) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Backend);
        }
        let index = self
            .record
            .retained
            .iter()
            .position(|a| a.scope() == scope)
            .ok_or(Error::Stale)?;
        engine.remove_home(self.owner(), self.record.retained[index])?;
        self.record.retained.remove(index);
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("visor-recovery-{}", Uuid::new_v4())))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[derive(Default)]
    struct Engine {
        stopped: Vec<Scope>,
        deleted: Vec<Scope>,
        fail: Option<Scope>,
    }
    impl Cleanup for Engine {
        fn remove_desktop(&mut self, _: Uuid, a: Owned) -> Result<(), Error> {
            self.stopped.push(a.scope());
            if self.fail == Some(a.scope()) {
                Err(Error::Backend)
            } else {
                Ok(())
            }
        }
        fn remove_home(&mut self, _: Uuid, a: Owned) -> Result<(), Error> {
            self.deleted.push(a.scope());
            Ok(())
        }
    }
    fn allocation() -> Allocation {
        Allocation {
            scope: Scope::recovered(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()),
            kind: Kind::Container,
            resources: Resources {
                memory_mib: 2048,
                cpu_millis: 1000,
            },
        }
    }
    #[test]
    fn interrupted_start_is_recovered_without_reusing_home_or_authority() {
        let temp = Temp::new();
        let mut journal = Journal::open(&temp.0).unwrap();
        let owner = journal.owner();
        let a = journal.reserve(allocation()).unwrap();
        drop(journal); // Simulate process loss before or after the engine call.
        let mut journal = Journal::open(&temp.0).unwrap();
        assert_eq!(journal.owner(), owner);
        assert_eq!(journal.active(), &[a]);
        let mut engine = Engine::default();
        journal.recover(&mut engine).unwrap();
        assert_eq!(engine.stopped, vec![a.scope()]);
        assert!(engine.deleted.is_empty());
        assert!(journal.active().is_empty());
        assert_eq!(journal.retained(), &[a]);
        assert_ne!(journal.reserve(allocation()).unwrap().scope(), a.scope());
    }
    #[test]
    fn failed_recovery_keeps_slot_and_continues_other_cleanup() {
        let temp = Temp::new();
        let mut journal = Journal::open(&temp.0).unwrap();
        let a = journal.reserve(allocation()).unwrap();
        let b = journal.reserve(allocation()).unwrap();
        let mut engine = Engine {
            fail: Some(a.scope()),
            ..Engine::default()
        };
        assert_eq!(journal.recover(&mut engine), Err(Error::Backend));
        assert_eq!(engine.stopped, vec![a.scope(), b.scope()]);
        assert_eq!(journal.active(), &[a]);
        assert_eq!(journal.retained(), &[b]);
        engine.fail = None;
        journal.recover(&mut engine).unwrap();
        assert!(journal.active().is_empty());
    }
    #[test]
    fn exclusive_manager_and_explicit_home_deletion() {
        let temp = Temp::new();
        let mut journal = Journal::open(&temp.0).unwrap();
        assert!(Journal::open(&temp.0).is_err());
        let a = journal.reserve(allocation()).unwrap();
        let b = journal.reserve(allocation()).unwrap();
        let mut engine = Engine::default();
        journal.stop(&mut engine, a.scope(), false).unwrap();
        assert_eq!(engine.deleted, vec![a.scope()]);
        assert_eq!(journal.active(), &[b]);
        assert_eq!(
            journal.stop(&mut engine, a.scope(), true),
            Err(Error::Stale)
        );
        journal.stop(&mut engine, b.scope(), true).unwrap();
        journal.delete_retained(&mut engine, b.scope()).unwrap();
        assert!(journal.retained().is_empty());
    }
    #[test]
    fn malformed_oversized_and_foreign_records_are_refused() {
        let temp = Temp::new();
        drop(Journal::open(&temp.0).unwrap());
        let path = temp.0.join("allocations.json");
        for bytes in [b"{}".to_vec(), vec![b' '; MAX_BYTES as usize + 1]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(Journal::open(&temp.0).is_err());
        }
    }
}
