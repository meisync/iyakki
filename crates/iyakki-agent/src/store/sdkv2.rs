//! The bridge side of SDK 1.2 (docs/development/SDK-BRIDGE-V2.md, SDK-API.md section 14): discovery, `markdown`, `embed`, `dash`, `flows`, `k3s`,
//! `media`, `audio`, `speech.listen` and `money`. `TileHost` (store/tile.rs) checks that the call is known to this SDK version and that the app holds the
//! capability, then hands it here. This file checks the arguments (unknown ones are ignored and reported back), keeps the limits, asks the person with a
//! card where the manifest table says to, writes the audit lines, and only then asks a [`Backend`] to do the work. A backend that is not wired answers
//! `Unavailable` with a fix line; the interface never changes when one is wired.
//!
//! Cards are `sdk.card` requests: the shell draws the words the host wrote (title and lines) with Allow and Don't allow, and answers with
//! `TileRuntime::answer_sdk`. A card that asks every time is never answered by a setting: only the person answers it (the fake first-use switch of the
//! headless harness does not touch it).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_core_api::sdkbridge::{self as bridge, Version};
use serde_json::{json, Map, Value};

use super::sdkv2_backends::{Backends, FakeBackend, Who};
use super::sdkv2_money::AppFile;
use super::tile::{Caps, Done, Outbox, Request};
use super::tilesdk::{code, SdkError, ANSWER_MS};
use crate::clock::Zone;

pub const MAX_CARDS: usize = 4;
pub const QUERIES_PER_MINUTE: usize = 30;
pub const QUERY_CHARS: usize = 8000;
pub const MAX_ROWS: usize = 1000;
pub const FLOW_RUNS_PER_HOUR: usize = 10;
pub const FLOW_INPUT_BYTES: usize = 16 * 1024;
pub const LISTENS_PER_HOUR: usize = 20;
pub const LISTEN_SECONDS: u64 = 60;
pub const VIEWS_PER_HOUR: usize = 20;
pub const MAX_EMBEDS: usize = 2;
pub const WAV_BASE64_CHARS: usize = 2_796_203;
pub const K3S_YAML_BYTES: usize = 64 * 1024;
pub const K3S_ITEMS: usize = 200;
pub const LOG_LINES: usize = 500;

pub(super) type ThenFn = Box<dyn FnOnce(&Sdk2) -> Result<Value, SdkError> + Send>;
type Then = ThenFn;

struct Pending {
    since: u64,
    first_use: Option<String>,
    op: String,
    then: Then,
}

#[derive(Default)]
struct St {
    embeds: BTreeMap<u32, String>,
    queries: Vec<u64>,
    flow_runs: Vec<u64>,
    views: Vec<u64>,
    listens: Vec<u64>,
    runs: BTreeSet<String>,
    run_keys: HashMap<String, String>,
    listening: Option<(u32, u64)>,
    audio: Option<u32>,
    media: BTreeSet<u32>,
    pending: HashMap<u32, Pending>,
    /// First-use answers when the app has no state file (tests, the emulator).
    memory: AppFile,
}

/// What a test can reach (`T.fake.v2`).
pub struct Fake2 {
    pub backend: Arc<FakeBackend>,
    /// Answers waiting for the next cards that ask every time (`true` allows). Empty: the card is shown and the person answers.
    pub answers: Mutex<std::collections::VecDeque<bool>>,
    /// How first-use cards are answered: `Some(true)` allow (the default), `Some(false)` deny, `None` ask.
    pub first_use: Mutex<Option<bool>>,
}

pub struct Sdk2 {
    app: String,
    user: String,
    out: Arc<Mutex<Outbox>>,
    pub(super) be: Backends,
    st: Mutex<St>,
    file: Option<PathBuf>,
    zone: Zone,
    level: Version,
    pub(super) skew_ms: AtomicU64,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    fakes: Option<Fake2>,
}

// ------------------------------------------------------------------------------------------------ small helpers

fn unsupported_error(op: &str, host: Version) -> SdkError {
    let p = bridge::unsupported(op, host);
    let mut e = SdkError::new(code::UNSUPPORTED, p["message"].as_str().unwrap_or("not available in this version").to_string());
    for k in ["op", "namespace", "sdk"] {
        e = e.with(k, p[k].clone());
    }
    e
}

pub fn unsupported_encoded(op: &str, level: Version) -> String {
    unsupported_error(op, level).encode()
}

fn obj(a: &Value) -> Result<&Map<String, Value>, SdkError> {
    a.as_object().ok_or_else(|| SdkError::bad("arguments", "must be an object"))
}

/// The argument names this call does not know, to be ignored and reported.
pub(super) fn unknown(a: &Value, known: &[&str]) -> Vec<String> {
    a.as_object().map(|o| o.keys().filter(|k| !known.contains(&k.as_str())).take(10).map(|k| k.chars().filter(|c| !c.is_control()).take(40).collect()).collect()).unwrap_or_default()
}

/// Add `ignored` to an object reply when something was ignored. Replies are always objects so a field can be added later.
pub(super) fn with_ignored(mut v: Value, ignored: Vec<String>) -> Value {
    if !ignored.is_empty() {
        if let Some(o) = v.as_object_mut() {
            o.insert("ignored".into(), json!(ignored));
        }
    }
    v
}

pub(super) fn req_str<'a>(a: &'a Value, k: &str, max: usize) -> Result<&'a str, SdkError> {
    let s = a.get(k).and_then(Value::as_str).ok_or_else(|| SdkError::bad(k, "must be a string"))?;
    if s.chars().count() > max || s.contains('\0') {
        return Err(SdkError::bad(k, &format!("longer than {max} characters")));
    }
    Ok(s)
}

fn opt_text<'a>(a: &'a Value, k: &str, max: usize) -> Result<Option<&'a str>, SdkError> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => req_str(a, k, max).map(Some),
    }
}

pub(super) fn opt_uint(a: &Value, k: &str, min: u64, max: u64) -> Result<Option<u64>, SdkError> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| SdkError::bad(k, "must be a whole number"))?;
            if n < min || n > max {
                return Err(SdkError::bad(k, &format!("from {min} to {max}")));
            }
            Ok(Some(n))
        }
    }
}

pub(super) fn req_uint(a: &Value, k: &str) -> Result<u64, SdkError> {
    a.get(k).and_then(Value::as_u64).ok_or_else(|| SdkError::bad(k, "must be a whole number"))
}

pub(super) fn plain_line(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

pub(super) fn path_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 100 && !p.starts_with('/') && p.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.starts_with('.')) && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

fn dns_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-.".contains(c)) && !s.starts_with(['-', '.']) && !s.ends_with(['-', '.'])
}

fn within(v: &mut Vec<u64>, now: u64, span: u64, max: usize) -> bool {
    v.retain(|t| now.saturating_sub(*t) < span);
    if v.len() >= max {
        return false;
    }
    v.push(now);
    true
}

fn quota(limit: &str) -> SdkError {
    SdkError::new(code::QUOTA, format!("too many: at most {limit}")).with("limit", limit)
}

fn no_cap(what: &str) -> SdkError {
    SdkError::new(code::NO_CAPABILITY, format!("this app did not ask for {what}: add it to capabilities in app.json (the person sees it when installing)"))
}

fn list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

pub(super) fn items_of(v: Value, cap: usize) -> Value {
    let mut a = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => o.remove("items").and_then(|x| if let Value::Array(a) = x { Some(a) } else { None }).unwrap_or_default(),
        _ => vec![],
    };
    a.truncate(cap);
    json!({ "items": a })
}

// ------------------------------------------------------------------------------------------------ the pure calls

/// The calls that need no state and no backend: discovery and `markdown.render`. `TileHost` answers them even when the build has no SDK part.
/// `None`: not one of these.
pub fn pure_call(op: &str, a: &Value, caps: &Caps, level: Version) -> Option<Result<Value, SdkError>> {
    let granted = |k: &str| caps.holds(k);
    match op {
        "sdk.capabilities" => Some(Ok(bridge::listing(level, &granted))),
        "sdk.has" => Some(req_str(a, "name", 80).map(|n| json!(bridge::has(level, n, &granted)))),
        "markdown.render" => Some(markdown_render(a)),
        _ => None,
    }
}

fn markdown_render(a: &Value) -> Result<Value, SdkError> {
    obj(a)?;
    let ignored = unknown(a, &["text", "width"]);
    let text = a.get("text").and_then(Value::as_str).ok_or_else(|| SdkError::bad("text", "must be a string"))?;
    let width = opt_uint(a, "width", 0, 4000)?.unwrap_or(0) as u32;
    let r = bridge::markdown::analyze(text, width).map_err(|e| SdkError::bad("text", &e))?;
    Ok(with_ignored(r.to_json(), ignored))
}

// ------------------------------------------------------------------------------------------------ construction

impl Sdk2 {
    pub fn new(app: &str, user: &str, out: Arc<Mutex<Outbox>>, be: Backends, file: Option<PathBuf>, zone: Zone, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Sdk2 {
        Sdk2 { app: app.into(), user: user.into(), out, be, st: Mutex::new(St::default()), file, zone, level: Version::CURRENT, skew_ms: AtomicU64::new(0), clock, fakes: None }
    }

    /// Fake backends and cards that wait for the test (`T.fake.v2`).
    pub fn with_fakes(mut self) -> Sdk2 {
        let (be, backend) = Backends::fake();
        self.be = be;
        self.fakes = Some(Fake2 { backend, answers: Mutex::new(Default::default()), first_use: Mutex::new(Some(true)) });
        self
    }

    pub fn with_zone(mut self, zone: Zone) -> Sdk2 {
        self.zone = zone;
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Sdk2 {
        self.clock = clock;
        self
    }

    /// `T.fake.v2.*`: the person's answers, the backends' replies and what they were asked. Only a runtime built with fakes answers.
    pub fn fake_op(&self, op: &str, a: &Value) -> Result<Value, String> {
        let f = self.fakes.as_ref().ok_or("no such bridge call: __test")?;
        let text = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match op {
            "answer" => f.answers.lock().unwrap_or_else(|e| e.into_inner()).push_back(a.get("allow").and_then(Value::as_bool).unwrap_or(false)),
            "reply" => f.backend.reply(&text("op"), a.get("value").cloned().unwrap_or(Value::Null)),
            "fail" => f.backend.fail(&text("op"), super::tilesdk::code_named(&text("code")), &text("message")),
            "calls" => return Ok(json!(f.backend.calls().into_iter().filter(|(o, _)| text("op").is_empty() || *o == text("op")).map(|(o, a)| json!({"op": o, "args": a})).collect::<Vec<_>>())),
            "heard" => {
                let h = self.st().listening.map(|(h, _)| h).ok_or("nothing is listening")?;
                self.heard(h, Ok(text("text")));
            }
            "event" => {
                let h = a.get("handle").and_then(Value::as_u64).ok_or("handle: a number")? as u32;
                self.event(h, &text("event"), a.get("payload").cloned().unwrap_or(json!({})));
            }
            "pending" => return Ok(json!(self.st().pending.len())),
            "limit" => {
                let (cur, per) = (text("currency"), a.get("perDayMinor").and_then(Value::as_u64).unwrap_or(0));
                let confirm = a.get("confirmEach").and_then(Value::as_bool);
                self.with_file(true, |file| {
                    file.money.currency = cur;
                    file.money.per_day_minor = per;
                    if let Some(c) = confirm {
                        file.money.confirm_each = c;
                    }
                });
            }
            _ => return Err(format!("no such fake: v2.{op}")),
        }
        Ok(Value::Null)
    }

    pub fn with_backends(mut self, be: Backends) -> Sdk2 {
        self.be = be;
        self
    }

    /// Play an older host (tests): calls newer than `level` are unknown.
    pub fn with_level(mut self, level: Version) -> Sdk2 {
        self.level = level;
        self
    }

    pub fn level(&self) -> Version {
        self.level
    }

    pub fn fakes(&self) -> Option<&Fake2> {
        self.fakes.as_ref()
    }

    pub(super) fn who(&self) -> Who {
        Who { app: self.app.clone(), user: self.user.clone() }
    }

    pub(super) fn app(&self) -> &str {
        &self.app
    }

    pub(super) fn zone(&self) -> &Zone {
        &self.zone
    }

    pub(super) fn now(&self) -> u64 {
        (self.clock)() + self.skew_ms.load(Ordering::SeqCst)
    }

    /// Unix seconds, moved by the fake clock's skew (a day of money limits ends at the person's midnight).
    pub(super) fn unix(&self) -> i64 {
        crate::clock::now_unix() + (self.skew_ms.load(Ordering::SeqCst) / 1000) as i64
    }

    fn st(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn out(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.out.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn audit(&self, action: &str, detail: &str) {
        let mut o = self.out();
        if o.audit.len() < 100 {
            o.audit.push((action.into(), plain_line(detail, 200)));
        }
    }

    /// Read (and, when `write`, save) the app's state file under the process-wide lock.
    pub(super) fn with_file<R>(&self, write: bool, f: impl FnOnce(&mut AppFile) -> R) -> R {
        match &self.file {
            Some(p) => AppFile::edit(p, write, f),
            None => {
                let mut st = self.st();
                f(&mut st.memory)
            }
        }
    }

    fn first_used(&self, key: &str) -> bool {
        self.with_file(false, |f| f.first_use.iter().any(|k| k == key))
    }

    fn remember_first_use(&self, key: &str) {
        self.with_file(true, |f| {
            if !f.first_use.iter().any(|k| k == key) && f.first_use.len() < 200 {
                f.first_use.push(key.into());
            }
        });
    }

    /// The person takes back the first-use answers of SDK 1.2 (Info -> Revoke).
    pub fn revoke(&self) {
        self.with_file(true, |f| f.first_use.clear());
    }

    pub fn ba(&self, op: &str, backend: &Arc<dyn super::sdkv2_backends::Backend>, args: &Value) -> Result<Value, SdkError> {
        backend.call(&self.who(), op, args)
    }

    // ---- delivering

    fn deliver(&self, id: u32, r: Result<Value, SdkError>) {
        let d = match r {
            Ok(v) => Done { id, ok: true, value: v.to_string() },
            Err(e) => Done { id, ok: false, value: e.encode() },
        };
        self.out().done.push(d);
    }

    /// An event for a handle the page holds (an embed, a listen). Backends call this from their own threads. At most 50 wait per handle.
    pub fn event(&self, handle: u32, event: &str, payload: Value) {
        let mut o = self.out();
        let waiting = o.emits.iter().filter(|e| e.handle == handle).count();
        if waiting >= super::tilesdk::EVENTS_PER_HANDLE {
            if let Some(i) = o.emits.iter().position(|e| e.handle == handle && e.event != "done" && e.event != "error") {
                o.emits.remove(i);
            }
            if !o.emits.iter().any(|e| e.handle == handle && e.event == "dropped") {
                o.emits.push(super::tile::Emit { handle, event: "dropped".into(), payload: json!({}) });
            }
        }
        o.emits.push(super::tile::Emit { handle, event: event.into(), payload });
    }

    // ---- cards

    /// Ask the person. `first_use`: the key under which the answer is kept (`None`: asked every time). The call returns `{pending}`; `then` runs on Allow.
    pub(super) fn ask(&self, op: &str, first_use: Option<&str>, title: &str, lines: Vec<String>, then: Then) -> Result<Value, SdkError> {
        // a first-use card that was answered once is not asked again
        if let Some(k) = first_use {
            if self.first_used(k) {
                return then(self);
            }
            let mode = self.fakes.as_ref().map(|f| *f.first_use.lock().unwrap_or_else(|e| e.into_inner()));
            match mode {
                Some(Some(true)) => {
                    self.remember_first_use(k);
                    return then(self);
                }
                Some(Some(false)) => return Err(denied()),
                _ => {}
            }
        } else if let Some(f) = &self.fakes {
            // the headless harness queues the person's answers; a real runtime has no such queue
            let next = f.answers.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            match next {
                Some(true) => return then(self),
                Some(false) => return Err(denied()),
                None => {}
            }
        }
        let now = self.now();
        let mut st = self.st();
        if st.pending.len() >= MAX_CARDS {
            return Err(quota("4 questions waiting for the person"));
        }
        let id = self.out().alloc();
        st.pending.insert(id, Pending { since: now, first_use: first_use.map(str::to_string), op: op.into(), then });
        drop(st);
        let args = json!({"ns": op, "app": self.app, "title": plain_line(title, 80), "lines": lines.iter().map(|l| plain_line(l, 240)).collect::<Vec<_>>(), "everyTime": first_use.is_none()});
        self.out().requests.push(Request { id, kind: "sdk.card".into(), args });
        Ok(json!({"pending": id}))
    }

    /// The person answered a card. False when there was no such card.
    pub fn answer(&self, id: u32, allow: bool) -> bool {
        let Some(p) = self.st().pending.remove(&id) else { return false };
        self.out().requests.retain(|r| r.id != id);
        if !allow {
            self.audit("app.sdk.deny", &format!("{} was refused {}", self.app, p.op));
            self.deliver(id, Err(denied()));
            return true;
        }
        if let Some(k) = &p.first_use {
            self.remember_first_use(k);
        }
        self.audit("app.sdk.allow", &format!("{} may {}", self.app, p.op));
        let r = (p.then)(self);
        self.deliver(id, r);
        true
    }

    /// Time limits: a card nobody answered in two minutes is a `Denied`; a listen that never ended is a `Timeout`.
    pub fn tick(&self) {
        let now = self.now();
        let late: Vec<u32> = self.st().pending.iter().filter(|(_, p)| now.saturating_sub(p.since) >= ANSWER_MS).map(|(i, _)| *i).collect();
        for id in late {
            self.st().pending.remove(&id);
            self.out().requests.retain(|r| r.id != id);
            self.deliver(id, Err(SdkError::new(code::DENIED, "the person did not answer in time")));
        }
        let over = self.st().listening.filter(|(_, until)| now >= *until).map(|(i, _)| i);
        if let Some(h) = over {
            self.st().listening = None;
            let _ = self.ba("speech.cancel", &self.be.listener, &json!({"id": h}));
            self.event(h, "error", SdkError::new(code::TIMEOUT, "the microphone heard nothing it could use in time").payload());
        }
    }

    /// The tile closed: embeds are unmounted, listening and sound stop, cards nobody answered vanish.
    pub fn close(&self) {
        let (embeds, listening, audio) = {
            let mut st = self.st();
            st.pending.clear();
            (std::mem::take(&mut st.embeds), st.listening.take(), st.audio.take())
        };
        for id in embeds.keys() {
            let _ = self.ba("embed.unmount", &self.be.embed, &json!({"id": id}));
        }
        if let Some((h, _)) = listening {
            let _ = self.ba("speech.cancel", &self.be.listener, &json!({"id": h}));
        }
        if let Some(id) = audio {
            let _ = self.ba("audio.stop", &self.be.playback, &json!({"id": id}));
        }
    }

    pub(super) fn media_add(&self, id: u32) {
        self.st().media.insert(id);
    }

    pub(super) fn media_has(&self, id: u32) -> bool {
        self.st().media.contains(&id)
    }

    pub(super) fn forget_media(&self, id: u32) {
        self.st().media.remove(&id);
    }

    pub(super) fn audio_id(&self) -> Option<u32> {
        self.st().audio
    }

    pub(super) fn set_audio(&self, id: Option<u32>) {
        self.st().audio = id;
    }

    pub fn waiting(&self) -> usize {
        self.st().pending.len()
    }

    // ---- dispatch

    /// One SDK 1.2 call. The caller (`TileHost`) has already made sure the name is known to this SDK version and that the app holds the capability
    /// it needs; this still checks the grant's parameters.
    pub fn call(&self, op: &str, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        if let Some(r) = pure_call(op, a, caps, self.level) {
            return r;
        }
        let a = if a.is_null() { &Value::Null } else { a };
        let empty = json!({});
        let a = if a.is_null() { &empty } else { a };
        obj(a)?;
        match op {
            "markdown.view" => self.markdown_view(a),
            "markdown.openFile" => self.markdown_open(a),
            "embed.mount" => self.embed_mount(a, caps),
            "embed.update" => self.embed_update(a),
            "embed.call" => self.embed_call(a),
            "embed.unmount" => self.embed_unmount(a),
            "embed.list" => Ok(json!({"items": self.st().embeds.iter().map(|(i, k)| json!({"id": i, "kind": k})).collect::<Vec<_>>()})),
            "dash.sources" => self.dash_sources(a, caps),
            "dash.query" => self.dash_query(a, caps),
            "dash.panel.add" => self.dash_panel(a, caps),
            "flows.list" => self.flows_list(a, caps),
            "flows.run" => self.flows_run(a, caps),
            "flows.status" => self.flows_status(a),
            "flows.cancel" => self.flows_cancel(a),
            "k3s.status" => self.k3s_status(caps),
            "k3s.get" => self.k3s_get(a),
            "k3s.logs" => self.k3s_logs(a),
            "k3s.apply" => self.k3s_apply(a, caps),
            "k3s.restart" => self.k3s_restart(a, caps),
            "media.library" | "media.play" | "media.pause" | "media.resume" | "media.stop" | "media.state" | "media.queue.add" | "media.queue.list" => self.media(op, a),
            "audio.play" | "audio.pause" | "audio.resume" | "audio.stop" | "audio.setVolume" | "audio.outputs" => self.audio(op, a),
            "speech.listen" => self.listen(a),
            "speech.cancel" => self.listen_cancel(a),
            "money.limits" | "money.spend" | "money.requestLimit" => self.money(op, a, caps),
            other => Err(unsupported_error(other, self.level)),
        }
    }

    // ---- markdown

    fn markdown_view(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["text", "title"]);
        let text = req_str(a, "text", bridge::markdown::MAX_TEXT)?;
        let title = opt_text(a, "title", 60)?.map(|t| plain_line(t, 60)).unwrap_or_default();
        if text.trim().is_empty() {
            return Err(SdkError::bad("text", "nothing to show"));
        }
        if !within(&mut self.st().views, self.now(), 3_600_000, VIEWS_PER_HOUR) {
            return Err(quota("20 viewers an hour"));
        }
        let r = self.ba("markdown.view", &self.be.viewer, &json!({"title": title, "text": text}))?;
        Ok(with_ignored(json!({"id": r.get("id").cloned().unwrap_or(Value::Null)}), ignored))
    }

    fn markdown_open(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &[]);
        let app = self.app.clone();
        self.ask("markdown.openFile", None, "An app wants to open a Markdown file", vec![format!("“{app}” would like to show a Markdown file in the viewer. You choose the file; the app never sees where it is.")], Box::new(move |s| {
            let r = s.ba("markdown.openFile", &s.be.viewer, &json!({}))?;
            Ok(with_ignored(json!({"id": r.get("id").cloned().unwrap_or(Value::Null)}), ignored))
        }))
    }

    // ---- embed

    fn embed_mount(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["target", "kind", "options"]);
        let kind = req_str(a, "kind", 16)?.to_string();
        if !bridge::EMBED_KINDS.contains(&kind.as_str()) {
            return Err(SdkError::bad("kind", "markdown, browser, media or dash"));
        }
        if !list(caps.v2.get("embed").unwrap_or(&Value::Null)).contains(&kind) {
            return Err(no_cap(&format!("embedding a {kind} viewer (embed: [\"{kind}\"])")));
        }
        let target = match a.get("target") {
            Some(Value::String(s)) => {
                let ok = !s.is_empty() && s.len() <= 64 && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                if !ok {
                    return Err(SdkError::bad("target", "an element id (letters, digits, - and _) or {x, y, w, h}"));
                }
                json!(s)
            }
            Some(Value::Object(o)) => {
                let n = |k: &str| o.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= 100_000.0);
                let (Some(x), Some(y), Some(w), Some(h)) = (n("x"), n("y"), n("w"), n("h")) else { return Err(SdkError::bad("target", "{x, y, w, h} in page pixels")) };
                if w < 120.0 || h < 80.0 || w > 900.0 || h > 2880.0 {
                    return Err(SdkError::bad("target", "at least 120 x 80 and at most 900 x 2880"));
                }
                json!({"x": x, "y": y, "w": w, "h": h})
            }
            _ => return Err(SdkError::bad("target", "an element id or {x, y, w, h}")),
        };
        let options = match a.get("options") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(SdkError::bad("options", "must be an object")),
        };
        if options.to_string().len() > bridge::markdown::MAX_TEXT + 4096 {
            return Err(SdkError::bad("options", "too large"));
        }
        let hosts = list(caps.v2.get("embed_hosts").unwrap_or(&Value::Null));
        match kind.as_str() {
            "browser" => {
                let url = options.get("url").and_then(Value::as_str).ok_or_else(|| SdkError::bad("options.url", "an http or https address"))?;
                let host = url_host(url).ok_or_else(|| SdkError::bad("options.url", "an http or https address"))?;
                if !hosts.contains(&host) {
                    return Err(SdkError::new(code::BLOCKED, format!("{host} is not in this app's embed_hosts")).with("host", host));
                }
            }
            "markdown" => {
                let (t, f) = (options.get("text"), options.get("file"));
                match (t, f) {
                    (Some(Value::String(_)), None) => {}
                    (None, Some(Value::String(p))) if path_ok(p) => {}
                    _ => return Err(SdkError::bad("options", "{text} or {file} (a path inside the app)")),
                }
            }
            "media" => {
                if !options.get("file").and_then(Value::as_str).is_some_and(path_ok) {
                    return Err(SdkError::bad("options.file", "a path inside the app"));
                }
            }
            _ => {
                if !options.get("dashboard").and_then(Value::as_str).is_some_and(|d| !d.is_empty() && d.len() <= 64) {
                    return Err(SdkError::bad("options.dashboard", "the name of a dashboard"));
                }
            }
        }
        {
            let st = self.st();
            if st.embeds.len() >= MAX_EMBEDS {
                return Err(quota("2 embedded viewers"));
            }
            if kind == "browser" && st.embeds.values().any(|k| k == "browser") {
                return Err(quota("1 embedded browser"));
            }
        }
        let id = self.out().alloc();
        let app = self.app.clone();
        let run: Then = {
            let (kind, hosts) = (kind.clone(), hosts.clone());
            Box::new(move |s| {
                let args = json!({"id": id, "kind": kind, "target": target, "options": options, "hosts": hosts});
                s.ba("embed.mount", &s.be.embed, &args)?;
                s.st().embeds.insert(id, kind);
                Ok(with_ignored(json!({"id": id}), ignored))
            })
        };
        if kind == "browser" {
            let lines = vec![format!("“{app}” would like to show a web page inside itself."), format!("It may visit only: {}.", hosts.join(", ")), "The page has its own cookies, no saved logins and no downloads. You are asked once.".into()];
            return self.ask("embed.mount", Some("embed.browser"), "An app wants to show a web page inside itself", lines, run);
        }
        run(self)
    }

    fn embed_of(&self, a: &Value) -> Result<(u32, String), SdkError> {
        let id = req_uint(a, "id")? as u32;
        let kind = self.st().embeds.get(&id).cloned().ok_or_else(|| SdkError::bad("id", "not an embedded viewer of this app"))?;
        Ok((id, kind))
    }

    fn embed_update(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["id", "options"]);
        let (id, kind) = self.embed_of(a)?;
        let options = a.get("options").filter(|v| v.is_object()).cloned().ok_or_else(|| SdkError::bad("options", "must be an object"))?;
        if kind == "browser" && options.get("url").is_some() {
            return Err(SdkError::bad("options.url", "a browser embed cannot be sent elsewhere: unmount it and mount again"));
        }
        self.ba("embed.update", &self.be.embed, &json!({"id": id, "options": options}))?;
        Ok(with_ignored(json!({"id": id}), ignored))
    }

    fn embed_call(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["id", "method", "args"]);
        let (id, kind) = self.embed_of(a)?;
        let method = req_str(a, "method", 32)?;
        let allowed: &[&str] = match kind.as_str() {
            "markdown" => &["scrollTo", "find", "focus", "blur"],
            "browser" => &["back", "forward", "reload", "stop", "focus", "blur"],
            "media" => &["play", "pause", "seek", "focus", "blur"],
            _ => &["setRange", "focus", "blur"],
        };
        if !allowed.contains(&method) {
            return Err(SdkError::bad("method", &format!("a {kind} viewer knows {}", allowed.join(", "))));
        }
        let args = match a.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(SdkError::bad("args", "must be an object")),
        };
        let r = self.ba("embed.call", &self.be.embed, &json!({"id": id, "method": method, "args": args}))?;
        Ok(with_ignored(json!({"result": r}), ignored))
    }

    fn embed_unmount(&self, a: &Value) -> Result<Value, SdkError> {
        let (id, _) = self.embed_of(a)?;
        self.st().embeds.remove(&id);
        let _ = self.ba("embed.unmount", &self.be.embed, &json!({"id": id}));
        Ok(json!({}))
    }

    // ---- dash

    fn granted(caps: &Caps, key: &str) -> Vec<String> {
        list(caps.v2.get(key).unwrap_or(&Value::Null))
    }

    fn dash_sources(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &[]);
        let names = Self::granted(caps, "dash");
        let r = self.ba("dash.sources", &self.be.dash, &json!({"names": names}))?;
        let mut v = items_of(r, 100);
        if let Some(a) = v["items"].as_array_mut() {
            a.retain(|s| s["name"].as_str().is_some_and(|n| names.iter().any(|g| g == n)));
        }
        Ok(with_ignored(v, ignored))
    }

    fn dash_query(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["source", "query", "range", "limit"]);
        let source = req_str(a, "source", 64)?.to_string();
        if !Self::granted(caps, "dash").contains(&source) {
            return Err(no_cap(&format!("the dashboard source {source:?} (dash: [\"{source}\"])")));
        }
        let query = req_str(a, "query", QUERY_CHARS)?.to_string();
        if query.trim().is_empty() {
            return Err(SdkError::bad("query", "empty"));
        }
        let limit = opt_uint(a, "limit", 1, MAX_ROWS as u64)?.unwrap_or(MAX_ROWS as u64) as usize;
        let range = match a.get("range") {
            None | Some(Value::Null) => Value::Null,
            Some(Value::Object(o)) => {
                let (f, t) = (o.get("from").and_then(Value::as_str), o.get("to").and_then(Value::as_str));
                match (f, t) {
                    (Some(f), Some(t)) if f.len() <= 64 && t.len() <= 64 && !f.contains('\0') && !t.contains('\0') => json!({"from": f, "to": t}),
                    _ => return Err(SdkError::bad("range", "{from, to} as short texts")),
                }
            }
            Some(_) => return Err(SdkError::bad("range", "{from, to}")),
        };
        if !within(&mut self.st().queries, self.now(), 60_000, QUERIES_PER_MINUTE) {
            return Err(quota("30 dashboard queries a minute"));
        }
        let key = format!("dash.query:{source}");
        let app = self.app.clone();
        let lines = vec![format!("“{app}” would like to read data from your dashboard source “{source}”. It can only read, never change anything."), "You are asked once for this source; you can take it back later.".into()];
        self.ask("dash.query", Some(&key), "An app wants to read dashboard data", lines, Box::new(move |s| {
            let r = s.ba("dash.query", &s.be.dash, &json!({"source": source, "query": query, "range": range, "limit": limit}))?;
            let mut cols = r.get("columns").cloned().unwrap_or(Value::Null);
            let mut rows = r.get("rows").and_then(Value::as_array).cloned().ok_or_else(|| SdkError::new(code::FAILED, "the data source gave no rows"))?;
            if !cols.is_array() {
                cols = json!([]);
            }
            let mut truncated = r.get("truncated").and_then(Value::as_bool).unwrap_or(false);
            if rows.len() > limit {
                rows.truncate(limit);
                truncated = true;
            }
            Ok(with_ignored(json!({"columns": cols, "rows": rows, "truncated": truncated}), ignored))
        }))
    }

    fn dash_panel(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["dashboard", "panel"]);
        let dashboard = req_str(a, "dashboard", 64)?.to_string();
        let p = a.get("panel").filter(|p| p.is_object()).ok_or_else(|| SdkError::bad("panel", "must be an object"))?;
        let title = req_str(p, "title", 80)?.to_string();
        let source = req_str(p, "source", 64)?.to_string();
        if !Self::granted(caps, "dash").contains(&source) {
            return Err(no_cap(&format!("the dashboard source {source:?} (dash: [\"{source}\"])")));
        }
        let query = req_str(p, "query", QUERY_CHARS)?.to_string();
        let visual = opt_text(p, "visual", 32)?.unwrap_or("timeseries").to_string();
        let app = self.app.clone();
        let lines = vec![format!("“{app}” would like to add a panel to your dashboard “{dashboard}”."), format!("Panel: {title}  (source {source})"), format!("Query: {}", plain_line(&query, 160))];
        self.ask("dash.panel.add", None, "An app wants to add a dashboard panel", lines, Box::new(move |s| {
            let r = s.ba("dash.panel.add", &s.be.dash, &json!({"dashboard": dashboard, "panel": {"title": title, "source": source, "query": query, "visual": visual}}))?;
            s.audit("app.dash.panel", &format!("{} added a panel to {}", s.app, dashboard));
            Ok(with_ignored(json!({"id": r.get("id").cloned().unwrap_or(Value::Null)}), ignored))
        }))
    }

    // ---- flows

    fn flows_list(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &[]);
        let names = Self::granted(caps, "flows");
        let r = self.ba("flows.list", &self.be.flows, &json!({"names": names}))?;
        let mut v = items_of(r, 100);
        if let Some(a) = v["items"].as_array_mut() {
            a.retain(|f| f["name"].as_str().is_some_and(|n| names.iter().any(|g| g == n)));
        }
        Ok(with_ignored(v, ignored))
    }

    fn flows_run(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["flow", "inputs", "key"]);
        let flow = req_str(a, "flow", 64)?.to_string();
        if !Self::granted(caps, "flows").contains(&flow) {
            return Err(no_cap(&format!("starting the flow {flow:?} (flows: [\"{flow}\"])")));
        }
        let inputs = match a.get("inputs") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(o)) if o.keys().all(|k| k.len() <= 64) && v.to_string().len() <= FLOW_INPUT_BYTES => v.clone(),
            Some(_) => return Err(SdkError::bad("inputs", "an object of at most 16 KB")),
        };
        let key = match opt_text(a, "key", 64)? {
            Some(k) if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c)) => return Err(SdkError::bad("key", "letters, digits and . _ : -")),
            k => k.map(str::to_string),
        };
        if let Some(k) = &key {
            if let Some(run) = self.st().run_keys.get(k) {
                return Ok(with_ignored(json!({"run": run, "repeated": true}), ignored));
            }
        }
        // it must be a published flow; the card names its version
        let info = self.ba("flows.list", &self.be.flows, &json!({"names": [flow]}))?;
        let found = items_of(info, 100)["items"].as_array().and_then(|a| a.iter().find(|f| f["name"] == flow.as_str()).cloned());
        let Some(found) = found else { return Err(SdkError::bad("flow", "not a published flow")) };
        let version = found["version"].as_u64().unwrap_or(0);
        if !within(&mut self.st().flow_runs, self.now(), 3_600_000, FLOW_RUNS_PER_HOUR) {
            return Err(quota("10 flow runs an hour"));
        }
        let app = self.app.clone();
        let mut lines = vec![format!("“{app}” would like to start your flow “{flow}” (version {version})."), "What goes in:".into()];
        let shown: Vec<String> = inputs.as_object().map(|o| o.iter().take(6).map(|(k, v)| format!("  {k}: {}", plain_line(&v.to_string(), 60))).collect()).unwrap_or_default();
        lines.extend(if shown.is_empty() { vec!["  nothing".to_string()] } else { shown });
        self.ask("flows.run", None, "An app wants to start a flow", lines, Box::new(move |s| {
            let r = s.ba("flows.run", &s.be.flows, &json!({"flow": flow, "version": version, "inputs": inputs, "key": key}))?;
            let run = r.get("run").and_then(Value::as_str).ok_or_else(|| SdkError::new(code::FAILED, "the flow engine gave no run"))?.to_string();
            {
                let mut st = s.st();
                if st.runs.len() < 200 {
                    st.runs.insert(run.clone());
                }
                if let Some(k) = key {
                    st.run_keys.insert(k, run.clone());
                }
            }
            s.audit("app.flow.run", &format!("{} started {} v{}", s.app, flow, version));
            Ok(with_ignored(json!({"run": run}), ignored))
        }))
    }

    fn own_run(&self, a: &Value) -> Result<String, SdkError> {
        let run = req_str(a, "run", 80)?.to_string();
        if !self.st().runs.contains(&run) {
            return Err(SdkError::bad("run", "not a run this app started"));
        }
        Ok(run)
    }

    fn flows_status(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["run"]);
        let run = self.own_run(a)?;
        let r = self.ba("flows.status", &self.be.flows, &json!({"run": run}))?;
        Ok(with_ignored(json!({"state": r.get("state").cloned().unwrap_or(Value::Null), "steps": r.get("steps").cloned().unwrap_or(json!([])), "error": r.get("error").cloned().unwrap_or(Value::Null)}), ignored))
    }

    fn flows_cancel(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["run"]);
        let run = self.own_run(a)?;
        let app = self.app.clone();
        self.ask("flows.cancel", None, "An app wants to stop a flow run", vec![format!("“{app}” would like to stop the flow run it started ({run}).")], Box::new(move |s| {
            let r = s.ba("flows.cancel", &s.be.flows, &json!({"run": run}))?;
            s.audit("app.flow.cancel", &format!("{} stopped a run", s.app));
            Ok(with_ignored(json!({"run": run, "state": r.get("state").cloned().unwrap_or(json!("cancelled"))}), ignored))
        }))
    }

    // ---- k3s

    fn k3s_manage(caps: &Caps) -> bool {
        caps.v2.get("k3s").and_then(Value::as_str) == Some("manage")
    }

    fn k3s_status(&self, caps: &Caps) -> Result<Value, SdkError> {
        let r = self.ba("k3s.status", &self.be.k3s, &json!({}))?;
        Ok(json!({"cluster": r.get("cluster").cloned().unwrap_or(Value::Null), "namespace": r.get("namespace").cloned().unwrap_or(Value::Null), "ready": r.get("ready").and_then(Value::as_bool).unwrap_or(false), "writable": Self::k3s_manage(caps)}))
    }

    fn k3s_get(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["kind", "name", "selector"]);
        const KINDS: [&str; 14] = ["pods", "deployments", "services", "configmaps", "secrets", "events", "ingresses", "statefulsets", "daemonsets", "jobs", "cronjobs", "replicasets", "endpoints", "persistentvolumeclaims"];
        let kind = req_str(a, "kind", 40)?.to_ascii_lowercase();
        if !KINDS.contains(&kind.as_str()) {
            return Err(SdkError::bad("kind", &format!("one of {}", KINDS.join(", "))));
        }
        let name = opt_text(a, "name", 253)?.map(str::to_string);
        if name.as_deref().is_some_and(|n| !dns_ok(n)) {
            return Err(SdkError::bad("name", "a Kubernetes name"));
        }
        let selector = opt_text(a, "selector", 200)?.map(str::to_string);
        if selector.as_deref().is_some_and(|s| !s.chars().all(|c| c.is_ascii_alphanumeric() || "=,!-_./ ()".contains(c))) {
            return Err(SdkError::bad("selector", "a label selector like app=web"));
        }
        let r = self.ba("k3s.get", &self.be.k3s, &json!({"kind": kind, "name": name, "selector": selector}))?;
        let mut v = items_of(r, K3S_ITEMS);
        if kind == "secrets" {
            if let Some(items) = v["items"].as_array_mut() {
                items.iter_mut().for_each(strip_secret);
            }
        }
        Ok(with_ignored(v, ignored))
    }

    fn k3s_logs(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["pod", "tail"]);
        let pod = req_str(a, "pod", 253)?.to_string();
        if !dns_ok(&pod) {
            return Err(SdkError::bad("pod", "a pod name"));
        }
        let tail = opt_uint(a, "tail", 1, LOG_LINES as u64)?.unwrap_or(100);
        let r = self.ba("k3s.logs", &self.be.k3s, &json!({"pod": pod, "tail": tail}))?;
        let lines: Vec<String> = match &r {
            Value::Array(_) => list(&r),
            v => list(v.get("lines").unwrap_or(&Value::Null)),
        };
        let lines: Vec<String> = lines.into_iter().rev().take(tail as usize).rev().map(|l| l.chars().take(2000).collect()).collect();
        Ok(with_ignored(json!({"lines": lines}), ignored))
    }

    fn k3s_apply(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["yaml"]);
        if !Self::k3s_manage(caps) {
            return Err(no_cap("changing the cluster (k3s: \"manage\")"));
        }
        let yaml = req_str(a, "yaml", K3S_YAML_BYTES)?.to_string();
        if yaml.trim().is_empty() {
            return Err(SdkError::bad("yaml", "empty"));
        }
        let plan = self.ba("k3s.plan", &self.be.k3s, &json!({"yaml": yaml}))?;
        if let Some(bad) = plan.get("refused").and_then(Value::as_array).filter(|a| !a.is_empty()) {
            let why = bad.iter().filter_map(Value::as_str).take(3).collect::<Vec<_>>().join("; ");
            return Err(SdkError::new(code::BLOCKED, format!("the cluster rules refuse this: {why}")));
        }
        let text = plan.get("plan").and_then(Value::as_str).unwrap_or("").to_string();
        let app = self.app.clone();
        let mut lines = vec![format!("“{app}” would like to change its own part of the cluster."), "The plan:".into()];
        lines.extend(text.lines().take(8).map(|l| format!("  {}", plain_line(l, 120))));
        self.ask("k3s.apply", None, "An app wants to change the cluster", lines, Box::new(move |s| {
            s.ba("k3s.apply", &s.be.k3s, &json!({"yaml": yaml}))?;
            s.audit("app.k3s.apply", &format!("{} applied a change", s.app));
            Ok(with_ignored(json!({"plan": text}), ignored))
        }))
    }

    fn k3s_restart(&self, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["name"]);
        if !Self::k3s_manage(caps) {
            return Err(no_cap("changing the cluster (k3s: \"manage\")"));
        }
        let name = req_str(a, "name", 253)?.to_string();
        if !dns_ok(&name) {
            return Err(SdkError::bad("name", "a workload name"));
        }
        let app = self.app.clone();
        self.ask("k3s.restart", None, "An app wants to restart a workload", vec![format!("“{app}” would like to restart “{name}” in its part of the cluster.")], Box::new(move |s| {
            s.ba("k3s.restart", &s.be.k3s, &json!({"name": name}))?;
            s.audit("app.k3s.restart", &format!("{} restarted {}", s.app, name));
            Ok(with_ignored(json!({"restarted": name}), ignored))
        }))
    }

    // ---- listening

    fn listen(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["lang", "maxSeconds", "device"]);
        let lang = opt_text(a, "lang", 16)?.map(str::to_string);
        if lang.as_deref().is_some_and(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            return Err(SdkError::bad("lang", "a language tag like en or ta-IN"));
        }
        let secs = opt_uint(a, "maxSeconds", 1, LISTEN_SECONDS)?.unwrap_or(15);
        let device = opt_text(a, "device", 80)?.map(str::to_string);
        if self.st().listening.is_some() {
            return Err(quota("one microphone use at a time"));
        }
        // ask the machine first: nobody is asked to allow what cannot work
        let mic = self.ba("speech.available", &self.be.listener, &json!({"device": device}))?;
        let mic_name = plain_line(mic.get("microphone").and_then(Value::as_str).unwrap_or("the microphone"), 80);
        if !within(&mut self.st().listens, self.now(), 3_600_000, LISTENS_PER_HOUR) {
            return Err(quota("20 microphone uses an hour"));
        }
        let app = self.app.clone();
        let lines = vec![format!("“{app}” would like to listen through {mic_name} for up to {secs} seconds and turn what it hears into text."), "The app gets the text only, never the sound. The sound is thrown away afterwards.".into()];
        self.ask("speech.listen", None, "An app wants to listen", lines, Box::new(move |s| {
            let id = s.out().alloc();
            s.ba("speech.listen", &s.be.listener, &json!({"id": id, "lang": lang, "max_seconds": secs, "device": device}))?;
            let until = s.now() + secs * 1000 + 30_000;
            s.st().listening = Some((id, until));
            s.audit("speech.listen", &format!("{} listened through {}", s.app, mic_name));
            Ok(with_ignored(json!({"handle": id}), ignored))
        }))
    }

    /// A recognised text (or the engine's failure) for a listen. Backends call it from their own thread. The sound never comes this way.
    pub fn heard(&self, handle: u32, r: Result<String, SdkError>) {
        {
            let mut st = self.st();
            if st.listening.map(|(h, _)| h) != Some(handle) {
                return;
            }
            st.listening = None;
        }
        match r {
            Ok(t) => {
                self.event(handle, "result", json!({"text": plain_line(&t, 4000)}));
                self.event(handle, "done", json!({}));
            }
            Err(e) => self.event(handle, "error", e.payload()),
        }
    }

    fn listen_cancel(&self, a: &Value) -> Result<Value, SdkError> {
        let h = req_uint(a, "handle")? as u32;
        let mine = self.st().listening.map(|(i, _)| i) == Some(h);
        if mine {
            self.st().listening = None;
            let _ = self.ba("speech.cancel", &self.be.listener, &json!({"id": h}));
            self.event(h, "done", json!({"cancelled": true}));
        }
        Ok(json!({}))
    }
}

pub(super) fn denied() -> SdkError {
    SdkError::new(code::DENIED, "the person did not allow this")
}

/// The host of an http or https address, lower case; `None` for anything else (no user names, no odd schemes).
fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let auth = rest.split(['/', '?', '#']).next()?;
    if auth.contains('@') || auth.is_empty() {
        return None;
    }
    let host = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(auth, |(h, _)| h);
    (!host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')).then(|| host.to_ascii_lowercase())
}

/// A Secret goes out as names only: no `data`, no `stringData`, no copy of it in the last-applied annotation.
fn strip_secret(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        if let Some(d) = o.remove("data") {
            let names: Vec<String> = d.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            o.insert("keys".into(), json!(names));
        }
        if let Some(d) = o.remove("stringData") {
            let names: Vec<String> = d.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            let mut all: Vec<String> = o.get("keys").map(list).unwrap_or_default();
            all.extend(names);
            o.insert("keys".into(), json!(all));
        }
        if let Some(a) = o.get_mut("metadata").and_then(|m| m.get_mut("annotations")).and_then(Value::as_object_mut) {
            a.remove("kubectl.kubernetes.io/last-applied-configuration");
        }
    }
}

