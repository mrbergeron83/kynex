//! The user's OpenRouter settings, app-wide (the key is the user's, not a
//! project's): My models, the default one, the output cap and the last key
//! check. Saved with the app's state; the key itself is in the keychain.

use serde::{Deserialize, Serialize};

use super::{KeyInfo, Problem};
use crate::provider::{DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_OPENROUTER_MODEL, OpenRouterSettings};

/// The free variant offered next to the default from the start.
const DEFAULT_FREE_MODEL: &str = "qwen/qwen3-coder:free";
/// The output cap's bounds: enough for an edit, and no more than models give.
pub const MIN_OUTPUT_TOKENS: u32 = 1000;
pub const MAX_OUTPUT_TOKENS: u32 = 128_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    /// My models: the only OpenRouter models the model menus offer.
    pub models: Vec<String>,
    /// The one new OpenRouter sessions start on.
    pub default_model: String,
    /// `CLAUDE_CODE_MAX_OUTPUT_TOKENS` for every OpenRouter session.
    pub max_output_tokens: u32,
    pub last_check: Option<LastCheck>,
    /// The user removed the key in Settings, so a setup found on launch
    /// (`OPENROUTER_API_KEY`…) isn't brought back in. Saving a key clears it.
    pub key_removed: bool,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            models: vec![DEFAULT_OPENROUTER_MODEL.into(), DEFAULT_FREE_MODEL.into()],
            default_model: DEFAULT_OPENROUTER_MODEL.into(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            last_check: None,
            key_removed: false,
        }
    }
}

/// How the last key check went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastCheck {
    /// When, in seconds since 1970.
    pub at: u64,
    /// The model the test request went to.
    pub model: String,
    pub ok: bool,
    /// The credit left when it passed, or what went wrong.
    pub message: String,
}

impl OpenRouterConfig {
    /// The default model, or the first of My models if it's gone.
    pub fn default_model(&self) -> &str {
        if self.models.contains(&self.default_model) {
            &self.default_model
        } else {
            self.models.first().map_or(DEFAULT_OPENROUTER_MODEL, String::as_str)
        }
    }

    /// What a session on `model` (or the default) gets.
    pub fn settings(&self, model: Option<&str>) -> OpenRouterSettings {
        let model = model.filter(|m| !m.trim().is_empty()).unwrap_or(self.default_model());
        OpenRouterSettings { model: model.to_string(), max_output_tokens: self.max_output_tokens }
    }

    /// Adds `id` to My models; fine if it's there already.
    pub fn add_model(&mut self, id: &str) -> Result<(), String> {
        let id = id.trim();
        if !valid_model_id(id) {
            return Err(format!("\"{id}\" isn't an OpenRouter model ID; they look like qwen/qwen3-coder."));
        }
        if !self.models.iter().any(|m| m == id) {
            self.models.push(id.to_string());
        }
        Ok(())
    }

    /// Removes `id` from My models. The last one stays: sessions need a model.
    /// The default moves to the first left if it was this one.
    pub fn remove_model(&mut self, id: &str) -> Result<(), String> {
        if self.models.len() <= 1 && self.models.iter().any(|m| m == id) {
            return Err("Keep at least one model: OpenRouter sessions need one.".into());
        }
        self.models.retain(|m| m != id);
        if self.default_model == id {
            self.default_model = self.models.first().cloned().unwrap_or_default();
        }
        Ok(())
    }

    /// Makes `id`, one of My models, the default.
    pub fn set_default(&mut self, id: &str) {
        if self.models.iter().any(|m| m == id) {
            self.default_model = id.to_string();
        }
    }

    /// Sets the output cap from what was typed: a whole number of tokens.
    pub fn set_max_output_tokens(&mut self, typed: &str) -> Result<(), String> {
        let digits: String = typed.chars().filter(|c| !matches!(c, ',' | '_' | ' ')).collect();
        let n: u32 = digits.parse().map_err(|_| format!("\"{}\" isn't a number of tokens.", typed.trim()))?;
        if !(MIN_OUTPUT_TOKENS..=MAX_OUTPUT_TOKENS).contains(&n) {
            return Err(format!("The output cap goes from {MIN_OUTPUT_TOKENS} to {MAX_OUTPUT_TOKENS} tokens."));
        }
        self.max_output_tokens = n;
        Ok(())
    }

    /// Remembers how a check of `model` went, at `at` (seconds since 1970).
    pub fn record_check(&mut self, model: &str, result: &Result<KeyInfo, Problem>, at: u64) {
        let (ok, message) = match result {
            Ok(info) => (true, info.credit()),
            Err(p) => (false, p.message()),
        };
        self.last_check = Some(LastCheck { at, model: model.to_string(), ok, message });
    }
}

/// Whether `id` looks like an OpenRouter model ID: `vendor/model`, maybe with a
/// `:variant`, no spaces.
pub fn valid_model_id(id: &str) -> bool {
    let Some((vendor, model)) = id.split_once('/') else { return false };
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@".contains(c));
    id.len() <= 200 && ok(vendor) && ok(model) && !vendor.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_the_coder_and_its_free_variant() {
        let c = OpenRouterConfig::default();
        assert_eq!(c.models, ["qwen/qwen3-coder", "qwen/qwen3-coder:free"]);
        assert_eq!(c.default_model(), "qwen/qwen3-coder");
        assert_eq!(c.settings(None), OpenRouterSettings { model: "qwen/qwen3-coder".into(), max_output_tokens: 8000 });
        assert_eq!(c.settings(Some("qwen/qwen3-coder:free")).model, "qwen/qwen3-coder:free");
    }

    #[test]
    fn adds_removes_and_picks_the_default() {
        let mut c = OpenRouterConfig::default();
        c.add_model(" moonshotai/kimi-k2 ").unwrap();
        c.add_model("moonshotai/kimi-k2").unwrap();
        assert_eq!(c.models.len(), 3);
        assert!(c.add_model("not a model").is_err());
        assert!(c.add_model("noslash").is_err());

        c.set_default("moonshotai/kimi-k2");
        assert_eq!(c.default_model(), "moonshotai/kimi-k2");
        c.set_default("not/mine");
        assert_eq!(c.default_model(), "moonshotai/kimi-k2");

        c.remove_model("moonshotai/kimi-k2").unwrap();
        assert_eq!(c.default_model(), "qwen/qwen3-coder");
        c.remove_model("qwen/qwen3-coder").unwrap();
        assert_eq!(c.default_model(), "qwen/qwen3-coder:free");
        let err = c.remove_model("qwen/qwen3-coder:free").unwrap_err();
        assert!(err.contains("at least one"), "{err}");
        assert_eq!(c.models, ["qwen/qwen3-coder:free"]);
    }

    #[test]
    fn a_default_that_went_missing_falls_back() {
        let c = OpenRouterConfig { default_model: "gone/model".into(), ..Default::default() };
        assert_eq!(c.default_model(), "qwen/qwen3-coder");
        let empty = OpenRouterConfig { models: Vec::new(), ..Default::default() };
        assert_eq!(empty.default_model(), DEFAULT_OPENROUTER_MODEL);
    }

    #[test]
    fn the_cap_is_a_number_in_range() {
        let mut c = OpenRouterConfig::default();
        c.set_max_output_tokens("16,000").unwrap();
        assert_eq!(c.max_output_tokens, 16000);
        assert!(c.set_max_output_tokens("lots").is_err());
        assert!(c.set_max_output_tokens("10").is_err());
        assert!(c.set_max_output_tokens("999999").is_err());
        assert_eq!(c.max_output_tokens, 16000);
    }

    #[test]
    fn model_ids() {
        for ok in ["qwen/qwen3-coder", "qwen/qwen3-coder:free", "openrouter/auto", "meta-llama/llama-3.1-8b-instruct"] {
            assert!(valid_model_id(ok), "{ok}");
        }
        for bad in ["", "qwen", "/x", "x/", "qwen/ coder", "a:b/c", "qwen/coder\n"] {
            assert!(!valid_model_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn remembers_the_last_check() {
        let mut c = OpenRouterConfig::default();
        let info = KeyInfo { limit: Some(10.0), limit_remaining: Some(4.5), ..Default::default() };
        c.record_check("qwen/qwen3-coder", &Ok(info), 100);
        let last = c.last_check.clone().unwrap();
        assert!(last.ok && last.message == "$4.50 left of $10.00" && last.at == 100);
        c.record_check("qwen/qwen3-coder", &Err(Problem::BadKey), 200);
        let last = c.last_check.unwrap();
        assert!(!last.ok && last.message.contains("401"));
    }

    #[test]
    fn round_trips_and_older_state_loads() {
        let c = OpenRouterConfig::default();
        let back: OpenRouterConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let old: OpenRouterConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old, OpenRouterConfig::default());
    }
}
