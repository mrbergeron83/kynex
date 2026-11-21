//! Menus and modals animating out as they close (`ui::motion::Leaving`).

use gpui::{AnyElement, Context, Window};

use super::KynexApp;
use super::app_settings::AppSettingsView;
use super::menus::OpenMenu;
use super::palette::PaletteView;
use crate::ui::motion::{Leaving, QUICK};

pub(super) struct Exits {
    pub menu: Leaving<OpenMenu>,
    pub palette: Leaving<PaletteView>,
    pub app_settings: Leaving<AppSettingsView>,
    pub about: Leaving<()>,
    pub git_file: Leaving<super::git_file_modal::GitDiffModal>,
}

impl Default for Exits {
    fn default() -> Self {
        Self {
            menu: Leaving::new(QUICK),
            palette: Leaving::new(QUICK),
            app_settings: Leaving::new(QUICK),
            about: Leaving::new(QUICK),
            git_file: Leaving::new(QUICK),
        }
    }
}

impl KynexApp {
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
