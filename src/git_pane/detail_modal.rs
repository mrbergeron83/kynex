//! What was picked in the Git pane (a commit in History: its message, author, files and
//! diff), shown in a modal over the window by the worktree's own [`GitView`].

use crate::ui::px;
use gpui::{Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Window, div, prelude::*};

use crate::git::GitView;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, icon_button};

/// What the view asks of the app.
pub enum DetailEvent {
    Close,
}

impl EventEmitter<DetailEvent> for DetailView {}

pub struct DetailView {
    git: Entity<GitView>,
    /// What it shows: "Commit", "Compare".
    title: &'static str,
    focus: FocusHandle,
    focused: bool,
}

impl DetailView {
    pub fn new(git: Entity<GitView>, title: &'static str, cx: &mut Context<Self>) -> Self {
        cx.observe(&git, |_, _, cx| cx.notify()).detach();
        Self { git, title, focus: cx.focus_handle(), focused: false }
    }
}

impl Focusable for DetailView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The keys (Esc) are the modal's, not the composer's behind it.
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        let t = theme(cx).clone();
        let body = self.git.update(cx, |g, cx| g.render_detail(cx));
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(32.))
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::GitCommit, t.text_muted).size_3p5())
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(t.text_muted).child(self.title))
            .child(div().flex_1())
            .child(
                icon_button("git-detail-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Close))),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .child(bar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
