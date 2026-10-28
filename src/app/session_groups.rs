//! Organising the sessions panel: groups that collapse, an archive, their
//! right-click menus, renaming a group in place, and dragging sessions between
//! them. The rules live in `kynex_core::groups`; this draws them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, MouseButton, MouseDownEvent, SharedString, Window, div,
    prelude::*, px,
};
use kynex_core::groups::SessionGroups;

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::theme;
use crate::ui::input::TextInput;
use crate::ui::{Icon, divider, icon, menu_item, popover};

/// What a session menu item does when clicked.
type MenuAction = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

/// Each folder's groups as loaded, and the panel's own view state.
pub(super) struct Organize {
    by_folder: HashMap<PathBuf, SessionGroups>,
    /// The group whose name is being edited, in which folder.
    editing: Option<(PathBuf, usize)>,
    name: Entity<TextInput>,
    /// The archived section is open (this run only; it starts closed).
    pub(super) show_archived: bool,
}

impl Organize {
    pub(super) fn new(name: Entity<TextInput>) -> Self {
        Self { by_folder: HashMap::new(), editing: None, name, show_archived: false }
    }
}

/// A session being dragged onto a group.
#[derive(Clone)]
pub(super) struct DraggedSession {
    pub id: String,
    pub title: SharedString,
}

impl Render for DraggedSession {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        popover(cx)
            .px_2()
            .py_1()
            .max_w(px(240.))
            .text_sm()
            .text_color(t.text)
            .child(div().truncate().child(self.title.clone()))
    }
}

impl KynexApp {
    /// A folder's groups, read from disk the first time.
    pub(super) fn groups_for(&mut self, cwd: &Path) -> &SessionGroups {
        self.organize.by_folder.entry(cwd.to_path_buf()).or_insert_with(|| SessionGroups::load(cwd))
    }

    /// Changes a folder's groups and saves them if anything changed.
    pub(super) fn edit_groups(&mut self, cwd: &Path, f: impl FnOnce(&mut SessionGroups)) {
        self.groups_for(cwd);
        let Some(groups) = self.organize.by_folder.get_mut(cwd) else { return };
        let before = groups.clone();
        f(groups);
        if *groups != before
            && let Err(e) = groups.save(cwd)
        {
            self.toast(Toast::error(e));
        }
    }

    /// Shows the name field in place of group `i`'s header.
    fn start_rename(&mut self, cwd: PathBuf, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.groups_for(&cwd).groups.get(i).map(|g| g.name.clone()).unwrap_or_default();
        self.organize.name.update(cx, |input, cx| input.set_text(name, cx));
        self.organize.name.read(cx).focus_handle(cx).focus(window, cx);
        self.organize.editing = Some((cwd, i));
        cx.notify();
    }

    /// The Sessions header's new-group button: an empty group, its name open for editing.
    pub(super) fn new_group(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut at = 0;
        self.edit_groups(&cwd, |g| at = g.add_group(""));
        self.start_rename(cwd, at, window, cx);
    }

    /// Enter in the name field: keeps the new name.
    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((cwd, i)) = self.organize.editing.take() else { return };
        let name = self.organize.name.read(cx).text().to_string();
        self.edit_groups(&cwd, |g| g.rename_group(i, &name));
        cx.notify();
    }

    /// Esc in the name field: leaves the name as it was. Returns whether it was open.
    pub(super) fn cancel_rename(&mut self) -> bool {
        self.organize.editing.take().is_some()
    }

    /// The menu for a session row: move it between groups, archive it.
    pub(super) fn session_menu_items(&mut self, id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(cwd) = self.active_session().map(|s| s.cwd.clone()) else { return Vec::new() };
        let groups = self.groups_for(&cwd).clone();
        let current = groups.group_of(id);
        let mut items = Vec::new();
        let item = |label: String, n: usize, cx: &mut Context<Self>, f: MenuAction| {
            menu_item(("session-menu", n), label, false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    f(this, window, cx);
                    cx.notify();
                }))
                .into_any_element()
        };
        for (i, g) in groups.groups.iter().enumerate().filter(|(i, _)| Some(*i) != current) {
            let (id, cwd) = (id.to_string(), cwd.clone());
            items.push(item(
                format!("Move to {}", g.name),
                i,
                cx,
                Box::new(move |this, _, _| this.edit_groups(&cwd, |g| g.move_to(&id, Some(i)))),
            ));
        }
        let (new_id, new_cwd) = (id.to_string(), cwd.clone());
        items.push(item(
            "New group…".into(),
            1000,
            cx,
            Box::new(move |this, window, cx| {
                let mut at = 0;
                this.edit_groups(&new_cwd, |g| {
                    at = g.add_group("");
                    g.move_to(&new_id, Some(at));
                });
                this.start_rename(new_cwd.clone(), at, window, cx);
            }),
        ));
        if let Some(i) = current {
            let (id, cwd) = (id.to_string(), cwd.clone());
            items.push(item(
                format!("Remove from {}", groups.groups[i].name),
                1001,
                cx,
                Box::new(move |this, _, _| this.edit_groups(&cwd, |g| g.move_to(&id, None))),
            ));
        }
        items.push(div().my_1().child(divider(cx)).into_any_element());
        let archived = groups.is_archived(id);
        let id = id.to_string();
        items.push(item(
            if archived { "Unarchive".into() } else { "Archive".into() },
            1002,
            cx,
            Box::new(move |this, _, _| {
                this.edit_groups(&cwd, |g| {
                    if archived {
                        g.unarchive(&id);
                    } else {
                        g.archive(&id);
                    }
                })
            }),
        ));
        items
    }

    /// The menu for a group header: rename or delete it.
    pub(super) fn group_menu_items(&mut self, i: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(cwd) = self.active_session().map(|s| s.cwd.clone()) else { return Vec::new() };
        let t = theme(cx).clone();
        let rename_cwd = cwd.clone();
        vec![
            menu_item("group-rename", "Rename", false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.start_rename(rename_cwd.clone(), i, window, cx);
                }))
                .into_any_element(),
            menu_item("group-delete", "Delete group", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.edit_groups(&cwd, |g| g.delete_group(i));
                    cx.notify();
                }))
                .into_any_element(),
        ]
    }

    /// A group: its header (click to collapse, right-click for its menu, or the
    /// name field while renaming) and its rows. Sessions dropped on it join it.
    pub(super) fn render_group(
        &mut self,
        cwd: &Path,
        i: usize,
        rows: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let Some(group) = self.groups_for(cwd).groups.get(i).cloned() else { return div().into_any_element() };
        let editing = self.organize.editing.as_ref().is_some_and(|(p, e)| p == cwd && *e == i);
        let (count, hover) = (rows.len(), t.hover);
        let (toggle_cwd, drop_cwd) = (cwd.to_path_buf(), cwd.to_path_buf());
        let header = div()
            .id(("group", i))
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.))
            .px_1p5()
            .rounded_md()
            .text_xs()
            .text_color(t.text_muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .child(
                icon(if group.collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, t.text_placeholder).size_3(),
            )
            .child(if editing {
                div()
                    .flex_1()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(t.border_focused)
                    .bg(t.editor)
                    .text_sm()
                    .child(self.organize.name.clone())
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(group.name)
                    .into_any_element()
            })
            .child(div().flex_none().text_color(t.text_placeholder).child(count.to_string()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !editing {
                    this.edit_groups(&toggle_cwd, |g| g.toggle_group(i));
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.open_menu(Menu::Group(i), ev.position, gpui::Anchor::TopLeft, cx)
                }),
            );
        div()
            .id(("group-block", i))
            .flex()
            .flex_col()
            .rounded_md()
            .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                cx.stop_propagation();
                this.edit_groups(&drop_cwd, |g| g.move_to(&d.id, Some(i)));
                cx.notify();
            }))
            .child(header)
            .when(!group.collapsed, |d| d.child(div().pl_3().children(rows)))
            .into_any_element()
    }

    /// The archive: a header that opens it, and its rows while open.
    pub(super) fn render_archived(&mut self, cwd: &Path, rows: Vec<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let open = self.organize.show_archived;
        let (count, hover) = (rows.len(), t.hover);
        let drop_cwd = cwd.to_path_buf();
        div()
            .id("archived-block")
            .flex()
            .flex_col()
            .mt_1()
            .rounded_md()
            .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                cx.stop_propagation();
                this.edit_groups(&drop_cwd, |g| g.archive(&d.id));
                cx.notify();
            }))
            .child(
                div()
                    .id("archived")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1p5()
                    .rounded_md()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size_3())
                    .child(div().flex_1().child("Archived"))
                    .child(count.to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.organize.show_archived = !this.organize.show_archived;
                        cx.notify();
                    })),
            )
            .when(open, |d| d.child(div().pl_3().children(rows)))
            .into_any_element()
    }
}
