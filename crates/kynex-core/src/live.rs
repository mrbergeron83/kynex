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
}

impl Live {
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
                true
            }
            None => false,
        }
    }

    /// Shows `s`. Sessions left behind keep running while busy, waiting on
    /// you or holding queued messages; idle ones close (their process with them) and resume from the
    /// transcript if picked again. Returns the uids that closed.
    pub fn show(&mut self, s: Session) -> Vec<Key> {
        self.sessions.push(s);
        self.active = self.sessions.len() - 1;
        self.prune()
    }

    /// Closes the idle sessions that aren't shown.
    pub fn prune(&mut self) -> Vec<Key> {
        let active = self.current().map(|s| s.uid);
        let mut closed = Vec::new();
        self.sessions.retain(|s| {
            // Closing the process would also stop its background tasks.
            let keep =
                Some(s.uid) == active || s.busy || s.waiting() || s.finished || !s.queue.is_empty() || s.has_tasks();
            if !keep {
                closed.push(s.uid);
            }
            keep
        });
        self.active = self.sessions.iter().position(|s| Some(s.uid) == active).unwrap_or(0);
        closed
    }

    /// Closes an open session that isn't working (its transcript stays).
    /// Returns whether it closed.
    pub fn close(&mut self, key: Key) -> bool {
        let Some(i) = self.sessions.iter().position(|s| s.uid == key && !s.busy) else { return false };
        let shown = self.current().map(|s| s.uid);
        self.sessions.remove(i);
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
    fn showing_a_session_closes_idle_ones_but_keeps_busy_ones() {
        let mut live = Live::default();
        let (a, mut b) = (session(), session());
        let (a_uid, b_uid) = (a.uid, b.uid);
        b.busy = true;
        live.show(a);
        assert!(live.show(b).contains(&a_uid));
        let c = session();
        let c_uid = c.uid;
        assert!(live.show(c).is_empty(), "busy b stays open");
        assert_eq!(live.sessions.iter().map(|s| s.uid).collect::<Vec<_>>(), [b_uid, c_uid]);
        assert_eq!(live.current().map(|s| s.uid), Some(c_uid));
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
