//! Menus, modals, panels and popups animating out as they close, the way they
//! animate in: each keeps what it was showing for a moment after it closes
//! (`ui::motion::Leaving`), and is drawn from that, out of the mouse's reach.

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AnyElement, Context, Entity, Window};

use super::KynexApp;
use super::app_settings::AppSettingsView;
use super::menus::OpenMenu;
use super::settings::SettingsView;
use crate::prompt::PromptView;
use crate::ui::completions::Completion;
use crate::ui::image_viewer::ImageViewer;
use crate::ui::motion::{Leaving, NORMAL, QUICK};

/// What's on its way out, one tracker per closable thing.
pub(super) struct Exits {
    pub menu: Leaving<OpenMenu>,
    pub settings: Leaving<SettingsView>,
    pub app_settings: Leaving<AppSettingsView>,
    pub about: Leaving<()>,
    pub git_modal: Leaving<PathBuf>,
    pub viewer: Leaving<Entity<ImageViewer>>,
    pub sessions: Leaving<()>,
    pub terminal: Leaving<()>,
    /// The prompt that had the composer's place.
    pub prompt: Leaving<Entity<PromptView>>,
    /// The slash-command list and its selection.
    pub slash: Leaving<(Rc<Vec<Completion>>, usize)>,
}

impl Default for Exits {
    fn default() -> Self {
        Self {
            menu: Leaving::new(QUICK),
            settings: Leaving::new(QUICK),
            app_settings: Leaving::new(QUICK),
            about: Leaving::new(QUICK),
            git_modal: Leaving::new(QUICK),
            viewer: Leaving::new(QUICK),
            sessions: Leaving::new(NORMAL),
            terminal: Leaving::new(QUICK),
            prompt: Leaving::new(QUICK),
            slash: Leaving::new(QUICK),
        }
    }
}

impl KynexApp {
    /// Draws what an `Option` field opens: as usual while it's set, and for a
    /// moment after it's cleared with what it last held put back, so `draw`
    /// can play it out (`Some` of how far it has gone).
    pub(super) fn draw_leaving<T: Clone>(
        &mut self,
        field: fn(&mut Self) -> &mut Option<T>,
        exit: fn(&mut Exits) -> &mut Leaving<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
        draw: impl FnOnce(&mut Self, Option<f32>, &mut Window, &mut Context<Self>) -> Option<AnyElement>,
    ) -> Option<AnyElement> {
        let open = field(self).clone();
        let Some((held, t)) = exit(&mut self.exits).track(open.as_ref(), window, cx) else {
            return open.is_some().then(|| draw(self, None, window, cx)).flatten();
        };
        *field(self) = Some(held);
        let drawn = draw(self, Some(t), window, cx);
        *field(self) = None;
        drawn
    }
}
