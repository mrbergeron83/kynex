//! What a session is up to, asked every frame: whether it's new, running, waiting
//! on you; its counts, todos and provider.

use super::*;

impl Session {
    /// Who answers this session.
    pub fn provider(&self) -> &crate::provider::Launch {
        &self.provider
    }

    /// Picks who answers this session. Only a new session can: once it has
    /// any activity (a message, a process, a conversation to resume) its
    /// provider is fixed, and switching means starting a new session.
    pub fn set_provider(&mut self, provider: crate::provider::Launch) -> Result<(), String> {
        if provider == self.provider {
            return Ok(());
        }
        if !self.is_new() {
            return Err(format!(
                "This session runs on {}; start a new session to use {}.",
                self.provider.provider().label(),
                provider.provider().label()
            ));
        }
        // A process started early, before anything was sent, starts again on it;
        // the id it reported names no conversation, so it goes too.
        if self.is_running() {
            self.kill();
        }
        self.session_id = None;
        self.provider = provider;
        Ok(())
    }

    /// A new session with no activity: not reopened, nothing sent, nothing in
    /// its transcript. A process started early to save time doesn't count, nor
    /// the id that process may already have reported.
    pub fn is_new(&self) -> bool {
        !self.reopened && !self.talked && self.items.is_empty()
    }

    /// Puts a reopened session back on the provider it ran on, until it has sent
    /// something here. Its conversation is the same; a process started early
    /// on another provider (before the OpenRouter key loaded) starts again on
    /// this one. One that has sent something keeps what it has.
    pub fn restore_provider(&mut self, provider: crate::provider::Launch) -> Result<(), String> {
        if provider == self.provider {
            return Ok(());
        }
        if self.talked {
            return Err(format!("This session already runs on {}.", self.provider.provider().label()));
        }
        if self.is_running() {
            self.kill();
        }
        self.provider = provider;
        Ok(())
    }

    /// Reopened (or forked) and nothing sent here yet: its provider is still
    /// the one recorded for it, and can be put back ([`Self::restore_provider`]).
    pub fn awaits_restore(&self) -> bool {
        !self.is_new() && !self.talked
    }

    /// Its process is running, or starting ([`Self::starting`]).
    pub fn is_running(&self) -> bool {
        self.child.is_some() || self.starting.is_some()
    }

    /// Its process is being started, off the UI thread; what's written to it
    /// meanwhile goes out once it's up.
    pub fn starting(&self) -> bool {
        self.starting.is_some()
    }

    /// Whether a permission prompt or question is waiting on the user.
    pub fn waiting(&self) -> bool {
        let key = (self.items.len(), self.perm_gen);
        if let Some((n, g, answer)) = self.waiting_cache.get()
            && (n, g) == key
        {
            return answer;
        }
        let answer = self.items.iter().any(|i| matches!(i, Item::Permission { state: PermState::Pending, .. }));
        self.waiting_cache.set(Some((key.0, key.1, answer)));
        answer
    }

    /// How many turns it has finished and how many subagents it has called, for
    /// the header; scanned again only when its items change.
    pub fn counts(&self) -> (usize, usize) {
        let key = (self.items.len(), self.revision);
        if let Some((n, r, turns, agents)) = self.counts_cache.get()
            && (n, r) == key
        {
            return (turns, agents);
        }
        let turns = self.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
        let agents = crate::claude::subagents::count(&self.items);
        self.counts_cache.set(Some((key.0, key.1, turns, agents)));
        (turns, agents)
    }

    /// Whether its last turn ended asking you to do something in the browser
    /// ([`crate::claude::mcp::handoff`]); worked out again only when its items change.
    pub fn wants_browser(&self) -> bool {
        let key = (self.items.len(), self.revision);
        if let Some((n, r, answer)) = self.browser_cache.get()
            && (n, r) == key
        {
            return answer;
        }
        let answer = crate::claude::mcp::handoff(&self.items).is_some();
        self.browser_cache.set(Some((key.0, key.1, answer)));
        answer
    }

    /// Claude's todo list as it stands ([`crate::claude::todos`]); read again only when its
    /// items change.
    pub fn todos(&self) -> TodoList {
        let key = (self.items.len(), self.revision);
        if let Some((n, r, list)) = &*self.todos_cache.borrow()
            && (*n, *r) == key
        {
            return list.clone();
        }
        let list = std::sync::Arc::new(crate::claude::todos::of(&self.items));
        *self.todos_cache.borrow_mut() = Some((key.0, key.1, list.clone()));
        list
    }

    /// Whether this session wants the user: a turn ended unseen, or a prompt waits.
    pub fn wants_you(&self) -> bool {
        self.finished || self.waiting()
    }

    /// Where its Kynex settings are read from: [`Self::settings_dir`], else its own folder.
    pub fn settings_folder(&self) -> &std::path::Path {
        self.settings_dir.as_deref().unwrap_or(&self.cwd)
    }

    /// The conversation a new process continues, if there is one to continue.
    pub(super) fn resume_id(&self) -> Option<&str> {
        self.fork_from.as_deref().or(self.session_id.as_deref().filter(|_| self.conversation))
    }
}
