//! GIT: a tile to browse the repositories of the Git hosts you connected (docs/GIT-TILE.md). The connection selector at the top is
//! the list of Settings -> Git (every host with its kind, address, test dot and where the data lives) plus "Local clones", the
//! repositories already on this computer; below it the repositories of the chosen host as a table, and Enter opens one: its files,
//! commits and diffs, branches and tags, issues and pull requests (read-only) and its details.
//!
//! The engine is `iyakki_agent::gitbrowse`: every remote read is a GET to the host's own API through `GitHttp` (token from the vault,
//! scrubbed errors, the account's network policy before each request, the certificate the person trusted for the host); a clone on this
//! computer is read with the git program, read-only. Clone, Fetch, Pull and Push are the jobs of the AI IDE (`gitjobs`), each after its
//! review. This file is the state and the keys of the list; `git_tile_repo.rs` has the repository view, `git_tile_ui.rs` and
//! `git_tile_repo_ui.rs` draw them.
//!
//! Nothing here asks a host while the tile is not on screen, nothing blocks the screen (every request runs on a short thread whose answer
//! is picked up on the heartbeat), and going back drops what was still on its way.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent::gitbrowse::{self as gb, BrowseErr, Browser, Egress, LocalClone, LocalRepo, Page, RepoItem};
use iyakki_agent::gitconn::{self, Conns, Connection, GitHttp, Placement};
use iyakki_agent::gitjobs::{self, Batch, CloneSpec, ItemState, Op, Phase, RepoStatus, Tools};
use iyakki_agent::gitkeys::{GitKeys, Who};
use iyakki_agent::k8sretry::{background_delay, SystemTimer, Timer, Tracker};
use iyakki_agent::wsexec::Runner;

use super::git_tile_repo::RepoView;
use super::k8s_tile::{Job, ListState};
use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::tooltip::Tooltip;
use crate::ui::{clipboard, Event, Key, Nav, TextField, R};

pub const ACCENT: [u8; 4] = [244, 124, 72, 255];

// click codes (Event::Tab)
pub(super) const T_SOURCE: usize = 1;
pub(super) const T_REFRESH: usize = 2;
pub(super) const T_LOG: usize = 3;
pub(super) const T_LOG_COPY: usize = 4;
pub(super) const T_ADD: usize = 5;
pub(super) const T_MORE: usize = 6;
pub(super) const T_CLEAR: usize = 7;
pub(super) const T_BACK: usize = 8;
pub(super) const T_CLONE: usize = 9;
pub(super) const T_IDE: usize = 10;
pub(super) const T_FETCH: usize = 11;
pub(super) const T_PULL: usize = 12;
pub(super) const T_PUSH: usize = 13;
pub(super) const T_COPY_URL: usize = 14;
pub(super) const T_BROWSER: usize = 15;
pub(super) const T_YES: usize = 16;
pub(super) const T_NO: usize = 17;
pub(super) const T_REF: usize = 18;
pub(super) const T_CLOSE: usize = 19;
pub(super) const T_TAB: usize = 20; // 20..25
pub(super) const T_SUB: usize = 30; // 30, 31: issues, pull requests
pub(super) const T_COPY_HTTPS: usize = 32;
pub(super) const T_COPY_SSH: usize = 33;
pub(super) const T_COPY_TEXT: usize = 34;
pub(super) const T_EDIT_CONN: usize = 35;
pub(super) const T_VIEW_RAW: usize = 36;
pub(super) const T_SORT: usize = 100; // 100..110
pub(super) const T_SRC_ROW: usize = 1_000;
pub(super) const T_ROW: usize = 100_000;
pub(super) const T_FROW: usize = 200_000;
pub(super) const T_CROW: usize = 300_000;
pub(super) const T_DROW: usize = 400_000;
pub(super) const T_BROW: usize = 500_000;
pub(super) const T_IROW: usize = 600_000;
pub(super) const T_REFROW: usize = 700_000;

pub(super) const F_FILTER: usize = 64_001;

pub(super) const WHEEL_ROWS: i32 = 3;
/// Repositories are not read again on choosing a host that was read this recently.
const FRESH_S: u64 = 60;
const LOG_KEEP: usize = 50;
pub(super) const EMBEDDED_NO_WRITE: &str = "The Git tab only reads. Clone, fetch and pull are in the Repositories panel of the AI IDE (they ask first).";

// ------------------------------------------------------------------------------------------------ the rig

/// What the tile uses that tests replace: the host's API, the git program, the clock and the places files go.
#[derive(Clone)]
pub struct Rig {
    pub http: Arc<dyn GitHttp>,
    pub run: Arc<dyn Runner>,
    pub timer: Arc<dyn Timer>,
    /// A private folder for pictures read out of a clone.
    pub scratch: PathBuf,
    /// The Repositories folder clones go into (none: from the account).
    pub repos_root: Option<PathBuf>,
    /// Tests: the programs the jobs use instead of the ones found on the system.
    pub tools: Option<Tools>,
    /// Tests: what answers "which host key does this server present".
    pub probe: Option<gitjobs::Probe>,
    /// The jobs of a started clone, fetch, pull or push are run (tests that only look at what was planned leave this off).
    pub run_jobs: bool,
}

impl Rig {
    pub fn system() -> Rig {
        let run: Arc<dyn Runner> = Arc::new(iyakki_agent::wsexec::System);
        Rig { http: Arc::new(iyakki_agent::gitnet::NetGit::default()), run, timer: Arc::new(SystemTimer::new()), scratch: std::env::temp_dir(), repos_root: None, tools: None, probe: None, run_jobs: true }
    }

    /// Nothing at all answers: no host, no git program. What the shell's own tests get.
    pub fn offline() -> Rig {
        Rig {
            http: Arc::new(gitconn::FakeGitHttp::default()),
            run: Arc::new(iyakki_agent::wsexec::FakeRunner::new(&[])),
            timer: iyakki_agent::k8sretry::FakeTimer::new(),
            scratch: std::env::temp_dir(),
            repos_root: None,
            tools: None,
            probe: None,
            run_jobs: false,
        }
    }
}

// ------------------------------------------------------------------------------------------------ one request

pub(super) enum Load<T> {
    None,
    Loading,
    Ready(T),
    Failed(BrowseErr),
}

/// One thing asked of a host or of git, with its answer, its last failure and when to try again. The last good answer stays when a
/// refresh fails (`stale`).
pub(super) struct Slot<T> {
    pub st: Load<T>,
    job: Option<Job<Result<T, BrowseErr>>>,
    /// The data shown is the last good one: the refresh failed with this.
    pub stale: Option<BrowseErr>,
    /// The timer's milliseconds of the next automatic try (transient failures and rate limits only).
    pub retry_at: Option<u64>,
    pub fails: u32,
}

/// Automatic tries of one thing before it waits for a click.
const MAX_AUTO: u32 = 6;

impl<T: Send + 'static> Slot<T> {
    pub fn new() -> Slot<T> {
        Slot { st: Load::None, job: None, stale: None, retry_at: None, fails: 0 }
    }
    pub fn ready(&self) -> Option<&T> {
        if let Load::Ready(v) = &self.st { Some(v) } else { None }
    }
    pub fn untouched(&self) -> bool {
        matches!(self.st, Load::None) && self.job.is_none()
    }
    pub fn due(&self, now_ms: u64) -> bool {
        self.job.is_none() && self.retry_at.is_some_and(|t| t <= now_ms) && self.fails < MAX_AUTO
    }
    /// Starts the request on its own thread.
    pub fn start(&mut self, f: impl FnOnce() -> Result<T, BrowseErr> + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        if !matches!(self.st, Load::Ready(_)) {
            self.st = Load::Loading;
        }
        self.retry_at = None;
        self.job = Some(Job::spawn(f));
    }
    /// Waits for `at_ms` before the next try (the host asked us to slow down).
    pub fn wait_until(&mut self, at_ms: u64) {
        if !matches!(self.st, Load::Ready(_)) {
            self.st = Load::Loading;
        }
        self.retry_at = Some(at_ms);
    }
    /// Forget the answer and any wait: the next look asks again.
    pub fn reset(&mut self) {
        *self = Slot::new();
    }
    /// The finished request, if there is one: Ok or the failure (also kept for the view).
    pub fn poll(&mut self, now_ms: u64) -> Option<Result<(), BrowseErr>> {
        let r = self.job.as_ref()?.take()?;
        self.job = None;
        Some(match r {
            Ok(v) => {
                self.st = Load::Ready(v);
                self.stale = None;
                self.fails = 0;
                self.retry_at = None;
                Ok(())
            }
            Err(e) => {
                self.fails += 1;
                self.retry_at = match &e {
                    BrowseErr::Transient(_) => Some(now_ms + background_delay(self.fails).as_millis() as u64),
                    BrowseErr::Rate(n) => Some(now_ms + n * 1000),
                    _ => None,
                };
                if matches!(self.st, Load::Ready(_)) {
                    self.stale = Some(e.clone());
                } else {
                    self.st = Load::Failed(e.clone());
                }
                Err(e)
            }
        })
    }
    /// What went wrong, in words, for a line under the data.
    pub fn problem(&self) -> Option<String> {
        match (&self.st, &self.stale) {
            (Load::Failed(e), _) | (_, Some(e)) => Some(e.text()),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------------------------------------ sources and rows

/// What the selector offers.
#[derive(Clone, Debug)]
pub(super) enum Src {
    Conn(Connection),
    Local,
}

impl Src {
    pub fn key(&self) -> String {
        match self {
            Src::Conn(c) => c.id.clone(),
            Src::Local => "local".into(),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Src::Conn(c) => c.name.clone(),
            Src::Local => "Local clones".into(),
        }
    }
}

/// Where a host's data lives, in the words the tile uses: this computer, the network, the cluster, the internet.
pub fn place_text(c: &Connection) -> &'static str {
    let h = c.host();
    if h == "localhost" || h == "::1" || h.starts_with("127.") {
        return "this computer";
    }
    match c.placement() {
        Placement::Cluster => "on the cluster",
        Placement::Lan => "on the network",
        Placement::Internet => "on the internet",
    }
}

/// One line of the list: a repository of a host, or a clone on this computer.
#[derive(Clone, Debug, Default)]
pub(super) struct Row {
    pub name: String,
    pub desc: String,
    pub branch: String,
    pub private: Option<bool>,
    pub updated: u64,
    pub stars: Option<u64>,
    pub size_kb: Option<u64>,
    /// Index into the clones on this computer.
    pub clone: Option<usize>,
    pub item: Option<RepoItem>,
}

pub(super) struct Col {
    pub title: &'static str,
    pub weight: f32,
    /// Higher goes first when there is no room.
    pub prio: u8,
    pub min: f32,
}

pub(super) const COLS: [Col; 7] = [
    Col { title: "Name", weight: 3.0, prio: 0, min: 150.0 },
    Col { title: "Description", weight: 4.0, prio: 2, min: 140.0 },
    Col { title: "Branch", weight: 1.2, prio: 4, min: 74.0 },
    Col { title: "Visibility", weight: 1.0, prio: 3, min: 76.0 },
    Col { title: "Updated", weight: 1.0, prio: 1, min: 66.0 },
    Col { title: "Stars", weight: 0.8, prio: 5, min: 58.0 },
    Col { title: "Size", weight: 0.8, prio: 6, min: 66.0 },
];

pub(super) fn cell(r: &Row, col: usize, now: u64) -> String {
    match col {
        0 => r.name.clone(),
        1 => r.desc.clone(),
        2 => r.branch.clone(),
        3 => match r.private {
            Some(true) => "private".into(),
            Some(false) => "public".into(),
            None => String::new(),
        },
        4 => if r.updated == 0 { String::new() } else { iyakki_agent::k8s::age_text(now.saturating_sub(r.updated)) },
        5 => r.stars.map(|n| n.to_string()).unwrap_or_default(),
        _ => r.size_kb.map(size_text).unwrap_or_default(),
    }
}

pub fn size_text(kb: u64) -> String {
    if kb < 1024 { format!("{kb} KB") } else if kb < 1024 * 1024 { format!("{:.1} MB", kb as f64 / 1024.0) } else { format!("{:.1} GB", kb as f64 / 1048576.0) }
}

pub fn bytes_text(n: u64) -> String {
    if n < 1024 { format!("{n} B") } else if n < 1024 * 1024 { format!("{:.1} KB", n as f64 / 1024.0) } else { format!("{:.1} MB", n as f64 / 1048576.0) }
}

fn compare(a: &Row, b: &Row, col: usize) -> std::cmp::Ordering {
    match col {
        0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        1 => a.desc.to_lowercase().cmp(&b.desc.to_lowercase()),
        2 => a.branch.to_lowercase().cmp(&b.branch.to_lowercase()),
        3 => a.private.cmp(&b.private),
        4 => a.updated.cmp(&b.updated),
        5 => a.stars.cmp(&b.stars),
        _ => a.size_kb.cmp(&b.size_kb),
    }
}

/// The repositories of one host as the tile holds them: the pages read so far and what happened.
pub(super) struct Board {
    pub items: Vec<RepoItem>,
    pub page: usize,
    pub more: bool,
    pub loaded: bool,
    /// Read the first page again (Refresh, or a list that is old): the list shown stays until the answer comes.
    pub reload: bool,
    pub err: Option<BrowseErr>,
    pub tracker: Tracker,
    pub job: Option<Job<(usize, Result<Page<RepoItem>, BrowseErr>)>>,
    /// The page the running request asks for.
    pub asking: usize,
    pub retry_at: Option<u64>,
    pub loaded_s: u64,
    pub version: u64,
}

impl Board {
    fn new() -> Board {
        Board { items: vec![], page: 0, more: false, loaded: false, reload: false, err: None, tracker: Tracker::default(), job: None, asking: 0, retry_at: None, loaded_s: 0, version: 0 }
    }
}

/// The clones on this computer.
#[derive(Default)]
pub(super) struct Locals {
    pub list: Vec<LocalClone>,
    pub loaded: bool,
    pub job: Option<Job<Vec<LocalClone>>>,
    pub version: u64,
    pub want: bool,
}

// ------------------------------------------------------------------------------------------------ cards

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Normal,
    Dim,
    Warn,
    Bold,
}

/// A change waiting for the person's yes.
#[derive(Clone, Debug)]
pub(super) enum Act {
    Clone { conn: Connection, full: String, url: String, then_ide: bool },
    Pull { clone: LocalClone },
    Push { clone: LocalClone, branch: String, remote: String },
    Browser { url: String },
    Host { host: String, port: u16 },
}

pub(super) struct Card {
    pub act: Act,
    pub title: String,
    pub lines: Vec<(String, Tone)>,
    pub yes: &'static str,
    pub no: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    List,
    Filter,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
    Sources,
    Refs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Wide,
    Medium,
    Phone,
}

impl Mode {
    pub fn of(width: f32, s: f32) -> Mode {
        let w = width / s.max(0.5);
        if w >= 1000.0 {
            Mode::Wide
        } else if w >= 620.0 {
            Mode::Medium
        } else {
            Mode::Phone
        }
    }
}

/// A started clone, fetch, pull or push.
pub(super) struct Run {
    pub batch: Batch,
    pub op: Op,
    pub label: String,
    /// Where the clone lands / the folder it runs in.
    pub dir: PathBuf,
    pub then_ide: Option<String>,
    pub told: bool,
}

// ------------------------------------------------------------------------------------------------ the app

pub struct GitApp {
    pub(super) rig: Rig,
    pub(super) user: String,
    pub(super) dir: PathBuf,
    pub(super) repos_root: PathBuf,
    pub(super) policy_fb: Option<PathBuf>,
    pub(super) srcs: Vec<Src>,
    pub(super) cur: usize,
    pub(super) boards: HashMap<String, Board>,
    pub(super) locals: Locals,
    pub(super) filter: TextField,
    pub(super) sort: Option<(usize, bool)>,
    pub(super) list: ListState,
    pub(super) src_list: ListState,
    pub(super) focus: Focus,
    pub(super) overlay: Option<Overlay>,
    pub(super) card: Option<Card>,
    pub(super) view: Option<Box<RepoView>>,
    pub(super) run: Option<Run>,
    pub(super) status: Option<(String, bool)>,
    pub(super) log: VecDeque<String>,
    pub(super) show_log: bool,
    pub(super) mouse: (i32, i32),
    pub(super) mode: Mode,
    pub(super) last_draw: Option<std::time::Instant>,
    pub(super) request: Option<(String, String)>,
    pub(super) tip: Tooltip,
    /// Until what timer millisecond each host asked us to slow down.
    pub(super) rate_until: HashMap<String, u64>,
    pub(super) order: Vec<usize>,
    pub(super) rows: Vec<Row>,
    order_key: Option<(String, u64, u64, String, Option<(usize, bool)>)>,
    entries_at: Option<std::time::Instant>,
    pub(super) status_job: Option<(Op, LocalClone, Job<Result<RepoStatus, BrowseErr>>)>,
    pub(super) host_busy: bool,
    /// After a clone that the AI IDE should open: the address key of the clone to look for.
    pub(super) pending_ide: Option<String>,
    /// Shown inside the AI IDE's Git tab: read-only (clone, fetch, pull and push are the IDE's Repositories panel's).
    pub(super) embedded: bool,
}

fn wall_to_log(secs: u64) -> String {
    format!("{:02}:{:02}:{:02} UTC", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

impl GitApp {
    pub fn new(cx: &AppCx) -> GitApp {
        Self::with_rig(cx, if cfg!(test) { Rig::offline() } else { Rig::system() })
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> GitApp {
        let dir = cx.services.dir().to_path_buf();
        let user = cx.session.user.clone();
        let (repos_root, policy_fb) = match rig.repos_root.clone() {
            Some(p) => (p, None),
            None => Self::default_root(cx),
        };
        let mut a = GitApp {
            rig,
            user,
            dir,
            repos_root,
            policy_fb,
            srcs: vec![],
            cur: 0,
            boards: HashMap::new(),
            locals: Locals::default(),
            filter: TextField::new("/ to filter", false),
            sort: None,
            list: ListState::default(),
            src_list: ListState::default(),
            focus: Focus::List,
            overlay: None,
            card: None,
            view: None,
            run: None,
            status: None,
            log: VecDeque::new(),
            show_log: false,
            mouse: (-1, -1),
            mode: Mode::Wide,
            last_draw: None,
            request: None,
            tip: Tooltip::new(),
            rate_until: HashMap::new(),
            order: vec![],
            rows: vec![],
            order_key: None,
            entries_at: None,
            status_job: None,
            host_busy: false,
            pending_ide: None,
            embedded: false,
        };
        a.refresh_entries(cx);
        let saved = a.saved_source();
        let want = saved.and_then(|k| a.srcs.iter().position(|s| s.key() == k)).or_else(|| {
            let d = a.conns(cx).default_connection()?;
            a.srcs.iter().position(|s| s.key() == d.id)
        });
        a.cur = want.unwrap_or(0);
        a.locals.want = true;
        a
    }

    /// `/home/<user>/Repositories`, or the account's folder under the data folder when it has no home to write in; the second value is the
    /// folder the jobs' write check must also allow.
    fn default_root(cx: &AppCx) -> (PathBuf, Option<PathBuf>) {
        let user = &cx.session.user;
        let home = PathBuf::from(format!("/home/{user}/Repositories"));
        let fb = cx.services.dir().join("repositories").join(user);
        let usable = cx.session.policy.allows_write(user, &home.to_string_lossy()) && home.parent().is_some_and(|p| p.is_dir());
        if usable { (home, None) } else { (fb.clone(), Some(fb)) }
    }

    pub(super) fn keys(&self, cx: &AppCx) -> GitKeys {
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        GitKeys::for_services(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)), iyakki_agent::gitkeys::shared_session())
    }

    pub(super) fn conns(&self, cx: &AppCx) -> Conns {
        self.keys(cx).conns()
    }

    fn pref_path(&self) -> PathBuf {
        let safe: String = self.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        self.dir.join(format!("git-tile-{safe}.json"))
    }

    fn saved_source(&self) -> Option<String> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(self.pref_path()).ok()?).ok()?;
        v.get("source")?.as_str().map(String::from)
    }

    fn save_source(&self) {
        if let Some(s) = self.srcs.get(self.cur) {
            let _ = std::fs::write(self.pref_path(), serde_json::json!({"source": s.key()}).to_string());
        }
    }

    // ---------------------------------------------------------------------------------------- the clock

    pub(super) fn now_ms(&self) -> u64 {
        self.rig.timer.now_ms()
    }
    pub(super) fn wall(&self) -> u64 {
        self.rig.timer.wall_secs()
    }
    /// A text input (the list filter) owns the keyboard.
    pub(super) fn wants_text(&self) -> bool {
        self.focus == Focus::Filter
    }
    /// A review card is waiting for an answer.
    pub(super) fn has_card(&self) -> bool {
        self.card.is_some()
    }
    pub(super) fn visible(&self) -> bool {
        self.last_draw.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
    }

    pub(super) fn say(&mut self, m: impl Into<String>, ok: bool) {
        let m = m.into();
        clipboard::toast(&m);
        self.status = Some((m, ok));
    }

    pub(super) fn log_push(&mut self, what: &str, e: &BrowseErr) {
        let kind = match e {
            BrowseErr::Transient(_) => "transient",
            BrowseErr::Rate(_) => "slow down",
            BrowseErr::Persistent(_) => "persistent",
            BrowseErr::TooBig(_) => "too large",
        };
        // the texts of a BrowseErr are already scrubbed; a line never holds a token or an address with a credential
        let line = format!("{}  {}  {}  \"{}\"", wall_to_log(self.wall()), what, kind, e.text());
        if self.log.len() >= LOG_KEEP {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    pub(super) fn log_text(&self) -> String {
        if self.log.is_empty() {
            "No problems with the hosts since this tile opened.".into()
        } else {
            self.log.iter().cloned().collect::<Vec<_>>().join("\n")
        }
    }

    // ---------------------------------------------------------------------------------------- sources

    /// Reads the connections again (Settings may have changed them) and keeps the chosen one.
    pub(super) fn refresh_entries(&mut self, cx: &AppCx) {
        self.entries_at = Some(std::time::Instant::now());
        let keep = self.srcs.get(self.cur).map(Src::key);
        let mut v: Vec<Src> = self.conns(cx).list().into_iter().map(Src::Conn).collect();
        v.push(Src::Local);
        if v.len() != self.srcs.len() || v.iter().zip(&self.srcs).any(|(a, b)| a.key() != b.key() || matches!((a, b), (Src::Conn(x), Src::Conn(y)) if x != y)) {
            self.srcs = v;
            if let Some(k) = keep {
                self.cur = self.srcs.iter().position(|s| s.key() == k).unwrap_or_else(|| self.first_choice());
                if self.view.as_ref().is_some_and(|v| v.src_key != k) && !self.srcs.iter().any(|s| s.key() == k) {
                    self.view = None;
                }
            }
            self.order_key = None;
        }
    }

    fn first_choice(&self) -> usize {
        0
    }

    pub(super) fn rows_dirty(&mut self) {
        self.order_key = None;
    }

    pub(super) fn src(&self) -> &Src {
        &self.srcs[self.cur.min(self.srcs.len() - 1)]
    }

    pub(super) fn src_key(&self) -> String {
        self.src().key()
    }

    pub(super) fn conn_list(&self) -> Vec<Connection> {
        self.srcs.iter().filter_map(|s| if let Src::Conn(c) = s { Some(c.clone()) } else { None }).collect()
    }

    pub(super) fn select_source(&mut self, i: usize) {
        if i >= self.srcs.len() {
            return;
        }
        self.cur = i;
        self.save_source();
        self.filter.clear();
        self.sort = None;
        self.list.reset();
        self.view = None;
        self.overlay = None;
        self.focus = Focus::List;
        self.order_key = None;
        self.status = None;
        let key = self.srcs[i].key();
        // read again when the last good list is old; the list shown meanwhile is that last one
        let wall = self.wall();
        if let Some(b) = self.boards.get_mut(&key) {
            if wall.saturating_sub(b.loaded_s) > FRESH_S && b.job.is_none() {
                b.reload = true;
                b.err = None;
                b.retry_at = None;
            }
        }
        if matches!(self.srcs[i], Src::Local) {
            self.locals.want = true;
        }
    }

    /// Whether the current source is a host (not the local clones) and its connection.
    pub(super) fn cur_conn(&self) -> Option<Connection> {
        if let Src::Conn(c) = self.src() { Some(c.clone()) } else { None }
    }

    pub(super) fn board(&mut self) -> &mut Board {
        let k = self.src_key();
        self.boards.entry(k).or_insert_with(Board::new)
    }

    // ---------------------------------------------------------------------------------------- loading the list

    pub(super) fn egress(&self, cx: &AppCx) -> Egress {
        let policy = cx.session.policy.clone();
        Arc::new(move |h| if policy.allows_host(h) { Ok(()) } else { Err(format!("Your account may not reach {h} (the network policy of this account).")) })
    }

    /// The API of one connection, with its token from the vault and the certificate the person trusted for it.
    pub(super) fn browser(&self, cx: &AppCx, c: &Connection) -> Result<Browser, BrowseErr> {
        browser_for(&self.conns(cx), self.rig.http.clone(), c, self.egress(cx), self.rig.timer.clone())
    }

    fn rate_left(&self, id: &str) -> Option<u64> {
        let now = self.now_ms();
        self.rate_until.get(id).copied().filter(|t| *t > now)
    }

    /// Seconds the person still waits for a host that asked us to slow down.
    pub(super) fn slow_down_s(&self) -> Option<u64> {
        let id = match &self.view {
            Some(v) => v.src_key.clone(),
            None => self.src_key(),
        };
        self.rate_left(&id).map(|t| t.saturating_sub(self.now_ms()).div_ceil(1000))
    }

    pub(super) fn start_repos(&mut self, cx: &AppCx, page: usize) {
        let Some(c) = self.cur_conn() else { return };
        if let Some(until) = self.rate_left(&c.id) {
            let b = self.board();
            b.retry_at = Some(until);
            return;
        }
        let (conns, http, eg, timer) = (self.conns(cx), self.rig.http.clone(), self.egress(cx), self.rig.timer.clone());
        let b = self.board();
        if b.job.is_some() {
            return;
        }
        b.asking = page;
        b.retry_at = None;
        b.reload = false;
        b.job = Some(Job::spawn(move || {
            let r = browser_for(&conns, http, &c, eg, timer).and_then(|b| b.repos(page));
            (page, r)
        }));
    }

    fn poll_boards(&mut self) {
        let now = self.now_ms();
        let wall = self.wall();
        let keys: Vec<String> = self.boards.keys().cloned().collect();
        for k in keys {
            let Some(done) = self.boards.get(&k).and_then(|b| b.job.as_ref()).and_then(|j| j.take()) else { continue };
            let name = self.srcs.iter().find(|s| s.key() == k).map(Src::name).unwrap_or_default();
            let b = self.boards.get_mut(&k).unwrap();
            b.job = None;
            match done {
                (page, Ok(p)) => {
                    if page <= 1 {
                        b.items = p.items;
                    } else {
                        for it in p.items {
                            if !b.items.iter().any(|x| x.full_name == it.full_name) {
                                b.items.push(it);
                            }
                        }
                    }
                    b.page = page.max(1);
                    b.more = p.more;
                    b.loaded = true;
                    b.err = None;
                    b.retry_at = None;
                    b.loaded_s = wall;
                    b.tracker.ok(wall);
                    b.version += 1;
                }
                (page, Err(e)) => {
                    b.tracker.failed(now, e.verdict(), &e.text(), false);
                    b.err = Some(e.clone());
                    b.retry_at = match &e {
                        BrowseErr::Transient(_) => Some(now + b.tracker.next_delay(std::time::Duration::from_secs(15)).as_millis() as u64),
                        BrowseErr::Rate(n) => Some(now + n * 1000),
                        _ => None,
                    };
                    if let BrowseErr::Rate(n) = &e {
                        self.rate_until.insert(k.clone(), now + n * 1000);
                    }
                    self.log_push(&format!("{name} repositories page {page}"), &e);
                }
            }
            self.order_key = None;
            self.end_refreshing();
        }
    }

    /// The "Refreshing..." line goes when an answer (good or bad) has come.
    pub(super) fn end_refreshing(&mut self) {
        if self.status.as_ref().is_some_and(|(t, _)| t.starts_with("Refreshing")) {
            self.status = None;
        }
    }

    pub(super) fn refresh_list(&mut self, cx: &AppCx) {
        self.refresh_entries(cx);
        match self.src() {
            Src::Local => self.locals.want = true,
            Src::Conn(c) => {
                let id = c.id.clone();
                self.rate_until.remove(&id);
                let b = self.board();
                b.reload = true;
                b.err = None;
                b.retry_at = None;
                b.tracker.reset_backoff();
            }
        }
        self.status = Some(("Refreshing\u{2026}".into(), true));
    }

    pub(super) fn load_more(&mut self, cx: &AppCx) {
        if self.cur_conn().is_none() {
            return;
        }
        let (more, page, busy) = {
            let b = self.board();
            (b.more, b.page, b.job.is_some())
        };
        if more && !busy {
            self.start_repos(cx, page + 1);
        }
    }

    // ---------------------------------------------------------------------------------------- the clones on this computer

    fn start_locals(&mut self) {
        if self.locals.job.is_some() {
            return;
        }
        self.locals.want = false;
        let (run, root) = (self.rig.run.clone(), self.repos_root.clone());
        let extra = self.workspace_repo_dirs();
        self.locals.job = Some(Job::spawn(move || gb::find_clones(&run, &root, &extra)));
    }

    /// The repositories the AI IDE's workspaces list (cloned there with "Clone repositories..."), as clones on this computer too.
    fn workspace_repo_dirs(&self) -> Vec<PathBuf> {
        let reg = iyakki_agent::workspace::Registry::load(&self.dir, &self.user);
        let mut v = Vec::new();
        for w in &reg.items {
            let root = w.root_path();
            if let Ok(c) = iyakki_agent::workspace::ProjectConfig::load(&root) {
                for r in c.repos {
                    if r.path != "." && !r.path.contains("..") {
                        v.push(root.join(&r.path));
                    }
                }
            }
            if root.join(".git").exists() {
                v.push(root);
            }
        }
        v
    }

    fn poll_locals(&mut self) {
        if let Some(l) = self.locals.job.as_ref().and_then(|j| j.take()) {
            self.locals.job = None;
            self.locals.list = l;
            self.locals.loaded = true;
            self.locals.version += 1;
            self.order_key = None;
        }
    }

    /// Index of the clone on this computer that is a copy of this repository of a host.
    pub(super) fn clone_of(&self, c: &Connection, it: &RepoItem) -> Option<usize> {
        let mine = [gb::remote_key(&it.https_url), gb::remote_key(&it.ssh_url), gb::remote_key(&format!("{}/{}", c.url, it.full_name))];
        self.locals.list.iter().position(|l| l.key().is_some_and(|k| mine.iter().any(|m| *m == k)))
    }

    // ---------------------------------------------------------------------------------------- rows

    pub(super) fn rows_now(&mut self) -> &[usize] {
        let key = self.src_key();
        let ver = self.boards.get(&key).map_or(0, |b| b.version);
        let k = (key.clone(), ver, self.locals.version, self.filter.value.to_lowercase(), self.sort);
        if self.order_key.as_ref() != Some(&k) {
            let mut rows: Vec<Row> = match self.src().clone() {
                Src::Local => self
                    .locals
                    .list
                    .iter()
                    .enumerate()
                    .map(|(i, l)| Row { name: l.name.clone(), desc: l.remote.clone().unwrap_or_else(|| "no remote".into()), branch: l.branch.clone(), clone: Some(i), ..Default::default() })
                    .collect(),
                Src::Conn(c) => self
                    .boards
                    .get(&key)
                    .map(|b| {
                        b.items
                            .iter()
                            .map(|it| Row {
                                name: it.full_name.clone(),
                                desc: it.description.clone(),
                                branch: it.default_branch.clone(),
                                private: Some(it.private),
                                updated: it.updated,
                                stars: it.stars,
                                size_kb: it.size_kb,
                                clone: self.clone_of(&c, it),
                                item: Some(it.clone()),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            let f = self.filter.value.to_lowercase();
            if !f.is_empty() {
                rows.retain(|r| r.name.to_lowercase().contains(&f) || r.desc.to_lowercase().contains(&f) || r.branch.to_lowercase().contains(&f));
            }
            let (col, desc) = self.sort.unwrap_or(if matches!(self.src(), Src::Local) { (0, false) } else { (4, true) });
            rows.sort_by(|a, b| {
                let o = compare(a, b, col).then_with(|| a.name.cmp(&b.name));
                if desc { o.reverse() } else { o }
            });
            self.order = (0..rows.len()).collect();
            self.rows = rows;
            self.order_key = Some(k);
        }
        &self.order
    }

    pub(super) fn selected_row(&mut self) -> Option<Row> {
        let n = self.rows_now().len();
        let i = self.list.sel.min(n.saturating_sub(1));
        self.order.get(i).and_then(|&k| self.rows.get(k)).cloned()
    }

    pub(super) fn sort_by(&mut self, col: usize) {
        let default = if matches!(self.src(), Src::Local) { (0, false) } else { (4, true) };
        let cur = self.sort.unwrap_or(default);
        self.sort = Some(if cur.0 == col { (col, !cur.1) } else { (col, col == 4 || col == 5 || col == 6) });
        self.list.reset();
    }

    // ---------------------------------------------------------------------------------------- the heartbeat

    pub(super) fn tick(&mut self, cx: &AppCx) {
        let now = self.now_ms();
        let vis = self.visible();
        if self.entries_at.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2)) && vis {
            self.refresh_entries(cx);
        }
        self.poll_boards();
        self.poll_locals();
        self.poll_status_job(cx);
        self.poll_run(cx);
        self.poll_view(now);
        if !vis {
            return;
        }
        // the list of the chosen host: the first read, a retry after a transient failure, "slow down" waits
        if self.view.is_none() {
            if self.cur_conn().is_some() {
                let due = {
                    let b = self.board();
                    b.job.is_none() && (((!b.loaded || b.reload) && b.err.is_none() && b.retry_at.is_none()) || b.retry_at.is_some_and(|t| t <= now))
                };
                if due {
                    let page = {
                        let b = self.board();
                        if b.loaded && !b.reload { b.asking.max(1) } else { 1 }
                    };
                    self.start_repos(cx, page);
                }
            }
        }
        if self.locals.want || (!self.locals.loaded && self.locals.job.is_none()) {
            self.start_locals();
        }
        self.drive_view(cx, now);
        if self.status.as_ref().is_some_and(|(t, _)| t.starts_with("The host asked us to slow down")) && self.slow_down_s().is_none() {
            self.status = None;
        }
    }

    /// The text for the status line: what happened last, else a wait the host asked for, else the age of stale data.
    pub(super) fn status_line(&mut self) -> Option<(String, bool)> {
        if let Some(n) = self.slow_down_s() {
            return Some((format!("The host asked us to slow down: retry in {n} s."), true));
        }
        if let Some(r) = &self.run {
            let (done, total) = (r.batch.done(), r.batch.items.len());
            if r.batch.pending() {
                let it = r.batch.items.first();
                let p = it.and_then(|i| i.progress).map(|p| format!(" {p:.0}%")).unwrap_or_default();
                let phase = if matches!(r.batch.phase, Phase::Confirm) { " - waiting for your decision about the host key" } else { "" };
                return Some((format!("{} {}{p}{phase} ({done}/{total})", capitalize(verb_ing(r.op)), r.label), true));
            }
        }
        if let Some((m, ok)) = &self.status {
            return Some((m.clone(), *ok));
        }
        if self.view.is_none() && self.cur_conn().is_some() {
            let now = self.now_ms();
            let wall = self.wall();
            let b = self.board();
            let st = b.tracker.status(now, wall);
            if st.failing && !st.banner && b.loaded {
                let age = st.last_ok_age_s.map(|a| format!(" \u{b7} the list is {a} s old")).unwrap_or_default();
                return Some((format!("Updating\u{2026} (the host did not answer, retrying in {} s){age}", st.retry_every_s), true));
            }
            if let Some(e) = b.err.clone() {
                if b.loaded && !matches!(e, BrowseErr::Rate(_)) {
                    return Some((format!("Showing the last list. {}", e.text()), false));
                }
            }
        }
        None
    }

    // ---------------------------------------------------------------------------------------- actions: review cards

    fn tools(&self, cx: &AppCx) -> Result<Tools, String> {
        match &self.rig.tools {
            Some(t) => Ok(t.clone()),
            None => Tools::detect(&cx.services.clis.search_path()),
        }
    }

    /// The jobs' context for a folder: its keys, programs and write check.
    pub(super) fn git_ctx(&self, cx: &AppCx, root: &std::path::Path, name: &str) -> Result<gitjobs::Ctx, String> {
        let tools = self.tools(cx)?;
        let search = cx.services.clis.search_path();
        let safe: String = self.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        let mut policy = cx.session.policy.clone();
        if let Some(fb) = &self.policy_fb {
            let fb = fb.to_string_lossy().into_owned();
            if !policy.fs_write.is_empty() && !policy.allows_write(&self.user, &fb) {
                policy.fs_write.push(fb);
            }
        }
        let u2 = self.user.clone();
        Ok(gitjobs::Ctx {
            keys: Arc::new(self.keys(cx)),
            tools,
            owner: self.user.clone(),
            workspace: name.to_string(),
            root: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            ws_keys: iyakki_agent::workspace::ProjectConfig::load(root).map(|c| c.git_keys).unwrap_or_default(),
            who: Who::Person,
            run_dir: cx.services.dir().join("git-run").join(safe),
            probe: self.rig.probe.clone().unwrap_or_else(|| gitjobs::real_probe(search)),
            scope: Arc::new(move |p: &std::path::Path| iyakki_agent::scope::check_write(&policy, &u2, p)),
        })
    }

    /// The clone address for a repository of a connection, by the way the connection signs in.
    pub(super) fn clone_url(c: &Connection, full: &str) -> String {
        c.remote_for(full)
    }

    /// Clone: the review of what is cloned from where into which folder, with whom it signs in.
    pub(super) fn begin_clone(&mut self, then_ide: bool) {
        if self.embedded {
            self.say(EMBEDDED_NO_WRITE, false);
            return;
        }
        if self.busy() {
            return;
        }
        let Some((conn, full)) = self.target_repo() else {
            self.say("Choose a repository of a host to clone.", false);
            return;
        };
        let url = Self::clone_url(&conn, &full);
        let (https, ssh) = conn.clone_urls(&full);
        let name = full.rsplit('/').next().unwrap_or(&full).to_string();
        let mut lines = vec![
            (format!("{full} from {} ({}, {})", conn.name, conn.kind.label(), place_text(&conn)), Tone::Bold),
            (format!("Copied into {}/{name} on this computer.", self.repos_root.display()), Tone::Normal),
            (format!("Address: {}", gitconn::scrub(if conn.method == gitconn::Method::Ssh { &ssh } else { &https }, "")), Tone::Normal),
            (format!("Signs in with: {}", conn.auth_label()), Tone::Dim),
        ];
        if then_ide {
            lines.push(("When it is done the AI IDE opens it as a workspace.".into(), Tone::Dim));
        }
        lines.push(("The files come to this computer; nothing on the host changes.".into(), Tone::Dim));
        self.card = Some(Card { act: Act::Clone { conn, full, url, then_ide }, title: "Clone this repository?".into(), lines, yes: "Clone", no: "Cancel" });
    }

    /// The repository the next action is about: the open one, else the selected row (only repositories of a host can be cloned).
    fn target_repo(&mut self) -> Option<(Connection, String)> {
        if let Some(v) = &self.view {
            let c = v.conn.clone()?;
            return Some((c, v.item.full_name.clone()));
        }
        let c = self.cur_conn()?;
        let r = self.selected_row()?;
        Some((c, r.item?.full_name))
    }

    /// The clone on this computer the next action is about.
    pub(super) fn target_clone(&mut self) -> Option<LocalClone> {
        if let Some(v) = &self.view {
            return v.clone.clone();
        }
        let r = self.selected_row()?;
        r.clone.and_then(|i| self.locals.list.get(i).cloned())
    }

    pub(super) fn busy(&mut self) -> bool {
        if self.run.as_ref().is_some_and(|r| r.batch.pending()) || self.status_job.is_some() {
            self.say("Something is still running here; wait for it to finish.", false);
            return true;
        }
        false
    }

    pub(super) fn begin_fetch(&mut self, cx: &AppCx) {
        if self.embedded {
            self.say(EMBEDDED_NO_WRITE, false);
            return;
        }
        if self.busy() {
            return;
        }
        let Some(cl) = self.target_clone() else {
            self.say("Fetch needs a clone on this computer: clone the repository first (C).", false);
            return;
        };
        self.start_update(cx, Op::Fetch, &cl);
    }

    pub(super) fn begin_status(&mut self, op: Op) {
        if self.embedded {
            self.say(EMBEDDED_NO_WRITE, false);
            return;
        }
        if self.busy() {
            return;
        }
        let Some(cl) = self.target_clone() else {
            self.say(format!("{} needs a clone on this computer: clone the repository first (C).", if op == Op::Pull { "Pull" } else { "Push" }), false);
            return;
        };
        let (run, dir, scratch) = (self.rig.run.clone(), cl.dir.clone(), self.rig.scratch.clone());
        self.status_job = Some((op, cl, Job::spawn(move || LocalRepo::new(run, &dir, &scratch).status())));
        self.say(format!("Looking at the clone before the {}\u{2026}", op.verb()), true);
    }

    fn poll_status_job(&mut self, cx: &AppCx) {
        let Some((_, _, j)) = &self.status_job else { return };
        let Some(r) = j.take() else { return };
        let (op, cl, _) = self.status_job.take().unwrap();
        self.status = None;
        match r {
            Err(e) => self.say(e.text(), false),
            Ok(st) if op == Op::Pull => {
                if st.dirty() {
                    let mut lines = vec![(format!("{} has changes that are not committed ({}).", cl.name, st.summary()), Tone::Warn)];
                    lines.push(("A pull here only moves forward (fast-forward only); it never merges and never rebases. Git stops by itself when a changed file would be overwritten, and tells you which.".into(), Tone::Dim));
                    self.card = Some(Card { act: Act::Pull { clone: cl }, title: "Pull over local changes?".into(), lines, yes: "Pull, fast-forward only", no: "Cancel" });
                } else {
                    self.start_update(cx, Op::Pull, &cl);
                }
            }
            Ok(st) => {
                if st.detached || st.branch.is_empty() {
                    self.say("Check out a branch first: a detached HEAD cannot be pushed.", false);
                    return;
                }
                let Some(remote) = LocalRepo::new(self.rig.run.clone(), &cl.dir, &self.rig.scratch).origin() else {
                    self.say("This clone has no origin to push to.", false);
                    return;
                };
                if !st.upstream.is_empty() && st.ahead == 0 {
                    self.say(format!("Nothing to push: {} is up to date with {}.", st.branch, st.upstream), true);
                    return;
                }
                let host = gitconn::host_of(&remote);
                let conn = self.conn_list().into_iter().find(|c| c.host() == host);
                let mut lines = vec![
                    (format!("Branch {} of {} goes to {}", st.branch, cl.name, remote), Tone::Bold),
                    (match conn {
                        Some(c) => format!("On {} ({}, {}).", c.name, c.kind.label(), place_text(&c)),
                        None => format!("On {host}: it is not one of your connections, so git uses the sign-in of its address."),
                    }, Tone::Normal),
                ];
                if st.upstream.is_empty() {
                    lines.push(("The branch is new on the host (no upstream yet).".into(), Tone::Normal));
                } else {
                    lines.push((format!("{} commit(s) ahead, {} behind {}.", st.ahead, st.behind, st.upstream), if st.behind > 0 { Tone::Warn } else { Tone::Normal }));
                }
                if st.dirty() {
                    lines.push(("Changes that are not committed stay here and are not sent.".into(), Tone::Dim));
                }
                lines.push(("Nothing is forced and nothing is deleted on the host: a push that is not a fast-forward is refused by git.".into(), Tone::Dim));
                self.card = Some(Card { act: Act::Push { clone: cl, branch: st.branch, remote }, title: "Push to the host?".into(), lines, yes: "Push", no: "Cancel" });
            }
        }
    }

    pub(super) fn begin_browser(&mut self) {
        let url = match &self.view {
            Some(v) if !v.item.web_url.is_empty() => v.item.web_url.clone(),
            Some(v) => v.clone.as_ref().and_then(|c| c.remote.clone()).map(|r| web_of(&r)).unwrap_or_default(),
            None => self.selected_row().and_then(|r| r.item).map(|i| i.web_url).unwrap_or_default(),
        };
        if url.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            self.say("This repository has no web page to open.", false);
            return;
        }
        let lines = vec![(url.clone(), Tone::Bold), ("The BROWSER tile opens this address in a new tab. Nothing is sent to it from here.".into(), Tone::Dim)];
        self.card = Some(Card { act: Act::Browser { url }, title: "Open this page in the browser?".into(), lines, yes: "Open", no: "Cancel" });
    }

    pub(super) fn copy_clone_url(&mut self, ssh: Option<bool>) {
        let (c, full) = match (&self.view, self.cur_conn()) {
            (Some(v), _) => match &v.conn {
                Some(c) => (c.clone(), v.item.full_name.clone()),
                None => {
                    let r = v.clone.as_ref().and_then(|c| c.remote.clone());
                    match r {
                        Some(r) => {
                            clipboard::copy(&r);
                            self.say("Copied the address of origin.", true);
                        }
                        None => self.say("This clone has no remote address.", false),
                    }
                    return;
                }
            },
            (None, Some(c)) => match self.selected_row().and_then(|r| r.item) {
                Some(it) => (c, it.full_name),
                None => return,
            },
            _ => return,
        };
        let (https, ssh_url) = c.clone_urls(&full);
        let ssh = ssh.unwrap_or(c.method == gitconn::Method::Ssh);
        let url = if ssh { ssh_url } else { https };
        clipboard::copy(&url);
        self.say(format!("Copied the {} address of {full}.", if ssh { "SSH" } else { "HTTPS" }), true);
    }

    pub(super) fn begin_ide(&mut self, cx: &AppCx) {
        if self.embedded {
            self.say(EMBEDDED_NO_WRITE, false);
            return;
        }
        match self.target_clone() {
            Some(cl) => self.open_ide(cx, &cl),
            None => {
                if self.cur_conn().is_none() && self.view.as_ref().is_none_or(|v| v.conn.is_none()) {
                    self.say("Choose a repository first.", false);
                } else {
                    self.begin_clone(true);
                }
            }
        }
    }

    fn open_ide(&mut self, cx: &AppCx, cl: &LocalClone) {
        let mut reg = iyakki_agent::workspace::Registry::load(&self.dir, &self.user);
        let canon = std::fs::canonicalize(&cl.dir).unwrap_or_else(|_| cl.dir.clone());
        let id = match reg.items.iter().find(|w| std::path::Path::new(&w.root) == canon) {
            Some(w) => w.id.clone(),
            None => {
                if !cx.session.policy.allows_write(&self.user, &canon.to_string_lossy()) && self.policy_fb.is_none() {
                    self.say(format!("Your account may not write in {}, so the AI IDE cannot take it as a workspace.", canon.display()), false);
                    return;
                }
                match reg.add(&cl.name, &canon) {
                    Ok(w) => {
                        cx.audit.record(&self.user, "ide.workspace.add", &w.root);
                        w.id
                    }
                    Err(e) => {
                        self.say(e, false);
                        return;
                    }
                }
            }
        };
        self.request = Some(("ide".into(), format!("ws:{id}")));
    }

    // ---------------------------------------------------------------------------------------- actions: starting the jobs

    pub(super) fn confirm(&mut self, cx: &AppCx) {
        let Some(card) = self.card.take() else { return };
        match card.act {
            Act::Clone { conn, full, url, then_ide } => self.start_clone(cx, &conn, &full, &url, then_ide),
            Act::Pull { clone } => self.start_update(cx, Op::Pull, &clone),
            Act::Push { clone, branch, remote } => self.start_push(cx, &clone, &branch, &remote),
            Act::Browser { url } => {
                self.request = Some(("browser".into(), format!("newtab:{url}")));
            }
            Act::Host { host, port } => {
                if let Some(r) = &mut self.run {
                    match r.batch.trust_host(&host, port) {
                        Ok(()) => self.status = Some((format!("Trusted {host}. Its key is recorded in your known hosts; a different key later is refused."), true)),
                        Err(e) => self.status = Some((e, false)),
                    }
                    self.host_busy = false;
                }
            }
        }
    }

    pub(super) fn cancel_card(&mut self) {
        if let Some(card) = self.card.take() {
            if let Act::Host { host, port } = card.act {
                if let Some(r) = &mut self.run {
                    r.batch.refuse_host(&host, port);
                }
                self.host_busy = false;
            }
        }
    }

    pub(super) fn start_clone(&mut self, cx: &AppCx, conn: &Connection, full: &str, url: &str, then_ide: bool) {
        if let Err(e) = std::fs::create_dir_all(&self.repos_root).map_err(|e| e.to_string()).and_then(|_| iyakki_agent::workspace::ProjectConfig::ensure(&self.repos_root, "Repositories").map(|_| ())) {
            self.say(format!("The Repositories folder {} could not be made: {e}", self.repos_root.display()), false);
            return;
        }
        let root = self.repos_root.clone();
        let ctx = match self.git_ctx(cx, &root, "Repositories") {
            Ok(c) => c,
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        let spec = CloneSpec { url: url.to_string(), branch: String::new(), depth: 0, folder: String::new() };
        match Batch::plan_clone(ctx, &[spec]) {
            Ok(b) => {
                cx.audit.record(&self.user, "git.clone.start", &format!("1 repositories into {}", "Repositories"));
                let name = b.items.first().map(|i| i.folder.clone()).unwrap_or_default();
                let key = gb::remote_key(url);
                self.run = Some(Run { batch: b, op: Op::Clone, label: format!("{full} from {}", conn.name), dir: root.join(name), then_ide: then_ide.then_some(key), told: false });
                self.status = None;
            }
            Err(errs) => self.say(errs.into_iter().take(3).map(|x| x.1).collect::<Vec<_>>().join("   "), false),
        }
    }

    fn start_update(&mut self, cx: &AppCx, op: Op, cl: &LocalClone) {
        let Some(remote) = LocalRepo::new(self.rig.run.clone(), &cl.dir, &self.rig.scratch).origin() else {
            self.say("This clone has no origin to fetch from.", false);
            return;
        };
        let ctx = match self.git_ctx(cx, &cl.dir, &cl.name) {
            Ok(c) => c,
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        let entry = iyakki_agent::gitsite::RepoEntry { url: remote, path: ".".into(), ..Default::default() };
        match Batch::plan_update(ctx, &[entry], op) {
            Ok(b) => {
                cx.audit.record(&self.user, &format!("git.{}.start", op.verb()), &format!("1 repositories in {}", cl.name));
                self.run = Some(Run { batch: b, op, label: cl.name.clone(), dir: cl.dir.clone(), then_ide: None, told: false });
                self.status = None;
            }
            Err(errs) => self.say(errs.into_iter().take(3).map(|x| x.1).collect::<Vec<_>>().join("   "), false),
        }
    }

    fn start_push(&mut self, cx: &AppCx, cl: &LocalClone, branch: &str, remote: &str) {
        let ctx = match self.git_ctx(cx, &cl.dir, &cl.name) {
            Ok(c) => c,
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        match Batch::plan_push(ctx, remote, ".", branch) {
            Ok(b) => {
                cx.audit.record(&self.user, "git.push", &format!("{} {}", gitconn::host_of(remote), cl.name));
                self.run = Some(Run { batch: b, op: Op::Push, label: cl.name.clone(), dir: cl.dir.clone(), then_ide: None, told: false });
                self.status = None;
            }
            Err(errs) => self.say(errs.into_iter().take(3).map(|x| x.1).collect::<Vec<_>>().join("   "), false),
        }
    }

    /// The running batch: its jobs, a new SSH host to decide about, and the end (the list of clones is read again, the IDE opens).
    fn poll_run(&mut self, cx: &AppCx) {
        let jobs = &cx.services.jobs;
        let run_jobs = self.rig.run_jobs;
        let mut finished: Option<(Op, String, bool, String, Option<String>)> = None;
        let mut prompt = None;
        if let Some(r) = &mut self.run {
            if run_jobs {
                r.batch.tick(jobs);
            }
            if r.batch.phase == Phase::Confirm {
                prompt = r.batch.prompts.first().cloned();
            }
            if !r.batch.pending() && !r.told {
                r.told = true;
                let failed = r.batch.failed() > 0;
                let why = r.batch.items.iter().find_map(|i| if let ItemState::Failed(m) = &i.state { Some(m.clone()) } else { None }).unwrap_or_default();
                finished = Some((r.op, r.label.clone(), failed, why, r.then_ide.clone()));
            }
        }
        if let Some(p) = prompt {
            if self.card.is_none() && !self.host_busy {
                self.host_busy = true;
                let mut lines = vec![(format!("You have not connected to {}{} before. It presents:", p.host, if p.port == 22 { String::new() } else { format!(":{}", p.port) }), Tone::Normal)];
                for k in &p.keys {
                    lines.push((format!("  {}  {}", k.kind, k.fingerprint()), Tone::Bold));
                }
                lines.push(("Compare this fingerprint with the one the site publishes or your server's owner gave you. Trust it only if it is the same: nothing is sent to the host before that.".into(), Tone::Dim));
                self.card = Some(Card { act: Act::Host { host: p.host.clone(), port: p.port }, title: format!("A new host: {}", p.host), lines, yes: "Trust this host", no: "Do not trust" });
            }
        }
        if let Some((op, label, failed, why, then_ide)) = finished {
            self.locals.want = true;
            if failed {
                self.say(format!("The {} of {label} failed: {}", op.verb(), if why.is_empty() { "git said nothing".to_string() } else { why }), false);
            } else {
                let dir = self.run.as_ref().map(|r| r.dir.clone()).unwrap_or_default();
                self.say(match op {
                    Op::Clone => format!("Cloned {label} into {}.", dir.display()),
                    Op::Fetch => format!("Fetched {label}."),
                    Op::Pull => format!("Pulled {label}."),
                    Op::Push => format!("Pushed {label}."),
                }, true);
                if let Some(key) = then_ide {
                    self.pending_ide = Some(key);
                }
            }
        }
        // after a clone, once the list of clones has been read again: open the AI IDE
        if self.locals.job.is_none() && !self.locals.want {
            if let Some(key) = self.pending_ide.take() {
                match self.locals.list.iter().find(|l| l.key().as_deref() == Some(key.as_str())).cloned() {
                    Some(cl) => self.open_ide(cx, &cl),
                    None => self.say("The clone is done, but it was not found among the clones on this computer.", false),
                }
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn verb_ing(op: Op) -> &'static str {
    match op {
        Op::Clone => "cloning",
        Op::Fetch => "fetching",
        Op::Pull => "pulling",
        Op::Push => "pushing",
    }
}

/// `https://host/team/app.git` -> `https://host/team/app`; an SSH address gives its https page.
pub(super) fn web_of(remote: &str) -> String {
    let r = remote.trim().trim_end_matches('/');
    let r = r.strip_suffix(".git").unwrap_or(r);
    if r.starts_with("https://") || r.starts_with("http://") {
        return r.to_string();
    }
    if let Some(rest) = r.strip_prefix("ssh://") {
        let rest = rest.rsplit('@').next().unwrap_or(rest);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        return format!("https://{}/{path}", host.split(':').next().unwrap_or(host));
    }
    if let Some((l, path)) = r.split_once(':') {
        if !l.contains('/') {
            return format!("https://{}/{}", l.rsplit('@').next().unwrap_or(l), path);
        }
    }
    String::new()
}

/// The API of a connection with its token from the vault (empty: public repositories only).
pub(super) fn browser_for(conns: &Conns, http: Arc<dyn GitHttp>, c: &Connection, eg: Egress, timer: Arc<dyn Timer>) -> Result<Browser, BrowseErr> {
    let token = if c.has_token() { conns.token(c).map_err(|e| BrowseErr::Persistent(gitconn::scrub(&e, "")))?.to_string() } else { String::new() };
    Ok(Browser::new(http, c.clone(), &token, conns.pinned_cert(c), eg).with_clock(Arc::new(move || timer.wall_secs())))
}

// ------------------------------------------------------------------------------------------------ events

impl GitApp {
    pub(super) fn handle_event(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.tip.event(ev);
        match ev {
            Event::Tick => {
                self.tick(cx);
                return Flow::Stay;
            }
            Event::MouseMove(x, y) | Event::MouseDown(x, y) => {
                self.mouse = (*x, *y);
                if matches!(ev, Event::MouseMove(..)) {
                    return Flow::Stay;
                }
            }
            Event::Scroll(d) => {
                self.wheel(*d);
                return Flow::Stay;
            }
            Event::Paste(t) => {
                if self.focus == Focus::Filter {
                    self.filter.paste(t);
                    self.list.reset();
                }
                return Flow::Stay;
            }
            Event::FocusAt(F_FILTER, x) => {
                self.focus = Focus::Filter;
                self.filter.click(*x);
                return Flow::Stay;
            }
            Event::Tab(n) => {
                if let Some(f) = self.click(*n, cx) {
                    return f;
                }
            }
            Event::Key(k) => return self.key(k, cx),
            _ => {}
        }
        Flow::Stay
    }

    fn wheel(&mut self, d: i32) {
        let rows = -d * WHEEL_ROWS;
        if let Some(o) = self.overlay {
            let n = if o == Overlay::Sources { self.srcs.len() + 1 } else { self.view.as_ref().map_or(0, |v| v.ref_rows().len()) };
            self.src_list.scroll(n, rows);
            return;
        }
        let (x, y) = self.mouse;
        if self.view.is_some() {
            self.view_wheel(rows, x, y);
            return;
        }
        if self.list.rect.contains(x, y) {
            let n = self.rows_now().len();
            self.list.scroll(n, rows);
        }
    }

    pub(super) fn click(&mut self, n: usize, cx: &AppCx) -> Option<Flow> {
        match n {
            T_SOURCE => {
                self.overlay = Some(Overlay::Sources);
                self.src_list.sel = self.cur;
                self.src_list.reveal = true;
            }
            T_REFRESH => {
                if self.view.is_some() {
                    self.refresh_view();
                } else {
                    self.refresh_list(cx);
                }
            }
            T_LOG => self.show_log = !self.show_log,
            T_LOG_COPY => {
                let t = self.log_text();
                clipboard::copy(&t);
                self.say("Copied the log.", true);
            }
            T_ADD => self.request = Some(("settings".into(), "git:add".into())),
            T_EDIT_CONN => self.request = Some(("settings".into(), "tab:git".into())),
            T_MORE => self.load_more(cx),
            T_CLEAR => {
                self.filter.clear();
                self.list.reset();
            }
            T_BACK => return self.back(),
            T_CLONE => self.begin_clone(false),
            T_IDE => self.begin_ide(cx),
            T_FETCH => self.begin_fetch(cx),
            T_PULL => self.begin_status(Op::Pull),
            T_PUSH => self.begin_status(Op::Push),
            T_COPY_URL => self.copy_clone_url(None),
            T_COPY_HTTPS => self.copy_clone_url(Some(false)),
            T_COPY_SSH => self.copy_clone_url(Some(true)),
            T_BROWSER => self.begin_browser(),
            T_YES => self.confirm(cx),
            T_NO => self.cancel_card(),
            T_CLOSE => self.overlay = None,
            T_REF => self.open_refs(),
            T_COPY_TEXT => self.copy_view_text(),
            T_VIEW_RAW => {
                if let Some(f) = self.view.as_mut().and_then(|v| v.files.file.as_mut()) {
                    f.raw = !f.raw;
                }
            }
            n if (T_TAB..T_TAB + 6).contains(&n) => self.set_tab(n - T_TAB),
            n if (T_SUB..T_SUB + 2).contains(&n) => self.set_sub(n - T_SUB == 1),
            n if (T_SORT..T_SORT + 20).contains(&n) => self.sort_by(n - T_SORT),
            n if (T_SRC_ROW..T_SRC_ROW + 1000).contains(&n) => self.pick_source(n - T_SRC_ROW),
            n if (T_REFROW..T_REFROW + 100_000).contains(&n) => self.pick_ref(n - T_REFROW),
            n if (T_ROW..T_ROW + 100_000).contains(&n) => {
                self.list.sel = n - T_ROW;
                self.focus = Focus::List;
                self.open_row(cx);
            }
            n if n >= T_FROW => self.view_click(n),
            _ => {}
        }
        None
    }

    /// Back: out of the card, the log, the repository, else out of the tile.
    pub(super) fn back(&mut self) -> Option<Flow> {
        if self.card.is_some() {
            self.cancel_card();
        } else if self.overlay.is_some() {
            self.overlay = None;
        } else if self.show_log {
            self.show_log = false;
        } else if self.view.is_some() {
            if !self.view_back() {
                self.leave_repo();
            }
        } else {
            return Some(Flow::Back);
        }
        None
    }

    fn pick_source(&mut self, i: usize) {
        if i == self.srcs.len() {
            self.overlay = None;
            self.request = Some(("settings".into(), "git:add".into()));
        } else {
            self.select_source(i);
        }
    }

    pub(super) fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        if self.card.is_some() {
            match k {
                Key::Esc | Key::Char('n') => self.cancel_card(),
                Key::Enter | Key::Char('y') => self.confirm(cx),
                _ => {}
            }
            return Flow::Stay;
        }
        if self.focus == Focus::Filter {
            return self.filter_key(k);
        }
        if let Some(o) = self.overlay {
            return self.overlay_key(o, k);
        }
        if self.show_log {
            match k {
                Key::Esc | Key::Char('L') => self.show_log = false,
                Key::Char('c') => {
                    let t = self.log_text();
                    clipboard::copy(&t);
                    self.say("Copied the log.", true);
                }
                _ => {}
            }
            return Flow::Stay;
        }
        // the keys that mean the same in both views
        match k {
            Key::Char('r') => {
                if self.view.is_some() {
                    self.refresh_view();
                } else {
                    self.refresh_list(cx);
                }
                return Flow::Stay;
            }
            Key::Char('L') => {
                self.show_log = true;
                return Flow::Stay;
            }
            Key::Char('a') if self.view.is_none() => {
                self.request = Some(("settings".into(), "git:add".into()));
                return Flow::Stay;
            }
            Key::Char('C') => {
                self.begin_clone(false);
                return Flow::Stay;
            }
            Key::Char('I') => {
                self.begin_ide(cx);
                return Flow::Stay;
            }
            Key::Char('F') => {
                self.begin_fetch(cx);
                return Flow::Stay;
            }
            Key::Char('P') => {
                self.begin_status(Op::Pull);
                return Flow::Stay;
            }
            Key::Char('U') => {
                self.begin_status(Op::Push);
                return Flow::Stay;
            }
            Key::Char('y') => {
                self.copy_clone_url(None);
                return Flow::Stay;
            }
            Key::Char('w') => {
                self.begin_browser();
                return Flow::Stay;
            }
            _ => {}
        }
        if self.view.is_some() {
            return self.view_key(k, cx);
        }
        let n = self.rows_now().len();
        match k {
            Key::Esc => {
                if !self.filter.value.is_empty() {
                    self.filter.clear();
                    self.list.reset();
                } else {
                    return Flow::Back;
                }
            }
            Key::Char('/') => self.focus = Focus::Filter,
            Key::Char('c') | Key::Char('s') => {
                self.overlay = Some(Overlay::Sources);
                self.src_list.sel = self.cur;
                self.src_list.reveal = true;
            }
            Key::Char('m') => self.load_more(cx),
            Key::Char('o') => {
                let cur = self.sort.map_or(if matches!(self.src(), Src::Local) { 0 } else { 4 }, |s| s.0);
                self.sort_by((cur + 1) % COLS.len());
            }
            Key::Char('O') => {
                let (c, d) = self.sort.unwrap_or(if matches!(self.src(), Src::Local) { (0, false) } else { (4, true) });
                self.sort = Some((c, !d));
                self.list.reset();
            }
            Key::Enter => self.open_row(cx),
            other => {
                if let Some(m) = nav_of(other) {
                    nav(&mut self.list, n, m);
                }
            }
        }
        Flow::Stay
    }

    fn filter_key(&mut self, k: &Key) -> Flow {
        match k {
            Key::Esc => {
                self.filter.clear();
                self.focus = Focus::List;
                self.list.reset();
            }
            Key::Enter => self.focus = Focus::List,
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                let n = self.rows_now().len();
                if let Some(m) = nav_of(k) {
                    nav(&mut self.list, n, m);
                }
            }
            other => {
                self.filter.handle(other);
                self.list.reset();
            }
        }
        Flow::Stay
    }

    fn overlay_key(&mut self, o: Overlay, k: &Key) -> Flow {
        let n = if o == Overlay::Sources { self.srcs.len() + 1 } else { self.view.as_ref().map_or(0, |v| v.ref_rows().len()) };
        match k {
            Key::Esc => self.overlay = None,
            Key::Enter => {
                let i = self.src_list.sel;
                if o == Overlay::Sources {
                    self.pick_source(i);
                } else {
                    self.pick_ref(i);
                }
            }
            other => {
                if let Some(m) = nav_of(other) {
                    nav(&mut self.src_list, n, m);
                }
            }
        }
        Flow::Stay
    }

    /// Enter on a row of the list: open the repository.
    pub(super) fn open_row(&mut self, cx: &AppCx) {
        let Some(row) = self.selected_row() else { return };
        self.open_repo(cx, &row);
    }

    pub(super) fn copy_view_text(&mut self) {
        if let Some(t) = self.view.as_ref().and_then(|v| v.copy_text()) {
            clipboard::copy(&t);
            self.say("Copied.", true);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum NavKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

pub(super) fn nav_of(k: &Key) -> Option<NavKey> {
    Some(match k {
        Key::Up => NavKey::Up,
        Key::Down => NavKey::Down,
        Key::PageUp => NavKey::PageUp,
        Key::PageDown => NavKey::PageDown,
        Key::Home => NavKey::Home,
        Key::End => NavKey::End,
        Key::Nav { nav, .. } => match nav {
            Nav::Up => NavKey::Up,
            Nav::Down => NavKey::Down,
            Nav::PageUp => NavKey::PageUp,
            Nav::PageDown => NavKey::PageDown,
            Nav::Home => NavKey::Home,
            Nav::End => NavKey::End,
            _ => return None,
        },
        _ => return None,
    })
}

pub(super) fn nav(l: &mut ListState, n: usize, k: NavKey) {
    match k {
        NavKey::Up => l.up(n, 1),
        NavKey::Down => l.down(n, 1),
        NavKey::PageUp => {
            let by = l.page();
            l.up(n, by);
        }
        NavKey::PageDown => {
            let by = l.page();
            l.down(n, by);
        }
        NavKey::Home => {
            l.sel = 0;
            l.reveal = true;
        }
        NavKey::End => {
            l.sel = n.saturating_sub(1);
            l.reveal = true;
        }
    }
}

impl App for GitApp {
    fn title(&self) -> &'static str {
        "GIT"
    }
    fn icon(&self) -> Icon {
        Icon::Git
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "\u{2191}\u{2193} move \u{b7} Enter open \u{b7} / filter \u{b7} c connections \u{b7} C clone \u{b7} r refresh \u{b7} Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        vec![
            "Up, Down, PageUp, PageDown, Home, End: move in the list that has the focus (Tab changes it in a repository)".into(),
            "Enter: open the repository, folder, file, commit or branch \u{b7} Esc or Backspace: back".into(),
            "/: filter the list \u{b7} o: sort by the next column \u{b7} O: reverse \u{b7} m: load more repositories".into(),
            "c: choose the connection (a host, or the clones on this computer) \u{b7} a: add a connection".into(),
            "In a repository: 1 Files \u{b7} 2 Commits \u{b7} 3 Branches \u{b7} 4 Issues and pull requests \u{b7} 5 Info \u{b7} g branch or tag".into(),
            "Files: Right opens a folder, Left closes it \u{b7} Commits: Enter shows the message and the diff".into(),
            "C clone \u{b7} I open in the AI IDE \u{b7} F fetch \u{b7} P pull \u{b7} U push (each asks first) \u{b7} y copy the clone address \u{b7} w open the web page".into(),
            "r: refresh (also ends a wait) \u{b7} L: the log of problems with the hosts (c copies it) \u{b7} c copies what the viewer shows".into(),
            "Nothing is changed on a host from here: it is read-only. Issues and pull requests are read-only too.".into(),
        ]
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.handle_event(ev, cx)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn wants_redraw(&mut self) -> bool {
        self.tip.take_changed()
    }
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        vec![crate::ctxmenu::Item::new("git.copy", "Copy what the viewer shows", "").enabled(self.view.as_ref().is_some_and(|v| v.copy_text().is_some()))]
    }
    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> Flow {
        if id == "git.copy" {
            self.copy_view_text();
        }
        Flow::Stay
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}

#[cfg(test)]
#[path = "git_tile_tests.rs"]
mod tests;
