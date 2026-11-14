//! Local per-crash review. No IPC route or remote approval entry point.
use crate::{crash_report::PendingCrash, crash_upload::Config, health_consent::Store};
use std::io;

/// Display one immutable sanitized preview, then consume it. Dismissal, errors
/// and missing desktop support never grant approval or create a retry queue.
pub fn review_and_send(config: &Config, store: &Store, report: PendingCrash) -> io::Result<bool> {
    let Some(report) = review(report, prompt)? else {
        return Ok(false);
    };
    config.send(report, store)?;
    Ok(true)
}
fn review(
    mut report: PendingCrash,
    show: impl FnOnce(&str) -> io::Result<bool>,
) -> io::Result<Option<PendingCrash>> {
    let preview = report.preview();
    let json = std::str::from_utf8(preview.sanitized_json)
        .map_err(|_| io::Error::other("invalid crash preview"))?;
    let text = format!("Visor closed unexpectedly. Sending this report is optional.\n\nDestination: {}\n\nOnly the report below will be sent. No memory dump, prompt text or file contents will be sent.\n\n{}\n\nSend this report?", preview.destination, json);
    if !show(&text)? {
        report.decline();
        return Ok(None);
    }
    let (id, digest, destination) = (
        preview.id,
        preview.sha256.to_owned(),
        preview.destination.to_owned(),
    );
    report.approve(id, &digest, &destination)?;
    Ok(Some(report))
}

#[cfg(target_os = "linux")]
fn prompt(text: &str) -> io::Result<bool> {
    let mut command = std::process::Command::new("/usr/bin/zenity");
    command.args([
        "--text-info",
        "--title=Review Visor crash report",
        "--width=760",
        "--height=560",
        "--checkbox=Send this sanitized report",
        "--ok-label=Send report",
        "--cancel-label=Delete report",
    ]);
    run_prompt(command, text)
}

#[cfg(target_os = "macos")]
fn prompt(text: &str) -> io::Result<bool> {
    // The program is constant; report text is stdin data, never script source.
    let script = r#"ObjC.import('Foundation');
const bytes = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
const text = ObjC.unwrap($.NSString.alloc.initWithDataEncoding(bytes, $.NSUTF8StringEncoding));
const app = Application.currentApplication(); app.includeStandardAdditions = true;
const choice = app.displayDialog(text, {withTitle:'Review Visor crash report', buttons:['Delete report','Send report'], defaultButton:'Delete report', cancelButton:'Delete report'});
if (choice.buttonReturned !== 'Send report') throw new Error('declined');
"#;
    let mut command = std::process::Command::new("/usr/bin/osascript");
    command.args(["-l", "JavaScript", "-e", script]);
    run_prompt(command, text)
}

#[cfg(unix)]
fn run_prompt(mut command: std::process::Command, text: &str) -> io::Result<bool> {
    use std::{io::Write, process::Stdio};
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| io::Error::other("crash dialog unavailable"))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("crash dialog unavailable"))
        .and_then(|mut input| input.write_all(text.as_bytes()));
    if written.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("crash dialog unavailable"));
    }
    match child.wait() {
        Ok(status) => Ok(status.success()),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::other("crash dialog unavailable"))
        }
    }
}

#[cfg(windows)]
fn prompt(text: &str) -> io::Result<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO,
    };
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Review Visor crash report"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // Owned, terminated strings remain alive through this synchronous call.
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        )
    };
    Ok(answer == IDYES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::{Category, Scope};
    use uuid::Uuid;
    fn fixture() -> (std::path::PathBuf, Store) {
        let path = std::env::temp_dir().join(format!("visor-dialog-{}", Uuid::new_v4()));
        let store = Store::open(
            &path,
            Scope {
                subject: Uuid::new_v4(),
                tenant: Uuid::new_v4(),
                device: Uuid::new_v4(),
                notice_version: Uuid::new_v4(),
                purpose_version: Uuid::new_v4(),
            },
        )
        .unwrap();
        store.decide(Category::Crashes, true).unwrap();
        (path, store)
    }
    fn pending(store: &Store) -> PendingCrash {
        PendingCrash::capture(
            store,
            "https://glitchtip.example.invalid/api/42/envelope/",
            &serde_json::json!({"signal":11,"prompt":"excluded-private-text"}),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn only_explicit_dialog_acceptance_approves_the_displayed_report() {
        let (path, store) = fixture();
        assert!(review(pending(&store), |_| Ok(false)).unwrap().is_none());
        assert!(review(pending(&store), |_| Err(io::Error::other("unavailable"))).is_err());
        let report = pending(&store);
        let expected = report.preview().sanitized_json.to_vec();
        let report = review(report, |text| {
            assert!(text.contains("https://glitchtip.example.invalid/api/42/envelope/"));
            assert!(text.contains(std::str::from_utf8(&expected).unwrap()));
            assert!(!text.contains("excluded-private-text"));
            Ok(true)
        })
        .unwrap()
        .unwrap();
        report
            .send_once(&store, |_, body| {
                assert_eq!(body, expected);
                Ok(())
            })
            .unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn withdrawal_while_dialog_is_open_refuses_send() {
        let (path, store) = fixture();
        let report = review(pending(&store), |_| {
            store.decide(Category::Crashes, false).unwrap();
            Ok(true)
        })
        .unwrap()
        .unwrap();
        assert!(report
            .send_once(&store, |_, _| panic!("withdrawn"))
            .is_err());
        std::fs::remove_dir_all(path).unwrap();
    }
}
