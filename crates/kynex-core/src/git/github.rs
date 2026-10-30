//! What GitHub knows about a repository's branches, through the `gh` CLI (its
//! login, never a token of Kynex's): each branch's pull request, and which
//! branches are protected. Everything here runs `gh` and blocks; without `gh`,
//! or signed out, it errs and the Git view does without.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// A pull request's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// How a pull request's checks stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checks {
    Passing,
    Failing,
    Pending,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub state: PrState,
    pub checks: Option<Checks>,
    pub url: String,
}

/// `gh` in `root`, never asking anything.
fn gh(root: &Path, args: &[&str]) -> Result<String, String> {
    if crate::wsl::parse(root).is_some() {
        return Err("GitHub lookups don't run for WSL folders".into());
    }
    let mut cmd = Command::new("gh");
    cmd.args(args).current_dir(root).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("Couldn't run gh: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Each branch's newest pull request, by its head branch name. Blocks.
pub fn pull_requests(root: &Path) -> Result<HashMap<String, PullRequest>, String> {
    let json = gh(
        root,
        &[
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "300",
            "--json",
            "headRefName,number,state,isDraft,statusCheckRollup,url",
        ],
    )?;
    Ok(parse_prs(&json))
}

pub(crate) fn parse_prs(json: &str) -> HashMap<String, PullRequest> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else { return HashMap::new() };
    let mut out: HashMap<String, PullRequest> = HashMap::new();
    for pr in &list {
        let (Some(head), Some(number)) = (pr["headRefName"].as_str(), pr["number"].as_u64()) else { continue };
        let state = match (pr["state"].as_str(), pr["isDraft"].as_bool()) {
            (Some("MERGED"), _) => PrState::Merged,
            (Some("CLOSED"), _) => PrState::Closed,
            (_, Some(true)) => PrState::Draft,
            _ => PrState::Open,
        };
        let pr = PullRequest {
            number,
            state,
            checks: checks(&pr["statusCheckRollup"]),
            url: pr["url"].as_str().unwrap_or("").into(),
        };
        // The newest PR from a branch is the one that matters.
        if out.get(head).is_none_or(|old| old.number < pr.number) {
            out.insert(head.to_string(), pr);
        }
    }
    out
}

/// Sums up `statusCheckRollup`: check runs (`conclusion`) and statuses (`state`).
fn checks(rollup: &Value) -> Option<Checks> {
    let list = rollup.as_array().filter(|l| !l.is_empty())?;
    let mut pending = false;
    for c in list {
        let result = c["conclusion"].as_str().filter(|s| !s.is_empty()).or(c["state"].as_str()).unwrap_or("");
        match result {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
                return Some(Checks::Failing);
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => pending = true,
        }
    }
    Some(if pending { Checks::Pending } else { Checks::Passing })
}

/// Which of `branches` GitHub protects: classic branch protection, or an
/// active ruleset that stops deleting or force-pushing it, or requires pull
/// requests. One `gh` call, plus one per branch for rulesets. Blocks.
pub fn protected(root: &Path, branches: &[String]) -> Result<HashSet<String>, String> {
    let classic = gh(root, &["api", "repos/{owner}/{repo}/branches?protected=true&per_page=100", "--paginate"])?;
    let mut out = parse_branch_names(&classic);
    for b in branches {
        if out.contains(b) {
            continue;
        }
        // Rulesets can name branches by pattern, so ask about each one.
        if let Ok(rules) = gh(root, &["api", &format!("repos/{{owner}}/{{repo}}/rules/branches/{}", encode(b))])
            && rules_protect(&rules)
        {
            out.insert(b.clone());
        }
    }
    Ok(out)
}

/// The names in a `branches` API listing; `--paginate` may join several arrays.
pub(crate) fn parse_branch_names(json: &str) -> HashSet<String> {
    serde_json::Deserializer::from_str(json)
        .into_iter::<Value>()
        .filter_map(Result::ok)
        .flat_map(|page| page.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["name"].as_str().map(Into::into))
        .collect()
}

/// Whether the rules that apply to a branch keep it from being deleted or
/// rewritten.
pub(crate) fn rules_protect(json: &str) -> bool {
    let Ok(Value::Array(rules)) = serde_json::from_str::<Value>(json) else { return false };
    rules
        .iter()
        .any(|r| matches!(r["type"].as_str(), Some("deletion" | "non_fast_forward" | "pull_request" | "update")))
}

/// A branch name as a URL path: its `/`s stay, other odd characters are escaped.
fn encode(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Opens branch `name`'s pull request in the browser. Blocks briefly.
pub fn open_pr(root: &Path, name: &str) -> Result<(), String> {
    gh(root, &["pr", "view", name, "--web"]).map(drop)
}

/// For the fuzz target: `gh`'s JSON, whatever it holds.
pub(crate) fn fuzz_json(text: &str) {
    let _ = parse_prs(text);
    let _ = parse_branch_names(text);
    let _ = rules_protect(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pull_requests() {
        let json = r#"[
            {"headRefName":"feat","number":12,"state":"MERGED","isDraft":false,"url":"u12",
             "statusCheckRollup":[{"conclusion":"SUCCESS"},{"state":"SUCCESS"}]},
            {"headRefName":"feat","number":9,"state":"CLOSED","isDraft":false,"url":"u9","statusCheckRollup":[]},
            {"headRefName":"wip","number":14,"state":"OPEN","isDraft":true,"url":"u14",
             "statusCheckRollup":[{"conclusion":"SUCCESS"},{"conclusion":"","state":"PENDING"}]},
            {"headRefName":"bad","number":15,"state":"OPEN","isDraft":false,"url":"u15",
             "statusCheckRollup":[{"conclusion":"FAILURE"},{"state":"PENDING"}]},
            {"number":1}
        ]"#;
        let prs = parse_prs(json);
        assert_eq!(prs.len(), 3);
        assert_eq!(
            (prs["feat"].number, prs["feat"].state, prs["feat"].checks),
            (12, PrState::Merged, Some(Checks::Passing))
        );
        assert_eq!((prs["wip"].state, prs["wip"].checks), (PrState::Draft, Some(Checks::Pending)));
        assert_eq!(prs["bad"].checks, Some(Checks::Failing));
        assert!(parse_prs("not json").is_empty());
    }

    #[test]
    fn reads_protection() {
        let pages = r#"[{"name":"main","protected":true}][{"name":"release/1"}]"#;
        assert_eq!(parse_branch_names(pages), HashSet::from(["main".to_string(), "release/1".to_string()]));
        assert!(rules_protect(r#"[{"type":"deletion","ruleset_id":1}]"#));
        assert!(!rules_protect(r#"[{"type":"required_signatures"}]"#));
        assert!(!rules_protect("[]"));
        assert_eq!(encode("feat/a b#1"), "feat/a%20b%231");
    }
}
