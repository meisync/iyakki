//! The data of flows and runs. Everything here is `serde`, so a remote backend only has to carry these types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::retry::RetryPolicy;

// ------------------------------------------------------------------------------------------------ flow

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Flow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub inputs: Vec<Param>,
    #[serde(default)]
    pub outputs: Vec<Param>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
    /// A production flow never gets an automatic fix.
    #[serde(default)]
    pub production: bool,
    #[serde(default)]
    pub meta: Meta,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Param {
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub description: String,
}

/// A change somebody made to a draft since the last publish (the "agent changed" badge).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Change {
    /// `user:<name>` or `agent:<name>`.
    pub who: String,
    #[serde(default)]
    pub node: String,
    pub summary: String,
    pub at_ms: i64,
    /// A heal fix the harness classed as simple.
    #[serde(default)]
    pub simple: bool,
}

impl Change {
    pub fn by_agent(&self) -> bool {
        self.who.starts_with("agent:")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Meta {
    /// The published version this file is (0 for a draft that was never published).
    #[serde(default)]
    pub version: u32,
    /// The published version the draft was started from.
    #[serde(default)]
    pub base_version: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub changelog: String,
    #[serde(default)]
    pub modified_by: Vec<Change>,
    #[serde(default)]
    pub hash: String,
}

/// Where this flow's sandbox Jobs run: the namespace (empty: `flows-<user>`, or `workspace:<name>` for `<name>-dev`) and the default image.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Sandbox {
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub image: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Defaults {
    #[serde(default = "d_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub heal: HealPolicy,
    #[serde(default)]
    pub runner: RunnerKind,
    #[serde(default = "d_par")]
    pub max_parallel: usize,
    #[serde(default = "d_conc")]
    pub max_concurrent_runs: usize,
    #[serde(default = "d_keep")]
    pub keep_runs: usize,
    #[serde(default)]
    pub sandbox: Sandbox,
    /// Browser nodes: how the run's browser session is made and approved.
    #[serde(default, skip_serializing_if = "BrowserDefaults::is_default")]
    pub browser: BrowserDefaults,
    /// Camera nodes: the person's explicit permission (nothing here is ever set by the assistant or arrives with an import).
    #[serde(default, skip_serializing_if = "CameraDefaults::is_default")]
    pub camera: CameraDefaults,
    /// Whether and how far a configured agent may fix and upgrade this flow by itself (see `assist`). Off unless the person sets it.
    #[serde(default, skip_serializing_if = "AssistPolicy::is_default")]
    pub assist: AssistPolicy,
}

impl CameraDefaults {
    pub fn is_default(&self) -> bool {
        *self == CameraDefaults::default()
    }
}

/// Settings of a flow's camera node (`cam_photo`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CameraDefaults {
    /// The person allows this flow to take pictures with the camera. Off: the camera step fails with a plain message.
    #[serde(default)]
    pub allowed: bool,
    /// Also when nobody started the run by hand (a schedule, an event, a file change). Off: only runs the person starts.
    #[serde(default)]
    pub unattended: bool,
    /// The person said "Block" to a permission request: the flow stops asking (switch it back in the flow's settings).
    #[serde(default)]
    pub blocked: bool,
}

impl BrowserDefaults {
    pub fn is_default(&self) -> bool {
        *self == BrowserDefaults::default()
    }
}

/// Settings of a flow's browser nodes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct BrowserDefaults {
    /// The flow is trusted: a run starts its browser session without asking (the host policy and the budgets still apply).
    #[serde(default)]
    pub trusted: bool,
    /// `isolated` (default: a fresh profile that is thrown away) or `person` (the person's own browser profile; asked every run).
    #[serde(default)]
    pub profile: String,
    /// Most browser actions and minutes in one run (0: the account's limits).
    #[serde(default)]
    pub max_actions: u32,
    #[serde(default)]
    pub max_minutes: u32,
}

fn d_timeout() -> u64 {
    30_000
}
fn d_par() -> usize {
    4
}
fn d_conc() -> usize {
    1
}
fn d_keep() -> usize {
    200
}

impl Default for Defaults {
    fn default() -> Defaults {
        Defaults { timeout_ms: d_timeout(), retry: RetryPolicy::default(), heal: HealPolicy::default(), runner: RunnerKind::Local, max_parallel: d_par(), max_concurrent_runs: d_conc(), keep_runs: d_keep(), sandbox: Sandbox::default(), browser: BrowserDefaults::default(), camera: CameraDefaults::default(), assist: AssistPolicy::default() }
    }
}

/// How far the configured agent works on this flow. A file written before this setting existed has none and loads as `Off`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssistMode {
    /// The agent does nothing on its own (the person can still ask it by hand).
    #[default]
    Off,
    /// The agent drafts fixes and upgrades and shows them; the person approves and publishes.
    Copilot,
    /// The agent drafts and tests by itself and, only when `auto_publish` is on and every guard passes, publishes (and tells the person).
    Autopilot,
}

impl AssistMode {
    pub fn label(self) -> &'static str {
        match self {
            AssistMode::Off => "off",
            AssistMode::Copilot => "copilot",
            AssistMode::Autopilot => "autopilot",
        }
    }
    pub fn parse(s: &str) -> Option<AssistMode> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" => AssistMode::Off,
            "copilot" | "co-pilot" => AssistMode::Copilot,
            "autopilot" | "auto-pilot" => AssistMode::Autopilot,
            _ => return None,
        })
    }
}

/// The per-flow `assist` setting: the mode, what the agent may work on and the limits it works within.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistPolicy {
    #[serde(default)]
    pub mode: AssistMode,
    /// Autopilot may publish a tested fix by itself. Off: autopilot drafts and tests, and publishing asks. Never set by the assistant.
    #[serde(default)]
    pub auto_publish: bool,
    /// Classes of work the agent may do on this flow.
    #[serde(default = "a_yes")]
    pub allow_fix: bool,
    #[serde(default)]
    pub allow_upgrade: bool,
    /// Publishes by the agent per flow per day, and drafts it may make per flow per day.
    #[serde(default = "a_pub")]
    pub max_publish_per_day: u32,
    #[serde(default = "a_drafts")]
    pub max_drafts_per_day: u32,
    /// What the agent may spend on this flow, per day and per fix, in cents.
    #[serde(default = "a_day_cents")]
    pub max_cents_per_day: u64,
    #[serde(default = "a_fix_cents")]
    pub max_cents_per_fix: u64,
    /// The most steps and the most changed settings a change may touch to be published without asking.
    #[serde(default = "a_nodes")]
    pub max_diff_nodes: u32,
    #[serde(default = "a_fields")]
    pub max_diff_fields: u32,
    /// A version the agent published is rolled back when one of its first this-many runs fails.
    #[serde(default = "a_watch")]
    pub watch_runs: u32,
    /// After a rollback autopilot pauses (and asks) for this many hours.
    #[serde(default = "a_cool")]
    pub cooldown_hours: u32,
}

fn a_yes() -> bool {
    true
}
fn a_pub() -> u32 {
    1
}
fn a_drafts() -> u32 {
    4
}
fn a_day_cents() -> u64 {
    50
}
fn a_fix_cents() -> u64 {
    10
}
fn a_nodes() -> u32 {
    2
}
fn a_fields() -> u32 {
    10
}
fn a_watch() -> u32 {
    3
}
fn a_cool() -> u32 {
    24
}

impl Default for AssistPolicy {
    fn default() -> AssistPolicy {
        AssistPolicy { mode: AssistMode::Off, auto_publish: false, allow_fix: true, allow_upgrade: false, max_publish_per_day: a_pub(), max_drafts_per_day: a_drafts(), max_cents_per_day: a_day_cents(), max_cents_per_fix: a_fix_cents(), max_diff_nodes: a_nodes(), max_diff_fields: a_fields(), watch_runs: a_watch(), cooldown_hours: a_cool() }
    }
}

impl AssistPolicy {
    pub fn is_default(&self) -> bool {
        *self == AssistPolicy::default()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum RunnerKind {
    #[default]
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "k3s-job")]
    K3sJob,
    #[serde(rename = "remote")]
    Remote,
}

impl RunnerKind {
    pub fn label(self) -> &'static str {
        match self {
            RunnerKind::Local => "local",
            RunnerKind::K3sJob => "sandbox (k3s)",
            RunnerKind::Remote => "remote",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HealMode {
    #[default]
    Off,
    Suggest,
    AutoFixSimple,
    AlwaysAsk,
}

impl HealMode {
    pub fn label(self) -> &'static str {
        match self {
            HealMode::Off => "off",
            HealMode::Suggest => "suggest",
            HealMode::AutoFixSimple => "auto-fix simple",
            HealMode::AlwaysAsk => "always ask",
        }
    }
    /// How much the agent may do on its own (for "widening autonomy asks").
    pub fn autonomy(self) -> u8 {
        match self {
            HealMode::Off => 0,
            HealMode::AlwaysAsk => 1,
            HealMode::Suggest => 2,
            HealMode::AutoFixSimple => 3,
        }
    }
    pub fn parse(s: &str) -> Option<HealMode> {
        Some(match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "off" => HealMode::Off,
            "suggest" => HealMode::Suggest,
            "auto_fix_simple" | "auto" | "autofixsimple" => HealMode::AutoFixSimple,
            "always_ask" | "ask" | "alwaysask" => HealMode::AlwaysAsk,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HealPolicy {
    #[serde(default)]
    pub mode: HealMode,
    #[serde(default = "h_run")]
    pub max_per_run: u32,
    #[serde(default = "h_day")]
    pub max_per_day: u32,
    #[serde(default = "h_cost")]
    pub cost_cap_cents: u64,
    /// Publish an agent's draft without asking when every change since the last publish was a simple fix. Off by default.
    #[serde(default)]
    pub auto_publish_simple: bool,
}

fn h_run() -> u32 {
    2
}
fn h_day() -> u32 {
    6
}
fn h_cost() -> u64 {
    50
}

impl Default for HealPolicy {
    fn default() -> HealPolicy {
        HealPolicy { mode: HealMode::Off, max_per_run: h_run(), max_per_day: h_day(), cost_cap_cents: h_cost(), auto_publish_simple: false }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Track {
    #[default]
    #[serde(rename = "pinned")]
    Pinned,
    #[serde(rename = "latest-published")]
    LatestPublished,
    #[serde(rename = "compatible")]
    Compatible,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    #[default]
    Fail,
    Continue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LibRef {
    pub id: String,
    #[serde(default)]
    pub version: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub runner: Option<RunnerKind>,
    #[serde(default)]
    pub heal: Option<HealPolicy>,
    #[serde(default)]
    pub track: Track,
    #[serde(default)]
    pub lib: Option<LibRef>,
    #[serde(default)]
    pub pos: Pos,
    #[serde(default)]
    pub on_error: OnError,
    /// The last change an agent made to this node since the last publish.
    #[serde(default)]
    pub modified_by: Option<Change>,
}

impl Node {
    pub fn new(id: &str, kind: &str, config: Value) -> Node {
        Node { id: id.into(), kind: kind.into(), name: String::new(), config, retry: None, timeout_ms: None, runner: None, heal: None, track: Track::Pinned, lib: None, pos: Pos::default(), on_error: OnError::Fail, modified_by: None }
    }
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    pub fn at(mut self, x: f32, y: f32) -> Node {
        self.pos = Pos { x, y };
        self
    }
    /// What counts for "this node is the same": everything except where it sits and who touched it.
    pub fn same_definition(&self, other: &Node) -> bool {
        let strip = |n: &Node| {
            let mut c = n.clone();
            c.pos = Pos::default();
            c.modified_by = None;
            c
        };
        strip(self) == strip(other)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub from: String,
    #[serde(default = "out_port")]
    pub port: String,
    pub to: String,
    #[serde(default)]
    pub when: String,
}

fn out_port() -> String {
    "out".into()
}

impl Edge {
    pub fn new(from: &str, to: &str) -> Edge {
        Edge { from: from.into(), port: "out".into(), to: to.into(), when: String::new() }
    }
    pub fn port(mut self, p: &str) -> Edge {
        self.port = p.into();
        self
    }
    pub fn when(mut self, w: &str) -> Edge {
        self.when = w.into();
        self
    }
    pub fn key(&self) -> String {
        format!("{}:{}>{}", self.from, self.port, self.to)
    }
}

impl Flow {
    pub fn new(id: &str, name: &str) -> Flow {
        Flow { id: id.into(), name: name.into(), ..Default::default() }
    }
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    pub fn incoming(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.to == id).collect()
    }
    pub fn outgoing(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.from == id).collect()
    }
    /// Ids of the nodes downstream of `id` (not including it).
    pub fn descendants(&self, id: &str) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut stack = vec![id.to_string()];
        while let Some(n) = stack.pop() {
            for e in self.edges.iter().filter(|e| e.from == n) {
                if !seen.contains(&e.to) && e.to != id {
                    seen.push(e.to.clone());
                    stack.push(e.to.clone());
                }
            }
        }
        seen
    }
    /// The node's effective retry policy: its own, else the flow's default.
    pub fn retry_of(&self, n: &Node) -> RetryPolicy {
        n.retry.clone().unwrap_or_else(|| self.defaults.retry.clone())
    }
    pub fn timeout_of(&self, n: &Node) -> u64 {
        n.timeout_ms.unwrap_or(self.defaults.timeout_ms)
    }
    pub fn heal_of(&self, n: &Node) -> HealPolicy {
        n.heal.clone().unwrap_or_else(|| self.defaults.heal.clone())
    }
    pub fn runner_of(&self, n: &Node) -> RunnerKind {
        n.runner.unwrap_or(self.defaults.runner)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Result<Flow, String> {
        serde_json::from_str(s).map_err(|e| format!("not a flow: {e}"))
    }
    /// A stable hash of the content, ignoring the version metadata's own hash field.
    pub fn content_hash(&self) -> String {
        let mut c = self.clone();
        c.meta.hash.clear();
        format!("{:016x}", crate::retry::hash_parts(&[&serde_json::to_string(&c).unwrap_or_default()]))
    }
}

// ------------------------------------------------------------------------------------------------ triggers

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerKind {
    Manual,
    Chat,
    Cron {
        expr: String,
        #[serde(default = "local_tz")]
        tz: String,
    },
    Interval {
        every_s: u64,
        #[serde(default)]
        anchor_ms: Option<i64>,
    },
    Once {
        at_ms: i64,
    },
    /// A CALENDAR event whose title contains `matching` starts.
    Calendar {
        matching: String,
    },
    FileChange {
        path: String,
    },
    OnFailure {
        flow: String,
    },
    Event {
        name: String,
    },
    /// Planned: needs an authenticated local endpoint.
    Webhook {
        path: String,
    },
}

fn local_tz() -> String {
    "local".into()
}

impl TriggerKind {
    pub fn label(&self) -> &'static str {
        match self {
            TriggerKind::Manual => "manual",
            TriggerKind::Chat => "chat",
            TriggerKind::Cron { .. } => "cron",
            TriggerKind::Interval { .. } => "interval",
            TriggerKind::Once { .. } => "once",
            TriggerKind::Calendar { .. } => "calendar",
            TriggerKind::FileChange { .. } => "file change",
            TriggerKind::OnFailure { .. } => "on failure",
            TriggerKind::Event { .. } => "event",
            TriggerKind::Webhook { .. } => "webhook (planned)",
        }
    }
    pub fn is_timed(&self) -> bool {
        matches!(self, TriggerKind::Cron { .. } | TriggerKind::Interval { .. } | TriggerKind::Once { .. })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    #[default]
    Skip,
    Queue,
    Allow,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Missed {
    Skip,
    RunOnce,
    CatchUp { max: u32 },
}

impl Default for Missed {
    fn default() -> Missed {
        Missed::RunOnce
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Trigger {
    pub id: String,
    #[serde(flatten)]
    pub kind: TriggerKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub missed: Missed,
    #[serde(default)]
    pub overlap: Overlap,
    #[serde(default)]
    pub jitter_s: u64,
    /// Inputs given to the run this trigger starts.
    #[serde(default)]
    pub inputs: Value,
}

fn yes() -> bool {
    true
}

impl Trigger {
    pub fn new(id: &str, kind: TriggerKind) -> Trigger {
        Trigger { id: id.into(), kind, enabled: true, missed: Missed::default(), overlap: Overlap::default(), jitter_s: 0, inputs: Value::Null }
    }
}

// ------------------------------------------------------------------------------------------------ run

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Scheduled,
    Queued,
    Running,
    Blocked,
    Retrying,
    Healing,
    Failed,
    Succeeded,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, RunStatus::Failed | RunStatus::Succeeded | RunStatus::Cancelled)
    }
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Scheduled => "scheduled",
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Blocked => "needs you",
            RunStatus::Retrying => "retrying",
            RunStatus::Healing => "healing",
            RunStatus::Failed => "failed",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Cancelled => "cancelled",
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, RunStatus::Running | RunStatus::Retrying | RunStatus::Healing | RunStatus::Queued)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    #[default]
    Pending,
    Running,
    Retrying,
    /// Waiting for a person (an approval) or a time.
    Waiting,
    Succeeded,
    Failed,
    Skipped,
    /// The program restarted while this ran and it could not be resumed safely.
    Interrupted,
    Cancelled,
}

impl NodeState {
    pub fn is_done(self) -> bool {
        matches!(self, NodeState::Succeeded | NodeState::Failed | NodeState::Skipped | NodeState::Cancelled)
    }
    pub fn label(self) -> &'static str {
        match self {
            NodeState::Pending => "pending",
            NodeState::Running => "running",
            NodeState::Retrying => "retrying",
            NodeState::Waiting => "waiting",
            NodeState::Succeeded => "done",
            NodeState::Failed => "failed",
            NodeState::Skipped => "skipped",
            NodeState::Interrupted => "interrupted",
            NodeState::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Attempt {
    pub n: u32,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub timeout_ms: u64,
    pub delay_before_ms: u64,
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    /// `retryable` or `permanent` for a failure.
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub output_preview: String,
    #[serde(default)]
    pub logs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct NodeRun {
    #[serde(default)]
    pub state: NodeState,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// How many attempts this node may make (for "2/4").
    #[serde(default)]
    pub max_attempts: u32,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub output: Value,
    /// The output port taken (`out`, `true`, `error`, a case).
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub ended_ms: i64,
    #[serde(default)]
    pub next_retry_ms: Option<i64>,
    #[serde(default)]
    pub wake_ms: Option<i64>,
    /// What a waiting node waits for.
    #[serde(default)]
    pub waiting: String,
    /// The step failed on a permission only the person can give (the screen offers the flow's settings, never the fixing agent).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_person: bool,
    /// Which permission that was (`camera`, `camera_unattended`; empty when it cannot be asked for).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permission: String,
    #[serde(default)]
    pub log: Vec<String>,
    /// The node definition changed under this run (a patch or a pulled update) before this node ran: its version note.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeState {
    #[default]
    Open,
    Taken,
    Dead,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct TriggerInfo {
    pub kind: String,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub detail: String,
    /// The tick that was due (for schedules), to show "for 14:05".
    #[serde(default)]
    pub due_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct LogLine {
    pub t_ms: i64,
    #[serde(default)]
    pub node: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// Waiting for a person.
    Proposed,
    Applied,
    Verified,
    Rejected,
    /// Applied, but the test or rerun failed.
    Failed,
    /// The agent is working on it.
    Investigating,
}

/// What the person sees for one proposed fix.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HealCard {
    pub id: String,
    pub node: String,
    pub state: CardState,
    pub diagnosis: String,
    pub patch: Option<crate::heal::Patch>,
    pub class: Option<crate::heal::Classification>,
    pub created_ms: i64,
    #[serde(default)]
    pub decided_by: String,
    #[serde(default)]
    pub note: String,
    /// The draft version the fix made (0: none yet).
    #[serde(default)]
    pub draft_rev: u32,
    #[serde(default)]
    pub cost_cents: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppliedPatch {
    pub card: String,
    pub node: String,
    pub by: String,
    pub at_ms: i64,
    pub summary: String,
    pub simple: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Run {
    /// Permissions the person gave THIS run only ("Allow this time"): `camera`, `camera_unattended`. They live on the run and never change the flow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<String>,
    pub id: String,
    pub flow_id: String,
    /// The flow as it was when the run began (the pinned version), with the node updates and patches applied since.
    pub flow: Flow,
    pub trigger: TriggerInfo,
    pub status: RunStatus,
    #[serde(default)]
    pub inputs: Value,
    #[serde(default)]
    pub outputs: Value,
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeRun>,
    #[serde(default)]
    pub edges: BTreeMap<String, EdgeState>,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub scheduled_ms: Option<i64>,
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub ended_ms: i64,
    #[serde(default)]
    pub error: String,
    /// Why the run needs you.
    #[serde(default)]
    pub blocked: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub heal: Vec<HealCard>,
    #[serde(default)]
    pub patches: Vec<AppliedPatch>,
    #[serde(default)]
    pub heal_spent: u32,
    #[serde(default)]
    pub heal_cents: u64,
    /// Node definitions waiting to be taken at the node's next boundary: node id to the new definition and where it came from.
    #[serde(default)]
    pub pending_updates: BTreeMap<String, PendingUpdate>,
    #[serde(default)]
    pub log: Vec<LogLine>,
    /// A counter that goes up with every change, so a screen knows when to redraw.
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub runner_note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingUpdate {
    pub node: Node,
    pub from_version: u32,
    pub to_version: u32,
    pub by: String,
}

impl Run {
    pub fn version(&self) -> u32 {
        self.flow.meta.version
    }
    pub fn duration_ms(&self, now_ms: i64) -> i64 {
        if self.started_ms == 0 {
            0
        } else if self.ended_ms > 0 {
            self.ended_ms - self.started_ms
        } else {
            (now_ms - self.started_ms).max(0)
        }
    }
    pub fn open_cards(&self) -> Vec<&HealCard> {
        self.heal.iter().filter(|c| c.state == CardState::Proposed || c.state == CardState::Investigating).collect()
    }
    pub fn summary(&self) -> RunSummary {
        let done = self.nodes.values().filter(|n| n.state.is_done()).count();
        RunSummary { id: self.id.clone(), flow_id: self.flow_id.clone(), flow_name: self.flow.name.clone(), version: self.version(), status: self.status, trigger: self.trigger.kind.clone(), created_ms: self.created_ms, scheduled_ms: self.scheduled_ms, started_ms: self.started_ms, ended_ms: self.ended_ms, nodes_done: done, nodes_total: self.nodes.len(), error: self.error.clone(), blocked: self.blocked.clone(), cards: self.open_cards().len(), rev: self.rev, parent: self.parent.clone() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunSummary {
    pub id: String,
    pub flow_id: String,
    pub flow_name: String,
    pub version: u32,
    pub status: RunStatus,
    pub trigger: String,
    pub created_ms: i64,
    pub scheduled_ms: Option<i64>,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub nodes_done: usize,
    pub nodes_total: usize,
    pub error: String,
    pub blocked: String,
    pub cards: usize,
    pub rev: u64,
    pub parent: Option<String>,
}

pub fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !s.starts_with('-')
}

/// A short id from a name: lowercase letters, digits and `-`.
pub fn slug(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').chars().take(40).collect::<String>();
    if s.is_empty() {
        "flow".into()
    } else {
        s
    }
}
