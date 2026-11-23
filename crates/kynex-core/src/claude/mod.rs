//! Pure-Rust native interface to the user's `claude` CLI over its bidirectional
//! NDJSON `stream-json` control protocol, plus session cost persistence and
//! transcript/composer helpers. Zero UI/gpui dependency.

pub mod client;
pub mod composer;
pub mod costs;
pub mod diff;
pub mod history;
pub mod input;
pub mod markdown;
pub mod prompt_history;
pub mod protocol;
pub mod rewind;
pub mod runs;
pub mod types;

pub use client::ClaudeClient;
pub use types::*;
