//! The flow model from data, and what the tile tells the assistant (F-FLOW-01): build a flow from a spec, apply edit operations, the model's
//! brief of a flow, the keys the FLOWS chats are kept under. Pure: no `Services`, no engine host. The agent re-exports it all from
//! `iyakki_agent::flows`.

use std::sync::{Arc, Mutex};

use iyakki_flow::layout;
use iyakki_flow::*;
use serde_json::{json, Value};

/// A webhook's id: letters, digits, - and _ (at most 48).
pub fn hook_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

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

/// The key one workflow's conversation is kept under (`None`: the conversation of no workflow open). Ids are hashed, so a flow id
/// cannot collide with another key or reach another person's.
pub fn chat_key_for(user: &str, flow: Option<&str>) -> String {
    let Some(flow) = flow else { return chat_key(user) };
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("flows\u{1}{user}\u{1}{flow}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("flowsf_{h:016x}")
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
        "webhook" => {
            let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|x| x.to_string()).collect::<Vec<_>>()).unwrap_or_default();
            TriggerKind::Webhook {
                path: s(v, "path").to_string(),
                methods: list("methods").into_iter().map(|m| m.to_ascii_uppercase()).collect(),
                max_body: v.get("max_body").and_then(Value::as_u64).unwrap_or(0) as usize,
                content_types: list("content_types"),
                rate_per_min: v.get("rate_per_min").and_then(Value::as_u64).unwrap_or(0).min(100_000) as u32,
                signed: v.get("signed").and_then(Value::as_bool).unwrap_or(false),
            }
        }
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
    if d.get("assist").is_some() {
        // the agent's own permission is the person's to give: only in the Flows screen (flow settings, Assist), never through the assistant
        return Err("only the person can switch Assist on, off or to autopilot (FLOWS, flow settings: Assist); the assistant cannot change it, and a new flow starts with Assist off".into());
    }
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
    if d.get("camera").is_some_and(|c| c.get("allowed").is_some() || c.get("unattended").is_some() || c.get("blocked").is_some()) {
        // the camera is private: only the person switches it on, in the flow's settings, after reading what it means
        return Err("only the person can let a flow use the camera (FLOWS, flow settings: Camera allowed); the assistant can add camera steps but not switch the permission on".into());
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
        Value::String(t) => match iyakki_credguard::detect(t) {
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
    if !f.inputs.is_empty() {
        let names: Vec<String> = f.inputs.iter().take(20).map(|p| format!("{}{}", p.name.chars().take(40).collect::<String>(), if p.required { " (required)" } else { "" })).collect();
        s.push_str(&format!("  inputs (names only): {}\n", names.join(", ")));
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
        TriggerKind::Webhook { path, signed, .. } => format!("webhook {path}{}", if *signed { " (signed)" } else { "" }),
        other => other.label().to_string(),
    }
}

pub fn run_line(r: &RunSummary) -> String {
    format!("{} {} v{} {} {}/{} steps{}{}", r.id, r.flow_id, r.version, r.status.label(), r.nodes_done, r.nodes_total, if r.error.is_empty() { String::new() } else { format!(" ERROR {}", r.error.chars().take(120).collect::<String>()) }, if r.blocked.is_empty() { String::new() } else { format!(" NEEDS YOU: {}", r.blocked.chars().take(120).collect::<String>()) })
}


/// A name for a log line or a chip: one line, no control characters, bounded.
pub fn plain_name(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}
