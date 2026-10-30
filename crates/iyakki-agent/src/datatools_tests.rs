//! The data tools through the real run loop: a scripted model asks for them, the person is asked before an export, and the
//! tables come back under the caps.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_sec::policy::Role;
use serde_json::{json, Value};

use crate::agent::{start, RunEvent, RunLimits, UserTools, Verdict};
use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
use crate::services::Services;

struct Dead;
impl Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Script {
    turns: Mutex<VecDeque<ModelTurn>>,
    offered: Mutex<Vec<Vec<String>>>,
}

impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("unused".into())
    }
    fn complete_tools(&self, _: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.offered.lock().unwrap().push(t.iter().map(|s| s.name.clone()).collect());
        self.turns.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
    }
}

fn call(id: &str, name: &str, args: Value) -> ModelTurn {
    ModelTurn { text: String::new(), calls: vec![ToolCall { id: id.into(), name: name.into(), args }] }
}

fn say(t: &str) -> ModelTurn {
    ModelTurn { text: t.into(), calls: vec![] }
}

fn run(script: Vec<ModelTurn>, dir: &std::path::Path, ask: &str, approve: bool) -> (Vec<RunEvent>, Vec<Vec<String>>) {
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let mut policy = Role::User.template();
    policy.fs_read = vec![dir.display().to_string()];
    policy.fs_write = vec![dir.display().to_string()];
    let tools = Arc::new(UserTools::new(&format!("alice-{}", dir.file_name().unwrap().to_string_lossy()), policy, svc, Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(dir));
    let backend = Arc::new(Script { turns: Mutex::new(script.into()), offered: Mutex::new(vec![]) });
    let h = start(backend.clone(), tools, vec![Message::system("SYS"), Message::user(ask)], RunLimits::default());
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(e) = h.events.recv_timeout(Duration::from_millis(50)) {
            if let RunEvent::Approval { id, .. } = &e {
                h.decide(id, if approve { Verdict::Approve } else { Verdict::Deny });
            }
            let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
            out.push(e);
            if done {
                break;
            }
        }
    }
    let offered = backend.offered.lock().unwrap().clone();
    (out, offered)
}

fn results(e: &[RunEvent]) -> Vec<(String, bool, String)> {
    e.iter().filter_map(|x| if let RunEvent::ToolResult { name, ok, text, .. } = x { Some((name.clone(), *ok, text.clone())) } else { None }).collect()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-datarun-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_model_opens_queries_and_exports_a_csv_with_the_person_asked_first() {
    let d = tmp("flow");
    std::fs::write(d.join("orders.csv"), "id,customer,total\n1,Ann,12.5\n2,Bob,40\n3,Ann,7.25\n4,Cy,99\n").unwrap();
    let (ev, offered) = run(
        vec![
            call("c1", "data_open", json!({"path": "orders.csv"})),
            call("c2", "data_query", json!({"handle": "d1", "ops": [{"op": "group_by", "by": ["customer"], "aggs": [{"column": "total", "func": "sum", "as": "spent"}]}, {"op": "sort", "keys": [{"column": "spent", "dir": "desc"}]}], "limit": 2})),
            call("c3", "data_export", json!({"handle": "d1", "filter": "total > 10", "path": "big.csv"})),
            say("Cy spent the most."),
        ],
        &d,
        "which customer spent most? save the big orders",
        true,
    );
    let r = results(&ev);
    assert_eq!(r.len(), 3, "{ev:?}");
    assert!(r[0].1 && r[0].2.contains("4 rows, 3 columns") && r[0].2.contains("customer (str)"), "{}", r[0].2);
    assert!(r[1].1 && r[1].2.contains("| Cy | 99.0 |") && r[1].2.contains("Rows 1-2 of 3"), "{}", r[1].2);
    assert!(r[2].1 && r[2].2.contains("Wrote 3 rows"), "{}", r[2].2);
    assert!(std::fs::read_to_string(d.join("big.csv")).unwrap().contains("Bob,40"));
    // reads are safe (no approval); the export asked
    let asked: Vec<&str> = ev.iter().filter_map(|e| if let RunEvent::Approval { name, .. } = e { Some(name.as_str()) } else { None }).collect();
    assert_eq!(asked, vec!["data_export"], "{ev:?}");
    for n in ["data_open", "data_describe", "data_query", "data_export"] {
        assert!(offered[0].iter().any(|t| t == n), "{n} offered: {:?}", offered[0]);
    }
}

#[test]
fn a_refused_export_writes_nothing() {
    let d = tmp("deny");
    std::fs::write(d.join("t.csv"), "a\n1\n2\n").unwrap();
    let (ev, _) = run(vec![call("c1", "data_open", json!({"path": "t.csv"})), call("c2", "data_export", json!({"handle": "d1", "path": "copy.csv"})), say("ok")], &d, "copy it", false);
    assert!(!d.join("copy.csv").exists(), "{ev:?}");
    assert!(results(&ev).iter().any(|r| r.0 == "data_export" && !r.1), "{ev:?}");
}

#[test]
fn the_file_tools_reach_a_shared_host_folder_only_within_its_mode_and_only_while_it_is_shared() {
    use crate::agent::Tools;
    let _g = crate::hostfiles::test_guard();
    let d = tmp("hostfiles");
    let (host, own) = (d.join("host"), d.join("own"));
    std::fs::create_dir_all(host.join("Documents/shared")).unwrap();
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(host.join("Documents/shared/n.txt"), "from the host").unwrap();
    std::fs::write(host.join("Documents/private.txt"), "not shared").unwrap();
    crate::hostfiles::set_enabled(true);
    crate::hostfiles::set_host_home(Some(host.clone()));
    let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let mut policy = Role::User.template();
    policy.fs_read = vec![own.display().to_string()];
    policy.fs_write = vec![own.display().to_string()];
    let tools = UserTools::new("amy-hostfiles", policy, svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(&own);
    // nothing before the grant
    assert!(tools.call("read_file", &json!({"path": "host:shared/n.txt"})).is_err());
    assert!(tools.call("read_file", &json!({"path": host.join("Documents/shared/n.txt").to_str().unwrap()})).is_err());
    let mut m = crate::hostfiles::HostMounts::load(svc.dir(), "amy-hostfiles");
    m.add(&std::fs::canonicalize(&host).unwrap(), "~/Documents/shared", None, false).unwrap();
    assert_eq!(tools.call("read_file", &json!({"path": "host:shared/n.txt"})).unwrap(), "from the host");
    assert!(tools.call("list_dir", &json!({"path": "host:shared"})).unwrap().contains("n.txt"));
    assert!(tools.call("read_file", &json!({"path": host.join("Documents/private.txt").to_str().unwrap()})).is_err(), "a neighbour is not shared");
    assert!(tools.call("read_file", &json!({"path": "host:shared/../private.txt"})).is_err(), "no way out of the share");
    // read-only until writing is turned on
    let e = tools.call("write_file", &json!({"path": "host:shared/out.txt", "content": "x"})).unwrap_err();
    assert!(e.contains("read-only"), "{e}");
    assert!(!host.join("Documents/shared/out.txt").exists());
    m.set_write("shared", true).unwrap();
    tools.call("write_file", &json!({"path": "host:shared/out.txt", "content": "x"})).unwrap();
    assert_eq!(std::fs::read_to_string(host.join("Documents/shared/out.txt")).unwrap(), "x");
    // and not after it is removed
    m.remove("shared").unwrap();
    assert!(tools.call("read_file", &json!({"path": "host:shared/n.txt"})).is_err());
    assert!(tools.call("write_file", &json!({"path": host.join("Documents/shared/again.txt").to_str().unwrap(), "content": "x"})).is_err());
    crate::hostfiles::set_host_home(None);
    crate::hostfiles::set_enabled(false);
}

#[test]
fn the_model_creates_a_csv_of_five_rows_after_approval_then_queries_it() {
    let d = tmp("create");
    let rows: Vec<Value> = (1..=5).map(|i| json!([i, format!("item {i}"), i * 10])).collect();
    let (ev, offered) = run(
        vec![
            call("c1", "data_create", json!({"name": "items.csv", "folder": d.display().to_string(), "columns": ["id", "item", "qty"], "rows": rows})),
            call("c2", "data_query", json!({"handle": "d1", "filter": "qty >= 30", "sort_by": "qty", "descending": true})),
            say("Three items have 30 or more."),
        ],
        &d,
        "make a table of 5 items and show the big ones",
        true,
    );
    let r = results(&ev);
    assert_eq!(r.len(), 2, "{ev:?}");
    assert!(r[0].1 && r[0].2.contains("5 rows, 3 columns") && r[0].2.contains("Opened as d1"), "{}", r[0].2);
    assert!(r[1].1 && r[1].2.contains("Rows 1-3 of 3") && r[1].2.contains("item 5"), "{}", r[1].2);
    let asked: Vec<(&str, &str)> = ev.iter().filter_map(|e| if let RunEvent::Approval { name, summary, .. } = e { Some((name.as_str(), summary.as_str())) } else { None }).collect();
    assert_eq!(asked.len(), 1, "{ev:?}");
    assert!(asked[0].1.contains("items.csv") && asked[0].1.contains("id, item, qty") && asked[0].1.contains("5 rows") && !asked[0].1.contains("item 3"), "{}", asked[0].1);
    assert!(offered[0].iter().any(|t| t == "data_create") && offered[0].iter().any(|t| t == "data_append"));
    assert!(std::fs::read_to_string(d.join("items.csv")).unwrap().starts_with("id,item,qty\n1,item 1,10\n"));
}

#[test]
fn a_refused_create_writes_nothing() {
    let d = tmp("create-deny");
    let (ev, _) = run(vec![call("c1", "data_create", json!({"name": "x.csv", "folder": d.display().to_string(), "columns": ["a"], "rows": [[1]]})), say("ok")], &d, "make it", false);
    assert!(!d.join("x.csv").exists(), "{ev:?}");
}
