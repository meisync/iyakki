//! Checks a flow before it is saved, run or published: the graph, every node by its kind, the triggers and the retry settings.

use std::collections::{BTreeMap, BTreeSet};

use crate::expr;
use crate::kinds::{Issue, KindStatus, Level, Registry};
use crate::model::*;
use crate::sched::Cron;

pub fn errors(issues: &[Issue]) -> Vec<&Issue> {
    issues.iter().filter(|i| i.level == Level::Error).collect()
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.level == Level::Error)
}

fn gerr(msg: impl Into<String>) -> Issue {
    Issue { level: Level::Error, node: None, msg: msg.into() }
}

fn gwarn(msg: impl Into<String>) -> Issue {
    Issue { level: Level::Warn, node: None, msg: msg.into() }
}

/// Nodes in a topological order, or `None` when there is a cycle.
pub fn topo_order(flow: &Flow) -> Option<Vec<String>> {
    let mut indeg: BTreeMap<&str, usize> = flow.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    for e in &flow.edges {
        if indeg.contains_key(e.from.as_str()) {
            if let Some(d) = indeg.get_mut(e.to.as_str()) {
                *d += 1;
            }
        }
    }
    let mut order = Vec::new();
    let mut ready: Vec<&str> = flow.nodes.iter().map(|n| n.id.as_str()).filter(|id| indeg[id] == 0).collect();
    while let Some(n) = ready.first().copied() {
        ready.remove(0);
        order.push(n.to_string());
        for e in flow.edges.iter().filter(|e| e.from == n) {
            if let Some(d) = indeg.get_mut(e.to.as_str()) {
                *d -= 1;
                if *d == 0 {
                    ready.push(e.to.as_str());
                }
            }
        }
    }
    (order.len() == flow.nodes.len()).then_some(order)
}

/// The ids of every node that runs before `id` on some path.
pub fn ancestors(flow: &Flow, id: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![id.to_string()];
    while let Some(n) = stack.pop() {
        for e in flow.edges.iter().filter(|e| e.to == n) {
            if seen.insert(e.from.clone()) {
                stack.push(e.from.clone());
            }
        }
    }
    seen
}

pub fn validate_flow(flow: &Flow, reg: &Registry) -> Vec<Issue> {
    let mut v = Vec::new();
    if !valid_id(&flow.id) {
        v.push(gerr("the flow id is letters, digits, - and _ (at most 48)"));
    }
    if flow.name.trim().is_empty() {
        v.push(gerr("the flow needs a name"));
    }
    if flow.nodes.len() > 300 {
        v.push(gerr("a flow has at most 300 nodes"));
    }
    let mut ids = BTreeSet::new();
    for n in &flow.nodes {
        if !valid_id(&n.id) {
            v.push(Issue::error(&n.id, "the node id is letters, digits, - and _"));
        }
        if !ids.insert(n.id.clone()) {
            v.push(Issue::error(&n.id, "two nodes have this id"));
        }
        match reg.get(&n.kind) {
            None => v.push(Issue::error(&n.id, format!("`{}` is not a node kind", n.kind))),
            Some(k) => {
                v.extend(k.validate(n, flow));
                if let KindStatus::Planned(why) = k.status() {
                    if k.validate(n, flow).is_empty() {
                        v.push(Issue::error(&n.id, format!("{} is planned: {why}", k.label())));
                    }
                }
                if flow.runner_of(n) == RunnerKind::K3sJob && !k.sandboxable() {
                    v.push(Issue::error(&n.id, format!("a {} node cannot run in a sandbox Job (only command and gRPC nodes can)", k.label())));
                }
                if flow.runner_of(n) == RunnerKind::Remote {
                    v.push(Issue::warn(&n.id, "the remote runner is not built: this node will fail until one is configured"));
                }
            }
        }
        if let Some(r) = &n.retry {
            for m in r.validate() {
                v.push(Issue::error(&n.id, format!("retry: {m}")));
            }
        }
        if n.timeout_ms == Some(0) {
            v.push(Issue::error(&n.id, "a timeout of 0 would stop the step at once"));
        }
    }
    for m in flow.defaults.retry.validate() {
        v.push(gerr(format!("default retry: {m}")));
    }
    if flow.defaults.max_parallel == 0 || flow.defaults.max_parallel > 32 {
        v.push(gerr("max_parallel is 1 to 32"));
    }
    let starts = flow.nodes.iter().filter(|n| n.kind == "start").count();
    if starts == 0 {
        v.push(gerr("a flow needs a start node"));
    } else if starts > 1 {
        v.push(gwarn("more than one start node: every one begins the run"));
    }
    let mut seen_edges = BTreeSet::new();
    for e in &flow.edges {
        let (Some(from), Some(to)) = (flow.node(&e.from), flow.node(&e.to)) else {
            v.push(gerr(format!("the edge {} -> {} points at a node that does not exist", e.from, e.to)));
            continue;
        };
        if e.from == e.to {
            v.push(gerr(format!("`{}` has an edge to itself", e.from)));
        }
        if !seen_edges.insert(e.key()) {
            v.push(gerr(format!("two identical edges {} -> {}", e.from, e.to)));
        }
        if let Some(k) = reg.get(&from.kind) {
            let ports = k.ports(from);
            if !ports.outputs.contains(&e.port) && !(e.port == "error" && k.can_fail()) {
                v.push(Issue::error(&from.id, format!("it has no output port `{}` (it has: {})", e.port, ports.outputs.join(", "))));
            }
        }
        if let Some(k) = reg.get(&to.kind) {
            if k.ports(to).inputs.is_empty() {
                v.push(Issue::error(&to.id, "a start node cannot take an edge"));
            }
        }
        if !e.when.trim().is_empty() {
            if let Err(err) = expr::parse(&e.when) {
                v.push(Issue::error(&e.from, format!("the condition on the edge to `{}`: {err}", e.to)));
            }
        }
    }
    let acyclic = topo_order(flow).is_some();
    if !acyclic {
        v.push(gerr("the flow has a cycle: a flow must be a graph without loops (use a for-each node to repeat)"));
    }
    if acyclic && starts > 0 {
        let mut reach: BTreeSet<String> = flow.nodes.iter().filter(|n| n.kind == "start").map(|n| n.id.clone()).collect();
        let mut stack: Vec<String> = reach.iter().cloned().collect();
        while let Some(n) = stack.pop() {
            for e in flow.outgoing(&n) {
                if reach.insert(e.to.clone()) {
                    stack.push(e.to.clone());
                }
            }
        }
        for n in &flow.nodes {
            if !reach.contains(&n.id) {
                v.push(Issue::warn(&n.id, "nothing leads here from a start node: it will never run"));
            }
        }
        // a node reading another that does not run before it
        for n in &flow.nodes {
            let anc = ancestors(flow, &n.id);
            for r in expr::node_refs(&n.config) {
                let id = r.split('.').next().unwrap_or("");
                if id != n.id && flow.node(id).is_some() && !anc.contains(id) {
                    v.push(Issue::warn(&n.id, format!("it reads `{id}`, which does not run before it")));
                }
                if flow.node(id).is_none() {
                    v.push(Issue::warn(&n.id, format!("it reads `{id}`, which is not a node of this flow")));
                }
            }
        }
    }
    for t in &flow.triggers {
        if !valid_id(&t.id) {
            v.push(gerr(format!("trigger id `{}` is letters, digits, - and _", t.id)));
        }
        match &t.kind {
            TriggerKind::Cron { expr, tz } => {
                if let Err(e) = Cron::parse(expr) {
                    v.push(gerr(format!("trigger {}: {e}", t.id)));
                }
                if crate::sched::basic_tz(tz).is_none() && !tz.contains('/') && !tz.eq_ignore_ascii_case("local") {
                    v.push(gerr(format!("trigger {}: `{tz}` is not a time zone (local, UTC, +05:30 or an IANA name)", t.id)));
                }
            }
            TriggerKind::Interval { every_s, .. } if *every_s < 5 => v.push(gerr(format!("trigger {}: an interval is at least 5 seconds", t.id))),
            TriggerKind::Webhook { .. } => v.push(gwarn(format!("trigger {}: webhooks are planned and will not fire yet", t.id))),
            TriggerKind::OnFailure { flow: f } if *f == flow.id => v.push(gerr(format!("trigger {}: a flow cannot start on its own failure", t.id))),
            _ => {}
        }
    }
    if flow.production && flow.defaults.heal.mode == HealMode::AutoFixSimple {
        v.push(gwarn("this production flow asks for automatic fixes, but the harness always asks first on production"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn linear() -> Flow {
        let mut f = Flow::new("f", "F");
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("a", "http", json!({"url": "https://x.example/a"})));
        f.nodes.push(Node::new("e", "end", json!({"outputs": {"v": "{{ nodes.a.output.status }}"}})));
        f.edges.push(Edge::new("s", "a"));
        f.edges.push(Edge::new("a", "e"));
        f
    }

    fn msgs(f: &Flow) -> Vec<String> {
        validate_flow(f, &Registry::builtin()).into_iter().map(|i| format!("{:?}:{}:{}", i.level, i.node.unwrap_or_default(), i.msg)).collect()
    }

    #[test]
    fn a_good_flow_is_clean() {
        assert!(msgs(&linear()).is_empty(), "{:?}", msgs(&linear()));
        assert_eq!(topo_order(&linear()).unwrap(), vec!["s", "a", "e"]);
        assert_eq!(ancestors(&linear(), "e").len(), 2);
    }

    #[test]
    fn the_graph_is_checked() {
        let mut f = linear();
        f.edges.push(Edge::new("e", "a"));
        assert!(msgs(&f).iter().any(|m| m.contains("cycle")), "{:?}", msgs(&f));
        assert!(msgs(&f).iter().any(|m| m.contains("end node has no outgoing")));
        let mut g = linear();
        g.edges.push(Edge::new("a", "ghost"));
        g.edges.push(Edge::new("a", "e"));
        let m = msgs(&g);
        assert!(m.iter().any(|x| x.contains("does not exist")) && m.iter().any(|x| x.contains("identical")), "{m:?}");
        let mut h = linear();
        h.nodes.push(Node::new("lost", "notify", json!({"title": "x"})));
        assert!(msgs(&h).iter().any(|m| m.contains("lost") && m.contains("never run")));
        let mut i = linear();
        i.nodes[0].kind = "http".into();
        assert!(msgs(&i).iter().any(|m| m.contains("needs a start")));
        let mut j = linear();
        j.edges[1].port = "true".into();
        assert!(msgs(&j).iter().any(|m| m.contains("no output port")));
        let mut k = linear();
        k.edges[1] = Edge::new("a", "e").port("error");
        assert!(msgs(&k).iter().all(|m| !m.contains("no output port")), "an error port exists on every node that can fail");
        let mut l = linear();
        l.edges[1] = Edge::new("a", "e").when("1 +");
        assert!(msgs(&l).iter().any(|m| m.contains("condition on the edge")));
        let mut m2 = linear();
        m2.nodes[1].id = "bad id".into();
        assert!(msgs(&m2).iter().any(|m| m.contains("letters, digits")));
        let mut n = linear();
        n.nodes.push(Node::new("a", "notify", json!({"title": "x"})));
        assert!(msgs(&n).iter().any(|m| m.contains("two nodes")));
    }

    #[test]
    fn nodes_are_checked_by_their_kind_and_runner() {
        let mut f = linear();
        f.nodes[1].config = json!({"url": "ftp://x"});
        assert!(msgs(&f).iter().any(|m| m.contains("http://")));
        let mut g = linear();
        g.nodes[1].runner = Some(RunnerKind::K3sJob);
        assert!(msgs(&g).iter().any(|m| m.contains("sandbox")));
        let mut h = linear();
        h.nodes[1] = Node::new("a", "cli", json!({"command": ["echo", "x"]}));
        h.nodes[1].runner = Some(RunnerKind::K3sJob);
        assert!(!msgs(&h).iter().any(|m| m.contains("sandbox")));
        let mut t = linear();
        t.nodes[1] = Node::new("a", "thrift", json!({"endpoint": "x:1", "service": "S", "method": "m"}));
        assert!(msgs(&t).iter().any(|m| m.contains("planned")));
        let mut r = linear();
        r.nodes[1].retry = Some(crate::retry::RetryPolicy::exponential(3, 100, 0.2, 10, 3.0));
        assert!(msgs(&r).iter().filter(|m| m.contains("retry:")).count() >= 2);
        let mut ghost = linear();
        ghost.nodes[2].config = json!({"outputs": {"v": "{{ nodes.zzz.output.q }}"}});
        assert!(msgs(&ghost).iter().any(|m| m.contains("not a node of this flow")));
        let mut order = linear();
        order.nodes[1].config = json!({"url": "https://x.example/{{ nodes.e.output.v }}"});
        assert!(msgs(&order).iter().any(|m| m.contains("does not run before")));
        let mut cred = linear();
        cred.nodes[1].config = json!({"url": "https://x.example", "headers": {"Authorization": "Bearer abc"}});
        assert!(msgs(&cred).iter().any(|m| m.contains("{{secret.NAME}}") || m.contains("credential")));
        let mut ok = linear();
        ok.nodes[1].config = json!({"url": "https://x.example", "headers": {"Authorization": "Bearer {{secret.tok}}"}});
        assert!(!msgs(&ok).iter().any(|m| m.contains("credential")));
    }

    #[test]
    fn triggers_are_checked() {
        let mut f = linear();
        f.triggers.push(Trigger::new("c", TriggerKind::Cron { expr: "61 * * * *".into(), tz: "local".into() }));
        f.triggers.push(Trigger::new("i", TriggerKind::Interval { every_s: 1, anchor_ms: None }));
        f.triggers.push(Trigger::new("w", TriggerKind::Webhook { path: "/x".into() }));
        f.triggers.push(Trigger::new("z", TriggerKind::Cron { expr: "* * * * *".into(), tz: "Mars".into() }));
        let m = msgs(&f);
        assert!(m.iter().any(|x| x.contains("minute")) && m.iter().any(|x| x.contains("at least 5")) && m.iter().any(|x| x.contains("planned")) && m.iter().any(|x| x.contains("time zone")), "{m:?}");
        let mut g = linear();
        g.triggers.push(Trigger::new("of", TriggerKind::OnFailure { flow: "f".into() }));
        assert!(msgs(&g).iter().any(|m| m.contains("its own failure")));
    }
}
