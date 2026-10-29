//! The languages the highlighter knows, how to recognise them from a code
//! fence or a file name, and the word tables of the C-like ones.

/// A language the highlighter knows. Close relatives share one: `C` is the
/// C family (C, C++, C#, Java, Kotlin, Swift…), `JavaScript` covers TypeScript
/// and JSX, `Html` covers XML and SVG.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lang {
    Rust,
    JavaScript,
    Python,
    Go,
    C,
    Json,
    Toml,
    Yaml,
    Shell,
    PowerShell,
    Css,
    Html,
    Sql,
}

impl Lang {
    /// Every language, for tests and fuzzing.
    pub const ALL: [Lang; 13] = [
        Lang::Rust,
        Lang::JavaScript,
        Lang::Python,
        Lang::Go,
        Lang::C,
        Lang::Json,
        Lang::Toml,
        Lang::Yaml,
        Lang::Shell,
        Lang::PowerShell,
        Lang::Css,
        Lang::Html,
        Lang::Sql,
    ];

    /// From a code fence's info string: its first word, any case
    /// (`"rust"`, `"ts"`, `"py title=x"`, `"rust,ignore"`).
    pub fn from_fence(info: &str) -> Option<Lang> {
        let info = info.trim_start_matches(['`', '~', ' ', '\t', '.', '{']);
        let end = info.find(|c: char| c.is_whitespace() || matches!(c, ',' | '{' | '}' | '`')).unwrap_or(info.len());
        by_name(&info[..end].to_ascii_lowercase())
    }

    /// From a file path's name or extension (`main.rs`, `Dockerfile`, `Cargo.lock`).
    pub fn from_path(path: &str) -> Option<Lang> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
        match name.as_str() {
            "dockerfile" | "containerfile" | "makefile" | "gnumakefile" | "pkgbuild" | ".bashrc" | ".zshrc"
            | ".profile" | ".bash_profile" | ".env" | ".envrc" => return Some(Lang::Shell),
            "cargo.lock" | "poetry.lock" | "uv.lock" | "pipfile" => return Some(Lang::Toml),
            _ => {}
        }
        let (stem, ext) = name.rsplit_once('.')?;
        if stem.is_empty() && ext.len() > 4 {
            return None; // a dotfile like `.gitignore`, not an extension
        }
        match ext {
            "m" | "mm" | "hh" | "hxx" | "pyw" => Some(if ext == "pyw" { Lang::Python } else { Lang::C }),
            "csproj" | "fsproj" | "vbproj" | "props" | "targets" | "resx" | "config" | "manifest" | "nuspec" => {
                Some(Lang::Html)
            }
            "ndjson" | "geojson" | "jsonl" => Some(Lang::Json),
            "ps" => None, // PostScript, not PowerShell
            _ => by_name(ext),
        }
    }
}

fn by_name(name: &str) -> Option<Lang> {
    Some(match name {
        "rust" | "rs" => Lang::Rust,
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "node" => {
            Lang::JavaScript
        }
        "python" | "py" | "python3" | "py3" | "pyi" | "gyp" => Lang::Python,
        "go" | "golang" => Lang::Go,
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "cs" | "csharp" | "c#" | "java" | "kotlin" | "kt"
        | "kts" | "swift" | "objc" | "objective-c" | "dart" | "scala" | "groovy" | "gradle" => Lang::C,
        "json" | "jsonc" | "json5" | "jsonl" => Lang::Json,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "bash" | "sh" | "zsh" | "ksh" | "shell" | "console" | "shellsession" | "terminal" | "fish" | "dockerfile"
        | "docker" | "make" | "makefile" | "env" | "dotenv" | "bashrc" | "zshrc" => Lang::Shell,
        "powershell" | "ps1" | "pwsh" | "ps" | "psm1" | "psd1" => Lang::PowerShell,
        "css" | "scss" | "less" | "sass" => Lang::Css,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "xaml" | "vue" | "svelte" | "plist" | "xsd" | "xsl" => Lang::Html,
        "sql" | "psql" | "pgsql" | "mysql" | "sqlite" | "plsql" | "tsql" => Lang::Sql,
        _ => return None,
    })
}

/// How a C-like language is lexed: its words, comments and strings.
pub(super) struct Spec {
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Block comments nest (Rust).
    pub nested: bool,
    /// Bytes that open a string.
    pub quotes: &'static [u8],
    /// Of those, the ones whose strings may run over several lines.
    pub multiline: &'static [u8],
    /// Of those, the ones without escapes (Go's backtick).
    pub raw: &'static [u8],
    /// Of those, the ones that also come tripled (`"""`), always multi-line.
    pub triple: &'static [u8],
    /// The escape byte in strings, or 0.
    pub esc: u8,
    /// Capitalised words are types (all-caps ones constants).
    pub caps_types: bool,
    pub ignore_case: bool,
    /// `@name` is a decorator or annotation.
    pub decorators: bool,
    /// More bytes allowed in identifiers (`$` in JavaScript).
    pub ident_extra: u8,
}

impl Spec {
    pub fn is_keyword(&self, w: &str) -> bool {
        find(self.keywords, w, self.ignore_case)
    }

    pub fn is_type(&self, w: &str) -> bool {
        find(self.types, w, self.ignore_case)
    }
}

pub(super) fn find(list: &[&str], w: &str, ignore_case: bool) -> bool {
    list.iter().any(|k| k.len() == w.len() && if ignore_case { k.eq_ignore_ascii_case(w) } else { *k == w })
}

pub(super) fn spec(lang: Lang) -> &'static Spec {
    match lang {
        Lang::JavaScript => &JS,
        Lang::Python => &PYTHON,
        Lang::Go => &GO,
        Lang::C => &C,
        Lang::Sql => &SQL,
        _ => &RUST,
    }
}

static RUST: Spec = Spec {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
        "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "yield",
    ],
    types: &[
        "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64",
    ],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    nested: true,
    quotes: b"\"",
    multiline: b"\"",
    raw: b"",
    triple: b"",
    esc: b'\\',
    caps_types: true,
    ignore_case: false,
    decorators: false,
    ident_extra: 0,
};

static JS: Spec = Spec {
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "keyof",
        "let",
        "namespace",
        "new",
        "null",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "satisfies",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &["any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown"],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    nested: false,
    quotes: b"\"'`",
    multiline: b"`",
    raw: b"",
    triple: b"",
    esc: b'\\',
    caps_types: true,
    ignore_case: false,
    decorators: true,
    ident_extra: b'$',
};

static PYTHON: Spec = Spec {
    keywords: &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "cls", "continue", "def",
        "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
        "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while", "with", "yield",
    ],
    types: &[
        "bool",
        "bytearray",
        "bytes",
        "complex",
        "dict",
        "float",
        "frozenset",
        "int",
        "list",
        "object",
        "set",
        "str",
        "tuple",
    ],
    line_comments: &["#"],
    block_comment: None,
    nested: false,
    quotes: b"\"'",
    multiline: b"",
    raw: b"",
    triple: b"\"'",
    esc: b'\\',
    caps_types: true,
    ignore_case: false,
    decorators: true,
    ident_extra: 0,
};

static GO: Spec = Spec {
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "false",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "iota",
        "map",
        "nil",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "true",
        "type",
        "var",
    ],
    types: &[
        "any",
        "bool",
        "byte",
        "complex64",
        "complex128",
        "error",
        "float32",
        "float64",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "rune",
        "string",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
    ],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    nested: false,
    quotes: b"\"'`",
    multiline: b"`",
    raw: b"`",
    triple: b"",
    esc: b'\\',
    caps_types: true,
    ignore_case: false,
    decorators: false,
    ident_extra: 0,
};

static C: Spec = Spec {
    keywords: &[
        "abstract",
        "alignas",
        "alignof",
        "as",
        "asm",
        "assert",
        "associatedtype",
        "async",
        "await",
        "base",
        "break",
        "by",
        "case",
        "catch",
        "checked",
        "class",
        "co_await",
        "co_return",
        "co_yield",
        "companion",
        "concept",
        "const",
        "const_cast",
        "constexpr",
        "continue",
        "data",
        "decltype",
        "default",
        "defer",
        "deinit",
        "delegate",
        "delete",
        "do",
        "dynamic_cast",
        "else",
        "enum",
        "event",
        "explicit",
        "export",
        "extends",
        "extension",
        "extern",
        "false",
        "fileprivate",
        "final",
        "finally",
        "fixed",
        "for",
        "foreach",
        "friend",
        "fun",
        "func",
        "get",
        "goto",
        "guard",
        "if",
        "implements",
        "implicit",
        "import",
        "in",
        "init",
        "inline",
        "inout",
        "instanceof",
        "interface",
        "internal",
        "is",
        "lateinit",
        "let",
        "lock",
        "mutable",
        "namespace",
        "native",
        "new",
        "nil",
        "noexcept",
        "null",
        "nullptr",
        "object",
        "open",
        "operator",
        "out",
        "override",
        "package",
        "params",
        "permits",
        "private",
        "protected",
        "protocol",
        "public",
        "readonly",
        "record",
        "ref",
        "register",
        "reinterpret_cast",
        "repeat",
        "requires",
        "restrict",
        "return",
        "rethrows",
        "sealed",
        "self",
        "Self",
        "set",
        "sizeof",
        "some",
        "stackalloc",
        "static",
        "static_assert",
        "static_cast",
        "struct",
        "subscript",
        "super",
        "suspend",
        "switch",
        "synchronized",
        "template",
        "this",
        "throw",
        "throws",
        "transient",
        "true",
        "try",
        "typealias",
        "typedef",
        "typeid",
        "typename",
        "union",
        "unsafe",
        "using",
        "val",
        "var",
        "virtual",
        "volatile",
        "when",
        "where",
        "while",
        "yield",
    ],
    types: &[
        "auto", "bool", "boolean", "byte", "char", "decimal", "double", "dynamic", "float", "int", "int8_t", "int16_t",
        "int32_t", "int64_t", "long", "nint", "nuint", "sbyte", "short", "signed", "size_t", "string", "uint",
        "uint8_t", "uint16_t", "uint32_t", "uint64_t", "ulong", "unsigned", "ushort", "void", "wchar_t",
    ],
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    nested: false,
    quotes: b"\"'",
    multiline: b"",
    raw: b"",
    triple: b"\"",
    esc: b'\\',
    caps_types: true,
    ignore_case: false,
    decorators: true,
    ident_extra: 0,
};

static SQL: Spec = Spec {
    keywords: &[
        "add",
        "all",
        "alter",
        "and",
        "as",
        "asc",
        "begin",
        "between",
        "by",
        "cascade",
        "case",
        "check",
        "column",
        "commit",
        "constraint",
        "create",
        "cross",
        "database",
        "declare",
        "default",
        "delete",
        "desc",
        "distinct",
        "drop",
        "else",
        "end",
        "exists",
        "false",
        "foreign",
        "from",
        "full",
        "function",
        "grant",
        "group",
        "having",
        "if",
        "in",
        "index",
        "inner",
        "insert",
        "into",
        "is",
        "join",
        "key",
        "left",
        "like",
        "limit",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "primary",
        "procedure",
        "references",
        "replace",
        "return",
        "returning",
        "returns",
        "revoke",
        "right",
        "rollback",
        "schema",
        "select",
        "set",
        "table",
        "then",
        "transaction",
        "trigger",
        "true",
        "union",
        "unique",
        "update",
        "using",
        "values",
        "view",
        "when",
        "where",
        "with",
    ],
    types: &[
        "bigint",
        "bigserial",
        "blob",
        "bool",
        "boolean",
        "bytea",
        "char",
        "date",
        "decimal",
        "double",
        "float",
        "int",
        "integer",
        "interval",
        "json",
        "jsonb",
        "numeric",
        "precision",
        "real",
        "serial",
        "smallint",
        "text",
        "time",
        "timestamp",
        "timestamptz",
        "tinyint",
        "uuid",
        "varchar",
    ],
    line_comments: &["--"],
    block_comment: Some(("/*", "*/")),
    nested: false,
    quotes: b"'\"`",
    multiline: b"'",
    raw: b"",
    triple: b"",
    esc: 0,
    caps_types: false,
    ignore_case: true,
    decorators: false,
    ident_extra: 0,
};

/// Shell words that are keywords.
pub(super) const SHELL_KEYWORDS: &[&str] = &[
    "break", "case", "continue", "declare", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for",
    "function", "if", "in", "local", "readonly", "return", "select", "then", "unset", "until", "while",
];

/// PowerShell keywords, any case.
pub(super) const PS_KEYWORDS: &[&str] = &[
    "begin",
    "break",
    "catch",
    "class",
    "continue",
    "data",
    "do",
    "dynamicparam",
    "else",
    "elseif",
    "end",
    "enum",
    "exit",
    "filter",
    "finally",
    "for",
    "foreach",
    "function",
    "if",
    "in",
    "param",
    "process",
    "return",
    "switch",
    "throw",
    "trap",
    "try",
    "until",
    "using",
    "while",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences() {
        assert_eq!(Lang::from_fence("rust"), Some(Lang::Rust));
        assert_eq!(Lang::from_fence("TS"), Some(Lang::JavaScript));
        assert_eq!(Lang::from_fence("py title=x"), Some(Lang::Python));
        assert_eq!(Lang::from_fence("rust,ignore"), Some(Lang::Rust));
        assert_eq!(Lang::from_fence("```pwsh"), Some(Lang::PowerShell));
        assert_eq!(Lang::from_fence("console"), Some(Lang::Shell));
        assert_eq!(Lang::from_fence("kotlin"), Some(Lang::C));
        assert_eq!(Lang::from_fence("svg"), Some(Lang::Html));
        assert_eq!(Lang::from_fence(""), None);
        assert_eq!(Lang::from_fence("text"), None);
        assert_eq!(Lang::from_fence("é"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(Lang::from_path("src/main.rs"), Some(Lang::Rust));
        assert_eq!(Lang::from_path("C:\\x\\App.TSX"), Some(Lang::JavaScript));
        assert_eq!(Lang::from_path("scripts/check.ps1"), Some(Lang::PowerShell));
        assert_eq!(Lang::from_path("Cargo.lock"), Some(Lang::Toml));
        assert_eq!(Lang::from_path("Cargo.toml"), Some(Lang::Toml));
        assert_eq!(Lang::from_path("a/Dockerfile"), Some(Lang::Shell));
        assert_eq!(Lang::from_path("kynex.csproj"), Some(Lang::Html));
        assert_eq!(Lang::from_path("yarn.lock"), None);
        assert_eq!(Lang::from_path(".gitignore"), None);
        assert_eq!(Lang::from_path("README"), None);
        assert_eq!(Lang::from_path(""), None);
    }
}
