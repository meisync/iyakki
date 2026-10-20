//! FLOWS, the agent's side: the real host of the workflow engine (`iyakki-flow`), the per-account environment (store, engine,
//! host), ways of making and editing flows from structured descriptions (what the assistant and the import use), the compact
//! redacted picture of flows and runs that the model gets, and the assistant's tools. See `docs/FLOWS.md`.

pub mod browser;
pub mod host;
pub mod programs;
pub mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_flow::kinds::Registry;
use iyakki_flow::layout;
use iyakki_flow::store::{FlowStore, FlowSummary};
use iyakki_flow::*;
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::services::Services;
use host::AgentHost;

pub const AGENT: &str = "agent:assistant";

/// The key the FLOWS tile's chats are kept under (apart from the ordinary chat's).
pub fn chat_key(user: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("flows\u{1}{user}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("flows_{h:016x}")
}

/// What the tile tells the assistant is open on the screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Open {
    pub flow: Option<String>,
    pub node: Option<String>,
    pub run: Option<String>,
    pub tab: String,
}

pub type SharedOpen = Arc<Mutex<Open>>;

pub struct FlowEnv {
    pub user: String,
    pub policy: Policy,
    pub svc: Arc<Services>,
    pub store: Arc<FlowStore>,
    pub engine: Engine,
    pub host: Arc<AgentHost>,
    pub reg: Arc<Registry>,
    /// `<home>/Flows`.
    pub dir: PathBuf,
    /// `<data>/flows/<user>`.
    pub data: PathBuf,
}

fn now_ms() -> i64 {
    crate::clock::now_unix() * 1000
}

static HUB: OnceLock<Mutex<HashMap<String, Arc<FlowEnv>>>> = OnceLock::new();

/// Whether the account may use FLOWS at all.
pub fn allowed(p: &Policy) -> bool {
    p.apps.contains(&App::Flows)
}

impl FlowEnv {
    /// One environment per account for the life of the process: its engine ticks in the background, so schedules run while the
    /// person is signed in whether or not the tile is open.
    pub fn open(svc: &Arc<Services>, user: &str, policy: &Policy) -> Result<Arc<FlowEnv>, String> {
        if !allowed(policy) {
            return Err("your account may not use FLOWS".into());
        }
        let key = format!("{}|{user}", svc.dir().display());
        let hub = HUB.get_or_init(|| Mutex::new(HashMap::new()));
        let mut g = hub.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.get(&key) {
            return Ok(e.clone());
        }
        let env = Arc::new(FlowEnv::build(svc, user, policy, true));
        env.engine.recover();
        g.insert(key, env.clone());
        Ok(env)
    }

    /// Start the account's scheduler at sign-in, when it has any flows (an account without flows costs nothing).
    pub fn start_for_login(svc: &Arc<Services>, user: &str, policy: &Policy) -> bool {
        if !allowed(policy) {
            return false;
        }
        let dir = crate::scope::home_root(user, svc.dir()).join("Flows");
        let any = std::fs::read_dir(&dir).map(|rd| rd.flatten().any(|e| e.path().join("flow.json").is_file())).unwrap_or(false);
        if !any {
            return false;
        }
        FlowEnv::open(svc, user, policy).is_ok()
    }

    /// Stop the account's engine at sign-out. Runs in progress stay on disk and are picked up at the next sign-in.
    pub fn stop_for_logout(svc: &Arc<Services>, user: &str) {
        let key = format!("{}|{user}", svc.dir().display());
        let env = HUB.get().and_then(|h| h.lock().unwrap_or_else(|e| e.into_inner()).remove(&key));
        if let Some(e) = env {
            std::thread::spawn(move || e.engine.halt());
        }
    }

    /// Is the account's scheduler going?
    pub fn scheduler_running(svc: &Arc<Services>, user: &str) -> bool {
        let key = format!("{}|{user}", svc.dir().display());
        HUB.get().is_some_and(|h| h.lock().unwrap_or_else(|e| e.into_inner()).get(&key).is_some_and(|e| e.engine.ticking()))
    }

    /// An administrator allows one more program for flow commands (the screen asks first; the call is audited).
    pub fn allow_program(&self, name: &str) -> Result<(), String> {
        if !self.policy.manage_settings {
            return Err("only an administrator may allow another program".into());
        }
        self.host.programs().add(name)?;
        iyakki_flow::host::Host::audit(self.host.as_ref(), "flow.program.allow", &format!("{name} by {}", self.who()));
        Ok(())
    }

    /// Make an environment (tests build their own, with the engine not ticking by itself).
    pub fn build(svc: &Arc<Services>, user: &str, policy: &Policy, auto_tick: bool) -> FlowEnv {
        FlowEnv::build_in(svc, user, policy, auto_tick, &crate::scope::home_root(user, svc.dir()))
    }

    /// Like `build`, with the account's home folder given (the flows live in `<home>/Flows`).
    pub fn build_in(svc: &Arc<Services>, user: &str, policy: &Policy, auto_tick: bool, home: &std::path::Path) -> FlowEnv {
        let dir = home.join("Flows");
        let data = svc.dir().join("flows").join(if iyakki_sec::policy::valid_username(user) { user } else { "user" });
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::create_dir_all(&data);
        let host = Arc::new(AgentHost::new(svc, user, policy, &data, &dir));
        let reg = Arc::new(Registry::builtin());
        let store = Arc::new(FlowStore::new(&dir, Arc::new(now_ms)).with_registry((*reg).clone()));
        let mut cfg = EngineCfg::new(&data);
        cfg.auto_tick = auto_tick;
        cfg.user = user.to_string();
        // sandbox Jobs: the dev cluster the machine is connected to, never one this code starts
        {
            let (svc2, user2, runner, root) = (svc.clone(), user.to_string(), host.runner.clone(), data.join("jobs"));
                        *host.k8s.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(move |ns: &str| {
                let ns = ns.to_string();
                let remembered = crate::devcluster::Choice::load(svc2.dir(), &user2);
                let res = crate::devcluster::resolve(&runner, &crate::devcluster::Env::real(), &crate::workspace::K8sConfig::default(), remembered.as_ref(), &ns, false);
                let p = res.reachable().ok_or_else(|| format!("no cluster is connected: {}", res.explain()))?.clone();
                let _ = std::fs::create_dir_all(&root);
                Ok(Arc::new(crate::k8stools::K8sTools::new(runner.clone(), p, &ns, "flows", &root, Arc::new(|_, _| {}))))
            }));
        }
        *host.default_image.lock().unwrap_or_else(|e| e.into_inner()) = std::env::var("IYAKKI_FLOW_IMAGE").unwrap_or_default();
        let engine = Engine::new(cfg, host.clone(), reg.clone(), Arc::new(SystemClock), store.clone());
        FlowEnv { user: user.into(), policy: policy.clone(), svc: svc.clone(), store, engine, host, reg, dir, data }
    }

    pub fn who(&self) -> String {
        format!("user:{}", self.user)
    }

    pub fn summaries(&self) -> Vec<FlowSummary> {
        self.store.summaries()
    }

    /// Validation of a flow, as lines `node: message` (errors first).
    pub fn issues(&self, f: &Flow) -> Vec<kinds::Issue> {
        let f = self.store.resolve_for_check(f);
        let mut v = validate::validate_flow(&f, &self.reg);
        v.sort_by_key(|i| i.level != kinds::Level::Error);
        v
    }

    pub fn issue_lines(&self, f: &Flow) -> Vec<String> {
        self.issues(f).into_iter().map(|i| format!("{}{}: {}", if i.level == kinds::Level::Error { "ERROR " } else { "warning " }, i.node.unwrap_or_else(|| "flow".into()), i.msg)).collect()
    }

    /// A draft saved from an edit; `by` is `user:<name>` or `agent:<name>`.
    pub fn save_edit(&self, flow: &Flow, by: &str, summary: &str, node: &str) -> Result<(), String> {
        let change = Change { who: by.to_string(), node: node.to_string(), summary: summary.to_string(), at_ms: now_ms(), simple: false };
        // a person's edits are not badges; an agent's are
        self.store.save_draft(flow, if by.starts_with("agent:") { Some(change) } else { None })
    }

    /// A new draft flow with a free id derived from `name`.
    pub fn create_draft(&self, mut f: Flow, by: &str) -> Result<Flow, String> {
        let base = if valid_id(&f.id) && !f.id.is_empty() { f.id.clone() } else { slug(&f.name) };
        let mut id = base.clone();
        let mut n = 2;
        while self.store.exists(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        f.id = id;
        if layout::unplaced(&f) {
            layout::auto_layout(&mut f);
        }
        f.meta = Meta::default();
        if by.starts_with("agent:") {
            f.meta.modified_by.push(Change { who: by.to_string(), node: String::new(), summary: "created this draft".into(), at_ms: now_ms(), simple: false });
        }
        self.store.create(f, by)
    }

    pub fn import_json(&self, text: &str, by: &str) -> Result<Flow, String> {
        let mut f = Flow::from_json(text)?;
        iyakki_flow::templates::sanitize_import(&mut f);
        if f.name.trim().is_empty() {
            f.name = f.id.clone();
        }
        if !valid_id(&f.id) {
            f.id = slug(&f.name);
        }
        self.create_draft(f, by)
    }

    pub fn export_json(&self, id: &str) -> Result<String, String> {
        let f = self.store.get_draft(id).ok_or_else(|| format!("there is no flow `{id}`"))?;
        Ok(f.to_json())
    }

    /// How many things need the person (blocked runs, with cards and approvals counted in).
    pub fn needs_you(&self) -> usize {
        self.engine.counts().needs_you
    }

    /// The hook for Store items (an `extension` with target `flows` and kind `template`): the item's JSON payload becomes a safe draft.
    pub fn from_store_template(&self, payload: &str, by: &str) -> Result<Flow, String> {
        let t = templates::from_json(payload)?;
        let f = templates::instantiate(&t, &t.flow.id, &t.name);
        self.create_draft(f, by)
    }

    pub fn from_template(&self, template: &str, name: &str, by: &str) -> Result<Flow, String> {
        let t = templates::get(template).ok_or_else(|| format!("there is no template `{template}` (try: {})", templates::all().iter().map(|t| t.id.clone()).collect::<Vec<_>>().join(", ")))?;
        let id = if name.is_empty() { t.flow.id.clone() } else { slug(name) };
        let f = templates::instantiate(&t, &id, name);
        self.create_draft(f, by)
    }
}

// ------------------------------------------------------------------------------------------------ building and editing from data

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn parse_opt<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Result<Option<T>, String> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => serde_json::from_value(x.clone()).map(Some).map_err(|e| format!("`{k}`: {e}")),
    }
}

pub fn trigger_from_value(v: &Value) -> Result<Trigger, String> {
    let kind = s(v, "kind");
    let id = if s(v, "id").is_empty() { kind.replace('_', "-") } else { s(v, "id").to_string() };
    let k = match kind {
        "manual" => TriggerKind::Manual,
        "chat" => TriggerKind::Chat,
        "cron" => TriggerKind::Cron { expr: s(v, "expr").to_string(), tz: if s(v, "tz").is_empty() { "local".into() } else { s(v, "tz").to_string() } },
        "interval" => TriggerKind::Interval { every_s: v.get("every_s").and_then(Value::as_u64).or_else(|| v.get("every_minutes").and_then(Value::as_u64).map(|m| m * 60)).ok_or("an interval trigger needs `every_s` (seconds)")?, anchor_ms: None },
        "once" => TriggerKind::Once { at_ms: v.get("at_ms").and_then(Value::as_i64).ok_or("a once trigger needs `at_ms`")? },
        "calendar" => TriggerKind::Calendar { matching: s(v, "matching").to_string() },
        "file_change" => TriggerKind::FileChange { path: s(v, "path").to_string() },
        "on_failure" => TriggerKind::OnFailure { flow: s(v, "flow").to_string() },
        "event" => TriggerKind::Event { name: s(v, "name").to_string() },
        "webhook" => TriggerKind::Webhook { path: s(v, "path").to_string() },
        other => return Err(format!("`{other}` is not a trigger kind (manual, chat, cron, interval, once, calendar, file_change, on_failure, event, webhook)")),
    };
    let mut t = Trigger::new(&id, k);
    if let Some(e) = v.get("enabled").and_then(Value::as_bool) {
        t.enabled = e;
    }
    if let Some(m) = parse_opt::<Missed>(v, "missed")? {
        t.missed = m;
    }
    if let Some(o) = parse_opt::<Overlap>(v, "overlap")? {
        t.overlap = o;
    }
    t.jitter_s = v.get("jitter_s").and_then(Value::as_u64).unwrap_or(0);
    if let Some(i) = v.get("inputs") {
        t.inputs = i.clone();
    }
    Ok(t)
}

pub fn node_from_value(v: &Value) -> Result<Node, String> {
    let id = s(v, "id");
    let kind = s(v, "kind");
    if id.is_empty() || kind.is_empty() {
        return Err("a node needs `id` and `kind`".into());
    }
    let mut n = Node::new(id, kind, v.get("config").cloned().unwrap_or_else(|| json!({})));
    n.name = s(v, "name").to_string();
    n.retry = parse_opt(v, "retry")?;
    n.timeout_ms = v.get("timeout_ms").and_then(Value::as_u64);
    n.runner = parse_opt(v, "runner")?;
    n.heal = parse_opt(v, "heal")?;
    if let Some(t) = parse_opt::<Track>(v, "track")? {
        n.track = t;
    }
    if let Some(o) = parse_opt::<OnError>(v, "on_error")? {
        n.on_error = o;
    }
    Ok(n)
}

fn edge_from_value(v: &Value) -> Result<Edge, String> {
    if s(v, "from").is_empty() || s(v, "to").is_empty() {
        return Err("an edge needs `from` and `to`".into());
    }
    Ok(Edge { from: s(v, "from").into(), port: if s(v, "port").is_empty() { "out".into() } else { s(v, "port").into() }, to: s(v, "to").into(), when: s(v, "when").into() })
}

/// A draft from a structured description: `{name, description, inputs[], nodes[], edges[] or chain, triggers[], defaults{}}`. A `start`
/// node is added when there is none (feeding the nodes nothing else feeds); with `chain: true` or no edges at all the nodes are
/// connected in the order given; positions are laid out.
pub fn build_flow(spec: &Value) -> Result<Flow, String> {
    let name = s(spec, "name");
    if name.trim().is_empty() {
        return Err("give the flow a name".into());
    }
    let mut f = Flow::new(&slug(name), name);
    f.description = s(spec, "description").into();
    for n in spec.get("nodes").and_then(Value::as_array).into_iter().flatten() {
        f.nodes.push(node_from_value(n)?);
    }
    if f.nodes.is_empty() {
        return Err("give at least one node".into());
    }
    for e in spec.get("edges").and_then(Value::as_array).into_iter().flatten() {
        f.edges.push(edge_from_value(e)?);
    }
    let chain = spec.get("chain").and_then(Value::as_bool).unwrap_or(f.edges.is_empty());
    if !f.nodes.iter().any(|n| n.kind == "start") {
        f.nodes.insert(0, Node::new(&unique_id(&f, "start"), "start", json!({})));
    }
    if chain && f.edges.is_empty() {
        let ids: Vec<String> = f.nodes.iter().map(|n| n.id.clone()).collect();
        for w in ids.windows(2) {
            f.edges.push(Edge::new(&w[0], &w[1]));
        }
    } else {
        let start = f.nodes.iter().find(|n| n.kind == "start").map(|n| n.id.clone()).unwrap_or_default();
        let roots: Vec<String> = f.nodes.iter().filter(|n| n.kind != "start" && f.incoming(&n.id).is_empty()).map(|n| n.id.clone()).collect();
        for r in roots {
            f.edges.push(Edge::new(&start, &r));
        }
    }
    for t in spec.get("triggers").and_then(Value::as_array).into_iter().flatten() {
        f.triggers.push(trigger_from_value(t)?);
    }
    for p in spec.get("inputs").and_then(Value::as_array).into_iter().flatten() {
        f.inputs.push(Param { name: s(p, "name").into(), ty: s(p, "type").into(), required: p.get("required").and_then(Value::as_bool).unwrap_or(false), default: p.get("default").cloned(), description: s(p, "description").into() });
    }
    if let Some(d) = spec.get("defaults") {
        apply_defaults(&mut f, d)?;
    }
    layout::auto_layout(&mut f);
    Ok(f)
}

fn unique_id(f: &Flow, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    while f.node(&id).is_some() {
        id = format!("{base}{n}");
        n += 1;
    }
    id
}

fn apply_defaults(f: &mut Flow, d: &Value) -> Result<(), String> {
    if let Some(t) = d.get("timeout_ms").and_then(Value::as_u64) {
        f.defaults.timeout_ms = t;
    }
    if let Some(r) = parse_opt::<RetryPolicy>(d, "retry")? {
        f.defaults.retry = r;
    }
    if let Some(r) = parse_opt::<RunnerKind>(d, "runner")? {
        f.defaults.runner = r;
    }
    if let Some(h) = parse_opt::<HealPolicy>(d, "heal")? {
        f.defaults.heal = h;
    }
    if let Some(n) = d.get("max_parallel").and_then(Value::as_u64) {
        f.defaults.max_parallel = n as usize;
    }
    if let Some(n) = d.get("max_concurrent_runs").and_then(Value::as_u64) {
        f.defaults.max_concurrent_runs = n as usize;
    }
    if let Some(b) = d.get("browser") {
        // the assistant may set budgets; marking a flow trusted, or using the person's own browser profile, is the person's call
        // (FLOWS, flow settings): it would take away the question that protects them
        if b.get("trusted").and_then(Value::as_bool) == Some(true) || b.get("profile").and_then(Value::as_str).is_some_and(|p| p != "isolated" && !p.is_empty()) {
            return Err("only the person can mark a flow trusted or let it use their own browser profile (FLOWS, flow settings); the assistant can set `max_actions` and `max_minutes`".into());
        }
        if let Some(n) = b.get("max_actions").and_then(Value::as_u64) {
            f.defaults.browser.max_actions = n.min(1000) as u32;
        }
        if let Some(n) = b.get("max_minutes").and_then(Value::as_u64) {
            f.defaults.browser.max_minutes = n.min(180) as u32;
        }
    }
    Ok(())
}

/// Where a node added after `after` goes: one column to its right, in the first free row; or past the right edge of the flow.
fn place_new(f: &Flow, after: Option<&str>) -> Pos {
    let x = match after.and_then(|a| f.node(a)) {
        Some(a) => a.pos.x + layout::COL_W,
        None => f.nodes.iter().map(|n| n.pos.x).fold(layout::MARGIN - layout::COL_W, f32::max) + layout::COL_W,
    };
    let mut row = 0;
    while f.nodes.iter().any(|n| (n.pos.x - x).abs() < 1.0 && (n.pos.y - (layout::MARGIN + row as f32 * layout::ROW_H)).abs() < 1.0) {
        row += 1;
    }
    Pos { x, y: layout::MARGIN + row as f32 * layout::ROW_H }
}

/// Apply structured edit operations to a flow (a copy: nothing changes unless all of them work). Returns what was done, one line each.
pub fn apply_ops(f: &mut Flow, ops: &[Value]) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let name = s(op, "op");
        let step = |e: String| format!("operation {} ({name}): {e}", i + 1);
        match name {
            "add_node" => {
                let n = node_from_value(op).map_err(step)?;
                if f.node(&n.id).is_some() {
                    return Err(step(format!("there is already a node `{}`", n.id)));
                }
                let id = n.id.clone();
                let mut n = n;
                n.pos = place_new(f, op.get("after").and_then(Value::as_str));
                f.nodes.push(n);
                if let Some(after) = op.get("after").and_then(Value::as_str) {
                    f.edges.push(Edge { from: after.into(), port: if s(op, "port").is_empty() { "out".into() } else { s(op, "port").into() }, to: id.clone(), when: s(op, "when").into() });
                }
                if let Some(before) = op.get("before").and_then(Value::as_str) {
                    f.edges.push(Edge::new(&id, before));
                }
                done.push(format!("added node {id}"));
            }
            "update_node" => {
                let id = s(op, "id");
                let n = f.node_mut(id).ok_or_else(|| step(format!("there is no node `{id}`")))?;
                if let Some(nm) = op.get("name").and_then(Value::as_str) {
                    n.name = nm.into();
                }
                if let Some(c) = op.get("config") {
                    if let (Value::Object(cur), Value::Object(new)) = (&mut n.config, c) {
                        for (k, v) in new {
                            if v.is_null() {
                                cur.remove(k);
                            } else {
                                cur.insert(k.clone(), v.clone());
                            }
                        }
                    } else {
                        n.config = c.clone();
                    }
                }
                if let Some(r) = op.get("retry") {
                    n.retry = if r.is_null() { None } else { Some(serde_json::from_value(r.clone()).map_err(|e| step(format!("retry: {e}")))?) };
                }
                if let Some(t) = op.get("timeout_ms") {
                    n.timeout_ms = t.as_u64();
                }
                if let Some(r) = parse_opt::<RunnerKind>(op, "runner").map_err(step)? {
                    n.runner = Some(r);
                }
                if let Some(h) = op.get("heal") {
                    n.heal = if h.is_null() { None } else { Some(serde_json::from_value(h.clone()).map_err(|e| step(format!("heal: {e}")))?) };
                }
                if let Some(t) = parse_opt::<Track>(op, "track").map_err(step)? {
                    n.track = t;
                }
                if let Some(o) = parse_opt::<OnError>(op, "on_error").map_err(step)? {
                    n.on_error = o;
                }
                done.push(format!("changed node {id}"));
            }
            "remove_node" => {
                let id = s(op, "id");
                if f.node(id).is_none() {
                    return Err(step(format!("there is no node `{id}`")));
                }
                f.nodes.retain(|n| n.id != id);
                f.edges.retain(|e| e.from != id && e.to != id);
                done.push(format!("removed node {id}"));
            }
            "add_edge" => {
                let e = edge_from_value(op).map_err(step)?;
                if f.node(&e.from).is_none() || f.node(&e.to).is_none() {
                    return Err(step("both ends must be nodes of the flow".into()));
                }
                if f.edges.iter().any(|x| x.key() == e.key()) {
                    return Err(step("that edge exists".into()));
                }
                done.push(format!("connected {} to {}", e.from, e.to));
                f.edges.push(e);
            }
            "remove_edge" => {
                let (from, to) = (s(op, "from"), s(op, "to"));
                let before = f.edges.len();
                f.edges.retain(|e| !(e.from == from && e.to == to && (s(op, "port").is_empty() || e.port == s(op, "port"))));
                if f.edges.len() == before {
                    return Err(step("no such edge".into()));
                }
                done.push(format!("disconnected {from} from {to}"));
            }
            "set_trigger" => {
                let t = trigger_from_value(op).map_err(step)?;
                done.push(format!("trigger {} ({})", t.id, t.kind.label()));
                f.triggers.retain(|x| x.id != t.id);
                f.triggers.push(t);
            }
            "remove_trigger" => {
                let id = s(op, "id");
                let before = f.triggers.len();
                f.triggers.retain(|t| t.id != id);
                if f.triggers.len() == before {
                    return Err(step(format!("no trigger `{id}`")));
                }
                done.push(format!("removed trigger {id}"));
            }
            "set_default" => {
                apply_defaults(f, op).map_err(step)?;
                done.push("changed the flow's defaults".into());
            }
            "set_meta" => {
                if let Some(n) = op.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()) {
                    f.name = n.into();
                }
                if let Some(d) = op.get("description").and_then(Value::as_str) {
                    f.description = d.into();
                }
                done.push("changed the name or description".into());
            }
            "set_input" => {
                let n = s(op, "name");
                if n.is_empty() {
                    return Err(step("an input needs a `name`".into()));
                }
                f.inputs.retain(|p| p.name != n);
                f.inputs.push(Param { name: n.into(), ty: s(op, "type").into(), required: op.get("required").and_then(Value::as_bool).unwrap_or(false), default: op.get("default").cloned(), description: s(op, "description").into() });
                done.push(format!("input {n}"));
            }
            other => return Err(step(format!("`{other}` is not an operation (add_node, update_node, remove_node, add_edge, remove_edge, set_trigger, remove_trigger, set_default, set_meta, set_input)"))),
        }
    }
    Ok(done)
}

// ------------------------------------------------------------------------------------------------ what the model is told

fn hide(v: &Value) -> Value {
    match v {
        Value::String(t) => match crate::credguard::detect(t) {
            Some(f) => Value::String(f.redacted),
            None => Value::String(t.chars().take(300).collect()),
        },
        Value::Array(a) => Value::Array(a.iter().take(50).map(hide).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), if k.to_ascii_lowercase().contains("password") || k.to_ascii_lowercase().contains("secret") && !x.as_str().unwrap_or("").contains("{{secret.") && x.is_string() && !k.eq_ignore_ascii_case("secret") { Value::String("(hidden)".into()) } else { hide(x) })).collect()),
        other => other.clone(),
    }
}

/// A flow for the model: ids, kinds, a redacted config, edges, triggers and policies (never a credential: configs hold only
/// `{{secret.NAME}}` references, and anything that looks like a credential is hidden anyway).
pub fn flow_brief(f: &Flow, full_config: bool) -> String {
    let mut s = format!("{} (id {}) version {}{}{}\n", f.name, f.id, f.meta.version, if f.production { ", PRODUCTION" } else { "" }, if f.meta.modified_by.iter().any(Change::by_agent) { ", draft has changes by the agent" } else { "" });
    if !f.description.is_empty() {
        s.push_str(&format!("  {}\n", f.description.chars().take(200).collect::<String>()));
    }
    for n in &f.nodes {
        let cfg = hide(&n.config).to_string();
        s.push_str(&format!("  node {} [{}]{}{}{}{}\n", n.id, n.kind, if n.name.is_empty() { String::new() } else { format!(" \"{}\"", n.name) }, n.retry.as_ref().map(|r| format!(" retry({})", r.describe())).unwrap_or_default(), n.timeout_ms.map(|t| format!(" timeout {t} ms")).unwrap_or_default(), if n.runner == Some(RunnerKind::K3sJob) { " sandbox" } else { "" }));
        if full_config && cfg != "{}" && cfg != "null" {
            s.push_str(&format!("      config {}\n", cfg.chars().take(600).collect::<String>()));
        }
    }
    for e in &f.edges {
        s.push_str(&format!("  edge {} -{}-> {}{}\n", e.from, e.port, e.to, if e.when.is_empty() { String::new() } else { format!(" when {}", e.when) }));
    }
    for t in &f.triggers {
        s.push_str(&format!("  trigger {} {}{}\n", t.id, trigger_text(&t.kind), if t.enabled { "" } else { " (off)" }));
    }
    s.push_str(&format!("  defaults: timeout {} ms, retry {}, heal {}, runner {}\n", f.defaults.timeout_ms, f.defaults.retry.describe(), f.defaults.heal.mode.label(), f.defaults.runner.label()));
    s
}

pub fn trigger_text(k: &TriggerKind) -> String {
    match k {
        TriggerKind::Cron { expr, tz } => format!("cron `{expr}` ({tz})"),
        TriggerKind::Interval { every_s, .. } => format!("every {every_s} s"),
        TriggerKind::Once { at_ms } => format!("once at {at_ms}"),
        TriggerKind::Calendar { matching } => format!("calendar events with `{matching}`"),
        TriggerKind::FileChange { path } => format!("when {path} changes"),
        TriggerKind::OnFailure { flow } => format!("when {flow} fails"),
        TriggerKind::Event { name } => format!("on event {name}"),
        TriggerKind::Webhook { path } => format!("webhook {path} (planned)"),
        other => other.label().to_string(),
    }
}

pub fn run_line(r: &RunSummary) -> String {
    format!("{} {} v{} {} {}/{} steps{}{}", r.id, r.flow_id, r.version, r.status.label(), r.nodes_done, r.nodes_total, if r.error.is_empty() { String::new() } else { format!(" ERROR {}", r.error.chars().take(120).collect::<String>()) }, if r.blocked.is_empty() { String::new() } else { format!(" NEEDS YOU: {}", r.blocked.chars().take(120).collect::<String>()) })
}

/// The compact picture the chat inside the FLOWS tile gets with every message (at most 3,500 characters).
pub fn context_text(env: &FlowEnv, open: &Open) -> String {
    let mut s = String::from("FLOWS (the person's workflows).\n");
    for f in env.summaries().iter().take(25) {
        s.push_str(&format!("- {} \"{}\" v{}{}{} {} nodes\n", f.id, f.name, f.published, if f.draft_changed { " (draft changed)" } else { "" }, if f.agent_changed { " (agent changed)" } else { "" }, f.nodes));
    }
    if let Some(id) = &open.flow {
        if let Some(f) = env.store.get_draft(id) {
            s.push_str(&format!("\nOPEN ON SCREEN: {}tab {}\n", open.node.as_ref().map(|n| format!("selected node {n}, ")).unwrap_or_default(), open.tab));
            s.push_str(&flow_brief(&f, true));
            for l in env.issue_lines(&f).iter().take(6) {
                s.push_str(&format!("  issue: {l}\n"));
            }
        }
    }
    let c = env.engine.counts();
    s.push_str(&format!("\nRUNS: {} running, {} need you, {} scheduled, {} failed, {} done\n", c.running, c.needs_you, c.scheduled, c.failed, c.done));
    for r in env.engine.summaries().iter().take(8) {
        s.push_str(&format!("  {}\n", run_line(r)));
    }
    if let Some(r) = &open.run {
        if let Some(run) = env.engine.get_run(r) {
            s.push_str(&format!("SELECTED RUN {}\n", run_line(&run.summary())));
            for (id, nr) in &run.nodes {
                if matches!(nr.state, NodeState::Failed | NodeState::Retrying | NodeState::Waiting | NodeState::Interrupted) {
                    s.push_str(&format!("  node {id} {} attempts {}/{} {}\n", nr.state.label(), nr.attempts.len(), nr.max_attempts, nr.error.chars().take(120).collect::<String>()));
                }
            }
        }
    }
    let red = s.chars().take(3500).collect::<String>();
    crate::credguard::detect(&red).map(|f| f.redacted).unwrap_or(red)
}

/// "Why did it fail?" from the data: the failing step, its attempts and what the numbers say. The model reads it and explains.
pub fn explain_failure(run: &Run) -> String {
    let mut s = format!("Run {} of {} (v{}) is {}.\n", run.id, run.flow.name, run.version(), run.status.label());
    if !run.error.is_empty() {
        s.push_str(&format!("Error: {}\n", run.error));
    }
    let failing: Vec<(&String, &NodeRun)> = run.nodes.iter().filter(|(_, n)| matches!(n.state, NodeState::Failed | NodeState::Interrupted)).collect();
    if failing.is_empty() {
        s.push_str("No step failed in this run.\n");
    }
    for (id, nr) in failing {
        let node = run.flow.node(id);
        s.push_str(&format!("\nStep {id}{}: {} after {} attempt(s) of {}.\n", node.map(|n| format!(" [{}]", n.kind)).unwrap_or_default(), nr.state.label(), nr.attempts.len(), nr.max_attempts));
        for a in &nr.attempts {
            s.push_str(&format!("  attempt {}: limit {} ms, waited {} ms before, took {} ms, {}{}\n", a.n, a.timeout_ms, a.delay_before_ms, (a.ended_ms - a.started_ms).max(0), if a.ok { "ok".to_string() } else { format!("failed ({}): {}", a.class, a.error.chars().take(200).collect::<String>()) }, ""));
        }
        let e = nr.error.to_ascii_lowercase();
        let mut hints = Vec::new();
        if e.contains("http 4") && !e.contains("http 408") && !e.contains("http 429") {
            hints.push("a 4xx answer means the request itself was refused (wrong address, missing sign-in, bad data); retrying does not help");
        }
        if e.contains("http 5") || e.contains("http 429") || e.contains("http 408") {
            hints.push("the server answered with an error it may recover from; more attempts or longer pauses could help");
        }
        if e.contains("took longer than") || e.contains("timed out") || e.contains("timeout") {
            hints.push("the step hit its time limit: check the limit against how long the work really takes");
        }
        if e.contains("not installed") || e.contains("older than") {
            hints.push("a command-line tool the step needs is missing or too old: it can be installed or upgraded from the CLI tile");
        }
        if e.contains("no secret") {
            hints.push("a vault secret the step uses is missing: it has to be stored by the person (vault_request)");
        }
        if e.contains("exited with code") {
            hints.push("the command ran and exited with an error: its last output line is above");
        }
        for h in hints {
            s.push_str(&format!("  note: {h}\n"));
        }
        if let Some(n) = node {
            s.push_str(&format!("  retry policy: {}; timeout {} ms; heal mode {}\n", run.flow.retry_of(n).describe(), run.flow.timeout_of(n), run.flow.heal_of(n).mode.label()));
        }
        for l in nr.log.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
            s.push_str(&format!("  | {l}\n"));
        }
    }
    for c in &run.heal {
        s.push_str(&format!("\nFix card {} for {}: {:?}. {}{}\n", c.id, c.node, c.state, c.diagnosis.chars().take(300).collect::<String>(), c.patch.as_ref().map(|p| format!(" Proposed: {} [{:?}]", p.summary(), c.class.as_ref().map(|c| c.class))).unwrap_or_default()));
    }
    let t = crate::credguard::detect(&s).map(|f| f.redacted).unwrap_or(s);
    t.chars().take(6000).collect()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod qa_fixture;
#[cfg(test)]
mod browser_tests;
