//! Explicit local destinations for one session. Remote session text is never a command.
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub enum Destination {
    Forum,
    Editor,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: &'static str,
    pub argument: String,
}

pub fn configured(dir: &Path, session: &str, destination: Destination) -> Result<Launch, String> {
    let session =
        crate::session_cli::canonical_session(session).ok_or("No valid active session")?;
    let file = std::fs::File::open(crate::settings::companion_path(dir))
        .map_err(|_| "Configure this session's destination in companion.toml")?;
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|_| "Cannot read companion destinations")?;
    if text.len() > 65536 {
        return Err("Companion configuration is too large".into());
    }
    let table: toml::Value =
        toml::from_str(&text).map_err(|_| "Invalid companion configuration")?;
    let key = match destination {
        Destination::Forum => "forum_url",
        Destination::Editor => "workspace",
    };
    let value = table
        .get("sessions")
        .and_then(|v| v.get(&session))
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .ok_or("Configure this session's destination in companion.toml")?;
    resolve(value, destination)
}

fn resolve(value: &str, destination: Destination) -> Result<Launch, String> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("Invalid session destination".into());
    }
    match destination {
        Destination::Forum => {
            let url = url::Url::parse(value).map_err(|_| "Invalid forum URL")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Forum destination must be HTTPS without credentials, query or fragment".into(),
                );
            }
            Ok(Launch {
                program: "xdg-open",
                argument: url.into(),
            })
        }
        Destination::Editor => {
            let path = PathBuf::from(value);
            if !path.is_absolute() || !path.is_dir() {
                return Err("Workspace must be an existing absolute directory".into());
            }
            Ok(Launch {
                program: "code",
                argument: value.into(),
            })
        }
    }
}

pub fn open(launch: Launch) -> Result<(), String> {
    // A single literal argument; no shell, command template or remote environment.
    std::process::Command::new(launch.program)
        .arg(launch.argument)
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not open this session destination".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forum_refuses_credentials_and_command_schemes() {
        for value in [
            "file:///tmp/a",
            "javascript:alert(1)",
            "http://example.test/t/1",
            "https://owner:token@example.test/t/1",
            "https://example.test/t/1?token=x",
            "https://example.test/t/1#secret",
            "https://example.test/\n",
        ] {
            assert!(resolve(value, Destination::Forum).is_err());
        }
        assert_eq!(
            resolve("https://forum.example.test/t/123", Destination::Forum).unwrap(),
            Launch {
                program: "xdg-open",
                argument: "https://forum.example.test/t/123".into()
            }
        );
    }
    #[test]
    fn workspace_is_a_literal_absolute_directory() {
        assert!(resolve("--install-extension", Destination::Editor).is_err());
        assert!(resolve("./workspace", Destination::Editor).is_err());
        let path =
            std::env::temp_dir().join(format!("visor-workspace-{}-$(false)", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let plan = resolve(path.to_str().unwrap(), Destination::Editor).unwrap();
        assert_eq!(plan.argument, path.to_str().unwrap());
        std::fs::remove_dir(&path).unwrap();
    }
    #[test]
    fn destinations_do_not_fall_back_to_another_session() {
        let dir = std::env::temp_dir().join(format!("visor-destinations-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        std::fs::write(
            crate::settings::companion_path(&dir),
            format!("[sessions.\"{id}\"]\nforum_url = \"https://forum.example.test/t/123\"\n"),
        )
        .unwrap();
        assert!(configured(&dir, id, Destination::Forum).is_ok());
        assert!(configured(
            &dir,
            "00000000-0000-4000-8000-000000000002",
            Destination::Forum
        )
        .is_err());
        assert!(configured(&dir, id, Destination::Editor).is_err());
        assert!(configured(&dir, "bad", Destination::Forum).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
