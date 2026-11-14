//! Device-local recovery and individual review at application startup.
use crate::{crash_report::PendingCrash, crash_upload::Config, health_consent::Store};
use hypermesh_local_storage as storage;
use std::{io, path::Path};

/// Called by the local application, never from an IPC request. Configuration
/// and report content must not be included in diagnostics.
pub fn startup() {
    let Some(path) = std::env::var_os("HYPERMESH_CRASH_CONFIG") else {
        return;
    };
    let result = recover_and_review(Path::new(&path), crate::crash_dialog::review_and_send)
        .and_then(|capture| match capture {
            Some((store, directory)) => crate::crash_capture::start(store, &directory),
            None => Ok(()),
        });
    if result.is_err() {
        eprintln!("Visor crash recovery unavailable");
    }
}

fn recover_and_review(
    path: &Path,
    mut review: impl FnMut(&Config, &Store, PendingCrash) -> io::Result<bool>,
) -> io::Result<Option<(Store, std::path::PathBuf)>> {
    if !path.is_absolute() {
        return Err(io::Error::other("private crash configuration required"));
    }
    let Some(config) = Config::read(path)? else {
        return Ok(None);
    };
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid crash directory"))?;
    let consent = directory.join("health-consent");
    storage::create_dir(&consent)?;
    let store = Store::open(&consent, config.scope.clone())?;
    let reports = crate::crash_recovery::recover(
        &store,
        config.destination(),
        &directory.join("crash-runs"),
    )?;
    // Recovery purges all raw runs before the first dialog. A failed or closed
    // dialog consumes that report; no automatic resend or persisted approval.
    for report in reports {
        review(&config, &store, report)?;
    }
    Ok(Some((store, directory.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::{Category, Scope};
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    use uuid::Uuid;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("visor-crash-startup-{}", Uuid::new_v4()));
            storage::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn write(path: &Path, bytes: &[u8]) {
        storage::open(path, storage::Open::New)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn fixture(directory: &Path) -> (PathBuf, Store, PathBuf) {
        let scope = Scope {
            subject: Uuid::new_v4(),
            tenant: Uuid::new_v4(),
            device: Uuid::new_v4(),
            notice_version: Uuid::new_v4(),
            purpose_version: Uuid::new_v4(),
        };
        let config = directory.join("crash.json");
        write(
            &config,
            &serde_json::to_vec(&serde_json::json!({
                "dsn":"https://fixture_key@glitchtip.example.invalid/42", "scope":scope,
            }))
            .unwrap(),
        );
        let consent = directory.join("health-consent");
        storage::create_dir(&consent).unwrap();
        let store = Store::open(&consent, scope).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let root = directory.join("crash-runs");
        storage::create_dir(&root).unwrap();
        let run = root.join(Uuid::new_v4().to_string());
        storage::create_dir(&run).unwrap();
        write(
            &run.join("ticket.json"),
            &serde_json::to_vec(&store.capture(Category::Crashes).unwrap().unwrap()).unwrap(),
        );
        storage::create_dir(&run.join("database")).unwrap();
        let pending = run.join("database/pending");
        storage::create_dir(&pending).unwrap();
        let mut dump = vec![0; 212];
        for (offset, value) in [
            (0, 0x504d444du32),
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
            dump[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        dump.extend_from_slice(b"excluded-private-prompt");
        write(&pending.join(format!("{}.dmp", Uuid::new_v4())), &dump);
        (config, store, run)
    }
    #[test]
    fn recovery_precedes_review_and_supplies_only_sanitized_unapproved_report() {
        let dir = Temp::new();
        let (config, _, run) = fixture(&dir.0);
        let mut calls = 0;
        recover_and_review(&config, |config, store, report| {
            calls += 1;
            assert!(!run.exists());
            let preview = report.preview();
            assert_eq!(preview.destination, config.destination());
            assert!(!String::from_utf8_lossy(preview.sanitized_json)
                .contains("excluded-private-prompt"));
            assert!(report
                .send_once(store, |_, _| panic!("approval required"))
                .is_err());
            Ok(false)
        })
        .unwrap();
        assert_eq!(calls, 1);
    }
    #[test]
    fn withdrawn_reports_are_purged_without_dialog_or_send() {
        let dir = Temp::new();
        let (config, store, run) = fixture(&dir.0);
        store.decide(Category::Crashes, false).unwrap();
        recover_and_review(&config, |_, _, _| panic!("withdrawn")).unwrap();
        assert!(!run.exists());
    }
    #[test]
    fn missing_config_does_not_create_consent_or_capture_state() {
        let dir = Temp::new();
        recover_and_review(&dir.0.join("missing.json"), |_, _, _| {
            panic!("unconfigured")
        })
        .unwrap();
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        assert!(
            recover_and_review(Path::new("relative.json"), |_, _, _| panic!("invalid")).is_err()
        );
    }
}
