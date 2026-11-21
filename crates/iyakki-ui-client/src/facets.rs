//! The assistant and camera facets over the UI socket (F-SDK-85, F-SDK-166): `assistant.*` and `cameras.*` calls, served from the core's
//! facets (`iyakki_core_api::Assistant`, `iyakki_core_api::Cameras`) for the person the listener is fixed to, and the client end that gives a UI
//! program outside the process the same two traits (`SocketAssistant`, `SocketCameras`).
//!
//! * **Who.** The person and the scopes are the listener's, never the request's. `assistant:use` and `cameras:view` open the domains; a yes to
//!   a card (a tool of the app, a camera consent, a snapshot, a move) needs the further scopes `assistant:approve` and `cameras:consent`. A
//!   no needs nobody. A local in-process context holds every scope of a signed-in person.
//! * **Whose sessions.** A connection reaches only the sessions and views it opened itself, whatever ids it makes up.
//! * **Events are pushed.** `assistant.subscribe {session}` starts a thread of the connection that hands the session's events to the client as
//!   soon as the core has them: a line `{"push":"assistant","session":..,"events":[..]}` between answers. The client keeps pushed lines in a
//!   queue; `SocketAssistant::poll` hands out what has arrived, `wait_events` blocks until some arrive. A subscribed session's `assistant.poll`
//!   answers an empty list (the thread owns the queue of the core). At most 4 subscriptions per connection; they end with the connection.
//! * **Pictures.** A line is at most 64 KiB, so a camera picture is cut into chunks of 40 KB of data: `cameras.frame {view, camera, after,
//!   format, offset}` answers `{seq, width, height, format, total, offset, data}` (base64), or `{"none":true}` when nothing is newer; the client asks on until it has `total` bytes and
//!   starts again when the sequence number changes under it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::assistant::{AssistEvent, Assistant, Choice, ContextItem, HistoryEntry, SessionInfo, SessionSpec};
use iyakki_core_api::contract::{AppEventFeed, CallContext, Flows, Resume, Scope, Settings};
use iyakki_core_api::cameras::{CamEvent, CamFrame, CameraInfo, Cameras, FrameFormat, Overlay, PtzCommand, ViewInfo, ViewSpec, ViewState};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::client::Connection;
use crate::server::Served;
use crate::{CallError, CallResult, ErrorCode, Request};

pub const ASSISTANT_USE: &str = "assistant:use";
pub const ASSISTANT_APPROVE: &str = "assistant:approve";
pub const CAMERAS_VIEW: &str = "cameras:view";
pub const CAMERAS_CONSENT: &str = "cameras:consent";
/// Change flows, the assistant's book and the outside-message policy through the object calls (F-SPLIT-03, D-S5-02). `flows:read` and `flows:run`
/// are the contract's own scopes.
pub const FLOWS_EDIT: &str = "flows:edit";
/// Make, renew and open webhooks, and allow a program to run in a flow: the asks (D-S5-02). Not in the scopes the core hands the UI program.
pub const FLOWS_HOOKS: &str = "flows:hooks";
/// Read the display lists the core draws for apps it runs (`bulk.get` block `display.<name>`, F-SPLIT-07).
pub const DISPLAY_VIEW: &str = "display:view";

/// The most subscriptions one connection may hold.
pub const MAX_SUBSCRIPTIONS: usize = 4;
/// Bytes of picture data in one chunk (about 54 KB of base64 text, inside the 64 KiB line).
pub const CHUNK: usize = 40 * 1024;
/// How often a subscription looks at the core's queue. The core's facet is pull-only; the wait is here, not in the UI.
const PUSH_EVERY: Duration = Duration::from_millis(30);

fn invalid(why: &str) -> CallError {
    CallError::new(ErrorCode::InvalidInput, "transport.bad_request", why)
}

fn refused(sentence: String) -> CallError {
    CallError::new(ErrorCode::Denied, "facet.refused", &sentence)
}

fn no_scope(scope: &str) -> CallError {
    CallError::new(ErrorCode::Denied, "transport.no_scope", &format!("This connection was not given the scope {scope}.")).hint("Pair again and allow it, or ask the person at the machine.")
}

fn missing(domain: &str) -> CallError {
    CallError::new(ErrorCode::Unsupported, "transport.no_domain", &format!("This machine does not serve {domain}.")).hint("Read `hello` for the domains it serves.")
}

fn text<'a>(a: &'a Value, k: &str) -> CallResult<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 64 * 1024 && !s.contains('\0')).ok_or_else(|| invalid(&format!("`{k}` is a text")))
}

fn flag(a: &Value, k: &str) -> CallResult<bool> {
    a.get(k).and_then(Value::as_bool).ok_or_else(|| invalid(&format!("`{k}` is true or false")))
}

fn opt(a: &Value, k: &str) -> CallResult<Option<String>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= 64 => Ok(Some(s.clone())),
        _ => Err(invalid(&format!("`{k}` is a text"))),
    }
}

fn typed<T: DeserializeOwned>(a: &Value, k: &str) -> CallResult<T> {
    serde_json::from_value(a.get(k).cloned().unwrap_or(Value::Null)).map_err(|e| invalid(&format!("`{k}` is wrong: {e}")))
}

fn enc<T: serde::Serialize>(v: &T) -> CallResult<Value> {
    serde_json::to_value(v).map_err(|e| CallError::new(ErrorCode::Internal, "transport.encode", &e.to_string()))
}

/// A subscription the connection asked for: the serve loop starts a thread for it (inside the connection's own thread scope, so the thread
/// ends with the connection and borrows its writer).
pub enum PushJob {
    Assistant { core: Arc<dyn Assistant>, user: String, session: String },
    /// The changes of the settings the listener may read (`settings.subscribe`), instead of the client asking `settings.watch` again and again.
    Settings { core: Arc<dyn Settings>, ctx: CallContext, from: u64 },
    /// The events of the person's flow runs (`flows.subscribe`): `{"push":"flows","events":[..],"gap":bool}`.
    Flows { core: Arc<dyn Flows>, ctx: CallContext, from: u64 },
    /// The app events the person registered for (`appevents.subscribe`): `{"push":"appevents","events":[..],"gap":bool}`. Reading hands the events out.
    AppEvents { core: Arc<dyn AppEventFeed>, ctx: CallContext, from: u64 },
}

/// What one connection owns: the sessions and views it opened and the pushes it asked for.
pub struct Conn {
    sessions: BTreeSet<String>,
    views: BTreeSet<String>,
    subscribed: BTreeSet<String>,
    /// Subscriptions not started yet (taken by the serve loop after each call).
    pub jobs: Vec<PushJob>,
    /// The sessions and views of this connection are closed when it ends.
    served: Option<(Option<Arc<dyn Assistant>>, Option<Arc<dyn Cameras>>, String)>,
}

impl Default for Conn {
    fn default() -> Conn {
        Conn::new()
    }
}

impl Conn {
    pub fn new() -> Conn {
        Conn { sessions: BTreeSet::new(), views: BTreeSet::new(), subscribed: BTreeSet::new(), jobs: Vec::new(), served: None }
    }

    /// The connection is over: the push threads stop and what it opened is closed (nothing keeps pulling from a camera for a gone UI).
    pub fn finish(&mut self, stop: &AtomicBool) {
        stop.store(true, Ordering::Relaxed);
        if let Some((a, c, user)) = self.served.take() {
            if let Some(a) = a {
                for s in &self.sessions {
                    a.close(&user, s);
                }
            }
            if let Some(c) = c {
                for v in &self.views {
                    c.close(&user, v);
                }
            }
        }
    }
}

/// The thread of one subscription: hand the session's events to the client as soon as the core has them, until the connection ends or a write
/// fails. A batch too big for one line is split in halves; a single event that is still too big is dropped.
pub fn push_loop<W: Write>(job: PushJob, out: &Mutex<W>, stop: &AtomicBool) {
    let (core, user, session) = match job {
        PushJob::Assistant { core, user, session } => (core, user, session),
        PushJob::Settings { core, ctx, from } => {
            return events_push_loop("settings", from, out, stop, |r| {
                let b = core.watch(&ctx, None, r, 100).ok()?;
                Some((b.events.iter().filter_map(|e| serde_json::to_value(e).ok()).collect(), b.next, b.gap))
            })
        }
        PushJob::Flows { core, ctx, from } => {
            return events_push_loop("flows", from, out, stop, |r| {
                let b = core.events(&ctx, None, r, 100).ok()?;
                Some((b.events.iter().filter_map(|e| serde_json::to_value(e).ok()).collect(), b.next, b.gap))
            })
        }
        PushJob::AppEvents { core, ctx, from } => {
            return events_push_loop("appevents", from, out, stop, |r| {
                let b = core.events(&ctx, r, 100).ok()?;
                Some((b.events.iter().filter_map(|e| serde_json::to_value(e).ok()).collect(), b.next, b.gap))
            })
        }
    };
    fn send<W: Write>(out: &Mutex<W>, session: &str, events: &[AssistEvent]) -> bool {
        let mut line = json!({"push": "assistant", "session": session, "events": events}).to_string();
        if line.len() >= crate::MAX_LINE {
            return match events.len() {
                0 | 1 => true,
                n => send(out, session, &events[..n / 2]) && send(out, session, &events[n / 2..]),
            };
        }
        line.push('\n');
        let mut o = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        o.write_all(line.as_bytes()).and_then(|_| o.flush()).is_ok()
    }
    while !stop.load(Ordering::Relaxed) {
        let events = core.poll(&user, &session);
        if !events.is_empty() && !send(out, &session, &events) {
            return;
        }
        std::thread::sleep(PUSH_EVERY);
    }
}

/// The thread of an event subscription (settings, flows, app events): `{"push":NAME,"events":[..],"gap":bool}` lines as the core's log grows,
/// through the same guarded read (`fetch`, which gets the point to continue from) the client could call itself. `None` from `fetch` (the scope
/// was taken away or the core has no such domain) ends the push, the connection goes on. A batch too big for one line is split in halves; one
/// event too big is dropped.
fn events_push_loop<W: Write>(name: &str, from: u64, out: &Mutex<W>, stop: &AtomicBool, mut fetch: impl FnMut(Resume) -> Option<(Vec<Value>, u64, bool)>) {
    fn send<W: Write>(out: &Mutex<W>, name: &str, events: &[Value], gap: bool) -> bool {
        let mut line = json!({"push": name, "events": events, "gap": gap}).to_string();
        if line.len() >= crate::MAX_LINE {
            return match events.len() {
                0 | 1 => true,
                n => send(out, name, &events[..n / 2], gap) && send(out, name, &events[n / 2..], false),
            };
        }
        line.push('\n');
        let mut o = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        o.write_all(line.as_bytes()).and_then(|_| o.flush()).is_ok()
    }
    let mut resume = Resume::After(from);
    while !stop.load(Ordering::Relaxed) {
        match fetch(resume) {
            Some((events, next, gap)) => {
                resume = Resume::After(next);
                if (!events.is_empty() || gap) && !send(out, name, &events, gap) {
                    return;
                }
            }
            None => return,
        }
        std::thread::sleep(PUSH_EVERY);
    }
}

/// `settings.subscribe`: start pushing the settings changes this listener may read. Once per connection.
fn settings_subscribe(s: &Served, conn: &mut Conn) -> CallResult<Value> {
    let core = s.settings.as_ref().ok_or_else(|| missing("settings"))?.clone();
    if !s.ctx.allows(Scope::SETTINGS_READ) {
        return Err(no_scope(Scope::SETTINGS_READ));
    }
    if conn.subscribed.contains("\u{0}settings") {
        return Ok(json!({"ok": true}));
    }
    if conn.subscribed.len() >= MAX_SUBSCRIPTIONS {
        return Err(CallError::new(ErrorCode::Busy, "facet.too_many_subscriptions", &format!("A connection may hold {MAX_SUBSCRIPTIONS} subscriptions at most.")));
    }
    // the point to continue from is taken HERE, before the answer: a change made right after `subscribe` returns is never missed
    let from = core.watch(&s.ctx, None, Resume::Latest, 1)?.next;
    conn.subscribed.insert("\u{0}settings".into());
    conn.jobs.push(PushJob::Settings { core, ctx: s.ctx.clone(), from });
    Ok(json!({"ok": true}))
}

/// `flows.subscribe` and `appevents.subscribe`: start pushing the events this listener may read. Once per connection each.
fn feed_subscribe(s: &Served, conn: &mut Conn, which: &str) -> CallResult<Value> {
    let mark = format!("\u{0}{which}");
    // the starting point is taken HERE, through the same guarded read the pushes use, before the answer: nothing after it is missed
    let job = if which == "flows" {
        let core = s.flows.as_ref().ok_or_else(|| missing("flows"))?.clone();
        let from = core.events(&s.ctx, None, Resume::Latest, 1)?.next;
        PushJob::Flows { core, ctx: s.ctx.clone(), from }
    } else {
        let core = s.appevents.as_ref().ok_or_else(|| missing("appevents"))?.clone();
        let from = core.events(&s.ctx, Resume::Latest, 1)?.next;
        PushJob::AppEvents { core, ctx: s.ctx.clone(), from }
    };
    if conn.subscribed.contains(&mark) {
        return Ok(json!({"ok": true}));
    }
    if conn.subscribed.len() >= MAX_SUBSCRIPTIONS {
        return Err(CallError::new(ErrorCode::Busy, "facet.too_many_subscriptions", &format!("A connection may hold {MAX_SUBSCRIPTIONS} subscriptions at most.")));
    }
    conn.subscribed.insert(mark);
    conn.jobs.push(job);
    Ok(json!({"ok": true}))
}

/// Serve one call of the two domains. `None`: the call is not of these domains.
pub fn dispatch(s: &Served, conn: &mut Conn, req: &Request) -> Option<CallResult<Value>> {
    let (domain, method) = req.call.split_once('.')?;
    match domain {
        "assistant" => Some(assistant(s, conn, method, &req.args)),
        "cameras" => Some(cameras(s, conn, method, &req.args)),
        "settings" if method == "subscribe" => Some(settings_subscribe(s, conn)),
        "flows" if method == "subscribe" => Some(feed_subscribe(s, conn, "flows")),
        "appevents" if method == "subscribe" => Some(feed_subscribe(s, conn, "appevents")),
        _ => None,
    }
}

fn assistant(s: &Served, conn: &mut Conn, method: &str, a: &Value) -> CallResult<Value> {
    let core = s.assistant.as_ref().ok_or_else(|| missing("assistant"))?.clone();
    if !s.ctx.allows(ASSISTANT_USE) {
        return Err(no_scope(ASSISTANT_USE));
    }
    let user = s.ctx.identity.user.clone();
    conn.served = Some((s.assistant.clone(), s.cameras.clone(), user.clone()));
    let mine = |conn: &Conn| -> CallResult<String> {
        let id = text(a, "session")?;
        if conn.sessions.contains(id) { Ok(id.to_string()) } else { Err(CallError::new(ErrorCode::NotFound, "facet.no_session", "no such assistant session")) }
    };
    let ok = || Ok(json!({"ok": true}));
    match method {
        "models" => enc(&core.models(&user)),
        "roles" => enc(&core.roles(&user)),
        "open" => {
            let spec: SessionSpec = typed(a, "spec")?;
            let info = core.open(&user, text(a, "app")?, &spec).map_err(refused)?;
            conn.sessions.insert(info.id.clone());
            enc(&info)
        }
        "set_context" => {
            let c: Vec<ContextItem> = typed(a, "context")?;
            core.set_context(&user, &mine(conn)?, &c).map_err(refused)?;
            ok()
        }
        "choose" => {
            core.choose(&user, &mine(conn)?, opt(a, "model")?.as_deref(), opt(a, "role")?.as_deref()).map_err(refused)?;
            ok()
        }
        "send" => {
            core.send(&user, &mine(conn)?, text(a, "text")?).map_err(refused)?;
            ok()
        }
        "poll" => {
            let id = mine(conn)?;
            if conn.subscribed.contains(&id) {
                return Ok(json!([]));
            }
            enc(&core.poll(&user, &id))
        }
        "subscribe" => {
            let id = mine(conn)?;
            if conn.subscribed.contains(&id) {
                return ok();
            }
            if conn.subscribed.len() >= MAX_SUBSCRIPTIONS {
                return Err(CallError::new(ErrorCode::Busy, "facet.too_many_subscriptions", &format!("A connection may hold {MAX_SUBSCRIPTIONS} subscriptions at most.")));
            }
            conn.subscribed.insert(id.clone());
            conn.jobs.push(PushJob::Assistant { core, user, session: id });
            ok()
        }
        "decide" => {
            let approve = flag(a, "approve")?;
            if approve && !s.ctx.allows(ASSISTANT_APPROVE) {
                return Err(no_scope(ASSISTANT_APPROVE));
            }
            core.decide(&user, &mine(conn)?, text(a, "call")?, approve).map_err(refused)?;
            ok()
        }
        "answer" => {
            core.answer(&user, &mine(conn)?, text(a, "call")?, flag(a, "ok")?, a.get("text").and_then(Value::as_str).filter(|t| t.len() <= 64 * 1024).ok_or_else(|| invalid("`text` is a text"))?).map_err(refused)?;
            ok()
        }
        "cancel" => {
            core.cancel(&user, &mine(conn)?).map_err(refused)?;
            ok()
        }
        "history" => enc(&core.history(&user, &mine(conn)?)),
        "close" => {
            let id = mine(conn)?;
            core.close(&user, &id);
            conn.sessions.remove(&id);
            conn.subscribed.remove(&id);
            ok()
        }
        other => Err(CallError::new(ErrorCode::Unsupported, "transport.unknown_call", &format!("This machine does not know the call assistant.{other}."))),
    }
}

fn cameras(s: &Served, conn: &mut Conn, method: &str, a: &Value) -> CallResult<Value> {
    let core = s.cameras.as_ref().ok_or_else(|| missing("cameras"))?.clone();
    if !s.ctx.allows(CAMERAS_VIEW) {
        return Err(no_scope(CAMERAS_VIEW));
    }
    let user = s.ctx.identity.user.clone();
    conn.served = Some((s.assistant.clone(), s.cameras.clone(), user.clone()));
    let mine = |conn: &Conn| -> CallResult<String> {
        let id = text(a, "view")?;
        if conn.views.contains(id) { Ok(id.to_string()) } else { Err(CallError::new(ErrorCode::NotFound, "facet.no_view", "no such camera view")) }
    };
    let ok = || Ok(json!({"ok": true}));
    let app = || text(a, "app");
    match method {
        "list" => enc(&core.list(&user, app()?)),
        "open" => {
            let spec: ViewSpec = typed(a, "spec")?;
            let info = core.open(&user, app()?, &spec).map_err(refused)?;
            conn.views.insert(info.id.clone());
            enc(&info)
        }
        "decide" => {
            let approve = flag(a, "approve")?;
            if approve && !s.ctx.allows(CAMERAS_CONSENT) {
                return Err(no_scope(CAMERAS_CONSENT));
            }
            core.decide(&user, &mine(conn)?, text(a, "call")?, approve).map_err(refused)?;
            ok()
        }
        "state" => enc(&core.state(&user, &mine(conn)?).map_err(refused)?),
        "set_visible" => {
            core.set_visible(&user, &mine(conn)?, flag(a, "visible")?).map_err(refused)?;
            ok()
        }
        "frame" => {
            let format = match a.get("format").and_then(Value::as_str).unwrap_or("jpeg") {
                "jpeg" => FrameFormat::Jpeg,
                "rgba" => FrameFormat::Rgba,
                _ => return Err(invalid("`format` is jpeg or rgba")),
            };
            let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let after = a.get("after").and_then(Value::as_u64).unwrap_or(0);
            match core.frame(&user, &mine(conn)?, text(a, "camera")?, after, format).map_err(refused)? {
                // `null` cannot be an answer (a result that is null reads as no result): "nothing newer" is said in words
                None => Ok(json!({"none": true})),
                Some(f) => {
                    let total = f.data.len();
                    let end = (offset + CHUNK).min(total);
                    if offset > total {
                        return Err(invalid("`offset` is past the end of the picture"));
                    }
                    Ok(json!({"seq": f.seq, "width": f.width, "height": f.height, "format": if f.format == FrameFormat::Jpeg { "jpeg" } else { "rgba" }, "total": total, "offset": offset, "data": b64_encode(&f.data[offset..end])}))
                }
            }
        }
        "overlay" => enc(&core.overlay(&user, &mine(conn)?, text(a, "camera")?).map_err(refused)?),
        "snapshot" => Ok(json!({"call": core.snapshot(&user, &mine(conn)?, text(a, "camera")?).map_err(refused)?})),
        "ptz" => {
            let cmd: PtzCommand = serde_json::from_value(a.clone()).map_err(|_| invalid("`cmd` is move (with pan, tilt, zoom) or stop"))?;
            Ok(json!({"call": core.ptz(&user, &mine(conn)?, text(a, "camera")?, cmd.checked().map_err(|e| invalid(&e))?).map_err(refused)?}))
        }
        "events" => enc(&core.events(&user, &mine(conn)?)),
        "close" => {
            let id = mine(conn)?;
            core.close(&user, &id);
            conn.views.remove(&id);
            ok()
        }
        other => Err(CallError::new(ErrorCode::Unsupported, "transport.unknown_call", &format!("This machine does not know the call cameras.{other}."))),
    }
}

// ------------------------------------------------------------------------------------------------ base64 (the crate depends on no other IYAKKI crate)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(b: &[u8]) -> String {
    let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18) as usize & 63] as char);
        o.push(B64[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    o
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|x| *x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = i == b.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, c) in q.iter().enumerate() {
            n = n << 6 | if j >= 4 - pad { 0 } else { val(*c)? };
        }
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------ the client end

/// The shared state of a UI's connection: the lines the machine pushed, by session.
#[derive(Default)]
pub struct Pushed {
    by_session: BTreeMap<String, VecDeque<AssistEvent>>,
}

/// `Assistant` over a connection. The `user` argument of the trait is ignored: who the caller is, is the machine's end of the transport.
pub struct SocketAssistant {
    conn: Arc<Mutex<Connection>>,
    pushed: Arc<Mutex<Pushed>>,
    subscribed: Mutex<BTreeSet<String>>,
}

impl SocketAssistant {
    pub fn new(conn: Arc<Mutex<Connection>>) -> SocketAssistant {
        SocketAssistant { conn, pushed: Arc::new(Mutex::new(Pushed::default())), subscribed: Mutex::new(BTreeSet::new()) }
    }

    fn call<T: DeserializeOwned>(&self, call: &str, args: Value) -> Result<T, String> {
        let v = {
            let mut c = self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let r = c.call(call, args);
            self.take_pushed(&mut c);
            r
        }
        .map_err(|e| e.message)?;
        serde_json::from_value(v).map_err(|_| "the machine's answer was not readable".to_string())
    }

    fn take_pushed(&self, c: &mut Connection) {
        let mut p = self.pushed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for line in c.take_pushed() {
            if line["push"] == "assistant" {
                if let (Some(id), Ok(ev)) = (line["session"].as_str(), serde_json::from_value::<Vec<AssistEvent>>(line["events"].clone())) {
                    p.by_session.entry(id.to_string()).or_default().extend(ev);
                }
            }
        }
    }

    /// Ask the machine to push this session's events (the UI no longer has to ask for them).
    pub fn subscribe(&self, session: &str) -> Result<(), String> {
        self.call::<Value>("assistant.subscribe", json!({"session": session}))?;
        self.subscribed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(session.to_string());
        Ok(())
    }

    /// Block until the machine has pushed events for this session (or the connection's read timeout passes), then hand them out.
    pub fn wait_events(&self, session: &str) -> Result<Vec<AssistEvent>, String> {
        loop {
            {
                let mut p = self.pushed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(q) = p.by_session.get_mut(session) {
                    if !q.is_empty() {
                        return Ok(q.drain(..).collect());
                    }
                }
            }
            let mut c = self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            c.read_push().map_err(|e| e.message)?;
            self.take_pushed(&mut c);
        }
    }
}

impl Assistant for SocketAssistant {
    fn models(&self, _user: &str) -> Vec<Choice> {
        self.call("assistant.models", json!({})).unwrap_or_default()
    }
    fn roles(&self, _user: &str) -> Vec<Choice> {
        self.call("assistant.roles", json!({})).unwrap_or_default()
    }
    fn open(&self, _user: &str, app: &str, spec: &SessionSpec) -> Result<SessionInfo, String> {
        self.call("assistant.open", json!({"app": app, "spec": spec}))
    }
    fn set_context(&self, _user: &str, session: &str, context: &[ContextItem]) -> Result<(), String> {
        self.call::<Value>("assistant.set_context", json!({"session": session, "context": context})).map(|_| ())
    }
    fn choose(&self, _user: &str, session: &str, model: Option<&str>, role: Option<&str>) -> Result<(), String> {
        self.call::<Value>("assistant.choose", json!({"session": session, "model": model, "role": role})).map(|_| ())
    }
    fn send(&self, _user: &str, session: &str, text: &str) -> Result<(), String> {
        self.call::<Value>("assistant.send", json!({"session": session, "text": text})).map(|_| ())
    }
    fn poll(&self, _user: &str, session: &str) -> Vec<AssistEvent> {
        let subscribed = self.subscribed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(session);
        let polled: Vec<AssistEvent> = self.call("assistant.poll", json!({"session": session})).unwrap_or_default();
        if !subscribed {
            return polled;
        }
        let mut p = self.pushed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        p.by_session.get_mut(session).map(|q| q.drain(..).collect()).unwrap_or_default()
    }
    fn decide(&self, _user: &str, session: &str, call: &str, approve: bool) -> Result<(), String> {
        self.call::<Value>("assistant.decide", json!({"session": session, "call": call, "approve": approve})).map(|_| ())
    }
    fn answer(&self, _user: &str, session: &str, call: &str, ok: bool, text: &str) -> Result<(), String> {
        self.call::<Value>("assistant.answer", json!({"session": session, "call": call, "ok": ok, "text": text})).map(|_| ())
    }
    fn cancel(&self, _user: &str, session: &str) -> Result<(), String> {
        self.call::<Value>("assistant.cancel", json!({"session": session})).map(|_| ())
    }
    fn history(&self, _user: &str, session: &str) -> Vec<HistoryEntry> {
        self.call("assistant.history", json!({"session": session})).unwrap_or_default()
    }
    fn close(&self, _user: &str, session: &str) {
        let _ = self.call::<Value>("assistant.close", json!({"session": session}));
        self.subscribed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(session);
        self.pushed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).by_session.remove(session);
    }
}

/// `Cameras` over a connection (the app name is part of each call; the person is the machine's).
pub struct SocketCameras {
    conn: Arc<Mutex<Connection>>,
}

impl SocketCameras {
    pub fn new(conn: Arc<Mutex<Connection>>) -> SocketCameras {
        SocketCameras { conn }
    }

    fn call<T: DeserializeOwned>(&self, call: &str, args: Value) -> Result<T, String> {
        let v = self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).call(call, args).map_err(|e| e.message)?;
        serde_json::from_value(v).map_err(|_| "the machine's answer was not readable".to_string())
    }
}

impl Cameras for SocketCameras {
    fn list(&self, _user: &str, app: &str) -> Vec<CameraInfo> {
        self.call("cameras.list", json!({"app": app})).unwrap_or_default()
    }
    fn open(&self, _user: &str, app: &str, spec: &ViewSpec) -> Result<ViewInfo, String> {
        self.call("cameras.open", json!({"app": app, "spec": spec}))
    }
    fn decide(&self, _user: &str, view: &str, call: &str, approve: bool) -> Result<(), String> {
        self.call::<Value>("cameras.decide", json!({"view": view, "call": call, "approve": approve})).map(|_| ())
    }
    fn state(&self, _user: &str, view: &str) -> Result<ViewState, String> {
        self.call("cameras.state", json!({"view": view}))
    }
    fn set_visible(&self, _user: &str, view: &str, visible: bool) -> Result<(), String> {
        self.call::<Value>("cameras.set_visible", json!({"view": view, "visible": visible})).map(|_| ())
    }
    fn frame(&self, _user: &str, view: &str, camera: &str, after: u64, format: FrameFormat) -> Result<Option<CamFrame>, String> {
        let fmt = if format == FrameFormat::Jpeg { "jpeg" } else { "rgba" };
        'again: for _ in 0..3 {
            let mut data: Vec<u8> = Vec::new();
            let mut head: Option<(u64, u32, u32, FrameFormat, usize)> = None;
            loop {
                let v: Value = self.call("cameras.frame", json!({"view": view, "camera": camera, "after": after, "format": fmt, "offset": data.len()}))?;
                if v["none"] == true {
                    if head.is_some() {
                        continue 'again; // the picture moved on under us: start over
                    }
                    return Ok(None);
                }
                let seq = v["seq"].as_u64().ok_or("the picture was not readable")?;
                let total = v["total"].as_u64().ok_or("the picture was not readable")? as usize;
                if head.is_some_and(|h| h.0 != seq || h.4 != total) {
                    continue 'again;
                }
                let chunk = b64_decode(v["data"].as_str().unwrap_or("")).ok_or("the picture was not readable")?;
                if v["offset"].as_u64() != Some(data.len() as u64) || chunk.is_empty() && total > data.len() || total > 16 * 1024 * 1024 {
                    return Err("the picture was not readable".into());
                }
                head = Some((seq, v["width"].as_u64().unwrap_or(0) as u32, v["height"].as_u64().unwrap_or(0) as u32, if v["format"] == "jpeg" { FrameFormat::Jpeg } else { FrameFormat::Rgba }, total));
                data.extend_from_slice(&chunk);
                if let (true, Some((seq, width, height, format, _))) = (data.len() >= total, head) {
                    return Ok(Some(CamFrame { seq, width, height, format, data: Arc::new(data) }));
                }
            }
        }
        Err("the picture kept changing; try again".into())
    }
    fn overlay(&self, _user: &str, view: &str, camera: &str) -> Result<Overlay, String> {
        self.call("cameras.overlay", json!({"view": view, "camera": camera}))
    }
    fn snapshot(&self, _user: &str, view: &str, camera: &str) -> Result<String, String> {
        let v: Value = self.call("cameras.snapshot", json!({"view": view, "camera": camera}))?;
        v["call"].as_str().map(str::to_string).ok_or_else(|| "the machine's answer was not readable".to_string())
    }
    fn ptz(&self, _user: &str, view: &str, camera: &str, cmd: PtzCommand) -> Result<Option<String>, String> {
        let mut a = serde_json::to_value(cmd).map_err(|_| "the command was not readable".to_string())?;
        a["view"] = json!(view);
        a["camera"] = json!(camera);
        let v: Value = self.call("cameras.ptz", a)?;
        Ok(v["call"].as_str().map(str::to_string))
    }
    fn events(&self, _user: &str, view: &str) -> Vec<CamEvent> {
        self.call("cameras.events", json!({"view": view})).unwrap_or_default()
    }
    fn close(&self, _user: &str, view: &str) {
        let _ = self.call::<Value>("cameras.close", json!({"view": view}));
    }
}
