//! The AI IDE's Git tab (docs/GIT-TILE.md, "In the AI IDE"). Two sub-views that never mix:
//!
//! * **This project**: the state of the folder open in the IDE when it is a git working copy: the branch with ahead and behind, the
//!   changed files with their status, the recent commits, and the diff of a changed file or of a commit. Opening a file from here
//!   puts it in the IDE's editor. It is read with the git program (`iyakki_agent::gitbrowse::LocalRepo`: `status`, `log`, `diff`,
//!   `show`; nothing is written, nothing touches the network, no credential is involved).
//! * **Connections**: the GIT tile itself ([`GitApp`]) drawn in the tab: the connection picker (the one of Settings, Git; the last
//!   choice is remembered per person), the repositories of the chosen host, and a repository's files, commits, branches and diffs.
//!   It is read-only here: clone, fetch, pull and push stay with the IDE's Repositories panel, which asks first.
//!
//! Every read runs on its own thread (`Slot`) and is picked up on the heartbeat; nothing is read while the tab is not drawn. Click ids
//! of this file are above those of the tile (`900_000...`); the IDE moves the whole tab into the range of [`GIT`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::gitbrowse::{BrowseErr, Change, ChangeKind, CommitInfo, Diff, LineKind, LocalRepo, RepoReader, Working};
use iyakki_agent::wsexec::Runner;

use super::git_tile::{GitApp, Load, Rig, Slot, ACCENT};
use super::git_tile_repo_ui::draw_text;
use super::git_tile_ui::{btn, btn_w, fit_list, para, sbar, Cursor};
use super::k8s_tile::{Line, ListState, TextView};
use super::{AppCx, Flow};
use crate::fx::{self, with_alpha, AMBER, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// The tab's code in the IDE's click-id routing (see `component::route`).
pub const GIT: usize = 6;

const P_SUB: usize = 900_001; // +0 This project, +1 Connections
const P_REFRESH: usize = 900_010;
const P_OPEN: usize = 900_011;
const P_BACK: usize = 900_012;
const P_CHG: usize = 910_000;
const P_COM: usize = 920_000;

/// Read the working copy again this often while the tab is drawn.
const REREAD: Duration = Duration::from_secs(5);
const KEY: [u8; 4] = rgb(130, 190, 255);

pub const NOT_A_REPO: &str = "This folder is not a Git working copy.";
pub const NOT_A_REPO_HINT: &str = "Run git init in the terminal to start one, or connect the project to a host with Connect Git (Repositories, \"Put this project under version control on Local Gitea\", after Store \u{2192} Setup \u{2192} Gitea is installed). The Connections view shows the repositories of your Git hosts.";
pub const NO_CHANGES: &str = "No changes: the working copy matches the last commit.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Project,
    Connections,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Changes,
    Commits,
    Diff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Nothing,
    Change(String),
    Commit(String),
}

pub struct GitTabView {
    pub sub: Sub,
    pub app: GitApp,
    root: PathBuf,
    run: Arc<dyn Runner>,
    tmp: PathBuf,
    born: Instant,
    pub(super) work: Slot<Working>,
    pub(super) commits: Slot<Vec<CommitInfo>>,
    read_at: Option<Instant>,
    list: ListState,
    clist: ListState,
    pane: Pane,
    shown: Shown,
    diff: Slot<Diff>,
    text: TextView,
    title: String,
    msg: Vec<String>,
    /// Narrow screens show the diff as a page of its own.
    page_diff: bool,
    last_draw: Option<Instant>,
    mouse: (i32, i32),
    rects: [R; 3],
    status: Option<(String, bool)>,
    open: Option<String>,
    request: Option<(String, String)>,
}

impl GitTabView {
    pub fn new(cx: &AppCx, root: PathBuf, rig: Rig) -> GitTabView {
        let (run, tmp) = (rig.run.clone(), rig.scratch.clone());
        let mut app = GitApp::with_rig(cx, rig);
        app.embedded = true;
        let sub = if root.join(".git").exists() { Sub::Project } else { Sub::Connections };
        GitTabView {
            sub,
            app,
            root,
            run,
            tmp,
            born: Instant::now(),
            work: Slot::new(),
            commits: Slot::new(),
            read_at: None,
            list: ListState::default(),
            clist: ListState::default(),
            pane: Pane::Changes,
            shown: Shown::Nothing,
            diff: Slot::new(),
            text: TextView::default(),
            title: String::new(),
            msg: vec![],
            page_diff: false,
            last_draw: None,
            mouse: (-1, -1),
            rects: [R::default(); 3],
            status: None,
            open: None,
            request: None,
        }
    }

    fn now_ms(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    fn visible(&self) -> bool {
        self.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(500))
    }

    fn repo(&self) -> LocalRepo {
        LocalRepo::new(self.run.clone(), &self.root, &self.tmp)
    }

    /// A text input of the tab owns the keyboard (the Connections filter).
    pub fn wants_text(&self) -> bool {
        self.sub == Sub::Connections && self.app.wants_text()
    }

    /// A card of the embedded tile (a review, a choice) is open.
    pub fn has_modal(&self) -> bool {
        self.sub == Sub::Connections && self.app.has_card()
    }

    /// A file the person asked to open in the editor (relative to the project folder).
    pub fn take_open(&mut self) -> Option<String> {
        self.open.take()
    }

    /// What the tab asks of the shell (add a connection in Settings, a web page).
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take().or_else(|| self.app.request.take())
    }

    pub fn status_line(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }

    #[cfg(test)]
    pub fn text_has(&self, needle: &str) -> bool {
        self.text.lines.iter().any(|l| l.text.contains(needle))
    }

    pub fn not_a_repo(&self) -> bool {
        self.work.problem().is_some_and(|p| p.contains("not a git repository"))
    }

    // ------------------------------------------------------------------------------------------------ reading

    fn start_reads(&mut self) {
        self.read_at = Some(Instant::now());
        let l = self.repo();
        self.work.start(move || l.working());
        // a repository without a commit has nothing to list
        let empty = self.work.ready().is_some_and(|w| w.status.branch.is_empty() && !w.status.detached);
        if !empty {
            let l = self.repo();
            self.commits.start(move || l.commits(""));
        }
    }

    /// The heartbeat: pick up finished reads, start the next one while drawn. True when something changed on screen.
    pub fn tick(&mut self, cx: &AppCx) -> bool {
        let mut changed = false;
        let now = self.now_ms();
        if self.sub == Sub::Connections || self.app.visible() {
            let _ = self.app.handle_event(&Event::Tick, cx);
        }
        if self.work.poll(now).is_some() {
            changed = true;
            let n = self.work.ready().map_or(0, |w| w.changes.len());
            self.list.sel = self.list.sel.min(n.saturating_sub(1));
        }
        if self.commits.poll(now).is_some() {
            changed = true;
            let n = self.commits.ready().map_or(0, |c| c.len());
            self.clist.sel = self.clist.sel.min(n.saturating_sub(1));
        }
        if self.diff.poll(now).is_some() {
            changed = true;
            self.rebuild_text();
        }
        if self.visible() && self.sub == Sub::Project && self.read_at.is_none_or(|t| t.elapsed() >= REREAD) {
            self.start_reads();
        }
        changed || (self.sub == Sub::Connections && self.visible())
    }

    fn rebuild_text(&mut self) {
        let mut lines: Vec<Line> = Vec::new();
        for (i, l) in self.msg.iter().enumerate() {
            lines.push(Line::plain(l.clone(), if i == 0 { TEXT } else { MUTED }));
        }
        if !self.msg.is_empty() {
            lines.push(Line::plain("", DIM));
        }
        match &self.diff.st {
            Load::Ready(d) => {
                if d.too_large {
                    lines.push(Line::plain(format!("This diff is larger than {} KB, so it is not shown here. The files it changes are listed below.", iyakki_agent::gitbrowse::DIFF_MAX / 1024), AMBER));
                    for f in &d.files {
                        lines.push(Line::plain(format!("  {} {}", f.status, f.name()), DIM));
                    }
                } else if d.files.is_empty() {
                    lines.push(Line::plain(if matches!(self.shown, Shown::Commit(_)) { "This commit changes no files (an empty or merge commit)." } else { "Nothing differs from the last commit for this file." }, DIM));
                }
                for f in d.files.iter().filter(|_| !d.too_large) {
                    if d.files.len() > 1 {
                        lines.push(Line::plain(format!("{} {}  +{} \u{2212}{}", f.status, f.name(), f.added, f.removed), KEY));
                    }
                    if f.binary && f.lines.iter().all(|l| l.kind == LineKind::Meta) {
                        lines.push(Line::plain(format!("{}: a binary file changed.", f.name()), DIM));
                    }
                    for l in &f.lines {
                        let c = match l.kind {
                            LineKind::Add => OK,
                            LineKind::Del => ERR,
                            LineKind::Hunk => CYAN,
                            LineKind::Meta => DIM,
                            LineKind::Ctx => TEXT,
                        };
                        lines.push(Line::plain(l.text.replace('\t', "    ").replace('\r', ""), c));
                    }
                }
            }
            Load::Failed(e) => lines.push(Line::plain(e.text(), ERR)),
            _ => lines.push(Line::plain("Reading the diff\u{2026}", DIM)),
        }
        self.text.set(lines, false);
    }

    fn changes(&self) -> &[Change] {
        self.work.ready().map(|w| w.changes.as_slice()).unwrap_or(&[])
    }

    fn change_at(&self, i: usize) -> Option<Change> {
        self.changes().get(i).cloned()
    }

    /// Show the diff of the changed file `i` (an untracked file has none: it opens in the editor instead).
    pub fn show_change(&mut self, i: usize) {
        let Some(c) = self.change_at(i) else { return };
        self.list.sel = i;
        self.pane = Pane::Changes;
        if c.kind == ChangeKind::Untracked {
            self.shown = Shown::Change(c.path.clone());
            self.diff.reset();
            self.diff.st = Load::Ready(Diff::default());
            self.title = format!("{} \u{b7} new, not tracked", c.path);
            self.msg = vec!["A new file that git does not track yet: there is nothing to compare it with. Press o to open it in the editor.".into()];
            self.rebuild_text();
            self.page_diff = true;
            return;
        }
        self.shown = Shown::Change(c.path.clone());
        self.title = format!("{} \u{b7} {} \u{b7} against the last commit", c.path, c.kind.word());
        self.msg = vec![];
        self.diff.reset();
        self.text.set(vec![Line::plain("Reading the diff\u{2026}", DIM)], false);
        let (l, p) = (self.repo(), c.path.clone());
        self.diff.start(move || l.diff_head(&p));
        self.page_diff = true;
    }

    pub fn show_commit(&mut self, i: usize) {
        let Some(c) = self.commits.ready().and_then(|v| v.get(i)).cloned() else { return };
        self.clist.sel = i;
        self.pane = Pane::Commits;
        self.shown = Shown::Commit(c.sha.clone());
        self.title = format!("{} \u{b7} {}", c.short(), c.author);
        self.msg = c.message.lines().map(String::from).collect();
        self.diff.reset();
        self.text.set(vec![Line::plain("Reading the diff\u{2026}", DIM)], false);
        let (l, sha) = (self.repo(), c.sha.clone());
        self.diff.start(move || l.diff(&sha));
        self.page_diff = true;
    }

    /// Open the selected changed file in the editor.
    pub fn open_selected(&mut self) {
        let Some(c) = self.change_at(self.list.sel) else {
            self.status = Some(("Choose a changed file first.".into(), false));
            return;
        };
        if c.kind == ChangeKind::Deleted {
            self.status = Some((format!("{} was deleted: there is no file to open. The diff shows what was removed.", c.path), false));
            return;
        }
        self.open = Some(c.path);
    }

    pub fn refresh(&mut self) {
        self.read_at = None;
        self.status = None;
    }

    /// Context for the assistant: the project's repository state as marked data (paths and counts only, never an address with a
    /// credential, never file contents).
    pub fn context_block(&self) -> Option<String> {
        let w = self.work.ready()?;
        let mut s = format!("Git working copy of this project: {}", w.status.summary());
        if !w.status.upstream.is_empty() {
            s.push_str(&format!(" (tracking {})", w.status.upstream));
        }
        s.push('\n');
        if w.changes.is_empty() {
            s.push_str("No changed files.\n");
        }
        for c in w.changes.iter().take(30) {
            s.push_str(&format!("{} {}\n", c.kind.letter(), c.path));
        }
        if w.changes.len() > 30 {
            s.push_str(&format!("... and {} more\n", w.changes.len() - 30));
        }
        if let Shown::Change(p) = &self.shown {
            s.push_str(&format!("The person is looking at the diff of {p}.\n"));
        }
        if let Some(c) = self.commits.ready() {
            for c in c.iter().take(5) {
                s.push_str(&format!("commit {} {}\n", c.short(), c.subject()));
            }
        }
        Some(s)
    }

    // ------------------------------------------------------------------------------------------------ input

    pub fn mouse(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
        self.app.mouse = (x, y);
    }

    /// True when the event was used.
    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> bool {
        match ev {
            Event::Tab(n) if *n >= 900_000 => {
                self.click(*n);
                true
            }
            Event::Key(k) => self.key(k, cx),
            Event::Scroll(d) => {
                if self.sub == Sub::Connections {
                    let _ = self.app.handle_event(ev, cx);
                } else {
                    self.wheel(*d);
                }
                true
            }
            Event::MouseMove(x, y) => {
                self.mouse(*x, *y);
                true
            }
            _ => {
                if self.sub == Sub::Connections {
                    let _ = self.app.handle_event(ev, cx);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn set_sub(&mut self, s: Sub) {
        self.sub = s;
        self.status = None;
        if s == Sub::Project {
            self.read_at = None;
        }
    }

    fn click(&mut self, n: usize) {
        match n {
            P_SUB => self.set_sub(Sub::Project),
            x if x == P_SUB + 1 => self.set_sub(Sub::Connections),
            P_REFRESH => self.refresh(),
            P_OPEN => self.open_selected(),
            P_BACK => {
                self.page_diff = false;
                self.shown = Shown::Nothing;
            }
            x if (P_CHG..P_CHG + 100_000).contains(&x) => self.show_change(x - P_CHG),
            x if (P_COM..P_COM + 100_000).contains(&x) => self.show_commit(x - P_COM),
            _ => {}
        }
    }

    fn wheel(&mut self, d: i32) {
        let rows = -d * 3;
        let (x, y) = self.mouse;
        if self.rects[0].contains(x, y) {
            let n = self.changes().len();
            self.list.scroll(n, rows);
        } else if self.rects[1].contains(x, y) {
            let n = self.commits.ready().map_or(0, |c| c.len());
            self.clist.scroll(n, rows);
        } else if self.rects[2].contains(x, y) {
            self.text.scroll(rows);
        }
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> bool {
        if self.sub == Sub::Connections {
            let back = matches!(self.app.handle_event(&Event::Key(k.clone()), cx), Flow::Back);
            if back {
                if matches!(k, Key::Esc) && self.root.join(".git").exists() {
                    self.set_sub(Sub::Project);
                    return true;
                }
                return false;
            }
            return true;
        }
        if let Some(n) = super::git_tile::nav_of(k) {
            match self.pane {
                Pane::Changes => {
                    let n_rows = self.changes().len();
                    super::git_tile::nav(&mut self.list, n_rows, n);
                }
                Pane::Commits => {
                    let n_rows = self.commits.ready().map_or(0, |c| c.len());
                    super::git_tile::nav(&mut self.clist, n_rows, n);
                }
                Pane::Diff => match n {
                    super::git_tile::NavKey::Up => self.text.scroll(-1),
                    super::git_tile::NavKey::Down => self.text.scroll(1),
                    super::git_tile::NavKey::PageUp => self.text.scroll(-(self.text.fit as i32).max(1)),
                    super::git_tile::NavKey::PageDown => self.text.scroll((self.text.fit as i32).max(1)),
                    super::git_tile::NavKey::Home => self.text.home(),
                    super::git_tile::NavKey::End => self.text.end(),
                },
            }
            return true;
        }
        match k {
            Key::Tab => {
                self.pane = match self.pane {
                    Pane::Changes => Pane::Commits,
                    Pane::Commits if self.shown != Shown::Nothing => Pane::Diff,
                    _ => Pane::Changes,
                };
            }
            Key::Enter => match self.pane {
                Pane::Changes => self.show_change(self.list.sel),
                Pane::Commits => self.show_commit(self.clist.sel),
                Pane::Diff => {}
            },
            Key::Char('d') => self.show_change(self.list.sel),
            Key::Char('o') => self.open_selected(),
            Key::Char('r') => self.refresh(),
            Key::Char('c') | Key::Char('2') => self.set_sub(Sub::Connections),
            Key::Esc => {
                if self.shown != Shown::Nothing {
                    self.shown = Shown::Nothing;
                    self.page_diff = false;
                    if self.pane == Pane::Diff {
                        self.pane = Pane::Changes;
                    }
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------------------------------------ drawing

    pub fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64, focused: bool) {
        self.last_draw = Some(Instant::now());
        let size = 13.0 * s;
        let row_h = super::ide_chrome::tool(s);
        let mut c = Cursor::new(area.x + 8.0 * s, area.y + 4.0 * s, area.w - 16.0 * s, row_h, 8.0 * s);
        for (i, (label, sub)) in [("This project", Sub::Project), ("Connections", Sub::Connections)].into_iter().enumerate() {
            let on = self.sub == sub;
            let (x, y) = c.place(btn_w(g, size, label));
            g.button(x, y + (row_h - size * 2.1) / 2.0, size, label, if on { ACCENT } else { MUTED }, Event::Tab(P_SUB + i));
        }
        if self.sub == Sub::Project && !self.not_a_repo() {
            btn(g, &mut c, size, "Refresh", MUTED, Event::Tab(P_REFRESH), true);
            let can_open = self.change_at(self.list.sel).is_some_and(|c| c.kind != ChangeKind::Deleted);
            btn(g, &mut c, size, "Open in editor", ACCENT, Event::Tab(P_OPEN), can_open);
        }
        let top = c.bottom() + 6.0 * s - area.y;
        let body = R::new(area.x, area.y + top, area.w, (area.h - top).max(0.0));
        match self.sub {
            Sub::Connections => {
                self.rects = [R::default(); 3];
                self.app.draw(g, body, s, cx, tick);
            }
            Sub::Project => self.draw_project(g, body, s, focused),
        }
    }

    fn draw_project(&mut self, g: &mut Gfx, body: R, s: f32, focused: bool) {
        self.rects = [R::default(); 3];
        let (x, w) = (body.x + 8.0 * s, body.w - 16.0 * s);
        let mut y = body.y + 2.0 * s;
        if self.not_a_repo() {
            g.text(x, y + 4.0 * s, 15.0 * s, TEXT, NOT_A_REPO, Face::Bold);
            para(g, x, y + 28.0 * s, w, 13.5 * s, DIM, NOT_A_REPO_HINT, 6, true);
            return;
        }
        if let (Load::Failed(e), true) = (&self.work.st, self.work.ready().is_none()) {
            para(g, x, y + 4.0 * s, w, 14.0 * s, ERR, &e.text(), 5, true);
            return;
        }
        // the header: branch, ahead and behind, upstream and counts on one line
        match self.work.ready() {
            None => {
                g.text(x, y + 2.0 * s, 14.0 * s, DIM, "Reading the working copy\u{2026}", Face::Sans);
                y += 22.0 * s;
            }
            Some(wk) => {
                let st = &wk.status;
                let head = st.summary();
                let hw = g.text_width(14.0 * s, &head, Face::Bold);
                let shown = g.fit(14.0 * s, &head, Face::Bold, w);
                g.text(x, y + 2.0 * s, 14.0 * s, KEY, &shown, Face::Bold);
                let mut parts = vec![];
                if !st.upstream.is_empty() {
                    parts.push(format!("tracking {}", st.upstream));
                } else if !st.branch.is_empty() {
                    parts.push("no upstream branch".to_string());
                }
                if wk.changes.is_empty() {
                    parts.push("clean".to_string());
                } else if st.conflicted > 0 {
                    parts.push(format!("{} in conflict", st.conflicted));
                }
                let room = w - hw - 12.0 * s;
                if room > 60.0 * s {
                    let line = g.fit(12.5 * s, &parts.join(" \u{b7} "), Face::Sans, room);
                    g.text(x + hw + 12.0 * s, y + 3.0 * s, 12.5 * s, if st.conflicted > 0 { ERR } else { MUTED }, &line, Face::Sans);
                }
                y += g.line_height(14.0 * s, Face::Bold) + 6.0 * s;
            }
        }
        if let Some((t, ok)) = &self.status {
            let h = para(g, x, y, w, 12.5 * s, if *ok { MUTED } else { ERR }, t, 3, true);
            y += h + 4.0 * s;
        }
        let avail = R::new(x, y, w, (body.y + body.h - y - 4.0 * s).max(0.0));
        if avail.h < 40.0 * s {
            return;
        }
        let wide = avail.w / s >= 620.0;
        let gap = 8.0 * s;
        let showing_diff = self.shown != Shown::Nothing;
        let (lists, viewer) = if wide {
            let lw = (avail.w * 0.42).max(260.0 * s).min(avail.w - 300.0 * s);
            (R::new(avail.x, avail.y, lw, avail.h), Some(R::new(avail.x + lw + gap, avail.y, avail.w - lw - gap, avail.h)))
        } else if self.page_diff && showing_diff {
            (R::default(), Some(avail))
        } else {
            (avail, None)
        };
        if lists.w > 0.0 {
            let ch = ((lists.h - gap) * 0.55).floor();
            let changes_r = R::new(lists.x, lists.y, lists.w, ch);
            let commits_r = R::new(lists.x, lists.y + ch + gap, lists.w, lists.h - ch - gap);
            self.draw_changes(g, changes_r, s, focused && self.pane == Pane::Changes);
            self.draw_commits(g, commits_r, s, focused && self.pane == Pane::Commits);
        }
        if let Some(v) = viewer {
            self.draw_viewer(g, v, s, focused && self.pane == Pane::Diff, !wide);
        }
    }

    fn panel(&self, g: &mut Gfx, r: R, s: f32, on: bool) {
        fx::panel(g, r, s, if on { ACCENT } else { rgb(60, 72, 112) });
    }

    fn draw_changes(&mut self, g: &mut Gfx, r: R, s: f32, on: bool) {
        self.panel(g, r, s, on);
        self.rects[0] = r;
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let (n, more) = self.work.ready().map_or((0, 0), |w| (w.changes.len(), w.more));
        g.text(x, r.y + 6.0 * s, 13.0 * s, MUTED, &format!("CHANGES \u{b7} {n}{}", if more > 0 { format!(" (+{more} more)") } else { String::new() }), Face::Bold);
        let list = R::new(x, r.y + 26.0 * s, w, (r.h - 32.0 * s).max(10.0));
        if n == 0 {
            let msg = match (&self.work.st, self.work.problem()) {
                (_, Some(p)) => p,
                (Load::Ready(_), _) => NO_CHANGES.to_string(),
                _ => "Reading the working copy\u{2026}".to_string(),
            };
            para(g, list.x + 2.0 * s, list.y + 2.0 * s, list.w, 13.0 * s, DIM, &msg, 3, true);
            return;
        }
        let row_h = 22.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut self.list, n, fit);
        let items = self.changes().to_vec();
        for k in top..(top + fit).min(n) {
            let c = &items[k];
            let ry = list.y + (k - top) as f32 * row_h;
            if ry + row_h > list.y + list.h + 0.5 {
                break;
            }
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 1.0 * s);
            let sel = k == self.list.sel;
            let hot = g.hovered(rr);
            if sel || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 5.0 * s, sel, hot, ACCENT);
            }
            let col = match c.kind {
                ChangeKind::Added => OK,
                ChangeKind::Deleted | ChangeKind::Conflicted => ERR,
                ChangeKind::Renamed => CYAN,
                ChangeKind::Untracked => DIM,
                ChangeKind::Modified => AMBER,
            };
            g.text(rr.x + 5.0 * s, ry + 3.0 * s, 12.5 * s, col, &c.kind.letter().to_string(), Face::Mono);
            let stage = if c.staged && !c.unstaged { "staged" } else if c.staged { "partly staged" } else { "" };
            let sw = if stage.is_empty() { 0.0 } else { g.text_width(11.0 * s, stage, Face::Sans) + 6.0 * s };
            let label = if c.from.is_empty() { c.path.clone() } else { format!("{} \u{2190} {}", c.path, c.from) };
            let nm = g.fit(12.5 * s, &label, Face::Sans, rr.w - 24.0 * s - sw);
            g.text(rr.x + 20.0 * s, ry + 3.0 * s, 12.5 * s, TEXT, &nm, Face::Sans);
            if !stage.is_empty() {
                g.text(rr.x + rr.w - sw, ry + 4.0 * s, 11.0 * s, DIM, stage, Face::Sans);
            }
            g.hit(rr, Event::Tab(P_CHG + k), &c.path);
        }
        sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
    }

    fn draw_commits(&mut self, g: &mut Gfx, r: R, s: f32, on: bool) {
        self.panel(g, r, s, on);
        self.rects[1] = r;
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        g.text(x, r.y + 6.0 * s, 13.0 * s, MUTED, "RECENT COMMITS", Face::Bold);
        let list = R::new(x, r.y + 26.0 * s, w, (r.h - 32.0 * s).max(10.0));
        let items = self.commits.ready().cloned().unwrap_or_default();
        if items.is_empty() {
            let no_commits = self.work.ready().is_some_and(|w| w.status.branch.is_empty() && !w.status.detached);
            let msg = match (&self.commits.st, self.commits.problem()) {
                _ if no_commits => "This repository has no commits yet.".to_string(),
                (_, Some(p)) => p,
                (Load::Ready(_), _) => "No commits.".to_string(),
                _ => "Reading the commits\u{2026}".to_string(),
            };
            para(g, list.x + 2.0 * s, list.y + 2.0 * s, list.w, 13.0 * s, DIM, &msg, 3, true);
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let row_h = 38.0 * s;
        let n = items.len();
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut self.clist, n, fit);
        for k in top..(top + fit).min(n) {
            let c = &items[k];
            let ry = list.y + (k - top) as f32 * row_h;
            if ry + row_h > list.y + list.h + 0.5 {
                break;
            }
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let chosen = self.shown == Shown::Commit(c.sha.clone());
            let sel = k == self.clist.sel && on;
            let hot = g.hovered(rr);
            if chosen || sel || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen || sel, hot, ACCENT);
            }
            let subj = g.fit(13.0 * s, &c.subject(), Face::Sans, rr.w - 16.0 * s);
            g.text(rr.x + 8.0 * s, ry + 2.0 * s, 13.0 * s, TEXT, &subj, Face::Sans);
            let ago = if c.when == 0 { String::new() } else { format!(" \u{b7} {}", iyakki_agent::k8s::age_text(now.saturating_sub(c.when))) };
            let meta = g.fit(11.5 * s, &format!("{}  {}{}", c.short(), c.author, ago), Face::Mono, rr.w - 16.0 * s);
            g.text(rr.x + 8.0 * s, ry + 2.0 * s + g.line_height(13.0 * s, Face::Sans) + 1.0 * s, 11.5 * s, DIM, &meta, Face::Mono);
            g.hit(rr, Event::Tab(P_COM + k), &c.subject());
        }
        sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
    }

    fn draw_viewer(&mut self, g: &mut Gfx, r: R, s: f32, on: bool, back: bool) {
        self.panel(g, r, s, on);
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let mut y = r.y + 6.0 * s;
        if back {
            let size = 12.5 * s;
            g.button(x, y, size, "Back", MUTED, Event::Tab(P_BACK));
            y += size * 2.1 + 6.0 * s;
        }
        if self.shown == Shown::Nothing {
            para(g, x, y + 2.0 * s, w, 13.5 * s, DIM, "Choose a changed file or a commit (Enter or a click) to read its diff. o opens the file in the editor.", 4, true);
            return;
        }
        let t = g.fit(13.0 * s, &self.title, Face::Bold, w);
        g.text(x, y, 13.0 * s, KEY, &t, Face::Bold);
        y += g.line_height(13.0 * s, Face::Bold) + 4.0 * s;
        let rect = R::new(x, y, w, (r.y + r.h - y - 6.0 * s).max(10.0));
        self.rects[2] = rect;
        draw_text(g, rect, s, &mut self.text, false);
    }
}

#[allow(dead_code)]
fn unused(_: BrowseErr) {
    let _ = with_alpha(DIM, 0);
}
