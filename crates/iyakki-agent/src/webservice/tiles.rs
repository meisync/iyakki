//! Tiles in a browser (docs/development/TILE-ABI.md, "In a browser"; FEATURES F-WASM-10, F-WEB-04).
//!
//! A WASM tile runs HERE, on the machine that serves the web UI (a desktop, or the cloud pod), inside the `iyakki-tile-host`
//! sandbox with its fuel, time and memory budgets. The browser only sends input events as JSON and draws the display list it gets
//! back (`ui/tilerender.js`), so the OS stays where it is and the tile looks the same as on the framebuffer shell.
//!
//! * `GET /api/v1/tiles`: the tiles the signed-in person may open.
//! * `POST /api/v1/tiles/{id}/events`: input events in, the new display list out. The first call for a person starts the tile
//!   (it needs `width` and `height`); `reset: true` throws the state away and starts again.
//!
//! A browser session is not extra authority. The tile's own manifest lists what it may ask for; whatever it may ask for is then
//! checked against the PERSON: only tools in the service profile's list, only tools that do not ask first (a tool that would
//! ask is refused and the tile is told "no": the browser has no approval prompt for tile calls yet), and the same audit log.
//! State is per person (and per tenant volume): a second person never reaches the first person's tile.

use super::handlers::{Ctx, Out};
use super::server::Fail;
use crate::agent::{Risk, Tools};
use crate::webservice::auth::Limiter;
use iyakki_core_api::AuditSink;
use iyakki_tile_abi::{limits, Color, Event};
use iyakki_tile_host::{export, Budgets, ServiceBackend, TileError, TileManifest, TileRunner};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Largest request body for the events route.
pub const MAX_BODY: usize = 64 * 1024;
/// Events in one request (the host adds at most one `Hit` per release).
pub const MAX_EVENTS: usize = 128;
/// Largest surface a browser may ask for, in logical pixels.
pub const MAX_SURFACE: i64 = 4096;
/// Tile sessions alive on one machine at a time, and how long an idle one is kept.
pub const MAX_SESSIONS: usize = 32;
pub const IDLE: Duration = Duration::from_secs(30 * 60);
/// Event requests per person per minute (the general per-person limit does not apply to this route).
pub const EVENTS_PER_MINUTE: u32 = 600;
/// Largest reply of one service call, in bytes.
const MAX_REPLY: usize = 32 * 1024;

/// One tile the machine can run.
#[derive(Clone)]
pub struct TileDef {
    pub id: String,
    pub title: String,
    /// `.wasm` bytes (WAT text is accepted too, for development).
    pub wasm: Arc<Vec<u8>>,
    /// The tile's manifest capabilities, `tools.<tool name>` entries.
    pub caps: Vec<String>,
    /// The size the page opens it at.
    pub width: i32,
    pub height: i32,
}

struct Session {
    runner: TileRunner,
    used: Instant,
    /// The surface the tile was last told about.
    size: (i32, i32),
}

type Key = (String, String, String);

pub struct TileHost {
    defs: Mutex<Vec<TileDef>>,
    sessions: Mutex<HashMap<Key, Arc<Mutex<Option<Session>>>>>,
    limiter: Mutex<Limiter>,
    budgets: Budgets,
    max_sessions: std::sync::atomic::AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The tiles every machine has: the example counter (hand-written WAT, see `iyakki-tile-example`).
pub fn builtin() -> Vec<TileDef> {
    vec![TileDef { id: "counter".into(), title: "Counter (example tile)".into(), wasm: Arc::new(iyakki_tile_example::TILE_WAT.as_bytes().to_vec()), caps: vec![], width: 320, height: 200 }]
}

impl TileHost {
    pub fn new(defs: Vec<TileDef>) -> TileHost {
        TileHost { defs: Mutex::new(defs), sessions: Mutex::new(HashMap::new()), limiter: Mutex::new(Limiter::new(EVENTS_PER_MINUTE)), budgets: Budgets::default(), max_sessions: std::sync::atomic::AtomicUsize::new(MAX_SESSIONS) }
    }

    /// Lower (or raise) the number of tiles that may be running at once.
    pub fn set_max_sessions(&self, n: usize) {
        self.max_sessions.store(n.max(1), std::sync::atomic::Ordering::Relaxed);
    }

    /// Change the per-person request limit (a minute's worth); counting starts again.
    pub fn set_rate(&self, per_minute: u32) {
        *lock(&self.limiter) = Limiter::new(per_minute.max(1));
    }

    pub fn register(&self, def: TileDef) {
        let mut d = lock(&self.defs);
        d.retain(|x| x.id != def.id);
        d.push(def);
    }

    pub fn list(&self) -> Vec<TileDef> {
        lock(&self.defs).clone()
    }

    pub fn open_sessions(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// The person's slot for this tile, made if needed. Idle slots are dropped first; at the cap a new person is told to wait.
    fn slot(&self, key: &Key) -> Result<Arc<Mutex<Option<Session>>>, Fail> {
        let mut map = lock(&self.sessions);
        map.retain(|_, s| match s.try_lock() {
            Ok(g) => g.as_ref().is_some_and(|x| x.used.elapsed() < IDLE),
            Err(_) => true, // in use right now
        });
        if let Some(s) = map.get(key) {
            return Ok(s.clone());
        }
        if map.len() >= self.max_sessions.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Fail::new(429, "busy", "too many tiles are open on this machine; close one or wait a few minutes"));
        }
        let s = Arc::new(Mutex::new(None));
        map.insert(key.clone(), s.clone());
        Ok(s)
    }

    fn drop_slot(&self, key: &Key) {
        lock(&self.sessions).remove(key);
    }
}

/// The service calls a tile can make: `call("tools", <tool name>, <arguments as JSON>)`, and nothing else. The person's tool set,
/// narrowed by the profile's allow-list, narrowed again by the tile's own manifest (checked by the runner before this is reached).
struct TileBackend {
    tools: Arc<dyn Tools>,
    audit: Arc<dyn AuditSink>,
    user: String,
    tile: String,
}

impl TileBackend {
    fn note(&self, what: &str, tool: &str) {
        let tool: String = tool.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(60).collect();
        self.audit.record(&self.user, &format!("tile.{what}"), &format!("tile={} tool={tool}", self.tile));
    }
}

impl ServiceBackend for TileBackend {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain != "tools" {
            return Err("unknown service".into());
        }
        let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
        if !args.is_object() {
            return Err("the arguments are a JSON object".into());
        }
        let Some(def) = self.tools.defs().into_iter().find(|d| d.spec.name == method) else {
            self.note("refused", method);
            return Err("that tool is not available to you here".into());
        };
        if def.risk != Risk::Safe {
            // a browser tile has no approval prompt yet: a tool that asks first is not run from here
            self.note("refused", method);
            return Err("that tool asks first and cannot be used from a tile in a browser".into());
        }
        if let Some(why) = self.tools.refusal(method, &args) {
            self.note("refused", method);
            return Err(why);
        }
        self.note("call", method);
        let text = self.tools.call(method, &args)?;
        let text: String = text.chars().take(MAX_REPLY / 4).collect();
        Ok(json!({"ok": true, "text": text}).to_string())
    }
    fn denied(&self, domain: &str, method: &str, _json: &str) {
        self.note("denied", &format!("{domain}_{method}"));
    }
}

fn int(v: &Value, k: &str, lo: i64, hi: i64) -> Result<i64, Fail> {
    match v.get(k).and_then(Value::as_i64) {
        Some(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err(Fail::new(400, "invalid_input", &format!("\"{k}\" is a whole number from {lo} to {hi}"))),
    }
}

fn colour(v: &Value, k: &str, default: Color) -> Result<Color, Fail> {
    let Some(s) = v.get(k) else { return Ok(default) };
    let bad = || Fail::new(400, "invalid_input", &format!("\"{k}\" is a colour like #rrggbb"));
    let t = s.as_str().and_then(|s| s.strip_prefix('#')).ok_or_else(bad)?;
    if !(t.len() == 6 || t.len() == 8) || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let b = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).unwrap_or(0);
    Ok(Color(b(0), b(2), b(4), if t.len() == 8 { b(6) } else { 255 }))
}

fn theme(v: &Value) -> Result<Event, Fail> {
    let dark = match v.get("dark") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => return Err(Fail::new(400, "invalid_input", "\"dark\" is true or false")),
    };
    let (bg, fg) = if dark { (Color(24, 24, 24, 255), Color(230, 230, 230, 255)) } else { (Color(245, 245, 245, 255), Color(30, 30, 30, 255)) };
    Ok(Event::Theme { dark, accent: colour(v, "accent", Color(40, 110, 220, 255))?, background: colour(v, "background", bg)?, foreground: colour(v, "foreground", fg)? })
}

/// One event from the page. Only the ABI's input events exist here; `Hit` is made by the host from a pointer release, never accepted.
fn parse_event(v: &Value) -> Result<Event, Fail> {
    let c = limits::MAX_COORD as i64;
    let ty = v.get("type").and_then(Value::as_str).ok_or_else(|| Fail::new(400, "invalid_input", "every event has a \"type\""))?;
    Ok(match ty {
        "pointer_down" => Event::PointerDown { x: int(v, "x", -c, c)? as i32, y: int(v, "y", -c, c)? as i32, button: int(v, "button", 0, 255)? as u8 },
        "pointer_up" => Event::PointerUp { x: int(v, "x", -c, c)? as i32, y: int(v, "y", -c, c)? as i32, button: int(v, "button", 0, 255)? as u8 },
        "pointer_move" => Event::PointerMove { x: int(v, "x", -c, c)? as i32, y: int(v, "y", -c, c)? as i32 },
        "key" => {
            let down = match v.get("down") {
                Some(Value::Bool(b)) => *b,
                _ => return Err(Fail::new(400, "invalid_input", "\"down\" is true or false")),
            };
            Event::Key { code: int(v, "code", 0, 0xFFFF)? as u32, mods: int(v, "mods", 0, 15)? as u8, down }
        }
        "text" => {
            let t = v.get("text").and_then(Value::as_str).ok_or_else(|| Fail::new(400, "invalid_input", "\"text\" is a string"))?;
            if t.len() > limits::MAX_TEXT {
                return Err(Fail::new(400, "invalid_input", "the text is too long"));
            }
            Event::TextInput { text: t.to_string() }
        }
        "tick" => Event::Tick { dt_ms: int(v, "dt_ms", 0, 60_000)? as u32 },
        "resize" => Event::Resize { w: int(v, "w", 1, MAX_SURFACE)? as i32, h: int(v, "h", 1, MAX_SURFACE)? as i32, scale_pct: int(v, "scale_pct", 50, 800)? as u32 },
        "theme" => theme(v)?,
        _ => return Err(Fail::new(400, "invalid_input", "unknown event type")),
    })
}

fn tile_failure(e: &TileError) -> Fail {
    match e {
        TileError::VersionMismatch { .. } | TileError::MissingExport(_) | TileError::Load(_) => Fail::new(501, "unsupported", &e.to_string()),
        _ => Fail::new(502, "unavailable", &e.to_string()),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn handle(c: &Ctx, id: &str) -> Out {
    match id {
        "tile_list" => list(c),
        "tile_events" => events(c),
        _ => Err(Fail::new(404, "not_found", "not found")),
    }
}

/// The tiles this person may open. Every signed-in person may open a tile; what a tile may DO is decided call by call.
pub fn visible(host: &TileHost, _p: &iyakki_core_api::Principal) -> Vec<TileDef> {
    host.list()
}

fn list(c: &Ctx) -> Out {
    let tiles: Vec<Value> = visible(&c.sh.tiles, c.who()).iter().map(|d| json!({"id": d.id, "title": d.title, "abi": iyakki_tile_abi::ABI_VERSION, "width": d.width, "height": d.height})).collect();
    Ok((200, json!({"tiles": tiles}), String::new()))
}

fn events(c: &Ctx) -> Out {
    let p = c.who();
    let tile_id = c.params.first().map(String::as_str).unwrap_or("");
    if !valid_id(tile_id) {
        return Err(Fail::new(404, "not_found", "no such tile"));
    }
    if c.req.body.len() > MAX_BODY {
        return Err(Fail::new(413, "too_large", "the body is too large"));
    }
    let host = &c.sh.tiles;
    let Some(def) = visible(host, p).into_iter().find(|d| d.id == tile_id) else { return Err(Fail::new(404, "not_found", "no such tile")) };
    let body = super::server::json_body(c.req)?;
    if !body.is_object() {
        return Err(Fail::new(400, "invalid_input", "the body is a JSON object"));
    }
    let raw = match body.get("events") {
        None => &[][..],
        Some(Value::Array(a)) => a.as_slice(),
        _ => return Err(Fail::new(400, "invalid_input", "\"events\" is a list")),
    };
    if raw.len() > MAX_EVENTS {
        return Err(Fail::new(413, "too_large", &format!("at most {MAX_EVENTS} events in one request")));
    }
    let mut evs = Vec::with_capacity(raw.len() + 4);
    for e in raw {
        evs.push(parse_event(e)?);
    }
    let reset = matches!(body.get("reset"), Some(Value::Bool(true)));
    let (ok_rate, wait) = lock(&host.limiter).allow(&p.user);
    if !ok_rate {
        return Err(Fail::new(429, "rate_limited", &format!("too many tile events; wait {wait} seconds")));
    }
    let dir = c.data_dir();
    let key: Key = (p.user.clone(), dir.to_string_lossy().into_owned(), def.id.clone());
    let slot = host.slot(&key)?;
    let mut guard = lock(&slot);
    if reset {
        *guard = None;
    }
    if guard.is_none() {
        // starting needs a surface size
        let params = (|| -> Result<(i32, i32, u32, Event), Fail> {
            let (w, h) = (int(&body, "width", 1, MAX_SURFACE)? as i32, int(&body, "height", 1, MAX_SURFACE)? as i32);
            let scale = match body.get("scale_pct") {
                None => 100,
                Some(_) => int(&body, "scale_pct", 50, 800)? as u32,
            };
            Ok((w, h, scale, theme(&body)?))
        })();
        let (w, h, scale, th) = match params {
            Ok(p) => p,
            Err(f) => {
                // nothing started: do not leave an empty place behind
                drop(guard);
                host.drop_slot(&key);
                return Err(f);
            }
        };
        let backend: Arc<dyn ServiceBackend> = Arc::new(TileBackend {
            tools: Arc::new(super::tools::ProfileTools::new(c.sh.deps.tools.tools_for(p), c.sh.allowed_tools.clone())),
            audit: c.sh.deps.audit.clone(),
            user: p.user.clone(),
            tile: def.id.clone(),
        });
        let manifest = TileManifest { name: def.id.clone(), abi: iyakki_tile_abi::ABI_VERSION, caps: def.caps.clone() };
        // the seed makes the tile's randomness differ between people and between starts, and be recorded nowhere else
        let seed = {
            let mut s = [0u8; 8];
            let _ = iyakki_sec::random_fill(&mut s);
            u64::from_le_bytes(s)
        };
        let started = TileRunner::load(&def.wasm, manifest, host.budgets.clone(), Some(backend), seed).and_then(|mut r| r.init(w, h, scale, th).map(|_| r));
        match started {
            Ok(runner) => *guard = Some(Session { runner, used: Instant::now(), size: (w, h) }),
            Err(e) => {
                drop(guard);
                host.drop_slot(&key);
                return Err(tile_failure(&e));
            }
        }
    }
    let s = guard.as_mut().expect("started above");
    s.used = Instant::now();

    // a release also becomes a Hit for the topmost hit region of the last picture, like `TileRunner::click` on the device
    let mut batch = Vec::with_capacity(evs.len() * 2);
    for e in evs {
        if let Event::Resize { w, h, .. } = &e {
            s.size = (*w, *h);
        }
        let hit = match &e {
            Event::PointerUp { x, y, .. } => s.runner.last_frame().and_then(|l| l.hit_test(*x, *y)),
            _ => None,
        };
        batch.push(e);
        if let Some(id) = hit {
            batch.push(Event::Hit { id });
        }
    }
    batch.truncate(limits::MAX_EVENTS);
    let size = s.size;
    let step = (|| -> Result<(bool, Value, u64, usize), TileError> {
        let redraw = if batch.is_empty() { false } else { s.runner.event(&batch)? };
        let fuel = s.runner.last_stats.fuel_used;
        let list = s.runner.frame()?;
        let (w, h) = size;
        let text = export::to_json(&list, w, h);
        let frame: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok((redraw, frame, fuel, text.len()))
    })();
    match step {
        Ok((redraw, frame, fuel, bytes)) => {
            let detail = format!("tile={} events={}", def.id, batch.len());
            Ok((200, json!({"id": def.id, "redraw": redraw, "frame": frame, "stats": {"fuel": fuel, "bytes": bytes}}), detail))
        }
        Err(e) => {
            // a stopped or crashed tile loses its state (same as a hot reload on the device); the page offers Reload
            drop(guard);
            host.drop_slot(&key);
            Err(tile_failure(&e))
        }
    }
}

/// The rows the chat tool `tiles_list` shows: the tiles a person may open and where.
pub fn describe(host: &TileHost) -> Vec<(String, String)> {
    host.list().into_iter().map(|d| (d.id, d.title)).collect()
}
