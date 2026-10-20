//! The AI IDE's feature tracking board: a project's features file (a markdown table such as `FEATURES.md`) as a kanban by
//! status, a sortable table, and a phase view, with a detail drawer. Edits change only the touched cells of the file
//! (see `iyakki_agent::featureboard`); the file is written atomically with a `.bak` for one step of undo, watched for
//! changes made by anyone else, and kept in sync with the editor tab of the same file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iyakki_agent::featureboard::{self as fb, Board, BoardFile, Disk, Feature, NewFeature, Update};

use iyakki_agent::tasks::AutoAssign;

use super::widgets::{Form, FormEvent};
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::textsel;
use crate::ui::{Event, Key, Nav, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// Dependencies between rows and the agents that work on them (the Graph view, the Agents strip, the agent panel).
#[path = "board_deps.rs"]
pub mod deps;

#[cfg(test)]
#[path = "board_deps_tests.rs"]
mod deps_tests;

/// `Event::Sub` code of the board's buttons and keys.
pub const SUB: u8 = 32;
pub const TAB_BASE: usize = 9_400_000;
/// Tabs from TAB_BASE + REVIEW_DONE are the Mark done buttons of review cards (by card index).
const REVIEW_DONE: usize = 2_000;
pub const CARD_BASE: usize = 9_420_000;
pub const ROW_BASE: usize = 100_000;
pub const FOCUS_FILTER: usize = 9_400_001;
pub const DRAWER_AREA: usize = 62_003;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Kanban,
    Table,
    Phases,
    /// The dependency graph of the rows, with the critical path.
    Graph,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Kanban, Mode::Table, Mode::Phases, Mode::Graph];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Kanban => "Board",
            Mode::Table => "Table",
            Mode::Phases => "Phases",
            Mode::Graph => "Graph",
        }
    }
}

/// What the board asks the IDE to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardAct {
    None,
    /// The key or click was used.
    Used,
    /// Open the assistant with this text.
    Ask(String),
    /// Open a file of the project at a line.
    Open(String, usize),
    /// The features file was written (the editor tab of the same file should reload).
    Saved(PathBuf),
    /// Leave the full-screen board / close it.
    Close,
    ToggleFull,
    /// Run the project's test command (the IDE's Tests button).
    RunTests,
    /// Run the project's coverage command.
    RunCoverage,
    /// A control of an agent's panel: the agent's key and the command (`s` stop, `p` pause, `r` resume, `m` message, `o` open its run).
    AgentCmd(String, char),
    /// Start an agent on each of these rows (they are ready: every dependency is done).
    AssignReady(Vec<String>),
    /// Change what happens to rows that become ready.
    AutoAssign(AutoAssign),
    /// Show the Notes tab, on this note when given.
    OpenNotes(Option<String>),
}

enum Modal {
    Add(Form),
    /// A comment is required before the move: (id, the status it moves to).
    Comment(String, String, Form),
    EditComment(String, Form),
    Blocker(String, Form),
    EditTitle(String, Form),
    Delete(String),
    /// The file changed on disk while the board had edits.
    Conflict,
}

pub fn status_color(st: &str) -> [u8; 4] {
    match st {
        "planned" => rgb(120, 170, 255),
        "partial" => AMBER,
        "done" => OK,
        "blocked" => ERR,
        "deprecated" => rgb(140, 150, 185),
        "pending" => CYAN,
        "review" => AMBER,
        _ => MAGENTA,
    }
}

/// "started 09:12 · 3 min" for a row in progress, "done 09:15 · took 3 min" for a finished one (times in the given zone).
pub fn time_line(f: &Feature, now: i64, zone: &iyakki_agent::clock::Zone) -> Option<String> {
    let start = fb::parse_stamp(&f.started)?;
    let hm = |t: i64| iyakki_agent::clock::format(zone, t, "%H:%M");
    match (fb::parse_stamp(&f.finished), f.status.as_str()) {
        (Some(end), _) => Some(format!("done {} · took {}", hm(end), fb::fmt_duration(end - start))),
        (None, "partial") => Some(format!("started {} · {}", hm(start), fb::fmt_duration(now - start))),
        (None, _) => Some(format!("started {}", hm(start))),
    }
}

pub struct BoardView {
    pub file: Option<BoardFile>,
    pub path: PathBuf,
    pub root: PathBuf,
    pub load_error: Option<String>,
    pub mode: Mode,
    pub sel: usize,
    pub status_filter: Option<String>,
    pub phase_filter: Option<String>,
    pub area_filter: Option<String>,
    pub text: TextField,
    pub text_focus: bool,
    /// The filter's caret is in its "on" half this frame (and the board owns the keyboard).
    caret_on: bool,
    pub sort: (usize, bool),
    pub drawer: bool,
    modal: Option<Modal>,
    pub msg: Option<(String, bool)>,
    undo: Vec<Board>,
    last_edit: Option<Instant>,
    /// Wait this long after the last edit before writing (0 in tests).
    pub autosave_after: Duration,
    pub disk_changed: bool,
    last_poll: Instant,
    pub poll_every: Duration,
    last_draw: Option<Instant>,
    drag: Option<String>,
    lane_rects: Vec<(String, R)>,
    card_rects: Vec<(String, R)>,
    first_lane: usize,
    lane_scroll: BTreeMap<String, usize>,
    table_scroll: usize,
    visible_rows: usize,
    pub full: bool,
    /// The workspace is open read-only (another window holds it): nothing may be created or changed.
    pub read_only: bool,
    /// The last test results and coverage of this workspace (`.iyakki/test-results.json`, `.iyakki/coverage.json`).
    pub results: Option<iyakki_agent::testresults::Results>,
    pub coverage: Option<iyakki_agent::coverage::Coverage>,
    quality_stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// Set by the IDE while a test or coverage run is going ("running tests…").
    pub busy: Option<String>,
    /// Show only bugs (rows with a `B-` id).
    pub bugs_only: bool,
    /// The last answer of a coverage request that could not run (the install hint).
    pub cov_note: Option<String>,
    pane: textsel::Pane,
    /// Dependencies and agents (see [`deps`]).
    pub ext: deps::Ext,
}

impl BoardView {
    pub fn new(root: &Path, path: &Path) -> BoardView {
        let mut b = BoardView {
            file: None,
            path: path.into(),
            root: root.into(),
            load_error: None,
            mode: Mode::Kanban,
            sel: 0,
            status_filter: None,
            phase_filter: None,
            area_filter: None,
            text: TextField::new("filter: id, words…  (/)", false),
            text_focus: false,
            caret_on: true,
            sort: (0, true),
            drawer: false,
            modal: None,
            msg: None,
            undo: vec![],
            last_edit: None,
            autosave_after: Duration::from_millis(1500),
            disk_changed: false,
            last_poll: Instant::now(),
            poll_every: Duration::from_secs(1),
            last_draw: None,
            drag: None,
            lane_rects: vec![],
            card_rects: vec![],
            first_lane: 0,
            lane_scroll: BTreeMap::new(),
            table_scroll: 0,
            visible_rows: 10,
            full: false,
            read_only: false,
            results: None,
            coverage: None,
            quality_stamp: (None, None),
            busy: None,
            bugs_only: false,
            cov_note: None,
            pane: textsel::Pane::new(DRAWER_AREA),
            ext: deps::Ext::new(),
        };
        b.reload();
        b
    }

    /// The file a new workspace starts its board from.
    pub const TEMPLATE: &'static str = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 1 · blocked 0 · deprecated 0 · total 1<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Started | Finished | Notes |\n|---|---|---|---|---|\n| F-01 | First feature (rename it, or ask the assistant to plan the work here) | planned | — | Created by IYAKKI. |\n";

    /// Write the starter board when there is no features file yet (never over an existing file) and show it.
    pub fn create_board(&mut self) -> BoardAct {
        if self.read_only {
            self.say("This workspace is open read-only (another window has it), so the board cannot be created here.", false);
            return BoardAct::Used;
        }
        if self.path.exists() {
            self.say("The features file already exists; it was not touched.", false);
            return BoardAct::Used;
        }
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match std::fs::write(&self.path, Self::TEMPLATE) {
            Ok(()) => {
                self.reload();
                self.say("Board created.", true);
                BoardAct::Saved(self.path.clone())
            }
            Err(e) => {
                self.say(format!("Could not create the board: {e}"), false);
                BoardAct::Used
            }
        }
    }

    /// Load the stored test results and coverage when their files changed (a run finished, or another window ran one).
    pub fn refresh_quality(&mut self, force: bool) {
        let root = self.root.clone();
        let m = |f: &str| std::fs::metadata(root.join(".iyakki").join(f)).and_then(|m| m.modified()).ok();
        let stamp = (m("test-results.json"), m("coverage.json"));
        if force || stamp != self.quality_stamp {
            self.quality_stamp = stamp;
            self.results = iyakki_agent::testresults::load(&root);
            self.coverage = iyakki_agent::coverage::load(&root);
            if self.coverage.is_some() {
                self.cov_note = None;
            }
        }
    }

    /// What the board knows about one row's tests and coverage: (tests listed, their last results, coverage of its files).
    pub fn row_quality(&self, f: &Feature) -> (usize, iyakki_agent::testresults::RowTests, Option<(u64, u64)>) {
        let rt = self.results.as_ref().map(|r| iyakki_agent::testresults::match_row(&f.tests, r)).unwrap_or(iyakki_agent::testresults::RowTests { listed: f.tests.len(), not_run: f.tests.len(), ..Default::default() });
        (f.tests.len(), rt, self.coverage.as_ref().and_then(|c| c.for_globs(&f.files)))
    }

    /// "3 tests · 12/14 pass · cov 82%" with its colour, "⚠ no tests" for a started row without any, nothing when the board does not
    /// track tests.
    fn quality_line(&self, f: &Feature) -> Option<(String, [u8; 4])> {
        if !self.tracks_tests() {
            return None;
        }
        let (n, rt, cov) = self.row_quality(f);
        if n == 0 {
            return matches!(f.status.as_str(), "partial" | "done").then(|| ("⚠ no tests".to_string(), AMBER));
        }
        let mut s = format!("{n} test{}", if n == 1 { "" } else { "s" });
        if !rt.label().is_empty() {
            s.push_str(&format!(" · {}", rt.label()));
        }
        if let Some((cv, t)) = cov {
            s.push_str(&format!(" · cov {}%", cv * 100 / t.max(1)));
        }
        let color = if rt.failed > 0 { ERR } else if rt.all_passed() { OK } else { fx::readable(MUTED, rgb(18, 24, 46), 4.5) };
        Some((s, color))
    }

    /// The board tracks tests (a Tests column, or stored results).
    fn tracks_tests(&self) -> bool {
        self.results.is_some() || self.file.as_ref().and_then(|f| f.board.cols().map(|c| c.tests.is_some())).unwrap_or(false)
    }

    /// Tests of the last run that no feature lists.
    pub fn unlinked_tests(&self) -> Vec<String> {
        let (Some(r), Some(f)) = (&self.results, &self.file) else { return vec![] };
        let all: Vec<String> = f.board.features().into_iter().flat_map(|x| x.tests).collect();
        iyakki_agent::testresults::unlinked(&all, r)
    }

    pub fn reload(&mut self) {
        match BoardFile::load(&self.path) {
            Ok(f) => {
                self.load_error = if f.board.has_table() { None } else { Some(format!("{} has no feature table (a markdown table with ID and Status columns).", self.path.display())) };
                self.file = Some(f);
            }
            Err(e) => {
                self.load_error = Some(e);
                self.file = None;
            }
        }
        self.refresh_quality(true);
        self.disk_changed = false;
        self.undo.clear();
        self.last_edit = None;
        self.sel = self.sel.min(self.filtered().len().saturating_sub(1));
    }

    pub fn is_dirty(&self) -> bool {
        self.file.as_ref().is_some_and(|f| f.dirty())
    }

    pub fn lanes(&self) -> Vec<String> {
        self.file.as_ref().map(|f| f.board.statuses(true)).unwrap_or_default()
    }

    /// The features that pass the filters, in the order of the current view.
    pub fn filtered(&self) -> Vec<Feature> {
        let Some(f) = &self.file else { return vec![] };
        let q = self.text.value.trim().to_lowercase();
        let mut v: Vec<Feature> = f
            .board
            .features()
            .into_iter()
            .filter(|x| self.status_filter.as_ref().is_none_or(|s| &x.status == s))
            .filter(|x| self.phase_filter.as_ref().is_none_or(|p| &x.phase == p))
            .filter(|x| self.area_filter.as_ref().is_none_or(|a| &x.area() == a))
            .filter(|x| !self.bugs_only || x.is_bug())
            .filter(|x| self.ext.agent_ok(&x.id))
            .filter(|x| q.is_empty() || x.id.to_lowercase().contains(&q) || x.title.to_lowercase().contains(&q) || x.notes.to_lowercase().contains(&q) || x.blockers().iter().any(|b| b.to_lowercase().contains(&q)))
            .collect();
        match self.mode {
            Mode::Table => {
                let (col, asc) = self.sort;
                let names: Vec<&str> = self.table_cols().iter().map(|c| c.0).collect();
                let name = names.get(col).copied().unwrap_or("");
                let now = iyakki_agent::clock::now_unix();
                let key = |x: &Feature| -> String {
                    match name {
                        "ID" => x.id.clone(),
                        "Feature" => x.title.to_lowercase(),
                        "Phase" => format!("{:>3}", x.phase.trim_start_matches('P')),
                        "Status" => x.status.clone(),
                        "Pri" => x.priority.clone(),
                        "Progress" => format!("{:03}", x.progress.unwrap_or(0)),
                        "Started" => format!("{:012}", fb::parse_stamp(&x.started).unwrap_or(0)),
                        "Finished" => format!("{:012}", fb::parse_stamp(&x.finished).unwrap_or(0)),
                        "Took" => format!("{:012}", x.duration(now).unwrap_or(0)),
                        "Sev" => format!("{}", match x.severity.as_str() { "high" => 3, "med" | "medium" => 2, "low" => 1, _ => 0 }),
                        "Tests" => {
                            let (n, rt, _) = self.row_quality(x);
                            format!("{n:05}{:05}", rt.passed)
                        }
                        "Cov" => format!("{:06}", self.row_quality(x).2.map_or(0, |(c, t)| c * 10_000 / t.max(1))),
                        "Depends" => self.depends_cell(x),
                        "Agent" => self.agent_cell(x),
                        _ => x.blockers().join(";"),
                    }
                };
                v.sort_by(|a, b| {
                    let o = if col == 2 { a.phase.trim_start_matches('P').parse::<u32>().unwrap_or(99).cmp(&b.phase.trim_start_matches('P').parse::<u32>().unwrap_or(99)) } else { key(a).cmp(&key(b)) };
                    if asc { o } else { o.reverse() }
                });
            }
            Mode::Phases => v.sort_by_key(|x| (x.phase.trim_start_matches('P').parse::<u32>().unwrap_or(99), x.id.clone())),
            Mode::Graph => v.sort_by(|a, b| a.id.cmp(&b.id)),
            Mode::Kanban => {
                // lane by lane, so the selection index walks the lanes in order
                let lanes = self.lanes();
                v.sort_by_key(|x| (lanes.iter().position(|l| *l == x.status).unwrap_or(99), x.id.clone()));
            }
        }
        v
    }

    pub fn selected(&self) -> Option<Feature> {
        self.filtered().get(self.sel).cloned()
    }

    pub fn select_id(&mut self, id: &str) {
        if let Some(i) = self.filtered().iter().position(|f| f.id == id) {
            self.sel = i;
        }
    }

    fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.msg = Some((m.into(), ok));
    }

    // ------------------------------------------------------------------------------------------------ editing

    /// Run an edit on the board: remembered for undo, marked dirty (written after a short idle time, or with Ctrl+S).
    pub fn edit<T>(&mut self, f: impl FnOnce(&mut Board) -> Result<T, String>) -> Result<T, String> {
        let Some(file) = self.file.as_mut() else { return Err("no features file loaded".into()) };
        let before = file.board.clone();
        match f(&mut file.board) {
            Ok(v) => {
                self.undo.push(before);
                if self.undo.len() > 50 {
                    self.undo.remove(0);
                }
                self.last_edit = Some(Instant::now());
                Ok(v)
            }
            Err(e) => {
                file.board = before;
                Err(e)
            }
        }
    }

    /// Change the status of a feature. Moving to partial or blocked without any comment on the row asks for one first
    /// (the repository's rule: those rows say why), so this returns `Ok(false)` after opening the comment dialog.
    pub fn set_status(&mut self, id: &str, status: &str) -> Result<bool, String> {
        let Some(cur) = self.file.as_ref().and_then(|f| f.board.get(id)) else { return Err(format!("no feature {id}")) };
        if cur.status == status {
            return Ok(true);
        }
        if matches!(status, "partial" | "blocked") && cur.notes.is_empty() && cur.blockers_cell.is_empty() {
            let f = Form::new(&format!("Why is {id} {status}?")).text("Comment", "", if status == "blocked" { "what blocks it (a comment is required)" } else { "what is left to do (a comment is required)" });
            self.modal = Some(Modal::Comment(id.to_string(), status.to_string(), f));
            return Ok(false);
        }
        let at = self.stamp_at();
        self.edit(|b| b.update(id, &Update { status: Some(status.into()), at, ..Default::default() }).map(|_| ()))?;
        if status == "done" {
            // a person's decision is as good as a check
            iyakki_agent::boardtrack::Tracker::new(&self.root, &self.path).mark_verified(&[id.to_string()]);
        }
        self.say(format!("{id} moved to {status}."), true);
        Ok(true)
    }

    /// The person's OK on a row in review: it is done, and counts as verified (a person's decision, recorded as such).
    pub fn mark_reviewed_done(&mut self, id: &str) -> BoardAct {
        if self.read_only {
            self.say("This workspace is open read-only, so nothing can be changed here.", false);
            return BoardAct::Used;
        }
        match self.set_status(id, "done") {
            Ok(true) => {
                iyakki_agent::boardtrack::Tracker::new(&self.root, &self.path).mark_verified(&[id.to_string()]);
                self.select_id(id);
            }
            Ok(false) => {}
            Err(e) => self.say(e, false),
        }
        BoardAct::Used
    }

    pub fn move_status(&mut self, id: &str, dir: i32) {
        let lanes = self.lanes();
        let Some(cur) = self.file.as_ref().and_then(|f| f.board.get(id)) else { return };
        let i = lanes.iter().position(|l| *l == cur.status).unwrap_or(0) as i32;
        let j = (i + dir).clamp(0, lanes.len() as i32 - 1) as usize;
        if j as i32 != i {
            if let Err(e) = self.set_status(id, &lanes[j]) {
                self.say(e, false);
            }
            self.select_id(id);
        }
    }

    fn cycle(&mut self, id: &str, what: &str) {
        let Some(cur) = self.file.as_ref().and_then(|f| f.board.get(id)) else { return };
        let r = match what {
            "phase" => {
                let i = fb::PHASES.iter().position(|p| *p == cur.phase).map_or(0, |i| i + 1) % fb::PHASES.len();
                self.edit(|b| b.update(id, &Update { phase: Some(fb::PHASES[i].into()), ..Default::default() }).map(|_| ()))
            }
            _ => {
                let i = fb::PRIORITIES.iter().position(|p| *p == cur.priority).map_or(0, |i| i + 1) % fb::PRIORITIES.len();
                self.edit(|b| b.update(id, &Update { priority: Some(fb::PRIORITIES[i].into()), ..Default::default() }).map(|_| ()))
            }
        };
        if let Err(e) = r {
            self.say(e, false);
        }
    }

    fn bump_progress(&mut self, id: &str, d: i32) {
        let Some(cur) = self.file.as_ref().and_then(|f| f.board.get(id)) else { return };
        let Some(p) = cur.progress else {
            return self.say("This table has no progress column.", false);
        };
        let n = (p as i32 + d).clamp(0, 100) as u32;
        let mut u = Update { progress: Some(n), ..Default::default() };
        // the file's rules tie status and progress together: 100% is done, 0% is planned
        if n == 100 && cur.status != "done" {
            u.status = Some("done".into());
        } else if n == 0 && cur.status == "partial" {
            u.status = Some("planned".into());
        } else if cur.status == "planned" && n > 0 {
            u.status = Some("partial".into());
            if cur.notes.is_empty() {
                let f = Form::new(&format!("Why is {id} partial?")).text("Comment", "", "what is left to do (a comment is required)");
                self.modal = Some(Modal::Comment(id.to_string(), "partial".into(), f));
                return;
            }
        }
        if let Err(e) = self.edit(|b| b.update(id, &u).map(|_| ())) {
            self.say(e, false);
        }
    }

    pub fn save(&mut self) -> BoardAct {
        let Some(f) = self.file.as_mut() else { return BoardAct::None };
        match f.save() {
            Ok(()) => {
                self.last_edit = None;
                self.disk_changed = false;
                self.say("Saved.", true);
                BoardAct::Saved(self.path.clone())
            }
            Err(e) => {
                self.say(format!("Could not save: {e}"), false);
                BoardAct::None
            }
        }
    }

    pub fn undo(&mut self) -> BoardAct {
        if let Some(prev) = self.undo.pop() {
            if let Some(f) = self.file.as_mut() {
                f.board = prev;
            }
            self.last_edit = Some(Instant::now());
            self.say("Undone.", true);
            return BoardAct::Used;
        }
        let Some(f) = self.file.as_mut() else { return BoardAct::None };
        match f.undo() {
            Ok(()) => {
                self.say("Went back to the version before the last save.", true);
                BoardAct::Saved(self.path.clone())
            }
            Err(e) => {
                self.say(e, false);
                BoardAct::None
            }
        }
    }

    /// Heartbeat: notice outside changes (once a second while drawn) and write after the idle time.
    pub fn tick(&mut self) -> BoardAct {
        let visible = self.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(600));
        if visible && self.last_poll.elapsed() >= self.poll_every {
            self.last_poll = Instant::now();
            self.refresh_quality(false);
            self.refresh_deps(false);
            let act = self.poll_disk();
            if act != BoardAct::None {
                return act;
            }
        }
        if self.is_dirty() && !self.disk_changed && self.modal.is_none() && self.last_edit.is_some_and(|t| t.elapsed() >= self.autosave_after) {
            return self.save();
        }
        BoardAct::None
    }

    /// Compare the file with what was loaded; reload when the board has no edits, otherwise raise the conflict bar.
    pub fn poll_disk(&mut self) -> BoardAct {
        let Some(f) = &self.file else {
            if self.path.exists() {
                self.reload();
            }
            return BoardAct::None;
        };
        match f.check_disk() {
            Disk::Same => {}
            Disk::Gone => {
                if !self.disk_changed {
                    self.disk_changed = true;
                    self.say("The features file was deleted or moved. Ctrl+S writes it again.", false);
                }
            }
            Disk::Changed => {
                if f.dirty() {
                    if !self.disk_changed {
                        self.disk_changed = true;
                        self.modal = Some(Modal::Conflict);
                    }
                } else {
                    let sel = self.selected().map(|x| x.id);
                    self.reload();
                    if let Some(id) = sel {
                        self.select_id(&id);
                    }
                    self.say("The file changed on disk: reloaded.", true);
                    return BoardAct::Used;
                }
            }
        }
        BoardAct::None
    }

    fn resolve_conflict(&mut self, how: char) {
        self.modal = None;
        match how {
            'r' => {
                self.reload();
                self.say("Took the file from disk; your unsaved edits were dropped.", true);
            }
            'm' => match self.file.as_mut().map(|f| f.merge_with_disk()) {
                Some(Ok(())) => {
                    self.disk_changed = false;
                    self.say("Merged: their changes plus your edits, row by row. Ctrl+S saves.", true);
                }
                Some(Err(e)) => self.say(format!("Could not merge: {e}"), false),
                None => {}
            },
            _ => {
                // keep mine: the next save overwrites the file (the other version stays in the .bak)
                self.disk_changed = false;
                if let Some(f) = self.file.as_mut() {
                    *f = {
                        let mut n = BoardFile::load(&self.path).unwrap_or_else(|_| BoardFile::load(&self.path).expect("file"));
                        n.board = f.board.clone();
                        n
                    };
                }
                self.say("Keeping your version; saving will replace the file (the old one goes to .bak).", true);
                self.last_edit = Some(Instant::now());
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ input

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    pub fn wants_text(&self) -> bool {
        self.text_focus
    }

    /// The keyboard went to another pane: the filter box gives up its caret.
    pub fn blur(&mut self) {
        self.text_focus = false;
    }

    fn submit_modal(&mut self) -> BoardAct {
        let Some(m) = self.modal.take() else { return BoardAct::None };
        match m {
            Modal::Add(f) => {
                let (prefix, title, phase, notes) = (f.value(0), f.value(1), f.value(2), f.value(3));
                let Some(bf) = self.file.as_ref() else { return BoardAct::None };
                let id = if prefix.to_uppercase().starts_with("F-") && prefix.matches('-').count() >= 2 { prefix.clone() } else { bf.board.next_id(&prefix.to_uppercase()) };
                let status = ["planned", "partial", "done", "blocked"][f.choice_idx(4).min(3)].to_string();
                if matches!(status.as_str(), "partial" | "blocked") && notes.trim().is_empty() {
                    let mut f = f;
                    f.error = Some(format!("A {status} feature needs a comment."));
                    self.modal = Some(Modal::Add(f));
                    return BoardAct::Used;
                }
                let nf = NewFeature { id: id.clone(), title, phase, status, priority: "P2".into(), notes };
                match self.edit(|b| b.add(&nf)) {
                    Ok(()) => {
                        self.select_id(&id);
                        self.say(format!("Added {id}."), true);
                    }
                    Err(e) => {
                        let mut f = f;
                        f.error = Some(e);
                        self.modal = Some(Modal::Add(f));
                    }
                }
            }
            Modal::Comment(id, status, f) => {
                let c = f.value(0);
                if c.trim().is_empty() {
                    let mut f = f;
                    f.error = Some("A comment is required.".into());
                    self.modal = Some(Modal::Comment(id, status, f));
                    return BoardAct::Used;
                }
                let u = Update { status: Some(status.clone()), comment: Some(c), at: self.stamp_at(), ..Default::default() };
                match self.edit(|b| b.update(&id, &u).map(|_| ())) {
                    Ok(()) => {
                        self.say(format!("{id} moved to {status}."), true);
                        self.select_id(&id);
                    }
                    Err(e) => self.say(e, false),
                }
            }
            Modal::EditComment(id, f) => {
                let u = Update { comment: Some(f.value(0)), ..Default::default() };
                if let Err(e) = self.edit(|b| b.update(&id, &u).map(|_| ())) {
                    self.say(e, false);
                }
            }
            Modal::EditTitle(id, f) => {
                let u = Update { title: Some(f.value(0)), ..Default::default() };
                if let Err(e) = self.edit(|b| b.update(&id, &u).map(|_| ())) {
                    self.say(e, false);
                }
            }
            Modal::Blocker(id, f) => {
                let t = f.value(0);
                if let Err(e) = self.edit(|b| b.add_blocker(&id, &t)) {
                    let mut f = f;
                    f.error = Some(e);
                    self.modal = Some(Modal::Blocker(id, f));
                } else {
                    self.say("Blocker added.", true);
                }
            }
            Modal::Delete(id) => match self.edit(|b| b.delete(&id)) {
                Ok(()) => {
                    self.drawer = false;
                    self.sel = self.sel.min(self.filtered().len().saturating_sub(1));
                    self.say(format!("Deleted {id}. Ctrl+Z brings it back."), true);
                }
                Err(e) => self.say(e, false),
            },
            Modal::Conflict => {}
        }
        BoardAct::Used
    }

    fn modal_event(&mut self, ev: &Event) -> BoardAct {
        if matches!(self.modal, Some(Modal::Conflict)) {
            match ev {
                Event::Key(Key::Char('r')) | Event::Sub(SUB, Key::Char('r')) => self.resolve_conflict('r'),
                Event::Key(Key::Char('m')) | Event::Sub(SUB, Key::Char('m')) => self.resolve_conflict('m'),
                Event::Key(Key::Char('k')) | Event::Sub(SUB, Key::Char('k')) | Event::Key(Key::Esc) => self.resolve_conflict('k'),
                _ => {}
            }
            return BoardAct::Used;
        }
        if let Some(Modal::Delete(id)) = &self.modal {
            let id = id.clone();
            match ev {
                Event::Key(Key::Char('y')) | Event::Sub(SUB, Key::Char('y')) => return self.submit_modal(),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) | Event::Sub(SUB, Key::Esc) => {
                    self.modal = None;
                    let _ = id;
                }
                _ => {}
            }
            return BoardAct::Used;
        }
        let ev2 = match ev {
            Event::Sub(SUB, k) => Event::Key(k.clone()),
            other => other.clone(),
        };
        let form = match self.modal.as_mut() {
            Some(Modal::Add(f)) | Some(Modal::Comment(_, _, f)) | Some(Modal::EditComment(_, f)) | Some(Modal::Blocker(_, f)) | Some(Modal::EditTitle(_, f)) => f,
            _ => return BoardAct::Used,
        };
        match form.handle_event(&ev2) {
            FormEvent::Cancel => self.modal = None,
            FormEvent::Submit => return self.submit_modal(),
            _ => {}
        }
        BoardAct::Used
    }

    pub fn handle(&mut self, ev: &Event) -> BoardAct {
        if self.modal.is_some() {
            return self.modal_event(ev);
        }
        if matches!(ev, Event::Key(Key::Char('C'))) && self.file.is_none() && !self.path.exists() {
            return self.create_board();
        }
        // a click anywhere else on the board takes the caret out of the filter box
        if matches!(ev, Event::Row(_) | Event::Tab(_) | Event::AreaAt(..)) {
            self.text_focus = false;
        }
        // the filter box takes typing
        if self.text_focus {
            match ev {
                Event::Key(Key::Esc) | Event::Key(Key::Enter) => {
                    self.text_focus = false;
                    return BoardAct::Used;
                }
                Event::Key(Key::Up) | Event::Key(Key::Down) => {}
                Event::Key(k) if !matches!(k, Key::Ctrl(_)) => {
                    self.text.handle(k);
                    self.sel = 0;
                    return BoardAct::Used;
                }
                Event::Paste(t) => {
                    self.text.paste(t);
                    return BoardAct::Used;
                }
                _ => {}
            }
        }
        match ev {
            Event::FocusAt(FOCUS_FILTER, x) => {
                self.text_focus = true;
                self.text.click(*x);
                return BoardAct::Used;
            }
            Event::Paste(t) if self.text_focus => {
                self.text.paste(t);
                return BoardAct::Used;
            }
            Event::Tab(t) if (TAB_BASE + deps::BASE + deps::AGENT_CHIP..TAB_BASE + deps::BASE + 6000).contains(t) => return self.deps_click(t - TAB_BASE - deps::BASE),
            Event::Tab(t) if (TAB_BASE + REVIEW_DONE..TAB_BASE + REVIEW_DONE + 10_000).contains(t) => {
                let i = t - TAB_BASE - REVIEW_DONE;
                if let Some((id, _)) = self.card_rects.get(i).cloned() {
                    return self.mark_reviewed_done(&id);
                }
                return BoardAct::Used;
            }
            Event::Tab(t) if (TAB_BASE + 400..TAB_BASE + 440).contains(t) => {
                // a bug's evidence: open the log (a folder opens its summary) or copy its path
                let n = t - TAB_BASE - 400;
                let (i, copy) = (n % 20, n >= 20);
                if let Some(f) = self.selected() {
                    if let Some(p) = f.logs.get(i) {
                        let rel = p.trim_end_matches('/').to_string();
                        let abs = self.root.join(&rel);
                        if copy {
                            crate::ui::clipboard::copy(&abs.display().to_string());
                        } else {
                            let file = if abs.is_dir() { format!("{rel}/summary.md") } else { rel };
                            return BoardAct::Open(file, 1);
                        }
                    }
                }
                return BoardAct::Used;
            }

            Event::Tab(t) if (TAB_BASE..TAB_BASE + 100).contains(t) => {
                return self.chip_click(t - TAB_BASE);
            }
            Event::Row(r) if (ROW_BASE..ROW_BASE + 100_000).contains(r) => {
                self.sel = r - ROW_BASE;
                if crate::ui::click_count() >= 2 {
                    self.drawer = true;
                }
                return BoardAct::Used;
            }
            Event::AreaAt(a, ..) if (CARD_BASE..CARD_BASE + 10_000).contains(a) => {
                let i = a - CARD_BASE;
                if let Some((id, _)) = self.card_rects.get(i).cloned() {
                    if !crate::ui::dragging() {
                        self.select_id(&id);
                        self.drag = Some(id);
                        if crate::ui::click_count() >= 2 {
                            self.drawer = true;
                            self.drag = None;
                        }
                    }
                }
                return BoardAct::Used;
            }
            Event::MouseUp(x, y) => {
                if let Some(id) = self.drag.take() {
                    if let Some((lane, _)) = self.lane_rects.iter().find(|(_, r)| r.contains(*x, *y)).cloned() {
                        let cur = self.file.as_ref().and_then(|f| f.board.get(&id)).map(|f| f.status);
                        if cur.as_deref() != Some(lane.as_str()) {
                            if let Err(e) = self.set_status(&id, &lane) {
                                self.say(e, false);
                            }
                            self.select_id(&id);
                        }
                    }
                    return BoardAct::Used;
                }
                return BoardAct::None;
            }
            Event::Scroll(d) => {
                match self.mode {
                    Mode::Graph => self.graph_scroll(*d),
                    Mode::Table | Mode::Phases => {
                        let n = self.filtered().len();
                        self.table_scroll = (self.table_scroll as i32 - d * 2).clamp(0, n.saturating_sub(self.visible_rows) as i32) as usize;
                    }
                    Mode::Kanban => {
                        // the lane under the pointer scrolls
                        let lane = self.lane_rects.iter().find(|(_, r)| r.contains(self.mouse_x(), self.mouse_y())).map(|x| x.0.clone());
                        if let Some(l) = lane {
                            let e = self.lane_scroll.entry(l).or_insert(0);
                            *e = (*e as i32 - d).max(0) as usize;
                        }
                    }
                }
                return BoardAct::Used;
            }
            _ => {}
        }
        let key = match ev {
            Event::Key(k) | Event::Sub(SUB, k) => k.clone(),
            _ => {
                if self.pane.handle(ev) {
                    return BoardAct::Used;
                }
                return BoardAct::None;
            }
        };
        self.key(&key)
    }

    fn mouse_x(&self) -> i32 {
        LAST_MOUSE.with(|m| m.get().0)
    }
    fn mouse_y(&self) -> i32 {
        LAST_MOUSE.with(|m| m.get().1)
    }

    fn chip_click(&mut self, n: usize) -> BoardAct {
        match n {
            0..=3 => {
                self.mode = Mode::ALL[n];
                self.sel = 0;
                self.table_scroll = 0;
            }
            10 => self.cycle_filter('s'),
            11 => self.cycle_filter('p'),
            12 => self.cycle_filter('a'),
            13 => {
                self.status_filter = None;
                self.phase_filter = None;
                self.area_filter = None;
                self.text.clear();
            }
            20..=29 => self.sort = (n - 20, if self.sort.0 == n - 20 { !self.sort.1 } else { true }),
            60..=61 => self.sort = (n - 50, if self.sort.0 == n - 50 { !self.sort.1 } else { true }),
            65 => {
                self.bugs_only = !self.bugs_only;
                self.sel = 0;
                self.table_scroll = 0;
            }
            62 => return BoardAct::RunTests,
            63 => return BoardAct::RunCoverage,
            64 => {
                let un = self.unlinked_tests();
                let list = un.iter().take(15).cloned().collect::<Vec<_>>().join("\n");
                return BoardAct::Ask(format!("These tests of the last run are not listed on any feature of the board. Link each to the feature it checks with board_link_tests (add a feature with board_add if none fits):\n{list}{}", if un.len() > 15 { format!("\n… and {} more", un.len() - 15) } else { String::new() }));
            }
            30 => return BoardAct::ToggleFull,
            // status chips in the summary
            40..=59 => {
                let lanes = self.lanes();
                if let Some(l) = lanes.get(n - 40) {
                    self.status_filter = if self.status_filter.as_ref() == Some(l) { None } else { Some(l.clone()) };
                }
            }
            // drawer: status chips
            60..=79 => {
                if let (Some(id), Some(l)) = (self.selected().map(|f| f.id), self.lanes().get(n - 60).cloned()) {
                    if let Err(e) = self.set_status(&id, &l) {
                        self.say(e, false);
                    }
                    self.select_id(&id);
                }
            }
            // drawer: tests
            100..=140 => {
                if let Some(f) = self.selected() {
                    if let Some(t) = f.tests.get(n - 100) {
                        return self.open_test(t);
                    }
                }
            }
            _ => {}
        }
        BoardAct::Used
    }

    fn cycle_filter(&mut self, which: char) {
        let Some(f) = &self.file else { return };
        let fs = f.board.features();
        let pick = |cur: &Option<String>, vals: Vec<String>| -> Option<String> {
            match cur {
                None => vals.first().cloned(),
                Some(c) => vals.iter().position(|v| v == c).and_then(|i| vals.get(i + 1)).cloned(),
            }
        };
        match which {
            's' => {
                let vals = f.board.statuses(true);
                self.status_filter = pick(&self.status_filter, vals);
            }
            'p' => {
                let mut vals: Vec<String> = fs.iter().map(|x| x.phase.clone()).filter(|p| !p.is_empty()).collect();
                vals.sort_by_key(|p| p.trim_start_matches('P').parse::<u32>().unwrap_or(99));
                vals.dedup();
                self.phase_filter = pick(&self.phase_filter, vals);
            }
            _ => {
                let vals = f.board.areas();
                self.area_filter = pick(&self.area_filter, vals);
            }
        }
        self.sel = 0;
    }

    pub fn open_test(&mut self, t: &str) -> BoardAct {
        let hits = fb::find_test(&self.root, t, 5);
        match hits.first() {
            Some((file, line)) => {
                let more = if hits.len() > 1 { format!(" (+{} more)", hits.len() - 1) } else { String::new() };
                self.say(format!("Opened {file}:{line}{more}"), true);
                BoardAct::Open(file.clone(), *line)
            }
            None => {
                self.say(format!("No test named {} found in this project.", t.rsplit("::").next().unwrap_or(t)), false);
                BoardAct::Used
            }
        }
    }

    /// Where the selection goes for an arrow key in the kanban: left and right change lane, up and down change card.
    fn kanban_step(&mut self, dx: i32, dy: i32) {
        let list = self.filtered();
        if list.is_empty() {
            return;
        }
        let lanes = self.lanes();
        let cur = list.get(self.sel).cloned().unwrap_or_else(|| list[0].clone());
        let li = lanes.iter().position(|l| *l == cur.status).unwrap_or(0) as i32;
        if dy != 0 {
            let in_lane: Vec<usize> = list.iter().enumerate().filter(|(_, f)| f.status == cur.status).map(|(i, _)| i).collect();
            let pos = in_lane.iter().position(|i| *i == self.sel).unwrap_or(0) as i32;
            let np = (pos + dy).clamp(0, in_lane.len() as i32 - 1) as usize;
            self.sel = in_lane[np];
        } else {
            let pos = list.iter().filter(|f| f.status == cur.status).position(|f| f.id == cur.id).unwrap_or(0);
            let mut l = li;
            loop {
                l += dx;
                if l < 0 || l >= lanes.len() as i32 {
                    break;
                }
                let in_lane: Vec<usize> = list.iter().enumerate().filter(|(_, f)| f.status == lanes[l as usize]).map(|(i, _)| i).collect();
                if !in_lane.is_empty() {
                    self.sel = in_lane[pos.min(in_lane.len() - 1)];
                    break;
                }
            }
        }
    }

    fn key(&mut self, k: &Key) -> BoardAct {
        if let Some(a) = self.deps_key(k) {
            return a;
        }
        let n = self.filtered().len();
        let sel = self.selected();
        let id = sel.as_ref().map(|f| f.id.clone());
        match k {
            Key::Ctrl('s') => return self.save(),
            Key::Ctrl('z') => return self.undo(),
            Key::Char('/') => self.text_focus = true,
            Key::Tab => {
                let i = Mode::ALL.iter().position(|m| *m == self.mode).unwrap_or(0);
                self.mode = Mode::ALL[(i + 1) % Mode::ALL.len()];
                self.sel = 0;
                self.table_scroll = 0;
            }
            Key::Char('v') => return BoardAct::ToggleFull,
            Key::Nav { nav: Nav::Left, ctrl: false, .. } | Key::Left if self.mode == Mode::Kanban => self.kanban_step(-1, 0),
            Key::Nav { nav: Nav::Right, ctrl: false, .. } | Key::Right if self.mode == Mode::Kanban => self.kanban_step(1, 0),
            Key::Up | Key::Nav { nav: Nav::Up, .. } => {
                if self.mode == Mode::Kanban { self.kanban_step(0, -1) } else { self.sel = self.sel.saturating_sub(1) }
            }
            Key::Down | Key::Nav { nav: Nav::Down, .. } => {
                if self.mode == Mode::Kanban { self.kanban_step(0, 1) } else { self.sel = (self.sel + 1).min(n.saturating_sub(1)) }
            }
            Key::PageUp => self.sel = self.sel.saturating_sub(self.visible_rows),
            Key::PageDown => self.sel = (self.sel + self.visible_rows).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::Enter => self.drawer = sel.is_some() && !self.drawer || (self.drawer && false),
            Key::Esc => {
                if self.drawer {
                    self.drawer = false;
                } else if self.status_filter.is_some() || self.phase_filter.is_some() || self.area_filter.is_some() || !self.text.value.is_empty() {
                    self.status_filter = None;
                    self.phase_filter = None;
                    self.area_filter = None;
                    self.text.clear();
                } else if self.full {
                    return BoardAct::Close;
                } else {
                    return BoardAct::None;
                }
            }
            Key::Char(']') => {
                if let Some(id) = id {
                    self.move_status(&id, 1)
                }
            }
            Key::Char('[') => {
                if let Some(id) = id {
                    self.move_status(&id, -1)
                }
            }
            Key::Char('+') | Key::Char('=') => {
                if let Some(id) = id {
                    self.bump_progress(&id, 10)
                }
            }
            Key::Char('-') => {
                if let Some(id) = id {
                    self.bump_progress(&id, -10)
                }
            }
            Key::Char('p') => {
                if let Some(id) = id {
                    self.cycle(&id, "phase")
                }
            }
            Key::Char('P') => {
                if let Some(id) = id {
                    self.cycle(&id, "priority")
                }
            }
            Key::Char('e') | Key::Char('c') => {
                if let Some(f) = sel {
                    let form = Form::new(&format!("Comment on {}", f.id)).text("Comment (the notes cell)", &f.notes, "what to know about this feature");
                    self.modal = Some(Modal::EditComment(f.id, form));
                }
            }
            Key::Char('T') => {
                if let Some(f) = sel {
                    let form = Form::new(&format!("Title of {}", f.id)).text("Feature", &f.title, "");
                    self.modal = Some(Modal::EditTitle(f.id, form));
                }
            }
            Key::Char('b') => {
                if let Some(f) = sel {
                    let form = Form::new(&format!("Add a blocker to {}", f.id)).text("Blocked by", "", "what has to happen first");
                    self.modal = Some(Modal::Blocker(f.id, form));
                }
            }
            Key::Char('B') => {
                if let Some(id) = id {
                    match self.edit(|b| b.clear_blockers(&id)) {
                        Ok(()) => self.say("Blockers cleared.", true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            Key::Char('a') => {
                let areas = self.file.as_ref().map(|f| f.board.areas()).unwrap_or_default();
                let start = sel.as_ref().map(|f| f.area()).or_else(|| areas.first().cloned()).unwrap_or_else(|| "NEW".into());
                let suggested = self.file.as_ref().map(|f| f.board.next_id(&start)).unwrap_or_default();
                let phase = sel.as_ref().map(|f| f.phase.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| "P5".into());
                let form = Form::new("Add a feature")
                    .text("Area prefix or full id", &start, &format!("an area such as {start} (next free id: {suggested}) or a full id"))
                    .text("Feature", "", "what it does, in one sentence")
                    .text("Phase", &phase, "P0 … P10")
                    .text("Comment", "", "needed for partial and blocked")
                    .choice("Status", &["planned", "partial", "done", "blocked"], 0);
                self.modal = Some(Modal::Add(form));
            }
            Key::Delete | Key::Char('x') => {
                if let Some(id) = id {
                    self.modal = Some(Modal::Delete(id));
                }
            }
            Key::Char('s') => self.cycle_filter('s'),
            Key::Char('f') => self.cycle_filter('a'),
            Key::Char('g') => self.cycle_filter('p'),
            Key::Char('S') if self.mode == Mode::Table => self.sort = ((self.sort.0 + 1) % self.table_cols().len(), true),
            Key::Char('r') => {
                self.reload();
                self.say("Reloaded from disk.", true);
            }
            Key::Char('i') | Key::Char('?') => {
                if let Some(f) = sel {
                    return BoardAct::Ask(ask_text(&f));
                }
            }
            Key::Char('o') => {
                if let Some(f) = sel {
                    if let Some(t) = f.tests.first() {
                        return self.open_test(t);
                    }
                    self.say("This feature lists no tests.", false);
                }
            }
            _ => return BoardAct::None,
        }
        self.sel = self.sel.min(self.filtered().len().saturating_sub(1));
        BoardAct::Used
    }

    // ------------------------------------------------------------------------------------------------ drawing

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], tick: u64) {
        self.last_draw = Some(Instant::now());
        LAST_MOUSE.with(|m| m.set(g.mouse));
        // the filter box keeps its caret only while the board owns the keyboard, and the caret blinks
        if !focused {
            self.text_focus = false;
        }
        self.caret_on = super::blink(tick);
        let pad = 10.0 * s;
        let mut y = r.y + 4.0 * s;
        let inner = R::new(r.x + pad, r.y, r.w - 2.0 * pad, r.h);
        if self.file.is_none() || self.load_error.is_some() {
            let msg = self.load_error.clone().unwrap_or_else(|| "No features file.".into());
            let missing = self.file.is_none() && !self.path.exists();
            let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "FEATURES.md".into());
            let hint = if missing { format!("{name} is not in this workspace yet. Create it to track the work here; the assistant keeps it current as it builds.") } else { format!("{msg} Fix the table in {name} (| ID | Feature | Status | Tests | Started | Finished | Notes |) and it appears here.") };
            fx::empty_state(g, r, s, crate::gfx::Icon::Runs, "No feature board yet", &hint, accent);
            if missing {
                let size = 14.0 * s;
                let label = "Create board";
                let w = g.text_width(size, label, crate::gfx::Face::Bold) + size * 1.7;
                let (bx, by) = (r.x + (r.w - w) / 2.0, r.y + (r.h / 2.0 - 40.0 * s).max(40.0 * s) + 140.0 * s);
                if self.read_only {
                    g.button_off(bx, by, size, label);
                    fx::status_line(g, (r.x + r.w / 2.0 - 150.0 * s).max(r.x), by + 40.0 * s, 300.0 * s, s, "Read-only: another window has this workspace.", false);
                } else {
                    g.button(bx, by, size, label, accent, Event::Key(Key::Char('C')));
                }
            }
            return;
        }
        self.ext.tick = tick;
        self.refresh_deps(false);
        y += self.draw_summary(g, R::new(inner.x, y, inner.w, 0.0), s, accent);
        y += self.draw_quality(g, R::new(inner.x, y, inner.w, 0.0), s);
        y += self.draw_filters(g, R::new(inner.x, y, inner.w, 0.0), s, accent);
        y += self.draw_agents_strip(g, R::new(inner.x, y, inner.w, 0.0), s, accent);
        // the conflict bar
        if self.disk_changed && !matches!(self.modal, Some(Modal::Conflict)) {
            let bar = R::new(inner.x, y, inner.w, 26.0 * s);
            g.rrect(bar.x, bar.y, bar.w, bar.h, 8.0 * s, rgb(52, 36, 18));
            g.rrect_outline(bar.x, bar.y, bar.w, bar.h, 8.0 * s, 1.0, AMBER);
            g.text(bar.x + 10.0 * s, bar.y + 6.0 * s, 12.0 * s, AMBER, "The file changed on disk and you have edits.", Face::Bold);
            y += 30.0 * s;
        }
        let status_h = 24.0 * s;
        let body = R::new(inner.x, y, inner.w, (r.y + r.h - y - status_h).max(40.0 * s));
        let panel_narrow = body.w < 700.0 * s;
        let (main, drawer, panel) = if self.ext.drill.is_some() && self.ext.drilled().is_some() {
            // an agent's panel takes the place of the details: beside the board when there is room, else over it
            if panel_narrow {
                (R::new(body.x, body.y, 0.0, body.h), None, Some(body))
            } else {
                let pw = (body.w * 0.46).clamp(320.0 * s, 560.0 * s).min(body.w - 200.0 * s);
                (R::new(body.x, body.y, body.w - pw - 8.0 * s, body.h), None, Some(R::new(body.x + body.w - pw, body.y, pw, body.h)))
            }
        } else if self.drawer && self.selected().is_some() {
            let dw = (body.w * 0.40).clamp(280.0 * s, 460.0 * s).min(body.w - 160.0 * s).max(200.0 * s);
            (R::new(body.x, body.y, body.w - dw - 8.0 * s, body.h), Some(R::new(body.x + body.w - dw, body.y, dw, body.h)), None)
        } else {
            (body, None, None)
        };
        if main.w < 1.0 {
            // the narrow panel covers the board: nothing of it is drawn (and none of its controls exist)
            self.lane_rects.clear();
            self.card_rects.clear();
        } else {
            match self.mode {
                Mode::Kanban => self.draw_kanban(g, main, s, focused, accent),
                Mode::Table => self.draw_table(g, main, s, focused, accent),
                Mode::Phases => self.draw_phases(g, main, s, focused, accent),
                Mode::Graph => self.draw_graph(g, main, s, focused, accent),
            }
        }
        if let Some(p) = panel {
            self.draw_agent_panel(g, p, s, accent, panel_narrow);
        }
        if let Some(d) = drawer {
            self.draw_drawer(g, d, s, accent);
        }
        // status line
        let sy = r.y + r.h - status_h + 2.0 * s;
        let dirty = self.is_dirty();
        let (txt, ok) = match &self.msg {
            Some((m, ok)) => (m.clone(), *ok),
            None => ("Enter details · [ ] move · e comment · b blocker · a add · x delete · Tab view · / filter · Ctrl+Z undo".to_string(), true),
        };
        let col = if self.msg.is_none() { DIM } else if ok { OK } else { ERR };
        let t = g.fit(12.0 * s, &txt, Face::Sans, inner.w - 120.0 * s);
        g.text(inner.x, sy + 3.0 * s, 12.0 * s, col, &t, Face::Sans);
        if dirty {
            let label = "● unsaved (Ctrl+S)";
            let w = g.text_width(12.0 * s, label, Face::Bold);
            g.text(inner.x + inner.w - w, sy + 3.0 * s, 12.0 * s, AMBER, label, Face::Bold);
        }
        // a card being dragged follows the pointer
        if let Some(id) = &self.drag {
            if crate::ui::dragging() {
                let (mx, my) = g.mouse;
                let gr = R::new(mx as f32 - 70.0 * s, my as f32 - 14.0 * s, 150.0 * s, 28.0 * s);
                g.rrect(gr.x, gr.y, gr.w, gr.h, 8.0 * s, with_alpha(accent, 190));
                g.text(gr.x + 10.0 * s, gr.y + 6.0 * s, 13.0 * s, rgb(8, 11, 26), id, Face::Bold);
            }
        }
        let _ = tick;
        self.draw_modal(g, r, s, accent);
    }

    fn draw_summary(&mut self, g: &mut Gfx, r: R, s: f32, _accent: [u8; 4]) -> f32 {
        let Some(f) = &self.file else { return 0.0 };
        let counts = f.board.count_by_status();
        let lanes = f.board.statuses(true);
        let total = f.board.len();
        let pct = f.board.percent_done();
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        g.text(r.x, r.y + 4.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
        let tw = g.text_width(15.0 * s, &name, Face::Bold);
        // percent done ring
        let rx = r.x + tw + 34.0 * s;
        g.ring(rx, r.y + 14.0 * s, 11.0 * s, 3.0 * s, rgb(30, 38, 66));
        fx::ring_gauge(g, rx, r.y + 14.0 * s, 11.0 * s, 3.0 * s, pct as f32 / 100.0, OK);
        g.text(rx + 18.0 * s, r.y + 6.0 * s, 13.0 * s, OK, &format!("{pct}% done"), Face::Bold);
        let mut x = rx + 18.0 * s + g.text_width(13.0 * s, &format!("{pct}% done"), Face::Bold) + 16.0 * s;
        let working = self.ext.agents_working();
        if working > 0 && x + 140.0 * s < r.x + r.w - 150.0 * s {
            x += fx::chip(g, x, r.y + 3.0 * s, s * 0.85, &format!("{working} agent{} working", if working == 1 { "" } else { "s" }), CYAN, true) + 8.0 * s;
        }
        for (i, l) in lanes.iter().enumerate() {
            let c = counts.get(l).copied().unwrap_or(0);
            if c == 0 && !fb::STATUSES.contains(&l.as_str()) {
                continue;
            }
            let label = format!("{l} {c}");
            let w = g.text_width(11.5 * s, &label, Face::Bold) + 20.0 * s;
            if x + w > r.x + r.w - 150.0 * s {
                break;
            }
            let on = self.status_filter.as_ref() == Some(l);
            let b = R::new(x, r.y + 2.0 * s, w, (25.0 * s).max(20.0));
            let col = status_color(l);
            // flat: a coloured dot and the count; a soft background only for the lane filtered on or under the pointer
            let hot = g.hovered(b);
            if on || hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if on { fx::over(fx::WELL, with_alpha(col, 70)) } else { rgb(34, 44, 78) });
            }
            g.circle(b.x + 7.0 * s, b.y + b.h / 2.0, 2.5 * s, col);
            g.text(b.x + 13.0 * s, b.y + (b.h - g.line_height(11.5 * s, Face::Bold)) / 2.0, 11.5 * s, if on { TEXT } else { fx::readable(col, fx::PANEL, 4.5) }, &label, Face::Bold);
            g.hit(b, Event::Tab(TAB_BASE + 40 + i), &format!("status {l}"));
            x += w + 5.0 * s;
        }
        // the file's own totals line at the right
        let totals = f.board.totals_in_file().unwrap_or_else(|| f.board.totals_text());
        let room = r.x + r.w - x - 170.0 * s;
        let t = g.fit(11.0 * s, &totals, Face::Mono, room.max(0.0));
        if room > 140.0 * s && !t.is_empty() {
            g.text(x + 4.0 * s, r.y + 6.0 * s, 11.0 * s, DIM, &t, Face::Mono);
        }
        let _ = total;
        let label = if self.full { "Exit full" } else { "Full screen" };
        let w = g.text_width(super::ide_chrome::cur().font_small * s + 1.0 * s, label, Face::Bold) + 16.0 * s;
        let bounds = R::new(r.x - 200.0 * s, r.y, r.w + 200.0 * s, 400.0 * s);
        super::ide_chrome::text_btn(g, r.x + r.w - w, r.y + 1.0 * s, s, label, CYAN, true, Event::Tab(TAB_BASE + 30), label, Some("Show the board over the whole IDE (v)"), bounds);
        28.0 * s
    }

    /// The line under the summary: overall test results, coverage, tests not linked to a feature, and the buttons that run them.
    fn draw_quality(&mut self, g: &mut Gfx, r: R, s: f32) -> f32 {
        let open_bugs = self.file.as_ref().map_or(0, |f| f.board.features().iter().filter(|x| x.is_bug() && x.status != "done").count());
        let any_bugs = self.file.as_ref().is_some_and(|f| f.board.features().iter().any(|x| x.is_bug()));
        if !self.tracks_tests() && self.coverage.is_none() && !any_bugs {
            return 0.0;
        }
        let size = 11.5 * s;
        let mut x = r.x;
        let y = r.y + 2.0 * s;
        let now = iyakki_agent::clock::now_unix();
        let put = |g: &mut Gfx, x: &mut f32, text: &str, color: [u8; 4]| {
            g.text(*x, y + 3.0 * s, size, color, text, Face::Bold);
            *x += g.text_width(size, text, Face::Bold) + 14.0 * s;
        };
        if any_bugs {
            // the Bugs chip: open bugs, and a click shows only bugs
            let label = format!("🐞 Bugs {open_bugs}");
            let w = g.text_width(size, &label, Face::Bold) + 14.0 * s;
            let b = R::new(x, y, w, 20.0 * s);
            if self.bugs_only || g.hovered(b) {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if self.bugs_only { fx::over(fx::WELL, with_alpha(ERR, 70)) } else { rgb(34, 44, 78) });
            }
            g.text(x + 7.0 * s, y + 3.0 * s, size, if open_bugs > 0 { ERR } else { MUTED }, &label, Face::Bold);
            g.hit(b, Event::Tab(TAB_BASE + 65), "bugs");
            x += w + 8.0 * s;
        }
        match &self.results {
            Some(res) => {
                let total = res.tests.len();
                let pass = res.tests.values().filter(|o| **o == iyakki_agent::testresults::Outcome::Pass).count();
                let fail = res.tests.values().filter(|o| **o == iyakki_agent::testresults::Outcome::Fail).count();
                let color = if fail > 0 { ERR } else { OK };
                put(g, &mut x, &format!("tests {pass}/{total} pass{} · {}", if fail > 0 { format!(" · {fail} fail") } else { String::new() }, iyakki_agent::testresults::ago(now, res.ran_at)), color);
            }
            None => put(g, &mut x, "tests not run yet", MUTED),
        }
        match &self.coverage {
            Some(c) => put(g, &mut x, &format!("coverage {}% · {}", c.overall().map_or("-".to_string(), |p| format!("{p:.0}")), iyakki_agent::testresults::ago(now, c.ran_at)), if c.overall().is_some_and(|p| p >= 70.0) { OK } else { AMBER }),
            None => put(g, &mut x, "coverage not measured", MUTED),
        }
        let un = self.unlinked_tests().len();
        if un > 0 {
            put(g, &mut x, &format!("{un} tests not linked to a feature"), AMBER);
        }
        if let Some(b) = &self.busy {
            put(g, &mut x, &format!("⟳ {b}"), CYAN);
        } else if let Some(n) = &self.cov_note {
            let room = (r.x + r.w - x - 250.0 * s).max(0.0);
            let t = g.fit(size, n, Face::Sans, room);
            if !t.is_empty() {
                g.text(x, y + 3.0 * s, size, AMBER, &t, Face::Sans);
            }
        }
        // buttons at the right
        let bounds = R::new(r.x - 200.0 * s, r.y, r.w + 200.0 * s, 300.0 * s);
        let mut bx = r.x + r.w;
        let small = super::ide_chrome::cur().font_small * s + 1.0 * s;
        let mut btn = |g: &mut Gfx, label: &str, tab: usize, tip: &str, on: bool| {
            let w = g.text_width(small, label, Face::Bold) + 16.0 * s;
            bx -= w + 4.0 * s;
            super::ide_chrome::text_btn(g, bx, y, s, label, CYAN, on, Event::Tab(TAB_BASE + tab), label, Some(tip), bounds);
        };
        if un > 0 {
            btn(g, "Link tests", 64, "Ask the assistant to link the unlinked tests to features (board_link_tests)", !self.read_only);
        }
        btn(g, "Coverage", 63, "Measure line coverage with the project's coverage command (or a detected tool; nothing is run that is not installed)", !self.read_only && self.busy.is_none());
        btn(g, "Run tests", 62, "Run the project's test command and show each feature's results (F5)", !self.read_only && self.busy.is_none());
        24.0 * s
    }

    fn draw_filters(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) -> f32 {
        let mut x = r.x;
        let y = r.y + 2.0 * s;
        // view toggle: flat text tabs with an underline
        for (i, m) in Mode::ALL.iter().enumerate() {
            x += super::ide_chrome::flat_tab(g, x, y - 2.0 * s, s, m.label(), *m == self.mode, accent, Event::Tab(TAB_BASE + i), m.label());
        }
        x += 8.0 * s;
        // the filters cycle through the values found: plain text, lit when one is set
        for (n, label, val) in [(10usize, "status", self.status_filter.clone()), (11, "phase", self.phase_filter.clone()), (12, "area", self.area_filter.clone())] {
            let txt = match &val {
                Some(v) => format!("{label}: {v}"),
                None => format!("{label}: all"),
            };
            let w = g.text_width(11.5 * s, &txt, Face::Bold) + 16.0 * s;
            if x + w > r.x + r.w - 150.0 * s {
                break;
            }
            let b = R::new(x, y, w, (25.0 * s).max(20.0));
            let hot = g.hovered(b);
            let on = val.is_some();
            if on || hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if on { fx::over(fx::WELL, with_alpha(MAGENTA, 60)) } else { rgb(34, 44, 78) });
            }
            g.text(b.x + 8.0 * s, b.y + (b.h - g.line_height(11.5 * s, Face::Bold)) / 2.0, 11.5 * s, if on { TEXT } else { fx::readable(MUTED, fx::PANEL, 4.5) }, &txt, Face::Bold);
            g.hit(b, Event::Tab(TAB_BASE + n), &txt);
            x += w + 2.0 * s;
        }
        let fw = (r.x + r.w - x).clamp(0.0, 230.0 * s);
        if fw > 90.0 * s {
            let fr = R::new(r.x + r.w - fw, y, fw, 24.0 * s);
            // the box registers its own click region
            self.text.render(g, fr, self.text_focus, s * 0.8, self.caret_on, Face::Sans, FOCUS_FILTER);
        }
        28.0 * s
    }

    #[allow(clippy::too_many_arguments)]
    fn feature_card(&mut self, g: &mut Gfx, f: &Feature, r: R, s: f32, selected: bool, focused: bool, accent: [u8; 4], idx: usize) {
        let col = if f.is_bug() && f.status != "done" { ERR } else { status_color(&f.status) };
        let hot = g.hovered(r);
        g.rrect(r.x, r.y + 2.0 * s, r.w, r.h, 10.0 * s, [0, 0, 0, 60]);
        g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, if selected { fx::over(rgb(24, 32, 60), with_alpha(accent, if focused { 46 } else { 22 })) } else if hot { rgb(26, 34, 62) } else { rgb(18, 24, 46) });
        g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, if selected { 1.6 * s } else { 1.0 }, if selected { with_alpha(accent, if focused { 255 } else { 150 }) } else { with_alpha(col, 70) });
        g.rrect(r.x + 1.0 * s, r.y + 10.0 * s, 3.0 * s, r.h - 20.0 * s, 1.5 * s, col);
        let x = r.x + 12.0 * s;
        let w = r.w - 22.0 * s;
        let mut y = r.y + 7.0 * s;
        g.text(x, y, 12.0 * s, if f.is_bug() { ERR } else { accent }, &f.id, Face::Bold);
        let mut cx = x + g.text_width(12.0 * s, &f.id, Face::Bold) + 8.0 * s;
        let sev = if f.is_bug() && !f.severity.is_empty() { f.severity.to_uppercase() } else { String::new() };
        for (txt, c) in [(if f.is_bug() { "🐞 BUG".to_string() } else { String::new() }, ERR), (sev, if f.severity == "high" { ERR } else { AMBER }), (f.phase.clone(), CYAN), (f.priority.clone(), if f.priority == "P0" { ERR } else { MUTED })] {
            if !txt.is_empty() && cx + 30.0 * s < x + w {
                cx += fx::chip(g, cx, y - 2.0 * s, s * 0.85, &txt, c, false) + 4.0 * s;
            }
        }
        y += 20.0 * s;
        let title = f.short_title();
        let lines = g.wrap(13.0 * s, &title, Face::Sans, w);
        for l in lines.iter().take(2) {
            g.text(x, y, 13.0 * s, TEXT, l, Face::Sans);
            y += 17.0 * s;
        }
        if let Some(p) = f.progress {
            let bar = R::new(x, y + 3.0 * s, w - 38.0 * s, 6.0 * s);
            fx::progress_bar(g, bar, p as f32 / 100.0, col, s);
            g.text(x + w - 34.0 * s, y - 1.0 * s, 11.0 * s, MUTED, &format!("{p}%"), Face::Bold);
            y += 14.0 * s;
        }
        let bl = f.blockers();
        if !bl.is_empty() {
            let t = g.fit(11.0 * s, &format!("⚠ {}", bl.join("; ")), Face::Sans, w);
            g.text(x, y, 11.0 * s, ERR, &t, Face::Sans);
            y += 16.0 * s;
        } else if !f.notes.is_empty() {
            let t = g.fit(11.0 * s, &f.notes, Face::Sans, w);
            g.text(x, y, 11.0 * s, fx::readable(MUTED, rgb(18, 24, 46), 4.5), &t, Face::Sans);
            y += 16.0 * s;
        }
        if let Some(line) = time_line(f, iyakki_agent::clock::now_unix(), &iyakki_agent::clock::Zone::system()) {
            let t = g.fit(11.0 * s, &line, Face::Sans, w);
            g.text(x, y, 11.0 * s, if f.status == "done" { OK } else { fx::readable(CYAN, rgb(18, 24, 46), 4.5) }, &t, Face::Sans);
            y += 15.0 * s;
        }
        if let Some((line, color)) = self.quality_line(f) {
            let t = g.fit(11.0 * s, &line, Face::Sans, w);
            g.text(x, y, 11.0 * s, color, &t, Face::Sans);
            y += 15.0 * s;
        }
        // what the row waits for and who works on it
        y += self.draw_card_deps(g, f, x, y, w, s, rgb(18, 24, 46));
        // work the assistant claimed that nothing could check: one click from the person marks it done
        if f.status == "review" && !self.read_only {
            let size = 11.0 * s;
            let bw = g.text_width(size, "Mark done", Face::Bold) + size * 1.7;
            g.button(x + w - bw, y, size, "Mark done", OK, Event::Tab(TAB_BASE + REVIEW_DONE + idx));
        }
    }

    fn draw_kanban(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        let lanes = self.lanes();
        let list = self.filtered();
        let sel_id = list.get(self.sel).map(|f| f.id.clone());
        self.lane_rects.clear();
        self.card_rects.clear();
        let gap = 8.0 * s;
        let min_w = 186.0 * s;
        let n = lanes.len().max(1);
        // as many lanes as fit side by side at a readable width; the others are a step away (Left/Right, or a click)
        let cols = (((r.w + gap) / (min_w + gap)).floor() as usize).clamp(1, n);
        let lane_w = (r.w - gap * (cols as f32 - 1.0)) / cols as f32;
        if let Some(i) = sel_id.as_ref().and_then(|id| list.iter().find(|f| &f.id == id)).and_then(|f| lanes.iter().position(|l| *l == f.status)) {
            if i < self.first_lane {
                self.first_lane = i;
            } else if i >= self.first_lane + cols {
                self.first_lane = i + 1 - cols;
            }
        }
        self.first_lane = self.first_lane.min(n - cols);
        let first_lane = self.first_lane;
        if first_lane > 0 {
            g.text(r.x - 8.0 * s, r.y + r.h / 2.0, 16.0 * s, with_alpha(accent, 200), "‹", Face::Bold);
        }
        if first_lane + cols < n {
            g.text(r.x + r.w - 5.0 * s, r.y + r.h / 2.0, 16.0 * s, with_alpha(accent, 200), "›", Face::Bold);
        }
        for (li, lane) in lanes.iter().enumerate().skip(first_lane).take(cols) {
            let x = r.x + (li - first_lane) as f32 * (lane_w + gap);
            let col = status_color(lane);
            let lr = R::new(x, r.y, lane_w, r.h);
            let cards: Vec<&Feature> = list.iter().filter(|f| &f.status == lane).collect();
            self.lane_rects.push((lane.clone(), lr));
            g.rrect(lr.x, lr.y, lr.w, lr.h, 12.0 * s, rgb(10, 14, 30));
            g.rrect_outline(lr.x, lr.y, lr.w, lr.h, 12.0 * s, 1.0, with_alpha(col, if self.drag.is_some() && lr.contains(g.mouse.0, g.mouse.1) { 255 } else { 70 }));
            g.rect(lr.x + 12.0 * s, lr.y + 30.0 * s, lr.w - 24.0 * s, 2.0 * s, with_alpha(col, 160));
            g.text(lr.x + 12.0 * s, lr.y + 8.0 * s, 13.0 * s, fx::readable(col, rgb(10, 14, 30), 4.5), &lane.to_uppercase(), Face::Bold);
            let cs = format!("{}", cards.len());
            let cw = g.text_width(12.0 * s, &cs, Face::Bold);
            g.text(lr.x + lr.w - 12.0 * s - cw, lr.y + 9.0 * s, 12.0 * s, MUTED, &cs, Face::Bold);
            // cards, scrolled by whole cards
            let area = R::new(lr.x + 6.0 * s, lr.y + 38.0 * s, lr.w - 12.0 * s, lr.h - 44.0 * s);
            let first = *self.lane_scroll.get(lane).unwrap_or(&0);
            let sel_pos = cards.iter().position(|f| Some(&f.id) == sel_id.as_ref());
            let first = first.min(cards.len().saturating_sub(1));
            let mut first = first;
            if let Some(p) = sel_pos {
                if p < first {
                    first = p;
                }
            }
            let mut y = area.y;
            let mut shown = 0;
            let mut last_shown = first;
            for (ci, f) in cards.iter().enumerate().skip(first) {
                let h = self.card_height_px(g, f, area.w, s);
                if y + h > area.y + area.h + 0.5 {
                    if sel_pos == Some(ci) && shown > 0 {
                        // the selection is below the fold: scroll down by one card and draw again next frame
                        first += 1;
                    }
                    break;
                }
                let cr = R::new(area.x, y, area.w, h);
                let idx = self.card_rects.len();
                self.card_rects.push((f.id.clone(), cr));
                let selected = Some(&f.id) == sel_id.as_ref();
                // the card's own click area first, so a button drawn on the card (Mark done) wins over it
                g.hit(cr, Event::Area(CARD_BASE + idx), &f.id);
                self.feature_card(g, f, cr, s, selected, focused, accent, idx);
                y += h + 6.0 * s;
                shown += 1;
                last_shown = ci;
            }
            if let Some(p) = sel_pos {
                if p > last_shown && shown > 0 {
                    first = (first + 1).min(cards.len().saturating_sub(1));
                }
            }
            self.lane_scroll.insert(lane.clone(), first);
            if first > 0 {
                g.text(lr.x + lr.w / 2.0 - 6.0 * s, lr.y + 33.0 * s, 10.0 * s, DIM, "▲", Face::Bold);
            }
            if first + shown < cards.len() {
                g.text(lr.x + lr.w / 2.0 - 6.0 * s, lr.y + lr.h - 14.0 * s, 10.0 * s, DIM, &format!("▼ {}", cards.len() - first - shown), Face::Bold);
            }
            if cards.is_empty() {
                g.text(lr.x + 14.0 * s, lr.y + 48.0 * s, 12.0 * s, DIM, "nothing here", Face::Sans);
            }
        }
    }

    fn card_height_px(&self, g: &mut Gfx, f: &Feature, w: f32, s: f32) -> f32 {
        let lines = g.wrap(13.0 * s, &f.short_title(), Face::Sans, w - 22.0 * s).len().clamp(1, 2) as f32;
        let mut h = 7.0 * s + 20.0 * s + lines * 17.0 * s + 8.0 * s;
        if f.progress.is_some() {
            h += 14.0 * s;
        }
        if !f.blockers().is_empty() || !f.notes.is_empty() {
            h += 16.0 * s;
        }
        if fb::parse_stamp(&f.started).is_some() {
            h += 15.0 * s;
        }
        if self.quality_line(f).is_some() {
            h += 15.0 * s;
        }
        if f.status == "review" {
            h += 26.0 * s;
        }
        h + self.deps_card_extra(f, s)
    }

    /// The time to write into Started / Finished on a status change: only for files that already have those columns
    /// (a board without them is not changed by a drag).
    fn stamp_at(&self) -> Option<i64> {
        self.file.as_ref().and_then(|f| f.board.cols().map(|c| c.started.is_some() || c.finished.is_some())).unwrap_or(false).then(iyakki_agent::clock::now_unix)
    }

    /// The table's columns; Started, Finished and Took appear when the file has the time columns.
    fn table_cols(&self) -> Vec<(&'static str, f32)> {
        let times = self.file.as_ref().and_then(|f| f.board.cols().map(|c| c.started.is_some() || c.finished.is_some())).unwrap_or(false);
        let mut v = vec![("ID", 0.09), ("Feature", 0.22), ("Phase", 0.05), ("Status", 0.08), ("Pri", 0.04), ("Progress", 0.07)];
        if times {
            v.extend([("Started", 0.09), ("Finished", 0.09), ("Took", 0.06)]);
        }
        let any_files = self.coverage.is_some() && self.file.as_ref().is_some_and(|f| f.board.features().iter().any(|x| !x.files.is_empty()));
        if self.file.as_ref().and_then(|f| f.board.cols().map(|c| c.severity.is_some())).unwrap_or(false) {
            v.push(("Sev", 0.05));
        }
        if self.tracks_tests() {
            v.push(("Tests", 0.13));
        }
        if any_files {
            v.push(("Cov", 0.05));
        }
        if self.show_depends_column() {
            v.push(("Depends", 0.09));
        }
        if self.show_agent_column() {
            v.push(("Agent", 0.10));
        }
        let used: f32 = v.iter().map(|c| c.1).sum();
        v.push(("Blockers / notes", (1.0 - used).max(0.15)));
        v
    }

    fn draw_table(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        let list = self.filtered();
        let cols = self.table_cols();
        let last_col = cols.len() - 1;
        let now = iyakki_agent::clock::now_unix();
        let zone = iyakki_agent::clock::Zone::system();
        let total: f32 = cols.iter().map(|c| c.1).sum();
        let mut x = r.x;
        let xs: Vec<f32> = cols.iter().map(|c| {
            let v = x;
            x += r.w * c.1 / total;
            v
        }).collect();
        for (i, (name, w)) in cols.iter().enumerate() {
            let on = self.sort.0 == i;
            let label = format!("{}{}", name.to_uppercase(), if on { if self.sort.1 { " ▲" } else { " ▼" } } else { "" });
            let b = R::new(xs[i], r.y, r.w * w / total, 22.0 * s);
            g.text(xs[i] + 22.0 * s * (i == 0) as u8 as f32 + 4.0 * s, r.y + 4.0 * s, 10.0 * s, if on { accent } else { MUTED }, &label, Face::Bold);
            g.hit(b, Event::Tab(TAB_BASE + if i < 10 { 20 + i } else { 60 + i - 10 }), &format!("sort {name}"));
        }
        g.gradient_rect(r.x, r.y + 20.0 * s, r.w, 1.0, (r.x, 0.0), (r.x + r.w, 0.0), &[(0.0, with_alpha(accent, 140)), (1.0, with_alpha(accent, 10))]);
        let body = R::new(r.x, r.y + 24.0 * s, r.w, (r.h - 24.0 * s).max(0.0));
        let rh = 26.0 * s;
        self.visible_rows = ((body.h / rh) as usize).max(1);
        if self.sel < self.table_scroll {
            self.table_scroll = self.sel;
        } else if self.sel >= self.table_scroll + self.visible_rows {
            self.table_scroll = self.sel + 1 - self.visible_rows;
        }
        self.table_scroll = self.table_scroll.min(list.len().saturating_sub(self.visible_rows));
        if list.is_empty() {
            g.text(body.x + 8.0 * s, body.y + 10.0 * s, 13.0 * s, DIM, "No feature matches the filters (Esc clears them).", Face::Sans);
            return;
        }
        for (i, f) in list.iter().enumerate().skip(self.table_scroll).take(self.visible_rows) {
            let y = body.y + (i - self.table_scroll) as f32 * rh;
            let rect = R::new(body.x, y, body.w - 8.0 * s, rh - 2.0 * s);
            let selected = i == self.sel;
            let hot = g.hovered(rect);
            if selected {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 7.0 * s, fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })));
                g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 7.0 * s, 1.0, with_alpha(accent, if focused { 190 } else { 90 }));
            } else if hot {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 7.0 * s, rgb(20, 27, 50));
            }
            let col = status_color(&f.status);
            g.circle(rect.x + 10.0 * s, rect.y + rect.h / 2.0, 4.0 * s, col);
            let bl = f.blockers();
            let last = if !bl.is_empty() { format!("⚠ {}", bl.join("; ")) } else { f.notes.clone() };
            let local = |t: &str| fb::parse_stamp(t).map(|u| iyakki_agent::clock::format(&zone, u, "%d %b %H:%M")).unwrap_or_default();
            let (n_tests, rt, cov) = self.row_quality(f);
            let warn_no_tests = n_tests == 0 && matches!(f.status.as_str(), "partial" | "done");
            let cells: Vec<String> = cols
                .iter()
                .map(|(name, _)| match *name {
                    "ID" => f.id.clone(),
                    "Feature" => f.title.clone(),
                    "Phase" => f.phase.clone(),
                    "Status" => f.status.clone(),
                    "Pri" => f.priority.clone(),
                    "Progress" => f.progress.map_or(String::new(), |p| format!("{p}%")),
                    "Started" => local(&f.started),
                    "Finished" => local(&f.finished),
                    "Took" => f.duration(now).map(fb::fmt_duration).unwrap_or_default(),
                    "Sev" => f.severity.clone(),
                    "Tests" if warn_no_tests => "⚠ no tests".to_string(),
                    "Tests" if n_tests == 0 => "0".to_string(),
                    "Tests" => if rt.label().is_empty() { format!("{n_tests}") } else { format!("{n_tests} · {}", rt.label()) },
                    "Cov" => cov.map_or("-".to_string(), |(cv, t)| format!("{}%", cv * 100 / t.max(1))),
                    "Depends" => self.depends_cell(f),
                    "Agent" => self.agent_cell(f),
                    _ => last.clone(),
                })
                .collect();
            for (ci, c) in cells.iter().enumerate() {
                let w = r.w * cols[ci].1 / total - 10.0 * s - if ci == 0 { 22.0 * s } else { 0.0 };
                let cx = xs[ci] + 4.0 * s + if ci == 0 { 22.0 * s } else { 0.0 };
                let color = match (ci, cols[ci].0) {
                    (0, _) => accent,
                    (3, _) => col,
                    (_, "Sev") => match f.severity.as_str() { "high" => ERR, "med" | "medium" => AMBER, _ => MUTED },
                    (_, "Tests") if warn_no_tests => AMBER,
                    (_, "Tests") if rt.failed > 0 => ERR,
                    (_, "Tests") if rt.all_passed() => OK,
                    (_, "Tests") => MUTED,
                    (c, _) if c == last_col && !bl.is_empty() => ERR,
                    (c, _) if c == last_col => MUTED,
                    _ => rgb(205, 212, 232),
                };
                let t = g.fit(12.0 * s, c, if ci == 3 || ci == 0 { Face::Bold } else { Face::Sans }, w.max(8.0));
                g.text(cx, rect.y + (rect.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, color, &t, if ci == 3 || ci == 0 { Face::Bold } else { Face::Sans });
            }
            g.hit(rect, Event::Row(ROW_BASE + i), &f.id);
        }
        fx::scrollbar(g, R::new(r.x + r.w - 6.0 * s, body.y, 6.0 * s, body.h), s, self.table_scroll, self.visible_rows, list.len(), accent);
    }

    fn draw_phases(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        let list = self.filtered();
        let rh = 24.0 * s;
        // rows: a heading per phase, then its features
        enum L<'a> {
            Head(String, usize, usize),
            Item(usize, &'a Feature),
        }
        let mut rows: Vec<L> = Vec::new();
        let mut i = 0;
        while i < list.len() {
            let ph = list[i].phase.clone();
            let group: Vec<(usize, &Feature)> = list.iter().enumerate().skip(i).take_while(|(_, f)| f.phase == ph).collect();
            let done = group.iter().filter(|(_, f)| f.status == "done").count();
            rows.push(L::Head(if ph.is_empty() { "no phase".into() } else { ph }, done, group.len()));
            for (gi, f) in &group {
                rows.push(L::Item(*gi, f));
            }
            i += group.len();
        }
        self.visible_rows = ((r.h / rh) as usize).max(1);
        // keep the selected feature in view
        let sel_row = rows.iter().position(|l| matches!(l, L::Item(i, _) if *i == self.sel)).unwrap_or(0);
        if sel_row < self.table_scroll {
            self.table_scroll = sel_row.saturating_sub(1);
        } else if sel_row >= self.table_scroll + self.visible_rows {
            self.table_scroll = sel_row + 1 - self.visible_rows;
        }
        self.table_scroll = self.table_scroll.min(rows.len().saturating_sub(self.visible_rows));
        for (k, l) in rows.iter().enumerate().skip(self.table_scroll).take(self.visible_rows) {
            let y = r.y + (k - self.table_scroll) as f32 * rh;
            match l {
                L::Head(ph, done, n) => {
                    g.text(r.x + 6.0 * s, y + 4.0 * s, 13.0 * s, CYAN, &format!("PHASE {}", ph.trim_start_matches('P')), Face::Bold);
                    let bar = R::new(r.x + 110.0 * s, y + 8.0 * s, (r.w * 0.3).min(260.0 * s), 7.0 * s);
                    fx::progress_bar(g, bar, *done as f32 / (*n).max(1) as f32, OK, s);
                    g.text(bar.x + bar.w + 10.0 * s, y + 4.0 * s, 12.0 * s, MUTED, &format!("{done}/{n} done"), Face::Bold);
                    g.rect(r.x, y + rh - 3.0 * s, r.w - 8.0 * s, 1.0, with_alpha(CYAN, 40));
                }
                L::Item(i, f) => {
                    let rect = R::new(r.x + 10.0 * s, y, r.w - 24.0 * s, rh - 2.0 * s);
                    let selected = *i == self.sel;
                    if selected {
                        g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })));
                        g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 6.0 * s, 1.0, with_alpha(accent, if focused { 190 } else { 90 }));
                    } else if g.hovered(rect) {
                        g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, rgb(20, 27, 50));
                    }
                    let col = status_color(&f.status);
                    g.circle(rect.x + 10.0 * s, rect.y + rect.h / 2.0, 4.0 * s, col);
                    g.text(rect.x + 22.0 * s, rect.y + 4.0 * s, 12.0 * s, accent, &f.id, Face::Bold);
                    let tx = rect.x + 112.0 * s;
                    let t = g.fit(12.0 * s, &f.short_title(), Face::Sans, (rect.w - 112.0 * s - 150.0 * s).max(20.0));
                    g.text(tx, rect.y + 4.0 * s, 12.0 * s, rgb(205, 212, 232), &t, Face::Sans);
                    let st = format!("{}{}", f.status, f.progress.map_or(String::new(), |p| format!(" {p}%")));
                    let sw = g.text_width(12.0 * s, &st, Face::Bold);
                    g.text(rect.x + rect.w - sw - 8.0 * s, rect.y + 4.0 * s, 12.0 * s, col, &st, Face::Bold);
                    g.hit(rect, Event::Row(ROW_BASE + i), &f.id);
                }
            }
        }
    }

    fn draw_drawer(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
        let Some(f) = self.selected() else { return };
        fx::panel(g, r, s, accent);
        let col = status_color(&f.status);
        let x = r.x + 14.0 * s;
        let w = r.w - 28.0 * s;
        let mut y = r.y + 10.0 * s;
        self.pane.begin(R::new(r.x, r.y, r.w, r.h));
        g.text(x, y, 16.0 * s, accent, &f.id, Face::Bold);
        let cl = g.text_width(11.0 * s, "Esc close", Face::Bold) + 14.0 * s;
        let cb = R::new(r.x + r.w - 12.0 * s - cl, y - 1.0 * s, cl, 22.0 * s);
        g.rrect(cb.x, cb.y, cb.w, cb.h, 11.0 * s, fx::WELL);
        g.text(cb.x + 7.0 * s, cb.y + 4.0 * s, 11.0 * s, MUTED, "Esc close", Face::Bold);
        g.hit(cb, Event::Sub(SUB, Key::Esc), "close details");
        y += 28.0 * s;
        // status chips (click to move)
        let mut cx = x;
        for (i, l) in self.lanes().iter().enumerate() {
            let on = *l == f.status;
            let lc = status_color(l);
            let cwid = g.text_width(11.0 * s, l, Face::Bold) + 16.0 * s;
            if cx + cwid > x + w {
                cx = x;
                y += 26.0 * s;
            }
            let b = R::new(cx, y, cwid, 22.0 * s);
            g.rrect(b.x, b.y, b.w, b.h, 11.0 * s, if on { fx::over(fx::WELL, with_alpha(lc, 100)) } else if g.hovered(b) { rgb(34, 44, 78) } else { fx::WELL });
            g.rrect_outline(b.x, b.y, b.w, b.h, 11.0 * s, if on { 1.6 * s } else { 1.0 }, with_alpha(lc, if on { 255 } else { 110 }));
            g.text(b.x + 8.0 * s, b.y + 5.0 * s, 11.0 * s, if on { TEXT } else { MUTED }, l, Face::Bold);
            g.hit(b, Event::Tab(TAB_BASE + 60 + i), &format!("move to {l}"));
            cx += cwid + 5.0 * s;
        }
        y += 32.0 * s;
        let mut para = 0u32;
        let line = |g: &mut Gfx, pane: &mut textsel::Pane, y: &mut f32, txt: &str, size: f32, face: Face, color: [u8; 4], para: &mut u32| {
            *para += 1;
            for l in g.wrap(size, txt, face, w) {
                if *y + size * 1.4 > r.y + r.h - 40.0 * s {
                    return;
                }
                g.text(x, *y, size, color, &l, face);
                pane.line(g, x, *y, size * 1.3, size, face, &l, *para, *para);
                *y += size * 1.35;
            }
        };
        line(g, &mut self.pane, &mut y, &f.title, 14.0 * s, Face::Bold, TEXT, &mut para);
        y += 8.0 * s;
        // phase, priority, progress
        let mut px = x;
        for (label, val, n) in [("phase", f.phase.clone(), "phase"), ("priority", f.priority.clone(), "priority")] {
            if val.is_empty() {
                continue;
            }
            let t = format!("{label} {val}");
            let cwid = g.text_width(11.0 * s, &t, Face::Bold) + 16.0 * s;
            let b = R::new(px, y, cwid, 22.0 * s);
            let hot = g.hovered(b);
            g.rrect(b.x, b.y, b.w, b.h, 11.0 * s, if hot { rgb(34, 44, 78) } else { fx::WELL });
            g.rrect_outline(b.x, b.y, b.w, b.h, 11.0 * s, 1.0, if hot { accent } else { rgb(60, 70, 104) });
            g.text(b.x + 8.0 * s, b.y + 5.0 * s, 11.0 * s, CYAN, &t, Face::Bold);
            g.hit(b, Event::Sub(SUB, Key::Char(if n == "phase" { 'p' } else { 'P' })), &format!("cycle {label}"));
            px += cwid + 5.0 * s;
        }
        y += 30.0 * s;
        if let Some(p) = f.progress {
            let bar = R::new(x + 64.0 * s, y + 4.0 * s, w - 64.0 * s - 90.0 * s, 8.0 * s);
            g.text(x, y, 11.0 * s, MUTED, "progress", Face::Bold);
            fx::progress_bar(g, bar, p as f32 / 100.0, col, s);
            g.text(bar.x + bar.w + 8.0 * s, y, 12.0 * s, TEXT, &format!("{p}%"), Face::Bold);
            let (mb, pb) = (R::new(x + w - 42.0 * s, y - 2.0 * s, 20.0 * s, 20.0 * s), R::new(x + w - 18.0 * s, y - 2.0 * s, 20.0 * s, 20.0 * s));
            for (b, lab, key) in [(mb, "−", '-'), (pb, "+", '+')] {
                g.rrect(b.x, b.y, b.w, b.h, 6.0 * s, if g.hovered(b) { rgb(40, 50, 86) } else { fx::WELL });
                g.text(b.x + 6.0 * s, b.y + 2.0 * s, 14.0 * s, TEXT, lab, Face::Bold);
                g.hit(b, Event::Sub(SUB, Key::Char(key)), &format!("progress {lab}"));
            }
            y += 26.0 * s;
        }
        // blockers
        let bl = f.blockers();
        g.text(x, y, 10.0 * s, DIM, "BLOCKERS", Face::Bold);
        y += 15.0 * s;
        if bl.is_empty() {
            g.text(x, y, 12.0 * s, DIM, "none", Face::Sans);
            y += 18.0 * s;
        }
        for b in &bl {
            line(g, &mut self.pane, &mut y, &format!("⚠ {b}"), 12.0 * s, Face::Sans, ERR, &mut para);
        }
        y += 4.0 * s;
        y += self.draw_drawer_deps(g, &f, x, y, w, r.y + r.h - 70.0 * s, s, accent);
        g.text(x, y, 10.0 * s, DIM, "COMMENTS", Face::Bold);
        y += 15.0 * s;
        if f.notes.is_empty() {
            g.text(x, y, 12.0 * s, DIM, "none", Face::Sans);
            y += 18.0 * s;
        } else {
            line(g, &mut self.pane, &mut y, &f.notes, 12.0 * s, Face::Sans, rgb(205, 212, 232), &mut para);
        }
        y += 4.0 * s;
        g.text(x, y, 10.0 * s, DIM, "TESTS", Face::Bold);
        y += 15.0 * s;
        if f.tests.is_empty() {
            g.text(x, y, 12.0 * s, DIM, "none listed", Face::Sans);
            y += 18.0 * s;
        }
        for (i, t) in f.tests.iter().enumerate().take(6) {
            if y + 18.0 * s > r.y + r.h - 70.0 * s {
                break;
            }
            let tr = R::new(x, y, w, 18.0 * s);
            let hot = g.hovered(tr);
            let txt = g.fit(11.0 * s, t, Face::Mono, w - 6.0);
            g.text(x, y + 1.0 * s, 11.0 * s, if hot { accent } else { fx::readable(CYAN, rgb(8, 11, 26), 4.5) }, &txt, Face::Mono);
            g.hit(tr, Event::Tab(TAB_BASE + 100 + i), &format!("open test {t}"));
            y += 18.0 * s;
        }
        if f.is_bug() {
            y += 4.0 * s;
            g.text(x, y, 10.0 * s, ERR, "BUG", Face::Bold);
            y += 15.0 * s;
            let head = format!("severity {} · found {}{}", if f.severity.is_empty() { "-" } else { &f.severity }, if f.found.is_empty() { "-" } else { &f.found }, if f.fix.is_empty() { String::new() } else { format!(" · fix {}", f.fix) });
            line(g, &mut self.pane, &mut y, &head, 11.0 * s, Face::Sans, rgb(205, 212, 232), &mut para);
            if !f.repro.is_empty() {
                line(g, &mut self.pane, &mut y, &format!("repro: {}", f.repro), 11.0 * s, Face::Sans, rgb(205, 212, 232), &mut para);
            }
            for (i, p) in f.logs.iter().enumerate().take(6) {
                if y + 20.0 * s > r.y + r.h - 70.0 * s {
                    break;
                }
                let row = R::new(x, y, w, 20.0 * s);
                let (ow, cw) = (g.text_width(10.5 * s, "Open", Face::Bold) + 12.0 * s, g.text_width(10.5 * s, "Copy path", Face::Bold) + 12.0 * s);
                let txt = g.fit(11.0 * s, p, Face::Mono, (w - ow - cw - 14.0 * s).max(20.0));
                g.text(x, y + 3.0 * s, 11.0 * s, fx::readable(CYAN, rgb(8, 11, 26), 4.5), &txt, Face::Mono);
                g.button(x + w - ow - cw - 6.0 * s, y, 10.5 * s, "Open", CYAN, Event::Tab(TAB_BASE + 400 + i));
                g.button(x + w - cw, y, 10.5 * s, "Copy path", MUTED, Event::Tab(TAB_BASE + 420 + i));
                let _ = row;
                y += 24.0 * s;
            }
        }
        // acceptance criteria are a checklist of what should hold, not tests: never counted, never run
        if !f.criteria.is_empty() {
            y += 4.0 * s;
            g.text(x, y, 10.0 * s, DIM, "CRITERIA (not tests)", Face::Bold);
            y += 15.0 * s;
            for t in f.criteria.iter().take(6) {
                if y + 18.0 * s > r.y + r.h - 70.0 * s {
                    break;
                }
                line(g, &mut self.pane, &mut y, &format!("☐ {t}"), 11.0 * s, Face::Sans, rgb(205, 212, 232), &mut para);
            }
        }
        let warns = f.warnings();
        if !warns.is_empty() && y + 18.0 * s < r.y + r.h - 44.0 * s {
            y += 4.0 * s;
            line(g, &mut self.pane, &mut y, &format!("⚠ {}", warns.join("; ")), 11.0 * s, Face::Sans, AMBER, &mut para);
        }
        self.pane.end(g, accent);
        // the buttons along the bottom
        let by = r.y + r.h - 36.0 * s;
        let mut bx = x;
        for (label, key, c) in [("Comment e", 'e', accent), ("Blocker b", 'b', AMBER), ("Ask AI i", 'i', MAGENTA), ("Tests o", 'o', CYAN), ("Delete x", 'x', ERR)] {
            let wd = g.text_width(11.0 * s, label, Face::Bold) + 16.0 * s;
            if bx + wd > r.x + r.w - 8.0 * s {
                break;
            }
            let b = R::new(bx, by, wd, 24.0 * s);
            let hot = g.hovered(b);
            g.rrect(b.x, b.y, b.w, b.h, 12.0 * s, if hot { rgb(40, 50, 86) } else { fx::WELL });
            g.rrect_outline(b.x, b.y, b.w, b.h, 12.0 * s, 1.0, with_alpha(c, if hot { 255 } else { 150 }));
            g.text(b.x + 8.0 * s, b.y + 6.0 * s, 11.0 * s, TEXT, label, Face::Bold);
            g.hit(b, Event::Sub(SUB, Key::Char(key)), label);
            bx += wd + 5.0 * s;
        }
    }

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4]) {
        let blink = (g.mouse.0 as u64 / 7) % 2 == 0 || true;
        match &self.modal {
            Some(Modal::Add(f)) | Some(Modal::Comment(_, _, f)) | Some(Modal::EditComment(_, f)) | Some(Modal::Blocker(_, f)) | Some(Modal::EditTitle(_, f)) => f.render(g, area, s, accent, blink),
            Some(Modal::Delete(id)) => {
                let p = fx::modal(g, area, s, 520.0 * s, 170.0 * s, ERR, true);
                g.text(p.x + 24.0 * s, p.y + 24.0 * s, 19.0 * s, TEXT, &format!("Delete {id} from the file?"), Face::Bold);
                g.text(p.x + 24.0 * s, p.y + 62.0 * s, 14.0 * s, MUTED, "The row is removed; Ctrl+Z brings it back.", Face::Sans);
                g.button(p.x + 24.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, "Delete", ERR, Event::Sub(SUB, Key::Char('y')));
                g.button(p.x + 140.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, "Cancel", MUTED, Event::Sub(SUB, Key::Esc));
            }
            Some(Modal::Conflict) => {
                let p = fx::modal(g, area, s, 600.0 * s, 210.0 * s, AMBER, false);
                g.text(p.x + 24.0 * s, p.y + 22.0 * s, 19.0 * s, TEXT, "The file changed on disk", Face::Bold);
                let lines = g.wrap(14.0 * s, "Someone (the editor, the assistant or another program) wrote the features file while you have unsaved edits on the board.", Face::Sans, p.w - 48.0 * s);
                for (i, l) in lines.iter().enumerate() {
                    g.text(p.x + 24.0 * s, p.y + 56.0 * s + i as f32 * 19.0 * s, 14.0 * s, MUTED, l, Face::Sans);
                }
                let by = p.y + p.h - 58.0 * s;
                let mut bx = p.x + 24.0 * s;
                for (label, c, key) in [("Reload (drop mine)  r", ERR, 'r'), ("Merge by row  m", accent, 'm'), ("Keep mine  k", AMBER, 'k')] {
                    bx += g.button(bx, by, 13.0 * s, label, c, Event::Sub(SUB, Key::Char(key))) + 10.0 * s;
                }
            }
            None => {}
        }
    }
}

thread_local! {
    static LAST_MOUSE: std::cell::Cell<(i32, i32)> = const { std::cell::Cell::new((-1, -1)) };
}

/// The text "ask the assistant about this feature" puts in the chat.
pub fn ask_text(f: &Feature) -> String {
    let mut t = format!("About feature {} on the board: {}\nStatus {}, phase {}, priority {}", f.id, f.title, f.status, f.phase, f.priority);
    if let Some(p) = f.progress {
        t.push_str(&format!(", progress {p}%"));
    }
    t.push('.');
    if !f.notes.is_empty() {
        t.push_str(&format!("\nNotes: {}", f.notes));
    }
    let bl = f.blockers();
    if !bl.is_empty() {
        t.push_str(&format!("\nBlockers: {}", bl.join("; ")));
    }
    if !f.tests.is_empty() {
        t.push_str(&format!("\nTests: {}", f.tests.join(", ")));
    }
    t.push_str("\nWhat is left to do, and what should I look at first?");
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURES: &str = "# T\n\n**Totals:** <!-- summary:start -->done 1 · partial 1 · planned 2 · blocked 0 · deprecated 0 · total 4<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | Alpha thing. More | P1 | done | P0 | 100% | `c::t::alpha_test` |  |\n| F-A-02 | Beta | P2 | partial | P1 | 40% | `c::t::beta` | Waiting for the vendor |\n| F-B-01 | Gamma | P3 | planned | P2 | 0% | — |  |\n| F-B-02 | Delta | P3 | planned | P3 | 0% | — |  |\n\ntail\n";

    pub(crate) fn world(tag: &str) -> (PathBuf, PathBuf) {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-board-ui-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/lib.rs"), "#[test]\nfn alpha_test() {}\n").unwrap();
        std::fs::write(d.join("FEATURES.md"), FEATURES).unwrap();
        (d.clone(), d.join("FEATURES.md"))
    }

    fn view(tag: &str) -> (BoardView, PathBuf, PathBuf) {
        let (root, f) = world(tag);
        let mut v = BoardView::new(&root, &f);
        v.autosave_after = Duration::ZERO;
        (v, root, f)
    }

    fn lines_changed(a: &str, b: &str) -> Vec<String> {
        a.lines().zip(b.lines()).filter(|(x, y)| x != y).map(|(_, y)| y.to_string()).collect()
    }

    fn draw(v: &mut BoardView, w: f32, h: f32) -> Gfx {
        let mut g = Gfx::new(w as u32, h as u32);
        g.clear(rgb(8, 11, 26));
        v.draw(&mut g, R::new(0.0, 0.0, w, h), 1.0, true, CYAN, 0);
        g
    }

    #[test]
    fn board_loads_the_table_and_shows_lanes_counts_and_progress() {
        let (mut v, _, _) = view("load");
        assert_eq!(v.lanes(), vec!["planned", "partial", "done", "blocked", "deprecated"]);
        assert_eq!(v.filtered().len(), 4);
        let f = v.file.as_ref().unwrap();
        assert_eq!(f.board.percent_done(), 25);
        let g = draw(&mut v, 1200.0, 600.0);
        assert!(g.hits.iter().any(|h| h.label == "F-B-01"), "the cards are clickable");
        assert!(g.hits.iter().any(|h| h.label == "Full screen") && g.hits.iter().any(|h| h.label == "Table"));
        for m in Mode::ALL {
            v.mode = m;
            draw(&mut v, 700.0, 400.0);
        }
    }

    #[test]
    fn board_moves_change_exactly_one_cell_and_sync_the_totals_when_saved() {
        let (mut v, _, f) = view("move");
        v.select_id("F-B-01");
        v.key(&Key::Char(']'));
        // planned -> partial needs a comment first
        assert!(v.has_modal(), "partial asks for a comment");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), FEATURES, "nothing is written yet");
        // an empty comment is refused
        v.handle(&Event::Key(Key::Enter));
        assert!(v.has_modal());
        for c in "half way there".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        assert!(!v.has_modal());
        assert!(v.is_dirty());
        assert_eq!(v.tick(), BoardAct::Saved(f.clone()), "written after the idle time");
        let text = std::fs::read_to_string(&f).unwrap();
        let changed = lines_changed(FEATURES, &text);
        assert_eq!(changed.len(), 2, "the row and the totals line: {changed:?}");
        assert!(changed.contains(&"| F-B-01 | Gamma | P3 | partial | P2 | 50% | — | half way there |".to_string()), "{changed:?}");
        assert!(text.contains("done 1 · partial 2 · planned 1 · blocked 0 · deprecated 0 · total 4"));
        assert!(f.with_file_name("FEATURES.md.bak").exists());
        // [ moves it back; planned needs no comment, 0% again
        v.select_id("F-B-01");
        v.key(&Key::Char('['));
        assert!(!v.has_modal());
        assert_eq!(v.tick(), BoardAct::Saved(f.clone()));
        assert!(std::fs::read_to_string(&f).unwrap().contains("| F-B-01 | Gamma | P3 | planned | P2 | 0% | — | half way there |"));
    }

    #[test]
    fn board_partial_and_blocked_need_a_comment_but_done_needs_none_and_warns_without_tests() {
        let (mut v, _, _) = view("rules");
        v.select_id("F-B-02");
        assert_eq!(v.set_status("F-B-02", "blocked"), Ok(false));
        assert!(v.has_modal());
        v.handle(&Event::Key(Key::Esc));
        assert!(!v.has_modal() && !v.is_dirty());
        // a row that already has notes moves straight away
        assert_eq!(v.set_status("F-A-02", "blocked"), Ok(true));
        assert!(v.is_dirty());
        assert_eq!(v.set_status("F-B-02", "done"), Ok(true));
        let f = v.file.as_ref().unwrap().board.get("F-B-02").unwrap();
        assert_eq!(f.progress, Some(100));
        assert!(f.warnings().iter().any(|w| w.contains("needs at least one test")), "{:?}", f.warnings());
        assert!(v.set_status("F-NOPE", "done").is_err());
    }

    #[test]
    fn board_edit_keys_phase_priority_progress_blockers_add_delete_undo() {
        let (mut v, _, f) = view("edits");
        v.select_id("F-A-02");
        v.key(&Key::Char('p'));
        assert_eq!(v.selected().unwrap().phase, "P3");
        v.key(&Key::Char('P'));
        assert_eq!(v.selected().unwrap().priority, "P2");
        v.key(&Key::Char('+'));
        assert_eq!(v.selected().unwrap().progress, Some(50));
        v.key(&Key::Char('b'));
        for c in "the new API".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        let sel = v.selected().unwrap();
        assert!(sel.notes.starts_with("Blocked by: the new API;"), "{}", sel.notes);
        assert!(sel.blockers().iter().any(|b| b.contains("the new API")));
        v.key(&Key::Char('B'));
        assert!(!v.selected().unwrap().notes.to_lowercase().contains("blocked by"));
        // add a feature: area prefix suggestion
        v.key(&Key::Char('a'));
        v.handle(&Event::Key(Key::Enter)); // area (prefilled A)
        for c in "Epsilon".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter)); // phase
        v.handle(&Event::Key(Key::Enter)); // comment
        v.handle(&Event::Key(Key::Enter)); // status
        v.handle(&Event::Key(Key::Enter)); // the last field submits
        assert!(!v.has_modal(), "{:?}", v.msg);
        assert!(v.file.as_ref().unwrap().board.get("F-A-03").is_some(), "the next free id of the area");
        assert_eq!(v.selected().unwrap().id, "F-A-03");
        // delete with confirmation
        v.key(&Key::Char('x'));
        assert!(v.has_modal());
        v.handle(&Event::Key(Key::Char('n')));
        assert!(v.file.as_ref().unwrap().board.get("F-A-03").is_some());
        v.key(&Key::Char('x'));
        v.handle(&Event::Key(Key::Char('y')));
        assert!(v.file.as_ref().unwrap().board.get("F-A-03").is_none());
        // everything undone step by step, in memory
        while v.is_dirty() {
            assert_eq!(v.undo(), BoardAct::Used);
        }
        assert_eq!(v.file.as_ref().unwrap().board.render(), FEATURES);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), FEATURES);
    }

    #[test]
    fn board_ctrl_z_after_a_save_restores_the_previous_file_from_the_bak() {
        let (mut v, _, f) = view("undo");
        v.select_id("F-B-02");
        v.key(&Key::Char('P'));
        assert_eq!(v.tick(), BoardAct::Saved(f.clone()));
        let saved = std::fs::read_to_string(&f).unwrap();
        assert_ne!(saved, FEATURES);
        // the in-memory stack still has the step: use it first, then save, then the .bak path
        let mut v2 = BoardView::new(&v.root, &f);
        v2.autosave_after = Duration::ZERO;
        assert_eq!(v2.undo(), BoardAct::Saved(f.clone()), "a fresh board has no memory: the .bak is the one step back");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), FEATURES);
        assert_eq!(v2.undo(), BoardAct::Saved(f.clone()), "and again is a redo");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), saved);
    }

    #[test]
    fn board_notices_changes_made_outside_and_reloads_or_asks() {
        let (mut v, _, f) = view("external");
        v.poll_every = Duration::ZERO;
        // nothing changed: nothing happens
        v.last_draw = Some(Instant::now());
        assert_eq!(v.poll_disk(), BoardAct::None);
        // changed outside, no edits here: reloaded by itself
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, FEATURES.replace("| F-B-02 | Delta | P3 | planned |", "| F-B-02 | Delta | P3 | blocked |")).unwrap();
        v.last_draw = Some(Instant::now());
        assert_eq!(v.tick(), BoardAct::Used);
        assert_eq!(v.file.as_ref().unwrap().board.get("F-B-02").unwrap().status, "blocked");
        assert!(v.msg.as_ref().unwrap().0.contains("reloaded"));
        // an edit here, then another outside change: a conflict, three ways out
        v.autosave_after = Duration::from_secs(3600);
        v.select_id("F-B-01");
        v.key(&Key::Char('P'));
        assert!(v.is_dirty());
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, std::fs::read_to_string(&f).unwrap().replace("| F-A-01 | Alpha thing. More | P1 | done |", "| F-A-01 | Alpha thing. More | P1 | deprecated |")).unwrap();
        v.last_draw = Some(Instant::now());
        v.tick();
        assert!(v.has_modal() && v.disk_changed, "the changed-on-disk bar");
        let g = draw(&mut v, 1000.0, 520.0);
        for l in ["Reload (drop mine)  r", "Merge by row  m", "Keep mine  k"] {
            assert!(g.hits.iter().any(|h| h.label == l), "{l}");
        }
        // merge: their change plus mine
        v.handle(&Event::Key(Key::Char('m')));
        let b = &v.file.as_ref().unwrap().board;
        assert_eq!(b.get("F-A-01").unwrap().status, "deprecated");
        assert_eq!(b.get("F-B-01").unwrap().priority, "P3", "my edit survived the merge");
        assert!(v.is_dirty());
        assert_eq!(v.save(), BoardAct::Saved(f.clone()));
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("deprecated") && text.contains("| F-B-01 | Gamma | P3 | planned | P3 |"));
        // reload: mine is dropped
        v.key(&Key::Char('P'));
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, text.replace("Delta", "Delta2")).unwrap();
        v.last_draw = Some(Instant::now());
        v.tick();
        v.handle(&Event::Key(Key::Char('r')));
        assert!(!v.is_dirty() && v.file.as_ref().unwrap().board.get("F-B-02").unwrap().title == "Delta2");
        // keep mine: saving replaces the file
        v.key(&Key::Char('P'));
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, std::fs::read_to_string(&f).unwrap().replace("Delta2", "Delta3")).unwrap();
        v.last_draw = Some(Instant::now());
        v.tick();
        v.handle(&Event::Key(Key::Char('k')));
        assert_eq!(v.save(), BoardAct::Saved(f.clone()));
        assert!(std::fs::read_to_string(&f).unwrap().contains("Delta2"), "mine won");
        // a deleted file is reported
        std::fs::remove_file(&f).unwrap();
        v.last_draw = Some(Instant::now());
        v.poll_disk();
        assert!(v.msg.as_ref().unwrap().0.contains("deleted"));
    }

    #[test]
    fn board_filters_sort_and_group_by_phase() {
        let (mut v, _, _) = view("filters");
        v.status_filter = Some("planned".into());
        assert_eq!(v.filtered().len(), 2);
        v.status_filter = None;
        v.area_filter = Some("B".into());
        assert!(v.filtered().iter().all(|f| f.area() == "B"));
        v.area_filter = None;
        for c in "vendor".chars() {
            v.text.handle(&Key::Char(c));
        }
        assert_eq!(v.filtered().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["F-A-02"], "free text finds notes and derived blockers");
        v.text.clear();
        v.phase_filter = Some("P3".into());
        assert_eq!(v.filtered().len(), 2);
        v.phase_filter = None;
        v.mode = Mode::Table;
        v.sort = (5, false);
        assert_eq!(v.filtered()[0].id, "F-A-01", "by progress, highest first");
        v.sort = (0, false);
        assert_eq!(v.filtered()[0].id, "F-B-02");
        v.mode = Mode::Phases;
        assert_eq!(v.filtered().iter().map(|f| f.phase.as_str()).collect::<Vec<_>>(), vec!["P1", "P2", "P3", "P3"]);
        // chips cycle the values
        v.mode = Mode::Kanban;
        v.chip_click(10);
        assert_eq!(v.status_filter.as_deref(), Some("planned"));
        v.chip_click(10);
        assert_eq!(v.status_filter.as_deref(), Some("partial"));
        v.chip_click(13);
        assert!(v.status_filter.is_none());
    }

    #[test]
    fn board_cards_move_between_lanes_by_mouse_drag() {
        let (mut v, _, f) = view("drag");
        v.autosave_after = Duration::ZERO;
        let g = draw(&mut v, 1250.0, 640.0);
        let card = g.hits.iter().find(|h| h.label == "F-B-02").expect("card").clone();
        let Event::Area(id) = card.ev else { panic!("{:?}", card.ev) };
        // press on the card, release over the DONE lane
        v.handle(&Event::AreaAt(id, 5.0, 5.0));
        assert_eq!(v.selected().unwrap().id, "F-B-02");
        let (_, done) = v.lane_rects.iter().find(|(l, _)| l == "done").cloned().unwrap();
        assert_eq!(v.handle(&Event::MouseUp((done.x + 30.0) as i32, (done.y + 60.0) as i32)), BoardAct::Used);
        assert_eq!(v.file.as_ref().unwrap().board.get("F-B-02").unwrap().status, "done");
        assert_eq!(v.tick(), BoardAct::Saved(f.clone()));
        // dropping on the lane it is already in changes nothing
        let g = draw(&mut v, 1250.0, 640.0);
        let card = g.hits.iter().find(|h| h.label == "F-B-01").unwrap().clone();
        let Event::Area(id) = card.ev else { panic!() };
        v.handle(&Event::AreaAt(id, 5.0, 5.0));
        let (_, planned) = v.lane_rects.iter().find(|(l, _)| l == "planned").cloned().unwrap();
        v.handle(&Event::MouseUp((planned.x + 30.0) as i32, (planned.y + 60.0) as i32));
        assert!(!v.is_dirty());
        // a drag to blocked asks for the comment first
        v.handle(&Event::AreaAt(id, 5.0, 5.0));
        let (_, blocked) = v.lane_rects.iter().find(|(l, _)| l == "blocked").cloned().unwrap();
        v.handle(&Event::MouseUp((blocked.x + 30.0) as i32, (blocked.y + 60.0) as i32));
        assert!(v.has_modal());
    }

    #[test]
    fn board_tests_open_by_name_and_the_assistant_gets_the_row() {
        let (mut v, _, _) = view("tests");
        v.select_id("F-A-01");
        assert_eq!(v.key(&Key::Char('o')), BoardAct::Open("src/lib.rs".into(), 2));
        v.select_id("F-B-01");
        v.key(&Key::Char('o'));
        assert!(v.msg.as_ref().unwrap().0.contains("no tests"));
        v.select_id("F-A-02");
        match v.key(&Key::Char('i')) {
            BoardAct::Ask(t) => assert!(t.contains("F-A-02") && t.contains("Waiting for the vendor") && t.contains("Blockers:") && t.contains("40%"), "{t}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(v.key(&Key::Char('v')), BoardAct::ToggleFull);
    }

    #[test]
    fn board_without_a_table_or_file_explains_itself() {
        let (root, f) = world("none");
        std::fs::write(&f, "# nothing here\n").unwrap();
        let mut v = BoardView::new(&root, &f);
        assert!(v.load_error.as_deref().unwrap().contains("no feature table"));
        draw(&mut v, 800.0, 500.0);
        std::fs::remove_file(&f).unwrap();
        let mut v = BoardView::new(&root, &f);
        assert!(v.file.is_none() && v.filtered().is_empty());
        draw(&mut v, 800.0, 500.0);
        assert_eq!(v.key(&Key::Char('a')), BoardAct::Used);
    }

    #[test]
    fn board_without_a_file_offers_create_board_which_writes_the_template_once_and_not_when_read_only() {
        let (root, f) = world("create");
        let _ = std::fs::remove_file(&f);
        let mut v = BoardView::new(&root, &f);
        draw(&mut v, 800.0, 500.0);
        v.read_only = true;
        assert_eq!(v.handle(&Event::Key(Key::Char('C'))), BoardAct::Used);
        assert!(!f.exists(), "read-only: nothing is created");
        draw(&mut v, 800.0, 500.0);
        v.read_only = false;
        assert_eq!(v.handle(&Event::Key(Key::Char('C'))), BoardAct::Saved(f.clone()));
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("| ID | Feature | Status | Tests | Started | Finished | Notes |") && text.contains("| F-01 |"), "{text}");
        assert!(v.file.is_some() && v.load_error.is_none() && v.filtered().len() == 1, "the new board is shown");
        // never over an existing file
        std::fs::write(&f, "mine\n").unwrap();
        assert_eq!(v.create_board(), BoardAct::Used);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "mine\n");
    }

    #[test]
    fn cards_and_table_show_when_work_started_finished_and_how_long() {
        let z = iyakki_agent::clock::Zone::utc();
        let t0 = fb::parse_stamp("2026-10-14 09:12").unwrap();
        let (root, f) = world("times");
        std::fs::write(&f, "| ID | Feature | Status | Tests | Started | Finished | Notes |\n|---|---|---|---|---|---|---|\n| T-1 | one | partial | — | 2026-10-14 09:12 |  | n |\n| T-2 | two | done | — | 2026-10-14 09:12 | 2026-10-14 09:15 |  |\n| T-3 | three | planned | — |  |  |  |\n").unwrap();
        let mut v = BoardView::new(&root, &f);
        let list = v.filtered();
        let by = |id: &str| list.iter().find(|x| x.id == id).unwrap().clone();
        assert_eq!(time_line(&by("T-1"), t0 + 180, &z).as_deref(), Some("started 09:12 · 3 min"));
        assert_eq!(time_line(&by("T-2"), t0 + 9999, &z).as_deref(), Some("done 09:15 · took 3 min"));
        assert_eq!(time_line(&by("T-3"), t0, &z), None);
        for mode in [Mode::Kanban, Mode::Table, Mode::Phases] {
            v.mode = mode;
            draw(&mut v, 900.0, 500.0);
            draw(&mut v, 420.0, 500.0);
        }
        v.mode = Mode::Table;
        assert_eq!(v.table_cols().iter().map(|c| c.0).collect::<Vec<_>>(), ["ID", "Feature", "Phase", "Status", "Pri", "Progress", "Started", "Finished", "Took", "Tests", "Blockers / notes"]);
        v.handle(&Event::Tab(TAB_BASE + 26));
        assert_eq!(v.filtered().first().map(|x| x.id.as_str()), Some("T-3"), "sorted by Started ascending: the row that has not started comes first");
        // a board without the columns keeps its seven columns and a drag does not add them
        let (root2, f2) = world("notimes");
        std::fs::write(&f2, "| ID | Feature | Status | Notes |\n|---|---|---|---|\n| A-1 | one | planned |  |\n").unwrap();
        let v2 = BoardView::new(&root2, &f2);
        assert_eq!(v2.table_cols().len(), 7);
        assert_eq!(v2.stamp_at(), None);
    }

    #[test]
    fn tests_and_coverage_show_on_cards_in_the_table_and_in_the_header_and_old_boards_are_unchanged() {
        use iyakki_agent::testresults::{Outcome, Results};
        let (root, f) = world("quality");
        std::fs::write(&f, "| ID | Feature | Status | Tests | Files | Notes |\n|---|---|---|---|---|---|\n| Q-1 | cart | done | `cart::adds`<br>`cart::removes` | src/cart.js |  |\n| Q-2 | tax | partial | — |  | wip |\n| Q-3 | menu | planned | `menu::opens` | src/ui/*.js |  |\n| Q-4 | done and untested | done |  |  |  |\n").unwrap();
        iyakki_agent::testresults::save(&root, &Results { ran_at: iyakki_agent::clock::now_unix() - 120, tests: [("cart::adds", Outcome::Pass), ("cart::removes", Outcome::Fail), ("other::unlisted", Outcome::Pass)].map(|(k, v)| (k.to_string(), v)).into() });
        iyakki_agent::coverage::save(&root, &iyakki_agent::coverage::Coverage { files: [("src/cart.js", (8, 10)), ("src/ui/menu.js", (1, 4))].map(|(k, v)| (k.to_string(), v)).into(), ran_at: iyakki_agent::clock::now_unix() });
        let mut v = BoardView::new(&root, &f);
        v.mode = Mode::Table;
        let by = |v: &BoardView, id: &str| v.filtered().into_iter().find(|x| x.id == id).unwrap();
        // per row: counts, last results, coverage of the named files, "no tests" for started rows without any
        let (n, rt, cov) = v.row_quality(&by(&v, "Q-1"));
        assert_eq!((n, rt.passed, rt.failed, cov), (2, 1, 1, Some((8, 10))));
        assert_eq!(rt.label(), "1/2 pass · 1 fail");
        assert_eq!(v.row_quality(&by(&v, "Q-3")).1.label(), "not run");
        assert_eq!(v.row_quality(&by(&v, "Q-3")).2, Some((1, 4)), "a glob: src/ui/*.js");
        assert_eq!(v.row_quality(&by(&v, "Q-2")).2, None, "no Files, no number");
        assert_eq!(v.quality_line(&by(&v, "Q-1")).unwrap().0, "2 tests · 1/2 pass · 1 fail · cov 80%");
        assert_eq!(v.quality_line(&by(&v, "Q-2")).unwrap().0, "⚠ no tests");
        assert_eq!(v.quality_line(&by(&v, "Q-4")).unwrap().0, "⚠ no tests");
        assert_eq!(v.quality_line(&by(&v, "Q-3")).unwrap().0, "1 test · not run · cov 25%");
        assert_eq!(v.unlinked_tests(), vec!["other::unlisted".to_string()]);
        // the Table has sortable Tests and Cov columns
        let names: Vec<&str> = v.table_cols().iter().map(|c| c.0).collect();
        assert_eq!(names, ["ID", "Feature", "Phase", "Status", "Pri", "Progress", "Tests", "Cov", "Blockers / notes"]);
        let ti = names.iter().position(|n| *n == "Tests").unwrap();
        v.sort = (ti, false);
        assert_eq!(v.filtered().first().map(|x| x.id.clone()), Some("Q-1".to_string()), "most tests first");
        v.sort = (ti, true);
        assert_eq!(v.filtered().last().map(|x| x.id.clone()), Some("Q-1".to_string()));
        let ci = names.iter().position(|n| *n == "Cov").unwrap();
        v.sort = (ci, false);
        assert_eq!(v.filtered().first().map(|x| x.id.clone()), Some("Q-1".to_string()), "best coverage first");
        // buttons
        assert_eq!(v.handle(&Event::Tab(TAB_BASE + 62)), BoardAct::RunTests);
        assert_eq!(v.handle(&Event::Tab(TAB_BASE + 63)), BoardAct::RunCoverage);
        match v.handle(&Event::Tab(TAB_BASE + 64)) {
            BoardAct::Ask(t) => assert!(t.contains("other::unlisted") && t.contains("board_link_tests")),
            other => panic!("{other:?}"),
        }
        // every view draws at the IDE's sizes and in narrow panes, controls inside the pane
        for mode in [Mode::Kanban, Mode::Table, Mode::Phases] {
            v.mode = mode;
            for (w, h) in [(1280.0, 720.0), (900.0, 500.0), (420.0, 500.0)] {
                let g = draw(&mut v, w, h);
                for hit in g.hits.iter().filter(|x| matches!(x.label.as_str(), "Run tests" | "Coverage" | "Link tests")) {
                    assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w + 1.0, "{} inside {w}: {:?}", hit.label, hit.rect);
                }
            }
        }
        let g = draw(&mut v, 1280.0, 720.0);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        assert!(["Run tests", "Coverage", "Link tests"].iter().all(|l| labels.contains(l)), "{labels:?}");
        // an old board without a Tests column and without stored results is unchanged
        let (root2, f2) = world("quality-old");
        std::fs::write(&f2, "| ID | Feature | Status | Notes |\n|---|---|---|---|\n| A-1 | one | done |  |\n").unwrap();
        let mut old = BoardView::new(&root2, &f2);
        old.mode = Mode::Table;
        assert_eq!(old.table_cols().len(), 7, "no Tests or Cov column");
        assert!(old.quality_line(&by(&old, "A-1")).is_none());
        let g = draw(&mut old, 1280.0, 720.0);
        assert!(!g.hits.iter().any(|h| h.label == "Run tests"), "no quality line on a board that tracks no tests");
        // a test run stores its results; the board loads them when the file changes
        let mut v2 = BoardView::new(&root, &f);
        v2.refresh_quality(true);
        assert!(v2.results.is_some() && v2.coverage.is_some());
        iyakki_agent::testresults::save(&root, &Results { ran_at: 5, tests: [("cart::adds".to_string(), Outcome::Pass)].into() });
        std::thread::sleep(Duration::from_millis(30));
        v2.refresh_quality(false);
        assert_eq!(v2.results.as_ref().unwrap().tests.len(), 1);
    }

    #[test]
    fn bugs_have_a_chip_a_filter_severity_sorting_and_evidence_links_and_old_boards_are_unchanged() {
        let (root, f) = world("bugs");
        std::fs::create_dir_all(root.join(".iyakki/bugs/B-APP-01")).unwrap();
        std::fs::write(root.join(".iyakki/bugs/B-APP-01/summary.md"), "# B-APP-01").unwrap();
        std::fs::write(&f, "| ID | Feature | Status | Severity | Repro | Logs | Found | Fix | Notes |\n|---|---|---|---|---|---|---|---|---|\n| F-APP-01 | the app | partial | | | | | | wip |\n| B-APP-01 | web crashes at start | planned | high | restart the deployment | .iyakki/bugs/B-APP-01/ | 2026-10-14 09:00 | F-APP-01 | |\n| B-APP-02 | wrong total | partial | low | add two items | | 2026-10-14 09:05 | | n |\n| B-APP-03 | old one | done | med | | | | | |\n").unwrap();
        let mut v = BoardView::new(&root, &f);
        v.mode = Mode::Table;
        assert_eq!(v.filtered().iter().filter(|x| x.is_bug()).count(), 3);
        // the header chip counts open bugs and a click filters to bugs
        let g = draw(&mut v, 1280.0, 720.0);
        let chip = g.hits.iter().find(|h| h.label == "bugs").expect("the Bugs chip").clone();
        assert!(chip.rect.x >= 0.0 && chip.rect.x + chip.rect.w <= 1280.0);
        v.handle(&chip.ev);
        assert!(v.bugs_only && v.filtered().iter().all(|x| x.is_bug()) && v.filtered().len() == 3);
        v.handle(&chip.ev);
        assert!(!v.bugs_only);
        // a Severity column that sorts
        let names: Vec<&str> = v.table_cols().iter().map(|c| c.0).collect();
        let si = names.iter().position(|n| *n == "Sev").expect("a Sev column");
        v.sort = (si, false);
        assert_eq!(v.filtered().first().map(|x| x.id.clone()), Some("B-APP-01".to_string()), "high first");
        v.sort = (si, true);
        assert_eq!(v.filtered().iter().filter(|x| x.is_bug()).next().map(|x| x.id.clone()), Some("B-APP-02".to_string()), "ascending: the low one comes before med and high");
        // the card detail: Open opens the folder's summary, Copy path copies the absolute path
        v.mode = Mode::Kanban;
        v.select_id("B-APP-01");
        v.drawer = true;
        let g = draw(&mut v, 1280.0, 720.0);
        let open = g.hits.iter().find(|h| h.label == "Open" && matches!(h.ev, Event::Tab(t) if t >= TAB_BASE + 400 && t < TAB_BASE + 420)).expect("Open for the log folder").clone();
        assert_eq!(v.handle(&open.ev), BoardAct::Open(".iyakki/bugs/B-APP-01/summary.md".to_string(), 1));
        let copy = g.hits.iter().find(|h| h.label == "Copy path").expect("Copy path").clone();
        assert_eq!(v.handle(&copy.ev), BoardAct::Used);
        assert!(crate::ui::clipboard::history().iter().any(|t| t.ends_with(".iyakki/bugs/B-APP-01") && t.starts_with('/')), "{:?}", crate::ui::clipboard::history());
        for (w, h) in [(1280.0, 720.0), (900.0, 500.0), (420.0, 500.0)] {
            for mode in [Mode::Kanban, Mode::Table, Mode::Phases] {
                v.mode = mode;
                draw(&mut v, w, h);
            }
        }
        // a board without bug rows or columns shows no Bugs chip and no Sev column
        let (root2, f2) = world("nobugs");
        let mut old = BoardView::new(&root2, &f2);
        old.mode = Mode::Table;
        let g = draw(&mut old, 1280.0, 720.0);
        assert!(!g.hits.iter().any(|h| h.label == "bugs"));
        assert!(!old.table_cols().iter().any(|c| c.0 == "Sev"));
    }

    #[test]
    fn board_works_on_another_projects_table_with_a_blockers_column() {
        let (root, f) = world("other");
        std::fs::write(&f, "| ID | Feature | Status | Blockers | Owner |\n|---|---|---|---|---|\n| X-1 | one | open | db | ann |\n| X-2 | two | blocked | X-1 | bob |\n| X-3 | three | open |  | cy |\n").unwrap();
        let mut v = BoardView::new(&root, &f);
        v.autosave_after = Duration::ZERO;
        assert_eq!(v.lanes(), vec!["planned", "partial", "done", "blocked", "deprecated", "open"]);
        v.select_id("X-3");
        v.key(&Key::Char('b'));
        for c in "X-1".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        v.tick();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("| X-3 | three | open | X-1 | cy |"), "{text}");
        assert!(text.contains("| X-1 | one | open | db | ann |"), "untouched rows stay as they were");
        for m in Mode::ALL {
            v.mode = m;
            draw(&mut v, 900.0, 500.0);
        }
    }
}
