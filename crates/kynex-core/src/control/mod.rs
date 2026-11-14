//! Controlling the running app from outside: a local socket in the data dir (a named pipe on
//! Windows, a Unix socket elsewhere) that speaks newline-delimited JSON, and the `knx`
//! subcommands that talk to it.
//!
//! - [`protocol`]: the requests, the response lines, and the parser for what arrives (fuzzed).
//! - [`args`]: command-line words into a request.
//! - [`server`] / [`client`]: the two ends of the socket.
//!
//! Who may connect: the socket's address is in `control.addr` in the data dir, and on Unix
//! the socket file is private to the user (mode 0600). On Windows the pipe's name is random
//! for each run, so only a process that can read the user's data dir finds it.

pub mod agent;
pub mod args;
pub mod client;
pub mod protocol;
pub mod server;

pub use protocol::Request;

use std::path::{Path, PathBuf};

/// The file in `dir` (the data dir) that says where the running app listens.
fn addr_file(dir: &Path) -> PathBuf {
    dir.join("control.addr")
}
