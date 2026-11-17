//! `knx` subcommands as a [`Request`]:
//!
//! ```text
//! knx project list | create <folder> | open <folder> | remove <folder>
//! knx workspace list | open <folder> | create <branch> [--session] [--prompt <text…>] | remove <folder> [--force]
//! knx pane list [--all] | split [row|column] | focus <id> | close [id]
//! knx pane read <id> [--lines N] | run <id|new> <command…> [--wait] | prompt <id> <text…> [--wait] | wait <id>
//! knx session list | prompt <s> <text…> [--wait] | read <s> | wait <s>
//! knx session new [--workspace <folder>] [--title <t>] [--pane right|down|tab|none] [--prompt <text…>]
//! knx session fork <s> [--at N] [--pane …] | open <id> [--pane …] | close <s> | rename <s> <title…>
//! knx session archive <s> | pin <s> [--off]
//! knx request status <id> | wait <id>
//! knx notify <title> [body…] [--pane <id> | --session <s>]
//! (waits take --timeout <seconds>)
//! knx events
//! knx api schema   (printed by knx itself: the protocol as JSON, no app needed)
//! ```

use std::path::Path;

use super::Request;
use super::protocol::{Place, SessionRef};

/// The first words that make a command line a subcommand, not a folder to open.
const GROUPS: [&str; 8] = ["project", "workspace", "pane", "session", "request", "notify", "events", "api"];

/// Whether `args` (without the program name) is a control subcommand.
pub fn is_command(args: &[String]) -> bool {
    args.first().is_some_and(|a| GROUPS.contains(&a.as_str()))
}

/// Whether `args` is `api schema`: answered by `knx` itself, not a request to the app.
pub fn wants_schema(args: &[String]) -> bool {
    matches!(args, [a, b] if a == "api" && b == "schema")
}

/// The request for `args`, with folders made absolute against the current folder.
pub fn parse(args: &[String]) -> Result<Request, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["events"] => Ok(Request::Events),
        ["notify", rest @ ..] => notify(rest),
        ["project", "list"] => Ok(Request::ProjectList),
        ["project", "create", p] => Ok(Request::ProjectCreate { path: absolute(p) }),
        ["project", "open", p] => Ok(Request::ProjectOpen { path: absolute(p) }),
        ["project", "remove", p] => Ok(Request::ProjectRemove { path: absolute(p) }),
        ["workspace", "list"] => Ok(Request::WorkspaceList),
        ["workspace", "open", p] => Ok(Request::WorkspaceOpen { path: absolute(p) }),
        ["workspace", "create", branch, rest @ ..] => {
            let o = options(rest, &["--session"], &["--why"])?;
            let prompt = o.prompt.clone();
            Ok(Request::WorkspaceCreate {
                branch: branch.to_string(),
                session: o.switches.contains(&"--session") || prompt.is_some(),
                prompt,
                why: o.value("--why").map(str::to_string),
            })
        }
        ["session", "new", rest @ ..] => {
            let o = options(rest, &[], &["--workspace", "--title", "--pane", "--why"])?;
            Ok(Request::SessionNew {
                workspace: o.value("--workspace").map(absolute),
                prompt: o.prompt.clone(),
                title: o.value("--title").map(str::to_string),
                pane: o.value("--pane").map(Place::parse).transpose()?,
                why: o.value("--why").map(str::to_string),
            })
        }
        ["session", "fork", s, rest @ ..] => {
            let o = options(rest, &[], &["--at", "--pane"])?;
            let at = o.value("--at").map(message_number).transpose()?;
            no_prompt(&o)?;
            Ok(Request::SessionFork {
                session: SessionRef::parse(s)?,
                at,
                pane: o.value("--pane").map(Place::parse).transpose()?,
            })
        }
        ["session", "open", id, rest @ ..] => {
            let o = options(rest, &[], &["--pane"])?;
            no_prompt(&o)?;
            Ok(Request::SessionOpen { session: id.to_string(), pane: o.value("--pane").map(Place::parse).transpose()? })
        }
        ["session", "close", s] => Ok(Request::SessionClose { session: SessionRef::parse(s)? }),
        ["session", "rename", s, title @ ..] => {
            Ok(Request::SessionRename { session: SessionRef::parse(s)?, title: title.join(" ") })
        }
        ["session", "archive", s] => Ok(Request::SessionArchive { session: SessionRef::parse(s)? }),
        ["session", "pin", s] => Ok(Request::SessionPin { session: SessionRef::parse(s)?, off: false }),
        ["session", "pin", s, "--off"] => Ok(Request::SessionPin { session: SessionRef::parse(s)?, off: true }),
        ["request", "status", n] => Ok(Request::RequestStatus { request: number(n)? }),
        ["request", "wait", n, rest @ ..] => {
            let f = flags(rest, None)?;
            Ok(Request::RequestWait { request: number(n)?, timeout: f.timeout })
        }
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
        ["session", "read", n] => Ok(Request::SessionRead { session: SessionRef::parse(n)? }),
        ["session", "wait", n, rest @ ..] => {
            let f = flags(rest, None)?;
            Ok(Request::SessionWait { session: SessionRef::parse(n)?, timeout: f.timeout })
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
            Ok(Request::SessionPrompt {
                session: SessionRef::parse(n)?,
                text: f.text,
                wait: f.wait,
                timeout: f.timeout,
            })
        }
        _ => Err(USAGE.into()),
    }
}

/// `notify`: `--pane` and `--session` anywhere; the first other word is the title, the rest the body.
fn notify(rest: &[&str]) -> Result<Request, String> {
    let (mut pane, mut session, mut text) = (None, None, Vec::new());
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            w @ ("--pane" | "--session") => {
                let v = rest.get(i + 1).ok_or_else(|| format!("{w} needs a value"))?;
                if w == "--pane" {
                    pane = Some(number(v)?);
                } else {
                    session = Some(SessionRef::parse(v)?);
                }
                i += 2;
            }
            w => {
                text.push(w);
                i += 1;
            }
        }
    }
    if pane.is_some() && session.is_some() {
        return Err("notify takes --pane or --session, not both".into());
    }
    let Some((title, body)) = text.split_first() else { return Err("notify needs a title".into()) };
    let body = (!body.is_empty()).then(|| body.join(" "));
    Ok(Request::Notify { title: title.to_string(), body, pane, session })
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

/// The options of a request that makes something: switches, `--name value` pairs, and
/// `--prompt`, which takes every word after it (so it comes last).
#[derive(Debug, Default)]
struct Options<'a> {
    switches: Vec<&'a str>,
    values: Vec<(&'a str, &'a str)>,
    prompt: Option<String>,
}

impl<'a> Options<'a> {
    fn value(&self, name: &str) -> Option<&'a str> {
        self.values.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

/// [`Options`] from `rest`; `switches` and `valued` are the names allowed besides `--prompt`.
fn options<'a>(rest: &[&'a str], switches: &[&'a str], valued: &[&str]) -> Result<Options<'a>, String> {
    let mut o = Options::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if w == "--prompt" {
            let text = rest[i + 1..].join(" ");
            if text.trim().is_empty() {
                return Err("--prompt needs a message".into());
            }
            o.prompt = Some(text);
            break;
        } else if let Some(s) = switches.iter().find(|s| **s == w) {
            o.switches.push(s);
            i += 1;
        } else if valued.contains(&w) {
            let v = rest.get(i + 1).ok_or_else(|| format!("{w} needs a value"))?;
            o.values.push((w, v));
            i += 2;
        } else {
            return Err(USAGE.into());
        }
    }
    Ok(o)
}

fn no_prompt(o: &Options) -> Result<(), String> {
    if o.prompt.is_some() { Err(USAGE.into()) } else { Ok(()) }
}

/// A message's number in a session, from 1.
fn message_number(s: &str) -> Result<usize, String> {
    s.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("`{s}` isn't a message number (from 1)"))
}

fn seconds(s: &str) -> Result<u64, String> {
    s.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("`{s}` isn't a number of seconds"))
}

fn absolute(p: &str) -> String {
    std::path::absolute(Path::new(p)).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
}

pub const USAGE: &str = "usage:
  knx project list | create <folder> | open <folder> | remove <folder>
  knx workspace list | open <folder> | remove <folder> [--force]
  knx workspace create <branch> [--session] [--why <reason>] [--prompt <message…>]
  knx pane list [--all] | split [row|column] | focus <id> | close [id]
  knx pane read <id> [--lines N] | run <id|new> <command…> [--wait] | prompt <id> <message…> [--wait] | wait <id>
  knx session list | prompt <s> <message…> [--wait] | read <s> | wait <s>
  knx session new [--workspace <folder>] [--title <t>] [--pane right|down|tab|none] [--why <reason>]
                  [--prompt <message…>]
  knx session fork <s> [--at <message number>] [--pane …] | open <session id> [--pane …]
  knx session close <s> | rename <s> <title…> | archive <s> | pin <s> [--off]
  knx request status <id> | wait <id>
  knx notify <title> [body…] [--pane <id in the shown workspace> | --session <s>]
  (<s>: a session's number or its session id; --wait and wait take --timeout <seconds>)
  knx events
  knx api schema        (the requests, answers and events as JSON; needs no running Kynex)
Requests that change something wait for your OK in Kynex; one not answered within a short
wait says `pending` with a request id for `knx request wait`.";

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
        assert!(is_command(&words("notify Done")));
        assert!(is_command(&words("workspace list")));
        assert!(is_command(&words("api schema")));
        assert!(wants_schema(&words("api schema")));
        assert!(!wants_schema(&words("api")) && !wants_schema(&words("api schema now")));
        assert!(parse(&words("api nope")).unwrap_err().contains("usage"));
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
            Ok(Request::WorkspaceCreate { branch: "fix/login".into(), session: false, prompt: None, why: None })
        );
        assert!(matches!(
            parse(&words("workspace remove sub --force")),
            Ok(Request::WorkspaceRemove { force: true, .. })
        ));
        assert!(matches!(parse(&words("workspace remove sub")), Ok(Request::WorkspaceRemove { force: false, .. })));
        assert_eq!(
            parse(&words("session prompt 2 fix the build --wait")),
            Ok(Request::SessionPrompt {
                session: SessionRef::Uid(2),
                text: "fix the build".into(),
                wait: true,
                timeout: None
            })
        );
        assert_eq!(
            parse(&words("notify Done --pane 3 all green")),
            Ok(Request::Notify { title: "Done".into(), body: Some("all green".into()), pane: Some(3), session: None })
        );
        assert_eq!(
            parse(&words("notify Done all --session 4 green")),
            Ok(Request::Notify {
                title: "Done".into(),
                body: Some("all green".into()),
                pane: None,
                session: Some(SessionRef::Uid(4))
            })
        );
        assert_eq!(
            parse(&words("notify Done")),
            Ok(Request::Notify { title: "Done".into(), body: None, pane: None, session: None })
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
            Ok(Request::SessionPrompt { session: SessionRef::Uid(2), text: "go".into(), wait: true, timeout: Some(9) })
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
        assert!(parse(&words("session new --prompt")).unwrap_err().contains("needs a message"));
        assert!(parse(&words("session new --title")).unwrap_err().contains("needs a value"));
        assert!(parse(&words("session new --pane left")).unwrap_err().contains("isn't right"));
        assert!(parse(&words("session new --colour red")).unwrap_err().contains("usage"));
        assert!(parse(&words("session fork 2 --at 0")).unwrap_err().contains("message number"));
        assert!(parse(&words("session fork 2 --prompt hi")).unwrap_err().contains("usage"));
        assert!(parse(&words("session pin 2 --on")).unwrap_err().contains("usage"));
        assert!(parse(&words("notify")).unwrap_err().contains("needs a title"));
        assert!(parse(&words("notify --pane 2")).unwrap_err().contains("needs a title"));
        assert!(parse(&words("notify hi --pane x")).unwrap_err().contains("isn't a number"));
        assert!(parse(&words("notify hi --session")).unwrap_err().contains("needs a value"));
        assert!(parse(&words("notify hi --pane 1 --session 2")).unwrap_err().contains("not both"));
        assert!(parse(&words("request wait x")).unwrap_err().contains("isn't a number"));
    }

    #[test]
    fn session_requests_from_words() {
        assert_eq!(
            parse(&words("session new --title Fix --pane none --why tests --prompt fix the --pane build")),
            Ok(Request::SessionNew {
                workspace: None,
                prompt: Some("fix the --pane build".into()),
                title: Some("Fix".into()),
                pane: Some(Place::Hidden),
                why: Some("tests".into()),
            })
        );
        let Ok(Request::SessionNew { workspace: Some(w), .. }) = parse(&words("session new --workspace sub")) else {
            panic!()
        };
        assert!(Path::new(&w).is_absolute());
        assert_eq!(
            parse(&words("session fork 5f3c --at 3 --pane tab")),
            Ok(Request::SessionFork { session: SessionRef::Id("5f3c".into()), at: Some(3), pane: Some(Place::Tab) })
        );
        assert_eq!(parse(&words("session open 5f3c")), Ok(Request::SessionOpen { session: "5f3c".into(), pane: None }));
        assert_eq!(parse(&words("session close 4")), Ok(Request::SessionClose { session: SessionRef::Uid(4) }));
        assert_eq!(
            parse(&words("session rename 4 Login fix")),
            Ok(Request::SessionRename { session: SessionRef::Uid(4), title: "Login fix".into() })
        );
        assert_eq!(parse(&words("session archive 4")), Ok(Request::SessionArchive { session: SessionRef::Uid(4) }));
        assert_eq!(
            parse(&words("session pin 4 --off")),
            Ok(Request::SessionPin { session: SessionRef::Uid(4), off: true })
        );
        assert_eq!(
            parse(&words("session read 5f3c")),
            Ok(Request::SessionRead { session: SessionRef::Id("5f3c".into()) })
        );
        assert_eq!(
            parse(&words("workspace create fix/x --session")),
            Ok(Request::WorkspaceCreate { branch: "fix/x".into(), session: true, prompt: None, why: None })
        );
        assert_eq!(
            parse(&words("workspace create fix/x --prompt do it")),
            Ok(Request::WorkspaceCreate {
                branch: "fix/x".into(),
                session: true,
                prompt: Some("do it".into()),
                why: None
            })
        );
        assert_eq!(parse(&words("request status 7")), Ok(Request::RequestStatus { request: 7 }));
        assert_eq!(
            parse(&words("request wait 7 --timeout 60")),
            Ok(Request::RequestWait { request: 7, timeout: Some(60) })
        );
        assert!(is_command(&words("request status 7")));
    }

    #[test]
    fn folders_are_made_absolute() {
        let Ok(Request::ProjectOpen { path }) = parse(&words("project open sub")) else { panic!() };
        assert!(Path::new(&path).is_absolute());
    }
}
