//! `knx` subcommands as a [`Request`]:
//!
//! ```text
//! knx project list | create <folder> | open <folder> | remove <folder>
//! knx workspace list | open <folder> | create <branch> | remove <folder> [--force]
//! knx pane list [--all] | split [row|column] | focus <id> | close [id]
//! knx pane read <id> [--lines N] | run <id|new> <command…> [--wait] | prompt <id> <text…> [--wait] | wait <id>
//! knx session list | prompt <id> <text…> [--wait] | read <id> | wait <id>
//! (waits take --timeout <seconds>)
//! knx events
//! ```

use std::path::Path;

use super::Request;

/// The first words that make a command line a subcommand, not a folder to open.
const GROUPS: [&str; 5] = ["project", "workspace", "pane", "session", "events"];

/// Whether `args` (without the program name) is a control subcommand.
pub fn is_command(args: &[String]) -> bool {
    args.first().is_some_and(|a| GROUPS.contains(&a.as_str()))
}

/// The request for `args`, with folders made absolute against the current folder.
pub fn parse(args: &[String]) -> Result<Request, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["events"] => Ok(Request::Events),
        ["project", "list"] => Ok(Request::ProjectList),
        ["project", "create", p] => Ok(Request::ProjectCreate { path: absolute(p) }),
        ["project", "open", p] => Ok(Request::ProjectOpen { path: absolute(p) }),
        ["project", "remove", p] => Ok(Request::ProjectRemove { path: absolute(p) }),
        ["workspace", "list"] => Ok(Request::WorkspaceList),
        ["workspace", "open", p] => Ok(Request::WorkspaceOpen { path: absolute(p) }),
        ["workspace", "create", branch] => Ok(Request::WorkspaceCreate { branch: branch.to_string() }),
        ["workspace", "remove", p] => Ok(Request::WorkspaceRemove { path: absolute(p), force: false }),
        ["workspace", "remove", p, "--force"] => Ok(Request::WorkspaceRemove { path: absolute(p), force: true }),
        ["pane", "list"] => Ok(Request::PaneList { all: false }),
        ["pane", "list", "--all"] => Ok(Request::PaneList { all: true }),
        ["pane", "split"] => Ok(Request::PaneSplit { axis: None }),
        ["pane", "split", a @ ("row" | "column")] => Ok(Request::PaneSplit { axis: Some(a.to_string()) }),
        ["pane", "focus", n] => Ok(Request::PaneFocus { pane: number(n)? }),
        ["pane", "close"] => Ok(Request::PaneClose { pane: None }),
        ["pane", "close", n] => Ok(Request::PaneClose { pane: Some(number(n)?) }),
        ["session", "list"] => Ok(Request::SessionList),
        ["session", "read", n] => Ok(Request::SessionRead { session: number(n)? }),
        ["session", "wait", n, rest @ ..] => {
            let f = flags(rest, None)?;
            Ok(Request::SessionWait { session: number(n)?, timeout: f.timeout })
        }
        ["pane", "read", n] => Ok(Request::PaneRead { pane: number(n)?, lines: None }),
        ["pane", "read", n, "--lines", l] => Ok(Request::PaneRead { pane: number(n)?, lines: Some(count(l)?) }),
        ["pane", "wait", n, rest @ ..] => {
            let f = flags(rest, None)?;
            Ok(Request::PaneWait { pane: number(n)?, timeout: f.timeout })
        }
        ["pane", "run", n, rest @ ..] => {
            let f = flags(rest, Some("pane run needs a command"))?;
            let pane = if *n == "new" { None } else { Some(number(n)?) };
            Ok(Request::PaneRun { pane, command: f.text, wait: f.wait, timeout: f.timeout })
        }
        ["pane", "prompt", n, rest @ ..] => {
            let f = flags(rest, Some("pane prompt needs a message"))?;
            Ok(Request::PanePrompt { pane: number(n)?, text: f.text, wait: f.wait, timeout: f.timeout })
        }
        ["session", "prompt", n, rest @ ..] => {
            let f = flags(rest, Some("session prompt needs a message"))?;
            Ok(Request::SessionPrompt { session: number(n)?, text: f.text, wait: f.wait, timeout: f.timeout })
        }
        _ => Err(USAGE.into()),
    }
}

fn number(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("`{s}` isn't a number"))
}

fn count(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("`{s}` isn't a number of lines"))
}

/// The words after an id: `--wait` and `--timeout N` before or after a text.
#[derive(Debug, Default, PartialEq)]
struct Flags {
    text: String,
    wait: bool,
    timeout: Option<u64>,
}

/// [`Flags`] from `rest`. Flags are only read before and after the text, so a command's own
/// `--wait` in the middle stays in it. `missing`: the error when a text is needed and there's
/// none; `None` when no text may be given.
fn flags(rest: &[&str], missing: Option<&str>) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut words = rest.to_vec();
    // From the front, then from the back.
    loop {
        match words.as_slice() {
            ["--wait", ..] => {
                f.wait = true;
                words.remove(0);
            }
            ["--timeout", s, ..] => {
                f.timeout = Some(seconds(s)?);
                words.drain(..2);
            }
            _ => break,
        }
    }
    loop {
        match words.as_slice() {
            [.., "--wait"] => {
                f.wait = true;
                words.pop();
            }
            [.., "--timeout", s] => {
                f.timeout = Some(seconds(s)?);
                words.truncate(words.len() - 2);
            }
            _ => break,
        }
    }
    f.text = words.join(" ");
    match missing {
        Some(e) if f.text.trim().is_empty() => Err(e.into()),
        None if !f.text.is_empty() => Err(USAGE.into()),
        _ => Ok(f),
    }
}

fn seconds(s: &str) -> Result<u64, String> {
    s.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("`{s}` isn't a number of seconds"))
}

fn absolute(p: &str) -> String {
    std::path::absolute(Path::new(p)).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
}

pub const USAGE: &str = "usage:
  knx project list | create <folder> | open <folder> | remove <folder>
  knx workspace list | open <folder> | create <branch> | remove <folder> [--force]
  knx pane list [--all] | split [row|column] | focus <id> | close [id]
  knx pane read <id> [--lines N] | run <id|new> <command…> [--wait] | prompt <id> <message…> [--wait] | wait <id>
  knx session list | prompt <id> <message…> [--wait] | read <id> | wait <id>
  (--wait and wait take --timeout <seconds>)
  knx events";

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn subcommands_are_told_from_folders() {
        assert!(is_command(&words("session list")));
        assert!(is_command(&words("events")));
        assert!(is_command(&words("workspace list")));
        assert!(!is_command(&words("C:/work/app")));
        assert!(!is_command(&[]));
    }

    #[test]
    fn words_become_requests() {
        assert_eq!(parse(&words("pane list")), Ok(Request::PaneList { all: false }));
        assert_eq!(parse(&words("pane list --all")), Ok(Request::PaneList { all: true }));
        assert_eq!(parse(&words("pane split column")), Ok(Request::PaneSplit { axis: Some("column".into()) }));
        assert_eq!(parse(&words("pane focus 4")), Ok(Request::PaneFocus { pane: 4 }));
        assert_eq!(
            parse(&words("workspace create fix/login")),
            Ok(Request::WorkspaceCreate { branch: "fix/login".into() })
        );
        assert!(matches!(
            parse(&words("workspace remove sub --force")),
            Ok(Request::WorkspaceRemove { force: true, .. })
        ));
        assert!(matches!(parse(&words("workspace remove sub")), Ok(Request::WorkspaceRemove { force: false, .. })));
        assert_eq!(
            parse(&words("session prompt 2 fix the build --wait")),
            Ok(Request::SessionPrompt { session: 2, text: "fix the build".into(), wait: true, timeout: None })
        );
        assert_eq!(parse(&words("pane read 3")), Ok(Request::PaneRead { pane: 3, lines: None }));
        assert_eq!(parse(&words("pane read 3 --lines 40")), Ok(Request::PaneRead { pane: 3, lines: Some(40) }));
        assert_eq!(
            parse(&words("pane run 3 cargo test --wait")),
            Ok(Request::PaneRun { pane: Some(3), command: "cargo test".into(), wait: true, timeout: None })
        );
        assert_eq!(
            parse(&words("pane prompt 5 add tests")),
            Ok(Request::PanePrompt { pane: 5, text: "add tests".into(), wait: false, timeout: None })
        );
        assert_eq!(parse(&words("pane wait 5")), Ok(Request::PaneWait { pane: 5, timeout: None }));
        assert_eq!(parse(&words("pane wait 5 --timeout 30")), Ok(Request::PaneWait { pane: 5, timeout: Some(30) }));
        assert_eq!(
            parse(&words("pane run new --wait --timeout 60 grep --wait x")),
            Ok(Request::PaneRun { pane: None, command: "grep --wait x".into(), wait: true, timeout: Some(60) })
        );
        assert_eq!(
            parse(&words("session prompt 2 --wait go --timeout 9")),
            Ok(Request::SessionPrompt { session: 2, text: "go".into(), wait: true, timeout: Some(9) })
        );
    }

    #[test]
    fn mistakes_say_how_to_do_it() {
        assert!(parse(&words("pane focus x")).unwrap_err().contains("isn't a number"));
        assert!(parse(&words("session prompt 2")).is_err());
        assert!(parse(&words("pane run 2 --wait")).unwrap_err().contains("needs a command"));
        assert!(parse(&words("pane read 2 --lines many")).unwrap_err().contains("isn't a number"));
        assert!(parse(&words("pane wait 2 --timeout 0")).unwrap_err().contains("seconds"));
        assert!(parse(&words("pane wait 2 now")).unwrap_err().contains("usage"));
        assert!(parse(&words("pane split diagonal")).unwrap_err().contains("usage"));
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn folders_are_made_absolute() {
        let Ok(Request::ProjectOpen { path }) = parse(&words("project open sub")) else { panic!() };
        assert!(Path::new(&path).is_absolute());
    }
}
