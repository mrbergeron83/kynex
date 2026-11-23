//! The chat composer's input features from the `claude` CLI, as rules the app only
//! draws: leading-character modes (`!` shell, `#` memory, `?` shortcuts), the shell
//! commands `!` runs, memory notes, and the thinking switch and `ultrathink` keyword.

pub mod input_mode;
pub mod memory;
pub mod shell;
pub mod thinking;
