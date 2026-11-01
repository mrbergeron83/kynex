//! Starter permission rules for a project, picked by what's in its folder
//! (git, Cargo, npm, Python, Go, .NET): the everyday commands a project's
//! Claude would otherwise ask about, one click to allow. Never a push, a
//! publish or a delete.

use std::path::Path;

use crate::rules::{self, Kind, Rule};

/// A group of rules for one thing found in the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Starter {
    pub label: &'static str,
    /// What was found: "a Cargo.toml".
    pub found: &'static str,
    /// In Claude Code's syntax, once per shell (`Bash(..)`, `PowerShell(..)`).
    pub rules: Vec<String>,
}

/// What's worth offering for `folder`. Blocks reading its top level.
pub fn detect(folder: &Path) -> Vec<Starter> {
    let has = |name: &str| folder.join(name).exists();
    let has_ext = |ext: &str| {
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == ext))
    };
    let mut out = Vec::new();
    let mut add = |label, found, specs: &[&str]| out.push(Starter { label, found, rules: both(specs) });
    if has(".git") {
        add(
            "Git: status, diff, log, show, add, commit",
            "a git repository",
            &["git status*", "git diff*", "git log*", "git show*", "git add *", "git commit *"],
        );
    }
    if has("Cargo.toml") {
        add(
            "Cargo: check, build, test, clippy, fmt",
            "a Cargo.toml",
            &["cargo check*", "cargo build*", "cargo test*", "cargo clippy*", "cargo fmt*"],
        );
    }
    if has("package.json") {
        add("npm: test, build, lint", "a package.json", &["npm test*", "npm run build*", "npm run lint*"]);
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        add("Python: pytest, ruff", "a Python project", &["pytest*", "python -m pytest*", "ruff*"]);
    }
    if has("go.mod") {
        add("Go: build, test, vet", "a go.mod", &["go build*", "go test*", "go vet*"]);
    }
    if has_ext("sln") || has_ext("csproj") {
        add(".NET: build, test", "a .NET project", &["dotnet build*", "dotnet test*"]);
    }
    out
}

fn both(specs: &[&str]) -> Vec<String> {
    ["Bash", "PowerShell"].iter().flat_map(|tool| specs.iter().map(move |s| format!("{tool}({s})"))).collect()
}

/// The rules of `starter` that `existing` doesn't already allow: a rule counts
/// as covered when a command it would match is allowed (`git *` covers
/// `git add *`). An ask rule doesn't cover it, so it's offered again.
pub fn missing(starter: &Starter, existing: &[Rule]) -> Vec<String> {
    starter
        .rules
        .iter()
        .filter(|rule| {
            let (tool, spec) =
                rule.split_once('(').map_or((rule.as_str(), ""), |(t, rest)| (t, rest.trim_end_matches(')')));
            // Only the same shell's rules count: Bash's don't cover PowerShell's.
            let same_shell: Vec<Rule> = existing
                .iter()
                .filter(|r| r.text == tool || r.text.starts_with(&format!("{tool}(")))
                .cloned()
                .collect();
            rules::test(&same_shell, &spec.replace('*', "x")).decision != Some(Kind::Allow)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Source;

    fn folder(files: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kynex-starters-{}-{}", std::process::id(), files.join("-")));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            if let Some(d) = f.strip_suffix('/') {
                std::fs::create_dir_all(dir.join(d)).unwrap();
            } else {
                std::fs::write(dir.join(f), "").unwrap();
            }
        }
        dir
    }

    #[test]
    fn offers_what_the_folder_has_for_both_shells() {
        let dir = folder(&[".git/", "Cargo.toml", "app.sln"]);
        let found: Vec<_> = detect(&dir).iter().map(|s| s.found).collect();
        assert_eq!(found, ["a git repository", "a Cargo.toml", "a .NET project"]);
        let git = &detect(&dir)[0];
        assert!(git.rules.contains(&"Bash(git commit *)".to_string()));
        assert!(git.rules.contains(&"PowerShell(git commit *)".to_string()));
        assert!(git.rules.iter().all(|r| !r.contains("push")));
        std::fs::remove_dir_all(&dir).ok();
        assert!(detect(&folder(&["notes.txt"])).is_empty());
    }

    #[test]
    fn leaves_out_what_is_already_allowed() {
        let git = Starter { label: "g", found: "g", rules: both(&["git status*", "git add *"]) };
        let allow = |t: &str| Rule { kind: Kind::Allow, text: t.into(), source: Source::User };
        // `git *` for Bash covers Bash's, not PowerShell's.
        let left = missing(&git, &[allow("Bash(git *)")]);
        assert_eq!(left, ["PowerShell(git status*)", "PowerShell(git add *)"]);
        assert!(missing(&git, &[allow("Bash(git *)"), allow("PowerShell(git *)")]).is_empty());
        assert_eq!(missing(&git, &[]).len(), 4);
    }
}
