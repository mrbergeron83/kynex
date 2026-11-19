//! Settings > General > Notifications: per event where the note goes (nothing, a toast, an OS
//! notification, both) and its sound (none, the OS's, your file), with sounds of their own for
//! a provider or an agent (`kynex_core::notify`).

use gpui::{Context, PathPromptOptions, div, prelude::*};
use kynex_core::notify::{self, Delivery, Event, SoundChoice};

use super::KynexApp;
use super::app_settings::choice;
use super::settings::heading;
use crate::theme::Palette;
use crate::ui::outline_button;

/// Whose sound: the general one, or a provider's or agent's own.
type Who = Option<String>;

impl KynexApp {
    pub(super) fn render_notifications(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let rules = &self.saved.notifications;
        let mut col = div().flex().flex_col().gap_1().child(heading(
            "Notifications",
            "Where a note about a session or agent you aren't looking at goes, and what it sounds like. The sound plays with the note. Kynex's icon shows a dot whenever a session needs you.",
            t,
        ));
        for (e, event) in Event::ALL.into_iter().enumerate() {
            let rule = rules.rule(event);
            col = col
                .child(div().pt_1().text_sm().child(event.label()))
                .children(Delivery::ALL.into_iter().enumerate().map(|(n, d)| {
                    choice(("delivery", e * 10 + n), rule.delivery == d, d.label().into(), t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_delivery(event, d, cx)))
                }))
                .child(div().pt_1().text_xs().text_color(t.text_muted).child("Sound"))
                .child(self.sound_picker(None, event, Some(rule.sound.clone()), e, t, cx));
        }
        let mut names: Vec<String> = notify::PROVIDERS.iter().map(|s| s.to_string()).collect();
        for r in &self.agent_rules {
            if !names.contains(&r.name) {
                names.push(r.name.clone());
            }
        }
        col = col.child(heading(
            "Sounds per provider and agent",
            "A provider or an agent can use its own sound for each event; \"Same as above\" keeps the general one.",
            t,
        ));
        for (a, name) in names.into_iter().enumerate() {
            let own = rules.overrides.get(&name).cloned().unwrap_or_default();
            col = col.child(div().pt_1().text_sm().child(name.clone()));
            for (e, event) in Event::ALL.into_iter().enumerate() {
                col = col.child(div().text_xs().text_color(t.text_muted).child(event.label())).child(
                    self.sound_picker(Some(name.clone()), event, own.get(event).cloned(), 10 + a * 2 + e, t, cx),
                );
            }
        }
        col
    }

    /// The choices for one sound: the current one (`None` for "same as general", which only an
    /// override has), and for a file its name with a button to pick another.
    fn sound_picker(
        &self,
        who: Who,
        event: Event,
        current: Option<SoundChoice>,
        id: usize,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let own = who.is_some();
        let mut options: Vec<(&'static str, Option<SoundChoice>)> = Vec::new();
        if own {
            options.push(("Same as above", None));
        }
        options.push(("No sound", Some(SoundChoice::None)));
        options.push(("Default sound", Some(SoundChoice::Default)));
        let mut col = div().flex().flex_col();
        for (n, (label, pick)) in options.into_iter().enumerate() {
            let w = who.clone();
            col = col.child(
                choice(("sound", id * 8 + n), current == pick, label.into(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_sound(w.clone(), event, pick.clone(), cx))),
            );
        }
        let file = match &current {
            Some(SoundChoice::File(p)) => Some(p.clone()),
            _ => None,
        };
        let label = match &file {
            Some(p) => format!(
                "Your file: {}",
                p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
            ),
            None => "Your file".to_string(),
        };
        // "Same as above" plays the general sound.
        let playing = current.clone().unwrap_or_else(|| self.saved.notifications.rule(event).sound.clone());
        col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().min_w_0().child(
                        choice(("sound", id * 8 + 7), file.is_some(), label, t)
                            .on_click(cx.listener(move |this, _, _, cx| this.pick_sound(who.clone(), event, cx))),
                    ),
                )
                .child(
                    outline_button(("sound-try", id), "Try", cx)
                        .on_click(cx.listener(move |_, _, _, _| playing.play(event))),
                ),
        )
    }

    fn set_delivery(&mut self, event: Event, delivery: Delivery, cx: &mut Context<Self>) {
        self.saved.notifications.rule_mut(event).delivery = delivery;
        self.save();
        cx.notify();
    }

    /// Sets a general sound (`who` is `None`), or a provider's or agent's own (`None` clears it).
    fn set_sound(&mut self, who: Who, event: Event, sound: Option<SoundChoice>, cx: &mut Context<Self>) {
        match who {
            None => self.saved.notifications.rule_mut(event).sound = sound.unwrap_or_default(),
            Some(name) => self.saved.notifications.set_override(&name, event, sound),
        }
        self.save();
        cx.notify();
    }

    fn pick_sound(&mut self, who: Who, event: Event, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use this sound".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |this, cx| this.set_sound(who, event, Some(SoundChoice::File(path)), cx)).ok();
            }
        })
        .detach();
    }
}
