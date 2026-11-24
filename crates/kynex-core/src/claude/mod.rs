//! The Claude pane's engine, gpui-free: a `claude` session over its stream-json
//! protocol ([`session`]), its log on disk, and the transcript, composer and
//! timeline helpers the pane draws from.

pub mod attach;
pub mod column;
pub mod commands;
pub mod costs;
pub mod edit_lines;
pub mod find;
pub mod input;
pub mod mcp;
pub mod mentions;
pub mod model_groups;
pub mod prices;
pub mod prompt_history;
pub mod pulse;
pub mod questions;
pub mod queue;
pub mod rewind;
pub mod session;
pub mod session_log;
pub mod spent;
pub mod subagents;
pub mod tasks;
pub mod timeline;
pub mod titles;
pub mod todos;
pub mod transcript;
pub mod usage;
pub mod why;
