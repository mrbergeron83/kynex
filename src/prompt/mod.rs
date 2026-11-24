//! The prompt Claude shows when it needs you: a tool to allow, a plan to approve
//! or questions to answer. For now its previews only, which the old chat's
//! transcript rows use too (`crate::claude_pane`); the card itself comes back with
//! the permission prompts (#545).
//!
//! | file           | what                                                    |
//! |----------------|---------------------------------------------------------|
//! | `preview.rs`   | titles, "don't ask again" wording, previews and diffs   |

mod preview;

/// The tool name a `knx` request shows under in the card.
pub const CARD_TOOL: &str = "kynex:request";

pub use preview::{diff_stat, preview, title};
