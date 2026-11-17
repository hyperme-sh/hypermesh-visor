//! Opt-in human feedback dialogs; sending requires separate explicit configuration.
use std::io::{Read, Write};
use std::process::{Command, Stdio};

const MAX_BYTES: usize = 4096;
const FAILURE: &str = "Feedback dialog failed. A previously dispatched report may have arrived.";
const NOTICE: &str = "Draft a human problem report or idea locally? Include only text you choose to type. Do not include prompts, documents, credentials or personal information. No attachments or automatic diagnostics are collected. Sending is disabled in this configuration. Closing any dialog discards this draft.";

pub fn run(confirmation: crate::visor::Confirmation) -> Result<(), String> {
    match crate::feedback_send::Native::from_env()? {
        Some(mut native) => crate::feedback_send::screen(dialog, &mut native, || confirmation.wait()),
        None => screen(dialog),
    }
}

pub(super) fn base(kind: &str, title: &str) -> Vec<String> {
    vec![
        format!("--{kind}"),
        format!("--title={title}"),
        "--width=700".into(),
        "--height=500".into(),
        "--timeout=900".into(),
    ]
}
pub(super) fn decode(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Feedback must be 4096 UTF-8 bytes or smaller.".into());
    }
    let value = String::from_utf8(bytes).map_err(|_| FAILURE)?;
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
    {
        return Err("Feedback contains unsupported control characters.".into());
    }
    Ok(value)
}
// Private edits use pipes only, never a shell, command arguments or logs.
fn dialog(args: &[String], input: Option<&str>) -> Result<Option<String>, String> {
    let mut child = Command::new("zenity")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| FAILURE)?;
    let result = (|| {
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or(FAILURE)?
                .write_all(input.as_bytes())
                .map_err(|_| FAILURE)?;
        }
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or(FAILURE)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| FAILURE)?;
        if bytes.len() > MAX_BYTES {
            return Err("Feedback must be 4096 UTF-8 bytes or smaller.".into());
        }
        let status = child.wait().map_err(|_| FAILURE)?;
        match status.code() {
            Some(0) => decode(bytes).map(Some),
            Some(1) | Some(5) => Ok(None),
            _ => Err(FAILURE.into()),
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn screen(
    mut ui: impl FnMut(&[String], Option<&str>) -> Result<Option<String>, String>,
) -> Result<(), String> {
    let mut consent = base("question", "Human feedback — local draft");
    consent.extend([
        "--no-markup".into(),
        format!("--text={NOTICE}"),
        "--ok-label=Opt in and draft".into(),
        "--cancel-label=Cancel".into(),
    ]);
    if ui(&consent, None)?.is_none() {
        return Ok(());
    }
    let mut category = base("list", "Feedback category");
    category.extend([
        "--no-markup".into(),
        "--column=Category".into(),
        "--".into(),
        "Problem".into(),
        "Idea".into(),
    ]);
    let Some(kind) = ui(&category, None)? else {
        return Ok(());
    };
    let kind = kind.trim();
    if kind != "Problem" && kind != "Idea" {
        return Err(FAILURE.into());
    }
    let mut draft = String::new();
    loop {
        let mut editor = base(
            "text-info",
            "Feedback text — manual text only, 4096 bytes maximum",
        );
        editor.extend([
            "--editable".into(),
            "--ok-label=Preview locally".into(),
            "--cancel-label=Discard draft".into(),
        ]);
        let Some(next) = ui(&editor, Some(&draft))? else {
            return Ok(());
        };
        draft = decode(next.into_bytes())?;
        if draft.trim().is_empty() {
            return Err("Enter a problem or idea before previewing.".into());
        }
        let preview = format!("{kind}\n\n{draft}\n\nLOCAL PREVIEW ONLY — sending is unavailable. No report has been sent.");
        let mut args = base("text-info", "Local feedback preview — no report sent");
        args.extend([
            "--ok-label=Edit draft".into(),
            "--cancel-label=Close and discard".into(),
        ]);
        // No HTML/Markdown mode: display user text literally.
        if ui(&args, Some(&preview))?.is_none() {
            return Ok(());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_text_before_opt_in() {
        let mut calls = 0;
        screen(|args, input| {
            calls += 1;
            assert!(args.iter().any(|a| a == "--question"));
            assert!(input.is_none());
            Ok(None)
        })
        .unwrap();
        assert_eq!(calls, 1);
    }
    #[test]
    fn private_plain_preview_and_edits() {
        let mut call = 0;
        screen(|args, input| {
            call += 1;
            assert!(args
                .iter()
                .all(|a| !a.contains("private-draft") && a != "--html" && a != "--markdown"));
            assert!(args.iter().any(|a| a == "--timeout=900"));
            match call {
                1 => Ok(Some(String::new())),
                2 => Ok(Some("Idea\n".into())),
                3 => {
                    assert_eq!(input, Some(""));
                    Ok(Some("private-draft <b>one</b>".into()))
                }
                4 => {
                    assert!(input.unwrap().contains("private-draft <b>one</b>"));
                    Ok(Some(String::new()))
                }
                5 => {
                    assert_eq!(input, Some("private-draft <b>one</b>"));
                    Ok(Some("replacement".into()))
                }
                6 => {
                    assert!(!input.unwrap().contains("private-draft"));
                    assert!(input.unwrap().contains("replacement"));
                    Ok(None)
                }
                _ => panic!("unexpected dialog"),
            }
        })
        .unwrap();
        assert_eq!(call, 6);
    }
    #[test]
    fn cancel_discards_without_advancing() {
        for cancel_at in 1..=4 {
            let mut call = 0;
            screen(|_, _| {
                call += 1;
                if call == cancel_at {
                    return Ok(None);
                }
                Ok(Some(
                    match call {
                        2 => "Problem",
                        3 => "typed report",
                        _ => "",
                    }
                    .into(),
                ))
            })
            .unwrap();
            assert_eq!(call, cancel_at);
        }
    }
    #[test]
    fn byte_and_control_limits() {
        assert!(decode(vec![b'a'; MAX_BYTES]).is_ok());
        assert!(decode(vec![b'a'; MAX_BYTES + 1]).is_err());
        assert!(decode("é".repeat(MAX_BYTES).into_bytes()).is_err());
        assert!(decode(vec![255]).is_err());
        assert!(decode(b"hello\0secret".to_vec()).is_err());
        assert!(decode(b"line\nnext\tcolumn".to_vec()).is_ok());
    }
    #[test]
    fn invalid_drafts_never_preview() {
        for text in [" ".to_owned(), "x".repeat(MAX_BYTES + 1)] {
            let mut call = 0;
            assert!(screen(|_, _| {
                call += 1;
                Ok(Some(match call {
                    1 => "".into(),
                    2 => "Problem".into(),
                    3 => text.clone(),
                    _ => panic!("invalid preview"),
                }))
            })
            .is_err());
            assert_eq!(call, 3);
        }
    }
}
