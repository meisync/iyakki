//! Agentic code editor model: buffer, syntax highlighting (iyakki-edit), branch diff
//! review against [`MemFs`] forks, and an execution/prompt console.
//!
//! Everything here is headless and unit-tested; the `gui` feature renders it
//! with egui (see `src/bin/editor.rs`).

use iyakki_vfs::{DiffKind, MemFs, VfsError};

// ---------- syntax highlighting (iyakki-edit) ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Highlight Rust source with IYAKKI's own highlighter. Spans are byte ranges, sorted, non-overlapping.
pub fn highlight_rust(src: &str) -> Vec<Span> {
    use iyakki_edit::{highlight_line, HlState, Lang};
    let mut out = Vec::new();
    let mut state = HlState::Normal;
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        let (spans, next) = highlight_line(Lang::Rust, text, state);
        state = next;
        let bytes: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
        for sp in spans {
            let kind = match sp.kind {
                iyakki_edit::TokenKind::Keyword => TokenKind::Keyword,
                iyakki_edit::TokenKind::String => TokenKind::String,
                iyakki_edit::TokenKind::Comment => TokenKind::Comment,
                iyakki_edit::TokenKind::Number | iyakki_edit::TokenKind::Constant => TokenKind::Number,
                iyakki_edit::TokenKind::Type => TokenKind::Type,
                iyakki_edit::TokenKind::Function | iyakki_edit::TokenKind::Macro => TokenKind::Function,
                _ => continue,
            };
            out.push(Span { start: offset + bytes[sp.start], end: offset + bytes[sp.end], kind });
        }
        offset += line.len();
    }
    out
}

// ---------- line diff ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Added(String),
    Removed(String),
}

/// Line-based diff using LCS. Falls back to remove-all/add-all for very large inputs.
pub fn diff_lines(a: &str, b: &str) -> Vec<DiffLine> {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    if x.len().saturating_mul(y.len()) > 4_000_000 {
        return x.iter().map(|l| DiffLine::Removed(l.to_string())).chain(y.iter().map(|l| DiffLine::Added(l.to_string()))).collect();
    }
    let (n, m) = (x.len(), y.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if x[i] == y[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if x[i] == y[j] {
            out.push(DiffLine::Same(x[i].into()));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(DiffLine::Removed(x[i].into()));
            i += 1;
        } else {
            out.push(DiffLine::Added(y[j].into()));
            j += 1;
        }
    }
    out.extend(x[i..].iter().map(|l| DiffLine::Removed(l.to_string())));
    out.extend(y[j..].iter().map(|l| DiffLine::Added(l.to_string())));
    out
}

// ---------- branch review ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: DiffKind,
    pub lines: Vec<DiffLine>,
}

/// Review of an agent's `MemFs` branch against the base it forked from.
pub struct BranchReview {
    pub base: MemFs,
    pub branch: MemFs,
}

impl BranchReview {
    pub fn new(base: MemFs, branch: MemFs) -> Self {
        Self { base, branch }
    }

    fn text(fs: &MemFs, p: &str) -> String {
        fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    }

    pub fn changes(&self) -> Vec<FileChange> {
        self.branch
            .diff(&self.base)
            .into_iter()
            .map(|(path, kind)| {
                let lines = diff_lines(&Self::text(&self.base, &path), &Self::text(&self.branch, &path));
                FileChange { path, kind, lines }
            })
            .collect()
    }

    /// Accept one file's change from the branch into the base.
    pub fn accept(&mut self, path: &str) -> Result<(), VfsError> {
        if self.branch.exists(path) {
            let data = self.branch.read(path)?;
            self.base.write(path, &data)
        } else {
            self.base.remove(path)
        }
    }

    /// Reject one file's change: revert the branch to the base's version.
    pub fn reject(&mut self, path: &str) -> Result<(), VfsError> {
        if self.base.exists(path) {
            let data = self.base.read(path)?;
            self.branch.write(path, &data)
        } else {
            self.branch.remove(path)
        }
    }

    pub fn accept_all(&mut self) -> Result<(), VfsError> {
        for c in self.changes() {
            self.accept(&c.path)?;
        }
        Ok(())
    }
}

// ---------- editor + console ----------

#[derive(Default)]
pub struct Console {
    pub lines: Vec<String>,
    pub prompts: Vec<String>,
}

impl Console {
    pub fn log(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
        if self.lines.len() > 5000 {
            self.lines.drain(..1000);
        }
    }
    pub fn submit_prompt(&mut self, s: &str) {
        self.prompts.push(s.to_string());
        self.log(format!("> {s}"));
    }
}

pub struct Editor {
    pub path: String,
    pub text: String,
    pub console: Console,
}

impl Editor {
    pub fn new(path: &str) -> Self {
        Self { path: path.into(), text: String::new(), console: Console::default() }
    }

    pub fn open(&mut self, fs: &MemFs, path: &str) -> Result<(), VfsError> {
        self.text = String::from_utf8_lossy(&fs.read(path)?).into_owned();
        self.path = path.into();
        Ok(())
    }

    pub fn save(&self, fs: &mut MemFs) -> Result<(), VfsError> {
        fs.write(&self.path, self.text.as_bytes())
    }

    pub fn spans(&self) -> Vec<Span> {
        if self.path.ends_with(".rs") { highlight_rust(&self.text) } else { Vec::new() }
    }

    /// Run a guest module from the VFS-less bytes and stream its log to the console.
    pub fn run_guest(&mut self, wat_or_wasm: &[u8], entry: &str) {
        use iyakki_wasm::{Sandbox, SandboxConfig};
        let fs = std::sync::Arc::new(std::sync::Mutex::new(MemFs::new()));
        let res = Sandbox::new(SandboxConfig::default())
            .and_then(|s| s.run(wat_or_wasm, entry, fs, iyakki_sec::CapabilityStore::new(), vec![]));
        match res {
            Ok(o) => {
                for l in String::from_utf8_lossy(&o.log).lines() {
                    self.console.log(l);
                }
                self.console.log(format!("exit: {:?} (fuel {})", o.result, o.fuel_used));
            }
            Err(e) => self.console.log(format!("error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust() {
        let src = "// hi\nfn add(a: u32) -> u32 { let s = \"x\"; a + 1 }";
        let spans = highlight_rust(src);
        let has = |k, t: &str| spans.iter().any(|s| s.kind == k && &src[s.start..s.end] == t);
        assert!(has(TokenKind::Comment, "// hi"));
        assert!(has(TokenKind::Keyword, "fn"));
        assert!(has(TokenKind::Keyword, "let"));
        assert!(has(TokenKind::Function, "add"));
        assert!(has(TokenKind::Type, "u32"));
        assert!(has(TokenKind::String, "\"x\""));
        assert!(has(TokenKind::Number, "1"));
        assert!(spans.windows(2).all(|w| w[0].end <= w[1].start), "sorted, non-overlapping");
    }

    #[test]
    fn diff_basic() {
        let d = diff_lines("a\nb\nc", "a\nx\nc\nd");
        assert_eq!(
            d,
            vec![
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Added("x".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn branch_review_accept_reject() {
        let mut base = MemFs::new();
        base.write("/a.rs", b"one\ntwo").unwrap();
        base.write("/gone", b"x").unwrap();
        let mut br = base.fork();
        br.write("/a.rs", b"one\n2").unwrap();
        br.write("/new", b"n").unwrap();
        br.remove("/gone").unwrap();
        let mut r = BranchReview::new(base, br);
        let ch = r.changes();
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].path, "/a.rs");
        assert!(ch[0].lines.contains(&DiffLine::Added("2".into())));
        r.accept("/a.rs").unwrap();
        r.reject("/new").unwrap();
        r.accept("/gone").unwrap();
        assert_eq!(r.base.read("/a.rs").unwrap(), b"one\n2");
        assert!(!r.base.exists("/gone"));
        assert!(!r.branch.exists("/new"));
        assert!(r.changes().is_empty());
    }

    #[test]
    fn editor_roundtrip_and_run() {
        let mut fs = MemFs::new();
        let mut ed = Editor::new("/m.rs");
        ed.text = "fn main() {}".into();
        ed.save(&mut fs).unwrap();
        let mut ed2 = Editor::new("/x");
        ed2.open(&fs, "/m.rs").unwrap();
        assert_eq!(ed2.text, "fn main() {}");
        assert!(!ed2.spans().is_empty());
        let wat = r#"(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1)
          (data (i32.const 0) "hello") (func (export "_start") (call $l (i32.const 0) (i32.const 5))))"#;
        ed2.run_guest(wat.as_bytes(), "_start");
        assert!(ed2.console.lines.iter().any(|l| l == "hello"));
        ed2.run_guest(b"garbage", "_start");
        assert!(ed2.console.lines.last().unwrap().starts_with("error"));
        ed2.console.submit_prompt("refactor");
        assert_eq!(ed2.console.prompts, vec!["refactor"]);
    }
}
