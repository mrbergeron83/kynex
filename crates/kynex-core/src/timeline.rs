//! A session's timeline: one entry per turn (the message you sent), with the
//! steps worth finding again under it: files edited, commands run, subagents,
//! plans and prompts, errors. Built from the same items the transcript draws,
//! so resumed sessions get one too.

use std::collections::HashMap;

use crate::fmt;
use crate::session::{Item, PermState};
use crate::tasks::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Edit,
    Command,
    Agent,
    Plan,
    Prompt,
    /// A background task ended.
    Task,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The item it points at in the transcript.
    pub item: usize,
    pub kind: StepKind,
    /// "Edited app.rs ×3", "cargo test", "Allowed Bash".
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// The `User` item that started it.
    pub item: usize,
    /// Its first line, as sent.
    pub prompt: String,
    /// The whole message, to match it with a rewind point.
    pub text: String,
    pub steps: Vec<Step>,
    /// "12.3s · 4 tool calls · edited a.rs", once the turn has ended.
    pub recap: Option<String>,
    /// The turn ended in an error.
    pub failed: bool,
}

/// The turns in `items`, oldest first. Anything before the first message (a
/// resumed session's leftovers) is skipped. Walks every item: renders use
/// [`Turns`], which only walks what changed.
pub fn turns(items: &[Item]) -> Vec<Turn> {
    let mut turns = Vec::new();
    walk(&mut turns, items, 0, &mut Vec::new());
    turns
}

/// A session's turns kept across renders. Items only change in place in two
/// ways that show here: they're appended to (often to the last turn), and a
/// permission prompt gets answered. So `update` reuses the turns while the
/// count is the same and no pending prompt was answered, and otherwise walks
/// again from the last turn's message: the turns before it are settled.
#[derive(Debug, Default)]
pub struct Turns {
    turns: Vec<Turn>,
    /// How many items the turns were built from.
    len: usize,
    /// Permission prompts still pending when last walked, by item.
    pending: Vec<usize>,
}

impl Turns {
    pub fn update(&mut self, items: &[Item]) -> &[Turn] {
        let answered = |i: &usize| !matches!(items.get(*i), Some(Item::Permission { state: PermState::Pending, .. }));
        if items.len() == self.len && !self.pending.iter().any(answered) {
            return &self.turns;
        }
        // Start over when items were dropped or replaced (a rewind, `/clear`),
        // or a prompt was answered in a settled turn.
        let last = self.turns.last().map_or(0, |t| t.item);
        let same_start = |t: &Turn| matches!(items.get(t.item), Some(Item::User { text, .. }) if *text == t.text);
        if items.len() < self.len
            || !self.turns.last().is_none_or(same_start)
            || self.pending.iter().any(|i| *i < last && answered(i))
        {
            *self = Self::default();
        }
        let from = self.turns.pop().map_or(0, |t| t.item);
        self.pending.retain(|i| *i < from);
        walk(&mut self.turns, items, from, &mut self.pending);
        self.len = items.len();
        &self.turns
    }

    /// The turns as of the last `update`.
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }
}

/// Adds the turns in `items[from..]` to `turns`, noting pending prompts.
fn walk(turns: &mut Vec<Turn>, items: &[Item], from: usize, pending: &mut Vec<usize>) {
    for (ix, item) in items.iter().enumerate().skip(from) {
        if let Item::Permission { state: PermState::Pending, .. } = item {
            pending.push(ix);
        }
        if let Item::User { text, .. } = item {
            let prompt = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("(image)").trim();
            turns.push(Turn {
                item: ix,
                prompt: clip(prompt, 100),
                text: text.clone(),
                steps: Vec::new(),
                recap: None,
                failed: false,
            });
            continue;
        }
        let Some(turn) = turns.last_mut() else { continue };
        match item {
            Item::ToolUse { name, summary, .. } => {
                let step = match name.as_str() {
                    "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                        Some((StepKind::Edit, format!("Edited {}", file_name(summary))))
                    }
                    "Bash" | "PowerShell" => Some((StepKind::Command, clip(summary, 80))),
                    "Task" | "Agent" => Some((StepKind::Agent, format!("Agent: {}", clip(summary, 70)))),
                    "ExitPlanMode" => Some((StepKind::Plan, "Proposed a plan".into())),
                    _ => None,
                };
                if let Some((kind, label)) = step {
                    push(&mut turn.steps, Step { item: ix, kind, label });
                }
            }
            Item::ToolResult { is_error: true, .. } => {
                push(&mut turn.steps, Step { item: ix, kind: StepKind::Error, label: "A tool failed".into() })
            }
            Item::Permission { tool, state, .. } => {
                let label = match state {
                    PermState::Pending => format!("Asks to use {tool}"),
                    PermState::Allowed(_) => format!("Allowed {tool}"),
                    PermState::Denied(_) => format!("Denied {tool}"),
                };
                push(&mut turn.steps, Step { item: ix, kind: StepKind::Prompt, label });
            }
            Item::Task(n) if n.status != Status::Event => {
                let (kind, label) = match n.status {
                    Status::Failed => (StepKind::Error, format!("Task failed: {}", clip(&n.summary, 70))),
                    _ => (StepKind::Task, clip(&n.summary, 80)),
                };
                push(&mut turn.steps, Step { item: ix, kind, label });
            }
            Item::Error(e) => {
                push(&mut turn.steps, Step { item: ix, kind: StepKind::Error, label: clip(e, 80) });
                turn.failed = true;
            }
            Item::Result { duration_ms, tools, files, cost, is_error } => {
                turn.recap = Some(fmt::recap(*duration_ms, *tools, files, *cost));
                turn.failed |= *is_error;
            }
            _ => {}
        }
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

/// The turn and step that contain item `item`: where the transcript is.
/// `None` before the first turn.
pub fn locate(turns: &[Turn], item: usize) -> Option<(usize, Option<usize>)> {
    let t = turns.iter().rposition(|t| t.item <= item)?;
    let step = turns[t].steps.iter().rposition(|s| s.item <= item);
    Some((t, step))
}

/// Which turns of a timeline show their steps. Until you choose, the latest
/// turn and the one in view are open and the rest folded; Collapse all /
/// Expand all set every turn, and a turn's chevron sets that one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fold {
    /// Every turn, after Collapse all (`false`) or Expand all (`true`).
    all: Option<bool>,
    /// Turns opened or folded one at a time, by their message's item.
    set: HashMap<usize, bool>,
}

impl Fold {
    /// Whether turn `n` of `turns` is open, with turn `current` in view.
    pub fn is_open(&self, turns: &[Turn], n: usize, current: Option<usize>) -> bool {
        let Some(turn) = turns.get(n) else { return false };
        self.set.get(&turn.item).copied().or(self.all).unwrap_or(n + 1 == turns.len() || current == Some(n))
    }

    /// Opens turn `n` if it's folded, folds it if it's open.
    pub fn toggle(&mut self, turns: &[Turn], n: usize, current: Option<usize>) {
        if let Some(turn) = turns.get(n) {
            let open = self.is_open(turns, n, current);
            self.set.insert(turn.item, !open);
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

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn user(text: &str) -> Item {
        Item::User { text: text.into(), images: Vec::new() }
    }

    fn tool(name: &str, summary: &str) -> Item {
        Item::ToolUse { id: String::new(), name: name.into(), summary: summary.into(), input: String::new() }
    }

    fn result(is_error: bool) -> Item {
        Item::ToolResult { id: String::new(), content: String::new(), images: Vec::new(), is_error }
    }

    fn sample() -> Vec<Item> {
        vec![
            Item::Assistant("left over from before".into()),
            user("Fix the parser\nand add a test"),
            Item::Assistant("Looking.".into()),
            tool("Read", "C:/x/src/parse.rs"),
            result(false),
            tool("Edit", "C:/x/src/parse.rs"),
            result(false),
            tool("Edit", "C:/x/src/parse.rs"),
            result(false),
            tool("Bash", "cargo test"),
            result(true),
            Item::Result { cost: 0.05, duration_ms: 12_300, is_error: false, tools: 4, files: vec!["parse.rs".into()] },
            user("Now ship it"),
            Item::Permission {
                request_id: String::new(),
                tool: "Bash".into(),
                input: Value::Null,
                suggestions: Value::Null,
                description: String::new(),
                state: PermState::Denied("no".into()),
            },
            Item::Error("Request failed".into()),
        ]
    }

    #[test]
    fn one_turn_per_message_with_its_notable_steps() {
        let turns = turns(&sample());
        assert_eq!(turns.len(), 2);
        let t = &turns[0];
        assert_eq!((t.item, t.prompt.as_str()), (1, "Fix the parser"));
        assert_eq!(t.text, "Fix the parser\nand add a test");
        let labels: Vec<_> = t.steps.iter().map(|s| (s.kind, s.label.as_str(), s.item)).collect();
        assert_eq!(
            labels,
            [
                (StepKind::Edit, "Edited parse.rs ×2", 5),
                (StepKind::Command, "cargo test", 9),
                (StepKind::Error, "A tool failed", 10),
            ],
            "reads are left out, repeats fold, and a step points at its first item"
        );
        assert_eq!(t.recap.as_deref(), Some(fmt::recap(12_300, 4, &["parse.rs".into()], 0.05).as_str()));
        assert!(!t.failed);

        let t = &turns[1];
        assert_eq!(t.steps[0].label, "Denied Bash");
        assert!(t.failed && t.recap.is_none());
    }

    fn permission(state: PermState) -> Item {
        Item::Permission {
            request_id: String::new(),
            tool: "Bash".into(),
            input: Value::Null,
            suggestions: Value::Null,
            description: String::new(),
            state,
        }
    }

    #[test]
    fn the_cache_keeps_up_with_appends_answers_and_rewinds() {
        let mut cache = Turns::default();
        let mut items = sample();
        assert_eq!(cache.update(&items), turns(&items));

        items.extend([permission(PermState::Pending), tool("Edit", "a.rs")]);
        assert_eq!(cache.update(&items), turns(&items), "appended to the last turn");
        items.push(user("Third"));
        assert_eq!(cache.update(&items), turns(&items), "a new turn");

        // A prompt answered in place, in a settled turn and then in the last one.
        let pending = items.len() - 3;
        items[pending] = permission(PermState::Allowed("ok".into()));
        assert_eq!(cache.update(&items), turns(&items));
        items.push(permission(PermState::Pending));
        cache.update(&items);
        *items.last_mut().unwrap() = permission(PermState::Denied("cancelled".into()));
        assert_eq!(cache.update(&items)[2].steps[0].label, "Denied Bash");

        // Rewound and regrown past where it was, with other messages.
        items.truncate(12);
        items.extend([
            user("Instead"),
            tool("Bash", "ls"),
            tool("Edit", "b.rs"),
            result(false),
            user("More"),
            user("x"),
            tool("Bash", "y"),
            result(true),
        ]);
        assert!(items.len() > cache.len);
        assert_eq!(cache.update(&items), turns(&items));
        items.clear();
        assert!(cache.update(&items).is_empty());
    }

    #[test]
    fn finds_the_turn_and_step_in_view() {
        let turns = turns(&sample());
        assert_eq!(locate(&turns, 0), None);
        assert_eq!(locate(&turns, 2), Some((0, None)));
        assert_eq!(locate(&turns, 7), Some((0, Some(0))));
        assert_eq!(locate(&turns, 9), Some((0, Some(1))));
        assert_eq!(locate(&turns, 13), Some((1, Some(0))));
    }

    #[test]
    fn labels_are_short() {
        let long = "x".repeat(300);
        let turns = turns(&[user(&long), tool("Bash", &long)]);
        assert!(turns[0].prompt.chars().count() <= 101);
        assert!(turns[0].steps[0].label.chars().count() <= 81);
        assert_eq!(super::turns(&[user("")])[0].prompt, "(image)");
    }

    #[test]
    fn folds_older_turns_until_told_otherwise() {
        let mut items = sample();
        items.extend([user("Third"), tool("Bash", "ls")]);
        let turns = turns(&items);
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
