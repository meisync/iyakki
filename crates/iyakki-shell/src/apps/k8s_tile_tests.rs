//! The K8S tile against a fake kubectl with canned JSON for every kind. No real cluster is ever asked: the rig's cluster client is a
//! `FakeKube`, the managed cluster's state is a value the test sets, and the audit log is a temp file.

use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};
use iyakki_agent::k8s::FakeKube;
use iyakki_agent::k8sexp::{Kind, Scope};
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NOW: u64 = 1_800_000_000;

fn ts(ago: u64) -> String {
    let t = (clock().max(NOW) - ago) as i64;
    let (days, rem) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn pod(name: &str, ns: &str, phase: &str, ready: bool, restarts: i64, state: Value, ago: u64) -> Value {
    json!({"kind":"Pod","metadata":{"name":name,"namespace":ns,"creationTimestamp":ts(ago),"labels":{"app":"web"},"ownerReferences":[{"kind":"ReplicaSet","name":"web-7d9"}]},
        "spec":{"nodeName":"node-a","containers":[{"name":"app","image":"reg/web:1"},{"name":"side","image":"reg/side:1"}]},
        "status":{"phase":phase,"podIP":"10.42.0.9","containerStatuses":[{"name":"app","ready":ready,"restartCount":restarts,"state":state,"lastState":{}},{"name":"side","ready":ready,"restartCount":0,"state":{"running":{}},"lastState":{}}]}})
}

fn shop_pods() -> Vec<Value> {
    let mut term = pod("web-term", "shop", "Running", true, 0, json!({"running":{}}), 500);
    term["metadata"]["deletionTimestamp"] = json!(ts(5));
    vec![
        pod("web-run", "shop", "Running", true, 0, json!({"running":{}}), 3 * 86_400),
        pod("web-pending", "shop", "Pending", false, 0, json!({"waiting":{"reason":"ContainerCreating"}}), 40),
        pod("web-crash", "shop", "Running", false, 14, json!({"waiting":{"reason":"CrashLoopBackOff"}}), 7200),
        pod("job-done", "shop", "Succeeded", false, 0, json!({"terminated":{"reason":"Completed"}}), 900),
        term,
        pod("web-pull", "shop", "Pending", false, 0, json!({"waiting":{"reason":"ImagePullBackOff"}}), 120),
    ]
}

fn list(items: Vec<Value>) -> String {
    json!({"kind":"List","items":items}).to_string()
}

fn secret() -> Value {
    json!({"kind":"Secret","type":"Opaque","metadata":{"name":"db","namespace":"shop","creationTimestamp":ts(60),
        "annotations":{"kubectl.kubernetes.io/last-applied-configuration":"{\"data\":{\"pw\":\"aHVudGVyMg==\"}}"}},"data":{"pw":"aHVudGVyMg==","user":"YWRtaW4="}})
}

/// One canned object per kind (namespace shop).
fn sample(kind: Kind) -> Value {
    let meta = |n: &str| json!({"name":n,"namespace":"shop","creationTimestamp":ts(3600),"labels":{"app":"web"}});
    match kind {
        Kind::Pods => shop_pods()[0].clone(),
        Kind::Deployments => json!({"kind":"Deployment","metadata":meta("web"),"spec":{"replicas":3,"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{"labels":{"app":"web"}},"spec":{"containers":[{"image":"reg/web:1"}]}}},"status":{"readyReplicas":2,"updatedReplicas":3,"availableReplicas":2}}),
        Kind::StatefulSets => json!({"kind":"StatefulSet","metadata":meta("db"),"spec":{"replicas":2,"selector":{"matchLabels":{"app":"db"}}},"status":{"readyReplicas":2}}),
        Kind::DaemonSets => json!({"kind":"DaemonSet","metadata":meta("agent"),"status":{"desiredNumberScheduled":1,"numberReady":1,"updatedNumberScheduled":1,"numberAvailable":1}}),
        Kind::ReplicaSets => json!({"kind":"ReplicaSet","metadata":meta("web-7d9"),"spec":{"replicas":3},"status":{"replicas":3,"readyReplicas":2}}),
        Kind::Jobs => json!({"kind":"Job","metadata":meta("migrate"),"spec":{"completions":1},"status":{"succeeded":1}}),
        Kind::CronJobs => json!({"kind":"CronJob","metadata":meta("nightly"),"spec":{"schedule":"0 3 * * *","suspend":false},"status":{"lastScheduleTime":ts(600)}}),
        Kind::Services => json!({"kind":"Service","metadata":meta("front"),"spec":{"type":"NodePort","clusterIP":"10.43.0.5","selector":{"app":"web"},"ports":[{"port":80,"protocol":"TCP","targetPort":8080,"nodePort":30080}]}}),
        Kind::Ingresses => json!({"kind":"Ingress","metadata":meta("site"),"spec":{"ingressClassName":"traefik","rules":[{"host":"shop.example"}]},"status":{"loadBalancer":{"ingress":[{"ip":"10.0.0.7"}]}}}),
        Kind::ConfigMaps => json!({"kind":"ConfigMap","metadata":meta("settings"),"data":{"a":"1"}}),
        Kind::Secrets => secret(),
        Kind::Pvcs => json!({"kind":"PersistentVolumeClaim","metadata":meta("data"),"spec":{"storageClassName":"local-path"},"status":{"phase":"Bound","capacity":{"storage":"1Gi"}}}),
        Kind::Events => json!({"kind":"Event","type":"Warning","reason":"BackOff","message":"Back-off restarting failed container","involvedObject":{"kind":"Pod","name":"web-crash"},"metadata":meta("web-crash.17a"),"lastTimestamp":ts(30)}),
        Kind::Nodes => json!({"kind":"Node","metadata":{"name":"node-a","creationTimestamp":ts(86_400),"labels":{"node-role.kubernetes.io/control-plane":"true"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"allocatable":{"cpu":"4","memory":"16314232Ki"},"nodeInfo":{"kubeletVersion":"v1.30.4+k3s1"},"addresses":[{"type":"InternalIP","address":"10.0.0.2"}]}}),
        Kind::Namespaces => json!({"kind":"Namespace","metadata":{"name":"shop","creationTimestamp":ts(86_400)},"status":{"phase":"Active"}}),
        Kind::ServiceAccounts => json!({"kind":"ServiceAccount","metadata":meta("default"),"secrets":[{"name":"x"}]}),
        Kind::Crds => json!({"kind":"CustomResourceDefinition","metadata":{"name":"widgets.example.com","creationTimestamp":ts(86_400)},"spec":{"group":"example.com","scope":"Namespaced","names":{"kind":"Widget"}}}),
    }
}

fn namespaces_and_pods() -> String {
    let mut items = vec![json!({"kind":"Namespace","metadata":{"name":"shop"},"status":{"phase":"Active"}}), json!({"kind":"Namespace","metadata":{"name":"empty"},"status":{"phase":"Active"}}), json!({"kind":"Namespace","metadata":{"name":"kube-system"},"status":{"phase":"Active"}})];
    items.extend(shop_pods());
    items.push(pod("coredns", "kube-system", "Running", true, 0, json!({"running":{}}), 9000));
    list(items)
}

fn cluster() -> Arc<FakeKube> {
    let k = Arc::new(FakeKube::new());
    for kind in Kind::ALL {
        k.answer(&format!("get {} -o json", kind.resource()), &list(vec![sample(kind)]));
        let name = sample(kind)["metadata"]["name"].as_str().unwrap().to_string();
        k.answer(&format!("get {} {} -o json", kind.resource(), name), &sample(kind).to_string());
    }
    k.answer("-n shop get pods -o json", &list(shop_pods()));
    k.answer("get pods -A -o json", &list(shop_pods()));
    for p in shop_pods() {
        let n = p["metadata"]["name"].as_str().unwrap().to_string();
        k.answer(&format!("get pods {n} -o json"), &p.to_string());
    }
    k.answer("get namespaces,pods -A -o json", &namespaces_and_pods());
    k.answer("get replicasets web-7d9 -o json", &json!({"metadata":{"name":"web-7d9","ownerReferences":[{"kind":"Deployment","name":"web"}]}}).to_string());
    k.answer("get events", &list(vec![sample(Kind::Events)]));
    k.answer("top nodes", "node-a 250m 6% 1024Mi 6%\n");
    k.answer("logs web-crash", "2026-10-22T10:00:00Z starting\n2026-10-22T10:00:01Z ERROR could not connect to db\n2026-10-22T10:00:02Z retrying\n");
    k
}

struct Fx {
    app: K8sApp,
    cx: AppCx,
    kube: Arc<FakeKube>,
    managed: Arc<Mutex<Managed>>,
    starts: Arc<AtomicUsize>,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

fn rig_for(kube: Arc<FakeKube>, managed: Arc<Mutex<Managed>>, starts: Arc<AtomicUsize>) -> Rig {
    let k2 = kube.clone();
    let m2 = managed.clone();
    Rig {
        kube_for: Arc::new(move |_| k2.clone()),
        managed: Arc::new(move || (m2.lock().unwrap().clone(), None)),
        start: Arc::new(move |_| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    }
}

fn fixture_with(tag: &str, role: Role, w: u32, h: u32, s: f32, state: Managed) -> Fx {
    // every test gets its own folder: the choices of a person are kept next to their settings
    static N: AtomicUsize = AtomicUsize::new(0);
    let mut cx = cx_with_admin_store(&format!("{tag}-{}", N.fetch_add(1, Ordering::SeqCst)));
    cx.session.role = role;
    cx.session.policy = Policy::resolve(role, &Default::default());
    let kube = cluster();
    let managed = Arc::new(Mutex::new(state));
    let starts = Arc::new(AtomicUsize::new(0));
    let app = K8sApp::with_rig(&cx, rig_for(kube.clone(), managed.clone(), starts.clone()));
    let mut g = Gfx::new(w, h);
    g.clear(rgb(8, 10, 22));
    Fx { app, cx, kube, managed, starts, g, area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 }
}

fn fixture() -> Fx {
    fixture_with("k8s-tile", Role::Admin, 1280, 720, 1.0, Managed::Running)
}

impl Fx {
    fn draw(&mut self) {
        let (w, h) = (self.area.w as u32 + self.area.x as u32, self.area.h as u32 + self.area.y as u32);
        // one canvas for the whole test: building a Gfx parses the fonts, and the glyphs it has drawn stay cached
        if (self.g.width(), self.g.height()) != (w, h) {
            self.g = Gfx::new(w, h);
        }
        self.g.clear_hits();
        self.g.clear_probes();
        self.g.clear(rgb(8, 10, 22));
        self.tick += 1;
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn send(&mut self, ev: Event) -> crate::apps::Flow {
        let f = self.app.handle(&ev, &self.cx);
        self.draw();
        f
    }
    fn key(&mut self, c: char) {
        self.send(Event::Key(Key::Char(c)));
    }
    fn press(&mut self, k: Key) {
        self.send(Event::Key(k));
    }
    fn click(&mut self, label: &str) {
        let ev = self.g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`: {:?}", self.g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>())).ev.clone();
        self.send(ev);
    }
    fn has_hit(&self, label: &str) -> bool {
        self.g.hits.iter().any(|h| h.label == label)
    }
    fn code(&mut self, n: usize) {
        self.send(Event::Tab(n));
    }
    /// Draw and tick until `ok` holds (a feed thread and job threads are answering).
    fn wait(&mut self, what: &str, ok: impl Fn(&mut Fx) -> bool) {
        let t0 = Instant::now();
        loop {
            self.draw();
            self.app.handle(&Event::Tick, &self.cx);
            self.draw();
            if ok(self) {
                return;
            }
            assert!(t0.elapsed() < Duration::from_secs(8), "timed out waiting for: {what}; status {:?}; calls {:?}", self.app.status, self.kube.calls());
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    fn list_loaded(&mut self) {
        self.wait("the list", |f| f.app.listing().is_some());
    }
    fn open_first(&mut self, name: &str) {
        self.list_loaded();
        let i = self.rows().iter().position(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}: {:?}", self.rows().iter().map(|r| r.name.clone()).collect::<Vec<_>>()));
        self.code(T_RES_ROW + i);
        self.wait("the details", |f| f.app.det.obj.ready().is_some());
    }
    fn rows(&mut self) -> Vec<iyakki_agent::k8sexp::Row> {
        let l = self.app.listing();
        match l {
            Some(l) => self.app.order().to_vec().iter().map(|&i| l.rows[i].clone()).collect(),
            None => vec![],
        }
    }
    fn view_text(&self) -> String {
        self.app.det.view.text()
    }
    fn audit_text(&self) -> String {
        // the log is JSON lines: a quote inside a detail is stored escaped
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default().replace("\\\"", "\"")
    }
    fn calls_with(&self, needle: &str) -> usize {
        self.kube.calls().iter().filter(|c| c.contains(needle)).count()
    }
    fn clipboard(&self) -> String {
        crate::ui::clipboard::latest().unwrap_or_default()
    }
}

// ------------------------------------------------------------------------------------------------ the cluster picker

#[test]
fn the_cluster_picker_shows_the_managed_cluster_with_its_state_and_the_saved_profiles() {
    let mut f = fixture();
    iyakki_agent::kubeprofiles::vault_add(f.cx.services.dir(), &f.cx.session.user, "lab", "uvault:lab-kube").unwrap();
    f.app.refresh_entries();
    assert_eq!(f.app.entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), vec!["Dev cluster (managed k3s)", "lab"]);
    f.draw();
    // two clusters: the switcher replaces the plain list box; its list names both
    f.code(T_SWITCH);
    assert!(f.has_hit("Manage clusters...") && f.g.hits.iter().any(|h| h.label.starts_with("lab (")) && f.g.hits.iter().any(|h| h.label.starts_with("This machine (")));
    // the kubeconfig reference is never a label, and never drawn or said
    assert!(!f.g.hits.iter().any(|h| h.label.contains("lab-kube") || h.label.contains("uvault")));
    for (state, chip) in [(Managed::Running, "Running"), (Managed::Starting { waiting_net: false }, "Starting"), (Managed::Starting { waiting_net: true }, "Waiting for the network"), (Managed::Stopped, "Stopped"), (Managed::NotSet, "Not set up"), (Managed::Failed("x".into()), "Failed")] {
        assert_eq!(state.chip(), chip);
    }
}

#[test]
fn a_stopped_cluster_says_so_with_a_start_button_that_starts_it() {
    let mut f = fixture_with("k8s-stopped", Role::Admin, 1280, 720, 1.0, Managed::Stopped);
    f.draw();
    assert!(f.has_hit("Start"), "{:?}", f.g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    let (title, text, _) = f.app.state_text();
    assert_eq!(title, "The dev cluster is stopped");
    assert!(text.contains("Start it"));
    assert!(f.kube.calls().is_empty(), "a stopped cluster is not asked anything");
    f.click("Start");
    f.wait("the start", |f| f.starts.load(Ordering::SeqCst) == 1);
    f.wait("the message", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("Starting the dev cluster")));
    // up: the tile fills in by itself
    *f.managed.lock().unwrap() = Managed::Running;
    f.wait("the list after the cluster came up", |f| f.app.listing().is_some_and(|l| l.cluster == iyakki_agent::k8s::Cluster::Ok));
    assert!(!f.has_hit("Start"));
}

#[test]
fn starting_and_waiting_for_the_network_and_not_set_up_are_never_blank() {
    for (state, title_has, button) in [(Managed::Starting { waiting_net: true }, "Waiting for the network", None), (Managed::Starting { waiting_net: false }, "starting", None), (Managed::NotSet, "No dev cluster", Some("Open Settings")), (Managed::Failed("k3s exited".into()), "failed", Some("Start"))] {
        let mut f = fixture_with("k8s-states", Role::Admin, 800, 480, 1.0, state);
        f.draw();
        let (title, text, _) = f.app.state_text();
        assert!(title.contains(title_has), "{title}");
        assert!(text.len() > 40, "plain words: {text}");
        if let Some(b) = button {
            assert!(f.has_hit(b), "{b}");
        }
        assert!(f.kube.calls().is_empty(), "nothing is asked of a cluster that is not running");
    }
}

#[test]
fn open_settings_asks_the_shell_for_the_dev_cluster_page() {
    let mut f = fixture_with("k8s-settings", Role::Admin, 1280, 720, 1.0, Managed::NotSet);
    f.draw();
    f.click("Open Settings");
    assert_eq!(crate::apps::App::take_request(&mut f.app), Some(("settings".to_string(), "tab:devcluster".to_string())));
}

#[test]
fn kubectl_missing_and_unreachable_get_the_plain_text_of_the_ide() {
    let mut f = fixture();
    f.kube.missing();
    f.list_loaded();
    f.wait("the banner", |f| f.app.state_text().0 == "kubectl was not found");
    let (_, text, _) = f.app.state_text();
    assert!(text.contains("Install kubectl"));
    assert!(f.has_hit("Refresh"));
    let mut f = fixture();
    // a missing configuration is a persistent failure: the banner comes at once (a refused connection of the managed k3s is
    // transient and waits, see the tests at the end)
    f.kube.fail("get", "error: no configuration has been provided, try setting KUBERNETES_MASTER environment variable");
    f.wait("the banner", |f| f.app.state_text().0 == "The cluster is not reachable");
}

#[test]
fn the_last_cluster_namespace_and_kind_are_remembered_per_person() {
    let mut f = fixture();
    iyakki_agent::kubeprofiles::vault_add(f.cx.services.dir(), &f.cx.session.user, "lab", "uvault:lab-kube").unwrap();
    f.app.refresh_entries();
    f.list_loaded();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.app.choose_kind(Kind::Services);
    f.app.select_cluster(1, true);
    f.app.choose_scope(Scope::Ns("kube-system".into()));
    f.app.select_cluster(0, true);
    assert_eq!((f.app.scope.clone(), f.app.kind), (Scope::Ns("shop".into()), Kind::Services), "back on the first cluster: its own choices");
    f.app.select_cluster(1, true);
    // a new tile for the same person starts where they left off
    let managed = Arc::new(Mutex::new(Managed::Running));
    let again = K8sApp::with_rig(&f.cx, rig_for(f.kube.clone(), managed.clone(), Arc::new(AtomicUsize::new(0))));
    assert_eq!(again.entry().key, "profile:lab");
    assert_eq!(again.scope, Scope::Ns("kube-system".into()));
    // another person starts clean
    let mut cx2 = f.cx.clone();
    cx2.session.user = "someone-else".into();
    let other = K8sApp::with_rig(&cx2, rig_for(f.kube.clone(), managed, Arc::new(AtomicUsize::new(0))));
    assert_eq!(other.scope, Scope::All);
    assert_eq!(other.kind, Kind::Pods);
    assert_eq!(other.entry().key, "managed");
}

// ------------------------------------------------------------------------------------------------ namespaces

#[test]
fn the_namespace_list_has_all_namespaces_pod_badges_and_a_red_mark_for_failures() {
    let mut f = fixture();
    f.wait("namespaces", |f| f.app.feed.ns().is_some());
    let rows = f.app.ns_rows();
    assert_eq!(rows[0].1, "All namespaces");
    assert_eq!(rows[0].2, Some(7), "all pods");
    assert_eq!(rows[0].3, 2);
    let by = |n: &str| rows.iter().find(|r| r.1 == n).unwrap().clone();
    assert_eq!((by("shop").2, by("shop").3), (Some(6), 2));
    assert_eq!((by("empty").2, by("empty").3), (Some(0), 0));
    assert_eq!((by("kube-system").2, by("kube-system").3), (Some(1), 0));
    f.draw();
    assert!(f.has_hit("shop") && f.has_hit("All namespaces") && f.has_hit("empty"));
}

#[test]
fn the_namespace_filter_narrows_the_list_and_escape_clears_it() {
    let mut f = fixture();
    f.wait("namespaces", |f| f.app.feed.ns().is_some());
    f.key('n');
    assert_eq!(f.app.focus, Focus::Ns, "n goes to the namespaces (a column at this width)");
    f.key('/');
    assert_eq!(f.app.focus, Focus::NsFilter);
    for c in "kube".chars() {
        f.key(c);
    }
    assert_eq!(f.app.ns_rows().iter().map(|r| r.1.clone()).collect::<Vec<_>>(), vec!["kube-system"]);
    f.press(Key::Esc);
    assert!(f.app.ns_filter.value.is_empty(), "Esc clears the filter");
    assert!(f.app.ns_rows().len() >= 4);
}

#[test]
fn choosing_a_namespace_lists_only_that_namespace_with_one_call() {
    let mut f = fixture();
    f.list_loaded();
    let i = f.app.ns_rows().iter().position(|r| r.1 == "shop").unwrap();
    f.code(T_NS_ROW + i);
    assert_eq!(f.app.scope, Scope::Ns("shop".into()));
    f.wait("the shop list", |f| f.app.listing().is_some());
    assert!(f.calls_with("-n shop get pods -o json") >= 1);
    let rows = f.rows();
    assert_eq!(rows.len(), 6);
    assert_eq!(f.app.prefs.of("managed").namespace, "shop");
}

#[test]
fn each_refresh_asks_one_list_call_and_one_namespace_call_and_nothing_when_hidden() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.wait("first answers", |f| f.app.listing().is_some() && f.app.feed.ns().is_some());
    let before_list = f.calls_with("-n shop get pods -o json");
    let before_ns = f.calls_with("namespaces,pods");
    f.key('r');
    f.wait("one more fetch", |f| f.calls_with("-n shop get pods -o json") > before_list);
    // let any in-flight fetch finish
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.calls_with("-n shop get pods -o json"), before_list + 1, "one list call per refresh, not one per row");
    assert_eq!(f.calls_with("namespaces,pods"), before_ns + 1, "one batch call for all namespaces and their pods");
    // the tile is not drawn for a while (another pane / the launcher): nothing is asked
    std::thread::sleep(Duration::from_millis(700));
    f.app.handle(&Event::Tick, &f.cx);
    std::thread::sleep(Duration::from_millis(100));
    let n = f.kube.calls().len();
    f.app.feed.refresh();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.kube.calls().len(), n, "not on screen: no kubectl");
}

// ------------------------------------------------------------------------------------------------ the explorer

#[test]
fn every_kind_has_a_table_with_the_right_columns() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    for kind in Kind::ALL {
        f.app.choose_kind(kind);
        f.wait(kind.label(), |f| f.app.listing().is_some());
        let rows = f.rows();
        assert!(!rows.is_empty(), "{kind:?}");
        let cols = iyakki_agent::k8sexp::columns(kind, false);
        assert_eq!(rows[0].cells.len(), cols.len(), "{kind:?}");
        // the list and the drawing agree: the header of every shown column is clickable for sorting
        assert!(f.has_hit("Name") || kind == Kind::Events, "{kind:?}: {:?}", f.g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    }
    f.app.choose_kind(Kind::Nodes);
    f.wait("nodes", |f| f.app.listing().is_some());
    let n = f.rows();
    assert_eq!(n[0].cells[6], "250m (6%)");
}

#[test]
fn events_are_newest_first_with_warnings_highlighted_and_pods_show_statuses() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    let by_status: Vec<String> = f.rows().iter().map(|r| r.cells[2].clone()).collect();
    for want in ["Running", "ContainerCreating", "CrashLoopBackOff", "Completed", "Terminating", "ImagePullBackOff"] {
        assert!(by_status.iter().any(|s| s == want), "{want}: {by_status:?}");
    }
    f.app.choose_kind(Kind::Events);
    f.wait("events", |f| f.app.listing().is_some());
    assert!(f.rows()[0].warn);
}

#[test]
fn the_header_counts_pods_and_the_ones_not_ready() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    let l = f.app.listing().unwrap();
    let line = iyakki_agent::k8sexp::count_line(Kind::Pods, &l.rows, l.total);
    assert_eq!(line, "6 pods \u{b7} 4 not ready");
}

#[test]
fn sorting_by_a_column_filtering_and_the_slash_key() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    f.click("Restarts");
    assert_eq!(f.rows().last().unwrap().name, "web-crash", "the first click sorts ascending: the 14 restarts come last");
    f.click("Restarts");
    assert_eq!(f.rows()[0].name, "web-crash");
    f.key('/');
    assert_eq!(f.app.focus, Focus::Filter);
    for c in "pull".chars() {
        f.key(c);
    }
    assert_eq!(f.rows().iter().map(|r| r.name.clone()).collect::<Vec<_>>(), vec!["web-pull"]);
    f.press(Key::Esc);
    assert!(f.app.filter.value.is_empty() && f.rows().len() == 6, "Esc clears the filter");
    // a filter that matches nothing says so
    f.key('/');
    for c in "zzz".chars() {
        f.key(c);
    }
    assert!(f.rows().is_empty());
}

#[test]
fn a_huge_list_shows_the_first_five_hundred_rows_and_says_how_many_more() {
    let mut f = fixture();
    let items: Vec<Value> = (0..3000).map(|i| pod(&format!("p{i:05}"), "shop", "Running", true, 0, json!({"running":{}}), 60)).collect();
    f.kube.answer("-n shop get pods -o json", &list(items));
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.wait("the big list", |f| f.app.listing().is_some_and(|l| l.total == 3000));
    assert_eq!(f.app.shown_rows(), iyakki_agent::k8sexp::MAX_SHOWN);
    assert_eq!(f.app.order().len(), 3000, "the filter and the order cover everything that is kept");
    f.press(Key::End);
    assert_eq!(f.app.res.sel, iyakki_agent::k8sexp::MAX_SHOWN - 1, "the cursor stops at the last row shown");
    f.key('/');
    for c in "p0299".chars() {
        f.key(c);
    }
    assert!(f.rows().len() <= 10 && f.app.shown_rows() <= 10, "refining the filter finds the rest");
}

// ------------------------------------------------------------------------------------------------ details

#[test]
fn a_pods_summary_yaml_events_and_related_objects() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.wait("summary", |f| f.view_text().contains("CrashLoopBackOff"));
    let t = f.view_text();
    assert!(t.contains("Status: CrashLoopBackOff"), "{t}");
    assert!(t.contains("What it means:") && t.contains("starts and then exits"), "{t}");
    assert!(t.contains("Owned by: ReplicaSet web-7d9"), "{t}");
    f.key('y');
    let y = f.view_text();
    assert!(y.contains("kind: Pod") && y.contains("name: web-crash"), "{y}");
    assert!(y.contains("restartCount: 14"), "{y}");
    f.key('e');
    f.wait("events", |f| f.view_text().contains("BackOff"));
    f.key('o');
    f.wait("related", |f| f.view_text().contains("Owner"));
    let r = f.view_text();
    assert!(r.contains("Owner: ReplicaSet web-7d9") && r.contains("Controlled by: Deployment web"), "{r}");
    assert!(r.contains("Runs on node: Node node-a"), "{r}");
    // a related line takes you to that object
    f.code(T_REL_ROW);
    assert_eq!(f.app.kind, Kind::ReplicaSets);
}

#[test]
fn secret_values_never_appear_until_they_are_revealed_after_asking() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.app.choose_kind(Kind::Secrets);
    f.open_first("db");
    for tab in ['s', 'y', 'e', 'o'] {
        f.key(tab);
        f.wait("tab", |f| !f.view_text().contains("Reading"));
        let t = f.view_text();
        for leak in ["hunter2", "aHVudGVyMg", "YWRtaW4", "admin", "last-applied"] {
            assert!(!t.contains(leak), "{tab}: {leak} leaked: {t}");
        }
    }
    f.key('y');
    assert!(f.view_text().contains(iyakki_agent::k8sexp::MASK), "{}", f.view_text());
    // copy while masked
    f.key('c');
    assert!(f.clipboard().contains(iyakki_agent::k8sexp::MASK) && !f.clipboard().contains("hunter2"));
    // the rendered hits, status and audit never hold a value either
    assert!(!f.audit_text().contains("hunter2"));
    // reveal asks first
    f.click("Reveal (asks first)");
    assert!(f.app.reveal_ask);
    f.draw();
    assert!(!f.view_text().contains("hunter2"), "nothing shown while the card is up");
    f.key('n');
    assert!(!f.app.reveal_ask && !f.app.det.reveal, "No keeps them hidden");
    f.click("Reveal (asks first)");
    f.click("Reveal");
    f.wait("the revealed values", |f| f.view_text().contains("hunter2"));
    assert!(f.view_text().contains("user: admin"));
    let a = f.audit_text();
    assert!(a.contains("k8s.secret.reveal") && a.contains("Secret/db"), "{a}");
    assert!(!a.contains("hunter2") && !a.contains("aHVudGVyMg"), "the audit has names only: {a}");
    // hide again, and moving to another object forgets the reveal
    f.click("Hide values");
    f.wait("masked again", |f| !f.view_text().contains("hunter2"));
}

#[test]
fn secret_lists_count_keys_and_never_carry_values() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.app.choose_kind(Kind::Secrets);
    f.list_loaded();
    let rows = f.rows();
    assert_eq!(rows[0].cells[1], "Opaque");
    assert_eq!(rows[0].cells[2], "2");
    assert!(!format!("{rows:?}").contains("hunter2"));
}

#[test]
fn pod_logs_with_container_previous_tail_follow_search_and_copy() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.key('l');
    assert_eq!(f.app.det.tab, DTab::Logs);
    f.wait("logs", |f| f.view_text().contains("could not connect"));
    assert!(f.calls_with("logs web-crash --timestamps --tail=500") >= 1, "{:?}", f.kube.calls());
    // the timestamp is not part of the text
    assert!(!f.view_text().contains("2026-10-22T"));
    // container picker: one chip per container
    assert!(f.has_hit("app") && f.has_hit("side"));
    f.click("side");
    f.wait("side logs", |f| f.calls_with(" -c side ") >= 1);
    // previous
    f.click("Previous");
    f.wait("previous logs", |f| f.calls_with("--previous") >= 1);
    f.click("Previous: on");
    // tail 100 / 500 / 2000
    f.click("Last 500");
    f.wait("tail 2000", |f| f.calls_with("--tail=2000") >= 1);
    f.click("Last 2000");
    f.wait("tail 100", |f| f.calls_with("--tail=100") >= 1);
    // search highlights (the view keeps every line; the field holds the text)
    f.key('/');
    assert_eq!(f.app.focus, Focus::LogSearch);
    for c in "error".chars() {
        f.key(c);
    }
    assert_eq!(f.app.det.logs.search.value, "error");
    f.press(Key::Enter);
    f.key('c');
    assert!(f.clipboard().contains("ERROR could not connect to db"));
}

#[test]
fn follow_mode_adds_only_new_lines_and_stays_inside_the_buffer() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.key('l');
    f.wait("logs", |f| f.app.det.logs.loaded && f.app.det.logs.buf.lines.len() == 3);
    assert!(f.app.det.logs.follow);
    // new lines arrive: the next poll asks only for what came after the last timestamp
    f.kube.answer("logs web-crash", "2026-10-22T10:00:02Z retrying\n2026-10-22T10:00:03Z connected\n");
    f.app.det.logs.polled = Some(Instant::now() - Duration::from_secs(5));
    f.wait("one more line", |f| f.app.det.logs.buf.lines.len() == 4);
    assert!(f.calls_with("--since-time=2026-10-22T10:00:02Z") >= 1, "{:?}", f.kube.calls());
    assert!(f.view_text().ends_with("connected"));
    assert!(f.app.det.view.stick, "follow keeps the end in view");
    // paused: nothing is asked, nothing is added
    f.click("Pause");
    let n = f.kube.calls().len();
    f.app.det.logs.polled = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..5 {
        f.app.handle(&Event::Tick, &f.cx);
        f.draw();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(f.kube.calls().len(), n, "paused");
    // the buffer is bounded
    let flood: String = (0..8000).map(|i| format!("2026-10-23T10:{:02}:{:02}.{:03}Z line {i}\n", (i / 3600) % 60, (i / 60) % 60, i % 1000)).collect();
    f.kube.answer("logs web-crash", &flood);
    f.click("Resume");
    f.app.det.logs.polled = Some(Instant::now() - Duration::from_secs(5));
    f.wait("the flood", |f| f.app.det.logs.buf.lines.len() >= iyakki_agent::k8sexp::LOG_CAP);
    assert_eq!(f.app.det.logs.buf.lines.len(), iyakki_agent::k8sexp::LOG_CAP, "bounded at 5000 lines");
    assert!(f.app.det.logs.buf.dropped > 0);
}

#[test]
fn saving_the_log_goes_through_the_host_save_dialog_in_window_mode() {
    let _guard = iyakki_agent::hostfiles::test_guard();
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.key('l');
    f.wait("logs", |f| f.app.det.logs.loaded && !f.app.det.logs.buf.lines.is_empty());
    iyakki_agent::hostfiles::set_enabled(true);
    f.click("Save");
    iyakki_agent::hostfiles::set_enabled(false);
    let (name, bytes) = crate::apps::App::take_host_save(&mut f.app).expect("the save dialog is asked for");
    assert_eq!(name, "shop-web-crash.log");
    let t = String::from_utf8(bytes).unwrap();
    assert!(t.contains("could not connect") && !t.contains("2026-10-22T10"), "{t}");
}

#[test]
fn exec_is_a_copied_command_not_a_terminal() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.click("Copy exec command");
    assert_eq!(f.clipboard(), "kubectl -n shop exec -it web-crash -c app -- sh", "a pod with two containers gets the first one named");
    assert!(f.calls_with("exec") == 0, "nothing is executed");
}

// ------------------------------------------------------------------------------------------------ changes

fn calls_after(f: &Fx, from: usize) -> Vec<String> {
    f.kube.calls()[from..].to_vec()
}

#[test]
fn changes_are_off_by_default_and_gated_by_role_and_switch() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    assert!(!f.app.allow_changes());
    assert!(!f.has_hit("Delete pod"), "no change buttons while read-only");
    let before = f.kube.calls().len();
    f.app.begin_action(2);
    assert!(f.app.pending.is_none(), "begin_action refuses while read-only");
    assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("Allow changes")));
    // switching on is remembered per cluster and audited
    f.click("Allow changes");
    assert!(f.app.allow_changes() && f.app.prefs.of("managed").allow_changes);
    assert!(f.audit_text().contains("k8s.allow_changes.on"));
    assert!(f.has_hit("Delete pod"));
    assert_eq!(calls_after(&f, before).iter().filter(|c| c.contains("delete")).count(), 0);
    // remembered by the next tile
    let again = K8sApp::with_rig(&f.cx, rig_for(f.kube.clone(), f.managed.clone(), Arc::new(AtomicUsize::new(0))));
    assert!(again.allow_changes());
    // not for another cluster
    f.click("Allow changes");
    assert!(!f.app.allow_changes());
}

#[test]
fn only_administrators_can_allow_changes() {
    for role in [Role::Operator, Role::User] {
        let mut f = fixture_with("k8s-role", role, 1280, 720, 1.0, Managed::Running);
        f.app.choose_scope(Scope::Ns("shop".into()));
        f.open_first("web-crash");
        f.click("Allow changes");
        assert!(!f.app.allow_changes(), "{role:?}");
        assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("administrators")), "{:?}", f.app.status);
        f.app.begin_action(2);
        assert!(f.app.pending.is_none());
        // even a saved switch from an earlier administrator session does not help
        f.app.prefs.edit("managed", |c| c.allow_changes = true);
        assert!(!f.app.allow_changes());
        assert!(f.calls_with("delete") == 0);
    }
}

#[test]
fn deleting_a_pod_asks_naming_the_pod_and_runs_the_exact_command() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.click("Allow changes");
    let before = f.kube.calls().len();
    f.click("Delete pod");
    let p = f.app.pending.as_ref().expect("a confirmation card");
    assert_eq!(p.action.title(), "Delete pod web-crash");
    assert!(p.action.warning(p.owned).contains("creates a new one"), "owned by a ReplicaSet: it comes back");
    assert!(f.has_hit("Delete") && f.has_hit("Cancel"));
    // cancelling runs nothing
    f.click("Cancel");
    assert!(f.app.pending.is_none());
    assert_eq!(calls_after(&f, before).iter().filter(|c| c.contains(" delete ")).count(), 0);
    // yes
    f.kube.answer("delete pod web-crash", "pod \"web-crash\" deleted\n");
    f.click("Delete pod");
    f.click("Delete");
    f.wait("the result", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    let del: Vec<String> = calls_after(&f, before).into_iter().filter(|c| c.contains(" delete ")).collect();
    assert_eq!(del, vec!["--request-timeout=25s -n shop delete pod web-crash --wait=false"], "{del:?}");
    let a = f.audit_text();
    assert!(a.contains("k8s.delete.pod") && a.contains("cluster=managed ns=shop Pod/web-crash") && a.contains("k8s.delete.pod.ok"), "{a}");
    assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("Delete pod web-crash done")), "{:?}", f.app.status);
}

#[test]
fn restart_and_scale_run_their_exact_commands() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.app.choose_kind(Kind::Deployments);
    f.open_first("web");
    f.click("Allow changes");
    let before = f.kube.calls().len();
    f.kube.answer("rollout restart", "deployment.apps/web restarted\n");
    f.kube.answer("rollout status", "deployment \"web\" successfully rolled out\n");
    f.click("Restart");
    f.press(Key::Enter);
    f.wait("restart", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    let c = calls_after(&f, before);
    assert!(c.iter().any(|x| x == "--request-timeout=25s -n shop rollout restart deployment/web"), "{c:?}");
    assert!(c.iter().any(|x| x == "--request-timeout=25s -n shop rollout status deployment/web --timeout=20s"), "{c:?}");
    // scale: the card starts at the current number and moves with the keys
    let before = f.kube.calls().len();
    f.kube.answer("scale deployment/web", "deployment.apps/web scaled\n");
    f.click("Scale");
    assert_eq!(f.app.pending.as_ref().unwrap().replicas, 3);
    f.key('-');
    f.key('-');
    f.click("\u{2212}");
    f.code(T_SCALE_UP);
    f.press(Key::Enter);
    f.wait("scale", |f| f.calls_with("--replicas=") >= 1);
    assert!(calls_after(&f, before).iter().any(|x| x == "--request-timeout=25s -n shop scale deployment/web --replicas=1"), "{:?}", calls_after(&f, before));
}

#[test]
fn deleting_other_resources_needs_the_name_typed() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.app.choose_kind(Kind::Services);
    f.open_first("front");
    f.click("Allow changes");
    let before = f.kube.calls().len();
    f.click("Delete");
    assert!(f.app.pending.is_some());
    f.press(Key::Enter);
    assert!(f.app.pending.is_some(), "Enter with nothing typed does not delete");
    for c in "fron".chars() {
        f.key(c);
    }
    f.press(Key::Enter);
    assert!(f.app.pending.is_some(), "a wrong name does not delete");
    f.key('t');
    f.kube.answer("delete services front", "service \"front\" deleted\n");
    f.press(Key::Enter);
    f.wait("delete", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    assert!(calls_after(&f, before).iter().any(|x| x == "--request-timeout=25s -n shop delete services front --wait=false"), "{:?}", calls_after(&f, before));
}

#[test]
fn cordon_and_uncordon_a_node() {
    let mut f = fixture();
    f.app.choose_kind(Kind::Nodes);
    f.open_first("node-a");
    f.click("Allow changes");
    let before = f.kube.calls().len();
    f.kube.answer("cordon node-a", "node/node-a cordoned\n");
    f.click("Cordon");
    f.key('y');
    f.wait("cordon", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    assert!(calls_after(&f, before).iter().any(|x| x == "--request-timeout=25s cordon node-a"), "{:?}", calls_after(&f, before));
    assert!(f.audit_text().contains("k8s.cordon") && f.audit_text().contains("Node/node-a"));
}

#[test]
fn a_failed_change_is_reported_and_the_switch_turned_off_mid_card_stops_it() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.click("Allow changes");
    f.kube.fail("delete pod web-crash", "Error from server (Forbidden): pods \"web-crash\" is forbidden");
    f.click("Delete pod");
    f.click("Delete");
    f.wait("failure", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("Forbidden")), "{:?}", f.app.status);
    assert!(f.audit_text().contains("k8s.delete.pod.failed"));
    // the switch goes off while a card is open: the yes does nothing
    f.app.panel = None;
    f.click("Delete pod");
    f.app.prefs.edit("managed", |c| c.allow_changes = false);
    let n = f.kube.calls().len();
    f.press(Key::Enter);
    assert_eq!(f.calls_with("delete pod").max(1), f.calls_with("delete pod"));
    assert_eq!(f.kube.calls().iter().skip(n).filter(|c| c.contains("delete")).count(), 0);
}

#[test]
fn there_is_a_button_to_the_ide_only_when_a_workspace_exists() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    assert!(!f.has_hit("Open in AI IDE"));
    f.app.ide_ok = true;
    f.draw();
    f.click("Open in AI IDE");
    assert_eq!(crate::apps::App::take_request(&mut f.app), Some(("ide".to_string(), String::new())));
}

// ------------------------------------------------------------------------------------------------ layout and input

fn assert_hits_inside(f: &Fx, what: &str) {
    let a = f.area;
    assert!(!f.g.hits.is_empty(), "{what}: nothing is clickable");
    for h in &f.g.hits {
        let r = h.rect;
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: `{}` {:?} is outside the app {:?}", h.label, r, a);
    }
}

#[test]
fn it_draws_at_every_size_with_every_hit_region_inside_the_app() {
    for (w, h, s) in [(800u32, 480u32, 1.0f32), (1280, 720, 1.0), (1920, 1080, 1.5), (360, 740, 1.0)] {
        let mut f = fixture_with("k8s-sizes", Role::Admin, w, h, s, Managed::Running);
        f.app.choose_scope(Scope::Ns("shop".into()));
        f.open_first("web-crash");
        f.click("Allow changes");
        f.draw();
        let mode = f.app.mode;
        assert_eq!(mode, if w >= 1000 { Mode::Wide } else if w >= 620 { Mode::Medium } else { Mode::Phone }, "{w}x{h}");
        assert_hits_inside(&f, &format!("{w}x{h} list"));
        // every pane and overlay
        if mode == Mode::Phone {
            f.press(Key::Enter);
            assert!(f.app.page_detail, "Enter drills down on a phone");
            assert_hits_inside(&f, &format!("{w}x{h} details"));
            assert!(f.has_hit("Back"));
        }
        for tab in [DTab::Summary, DTab::Yaml, DTab::Events, DTab::Related, DTab::Logs] {
            f.app.set_tab(tab);
            f.wait("tab", |f| !f.view_text().ends_with("\u{2026}"));
            assert_hits_inside(&f, &format!("{w}x{h} {tab:?}"));
        }
        f.app.set_tab(DTab::Summary);
        for key in ['n', 'k'] {
            if mode != Mode::Phone || !f.app.page_detail {
                f.key(key);
                assert_hits_inside(&f, &format!("{w}x{h} overlay {key}"));
                f.press(Key::Esc);
            }
        }
        f.app.page_detail = false;
        f.click("cluster");
        assert_hits_inside(&f, &format!("{w}x{h} clusters"));
        f.press(Key::Esc);
        f.app.begin_action(2);
        f.draw();
        assert_hits_inside(&f, &format!("{w}x{h} card"));
        assert!(f.g.rgba().iter().any(|b| *b != 0), "drawn");
    }
}

#[test]
fn it_works_inside_a_pane_of_half_the_screen() {
    // a 50:50 split of 1280x720: the right pane is 640 wide and offset
    let mut f = fixture_with("k8s-pane", Role::Admin, 1280, 720, 1.0, Managed::Running);
    f.area = R::new(640.0, 40.0, 640.0, 680.0);
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.draw();
    assert_eq!(f.app.mode, Mode::Medium);
    assert_hits_inside(&f, "right pane");
    f.area = R::new(0.0, 40.0, 400.0, 400.0);
    f.draw();
    assert_eq!(f.app.mode, Mode::Phone);
    assert_hits_inside(&f, "narrow pane");
    f.area = R::new(0.0, 0.0, 800.0, 240.0);
    f.draw();
    assert_hits_inside(&f, "short pane");
}

#[test]
fn the_wheel_and_the_trackpad_scroll_the_list_under_the_pointer() {
    let mut f = fixture_with("k8s-wheel", Role::Admin, 800, 480, 1.0, Managed::Running);
    let items: Vec<Value> = (0..200).map(|i| pod(&format!("p{i:03}"), "shop", "Running", true, 0, json!({"running":{}}), 60)).collect();
    f.kube.answer("-n shop get pods -o json", &list(items));
    f.kube.answer("get pods p199 -o json", &pod("p199", "shop", "Running", true, 0, json!({"running":{}}), 60).to_string());
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.wait("the list", |f| f.app.listing().is_some_and(|l| l.total == 200));
    f.draw();
    assert!(f.app.res.fit > 0 && f.app.res.fit < 200);
    let (x, y) = (f.app.res.rect.x + 40.0, f.app.res.rect.y + 40.0);
    f.send(Event::MouseMove(x as i32, y as i32));
    assert_eq!(f.app.res.top, 0);
    f.send(Event::Scroll(-1));
    assert_eq!(f.app.res.top, 3, "one wheel notch down moves three rows");
    f.send(Event::Scroll(1));
    assert_eq!(f.app.res.top, 0, "and back up");
    // a trackpad sends many small events
    for _ in 0..10 {
        f.send(Event::Scroll(-1));
    }
    assert_eq!(f.app.res.top, 30);
    for _ in 0..1000 {
        f.app.handle(&Event::Scroll(-1), &f.cx);
    }
    f.draw();
    assert_eq!(f.app.res.top, 200 - f.app.res.fit, "stops at the end");
    // scrolled off the selection: the click regions follow the rows
    let hit = f.g.hits.iter().find(|h| h.label == "p199").expect("the last row is clickable");
    assert!(hit.rect.y >= f.app.res.rect.y && hit.rect.y + hit.rect.h <= f.app.res.rect.y + f.app.res.rect.h + 0.5);
    assert_hits_inside(&f, "scrolled");
    // over the details the wheel scrolls the text instead
    f.open_first("p199");
    let v = f.app.det.view.rect;
    f.send(Event::MouseMove((v.x + 10.0) as i32, (v.y + 10.0) as i32));
    f.app.set_tab(DTab::Yaml);
    f.wait("yaml", |f| f.view_text().contains("kind: Pod"));
    let top0 = f.app.det.view.top;
    f.send(Event::Scroll(-1));
    assert!(f.app.det.view.top > top0 || f.app.det.view.lines.len() <= f.app.det.view.fit);
}

#[test]
fn the_keyboard_moves_opens_and_goes_back() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    f.press(Key::Down);
    f.press(Key::Down);
    assert_eq!(f.app.res.sel, 2);
    f.press(Key::Home);
    assert_eq!(f.app.res.sel, 0);
    f.press(Key::End);
    assert_eq!(f.app.res.sel, 5);
    f.press(Key::PageUp);
    assert!(f.app.res.sel < 5);
    f.press(Key::Home);
    f.press(Key::Enter);
    assert_eq!(f.app.focus, Focus::Detail);
    f.press(Key::Esc);
    assert_eq!(f.app.focus, Focus::Res);
    // kinds with k: arrows and Enter choose
    f.key('k');
    assert_eq!(f.app.focus, Focus::Kinds);
    f.press(Key::Down);
    f.press(Key::Enter);
    assert_eq!(f.app.kind, Kind::Deployments);
    // Esc at the top leaves the tile
    assert!(matches!(f.send(Event::Key(Key::Esc)), crate::apps::Flow::Back));
}

#[test]
fn number_keys_do_nothing() {
    let mut f = fixture();
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    let before = (f.app.scope.clone(), f.app.kind, f.app.res.sel, f.app.focus, f.app.overlay, f.app.det.tab, f.kube.calls().len());
    for c in '0'..='9' {
        f.key(c);
    }
    assert_eq!(before.0, f.app.scope);
    assert_eq!((before.1, before.2, before.3, before.4, before.5), (f.app.kind, f.app.res.sel, f.app.focus, f.app.overlay, f.app.det.tab));
    assert!(f.app.pending.is_none());
}

#[test]
fn a_phone_goes_from_the_list_to_the_details_and_back() {
    let mut f = fixture_with("k8s-phone", Role::Admin, 360, 740, 1.0, Managed::Running);
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.list_loaded();
    f.draw();
    assert_eq!(f.app.mode, Mode::Phone);
    assert!(f.has_hit("namespace") && f.has_hit("kind") && f.has_hit("cluster"), "drop-downs in the header");
    f.code(T_RES_ROW + 2);
    assert!(f.app.page_detail);
    assert!(f.has_hit("Back"));
    f.click("Back");
    assert!(!f.app.page_detail);
    f.click("namespace");
    assert_eq!(f.app.overlay, Some(Overlay::Namespaces));
    assert!(f.has_hit("shop"));
    f.press(Key::Esc);
    assert_eq!(f.app.overlay, None);
}

#[test]
fn the_command_palette_of_the_help_page_lists_the_keys() {
    let f = fixture();
    let h = crate::apps::App::help(&f.app).join("\n");
    for k in ["/", "n:", "k:", "l Logs", "y YAML", "r:", "Allow changes"] {
        assert!(h.contains(k), "{k}");
    }
}

/// Pictures of the tile for a person to look at: `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib k8s_tile::tests::pictures -- --ignored`.
#[test]
#[ignore]
fn pictures() {
    let Ok(dir) = std::env::var("IYAKKI_PICTURES") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, w, h, s) in [("wide", 1280u32, 720u32, 1.0f32), ("small", 800, 480, 1.0), ("phone", 360, 740, 1.0), ("full", 1920, 1080, 1.5)] {
        let mut f = fixture_with("k8s-pictures", Role::Admin, w, h, s, Managed::Running);
        f.app.choose_scope(Scope::Ns("shop".into()));
        f.open_first("web-crash");
        f.click("Allow changes");
        f.key('y');
        f.wait("yaml", |f| f.view_text().contains("kind: Pod"));
        f.g.pm.save_png(dir.join(format!("{name}-yaml.png"))).unwrap();
        f.key('l');
        f.wait("logs", |f| f.view_text().contains("could not connect"));
        f.g.pm.save_png(dir.join(format!("{name}-logs.png"))).unwrap();
        f.key('s');
        f.draw();
        f.g.pm.save_png(dir.join(format!("{name}-summary.png"))).unwrap();
        f.click("Delete pod");
        f.g.pm.save_png(dir.join(format!("{name}-card.png"))).unwrap();
    }
    let mut f = fixture_with("k8s-pictures2", Role::Admin, 1280, 720, 1.0, Managed::Stopped);
    f.draw();
    f.g.pm.save_png(dir.join("stopped.png")).unwrap();
}

// ------------------------------------------------------------------------------------------------ the cluster switcher in the header

/// Two more clusters in the person's registry (a remote one and a production-looking one); saved, so the switcher shows.
fn add_clusters(f: &mut Fx) {
    let dir = f.cx.services.dir().to_path_buf();
    let user = f.cx.session.user.clone();
    let mut r = iyakki_agent::clusters::Registry::load(&dir, &user);
    r.add_remote("gpu-box", iyakki_agent::clusters::Kind::Remote, "/fake/gpu.yaml", "gpu", "").unwrap();
    r.add_remote("prod-eu", iyakki_agent::clusters::Kind::Remote, "/fake/prod.yaml", "", "").unwrap();
    r.save(&dir, &user).unwrap();
    f.app = K8sApp::with_rig(&f.cx, rig_for(f.kube.clone(), f.managed.clone(), f.starts.clone()));
    f.draw();
}

fn switcher_hits(f: &Fx) -> Vec<(String, R)> {
    f.g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_SWITCH..T_SWITCH + 1000).contains(&n))).map(|h| (h.label.clone(), h.rect)).collect()
}

#[test]
fn one_cluster_keeps_the_plain_cluster_box_and_no_switcher() {
    let mut f = fixture();
    f.draw();
    assert!(switcher_hits(&f).is_empty(), "nothing of the switcher is clickable with one cluster");
    assert!(f.has_hit("cluster"), "the old list box is still there");
    assert!(!f.app.host.visible());
    assert_eq!(f.app.phrase(), "cluster \"This machine\" (local)");
}

#[test]
fn the_switcher_sits_in_the_header_at_every_size_without_covering_the_other_controls() {
    for (w, h, s) in [(360u32, 740u32, 1.0f32), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0)] {
        let mut f = fixture_with("k8s-switch-sizes", Role::Admin, w, h, s, Managed::Running);
        add_clusters(&mut f);
        f.draw();
        let sw = switcher_hits(&f);
        assert_eq!(sw.len(), 1, "{w}x{h}: the closed box only: {sw:?}");
        let b = sw[0].1;
        assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= w as f32 + 0.5 && b.y + b.h <= h as f32 + 0.5, "{w}x{h}: {b:?} inside the tile");
        assert!(sw[0].0.contains("cluster switcher") && sw[0].0.contains("This machine") || sw[0].0.contains("Dev cluster") || sw[0].0.contains("local"), "{}", sw[0].0);
        for other in ["Refresh", "Allow changes"] {
            if let Some(o) = f.g.hits.iter().find(|x| x.label == other) {
                let (a, c) = (b, o.rect);
                let overlap = a.x < c.x + c.w - 0.5 && c.x < a.x + a.w - 0.5 && a.y < c.y + c.h - 0.5 && c.y < a.y + a.h - 0.5;
                assert!(!overlap, "{w}x{h}: the switcher {a:?} overlaps `{other}` {c:?}");
            }
        }
        // open: every hit stays inside the tile, "Manage clusters..." is reachable
        f.code(T_SWITCH);
        assert!(f.app.host.sw.open);
        assert!(f.has_hit("Manage clusters..."), "{w}x{h}");
        for hh in &f.g.hits {
            let r = hh.rect;
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: `{}` {r:?} outside", hh.label);
        }
    }
}

#[test]
fn choosing_a_cluster_in_the_switcher_changes_what_the_tile_reads_and_survives_a_new_tile() {
    let mut f = fixture();
    add_clusters(&mut f);
    assert_eq!(f.app.entry().key, "managed");
    assert_eq!(f.app.entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), vec!["Dev cluster (managed k3s)", "gpu-box", "prod-eu"]);
    f.code(T_SWITCH);
    let gpu_row = f.g.hits.iter().find(|h| h.label.starts_with("gpu-box")).expect("a row for gpu-box").ev.clone();
    f.send(gpu_row);
    assert_eq!(f.app.entry().key, "profile:gpu-box");
    assert_eq!((f.app.entry().target.kubeconfig.as_str(), f.app.entry().target.context.as_str()), ("/fake/gpu.yaml", "gpu"), "kubectl now points at that cluster's kubeconfig and context");
    assert!(!f.app.host.sw.open);
    let reg = iyakki_agent::clusters::Registry::load(f.cx.services.dir(), &f.cx.session.user);
    assert_eq!(reg.active_entry().id, "gpu-box", "the account's active cluster is the one chosen (the AI IDE and Setup follow it)");
    assert!(f.audit_text().contains("cluster.switch") && f.audit_text().contains("cluster \"gpu-box\" (remote)"));
    let again = K8sApp::with_rig(&f.cx, rig_for(f.kube.clone(), f.managed.clone(), f.starts.clone()));
    assert_eq!(again.entry().key, "profile:gpu-box", "a new tile starts on the active cluster");
    // someone switches in Settings -> Clusters: the open tile follows
    iyakki_agent::clusters::switch(f.cx.services.dir(), f.cx.services.dir(), &f.cx.session.user, Role::Admin, "local").unwrap();
    f.app.managed_at = None;
    f.wait("the follow", |f| f.app.entry().key == "managed");
}

#[test]
fn manage_clusters_asks_the_shell_for_settings_clusters_and_a_guest_switch_is_refused() {
    let mut f = fixture();
    add_clusters(&mut f);
    f.code(T_SWITCH);
    f.click("Manage clusters...");
    assert_eq!(crate::apps::App::take_request(&mut f.app), Some(("settings".to_string(), "tab:clusters".to_string())));
    // the host refuses a guest (no rows for a guest at all)
    let mut g = fixture_with("k8s-switch-guest", Role::Guest, 1280, 720, 1.0, Managed::Running);
    add_clusters(&mut g);
    assert!(!g.app.host.visible(), "a guest sees no switcher");
}

#[test]
fn every_change_card_audit_line_and_allow_message_names_the_cluster() {
    let mut f = fixture();
    add_clusters(&mut f);
    f.app.select_cluster(1, true);
    f.app.choose_scope(Scope::Ns("shop".into()));
    f.open_first("web-crash");
    f.click("Allow changes");
    let said = f.app.status.as_ref().unwrap().0.clone();
    assert!(said.contains("Changes are allowed on cluster \"gpu-box\" (remote)"), "{said}");
    assert!(f.audit_text().contains("k8s.allow_changes.on") && f.audit_text().contains("on cluster \"gpu-box\" (remote)"));
    f.click("Delete pod");
    let card = f.app.card_cluster(&f.app.pending.as_ref().unwrap().action);
    assert_eq!(card, "Delete pod web-crash from cluster \"gpu-box\" (remote)");
    f.kube.answer("delete pod web-crash", "pod \"web-crash\" deleted\n");
    f.click("Delete");
    f.wait("the result", |f| f.app.panel.as_ref().is_some_and(|p| p.finished_at.is_some()));
    let a = f.audit_text();
    assert!(a.contains("cluster=profile:gpu-box ns=shop Pod/web-crash on cluster \"gpu-box\" (remote)"), "{a}");
    // restart and scale say "on"
    f.app.choose_kind(Kind::Deployments);
    f.open_first("web");
    f.click("Restart");
    assert_eq!(f.app.card_cluster(&f.app.pending.as_ref().unwrap().action), "Restart deployment web on cluster \"gpu-box\" (remote)");
}

#[test]
fn a_production_looking_cluster_stays_read_only_in_the_tile_whatever_the_switcher_says() {
    let mut f = fixture();
    add_clusters(&mut f);
    f.app.select_cluster(2, true);
    assert_eq!(f.app.entry().label, "prod-eu");
    assert!(f.app.change_block().is_some_and(|m| m.contains("looks like production")));
}

// ------------------------------------------------------------------------------------------------ transient failures

use iyakki_agent::k8sretry::{FakeTimer, Retry, QUICK_RETRY_MS};

/// A tile whose feed runs on a fake clock: waits between retries cost nothing and the test moves time.
fn flaky() -> (Fx, Arc<FakeTimer>) {
    let mut f = fixture_with("k8s-flaky", Role::Admin, 1280, 720, 1.0, Managed::Running);
    let timer = FakeTimer::new();
    f.app.use_retry(Retry::new(&QUICK_RETRY_MS, timer.clone()));
    f.list_loaded();
    f.wait("the namespaces", |f| f.app.feed.ns().is_some());
    (f, timer)
}

fn fails_now(f: &Fx, what: &str) {
    f.kube.fail("get", what);
}

fn works_again(f: &Fx) {
    f.kube.rules.lock().unwrap().remove(0);
}

/// Move the fake clock until the feed has made `n` more fetches.
fn more_fetches(f: &mut Fx, t: &FakeTimer, n: u64) {
    let target = f.app.feed.fetches() + n;
    while f.app.feed.fetches() < target {
        let now = f.app.feed.fetches();
        t.advance(16_000);
        f.wait("a background try", |f| f.app.feed.fetches() > now);
    }
}

#[test]
fn a_transient_failure_keeps_the_list_on_screen_with_a_dim_updating_line() {
    let (mut f, _t) = flaky();
    let rows = f.rows().len();
    assert!(rows > 0 && f.app.stale_text().is_none());
    fails_now(&f, "Error from server: etcdserver: request timed out");
    f.key('r');
    f.wait("the stale line", |f| f.app.stale_text().is_some());
    let line = f.app.stale_text().unwrap();
    assert!(line.contains("Updating") && line.contains("the cluster did not answer, retrying") && line.contains("s old"), "{line}");
    // the table is still there and no banner replaced it
    assert_eq!(f.rows().len(), rows);
    assert!(f.app.listing().is_some_and(|l| l.cluster == iyakki_agent::k8s::Cluster::Ok));
    assert!(f.app.state_text().0 == "Reading from the cluster\u{2026}", "no banner: {:?}", f.app.state_text().0);
    assert!(f.has_hit("Refresh"));
}

#[test]
fn after_the_window_the_banner_says_when_it_last_worked_and_the_log_has_the_error() {
    let (mut f, t) = flaky();
    fails_now(&f, "Unable to connect to the server: net/http: TLS handshake timeout");
    f.key('r');
    f.wait("the stale line", |f| f.app.stale_text().is_some());
    while !f.app.feed.status().banner {
        more_fetches(&mut f, &t, 1);
    }
    f.draw();
    let (title, text, _) = f.app.state_text();
    assert_eq!(title, "The cluster is not reachable");
    assert!(text.contains("TLS handshake timeout"), "the real error stays visible: {text}");
    assert!(text.contains("Last worked") && text.contains(" s ago.") && text.contains("Retrying every") && text.contains("Press Refresh to retry now"), "{text}");
    assert!(f.has_hit("Refresh") && f.has_hit("Show log"));
    // the log shows what happened and copies
    f.click("Show log");
    assert!(f.app.show_log);
    f.draw();
    f.code(T_LOG_COPY);
    let log = f.clipboard();
    assert!(log.contains("get namespaces") && log.contains("TLS handshake timeout") && log.contains("transient, try 4"), "{log}");
    assert!(!log.contains("kubeconfig=/") && !log.contains("--context="), "no secrets: {log}");
    f.press(Key::Esc);
    assert!(!f.app.show_log, "Esc closes the log first");
    // the cluster comes back: the banner goes away by itself
    works_again(&f);
    more_fetches(&mut f, &t, 1);
    f.wait("the data", |f| f.app.state_text().0 == "Reading from the cluster\u{2026}" && f.app.listing().is_some_and(|l| l.cluster == iyakki_agent::k8s::Cluster::Ok));
    assert!(f.app.stale_text().is_none() && !f.rows().is_empty());
}

#[test]
fn a_persistent_failure_shows_the_banner_at_once_with_no_stale_wait() {
    let (mut f, _t) = flaky();
    fails_now(&f, "Error from server (Forbidden): pods is forbidden: User \"x\" cannot list resource \"pods\"");
    f.key('r');
    f.wait("the banner", |f| f.app.state_text().0 == "This account may not read the cluster");
    assert!(f.app.stale_text().is_none());
}

#[test]
fn a_refused_connection_of_the_managed_k3s_is_a_quiet_retry_not_a_banner() {
    let (mut f, t) = flaky();
    fails_now(&f, "The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?");
    f.key('r');
    f.wait("the stale line", |f| f.app.stale_text().is_some());
    let _ = &t;
    assert!(f.app.feed.status().failing && !f.app.feed.status().banner, "a refused connection of the managed k3s is transient");
    // the managed state says Starting: the tile says so in plain words and the feed waits longer before any banner
    *f.managed.lock().unwrap() = Managed::Starting { waiting_net: false };
    f.app.refresh_entries();
    f.draw();
    assert_eq!(f.app.state_text().0, "The dev cluster is starting");
    f.app.handle(&Event::Tick, &f.cx);
    assert!(f.app.feed.status().failing);
}

#[test]
fn refresh_and_r_end_a_retry_wait_at_once() {
    let (mut f, t) = flaky();
    fails_now(&f, "Unable to connect to the server: EOF");
    f.key('r');
    f.wait("the stale line", |f| f.app.stale_text().is_some());
    more_fetches(&mut f, &t, 3);
    assert!(f.app.feed.status().fails >= 3);
    // a wait is cut short by Refresh (the button does the same as r)
    t.cancel_next.store(true, Ordering::SeqCst);
    let n = f.app.feed.fetches();
    t.advance(20_000);
    f.wait("the wait was cancelled", |_| !t.cancel_next.load(Ordering::SeqCst));
    f.code(T_REFRESH);
    // no time passes on the fake clock, yet the feed asks again: Refresh did not wait for the backoff (the Tracker test covers the reset)
    f.wait("a refresh right after", |f| f.app.feed.fetches() > n);
}

#[test]
fn show_log_is_empty_and_calm_when_nothing_failed() {
    let (mut f, _t) = flaky();
    f.click("Show log");
    f.draw();
    assert!(f.app.show_log && f.has_hit("Copy") && f.has_hit("Hide log"));
    f.code(T_LOG_COPY);
    assert!(f.clipboard().contains("No kubectl problems"));
    f.key('L');
    assert!(!f.app.show_log);
}
