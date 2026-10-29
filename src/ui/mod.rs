//! Kynex's handful of components, in the spirit of Zed's `ui` crate: small
//! `RenderOnce`-style builders over `div`, styled from the theme global.

pub mod code;
pub mod completions;
pub mod ease_height;
pub mod image_viewer;
pub mod input;
pub mod mark;
pub mod motion;
pub mod scrollbar;
pub mod selectable;
pub mod smooth_scroll;
pub mod wash;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyView, App, AssetSource, Context, ElementId, FontWeight, Hsla, IntoElement, Pixels,
    SharedString, Stateful, Transformation, Window, div, percentage, prelude::*, px, svg,
};

use kynex_core::fmt::Mods;

use crate::theme::{label_font, theme, ui_font};
use wash::wash;

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
    FolderPlus,
    Terminal,
    Code,
    Trash,
    ChevronDown,
    ChevronRight,
    CollapseAll,
    ExpandAll,
    Paperclip,
    ArrowUp,
    Stop,
    Loader,
    Check,
    Refresh,
    Copy,
    Dot,
    Minus,
    Undo,
    ArrowDown,
    GitCommit,
    Play,
    Shield,
    Timeline,
    Search,
    /// The pointer on the mark's knob, drawn filled.
    MarkPointer,
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
            Icon::FolderPlus => "icons/folder-plus.svg",
            Icon::Terminal => "icons/terminal.svg",
            Icon::Code => "icons/code.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::CollapseAll => "icons/chevrons-down-up.svg",
            Icon::ExpandAll => "icons/chevrons-up-down.svg",
            Icon::Paperclip => "icons/paperclip.svg",
            Icon::ArrowUp => "icons/arrow-up.svg",
            Icon::Stop => "icons/stop.svg",
            Icon::Loader => "icons/loader.svg",
            Icon::Check => "icons/check.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Copy => "icons/copy.svg",
            Icon::Dot => "icons/dot.svg",
            Icon::Minus => "icons/minus.svg",
            Icon::Undo => "icons/undo.svg",
            Icon::ArrowDown => "icons/arrow-down.svg",
            Icon::GitCommit => "icons/git-commit.svg",
            Icon::Play => "icons/play.svg",
            Icon::Shield => "icons/shield.svg",
            Icon::Timeline => "icons/timeline.svg",
            Icon::Search => "icons/search.svg",
            Icon::MarkPointer => "icons/mark-pointer.svg",
        }
    }
}

/// Line icons drawn on a 24px grid, Lucide style.
const ICONS: [(&str, &str); 35] = [
    // Steps on a line, for the session timeline.
    (
        "timeline",
        r#"<circle cx="6" cy="5" r="1.5"/><circle cx="6" cy="12" r="1.5"/><circle cx="6" cy="19" r="1.5"/><path d="M6 6.5v4M6 13.5v4M11 5h9M11 12h9M11 19h6"/>"#,
    ),
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
    (
        "folder-plus",
        r#"<path d="M12 10v6M9 13h6"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
    ),
    ("terminal", r#"<path d="m4 17 6-6-6-6M12 19h8"/>"#),
    ("code", r#"<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"/>"#),
    ("trash", r#"<path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#),
    ("chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#),
    ("chevron-right", r#"<path d="m9 18 6-6-6-6"/>"#),
    ("chevrons-down-up", r#"<path d="m7 20 5-5 5 5M7 4l5 5 5-5"/>"#),
    ("chevrons-up-down", r#"<path d="m7 15 5 5 5-5M7 9l5-5 5 5"/>"#),
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
    ("minus", r#"<path d="M5 12h14"/>"#),
    ("undo", r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#),
    ("arrow-down", r#"<path d="M12 5v14M19 12l-7 7-7-7"/>"#),
    ("git-commit", r#"<circle cx="12" cy="12" r="3"/><path d="M3 12h6M15 12h6"/>"#),
    (
        "play",
        r#"<path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z" fill="black"/>"#,
    ),
    ("shield", r#"<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>"#),
    ("search", r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-4.35-4.35"/>"#),
    // The knob's box: the mark's pointer (x 47 y 27, 6×21, rotated 38° about the
    // knob centre) scaled from the 62-unit knob to 24, so the svg rotates about the knob.
    (
        "mark-pointer",
        r#"<rect x="10.839" y="1.548" width="2.323" height="8.129" rx="1.161" fill="black" stroke="none" transform="rotate(38 12 12)"/>"#,
    ),
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

/// `logical` pixels rounded to a whole number of device pixels (at least one),
/// for things painted by hand: gpui snaps each edge of a quad on its own, so a
/// 1.5px caret would be 1 or 2 device pixels wide depending on where it sits.
pub fn device_px(logical: f32, window: &Window) -> Pixels {
    let scale = window.scale_factor();
    px((logical * scale).round().max(1.) / scale)
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
        .relative()
        .active(move |s| s.bg(active))
        .child(wash(hover, None))
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
        .relative()
        .active(move |s| s.bg(active))
        .child(wash(hover, None))
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
        .text_color(t.on_accent)
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
        .relative()
        .child(wash(hover, None))
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

    /// `text`, with the shortcut that runs `action` (as bound now, see
    /// [`shortcut`]) and then `more`, if not empty: "Ctrl+B", "⌘B · more".
    pub fn for_action(
        text: impl Into<SharedString>,
        action: impl gpui::Action,
        more: &'static str,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| {
            let meta = match shortcut(&action, cx) {
                Some(keys) if more.is_empty() => Some(keys.into()),
                Some(keys) => Some(format!("{keys} · {more}").into()),
                None => (!more.is_empty()).then(|| more.into()),
            };
            cx.new(|_| Tooltip { text: text.clone(), meta }).into()
        }
    }
}

/// The shortcut that runs `action`, as the user would press it: their
/// `keymap.json` rebinds included, "⌘B" on macOS, "Ctrl+B" elsewhere. `None`
/// if it's unbound.
pub fn shortcut(action: &dyn gpui::Action, cx: &App) -> Option<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    // The last binding added wins, as gpui dispatches it.
    let binding = keymap.bindings_for_action(action).next_back()?;
    let keys: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|k| {
            let m = k.modifiers();
            let mods = Mods { control: m.control, alt: m.alt, shift: m.shift, platform: m.platform };
            kynex_core::fmt::keystroke(mods, k.key(), cfg!(target_os = "macos"))
        })
        .collect();
    Some(keys.join(" "))
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let card = motion::fade_in(
            "tooltip-in",
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
                .font_family(ui_font())
                .text_xs()
                .text_color(t.text)
                .child(self.text.clone())
                .children(self.meta.clone().map(|m| div().text_color(t.text_muted).child(m))),
            motion::QUICK,
        );
        div().pl_2().pt_2().child(card)
    }
}

/// A key binding with `secondary` as Cmd on macOS and Ctrl elsewhere.
/// Bindings are registered once at startup, so leaking the string is fine.
pub fn keys(binding: &str) -> &'static str {
    binding.replace("secondary", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" }).leak()
}

/// A section heading, engraved-panel style: SESSIONS, STAGED.
pub fn section_label(text: &str, cx: &App) -> gpui::Div {
    div()
        .font_family(label_font())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(10.5))
        .text_color(theme(cx).text_muted)
        .child(text.to_uppercase())
}

/// What a session is doing, shown as an indicator lamp (IEC 60073 colours).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lamp {
    Idle,
    Working,
    NeedsYou,
    Done,
}

impl Lamp {
    pub fn of(busy: bool, waiting: bool, finished: bool) -> Self {
        if waiting {
            Lamp::NeedsYou
        } else if busy {
            Lamp::Working
        } else if finished {
            Lamp::Done
        } else {
            Lamp::Idle
        }
    }

    pub fn color(self, cx: &App) -> Option<Hsla> {
        let t = theme(cx);
        match self {
            Lamp::Idle => None,
            Lamp::Working => Some(t.accent),
            Lamp::NeedsYou => Some(t.warning),
            Lamp::Done => Some(t.success),
        }
    }
}

/// The pulse of a working lamp: 1.6 s, easing in and out.
pub fn pulse() -> Animation {
    Animation::new(Duration::from_millis(1600)).repeat().with_easing(gpui::pulsating_between(0.3, 1.))
}

/// A 7 px lamp: pulsing petrol while working, amber when it needs you, green when
/// done, a hollow ring when idle.
pub fn lamp(id: impl Into<ElementId>, state: Lamp, cx: &App) -> gpui::AnyElement {
    let dot = div().flex_none().size(px(7.)).rounded_full();
    match state.color(cx) {
        None => dot.border_1().border_color(theme(cx).text_placeholder).into_any_element(),
        Some(c) if state == Lamp::Working => {
            dot.bg(c).with_animation(id, pulse(), |d, a| d.opacity(a)).into_any_element()
        }
        Some(c) => dot.bg(c).into_any_element(),
    }
}

/// A horizontal hairline.
pub fn divider(cx: &App) -> impl IntoElement + use<> {
    div().h(px(1.)).w_full().flex_none().bg(theme(cx).border_variant)
}

/// A text field shown in place of a name while it's renamed (a group, a
/// session). Clicks stay in it, so the row or title under it doesn't react.
pub fn name_field(input: gpui::Entity<input::TextInput>, t: &crate::theme::Palette) -> gpui::Div {
    div()
        .min_w_0()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(t.border_focused)
        .bg(t.editor)
        .text_sm()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(input)
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
        .relative()
        .child(wash(hover, selected.then_some(t.active)))
        .child(div().flex_1().truncate().child(label.into()))
        .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
}
