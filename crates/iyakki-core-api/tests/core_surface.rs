//! Charter step 2 (docs/development/CORE-COMPATIBILITY-DESIGN.md sections 8 and 10): the public-surface snapshot of `iyakki-core-api`.
//!
//! A small tolerant scanner (no dependency, no `rustdoc` JSON) reads the source of this crate, follows the `pub mod` declarations from
//! `src/lib.rs`, and lists the public items one per line: modules, structs and their public fields, enums and their variants, traits
//! and their methods, public functions and methods of inherent impls, constants, type aliases, re-exports and the names of trait
//! impls. `#[serde(..)]`, `#[cfg(..)]`, `#[non_exhaustive]` and `#[deprecated]` attributes are part of the line they belong to (they are
//! the wire shape). Doc comments, derives, bodies, private items and test code are not.
//!
//! `docs/development/core-surface.txt` holds that list. The gate is a ratchet:
//!   * an item that is in the code and not in the file: ADDITION, the file must be updated (append);
//!   * an item that is in the file and not in the code: REMOVED or CHANGED SHAPE; it fails until the file records it as a
//!     `removed<TAB>item<TAB>note` line (the note says why and which version/alias covers it). A silent disappearance never passes;
//!   * `dist/release/compat-check.sh` also compares the file with the committed one: a line of the committed file may not vanish from
//!     the working file without turning into a `removed` line.
//!
//! Update the file with `IYAKKI_CORE_SURFACE_WRITE=1 cargo test -p iyakki-core-api --test core_surface -- --nocapture`. Additions are
//! appended. A removal or change needs `IYAKKI_CORE_SURFACE_NOTE="why"`; the gone lines are then kept as `removed` lines with the note.
//!
//! Limits (honest): items behind a non-`pub mod` are not listed (a `pub use` of them is); derives and `impl Trait for Type` bodies are not
//! compared; macros that generate items are not seen.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const FILE: &str = "docs/development/core-surface.txt";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ---------------------------------------------------------------------------------------------------------- the scanner

/// Comments out, string and char contents made harmless (structure characters inside them replaced), everything else as it was.
fn clean(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut o = String::with_capacity(src.len());
    let mut i = 0;
    let hide = |c: char| if "{}()[]<>,;#".contains(c) { '_' } else { c };
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            o.push(' ');
        } else if c == 'r' && matches!(b.get(i + 1), Some('"') | Some('#')) && {
            let mut j = i + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            b.get(j) == Some(&'"')
        } {
            // raw string r#"..."#
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            j += 1;
            o.push('"');
            while j < b.len() {
                if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                o.push(hide(b[j]));
                j += 1;
            }
            o.push('"');
            i = j;
        } else if c == '"' {
            o.push('"');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    o.push('_');
                    i += 1;
                }
                o.push(hide(b[i]));
                i += 1;
            }
            o.push('"');
            i += 1;
        } else if c == '\'' {
            // a char literal ('x', '\n', '{') or a lifetime ('a)
            let lit = if b.get(i + 1) == Some(&'\\') { b.iter().skip(i + 2).position(|&x| x == '\'').map(|p| i + 2 + p) } else if b.get(i + 2) == Some(&'\'') { Some(i + 2) } else { None };
            if let Some(end) = lit {
                o.push_str("'_'");
                i = end + 1;
            } else {
                o.push(c);
                i += 1;
            }
        } else {
            o.push(c);
            i += 1;
        }
    }
    o
}

fn squeeze(s: &str) -> String {
    let mut o = String::new();
    for w in s.split_whitespace() {
        if !o.is_empty() {
            o.push(' ');
        }
        o.push_str(w);
    }
    for (a, b) in [("( ", "("), (" )", ")"), ("[ ", "["), (" ]", "]"), ("< ", "<"), (" >", ">"), (" ,", ","), (" ;", ";"), (", )", ")"), (", ]", "]"), (", }", " }"), (" :", ":"), ("& ", "&")] {
        while o.contains(a) {
            o = o.replace(a, b);
        }
    }
    o
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
}

struct Scan<'a> {
    c: &'a [char],
    i: usize,
}

impl Scan<'_> {
    fn ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn balanced(&mut self, open: char, close: char) -> String {
        let start = self.i;
        let mut d = 0;
        while self.i < self.c.len() {
            if self.c[self.i] == open {
                d += 1;
            } else if self.c[self.i] == close {
                d -= 1;
                if d == 0 {
                    self.i += 1;
                    break;
                }
            }
            self.i += 1;
        }
        self.c[start..self.i.min(self.c.len())].iter().collect()
    }

    /// Text up to (not including) the first of `stops` at depth 0 of () [] <>; with `braces`, {} count as depth too. Returns the stop char.
    fn until(&mut self, stops: &[char], braces: bool, generics: bool) -> (String, Option<char>) {
        let start = self.i;
        let (mut p, mut a) = (0i32, 0i32);
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            let prev = if self.i > 0 { self.c[self.i - 1] } else { ' ' };
            if p == 0 && a == 0 && stops.contains(&ch) {
                let s: String = self.c[start..self.i].iter().collect();
                return (s, Some(ch));
            }
            match ch {
                '(' | '[' => p += 1,
                ')' | ']' => p -= 1,
                '{' if braces => p += 1,
                '}' if braces => {
                    if p == 0 {
                        break;
                    }
                    p -= 1;
                }
                '<' if generics => a += 1,
                '>' if generics && prev != '-' && prev != '=' => a = (a - 1).max(0),
                '}' => break,
                _ => {}
            }
            self.i += 1;
        }
        (self.c[start..self.i].iter().collect(), None)
    }
}

fn vis(head: &str) -> (bool, &str) {
    let h = head.trim_start();
    if let Some(r) = h.strip_prefix("pub") {
        if let Some(r2) = r.strip_prefix('(') {
            let close = r2.find(')').map_or(0, |p| p + 1);
            return (false, &r2[close..]);
        }
        if r.starts_with(char::is_whitespace) {
            return (true, r.trim_start());
        }
    }
    (false, h)
}

fn strip_quals(mut h: &str) -> &str {
    loop {
        let n = ["unsafe ", "async ", "default "].iter().find_map(|q| h.strip_prefix(q));
        match n {
            Some(r) => h = r,
            None => return h,
        }
    }
}

struct Ctx<'a> {
    out: &'a mut Vec<String>,
    dir: &'a Path,
}

fn attr_text(attrs: &[String]) -> String {
    let keep: Vec<String> = attrs
        .iter()
        .filter(|a| {
            let a = a.as_str();
            (a.starts_with("serde") || a.starts_with("cfg") || a.starts_with("non_exhaustive") || a.starts_with("deprecated")) && !a.starts_with("cfg(test)") && !a.starts_with("cfg(not(test))")
        })
        .map(|a| format!("[{a}]"))
        .collect();
    if keep.is_empty() { String::new() } else { format!("{} ", keep.join("")) }
}

/// Parse items of one block (module body, struct body, enum body, trait body, impl body). `path` is the module path, `owner` the
/// type or trait the members belong to.
fn block(s: &mut Scan, ctx: &mut Ctx, kind: Kind, path: &str, owner: &str, public_owner: bool, depth_file: &mut Vec<(String, PathBuf)>) {
    let mut attrs: Vec<String> = Vec::new();
    loop {
        s.ws();
        if s.i >= s.c.len() {
            return;
        }
        match s.c[s.i] {
            '}' => {
                s.i += 1;
                return;
            }
            ',' | ';' => {
                s.i += 1;
                continue;
            }
            '#' => {
                s.i += 1;
                if s.c.get(s.i) == Some(&'!') {
                    s.i += 1;
                }
                s.ws();
                let t = s.balanced('[', ']');
                let inner = t.trim_start_matches('[').trim_end_matches(']');
                attrs.push(squeeze(inner));
                continue;
            }
            _ => {}
        }
        let at = attr_text(&attrs);
        let test_only = attrs.iter().any(|a| a.starts_with("cfg(test)"));
        attrs.clear();
        match kind {
            Kind::Struct | Kind::Enum => {
                let (text, _) = s.until(&[','], true, true);
                let text = squeeze(&text);
                if text.is_empty() {
                    continue;
                }
                if kind == Kind::Struct {
                    let (p, rest) = vis(&text);
                    if p && public_owner {
                        ctx.out.push(format!("{path} | {at}{owner}.{rest}"));
                    }
                } else if public_owner {
                    ctx.out.push(format!("{path} | {at}{owner}::{text}"));
                }
                continue;
            }
            _ => {}
        }
        // an item
        let peek: String = s.c[s.i..].iter().take(200).collect();
        let (is_pub, h) = vis(&peek);
        let in_trait = kind == Kind::Trait;
        let word = strip_quals(h).split(|c: char| c.is_whitespace() || c == '(' || c == '<' || c == '{' || c == '!').next().unwrap_or("");
        let simple = matches!(word, "const" | "static" | "type" | "use") && !strip_quals(h).starts_with("const fn") && !strip_quals(h).starts_with("const unsafe fn");
        let (head, stop) = if simple { s.until(&[';'], true, false) } else { s.until(&[';', '{'], false, true) };
        let head = squeeze(&head);
        let (_, h2) = vis(&head);
        let h2 = strip_quals(h2);
        let h2 = match h2.strip_prefix("const ") {
            Some(r) if r.starts_with("fn ") || r.starts_with("unsafe fn ") => strip_quals(r),
            _ => h2,
        };
        let h2 = if h2.starts_with("extern") && h2.contains(" fn ") { &h2[h2.find("fn ").unwrap_or(0)..] } else { h2 };
        let first = h2.split(|c: char| c.is_whitespace() || c == '(' || c == '<' || c == '!').next().unwrap_or("");
        let shown = is_pub || in_trait;
        let line = |what: String, out: &mut Vec<String>| {
            if shown && public_owner && !test_only {
                out.push(format!("{path} | {}{what}", at));
            }
        };
        match (first, stop) {
            ("mod", Some(';')) => {
                let name = h2["mod ".len()..].trim().to_string();
                if is_pub && !test_only {
                    ctx.out.push(format!("{path} | {at}pub mod {name}"));
                    depth_file.push((name, ctx.dir.to_path_buf()));
                }
            }
            ("mod", Some('{')) => {
                s.i += 1;
                let name = h2["mod ".len()..].trim().to_string();
                if is_pub && !test_only {
                    ctx.out.push(format!("{path} | {at}pub mod {name}"));
                    let sub = format!("{path}::{name}");
                    block(s, ctx, Kind::Module, &sub, "", true, depth_file);
                } else {
                    skip_block(s);
                }
            }
            ("struct" | "union", Some('{')) => {
                s.i += 1;
                let name = h2.split_whitespace().nth(1).unwrap_or("").to_string();
                let pubn = is_pub && !test_only;
                if pubn {
                    ctx.out.push(format!("{path} | {at}pub {h2}"));
                    block(s, ctx, Kind::Struct, path, strip_gen(&name), true, depth_file);
                } else {
                    skip_block(s);
                }
            }
            ("struct", _) => line(format!("pub {h2}"), ctx.out),
            ("enum", Some('{')) => {
                s.i += 1;
                let name = h2.split_whitespace().nth(1).unwrap_or("").to_string();
                if is_pub && !test_only {
                    ctx.out.push(format!("{path} | {at}pub {h2}"));
                    block(s, ctx, Kind::Enum, path, strip_gen(&name), true, depth_file);
                } else {
                    skip_block(s);
                }
            }
            ("trait", Some('{')) => {
                s.i += 1;
                let name = h2.split_whitespace().nth(1).unwrap_or("").to_string();
                if is_pub && !test_only {
                    ctx.out.push(format!("{path} | {at}pub {h2}"));
                    block(s, ctx, Kind::Trait, path, strip_gen(&name), true, depth_file);
                } else {
                    skip_block(s);
                }
            }
            ("impl", Some('{')) => {
                s.i += 1;
                if test_only {
                    skip_block(s);
                } else if h2.contains(" for ") {
                    ctx.out.push(format!("{path} | {at}{h2}"));
                    skip_block(s);
                } else {
                    let ty = impl_type(h2);
                    let cfgs = at.clone();
                    let mut sub = Vec::new();
                    let mut inner = Ctx { out: &mut sub, dir: ctx.dir };
                    block(s, &mut inner, Kind::Impl, path, &ty, true, depth_file);
                    for l in sub {
                        ctx.out.push(l.replacen(" | ", &format!(" | {cfgs}"), 1));
                    }
                }
            }
            ("fn", Some('{')) => {
                s.i += 1;
                skip_block(s);
                line(fn_text(h2, owner, in_trait, kind), ctx.out);
            }
            ("fn", _) => line(fn_text(h2, owner, in_trait, kind), ctx.out),
            ("const" | "static", _) => {
                let (kw, rest) = h2.split_once(' ').unwrap_or((h2, ""));
                let t = if rest.len() > 160 { format!("{}...", &rest[..rest.char_indices().nth(160).map_or(rest.len(), |x| x.0)]) } else { rest.to_string() };
                let t = if kind == Kind::Impl || in_trait { format!("{owner}::{t}") } else { t };
                line(format!("pub {kw} {t}"), ctx.out);
            }
            ("type", _) => {
                let rest = h2.trim_start_matches("type ");
                let t = if kind == Kind::Impl || in_trait { format!("{owner}::{rest}") } else { rest.to_string() };
                line(format!("pub type {t}"), ctx.out);
            }
            ("use", _) => line(format!("pub {h2}"), ctx.out),
            ("extern" | "macro_rules", Some('{')) => {
                s.i += 1;
                skip_block(s);
            }
            (_, Some('{')) => {
                s.i += 1;
                skip_block(s);
            }
            (_, Some(';')) => {}
            _ => {
                if s.i < s.c.len() {
                    s.i += 1;
                }
            }
        }
        if let Some(';') = stop {
            if s.i < s.c.len() && s.c[s.i] == ';' {
                s.i += 1;
            }
        }
    }
}

fn impl_type(h: &str) -> String {
    let mut r = h.trim_start_matches("impl").trim_start();
    if r.starts_with('<') {
        let mut d = 0;
        let mut end = 0;
        for (k, ch) in r.char_indices() {
            match ch {
                '<' => d += 1,
                '>' => {
                    d -= 1;
                    if d == 0 {
                        end = k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        r = r[end..].trim_start();
    }
    strip_gen(r.split(" where ").next().unwrap_or(r)).to_string()
}

fn strip_gen(n: &str) -> &str {
    n.split('<').next().unwrap_or(n).trim()
}

fn fn_text(h: &str, owner: &str, in_trait: bool, kind: Kind) -> String {
    if kind == Kind::Impl || in_trait {
        let rest = h.trim_start_matches("fn ");
        format!("pub fn {owner}::{rest}")
    } else {
        format!("pub {h}")
    }
}

fn skip_block(s: &mut Scan) {
    let mut d = 1;
    while s.i < s.c.len() && d > 0 {
        match s.c[s.i] {
            '{' => d += 1,
            '}' => d -= 1,
            _ => {}
        }
        s.i += 1;
    }
}

fn scan_file(file: &Path, path: &str, out: &mut Vec<String>) {
    let src = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let cleaned = clean(&src);
    let chars: Vec<char> = cleaned.chars().collect();
    let mut s = Scan { c: &chars, i: 0 };
    let dir = file.parent().unwrap().to_path_buf();
    let mut subs: Vec<(String, PathBuf)> = Vec::new();
    let mut ctx = Ctx { out, dir: &dir };
    // a file is a module body; an unmatched `}` cannot happen in a well-formed file
    block(&mut s, &mut ctx, Kind::Module, path, "", true, &mut subs);
    let is_root = file.file_name().is_some_and(|n| n == "lib.rs" || n == "mod.rs");
    for (name, _) in subs {
        let base = if is_root { dir.clone() } else { dir.join(file.file_stem().unwrap()) };
        let a = base.join(format!("{name}.rs"));
        let b = base.join(&name).join("mod.rs");
        let f = if a.is_file() { a } else { b };
        scan_file(&f, &format!("{path}::{name}"), ctx.out);
    }
}

fn surface() -> Vec<String> {
    let mut out = Vec::new();
    scan_file(&root().join("crates/iyakki-core-api/src/lib.rs"), "crate", &mut out);
    let set: BTreeSet<String> = out.into_iter().map(|l| squeeze(&l)).collect();
    set.into_iter().collect()
}

// ---------------------------------------------------------------------------------------------------------- the file

struct Snapshot {
    items: Vec<String>,
    removed: Vec<(String, String)>,
    header: Vec<String>,
}

fn read_file(text: &str) -> Snapshot {
    let (mut items, mut removed, mut header) = (Vec::new(), Vec::new(), Vec::new());
    for l in text.lines() {
        if l.starts_with('#') || l.trim().is_empty() {
            header.push(l.to_string());
        } else if let Some(r) = l.strip_prefix("removed\t") {
            let (item, note) = r.split_once('\t').unwrap_or((r, ""));
            removed.push((item.to_string(), note.to_string()));
        } else {
            items.push(l.to_string());
        }
    }
    Snapshot { items, removed, header }
}

fn file_text(s: &Snapshot) -> String {
    let mut o = String::new();
    for h in &s.header {
        o.push_str(h);
        o.push('\n');
    }
    for i in &s.items {
        o.push_str(i);
        o.push('\n');
    }
    for (i, n) in &s.removed {
        o.push_str(&format!("removed\t{i}\t{n}\n"));
    }
    o
}

const HEADER: &str = "# The public surface of crates/iyakki-core-api, one item per line (docs/development/CORE-COMPATIBILITY-DESIGN.md section 8).\n# Written by crates/iyakki-core-api/tests/core_surface.rs; do not edit by hand. A new item is appended by running the test with\n# IYAKKI_CORE_SURFACE_WRITE=1. An item that disappears or changes shape becomes a `removed<TAB>item<TAB>note` line, which needs\n# IYAKKI_CORE_SURFACE_NOTE=\"why, and which version or alias covers it\". Gate: ./run.sh compat-check.\n";

#[test]
fn the_public_surface_of_the_core_api_only_grows_and_every_removal_has_a_note() {
    let now = surface();
    let path = root().join(FILE);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut snap = read_file(&text);
    if std::env::var("IYAKKI_CORE_SURFACE_WRITE").is_ok() {
        let had: BTreeSet<&String> = snap.items.iter().collect();
        let cur: BTreeSet<&String> = now.iter().collect();
        let gone: Vec<String> = snap.items.iter().filter(|i| !cur.contains(i)).cloned().collect();
        let new: Vec<String> = now.iter().filter(|i| !had.contains(i)).cloned().collect();
        let note = std::env::var("IYAKKI_CORE_SURFACE_NOTE").unwrap_or_default();
        assert!(gone.is_empty() || !note.trim().is_empty(), "these items disappeared or changed shape; set IYAKKI_CORE_SURFACE_NOTE=\"why\" to record them:\n  {}", gone.join("\n  "));
        for g in gone {
            snap.removed.push((g, note.trim().to_string()));
        }
        snap.items.retain(|i| cur.contains(i));
        snap.items.extend(new);
        if snap.header.is_empty() {
            snap.header = HEADER.lines().map(String::from).collect();
        }
        std::fs::write(&path, file_text(&snap)).unwrap();
        println!("core-surface: wrote {} items, {} removed lines", snap.items.len(), snap.removed.len());
        return;
    }
    let had: BTreeSet<&String> = snap.items.iter().collect();
    let cur: BTreeSet<&String> = now.iter().collect();
    let added: Vec<&&String> = cur.difference(&had).collect();
    let gone: Vec<&&String> = had.difference(&cur).collect();
    let mut msg = String::new();
    if !gone.is_empty() {
        msg.push_str("\nREMOVED or CHANGED SHAPE (not allowed without a note; additions only, charter section 4): record it with IYAKKI_CORE_SURFACE_WRITE=1 IYAKKI_CORE_SURFACE_NOTE=\"why\":\n");
        for g in gone {
            msg.push_str(&format!("  - {g}\n"));
        }
    }
    if !added.is_empty() {
        msg.push_str("\nNEW public items; append them with IYAKKI_CORE_SURFACE_WRITE=1 cargo test -p iyakki-core-api --test core_surface:\n");
        for a in added {
            msg.push_str(&format!("  + {a}\n"));
        }
    }
    assert!(msg.is_empty(), "{msg}");
    for (i, n) in &snap.removed {
        assert!(!n.trim().is_empty(), "removed line without a note: {i}");
    }
}

#[test]
fn the_scanner_reads_the_shapes_it_is_meant_to() {
    let src = "
        //! doc
        pub const V: u32 = 1; // note
        pub(crate) const HIDDEN: u32 = 2;
        #[derive(Clone)]
        #[serde(tag = \"t\", rename_all = \"snake_case\")]
        pub enum Msg { A, B { x: u32, y: Vec<String> }, #[serde(rename = \"c\")] C(u8) }
        pub struct S { pub a: BTreeMap<String, u32>, b: u8, #[serde(default)] pub c: Option<String> }
        pub trait T { fn one(&self) -> u32; fn two(&mut self, x: &str) -> Result<(), E> { Ok(()) } }
        impl S { pub fn new(a: u32) -> S { S { a: 1 } } fn private(&self) {} pub const K: u8 = 3; }
        impl T for S { fn one(&self) -> u32 { 1 } }
        pub fn free<T: Fn(u32) -> u32>(f: T) -> u32 { f(1) }
        pub use other::{A, B};
        mod private_mod { pub fn hidden() {} }
        #[cfg(test)]
        mod tests;
    ";
    let c = clean(src);
    let chars: Vec<char> = c.chars().collect();
    let mut s = Scan { c: &chars, i: 0 };
    let mut out = Vec::new();
    let dir = PathBuf::from(".");
    let mut ctx = Ctx { out: &mut out, dir: &dir };
    block(&mut s, &mut ctx, Kind::Module, "m", "", true, &mut Vec::new());
    let text = out.iter().map(|l| squeeze(l)).collect::<Vec<_>>().join("\n");
    for want in [
        "m | pub const V: u32 = 1",
        "m | [serde(tag = \"t\", rename_all = \"snake_case\")] pub enum Msg",
        "m | Msg::A",
        "m | Msg::B { x: u32, y: Vec<String> }",
        "m | [serde(rename = \"c\")] Msg::C(u8)",
        "m | pub struct S",
        "m | S.a: BTreeMap<String, u32>",
        "m | [serde(default)] S.c: Option<String>",
        "m | pub trait T",
        "m | pub fn T::one(&self) -> u32",
        "m | pub fn T::two(&mut self, x: &str) -> Result<(), E>",
        "m | pub fn S::new(a: u32) -> S",
        "m | pub const S::K: u8 = 3",
        "m | impl T for S",
        "m | pub fn free<T: Fn(u32) -> u32>(f: T) -> u32",
        "m | pub use other::{A, B}",
    ] {
        assert!(text.contains(want), "scanner lost {want:?}; got:\n{text}");
    }
    for no in ["HIDDEN", "S.b", "private", "hidden", "tests"] {
        assert!(!text.contains(no), "scanner showed {no:?}:\n{text}");
    }
}
