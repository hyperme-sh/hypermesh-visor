//! Local Linux daemon the supervisor starts so a caller can see and drive a desktop.
//! The session verbs are the stable surface. Version one implements them on Linux.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("hypermesh-visor version one is Linux only");

#[cfg(all(feature = "desktop", feature = "headless"))]
compile_error!("desktop and headless are exclusive; use --no-default-features --features headless");

mod api;
mod automation;
mod bind;
mod capabilities;
mod companion_sock;
mod desktop;
mod harness;
mod input;
mod local_peer;
#[cfg(all(target_os = "macos", feature = "desktop"))]
mod native_engine;
#[cfg(any(target_os = "macos", test))]
mod native_prompt;
mod mcp;
mod orchestrator;
mod prompt;
mod session;
mod stream;
pub mod telemetry;
mod vault;
mod usage;
pub use usage::ResponseUsage;

pub use api::{serve, serve_until, AppState};
pub use automation::SHUTDOWN_DEADLINE;
pub use bind::parse_listen;
pub use companion_sock::{control_socket_file, read_api_token, runtime_dir};
#[cfg(feature = "desktop")]
pub use desktop::LinuxDesktop;
#[cfg(all(target_os = "macos", feature = "desktop"))]
pub use desktop::MacDesktop;
pub use desktop::{current_euid, Desktop, FixtureDesktop, HeadlessDesktop};
pub use hypermesh_session::{refresh_token, MemoryStore, SessionStore};
pub use input::{key_chord_request, SCROLL_MAX_TICKS};
pub use prompt::{open_supervisor_door, DoorConfig, PromptCompletion, PromptDoor, DEFAULT_CATALOG_ID};

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
