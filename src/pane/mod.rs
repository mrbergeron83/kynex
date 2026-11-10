//! `SessionPane`: one session view's message box and everything that goes with it. The
//! window holds exactly one for now; each pane will own its composer, so what used to be
//! single fields on `KynexApp` lives here.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{Bounds, Entity, Pixels};
use kynex_core::live::Key;
use kynex_core::session::Attachment;

use crate::app::find::FindBar;
use crate::app::history::HistorySearch;
use crate::app::mentions::Mentions;
use crate::app::prompts::ActivePrompt;
use crate::app::rewind::RewindMenu;
use crate::app::slash::Slash;
use crate::app::timeline::Flash;
use crate::prompt::PromptView;
use crate::ui::input::TextInput;
use crate::ui::motion::{Leaving, QUICK};

pub struct SessionPane {
    /// The message box.
    pub(crate) composer: Entity<TextInput>,
    /// Images attached to the message being written.
    pub(crate) attachments: Vec<Attachment>,
    /// The session's uid and draft key whose message is in the composer.
    pub(crate) draft_for: Option<(Key, String)>,
    /// The slash-command popup.
    pub(crate) slash: Slash,
    /// The `@file` mention popup.
    pub(crate) mentions: Mentions,
    /// Find in the transcript (Ctrl+F).
    pub(crate) find: Option<FindBar>,
    /// Ctrl+R over the sent messages.
    pub(crate) history_search: Option<HistorySearch>,
    /// The rewind popup (Esc Esc on an empty composer), while open.
    pub(crate) rewind: Option<RewindMenu>,
    /// The transcript row lit up after a jump from the timeline.
    pub(crate) flash: Option<Flash>,
    /// When Esc last went to a draft, for Esc Esc.
    pub(crate) last_esc: Option<Instant>,
    /// The conversation's free area last frame (between its header and the composer),
    /// which the toasts stay clear of; `None` in other views.
    pub(crate) toast_room: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The permission, plan or question prompt up in this pane, in the composer's place.
    pub(crate) prompt: Option<ActivePrompt>,
    /// The prompt that just closed, fading out.
    pub(crate) prompt_exit: Leaving<Entity<PromptView>>,
    /// Ctrl+O: every tool call in the transcript shown open.
    pub(crate) expand_tools: bool,
}

impl SessionPane {
    pub fn new(composer: Entity<TextInput>) -> Self {
        Self {
            composer,
            attachments: Vec::new(),
            draft_for: None,
            slash: Slash::default(),
            mentions: Mentions::default(),
            find: None,
            history_search: None,
            rewind: None,
            flash: None,
            last_esc: None,
            toast_room: Rc::default(),
            prompt: None,
            prompt_exit: Leaving::new(QUICK),
            expand_tools: false,
        }
    }

    /// The pane has a prompt waiting on you: its "needs you" state, whether or not you're in it.
    pub fn needs_you(&self) -> bool {
        self.prompt.is_some()
    }

    /// Where the keyboard belongs: the prompt, when it is for the session you're in, else
    /// the composer. A prompt in another pane never pulls focus away.
    pub fn prompt_has_focus(&self, focused: Option<Key>) -> bool {
        self.prompt.as_ref().is_some_and(|p| prompt_focus(p.key, focused))
    }
}

/// Whether a prompt for session `prompt` takes the keyboard when `focused` is the session
/// being typed in.
fn prompt_focus(prompt: Key, focused: Option<Key>) -> bool {
    Some(prompt) == focused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_in_another_pane_never_takes_focus() {
        assert!(prompt_focus(1, Some(1)));
        assert!(!prompt_focus(2, Some(1)), "it raises needs-you instead");
        assert!(!prompt_focus(1, None));
    }
}
