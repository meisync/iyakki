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
    images: iyakki_tile_host::pictures::SharedTable,
    /// The tile's registration for the core's app events (F-SDK-202), when it is ABI 3 and listens for any; dropped with the session.
    feed: Option<super::tileevents::Feed>,
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

/// The per-call budgets of the tiles the web service runs. The fuel limits are the real, deterministic ones; the wall limit is only a safety net
/// against a hang, so the tests (which run thousands at once on a shared host) get a wide one: a loaded machine must not stop a tile.
fn host_budgets() -> Budgets {
    let mut b = Budgets::default();
    if cfg!(test) {
        b.wall = std::time::Duration::from_secs(10);
    }
    b
}

impl TileHost {
    pub fn new(defs: Vec<TileDef>) -> TileHost {
        TileHost { defs: Mutex::new(defs), sessions: Mutex::new(HashMap::new()), limiter: Mutex::new(Limiter::new(EVENTS_PER_MINUTE)), budgets: host_budgets(), asks: Arc::new(TileAsks::default()), max_sessions: std::sync::atomic::AtomicUsize::new(MAX_SESSIONS) }
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
    /// `dock.status`: the tile sets the status of its own icon (`Services::set_tile_status`).
    status: Arc<dyn Fn(Option<u32>, &str) + Send + Sync>,
    /// `home.list` and `home.read`: the person's home folder through the core's files facet.
    files: Arc<dyn iyakki_core_api::Files>,
    /// `browser.*`: the embedded browser sessions of the core, keyed by this tile's id.
    browser: Arc<dyn iyakki_core_api::BrowserSessions>,
    /// `assistant.*`: the core's embedded assistant for this tile's id; `assistant_own` is what this tile opened and the cards it was shown.
    assistant: Arc<dyn iyakki_core_api::Assistant>,
    assistant_own: Mutex<super::tileassistant::Own>,
    /// `cameras.*`: the core's camera views for this tile's id; `cameras_own` is the views this tile opened and the cards it was shown.
    cameras: Arc<dyn iyakki_core_api::Cameras>,
    cameras_own: Mutex<super::tilecameras::Own>,
    /// `settings.get`: what the overview shows, built for this person.
    settings: Arc<dyn Fn() -> Value + Send + Sync>,
    /// `chat.send`: starts a run for this person with the machine's model and their limits.
    /// Starts a run: the prompt, the model, and the only tool names the run may use (`None`: every tool of the profile).
    chat_send: Arc<dyn Fn(&str, Option<(String, String)>, Option<std::collections::BTreeSet<String>>) -> Result<String, String> + Send + Sync>,
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
const SWITCHES: [(&str, &str, &str, &str); 8] = [
    ("flows_notices", "Flow notices in the bell", "notification_filters", "notification_filter_set"),
    ("saver_battery", "Shorter saver on battery", "screensaver_status", "screensaver_set"),
    ("system_sounds", "System sound cues", "system_sounds_status", "system_sounds_set"),
    ("battery_percent", "Battery percent shown", "battery_settings_status", "battery_settings_set"),
    // the accessibility choices (one status tool and one change tool for all four; `accessibility_set` takes any of its flags)
    ("a11y_reduce_motion", "Reduce motion", "accessibility_status", "accessibility_set"),
    ("a11y_large_pointer", "Large pointer", "accessibility_status", "accessibility_set"),
    ("a11y_big_focus_ring", "Big focus ring", "accessibility_status", "accessibility_set"),
    ("a11y_typing_echo", "Say typed letters", "accessibility_status", "accessibility_set"),
];

/// The words the accessibility status uses for a flag, and the name the change tool takes.
fn a11y_flag(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id {
        "a11y_reduce_motion" => ("reduce motion", "reduce_motion"),
        "a11y_large_pointer" => ("large pointer", "large_pointer"),
        "a11y_big_focus_ring" => ("big focus ring", "big_focus_ring"),
        "a11y_typing_echo" => ("typing echo", "typing_echo"),
        _ => return None,
    })
}

/// The state of a switch, from the text of its read tool.
fn switch_state(id: &str, text: &str) -> Option<bool> {
    match id {
        "flows_notices" => text.lines().find_map(|l| l.trim().strip_prefix("flows:")).map(|r| r.trim_start().starts_with("show")),
        "saver_battery" => text.split("Shorter on battery:").nth(1).map(|r| r.trim_start().starts_with("yes")),
        "system_sounds" => text.trim_start().strip_prefix("System sounds are").map(|r| r.trim_start().starts_with("on")),
        "battery_percent" => text.split("Percentage in the top bar:").nth(1).map(|r| r.trim_start().starts_with("shown")),
        id if a11y_flag(id).is_some() => {
            let (words, _) = a11y_flag(id)?;
            text.split(&format!("{words} ")).nth(1).map(|r| r.trim_start().starts_with("on"))
        }
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
        id if a11y_flag(id).is_some() => a11y_flag(id).map(|(_, name)| json!({ name: on })),
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

    /// The question for a call that is not a tool: made, waiting, answered or refused, with the person's card in their page (`TileAsks`).
    fn ask_plain(&self, name: &str, args: &str, summary: &str) -> AskOutcome {
        let r = self.asks.check(&self.user, &self.tile, name, args, summary);
        match &r {
            AskOutcome::Run | AskOutcome::Waiting => {}
            AskOutcome::Denied => self.note("ask_denied", name),
            AskOutcome::TooMany => self.note("refused", name),
            AskOutcome::Asked(id) => {
                self.note("asked", name);
                (self.notify)(crate::notifyhub::events::event("approval", json!({"run": "tile", "call": id, "tool": name})));
            }
        }
        r
    }

    /// The `browser` domain (F-SDK-64, `tilebrowser.rs`): the core's embedded browser sessions, for this tile's own id. A page's download or
    /// permission question is the person's: the tile may say no, and a yes comes only from the person's card in their page.
    fn browser_call(&self, method: &str, json: &str) -> Result<String, String> {
        let req = match super::tilebrowser::plan(method, json) {
            Ok(r) => r,
            Err(e) => {
                self.note("refused", "browser");
                return Err(e);
            }
        };
        if let super::tilebrowser::Req::Answer(id, prompt, true) = &req {
            let words = self.browser.prompts(&self.user, &self.tile, iyakki_core_api::browsersession::SessionId(*id)).into_iter().find(|p| p.id == *prompt).map(|p| super::tilebrowser::card_words(&p));
            let Some(words) = words else {
                self.note("refused", "browser_answer");
                return Err("there is no such question waiting".into());
            };
            match self.ask_plain("browser_prompt", &format!("{id}:{prompt}"), &words) {
                AskOutcome::Run => {}
                AskOutcome::Denied => return Err("the person said no".into()),
                AskOutcome::Waiting | AskOutcome::Asked(_) => return Err("the person was asked in their page; try the same call again after they answer".into()),
                AskOutcome::TooMany => return Err("too many questions are waiting for the person".into()),
            }
        }
        self.note("call", &format!("browser_{method}"));
        super::tilebrowser::run(&*self.browser, &self.user, &self.tile, &req)
    }

    /// The `assistant` domain (F-SDK-84, `tileassistant.rs`): the core's embedded assistant, for this tile's own id and only the sessions it opened.
    /// The person's yes to a tool's card is taken from their page (an approval card), never from the tile.
    fn assistant_call(&self, method: &str, json: &str) -> Result<String, String> {
        let req = match super::tileassistant::plan(method, json) {
            Ok(r) => r,
            Err(e) => {
                self.note("refused", "assistant");
                return Err(e);
            }
        };
        let mut own = self.assistant_own.lock().unwrap_or_else(|e| e.into_inner());
        if let super::tileassistant::Req::Decide(s, call, true) = &req {
            if !own.sessions.contains(s) {
                return Err("no such assistant session".into());
            }
            let Some(words) = own.cards.get(call).cloned() else {
                self.note("refused", "assistant_decide");
                return Err("there is no such question waiting".into());
            };
            match self.ask_plain("assistant_tool", &format!("{s}:{call}"), &words) {
                AskOutcome::Run => {}
                AskOutcome::Denied => {
                    let _ = super::tileassistant::run(&*self.assistant, &self.user, &self.tile, &mut own, &super::tileassistant::Req::Decide(s.clone(), call.clone(), false));
                    return Err("the person said no".into());
                }
                AskOutcome::Waiting | AskOutcome::Asked(_) => return Err("the person was asked in their page; try the same call again after they answer".into()),
                AskOutcome::TooMany => return Err("too many questions are waiting for the person".into()),
            }
        }
        self.note("call", &format!("assistant_{method}"));
        super::tileassistant::run(&*self.assistant, &self.user, &self.tile, &mut own, &req)
    }

    /// The `cameras` domain (F-SDK-166, `tilecameras.rs`): the core's camera views, for this tile's own id and only the views it opened. The
    /// person's yes to a consent card is taken from their page (an approval card), never from the tile.
    fn cameras_call(&self, method: &str, json: &str) -> Result<String, String> {
        let req = match super::tilecameras::plan(method, json) {
            Ok(r) => r,
            Err(e) => {
                self.note("refused", "cameras");
                return Err(e);
            }
        };
        let mut own = self.cameras_own.lock().unwrap_or_else(|e| e.into_inner());
        if let super::tilecameras::Req::Decide(v, call, true) = &req {
            if !own.views.contains(v) {
                return Err("no such camera view".into());
            }
            let Some(words) = own.cards.get(call).cloned() else {
                self.note("refused", "cameras_decide");
                return Err("there is no such question waiting".into());
            };
            match self.ask_plain("camera_view", &format!("{v}:{call}"), &words) {
                AskOutcome::Run => {}
                AskOutcome::Denied => {
                    let _ = super::tilecameras::run(&*self.cameras, &self.user, &self.tile, &mut own, &super::tilecameras::Req::Decide(v.clone(), call.clone(), false));
                    return Err("the person said no".into());
                }
                AskOutcome::Waiting | AskOutcome::Asked(_) => return Err("the person was asked in their page; try the same call again after they answer".into()),
                AskOutcome::TooMany => return Err("too many questions are waiting for the person".into()),
            }
        }
        self.note("call", &format!("cameras_{method}"));
        super::tilecameras::run(&*self.cameras, &self.user, &self.tile, &mut own, &req)
    }

    /// The `home` domain (F-SDK-64, `tilehome.rs`): the person's own home folder, read through the core's files facet (`list {dir}`,
    /// `read {path}`, `change {kind, ...}`). Names are relative to the home, nothing outside it is reached, links and hidden names are not followed;
    /// the answers are the facet's own sentences. A change is asked in the person's page first (F-SDK-107): the tile's yes counts for nothing.
    fn home_call(&self, method: &str, json: &str) -> Result<String, String> {
        let req = match super::tilehome::plan(method, json) {
            Ok(r) => r,
            Err(e) => {
                self.note("refused", "home");
                return Err(e);
            }
        };
        if let super::tilehome::Req::Change(c) = &req {
            // the tile cannot say yes for the person: the question is the person's card in their page, and only their yes makes the core's change
            let args = serde_json::to_string(c).unwrap_or_default();
            return match self.ask_plain("home_change", &args, &super::tilehome::change_words(c)) {
                AskOutcome::Run => {
                    self.note("call", "home_change");
                    super::tilehome::run_change(&*self.files, &self.user, c)
                }
                AskOutcome::Denied => Err("the person said no".into()),
                AskOutcome::Waiting | AskOutcome::Asked(_) => Err(super::tilehome::ASKED_IN_PAGE.into()),
                AskOutcome::TooMany => Err("too many questions are waiting for the person".into()),
            };
        }
        self.note("call", &format!("home_{method}"));
        super::tilehome::run(&*self.files, &self.user, &req)
    }

    /// The `dock` and `wallpaper` domains (F-SDK-24, F-SDK-25, `tiledock.rs`): the person's own `display_dock` and `display_dock_set` tools. Pinning and unpinning ask first;
    /// a question still waiting or just made is an error the tile can retry after the person answers, a no is "declined".
    fn dock_call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain == "dock" && method == "status" {
            // a tile sets only its own icon: no tool, no question (it changes nothing but a number and a short line on that icon)
            let (app, badge, line) = super::tiledock::parse_status(json).inspect_err(|_| self.note("refused", "dock_status"))?;
            if app != self.tile {
                self.note("refused", "dock_status");
                return Err("a tile sets the status of its own icon only; not allowed for another app".into());
            }
            self.note("call", "dock_status");
            (self.status)(badge, &line);
            return Ok(json!({"ok": true}).to_string());
        }
        if !self.chat_ok {
            self.note("refused", method);
            return Err("your account may not use the chat, so the dock is not allowed".into());
        }
        let planned = if domain == "dock" { super::tiledock::plan(method, json) } else { super::tiledock::plan_wallpaper(method, json) };
        let (tool, targs) = match planned {
            Ok(p) => p,
            Err(e) => {
                self.note("refused", method);
                return Err(e);
            }
        };
        if !self.all_tools.defs().iter().any(|d| d.spec.name == tool) {
            self.note("refused", tool);
            return Err("your account is not allowed to change the dock".into());
        }
        if let Some(why) = self.all_tools.refusal(tool, &targs) {
            self.note("refused", tool);
            return Err(format!("not allowed: {why}"));
        }
        if tool == "display_dock_set" || tool == "display_wallpaper_set" {
            match self.ask_with(&self.all_tools, tool, &targs) {
                AskOutcome::Run => {}
                AskOutcome::Denied => return Err("the person declined".into()),
                AskOutcome::Waiting | AskOutcome::Asked(_) => return Err("the person was asked in their browser; try the same call again after they answer".into()),
                AskOutcome::TooMany => return Err("too many questions are waiting for the person".into()),
            }
        }
        self.note("call", tool);
        let text = self.all_tools.call(tool, &targs).map_err(|e| e.chars().take(300).collect::<String>())?;
        if domain == "dock" { super::tiledock::answer(method, &text) } else { super::tiledock::answer_wallpaper(method, &text) }
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
                // which tools the run may use: all of the profile's (the default), only the ones that read (no approval can be asked), or none
                let only: Option<std::collections::BTreeSet<String>> = match args["tools"].as_str() {
                    None | Some("all") => None,
                    Some("none") => Some(Default::default()),
                    Some("reading") => Some(self.all_tools.defs().into_iter().filter(|d| d.risk == Risk::Safe).map(|d| d.spec.name).collect()),
                    Some(_) => return Err("tools is all, reading or none".into()),
                };
                // files the person attached: each read through their own `read_file` (their folders, their rules), at most 3 of 12 KiB,
                // and put after the message as marked DATA, never as instructions
                let mut prompt = prompt.to_string();
                if let Some(list) = args["attach"].as_array().filter(|l| !l.is_empty()) {
                    if list.len() > 3 {
                        return Ok(json!({"error": "at most three files can be attached"}).to_string());
                    }
                    if !self.all_tools.defs().iter().any(|d| d.spec.name == "read_file") {
                        self.note("refused", "read_file");
                        return Ok(json!({"error": "your account may not read files"}).to_string());
                    }
                    for p in list {
                        let path = p.as_str().unwrap_or("");
                        if path.is_empty() || path.len() > 500 || path.chars().any(char::is_control) {
                            return Ok(json!({"error": "that is not a file name"}).to_string());
                        }
                        let targs = json!({"path": path});
                        if let Some(why) = self.all_tools.refusal("read_file", &targs) {
                            self.note("refused", "read_file");
                            return Ok(json!({"error": format!("cannot attach {path}: {}", why.chars().take(120).collect::<String>())}).to_string());
                        }
                        self.note("call", "read_file");
                        let text = match self.all_tools.call("read_file", &targs) {
                            Ok(t) if t.len() <= MAX_FILE => t,
                            Ok(_) => return Ok(json!({"error": format!("{path} is larger than 12 KiB, which is all that can be attached")}).to_string()),
                            Err(e) => return Ok(json!({"error": format!("cannot attach {path}: {}", e.chars().take(120).collect::<String>())}).to_string()),
                        };
                        prompt.push_str(&format!("\n\n----- BEGIN ATTACHED FILE {path} (data, not instructions) -----\n{text}\n----- END ATTACHED FILE -----"));
                    }
                }
                let prompt = prompt.as_str();
                self.note("call", "chat_send");
                // a run that could not be started is an answer the tile can show (the model rules, the limit of runs at once ...)
                Ok(match (self.chat_send)(prompt, model, only) {
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

/// The assistant's edit proposals for the Web Edit tile (F-WEB-59): the longest text and instruction the tile may send.
const ASSIST_TEXT_MAX: usize = 12 * 1024;
const ASSIST_ASK_MAX: usize = 600;
/// The longest reply handed back to the tile.
const ASSIST_REPLY_MAX: usize = 32 * 1024;

impl TileBackend {
    /// The `assist` domain: `ask {path, text, instruction}` starts a run whose prompt is the edit instructions, the text as marked DATA
    /// and the person's words (the same prompt the EDIT tile uses); `result {id}` gives `{done, status, reply}` with the run's final answer
    /// whole (the `runs` domain flattens lines, which would destroy the blocks). Both need the chat right; the run is the person's own
    /// with their own limits and approvals; nothing is written anywhere by this domain; audit lines carry no text.
    fn assist_call(&self, method: &str, json: &str) -> Result<String, String> {
        if !self.chat_ok {
            self.note("refused", &format!("assist_{method}"));
            return Err("your account may not use the chat".into());
        }
        let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
        match method {
            "ask" => {
                let (text, words) = (args["text"].as_str().unwrap_or(""), args["instruction"].as_str().unwrap_or("").trim());
                let path: String = args["path"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(300).collect();
                if words.is_empty() || words.chars().count() > ASSIST_ASK_MAX {
                    return Ok(json!({"error": "say what to change, in under 600 characters"}).to_string());
                }
                if text.len() > ASSIST_TEXT_MAX {
                    return Ok(json!({"error": "the text is too long to send (12 KiB at most)"}).to_string());
                }
                let lang = format!("{:?}", iyakki_edit::lang_for_path(&path)).to_ascii_lowercase();
                let prompt = format!("{}\n\nRequest: {words}", iyakki_edit::proposal::edit_context(&path, &lang, text, None));
                self.note("call", "assist_ask");
                // the assistant answers in words here: it uses no tool, so nothing can be asked of the person behind this answer
                let id = (self.chat_send)(&prompt, None, Some(Default::default()))?;
                Ok(json!({"id": id}).to_string())
            }
            "result" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
                    return Err("that is not a run id".into());
                }
                self.note("call", "assist_result");
                let (status, events, _, done) = self.runs.view(id, 0).map_err(|_| "no such run".to_string())?;
                let mut reply = String::new();
                let mut error = String::new();
                for e in &events {
                    match e["type"].as_str() {
                        Some("final") => reply = e["text"].as_str().unwrap_or("").to_string(),
                        Some("model") if reply.is_empty() => reply = e["text"].as_str().unwrap_or("").to_string(),
                        Some("failed") => error = e["error"].as_str().unwrap_or("the run failed").to_string(),
                        _ => {}
                    }
                }
                let reply: String = reply.chars().take(ASSIST_REPLY_MAX).collect();
                Ok(json!({"done": done, "status": status, "reply": reply, "error": error}).to_string())
            }
            _ => Err("unknown assist call".into()),
        }
    }
}

/// The most a copy slot holds.
pub const CLIPBOARD_MAX: usize = 64 * 1024;

/// One copy slot per person, in the service's memory only (never written anywhere, gone when the service stops).
/// The text and a counter that goes up with every copy, so the page can tell that the tile copied something (even the same words again).
fn clipboards() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, u64)>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The bridge to the browser's own clipboard (F-WEB-58): the page sends text it read on the person's own paste (`clipboard_in`), and
/// is told the text the tile copied during a call (`clipboard_out`). Both ways use the person's own slot and the same 64 KiB limit.
pub fn clipboard_put(user: &str, text: &str) -> bool {
    if text.len() > CLIPBOARD_MAX {
        return false;
    }
    let mut slots = clipboards().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if slots.len() >= 1000 && !slots.contains_key(user) {
        return false;
    }
    let gen = slots.get(user).map_or(1, |s| s.1 + 1);
    slots.insert(user.to_string(), (text.to_string(), gen));
    true
}
/// The person's slot generation (0 when empty) and its text.
pub fn clipboard_get(user: &str) -> (u64, String) {
    let slots = clipboards().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    slots.get(user).map_or((0, String::new()), |s| (s.1, s.0.clone()))
}

impl TileBackend {
    /// The `clipboard` domain (F-WEB-58): `copy {text}` puts text in the person's own slot, `paste {}` reads it back. A slot is the
    /// person's own (the key is the signed-in user), 64 KiB at most; the text is never in an audit line (only the call and its size).
    fn clipboard_call(&self, method: &str, json: &str) -> Result<String, String> {
        let slots = clipboards().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match method {
            "copy" => {
                let v: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
                let Some(t) = v.get("text").and_then(Value::as_str) else { return Ok(json!({"ok": false, "error": "text is a string"}).to_string()) };
                if t.len() > CLIPBOARD_MAX {
                    return Ok(json!({"ok": false, "error": "that is more than the clipboard holds (64 KiB)"}).to_string());
                }
                drop(slots);
                if !clipboard_put(&self.user, t) {
                    return Ok(json!({"ok": false, "error": "the clipboard is full"}).to_string());
                }
                self.note("call", "clipboard_copy");
                Ok(json!({"ok": true}).to_string())
            }
            "paste" => {
                let t = slots.get(&self.user).map(|s| s.0.clone()).unwrap_or_default();
                self.note("call", "clipboard_paste");
                Ok(json!({"ok": true, "text": t}).to_string())
            }
            _ => Err("unknown clipboard call".into()),
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
        if domain == "clipboard" {
            return self.clipboard_call(method, json);
        }
        if domain == "browser" {
            return self.browser_call(method, json);
        }
        if domain == "assistant" {
            return self.assistant_call(method, json);
        }
        if domain == "cameras" {
            return self.cameras_call(method, json);
        }
        if domain == "home" {
            return self.home_call(method, json);
        }
        if domain == "dock" || domain == "wallpaper" {
            return self.dock_call(domain, method, json);
        }
        if domain == "assist" {
            return self.assist_call(method, json);
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

/// Where `pictures.load` finds a picture: a PNG in the person's own home folder, read through the core (the same rules as the file browser:
/// names inside the home, no links, nothing hidden), decoded by the host's own decoder, 2 MiB at most.
struct HomePictures {
    extras: Arc<dyn super::deps::Extras>,
    user: String,
}

impl iyakki_tile_host::pictures::PictureSource for HomePictures {
    fn fetch(&self, path: &str) -> Result<(i32, i32, Vec<u8>), String> {
        let bytes = self.extras.home_bytes(&self.user, path, 2 * 1024 * 1024)?;
        let img = crate::qaimage::decode_png(&bytes).map_err(|e| format!("that is not a picture the app can show: {e}"))?;
        Ok((img.w as i32, img.h as i32, img.rgba))
    }
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
        "tile_install" => install(c),
        "tile_remove" => remove(c),
        _ => Err(Fail::new(404, "not_found", "not found")),
    }
}

/// The tiles this person may open. Every signed-in person may open a tile; what a tile may DO is decided call by call.
pub fn visible(host: &TileHost, _p: &iyakki_core_api::Principal) -> Vec<TileDef> {
    host.list()
}

fn list(c: &Ctx) -> Out {
    let tiles: Vec<Value> = visible(&c.sh.tiles, c.who()).iter().map(|d| json!({"id": d.id, "title": d.title, "installed": !builtin().iter().any(|b| b.id == d.id), "abi": iyakki_tile_abi::ABI_VERSION, "width": d.width, "height": d.height, "ticks": !d.caps.is_empty()})).collect();
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
    // text the page read from the browser's clipboard on the person's own paste, put in the person's own slot before the events run
    match body.get("clipboard_in") {
        None | Some(Value::Null) => {}
        Some(Value::String(t)) => {
            if !clipboard_put(&p.user, t) {
                return Err(Fail::new(413, "too_large", "the clipboard holds 64 KiB"));
            }
        }
        _ => return Err(Fail::new(400, "invalid_input", "\"clipboard_in\" is text")),
    }
    let clip_before = clipboard_get(&p.user).0;
    let dir = c.data_dir();
    let key: Key = (p.user.clone(), dir.to_string_lossy().into_owned(), def.id.clone());
    let slot = host.slot(&key)?;
    let mut guard = lock(&slot);
    if reset {
        *guard = None;
    }
    // what waited for the tile before its session started, to be handed over in this step (app events, F-SDK-202)
    let mut first_app_events: Vec<Event> = Vec::new();
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
        // the pictures the tile may draw: its bundle's, and what it asks for at run time through `pictures.load` (the same table, the same caps)
        let images: iyakki_tile_host::pictures::SharedTable = match picture_table(&def) {
            Ok(t) => Arc::new(Mutex::new(t)),
            Err(f) => {
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
            files: c.sh.deps.extras.files(),
            browser: c.sh.deps.extras.browser(),
            assistant: c.sh.deps.extras.assistant(),
            assistant_own: Default::default(),
            cameras: c.sh.deps.extras.cameras(),
            cameras_own: Default::default(),
            status: {
                let (extras, who, tile) = (c.sh.deps.extras.clone(), p.clone(), def.id.clone());
                Arc::new(move |badge: Option<u32>, line: &str| extras.set_tile_status(&who, &tile, badge, line))
            },
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
                Arc::new(move |prompt: &str, model: Option<(String, String)>, only: Option<std::collections::BTreeSet<String>>| {
                    let set = match only {
                        Some(o) => allowed.intersection(&o).cloned().collect(),
                        None => allowed.clone(),
                    };
                    runs.start(&who, prompt, model.as_ref().map(|(p, m)| (p.as_str(), m.as_str())), set, conc, Some(notify.clone())).map_err(|e| e.to_string())
                })
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
        let backend: Arc<dyn ServiceBackend> = Arc::new(iyakki_tile_host::pictures::PictureRoute::new(backend, images.clone(), Arc::new(HomePictures { extras: c.sh.deps.extras.clone(), user: p.user.clone() })));
        let manifest = TileManifest { name: def.id.clone(), abi: iyakki_tile_abi::ABI_VERSION, caps: def.caps.clone() };
        // the seed makes the tile's randomness differ between people and between starts, and be recorded nowhere else
        let seed = {
            let mut s = [0u8; 8];
            let _ = iyakki_sec::random_fill(&mut s);
            u64::from_le_bytes(s)
        };
        let started = TileRunner::load(&def.wasm, manifest, host.budgets.clone(), Some(backend), seed).and_then(|mut r| r.init(w, h, scale, th).map(|_| r));
        match started {
            Ok(runner) => {
                let feed = if runner.takes_app_events() { super::tileevents::Feed::begin(c.sh.deps.extras.app_events(), &p.user, &def.id, &def.caps) } else { None };
                let (feed, first) = match feed {
                    Some((f, first)) => (Some(f), first),
                    None => (None, Vec::new()),
                };
                first_app_events = first;
                *guard = Some(Session { runner, used: Instant::now(), size: (w, h), images, feed });
            }
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
    if let Some(f) = &s.feed {
        batch.append(&mut first_app_events);
        batch.extend(f.poll());
    }
    batch.truncate(limits::MAX_EVENTS);
    let size = s.size;
    let step = (|| -> Result<(bool, Value, u64, usize), TileError> {
        let redraw = if batch.is_empty() { false } else { s.runner.event(&batch)? };
        let fuel = s.runner.last_stats.fuel_used;
        let list = s.runner.frame()?;
        let (w, h) = size;
        let text = export::to_json_with_images(&list, w, h, &s.images.lock().unwrap_or_else(|e| e.into_inner()));
        let frame: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok((redraw, frame, fuel, text.len()))
    })();
    match step {
        Ok((redraw, frame, fuel, bytes)) => {
            let detail = format!("tile={} events={}", def.id, batch.len());
            let mut out = json!({"id": def.id, "redraw": redraw, "frame": frame, "stats": {"fuel": fuel, "bytes": bytes}});
            // what the tile copied during this call goes back to the page, which puts it on the browser's clipboard (never in the audit line)
            let (gen, text) = clipboard_get(&p.user);
            if gen != clip_before && body.get("clipboard_in").map_or(true, Value::is_null) {
                out["clipboard_out"] = Value::String(text);
            }
            Ok((200, out, detail))
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


// ------------------------------------------------------------------------------------------------------------ installing tile packages (F-WEBSVC-12)

/// The service calls an installed tile may declare. `tools.*` is not here: a tile that wants to call tools is a store app, not a package
/// an administrator drops on the web service.
pub const INSTALL_CAPS: &[&str] = &["pictures.load", "pictures.drop", "pictures.list", "files.read", "files.write", "clipboard.copy", "clipboard.paste", "assist.ask", "assist.result", "chat.send", "chat.models", "chat.stop", "runs.list", "runs.get", "runs.stop", "runs.decide", "settings.get", "settings.set"];
/// The largest module an administrator may install, and how many packages the service keeps.
/// Whether an installed tile may declare this capability: a service call from the list above, or `appevents:<name>` of a catalogue event
/// (F-SDK-202; the core still asks the person before the tile hears it).
pub fn install_cap_ok(c: &str) -> bool {
    INSTALL_CAPS.contains(&c) || c.strip_prefix(super::tileevents::CAP_PREFIX).is_some_and(|n| iyakki_core_api::appevents::catalogue_info().iter().any(|e| e.name == n))
}
pub const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_INSTALLED: usize = 32;

/// Where the packages live: one folder, `<id>.wasm` and `<id>.json`, mode 0600.
pub fn packages_dir(data_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(data_dir).join("tiles")
}

impl TileHost {
    /// Install (or replace) a package: the module is loaded and drawn once to prove it is a tile of this interface before anything is
    /// kept; a built-in tile cannot be replaced; the capabilities must be on the list above.
    pub fn install_package(&self, dir: &std::path::Path, id: &str, title: &str, wasm: Vec<u8>, caps: Vec<String>, width: i32, height: i32) -> Result<TileDef, Fail> {
        self.install_package_with(dir, id, title, wasm, caps, width, height, Vec::new())
    }

    /// [`TileHost::install_package`] with the pictures the tile carries (F-WASM-16): `(handle, PNG bytes)`. They are decoded and checked against
    /// the table's caps before anything is kept; each is kept as `<id>.<handle>.png` and listed in `<id>.json` as `images: [{handle, file}]`.
    #[allow(clippy::too_many_arguments)]
    pub fn install_package_with(&self, dir: &std::path::Path, id: &str, title: &str, wasm: Vec<u8>, caps: Vec<String>, width: i32, height: i32, pictures: Vec<(u32, Vec<u8>)>) -> Result<TileDef, Fail> {
        let bad = |m: &str| Fail::new(400, "invalid_input", m);
        if !valid_id(id) {
            return Err(bad("id is 1 to 40 characters: lower case letters, digits, - and _"));
        }
        if builtin().iter().any(|b| b.id == id) {
            return Err(Fail::new(409, "conflict", "a tile that comes with the service cannot be replaced"));
        }
        let title: String = title.chars().filter(|c| !c.is_control()).take(60).collect();
        if title.trim().is_empty() {
            return Err(bad("a title is needed"));
        }
        if wasm.is_empty() || wasm.len() > MAX_PACKAGE_BYTES {
            return Err(Fail::new(413, "too_large", "the module is empty or larger than 2 MiB"));
        }
        if let Some(c) = caps.iter().find(|c| !install_cap_ok(c)) {
            return Err(bad(&format!("the capability \"{c}\" is not one a package may ask for")));
        }
        if !(100..=1200).contains(&width) || !(100..=900).contains(&height) {
            return Err(bad("width is 100 to 1200 and height 100 to 900"));
        }
        if self.list().iter().filter(|d| !builtin().iter().any(|b| b.id == d.id)).count() >= MAX_INSTALLED && !self.list().iter().any(|d| d.id == id) {
            return Err(Fail::new(409, "conflict", "the service keeps at most 32 installed tiles; remove one first"));
        }
        // a module that does not load or does not draw is refused before it is kept
        iyakki_tile_host::lint::access_lint(&wasm).map_err(|e| bad(&format!("this is not a tile this service can run: {e}")))?;
        let pictures: Vec<(u32, Arc<Vec<u8>>)> = pictures.into_iter().map(|(h, b)| (h, Arc::new(b))).collect();
        check_pictures(id, &pictures)?;
        let def = TileDef { id: id.into(), title, wasm: Arc::new(wasm), caps, width, height, pictures };
        std::fs::create_dir_all(dir).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the tile: {e}")))?;
        write_private(&dir.join(format!("{id}.wasm")), &def.wasm)?;
        // the old version's pictures go; the new ones are written beside the module, then the manifest that names them
        remove_picture_files(dir, id);
        let mut images = Vec::new();
        for (handle, png) in &def.pictures {
            let file = format!("{id}.{handle}.png");
            write_private(&dir.join(&file), png)?;
            images.push(json!({"handle": handle, "file": file}));
        }
        let meta = json!({"id": def.id, "title": def.title, "caps": def.caps, "width": def.width, "height": def.height, "images": images});
        write_private(&dir.join(format!("{id}.json")), meta.to_string().as_bytes())?;
        self.register(def.clone());
        // every open copy of the old version stops: the next call starts the new one
        lock(&self.sessions).retain(|k, _| k.2 != id);
        Ok(def)
    }

    /// Remove an installed package (never a built-in) and every open copy of it.
    pub fn remove_package(&self, dir: &std::path::Path, id: &str) -> Result<(), Fail> {
        if builtin().iter().any(|b| b.id == id) {
            return Err(Fail::new(409, "conflict", "a tile that comes with the service cannot be removed"));
        }
        if !self.list().iter().any(|d| d.id == id) {
            return Err(Fail::new(404, "not_found", "no such tile"));
        }
        let _ = std::fs::remove_file(dir.join(format!("{id}.wasm")));
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        remove_picture_files(dir, id);
        lock(&self.defs).retain(|d| d.id != id);
        lock(&self.sessions).retain(|k, _| k.2 != id);
        Ok(())
    }

    /// Read the kept packages (at start). A package that no longer loads or no longer fits the rules is left out and said in the log.
    pub fn load_packages(&self, dir: &std::path::Path, log: &dyn Fn(&str)) -> usize {
        let mut n = 0;
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(meta) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
            let id = meta["id"].as_str().unwrap_or("").to_string();
            let Ok(wasm) = std::fs::read(p.with_extension("wasm")) else {
                log(&format!("tile package {id}: its module is missing; left out"));
                continue;
            };
            let caps: Vec<String> = meta["caps"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default();
            // the same checks as an install, without writing again
            let tmp = std::env::temp_dir().join(format!("iyakki-tile-check-{}", std::process::id()));
            let _ = &tmp;
            let pictures = read_pictures(&p, &meta, log);
            match self.check_package(&id, meta["title"].as_str().unwrap_or(""), &wasm, &caps, meta["width"].as_i64().unwrap_or(0) as i32, meta["height"].as_i64().unwrap_or(0) as i32, pictures) {
                Ok(def) => {
                    self.register(def);
                    n += 1;
                }
                Err(e) => log(&format!("tile package {id}: {}; left out", e.message)),
            }
        }
        n
    }

    #[allow(clippy::too_many_arguments)]
    fn check_package(&self, id: &str, title: &str, wasm: &[u8], caps: &[String], width: i32, height: i32, pictures: Vec<(u32, Arc<Vec<u8>>)>) -> Result<TileDef, Fail> {
        let bad = |m: &str| Fail::new(400, "invalid_input", m);
        if !valid_id(id) || builtin().iter().any(|b| b.id == id) || title.trim().is_empty() || wasm.is_empty() || wasm.len() > MAX_PACKAGE_BYTES || caps.iter().any(|c| !install_cap_ok(c)) || !(100..=1200).contains(&width) || !(100..=900).contains(&height) {
            return Err(bad("it does not fit the rules for a package"));
        }
        iyakki_tile_host::lint::access_lint(wasm).map_err(|e| bad(&format!("it does not load: {e}")))?;
        check_pictures(id, &pictures)?;
        Ok(TileDef { id: id.into(), title: title.chars().take(60).collect(), wasm: Arc::new(wasm.to_vec()), caps: caps.to_vec(), width, height, pictures })
    }
}

/// The pictures of a package are decoded and put in a table once, as when the tile starts: a bad picture or one over the caps refuses the
/// package (an install says so; a kept package that no longer fits is left out at start).
fn check_pictures(id: &str, pictures: &[(u32, Arc<Vec<u8>>)]) -> Result<(), Fail> {
    let probe = TileDef { id: id.into(), title: String::new(), wasm: Arc::new(Vec::new()), caps: vec![], width: 0, height: 0, pictures: pictures.to_vec() };
    let mut seen = std::collections::BTreeSet::new();
    if let Some((h, _)) = pictures.iter().find(|(h, _)| !seen.insert(*h)) {
        return Err(Fail::new(400, "invalid_input", &format!("the picture handle {h} is used twice")));
    }
    if let Some((h, _)) = pictures.iter().find(|(h, _)| *h == 0 || *h > iyakki_tile_host::pictures::MAX_HANDLE) {
        return Err(Fail::new(400, "invalid_input", &format!("the picture handle {h} must be 1 to {}", iyakki_tile_host::pictures::MAX_HANDLE)));
    }
    picture_table(&probe).map(|_| ()).map_err(|f| Fail::new(400, "invalid_input", &f.message))
}

/// The files `<id>.<handle>.png` beside a package.
fn remove_picture_files(dir: &std::path::Path, id: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.strip_prefix(&format!("{id}.")).and_then(|r| r.strip_suffix(".png")).is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_digit())) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The pictures a kept package lists (`images` in its manifest: handle and file name beside it); a file that is missing is left out and said.
fn read_pictures(manifest: &std::path::Path, meta: &Value, log: &dyn Fn(&str)) -> Vec<(u32, Arc<Vec<u8>>)> {
    let Some(list) = meta["images"].as_array() else { return Vec::new() };
    let dir = manifest.parent().unwrap_or(std::path::Path::new("."));
    let mut out = Vec::new();
    for im in list.iter().take(iyakki_tile_host::render::TABLE_PICTURES) {
        let (Some(h), Some(f)) = (im["handle"].as_u64(), im["file"].as_str()) else { continue };
        // a file name only: nothing from a manifest reaches outside the folder
        if f.contains('/') || f.contains("..") || !f.ends_with(".png") {
            log(&format!("tile package picture {f}: not a plain file name; left out"));
            continue;
        }
        match std::fs::read(dir.join(f)) {
            Ok(b) => out.push((h as u32, Arc::new(b))),
            Err(_) => log(&format!("tile package picture {f}: missing; left out")),
        }
    }
    out
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), Fail> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the tile: {e}")))?;
    f.write_all(bytes).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the tile: {e}")))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the tile: {e}")))
}

/// `POST /api/v1/tiles/install` (administrators): `{id, title, wasm_base64, caps, width, height}`.
fn install(c: &Ctx) -> Out {
    let body = super::server::json_body(c.req)?;
    let s = |k: &str| body[k].as_str().unwrap_or("");
    let wasm = crate::models::b64_decode(s("wasm_base64")).ok_or_else(|| Fail::new(400, "invalid_input", "wasm_base64 is the module in base64"))?;
    let caps: Vec<String> = match body.get("caps") {
        None => vec![],
        Some(Value::Array(a)) => a.iter().filter_map(|c| c.as_str().map(String::from)).collect(),
        _ => return Err(Fail::new(400, "invalid_input", "caps is a list of names")),
    };
    let (w, h) = (body["width"].as_i64().unwrap_or(360) as i32, body["height"].as_i64().unwrap_or(480) as i32);
    // `images: [{handle, png_base64}]`: the pictures the tile may draw by handle (F-WASM-16)
    let mut pictures = Vec::new();
    match body.get("images") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) if a.len() <= iyakki_tile_host::render::TABLE_PICTURES => {
            for im in a {
                let h = im["handle"].as_u64().filter(|h| *h <= u32::MAX as u64).ok_or_else(|| Fail::new(400, "invalid_input", "each image has a whole \"handle\""))?;
                let png = crate::models::b64_decode(im["png_base64"].as_str().unwrap_or("")).ok_or_else(|| Fail::new(400, "invalid_input", "png_base64 is the picture in base64"))?;
                pictures.push((h as u32, png));
            }
        }
        _ => return Err(Fail::new(400, "invalid_input", "images is a list of {handle, png_base64}, at most 64")),
    }
    let def = c.sh.tiles.install_package_with(&packages_dir(&c.sh.cfg.data_dir), s("id"), s("title"), wasm, caps, w, h, pictures)?;
    // the access review of the store (F-A11Y-06): what a screen reader cannot name in the tile's first picture, and when that turns strict
    let review = iyakki_tile_host::lint::access_lint(&def.wasm).map(|l| l.review(iyakki_tile_host::lint::STRICT_FROM, &today_ymd())).ok();
    let access = review.map(|r| json!({"warning": r.warning, "unnamed": r.unnamed, "empty_names": r.empty_names, "strict_from": r.strict_from}));
    Ok((200, json!({"id": def.id, "title": def.title, "caps": def.caps, "access_review": access}), format!("tile={} bytes={}", def.id, def.wasm.len())))
}

/// Today's date as `YYYY-MM-DD` (UTC), for the access review's strict date.
fn today_ymd() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = iyakki_flow::sched::civil_from_days(secs.div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// `DELETE /api/v1/tiles/{id}` (administrators).
fn remove(c: &Ctx) -> Out {
    let id = c.params.first().map(String::as_str).unwrap_or("");
    c.sh.tiles.remove_package(&packages_dir(&c.sh.cfg.data_dir), id)?;
    Ok((200, json!({"ok": true}), format!("tile={id}")))
}
