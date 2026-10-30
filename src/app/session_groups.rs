//! Organising the sessions panel: groups that collapse, an archive, pinned
//! sessions, their right-click menus, renaming a group or session in place,
//! and dragging sessions between them. The rules live in `kynex_core::groups`;
//! this draws them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, MouseButton, MouseDownEvent, SharedString, Window, div,
    prelude::*,
};
use kynex_core::groups::{Selection, SessionGroups};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{theme, ui_font};
use crate::ui::input::TextInput;
use crate::ui::{divider, menu_item, popover, section_label};

/// What a session menu item does when clicked.
type MenuAction = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

/// What the name field is renaming.
#[derive(Clone, PartialEq)]
enum Editing {
    /// Group `i` in this folder.
    Group(PathBuf, usize),
    /// A session, in its panel row or (`in_header`) the session header.
    Session { cwd: PathBuf, id: String, in_header: bool },
}

/// Each folder's groups as loaded, and the panel's own view state.
pub(super) struct Organize {
    by_folder: HashMap<PathBuf, SessionGroups>,
    /// What's being renamed, if anything.
    editing: Option<Editing>,
    name: Entity<TextInput>,
    /// The archived section is open (this run only; it starts closed).
    pub(super) show_archived: bool,
    /// Sessions picked to archive or delete together, and the folder they're in.
    pub(super) selection: Selection,
    pub(super) selection_cwd: Option<PathBuf>,
    /// The past sessions' rows in the order drawn, for Shift+click runs.
    pub(super) shown: Vec<String>,
    /// "Delete" was clicked on the picked sessions and waits for Yes.
    pub(super) confirm_bulk_delete: bool,
    /// The row the list's ↑ / ↓ keys are on.
    pub(super) cursor: Option<String>,
}

impl Organize {
    pub(super) fn new(name: Entity<TextInput>) -> Self {
        Self {
            by_folder: HashMap::new(),
            editing: None,
            name,
            show_archived: false,
            selection: Selection::default(),
            selection_cwd: None,
            shown: Vec::new(),
            confirm_bulk_delete: false,
            cursor: None,
        }
    }

    /// A session's name given by hand, if its folder's groups are loaded.
    pub(super) fn name_of(&self, cwd: &Path, id: &str) -> Option<String> {
        self.by_folder.get(cwd)?.name_of(id).map(str::to_string)
    }
}

/// Sessions being dragged onto a group, the archive or a pinned row: the one
/// grabbed, and with it every other picked one.
#[derive(Clone)]
pub(super) struct DraggedSession {
    pub ids: Vec<String>,
    pub title: SharedString,
}

/// A group header being dragged to another place among the groups.
#[derive(Clone)]
pub(super) struct DraggedGroup {
    pub index: usize,
    pub name: SharedString,
}

impl Render for DraggedGroup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        popover(cx).font_family(ui_font()).px_2().py_1().text_sm().text_color(t.text_muted).child(self.name.clone())
    }
}

impl Render for DraggedSession {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Drawn outside the window's root, so it doesn't inherit the font.
        popover(cx).font_family(ui_font()).px_2().py_1().max_w(px(240.)).text_sm().text_color(t.text).child(
            div().truncate().child(match self.ids.len() {
                1 => self.title.clone(),
                n => format!("{n} sessions").into(),
            }),
        )
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
    pub(super) fn start_group_rename(&mut self, cwd: PathBuf, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        // One open at a time: the field moves, so keep what the last one had.
        self.commit_rename(cx);
        let name = self.groups_for(&cwd).groups.get(i).map(|g| g.name.clone()).unwrap_or_default();
        self.organize.name.update(cx, |input, cx| input.set_text(name, cx));
        self.organize.name.read(cx).focus_handle(cx).focus(window, cx);
        self.organize.editing = Some(Editing::Group(cwd, i));
        cx.notify();
    }

    /// Shows the name field in place of a session's name: in its panel row, or
    /// in the session header (`in_header`), filled with the name it has now.
    pub(super) fn start_session_rename(
        &mut self,
        cwd: PathBuf,
        id: String,
        in_header: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let listed = self.sessions.as_ref().and_then(|l| l.items.iter().find(|i| i.id == id)).map(|i| i.title.clone());
        let name = self.groups_for(&cwd).name_of(&id).map(str::to_string).or(listed).unwrap_or_default();
        self.organize.name.update(cx, |input, cx| input.set_text(name, cx));
        self.organize.name.read(cx).focus_handle(cx).focus(window, cx);
        self.organize.editing = Some(Editing::Session { cwd, id, in_header });
        cx.notify();
    }

    /// The name field, while it's renaming session `id` in the panel row or (`in_header`) the header.
    pub(super) fn session_name_field(&self, id: &str, in_header: bool, cx: &mut Context<Self>) -> Option<gpui::Div> {
        matches!(&self.organize.editing, Some(Editing::Session { id: e, in_header: h, .. }) if e == id && *h == in_header)
            .then(|| self.rename_field(cx))
    }

    /// The name field in place: Enter or a click anywhere else keeps the name,
    /// Esc leaves it as it was.
    fn rename_field(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        crate::ui::name_field(self.organize.name.clone(), &t)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_rename(cx)))
    }

    /// A session's name given by hand, which wins over generated ones.
    pub(super) fn given_name(&mut self, cwd: &Path, id: &str) -> Option<String> {
        self.groups_for(cwd).name_of(id).map(str::to_string)
    }

    /// The Sessions header's new-group button: an empty group, its name open for editing.
    pub(super) fn new_group(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut at = 0;
        self.edit_groups(&cwd, |g| at = g.add_group(""));
        self.start_group_rename(cwd, at, window, cx);
    }

    /// Enter in the name field: keeps the new name. A session's blank name
    /// goes back to the generated one.
    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.organize.editing.take() else { return };
        let name = self.organize.name.read(cx).text().to_string();
        match editing {
            Editing::Group(cwd, i) => self.edit_groups(&cwd, |g| g.rename_group(i, &name)),
            Editing::Session { cwd, id, .. } => {
                self.edit_groups(&cwd, |g| g.rename(&id, &name));
                // Re-read the list so the new name shows everywhere.
                self.reread_sessions();
            }
        }
        cx.notify();
    }

    /// Esc in the name field: leaves the name as it was. Returns whether it was open.
    pub(super) fn cancel_rename(&mut self) -> bool {
        self.organize.editing.take().is_some()
    }

    /// The menu for a session row: rename or pin it, move it between groups, archive it.
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
        let (rename_id, rename_cwd) = (id.to_string(), cwd.clone());
        items.push(item(
            "Rename".into(),
            2000,
            cx,
            Box::new(move |this, window, cx| {
                this.start_session_rename(rename_cwd.clone(), rename_id.clone(), false, window, cx)
            }),
        ));
        let (pin_id, pin_cwd) = (id.to_string(), cwd.clone());
        items.push(item(
            if groups.is_pinned(id) { "Unpin".into() } else { "Pin to top".into() },
            2001,
            cx,
            Box::new(move |this, _, _| this.edit_groups(&pin_cwd, |g| g.toggle_pin(&pin_id))),
        ));
        items.push(div().my_1().child(divider(cx)).into_any_element());
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
                this.start_group_rename(new_cwd.clone(), at, window, cx);
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
        let (archive_id, archive_cwd) = (id.to_string(), cwd.clone());
        items.push(item(
            if archived { "Unarchive".into() } else { "Archive".into() },
            1002,
            cx,
            Box::new(move |this, _, _| {
                this.edit_groups(&archive_cwd, |g| {
                    if archived {
                        g.unarchive(&archive_id);
                    } else {
                        g.archive(&archive_id);
                    }
                })
            }),
        ));
        // Its disk use, which opens Settings → Storage on it.
        let size = self.disk_use(cwd, id, cx).map_or_else(|| "measuring…".into(), kynex_core::fmt::bytes);
        let id = id.to_string();
        items.push(item(
            format!("On disk: {size}…"),
            1003,
            cx,
            Box::new(move |this, _, cx| this.show_storage_of(id.clone(), cx)),
        ));
        items
    }

    /// The menu for a group header: rename or delete it.
    pub(super) fn group_menu_items(&mut self, i: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(cwd) = self.active_session().map(|s| s.cwd.clone()) else { return Vec::new() };
        let t = theme(cx).clone();
        let (rename_cwd, archive_cwd) = (cwd.clone(), cwd.clone());
        vec![
            menu_item("group-rename", "Rename", false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.start_group_rename(rename_cwd.clone(), i, window, cx);
                }))
                .into_any_element(),
            div().my_1().child(divider(cx)).into_any_element(),
            menu_item("group-delete", "Delete group, keep its sessions", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.edit_groups(&cwd, |g| g.delete_group(i));
                    cx.notify();
                }))
                .into_any_element(),
            menu_item("group-delete-archive", "Delete group and archive its sessions", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.edit_groups(&archive_cwd, |g| g.delete_group_archiving(i));
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
        let editing = matches!(&self.organize.editing, Some(Editing::Group(p, e)) if p == cwd && *e == i);
        let (count, hover) = (rows.len(), t.hover);
        let (toggle_cwd, drop_cwd) = (cwd.to_path_buf(), cwd.to_path_buf());
        // A section header like the date ones: its name, and its count while folded.
        let label = if group.collapsed { format!("{} · {count}", group.name) } else { group.name.clone() };
        let text = t.text;
        let header = match editing {
            true => div().pt_3().pb_1().px_1().child(self.rename_field(cx)),
            false => section_label(&label, cx).pt_3().pb_1().px_2().truncate(),
        }
        .id(("group", i))
        .cursor_pointer()
        .hover(move |s| s.text_color(text))
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
        )
        // Dragged onto another group, it takes that group's place.
        .on_drag(DraggedGroup { index: i, name: group.name.clone().into() }, |d, _, _, cx| cx.new(|_| d.clone()));
        let reorder_cwd = cwd.to_path_buf();
        div()
            .id(("group-block", i))
            .flex()
            .flex_col()
            .rounded_md()
            .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                cx.stop_propagation();
                this.edit_groups(&drop_cwd, |g| d.ids.iter().for_each(|id| g.move_to(id, Some(i))));
                this.organize.selection.clear();
                cx.notify();
            }))
            .drag_over::<DraggedGroup>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedGroup, _, cx| {
                cx.stop_propagation();
                this.edit_groups(&reorder_cwd, |g| g.move_group(d.index, i));
                cx.notify();
            }))
            .child(header)
            .when(!group.collapsed, |d| d.children(rows))
            .into_any_element()
    }

    /// The archive: a header that opens it, and its rows while open.
    pub(super) fn render_archived(&mut self, cwd: &Path, rows: Vec<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let open = self.organize.show_archived;
        let (count, hover, text) = (rows.len(), t.hover, t.text);
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
                this.edit_groups(&drop_cwd, |g| d.ids.iter().for_each(|id| g.archive(id)));
                this.organize.selection.clear();
                cx.notify();
            }))
            .child(
                section_label(&if open { "Archived".to_string() } else { format!("Archived · {count}") }, cx)
                    .id("archived")
                    .pt_3()
                    .pb_1()
                    .px_2()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(text))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.organize.show_archived = !this.organize.show_archived;
                        cx.notify();
                    })),
            )
            .when(open, |d| d.child(div().pl_3().children(rows)))
            .into_any_element()
    }
}
