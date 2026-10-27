//! Kynex's logic, with no UI: everything here is plain Rust, testable with
//! `cargo test -p kynex-core`. Blocking calls (processes, git, the network) are
//! marked as such; the app runs them off the UI thread.

pub mod attach;
pub mod cli;
pub mod commands;
pub mod fmt;
pub mod git;
pub mod groups;
pub mod history;
pub mod live;
pub mod motion;
pub mod open;
pub mod paths;
pub mod process;
pub mod project;
pub mod questions;
pub mod queue;
pub mod rewind;
pub mod session;
pub mod terminal;
pub mod timeline;
pub mod titles;
pub mod transcript;
pub mod update;
pub mod wsl;
