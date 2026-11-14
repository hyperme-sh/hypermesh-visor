//! Private category decisions. Stored consent alone never authorizes an upload.
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    OsHealth,
    Sensors,
    ServiceHealth,
    Crashes,
    HumanReports,
}
impl Category {
    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub subject: Uuid,
    pub tenant: Uuid,
    pub device: Uuid,
    pub notice_version: Uuid,
    pub purpose_version: Uuid,
}
impl Scope {
    fn valid(&self) -> bool {
        [
            self.subject,
            self.tenant,
            self.device,
            self.notice_version,
            self.purpose_version,
        ]
        .iter()
        .all(|id| !id.is_nil())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    enabled: bool,
    revision: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    scope: Scope,
    categories: [Decision; 5],
}

/// A record may be collected only under this exact category decision. A later
/// opt-in cannot revive a queued record from an earlier decision.
#[derive(Clone, Debug, Serialize)]
pub struct Ticket {
    category: Category,
    revision: Uuid,
    scope: Scope,
}
impl Ticket {
    pub(crate) fn matches_scope(&self, scope: &Scope) -> bool {
        self.scope == *scope
    }
    pub(crate) fn crash_tenant(&self) -> Option<Uuid> {
        (self.category == Category::Crashes).then_some(self.scope.tenant)
    }
}

pub struct Store {
    directory: PathBuf,
    scope: Scope,
    // Deliberately not persisted: restart/offline startup requires fresh sync.
    synchronized: Mutex<[Option<(Uuid, Instant)>; 5]>,
}
/// Held by the process that owns the native crash writer. Stop and reap that
/// writer before dropping this lease or completing withdrawal.
pub struct CrashCaptureLease<'a> {
    store: &'a Store,
    file: fs::File,
    ticket: Ticket,
}
impl CrashCaptureLease<'_> {
    pub fn ticket(&self) -> &Ticket {
        &self.ticket
    }
    /// The caller must already have stopped its writer and purged raw files.
    pub fn withdraw(self) -> io::Result<()> {
        self.store
            .decide_without_capture_guard(Category::Crashes, false)?;
        Ok(())
    }
}
impl Drop for CrashCaptureLease<'_> {
    fn drop(&mut self) {
        drop(storage::UnlockOnDrop(&self.file));
    }
}
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "telemetry consent unavailable",
    )
}
impl Store {
    /// The caller supplies an owner-controlled, dedicated directory and a scope
    /// obtained from trusted enrollment and the displayed notice, never an event.
    pub fn open(directory: &Path, scope: Scope) -> io::Result<Self> {
        if !scope.valid() {
            return Err(refused());
        }
        storage::create_dir(directory)?;
        let store = Self {
            directory: directory.into(),
            scope,
            synchronized: Mutex::new([None; 5]),
        };
        store.locked(|| {
            if store.read()?.is_some_and(|r| r.scope != store.scope) {
                let _capture = store.capture_lock()?;
                store.write_locked(&store.default_record())?;
            }
            Ok(())
        })?;
        Ok(store)
    }
    fn locked<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        storage::check_dir(&self.directory)?;
        let file = storage::open(&self.directory.join("lock"), storage::Open::Lock)?;
        storage::lock(&file)?;
        let _unlock = storage::UnlockOnDrop(&file);
        operation()
    }
    fn read(&self) -> io::Result<Option<Record>> {
        let path = self.directory.join("consent.json");
        let file = match storage::open(&path, storage::Open::Read { private: true }) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(refused());
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| refused())?;
        if record.version != 1
            || !record.scope.valid()
            || record.categories.iter().any(|d| d.revision.is_nil())
        {
            return Err(refused());
        }
        Ok(Some(record))
    }
    fn default_record(&self) -> Record {
        Record {
            version: 1,
            scope: self.scope.clone(),
            categories: std::array::from_fn(|_| Decision {
                enabled: false,
                revision: Uuid::new_v4(),
            }),
        }
    }
    // Caller holds the cross-process lock from reading through replacement.
    fn write_locked(&self, record: &Record) -> io::Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|_| refused())?;
        let temporary = self.directory.join(format!(".{}", Uuid::new_v4()));
        let result = (|| {
            let mut file = storage::open(&temporary, storage::Open::New)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            storage::replace(&temporary, &self.directory.join("consent.json"))?;
            storage::sync_dir(&self.directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    /// Explicit local choice after displaying purpose, destination and retention.
    /// Switching identity or notice resets every category to off.
    pub fn decide(&self, category: Category, enabled: bool) -> io::Result<Uuid> {
        let _capture = if category == Category::Crashes {
            Some(self.capture_lock()?)
        } else {
            None
        };
        self.decide_without_capture_guard(category, enabled)
    }
    fn decide_without_capture_guard(&self, category: Category, enabled: bool) -> io::Result<Uuid> {
        let mut synced = self.synchronized.lock().map_err(|_| refused())?;
        self.locked(|| {
            let current = self.read()?;
            if current.as_ref().is_some_and(|r| r.scope != self.scope) {
                return Err(refused());
            }
            let mut record = current.unwrap_or_else(|| self.default_record());
            let revision = Uuid::new_v4();
            record.categories[category.index()] = Decision { enabled, revision };
            // Revoke the in-process upload authority before persistence is attempted.
            *synced = [None; 5];
            self.write_locked(&record)?;
            Ok(revision)
        })
    }
    pub fn capture(&self, category: Category) -> io::Result<Option<Ticket>> {
        self.locked(|| {
            Ok(self
                .read()?
                .filter(|r| r.scope == self.scope)
                .and_then(|r| {
                    let decision = &r.categories[category.index()];
                    decision.enabled.then(|| Ticket {
                        category,
                        revision: decision.revision,
                        scope: self.scope.clone(),
                    })
                }))
        })
    }
    pub(crate) fn capture_lock(&self) -> io::Result<fs::File> {
        storage::check_dir(&self.directory)?;
        let file = storage::open(
            &self.directory.join("crash-capture.lock"),
            storage::Open::Lock,
        )?;
        storage::lock(&file)?;
        Ok(file)
    }
    /// No native writer may start without this enabled, original decision.
    /// A second writer, crash decision change or scope reset fails while held.
    /// Other categories remain independently editable.
    pub fn capture_lease(&self) -> io::Result<Option<CrashCaptureLease<'_>>> {
        let file = self.capture_lock()?;
        let Some(ticket) = self.capture(Category::Crashes)? else {
            return Ok(None);
        };
        Ok(Some(CrashCaptureLease {
            store: self,
            file,
            ticket,
        }))
    }
    /// Restore only an exact original decision from private capture metadata.
    /// Parsing a record never grants consent or changes its revision.
    pub(crate) fn restore_crash_ticket(&self, bytes: &[u8]) -> io::Result<Ticket> {
        if bytes.len() > 4096 {
            return Err(refused());
        }
        let saved: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| refused())?;
        let ticket = self.capture(Category::Crashes)?.ok_or_else(refused)?;
        if serde_json::to_value(&ticket).map_err(|_| refused())? != saved {
            return Err(refused());
        }
        Ok(ticket)
    }
    /// Called only after the authenticated consent service confirms the exact
    /// subject/tenant/device/category/notice/purpose/revision. Not an HTTP route.
    pub fn synchronized(&self, ticket: &Ticket) -> io::Result<()> {
        let mut synced = self.synchronized.lock().map_err(|_| refused())?;
        self.locked(|| {
            self.check(ticket)?;
            synced[ticket.category.index()] = Some((ticket.revision, Instant::now()));
            Ok(())
        })
    }
    pub(crate) fn validate_capture(&self, ticket: &Ticket) -> io::Result<()> {
        self.locked(|| self.check(ticket))
    }
    /// v0 crash consent is device-local. Hold the decision lock through the
    /// bounded send so withdrawal cannot complete while a send is active.
    pub(crate) fn upload_local_crash<T>(
        &self,
        ticket: &Ticket,
        send: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        if ticket.category != Category::Crashes {
            return Err(refused());
        }
        self.locked(|| {
            self.check(ticket)?;
            send()
        })
    }
    fn check(&self, ticket: &Ticket) -> io::Result<()> {
        let record = self.read()?.ok_or_else(refused)?;
        let decision = &record.categories[ticket.category.index()];
        if ticket.scope != self.scope
            || record.scope != self.scope
            || !decision.enabled
            || decision.revision != ticket.revision
        {
            return Err(refused());
        }
        Ok(())
    }
    /// Serializes one bounded upload with withdrawal, including across processes.
    /// The callback must have a short timeout and must not queue deferred sends.
    /// Withdrawal cannot report success while this callback is still active.
    pub fn upload<T>(
        &self,
        ticket: &Ticket,
        send: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        let mut synced = self.synchronized.lock().map_err(|_| refused())?;
        let Some((revision, confirmed)) = synced[ticket.category.index()].take() else {
            return Err(refused());
        };
        if revision != ticket.revision || confirmed.elapsed() >= Duration::from_secs(5) {
            return Err(refused());
        }
        self.locked(|| {
            self.check(ticket)?;
            send()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("visor-consent-{}", Uuid::new_v4())))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn scope() -> Scope {
        Scope {
            subject: Uuid::new_v4(),
            tenant: Uuid::new_v4(),
            device: Uuid::new_v4(),
            notice_version: Uuid::new_v4(),
            purpose_version: Uuid::new_v4(),
        }
    }
    #[test]
    fn private_capture_metadata_cannot_restore_an_old_or_changed_decision() {
        let dir = Temp::new();
        let store = Store::open(&dir.0, scope()).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let bytes =
            serde_json::to_vec(&store.capture(Category::Crashes).unwrap().unwrap()).unwrap();
        assert!(store.restore_crash_ticket(&bytes).is_ok());
        let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        changed["category"] = serde_json::json!("sensors");
        assert!(store
            .restore_crash_ticket(&serde_json::to_vec(&changed).unwrap())
            .is_err());
        store.decide(Category::Crashes, false).unwrap();
        assert!(store.restore_crash_ticket(&bytes).is_err());
        store.decide(Category::Crashes, true).unwrap();
        assert!(store.restore_crash_ticket(&bytes).is_err());
    }
    #[test]
    fn capture_lease_prevents_successful_withdrawal_until_writer_stops() {
        let dir = Temp::new();
        let original = scope();
        let store = Store::open(&dir.0, original.clone()).unwrap();
        assert!(store.capture_lease().unwrap().is_none());
        store.decide(Category::Crashes, true).unwrap();
        let lease = store.capture_lease().unwrap().unwrap();
        assert!(store.capture_lease().is_err());
        assert!(store.decide(Category::Crashes, false).is_err());
        store.decide(Category::Sensors, true).unwrap();
        assert!(Store::open(&dir.0, scope()).is_err());
        let same = Store::open(&dir.0, original).unwrap();
        assert!(same.capture(Category::Crashes).unwrap().is_some());
        lease.withdraw().unwrap();
        assert!(same.capture(Category::Crashes).unwrap().is_none());
        assert!(same.capture(Category::Sensors).unwrap().is_some());
        let replacement = Store::open(&dir.0, scope()).unwrap();
        assert!(same.decide(Category::Sensors, true).is_err());
        assert!(replacement.capture(Category::Sensors).unwrap().is_none());
    }
    #[test]
    fn default_off_and_independent_categories() {
        let dir = Temp::new();
        let store = Store::open(&dir.0, scope()).unwrap();
        for c in [
            Category::OsHealth,
            Category::Sensors,
            Category::ServiceHealth,
            Category::Crashes,
            Category::HumanReports,
        ] {
            assert!(store.capture(c).unwrap().is_none());
        }
        store.decide(Category::Crashes, true).unwrap();
        assert!(store.capture(Category::Crashes).unwrap().is_some());
        assert!(store.capture(Category::Sensors).unwrap().is_none());
        store.decide(Category::Sensors, true).unwrap();
        store.decide(Category::Crashes, false).unwrap();
        assert!(store.capture(Category::Crashes).unwrap().is_none());
        assert!(store.capture(Category::Sensors).unwrap().is_some());
    }
    #[test]
    fn upload_requires_fresh_single_use_sync_and_restart_resync() {
        let dir = Temp::new();
        let scope = scope();
        let store = Store::open(&dir.0, scope.clone()).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let ticket = store.capture(Category::Crashes).unwrap().unwrap();
        assert!(store
            .upload(&ticket, || -> io::Result<()> { panic!("must not send") })
            .is_err());
        store.synchronized(&ticket).unwrap();
        assert_eq!(store.upload(&ticket, || Ok(7)).unwrap(), 7);
        assert!(store
            .upload(&ticket, || -> io::Result<()> { panic!("must not retry") })
            .is_err());
        store.synchronized(&ticket).unwrap();
        let restarted = Store::open(&dir.0, scope).unwrap();
        assert!(restarted
            .upload(&ticket, || -> io::Result<()> { panic!("must resync") })
            .is_err());
        store.synchronized.lock().unwrap()[Category::Crashes.index()] =
            Some((ticket.revision, Instant::now() - Duration::from_secs(6)));
        assert!(store
            .upload(&ticket, || -> io::Result<()> { panic!("expired sync") })
            .is_err());
    }
    #[test]
    fn withdrawal_and_reenable_cannot_revive_old_records() {
        let dir = Temp::new();
        let scope = scope();
        let first = Store::open(&dir.0, scope.clone()).unwrap();
        first.decide(Category::Crashes, true).unwrap();
        let old = first.capture(Category::Crashes).unwrap().unwrap();
        first.synchronized(&old).unwrap();
        let second = Store::open(&dir.0, scope).unwrap();
        second.decide(Category::Crashes, false).unwrap();
        assert!(first
            .upload(&old, || -> io::Result<()> { panic!("withdrawn") })
            .is_err());
        second.decide(Category::Crashes, true).unwrap();
        assert!(first.synchronized(&old).is_err());
        let new = first.capture(Category::Crashes).unwrap().unwrap();
        assert_ne!(old.revision, new.revision);
    }
    #[test]
    fn identity_or_notice_change_resets_all_categories() {
        for field in 0..5 {
            let dir = Temp::new();
            let old_scope = scope();
            let store = Store::open(&dir.0, old_scope.clone()).unwrap();
            store.decide(Category::Crashes, true).unwrap();
            let old = store.capture(Category::Crashes).unwrap().unwrap();
            let mut new_scope = old_scope;
            *match field {
                0 => &mut new_scope.subject,
                1 => &mut new_scope.tenant,
                2 => &mut new_scope.device,
                3 => &mut new_scope.notice_version,
                _ => &mut new_scope.purpose_version,
            } = Uuid::new_v4();
            let other = Store::open(&dir.0, new_scope).unwrap();
            assert!(other.capture(Category::Crashes).unwrap().is_none());
            assert!(other.synchronized(&old).is_err());
            assert!(store.synchronized(&old).is_err());
            other.decide(Category::Sensors, true).unwrap();
            assert!(other.capture(Category::Crashes).unwrap().is_none());
            assert!(store.synchronized(&old).is_err());
        }
    }
    #[test]
    fn opening_current_scope_preserves_a_completed_opt_in() {
        let dir = Temp::new();
        let old = Store::open(&dir.0, scope()).unwrap();
        old.decide(Category::Crashes, true).unwrap();
        let new_scope = scope();
        let first = Store::open(&dir.0, new_scope.clone()).unwrap();
        assert!(first.capture(Category::Crashes).unwrap().is_none());
        first.decide(Category::Crashes, true).unwrap();
        let ticket = first.capture(Category::Crashes).unwrap().unwrap();
        first
            .locked(|| {
                assert!(Store::open(&dir.0, new_scope.clone()).is_err());
                Ok(())
            })
            .unwrap();
        let second = Store::open(&dir.0, new_scope).unwrap();
        assert_eq!(
            second.capture(Category::Crashes).unwrap().unwrap().revision,
            ticket.revision
        );
        first.synchronized(&ticket).unwrap();
        first.upload(&ticket, || Ok(())).unwrap();
    }
    #[test]
    fn corrupt_storage_fails_closed() {
        let dir = Temp::new();
        let store = Store::open(&dir.0, scope()).unwrap();
        let mut file = storage::open(&dir.0.join("consent.json"), storage::Open::New).unwrap();
        file.write_all(b"not json").unwrap();
        drop(file);
        assert!(store.capture(Category::Crashes).is_err());
        assert!(store.decide(Category::Crashes, true).is_err());
    }
    #[test]
    fn withdrawal_does_not_succeed_during_upload() {
        let dir = Temp::new();
        let scope = scope();
        let first = Store::open(&dir.0, scope.clone()).unwrap();
        let second = Store::open(&dir.0, scope).unwrap();
        first.decide(Category::Crashes, true).unwrap();
        let ticket = first.capture(Category::Crashes).unwrap().unwrap();
        first.synchronized(&ticket).unwrap();
        first
            .upload(&ticket, || {
                assert!(second.decide(Category::Crashes, false).is_err());
                Ok(())
            })
            .unwrap();
        second.decide(Category::Crashes, false).unwrap();
        assert!(first.synchronized(&ticket).is_err());
    }
}
