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
    "where",
    "get-command",
    "gcm",
    "which",
    "stat",
    "file",
    "tree",
    "du",
    "df",
    "uname",
    "id",
    "realpath",
    "readlink",
    "dirname",
    "basename",
    "diff",
    "uniq",
    "cut",
    "tr",
    "nl",
    "bat",
    "jq",
    "sha256sum",
    "md5sum",
    // Shaping output.
    "select-object",
    "select",
    "where-object",
    "where",
    "?",
    "foreach-object",
    "foreach",
    "%",
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

/// PowerShell shaping cmdlets that accept a `{ ... }` script block or
/// `@{ ... }` calculated-property hashtable.
const BLOCK_SHAPERS: &[&str] = &[
    "foreach-object",
    "foreach",
    "%",
    "where-object",
    "where",
    "?",
    "select-object",
    "select",
    "sort-object",
    "sort",
    "group-object",
    "measure-object",
    "format-table",
    "ft",
    "format-list",
    "fl",
];

/// Whether `command` only reads.
pub fn is_read_only(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() || command.len() > 2000 {
        return false;
    }
    let command = strip_stderr_redirects(command);
    // Running a subexpression, escaping (PowerShell's backtick), or a newline
    // that starts another command.
    if command.contains(['`', '\n', '\r']) || command.contains("$(") || command.contains("@(") {
        return false;
    }
    let Some(parts) = split_parts(&command) else {
        return false;
    };
    !parts.is_empty() && parts.iter().all(|part| part_reads(part))
}

/// Splits `command` on `|`, `;`, `&&`, and `||` outside quotes and `{ ... }` blocks.
/// Returns `None` on unclosed quotes/braces, redirection (`>`), or a lone `&` outside quotes.
pub(crate) fn split_parts(command: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let bytes = command.as_bytes();
    let mut quote: Option<u8> = None;
    let mut brace_depth: usize = 0;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match (quote, b) {
            (Some(q), c) if c == q => {
                quote = None;
                i += 1;
            }
            (Some(_), _) => {
                i += 1;
            }
            (None, b'"' | b'\'') => {
                quote = Some(b);
                i += 1;
            }
            (None, b'>') => return None,
            (None, b'{') => {
                brace_depth += 1;
                i += 1;
            }
            (None, b'}') => {
                brace_depth = brace_depth.checked_sub(1)?;
                i += 1;
            }
            (None, b'|') if brace_depth == 0 => {
                let part = command[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                i += if bytes.get(i + 1) == Some(&b'|') { 2 } else { 1 };
                start = i;
            }
            (None, b';') if brace_depth == 0 => {
                let part = command[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                i += 1;
                start = i;
            }
            (None, b'&') if brace_depth == 0 => {
                if bytes.get(i + 1) != Some(&b'&') {
                    return None;
                }
                let part = command[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                i += 2;
                start = i;
            }
            (None, b'&') => {
                // Inside `{ ... }`, reject bare `&` call operator; allow `&&`.
                if bytes.get(i + 1) != Some(&b'&') {
                    return None;
                }
                i += 2;
            }
            (None, _) => {
                i += 1;
            }
        }
    }
    if quote.is_some() || brace_depth != 0 {
        return None;
    }
    let tail = command[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Some(parts)
}

/// Removes harmless stderr-only redirections (`2>&1`, `2>$null`, `2>/dev/null`)
/// when they stand as their own tokens, so they don't trip the `>`/`&` check.
pub(crate) fn strip_stderr_redirects(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let at_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if at_boundary {
            let rest = &command[i..];
            let matched = ["2>&1", "2>$null", "2>/dev/null"].iter().find(|pat| {
                rest.len() >= pat.len()
                    && rest[..pat.len()].eq_ignore_ascii_case(pat)
                    && rest.as_bytes().get(pat.len()).is_none_or(|b| matches!(b, b' ' | b'\t' | b'|' | b';' | b'&'))
            });
            if let Some(pat) = matched {
                i += pat.len();
                continue;
            }
        }
        let ch = command[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Whether one command, with no separators in it, only reads.
fn part_reads(part: &str) -> bool {
    let words = words(part);
    let Some(first) = words.first() else { return false };
    let first = first.to_ascii_lowercase();
    let first = first.strip_suffix(".exe").unwrap_or(&first);
    if part.contains('{') || part.contains('}') {
        if !BLOCK_SHAPERS.contains(&first) {
            return false;
        }
        return blocks_read_only(part);
    }
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
    if first == "gh" {
        return gh_reads(&words[1..]);
    }
    READERS.contains(&first)
}

/// Validates every `{ ... }` script block in a PowerShell shaping cmdlet.
/// Allows property/expression access, `[PSCustomObject]@{ ... }` hashtables,
/// and read-only subexpressions like `(Get-Content $_.FullName).Count`, while
/// rejecting mutating commands or .NET write methods.
fn blocks_read_only(part: &str) -> bool {
    let bytes = part.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        match (quote, bytes[i]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(bytes[i]),
            (None, b'{') => {
                let Some(end) = matching_delim(part, i, b'{', b'}') else {
                    return false;
                };
                if !script_block_reads(&part[i + 1..end]) {
                    return false;
                }
                i = end + 1;
                continue;
            }
            (None, b'}') => return false,
            (None, _) => {}
        }
        i += 1;
    }
    true
}

/// Whether the body of a `{ ... }` PowerShell block only reads/shapes data.
fn script_block_reads(body: &str) -> bool {
    let body = body.trim();
    if body.is_empty() {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    // Reject mutating .NET static/instance calls.
    for bad in
        ["::delete", "::write", "::create", "::move", "::copy", "::remove", "::set", ".delete(", ".move(", ".copy("]
    {
        if lower.contains(bad) {
            return false;
        }
    }
    let Some(stmts) = split_parts(body) else {
        return false;
    };
    stmts.into_iter().all(block_stmt_reads)
}

/// Whether one statement inside a `{ ... }` block is read-only.
fn block_stmt_reads(stmt: &str) -> bool {
    let stmt = stmt.trim();
    if stmt.is_empty() {
        return true;
    }
    // Check any parenthesized subexpressions `(Get-Location)` / `(Get-Content ...)`.
    if !parens_read_only(stmt) {
        return false;
    }
    // Expression starting with `$`, `[`, `(`, `'`, `"`, `@`, or a digit:
    // e.g. `$_.Length -gt 10`, `[PSCustomObject]@{ Path = ...; Lines = ... }`.
    let first = stmt.as_bytes()[0];
    if matches!(first, b'$' | b'[' | b'(' | b'\'' | b'"' | b'@' | b'0'..=b'9') {
        // Validate any nested `{ ... }` (e.g. `@{ ... }` hashtable entries).
        return blocks_read_only(stmt);
    }
    // Inside `@{ Key = Expr; Key2 = Expr2 }`, an entry looks like `Name = <expr>`.
    if let Some((lhs, rhs)) = stmt.split_once('=')
        && !lhs
            .contains(|c: char| c.is_whitespace() && !lhs.trim().chars().all(|k| k.is_ascii_alphanumeric() || k == '_'))
        && lhs.trim().chars().all(|k| k.is_ascii_alphanumeric() || k == '_')
        && !lhs.trim().is_empty()
    {
        return block_stmt_reads(rhs);
    }
    // Otherwise it's a command invocation: must pass `part_reads`.
    part_reads(stmt)
}

/// Checks every top-level `(...)` group in `expr`: if it starts with a command
/// name (not a variable, literal, or nested expr), it must pass `is_read_only`.
fn parens_read_only(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        match (quote, bytes[i]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(bytes[i]),
            (None, b'(') => {
                let Some(end) = matching_delim(expr, i, b'(', b')') else {
                    return false;
                };
                let inner = expr[i + 1..end].trim();
                // Skip method call argument lists like `.Replace(a, b)`: preceded by an identifier char.
                let is_call_args = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                if !is_call_args && !inner.is_empty() {
                    let b0 = inner.as_bytes()[0];
                    if b0.is_ascii_alphabetic() {
                        if !is_read_only(inner) {
                            return false;
                        }
                    } else if !parens_read_only(inner) {
                        return false;
                    }
                }
                i = end + 1;
                continue;
            }
            (None, _) => {}
        }
        i += 1;
    }
    true
}

/// Finds the matching closing delimiter for `open` at `start`, respecting quotes.
fn matching_delim(s: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match (quote, b) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, c) if c == open => depth += 1,
            (None, c) if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            (None, _) => {}
        }
    }
    None
}

/// Whether `gh <args>` only reads.
fn gh_reads(args: &[String]) -> bool {
    let Some(first) = args.first().map(|s| s.to_ascii_lowercase()) else { return false };
    let second = args.get(1).map(|s| s.to_ascii_lowercase());
    matches!(
        (first.as_str(), second.as_deref()),
        ("status" | "version" | "--version", _)
            | ("search", Some(_))
            | ("auth", Some("status"))
            | ("pr", Some("list" | "view" | "status" | "diff" | "checks"))
            | ("issue", Some("list" | "view" | "status"))
            | ("repo" | "run" | "release" | "workflow", Some("list" | "view"))
    )
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
            "git.exe status 2>&1",
            "rg -n foo src 2>$null",
            "rg -n foo src 2>/dev/null | head -20",
            "gh pr list --state open",
            "gh issue view 511 --json title",
            "gh status",
            "gh auth status",
            "gh run view 123",
            "gh search issues foo",
            "where.exe git",
            r#"git grep -n "agents_pane\|AgentsPane\|Pane::Agents" -- src/; echo "---475---"; git grep -n "fn shell_of\|pwsh" -- crates/ src/"#,
            r#"rg "foo|bar" src"#,
            r#"Get-ChildItem -Path src, crates\kynex-core\src -Recurse -File -Filter *.rs | ForEach-Object { [PSCustomObject]@{ Path = $_.FullName.Replace((Get-Location).Path + '\', ''); Lines = (Get-Content $_.FullName).Count } } | Sort-Object Lines -Descending | Select-Object -First 20 | Format-Table -AutoSize"#,
            r#"Get-ChildItem src | Where-Object { $_.Length -gt 1000 } | Select-Object Name, Length"#,
            r#"Get-ChildItem src | % { $_.Name }"#,
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
            "git status 2>err.txt",
            "gh pr create --title x",
            "gh issue close 1",
            "gh api -X POST /repos/a/b",
            "Get-ChildItem | Out-File list.txt",
            "Get-Content a.txt | Set-Content b.txt",
            "powershell -Command Get-ChildItem",
            "Get-ChildItem $(Remove-Item x)",
            "Get-ChildItem | ForEach-Object { Remove-Item $_ }",
            "Get-ChildItem | ForEach-Object { [System.IO.File]::Delete($_.FullName) }",
            "Get-ChildItem | ForEach-Object { (Remove-Item $_) }",
            "Get-ChildItem | Where-Object { $_.Delete() }",
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
        // A separator inside quotes stays inside the argument; unclosed quotes still ask.
        assert!(is_read_only("echo 'a; Remove-Item x'"));
        assert!(!is_read_only("echo 'a; Remove-Item x"));
    }
}
