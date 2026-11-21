//! Organising the sessions tree: groups, archive, pinning, renaming, and drag-and-drop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::ui::px;
use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, MouseButton, MouseDownEvent, SharedString, Window, div,
    prelude::*,
};
use kynex_core::groups::{GroupId, Selection, SessionGroups};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{theme, ui_font};
use crate::ui::input::TextInput;
use crate::ui::{Lamp, divider, lamp_count, menu_item, popover, section_label};

type MenuAction = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

#[derive(Clone, PartialEq)]
enum Editing {
    Group(PathBuf, GroupId),
    Session { cwd: PathBuf, id: String },
}

pub(super) struct Organize {
    by_folder: HashMap<PathBuf, Rc<SessionGroups>>,
    /// Bumped whenever any folder's `SessionGroups` changes, to invalidate cached tree sections.
    pub(super) generation: u64,
    editing: Option<Editing>,
    name: Entity<TextInput>,
    pub(super) selection: Selection,
    pub(super) selection_cwd: Option<PathBuf>,
    pub(super) shown: Vec<(PathBuf, String)>,
    pub(super) confirm_bulk_delete: bool,
    pub(super) cursor: Option<String>,
}

impl Organize {
    pub(super) fn new(name: Entity<TextInput>) -> Self {
        Self {
            by_folder: HashMap::new(),
            generation: 0,
            editing: None,
            name,
            selection: Selection::default(),
            selection_cwd: None,
            shown: Vec::new(),
            confirm_bulk_delete: false,
            cursor: None,
        }
    }

    pub(super) fn name_of(&self, cwd: &Path, id: &str) -> Option<String> {
        self.by_folder.get(cwd)?.name_of(id).map(str::to_string)
    }
}

#[derive(Clone)]
pub(super) struct DraggedSession {
    pub cwd: PathBuf,
    pub ids: Vec<String>,
    pub title: SharedString,
}

#[derive(Clone)]
pub(super) struct DraggedGroup {
    pub cwd: PathBuf,
    pub id: GroupId,
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
        popover(cx).font_family(ui_font()).px_2().py_1().max_w(px(240.)).text_sm().text_color(t.text).child(
            div().truncate().child(match self.ids.len() {
                1 => self.title.clone(),
                n => format!("{n} sessions").into(),
            }),
        )
    }
}

impl KynexApp {
    pub(super) fn groups_for(&mut self, cwd: &Path) -> &SessionGroups {
        self.organize.by_folder.entry(cwd.to_path_buf()).or_insert_with(|| Rc::new(SessionGroups::load(cwd)))
    }

    pub(super) fn groups_rc(&mut self, cwd: &Path) -> Rc<SessionGroups> {
        self.organize.by_folder.entry(cwd.to_path_buf()).or_insert_with(|| Rc::new(SessionGroups::load(cwd))).clone()
    }

    pub(super) fn edit_groups(&mut self, cwd: &Path, f: impl FnOnce(&mut SessionGroups)) {
        self.groups_for(cwd);
        let Some(entry) = self.organize.by_folder.get_mut(cwd) else { return };
        let before = (**entry).clone();
        let groups = Rc::make_mut(entry);
        f(groups);
        if *groups != before {
            self.organize.generation = self.organize.generation.wrapping_add(1);
            if let Err(e) = groups.save(cwd) {
                self.toast(Toast::error(e));
            }
        }
    }

    pub(super) fn start_group_rename(
        &mut self,
        cwd: PathBuf,
        id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let name = self.groups_for(&cwd).group(id).map(|g| g.name.clone()).unwrap_or_default();
        self.organize.name.update(cx, |input, cx| input.set_text(name, cx));
        self.organize.name.read(cx).focus_handle(cx).focus(window, cx);
        self.organize.editing = Some(Editing::Group(cwd, id));
        cx.notify();
    }

    pub(super) fn start_session_rename(
        &mut self,
        cwd: PathBuf,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let listed = self.listed_title(&cwd, &id);
        let name = self.groups_for(&cwd).name_of(&id).map(str::to_string).or(listed).unwrap_or_default();
        self.organize.name.update(cx, |input, cx| input.set_text(name, cx));
        self.organize.name.read(cx).focus_handle(cx).focus(window, cx);
        self.organize.editing = Some(Editing::Session { cwd, id });
        cx.notify();
    }

    pub(super) fn session_name_field(&self, id: &str, cx: &mut Context<Self>) -> Option<gpui::Div> {
        matches!(&self.organize.editing, Some(Editing::Session { id: e, .. }) if e == id).then(|| self.rename_field(cx))
    }

    fn rename_field(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        crate::ui::name_field(self.organize.name.clone(), &t)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_rename(cx)))
    }

    pub(super) fn given_name(&mut self, cwd: &Path, id: &str) -> Option<String> {
        self.groups_for(cwd).name_of(id).map(str::to_string)
    }

    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.organize.editing.take() else { return };
        let name = self.organize.name.read(cx).text().to_string();
        match editing {
            Editing::Group(cwd, id) => self.edit_groups(&cwd, |g| g.rename_group(id, &name)),
            Editing::Session { cwd, id } => {
                self.edit_groups(&cwd, |g| g.rename(&id, &name));
                self.reread_sessions();
            }
        }
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self) -> bool {
        self.organize.editing.take().is_some()
    }

    pub(super) fn session_menu_items(&mut self, id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(cwd) = self.folder_of_listed(id).or_else(|| self.workspace_cwd()) else {
            return Vec::new();
        };
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
        for (n, label, side) in [
            (2100, "Open to the side", kynex_core::layout::Direction::Right),
            (2101, "Open below", kynex_core::layout::Direction::Down),
        ] {
            let open_id = id.to_string();
            items.push(item(
                label.into(),
                n,
                cx,
                Box::new(move |this, window, cx| this.open_beside(open_id.clone(), None, Some(side), window, cx)),
            ));
        }
        if let Some(agent_id) = self.live_agent_of_session(&cwd, id) {
            items.push(item(
                "Stop live session".into(),
                2102,
                cx,
                Box::new(move |this, window, cx| this.stop_agent(agent_id, window, cx)),
            ));
        }
        items.push(div().my_1().child(divider(cx)).into_any_element());
        let (rename_id, rename_cwd) = (id.to_string(), cwd.clone());
        items.push(item(
            "Rename".into(),
            2000,
            cx,
            Box::new(move |this, window, cx| {
                this.start_session_rename(rename_cwd.clone(), rename_id.clone(), window, cx)
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
        for (i, g) in groups.groups.iter().enumerate().filter(|(_, g)| Some(g.id) != current) {
            let gid = g.id;
            let (id, cwd) = (id.to_string(), cwd.clone());
            items.push(item(
                format!("Move to {}", g.name),
                i,
                cx,
                Box::new(move |this, _, _| {
                    this.edit_groups(&cwd, |g| {
                        g.move_to(&id, Some(gid));
                    })
                }),
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
        if let Some(group) = current.and_then(|c| groups.group(c)) {
            let (id, cwd) = (id.to_string(), cwd.clone());
            items.push(item(
                format!("Remove from {}", group.name),
                1001,
                cx,
                Box::new(move |this, _, _| {
                    this.edit_groups(&cwd, |g| {
                        g.move_to(&id, None);
                    })
                }),
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
                });
            }),
        ));
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

    pub(super) fn group_menu_items(&mut self, cwd: &Path, id: GroupId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let cwd = cwd.to_path_buf();
        let t = theme(cx).clone();
        let (rename_cwd, archive_cwd) = (cwd.clone(), cwd.clone());
        vec![
            menu_item("group-rename", "Rename", false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.start_group_rename(rename_cwd.clone(), id, window, cx);
                }))
                .into_any_element(),
            div().my_1().child(divider(cx)).into_any_element(),
            menu_item("group-delete", "Delete group, keep its sessions", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.edit_groups(&cwd, |g| g.delete_group(id));
                    cx.notify();
                }))
                .into_any_element(),
            menu_item("group-delete-archive", "Delete group and archive its sessions", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.edit_groups(&archive_cwd, |g| g.delete_group_archiving(id));
                    cx.notify();
                }))
                .into_any_element(),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_group(
        &mut self,
        cwd: &Path,
        i: usize,
        count: usize,
        rows: Vec<AnyElement>,
        news: &[Lamp],
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let Some(group) = self.groups_for(cwd).groups.get(i).cloned() else { return div().into_any_element() };
        let id = group.id;
        let editing = matches!(&self.organize.editing, Some(Editing::Group(p, e)) if p == cwd && *e == id);
        let hover = t.hover;
        let (toggle_cwd, drop_cwd, menu_cwd) = (cwd.to_path_buf(), cwd.to_path_buf(), cwd.to_path_buf());
        let (reorder_cwd, group_cwd) = (cwd.to_path_buf(), cwd.to_path_buf());
        let label = if open { group.name.clone() } else { format!("{} · {count}", group.name) };
        let tell = (!open && !news.is_empty())
            .then(|| lamp_count(("group-news", i), Lamp::most_urgent(news.iter().copied()), news.len(), cx));
        let text = t.text;
        let header = match editing {
            true => div().pt_3().pb_1().px_1().child(self.rename_field(cx)),
            false => div()
                .flex()
                .items_center()
                .gap_1p5()
                .pt_3()
                .pb_1()
                .px_2()
                .group("group-header")
                .child(
                    section_label(&label, cx)
                        .min_w_0()
                        .truncate()
                        .group_hover("group-header", move |s| s.text_color(text)),
                )
                .children(tell),
        }
        .id(("group", i))
        .cursor_pointer()
        .hover(move |s| s.text_color(text))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if !editing {
                this.edit_groups(&toggle_cwd, |g| g.toggle_group(id));
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_menu(Menu::Group(menu_cwd.clone(), id), ev.position, gpui::Anchor::TopLeft, cx)
            }),
        )
        .on_drag(DraggedGroup { cwd: group_cwd, id, name: group.name.clone().into() }, |d, _, _, cx| {
            cx.new(|_| d.clone())
        });
        div()
            .id(("group-block", i))
            .flex()
            .flex_col()
            .rounded_md()
            .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                cx.stop_propagation();
                if d.cwd != drop_cwd {
                    return;
                }
                this.edit_groups(&drop_cwd, |g| {
                    d.ids.iter().for_each(|sid| {
                        g.drop_into(sid, Some(id));
                    })
                });
                this.organize.selection.clear();
                cx.notify();
            }))
            .drag_over::<DraggedGroup>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedGroup, _, cx| {
                cx.stop_propagation();
                if d.cwd != reorder_cwd {
                    return;
                }
                this.edit_groups(&reorder_cwd, |g| g.move_group(d.id, id));
                cx.notify();
            }))
            .child(header)
            .when(open, |d| d.children(rows))
            .into_any_element()
    }

    pub(super) fn render_archived(&mut self, cwd: &Path, rows: Vec<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let hover = theme(cx).hover;
        let drop_cwd = cwd.to_path_buf();
        div()
            .id("archived-block")
            .flex()
            .flex_col()
            .opacity(0.55)
            .rounded_md()
            .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                cx.stop_propagation();
                if d.cwd != drop_cwd {
                    return;
                }
                this.edit_groups(&drop_cwd, |g| {
                    for id in &d.ids {
                        g.archive(id);
                    }
                });
                this.organize.selection.clear();
                cx.notify();
            }))
            .children(rows)
            .into_any_element()
    }
}
