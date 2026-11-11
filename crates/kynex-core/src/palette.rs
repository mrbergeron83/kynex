//! The command palette (Ctrl+K): sessions of every open project, the projects,
//! and workspace actions in one list, ranked as you type. The app says what
//! the entries are and what picking one does; this only orders them.

use std::time::SystemTime;

use crate::fuzzy;

/// Sessions an empty query shows, newest first, before anything else.
const RECENT_SESSIONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Project,
    Action,
}

/// What an open session is doing, for the palette's filters (mirrors the app's lamp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    NeedsYou,
    Done,
}

impl State {
    /// The word shown beside a session in that state.
    pub fn word(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::NeedsYou => "needs you",
            State::Done => "done",
        }
    }
}

/// Which sessions the palette lists. A past session that isn't open has no state, so only
/// `All` shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    NeedsYou,
    Working,
    Idle,
    Done,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::NeedsYou, Filter::Working, Filter::Idle, Filter::Done];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::NeedsYou => "Needs you",
            Filter::Working => "Working",
            Filter::Idle => "Idle",
            Filter::Done => "Done",
        }
    }

    /// Whether a session in `state` passes.
    pub fn matches(self, state: Option<State>) -> bool {
        matches!(
            (self, state),
            (Filter::All, _)
                | (Filter::NeedsYou, Some(State::NeedsYou))
                | (Filter::Working, Some(State::Working))
                | (Filter::Idle, Some(State::Idle))
                | (Filter::Done, Some(State::Done))
        )
    }

    /// The next (`1`) or previous (`-1`) filter, wrapping round.
    pub fn step(self, by: isize) -> Filter {
        let at = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

/// Where a session sits in the list: its project's and workspace's places, and the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// The project's place among the open ones.
    pub project: usize,
    /// The workspace's place in its project: the main tree first, then by label.
    pub workspace: usize,
    /// The header: see [`section_label`].
    pub label: String,
}

/// A section's header: `project › workspace`, or just the project when it has one workspace.
pub fn section_label(project: &str, workspace: &str, workspaces: usize) -> String {
    if workspaces < 2 { project.to_string() } else { format!("{project} › {workspace}") }
}

/// The workspaces' places, main tree first then by label: `places[i]` is where
/// `(main, label)` pair `i` goes.
pub fn workspace_places(workspaces: &[(bool, &str)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..workspaces.len()).collect();
    order.sort_by(|&a, &b| {
        let (ma, la) = workspaces[a];
        let (mb, lb) = workspaces[b];
        mb.cmp(&ma).then_with(|| la.to_lowercase().cmp(&lb.to_lowercase()))
    });
    let mut places = vec![0; workspaces.len()];
    for (place, i) in order.into_iter().enumerate() {
        places[i] = place;
    }
    places
}

/// One thing the palette can list.
#[derive(Clone, Debug)]
pub struct Entry {
    pub kind: Kind,
    /// What's shown and matched first: a session's title, a project's name,
    /// an action's label.
    pub title: String,
    /// Matched too, for less: a session's project, an action's other words
    /// (`dark`, `appearance`).
    pub keywords: String,
    /// When a session was last active, to put newer ones first.
    pub when: Option<SystemTime>,
    /// A session's project and workspace.
    pub section: Option<Section>,
    /// An open session's state.
    pub state: Option<State>,
}

/// One result: an index into the entries, and the byte offsets in its title of
/// the characters the query matched, to bold them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub positions: Vec<usize>,
}

/// The entries, with their titles and keywords lowercased once.
#[derive(Default)]
pub struct Palette {
    pub entries: Vec<Entry>,
    lower: Vec<(String, String)>,
}

impl Palette {
    pub fn new(entries: Vec<Entry>) -> Self {
        let lower = entries.iter().map(|e| (fuzzy::lower(&e.title), fuzzy::lower(&e.keywords))).collect();
        Self { entries, lower }
    }

    /// The best `limit` entries for `query`, best first.
    ///
    /// With nothing typed: the newest sessions, then the actions named in
    /// `recent` (most recent first), the projects, and the other actions.
    /// Otherwise each run of matched letters must start a word (letters picked
    /// from all over a long title are noise); a match in the title beats one
    /// only in the keywords; ties go to the newer session, then to the order
    /// the entries came in.
    pub fn matching(&self, query: &str, recent: &[&str], limit: usize) -> Vec<Match> {
        self.matching_in(query, recent, limit, Filter::All)
    }

    /// How many sessions each of [`Filter::ALL`] lets through.
    pub fn counts(&self) -> [usize; 5] {
        let mut out = [0; 5];
        for e in self.entries.iter().filter(|e| e.kind == Kind::Session) {
            for (n, f) in out.iter_mut().zip(Filter::ALL) {
                *n += usize::from(f.matches(e.state));
            }
        }
        out
    }

    /// [`Self::matching`] under a filter: with one other than `All`, only the sessions
    /// in that state, all of them when nothing is typed.
    pub fn matching_in(&self, query: &str, recent: &[&str], limit: usize, filter: Filter) -> Vec<Match> {
        let q = fuzzy::lower(query.trim());
        let tidy = |text: &str| fuzzy::find_words(text, &q);
        let keep = |i: usize| {
            filter == Filter::All || (self.entries[i].kind == Kind::Session && filter.matches(self.entries[i].state))
        };
        let mut found: Vec<(i64, usize, Vec<usize>)> = if q.is_empty() && filter != Filter::All {
            (0..self.entries.len()).filter(|&i| keep(i)).map(|i| (10_000, i, Vec::new())).collect()
        } else if q.is_empty() {
            self.empty(recent)
        } else {
            self.lower
                .iter()
                .enumerate()
                .filter(|(i, _)| keep(*i))
                .filter_map(|(i, (title, keywords))| {
                    if let Some(p) = tidy(title) {
                        // A short title the query covers more of reads as the closer match.
                        let mut score = fuzzy::score(title, &p) + 20 - title.len() as i64 / 8;
                        if p.first() == Some(&0) {
                            score += 10;
                        }
                        Some((score, i, p))
                    } else {
                        let p = tidy(keywords)?;
                        Some((fuzzy::score(keywords, &p), i, Vec::new()))
                    }
                })
                .collect()
        };
        let when = |i: usize| self.entries[i].when;
        found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| when(b.1).cmp(&when(a.1))).then_with(|| a.1.cmp(&b.1)));
        found.into_iter().take(limit).map(|(_, index, positions)| Match { index, positions }).collect()
    }

    /// `found` grouped by kind, the kind with the best match first; sessions go by project,
    /// then workspace, and the ranking holds within each group.
    pub fn grouped(&self, mut found: Vec<Match>) -> Vec<Match> {
        let mut kinds: Vec<Kind> = Vec::new();
        for m in &found {
            let k = self.entries[m.index].kind;
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
        found.sort_by_key(|m| {
            let e = &self.entries[m.index];
            let at = e.section.as_ref().map_or((0, 0), |s| (s.project, s.workspace));
            (kinds.iter().position(|k| *k == e.kind), at)
        });
        found
    }

    /// The order for an empty query, as scores: sessions (newest first, by the
    /// tie-break), recent actions, projects, the rest.
    fn empty(&self, recent: &[&str]) -> Vec<(i64, usize, Vec<usize>)> {
        let mut sessions: Vec<usize> =
            (0..self.entries.len()).filter(|&i| self.entries[i].kind == Kind::Session).collect();
        sessions.sort_by(|&a, &b| self.entries[b].when.cmp(&self.entries[a].when));
        sessions.truncate(RECENT_SESSIONS);
        let mut out: Vec<(i64, usize, Vec<usize>)> = sessions.into_iter().map(|i| (10_000, i, Vec::new())).collect();
        for (i, e) in self.entries.iter().enumerate() {
            let score = match e.kind {
                Kind::Session => continue,
                Kind::Action => recent.iter().position(|r| *r == e.title).map_or(0, |r| 5_000 - r as i64),
                Kind::Project => 1,
            };
            out.push((score, i, Vec::new()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn entry(kind: Kind, title: &str, keywords: &str, age: u64) -> Entry {
        let when = (kind == Kind::Session).then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age));
        Entry { kind, title: title.into(), keywords: keywords.into(), when, section: None, state: None }
    }

    fn palette() -> Palette {
        Palette::new(vec![
            entry(Kind::Session, "Fix the terminal resize", "kynex", 50),
            entry(Kind::Session, "Add file mentions", "kynex", 10),
            entry(Kind::Session, "Deploy notes", "otto", 99),
            entry(Kind::Project, "kynex", "C:/repos/kynex", 0),
            entry(Kind::Action, "Toggle terminal", "panel shell", 0),
            entry(Kind::Action, "New session", "", 0),
            entry(Kind::Action, "Theme: Kynex Dark", "appearance colours", 0),
        ])
    }

    fn titles(p: &Palette, q: &str, recent: &[&str]) -> Vec<String> {
        p.matching(q, recent, 20).into_iter().map(|m| p.entries[m.index].title.clone()).collect()
    }

    #[test]
    fn empty_query_shows_recent_sessions_then_recent_actions() {
        let p = palette();
        assert_eq!(
            titles(&p, "", &["New session"]),
            [
                "Add file mentions",
                "Fix the terminal resize",
                "Deploy notes",
                "New session",
                "kynex",
                "Toggle terminal",
                "Theme: Kynex Dark"
            ]
        );
    }

    #[test]
    fn titles_beat_keywords() {
        let p = palette();
        assert_eq!(titles(&p, "term", &[])[..2], ["Toggle terminal", "Fix the terminal resize"]);
        // Only the keywords: the sessions in that project.
        assert_eq!(titles(&p, "otto", &[]), ["Deploy notes"]);
        assert_eq!(titles(&p, "dark", &[]), ["Theme: Kynex Dark"]);
        assert_eq!(titles(&p, "colours", &[]), ["Theme: Kynex Dark"]);
        assert!(titles(&p, "zzz", &[]).is_empty());
        // Letters scattered mid-word don't count.
        assert_eq!(titles(&p, "term", &[]).len(), 2);
        assert!(titles(&p, "eoe", &[]).is_empty());
    }

    #[test]
    fn newer_sessions_win_ties() {
        let p = Palette::new(vec![entry(Kind::Session, "notes", "", 90), entry(Kind::Session, "notes", "", 5)]);
        assert_eq!(p.matching("notes", &[], 5)[0].index, 1);
    }

    #[test]
    fn marks_matched_characters() {
        let p = palette();
        let m = &p.matching("ns", &[], 1)[0];
        assert_eq!(p.entries[m.index].title, "New session");
        assert_eq!(m.positions, [0, 4]);
        assert!(p.matching("panel", &[], 1)[0].positions.is_empty());
    }

    fn session(title: &str, project: usize, workspace: usize, state: Option<State>, age: u64) -> Entry {
        let mut e = entry(Kind::Session, title, "", age);
        e.section = Some(Section { project, workspace, label: format!("p{project}") });
        e.state = state;
        e
    }

    #[test]
    fn sessions_group_by_project_then_workspace() {
        let p = Palette::new(vec![
            session("a", 1, 0, None, 1),
            session("b", 0, 1, None, 2),
            session("c", 0, 0, None, 3),
            entry(Kind::Action, "New session", "", 0),
            session("d", 0, 0, None, 9),
        ]);
        let found = p.grouped(p.matching("", &[], 20));
        let titles: Vec<_> = found.iter().map(|m| p.entries[m.index].title.as_str()).collect();
        // Newest first inside a group, actions last.
        assert_eq!(titles, ["c", "d", "b", "a", "New session"]);
    }

    #[test]
    fn filters_pick_sessions_by_state() {
        let p = Palette::new(vec![
            session("one", 0, 0, Some(State::NeedsYou), 1),
            session("two", 0, 0, Some(State::Working), 2),
            session("three", 0, 0, None, 3),
            entry(Kind::Action, "New session", "", 0),
        ]);
        assert_eq!(p.counts(), [3, 1, 1, 0, 0]);
        let names = |f| -> Vec<_> {
            p.matching_in("", &[], 20, f).into_iter().map(|m| p.entries[m.index].title.clone()).collect()
        };
        assert_eq!(names(Filter::NeedsYou), ["one"]);
        assert_eq!(names(Filter::Working), ["two"]);
        assert!(names(Filter::Done).is_empty());
        assert_eq!(p.matching_in("t", &[], 20, Filter::NeedsYou).len(), 0);
        assert_eq!(names(Filter::All).len(), 4);
        assert!(!Filter::Idle.matches(None) && Filter::All.matches(None));
    }

    #[test]
    fn filters_cycle_and_workspaces_sort() {
        assert_eq!(Filter::All.step(-1), Filter::Done);
        assert_eq!(Filter::Done.step(1), Filter::All);
        assert_eq!(workspace_places(&[(false, "b"), (false, "A"), (true, "zz")]), [2, 1, 0]);
        assert_eq!(section_label("kynex", "x", 1), "kynex");
        assert_eq!(section_label("kynex", "x", 2), "kynex › x");
    }
}
