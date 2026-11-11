//! `knx` subcommands as a [`Request`]:
//!
//! ```text
//! knx project list | create <folder> | open <folder> | remove <folder>
//! knx workspace list | open <folder> | create <branch> | remove <folder> [--force]
//! knx pane list | split [row|column] | focus <id> | close [id]
//! knx session list | prompt <id> <text…> [--wait] | read <id> | wait <id>
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
        ["pane", "list"] => Ok(Request::PaneList),
        ["pane", "split"] => Ok(Request::PaneSplit { axis: None }),
        ["pane", "split", a @ ("row" | "column")] => Ok(Request::PaneSplit { axis: Some(a.to_string()) }),
        ["pane", "focus", n] => Ok(Request::PaneFocus { pane: number(n)? }),
        ["pane", "close"] => Ok(Request::PaneClose { pane: None }),
        ["pane", "close", n] => Ok(Request::PaneClose { pane: Some(number(n)?) }),
        ["session", "list"] => Ok(Request::SessionList),
        ["session", "read", n] => Ok(Request::SessionRead { session: number(n)? }),
        ["session", "wait", n] => Ok(Request::SessionWait { session: number(n)? }),
        ["session", "prompt", n, rest @ ..] => {
            let wait = rest.contains(&"--wait");
            let text = rest.iter().filter(|w| **w != "--wait").copied().collect::<Vec<_>>().join(" ");
            if text.trim().is_empty() {
                return Err("session prompt needs a message".into());
            }
            Ok(Request::SessionPrompt { session: number(n)?, text, wait })
        }
        _ => Err(USAGE.into()),
    }
}

fn number(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("`{s}` isn't a number"))
}

fn absolute(p: &str) -> String {
    std::path::absolute(Path::new(p)).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
}

pub const USAGE: &str = "usage:
  knx project list | create <folder> | open <folder> | remove <folder>
  knx workspace list | open <folder> | create <branch> | remove <folder> [--force]
  knx pane list | split [row|column] | focus <id> | close [id]
  knx session list | prompt <id> <message…> [--wait] | read <id> | wait <id>
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
        assert_eq!(parse(&words("pane list")), Ok(Request::PaneList));
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
            Ok(Request::SessionPrompt { session: 2, text: "fix the build".into(), wait: true })
        );
    }

    #[test]
    fn mistakes_say_how_to_do_it() {
        assert!(parse(&words("pane focus x")).unwrap_err().contains("isn't a number"));
        assert!(parse(&words("session prompt 2")).is_err());
        assert!(parse(&words("pane split diagonal")).unwrap_err().contains("usage"));
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn folders_are_made_absolute() {
        let Ok(Request::ProjectOpen { path }) = parse(&words("project open sub")) else { panic!() };
        assert!(Path::new(&path).is_absolute());
    }
}
