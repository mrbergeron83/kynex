//! Knowing when a command run in a shell has finished, how, and what it printed
//! (`knx pane run --wait`).
//!
//! Shells say it through the window title, which the terminal's listener reads and keeps
//! from the tab: PowerShell and bash with Kynex's hooks ([`super::hooks`]) say `knx-exec:<n>`
//! as a command starts and `knx-prompt:<p>:<n>:<exit>` at each prompt. Other shells get the
//! typed command [`wrap`]ped in a few words that say `knx-done:<n>:<exit>` when it's done.
//! Either way what the command prints is linked (OSC 8, which Kynex doesn't draw) to
//! [`output_link`], so it can be found in the grid however it scrolled or wrapped.

use std::path::Path;

/// What a title says, when it's one of Kynex's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A [`wrap`]ped command `n` finished with `code`.
    Done { n: u64, code: i32 },
    /// The shell shows its `p`th prompt; the last command it ran was its `n`th, which ended
    /// with `code`.
    Prompt { p: u64, n: u64, code: i32 },
    /// The shell starts its `n`th command.
    Exec { n: u64 },
}

/// The mark a title carries, if it is one.
pub fn parse(title: &str) -> Option<Mark> {
    let title = title.trim();
    if let Some(rest) = title.strip_prefix("knx-done:") {
        let (n, code) = rest.split_once(':')?;
        return Some(Mark::Done { n: n.parse().ok()?, code: code.parse().ok()? });
    }
    if let Some(rest) = title.strip_prefix("knx-prompt:") {
        let mut parts = rest.splitn(3, ':');
        let (p, n, code) = (parts.next()?, parts.next()?, parts.next()?);
        return Some(Mark::Prompt { p: p.parse().ok()?, n: n.parse().ok()?, code: code.parse().ok()? });
    }
    let n = title.strip_prefix("knx-exec:")?;
    Some(Mark::Exec { n: n.parse().ok()? })
}

/// The link a hooked shell's command `n` prints under.
pub fn output_link(n: u64) -> String {
    format!("knx:out:{n}")
}

/// The link a [`wrap`]ped command `n` prints under.
pub fn wrapped_link(n: u64) -> String {
    format!("knx:run:{n}")
}

/// `command` as typed with marker `n`, for a shell without Kynex's hooks: what to send for
/// the shell `program` to say when it's done; `None` for a shell it doesn't know. The marker
/// goes on a line of its own, so a command ending in a comment, `&` or `;` still gets it.
/// Only the last line's exit code is told.
pub fn wrap(program: &Path, command: &str, n: u64) -> Option<String> {
    let command = command.trim_end();
    let link = wrapped_link(n);
    Some(match shell_of(program)? {
        // `$?` is false for a failed cmdlet as well as a native command; `$LASTEXITCODE` is
        // reset first so an older one doesn't stand in for a cmdlet's failure.
        Syntax::PowerShell => format!(
            "$global:LASTEXITCODE=0; [Console]::Write(\"$([char]27)]8;;{link}$([char]27)\\\"); {command}\n\
             $k=$(if($?){{0}}elseif($LASTEXITCODE){{$LASTEXITCODE}}else{{1}}); [Console]::Write(\"$([char]27)]8;;$([char]27)\\\"); $Host.UI.RawUI.WindowTitle=\"knx-done:{n}:$k\""
        ),
        // A group, so the link closes before the next prompt; the group's status is the
        // command's. In fish's single quotes `\\` is one backslash, so printf's is `\\\\`.
        Syntax::Fish => format!(
            "begin; printf '\\e]8;;{link}\\e\\\\\\\\'; {command}\nend; printf '\\e]8;;\\e\\\\\\\\\\e]0;knx-done:{n}:%s\\a' $status"
        ),
        Syntax::Posix => format!(
            "{{ printf '\\033]8;;{link}\\033\\\\'; {command}\n}}; printf '\\033]8;;\\033\\\\\\033]0;knx-done:{n}:%s\\007' $?"
        ),
    })
}

/// What a [`wrap`]ped command `n` printed, from the text linked to [`wrapped_link`]: the
/// shell's next prompt with the marker line typed after it goes.
pub fn wrapped_output(linked: &str, n: u64) -> String {
    let marker = format!("knx-done:{n}");
    let lines: Vec<&str> = linked.lines().collect();
    let end = lines.iter().rposition(|l| l.contains(&marker)).unwrap_or(lines.len());
    lines[..end].join("\n")
}

enum Syntax {
    PowerShell,
    Fish,
    Posix,
}

fn shell_of(program: &Path) -> Option<Syntax> {
    let stem = program.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match stem.as_str() {
        "pwsh" | "powershell" => Some(Syntax::PowerShell),
        "fish" => Some(Syntax::Fish),
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" | "ash" => Some(Syntax::Posix),
        _ => None,
    }
}

/// The last `n` lines of `text`, blank lines at its end left out.
pub fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shell_gets_its_own_marker() {
        let ps = wrap(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe"), "dir\n", 4).unwrap();
        assert!(ps.starts_with("$global:LASTEXITCODE=0; [Console]::Write("), "{ps}");
        assert!(ps.contains("knx:run:4"), "{ps}");
        assert!(ps.contains("; dir\n$k=$(if($?)"), "{ps}");
        assert!(ps.ends_with("WindowTitle=\"knx-done:4:$k\""), "{ps}");
        assert!(wrap(Path::new("powershell.exe"), "x", 1).unwrap().contains("WindowTitle"));
        let fish = wrap(Path::new("/usr/bin/fish"), "ls", 2).unwrap();
        assert_eq!(
            fish,
            r"begin; printf '\e]8;;knx:run:2\e\\\\'; ls
end; printf '\e]8;;\e\\\\\e]0;knx-done:2:%s\a' $status"
        );
        let zsh = wrap(Path::new("/bin/zsh"), "ls # all", 3).unwrap();
        assert!(zsh.starts_with("{ printf '\\033]8;;knx:run:3\\033\\\\'; ls # all\n}; printf"), "{zsh}");
        assert!(zsh.ends_with("knx-done:3:%s\\007' $?"), "{zsh}");
        assert!(wrap(Path::new(r"C:\Git\bin\bash.exe"), "ls &", 3).unwrap().contains("; ls &\n}; printf"));
    }

    #[test]
    fn a_shell_it_doesnt_know_gets_no_marker() {
        for other in ["/usr/bin/nu", "cmd.exe", "/usr/bin/xonsh", ""] {
            assert_eq!(wrap(Path::new(other), "ls", 1), None, "{other}");
        }
    }

    #[test]
    fn marks_parse_and_other_titles_dont() {
        assert_eq!(parse("knx-done:7:0"), Some(Mark::Done { n: 7, code: 0 }));
        assert_eq!(parse("knx-done:7:-1073741510"), Some(Mark::Done { n: 7, code: -1073741510 }));
        assert_eq!(parse("knx-prompt:3:2:127"), Some(Mark::Prompt { p: 3, n: 2, code: 127 }));
        assert_eq!(parse("knx-exec:9"), Some(Mark::Exec { n: 9 }));
        for other in [
            "",
            "pwsh",
            "knx-done:",
            "knx-done:x:0",
            "knx-done:1:",
            "knx-done:1:True",
            "a knx-done:1:0",
            "knx-prompt:1:2",
            "knx-exec:",
        ] {
            assert_eq!(parse(other), None, "{other}");
        }
    }

    #[test]
    fn a_wrapped_commands_output_stops_before_its_marker_line() {
        let linked = "hello\nworld\nPS C:\\> $k=$(if($?)...knx-done:5:$k\"\n";
        assert_eq!(wrapped_output(linked, 5), "hello\nworld");
        assert_eq!(wrapped_output("only\n", 5), "only");
    }

    #[test]
    fn tail_skips_the_blank_end() {
        assert_eq!(tail("a\nb\nc\n\n  \n", 2), "b\nc");
        assert_eq!(tail("a\nb", 10), "a\nb");
        assert_eq!(tail("", 3), "");
        assert_eq!(tail("a\nb", 0), "");
    }
}
