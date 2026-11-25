//! The K8S tile over a fake port (no session, no audit log, no core files): what the tile asks of the rest of the system is exactly the
//! `K8sPort` calls. The cluster switcher is still built from the shell's context (`ClusterHost`, shared with the AI IDE: the remaining blocker
//! for a tile crate), so the fake lends the one thing it cannot make itself.

use super::*;
use crate::apps::cluster_host::ClusterHost;
use iyakki_k8s_model::k8sexp::Kind;
use iyakki_tile_k8s::FwdSpec;
use iyakki_sec::policy::Role;
use std::path::PathBuf;
use crate::ui::TextField;
use std::sync::{Arc, Mutex};
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_k8s_model::k8sexp::LogLine;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakePort {
    cx: AppCx,
    home: PathBuf,
    writable: bool,
    audit: Mutex<Vec<(String, String)>>,
    reads: Mutex<Vec<(String, usize)>>,
}

impl K8sPort for FakePort {
    fn user(&self) -> String {
        "ada".into()
    }
    fn role(&self) -> Role {
        Role::Admin
    }
    fn data_dir(&self) -> PathBuf {
        self.home.join("data")
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push((action.to_string(), detail.to_string()));
    }
    fn read_text(&self, path: &str, max: usize) -> Result<String, String> {
        self.reads.lock().unwrap().push((path.to_string(), max));
        Ok("apiVersion: v1\nkind: ConfigMap\nmetadata: {name: x}\n".into())
    }
    fn home_root(&self) -> PathBuf {
        self.home.clone()
    }
    fn allows_write(&self, _path: &str) -> bool {
        self.writable
    }
    fn work_report(&self) -> Option<Arc<dyn iyakki_k8s_model::k8sexp::WorkReport + Send + Sync>> {
        None
    }
    fn cluster_host(&self, base: usize, workspace: &str) -> ClusterHost {
        crate::apps::cluster_host::new(base, &self.cx, workspace)
    }
}

fn port(tag: &str, writable: bool) -> FakePort {
    static N: AtomicUsize = AtomicUsize::new(0);
    let home = temp_dir(&format!("{tag}-{}", N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(home.join("data")).unwrap();
    FakePort { cx: cx_with_admin_store(&format!("{tag}-cx-{}", N.fetch_add(1, Ordering::SeqCst))), home, writable, audit: Mutex::new(vec![]), reads: Mutex::new(vec![]) }
}

fn app(p: &FakePort) -> K8sApp {
    K8sApp::with_port(p, Rig::offline())
}

fn with_log(a: &mut K8sApp) {
    a.det.key = Some((Kind::Pods, "shop".into(), "web".into()));
    a.det.logs.buf.lines.push_back(LogLine { ts: "2026-10-22T10:00:00Z".into(), text: "starting".into() });
    a.det.logs.buf.lines.push_back(LogLine { ts: "2026-10-22T10:00:01Z".into(), text: "ready".into() });
}

#[test]
fn the_tile_takes_the_person_and_the_role_from_the_port() {
    let p = port("k8s-port-who", true);
    let a = app(&p);
    assert_eq!(a.user, "ada");
    assert!(a.admin);
    assert_eq!(a.dir, p.data_dir());
}

#[test]
fn saving_a_log_writes_under_the_ports_home_only_when_the_port_allows_it() {
    let p = port("k8s-port-save", false);
    let mut a = app(&p);
    with_log(&mut a);
    a.save_logs(&p);
    assert_eq!(a.status.as_ref().map(|s| s.0.as_str()), Some("Your account may not write there."));
    assert!(!p.home.join("shop-web.log").exists(), "nothing is written when the port says no");

    let p = port("k8s-port-save-ok", true);
    let mut a = app(&p);
    with_log(&mut a);
    a.save_logs(&p);
    assert_eq!(std::fs::read_to_string(p.home.join("shop-web.log")).unwrap(), "starting\nready\n");
    assert!(a.status.as_ref().is_some_and(|s| s.0.starts_with("Saved ") && s.1));
}

#[test]
fn allow_changes_is_audited_through_the_port_with_the_cluster_and_the_switch() {
    let p = port("k8s-port-allow", true);
    let mut a = app(&p);
    a.set_allow(true, &p);
    a.set_allow(false, &p);
    let log = p.audit.lock().unwrap().clone();
    let names: Vec<&str> = log.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(names, ["k8s.allow_changes.on", "k8s.allow_changes.off"], "{log:?}");
    assert!(log[0].1.starts_with("cluster="), "{}", log[0].1);
}

#[test]
fn a_manifest_is_read_through_the_ports_file_rule_with_the_apply_size_limit() {
    let p = port("k8s-port-apply", true);
    let mut a = app(&p);
    a.apply_path = Some(TextField::new("path", false));
    if let Some(f) = &mut a.apply_path {
        f.value = "deploy.yaml".into();
    }
    a.apply_load(&p);
    let reads = p.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(reads[0].0.ends_with("deploy.yaml"));
    assert_eq!(reads[0].1, iyakki_k8s_model::k8sexptools::MAX_YAML);
}

#[test]
fn the_real_port_forwards_through_the_cores_facet_and_shows_the_one_list() {
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    let mut p = port("k8s-port-fwd", true);
    p.cx.os = os.clone();
    let a = CxPort(&p.cx);
    assert!(a.forwards().is_empty());
    let spec = FwdSpec { kubeconfig: String::new(), context: "dev".into(), namespace: "shop".into(), kind: "svc".into(), name: "web".into(), remote: 80, local: None };
    let row = a.start_forward(&spec).unwrap();
    assert_eq!((row.id, row.local, row.target.as_str()), (1, 40000, "svc/web"));
    assert_eq!(os.port_forwards_fake().spawned()[0][..2], ["--context=dev", "port-forward"]);
    // what another caller of the same facet started is on the tile's list too
    iyakki_core_api::Os::port_forwards(&*os).start(&iyakki_core_api::portforward::Request { kubeconfig: String::new(), context: String::new(), namespace: "shop".into(), kind: iyakki_core_api::portforward::Target::Pod, name: "web-1".into(), remote: 8080, local: Some(18080) }).unwrap();
    assert_eq!(a.forwards().len(), 2);
    assert!(a.stop_forward(1) && !a.stop_forward(1));
    assert_eq!(a.forwards().iter().map(|r| r.id).collect::<Vec<_>>(), [2]);
    let bad = FwdSpec { kind: "node".into(), ..spec };
    assert!(a.start_forward(&bad).is_err());
}

#[test]
fn the_item_the_tile_hands_the_terminal_is_the_one_the_sdk_terminal_reads() {
    let item = iyakki_tile_k8s::forwards::pod_item("/k/c.yaml", "dev", "shop", "web-1", "app");
    let p = iyakki_ui_sdk::terminal::PodSession::from_item(&item).expect("the two sides agree");
    assert_eq!((p.kubeconfig.as_str(), p.context.as_str(), p.namespace.as_str(), p.pod.as_str(), p.container.as_str(), p.shell.as_str()), ("/k/c.yaml", "dev", "shop", "web-1", "app", ""));
}
