//! The open sessions in one folder, and how long an idle one keeps its process.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session::Session;

/// The "Sleep idle sessions" setting: how long a session that
/// [can sleep](Session::can_sleep) keeps its `claude` process (and the MCP
/// servers under it) once nothing happens in it. A sleeping session keeps its
/// transcript and starts again when shown, typed in or sent a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleepAfter {
    Minutes15,
    #[default]
    Minutes30,
    Hour1,
    Never,
}

impl SleepAfter {
    pub const ALL: [Self; 4] = [Self::Minutes15, Self::Minutes30, Self::Hour1, Self::Never];

    pub fn label(self) -> &'static str {
        match self {
            Self::Minutes15 => "After 15 minutes idle",
            Self::Minutes30 => "After 30 minutes idle",
            Self::Hour1 => "After an hour idle",
            Self::Never => "Never",
        }
    }

    pub fn duration(self) -> Option<Duration> {
        const MIN: u64 = 60;
        match self {
            Self::Minutes15 => Some(Duration::from_secs(15 * MIN)),
            Self::Minutes30 => Some(Duration::from_secs(30 * MIN)),
            Self::Hour1 => Some(Duration::from_secs(60 * MIN)),
            Self::Never => None,
        }
    }
}

/// A session's [`Session::uid`]: transcripts, expanded tools and prompts follow it.
pub type Key = u64;

/// The open conversations in one folder (a project, or the chat folder). Each
/// runs its own `claude` process, so one can keep working while you're in another.
#[derive(Default)]
pub struct Live {
    pub sessions: Vec<Session>,
    /// The one shown, and talked to from the composer.
    pub active: usize,
    /// Sessions in the order they were last shown, latest last.
    shown: Vec<Key>,
    /// The sessions on screen, one per visible pane. The focused one is [`Self::current`].
    visible: Vec<Key>,
}

/// How many `claude` processes run at once: every visible pane holds one, so this also
/// bounds how many panes can be up.
pub const MAX_RUNNING: usize = 8;

/// Idle sessions left behind that stay open, latest first: switching back to
/// one costs nothing, and its process (if warmed up) is still there (#157).
const KEEP_IDLE: usize = 2;

/// The rule behind [`Live::room_for`]: a session with a process already always has room.
fn check_room(has_process: bool, running: usize, cap: usize) -> Result<(), String> {
    if has_process || running < cap {
        return Ok(());
    }
    Err(format!("{cap} sessions are running already. Close a pane or wait for one to finish to start another."))
}

impl Live {
    /// Just `s`, shown.
    pub fn of(s: Session) -> Self {
        let mut live = Self::default();
        live.show(s);
        live
    }

    pub fn current(&self) -> Option<&Session> {
        self.sessions.get(self.active)
    }

    pub fn current_mut(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.active)
    }

    pub fn busy(&self) -> bool {
        self.sessions.iter().any(|s| s.busy)
    }

    pub fn waiting(&self) -> bool {
        self.sessions.iter().any(Session::waiting)
    }

    /// A session here finished a turn you haven't seen.
    pub fn finished(&self) -> bool {
        self.sessions.iter().any(|s| s.finished)
    }

    /// Sets which sessions are on screen (one per pane). They are never pruned.
    /// Keys that aren't open here are dropped.
    pub fn set_visible(&mut self, keys: &[Key]) {
        self.visible = keys.iter().copied().filter(|k| self.sessions.iter().any(|s| s.uid == *k)).collect();
    }

    pub fn visible(&self) -> &[Key] {
        &self.visible
    }

    pub fn is_visible(&self, key: Key) -> bool {
        self.visible.contains(&key)
    }

    /// Focuses the open session `key` (the one the composer talks to). False if it isn't open.
    pub fn focus(&mut self, key: Key) -> bool {
        self.show_where(|s| s.uid == key)
    }

    /// How many sessions have a `claude` process, or are starting one.
    pub fn running(&self) -> usize {
        self.sessions.iter().filter(|s| s.is_running()).count()
    }

    /// Whether the session `key` may start a process: it has one already, or there is
    /// room under [`MAX_RUNNING`]. The error is fit to show as it is.
    pub fn room_for(&self, key: Key) -> Result<(), String> {
        let has = self.sessions.iter().any(|s| s.uid == key && s.is_running());
        check_room(has, self.running(), MAX_RUNNING)
    }

    /// Shows the first open session that matches, if any.
    pub fn show_where(&mut self, f: impl Fn(&Session) -> bool) -> bool {
        match self.sessions.iter().position(f) {
            Some(i) => {
                self.active = i;
                self.mark_shown();
                true
            }
            None => false,
        }
    }

    /// Shows `s`. Sessions left behind keep running while busy, waiting on
    /// you, holding queued messages or driving a browser, and the last [`KEEP_IDLE`] shown stay
    /// open too; other idle ones close (their process with them) and resume
    /// from the transcript if picked again. Returns the uids that closed.
    pub fn show(&mut self, s: Session) -> Vec<Key> {
        self.sessions.push(s);
        self.active = self.sessions.len() - 1;
        self.mark_shown();
        self.prune()
    }

    fn mark_shown(&mut self) {
        let Some(uid) = self.current().map(|s| s.uid) else { return };
        self.shown.retain(|k| *k != uid);
        self.shown.push(uid);
    }

    /// Closes the idle sessions that aren't shown or among the last shown. New sessions
    /// don't count among the last shown, and are left to [`Self::close_unused`].
    pub fn prune(&mut self) -> Vec<Key> {
        let active = self.current().map(|s| s.uid);
        let used: Vec<Key> = self.sessions.iter().filter(|s| !s.is_new()).map(|s| s.uid).collect();
        let recent: Vec<Key> = self
            .shown
            .iter()
            .rev()
            .filter(|k| Some(**k) != active && used.contains(k))
            .take(KEEP_IDLE)
            .copied()
            .collect();
        self.close_unless(|this, s| {
            Some(s.uid) == active
                || s.is_new()
                || recent.contains(&s.uid)
                || this.visible.contains(&s.uid)
                || s.finished
                || !s.can_sleep()
        })
    }

    /// Closes the new sessions (nothing sent, nothing to resume) that were left: not
    /// shown, in no pane, and not in `keep` (one holding a draft, say). There's nothing to
    /// come back to in them, so they don't wait their turn like [`Self::prune`]'s.
    pub fn close_unused(&mut self, keep: impl Fn(&Session) -> bool) -> Vec<Key> {
        let active = self.current().map(|s| s.uid);
        self.close_unless(|this, s| !s.is_new() || Some(s.uid) == active || this.visible.contains(&s.uid) || keep(s))
    }

    /// Closes every session `keep` refuses, keeping the shown one shown.
    fn close_unless(&mut self, keep: impl Fn(&Self, &Session) -> bool) -> Vec<Key> {
        let active = self.current().map(|s| s.uid);
        let gone: Vec<Key> = self.sessions.iter().filter(|s| !keep(self, s)).map(|s| s.uid).collect();
        if gone.is_empty() {
            return gone;
        }
        self.sessions.retain(|s| !gone.contains(&s.uid));
        self.active = self.sessions.iter().position(|s| Some(s.uid) == active).unwrap_or(0);
        self.shown.retain(|k| !gone.contains(k));
        self.visible.retain(|k| !gone.contains(k));
        gone
    }

    /// Closes every session `f` picks, working or not (their processes die with
    /// them; transcripts stay). Returns the uids that closed.
    pub fn close_where(&mut self, f: impl Fn(&Session) -> bool) -> Vec<Key> {
        let shown = self.current().map(|s| s.uid);
        let mut closed = Vec::new();
        self.sessions.retain(|s| {
            let gone = f(s);
            if gone {
                closed.push(s.uid);
            }
            !gone
        });
        self.shown.retain(|k| !closed.contains(k));
        self.visible.retain(|k| !closed.contains(k));
        self.active = self.sessions.iter().position(|s| Some(s.uid) == shown).unwrap_or(0);
        closed
    }

    /// Closes an open session that isn't working (its transcript stays).
    /// Returns whether it closed.
    pub fn close(&mut self, key: Key) -> bool {
        let Some(i) = self.sessions.iter().position(|s| s.uid == key && !s.busy) else { return false };
        let shown = self.current().map(|s| s.uid);
        self.sessions.remove(i);
        self.shown.retain(|k| *k != key);
        self.visible.retain(|k| *k != key);
        self.active = self.sessions.iter().position(|s| Some(s.uid) == shown).unwrap_or(0);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::{Item, Kind};

    /// A session that has been used: it has a conversation to come back to.
    fn session() -> Session {
        let mut s = fresh();
        s.items.push(Item::Assistant("hi".into()));
        s
    }

    /// A new session: nothing sent yet.
    fn fresh() -> Session {
        Session::new(Kind::Chat, PathBuf::from("."))
    }

    #[test]
    fn a_new_session_left_closes_and_takes_no_place_among_the_last_shown() {
        let mut live = Live::default();
        let (a, b, n) = (session(), session(), fresh());
        let (a_uid, b_uid, n_uid) = (a.uid, b.uid, n.uid);
        live.show(a);
        live.show(b);
        assert!(live.show(n).is_empty(), "the new one is shown");
        // Back to the first: the new one left behind stays until close_unused, and
        // doesn't push the second out of the last shown.
        assert!(live.show_where(|s| s.uid == a_uid));
        assert!(live.prune().is_empty());
        assert_eq!(live.close_unused(|_| false), [n_uid]);
        assert_eq!(live.sessions.iter().map(|s| s.uid).collect::<Vec<_>>(), [a_uid, b_uid]);
        assert_eq!(live.current().map(|s| s.uid), Some(a_uid));
        assert!(live.close_unused(|_| false).is_empty());
    }

    #[test]
    fn close_unused_spares_the_shown_one_panes_and_kept_ones() {
        let mut live = Live::default();
        let (p, d, n, shown) = (fresh(), fresh(), fresh(), fresh());
        let (p_uid, d_uid, n_uid, shown_uid) = (p.uid, d.uid, n.uid, shown.uid);
        for s in [p, d, n, shown] {
            live.show(s);
        }
        live.set_visible(&[p_uid]);
        // The pane's, the one with a draft and the shown one stay.
        assert_eq!(live.close_unused(|s| s.uid == d_uid), [n_uid]);
        assert_eq!(live.current().map(|s| s.uid), Some(shown_uid));
        // Out of its pane, the pane's goes too.
        live.set_visible(&[]);
        assert_eq!(live.close_unused(|s| s.uid == d_uid), [p_uid]);
    }

    #[test]
    fn close_where_closes_even_a_working_session_and_keeps_the_one_shown() {
        let mut live = Live::default();
        let mut s: Vec<Session> = (0..3).map(|_| session()).collect();
        s[0].busy = true;
        let uids: Vec<Key> = s.iter().map(|s| s.uid).collect();
        for s in s {
            live.show(s);
        }
        // The first is working, the second idle, the third shown.
        let closed = live.close_where(|s| s.uid == uids[0]);
        assert_eq!(closed, [uids[0]]);
        assert_eq!(live.current().map(|s| s.uid), Some(uids[2]));
        // Closing the shown one falls back to the first left.
        assert_eq!(live.close_where(|s| s.uid == uids[2]), [uids[2]]);
        assert_eq!(live.current().map(|s| s.uid), Some(uids[1]));
        assert!(live.close_where(|_| false).is_empty());
    }

    #[test]
    fn showing_a_session_keeps_the_last_ones_and_busy_ones() {
        let mut live = Live::default();
        let mut s: Vec<Session> = (0..5).map(|_| session()).collect();
        let uids: Vec<Key> = s.iter().map(|s| s.uid).collect();
        s[0].busy = true;
        for s in s.drain(..4) {
            assert!(live.show(s).is_empty(), "busy, or among the last {KEEP_IDLE} shown");
        }
        // The fifth leaves the second (idle, no longer recent) behind.
        assert_eq!(live.show(s.remove(0)), [uids[1]]);
        assert_eq!(live.sessions.iter().map(|s| s.uid).collect::<Vec<_>>(), [uids[0], uids[2], uids[3], uids[4]]);
        assert_eq!(live.current().map(|s| s.uid), Some(uids[4]));
        // Going back to the third makes it the latest again; then the fourth goes.
        assert!(live.show_where(|s| s.uid == uids[2]));
        assert!(live.prune().is_empty());
        assert_eq!(live.show(session()), [uids[3]]);
    }

    #[test]
    fn queued_messages_keep_a_session_open() {
        let mut live = Live::default();
        let mut a = session();
        a.queue.push("later".into(), Vec::new());
        let a_uid = a.uid;
        live.show(a);
        assert!(live.show(session()).is_empty());
        assert_eq!(live.sessions[0].uid, a_uid);
    }

    #[test]
    fn background_tasks_keep_a_session_open() {
        let mut live = Live::default();
        let mut a = session();
        a.tasks.apply(&serde_json::json!({ "subtype": "task_started", "task_id": "b1", "task_type": "local_bash" }));
        let a_uid = a.uid;
        live.show(a);
        assert!(live.show(session()).is_empty());
        assert_eq!(live.sessions[0].uid, a_uid);
    }

    #[test]
    fn a_session_driving_a_browser_stays_open() {
        let mut live = Live::default();
        let mut a = session();
        a.browser = true;
        let a_uid = a.uid;
        live.show(a);
        for _ in 0..KEEP_IDLE + 1 {
            assert!(live.show(session()).is_empty());
        }
        assert_eq!(live.sessions[0].uid, a_uid);
    }

    #[test]
    fn visible_sessions_are_never_pruned() {
        let mut live = Live::default();
        let first: Vec<Session> = (0..3).map(|_| session()).collect();
        let uids: Vec<Key> = first.iter().map(|s| s.uid).collect();
        for s in first {
            assert!(live.show(s).is_empty());
        }
        // Idle and about to fall out of the last shown: the first would go, but it is on screen.
        live.set_visible(&[uids[0], uids[2]]);
        assert!(live.is_visible(uids[0]));
        assert!(live.prune().is_empty());
        assert!(live.show(session()).iter().all(|k| *k != uids[0]));
        assert!(live.sessions.iter().any(|s| s.uid == uids[0]));
        assert_eq!(live.visible(), [uids[0], uids[2]]);
        // Off screen, it goes at the next prune.
        live.set_visible(&[uids[2]]);
        assert!(live.prune().contains(&uids[0]));
        assert!(!live.is_visible(uids[0]));
    }

    #[test]
    fn set_visible_drops_unknown_keys_and_focus_picks_the_current() {
        let mut live = Live::default();
        let (a, b) = (session(), session());
        let (ak, bk) = (a.uid, b.uid);
        live.show(a);
        live.show(b);
        live.set_visible(&[ak, bk, 12345]);
        assert_eq!(live.visible(), [ak, bk]);
        assert!(live.focus(ak));
        assert_eq!(live.current().map(|s| s.uid), Some(ak));
        assert!(!live.focus(12345));
        assert!(live.close_where(|s| s.uid == bk) == [bk]);
        assert_eq!(live.visible(), [ak]);
    }

    #[test]
    fn the_cap_refuses_a_new_process_with_a_message() {
        assert!(check_room(false, MAX_RUNNING - 1, MAX_RUNNING).is_ok());
        let err = check_room(false, MAX_RUNNING, MAX_RUNNING).unwrap_err();
        assert!(err.contains(&MAX_RUNNING.to_string()) && err.contains("Close a pane"));
        assert!(check_room(true, MAX_RUNNING, MAX_RUNNING).is_ok(), "one that runs already keeps its process");
    }

    #[test]
    fn room_for_is_open_with_no_processes() {
        let mut live = Live::default();
        let s = session();
        let key = s.uid;
        live.show(s);
        assert_eq!(live.running(), 0);
        assert!(live.room_for(key).is_ok());
    }

    #[test]
    fn sleep_after_durations() {
        assert_eq!(SleepAfter::default(), SleepAfter::Minutes30);
        assert_eq!(SleepAfter::Minutes15.duration(), Some(Duration::from_secs(900)));
        assert_eq!(SleepAfter::Never.duration(), None);
    }

    #[test]
    fn close_keeps_the_shown_session_shown() {
        let mut live = Live::default();
        let mut a = session();
        a.finished = true;
        let a_uid = a.uid;
        live.show(a);
        let b = session();
        let b_uid = b.uid;
        live.show(b);
        assert!(live.close(a_uid));
        assert_eq!(live.current().map(|s| s.uid), Some(b_uid));
    }
}
