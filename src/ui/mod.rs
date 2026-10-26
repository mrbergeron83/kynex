//! Kynex's handful of components, in the spirit of Zed's `ui` crate: small
//! `RenderOnce`-style builders over `div`, styled from the theme global.

pub mod input;
pub mod selectable;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyView, App, AssetSource, Context, ElementId, Hsla, IntoElement, Pixels, SharedString,
    Stateful, Transformation, Window, div, percentage, prelude::*, px, svg,
};

use crate::theme::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Close,
    Minimize,
    Maximize,
    Restore,
    Plus,
    Settings,
    PanelLeft,
    GitBranch,
    Message,
    Folder,
    Terminal,
    Code,
    Trash,
    ChevronDown,
    ChevronRight,
    Paperclip,
    ArrowUp,
    Stop,
    Loader,
    Check,
    Refresh,
    Copy,
    Dot,
}

impl Icon {
    fn path(self) -> &'static str {
        match self {
            Icon::Close => "icons/close.svg",
            Icon::Minimize => "icons/minimize.svg",
            Icon::Maximize => "icons/maximize.svg",
            Icon::Restore => "icons/restore.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::PanelLeft => "icons/panel-left.svg",
            Icon::GitBranch => "icons/git-branch.svg",
            Icon::Message => "icons/message.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::Terminal => "icons/terminal.svg",
            Icon::Code => "icons/code.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::Paperclip => "icons/paperclip.svg",
            Icon::ArrowUp => "icons/arrow-up.svg",
            Icon::Stop => "icons/stop.svg",
            Icon::Loader => "icons/loader.svg",
            Icon::Check => "icons/check.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Copy => "icons/copy.svg",
            Icon::Dot => "icons/dot.svg",
        }
    }
}

/// Line icons drawn on a 24px grid, Lucide style.
const ICONS: [(&str, &str); 23] = [
    ("close", r#"<path d="M18 6 6 18M6 6l12 12"/>"#),
    ("minimize", r#"<path d="M5 12h14"/>"#),
    ("maximize", r#"<rect x="5" y="5" width="14" height="14" rx="1.5"/>"#),
    (
        "restore",
        r#"<rect x="4" y="8" width="12" height="12" rx="1.5"/><path d="M8 8V5.5A1.5 1.5 0 0 1 9.5 4h9A1.5 1.5 0 0 1 20 5.5v9a1.5 1.5 0 0 1-1.5 1.5H16"/>"#,
    ),
    ("plus", r#"<path d="M12 5v14M5 12h14"/>"#),
    ("settings", r#"<path d="M20 7h-9M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>"#),
    ("panel-left", r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>"#),
    (
        "git-branch",
        r#"<path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#,
    ),
    ("message", r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#),
    (
        "folder",
        r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
    ),
    ("terminal", r#"<path d="m4 17 6-6-6-6M12 19h8"/>"#),
    ("code", r#"<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"/>"#),
    ("trash", r#"<path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#),
    ("chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#),
    ("chevron-right", r#"<path d="m9 18 6-6-6-6"/>"#),
    (
        "paperclip",
        r#"<path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>"#,
    ),
    ("arrow-up", r#"<path d="m5 12 7-7 7 7M12 19V5"/>"#),
    ("stop", r#"<rect x="6" y="6" width="12" height="12" rx="1.5" fill="black"/>"#),
    ("loader", r#"<path d="M21 12a9 9 0 1 1-6.22-8.56"/>"#),
    ("check", r#"<path d="M20 6 9 17l-5-5"/>"#),
    ("refresh", r#"<path d="M3 12a9 9 0 0 1 15.74-6.26L21 8M21 3v5h-5M21 12a9 9 0 0 1-15.74 6.26L3 16M8 16H3v5"/>"#),
    (
        "copy",
        r#"<rect x="8" y="8" width="13" height="13" rx="2"/><path d="M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2"/>"#,
    ),
    ("dot", r#"<circle cx="12" cy="12" r="5" fill="black" stroke="none"/>"#),
];

/// Serves the icons to gpui's `svg()` element.
pub struct Assets;

fn icon_files() -> &'static HashMap<String, Vec<u8>> {
    static FILES: OnceLock<HashMap<String, Vec<u8>>> = OnceLock::new();
    FILES.get_or_init(|| {
        ICONS
            .iter()
            .map(|(name, body)| {
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
                );
                (format!("icons/{name}.svg"), svg.into_bytes())
            })
            .collect()
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_files().get(path).map(|b| Cow::Borrowed(b.as_slice())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(icon_files().keys().filter(|k| k.starts_with(path)).map(|k| k.clone().into()).collect())
    }
}

pub fn icon(icon: Icon, color: Hsla) -> gpui::Svg {
    svg().path(icon.path()).size_4().flex_none().text_color(color)
}

/// A spinning loader icon.
pub fn spinner(id: impl Into<ElementId>, color: Hsla, size: Pixels) -> gpui::AnimationElement<gpui::Svg> {
    icon(Icon::Loader, color).size(size).with_animation(
        id,
        Animation::new(Duration::from_millis(900)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

/// A borderless button: text and/or icon, a hover wash, rounded.
pub fn ghost_button(id: impl Into<ElementId>, cx: &App) -> Stateful<gpui::Div> {
    let t = theme(cx);
    let hover = t.hover;
    let active = t.active;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .h(px(24.))
        .px_2()
        .rounded_md()
        .text_sm()
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(active))
}

pub fn icon_button(id: impl Into<ElementId>, which: Icon, cx: &App) -> Stateful<gpui::Div> {
    let t = theme(cx);
    let hover = t.hover;
    let active = t.active;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(active))
        .child(icon(which, t.text_muted))
}

/// A filled button for the one thing a view is asking you to do.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>, cx: &App) -> Stateful<gpui::Div> {
    let t = theme(cx);
    let accent = t.accent;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px_2p5()
        .rounded_md()
        .text_sm()
        .bg(accent)
        .text_color(t.editor)
        .cursor_pointer()
        .hover(move |s| s.opacity(0.9))
        .child(label.into())
}

/// A bordered button, for secondary actions next to a primary one.
pub fn outline_button(id: impl Into<ElementId>, label: impl Into<SharedString>, cx: &App) -> Stateful<gpui::Div> {
    let t = theme(cx);
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px_2p5()
        .rounded_md()
        .border_1()
        .border_color(t.border)
        .text_sm()
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

/// Hover text, Zed style: a small elevated card.
pub struct Tooltip {
    text: SharedString,
    meta: Option<SharedString>,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone(), meta: None }).into()
    }

    pub fn with_meta(
        text: impl Into<SharedString>,
        meta: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let (text, meta) = (text.into(), meta.into());
        move |_, cx| cx.new(|_| Tooltip { text: text.clone(), meta: Some(meta.clone()) }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div().pl_2().pt_2().child(
            div()
                .flex()
                .flex_col()
                .max_w(px(420.))
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .bg(t.elevated)
                .shadow_md()
                .text_xs()
                .text_color(t.text)
                .child(self.text.clone())
                .children(self.meta.clone().map(|m| div().text_color(t.text_muted).child(m))),
        )
    }
}

/// A key binding with `secondary` as Cmd on macOS and Ctrl elsewhere.
/// Bindings are registered once at startup, so leaking the string is fine.
pub fn keys(binding: &str) -> &'static str {
    binding.replace("secondary", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" }).leak()
}

/// A horizontal hairline.
pub fn divider(cx: &App) -> impl IntoElement + use<> {
    div().h(px(1.)).w_full().flex_none().bg(theme(cx).border_variant)
}

/// A rounded, bordered panel for menus, popovers and modals.
pub fn popover(cx: &App) -> gpui::Div {
    let t = theme(cx);
    div().flex().flex_col().p_1().rounded_lg().border_1().border_color(t.border).bg(t.elevated).shadow_lg()
}

/// One row in a menu or picker.
pub fn menu_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> Stateful<gpui::Div> {
    let t = theme(cx);
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .h(px(26.))
        .px_2()
        .rounded_md()
        .text_sm()
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when(selected, |d| d.bg(t.active))
        .child(div().flex_1().truncate().child(label.into()))
        .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
}
