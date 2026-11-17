//! Owner-wide admission and bounded local retention. No network access.
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub mod host;
pub mod delegation;
mod ipc;
pub mod platform;
pub const MAX_AGE: u64 = 7 * 24 * 60 * 60;
pub const MAX_BYTES: u64 = 160 * 1024 * 1024;
pub const HEARTBEAT_AGE: u64 = 2;
const MAX_ENTRIES: usize = 4096;
pub fn refused() -> io::Error {
    io::Error::other("installed crash cleanup service unavailable or private retention refused")
}
pub fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| refused())
}
fn private(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(refused());
    }
    #[cfg(unix)]
    if fs::canonicalize(path)? != path {
        return Err(refused());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        for part in path.ancestors() {
            if fs::symlink_metadata(part)?.file_attributes() & 0x400 != 0 {
                return Err(refused());
            }
        }
    }
    storage::check_dir(path)
}
fn read<T: for<'a> Deserialize<'a>>(path: &Path) -> io::Result<T> {
    let mut bytes = Vec::new();
    storage::open(path, storage::Open::Read { private: true })?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(refused());
    }
    serde_json::from_slice(&bytes).map_err(|_| refused())
}
fn write<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path.parent().ok_or_else(refused)?;
    private(parent)?;
    let temporary = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = storage::open(&temporary, storage::Open::New)?;
    file.write_all(&serde_json::to_vec(value).map_err(|_| refused())?)?;
    file.sync_all()?;
    drop(file);
    storage::replace(&temporary, path)?;
    storage::sync_dir(parent)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    directory: PathBuf,
    #[serde(default, skip_serializing_if = "Authority::is_visor")]
    authority: Authority,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pulse {
    time: u64,
    pid: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    started: u64,
}

/// Fixed OS account profile, never HOME/USERPROFILE or an application override.
pub fn root() -> io::Result<PathBuf> {
    let home = platform::home()?;
    #[cfg(unix)]
    let home = fs::canonicalize(home)?;
    Ok(home.join(".hypermesh-crash-retention"))
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    #[default]
    Visor,
    Host,
}
impl Authority {
    fn is_visor(&self) -> bool {
        *self == Self::Visor
    }
}
pub struct Owner {
    authority: Authority,
    root: PathBuf,
    directory: PathBuf,
}
impl Owner {
    pub fn open() -> io::Result<Self> {
        Self::at(root()?)
    }
    fn at(root: PathBuf) -> io::Result<Self> {
        private(&root)?;
        let registration: Registration = read(&root.join("directory.json"))?;
        // The OS-profile location is the authority. Account-owned registration
        // bytes cannot redirect admission away from the running service's root.
        if registration.directory != root.join("config") {
            return Err(refused());
        }
        private(&registration.directory)?;
        if registration.authority == Authority::Host {
            host::require_root()?;
        }
        Ok(Self {
            authority: registration.authority,
            root,
            directory: registration.directory,
        })
    }
    /// Registration cannot silently move the quota to another directory.
    pub fn register() -> io::Result<()> {
        let root = root()?;
        storage::create_dir(&root)?;
        let directory = root.join("config");
        storage::create_dir(&directory)?;
        Self::register_at(root, &directory)
    }
    pub fn authority(&self) -> Authority {
        self.authority
    }
    pub fn register_host() -> io::Result<()> {
        host::require_root()?;
        let root = root()?;
        storage::create_dir(&root)?;
        let directory = root.join("config");
        storage::create_dir(&directory)?;
        Self::register_authority(root, &directory, Authority::Host)
    }
    fn register_at(root: PathBuf, directory: &Path) -> io::Result<()> {
        Self::register_authority(root, directory, Authority::Visor)
    }
    fn register_authority(root: PathBuf, directory: &Path, authority: Authority) -> io::Result<()> {
        if directory != root.join("config") {
            return Err(refused());
        }
        private(directory)?;
        storage::create_dir(&directory.join("health-consent"))?;
        storage::create_dir(&root)?;
        private(&root)?;
        let lock = lock(&root.join("registration.lock"))?;
        let _unlock = storage::UnlockOnDrop(&lock);
        let path = root.join("directory.json");
        if path.try_exists()? {
            let old: Registration = read(&path)?;
            if old.directory != directory || old.authority != authority {
                return Err(refused());
            }
        } else {
            write(
                &path,
                &Registration {
                    authority,
                    directory: directory.to_owned(),
                },
            )?;
        }
        Ok(())
    }
    pub fn require(&self, directory: &Path) -> io::Result<()> {
        match self.authority {
            Authority::Visor => self.require_with(directory, now, platform::verify_service)?,
            Authority::Host => self.require_with(directory, now, platform::verify_host_service)?,
        }
        ipc::request(&self.root, uuid::Uuid::nil(), self.authority)
    }
    fn require_with(
        &self,
        directory: &Path,
        clock: impl Fn() -> io::Result<u64>,
        verify: impl Fn(u32) -> io::Result<()>,
    ) -> io::Result<()> {
        private(&self.root)?;
        private(directory)?;
        if directory != self.directory {
            return Err(refused());
        }
        // An existing file is not enough: the service must still hold its OS lock.
        let lease = storage::open(
            &self.root.join("service.lock"),
            storage::Open::Read { private: true },
        )?;
        match storage::lock(&lease) {
            Ok(()) => {
                drop(storage::UnlockOnDrop(&lease));
                return Err(refused());
            }
            Err(e) if contended(&e) => (),
            Err(_) => return Err(refused()),
        }
        let pulse: Pulse = read(&self.root.join("heartbeat.json"))?;
        verify(pulse.pid)?;
        if clock()?
            .checked_sub(pulse.time)
            .filter(|age| *age <= HEARTBEAT_AGE)
            .is_none()
        {
            return Err(refused());
        }
        Ok(())
    }
    pub fn service(self, mut stop: impl FnMut() -> bool) -> io::Result<()> {
        let lease = lock(&self.root.join("service.lock"))?;
        let _unlock = storage::UnlockOnDrop(&lease);
        // Never accept a stale heartbeat from a previous process.
        let heartbeat = self.root.join("heartbeat.json");
        let initial = now()?;
        let mut last = initial;
        if heartbeat.try_exists()? {
            // A restart must not forget an observed clock rollback.
            let prior: Pulse = read(&heartbeat)?;
            last = last.max(prior.time);
            fs::remove_file(&heartbeat)?;
        }
        let server = ipc::Server::bind(&self.root)?;
        let mut ages = Ages::default();
        // Unknown pre-existing runs cannot inherit a new age after restart.
        while !self.tick_with(now, true)? {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        loop {
            let time = now()?;
            let stopping = stop() || time < last;
            ages.expire(&self.directory, time)?;
            let ready = self.tick_ages(now, stopping, Some(&ages))?;
            if ready && stopping {
                return Ok(());
            }
            if !stopping {
                last = time;
            }
            server.poll(|id| {
                if !ready || stopping {
                    return false;
                }
                if id.is_nil() {
                    return true;
                }
                ages.admit(&self.directory, id, now().unwrap_or(0)).is_ok()
            })?;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(test)]
    fn tick(&self, time: u64, stopping: bool) -> io::Result<bool> {
        self.tick_with(|| Ok(time), stopping)
    }
    fn tick_with(&self, clock: impl Fn() -> io::Result<u64>, stopping: bool) -> io::Result<bool> {
        self.tick_ages(clock, stopping, None)
    }
    fn tick_ages(
        &self,
        clock: impl Fn() -> io::Result<u64>,
        stopping: bool,
        ages: Option<&Ages>,
    ) -> io::Result<bool> {
        let heartbeat = self.root.join("heartbeat.json");
        match lock(&self.directory.join("health-consent/crash-capture.lock")) {
            Ok(writer) => {
                let _unlock = storage::UnlockOnDrop(&writer);
                let runs = runs(&self.directory)?;
                if stopping {
                    purge(&runs, &self.directory.join("crash-runs"))?;
                } else {
                    if let Some(ages) = ages {
                        for run in &runs {
                            let id = uuid::Uuid::parse_str(
                                run.file_name()
                                    .and_then(|s| s.to_str())
                                    .ok_or_else(refused)?,
                            )
                            .map_err(|_| refused())?;
                            if !ages.0.contains_key(&id) {
                                request_stop(run)?;
                            }
                        }
                    }
                    prune_locked(&self.directory, clock()?)?;
                }
            }
            Err(e) if contended(&e) => {
                // Initialization holds the writer lock before publishing its
                // stamp. Only a ready handler can be expired on this path.
                let mut blocked = stopping;
                for run in runs(&self.directory)? {
                    if !run.join("handler-active").try_exists()? {
                        continue;
                    }
                    if stopping
                        || !fresh(&run, clock()?)
                        || run.join("stop-handler").try_exists()?
                    {
                        request_stop(&run)?;
                        blocked = true;
                    }
                }
                if blocked {
                    if heartbeat.try_exists()? {
                        fs::remove_file(&heartbeat)?;
                    }
                    return Ok(false);
                }
            }
            Err(e) => return Err(e),
        }
        if !stopping {
            write(
                &heartbeat,
                &Pulse {
                    time: clock()?,
                    pid: std::process::id(),
                },
            )?;
        } else if heartbeat.try_exists()? {
            fs::remove_file(&heartbeat)?;
        }
        Ok(true)
    }
}

/// First admission is held only in service memory. Mutable disk timestamps can
/// cause earlier refusal, but cannot reset the authoritative age. Restart purges
/// all unknown runs before publishing readiness.
#[derive(Default)]
struct Ages(std::collections::HashMap<uuid::Uuid, (u64, std::time::Instant)>);
impl Ages {
    fn admit(&mut self, directory: &Path, id: uuid::Uuid, time: u64) -> io::Result<()> {
        let run = directory.join("crash-runs").join(id.to_string());
        private(&run)?;
        if run.join("stop-handler").try_exists()? {
            return Err(refused());
        }
        if let Some((started, monotonic)) = self.0.get(&id) {
            if time
                .checked_sub(*started)
                .is_none_or(|age| age >= MAX_AGE - HEARTBEAT_AGE)
                || monotonic.elapsed().as_secs() >= MAX_AGE - HEARTBEAT_AGE
            {
                return Err(refused());
            }
        } else {
            if time == 0
                || self.0.len() >= MAX_ENTRIES
                || run.join("handler-active").try_exists()?
            {
                return Err(refused());
            }
            let (mut entries, mut count, mut bytes) = (0, 0, 0);
            inventory(&run, 0, &mut entries, &mut count, &mut bytes)?;
            if count != 0 || bytes != 0 {
                return Err(refused());
            }
            self.0.insert(id, (time, std::time::Instant::now()));
        }
        Ok(())
    }
    fn expire(&self, directory: &Path, time: u64) -> io::Result<()> {
        for run in runs(directory)? {
            let id = uuid::Uuid::parse_str(
                run.file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(refused)?,
            )
            .map_err(|_| refused())?;
            let expired = match self.0.get(&id) {
                Some((started, monotonic)) => {
                    time.checked_sub(*started)
                        .is_none_or(|age| age >= MAX_AGE - HEARTBEAT_AGE)
                        || monotonic.elapsed().as_secs() >= MAX_AGE - HEARTBEAT_AGE
                }
                None => run.join("handler-active").try_exists()?,
            };
            if expired {
                request_stop(&run)?;
            }
        }
        Ok(())
    }
}

fn contended(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        error.kind() == io::ErrorKind::WouldBlock
    }
}
fn lock(path: &Path) -> io::Result<File> {
    private(path.parent().ok_or_else(refused)?)?;
    let file = storage::open(path, storage::Open::Lock)?;
    storage::lock(&file)?;
    Ok(file)
}
pub fn require_service(directory: &Path) -> io::Result<()> {
    let owner = Owner::open()?;
    if owner.authority != Authority::Visor {
        return Err(refused());
    }
    owner.require(directory)
}
pub fn stamp(run: &Path) -> io::Result<()> {
    private(run)?;
    if run.join("retention.json").try_exists()? {
        return Err(refused());
    }
    write(&run.join("retention.json"), &Stamp { started: now()? })
}
fn fresh(run: &Path, time: u64) -> bool {
    let Ok(stamp) = read::<Stamp>(&run.join("retention.json")) else {
        return false;
    };
    // Reserve two seconds for stop and removal before the seven-day maximum.
    stamp.started > 0
        && time
            .checked_sub(stamp.started)
            .is_some_and(|age| age < MAX_AGE - HEARTBEAT_AGE)
}
fn runs(directory: &Path) -> io::Result<Vec<PathBuf>> {
    private(directory)?;
    runs_at(&directory.join("crash-runs"))
}
fn runs_at(root: &Path) -> io::Result<Vec<PathBuf>> {
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    private(&root)?;
    let mut result = Vec::new();
    for entry in fs::read_dir(&root)?.take(MAX_ENTRIES + 1) {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(refused)?;
        let id = uuid::Uuid::parse_str(name).map_err(|_| refused())?;
        if id.is_nil() || id.to_string() != name || result.len() == MAX_ENTRIES {
            return Err(refused());
        }
        private(&path)?;
        result.push(path);
    }
    Ok(result)
}
fn inventory(
    path: &Path,
    depth: usize,
    entries: &mut usize,
    count: &mut usize,
    bytes: &mut u64,
) -> io::Result<()> {
    if depth > 8 {
        return Err(refused());
    }
    private(path)?;
    for entry in fs::read_dir(path)? {
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(refused());
        }
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            inventory(&path, depth + 1, entries, count, bytes)?;
        } else {
            let file = storage::open(&path, storage::Open::Read { private: true })?;
            if path.extension().and_then(|s| s.to_str()) == Some("dmp") {
                *count += 1;
                *bytes = bytes
                    .checked_add(file.metadata()?.len())
                    .ok_or_else(refused)?;
            }
        }
    }
    Ok(())
}
fn purge(runs: &[PathBuf], root: &Path) -> io::Result<()> {
    for run in runs {
        private(run)?;
        fs::remove_dir_all(run)?;
        storage::sync_dir(root)?;
    }
    Ok(())
}
/// Caller must own the configured capture lock. Validate before deleting or reading raw bytes.
pub fn prune_locked(directory: &Path, time: u64) -> io::Result<()> {
    prune_runs_locked(&directory.join("crash-runs"), time)
}
/// Caller owns the capture lock for this raw root.
pub fn prune_runs_locked(root: &Path, time: u64) -> io::Result<()> {
    let runs = runs_at(root)?;
    let (mut entries, mut count, mut bytes) = (0, 0, 0);
    for run in &runs {
        inventory(run, 0, &mut entries, &mut count, &mut bytes)?;
    }
    let invalid: Vec<_> = runs
        .into_iter()
        .filter(|run| {
            count > 5 || bytes > MAX_BYTES || !fresh(run, time) || run.join("stop-handler").exists()
        })
        .collect();
    purge(&invalid, root)
}
/// Called before native entry and every raw write, including after resume.
pub fn permit(run: &Path) -> io::Result<()> {
    let directory = run.parent().and_then(Path::parent).ok_or_else(refused)?;
    let owner = Owner::open()?;
    owner.require(directory)?;
    let id = run
        .file_name()
        .and_then(|v| v.to_str())
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .ok_or_else(refused)?;
    ipc::request(&owner.root, id, owner.authority)?;
    let time = now()?;
    if !fresh(run, time) {
        return Err(refused());
    }
    let all = runs(directory)?;
    if all.len() != 1 || all[0] != run {
        return Err(refused());
    }
    if run.join("stop-handler").try_exists()? {
        return Err(refused());
    }
    Ok(())
}
pub fn request_stop(run: &Path) -> io::Result<()> {
    private(run)?;
    let path = run.join("stop-handler");
    match storage::open(&path, storage::Open::New) {
        Ok(mut file) => {
            file.write_all(b"stop")?;
            file.sync_all()
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let mut value = Vec::new();
            storage::open(&path, storage::Open::Read { private: true })?
                .take(5)
                .read_to_end(&mut value)?;
            if value == b"stop" {
                Ok(())
            } else {
                Err(refused())
            }
        }
        Err(e) => Err(e),
    }
}

/// An old database cannot gain five more reservations by starting another handler.
pub fn admit_handler(run: &Path) -> io::Result<()> {
    permit(run)?;
    let (mut entries, mut count, mut bytes) = (0, 0, 0);
    inventory(run, 0, &mut entries, &mut count, &mut bytes)?;
    if count != 0 || bytes != 0 {
        return Err(refused());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Temp, Owner) {
        let mut temp =
            Temp(std::env::temp_dir().join(format!("retention-unit-{}", uuid::Uuid::new_v4())));
        storage::create_dir(&temp.0).unwrap();
        // macOS's temporary directory can have an OS-provided alias.
        temp.0 = fs::canonicalize(&temp.0).unwrap();
        let root = temp.0.join("owner");
        storage::create_dir(&root).unwrap();
        let directory = root.join("config");
        storage::create_dir(&directory).unwrap();
        Owner::register_at(root.clone(), &directory).unwrap();
        (temp, Owner::at(root).unwrap())
    }
    fn run(owner: &Owner, time: u64, size: u64) -> PathBuf {
        let root = owner.directory.join("crash-runs");
        storage::create_dir(&root).unwrap();
        let run = root.join(uuid::Uuid::new_v4().to_string());
        storage::create_dir(&run).unwrap();
        write(&run.join("retention.json"), &Stamp { started: time }).unwrap();
        let file = storage::open(&run.join("synthetic.dmp"), storage::Open::New).unwrap();
        file.set_len(size).unwrap();
        run
    }
    #[test]
    fn authority_registration_cannot_change_an_existing_scope() {
        let (_temp, owner) = fixture();
        assert_eq!(owner.authority(), Authority::Visor);
        assert!(
            Owner::register_authority(owner.root.clone(), &owner.directory, Authority::Host)
                .is_err()
        );
        let old: Registration =
            serde_json::from_value(serde_json::json!({"directory":owner.directory})).unwrap();
        assert_eq!(old.authority, Authority::Visor);
        assert!(serde_json::from_value::<Registration>(
            serde_json::json!({"directory":owner.directory,"authority":"unknown"})
        )
        .is_err());
    }
    #[test]
    fn registered_run_survives_the_application_to_handler_lock_gap() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 0);
        fs::remove_file(raw.join("synthetic.dmp")).unwrap();
        let id = uuid::Uuid::parse_str(raw.file_name().unwrap().to_str().unwrap()).unwrap();
        let application = lock(&owner.directory.join("health-consent/crash-capture.lock")).unwrap();
        let mut ages = Ages::default();
        ages.admit(&owner.directory, id, 100).unwrap();
        drop(storage::UnlockOnDrop(&application));
        // Cleanup wins the lock before the handler. A reserved run survives.
        owner.tick_ages(|| Ok(101), false, Some(&ages)).unwrap();
        assert!(raw.exists());
        assert!(!raw.join("stop-handler").exists());
        let handler = lock(&owner.directory.join("health-consent/crash-capture.lock")).unwrap();
        ages.admit(&owner.directory, id, 101).unwrap();
        assert_eq!(ages.0[&id].0, 100);
        drop(storage::UnlockOnDrop(&handler));
    }
    #[test]
    fn rewritten_disk_age_cannot_extend_service_age() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 0);
        fs::remove_file(raw.join("synthetic.dmp")).unwrap();
        let id = uuid::Uuid::parse_str(raw.file_name().unwrap().to_str().unwrap()).unwrap();
        let mut ages = Ages::default();
        ages.admit(&owner.directory, id, 100).unwrap();
        write(
            &raw.join("retention.json"),
            &Stamp {
                started: 100 + MAX_AGE,
            },
        )
        .unwrap();
        assert!(ages.admit(&owner.directory, id, 100 + MAX_AGE).is_err());
        ages.expire(&owner.directory, 100 + MAX_AGE).unwrap();
        assert!(raw.join("stop-handler").exists());
        owner.tick(100 + MAX_AGE, false).unwrap();
        assert!(!raw.exists());
    }
    #[test]
    fn restart_discards_unknown_raw_before_readiness() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        // A freshly rewritten disk timestamp cannot survive the startup purge.
        write(&raw.join("retention.json"), &Stamp { started: 101 }).unwrap();
        assert!(owner.tick(101, true).unwrap());
        assert!(!raw.exists());
        assert!(!owner.root.join("heartbeat.json").exists());
    }
    #[test]
    fn unknown_stopped_run_is_purged_even_with_fresh_disk_time() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        owner
            .tick_ages(|| Ok(100), false, Some(&Ages::default()))
            .unwrap();
        assert!(!raw.exists());
    }
    #[test]
    fn unknown_existing_dump_cannot_gain_a_new_service_age() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        let id = uuid::Uuid::parse_str(raw.file_name().unwrap().to_str().unwrap()).unwrap();
        assert!(Ages::default().admit(&owner.directory, id, 101).is_err());
    }
    #[test]
    fn stopped_app_expiry_and_restart_remove_before_heartbeat() {
        let (_temp, owner) = fixture();
        let old = run(&owner, 100, 1);
        assert!(owner.tick(100 + MAX_AGE, false).unwrap());
        assert!(!old.exists());
        let pulse: Pulse = read(&owner.root.join("heartbeat.json")).unwrap();
        assert_eq!(pulse.time, 100 + MAX_AGE);
    }
    #[test]
    fn seven_day_boundary_is_conservatively_bounded() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        prune_locked(&owner.directory, 100 + MAX_AGE - HEARTBEAT_AGE - 1).unwrap();
        assert!(raw.exists());
        prune_locked(&owner.directory, 100 + MAX_AGE - HEARTBEAT_AGE).unwrap();
        assert!(!raw.exists());
    }
    #[test]
    fn six_reports_and_oversized_aggregate_are_purged() {
        let (_temp, owner) = fixture();
        let mut raw = Vec::new();
        for _ in 0..6 {
            raw.push(run(&owner, 100, 1));
        }
        prune_locked(&owner.directory, 101).unwrap();
        assert!(raw.iter().all(|p| !p.exists()));
        let raw = run(&owner, 100, MAX_BYTES + 1);
        prune_locked(&owner.directory, 101).unwrap();
        assert!(!raw.exists());
    }
    #[test]
    fn five_reports_at_limit_remain_until_expiry() {
        let (_temp, owner) = fixture();
        let mut raw = Vec::new();
        for _ in 0..5 {
            raw.push(run(&owner, 100, MAX_BYTES / 5));
        }
        prune_locked(&owner.directory, 101).unwrap();
        assert!(raw.iter().all(|p| p.exists()));
    }
    #[test]
    fn malformed_future_missing_and_zero_times_purge_without_reading_raw() {
        let (_temp, owner) = fixture();
        for value in [
            b"bad".as_slice(),
            br#"{"started":101}"#,
            br#"{"started":0}"#,
        ] {
            let raw = run(&owner, 99, 1);
            fs::write(raw.join("retention.json"), value).unwrap();
            prune_locked(&owner.directory, 100).unwrap();
            assert!(!raw.exists());
        }
        let raw = run(&owner, 99, 1);
        fs::remove_file(raw.join("retention.json")).unwrap();
        prune_locked(&owner.directory, 100).unwrap();
        assert!(!raw.exists());
    }
    #[test]
    fn active_writer_stops_before_expiry_or_withdrawal_purge() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        drop(storage::open(&raw.join("handler-active"), storage::Open::New).unwrap());
        let writer = lock(&owner.directory.join("health-consent/crash-capture.lock")).unwrap();
        assert!(!owner.tick(100 + MAX_AGE, false).unwrap());
        assert!(raw.exists());
        assert!(raw.join("stop-handler").exists());
        assert!(!owner.root.join("heartbeat.json").exists());
        drop(storage::UnlockOnDrop(&writer));
        assert!(owner.tick(100 + MAX_AGE, false).unwrap());
        assert!(!raw.exists());
        let raw = run(&owner, 100 + MAX_AGE, 1);
        assert!(owner.tick(100 + MAX_AGE, true).unwrap());
        assert!(!raw.exists());
    }
    #[test]
    fn service_lease_and_fresh_pulse_are_both_required() {
        let (_temp, owner) = fixture();
        write(
            &owner.root.join("heartbeat.json"),
            &Pulse { time: 100, pid: 42 },
        )
        .unwrap();
        let lease = lock(&owner.root.join("service.lock")).unwrap();
        owner
            .require_with(&owner.directory, || Ok(100), |_| Ok(()))
            .unwrap();
        assert!(owner
            .require_with(&owner.directory, || Ok(99), |_| Ok(()))
            .is_err());
        assert!(owner
            .require_with(&owner.directory, || Ok(103), |_| Ok(()))
            .is_err());
        drop(storage::UnlockOnDrop(&lease));
        assert!(owner
            .require_with(&owner.directory, || Ok(100), |_| Ok(()))
            .is_err());
    }
    #[test]
    fn second_configuration_cannot_reset_owner_quota() {
        let (temp, owner) = fixture();
        let second = temp.0.join("second");
        storage::create_dir(&second).unwrap();
        assert!(Owner::register_at(owner.root.clone(), &second).is_err());
        assert!(owner.require_with(&second, || Ok(100), |_| Ok(())).is_err());
    }
    #[test]
    fn linked_raw_files_fail_closed_without_touching_the_target() {
        let (temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        let outside = temp.0.join("outside");
        fs::hard_link(raw.join("synthetic.dmp"), &outside).unwrap();
        assert!(prune_locked(&owner.directory, 100 + MAX_AGE).is_err());
        assert!(outside.exists());
        assert!(raw.exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_directory_and_file_are_refused() {
        use std::os::unix::fs::symlink;
        let (temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        let outside = temp.0.join("outside");
        storage::create_dir(&outside).unwrap();
        symlink(&outside, raw.join("linked")).unwrap();
        assert!(prune_locked(&owner.directory, 100 + MAX_AGE).is_err());
        fs::remove_file(raw.join("linked")).unwrap();
        fs::remove_file(raw.join("synthetic.dmp")).unwrap();
        symlink(&outside, raw.join("synthetic.dmp")).unwrap();
        assert!(prune_locked(&owner.directory, 100 + MAX_AGE).is_err());
    }
    #[test]
    fn rollback_and_stopped_marker_do_not_extend_retention() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        owner.tick(99, false).unwrap();
        assert!(!raw.exists());
        let raw = run(&owner, 100, 1);
        request_stop(&raw).unwrap();
        prune_locked(&owner.directory, 101).unwrap();
        assert!(!raw.exists());
    }
    #[cfg(windows)]
    #[test]
    fn junction_is_refused_without_following_its_target() {
        let (temp, owner) = fixture();
        let raw = run(&owner, 100, 1);
        let outside = temp.0.join("outside");
        storage::create_dir(&outside).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(raw.join("junction"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(prune_locked(&owner.directory, 100 + MAX_AGE).is_err());
        assert!(outside.exists());
        fs::remove_dir(raw.join("junction")).unwrap();
    }
    #[test]
    fn forged_heartbeat_cannot_replace_os_service_identity() {
        let (_temp, owner) = fixture();
        let lease = lock(&owner.root.join("service.lock")).unwrap();
        write(
            &owner.root.join("heartbeat.json"),
            &Pulse { time: 100, pid: 42 },
        )
        .unwrap();
        assert!(owner
            .require_with(&owner.directory, || Ok(100), |_| Err(refused()))
            .is_err());
        assert!(owner
            .require_with(
                &owner.directory,
                || Ok(100),
                |pid| if pid == 7 { Ok(()) } else { Err(refused()) }
            )
            .is_err());
        drop(storage::UnlockOnDrop(&lease));
    }
    #[test]
    fn initializing_writer_is_not_mistaken_for_an_invalid_run() {
        let (_temp, owner) = fixture();
        let raw = run(&owner, 101, 1);
        fs::remove_file(raw.join("retention.json")).unwrap();
        let writer = lock(&owner.directory.join("health-consent/crash-capture.lock")).unwrap();
        assert!(owner.tick(100, false).unwrap());
        assert!(!raw.join("stop-handler").exists());
        write(&raw.join("retention.json"), &Stamp { started: 101 }).unwrap();
        assert!(owner.tick(100, false).unwrap());
        assert!(!raw.join("stop-handler").exists());
        drop(storage::open(&raw.join("handler-active"), storage::Open::New).unwrap());
        assert!(owner.tick_with(|| Ok(101), false).unwrap());
        assert!(!raw.join("stop-handler").exists());
        drop(storage::UnlockOnDrop(&writer));
    }
    #[test]
    fn rewritten_registration_cannot_redirect_admission_or_cleanup() {
        let (temp, owner) = fixture();
        let second = temp.0.join("redirected");
        storage::create_dir(&second).unwrap();
        write(
            &owner.root.join("directory.json"),
            &Registration {
                authority: Authority::Visor,
                directory: second.clone(),
            },
        )
        .unwrap();
        assert!(Owner::at(owner.root.clone()).is_err());
        assert!(owner.require_with(&second, || Ok(100), |_| Ok(())).is_err());
        let raw = run(&owner, 100, 1);
        owner.tick(100 + MAX_AGE, false).unwrap();
        assert!(!raw.exists());
    }
}
