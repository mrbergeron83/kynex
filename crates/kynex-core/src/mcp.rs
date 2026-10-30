//! MCP tools and servers, in words: `mcp__playwright-edge__browser_click`
//! reads as "Edge · Click \"Sign in\"", a server's status from the CLI's
//! `init` message, and when Claude has stopped to wait for you in the browser
//! (a log-in, MFA, a consent screen).

use serde_json::Value;

use crate::session::Item;

/// `mcp__<server>__<tool>` split into server and tool; `None` for built-in tools.
pub fn split(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.split_once("__").filter(|(s, t)| !s.is_empty() && !t.is_empty())
}

/// A server as people know it: `playwright-edge` is "Edge", `claude_ai_Claude_Docs`
/// "Claude Docs", `plugin_figma_figma` "figma".
pub fn server_label(server: &str) -> String {
    let lower = server.to_ascii_lowercase();
    if lower.contains("playwright") || lower.contains("browser") || lower.contains("puppeteer") {
        return match () {
            _ if lower.contains("edge") => "Edge",
            _ if lower.contains("chrome") => "Chrome",
            _ if lower.contains("firefox") => "Firefox",
            _ if lower.contains("webkit") || lower.contains("safari") => "Safari",
            _ => "Browser",
        }
        .into();
    }
    if let Some(rest) = server.strip_prefix("claude_ai_") {
        return rest.replace('_', " ");
    }
    if let Some(rest) = server.strip_prefix("plugin_") {
        // `plugin_<plugin>_<server>`, usually the same name twice.
        return match rest.split_once('_') {
            Some((a, b)) if a == b => a.into(),
            _ => rest.into(),
        };
    }
    server.into()
}

/// What one MCP call does: a verb ("Click") and its target ("\"Sign in\"").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub verb: String,
    pub target: String,
}

/// Whether `tool` drives a browser (the Playwright servers' `browser_*`).
pub fn is_browser(tool: &str) -> bool {
    tool.starts_with("browser_")
}

/// A call to MCP tool `tool` with `input`, in words. Playwright's browser
/// tools get their own verbs; others read as their name ("Create issue").
pub fn action(tool: &str, input: &Value) -> Action {
    let s = |k: &str| input[k].as_str().unwrap_or("").trim().to_string();
    let quoted = |k: &str| {
        let v = s(k);
        if v.is_empty() { v } else { format!("\u{201c}{}\u{201d}", clip(&v, 60)) }
    };
    let (verb, target) = match tool.strip_prefix("browser_").unwrap_or("") {
        "navigate" => ("Open", s("url")),
        "navigate_back" => ("Go back", String::new()),
        "click" if input["doubleClick"] == true => ("Double-click", quoted("element")),
        "click" => ("Click", quoted("element")),
        "hover" => ("Hover", quoted("element")),
        "type" => {
            let into = quoted("element");
            let text = quoted("text");
            ("Type", if into.is_empty() { text } else { format!("{text} into {into}") })
        }
        "press_key" => ("Press", s("key")),
        "select_option" => ("Select", quoted("element")),
        "fill_form" => {
            let n = input["fields"].as_array().map_or(0, Vec::len);
            ("Fill in", plural(n, "field"))
        }
        "file_upload" => ("Upload", plural(input["paths"].as_array().map_or(0, Vec::len), "file")),
        "drag" => ("Drag", format!("{} to {}", quoted("startElement"), quoted("endElement"))),
        "take_screenshot" => ("Screenshot", quoted("element")),
        "snapshot" => ("Read the page", String::new()),
        "wait_for" => match (s("text"), s("textGone"), input["time"].as_f64()) {
            (t, _, _) if !t.is_empty() => ("Wait for", quoted("text")),
            (_, g, _) if !g.is_empty() => ("Wait until gone", quoted("textGone")),
            (_, _, Some(secs)) => ("Wait", format!("{secs}s")),
            _ => ("Wait", String::new()),
        },
        "tabs" => match s("action").as_str() {
            "new" => ("New tab", String::new()),
            "close" => ("Close tab", String::new()),
            "select" => ("Switch tab", String::new()),
            _ => ("List tabs", String::new()),
        },
        "close" => ("Close the browser", String::new()),
        "resize" => ("Resize", format!("{}×{}", input["width"], input["height"])),
        "handle_dialog" if input["accept"] == true => ("Accept the dialog", String::new()),
        "handle_dialog" => ("Dismiss the dialog", String::new()),
        "evaluate" | "run_code" => ("Run script", String::new()),
        "console_messages" => ("Read the console", String::new()),
        "network_requests" => ("Read network requests", String::new()),
        "install" => ("Install the browser", String::new()),
        _ => return Action { verb: humanize(tool), target: String::new() },
    };
    Action { verb: verb.into(), target }
}

/// A tool call's row in the transcript: ("Edge · Click", "\"Sign in\"").
/// `None` for built-in tools.
pub fn describe(name: &str, input: &Value) -> Option<(String, String)> {
    let (server, tool) = split(name)?;
    let a = action(tool, input);
    Some((format!("{} \u{b7} {}", server_label(server), a.verb), a.target))
}

/// The headline of a permission prompt for an MCP call, e.g.
/// `Click "Delete repository" in Edge?`. `None` for built-in tools.
pub fn ask(name: &str, input: &Value) -> Option<String> {
    let (server, tool) = split(name)?;
    let label = server_label(server);
    if !is_browser(tool) {
        return Some(format!("Allow Claude to use {tool} from {label}?"));
    }
    let a = action(tool, input);
    let what = if a.target.is_empty() { a.verb } else { format!("{} {}", a.verb, a.target) };
    Some(format!("{what} in {label}?"))
}

/// The permission rule covering every tool of the server `name` belongs to.
pub fn server_rule(name: &str) -> Option<String> {
    split(name).map(|(server, _)| format!("mcp__{server}"))
}

/// "Edge: 14 actions (3 screenshots)", for a run of calls to one server.
pub fn run_summary(server: &str, calls: usize, screenshots: usize) -> String {
    let label = server_label(server);
    let mut s = format!("{label}: {}", plural(calls, "action"));
    if screenshots > 0 {
        s.push_str(&format!(" ({})", plural(screenshots, "screenshot")));
    }
    s
}

/// An MCP server as the CLI reported it when the process started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connected,
    Pending,
    NeedsAuth,
    Failed,
    Disabled,
    Other(String),
}

impl Status {
    fn parse(s: &str) -> Self {
        match s {
            "connected" => Self::Connected,
            "pending" | "connecting" => Self::Pending,
            "needs-auth" | "needs_auth" => Self::NeedsAuth,
            "failed" => Self::Failed,
            "disabled" => Self::Disabled,
            other => Self::Other(other.into()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Connected => "connected",
            Self::Pending => "connecting",
            Self::NeedsAuth => "needs auth",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
            Self::Other(s) => s,
        }
    }
}

/// The servers in a stream-json `system` `init` message's `mcp_servers`.
pub fn servers(init: &Value) -> Vec<Server> {
    init["mcp_servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s["name"].as_str()?.to_string();
            Some(Server { name, status: Status::parse(s["status"].as_str().unwrap_or("")) })
        })
        .collect()
}

/// The tools each MCP server offers, from a `system` `init` message: its
/// `tools` are named `mcp__<server>__<tool>`, with the server's name spelled
/// as tool names allow; the keys here are the names [`servers`] reports.
pub fn tools(init: &Value) -> std::collections::BTreeMap<String, Vec<String>> {
    let names: Vec<String> = servers(init).into_iter().map(|s| s.name).collect();
    let mut out: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for t in init["tools"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some((server, tool)) = split(t) else { continue };
        let name = names.iter().find(|n| tool_spelling(n) == server).cloned().unwrap_or_else(|| server.to_string());
        out.entry(name).or_default().push(tool.to_string());
    }
    out
}

/// A server's name as tool names spell it: anything but letters, digits, `-`
/// and `_` becomes `_` (`claude.ai Figma` → `claude_ai_Figma`).
pub(crate) fn tool_spelling(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// All servers together, worst first: one failed makes it `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Ok,
    Waiting,
    Failed,
}

/// How the session's servers are doing; `None` when it has none (disabled ones don't count).
pub fn health(servers: &[Server]) -> Option<Health> {
    servers
        .iter()
        .filter_map(|s| match s.status {
            Status::Connected => Some(Health::Ok),
            Status::Pending | Status::NeedsAuth => Some(Health::Waiting),
            Status::Failed | Status::Other(_) => Some(Health::Failed),
            Status::Disabled => None,
        })
        .max()
}

/// Claude stopped to wait for you in the browser: its last turn drove a
/// browser and ended asking you to log in, pass MFA, accept something…
/// Gives the newest screenshot's result item, if the turn took one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handoff {
    pub screenshot: Option<usize>,
}

/// Whether the last turn in `items` handed the browser to you; see [`Handoff`].
pub fn handoff(items: &[Item]) -> Option<Handoff> {
    // Only a finished turn waits for you.
    let (end, Item::Result { is_error: false, .. }) = (items.len().checked_sub(1)?, items.last()?) else {
        return None;
    };
    // The last turn alone: one a background task started (no message from you)
    // ends the one before, which may have asked you to log in an hour ago.
    let start = items[..end]
        .iter()
        .rposition(|i| matches!(i, Item::User { .. } | Item::Task(_) | Item::Result { .. }))
        .map_or(0, |i| i + 1);
    let turn = &items[start..end];
    let browsed =
        turn.iter().any(|i| matches!(i, Item::ToolUse { name, .. } if split(name).is_some_and(|(_, t)| is_browser(t))));
    if !browsed {
        return None;
    }
    // What Claude said last, after its last call: the ask has to be there, not
    // in a note on the way ("fixed the login form").
    let last_call = turn.iter().rposition(|i| matches!(i, Item::ToolUse { .. }))?;
    let said = match turn[last_call..].iter().rev().find(|i| matches!(i, Item::Assistant(t) if !t.trim().is_empty())) {
        Some(Item::Assistant(t)) => t.to_lowercase().replace('’', "'"),
        _ => return None,
    };
    if !asks_you_in_the_browser(&said) {
        return None;
    }
    let screenshot = turn
        .iter()
        .rposition(|i| matches!(i, Item::ToolResult { images, .. } if !images.is_empty()))
        .map(|i| start + i);
    Some(Handoff { screenshot })
}

/// Whether `said` (lowercase) asks you to do something in the browser yourself:
/// log in, pass MFA, solve a captcha. Mentioning a login isn't enough, and
/// neither is "let me know when" on its own: that ends plenty of proposals.
fn asks_you_in_the_browser(said: &str) -> bool {
    const ACTS: &[&str] = &[
        "log in",
        "log into",
        "login",
        "sign in",
        "sign into",
        "authenticate",
        "mfa",
        "2fa",
        "two-factor",
        "verification code",
        "one-time code",
        "captcha",
        "logged in",
        "signed in",
        "logging in",
        "signing in",
    ];
    // Words that may come between the ask and the act: "once you've finished the MFA".
    const FILLER: &[&str] =
        &["finish", "finished", "complete", "completed", "done", "pass", "passed", "the", "with", "your", "a"];
    const ASKS: &[&str] = &[
        "please ",
        "you'll need to ",
        "you will need to ",
        "you need to ",
        "need you to ",
        "could you ",
        "can you ",
        "go ahead and ",
        "complete the ",
        "finish the ",
        "enter the ",
        "solve the ",
        "once you're ",
        "once you've ",
        "once you have ",
        "after you ",
        "when you've ",
        "when you have ",
        "when you're ",
    ];
    // An ask right before an act: "please log in", "once you've finished the
    // MFA prompt", but not "please review the login fix".
    ASKS.iter().any(|ask| {
        said.match_indices(ask).any(|(at, _)| {
            let words: Vec<&str> =
                said[at + ask.len()..].split_whitespace().skip_while(|w| FILLER.contains(w)).take(3).collect();
            let rest = words.join(" ");
            ACTS.iter().any(|act| rest.starts_with(act))
        })
    })
}

/// What the Continue button of a [`Handoff`] sends: only that the browser
/// step is done, so it can't read as a yes to anything else Claude proposed.
pub const CONTINUE: &str =
    "I've done the step you asked for in the browser (logging in, confirming). Carry on with the browser task.";

/// "create_issue" reads as "Create issue".
fn humanize(tool: &str) -> String {
    let words = tool.replace(['_', '-'], " ");
    let mut chars = words.trim().chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

fn clip(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > max || s.contains('\n') {
        format!("{}…", first.chars().take(max).collect::<String>())
    } else {
        first.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_tool_names() {
        assert_eq!(split("mcp__playwright-edge__browser_click"), Some(("playwright-edge", "browser_click")));
        assert_eq!(split("mcp__claude_ai_Claude_Docs__batch"), Some(("claude_ai_Claude_Docs", "batch")));
        assert_eq!(split("Bash"), None);
        assert_eq!(split("mcp__x"), None);
        assert_eq!(split("mcp____y"), None);
    }

    #[test]
    fn groups_tools_by_server() {
        let init = json!({
            "mcp_servers": [{ "name": "playwright-edge", "status": "connected" }, { "name": "claude.ai Figma", "status": "connected" }],
            "tools": ["Bash", "mcp__playwright-edge__browser_click", "mcp__playwright-edge__browser_navigate",
                      "mcp__claude_ai_Figma__get_screenshot", "mcp__gone__x"],
        });
        let tools = tools(&init);
        assert_eq!(tools["playwright-edge"], ["browser_click", "browser_navigate"]);
        assert_eq!(tools["claude.ai Figma"], ["get_screenshot"]);
        // A server not in the list keeps the name its tools use.
        assert_eq!(tools["gone"], ["x"]);
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn labels_servers() {
        assert_eq!(server_label("playwright-edge"), "Edge");
        assert_eq!(server_label("plugin_playwright_playwright"), "Browser");
        assert_eq!(server_label("claude_ai_Claude_Docs"), "Claude Docs");
        assert_eq!(server_label("plugin_figma_figma"), "figma");
        assert_eq!(server_label("github"), "github");
    }

    #[test]
    fn describes_browser_actions() {
        let d = |tool: &str, input| describe(&format!("mcp__playwright-edge__{tool}"), &input).unwrap();
        assert_eq!(
            d("browser_click", json!({ "element": "Sign in", "ref": "e42" })),
            ("Edge \u{b7} Click".into(), "\u{201c}Sign in\u{201d}".into())
        );
        assert_eq!(d("browser_navigate", json!({ "url": "https://x.dev" })).1, "https://x.dev");
        assert_eq!(d("browser_take_screenshot", json!({})), ("Edge \u{b7} Screenshot".into(), String::new()));
        assert_eq!(
            d("browser_type", json!({ "element": "Search", "text": "kynex" })).1,
            "\u{201c}kynex\u{201d} into \u{201c}Search\u{201d}"
        );
        assert_eq!(d("browser_fill_form", json!({ "fields": [{}, {}] })).1, "2 fields");
        assert_eq!(d("browser_tabs", json!({ "action": "new" })).0, "Edge \u{b7} New tab");
        assert_eq!(
            describe("mcp__github__create_issue", &json!({})),
            Some(("github \u{b7} Create issue".into(), String::new()))
        );
        assert_eq!(describe("Read", &json!({})), None);
    }

    #[test]
    fn asks_in_words() {
        let click = json!({ "element": "Delete repository" });
        assert_eq!(
            ask("mcp__playwright-edge__browser_click", &click).unwrap(),
            "Click \u{201c}Delete repository\u{201d} in Edge?"
        );
        assert_eq!(ask("mcp__playwright-edge__browser_snapshot", &json!({})).unwrap(), "Read the page in Edge?");
        assert_eq!(
            ask("mcp__github__create_issue", &json!({})).unwrap(),
            "Allow Claude to use create_issue from github?"
        );
        assert_eq!(server_rule("mcp__playwright-edge__browser_click").as_deref(), Some("mcp__playwright-edge"));
        assert_eq!(server_rule("Bash"), None);
    }

    #[test]
    fn server_health() {
        let init = json!({ "mcp_servers": [
            { "name": "a", "status": "connected" },
            { "name": "b", "status": "needs-auth" },
            { "name": "c", "status": "disabled" },
        ] });
        let mut list = servers(&init);
        assert_eq!(list[1], Server { name: "b".into(), status: Status::NeedsAuth });
        assert_eq!(health(&list), Some(Health::Waiting));
        list[0].status = Status::Failed;
        assert_eq!(health(&list), Some(Health::Failed));
        assert_eq!(health(&list[2..]), None, "disabled ones don't count");
        assert_eq!(health(&[]), None);
        assert!(servers(&json!({})).is_empty());
    }

    #[test]
    fn summarises_runs() {
        assert_eq!(run_summary("playwright-edge", 14, 3), "Edge: 14 actions (3 screenshots)");
        assert_eq!(run_summary("github", 1, 0), "github: 1 action");
    }

    fn call(name: &str) -> Item {
        Item::ToolUse {
            id: "x".into(),
            name: name.into(),
            summary: String::new(),
            input: "{}".into(),
            timing: Default::default(),
        }
    }

    fn shot() -> Item {
        let preview: crate::session::Preview = std::sync::Arc::new(());
        Item::ToolResult { id: "x".into(), content: String::new(), images: vec![preview], is_error: false }
    }

    fn end() -> Item {
        Item::Result { cost: 0.0, duration_ms: 0, is_error: false, tools: 0, files: Vec::new() }
    }

    /// A turn: you ask, Claude calls `tool`, gets a screenshot back, says `said`.
    fn turn(tool: &str, said: &str) -> Vec<Item> {
        let user = Item::User { text: "go".into(), images: Vec::new() };
        vec![user, call(tool), shot(), Item::Assistant(said.into()), end()]
    }

    #[test]
    fn hands_off_after_asking_for_a_login() {
        const CLICK: &str = "mcp__playwright-edge__browser_click";
        let asks = "Please sign in, and let me know when you're done.";
        assert_eq!(handoff(&turn(CLICK, asks)), Some(Handoff { screenshot: Some(2) }));
        // Still working, no browser, or nothing asked: no handoff.
        assert_eq!(handoff(&turn(CLICK, asks)[..4]), None);
        assert_eq!(handoff(&turn("Bash", asks)), None);
        assert_eq!(handoff(&turn(CLICK, "All set.")), None);
        // A new message ends it.
        let mut after = turn(CLICK, asks);
        after.push(Item::User { text: "ok".into(), images: Vec::new() });
        assert_eq!(handoff(&after), None);
        // So does a turn a background task started, which proposes something else.
        let mut task = turn(CLICK, asks);
        task.push(Item::Task(crate::tasks::parse_notification(NOTE).expect("a notice")));
        task.extend([Item::Assistant("CI passed. Want me to merge the PR?".into()), end()]);
        assert_eq!(handoff(&task), None);
    }

    const NOTE: &str = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n<summary>CI</summary>\n</task-notification>";

    #[test]
    fn a_mention_is_not_an_ask() {
        const CLICK: &str = "mcp__playwright-edge__browser_click";
        for said in [
            "Fixed the login form; the consent banner shows again.",
            "Please review the login fix when you have a moment.",
            "Let me know when you want the next step.",
            "Once you've reviewed the PR I can merge it.",
            "Want me to also add a sign-in test?",
        ] {
            assert_eq!(handoff(&turn(CLICK, said)), None, "{said}");
        }
        for said in [
            "The page wants a password. Please log in, then press Continue.",
            "You’ll need to sign in to GitHub first.",
            "Once you're logged in I'll carry on.",
            "Let me know once you've finished the MFA prompt.",
            "Could you solve the captcha?",
            "Please enter the verification code from your phone.",
        ] {
            assert!(handoff(&turn(CLICK, said)).is_some(), "{said}");
        }
        // The ask has to be the last thing said, not a note before more work.
        let mut later = turn(CLICK, "Please log in.");
        later.insert(4, Item::Assistant("Logged in already, so I went on. Done.".into()));
        assert_eq!(handoff(&later), None);
    }
}
