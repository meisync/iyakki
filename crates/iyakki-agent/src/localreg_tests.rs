use super::*;
use crate::ocipush::tests::FakeRegistry;
use std::sync::Mutex;

/// A cluster that remembers what was applied to it: no real kubectl, no real cluster.
#[derive(Default)]
pub struct St {
    pub down: bool,
    pub ready_node: bool,
    pub storage: Option<String>,
    pub ns: bool,
    pub svc_port: Option<u16>,
    pub size: String,
    pub ready: bool,
    pub rollout_ok: bool,
    pub other_ports: Vec<u16>,
    pub pods: String,
    pub apply_error: Option<String>,
    pub manifest: String,
}

pub struct Cluster {
    pub st: Mutex<St>,
    pub calls: Mutex<Vec<String>>,
}

impl Cluster {
    pub fn healthy() -> Arc<Cluster> {
        Arc::new(Cluster { st: Mutex::new(St { ready_node: true, storage: Some("local-path".into()), rollout_ok: true, pods: r#"{"items":[]}"#.into(), ..Default::default() }), calls: Mutex::new(vec![]) })
    }
    pub fn lines(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn did(&self, what: &str) -> bool {
        self.lines().iter().any(|l| l.contains(what))
    }
}

impl Runner for Cluster {
    fn run(&self, _cwd: &std::path::Path, argv: &[String], _env: &[(String, String)], _t: Duration, _cap: usize, _s: Option<crate::wsexec::Sink>) -> Out {
        let line = argv.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let mut st = self.st.lock().unwrap();
        let ok = |s: String| Out { code: Some(0), stdout: s, ..Default::default() };
        let bad = |s: &str| Out { code: Some(1), stderr: s.into(), ..Default::default() };
        if st.down {
            return bad("The connection to the server localhost:6443 was refused - did you specify the right host or port? connection refused");
        }
        let has = |w: &str| line.contains(w);
        if has("get nodes") {
            return ok(serde_json::json!({ "items": [{ "metadata": { "name": "n1" }, "status": { "conditions": [{ "type": "Ready", "status": if st.ready_node { "True" } else { "False" } }] } }] }).to_string());
        }
        if has("get storageclass") {
            let items: Vec<Value> = st.storage.iter().map(|n| serde_json::json!({ "metadata": { "name": n, "annotations": { "storageclass.kubernetes.io/is-default-class": "true" } } })).collect();
            return ok(serde_json::json!({ "items": items }).to_string());
        }
        if has("get services -A") {
            let mut items: Vec<Value> = st.other_ports.iter().map(|p| serde_json::json!({ "metadata": { "name": "x", "namespace": "other" }, "spec": { "ports": [{ "nodePort": p }] } })).collect();
            if let Some(p) = st.svc_port {
                items.push(serde_json::json!({ "metadata": { "name": "registry", "namespace": "iyakki-registry" }, "spec": { "ports": [{ "nodePort": p }] } }));
            }
            return ok(serde_json::json!({ "items": items }).to_string());
        }
        if has("get service registry") {
            return match st.svc_port {
                Some(p) => ok(serde_json::json!({ "spec": { "ports": [{ "port": 5000, "nodePort": p }] } }).to_string()),
                None => bad("Error from server (NotFound): services \"registry\" not found"),
            };
        }
        if has("get deployment registry") {
            return ok(serde_json::json!({ "status": { "readyReplicas": if st.ready { 1 } else { 0 } } }).to_string());
        }
        if has("get pvc registry-data") {
            return if st.size.is_empty() { bad("NotFound") } else { ok(st.size.clone()) };
        }
        if has("get pods -n iyakki-registry") {
            return ok(st.pods.clone());
        }
        if has("get namespace iyakki-registry") {
            return if st.ns { ok("namespace/iyakki-registry".into()) } else { bad("Error from server (NotFound): namespaces \"iyakki-registry\" not found") };
        }
        if has("create namespace") {
            st.ns = true;
            return ok("namespace/iyakki-registry created".into());
        }
        if has("apply -f") {
            if let Some(e) = st.apply_error.clone() {
                return bad(&e);
            }
            let file = argv.iter().position(|a| a == "-f").and_then(|i| argv.get(i + 1)).cloned().unwrap_or_default();
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            st.svc_port = text.lines().find_map(|l| l.trim().strip_prefix("nodePort: ")).and_then(|p| p.parse().ok());
            st.size = text.lines().find_map(|l| l.trim().strip_prefix("storage: ")).unwrap_or("").to_string();
            st.manifest = text;
            st.ready = st.rollout_ok;
            return ok("persistentvolumeclaim/registry-data created\ndeployment.apps/registry created\nservice/registry created".into());
        }
        if has("rollout status") {
            return if st.rollout_ok { ok("deployment \"registry\" successfully rolled out".into()) } else { bad("error: timed out waiting for the condition") };
        }
        if has("delete namespace") {
            st.ns = false;
            st.svc_port = None;
            st.ready = false;
            return ok("namespace \"iyakki-registry\" deleted".into());
        }
        bad(&format!("unexpected: {line}"))
    }
    fn has(&self, p: &str) -> bool {
        p == "kubectl"
    }
}

fn target() -> Target {
    Target { name: "iyakki-dev".into(), kubeconfig: "/data/k3s-dev/kubeconfig".into(), context: String::new(), node_host: "localhost".into() }
}

fn opts(dir: &std::path::Path) -> Opts {
    Opts { size_gi: 10, node_port: 0, wait: Wait::NONE, work_dir: dir.to_path_buf() }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-localreg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn rt(c: &Arc<Cluster>) -> Arc<dyn Runner> {
    c.clone()
}

#[test]
fn the_dependency_names_what_is_missing() {
    let c = Cluster::healthy();
    assert!(check_dependency(&rt(&c), &target()).met());
    c.st.lock().unwrap().down = true;
    let Dependency::Missing(m) = check_dependency(&rt(&c), &target()) else { panic!() };
    assert!(m.contains("does not answer") && m.contains("Dev cluster"), "{m}");
    let c = Cluster::healthy();
    c.st.lock().unwrap().ready_node = false;
    let Dependency::Missing(m) = check_dependency(&rt(&c), &target()) else { panic!() };
    assert!(m.contains("no node is Ready"), "{m}");
    let c = Cluster::healthy();
    c.st.lock().unwrap().storage = None;
    let Dependency::Missing(m) = check_dependency(&rt(&c), &target()) else { panic!() };
    assert!(m.contains("StorageClass") && m.contains("local-path"), "{m}");
}

#[test]
fn nothing_is_created_when_the_dependency_is_not_met() {
    let d = tmp("dep");
    let c = Cluster::healthy();
    c.st.lock().unwrap().storage = None;
    let reg = FakeRegistry::default();
    let e = create(&rt(&c), &reg, &target(), &opts(&d), &mut |_| {}).unwrap_err();
    assert!(e.contains("StorageClass"), "{e}");
    assert!(!c.did("create namespace") && !c.did("apply"), "{:?}", c.lines());
    assert!(!d.exists(), "no file was written");
}

#[test]
fn create_builds_the_objects_and_needs_no_node_change() {
    let d = tmp("create");
    let c = Cluster::healthy();
    let reg = FakeRegistry::default();
    let mut said = Vec::new();
    let info = create(&rt(&c), &reg, &target(), &opts(&d), &mut |l| said.push(l.to_string())).unwrap();
    assert_eq!((info.push.as_str(), info.pull.as_str(), info.node_port, info.ready), ("localhost:30500", "localhost:30500", 30500, true));
    assert_eq!(info.storage, "10Gi");
    assert_eq!(info.images, Some(0));
    let m = c.st.lock().unwrap().manifest.clone();
    for want in ["image: docker.io/library/registry:2", "kind: PersistentVolumeClaim", "storageClassName: local-path", "storage: 10Gi", "type: NodePort", "nodePort: 30500", "kind: Deployment", "mountPath: /var/lib/registry", "allowPrivilegeEscalation: false"] {
        assert!(m.contains(want), "{want} missing in\n{m}");
    }
    // the order: namespace, then objects, then the rollout
    let lines = c.lines();
    let pos = |w: &str| lines.iter().position(|l| l.contains(w)).unwrap_or_else(|| panic!("{w} not called: {lines:?}"));
    assert!(pos("create namespace iyakki-registry") < pos("apply -f") && pos("apply -f") < pos("rollout status"));
    assert!(lines.iter().any(|l| l.contains("apply -f") && l.contains("-n iyakki-registry")));
    // no root, no node files, no restart of anything
    assert!(!lines.iter().any(|l| l.contains("systemctl") || l.contains("registries.yaml") || l.contains("restart") || l.contains("sudo") || l.contains("podman") || l.contains("docker")), "{lines:?}");
    assert!(said.iter().any(|l| l.starts_with("[1/6]")) && said.last().unwrap().contains("pods pull localhost:30500"), "{said:?}");
    assert!(said.iter().any(|l| l.starts_with("[6/6] done: registry ready")), "{said:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_volume_size_is_the_choice() {
    let d = tmp("size");
    let c = Cluster::healthy();
    let reg = FakeRegistry::default();
    let o = Opts { size_gi: 20, ..opts(&d) };
    let info = create(&rt(&c), &reg, &target(), &o, &mut |_| {}).unwrap();
    assert_eq!(info.storage, "20Gi");
    assert!(c.st.lock().unwrap().manifest.contains("storage: 20Gi"));
    assert!(create(&rt(&Cluster::healthy()), &reg, &target(), &Opts { size_gi: 0, ..opts(&d) }, &mut |_| {}).unwrap_err().contains("not a volume size"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_second_click_changes_nothing_and_says_what_is_there() {
    let d = tmp("again");
    let c = Cluster::healthy();
    let reg = FakeRegistry::default();
    create(&rt(&c), &reg, &target(), &opts(&d), &mut |_| {}).unwrap();
    // two images were pushed since
    let img = ocipush::build_image(&ocipush::ImageSpec { file: "iyakki", contents: b"x", arch: "arm64", user: "1:1", args: vec![], port: 8080 }).unwrap();
    ocipush::push(&reg, "localhost:30500", "iyakki-platform/iyakki-store-server", "1.0.0", &img, &mut |_| {}).unwrap();
    ocipush::push(&reg, "localhost:30500", "iyakki-platform/iyakki-store-server", "1.0.1", &img, &mut |_| {}).unwrap();
    c.calls.lock().unwrap().clear();
    let mut said = Vec::new();
    let info = create(&rt(&c), &reg, &target(), &opts(&d), &mut |l| said.push(l.to_string())).unwrap();
    assert_eq!(info.images, Some(2));
    assert!(!c.did("apply") && !c.did("create namespace") && !c.did("rollout"), "{:?}", c.lines());
    assert!(said.iter().any(|l| l.contains("exists already") && l.contains("registry ready") && l.contains("localhost:30500") && l.contains("used by 2 images")), "{said:?}");
    let s = inspect(&rt(&c), &reg, &target());
    assert_eq!(status_line(&s), "registry ready \u{b7} localhost:30500 \u{b7} used by 2 images");
    assert_eq!(status_line(&State::Absent), "no local registry yet");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_registry_that_exists_but_is_not_ready_is_applied_again() {
    let d = tmp("starting");
    let c = Cluster::healthy();
    {
        let mut st = c.st.lock().unwrap();
        st.ns = true;
        st.svc_port = Some(30500);
        st.size = "10Gi".into();
        st.pods = r#"{"items":[{"status":{"containerStatuses":[{"state":{"waiting":{"reason":"ImagePullBackOff"}}}]}}]}"#.into();
    }
    let s = inspect(&rt(&c), &FakeRegistry::default(), &target());
    assert!(matches!(&s, State::Starting(_, why) if why.contains("Docker Hub")), "{s:?}");
    assert!(status_line(&s).starts_with("registry starting"));
    let mut said = Vec::new();
    create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |l| said.push(l.to_string())).unwrap();
    assert!(c.did("apply -f") && !c.did("create namespace"), "{:?}", c.lines());
    assert!(said.iter().any(|l| l.contains("not ready") && l.contains("applying it again")), "{said:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ports_in_use_are_avoided_or_reported() {
    let d = tmp("ports");
    let c = Cluster::healthy();
    c.st.lock().unwrap().other_ports = vec![30500, 30501];
    let info = create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |_| {}).unwrap();
    assert_eq!(info.node_port, 30502);
    let c = Cluster::healthy();
    c.st.lock().unwrap().other_ports = vec![30777];
    let e = create(&rt(&c), &FakeRegistry::default(), &target(), &Opts { node_port: 30777, ..opts(&d) }, &mut |_| {}).unwrap_err();
    assert!(e.contains("30777") && e.contains("another Service") && e.contains("next free"), "{e}");
    assert!(!c.did("apply"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn failures_say_what_to_do() {
    let d = tmp("fail");
    // the cluster is down
    let c = Cluster::healthy();
    c.st.lock().unwrap().down = true;
    assert!(create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |_| {}).unwrap_err().contains("does not answer"));
    // forbidden
    let c = Cluster::healthy();
    c.st.lock().unwrap().apply_error = Some("Error from server (Forbidden): deployments.apps is forbidden: User \"x\" cannot create".into());
    let e = create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |_| {}).unwrap_err();
    assert!(e.contains("kubectl apply failed") && e.contains("refused the request") && e.contains("administrator"), "{e}");
    // the image cannot be pulled
    let c = Cluster::healthy();
    {
        let mut st = c.st.lock().unwrap();
        st.rollout_ok = false;
        st.pods = r#"{"items":[{"status":{"containerStatuses":[{"state":{"waiting":{"reason":"ErrImagePull"}}}]}}]}"#.into();
    }
    let e = create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |_| {}).unwrap_err();
    assert!(e.contains("did not become ready") && e.contains("Docker Hub") && e.contains("Retry"), "{e}");
    // the volume is not provisioned
    let c = Cluster::healthy();
    {
        let mut st = c.st.lock().unwrap();
        st.rollout_ok = false;
        st.pods = r#"{"items":[{"status":{"conditions":[{"type":"PodScheduled","status":"False","message":"0/1 nodes are available: pod has unbound immediate PersistentVolumeClaims."}]}}]}"#.into();
    }
    let e = create(&rt(&c), &FakeRegistry::default(), &target(), &opts(&d), &mut |_| {}).unwrap_err();
    assert!(e.contains("volume is not provisioned"), "{e}");
    // the pod runs but the address does not answer from here
    let c = Cluster::healthy();
    let reg = FakeRegistry::default();
    *reg.down.lock().unwrap() = true;
    let e = create(&rt(&c), &reg, &target(), &opts(&d), &mut |_| {}).unwrap_err();
    assert!(e.contains("localhost:30500 does not answer from this machine") && e.contains("firewall") && e.contains("Retry"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_cluster_on_another_machine_is_pushed_to_at_its_address_and_pulled_as_localhost() {
    let d = tmp("remote");
    let c = Cluster::healthy();
    let t = Target { name: "lab".into(), kubeconfig: "/k".into(), context: "lab".into(), node_host: "10.9.8.7".into() };
    let info = create(&rt(&c), &FakeRegistry::default(), &t, &opts(&d), &mut |_| {}).unwrap();
    assert_eq!((info.push.as_str(), info.pull.as_str()), ("10.9.8.7:30500", "localhost:30500"));
    assert!(c.lines().iter().any(|l| l.contains("--context lab")), "{:?}", c.lines());
    assert!(describe_plan(10, &t).contains("10.9.8.7:<port>") && describe_plan(10, &t).contains("NOT restarted"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn removing_asks_for_the_name_and_deletes_only_the_namespace() {
    let d = tmp("remove");
    let c = Cluster::healthy();
    let reg = FakeRegistry::default();
    create(&rt(&c), &reg, &target(), &opts(&d), &mut |_| {}).unwrap();
    c.calls.lock().unwrap().clear();
    let e = remove(&rt(&c), &reg, &target(), "yes", &mut |_| {}).unwrap_err();
    assert!(e.contains("Type iyakki-registry"), "{e}");
    assert!(!c.did("delete"), "nothing is deleted without the typed name");
    let mut said = Vec::new();
    remove(&rt(&c), &reg, &target(), "iyakki-registry", &mut |l| said.push(l.to_string())).unwrap();
    let del: Vec<String> = c.lines().into_iter().filter(|l| l.contains("delete")).collect();
    assert_eq!(del.len(), 1);
    assert!(del[0].contains("delete namespace iyakki-registry"), "{del:?}");
    assert!(matches!(inspect(&rt(&c), &reg, &target()), State::Absent));
    // removing what is not there is not an error and deletes nothing
    c.calls.lock().unwrap().clear();
    remove(&rt(&c), &reg, &target(), "iyakki-registry", &mut |_| {}).unwrap();
    assert!(!c.did("delete"));
    assert!(describe_remove(&State::Absent).contains("Type iyakki-registry"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_managed_cluster_uses_k3s_kubectl_when_the_machine_has_none() {
    struct Inner(Mutex<Vec<Vec<String>>>, bool);
    impl Runner for Inner {
        fn run(&self, _c: &std::path::Path, argv: &[String], _e: &[(String, String)], _t: Duration, _cap: usize, _s: Option<crate::wsexec::Sink>) -> Out {
            self.0.lock().unwrap().push(argv.to_vec());
            Out { code: Some(0), ..Default::default() }
        }
        fn has(&self, p: &str) -> bool {
            self.1 && p == "kubectl"
        }
    }
    let d = tmp("k3s");
    std::fs::create_dir_all(&d).unwrap();
    let k3s = d.join("k3s");
    std::fs::write(&k3s, "x").unwrap();
    let inner = Arc::new(Inner(Mutex::new(vec![]), false));
    let r = K3sKubectl { inner: inner.clone(), k3s: k3s.clone() };
    assert!(r.has("kubectl"));
    r.run(&d, &["kubectl".into(), "--kubeconfig".into(), "/kc".into(), "get".into(), "nodes".into()], &[], Duration::from_secs(1), 10, None);
    assert_eq!(inner.0.lock().unwrap()[0], vec![k3s.display().to_string(), "kubectl".into(), "--kubeconfig".into(), "/kc".into(), "get".into(), "nodes".into()]);
    // a kubectl on the machine is preferred; other programs are never rewritten
    let inner = Arc::new(Inner(Mutex::new(vec![]), true));
    let r = K3sKubectl { inner: inner.clone(), k3s };
    r.run(&d, &["kubectl".into(), "get".into()], &[], Duration::from_secs(1), 10, None);
    r.run(&d, &["curl".into()], &[], Duration::from_secs(1), 10, None);
    let calls = inner.0.lock().unwrap().clone();
    assert_eq!((calls[0][0].as_str(), calls[1][0].as_str()), ("kubectl", "curl"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn explanations_are_plain() {
    assert!(explain_kubectl("dial tcp 127.0.0.1:6443: connect: connection refused").contains("start it first"));
    assert!(explain_kubectl("Error from server (Forbidden): x").contains("administrator"));
    assert_eq!(explain_kubectl("\n  something odd \nmore"), "something odd");
    assert!(diagnose_pods(r#"{"items":[{"status":{"containerStatuses":[{"state":{"waiting":{"reason":"CrashLoopBackOff","message":"back-off"}}}]}}]}"#).unwrap().contains("does not stay up"));
    assert!(diagnose_pods(r#"{"items":[]}"#).is_none());
}

#[test]
fn a_machine_with_only_the_managed_cluster_runs_kubectl_as_k3s_kubectl() {
    let k3s = std::path::Path::new("/data/k3s-dev/bin/k3s");
    let argv: Vec<String> = ["kubectl", "--kubeconfig", "/data/k3s-dev/kubeconfig", "get", "nodes"].iter().map(|s| s.to_string()).collect();
    let r = crate::wsexec::kubectl_rewrite(&argv, false, Some(k3s)).unwrap();
    assert_eq!(r[..3], ["/data/k3s-dev/bin/k3s", "kubectl", "--kubeconfig"]);
    assert_eq!(r.len(), argv.len() + 1);
    assert!(crate::wsexec::kubectl_rewrite(&argv, true, Some(k3s)).is_none(), "a kubectl on the machine is used as it is");
    assert!(crate::wsexec::kubectl_rewrite(&argv, false, None).is_none(), "no managed cluster: nothing to fall back to");
    assert!(crate::wsexec::kubectl_rewrite(&["curl".to_string()], false, Some(k3s)).is_none());
}

#[test]
fn a_cluster_that_is_still_starting_is_waited_for_with_progress_then_the_plan_proceeds() {
    let p = crate::devcluster::ClusterProfile { name: "k3s".into(), kubeconfig: "/k".into(), context: String::new(), server: "https://127.0.0.1:6443".into(), source: crate::devcluster::Source::K3s, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
    let c = Cluster::healthy();
    c.st.lock().unwrap().ready_node = false;
    let looks = Arc::new(std::sync::atomic::AtomicU32::new(0));
    // becomes Ready after three looks at the nodes
    struct Slow(Arc<Cluster>, Arc<std::sync::atomic::AtomicU32>);
    impl Runner for Slow {
        fn run(&self, cwd: &std::path::Path, argv: &[String], env: &[(String, String)], t: Duration, cap: usize, s: Option<crate::wsexec::Sink>) -> Out {
            if argv.join(" ").contains("get nodes") && self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
                self.0.st.lock().unwrap().ready_node = true;
            }
            self.0.run(cwd, argv, env, t, cap, s)
        }
        fn has(&self, _: &str) -> bool {
            false
        }
    }
    let pl = crate::platform::Platform {
        run: Arc::new(Slow(c.clone(), looks.clone())),
        dir: tmp("wait"),
        user: "a".into(),
        vault: Arc::new(crate::platform::MemVault::default()),
        audit: Arc::new(|_, _| {}),
        env: crate::devcluster::Env::default(),
        tmp: std::env::temp_dir(),
        http: Arc::new(crate::platform::connect::FakeHttp::default()),
        gen: Arc::new(|n| "x".repeat(n)),
        live: None,
    };
    let mut said = Vec::new();
    crate::platform::wait_for_cluster(&pl, &p, Wait { tries: 10, pause: Duration::ZERO }, &mut |l| said.push(l.to_string()));
    assert!(looks.load(std::sync::atomic::Ordering::SeqCst) >= 3);
    assert!(said.first().unwrap().contains("waiting for the dev cluster") && said.first().unwrap().contains("no node is Ready"), "{said:?}");
    assert_eq!(said.last().unwrap(), "the cluster is Ready");
    // another cluster is not waited for
    let mut other = p.clone();
    other.local = false;
    let before = looks.load(std::sync::atomic::Ordering::SeqCst);
    crate::platform::wait_for_cluster(&pl, &other, Wait { tries: 10, pause: Duration::ZERO }, &mut |_| panic!("no wait"));
    assert_eq!(looks.load(std::sync::atomic::Ordering::SeqCst), before);
    // it never gets Ready: the wait ends and the plan says no node is Ready
    let c2 = Cluster::healthy();
    c2.st.lock().unwrap().ready_node = false;
    let pl2 = crate::platform::Platform { run: c2.clone(), ..pl };
    crate::platform::wait_for_cluster(&pl2, &p, Wait { tries: 3, pause: Duration::ZERO }, &mut |_| {});
}

#[test]
fn the_messages_say_what_is_wrong_and_what_to_do() {
    let mut p = crate::devcluster::ClusterProfile { name: "k3s".into(), kubeconfig: "/k".into(), context: String::new(), server: "https://127.0.0.1:6443".into(), source: crate::devcluster::Source::K3s, exists: true, readable: true, tested: true, reachable: false, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: "connection refused".into(), fix: String::new() };
    let r = crate::platform::deploy_refusal(&p).unwrap();
    assert!(r.contains("does not answer") && r.contains("up to two minutes") && r.contains("Settings"), "{r}");
    p.local = false;
    assert!(crate::platform::deploy_refusal(&p).unwrap().contains("reachable from this machine"));
    p.exists = false;
    assert!(crate::platform::deploy_refusal(&p).unwrap().contains("cannot be read"));
    assert!(explain_kubectl("kubectl is not installed").contains("Setup"));
}
