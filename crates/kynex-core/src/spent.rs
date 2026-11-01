//! What a session has used since its process started: tokens and the CLI's
//! cost estimate, its subagents included, added up from each turn's `result`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spent {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// The CLI's estimate in USD, at list prices.
    pub cost: f64,
    /// The parts of `cost` that went to each model family, so what a mix of models cost
    /// (careful mode's Opus planning and Haiku doing) can be told apart.
    pub opus_cost: f64,
    pub sonnet_cost: f64,
    pub haiku_cost: f64,
}

impl Spent {
    /// Every token sent and received, cache reads included.
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// Adds a `result` line's turn: `model_usage_since_last_result` (every
    /// model the turn used, subagents included), else its main-thread `usage`.
    pub fn add_turn(&mut self, result: &Value) {
        if let Some(models) = result["model_usage_since_last_result"].as_object() {
            for (model, u) in models {
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                self.input += n("inputTokens");
                self.output += n("outputTokens");
                self.cache_read += n("cacheReadInputTokens");
                self.cache_write += n("cacheCreationInputTokens");
                let cost = u["costUSD"].as_f64().unwrap_or(0.0);
                self.cost += cost;
                let model = model.to_lowercase();
                if model.contains("opus") {
                    self.opus_cost += cost;
                } else if model.contains("sonnet") {
                    self.sonnet_cost += cost;
                } else if model.contains("haiku") {
                    self.haiku_cost += cost;
                }
            }
            return;
        }
        let u = &result["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        self.input += n("input_tokens");
        self.output += n("output_tokens");
        self.cache_read += n("cache_read_input_tokens");
        self.cache_write += n("cache_creation_input_tokens");
    }

    pub fn plus(self, other: Spent) -> Spent {
        Spent {
            input: self.input + other.input,
            output: self.output + other.output,
            cache_read: self.cache_read + other.cache_read,
            cache_write: self.cache_write + other.cache_write,
            cost: self.cost + other.cost,
            opus_cost: self.opus_cost + other.opus_cost,
            sonnet_cost: self.sonnet_cost + other.sonnet_cost,
            haiku_cost: self.haiku_cost + other.haiku_cost,
        }
    }

    /// What each model family cost, as (name, USD), the pricier first, leaving out the
    /// ones that cost nothing.
    pub fn by_model(&self) -> Vec<(&'static str, f64)> {
        [("Opus", self.opus_cost), ("Sonnet", self.sonnet_cost), ("Haiku", self.haiku_cost)]
            .into_iter()
            .filter(|(_, c)| *c > 0.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn adds_every_model_of_a_turn() {
        let mut s = Spent::default();
        s.add_turn(&json!({
            "usage": { "input_tokens": 999 },
            "model_usage_since_last_result": {
                "claude-haiku-5-5": { "inputTokens": 2, "outputTokens": 13, "cacheReadInputTokens": 100,
                    "cacheCreationInputTokens": 600, "costUSD": 0.01 },
                "claude-opus-5-5": { "inputTokens": 5, "outputTokens": 7, "costUSD": 0.5 },
            },
        }));
        assert_eq!((s.input, s.output, s.cache_read, s.cache_write), (7, 20, 100, 600));
        assert_eq!(s.tokens(), 727);
        assert!((s.cost - 0.51).abs() < 1e-9);
        // Each family's share, the pricier first; a family that cost nothing is left out.
        assert_eq!(s.by_model(), [("Opus", 0.5), ("Haiku", 0.01)]);
        let twice = s.plus(s);
        assert_eq!(twice.by_model(), [("Opus", 1.0), ("Haiku", 0.02)]);
    }

    #[test]
    fn falls_back_to_the_turns_usage() {
        let mut s = Spent::default();
        s.add_turn(&json!({ "usage": { "input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 5 } }));
        s.add_turn(&json!({ "type": "result" }));
        assert_eq!(s.tokens(), 12);
        assert_eq!(s.cost, 0.0);
    }
}
