//! OpenRouter's model catalogue (`/api/v1/models`, no key needed), for adding
//! models to My models: names, prices and which ones are free.

use serde_json::Value;

const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// The most matches a search shows.
pub const SEARCH_LIMIT: usize = 40;

/// One model OpenRouter offers.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogModel {
    /// What sessions ask for, e.g. `qwen/qwen3-coder:free`.
    pub id: String,
    pub name: String,
    /// Dollars per million input / output tokens; `None` when OpenRouter
    /// doesn't say or it varies (a router like `openrouter/auto`).
    pub prompt: Option<f64>,
    pub completion: Option<f64>,
    /// The context window, in tokens.
    pub context: Option<u64>,
    /// It takes tool calls. Claude Code works through tools, so a model
    /// without them can only chat.
    pub tools: bool,
}

impl CatalogModel {
    /// A `:free` variant, or one that costs nothing either way. Free models
    /// are rate-limited, and many accounts may share them.
    pub fn is_free(&self) -> bool {
        self.id.ends_with(":free") || (self.prompt == Some(0.0) && self.completion == Some(0.0))
    }

    /// Its price in words: "Free", or "$0.22 in · $0.95 out per M tokens".
    pub fn price(&self) -> String {
        if self.is_free() {
            return "Free".into();
        }
        match (self.prompt, self.completion) {
            (Some(i), Some(o)) => format!("${} in · ${} out per M tokens", dollars(i), dollars(o)),
            _ => "Price varies".into(),
        }
    }

    /// The context window in words: "262K context".
    pub fn context_label(&self) -> Option<String> {
        let c = self.context.filter(|c| *c > 0)?;
        Some(if c >= 1_000_000 && c % 1_000_000 == 0 {
            format!("{}M context", c / 1_000_000)
        } else {
            format!("{}K context", (c + 500) / 1000)
        })
    }
}

/// Dollars with as many decimals as small prices need: 3, 0.22, 0.075.
fn dollars(d: f64) -> String {
    let s = if d >= 1.0 { format!("{d:.2}") } else { format!("{d:.3}") };
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.to_string() }
}

/// Fetches the catalogue with `curl`. Blocks on a request (a second or two).
pub fn catalogue() -> Result<Vec<CatalogModel>, String> {
    let out = crate::cli::command("curl")
        .args(["-fsSL", "--max-time", "30", MODELS_URL])
        .output()
        .map_err(|e| format!("failed to start `curl`: {e}"))?;
    if !out.status.success() {
        return Err(format!("Couldn't load OpenRouter's models: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// The catalogue from `/api/v1/models`'s body, sorted by ID.
pub(crate) fn parse(body: &str) -> Result<Vec<CatalogModel>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("OpenRouter's model list didn't parse: {e}"))?;
    let data = v["data"].as_array().ok_or("OpenRouter's model list has no models")?;
    // Per-token prices, as strings ("0.00000022"); "-1" means it varies.
    let per_million = |p: &Value| {
        let n = p.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| p.as_f64())?;
        (n >= 0.0 && n.is_finite()).then_some(n * 1_000_000.0)
    };
    let mut models: Vec<CatalogModel> = data
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            Some(CatalogModel {
                id: id.to_string(),
                name: m["name"].as_str().unwrap_or(id).trim().to_string(),
                prompt: per_million(&m["pricing"]["prompt"]),
                completion: per_million(&m["pricing"]["completion"]),
                context: m["context_length"].as_u64(),
                // Lists without the field are taken at their word: assume tools.
                tools: m["supported_parameters"].as_array().is_none_or(|p| p.iter().any(|v| v == "tools")),
            })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// The models matching `query`, best first, at most [`SEARCH_LIMIT`]: every
/// word of it must be in the ID or the name, any case. IDs that start with
/// the query come first, then IDs that hold it, then the rest.
pub fn search<'a>(models: &'a [CatalogModel], query: &str) -> Vec<&'a CatalogModel> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut found: Vec<(u8, &CatalogModel)> = models
        .iter()
        .filter_map(|m| {
            let (id, name) = (m.id.to_lowercase(), m.name.to_lowercase());
            if !words.iter().all(|w| id.contains(w) || name.contains(w)) {
                return None;
            }
            let rank = if query.is_empty() || id.starts_with(&query) {
                0
            } else if id.contains(&query) || id.split('/').nth(1).is_some_and(|n| n.starts_with(words[0])) {
                1
            } else {
                2
            };
            Some((rank, m))
        })
        .collect();
    // Models that can't use tools go last: Claude Code can't edit files with them.
    found.sort_by_key(|(rank, m)| (!m.tools, *rank, m.id.len()));
    found.into_iter().take(SEARCH_LIMIT).map(|(_, m)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"data":[
        {"id":"qwen/qwen3-coder","name":"Qwen: Qwen3 Coder 480B A35B","context_length":262144,
         "pricing":{"prompt":"0.00000022","completion":"0.00000095"}},
        {"id":"qwen/qwen3-coder:free","name":"Qwen: Qwen3 Coder 480B A35B (free)","context_length":262144,
         "pricing":{"prompt":"0","completion":"0"}},
        {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
         "pricing":{"prompt":"-1","completion":"-1"}, "supported_parameters":["temperature"]},
        {"id":"anthropic/claude-sonnet-4","name":"Anthropic: Claude Sonnet 4","context_length":1000000,
         "pricing":{"prompt":"0.000003","completion":"0.000015"}},
        {"name":"no id"},
        {"id":"qwen/qwen3-coder","name":"duplicate"}
    ]}"#;

    #[test]
    fn reads_the_catalogue() {
        let models = parse(BODY).unwrap();
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["anthropic/claude-sonnet-4", "openrouter/auto", "qwen/qwen3-coder", "qwen/qwen3-coder:free"]);
        let coder = &models[2];
        assert_eq!(coder.price(), "$0.22 in · $0.95 out per M tokens");
        assert_eq!(coder.context_label().as_deref(), Some("262K context"));
        assert!(!coder.is_free());
        assert!(models[3].is_free());
        assert_eq!(models[3].price(), "Free");
        assert_eq!(models[1].price(), "Price varies");
        assert_eq!(models[1].context_label().as_deref(), Some("2M context"));
        assert!(!models[1].tools && coder.tools);
        assert_eq!(models[0].price(), "$3 in · $15 out per M tokens");
    }

    #[test]
    fn bad_lists_say_so() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"error":"x"}"#).is_err());
        assert_eq!(parse(r#"{"data":[]}"#).unwrap(), []);
    }

    #[test]
    fn searches_ids_and_names() {
        let models = parse(BODY).unwrap();
        let ids = |q| search(&models, q).into_iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids("qwen"), ["qwen/qwen3-coder", "qwen/qwen3-coder:free"]);
        assert_eq!(ids("coder free"), ["qwen/qwen3-coder:free"]);
        assert_eq!(ids("SONNET"), ["anthropic/claude-sonnet-4"]);
        assert_eq!(ids("router"), ["openrouter/auto"]);
        // No tools: last, even when its ID matches best.
        assert_eq!(ids("o").last().copied(), Some("openrouter/auto"));
        assert!(ids("nothing-like-this").is_empty());
        assert_eq!(ids("").len(), 4);
    }
}
