//! Kynex's logic, with no UI: everything here is plain Rust, testable with
//! `cargo test -p kynex-core`. Blocking calls (processes, git, the network) are
//! marked as such; the app runs them off the UI thread.

pub mod agent_cli;
pub mod agent_panes;
pub mod antigravity;
pub mod attention;
pub mod awake;
pub mod binding;
pub mod claude;
pub mod cli;
pub mod control;
pub mod dates;
pub mod env_path;
pub mod file_view;
pub mod fmt;
pub mod fresh_env;
#[doc(hidden)]
pub mod fuzz;
pub mod fuzzy;
pub mod git;
pub mod grok;
pub mod groups;
pub mod highlight;
pub mod hooks;
pub mod layout;
pub mod links;
pub mod lru;
pub mod markdown;
pub mod migrate;
pub mod motion;
pub mod named_layouts;
pub mod open;
pub mod openrouter;
pub mod palette;
pub mod paths;
pub mod process;
pub mod project;
pub mod provider;
pub mod removed;
pub mod resources;
pub mod running;
pub mod scale;
pub mod scrollback;
pub mod secrets;
pub mod sessions;
pub mod shortcuts;
pub mod snapshots;
pub mod storage;
pub mod tab_order;
pub mod terminal;
pub mod themes;
pub mod tile_style;
pub mod tiling;
pub mod toasts;
pub mod trace;
pub mod tree;
pub mod update;
pub mod user_commands;
pub mod watch;
#[cfg(windows)]
pub mod win_shell;
pub mod workspace;
pub mod zoom;
