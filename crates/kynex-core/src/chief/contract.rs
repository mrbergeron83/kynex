//! The contract between the chief and its workers: how a worker works (in
//! its own worktree, never asking the user), how each of its turns ends (a
//! report the chief reads), and the commands Kynex warns about. Workers run
//! without permission prompts, so nothing is blocked; risky commands are
//! called out instead.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The branch each worker's worktree is on; its folder is `<repo>-chief-worker`.
pub const BRANCH: &str = "chief-worker";

/// The worktree a project's worker changes, made the first time. `None` for a
/// folder that isn't a git repository's root: the worker works in it directly.
/// Blocks on git.
pub fn worktree(project: &Path) -> Result<Option<PathBuf>, String> {
    if !project.join(".git").exists() {
        return Ok(None);
    }
    let path = crate::git::worktree::default_worktree_path(project, BRANCH);
    if !path.exists() {
        crate::git::worktree::add_worktree(project, &path, BRANCH)
            .map_err(|e| format!("couldn't make the chief worker's worktree: {e}"))?;
    }
    Ok(Some(path))
}

/// The `claude` arguments that make a session in `project` a chief worker:
/// no permission prompts, no questions to the user, its worktree and its
/// instructions. Blocks on git the first time (see [`worktree`]).
pub fn worker_args(project: &Path) -> Result<Vec<String>, String> {
    let tree = worktree(project)?;
    let mut args: Vec<String> =
        ["--dangerously-skip-permissions", "--disallowedTools", "AskUserQuestion"].map(String::from).to_vec();
    if let Some(t) = &tree {
        args.extend(["--add-dir".into(), t.display().to_string()]);
    }
    args.extend(["--append-system-prompt".into(), worker_prompt(project, tree.as_deref())]);
    Ok(args)
}

fn worker_prompt(project: &Path, tree: Option<&Path>) -> String {
    let place = match tree {
        Some(t) => format!(
            "Your working copy is the git worktree {} on branch {BRANCH}. Make every change there: edit \
             files under it and run commands from it (cd into it first). You may read {} and any other \
             worktree, but never change them. Commit your work on {BRANCH} when a piece is done; push \
             only when the task says to.",
            t.display(),
            project.display()
        ),
        None => format!("This project isn't a git repository: you work in {} directly.", project.display()),
    };
    format!(
        "You are a chief worker in Kynex. The user's chief of staff sends you work for this project and \
         reads your reports. You run with no permission prompts, so take care with anything that can't be \
         undone.

{place}

You can't ask the user anything. When you need a decision you can't make from the task, stop and ask \
it in your report.

End every turn with this report, as the last thing you write:

STATUS: done | blocked | failed
SUMMARY: what you did, in a few lines
CHANGED: the files, commits and branch you changed, or none
QUESTION: what you need decided (only when STATUS is blocked)"
    )
}

/// How a worker says its turn went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Blocked,
    Failed,
}

/// A worker's end-of-turn report, as far as Kynex reads it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Option<Status>,
    /// What it needs decided, when blocked.
    pub question: Option<String>,
}

/// Reads the report at the end of a worker's reply. Lenient: any case, and
/// markdown around the keys (`**STATUS:** done`).
pub fn parse_report(reply: &str) -> Report {
    let mut report = Report::default();
    let mut question: Option<Vec<&str>> = None;
    for line in reply.lines() {
        let bare = line.trim().trim_start_matches(['*', '_', '#', '-', ' ', '`']);
        let field = bare.split_once(':').map(|(k, v)| (k.trim_end_matches(['*', '_', '`']).trim(), v));
        match field {
            Some((k, v)) if k.eq_ignore_ascii_case("status") => {
                let v = v.trim().trim_matches(['*', '_', '`', ' ']).to_lowercase();
                report.status = match v.split_whitespace().next() {
                    Some("done") => Some(Status::Done),
                    Some("blocked") => Some(Status::Blocked),
                    Some("failed") => Some(Status::Failed),
                    _ => report.status,
                };
                question = None;
            }
            Some((k, v)) if k.eq_ignore_ascii_case("question") => {
                question = Some(vec![v.trim().trim_start_matches(['*', '_', ' '])]);
            }
            Some((k, _)) if ["summary", "changed"].iter().any(|f| k.eq_ignore_ascii_case(f)) => question = None,
            _ => {
                if let Some(q) = &mut question {
                    q.push(line.trim());
                }
            }
        }
        if let Some(q) = &question {
            let text = q.join("\n").trim().to_string();
            report.question = (!text.is_empty()).then_some(text);
        }
    }
    report
}

/// Why a tool call deserves a warning, if it does: pushes, force pushes,
/// branch deletion, hard resets, recursive deletes, merges and publishing.
/// `input` is the call's input as the transcript keeps it (pretty JSON).
pub fn risky(tool: &str, input: &str) -> Option<&'static str> {
    if !matches!(tool, "Bash" | "PowerShell") {
        return None;
    }
    let input: Value = serde_json::from_str(input).ok()?;
    let command = input["command"].as_str()?.to_lowercase();
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let has = |s: &str| command.contains(s);
    let push = has("git push");
    Some(if push && (has(" --delete") || has(" -d ") || has(" :")) {
        "deletes a remote branch"
    } else if push && (has("--force") || has(" -f") || has(" +")) {
        "force-pushes"
    } else if push {
        "pushes to a remote"
    } else if has("git branch -d") || has("git branch --delete") {
        "deletes a branch"
    } else if has("git reset --hard") {
        "throws away changes (git reset --hard)"
    } else if flags_after(&command, "git clean").any(|f| f.contains('f')) {
        "deletes untracked files"
    } else if has("rm -rf") || has("rm -fr") || has("rm -r ") || (has("remove-item") && has("-recurse")) {
        "deletes files recursively"
    } else if has("gh pr merge") {
        "merges a pull request"
    } else if has("npm publish") || has("cargo publish") || has("gh release create") {
        "publishes a release"
    } else {
        return None;
    })
}

/// The short flags (`-fd`) after `program` in `command`.
fn flags_after<'a>(command: &'a str, program: &str) -> impl Iterator<Item = &'a str> {
    let rest = command.split_once(program).map_or("", |(_, r)| r);
    rest.split_whitespace()
        .take_while(|w| !matches!(*w, "&&" | "||" | ";" | "|"))
        .filter(|w| w.starts_with('-') && !w.starts_with("--"))
}

/// Checks that `path` can become a project: an absolute path to a folder.
pub fn existing_project(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err(format!("{path} isn't a full path"));
    }
    if !p.is_dir() {
        return Err(format!("{path} isn't a folder; create_project makes a new one"));
    }
    Ok(p)
}

/// Makes a new project folder at `path` with `git init`. Refuses a folder
/// that already has something in it. Blocks on git.
pub fn create_project(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err(format!("{path} isn't a full path"));
    }
    if p.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{path} already has files in it; open_project opens it as it is"));
    }
    std::fs::create_dir_all(&p).map_err(|e| format!("couldn't make {path}: {e}"))?;
    let out = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&p)
        .output()
        .map_err(|e| format!("couldn't run git init: {e}"))?;
    if !out.status.success() {
        return Err(format!("git init failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reports() {
        let r = parse_report("Did it.\n\nSTATUS: done\nSUMMARY: added x\nCHANGED: a.rs");
        assert_eq!(r, Report { status: Some(Status::Done), question: None });
        let r = parse_report(
            "**STATUS:** Blocked\n**SUMMARY:** half way\n**QUESTION:** Postgres or SQLite?\nThe tests assume one.",
        );
        assert_eq!(r.status, Some(Status::Blocked));
        assert_eq!(r.question.as_deref(), Some("Postgres or SQLite?\nThe tests assume one."));
        assert_eq!(parse_report("just text"), Report::default());
        assert_eq!(parse_report("status: failed — build broke").status, Some(Status::Failed));
    }

    #[test]
    fn warns_about_risky_commands() {
        let bash = |c: &str| risky("Bash", &serde_json::json!({ "command": c }).to_string());
        assert_eq!(bash("git push origin chief-worker"), Some("pushes to a remote"));
        assert_eq!(bash("git push --force-with-lease"), Some("force-pushes"));
        assert_eq!(bash("git  push origin +main"), Some("force-pushes"));
        assert_eq!(bash("git push origin --delete old"), Some("deletes a remote branch"));
        assert_eq!(bash("git branch -D old"), Some("deletes a branch"));
        assert_eq!(bash("cd x && git reset --hard HEAD~1"), Some("throws away changes (git reset --hard)"));
        assert_eq!(bash("rm -rf target"), Some("deletes files recursively"));
        assert_eq!(bash("cargo publish -p kynex-core"), Some("publishes a release"));
        assert_eq!(
            risky("PowerShell", r#"{"command":"Remove-Item -Recurse -Force build"}"#),
            Some("deletes files recursively")
        );
        assert_eq!(bash("git clean -fdx"), Some("deletes untracked files"));
        assert_eq!(bash("git clean -n -- fixtures"), None);
        assert_eq!(bash("git status && cargo test"), None);
        assert_eq!(risky("Read", r#"{"command":"git push"}"#), None);
        assert_eq!(risky("Bash", "not json"), None);
    }

    #[test]
    fn projects_are_full_paths() {
        let dir = std::env::temp_dir().join(format!("kynex-chief-{}", std::process::id()));
        let path = dir.join("new");
        assert!(existing_project("relative").is_err());
        assert!(existing_project(&path.display().to_string()).is_err());
        let made = create_project(&path.display().to_string()).unwrap();
        assert!(made.join(".git").exists());
        std::fs::write(made.join("a.txt"), "x").unwrap();
        assert!(create_project(&path.display().to_string()).unwrap_err().contains("already has files"));
        assert_eq!(existing_project(&path.display().to_string()), Ok(made));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "needs the claude CLI and uses a little quota"]
    fn a_worker_keeps_the_contract() {
        let dir = std::env::temp_dir().join(format!("kynex-chief-live-{}", std::process::id()));
        let repo = dir.join("repo");
        create_project(&repo.display().to_string()).unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap();
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start"]);
        let out = std::process::Command::new(crate::cli::program())
            .args(["-p", "Create hello.txt containing the word hi. Don't commit.", "--model", "haiku"])
            .args(worker_args(&repo).unwrap())
            .current_dir(&repo)
            .output()
            .unwrap();
        let reply = String::from_utf8_lossy(&out.stdout).to_string();
        let tree = dir.join("repo-chief-worker");
        assert!(tree.join("hello.txt").exists(), "not in its worktree: {reply}");
        assert!(!repo.join("hello.txt").exists(), "changed the project's own copy: {reply}");
        assert_eq!(parse_report(&reply).status, Some(Status::Done), "{reply}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workers_get_their_worktree() {
        let dir = std::env::temp_dir().join(format!("kynex-chief-wt-{}", std::process::id()));
        let repo = dir.join("repo");
        create_project(&repo.display().to_string()).unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap();
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start"]);
        let tree = worktree(&repo).unwrap().expect("a git repo gets a worktree");
        assert_eq!(tree, dir.join("repo-chief-worker"));
        assert!(tree.join(".git").exists());
        // The second time it's the same one.
        assert_eq!(worktree(&repo).unwrap(), Some(tree.clone()));
        let args = worker_args(&repo).unwrap();
        assert!(args.contains(&tree.display().to_string()));
        assert!(args.last().unwrap().contains("STATUS: done | blocked | failed"));
        // A plain folder has none.
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(worktree(&plain).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
