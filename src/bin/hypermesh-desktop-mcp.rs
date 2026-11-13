//! Packaged desktop entry point; the portable client owns the protocol.
fn main() { hypermesh_visor_client::run_with_version(env!("CARGO_PKG_VERSION")); }
