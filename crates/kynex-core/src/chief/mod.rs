//! The chief of staff (#211): a Chat-tab session, on Haiku by default, that
//! takes your goals, dispatches the work to one *chief worker* per project
//! and reports back. Workers are project sessions that run without permission
//! prompts, each in its own worktree, and may use subagents for their work
//! ([`contract`] has the rules between the two).
//!
//! The chief's tools come from Kynex itself: a small MCP server over HTTP on
//! localhost ([`Server`]). Tool calls go to the app as [`Request`]s, since the
//! sessions live there; the app answers each one. When a worker ends a turn
//! the app sends the chief a message ([`turn_notice`]), so the chief never polls.

mod contract;
mod server;

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

pub use contract::{BRANCH, Report, Status, create_project, existing_project, parse_report, risky, worker_args};
pub use server::{Request, Server, launch_args_for, set_launch_args};

use crate::fmt::tokens;
use crate::session::Item;
use crate::spent::Spent;

/// The model the chief runs on unless picked otherwise: it only plans and dispatches.
pub const DEFAULT_MODEL: &str = "haiku";

/// What Kynex's own messages to the chief start with, so they're told apart from yours.
pub const NOTICE: &str = "[Kynex]";

/// The MCP server's name, so its tools are `mcp__kynex__…`.
const NAME: &str = "kynex";

/// The longest worker reply passed on to the chief; the rest is cut.
const REPLY_LIMIT: usize = 4000;

const SYSTEM_PROMPT: &str = "\
You are the user's chief of staff, inside Kynex. You don't write code or read files yourself: \
you plan, dispatch work and report back.

Each project has one chief worker: a Claude Code session working in that project, on the \
project's own model, in its own git worktree (branch chief-worker), with no permission prompts. \
A worker can run its own subagents, so give one worker a whole goal rather than splitting it \
across messages.

Tools:
- list_projects: the projects you can dispatch to.
- open_project(path): opens an existing folder as a project. create_project(path): makes a new \
folder with git init and opens it. Both take full paths.
- dispatch(project, prompt): sends work to a project's worker, starting it if needed. A busy \
worker queues the message and takes it when its turn ends.
- status: every worker, what it's doing and what it has used.
- result(project): a worker's latest report.
- stop(project): stops a worker's current turn.

Write each dispatch as complete, self-contained instructions: the goal, constraints and what done \
means. Workers end every turn with a report (STATUS: done | blocked | failed, SUMMARY, CHANGED, \
and QUESTION when blocked), and Kynex sends it to you in a message starting with \"[Kynex]\". \
Then tell the user briefly what happened and dispatch follow-ups if the goal needs them.

Workers can't ask the user anything: a blocked worker's question comes to you. Answer it with \
dispatch when the user's goal already decides it; otherwise ask the user, then pass their answer \
on with dispatch. Kynex also tells you when a worker runs a risky command (a push, a force push, \
deleting branches or files, publishing): mention those to the user.

Don't call status in a loop; wait for Kynex's messages. Keep your replies short.";

/// A tool call from the chief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Projects,
    Status,
    Dispatch { project: String, prompt: String },
    Result { project: String },
    Stop { project: String },
    OpenProject { path: String },
    CreateProject { path: String },
}

impl Call {
    /// The call named `name` with `args`, or why it can't be made.
    pub fn parse(name: &str, args: &Value) -> Result<Self, String> {
        let text = |k: &str| {
            args[k]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .ok_or(format!("`{k}` is missing"))
        };
        Ok(match name {
            "list_projects" => Self::Projects,
            "status" => Self::Status,
            "dispatch" => Self::Dispatch { project: text("project")?, prompt: text("prompt")? },
            "result" => Self::Result { project: text("project")? },
            "stop" => Self::Stop { project: text("project")? },
            "open_project" => Self::OpenProject { path: text("path")? },
            "create_project" => Self::CreateProject { path: text("path")? },
            _ => return Err(format!("no tool called {name}")),
        })
    }
}

/// The tools, as `tools/list` gives them.
fn tools() -> Value {
    let project = json!({ "type": "string", "description": "The project's folder name (or full path), as list_projects gives it" });
    let path = json!({ "type": "string", "description": "The folder's full path" });
    let none = json!({ "type": "object", "properties": {} });
    let one = |k: &str, v: &Value| json!({ "type": "object", "required": [k], "properties": { k: v } });
    json!([
        { "name": "list_projects", "description": "The projects open in Kynex, which work can be dispatched to.",
          "inputSchema": none },
        { "name": "open_project", "description": "Opens an existing folder as a project in Kynex, without switching the user's tab.",
          "inputSchema": one("path", &path) },
        { "name": "create_project", "description": "Makes a new, empty project folder (with git init) and opens it in Kynex.",
          "inputSchema": one("path", &path) },
        { "name": "status", "description": "Every chief worker: its project, model, what it's doing, how long, and the tokens it has used.",
          "inputSchema": none },
        { "name": "dispatch", "description": "Sends work to a project's chief worker, starting it if needed. A busy worker queues it.",
          "inputSchema": { "type": "object", "required": ["project", "prompt"], "properties": {
              "project": project,
              "prompt": { "type": "string", "description": "Complete, self-contained instructions for the worker" } } } },
        { "name": "result", "description": "A project's chief worker's latest report.",
          "inputSchema": one("project", &project) },
        { "name": "stop", "description": "Stops a project's chief worker's current turn, and what it had queued.",
          "inputSchema": one("project", &project) },
    ])
}

/// The project `name` picks among `projects`: its full path, or its folder
/// name (any case).
pub fn find_project<'a>(projects: &'a [PathBuf], name: &str) -> Result<&'a PathBuf, String> {
    let name = name.trim();
    if let Some(p) = projects.iter().find(|p| p.as_os_str() == name) {
        return Ok(p);
    }
    let matches: Vec<&PathBuf> =
        projects.iter().filter(|p| crate::paths::project_name(p).eq_ignore_ascii_case(name)).collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no project called {name}; list_projects has them, open_project adds one")),
        _ => Err(format!(
            "more than one project is called {name}; use the full path: {}",
            matches.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// `list_projects`' answer.
pub fn projects_text(projects: &[PathBuf]) -> String {
    if projects.is_empty() {
        return "No projects are open in Kynex yet: open_project or create_project adds one.".into();
    }
    projects
        .iter()
        .map(|p| format!("- {} ({})", crate::paths::project_name(p), p.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a chief worker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    Working,
    /// A prompt in its session waits on the user.
    NeedsYou,
    /// Its last report asks a question.
    Blocked,
    Idle,
    /// Its process isn't running; it resumes on the next dispatch.
    #[default]
    Asleep,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsYou => "needs the user",
            Self::Blocked => "blocked on a question",
            Self::Idle => "idle",
            Self::Asleep => "asleep",
        }
    }
}

/// A chief worker as the chief and the reporter see it.
#[derive(Debug, Clone, Default)]
pub struct Worker {
    pub path: PathBuf,
    pub model: String,
    pub state: State,
    /// How long its current turn has run.
    pub turn: Option<Duration>,
    pub spent: Spent,
    /// Subagents and background tasks running under it.
    pub agents: usize,
    /// Messages waiting for its turn to end.
    pub queued: usize,
    /// What it asks, when blocked.
    pub question: Option<String>,
    /// Risky commands it ran this turn.
    pub warnings: Vec<String>,
}

impl Worker {
    pub fn name(&self) -> String {
        crate::paths::project_name(&self.path)
    }

    /// One line: `kynex: working 3m 12s, opus, 2 subagents, 1 queued, 1.2M tokens`.
    pub fn line(&self) -> String {
        let mut parts = vec![self.state.label().to_string()];
        if let (State::Working, Some(t)) = (self.state, self.turn) {
            parts[0] = format!("working {}", crate::fmt::elapsed(t));
        }
        if !self.model.is_empty() {
            parts.push(self.model.clone());
        }
        if self.agents > 0 {
            parts.push(format!("{} subagent{}", self.agents, if self.agents == 1 { "" } else { "s" }));
        }
        if self.queued > 0 {
            parts.push(format!("{} queued", self.queued));
        }
        parts.push(format!("{} tokens", tokens(self.spent.tokens())));
        format!("{}: {}", self.name(), parts.join(", "))
    }
}

/// `status`' answer.
pub fn status_text(workers: &[Worker]) -> String {
    if workers.is_empty() {
        return "No chief workers yet. dispatch starts one.".into();
    }
    let total = workers.iter().fold(Spent::default(), |a, w| a.plus(w.spent));
    let mut lines: Vec<String> = workers.iter().map(|w| format!("- {}", w.line())).collect();
    lines.push(format!("All workers: {} tokens", tokens(total.tokens())));
    lines.join("\n")
}

/// The latest reply in `items`: the assistant text since the last message sent.
pub fn last_reply(items: &[Item]) -> Option<String> {
    let turn = items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |i| i + 1);
    // The report is the end of the turn; earlier text is narration between steps.
    items[turn..].iter().rev().find_map(|i| match i {
        Item::Assistant(t) if !t.trim().is_empty() => Some(clip(t.trim())),
        _ => None,
    })
}

fn clip(text: &str) -> String {
    if text.len() <= REPLY_LIMIT {
        return text.to_string();
    }
    let mut end = REPLY_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… cut; the rest is in the worker's session]", &text[..end])
}

/// How a worker's turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Done,
    Failed,
}

/// A turn a worker ended, for [`turn_notice`].
#[derive(Debug, Clone, PartialEq)]
pub struct TurnEnd {
    pub ending: Ending,
    /// Its last reply: the report.
    pub reply: Option<String>,
    /// The files it edited.
    pub files: Vec<String>,
    /// The last error it hit, if any.
    pub error: Option<String>,
}

/// How many turns ended in `items`, and the ones after the first `reported`.
/// A worker with queued messages starts the next as soon as one ends, so turns
/// are counted by their results, not by the session going idle. (A log read
/// back has no results, so history never counts.)
pub fn turns_ended(items: &[Item], reported: usize) -> (usize, Vec<TurnEnd>) {
    let results: Vec<usize> =
        items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Result { .. })).map(|(i, _)| i).collect();
    let new = results
        .iter()
        .skip(reported)
        .filter_map(|&i| {
            let Item::Result { is_error, files, .. } = &items[i] else { return None };
            let start = items[..i].iter().rposition(|x| matches!(x, Item::User { .. })).unwrap_or(0);
            let error = items[start..i].iter().rev().find_map(|x| match x {
                Item::Error(e) => Some(e.trim().to_string()),
                _ => None,
            });
            Some(TurnEnd {
                ending: if *is_error { Ending::Failed } else { Ending::Done },
                reply: last_reply(&items[..i]),
                files: files.clone(),
                error,
            })
        })
        .collect();
    (results.len(), new)
}

/// A turn that ended with no result: the process stopped.
pub fn cut_short(items: &[Item]) -> TurnEnd {
    let start = items.iter().rposition(|x| matches!(x, Item::User { .. })).unwrap_or(0);
    let error = items[start..].iter().rev().find_map(|x| match x {
        Item::Error(e) => Some(e.trim().to_string()),
        _ => None,
    });
    TurnEnd { ending: Ending::Failed, reply: last_reply(items), files: Vec::new(), error }
}

/// The message the chief gets when worker `w` ends a turn.
pub fn turn_notice(w: &Worker, t: &TurnEnd) -> String {
    let name = w.name();
    let report = t.reply.as_deref().map(parse_report).unwrap_or_default();
    let mut lines = vec![match (t.ending, report.status) {
        (_, Some(Status::Blocked)) => format!(
            "{NOTICE} The {name} worker is blocked and asks: {}\nAnswer with dispatch({name}, …) if the \
             user's goal decides it; otherwise ask the user and pass their answer on.",
            report.question.as_deref().unwrap_or("(see its report)")
        ),
        (Ending::Failed, _) | (_, Some(Status::Failed)) => format!("{NOTICE} The {name} worker's turn failed."),
        _ => format!("{NOTICE} The {name} worker finished its turn."),
    }];
    if !t.files.is_empty() {
        let names: Vec<&str> = t.files.iter().map(|f| crate::fmt::file_name(f)).collect();
        lines.push(format!("Files changed: {}.", names.join(", ")));
    }
    if !w.warnings.is_empty() {
        lines.push(format!("Warnings: it {}.", w.warnings.join("; it ")));
    }
    if let (Ending::Failed, Some(e)) = (t.ending, &t.error) {
        lines.push(format!("Error: {e}"));
    }
    lines.push(format!("Used so far: {} tokens.", tokens(w.spent.tokens())));
    match &t.reply {
        Some(r) => format!("{}\n\nIts report:\n{r}", lines.join("\n")),
        None => format!("{}\n\nIt wrote no report.", lines.join("\n")),
    }
}

/// The message the chief gets when a prompt in worker `w`'s session waits on the user.
pub fn waiting_notice(w: &Worker) -> String {
    format!("{NOTICE} The {} worker is waiting for the user to answer a prompt in its session.", w.name())
}

/// The message the chief gets after a restart cut short the workers in `names`.
pub fn cut_off_notice(names: &[String]) -> String {
    format!(
        "{NOTICE} Kynex restarted while the {} worker{} working. {} turn{} cut short, and anything queued \
         was dropped: dispatch again to carry on.",
        names.join(", "),
        if names.len() == 1 { " was" } else { "s were" },
        if names.len() == 1 { "Its" } else { "Their" },
        if names.len() == 1 { " was" } else { "s were" },
    )
}

/// `result`'s answer.
pub fn result_text(w: &Worker, reply: Option<&str>) -> String {
    match reply {
        Some(r) => format!("{}\n\n{r}", w.line()),
        None => format!("{}\n\nNo report yet.", w.line()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(state: State) -> Worker {
        Worker {
            path: PathBuf::from("/r/kynex"),
            model: "opus".into(),
            state,
            turn: Some(Duration::from_secs(192)),
            spent: Spent { input: 1_000, output: 200_000, cache_read: 1_000_000, ..Default::default() },
            agents: 2,
            queued: 1,
            ..Default::default()
        }
    }

    #[test]
    fn parses_calls() {
        assert_eq!(Call::parse("status", &Value::Null), Ok(Call::Status));
        assert_eq!(
            Call::parse("dispatch", &json!({ "project": " kynex ", "prompt": "fix #1" })),
            Ok(Call::Dispatch { project: "kynex".into(), prompt: "fix #1".into() })
        );
        assert_eq!(Call::parse("dispatch", &json!({ "project": "kynex" })), Err("`prompt` is missing".into()));
        assert_eq!(
            Call::parse("create_project", &json!({ "path": "/r/new" })),
            Ok(Call::CreateProject { path: "/r/new".into() })
        );
        assert!(Call::parse("rm", &json!({})).is_err());
    }

    #[test]
    fn every_tool_parses() {
        for t in tools().as_array().unwrap() {
            let args = json!({ "project": "p", "prompt": "x", "path": "/p" });
            assert!(Call::parse(t["name"].as_str().unwrap(), &args).is_ok(), "{}", t["name"]);
        }
    }

    #[test]
    fn finds_projects_by_name_or_path() {
        let projects = [PathBuf::from("/r/kynex"), PathBuf::from("/w/Azure"), PathBuf::from("/x/azure")];
        assert_eq!(find_project(&projects, "Kynex"), Ok(&projects[0]));
        assert_eq!(find_project(&projects, "/w/Azure"), Ok(&projects[1]));
        assert!(find_project(&projects, "azure").unwrap_err().contains("more than one"));
        assert!(find_project(&projects, "otto").is_err());
    }

    #[test]
    fn describes_workers() {
        let w = worker(State::Working);
        assert_eq!(w.line(), "kynex: working 3m 12s, opus, 2 subagents, 1 queued, 1.2M tokens");
        assert!(status_text(&[w.clone(), w]).ends_with("All workers: 2.4M tokens"));
        assert!(status_text(&[]).contains("No chief workers"));
    }

    #[test]
    fn the_last_reply_is_the_end_of_the_turn() {
        let items = vec![
            Item::Assistant("old".into()),
            Item::User { text: "go".into(), images: vec![] },
            Item::Assistant("Looking…".into()),
            Item::Assistant("Done: fixed it.".into()),
        ];
        assert_eq!(last_reply(&items).as_deref(), Some("Done: fixed it."));
        assert_eq!(last_reply(&items[..2]), None);
        let long = "é".repeat(REPLY_LIMIT);
        assert!(last_reply(&[Item::Assistant(long)]).unwrap().ends_with("worker's session]"));
    }

    fn result(is_error: bool, files: &[&str]) -> Item {
        Item::Result {
            cost: 0.0,
            duration_ms: 0,
            is_error,
            tools: 0,
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn every_turn_that_ended_counts() {
        let user = || Item::User { text: "go".into(), images: vec![] };
        let items = vec![
            user(),
            Item::Assistant("first done".into()),
            result(false, &["/r/a.rs"]),
            user(),
            Item::Error("API overloaded".into()),
            Item::Assistant("second broke".into()),
            result(true, &[]),
            user(),
        ];
        let (count, ended) = turns_ended(&items, 0);
        assert_eq!(count, 2);
        assert_eq!(ended[0].ending, Ending::Done);
        assert_eq!(ended[0].files, ["/r/a.rs"]);
        assert_eq!(ended[0].error, None);
        assert_eq!(ended[1].reply.as_deref(), Some("second broke"));
        assert_eq!(ended[1].error.as_deref(), Some("API overloaded"));
        assert_eq!(turns_ended(&items, 1).1.len(), 1);
        assert!(turns_ended(&items, 2).1.is_empty());
        let cut = cut_short(&[user(), Item::Error("claude process exited".into())]);
        assert_eq!((cut.ending, cut.error.as_deref()), (Ending::Failed, Some("claude process exited")));
    }

    #[test]
    fn notices_carry_the_report() {
        let mut w = worker(State::Idle);
        let done = TurnEnd {
            ending: Ending::Done,
            reply: Some("STATUS: done\nSUMMARY: added it".into()),
            files: vec!["C:\\r\\src\\a.rs".into()],
            error: None,
        };
        let n = turn_notice(&w, &done);
        assert!(n.starts_with("[Kynex] The kynex worker finished its turn.\nFiles changed: a.rs."));
        assert!(n.ends_with("Its report:\nSTATUS: done\nSUMMARY: added it"));
        w.warnings = vec!["pushes to a remote".into(), "force-pushes".into()];
        let blocked = TurnEnd { reply: Some("STATUS: blocked\nQUESTION: Postgres or SQLite?".into()), ..done };
        let n = turn_notice(&w, &blocked);
        assert!(n.starts_with("[Kynex] The kynex worker is blocked and asks: Postgres or SQLite?"));
        assert!(n.contains("Warnings: it pushes to a remote; it force-pushes."));
        let failed = TurnEnd { ending: Ending::Failed, reply: None, files: vec![], error: Some("exited".into()) };
        let n = turn_notice(&w, &failed);
        assert!(n.contains("turn failed") && n.contains("Error: exited") && n.ends_with("It wrote no report."));
        assert!(waiting_notice(&w).starts_with(NOTICE));
        assert!(cut_off_notice(&["a".into()]).contains("the a worker was working. Its turn was"));
        assert!(cut_off_notice(&["a".into(), "b".into()]).contains("a, b workers were working. Their turns were"));
    }
}
