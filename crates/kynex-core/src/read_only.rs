//! Whether a shell command only reads, so an agent may run it without asking,
//! as Claude Code lets `ls` or `git status` through. Used for agents whose own
//! allowlist Kynex can't set (Antigravity's shell tool over ACP).
//!
//! Conservative on purpose: a command passes only when every part of it (split
//! on `|`, `;`, `&&`, `||`) starts with a known reader and nothing in it could
//! write, run other code or reach outside (redirection, subexpressions, call
//! operators, `-Command`, a git subcommand that changes things).

/// PowerShell cmdlets, their aliases and plain programs that only read or
/// shape what they're given. Compared without case.
const READERS: &[&str] = &[
    // Files and folders.
    "get-childitem",
    "gci",
    "ls",
    "dir",
    "get-content",
    "gc",
    "cat",
    "type",
    "get-item",
    "gi",
    "get-itemproperty",
    "test-path",
    "resolve-path",
    "split-path",
    "join-path",
    "get-location",
    "gl",
    "pwd",
    "get-filehash",
    "get-acl",
    // Searching.
    "select-string",
    "sls",
    "rg",
    "grep",
    "findstr",
    "find",
    "where.exe",
    "get-command",
    "gcm",
    "which",
    // Shaping output.
    "select-object",
    "select",
    "where-object",
    "where",
    "?",
    "sort-object",
    "sort",
    "measure-object",
    "measure",
    "group-object",
    "format-table",
    "ft",
    "format-list",
    "fl",
    "out-string",
    "convertto-json",
    "convertfrom-json",
    "head",
    "tail",
    "wc",
    "echo",
    "write-output",
    "write-host",
    // About the machine.
    "get-date",
    "get-process",
    "ps",
    "get-psdrive",
    "hostname",
    "whoami",
    "get-host",
    "$psversiontable",
];

/// Git subcommands that only read. `branch`, `tag`, `remote` and `stash` read
/// only when they list (see [`git_reads`]).
const GIT_READERS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "blame",
    "describe",
    "shortlog",
    "cat-file",
    "grep",
    "reflog",
    "rev-list",
    "merge-base",
    "name-rev",
    "count-objects",
    "version",
    "--version",
];

/// Whether `command` only reads.
pub fn is_read_only(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() || command.len() > 2000 {
        return false;
    }
    // Writing to a file, running a subexpression or a script block, escaping
    // (PowerShell's backtick, a newline that starts another command).
    if command.contains(['>', '`', '\n', '\r', '{', '}']) || command.contains("$(") || command.contains("@(") {
        return false;
    }
    // `&&` and `||` chain commands; a lone `&` is PowerShell's call operator (or
    // bash's background), which runs whatever follows.
    let chained = command.replace("&&", ";").replace("||", ";");
    if chained.contains('&') {
        return false;
    }
    chained.split(['|', ';']).filter(|p| !p.trim().is_empty()).all(|part| part_reads(part.trim()))
}

/// Whether one command, with no separators in it, only reads.
fn part_reads(part: &str) -> bool {
    let words = words(part);
    let Some(first) = words.first() else { return false };
    let first = first.to_ascii_lowercase();
    // Anything that runs other code, writes as it goes, or never ends.
    let risky = |w: &String| {
        let w = w.to_ascii_lowercase();
        matches!(w.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir" | "-delete" | "-fprint" | "-fls" | "--output")
            || w.starts_with("--output=")
            || w.starts_with("-outfile")
            || w.starts_with("-wait")
    };
    if words.iter().skip(1).any(risky) {
        return false;
    }
    // Unix `sort -o file` writes.
    if first == "sort" && words.iter().skip(1).any(|w| w.starts_with("-o")) {
        return false;
    }
    if first == "git" {
        return git_reads(&words[1..]);
    }
    READERS.contains(&first.as_str())
}

/// Whether `git <args>` only reads.
fn git_reads(args: &[String]) -> bool {
    // Options before the subcommand, like `-C dir` or `--no-pager` (case matters:
    // `-C` is a folder, `-c key=value` can run anything through aliases and pagers).
    let mut rest = args.iter();
    let sub = loop {
        match rest.next().map(String::as_str) {
            Some("-c") => return false,
            Some(a) if a.starts_with("--config-env") || a.starts_with("--exec-path") => return false,
            Some("-C") => {
                rest.next();
            }
            Some("--version") => break "--version".to_string(),
            Some(a) if a.starts_with('-') => {}
            Some(a) => break a.to_ascii_lowercase(),
            None => return false,
        }
    };
    let rest: Vec<String> = rest.map(|a| a.to_ascii_lowercase()).collect();
    // These read only when they list: no other argument, or only listing flags.
    let lists = |flags: &[&str]| rest.iter().all(|a| flags.contains(&a.as_str()));
    match sub.as_str() {
        "branch" => lists(&["-a", "--all", "-r", "--remotes", "-v", "-vv", "--list", "--show-current"]),
        "tag" => lists(&["-l", "--list"]),
        "remote" => lists(&["-v", "--verbose"]),
        "stash" => rest.first().is_some_and(|a| a == "list" || a == "show"),
        "config" => rest.first().is_some_and(|a| matches!(a.as_str(), "--get" | "--list" | "-l" | "--get-all")),
        s => GIT_READERS.contains(&s),
    }
}

/// `part` split into words, quotes kept together and taken off.
fn words(part: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    for c in part.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, c) => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_commands_pass() {
        for c in [
            "Get-ChildItem",
            "Get-ChildItem -Recurse -Filter *.rs | Select-Object -First 20",
            "gci src | Where-Object Name -like '*.rs'",
            "Get-Content README.md -TotalCount 40",
            "Select-String -Path src\\*.rs -Pattern 'fn main'",
            "ls; pwd",
            "dir /s /b",
            "rg -n \"TODO\" crates",
            "git status",
            "git -C crates/kynex-core log --oneline -5",
            "git --no-pager diff HEAD~1 -- src",
            "git branch -a",
            "git stash list",
            "Test-Path Cargo.toml && Get-Item Cargo.toml",
            "echo done",
            "rg -e foo -c src",
            "git --version",
        ] {
            assert!(is_read_only(c), "{c}");
        }
    }

    #[test]
    fn anything_that_could_write_or_run_code_asks() {
        for c in [
            "",
            "Remove-Item foo",
            "Get-ChildItem | Remove-Item",
            "Get-Content a.txt > b.txt",
            "Get-ChildItem | Out-File list.txt",
            "Get-Content a.txt | Set-Content b.txt",
            "powershell -Command Get-ChildItem",
            "Get-ChildItem $(Remove-Item x)",
            "Get-ChildItem | ForEach-Object { Remove-Item $_ }",
            "Invoke-Expression 'ls'",
            "iex ls",
            "& ls",
            "ls & rm x",
            "sort -o out.txt in.txt",
            "git --exec-path=/tmp log",
            "git push",
            "git commit -m x",
            "git branch -D main",
            "git tag v1",
            "git stash",
            "git -c core.pager=evil log",
            "git config user.name x",
            "git diff --output=x.patch",
            "find . -delete",
            "find . -exec rm {} ;",
            "Get-Content log.txt -Wait",
            "Invoke-WebRequest https://example.com -OutFile x",
            "cargo build",
            "npm test",
            "ls\nrm -rf /",
            "ls `; rm x",
        ] {
            assert!(!is_read_only(c), "{c}");
        }
    }

    #[test]
    fn quotes_keep_words_together() {
        assert_eq!(words(r#"Select-String -Pattern "a b" 'c d'"#), ["Select-String", "-Pattern", "a b", "c d"]);
        // A separator inside quotes still splits: asking is the safe side.
        assert!(!is_read_only("echo 'a; Remove-Item x'"));
    }
}
