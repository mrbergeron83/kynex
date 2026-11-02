//! What Grok reports about tokens and cost, in the shapes a session reads
//! from `claude`: the context in use after each model call, and each turn's
//! tokens and cost on its `result`.
//!
//! Grok counts cached tokens inside its input (`inputTokens` includes
//! `cachedReadTokens`); Claude's input leaves them out, so they're taken out
//! here. Cost comes in USD ticks, 1e10 to the dollar.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

/// Grok's ticks per US dollar.
const TICKS_PER_USD: f64 = 1e10;

/// The session's models: the one in use, each one's context window, and the
/// list for the model picker with the effort levels each takes.
#[derive(Debug, Default)]
pub(crate) struct Models {
    pub current: String,
    windows: HashMap<String, u64>,
    /// Each model's own effort level, for going back to the default.
    default_efforts: HashMap<String, String>,
    pub list: Vec<crate::session::ModelInfo>,
    /// The list changed since it was last taken.
    pub fresh: bool,
}

impl Models {
    /// Takes in a model list (`session/new`'s `models`, `_x.ai/models/update`'s params).
    pub fn read(&mut self, models: &Value) {
        if let Some(id) = models["currentModelId"].as_str().filter(|m| !m.is_empty()) {
            self.current = id.to_string();
        }
        let Some(available) = models["availableModels"].as_array() else { return };
        let mut list = Vec::new();
        for m in available {
            let Some(id) = m["modelId"].as_str() else { continue };
            if let Some(window) = m["_meta"]["totalContextTokens"].as_u64() {
                self.windows.insert(id.to_string(), window);
            }
            let efforts = m["_meta"]["reasoningEfforts"].as_array().cloned().unwrap_or_default();
            if let Some(default) = efforts.iter().find(|e| e["default"] == true).and_then(|e| e["value"].as_str()) {
                self.default_efforts.insert(id.to_string(), default.to_string());
            }
            list.push(crate::session::ModelInfo {
                value: id.to_string(),
                name: m["name"].as_str().unwrap_or(id).to_string(),
                description: m["description"].as_str().unwrap_or("").to_string(),
                efforts: efforts.iter().filter_map(|e| e["value"].as_str().map(str::to_string)).collect(),
            });
        }
        if !list.is_empty() && list != self.list {
            self.list = list;
            self.fresh = true;
        }
    }

    /// The effort the current model starts on, as Grok says.
    pub fn default_effort(&self) -> Option<&str> {
        self.default_efforts.get(&self.current).map(String::as_str)
    }

    pub fn window(&self) -> Option<u64> {
        self.windows.get(&self.current).copied()
    }
}

/// A model call's usage (`response_completed`, snake case) as the empty
/// assistant message that carries it, so the context meter moves mid-turn.
pub(crate) fn context(model: &str, usage: &Value) -> Option<Value> {
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let (input, cached) = (n("input_tokens"), n("cache_read_input_tokens"));
    if input == 0 {
        return None;
    }
    Some(json!({ "type": "assistant", "message": { "model": model, "content": [], "usage": {
        "input_tokens": input.saturating_sub(cached),
        "cache_read_input_tokens": cached,
        "cache_creation_input_tokens": n("cache_creation_input_tokens"),
        "output_tokens": n("output_tokens"),
    } } }))
}

/// A turn's usage (`session/prompt`'s `_meta.usage`) as the `result` fields
/// `claude` sends: the turn's tokens and cost per model, the session's running
/// cost (added to `total`), and the model's context window.
pub(crate) fn turn(usage: &Value, models: &Models, total: &mut f64) -> Map<String, Value> {
    let entry = |u: &Value| {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let cached = n("cachedReadTokens");
        let cost = u["costUsdTicks"].as_i64().filter(|t| *t > 0).map_or(0.0, |t| t as f64 / TICKS_PER_USD);
        (
            json!({
                "inputTokens": n("inputTokens").saturating_sub(cached),
                "outputTokens": n("outputTokens"),
                "cacheReadInputTokens": cached,
                "cacheCreationInputTokens": n("cacheCreationTokens"),
                "costUSD": cost,
            }),
            cost,
        )
    };
    let mut by_model = Map::new();
    let mut cost = 0.0;
    match usage["modelUsage"].as_object().filter(|m| !m.is_empty()) {
        Some(models) => {
            for (model, u) in models {
                let (e, c) = entry(u);
                by_model.insert(model.clone(), e);
                cost += c;
            }
        }
        None if usage.is_object() => {
            let (e, c) = entry(usage);
            by_model.insert(models.current.clone(), e);
            cost = c;
        }
        None => {}
    }
    *total += cost;
    let mut out = Map::new();
    out.insert("total_cost_usd".into(), json!(*total));
    if !by_model.is_empty() {
        out.insert("model_usage_since_last_result".into(), Value::Object(by_model));
    }
    if let Some(window) = models.window() {
        out.insert("modelUsage".into(), json!({ models.current.clone(): { "contextWindow": window } }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Models {
        let mut m = Models::default();
        m.read(&json!({ "currentModelId": "grok-4.7", "availableModels": [
            { "modelId": "grok-4.7", "name": "Grok 4.7", "description": "Frontier", "_meta": {
                "totalContextTokens": 256000, "reasoningEfforts": [
                    { "value": "high", "default": true }, { "value": "medium", "default": false }] } },
            { "modelId": "grok-4.7-build-fast", "_meta": { "totalContextTokens": 128000 } }
        ] }));
        m
    }

    #[test]
    fn the_list_names_each_model_and_its_efforts() {
        let mut m = models();
        assert!(m.fresh);
        assert_eq!(m.list[0].name, "Grok 4.7");
        assert_eq!(m.list[0].efforts, ["high", "medium"]);
        // A model with no name is named by its id.
        assert_eq!(m.list[1].name, "grok-4.7-build-fast");
        assert_eq!(m.default_effort(), Some("high"));
        // The same list again isn't news.
        m.fresh = false;
        m.read(&json!({ "availableModels": [
            { "modelId": "grok-4.7", "name": "Grok 4.7", "description": "Frontier", "_meta": {
                "totalContextTokens": 256000, "reasoningEfforts": [
                    { "value": "high", "default": true }, { "value": "medium", "default": false }] } },
            { "modelId": "grok-4.7-build-fast", "_meta": { "totalContextTokens": 128000 } }
        ] }));
        assert!(!m.fresh);
    }

    #[test]
    fn knows_the_current_models_window() {
        let mut m = models();
        assert_eq!(m.window(), Some(256_000));
        m.read(&json!({ "currentModelId": "grok-4.7-build-fast" }));
        assert_eq!(m.window(), Some(128_000));
        m.current = "unknown".into();
        assert_eq!(m.window(), None);
    }

    #[test]
    fn a_model_call_moves_the_context_with_cache_reads_apart() {
        let usage = json!({ "input_tokens": 18838, "output_tokens": 113, "cache_read_input_tokens": 1664 });
        let v = context("grok-4.7", &usage).unwrap();
        let u = &v["message"]["usage"];
        assert_eq!((u["input_tokens"].as_u64(), u["cache_read_input_tokens"].as_u64()), (Some(17174), Some(1664)));
        assert_eq!(v["message"]["model"], "grok-4.7");
        assert!(context("m", &json!({})).is_none());
    }

    #[test]
    fn a_turn_reports_tokens_and_cost_in_dollars() {
        let usage = json!({ "inputTokens": 62282, "outputTokens": 342, "cachedReadTokens": 42880,
            "costUsdTicks": 622960000, "modelUsage": { "grok-4.7-build": {
                "inputTokens": 62282, "outputTokens": 342, "cachedReadTokens": 42880, "costUsdTicks": 622960000 } } });
        let mut total = 0.01;
        let out = turn(&usage, &models(), &mut total);
        let m = &out["model_usage_since_last_result"]["grok-4.7-build"];
        assert_eq!(m["inputTokens"], 19402);
        assert_eq!(m["cacheReadInputTokens"], 42880);
        assert!((m["costUSD"].as_f64().unwrap() - 0.062296).abs() < 1e-9);
        assert!((total - 0.072296).abs() < 1e-9);
        assert_eq!(out["modelUsage"]["grok-4.7"]["contextWindow"], 256000);

        // Into a session: the turn's tokens and cost add up as Claude's do.
        let mut spent = crate::spent::Spent::default();
        spent.add_turn(&Value::Object(out));
        assert_eq!((spent.input, spent.output, spent.cache_read), (19402, 342, 42880));
        assert!((spent.cost - 0.062296).abs() < 1e-9);
    }

    #[test]
    fn a_turn_without_a_model_breakdown_or_cost_still_counts() {
        let usage = json!({ "inputTokens": 100, "outputTokens": 5, "costUsdTicks": 0 });
        let mut total = 0.0;
        let out = turn(&usage, &models(), &mut total);
        assert_eq!(out["model_usage_since_last_result"]["grok-4.7"]["inputTokens"], 100);
        assert_eq!(total, 0.0);
        let none = turn(&Value::Null, &Models::default(), &mut total);
        assert!(none.get("model_usage_since_last_result").is_none() && none.get("modelUsage").is_none());
    }
}
