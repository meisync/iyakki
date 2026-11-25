//! The K8S tile and the cluster switcher over fake ports: no session, no audit log, no agent, no cluster. The registry is a folder of its own per test.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_cluster_model::cloudlaunch::{LaunchSpec, Plan};
use iyakki_cluster_model::clusters::{ClusterEntry, Kind, Registry, Status};
use iyakki_k8s_model::k8sexp::WorkReport;
use iyakki_sec::policy::Role;

use crate::cluster_host::{ClusterHost, ClusterHostPort, HostOut};
use crate::port::{CloudKit, K8sPort};
use crate::ui::Event;
use crate::{K8sApp, Rig};

static N: AtomicUsize = AtomicUsize::new(0);

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tile-k8s-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Host {
    dir: PathBuf,
    role: Role,
    audit: Mutex<Vec<(String, String)>>,
    switches: Mutex<Vec<String>>,
}

impl Host {
    fn new(tag: &str, role: Role) -> Arc<Host> {
        Arc::new(Host { dir: temp(tag), role, audit: Mutex::new(vec![]), switches: Mutex::new(vec![]) })
    }
    fn add_remote(&self, name: &str) {
        let mut r = Registry::load(&self.dir, "ada");
        r.add_remote(name, Kind::Remote, &format!("/fake/{name}.yaml"), "", "").unwrap();
        r.save(&self.dir, "ada").unwrap();
    }
}

impl ClusterHostPort for Arc<Host> {
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn user(&self) -> String {
        "ada".into()
    }
    fn role(&self) -> Role {
        self.role
    }
    fn local_status(&self, _e: &ClusterEntry) -> Status {
        Status::unknown()
    }
    fn remote_status(&self, _e: &ClusterEntry) -> Status {
        Status::unknown()
    }
    fn refresh_gpus(&self, _visible: &[ClusterEntry]) {}
    fn gpu_summary(&self, id: &str) -> Option<String> {
        (id == "gpu-box").then(|| "2 x Fake GPU".to_string())
    }
    fn switch_account(&self, id: &str) -> Result<ClusterEntry, String> {
        self.switches.lock().unwrap().push(id.to_string());
        let mut r = Registry::load(&self.dir, "ada");
        r.set_active(id)?;
        let e = r.active_entry();
        r.save(&self.dir, "ada")?;
        Ok(e)
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push((action.into(), detail.into()));
    }
}

struct Kit;

impl CloudKit for Kit {
    fn cloud_default_image(&self) -> String {
        "registry.example/iyakki:test".into()
    }
    fn cloud_plan(&self, l: &LaunchSpec) -> Plan {
        Plan { blockers: if l.tenant.is_empty() { vec!["name the tenant".into()] } else { vec![] }, ..Default::default() }
    }
    fn cloud_preview(&self, l: &LaunchSpec, p: &Plan) -> String {
        format!("tenant {} blockers {}", l.tenant, p.blockers.len())
    }
    fn cloud_execute(&self, _l: &LaunchSpec, _p: &Plan, _w: &std::path::Path, _k: &mut dyn FnMut(&[String]) -> Result<String, String>, _push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        Ok(vec!["applied".into()])
    }
    fn cloud_cli(&self, _a: &[String], _o: &mut dyn FnMut(&str)) -> Result<(), String> {
        Ok(())
    }
}

struct Port {
    host: Arc<Host>,
    audit: Mutex<Vec<String>>,
    /// The forwards that run and the specs the tile sent (the fake of the core's facet).
    fwd: Mutex<Vec<crate::FwdRow>>,
    sent: Mutex<Vec<crate::FwdSpec>>,
    refuse: Mutex<Option<String>>,
}

impl K8sPort for Port {
    fn user(&self) -> String {
        "ada".into()
    }
    fn role(&self) -> Role {
        self.host.role
    }
    fn data_dir(&self) -> PathBuf {
        self.host.dir.clone()
    }
    fn audit(&self, action: &str, _detail: &str) {
        self.audit.lock().unwrap().push(action.into());
    }
    fn read_text(&self, _p: &str, _max: usize) -> Result<String, String> {
        Err("no files".into())
    }
    fn home_root(&self) -> PathBuf {
        self.host.dir.clone()
    }
    fn allows_write(&self, _p: &str) -> bool {
        true
    }
    fn work_report(&self) -> Option<Arc<dyn WorkReport + Send + Sync>> {
        None
    }
    fn cluster_host(&self, base: usize, workspace: &str) -> ClusterHost {
        ClusterHost::new(base, Arc::new(self.host.clone()), workspace)
    }
    fn cloud_kit(&self) -> Arc<dyn CloudKit> {
        Arc::new(Kit)
    }
    fn instant(&self) -> bool {
        true
    }
    fn forwards(&self) -> Vec<crate::FwdRow> {
        self.fwd.lock().unwrap().clone()
    }
    fn start_forward(&self, spec: &crate::FwdSpec) -> Result<crate::FwdRow, String> {
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(spec.clone());
        let mut v = self.fwd.lock().unwrap();
        let row = crate::FwdRow { id: v.len() as u32 + 1, namespace: spec.namespace.clone(), target: format!("{}/{}", spec.kind, spec.name), remote: spec.remote, local: spec.local.unwrap_or(40000) };
        v.push(row.clone());
        Ok(row)
    }
    fn stop_forward(&self, id: u32) -> bool {
        let mut v = self.fwd.lock().unwrap();
        let n = v.len();
        v.retain(|r| r.id != id);
        v.len() != n
    }
}

fn port(tag: &str, role: Role) -> Port {
    Port { host: Host::new(tag, role), audit: Mutex::new(vec![]), fwd: Mutex::new(vec![]), sent: Mutex::new(vec![]), refuse: Mutex::new(None) }
}

#[test]
fn one_cluster_means_no_switcher_and_the_phrase_names_this_machine() {
    let h = Host::new("one", Role::Admin);
    let host = ClusterHost::new(7_000, Arc::new(h.clone()), "");
    assert!(!host.visible());
    assert_eq!(host.phrase(), "cluster \"This machine\" (local)");
}

#[test]
fn the_switch_is_asked_of_the_port_audited_and_the_rows_read_again() {
    let h = Host::new("switch", Role::Admin);
    h.add_remote("gpu-box");
    let mut host = ClusterHost::new(7_000, Arc::new(h.clone()), "");
    assert!(host.visible());
    assert_eq!(host.rows().iter().find(|r| r.id == "gpu-box").unwrap().gpu.as_deref(), Some("2 x Fake GPU"));
    let e = host.switch_account("gpu-box", &|_| None).unwrap();
    assert_eq!(e.id, "gpu-box");
    assert_eq!(host.effective().unwrap().id, "gpu-box");
    assert_eq!(*h.switches.lock().unwrap(), vec!["gpu-box".to_string()]);
    assert_eq!(host.handle(&Event::Tab(1), &|_| None), HostOut::None);
}

#[test]
fn the_tile_starts_over_a_fake_port_and_offline_rig_and_says_where_kubeconfigs_live() {
    let p = port("tile", Role::Admin);
    let a = K8sApp::with_port(&p, Rig::offline());
    assert_eq!(a.title(), "K8S");
    assert!(a.admin && !a.ide_ok);
    let rig = Rig::offline();
    let mut r = Registry::default();
    let second = r.add_local("Second").unwrap();
    let path = (rig.local_kubeconfig)(&p.host.dir, &second);
    assert!(path.ends_with("k3s-dev-second/kubeconfig"), "{path}");
}

#[test]
fn launch_cloud_mode_needs_an_administrator_and_takes_its_image_and_plan_from_the_kit() {
    let p = port("cloud", Role::User);
    let mut a = K8sApp::with_port(&p, Rig::offline());
    a.open_cloud();
    assert!(a.cloud.is_none() && a.status.as_ref().is_some_and(|(t, ok)| t.contains("Only administrators") && !ok));
    let p = port("cloud2", Role::Admin);
    let mut a = K8sApp::with_port(&p, Rig::offline());
    a.open_cloud();
    let f = a.cloud.as_ref().expect("the card opens");
    assert_eq!(f.fields[1].value, "registry.example/iyakki:test");
}

fn type_text(a: &mut K8sApp, p: &Port, text: &str) {
    for c in text.chars() {
        a.handle(&Event::Key(crate::ui::Key::Char(c)), p);
    }
}

fn running_tile(tag: &str, role: Role) -> (Port, K8sApp) {
    let p = port(tag, role);
    let mut a = K8sApp::with_port(&p, Rig::offline());
    a.managed = crate::Managed::Running;
    (p, a)
}

#[test]
fn the_forwards_window_is_for_administrators_and_a_running_cluster() {
    let (p, mut a) = running_tile("fwd-role", Role::User);
    a.handle(&Event::Tab(crate::T_FWD), &p);
    assert!(a.fwd.is_none() && a.status.as_ref().is_some_and(|(t, ok)| t.contains("Only administrators") && !ok));
    let (p, mut a) = running_tile("fwd-down", Role::Admin);
    a.managed = crate::Managed::Stopped;
    a.handle(&Event::Tab(crate::T_FWD), &p);
    assert!(a.fwd.is_none() && a.status.as_ref().is_some_and(|(t, _)| t.contains("not running")));
    let (p, mut a) = running_tile("fwd-open", Role::Admin);
    p.fwd.lock().unwrap().push(crate::FwdRow { id: 7, namespace: "shop".into(), target: "svc/web".into(), remote: 80, local: 40007 });
    a.handle(&Event::Key(crate::ui::Key::Char('F')), &p);
    assert_eq!(a.fwd.as_ref().unwrap().rows.len(), 1, "the window lists what runs");
    a.handle(&Event::Key(crate::ui::Key::Esc), &p);
    assert!(a.fwd.is_none());
}

#[test]
fn a_forward_is_started_through_the_port_with_the_namespace_and_cluster_and_audited_and_stopped_by_the_key() {
    let (p, mut a) = running_tile("fwd-go", Role::Admin);
    a.handle(&Event::Tab(crate::T_FWD), &p);
    // no namespace picked: said in words, nothing sent
    type_text(&mut a, &p, "svc/web 80");
    a.handle(&Event::Key(crate::ui::Key::Enter), &p);
    assert!(a.fwd.as_ref().unwrap().note.contains("Pick one namespace") && p.sent.lock().unwrap().is_empty());
    a.scope = iyakki_k8s_model::k8sexp::Scope::Ns("shop-dev".into());
    a.handle(&Event::Key(crate::ui::Key::Enter), &p);
    let sent = p.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].namespace.as_str(), sent[0].kind.as_str(), sent[0].name.as_str(), sent[0].remote, sent[0].local), ("shop-dev", "svc", "web", 80, None));
    let w = a.fwd.as_ref().unwrap();
    assert!(w.note.contains("Forwarding #1") && w.rows.len() == 1 && w.field.value.is_empty());
    assert!(p.audit.lock().unwrap().iter().any(|l| l == "k8s.port_forward"));
    // words that are not a forward, and a refusal of the core, stay in the window
    type_text(&mut a, &p, "node/n1 80");
    a.handle(&Event::Key(crate::ui::Key::Enter), &p);
    assert!(a.fwd.as_ref().unwrap().note.contains("pod, svc or deployment"));
    a.fwd.as_mut().unwrap().field.set("pod/web-1 8080");
    *p.refuse.lock().unwrap() = Some("card_declined".into());
    a.handle(&Event::Key(crate::ui::Key::Enter), &p);
    assert_eq!(a.fwd.as_ref().unwrap().note, "card_declined");
    assert_eq!(p.sent.lock().unwrap().len(), 1, "a refused one is not sent twice");
    // Delete stops the selected one
    a.handle(&Event::Key(crate::ui::Key::Delete), &p);
    assert!(a.fwd.as_ref().unwrap().note.contains("Stopped #1") && a.fwd.as_ref().unwrap().rows.is_empty() && p.fwd.lock().unwrap().is_empty());
    assert!(p.audit.lock().unwrap().iter().any(|l| l == "k8s.port_forward_stop"));
    a.handle(&Event::Key(crate::ui::Key::Delete), &p);
    assert!(a.fwd.as_ref().unwrap().note.contains("No forward is selected"));
}

#[test]
fn a_selected_pod_prefills_the_field_and_its_namespace_is_the_one_used() {
    let (p, mut a) = running_tile("fwd-pre", Role::Admin);
    a.det.key = Some((iyakki_k8s_model::k8sexp::Kind::Pods, "team-a".into(), "web-1".into()));
    a.handle(&Event::Tab(crate::T_FWD), &p);
    assert_eq!(a.fwd.as_ref().unwrap().field.value, "pod/web-1 ");
    type_text(&mut a, &p, "8080");
    a.handle(&Event::Key(crate::ui::Key::Enter), &p);
    let sent = p.sent.lock().unwrap().clone();
    assert_eq!((sent[0].namespace.as_str(), sent[0].kind.as_str(), sent[0].remote), ("team-a", "pod", 8080));
    // the window draws without panicking, with the row and the buttons
    let mut g = crate::gfx::Gfx::new(900, 700);
    a.render(&mut g, crate::ui::R::new(0.0, 0.0, 900.0, 700.0), 1.0, &p, 0);
}

#[test]
fn open_a_terminal_hands_the_terminal_an_item_naming_the_pod_and_the_cluster_and_needs_a_selected_pod() {
    let (p, mut a) = running_tile("term", Role::Admin);
    a.handle(&Event::Key(crate::ui::Key::Char('T')), &p);
    assert!(a.take_request().is_none() && a.status.as_ref().is_some_and(|(t, ok)| t.contains("Select a pod") && !ok));
    a.det.key = Some((iyakki_k8s_model::k8sexp::Kind::Services, "team-a".into(), "web".into()));
    a.handle(&Event::Tab(crate::T_TERM), &p);
    assert!(a.take_request().is_none(), "only a pod");
    a.det.key = Some((iyakki_k8s_model::k8sexp::Kind::Pods, "team-a".into(), "web-1".into()));
    a.det.containers = vec!["app".into(), "sidecar".into()];
    a.handle(&Event::Key(crate::ui::Key::Char('T')), &p);
    let (app, item) = a.take_request().expect("the Terminal is asked");
    assert_eq!(app, "cli");
    assert!(item.starts_with("pod:") && item.contains("\nteam-a\nweb-1\n"), "{item:?}");
    assert!(a.take_request().is_none(), "taken once");
}
