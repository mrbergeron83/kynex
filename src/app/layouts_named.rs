//! Named layouts: save, apply, export, and import pane layouts (`kynex_core::named_layouts`).

use gpui::{AnyElement, Context, Focusable, PathPromptOptions, Window, div, prelude::*};
use kynex_core::named_layouts::{self, NamedLayout};
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{divider, menu_item, px};

impl KynexApp {
    fn current_named_layout(&self) -> Option<NamedLayout> {
        let (_, cwd) = self.tiling_scope()?;
        Some(named_layouts::export(&self.tiles.get(&cwd)?.save()))
    }

    pub(super) fn named_layout_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut items: Vec<AnyElement> = vec![div().my_1().child(divider(cx)).into_any_element()];
        match self.layout_name.clone() {
            Some(input) => items.push(div().p_1().w(px(240.)).child(input).into_any_element()),
            None => items.push(
                menu_item("layout-save", "Save layout as…", false, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.ask_layout_name(window, cx)))
                    .into_any_element(),
            ),
        }
        let names = named_layouts::list_in(&named_layouts::dir());
        if !names.is_empty() {
            items.push(
                div().px_2().pt_1().text_xs().text_color(t.text_placeholder).child("Apply layout").into_any_element(),
            );
        }
        for (n, name) in names.into_iter().enumerate() {
            items.push(
                menu_item(("layout-named", n), name.clone(), false, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        this.apply_named_layout(name.clone(), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        items.push(div().my_1().child(divider(cx)).into_any_element());
        items.push(
            menu_item("layout-export", "Export this layout…", false, cx)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    this.export_layout(cx);
                }))
                .into_any_element(),
        );
        items.push(
            menu_item("layout-import", "Import a layout…", false, cx)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    this.import_layout(cx);
                }))
                .into_any_element(),
        );
        items
    }

    fn ask_layout_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Name this layout"));
        cx.subscribe(&input, |this, input, ev, cx| {
            if let InputEvent::Submit = ev {
                let name = input.read(cx).text().to_string();
                this.layout_name = None;
                this.menu = None;
                this.save_named_layout(name, cx);
            }
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.layout_name = Some(input);
        cx.notify();
    }

    fn save_named_layout(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(layout) = self.current_named_layout() else { return };
        let task = cx.background_spawn(async move { named_layouts::save_in(&named_layouts::dir(), &name, &layout) });
        cx.spawn(async move |this, cx| {
            let done = task.await;
            this.update(cx, |this, cx| {
                this.toast(match done {
                    Ok(name) => Toast::info(format!("Saved the layout {name}")),
                    Err(e) => Toast::error(e),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn apply_named_layout(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { named_layouts::load_in(&named_layouts::dir(), &name) });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = task.await;
            this.update_in(cx, |this, window, cx| match loaded {
                Ok(layout) => this.apply_layout(&layout, window, cx),
                Err(e) => {
                    this.toast(Toast::error(e));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_layout(&mut self, layout: &NamedLayout, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd)) = self.tiling_scope() else { return };
        self.tiles.remove(&cwd);
        self.saved_workspaces(&project).set_layout(&cwd, Some(named_layouts::apply(layout)));
        self.focus_input(window, cx);
        cx.notify();
    }

    fn export_layout(&mut self, cx: &mut Context<Self>) {
        let Some(layout) = self.current_named_layout() else { return };
        let dir = named_layouts::dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = cx.prompt_for_new_path(&dir, Some("layout.json"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let done = cx.background_spawn(async move { named_layouts::export_to(&path, &layout) }).await;
            this.update(cx, |this, cx| {
                this.toast(match done {
                    Ok(()) => Toast::info("Exported the layout"),
                    Err(e) => Toast::error(e),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn import_layout(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import layout".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let done = cx
                .background_spawn(async move {
                    let (name, layout) = named_layouts::import_from(&path)?;
                    named_layouts::save_in(&named_layouts::dir(), &name, &layout)
                })
                .await;
            this.update(cx, |this, cx| {
                this.toast(match done {
                    Ok(name) => Toast::info(format!("Imported the layout {name}: apply it from the layouts menu")),
                    Err(e) => Toast::error(e),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
