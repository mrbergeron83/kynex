//! Settings → Storage: how much disk Claude Code's sessions take (logs,
//! scratchpads, file checkpoints), by project, and freeing it. Every deletion
//! asks first, says what it frees, and never touches a pinned or open
//! session. The list of sessions is `storage_list.rs`; `kynex_core::storage`
//! walks and deletes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

use crate::ui::px;
use gpui::{Context, Entity, div, prelude::*};
use kynex_core::fmt::bytes;
use kynex_core::paths::project_name;
use kynex_core::session;
use kynex_core::storage::{self, Part, Roots, SessionUse, Sort, Usage};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::settings::heading;
use crate::theme::{Palette, mono};
use crate::ui::input::TextInput;
use crate::ui::{ghost_button, outline_button};

pub(super) struct StorageView {
    /// `None` while the folders are walked.
    pub(super) usage: Option<Rc<Usage>>,
    /// Each session's title and the folder it ran in, read from its log after
    /// the sizes are in.
    pub(super) info: HashMap<String, LogInfo>,
    /// A deletion waiting for Yes, shown under what asked for it.
    pub(super) pending: Option<Pending>,
    /// A deletion under way.
    pub(super) deleting: bool,
    pub(super) filter: Entity<TextInput>,
    pub(super) sort: Sort,
    /// Sessions showing where their files are.
    pub(super) open: HashSet<String>,
    /// How many of the list are shown.
    pub(super) shown: usize,
}

#[derive(Clone, Default)]
pub(super) struct LogInfo {
    pub(super) title: Option<String>,
    pub(super) cwd: Option<PathBuf>,
}

#[derive(Clone)]
pub(super) struct Pending {
    /// Which row or cleanup line asked: a session id, or a cleanup's name.
    pub(super) at: String,
    targets: Vec<(String, String)>,
    parts: &'static [Part],
    question: String,
}

/// Rows shown at first, and added by "Show more".
pub(super) const PAGE: usize = 25;

impl KynexApp {
    /// Walks the session folders again, off the UI thread: sizes first, then
    /// each log's title and folder.
    pub(super) fn scan_storage(&mut self, cx: &mut Context<Self>) {
        if self.storage.is_none() {
            let filter = cx.new(|cx| TextInput::new(cx).placeholder("Filter by title or project"));
            self.storage = Some(StorageView {
                usage: None,
                info: HashMap::new(),
                pending: None,
                deleting: false,
                filter,
                sort: Sort::default(),
                open: HashSet::new(),
                shown: PAGE,
            });
        }
        if let Some(v) = &mut self.storage {
            v.usage = None;
            v.pending = None;
        }
        cx.spawn(async move |this, cx| {
            let usage =
                cx.background_spawn(async { Roots::local().map(|r| storage::scan(&r)).unwrap_or_default() }).await;
            let logs: Vec<(String, PathBuf)> = Roots::local()
                .map(|r| {
                    let log = |u: &SessionUse| r.projects.join(&u.slug).join(format!("{}.jsonl", u.id));
                    usage.sessions.iter().filter(|u| u.has_log).map(|u| (u.id.clone(), log(u))).collect()
                })
                .unwrap_or_default();
            let usage = Rc::new(usage);
            this.update(cx, |this, cx| {
                if let Some(v) = &mut this.storage {
                    v.usage = Some(usage);
                }
                cx.notify();
            })
            .ok();
            let info: HashMap<String, LogInfo> = cx
                .background_spawn(async move {
                    let read = |p: &PathBuf| LogInfo { title: session::title_of(p), cwd: session::cwd_of(p) };
                    logs.into_iter().map(|(id, p)| (id, read(&p))).collect()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(v) = &mut this.storage {
                    v.info = info;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The open project a session folder belongs to.
    pub(super) fn project_for_slug(&self, slug: &str) -> Option<PathBuf> {
        self.saved.projects.iter().map(|p| p.path.clone()).find(|p| storage::slug(p).eq_ignore_ascii_case(slug))
    }

    /// Sessions a cleanup leaves alone: open in Kynex, or pinned.
    pub(super) fn keeps(&mut self, u: &SessionUse) -> bool {
        let open = self.all_sessions().any(|s| s.session_id.as_deref() == Some(u.id.as_str()));
        let pinned = self.project_for_slug(&u.slug).is_some_and(|p| self.groups_for(&p).is_pinned(&u.id));
        open || pinned
    }

    pub(super) fn ask(&mut self, at: String, targets: Vec<&SessionUse>, parts: &'static [Part], what: &str) {
        let freed: u64 = targets
            .iter()
            .map(|u| {
                parts
                    .iter()
                    .map(|p| match p {
                        Part::Log => u.sizes.log,
                        Part::Scratch => u.sizes.scratch,
                        Part::Checkpoints => u.sizes.checkpoints,
                    })
                    .sum::<u64>()
            })
            .sum();
        let n = targets.len();
        let question = match n {
            0 => format!("Nothing to {}.", what.to_lowercase()),
            _ => format!("{what} {}? Frees {}.", count(n, "session"), bytes(freed)),
        };
        let targets = targets.iter().map(|u| (u.slug.clone(), u.id.clone())).collect();
        if let Some(v) = &mut self.storage {
            v.pending = Some(Pending { at, targets, parts, question });
        }
    }

    /// Does the deletion waiting for Yes, then walks the folders again.
    fn confirm_storage(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &mut self.storage else { return };
        let Some(p) = view.pending.take().filter(|p| !p.targets.is_empty()) else { return };
        view.deleting = true;
        // Deleted logs leave the sessions panel and their groups.
        if p.parts.contains(&Part::Log) {
            for (slug, id) in &p.targets {
                if let Some(path) = self.project_for_slug(slug) {
                    self.edit_groups(&path, |g| g.forget(id));
                }
            }
            self.reread_sessions();
        }
        let (targets, parts) = (p.targets, p.parts);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let roots = Roots::local().ok_or("Couldn't find your home folder.")?;
                    let mut freed = 0;
                    for (slug, id) in &targets {
                        freed += storage::delete(&roots, slug, id, parts)?;
                    }
                    Ok::<u64, String>(freed)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(v) = &mut this.storage {
                    v.deleting = false;
                }
                match result {
                    Ok(freed) => this.toast(Toast::success(format!("Freed {}", bytes(freed)))),
                    Err(e) => this.toast(Toast::error(e)),
                };
                this.scan_storage(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_storage(&mut self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        if self.storage.is_none() {
            self.scan_storage(cx);
        }
        let head = div().flex().flex_col().gap_2().child(heading(
            "Storage",
            "What Claude Code keeps for each session: its log, its scratchpad (temporary files) and file checkpoints, which rewind restores files from.",
            t,
        ));
        let Some(usage) = self.storage.as_ref().and_then(|v| v.usage.clone()) else {
            return head.child(div().text_sm().text_color(t.text_placeholder).child("Measuring…"));
        };
        let total = usage.total();
        let summary = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_size(px(18.)).font_family(mono()).child(bytes(total.total())))
            .child(div().flex_1().text_xs().text_color(t.text_muted).child(format!(
                "logs {} · scratchpads {} · checkpoints {}",
                bytes(total.log),
                bytes(total.scratch),
                bytes(total.checkpoints)
            )))
            .child(
                outline_button("storage-refresh", "Measure again", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.scan_storage(cx))),
            );

        let projects =
            div().flex().flex_col().gap_0p5().children(usage.by_project().into_iter().take(6).map(|(slug, sizes)| {
                let name = self.project_for_slug(&slug).map_or_else(|| slug.clone(), |p| project_name(&p));
                let name = if slug.is_empty() { "Checkpoints of sessions that are gone".into() } else { name };
                line(name, bytes(sizes.total()), t)
            }));

        let cleanup = self.render_cleanup(&usage, t, cx);
        let list = self.render_storage_list(&usage, t, cx);
        head.gap_4()
            .child(summary)
            .child(section("By project", t).child(projects))
            .child(section("Clean up", t).child(cleanup))
            .child(section("Sessions", t).child(list))
    }

    fn render_cleanup(&mut self, usage: &Rc<Usage>, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let pending = self.storage.as_ref().and_then(|v| v.pending.clone());
        let now = SystemTime::now();
        let older = |days: u64, label: &'static str| {
            let usage = usage.clone();
            let click = cx.listener(move |this, _, _, cx| {
                let old: Vec<SessionUse> = usage.older_than(days, now, |_| false).into_iter().cloned().collect();
                let targets: Vec<SessionUse> = old.into_iter().filter(|u| !this.keeps(u)).collect();
                this.ask("older".into(), targets.iter().collect(), &Part::ALL, "Delete");
                cx.notify();
            });
            ghost_button(("storage-older", days as usize), cx).text_xs().child(label).on_click(click)
        };
        let (d30, d90, d365) = (older(30, "30 days"), older(90, "90 days"), older(365, "a year"));
        let leftovers = usage.leftovers().len();
        let left_size: u64 = usage.leftovers().iter().map(|u| u.sizes.total()).sum();
        let usage2 = usage.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().text_sm().child("Delete sessions not used in"))
                    .child(d30)
                    .child(d90)
                    .child(d365),
            )
            .children(pending.clone().filter(|p| p.at == "older").map(|p| self.render_confirm(p, t, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().text_sm().child(format!(
                        "Left from deleted sessions: {} · {}",
                        count(leftovers, "scratchpad"),
                        bytes(left_size)
                    )))
                    .when(leftovers > 0, |d| {
                        d.child(ghost_button("storage-leftovers", cx).text_xs().child("Clear").on_click(cx.listener(
                            move |this, _, _, cx| {
                                let targets = usage2.leftovers();
                                this.ask("leftovers".into(), targets, &Part::ALL, "Clear leftovers of");
                                cx.notify();
                            },
                        )))
                    }),
            )
            .children(pending.filter(|p| p.at == "leftovers").map(|p| self.render_confirm(p, t, cx)))
            .child(
                div()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child("Pinned sessions and sessions open in Kynex are kept."),
            )
    }

    pub(super) fn render_confirm(&self, p: Pending, t: &Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let deleting = self.storage.as_ref().is_some_and(|v| v.deleting);
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(t.error.opacity(0.08))
            .child(div().flex_1().text_xs().text_color(t.text).child(p.question.clone()))
            .when(!p.targets.is_empty() && !deleting, |d| {
                d.child(
                    outline_button("storage-yes", "Yes, delete", cx)
                        .text_color(t.error)
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_storage(cx))),
                )
            })
            .child(outline_button("storage-no", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                if let Some(v) = &mut this.storage {
                    v.pending = None;
                }
                cx.notify();
            })))
            .into_any_element()
    }
}

fn section(title: &'static str, t: &Palette) -> gpui::Div {
    div().flex().flex_col().gap_1p5().child(div().text_xs().text_color(t.text_muted).child(title))
}

fn line(name: String, size: String, t: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .child(div().flex_1().min_w_0().truncate().child(name))
        .child(div().flex_none().font_family(mono()).text_color(t.text_muted).child(size))
}

pub(super) fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
