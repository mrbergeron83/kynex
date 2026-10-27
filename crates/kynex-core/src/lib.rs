//! Kynex's logic, with no UI: everything here is plain Rust, testable with
//! `cargo test -p kynex-core`. Blocking calls (processes, git, the network) are
//! marked as such; the app runs them off the UI thread.

pub mod attach;
pub mod cli;
pub mod fmt;
pub mod git;
pub mod live;
pub mod open;
pub mod paths;
pub mod project;
pub mod session;
pub mod terminal;
pub mod titles;
pub mod update;
pub mod wsl;
