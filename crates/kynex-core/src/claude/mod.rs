//! Pure-Rust native interface to the user's `claude` CLI over its bidirectional
//! NDJSON `stream-json` control protocol, plus session cost persistence and
//! transcript/composer helpers. Zero UI/gpui dependency.

pub mod attach;
pub mod client;
pub mod column;
pub mod commands;
pub mod composer;
pub mod costs;
pub mod diff;
pub mod edit_lines;
pub mod find;
pub mod history;
pub mod input;
pub mod markdown;
pub mod mcp;
pub mod mentions;
pub mod model_groups;
pub mod prices;
pub mod prompt_history;
pub mod protocol;
pub mod pulse;
pub mod questions;
pub mod queue;
pub mod rewind;
pub mod runs;
pub mod session;
pub mod session_log;
pub mod spent;
pub mod subagents;
pub mod tasks;
pub mod timeline;
pub mod titles;
pub mod todos;
pub mod transcript;
pub mod types;
pub mod usage;
pub mod why;

pub use client::ClaudeClient;
pub use types::*;
