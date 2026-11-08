//! The kinds that shape a flow rather than call out: start, end, condition, switch, parallel, join, delay, approval, notify,
//! sub-flow and for-each.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::expr;
use crate::host::Notice;

pub fn register(r: &mut Registry) {
    for k in [
        Arc::new(Start) as Arc<dyn NodeKind>,
        Arc::new(End),
        Arc::new(Condition),
        Arc::new(Switch),
        Arc::new(Parallel),
        Arc::new(Join),
        Arc::new(Delay),
        Arc::new(Approval),
        Arc::new(Notify),
        Arc::new(Subflow),
        Arc::new(Foreach),
    ] {
        r.register(k);
    }
}

pub const MAX_FOREACH: u64 = 1000;
pub const DEFAULT_FOREACH: u64 = 100;
pub const MAX_DELAY_MS: u64 = 7 * 24 * 3600 * 1000;

struct Start;
impl NodeKind for Start {
    fn id(&self) -> &'static str {
        "start"
    }
    fn label(&self) -> &'static str {
        "Start"
    }
    fn blurb(&self) -> &'static str {
        "Where a run begins; its output is the run's inputs."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![]
    }
    fn ports(&self, _n: &Node) -> Ports {
        Ports { inputs: vec![], outputs: vec!["out".into()] }
    }
    fn can_fail(&self) -> bool {
        false
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        if flow.incoming(&node.id).is_empty() {
            vec![]
        } else {
            vec![Issue::error(&node.id, "a start node has no incoming edge")]
        }
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, _c: &Value) -> NodeResult {
        Ok(NodeOutput::out(cx.ctx.get("inputs").cloned().unwrap_or(json!({}))))
    }
}

struct End;
impl NodeKind for End {
    fn id(&self) -> &'static str {
        "end"
    }
    fn label(&self) -> &'static str {
        "End"
    }
    fn blurb(&self) -> &'static str {
        "Where a path ends; `outputs` become the run's outputs."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![Field::new("outputs", "Outputs", FieldKind::Json, "name: value, values may use {{ templates }}")]
    }
    fn ports(&self, _n: &Node) -> Ports {
        Ports { inputs: vec!["in".into()], outputs: vec![] }
    }
    fn can_fail(&self) -> bool {
        false
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        if flow.outgoing(&node.id).is_empty() {
            vec![]
        } else {
            vec![Issue::error(&node.id, "an end node has no outgoing edge")]
        }
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, _cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        Ok(NodeOutput::out(cfg.get("outputs").cloned().filter(|v| v.is_object()).unwrap_or(json!({}))))
    }
}

struct Condition;
impl NodeKind for Condition {
    fn id(&self) -> &'static str {
        "condition"
    }
    fn label(&self) -> &'static str {
        "Condition"
    }
    fn blurb(&self) -> &'static str {
        "Takes the `true` or the `false` path by an expression."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![Field::new("expr", "Expression", FieldKind::Text, "for example nodes.fetch.output.status == 200 && len(nodes.fetch.output.items) > 0").required()]
    }
    fn ports(&self, _n: &Node) -> Ports {
        Ports { inputs: vec!["in".into()], outputs: vec!["true".into(), "false".into()] }
    }
    fn raw_fields(&self) -> &'static [&'static str] {
        &["expr"]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        match expr::parse(s(&node.config, "expr")) {
            _ if s(&node.config, "expr").trim().is_empty() => vec![Issue::error(&node.id, "the expression is empty")],
            Err(e) => vec![Issue::error(&node.id, format!("expression: {e}"))],
            Ok(_) => vec![],
        }
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let v = expr::eval_str(s(cfg, "expr"), cx.ctx).map_err(|e| NodeError::config(format!("expression: {e}")))?;
        let t = expr::truthy(&v);
        cx.log(format!("{} -> {t}", s(cfg, "expr")));
        Ok(NodeOutput::out(json!({"value": t, "result": v})).port(if t { "true" } else { "false" }))
    }
}

struct Switch;
impl NodeKind for Switch {
    fn id(&self) -> &'static str {
        "switch"
    }
    fn label(&self) -> &'static str {
        "Switch"
    }
    fn blurb(&self) -> &'static str {
        "Takes the path of the first matching case, else `default`."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("value", "Value", FieldKind::Text, "an expression, for example nodes.fetch.output.status").required(),
            Field::new("cases", "Cases", FieldKind::Json, "[{\"equals\": 200, \"port\": \"ok\"}, {\"when\": \"value > 500\", \"port\": \"down\"}]").required(),
        ]
    }
    fn ports(&self, node: &Node) -> Ports {
        let mut out: Vec<String> = node.config.get("cases").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.get("port").and_then(Value::as_str)).map(str::to_string).collect()).unwrap_or_default();
        out.dedup();
        out.push("default".into());
        Ports { inputs: vec!["in".into()], outputs: out }
    }
    fn raw_fields(&self) -> &'static [&'static str] {
        &["value", "cases"]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        if let Err(e) = expr::parse(s(&node.config, "value")) {
            v.push(Issue::error(&node.id, format!("value: {e}")));
        }
        match node.config.get("cases").and_then(Value::as_array) {
            Some(cases) if !cases.is_empty() => {
                for (i, c) in cases.iter().enumerate() {
                    if c.get("port").and_then(Value::as_str).is_none_or(str::is_empty) {
                        v.push(Issue::error(&node.id, format!("case {} has no port name", i + 1)));
                    }
                    if c.get("equals").is_none() && c.get("when").and_then(Value::as_str).is_none() {
                        v.push(Issue::error(&node.id, format!("case {} needs `equals` or `when`", i + 1)));
                    }
                    if let Some(w) = c.get("when").and_then(Value::as_str) {
                        if let Err(e) = expr::parse(w) {
                            v.push(Issue::error(&node.id, format!("case {}: {e}", i + 1)));
                        }
                    }
                }
            }
            _ => v.push(Issue::error(&node.id, "give at least one case")),
        }
        v
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let val = expr::eval_str(s(cfg, "value"), cx.ctx).map_err(|e| NodeError::config(format!("value: {e}")))?;
        let mut ctx = cx.ctx.clone();
        if let Value::Object(o) = &mut ctx {
            o.insert("value".into(), val.clone());
        }
        for c in cfg.get("cases").and_then(Value::as_array).cloned().unwrap_or_default() {
            let port = c.get("port").and_then(Value::as_str).unwrap_or("").to_string();
            let hit = if let Some(eq) = c.get("equals") {
                match (&val, eq) {
                    (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
                    _ => val == *eq,
                }
            } else {
                expr::condition(c.get("when").and_then(Value::as_str).unwrap_or(""), &ctx).map_err(|e| NodeError::config(format!("case `{port}`: {e}")))?
            };
            if hit && !port.is_empty() {
                cx.log(format!("{} -> {port}", preview(&val, 60)));
                return Ok(NodeOutput::out(json!({"value": val, "port": port})).port(&port));
            }
        }
        cx.log(format!("{} -> default", preview(&val, 60)));
        Ok(NodeOutput::out(json!({"value": val, "port": "default"})).port("default"))
    }
}

struct Parallel;
impl NodeKind for Parallel {
    fn id(&self) -> &'static str {
        "parallel"
    }
    fn label(&self) -> &'static str {
        "Parallel"
    }
    fn blurb(&self) -> &'static str {
        "Fan out: every edge from it starts at once (up to the flow's parallel limit)."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![]
    }
    fn can_fail(&self) -> bool {
        false
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        if flow.outgoing(&node.id).len() < 2 {
            vec![Issue::warn(&node.id, "a parallel node with fewer than two branches does nothing")]
        } else {
            vec![]
        }
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, _c: &Value) -> NodeResult {
        Ok(NodeOutput::out(cx.ctx.get("input").cloned().unwrap_or(Value::Null)))
    }
}

struct Join;
impl NodeKind for Join {
    fn id(&self) -> &'static str {
        "join"
    }
    fn label(&self) -> &'static str {
        "Join"
    }
    fn blurb(&self) -> &'static str {
        "Waits for every branch to finish; `all` needs each branch to have run, `any` needs one."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![Field::new("mode", "Run when", FieldKind::Choice(&["any", "all"]), "any: at least one branch ran. all: every branch ran.").default(json!("any"))]
    }
    fn can_fail(&self) -> bool {
        false
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        if flow.incoming(&node.id).len() < 2 {
            v.push(Issue::warn(&node.id, "a join with fewer than two incoming edges does nothing"));
        }
        if !matches!(s(&node.config, "mode"), "" | "any" | "all") {
            v.push(Issue::error(&node.id, "mode is `any` or `all`"));
        }
        v
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, _c: &Value) -> NodeResult {
        Ok(NodeOutput::out(json!({"branches": cx.ctx.get("input").cloned().unwrap_or(Value::Null)})))
    }
}

struct Delay;
impl NodeKind for Delay {
    fn id(&self) -> &'static str {
        "delay"
    }
    fn label(&self) -> &'static str {
        "Delay"
    }
    fn blurb(&self) -> &'static str {
        "Waits (without holding a worker); a restart waits only for the rest."
    }
    fn category(&self) -> &'static str {
        "Control"
    }
    fn schema(&self) -> Vec<Field> {
        vec![Field::new("ms", "Milliseconds", FieldKind::Number, "how long to wait"), Field::new("seconds", "Seconds", FieldKind::Number, "or seconds"), Field::new("at_ms", "Until (epoch ms)", FieldKind::Number, "or a time")]
    }
    fn can_fail(&self) -> bool {
        false
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        if u(c, "ms").is_none() && u(c, "seconds").is_none() && u(c, "at_ms").is_none() && !s(c, "ms").contains("{{") && !s(c, "seconds").contains("{{") {
            vec![Issue::error(&node.id, "give `ms`, `seconds` or `at_ms`")]
        } else {
            vec![]
        }
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let now = cx.clock.now_ms();
        let at = if let Some(a) = u(cfg, "at_ms") {
            a as i64
        } else {
            let ms = u(cfg, "ms").or_else(|| u(cfg, "seconds").map(|x| x.saturating_mul(1000))).ok_or_else(|| NodeError::config("give `ms`, `seconds` or `at_ms`"))?;
            now + ms.min(MAX_DELAY_MS) as i64
        };
        let at = at.min(now + MAX_DELAY_MS as i64);
        cx.log(format!("waiting {} ms", (at - now).max(0)));
        Ok(NodeOutput::waiting(WaitFor::Until { at_ms: at }))
    }
    fn resume(&self, _cx: &NodeCx, _n: &Node, d: &Value) -> NodeResult {
        Ok(NodeOutput::out(json!({"waited_until_ms": d.get("at_ms").cloned().unwrap_or(Value::Null)})))
    }
}

struct Approval;
impl NodeKind for Approval {
    fn id(&self) -> &'static str {
        "approval"
    }
    fn label(&self) -> &'static str {
        "Approval"
    }
    fn blurb(&self) -> &'static str {
        "Stops the run until a person approves or rejects (it shows as `needs you`)."
    }
    fn category(&self) -> &'static str {
        "Human"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("message", "Question", FieldKind::Multiline, "what the person is asked").required(),
            Field::new("timeout_s", "Wait at most (s)", FieldKind::Number, "0 or empty: wait for ever"),
            Field::new("on_timeout", "When time runs out", FieldKind::Choice(&["reject", "approve"]), "").default(json!("reject")),
        ]
    }
    fn ports(&self, _n: &Node) -> Ports {
        Ports { inputs: vec!["in".into()], outputs: vec!["approved".into(), "rejected".into()] }
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        if s(&node.config, "message").trim().is_empty() {
            v.push(Issue::error(&node.id, "ask a question"));
        }
        if !matches!(s(&node.config, "on_timeout"), "" | "reject" | "approve") {
            v.push(Issue::error(&node.id, "on_timeout is `reject` or `approve`"));
        }
        v
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let t = u(cfg, "timeout_s").filter(|x| *x > 0);
        cx.log(format!("waiting for a decision: {}", preview(&json!(s(cfg, "message")), 80)));
        Ok(NodeOutput::waiting(WaitFor::Approval { message: s(cfg, "message").to_string(), timeout_s: t, on_timeout: if s(cfg, "on_timeout").is_empty() { "reject".into() } else { s(cfg, "on_timeout").to_string() } }))
    }
    fn resume(&self, cx: &NodeCx, node: &Node, d: &Value) -> NodeResult {
        let approved = d.get("approved").and_then(Value::as_bool).unwrap_or(false);
        let by = d.get("by").and_then(Value::as_str).unwrap_or("");
        let note = d.get("note").and_then(Value::as_str).unwrap_or("");
        let out = json!({"approved": approved, "by": by, "note": note, "timed_out": d.get("timed_out").cloned().unwrap_or(json!(false))});
        if approved {
            return Ok(NodeOutput::out(out).port("approved"));
        }
        if cx.flow.outgoing(&node.id).iter().any(|e| e.port == "rejected") {
            Ok(NodeOutput::out(out).port("rejected"))
        } else {
            Err(NodeError::new(format!("{} {}{}", if d.get("timed_out").and_then(Value::as_bool).unwrap_or(false) { "nobody answered in time, so it was rejected" } else { "rejected by" }, by, if note.is_empty() { String::new() } else { format!(": {note}") })))
        }
    }
}

/// `inbox, Matrix` -> `["inbox", "matrix"]`.
fn channel_names(list: &str) -> Vec<String> {
    list.split([',', ';', ' ']).map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()).collect()
}

struct Notify;
impl NodeKind for Notify {
    fn id(&self) -> &'static str {
        "notify"
    }
    fn label(&self) -> &'static str {
        "Notify"
    }
    fn blurb(&self) -> &'static str {
        "Puts a note in the INBOX, and tells you in a Matrix room if you ask."
    }
    fn category(&self) -> &'static str {
        "Human"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("title", "Title", FieldKind::Text, "").required(),
            Field::new("body", "Text", FieldKind::Multiline, "may use {{ templates }}"),
            Field::new("level", "Level", FieldKind::Choice(&["info", "warn", "error"]), "").default(json!("info")),
            Field::new("to", "Send to", FieldKind::Text, "cloud tenants only: me (default), team, role:admin, role:tenant_admin, or a person's name; a desktop has one person"),
            Field::new("channels", "Also tell me by", FieldKind::Text, "inbox, matrix (the INBOX note is always made). Matrix needs the account in Settings, Notifications"),
            Field::new("room", "Matrix room", FieldKind::Text, "the name you saved the room under; empty: your only or main room"),
            Field::new("attach", "Picture", FieldKind::Text, "a camera step's file, for example {{ nodes.cam.output.path }}; only a picture of this run can be sent"),
            Field::new("dry_run", "Dry run", FieldKind::Bool, "check and show what would be sent, send nothing"),
        ]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let mut v = vec![];
        if s(&node.config, "title").trim().is_empty() {
            v.push(Issue::error(&node.id, "give a title"));
        }
        for c in channel_names(s(&node.config, "channels")) {
            if c != "matrix" && c != "inbox" {
                v.push(Issue::error(&node.id, format!("unknown channel `{c}` (use inbox or matrix)")));
            }
        }
        v
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let level = if s(cfg, "level").is_empty() { "info" } else { s(cfg, "level") };
        let n = Notice { title: s(cfg, "title").to_string(), body: s(cfg, "body").to_string(), level: level.to_string(), flow_id: cx.flow.id.clone(), run_id: cx.run_id.to_string(), needs_you: false };
        cx.host.notify_to(&n, cfg.get("to").unwrap_or(&Value::Null)).map_err(NodeError::new)?;
        cx.log(format!("notified: {}", n.title));
        let mut out = json!({"sent": true, "title": n.title});
        let extra: Vec<String> = channel_names(s(cfg, "channels")).into_iter().filter(|c| c != "inbox").collect();
        if !extra.is_empty() {
            let c = crate::host::ChannelNotice { channels: extra, room: s(cfg, "room").trim().to_string(), attach: s(cfg, "attach").trim().to_string(), dry_run: b(cfg, "dry_run", false), dedupe_key: format!("{}:{}:{}", cx.flow.id, cx.run_id, node.id) };
            // the INBOX note is made; a channel that cannot be reached is said plainly and does not fail the run
            match cx.host.notify_channels(&n, &c) {
                Ok(lines) => {
                    for l in &lines {
                        cx.log(l.clone());
                    }
                    out["channels"] = json!({"ok": true, "lines": lines});
                }
                Err(e) => {
                    cx.log(format!("not sent through {}: {e}", c.channels.join(", ")));
                    out["channels"] = json!({"ok": false, "error": e});
                }
            }
        }
        Ok(NodeOutput::out(out))
    }
}

struct Subflow;
impl NodeKind for Subflow {
    fn id(&self) -> &'static str {
        "subflow"
    }
    fn label(&self) -> &'static str {
        "Sub-flow"
    }
    fn blurb(&self) -> &'static str {
        "Runs another flow once and returns its outputs."
    }
    fn category(&self) -> &'static str {
        "Flow"
    }
    fn schema(&self) -> Vec<Field> {
        vec![Field::new("flow", "Flow id", FieldKind::Text, "the flow to run (its latest published version)").required(), Field::new("inputs", "Inputs", FieldKind::Json, "name: value, values may use {{ templates }}")]
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        let f = s(&node.config, "flow");
        if f.is_empty() {
            vec![Issue::error(&node.id, "name the flow to run")]
        } else if f == flow.id {
            vec![Issue::error(&node.id, "a flow cannot run itself")]
        } else {
            vec![]
        }
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let out = cx.sub.run_sub(s(cfg, "flow"), cfg.get("inputs").cloned().unwrap_or(json!({})), cx.cancel)?;
        Ok(NodeOutput::out(out))
    }
}

struct Foreach;
impl NodeKind for Foreach {
    fn id(&self) -> &'static str {
        "foreach"
    }
    fn label(&self) -> &'static str {
        "For each"
    }
    fn blurb(&self) -> &'static str {
        "Runs a flow once per item of a list (capped)."
    }
    fn category(&self) -> &'static str {
        "Flow"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("items", "Items", FieldKind::Text, "a list, for example {{ nodes.fetch.output.items }}").required(),
            Field::new("flow", "Flow id", FieldKind::Text, "run once per item; the item is `inputs._item`, its place `inputs._index`").required(),
            Field::new("inputs", "Inputs", FieldKind::Json, "extra inputs; may use {{ item }} and {{ index }}"),
            Field::new("max_items", "At most", FieldKind::Number, "default 100, never more than 1000").default(json!(100)),
            Field::new("concurrency", "At a time", FieldKind::Number, "default 1, at most 8").default(json!(1)),
            Field::new("stop_on_error", "Stop at the first failure", FieldKind::Bool, "").default(json!(true)),
        ]
    }
    fn raw_fields(&self) -> &'static [&'static str] {
        &["inputs"]
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        let c = &node.config;
        if c.get("items").is_none() {
            v.push(Issue::error(&node.id, "give `items`"));
        }
        match s(c, "flow") {
            "" => v.push(Issue::error(&node.id, "name the flow to run per item")),
            f if f == flow.id => v.push(Issue::error(&node.id, "a flow cannot run itself")),
            _ => {}
        }
        if u(c, "max_items").is_some_and(|m| m > MAX_FOREACH) {
            v.push(Issue::error(&node.id, format!("max_items is at most {MAX_FOREACH}")));
        }
        v
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let items: Vec<Value> = match cfg.get("items") {
            Some(Value::Array(a)) => a.clone(),
            Some(Value::Null) | None => vec![],
            Some(other) => return Err(NodeError::config(format!("`items` must be a list, it is {}", preview(other, 40)))),
        };
        let cap = u(cfg, "max_items").unwrap_or(DEFAULT_FOREACH).clamp(1, MAX_FOREACH) as usize;
        if items.len() > cap {
            return Err(NodeError::config(format!("{} items is more than the cap of {cap} (raise `max_items`, at most {MAX_FOREACH})", items.len())));
        }
        let conc = u(cfg, "concurrency").unwrap_or(1).clamp(1, 8) as usize;
        let stop = b(cfg, "stop_on_error", true);
        let flow = s(cfg, "flow").to_string();
        let one = |i: usize, item: &Value| -> Result<Value, NodeError> {
            let mut inputs = cx.resolve(cfg.get("inputs").unwrap_or(&json!({})), &[("item", item.clone()), ("index", json!(i))])?;
            if !inputs.is_object() {
                inputs = json!({});
            }
            inputs["_item"] = item.clone();
            inputs["_index"] = json!(i);
            cx.sub.run_sub(&flow, inputs, cx.cancel)
        };
        let mut results: Vec<Value> = vec![Value::Null; items.len()];
        let mut failed = 0usize;
        let mut first_err: Option<NodeError> = None;
        let mut i = 0;
        while i < items.len() {
            if cx.cancel.is_cancelled() {
                return Err(NodeError::cancelled());
            }
            let batch: Vec<usize> = (i..items.len().min(i + conc)).collect();
            let outs: Vec<(usize, Result<Value, NodeError>)> = if batch.len() == 1 {
                vec![(batch[0], one(batch[0], &items[batch[0]]))]
            } else {
                std::thread::scope(|sc| {
                    let (one, items) = (&one, &items);
                    let hs: Vec<_> = batch.iter().map(|&ix| (ix, sc.spawn(move || one(ix, &items[ix])))).collect();
                    hs.into_iter().map(|(ix, h)| (ix, h.join().unwrap_or_else(|_| Err(NodeError::new("a sub-flow panicked"))))).collect()
                })
            };
            for (ix, r) in outs {
                match r {
                    Ok(v) => results[ix] = v,
                    Err(e) => {
                        failed += 1;
                        cx.log(format!("item {ix} failed: {}", e.message));
                        results[ix] = json!({"error": e.message});
                        first_err.get_or_insert(e);
                    }
                }
            }
            if stop && first_err.is_some() {
                break;
            }
            i += conc;
        }
        if stop {
            if let Some(e) = first_err {
                return Err(e);
            }
        }
        Ok(NodeOutput::out(json!({"items": results, "count": items.len(), "failed": failed})))
    }
}
