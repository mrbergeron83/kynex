#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Kynex: a desktop workspace for Claude Code. This crate is the gpui app;
//! logic without UI lives in `kynex-core` (crates/kynex-core).

mod app;
mod chrome;
mod git;
mod markdown;
mod prompt;
mod terminal;
mod theme;
mod ui;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, prelude::*, px, size,
};

fn main() {
    // Started from Finder or the Dock, macOS gives no shell PATH: find `claude` and friends anyway.
    // SAFETY: first thing in main, before any other thread starts.
    unsafe { kynex_core::env_path::fix() };
    // Sessions, shells and helpers end with Kynex, even if it crashes or is killed.
    if let Err(e) = kynex_core::process::tie_children_to_kynex() {
        eprintln!("kynex: {e}; child processes may outlive Kynex");
    }
    // Images read back from logs and tool results are shown as gpui images.
    kynex_core::session::set_preview_maker(ui::image_viewer::preview);
    gpui_platform::application().with_assets(ui::Assets).run(|cx: &mut App| {
        // Before any window: the taskbar groups by this ID, and notifications need one.
        cx.set_app_identity(kynex_core::attention::APP_ID, "Kynex");
        // Kynex's own shortcuts get the same ID, or a pinned one would sit apart from the window.
        #[cfg(windows)]
        if let Ok(exe) = std::env::current_exe() {
            std::thread::spawn(move || {
                use kynex_core::win_shell;
                win_shell::stamp_shortcuts(&win_shell::shortcut_folders(), &exe, kynex_core::attention::APP_ID)
            });
        }
        let saved = app::Saved::load();
        theme::set(saved.theme, cx);
        theme::init_fonts(cx);
        theme::set_fonts(&saved.ui_font, &saved.code_font);
        ui::motion::apply(saved.motion, cx);
        kynex_core::cli::set_program(saved.cli_path.clone());
        kynex_core::terminal::ShellKind::set_default(saved.shell);
        ui::input::bind_keys(cx);
        prompt::bind_keys(cx);
        terminal::bind_keys(cx);
        app::bind_keys(cx);

        let bounds = match saved.window {
            Some(w) if w.width >= 640. && w.height >= 400. => {
                let b = Bounds::new(point(px(w.x), px(w.y)), size(px(w.width), px(w.height)));
                if w.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }
            }
            _ => WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(800.)), cx)),
        };
        // For test instances driven by tools: open without taking focus.
        let background = std::env::var_os("KYNEX_BACKGROUND").is_some();
        let options = WindowOptions {
            focus: !background,
            window_bounds: Some(bounds),
            window_min_size: Some(size(px(640.), px(400.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Kynex".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.), px(11.))),
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            app_id: Some("kynex".into()),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| cx.new(|cx| app::KynexApp::new(saved, window, cx)))
            .expect("failed to open the Kynex window");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if !background {
            cx.activate(true);
        }
    });
}
