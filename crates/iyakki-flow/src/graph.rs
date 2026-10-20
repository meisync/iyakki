//! The graph rules of a run, as plain functions over a [`Run`]: which nodes are ready, what flows along an edge, which paths are dead.
//! No threads, no clock, no host: the property tests drive these directly.

use serde_json::{json, Map, Value};

use crate::expr;
use crate::kinds::Registry;
use crate::model::*;

/// Fill in a node state for every node and an `Open` state for every edge.
pub fn init(run: &mut Run) {
    for n in &run.flow.nodes {
        run.nodes.entry(n.id.clone()).or_default();
    }
    for e in &run.flow.edges {
        run.edges.entry(e.key()).or_default();
    }
}

pub fn status_word(s: NodeState) -> &'static str {
    match s {
        NodeState::Succeeded => "succeeded",
        other => other.label(),
    }
}

/// The context templates and expressions see.
pub fn ctx_for(run: &Run, input: &Value, extra: &[(&str, Value)]) -> Value {
    let mut nodes = Map::new();
    for (id, nr) in &run.nodes {
        if nr.state == NodeState::Pending && nr.output.is_null() {
            continue;
        }
        nodes.insert(id.clone(), json!({"output": nr.output, "status": status_word(nr.state), "error": nr.error, "port": nr.port}));
    }
    let mut c = json!({
        "inputs": run.inputs,
        "vars": run.flow.vars,
        "nodes": Value::Object(nodes),
        "input": input,
        "run": {"id": run.id, "trigger": run.trigger.kind, "flow": run.flow_id, "version": run.flow.meta.version},
    });
    if let Value::Object(o) = &mut c {
        if let Some(item) = run.inputs.get("_item") {
            o.insert("item".into(), item.clone());
            o.insert("index".into(), run.inputs.get("_index").cloned().unwrap_or(Value::Null));
        }
        for (k, v) in extra {
            o.insert((*k).to_string(), v.clone());
        }
    }
    c
}

fn join_all(run: &Run, id: &str) -> bool {
    run.flow.node(id).is_some_and(|n| n.kind == "join" && n.config.get("mode").and_then(Value::as_str) == Some("all"))
}

/// The edges into `id` and their states, in flow order.
fn incoming_states(run: &Run, id: &str) -> Vec<(String, EdgeState)> {
    run.flow.edges.iter().filter(|e| e.to == id).map(|e| (e.from.clone(), run.edges.get(&e.key()).copied().unwrap_or_default())).collect()
}

/// Nodes that may start now, in the flow's node order.
pub fn ready(run: &Run, reg: &Registry) -> Vec<String> {
    let _ = reg;
    let mut out = Vec::new();
    for n in &run.flow.nodes {
        let Some(nr) = run.nodes.get(&n.id) else { continue };
        if nr.state != NodeState::Pending {
            continue;
        }
        let inc = incoming_states(run, &n.id);
        if inc.is_empty() {
            if n.kind == "start" {
                out.push(n.id.clone());
            }
            continue;
        }
        if inc.iter().any(|(_, s)| *s == EdgeState::Open) {
            continue;
        }
        let taken = inc.iter().filter(|(_, s)| *s == EdgeState::Taken).count();
        if taken == 0 || (join_all(run, &n.id) && taken < inc.len()) {
            continue;
        }
        out.push(n.id.clone());
    }
    out
}

/// What a node receives: the one output flowing in, or an object by source node when several do. A start node gets the run's inputs.
pub fn input_for(run: &Run, id: &str) -> Value {
    let taken: Vec<(&str, &Value)> = run.flow.edges.iter().filter(|e| e.to == id && run.edges.get(&e.key()) == Some(&EdgeState::Taken)).filter_map(|e| run.nodes.get(&e.from).map(|nr| (e.from.as_str(), &nr.output))).collect();
    match taken.as_slice() {
        [] => run.inputs.clone(),
        [(_, v)] => (*v).clone(),
        many => {
            let mut m = Map::new();
            for (k, v) in many {
                m.insert((*k).to_string(), (*v).clone());
            }
            Value::Object(m)
        }
    }
}

/// Set the state of the edges leaving `id` from how it ended, then kill the paths that can no longer run.
pub fn settle(run: &mut Run, id: &str, now_ms: i64) {
    settle_one(run, id);
    propagate(run, now_ms);
}

fn settle_one(run: &mut Run, id: &str) {
    let Some(nr) = run.nodes.get(id) else { return };
    let (state, port, output) = (nr.state, nr.port.clone(), nr.output.clone());
    let edges: Vec<Edge> = run.flow.edges.iter().filter(|e| e.from == id).cloned().collect();
    if edges.is_empty() {
        return;
    }
    let ctx = if edges.iter().any(|e| !e.when.trim().is_empty()) && state == NodeState::Succeeded { Some(ctx_for(run, &output, &[("output", output.clone())])) } else { None };
    for e in edges {
        let st = match state {
            NodeState::Succeeded => {
                let when_ok = e.when.trim().is_empty() || ctx.as_ref().is_some_and(|c| expr::condition(&e.when, c).unwrap_or(false));
                if e.port == port && when_ok {
                    EdgeState::Taken
                } else {
                    EdgeState::Dead
                }
            }
            NodeState::Failed => {
                if e.port == "error" {
                    EdgeState::Taken
                } else {
                    EdgeState::Dead
                }
            }
            NodeState::Skipped | NodeState::Cancelled => EdgeState::Dead,
            _ => continue,
        };
        run.edges.insert(e.key(), st);
    }
}

/// A node whose incoming edges are all decided and none is taken (or a join that needs all of them) is skipped, which kills its
/// outgoing edges in turn.
pub fn propagate(run: &mut Run, now_ms: i64) {
    loop {
        let mut changed = false;
        let ids: Vec<String> = run.flow.nodes.iter().map(|n| n.id.clone()).collect();
        for id in ids {
            if run.nodes.get(&id).map(|n| n.state) != Some(NodeState::Pending) {
                continue;
            }
            let inc = incoming_states(run, &id);
            let kind_start = run.flow.node(&id).is_some_and(|n| n.kind == "start");
            let dead = if inc.is_empty() {
                !kind_start
            } else if inc.iter().any(|(_, s)| *s == EdgeState::Open) {
                false
            } else {
                let taken = inc.iter().filter(|(_, s)| *s == EdgeState::Taken).count();
                taken == 0 || (join_all(run, &id) && taken < inc.len())
            };
            if dead {
                if let Some(nr) = run.nodes.get_mut(&id) {
                    nr.state = NodeState::Skipped;
                    nr.ended_ms = now_ms;
                }
                settle_one(run, &id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Reset a node and everything after it to pending (rerun from here).
pub fn reset_from(run: &mut Run, id: &str) -> Vec<String> {
    let mut ids = run.flow.descendants(id);
    ids.insert(0, id.to_string());
    for n in &ids {
        if let Some(nr) = run.nodes.get_mut(n) {
            let attempts = std::mem::take(&mut nr.attempts);
            *nr = NodeRun { attempts, ..Default::default() };
        }
        for e in run.flow.edges.iter().filter(|e| &e.from == n) {
            run.edges.insert(e.key(), EdgeState::Open);
        }
    }
    // an edge from an untouched node into a reset one keeps its state: the upstream result is still good
    ids
}

pub fn all_done(run: &Run) -> bool {
    run.nodes.values().all(|n| n.state.is_done())
}

/// The nodes that failed for good (not routed anywhere).
pub fn failed_unrouted(run: &Run) -> Vec<String> {
    run.flow.nodes.iter().filter(|n| run.nodes.get(&n.id).is_some_and(|r| r.state == NodeState::Failed)).filter(|n| !run.flow.outgoing(&n.id).iter().any(|e| e.port == "error")).map(|n| n.id.clone()).collect()
}

#[cfg(test)]
pub fn test_run(flow: Flow) -> Run {
    let mut r = Run { id: "r1".into(), flow_id: flow.id.clone(), flow, trigger: TriggerInfo { kind: "manual".into(), ..Default::default() }, status: RunStatus::Running, inputs: json!({"n": 2}), outputs: Value::Null, nodes: std::collections::BTreeMap::new(), edges: std::collections::BTreeMap::new(), created_ms: 0, scheduled_ms: None, started_ms: 0, ended_ms: 0, error: String::new(), blocked: String::new(), idempotency_key: None, parent: None, heal: vec![], patches: vec![], heal_spent: 0, heal_cents: 0, pending_updates: std::collections::BTreeMap::new(), log: vec![], rev: 0, runner_note: String::new() };
    init(&mut r);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn mk(flow: Flow) -> Run {
        super::test_run(flow)
    }

    pub fn finish(r: &mut Run, id: &str, out: Value, port: &str) {
        let nr = r.nodes.get_mut(id).unwrap();
        nr.state = NodeState::Succeeded;
        nr.output = out;
        nr.port = port.into();
        settle(r, id, 1);
    }

    fn diamond() -> Flow {
        let mut f = Flow::new("f", "F");
        for (id, k) in [("s", "start"), ("c", "condition"), ("a", "http"), ("b", "http"), ("j", "join"), ("e", "end")] {
            f.nodes.push(Node::new(id, k, json!({})));
        }
        f.edges = vec![Edge::new("s", "c"), Edge::new("c", "a").port("true"), Edge::new("c", "b").port("false"), Edge::new("a", "j"), Edge::new("b", "j"), Edge::new("j", "e")];
        f
    }

    #[test]
    fn start_is_ready_first_and_order_follows_the_flow() {
        let r = mk(diamond());
        let reg = Registry::builtin();
        assert_eq!(ready(&r, &reg), vec!["s"]);
        let mut r = r;
        finish(&mut r, "s", json!({}), "out");
        assert_eq!(ready(&r, &reg), vec!["c"]);
    }

    #[test]
    fn a_condition_kills_the_other_branch_and_the_join_still_runs() {
        let reg = Registry::builtin();
        let mut r = mk(diamond());
        finish(&mut r, "s", json!({}), "out");
        finish(&mut r, "c", json!({"value": true}), "true");
        assert_eq!(r.nodes["b"].state, NodeState::Skipped, "the false branch is dead");
        assert_eq!(ready(&r, &reg), vec!["a"]);
        finish(&mut r, "a", json!({"x": 1}), "out");
        assert_eq!(ready(&r, &reg), vec!["j"], "any: one branch ran, the other is dead");
        assert_eq!(input_for(&r, "j"), json!({"x": 1}), "one input comes as itself");
        finish(&mut r, "j", json!({}), "out");
        assert_eq!(ready(&r, &reg), vec!["e"]);
        finish(&mut r, "e", json!({}), "out");
        assert!(all_done(&r));
    }

    #[test]
    fn join_all_needs_every_branch() {
        let reg = Registry::builtin();
        let mut f = diamond();
        f.node_mut("j").unwrap().config = json!({"mode": "all"});
        let mut r = mk(f);
        finish(&mut r, "s", json!({}), "out");
        finish(&mut r, "c", json!({}), "true");
        finish(&mut r, "a", json!(1), "out");
        assert_eq!(r.nodes["j"].state, NodeState::Skipped, "a branch is dead, so `all` skips");
        assert!(ready(&r, &reg).is_empty());
        // fan-out and fan-in: both run
        let mut g = Flow::new("g", "G");
        for (id, k) in [("s", "start"), ("p", "parallel"), ("a", "http"), ("b", "http"), ("j", "join")] {
            g.nodes.push(Node::new(id, k, json!(if id == "j" { json!({"mode": "all"}) } else { json!({}) })));
        }
        g.edges = vec![Edge::new("s", "p"), Edge::new("p", "a"), Edge::new("p", "b"), Edge::new("a", "j"), Edge::new("b", "j")];
        let mut r = mk(g);
        finish(&mut r, "s", json!({}), "out");
        finish(&mut r, "p", json!({}), "out");
        assert_eq!(ready(&r, &reg), vec!["a", "b"], "both branches start together");
        finish(&mut r, "a", json!("A"), "out");
        assert!(ready(&r, &reg) == vec!["b"], "j waits for b");
        finish(&mut r, "b", json!("B"), "out");
        assert_eq!(ready(&r, &reg), vec!["j"]);
        assert_eq!(input_for(&r, "j"), json!({"a": "A", "b": "B"}));
    }

    #[test]
    fn edge_conditions_route_data() {
        let mut f = Flow::new("f", "F");
        for (id, k) in [("s", "start"), ("a", "http"), ("big", "notify"), ("small", "notify")] {
            f.nodes.push(Node::new(id, k, json!({})));
        }
        f.edges = vec![Edge::new("s", "a"), Edge::new("a", "big").when("output.n > 10"), Edge::new("a", "small").when("nodes.a.output.n <= 10")];
        let mut r = mk(f.clone());
        finish(&mut r, "s", json!({}), "out");
        finish(&mut r, "a", json!({"n": 50}), "out");
        assert_eq!((r.nodes["big"].state, r.nodes["small"].state), (NodeState::Pending, NodeState::Skipped));
        let mut r2 = mk(f);
        finish(&mut r2, "s", json!({}), "out");
        finish(&mut r2, "a", json!({"n": 3}), "out");
        assert_eq!((r2.nodes["big"].state, r2.nodes["small"].state), (NodeState::Skipped, NodeState::Pending));
    }

    #[test]
    fn failures_follow_the_error_port_only() {
        let mut f = Flow::new("f", "F");
        for (id, k) in [("s", "start"), ("a", "http"), ("ok", "notify"), ("bad", "notify")] {
            f.nodes.push(Node::new(id, k, json!({})));
        }
        f.edges = vec![Edge::new("s", "a"), Edge::new("a", "ok"), Edge::new("a", "bad").port("error")];
        let mut r = mk(f);
        finish(&mut r, "s", json!({}), "out");
        let nr = r.nodes.get_mut("a").unwrap();
        nr.state = NodeState::Failed;
        nr.error = "boom".into();
        settle(&mut r, "a", 1);
        assert_eq!((r.nodes["ok"].state, r.nodes["bad"].state), (NodeState::Skipped, NodeState::Pending));
        assert!(failed_unrouted(&r).is_empty(), "an error edge handles it");
        let ctx = ctx_for(&r, &Value::Null, &[]);
        assert_eq!(ctx["nodes"]["a"]["status"], "failed");
        assert_eq!(ctx["nodes"]["a"]["error"], "boom");
    }

    #[test]
    fn orphans_never_run_and_reset_reopens_the_tail() {
        let reg = Registry::builtin();
        let mut f = diamond();
        f.nodes.push(Node::new("orphan", "notify", json!({})));
        let mut r = mk(f);
        assert_eq!(ready(&r, &reg), vec!["s"]);
        propagate(&mut r, 1);
        assert_eq!(r.nodes["orphan"].state, NodeState::Skipped);
        finish(&mut r, "s", json!({}), "out");
        finish(&mut r, "c", json!({}), "true");
        finish(&mut r, "a", json!(1), "out");
        let ids = reset_from(&mut r, "a");
        assert_eq!(ids, vec!["a", "j", "e"]);
        assert_eq!(r.nodes["a"].state, NodeState::Pending);
        assert_eq!(ready(&r, &reg), vec!["a"], "c is still done and its edge still taken");
        assert_eq!(r.nodes["b"].state, NodeState::Skipped, "untouched");
    }

    #[test]
    fn the_context_has_inputs_nodes_and_the_item() {
        let mut r = mk(diamond());
        r.inputs = json!({"n": 2, "_item": {"id": 7}, "_index": 3});
        finish(&mut r, "s", json!({"hello": 1}), "out");
        let c = ctx_for(&r, &json!("in"), &[("x", json!(1))]);
        assert_eq!(c["inputs"]["n"], 2);
        assert_eq!(c["nodes"]["s"]["output"]["hello"], 1);
        assert_eq!(c["nodes"]["s"]["status"], "succeeded");
        assert_eq!(c["item"]["id"], 7);
        assert_eq!(c["index"], 3);
        assert_eq!(c["x"], 1);
        assert_eq!(c["run"]["id"], "r1");
        assert!(c["nodes"].get("e").is_none(), "pending nodes are not in the context");
    }
}
