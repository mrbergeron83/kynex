//! Kynex's logic, with no UI: everything here is plain Rust, testable with
//! `cargo test -p kynex-core`. Blocking calls (processes, git, the network) are
//! marked as such; the app runs them off the UI thread.

pub mod attach;
pub mod attention;
pub mod awake;
pub mod cli;
pub mod column;
pub mod commands;
pub mod dates;
pub mod env_path;
pub mod extensions;
pub mod fmt;
#[doc(hidden)]
pub mod fuzz;
pub mod fuzzy;
pub mod git;
pub mod groups;
pub mod highlight;
pub mod history;
pub mod links;
pub mod live;
pub mod lru;
pub mod markdown;
pub mod mcp;
pub mod mcp_config;
pub mod mentions;
pub mod motion;
pub mod open;
pub mod palette;
pub mod paths;
pub mod process;
pub mod project;
pub mod pulse;
pub mod questions;
pub mod queue;
pub mod resources;
pub mod rewind;
pub mod scale;
pub mod session;
pub mod session_log;
pub mod tasks;
pub mod terminal;
pub mod timeline;
pub mod titles;
pub mod toasts;
pub mod transcript;
pub mod tray;
pub mod update;
pub mod usage;
#[cfg(windows)]
pub mod win_shell;
pub mod wsl;
pub mod zoom;
