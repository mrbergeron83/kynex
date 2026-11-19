//! What an ACP agent reports about models, tokens and cost, in the shapes a
//! session reads from `claude`: the context in use after each model call, and
//! each turn's tokens and cost on its `result`.
//!
//! Grok counts cached tokens inside its input (`inputTokens` includes
//! `cachedReadTokens`); Claude's input leaves them out, so they're taken out
//! here. Its cost comes in USD ticks, 1e10 to the dollar. Antigravity counts
//! them apart already, sends no cost, and names its models by placeholder ids
//! (`MODEL_PLACEHOLDER_M364`) whose name is the real one (`google/gemini-4-hydrogen`).

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
    /// Antigravity: each model's name (what Kynex calls it) -> the id it goes by on the wire.
    wire: HashMap<String, String>,
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

    /// Takes in Antigravity's model list (`session/new`'s `models`): named by
    /// their real names, which stay the same from run to run, unlike the ids.
    pub fn read_named(&mut self, models: &Value) {
        let Some(available) = models["availableModels"].as_array() else { return };
        let mut list = Vec::new();
        for m in available {
            let Some(id) = m["modelId"].as_str() else { continue };
            let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id);
            self.wire.insert(name.to_string(), id.to_string());
            list.push(crate::session::ModelInfo {
                value: name.to_string(),
                name: name.to_string(),
                description: m["description"].as_str().unwrap_or("").to_string(),
                efforts: Vec::new(),
            });
        }
        if let Some(id) = models["currentModelId"].as_str() {
            self.current = self.name_of(id);
        }
        if !list.is_empty() && list != self.list {
            self.list = list;
            self.fresh = true;
        }
    }

    /// Takes in Antigravity's `configOptions`: the model in use.
    pub fn read_config(&mut self, options: &Value) {
        let model = options.as_array().into_iter().flatten().find(|o| o["id"] == "model");
        if let Some(id) = model.and_then(|o| o["currentValue"].as_str()) {
            self.current = self.name_of(id);
        }
    }

    /// The name of the model with wire id `id`, or `id` itself.
    fn name_of(&self, id: &str) -> String {
        self.wire.iter().find(|(_, w)| *w == id).map_or_else(|| id.to_string(), |(n, _)| n.clone())
    }

    /// The id `model` (a name, or an id) goes by on the wire.
    pub fn wire_id(&self, model: &str) -> String {
        self.wire.get(model).cloned().unwrap_or_else(|| model.to_string())
    }

    /// The current model's context window, as the agent reported it.
    pub fn set_window(&mut self, size: u64) {
        if size > 0 {
            self.windows.insert(self.current.clone(), size);
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

/// Antigravity's context in use (`usage_update`'s `used`) as the empty assistant
/// message that carries it, all of it as input.
pub(crate) fn context_used(model: &str, used: u64) -> Value {
    json!({ "type": "assistant", "message": { "model": model, "content": [], "usage": {
        "input_tokens": used, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 0,
    } } })
}

/// A turn's usage (Grok's `session/prompt` `_meta.usage`, Antigravity's
/// `usage`) as the `result` fields `claude` sends: the turn's tokens and cost
/// per model, the session's running cost (added to `total`), and the model's
/// context window. `cached_in_input`: the input count includes cache reads (Grok).
pub(crate) fn turn(usage: &Value, models: &Models, total: &mut f64, cached_in_input: bool) -> Map<String, Value> {
    let entry = |u: &Value| {
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let cached = n("cachedReadTokens");
        let input = if cached_in_input { n("inputTokens").saturating_sub(cached) } else { n("inputTokens") };
        let cost = u["costUsdTicks"].as_i64().filter(|t| *t > 0).map_or(0.0, |t| t as f64 / TICKS_PER_USD);
        (
            json!({
                "inputTokens": input,
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
        let out = turn(&usage, &models(), &mut total, true);
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
        let out = turn(&usage, &models(), &mut total, true);
        assert_eq!(out["model_usage_since_last_result"]["grok-4.7"]["inputTokens"], 100);
        assert_eq!(total, 0.0);
        let none = turn(&Value::Null, &Models::default(), &mut total, true);
        assert!(none.get("model_usage_since_last_result").is_none() && none.get("modelUsage").is_none());
    }

    #[test]
    fn antigravity_models_go_by_name_and_their_ids_on_the_wire() {
        let mut m = Models::default();
        m.read_named(&json!({ "availableModels": [
            { "modelId": "MODEL_PLACEHOLDER_M364", "name": "google/gemini-4-hydrogen" },
            { "modelId": "MODEL_PLACEHOLDER_M365", "name": "google/gemini-4-carbon" },
        ] }));
        let names: Vec<_> = m.list.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(names, ["google/gemini-4-hydrogen", "google/gemini-4-carbon"]);
        assert_eq!(m.wire_id("google/gemini-4-carbon"), "MODEL_PLACEHOLDER_M365");
        assert_eq!(m.wire_id("unknown/model"), "unknown/model");
        m.read_config(&json!([{ "id": "model", "currentValue": "MODEL_PLACEHOLDER_M364" }]));
        assert_eq!(m.current, "google/gemini-4-hydrogen");
        m.set_window(200_000);
        assert_eq!(m.window(), Some(200_000));
    }

    #[test]
    fn antigravity_turns_count_cache_reads_apart_already() {
        let usage = json!({ "inputTokens": 13320, "outputTokens": 243, "cachedReadTokens": 13785 });
        let m = Models { current: "google/gemini-4-carbon".into(), ..Default::default() };
        let mut total = 0.0;
        let out = turn(&usage, &m, &mut total, false);
        let u = &out["model_usage_since_last_result"]["google/gemini-4-carbon"];
        assert_eq!((u["inputTokens"].as_u64(), u["cacheReadInputTokens"].as_u64()), (Some(13320), Some(13785)));
        assert_eq!(total, 0.0);
    }
}
