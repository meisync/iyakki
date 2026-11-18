use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{hard_ask, Risk, Tools};
use crate::ollamatools::*;
use crate::quickinstall::ollama_cluster::{HostInfo, Mode, Where};

struct Fake {
    runs: Where,
    host: HostInfo,
    started: Mutex<Vec<Option<Mode>>>,
}

impl Backend for Fake {
    fn where_runs(&self) -> Where {
        self.runs.clone()
    }
    fn host_info(&self) -> HostInfo {
        self.host.clone()
    }
    fn install(&self, how: Option<Mode>) -> Result<String, String> {
        self.started.lock().unwrap().push(how);
        Ok("started 2 job(s) for Ollama".into())
    }
}

const GB: u64 = 1 << 30;

fn rig(runs: Where, bytes: u64, free: u64) -> (OllamaTools, Arc<Fake>) {
    let f = Arc::new(Fake { runs, host: HostInfo { models_dir: "/data/ollama/models".into(), models_bytes: bytes, free_bytes: Some(free) }, started: Mutex::new(vec![]) });
    (OllamaTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn the_status_is_free_and_the_install_asks_every_time_and_cannot_be_remembered() {
    let (t, _) = rig(Where::Nowhere, 0, 100 * GB);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(READ_TOOL), Some(Risk::Safe));
    assert_eq!(risk(WRITE_TOOL), Some(Risk::Ask));
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL));
    assert!(t.grant_blocker(WRITE_TOOL, &json!({})).is_some());
    assert!(hard_ask(WRITE_TOOL, &json!({})).is_some(), "Auto never runs it");
    assert!(hard_ask(READ_TOOL, &json!({})).is_none());
}

#[test]
fn the_status_line_says_where_ollama_runs() {
    let (t, _) = rig(Where::Host("0.5.1".into()), 0, 0);
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("runs on this machine's host"));
    let (t, _) = rig(Where::Cluster("in the cluster: http://127.0.0.1:11436".into()), 0, 0);
    let s = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("runs in the cluster") && s.contains("Memory:") && s.contains("CPU:"), "{s}");
}

#[test]
fn a_new_install_in_the_cluster_starts_the_plain_job() {
    let (t, f) = rig(Where::Nowhere, 0, 100 * GB);
    assert!(t.preview(WRITE_TOOL, &json!({})).unwrap().contains("Install Ollama in the cluster"));
    t.call(WRITE_TOOL, &json!({"how": "cluster"})).unwrap();
    assert_eq!(f.started.lock().unwrap().as_slice(), [None]);
    assert!(t.refusal(WRITE_TOOL, &json!({"how": "move"})).unwrap().contains("no Ollama"));
}

#[test]
fn an_ollama_on_the_host_is_moved_or_started_fresh_only_by_name() {
    let (t, f) = rig(Where::Host("0.5.1".into()), 5 * GB, 100 * GB);
    assert!(t.refusal(WRITE_TOOL, &json!({})).unwrap().contains("how=move"));
    let card = t.preview(WRITE_TOOL, &json!({"how": "move"})).unwrap();
    assert!(card.contains("nothing is deleted from the host") && card.contains("5.0 GB"), "{card}");
    t.call(WRITE_TOOL, &json!({"how": "move"})).unwrap();
    t.call(WRITE_TOOL, &json!({"how": "fresh"})).unwrap();
    assert_eq!(f.started.lock().unwrap().as_slice(), [Some(Mode::Move), Some(Mode::Fresh)]);
    assert!(t.refusal(WRITE_TOOL, &json!({"how": "sideways"})).unwrap().contains("cluster, move or fresh"));
}

#[test]
fn a_move_that_is_too_large_or_has_no_room_is_refused_and_starts_nothing() {
    for (bytes, free) in [(40 * GB, 500 * GB), (5 * GB, 5 * GB)] {
        let (t, f) = rig(Where::Host("0.5.1".into()), bytes, free);
        let why = t.refusal(WRITE_TOOL, &json!({"how": "move"})).unwrap();
        assert!(why.contains("Start fresh"), "{why}");
        assert!(t.call(WRITE_TOOL, &json!({"how": "move"})).is_err());
        assert!(f.started.lock().unwrap().is_empty());
        // starting fresh is still possible
        assert!(t.refusal(WRITE_TOOL, &json!({"how": "fresh"})).is_none());
    }
}

#[test]
fn an_ollama_already_in_the_cluster_is_not_installed_again() {
    let (t, f) = rig(Where::Cluster("in the cluster".into()), 0, 0);
    assert!(t.refusal(WRITE_TOOL, &json!({})).unwrap().contains("already runs in the cluster"));
    assert!(f.started.lock().unwrap().is_empty());
}
