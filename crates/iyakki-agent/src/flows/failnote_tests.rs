// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! A failed run reaches the INBOX once, with no input value, unless the offer is off for the flow or in general. The network is dead,
//! so the run fails for real in the engine; no model is involved.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iyakki_flow::{RunStatus, StartRequest};
use iyakki_sec::policy::{Policy, Role};
use serde_json::json;

use super::failfix::FailBook;
use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct W {
    env: Arc<FlowEnv>,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn world(tag: &str) -> W {
    let dir = std::env::temp_dir().join(format!("iyakki-failnote-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("failnotetester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &dir.join("home")));
    W { env, dir }
}

fn failing_run(w: &W, name: &str, secret: &str) -> String {
    let mut f = build_flow(&json!({"name": name, "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "https://api.example.com/old"}}]})).unwrap();
    let id = f.id.clone();
    f.defaults.assist = Default::default();
    if w.env.store.get_draft(&id).is_none() {
        w.env.create_draft(f, "user:t").unwrap();
        w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    }
    let o = w.env.engine.start_run(StartRequest::new(&id).inputs(json!({"api": secret}))).unwrap();
    let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(30), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    o.run_id
}

fn notes(w: &W) -> Vec<crate::inbox::Notification> {
    w.env.svc.inbox.list(&w.env.user).into_iter().filter(|n| n.title.starts_with("Flow failed")).collect()
}

#[test]
fn a_failed_run_makes_one_inbox_note_without_input_values() {
    let w = world("once");
    let secret = "tok_Zq8Wm3Xk9Vn2Bd7L";
    let run = failing_run(&w, "Watch", secret);
    let lines = w.env.notify_new_failures();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let n = notes(&w);
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].title, "Flow failed: Watch");
    assert!(n[0].body.contains("Run failed") && n[0].body.contains("Open FLOWS") && !n[0].body.contains(secret), "{}", n[0].body);
    assert_eq!(n[0].link.as_ref().map(|l| (l.app.as_str(), l.id.as_str())), Some(("flows", run.as_str())));
    // the same failure is noted once, also after a restart of the pass
    assert!(w.env.notify_new_failures().is_empty());
    assert_eq!(notes(&w).len(), 1);
}

#[test]
fn the_offer_off_for_a_flow_or_in_general_means_no_inbox_note() {
    let w = world("off");
    let mut book = FailBook::open(&w.env.data);
    book.set_offer_for("watch", false);
    failing_run(&w, "Watch", "x");
    assert!(w.env.notify_new_failures().is_empty() && notes(&w).is_empty(), "off for this flow");
    // another flow is still noted
    failing_run(&w, "Other", "x");
    assert_eq!(w.env.notify_new_failures().len(), 1);
    // the general switch off silences everything, and turning the flow on again notes its next failure
    book.set_offer(false);
    failing_run(&w, "Other", "x");
    assert!(w.env.notify_new_failures().is_empty());
    book.set_offer(true);
    book.set_offer_for("watch", true);
    failing_run(&w, "Watch", "x");
    assert_eq!(w.env.notify_new_failures().len(), 2, "Watch, and the Other failure that was held back while the switch was off");
}

#[test]
fn a_success_after_a_failure_is_not_noted_and_old_failures_are_left_alone() {
    let w = world("old");
    failing_run(&w, "Watch", "x");
    // the pass runs after a later success: the newest run decides
    let ok = build_flow(&json!({"name": "Fine", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hello"}}]})).unwrap();
    let id = ok.id.clone();
    w.env.create_draft(ok, "user:t").unwrap();
    w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let o = w.env.engine.start_run(StartRequest::new(&id)).unwrap();
    let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(30), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(w.env.notify_new_failures().len(), 1, "only Watch, which failed; Fine succeeded");
}
