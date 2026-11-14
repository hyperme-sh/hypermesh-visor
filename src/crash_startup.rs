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

/// A local, explicit category setting; enabling never approves an upload.
/// Native capture starts on the next application startup.
pub fn set_crash_consent(enabled: bool) -> io::Result<()> {
    let path = std::env::var_os("HYPERMESH_CRASH_CONFIG")
        .ok_or_else(|| io::Error::other("crash reporting is not configured"))?;
    set_consent_at(Path::new(&path), enabled)
}
/// Local native UI only; no network router exposes these decisions.
pub(crate) fn consent_ui(
    action: &str,
    decision: &serde_json::Value,
) -> io::Result<serde_json::Value> {
    let path = std::env::var_os("HYPERMESH_CRASH_CONFIG")
        .ok_or_else(|| io::Error::other("crash reporting is not configured"))?;
    consent_ui_at(Path::new(&path), action, decision)
}
fn consent_ui_at(
    path: &Path,
    action: &str,
    decision: &serde_json::Value,
) -> io::Result<serde_json::Value> {
    use crate::health_consent::Category;
    use sha2::{Digest, Sha256};
    if !path.is_absolute() {
        return Err(io::Error::other("private crash configuration required"));
    }
    let config =
        Config::read(path)?.ok_or_else(|| io::Error::other("crash reporting is not configured"))?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid crash directory"))?;
    let store = Store::open(&directory.join("health-consent"), config.scope.clone())?;
    let ticket = store.capture(Category::Crashes)?;
    let binding = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&serde_json::json!({
            "scope":config.scope, "destination":config.destination(), "ticket":ticket,
        }))?)
    );
    if action == "crash-consent-set" {
        if decision["binding"].as_str() != Some(binding.as_str()) {
            return Err(io::Error::other(
                "crash configuration or decision changed; reopen controls",
            ));
        }
        match decision["choice"].as_str() {
            Some("enable") if ticket.is_none() => {
                store.decide(Category::Crashes, true)?;
            }
            Some("disable") => {
                crate::crash_capture::withdraw_configured(&store, directory)?;
            }
            _ => return Err(io::Error::other("invalid crash consent choice")),
        }
        return Ok(serde_json::json!({"saved":true}));
    }
    if action != "crash-consent-status" {
        return Err(io::Error::other("invalid crash consent action"));
    }
    Ok(
        serde_json::json!({"enabled":ticket.is_some(), "binding":binding,
        "notice":format!("Crash diagnostics help diagnose Visor failures. Capture is off unless you enable it. Private local minidumps may contain process memory. On the next startup, Visor reduces them to a fixed crash code, version, event ID, time and tenant tag, then deletes raw files before review. Raw dumps are never uploaded. Each sanitized upload to {} requires a separate approval. Reports expire after 24 hours; raw runs are cleaned on startup or successful withdrawal. Enable takes effect after restarting Visor. Disable stops capture and purges local raw runs before it succeeds.", config.destination())}),
    )
}

fn set_consent_at(path: &Path, enabled: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("private crash configuration required"));
    }
    let config =
        Config::read(path)?.ok_or_else(|| io::Error::other("crash reporting is not configured"))?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid crash directory"))?;
    let store = Store::open(&directory.join("health-consent"), config.scope)?;
    if enabled {
        store.decide(crate::health_consent::Category::Crashes, true)?;
        Ok(())
    } else {
        crate::crash_capture::withdraw_configured(&store, directory)
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
    fn native_consent_preview_binds_choice_to_config_and_current_decision() {
        let dir = Temp::new();
        let (config, store, run) = fixture(&dir.0);
        let preview =
            consent_ui_at(&config, "crash-consent-status", &serde_json::Value::Null).unwrap();
        assert_eq!(preview["enabled"], true);
        assert!(!preview.to_string().contains("fixture_key"));
        assert!(consent_ui_at(
            &config,
            "crash-consent-set",
            &serde_json::json!({"choice":"disable","binding":"old"})
        )
        .is_err());
        assert!(run.exists());
        consent_ui_at(
            &config,
            "crash-consent-set",
            &serde_json::json!({"choice":"disable","binding":preview["binding"]}),
        )
        .unwrap();
        assert!(!run.exists());
        assert!(store.capture(Category::Crashes).unwrap().is_none());
        assert!(consent_ui_at(
            &config,
            "crash-consent-set",
            &serde_json::json!({"choice":"enable","binding":preview["binding"]})
        )
        .is_err());
        let preview =
            consent_ui_at(&config, "crash-consent-status", &serde_json::Value::Null).unwrap();
        consent_ui_at(
            &config,
            "crash-consent-set",
            &serde_json::json!({"choice":"enable","binding":preview["binding"]}),
        )
        .unwrap();
        assert!(store.capture(Category::Crashes).unwrap().is_some());
    }
    #[test]
    fn explicit_local_setting_purges_on_withdrawal_without_review() {
        let dir = Temp::new();
        let (config, store, run) = fixture(&dir.0);
        set_consent_at(&config, false).unwrap();
        assert!(!run.exists());
        assert!(store.capture(Category::Crashes).unwrap().is_none());
        set_consent_at(&config, true).unwrap();
        assert!(store.capture(Category::Crashes).unwrap().is_some());
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
