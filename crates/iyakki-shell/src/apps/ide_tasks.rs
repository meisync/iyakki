//! The Tasks tab of the AI IDE: parallel tasks as cards on the left, the selected task on the right (summary, files, diff, log)
//! with its controls. See `docs/PARALLEL-TASKS.md`; the scheduler, isolation and merge are `iyakki_agent::tasks`.
//!
//! One keyboard owner: the tab's letters are shortcuts (it has no text field); every text input (new task, follow-up) is an IDE
//! dialog. Git work (the diff, the dry run of a merge) runs on a worker thread, never in a frame.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use iyakki_agent::agent::Verdict;
use iyakki_agent::tasks::{Manager, MergeCheck, Status, Task};

use super::ide_chrome as chrome;
use super::ScrollLog;
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::textsel;
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

pub const ROW_BASE: usize = 600_000;
pub const FILE_BASE: usize = 620_000;
pub const TAB_BASE: usize = 9_700_000;
/// `Event::Tab(APPROVE_BASE + 2 * i)` allows the i-th waiting approval, `+ 1` denies it.
pub const APPROVE_BASE: usize = 9_700_100;
pub const SUB: u8 = 36;
pub const DETAIL_AREA: usize = 63_501;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DTab {
    Overview,
    Files,
    Diff,
    Log,
}

impl DTab {
    const ALL: [DTab; 4] = [DTab::Overview, DTab::Files, DTab::Diff, DTab::Log];
    fn label(self) -> &'static str {
        match self {
            DTab::Overview => "Summary",
            DTab::Files => "Files",
            DTab::Diff => "Diff",
            DTab::Log => "Log",
        }
    }
}

/// What the view asks the IDE to do (the IDE owns dialogs and the editor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TasksAct {
    None,
    Used,
    New,
    /// Merge this task: the IDE shows the dry run and asks.
    Merge(String),
    Discard(String),
    Followup(String),
    /// Open this file (relative to the workspace root) in the editor.
    Open(String),
    Back,
}

#[derive(Clone, Debug, Default)]
struct FileDiff {
    path: String,
    status: char,
    adds: usize,
    dels: usize,
    lines: Vec<String>,
}

/// What the worker thread read from git for one task.
#[derive(Clone, Debug, Default)]
struct Review {
    id: String,
    key: String,
    files: Vec<FileDiff>,
    check: Option<Result<MergeCheck, String>>,
    error: Option<String>,
}

pub struct TasksView {
    pub mgr: Manager,
    pub sel: usize,
    scroll: usize,
    visible: usize,
    pub tab: DTab,
    /// Narrow window: the detail is a page of its own.
    pub page: bool,
    detail: ScrollLog,
    detail_key: String,
    collapsed: BTreeSet<String>,
    files_sel: usize,
    review: Arc<Mutex<Option<Review>>>,
    review_for: String,
    working: Arc<Mutex<BTreeSet<String>>>,
    pub status: Option<(String, bool)>,
    /// The workspace's test command (program and arguments); empty: none.
    pub test_cmd: Vec<String>,
    root_rel: String,
    last_draw: Option<Instant>,
    /// The last frame showed the list and the detail as pages of their own (a narrow pane).
    narrow: bool,
    /// Who is at the keyboard, for the audit line of a mode switch.
    pub user: String,
}

fn status_color(s: &Task) -> [u8; 4] {
    if s.kept {
        return CYAN;
    }
    match s.status {
        Status::Queued => MUTED,
        Status::Running => CYAN,
        Status::NeedsYou => AMBER,
        Status::Paused | Status::Interrupted => rgb(170, 150, 255),
        Status::Failed => ERR,
        Status::Done => OK,
        Status::Merged => MAGENTA,
        Status::Discarded => DIM,
    }
}

/// What a switch of a running task's mode does (the line under the buttons after the switch).
fn task_mode_effect(m: iyakki_agent::livemode::Mode) -> &'static str {
    match m {
        iyakki_agent::livemode::Mode::Autopilot => "it keeps working through its plan",
        iyakki_agent::livemode::Mode::Auto => "it finishes the current step and pauses (Resume works the plan again); a waiting card it would run is approved now",
        iyakki_agent::livemode::Mode::Ask => "it finishes the current step and pauses (Resume works the plan again); until then risky calls ask",
    }
}

fn status_text(t: &Task) -> String {
    if t.kept {
        "kept".into()
    } else {
        t.status.label().to_string()
    }
}

fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let path = rest.split(" b/").last().unwrap_or(rest).to_string();
            out.push(FileDiff { path, status: 'M', ..Default::default() });
        }
        let Some(f) = out.last_mut() else { continue };
        if l.starts_with("new file mode") {
            f.status = 'A';
        } else if l.starts_with("deleted file mode") {
            f.status = 'D';
        } else if l.starts_with('+') && !l.starts_with("+++") {
            f.adds += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            f.dels += 1;
        }
        f.lines.push(l.to_string());
    }
    out
}

fn diff_color(l: &str) -> [u8; 4] {
    if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff --git") || l.starts_with("index ") || l.starts_with("new file") || l.starts_with("deleted file") {
        rgb(150, 160, 190)
    } else if l.starts_with('+') {
        rgb(110, 225, 150)
    } else if l.starts_with('-') {
        rgb(255, 120, 130)
    } else if l.starts_with("@@") {
        CYAN
    } else {
        [0, 0, 0, 0]
    }
}

fn clock(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}h {m:02}m") } else if m > 0 { format!("{m}m {s:02}s") } else { format!("{s}s") }
}

impl TasksView {
    pub fn new(mgr: Manager, test_cmd: &str) -> TasksView {
        let mut detail = ScrollLog::default();
        detail.pane = textsel::Pane::new(DETAIL_AREA);
        TasksView { mgr, sel: 0, scroll: 0, visible: 6, tab: DTab::Overview, page: false, detail, detail_key: String::new(), collapsed: BTreeSet::new(), files_sel: 0, review: Arc::new(Mutex::new(None)), review_for: String::new(), working: Arc::new(Mutex::new(BTreeSet::new())), status: None, test_cmd: test_cmd.split_whitespace().map(str::to_string).collect(), root_rel: String::new(), last_draw: None, narrow: false, user: "the person".into() }
    }

    /// The tasks in the order shown: unfinished first (waiting for you, running, queued, paused, done), then the rest.
    pub fn tasks(&self) -> Vec<Task> {
        let mut v = self.mgr.list();
        let rank = |t: &Task| match t.status {
            Status::NeedsYou => 0,
            Status::Running => 1,
            Status::Queued => 2,
            Status::Paused | Status::Interrupted => 3,
            Status::Failed => 4,
            Status::Done => 5,
            Status::Merged => 6,
            Status::Discarded => 7,
        };
        v.sort_by_key(|t| (rank(t), std::cmp::Reverse(t.created)));
        v
    }

    pub fn selected(&self) -> Option<Task> {
        self.tasks().get(self.sel).cloned()
    }

    pub fn select_id(&mut self, id: &str) {
        if let Some(i) = self.tasks().iter().position(|t| t.id == id) {
            self.sel = i;
        }
    }

    pub fn set_root(&mut self, rel: &str) {
        self.root_rel = rel.to_string();
    }

    /// The text of the dry run the last review made for this task (None while it is being read).
    pub fn check_of(&self, id: &str) -> Option<Result<MergeCheck, String>> {
        self.review.lock().ok().and_then(|g| g.as_ref().filter(|r| r.id == id).and_then(|r| r.check.clone()))
    }

    /// Read the diff and the dry run of the selected task on a worker thread when it changed.
    pub fn tick(&mut self) {
        self.detail.on_tick();
        let Some(t) = self.selected() else { return };
        if t.status.active() || t.status.finished() || !t.tree.exists() {
            return;
        }
        let key = format!("{}:{}:{}:{}", t.id, t.status.label(), t.base, t.ended);
        let have = self.review.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|r| r.key == key));
        let busy = self.working.lock().ok().is_some_and(|w| w.contains(&key));
        if have || busy {
            return;
        }
        if let Ok(mut w) = self.working.lock() {
            w.insert(key.clone());
        }
        let (mgr, slot, working, id) = (self.mgr.clone(), self.review.clone(), self.working.clone(), t.id.clone());
        self.review_for = key.clone();
        std::thread::spawn(move || {
            let mut r = Review { id: id.clone(), key: key.clone(), ..Default::default() };
            match mgr.diff(&id) {
                Ok(d) => r.files = parse_diff(&d),
                Err(e) => r.error = Some(e),
            }
            r.check = Some(mgr.merge_check(&id));
            if let Ok(mut g) = slot.lock() {
                *g = Some(r);
            }
            if let Ok(mut w) = working.lock() {
                w.remove(&key);
            }
        });
    }

    fn review_now(&self, id: &str) -> Option<Review> {
        self.review.lock().ok().and_then(|g| g.clone()).filter(|r| r.id == id)
    }

    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.status = Some((text.into(), ok));
    }

    /// Forget what was read: the next tick reads it again.
    fn stale(&mut self) {
        if let Ok(mut g) = self.review.lock() {
            *g = None;
        }
    }

    /// The last frame showed the list and the detail as separate pages.
    pub fn narrow(&self) -> bool {
        self.narrow
    }

    /// The diff and the dry run of the selected task have been read.
    pub fn review_ready(&self) -> bool {
        self.selected().and_then(|t| self.review_now(&t.id)).is_some_and(|r| r.check.is_some())
    }

    /// The text of the detail pane as last built.
    pub fn detail_lines(&self) -> &[(String, [u8; 4])] {
        &self.detail.lines
    }

    pub fn first_approval(&self, id: &str) -> Option<iyakki_agent::tasks::Approval> {
        self.mgr.approvals().into_iter().find(|a| a.task == id)
    }

    fn answer(&mut self, a: &iyakki_agent::tasks::Approval, v: Verdict) {
        match self.mgr.answer(&a.task, &a.call_id, v) {
            Ok(()) => self.say(format!("{}: {}", a.name, if v == Verdict::Deny { "denied" } else { "allowed" }), true),
            Err(e) => self.say(e, false),
        }
    }

    /// Run the action of a button or key on the selected task.
    fn act(&mut self, c: char) -> TasksAct {
        let Some(t) = self.selected() else { return if c == 'n' { TasksAct::New } else { TasksAct::None } };
        let id = t.id.clone();
        let res = match c {
            'n' => return TasksAct::New,
            's' => self.mgr.stop(&id).map(|_| "stopping".to_string()),
            'p' => self.mgr.pause(&id).map(|_| "pausing: it stops after saving its place".to_string()),
            'r' => self.mgr.retry(&id).map(|_| "queued again".to_string()),
            // the task's approval mode, live: Autopilot -> Ask first -> Auto-approve safe -> Autopilot
            'a' => self.mgr.cycle_mode(&id, &self.user).map(|m| format!("approvals: {} — {}", m.label(), task_mode_effect(m))),
            'm' => return TasksAct::Merge(id),
            'd' => return TasksAct::Discard(id),
            'f' => return TasksAct::Followup(id),
            'k' => self.mgr.keep(&id),
            'b' => {
                let r = self.mgr.rebase(&id);
                self.stale();
                r
            }
            't' => {
                if self.test_cmd.is_empty() {
                    Err("this workspace has no test command (set it in the IDE settings)".to_string())
                } else {
                    let (mgr, cmd, tid) = (self.mgr.clone(), self.test_cmd.clone(), id.clone());
                    std::thread::spawn(move || {
                        let _ = mgr.run_tests(&tid, &cmd[0], &cmd[1..]);
                    });
                    Ok("running the tests in the task's folder…".to_string())
                }
            }
            'y' | 'x' => match self.first_approval(&id) {
                Some(a) => {
                    self.answer(&a, if c == 'y' { Verdict::Approve } else { Verdict::Deny });
                    return TasksAct::Used;
                }
                None => Err("nothing is waiting in this task".to_string()),
            },
            _ => return TasksAct::None,
        };
        match res {
            Ok(m) => self.say(m, true),
            Err(e) => self.say(e, false),
        }
        TasksAct::Used
    }

    pub fn handle(&mut self, ev: &Event) -> TasksAct {
        if self.tab == DTab::Overview || self.tab == DTab::Diff || self.tab == DTab::Log {
            if self.detail.handle(ev) {
                return TasksAct::Used;
            }
        }
        let n = self.tasks().len();
        let key = match ev {
            Event::Key(k) | Event::Sub(SUB, k) => k.clone(),
            Event::Row(r) if (ROW_BASE..ROW_BASE + 1000).contains(r) => {
                self.sel = (r - ROW_BASE).min(n.saturating_sub(1));
                self.page = true;
                self.detail_key.clear();
                return TasksAct::Used;
            }
            Event::Row(r) if (FILE_BASE..FILE_BASE + 5000).contains(r) => {
                self.files_sel = r - FILE_BASE;
                return self.toggle_file();
            }
            Event::Tab(t) if (TAB_BASE..TAB_BASE + 10).contains(t) => {
                self.tab = DTab::ALL[(t - TAB_BASE).min(3)];
                self.detail_key.clear();
                return TasksAct::Used;
            }
            Event::Tab(t) if *t >= APPROVE_BASE && *t < APPROVE_BASE + 100 => {
                let i = (t - APPROVE_BASE) / 2;
                if let Some(a) = self.mgr.approvals().get(i).cloned() {
                    self.answer(&a, if (t - APPROVE_BASE) % 2 == 0 { Verdict::Approve } else { Verdict::Deny });
                }
                return TasksAct::Used;
            }
            Event::Scroll(d) => {
                if self.tab == DTab::Files {
                    self.files_sel = (self.files_sel as i32 - d).max(0) as usize;
                } else {
                    self.detail.scroll_by(*d * 3);
                }
                return TasksAct::Used;
            }
            _ => return TasksAct::None,
        };
        match key {
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::Enter => {
                if self.tab == DTab::Files && !self.page_is_list() {
                    return self.toggle_file();
                }
                self.page = true;
            }
            Key::Tab => {
                let i = DTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.tab = DTab::ALL[(i + 1) % 4];
            }
            Key::BackTab => {
                let i = DTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.tab = DTab::ALL[(i + 3) % 4];
            }
            Key::Char('j') if self.tab == DTab::Files => self.files_sel += 1,
            Key::Char('K') if self.tab == DTab::Files => self.files_sel = self.files_sel.saturating_sub(1),
            Key::Char('e') => {
                if let Some(r) = self.selected().and_then(|t| self.review_now(&t.id)) {
                    self.collapsed.clear();
                    let _ = r;
                }
            }
            Key::Char('c') => {
                if let Some(r) = self.selected().and_then(|t| self.review_now(&t.id)) {
                    self.collapsed = r.files.iter().map(|f| f.path.clone()).collect();
                }
            }
            Key::Char('o') => return self.open_file(),
            Key::Char('+') | Key::Char('=') => {
                let c = self.mgr.concurrency();
                self.mgr.set_concurrency(c + 1);
                self.say(format!("up to {} tasks run at once (at most {})", self.mgr.concurrency(), iyakki_agent::tasks::HARD_CAP), true);
            }
            Key::Char('-') => {
                let c = self.mgr.concurrency();
                self.mgr.set_concurrency(c.saturating_sub(1));
                self.say(format!("up to {} tasks run at once", self.mgr.concurrency()), true);
            }
            Key::Esc => {
                if self.page {
                    self.page = false;
                    return TasksAct::Back;
                }
                return TasksAct::None;
            }
            Key::Char(c) if "nspmdfkbtyxra".contains(c) => return self.act(c),
            _ => return TasksAct::None,
        }
        self.detail_key.clear();
        TasksAct::Used
    }

    fn page_is_list(&self) -> bool {
        false
    }

    fn files_of(&self) -> Vec<FileDiff> {
        self.selected().and_then(|t| self.review_now(&t.id)).map(|r| r.files).unwrap_or_default()
    }

    fn toggle_file(&mut self) -> TasksAct {
        let files = self.files_of();
        if let Some(f) = files.get(self.files_sel) {
            if !self.collapsed.remove(&f.path) {
                self.collapsed.insert(f.path.clone());
            }
            self.detail_key.clear();
        }
        TasksAct::Used
    }

    fn open_file(&mut self) -> TasksAct {
        let (Some(t), files) = (self.selected(), self.files_of()) else { return TasksAct::None };
        match files.get(self.files_sel.min(files.len().saturating_sub(1))) {
            Some(f) if t.tree.exists() && f.status != 'D' => {
                let abs = t.tree.join(&f.path);
                let rel = abs.strip_prefix(std::path::Path::new(&self.root_rel)).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| abs.to_string_lossy().into_owned());
                TasksAct::Open(rel)
            }
            _ => TasksAct::None,
        }
    }

    // ---------------------------------------------------------------------------------------------- drawing

    /// Text of the Summary tab.
    fn overview(&self, t: &Task) -> Vec<(String, [u8; 4])> {
        let now = iyakki_agent::clock::now_unix();
        let mut v: Vec<(String, [u8; 4])> = Vec::new();
        let none = [0u8, 0, 0, 0];
        v.push((format!("Goal: {}", t.goal), none));
        let model = if t.model.is_empty() { "default model".to_string() } else { t.model.clone() };
        v.push((format!("Agent: {} · {model} · session {}", t.agent, t.session), none));
        if !t.rows.is_empty() {
            v.push((format!("Board rows: {}", t.rows.join(", ")), none));
        }
        v.push((format!("Branch: {} · {} · base {}", if t.branch.is_empty() { "-" } else { &t.branch }, if t.mode == iyakki_agent::tasks::Mode::Git { "git worktree" } else { "copy + patch" }, t.base.chars().take(8).collect::<String>()), none));
        v.push((format!("Folder: {}", t.tree.display()), none));
        v.push((format!("Namespace (reserved, deploys are refused): {} · port {}", t.namespace, t.port), none));
        let b = &t.budgets;
        v.push((format!("Budgets: {} steps · {} min · tokens {} · cost {}", b.max_steps, b.max_minutes, if b.max_tokens == 0 { "default".to_string() } else { b.max_tokens.to_string() }, if b.max_cost_cents == 0 { "not capped".to_string() } else { format!("{} cents", b.max_cost_cents) }), none));
        v.push((format!("Used: {} · {} tokens · {:.1} cents · {}", clock(t.elapsed(now)), t.tokens, t.cost_cents, t.progress_text()), none));
        if !t.note.is_empty() {
            v.push((format!("Note: {}", t.note), AMBER));
        }
        if !t.error.is_empty() {
            v.push((format!("Problem: {}", t.error), ERR));
        }
        for a in self.mgr.approvals().iter().filter(|a| a.task == t.id) {
            v.push((format!("Waiting for you: {} {}", a.name, a.summary), AMBER));
            if !a.detail.is_empty() {
                v.push((a.detail.clone(), none));
            }
        }
        if let Some(c) = self.check_of(&t.id) {
            match c {
                Ok(c) => v.push((format!("Merge check: {}", c.message), if c.clean { OK } else { ERR })),
                Err(e) => v.push((format!("Merge check: {e}"), ERR)),
            }
        }
        if !t.tests.is_empty() {
            v.push((format!("Tests in the task's folder: {}", t.tests), if t.tests.starts_with("tests passed") { OK } else { ERR }));
        }
        if !t.summary.is_empty() {
            v.push((String::new(), none));
            v.push(("Summary (what the harness executed):".into(), CYAN));
            for l in t.summary.lines() {
                v.push((l.to_string(), none));
            }
        }
        v
    }

    fn rebuild_detail(&mut self, t: &Task) {
        let key = format!("{}:{}:{:?}:{}:{}:{}:{}:{}:{}", t.id, t.status.label(), self.tab, t.tokens, t.last, t.tests.len(), self.mgr.log(&t.id).len(), self.review_now(&t.id).map_or(0, |r| r.files.len() + r.check.is_some() as usize), self.collapsed.len());
        if key == self.detail_key {
            return;
        }
        self.detail_key = key;
        let lines: Vec<(String, [u8; 4])> = match self.tab {
            DTab::Overview => self.overview(t),
            DTab::Log => {
                let l = self.mgr.log(&t.id);
                if l.is_empty() { vec![("Nothing has happened yet.".into(), DIM)] } else { l.into_iter().map(|x| (x, [0, 0, 0, 0])).collect() }
            }
            DTab::Diff => match self.review_now(&t.id) {
                Some(r) if r.error.is_some() => vec![(r.error.unwrap_or_default(), ERR)],
                Some(r) if r.files.is_empty() => vec![("The task changed no files.".into(), DIM)],
                Some(r) => r
                    .files
                    .iter()
                    .flat_map(|f| {
                        let open = !self.collapsed.contains(&f.path);
                        let mut v = vec![(format!("{} {}  {}  +{} -{}", if open { "▾" } else { "▸" }, f.path, f.status, f.adds, f.dels), TEXT)];
                        if open {
                            v.extend(f.lines.iter().map(|l| (l.clone(), diff_color(l))));
                        }
                        v
                    })
                    .collect(),
                None if t.status.active() => vec![("The diff is read when the task stops (a running task is still writing its files).".into(), DIM)],
                None => vec![("Reading the diff…".into(), DIM)],
            },
            DTab::Files => Vec::new(),
        };
        self.detail.lines = lines;
        self.detail.scroll = 0;
    }

    fn buttons(t: &Task) -> Vec<(&'static str, char, [u8; 4], &'static str)> {
        match t.status {
            Status::Queued => vec![("Stop", 's', ERR, "Stop before it starts (s)")],
            Status::Running => vec![("Pause", 'p', rgb(170, 150, 255), "Save its place and stop (p)"), ("Stop", 's', ERR, "Stop the task (s)")],
            Status::NeedsYou => vec![("Allow", 'y', OK, "Allow what it asks (y)"), ("Deny", 'x', ERR, "Deny it (x)"), ("Stop", 's', ERR, "Stop the task (s)")],
            Status::Paused | Status::Interrupted => vec![("Resume", 'r', OK, "Continue from the saved place (r)"), ("Discard", 'd', ERR, "Remove the task and its branch (d)")],
            Status::Failed => vec![("Retry", 'r', OK, "Continue with fresh budgets (r)"), ("Follow-up", 'f', CYAN, "Tell it what to do next (f)"), ("Discard", 'd', ERR, "Remove the task and its branch (d)")],
            Status::Done => vec![("Merge", 'm', OK, "Apply it to the workspace (m)"), ("Tests", 't', AMBER, "Run the tests in the task's folder (t)"), ("Follow-up", 'f', CYAN, "Tell it what to do next (f)"), ("Rebase", 'b', CYAN, "Start from the latest commit (b)"), ("Keep", 'k', MUTED, "Keep the branch, remove the folder (k)"), ("Discard", 'd', ERR, "Remove the task and its branch (d)")],
            Status::Merged | Status::Discarded => vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], _tick: u64) {
        self.last_draw = Some(Instant::now());
        let tasks = self.tasks();
        self.sel = self.sel.min(tasks.len().saturating_sub(1));
        let narrow = r.w < 560.0 * s;
        self.narrow = narrow;
        let list_w = if narrow { r.w } else { (r.w * 0.38).clamp(230.0 * s, 430.0 * s) };
        let show_list = !narrow || !self.page;
        let show_detail = !narrow || self.page;
        if show_list {
            let lr = R::new(r.x, r.y, if show_detail { list_w - 6.0 * s } else { r.w }, r.h);
            self.draw_list(g, lr, s, focused, accent, &tasks);
        }
        if show_detail {
            let dr = if narrow { r } else { R::new(r.x + list_w, r.y, r.w - list_w, r.h) };
            self.draw_detail(g, dr, s, focused, accent, tasks.get(self.sel).cloned(), narrow);
        }
    }

    fn draw_list(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], tasks: &[Task]) {
        let m = chrome::cur();
        let pad = 8.0 * s;
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        // the top row: counts and the buttons
        let (run, need, queued) = self.mgr.counts();
        let cs = s * 0.85;
        let mut x = r.x + pad;
        let y = r.y + 3.0 * s;
        x += fx::chip(g, x, y + 1.0 * s, cs, &format!("{run} running"), CYAN, run > 0) + 5.0 * s;
        x += fx::chip(g, x, y + 1.0 * s, cs, &format!("{need} need you"), AMBER, need > 0) + 5.0 * s;
        fx::chip(g, x, y + 1.0 * s, cs, &format!("{queued} queued"), MUTED, false);
        let rowh = chrome::tool(s);
        let by = y + rowh + 2.0 * s;
        let mut bx = r.x + pad;
        bx += chrome::text_btn(g, bx, by, s, "+ New task", OK, true, Event::Sub(SUB, Key::Char('n')), "New task", Some("Describe a piece of work to run in its own copy (n)"), bounds) + 4.0 * s;
        bx += chrome::text_btn(g, bx, by, s, "−", MUTED, true, Event::Sub(SUB, Key::Char('-')), "fewer at once", Some("Run fewer tasks at once (-)"), bounds);
        let max = format!("max {}", self.mgr.concurrency());
        let w = g.text_width(m.font_small * s, &max, Face::Bold);
        g.text(bx + 4.0 * s, by + (rowh - g.line_height(m.font_small * s, Face::Bold)) / 2.0, m.font_small * s, chrome::muted(), &max, Face::Bold);
        bx += w + 12.0 * s;
        chrome::text_btn(g, bx, by, s, "+", MUTED, true, Event::Sub(SUB, Key::Char('+')), "more at once", Some("Run more tasks at once, up to 6 (+)"), bounds);
        let mut top = by + rowh + 6.0 * s;
        // the one queue of approvals, tagged with the task
        let appr = self.mgr.approvals();
        let tag = |id: &str| tasks.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_else(|| id.to_string());
        for (i, a) in appr.iter().enumerate().take(3) {
            let h = 46.0 * s;
            let rect = R::new(r.x + pad, top, r.w - 2.0 * pad, h);
            g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, fx::over(fx::WELL, with_alpha(AMBER, 26)));
            g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 6.0 * s, 1.0 * s, with_alpha(AMBER, 120));
            let line = format!("[{}] {} {}", tag(&a.task), a.name, a.summary);
            let t = g.fit(m.font_small * s, &line, Face::Bold, rect.w - 150.0 * s);
            g.text(rect.x + 8.0 * s, rect.y + 5.0 * s, m.font_small * s, fx::readable(AMBER, fx::WELL, 4.5), &t, Face::Bold);
            let bw = chrome::text_btn(g, rect.x + 6.0 * s, rect.y + h - chrome::tool(s) - 2.0 * s, s, "Allow", OK, true, Event::Tab(APPROVE_BASE + 2 * i), "allow", Some("Allow this call of this task only"), bounds);
            chrome::text_btn(g, rect.x + 10.0 * s + bw, rect.y + h - chrome::tool(s) - 2.0 * s, s, "Deny", ERR, true, Event::Tab(APPROVE_BASE + 2 * i + 1), "deny", Some("Deny it"), bounds);
            top += h + 4.0 * s;
        }
        if appr.len() > 3 {
            g.text(r.x + pad, top, m.font_small * s, DIM, &format!("+{} more waiting", appr.len() - 3), Face::Sans);
            top += 16.0 * s;
        }
        if let Some((msg, ok)) = &self.status {
            let t = g.fit(m.font_small * s, msg, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, top, m.font_small * s, if *ok { fx::readable(OK, fx::PANEL, 4.5) } else { fx::readable(ERR, fx::PANEL, 4.5) }, &t, Face::Sans);
            top += 16.0 * s;
        }
        let list = R::new(r.x, top, r.w, (r.y + r.h - top).max(0.0));
        if tasks.is_empty() {
            let lines = ["No tasks yet.", "Tasks run in parallel, each in its own copy of the workspace, and you review and merge what they make.", "Press n for a new task, or right-click a row on the Board (Run as task), or ask the assistant to do things in parallel."];
            let mut y = list.y + 8.0 * s;
            for (i, l) in lines.iter().enumerate() {
                for w in g.wrap(m.font_ui * s, l, Face::Sans, list.w - 2.0 * pad) {
                    g.text(list.x + pad, y, m.font_ui * s, if i == 0 { TEXT } else { MUTED }, &w, if i == 0 { Face::Bold } else { Face::Sans });
                    y += g.line_height(m.font_ui * s, Face::Sans) + 2.0 * s;
                }
                y += 6.0 * s;
            }
            return;
        }
        let ch = (m.row_h * s * 2.9).max(54.0 * s);
        self.visible = ((list.h / ch) as usize).max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible {
            self.scroll = self.sel + 1 - self.visible;
        }
        let now = iyakki_agent::clock::now_unix();
        for (i, t) in tasks.iter().enumerate().skip(self.scroll).take(self.visible) {
            let y = list.y + (i - self.scroll) as f32 * ch;
            let rect = R::new(list.x + pad, y, list.w - 2.0 * pad - 6.0 * s, ch - 4.0 * s);
            let selected = i == self.sel;
            let hot = g.hovered(rect);
            let col = status_color(t);
            g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, if selected { fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })) } else if hot { rgb(20, 27, 50) } else { fx::WELL });
            g.rect(rect.x, rect.y + 3.0 * s, 3.0 * s, rect.h - 6.0 * s, col);
            let status = status_text(t);
            let cw = g.text_width(m.font_small * s, &status, Face::Bold) + 14.0 * s;
            let title = g.fit(m.font_ui * s, &t.title, Face::Bold, rect.w - cw - 24.0 * s);
            g.text(rect.x + 10.0 * s, rect.y + 4.0 * s, m.font_ui * s, TEXT, &title, Face::Bold);
            fx::chip(g, rect.x + rect.w - cw - 6.0 * s, rect.y + 3.0 * s, cs, &status, col, matches!(t.status, Status::Running | Status::NeedsYou));
            let model = if t.model.is_empty() { String::new() } else { format!(" · {}", t.model) };
            let l2 = format!("{}{model} · {} · {}{}", t.agent, t.progress_text(), clock(t.elapsed(now)), if t.tokens > 0 { format!(" · {}k tok", t.tokens / 1000) } else { String::new() });
            let l2 = g.fit(m.font_small * s, &l2, Face::Sans, rect.w - 20.0 * s);
            g.text(rect.x + 10.0 * s, rect.y + 4.0 * s + g.line_height(m.font_ui * s, Face::Bold) + 1.0 * s, m.font_small * s, chrome::muted(), &l2, Face::Sans);
            let l3 = if t.branch.is_empty() { t.last.clone() } else { format!("{} · {}", t.branch.rsplit('/').next().unwrap_or(&t.branch), t.last) };
            let l3 = g.fit(m.font_small * s, &l3, Face::Sans, rect.w - 20.0 * s);
            g.text(rect.x + 10.0 * s, rect.y + rect.h - g.line_height(m.font_small * s, Face::Sans) - 4.0 * s, m.font_small * s, DIM, &l3, Face::Sans);
            g.hit(rect, Event::Row(ROW_BASE + i), &format!("task {}", t.id));
        }
        fx::scrollbar(g, R::new(r.x + r.w - 6.0 * s, list.y, 6.0 * s, list.h), s, self.scroll, self.visible, tasks.len(), accent);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_detail(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], t: Option<Task>, narrow: bool) {
        let m = chrome::cur();
        fx::panel(g, r, s, accent);
        let Some(t) = t else {
            g.text(r.x + 14.0 * s, r.y + 14.0 * s, m.font_ui * s, MUTED, "Select a task.", Face::Sans);
            return;
        };
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        let pad = 10.0 * s;
        let rowh = chrome::tool(s);
        let mut y = r.y + 6.0 * s;
        if narrow {
            let w = chrome::text_btn(g, r.x + 4.0 * s, y, s, "‹ Tasks", CYAN, true, Event::Sub(SUB, Key::Esc), "back to the list", Some("Back to the list (Esc)"), bounds);
            let _ = w;
            y += rowh + 2.0 * s;
        }
        let col = status_color(&t);
        let status = status_text(&t);
        let cw = g.text_width(m.font_small * s, &status, Face::Bold) + 14.0 * s;
        let title = g.fit(m.font_ui * s * 1.15, &t.title, Face::Bold, r.w - cw - 3.0 * pad);
        g.text(r.x + pad, y, m.font_ui * s * 1.15, TEXT, &title, Face::Bold);
        fx::chip(g, r.x + r.w - cw - pad, y + 1.0 * s, s * 0.85, &status, col, matches!(t.status, Status::Running | Status::NeedsYou));
        y += g.line_height(m.font_ui * s * 1.15, Face::Bold) + 4.0 * s;
        // the buttons of this status
        let mut bx = r.x + pad - 8.0 * s;
        for (label, c, color, tip) in Self::buttons(&t) {
            if bx + 90.0 * s > r.x + r.w && bx > r.x + 40.0 * s {
                bx = r.x + pad - 8.0 * s;
                y += rowh + 2.0 * s;
            }
            bx += chrome::text_btn(g, bx, y, s, label, color, true, Event::Sub(SUB, Key::Char(c)), label, Some(tip), bounds) + 2.0 * s;
        }
        // the task's approval mode, live while it runs (a, or a click, cycles it)
        if let Some(mode) = self.mgr.mode(&t.id) {
            let label = mode.label();
            if bx + 90.0 * s > r.x + r.w && bx > r.x + 40.0 * s {
                bx = r.x + pad - 8.0 * s;
                y += rowh + 2.0 * s;
            }
            let color = match mode {
                iyakki_agent::livemode::Mode::Autopilot => OK,
                iyakki_agent::livemode::Mode::Auto => AMBER,
                iyakki_agent::livemode::Mode::Ask => MUTED,
            };
            let tip = format!("Approvals of this task: {label} (a cycles). {}", iyakki_agent::livemode::DURING_RUN);
            chrome::text_btn(g, bx, y, s, label, color, true, Event::Sub(SUB, Key::Char('a')), label, Some(&tip), bounds);
        }
        y += rowh + 4.0 * s;
        // the tabs
        let mut tx = r.x + pad - 6.0;
        let nfiles = self.files_of().len();
        for (i, tab) in DTab::ALL.iter().enumerate() {
            let label = match tab {
                DTab::Files if nfiles > 0 => format!("Files {nfiles}"),
                o => o.label().to_string(),
            };
            tx += chrome::flat_tab(g, tx, y, s, &label, self.tab == *tab, accent, Event::Tab(TAB_BASE + i), &label);
        }
        y += m.tab_h * s + 2.0 * s;
        chrome::focus_line(g, r.x + 1.0, y, r.w - 2.0, s, focused, accent);
        y += 6.0 * s;
        let body = R::new(r.x + pad, y, r.w - 2.0 * pad, (r.y + r.h - y - 6.0 * s).max(0.0));
        match self.tab {
            DTab::Files => self.draw_files(g, body, s, accent, &t),
            tab => {
                self.rebuild_detail(&t);
                let (size, face) = if tab == DTab::Diff { (m.font_small * s + 0.5 * s, Face::Mono) } else { (m.font_small * s + 1.0 * s, Face::Sans) };
                self.detail.render(g, body, size, face);
            }
        }
    }

    fn draw_files(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4], t: &Task) {
        let m = chrome::cur();
        let files = self.files_of();
        if files.is_empty() {
            let msg = if t.status.active() { "The files are listed when the task stops." } else if self.review_now(&t.id).is_some() { "The task changed no files." } else { "Reading the changed files…" };
            g.text(r.x + 4.0 * s, r.y + 4.0 * s, m.font_ui * s, DIM, msg, Face::Sans);
            return;
        }
        self.files_sel = self.files_sel.min(files.len() - 1);
        let rh = m.row_h * s;
        let vis = ((r.h / rh) as usize).max(1);
        let first = self.files_sel.saturating_sub(vis - 1);
        for (i, f) in files.iter().enumerate().skip(first).take(vis) {
            let rect = R::new(r.x, r.y + (i - first) as f32 * rh, r.w - 8.0 * s, rh);
            let hot = g.hovered(rect);
            if i == self.files_sel || hot {
                fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 4.0 * s, i == self.files_sel, hot, accent);
            }
            let open = !self.collapsed.contains(&f.path);
            let col = match f.status {
                'A' => OK,
                'D' => ERR,
                _ => CYAN,
            };
            let ty = rect.y + (rect.h - g.line_height(m.font_ui * s, Face::Sans)) / 2.0;
            g.text(rect.x + 4.0 * s, ty, m.font_ui * s, MUTED, if open { "▾" } else { "▸" }, Face::Bold);
            g.text(rect.x + 20.0 * s, ty, m.font_ui * s, col, &f.status.to_string(), Face::Bold);
            let counts = format!("+{} -{}", f.adds, f.dels);
            let cwid = g.text_width(m.font_small * s, &counts, Face::Mono);
            let name = g.fit(m.font_ui * s, &f.path, Face::Sans, rect.w - cwid - 56.0 * s);
            g.text(rect.x + 36.0 * s, ty, m.font_ui * s, TEXT, &name, Face::Sans);
            g.text(rect.x + rect.w - cwid - 6.0 * s, ty, m.font_small * s, chrome::muted(), &counts, Face::Mono);
            g.hit(rect, Event::Row(FILE_BASE + i), &f.path);
        }
        let hint = "Click a file to fold it in the Diff · o opens it in the editor · c folds all · e unfolds all";
        g.text(r.x + 4.0 * s, r.y + r.h - 14.0 * s, m.font_small * s, DIM, hint, Face::Sans);
    }

    /// Chip text for the IDE's top bar: "2 running · 1 needs you"; `None` when no task is active or waiting.
    pub fn chip(&self) -> Option<(String, bool)> {
        let (run, need, queued) = self.mgr.counts();
        if run + need + queued == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if run > 0 {
            parts.push(format!("{run} running"));
        }
        if need > 0 {
            parts.push(format!("{need} need you"));
        }
        if queued > 0 && run + need == 0 {
            parts.push(format!("{queued} queued"));
        }
        Some((parts.join(" · "), need > 0))
    }

    /// Copy, Select all and Copy all for the right-click menu over the detail text.
    pub fn context_items(&self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.tab == DTab::Files {
            return Vec::new();
        }
        let mut v = self.detail.pane.context_items(x, y);
        if !v.is_empty() {
            v.push(crate::ctxmenu::Item::new("tasks.copyall", "Copy all", "").enabled(self.detail.pane.has_text()));
        }
        v
    }

    pub fn context_pick(&mut self, id: &str) -> bool {
        match id {
            "copy" => {
                if let Some(t) = self.detail.pane.selected_text() {
                    crate::ui::clipboard::copy(&t);
                }
                true
            }
            "selall" => {
                self.detail.pane.select_all();
                true
            }
            "tasks.copyall" => {
                if let Some(t) = self.detail.pane.all_text() {
                    crate::ui::clipboard::copy(&t);
                }
                true
            }
            _ => false,
        }
    }
}

impl Drop for TasksView {
    /// A workspace that closes stops its running tasks and saves their place (they show `interrupted` next time).
    fn drop(&mut self) {
        self.mgr.shutdown();
    }
}
