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
use crate::app::rewind::RewindMenu;
use crate::app::slash::Slash;
use crate::app::timeline::Flash;
use crate::ui::input::TextInput;

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
            expand_tools: false,
        }
    }
}
