//! Settings → Storage's list of sessions: filtered by title or project, sorted
//! by size, last use or name. Each row goes to its session (opening its
//! project's tab if needed), shows where its log, scratchpad and checkpoints
//! are with a way to each folder, and frees them after asking.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{Context, SharedString, div, prelude::*};
use kynex_core::fmt::{ago, bytes};
use kynex_core::open::Opener;
use kynex_core::paths::project_name;
use kynex_core::session::Kind;
use kynex_core::storage::{self, Part, Roots, SessionUse, Sort, Usage};

use super::KynexApp;
use super::state::Active;
use super::storage::{PAGE, count};
use crate::theme::{Palette, mono};
use crate::ui::{Icon, Tooltip, ghost_button, icon};

impl KynexApp {
    /// What a session is known by: its title and its project.
    fn stored_label(&self, u: &SessionUse) -> (String, String) {
        let info = self.storage.as_ref().and_then(|v| v.info.get(&u.id));
        let title = info.and_then(|i| i.title.clone()).unwrap_or_else(|| u.id[..8].to_string());
        let place = self
            .project_for_slug(&u.slug)
            .or_else(|| info.and_then(|i| i.cwd.clone()))
            .map_or_else(|| u.slug.clone(), |p| project_name(&p));
        (title, place)
    }

    /// The folder to open a session in: its open project, else the folder its
    /// log says it ran in, if that's still there.
    fn stored_folder(&self, u: &SessionUse) -> Option<PathBuf> {
        self.project_for_slug(&u.slug).or_else(|| {
            let cwd = self.storage.as_ref()?.info.get(&u.id)?.cwd.clone()?;
            cwd.is_dir().then_some(cwd)
        })
    }

    /// Shows a session from the list: its project's tab (opened if it isn't),
    /// the session in it.
    fn go_to_stored(&mut self, path: PathBuf, id: String, cx: &mut Context<Self>) {
        let i = match self.saved.projects.iter().position(|p| p.path == path) {
            Some(i) => i,
            None => {
                self.add_project(path.clone());
                self.saved.projects.len() - 1
            }
        };
        self.app_settings = None;
        self.saved.active = Active::Project(i);
        self.save();
        self.show_code_view();
        self.switch_session(Kind::Code, path, id);
        cx.notify();
    }

    pub(super) fn render_storage_list(&mut self, usage: &Rc<Usage>, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(view) = self.storage.as_ref() else { return div() };
        let (filter, sort, shown) = (view.filter.clone(), view.sort, view.shown);
        let query = filter.read(cx).text().to_string();
        let logged: Vec<SessionUse> = usage.sessions.iter().filter(|u| u.has_log).cloned().collect();
        let order = storage::arrange(
            &logged,
            |u| {
                let (title, place) = self.stored_label(u);
                format!("{title} {place} {}", u.id)
            },
            &query,
            sort,
        );
        let total = order.len();
        let mut rows = Vec::new();
        for &i in order.iter().take(shown) {
            rows.push(self.render_stored(&logged[i], t, cx));
        }

        let sort_chip = |which: Sort, label: &'static str, cx: &mut Context<Self>| {
            ghost_button(("storage-sort", which as usize), cx)
                .text_xs()
                .when(sort == which, |d| d.bg(t.active).text_color(t.text))
                .when(sort != which, |d| d.text_color(t.text_muted))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(v) = &mut this.storage {
                        v.sort = which;
                    }
                    cx.notify();
                }))
        };
        let chips = [(Sort::Size, "Size"), (Sort::Recent, "Last used"), (Sort::Name, "Name")]
            .map(|(which, label)| sort_chip(which, label, cx));
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(crate::ui::px(28.))
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .child(icon(Icon::Search, t.text_muted).size_3p5().flex_none())
                    .child(div().flex_1().min_w_0().text_sm().child(filter)),
            )
            .children(chips);
        let more = total.saturating_sub(shown);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(bar)
            .child(div().text_xs().text_color(t.text_placeholder).child(match query.trim().is_empty() {
                true => count(total, "session"),
                false => format!("{} match", count(total, "session")),
            }))
            .child(div().flex().flex_col().gap_1().children(rows))
            .when(more > 0, |d| {
                d.child(
                    ghost_button("storage-more", cx)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(format!("Show {} more", more.min(PAGE)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(v) = &mut this.storage {
                                v.shown += PAGE;
                            }
                            cx.notify();
                        })),
                )
            })
    }

    fn render_stored(&mut self, u: &SessionUse, t: &Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (title, place) = self.stored_label(u);
        let folder = self.stored_folder(u);
        let kept = self.keeps(u);
        let view = self.storage.as_ref();
        let pending = view.and_then(|v| v.pending.clone()).filter(|p| p.at == u.id);
        let expanded = view.is_some_and(|v| v.open.contains(&u.id));
        let n = id_hash(&u.id);
        let id = SharedString::from(u.id.clone());
        let clear_scratch = Self::ask_on_click(u, &[Part::Scratch], "Clear the scratchpad of", cx);
        let clear_checkpoints = Self::ask_on_click(u, &[Part::Checkpoints], "Clear the file checkpoints of", cx);
        let delete = Self::ask_on_click(u, &Part::ALL, "Delete", cx);
        let go = match folder {
            Some(path) => {
                let id = u.id.clone();
                ghost_button(("storage-go", n), cx)
                    .text_xs()
                    .child("Go to session")
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to_stored(path.clone(), id.clone(), cx)))
            }
            None => ghost_button(("storage-go", n), cx)
                .text_xs()
                .text_color(t.text_placeholder)
                .child("Go to session")
                .tooltip(Tooltip::text("The folder it ran in isn't there any more")),
        };
        let toggle = {
            let id = u.id.clone();
            ghost_button(("storage-files", n), cx)
                .text_xs()
                .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_muted).size_3())
                .child("Files")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(v) = &mut this.storage
                        && !v.open.remove(&id)
                    {
                        v.open.insert(id.clone());
                    }
                    cx.notify();
                }))
        };
        let files = expanded.then(|| self.render_stored_files(u, t, cx));
        div()
            .id(("stored", n))
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(t.border_variant)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
                    .child(div().flex_none().font_family(mono()).text_sm().child(bytes(u.sizes.total()))),
            )
            .child(div().text_xs().text_color(t.text_muted).truncate().child(format!(
                "{place} · {} · log {} · scratchpad {} · checkpoints {}",
                ago(u.modified),
                bytes(u.sizes.log),
                bytes(u.sizes.scratch),
                bytes(u.sizes.checkpoints)
            )))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(go)
                    .child(toggle)
                    .when(u.sizes.scratch > 0, |d| {
                        d.child(
                            ghost_button(("storage-scratch", n), cx)
                                .text_xs()
                                .child("Clear scratchpad")
                                .on_click(clear_scratch),
                        )
                    })
                    .when(u.sizes.checkpoints > 0, |d| {
                        d.child(
                            ghost_button(("storage-checkpoints", n), cx)
                                .text_xs()
                                .child("Clear checkpoints")
                                .tooltip(Tooltip::text("Rewind won't be able to restore files in this session"))
                                .on_click(clear_checkpoints),
                        )
                    })
                    .when(!kept, |d| {
                        d.child(
                            ghost_button(("storage-delete", n), cx)
                                .text_xs()
                                .text_color(t.error)
                                .child("Delete session")
                                .on_click(delete),
                        )
                    })
                    .when(kept, |d| {
                        d.child(div().px_2().text_xs().text_color(t.text_placeholder).child("pinned or open"))
                    }),
            )
            .children(files)
            .children(pending.map(|p| self.render_confirm(p, t, cx)))
            .tooltip(Tooltip::text(id))
            .into_any_element()
    }

    /// Where a session's parts are: size, path, and a way to each folder.
    fn render_stored_files(&mut self, u: &SessionUse, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(roots) = Roots::local() else { return div() };
        let n = id_hash(&u.id);
        let rows = storage::locations(&roots, u).into_iter().map(|(part, path)| {
            let (label, size) = match part {
                Part::Log => ("Log", u.sizes.log),
                Part::Scratch => ("Scratchpad", u.sizes.scratch),
                Part::Checkpoints => ("Checkpoints", u.sizes.checkpoints),
            };
            // The log is a file: its folder opens, where the log's own folder sits beside it.
            let folder = if path.is_file() { path.parent().map(Path::to_path_buf) } else { Some(path.clone()) };
            let folder = folder.filter(|f| f.is_dir());
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .child(div().flex_none().w(crate::ui::px(84.)).text_color(t.text_muted).child(label))
                .child(div().flex_none().w(crate::ui::px(60.)).font_family(mono()).child(bytes(size)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(mono())
                        .text_color(t.text_placeholder)
                        .child(path.display().to_string()),
                )
                .child(match folder {
                    Some(f) => ghost_button(("storage-show", n * 4 + part as usize), cx)
                        .text_xs()
                        .child("Show in folder")
                        .on_click(cx.listener(move |this, _, _, _| this.open_in(Opener::Explorer, &f)))
                        .into_any_element(),
                    None => div().px_2().text_color(t.text_placeholder).child("none").into_any_element(),
                })
        });
        div().flex().flex_col().gap_1().pt_1().mt_1().border_t_1().border_color(t.border_variant).children(rows)
    }

    /// A click that asks before freeing `parts` of session `u`.
    fn ask_on_click(
        u: &SessionUse,
        parts: &'static [Part],
        what: &'static str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
        let u = u.clone();
        cx.listener(move |this, _, _, cx| {
            this.ask(u.id.clone(), vec![&u], parts, what);
            cx.notify();
        })
    }
}

/// A number for an element id from a session id.
fn id_hash(id: &str) -> usize {
    id.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}
