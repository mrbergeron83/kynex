//! Toasts: the short notes in the corner of the window ("Background task
//! finished in …", "Kynex 0.0.7 is available", an error). The rules live
//! here, the app only draws them: how long each kind stays, pausing while you
//! point at them or are away, merging repeats, and how many show at once.
//!
//! Time only moves through [`Toasts::tick`], so the rules test without a clock.

use std::time::Duration;

use crate::live::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

impl Kind {
    /// How long a toast stays on its own: errors wait for you.
    pub fn lifetime(self) -> Option<Duration> {
        match self {
            Kind::Info | Kind::Success => Some(Duration::from_secs(5)),
            Kind::Warning => Some(Duration::from_secs(8)),
            Kind::Error => None,
        }
    }
}

/// What a toast's button (and a click on it) does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// "Go to session": shows that session.
    GoToSession(Key),
    /// "View": the About modal, where a new version installs.
    ViewUpdate,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::GoToSession(_) => "Go to session",
            Action::ViewUpdate => "View",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// Stable while it's up, for drawing and closing it.
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    /// Shown after the title, muted and cut to fit.
    pub detail: String,
    pub action: Option<Action>,
    /// The session it's about: toasts of one kind for one session merge, and
    /// go once that session is shown.
    pub session: Option<Key>,
    /// How many notes merged into this one.
    pub count: usize,
    lifetime: Option<Duration>,
    /// Time up while not paused.
    age: Duration,
    /// Closing: how far into its exit.
    leaving: Option<Duration>,
}

impl Toast {
    fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self {
            id: 0,
            kind,
            title: title.into(),
            detail: String::new(),
            action: None,
            session: None,
            count: 1,
            lifetime: kind.lifetime(),
            age: Duration::ZERO,
            leaving: None,
        }
    }

    pub fn info(title: impl Into<String>) -> Self {
        Self::new(Kind::Info, title)
    }

    pub fn success(title: impl Into<String>) -> Self {
        Self::new(Kind::Success, title)
    }

    pub fn warning(title: impl Into<String>) -> Self {
        Self::new(Kind::Warning, title)
    }

    pub fn error(title: impl Into<String>) -> Self {
        Self::new(Kind::Error, title)
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    pub fn session(mut self, key: Key) -> Self {
        self.session = Some(key);
        self
    }

    /// Stays this long instead of its kind's usual time.
    pub fn lasting(mut self, lifetime: Duration) -> Self {
        self.lifetime = Some(lifetime);
        self
    }

    /// Closing, and how far: 0 just started, 1 gone. `None` while it's up.
    pub fn leaving(&self, exit: Duration) -> Option<f32> {
        let t = self.leaving?;
        Some(if exit.is_zero() { 1. } else { (t.as_secs_f32() / exit.as_secs_f32()).min(1.) })
    }
}

/// The toasts up now, oldest first; the newest draws at the bottom.
#[derive(Debug, Default)]
pub struct Toasts {
    list: Vec<Toast>,
    next_id: u64,
    /// "+N more" was opened.
    expanded: bool,
}

impl Toasts {
    /// At most this many show; the rest wait behind "+N more".
    pub const VISIBLE: usize = 3;

    /// Puts `toast` up, or merges it into one of the same kind for the same
    /// session: that one counts it, takes the new title and detail, and starts
    /// its time again. Returns the toast up now, for its `count`.
    pub fn push(&mut self, toast: Toast) -> &mut Toast {
        let same = toast.session.is_some().then(|| {
            self.list.iter().position(|t| t.leaving.is_none() && t.kind == toast.kind && t.session == toast.session)
        });
        if let Some(Some(i)) = same {
            let mut old = self.list.remove(i);
            old.count += 1;
            old.title = toast.title;
            old.detail = toast.detail;
            old.action = toast.action.or(old.action);
            old.age = Duration::ZERO;
            self.list.push(old);
        } else {
            self.next_id += 1;
            self.list.push(Toast { id: self.next_id, ..toast });
        }
        self.list.last_mut().unwrap()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Time going by: `dt` since the last tick. Toasts age only while not
    /// `paused` (the pointer is on them, or the window isn't in front); closing
    /// ones play out their `exit` regardless. True if anything changed.
    pub fn tick(&mut self, dt: Duration, paused: bool, exit: Duration) -> bool {
        let before = self.list.len();
        let mut changed = false;
        for t in &mut self.list {
            match &mut t.leaving {
                Some(l) => {
                    *l += dt;
                    changed = true;
                }
                None if !paused => {
                    t.age += dt;
                    if t.lifetime.is_some_and(|life| t.age >= life) {
                        t.leaving = Some(Duration::ZERO);
                        changed = true;
                    }
                }
                None => {}
            }
        }
        self.list.retain(|t| t.leaving.is_none_or(|l| l < exit));
        if self.list.len() <= Self::VISIBLE {
            self.expanded = false;
        }
        changed || self.list.len() != before
    }

    /// Closes one: you clicked its ×, or its button.
    pub fn dismiss(&mut self, id: u64) {
        if let Some(t) = self.list.iter_mut().find(|t| t.id == id && t.leaving.is_none()) {
            t.leaving = Some(Duration::ZERO);
        }
    }

    /// Closes those `f` picks, e.g. the update's once About is open.
    pub fn dismiss_where(&mut self, f: impl Fn(&Toast) -> bool) {
        for t in self.list.iter_mut().filter(|t| t.leaving.is_none() && f(t)) {
            t.leaving = Some(Duration::ZERO);
        }
    }

    /// The session `key` is shown: its own toasts say nothing new.
    pub fn seen(&mut self, key: Key) {
        self.dismiss_where(|t| t.session == Some(key));
    }

    /// The toasts to draw, oldest first, and how many more are behind "+N more".
    pub fn visible(&self) -> (&[Toast], usize) {
        let up = self.list.iter().filter(|t| t.leaving.is_none()).count();
        if self.expanded || up <= Self::VISIBLE {
            return (&self.list, 0);
        }
        // The newest `VISIBLE` that are up, with any closing among them.
        let mut start = self.list.len();
        let mut shown = 0;
        while start > 0 && shown < Self::VISIBLE {
            start -= 1;
            if self.list[start].leaving.is_none() {
                shown += 1;
            }
        }
        (&self.list[start..], up - shown)
    }

    /// "+N more": shows every toast until there are few enough again.
    pub fn expand(&mut self) {
        self.expanded = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXIT: Duration = Duration::from_millis(150);

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    /// Ticks in steps until `total` went by.
    fn run(toasts: &mut Toasts, total: f32, paused: bool) {
        let mut t = 0.;
        while t < total {
            toasts.tick(secs(0.05), paused, EXIT);
            t += 0.05;
        }
    }

    #[test]
    fn info_and_warnings_close_on_their_own_errors_wait() {
        let mut t = Toasts::default();
        t.push(Toast::info("saved"));
        t.push(Toast::warning("careful"));
        t.push(Toast::error("broke"));
        run(&mut t, 5.3, false);
        let titles: Vec<_> = t.visible().0.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["careful", "broke"]);
        run(&mut t, 3., false);
        assert_eq!(t.visible().0.len(), 1);
        run(&mut t, 600., false);
        assert_eq!(t.visible().0[0].title, "broke");
        let id = t.visible().0[0].id;
        t.dismiss(id);
        run(&mut t, 0.2, false);
        assert!(t.is_empty());
    }

    #[test]
    fn time_stops_while_paused_and_closing_plays_out() {
        let mut t = Toasts::default();
        t.push(Toast::success("done"));
        run(&mut t, 4., false);
        run(&mut t, 30., true);
        assert_eq!(t.visible().0.len(), 1, "paused, it waits");
        run(&mut t, 1.05, false);
        let leaving = t.visible().0[0].leaving(EXIT);
        assert!(leaving.is_some_and(|l| l < 1.), "{leaving:?}");
        // Pausing doesn't hold a toast that's already closing.
        run(&mut t, 0.2, true);
        assert!(t.is_empty());
    }

    #[test]
    fn repeats_for_one_session_merge_and_start_again() {
        let mut t = Toasts::default();
        t.push(Toast::success("task finished").session(7).detail("a"));
        run(&mut t, 4., false);
        let merged = t.push(Toast::success("2 tasks finished").session(7).detail("b"));
        assert_eq!((merged.count, merged.title.as_str(), merged.detail.as_str()), (2, "2 tasks finished", "b"));
        // Another session, or another kind, stands apart.
        t.push(Toast::success("task finished").session(8));
        t.push(Toast::error("task failed").session(7));
        assert_eq!(t.visible().0.len(), 3);
        run(&mut t, 4., false);
        assert_eq!(t.visible().0.len(), 3, "the merge started its time again");
    }

    #[test]
    fn three_show_and_the_rest_wait_behind_more() {
        let mut t = Toasts::default();
        for n in 0..5 {
            t.push(Toast::error(format!("e{n}")));
        }
        let (shown, more) = t.visible();
        let titles: Vec<_> = shown.iter().map(|t| t.title.as_str()).collect();
        assert_eq!((titles, more), (vec!["e2", "e3", "e4"], 2));
        t.expand();
        assert_eq!(t.visible(), (&t.list[..], 0));
        // Closing down to three folds it again.
        let ids: Vec<_> = t.list.iter().take(2).map(|t| t.id).collect();
        ids.into_iter().for_each(|id| t.dismiss(id));
        run(&mut t, 0.2, false);
        t.push(Toast::error("e5"));
        assert_eq!(t.visible().1, 1);
    }

    #[test]
    fn showing_a_session_closes_its_toasts() {
        let mut t = Toasts::default();
        t.push(Toast::success("done").session(3).action(Action::GoToSession(3)));
        t.push(Toast::info("other"));
        t.seen(3);
        run(&mut t, 0.2, false);
        assert_eq!(t.visible().0.len(), 1);
        assert_eq!(t.visible().0[0].title, "other");
    }

    #[test]
    fn a_toast_can_last_longer_and_exits_at_once_without_motion() {
        let mut t = Toasts::default();
        t.push(Toast::info("Kynex 9 is available").lasting(secs(8.)));
        run(&mut t, 6., false);
        assert_eq!(t.visible().0.len(), 1);
        run(&mut t, 2.1, false);
        t.tick(Duration::ZERO, false, Duration::ZERO);
        assert!(t.is_empty());
    }
}
