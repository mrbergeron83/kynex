//! File paths in Claude's text as links (`src/app/mod.rs:123`): found by
//! `kynex_core::links`, and linked only once the file is known to exist. That's
//! checked off the UI thread and cached; windows redraw when an answer comes in.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui::App;
use kynex_core::links;

use crate::ui::selectable::Link;

enum Known {
    Checking,
    File,
    /// Not a file when looked at; looked at again after a while, as Claude may
    /// mention a file before it writes it.
    Missing(Instant),
}

const RECHECK: Duration = Duration::from_secs(10);

static KNOWN: Mutex<Option<HashMap<PathBuf, Known>>> = Mutex::new(None);

/// The file links in `text`, for paths against `cwd` that exist. Paths not yet
/// checked are looked up in the background, and link on a later frame.
pub fn find(cwd: &Path, text: &str, cx: &App) -> Vec<(Range<usize>, Link)> {
    let found = links::find(text);
    if found.is_empty() {
        return Vec::new();
    }
    let Ok(mut known) = KNOWN.lock() else { return Vec::new() };
    let known = known.get_or_insert_with(HashMap::new);
    if known.len() >= 8192 {
        known.clear();
    }
    let mut out = Vec::new();
    let mut check = Vec::new();
    for r in found {
        let path = links::resolve(cwd, &r.path);
        match known.get(&path) {
            Some(Known::File) => out.push((r.range, Link::File { path, line: r.line, col: r.col })),
            Some(Known::Checking) => {}
            Some(Known::Missing(at)) if at.elapsed() < RECHECK => {}
            _ => {
                known.insert(path.clone(), Known::Checking);
                check.push(path);
            }
        }
    }
    if !check.is_empty() {
        cx.spawn(async move |cx| {
            let answers = cx
                .background_executor()
                .spawn(async move { check.into_iter().map(|p| (p.is_file(), p)).collect::<Vec<_>>() })
                .await;
            let any = answers.iter().any(|(file, _)| *file);
            if let Ok(mut known) = KNOWN.lock() {
                let known = known.get_or_insert_with(HashMap::new);
                for (file, path) in answers {
                    known.insert(path, if file { Known::File } else { Known::Missing(Instant::now()) });
                }
            }
            if any {
                cx.update(|cx| cx.refresh_windows());
            }
        })
        .detach();
    }
    out
}

/// Opens a linked file in the editor (VS Code at the line when it's installed).
pub fn open(path: PathBuf, line: Option<u32>, col: Option<u32>, cx: &App) {
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = kynex_core::open::open_file(&path, line, col) {
                eprintln!("kynex: {e}");
            }
        })
        .detach();
}
