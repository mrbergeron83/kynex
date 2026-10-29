use super::Token::*;
use super::*;

/// The tokens of `code` as (text, kind), checking the ranges on the way.
fn toks(lang: Lang, code: &str) -> Vec<(&str, Token)> {
    let ranges = highlight(lang, code);
    check(code, &ranges);
    ranges.into_iter().map(|(r, t)| (&code[r], t)).collect()
}

fn check(text: &str, ranges: &[(Range<usize>, Token)]) {
    let mut end = 0;
    for (r, _) in ranges {
        assert!(r.start >= end && r.start < r.end && r.end <= text.len(), "{ranges:?} in {text:?}");
        assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end), "{ranges:?} in {text:?}");
        end = r.end;
    }
}

fn has(lang: Lang, code: &str, want: &[(&str, Token)]) {
    let got = toks(lang, code);
    for w in want {
        assert!(got.contains(w), "{w:?} not in {got:?}");
    }
}

#[test]
fn rust_basics() {
    has(
        Lang::Rust,
        "pub fn main() -> Vec<u32> { let x = 0x1F_u8 + 1.5e-3; println!(\"hi {x}\"); Foo::new(MAX) } // done",
        &[
            ("pub", Keyword),
            ("fn", Keyword),
            ("main", Function),
            ("Vec", Type),
            ("u32", Type),
            ("0x1F_u8", Number),
            ("1.5e-3", Number),
            ("println!", Function),
            ("\"hi {x}\"", String),
            ("Foo", Type),
            ("new", Function),
            ("MAX", Number),
            ("// done", Comment),
        ],
    );
    has(Lang::Rust, "fn run<T>(x: T)", &[("run", Function)]);
    has(Lang::Rust, "struct Wrapper;", &[("Wrapper", Type)]);
}

#[test]
fn punctuation_runs_are_one_range() {
    has(Lang::Rust, "a::<b>()", &[("::<", Punctuation), (">()", Punctuation)]);
    has(Lang::JavaScript, "x => {});", &[("=>", Punctuation), ("{});", Punctuation)]);
}

#[test]
fn lifetimes_are_not_chars() {
    has(
        Lang::Rust,
        "fn f<'a>(x: &'a str, c: char) -> bool { c == 'x' || c == '\\'' || c == 'é' || x == \"'\" }",
        &[("'a", Keyword), ("'x'", String), ("'\\''", String), ("'é'", String), ("\"'\"", String), ("str", Type)],
    );
    has(Lang::Rust, "impl<'de> X<'de> for Y {}", &[("'de", Keyword), ("Y", Type)]);
    has(Lang::Rust, "let b = b'x'; let s = b\"y\";", &[("b'x'", String), ("b\"y\"", String)]);
}

#[test]
fn rust_raw_strings_and_nested_comments_span_lines() {
    let code = "let s = r#\"one \" still\ntwo\"#; x\n/* a /* b */ c\nd */ y";
    let got = toks(Lang::Rust, code);
    assert!(got.contains(&("r#\"one \" still", String)), "{got:?}");
    assert!(got.contains(&("two\"#", String)), "{got:?}");
    assert!(got.contains(&("/* a /* b */ c", Comment)), "{got:?}");
    assert!(got.contains(&("d */", Comment)), "{got:?}");
    assert!(!got.iter().any(|(s, _)| *s == "y" || *s == "x"), "{got:?}");
}

#[test]
fn block_comment_spans_lines() {
    let mut h = Highlighter::new(Lang::C);
    assert_eq!(h.line("int a; /* start"), vec![(0..3, Type), (5..6, Punctuation), (7..15, Comment)]);
    assert_eq!(h.line("middle \"not a string"), vec![(0..20, Comment)]);
    assert_eq!(h.line("end */ return;"), vec![(0..6, Comment), (7..13, Keyword), (13..14, Punctuation)]);
}

#[test]
fn diff_sides_keep_their_own_state() {
    let lines = [
        (Side::Both, "let a = 1;"),
        (Side::Old, "/* old comment"),
        (Side::Old, "still old */"),
        (Side::Old, "let x = r\"open"),
        (Side::New, "let b = 2;"),
        (Side::Both, "done();"),
        (Side::New, "fn f() {}"),
    ];
    let got = diff(Lang::Rust, lines);
    assert_eq!(got.len(), lines.len());
    assert_eq!(got[2], vec![(0..12, Comment)]);
    assert_eq!(got[4][0], (0..3, Keyword), "the old comment leaked: {:?}", got[4]);
    // Context lines take the new side, where no string is open.
    assert_eq!(got[5][0], (0..4, Function), "{:?}", got[5]);
    assert_eq!(got[6][1], (3..4, Function));
}

#[test]
fn python() {
    has(
        Lang::Python,
        "@app.route('/')\ndef index(self) -> None:\n    return f\"{x}\" + rb'\\x00' # hi",
        &[
            ("@app.route", Function),
            ("'/'", String),
            ("def", Keyword),
            ("index", Function),
            ("self", Keyword),
            ("None", Keyword),
            ("f\"{x}\"", String),
            ("rb'\\x00'", String),
            ("# hi", Comment),
        ],
    );
    let got = toks(Lang::Python, "s = \"\"\"doc\nstill 'doc'\n\"\"\" + 1");
    assert_eq!(&got[..4], &[("=", Punctuation), ("\"\"\"doc", String), ("still 'doc'", String), ("\"\"\"", String)]);
    assert_eq!(got[5], ("1", Number));
}

#[test]
fn javascript_template_literal_spans_lines() {
    let got = toks(Lang::JavaScript, "const s = `a\n${b} // no\n`; export default $el;");
    assert!(got.contains(&("${b} // no", String)), "{got:?}");
    assert!(got.contains(&("export", Keyword)), "{got:?}");
    assert!(!got.iter().any(|(_, t)| *t == Comment), "{got:?}");
    has(
        Lang::JavaScript,
        "let n: number = useState(null);",
        &[("number", Type), ("useState", Function), ("null", Keyword)],
    );
}

#[test]
fn go_and_c_family() {
    has(
        Lang::Go,
        "func (s *Server) Run() error { return fmt.Errorf(`raw \\`) }",
        &[("func", Keyword), ("Server", Type), ("Run", Function), ("error", Type), ("`raw \\`", String)],
    );
    has(
        Lang::C,
        "#include <stdio.h>\n@Override public static void main(String[] args) { char c = 'x'; }",
        &[("#include", Keyword), ("@Override", Function), ("void", Type), ("String", Type), ("'x'", String)],
    );
}

#[test]
fn sql_ignores_case() {
    has(
        Lang::Sql,
        "SELECT name, count(*) FROM users WHERE id = 'it''s' -- note",
        &[("SELECT", Keyword), ("count", Function), ("FROM", Keyword), ("'it''s'", String), ("-- note", Comment)],
    );
}

#[test]
fn data_formats() {
    has(
        Lang::Json,
        r#"{"name": "kynex", "n": -1.5e3, "ok": true}"#,
        &[("\"name\"", Type), ("\"kynex\"", String), ("-1.5e3", Number), ("true", Keyword)],
    );
    has(
        Lang::Toml,
        "[[bin]]\nname = \"x\" # c\nfeatures = [\n  \"a\",\n]\nkey = { a = 1, b = true }\nwhen = 1979-05-27",
        &[
            ("bin", Type),
            ("name", Type),
            ("\"x\"", String),
            ("# c", Comment),
            ("\"a\"", String),
            ("a", Type),
            ("b", Type),
            ("1", Number),
            ("true", Keyword),
            ("1979-05-27", Number),
        ],
    );
    let got = toks(Lang::Toml, "s = '''\nnot = key\n'''");
    assert!(got.contains(&("not = key", String)), "{got:?}");
    has(
        Lang::Yaml,
        "---\non:\n  - name: Build it # c\n    run: cargo test\n    if: true\n    n: 3\n    s: \"q\"\n    a: *anchor",
        &[
            ("---", Punctuation),
            ("on", Type),
            ("name", Type),
            ("Build it", String),
            ("# c", Comment),
            ("cargo test", String),
            ("true", Keyword),
            ("3", Number),
            ("\"q\"", String),
            ("*anchor", Function),
        ],
    );
    has(Lang::Yaml, "url: http://x:1/a", &[("url", Type), ("http://x:1/a", String)]);
}

#[test]
fn shell() {
    has(
        Lang::Shell,
        "$ FOO=1 git commit -m \"msg $x\" | grep -v 'y' && echo ${HOME} $1 # done",
        &[
            ("$", Punctuation),
            ("FOO", Type),
            ("git", Function),
            ("\"msg $x\"", String),
            ("grep", Function),
            ("'y'", String),
            ("echo", Function),
            ("${HOME}", Type),
            ("$1", Type),
            ("# done", Comment),
        ],
    );
    let got = toks(Lang::Shell, "if [ -f a#b ]; then sudo rm x; fi");
    assert!(got.contains(&("if", Keyword)) && got.contains(&("then", Keyword)), "{got:?}");
    assert!(got.contains(&("sudo", Function)) && got.contains(&("rm", Function)), "{got:?}");
    assert!(!got.iter().any(|(_, t)| *t == Comment), "{got:?}");
}

#[test]
fn powershell() {
    has(
        Lang::PowerShell,
        "$files = Get-ChildItem -Path $env:USERPROFILE | Where-Object { $_.Length -gt 10 } # c",
        &[
            ("$files", Type),
            ("Get-ChildItem", Function),
            ("$env:USERPROFILE", Type),
            ("Where-Object", Function),
            ("$_", Type),
            ("10", Number),
            ("# c", Comment),
        ],
    );
    has(
        Lang::PowerShell,
        "if ($x -eq $true) { [string]::Join(',', $a) }",
        &[("if", Keyword), ("$true", Keyword), ("string", Type)],
    );
    let got = toks(Lang::PowerShell, "$s = @'\n# not a comment $x\n'@\n<# block\n#> cargo build");
    assert!(got.contains(&("# not a comment $x", String)), "{got:?}");
    assert!(got.contains(&("'@", String)), "{got:?}");
    assert!(got.contains(&("<# block", Comment)) && got.contains(&("#>", Comment)), "{got:?}");
    assert!(got.contains(&("cargo", Function)), "{got:?}");
}

#[test]
fn html_and_css() {
    let got = toks(Lang::Html, "<!-- a\nb --><a href=\"/x\" data-y=z\n  hidden>T&amp;C</a>");
    for w in [
        ("<!-- a", Comment),
        ("b -->", Comment),
        ("<", Punctuation),
        ("a", Keyword),
        ("href", Type),
        ("\"/x\"", String),
        ("z", String),
        ("hidden", Type),
        ("&amp;", Number),
        ("</", Punctuation),
    ] {
        assert!(got.contains(&w), "{w:?} not in {got:?}");
    }
    has(
        Lang::Css,
        "@media (max-width: 600px) {\n  .card:hover, #main { color: #fff; margin: -1.5em !important; background: url(\"a.png\") }\n}",
        &[
            ("@media", Keyword),
            ("600px", Number),
            ("color", Type),
            ("#fff", Number),
            ("-1.5em", Number),
            ("!important", Keyword),
            ("url", Function),
            ("\"a.png\"", String),
        ],
    );
    has(Lang::Css, ".card:hover, #main {}", &[(".card", Type), ("hover", Keyword), ("#main", Type)]);
}

#[test]
fn offsets_shift_per_line() {
    let code = "let a = 1;\r\n\n  let b = \"x\";";
    let got = highlight(Lang::Rust, code);
    check(code, &got);
    assert_eq!(got.first(), Some(&(0..3, Keyword)));
    assert!(got.contains(&(15..18, Keyword)), "{got:?}");
    assert!(got.contains(&(23..26, String)), "{got:?}");
    assert_eq!(&code[23..26], "\"x\"");
}

/// Random text from the bytes that steer the lexers, in every language.
#[test]
fn random_text_keeps_the_rules() {
    let parts = [
        "'", "\"", "`", "#", "/", "*", "<", ">", "!", "-", "@", "$", "{", "}", "[", "]", "(", ")", "=", ":", "\\", "r",
        "b", "x", "é", "😀", " ", "\n", "\r", "1", ".", ",", "&", ";", "|", "?", "%",
    ];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..3000 {
        let mut text = std::string::String::new();
        for _ in 0..(seed % 40) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            text.push_str(parts[(seed % parts.len() as u64) as usize]);
        }
        crate::fuzz::highlight(&text);
    }
}

#[test]
fn never_splits_a_char() {
    let inputs = [
        "é'é' 'é \"é\\é\" ünï/*ç*/ «x» 😀(1) $é ${é} #é <é é=é> @é 1é .é -é",
        "'\u{301}' r#\"é\n\"é#' `é` '''é\n'é'''é: é\n\u{feff}é = [é",
        "\\",
        "'",
        "\"",
        "r#",
        "b'",
        "<",
        "<!--",
        "$",
        "@'",
        "#",
        "/*",
        "--",
        "0x",
        "1e",
        "1e-",
        ".5",
        "'a",
        "\r",
    ];
    for lang in Lang::ALL {
        for text in inputs {
            check(text, &highlight(lang, text));
            let mut h = Highlighter::new(lang);
            for line in text.split('\n') {
                check(line, &h.line(line));
            }
        }
    }
}
