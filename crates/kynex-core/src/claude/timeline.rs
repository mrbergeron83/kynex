//! The chat's timeline: one entry per turn (the message you sent), with the
//! steps worth finding again under it: files edited, commands run, subagents,
//! plans and prompts, errors. Built from the chat's transcript rows, described
//! to it as [`Row`]s, so resumed sessions get one too.

use std::collections::HashMap;

use crate::fmt;

/// What a transcript row is, as far as the timeline cares.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row<'a> {
    /// A message you sent: a new turn.
    UserPrompt(&'a str),
    /// An `Edit` / `Write` / … call on `path`.
    Edit { path: &'a str, is_error: bool },
    /// Any other tool call, with its one-line summary.
    ToolCall { name: &'a str, summary: &'a str, is_error: bool },
    /// The record a permission prompt left: "Bash cargo test", allowed or not.
    PermissionNote { title: &'a str, allowed: bool },
    /// A notice in the transcript; errors are steps.
    Notice { text: &'a str, is_error: bool },
    /// The rule that ends a turn.
    TurnEnd { duration_ms: u64, tools: usize, files: &'a [String], cost_usd: Option<f64>, is_error: bool },
    /// Prose, thinking, dividers: nothing to list.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Edit,
    Command,
    Agent,
    Plan,
    Prompt,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The row it points at in the transcript.
    pub row: usize,
    pub kind: StepKind,
    /// "Edited app.rs ×3", "cargo test", "Allowed Bash".
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// The `UserPrompt` row that started it.
    pub row: usize,
    /// Its first line, as sent.
    pub prompt: String,
    /// The whole message, to tell it apart once rows move.
    pub text: String,
    pub steps: Vec<Step>,
    /// "12.3s · 4 tool calls · edited a.rs", once the turn has ended.
    pub recap: Option<String>,
    /// The turn ended in an error.
    pub failed: bool,
}

/// The turns in rows `0..len` (`row(i)` describes row `i`), oldest first.
/// Anything before the first message is skipped. Walks every row: renders use
/// [`Turns`], which only walks the last turn again.
pub fn turns<'a>(len: usize, row: impl Fn(usize) -> Row<'a>) -> Vec<Turn> {
    let mut turns = Vec::new();
    walk(&mut turns, len, 0, &row);
    turns
}

/// A chat's turns kept across renders. Rows are appended (mostly to the last
/// turn) and change in place while their call runs, so `update` keeps the
/// turns before the last one and walks again from its message: the turns
/// before it are settled.
#[derive(Debug, Default)]
pub struct Turns {
    turns: Vec<Turn>,
    /// How many rows the turns were built from.
    len: usize,
}

impl Turns {
    pub fn update<'a>(&mut self, len: usize, row: impl Fn(usize) -> Row<'a>) -> &[Turn] {
        // Start over when rows were dropped or moved (`/clear`, older history
        // put in front): the last turn's message is no longer where it was.
        let same_start = |t: &Turn| t.row < len && matches!(row(t.row), Row::UserPrompt(text) if text == t.text);
        if len < self.len || !self.turns.last().is_none_or(same_start) {
            *self = Self::default();
        }
        let from = self.turns.pop().map_or(0, |t| t.row);
        walk(&mut self.turns, len, from, &row);
        self.len = len;
        &self.turns
    }

    /// The turns as of the last `update`.
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }
}

/// Adds the turns in rows `from..len` to `turns`.
fn walk<'a>(turns: &mut Vec<Turn>, len: usize, from: usize, row: &impl Fn(usize) -> Row<'a>) {
    for ix in from..len {
        let r = row(ix);
        if let Row::UserPrompt(text) = r {
            let prompt = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("(image)").trim();
            turns.push(Turn {
                row: ix,
                prompt: clip(prompt, 100),
                text: text.to_string(),
                steps: Vec::new(),
                recap: None,
                failed: false,
            });
            continue;
        }
        let Some(turn) = turns.last_mut() else { continue };
        match r {
            Row::Edit { path, is_error } => {
                push(
                    &mut turn.steps,
                    Step { row: ix, kind: StepKind::Edit, label: format!("Edited {}", fmt::file_name(path)) },
                );
                if is_error {
                    push(&mut turn.steps, tool_failed(ix));
                }
            }
            Row::ToolCall { name, summary, is_error } => {
                let step = match name {
                    "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                        Some((StepKind::Edit, format!("Edited {}", fmt::file_name(summary))))
                    }
                    "Bash" | "PowerShell" => Some((StepKind::Command, clip(summary, 80))),
                    "Task" | "Agent" => Some((StepKind::Agent, format!("Agent: {}", clip(summary, 70)))),
                    "ExitPlanMode" => Some((StepKind::Plan, "Proposed a plan".into())),
                    _ => None,
                };
                if let Some((kind, label)) = step {
                    push(&mut turn.steps, Step { row: ix, kind, label });
                }
                if is_error {
                    push(&mut turn.steps, tool_failed(ix));
                }
            }
            Row::PermissionNote { title, allowed } => {
                // "Bash cargo test" → "Allowed Bash"; a plan is approved or sent back.
                let label = match (title, allowed) {
                    ("Plan", true) => "Approved the plan".to_string(),
                    ("Plan", false) => "Sent the plan back".to_string(),
                    _ => {
                        let tool = title.split_whitespace().next().unwrap_or(title);
                        format!("{} {tool}", if allowed { "Allowed" } else { "Denied" })
                    }
                };
                push(&mut turn.steps, Step { row: ix, kind: StepKind::Prompt, label });
            }
            Row::Notice { text, is_error: true } => {
                push(&mut turn.steps, Step { row: ix, kind: StepKind::Error, label: clip(text, 80) });
                turn.failed = true;
            }
            Row::TurnEnd { duration_ms, tools, files, cost_usd, is_error } => {
                turn.recap = Some(recap(duration_ms, tools, files, cost_usd));
                turn.failed |= is_error;
            }
            Row::UserPrompt(_) | Row::Notice { .. } | Row::Other => {}
        }
    }
}

fn tool_failed(row: usize) -> Step {
    Step { row, kind: StepKind::Error, label: "A tool failed".into() }
}

/// [`fmt::recap`], leaving the cost out when there's none (OpenRouter).
fn recap(duration_ms: u64, tools: usize, files: &[String], cost: Option<f64>) -> String {
    let full = fmt::recap(duration_ms, tools, files, cost.unwrap_or(0.));
    match cost {
        Some(_) => full,
        None => full.rsplit_once(" · ").map_or(full.clone(), |(rest, _)| rest.to_string()),
    }
}

/// Adds a step, folding a repeat of the one before ("Edited a.rs" twice
/// becomes "Edited a.rs ×2") so a busy turn stays short.
fn push(steps: &mut Vec<Step>, step: Step) {
    if let Some(last) = steps.last_mut()
        && last.kind == step.kind
    {
        let (base, n) = match last.label.rsplit_once(" ×") {
            Some((base, n)) if n.parse::<usize>().is_ok() => (base.to_string(), n.parse::<usize>().unwrap_or(1)),
            _ => (last.label.clone(), 1),
        };
        if base == step.label {
            last.label = format!("{base} ×{}", n + 1);
            return;
        }
    }
    steps.push(step);
}

/// The turn and step that contain row `row`: where the transcript is.
/// `None` before the first turn.
pub fn locate(turns: &[Turn], row: usize) -> Option<(usize, Option<usize>)> {
    let t = turns.iter().rposition(|t| t.row <= row)?;
    let step = turns[t].steps.iter().rposition(|s| s.row <= row);
    Some((t, step))
}

/// Which turns of a timeline show their steps. Until you choose, the latest
/// turn and the one in view are open and the rest folded; Collapse all /
/// Expand all set every turn, and a turn's chevron sets that one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fold {
    /// Every turn, after Collapse all (`false`) or Expand all (`true`).
    all: Option<bool>,
    /// Turns opened or folded one at a time, by their message's row.
    set: HashMap<usize, bool>,
}

impl Fold {
    /// Whether turn `n` of `turns` is open, with turn `current` in view.
    pub fn is_open(&self, turns: &[Turn], n: usize, current: Option<usize>) -> bool {
        let Some(turn) = turns.get(n) else { return false };
        self.set.get(&turn.row).copied().or(self.all).unwrap_or(n + 1 == turns.len() || current == Some(n))
    }

    /// Opens turn `n` if it's folded, folds it if it's open.
    pub fn toggle(&mut self, turns: &[Turn], n: usize, current: Option<usize>) {
        if let Some(turn) = turns.get(n) {
            let open = self.is_open(turns, n, current);
            self.set.insert(turn.row, !open);
        }
    }

    /// Opens (or folds) every turn, forgetting the ones set one at a time.
    pub fn set_all(&mut self, open: bool) {
        self.all = Some(open);
        self.set.clear();
    }

    /// Whether any turn with steps is open: Collapse all has something to do.
    pub fn any_open(&self, turns: &[Turn], current: Option<usize>) -> bool {
        (0..turns.len()).any(|n| !turns[n].steps.is_empty() && self.is_open(turns, n, current))
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owned rows, described as [`Row`]s on demand like the app's.
    #[derive(Clone)]
    enum R {
        User(&'static str),
        Edit(&'static str, bool),
        Tool(&'static str, &'static str, bool),
        Perm(&'static str, bool),
        Notice(&'static str, bool),
        End(Vec<String>, Option<f64>, bool),
        Prose,
    }

    fn describe(rows: &[R], i: usize) -> Row<'_> {
        match &rows[i] {
            R::User(t) => Row::UserPrompt(t),
            R::Edit(p, e) => Row::Edit { path: p, is_error: *e },
            R::Tool(n, s, e) => Row::ToolCall { name: n, summary: s, is_error: *e },
            R::Perm(t, a) => Row::PermissionNote { title: t, allowed: *a },
            R::Notice(t, e) => Row::Notice { text: t, is_error: *e },
            R::End(files, cost, e) => {
                Row::TurnEnd { duration_ms: 12_300, tools: 4, files, cost_usd: *cost, is_error: *e }
            }
            R::Prose => Row::Other,
        }
    }

    fn all(rows: &[R]) -> Vec<Turn> {
        turns(rows.len(), |i| describe(rows, i))
    }

    fn sample() -> Vec<R> {
        vec![
            R::Prose,
            R::User("Fix the parser\nand add a test"),
            R::Prose,
            R::Tool("Read", "src/parse.rs", false),
            R::Edit("C:/x/src/parse.rs", false),
            R::Edit("C:/x/src/parse.rs", false),
            R::Tool("Bash", "cargo test", true),
            R::End(vec!["parse.rs".into()], Some(0.05), false),
            R::User("Now ship it"),
            R::Perm("Bash git push", false),
            R::Notice("Request failed", true),
        ]
    }

    #[test]
    fn one_turn_per_message_with_its_notable_steps() {
        let turns = all(&sample());
        assert_eq!(turns.len(), 2);
        let t = &turns[0];
        assert_eq!((t.row, t.prompt.as_str()), (1, "Fix the parser"));
        assert_eq!(t.text, "Fix the parser\nand add a test");
        let labels: Vec<_> = t.steps.iter().map(|s| (s.kind, s.label.as_str(), s.row)).collect();
        assert_eq!(
            labels,
            [
                (StepKind::Edit, "Edited parse.rs ×2", 4),
                (StepKind::Command, "cargo test", 6),
                (StepKind::Error, "A tool failed", 6),
            ],
            "reads are left out, repeats fold, and a step points at its first row"
        );
        assert_eq!(t.recap.as_deref(), Some(fmt::recap(12_300, 4, &["parse.rs".into()], 0.05).as_str()));
        assert!(!t.failed);

        let t = &turns[1];
        assert_eq!(t.steps[0].label, "Denied Bash");
        assert_eq!(t.steps[1].label, "Request failed");
        assert!(t.failed && t.recap.is_none());
    }

    #[test]
    fn subagents_plans_and_a_recap_without_cost() {
        let rows = vec![
            R::User("Go"),
            R::Tool("Agent", "Explore the repo", false),
            R::Tool("ExitPlanMode", "", false),
            R::Perm("Plan", true),
            R::End(Vec::new(), None, true),
        ];
        let t = &all(&rows)[0];
        let labels: Vec<_> = t.steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Agent: Explore the repo", "Proposed a plan", "Approved the plan"]);
        assert!(!t.recap.as_deref().unwrap().contains('$'));
        assert!(t.failed);
    }

    #[test]
    fn the_cache_keeps_up_with_appends_changes_and_clears() {
        let mut cache = Turns::default();
        let mut rows = sample();
        let update = |c: &mut Turns, rows: &[R]| c.update(rows.len(), |i| describe(rows, i)).to_vec();
        assert_eq!(update(&mut cache, &rows), all(&rows));

        rows.push(R::Edit("a.rs", false));
        assert_eq!(update(&mut cache, &rows), all(&rows), "appended to the last turn");
        rows.push(R::User("Third"));
        rows.push(R::Tool("Bash", "ls", false));
        assert_eq!(update(&mut cache, &rows), all(&rows), "a new turn");
        // A call in the last turn failed in place.
        *rows.last_mut().unwrap() = R::Tool("Bash", "ls", true);
        assert_eq!(update(&mut cache, &rows), all(&rows));

        // Older history put in front: rows move.
        rows.insert(0, R::User("Earlier"));
        assert_eq!(update(&mut cache, &rows), all(&rows));
        rows.clear();
        assert!(update(&mut cache, &rows).is_empty());
        rows.push(R::User("Fresh"));
        assert_eq!(update(&mut cache, &rows), all(&rows));
    }

    #[test]
    fn finds_the_turn_and_step_in_view() {
        let turns = all(&sample());
        assert_eq!(locate(&turns, 0), None);
        assert_eq!(locate(&turns, 2), Some((0, None)));
        assert_eq!(locate(&turns, 5), Some((0, Some(0))));
        assert_eq!(locate(&turns, 6), Some((0, Some(2))));
        assert_eq!(locate(&turns, 9), Some((1, Some(0))));
    }

    #[test]
    fn labels_are_short() {
        let long: &'static str = "x".repeat(300).leak();
        let turns = all(&[R::User(long), R::Tool("Bash", long, false)]);
        assert!(turns[0].prompt.chars().count() <= 101);
        assert!(turns[0].steps[0].label.chars().count() <= 81);
        assert_eq!(all(&[R::User("")])[0].prompt, "(image)");
    }

    #[test]
    fn folds_older_turns_until_told_otherwise() {
        let mut rows = sample();
        rows.extend([R::User("Third"), R::Tool("Bash", "ls", false)]);
        let turns = all(&rows);
        let mut fold = Fold::default();
        let open = |f: &Fold, current| (0..3).map(|n| f.is_open(&turns, n, current)).collect::<Vec<_>>();
        assert_eq!(open(&fold, None), [false, false, true], "only the latest at first");
        assert_eq!(open(&fold, Some(0)), [true, false, true], "and the one in view");

        fold.toggle(&turns, 0, Some(0));
        fold.toggle(&turns, 1, Some(0));
        assert_eq!(open(&fold, None), [false, true, true], "a chevron's choice sticks as you scroll");

        fold.set_all(false);
        assert_eq!(open(&fold, Some(0)), [false; 3]);
        assert!(!fold.any_open(&turns, Some(0)));
        fold.toggle(&turns, 2, None);
        assert!(fold.any_open(&turns, None));
        fold.set_all(true);
        assert_eq!(open(&fold, None), [true; 3]);
    }
}
