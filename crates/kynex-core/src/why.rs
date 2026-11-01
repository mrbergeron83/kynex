//! Why Claude Code is asking: the reason a `can_use_tool` request carries
//! (`decision_reason_type`, and for a rule `decision_reason_rule_source`), in
//! words, so a prompt in Auto mode isn't a mystery.

use serde_json::Value;

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
}
