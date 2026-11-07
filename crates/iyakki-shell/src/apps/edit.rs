//! EDIT: a small code editor. Files are opened and saved through the same permission checks as the CLI
//! and the agent's file tools (symlinks resolved first, then checked against the user's read/write
//! scopes). The text engine, highlighter and diff live in `iyakki-edit`.
//!
//! The assistant never writes here: its replies are turned into *proposals* (see `propose`) that show up
//! as highlighted hunks you accept or reject one by one, and even accepted hunks only change the
//! in-memory text until you save.

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};

use iyakki_edit::diff::{apply_hunks, diff_hunks, summary};
use iyakki_edit::proposal::parse_reply;
use iyakki_edit::syntax::{lang_for_path, Highlighter, Lang, TokenKind};
use iyakki_edit::{Buffer, Hunk, Move};

use super::widgets::{render_confirm, Form, FormEvent, PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, Nav, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(110, 224, 160);
const AREA_TEXT: usize = 1;
const AREA_REVIEW: usize = 2;
const F_QUERY: usize = 10;
const F_REPLACE: usize = 11;
const F_GOTO: usize = 12;
const TAB: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    New,
    Open,
    Quit,
    CloseTab,
    /// Ask the assistant to continue the file at the cursor (F6).
    Complete,
}

enum Modal {
    Open { picker: Picker, dir: PathBuf },
    PathForm(Form, bool /* save-as */),
    Confirm(Pending, String),
    Overwrite(PathBuf),
}

struct Find {
    query: TextField,
    replace: TextField,
    show_replace: bool,
    focus_replace: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Same,
    Del,
    Add,
}

struct RRow {
    kind: RowKind,
    text: String,
    hunk: Option<usize>,
    lineno: Option<usize>,
    rejected: bool,
}

struct Review {
    base: Vec<String>,
    hunks: Vec<Hunk>,
    decided: Vec<Option<bool>>,
    cur: usize,
    rows: Vec<RRow>,
    scroll: usize,
    new_trailing: bool,
    old_trailing: bool,
    explanation: String,
}

impl Review {
    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut pos = 0;
        let same = |rows: &mut Vec<RRow>, from: usize, to: usize, base: &[String]| {
            for (i, l) in base[from..to].iter().enumerate() {
                rows.push(RRow { kind: RowKind::Same, text: l.clone(), hunk: None, lineno: Some(from + i + 1), rejected: false });
            }
        };
        for (i, h) in self.hunks.iter().enumerate() {
            same(&mut rows, pos, h.old_start, &self.base);
            match self.decided[i] {
                None => {
                    for (k, l) in h.old.iter().enumerate() {
                        rows.push(RRow { kind: RowKind::Del, text: l.clone(), hunk: Some(i), lineno: Some(h.old_start + k + 1), rejected: false });
                    }
                    for l in &h.new {
                        rows.push(RRow { kind: RowKind::Add, text: l.clone(), hunk: Some(i), lineno: None, rejected: false });
                    }
                }
                Some(true) => {
                    for l in &h.new {
                        rows.push(RRow { kind: RowKind::Add, text: l.clone(), hunk: Some(i), lineno: None, rejected: false });
                    }
                }
                Some(false) => {
                    for (k, l) in h.old.iter().enumerate() {
                        rows.push(RRow { kind: RowKind::Same, text: l.clone(), hunk: Some(i), lineno: Some(h.old_start + k + 1), rejected: true });
                    }
                }
            }
            pos = h.old_start + h.old.len();
        }
        same(&mut rows, pos, self.base.len(), &self.base);
        self.rows = rows;
    }

    fn focus_current(&mut self) {
        if let Some(i) = self.rows.iter().position(|r| r.hunk == Some(self.cur)) {
            self.scroll = i.saturating_sub(4);
        }
    }

    fn accepted(&self) -> usize {
        self.decided.iter().filter(|d| **d == Some(true)).count()
    }

    fn result_text(&self) -> String {
        let accept: Vec<bool> = self.decided.iter().map(|d| *d == Some(true)).collect();
        let lines = apply_hunks(&self.base, &self.hunks, &accept);
        let mut t = lines.join("\n");
        let trailing = if self.accepted() > 0 { self.new_trailing } else { self.old_trailing };
        if trailing && !t.is_empty() {
            t.push('\n');
        }
        t
    }
}

/// One open file while it is not the one on screen.
struct DocState {
    buf: Buffer,
    hl: Highlighter,
    path: Option<PathBuf>,
    writable: bool,
    scroll: usize,
    hscroll: usize,
    follow: bool,
}

pub const MAX_DOCS: usize = 12;

/// What running the open file printed (shown under the text until closed).
struct RunOutput {
    title: String,
    lines: Vec<String>,
    ok: bool,
}

pub struct EditApp {
    /// The open file, queued to be saved on the host computer (window mode).
    host_save: Option<(String, Vec<u8>)>,
    buf: Buffer,
    hl: Highlighter,
    path: Option<PathBuf>,
    writable: bool,
    scroll: usize,
    hscroll: usize,
    rows_visible: usize,
    cols_visible: usize,
    follow: bool,
    geo: Geo,
    find: Option<Find>,
    goto: Option<TextField>,
    case_sensitive: bool,
    modal: Option<Modal>,
    status: Option<(String, bool)>,
    review: Option<Review>,
    /// When the person last typed into the file, and until when keys must not answer a suggestion that arrived meanwhile.
    last_typed: Option<std::time::Instant>,
    review_guard: Option<std::time::Instant>,
    dir: PathBuf,
    /// A file chosen in the open dialog while the current text has unsaved changes: opened if the user confirms.
    parked_open: Option<PathBuf>,
    /// Embedded in the chat: Esc and some chrome behave differently.
    pub embedded: bool,
    /// Drawn inside the AI IDE: flat tabs, a one-line text toolbar and smaller type (the IDE's density sets the scale).
    pub ide: bool,
    /// An AI IDE workspace's folder: nothing outside it is opened or saved.
    pub root: Option<PathBuf>,
    /// Without a workspace: the folders the Open dialog and typed paths stay inside (home, workspaces, narrow scopes).
    confine: Option<iyakki_agent::scope::FileRoots>,
    /// The other files that are open (the one on screen lives in the fields above and has `None` here).
    docs: Vec<Option<DocState>>,
    active: usize,
    /// A run of the open file in progress, its result, and the panel that shows it.
    running: Option<(String, std::sync::mpsc::Receiver<(bool, String)>)>,
    output: Option<RunOutput>,
    /// A completion being written by the assistant.
    completing: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// The text before and after the cursor when the completion was asked for.
    complete_at: Option<(String, String)>,
    /// Markdown files show rendered in the AI IDE (`edit_preview.rs`); the person can switch a file to the text.
    md: super::edit_preview::MdPreview,
}

#[derive(Clone, Copy, Default)]
struct Geo {
    cw: f32,
    lh: f32,
}

/// Tabs become spaces and control characters become dots for display; returns the characters shown and,
/// for each character of the line (plus the end), the display column it starts at.
fn expand(line: &str) -> (Vec<char>, Vec<usize>) {
    let mut out = Vec::new();
    let mut map = Vec::new();
    for c in line.chars() {
        map.push(out.len());
        if c == '\t' {
            let n = TAB - out.len() % TAB;
            out.extend(std::iter::repeat_n(' ', n));
        } else if c.is_control() {
            out.push('·');
        } else {
            out.push(c);
        }
    }
    map.push(out.len());
    (out, map)
}

/// Character index whose display column is at or before `dcol`.
fn char_at_display(map: &[usize], dcol: usize) -> usize {
    let n = map.len().saturating_sub(1);
    match map.binary_search(&dcol) {
        Ok(i) => i.min(n),
        Err(i) => i.saturating_sub(1).min(n),
    }
}

fn kind_color(k: TokenKind) -> [u8; 4] {
    match k {
        TokenKind::Keyword => rgb(196, 152, 255),
        TokenKind::Type => rgb(94, 214, 214),
        TokenKind::Function => rgb(120, 172, 255),
        TokenKind::Macro => rgb(255, 170, 110),
        TokenKind::String => rgb(150, 224, 140),
        TokenKind::Number | TokenKind::Constant => rgb(255, 200, 120),
        TokenKind::Comment => rgb(112, 124, 152),
        TokenKind::Attribute => rgb(255, 150, 175),
        TokenKind::Variable => rgb(124, 204, 255),
        TokenKind::Heading => rgb(255, 196, 94),
        TokenKind::Key => rgb(120, 172, 255),
    }
}

fn read_text_file(cx: &AppCx, input: &str) -> Result<(PathBuf, String), String> {
    let real = iyakki_agent::scope::resolve(Path::new("/"), input.trim())?;
    iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &real)?;
    let meta = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", real.display()));
    }
    if meta.len() as usize > iyakki_edit::buffer::MAX_BYTES {
        return Err(format!("{} is too large to edit here (limit 2 MiB)", real.display()));
    }
    let bytes = std::fs::read(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8 text", real.display()))?;
    Ok((real, text))
}

fn write_atomic(real: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = real.parent().ok_or("no parent directory")?;
    let name = real.file_name().ok_or("no file name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.iyakki-tmp"));
    let mode = std::fs::metadata(real).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o644);
    let r = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, real)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(|e| format!("{}: {e}", real.display()))
}

impl EditApp {
    pub fn new(cx: &AppCx) -> EditApp {
        let confine = iyakki_agent::scope::FileRoots::new(&cx.session.policy, &cx.session.user, cx.services.dir());
        let dir = confine.home.clone();
        EditApp {
            host_save: None,
            buf: Buffer::new(),
            hl: Highlighter::new(Lang::Plain),
            path: None,
            writable: true,
            scroll: 0,
            hscroll: 0,
            rows_visible: 20,
            cols_visible: 80,
            follow: true,
            geo: Geo::default(),
            find: None,
            goto: None,
            case_sensitive: false,
            modal: None,
            status: None,
            review: None,
            last_typed: None,
            review_guard: None,
            dir,
            parked_open: None,
            embedded: false,
            ide: false,
            root: None,
            confine: Some(confine),
            docs: vec![None],
            active: 0,
            running: None,
            completing: None,
            complete_at: None,
            md: Default::default(),
            output: None,
        }
    }

    // ------------------------------------------------------------------ several files

    fn take_doc(&mut self) -> DocState {
        DocState {
            buf: std::mem::replace(&mut self.buf, Buffer::new()),
            hl: std::mem::replace(&mut self.hl, Highlighter::new(Lang::Plain)),
            path: self.path.take(),
            writable: self.writable,
            scroll: self.scroll,
            hscroll: self.hscroll,
            follow: self.follow,
        }
    }

    fn put_doc(&mut self, d: DocState) {
        self.buf = d.buf;
        self.hl = d.hl;
        self.path = d.path;
        self.writable = d.writable;
        self.scroll = d.scroll;
        self.hscroll = d.hscroll;
        self.follow = d.follow;
        self.review = None;
        self.find = None;
        self.goto = None;
    }

    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    pub fn active_doc(&self) -> usize {
        self.active
    }

    fn doc_title(path: Option<&Path>, dirty: bool) -> String {
        let name = path.and_then(|p| p.file_name()).map_or("untitled".to_string(), |n| n.to_string_lossy().into_owned());
        if dirty { format!("{name} ●") } else { name }
    }

    /// The names of the open files, in order.
    pub fn doc_titles(&self) -> Vec<String> {
        (0..self.docs.len())
            .map(|i| match &self.docs[i] {
                Some(d) => Self::doc_title(d.path.as_deref(), d.buf.is_dirty()),
                None => Self::doc_title(self.path.as_deref(), self.buf.is_dirty()),
            })
            .collect()
    }

    fn any_dirty(&self) -> bool {
        self.buf.is_dirty() || self.docs.iter().flatten().any(|d| d.buf.is_dirty())
    }

    fn switch_doc(&mut self, i: usize) {
        if i >= self.docs.len() || i == self.active {
            return;
        }
        let cur = self.take_doc();
        self.docs[self.active] = Some(cur);
        if let Some(d) = self.docs[i].take() {
            self.put_doc(d);
        }
        self.active = i;
        self.status = None;
    }

    /// An empty file in a tab of its own.
    fn new_doc(&mut self) {
        if self.embedded {
            return self.new_file();
        }
        if self.docs.len() >= MAX_DOCS {
            return self.fail(format!("At most {MAX_DOCS} files can be open; close one first."));
        }
        let cur = self.take_doc();
        self.docs[self.active] = Some(cur);
        self.docs.push(None);
        self.active = self.docs.len() - 1;
        self.put_doc(DocState { buf: Buffer::new(), hl: Highlighter::new(Lang::Plain), path: None, writable: true, scroll: 0, hscroll: 0, follow: true });
        self.status = None;
    }

    /// Close one file; the last one left becomes an empty file instead of leaving the editor.
    fn close_doc(&mut self, i: usize) {
        if i >= self.docs.len() {
            return;
        }
        if self.docs.len() == 1 {
            return self.new_file();
        }
        if i == self.active {
            let _gone = self.take_doc();
            self.docs.remove(i);
            let next = i.min(self.docs.len() - 1);
            if let Some(d) = self.docs[next].take() {
                self.put_doc(d);
            }
            self.active = next;
        } else {
            self.docs.remove(i);
            if i < self.active {
                self.active -= 1;
            }
        }
    }

    /// A click on a tab's close mark: the question about unsaved work is asked for the file being closed.
    fn request_close_doc(&mut self, i: usize, cx: &AppCx) {
        if i != self.active {
            self.switch_doc(i);
        }
        self.request(Pending::CloseTab, cx);
    }

    fn step_doc(&mut self, forward: bool) {
        let n = self.docs.len();
        if n > 1 {
            self.switch_doc(if forward { (self.active + 1) % n } else { (self.active + n - 1) % n });
        }
    }

    // ------------------------------------------------------------------ running the open file

    /// F5: run the file. WebAssembly (`.wasm`, `.wat`) runs in the supervisor's sandbox with this account's limits;
    /// Python, JavaScript and shell scripts run in the container cluster with no network.
    fn run_file(&mut self, cx: &AppCx) {
        if self.running.is_some() {
            return self.fail("A run is already going; wait for it to end.");
        }
        let Some(path) = self.path.clone() else {
            return self.fail("Save the file first (Ctrl+S) so it has a name and a kind.");
        };
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let code = self.buf.text();
        let (tx, rx) = std::sync::mpsc::channel();
        match ext.as_str() {
            "wasm" | "wat" => {
                cx.audit.record(&cx.session.user, "edit.run", &format!("wasm {name}"));
                let (host, user, policy) = (cx.host.clone(), cx.session.user.clone(), cx.session.policy.clone());
                let bytes = code.into_bytes();
                let _ = std::thread::Builder::new().name("iyakki-edit-run".into()).spawn(move || {
                    let r = host.run_wasm(&user, &policy, &bytes, "_start");
                    let _ = tx.send(match r {
                        Ok(out) => (true, out),
                        Err(e) => (false, e),
                    });
                });
            }
            "py" | "js" | "mjs" | "sh" => {
                if cx.session.policy.cli != iyakki_sec::policy::CliLevel::Full {
                    return self.fail("Running scripts needs the full command-line permission for your account.");
                }
                let lang = match ext.as_str() {
                    "py" => "python",
                    "sh" => "shell",
                    _ => "node",
                };
                if cx.services.cluster.state() != iyakki_agent::cluster::ClusterState::Running {
                    return self.fail("Start the container cluster first (CLI → CLIs → k3s) to run scripts here; WebAssembly files run without it.");
                }
                cx.audit.record(&cx.session.user, "edit.run", &format!("{lang} {name}"));
                let svc = cx.services.clone();
                let _ = std::thread::Builder::new().name("iyakki-edit-run".into()).spawn(move || {
                    let r = svc.cluster.sandbox_run(lang, &code, 60, false);
                    let _ = tx.send(match r {
                        Ok(res) => (res.ok, res.output),
                        Err(e) => (false, e),
                    });
                });
            }
            _ => return self.fail("Only .py, .js, .sh and WebAssembly (.wasm, .wat) files can be run from here."),
        }
        self.output = None;
        self.running = Some((name.clone(), rx));
        self.ok(format!("Running {name}…"));
    }

    fn poll_run(&mut self) {
        let Some((name, rx)) = &self.running else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => (false, "the run stopped without an answer".into()),
        };
        let name = name.clone();
        self.running = None;
        let lines: Vec<String> = r.1.lines().take(400).map(|l| l.chars().take(400).collect()).collect();
        self.output = Some(RunOutput { title: name.clone(), lines: if lines.is_empty() { vec!["(no output)".into()] } else { lines }, ok: r.0 });
        if r.0 {
            self.ok(format!("{name} finished."));
        } else {
            self.fail(format!("{name} did not finish cleanly."));
        }
    }

    /// The files open in tabs (saved ones), in tab order.
    pub fn open_paths(&self) -> Vec<PathBuf> {
        (0..self.docs.len())
            .filter_map(|i| match &self.docs[i] {
                Some(d) => d.path.clone(),
                None => self.path.clone(),
            })
            .collect()
    }

    /// Is this file open in a tab (and has it unsaved changes)?
    pub fn tab_state(&self, p: &Path) -> Option<bool> {
        if self.path.as_deref() == Some(p) {
            return Some(self.buf.is_dirty());
        }
        self.docs.iter().flatten().find(|d| d.path.as_deref() == Some(p)).map(|d| d.buf.is_dirty())
    }

    pub fn any_unsaved(&self) -> bool {
        self.any_dirty()
    }

    /// Re-read an open file from disk (the board or the assistant changed it); a tab with unsaved changes is left alone.
    /// Returns true when the tab now shows the file's new content.
    pub fn reload_clean(&mut self, cx: &AppCx, p: &Path) -> bool {
        let Some(i) = (0..self.docs.len()).find(|i| match &self.docs[*i] {
            Some(d) => d.path.as_deref() == Some(p),
            None => self.path.as_deref() == Some(p),
        }) else { return false };
        if self.tab_state(p) != Some(false) {
            return false;
        }
        let Ok((real, text)) = read_text_file(cx, &p.to_string_lossy()) else { return false };
        if self.text_of_doc(i).as_deref() == Some(text.as_str()) {
            return true;
        }
        let back = self.active;
        self.switch_doc(i);
        let (scroll, hscroll) = (self.scroll, self.hscroll);
        let ok = self.load(real, &text, cx).is_ok();
        self.scroll = scroll;
        self.hscroll = hscroll;
        self.switch_doc(back);
        ok
    }

    fn text_of_doc(&self, i: usize) -> Option<String> {
        match self.docs.get(i)? {
            Some(d) => Some(d.buf.text()),
            None => Some(self.buf.text()),
        }
    }

    /// Show the cursor at a 1-based line.
    /// A Markdown file open in the AI IDE (shown rendered unless the person chose the text).
    fn md_capable(&self) -> bool {
        self.ide && !self.embedded && self.path.as_deref().is_some_and(super::edit_preview::is_markdown)
    }

    /// The rendered view is what is on screen now (not while the assistant's change is being reviewed).
    /// A web address clicked in the Markdown page (the AI IDE opens it in its Browser tab).
    pub fn take_md_link(&mut self) -> Option<String> {
        self.md.take_clicked()
    }

    pub fn md_previewing(&self) -> bool {
        self.md_capable() && self.review.is_none() && self.path.as_deref().is_some_and(|p| !self.md.editing(p))
    }

    pub fn md_set_edit(&mut self, on: bool) {
        if let Some(p) = self.path.clone() {
            self.md.set_edit(&p, on);
            if on {
                self.follow = true;
            }
        }
    }

    pub fn goto_line(&mut self, line: usize) {
        // somebody asked for a line (a search hit, a problem, a link): that is the text, not the picture
        self.md_set_edit(true);
        self.buf.goto_line(line.clamp_safe(1, self.buf.line_count().max(1)));
        self.follow = true;
    }

    /// Close the tab of a file (no questions asked: the IDE only calls it for files that are gone or clean).
    pub fn close_path(&mut self, p: &Path) {
        if let Some(i) = (0..self.docs.len()).find(|i| match &self.docs[*i] {
            Some(d) => d.path.as_deref() == Some(p),
            None => self.path.as_deref() == Some(p),
        }) {
            self.close_doc(i);
        }
    }

    pub fn set_status(&mut self, msg: &str, ok: bool) {
        if ok { self.ok(msg) } else { self.fail(msg) }
    }

    /// What a toolbar button sends: a plain key, or a `Sub` event when embedded so the host can tell it
    /// apart from its own buttons.
    fn bev(&self, k: Key) -> Event {
        if self.embedded { Event::Sub(1, k) } else { Event::Key(k) }
    }

    // ------------------------------------------------------------------ public surface (chat uses these)

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn text(&self) -> String {
        self.buf.text()
    }
    pub fn is_dirty(&self) -> bool {
        self.buf.is_dirty()
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
    /// The "discard unsaved changes and close?" question is on screen.
    pub fn confirming_quit(&self) -> bool {
        matches!(self.modal, Some(Modal::Confirm(Pending::Quit, _)))
    }
    pub fn in_review(&self) -> bool {
        self.review.is_some()
    }
    pub fn lang(&self) -> Lang {
        self.hl.lang()
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }
    pub fn buffer(&self) -> &Buffer {
        &self.buf
    }
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buf
    }
    pub fn pending_hunks(&self) -> usize {
        self.review.as_ref().map_or(0, |r| r.decided.iter().filter(|d| d.is_none()).count())
    }

    /// The selected lines (1-based, inclusive) and their text, for the assistant.
    pub fn selection_info(&self) -> Option<(usize, usize, String)> {
        let (a, b) = self.buf.selection()?;
        let last = if b.1 == 0 && b.0 > a.0 { b.0 - 1 } else { b.0 };
        let text = (a.0..=last).map(|i| self.buf.line(i)).collect::<Vec<_>>().join("\n");
        Some((a.0 + 1, last + 1, text))
    }

    fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }
    fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    fn set_lang_for(&mut self, path: Option<&Path>, name_hint: &str) {
        let lang = path.map_or_else(|| lang_for_path(name_hint), |p| lang_for_path(&p.to_string_lossy()));
        self.hl.set_lang(lang);
    }

    fn load(&mut self, real: PathBuf, text: &str, cx: &AppCx) -> Result<(), String> {
        let buf = Buffer::from_text(text).map_err(|e| e.to_string())?;
        self.writable = cx.session.policy.allows_write(&cx.session.user, &real.to_string_lossy());
        self.buf = buf;
        self.set_lang_for(Some(&real), "");
        if let Some(d) = real.parent() {
            self.dir = d.to_path_buf();
        }
        self.path = Some(real);
        self.scroll = 0;
        self.hscroll = 0;
        self.review = None;
        self.find = None;
        self.goto = None;
        self.follow = true;
        Ok(())
    }

    /// Open a file (resolved and permission-checked). Errors are shown in the status line too.
    pub fn open_path(&mut self, cx: &AppCx, input: &str) -> Result<(), String> {
        if let Some(root) = &self.root {
            if let Err(e) = iyakki_agent::workspace::resolve_in(root, input.trim()) {
                self.fail(e.clone());
                return Err(e);
            }
        }
        let confined;
        let input = match (&self.root, &self.confine) {
            (None, Some(c)) => match c.resolve(&c.home, input) {
                Ok(p) => {
                    confined = p.to_string_lossy().into_owned();
                    confined.as_str()
                }
                Err(e) => {
                    self.fail(e.clone());
                    return Err(e);
                }
            },
            _ => input,
        };
        let r = read_text_file(cx, input).and_then(|(real, text)| {
            cx.audit.record(&cx.session.user, "edit.open", &real.to_string_lossy());
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !self.embedded {
                // already open: go to it. Otherwise it gets a tab, unless this one is an untouched empty file.
                let open_at = if self.path.as_deref() == Some(real.as_path()) { Some(self.active) } else { self.docs.iter().position(|d| d.as_ref().is_some_and(|d| d.path.as_deref() == Some(real.as_path()))) };
                if let Some(i) = open_at {
                    self.switch_doc(i);
                    return Ok(name);
                }
                let pristine = self.path.is_none() && !self.buf.is_dirty() && self.buf.byte_len() == 0;
                if !pristine {
                    if self.docs.len() >= MAX_DOCS {
                        return Err(format!("At most {MAX_DOCS} files can be open; close one first."));
                    }
                    self.new_doc();
                }
            }
            self.load(real, &text, cx).map(|_| name)
        });
        match r {
            Ok(name) => {
                let ro = if self.writable { "" } else { " (read-only for your account)" };
                self.ok(format!("Opened {name}{ro}."));
                Ok(())
            }
            Err(e) => {
                self.fail(e.clone());
                Err(e)
            }
        }
    }

    fn new_file(&mut self) {
        self.buf = Buffer::new();
        self.path = None;
        self.writable = true;
        self.hl.set_lang(Lang::Plain);
        self.scroll = 0;
        self.hscroll = 0;
        self.review = None;
        self.status = None;
    }

    fn save_to(&mut self, cx: &AppCx, input: &str) -> Result<(), String> {
        if let Some(root) = &self.root {
            iyakki_agent::workspace::resolve_in(root, input.trim())?;
        }
        let real = iyakki_agent::scope::resolve(Path::new("/"), input.trim())?;
        iyakki_agent::scope::check_write(&cx.session.policy, &cx.session.user, &real)?;
        if real.is_dir() {
            return Err(format!("{} is a directory", real.display()));
        }
        write_atomic(&real, &self.buf.text())?;
        cx.audit.record(&cx.session.user, "edit.save", &real.to_string_lossy());
        self.buf.mark_saved();
        self.writable = true;
        self.set_lang_for(Some(&real), "");
        self.hl.invalidate(0);
        if let Some(d) = real.parent() {
            self.dir = d.to_path_buf();
        }
        let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.path = Some(real);
        self.ok(format!("Saved {name} ({} bytes).", self.buf.byte_len()));
        Ok(())
    }

    fn save(&mut self, cx: &AppCx) {
        if self.review.is_some() {
            self.fail("Finish reviewing the assistant's changes first (Enter applies, Esc discards).");
            return;
        }
        match self.path.clone() {
            Some(p) => {
                if let Err(e) = self.save_to(cx, &p.to_string_lossy()) {
                    cx.audit.record(&cx.session.user, "edit.save.denied", &p.to_string_lossy());
                    self.fail(e);
                }
            }
            None => self.open_save_as(),
        }
    }

    fn open_save_as(&mut self) {
        let start = self.path.as_ref().map_or_else(|| format!("{}/", self.dir.display()), |p| p.display().to_string());
        let f = Form::new("Save as").path("File path", &start, "where to save (you need write access there)", crate::filepicker::PathKind::Save, &[]);
        self.modal = Some(Modal::PathForm(f, true));
    }

    // ------------------------------------------------------------------ open dialog

    /// Is this real path one the Open dialog may show? (Inside a workspace the workspace decides; otherwise the roots.)
    fn pick_ok(&self, real: &Path) -> bool {
        self.root.is_some() || self.confine.as_ref().is_none_or(|c| c.contains(real))
    }

    fn list_dir(&self, cx: &AppCx, dir: &Path) -> Vec<String> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten().take(5000) {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if !cx.session.policy.allows_read(&cx.session.user, &p.to_string_lossy()) {
                    continue;
                }
                // a link that leads out of the roots is not shown
                if self.confine.is_some() && !std::fs::canonicalize(&p).is_ok_and(|r| self.pick_ok(&r)) {
                    continue;
                }
                match std::fs::metadata(&p) {
                    Ok(m) if m.is_dir() => dirs.push(format!("{name}/")),
                    Ok(m) if m.is_file() => files.push(name),
                    _ => {}
                }
            }
        }
        dirs.sort();
        files.sort();
        let mut items = vec!["Type a path…".to_string()];
        if dir.parent().is_some_and(|p| cx.session.policy.allows_read(&cx.session.user, &p.to_string_lossy()) && self.pick_ok(p)) {
            items.push("../".into());
        }
        items.extend(dirs);
        items.extend(files);
        items
    }

    fn open_dialog(&mut self, cx: &AppCx, dir: PathBuf) {
        // never show a folder outside the roots: go back to the start
        let dir = match std::fs::canonicalize(&dir) {
            Ok(r) if self.pick_ok(&r) => r,
            _ if self.confine.is_some() && self.root.is_none() => self.confine.as_ref().map(|c| c.home.clone()).unwrap_or(dir),
            _ => dir,
        };
        let items = self.list_dir(cx, &dir);
        self.modal = Some(Modal::Open { picker: Picker::new(&format!("Open — {}", dir.display()), items), dir });
    }

    // ------------------------------------------------------------------ proposals from the assistant

    /// Turn an assistant reply into a reviewable proposal. `Ok(None)`: the reply holds no edit.
    /// `Ok(Some(text))`: review started; `text` is the assistant's explanation. Nothing changes until
    /// the user accepts hunks.
    pub fn propose(&mut self, reply: &str) -> Result<Option<String>, String> {
        let current = self.buf.text();
        let Some(p) = parse_reply(reply, &current)? else { return Ok(None) };
        let strip = |t: &str| -> Vec<String> {
            let t = t.replace("\r\n", "\n");
            let body = t.strip_suffix('\n').unwrap_or(&t).to_string();
            body.split('\n').map(String::from).collect()
        };
        let (old, new) = (strip(&current), strip(&p.new_text));
        let hunks = diff_hunks(&old, &new);
        if hunks.is_empty() {
            return Err("The suggested edit changes nothing in the file.".into());
        }
        let n = hunks.len();
        let (added, removed) = summary(&hunks);
        let mut r = Review { base: old, decided: vec![None; n], hunks, cur: 0, rows: vec![], scroll: 0, new_trailing: p.new_text.ends_with('\n'), old_trailing: current.ends_with('\n'), explanation: p.explanation.clone() };
        r.rebuild();
        r.focus_current();
        self.review = Some(r);
        // a suggestion that arrives while the person is typing must not turn the next letters into answers (y, n, a, x, j...)
        let now = std::time::Instant::now();
        self.review_guard = self.last_typed.filter(|t| now.duration_since(*t) < std::time::Duration::from_millis(1500)).map(|_| now + std::time::Duration::from_millis(800));
        self.find = None;
        self.goto = None;
        self.modal = None;
        self.ok(format!("The assistant suggests {n} change{} (+{added} −{removed}). y accept · n reject · a accept all · Enter apply · Esc discard.", if n == 1 { "" } else { "s" }));
        Ok(Some(p.explanation))
    }

    fn review_key(&mut self, k: &Key, cx: &AppCx) {
        if matches!(k, Key::Char(_)) && self.review_guard.is_some_and(|t| std::time::Instant::now() < t) {
            // still typing: the guard stays a little longer
            self.review_guard = Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
            self.ok("The assistant's suggestion arrived while you were typing; those keys were not taken as answers. y accept · n reject · a accept all.");
            return;
        }
        let Some(r) = self.review.as_mut() else { return };
        let n = r.hunks.len();
        let next_undecided = |r: &Review| (1..=n).map(|d| (r.cur + d) % n).find(|i| r.decided[*i].is_none());
        match k {
            Key::Char('y') | Key::Char('Y') => {
                r.decided[r.cur] = Some(true);
                if let Some(i) = next_undecided(r) {
                    r.cur = i;
                }
                r.rebuild();
                r.focus_current();
            }
            Key::Char('n') | Key::Char('N') => {
                r.decided[r.cur] = Some(false);
                if let Some(i) = next_undecided(r) {
                    r.cur = i;
                }
                r.rebuild();
                r.focus_current();
            }
            Key::Char('a') | Key::Char('A') => {
                for d in r.decided.iter_mut().filter(|d| d.is_none()) {
                    *d = Some(true);
                }
                r.rebuild();
                self.finish_review(cx);
            }
            Key::Char('x') | Key::Char('X') | Key::Esc => {
                self.review = None;
                cx.audit.record(&cx.session.user, "edit.discard", "");
                self.ok("Discarded the assistant's suggestion; your file is unchanged.");
            }
            Key::Char(']') | Key::Down | Key::Char('j') => {
                r.cur = (r.cur + 1) % n;
                r.focus_current();
            }
            Key::Char('[') | Key::Up | Key::Char('k') => {
                r.cur = (r.cur + n - 1) % n;
                r.focus_current();
            }
            Key::PageDown => r.scroll = (r.scroll + self.rows_visible.max(1)).min(r.rows.len().saturating_sub(1)),
            Key::PageUp => r.scroll = r.scroll.saturating_sub(self.rows_visible.max(1)),
            Key::Enter => self.finish_review(cx),
            Key::Ctrl('s') => self.fail("Finish reviewing the assistant's changes first (Enter applies, Esc discards)."),
            _ => {}
        }
    }

    fn finish_review(&mut self, cx: &AppCx) {
        let Some(r) = self.review.take() else { return };
        let (acc, total) = (r.accepted(), r.hunks.len());
        if acc == 0 {
            cx.audit.record(&cx.session.user, "edit.discard", &format!("0/{total}"));
            self.ok("No changes accepted; your file is unchanged.");
            return;
        }
        match self.buf.set_text_undoable(&r.result_text()) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "edit.apply", &format!("{acc}/{total} hunks"));
                self.follow = true;
                self.ok(format!("Applied {acc} of {total} change{}. Ctrl+Z undoes; Ctrl+S saves.", if total == 1 { "" } else { "s" }));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    // ------------------------------------------------------------------ editing keys

    fn after_edit(&mut self) {
        if let Some(l) = self.buf.take_dirty() {
            self.hl.invalidate(l);
        }
        self.follow = true;
    }

    fn do_edit(&mut self, r: Result<(), iyakki_edit::EditError>) {
        if let Err(e) = r {
            self.fail(e.to_string());
        }
        self.after_edit();
    }

    fn mv(&mut self, m: Move, extend: bool) {
        self.buf.move_to(m, extend);
        self.follow = true;
    }

    fn nav(&mut self, nav: Nav, shift: bool, ctrl: bool) {
        let rows = self.rows_visible.max(1);
        let m = match (nav, ctrl) {
            (Nav::Left, true) => Move::WordLeft,
            (Nav::Right, true) => Move::WordRight,
            (Nav::Home, true) => Move::DocStart,
            (Nav::End, true) => Move::DocEnd,
            (Nav::Left, _) => Move::Left,
            (Nav::Right, _) => Move::Right,
            (Nav::Up, _) => Move::Up,
            (Nav::Down, _) => Move::Down,
            (Nav::Home, _) => Move::Home,
            (Nav::End, _) => Move::End,
            (Nav::PageUp, _) => Move::PageUp(rows),
            (Nav::PageDown, _) => Move::PageDown(rows),
        };
        self.mv(m, shift);
    }

    fn start_find(&mut self, replace: bool) {
        let seed = self.buf.selected_text().filter(|t| !t.contains('\n') && t.chars().count() < 100);
        let f = self.find.get_or_insert_with(|| Find { query: TextField::new("Find…", false), replace: TextField::new("Replace with…", false), show_replace: false, focus_replace: false });
        f.show_replace |= replace;
        f.focus_replace = false;
        if let Some(s) = seed {
            f.query.set(&s);
        }
        self.goto = None;
    }

    fn find_step(&mut self, forward: bool) {
        let Some(f) = &self.find else { return };
        let q = f.query.value.clone();
        if q.is_empty() {
            return;
        }
        if self.buf.select_next(&q, self.case_sensitive, forward) {
            self.follow = true;
        } else {
            self.fail(format!("“{q}” not found."));
        }
    }

    fn find_key(&mut self, k: &Key) {
        let Some(f) = self.find.as_mut() else { return };
        match k {
            Key::Esc => {
                self.find = None;
            }
            Key::Tab | Key::BackTab if f.show_replace => f.focus_replace = !f.focus_replace,
            Key::Enter if f.focus_replace => self.replace_current(),
            Key::Enter | Key::Down => self.find_step(true),
            Key::Up => self.find_step(false),
            Key::Ctrl('h') => f.show_replace = !f.show_replace,
            other => {
                let field = if f.focus_replace { &mut f.replace } else { &mut f.query };
                let before = field.value.clone();
                field.handle(other);
                let changed = !f.focus_replace && f.query.value != before;
                if changed {
                    // incremental search: jump to the first match at or after where the search began
                    let q = f.query.value.clone();
                    let from = self.buf.selection().map_or(self.buf.cursor, |(a, _)| a);
                    if let Some((a, b)) = self.buf.find_next(&q, self.case_sensitive, from, true) {
                        self.buf.select_range(a, b);
                        self.follow = true;
                    }
                }
            }
        }
    }

    fn replace_current(&mut self) {
        let Some(f) = &self.find else { return };
        let (q, with) = (f.query.value.clone(), f.replace.value.clone());
        if q.is_empty() {
            return;
        }
        let selected_is_match = self.buf.selected_text().is_some_and(|t| if self.case_sensitive { t == q } else { t.to_lowercase() == q.to_lowercase() });
        if selected_is_match {
            let r = self.buf.insert(&with.replace('\n', " "));
            self.do_edit(r);
        }
        self.find_step(true);
    }

    fn replace_all(&mut self) {
        let Some(f) = &self.find else { return };
        let (q, with) = (f.query.value.clone(), f.replace.value.clone());
        match self.buf.replace_all(&q, &with, self.case_sensitive) {
            Ok(0) => self.fail(format!("“{q}” not found.")),
            Ok(n) => {
                self.ok(format!("Replaced {n} occurrence{}.", if n == 1 { "" } else { "s" }));
                self.after_edit();
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    fn goto_key(&mut self, k: &Key) {
        let Some(g) = self.goto.as_mut() else { return };
        match k {
            Key::Esc => self.goto = None,
            Key::Enter => {
                let n: usize = g.value.trim().parse().unwrap_or(0);
                if n == 0 {
                    self.fail("Enter a line number.");
                } else {
                    self.buf.goto_line(n.min(self.buf.line_count()));
                    self.follow = true;
                    self.goto = None;
                }
            }
            Key::Char(c) if !c.is_ascii_digit() => {}
            other => {
                g.handle(other);
            }
        }
    }

    fn request(&mut self, p: Pending, cx: &AppCx) {
        // with several files open, a new file gets a tab of its own and nothing is lost
        if p == Pending::New && !self.embedded {
            return self.new_doc();
        }
        if p == Pending::Open && !self.embedded {
            return self.run_pending(p, cx);
        }
        if p == Pending::Complete {
            return self.ask_complete(cx);
        }
        if p == Pending::CloseTab && !self.buf.is_dirty() {
            return self.close_doc(self.active);
        }
        if p == Pending::Quit && self.any_dirty() && !self.buf.is_dirty() {
            self.modal = Some(Modal::Confirm(p, "Another open file has unsaved changes. Close the editor and discard them?".into()));
            return;
        }
        if self.buf.is_dirty() && p != Pending::Open {
            let what = match p {
                Pending::New => "Start a new file and discard unsaved changes?",
                Pending::CloseTab => "Close this file and discard its unsaved changes?",
                _ => "Close the editor and discard unsaved changes?",
            };
            self.modal = Some(Modal::Confirm(p, what.into()));
            return;
        }
        self.run_pending(p, cx);
    }

    fn run_pending(&mut self, p: Pending, cx: &AppCx) {
        match p {
            Pending::New => self.new_file(),
            Pending::Open => {
                let dir = self.dir.clone();
                self.open_dialog(cx, dir);
            }
            Pending::Quit => {}
            Pending::CloseTab => self.close_doc(self.active),
            Pending::Complete => self.start_complete(cx),
        }
    }

    /// F6: before anything is sent, say where the lines would go and wait for a yes.
    fn ask_complete(&mut self, cx: &AppCx) {
        if self.completing.is_some() {
            return self.fail("The assistant is already writing; wait for it (Esc stops waiting).");
        }
        if !self.writable {
            return self.fail("This file is read-only for your account, so there is nothing to continue.");
        }
        let dest = match super::browser_assist::destination(cx) {
            Ok(d) => d,
            Err(e) => return self.fail(e),
        };
        let (before, after) = self.around_cursor();
        let lines = before.lines().count() + after.lines().count();
        self.modal = Some(Modal::Confirm(Pending::Complete, format!("Send {lines} lines to {dest} to continue at the cursor?")));
    }

    /// The text up to the cursor and from it, cut to what a model needs to see.
    fn around_cursor(&self) -> (String, String) {
        let (line, col) = self.buf.cursor;
        let n = self.buf.line_count();
        let cur = self.buf.line(line.min(n.saturating_sub(1)));
        let split = cur.char_indices().nth(col).map_or(cur.len(), |(i, _)| i);
        let first = line.saturating_sub(150);
        let mut before: Vec<String> = (first..line.min(n)).map(|i| self.buf.line(i).to_string()).collect();
        before.push(cur[..split].to_string());
        let mut after = vec![cur[split..].to_string()];
        after.extend((line + 1..n.min(line + 41)).map(|i| self.buf.line(i).to_string()));
        (before.join("\n"), after.join("\n"))
    }

    fn start_complete(&mut self, cx: &AppCx) {
        let (before, after) = self.around_cursor();
        let name = self.path.as_ref().and_then(|p| p.file_name()).map_or("untitled".to_string(), |n| n.to_string_lossy().into_owned());
        let history = vec![
            iyakki_agent::chat::Message::system("You continue source files at the cursor. Reply with ONLY the text to insert at <CURSOR>: no explanation, no code fences, nothing repeated from before or after the cursor. If nothing fits, reply with nothing."),
            iyakki_agent::chat::Message::user(format!("File: {name}\n\n{before}<CURSOR>{after}")),
        ];
        let chat = cx.chat.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        cx.audit.record(&cx.session.user, "edit.complete", &name);
        std::thread::spawn(move || {
            let _ = tx.send(chat.complete(&history));
        });
        self.completing = Some(rx);
        self.complete_at = Some((before, after));
        self.ok("Asking the assistant… (Esc stops waiting)");
    }

    fn poll_complete(&mut self) {
        let Some(rx) = &self.completing else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the assistant stopped without an answer".into()),
        };
        self.completing = None;
        let Some((before, after)) = self.complete_at.take() else { return };
        let text = match r {
            Ok(t) => t,
            Err(e) => return self.fail(format!("The assistant could not continue: {e}")),
        };
        // models like to wrap code in fences even when told not to
        let mut body = text.trim_matches('\n').to_string();
        if body.starts_with("```") {
            let inner: Vec<&str> = body.lines().collect();
            if inner.len() >= 2 && inner.last().is_some_and(|l| l.trim() == "```") {
                body = inner[1..inner.len() - 1].join("\n");
            }
        }
        if body.trim().is_empty() {
            return self.fail("The assistant had nothing to add here.");
        }
        let body: String = body.chars().take(8000).collect();
        // the file must not have changed meanwhile, or the splice would land in the wrong place
        let now = self.buf.text();
        if now != format!("{before}{after}") && now.trim_end() != format!("{before}{after}").trim_end() {
            return self.fail("The file changed while the assistant was writing, so its answer was dropped.");
        }
        let new_text = format!("{before}{body}{after}");
        let reply = format!("```file\n{}\n```", new_text.trim_end_matches('\n'));
        match self.propose(&reply) {
            Ok(Some(_)) => {}
            Ok(None) => self.fail("The assistant's answer could not be used."),
            Err(e) => self.fail(e),
        }
    }

    fn modal_event(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let Some(modal) = self.modal.take() else { return Flow::Stay };
        match modal {
            Modal::Open { mut picker, dir } => match picker.handle_event(ev) {
                PickEvent::Cancel => {}
                PickEvent::None => self.modal = Some(Modal::Open { picker, dir }),
                PickEvent::Chosen(item) => {
                    if item == "Type a path…" {
                        let f = Form::new("Open file").path("File path", &format!("{}/", dir.display()), "an absolute path you may read", crate::filepicker::PathKind::File, &[]);
                        self.modal = Some(Modal::PathForm(f, false));
                    } else if item == "../" {
                        let parent = dir.parent().map_or(dir.clone(), Path::to_path_buf);
                        self.open_dialog(cx, parent);
                    } else if let Some(d) = item.strip_suffix('/') {
                        self.open_dialog(cx, dir.join(d));
                    } else {
                        if self.buf.is_dirty() && self.embedded {
                            self.modal = Some(Modal::Confirm(Pending::Open, format!("Discard unsaved changes and open {item}?")));
                            self.parked_open = Some(dir.join(&item));
                        } else {
                            let _ = self.open_path(cx, &dir.join(&item).to_string_lossy());
                        }
                    }
                }
            },
            Modal::PathForm(mut f, save_as) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => {
                    let path = f.value(0);
                    if save_as {
                        let real = iyakki_agent::scope::resolve(Path::new("/"), path.trim());
                        match real {
                            Ok(p) if p.exists() && self.path.as_ref() != Some(&p) => self.modal = Some(Modal::Overwrite(p)),
                            Ok(_) => match self.save_to(cx, &path) {
                                Ok(()) => {}
                                Err(e) => {
                                    f.error = Some(e);
                                    self.modal = Some(Modal::PathForm(f, save_as));
                                }
                            },
                            Err(e) => {
                                f.error = Some(e);
                                self.modal = Some(Modal::PathForm(f, save_as));
                            }
                        }
                    } else if let Err(e) = self.open_path(cx, &path) {
                        f.error = Some(e);
                        self.modal = Some(Modal::PathForm(f, save_as));
                    }
                }
                _ => self.modal = Some(Modal::PathForm(f, save_as)),
            },
            Modal::Confirm(p, text) => match ev {
                Event::Key(Key::Char('y')) => {
                    if p == Pending::Quit {
                        return Flow::Back;
                    }
                    if p == Pending::Open {
                        if let Some(path) = self.parked_open.take() {
                            let _ = self.open_path(cx, &path.to_string_lossy());
                        }
                    } else {
                        self.run_pending(p, cx);
                    }
                }
                Event::Key(Key::Esc) | Event::Key(Key::Char(_)) => {}
                _ => self.modal = Some(Modal::Confirm(p, text)),
            },
            Modal::Overwrite(p) => match ev {
                Event::Key(Key::Char('y')) => {
                    if let Err(e) = self.save_to(cx, &p.to_string_lossy()) {
                        self.fail(e);
                    }
                }
                Event::Key(Key::Esc) | Event::Key(Key::Char(_)) => {}
                _ => self.modal = Some(Modal::Overwrite(p)),
            },
        }
        Flow::Stay
    }

    /// A press in the text (`dragging` false), or the same press repeated while the pointer moves with the button held:
    /// the caret goes there, a drag extends the selection (and scrolls when the pointer leaves the text), a double click
    /// selects the word and a triple click the line.
    fn click_text(&mut self, x: f32, y: f32) {
        if self.geo.lh <= 0.0 || self.geo.cw <= 0.0 {
            return;
        }
        let dragging = crate::ui::dragging();
        if dragging {
            // past the top or the bottom edge: scroll that way
            let rows = self.rows_visible.max(1) as f32;
            if y < 0.0 {
                self.scroll = self.scroll.saturating_sub(1);
            } else if y > rows * self.geo.lh {
                self.scroll = (self.scroll + 1).min(self.buf.line_count().saturating_sub(1));
            }
        }
        let line = (self.scroll + (y / self.geo.lh).max(0.0) as usize).min(self.buf.line_count() - 1);
        let dcol = self.hscroll + (x / self.geo.cw).round().max(0.0) as usize;
        let (_, map) = expand(self.buf.line(line));
        let col = char_at_display(&map, dcol);
        if dragging {
            self.buf.set_cursor((line, col), true);
        } else {
            match crate::ui::click_count() {
                2 => {
                    let chars: Vec<char> = self.buf.line(line).chars().collect();
                    let word = |c: char| c.is_alphanumeric() || c == '_';
                    let at = col.min(chars.len().saturating_sub(1));
                    if chars.is_empty() {
                        self.buf.set_cursor((line, 0), false);
                    } else {
                        let class = |c: char| if word(c) { 0 } else if c.is_whitespace() { 1 } else { 2 };
                        let k = class(chars[at]);
                        let (mut a, mut b) = (at, at + 1);
                        while a > 0 && class(chars[a - 1]) == k {
                            a -= 1;
                        }
                        while b < chars.len() && class(chars[b]) == k {
                            b += 1;
                        }
                        self.buf.select_range((line, a), (line, b));
                    }
                }
                3 => {
                    let end = self.buf.line(line).chars().count();
                    if line + 1 < self.buf.line_count() {
                        self.buf.select_range((line, 0), (line + 1, 0));
                    } else {
                        self.buf.select_range((line, 0), (line, end));
                    }
                }
                _ => self.buf.set_cursor((line, col), false),
            }
        }
        self.follow = false;
    }

    fn ensure_visible(&mut self) {
        let rows = self.rows_visible.max(1);
        let (line, col) = self.buf.cursor;
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
        let (_, map) = expand(self.buf.line(line));
        let d = map[col.min(map.len() - 1)];
        let cols = self.cols_visible.max(8);
        if d < self.hscroll {
            self.hscroll = d;
        } else if d + 2 >= self.hscroll + cols {
            self.hscroll = d + 3 - cols;
        }
        self.scroll = self.scroll.min(self.buf.line_count().saturating_sub(1));
    }

    // ------------------------------------------------------------------ drawing

    fn text_color_runs(&mut self, line_idx: usize, chars: usize, map: &[usize]) -> Vec<(usize, usize, [u8; 4])> {
        // runs in display columns: (start, end, colour); gaps are plain text
        let spans = self.hl.line_spans(self.buf.lines(), line_idx);
        let total = map[chars];
        let mut runs = Vec::new();
        let mut pos = 0;
        for sp in spans {
            let (s, e) = (map[sp.start.min(chars)], map[sp.end.min(chars)]);
            if s > pos {
                runs.push((pos, s, TEXT));
            }
            if e > s {
                runs.push((s, e, kind_color(sp.kind)));
            }
            pos = pos.max(e);
        }
        if pos < total {
            runs.push((pos, total, TEXT));
        }
        runs
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        if let Some(l) = self.buf.take_dirty() {
            self.hl.invalidate(l);
        }
        let ide = self.ide;
        let m = super::ide_chrome::cur();
        let size = if ide { 14.5 * s } else { 15.0 * s };
        let (lh, cw) = (g.line_height(size, Face::Mono), g.text_width(size, "M", Face::Mono));
        self.geo = Geo { cw, lh };
        let (pad, bar_h) = if ide { (8.0 * s, m.status_h * s) } else { (14.0 * s, 28.0 * s) };
        let x = area.x + pad;
        let w = area.w - 2.0 * pad;
        // the files that are open, when there are several
        let tabs_h = if self.docs.len() > 1 && !self.embedded { if ide { m.tab_h * s } else { 32.0 * s } } else { 0.0 };
        if tabs_h > 0.0 {
            let titles = self.doc_titles();
            let n = titles.len();
            let tw = ((w - 4.0 * s) / n as f32).clamp_safe(if ide { 80.0 * s } else { 90.0 * s }, 240.0 * s);
            let mut tx = x;
            for (i, t) in titles.iter().enumerate() {
                let on = i == self.active;
                if ide {
                    // flat tabs: the open one is brighter with an underline; the cross shows on the open one and on hover
                    let r = R::new(tx, area.y + 1.0 * s, tw - 2.0 * s, tabs_h - 1.0 * s);
                    let hot = g.hovered(r);
                    if on {
                        g.rect(r.x, r.y, r.w, r.h, rgb(18, 24, 46));
                        g.rect(r.x, r.y + r.h - 2.0 * s, r.w, 2.0 * s.max(1.0), ACCENT);
                    } else if hot {
                        g.rect(r.x, r.y, r.w, r.h, rgb(15, 20, 38));
                    }
                    g.hit(r, Event::Tab(200 + i), "tab");
                    let fsz = m.font_ui * s;
                    let face = if on { Face::Bold } else { Face::Sans };
                    let label = g.fit(fsz, t, face, r.w - 30.0 * s);
                    g.text(r.x + 8.0 * s, r.y + (r.h - g.line_height(fsz, face)) / 2.0, fsz, if on { TEXT } else { super::ide_chrome::muted() }, &label, face);
                    let ch = (20.0 * s).max(20.0).min(r.h);
                    let cr = R::new(r.x + r.w - ch - 2.0 * s, r.y + (r.h - ch) / 2.0, ch, ch);
                    let chot = g.hovered(cr);
                    if chot {
                        g.rrect(cr.x, cr.y, cr.w, cr.h, 4.0 * s, rgb(52, 60, 96));
                    }
                    if on || hot || chot {
                        let xw = g.text_width(fsz, "×", Face::Sans);
                        g.text(cr.x + (cr.w - xw) / 2.0, cr.y + (cr.h - g.line_height(fsz, Face::Sans)) / 2.0, fsz, super::ide_chrome::muted(), "×", Face::Sans);
                    }
                    g.hit(cr, Event::Tab(300 + i), "close tab");
                    tx += tw;
                    continue;
                }
                let r = R::new(tx, area.y + 4.0 * s, tw - 4.0 * s, tabs_h - 4.0 * s);
                let hot = g.hovered(r);
                g.rrect(r.x, r.y, r.w, r.h + 8.0 * s, 9.0 * s, if on { crate::fx::over(rgb(18, 24, 46), [ACCENT[0], ACCENT[1], ACCENT[2], 40]) } else if hot { rgb(22, 29, 54) } else { rgb(14, 18, 34) });
                if on {
                    g.rrect_outline(r.x, r.y, r.w, r.h + 8.0 * s, 9.0 * s, 1.2 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 150]);
                    g.rect(r.x + 10.0 * s, r.y + r.h - 1.0 * s, r.w - 20.0 * s, 2.0 * s, ACCENT);
                }
                g.hit(r, Event::Tab(200 + i), "tab");
                let label = g.fit(13.0 * s, t, if on { Face::Bold } else { Face::Sans }, r.w - 40.0 * s);
                g.text(r.x + 10.0 * s, r.y + 6.0 * s, 13.0 * s, if on { TEXT } else { MUTED }, &label, if on { Face::Bold } else { Face::Sans });
                let cr = R::new(r.x + r.w - 24.0 * s, r.y + 3.0 * s, 20.0 * s, 20.0 * s);
                if g.hovered(cr) {
                    g.rrect(cr.x, cr.y, cr.w, cr.h, 6.0 * s, rgb(52, 60, 96));
                }
                g.hit(cr, Event::Tab(300 + i), "close tab");
                g.text(cr.x + 5.5 * s, cr.y + 1.0 * s, 14.0 * s, MUTED, "×", Face::Sans);
                tx += tw;
            }
        }
        if ide && tabs_h > 0.0 {
            super::ide_chrome::strip_line(g, area.x + 1.0, area.y + tabs_h, area.w - 2.0);
        }
        // toolbar
        let tool_h = if ide { super::ide_chrome::tool(s) } else { 0.0 };
        let ty = area.y + tabs_h + if ide { 3.0 * s } else { 6.0 * s };
        let mut bx = x;
        let embedded = self.embedded;
        let bounds = area;
        let mut btn = |g: &mut Gfx, label: &str, col: [u8; 4], key: Key, on: bool| {
            if ide {
                let bw = g.text_width(m.font_small * s + 1.0 * s, label, Face::Bold) + 16.0 * s;
                if bx + bw > x + w * 0.62 {
                    return;
                }
                let tip = match key {
                    Key::Ctrl(c) => format!("{label} (Ctrl+{})", c.to_ascii_uppercase()),
                    _ => label.to_string(),
                };
                super::ide_chrome::text_btn(g, bx, ty, s, label, col, on, if embedded { Event::Sub(1, key) } else { Event::Key(key) }, label, Some(&tip), bounds);
                bx += bw + 1.0 * s;
                return;
            }
            let bw = g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.7;
            if bx + bw > x + w * 0.62 {
                return;
            }
            if on {
                g.button(bx, ty, 13.0 * s, label, col, if embedded { Event::Sub(1, key) } else { Event::Key(key) });
            } else {
                g.button_off(bx, ty, 13.0 * s, label);
            }
            bx += bw + 6.0 * s;
        };
        let in_review = self.review.is_some();
        if self.md_capable() {
            // the switch between the rendered page and the text comes first: it is what a Markdown file is mostly about
            btn(g, if self.md_previewing() { "Edit" } else { "Preview" }, ACCENT, Key::Ctrl('e'), !in_review);
        }
        btn(g, "Open", ACCENT, Key::Ctrl('o'), !in_review);
        btn(g, "Save", ACCENT, Key::Ctrl('s'), !in_review);
        btn(g, "New", TEXT, Key::Ctrl('n'), !in_review);
        btn(g, "Undo", TEXT, Key::Ctrl('z'), !in_review && self.buf.can_undo());
        btn(g, "Redo", TEXT, Key::Ctrl('y'), !in_review && self.buf.can_redo());
        btn(g, "Find", TEXT, Key::Ctrl('f'), !in_review);
        if !embedded && self.path.as_ref().and_then(|p| p.extension()).is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "py" | "js" | "mjs" | "sh" | "wasm" | "wat")) {
            btn(g, "Run", OK, Key::F(5), !in_review && self.running.is_none());
        }
        if !embedded {
            btn(g, "Complete", ACCENT, Key::F(6), !in_review && self.writable && self.completing.is_none());
        }
        // file name, dirty marker and language on the right
        let name = self.path.as_ref().and_then(|p| p.file_name()).map_or("untitled".to_string(), |n| n.to_string_lossy().into_owned());
        let label = format!("{}{}", name, if self.buf.is_dirty() { " ●" } else { "" });
        let lang = self.hl.lang().name();
        if ide {
            let cs = s * 0.85;
            let chip_w = g.text_width(12.0 * cs, lang, Face::Bold) + 12.0 * cs * 1.4;
            let lw = crate::fx::chip(g, x + w - chip_w, ty + (tool_h - 12.0 * cs * 1.9) / 2.0, cs, lang, ACCENT, true);
            let fsz = m.font_ui * s;
            let max_label = (x + w - lw - bx - 16.0 * s).max(40.0 * s);
            let label = g.fit(fsz, &label, Face::Bold, max_label);
            let lbw = g.text_width(fsz, &label, Face::Bold);
            g.text(x + w - lw - 8.0 * s - lbw, ty + (tool_h - g.line_height(fsz, Face::Bold)) / 2.0, fsz, if self.buf.is_dirty() { rgb(255, 196, 94) } else { TEXT }, &label, Face::Bold);
        } else {
            let lw = g.text_width(13.0 * s, lang, Face::Bold) + 20.0 * s;
            crate::fx::chip(g, x + w - lw, ty + 4.0 * s, s, lang, ACCENT, true);
            let max_label = (x + w - lw - bx - 20.0 * s).max(40.0 * s);
            let label = g.fit(15.0 * s, &label, Face::Bold, max_label);
            let lbw = g.text_width(15.0 * s, &label, Face::Bold);
            g.text(x + w - lw - 10.0 * s - lbw, ty + 6.0 * s, 15.0 * s, if self.buf.is_dirty() { rgb(255, 196, 94) } else { TEXT }, &label, Face::Bold);
        }
        // text area
        let find_h = if self.find.as_ref().is_some_and(|f| f.show_replace) { 84.0 * s } else if self.find.is_some() || self.goto.is_some() { 46.0 * s } else { 0.0 };
        let top_off = if ide { tool_h + 6.0 * s } else { 44.0 * s };
        let top = area.y + tabs_h + top_off;
        // the result of running the file takes some of the room under the text
        let out_h = if self.output.is_some() && !self.embedded { (area.h * 0.28).clamp_safe(80.0 * s, 240.0 * s) } else { 0.0 };
        let body = R::new(x, top, w, area.h - tabs_h - top_off - bar_h - find_h - 6.0 * s - out_h);
        self.rows_visible = ((body.h / lh) as usize).max(1);
        if self.review.is_some() {
            self.draw_review(g, body, s, size, lh, cw);
        } else if self.md_previewing() {
            let text = self.buf.text();
            self.md.draw(g, body, s, self.path.as_deref(), &text, ACCENT);
        } else {
            self.draw_buffer(g, body, s, size, lh, cw, tick, cx);
        }
        // what running the file printed
        if let Some(o) = &self.output {
            let r = R::new(x, body.y + body.h + 4.0 * s, w, out_h - 6.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, rgb(8, 11, 22));
            g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.5 * s, if o.ok { rgb(50, 130, 90) } else { rgb(150, 66, 80) });
            g.text(r.x + 12.0 * s, r.y + 6.0 * s, 13.0 * s, if o.ok { OK } else { ERR }, &format!("{} · {}", o.title, if o.ok { "finished" } else { "failed" }), Face::Bold);
            g.text(r.x + r.w - 150.0 * s, r.y + 6.0 * s, 12.0 * s, DIM, "Esc closes this", Face::Sans);
            let (osize, rows) = (13.0 * s, (((r.h - 30.0 * s) / (g.line_height(13.0 * s, Face::Mono) + 2.0 * s)) as usize).max(1));
            let olh = g.line_height(osize, Face::Mono) + 2.0 * s;
            for (i, l) in o.lines.iter().take(rows).enumerate() {
                let l = g.fit(osize, l, Face::Mono, r.w - 24.0 * s);
                g.text(r.x + 12.0 * s, r.y + 26.0 * s + i as f32 * olh, osize, TEXT, &l, Face::Mono);
            }
        }
        // find / goto bar
        let fy = body.y + body.h + out_h + 4.0 * s;
        if let Some(f) = &self.find {
            let fw = (w * 0.34).max(160.0 * s);
            f.query.render(g, R::new(x, fy, fw, 36.0 * s), !f.focus_replace, s, blink(tick), Face::Sans, F_QUERY);
            let mut bx = x + fw + 8.0 * s;
            let n = if f.query.value.is_empty() { 0 } else { self.buf.find_all(&f.query.value, self.case_sensitive).len() };
            for (label, key) in [("Next", Key::Down), ("Prev", Key::Up)] {
                bx += g.button(bx, fy + 3.0 * s, 13.0 * s, label, ACCENT, self.bev(key)) + 6.0 * s;
            }
            let aa = if self.case_sensitive { "Aa: on" } else { "Aa: off" };
            bx += g.button(bx, fy + 3.0 * s, 13.0 * s, aa, if self.case_sensitive { OK } else { MUTED }, self.bev(Key::Ctrl('e'))) + 6.0 * s;
            if !f.show_replace {
                bx += g.button(bx, fy + 3.0 * s, 13.0 * s, "Replace…", TEXT, self.bev(Key::Ctrl('h'))) + 6.0 * s;
            }
            g.button(bx, fy + 3.0 * s, 13.0 * s, "Close", MUTED, self.bev(Key::Esc));
            let count = if f.query.value.is_empty() { String::new() } else { format!("{n} match{}", if n == 1 { "" } else { "es" }) };
            g.text(x + w - 110.0 * s, fy + 10.0 * s, 13.0 * s, MUTED, &count, Face::Sans);
            if f.show_replace {
                f.replace.render(g, R::new(x, fy + 40.0 * s, fw, 36.0 * s), f.focus_replace, s, blink(tick), Face::Sans, F_REPLACE);
                let mut rx = x + fw + 8.0 * s;
                rx += g.button(rx, fy + 43.0 * s, 13.0 * s, "Replace", ACCENT, self.bev(Key::Enter)) + 6.0 * s;
                g.button(rx, fy + 43.0 * s, 13.0 * s, "Replace all", ACCENT, self.bev(Key::Ctrl('r')));
            }
        } else if let Some(gt) = &self.goto {
            g.text(x, fy + 10.0 * s, 14.0 * s, MUTED, "Go to line", Face::Sans);
            gt.render(g, R::new(x + 100.0 * s, fy, 140.0 * s, 36.0 * s), true, s, blink(tick), Face::Sans, F_GOTO);
            g.button(x + 252.0 * s, fy + 3.0 * s, 13.0 * s, "Go", ACCENT, self.bev(Key::Enter));
            g.button(x + 300.0 * s, fy + 3.0 * s, 13.0 * s, "Close", MUTED, self.bev(Key::Esc));
        }
        // status bar
        let sy = area.y + area.h - bar_h;
        let fs = if ide { m.font_small * s } else { 13.0 * s };
        let tyo = if ide { (bar_h - g.line_height(fs, Face::Sans)) / 2.0 } else { 6.0 * s };
        let dimc = if ide { super::ide_chrome::dim() } else { MUTED };
        g.gradient_rect(area.x, sy, area.w, bar_h, (0.0, sy), (0.0, sy + bar_h), &[(0.0, rgb(17, 22, 42)), (1.0, rgb(10, 13, 26))]);
        g.rect(area.x, sy, area.w, 1.0, [ACCENT[0], ACCENT[1], ACCENT[2], 90]);
        if self.running.is_some() {
            g.text(x + w * 0.45, sy + tyo, fs, rgb(255, 196, 94), &format!("running{}", ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
        }
        let (l, c) = self.buf.cursor;
        let (_, map) = expand(self.buf.line(l));
        let pos = format!("Ln {}, Col {}  ·  {} lines  ·  {}  ·  {}", l + 1, map[c.min(map.len() - 1)] + 1, self.buf.line_count(), if self.buf.uses_crlf() { "CRLF" } else { "LF" }, if self.writable { "read-write" } else { "read-only" });
        g.text(x, sy + tyo, fs, dimc, &pos, Face::Sans);
        if let Some((m, ok)) = &self.status {
            let pw = g.text_width(fs, &pos, Face::Sans);
            let room = (w - pw - 24.0 * s).max(40.0 * s);
            let m = g.fit(fs, m, Face::Sans, room);
            let mw = g.text_width(fs, &m, Face::Sans);
            g.text(x + w - mw, sy + tyo, fs, if *ok { OK } else { ERR }, &m, Face::Sans);
        }
        // modals
        match &self.modal {
            Some(Modal::Open { picker, .. }) => picker.render(g, area, s, ACCENT),
            Some(Modal::PathForm(f, _)) => f.render(g, area, s, ACCENT, blink(tick)),
            Some(Modal::Confirm(Pending::Complete, t)) => render_confirm(g, area, s, t, "Send"),
            Some(Modal::Confirm(_, t)) => render_confirm(g, area, s, t, "Discard"),
            Some(Modal::Overwrite(p)) => render_confirm(g, area, s, &format!("Overwrite {}?", p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())), "Overwrite"),
            None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_buffer(&mut self, g: &mut Gfx, body: R, s: f32, size: f32, lh: f32, cw: f32, tick: u64, _cx: &AppCx) {
        g.rrect(body.x, body.y, body.w, body.h, 10.0 * s, rgb(12, 15, 28));
        let digits = self.buf.line_count().to_string().len().max(3);
        let gutter = digits as f32 * cw + 22.0 * s;
        let text_x = body.x + gutter;
        let text_w = body.w - gutter - 8.0 * s;
        self.cols_visible = ((text_w / cw) as usize).max(8);
        if self.follow {
            self.ensure_visible();
        }
        self.scroll = self.scroll.min(self.buf.line_count().saturating_sub(1));
        g.rect(body.x, body.y, gutter - 6.0 * s, body.h, rgb(16, 20, 36));
        g.rrect_outline(body.x, body.y, body.w, body.h, 10.0 * s, 1.0, [ACCENT[0], ACCENT[1], ACCENT[2], 46]);
        g.hit(R::new(text_x, body.y, text_w + 8.0 * s, body.h), Event::Area(AREA_TEXT), "editor");
        let sel = self.buf.selection();
        let cur = self.buf.cursor;
        let n = self.buf.line_count();
        // backgrounds first (current line, selection) so glyphs that hang below the baseline are never painted over
        for row in 0..self.rows_visible {
            let li = self.scroll + row;
            let y = body.y + row as f32 * lh;
            if li >= n || y + lh > body.y + body.h {
                break;
            }
            let (chars, map) = expand(self.buf.line(li));
            let nchars = self.buf.line(li).chars().count();
            if li == cur.0 && super::caret_allowed() {
                g.rect(text_x - 4.0 * s, y, text_w + 8.0 * s, lh, rgb(22, 28, 50));
            }
            if let Some((a, b)) = sel {
                if li >= a.0 && li <= b.0 {
                    let from = if li == a.0 { map[a.1.min(nchars)] } else { 0 };
                    let to = if li == b.0 { map[b.1.min(nchars)] } else { chars.len() + 1 };
                    let (f2, t2) = (from.max(self.hscroll), to.min(self.hscroll + self.cols_visible + 1));
                    if t2 > f2 {
                        g.rect(text_x + (f2 - self.hscroll) as f32 * cw, y, (t2 - f2) as f32 * cw, lh, rgb(48, 74, 134));
                    }
                }
            }
        }
        // the bracket next to the caret and its partner
        if let Some((p, q)) = self.buf.match_bracket().filter(|_| super::caret_allowed()) {
            for (l, c) in [p, q] {
                if l >= self.scroll && l < self.scroll + self.rows_visible && l < n {
                    let (_, map) = expand(self.buf.line(l));
                    let d = map[c.min(map.len() - 1)];
                    if d >= self.hscroll && d <= self.hscroll + self.cols_visible {
                        let y = body.y + (l - self.scroll) as f32 * lh;
                        let bx = text_x + (d - self.hscroll) as f32 * cw;
                        g.rect(bx, y, cw, lh, rgb(54, 70, 112));
                        g.rrect_outline(bx, y + 1.0, cw, lh - 2.0, 2.0 * s, 1.0 * s, rgb(120, 160, 255));
                    }
                }
            }
        }
        for row in 0..self.rows_visible {
            let li = self.scroll + row;
            let y = body.y + row as f32 * lh;
            if li >= n || y + lh > body.y + body.h {
                break;
            }
            let line = self.buf.line(li).to_string();
            let (chars, map) = expand(&line);
            let nchars = line.chars().count();
            let num = (li + 1).to_string();
            let nw = g.text_width(size * 0.9, &num, Face::Mono);
            g.text(text_x - 14.0 * s - nw, y + 1.0 * s, size * 0.9, if li == cur.0 && super::caret_allowed() { MUTED } else { DIM }, &num, Face::Mono);
            // coloured runs
            if nchars <= 3000 {
                for (a, b, col) in self.text_color_runs(li, nchars, &map) {
                    let (f2, t2) = (a.max(self.hscroll), b.min(self.hscroll + self.cols_visible + 1));
                    if t2 > f2 {
                        let seg: String = chars[f2..t2.min(chars.len())].iter().collect();
                        g.text(text_x + (f2 - self.hscroll) as f32 * cw, y, size, col, &seg, Face::Mono);
                    }
                }
            } else {
                let seg: String = chars.iter().skip(self.hscroll).take(self.cols_visible + 1).collect();
                g.text(text_x, y, size, TEXT, &seg, Face::Mono);
            }
            if li == cur.0 && blink(tick) {
                crate::ui::note_caret();
                let d = map[cur.1.min(nchars)];
                if d >= self.hscroll {
                    g.rect(text_x + (d - self.hscroll) as f32 * cw, y + 1.0 * s, 2.0 * s, lh - 2.0 * s, rgb(230, 240, 255));
                }
            }
        }
        // vertical scrollbar
        if n > self.rows_visible {
            let th = (body.h * self.rows_visible as f32 / n as f32).max(24.0 * s);
            let ty = body.y + (body.h - th) * self.scroll as f32 / (n - self.rows_visible).max(1) as f32;
            g.rrect(body.x + body.w - 8.0 * s, ty, 5.0 * s, th, 2.5 * s, rgb(52, 62, 100));
        }
    }

    fn draw_review(&mut self, g: &mut Gfx, body: R, s: f32, size: f32, lh: f32, cw: f32) {
        let Some(r) = self.review.as_mut() else { return };
        g.rrect(body.x, body.y, body.w, body.h, 10.0 * s, rgb(12, 15, 28));
        // header: counts and the buttons that decide
        let head = 40.0 * s;
        let pending = r.decided.iter().filter(|d| d.is_none()).count();
        let title = format!("Assistant's suggestion · change {} of {} · {} undecided", r.cur + 1, r.hunks.len(), pending);
        g.rect(body.x, body.y, body.w, head, rgb(24, 30, 52));
        let buttons = [("Discard", ERR, Key::Esc), ("Apply", ACCENT, Key::Enter), ("Accept all", OK, Key::Char('a')), ("Reject", ERR, Key::Char('n')), ("Accept", OK, Key::Char('y'))];
        // as many buttons as fit beside the title; the title gets what is left
        let widths: Vec<f32> = buttons.iter().map(|(l, _, _)| g.text_width(13.0 * s, l, Face::Bold) + 13.0 * s * 1.7 + 6.0 * s).collect();
        let mut shown = buttons.len();
        while shown > 1 && widths[..shown].iter().sum::<f32>() > body.w - 150.0 * s {
            shown -= 1;
        }
        let used: f32 = widths[..shown].iter().sum();
        let t = g.fit(14.0 * s, &title, Face::Bold, (body.w - used - 28.0 * s).max(40.0 * s));
        g.text(body.x + 12.0 * s, body.y + 11.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
        let mut bx = body.x + body.w - 8.0 * s;
        for (i, (label, col, key)) in buttons.iter().take(shown).enumerate() {
            bx -= widths[i];
            g.button(bx + 6.0 * s, body.y + 5.0 * s, 13.0 * s, label, *col, Event::Key(key.clone()));
        }
        let list = R::new(body.x, body.y + head, body.w, body.h - head);
        let rows = ((list.h / lh) as usize).max(1);
        self.rows_visible = rows;
        r.scroll = r.scroll.min(r.rows.len().saturating_sub(1));
        g.hit(list, Event::Area(AREA_REVIEW), "review");
        let gutter = 6.0 * cw + 10.0 * s;
        let cols = (((list.w - gutter - 12.0 * s) / cw) as usize).max(8);
        for (i, row) in r.rows.iter().enumerate().skip(r.scroll).take(rows) {
            let y = list.y + (i - r.scroll) as f32 * lh;
            let current = row.hunk == Some(r.cur);
            let (bg, mark, col) = match (row.kind, row.rejected) {
                (RowKind::Add, _) => (Some(rgb(26, 66, 44)), "+", rgb(170, 240, 180)),
                (RowKind::Del, _) => (Some(rgb(78, 32, 38)), "−", rgb(255, 170, 170)),
                (RowKind::Same, true) => (None, "✗", DIM),
                (RowKind::Same, false) => (None, " ", TEXT),
            };
            if let Some(bg) = bg {
                g.rect(list.x, y, list.w, lh, bg);
            }
            if current && row.hunk.is_some() {
                g.rect(list.x, y, 4.0 * s, lh, ACCENT);
            }
            let num = row.lineno.map_or(String::new(), |n| n.to_string());
            let nw = g.text_width(size * 0.9, &num, Face::Mono);
            g.text(list.x + gutter - 14.0 * s - nw, y + 1.0 * s, size * 0.9, DIM, &num, Face::Mono);
            g.text(list.x + gutter - 10.0 * s, y, size, col, mark, Face::Mono);
            let (chars, _) = expand(&row.text);
            let seg: String = chars.iter().take(cols).collect();
            g.text(list.x + gutter + cw, y, size, if row.rejected { DIM } else { col }, &seg, Face::Mono);
        }
        if !r.explanation.is_empty() {
            // the assistant's note, one line, under the list header
            let note = g.fit(13.0 * s, &r.explanation.replace('\n', " "), Face::Sans, body.w - 24.0 * s);
            g.text(body.x + 12.0 * s, body.y + body.h - 20.0 * s, 13.0 * s, MUTED, &note, Face::Sans);
        }
    }
}

impl App for EditApp {
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        if let Some(u) = self.md.take_clicked() {
            super::browser::request_open(&u);
            return Some(("browser".into(), u));
        }
        iyakki_agent::sharedfolder::take_share_ask().map(|f| ("settings".into(), format!("share:{f}")))
    }

    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        use crate::ctxmenu::{Item, COPY, CUT, PASTE, SELECT_ALL};
        let sel = self.buf.selection().is_some();
        let mut v = vec![Item::new(CUT, "Cut", "Ctrl+X").enabled(sel), Item::new(COPY, "Copy", "Ctrl+C").enabled(sel), Item::new(PASTE, "Paste", "Ctrl+V").needs_clip(), Item::new(SELECT_ALL, "Select all", "Ctrl+A")];
        if self.hl.lang().line_comment().is_some() {
            v.push(Item::separator());
            v.push(Item::new("comment", "Comment line", "Ctrl+L"));
        }
        if self.path.as_ref().is_some_and(|p| p.parent().is_some_and(|d| d.is_dir())) {
            v.push(Item::separator());
            v.push(Item::new("share", "Share this file's folder…", ""));
        }
        if iyakki_agent::hostfiles::enabled() {
            v.push(Item::separator());
            v.push(Item::new("hostsave", "Save a copy on this computer…", "Ctrl+Shift+H"));
        }
        v
    }

    fn context_pick(&mut self, id: &str, cx: &AppCx) -> Flow {
        if id == "comment" {
            return self.handle(&Event::Key(Key::Ctrl('l')), cx);
        }
        if id == "share" {
            if let Some(d) = self.path.as_ref().and_then(|p| p.parent()) {
                iyakki_agent::sharedfolder::ask_share(&d.to_string_lossy());
            }
            return Flow::Stay;
        }
        if id == "hostsave" {
            return self.handle(&Event::Key(Key::Ctrl('H')), cx);
        }
        Flow::Stay
    }

    fn title(&self) -> &'static str {
        "Edit"
    }
    fn icon(&self) -> Icon {
        Icon::Edit
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.review.is_some() {
            "y accept · n reject · a accept all · Enter apply · Esc discard".into()
        } else {
            "Ctrl+S save · Ctrl+O open · Ctrl+N new tab · Ctrl+W close tab · Ctrl+F find · Ctrl+L comment · F5 run · Ctrl+Z undo · Esc back".into()
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.poll_run();
            self.poll_complete();
            return Flow::Stay;
        }
        if self.completing.is_some() && matches!(ev, Event::Key(Key::Esc)) {
            self.completing = None;
            self.complete_at = None;
            self.ok("Stopped waiting for the assistant.");
            return Flow::Stay;
        }
        if self.modal.is_some() {
            return self.modal_event(ev, cx);
        }
        if self.review.is_some() {
            match ev {
                Event::Key(k) => self.review_key(k, cx),
                Event::Scroll(d) => {
                    if let Some(r) = self.review.as_mut() {
                        r.scroll = (r.scroll as i32 - d * 3).clamp_safe(0, r.rows.len().saturating_sub(1) as i32) as usize;
                    }
                }
                Event::AreaAt(AREA_REVIEW, _, y) => {
                    let lh = self.geo.lh.max(1.0);
                    if let Some(r) = self.review.as_mut() {
                        let i = r.scroll + (y / lh) as usize;
                        if let Some(h) = r.rows.get(i).and_then(|row| row.hunk) {
                            r.cur = h;
                        }
                    }
                }
                _ => {}
            }
            return Flow::Stay;
        }
        // the find / go-to bars take typing while open
        if self.find.is_some() {
            match ev {
                Event::Key(Key::Ctrl('e')) => self.case_sensitive = !self.case_sensitive,
                Event::Key(Key::Ctrl('r')) => self.replace_all(),
                Event::Key(Key::Ctrl('f')) | Event::Key(Key::Ctrl('s')) | Event::Key(Key::Ctrl('z')) | Event::Key(Key::Ctrl('y')) | Event::Key(Key::Ctrl('o')) | Event::Key(Key::Ctrl('n')) | Event::Key(Key::Ctrl('g')) => {}
                Event::Key(k) => {
                    self.find_key(k);
                    return Flow::Stay;
                }
                Event::Paste(t) => {
                    if let Some(f) = self.find.as_mut() {
                        if f.focus_replace { f.replace.paste(t) } else { f.query.paste(t) };
                    }
                    return Flow::Stay;
                }
                Event::FocusAt(id, x) => {
                    if let Some(f) = self.find.as_mut() {
                        match *id {
                            F_QUERY => {
                                f.focus_replace = false;
                                f.query.click(*x);
                            }
                            F_REPLACE => {
                                f.focus_replace = true;
                                f.replace.click(*x);
                            }
                            _ => {}
                        }
                    }
                    return Flow::Stay;
                }
                _ => {}
            }
        }
        if self.goto.is_some() {
            match ev {
                Event::Key(k) => {
                    self.goto_key(k);
                    return Flow::Stay;
                }
                Event::Paste(t) => {
                    if let Some(g) = self.goto.as_mut() {
                        g.paste(&t.chars().filter(char::is_ascii_digit).collect::<String>());
                    }
                    return Flow::Stay;
                }
                Event::FocusAt(F_GOTO, x) => {
                    if let Some(g) = self.goto.as_mut() {
                        g.click(*x);
                    }
                    return Flow::Stay;
                }
                _ => {}
            }
        }
        if self.md_previewing() {
            match ev {
                Event::Tab(n) if *n >= super::edit_preview::LINK_BASE => {
                    self.md.click_link(*n - super::edit_preview::LINK_BASE);
                    return Flow::Stay;
                }
                Event::Key(Key::Ctrl('e') | Key::Char('e') | Key::Enter) => {
                    self.md_set_edit(true);
                    return Flow::Stay;
                }
                // find and go to line work on the text
                Event::Key(Key::Ctrl('f' | 'h' | 'g')) => self.md_set_edit(true),
                Event::Key(k) if self.md.scroll_key(k) => return Flow::Stay,
                Event::Scroll(d) => {
                    self.md.scroll_by(*d);
                    return Flow::Stay;
                }
                // nothing is typed into a picture
                Event::Paste(_) | Event::AreaAt(..) | Event::Key(Key::Char(_) | Key::Backspace | Key::Delete | Key::Tab | Key::BackTab) => return Flow::Stay,
                _ => {}
            }
        }
        match ev {
            Event::Key(Key::Ctrl('e')) if self.md_capable() => self.md_set_edit(false),
            Event::Key(Key::Ctrl('s')) => self.save(cx),
            // a copy of the open file on the computer the window runs on (window mode): the shell asks where
            Event::Key(Key::Ctrl('H')) => {
                if !iyakki_agent::hostfiles::enabled() {
                    self.fail(iyakki_agent::hostfiles::UNAVAILABLE);
                } else {
                    let name = self.path.as_ref().and_then(|p| p.file_name()).map_or("untitled.txt".to_string(), |n| n.to_string_lossy().into_owned());
                    self.host_save = Some((name, self.buf.text().into_bytes()));
                }
            }
            Event::Key(Key::Ctrl('w')) if !self.embedded => self.request(Pending::CloseTab, cx),
            Event::Key(Key::Nav { nav: Nav::PageDown, ctrl: true, .. }) if !self.embedded => self.step_doc(true),
            Event::Key(Key::Nav { nav: Nav::PageUp, ctrl: true, .. }) if !self.embedded => self.step_doc(false),
            Event::Tab(n) if !self.embedded && *n >= 300 && *n < 400 => self.request_close_doc(*n - 300, cx),
            Event::Tab(n) if !self.embedded && *n >= 200 && *n < 300 => self.switch_doc(*n - 200),
            Event::Key(Key::F(5)) if !self.embedded => self.run_file(cx),
            Event::Key(Key::F(6)) if !self.embedded => self.request(Pending::Complete, cx),
            Event::Key(Key::Ctrl('l')) => match self.hl.lang().line_comment() {
                Some(prefix) => {
                    let r = self.buf.toggle_comment(prefix);
                    self.do_edit(r);
                }
                None => self.fail(format!("{} has no line comments.", self.hl.lang().name())),
            },
            Event::Key(Key::Ctrl('o')) => self.request(Pending::Open, cx),
            Event::Key(Key::Ctrl('n')) => self.request(Pending::New, cx),
            Event::Key(Key::Ctrl('z')) => {
                if self.buf.undo() {
                    self.after_edit();
                }
            }
            Event::Key(Key::Ctrl('y')) | Event::Key(Key::Ctrl('Z')) => {
                if self.buf.redo() {
                    self.after_edit();
                }
            }
            Event::Key(Key::Ctrl('f')) => self.start_find(false),
            Event::Key(Key::Ctrl('h')) => self.start_find(true),
            Event::Key(Key::Ctrl('g')) => {
                self.find = None;
                self.goto = Some(TextField::new("line number", false));
            }
            Event::Key(Key::Ctrl('a')) => self.buf.select_all(),
            Event::Key(Key::Ctrl('c')) => {
                if let Some(t) = self.buf.selected_text() {
                    crate::ui::clipboard::copy(&t);
                }
            }
            Event::Key(Key::Ctrl('x')) => match self.buf.cut() {
                Ok(Some(t)) => {
                    crate::ui::clipboard::copy(&t);
                    self.after_edit();
                }
                Ok(None) => {}
                Err(e) => self.fail(e.to_string()),
            },
            Event::Key(Key::Ctrl('k')) => {
                let r = self.buf.delete_line();
                self.do_edit(r);
            }
            Event::Key(Key::Esc) => {
                if self.find.is_some() || self.goto.is_some() {
                    self.find = None;
                    self.goto = None;
                } else if self.output.is_some() {
                    self.output = None;
                } else if self.buf.is_dirty() && !self.embedded {
                    self.modal = Some(Modal::Confirm(Pending::Quit, "Close the editor and discard unsaved changes?".into()));
                } else if self.any_dirty() && !self.embedded {
                    self.modal = Some(Modal::Confirm(Pending::Quit, "Another open file has unsaved changes. Close the editor and discard them?".into()));
                } else {
                    return Flow::Back;
                }
            }
            Event::Key(Key::Enter) => {
                let r = if matches!(self.hl.lang(), Lang::Plain | Lang::Markdown) { self.buf.newline() } else { self.buf.newline_pair() };
                self.do_edit(r);
            }
            Event::Key(Key::Backspace) => {
                let r = self.buf.backspace_pair();
                self.do_edit(r);
            }
            Event::Key(Key::Delete) => {
                let r = self.buf.delete();
                self.do_edit(r);
            }
            Event::Key(Key::Tab) => {
                let r = self.buf.indent();
                self.do_edit(r);
            }
            Event::Key(Key::BackTab) => {
                let r = self.buf.unindent();
                self.do_edit(r);
            }
            Event::Key(Key::Left) => self.nav(Nav::Left, false, false),
            Event::Key(Key::Right) => self.nav(Nav::Right, false, false),
            Event::Key(Key::Up) => self.nav(Nav::Up, false, false),
            Event::Key(Key::Down) => self.nav(Nav::Down, false, false),
            Event::Key(Key::Home) => self.nav(Nav::Home, false, false),
            Event::Key(Key::End) => self.nav(Nav::End, false, false),
            Event::Key(Key::PageUp) => self.nav(Nav::PageUp, false, false),
            Event::Key(Key::PageDown) => self.nav(Nav::PageDown, false, false),
            Event::Key(Key::Nav { nav, shift, ctrl }) => self.nav(*nav, *shift, *ctrl),
            Event::Key(Key::Char(c)) if !c.is_control() => {
                // code files pair brackets and quotes as they are typed; plain text and Markdown do not
                let r = if matches!(self.hl.lang(), Lang::Plain | Lang::Markdown) { self.buf.insert(&c.to_string()) } else { self.buf.type_char(*c) };
                self.last_typed = Some(std::time::Instant::now());
                self.do_edit(r);
            }
            Event::Paste(t) => {
                let r = self.buf.insert(t);
                self.do_edit(r);
            }
            Event::Scroll(d) => {
                self.follow = false;
                self.scroll = (self.scroll as i32 - d * 3).clamp_safe(0, self.buf.line_count().saturating_sub(1) as i32) as usize;
            }
            Event::AreaAt(AREA_TEXT, x, y) => self.click_text(*x, *y),
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_backend;
    use iyakki_agent::chat::Unconfigured;

    fn world(tag: &str) -> (AppCx, PathBuf) {
        let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-edit-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let (mut cx, _d) = cx_with_backend(tag, Unconfigured);
        cx.session.policy.fs_read = vec![home.display().to_string()];
        cx.session.policy.fs_write = vec![home.display().to_string()];
        (cx, home)
    }

    fn key(a: &mut EditApp, cx: &AppCx, k: Key) -> Flow {
        a.handle(&Event::Key(k), cx)
    }
    fn typ(a: &mut EditApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                key(a, cx, Key::Enter);
            } else {
                key(a, cx, Key::Char(c));
            }
        }
    }
    fn render(a: &mut EditApp, cx: &AppCx) -> Gfx {
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, cx, 0);
        g
    }
    fn app_with(cx: &AppCx, home: &Path, name: &str, text: &str) -> EditApp {
        let p = home.join(name);
        std::fs::write(&p, text).unwrap();
        let mut a = EditApp::new(cx);
        a.open_path(cx, &p.to_string_lossy()).unwrap();
        a
    }

    #[test]
    fn open_edit_and_save_a_file_inside_the_users_scope() {
        let (cx, home) = world("e1");
        let mut a = app_with(&cx, &home, "hello.rs", "fn main() {\n}\n");
        assert_eq!((a.lang(), a.is_dirty()), (Lang::Rust, false));
        key(&mut a, &cx, Key::Nav { nav: Nav::Down, shift: false, ctrl: false });
        key(&mut a, &cx, Key::Up);
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "println!(\"hi\");");
        assert!(a.is_dirty());
        assert_eq!(a.text(), "fn main() {\n    println!(\"hi\");\n}\n", "Enter after an opening brace indents");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(a.status().unwrap().1, "{:?}", a.status());
        assert!(!a.is_dirty());
        assert_eq!(std::fs::read_to_string(home.join("hello.rs")).unwrap(), "fn main() {\n    println!(\"hi\");\n}\n");
        assert!(!home.join(".hello.rs.iyakki-tmp").exists(), "no temp file is left behind");
        let log = cx.audit.recent(10);
        assert!(log.iter().any(|l| l.contains("edit.open")) && log.iter().any(|l| l.contains("edit.save")));
    }

    #[test]
    fn the_open_file_can_be_saved_as_a_copy_on_the_host_in_window_mode_only() {
        let _g = iyakki_agent::hostfiles::test_guard();
        let (cx, home) = world("e-host");
        let mut a = app_with(&cx, &home, "report.md", "# Report\n");
        iyakki_agent::hostfiles::set_enabled(false);
        key(&mut a, &cx, Key::Ctrl('H'));
        assert!(a.take_host_save().is_none() && a.status().is_some_and(|s| !s.1 && s.0.contains("desktop window")), "{:?}", a.status());
        iyakki_agent::hostfiles::set_enabled(true);
        key(&mut a, &cx, Key::Ctrl('H'));
        assert_eq!(a.take_host_save(), Some(("report.md".to_string(), b"# Report\n".to_vec())));
        assert!(a.context_items(10, 10).iter().any(|i| i.id == "hostsave"));
        iyakki_agent::hostfiles::set_enabled(false);
        assert!(!a.context_items(10, 10).iter().any(|i| i.id == "hostsave"));
    }

    #[test]
    fn scope_checks_apply_to_reading_writing_and_symlinks() {
        let (mut cx, home) = world("e2");
        let mut a = EditApp::new(&cx);
        assert!(a.open_path(&cx, "/etc/passwd").is_err());
        assert!(a.status().is_some_and(|s| !s.1 && (s.0.contains("permission denied") || s.0.contains("outside your home folder"))), "{:?}", a.status());
        // a symlink inside the home that points outside is checked at its destination
        std::os::unix::fs::symlink("/etc/passwd", home.join("link")).unwrap();
        assert!(a.open_path(&cx, &home.join("link").to_string_lossy()).is_err());
        // read-only scope: the file opens, but saving is refused and audited
        std::fs::write(home.join("ro.txt"), "data\n").unwrap();
        cx.session.policy.fs_write = vec![];
        let mut r = EditApp::new(&cx);
        r.open_path(&cx, &home.join("ro.txt").to_string_lossy()).unwrap();
        assert!(r.status().unwrap().0.contains("read-only"));
        typ(&mut r, &cx, "x");
        key(&mut r, &cx, Key::Ctrl('s'));
        assert!(r.status().is_some_and(|s| !s.1 && s.0.contains("permission denied")), "{:?}", r.status());
        assert_eq!(std::fs::read_to_string(home.join("ro.txt")).unwrap(), "data\n");
        assert!(r.is_dirty(), "unsaved work is not lost on a refused save");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("edit.save.denied")));
        // folders, binary files, invalid UTF-8 and huge files are refused with a reason
        std::fs::create_dir_all(home.join("dir")).unwrap();
        std::fs::write(home.join("bin"), [0u8, 1, 2, 0]).unwrap();
        std::fs::write(home.join("latin1"), [0xE9u8, 0xE8]).unwrap();
        std::fs::write(home.join("big"), vec![b'x'; iyakki_edit::buffer::MAX_BYTES + 1]).unwrap();
        for (f, why) in [("dir", "not a file"), ("bin", "binary"), ("latin1", "UTF-8"), ("big", "too large")] {
            let e = a.open_path(&cx, &home.join(f).to_string_lossy()).unwrap_err();
            assert!(e.contains(why), "{f}: {e}");
        }
    }

    #[test]
    fn typing_selection_clipboard_and_undo_through_keys() {
        let (cx, home) = world("e3");
        let mut a = app_with(&cx, &home, "t.txt", "alpha beta\ngamma\n");
        crate::ui::clipboard::clear();
        key(&mut a, &cx, Key::Nav { nav: Nav::Right, shift: true, ctrl: true });
        assert_eq!(a.buffer().selected_text().as_deref(), Some("alpha"), "Ctrl+Shift+Right selects a word");
        key(&mut a, &cx, Key::Ctrl('c'));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("alpha"));
        key(&mut a, &cx, Key::Nav { nav: Nav::Down, shift: true, ctrl: false });
        assert!(a.buffer().selected_text().unwrap().contains('\n'));
        assert_eq!(a.buffer().selected_text().as_deref(), Some("alpha beta\ngamma"));
        key(&mut a, &cx, Key::Ctrl('x'));
        assert_eq!(a.text(), "");
        a.handle(&Event::Paste("PASTED ".into()), &cx);
        assert_eq!(a.text(), "PASTED \n");
        key(&mut a, &cx, Key::Ctrl('z'));
        assert_eq!(a.text(), "", "undo the paste");
        key(&mut a, &cx, Key::Ctrl('z'));
        assert_eq!(a.text(), "alpha beta\ngamma\n", "undo the cut");
        key(&mut a, &cx, Key::Ctrl('y'));
        assert_eq!(a.text(), "", "redo");
        key(&mut a, &cx, Key::Ctrl('a'));
        typ(&mut a, &cx, "new");
        assert_eq!(a.text(), "new\n");
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::BackTab);
        key(&mut a, &cx, Key::Ctrl('k'));
        assert_eq!(a.text(), "", "Ctrl+K deleted the only line");
    }

    #[test]
    fn the_context_menu_has_cut_copy_paste_select_all_and_comment_line_by_what_applies() {
        use crate::apps::App;
        let _g = iyakki_agent::hostfiles::test_guard();
        iyakki_agent::hostfiles::set_enabled(false);
        crate::ui::clipboard::clear();
        let (cx, home) = world("e-ctx");
        let mut code = app_with(&cx, &home, "m.rs", "fn main() {}\n");
        let items = code.context_items(10, 10);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["cut", "copy", "paste", "selall", "", "comment", "", "share"], "the last entry shares the file's folder (shared folders)");
        assert!(!items[0].enabled && !items[1].enabled, "no selection yet");
        assert!(items[2].needs_clip && items[3].enabled);
        // after Select all, Cut and Copy are usable; "Comment line" runs the editor's own Ctrl+L
        key(&mut code, &cx, Key::Ctrl('a'));
        assert!(code.context_items(10, 10)[0].enabled);
        key(&mut code, &cx, Key::Left);
        assert!(matches!(code.context_pick("comment", &cx), Flow::Stay));
        assert!(code.text().starts_with("// fn main"), "{:?}", code.text());
        // plain text has no comments to offer
        let text = app_with(&cx, &home, "n.txt", "hello\n");
        let mut text = text;
        assert!(!text.context_items(1, 1).iter().any(|i| i.id == "comment"));
    }

    #[test]
    fn find_replace_and_go_to_line() {
        let (cx, home) = world("e4");
        let mut a = app_with(&cx, &home, "f.txt", "one two\nTwo three\ntwo\n");
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, "two");
        assert_eq!(a.buffer().selection(), Some(((0, 4), (0, 7))), "incremental search selects the first match");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.buffer().selection(), Some(((1, 0), (1, 3))), "case-insensitive by default");
        key(&mut a, &cx, Key::Up);
        assert_eq!(a.buffer().selection(), Some(((0, 4), (0, 7))));
        a.handle(&Event::Key(Key::Ctrl('e')), &cx);
        key(&mut a, &cx, Key::Down);
        assert_eq!(a.buffer().selection(), Some(((2, 0), (2, 3))), "case-sensitive skips “Two”");
        typ(&mut a, &cx, "");
        // replace: Tab to the replacement, then Ctrl+R replaces all
        key(&mut a, &cx, Key::Ctrl('h'));
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "2");
        a.handle(&Event::Key(Key::Ctrl('r')), &cx);
        assert_eq!(a.text(), "one 2\nTwo three\n2\n");
        assert!(a.status().unwrap().0.contains("Replaced 2"));
        key(&mut a, &cx, Key::Ctrl('z'));
        assert_eq!(a.text(), "one two\nTwo three\ntwo\n", "replace-all is one undo step");
        key(&mut a, &cx, Key::Esc);
        assert!(!render(&mut a, &cx).hits.iter().any(|h| h.label == "Replace all"), "Esc closes the find bar");
        // no match is reported
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, "zzz");
        key(&mut a, &cx, Key::Enter);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not found")));
        key(&mut a, &cx, Key::Esc);
        // go to line
        key(&mut a, &cx, Key::Ctrl('g'));
        typ(&mut a, &cx, "x2y");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.buffer().cursor.0, 1, "letters are ignored; 2 → line 2");
        key(&mut a, &cx, Key::Ctrl('g'));
        typ(&mut a, &cx, "99");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.buffer().cursor.0, 2, "beyond the end goes to the last line");
    }

    #[test]
    fn unsaved_work_is_protected_on_new_open_and_close() {
        let (cx, home) = world("e5");
        let mut a = app_with(&cx, &home, "a.txt", "keep\n");
        typ(&mut a, &cx, "X");
        // a new file has a tab of its own, so nothing is discarded and nothing is asked
        key(&mut a, &cx, Key::Ctrl('n'));
        assert!(!a.has_modal() && a.doc_count() == 2);
        key(&mut a, &cx, Key::Nav { nav: Nav::PageUp, shift: false, ctrl: true });
        assert_eq!(a.text(), "Xkeep\n");
        key(&mut a, &cx, Key::Esc);
        assert!(a.has_modal() && matches!(key(&mut a, &cx, Key::Char('n')), Flow::Stay), "closing asks; any other letter cancels");
        assert!(!a.has_modal());
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(key(&mut a, &cx, Key::Char('y')), Flow::Back), "y confirms leaving");
        // inside the chat (embedded) there are no tabs: starting a new file asks before it discards
        let mut e = app_with(&cx, &home, "b.txt", "keep\n");
        e.embedded = true;
        typ(&mut e, &cx, "X");
        key(&mut e, &cx, Key::Ctrl('n'));
        assert!(e.has_modal(), "asks before discarding");
        key(&mut e, &cx, Key::Esc);
        assert_eq!(e.text(), "Xkeep\n");
        key(&mut e, &cx, Key::Ctrl('n'));
        key(&mut e, &cx, Key::Char('y'));
        assert_eq!((e.text(), e.path().is_none(), e.doc_count()), (String::new(), true, 1));
        // a clean buffer closes at once
        let mut c = EditApp::new(&cx);
        assert!(matches!(key(&mut c, &cx, Key::Esc), Flow::Back));
    }

    #[test]
    fn the_open_dialog_browses_inside_the_scope_and_opens_files() {
        let (cx, home) = world("e6");
        std::fs::create_dir_all(home.join("src")).unwrap();
        std::fs::write(home.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        std::fs::write(home.join("README.md"), "# hi\n").unwrap();
        let mut a = EditApp::new(&cx);
        a.dir = home.clone();
        key(&mut a, &cx, Key::Ctrl('o'));
        assert!(a.has_modal());
        let items = |a: &EditApp| match &a.modal {
            Some(Modal::Open { picker, .. }) => picker.visible().into_iter().cloned().collect::<Vec<_>>(),
            _ => vec![],
        };
        assert_eq!(items(&a), vec!["Type a path…", "src/", "README.md"], "no ../ at the top of the scope");
        // filter then choose the folder
        typ(&mut a, &cx, "src");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(items(&a), vec!["Type a path…", "../", "lib.rs"]);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal());
        assert_eq!((a.lang(), a.text()), (Lang::Rust, "pub fn f() {}\n".to_string()));
        // typing a path
        key(&mut a, &cx, Key::Ctrl('o'));
        key(&mut a, &cx, Key::Enter); // "Type a path…"
        let p = home.join("README.md").display().to_string();
        for _ in 0..200 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, &p);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.text(), "# hi\n");
        assert_eq!(a.lang(), Lang::Markdown);
        // a wrong path keeps the dialog open with the reason
        key(&mut a, &cx, Key::Ctrl('o'));
        key(&mut a, &cx, Key::Enter);
        for _ in 0..200 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "/etc/passwd");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal());
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.text(), "# hi\n");
    }

    #[test]
    fn save_as_new_file_asks_before_overwriting() {
        let (cx, home) = world("e7");
        let mut a = EditApp::new(&cx);
        a.dir = home.clone();
        typ(&mut a, &cx, "fresh");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(a.has_modal(), "no path yet: asks for one");
        for _ in 0..300 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, &format!("{}/new.py", home.display()));
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal());
        assert_eq!(std::fs::read_to_string(home.join("new.py")).unwrap(), "fresh\n");
        assert_eq!(a.lang(), Lang::Python, "the language follows the new name");
        // saving a different buffer over an existing file asks first
        std::fs::write(home.join("other.txt"), "old\n").unwrap();
        let mut b = EditApp::new(&cx);
        b.dir = home.clone();
        typ(&mut b, &cx, "mine");
        key(&mut b, &cx, Key::Ctrl('s'));
        for _ in 0..300 {
            key(&mut b, &cx, Key::Backspace);
        }
        typ(&mut b, &cx, &format!("{}/other.txt", home.display()));
        key(&mut b, &cx, Key::Enter);
        assert!(matches!(b.modal, Some(Modal::Overwrite(_))));
        key(&mut b, &cx, Key::Esc);
        assert_eq!(std::fs::read_to_string(home.join("other.txt")).unwrap(), "old\n");
        key(&mut b, &cx, Key::Ctrl('s'));
        for _ in 0..300 {
            key(&mut b, &cx, Key::Backspace);
        }
        typ(&mut b, &cx, &format!("{}/other.txt", home.display()));
        key(&mut b, &cx, Key::Enter);
        key(&mut b, &cx, Key::Char('y'));
        assert_eq!(std::fs::read_to_string(home.join("other.txt")).unwrap(), "mine\n");
        // outside the write scope: refused and the form stays open
        let mut c = EditApp::new(&cx);
        typ(&mut c, &cx, "x");
        key(&mut c, &cx, Key::Ctrl('s'));
        for _ in 0..300 {
            key(&mut c, &cx, Key::Backspace);
        }
        typ(&mut c, &cx, "/tmp/iyakki-edit-should-not-exist");
        key(&mut c, &cx, Key::Enter);
        assert!(c.has_modal() && !Path::new("/tmp/iyakki-edit-should-not-exist").exists());
    }

    struct Completer(std::sync::Arc<std::sync::Mutex<Vec<String>>>, &'static str);
    impl iyakki_agent::chat::ChatBackend for Completer {
        fn complete(&self, history: &[iyakki_agent::chat::Message]) -> Result<String, String> {
            self.0.lock().unwrap().push(history.last().map(|m| m.content.clone()).unwrap_or_default());
            Ok(self.1.to_string())
        }
    }

    fn with_assistant(cx: &mut AppCx, reply: &'static str) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        cx.chat = std::sync::Arc::new(Completer(seen.clone(), reply));
        let mut p = iyakki_agent::providers::ProviderCfg::new("cloud", iyakki_agent::providers::Kind::OpenAi);
        p.base_url = Some("https://api.example.com/v1".into());
        p.key_ref = Some("vault:cloud-key".into());
        let mut st = cx.services.settings.lock().unwrap();
        st.upsert_provider(p).unwrap();
        st.set_chat(Some(iyakki_agent::settings::ModelChoice { provider: "cloud".into(), model: "big".into() })).unwrap();
        seen
    }

    fn wait_complete(a: &mut EditApp, cx: &AppCx) {
        for _ in 0..400 {
            a.handle(&Event::Tick, cx);
            if a.completing.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the assistant never answered");
    }

    #[test]
    fn f6_asks_first_then_the_assistants_continuation_arrives_as_a_reviewable_change() {
        let (mut cx, home) = world("ecomplete");
        let seen = with_assistant(&mut cx, "```rust\nprintln!(\"hi\");\n```");
        let mut a = app_with(&cx, &home, "m.rs", "fn main() {\n    \n}\n");
        a.buf.set_cursor((1, 4), false);
        key(&mut a, &cx, Key::F(6));
        assert!(a.has_modal() && seen.lock().unwrap().is_empty(), "nothing is sent before the yes");
        let g = render(&mut a, &cx);
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('y'))));
        key(&mut a, &cx, Key::Char('y'));
        wait_complete(&mut a, &cx);
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("fn main() {\n    <CURSOR>\n}"), "{:?}", sent[0]);
        assert!(a.review.is_some(), "{:?}", a.status);
        assert_eq!(a.buf.text(), "fn main() {\n    \n}\n", "the file is unchanged until the change is accepted");
        key(&mut a, &cx, Key::Char('a'));
        assert!(a.review.is_none(), "accept all applies");
        assert_eq!(a.buf.text(), "fn main() {\n    println!(\"hi\");\n}\n");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("edit.complete") && l.contains("m.rs")));
    }

    #[test]
    fn f6_without_an_assistant_says_so_and_esc_stops_waiting() {
        let (cx, home) = world("ecomplete2");
        let mut a = app_with(&cx, &home, "m.rs", "fn main() {}\n");
        key(&mut a, &cx, Key::F(6));
        assert!(!a.has_modal());
        assert!(a.status.as_ref().is_some_and(|(m, ok)| m.contains("No assistant") && !ok), "{:?}", a.status);
    }

    const SRC: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n    println!(\"{}\", a + b);\n}\n";

    fn blocks(pairs: &[(&str, &str)]) -> String {
        pairs.iter().map(|(s, r)| format!("<<<<<<< SEARCH\n{s}\n=======\n{r}\n>>>>>>> REPLACE\n")).collect()
    }

    #[test]
    fn assistant_proposals_are_reviewed_hunk_by_hunk_and_never_touch_the_disk() {
        let (cx, home) = world("e8");
        let mut a = app_with(&cx, &home, "m.rs", SRC);
        let reply = format!("I changed two things.\n{}", blocks(&[("    let a = 1;", "    let a = 10;"), ("    println!(\"{}\", a + b);", "    println!(\"{}\", a * b);")]));
        let note = a.propose(&reply).unwrap().unwrap();
        assert_eq!(note, "I changed two things.");
        assert!(a.in_review() && a.pending_hunks() == 2);
        assert_eq!(a.text(), SRC, "nothing changes until a hunk is accepted");
        assert!(!a.is_dirty());
        // accept the first, reject the second, apply
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.pending_hunks(), 1);
        key(&mut a, &cx, Key::Char('n'));
        key(&mut a, &cx, Key::Enter);
        assert!(!a.in_review());
        assert_eq!(a.text(), SRC.replace("let a = 1;", "let a = 10;"));
        assert!(a.is_dirty(), "applied but not saved");
        assert_eq!(std::fs::read_to_string(home.join("m.rs")).unwrap(), SRC, "the file on disk is untouched");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("edit.apply") && l.contains("1/2")));
        // the whole application is one undo step
        key(&mut a, &cx, Key::Ctrl('z'));
        assert_eq!(a.text(), SRC);
    }

    #[test]
    fn review_accept_all_discard_navigation_and_clicks() {
        let (cx, home) = world("e9");
        let mut a = app_with(&cx, &home, "m.rs", SRC);
        let reply = blocks(&[("    let a = 1;", "    let a = 10;"), ("    println!(\"{}\", a + b);", "    let c = 3;\n    println!(\"{}\", a + b + c);")]);
        a.propose(&reply).unwrap();
        // the review renders and its rows are clickable
        let g = render(&mut a, &cx);
        assert!(g.hits.iter().any(|h| h.label == "review") && g.hits.iter().any(|h| h.label == "Accept all"));
        key(&mut a, &cx, Key::Char(']'));
        assert_eq!(a.review.as_ref().unwrap().cur, 1);
        key(&mut a, &cx, Key::Char('['));
        assert_eq!(a.review.as_ref().unwrap().cur, 0);
        // clicking a changed line selects its hunk
        let lh = a.geo.lh;
        let row = a.review.as_ref().unwrap().rows.iter().position(|r| r.hunk == Some(1)).unwrap();
        a.handle(&Event::AreaAt(AREA_REVIEW, 50.0, (row - a.review.as_ref().unwrap().scroll) as f32 * lh + 2.0), &cx);
        assert_eq!(a.review.as_ref().unwrap().cur, 1);
        // other keys do not edit the file while reviewing
        typ(&mut a, &cx, "zzz");
        assert_eq!(a.text(), SRC);
        key(&mut a, &cx, Key::Char('a'));
        assert_eq!(a.text(), SRC.replace("let a = 1;", "let a = 10;").replace("    println!(\"{}\", a + b);", "    let c = 3;\n    println!(\"{}\", a + b + c);"));
        // discard
        let mut d = app_with(&cx, &home, "n.rs", SRC);
        d.propose(&reply).unwrap();
        key(&mut d, &cx, Key::Esc);
        assert!(!d.in_review() && d.text() == SRC && !d.is_dirty());
        assert!(d.status().unwrap().0.contains("Discarded"));
        // enter with nothing accepted changes nothing
        let mut e = app_with(&cx, &home, "o.rs", SRC);
        e.propose(&reply).unwrap();
        key(&mut e, &cx, Key::Enter);
        assert!(!e.in_review() && e.text() == SRC && !e.is_dirty());
    }

    #[test]
    fn bad_replies_are_explained_and_change_nothing() {
        let (cx, home) = world("e10");
        let mut a = app_with(&cx, &home, "m.rs", SRC);
        assert_eq!(a.propose("Sure! Use a for loop here.").unwrap(), None, "a plain answer is not an edit");
        assert_eq!(a.propose("```rust\nlet x = 1;\n```").unwrap(), None, "a code snippet is not a whole-file replacement");
        let e = a.propose(&blocks(&[("not in the file", "x")])).unwrap_err();
        assert!(e.contains("not in the file"), "{e}");
        let e = a.propose(&blocks(&[("    let a = 1;", "    let a = 1;")])).unwrap_err();
        assert!(e.contains("changes nothing"), "{e}");
        let e = a.propose("<<<<<<< SEARCH\nfoo\n").unwrap_err();
        assert!(e.contains("incomplete"));
        assert!(!a.in_review() && a.text() == SRC);
        // a whole-file replacement needs the `file` label and shows as one review
        a.propose("```file\nfn main() {}\n```").unwrap().unwrap();
        assert_eq!(a.pending_hunks(), 1);
        key(&mut a, &cx, Key::Char('a'));
        assert_eq!(a.text(), "fn main() {}\n");
    }

    #[test]
    fn saving_is_blocked_during_review_and_selection_info_feeds_the_assistant() {
        let (cx, home) = world("e11");
        let mut a = app_with(&cx, &home, "m.rs", SRC);
        a.buffer_mut().select_range((1, 0), (2, 14));
        assert_eq!(a.selection_info(), Some((2, 3, "    let a = 1;\n    let b = 2;".to_string())));
        a.buffer_mut().select_range((1, 0), (3, 0));
        assert_eq!(a.selection_info().map(|s| (s.0, s.1)), Some((2, 3)), "a selection ending at column 0 does not include that line");
        a.buffer_mut().clear_selection();
        assert_eq!(a.selection_info(), None);
        a.propose(&blocks(&[("    let a = 1;", "    let a = 9;")])).unwrap();
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Finish reviewing")));
    }

    #[test]
    fn clicks_place_the_caret_and_everything_renders() {
        let (cx, home) = world("e12");
        let mut a = app_with(&cx, &home, "c.rs", "fn main() {\n\tlet s = \"héllo\"; // note\n}\n");
        let g = render(&mut a, &cx);
        let area = g.hits.iter().find(|h| h.label == "editor").expect("the text area is clickable").rect;
        assert!(a.geo.cw > 0.0);
        // click on the second line, a few characters in (tab = 4 columns)
        let (cw, lh) = (a.geo.cw, a.geo.lh);
        a.handle(&Event::AreaAt(AREA_TEXT, cw * 8.0, lh * 1.0 + 3.0), &cx);
        assert_eq!(a.buffer().cursor, (1, 5), "display column 8 = after the tab and 4 characters");
        a.handle(&Event::AreaAt(AREA_TEXT, 5000.0, lh * 1.0 + 3.0), &cx);
        assert_eq!(a.buffer().cursor.1, "\tlet s = \"héllo\"; // note".chars().count(), "past the end goes to the end of the line");
        a.handle(&Event::AreaAt(AREA_TEXT, 0.0, 9999.0), &cx);
        assert_eq!(a.buffer().cursor.0, 2, "below the text goes to the last line");
        assert!(area.w > 100.0);
        // wheel scrolling
        a.handle(&Event::Scroll(-1), &cx);
        // every state draws without panicking, at several sizes
        for (w, h, s) in [(1100u32, 650u32, 1.0f32), (640, 480, 0.6), (1920, 1080, 1.5)] {
            let mut g = Gfx::new(w, h);
            for step in 0..6 {
                g.clear(rgb(0, 0, 0));
                match step {
                    1 => {
                        key(&mut a, &cx, Key::Ctrl('f'));
                    }
                    2 => {
                        key(&mut a, &cx, Key::Ctrl('h'));
                    }
                    3 => {
                        key(&mut a, &cx, Key::Esc);
                        key(&mut a, &cx, Key::Ctrl('g'));
                    }
                    4 => {
                        key(&mut a, &cx, Key::Esc);
                        key(&mut a, &cx, Key::Ctrl('o'));
                    }
                    5 => {
                        key(&mut a, &cx, Key::Esc);
                        let _ = a.propose(&blocks(&[("fn main() {", "fn main() { // entry")]));
                    }
                    _ => {}
                }
                a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, &cx, step);
            }
            key(&mut a, &cx, Key::Esc);
        }
    }

    #[test]
    fn highlighting_reaches_the_screen() {
        let (cx, home) = world("e13");
        let mut a = app_with(&cx, &home, "h.rs", "fn main() {}\n");
        let g = render(&mut a, &cx);
        // keyword colour pixels exist somewhere in the text area (the plain text colour is different)
        let kw = kind_color(TokenKind::Keyword);
        let px = g.rgba();
        let found = px.chunks(4).any(|p| (p[0] as i32 - kw[0] as i32).abs() < 12 && (p[1] as i32 - kw[1] as i32).abs() < 12 && (p[2] as i32 - kw[2] as i32).abs() < 12);
        assert!(found, "the keyword `fn` is drawn in the keyword colour");
    }

    #[test]
    fn display_columns_expand_tabs_and_map_back() {
        let (chars, map) = expand("a\tb");
        assert_eq!(chars.iter().collect::<String>(), "a   b");
        assert_eq!(map, vec![0, 1, 4, 5]);
        assert_eq!(char_at_display(&map, 0), 0);
        assert_eq!(char_at_display(&map, 2), 1, "inside the tab: the tab itself");
        assert_eq!(char_at_display(&map, 4), 2);
        assert_eq!(char_at_display(&map, 99), 3);
        assert_eq!(expand("\u{1}x").0.iter().collect::<String>(), "·x", "control characters are shown as dots");
    }

    // ------------------------------------------------------------------ tabs, code-editing helpers, mouse, running

    fn click_at(a: &mut EditApp, cx: &AppCx, line: usize, col: usize, clicks: u8, drag: bool) {
        let (lh, cw) = (a.geo.lh, a.geo.cw);
        crate::ui::set_click_count(clicks);
        crate::ui::set_dragging(drag);
        a.handle(&Event::AreaAt(AREA_TEXT, col as f32 * cw + cw * 0.2, (line as f32 - a.scroll as f32) * lh + lh / 2.0), cx);
        crate::ui::set_dragging(false);
        crate::ui::set_click_count(1);
    }

    #[test]
    fn several_files_open_in_tabs_that_keep_their_own_text_and_nothing_unsaved_is_lost() {
        let (cx, home) = world("etabs");
        let mut a = app_with(&cx, &home, "one.rs", "fn one() {}\n");
        let two = home.join("two.py");
        std::fs::write(&two, "print(2)\n").unwrap();
        a.open_path(&cx, &two.to_string_lossy()).unwrap();
        assert_eq!((a.doc_count(), a.active_doc()), (2, 1));
        assert_eq!(a.doc_titles(), vec!["one.rs".to_string(), "two.py".to_string()]);
        assert_eq!(a.lang(), Lang::Python);
        typ(&mut a, &cx, "# note ");
        assert_eq!(a.doc_titles()[1], "two.py ●", "a changed file is marked");
        key(&mut a, &cx, Key::Nav { nav: Nav::PageUp, shift: false, ctrl: true });
        assert_eq!((a.active_doc(), a.path().unwrap().file_name().unwrap().to_string_lossy().to_string()), (0, "one.rs".to_string()));
        assert_eq!(a.lang(), Lang::Rust);
        assert_eq!(a.text(), "fn one() {}\n");
        key(&mut a, &cx, Key::Nav { nav: Nav::PageDown, shift: false, ctrl: true });
        assert!(a.text().starts_with("# note "), "the other file's edit is still there");
        // opening a file that is open goes to it instead of opening it twice
        a.open_path(&cx, &home.join("one.rs").to_string_lossy()).unwrap();
        assert_eq!((a.doc_count(), a.active_doc()), (2, 0));
        // the strip is drawn with a hit region for every tab and its close mark
        let g = render(&mut a, &cx);
        assert_eq!(g.hits.iter().filter(|h| h.label == "tab").count(), 2);
        assert_eq!(g.hits.iter().filter(|h| h.label == "close tab").count(), 2);
        a.handle(&Event::Tab(201), &cx);
        assert_eq!(a.active_doc(), 1);
        // Esc leaves the editor only after asking, because the other tab is changed
        key(&mut a, &cx, Key::Nav { nav: Nav::PageUp, shift: false, ctrl: true });
        assert!(matches!(key(&mut a, &cx, Key::Esc), Flow::Stay));
        assert!(a.has_modal());
        key(&mut a, &cx, Key::Char('n'));
        // closing the changed tab asks, answering no keeps it
        key(&mut a, &cx, Key::Nav { nav: Nav::PageDown, shift: false, ctrl: true });
        key(&mut a, &cx, Key::Ctrl('w'));
        assert!(a.has_modal() && a.doc_count() == 2);
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(a.doc_count(), 2);
        key(&mut a, &cx, Key::Ctrl('w'));
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!((a.doc_count(), a.active_doc()), (1, 0));
        assert_eq!(a.text(), "fn one() {}\n");
        // a new file gets a tab of its own, with nothing to confirm
        key(&mut a, &cx, Key::Ctrl('n'));
        assert_eq!((a.doc_count(), a.active_doc(), a.text().as_str()), (2, 1, ""));
        // closing the last tab leaves an empty file rather than the editor
        key(&mut a, &cx, Key::Ctrl('w'));
        key(&mut a, &cx, Key::Ctrl('w'));
        assert_eq!((a.doc_count(), a.text().as_str()), (1, ""));
        assert!(a.path().is_none());
    }

    #[test]
    fn code_files_pair_brackets_and_quotes_highlight_the_partner_and_toggle_comments_but_text_files_do_not() {
        let (cx, home) = world("ecode");
        let mut a = app_with(&cx, &home, "x.rs", "");
        typ(&mut a, &cx, "fn f(a: u8) {\n");
        assert_eq!(a.buffer().line(0), "fn f(a: u8) {", "the parenthesis was typed over");
        assert_eq!(a.buffer().line(1), "    ", "Enter between the pair opens an indented line");
        assert_eq!(a.buffer().line(2), "}", "and the closing brace stays on a line of its own");
        typ(&mut a, &cx, "let s = \"hi\";");
        assert_eq!(a.buffer().line(1), "    let s = \"hi\";", "the quote pair was typed over: {:?}", a.buffer().line(1));
        // the caret next to a bracket marks its partner on screen
        key(&mut a, &cx, Key::Up);
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::Left);
        assert!(a.buffer().match_bracket().is_some());
        let plain = render(&mut a, &cx);
        assert!(plain.rgba().chunks(4).any(|p| p[0] == 120 && p[1] == 160 && p[2] == 255), "the outline of the matching bracket is drawn");
        // backspace between a pair takes both
        let mut b = app_with(&cx, &home, "y.rs", "");
        typ(&mut b, &cx, "(");
        assert_eq!(b.buffer().line(0), "()");
        key(&mut b, &cx, Key::Backspace);
        assert_eq!(b.buffer().line(0), "");
        // Ctrl+L comments the line with the language's own mark, and back
        let mut c = app_with(&cx, &home, "z.py", "x = 1\ny = 2\n");
        key(&mut c, &cx, Key::Ctrl('a'));
        key(&mut c, &cx, Key::Ctrl('l'));
        assert_eq!(c.text(), "# x = 1\n# y = 2\n");
        key(&mut c, &cx, Key::Ctrl('l'));
        assert_eq!(c.text(), "x = 1\ny = 2\n");
        // plain text has no pairs and no comments
        let mut t = app_with(&cx, &home, "n.txt", "");
        typ(&mut t, &cx, "(hi");
        assert_eq!(t.buffer().line(0), "(hi");
        key(&mut t, &cx, Key::Ctrl('l'));
        assert!(t.status().unwrap().0.contains("no line comments"));
    }

    #[test]
    fn the_mouse_drags_a_selection_picks_words_and_lines_and_scrolls_when_the_pointer_leaves() {
        let (cx, home) = world("emouse");
        let text: String = (0..80).map(|i| format!("line {i} alpha beta_gamma delta\n")).collect();
        let mut a = app_with(&cx, &home, "m.txt", &text);
        let _ = render(&mut a, &cx);
        click_at(&mut a, &cx, 2, 5, 1, false);
        assert_eq!(a.buffer().cursor, (2, 5));
        click_at(&mut a, &cx, 4, 9, 1, true);
        assert_eq!(a.selection_info().map(|(l0, l1, _)| (l0, l1)), Some((3, 5)));
        assert_eq!(a.buffer().selection(), Some(((2, 5), (4, 9))));
        click_at(&mut a, &cx, 10, 10, 2, false);
        assert_eq!(a.buffer().selected_text().as_deref(), Some("alpha"), "double click: a word");
        click_at(&mut a, &cx, 10, 17, 2, false);
        assert_eq!(a.buffer().selected_text().as_deref(), Some("beta_gamma"), "an underscore belongs to the word");
        click_at(&mut a, &cx, 10, 3, 3, false);
        assert_eq!(a.buffer().selected_text().as_deref(), Some("line 10 alpha beta_gamma delta\n"), "triple click: the line");
        // dragging below the text scrolls it
        click_at(&mut a, &cx, 1, 0, 1, false);
        let before = a.scroll;
        crate::ui::set_dragging(true);
        for _ in 0..5 {
            a.handle(&Event::AreaAt(AREA_TEXT, 30.0, 10_000.0), &cx);
        }
        crate::ui::set_dragging(false);
        assert!(a.scroll > before && a.buffer().selection().is_some());
    }

    #[test]
    fn f5_runs_webassembly_files_in_the_sandbox_and_says_why_other_files_cannot_run() {
        let (cx, home) = world("erun");
        let wat = "(module (func (export \"_start\")))";
        let mut a = app_with(&cx, &home, "t.wat", wat);
        key(&mut a, &cx, Key::F(5));
        assert!(a.running.is_some());
        for _ in 0..400 {
            a.handle(&Event::Tick, &cx);
            if a.running.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let out = a.output.as_ref().expect("a result");
        assert!(out.ok && out.lines == vec!["ok".to_string()], "{:?}", out.lines);
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("edit.run") && l.contains("t.wat")));
        let g = render(&mut a, &cx);
        assert!(g.hits.iter().any(|h| h.label == "Run"), "a Run button for runnable files");
        key(&mut a, &cx, Key::Esc);
        assert!(a.output.is_none(), "Esc closes the result first");
        // a Python file needs the container cluster
        let mut p = app_with(&cx, &home, "t.py", "print(1)\n");
        key(&mut p, &cx, Key::F(5));
        assert!(p.status().unwrap().0.contains("container cluster"), "{:?}", p.status());
        // an account without the full command-line permission may not run scripts
        let mut cx2 = cx.clone();
        cx2.session.policy.cli = iyakki_sec::policy::CliLevel::Restricted;
        let mut q = app_with(&cx2, &home, "u.py", "print(1)\n");
        key(&mut q, &cx2, Key::F(5));
        assert!(q.status().unwrap().0.contains("full command-line permission"));
        // other files and unsaved ones
        let mut r = app_with(&cx, &home, "notes.md", "x");
        key(&mut r, &cx, Key::F(5));
        assert!(r.status().unwrap().0.contains("Only .py"));
        let mut n = EditApp::new(&cx);
        key(&mut n, &cx, Key::F(5));
        assert!(n.status().unwrap().0.contains("Save the file first"));
    }

    fn has_text_area(g: &Gfx) -> bool {
        g.hits.iter().any(|h| h.label == "editor")
    }

    #[test]
    fn a_markdown_file_in_the_ide_opens_as_a_preview_and_the_person_can_switch_to_the_text_and_back() {
        let (cx, home) = world("md1");
        let mut a = app_with(&cx, &home, "README.md", "# Title\n\nSome *text* and a list:\n\n- one\n- two\n");
        a.ide = true;
        assert!(a.md_previewing(), "a Markdown file is shown rendered first");
        assert!(!has_text_area(&render(&mut a, &cx)), "no text area is drawn under the picture");
        // nothing is typed into the picture
        key(&mut a, &cx, Key::Char('x'));
        key(&mut a, &cx, Key::Backspace);
        assert_eq!(a.text(), "# Title\n\nSome *text* and a list:\n\n- one\n- two\n");
        assert!(!a.is_dirty());
        // the key e (and the toolbar's Edit, which sends Ctrl+E) opens the text
        key(&mut a, &cx, Key::Char('e'));
        assert!(!a.md_previewing());
        assert!(has_text_area(&render(&mut a, &cx)));
        key(&mut a, &cx, Key::Char('z'));
        assert!(a.is_dirty() && a.text().contains('z'), "typing works in the text");
        // Preview (Ctrl+E) shows the text as it is now, saved or not
        key(&mut a, &cx, Key::Ctrl('e'));
        assert!(a.md_previewing());
        assert!(a.is_dirty(), "the change is still there, unsaved");
        key(&mut a, &cx, Key::Ctrl('e'));
        assert!(!a.md_previewing());
    }

    #[test]
    fn a_web_link_in_the_markdown_page_is_clickable_at_four_sizes_and_other_links_are_not_opened() {
        let (cx, home) = world("md4");
        let mut a = app_with(&cx, &home, "links.md", "# T\n\nRead [the docs](https://example.com/docs) and [the file](other.md) and [bad](javascript:alert(1)).\n");
        a.ide = true;
        for (w, h, sc) in [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 1.5), (1024.0, 600.0, 0.8), (800.0, 480.0, 0.62)] {
            let mut g = Gfx::new(w as u32, h as u32);
            g.clear(rgb(0, 0, 0));
            a.render(&mut g, R::new(0.0, 72.0 * sc, w, h - 72.0 * sc), sc, &cx, 0);
            let links: Vec<_> = g.hits.iter().filter(|x| x.label == "link").collect();
            assert!(!links.is_empty(), "{w}x{h}");
            assert!(links.iter().all(|x| x.rect.x >= 0.0 && x.rect.x + x.rect.w <= w && x.rect.y + x.rect.h <= h + 1.0), "{w}x{h}");
        }
        let g = render(&mut a, &cx);
        let hits: Vec<usize> = g.hits.iter().filter_map(|x| if let (true, Event::Tab(n)) = (x.label == "link", &x.ev) { Some(*n) } else { None }).collect();
        assert_eq!(hits.len(), 2, "the script link is not drawn as a link at all");
        for n in hits {
            a.handle(&Event::Tab(n), &cx);
        }
        assert_eq!(a.take_md_link().as_deref(), Some("https://example.com/docs"), "only the web address is handed on");
        assert!(a.take_md_link().is_none());
        assert!(a.md_previewing());
    }

    #[test]
    fn only_markdown_files_in_the_ide_get_the_preview_and_a_requested_line_opens_the_text() {
        let (cx, home) = world("md2");
        let mut code = app_with(&cx, &home, "main.rs", "fn main() {}\n");
        code.ide = true;
        assert!(!code.md_previewing());
        let mut plain = app_with(&cx, &home, "notes.md", "text\n");
        assert!(!plain.md_previewing(), "outside the IDE (the Edit tile) a Markdown file is the text, as before");
        plain.ide = true;
        assert!(plain.md_previewing());
        plain.goto_line(1);
        assert!(!plain.md_previewing(), "a search hit or a problem wants the line");
        // the choice is per file
        let other = home.join("other.md");
        std::fs::write(&other, "# o\n").unwrap();
        plain.open_path(&cx, &other.to_string_lossy()).unwrap();
        assert!(plain.md_previewing(), "another Markdown file starts as a preview");
    }

    #[test]
    fn the_preview_scrolls_by_wheel_and_keys_and_find_switches_to_the_text() {
        let (cx, home) = world("md3");
        let body: String = (0..120).map(|i| format!("Line number {i} of the long page.\n\n")).collect();
        let mut a = app_with(&cx, &home, "long.md", &body);
        a.ide = true;
        render(&mut a, &cx);
        a.handle(&Event::Scroll(-5), &cx);
        key(&mut a, &cx, Key::PageDown);
        render(&mut a, &cx);
        assert!(a.md_previewing(), "scrolling keeps the picture");
        key(&mut a, &cx, Key::Ctrl('f'));
        assert!(!a.md_previewing(), "Find works on the text");
    }


    #[test]
    #[ignore = "writes a picture: IYAKKI_MD_PIC=file.png cargo test -p iyakki-shell md_picture -- --ignored"]
    fn md_picture() {
        let Ok(out) = std::env::var("IYAKKI_MD_PIC") else { return };
        let (cx, home) = world("mdpic");
        let mut a = app_with(&cx, &home, "README.md", "# IYAKKI\n\nAn **OS for agents**. See `iyakki dev` and [the guide](https://example.org).\n\n## Install\n\n- write the stick\n- boot it\n\n> a quote\n\n```\ncargo build\n```\n");
        a.ide = true;
        render(&mut a, &cx).pm.save_png(out).unwrap();
    }

}
