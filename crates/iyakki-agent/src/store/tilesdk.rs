//! The SDK part of the tile bridge that costs something or leaves the page: `iyakki.llm` (a bare model through the person's own
//! provider setup), `iyakki.models.list`, `iyakki.llm.usage` and `iyakki.speech.say` (docs/development/SDK-API.md, sections 3, 4.3, 4.7, 6).
//!
//! `TileHost` (store/tile.rs) checks the capability and hands the call here. Everything the spec says an app must not see stays on this
//! side: the provider, its address and its key never reach the page (the app gets model ids and a `where` label), the person's first
//! answer is asked once per app and kept, the daily budget is checked before the provider is asked, and every call is written to the
//! usage log with the app's id, its outcome and its time. A model is slow, so a call that is not answered at once becomes a *job*: the
//! page gets a promise (or a stream handle), a worker thread does the work, and [`TileSdk::tick`] plus the runtime's `pump` bring the
//! answer back on the page's next turn.
//!
//! Every backend is a trait with a fake in this file, so an app is testable without a model or a speaker (`T.fake.*` in the headless
//! harness). The fakes only replace the model and the speaker: argument checks, capabilities, approvals, quotas and error codes are
//! the real ones.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::tile::Outbox;
use crate::clock::Zone;
use crate::services::Services;
use crate::usage::{Call, Totals, UsageLog};

/// Longest prompt (all messages together), in characters.
pub const PROMPT_CHARS: usize = 100_000;
pub const MAX_MESSAGES: usize = 64;
/// The most tokens one answer may have.
pub const MAX_TOKENS: u64 = 4096;
pub const DEFAULT_TOKENS: u64 = 1024;
/// Model calls per minute and per app.
pub const LLM_PER_MINUTE: usize = 20;
/// Calls in flight (waiting for an answer of the person or of the model) per app.
pub const MAX_JOBS: usize = 4;
/// How long a model or a voice may take.
pub const JOB_MS: u64 = 60_000;
/// How long the person has to answer a first-use card.
pub const ANSWER_MS: u64 = 120_000;
/// Texts read aloud per hour and per app.
pub const SAY_PER_HOUR: usize = 30;
pub const SAY_CHARS: usize = 20_000;
/// Events waiting for the page per stream.
pub const EVENTS_PER_HANDLE: usize = 50;
/// The daily token budget of an app that uses a model that is not on this computer, until the person sets another.
pub const DEFAULT_CLOUD_TOKENS: u64 = 200_000;

pub const ERROR_PREFIX: &str = "iyakki-error:";

// ------------------------------------------------------------------------------------------------ errors

/// The error codes of docs/development/SDK-API.md, section 3. They are stable.
pub mod code {
    pub const NO_CAPABILITY: &str = "NoCapability";
    pub const DENIED: &str = "Denied";
    pub const BAD_ARGUMENT: &str = "BadArgument";
    pub const QUOTA: &str = "QuotaExceeded";
    pub const BUDGET_STOP: &str = "BudgetStop";
    pub const UNAVAILABLE: &str = "Unavailable";
    pub const BUSY: &str = "Busy";
    pub const TIMEOUT: &str = "Timeout";
    pub const BLOCKED: &str = "Blocked";
    pub const FAILED: &str = "Failed";
}

/// An error with a code the page can test (`e.code`), plain words, and the extra fields of its code (`limit`, `resets_at`, `holder`, `fix`).
#[derive(Clone, Debug, PartialEq)]
pub struct SdkError {
    pub code: &'static str,
    pub message: String,
    pub extra: Map<String, Value>,
}

impl SdkError {
    pub fn new(code: &'static str, message: impl Into<String>) -> SdkError {
        SdkError { code, message: message.into(), extra: Map::new() }
    }

    pub fn with(mut self, k: &str, v: impl Into<Value>) -> SdkError {
        self.extra.insert(k.into(), v.into());
        self
    }

    pub fn bad(field: &str, why: &str) -> SdkError {
        SdkError::new(code::BAD_ARGUMENT, format!("{field}: {why}")).with("field", field)
    }

    pub fn payload(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("code".into(), json!(self.code));
        m.insert("message".into(), json!(self.message));
        Value::Object(m)
    }

    /// What goes through the bridge: the page turns it back into an `Error` with `.code` and the extra fields.
    pub fn encode(&self) -> String {
        format!("{ERROR_PREFIX}{}", self.payload())
    }

    /// The code and message of an encoded error (tests, the shell's audit).
    pub fn decode(s: &str) -> Option<(String, String)> {
        let v: Value = serde_json::from_str(s.strip_prefix(ERROR_PREFIX)?).ok()?;
        Some((v["code"].as_str()?.into(), v["message"].as_str()?.into()))
    }
}

/// An engine's own error text without anything that names a server or looks like a key (the spec: `Failed.detail` is redacted).
pub fn plain(e: &str) -> String {
    let words: Vec<&str> = e.split_whitespace().filter(|w| !w.contains("://") && !w.starts_with("sk-") && !w.to_ascii_lowercase().starts_with("bearer")).collect();
    let t: String = words.join(" ").chars().filter(|c| !c.is_control()).take(200).collect();
    if t.is_empty() { "the model did not answer".into() } else { t }
}

// ------------------------------------------------------------------------------------------------ the model

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    /// (role, content), role being `system`, `user` or `assistant`.
    pub messages: Vec<(String, String)>,
    pub model: Option<String>,
    pub max_tokens: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatReply {
    pub text: String,
    pub input: u64,
    pub output: u64,
    pub estimated: bool,
    pub model: String,
}

/// A model the app may name. `provider` is for the log and the person's card, never for the page.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub place: &'static str,
}

impl ModelInfo {
    /// Free: nothing leaves this computer and nothing is billed.
    pub fn free(&self) -> bool {
        self.place == "this computer"
    }

    fn for_page(&self) -> Value {
        json!({"id": self.id, "label": self.label, "kind": "chat", "where": self.place, "free": self.free()})
    }
}

pub trait LlmBackend: Send + Sync {
    /// Answer the conversation; `on_text` gets each piece as it is written and returns false to stop early.
    fn chat(&self, req: &ChatRequest, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatReply, SdkError>;
    /// The models an app may name; the first is the one used when it names none.
    fn models(&self) -> Vec<ModelInfo>;
    /// The answer comes at once on the calling thread (a fake), so no worker is needed.
    fn inline(&self) -> bool {
        false
    }
}

// ------------------------------------------------------------------------------------------------ the voice

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SayOpts {
    pub voice: Option<String>,
    pub lang: Option<String>,
    pub speed: Option<f32>,
}

pub trait SpeechBackend: Send + Sync {
    /// Start reading `text` aloud for `app` and return an id once the sound has started. `Busy` when the speaker cannot be shared.
    fn say(&self, app: &str, text: &str, opts: &SayOpts) -> Result<u32, SdkError>;
    /// Stop one reading (`None`: all of this app's).
    fn stop(&self, app: &str, id: Option<u32>);
    /// Plain words for the first-use card: where the text goes.
    fn destination(&self) -> String;
    fn inline(&self) -> bool {
        false
    }
}

// ------------------------------------------------------------------------------------------------ fakes

#[derive(Default)]
struct FakeLlmState {
    reply: Option<String>,
    fail: Option<String>,
    usage: Option<(u64, u64)>,
    models: Option<Vec<ModelInfo>>,
    seen: Vec<ChatRequest>,
}

/// A model that answers what the test says and never leaves the process.
#[derive(Default)]
pub struct FakeLlm(Mutex<FakeLlmState>);

impl FakeLlm {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeLlmState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn reply(&self, text: &str) {
        let mut s = self.st();
        s.reply = Some(text.into());
        s.fail = None;
    }

    pub fn fail(&self, code: &str) {
        self.st().fail = Some(code.into());
    }

    pub fn usage(&self, input: u64, output: u64) {
        self.st().usage = Some((input, output));
    }

    pub fn set_models(&self, m: Vec<ModelInfo>) {
        self.st().models = Some(m);
    }

    /// The requests the backend was asked, in order (tests).
    pub fn seen(&self) -> Vec<ChatRequest> {
        self.st().seen.clone()
    }
}

fn code_named(c: &str) -> &'static str {
    [code::DENIED, code::BAD_ARGUMENT, code::QUOTA, code::BUDGET_STOP, code::UNAVAILABLE, code::BUSY, code::TIMEOUT, code::BLOCKED, code::FAILED].into_iter().find(|k| *k == c).unwrap_or(code::FAILED)
}

pub fn fake_model() -> ModelInfo {
    ModelInfo { id: "fake-model".into(), label: "Fake model".into(), provider: "fake".into(), place: "this computer" }
}

impl LlmBackend for FakeLlm {
    fn chat(&self, req: &ChatRequest, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatReply, SdkError> {
        let (reply, fail, usage, model) = {
            let mut s = self.st();
            s.seen.push(req.clone());
            (s.reply.clone().unwrap_or_else(|| "ok".into()), s.fail.clone(), s.usage, s.models.as_ref().and_then(|m| m.first().cloned()).unwrap_or_else(fake_model))
        };
        if let Some(c) = fail {
            return Err(SdkError::new(code_named(&c), format!("the fake model was told to fail with {c}")));
        }
        let mut said = String::new();
        for (i, w) in reply.split(' ').enumerate() {
            let piece = if i == 0 { w.to_string() } else { format!(" {w}") };
            said.push_str(&piece);
            if !on_text(&piece) {
                break;
            }
        }
        let (input, output, estimated) = match usage {
            Some((i, o)) => (i, o, false),
            None => (req.messages.iter().map(|m| m.1.chars().count() as u64).sum::<u64>().div_ceil(4), (said.chars().count() as u64).div_ceil(4), true),
        };
        Ok(ChatReply { text: said, input, output, estimated, model: req.model.clone().unwrap_or(model.id) })
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.st().models.clone().unwrap_or_else(|| vec![fake_model()])
    }

    fn inline(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct FakeSpeechState {
    busy: Option<String>,
    said: Vec<(String, SayOpts)>,
    next: u32,
    stopped: Vec<u32>,
}

/// A speaker that is never heard: it keeps what it was asked to say.
#[derive(Default)]
pub struct FakeSpeech(Mutex<FakeSpeechState>);

impl FakeSpeech {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeSpeechState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The speaker is held by `holder` (`None`: free again).
    pub fn set_busy(&self, holder: Option<&str>) {
        self.st().busy = holder.map(str::to_string);
    }

    pub fn said(&self) -> Vec<(String, SayOpts)> {
        self.st().said.clone()
    }

    pub fn stopped(&self) -> Vec<u32> {
        self.st().stopped.clone()
    }
}

impl SpeechBackend for FakeSpeech {
    fn say(&self, _app: &str, text: &str, opts: &SayOpts) -> Result<u32, SdkError> {
        let mut s = self.st();
        if let Some(h) = &s.busy {
            return Err(busy(h));
        }
        s.said.push((text.into(), opts.clone()));
        s.next += 1;
        Ok(s.next)
    }

    fn stop(&self, _app: &str, id: Option<u32>) {
        let mut s = self.st();
        let n = s.next;
        s.stopped.push(id.unwrap_or(n));
    }

    fn destination(&self) -> String {
        "Text stays on this computer (a test voice).".into()
    }

    fn inline(&self) -> bool {
        true
    }
}

pub fn busy(holder: &str) -> SdkError {
    SdkError::new(code::BUSY, format!("the speaker is in use by {holder}: try again when it is done")).with("holder", holder)
}

// ------------------------------------------------------------------------------------------------ the numbers

/// Where an app's calls are written and read back. The real one is the usage log of the data folder.
pub trait Meter: Send + Sync {
    fn record(&self, user: &str, c: &Call);
    /// Today's model use (`llm`) of `src` in the local day, and when the day ends.
    fn today(&self, user: &str, src: &str) -> (Totals, i64);
    /// The last `days` local days of `src`'s model use, oldest first.
    fn days(&self, user: &str, src: &str, days: usize) -> Vec<(String, Totals)>;
    /// Every source's use by kind, since the log began (what the Settings page shows).
    fn by_source(&self, user: &str) -> Vec<Totals>;
}

fn log_today(log: &UsageLog, zone: &Zone, user: &str, src: &str) -> (Totals, i64) {
    let now = crate::clock::now_unix();
    let (from, to) = UsageLog::today_span(now, zone);
    (log.totals_of(Some(user), src, Some("llm"), from), to)
}

/// A log in a folder of its own, days in UTC (tests and tools without settings).
pub struct LogMeter(pub UsageLog);

impl Meter for LogMeter {
    fn record(&self, user: &str, c: &Call) {
        self.0.record_call(crate::clock::now_unix(), user, c);
    }

    fn today(&self, user: &str, src: &str) -> (Totals, i64) {
        log_today(&self.0, &Zone::utc(), user, src)
    }

    fn days(&self, user: &str, src: &str, days: usize) -> Vec<(String, Totals)> {
        self.0.days_of(Some(user), src, Some("llm"), days, crate::clock::now_unix(), &Zone::utc())
    }

    fn by_source(&self, user: &str) -> Vec<Totals> {
        self.0.by_source(Some(user), 0)
    }
}

/// The usage log of the running system, days in the person's time zone.
pub struct ServicesMeter(pub Arc<Services>);

impl Meter for ServicesMeter {
    fn record(&self, user: &str, c: &Call) {
        self.0.usage.record_call(crate::clock::now_unix(), user, c);
    }

    fn today(&self, user: &str, src: &str) -> (Totals, i64) {
        log_today(&self.0.usage, &Zone::system(), user, src)
    }

    fn days(&self, user: &str, src: &str, days: usize) -> Vec<(String, Totals)> {
        self.0.usage.days_of(Some(user), src, Some("llm"), days, crate::clock::now_unix(), &Zone::system())
    }

    fn by_source(&self, user: &str) -> Vec<Totals> {
        self.0.usage.by_source(Some(user), 0)
    }
}

// ------------------------------------------------------------------------------------------------ what the person decided

/// What is kept per app and person: the first-use answers and the daily limit the person set. The app cannot change any of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub llm: bool,
    pub speech: bool,
    /// Daily tokens; `None`: the default for the model in use (200,000 for a model that is not on this computer, none for one that is).
    pub tokens_per_day: Option<u64>,
}

/// How the first-use card is answered. Only tests change it; a real runtime always asks the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approve {
    Ask,
    Allow,
    Deny,
}

/// What a test can reach (`T.fake`).
pub struct Fakes {
    pub llm: Arc<FakeLlm>,
    pub speech: Arc<FakeSpeech>,
    pub skew_ms: AtomicU64,
    pub approve: Mutex<Approve>,
}

enum Work {
    Chat { req: ChatRequest, stream: bool },
    Say { text: String, opts: SayOpts },
}

enum Stage {
    Approval { since: u64 },
    Running { since: u64, cancel: Arc<AtomicBool> },
}

struct Job {
    work: Work,
    stage: Stage,
    /// `llm` or `speech`: which first-use answer and which in-flight count it belongs to.
    ns: &'static str,
}

struct State {
    saved: Saved,
    path: Option<PathBuf>,
    llm_calls: Vec<u64>,
    says: Vec<u64>,
    jobs: HashMap<u32, Job>,
    warned_day: i64,
}

type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

/// One app's SDK state and backends.
pub struct TileSdk {
    app: String,
    user: String,
    llm: Arc<dyn LlmBackend>,
    speech: Arc<dyn SpeechBackend>,
    meter: Arc<dyn Meter>,
    out: Arc<Mutex<Outbox>>,
    state: Mutex<State>,
    clock: Clock,
    fakes: Option<Fakes>,
}

fn ms_now() -> Clock {
    let t0 = std::time::Instant::now();
    Box::new(move || t0.elapsed().as_millis() as u64)
}

impl TileSdk {
    pub fn new(app: &str, user: &str, llm: Arc<dyn LlmBackend>, speech: Arc<dyn SpeechBackend>, meter: Arc<dyn Meter>, out: Arc<Mutex<Outbox>>, state_file: Option<&Path>) -> TileSdk {
        let saved = state_file.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let st = State { saved, path: state_file.map(Path::to_path_buf), llm_calls: vec![], says: vec![], jobs: HashMap::new(), warned_day: 0 };
        TileSdk { app: app.into(), user: user.into(), llm, speech, meter, out, state: Mutex::new(st), clock: ms_now(), fakes: None }
    }

    /// The running system: the person's providers, the voice of Settings -> Voice, the usage log of the data folder.
    pub fn for_services(svc: &Arc<Services>, user: &str, app: &str, out: Arc<Mutex<Outbox>>) -> TileSdk {
        let file = svc.dir().join("appdata").join("tile-sdk").join(user).join(format!("{app}.json"));
        TileSdk::new(app, user, Arc::new(ServicesLlm { svc: svc.clone() }), Arc::new(SystemSpeech::new(svc.clone())), Arc::new(ServicesMeter(svc.clone())), out, Some(&file))
    }

    /// Fakes for every backend, a usage log in `dir`, and the first-use cards answered "allow" until a test says otherwise.
    pub fn fake(app: &str, out: Arc<Mutex<Outbox>>, dir: &Path) -> TileSdk {
        let _ = std::fs::create_dir_all(dir);
        let (llm, speech) = (Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()));
        let mut s = TileSdk::new(app, "tester", llm.clone(), speech.clone(), Arc::new(LogMeter(UsageLog::open(&dir.join("usage.jsonl")))), out, None);
        s.fakes = Some(Fakes { llm, speech, skew_ms: AtomicU64::new(0), approve: Mutex::new(Approve::Allow) });
        s
    }

    pub fn with_clock(mut self, c: impl Fn() -> u64 + Send + Sync + 'static) -> TileSdk {
        self.clock = Box::new(c);
        self
    }

    pub fn fakes(&self) -> Option<&Fakes> {
        self.fakes.as_ref()
    }

    fn now(&self) -> u64 {
        (self.clock)() + self.fakes.as_ref().map_or(0, |f| f.skew_ms.load(Ordering::SeqCst))
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn out(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.out.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn src(&self) -> String {
        format!("app:{}", self.app)
    }

    fn audit(&self, action: &str, detail: &str) {
        let mut o = self.out();
        if o.audit.len() < 100 {
            o.audit.push((action.into(), detail.into()));
        }
    }

    fn save(st: &State) {
        let Some(p) = &st.path else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, serde_json::to_vec_pretty(&st.saved).unwrap_or_default());
    }

    // ---- what the person set

    pub fn saved(&self) -> Saved {
        self.st().saved.clone()
    }

    /// The person (Settings, the Info panel) changes the daily token limit. Never reachable from the page.
    pub fn set_tokens_per_day(&self, n: Option<u64>) {
        let mut st = self.st();
        st.saved.tokens_per_day = n;
        Self::save(&st);
        drop(st);
        self.audit("app.budget.set", &format!("{} tokens/day: {}", self.app, n.map_or("default".to_string(), |n| n.to_string())));
    }

    /// The person takes back the first-use answers (Info -> Revoke).
    pub fn revoke(&self) {
        let mut st = self.st();
        st.saved.llm = false;
        st.saved.speech = false;
        Self::save(&st);
    }

    /// The limit now in force: the person's, else the default for the model in use.
    pub fn token_cap(&self) -> Option<u64> {
        if let Some(n) = self.st().saved.tokens_per_day {
            return Some(n);
        }
        let paid = self.llm.models().first().is_some_and(|m| !m.free());
        paid.then_some(DEFAULT_CLOUD_TOKENS)
    }

    /// One line for the Info panel, or `None` when nothing was used and no limit applies.
    pub fn summary(&self) -> Option<String> {
        let (t, _) = self.meter.today(&self.user, &self.src());
        let cap = self.token_cap();
        if t.calls == 0 && cap.is_none() {
            return None;
        }
        let used = format!("Today: {} call{}, {} tokens{}", t.calls, if t.calls == 1 { "" } else { "s" }, t.tokens(), if t.estimated { " (estimated)" } else { "" });
        Some(match cap {
            Some(c) => format!("{used}, of your limit of {c} tokens a day (not priced in money)"),
            None => format!("{used}, no daily limit for a model on this computer"),
        })
    }

    // ---- the calls

    /// `models.list`
    pub fn models(&self, a: &Value) -> Result<Value, SdkError> {
        check_keys(a, &["kind"])?;
        let kind = opt_str(a, "kind", 16)?;
        if let Some(k) = kind {
            if !["chat", "embedding", "image", "stt", "tts"].contains(&k) {
                return Err(SdkError::bad("kind", "chat, embedding, image, stt or tts"));
            }
            if k != "chat" {
                return Ok(json!([]));
            }
        }
        Ok(Value::Array(self.llm.models().iter().map(ModelInfo::for_page).collect()))
    }

    /// `llm.usage`: this app's own numbers.
    pub fn usage(&self, a: &Value) -> Result<Value, SdkError> {
        check_keys(a, &["since"])?;
        let since = opt_str(a, "since", 8)?.unwrap_or("today");
        let n = match since {
            "today" => 1,
            "7d" => 7,
            "30d" => 30,
            _ => return Err(SdkError::bad("since", "today, 7d or 30d")),
        };
        let src = self.src();
        let (today, resets) = self.meter.today(&self.user, &src);
        let days = self.meter.days(&self.user, &src, n.max(7));
        let mut range = Totals { src: src.clone(), ..Default::default() };
        for (_, d) in days.iter().skip(days.len().saturating_sub(n)) {
            range.calls += d.calls;
            range.input += d.input;
            range.output += d.output;
            range.errors += d.errors;
            range.ms += d.ms;
        }
        let cap = self.token_cap();
        let totals = |t: &Totals| json!({"calls": t.calls, "input": t.input, "output": t.output, "errors": t.errors, "meanMs": t.mean_ms(), "estimated": t.estimated, "cents": 0, "priced": false});
        Ok(json!({
            "since": since,
            "today": totals(&today),
            "range": totals(&range),
            "limits": {"tokensPerDay": cap, "centsPerDay": null},
            "remaining": {"tokens": cap.map(|c| c.saturating_sub(today.tokens())), "cents": null},
            "resetsAt": resets,
            "byDay": days.iter().map(|(d, t)| json!({"day": d, "calls": t.calls, "input": t.input, "output": t.output})).collect::<Vec<_>>(),
        }))
    }

    /// `llm.chat` and `llm.stream`. A chat gives its answer (inline) or `{pending}`; a stream gives `{handle}`.
    pub fn chat(self: &Arc<Self>, a: &Value, stream: bool) -> Result<Value, SdkError> {
        let req = parse_chat(a)?;
        let stream = stream || a.get("stream").and_then(Value::as_bool).unwrap_or(false);
        if let Some(m) = &req.model {
            if !self.llm.models().iter().any(|x| &x.id == m) {
                return Err(SdkError::bad("model", "not one of the ids models.list() gives"));
            }
        }
        let now = self.now();
        {
            let mut st = self.st();
            if st.jobs.values().filter(|j| j.ns == "llm").count() >= MAX_JOBS {
                return Err(quota("calls waiting at once"));
            }
            if stream && st.jobs.values().any(|j| matches!(j.work, Work::Chat { stream: true, .. })) {
                return Err(quota("one stream at a time"));
            }
            st.llm_calls.retain(|t| now.saturating_sub(*t) < 60_000);
            if st.llm_calls.len() >= LLM_PER_MINUTE {
                return Err(quota("20 model calls a minute"));
            }
        }
        self.check_budget(&req)?;
        self.st().llm_calls.push(now);
        let id = self.out().alloc();
        let job = Job { work: Work::Chat { req, stream }, stage: Stage::Approval { since: now }, ns: "llm" };
        let r = self.submit(id, job);
        if stream {
            match r {
                Ok(Some(v)) => self.emit(id, "done", v),
                Ok(None) => {}
                Err(e) => self.emit(id, "error", e.payload()),
            }
            return Ok(json!({"handle": id}));
        }
        match r? {
            Some(v) => Ok(v),
            None => Ok(json!({"pending": id})),
        }
    }

    /// `llm.cancel`: stop a stream (the partial answer is still counted).
    pub fn cancel(&self, a: &Value) -> Result<Value, SdkError> {
        check_keys(a, &["handle"])?;
        let h = a.get("handle").and_then(Value::as_u64).ok_or_else(|| SdkError::bad("handle", "a number"))? as u32;
        let mut st = self.st();
        match st.jobs.get(&h).map(|j| &j.stage) {
            Some(Stage::Running { cancel, .. }) => cancel.store(true, Ordering::SeqCst),
            Some(Stage::Approval { .. }) => {
                st.jobs.remove(&h);
                drop(st);
                self.out().requests.retain(|r| r.id != h);
                self.emit(h, "done", json!({"text": "", "cancelled": true}));
            }
            None => {}
        }
        Ok(Value::Null)
    }

    /// `speech.say`
    pub fn say(self: &Arc<Self>, a: &Value) -> Result<Value, SdkError> {
        check_keys(a, &["text", "voice", "lang", "speed"])?;
        let text = a.get("text").and_then(Value::as_str).ok_or_else(|| SdkError::bad("text", "must be a string"))?;
        if text.trim().is_empty() {
            return Err(SdkError::bad("text", "nothing to say"));
        }
        if text.chars().count() > SAY_CHARS {
            return Err(SdkError::bad("text", "longer than 20000 characters"));
        }
        let voice = opt_str(a, "voice", 80)?.map(str::to_string);
        let lang = opt_str(a, "lang", 16)?.map(str::to_string);
        if lang.as_deref().is_some_and(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            return Err(SdkError::bad("lang", "a language tag like en or ta-IN"));
        }
        let speed = match a.get("speed") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let f = v.as_f64().ok_or_else(|| SdkError::bad("speed", "a number from 0.7 to 1.5"))?;
                if !(0.7..=1.5).contains(&f) {
                    return Err(SdkError::bad("speed", "from 0.7 to 1.5"));
                }
                Some(f as f32)
            }
        };
        let now = self.now();
        {
            let mut st = self.st();
            if st.jobs.values().filter(|j| j.ns == "speech").count() >= MAX_JOBS {
                return Err(quota("readings waiting at once"));
            }
            st.says.retain(|t| now.saturating_sub(*t) < 3_600_000);
            if st.says.len() >= SAY_PER_HOUR {
                return Err(quota("30 readings an hour"));
            }
            st.says.push(now);
        }
        let id = self.out().alloc();
        let job = Job { work: Work::Say { text: text.into(), opts: SayOpts { voice, lang, speed } }, stage: Stage::Approval { since: now }, ns: "speech" };
        Ok(match self.submit(id, job)? {
            Some(v) => v,
            None => json!({"pending": id}),
        })
    }

    /// `speech.stop`
    pub fn stop(&self, a: &Value) -> Result<Value, SdkError> {
        check_keys(a, &["id"])?;
        let id = match a.get("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or_else(|| SdkError::bad("id", "a number"))? as u32),
        };
        self.speech.stop(&self.app, id);
        Ok(Value::Null)
    }

    // ---- budget

    fn check_budget(&self, req: &ChatRequest) -> Result<(), SdkError> {
        let Some(cap) = self.token_cap() else { return Ok(()) };
        let (today, resets) = self.meter.today(&self.user, &self.src());
        // the prompt is estimated the way the log estimates (about four characters a token), so a stopped app costs nothing
        let prompt = req.messages.iter().map(|m| 4 + m.1.chars().count() as u64 / 4).sum::<u64>();
        if today.tokens() + prompt > cap || today.tokens() >= cap {
            self.audit("app.budget.stop", &format!("{} reached {cap} tokens today", self.app));
            return Err(SdkError::new(code::BUDGET_STOP, format!("this app has used its {cap} tokens for today: it can ask again after midnight")).with("resets_at", resets).with("limit", "tokens per day"));
        }
        Ok(())
    }

    /// At 80 percent the person is told once a day (a note in the inbox and an audit line).
    fn warn_if_near(&self) {
        let Some(cap) = self.token_cap() else { return };
        let (today, resets) = self.meter.today(&self.user, &self.src());
        if today.tokens() * 5 < cap * 4 {
            return;
        }
        let mut st = self.st();
        if st.warned_day == resets {
            return;
        }
        st.warned_day = resets;
        drop(st);
        self.out().notices.push(("Usage limit".into(), format!("This app has used {} of its {cap} tokens for today.", today.tokens())));
        self.audit("app.budget.warn", &format!("{} used {} of {cap} tokens", self.app, today.tokens()));
    }

    // ---- approval, running, delivering

    fn mode(&self) -> Approve {
        self.fakes.as_ref().map_or(Approve::Ask, |f| *f.approve.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn approved(&self, ns: &str) -> bool {
        let st = self.st();
        if ns == "llm" { st.saved.llm } else { st.saved.speech }
    }

    fn grant(&self, ns: &str) {
        let mut st = self.st();
        if ns == "llm" {
            st.saved.llm = true;
        } else {
            st.saved.speech = true;
        }
        Self::save(&st);
    }

    /// Ask the person if this app has not been allowed yet, else run. `Ok(Some(v))`: done at once; `Ok(None)`: it will be delivered later.
    fn submit(self: &Arc<Self>, id: u32, job: Job) -> Result<Option<Value>, SdkError> {
        let ns = job.ns;
        if !self.approved(ns) {
            match self.mode() {
                Approve::Allow => self.grant(ns),
                Approve::Deny => return Err(denied(ns)),
                Approve::Ask => {
                    let args = self.card(ns, &job);
                    self.st().jobs.insert(id, job);
                    self.out().requests.push(super::tile::Request { id, kind: "sdk.approve".into(), args });
                    return Ok(None);
                }
            }
        }
        self.run(id, job)
    }

    fn card(&self, ns: &str, job: &Job) -> Value {
        if ns == "speech" {
            return json!({"ns": "speech", "app": self.app, "where": self.speech.destination(), "chars": match &job.work { Work::Say { text, .. } => text.chars().count(), _ => 0 }});
        }
        let m = self.llm.models().into_iter().next();
        let (place, provider, model) = m.map_or(("", String::new(), String::new()), |m| (m.place, m.provider, m.id));
        let leaves = place != "this computer";
        json!({"ns": "llm", "app": self.app, "provider": provider, "model": model, "where": place, "leaves": leaves, "cap": self.token_cap()})
    }

    /// The person answered a first-use card.
    pub fn answer(self: &Arc<Self>, id: u32, allow: bool) -> bool {
        let Some(job) = self.st().jobs.remove(&id) else { return false };
        self.out().requests.retain(|r| r.id != id);
        let stream = matches!(job.work, Work::Chat { stream: true, .. });
        if !allow {
            self.deliver(id, stream, Err(denied(job.ns)));
            return true;
        }
        self.grant(job.ns);
        match self.run(id, job) {
            Ok(Some(v)) => self.deliver(id, stream, Ok(v)),
            Ok(None) => {}
            Err(e) => self.deliver(id, stream, Err(e)),
        }
        true
    }

    fn run(self: &Arc<Self>, id: u32, mut job: Job) -> Result<Option<Value>, SdkError> {
        let cancel = Arc::new(AtomicBool::new(false));
        job.stage = Stage::Running { since: self.now(), cancel: cancel.clone() };
        let inline = match &job.work {
            Work::Chat { .. } => self.llm.inline(),
            Work::Say { .. } => self.speech.inline(),
        };
        let (stream, work) = match &job.work {
            Work::Chat { req, stream } => (*stream, Work::Chat { req: req.clone(), stream: *stream }),
            Work::Say { text, opts } => (false, Work::Say { text: text.clone(), opts: opts.clone() }),
        };
        if inline {
            return self.execute(id, &work, &cancel).map(Some);
        }
        self.st().jobs.insert(id, job);
        let me = self.clone();
        std::thread::spawn(move || {
            let r = me.execute(id, &work, &cancel);
            // a late answer (after Timeout, or after the tile closed) is counted but not delivered
            if me.st().jobs.remove(&id).is_some() {
                me.deliver(id, stream, r);
            }
        });
        Ok(None)
    }

    /// Do the work on this thread and write the call to the usage log.
    fn execute(&self, id: u32, work: &Work, cancel: &AtomicBool) -> Result<Value, SdkError> {
        let t0 = std::time::Instant::now();
        match work {
            Work::Chat { req, stream } => {
                let handle = stream.then_some(id);
                let r = self.llm.chat(req, &mut |piece| {
                    if let Some(h) = handle {
                        if !piece.is_empty() {
                            self.emit(h, "text", json!({"text": piece}));
                        }
                    }
                    !cancel.load(Ordering::SeqCst)
                });
                let default = self.llm.models().into_iter().next();
                let ms = t0.elapsed().as_millis() as u64;
                let (provider, model) = default.map_or((String::new(), String::new()), |m| (m.provider, m.id));
                match r {
                    Ok(rep) => {
                        let call = Call { src: self.src(), cat: "llm".into(), provider, model: rep.model.clone(), input: rep.input, output: rep.output, estimated: rep.estimated, failed: false, ms };
                        self.meter.record(&self.user, &call);
                        self.warn_if_near();
                        Ok(json!({"text": rep.text, "usage": {"input": rep.input, "output": rep.output, "estimated": rep.estimated}, "model": rep.model, "cancelled": cancel.load(Ordering::SeqCst)}))
                    }
                    Err(e) => {
                        self.meter.record(&self.user, &Call { src: self.src(), cat: "llm".into(), provider, model, failed: true, ms, ..Default::default() });
                        Err(e)
                    }
                }
            }
            Work::Say { text, opts } => {
                let r = self.speech.say(&self.app, text, opts);
                let ms = t0.elapsed().as_millis() as u64;
                let call = Call { src: self.src(), cat: "tts".into(), input: text.chars().count() as u64, failed: r.is_err(), ms, ..Default::default() };
                // Busy is not a failure of the voice: nothing was spoken and nothing was tried
                if !matches!(&r, Err(e) if e.code == code::BUSY) {
                    self.meter.record(&self.user, &call);
                }
                r.map(|v| json!({"id": v}))
            }
        }
    }

    /// Bring a late answer to the page: a promise settled, or the last event of a stream.
    fn deliver(&self, id: u32, stream: bool, r: Result<Value, SdkError>) {
        if stream {
            match r {
                Ok(v) => self.emit(id, "done", v),
                Err(e) => self.emit(id, "error", e.payload()),
            }
            return;
        }
        let d = match r {
            Ok(v) => super::tile::Done { id, ok: true, value: v.to_string() },
            Err(e) => super::tile::Done { id, ok: false, value: e.encode() },
        };
        self.out().done.push(d);
    }

    /// Queue an event for the page; at most [`EVENTS_PER_HANDLE`] wait per stream, the oldest are dropped with one `dropped` event.
    fn emit(&self, handle: u32, event: &str, payload: Value) {
        let mut o = self.out();
        let waiting = o.emits.iter().filter(|e| e.handle == handle).count();
        if waiting >= EVENTS_PER_HANDLE && event == "text" {
            if let Some(i) = o.emits.iter().position(|e| e.handle == handle && e.event == "text") {
                o.emits.remove(i);
            }
            if !o.emits.iter().any(|e| e.handle == handle && e.event == "dropped") {
                o.emits.push(super::tile::Emit { handle, event: "dropped".into(), payload: json!({})});
            }
        }
        o.emits.push(super::tile::Emit { handle, event: event.into(), payload });
    }

    /// Time limits: a card nobody answered in two minutes is a `Denied`, a call that takes over a minute a `Timeout`.
    pub fn tick(&self) {
        let now = self.now();
        let mut late: Vec<(u32, bool, SdkError)> = vec![];
        {
            let mut st = self.st();
            let ids: Vec<u32> = st.jobs.keys().copied().collect();
            for id in ids {
                let Some(j) = st.jobs.get(&id) else { continue };
                let stream = matches!(j.work, Work::Chat { stream: true, .. });
                match &j.stage {
                    Stage::Approval { since } if now.saturating_sub(*since) >= ANSWER_MS => late.push((id, stream, SdkError::new(code::DENIED, "the person did not answer in time"))),
                    Stage::Running { since, cancel } if now.saturating_sub(*since) >= JOB_MS => {
                        cancel.store(true, Ordering::SeqCst);
                        late.push((id, stream, SdkError::new(code::TIMEOUT, "the model took longer than 60 seconds")));
                    }
                    _ => continue,
                }
                st.jobs.remove(&id);
            }
        }
        for (id, stream, e) in late {
            self.out().requests.retain(|r| r.id != id);
            self.deliver(id, stream, Err(e));
        }
    }

    /// The tile is closed: nothing in flight is delivered, streams stop, readings stop.
    pub fn close(&self) {
        let mut st = self.st();
        for j in st.jobs.values() {
            if let Stage::Running { cancel, .. } = &j.stage {
                cancel.store(true, Ordering::SeqCst);
            }
        }
        st.jobs.clear();
        drop(st);
        self.speech.stop(&self.app, None);
    }

    /// Everything this person's log holds by source (tests; the Settings page reads the log itself).
    pub fn log_by_source(&self) -> Vec<Totals> {
        self.meter.by_source(&self.user)
    }

    /// Calls and requests still waiting (tests).
    pub fn waiting(&self) -> usize {
        self.st().jobs.len()
    }

    /// `T.fake.*`: only a runtime built with fakes answers.
    pub fn fake_op(&self, op: &str, a: &Value) -> Result<Value, String> {
        let f = self.fakes.as_ref().ok_or("no such bridge call: __test")?;
        let text = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match op {
            "llm.reply" => f.llm.reply(&text("text")),
            "llm.fail" => f.llm.fail(&text("code")),
            "llm.usage" => f.llm.usage(a["input"].as_u64().unwrap_or(0), a["output"].as_u64().unwrap_or(0)),
            "models.set" => {
                let list: Vec<ModelInfo> = a["models"].as_array().into_iter().flatten().map(|m| ModelInfo { id: m["id"].as_str().unwrap_or("").into(), label: m["label"].as_str().unwrap_or("").into(), provider: "fake".into(), place: match m["where"].as_str() { Some("network") => "network", Some("internet") => "internet", _ => "this computer" } }).collect();
                f.llm.set_models(list);
            }
            "speech.busy" => f.speech.set_busy(a.get("holder").and_then(Value::as_str)),
            "speech.said" => return Ok(Value::Array(f.speech.said().iter().map(|(t, o)| json!({"text": t, "voice": o.voice, "lang": o.lang, "speed": o.speed})).collect())),
            "approve.set" => {
                *f.approve.lock().unwrap_or_else(|e| e.into_inner()) = match text("mode").as_str() {
                    "allow" => Approve::Allow,
                    "deny" => Approve::Deny,
                    "ask" => Approve::Ask,
                    _ => return Err("approve: allow, deny or ask".into()),
                };
                if text("mode") == "ask" {
                    let mut st = self.st();
                    st.saved.llm = false;
                    st.saved.speech = false;
                }
            }
            "budget.set" => self.set_tokens_per_day(a["tokensPerDay"].as_u64()),
            "clock.advance" => {
                f.skew_ms.fetch_add(a["ms"].as_u64().unwrap_or(0), Ordering::SeqCst);
            }
            _ => return Err(format!("no such fake: {op}")),
        }
        Ok(Value::Null)
    }
}

fn quota(limit: &str) -> SdkError {
    SdkError::new(code::QUOTA, format!("too many: at most {limit}")).with("limit", limit)
}

fn denied(ns: &str) -> SdkError {
    SdkError::new(code::DENIED, if ns == "llm" { "the person did not allow this app to use the AI model" } else { "the person did not allow this app to read aloud" })
}

// ------------------------------------------------------------------------------------------------ arguments

fn check_keys(a: &Value, known: &[&str]) -> Result<(), SdkError> {
    let o = a.as_object().ok_or_else(|| SdkError::bad("arguments", "must be an object"))?;
    match o.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(SdkError::bad(&k.chars().take(40).collect::<String>(), "unknown option")),
        None => Ok(()),
    }
}

fn opt_str<'a>(a: &'a Value, k: &str, max: usize) -> Result<Option<&'a str>, SdkError> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| SdkError::bad(k, "must be a string"))?;
            if s.chars().count() > max || s.contains('\0') {
                return Err(SdkError::bad(k, "too long"));
            }
            Ok(Some(s))
        }
    }
}

/// `{messages, model?, max_tokens?, stream?}` checked the way the page checks it (and again here: a page may skip its own checks).
pub fn parse_chat(a: &Value) -> Result<ChatRequest, SdkError> {
    check_keys(a, &["messages", "model", "max_tokens", "stream"])?;
    let list = a.get("messages").and_then(Value::as_array).ok_or_else(|| SdkError::bad("messages", "must be a list of {role, content}"))?;
    if list.is_empty() || list.len() > MAX_MESSAGES {
        return Err(SdkError::bad("messages", "1 to 64 messages"));
    }
    let mut messages = Vec::new();
    let mut chars = 0usize;
    for m in list {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        if !["system", "user", "assistant"].contains(&role) {
            return Err(SdkError::bad("messages", "role is system, user or assistant"));
        }
        let content = m.get("content").and_then(Value::as_str).ok_or_else(|| SdkError::bad("messages", "content must be a string"))?;
        chars += content.chars().count();
        if chars > PROMPT_CHARS {
            return Err(SdkError::bad("messages", "more than 100000 characters in all"));
        }
        if content.contains('\0') {
            return Err(SdkError::bad("messages", "content has a NUL character"));
        }
        messages.push((role.to_string(), content.to_string()));
    }
    if !messages.iter().any(|m| m.0 == "user") {
        return Err(SdkError::bad("messages", "needs at least one user message"));
    }
    let model = opt_str(a, "model", 120)?.map(str::to_string);
    let max_tokens = match a.get("max_tokens") {
        None | Some(Value::Null) => DEFAULT_TOKENS,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| SdkError::bad("max_tokens", "a whole number from 1 to 4096"))?;
            if n == 0 || n > MAX_TOKENS {
                return Err(SdkError::bad("max_tokens", "from 1 to 4096"));
            }
            n
        }
    };
    if let Some(s) = a.get("stream") {
        if !s.is_boolean() {
            return Err(SdkError::bad("stream", "true or false"));
        }
    }
    Ok(ChatRequest { messages, model, max_tokens })
}

// ------------------------------------------------------------------------------------------------ the real model

/// The person's chat model (Settings -> Models) as the app's `llm`. The app never sees the provider, its address or its key: a call
/// goes through the same backend as the assistant's chat, which looks the provider up and reads the key from the vault.
pub struct ServicesLlm {
    pub svc: Arc<Services>,
}

fn place_of(cfg: &crate::providers::ProviderCfg) -> &'static str {
    use crate::speechtime::Locality;
    match cfg.base().map(|b| crate::speechtime::locality(&b)) {
        Ok(Locality::ThisComputer) => "this computer",
        Ok(Locality::Network) => "network",
        _ => "internet",
    }
}

impl LlmBackend for ServicesLlm {
    fn models(&self) -> Vec<ModelInfo> {
        let s = self.svc.settings.lock().unwrap_or_else(|e| e.into_inner());
        let st = s.get();
        let Some(c) = &st.chat else { return vec![] };
        let place = st.provider(&c.provider).map_or("internet", place_of);
        vec![ModelInfo { id: c.model.clone(), label: c.model.clone(), provider: c.provider.clone(), place }]
    }

    fn chat(&self, req: &ChatRequest, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatReply, SdkError> {
        let model = self.models().into_iter().next().ok_or_else(|| SdkError::new(code::UNAVAILABLE, "no chat model configured").with("fix", "Open Settings -> Models and pick a model."))?;
        let history: Vec<crate::chat::Message> = req
            .messages
            .iter()
            .map(|(r, c)| match r.as_str() {
                "system" => crate::chat::Message::system(c.clone()),
                "assistant" => crate::chat::Message::assistant(c.clone()),
                _ => crate::chat::Message::user(c.clone()),
            })
            .collect();
        crate::providers::forget_usage();
        // the provider takes no output limit through the common backend: stop reading when the answer is long enough
        let limit = req.max_tokens as usize * 4;
        let mut seen = 0usize;
        let backend = self.svc.chat_backend();
        let turn = backend
            .complete_tools_stream(&history, &[], &mut |piece| {
                seen += piece.chars().count();
                on_text(piece) && seen < limit
            })
            .map_err(|e| SdkError::new(code::FAILED, plain(&e)).with("detail", plain(&e)))?;
        let text: String = turn.text.chars().take(limit).collect();
        let reported = crate::providers::reported_usage();
        let (input, output) = reported.unwrap_or((crate::agent::estimate_tokens(&history), (text.chars().count() as u64).div_ceil(4)));
        Ok(ChatReply { text, input, output, estimated: reported.is_none(), model: model.id })
    }
}

// ------------------------------------------------------------------------------------------------ the real voice

static VOICES: Mutex<Vec<(u32, String, crate::voice::Player)>> = Mutex::new(Vec::new());
static NEXT_VOICE: AtomicU64 = AtomicU64::new(1);

/// Whether the speaker can take another program, and who holds it if not. `shared` is whether the output goes through a mixer
/// (`softvol` over `dmix`, or a sound server): then everyone plays at once. On the one-program speaker (`plughw`/`hw`, the T2 Mac)
/// the programs that have the device open are named; an app's own reading counts too.
pub fn speaker_holder(shared: bool, holders: &[String], mine: Option<&str>) -> Option<String> {
    if let Some(m) = mine {
        return Some(m.into());
    }
    if shared {
        return None;
    }
    holders.first().cloned()
}

/// `plughw:2,0` / `hw:2,0` -> (2, 0).
pub fn card_device(dev: &str) -> Option<(u32, u32)> {
    let (_, rest) = dev.split_once(':')?;
    let (c, d) = rest.split_once(',')?;
    Some((c.parse().ok()?, d.split(|ch: char| !ch.is_ascii_digit()).next()?.parse().ok()?))
}

/// The holders of the output `device` under `proc_root`, or none when it is shared.
pub fn exclusive_holders(proc_root: &Path, device: Option<&str>, shared: bool) -> Vec<String> {
    match (shared, device.and_then(card_device)) {
        (false, Some((c, d))) => crate::audioout::pcm_holders(proc_root, c, d),
        _ => vec![],
    }
}

pub struct SystemSpeech {
    svc: Arc<Services>,
}

impl SystemSpeech {
    pub fn new(svc: Arc<Services>) -> SystemSpeech {
        SystemSpeech { svc }
    }

    fn engine(&self, lang: Option<&str>) -> Result<crate::voice::SpeechEngine, SdkError> {
        match self.svc.effective_tts(lang) {
            None => Err(SdkError::new(code::UNAVAILABLE, "no text-to-speech engine is set up").with("fix", "Open Settings -> Voice and choose a voice.")),
            Some(crate::voice::SpeechEngine::Browser) => Err(SdkError::new(code::UNAVAILABLE, "the chosen voice speaks only inside the browser").with("fix", "Choose another voice in Settings -> Voice.")),
            Some(e) => Ok(e),
        }
    }

    fn reap(v: &mut Vec<(u32, String, crate::voice::Player)>) {
        v.retain_mut(|(_, _, p)| p.is_playing());
    }
}

impl SpeechBackend for SystemSpeech {
    fn say(&self, app: &str, text: &str, opts: &SayOpts) -> Result<u32, SdkError> {
        let engine = self.engine(opts.lang.as_deref())?;
        if let Some((d, _)) = crate::voicedesk::consent_text(&self.svc, &engine, false) {
            return Err(SdkError::new(code::BLOCKED, format!("the voice server {} is not allowed to get text yet", d.name)).with("fix", "Allow it once in the VOICE tile or Settings -> Models."));
        }
        let status = crate::audioout::global().status();
        // with the IYAKKI mixer on (Settings -> Sound) programs share the speaker: priority and ducking decide, not Busy
        let shared = status.mix.is_some() || status.device.is_none() || crate::audiosink::mixing();
        let holders = exclusive_holders(Path::new("/proc"), status.device.as_deref(), shared);
        {
            let mut v = VOICES.lock().unwrap_or_else(|e| e.into_inner());
            Self::reap(&mut v);
            let mine = v.iter().find(|(_, a, _)| !shared || a == app).map(|(_, a, _)| if a == app { "this app".to_string() } else { format!("the app {a}") });
            if let Some(h) = speaker_holder(shared, &holders, mine.as_deref()) {
                return Err(busy(&h));
            }
        }
        let v = self.svc.voice_settings();
        let sopts = crate::voice::SpeakOpts { voice: opts.voice.clone(), speed: opts.speed.unwrap_or(v.speed_pct as f32 / 100.0), lang: opts.lang.clone() };
        let up = self.svc.slow_upstream();
        let wav = crate::voice::synthesize_with(&engine, text, &sopts, &self.svc.secrets, up.as_ref()).map_err(|e| SdkError::new(code::FAILED, plain(&crate::voicedesk::explain_tts(&engine, &e))))?;
        let player = crate::voice::Player::start(&v.play_cmd, &wav).map_err(|e| SdkError::new(code::UNAVAILABLE, plain(&e)).with("fix", "Check the play command in Settings -> Voice."))?;
        let id = NEXT_VOICE.fetch_add(1, Ordering::SeqCst) as u32;
        VOICES.lock().unwrap_or_else(|e| e.into_inner()).push((id, app.into(), player));
        Ok(id)
    }

    fn stop(&self, app: &str, id: Option<u32>) {
        let mut v = VOICES.lock().unwrap_or_else(|e| e.into_inner());
        v.retain_mut(|(i, a, p)| {
            let mine = a == app && id.is_none_or(|x| x == *i);
            if mine {
                p.stop();
            }
            !mine
        });
    }

    fn destination(&self) -> String {
        match self.svc.effective_tts(None) {
            Some(e) => crate::voicedesk::where_text(&self.svc, &e, "Text"),
            None => "No voice is set up yet (Settings -> Voice).".into(),
        }
    }
}
