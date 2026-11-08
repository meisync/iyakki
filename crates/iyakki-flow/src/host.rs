//! The seam between the engine and the world. The engine never opens a socket, starts a program or reads a vault: it asks the
//! [`Host`]. The agent crate implements it with the real HTTP stack, the argument-array runner, the browser automation, the
//! assistant and the INBOX; tests implement it with [`FakeHost`].

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::RunnerKind;
use crate::retry::NodeError;

// ------------------------------------------------------------------------------------------------ cancel and clock

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    /// Wait `ms`, ending early when `cancel` is set. True when the whole wait passed.
    fn sleep_ms(&self, ms: u64, cancel: &Cancel) -> bool;
    /// A clock that only moves when told to (tests): the engine moves it forward itself when everything is waiting for time.
    fn is_virtual(&self) -> bool {
        false
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
    }
    fn sleep_ms(&self, ms: u64, cancel: &Cancel) -> bool {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(Duration::from_millis(25)));
        }
    }
}

/// A clock that only moves when somebody sleeps or advances it, so retries with a 10 minute backoff take no time in a test.
pub struct FakeClock {
    now: AtomicI64,
    pub slept: Mutex<Vec<u64>>,
}

impl FakeClock {
    pub fn new(start_ms: i64) -> Arc<FakeClock> {
        Arc::new(FakeClock { now: AtomicI64::new(start_ms), slept: Mutex::new(Vec::new()) })
    }
    pub fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
    pub fn set(&self, ms: i64) {
        self.now.store(ms, Ordering::SeqCst);
    }
    pub fn sleeps(&self) -> Vec<u64> {
        self.slept.lock().unwrap().clone()
    }
}

impl Clock for FakeClock {
    fn is_virtual(&self) -> bool {
        true
    }
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64, cancel: &Cancel) -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        self.slept.lock().unwrap().push(ms);
        self.now.fetch_add(ms as i64, Ordering::SeqCst);
        // a real moment passes so that other threads get to run
        std::thread::sleep(Duration::from_millis(1));
        true
    }
}

// ------------------------------------------------------------------------------------------------ redaction

/// Remembers the secrets a run resolved and removes them from anything that is stored or shown.
#[derive(Default)]
pub struct Redactor {
    secrets: Mutex<Vec<String>>,
}

impl Redactor {
    pub fn new() -> Redactor {
        Redactor::default()
    }
    pub fn add(&self, secret: &str) {
        if secret.chars().count() >= 3 {
            let mut g = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
            if !g.iter().any(|s| s == secret) {
                g.push(secret.to_string());
                g.sort_by_key(|s| std::cmp::Reverse(s.len()));
            }
        }
    }
    pub fn text(&self, s: &str) -> String {
        let g = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = s.to_string();
        for sec in g.iter() {
            if out.contains(sec.as_str()) {
                out = out.replace(sec.as_str(), "***");
            }
        }
        out
    }
    pub fn value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.value(x))).collect()),
            other => other.clone(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.secrets.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }
}

// ------------------------------------------------------------------------------------------------ requests

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TlsOpts {
    #[serde(default)]
    pub ca_file: String,
    #[serde(default)]
    pub insecure: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_ms: u64,
    pub tls: TlsOpts,
    pub follow_redirects: bool,
    /// Which run and node asked (for the audit line).
    pub run_id: String,
    pub node_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    /// Names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed_ms: u64,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    /// Relative to the flow's workspace; the host refuses anything that leaves it.
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub timeout_ms: u64,
    pub runner: RunnerKind,
    /// The container image for a k3s Job (empty: the default tools image).
    pub image: String,
    /// The namespace setting of the flow's sandbox (empty: the default; `workspace:<name>`).
    pub namespace: String,
    pub flow_id: String,
    pub run_id: String,
    pub node_id: String,
    /// A secret was used to build this request (an environment value or an argument): a sandbox Job would store it in plain text.
    pub secrets: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    pub missing: bool,
    /// Where it ran, for the log ("local", "job flow-x-y in namespace shop-dev").
    pub ran_in: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliInfo {
    pub installed: bool,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserRequest {
    /// Steps in the browser automation's own JSON language (see `docs/BROWSER-AGENT.md`).
    pub steps: Value,
    pub allow_domains: Vec<String>,
    /// `read_only` or `interactive`.
    pub level: String,
    /// `headless` or `ide`.
    pub target: String,
    pub timeout_ms: u64,
    /// `[{name, kind: title|url|text|regex, arg}]`.
    pub extract: Vec<(String, String, String)>,
    pub run_id: String,
    pub node_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserResult {
    pub report: String,
    pub extracted: BTreeMap<String, Value>,
    pub screenshots: Vec<String>,
    pub url: String,
}

/// One action of a browser node on the run's own browser session (`kinds/browser_qa.rs`). `args` can hold text a secret was
/// resolved into, so this type has no `Debug` that prints it.
#[derive(Clone, Default, PartialEq)]
pub struct BrowserOp {
    /// `open`, `screenshot`, `dom`, `assert`, `click`, `type`, `fill`, `submit`, `key`, `hover`, `scroll`, `select`, `wait`,
    /// `extract`, `js`, `frame`, `viewport`, `storage`, `download`, `compare`.
    pub op: String,
    pub args: Value,
    pub timeout_ms: u64,
    pub flow_id: String,
    pub run_id: String,
    pub node_id: String,
    pub node_title: String,
    /// The hosts the flow's Open URL nodes name (for the one question per run).
    pub hosts: Vec<String>,
    /// The flow is marked trusted: no question per run (the host policy still applies).
    pub trusted: bool,
    /// `isolated` (a fresh profile, thrown away) or `person` (the person's own browser profile, asked every run).
    pub profile: String,
    /// Values typed or set by this op that came from the vault: removed from everything the host returns.
    pub secrets: Vec<String>,
    /// This run's budgets: browser actions and minutes (0: the account's limits).
    pub max_actions: u32,
    pub max_minutes: u32,
}

impl std::fmt::Debug for BrowserOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BrowserOp({} in {}/{})", self.op, self.run_id, self.node_id)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserOpResult {
    pub value: Value,
    /// Files this op made (screenshots, diffs, downloads): shown in the run view and the report.
    pub artifacts: Vec<String>,
    /// Lines for the node's log.
    pub log: Vec<String>,
}

/// What the run view shows next to the "Browser session" badge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserSessionInfo {
    pub run_id: String,
    pub hosts: Vec<String>,
    pub profile: String,
    pub actions: u32,
    pub started_ms: i64,
    pub url: String,
    /// The latest live picture (a PNG file), if the engine can draw.
    pub preview: Option<String>,
    pub engine: String,
}

/// One picture from the camera for a `cam_photo` node. The engine has already checked the flow's camera permission; the host
/// checks it again and adds its own rules (the account, a camera that is there and free).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CamPhotoRequest {
    pub flow_id: String,
    pub run_id: String,
    pub node_id: String,
    /// A camera by name (empty: the one the shell uses).
    pub device: String,
    /// Keep the picture after the run (in the run's private folder); default is to delete it when the run ends.
    pub keep: bool,
    /// The person allowed this flow to use the camera, and whether also for runs nobody started by hand.
    pub allowed: bool,
    pub unattended: bool,
    /// The run was started by the person (by hand or from the chat).
    pub attended: bool,
    /// A later step of the flow shows this picture to a model: the host refuses before taking it when the chosen model cannot see
    /// pictures.
    pub needs_vision: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CamPhotoResult {
    /// The picture file (a JPEG in the run's private folder).
    pub path: String,
    /// Unix seconds.
    pub taken_at: i64,
    pub device: String,
    /// Lines for the node's log (for example the warning that the model is a cloud service).
    pub log: Vec<String>,
}

/// A step stopped for want of a permission only the person can give: the host asks them (popup, inbox) and never decides itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionNeed {
    pub flow_id: String,
    pub flow_name: String,
    pub run_id: String,
    pub node_id: String,
    pub node_title: String,
    /// `camera` or `camera_unattended`.
    pub permission: String,
    pub message: String,
    /// The flow already says "Block": do not ask.
    pub blocked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentRequest {
    pub prompt: String,
    /// Picture files (a screenshot from a browser node) shown to the model; it must be able to look at pictures.
    pub images: Vec<String>,
    pub tools: Vec<String>,
    pub output_schema: Value,
    pub agent: String,
    pub max_tokens: u64,
    pub max_cents: u64,
    pub timeout_ms: u64,
    pub run_id: String,
    pub node_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentResult {
    pub text: String,
    pub cents: u64,
    /// Lines for the node's log (which vision model looked at the picture, that it had to be loaded first).
    pub log: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptRequest {
    /// `js` or `wasm`.
    pub lang: String,
    pub source: String,
    pub package: String,
    pub args: Value,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// `info`, `warn`, `error`.
    pub level: String,
    pub flow_id: String,
    pub run_id: String,
    /// The note links to this run when it needs the person (`needs_you`).
    pub needs_you: bool,
}

/// A request to also tell the person through a messaging channel (today `matrix`). The engine only carries the shape and checks it;
/// the host does the sending (docs/development/MESSAGING-CONNECTORS-PLAN.md, section 7).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelNotice {
    /// Lower-case channel names other than `inbox`.
    pub channels: Vec<String>,
    /// The name of one of the person's saved rooms (empty: their only or main room).
    pub room: String,
    /// A picture of this run (the camera step's file); empty: none.
    pub attach: String,
    /// Render and check everything, send nothing.
    pub dry_run: bool,
    /// The same key never sends twice.
    pub dedupe_key: String,
}

/// What the healing agent is asked, already redacted and compact.
#[derive(Clone, Debug, PartialEq)]
pub struct HealRequest {
    pub run_id: String,
    pub flow_id: String,
    pub node_id: String,
    /// The failure context as text (node definition, last attempts, versions, the flow's shape).
    pub context: String,
    /// The agent to ask (empty: the built-in assistant).
    pub agent: String,
    pub max_cents: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HealProposal {
    pub diagnosis: String,
    pub patch: Option<crate::heal::Patch>,
    pub cents: u64,
}

// ------------------------------------------------------------------------------------------------ the trait

fn na<T>(what: &str) -> Result<T, NodeError> {
    Err(NodeError { message: format!("{what} is not available here"), config: true, ..Default::default() })
}

pub trait Host: Send + Sync {
    fn http(&self, _req: &HttpRequest, _cancel: &Cancel) -> Result<HttpResponse, NodeError> {
        na("HTTP")
    }
    /// Runs a command by the runner the request names (local, k3s-job, remote).
    fn exec(&self, _req: &ExecRequest, _cancel: &Cancel) -> Result<ExecResult, NodeError> {
        na("running commands")
    }
    fn cli_info(&self, _id: &str) -> Option<CliInfo> {
        None
    }
    /// The CLI is in the catalog (installed or not).
    fn cli_known(&self, id: &str) -> bool {
        self.cli_info(id).is_some()
    }
    /// Install or upgrade a catalog CLI to at least `min_version` (the host asks the person where its rules say so).
    fn cli_upgrade(&self, id: &str, _min_version: &str) -> Result<String, String> {
        Err(format!("cannot install {id} here"))
    }
    fn browser(&self, _req: &BrowserRequest, _cancel: &Cancel) -> Result<BrowserResult, NodeError> {
        na("the browser")
    }
    /// One action of a browser node. The session belongs to the run and is made on the first call.
    fn browser_op(&self, _req: &BrowserOp, _cancel: &Cancel) -> Result<BrowserOpResult, NodeError> {
        na("the browser session")
    }
    /// The run is over (finished, failed or cancelled): close its browser session.
    fn browser_end(&self, _run_id: &str) {}
    /// The run's open browser session, if there is one.
    fn browser_session(&self, _run_id: &str) -> Option<BrowserSessionInfo> {
        None
    }
    /// Take one picture with the camera into the run's private folder.
    fn cam_photo(&self, _req: &CamPhotoRequest, _cancel: &Cancel) -> Result<CamPhotoResult, NodeError> {
        na("the camera")
    }
    /// A step stopped for want of a permission the person can give by answering a request.
    fn permission_needed(&self, _n: &PermissionNeed) {}
    /// The run is over: delete its camera pictures (those of nodes that did not ask to keep them).
    fn cam_end(&self, _run_id: &str) {}
    fn agent(&self, _req: &AgentRequest, _cancel: &Cancel) -> Result<AgentResult, NodeError> {
        na("the assistant")
    }
    fn script(&self, _req: &ScriptRequest, _cancel: &Cancel) -> Result<Value, NodeError> {
        na("scripts")
    }
    fn notify(&self, _n: &Notice) -> Result<(), String> {
        Ok(())
    }
    /// A flow's `notify` step with its own `to` (who should get it: a word like `team` or `role:admin`, or a list of people; `Null` when the
    /// step names nobody). A desktop has one person, so the default ignores it; a cloud tenant sends it to the people it names.
    fn notify_to(&self, n: &Notice, _to: &Value) -> Result<(), String> {
        self.notify(n)
    }
    /// Tell the person through the channels too. Returns one plain line per channel; an `Err` is one plain sentence and does not fail the run.
    fn notify_channels(&self, _n: &Notice, _c: &ChannelNotice) -> Result<Vec<String>, String> {
        Err("no messaging account is set up on this computer (Settings, Notifications, Matrix account)".into())
    }
    /// A vault secret by name (never logged: the engine registers it with the redactor).
    fn secret(&self, name: &str) -> Result<String, String> {
        Err(format!("there is no secret `{name}` in your vault"))
    }
    /// The same, asked on behalf of ONE RUN: the engine always asks this one (never `secret` directly), so a host can decide by who started
    /// the run. A cloud tenant refuses a run that nobody started by hand any secret that a team admin did not mark for scheduled runs.
    /// The default is the plain lookup.
    fn secret_for_run(&self, _run: &RunSecretCx, name: &str) -> Result<String, String> {
        self.secret(name)
    }
    fn audit(&self, _action: &str, _detail: &str) {}
    /// Ask the healing agent for a diagnosis and a patch.
    fn propose_patch(&self, _req: &HealRequest, _cancel: &Cancel) -> Result<HealProposal, String> {
        Err("no assistant is available to look at this".into())
    }
    /// A fingerprint of a file or folder for `file_change` triggers.
    fn file_fingerprint(&self, _path: &str) -> Option<String> {
        None
    }
    /// Calendar event starts in `(since_ms, to_ms]` whose title contains `matching`.
    fn calendar_starts(&self, _matching: &str, _since_ms: i64, _to_ms: i64) -> Vec<i64> {
        vec![]
    }
    /// A folder the evidence of a failed run is written to.
    fn evidence_dir(&self, _flow_id: &str, _run_id: &str, _node_id: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// Time zones by name for cron triggers.
    fn tz(&self, name: &str) -> Option<Arc<dyn crate::sched::Tz>> {
        crate::sched::basic_tz(name)
    }
}

// ------------------------------------------------------------------------------------------------ a scripted host

type HttpFn = Arc<dyn Fn(&HttpRequest) -> Result<HttpResponse, NodeError> + Send + Sync>;
type BrowserOpFn = Arc<dyn Fn(&BrowserOp) -> Result<BrowserOpResult, NodeError> + Send + Sync>;
/// Whose run asks for a secret: enough for a host to tell a run started by a person from one started by a timer or an event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSecretCx {
    pub run_id: String,
    pub flow_id: String,
    /// How the run was started: `manual`, `chat`, `test`, `contract` (a person asked), else a timer, event or hook kind.
    pub trigger: String,
}

impl RunSecretCx {
    /// From the template context of a node (`ctx.run.id`, `.flow`, `.trigger`).
    pub fn from_ctx(ctx: &Value) -> RunSecretCx {
        let g = |k: &str| ctx.get("run").and_then(|r| r.get(k)).and_then(|v| v.as_str()).unwrap_or("").to_string();
        RunSecretCx { run_id: g("id"), flow_id: g("flow"), trigger: g("trigger") }
    }

    /// A person started this run just now (so it may use that person's own secrets). Anything else, and any trigger this code does not
    /// know, is "nobody is there".
    pub fn attended(&self) -> bool {
        matches!(self.trigger.as_str(), "manual" | "chat" | "test" | "contract")
    }
}

type ExecFn = Arc<dyn Fn(&ExecRequest) -> Result<ExecResult, NodeError> + Send + Sync>;

type SecretRule = Arc<dyn Fn(&RunSecretCx, &str) -> Option<Result<String, String>> + Send + Sync>;

/// A host for tests: scripted answers, and a record of what was asked.
#[derive(Default)]
pub struct FakeHost {
    pub http_fn: Mutex<Option<HttpFn>>,
    pub exec_fn: Mutex<Option<ExecFn>>,
    pub http_calls: Mutex<Vec<HttpRequest>>,
    pub exec_calls: Mutex<Vec<ExecRequest>>,
    pub browser_calls: Mutex<Vec<BrowserRequest>>,
    pub browser_results: Mutex<VecDeque<Result<BrowserResult, NodeError>>>,
    pub browser_ops: Mutex<Vec<BrowserOp>>,
    pub browser_op_fn: Mutex<Option<BrowserOpFn>>,
    pub browser_ended: Arc<Mutex<Vec<String>>>,
    pub cam_calls: Mutex<Vec<CamPhotoRequest>>,
    pub cam_results: Mutex<VecDeque<Result<CamPhotoResult, NodeError>>>,
    pub cam_ended: Mutex<Vec<String>>,
    pub permission_needs: Mutex<Vec<PermissionNeed>>,
    pub agent_results: Mutex<VecDeque<Result<AgentResult, NodeError>>>,
    pub agent_calls: Mutex<Vec<AgentRequest>>,
    pub script_results: Mutex<VecDeque<Result<Value, NodeError>>>,
    pub notices: Mutex<Vec<Notice>>,
    /// What `notify` asked of the channels, and the answer to give (none: the default refusal).
    pub channel_calls: Mutex<Vec<ChannelNotice>>,
    pub channel_result: Mutex<Option<Result<Vec<String>, String>>>,
    pub secrets: Mutex<BTreeMap<String, String>>,
    /// What `secret_for_run` was asked (the run and the name), and an optional rule that answers instead of the plain lookup.
    /// The `to` of every `notify` step that named one (or `Null`).
    pub notify_tos: Mutex<Vec<Value>>,
    pub secret_asks: Mutex<Vec<(RunSecretCx, String)>>,
    pub secret_rule: Mutex<Option<SecretRule>>,
    pub audits: Mutex<Vec<String>>,
    pub clis: Mutex<BTreeMap<String, CliInfo>>,
    pub upgrades: Mutex<Vec<(String, String)>>,
    pub proposals: Mutex<VecDeque<Result<HealProposal, String>>>,
    pub heal_requests: Mutex<Vec<HealRequest>>,
    pub fingerprints: Mutex<BTreeMap<String, String>>,
    pub evidence: Mutex<Option<std::path::PathBuf>>,
}

impl FakeHost {
    pub fn new() -> Arc<FakeHost> {
        Arc::new(FakeHost::default())
    }
    pub fn on_http(&self, f: impl Fn(&HttpRequest) -> Result<HttpResponse, NodeError> + Send + Sync + 'static) -> &Self {
        *self.http_fn.lock().unwrap() = Some(Arc::new(f));
        self
    }
    pub fn on_exec(&self, f: impl Fn(&ExecRequest) -> Result<ExecResult, NodeError> + Send + Sync + 'static) -> &Self {
        *self.exec_fn.lock().unwrap() = Some(Arc::new(f));
        self
    }
    /// The runs whose browser session was ended, shared so a test can watch from another thread.
    pub fn browser_ended_arc(&self) -> Arc<Mutex<Vec<String>>> {
        self.browser_ended.clone()
    }
    pub fn on_browser_op(&self, f: impl Fn(&BrowserOp) -> Result<BrowserOpResult, NodeError> + Send + Sync + 'static) -> &Self {
        *self.browser_op_fn.lock().unwrap() = Some(Arc::new(f));
        self
    }
    pub fn secret_rule(&self, f: impl Fn(&RunSecretCx, &str) -> Option<Result<String, String>> + Send + Sync + 'static) -> &Self {
        *self.secret_rule.lock().unwrap() = Some(Arc::new(f));
        self
    }
    pub fn secret_is(&self, name: &str, value: &str) -> &Self {
        self.secrets.lock().unwrap().insert(name.into(), value.into());
        self
    }
    pub fn cli_is(&self, id: &str, version: &str) -> &Self {
        self.clis.lock().unwrap().insert(id.into(), CliInfo { installed: true, version: version.into() });
        self
    }
    pub fn propose(&self, p: Result<HealProposal, String>) -> &Self {
        self.proposals.lock().unwrap().push_back(p);
        self
    }
    pub fn audit_lines(&self) -> Vec<String> {
        self.audits.lock().unwrap().clone()
    }
}

pub fn ok_response(status: u16, body: &str) -> HttpResponse {
    HttpResponse { status, headers: vec![("content-type".into(), "application/json".into())], body: body.into(), elapsed_ms: 1 }
}

pub fn ok_exec(stdout: &str) -> ExecResult {
    ExecResult { code: Some(0), stdout: stdout.into(), ran_in: "local".into(), ..Default::default() }
}

pub fn failed_exec(code: i32, stderr: &str) -> ExecResult {
    ExecResult { code: Some(code), stderr: stderr.into(), ran_in: "local".into(), ..Default::default() }
}

impl Host for FakeHost {
    fn http(&self, req: &HttpRequest, _c: &Cancel) -> Result<HttpResponse, NodeError> {
        self.http_calls.lock().unwrap().push(req.clone());
        let f = self.http_fn.lock().unwrap().clone();
        match f {
            Some(f) => f(req),
            None => na("HTTP"),
        }
    }
    fn exec(&self, req: &ExecRequest, _c: &Cancel) -> Result<ExecResult, NodeError> {
        self.exec_calls.lock().unwrap().push(req.clone());
        let f = self.exec_fn.lock().unwrap().clone();
        match f {
            Some(f) => f(req),
            None => na("running commands"),
        }
    }
    fn cli_info(&self, id: &str) -> Option<CliInfo> {
        Some(self.clis.lock().unwrap().get(id).cloned().unwrap_or_default())
    }
    fn cli_known(&self, id: &str) -> bool {
        self.clis.lock().unwrap().contains_key(id)
    }
    fn cli_upgrade(&self, id: &str, min: &str) -> Result<String, String> {
        self.upgrades.lock().unwrap().push((id.into(), min.into()));
        self.clis.lock().unwrap().insert(id.into(), CliInfo { installed: true, version: min.into() });
        Ok(format!("{id} {min} installed"))
    }
    fn browser(&self, req: &BrowserRequest, _c: &Cancel) -> Result<BrowserResult, NodeError> {
        self.browser_calls.lock().unwrap().push(req.clone());
        self.browser_results.lock().unwrap().pop_front().unwrap_or_else(|| Ok(BrowserResult { report: "ok".into(), ..Default::default() }))
    }
    fn browser_op(&self, req: &BrowserOp, _c: &Cancel) -> Result<BrowserOpResult, NodeError> {
        self.browser_ops.lock().unwrap().push(req.clone());
        let f = self.browser_op_fn.lock().unwrap().clone();
        match f {
            Some(f) => f(req),
            None => Ok(BrowserOpResult { value: serde_json::json!({"ok": true}), ..Default::default() }),
        }
    }
    fn browser_end(&self, run_id: &str) {
        self.browser_ended.lock().unwrap().push(run_id.to_string());
    }
    fn cam_photo(&self, req: &CamPhotoRequest, _c: &Cancel) -> Result<CamPhotoResult, NodeError> {
        self.cam_calls.lock().unwrap().push(req.clone());
        self.cam_results.lock().unwrap().pop_front().unwrap_or_else(|| Ok(CamPhotoResult { path: "/photos/one.jpg".into(), taken_at: 1_700_000_000, device: "video0".into(), log: vec![] }))
    }
    fn cam_end(&self, run_id: &str) {
        self.cam_ended.lock().unwrap().push(run_id.to_string());
    }
    fn permission_needed(&self, n: &PermissionNeed) {
        self.permission_needs.lock().unwrap().push(n.clone());
    }
    fn agent(&self, req: &AgentRequest, _c: &Cancel) -> Result<AgentResult, NodeError> {
        self.agent_calls.lock().unwrap().push(req.clone());
        self.agent_results.lock().unwrap().pop_front().unwrap_or_else(|| Ok(AgentResult { text: "done".into(), cents: 1, log: vec![] }))
    }
    fn script(&self, _r: &ScriptRequest, _c: &Cancel) -> Result<Value, NodeError> {
        self.script_results.lock().unwrap().pop_front().unwrap_or(Ok(Value::Null))
    }
    fn notify_to(&self, n: &Notice, to: &Value) -> Result<(), String> {
        self.notify_tos.lock().unwrap().push(to.clone());
        self.notify(n)
    }
    fn notify(&self, n: &Notice) -> Result<(), String> {
        self.notices.lock().unwrap().push(n.clone());
        Ok(())
    }
    fn notify_channels(&self, _n: &Notice, c: &ChannelNotice) -> Result<Vec<String>, String> {
        self.channel_calls.lock().unwrap().push(c.clone());
        self.channel_result.lock().unwrap().clone().unwrap_or_else(|| Err("no messaging account is set up on this computer (Settings, Notifications, Matrix account)".into()))
    }
    fn secret_for_run(&self, run: &RunSecretCx, name: &str) -> Result<String, String> {
        self.secret_asks.lock().unwrap().push((run.clone(), name.to_string()));
        let rule = self.secret_rule.lock().unwrap().clone();
        if let Some(a) = rule.and_then(|r| r(run, name)) {
            return a;
        }
        self.secret(name)
    }
    fn secret(&self, name: &str) -> Result<String, String> {
        self.secrets.lock().unwrap().get(name).cloned().ok_or_else(|| format!("there is no secret `{name}` in your vault"))
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audits.lock().unwrap().push(format!("{action} {detail}"));
    }
    fn propose_patch(&self, req: &HealRequest, _c: &Cancel) -> Result<HealProposal, String> {
        self.heal_requests.lock().unwrap().push(req.clone());
        self.proposals.lock().unwrap().pop_front().unwrap_or_else(|| Err("no proposal scripted".into()))
    }
    fn file_fingerprint(&self, path: &str) -> Option<String> {
        self.fingerprints.lock().unwrap().get(path).cloned()
    }
    fn evidence_dir(&self, flow_id: &str, run_id: &str, node_id: &str) -> Option<std::path::PathBuf> {
        self.evidence.lock().unwrap().as_ref().map(|d| d.join(flow_id).join("evidence").join(run_id).join(node_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redactor_removes_every_secret_and_longer_ones_first() {
        let r = Redactor::new();
        r.add("hunter22");
        r.add("hunter22-extra");
        r.add("ab");
        assert_eq!(r.text("token=hunter22-extra and hunter22 and ab"), "token=*** and *** and ab");
        assert_eq!(r.value(&serde_json::json!({"a": ["x hunter22"], "n": 1})), serde_json::json!({"a": ["x ***"], "n": 1}));
    }

    #[test]
    fn the_fake_clock_only_moves_when_slept() {
        let c = FakeClock::new(1000);
        let cancel = Cancel::new();
        assert!(c.sleep_ms(500, &cancel));
        assert_eq!(c.now_ms(), 1500);
        cancel.cancel();
        assert!(!c.sleep_ms(500, &cancel));
        assert_eq!(c.now_ms(), 1500);
        assert_eq!(c.sleeps(), vec![500]);
    }

    #[test]
    fn the_real_clock_wakes_on_cancel() {
        let c = SystemClock;
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c2.cancel();
        });
        let started = Instant::now();
        assert!(!c.sleep_ms(10_000, &cancel));
        assert!(started.elapsed() < Duration::from_secs(2));
        t.join().unwrap();
    }
}
