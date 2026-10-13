//! A small hand-written syntax highlighter.
//!
//! It is deliberately simple: each line is scanned left to right with a tiny state carried from the end
//! of one line to the start of the next (open block comments, multi-line strings, Markdown fences), so
//! editing a line only forces re-scanning until the state stops changing. It does not parse: it is
//! right for the everyday shapes of code and degrades to plain text, never to a wrong crash.
//!
//! Spans use *character* columns, are sorted and never overlap; text without a span is plain.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    Macro,
    String,
    Number,
    Constant,
    Comment,
    Attribute,
    Variable,
    Heading,
    Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Plain,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    C,
    Cpp,
    Java,
    Go,
    Shell,
    Json,
    Toml,
    Yaml,
    Markdown,
}

impl Lang {
    /// What starts a line comment in this language (`None` when it has none).
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Rust | Lang::JavaScript | Lang::TypeScript | Lang::C | Lang::Cpp | Lang::Java | Lang::Go => Some("//"),
            Lang::Python | Lang::Shell | Lang::Toml | Lang::Yaml => Some("#"),
            Lang::Plain | Lang::Json | Lang::Markdown => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Plain => "Plain text",
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::C => "C",
            Lang::Cpp => "C++",
            Lang::Java => "Java",
            Lang::Go => "Go",
            Lang::Shell => "Shell",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Markdown => "Markdown",
        }
    }
}

pub fn lang_for_path(path: &str) -> Lang {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "makefile" | "dockerfile" | ".bashrc" | ".profile" | ".zshrc" => return if lower == "makefile" || lower == "dockerfile" { Lang::Shell } else { Lang::Shell },
        "cargo.lock" => return Lang::Toml,
        _ => {}
    }
    match lower.rsplit_once('.').map(|(_, e)| e) {
        Some("rs") => Lang::Rust,
        Some("py" | "pyw") => Lang::Python,
        Some("js" | "mjs" | "cjs" | "jsx") => Lang::JavaScript,
        Some("ts" | "tsx") => Lang::TypeScript,
        Some("c" | "h") => Lang::C,
        Some("cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx") => Lang::Cpp,
        Some("java") => Lang::Java,
        Some("go") => Lang::Go,
        Some("sh" | "bash" | "zsh" | "conf" | "env") => Lang::Shell,
        Some("json" | "jsonc") => Lang::Json,
        Some("toml") => Lang::Toml,
        Some("yml" | "yaml") => Lang::Yaml,
        Some("md" | "markdown") => Lang::Markdown,
        _ => Lang::Plain,
    }
}

/// What carries over from the end of a line to the start of the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HlState {
    #[default]
    Normal,
    /// Inside a block comment (nesting depth, for languages that nest them).
    Block(u8),
    /// Inside a double-quoted string that continues on the next line (Rust).
    Str,
    /// Inside a triple-quoted string (Python / TOML); the quote character.
    Triple(char),
    /// Inside a Rust raw string with this many `#`.
    Raw(u8),
    /// Inside a backtick string (JavaScript template, Go raw string).
    Backtick,
    /// Inside a Markdown fenced code block.
    Fence,
}

const MAX_LINE: usize = 3000;

struct Def {
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    line_comment: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    nested: bool,
    quotes: &'static [char],
    triple: bool,
    backtick: bool,
    rust: bool,
    directive: bool,
    at_attr: bool,
    dollar: bool,
    shell: bool,
    fn_kw: &'static [&'static str],
    type_kw: &'static [&'static str],
}

const NONE: &[&str] = &[];

const RUST: Def = Def {
    keywords: &["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while"],
    types: &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: true,
    quotes: &['"'],
    triple: false,
    backtick: false,
    rust: true,
    directive: false,
    at_attr: false,
    dollar: false,
    shell: false,
    fn_kw: &["fn"],
    type_kw: &["struct", "enum", "trait", "impl", "type", "mod"],
};

const PYTHON: Def = Def {
    keywords: &["and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "cls", "True", "try", "while", "with", "yield"],
    types: &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object"],
    line_comment: &["#"],
    block: None,
    nested: false,
    quotes: &['"', '\''],
    triple: true,
    backtick: false,
    rust: false,
    directive: false,
    at_attr: true,
    dollar: false,
    shell: false,
    fn_kw: &["def"],
    type_kw: &["class"],
};

const JS: Def = Def {
    keywords: &["async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "false", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "with", "yield", "interface", "type", "enum", "implements", "readonly", "public", "private", "protected", "abstract", "as", "namespace", "declare"],
    types: &["string", "number", "boolean", "any", "unknown", "never", "object", "Promise", "Array", "Map", "Set"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: true,
    rust: false,
    directive: false,
    at_attr: true,
    dollar: false,
    shell: false,
    fn_kw: &["function"],
    type_kw: &["class", "interface", "type", "enum", "namespace"],
};

const C: Def = Def {
    keywords: &["auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while", "NULL", "true", "false"],
    types: &["int", "char", "short", "long", "float", "double", "void", "unsigned", "signed", "size_t", "bool", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: false,
    rust: false,
    directive: true,
    at_attr: false,
    dollar: false,
    shell: false,
    fn_kw: NONE,
    type_kw: &["struct", "enum", "union"],
};

const CPP: Def = Def {
    keywords: &["auto", "break", "case", "catch", "class", "const", "constexpr", "continue", "default", "delete", "do", "else", "enum", "explicit", "extern", "false", "final", "for", "friend", "goto", "if", "inline", "namespace", "new", "noexcept", "nullptr", "operator", "override", "private", "protected", "public", "return", "sizeof", "static", "struct", "switch", "template", "this", "throw", "true", "try", "typedef", "typename", "union", "using", "virtual", "volatile", "while"],
    types: &["int", "char", "short", "long", "float", "double", "void", "unsigned", "signed", "bool", "size_t", "string", "vector", "map", "set", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: false,
    rust: false,
    directive: true,
    at_attr: false,
    dollar: false,
    shell: false,
    fn_kw: NONE,
    type_kw: &["struct", "enum", "union", "class", "namespace"],
};

const JAVA: Def = Def {
    keywords: &["abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends", "final", "finally", "for", "if", "implements", "import", "instanceof", "interface", "native", "new", "null", "package", "private", "protected", "public", "return", "static", "super", "switch", "synchronized", "this", "throw", "throws", "try", "var", "void", "volatile", "while", "true", "false"],
    types: &["int", "long", "short", "byte", "char", "float", "double", "boolean", "String", "Object", "List", "Map"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: false,
    rust: false,
    directive: false,
    at_attr: true,
    dollar: false,
    shell: false,
    fn_kw: NONE,
    type_kw: &["class", "interface", "enum"],
};

const GO: Def = Def {
    keywords: &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "nil", "true", "false"],
    types: &["int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "float32", "float64", "string", "bool", "byte", "rune", "error", "any"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: true,
    rust: false,
    directive: false,
    at_attr: false,
    dollar: false,
    shell: false,
    fn_kw: &["func"],
    type_kw: &["type", "struct", "interface"],
};

const SHELL: Def = Def {
    keywords: &["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "select", "return", "exit", "export", "local", "readonly", "set", "unset", "source", "alias", "true", "false"],
    types: NONE,
    line_comment: &["#"],
    block: None,
    nested: false,
    quotes: &['"', '\''],
    triple: false,
    backtick: false,
    rust: false,
    directive: false,
    at_attr: false,
    dollar: true,
    shell: true,
    fn_kw: &["function"],
    type_kw: NONE,
};

fn def_for(lang: Lang) -> Option<&'static Def> {
    Some(match lang {
        Lang::Rust => &RUST,
        Lang::Python => &PYTHON,
        Lang::JavaScript | Lang::TypeScript => &JS,
        Lang::C => &C,
        Lang::Cpp => &CPP,
        Lang::Java => &JAVA,
        Lang::Go => &GO,
        Lang::Shell => &SHELL,
        _ => return None,
    })
}

fn starts(cs: &[char], i: usize, pat: &str) -> bool {
    let mut k = i;
    for p in pat.chars() {
        if cs.get(k) != Some(&p) {
            return false;
        }
        k += 1;
    }
    true
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Out(Vec<Span>);
impl Out {
    fn add(&mut self, start: usize, end: usize, kind: TokenKind) {
        if end > start {
            self.0.push(Span { start, end, kind });
        }
    }
}

/// End (exclusive) of a quoted string starting at `i` (cs[i] is the opening quote); `closed` if it ended.
fn scan_quoted(cs: &[char], i: usize, q: char, escapes: bool) -> (usize, bool) {
    let mut k = i + 1;
    while k < cs.len() {
        if escapes && cs[k] == '\\' {
            k += 2;
            continue;
        }
        if cs[k] == q {
            return (k + 1, true);
        }
        k += 1;
    }
    (cs.len(), false)
}

/// Continue a string from the start of a line (no opening quote on this line).
fn scan_str_continue(cs: &[char], q: char) -> (usize, bool) {
    let mut k = 0;
    while k < cs.len() {
        if cs[k] == '\\' {
            k += 2;
            continue;
        }
        if cs[k] == q {
            return (k + 1, true);
        }
        k += 1;
    }
    (cs.len(), false)
}

fn scan_number(cs: &[char], i: usize) -> usize {
    let mut k = i;
    let hex = starts(cs, i, "0x") || starts(cs, i, "0X");
    while k < cs.len() {
        let c = cs[k];
        if c.is_ascii_alphanumeric() || c == '_' {
            k += 1;
        } else if c == '.' && cs.get(k + 1).is_some_and(|d| d.is_ascii_digit()) {
            k += 1;
        } else if (c == '+' || c == '-') && !hex && k > i && matches!(cs[k - 1], 'e' | 'E') && cs.get(k + 1).is_some_and(|d| d.is_ascii_digit()) {
            k += 1;
        } else {
            break;
        }
    }
    k
}

/// Close of a block comment starting the scan at `from`; returns (end, closed, depth).
fn scan_block(cs: &[char], from: usize, open: &str, close: &str, nested: bool, mut depth: u8) -> (usize, bool, u8) {
    let mut k = from;
    while k < cs.len() {
        if nested && starts(cs, k, open) {
            depth = depth.saturating_add(1);
            k += open.chars().count();
        } else if starts(cs, k, close) {
            k += close.chars().count();
            if depth <= 1 {
                return (k, true, 0);
            }
            depth -= 1;
        } else {
            k += 1;
        }
    }
    (cs.len(), false, depth)
}

/// Closing `"` followed by `hashes` `#`s, from `from`.
fn scan_raw(cs: &[char], from: usize, hashes: usize) -> Option<usize> {
    let mut k = from;
    while k < cs.len() {
        if cs[k] == '"' && (0..hashes).all(|h| cs.get(k + 1 + h) == Some(&'#')) {
            return Some(k + 1 + hashes);
        }
        k += 1;
    }
    None
}

fn scan_backtick(cs: &[char], from: usize, escapes: bool) -> Option<usize> {
    let mut k = from;
    while k < cs.len() {
        if escapes && cs[k] == '\\' {
            k += 2;
            continue;
        }
        if cs[k] == '`' {
            return Some(k + 1);
        }
        k += 1;
    }
    None
}

fn scan_code(def: &Def, cs: &[char], state: HlState) -> (Vec<Span>, HlState) {
    let n = cs.len();
    let mut out = Out(Vec::new());
    let mut i = 0;
    let block = def.block.unwrap_or(("", ""));
    // resume whatever the previous line left open
    match state {
        HlState::Block(d) => {
            let (e, closed, d2) = scan_block(cs, 0, block.0, block.1, def.nested, d.max(1));
            out.add(0, e, TokenKind::Comment);
            if !closed {
                return (out.0, HlState::Block(d2));
            }
            i = e;
        }
        HlState::Str => {
            let (e, closed) = scan_str_continue(cs, '"');
            out.add(0, e, TokenKind::String);
            if !closed {
                return (out.0, HlState::Str);
            }
            i = e;
        }
        HlState::Triple(q) => {
            let mut k = 0;
            let mut end = None;
            while k < n {
                if cs[k] == '\\' {
                    k += 2;
                    continue;
                }
                if cs[k] == q && cs.get(k + 1) == Some(&q) && cs.get(k + 2) == Some(&q) {
                    end = Some(k + 3);
                    break;
                }
                k += 1;
            }
            out.add(0, end.unwrap_or(n), TokenKind::String);
            match end {
                Some(e) => i = e,
                None => return (out.0, HlState::Triple(q)),
            }
        }
        HlState::Raw(h) => match scan_raw(cs, 0, h as usize) {
            Some(e) => {
                out.add(0, e, TokenKind::String);
                i = e;
            }
            None => {
                out.add(0, n, TokenKind::String);
                return (out.0, HlState::Raw(h));
            }
        },
        HlState::Backtick => match scan_backtick(cs, 0, def.backtick && !def.rust && def.keywords.contains(&"function")) {
            Some(e) => {
                out.add(0, e, TokenKind::String);
                i = e;
            }
            None => {
                out.add(0, n, TokenKind::String);
                return (out.0, HlState::Backtick);
            }
        },
        _ => {}
    }
    let mut pending: Option<TokenKind> = None; // the kind the next identifier takes (after `fn`, `class`, …)
    let first_nonspace = cs.iter().position(|c| !c.is_whitespace()).unwrap_or(n);
    while i < n {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if let Some(lc) = def.line_comment.iter().find(|lc| starts(cs, i, lc)) {
            let shell_ok = !def.shell || i == 0 || cs[i - 1].is_whitespace() || matches!(cs[i - 1], ';' | '(' | '{');
            let dollar_hash = *lc == "#" && def.shell && i > 0 && cs[i - 1] == '$';
            if shell_ok && !dollar_hash && !(def.rust && starts(cs, i, "#[")) {
                out.add(i, n, TokenKind::Comment);
                return (out.0, HlState::Normal);
            }
        }
        if def.block.is_some() && starts(cs, i, block.0) {
            let (e, closed, d) = scan_block(cs, i + block.0.chars().count(), block.0, block.1, def.nested, 1);
            out.add(i, e, TokenKind::Comment);
            if !closed {
                return (out.0, HlState::Block(d.max(1)));
            }
            i = e;
            pending = None;
            continue;
        }
        // C preprocessor line
        if def.directive && c == '#' && i == first_nonspace {
            let mut k = i + 1;
            while k < n && cs[k].is_whitespace() {
                k += 1;
            }
            while k < n && is_ident(cs[k]) {
                k += 1;
            }
            out.add(i, k, TokenKind::Attribute);
            i = k;
            continue;
        }
        // Rust attributes
        if def.rust && c == '#' && (starts(cs, i, "#[") || starts(cs, i, "#![")) {
            let mut depth = 0;
            let mut k = i;
            while k < n {
                match cs[k] {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            k += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            out.add(i, k.min(n), TokenKind::Attribute);
            i = k.min(n);
            continue;
        }
        // decorators / annotations
        if def.at_attr && c == '@' && cs.get(i + 1).is_some_and(|d| is_ident_start(*d)) && (i == first_nonspace || !def.keywords.contains(&"def")) {
            let mut k = i + 1;
            while k < n && (is_ident(cs[k]) || cs[k] == '.') {
                k += 1;
            }
            out.add(i, k, TokenKind::Attribute);
            i = k;
            continue;
        }
        // shell variables
        if def.dollar && c == '$' {
            let mut k = i + 1;
            if cs.get(k) == Some(&'{') {
                while k < n && cs[k] != '}' {
                    k += 1;
                }
                k = (k + 1).min(n);
            } else if cs.get(k) == Some(&'(') {
                k += 1; // $( … ): colour just the opener, the command inside is scanned normally
            } else if cs.get(k).is_some_and(|d| is_ident(*d)) {
                while k < n && is_ident(cs[k]) {
                    k += 1;
                }
            } else if cs.get(k).is_some() {
                k += 1;
            }
            out.add(i, k, TokenKind::Variable);
            i = k;
            continue;
        }
        // Rust lifetimes and character literals
        if def.rust && c == '\'' {
            if cs.get(i + 1) == Some(&'\\') {
                let (e, _) = scan_quoted(cs, i, '\'', true);
                out.add(i, e, TokenKind::String);
                i = e;
            } else if cs.get(i + 2) == Some(&'\'') {
                out.add(i, i + 3, TokenKind::String);
                i += 3;
            } else if cs.get(i + 1).is_some_and(|d| is_ident_start(*d)) {
                let mut k = i + 1;
                while k < n && is_ident(cs[k]) {
                    k += 1;
                }
                out.add(i, k, TokenKind::Type);
                i = k;
            } else {
                i += 1;
            }
            pending = None;
            continue;
        }
        // strings
        if def.quotes.contains(&c) {
            pending = None;
            if def.triple && cs.get(i + 1) == Some(&c) && cs.get(i + 2) == Some(&c) {
                let mut k = i + 3;
                let mut end = None;
                while k < n {
                    if cs[k] == '\\' {
                        k += 2;
                        continue;
                    }
                    if cs[k] == c && cs.get(k + 1) == Some(&c) && cs.get(k + 2) == Some(&c) {
                        end = Some(k + 3);
                        break;
                    }
                    k += 1;
                }
                out.add(i, end.unwrap_or(n), TokenKind::String);
                match end {
                    Some(e) => {
                        i = e;
                        continue;
                    }
                    None => return (out.0, HlState::Triple(c)),
                }
            }
            let escapes = !(def.shell && c == '\'');
            let (e, closed) = scan_quoted(cs, i, c, escapes);
            out.add(i, e, TokenKind::String);
            if !closed && def.rust && c == '"' {
                return (out.0, HlState::Str);
            }
            i = e;
            continue;
        }
        if c == '`' && def.backtick {
            pending = None;
            let js = def.keywords.contains(&"function");
            match scan_backtick(cs, i + 1, js) {
                Some(e) => {
                    out.add(i, e, TokenKind::String);
                    i = e;
                }
                None => {
                    out.add(i, n, TokenKind::String);
                    return (out.0, HlState::Backtick);
                }
            }
            continue;
        }
        // numbers
        if c.is_ascii_digit() {
            let e = scan_number(cs, i);
            out.add(i, e, TokenKind::Number);
            i = e;
            pending = None;
            continue;
        }
        // identifiers, keywords, prefixed strings
        if is_ident_start(c) {
            let mut j = i;
            while j < n && is_ident(cs[j]) {
                j += 1;
            }
            let word: String = cs[i..j].iter().collect();
            // string prefixes: Rust r"…" r#"…"# b"…" br#"…"#, Python f"…" rb'…'
            let quote_next = cs.get(j).is_some_and(|q| def.quotes.contains(q));
            if def.rust && (word == "r" || word == "br") && matches!(cs.get(j), Some('"' | '#')) {
                let mut h = 0;
                while cs.get(j + h) == Some(&'#') {
                    h += 1;
                }
                if cs.get(j + h) == Some(&'"') {
                    match scan_raw(cs, j + h + 1, h) {
                        Some(e) => {
                            out.add(i, e, TokenKind::String);
                            i = e;
                        }
                        None => {
                            out.add(i, n, TokenKind::String);
                            return (out.0, HlState::Raw(h.min(255) as u8));
                        }
                    }
                    continue;
                }
            }
            if quote_next && (word.len() <= 2) && word.chars().all(|ch| matches!(ch.to_ascii_lowercase(), 'r' | 'b' | 'f' | 'u')) && (def.triple || def.rust) {
                let q = cs[j];
                if def.triple && cs.get(j + 1) == Some(&q) && cs.get(j + 2) == Some(&q) {
                    // prefixed triple-quoted string: let the quote branch handle it, colouring the prefix as part of it
                    let mut k = j + 3;
                    let mut end = None;
                    while k < n {
                        if cs[k] == '\\' {
                            k += 2;
                            continue;
                        }
                        if cs[k] == q && cs.get(k + 1) == Some(&q) && cs.get(k + 2) == Some(&q) {
                            end = Some(k + 3);
                            break;
                        }
                        k += 1;
                    }
                    out.add(i, end.unwrap_or(n), TokenKind::String);
                    match end {
                        Some(e) => {
                            i = e;
                            continue;
                        }
                        None => return (out.0, HlState::Triple(q)),
                    }
                }
                let (e, _) = scan_quoted(cs, j, q, !word.contains(['r', 'R']));
                out.add(i, e, TokenKind::String);
                i = e;
                continue;
            }
            let next = cs[j..].iter().find(|ch| !ch.is_whitespace()).copied();
            let kind = if let Some(k) = pending.take() {
                Some(k)
            } else if def.keywords.contains(&word.as_str()) {
                None // handled below so `fn` / `class` can set `pending`
            } else if def.types.contains(&word.as_str()) {
                Some(TokenKind::Type)
            } else if def.rust && cs.get(j) == Some(&'!') && cs.get(j + 1) != Some(&'=') {
                out.add(i, j + 1, TokenKind::Macro);
                i = j + 1;
                continue;
            } else if next == Some('(') {
                Some(TokenKind::Function)
            } else if word.chars().next().is_some_and(|ch| ch.is_uppercase()) {
                if word.len() >= 2 && word.chars().all(|ch| ch.is_uppercase() || ch.is_ascii_digit() || ch == '_') { Some(TokenKind::Constant) } else { Some(TokenKind::Type) }
            } else {
                None
            };
            match kind {
                Some(k) => out.add(i, j, k),
                None if def.keywords.contains(&word.as_str()) => {
                    out.add(i, j, TokenKind::Keyword);
                    if def.fn_kw.contains(&word.as_str()) {
                        pending = Some(TokenKind::Function);
                    } else if def.type_kw.contains(&word.as_str()) {
                        pending = Some(TokenKind::Type);
                    }
                    i = j;
                    continue;
                }
                None => {}
            }
            i = j;
            continue;
        }
        pending = None;
        i += 1;
    }
    (out.0, HlState::Normal)
}

fn scan_json(cs: &[char]) -> Vec<Span> {
    let n = cs.len();
    let mut out = Out(Vec::new());
    let mut i = 0;
    while i < n {
        let c = cs[i];
        if c == '"' {
            let (e, _) = scan_quoted(cs, i, '"', true);
            let after = cs[e..].iter().find(|ch| !ch.is_whitespace());
            out.add(i, e, if after == Some(&':') { TokenKind::Key } else { TokenKind::String });
            i = e;
        } else if c.is_ascii_digit() || (c == '-' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let e = scan_number(cs, i + 1);
            out.add(i, e, TokenKind::Number);
            i = e;
        } else if is_ident_start(c) {
            let mut j = i;
            while j < n && is_ident(cs[j]) {
                j += 1;
            }
            let w: String = cs[i..j].iter().collect();
            if matches!(w.as_str(), "true" | "false" | "null") {
                out.add(i, j, TokenKind::Keyword);
            }
            i = j;
        } else if starts(cs, i, "//") {
            out.add(i, n, TokenKind::Comment);
            break;
        } else {
            i += 1;
        }
    }
    out.0
}

/// TOML and YAML share most of their surface: comments, keys, strings, numbers, booleans.
fn scan_config(cs: &[char], toml: bool, state: HlState) -> (Vec<Span>, HlState) {
    let n = cs.len();
    let mut out = Out(Vec::new());
    let mut i = 0;
    if let HlState::Triple(q) = state {
        let mut k = 0;
        let mut end = None;
        while k + 2 < n + 2 && k < n {
            if cs[k] == q && cs.get(k + 1) == Some(&q) && cs.get(k + 2) == Some(&q) {
                end = Some(k + 3);
                break;
            }
            k += 1;
        }
        out.add(0, end.unwrap_or(n), TokenKind::String);
        match end {
            Some(e) => i = e,
            None => return (out.0, HlState::Triple(q)),
        }
    }
    let first = cs.iter().position(|c| !c.is_whitespace()).unwrap_or(n);
    if i == 0 && first < n {
        let t: String = cs[first..].iter().collect();
        if toml && t.starts_with('[') {
            let e = cs.iter().position(|c| *c == '#').unwrap_or(n);
            out.add(first, cs[..e].iter().rposition(|c| !c.is_whitespace()).map_or(first, |p| p + 1), TokenKind::Heading);
            i = e;
        } else if !toml && (t.starts_with("---") || t.starts_with("...")) {
            out.add(first, n, TokenKind::Heading);
            return (out.0, HlState::Normal);
        } else {
            // `key =` / `key:` (optionally after a YAML list dash)
            let mut k = first;
            if !toml && cs[k] == '-' && cs.get(k + 1) == Some(&' ') {
                k += 2;
                while k < n && cs[k] == ' ' {
                    k += 1;
                }
            }
            let ks = k;
            if k < n && (cs[k] == '"' || cs[k] == '\'') {
                let (e, closed) = scan_quoted(cs, k, cs[k], true);
                k = if closed { e } else { n };
            } else {
                while k < n && !matches!(cs[k], '=' | ':' | '#') && !(cs[k] == ' ' && cs[k..].iter().find(|c| **c != ' ') .is_some_and(|c| matches!(c, '=' | ':'))) {
                    k += 1;
                }
            }
            let mut ke = k;
            while ke > ks && cs[ke - 1] == ' ' {
                ke -= 1;
            }
            let mut after = k;
            while after < n && cs[after] == ' ' {
                after += 1;
            }
            let sep = cs.get(after).copied();
            let is_key = ke > ks && ((toml && sep == Some('=')) || (!toml && sep == Some(':') && cs.get(after + 1).is_none_or(|c| *c == ' ')));
            if is_key {
                out.add(ks, ke, TokenKind::Key);
                i = after + 1;
            }
        }
    }
    while i < n {
        let c = cs[i];
        if c == '#' && (i == 0 || cs[i - 1].is_whitespace() || toml) {
            out.add(i, n, TokenKind::Comment);
            break;
        }
        if c == '"' || c == '\'' {
            if toml && cs.get(i + 1) == Some(&c) && cs.get(i + 2) == Some(&c) {
                let mut k = i + 3;
                let mut end = None;
                while k < n {
                    if cs[k] == c && cs.get(k + 1) == Some(&c) && cs.get(k + 2) == Some(&c) {
                        end = Some(k + 3);
                        break;
                    }
                    k += 1;
                }
                out.add(i, end.unwrap_or(n), TokenKind::String);
                match end {
                    Some(e) => {
                        i = e;
                        continue;
                    }
                    None => return (out.0, HlState::Triple(c)),
                }
            }
            let (e, _) = scan_quoted(cs, i, c, c == '"');
            out.add(i, e, TokenKind::String);
            i = e;
        } else if c.is_ascii_digit() || (matches!(c, '-' | '+') && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && (i == 0 || !is_ident(cs[i - 1]))) {
            if i > 0 && is_ident(cs[i - 1]) {
                i += 1;
                continue;
            }
            let e = scan_number(cs, i + 1);
            out.add(i, e, TokenKind::Number);
            i = e;
        } else if is_ident_start(c) {
            let mut j = i;
            while j < n && (is_ident(cs[j]) || cs[j] == '-' || cs[j] == '.') {
                j += 1;
            }
            let w: String = cs[i..j].iter().collect();
            let kw: &[&str] = if toml { &["true", "false", "inf", "nan"] } else { &["true", "false", "null", "yes", "no", "on", "off", "True", "False", "Null", "~"] };
            if kw.contains(&w.as_str()) {
                out.add(i, j, TokenKind::Keyword);
            }
            i = j;
        } else if !toml && (c == '&' || c == '*') && cs.get(i + 1).is_some_and(|d| is_ident_start(*d)) {
            let mut j = i + 1;
            while j < n && is_ident(cs[j]) {
                j += 1;
            }
            out.add(i, j, TokenKind::Variable);
            i = j;
        } else {
            i += 1;
        }
    }
    (out.0, HlState::Normal)
}

fn scan_markdown(cs: &[char], state: HlState) -> (Vec<Span>, HlState) {
    let n = cs.len();
    let mut out = Out(Vec::new());
    let first = cs.iter().position(|c| !c.is_whitespace()).unwrap_or(n);
    let fence = first < n && (starts(cs, first, "```") || starts(cs, first, "~~~"));
    if fence {
        out.add(0, n, TokenKind::Attribute);
        return (out.0, if state == HlState::Fence { HlState::Normal } else { HlState::Fence });
    }
    if state == HlState::Fence {
        out.add(0, n, TokenKind::String);
        return (out.0, HlState::Fence);
    }
    if first < n && cs[first] == '#' {
        let hashes = cs[first..].iter().take_while(|c| **c == '#').count();
        if hashes <= 6 && cs.get(first + hashes).is_none_or(|c| *c == ' ') {
            out.add(first, n, TokenKind::Heading);
            return (out.0, HlState::Normal);
        }
    }
    if first < n && cs[first] == '>' {
        out.add(first, n, TokenKind::Comment);
        return (out.0, HlState::Normal);
    }
    let mut i = 0;
    // list bullets and numbers
    if first < n {
        let bullet = matches!(cs[first], '-' | '*' | '+') && cs.get(first + 1) == Some(&' ');
        let digits = cs[first..].iter().take_while(|c| c.is_ascii_digit()).count();
        if bullet {
            out.add(first, first + 1, TokenKind::Attribute);
            i = first + 1;
        } else if digits > 0 && matches!(cs.get(first + digits), Some('.' | ')')) && cs.get(first + digits + 1) == Some(&' ') {
            out.add(first, first + digits + 1, TokenKind::Attribute);
            i = first + digits + 1;
        }
    }
    while i < n {
        let c = cs[i];
        if c == '`' {
            let close = cs[i + 1..].iter().position(|d| *d == '`');
            match close {
                Some(p) => {
                    out.add(i, i + p + 2, TokenKind::String);
                    i += p + 2;
                }
                None => i += 1,
            }
        } else if (c == '*' || c == '_') && cs.get(i + 1) == Some(&c) {
            let pat: String = [c, c].iter().collect();
            let close = (i + 2..n).find(|k| starts(cs, *k, &pat));
            match close {
                Some(e) if e > i + 2 => {
                    out.add(i, e + 2, TokenKind::Keyword);
                    i = e + 2;
                }
                _ => i += 2,
            }
        } else if c == '[' {
            // [text](url)
            let close = cs[i + 1..].iter().position(|d| *d == ']').map(|p| i + 1 + p);
            match close {
                Some(e) if cs.get(e + 1) == Some(&'(') => {
                    let url_end = cs[e + 2..].iter().position(|d| *d == ')').map(|p| e + 2 + p + 1);
                    match url_end {
                        Some(u) => {
                            out.add(i, e + 1, TokenKind::Function);
                            out.add(e + 1, u, TokenKind::Comment);
                            i = u;
                        }
                        None => i += 1,
                    }
                }
                _ => i += 1,
            }
        } else {
            i += 1;
        }
    }
    (out.0, HlState::Normal)
}

/// Highlight one line. `state` is what the previous line returned (`HlState::Normal` for the first line).
pub fn highlight_line(lang: Lang, line: &str, state: HlState) -> (Vec<Span>, HlState) {
    let cs: Vec<char> = line.chars().collect();
    if cs.len() > MAX_LINE {
        return (vec![], state);
    }
    if let Some(def) = def_for(lang) {
        return scan_code(def, &cs, state);
    }
    match lang {
        Lang::Json => (scan_json(&cs), HlState::Normal),
        Lang::Toml => scan_config(&cs, true, state),
        Lang::Yaml => scan_config(&cs, false, state),
        Lang::Markdown => scan_markdown(&cs, state),
        _ => (vec![], HlState::Normal),
    }
}

/// Incremental per-document cache of the state at the start of each line.
#[derive(Default)]
pub struct Highlighter {
    lang: Option<Lang>,
    starts: Vec<HlState>,
}

impl Highlighter {
    pub fn new(lang: Lang) -> Highlighter {
        Highlighter { lang: Some(lang), starts: vec![HlState::Normal] }
    }

    pub fn lang(&self) -> Lang {
        self.lang.unwrap_or(Lang::Plain)
    }

    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = Some(lang);
        self.starts.truncate(1);
        if self.starts.is_empty() {
            self.starts.push(HlState::Normal);
        }
    }

    /// Lines at or after `line` changed: forget what we computed for them.
    pub fn invalidate(&mut self, line: usize) {
        self.starts.truncate(line.saturating_add(1).max(1));
    }

    /// Spans for line `i`, computing carried state from the last known line up to it.
    pub fn line_spans(&mut self, lines: &[String], i: usize) -> Vec<Span> {
        let lang = self.lang();
        if self.starts.is_empty() {
            self.starts.push(HlState::Normal);
        }
        while self.starts.len() <= i {
            let k = self.starts.len() - 1;
            let st = self.starts[k];
            let next = highlight_line(lang, lines.get(k).map_or("", String::as_str), st).1;
            self.starts.push(next);
        }
        highlight_line(lang, lines.get(i).map_or("", String::as_str), self.starts[i]).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::{Attribute, Comment, Constant, Function, Heading, Key, Keyword, Macro, Number, Type, Variable};
    const STR: TokenKind = TokenKind::String;

    /// (text, kind) of every span in one line.
    fn toks(lang: Lang, line: &str) -> Vec<(String, TokenKind)> {
        let (spans, _) = highlight_line(lang, line, HlState::Normal);
        let cs: Vec<char> = line.chars().collect();
        spans.iter().map(|s| (cs[s.start..s.end].iter().collect(), s.kind)).collect()
    }
    fn has(t: &[(String, TokenKind)], text: &str, kind: TokenKind) -> bool {
        t.iter().any(|(s, k)| s == text && *k == kind)
    }

    #[test]
    fn languages_are_chosen_by_file_name() {
        for (p, l) in [("/a/main.rs", Lang::Rust), ("x.PY", Lang::Python), ("a.tsx", Lang::TypeScript), ("a.js", Lang::JavaScript), ("a.h", Lang::C), ("a.cpp", Lang::Cpp), ("A.java", Lang::Java), ("a.go", Lang::Go), ("run.sh", Lang::Shell), ("Makefile", Lang::Shell), ("a.json", Lang::Json), ("Cargo.toml", Lang::Toml), ("Cargo.lock", Lang::Toml), ("c.yml", Lang::Yaml), ("README.md", Lang::Markdown), ("notes.txt", Lang::Plain), ("noext", Lang::Plain), ("/dir.rs/file", Lang::Plain)] {
            assert_eq!(lang_for_path(p), l, "{p}");
        }
        assert_eq!(Lang::Rust.name(), "Rust");
    }

    #[test]
    fn rust_basics() {
        let t = toks(Lang::Rust, "pub fn add(a: i32, b: &str) -> Result<u8, MyError> { // note");
        assert!(has(&t, "pub", Keyword) && has(&t, "fn", Keyword));
        assert!(has(&t, "add", Function), "{t:?}");
        assert!(has(&t, "i32", Type) && has(&t, "str", Type) && has(&t, "Result", Type) && has(&t, "MyError", Type));
        assert!(has(&t, "// note", Comment));
        let m = toks(Lang::Rust, "    println!(\"x = {}\", x_1 != 2.5e-3f64);");
        assert!(has(&m, "println!", Macro) && has(&m, "\"x = {}\"", STR) && has(&m, "2.5e-3f64", Number), "{m:?}");
        assert!(!m.iter().any(|(s, _)| s == "x_1"));
        let a = toks(Lang::Rust, "#[derive(Debug, Clone)] struct Point<'a> { x: &'a u32 }");
        assert!(has(&a, "#[derive(Debug, Clone)]", Attribute) && has(&a, "struct", Keyword) && has(&a, "Point", Type) && has(&a, "'a", Type), "{a:?}");
        let c = toks(Lang::Rust, "let c = 'x'; let n = '\\n'; let q = '\\'';");
        assert_eq!(c.iter().filter(|(_, k)| *k == STR).count(), 3, "{c:?}");
        let k = toks(Lang::Rust, "const MAX_SIZE: usize = 0xFF_u8 as usize;");
        assert!(has(&k, "MAX_SIZE", Constant) && has(&k, "0xFF_u8", Number) && has(&k, "as", Keyword), "{k:?}");
        // != is not a macro, x! is
        assert!(!toks(Lang::Rust, "a != b").iter().any(|(_, k)| *k == Macro));
    }

    #[test]
    fn rust_multiline_constructs_carry_state() {
        let (s, st) = highlight_line(Lang::Rust, "let a = 1; /* start", HlState::Normal);
        assert_eq!(st, HlState::Block(1));
        assert_eq!(s.last().unwrap().kind, Comment);
        let (s, st) = highlight_line(Lang::Rust, "still /* nested */ comment", st);
        assert_eq!((s.len(), st), (1, HlState::Block(1)), "nested open+close stays inside");
        let (s, st) = highlight_line(Lang::Rust, "end */ let b = 2;", st);
        assert_eq!(st, HlState::Normal);
        assert_eq!(s[0], Span { start: 0, end: 6, kind: Comment });
        assert!(s.iter().any(|x| x.kind == Keyword));
        let (_, st) = highlight_line(Lang::Rust, "let s = \"multi", HlState::Normal);
        assert_eq!(st, HlState::Str);
        let (s, st) = highlight_line(Lang::Rust, "line\"; let x = 1;", st);
        assert_eq!((s[0].kind, st), (STR, HlState::Normal));
        let (_, st) = highlight_line(Lang::Rust, "let r = r#\"raw \" text", HlState::Normal);
        assert_eq!(st, HlState::Raw(1));
        let (s, st) = highlight_line(Lang::Rust, "more\"# + 1", st);
        assert_eq!((s[0], st), (Span { start: 0, end: 6, kind: STR }, HlState::Normal));
        let one = toks(Lang::Rust, "let r = r\"a\\b\"; let b = b\"x\";");
        assert_eq!(one.iter().filter(|(_, k)| *k == STR).count(), 2, "{one:?}");
    }

    #[test]
    fn python() {
        let t = toks(Lang::Python, "def greet(name: str = \"hi\") -> None:  # say hi");
        assert!(has(&t, "def", Keyword) && has(&t, "greet", Function) && has(&t, "str", Type) && has(&t, "\"hi\"", STR) && has(&t, "None", Keyword) && has(&t, "# say hi", Comment), "{t:?}");
        let c = toks(Lang::Python, "class Shape(Base):");
        assert!(has(&c, "class", Keyword) && has(&c, "Shape", Type) && has(&c, "Base", Type));
        let d = toks(Lang::Python, "@app.route('/x')");
        assert!(has(&d, "@app.route", Attribute));
        let f = toks(Lang::Python, "x = f\"v={y}\" + r'\\d+' + 0b101");
        assert!(has(&f, "f\"v={y}\"", STR) && has(&f, "r'\\d+'", STR) && has(&f, "0b101", Number), "{f:?}");
        let (_, st) = highlight_line(Lang::Python, "doc = \"\"\"first", HlState::Normal);
        assert_eq!(st, HlState::Triple('"'));
        let (s, st) = highlight_line(Lang::Python, "middle \" quote", st);
        assert_eq!((s.len(), st), (1, HlState::Triple('"')));
        let (s, st) = highlight_line(Lang::Python, "end\"\"\"; y = 1", st);
        assert_eq!((s[0].kind, st), (STR, HlState::Normal));
        let one = toks(Lang::Python, "s = '''one line'''");
        assert_eq!(one.last().unwrap().1, STR);
    }

    #[test]
    fn javascript_typescript_and_templates() {
        let t = toks(Lang::JavaScript, "export async function load(url) { const r = await fetch(`${url}/x`); return r ?? null; }");
        assert!(has(&t, "export", Keyword) && has(&t, "function", Keyword) && has(&t, "load", Function) && has(&t, "const", Keyword) && has(&t, "fetch", Function) && has(&t, "`${url}/x`", STR) && has(&t, "null", Keyword), "{t:?}");
        let (_, st) = highlight_line(Lang::JavaScript, "const s = `line one", HlState::Normal);
        assert_eq!(st, HlState::Backtick);
        let (_, st) = highlight_line(Lang::JavaScript, "line two`;", st);
        assert_eq!(st, HlState::Normal);
        let ts = toks(Lang::TypeScript, "interface User { id: number; name: string }");
        assert!(has(&ts, "interface", Keyword) && has(&ts, "User", Type) && has(&ts, "number", Type) && has(&ts, "string", Type), "{ts:?}");
        let c = toks(Lang::JavaScript, "a = b / c; // not a regex");
        assert!(has(&c, "// not a regex", Comment));
    }

    #[test]
    fn c_cpp_java_go() {
        let c = toks(Lang::C, "#include <stdio.h>");
        assert!(has(&c, "#include", Attribute));
        let c2 = toks(Lang::C, "  int main(void) { printf(\"hi\\n\"); return 0; }");
        assert!(has(&c2, "int", Type) && has(&c2, "main", Function) && has(&c2, "void", Type) && has(&c2, "printf", Function) && has(&c2, "return", Keyword) && has(&c2, "0", Number), "{c2:?}");
        let cpp = toks(Lang::Cpp, "template <typename T> class Foo : public Bar {};");
        assert!(has(&cpp, "template", Keyword) && has(&cpp, "class", Keyword) && has(&cpp, "Foo", Type) && has(&cpp, "public", Keyword), "{cpp:?}");
        let j = toks(Lang::Java, "@Override public void run() { String s = null; }");
        assert!(has(&j, "@Override", Attribute) && has(&j, "public", Keyword) && has(&j, "run", Function) && has(&j, "String", Type) && has(&j, "null", Keyword), "{j:?}");
        let g = toks(Lang::Go, "func (s *Server) Start(addr string) error { return nil }");
        assert!(has(&g, "func", Keyword) && has(&g, "string", Type) && has(&g, "error", Type) && has(&g, "nil", Keyword) && has(&g, "Server", Type), "{g:?}");
        let (_, st) = highlight_line(Lang::C, "/* a comment", HlState::Normal);
        assert_eq!(st, HlState::Block(1));
        let (_, st) = highlight_line(Lang::C, "x */ int a;", st);
        assert_eq!(st, HlState::Normal);
    }

    #[test]
    fn shell() {
        let t = toks(Lang::Shell, "if [ -n \"$HOME\" ]; then echo ${PATH} $1 $(date); fi # done");
        assert!(has(&t, "if", Keyword) && has(&t, "then", Keyword) && has(&t, "fi", Keyword) && has(&t, "\"$HOME\"", STR) && has(&t, "${PATH}", Variable) && has(&t, "$1", Variable) && has(&t, "# done", Comment), "{t:?}");
        let h = toks(Lang::Shell, "echo $# items");
        assert!(!h.iter().any(|(_, k)| *k == Comment), "$# is not a comment: {h:?}");
        let sq = toks(Lang::Shell, "echo 'it is $HOME\\' x");
        assert!(has(&sq, "'it is $HOME\\'", STR), "{sq:?}");
        assert!(toks(Lang::Shell, "# only a comment").iter().all(|(_, k)| *k == Comment));
    }

    #[test]
    fn json() {
        let t = toks(Lang::Json, "  \"name\": \"iyakki\", \"n\": -12.5e3, \"ok\": true, \"x\": null, \"arr\": [1, 2]");
        assert!(has(&t, "\"name\"", Key) && has(&t, "\"iyakki\"", STR) && has(&t, "-12.5e3", Number) && has(&t, "true", Keyword) && has(&t, "null", Keyword) && has(&t, "\"arr\"", Key), "{t:?}");
        assert!(!has(&t, "\"iyakki\"", Key));
        let esc = toks(Lang::Json, "\"a\\\"b\": 1");
        assert!(has(&esc, "\"a\\\"b\"", Key), "{esc:?}");
    }

    #[test]
    fn toml_and_yaml() {
        let t = toks(Lang::Toml, "[dependencies]");
        assert_eq!(t, vec![("[dependencies]".to_string(), Heading)]);
        let t = toks(Lang::Toml, "serde = { version = \"1\", features = [\"derive\"] } # why");
        assert!(has(&t, "serde", Key) && has(&t, "\"1\"", STR) && has(&t, "# why", Comment), "{t:?}");
        let t = toks(Lang::Toml, "opt-level = 3");
        assert!(has(&t, "opt-level", Key) && has(&t, "3", Number), "{t:?}");
        let t = toks(Lang::Toml, "enabled = true");
        assert!(has(&t, "enabled", Key) && has(&t, "true", Keyword));
        let (_, st) = highlight_line(Lang::Toml, "desc = \"\"\"first", HlState::Normal);
        assert_eq!(st, HlState::Triple('"'));
        let (s, st) = highlight_line(Lang::Toml, "second\"\"\"", st);
        assert_eq!((s[0].kind, st), (STR, HlState::Normal));
        let y = toks(Lang::Yaml, "  - name: iyakki  # the os");
        assert!(has(&y, "name", Key) && has(&y, "# the os", Comment), "{y:?}");
        let y = toks(Lang::Yaml, "replicas: 3");
        assert!(has(&y, "replicas", Key) && has(&y, "3", Number));
        let y = toks(Lang::Yaml, "enabled: yes");
        assert!(has(&y, "yes", Keyword));
        assert_eq!(toks(Lang::Yaml, "---"), vec![("---".to_string(), Heading)]);
        let url = toks(Lang::Yaml, "url: http://example.com:80/x");
        assert!(has(&url, "url", Key) && !has(&url, "http", Key), "{url:?}");
    }

    #[test]
    fn markdown() {
        assert_eq!(toks(Lang::Markdown, "## Title here"), vec![("## Title here".to_string(), Heading)]);
        assert!(toks(Lang::Markdown, "#hashtag").is_empty(), "no space, not a heading");
        let t = toks(Lang::Markdown, "- use `code` and **bold** and [link](http://x.y)");
        assert!(has(&t, "-", Attribute) && has(&t, "`code`", STR) && has(&t, "**bold**", Keyword) && has(&t, "[link]", Function) && has(&t, "(http://x.y)", Comment), "{t:?}");
        let (s, st) = highlight_line(Lang::Markdown, "```rust", HlState::Normal);
        assert_eq!((s[0].kind, st), (Attribute, HlState::Fence));
        let (s, st) = highlight_line(Lang::Markdown, "let x = 1; # not a heading", st);
        assert_eq!((s[0].kind, st), (STR, HlState::Fence));
        let (_, st) = highlight_line(Lang::Markdown, "```", st);
        assert_eq!(st, HlState::Normal);
        assert!(has(&toks(Lang::Markdown, "1. first"), "1.", Attribute));
        assert!(has(&toks(Lang::Markdown, "> quoted"), "> quoted", Comment));
    }

    #[test]
    fn plain_text_has_no_spans_and_long_lines_are_skipped() {
        assert!(toks(Lang::Plain, "fn main() {}").is_empty());
        let long = "x".repeat(MAX_LINE + 1);
        let (s, st) = highlight_line(Lang::Rust, &long, HlState::Block(2));
        assert!(s.is_empty());
        assert_eq!(st, HlState::Block(2), "state is preserved across a skipped line");
    }

    #[test]
    fn unicode_columns_are_character_columns() {
        let (s, _) = highlight_line(Lang::Rust, "let é = \"ünï\"; // ✓", HlState::Normal);
        let cs: Vec<char> = "let é = \"ünï\"; // ✓".chars().collect();
        let strings: Vec<String> = s.iter().filter(|x| x.kind == STR).map(|x| cs[x.start..x.end].iter().collect()).collect();
        assert_eq!(strings, vec!["\"ünï\""]);
        assert_eq!(s.last().unwrap().end, cs.len());
    }

    #[test]
    fn spans_are_always_sorted_in_range_and_disjoint_even_on_garbage() {
        let langs = [Lang::Plain, Lang::Rust, Lang::Python, Lang::JavaScript, Lang::TypeScript, Lang::C, Lang::Cpp, Lang::Java, Lang::Go, Lang::Shell, Lang::Json, Lang::Toml, Lang::Yaml, Lang::Markdown];
        let alphabet: Vec<char> = "ab_ 1.e-+\"'`\\/*#!<>[](){}=:$@&%;,~é\t".chars().collect();
        let mut s = 0xdead_beef_cafe_f00du64;
        let mut rnd = move |n: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % n as u64) as usize
        };
        for lang in langs {
            let mut st = HlState::Normal;
            for _ in 0..800 {
                let line: String = (0..rnd(60)).map(|_| alphabet[rnd(alphabet.len())]).collect();
                let n = line.chars().count();
                let (spans, next) = highlight_line(lang, &line, st);
                let mut end = 0;
                for sp in &spans {
                    assert!(sp.start >= end && sp.end > sp.start && sp.end <= n, "{lang:?} {line:?} {spans:?}");
                    end = sp.end;
                }
                st = next;
            }
        }
    }

    #[test]
    fn the_cache_recomputes_only_from_the_changed_line() {
        let lines: Vec<String> = ["let a = 1;", "/* open", "inside", "close */ let b = 2;", "let c = 3;"].iter().map(|s| s.to_string()).collect();
        let mut h = Highlighter::new(Lang::Rust);
        assert!(h.line_spans(&lines, 2).iter().all(|s| s.kind == Comment), "line 2 is inside the comment");
        assert!(h.line_spans(&lines, 4).iter().any(|s| s.kind == Keyword), "after the comment closes, code again");
        // edit line 1 so the comment never opens
        let mut l2 = lines.clone();
        l2[1] = "// closed".into();
        h.invalidate(1);
        assert!(h.line_spans(&l2, 2).is_empty() || h.line_spans(&l2, 2).iter().all(|s| s.kind != Comment));
        // out-of-range lines are harmless
        assert!(h.line_spans(&l2, 99).is_empty());
        h.set_lang(Lang::Plain);
        assert!(h.line_spans(&l2, 0).is_empty());
    }
}
