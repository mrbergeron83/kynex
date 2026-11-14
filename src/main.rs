#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Kynex: a desktop workspace for Claude Code. This crate is the gpui app;
//! logic without UI lives in `kynex-core` (crates/kynex-core).

mod app;
mod chrome;
mod file_links;
mod git;
mod git_pane;
mod markdown;
mod pane;
mod prompt;
mod scratch;
mod terminal;
mod theme;
mod ui;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, prelude::*, px, size,
};

fn main() {
    // Started by a session as its OpenRouter chat harness, not as the app. Its
    // environment (PATH included) is the app's, so nothing else needs setting up.
    if let Some(code) = kynex_core::chat_api::main_if_asked() {
        std::process::exit(code);
    }
    // `knx context | pane | session | events …`: ask the running app and print what it says.
    // `knx <folder>`: the running app opens it; with none, this one starts on it.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if kynex_core::control::args::is_command(&args) {
        kynex_core::control::client::attach_console();
        std::process::exit(kynex_core::control::client::run(&args));
    }
    if let Some(folder) = args.first().filter(|a| std::path::Path::new(a).is_dir())
        && let Ok(folder) = std::path::absolute(folder)
    {
        if kynex_core::control::client::open_folder(&folder.to_string_lossy()) {
            return;
        }
        let _ = app::OPEN_AT_START.set(folder);
    }
    // Started from Finder or the Dock, macOS gives no shell PATH: find `claude` and friends anyway.
    // SAFETY: first thing in main, before any other thread starts.
    unsafe { kynex_core::env_path::fix() };
    // Started from a session's shell (a dev build, say): that session's identity isn't ours.
    // SAFETY: as above, no other thread runs yet.
    unsafe { kynex_core::control::agent::forget_inherited() };
    // Sessions, shells and helpers end with Kynex, even if it crashes or is killed.
    if let Err(e) = kynex_core::process::tie_children_to_kynex() {
        eprintln!("kynex: {e}; child processes may outlive Kynex");
    }
    // Images read back from logs and tool results are shown as gpui images.
    kynex_core::session::set_preview_maker(ui::image_viewer::preview);
    gpui_platform::application().with_assets(ui::Assets).run(|cx: &mut App| {
        // Before any window: the taskbar groups by this ID, and notifications need one.
        cx.set_app_identity(kynex_core::attention::APP_ID, "Kynex");
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut saved = app::Saved::load();
        // `install.ps1` gives its shortcuts the same ID; those from before it did get it once.
        // A new version may have a new icon, which the shell won't show until told to.
        #[cfg(windows)]
        if let Ok(exe) = std::env::current_exe() {
            let stamp = !std::mem::replace(&mut saved.shortcuts_stamped, true);
            let version = env!("CARGO_PKG_VERSION");
            let icons = saved.icons_shown_for.as_deref() != Some(version);
            saved.icons_shown_for = Some(version.to_string());
            if stamp || icons {
                std::thread::spawn(move || {
                    use kynex_core::win_shell;
                    if stamp {
                        win_shell::stamp_shortcuts(&win_shell::shortcut_folders(), &exe, kynex_core::attention::APP_ID);
                    }
                    if icons {
                        win_shell::refresh_icons(&win_shell::icon_folders(), &exe);
                    }
                });
            }
        }
        theme::set_saved(saved.theme, saved.custom_theme.as_deref(), cx);
        theme::init_fonts(cx);
        theme::set_fonts(&saved.ui_font, &saved.code_font);
        ui::motion::apply(saved.motion, cx);
        kynex_core::cli::set_program(saved.cli_path.clone());
        kynex_core::delegate::set_default(saved.delegate);
        kynex_core::browser::set(saved.browser);
        kynex_core::terminal::ShellKind::set_default(saved.shell);
        ui::input::bind_keys(cx);
        prompt::bind_keys(cx);
        terminal::bind_keys(cx);
        app::bind_keys(cx);
        app::bind_command_keys(&saved.projects.iter().map(|p| p.path.clone()).collect::<Vec<_>>(), cx);

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
