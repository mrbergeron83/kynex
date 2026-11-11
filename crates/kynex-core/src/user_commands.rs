//! Custom commands: programs a key or the palette opens in the selected workspace's
//! folder, after herdr's `[[keys.command]]` (one key for lazygit, say). Each is a name, an
//! argv (never a shell string), where it shows (a popup over the window, a pane, or
//! nowhere) and an optional key.
//!
//! There are two lists: the user's, `<data dir>/commands.json`, and each project's,
//! `commands.json` in its settings folder ([`crate::project::dir`]). A project's command
//! replaces the user's of the same name. Both files read as
//! `{ "commands": [ { "name": "lazygit", "argv": ["lazygit"], "placement": "popup", "key": "alt-g" } ] }`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::data_dir;
use crate::terminal::{ShellKind, ShellSpec};

const FILE: &str = "commands.json";

/// Where a command's program shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Over the window, closing when the program exits.
    #[default]
    Popup,
    /// A terminal pane of the workspace.
    Pane,
    /// No window: it runs, and says so only if it fails.
    Background,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserCommand {
    pub name: String,
    /// The program and its arguments, as given: no shell reads them.
    pub argv: Vec<String>,
    #[serde(default)]
    pub placement: Placement,
    /// A key for it, in keymap form (`alt-g`, `ctrl-shift-l`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    commands: Vec<UserCommand>,
}

/// The commands in `json`, the ones that can't run left out: no name, no program, or a
/// name used before. `Err` if it isn't a command list at all.
pub fn parse(json: &str) -> Result<Vec<UserCommand>, String> {
    let file: File = serde_json::from_str(json).map_err(|e| format!("Couldn't read the commands: {e}"))?;
    let mut out: Vec<UserCommand> = Vec::new();
    for mut c in file.commands {
        c.name = c.name.trim().to_string();
        c.key = c.key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let runnable = c.argv.first().is_some_and(|p| !p.trim().is_empty());
        if !c.name.is_empty() && runnable && !out.iter().any(|o| o.name == c.name) {
            out.push(c);
        }
    }
    Ok(out)
}

/// The user's list file.
pub fn user_file() -> PathBuf {
    data_dir().join(FILE)
}

/// Project `project`'s list file.
pub fn project_file(project: &Path) -> PathBuf {
    crate::project::dir(project).join(FILE)
}

/// The commands in `path`; none if it's missing. A file that doesn't read is an `Err` to
/// show. Blocks on the file.
pub fn load(path: &Path) -> Result<Vec<UserCommand>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
    }
}

/// The user's commands with project `project`'s (none for chat): the project's win by
/// name, then come in their file's order after the user's. Errors are for showing; what
/// read fine is still there. Blocks on the files.
pub fn for_project(project: Option<&Path>) -> (Vec<UserCommand>, Vec<String>) {
    let mut errors = Vec::new();
    let mut all = load(&user_file()).unwrap_or_else(|e| {
        errors.push(e);
        Vec::new()
    });
    if let Some(p) = project {
        match load(&project_file(p)) {
            Ok(own) => merge(&mut all, own),
            Err(e) => errors.push(e),
        }
    }
    (all, errors)
}

/// `own` over `base`: a command of the same name takes the other's place.
pub fn merge(base: &mut Vec<UserCommand>, own: Vec<UserCommand>) {
    for c in own {
        match base.iter_mut().find(|b| b.name == c.name) {
            Some(b) => *b = c,
            None => base.push(c),
        }
    }
}

/// What a command's program is told about where it runs.
pub fn env(workspace: &Path, branch: Option<&str>, session_id: Option<&str>) -> Vec<(String, String)> {
    let mut env = vec![("KYNEX_WORKSPACE_DIR".to_string(), workspace.to_string_lossy().into_owned())];
    env.push(("KYNEX_BRANCH".to_string(), branch.unwrap_or_default().to_string()));
    env.push(("KYNEX_SESSION_ID".to_string(), session_id.unwrap_or_default().to_string()));
    env
}

/// `c`'s program, found on PATH unless it's a path (taken from workspace `ws`), ready for a
/// terminal. Blocks on PATH.
pub fn spec(c: &UserCommand, ws: &Path, env: Vec<(String, String)>) -> Result<ShellSpec, String> {
    let program = program(c, ws)?;
    Ok(ShellSpec {
        kind: ShellKind::default_shell(),
        program,
        args: c.argv[1..].to_vec(),
        env,
        title: Some(c.name.clone()),
    })
}

/// `c`'s program on disk: a path as given (a relative one from workspace `ws`), else found
/// on PATH as it is now, with Windows' `.exe`, `.cmd`, `.bat` and `.com` tried for a bare
/// name. Blocks on PATH.
pub fn program(c: &UserCommand, ws: &Path) -> Result<PathBuf, String> {
    let first = c.argv.first().map(|p| p.trim()).filter(|p| !p.is_empty()).ok_or("The command has no program.")?;
    let path = Path::new(first);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if path.components().count() > 1 {
        return Ok(ws.join(path));
    }
    let fresh = crate::fresh_env::vars().into_iter().find(|(n, _)| n.eq_ignore_ascii_case("PATH")).map(|(_, v)| v);
    let dirs = fresh.or_else(|| std::env::var_os("PATH")).unwrap_or_default();
    let names: Vec<String> = if cfg!(windows) && path.extension().is_none() {
        ["exe", "cmd", "bat", "com"].iter().map(|e| format!("{first}.{e}")).collect()
    } else {
        vec![first.to_string()]
    };
    std::env::split_paths(&dirs)
        .find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
        .ok_or_else(|| format!("{first} wasn't found on PATH."))
}

/// A process for `spec` with no window: in `ws`, with PATH as it is now and `spec`'s
/// variables, its console hidden on Windows.
pub fn background(spec: &ShellSpec, ws: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args).current_dir(ws).envs(crate::fresh_env::vars()).envs(spec.env.iter().cloned());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_reads_with_its_defaults_and_without_what_cannot_run() {
        let json = r#"{ "commands": [
            { "name": " lazygit ", "argv": ["lazygit"], "key": "alt-g" },
            { "name": "logs", "argv": ["tail", "-f", "log.txt"], "placement": "pane", "key": " " },
            { "name": "", "argv": ["x"] },
            { "name": "empty", "argv": [] },
            { "name": "blank", "argv": [" "] },
            { "name": "lazygit", "argv": ["other"] },
            { "name": "sync", "argv": ["git", "fetch"], "placement": "background" }
        ] }"#;
        let c = parse(json).unwrap();
        assert_eq!(c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["lazygit", "logs", "sync"]);
        assert_eq!((c[0].placement, c[0].key.as_deref()), (Placement::Popup, Some("alt-g")));
        assert_eq!((c[1].placement, c[1].key.as_deref()), (Placement::Pane, None));
        assert_eq!(c[2].placement, Placement::Background);
        assert_eq!(c[1].argv, ["tail", "-f", "log.txt"]);
    }

    #[test]
    fn a_file_that_is_not_a_list_is_an_error() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{ "commands": "lazygit" }"#).is_err());
        assert_eq!(parse("{}").unwrap(), []);
    }

    #[test]
    fn a_projects_command_takes_the_users_place() {
        let c = |name: &str, p: &str| UserCommand {
            name: name.into(),
            argv: vec![p.into()],
            placement: Placement::Popup,
            key: None,
        };
        let mut base = vec![c("a", "1"), c("b", "2")];
        merge(&mut base, vec![c("b", "3"), c("c", "4")]);
        let got: Vec<_> = base.iter().map(|c| (c.name.as_str(), c.argv[0].as_str())).collect();
        assert_eq!(got, [("a", "1"), ("b", "3"), ("c", "4")]);
    }

    #[test]
    fn the_program_is_told_where_it_runs() {
        let env = env(Path::new("/w"), Some("main"), None);
        assert!(env.contains(&("KYNEX_BRANCH".into(), "main".into())));
        assert!(env.contains(&("KYNEX_SESSION_ID".into(), String::new())));
        assert_eq!(env[0].0, "KYNEX_WORKSPACE_DIR");
    }

    #[test]
    fn a_path_is_used_as_it_is_and_a_missing_program_says_so() {
        let mk =
            |p: &str| UserCommand { name: "x".into(), argv: vec![p.into()], placement: Placement::Pane, key: None };
        let ws = Path::new("/w");
        assert_eq!(program(&mk("./bin/tool"), ws).unwrap(), ws.join("./bin/tool"));
        let e = program(&mk("surely-not-a-program-kynex"), ws).unwrap_err();
        assert!(e.contains("surely-not-a-program-kynex"));
        let s = spec(&UserCommand { argv: vec!["./t".into(), "-a".into()], ..mk("") }, ws, vec![]).unwrap();
        assert_eq!((s.args, s.title.as_deref()), (vec!["-a".to_string()], Some("x")));
    }
}
