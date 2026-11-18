//! The `remote_flow` node: run one of the person's flows on another IYAKKI and take the result back (F-FLOW-16). The engine does
//! not know how: it asks [`Host::remote_flow`], and the host finds the machine in the person's saved list (a name, never an
//! address in the flow), signs in with a token from the person's vault and follows the run through that machine's own web service.
//! Everything that machine decides stays decided there: a step of the remote run that needs a person waits for that person, and
//! this node then fails with what it asks, it never answers it.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::host::RemoteFlowRequest;

pub fn register(r: &mut Registry) {
    r.register(Arc::new(RemoteFlowKind));
}

pub const MAX_WAIT_S: u64 = 3600;

/// A saved machine's name: what the person typed when they saved it.
pub fn valid_machine(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

struct RemoteFlowKind;
impl NodeKind for RemoteFlowKind {
    fn id(&self) -> &'static str {
        "remote_flow"
    }
    fn label(&self) -> &'static str {
        "Run on another IYAKKI"
    }
    fn blurb(&self) -> &'static str {
        "Runs a flow on one of your saved machines and follows it; output run, status and steps."
    }
    fn category(&self) -> &'static str {
        "Call"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("machine", "Machine", FieldKind::Text, "the name of a saved machine (Settings, or the chat tool remote_machine_add); never an address").required(),
            Field::new("flow", "Flow", FieldKind::Text, "the id of the flow over there (its published version runs)").required(),
            Field::new("inputs", "Inputs", FieldKind::Json, "an object; may use {{ templates }}"),
            Field::new("wait_s", "Wait up to (s)", FieldKind::Number, "how long to follow the run; it goes on over there after that").default(json!(300)),
        ]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let m = s(c, "machine");
        if m.is_empty() {
            v.push(Issue::error(&node.id, "name a saved machine"));
        } else if !m.contains("{{") && !valid_machine(&m) {
            v.push(Issue::error(&node.id, "a machine name is letters, digits, - and _ (a saved name, not an address)"));
        }
        let f = s(c, "flow");
        if f.is_empty() {
            v.push(Issue::error(&node.id, "give the id of the flow over there"));
        } else if !f.contains("{{") && !valid_id(&f) {
            v.push(Issue::error(&node.id, "a flow id is letters, digits, - and _"));
        }
        if let Some(i) = c.get("inputs") {
            if !(i.is_null() || i.is_object() || i.as_str().is_some_and(|t| t.contains("{{"))) {
                v.push(Issue::error(&node.id, "inputs are an object"));
            }
        }
        if let Some(w) = c.get("wait_s") {
            let ok = w.as_u64().is_some_and(|n| (1..=MAX_WAIT_S).contains(&n)) || w.as_str().is_some_and(|t| t.contains("{{"));
            if !ok {
                v.push(Issue::error(&node.id, format!("wait is from 1 to {MAX_WAIT_S} seconds")));
            }
        }
        v
    }
    fn can_fail(&self) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, _node: &Node, cfg: &Value) -> NodeResult {
        let machine = s(cfg, "machine").trim().to_string();
        let flow = s(cfg, "flow").trim().to_string();
        if !valid_machine(&machine) || !valid_id(&flow) {
            return Err(NodeError::config("the machine or the flow name is not usable"));
        }
        let inputs = match cfg.get("inputs") {
            None | Some(Value::Null) => Value::Null,
            Some(v) if v.is_object() => v.clone(),
            _ => return Err(NodeError::config("inputs are an object")),
        };
        let wait_s = cfg.get("wait_s").and_then(Value::as_u64).unwrap_or(300).clamp(1, MAX_WAIT_S);
        let req = RemoteFlowRequest { run_id: cx.run_id.to_string(), flow_id: cx.flow.id.clone(), node_id: cx.node_id.to_string(), machine: machine.clone(), flow: flow.clone(), inputs, wait_s };
        cx.log(format!("remote: running flow {flow} on {machine}"));
        let r = cx.host.remote_flow(&req, cx.cancel)?;
        let steps: Vec<Value> = r.steps.iter().map(|(n, st)| json!({"node": n, "state": st})).collect();
        if !r.waiting.is_empty() {
            return Err(NodeError::new(format!("the run on {machine} waits for a person there: {}", r.waiting)));
        }
        if !r.ended {
            return Err(NodeError::timeout(format!("the run {} on {machine} was still {} after {wait_s} s (it goes on over there)", r.run, r.status)));
        }
        if r.status != "succeeded" {
            return Err(NodeError::new(format!("the run {} on {machine} {}{}", r.run, r.status, if r.error.is_empty() { String::new() } else { format!(": {}", r.error) })));
        }
        Ok(NodeOutput { value: json!({"run": r.run, "status": r.status, "steps": steps, "machine": machine}), port: "out".into(), wait: None, artifacts: vec![] })
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        default_class(e)
    }
    fn describe(&self, node: &Node) -> String {
        format!("run flow {} on {}", s(&node.config, "flow"), s(&node.config, "machine"))
    }
}
