//! Finding an OpenRouter setup made by hand before Kynex had one: the key in
//! `OPENROUTER_API_KEY` (with `OR_MODEL`), as `setx` leaves it, or an
//! `ANTHROPIC_*` setup pointing at OpenRouter, in the environment or in the
//! `env` block of `~/.claude/settings.json`. Kynex moves what it finds into
//! the keychain and its settings on launch.

use serde_json::Value;

use super::valid_model_id;

/// Where a setup was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `OPENROUTER_API_KEY` (and `OR_MODEL`).
    OpenRouterKeyVariable,
    /// `ANTHROPIC_BASE_URL` set to OpenRouter, with `ANTHROPIC_AUTH_TOKEN`.
    AnthropicVariables,
    /// The `env` block of `~/.claude/settings.json`.
    ClaudeSettings,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::OpenRouterKeyVariable => "OPENROUTER_API_KEY in your environment",
            Source::AnthropicVariables => "the ANTHROPIC_* variables in your environment",
            Source::ClaudeSettings => "the env block of ~/.claude/settings.json",
        }
    }
}

/// A setup found, the key with it.
#[derive(Clone, PartialEq)]
pub struct Found {
    pub key: String,
    /// The model it used, if it named a valid one.
    pub model: Option<String>,
    pub source: Source,
}

/// Never prints the key.
impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found").field("model", &self.model).field("source", &self.source).finish_non_exhaustive()
    }
}

/// The first setup found: in the variables `var` gives, then in the text of
/// a Claude settings file.
pub fn find(var: impl Fn(&str) -> Option<String>, settings_json: Option<&str>) -> Option<Found> {
    from_variables(&var).or_else(|| {
        let v: Value = serde_json::from_str(settings_json?).ok()?;
        let env = v.get("env")?.as_object()?;
        let get = |k: &str| env.get(k).and_then(Value::as_str).map(str::to_string);
        from_variables(&get).map(|f| Found { source: Source::ClaudeSettings, ..f })
    })
}

fn from_variables(var: &impl Fn(&str) -> Option<String>) -> Option<Found> {
    let get = |k: &str| var(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let model = |k: &str| get(k).filter(|m| valid_model_id(m));
    if let Some(key) = get("OPENROUTER_API_KEY") {
        return Some(Found { key, model: model("OR_MODEL"), source: Source::OpenRouterKeyVariable });
    }
    let via_openrouter = get("ANTHROPIC_BASE_URL").is_some_and(|u| u.to_ascii_lowercase().contains("openrouter.ai"));
    let key = get("ANTHROPIC_AUTH_TOKEN").filter(|_| via_openrouter)?;
    Some(Found { key, model: model("ANTHROPIC_MODEL"), source: Source::AnthropicVariables })
}

/// A setup on this computer: Kynex's environment, then (on Windows) the
/// user's saved variables, which `setx` changes without telling running
/// programs, then `~/.claude/settings.json`. Blocks on files and, on
/// Windows, `reg`.
pub fn find_existing() -> Option<Found> {
    let settings = dirs::home_dir().and_then(|h| std::fs::read_to_string(h.join(".claude").join("settings.json")).ok());
    find(|k| std::env::var(k).ok().or_else(|| user_variable(k)), settings.as_deref())
}

/// A variable saved for the user (`setx`), read from the registry.
#[cfg(windows)]
fn user_variable(name: &str) -> Option<String> {
    let out = crate::cli::command("reg").args(["query", r"HKCU\Environment", "/v", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    reg_value(&String::from_utf8_lossy(&out.stdout), name)
}

#[cfg(not(windows))]
fn user_variable(_: &str) -> Option<String> {
    None
}

/// The value in `reg query`'s output: `    NAME    REG_SZ    value`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn reg_value(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(name)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("REG_EXPAND_SZ").or_else(|| rest.strip_prefix("REG_SZ"))?;
        Some(rest.trim().to_string()).filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn finds_the_setx_setup() {
        let f = find(vars(&[("OPENROUTER_API_KEY", " sk-or-1 "), ("OR_MODEL", "qwen/qwen3-coder:free")]), None);
        let f = f.unwrap();
        assert_eq!((f.key.as_str(), f.model.as_deref()), ("sk-or-1", Some("qwen/qwen3-coder:free")));
        assert_eq!(f.source, Source::OpenRouterKeyVariable);
        let no_model = find(vars(&[("OPENROUTER_API_KEY", "k"), ("OR_MODEL", "not a model")]), None).unwrap();
        assert_eq!(no_model.model, None);
    }

    #[test]
    fn finds_anthropic_variables_only_when_they_point_at_openrouter() {
        let f = find(
            vars(&[
                ("ANTHROPIC_BASE_URL", "https://openrouter.ai/api"),
                ("ANTHROPIC_AUTH_TOKEN", "sk-or-2"),
                ("ANTHROPIC_MODEL", "moonshotai/kimi-k2"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!((f.key.as_str(), f.model.as_deref()), ("sk-or-2", Some("moonshotai/kimi-k2")));
        assert_eq!(f.source, Source::AnthropicVariables);
        // A gateway of the user's own isn't OpenRouter.
        let gateway = vars(&[("ANTHROPIC_BASE_URL", "https://llm.example.com"), ("ANTHROPIC_AUTH_TOKEN", "t")]);
        assert_eq!(find(gateway, None), None);
        assert_eq!(find(vars(&[("ANTHROPIC_API_KEY", "sk-ant")]), None), None);
    }

    #[test]
    fn falls_back_to_claude_settings() {
        let json =
            r#"{ "env": { "ANTHROPIC_BASE_URL": "https://openrouter.ai/api", "ANTHROPIC_AUTH_TOKEN": "sk-or-3" } }"#;
        let f = find(vars(&[]), Some(json)).unwrap();
        assert_eq!((f.key.as_str(), f.source), ("sk-or-3", Source::ClaudeSettings));
        // The environment comes first.
        let f = find(vars(&[("OPENROUTER_API_KEY", "env")]), Some(json)).unwrap();
        assert_eq!(f.key, "env");
        assert_eq!(find(vars(&[]), Some("not json")), None);
        assert_eq!(find(vars(&[]), Some(r#"{"env":{"FOO":"1"}}"#)), None);
        assert_eq!(find(vars(&[("OPENROUTER_API_KEY", "  ")]), None), None);
    }

    #[test]
    fn debug_hides_the_key() {
        let f = Found { key: "sk-or-secret".into(), model: None, source: Source::ClaudeSettings };
        assert!(!format!("{f:?}").contains("sk-or-secret"));
    }

    #[test]
    fn reads_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    OPENROUTER_API_KEY    REG_SZ    sk-or-v1-abc\r\n\r\n";
        assert_eq!(reg_value(out, "OPENROUTER_API_KEY").as_deref(), Some("sk-or-v1-abc"));
        let expand = "    OR_MODEL    REG_EXPAND_SZ    qwen/qwen3-coder\n";
        assert_eq!(reg_value(expand, "OR_MODEL").as_deref(), Some("qwen/qwen3-coder"));
        assert_eq!(reg_value("    OTHER    REG_SZ    x", "OR_MODEL"), None);
        assert_eq!(reg_value("", "OR_MODEL"), None);
    }
}
