//! FLOWS assist on disk: the suggestions of a flow (a proposed flow, its diff, reason, QA and trial result, guard lines), the little
//! book that holds the counters (what was drafted, published and spent today, a pause, a cool-down, the version being watched) and
//! the account-wide switches. No secrets are written: a suggestion holds a flow (which holds references, never values), a diff in
//! text, and short reasons already cleaned by the caller.
//!
//! ```text
//! <flows>/<flow-id>/assist.json                   the book
//! <flows>/<flow-id>/suggestions/<id>.json         one suggestion each
//! <flows>/_assist.json                            account-wide: kill switch, publish cap, publishes today
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::assist::{Class, TrialState};
use crate::model::*;
use crate::store::NowFn;

/// Layout of the assist files (see `iyakki-schema`).
pub const ASSIST_SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "flow assist", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "first layout", run: |_| {} }] };

const DAY_MS: i64 = 86_400_000;
/// The most suggestions kept per flow (the oldest decided ones go first).
const KEEP: usize = 40;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting for the person.
    #[default]
    Pending,
    /// The person pressed Approve: the change is in the draft and the publish question is open.
    Loaded,
    /// The person published it.
    Approved,
    /// Autopilot published it within its limits.
    AutoPublished,
    /// The person said no.
    Rejected,
    /// The flow moved on (another version was published or the run was fixed another way).
    Superseded,
    /// Autopilot published it and then rolled it back.
    RolledBack,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pending => "waiting for you",
            Status::Loaded => "approved, publish next",
            Status::Approved => "published by you",
            Status::AutoPublished => "published by autopilot",
            Status::Rejected => "rejected",
            Status::Superseded => "out of date",
            Status::RolledBack => "rolled back",
        }
    }
    pub fn open(self) -> bool {
        matches!(self, Status::Pending | Status::Loaded)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Suggestion {
    #[serde(default)]
    pub schema: u32,
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
    pub id: String,
    pub flow_id: String,
    pub class: Class,
    #[serde(default)]
    pub status: Status,
    pub created_ms: i64,
    #[serde(default)]
    pub decided_ms: i64,
    /// The published version the change was made from.
    pub base_version: u32,
    /// The failed run it answers (empty for an upgrade or an on-demand look).
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub node: String,
    /// The agent's diagnosis or the reason for the upgrade, short and redacted.
    pub reason: String,
    /// One line: what changes.
    pub summary: String,
    /// The diff as text lines (stored so the person sees exactly what would be published).
    pub diff: Vec<String>,
    /// The whole proposed flow (the draft the person approves).
    pub proposed: Flow,
    /// Content hash of `proposed` (to see that the draft still is this).
    pub hash: String,
    #[serde(default)]
    pub qa: Vec<String>,
    #[serde(default)]
    pub trial: TrialState,
    /// (guard, passed, reason)
    #[serde(default)]
    pub guards: Vec<(String, bool, String)>,
    /// Why this is a question and not an automatic publish (empty when it was published by itself).
    #[serde(default)]
    pub asks: String,
    pub by: String,
    #[serde(default)]
    pub cents: u64,
    #[serde(default)]
    pub published_version: u32,
}

/// What a flow's counters remember.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub at_ms: i64,
    /// `draft`, `publish`, `rollback`, `spend`, `pause`, `resume`, `note`.
    pub kind: String,
    #[serde(default)]
    pub cents: u64,
    #[serde(default)]
    pub note: String,
}

/// A version the agent published, being watched for failures in its first runs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Watch {
    pub version: u32,
    /// The version to go back to.
    pub previous: u32,
    pub since_ms: i64,
    pub suggestion: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Book {
    #[serde(default)]
    pub schema: u32,
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub cooldown_until_ms: i64,
    #[serde(default)]
    pub next: u32,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub watch: Option<Watch>,
    /// Failed runs already looked at (so each failure is answered once).
    #[serde(default)]
    pub seen_runs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Global {
    #[serde(default)]
    pub schema: u32,
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
    /// Off: no agent works on any flow by itself.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Publishes by the agent per day across all flows.
    #[serde(default = "three")]
    pub max_publish_per_day: u32,
    /// Publish times (ms) of the last day, across flows.
    #[serde(default)]
    pub publishes: Vec<i64>,
}

fn yes() -> bool {
    true
}
fn three() -> u32 {
    3
}

impl Default for Global {
    fn default() -> Global {
        Global { schema: 0, extra: Default::default(), enabled: true, max_publish_per_day: 3, publishes: vec![] }
    }
}

/// The store of suggestions and counters under the flows folder.
#[derive(Clone)]
pub struct AssistStore {
    root: PathBuf,
    now: NowFn,
}

fn read<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    if !path.is_file() {
        return T::default();
    }
    iyakki_schema::load_typed::<T>(&ASSIST_SCHEMA, path, iyakki_schema::now_secs()).1
}

impl AssistStore {
    pub fn new(root: &Path, now: NowFn) -> AssistStore {
        AssistStore { root: root.to_path_buf(), now }
    }

    pub fn from_fn(root: &Path, f: impl Fn() -> i64 + Send + Sync + 'static) -> AssistStore {
        AssistStore::new(root, Arc::new(f))
    }

    pub fn now(&self) -> i64 {
        (self.now)()
    }

    fn flow_dir(&self, flow: &str) -> Result<PathBuf, String> {
        if !valid_id(flow) {
            return Err(format!("`{flow}` is not a flow id"));
        }
        Ok(self.root.join(flow))
    }

    // ------------------------------------------------------------------------------------------ global

    pub fn global(&self) -> Global {
        read(&self.root.join("_assist.json"))
    }

    fn save_global(&self, g: &Global) -> Result<(), String> {
        let _ = std::fs::create_dir_all(&self.root);
        iyakki_schema::save_typed(&ASSIST_SCHEMA, &self.root.join("_assist.json"), g, 0o644)
    }

    /// The kill switch (and the account-wide cap).
    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.global();
        g.enabled = on;
        self.save_global(&g)
    }

    pub fn set_global_cap(&self, per_day: u32) -> Result<(), String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.global();
        g.max_publish_per_day = per_day.min(50);
        self.save_global(&g)
    }

    pub fn publishes_today_all(&self) -> u32 {
        let since = self.now() - DAY_MS;
        self.global().publishes.iter().filter(|t| **t > since).count() as u32
    }

    // ------------------------------------------------------------------------------------------ the book

    pub fn book(&self, flow: &str) -> Book {
        self.flow_dir(flow).map(|d| read(&d.join("assist.json"))).unwrap_or_default()
    }

    fn save_book(&self, flow: &str, b: &Book) -> Result<(), String> {
        let d = self.flow_dir(flow)?;
        if !d.is_dir() {
            return Err(format!("there is no flow `{flow}`"));
        }
        iyakki_schema::save_typed(&ASSIST_SCHEMA, &d.join("assist.json"), b, 0o644)
    }

    /// Change the book under the lock.
    pub fn with_book<R>(&self, flow: &str, f: impl FnOnce(&mut Book) -> R) -> Result<R, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut b = self.book(flow);
        let r = f(&mut b);
        // the event list stays small
        if b.events.len() > 200 {
            let cut = b.events.len() - 200;
            b.events.drain(..cut);
        }
        if b.seen_runs.len() > 200 {
            let cut = b.seen_runs.len() - 200;
            b.seen_runs.drain(..cut);
        }
        self.save_book(flow, &b)?;
        Ok(r)
    }

    pub fn record(&self, flow: &str, kind: &str, cents: u64, note: &str) -> Result<(), String> {
        let at = self.now();
        let note: String = note.chars().take(200).collect();
        let kind = kind.to_string();
        self.with_book(flow, move |b| b.events.push(Event { at_ms: at, kind, cents, note }))
    }

    pub fn count_since(&self, flow: &str, kind: &str, since_ms: i64) -> u32 {
        self.book(flow).events.iter().filter(|e| e.kind == kind && e.at_ms > since_ms).count() as u32
    }

    pub fn drafts_today(&self, flow: &str) -> u32 {
        self.count_since(flow, "draft", self.now() - DAY_MS)
    }

    pub fn publishes_today(&self, flow: &str) -> u32 {
        self.count_since(flow, "publish", self.now() - DAY_MS)
    }

    pub fn cents_today(&self, flow: &str) -> u64 {
        let since = self.now() - DAY_MS;
        self.book(flow).events.iter().filter(|e| e.at_ms > since).map(|e| e.cents).sum()
    }

    /// A publish by the agent: counted for the flow and for the account.
    pub fn record_publish(&self, flow: &str, note: &str) -> Result<(), String> {
        self.record(flow, "publish", 0, note)?;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.global();
        let now = self.now();
        g.publishes.retain(|t| *t > now - DAY_MS);
        g.publishes.push(now);
        self.save_global(&g)
    }

    pub fn paused(&self, flow: &str) -> bool {
        self.book(flow).paused
    }

    pub fn set_paused(&self, flow: &str, on: bool) -> Result<(), String> {
        let at = self.now();
        self.with_book(flow, move |b| {
            b.paused = on;
            b.events.push(Event { at_ms: at, kind: if on { "pause" } else { "resume" }.into(), cents: 0, note: String::new() });
        })
    }

    pub fn cooling(&self, flow: &str) -> bool {
        self.book(flow).cooldown_until_ms > self.now()
    }

    pub fn start_cooldown(&self, flow: &str, hours: u32) -> Result<(), String> {
        let until = self.now() + hours as i64 * 3_600_000;
        self.with_book(flow, move |b| b.cooldown_until_ms = until)
    }

    pub fn watch(&self, flow: &str) -> Option<Watch> {
        self.book(flow).watch
    }

    pub fn set_watch(&self, flow: &str, w: Option<Watch>) -> Result<(), String> {
        self.with_book(flow, move |b| b.watch = w)
    }

    pub fn flows_watched(&self, ids: &[String]) -> Vec<(String, Watch)> {
        ids.iter().filter_map(|id| self.watch(id).map(|w| (id.clone(), w))).collect()
    }

    // ------------------------------------------------------------------------------------------ suggestions

    fn sdir(&self, flow: &str) -> Result<PathBuf, String> {
        Ok(self.flow_dir(flow)?.join("suggestions"))
    }

    /// Keep a new suggestion; returns it with its id.
    pub fn add(&self, mut s: Suggestion) -> Result<Suggestion, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.sdir(&s.flow_id)?;
        if !self.flow_dir(&s.flow_id)?.is_dir() {
            return Err(format!("there is no flow `{}`", s.flow_id));
        }
        let mut b = self.book(&s.flow_id);
        b.next += 1;
        s.id = format!("s{}", b.next);
        self.save_book(&s.flow_id, &b)?;
        let _ = std::fs::create_dir_all(&dir);
        iyakki_schema::save_typed(&ASSIST_SCHEMA, &dir.join(format!("{}.json", s.id)), &s, 0o644)?;
        self.trim(&s.flow_id);
        Ok(s)
    }

    fn trim(&self, flow: &str) {
        let mut all = self.list_unlocked(flow);
        if all.len() <= KEEP {
            return;
        }
        all.sort_by_key(|s| (s.status.open(), s.created_ms));
        let extra = all.len() - KEEP;
        if let Ok(d) = self.sdir(flow) {
            for s in all.into_iter().filter(|s| !s.status.open()).take(extra) {
                let _ = std::fs::remove_file(d.join(format!("{}.json", s.id)));
            }
        }
    }

    fn list_unlocked(&self, flow: &str) -> Vec<Suggestion> {
        let Ok(d) = self.sdir(flow) else { return vec![] };
        let mut v: Vec<Suggestion> = std::fs::read_dir(d)
            .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).filter_map(|e| std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Suggestion>(&t).ok())).collect())
            .unwrap_or_default();
        v.sort_by_key(|s| std::cmp::Reverse(s.created_ms));
        v
    }

    /// Newest first.
    pub fn list(&self, flow: &str) -> Vec<Suggestion> {
        self.list_unlocked(flow)
    }

    pub fn open_list(&self, flow: &str) -> Vec<Suggestion> {
        self.list(flow).into_iter().filter(|s| s.status.open()).collect()
    }

    pub fn get(&self, flow: &str, id: &str) -> Option<Suggestion> {
        if !valid_id(id) {
            return None;
        }
        let p = self.sdir(flow).ok()?.join(format!("{id}.json"));
        std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    pub fn for_run(&self, flow: &str, run: &str) -> Option<Suggestion> {
        if run.is_empty() {
            return None;
        }
        self.list(flow).into_iter().find(|s| s.run_id == run)
    }

    /// Change one suggestion under the lock.
    pub fn update(&self, flow: &str, id: &str, f: impl FnOnce(&mut Suggestion)) -> Result<Suggestion, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.get(flow, id).ok_or_else(|| format!("there is no suggestion `{id}`"))?;
        f(&mut s);
        let p = self.sdir(flow)?.join(format!("{id}.json"));
        iyakki_schema::save_typed(&ASSIST_SCHEMA, &p, &s, 0o644)?;
        Ok(s)
    }

    pub fn set_status(&self, flow: &str, id: &str, st: Status) -> Result<Suggestion, String> {
        let at = self.now();
        self.update(flow, id, move |s| {
            s.status = st;
            s.decided_ms = at;
        })
    }

    /// Mark every open suggestion of the flow that was made from an older version as out of date.
    pub fn supersede_older(&self, flow: &str, published: u32) -> usize {
        let mut n = 0;
        for s in self.open_list(flow) {
            if s.base_version < published && self.set_status(flow, &s.id, Status::Superseded).is_ok() {
                n += 1;
            }
        }
        n
    }
}
