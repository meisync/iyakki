//! GIT: the view of one repository (docs/GIT-TILE.md): Files (a tree that opens folder by folder, a viewer for text, README, pictures and
//! binary files), Commits (the last 50, with the message and the diff of a chosen commit), Branches and tags, Issues and pull requests
//! (read-only) and Info. The data comes from a `RepoReader`: the host's API for a repository of a connection, git for a clone on this
//! computer. Every load is one request on its own thread, started only while the tile is on screen; leaving the repository drops what was
//! still on its way (a request that already left finishes and is thrown away).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_agent::gitbrowse::{self as gb, BrowseErr, CommitInfo, Content, Diff, Dir, EntryKind, FileData, IssueInfo, LineKind, LocalClone, RefInfo, RepoItem, RepoReader};
use iyakki_agent::gitconn::Connection;
use tiny_skia::Pixmap;

use super::git_tile::*;
use super::k8s_tile::{Line, ListState, TextView};
use super::AppCx;
use crate::fx::{AMBER, CYAN};
use crate::ui::{Key, DIM, ERR, OK, R, TEXT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Files,
    Commits,
    Branches,
    Issues,
    Info,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Files, Tab::Commits, Tab::Branches, Tab::Issues, Tab::Info];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Commits => "Commits",
            Tab::Branches => "Branches",
            Tab::Issues => "Issues/PRs",
            Tab::Info => "Info",
        }
    }
}

/// One line of the file tree: a file or folder, or a note ("Loading...").
#[derive(Clone, Debug)]
pub(super) struct FRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub kind: Option<EntryKind>,
    pub size: u64,
    pub open: bool,
    pub note: bool,
}

/// What the viewer shows for the open file.
pub(super) enum Shown {
    Loading,
    Text,
    Md,
    Image(Arc<Pixmap>),
    Binary(String),
    TooBig,
    Failed(String),
}

pub(super) struct MdView {
    pub src: String,
    pub layout: Option<(i32, crate::mdtext::Layout)>,
    pub top: f32,
    pub rect: R,
}

pub(super) struct FileOpen {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub slot: Slot<FileData>,
    pub shown: Shown,
    pub text: TextView,
    pub md: Option<MdView>,
    /// Markdown shown as its source.
    pub raw: bool,
    pub bytes: usize,
}

pub(super) struct Files {
    pub dirs: HashMap<String, Slot<Dir>>,
    pub open: HashSet<String>,
    pub rows: Vec<FRow>,
    pub list: ListState,
    pub file: Option<FileOpen>,
    pub readme_done: bool,
    pub dirty: bool,
}

pub(super) struct Commits {
    pub slot: Slot<Vec<CommitInfo>>,
    pub list: ListState,
    /// The commit whose message and diff are shown.
    pub open: Option<String>,
    pub diff: Slot<Diff>,
    pub files: ListState,
    pub text: TextView,
    pub msg: Vec<String>,
}

pub(super) struct Refs {
    pub branches: Slot<Vec<RefInfo>>,
    pub tags: Slot<Vec<RefInfo>>,
    pub list: ListState,
}

pub(super) struct Issues {
    pub prs: bool,
    pub issues: Slot<Vec<IssueInfo>>,
    pub pulls: Slot<Vec<IssueInfo>>,
    pub list: ListState,
    pub open: Option<u64>,
    pub body: TextView,
}

#[derive(Default)]
pub(super) struct Rects {
    pub left: R,
    pub right: R,
}

pub(super) struct RepoView {
    pub src_key: String,
    pub conn: Option<Connection>,
    pub item: RepoItem,
    pub clone: Option<LocalClone>,
    pub reader: Arc<dyn RepoReader>,
    pub cancel: Arc<AtomicBool>,
    pub tab: Tab,
    /// The branch or tag being looked at (empty: the host's default).
    pub rf: String,
    pub files: Files,
    pub commits: Commits,
    pub refs: Refs,
    pub issues: Issues,
    pub right: bool,
    /// Phone width: the right pane is a page of its own.
    pub page_detail: bool,
    pub rects: Rects,
}

impl Drop for RepoView {
    fn drop(&mut self) {
        // what is still waiting to start is never started; a request already on its way ends by itself
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn mk_files() -> Files {
    Files { dirs: HashMap::new(), open: HashSet::new(), rows: vec![], list: ListState::default(), file: None, readme_done: false, dirty: true }
}
fn mk_commits() -> Commits {
    Commits { slot: Slot::new(), list: ListState::default(), open: None, diff: Slot::new(), files: ListState::default(), text: TextView::default(), msg: vec![] }
}

impl RepoView {
    pub fn new(src_key: String, conn: Option<Connection>, item: RepoItem, clone: Option<LocalClone>, reader: Arc<dyn RepoReader>, rf: String) -> RepoView {
        RepoView {
            src_key,
            conn,
            item,
            clone,
            reader,
            cancel: Arc::new(AtomicBool::new(false)),
            tab: Tab::Files,
            rf,
            files: mk_files(),
            commits: mk_commits(),
            refs: Refs { branches: Slot::new(), tags: Slot::new(), list: ListState::default() },
            issues: Issues { prs: false, issues: Slot::new(), pulls: Slot::new(), list: ListState::default(), open: None, body: TextView::default() },
            right: false,
            page_detail: false,
            rects: Rects::default(),
        }
    }

    /// The branch or tag as the header shows it.
    pub fn ref_label(&self) -> String {
        if self.rf.is_empty() { "default branch".into() } else { self.rf.clone() }
    }

    /// The names the reference chooser lists: (name, tag, default).
    pub fn ref_rows(&self) -> Vec<(String, bool, bool)> {
        let mut v: Vec<(String, bool, bool)> = self.refs.branches.ready().map(|b| b.iter().map(|r| (r.name.clone(), false, r.default)).collect()).unwrap_or_default();
        if v.is_empty() && !self.item.default_branch.is_empty() {
            v.push((self.item.default_branch.clone(), false, true));
        }
        if let Some(t) = self.refs.tags.ready() {
            v.extend(t.iter().map(|r| (r.name.clone(), true, false)));
        }
        v
    }

    /// What the viewer shows as plain text, for Copy.
    pub fn copy_text(&self) -> Option<String> {
        match self.tab {
            Tab::Files => {
                let f = self.files.file.as_ref()?;
                match &f.shown {
                    Shown::Md if !f.raw => f.md.as_ref().map(|m| m.src.clone()),
                    Shown::Text | Shown::Md => Some(f.text.lines.iter().map(|l| l.text.chars().skip(7).collect::<String>()).collect::<Vec<_>>().join("\n")),
                    _ => None,
                }
            }
            Tab::Commits => {
                let t = self.commits.text.text();
                let m = self.commits.msg.join("\n");
                (!m.is_empty() || !t.is_empty()).then(|| if t.is_empty() { m } else { format!("{m}\n\n{t}") })
            }
            Tab::Issues => (!self.issues.body.lines.is_empty()).then(|| self.issues.body.text()),
            _ => None,
        }
    }

    // ---------------------------------------------------------------------------------------- the file tree

    pub fn rebuild_rows(&mut self) {
        fn walk(f: &Files, path: &str, depth: usize, out: &mut Vec<FRow>) {
            let Some(slot) = f.dirs.get(path) else { return };
            match &slot.st {
                Load::Ready(d) => {
                    for e in &d.entries {
                        let open = e.kind == EntryKind::Dir && f.open.contains(&e.path);
                        out.push(FRow { path: e.path.clone(), name: e.name.clone(), depth, kind: Some(e.kind), size: e.size, open, note: false });
                        if open {
                            walk(f, &e.path, depth + 1, out);
                        }
                    }
                    if d.cut {
                        out.push(FRow { path: String::new(), name: format!("(only the first {} entries are listed)", gb::DIR_MAX), depth, kind: None, size: 0, open: false, note: true });
                    }
                }
                Load::Loading | Load::None => out.push(FRow { path: String::new(), name: "Loading\u{2026}".into(), depth, kind: None, size: 0, open: false, note: true }),
                Load::Failed(e) => out.push(FRow { path: String::new(), name: e.text(), depth, kind: None, size: 0, open: false, note: true }),
            }
        }
        let mut rows = Vec::new();
        walk(&self.files, "", 0, &mut rows);
        self.files.rows = rows;
        self.files.dirty = false;
    }

    /// Opens or closes the folder at row `i`, or opens the file there.
    pub fn activate_row(&mut self, i: usize) -> Option<String> {
        let r = self.files.rows.get(i)?.clone();
        if r.note {
            return None;
        }
        match r.kind? {
            EntryKind::Dir => {
                if r.open {
                    self.files.open.remove(&r.path);
                } else {
                    self.files.open.insert(r.path.clone());
                    self.files.dirs.entry(r.path.clone()).or_insert_with(Slot::new);
                }
                self.files.dirty = true;
                self.rebuild_rows();
                None
            }
            EntryKind::File => {
                self.open_file(&r.path, &r.name, r.size);
                self.right = true;
                self.page_detail = true;
                None
            }
            EntryKind::Link => Some(format!("{} is a link; the viewer does not follow links.", r.name)),
            EntryKind::Submodule => Some(format!("{} is a submodule: another repository, not part of this one's files.", r.name)),
        }
    }

    pub fn open_file(&mut self, path: &str, name: &str, size: u64) {
        self.files.file = Some(FileOpen { path: path.into(), name: name.into(), size, slot: Slot::new(), shown: Shown::Loading, text: TextView::default(), md: None, raw: false, bytes: 0 });
    }

    fn set_file_content(f: &mut FileOpen, data: FileData) {
        f.bytes = data.bytes.len();
        f.size = data.size;
        if data.too_big {
            f.shown = Shown::TooBig;
            return;
        }
        match gb::classify(&f.name, &data.bytes) {
            Content::Text(t) => {
                let mut lines = Vec::new();
                for (i, l) in t.lines().enumerate() {
                    if i >= MAX_TEXT_LINES {
                        lines.push(Line::plain(format!("{:>5}  (the rest of the file is not shown)", ""), DIM));
                        break;
                    }
                    lines.push(Line::plain(format!("{:>5}  {}", i + 1, l.replace('\t', "    ").replace('\r', "")), TEXT));
                }
                f.text.set(lines, false);
                if gb::is_markdown(&f.name) {
                    f.md = Some(MdView { src: t, layout: None, top: 0.0, rect: R::default() });
                    f.shown = Shown::Md;
                } else {
                    f.shown = Shown::Text;
                }
            }
            Content::Image => {
                if let Some((w, h)) = super::lab_pic::header_size(&data.bytes) {
                    if w as u64 * h as u64 > 64_000_000 {
                        f.shown = Shown::Binary(format!("picture, {w}\u{d7}{h} pixels (too large to draw)"));
                        return;
                    }
                }
                f.shown = match super::browser_images::to_pixmap(&data.bytes) {
                    Ok(pm) => Shown::Image(super::lab_pic::thumb(&pm, 1600)),
                    Err(e) => Shown::Binary(format!("picture that could not be drawn ({e})")),
                };
            }
            Content::Binary(label) => f.shown = Shown::Binary(label),
        }
    }

    // ---------------------------------------------------------------------------------------- commits

    pub fn open_commit(&mut self, i: usize) {
        let Some(c) = self.commits.slot.ready().and_then(|v| v.get(i)).cloned() else { return };
        self.commits.open = Some(c.sha.clone());
        self.commits.msg = c.message.lines().map(String::from).collect();
        self.commits.diff.reset();
        self.commits.files = ListState::default();
        self.commits.text = TextView::default();
        self.right = true;
        self.page_detail = true;
    }

    /// The lines of the chosen file of the diff, with colours.
    pub fn rebuild_diff(&mut self) {
        let sel = self.commits.files.sel;
        let mut lines = Vec::new();
        if let Some(d) = self.commits.diff.ready() {
            if d.too_large {
                lines.push(Line::plain(format!("This diff is larger than {} KB, so it is not shown here. The files it changes are listed on the left.", gb::DIFF_MAX / 1024), AMBER));
                lines.push(Line::plain("Open the commit on the host's web page (w) to read all of it.", DIM));
            } else if d.files.is_empty() {
                lines.push(Line::plain("This commit changes no files (an empty or merge commit).", DIM));
            } else if let Some(f) = d.files.get(sel.min(d.files.len() - 1)) {
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
        self.commits.text.set(lines, false);
    }

    // ---------------------------------------------------------------------------------------- issues

    pub fn open_issue(&mut self, i: usize) {
        let list = if self.issues.prs { self.issues.pulls.ready() } else { self.issues.issues.ready() };
        let Some(it) = list.and_then(|v| v.get(i)).cloned() else { return };
        let mut lines = vec![
            Line::plain(format!("#{} {}", it.number, it.title), TEXT),
            Line::plain(format!("{} \u{b7} opened by {}{}", it.state, if it.author.is_empty() { "someone" } else { &it.author }, if it.labels.is_empty() { String::new() } else { format!(" \u{b7} {}", it.labels.join(", ")) }), DIM),
            Line::plain("", TEXT),
        ];
        if it.body.trim().is_empty() {
            lines.push(Line::plain("(no description)", DIM));
        } else {
            for l in it.body.lines().take(2000) {
                lines.push(Line::plain(l.replace('\t', "    ").replace('\r', ""), TEXT));
            }
        }
        lines.push(Line::plain("", TEXT));
        lines.push(Line::plain("Read-only: nothing can be written or commented on from this tile.", DIM));
        self.issues.open = Some(it.number);
        self.issues.body.set(lines, false);
        self.right = true;
        self.page_detail = true;
    }
}

/// The most lines of a text file the viewer builds.
pub(super) const MAX_TEXT_LINES: usize = 50_000;

fn guard<T>(cancel: &AtomicBool, f: impl FnOnce() -> Result<T, BrowseErr>) -> Result<T, BrowseErr> {
    if cancel.load(Ordering::SeqCst) { Err(BrowseErr::Persistent("cancelled".into())) } else { f() }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Want {
    Dir(String),
    File,
    Commits,
    Diff,
    Branches,
    Tags,
    Issues,
    Pulls,
}

impl GitApp {
    // ---------------------------------------------------------------------------------------- opening and leaving

    pub(super) fn open_repo(&mut self, cx: &AppCx, row: &Row) {
        let view = match self.src().clone() {
            Src::Conn(c) => {
                let Some(item) = row.item.clone() else { return };
                let browser = match self.browser(cx, &c) {
                    Ok(b) => b,
                    Err(e) => {
                        self.say(e.text(), false);
                        return;
                    }
                };
                let reader = Arc::new(browser.repo(&item.full_name).with_default(&item.default_branch));
                cx.audit.record(&self.user, "git.browse.open", &format!("{} {}", c.name, item.full_name));
                let clone = row.clone.and_then(|i| self.locals.list.get(i).cloned());
                let rf = item.default_branch.clone();
                RepoView::new(c.id.clone(), Some(c), item, clone, reader, rf)
            }
            Src::Local => {
                let Some(cl) = row.clone.and_then(|i| self.locals.list.get(i).cloned()) else { return };
                cx.audit.record(&self.user, "git.browse.open", &format!("local {}", cl.name));
                let reader = Arc::new(gb::LocalRepo::new(self.rig.run.clone(), &cl.dir, &self.rig.scratch));
                let item = RepoItem { full_name: cl.name.clone(), description: cl.remote.clone().unwrap_or_default(), default_branch: cl.branch.clone(), web_url: cl.remote.as_deref().map(web_of).unwrap_or_default(), private: false, ..Default::default() };
                let rf = cl.branch.clone();
                RepoView::new("local".into(), None, item, Some(cl), reader, rf)
            }
        };
        self.view = Some(Box::new(view));
        self.focus = Focus::Left;
        self.status = None;
        self.overlay = None;
    }

    pub(super) fn leave_repo(&mut self) {
        // dropping the view cancels what was still waiting to start
        self.view = None;
        self.focus = Focus::List;
        self.overlay = None;
        self.status = None;
        self.order_key_reset();
    }

    /// One step back inside the repository: from the right pane of a phone to its list. True when something was undone.
    pub(super) fn view_back(&mut self) -> bool {
        let Some(v) = &mut self.view else { return false };
        if v.page_detail {
            v.page_detail = false;
            v.right = false;
            self.focus = Focus::Left;
            return true;
        }
        false
    }

    pub(super) fn refresh_view(&mut self) {
        let Some(v) = &mut self.view else { return };
        fn go<T: Send + 'static>(s: &mut Slot<T>) {
            s.fails = 0;
            s.stale = None;
            s.retry_at = Some(0);
        }
        for s in v.files.dirs.values_mut() {
            go(s);
        }
        if let Some(f) = &mut v.files.file {
            go(&mut f.slot);
        }
        go(&mut v.commits.slot);
        go(&mut v.refs.branches);
        go(&mut v.refs.tags);
        go(&mut v.issues.issues);
        go(&mut v.issues.pulls);
        if v.commits.open.is_some() {
            go(&mut v.commits.diff);
        }
        self.rate_until.remove(&v.src_key.clone());
        self.status = Some(("Refreshing\u{2026}".into(), true));
    }

    pub(super) fn set_tab(&mut self, i: usize) {
        let Some(v) = &mut self.view else { return };
        if let Some(t) = Tab::ALL.get(i) {
            v.tab = *t;
            v.right = false;
            v.page_detail = false;
            self.focus = Focus::Left;
        }
    }

    pub(super) fn set_sub(&mut self, prs: bool) {
        if let Some(v) = &mut self.view {
            v.issues.prs = prs;
            v.issues.list = ListState::default();
            v.issues.open = None;
            v.issues.body = TextView::default();
        }
    }

    pub(super) fn open_refs(&mut self) {
        if self.view.is_some() {
            self.overlay = Some(Overlay::Refs);
            self.src_list = ListState::default();
            if let Some(v) = &self.view {
                let cur = v.ref_rows().iter().position(|r| r.0 == v.rf).unwrap_or(0);
                self.src_list.sel = cur;
                self.src_list.reveal = true;
            }
        }
    }

    pub(super) fn pick_ref(&mut self, i: usize) {
        let Some(v) = &mut self.view else { return };
        let Some((name, _, _)) = v.ref_rows().get(i).cloned() else { return };
        self.overlay = None;
        if name == v.rf {
            return;
        }
        v.rf = name;
        v.files = mk_files();
        v.commits = mk_commits();
        v.right = false;
        v.page_detail = false;
        v.tab = if v.tab == Tab::Branches { Tab::Files } else { v.tab };
    }

    // ---------------------------------------------------------------------------------------- loading

    fn rate_wait_view(&self) -> Option<u64> {
        let k = self.view.as_ref()?.src_key.clone();
        let now = self.now_ms();
        self.rate_until.get(&k).copied().filter(|t| *t > now)
    }

    /// What the open tab needs that is not there yet (or is due again).
    fn wants(&self, now: u64) -> Vec<Want> {
        let Some(v) = &self.view else { return vec![] };
        let mut w = Vec::new();
        let due = |s: bool, d: bool| s || d;
        match v.tab {
            Tab::Files => {
                if v.files.dirs.get("").is_none_or(|s| due(s.untouched(), s.due(now))) {
                    w.push(Want::Dir(String::new()));
                }
                for (p, s) in &v.files.dirs {
                    if !p.is_empty() && v.files.open.contains(p) && (s.untouched() || s.due(now)) {
                        w.push(Want::Dir(p.clone()));
                    }
                }
                if let Some(f) = &v.files.file {
                    if f.slot.untouched() || f.slot.due(now) {
                        w.push(Want::File);
                    }
                }
            }
            Tab::Commits => {
                if v.commits.slot.untouched() || v.commits.slot.due(now) {
                    w.push(Want::Commits);
                }
                if v.commits.open.is_some() && (v.commits.diff.untouched() || v.commits.diff.due(now)) {
                    w.push(Want::Diff);
                }
            }
            Tab::Branches => {
                if v.refs.branches.untouched() || v.refs.branches.due(now) {
                    w.push(Want::Branches);
                }
                if v.refs.tags.untouched() || v.refs.tags.due(now) {
                    w.push(Want::Tags);
                }
            }
            Tab::Issues => {
                if !v.issues.prs && (v.issues.issues.untouched() || v.issues.issues.due(now)) {
                    w.push(Want::Issues);
                }
                if v.issues.prs && (v.issues.pulls.untouched() || v.issues.pulls.due(now)) {
                    w.push(Want::Pulls);
                }
            }
            Tab::Info => {}
        }
        // the chooser of the branch or tag needs both lists
        if self.overlay == Some(Overlay::Refs) {
            if v.refs.branches.untouched() || v.refs.branches.due(now) {
                w.push(Want::Branches);
            }
            if v.refs.tags.untouched() || v.refs.tags.due(now) {
                w.push(Want::Tags);
            }
        }
        w.dedup();
        w
    }

    pub(super) fn drive_view(&mut self, _cx: &AppCx, now: u64) {
        let wait = self.rate_wait_view();
        for w in self.wants(now) {
            self.start_want(w, wait);
        }
        // the README is shown first
        let Some(v) = &mut self.view else { return };
        if v.tab == Tab::Files && !v.files.readme_done && v.files.file.is_none() {
            if let Some(d) = v.files.dirs.get("").and_then(|s| s.ready()) {
                v.files.readme_done = true;
                if let Some(e) = gb::find_readme(d).cloned() {
                    v.open_file(&e.path, &e.name, e.size);
                }
            }
        }
    }

    fn start_want(&mut self, w: Want, wait: Option<u64>) {
        let Some(v) = &mut self.view else { return };
        let (reader, cancel, rf) = (v.reader.clone(), v.cancel.clone(), v.rf.clone());
        macro_rules! go {
            ($slot:expr, $body:expr) => {{
                let slot = $slot;
                match wait {
                    Some(until) => slot.wait_until(until),
                    None => slot.start(move || guard(&cancel, $body)),
                }
            }};
        }
        match w {
            Want::Dir(p) => {
                let slot = v.files.dirs.entry(p.clone()).or_insert_with(Slot::new);
                go!(slot, move || reader.dir(&rf, &p));
                v.files.dirty = true;
            }
            Want::File => {
                if let Some(f) = &mut v.files.file {
                    let (path, size) = (f.path.clone(), f.size);
                    go!(&mut f.slot, move || reader.file(&rf, &path, size));
                }
            }
            Want::Commits => go!(&mut v.commits.slot, move || reader.commits(&rf)),
            Want::Diff => {
                if let Some(sha) = v.commits.open.clone() {
                    go!(&mut v.commits.diff, move || reader.diff(&sha));
                }
            }
            Want::Branches => go!(&mut v.refs.branches, move || reader.branches()),
            Want::Tags => go!(&mut v.refs.tags, move || reader.tags()),
            Want::Issues => go!(&mut v.issues.issues, move || reader.issues(false)),
            Want::Pulls => go!(&mut v.issues.pulls, move || reader.issues(true)),
        }
    }

    /// Collects the answers; a rate limit makes every slot of the host wait; a failure goes to the log.
    pub(super) fn poll_view(&mut self, now: u64) {
        let Some(v) = &mut self.view else { return };
        let mut errs: Vec<(String, BrowseErr)> = Vec::new();
        let mut changed_rows = false;
        for (p, s) in v.files.dirs.iter_mut() {
            if let Some(r) = s.poll(now) {
                changed_rows = true;
                if let Err(e) = r {
                    errs.push((format!("folder {}", if p.is_empty() { "/" } else { p }), e));
                }
            }
        }
        if changed_rows {
            v.files.dirty = true;
        }
        if let Some(f) = &mut v.files.file {
            if let Some(r) = f.slot.poll(now) {
                match r {
                    Ok(()) => {
                        if let Load::Ready(d) = std::mem::replace(&mut f.slot.st, Load::None) {
                            RepoView::set_file_content(f, d);
                        }
                        // the bytes are not kept: what is shown is
                        f.slot.st = Load::Ready(FileData::default());
                    }
                    Err(e) => {
                        f.shown = Shown::Failed(e.text());
                        errs.push((format!("file {}", f.path), e));
                    }
                }
            }
        }
        macro_rules! plain {
            ($slot:expr, $what:expr) => {
                if let Some(Err(e)) = $slot.poll(now) {
                    errs.push(($what.to_string(), e));
                }
            };
        }
        plain!(v.commits.slot, "commits");
        if let Some(r) = v.commits.diff.poll(now) {
            match r {
                Ok(()) => v.rebuild_diff(),
                Err(e) => errs.push(("diff".into(), e)),
            }
        }
        plain!(v.refs.branches, "branches");
        plain!(v.refs.tags, "tags");
        plain!(v.issues.issues, "issues");
        plain!(v.issues.pulls, "pull requests");
        if v.files.dirty {
            v.rebuild_rows();
        }
        let key = v.src_key.clone();
        let any = !errs.is_empty() || changed_rows;
        for (what, e) in errs {
            if what != "cancelled" {
                self.log_push(&what, &e);
            }
            if let BrowseErr::Rate(n) = e {
                self.rate_until.insert(key.clone(), now + n * 1000);
            }
        }
        if any {
            self.end_refreshing();
        }
    }

    // ---------------------------------------------------------------------------------------- keys and clicks

    pub(super) fn view_wheel(&mut self, rows: i32, x: i32, y: i32) {
        let Some(v) = &mut self.view else { return };
        if v.rects.right.contains(x, y) {
            match v.tab {
                Tab::Files => {
                    if let Some(f) = &mut v.files.file {
                        if let (Shown::Md, false) = (&f.shown, f.raw) {
                            if let Some(m) = &mut f.md {
                                m.top = (m.top + rows as f32 * 40.0).max(0.0);
                            }
                        } else {
                            f.text.scroll(rows);
                        }
                    }
                }
                Tab::Commits => v.commits.text.scroll(rows),
                Tab::Issues => v.issues.body.scroll(rows),
                _ => {}
            }
        } else if v.rects.left.contains(x, y) {
            match v.tab {
                Tab::Files => {
                    let n = v.files.rows.len();
                    v.files.list.scroll(n, rows);
                }
                Tab::Commits => {
                    let n = v.commits.slot.ready().map_or(0, Vec::len);
                    v.commits.list.scroll(n, rows);
                }
                Tab::Branches => {
                    let n = v.ref_rows().len();
                    v.refs.list.scroll(n, rows);
                }
                Tab::Issues => {
                    let n = if v.issues.prs { v.issues.pulls.ready().map_or(0, Vec::len) } else { v.issues.issues.ready().map_or(0, Vec::len) };
                    v.issues.list.scroll(n, rows);
                }
                Tab::Info => {}
            }
        }
    }

    /// A click on a row of the repository view.
    pub(super) fn view_click(&mut self, n: usize) {
        let Some(v) = &mut self.view else { return };
        if n >= T_REFROW {
            return;
        }
        if n >= T_IROW {
            let i = n - T_IROW;
            v.issues.list.sel = i;
            self.focus = Focus::Left;
            v.open_issue(i);
        } else if n >= T_BROW {
            let i = n - T_BROW;
            v.refs.list.sel = i;
            self.use_branch_row(i);
        } else if n >= T_DROW {
            v.commits.files.sel = n - T_DROW;
            v.rebuild_diff();
            self.focus = Focus::Right;
        } else if n >= T_CROW {
            let i = n - T_CROW;
            v.commits.list.sel = i;
            v.open_commit(i);
            self.focus = Focus::Left;
        } else if n >= T_FROW {
            let i = n - T_FROW;
            v.files.list.sel = i;
            self.focus = Focus::Left;
            if let Some(m) = v.activate_row(i) {
                self.say(m, false);
            }
        }
    }

    fn use_branch_row(&mut self, i: usize) {
        let Some(v) = &mut self.view else { return };
        let rows = v.ref_rows();
        if rows.get(i).is_some() {
            self.pick_ref(i);
            if let Some(v) = &mut self.view {
                v.tab = Tab::Files;
            }
        }
    }

    pub(super) fn view_key(&mut self, k: &Key, cx: &AppCx) -> super::Flow {
        let _ = cx;
        let phone = self.mode == Mode::Phone;
        match k {
            Key::Esc | Key::Backspace => {
                if self.card.is_some() {
                    self.cancel_card();
                } else if let Some(f) = self.back() {
                    return f;
                }
                return super::Flow::Stay;
            }
            Key::Char(c @ '1'..='5') => {
                self.set_tab(*c as usize - '1' as usize);
                return super::Flow::Stay;
            }
            Key::Char('g') => {
                self.open_refs();
                return super::Flow::Stay;
            }
            Key::Char('c') => {
                self.copy_view_text();
                return super::Flow::Stay;
            }
            Key::Tab | Key::BackTab => {
                if let Some(v) = &mut self.view {
                    if !phone && matches!(v.tab, Tab::Files | Tab::Commits | Tab::Issues) {
                        v.right = !v.right;
                        self.focus = if v.right { Focus::Right } else { Focus::Left };
                    }
                }
                return super::Flow::Stay;
            }
            _ => {}
        }
        let right = self.focus == Focus::Right && self.view.as_ref().is_some_and(|v| v.right || phone);
        let Some(v) = &mut self.view else { return super::Flow::Stay };
        let nk = nav_of(k);
        match (v.tab, k) {
            (Tab::Files, Key::Char('v')) => {
                if let Some(f) = &mut v.files.file {
                    f.raw = !f.raw;
                }
            }
            (Tab::Issues, Key::Char('i')) => self.set_sub(false),
            (Tab::Issues, Key::Char('p')) => self.set_sub(true),
            (Tab::Files, Key::Right) => {
                let i = v.files.list.sel;
                if v.files.rows.get(i).is_some_and(|r| r.kind == Some(EntryKind::Dir) && !r.open) {
                    v.activate_row(i);
                } else {
                    v.right = true;
                    self.focus = Focus::Right;
                }
            }
            (Tab::Files, Key::Left) => {
                let i = v.files.list.sel;
                if right {
                    v.right = false;
                    self.focus = Focus::Left;
                } else if let Some(r) = v.files.rows.get(i).cloned() {
                    if r.kind == Some(EntryKind::Dir) && r.open {
                        v.activate_row(i);
                    } else if r.depth > 0 {
                        // to the folder that holds it
                        let parent = r.path.rsplit_once('/').map_or("", |p| p.0).to_string();
                        if let Some(j) = v.files.rows.iter().position(|x| x.path == parent && x.kind == Some(EntryKind::Dir)) {
                            v.files.list.sel = j;
                            v.files.list.reveal = true;
                        }
                    }
                }
            }
            (Tab::Commits, Key::Right) | (Tab::Issues, Key::Right) => {
                v.right = true;
                self.focus = Focus::Right;
            }
            (Tab::Commits, Key::Left) | (Tab::Issues, Key::Left) => {
                v.right = false;
                self.focus = Focus::Left;
            }
            (Tab::Commits, Key::Char('[')) | (Tab::Commits, Key::Char(']')) => {
                let n = v.commits.diff.ready().map_or(0, |d| d.files.len());
                v.commits.files.sel = if *k == Key::Char(']') { (v.commits.files.sel + 1).min(n.saturating_sub(1)) } else { v.commits.files.sel.saturating_sub(1) };
                v.rebuild_diff();
            }
            (_, Key::Enter) => match v.tab {
                Tab::Files => {
                    let i = v.files.list.sel;
                    if let Some(m) = v.activate_row(i) {
                        self.say(m, false);
                    }
                }
                Tab::Commits => {
                    let i = v.commits.list.sel;
                    v.open_commit(i);
                }
                Tab::Branches => {
                    let i = v.refs.list.sel;
                    self.use_branch_row(i);
                }
                Tab::Issues => {
                    let i = v.issues.list.sel;
                    v.open_issue(i);
                }
                Tab::Info => {}
            },
            (_, _) if nk.is_some() => {
                let nk = nk.unwrap();
                if right {
                    let page = v.rects.right.h.max(40.0) as i32 / 18;
                    let t = match v.tab {
                        Tab::Files => v.files.file.as_mut().map(|f| &mut f.text),
                        Tab::Commits => Some(&mut v.commits.text),
                        Tab::Issues => Some(&mut v.issues.body),
                        _ => None,
                    };
                    if let Some(t) = t {
                        match nk {
                            NavKey::Up => t.scroll(-1),
                            NavKey::Down => t.scroll(1),
                            NavKey::PageUp => t.scroll(-page.max(1)),
                            NavKey::PageDown => t.scroll(page.max(1)),
                            NavKey::Home => t.home(),
                            NavKey::End => t.end(),
                        }
                    }
                    if let (Tab::Files, Some(f)) = (v.tab, &mut v.files.file) {
                        if let (Shown::Md, false, Some(m)) = (&f.shown, f.raw, &mut f.md) {
                            match nk {
                                NavKey::Up => m.top = (m.top - 40.0).max(0.0),
                                NavKey::Down => m.top += 40.0,
                                NavKey::PageUp => m.top = (m.top - m.rect.h).max(0.0),
                                NavKey::PageDown => m.top += m.rect.h,
                                NavKey::Home => m.top = 0.0,
                                NavKey::End => m.top = f32::MAX / 4.0,
                            }
                        }
                    }
                } else {
                    match v.tab {
                        Tab::Files => {
                            let n = v.files.rows.len();
                            nav(&mut v.files.list, n, nk);
                        }
                        Tab::Commits => {
                            let n = v.commits.slot.ready().map_or(0, Vec::len);
                            nav(&mut v.commits.list, n, nk);
                        }
                        Tab::Branches => {
                            let n = v.ref_rows().len();
                            nav(&mut v.refs.list, n, nk);
                        }
                        Tab::Issues => {
                            let n = if v.issues.prs { v.issues.pulls.ready().map_or(0, Vec::len) } else { v.issues.issues.ready().map_or(0, Vec::len) };
                            nav(&mut v.issues.list, n, nk);
                        }
                        Tab::Info => {}
                    }
                }
            }
            _ => {}
        }
        super::Flow::Stay
    }

    fn order_key_reset(&mut self) {
        self.rows_dirty();
    }
}
