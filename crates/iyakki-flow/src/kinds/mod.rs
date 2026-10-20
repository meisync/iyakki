//! Node kinds: a registry of things a node can be. Each kind describes itself (a schema the properties form is built from, its
//! ports), checks its own configuration, runs, and says which of its failures are worth retrying. Store extensions can register
//! more kinds later. See `docs/FLOWS.md` ("Node SDK").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::host::{Cancel, Clock, Host, Redactor};
use crate::model::*;
use crate::retry::{ErrClass, NodeError};

pub mod browser_qa;
pub mod cli;
pub mod control;
pub mod http;
pub mod other;

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Multiline,
    Number,
    Bool,
    Choice(&'static [&'static str]),
    /// Free JSON (an object or a list).
    Json,
    /// A list of text lines.
    List,
    /// The name of a vault secret.
    Secret,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub help: &'static str,
    pub required: bool,
    pub default: Option<Value>,
}

impl Field {
    pub fn new(key: &'static str, label: &'static str, kind: FieldKind, help: &'static str) -> Field {
        Field { key, label, kind, help, required: false, default: None }
    }
    pub fn required(mut self) -> Field {
        self.required = true;
        self
    }
    pub fn default(mut self, v: Value) -> Field {
        self.default = Some(v);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub level: Level,
    pub node: Option<String>,
    pub msg: String,
}

impl Issue {
    pub fn error(node: &str, msg: impl Into<String>) -> Issue {
        Issue { level: Level::Error, node: Some(node.into()), msg: msg.into() }
    }
    pub fn warn(node: &str, msg: impl Into<String>) -> Issue {
        Issue { level: Level::Warn, node: Some(node.into()), msg: msg.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ports {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

impl Ports {
    pub fn standard() -> Ports {
        Ports { inputs: vec!["in".into()], outputs: vec!["out".into()] }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum KindStatus {
    Ready,
    /// Known but not built: the reason is shown in the palette and by validation.
    Planned(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub enum WaitFor {
    Approval { message: String, timeout_s: Option<u64>, on_timeout: String },
    Until { at_ms: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeOutput {
    pub value: Value,
    pub port: String,
    pub wait: Option<WaitFor>,
    pub artifacts: Vec<String>,
}

impl NodeOutput {
    pub fn out(value: Value) -> NodeOutput {
        NodeOutput { value, port: "out".into(), wait: None, artifacts: vec![] }
    }
    pub fn port(mut self, p: &str) -> NodeOutput {
        self.port = p.into();
        self
    }
    pub fn waiting(w: WaitFor) -> NodeOutput {
        NodeOutput { value: Value::Null, port: "out".into(), wait: Some(w), artifacts: vec![] }
    }
}

pub type NodeResult = Result<NodeOutput, NodeError>;

/// Runs another flow once (the engine's implementation drives a child run).
pub trait SubRunner: Send + Sync {
    fn run_sub(&self, flow_ref: &str, inputs: Value, cancel: &Cancel) -> Result<Value, NodeError>;
}

pub struct NoSub;

impl SubRunner for NoSub {
    fn run_sub(&self, _f: &str, _i: Value, _c: &Cancel) -> Result<Value, NodeError> {
        Err(NodeError::config("sub-flows are not available here"))
    }
}

/// What a kind gets for one attempt.
pub struct NodeCx<'a> {
    pub run_id: &'a str,
    pub flow: &'a Flow,
    pub node_id: &'a str,
    pub attempt: u32,
    /// How long this attempt may take.
    pub timeout_ms: u64,
    /// The template context: `inputs`, `vars`, `nodes`, `input`, `run`, `item`.
    pub ctx: &'a Value,
    pub host: &'a dyn Host,
    pub cancel: &'a Cancel,
    pub clock: &'a dyn Clock,
    pub redactor: &'a Redactor,
    pub runner: RunnerKind,
    pub sub: &'a dyn SubRunner,
    pub logs: Mutex<Vec<String>>,
}

impl NodeCx<'_> {
    pub fn log(&self, line: impl AsRef<str>) {
        let l = self.redactor.text(line.as_ref());
        let mut g = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        if g.len() < 200 {
            g.push(l.chars().take(400).collect());
        }
    }

    /// Fill in templates (no secrets) with extra names in scope (`item`, `index`).
    pub fn resolve(&self, v: &Value, extra: &[(&str, Value)]) -> Result<Value, NodeError> {
        let mut c = self.ctx.clone();
        if let Value::Object(o) = &mut c {
            for (k, x) in extra {
                o.insert((*k).to_string(), x.clone());
            }
        }
        crate::expr::resolve(v, &c, None).map_err(|e| NodeError::config(e.0))
    }
}

pub trait NodeKind: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn blurb(&self) -> &'static str;
    /// Palette group: Control, Call, Run, Human, Flow.
    fn category(&self) -> &'static str;
    fn status(&self) -> KindStatus {
        KindStatus::Ready
    }
    fn schema(&self) -> Vec<Field>;
    fn ports(&self, _node: &Node) -> Ports {
        Ports::standard()
    }
    /// Can fail (and so has an `error` port).
    fn can_fail(&self) -> bool {
        true
    }
    /// Top-level config keys whose templates may use `{{secret.NAME}}`.
    fn secret_fields(&self) -> &'static [&'static str] {
        &[]
    }
    /// Config keys the engine leaves alone (expressions, or filled in by the kind itself).
    fn raw_fields(&self) -> &'static [&'static str] {
        &[]
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue>;
    /// Safe to run again after a restart, and safe to try with recorded inputs (read-only, no side effects).
    fn idempotent(&self, _node: &Node) -> bool {
        false
    }
    /// Can run in a k3s Job.
    fn sandboxable(&self) -> bool {
        false
    }
    /// `cfg` is the node's config with templates filled in (except `raw_fields`).
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult;
    /// Finish a node that was waiting (an approval decided, a time reached). `decision` is what the person or the clock said.
    fn resume(&self, _cx: &NodeCx, _node: &Node, _decision: &Value) -> NodeResult {
        Ok(NodeOutput::out(Value::Null))
    }
    fn classify_error(&self, e: &NodeError, _node: &Node) -> ErrClass {
        default_class(e)
    }
    /// One line for the card and the log: what this node does.
    fn describe(&self, node: &Node) -> String {
        format!("{} {}", self.label(), node.title())
    }
}

/// The classification every kind starts from: timeouts and connection trouble retry; a wrong configuration, a missing program and
/// a cancel do not.
pub fn default_class(e: &NodeError) -> ErrClass {
    if e.cancelled || e.config || e.missing || e.needs_cli.is_some() {
        return ErrClass::Permanent;
    }
    if let Some(t) = e.transient {
        return if t { ErrClass::Retryable } else { ErrClass::Permanent };
    }
    if e.timed_out || e.connection {
        ErrClass::Retryable
    } else {
        ErrClass::Permanent
    }
}

#[derive(Clone, Default)]
pub struct Registry {
    kinds: BTreeMap<String, Arc<dyn NodeKind>>,
}

impl Registry {
    pub fn empty() -> Registry {
        Registry::default()
    }
    pub fn builtin() -> Registry {
        let mut r = Registry::empty();
        control::register(&mut r);
        http::register(&mut r);
        cli::register(&mut r);
        other::register(&mut r);
        browser_qa::register(&mut r);
        r
    }
    pub fn register(&mut self, k: Arc<dyn NodeKind>) {
        self.kinds.insert(k.id().to_string(), k);
    }
    pub fn get(&self, id: &str) -> Option<&Arc<dyn NodeKind>> {
        self.kinds.get(id)
    }
    pub fn all(&self) -> Vec<&Arc<dyn NodeKind>> {
        let order = [
            "start", "end", "http", "cli", "browser", "agent", "script", "grpc", "thrift", "remote", "condition", "switch", "parallel", "join", "delay", "approval", "notify", "subflow", "foreach", "br_open", "br_screenshot", "br_dom", "br_assert", "br_click", "br_type", "br_fill", "br_submit", "br_key", "br_hover", "br_scroll", "br_select", "br_wait",
            "br_extract", "br_js", "br_frame", "br_viewport", "br_storage", "br_download", "br_compare",
        ];
        let mut v: Vec<&Arc<dyn NodeKind>> = self.kinds.values().collect();
        v.sort_by_key(|k| (order.iter().position(|o| *o == k.id()).unwrap_or(99), k.id()));
        v
    }
    pub fn ids(&self) -> Vec<String> {
        self.all().iter().map(|k| k.id().to_string()).collect()
    }
}

// ------------------------------------------------------------------------------------------------ config helpers

pub fn s<'a>(cfg: &'a Value, key: &str) -> &'a str {
    cfg.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn u(cfg: &Value, key: &str) -> Option<u64> {
    match cfg.get(key) {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Some(Value::String(t)) => t.trim().parse().ok(),
        _ => None,
    }
}

pub fn b(cfg: &Value, key: &str, default: bool) -> bool {
    cfg.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn list_of_strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(crate::expr::text).collect(),
        Value::String(t) if !t.is_empty() => t.lines().map(str::to_string).collect(),
        _ => vec![],
    }
}

/// A short text of a value for logs and previews.
pub fn preview(v: &Value, n: usize) -> String {
    let t = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if t.chars().count() <= n {
        t
    } else {
        format!("{}...", t.chars().take(n).collect::<String>())
    }
}

/// Parse stdout the way `parse` says: `text`, `json`, `lines`, `regex` (with `pattern`; named groups are not supported, groups are
/// numbered `1`, `2`..., and `match` is the whole match). Returns the `data` value.
pub fn parse_output(mode: &str, pattern: &str, text: &str) -> Result<Value, NodeError> {
    match mode {
        "" | "text" => Ok(Value::String(text.trim_end().to_string())),
        "json" => {
            let t = text.trim();
            serde_json::from_str(t).or_else(|_| {
                // several JSON documents one after another (a streaming gRPC call): a list of them
                let mut out = Vec::new();
                let mut de = serde_json::Deserializer::from_str(t).into_iter::<Value>();
                for v in de.by_ref() {
                    out.push(v.map_err(|e| NodeError::new(format!("the output is not JSON: {e}")))?);
                }
                if out.is_empty() {
                    Err(NodeError::new("the output is empty, JSON was expected"))
                } else if out.len() == 1 {
                    Ok(out.remove(0))
                } else {
                    Ok(Value::Array(out))
                }
            })
        }
        "lines" => Ok(Value::Array(text.lines().filter(|l| !l.trim().is_empty()).take(10_000).map(|l| json!(l)).collect())),
        "regex" => {
            let re = crate::minire::Re::new(pattern).map_err(|e| NodeError::config(format!("the pattern is not valid: {e}")))?;
            match re.captures(text) {
                Some(c) => {
                    let mut o = serde_json::Map::new();
                    o.insert("match".into(), json!(c.first().cloned().unwrap_or_default()));
                    for (i, g) in c.iter().enumerate().skip(1) {
                        o.insert(i.to_string(), json!(g));
                    }
                    Ok(Value::Object(o))
                }
                None => Err(NodeError::new("the output did not match the pattern")),
            }
        }
        other => Err(NodeError::config(format!("`{other}` is not a parse mode (text, json, lines, regex)"))),
    }
}

/// Header names that carry credentials: a literal value for one is refused by `validate` (use `{{secret.NAME}}`).
pub fn credential_header(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    ["authorization", "cookie", "token", "secret", "password", "api-key", "apikey", "api_key"].iter().any(|w| l.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_lists_every_kind_in_palette_order() {
        let r = Registry::builtin();
        let ids = r.ids();
        for k in ["start", "end", "http", "cli", "browser", "agent", "script", "grpc", "thrift", "remote", "condition", "switch", "parallel", "join", "delay", "approval", "notify", "subflow", "foreach"] {
            assert!(ids.iter().any(|i| i == k), "{k}");
        }
        assert_eq!(ids[0], "start");
        assert_eq!(r.get("thrift").unwrap().status(), KindStatus::Planned("the thrift adapter is not built yet"));
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn output_parsing_modes() {
        assert_eq!(parse_output("json", "", " {\"a\": 1} ").unwrap(), json!({"a": 1}));
        assert_eq!(parse_output("json", "", "{\"a\":1}\n{\"a\":2}").unwrap(), json!([{"a": 1}, {"a": 2}]));
        assert!(parse_output("json", "", "nope").is_err());
        assert_eq!(parse_output("lines", "", "a\n\nb\n").unwrap(), json!(["a", "b"]));
        assert_eq!(parse_output("regex", r"v(\d+)\.(\d+)", "tool v12.34 ok").unwrap(), json!({"match": "v12.34", "1": "12", "2": "34"}));
        assert!(parse_output("regex", r"zzz", "abc").is_err());
        assert!(parse_output("xml", "", "").is_err());
        assert_eq!(parse_output("text", "", "hi\n").unwrap(), json!("hi"));
    }

    #[test]
    fn the_default_class() {
        assert_eq!(default_class(&NodeError::timeout("x")), ErrClass::Retryable);
        assert_eq!(default_class(&NodeError { connection: true, ..Default::default() }), ErrClass::Retryable);
        assert_eq!(default_class(&NodeError::config("x")), ErrClass::Permanent);
        assert_eq!(default_class(&NodeError::cancelled()), ErrClass::Permanent);
        assert_eq!(default_class(&NodeError::new("x")), ErrClass::Permanent);
        assert_eq!(default_class(&NodeError { transient: Some(true), ..Default::default() }), ErrClass::Retryable);
    }
}
