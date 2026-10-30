//! The open sessions in one folder.

use crate::session::Session;

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
}

/// Idle sessions left behind that stay open, latest first: switching back to
/// one costs nothing, and its process (if warmed up) is still there (#157).
const KEEP_IDLE: usize = 2;

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

    /// Closes the idle sessions that aren't shown or among the last shown.
    pub fn prune(&mut self) -> Vec<Key> {
        let active = self.current().map(|s| s.uid);
        let recent: Vec<Key> =
            self.shown.iter().rev().filter(|k| Some(**k) != active).take(KEEP_IDLE).copied().collect();
        let mut closed = Vec::new();
        self.sessions.retain(|s| {
            // Closing the process would also stop its background tasks, and
            // drop its browser pairing.
            let keep = Some(s.uid) == active
                || recent.contains(&s.uid)
                || s.busy
                || s.waiting()
                || s.finished
                || !s.queue.is_empty()
                || s.has_tasks()
                || s.browser;
            if !keep {
                closed.push(s.uid);
            }
            keep
        });
        self.active = self.sessions.iter().position(|s| Some(s.uid) == active).unwrap_or(0);
        let open: Vec<Key> = self.sessions.iter().map(|s| s.uid).collect();
        self.shown.retain(|k| open.contains(k));
        closed
    }

    /// Closes an open session that isn't working (its transcript stays).
    /// Returns whether it closed.
    pub fn close(&mut self, key: Key) -> bool {
        let Some(i) = self.sessions.iter().position(|s| s.uid == key && !s.busy) else { return false };
        let shown = self.current().map(|s| s.uid);
        self.sessions.remove(i);
        self.shown.retain(|k| *k != key);
        self.active = self.sessions.iter().position(|s| Some(s.uid) == shown).unwrap_or(0);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::Kind;

    fn session() -> Session {
        Session::new(Kind::Chat, PathBuf::from("."))
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
