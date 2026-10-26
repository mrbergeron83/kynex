#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
mod git;
mod project;
mod prompt;
mod session;
mod theme;
mod wsl;

fn main() -> eframe::Result {
    #[cfg(feature = "hotpatch")]
    dioxus_devtools::connect_subsecond();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Kynex")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            .with_drag_and_drop(true)
            // Kynex draws its own Hyprland-style frame; see chrome.rs.
            .with_decorations(false)
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native("Kynex", options, Box::new(|cc| {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        #[cfg(feature = "hotpatch")]
        {
            let ctx = cc.egui_ctx.clone();
            subsecond::register_handler(std::sync::Arc::new(move || ctx.request_repaint()));
        }
        Ok(Box::new(app::KynexApp::new(cc)))
    }))
}
