//! Slash commands: which ones a folder's sessions have (skills, plugin and
//! project commands, built-ins), finding them as you type, and what Kynex does
//! with the built-ins that only make sense in the terminal CLI.

use std::path::Path;

use serde_json::Value;

/// A command a session can run, as the CLI's `initialize` handshake reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    /// Without the slash, e.g. `compact` or `plugin:skill`.
    pub name: String,
    pub description: String,
    /// What it takes after the name, e.g. `[model]`; empty if nothing.
    pub argument_hint: String,
    pub aliases: Vec<String>,
}

/// The commands sessions started in `cwd` have. They differ per folder
/// (project `.claude/commands`, skills, plugins). Blocks for a second or two
/// while a `claude` process starts; costs nothing.
pub fn list(cwd: &Path) -> Result<Vec<SlashCommand>, String> {
    Ok(from_init(&crate::session::initialize(cwd)?))
}

/// The commands in an `initialize` response, minus internal and retired ones.
fn from_init(v: &Value) -> Vec<SlashCommand> {
    let text = |c: &Value, k: &str| c[k].as_str().unwrap_or("").trim().to_string();
    v["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let name = c["name"].as_str()?.to_string();
            let description = text(c, "description");
            let hidden =
                name.starts_with("__") || description.starts_with("(removed)") || description.starts_with("Renamed to");
            (!hidden).then(|| SlashCommand {
                aliases: c["aliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect(),
                argument_hint: text(c, "argumentHint"),
                description,
                name,
            })
        })
        .collect()
}

/// The commands that match what's typed after the slash: names starting with
/// it first, then aliases starting with it, then names containing it.
pub fn matching<'a>(commands: &'a [SlashCommand], query: &str) -> Vec<&'a SlashCommand> {
    let q = query.to_lowercase();
    let rank = |c: &SlashCommand| {
        let name = c.name.to_lowercase();
        if name.starts_with(&q) {
            Some(0)
        } else if c.aliases.iter().any(|a| a.to_lowercase().starts_with(&q)) {
            Some(1)
        } else if name.contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    let mut found: Vec<(u8, &SlashCommand)> = commands.iter().filter_map(|c| Some((rank(c)?, c))).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    found.into_iter().map(|(_, c)| c).collect()
}

/// A message that's a slash command, split into its name (no slash) and arguments.
pub fn parse(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || "-_:.".contains(c));
    valid.then(|| (name, args.trim()))
}

/// What Kynex does when a command is sent.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Send it to Claude as the message; the CLI runs it.
    Send,
    /// `/clear`: a new session (the old one stays in the list).
    NewSession,
    /// `/model`: switch to the model given, or open the picker.
    Model(String),
    /// `/effort`: set the project's effort, or open the picker.
    Effort(String),
    /// `/resume`: the sessions panel.
    Sessions,
    /// Only does something in the terminal CLI's own UI.
    TerminalOnly,
}

/// What sending `/name args` does. Aliases count: `/new` is `/clear`.
pub fn action(name: &str, args: &str) -> Action {
    match name {
        "clear" | "reset" | "new" => Action::NewSession,
        "model" => Action::Model(args.to_string()),
        "effort" => Action::Effort(args.to_string()),
        "resume" | "continue" => Action::Sessions,
        "color" | "focus" | "theme" | "vim" | "terminal-setup" | "statusline" | "keybindings" | "ide" | "exit"
        | "quit" | "heapdump" => Action::TerminalOnly,
        _ => Action::Send,
    }
}

/// A logged slash command as it was typed: `/name args`, from the
/// `<command-name>` / `<command-args>` wrappers Claude Code saves it in.
pub fn unwrap(text: &str) -> Option<String> {
    let tag = |t: &str| {
        let open = format!("<{t}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{t}>"))? + start;
        Some(text[start..end].trim())
    };
    let name = tag("command-name").filter(|n| !n.is_empty())?;
    let slash = if name.starts_with('/') { "" } else { "/" };
    Some(match tag("command-args").filter(|a| !a.is_empty()) {
        Some(args) => format!("{slash}{name} {args}"),
        None => format!("{slash}{name}"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn cmd(name: &str, aliases: &[&str]) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn reads_the_initialize_response() {
        let v = json!({ "commands": [
            { "name": "compact", "description": "Free up context", "argumentHint": "<instructions>", "builtin": true },
            { "name": "clear", "description": "New session", "argumentHint": "", "aliases": ["reset", "new"] },
            { "name": "__remote-workflow", "description": "internal" },
            { "name": "agents", "description": "(removed) Ask Claude" },
            { "name": "extra-usage", "description": "Renamed to /usage-credits" },
            { "description": "no name" },
        ]});
        let cmds = from_init(&v);
        assert_eq!(cmds.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["compact", "clear"]);
        assert_eq!(cmds[0].argument_hint, "<instructions>");
        assert_eq!(cmds[1].aliases, ["reset", "new"]);
    }

    #[test]
    fn matches_prefixes_then_aliases_then_anywhere() {
        let cmds = [cmd("usage", &["cost"]), cmd("compact", &[]), cmd("context", &[]), cmd("plugin:commit", &[])];
        let names = |q| matching(&cmds, q).iter().map(|c| c.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names("co"), ["compact", "context", "usage", "plugin:commit"]);
        assert_eq!(names("COMM"), ["plugin:commit"]);
        assert_eq!(names(""), ["compact", "context", "plugin:commit", "usage"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("/model opus"), Some(("model", "opus")));
        assert_eq!(parse("  /compact  keep the plan \n"), Some(("compact", "keep the plan")));
        assert_eq!(parse("/plugin:skill"), Some(("plugin:skill", "")));
        assert_eq!(parse("/"), None);
        assert_eq!(parse("/usr/bin is a path"), None);
        assert_eq!(parse("hello /model"), None);
    }

    #[test]
    fn maps_terminal_built_ins() {
        assert_eq!(action("new", ""), Action::NewSession);
        assert_eq!(action("model", "opus"), Action::Model("opus".into()));
        assert_eq!(action("resume", ""), Action::Sessions);
        assert_eq!(action("vim", ""), Action::TerminalOnly);
        assert_eq!(action("compact", ""), Action::Send);
        assert_eq!(action("my-skill", "x"), Action::Send);
    }

    #[test]
    fn unwraps_logged_commands() {
        let logged = "<command-message>plan</command-message>\n<command-name>/plan</command-name>\n<command-args>add retention</command-args>";
        assert_eq!(unwrap(logged).as_deref(), Some("/plan add retention"));
        let bare = "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>";
        assert_eq!(unwrap(bare).as_deref(), Some("/clear"));
        assert_eq!(unwrap("just text"), None);
    }

    #[test]
    #[ignore = "needs the claude CLI"]
    fn lists_live() {
        let cmds = list(&std::env::temp_dir()).unwrap();
        assert!(cmds.iter().any(|c| c.name == "compact"));
    }
}
