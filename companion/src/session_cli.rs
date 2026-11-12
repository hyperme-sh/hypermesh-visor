//! Interactive prompt entry for one existing session. No automatic retries.
use std::io::{BufRead, Read, Write};

const MAX_PROMPT_BYTES: usize = 16 * 1024;

pub fn canonical_session(value: &str) -> Option<String> {
    (value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }))
    .then(|| value.to_ascii_lowercase())
}

pub fn run(
    mut input: impl BufRead,
    output: &mut impl Write,
    session: &str,
    mut execute: impl FnMut(&[String], &str) -> Result<(), String>,
) -> Result<(), String> {
    let session = canonical_session(session).ok_or("a canonical session UUID is required")?;
    writeln!(
        output,
        "Visor session {session}\nEnter one prompt per line. /quit or EOF closes this terminal."
    )
    .map_err(|_| "terminal output closed")?;
    loop {
        write!(output, "> ")
            .and_then(|_| output.flush())
            .map_err(|_| "terminal output closed")?;
        let mut line = String::new();
        let count = (&mut input)
            .take((MAX_PROMPT_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|_| "could not read prompt")?;
        if count == 0 {
            return Ok(());
        }
        if count > MAX_PROMPT_BYTES {
            return Err("prompt exceeds 16 KiB; nothing was sent".into());
        }
        let prompt = line.trim_end_matches(['\r', '\n']);
        if prompt == "/quit" {
            return Ok(());
        }
        if prompt.trim().is_empty() {
            continue;
        }
        let args = vec!["prompt".into(), "--session".into(), session.clone()];
        if let Err(error) = execute(&args, prompt) {
            writeln!(output, "{error}; prompt was not retried")
                .map_err(|_| "terminal output closed")?;
        }
    }
}

pub fn start(session: &str) -> Result<(), String> {
    run(
        std::io::stdin().lock(),
        &mut std::io::stdout(),
        session,
        |args, prompt| dispatch(std::path::Path::new("hypermesh"), args, prompt),
    )
}

fn dispatch(program: &std::path::Path, args: &[String], prompt: &str) -> Result<(), String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "could not start the Hypermesh CLI".to_owned())?;
    let sent = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        .and_then(|mut input| input.write_all(prompt.as_bytes()));
    if sent.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("could not send prompt to the Hypermesh CLI".into());
    }
    let status = child
        .wait()
        .map_err(|_| "could not wait for the Hypermesh CLI".to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err("Hypermesh CLI reported a failure".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "01234567-89ab-cdef-0123-456789abcdef";

    #[test]
    fn child_process_gets_exact_stdin_without_prompt_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "visor-cli-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let program = dir.join("mock cli");
        std::fs::write(&program, "#!/usr/bin/env python3\nimport json, pathlib, sys\ntext=sys.stdin.read()\npathlib.Path(__file__).with_suffix('.json').write_text(json.dumps([sys.argv[1:],text]))\nsys.exit(7 if text == 'fail' else 0)\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let args = vec!["prompt".into(), "--session".into(), SESSION.into()];
        let prompt = "private prompt; $(not a command) — text";
        dispatch(&program, &args, prompt).unwrap();
        let recorded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(program.with_extension("json")).unwrap())
                .unwrap();
        assert_eq!(recorded, serde_json::json!([args, prompt]));
        assert!(dispatch(&program, &args, "fail").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn identity_validation_never_accepts_paths_options_or_shell_text() {
        assert_eq!(
            canonical_session(&SESSION.to_uppercase()).as_deref(),
            Some(SESSION)
        );
        for value in [
            "",
            "--help",
            "../../../other",
            "$(echo bad)",
            "0123456789abcdef0123456789abcdef",
            "01234567-89ab-cdef-0123-456789abcdeg",
        ] {
            assert!(canonical_session(value).is_none());
            let mut calls = 0;
            assert!(run(
                std::io::Cursor::new("hello\n"),
                &mut Vec::new(),
                value,
                |_, _| {
                    calls += 1;
                    Ok(())
                }
            )
            .is_err());
            assert_eq!(calls, 0);
        }
    }

    #[test]
    fn prompts_are_literal_and_session_is_fixed_without_retries() {
        let mut calls = Vec::new();
        let mut output = Vec::new();
        run(
            std::io::Cursor::new("\n$(touch /tmp/no) --session other\nnext\n/quit\nignored\n"),
            &mut output,
            SESSION,
            |args, prompt| {
                calls.push((args.to_vec(), prompt.to_owned()));
                Err("fixture failure".into())
            },
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, ["prompt", "--session", SESSION]);
        assert_eq!(calls[0].1, "$(touch /tmp/no) --session other");
        assert_eq!(calls[1].1, "next");
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("prompt was not retried"));
    }

    #[test]
    fn oversized_prompt_and_output_loss_never_dispatch() {
        let mut calls = 0;
        let input = "x".repeat(MAX_PROMPT_BYTES + 1);
        assert!(run(
            std::io::Cursor::new(input),
            &mut Vec::new(),
            SESSION,
            |_, _| {
                calls += 1;
                Ok(())
            }
        )
        .is_err());
        assert_eq!(calls, 0);
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(run(
            std::io::Cursor::new("hello\n"),
            &mut Closed,
            SESSION,
            |_, _| {
                calls += 1;
                Ok(())
            }
        )
        .is_err());
        assert_eq!(calls, 0);
        run(
            std::io::Cursor::new(""),
            &mut Vec::new(),
            SESSION,
            |_, _| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 0);
    }
}
