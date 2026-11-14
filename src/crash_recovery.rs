//! Recovery of private, stopped capture runs. No network or desktop operations.
use crate::{crash_report::PendingCrash, health_consent::Store};
use hypermesh_local_storage as storage;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_REPORTS: usize = 5;
const MAX_ENTRIES: usize = 1024;
fn refused() -> io::Error {
    io::Error::other("private crash recovery refused")
}

fn entries(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let paths: Vec<_> = fs::read_dir(directory)?
        .take(MAX_ENTRIES + 1)
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()?;
    if paths.len() > MAX_ENTRIES {
        return Err(refused());
    }
    Ok(paths)
}
fn uuid_name(path: &Path) -> Option<Uuid> {
    let name = path.file_name()?.to_str()?;
    let id = Uuid::parse_str(name).ok()?;
    (!id.is_nil() && name == id.to_string()).then_some(id)
}

/// Each run is an owner-private UUID directory containing `ticket.json` and
/// an SDK `database` directory. The handler must own the capture lease while
/// it writes. Recovery refuses to touch any runs while that lease is active.
/// All raw run contents are removed before any sanitized preview is returned.
pub fn recover(
    store: &Store,
    destination: &str,
    directory: &Path,
) -> io::Result<Vec<PendingCrash>> {
    let guard = store.recovery_guard()?;
    match fs::symlink_metadata(directory) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(refused()),
        Ok(_) => storage::check_dir(directory).map_err(|_| refused())?,
    }
    let mut runs = entries(directory)?;
    runs.sort();
    // Reject unknown or unsafe roots before consuming any valid run.
    for run in &runs {
        if uuid_name(run).is_none() {
            return Err(refused());
        }
        storage::check_dir(run).map_err(|_| refused())?;
    }
    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for run in &runs {
        // Invalid, missing, withdrawn or superseded metadata means delete, not
        // re-consent. The raw dump is not read on this path.
        let ticket = (|| {
            let file = storage::open(
                &run.join("ticket.json"),
                storage::Open::Read { private: true },
            )?;
            let mut bytes = Vec::new();
            file.take(4097).read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        })();
        let ticket = match ticket {
            Ok(bytes) => guard.restore(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(ticket) = ticket {
            let database = run.join("database");
            if storage::check_dir(&database).is_ok() {
                // These are the completed-report locations in the platform
                // Crashpad databases. Never read the in-progress `new` folder.
                for name in ["pending", "completed", "reports"] {
                    let folder = database.join(name);
                    if storage::check_dir(&folder).is_err() {
                        continue;
                    }
                    let mut dumps = entries(&folder)?;
                    dumps.sort();
                    for path in dumps {
                        if candidates.len() == MAX_REPORTS {
                            break;
                        }
                        if path.extension().and_then(|s| s.to_str()) != Some("dmp") {
                            continue;
                        }
                        let stem = path.with_extension("");
                        let Some(id) = uuid_name(&stem) else {
                            continue;
                        };
                        if !seen.insert(id) {
                            continue;
                        }
                        candidates.push((ticket.clone(), folder.clone(), id));
                    }
                }
            }
        }
    }
    // All fallible metadata reads and directory enumeration finish before any
    // report is consumed. The capture lock keeps crash decisions stable here.
    let mut reports = Vec::new();
    for (ticket, folder, id) in candidates {
        if let Ok(report) = PendingCrash::from_recovery_dump(ticket, destination, &folder, id) {
            reports.push(report);
        }
    }
    for run in &runs {
        // remove_dir_all removes links themselves, never their targets. The
        // private run root was validated above, and no writer holds the lease.
        fs::remove_dir_all(&run).map_err(|_| refused())?;
        storage::sync_dir(directory).map_err(|_| refused())?;
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::{Category, Scope};
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("visor-recovery-{}", Uuid::new_v4()));
            storage::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn store(directory: &Path) -> Store {
        Store::open(
            directory,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap()
    }
    fn put(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn run(root: &Path, store: &Store, location: &str) -> PathBuf {
        let run = root.join(Uuid::new_v4().to_string());
        storage::create_dir(&run).unwrap();
        let ticket = store.capture(Category::Crashes).unwrap().unwrap();
        storage::open(&run.join("ticket.json"), storage::Open::New)
            .unwrap()
            .write_all(&serde_json::to_vec(&ticket).unwrap())
            .unwrap();
        let db = run.join("database");
        storage::create_dir(&db).unwrap();
        let pending = db.join(location);
        storage::create_dir(&pending).unwrap();
        let mut bytes = vec![0; 212];
        for (offset, value) in [
            (0, 0x504d444d),
            (4, 0xa793),
            (8, 1),
            (12, 32),
            (
                20,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32,
            ),
            (32, 6),
            (36, 168),
            (40, 44),
            (52, 0xc0000005),
        ] {
            put(&mut bytes, offset, value);
        }
        bytes.extend_from_slice(b"excluded-private-prompt");
        storage::open(
            &pending.join(format!("{}.dmp", Uuid::new_v4())),
            storage::Open::New,
        )
        .unwrap()
        .write_all(&bytes)
        .unwrap();
        run
    }
    const DEST: &str = "https://glitchtip.example.invalid/api/42/envelope/";
    #[test]
    fn validated_recovery_survives_later_consent_lock_contention() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        let pending = run(&root.0, &store, "pending");
        let guard = store.recovery_guard().unwrap();
        let ticket = guard
            .restore(&fs::read(pending.join("ticket.json")).unwrap())
            .unwrap()
            .unwrap();
        let folder = pending.join("database/pending");
        let path = entries(&folder).unwrap().pop().unwrap();
        let id = uuid_name(&path.with_extension("")).unwrap();
        let lock = storage::open(&consent.0.join("lock"), storage::Open::Lock).unwrap();
        storage::lock(&lock).unwrap();
        let _unlock = storage::UnlockOnDrop(&lock);
        let report = PendingCrash::from_recovery_dump(ticket, DEST, &folder, id).unwrap();
        assert!(!path.exists());
        assert!(!report.preview().sanitized_json.is_empty());
        assert!(store.decide(Category::Crashes, false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn unsafe_later_ticket_preserves_earlier_runs() {
        use std::os::unix::fs::PermissionsExt;
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        let mut runs = [
            run(&root.0, &store, "pending"),
            run(&root.0, &store, "pending"),
        ];
        runs.sort();
        let ticket = runs[1].join("ticket.json");
        fs::set_permissions(&ticket, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(recover(&store, DEST, &root.0).is_err());
        for pending in &runs {
            assert_eq!(
                fs::read_dir(pending.join("database/pending"))
                    .unwrap()
                    .count(),
                1
            );
        }
        fs::set_permissions(&ticket, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(recover(&store, DEST, &root.0).unwrap().len(), 2);
    }
    #[test]
    fn unknown_entry_preserves_all_valid_runs() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        let pending = run(&root.0, &store, "pending");
        let unknown = root.0.join("z-invalid");
        storage::create_dir(&unknown).unwrap();
        assert!(recover(&store, DEST, &root.0).is_err());
        assert_eq!(
            fs::read_dir(pending.join("database/pending"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir(&unknown).unwrap();
        assert_eq!(recover(&store, DEST, &root.0).unwrap().len(), 1);
    }
    #[test]
    fn busy_consent_preserves_run_until_validation_can_complete() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        let pending = run(&root.0, &store, "pending");
        let lock = storage::open(&consent.0.join("lock"), storage::Open::Lock).unwrap();
        storage::lock(&lock).unwrap();
        let unlock = storage::UnlockOnDrop(&lock);
        assert!(recover(&store, DEST, &root.0).is_err());
        assert!(pending.join("ticket.json").exists());
        assert_eq!(
            fs::read_dir(pending.join("database/pending"))
                .unwrap()
                .count(),
            1
        );
        drop(unlock);
        assert_eq!(recover(&store, DEST, &root.0).unwrap().len(), 1);
        assert!(!pending.exists());
    }
    #[test]
    fn completed_runs_are_removed_before_unapproved_previews_return() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        for name in ["pending", "completed", "reports"] {
            run(&root.0, &store, name);
        }
        let reports = recover(&store, DEST, &root.0).unwrap();
        assert_eq!(reports.len(), 3);
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
        for report in reports {
            assert!(!String::from_utf8_lossy(report.preview().sanitized_json)
                .contains("excluded-private-prompt"));
            assert!(report
                .send_once(&store, |_, _| panic!("not approved"))
                .is_err());
        }
    }
    #[test]
    fn withdrawal_and_regrant_purge_raw_runs_without_preview() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        for regrant in [false, true] {
            store.decide(Category::Crashes, true).unwrap();
            run(&root.0, &store, "pending");
            store.decide(Category::Crashes, false).unwrap();
            if regrant {
                store.decide(Category::Crashes, true).unwrap();
            }
            assert!(recover(&store, DEST, &root.0).unwrap().is_empty());
            assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
        }
    }
    #[test]
    #[cfg(unix)]
    fn cleanup_does_not_follow_links_outside_a_run() {
        let consent = Temp::new();
        let root = Temp::new();
        let outside = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        let run = run(&root.0, &store, "pending");
        fs::write(outside.0.join("keep"), b"unrelated private file").unwrap();
        fs::remove_dir_all(run.join("database")).unwrap();
        std::os::unix::fs::symlink(&outside.0, run.join("database")).unwrap();
        assert!(recover(&store, DEST, &root.0).unwrap().is_empty());
        assert!(!run.exists());
        assert_eq!(
            fs::read(outside.0.join("keep")).unwrap(),
            b"unrelated private file"
        );
    }
    #[test]
    fn active_writer_is_untouched_and_recovery_is_bounded() {
        let consent = Temp::new();
        let root = Temp::new();
        let store = store(&consent.0);
        store.decide(Category::Crashes, true).unwrap();
        for _ in 0..6 {
            run(&root.0, &store, "pending");
        }
        let lease = store.capture_lease().unwrap().unwrap();
        assert!(recover(&store, DEST, &root.0).is_err());
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 6);
        drop(lease);
        assert_eq!(recover(&store, DEST, &root.0).unwrap().len(), 5);
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    }
}
