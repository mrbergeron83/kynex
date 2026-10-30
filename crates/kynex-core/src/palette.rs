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
        let q = fuzzy::lower(query.trim());
        let tidy = |text: &str| fuzzy::find_words(text, &q);
        let mut found: Vec<(i64, usize, Vec<usize>)> = if q.is_empty() {
            self.empty(recent)
        } else {
            self.lower
                .iter()
                .enumerate()
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
        Entry { kind, title: title.into(), keywords: keywords.into(), when }
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
}
