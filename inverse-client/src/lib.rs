//! Portable inverse MCP protocol client. It does not host a desktop daemon.
// Shared implementations also contain daemon-only helpers.
#[allow(dead_code)]
#[path = "../../src/input.rs"]
mod input;
#[allow(dead_code)]
#[path = "../../src/telemetry.rs"]
mod telemetry;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../shared/local_runtime.rs"]
mod local_runtime;
mod client;
pub mod acp;
pub use client::{run, run_with_version};
