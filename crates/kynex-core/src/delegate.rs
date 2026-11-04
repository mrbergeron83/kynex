//! Delegating to cheaper models in project sessions (#272). A session on a
//! stronger model gets subagents (`--agents`, mostly Haiku) and a short rule on
//! what to hand them: contained, clear work, check runs and web lookups go to
//! them, and design, debugging and anything risky stay with the main model.
//! The main model plans and checks, so the cheap one never has to judge
//! whether a task is beyond it.
//!
//! The agents are Kynex's own, kept in `<data dir>/agents.json` and passed on
//! the command line, so nothing is written to the user's `.claude` folders.
//! The list starts as [`defaults`]; Settings → Agents edits it. Delegating is
//! on by default (App settings); a project can turn it, or single agents, off
//! ([`crate::project::ClaudeSettings`]). KNX Ops workers have careful mode
//! instead ([`crate::careful`]).
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// One subagent: what `claude --agents` takes, and whether it's in use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// What the main model calls it: lowercase letters, digits and dashes.
    pub name: String,
    /// When to use it. The main model reads it in the agent list and in the rule.
    pub description: String,
    /// Its instructions.
    pub prompt: String,
    /// `haiku`, `sonnet`, `opus`, or empty for the session's own model.
    pub model: String,
    /// The tools it may use; empty is every tool.
    pub tools: Vec<String>,
    pub on: bool,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            prompt: String::new(),
            model: "haiku".into(),
            tools: Vec::new(),
            on: true,
        }
    }
}

/// The models an agent can run on, with their labels; empty is the session's own.
pub const MODELS: [(&str, &str); 4] =
    [("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus"), ("", "Same as the session")];

const DOER: &str = "You carry out the task you're given, exactly as described. Make the changes and run the checks \
it names. Report what you changed, what the checks said, and anything that didn't match the task. If something \
doesn't match or you'd have to guess, stop and say what's different; don't improvise around it.";

/// The agents Kynex starts with, and goes back to on "Restore defaults".
pub fn defaults() -> Vec<Agent> {
    let agent = |name: &str, description: &str, prompt: &str, model: &str, tools: &[&str]| Agent {
        name: name.into(),
        description: description.into(),
        prompt: prompt.into(),
        model: model.into(),
        tools: tools.iter().map(|t| t.to_string()).collect(),
        on: true,
    };
    vec![
        agent(
            "haiku-worker",
            "Fast, cheap: contained, clear work where you can say which files and what change (renames, \
             boilerplate, ports or tests from a spec, the same edit across files). Independent items can go to \
             several at once.",
            DOER,
            "haiku",
            &[],
        ),
        agent(
            "sonnet-worker",
            "Mid-priced: multi-file changes you've designed that are too much for Haiku, and fixing what a Haiku \
             worker got wrong.",
            DOER,
            "sonnet",
            &[],
        ),
        agent(
            "checker",
            "Every build, test or lint run; it returns only the failures. Never edits.",
            "Run the checks you're asked to. Change nothing. Report PASS, or each failure as file:line and the error \
             in one line, grouped when many share a cause.",
            "haiku",
            &["Bash", "Read", "Grep", "Glob"],
        ),
        agent(
            "researcher",
            "Docs, APIs, changelogs and anything else on the web; answers briefly with links.",
            "Answer the question from the web, official docs first. Give the answer, a short example only if asked, \
             and the links you used. Say so if you couldn't confirm it.",
            "haiku",
            &["WebSearch", "WebFetch", "Read"],
        ),
    ]
}

fn file() -> PathBuf {
    crate::paths::data_dir().join("agents.json")
}

/// The agents, from their file; [`defaults`] until the list is first changed,
/// or if the file can't be read. Blocks on the disk.
pub fn load() -> Vec<Agent> {
    std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(defaults)
}

/// Saves the list. Blocks on the disk.
pub fn save(agents: &[Agent]) -> Result<(), String> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    crate::project::write_json(&file(), &agents)
}

/// Whether `agent` can join `others` as it is: a name the CLI takes, not
/// already used, and something for the main model to go on.
pub fn check(agent: &Agent, others: &[Agent]) -> Result<(), String> {
    let name = agent.name.as_str();
    if name.is_empty() {
        return Err("Give it a name.".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("Names use lowercase letters, digits and dashes, like test-writer.".into());
    }
    if others.iter().any(|a| a.name == name) {
        return Err(format!("There's already an agent called {name}."));
    }
    if agent.description.trim().is_empty() {
        return Err("Say when to use it: Claude picks agents by their description.".into());
    }
    if agent.prompt.trim().is_empty() {
        return Err("Give it instructions.".into());
    }
    Ok(())
}

/// Tools typed as a list: `Bash, Read Grep` → `["Bash", "Read", "Grep"]`.
pub fn parse_tools(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n']).map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect()
}

/// The app-wide default, which projects without their own choice follow.
static DEFAULT: AtomicBool = AtomicBool::new(true);

/// Sets the app-wide default; sessions started from now on follow it.
pub fn set_default(on: bool) {
    DEFAULT.store(on, Ordering::Relaxed);
}

pub fn default_on() -> bool {
    DEFAULT.load(Ordering::Relaxed)
}

/// Whether a project delegates: its own choice, else `default`.
pub fn enabled(project: Option<bool>, default: bool) -> bool {
    project.unwrap_or(default)
}

/// Every subagent's report goes into the main model's context, so it's capped.
/// Added to each agent's instructions when a session starts.
const REPORT: &str = "Your report goes into a more expensive model's context: at most 10 lines, no code it didn't \
ask for, no restating the task.";

const RULE_START: &str = "Delegate to save your tokens and context. Subagents start fresh: give them the files, the \
exact task and how to check it.";
const RULE_END: &str = "- Explore: finding things in the code.
Hand work to the agents named above, not general-purpose: that one runs on your own model and saves nothing.
Keep for yourself: design, debugging a cause you haven't found, concurrency, security, data changes, vague \
requirements, and anything quicker to do than to explain. If a subagent's checks fail, fix it or sharpen the \
instruction; don't resend the same task.";
/// Added after every rule, the made one or a project's own, so Claude points
/// the user at the setting rather than an internal field name.
const WHERE: &str = "You're running in Kynex. If the user wants to change what you delegate, point them to the \
Agents button in the project's toolbar, where they can turn it off, pick agents or write this project's own rule in \
place of this one; the agents themselves are in Settings (Ctrl+,) → Agents.";

fn family(model: &str) -> &'static str {
    let m = model.to_ascii_lowercase();
    ["haiku", "sonnet", "opus"].into_iter().find(|f| m.contains(f)).unwrap_or("")
}

/// The agents a session on `model` gets: the ones on, minus those its project
/// turned off, minus workers on the session's own model (handing work to the
/// same model saves nothing; an agent with fewer tools still saves context).
pub fn for_session<'a>(agents: &'a [Agent], off: &[String], model: &str) -> Vec<&'a Agent> {
    let session = family(model);
    agents
        .iter()
        .filter(|a| a.on && !off.contains(&a.name))
        .filter(|a| !(a.tools.is_empty() && !session.is_empty() && family(&a.model) == session))
        .collect()
}

/// What the main model is told: when to use each agent, from their descriptions.
/// Explicit triggers rather than "use them when it helps", since a model asked
/// to judge that keeps the work. Short, since every turn carries it.
pub fn rule(agents: &[&Agent]) -> String {
    let mut s = String::from(RULE_START);
    for a in agents {
        s.push_str(&format!("\n- {}: {}", a.name, a.description.trim()));
    }
    s.push('\n');
    s.push_str(RULE_END);
    s
}

/// A message asking Claude to write a project's own rule for `agents`, for the
/// user to send and paste the answer into the project's Agents settings.
pub fn rule_request(agents: &[&Agent]) -> String {
    let mut s = String::from(
        "Write a delegation rule for this project, for Kynex's project settings → Agents → \"This project's own \
         rule\". It replaces the default rule below, so keep what still applies. Look at how this project builds, \
         tests and lints, its layout, and any plan or task files, then say concretely what goes to each agent: \
         the exact commands for the checker, which kinds of edits or which files suit each worker, and what I \
         should keep for myself. Plain sentences, under 12 lines. Answer with only the rule, in a single \
         ```text code block so I can copy it.\n\n\
         The agents:",
    );
    for a in agents {
        s.push_str(&format!("\n- {}: {}", a.name, a.description.trim()));
    }
    s.push_str("\n\nThe default rule:\n");
    s.push_str(&rule(agents));
    s
}

/// A message asking Claude to suggest new agents for this project, in the
/// fields Settings → Agents asks for.
pub fn agents_request(agents: &[&Agent]) -> String {
    let mut s = String::from(
        "Suggest up to three subagents that would save tokens on this project, beyond the ones I have. Look at \
         its build, test and lint commands, its layout and the work that repeats. For each give the fields of \
         Kynex's Settings → Agents → Add agent, ready to paste: Name (lowercase-with-dashes), When to use it \
         (one or two sentences; the main model picks agents by this), Instructions, Model (haiku, sonnet or \
         opus; haiku unless it needs judgement) and Tools (blank for all, or a list like Bash, Read, Grep; \
         leave out Edit and Write for one that only checks or reads). Head each agent with its name, then put \
         each field under its label in its own ```text code block, so I can copy them one by one.\n\n\
         The agents I have:",
    );
    for a in agents {
        s.push_str(&format!("\n- {}: {}", a.name, a.description.trim()));
    }
    s
}

/// `agents` as `claude --agents` takes them, each with the report cap.
pub fn agents_json(agents: &[&Agent]) -> String {
    let mut map = Map::new();
    for a in agents {
        let mut v = json!({
            "description": a.description.trim(),
            "prompt": format!("{} {REPORT}", a.prompt.trim()),
        });
        if !a.model.is_empty() {
            v["model"] = json!(a.model);
        }
        if !a.tools.is_empty() {
            v["tools"] = json!(a.tools);
        }
        map.insert(a.name.clone(), v);
    }
    Value::Object(map).to_string()
}

/// The flags for a session on `model` (empty for the CLI's default): the agents
/// it gets, and the project's own rule or else one made from them. None when it
/// gets no agents.
pub fn cli_args(agents: &[Agent], off: &[String], model: &str, own_rule: &str) -> Vec<String> {
    let mine = for_session(agents, off, model);
    if mine.is_empty() {
        return Vec::new();
    }
    let rule = if own_rule.trim().is_empty() { rule(&mine) } else { own_rule.trim().to_string() };
    vec!["--agents".into(), agents_json(&mine), "--append-system-prompt".into(), format!("{rule}\n{WHERE}")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(agents: &[&Agent]) -> Vec<String> {
        agents.iter().map(|a| a.name.clone()).collect()
    }

    #[test]
    fn a_project_choice_wins_over_the_default() {
        assert!(enabled(None, true));
        assert!(!enabled(None, false));
        assert!(enabled(Some(true), false));
        assert!(!enabled(Some(false), true));
    }

    #[test]
    fn the_defaults_are_valid_and_the_checkers_read_only() {
        let all = defaults();
        for (i, a) in all.iter().enumerate() {
            let others: Vec<_> = all.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, a)| a.clone()).collect();
            assert_eq!(check(a, &others), Ok(()), "{}", a.name);
        }
        for a in all.iter().filter(|a| a.name == "checker" || a.name == "researcher") {
            assert!(!a.tools.is_empty() && !a.tools.iter().any(|t| t == "Edit" || t == "Write"), "{}", a.name);
        }
    }

    #[test]
    fn each_session_gets_the_agents_that_save_it_something() {
        let all = defaults();
        let all_four = ["haiku-worker", "sonnet-worker", "checker", "researcher"].map(String::from).to_vec();
        assert_eq!(names(&for_session(&all, &[], "opus")), all_four);
        assert_eq!(names(&for_session(&all, &[], "")), all_four);
        // A Sonnet session has no use for a Sonnet worker, nor Haiku for a Haiku one;
        // the read-only Haiku helpers still keep logs and web pages out of its context.
        assert!(!names(&for_session(&all, &[], "claude-sonnet-5-5")).contains(&"sonnet-worker".to_string()));
        assert_eq!(names(&for_session(&all, &[], "haiku")), ["sonnet-worker", "checker", "researcher"]);
    }

    #[test]
    fn agents_turned_off_are_left_out() {
        let mut all = defaults();
        all[3].on = false;
        let mine = for_session(&all, &["checker".into()], "opus");
        assert_eq!(names(&mine), ["haiku-worker", "sonnet-worker"]);
        let off = ["haiku-worker", "sonnet-worker", "checker"].map(String::from);
        assert!(cli_args(&all, &off, "opus", "").is_empty());
    }

    #[test]
    fn the_flags_carry_the_agents_and_a_rule_made_from_them() {
        let all = defaults();
        let args = cli_args(&all, &[], "opus", "");
        assert_eq!(args[0], "--agents");
        let v: Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(v["haiku-worker"]["model"], "haiku");
        assert!(v["haiku-worker"]["tools"].is_null());
        assert!(v["checker"]["prompt"].as_str().unwrap().ends_with(REPORT));
        assert_eq!(args[2], "--append-system-prompt");
        for a in &all {
            assert!(args[3].contains(&format!("- {}: ", a.name)), "{}", a.name);
        }
        // A project's own rule replaces the made one; both say where to change it.
        assert!(args[3].ends_with(WHERE));
        assert_eq!(cli_args(&all, &[], "opus", "Give it all tests.")[3], format!("Give it all tests.\n{WHERE}"));
    }

    #[test]
    fn the_requests_name_every_agent() {
        let all = defaults();
        let mine: Vec<&Agent> = all.iter().collect();
        let (rule_ask, agents_ask) = (rule_request(&mine), agents_request(&mine));
        for a in &all {
            assert!(rule_ask.contains(&format!("- {}: ", a.name)), "{}", a.name);
            assert!(agents_ask.contains(&format!("- {}: ", a.name)), "{}", a.name);
        }
        assert!(rule_ask.ends_with(RULE_END));
    }

    #[test]
    fn an_agent_on_the_sessions_model_has_no_model_key() {
        let a = Agent {
            name: "x".into(),
            description: "d".into(),
            prompt: "p".into(),
            model: String::new(),
            ..Default::default()
        };
        let v: Value = serde_json::from_str(&agents_json(&[&a])).unwrap();
        assert!(v["x"]["model"].is_null());
    }

    #[test]
    fn checks_a_new_agent() {
        let ok =
            Agent { name: "test-writer".into(), description: "d".into(), prompt: "p".into(), ..Default::default() };
        assert_eq!(check(&ok, &[]), Ok(()));
        assert!(check(&ok, std::slice::from_ref(&ok)).is_err());
        assert!(check(&Agent { name: "Test Writer".into(), ..ok.clone() }, &[]).is_err());
        assert!(check(&Agent { description: " ".into(), ..ok.clone() }, &[]).is_err());
        assert!(check(&Agent { prompt: String::new(), ..ok }, &[]).is_err());
    }

    #[test]
    fn reads_a_typed_tool_list() {
        assert_eq!(parse_tools("Bash, Read  Grep\n"), ["Bash", "Read", "Grep"]);
        assert!(parse_tools(" , ").is_empty());
    }
}
