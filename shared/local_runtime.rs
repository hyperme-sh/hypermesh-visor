//! Local client path selection shared by the daemon and inverse MCP client.
use std::{fs, io, path::{Path, PathBuf}};

pub const SOCKET_NAME: &str = "companion.sock";
/// Full control API for the same user. It does not carry the companion's
/// pause-lift privilege.
pub const CONTROL_SOCKET_NAME: &str = "control.sock";
/// Per-launch bearer for the opt-in TCP listener. Mode 0600.
pub const API_TOKEN_NAME: &str = "api.token";

/// `sun_path` holds 108 bytes including the trailing NUL.
#[cfg(target_os = "macos")]
const SUN_PATH_MAX: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH_MAX: usize = 108;

pub fn runtime_dir() -> io::Result<PathBuf> {
    runtime_dir_from(
        std::env::var("HYPERMESH_RUNTIME_DIR").ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
    .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

/// `HYPERMESH_RUNTIME_DIR`, then `$XDG_RUNTIME_DIR/hypermesh`, then
/// `$XDG_STATE_HOME/hypermesh/run`, then `$HOME/.local/state/hypermesh/run`.
/// The socket path has to fit in [`SUN_PATH_MAX`].
pub fn runtime_dir_from(
    override_dir: Option<&str>,
    xdg_runtime: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, &'static str> {
    let dir = if let Some(dir) = nonempty(override_dir) {
        PathBuf::from(dir)
    } else if let Some(dir) = nonempty(xdg_runtime) {
        PathBuf::from(dir).join("hypermesh")
    } else if let Some(dir) = nonempty(xdg_state) {
        PathBuf::from(dir).join("hypermesh").join("run")
    } else if let Some(dir) = nonempty(home) {
        PathBuf::from(dir)
            .join(".local")
            .join("state")
            .join("hypermesh")
            .join("run")
    } else {
        return Err(
            "companion socket needs HYPERMESH_RUNTIME_DIR, XDG_RUNTIME_DIR, XDG_STATE_HOME, or HOME",
        );
    };
    let socket = dir.join(SOCKET_NAME);
    if socket.as_os_str().len() >= SUN_PATH_MAX {
        return Err("companion socket path does not fit in sun_path");
    }
    Ok(dir)
}

pub fn socket_file(dir: &Path) -> PathBuf {
    dir.join(SOCKET_NAME)
}

pub fn control_socket_file(dir: &Path) -> PathBuf {
    dir.join(CONTROL_SOCKET_NAME)
}

pub fn api_token_file(dir: &Path) -> PathBuf {
    dir.join(API_TOKEN_NAME)
}

pub fn read_api_token(dir: &Path) -> io::Result<String> {
    let text = fs::read_to_string(api_token_file(dir))?;
    let token = text.trim();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "api token is empty",
        ));
    }
    Ok(token.to_string())
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
