#![allow(dead_code)]
//! Shared pieces of the engine tests: a temp folder, an engine over a real flow store with a fake host and a fake clock.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_flow::host::{ok_response, FakeClock, FakeHost};
use iyakki_flow::store::PublishReq;
use iyakki_flow::*;
use serde_json::{json, Value};

static N: AtomicU64 = AtomicU64::new(0);

pub struct Env {
    pub dir: PathBuf,
    pub engine: Engine,
    pub host: Arc<FakeHost>,
    pub clock: Arc<FakeClock>,
    pub store: Arc<FlowStore>,
}

pub fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("flow-it-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn env(tag: &str) -> Env {
    let dir = tmp(tag);
    env_in(dir, FakeHost::new(), FakeClock::new(1_000_000))
}

pub fn env_in(dir: PathBuf, host: Arc<FakeHost>, clock: Arc<FakeClock>) -> Env {
    let c2 = clock.clone();
    let store = Arc::new(FlowStore::new(&dir.join("flows"), Arc::new(move || c2.now_ms())));
    let engine = Engine::new(EngineCfg::new(&dir.join("engine")), host.clone(), Arc::new(Registry::builtin()), clock.clone(), store.clone());
    Env { dir, engine, host, clock, store }
}

impl Env {
    /// A second engine over the same folders (a restart).
    pub fn restart(&self) -> Env {
        env_in(self.dir.clone(), self.host.clone(), self.clock.clone())
    }
    pub fn publish(&self, f: Flow) -> u32 {
        let id = f.id.clone();
        self.store.create(f, "user:test").unwrap();
        self.store.publish(&id, &PublishReq { by: "user:test", label: "", changelog: "test", approved_by: None }).unwrap()
    }
    pub fn run(&self, id: &str) -> Run {
        let o = self.engine.start_run(StartRequest::new(id)).unwrap();
        self.finish(&o.run_id)
    }
    /// Wait until the run stops moving (terminal, blocked or scheduled).
    pub fn finish(&self, run_id: &str) -> Run {
        self.engine.wait_for(run_id, Duration::from_secs(10), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap()
    }
}

pub fn http_node(id: &str, url: &str) -> Node {
    Node::new(id, "http", json!({"url": url}))
}

/// start -> a (http) -> end
pub fn linear(id: &str) -> Flow {
    let mut f = Flow::new(id, id);
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(http_node("a", "https://api.example.com/a"));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"status": "{{ nodes.a.output.status }}"}})));
    f.edges.push(Edge::new("s", "a"));
    f.edges.push(Edge::new("a", "e"));
    f
}

pub fn ok_json(body: &str) -> Result<host::HttpResponse, NodeError> {
    Ok(ok_response(200, body))
}

pub fn status(code: u16, body: &str) -> Result<host::HttpResponse, NodeError> {
    Ok(ok_response(code, body))
}

pub fn node_state(r: &Run, id: &str) -> NodeState {
    r.nodes[id].state
}

pub fn out(r: &Run, id: &str) -> Value {
    r.nodes[id].output.clone()
}
