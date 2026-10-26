#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
mod cli;
mod git;
mod markdown;
mod project;
mod prompt;
mod session;
mod theme;
mod ui;
mod wsl;

use gpui::{
    App, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, prelude::*, px, size,
};

fn main() {
    gpui_platform::application().with_assets(ui::Assets).run(|cx: &mut App| {
        let saved = app::Saved::load();
        theme::set(saved.theme(), cx);
        theme::init_fonts(cx);
        ui::input::bind_keys(cx);
        prompt::bind_keys(cx);
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
