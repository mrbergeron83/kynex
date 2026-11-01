//! The permission rules in effect for a project, from every place they live,
//! and what they'd do to a command, so nobody has to guess why Claude asks.
//!
//! The CLI merges allow / ask / deny rules from your user settings, the repo's
//! `.claude/settings.json`, `settings.local.json` and what Kynex passes with
//! `--settings`. A deny beats an ask, which beats an allow, and an ask rule
//! prompts even in Auto mode. A command chained with `&&` / `;` / `|` is judged
//! one part at a time. This is Kynex's reading of those rules: close, not the
//! CLI's own matcher, and only shell (`Bash` / `PowerShell`) rules are tried.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::project::Permissions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Allow,
    Ask,
    Deny,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Allow => "Allow",
            Kind::Ask => "Ask",
            Kind::Deny => "Deny",
        }
    }
}

/// Where a rule lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    User,
    Project,
    Local,
    Kynex,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::User, Source::Project, Source::Local, Source::Kynex];

    pub fn label(self) -> &'static str {
        match self {
            Source::User => "Your user settings",
            Source::Project => "This project's settings",
            Source::Local => "This project's local settings",
            Source::Kynex => "Kynex's rules for this project",
        }
    }

    /// The file, shown under the label.
    pub fn file(self) -> &'static str {
        match self {
            Source::User => "~/.claude/settings.json",
            Source::Project => ".claude/settings.json",
            Source::Local => ".claude/settings.local.json",
            Source::Kynex => "the Permissions section",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: Kind,
    /// In Claude Code's syntax: `Bash(git push*)`, `Edit`.
    pub text: String,
    pub source: Source,
}

/// The rules in the files, and what couldn't be read (in words to show as-is).
/// Kynex's own come from [`Permissions`], so edits show at once: add them
/// with [`kynex_rules`]. Blocks reading up to three small files.
pub fn load_files(cwd: &Path) -> (Vec<Rule>, Vec<String>) {
    let user = crate::extensions::Paths::detect().map(|p| p.dir.join("settings.json"));
    let files = [
        (Source::User, user),
        (Source::Project, Some(cwd.join(".claude").join("settings.json"))),
        (Source::Local, Some(cwd.join(".claude").join("settings.local.json"))),
    ];
    let (mut rules, mut errors) = (Vec::new(), Vec::new());
    for (source, path) in files {
        let Some(path) = path else { continue };
        match read(&path) {
            Ok(Some(json)) => rules.extend(from_settings(&json, source)),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    (rules, errors)
}

fn read(path: &PathBuf) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
    };
    serde_json::from_str(&text).map(Some).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))
}

/// The `permissions` allow / ask / deny lists of a settings file.
pub fn from_settings(json: &Value, source: Source) -> Vec<Rule> {
    let mut rules = Vec::new();
    for (key, kind) in [("deny", Kind::Deny), ("ask", Kind::Ask), ("allow", Kind::Allow)] {
        for text in json["permissions"][key].as_array().into_iter().flatten().filter_map(Value::as_str) {
            rules.push(Rule { kind, text: text.trim().to_string(), source });
        }
    }
    rules
}

/// Kynex's own rules for the project.
pub fn kynex_rules(perms: &Permissions) -> Vec<Rule> {
    let rule = |kind, text: &String| Rule { kind, text: text.clone(), source: Source::Kynex };
    perms.deny.iter().map(|t| rule(Kind::Deny, t)).chain(perms.allow.iter().map(|t| rule(Kind::Allow, t))).collect()
}

/// What the rules do to a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// `None`: no rule decides, so the permission mode does (in Auto mode,
    /// Claude's own check).
    pub decision: Option<Kind>,
    /// The rules that decided it, as indexes into the list given.
    pub by: Vec<usize>,
}

/// What `rules` do to a shell `command`: any part a deny rule matches is
/// denied, else any part an ask rule matches asks, else it's allowed if every
/// part matches an allow rule. A rule matches the whole chain only when it
/// spells it out exactly: a `*` never reaches across `&&` or `;`.
pub fn test(rules: &[Rule], command: &str) -> Verdict {
    let whole = command.trim();
    let mut parts = split_command(command);
    if parts.len() > 1 {
        parts.push(whole.to_string());
    }
    let matching = |kind: Kind, part: &str| -> Vec<usize> {
        let chain = part == whole && parts.len() > 1;
        (0..rules.len())
            .filter(|&i| {
                rules[i].kind == kind
                    && if chain { is_exactly(&rules[i].text, part) } else { matches_shell(&rules[i].text, part) }
            })
            .collect()
    };
    for kind in [Kind::Deny, Kind::Ask] {
        let by: Vec<usize> = parts.iter().flat_map(|p| matching(kind, p)).fold(Vec::new(), |mut v, i| {
            if !v.contains(&i) {
                v.push(i);
            }
            v
        });
        if !by.is_empty() {
            return Verdict { decision: Some(kind), by };
        }
    }
    let each: Vec<Vec<usize>> = split_command(command).iter().map(|p| matching(Kind::Allow, p)).collect();
    let all_allowed = !each.is_empty() && each.iter().all(|m| !m.is_empty());
    let whole_allowed = if parts.len() > 1 { matching(Kind::Allow, whole) } else { Vec::new() };
    if all_allowed {
        let mut by: Vec<usize> = each.iter().filter_map(|m| m.first().copied()).collect();
        by.dedup();
        return Verdict { decision: Some(Kind::Allow), by };
    }
    if !whole_allowed.is_empty() {
        return Verdict { decision: Some(Kind::Allow), by: whole_allowed };
    }
    Verdict { decision: None, by: Vec::new() }
}

/// Splits a shell command at `&&`, `||`, `;`, `|` and newlines outside quotes.
pub fn split_command(command: &str) -> Vec<String> {
    let (mut parts, mut cur) = (Vec::new(), String::new());
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '&' | '|') => {
                // `&&`, `||` and a lone `|` end a part; a lone `&` does too.
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                parts.push(std::mem::take(&mut cur));
            }
            (None, ';' | '\n') => parts.push(std::mem::take(&mut cur)),
            (None, c) => cur.push(c),
        }
    }
    parts.push(cur);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// A rule's tool and what's in its brackets: `Bash(git *)` is `("Bash", Some("git *"))`.
fn parse(rule: &str) -> (&str, Option<&str>) {
    match rule.split_once('(') {
        Some((tool, rest)) => (tool.trim(), Some(rest.strip_suffix(')').unwrap_or(rest))),
        None => (rule.trim(), None),
    }
}

/// Whether a rule is for the shell tools, and so matches commands.
pub fn is_shell(rule: &str) -> bool {
    matches!(parse(rule).0, "Bash" | "PowerShell")
}

/// Whether a shell rule is exactly this command, no wildcard involved.
fn is_exactly(rule: &str, command: &str) -> bool {
    let (tool, spec) = parse(rule);
    matches!(tool, "Bash" | "PowerShell") && spec == Some(command.trim()) && !command.contains('*')
}

fn matches_shell(rule: &str, command: &str) -> bool {
    let (tool, spec) = parse(rule);
    if !matches!(tool, "Bash" | "PowerShell") {
        return false;
    }
    let Some(spec) = spec else { return true };
    let command = command.trim();
    // The older `prefix:*`: the prefix as whole words.
    if let Some(prefix) = spec.strip_suffix(":*") {
        return command == prefix || command.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(' '));
    }
    // `git commit *` also takes `git commit` alone.
    if let Some(prefix) = spec.strip_suffix(" *")
        && command == prefix
    {
        return true;
    }
    glob(spec, command)
}

/// `*` matches any run of characters, the rest must match exactly.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Where an ask or deny rule cuts into an allow rule: commands both match, so
/// the allow doesn't apply to them. Words for the Rules view, one per pair.
pub fn conflicts(rules: &[Rule]) -> Vec<String> {
    let literal = |text: &str| -> String {
        let spec = parse(text).1.unwrap_or("");
        let spec = spec.strip_suffix(":*").unwrap_or(spec);
        spec.split('*').next().unwrap_or("").trim_end().to_string()
    };
    let mut out = Vec::new();
    for over in rules.iter().filter(|r| r.kind != Kind::Allow && is_shell(&r.text)) {
        for under in rules.iter().filter(|r| r.kind == Kind::Allow && is_shell(&r.text)) {
            let (a, b) = (literal(&over.text), literal(&under.text));
            // They share commands when one's fixed start begins the other's.
            if a.starts_with(&b) || b.starts_with(&a) {
                let what = if over.kind == Kind::Ask { "asks" } else { "denies" };
                out.push(format!(
                    "{} ({}, {}) {what} even where {} ({}, {}) allows.",
                    over.text,
                    over.kind.label().to_lowercase(),
                    over.source.label().to_lowercase(),
                    under.text,
                    under.kind.label().to_lowercase(),
                    under.source.label().to_lowercase(),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rule(kind: Kind, text: &str, source: Source) -> Rule {
        Rule { kind, text: text.into(), source }
    }

    #[test]
    fn reads_all_three_lists_of_a_settings_file() {
        let j =
            json!({ "permissions": { "allow": ["Bash(git *)"], "ask": ["Bash(git push*)"], "deny": ["Bash(rm *)"] } });
        let rules = from_settings(&j, Source::Project);
        assert_eq!(rules.len(), 3);
        assert!(rules.contains(&rule(Kind::Ask, "Bash(git push*)", Source::Project)));
        assert!(from_settings(&json!({}), Source::User).is_empty());
    }

    #[test]
    fn splits_chains_outside_quotes() {
        assert_eq!(
            split_command("git add -A && git commit -m x; git push | cat"),
            ["git add -A", "git commit -m x", "git push", "cat"]
        );
        assert_eq!(split_command(r#"git commit -m "a && b" && git push"#), [r#"git commit -m "a && b""#, "git push"]);
        assert!(split_command("  ").is_empty());
    }

    #[test]
    fn matches_patterns_the_way_rules_are_written() {
        assert!(matches_shell("Bash(git *)", "git status"));
        assert!(!matches_shell("Bash(git *)", "gitk"));
        assert!(matches_shell("Bash(git commit *)", "git commit"));
        assert!(matches_shell("Bash(git push*)", "git push -q"));
        assert!(matches_shell("PowerShell(cargo test*)", "cargo test -p x"));
        assert!(matches_shell("Bash(npm:*)", "npm test"));
        assert!(!matches_shell("Bash(npm:*)", "npmx"));
        assert!(matches_shell("Bash", "anything"));
        assert!(matches_shell("Bash(cargo test*--ignored*)", "cargo test -p x -- --ignored"));
        assert!(!matches_shell("Edit", "git status"));
    }

    #[test]
    fn an_ask_rule_in_a_chain_makes_the_chain_ask() {
        // tikebec: git allowed, push asks; one chained command prompted.
        let rules =
            [rule(Kind::Allow, "Bash(git *)", Source::User), rule(Kind::Ask, "Bash(git push*)", Source::Project)];
        let v = test(&rules, "git add -A && git commit -qm x && git push -q && git log -1");
        assert_eq!((v.decision, v.by), (Some(Kind::Ask), vec![1]));
        let v = test(&rules, "git add -A && git commit -qm x");
        assert_eq!(v.decision, Some(Kind::Allow));
    }

    #[test]
    fn deny_beats_ask_beats_allow_and_no_rule_leaves_it_to_the_mode() {
        let rules = [
            rule(Kind::Allow, "Bash(git *)", Source::User),
            rule(Kind::Ask, "Bash(git push*)", Source::Project),
            rule(Kind::Deny, "Bash(git push --force*)", Source::Local),
        ];
        assert_eq!(test(&rules, "git push --force").decision, Some(Kind::Deny));
        assert_eq!(test(&rules, "git push").decision, Some(Kind::Ask));
        assert_eq!(test(&rules, "rm -rf x"), Verdict { decision: None, by: Vec::new() });
        // One part nothing allows leaves the whole to the mode.
        assert_eq!(test(&rules, "git status && make").decision, None);
        assert_eq!(test(&rules, "").decision, None);
    }

    #[test]
    fn an_exact_allow_covers_the_whole_command() {
        let rules = [rule(Kind::Allow, "Bash(sed -i 's/a/b/' f && git add f)", Source::Kynex)];
        assert_eq!(test(&rules, "sed -i 's/a/b/' f && git add f").decision, Some(Kind::Allow));
    }

    #[test]
    fn says_where_an_ask_cuts_into_an_allow() {
        let rules =
            [rule(Kind::Allow, "Bash(git *)", Source::User), rule(Kind::Ask, "Bash(git push*)", Source::Project)];
        let c = conflicts(&rules);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("Bash(git push*)") && c[0].contains("asks") && c[0].contains("Bash(git *)"), "{c:?}");
        // Unrelated rules don't clash.
        let rules =
            [rule(Kind::Allow, "Bash(git *)", Source::User), rule(Kind::Ask, "Bash(cargo run*)", Source::Project)];
        assert!(conflicts(&rules).is_empty());
    }

    #[test]
    fn loads_files_and_says_what_it_couldnt_read() {
        let dir = std::env::temp_dir().join(format!("kynex-rules-{}", std::process::id()));
        let claude = dir.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("settings.json"), r#"{"permissions":{"ask":["Bash(git push*)"]}}"#).unwrap();
        std::fs::write(claude.join("settings.local.json"), "{ nope").unwrap();
        let (rules, errors) = load_files(&dir);
        assert!(rules.contains(&rule(Kind::Ask, "Bash(git push*)", Source::Project)));
        assert!(errors.iter().any(|e| e.contains("settings.local.json") && e.contains("valid JSON")), "{errors:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
