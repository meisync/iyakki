use std::sync::Mutex;
use std::time::Duration;

use iyakki_sec::policy::{Policy, Role};

use super::*;
use crate::flows::FlowEnv;
use crate::services::Services;

fn who() -> Who {
    Who { app: "notes".into(), user: "amy".into() }
}

// ---- dash, with a fake port

struct FakeDash {
    asked: Mutex<Vec<String>>,
    rows: usize,
}
impl DashPort for FakeDash {
    fn sources(&self, _: &str) -> Vec<SourceInfo> {
        vec![SourceInfo { name: "prom".into(), kind: "prometheus".into(), language: "PromQL".into() }, SourceInfo { name: "logs".into(), kind: "duckdb".into(), language: "SQL".into() }]
    }
    fn query(&self, user: &str, source: &str, text: &str, range: &Value, limit: usize) -> Result<(Value, Vec<Value>), String> {
        self.asked.lock().unwrap().push(format!("{user}|{source}|{text}|{range}|{limit}"));
        if text.contains("DROP") {
            return Err("this source is read only".into());
        }
        Ok((json!([{"name": "n", "type": "int"}]), (0..self.rows).map(|i| json!([i])).collect()))
    }
    fn add_panel(&self, user: &str, dashboard: &str, title: &str, source: &str, _: &str, visual: &str) -> Result<String, String> {
        self.asked.lock().unwrap().push(format!("add|{user}|{dashboard}|{title}|{source}|{visual}"));
        Ok("p7".into())
    }
}

#[test]
fn dash_lists_only_the_granted_sources_cuts_rows_to_the_limit_and_passes_a_refusal_through() {
    let port = Arc::new(FakeDash { asked: Mutex::new(vec![]), rows: 12 });
    let be = DashEngine { port: port.clone() };
    let v = be.call(&who(), "dash.sources", &json!({"names": ["prom"]})).unwrap();
    assert_eq!(v, json!([{"name": "prom", "kind": "prometheus", "language": "PromQL"}]), "the address and credentials are never part of it");
    let r = be.call(&who(), "dash.query", &json!({"source": "prom", "query": "up", "range": {"from": "now-1h", "to": "now"}, "limit": 10})).unwrap();
    assert_eq!(r["rows"].as_array().unwrap().len(), 10);
    assert_eq!(r["truncated"], true);
    assert_eq!(port.asked.lock().unwrap()[0], "amy|prom|up|{\"from\":\"now-1h\",\"to\":\"now\"}|11");
    let ok = be.call(&who(), "dash.query", &json!({"source": "prom", "query": "up", "limit": 50})).unwrap();
    assert_eq!((ok["rows"].as_array().unwrap().len(), ok["truncated"].clone()), (12, json!(false)));
    let e = be.call(&who(), "dash.query", &json!({"source": "logs", "query": "DROP TABLE t"})).unwrap_err();
    assert!(e.message.contains("read only"), "{e:?}");
    assert_eq!(be.call(&who(), "dash.panel.add", &json!({"dashboard": "Home", "panel": {"title": "CPU", "source": "prom", "query": "up", "visual": "line"}})).unwrap(), json!({"id": "p7"}));
    assert!(be.call(&who(), "dash.nope", &json!({})).is_err());
}

// ---- flows, over the real flow engine (no network: the one node fails offline, which is all these tests need)

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct World {
    port: Arc<RealFlows>,
    env: Arc<FlowEnv>,
    dir: std::path::PathBuf,
}
impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn world(tag: &str) -> World {
    let dir = std::env::temp_dir().join(format!("iyakki-sdkeng-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, "amy", &p, false, &dir.join("home")));
    let e2 = env.clone();
    let port = Arc::new(RealFlows { svc, policy: p, open: Box::new(move |_| Ok(e2.clone())) });
    World { port, env, dir }
}

fn publish(w: &World, name: &str) -> String {
    let mut f = crate::flows::build_flow(&json!({"name": name, "nodes": [{"id": "call", "kind": "http", "name": "Call", "config": {"url": "https://api.example.com/x"}}]})).unwrap();
    f.defaults.assist = Default::default();
    let id = f.id.clone();
    w.env.create_draft(f, "user:t").unwrap();
    w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    id
}

#[test]
fn flows_list_only_published_flows_by_name_and_start_status_and_cancel_by_run_id() {
    let w = world("run");
    let be = FlowsEngine { port: w.port.clone() };
    // a flow that is only a draft is not offered
    let mut draft = crate::flows::build_flow(&json!({"name": "Draft only", "nodes": [{"id": "n", "kind": "http", "name": "N", "config": {"url": "https://a.example.com"}}]})).unwrap();
    draft.defaults.assist = Default::default();
    w.env.create_draft(draft, "user:t").unwrap();
    let id = publish(&w, "Backup");
    let l = be.call(&who(), "flows.list", &json!({"names": ["Backup", "Draft only"]})).unwrap();
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert_eq!((l[0]["name"].as_str(), l[0]["version"].as_u64(), l[0]["id"].as_str()), (Some("Backup"), Some(1), Some(id.as_str())));
    assert!(be.call(&who(), "flows.run", &json!({"flow": "Draft only", "inputs": {}})).unwrap_err().message.contains("not a published flow"));
    // a card for an old version is refused once the flow moved on
    assert!(be.call(&who(), "flows.run", &json!({"flow": "Backup", "version": 5, "inputs": {}})).unwrap_err().message.contains("published again"));
    let r = be.call(&who(), "flows.run", &json!({"flow": "Backup", "version": 1, "inputs": {}, "key": "k1"})).unwrap();
    let run = r["run"].as_str().unwrap().to_string();
    let done = w.env.engine.wait_for(&run, Duration::from_secs(30), |r| r.status.is_terminal()).unwrap();
    assert_eq!(done.trigger.kind, "manual");
    let st = be.call(&who(), "flows.status", &json!({"run": run})).unwrap();
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["steps"][0]["node"], "call");
    assert!(be.call(&who(), "flows.status", &json!({"run": "nope"})).is_err());
    assert!(be.call(&who(), "flows.nope", &json!({})).is_err());
}
