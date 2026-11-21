//! Opening files in the user's editor (VS Code at a specific line/column when installed).

use std::path::PathBuf;

use gpui::App;

/// Opens a file in the external editor (VS Code at the line/column when installed).
pub fn open(path: PathBuf, line: Option<u32>, col: Option<u32>, cx: &App) {
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = kynex_core::open::open_file(&path, line, col) {
                eprintln!("kynex: {e}");
            }
        })
        .detach();
}
