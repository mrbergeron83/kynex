//! `knx` subcommands into a [`Request`], plus `knx hook <provider> [event]`.

use std::path::Path;

use super::Request;
use super::protocol::{PaneTarget, Place};
use crate::layout::Direction;

const GROUPS: [&str; 7] = ["hook", "project", "workspace", "pane", "session", "events", "api"];

pub fn is_command(args: &[String]) -> bool {
    args.first().is_some_and(|a| GROUPS.contains(&a.as_str()))
}

pub fn wants_schema(args: &[String]) -> bool {
    matches!(args, [a, b] if a == "api" && b == "schema")
}

pub fn wants_hook(args: &[String]) -> Option<(&str, Option<&str>)> {
    match args {
        [h, provider] if h == "hook" => Some((provider.as_str(), None)),
        [h, provider, event] if h == "hook" => Some((provider.as_str(), Some(event.as_str()))),
        _ => None,
    }
}

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
        ["workspace", "create", branch, rest @ ..] => {
            let o = options(rest, &["--provider"])?;
            Ok(Request::WorkspaceCreate {
                branch: branch.to_string(),
                provider: o.value("--provider").map(str::to_string),
            })
        }
        ["workspace", "remove", p] => Ok(Request::WorkspaceRemove { path: absolute(p), force: false }),
        ["workspace", "remove", p, "--force"] => Ok(Request::WorkspaceRemove { path: absolute(p), force: true }),
        ["pane", "list"] => Ok(Request::PaneList { all: false }),
        ["pane", "list", "--all"] => Ok(Request::PaneList { all: true }),
        ["pane", "split"] => Ok(Request::PaneSplit { axis: None }),
        ["pane", "split", a @ ("row" | "column")] => Ok(Request::PaneSplit { axis: Some(a.to_string()) }),
        ["pane", "focus", n] => Ok(Request::PaneFocus { pane: number(n)? }),
        ["pane", "move", n, d] => Ok(Request::PaneMove { pane: number(n)?, to: PaneTarget::Dir(direction(d)?) }),
        ["pane", "swap", n, o] => Ok(Request::PaneSwap { pane: number(n)?, other: number(o)? }),
        ["pane", "resize", n, d] => Ok(Request::PaneResize { pane: number(n)?, dir: direction(d)?, by: None }),
        ["pane", "resize", n, d, "--by", px] => {
            let by = px.parse().ok().filter(|p| *p > 0).ok_or_else(|| format!("`{px}` isn't a number of pixels"))?;
            Ok(Request::PaneResize { pane: number(n)?, dir: direction(d)?, by: Some(by) })
        }
        ["pane", "zoom", n] => Ok(Request::PaneZoom { pane: number(n)?, off: false }),
        ["pane", "zoom", n, "--off"] => Ok(Request::PaneZoom { pane: number(n)?, off: true }),
        ["pane", "close"] => Ok(Request::PaneClose { pane: None }),
        ["pane", "close", n] => Ok(Request::PaneClose { pane: Some(number(n)?) }),
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
        ["session", "list"] => Ok(Request::SessionList),
        ["session", "new", rest @ ..] => {
            let o = options(rest, &["--workspace", "--provider", "--model", "--pane"])?;
            Ok(Request::SessionNew {
                workspace: o.value("--workspace").map(absolute),
                provider: o.value("--provider").map(str::to_string),
                model: o.value("--model").map(str::to_string),
                pane: o.value("--pane").map(Place::parse).transpose()?,
            })
        }
        ["session", "open", id, rest @ ..] => {
            let o = options(rest, &["--pane"])?;
            Ok(Request::SessionOpen { session: id.to_string(), pane: o.value("--pane").map(Place::parse).transpose()? })
        }
        ["session", "rename", id, title @ ..] => {
            Ok(Request::SessionRename { session: id.to_string(), title: title.join(" ") })
        }
        ["session", "archive", id] => Ok(Request::SessionArchive { session: id.to_string() }),
        ["session", "pin", id] => Ok(Request::SessionPin { session: id.to_string(), off: false }),
        ["session", "pin", id, "--off"] => Ok(Request::SessionPin { session: id.to_string(), off: true }),
        _ => Err(USAGE.into()),
    }
}

fn number(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("`{s}` isn't a number"))
}

fn direction(s: &str) -> Result<Direction, String> {
    match s {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        _ => Err(format!("`{s}` isn't left, right, up or down")),
    }
}

fn count(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("`{s}` isn't a number of lines"))
}

#[derive(Debug, Default, PartialEq)]
struct Flags {
    text: String,
    wait: bool,
    timeout: Option<u64>,
}

fn flags(rest: &[&str], missing: Option<&str>) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut words = rest.to_vec();
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

#[derive(Debug, Default)]
struct Options<'a> {
    values: Vec<(&'a str, &'a str)>,
}

impl<'a> Options<'a> {
    fn value(&self, name: &str) -> Option<&'a str> {
        self.values.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

fn options<'a>(rest: &[&'a str], valued: &[&str]) -> Result<Options<'a>, String> {
    let mut o = Options::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if valued.contains(&w) {
            let v = rest.get(i + 1).ok_or_else(|| format!("{w} needs a value"))?;
            o.values.push((w, v));
            i += 2;
        } else {
            return Err(USAGE.into());
        }
    }
    Ok(o)
}

fn seconds(s: &str) -> Result<u64, String> {
    s.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("`{s}` isn't a number of seconds"))
}

fn absolute(p: &str) -> String {
    std::path::absolute(Path::new(p)).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
}

pub const USAGE: &str = "usage:
  knx project list | create <folder> | open <folder> | remove <folder>
  knx workspace list | open <folder> | create <branch> [--provider <p>] | remove <folder> [--force]
  knx pane list [--all] | split [row|column] | focus <id> | close [id]
  knx pane move <id> left|right|up|down | swap <id> <other>
  knx pane resize <id> left|right|up|down [--by <px>] | zoom <id> [--off]
  knx pane read <id> [--lines N] | run <id|new> <command…> [--wait] | prompt <id> <message…> [--wait] | wait <id>
  knx session list | new [--workspace <folder>] [--provider <p>] [--model <m>] [--pane right|down]
  knx session open <id> [--pane right|down] | rename <id> <title…> | archive <id> | pin <id> [--off]
  knx events
  knx api schema";

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn subcommands_and_hooks_parse() {
        assert!(is_command(&words("hook claude Stop")));
        assert_eq!(wants_hook(&words("hook claude Stop")), Some(("claude", Some("Stop"))));
        assert_eq!(wants_hook(&words("hook agy")), Some(("agy", None)));
        assert_eq!(
            parse(&words("session new --provider claude-openrouter --model qwen/qwen3-coder --pane down")),
            Ok(Request::SessionNew {
                workspace: None,
                provider: Some("claude-openrouter".into()),
                model: Some("qwen/qwen3-coder".into()),
                pane: Some(Place::Down),
            })
        );
        assert_eq!(parse(&words("pane list --all")), Ok(Request::PaneList { all: true }));
        assert_eq!(
            parse(&words("pane prompt 5 fix tests --wait --timeout 30")),
            Ok(Request::PanePrompt { pane: 5, text: "fix tests".into(), wait: true, timeout: Some(30) })
        );
    }
}
