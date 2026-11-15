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
//! checked against the PERSON: only tools in the service profile's list, a tool that asks first is not run until the person answers an
//! approval card in the browser (the tile is told to try again; an answer is good for one identical call), and the same audit log.
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
    /// Pictures the tile may draw: (handle, PNG bytes). Decoded ONCE when the tile starts, with the host's own decoder, into the tile's
    /// picture table (at most 64 pictures, 1024 x 1024 each, 4 MB of pixels); a tile only names a handle, so nothing a tile does at run
    /// time allocates picture memory, and a bad or too big picture stops the tile from starting.
    pub pictures: Vec<(u32, Arc<Vec<u8>>)>,
}

struct Session {
    runner: TileRunner,
    used: Instant,
    /// The surface the tile was last told about.
    size: (i32, i32),
    /// The pictures the tile may draw (from its definition), sent with every frame that names them.
    images: iyakki_tile_host::ImageTable,
}

type Key = (String, String, String);

pub struct TileHost {
    defs: Mutex<Vec<TileDef>>,
    sessions: Mutex<HashMap<Key, Arc<Mutex<Option<Session>>>>>,
    limiter: Mutex<Limiter>,
    budgets: Budgets,
    max_sessions: std::sync::atomic::AtomicUsize,
    /// Tool calls a tile made that ask first, waiting for the person's own click in the browser.
    pub asks: Arc<TileAsks>,
}

/// How long a tile's question (or the person's answer to it) stays good.
/// The most a tile reads or writes of one file (its reply buffer is 24 KiB).
pub const MAX_FILE: usize = 12 * 1024;

const ASK_LIFE: Duration = Duration::from_secs(600);
/// Questions waiting at once for one person; more are refused until some are answered.
const MAX_ASKS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AskState {
    Waiting,
    Approved,
    Denied,
}

struct Ask {
    id: String,
    user: String,
    tile: String,
    tool: String,
    args: String,
    /// What will happen, in words (the tool's own description), shown on the card.
    summary: String,
    state: AskState,
    at: Instant,
}

/// What the person sees for one waiting question.
pub struct AskView {
    pub id: String,
    pub tile: String,
    pub tool: String,
    pub detail: String,
    pub summary: String,
    pub age_secs: u64,
}

/// Questions from tiles. An answer is good for one call of the same tool with the same arguments, by the same tile, for the same
/// person, within ten minutes; nothing else can use it, and only the person's own answer (never the tile, never model text) sets it.
#[derive(Default)]
pub struct TileAsks {
    list: Mutex<Vec<Ask>>,
}

#[derive(Debug)]
pub enum AskOutcome {
    /// The person approved this exact call: run it once.
    Run,
    Denied,
    Waiting,
    /// A new question was made (its id).
    Asked(String),
    TooMany,
}

impl TileAsks {
    fn sweep(list: &mut Vec<Ask>) {
        list.retain(|a| a.at.elapsed() < ASK_LIFE);
    }

    #[cfg(test)]
    pub fn check_for_test(&self, user: &str, tile: &str, tool: &str, args: &str) -> AskOutcome {
        self.check(user, tile, tool, args, "")
    }

    fn check(&self, user: &str, tile: &str, tool: &str, args: &str, summary: &str) -> AskOutcome {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        if let Some(i) = l.iter().position(|a| a.user == user && a.tile == tile && a.tool == tool && a.args == args) {
            return match l[i].state {
                AskState::Waiting => AskOutcome::Waiting,
                s => {
                    l.remove(i);
                    if s == AskState::Approved { AskOutcome::Run } else { AskOutcome::Denied }
                }
            };
        }
        if l.iter().filter(|a| a.user == user).count() >= MAX_ASKS {
            return AskOutcome::TooMany;
        }
        let mut b = [0u8; 8];
        let _ = iyakki_sec::random_fill(&mut b);
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        l.push(Ask { id: id.clone(), user: user.into(), tile: tile.into(), tool: tool.into(), args: args.into(), summary: summary.chars().take(300).collect(), state: AskState::Waiting, at: Instant::now() });
        AskOutcome::Asked(id)
    }

    /// The person's waiting questions, oldest first.
    pub fn waiting(&self, user: &str) -> Vec<AskView> {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        l.iter()
            .filter(|a| a.user == user && a.state == AskState::Waiting)
            .map(|a| AskView { id: a.id.clone(), tile: a.tile.clone(), tool: a.tool.clone(), detail: a.args.chars().take(400).collect(), summary: a.summary.clone(), age_secs: a.at.elapsed().as_secs() })
            .collect()
    }

    /// The person's answer. Only their own questions are found; answering twice is refused.
    pub fn decide(&self, user: &str, id: &str, approve: bool) -> Result<(String, String), &'static str> {
        let mut l = lock(&self.list);
        Self::sweep(&mut l);
        let Some(a) = l.iter_mut().find(|a| a.user == user && a.id == id) else { return Err("no such waiting request") };
        if a.state != AskState::Waiting {
            return Err("already answered");
        }
        a.state = if approve { AskState::Approved } else { AskState::Denied };
        a.at = Instant::now();
        Ok((a.tile.clone(), a.tool.clone()))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The tiles every machine has: the example counter (hand-written WAT, see `iyakki-tile-example`).
pub fn builtin() -> Vec<TileDef> {
    vec![
        TileDef { id: "counter".into(), title: "Counter (example tile)".into(), wasm: Arc::new(iyakki_tile_example::TILE_WAT.as_bytes().to_vec()), caps: vec![], width: 320, height: 200, pictures: vec![] },
        // the Runs page as a tile (F-WEB-55): reads the person's own runs through the `runs` service domain, nothing else
        // the Settings overview, the scratch editor and the chat as tiles (F-WEB-56, F-WEB-54, F-WEB-53): first slices, see the rows
        TileDef { id: "settings".into(), title: "Settings (overview)".into(), wasm: Arc::new(iyakki_tile_settings::TILE_WASM.to_vec()), caps: iyakki_tile_settings::CAPS.iter().map(|s| s.to_string()).collect(), width: 360, height: 480, pictures: vec![] },
        TileDef { id: "edit".into(), title: "Edit (scratch)".into(), wasm: Arc::new(iyakki_tile_edit::TILE_WASM.to_vec()), caps: iyakki_tile_edit::CAPS.iter().map(|s| s.to_string()).collect(), width: 480, height: 360, pictures: vec![] },
        TileDef { id: "chat".into(), title: "Chat".into(), wasm: Arc::new(iyakki_tile_chat::TILE_WASM.to_vec()), caps: iyakki_tile_chat::CAPS.iter().map(|s| s.to_string()).collect(), width: 360, height: 480, pictures: vec![] },
        TileDef { id: "runs".into(), title: "Runs".into(), wasm: Arc::new(iyakki_tile_runs::TILE_WASM.to_vec()), caps: iyakki_tile_runs::CAPS.iter().map(|s| s.to_string()).collect(), width: 360, height: 480, pictures: vec![] },
    ]
}

impl TileHost {
    pub fn new(defs: Vec<TileDef>) -> TileHost {
        TileHost { defs: Mutex::new(defs), sessions: Mutex::new(HashMap::new()), limiter: Mutex::new(Limiter::new(EVENTS_PER_MINUTE)), budgets: Budgets::default(), asks: Arc::new(TileAsks::default()), max_sessions: std::sync::atomic::AtomicUsize::new(MAX_SESSIONS) }
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
    /// The person's whole tool set (not narrowed to the profile): only the `files` domain reaches it, and only for `read_file` and `write_file`.
    all_tools: Arc<dyn Tools>,
    audit: Arc<dyn AuditSink>,
    user: String,
    tile: String,
    asks: Arc<TileAsks>,
    notify: super::runs::Notify,
    runs: super::runs::RunReader,
    /// The person may use the chat (the `Chat` right the routes check): the `runs` and `chat` domains are closed without it.
    chat_ok: bool,
    /// `settings.get`: what the overview shows, built for this person.
    settings: Arc<dyn Fn() -> Value + Send + Sync>,
    /// `chat.send`: starts a run for this person with the machine's model and their limits.
    chat_send: Arc<dyn Fn(&str, Option<(String, String)>) -> Result<String, String> + Send + Sync>,
    /// `chat.models`: the models a tile may choose from (the machine's list without the coding agents that need a tenant's switch).
    chat_models: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    /// `chat.stop`: stops one of the person's own runs.
    chat_stop: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    /// `runs.get` adds the questions of the run that wait for this person: (call id, tool, summary).
    pending_of: Arc<dyn Fn(&str) -> Vec<(String, String, String)> + Send + Sync>,
    /// `runs.decide`: the person's answer to one of their own waiting questions (true: approve); returns the tool's name.
    decide: Arc<dyn Fn(&str, &str, bool) -> Result<String, String> + Send + Sync>,
}

impl TileBackend {
    fn note(&self, what: &str, tool: &str) {
        let tool: String = tool.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(60).collect();
        self.audit.record(&self.user, &format!("tile.{what}"), &format!("tile={} tool={tool}", self.tile));
    }
}

/// One event of a run as a line of text for a tile (nothing for what is only for a screen).
fn run_line(e: &Value) -> Option<String> {
    let s = |k: &str| e[k].as_str().unwrap_or("");
    let line = match e["type"].as_str()? {
        "model" if !s("text").trim().is_empty() => format!("assistant: {}", s("text")),
        "approval" => format!("asks: {}", s("summary")),
        "decision" => format!("{} {} ({})", s("name"), if e["approved"].as_bool().unwrap_or(false) { "approved" } else { "refused" }, s("by")),
        "tool_result" => format!("tool {} {}: {}", s("name"), if e["ok"].as_bool().unwrap_or(false) { "ok" } else { "failed" }, s("text")),
        "final" => format!("answer: {}", s("text")),
        "failed" => format!("failed: {}", s("error")),
        "stopped" => format!("stopped: {}", s("reason")),
        "note" => s("text").to_string(),
        _ => return None,
    };
    Some(line.chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect())
}

impl TileBackend {
    /// The `runs` domain: the person's own runs, read only (the list, and the lines of one run).
    fn runs_call(&self, method: &str, json: &str) -> Result<String, String> {
        if !self.chat_ok {
            self.note("refused", method);
            return Err("your account may not use the chat".into());
        }
        match method {
            "list" => {
                self.note("call", "runs_list");
                let rows: Vec<Value> = self.runs.list().into_iter().take(40).map(|r| {
                    json!({"id": r["id"], "status": r["status"], "prompt": r["prompt"].as_str().unwrap_or("").chars().take(100).collect::<String>(), "started": r["started"]})
                }).collect();
                Ok(json!({"runs": rows}).to_string())
            }
            "get" => {
                let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
                    return Err("that is not a run id".into());
                }
                self.note("call", "runs_get");
                let (status, events, _, done) = self.runs.view(id, 0).map_err(|_| "no such run".to_string())?;
                let mut lines: Vec<String> = events.iter().filter_map(run_line).collect();
                if lines.len() > 60 {
                    lines.drain(..lines.len() - 60);
                }
                let pending: Vec<Value> = (self.pending_of)(id).into_iter().map(|(c, t, s)| json!({"call": c, "tool": t, "summary": s.chars().take(200).collect::<String>()})).collect();
                Ok(json!({"id": id, "status": status, "done": done, "lines": lines, "pending": pending}).to_string())
            }
            "stop" => {
                // the person's own click on Stop in the Runs tile: one of their own runs is asked to stop (the same call the chat tile's Stop makes)
                let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
                    return Err("that is not a run id".into());
                }
                self.note("call", "runs_stop");
                (self.chat_stop)(id)?;
                Ok(json!({"ok": true}).to_string())
            }
            "decide" => {
                // the person's own click in the Runs tile (the host adds a Hit only for a real pointer release): approve or deny ONE waiting question
                // of one of their own runs, once; the same rules as the approvals route (a question that is not theirs or not waiting is "not found")
                let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let (run, call) = (args["run"].as_str().unwrap_or(""), args["call"].as_str().unwrap_or(""));
                let ok_id = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
                let approve = match args["verdict"].as_str() {
                    Some("approve") => true,
                    Some("deny") => false,
                    _ => return Err("the verdict is approve or deny".into()),
                };
                if !ok_id(run) || !ok_id(call) {
                    return Err("that is not a run or a question".into());
                }
                self.note("call", "runs_decide");
                let tool = (self.decide)(run, call, approve)?;
                self.audit.record(&self.user, "tile.decide", &format!("tile={} run={run} call={call} tool={} verdict={}", self.tile, tool.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(60).collect::<String>(), if approve { "approve" } else { "deny" }));
                Ok(json!({"ok": true}).to_string())
            }
            _ => {
                self.note("refused", method);
                Err("that is not a runs call".into())
            }
        }
    }
}

/// The switches a tile may change, each by one of the person's own Settings tools: (id, label, read tool, change tool). A switch is
/// listed only for an account that has both tools (the sound and battery ones are administrator tools).
const SWITCHES: [(&str, &str, &str, &str); 4] = [
    ("flows_notices", "Flow notices in the bell", "notification_filters", "notification_filter_set"),
    ("saver_battery", "Shorter saver on battery", "screensaver_status", "screensaver_set"),
    ("system_sounds", "System sound cues", "system_sounds_status", "system_sounds_set"),
    ("battery_percent", "Battery percent shown", "battery_settings_status", "battery_settings_set"),
];

/// The state of a switch, from the text of its read tool.
fn switch_state(id: &str, text: &str) -> Option<bool> {
    match id {
        "flows_notices" => text.lines().find_map(|l| l.trim().strip_prefix("flows:")).map(|r| r.trim_start().starts_with("show")),
        "saver_battery" => text.split("Shorter on battery:").nth(1).map(|r| r.trim_start().starts_with("yes")),
        "system_sounds" => text.trim_start().strip_prefix("System sounds are").map(|r| r.trim_start().starts_with("on")),
        "battery_percent" => text.split("Percentage in the top bar:").nth(1).map(|r| r.trim_start().starts_with("shown")),
        _ => None,
    }
}

/// The arguments of the change tool for a switch.
fn switch_args(id: &str, on: bool) -> Option<Value> {
    match id {
        "flows_notices" => Some(json!({"source": "flows", "level": if on { "show" } else { "quiet" }})),
        "saver_battery" => Some(json!({"battery_shorter": on})),
        "system_sounds" => Some(json!({"enabled": on})),
        "battery_percent" => Some(json!({"show_percent": on})),
        _ => None,
    }
}

impl TileBackend {
    /// The switches this person may see: the ones whose tools their account has (their state is read by the free tool).
    fn toggles(&self) -> Vec<Value> {
        let defs = self.all_tools.defs();
        SWITCHES
            .iter()
            .filter(|(_, _, read, change)| defs.iter().any(|d| d.spec.name == *read) && defs.iter().any(|d| d.spec.name == *change))
            .filter_map(|(id, label, read, _)| {
                let text = self.all_tools.call(read, &json!({})).ok()?;
                Some(json!({"id": id, "label": label, "on": switch_state(id, &text)?}))
            })
            .collect()
    }

    /// `settings.set {id, on}`: one of the listed switches, through the person's own change tool, which asks first (an approval card, the
    /// person's own click). Answers: `{ok:true}`, `{ok:false,asked:true}` while the question waits, or `{ok:false,error}`.
    fn toggle_set(&self, json: &str) -> Result<String, String> {
        if !self.chat_ok {
            self.note("refused", "settings_set");
            return Err("your account may not use the chat".into());
        }
        let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
        let (Some(id), Some(on)) = (args["id"].as_str(), args["on"].as_bool()) else { return Err("send {id, on}".into()) };
        let Some((_, _, _, tool)) = SWITCHES.iter().find(|s| s.0 == id) else {
            self.note("refused", "settings_set");
            return Err("that is not a switch a tile may change".into());
        };
        let fail = |e: &str| Ok(json!({"ok": false, "error": e}).to_string());
        let targs = switch_args(id, on).unwrap_or(Value::Null);
        if !self.all_tools.defs().iter().any(|d| d.spec.name == *tool) {
            self.note("refused", tool);
            return fail("your account may not change that");
        }
        if let Some(why) = self.all_tools.refusal(tool, &targs) {
            self.note("refused", tool);
            return fail(&why);
        }
        match self.ask_with(&self.all_tools, tool, &targs) {
            AskOutcome::Run => {}
            AskOutcome::Denied => return fail("you said no"),
            AskOutcome::Waiting | AskOutcome::Asked(_) => return Ok(json!({"ok": false, "asked": true}).to_string()),
            AskOutcome::TooMany => return fail("too many questions are waiting for you"),
        }
        self.note("call", tool);
        match self.all_tools.call(tool, &targs) {
            Ok(_) => Ok(json!({"ok": true}).to_string()),
            Err(e) => fail(&e.chars().take(300).collect::<String>()),
        }
    }

    /// The question for a tool that asks first, through `tools` (the same ones the tool itself would ask): made, waiting, answered, or refused.
    /// A new question is announced on the person's event stream; every outcome is one audit line without arguments.
    fn ask_with(&self, tools: &Arc<dyn Tools>, name: &str, args: &Value) -> AskOutcome {
        let summary = tools.describe(name, args);
        let r = self.asks.check(&self.user, &self.tile, name, &args.to_string(), &summary);
        match &r {
            AskOutcome::Run => {}
            AskOutcome::Denied => self.note("ask_denied", name),
            AskOutcome::Waiting => {}
            AskOutcome::TooMany => self.note("refused", name),
            AskOutcome::Asked(id) => {
                self.note("asked", name);
                (self.notify)(crate::notifyhub::events::event("approval", json!({"run": "tile", "call": id, "tool": name})));
            }
        }
        r
    }

    fn ask(&self, name: &str, args: &Value) -> AskOutcome {
        self.ask_with(&self.tools, name, args)
    }

    /// The `files` domain (F-WEB-54): `read {path}` and `write {path, text}` on the person's own files, by their own `read_file` and
    /// `write_file` tools (their folders, their write rules, the tool's own refusals), which the service profiles do not list for a
    /// browser. A write is a tool that asks first: it waits for the person's click on the approval card like any tile call. Answers are
    /// always JSON the tile can read: `{ok:true,text}`, `{ok:false,asked:true}` while the question waits, or `{ok:false,error}`.
    fn files_call(&self, method: &str, json: &str) -> Result<String, String> {
        if !self.chat_ok {
            self.note("refused", method);
            return Err("your account may not use the chat".into());
        }
        let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
        let path = args["path"].as_str().unwrap_or("");
        if path.is_empty() || path.len() > 500 || path.chars().any(char::is_control) {
            return Ok(json!({"ok": false, "error": "that is not a file name"}).to_string());
        }
        let fail = |e: &str| Ok(json!({"ok": false, "error": e}).to_string());
        let (tool, targs) = match method {
            "read" => ("read_file", json!({"path": path})),
            "write" => {
                let text = args["text"].as_str().unwrap_or("");
                if text.len() > MAX_FILE {
                    return fail("the text is larger than 12 KiB, which is all a tile can save");
                }
                ("write_file", json!({"path": path, "content": text}))
            }
            _ => {
                self.note("refused", method);
                return Err("that is not a files call".into());
            }
        };
        if !self.all_tools.defs().iter().any(|d| d.spec.name == tool) {
            self.note("refused", tool);
            return fail(if tool == "read_file" { "your account may not read files" } else { "your account may not write files" });
        }
        if let Some(why) = self.all_tools.refusal(tool, &targs) {
            self.note("refused", tool);
            return fail(&why);
        }
        if tool == "write_file" {
            match self.ask_with(&self.all_tools, tool, &targs) {
                AskOutcome::Run => {}
                AskOutcome::Denied => return fail("you said no"),
                AskOutcome::Waiting | AskOutcome::Asked(_) => return Ok(json!({"ok": false, "asked": true}).to_string()),
                AskOutcome::TooMany => return fail("too many questions are waiting for you"),
            }
        }
        self.note("call", tool);
        match self.all_tools.call(tool, &targs) {
            Ok(text) if method == "read" && text.len() > MAX_FILE => fail("the file is larger than 12 KiB, which is all a tile can open"),
            Ok(text) => Ok(json!({"ok": true, "text": text}).to_string()),
            Err(e) => fail(&e.chars().take(300).collect::<String>()),
        }
    }

    /// The `settings` domain (`get`: an overview that only reads) and the `chat` domain (`send`: one message starts one run).
    fn page_call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        match (domain, method) {
            ("settings", "get") => {
                self.note("call", "settings_get");
                let mut v = (self.settings)();
                if self.chat_ok {
                    v["toggles"] = json!(self.toggles());
                }
                Ok(v.to_string())
            }
            ("settings", "set") => self.toggle_set(json),
            ("chat", "models") | ("chat", "stop") | ("chat", "send") if !self.chat_ok => {
                self.note("refused", &format!("chat_{method}"));
                Err("your account may not use the chat".into())
            }
            ("chat", "models") => {
                self.note("call", "chat_models");
                let v: Vec<Value> = (self.chat_models)().into_iter().map(|(p, m)| json!({"provider": p, "model": m})).collect();
                Ok(json!({"models": v}).to_string())
            }
            ("chat", "stop") => {
                let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
                    return Err("that is not a run id".into());
                }
                self.note("call", "chat_stop");
                (self.chat_stop)(id)?;
                Ok(json!({"ok": true}).to_string())
            }
            ("chat", "send") => {
                let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let prompt = args["prompt"].as_str().unwrap_or("");
                let model = match (args["provider"].as_str(), args["model"].as_str()) {
                    (None, None) => None,
                    (Some(p), Some(m)) => {
                        // only a pair the service itself lists, and never a coding agent (those need a tenant's switch and are chosen on the Chat page)
                        if !(self.chat_models)().iter().any(|(lp, lm)| lp == p && lm == m) {
                            self.note("refused", "chat_send");
                            return Err("that model is not one you can choose here".into());
                        }
                        Some((p.to_string(), m.to_string()))
                    }
                    _ => return Err("give the provider and the model together, or neither".into()),
                };
                self.note("call", "chat_send");
                // a run that could not be started is an answer the tile can show (the model rules, the limit of runs at once ...)
                Ok(match (self.chat_send)(prompt, model) {
                    Ok(id) => json!({"id": id}),
                    Err(e) => json!({"error": e.chars().take(200).collect::<String>()}),
                }
                .to_string())
            }
            _ => {
                self.note("refused", method);
                Err("that is not a call this tile may make".into())
            }
        }
    }
}

impl ServiceBackend for TileBackend {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain == "runs" {
            return self.runs_call(method, json);
        }
        if domain == "files" {
            return self.files_call(method, json);
        }
        if domain == "settings" || domain == "chat" {
            return self.page_call(domain, method, json);
        }
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
        let asks_first = def.risk != Risk::Safe;
        if let Some(why) = self.tools.refusal(method, &args) {
            self.note("refused", method);
            return Err(why);
        }
        if asks_first {
            // a tool that asks first runs only on this person's own click: the question shows in their browser as an approval card
            match self.ask(method, &args) {
                AskOutcome::Run => {}
                AskOutcome::Denied => return Err("the person said no".into()),
                AskOutcome::Waiting => return Err("still waiting for the person to answer; try again after they do".into()),
                AskOutcome::TooMany => return Err("too many questions are waiting for the person".into()),
                AskOutcome::Asked(_) => return Err("this tool asks first: the person was asked in their browser; try the same call again after they answer".into()),
            }
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

/// The picture table of a tile: every declared picture decoded and checked against the table's caps before the tile runs.
fn picture_table(def: &TileDef) -> Result<iyakki_tile_host::ImageTable, Fail> {
    let mut t = iyakki_tile_host::ImageTable::new();
    for (handle, png) in &def.pictures {
        let img = crate::qaimage::decode_png(png).map_err(|e| Fail::new(501, "unsupported", &format!("the tile {} has a picture (handle {handle}) that cannot be read: {e}", def.id)))?;
        t.put(*handle, img.w as i32, img.h as i32, img.rgba).map_err(|e| Fail::new(501, "unsupported", &format!("the tile {} has a picture (handle {handle}) the table refuses: {e:?}", def.id)))?;
    }
    Ok(t)
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
    let tiles: Vec<Value> = visible(&c.sh.tiles, c.who()).iter().map(|d| json!({"id": d.id, "title": d.title, "abi": iyakki_tile_abi::ABI_VERSION, "width": d.width, "height": d.height, "ticks": !d.caps.is_empty()})).collect();
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
        let all_tools = c.sh.deps.tools.tools_for(p);
        let backend: Arc<dyn ServiceBackend> = Arc::new(TileBackend {
            tools: Arc::new(super::tools::ProfileTools::new(all_tools.clone(), c.sh.allowed_tools.clone())),
            all_tools,
            audit: c.sh.deps.audit.clone(),
            user: p.user.clone(),
            tile: def.id.clone(),
            asks: host.asks.clone(),
            notify: c.notifier(),
            runs: c.sh.runs.reader(p),
            chat_ok: p.policy.agent && p.policy.apps.contains(&iyakki_sec::policy::App::Chat),
            settings: {
                let (deps, who, tools) = (c.sh.deps.clone(), p.clone(), c.sh.allowed_tools.len());
                Arc::new(move || {
                    let chat = who.policy.agent && who.policy.apps.contains(&iyakki_sec::policy::App::Chat);
                    json!({
                        "user": who.user, "role": who.role, "admin": who.admin,
                        "models": if chat { deps.extras.models(&who).into_iter().take(10).collect::<Vec<_>>() } else { vec![] },
                        "web": if who.admin { deps.extras.web_access(&who).ok().map(|w| json!({"on": w["on"], "port": w["port"], "status": w["status"]})) } else { None },
                        "tools": if chat { tools } else { 0 },
                    })
                })
            },
            chat_send: {
                let (runs, who, notify, conc) = (c.sh.runs.clone(), p.clone(), c.notifier(), c.sh.cfg.limits.concurrent_runs);
                let allowed = super::tools::allowed_set(c.sh.profile, &c.sh.cfg.enable_tools, &[]).unwrap_or_default();
                Arc::new(move |prompt: &str, model: Option<(String, String)>| runs.start(&who, prompt, model.as_ref().map(|(p, m)| (p.as_str(), m.as_str())), allowed.clone(), conc, Some(notify.clone())).map_err(|e| e.to_string()))
            },
            chat_models: {
                let (deps, who) = (c.sh.deps.clone(), p.clone());
                Arc::new(move || {
                    deps.extras.models(&who).into_iter().filter_map(|m| Some((m["provider"].as_str()?.to_string(), m["model"].as_str()?.to_string()))).filter(|(p, _)| !matches!(p.as_str(), "claude" | "copilot")).take(10).collect()
                })
            },
            pending_of: {
                let (deps, user) = (c.sh.deps.clone(), p.user.clone());
                Arc::new(move |run: &str| deps.store.pending_approvals(Some(&user)).into_iter().filter(|a| a.run_id == run).map(|a| (a.call_id, a.tool, a.summary)).collect())
            },
            decide: {
                let (deps, user) = (c.sh.deps.clone(), p.user.clone());
                Arc::new(move |run: &str, call: &str, approve: bool| {
                    let a = deps.store.pending_approvals(Some(&user)).into_iter().find(|a| a.run_id == run && a.call_id == call).ok_or_else(|| "no such waiting question".to_string())?;
                    let verdict = if approve { crate::agent::Verdict::Approve } else { crate::agent::Verdict::Deny };
                    deps.store.decide_pending(&user, false, run, call, verdict).map_err(|e| e.to_string())?;
                    Ok(a.tool)
                })
            },
            chat_stop: {
                let (runs, who) = (c.sh.runs.clone(), p.clone());
                Arc::new(move |id: &str| runs.stop(&who, id).map_err(|e| e.to_string()))
            },
        });
        let images = match picture_table(&def) {
            Ok(t) => t,
            Err(f) => {
                drop(guard);
                host.drop_slot(&key);
                return Err(f);
            }
        };
        let manifest = TileManifest { name: def.id.clone(), abi: iyakki_tile_abi::ABI_VERSION, caps: def.caps.clone() };
        // the seed makes the tile's randomness differ between people and between starts, and be recorded nowhere else
        let seed = {
            let mut s = [0u8; 8];
            let _ = iyakki_sec::random_fill(&mut s);
            u64::from_le_bytes(s)
        };
        let started = TileRunner::load(&def.wasm, manifest, host.budgets.clone(), Some(backend), seed).and_then(|mut r| r.init(w, h, scale, th).map(|_| r));
        match started {
            Ok(runner) => *guard = Some(Session { runner, used: Instant::now(), size: (w, h), images }),
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
        let text = export::to_json_with_images(&list, w, h, &s.images);
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
