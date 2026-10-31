//! Checking an OpenRouter key before a session needs it: is it valid, how much
//! credit is left, and does the chosen model answer. Uses `curl`, like the
//! update check, so Kynex needs no HTTP stack. The key goes to curl on stdin
//! (`--config -`), never as an argument another process could read.

pub(crate) mod catalogue;
mod config;
pub mod import;

pub use catalogue::{CatalogModel, SEARCH_LIMIT, catalogue, search};
pub use config::{LastCheck, MAX_OUTPUT_TOKENS, MIN_OUTPUT_TOKENS, OpenRouterConfig, valid_model_id};

use std::io::Write;
use std::process::Stdio;

use serde_json::{Value, json};

const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
/// The Anthropic-compatible endpoint the CLI itself calls through OpenRouter.
const MESSAGES_URL: &str = "https://openrouter.ai/api/v1/messages";
/// What the test request may produce: enough for a word, so it costs next to nothing.
const PROBE_MAX_TOKENS: u32 = 16;

/// What `/api/v1/key` says about a key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyInfo {
    /// The name given to the key on openrouter.ai, if any.
    pub label: Option<String>,
    /// Dollars spent with it so far.
    pub usage: f64,
    /// Its spending limit in dollars; `None` when it has none.
    pub limit: Option<f64>,
    /// Dollars left under that limit; `None` when it has none.
    pub limit_remaining: Option<f64>,
    /// An account that never bought credits: free models only, with low rate limits.
    pub is_free_tier: bool,
}

impl KeyInfo {
    /// The credit in a few characters, for the footer: "$4.20 left", "$1.50 used", "Free".
    pub fn short(&self) -> String {
        match self.limit_remaining {
            Some(left) => format!("${left:.2} left"),
            None if self.is_free_tier => "Free tier".into(),
            None => format!("${:.2} used", self.usage),
        }
    }

    /// The credit left, in words: "$4.20 left of $10.00", "$1.50 used, no limit".
    pub fn credit(&self) -> String {
        match (self.limit_remaining, self.limit) {
            (Some(left), Some(limit)) => format!("${left:.2} left of ${limit:.2}"),
            (Some(left), None) => format!("${left:.2} left"),
            _ if self.is_free_tier => "free tier: free models only".into(),
            _ => format!("${:.2} used, no limit on this key", self.usage),
        }
    }
}

/// Why a check failed, each with a message the user can act on ([`Problem::message`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// 401: wrong or revoked.
    BadKey,
    /// 402: not enough credit for the request.
    NoCredits,
    /// 429: too many requests, common on `:free` models.
    RateLimited,
    /// OpenRouter has no such model.
    ModelNotFound(String),
    /// The key isn't one curl can be given (a line break or control character in it).
    Malformed,
    /// curl couldn't run or reach OpenRouter.
    Network(String),
    /// Any other answer: its HTTP status and OpenRouter's message.
    Other(u16, String),
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Problem::BadKey => "OpenRouter didn't accept the key (401): it's wrong or has been revoked. \
                 Check it at openrouter.ai/keys."
                .into(),
            Problem::NoCredits => "Out of credits (402): OpenRouter can't cover a request of this size. \
                 Add credits, pick a free model (one ending in :free), or lower the output cap."
                .into(),
            Problem::RateLimited => "Rate-limited (429): too many requests right now, which is common on \
                 :free models. Wait a minute, or pick a paid model."
                .into(),
            Problem::ModelNotFound(model) => {
                format!("OpenRouter has no model \"{model}\". Check the ID at openrouter.ai/models.")
            }
            Problem::Malformed => "That doesn't look like an OpenRouter key: it has a line break in it.".into(),
            Problem::Network(e) => format!("Couldn't reach OpenRouter: {e}"),
            Problem::Other(status, msg) if msg.is_empty() => format!("OpenRouter answered {status}."),
            Problem::Other(status, msg) => format!("OpenRouter answered {status}: {msg}"),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// A saved key as Settings shows it: its prefix and last four characters,
/// `sk-or-v1-…3f9a`, enough to tell keys apart and never enough to use one.
pub fn key_hint(key: &str) -> String {
    let key = key.trim();
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 16 {
        return "…".repeat(usize::from(!chars.is_empty()));
    }
    let prefix: String = match key.match_indices('-').nth(2) {
        Some((i, _)) if i < 12 => key[..=i].to_string(),
        _ => chars[..3].iter().collect(),
    };
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{tail}")
}

/// What OpenRouter says about `key` now: its credit, with no test request.
/// Blocks on one request.
pub fn credit(key: &str) -> Result<KeyInfo, Problem> {
    let (status, body) = request(key, KEY_URL, None)?;
    answer(status, &body, "")?;
    key_info(&body).ok_or_else(|| Problem::Other(status, "no key details in the answer".into()))
}

/// Checks `key` and `model`: the key's validity and credit, then one tiny
/// request to `model` on the endpoint sessions use. Blocks on two requests
/// (a few seconds).
pub fn check(key: &str, model: &str) -> Result<KeyInfo, Problem> {
    let (status, body) = request(key, KEY_URL, None)?;
    answer(status, &body, model)?;
    let info = key_info(&body).ok_or_else(|| Problem::Other(status, "no key details in the answer".into()))?;
    let probe = json!({
        "model": model,
        "max_tokens": PROBE_MAX_TOKENS,
        "messages": [{ "role": "user", "content": "Reply with OK." }],
    });
    let (status, body) = request(key, MESSAGES_URL, Some(&probe))?;
    answer(status, &body, model)?;
    Ok(info)
}

/// One request through curl, its config (the key included) on stdin; the
/// HTTP status and body.
fn request(key: &str, url: &str, body: Option<&Value>) -> Result<(u16, String), Problem> {
    let config = curl_config(key, url, body).ok_or(Problem::Malformed)?;
    let mut child = crate::cli::command("curl")
        .args(["-sS", "--max-time", "30", "--config", "-", "-w", "\n%{http_code}"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| Problem::Network(format!("failed to start `curl`: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // Dropped at the end of this block, so curl sees the end of its config.
        stdin.write_all(config.as_bytes()).map_err(|e| Problem::Network(format!("curl: {e}")))?;
    }
    let out = child.wait_with_output().map_err(|e| Problem::Network(format!("curl: {e}")))?;
    if !out.status.success() {
        return Err(Problem::Network(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    split_status(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| Problem::Network("curl gave no HTTP status".into()))
}

/// curl's config for one request: the URL, the key as a bearer token, and a
/// JSON body if any. `None` if the key holds a line break or control
/// character, which would end its line of the config early.
pub(crate) fn curl_config(key: &str, url: &str, body: Option<&Value>) -> Option<String> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(char::is_control) {
        return None;
    }
    let mut config = format!("url = {}\n", quoted(url));
    config += &format!("header = {}\n", quoted(&format!("Authorization: Bearer {key}")));
    if let Some(body) = body {
        config += &format!("header = {}\n", quoted("Content-Type: application/json"));
        config += &format!("header = {}\n", quoted("anthropic-version: 2023-06-01"));
        config += &format!("data = {}\n", quoted(&body.to_string()));
    }
    Some(config)
}

/// A curl config string: in double quotes, with `\` and `"` escaped and
/// line breaks written as `\n`.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The body and the status curl's `-w "\n%{http_code}"` put after it.
pub(crate) fn split_status(output: &str) -> Option<(u16, String)> {
    let (body, status) = output.rsplit_once('\n')?;
    Some((status.trim().parse().ok()?, body.to_string()))
}

/// What an answer means: fine, or the [`Problem`] it reports. OpenRouter
/// sometimes reports an error in a 200's body, so the body is read too.
pub(crate) fn answer(status: u16, body: &str, model: &str) -> Result<(), Problem> {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = &v["error"];
    let code = error["code"].as_u64().and_then(|c| u16::try_from(c).ok());
    let status = if (200..300).contains(&status) && error.is_object() { code.unwrap_or(500) } else { status };
    if (200..300).contains(&status) {
        return Ok(());
    }
    let msg = error["message"].as_str().or_else(|| error.as_str()).unwrap_or("").trim().to_string();
    let lower = msg.to_ascii_lowercase();
    let no_model = lower.contains("not a valid model")
        || lower.contains("no endpoints found")
        || (lower.contains("model") && (lower.contains("not found") || lower.contains("does not exist")));
    Err(match status {
        401 => Problem::BadKey,
        402 => Problem::NoCredits,
        429 => Problem::RateLimited,
        400 | 404 if no_model => Problem::ModelNotFound(model.to_string()),
        _ => Problem::Other(status, msg.chars().take(300).collect()),
    })
}

/// The key's details from `/api/v1/key`'s body.
pub(crate) fn key_info(body: &str) -> Option<KeyInfo> {
    let v: Value = serde_json::from_str(body).ok()?;
    let d = v.get("data")?.as_object()?;
    Some(KeyInfo {
        label: d.get("label").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        usage: d.get("usage").and_then(Value::as_f64).unwrap_or(0.0),
        limit: d.get("limit").and_then(Value::as_f64),
        limit_remaining: d.get("limit_remaining").and_then(Value::as_f64),
        is_free_tier: d.get("is_free_tier").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_goes_in_the_config_quoted() {
        let config = curl_config(" sk-or-v1-abc\n", KEY_URL, None).unwrap();
        assert_eq!(
            config,
            "url = \"https://openrouter.ai/api/v1/key\"\nheader = \"Authorization: Bearer sk-or-v1-abc\"\n"
        );
        let body = json!({ "model": "qwen/qwen3-coder", "messages": [{ "content": "say \"hi\"\n" }] });
        let config = curl_config("k", MESSAGES_URL, Some(&body)).unwrap();
        assert!(config.contains("header = \"anthropic-version: 2023-06-01\"\n"), "{config}");
        // The body stays on one line, its quotes and escapes escaped again.
        let data = config.lines().find(|l| l.starts_with("data = ")).unwrap();
        assert!(data.contains(r#"\"model\":\"qwen/qwen3-coder\""#), "{data}");
        assert!(data.contains(r#"say \\\"hi\\\"\\n"#), "{data}");
    }

    #[test]
    fn a_key_that_would_break_the_config_is_refused() {
        assert_eq!(curl_config("sk-or\nurl = \"https://evil.example\"", KEY_URL, None), None);
        assert_eq!(curl_config("sk\u{7}or", KEY_URL, None), None);
        assert_eq!(curl_config("   ", KEY_URL, None), None);
    }

    #[test]
    fn hints_show_the_prefix_and_last_four() {
        assert_eq!(key_hint("sk-or-v1-0123456789abcdef3f9a"), "sk-or-v1-…3f9a");
        assert_eq!(key_hint("abcdefghijklmnopqrstuvwxyz"), "abc…wxyz");
        assert_eq!(key_hint("short"), "…");
        assert_eq!(key_hint(""), "");
    }

    #[test]
    fn splits_the_status_from_the_body() {
        assert_eq!(split_status("{\"a\":1}\n200"), Some((200, "{\"a\":1}".into())));
        assert_eq!(split_status("line one\nline two\n401"), Some((401, "line one\nline two".into())));
        assert_eq!(split_status("\n000"), Some((0, String::new())));
        assert_eq!(split_status("no status"), None);
    }

    #[test]
    fn statuses_become_problems() {
        let err = |status, body: &str| answer(status, body, "qwen/x").unwrap_err();
        assert_eq!(answer(200, r#"{"data":{}}"#, "m"), Ok(()));
        assert_eq!(err(401, r#"{"error":{"code":401,"message":"User not found."}}"#), Problem::BadKey);
        assert_eq!(err(402, r#"{"error":{"message":"Insufficient credits"}}"#), Problem::NoCredits);
        assert_eq!(err(429, ""), Problem::RateLimited);
        assert_eq!(
            err(400, r#"{"error":{"message":"qwen/x is not a valid model ID"}}"#),
            Problem::ModelNotFound("qwen/x".into())
        );
        assert_eq!(
            err(404, r#"{"error":{"message":"No endpoints found for qwen/x."}}"#),
            Problem::ModelNotFound("qwen/x".into())
        );
        // The Anthropic-shaped error the messages endpoint may send.
        assert_eq!(
            err(404, r#"{"type":"error","error":{"type":"not_found_error","message":"model: qwen/x not found"}}"#),
            Problem::ModelNotFound("qwen/x".into())
        );
        assert_eq!(err(500, r#"{"error":{"message":"Internal"}}"#), Problem::Other(500, "Internal".into()));
        assert_eq!(err(503, "<html>"), Problem::Other(503, String::new()));
    }

    #[test]
    fn an_error_inside_a_200_counts() {
        let body = r#"{"error":{"code":402,"message":"Prompt tokens limit exceeded"}}"#;
        assert_eq!(answer(200, body, "m"), Err(Problem::NoCredits));
    }

    #[test]
    fn every_problem_says_what_to_do() {
        assert!(Problem::BadKey.message().contains("wrong or has been revoked"));
        let no_credits = Problem::NoCredits.message();
        assert!(no_credits.contains(":free") && no_credits.contains("output cap"), "{no_credits}");
        assert!(Problem::RateLimited.message().contains(":free"));
        assert!(Problem::ModelNotFound("a/b".into()).message().contains("\"a/b\""));
        assert_eq!(Problem::Other(418, String::new()).to_string(), "OpenRouter answered 418.");
    }

    #[test]
    fn reads_key_details() {
        let body = r#"{"data":{"label":"sk-or-v1-abc...xyz","usage":1.25,"limit":10,"limit_remaining":8.75,
                       "is_free_tier":false,"rate_limit":{"requests":-1,"interval":"10s"}}}"#;
        let info = key_info(body).unwrap();
        assert_eq!(info.label.as_deref(), Some("sk-or-v1-abc...xyz"));
        assert_eq!((info.usage, info.limit, info.limit_remaining), (1.25, Some(10.0), Some(8.75)));
        assert_eq!(info.credit(), "$8.75 left of $10.00");
        assert_eq!(info.short(), "$8.75 left");

        let unlimited = key_info(r#"{"data":{"usage":1.5,"limit":null,"limit_remaining":null}}"#).unwrap();
        assert_eq!(unlimited.credit(), "$1.50 used, no limit on this key");
        assert_eq!(unlimited.short(), "$1.50 used");
        let free = key_info(r#"{"data":{"usage":0,"is_free_tier":true}}"#).unwrap();
        assert_eq!(free.credit(), "free tier: free models only");

        assert_eq!(key_info("not json"), None);
        assert_eq!(key_info(r#"{"error":{}}"#), None);
    }

    #[test]
    #[ignore = "needs network"]
    fn a_wrong_key_is_refused_live() {
        assert_eq!(check("sk-or-v1-kynex-not-a-real-key", "qwen/qwen3-coder"), Err(Problem::BadKey));
    }

    /// Run with `OPENROUTER_API_KEY` set: `cargo test -p kynex-core -- --ignored checks_a_real_key`.
    #[test]
    #[ignore = "needs network and OPENROUTER_API_KEY; sends one tiny request"]
    fn checks_a_real_key() {
        let key = std::env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY");
        let info = check(&key, "qwen/qwen3-coder").unwrap();
        eprintln!("{}", info.credit());
        assert_eq!(check(&key, "kynex/no-such-model"), Err(Problem::ModelNotFound("kynex/no-such-model".into())));
    }
}
