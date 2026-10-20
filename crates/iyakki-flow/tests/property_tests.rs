//! Property tests with a small deterministic generator (no external crate): random graphs, random choices, and the rules that must
//! hold for all of them.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use iyakki_flow::graph;
use iyakki_flow::*;
use serde_json::{json, Value};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

/// A random acyclic flow: node 0 is the start, every other node has at least one edge from an earlier node; some nodes are
/// conditions (ports true/false), some are joins. Returns the flow and, for each node, the edges that came in.
fn random_flow(rng: &mut Rng, n: usize) -> Flow {
    let mut f = Flow::new("rf", "Random");
    f.defaults.max_parallel = 1 + rng.below(4);
    f.nodes.push(Node::new("n0", "start", json!({})));
    let mut kinds = vec!["start".to_string()];
    for i in 1..n {
        let kind = match rng.below(5) {
            0 => "condition",
            1 => "join",
            _ => "notify",
        };
        let cfg = match kind {
            "condition" => json!({"expr": if rng.chance(50) { "true" } else { "false" }}),
            "join" => json!({"mode": if rng.chance(50) { "all" } else { "any" }}),
            _ => json!({"title": format!("n{i}")}),
        };
        f.nodes.push(Node::new(&format!("n{i}"), kind, cfg));
        kinds.push(kind.to_string());
        let parents = 1 + rng.below(3.min(i));
        let mut seen = BTreeSet::new();
        for _ in 0..parents {
            let p = rng.below(i);
            if !seen.insert(p) {
                continue;
            }
            let port = if kinds[p] == "condition" { if rng.chance(50) { "true" } else { "false" } } else { "out" };
            f.edges.push(Edge::new(&format!("n{p}"), &format!("n{i}")).port(port));
        }
    }
    f
}

/// What should happen, worked out by the simplest possible reading of the rules: a node runs when every incoming edge is decided
/// and at least one is taken (a join with `all` needs every one); an edge is taken when its source ran and chose its port.
fn expected(f: &Flow) -> BTreeMap<String, bool> {
    let mut ran: BTreeMap<String, bool> = BTreeMap::new();
    let mut port: BTreeMap<String, String> = BTreeMap::new();
    for n in &f.nodes {
        let incoming: Vec<&Edge> = f.incoming(&n.id);
        let r = if incoming.is_empty() {
            n.kind == "start"
        } else {
            let taken = incoming.iter().filter(|e| ran[&e.from] && port[&e.from] == e.port).count();
            if n.kind == "join" && n.config["mode"] == "all" { taken == incoming.len() } else { taken > 0 }
        };
        ran.insert(n.id.clone(), r);
        port.insert(n.id.clone(), if n.kind == "condition" { if n.config["expr"] == "true" { "true".into() } else { "false".into() } } else { "out".into() });
    }
    ran
}

#[test]
fn random_graphs_run_exactly_the_nodes_the_rules_say_in_a_valid_order() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for round in 0..60 {
        let n = 3 + rng.below(12);
        let mut f = random_flow(&mut rng, n);
        f.id = format!("rf{round}");
        let issues = validate::validate_flow(&f, &Registry::builtin());
        let blocking: Vec<_> = validate::errors(&issues).iter().map(|i| i.msg.clone()).collect();
        assert!(blocking.is_empty(), "{blocking:?}\n{}", f.to_json());
        let e = env("prop");
        // the order in which notify nodes were executed
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let _ = &order;
        e.publish(f.clone());
        let r = e.run(&f.id);
        assert_eq!(r.status, RunStatus::Succeeded, "round {round}: {}\n{}", r.error, f.to_json());
        let want = expected(&f);
        for node in &f.nodes {
            let st = r.nodes[&node.id].state;
            // 1. every node ends up in a final state: done or skipped, never left behind
            assert!(matches!(st, NodeState::Succeeded | NodeState::Skipped), "round {round}: {} is {st:?}", node.id);
            // 2. it ran exactly when the rules say
            assert_eq!(st == NodeState::Succeeded, want[&node.id], "round {round}: node {} ({}), flow:\n{}", node.id, node.kind, f.to_json());
        }
        // 3. a node never started before everything feeding it had ended
        for edge in &f.edges {
            let (a, b) = (&r.nodes[&edge.from], &r.nodes[&edge.to]);
            if a.state == NodeState::Succeeded && b.state == NodeState::Succeeded && a.port == edge.port {
                assert!(a.ended_ms <= b.started_ms, "round {round}: {} started at {} before {} ended at {}", edge.to, b.started_ms, edge.from, a.ended_ms);
            }
        }
        // 4. a node that ran has at least one parent that ran (the start excepted)
        for node in f.nodes.iter().skip(1) {
            if r.nodes[&node.id].state == NodeState::Succeeded {
                assert!(f.incoming(&node.id).iter().any(|ed| r.nodes[&ed.from].state == NodeState::Succeeded), "round {round}: {} ran without a parent", node.id);
            }
        }
        let _ = std::fs::remove_dir_all(&e.dir);
    }
}

#[test]
fn notify_order_is_always_a_topological_order_even_with_parallel_branches() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for round in 0..25 {
        let n = 6 + rng.below(8);
        let mut f = Flow::new(&format!("tp{round}"), "TP");
        f.defaults.max_parallel = 1 + rng.below(4);
        f.nodes.push(Node::new("n0", "start", json!({})));
        for i in 1..n {
            f.nodes.push(Node::new(&format!("n{i}"), "notify", json!({"title": format!("n{i}")})));
            let mut seen = BTreeSet::new();
            for _ in 0..(1 + rng.below(2)) {
                let p = rng.below(i);
                if seen.insert(p) {
                    f.edges.push(Edge::new(&format!("n{p}"), &format!("n{i}")));
                }
            }
        }
        let e = env("prop2");
        e.publish(f.clone());
        let r = e.run(&f.id);
        assert_eq!(r.status, RunStatus::Succeeded);
        let titles: Vec<String> = e.host.notices.lock().unwrap().iter().map(|n| n.title.clone()).collect();
        let pos: BTreeMap<&str, usize> = titles.iter().enumerate().map(|(i, t)| (t.as_str(), i)).collect();
        assert_eq!(titles.len(), n - 1, "every notify node ran once");
        for edge in &f.edges {
            if edge.from == "n0" {
                continue;
            }
            assert!(pos[edge.from.as_str()] < pos[edge.to.as_str()], "round {round}: {} ran after {}", edge.from, edge.to);
        }
        let _ = std::fs::remove_dir_all(&e.dir);
    }
}

#[test]
fn the_pure_graph_functions_never_leave_a_node_pending_once_everything_ended() {
    let mut rng = Rng(42);
    let reg = Registry::builtin();
    for _ in 0..300 {
        let n = 2 + rng.below(15);
        let f = random_flow(&mut rng, n);
        let mut run = iyakki_flow::engine::blank_run("r", f, json!({}), TriggerInfo::default(), 0);
        graph::propagate(&mut run, 1);
        let mut guard = 0;
        loop {
            let ready = graph::ready(&run, &reg);
            if ready.is_empty() {
                break;
            }
            // finish them in a random order, a condition choosing a random port
            let pick = ready[rng.below(ready.len())].clone();
            let kind = run.flow.node(&pick).unwrap().kind.clone();
            let port = if kind == "condition" { if rng.chance(50) { "true" } else { "false" } } else { "out" };
            let nr = run.nodes.get_mut(&pick).unwrap();
            nr.state = NodeState::Succeeded;
            nr.port = port.into();
            nr.output = Value::Null;
            graph::settle(&mut run, &pick, 1);
            guard += 1;
            assert!(guard < 100);
        }
        assert!(run.nodes.values().all(|n| n.state.is_done()), "something was left pending:\n{}", run.flow.to_json());
        let open: Vec<_> = run.flow.edges.iter().filter(|e| run.nodes[&e.from].state.is_done() && run.edges[&e.key()] == EdgeState::Open).collect();
        assert!(open.is_empty(), "an edge from a finished node is still open: {open:?}");
    }
}

#[test]
fn templates_never_evaluate_data_and_never_panic_on_random_input() {
    let mut rng = Rng(7);
    let ctx = json!({"inputs": {"a": "{{ 1 + 1 }}", "n": 3, "list": [1, 2, 3]}, "nodes": {"x": {"output": {"v": "{{ nodes.x.output.v }}"}}}});
    let alphabet: Vec<&str> = vec!["{{", "}}", " ", "inputs.a", "inputs.n", "nodes.x.output.v", "+", "-", "(", ")", "[", "]", "'", "\"", "len(", "1", "x", ".", ",", "&&", "!", "secret.", "default(", "in", "=="];
    for _ in 0..3000 {
        let len = 1 + rng.below(12);
        let s: String = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        // must return, never panic; whatever comes back is plain data
        if let Ok(v) = expr::render(&s, &ctx, None) {
            if let Value::String(t) = &v {
                // a value inserted from the context is never evaluated again
                assert!(!(t == "2" && s.contains("inputs.a") && !s.contains("1 + 1")), "{s}");
            }
        }
        let _ = expr::eval_str(&s, &ctx);
    }
    assert_eq!(expr::render("{{ inputs.a }}", &ctx, None).unwrap(), json!("{{ 1 + 1 }}"));
    assert_eq!(expr::render("{{ nodes.x.output.v }}", &ctx, None).unwrap(), json!("{{ nodes.x.output.v }}"));
}

#[test]
fn backoff_delays_stay_in_bounds_for_random_policies() {
    let mut rng = Rng(99);
    for _ in 0..2000 {
        let base = 1 + rng.below(5000) as u64;
        let factor = 1.0 + (rng.below(300) as f64) / 100.0;
        let max = base + rng.below(60_000) as u64;
        let jitter = (rng.below(100) as f64) / 100.0;
        let p = RetryPolicy::exponential(1 + rng.below(10) as u32, base, factor, max, jitter);
        assert!(p.validate().is_empty());
        let seed = rng.next();
        let mut last_nominal = 0.0;
        for n in 2..=p.total_attempts().max(2) {
            let nominal = (base as f64 * factor.powi(n as i32 - 2)).min(max as f64);
            let d = p.delay_before(n, None, seed) as f64;
            assert!(d >= nominal * (1.0 - jitter) - 1.0 && d <= nominal * (1.0 + jitter) + 1.0, "n={n} d={d} nominal={nominal} jitter={jitter}");
            assert!(nominal >= last_nominal - 1e-6, "the nominal delay never shrinks");
            last_nominal = nominal;
            assert!(d <= retry::MAX_DELAY_MS as f64);
        }
        assert_eq!(p.delay_before(1, None, seed), 0);
    }
    let _ = Duration::from_secs(1);
}
