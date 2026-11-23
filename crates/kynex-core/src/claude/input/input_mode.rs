//! What the composer is in: a prompt for Claude, or one of the CLI's modes a leading
//! character switches to. `!` runs a shell command, `#` adds a line to a memory file
//! (CLAUDE.md) and `?` shows the shortcuts. The character itself isn't kept: the mode
//! shows instead, and Backspace in the empty field goes back to a prompt.

/// The composer's mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    /// A message for Claude.
    #[default]
    Prompt,
    /// `!`: a shell command, run in the session's folder.
    Shell,
    /// `#`: a note for a memory file.
    Memory,
    /// `?`: the composer's shortcuts, shown while the field is empty.
    Shortcuts,
}

/// The composer's text and mode after a change, when the change switched modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub mode: InputMode,
    pub text: String,
}

/// What Enter does with the composer's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submit {
    /// Send it to Claude.
    Send(String),
    /// Run this shell command.
    Shell(String),
    /// Ask where this note goes, then add it there.
    Memory(String),
    /// Nothing to do (an empty command or note, the shortcuts).
    Nothing,
}

impl InputMode {
    /// The character that switches to this mode from an empty prompt.
    pub fn prefix(self) -> Option<char> {
        match self {
            InputMode::Prompt => None,
            InputMode::Shell => Some('!'),
            InputMode::Memory => Some('#'),
            InputMode::Shortcuts => Some('?'),
        }
    }

    fn from_prefix(c: char) -> Option<Self> {
        [InputMode::Shell, InputMode::Memory, InputMode::Shortcuts].into_iter().find(|m| m.prefix() == Some(c))
    }

    /// Its name, shown beside the field.
    pub fn label(self) -> &'static str {
        match self {
            InputMode::Prompt => "",
            InputMode::Shell => "Shell",
            InputMode::Memory => "Memory",
            InputMode::Shortcuts => "Shortcuts",
        }
    }

    /// The field's placeholder in this mode.
    pub fn placeholder(self) -> &'static str {
        match self {
            InputMode::Prompt => "",
            InputMode::Shell => "Run a command in the session's folder; Claude sees it with your next message",
            InputMode::Memory => "Add a line to CLAUDE.md…",
            InputMode::Shortcuts => "Type to write a message · Backspace or Esc to close",
        }
    }

    /// Whether the mode works in a session of this sort: a chat has no folder to
    /// run commands in or a project to remember for.
    pub fn allowed(self, code_session: bool) -> bool {
        code_session || matches!(self, InputMode::Prompt | InputMode::Shortcuts)
    }

    /// The composer's text after a change from `prev` to `text` in this mode, when it
    /// switches modes. In an empty prompt `!` or `#` switches to its mode (what follows
    /// it, from a paste or a recalled message, stays) and a lone `?` shows the shortcuts.
    /// A `#` with more than one line after it is markdown, not a note: it stays a
    /// prompt. Typing over the shortcuts goes back to a prompt that starts with the `?`.
    pub fn on_change(self, prev: &str, text: &str, code_session: bool) -> Option<Switch> {
        match self {
            InputMode::Prompt if prev.is_empty() => {
                let mut chars = text.chars();
                let mode = chars.next().and_then(Self::from_prefix).filter(|m| m.allowed(code_session))?;
                let rest = chars.as_str();
                match mode {
                    InputMode::Shortcuts if !rest.is_empty() => None,
                    InputMode::Memory if rest.contains('\n') => None,
                    _ => Some(Switch { mode, text: rest.to_string() }),
                }
            }
            InputMode::Shortcuts if !text.is_empty() => {
                Some(Switch { mode: InputMode::Prompt, text: format!("?{text}") })
            }
            _ => None,
        }
    }

    /// The mode after Backspace (or Esc) in an empty field: back to a prompt.
    pub fn on_backspace(self, text: &str) -> Option<InputMode> {
        (self != InputMode::Prompt && text.is_empty()).then_some(InputMode::Prompt)
    }

    /// What Enter does with `text` in this mode.
    pub fn submit(self, text: &str) -> Submit {
        let t = text.trim();
        match self {
            InputMode::Prompt => Submit::Send(t.to_string()),
            InputMode::Shortcuts => Submit::Nothing,
            _ if t.is_empty() => Submit::Nothing,
            InputMode::Shell => Submit::Shell(t.to_string()),
            InputMode::Memory => Submit::Memory(t.to_string()),
        }
    }

    /// `text` with this mode's prefix, as kept in the history and drafts: recalled
    /// into an empty composer, it switches back to this mode.
    pub fn prefixed(self, text: &str) -> String {
        match self.prefix() {
            Some(c) if self != InputMode::Shortcuts => format!("{c}{text}"),
            _ => text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InputMode::*;
    use super::*;

    fn switch(mode: InputMode, text: &str) -> Option<Switch> {
        Some(Switch { mode, text: text.into() })
    }

    #[test]
    fn a_leading_character_in_an_empty_prompt_switches() {
        assert_eq!(Prompt.on_change("", "!", true), switch(Shell, ""));
        assert_eq!(Prompt.on_change("", "#", true), switch(Memory, ""));
        assert_eq!(Prompt.on_change("", "?", true), switch(Shortcuts, ""));
        // Pasted or recalled with the prefix: the rest stays.
        assert_eq!(Prompt.on_change("", "!git status", true), switch(Shell, "git status"));
        assert_eq!(Prompt.on_change("", "#use tabs", true), switch(Memory, "use tabs"));
    }

    #[test]
    fn only_from_an_empty_prompt() {
        assert_eq!(Prompt.on_change("hi", "hi!", true), None);
        assert_eq!(Prompt.on_change("", "hello", true), None);
        assert_eq!(Shell.on_change("", "!", true), None);
        // `?` with more after it is a question, a `#` heading with a body is markdown.
        assert_eq!(Prompt.on_change("", "?why", true), None);
        assert_eq!(Prompt.on_change("", "# Title\n\nbody", true), None);
    }

    #[test]
    fn a_chat_has_no_shell_or_memory() {
        assert_eq!(Prompt.on_change("", "!", false), None);
        assert_eq!(Prompt.on_change("", "#", false), None);
        assert_eq!(Prompt.on_change("", "?", false), switch(Shortcuts, ""));
    }

    #[test]
    fn typing_over_the_shortcuts_keeps_the_question_mark() {
        assert_eq!(Shortcuts.on_change("", "w", true), switch(Prompt, "?w"));
        assert_eq!(Shortcuts.on_change("", "", true), None);
    }

    #[test]
    fn backspace_in_an_empty_field_leaves_the_mode() {
        assert_eq!(Shell.on_backspace(""), Some(Prompt));
        assert_eq!(Memory.on_backspace(""), Some(Prompt));
        assert_eq!(Shell.on_backspace("ls"), None);
        assert_eq!(Prompt.on_backspace(""), None);
    }

    #[test]
    fn enter_does_what_the_mode_says() {
        assert_eq!(Prompt.submit(" hi "), Submit::Send("hi".into()));
        assert_eq!(Shell.submit(" ls -la "), Submit::Shell("ls -la".into()));
        assert_eq!(Memory.submit("tabs, not spaces"), Submit::Memory("tabs, not spaces".into()));
        assert_eq!(Shell.submit("  "), Submit::Nothing);
        assert_eq!(Shortcuts.submit(""), Submit::Nothing);
    }

    #[test]
    fn prefixed_round_trips_through_on_change() {
        for mode in [Shell, Memory] {
            let kept = mode.prefixed("x");
            assert_eq!(Prompt.on_change("", &kept, true), switch(mode, "x"));
        }
        assert_eq!(Prompt.prefixed("x"), "x");
        assert_eq!(Shortcuts.prefixed(""), "");
    }
}
