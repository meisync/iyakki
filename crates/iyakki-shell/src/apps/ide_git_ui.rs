//! The dialogs of the AI IDE's git support: "Clone repositories…" (one address per line, a compact table with branch, depth and
//! folder per repository, the workspace to clone into) and the "Repositories" panel (state, progress and last git message per
//! repository, Fetch all, Pull all, retry, commands, remove). The work is in `iyakki_agent::{gitjobs, gitkeys, gitsite}` and
//! `ide_git.rs`; this file is the screens and their keys. Nothing here starts a program.

use crate::safeclamp::SafeClamp;
use std::collections::BTreeMap;
use std::sync::Arc;

use iyakki_agent::gitjobs::GitCheck;
use iyakki_agent::gitkeys::{Auth, GitKeys};
use iyakki_agent::gitsite::{self, CloneLine, ParseOptions, RepoUrl};

use super::multiline::MultiLine;
use crate::fx::{self, AMBER, CYAN};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// Click ids of these dialogs (above the IDE's own ranges).
pub const GIT_BASE: usize = 8_800_000;
const AREA_ID: usize = GIT_BASE + 1;
const NEWNAME_ID: usize = 10;
const CELL_ID: usize = 100;

// ------------------------------------------------------------------ the clone dialog

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foc {
    Area,
    Target,
    NewName,
    /// (row, column): 0 branch, 1 depth, 2 folder.
    Cell(usize, usize),
}

struct Cells {
    url: String,
    cols: [TextField; 3],
}

pub struct RowInfo {
    pub n: usize,
    pub line: CloneLine,
    pub parsed: Result<RepoUrl, String>,
    pub uses: String,
    /// The address git gets (the line itself, or what `@instance/owner/repo` resolved to).
    pub url: String,
    /// The platform service the line named, if it did.
    pub via: Option<iyakki_agent::gitjobs::InstanceRef>,
}

/// Resolves `@instance/owner/repo` (see `gitjobs::expand_instance`).
pub type InstanceFn = Arc<dyn Fn(&str) -> Option<Result<iyakki_agent::gitjobs::InstanceRef, String>> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DlgEvent {
    None,
    Cancel,
    Submit,
}

pub struct CloneDialog {
    pub area: MultiLine,
    pub focus: Foc,
    /// Clone into a workspace made for it (named in `new_name`) instead of the open one.
    pub target_new: bool,
    /// A workspace is open to clone into.
    pub can_current: bool,
    pub cur_name: String,
    pub new_name: TextField,
    cells: Vec<Cells>,
    pub rows: Vec<RowInfo>,
    scroll: usize,
    pub error: Option<String>,
    synced: String,
    keys: Arc<GitKeys>,
    ws_keys: BTreeMap<String, String>,
    allow_local: bool,
    inst: Option<InstanceFn>,
    /// git is missing: the message, shown in place of the table.
    pub git_missing: Option<String>,
    /// Where the table was last drawn (tests).
    pub last_rows_shown: usize,
}

impl CloneDialog {
    pub fn new(keys: Arc<GitKeys>, ws_keys: BTreeMap<String, String>, cur_name: Option<String>, git: &GitCheck, initial: &str) -> CloneDialog {
        let mut area = MultiLine::new(20_000);
        area.enter_newline = true;
        area.min_rows = 3;
        area.max_rows = 6;
        if !initial.is_empty() {
            area.set(initial);
        }
        let can_current = cur_name.is_some();
        let mut new_name = TextField::new("name of the new workspace", false);
        new_name.max = 60;
        let mut d = CloneDialog {
            area,
            focus: Foc::Area,
            target_new: !can_current,
            can_current,
            cur_name: cur_name.unwrap_or_default(),
            new_name,
            cells: vec![],
            rows: vec![],
            scroll: 0,
            error: None,
            synced: "\u{1}".into(),
            keys,
            ws_keys,
            allow_local: false,
            inst: None,
            git_missing: match git {
                GitCheck::Missing { message } => Some(message.clone()),
                _ => None,
            },
            last_rows_shown: 0,
        };
        d.sync();
        d
    }

    /// How `@instance/owner/repo` lines are read (the platform services' git services).
    pub fn with_instances(mut self, f: InstanceFn) -> CloneDialog {
        self.inst = Some(f);
        self.synced = "\u{1}".into();
        self.sync();
        self
    }

    /// Tests may clone from local paths.
    pub fn allow_local(mut self, on: bool) -> CloneDialog {
        self.allow_local = on;
        self.synced = "\u{1}".into();
        self.sync();
        self
    }

    /// Parse the text area again when it changed; the cells of a line that is still there keep what was typed in them.
    pub fn sync(&mut self) {
        if self.synced == self.area.text {
            return;
        }
        self.synced = self.area.text.clone();
        let lines = gitsite::split_lines(&self.area.text);
        let mut old: Vec<Cells> = std::mem::take(&mut self.cells);
        let mut rows = Vec::new();
        let mut cells = Vec::new();
        for (i, l) in lines.into_iter().enumerate() {
            let (parsed, url, via) = match self.inst.as_ref().and_then(|f| f(&l.url)) {
                Some(Ok(r)) => (gitsite::parse(&r.url, ParseOptions { allow_local: self.allow_local }), r.url.clone(), Some(r)),
                Some(Err(e)) => (Err(e), l.url.clone(), None),
                None => (gitsite::parse(&l.url, ParseOptions { allow_local: self.allow_local }), l.url.clone(), None),
            };
            let uses = match &parsed {
                Ok(u) => match self.keys.resolve(u, &self.ws_keys) {
                    Auth::Key { id, .. } => format!("key {id}"),
                    Auth::Token { .. } => "token".to_string(),
                    Auth::None if u.is_ssh() => "no key set".to_string(),
                    Auth::None if via.as_ref().is_some_and(|v| v.token_vault.is_some()) => format!("token of {}", via.as_ref().map(|v| v.instance.as_str()).unwrap_or("")),
                    Auth::None => "no login".to_string(),
                },
                Err(_) => String::new(),
            };
            let c = match old.iter().position(|c| c.url == l.url) {
                Some(p) => old.remove(p),
                None => {
                    let mk = |ph: &str, max: usize| {
                        let mut t = TextField::new(ph, false);
                        t.max = max;
                        t
                    };
                    Cells { url: l.url.clone(), cols: [mk("branch", 120), mk("all", 6), mk("auto", 80)] }
                }
            };
            cells.push(c);
            rows.push(RowInfo { n: i + 1, line: l, parsed, uses, url, via });
        }
        self.cells = cells;
        self.rows = rows;
        let max = self.rows.len().saturating_sub(1);
        self.scroll = self.scroll.min(max);
        if let Foc::Cell(r, _) = self.focus {
            if r >= self.rows.len() {
                self.focus = Foc::Area;
            }
        }
    }

    fn order(&self) -> Vec<Foc> {
        let mut v = vec![Foc::Area];
        if self.can_current {
            v.push(Foc::Target);
        }
        if self.target_new {
            v.push(Foc::NewName);
        }
        for r in 0..self.rows.len() {
            for c in 0..3 {
                v.push(Foc::Cell(r, c));
            }
        }
        v
    }

    fn step(&mut self, back: bool) {
        let o = self.order();
        let i = o.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = if back { o[(i + o.len() - 1) % o.len()] } else { o[(i + 1) % o.len()] };
        if let Foc::Cell(r, _) = self.focus {
            self.show_row(r);
        }
    }

    fn show_row(&mut self, r: usize) {
        if r < self.scroll {
            self.scroll = r;
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Foc::NewName => Some(&mut self.new_name),
            Foc::Cell(r, c) => self.cells.get_mut(r).map(|x| &mut x.cols[c]),
            _ => None,
        }
    }

    pub fn branch_of(&self, i: usize) -> String {
        let c = self.cells[i].cols[0].value.trim();
        if c.is_empty() { self.rows[i].line.branch.clone() } else { c.to_string() }
    }

    /// What the person asked for, line by line. Errors are collected for the whole list.
    pub fn specs(&self) -> Result<Vec<iyakki_agent::gitjobs::CloneSpec>, Vec<(usize, String)>> {
        let mut errs = vec![];
        let mut out = vec![];
        for (i, r) in self.rows.iter().enumerate() {
            if let Err(e) = &r.parsed {
                errs.push((r.n, e.clone()));
                continue;
            }
            let depth_text = self.cells[i].cols[1].value.trim();
            let depth = if depth_text.is_empty() { 0 } else { depth_text.parse::<u32>().unwrap_or(u32::MAX) };
            if depth == u32::MAX {
                errs.push((r.n, format!("`{depth_text}` is not a depth (a whole number, or empty for the full history)")));
                continue;
            }
            out.push(iyakki_agent::gitjobs::CloneSpec { url: r.url.clone(), branch: self.branch_of(i), depth, folder: self.cells[i].cols[2].value.trim().to_string(), submodules: false });
        }
        if self.rows.is_empty() {
            errs.push((0, "write at least one repository address, one per line".into()));
        }
        if errs.is_empty() { Ok(out) } else { Err(errs) }
    }

    pub fn handle(&mut self, ev: &Event) -> DlgEvent {
        let out = self.handle_inner(ev);
        self.sync();
        out
    }

    fn handle_inner(&mut self, ev: &Event) -> DlgEvent {
        match ev {
            Event::Key(Key::Esc) => return DlgEvent::Cancel,
            Event::Key(Key::Ctrl('s')) => return DlgEvent::Submit,
            Event::Key(Key::Tab) => self.step(false),
            Event::Key(Key::BackTab) => self.step(true),
            Event::Key(k) => match self.focus {
                Foc::Area => {
                    self.area.key(k);
                }
                Foc::Target => match k {
                    Key::Left | Key::Right | Key::Char(' ') | Key::Enter => self.target_new = !self.target_new,
                    Key::Down => self.step(false),
                    Key::Up => self.step(true),
                    _ => {}
                },
                _ => {
                    if matches!(k, Key::Enter) {
                        self.step(false);
                    } else if matches!(k, Key::Down) {
                        if let Foc::Cell(r, c) = self.focus {
                            if r + 1 < self.rows.len() {
                                self.focus = Foc::Cell(r + 1, c);
                            }
                        }
                    } else if matches!(k, Key::Up) {
                        if let Foc::Cell(r, c) = self.focus {
                            if r > 0 {
                                self.focus = Foc::Cell(r - 1, c);
                                self.show_row(r - 1);
                            }
                        }
                    } else if let Some(f) = self.field() {
                        f.handle(k);
                    }
                }
            },
            Event::Paste(t) => match self.focus {
                Foc::Area => {
                    self.area.paste(t);
                }
                Foc::Target => {}
                _ => {
                    let one: String = t.lines().next().unwrap_or("").to_string();
                    if let Some(f) = self.field() {
                        f.paste(&one);
                    }
                }
            },
            Event::AreaAt(AREA_ID, x, y) => {
                self.focus = Foc::Area;
                self.area.press(*x, *y);
            }
            Event::FocusAt(id, x) => {
                if *id == NEWNAME_ID && self.target_new {
                    self.focus = Foc::NewName;
                    self.new_name.click(*x);
                } else if *id >= CELL_ID {
                    let (r, c) = ((*id - CELL_ID) / 3, (*id - CELL_ID) % 3);
                    if r < self.cells.len() {
                        self.focus = Foc::Cell(r, c);
                        self.cells[r].cols[c].click(*x);
                    }
                }
            }
            Event::Tab(n) if *n == GIT_BASE || *n == GIT_BASE + 2 => {
                if self.can_current {
                    self.focus = Foc::Target;
                    self.target_new = *n == GIT_BASE + 2;
                }
            }
            Event::Scroll(d) => {
                let max = self.rows.len().saturating_sub(1);
                if !self.area.wheel(*d) {
                    self.scroll = (self.scroll as i32 - d).clamp_safe(0, max as i32) as usize;
                }
            }
            _ => {}
        }
        DlgEvent::None
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        self.sync();
        let want_h = 640.0 * s;
        let p = fx::modal(g, area, s, 880.0 * s, want_h, accent, false);
        let (x, w) = (p.x + 26.0 * s, p.w - 52.0 * s);
        fx::modal_title(g, p, s, "Clone repositories", accent);
        let mut y = p.y + 60.0 * s;
        if let Some(m) = self.git_missing.clone() {
            for (i, l) in g.wrap(14.0 * s, &m, Face::Sans, w).iter().take(6).enumerate() {
                g.text(x, y + i as f32 * 20.0 * s, 14.0 * s, AMBER, l, Face::Sans);
            }
            g.button(p.x + p.w - 26.0 * s - 70.0 * s, p.y + p.h - 52.0 * s, 14.0 * s, "Close", accent, Event::Key(Key::Esc));
            return;
        }
        g.text(x, y, 13.0 * s, if self.focus == Foc::Area { TEXT } else { MUTED }, "Repositories: one address per line (https://…, git@host:owner/repo or @gitea-name/owner/repo). Add #branch after an address.", Face::Bold);
        y += 22.0 * s;
        let tick = if blink { 0 } else { 15 };
        let used = self.area.render(g, x, y, w, s, 15.0 * s, 8.0 * s, 56.0 * s, self.focus == Foc::Area, tick, "https://git.example.org/team/app.git\ngit@github.com:owner/tool.git #dev\n…", accent, AREA_ID, "repositories", false, true);
        y += used + 8.0 * s;
        // the target workspace
        g.text(x, y + 8.0 * s, 13.0 * s, if matches!(self.focus, Foc::Target) { TEXT } else { MUTED }, "Into", Face::Bold);
        let mut tx = x + 44.0 * s;
        if self.can_current {
            let label = format!("{}  {}", if self.target_new { "○" } else { "●" }, if self.cur_name.is_empty() { "this workspace".to_string() } else { format!("this workspace ({})", self.cur_name) });
            tx += g.button(tx, y, 13.0 * s, &label, if self.target_new { MUTED } else { accent }, Event::Tab(GIT_BASE)) + 8.0 * s;
            tx += g.button(tx, y, 13.0 * s, &format!("{}  a new workspace", if self.target_new { "●" } else { "○" }), if self.target_new { accent } else { MUTED }, Event::Tab(GIT_BASE + 2)) + 10.0 * s;
            if matches!(self.focus, Foc::Target) {
                fx::chip(g, tx, y + 3.0 * s, s * 0.8, "← → to choose", CYAN, false);
            }
        } else {
            g.text(tx, y + 8.0 * s, 13.0 * s, MUTED, "a new workspace (none is open)", Face::Sans);
            tx += 230.0 * s;
        }
        if self.target_new {
            let fw = (x + w - tx - 4.0 * s).clamp_safe(120.0 * s, 330.0 * s);
            let r = R::new((x + w - fw).max(tx), y, fw, 30.0 * s);
            self.new_name.render(g, r, self.focus == Foc::NewName, s, blink, Face::Sans, NEWNAME_ID);
        }
        y += 42.0 * s;
        // the table
        let head_y = y;
        let cols = table_cols(x, w, s);
        for (label, cx) in [("Repository", cols[0]), ("Branch", cols[1]), ("Depth", cols[2]), ("Folder", cols[3]), ("Login", cols[4])] {
            g.text(cx, head_y, 11.0 * s, DIM, label, Face::Bold);
        }
        y += 18.0 * s;
        let rh = 32.0 * s;
        let bottom = p.y + p.h - 74.0 * s;
        let shown = (((bottom - y) / rh) as usize).max(1);
        self.last_rows_shown = shown.min(self.rows.len());
        if self.rows.is_empty() {
            g.text(x, y + 4.0 * s, 13.0 * s, DIM, "Nothing yet: paste or type the addresses above.", Face::Sans);
        }
        let first = self.scroll.min(self.rows.len().saturating_sub(shown));
        self.scroll = first;
        for (k, i) in (first..self.rows.len().min(first + shown)).enumerate() {
            let ry = y + k as f32 * rh;
            let r = &self.rows[i];
            let name = match &r.parsed {
                Ok(u) => format!("{}. {}", r.n, u.short()),
                Err(_) => format!("{}. {}", r.n, r.line.url),
            };
            let nm = g.fit(13.0 * s, &name, Face::Sans, cols[1] - cols[0] - 8.0 * s);
            g.text(cols[0], ry + 7.0 * s, 13.0 * s, if r.parsed.is_ok() { TEXT } else { ERR }, &nm, Face::Sans);
            for c in 0..3 {
                let width = [cols[2] - cols[1] - 8.0 * s, cols[3] - cols[2] - 8.0 * s, cols[4] - cols[3] - 8.0 * s][c];
                let fr = R::new(cols[1 + c], ry, width, 28.0 * s);
                let ph = if c == 0 && !r.line.branch.is_empty() { r.line.branch.clone() } else { String::new() };
                if c == 0 && !ph.is_empty() && self.cells[i].cols[0].value.is_empty() && self.focus != Foc::Cell(i, 0) {
                    fx::field_frame(g, fr, s, false, accent);
                    let phs = g.fit(14.0 * s, &ph, Face::Sans, fr.w - 16.0 * s);
                    g.text(fr.x + 10.0 * s, fr.y + 6.0 * s, 14.0 * s, MUTED, &phs, Face::Sans);
                    g.hit(fr, Event::Focus(CELL_ID + i * 3), "branch");
                } else {
                    self.cells[i].cols[c].render(g, fr, self.focus == Foc::Cell(i, c), s, blink, Face::Sans, CELL_ID + i * 3 + c);
                }
            }
            let (txt, col) = match &r.parsed {
                Ok(_) if r.uses == "no key set" => ("no key set".to_string(), AMBER),
                Ok(_) => (r.uses.clone(), MUTED),
                Err(e) => (e.clone(), ERR),
            };
            let t = g.fit(12.0 * s, &txt, Face::Sans, x + w - cols[4] - 2.0 * s);
            g.text(cols[4], ry + 8.0 * s, 12.0 * s, col, &t, Face::Sans);
        }
        if self.rows.len() > shown {
            fx::scrollbar(g, R::new(p.x + p.w - 14.0 * s, y, 5.0 * s, shown as f32 * rh), s, first, shown, self.rows.len(), accent);
        }
        // message and buttons
        if let Some(e) = &self.error {
            let e = g.fit(13.0 * s, e, Face::Sans, w);
            g.text(x, p.y + p.h - 70.0 * s + 4.0 * s, 13.0 * s, ERR, &e, Face::Sans);
        } else if !self.rows.is_empty() {
            let ok = self.rows.iter().filter(|r| r.parsed.is_ok()).count();
            let msg = format!("{} to clone, at most 2 at a time. Keys and tokens come from Settings → Git and SSH keys; nothing secret goes in an address.", if ok == 1 { "1 repository".to_string() } else { format!("{ok} repositories") });
            let m = g.fit(12.0 * s, &msg, Face::Sans, w);
            g.text(x, p.y + p.h - 70.0 * s + 4.0 * s, 12.0 * s, DIM, &m, Face::Sans);
        }
        let by = p.y + p.h - 50.0 * s;
        g.button(p.x + p.w - 26.0 * s - 90.0 * s, by, 14.0 * s, "Clone", accent, Event::Key(Key::Ctrl('s')));
        g.button(p.x + p.w - 26.0 * s - 90.0 * s - 100.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
        let help = g.fit(12.0 * s, "Tab moves · Ctrl+S clones · Esc closes", Face::Sans, w - 230.0 * s);
        g.text(x, by + 8.0 * s, 12.0 * s, DIM, &help, Face::Sans);
    }
}

fn table_cols(x: f32, w: f32, s: f32) -> [f32; 5] {
    let _ = s;
    [x, x + w * 0.30, x + w * 0.50, x + w * 0.60, x + w * 0.80]
}

// ------------------------------------------------------------------ the repositories panel

/// One line of the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoRow {
    pub folder: String,
    pub short: String,
    /// `main ↑1 ↓2 ✎3`, or the status error.
    pub status: String,
    /// `queued`, `cloning 45%`, `done`, `failed`, or empty when no batch is working on it.
    pub work: String,
    pub work_failed: bool,
    pub last: String,
    pub test: String,
    pub run: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReposAct {
    None,
    Close,
    /// "Connect Git / Gitea": the connection flow for this project.
    Connect,
    /// Push the project's branch to `origin` (a review comes first).
    Push,
    /// "Put this project under version control on Local Gitea" (only offered while the project has no remote).
    LocalGitea,
    /// Remove `origin` (a confirmation comes first).
    Disconnect,
    CloneMore,
    FetchAll,
    PullAll,
    Refresh,
    /// The keys this workspace uses for some hosts.
    Keys,
    Retry(usize),
    Commands(usize),
    Remove(usize),
}

/// What the panel's header says about the project's own git setup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WsHead {
    pub is_repo: bool,
    pub branch: String,
    /// `origin`, with no credential in it.
    pub origin: Option<String>,
    pub commits: usize,
    pub changed: usize,
    /// The connection (Settings → Git) that serves the host of `origin`, if there is one.
    pub conn: Option<String>,
    pub host: String,
    pub kind: String,
    pub place: String,
}

impl WsHead {
    /// The line under the title: who the project is connected to.
    pub fn line(&self) -> String {
        match &self.origin {
            Some(o) => format!("Connected to {}{} · {} · branch {}", self.conn.as_deref().map_or_else(|| self.host.clone(), |c| format!("{c} ({})", self.host)), if self.place.is_empty() { String::new() } else { format!(" · {}", self.place) }, o, if self.branch.is_empty() { "(detached)" } else { &self.branch }),
            None => "This project is not connected to Git.".to_string(),
        }
    }
}

pub struct ReposPanel {
    pub head: Option<WsHead>,
    pub sel: usize,
    scroll: usize,
    pub git: GitCheck,
    /// The last frame's row count and what it showed (tests).
    pub last_shown: usize,
}

impl ReposPanel {
    pub fn new(git: GitCheck) -> ReposPanel {
        ReposPanel { head: None, sel: 0, scroll: 0, git, last_shown: 0 }
    }

    pub fn handle(&mut self, ev: &Event, rows: &[RepoRow]) -> ReposAct {
        let n = rows.len();
        match ev {
            Event::Key(Key::Esc) => ReposAct::Close,
            Event::Key(Key::Up) => {
                self.sel = self.sel.saturating_sub(1);
                ReposAct::None
            }
            Event::Key(Key::Down) => {
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
                ReposAct::None
            }
            Event::Key(Key::Char(c)) => match c.to_ascii_lowercase() {
                'c' => ReposAct::CloneMore,
                'o' => ReposAct::Connect,
                'u' => ReposAct::Push,
                'l' => ReposAct::LocalGitea,
                'd' => ReposAct::Disconnect,
                'f' => ReposAct::FetchAll,
                'p' => ReposAct::PullAll,
                'r' if n > 0 => ReposAct::Retry(self.sel),
                'e' if n > 0 => ReposAct::Commands(self.sel),
                'x' if n > 0 => ReposAct::Remove(self.sel),
                'g' => ReposAct::Refresh,
                'k' => ReposAct::Keys,
                _ => ReposAct::None,
            },
            Event::Key(Key::Delete) if n > 0 => ReposAct::Remove(self.sel),
            Event::Scroll(d) => {
                self.scroll = (self.scroll as i32 - d).clamp_safe(0, n.saturating_sub(1) as i32) as usize;
                ReposAct::None
            }
            Event::Row(i) if *i < n => {
                self.sel = *i;
                ReposAct::None
            }
            Event::Tab(t) if (GIT_BASE + 100..GIT_BASE + 100 + 9).contains(t) => match t - (GIT_BASE + 100) {
                0 => ReposAct::CloneMore,
                1 => ReposAct::FetchAll,
                2 => ReposAct::PullAll,
                3 => ReposAct::Refresh,
                4 => ReposAct::Keys,
                5 => ReposAct::Connect,
                6 => ReposAct::Push,
                7 => ReposAct::Disconnect,
                8 => ReposAct::LocalGitea,
                _ => ReposAct::Close,
            },
            Event::Tab(t) if *t >= GIT_BASE + 1000 => {
                let (i, k) = ((t - (GIT_BASE + 1000)) / 4, (t - (GIT_BASE + 1000)) % 4);
                if i >= n {
                    return ReposAct::None;
                }
                self.sel = i;
                match k {
                    0 => ReposAct::Retry(i),
                    1 => ReposAct::Commands(i),
                    _ => ReposAct::Remove(i),
                }
            }
            _ => ReposAct::None,
        }
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], rows: &[RepoRow], note: &str) {
        let p = fx::modal(g, area, s, 900.0 * s, 600.0 * s, accent, false);
        let (x, w) = (p.x + 24.0 * s, p.w - 48.0 * s);
        fx::modal_title(g, p, s, "Repositories", accent);
        let mut y = p.y + 58.0 * s;
        let top = match &self.git {
            GitCheck::Ready { version, .. } => (format!("git {version}"), MUTED),
            GitCheck::Missing { message } => (message.clone(), AMBER),
        };
        for (i, l) in g.wrap(12.0 * s, &top.0, Face::Sans, w).iter().take(3).enumerate() {
            g.text(x, y + i as f32 * 16.0 * s, 12.0 * s, top.1, l, Face::Sans);
        }
        y += if matches!(self.git, GitCheck::Missing { .. }) { 52.0 * s } else { 22.0 * s };
        // the buttons of the whole list
        let mut bx = x;
        let mut ids = 0usize;
        for (label, color) in [("Clone…  c", accent), ("Fetch all  f", CYAN), ("Pull all  p", CYAN), ("Refresh  g", MUTED), ("Keys…  k", MUTED)] {
            bx += g.button(bx, y, 12.0 * s, label, color, Event::Tab(GIT_BASE + 100 + ids)) + 8.0 * s;
            ids += 1;
        }
        y += 38.0 * s;
        // the project's own git setup: who it is connected to (with Fetch, Pull, Push and Disconnect), or the prompt to connect it
        if let Some(h) = self.head.clone() {
            let connected = h.origin.is_some();
            let line = g.fit(13.0 * s, &h.line(), Face::Bold, w);
            g.text(x, y, 13.0 * s, if connected { OK } else { AMBER }, &line, Face::Bold);
            y += 22.0 * s;
            let mut bx = x;
            if connected {
                bx += g.button(bx, y, 12.0 * s, "Fetch  f", CYAN, Event::Tab(GIT_BASE + 100 + 1)) + 8.0 * s;
                bx += g.button(bx, y, 12.0 * s, "Pull  p", CYAN, Event::Tab(GIT_BASE + 100 + 2)) + 8.0 * s;
                bx += g.button(bx, y, 12.0 * s, "Push…  u", accent, Event::Tab(GIT_BASE + 100 + 6)) + 8.0 * s;
                bx += g.button(bx, y, 12.0 * s, "Change…  o", MUTED, Event::Tab(GIT_BASE + 100 + 5)) + 8.0 * s;
                g.button(bx, y, 12.0 * s, "Disconnect  d", ERR, Event::Tab(GIT_BASE + 100 + 7));
            } else {
                bx += g.button(bx, y, 13.0 * s, "Connect to Git or Gitea  o", accent, Event::Tab(GIT_BASE + 100 + 5)) + 12.0 * s;
                bx += g.button(bx, y, 13.0 * s, &format!("{}  l", iyakki_agent::quickinstall::gitea::OFFER_LABEL), OK, Event::Tab(GIT_BASE + 100 + 8)) + 12.0 * s;
                g.text(bx, y + 4.0 * s, 12.0 * s, DIM, "Or a repository on GitHub, GitLab or any host.", Face::Sans);
            }
            y += 36.0 * s;
        }
        let rh = 54.0 * s;
        let bottom = p.y + p.h - 66.0 * s;
        let shown = (((bottom - y) / rh) as usize).max(1);
        self.last_shown = shown.min(rows.len());
        if rows.is_empty() {
            g.text(x, y + 6.0 * s, 14.0 * s, DIM, "No repository in this workspace yet. Press c to clone several at once.", Face::Sans);
        }
        let first = self.scroll.min(rows.len().saturating_sub(shown));
        self.scroll = first;
        self.sel = self.sel.min(rows.len().saturating_sub(1));
        for (k, i) in (first..rows.len().min(first + shown)).enumerate() {
            let r = &rows[i];
            let ry = y + k as f32 * rh;
            let rect = R::new(x, ry, w, rh - 4.0 * s);
            let on = i == self.sel;
            g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, if on { fx::with_alpha(accent, 40) } else if g.hovered(rect) { crate::gfx::rgb(26, 33, 60) } else { crate::gfx::rgb(20, 26, 48) });
            g.hit(rect, Event::Row(i), &r.folder);
            let name = g.fit(14.0 * s, &if r.folder == "." { "this project".to_string() } else { format!("{}/", r.folder) }, Face::Bold, 200.0 * s);
            g.text(rect.x + 10.0 * s, rect.y + 6.0 * s, 14.0 * s, TEXT, &name, Face::Bold);
            let st = g.fit(12.0 * s, &r.status, Face::Mono, 250.0 * s);
            g.text(rect.x + 10.0 * s, rect.y + 28.0 * s, 12.0 * s, if r.status.contains('✎') || r.status.contains('↓') { AMBER } else { MUTED }, &st, Face::Mono);
            let mx = rect.x + 270.0 * s;
            let bw = 3.0 * 78.0 * s;
            let mw = (rect.x + rect.w - bw - mx - 8.0 * s).max(40.0 * s);
            let top = if r.work.is_empty() { r.short.clone() } else { r.work.clone() };
            let tops = g.fit(12.0 * s, &top, Face::Bold, mw);
            g.text(mx, rect.y + 6.0 * s, 12.0 * s, if r.work_failed { ERR } else if r.work.is_empty() { MUTED } else { OK }, &tops, Face::Bold);
            let sub = if !r.last.is_empty() { r.last.clone() } else if !r.test.is_empty() || !r.run.is_empty() { format!("test: {}  run: {}", if r.test.is_empty() { "-" } else { &r.test }, if r.run.is_empty() { "-" } else { &r.run }) } else { String::new() };
            let subs = g.fit(12.0 * s, &sub, Face::Sans, mw);
            g.text(mx, rect.y + 28.0 * s, 12.0 * s, DIM, &subs, Face::Sans);
            let mut bx = rect.x + rect.w - bw - 4.0 * s;
            for (j, (label, col)) in [("Retry", AMBER), ("Commands", CYAN), ("Remove", ERR)].into_iter().enumerate() {
                if j == 0 && !r.work_failed {
                    bx += 78.0 * s;
                    continue;
                }
                let bwid = g.button(bx, rect.y + 10.0 * s, 13.0 * s, label, col, Event::Tab(GIT_BASE + 1000 + i * 4 + j));
                bx += bwid.max(78.0 * s - 6.0 * s) + 6.0 * s;
            }
        }
        if rows.len() > shown {
            fx::scrollbar(g, R::new(p.x + p.w - 12.0 * s, y, 5.0 * s, shown as f32 * rh), s, first, shown, rows.len(), accent);
        }
        if !note.is_empty() {
            let n = g.fit(12.0 * s, note, Face::Sans, w - 120.0 * s);
            g.text(x, p.y + p.h - 58.0 * s, 12.0 * s, AMBER, &n, Face::Sans);
        }
        g.button(p.x + p.w - 24.0 * s - 80.0 * s, p.y + p.h - 50.0 * s, 14.0 * s, "Close", accent, Event::Key(Key::Esc));
        let help = g.fit(12.0 * s, "↑ ↓ choose · r retry · e commands · x remove · k keys · o connect Git · l Local Gitea · u push · d disconnect · Esc closes", Face::Sans, w - 120.0 * s);
        g.text(x, p.y + p.h - 42.0 * s, 12.0 * s, DIM, &help, Face::Sans);
    }
}
