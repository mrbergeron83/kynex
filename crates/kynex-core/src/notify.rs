//! Where each note goes and what it sounds like (#448, #447): per event (a session finished,
//! a session needs you) a delivery (nothing, a toast, an OS notification, both) and a sound
//! (none, the OS's own, or your file), with optional overrides of the sound per agent or
//! provider. The app asks [`Rules::resolve`] and [`Delivery::route`]; nothing here draws.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::attention::Why;
use crate::sound::{self, Sound};

/// What happened. A later one outranks an earlier one when both are due at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    /// A turn ended and you haven't looked at it.
    Finished,
    /// A permission prompt, a question, or a browser step waits for you.
    NeedsYou,
}

impl Event {
    pub const ALL: [Event; 2] = [Event::Finished, Event::NeedsYou];

    pub fn label(self) -> &'static str {
        match self {
            Event::Finished => "A session finishes",
            Event::NeedsYou => "A session needs you",
        }
    }

    /// The OS sound used when the rule says "default".
    pub fn default_sound(self) -> Sound {
        match self {
            Event::Finished => Sound::Done,
            Event::NeedsYou => Sound::NeedsYou,
        }
    }
}

impl From<Why> for Event {
    fn from(why: Why) -> Self {
        match why {
            Why::Finished => Event::Finished,
            Why::Answer | Why::Browser => Event::NeedsYou,
        }
    }
}

/// Where a note goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    #[default]
    Off,
    /// A toast in the window.
    Toast,
    /// An OS notification, even with Kynex in front (unless you're looking at that session).
    System,
    /// An OS notification while Kynex is in the background, a toast while it's in front.
    Both,
}

/// How one note is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Nothing,
    Toast,
    Os,
}

impl Delivery {
    pub const ALL: [Delivery; 4] = [Delivery::Off, Delivery::Toast, Delivery::System, Delivery::Both];

    pub fn label(self) -> &'static str {
        match self {
            Delivery::Off => "Off",
            Delivery::Toast => "Toast",
            Delivery::System => "System notification",
            Delivery::Both => "Both: system in the background, toast in front",
        }
    }

    /// How to show a note now: `window_active` is whether Kynex is in front, `on_screen`
    /// whether the session it's about is in sight. What you're looking at says nothing.
    pub fn route(self, window_active: bool, on_screen: bool) -> Route {
        if window_active && on_screen {
            return Route::Nothing;
        }
        match self {
            Delivery::Off => Route::Nothing,
            Delivery::Toast => Route::Toast,
            Delivery::System => Route::Os,
            Delivery::Both if window_active => Route::Toast,
            Delivery::Both => Route::Os,
        }
    }
}

/// A sound for an event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundChoice {
    #[default]
    None,
    /// The OS's own sound for the event.
    Default,
    /// Your sound file; the default one if it can't be played.
    File(PathBuf),
}

impl SoundChoice {
    /// Plays it for `event` and returns at once. On macOS only from the main thread.
    pub fn play(&self, event: Event) {
        match self {
            SoundChoice::None => {}
            SoundChoice::Default => sound::play(event.default_sound()),
            SoundChoice::File(path) => sound::play_file(path, event.default_sound()),
        }
    }
}

/// What one event does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventRule {
    pub delivery: Delivery,
    pub sound: SoundChoice,
}

/// An agent's or provider's own sounds; `None` means the general one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Override {
    pub finished: Option<SoundChoice>,
    pub needs_you: Option<SoundChoice>,
}

impl Override {
    pub fn get(&self, event: Event) -> Option<&SoundChoice> {
        match event {
            Event::Finished => self.finished.as_ref(),
            Event::NeedsYou => self.needs_you.as_ref(),
        }
    }

    fn slot(&mut self, event: Event) -> &mut Option<SoundChoice> {
        match event {
            Event::Finished => &mut self.finished,
            Event::NeedsYou => &mut self.needs_you,
        }
    }
}

/// What an event comes to for one session or agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub delivery: Delivery,
    pub sound: SoundChoice,
}

/// All of it, as saved in `state.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub finished: EventRule,
    pub needs_you: EventRule,
    /// By agent or provider name.
    pub overrides: BTreeMap<String, Override>,
}

impl Rules {
    pub fn rule(&self, event: Event) -> &EventRule {
        match event {
            Event::Finished => &self.finished,
            Event::NeedsYou => &self.needs_you,
        }
    }

    pub fn rule_mut(&mut self, event: Event) -> &mut EventRule {
        match event {
            Event::Finished => &mut self.finished,
            Event::NeedsYou => &mut self.needs_you,
        }
    }

    /// The delivery and sound for `event`, from `agent` (an agent's or provider's name) if
    /// it has its own sound for it.
    pub fn resolve(&self, event: Event, agent: Option<&str>) -> Resolved {
        let rule = self.rule(event);
        let own = agent.and_then(|a| self.overrides.get(a)).and_then(|o| o.get(event));
        Resolved { delivery: rule.delivery, sound: own.unwrap_or(&rule.sound).clone() }
    }

    /// Whether anything is delivered at all.
    pub fn any_on(&self) -> bool {
        Event::ALL.iter().any(|e| self.rule(*e).delivery != Delivery::Off)
    }

    /// Sets (or with `None` clears) `agent`'s own sound for `event`; an override with
    /// nothing left in it goes away.
    pub fn set_override(&mut self, agent: &str, event: Event, sound: Option<SoundChoice>) {
        *self.overrides.entry(agent.to_string()).or_default().slot(event) = sound;
        if self.overrides.get(agent).is_some_and(|o| *o == Override::default()) {
            self.overrides.remove(agent);
        }
    }
}

/// The names a session's provider goes by in [`Rules::overrides`].
pub const PROVIDERS: [&str; 3] = ["Claude Code", "OpenRouter", "Grok"];

/// The name of `launch` among [`PROVIDERS`].
pub fn provider_name(launch: &crate::provider::Launch) -> &'static str {
    use crate::provider::Launch;
    match launch {
        Launch::Anthropic => PROVIDERS[0],
        Launch::OpenRouter { .. } | Launch::OpenRouterChat { .. } => PROVIDERS[1],
        Launch::Grok { .. } => PROVIDERS[2],
    }
}

/// Of the sounds due at once, the one of the most urgent event (the first on a tie).
pub fn loudest(due: impl IntoIterator<Item = (Event, SoundChoice)>) -> Option<(Event, SoundChoice)> {
    due.into_iter().fold(None, |best, next| match best {
        Some(b) if b.0 >= next.0 => Some(b),
        _ => Some(next),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(p: &str) -> SoundChoice {
        SoundChoice::File(p.into())
    }

    #[test]
    fn why_maps_to_events() {
        assert_eq!(Event::from(Why::Finished), Event::Finished);
        assert_eq!(Event::from(Why::Answer), Event::NeedsYou);
        assert_eq!(Event::from(Why::Browser), Event::NeedsYou);
    }

    #[test]
    fn defaults_are_quiet() {
        let r = Rules::default();
        assert!(!r.any_on());
        assert_eq!(r.resolve(Event::Finished, None), Resolved { delivery: Delivery::Off, sound: SoundChoice::None });
    }

    #[test]
    fn routes() {
        use Delivery::*;
        // In sight and in front: nothing, whatever the delivery.
        for d in Delivery::ALL {
            assert_eq!(d.route(true, true), Route::Nothing);
        }
        assert_eq!(Off.route(false, false), Route::Nothing);
        assert_eq!(Toast.route(true, false), Route::Toast);
        assert_eq!(Toast.route(false, false), Route::Toast);
        assert_eq!(System.route(true, false), Route::Os);
        assert_eq!(System.route(false, true), Route::Os);
        assert_eq!(Both.route(true, false), Route::Toast);
        // The window in the background: on screen means nothing.
        assert_eq!(Both.route(false, true), Route::Os);
    }

    #[test]
    fn an_override_wins_for_its_agent_only() {
        let mut r = Rules::default();
        r.rule_mut(Event::Finished).sound = SoundChoice::Default;
        r.set_override("Grok", Event::Finished, Some(file("g.wav")));
        assert_eq!(r.resolve(Event::Finished, Some("Grok")).sound, file("g.wav"));
        assert_eq!(r.resolve(Event::Finished, Some("Other")).sound, SoundChoice::Default);
        assert_eq!(r.resolve(Event::Finished, None).sound, SoundChoice::Default);
        // Another event isn't touched, and an override can silence one.
        assert_eq!(r.resolve(Event::NeedsYou, Some("Grok")).sound, SoundChoice::None);
        r.set_override("Grok", Event::NeedsYou, Some(SoundChoice::None));
        r.rule_mut(Event::NeedsYou).sound = SoundChoice::Default;
        assert_eq!(r.resolve(Event::NeedsYou, Some("Grok")).sound, SoundChoice::None);
    }

    #[test]
    fn clearing_the_last_override_removes_the_entry() {
        let mut r = Rules::default();
        r.set_override("A", Event::Finished, Some(file("a.wav")));
        r.set_override("A", Event::NeedsYou, Some(file("b.wav")));
        r.set_override("A", Event::Finished, None);
        assert_eq!(r.overrides.len(), 1);
        r.set_override("A", Event::NeedsYou, None);
        assert!(r.overrides.is_empty());
    }

    #[test]
    fn loudest_is_the_most_urgent_event() {
        assert_eq!(loudest([]), None);
        let got = loudest([
            (Event::Finished, file("f.wav")),
            (Event::NeedsYou, file("n1.wav")),
            (Event::NeedsYou, file("n2.wav")),
            (Event::Finished, file("f2.wav")),
        ]);
        assert_eq!(got, Some((Event::NeedsYou, file("n1.wav"))));
    }

    #[test]
    fn rules_round_trip_and_tolerate_missing_fields() {
        let mut r =
            Rules { finished: EventRule { delivery: Delivery::Both, sound: file("x.wav") }, ..Rules::default() };
        r.set_override("Claude Code", Event::NeedsYou, Some(SoundChoice::Default));
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Rules>(&text).unwrap(), r);
        assert_eq!(serde_json::from_str::<Rules>("{}").unwrap(), Rules::default());
        assert!(serde_json::from_str::<Rules>(r#"{"finished":{"delivery":"Toast"}}"#).unwrap().any_on());
    }
}
