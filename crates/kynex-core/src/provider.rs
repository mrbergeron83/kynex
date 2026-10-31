//! Who answers a session: Anthropic (the CLI as the user set it up) or
//! OpenRouter (an API key and a model of the user's choice). Each session has
//! its own [`Launch`]; Kynex starts every `claude` process itself, so the
//! provider is just the environment that process gets ([`Launch::apply`], in
//! `session::claude_command`), with no shell profile or `setx`.

use std::ffi::{OsStr, OsString};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Where OpenRouter's Anthropic-compatible API lives.
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";
/// The model OpenRouter sessions use unless the user picks another.
pub const DEFAULT_OPENROUTER_MODEL: &str = "qwen/qwen3-coder";
/// The output cap OpenRouter sessions get unless the user picks another. With
/// no cap the CLI asks for 65,536 tokens, which OpenRouter refuses (402) when
/// the credits don't cover that much.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8000;

const BASE_URL: &str = "ANTHROPIC_BASE_URL";
const MAX_OUTPUT_TOKENS: &str = "CLAUDE_CODE_MAX_OUTPUT_TOKENS";
/// The model variables: the main one and the defaults behind the CLI's
/// opus / sonnet / haiku aliases, which background calls use.
const MODELS: [&str; 4] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];
/// An OpenRouter setup made by hand, besides its base URL and models: removed
/// from an Anthropic session that inherits one.
const OPENROUTER_LEFTOVERS: [&str; 2] = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_SMALL_FAST_MODEL"];
/// What would take an OpenRouter session's requests somewhere else, or ask
/// OpenRouter for a Claude model: removed from it (removed, not set to empty).
const NOT_FOR_OPENROUTER: [&str; 5] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    OpenRouter,
}

impl Provider {
    /// Whether its models take Claude's `--effort` levels. OpenRouter's
    /// mostly don't, so OpenRouter sessions get none and the picker hides.
    pub fn takes_effort(self) -> bool {
        self == Provider::Anthropic
    }

    /// For `skip_serializing_if`: Anthropic, the default, isn't written.
    pub fn is_anthropic_ref(&self) -> bool {
        *self == Provider::Anthropic
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenRouter => "OpenRouter",
        }
    }
}

/// OpenRouter's settings. The key isn't here: it's a secret, kept out of
/// anything saved as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRouterSettings {
    /// An OpenRouter model ID, like `qwen/qwen3-coder` or `qwen/qwen3-coder:free`.
    pub model: String,
    /// `CLAUDE_CODE_MAX_OUTPUT_TOKENS`: the most tokens one reply may use.
    pub max_output_tokens: u32,
}

impl Default for OpenRouterSettings {
    fn default() -> Self {
        Self { model: DEFAULT_OPENROUTER_MODEL.into(), max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS }
    }
}

/// A session's provider, with everything its `claude` processes need to use it.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Launch {
    /// The CLI as the user set it up: their login, API key, gateway or cloud.
    #[default]
    Anthropic,
    OpenRouter {
        key: String,
        settings: OpenRouterSettings,
    },
}

/// Never prints the key.
impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Launch::Anthropic => f.write_str("Anthropic"),
            Launch::OpenRouter { settings, .. } => {
                f.debug_struct("OpenRouter").field("key", &"<hidden>").field("settings", settings).finish()
            }
        }
    }
}

/// How a `claude` process's environment differs from Kynex's: variables to
/// set, and inherited ones to remove.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Env {
    pub set: Vec<(&'static str, String)>,
    pub remove: Vec<OsString>,
}

impl Launch {
    /// OpenRouter with `settings` and the key saved in `keychain`.
    /// Blocks on the keychain.
    pub fn openrouter(settings: OpenRouterSettings, keychain: &crate::secrets::Keychain) -> Result<Launch, String> {
        let key = keychain
            .get(crate::secrets::Secret::OpenRouterKey)?
            .ok_or("No OpenRouter key is saved. Add one in Settings → Providers.")?;
        Ok(Launch::OpenRouter { key, settings })
    }

    /// This provider for a session on `model`: OpenRouter's main and background
    /// models all become it, so a switch reaches every call; empty keeps its
    /// own. Anthropic is unchanged (its model goes as `--model`).
    pub fn with_model(&self, model: &str) -> Launch {
        match self {
            Launch::OpenRouter { key, settings } if !model.trim().is_empty() => Launch::OpenRouter {
                key: key.clone(),
                settings: OpenRouterSettings { model: model.trim().to_string(), ..settings.clone() },
            },
            _ => self.clone(),
        }
    }

    pub fn provider(&self) -> Provider {
        match self {
            Launch::Anthropic => Provider::Anthropic,
            Launch::OpenRouter { .. } => Provider::OpenRouter,
        }
    }

    /// The changes to an environment of `inherited` names and values.
    ///
    /// **Anthropic** leaves the user's own setup alone (an API key, a gateway's
    /// `ANTHROPIC_BASE_URL`, Bedrock / Vertex), and only removes an OpenRouter
    /// setup made by hand: when the inherited `ANTHROPIC_BASE_URL` is
    /// OpenRouter's, it goes with its token and model variables.
    ///
    /// **OpenRouter** gets exactly the variables of the tested manual setup: the
    /// base URL, the key as `ANTHROPIC_AUTH_TOKEN`, the model as
    /// `ANTHROPIC_MODEL` and as the opus / sonnet / haiku defaults (so
    /// background calls and the CLI's aliases never ask OpenRouter for a Claude
    /// model), and the output cap; and loses `ANTHROPIC_API_KEY` and anything
    /// else that would send it elsewhere.
    pub fn env<I, N, V>(&self, inherited: I) -> Env
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let inherited: Vec<(OsString, OsString)> =
            inherited.into_iter().map(|(n, v)| (n.as_ref().to_owned(), v.as_ref().to_owned())).collect();
        let (set, unwanted): (Vec<(&'static str, String)>, Vec<&str>) = match self {
            Launch::Anthropic => {
                let via_openrouter = inherited.iter().any(|(n, v)| same_name(n, BASE_URL) && is_openrouter(v));
                let unwanted = if via_openrouter {
                    [BASE_URL].into_iter().chain(MODELS).chain(OPENROUTER_LEFTOVERS).collect()
                } else {
                    Vec::new()
                };
                (Vec::new(), unwanted)
            }
            Launch::OpenRouter { key, settings } => {
                let model = settings.model.trim();
                let model = if model.is_empty() { DEFAULT_OPENROUTER_MODEL } else { model };
                let cap = if settings.max_output_tokens == 0 {
                    DEFAULT_MAX_OUTPUT_TOKENS
                } else {
                    settings.max_output_tokens
                };
                let mut set =
                    vec![(BASE_URL, OPENROUTER_BASE_URL.to_string()), ("ANTHROPIC_AUTH_TOKEN", key.trim().to_string())];
                set.extend(MODELS.map(|m| (m, model.to_string())));
                set.push((MAX_OUTPUT_TOKENS, cap.to_string()));
                (set, NOT_FOR_OPENROUTER.to_vec())
            }
        };
        let remove = inherited
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| unwanted.iter().any(|u| same_name(name, u)) && !set.iter().any(|(s, _)| same_name(name, s)))
            .collect();
        Env { set, remove }
    }

    /// Gives `cmd` this provider's environment, on top of Kynex's own.
    pub fn apply(&self, cmd: &mut Command) {
        let env = self.env(std::env::vars_os());
        for name in &env.remove {
            cmd.env_remove(name);
        }
        for (name, value) in &env.set {
            cmd.env(name, value);
        }
    }
}

/// Whether a base URL is OpenRouter's.
fn is_openrouter(url: &OsStr) -> bool {
    url.to_str().is_some_and(|u| u.to_ascii_lowercase().contains("openrouter.ai"))
}

/// Variable names compare without case on Windows, exactly elsewhere.
fn same_name(a: &OsStr, b: &str) -> bool {
    match a.to_str() {
        Some(a) if cfg!(windows) => a.eq_ignore_ascii_case(b),
        Some(a) => a == b,
        None => false,
    }
}

/// The provider variables in the `env` block of a Claude settings file's text.
/// The CLI applies that block to itself, so these may override what Kynex sets.
pub fn settings_overrides(settings_json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(settings_json) else { return Vec::new() };
    let Some(env) = v["env"].as_object() else { return Vec::new() };
    let decides_provider = |k: &str| {
        let k = OsStr::new(k);
        [BASE_URL, "ANTHROPIC_AUTH_TOKEN", MAX_OUTPUT_TOKENS]
            .into_iter()
            .chain(MODELS)
            .chain(NOT_FOR_OPENROUTER)
            .any(|n| same_name(k, n))
    };
    let mut names: Vec<String> = env.keys().filter(|k| decides_provider(k)).cloned().collect();
    names.sort();
    names
}

/// The provider variables in the `env` block of the user's
/// `~/.claude/settings.json`. Reads the file.
pub fn user_settings_overrides() -> Vec<String> {
    dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".claude").join("settings.json")).ok())
        .map(|s| settings_overrides(&s))
        .unwrap_or_default()
}

/// A warning for the user about `names` (from [`settings_overrides`]), or
/// `None` when there's nothing to warn about.
pub fn overrides_warning(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "The \"env\" block in ~/.claude/settings.json sets {}, which may override the provider Kynex picks. \
         Remove {} to let Kynex choose.",
        names.join(", "),
        if names.len() == 1 { "it" } else { "them" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openrouter(model: &str, cap: u32) -> Launch {
        Launch::OpenRouter {
            key: "sk-or-test".into(),
            settings: OpenRouterSettings { model: model.into(), max_output_tokens: cap },
        }
    }

    fn get<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
        env.set.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }

    fn removed(env: &Env) -> Vec<&str> {
        env.remove.iter().map(|n| n.to_str().unwrap()).collect()
    }

    const NONE: [(&str, &str); 0] = [];

    #[test]
    fn openrouter_sets_the_tested_variables() {
        let env = openrouter("qwen/qwen3-coder:free", 8000).env(NONE);
        assert_eq!(get(&env, "ANTHROPIC_BASE_URL"), Some("https://openrouter.ai/api"));
        assert_eq!(get(&env, "ANTHROPIC_AUTH_TOKEN"), Some("sk-or-test"));
        for name in MODELS {
            assert_eq!(get(&env, name), Some("qwen/qwen3-coder:free"), "{name}");
        }
        assert_eq!(get(&env, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"), Some("8000"));
        assert_eq!(env.set.len(), 7);
    }

    #[test]
    fn openrouter_defaults_fill_blanks() {
        let env = openrouter("  ", 0).env(NONE);
        assert_eq!(get(&env, "ANTHROPIC_MODEL"), Some(DEFAULT_OPENROUTER_MODEL));
        assert_eq!(get(&env, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"), Some("8000"));
        let defaults = OpenRouterSettings::default();
        assert_eq!((defaults.model.as_str(), defaults.max_output_tokens), ("qwen/qwen3-coder", 8000));
    }

    #[test]
    fn openrouter_drops_what_would_send_it_elsewhere() {
        let inherited = [
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("ANTHROPIC_SMALL_FAST_MODEL", "claude-haiku"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("ANTHROPIC_MODEL", "opus"),
            ("OPENROUTER_API_KEY", "sk-or"),
            ("PATH", "/bin"),
        ];
        let env = openrouter("m", 100).env(inherited);
        // ANTHROPIC_MODEL is overwritten, not removed; the rest isn't the CLI's business.
        assert_eq!(removed(&env), ["ANTHROPIC_API_KEY", "ANTHROPIC_SMALL_FAST_MODEL", "CLAUDE_CODE_USE_BEDROCK"]);
    }

    #[test]
    fn anthropic_leaves_other_setups_alone() {
        let inherited = [
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("ANTHROPIC_BASE_URL", "https://llm-gateway.example.com"),
            ("ANTHROPIC_AUTH_TOKEN", "gateway-token"),
            ("ANTHROPIC_MODEL", "claude-opus-4"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "32000"),
        ];
        assert_eq!(Launch::Anthropic.env(inherited), Env::default());
    }

    #[test]
    fn anthropic_drops_a_hand_made_openrouter_setup() {
        let inherited = [
            ("ANTHROPIC_BASE_URL", "https://openrouter.ai/api"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-or"),
            ("ANTHROPIC_MODEL", "qwen/qwen3-coder"),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "qwen/qwen3-coder"),
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "8000"),
            ("OPENROUTER_API_KEY", "sk-or"),
        ];
        let env = Launch::Anthropic.env(inherited);
        assert!(env.set.is_empty());
        // The user's own API key and output cap stay.
        assert_eq!(
            removed(&env),
            ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"]
        );
    }

    #[test]
    fn names_match_without_case_only_on_windows() {
        let env = Launch::Anthropic.env([("anthropic_base_url", "https://OpenRouter.ai/api")]);
        assert_eq!(env.remove.len(), usize::from(cfg!(windows)));
    }

    #[test]
    fn apply_sets_and_removes_on_the_command() {
        let mut cmd = Command::new("claude");
        openrouter("m", 500).apply(&mut cmd);
        let envs: Vec<_> =
            cmd.get_envs().map(|(k, v)| (k.to_str().unwrap().to_string(), v.map(|v| v.to_owned()))).collect();
        assert!(envs.contains(&("ANTHROPIC_MODEL".into(), Some("m".into()))));
        assert!(envs.contains(&("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), Some("500".into()))));
    }

    #[test]
    fn debug_hides_the_key() {
        let shown = format!("{:?}", openrouter("m", 1));
        assert!(!shown.contains("sk-or-test"), "{shown}");
        assert!(shown.contains("<hidden>"));
    }

    #[test]
    fn finds_provider_variables_in_a_settings_env_block() {
        let json = r#"{ "env": { "ANTHROPIC_BASE_URL": "x", "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "1", "FOO": "y",
                       "ANTHROPIC_AUTH_TOKEN": "z" }, "model": "opus" }"#;
        assert_eq!(
            settings_overrides(json),
            ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_MAX_OUTPUT_TOKENS"]
        );
        assert!(settings_overrides(r#"{ "env": { "FOO": "1" } }"#).is_empty());
        assert!(settings_overrides("not json").is_empty());
        assert!(settings_overrides("{}").is_empty());
    }

    #[test]
    fn warns_only_when_something_overrides() {
        assert_eq!(overrides_warning(&[]), None);
        let one = overrides_warning(&["ANTHROPIC_BASE_URL".into()]).unwrap();
        assert!(one.contains("ANTHROPIC_BASE_URL") && one.contains("Remove it"), "{one}");
        let two = overrides_warning(&["A".into(), "B".into()]).unwrap();
        assert!(two.contains("A, B") && two.contains("Remove them"), "{two}");
    }

    #[test]
    fn openrouter_takes_its_key_from_the_keychain() {
        use crate::secrets::{Keychain, Secret};
        let k = Keychain::with_store(keyring_core::mock::Store::new().unwrap(), "kynex-test");
        let err = Launch::openrouter(OpenRouterSettings::default(), &k).unwrap_err();
        assert!(err.contains("No OpenRouter key"), "{err}");
        k.set(Secret::OpenRouterKey, "sk-or-saved").unwrap();
        let launch = Launch::openrouter(OpenRouterSettings::default(), &k).unwrap();
        assert_eq!(get(&launch.env(NONE), "ANTHROPIC_AUTH_TOKEN"), Some("sk-or-saved"));
    }

    #[test]
    fn a_session_model_reaches_every_openrouter_model_variable() {
        let env = openrouter("qwen/qwen3-coder", 8000).with_model("moonshotai/kimi-k2").env(NONE);
        for name in MODELS {
            assert_eq!(get(&env, name), Some("moonshotai/kimi-k2"), "{name}");
        }
        assert_eq!(get(&env, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"), Some("8000"));
        let kept = openrouter("qwen/qwen3-coder", 8000).with_model(" ").env(NONE);
        assert_eq!(get(&kept, "ANTHROPIC_MODEL"), Some("qwen/qwen3-coder"));
        assert_eq!(Launch::Anthropic.with_model("opus"), Launch::Anthropic);
        assert!(Provider::Anthropic.takes_effort() && !Provider::OpenRouter.takes_effort());
    }

    #[test]
    fn provider_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Provider::OpenRouter).unwrap(), "\"openrouter\"");
        assert_eq!(serde_json::from_str::<Provider>("\"anthropic\"").unwrap(), Provider::Anthropic);
    }
}
