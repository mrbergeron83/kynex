//! Why Claude Code is asking: the reason a `can_use_tool` request carries
//! (`decision_reason_type`, and for a rule `decision_reason_rule_source`), in
//! words, so a prompt in Auto mode isn't a mystery. And when it didn't ask at
//! all: a call auto mode's safety check refused outright, and the allow rules
//! that would let it through next time.

use serde_json::Value;

/// What the CLI says when auto mode's safety check refuses a call without
/// asking. No prompt comes; it's the call's error result.
const AUTO_DENIED: &str = "denied by the Claude Code auto mode classifier";

/// Why auto mode refused a call without asking ("Modify Shared Resources"),
/// read from its error result; empty if it gave no reason. `None` for any
/// other result.
pub fn auto_denied(content: &str) -> Option<String> {
    let rest = &content[content.find(AUTO_DENIED)? + AUTO_DENIED.len()..];
    let Some(reason) = rest.find("Reason:").map(|i| rest[i + "Reason:".len()..].trim_start()) else {
        return Some(String::new());
    };
    let reason = match reason.strip_prefix('[') {
        Some(r) => r.split(']').next().unwrap_or(""),
        None => reason.split(['\n', '.']).next().unwrap_or(""),
    };
    Some(reason.trim().to_string())
}

/// Allow rules that would let a refused call through next time, narrowest
/// first: an MCP tool, then its whole server; a shell command exactly as it
/// ran. Empty for anything else, which is better set in the rules editor than
/// guessed at.
pub fn allow_rules(tool: &str, input: &Value) -> Vec<String> {
    if crate::mcp::split(tool).is_some() {
        return std::iter::once(tool.to_string()).chain(crate::mcp::server_rule(tool)).collect();
    }
    match (tool, input["command"].as_str().map(str::trim)) {
        ("Bash" | "PowerShell", Some(cmd)) if !cmd.is_empty() && !cmd.contains('\n') && cmd.len() <= 200 => {
            vec![format!("{tool}({cmd})")]
        }
        _ => Vec::new(),
    }
}

/// A sentence for why the CLI asked about this request, or empty if it
/// didn't say. `request` is the control request's `request` object.
pub fn explain(request: &Value) -> String {
    let kind = request["decision_reason_type"].as_str().unwrap_or("");
    let source = request["decision_reason_rule_source"].as_str().unwrap_or("");
    match kind {
        "" => String::new(),
        "rule" if source.is_empty() => "An ask rule matches this.".into(),
        "rule" => format!("An ask rule in {} matches this.", place(source)),
        "mode" => "The permission mode asks for this.".into(),
        "classifier" => "Auto mode's safety check flagged this.".into(),
        "hook" => "A hook asked.".into(),
        "workingDir" => "It reaches outside the project's folders.".into(),
        "safetyCheck" => "Claude Code's built-in safety check asks for this (a protected path or command).".into(),
        "subcommandResults" => "One part of this command needs approval.".into(),
        "sandboxOverride" => "It needs to leave the sandbox.".into(),
        other => format!("Claude Code asked ({other})."),
    }
}

/// Where a rule lives, as the CLI names its settings sources.
fn place(source: &str) -> String {
    match source {
        "userSettings" => "your user settings (~/.claude/settings.json)".into(),
        "projectSettings" => "this project's .claude/settings.json".into(),
        "localSettings" => "this project's .claude/settings.local.json".into(),
        // Kynex passes the project's rules and the provider's variables with `--settings`.
        "flagSettings" => "Kynex's rules for this project (Claude settings)".into(),
        "policySettings" => "your organisation's managed settings".into(),
        "cliArg" => "a command-line rule".into(),
        "command" => "a slash command".into(),
        "session" => "this session".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn says_which_file_an_ask_rule_is_in() {
        let r = json!({ "decision_reason_type": "rule", "decision_reason_rule_source": "projectSettings" });
        assert_eq!(explain(&r), "An ask rule in this project's .claude/settings.json matches this.");
        let r = json!({ "decision_reason_type": "rule", "decision_reason_rule_source": "flagSettings" });
        assert!(explain(&r).contains("Kynex's rules"));
        let r = json!({ "decision_reason_type": "rule" });
        assert_eq!(explain(&r), "An ask rule matches this.");
    }

    #[test]
    fn explains_the_other_reasons_and_survives_unknown_ones() {
        assert!(explain(&json!({ "decision_reason_type": "classifier" })).contains("Auto mode"));
        assert!(explain(&json!({ "decision_reason_type": "workingDir" })).contains("outside"));
        assert_eq!(explain(&json!({ "decision_reason_type": "somethingNew" })), "Claude Code asked (somethingNew).");
        // An older CLI says nothing; so does the prompt.
        assert_eq!(explain(&json!({ "tool_name": "Bash" })), "");
        assert_eq!(explain(&Value::Null), "");
    }

    #[test]
    fn reads_an_auto_mode_refusal() {
        let denied = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: \
            [Modify Shared Resources]. If you have other tasks that don't depend on this action, continue.";
        assert_eq!(auto_denied(denied).as_deref(), Some("Modify Shared Resources"));
        let plain = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: \
            Runs code in the page. Try something else.";
        assert_eq!(auto_denied(plain).as_deref(), Some("Runs code in the page"));
        let bare = "Permission for this action was denied by the Claude Code auto mode classifier.";
        assert_eq!(auto_denied(bare).as_deref(), Some(""));
        assert_eq!(auto_denied("Permission to use Bash has been denied."), None);
        assert_eq!(auto_denied("Error: element not found"), None);
    }

    #[test]
    fn offers_rules_that_fit_the_call() {
        let click = "mcp__playwright-edge__browser_click";
        assert_eq!(allow_rules(click, &json!({})), [click, "mcp__playwright-edge"]);
        assert_eq!(allow_rules("PowerShell", &json!({ "command": " git push " })), ["PowerShell(git push)"]);
        assert!(allow_rules("Bash", &json!({ "command": "a\nb" })).is_empty());
        assert!(allow_rules("Bash", &json!({})).is_empty());
        assert!(allow_rules("Write", &json!({ "file_path": "x" })).is_empty());
    }
}
