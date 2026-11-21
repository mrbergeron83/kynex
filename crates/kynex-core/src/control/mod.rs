//! Controlling the running app from outside: a local socket in the data dir (a named pipe on
//! Windows, a Unix socket elsewhere) that speaks newline-delimited JSON, the `knx` subcommands,
//! and the fast `knx hook` callback for provider CLIs.

pub mod agent;
pub mod args;
pub mod client;
pub mod protocol;
pub mod schema;
pub mod server;

pub use protocol::Request;

use std::path::{Path, PathBuf};

/// The file in `dir` (the data dir) that says where the running app listens.
fn addr_file(dir: &Path) -> PathBuf {
    dir.join("control.addr")
}
