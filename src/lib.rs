//! Local Linux daemon the supervisor starts so a caller can see and drive a desktop.
//! The session verbs are the stable surface. Version one implements them on Linux.

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("hypermesh-visor version one is Linux only");

#[cfg(all(feature = "desktop", feature = "headless"))]
compile_error!("desktop and headless are exclusive; use --no-default-features --features headless");

mod api;
mod automation;
mod bind;
mod capabilities;
#[cfg(unix)]
mod companion_sock;
#[cfg(windows)]
#[path = "companion_windows.rs"]
mod companion_sock;
mod conductor_socket;
mod desktop;
mod harness;
mod host_bridge;
mod input;
#[cfg(unix)]
mod local_peer;
#[cfg(unix)]
#[path = "../shared/local_runtime.rs"]
mod local_runtime;
#[cfg(all(any(target_os = "macos", windows), feature = "desktop"))]
mod native_engine;
#[cfg(all(any(target_os = "macos", windows), feature = "desktop"))]
#[path = "../shared/session_destinations.rs"]
mod session_destinations;
#[cfg(any(target_os = "macos", windows, test))]
mod native_prompt;
mod mcp;
mod orchestrator;
mod prompt;
mod session;
mod stream;
pub mod telemetry;
pub mod health_consent;
pub mod crash_report;
pub mod crash_dump;
pub mod crash_upload;
pub mod crash_dialog;
pub mod crash_recovery;
pub mod crash_startup;
pub mod crash_capture;
mod vault;
mod usage;
pub use usage::ResponseUsage;

pub use api::{serve, serve_until, AppState};
pub use automation::SHUTDOWN_DEADLINE;
pub use bind::parse_listen;
pub use conductor_socket::default_path as default_conductor_socket;
pub use companion_sock::{control_socket_file, read_api_token, runtime_dir};
#[cfg(all(unix, feature = "desktop"))]
pub use desktop::LinuxDesktop;
#[cfg(all(any(target_os = "macos", windows), feature = "desktop"))]
pub use desktop::NativeDesktop;
#[cfg(all(target_os = "macos", feature = "desktop"))]
pub use desktop::MacDesktop;
#[cfg(unix)]
pub use desktop::current_euid;
pub use desktop::{Desktop, FixtureDesktop, HeadlessDesktop};
pub use hypermesh_session::{refresh_token, MemoryStore, SessionStore};
pub use input::{key_chord_request, SCROLL_MAX_TICKS};
pub use prompt::{open_supervisor_door, DoorConfig, PromptCancellation, PromptCompletion, PromptDoor, DEFAULT_CATALOG_ID};

#[cfg(test)]
mod session_key {
    use super::*;
    use hypermesh_session::SessionStore;

    #[test]
    fn visor_reads_the_shared_refresh_token() {
        let store = MemoryStore::new();
        assert_eq!(refresh_token(&store).unwrap(), None);
        store.put_refresh_token("refresh-1").unwrap();
        assert_eq!(refresh_token(&store).unwrap().as_deref(), Some("refresh-1"));
        let shown = format!("{store:?}");
        assert!(!shown.contains("refresh-1"));
    }
}
