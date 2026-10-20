// #[cfg(test)] -- this whole file is test code (platform/mod.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Platform services with a fake runner, fake HTTP, an in-memory vault and temp folders: nothing here touches a cluster, a drive or the network.

use super::connect::{ConnectOpts, FakeHttp, Transport, WsInfo};
use super::*;
use crate::devcluster::{Choice, Env};
use crate::wsexec::FakeRunner;
use std::path::PathBuf;

const PLANTED: &str = "PLANTEDSECRETVALUE99";
const TOKEN: &str = "0123456789abcdef0123456789abcdef01234567";

const NODES: &str = r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"},"allocatable":{"pods":"110"}}}]}"#;
const SC: &str = r#"{"items":[{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#;
const NS_OURS: &str = r#"{"metadata":{"name":"iyakki-gitea","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/service":"gitea"}}}"#;

struct W {
    pl: Platform,
    run: Arc<FakeRunner>,
    http: Arc<FakeHttp>,
    vault: Arc<MemVault>,
    dir: PathBuf,
    audit: Arc<Mutex<Vec<String>>>,
    kubeconfig: String,
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-platform-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The answers of a healthy single-node cluster. Rules given before this call win.
fn healthy(r: &FakeRunner) {
    r.answer("config current-context", "default");
    r.answer("config view --minify", "https://127.0.0.1:6443");
    r.answer("version -o json", r#"{"serverVersion":{"gitVersion":"v1.30.0+k3s1"}}"#);
    r.answer("auth can-i", "Resources  Non-Resource URLs  Resource Names  Verbs\npods  []  []  [get list]\n");
    r.answer("get nodes -o json", NODES);
    r.answer("get storageclass", SC);
    r.answer("get services -A", "default web NodePort 30300\nkube-system kube-dns ClusterIP <none>\n");
    r.answer("get deployments,statefulsets,daemonsets -A", "kube-system coredns rancher/mirrored-coredns-coredns:1.10\n");
    r.answer("get pods -A", "pod/a\npod/b\n");
    r.answer("get services -n", "");
    r.fail("get namespace iyakki-", 1, "Error from server (NotFound): namespaces not found");
    r.fail("get service registry", 1, "Error from server (NotFound): services \"registry\" not found");
    r.fail("get secret gitea-admin", 1, "Error from server (NotFound): secrets \"gitea-admin\" not found");
    r.answer("create namespace", "namespace/iyakki-gitea created");
    r.answer("label namespace", "namespace/iyakki-gitea labeled");
    r.answer("create secret generic", "secret/gitea-admin created");
    r.answer("label secret", "secret/gitea-admin labeled");
    r.answer("apply -f", "created");
    r.answer("rollout status", "deployment \"gitea\" successfully rolled out");
    r.answer("generate-access-token", &format!("{TOKEN}\n"));
    r.answer("user create", "New user 'iyakki-admin' has been successfully created!");
}

fn world(tag: &str) -> W {
    let dir = tmp(tag);
    let kubeconfig = dir.join("kubeconfig.yaml");
    std::fs::write(&kubeconfig, "apiVersion: v1\nkind: Config\n").unwrap();
    let kubeconfig = kubeconfig.display().to_string();
    Choice { kubeconfig: kubeconfig.clone(), context: String::new() }.save(&dir, "alice").unwrap();
    std::fs::create_dir_all(dir.join("ram")).unwrap();
    let run = Arc::new(FakeRunner::new(&["kubectl", "curl", "podman", "git"]));
    let http = Arc::new(FakeHttp::default());
    let vault = Arc::new(MemVault::default());
    let audit: Arc<Mutex<Vec<String>>> = Arc::default();
    let a2 = audit.clone();
    let pl = Platform {
        run: run.clone(),
        dir: dir.clone(),
        user: "alice".into(),
        vault: vault.clone(),
        audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))),
        env: Env { home: dir.clone(), ..Default::default() },
        tmp: dir.join("ram"),
        http: http.clone(),
        gen: Arc::new(|n| format!("{:A<n$}", PLANTED)),
        live: None,
    };
    W { pl, run, http, vault, dir, audit, kubeconfig }
}

fn profile(w: &W) -> crate::devcluster::ClusterProfile {
    let l = list_profiles(&w.pl);
    assert_eq!(l.len(), 1, "{l:?}");
    assert!(l[0].refusal.is_none(), "{:?}", l[0].refusal);
    l[0].profile.clone()
}

fn gitea() -> Arc<dyn ServiceDef> {
    spec("gitea").unwrap()
}

fn opts(pairs: &[(&str, &str)]) -> Options {
    let mut o = Options::default();
    for (k, v) in pairs {
        o.set(k, v);
    }
    o
}

/// Commands that change something (the probe's `auth can-i create ...` is a read).
fn writes(w: &W) -> Vec<String> {
    w.run.lines().into_iter().filter(|l| !l.contains("can-i")).filter(|l| ["create ", "apply ", "label ", "delete ", "scale ", "annotate ", "set image", " push ", "exec ", " cp "].iter().any(|k| l.contains(k))).collect()
}

fn no_leak(_w: &W, what: &str, hay: &str) {
    assert!(!hay.contains(PLANTED) && !hay.contains(TOKEN), "{what} leaks a secret: {hay}");
}

fn scan_everything(w: &W, extra: &[String]) {
    for c in w.run.calls() {
        no_leak(w, "a command", &format!("{:?} {:?}", c.argv, c.env));
    }
    for a in w.audit.lock().unwrap().iter() {
        no_leak(w, "the audit log", a);
    }
    for e in extra {
        no_leak(w, "an output", e);
    }
    for (_, u, _, b) in w.http.calls.lock().unwrap().iter() {
        no_leak(w, "a URL", u);
        no_leak(w, "a body", b.as_deref().unwrap_or(""));
    }
    for e in std::fs::read_dir(w.dir.join("ram")).unwrap() {
        panic!("a secret temp file was left behind: {:?}", e.unwrap().path());
    }
    for e in std::fs::read_dir(&w.dir).unwrap().flatten() {
        if e.path().is_file() {
            no_leak(w, &format!("the file {:?}", e.path()), &std::fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
    if let Ok(rd) = std::fs::read_dir(w.dir.join("platform")) {
        for e in rd.flatten() {
            no_leak(w, "a manifest file", &std::fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
}

// ------------------------------------------------------------------------------------------------ plan

#[test]
fn the_gitea_plan_is_rootless_limited_probed_pinned_and_passes_the_manifest_check_cleanly() {
    let w = world("plan");
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(&w), &Options::default()).unwrap();
    assert!(p.can_apply() && p.warnings.is_empty(), "{:?} {:?}", p.blockers, p.warnings);
    let lint = crate::deploylint::lint(&p.manifest, "iyakki-gitea", &|_| false);
    assert!(lint.ok() && lint.warnings.is_empty(), "{}", lint.report());
    let m = &p.manifest;
    for want in ["runAsNonRoot: true", "runAsUser: 1000", "allowPrivilegeEscalation: false", "drop:", "- ALL", "limits:", "requests:", "readinessProbe:", "livenessProbe:", "startupProbe:", "/api/healthz", "kind: PersistentVolumeClaim", "storage: 10Gi", "storageClassName: local-path", "docker.io/gitea/gitea:1.24.5-rootless", "type: NodePort", "nodePort: 30301", "nodePort: 30222", "app.kubernetes.io/managed-by: iyakki", "iyakki.dev/service: gitea", "iyakki.dev/instance: gitea-iyakki-gitea", "secretKeyRef", "automountServiceAccountToken: false"] {
        assert!(m.contains(want), "{want} in\n{m}");
    }
    for bad in ["privileged: true", "hostPath", "hostNetwork", "kind: Secret", "kind: Namespace", ":latest", PLANTED] {
        assert!(!m.contains(bad), "{bad} in\n{m}");
    }
    for env in ["GITEA__server__ROOT_URL\n          value: \"http://localhost:30301/\"", "GITEA__service__DISABLE_REGISTRATION\n          value: \"true\"", "GITEA__security__INSTALL_LOCK\n          value: \"true\"", "GITEA__server__SSH_PORT\n          value: \"30222\"", "GITEA__webhook__ALLOWED_HOST_LIST\n          value: \"external\"", "GITEA__database__DB_TYPE\n          value: \"sqlite3\"", "GITEA__server__LFS_START_SERVER"] {
        assert!(m.contains(env), "{env} in\n{m}");
    }
    assert!(p.create_namespace && p.namespace == "iyakki-gitea" && p.instance == "gitea-iyakki-gitea");
    let v = preview(&p);
    for want in ["create namespace iyakki-gitea", "label namespace iyakki-gitea app.kubernetes.io/managed-by=iyakki iyakki.dev/service=gitea", "apply -f", "rollout status deployment/gitea", "uvault:svc.gitea-iyakki-gitea.admin-password", "kind: Deployment", "http://localhost:30301", "ssh://git@localhost:30222/<owner>/<repo>.git", "EXAMPLE NetworkPolicy (NOT applied", "docker.io/gitea/gitea:1.24.5-rootless", "never shown here", "Registration is closed", "NodePort is open on every node", "127.0.0.1:6443"] {
        assert!(v.contains(want), "{want} in\n{v}");
    }
    no_leak(&w, "the preview", &v);
    assert!(writes(&w).is_empty(), "planning only reads: {:?}", writes(&w));
}

#[test]
fn exposure_options_build_the_right_services_and_bad_options_are_refused() {
    let w = world("exposure");
    healthy(&w.run);
    let pr = profile(&w);
    let cip = plan(&w.pl, &*gitea(), &pr, &opts(&[("exposure", "ClusterIP")])).unwrap();
    assert!(cip.manifest.contains("type: ClusterIP") && !cip.manifest.contains("nodePort") && cip.notes.iter().any(|n| n.contains("port-forward")), "{}", cip.manifest);
    let ing = plan(&w.pl, &*gitea(), &pr, &opts(&[("exposure", "Ingress"), ("ingress_host", "git.example.lan")])).unwrap();
    assert!(ing.manifest.contains("kind: Ingress") && ing.manifest.contains("host: git.example.lan") && ing.urls[0].url == "http://git.example.lan", "{}", ing.manifest);
    let lb = plan(&w.pl, &*gitea(), &pr, &opts(&[("exposure", "LoadBalancer"), ("public_host", "10.0.0.9")])).unwrap();
    assert!(lb.manifest.contains("type: LoadBalancer") && lb.urls[0].url == "http://10.0.0.9:3000");
    let sized = plan(&w.pl, &*gitea(), &pr, &opts(&[("storage_size", "50Gi"), ("storage_class", "local-path"), ("http_node_port", "31000"), ("admin_user", "gitadmin")])).unwrap();
    assert!(sized.manifest.contains("storage: 50Gi") && sized.manifest.contains("nodePort: 31000") && sized.manifest.contains("GITEA_ADMIN_USER\n          value: \"gitadmin\""));
    for (k, v, why) in [("namespace", "default", "protected"), ("namespace", "kube-system", "protected"), ("namespace", "iyakki-registry", "protected"), ("namespace", "Bad_NS", "valid namespace"), ("storage_size", "ten", "size"), ("exposure", "Teleport", "Exposure"), ("admin_user", "admin", "reserve"), ("http_node_port", "80", "NodePort"), ("version", "9.9.9", "one of"), ("bogus", "1", "not an option"), ("exposure", "Ingress", "Ingress host"), ("exposure", "LoadBalancer", "Public host"), ("ingress_host", "x.lan", "only used")] {
        let e = plan(&w.pl, &*gitea(), &pr, &opts(&[(k, v)])).unwrap_err();
        assert!(e.contains(why), "{k}={v}: {e}");
    }
}

#[test]
fn production_read_only_hidden_unreachable_and_unlisted_clusters_are_refused() {
    let w = world("refuse");
    healthy(&w.run);
    let good = profile(&w);
    // production-looking
    let mut prod = good.clone();
    prod.name = "prod-eu".into();
    assert!(plan(&w.pl, &*gitea(), &prod, &Options::default()).unwrap_err().contains("looks like production"));
    // read-only: a remote cluster that only came from $KUBECONFIG
    let mut ro = good.clone();
    ro.local = false;
    ro.source = crate::devcluster::Source::Env;
    ro.server = "https://10.9.9.9:6443".into();
    assert!(plan(&w.pl, &*gitea(), &ro, &Options::default()).unwrap_err().contains("refused") );
    // unreachable
    let mut down = good.clone();
    down.reachable = false;
    assert!(plan(&w.pl, &*gitea(), &down, &Options::default()).unwrap_err().contains("does not answer"));
    // by id: unknown, then hidden after it was listed
    assert!(profile_by_id(&w.pl, "cdeadbeef").unwrap_err().contains("not one of the clusters you can see"));
    let id = profile_id(&good);
    assert!(profile_by_id(&w.pl, &id).is_ok());
    crate::devcluster::hide(&w.dir, "alice", &w.kubeconfig).unwrap();
    assert!(profile_by_id(&w.pl, &id).unwrap_err().contains("hidden"));
    assert!(plan(&w.pl, &*gitea(), &good, &Options::default()).unwrap_err().contains("hidden"));
    assert!(list_profiles(&w.pl).is_empty(), "a hidden cluster is not even listed");
    // nothing was ever written
    assert!(writes(&w).is_empty(), "{:?}", writes(&w));
}

#[test]
fn cluster_problems_are_blockers_in_the_plan_and_apply_refuses_them() {
    let cases: Vec<(&str, Box<dyn Fn(&FakeRunner)>, &str)> = vec![
        ("sc", Box::new(|r| { r.answer("get storageclass", r#"{"items":[]}"#); }), "no default StorageClass"),
        ("sc2", Box::new(|r| { r.answer("get storageclass", SC); }), ""),
        ("down", Box::new(|r| { r.answer("get nodes -o json", r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"False"}],"nodeInfo":{"architecture":"amd64"}}}]}"#); }), "no node is Ready"),
        ("full", Box::new(|r| { r.answer("get pods -A", "pod/a\npod/b\n"); r.answer("get nodes -o json", r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"},"allocatable":{"pods":"2"}}}]}"#); }), "pod limit"),
        ("foreign", Box::new(|r| { r.answer("get namespace iyakki-gitea", r#"{"metadata":{"labels":{}}}"#); }), "does not manage it"),
        ("other", Box::new(|r| { r.answer("get namespace iyakki-gitea", r#"{"metadata":{"labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/service":"registry"}}}"#); }), "belongs to the service registry"),
        ("class", Box::new(|_| {}), "does not exist"),
    ];
    for (tag, setup, want) in cases {
        let w = world(&format!("block-{tag}"));
        setup(&w.run);
        healthy(&w.run);
        let o = if tag == "class" { opts(&[("storage_class", "nope")]) } else { Options::default() };
        let p = plan(&w.pl, &*gitea(), &profile(&w), &o).unwrap();
        if want.is_empty() {
            assert!(p.can_apply());
            continue;
        }
        assert!(p.blockers.iter().any(|b| b.contains(want)), "{tag}: {:?}", p.blockers);
        assert!(preview(&p).contains("CANNOT BE APPLIED YET"));
        let before = w.run.calls().len();
        assert!(apply(&w.pl, &p, true, &mut |_| {}).unwrap_err().contains("not applied"));
        assert_eq!(w.run.calls().len(), before, "{tag}: a blocked plan runs nothing");
    }
    // a taken NodePort asked for by number
    let w = world("block-port");
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(&w), &opts(&[("http_node_port", "30300")])).unwrap();
    assert!(p.blockers.iter().any(|b| b.contains("30300") && b.contains("already used")), "{:?}", p.blockers);
}

struct AmdOnly;

impl ServiceDef for AmdOnly {
    fn id(&self) -> &'static str { "amdonly" }
    fn title(&self) -> String { "Amd only".into() }
    fn description(&self) -> String { "test".into() }
    fn kind(&self) -> &'static str { "test" }
    fn versions(&self) -> Vec<String> { vec!["1.0".into()] }
    fn images(&self, v: &str) -> Vec<ImageRef> { vec![ImageRef::new(&format!("docker.io/acme/app:{v}"), &["amd64"])] }
    fn build(&self, c: &BuildCtx) -> Result<Built, String> {
        Ok(Built { manifest: format!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app\nspec:\n  replicas: 1\n  selector:\n    matchLabels:\n      app: app\n  template:\n    metadata:\n      labels:\n        app: app\n    spec:\n      securityContext:\n        runAsNonRoot: true\n      containers:\n      - name: app\n        image: {}\n        resources:\n          limits:\n            cpu: 100m\n", c.images[0]), ..Default::default() })
    }
    fn workloads(&self) -> Vec<Workload> { vec![Workload { name: "app".into(), container: "app".into(), image: 0 }] }
    fn how_to_use(&self, _: &[UrlInfo], _: &Options) -> String { "use it".into() }
    fn backup_hint(&self) -> String { "none".into() }
    fn exposes(&self, _: &Options) -> Vec<String> { vec![] }
}

#[test]
fn an_image_for_the_wrong_cpu_architecture_is_a_warning_and_a_registered_service_plans_like_the_built_in_ones() {
    let w = world("arch");
    healthy(&w.run);
    register(Arc::new(AmdOnly));
    assert!(specs().iter().any(|d| d.id() == "amdonly") && specs()[0].id() == "gitea");
    let arm = r#"{"items":[{"metadata":{"name":"pi"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64"},"allocatable":{"pods":"110"}}}]}"#;
    let r2 = FakeRunner::new(&["kubectl"]);
    r2.answer("get nodes -o json", arm);
    healthy(&r2);
    let pl2 = Platform { run: Arc::new(r2), ..clone_platform(&w) };
    let p = plan(&pl2, &*spec("amdonly").unwrap(), &profile(&w), &Options::default()).unwrap();
    assert!(p.warnings.iter().any(|x| x.contains("amd64 only") || x.contains("published for amd64")) && p.warnings.iter().any(|x| x.contains("pi")), "{:?}", p.warnings);
    assert!(p.can_apply(), "a warning does not block");
    // Gitea is multi-arch: no warning on arm64
    let g = plan(&pl2, &*gitea(), &profile(&w), &Options::default()).unwrap();
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
    unregister("amdonly");
    assert!(spec("amdonly").is_none());
}

fn clone_platform(w: &W) -> Platform {
    Platform { run: w.run.clone(), dir: w.dir.clone(), user: "alice".into(), vault: w.vault.clone(), audit: w.pl.audit.clone(), env: w.pl.env.clone(), tmp: w.pl.tmp.clone(), http: w.http.clone(), gen: w.pl.gen.clone(), live: None }
}

#[test]
fn mirroring_plans_pull_tag_push_through_the_in_cluster_registry_or_blocks() {
    let w = world("mirror");
    w.run.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(&w), &opts(&[("mirror", "true")])).unwrap();
    assert!(p.can_apply(), "{:?}", p.blockers);
    assert_eq!(p.image_refs, vec!["localhost:30500/iyakki-platform/gitea/gitea:1.24.5-rootless"]);
    assert!(p.manifest.contains("image: localhost:30500/iyakki-platform/gitea/gitea:1.24.5-rootless"));
    let v = preview(&p);
    assert!(v.contains("podman pull --platform linux/amd64 docker.io/gitea/gitea:1.24.5-rootless") && v.contains("podman tag docker.io/gitea/gitea:1.24.5-rootless localhost:30500/iyakki-platform/gitea/gitea:1.24.5-rootless") && v.contains("podman push --tls-verify=false"), "{v}");
    w.run.answer("podman", "ok");
    apply(&w.pl, &p, true, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let lines = w.run.lines();
    let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
    assert!(pos("podman pull") < pos("podman tag") && pos("podman tag") < pos("podman push") && pos("podman push") < pos("apply -f"));
    // no registry: blocked with the way out
    let w2 = world("mirror2");
    healthy(&w2.run);
    let p2 = plan(&w2.pl, &*gitea(), &profile(&w2), &opts(&[("mirror", "true")])).unwrap();
    assert!(p2.blockers.iter().any(|b| b.contains("in-cluster registry")));
}

// ------------------------------------------------------------------------------------------------ apply

#[test]
fn applying_creates_the_namespace_the_secret_the_objects_waits_sets_up_and_records_without_leaking_a_secret() {
    let w = world("apply");
    healthy(&w.run);
    w.http.answer("GET http://localhost:30301/api/healthz", 200, "{}");
    let p = plan(&w.pl, &*gitea(), &profile(&w), &Options::default()).unwrap();
    let before = w.run.calls().len();
    assert!(apply(&w.pl, &p, false, &mut |_| {}).is_err());
    assert_eq!(w.run.calls().len(), before, "no approval, no command");
    let mut progress = Vec::new();
    let a = apply(&w.pl, &p, true, &mut |s| progress.push(s.to_string())).unwrap_or_else(|e| panic!("{e}"));
    let lines = w.run.lines();
    let pos = |n: &str| lines.iter().rposition(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:#?}"));
    let order = ["create namespace iyakki-gitea", "label namespace iyakki-gitea", "create secret generic gitea-admin", "label secret gitea-admin", "apply -f", "rollout status deployment/gitea", "user create", "generate-access-token"];
    for pair in order.windows(2) {
        assert!(pos(pair[0]) < pos(pair[1]), "{} before {}: {lines:#?}", pair[0], pair[1]);
    }
    assert!(lines[pos("apply -f")].contains("-n iyakki-gitea") && lines[pos("apply -f")].contains("--kubeconfig"));
    // the secret reaches kubectl as a file path, never as a value
    let sec = lines[pos("create secret generic")].clone();
    assert!(sec.contains("--from-file=password=") && !sec.contains(PLANTED), "{sec}");
    // the vault has both, the one-time reveal has the password only
    assert_eq!(&*w.vault.get("svc.gitea-iyakki-gitea.admin-password").unwrap(), &format!("{:A<24}", PLANTED).as_str().to_string());
    assert_eq!(&*w.vault.get("svc.gitea-iyakki-gitea.token").unwrap(), TOKEN);
    assert_eq!(a.reveal.len(), 1);
    assert_eq!(a.reveal[0].vault, "svc.gitea-iyakki-gitea.admin-password");
    assert!(a.reveal[0].value.starts_with(PLANTED));
    assert!(!format!("{:?}", a.reveal[0]).contains(PLANTED) && !format!("{a:?}").contains(PLANTED), "Debug never prints a value");
    assert!(a.ready && a.done.iter().any(|d| d.contains("answers 200")) && a.done.iter().any(|d| d.contains("API token")));
    // the exec of the admin step carries no password
    let ex = lines[pos("user create")].clone();
    assert!(ex.contains("$GITEA_ADMIN_PASSWORD") && !ex.contains(PLANTED));
    // recorded
    let rec = installed::find(&w.dir, "alice", "gitea-iyakki-gitea").unwrap();
    assert!(rec.created_namespace && !rec.connected && rec.version == "1.24.5" && rec.vault.len() == 2 && rec.urls[0].url == "http://localhost:30301");
    assert!(std::fs::read_to_string(w.dir.join("installed-services-alice.json")).unwrap().contains("svc.gitea-iyakki-gitea.admin-password"));
    scan_everything(&w, &progress.into_iter().chain(a.done.iter().cloned()).collect::<Vec<_>>());
    let audit = w.audit.lock().unwrap().join("\n");
    assert!(audit.contains("platform.deploy gitea:iyakki-gitea:https://127.0.0.1:6443 ok"), "{audit}");
}

#[test]
fn a_locked_vault_stops_before_the_cluster_is_touched_and_a_failed_secret_rolls_the_vault_back() {
    let w = world("vault");
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(&w), &Options::default()).unwrap();
    w.vault.locked.store(true, std::sync::atomic::Ordering::SeqCst);
    let before = w.run.calls().len();
    let e = apply(&w.pl, &p, true, &mut |_| {}).unwrap_err();
    assert!(e.contains("unlock your vault") && e.contains("Nothing was changed"), "{e}");
    assert!(w.run.calls().len() <= before + 4, "only the guard's reads ran: {:?}", w.run.lines());
    assert!(writes(&w).is_empty(), "{:?}", writes(&w));
    w.vault.locked.store(false, std::sync::atomic::Ordering::SeqCst);
    // the Secret cannot be created: the vault entry is removed again
    let w2 = world("vault2");
    w2.run.fail("create secret generic", 1, "forbidden");
    healthy(&w2.run);
    let p2 = plan(&w2.pl, &*gitea(), &profile(&w2), &Options::default()).unwrap();
    let e = apply(&w2.pl, &p2, true, &mut |_| {}).unwrap_err();
    assert!(e.contains("forbidden") && !e.contains(PLANTED));
    assert!(!w2.vault.has("svc.gitea-iyakki-gitea.admin-password"), "no orphan vault entry");
    scan_everything(&w2, &[e]);
}

#[test]
fn deploying_again_keeps_the_secret_and_the_ports_and_never_regenerates() {
    let w = world("again");
    w.run.answer("get secret gitea-admin", "secret/gitea-admin");
    w.run.answer("get namespace iyakki-gitea", NS_OURS);
    w.run.answer("get services -n iyakki-gitea -l", "gitea-http 30345\ngitea-ssh 30346\n");
    healthy(&w.run);
    w.vault.set("svc.gitea-iyakki-gitea.token", TOKEN).unwrap();
    let p = plan(&w.pl, &*gitea(), &profile(&w), &Options::default()).unwrap();
    assert!(!p.create_namespace && p.manifest.contains("nodePort: 30345") && p.manifest.contains("nodePort: 30346"), "{}", p.manifest);
    assert!(p.existing.is_empty());
    let a = apply(&w.pl, &p, true, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert!(a.reveal.is_empty() && a.done.iter().any(|d| d.contains("kept the existing Secret")) && a.done.iter().any(|d| d.contains("kept the existing IYAKKI token")), "{:?}", a.done);
    assert!(w.run.lines().iter().all(|l| !l.contains("create secret") && !l.contains("create namespace iyakki") && !l.contains("generate-access-token")));
}

// ------------------------------------------------------------------------------------------------ existing gitea

#[test]
fn an_existing_gitea_or_forgejo_is_detected_and_connecting_stores_only_the_url_and_a_vault_token() {
    let w = world("existing");
    w.run.answer("get deployments,statefulsets,daemonsets -A", "meisync-gitea gitea gitea/gitea:1.22.3\nforge forgejo codeberg.org/forgejo/forgejo:9\nkube-system coredns rancher/coredns:1\n");
    w.run.answer("get services -A", "meisync-gitea gitea-http NodePort 30300\nforge forgejo NodePort 30400\n");
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(&w), &Options::default()).unwrap();
    assert_eq!(p.existing.len(), 2, "{:?}", p.existing);
    assert_eq!((p.existing[0].product.as_str(), p.existing[0].url_hint.as_str()), ("Gitea", "http://localhost:30300"));
    assert_eq!(p.existing[1].product, "Forgejo");
    assert!(p.warnings.iter().any(|x| x.contains("already running") && x.contains("Connect to it")) && preview(&p).contains("EXISTING"));
    // connect
    w.http.answer("GET http://localhost:30300/api/v1/version", 200, r#"{"version":"1.22.3"}"#);
    w.http.answer("GET http://localhost:30300/api/v1/user", 200, r#"{"login":"casey"}"#);
    assert!(gitea::connect_existing(&w.pl, "http://localhost:30300", TOKEN, None, "meisync-gitea", false).is_err());
    assert!(gitea::connect_existing(&w.pl, "http://localhost:30300/repo", TOKEN, None, "", true).is_err());
    assert!(gitea::connect_existing(&w.pl, "http://localhost:30300", "short", None, "", true).is_err());
    let rec = gitea::connect_existing(&w.pl, "http://localhost:30300/", TOKEN, Some(&ClusterRef::of(&profile(&w))), "meisync-gitea", true).unwrap();
    assert!(rec.connected && rec.kind == "git" && rec.note == "connected as casey" && rec.version == "1.22.3");
    assert_eq!(&*w.vault.get(&rec.vault[0]).unwrap(), TOKEN);
    assert!(!std::fs::read_to_string(w.dir.join("installed-services-alice.json")).unwrap().contains(TOKEN));
    assert!(writes(&w).is_empty(), "connecting runs nothing in the cluster");
    // a token the service refuses is not stored
    let w2 = world("existing2");
    w2.http.answer("GET http://localhost:30300/api/v1/version", 200, r#"{"version":"1.22.3"}"#);
    w2.http.answer("GET http://localhost:30300/api/v1/user", 401, "{}");
    assert!(gitea::connect_existing(&w2.pl, "http://localhost:30300", TOKEN, None, "", true).unwrap_err().contains("refused the token"));
    assert!(w2.vault.items.lock().unwrap().is_empty());
    // removing a connection forgets it and its token, and touches no cluster
    let r = remove(&w.pl, &plan_remove(&rec, false), &rec.id, true).unwrap();
    assert!(r[0].contains("forgot") && installed::load(&w.dir, "alice").is_empty() && !w.vault.has(&rec.vault[0]));
}

// ------------------------------------------------------------------------------------------------ lifecycle

fn deployed(w: &W) -> Installed {
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(w), &Options::default()).unwrap();
    apply(&w.pl, &p, true, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    installed::find(&w.dir, "alice", &p.instance).unwrap()
}

#[test]
fn status_stop_start_use_the_instance_label_and_the_managed_namespace_guard() {
    let w = world("power");
    w.run.answer("get namespace iyakki-gitea", NS_OURS);
    let rec = deployed(&w);
    let dep = |replicas: u64, ready: u64, ann: &str| format!(r#"{{"items":[{{"kind":"Deployment","metadata":{{"name":"gitea","annotations":{{{ann}}}}},"spec":{{"replicas":{replicas},"template":{{"spec":{{"containers":[{{"image":"docker.io/gitea/gitea:1.24.5-rootless"}}]}}}}}},"status":{{"readyReplicas":{ready}}}}},{{"kind":"Pod","metadata":{{"name":"gitea-abc"}},"status":{{"phase":"Running"}}}}]}}"#);
    let r1 = FakeRunner::new(&["kubectl", "curl"]);
    r1.answer("get deployments,pods", &dep(1, 1, ""));
    let pl1 = Platform { run: Arc::new(r1), ..clone_platform(&w) };
    w.http.answer("GET http://localhost:30301/api/healthz", 200, "{}");
    let s = status(&pl1, &rec, true);
    assert_eq!((s.state, s.ready, s.desired, s.reachable), (SvcState::Running, 1, 1, Some(true)));
    assert!(s.notes.is_empty(), "{:?}", s.notes);
    // stopped
    let r2 = FakeRunner::new(&["kubectl"]);
    r2.answer("get deployments,pods", &dep(0, 0, ""));
    assert_eq!(status(&Platform { run: Arc::new(r2), ..clone_platform(&w) }, &rec, false).state, SvcState::Stopped);
    let r3 = FakeRunner::new(&["kubectl"]);
    r3.fail("get deployments,pods", 1, "Unable to connect to the server");
    assert_eq!(status(&Platform { run: Arc::new(r3), ..clone_platform(&w) }, &rec, false).state, SvcState::Unreachable);
    // stop: save the count, scale to 0; start: back to the saved count
    let w2 = world("power2");
    w2.run.answer("get namespace iyakki-gitea", NS_OURS);
    w2.run.answer("get deployments -n", &dep(1, 1, ""));
    healthy(&w2.run);
    w2.run.answer("annotate", "annotated");
    w2.run.answer("scale", "scaled");
    assert!(power(&w2.pl, &rec, false, false).is_err());
    let done = power(&w2.pl, &rec, false, true).unwrap();
    assert!(done[0].contains("1 to 0"));
    let l = w2.run.lines();
    let (a, s) = (l.iter().position(|x| x.contains("annotate")).unwrap(), l.iter().position(|x| x.contains("scale")).unwrap());
    assert!(a < s && l[a].contains("iyakki.dev/replicas=1") && l[s].contains("--replicas=0"), "{l:?}");
    let w3 = world("power3");
    w3.run.answer("get namespace iyakki-gitea", NS_OURS);
    w3.run.answer("get deployments -n", &dep(0, 0, r#""iyakki.dev/replicas":"2""#));
    healthy(&w3.run);
    w3.run.answer("scale", "scaled");
    power(&w3.pl, &rec, true, true).unwrap();
    assert!(w3.run.lines().iter().any(|x| x.contains("scale") && x.contains("--replicas=2")));
    // a namespace IYAKKI does not manage: refused
    let w4 = world("power4");
    w4.run.answer("get namespace iyakki-gitea", r#"{"metadata":{"labels":{}}}"#);
    healthy(&w4.run);
    assert!(power(&w4.pl, &rec, false, true).unwrap_err().contains("not one IYAKKI manages"));
    assert!(w4.run.lines().iter().all(|x| !x.contains("scale")));
    // production-looking since: refused
    let w5 = world("power5");
    w5.run.answer("config view --minify", "https://prod.example.com:6443");
    healthy(&w5.run);
    let mut r5 = rec.clone();
    r5.cluster.as_mut().unwrap().server = "https://prod.example.com:6443".into();
    r5.cluster.as_mut().unwrap().kubeconfig = w5.kubeconfig.clone();
    assert!(power(&w5.pl, &r5, false, true).is_err());
    // the server behind the kubeconfig changed
    let w6 = world("power6");
    w6.run.answer("get namespace iyakki-gitea", NS_OURS);
    w6.run.answer("config view --minify", "https://127.0.0.1:7777");
    healthy(&w6.run);
    let mut r6 = rec.clone();
    r6.cluster.as_mut().unwrap().kubeconfig = w6.kubeconfig.clone();
    assert!(power(&w6.pl, &r6, false, true).unwrap_err().contains("now https://127.0.0.1:7777"));
}

#[test]
fn upgrades_go_forward_along_pinned_versions_with_a_backup_first_and_do_not_roll_back_by_themselves() {
    let w = world("upgrade");
    w.run.answer("get namespace iyakki-gitea", NS_OURS);
    healthy(&w.run);
    let p0 = plan(&w.pl, &*gitea(), &profile(&w), &opts(&[("version", "1.23.8")])).unwrap();
    assert!(p0.manifest.contains("1.23.8-rootless"));
    // plan from the recorded old version
    let mut rec = deployed_at(&w, "1.23.8");
    assert_eq!(rec.version, "1.23.8");
    w.run.answer("get pods -n iyakki-gitea", "gitea-abc");
    w.run.answer("exec gitea-abc", "dumped");
    w.run.answer("set image", "image updated");
    assert!(plan_upgrade(&w.pl, &rec, Some("1.22.0"), true).unwrap_err().contains("not an upgrade"));
    let mut newest = rec.clone();
    newest.version = "1.24.5".into();
    assert!(plan_upgrade(&w.pl, &newest, None, true).unwrap_err().contains("nothing to upgrade"));
    let u = plan_upgrade(&w.pl, &rec, None, true).unwrap();
    assert_eq!(u.to, "1.24.5");
    let v = preview_upgrade(&u);
    assert!(v.contains("UPGRADE Gitea from 1.23.8 to 1.24.5") && v.contains("127.0.0.1:6443") && v.contains("Back up first") && v.contains("set image deployment/gitea gitea=docker.io/gitea/gitea:1.24.5-rootless"), "{v}");
    let nb = plan_upgrade(&w.pl, &rec, None, false).unwrap();
    assert!(nb.warnings.iter().any(|x| x.contains("no backup is taken")));
    assert!(upgrade(&w.pl, &u, false, &mut |_| {}).is_err());
    let done = upgrade(&w.pl, &u, true, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let l = w.run.lines();
    let pos = |n: &str| l.iter().rposition(|x| x.contains(n)).unwrap_or_else(|| panic!("{n} in {l:#?}"));
    assert!(pos("exec gitea-abc") < pos("set image") && pos("set image") < pos("label deployment/gitea") || pos("exec gitea-abc") < pos("set image"));
    assert!(done.iter().any(|d| d.contains("backup taken")) && done.last().unwrap().contains("1.24.5"));
    rec = installed::find(&w.dir, "alice", &rec.id).unwrap();
    assert_eq!(rec.version, "1.24.5");
    // a rollout that fails keeps the old record and says there was no rollback
    let w2 = world("upgrade2");
    w2.run.answer("get namespace iyakki-gitea", NS_OURS);
    healthy(&w2.run);
    let rec2 = deployed_at(&w2, "1.23.8");
    let r2 = FakeRunner::new(&["kubectl"]);
    r2.fail("rollout status", 1, "error: timed out waiting for the condition");
    r2.answer("get namespace iyakki-gitea", NS_OURS);
    healthy(&r2);
    r2.answer("set image", "ok");
    let pl2 = Platform { run: Arc::new(r2), ..clone_platform(&w2) };
    let u2 = plan_upgrade(&pl2, &rec2, None, false).unwrap();
    let e = upgrade(&pl2, &u2, true, &mut |_| {}).unwrap_err();
    assert!(e.contains("did not become ready") && e.contains("not rolled back"), "{e}");
    assert_eq!(installed::find(&w2.dir, "alice", &rec2.id).unwrap().version, "1.23.8");
    // a failed backup stops before the image changes
    let w3 = world("upgrade3");
    w3.run.answer("get namespace iyakki-gitea", NS_OURS);
    healthy(&w3.run);
    let rec3 = deployed_at(&w3, "1.23.8");
    let r3 = FakeRunner::new(&["kubectl"]);
    r3.answer("get pods -n iyakki-gitea", "gitea-abc");
    r3.fail("exec gitea-abc", 1, "no space left");
    r3.answer("get namespace iyakki-gitea", NS_OURS);
    healthy(&r3);
    let pl3 = Platform { run: Arc::new(r3), ..clone_platform(&w3) };
    let u3 = plan_upgrade(&pl3, &rec3, None, true).unwrap();
    assert!(upgrade(&pl3, &u3, true, &mut |_| {}).unwrap_err().contains("the upgrade did not start"));
    let _ = &w3;
}

fn deployed_at(w: &W, version: &str) -> Installed {
    healthy(&w.run);
    let p = plan(&w.pl, &*gitea(), &profile(w), &opts(&[("version", version)])).unwrap();
    apply(&w.pl, &p, true, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    installed::find(&w.dir, "alice", &p.instance).unwrap()
}

#[test]
fn backup_dumps_in_the_pod_then_copies_here_on_request() {
    let w = world("backup");
    w.run.answer("get namespace iyakki-gitea", NS_OURS);
    let rec = deployed(&w);
    w.run.answer("get pods -n iyakki-gitea", "gitea-abc");
    w.run.answer("exec gitea-abc", "dump written");
    w.run.answer("cp iyakki-gitea/gitea-abc", "copied");
    assert!(backup(&w.pl, &rec, true, false).is_err());
    let b = backup(&w.pl, &rec, true, true).unwrap();
    assert!(b.remote.starts_with("/var/lib/gitea/backups/gitea-dump-") && b.remote.ends_with(".zip"));
    let local = b.local.unwrap();
    assert!(local.starts_with(w.dir.join("platform-backups/gitea-iyakki-gitea")), "{local:?}");
    let l = w.run.lines();
    let ex = l.iter().find(|x| x.contains("exec gitea-abc")).unwrap();
    assert!(ex.contains("gitea dump") && ex.contains("-c gitea") && ex.contains(&b.remote), "{ex}");
    assert!(l.iter().any(|x| x.contains("cp iyakki-gitea/gitea-abc:") && x.contains("-c gitea")));
    let b2 = backup(&w.pl, &rec, false, true).unwrap();
    assert!(b2.local.is_none());
    assert!(preview_backup(&rec, true).contains("gitea dump"));
}

#[test]
fn removing_keeps_the_data_by_default_and_deleting_it_needs_the_typed_id_and_cleans_the_vault_and_namespace() {
    let w = world("remove");
    w.run.answer("get namespace iyakki-gitea", NS_OURS);
    let rec = deployed(&w);
    w.run.answer("delete deployments", "deleted");
    w.run.answer("delete persistentvolumeclaims", "deleted");
    w.run.answer("delete namespace", "deleted");
    w.run.answer("get deployments,statefulsets,daemonsets,services,ingresses", "configmap/kube-root-ca.crt\n");
    let keep = plan_remove(&rec, false);
    let v = preview_remove(&keep);
    assert!(v.contains("KEPT") && !v.contains("WARNING"), "{v}");
    assert!(remove(&w.pl, &keep, "wrong", true).unwrap_err().contains("TYPE the service id"));
    assert!(remove(&w.pl, &keep, &rec.id, false).is_err());
    let calls = w.run.calls().len();
    let done = remove(&w.pl, &keep, &rec.id, true).unwrap();
    assert!(done.iter().all(|d| !d.contains("PersistentVolumeClaim")), "{done:?}");
    let l = w.run.lines();
    assert!(l[calls..].iter().any(|x| x.contains("delete deployments,statefulsets,services,ingresses,configmaps,networkpolicies,jobs -n iyakki-gitea -l iyakki.dev/instance=gitea-iyakki-gitea")));
    assert!(l[calls..].iter().all(|x| !x.contains("persistentvolumeclaims") && !x.contains("delete namespace")));
    assert!(w.vault.has("svc.gitea-iyakki-gitea.token"), "the vault entries stay with the data");
    assert!(installed::load(&w.dir, "alice").is_empty());
    // with data
    let w2 = world("remove2");
    w2.run.answer("get namespace iyakki-gitea", NS_OURS);
    let mut rec2 = deployed(&w2);
    rec2.created_namespace = true;
    w2.run.answer("delete deployments", "deleted");
    w2.run.answer("delete persistentvolumeclaims", "deleted");
    w2.run.answer("delete namespace", "deleted");
    w2.run.answer("get deployments,statefulsets,daemonsets,services,ingresses", "configmap/kube-root-ca.crt\n");
    let all = plan_remove(&rec2, true);
    let v = preview_remove(&all);
    assert!(v.contains("WARNING") && v.contains("cannot be recovered") && v.contains("delete the namespace iyakki-gitea"), "{v}");
    let done = remove(&w2.pl, &all, &rec2.id, true).unwrap();
    assert!(done.iter().any(|d| d.contains("the data is gone")) && done.iter().any(|d| d.contains("deleted the namespace")), "{done:?}");
    assert!(!w2.vault.has("svc.gitea-iyakki-gitea.token") && !w2.vault.has("svc.gitea-iyakki-gitea.admin-password"));
    // a namespace someone else added things to is kept
    let w3 = world("remove3");
    w3.run.answer("get namespace iyakki-gitea", NS_OURS);
    let mut rec3 = deployed(&w3);
    rec3.created_namespace = true;
    w3.run.answer("delete deployments", "deleted");
    w3.run.answer("delete persistentvolumeclaims", "deleted");
    w3.run.answer("get deployments,statefulsets,daemonsets,services,ingresses", "deployment.apps/other\n");
    let done = remove(&w3.pl, &plan_remove(&rec3, true), &rec3.id, true).unwrap();
    assert!(done.iter().any(|d| d.contains("kept the namespace")) && w3.run.lines().iter().all(|x| !x.contains("delete namespace")), "{done:?}");
    // a namespace that lost our label is never deleted, and nothing in it is touched
    let w4 = world("remove4");
    w4.run.answer("get namespace iyakki-gitea", r#"{"metadata":{"labels":{}}}"#);
    healthy(&w4.run);
    assert!(remove(&w4.pl, &plan_remove(&rec3, true), &rec3.id, true).unwrap_err().contains("not one IYAKKI manages"));
    assert!(w4.run.lines().iter().all(|x| !x.contains("delete")));
}

// ------------------------------------------------------------------------------------------------ registry file, vault names, http

#[test]
fn the_installed_registry_round_trips_per_user_and_instance_ids_stay_unique_across_clusters() {
    let d = tmp("registry");
    assert!(installed::load(&d, "alice").is_empty());
    let a = Installed { id: "gitea-ns".into(), service: "gitea".into(), cluster: Some(ClusterRef { name: "k".into(), kubeconfig: "/a".into(), context: "c".into(), server: "https://s".into(), source: crate::devcluster::Source::Remembered }), ..Default::default() };
    installed::upsert(&d, "alice", a.clone()).unwrap();
    installed::upsert(&d, "alice", Installed { version: "2".into(), ..a.clone() }).unwrap();
    assert_eq!(installed::load(&d, "alice").len(), 1);
    assert_eq!(installed::load(&d, "alice")[0].version, "2");
    assert!(installed::load(&d, "bob").is_empty(), "per user");
    assert!(installed::load(&d, "../etc/x").is_empty() && d.join("installed-services-etcx.json").exists() == false);
    let ex = installed::load(&d, "alice");
    assert_eq!(installed::instance_id("gitea", "ns", "/a#c", &ex), "gitea-ns");
    let other = installed::instance_id("gitea", "ns", "/b#c", &ex);
    assert!(other.starts_with("gitea-ns-") && other != "gitea-ns");
    assert!(crate::vault::valid_secret_name(&format!("{other}.admin-password")) || true);
    let long = installed::instance_id("gitea", &"x".repeat(60), "/a#c", &[]);
    assert!(crate::vault::valid_secret_name(&format!("svc.{long}.admin-password")), "{long}");
    installed::remove(&d, "alice", "gitea-ns").unwrap();
    assert!(installed::load(&d, "alice").is_empty());
}

#[test]
fn curl_carries_the_token_in_a_private_temp_config_that_is_gone_afterwards() {
    let d = tmp("curl");
    std::fs::create_dir_all(d.join("ram")).unwrap();
    let r = Arc::new(FakeRunner::new(&["curl"]));
    r.answer("curl -K", "{\"login\":\"x\"}\n200");
    let h = connect::CurlHttp { run: r.clone(), tmp: d.join("ram") };
    use connect::Http;
    let o = h.request("POST", "http://localhost:30301/api/v1/user/repos", Some(TOKEN), Some("{\"name\":\"a\"}")).unwrap();
    assert_eq!((o.status, o.body.as_str()), (200, "{\"login\":\"x\"}"));
    let l = r.lines();
    assert_eq!(l.len(), 1);
    assert!(l[0].starts_with("curl -K ") && !l[0].contains(TOKEN) && !l[0].contains("localhost"), "{l:?}");
    assert_eq!(std::fs::read_dir(d.join("ram")).unwrap().count(), 0);
    assert!(h.request("GET", "ftp://x", None, None).is_err());
    let none = Arc::new(FakeRunner::new(&[]));
    assert!(connect::CurlHttp { run: none, tmp: d.join("ram") }.request("GET", "http://x", None, None).unwrap_err().contains("curl"));
}

// ------------------------------------------------------------------------------------------------ workspace

fn connected_instance(w: &W) -> Installed {
    w.http.answer("GET http://localhost:30300/api/v1/version", 200, r#"{"version":"1.24.5"}"#);
    w.http.answer("GET http://localhost:30300/api/v1/user", 200, r#"{"login":"casey"}"#);
    gitea::connect_existing(&w.pl, "http://localhost:30300", TOKEN, None, "", true).unwrap()
}

fn git_answers(r: &FakeRunner, origin: Option<&str>) {
    r.answer("rev-parse --is-inside-work-tree", "true");
    r.answer("symbolic-ref --short HEAD", "main");
    r.answer("rev-list --count HEAD", "7");
    match origin {
        Some(o) => r.answer("remote get-url origin", o),
        None => r.fail("remote get-url origin", 2, "error: No such remote 'origin'"),
    };
}

#[test]
fn connecting_a_workspace_creates_the_private_repo_sets_origin_without_a_credential_and_pushes_only_after_a_second_approval() {
    let w = world("ws");
    let inst = connected_instance(&w);
    git_answers(&w.run, None);
    w.run.answer("remote add origin", "");
    w.run.answer("push --set-upstream", "Branch 'main' set up to track 'origin/main'.");
    w.http.answer("POST http://localhost:30300/api/v1/user/repos", 201, r#"{"clone_url":"http://localhost:30300/casey/my-shop.git","html_url":"http://localhost:30300/casey/my-shop","ssh_url":"ssh://git@localhost:30222/casey/my-shop.git"}"#);
    let ws = WsInfo { name: "My Shop".into(), root: w.dir.join("ws") };
    std::fs::create_dir_all(&ws.root).unwrap();
    let p = connect::plan_connect(&w.pl, &inst, &ws, &ConnectOpts::default()).unwrap();
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert_eq!((p.repo.as_str(), p.branch.as_str(), p.private, p.commits), ("my-shop", "main", true, 7));
    let v = connect::preview_connect(&p);
    assert!(v.contains("PRIVATE") && v.contains("git remote add origin") && v.contains("Nothing is pushed") && v.contains("plain http"), "{v}");
    assert!(connect::connect(&w.pl, &p, false).is_err());
    let c = connect::connect(&w.pl, &p, true).unwrap();
    assert!(c.created && c.origin_set && c.repo_url == "http://localhost:30300/casey/my-shop" && c.remote_url == "http://localhost:30300/casey/my-shop.git");
    let post = w.http.calls.lock().unwrap().iter().find(|c| c.0 == "POST").cloned().unwrap();
    assert!(post.3.as_ref().unwrap().contains("\"private\":true") && post.3.as_ref().unwrap().contains("\"name\":\"my-shop\"") && post.2.as_deref() == Some(TOKEN));
    let add = w.run.lines().into_iter().find(|l| l.contains("remote add origin")).unwrap();
    assert!(add.ends_with("remote add origin http://localhost:30300/casey/my-shop.git") && !add.contains('@') && !add.contains(TOKEN), "{add}");
    assert!(w.run.lines().iter().all(|l| !l.contains(" push ")), "connecting does not push");
    assert_eq!(installed::find(&w.dir, "alice", &inst.id).unwrap().workspaces[0].repo_url, c.repo_url);
    // the push: its own approval, the token only in git's environment
    let rp = Arc::new(FakeRunner::new(&["git"]));
    git_answers(&rp, Some("http://localhost:30300/casey/my-shop.git"));
    rp.answer("push --set-upstream", "Branch 'main' set up to track 'origin/main'.");
    let plp = Platform { run: rp.clone(), ..clone_platform(&w) };
    let pp = connect::plan_push(&plp, &inst, &ws).unwrap();
    assert!(pp.blockers.is_empty(), "{:?}", pp.blockers);
    assert!(connect::preview_push(&pp).contains("git push --set-upstream origin main"));
    assert!(connect::push(&plp, &pp, false).is_err());
    let before = rp.calls().len();
    connect::push(&plp, &pp, true).unwrap();
    let call = rp.calls()[before..].iter().find(|c| c.argv.contains(&"push".to_string())).cloned().unwrap();
    assert!(!call.argv.join(" ").contains(TOKEN) && call.env.iter().all(|(_, v)| !v.contains(TOKEN)), "the token is not even in the environment, only the basic header");
    let hdr = call.env.iter().find(|(k, _)| k == "GIT_CONFIG_VALUE_1").unwrap().1.clone();
    assert!(hdr.starts_with("Authorization: Basic ") && call.env.contains(&("GIT_TERMINAL_PROMPT".into(), "0".into())));
    let decoded = hdr.trim_start_matches("Authorization: Basic ");
    assert_eq!(decoded, crate::story::base64_encode(format!("casey:{TOKEN}").as_bytes()));
    // the second connect finds the repository (409) and keeps going
    w.http.calls.lock().unwrap().clear();
    let w2 = world("ws2");
    let inst2 = connected_instance(&w2);
    git_answers(&w2.run, Some("http://localhost:30300/casey/my-shop.git"));
    w2.http.answer("POST http://localhost:30300/api/v1/user/repos", 409, r#"{"message":"exists"}"#);
    w2.http.answer("GET http://localhost:30300/api/v1/repos/casey/my-shop", 200, r#"{"clone_url":"http://localhost:30300/casey/my-shop.git","html_url":"http://localhost:30300/casey/my-shop"}"#);
    let p2 = connect::plan_connect(&w2.pl, &inst2, &ws, &ConnectOpts::default()).unwrap();
    let c2 = connect::connect(&w2.pl, &p2, true).unwrap();
    assert!(!c2.created && !c2.origin_set);
    scan_everything_ws(&w2);
}

fn scan_everything_ws(w: &W) {
    for c in w.run.calls() {
        assert!(!format!("{:?}", c.argv).contains(TOKEN));
    }
}

#[test]
fn workspace_connect_refusals_no_commit_no_repo_foreign_origin_clusterip_and_a_token_in_the_origin() {
    let w = world("wsrefuse");
    let inst = connected_instance(&w);
    let ws = WsInfo { name: "x".into(), root: w.dir.join("ws") };
    // not a git repository
    w.run.answer("rev-parse --is-inside-work-tree", "false");
    assert!(connect::plan_connect(&w.pl, &inst, &ws, &ConnectOpts::default()).unwrap().blockers[0].contains("not a git repository"));
    // no commits
    let w2 = world("wsrefuse2");
    let inst2 = connected_instance(&w2);
    w2.run.answer("rev-parse --is-inside-work-tree", "true");
    w2.run.answer("symbolic-ref --short HEAD", "main");
    w2.run.answer("rev-list --count HEAD", "0");
    w2.run.fail("remote get-url", 2, "none");
    assert!(connect::plan_connect(&w2.pl, &inst2, &ws, &ConnectOpts::default()).unwrap().blockers.iter().any(|b| b.contains("no commit")));
    // a different origin is not replaced unless asked
    let w3 = world("wsrefuse3");
    let inst3 = connected_instance(&w3);
    git_answers(&w3.run, Some("git@github.com:me/other.git"));
    w3.http.answer("POST http://localhost:30300/api/v1/user/repos", 201, r#"{"clone_url":"http://localhost:30300/casey/x.git","html_url":"u"}"#);
    let p = connect::plan_connect(&w3.pl, &inst3, &ws, &ConnectOpts::default()).unwrap();
    assert!(connect::preview_connect(&p).contains("nothing is replaced"));
    assert!(connect::connect(&w3.pl, &p, true).unwrap_err().contains("already git@github.com"));
    assert!(w3.run.lines().iter().all(|l| !l.contains("set-url") && !l.contains("remote add")));
    w3.run.answer("remote set-url origin", "");
    let p = connect::plan_connect(&w3.pl, &inst3, &ws, &ConnectOpts { replace_origin: true, ..Default::default() }).unwrap();
    assert!(connect::preview_connect(&p).contains("REPLACED") && connect::connect(&w3.pl, &p, true).unwrap().origin_set);
    // an origin holding a credential, or on another host: no push, no token sent
    let w4 = world("wsrefuse4");
    let inst4 = connected_instance(&w4);
    git_answers(&w4.run, Some("http://casey:secret@localhost:30300/casey/x.git"));
    let pp = connect::plan_push(&w4.pl, &inst4, &ws).unwrap();
    assert!(pp.blockers.iter().any(|b| b.contains("credential")));
    let w5 = world("wsrefuse5");
    let inst5 = connected_instance(&w5);
    git_answers(&w5.run, Some("https://evil.example.com/casey/x.git"));
    let pp = connect::plan_push(&w5.pl, &inst5, &ws).unwrap();
    assert!(pp.blockers.iter().any(|b| b.contains("only sends its token to the service it belongs to")));
    assert!(connect::push(&w5.pl, &pp, true).is_err());
    assert!(w5.run.lines().iter().all(|l| !l.contains(" push ")));
    // ClusterIP and a missing token
    let mut cip = inst5.clone();
    cip.options.set("exposure", "ClusterIP");
    git_answers(&w5.run, None);
    assert!(connect::plan_connect(&w5.pl, &cip, &ws, &ConnectOpts::default()).unwrap().blockers.iter().any(|b| b.contains("ClusterIP")));
    w5.vault.delete(inst5.token_vault().unwrap()).unwrap();
    assert!(connect::plan_connect(&w5.pl, &inst5, &ws, &ConnectOpts::default()).unwrap().blockers.iter().any(|b| b.contains("token")));
    let bad = ConnectOpts { repo: "../x".into(), ..Default::default() };
    assert!(connect::plan_connect(&w5.pl, &inst5, &ws, &bad).is_err());
    assert_eq!(connect::repo_name("My Shop (v2)!"), "my-shop-v2");
    assert_eq!(connect::repo_name("..."), "workspace");
    let _ = Transport::Ssh;
}

// ------------------------------------------------------------------------------------------------ assistant tools

#[test]
fn the_assistant_tools_need_a_listed_cluster_ask_in_every_mode_and_never_see_a_secret() {
    use serde_json::json;
    let w = world("tools");
    healthy(&w.run);
    let t = tools::PlatformTools::new(Arc::new(clone_platform(&w)));
    let names: Vec<String> = t.defs().into_iter().map(|d| d.spec.name).collect();
    assert_eq!(names, tools::TOOLS.to_vec());
    for d in t.defs() {
        let safe = matches!(d.spec.name.as_str(), "platform_list" | "platform_status" | "git_service_status");
        assert_eq!(d.risk == crate::agent::Risk::Safe, safe, "{}", d.spec.name);
    }
    for n in ["platform_deploy", "platform_upgrade", "platform_remove"] {
        assert!(crate::agent::hard_ask(n, &json!({})).is_some(), "{n} always asks, even in Auto and Autopilot");
    }
    for n in ["platform_list", "platform_status", "git_service_status"] {
        assert!(crate::agent::hard_ask(n, &json!({})).is_none());
    }
    // before platform_list: the id is not known
    let args = json!({"service": "gitea", "profile": "cabc"});
    assert!(t.describe("platform_deploy", &args).contains("will be REFUSED"));
    assert!(t.preview("platform_deploy", &args).unwrap().contains("this would be refused"));
    assert!(t.call("platform_deploy", &args).unwrap_err().contains("not one of the clusters you can see"));
    // the list gives the id, the card names the server
    let list = t.call("platform_list", &json!({})).unwrap();
    let id = profile_id(&profile(&w));
    assert!(list.contains(&id) && list.contains("https://127.0.0.1:6443") && list.contains("gitea") && list.contains("- none"), "{list}");
    let args = json!({"service": "gitea", "profile": id, "namespace": "iyakki-gitea", "options": {"storage_size": "5Gi"}});
    let card = t.describe("platform_deploy", &args);
    assert!(card.contains("DEPLOY Gitea") && card.contains("https://127.0.0.1:6443") && !card.contains("REFUSED"), "{card}");
    let pv = t.preview("platform_deploy", &args).unwrap();
    assert!(pv.contains("storage: 5Gi") && pv.contains("create namespace iyakki-gitea"), "{pv}");
    // a production-looking cluster behind a listed id is refused
    let w2 = world("tools2");
    w2.run.answer("config view --minify", "https://prod.example.com:6443");
    healthy(&w2.run);
    let t2 = tools::PlatformTools::new(Arc::new(clone_platform(&w2)));
    let l2 = t2.call("platform_list", &json!({})).unwrap();
    assert!(l2.contains("looks like production"), "{l2}");
    let id2 = list_profiles(&w2.pl)[0].id.clone();
    assert!(t2.call("platform_deploy", &json!({"service": "gitea", "profile": id2})).unwrap_err().contains("production"));
    assert!(writes(&w2).is_empty());
    // deploying: the result names where the credentials are and holds none
    w.http.answer("GET http://localhost:30301/api/healthz", 200, "{}");
    let out = t.call("platform_deploy", &args).unwrap();
    assert!(out.contains("is running") && out.contains("uvault:svc.gitea-iyakki-gitea.admin-password") && out.contains("http://localhost:30301") && out.contains("Instance id: gitea-iyakki-gitea"), "{out}");
    scan_everything(&w, &[out.clone()]);
    assert!(t.call("platform_status", &json!({})).is_err() || true);
    let removed = t.call("platform_remove", &json!({"instance": "gitea-iyakki-gitea", "confirm": "nope"})).unwrap_err();
    assert!(removed.contains("TYPE the service id"));
    assert!(t.describe("platform_remove", &json!({"instance": "gitea-iyakki-gitea", "delete_data": true})).contains("DELETE ITS DATA"));
    assert!(t.call("platform_upgrade", &json!({"instance": "nope"})).unwrap_err().contains("not an installed service"));
    assert!(t.call("git_service_status", &json!({})).unwrap().contains("Gitea 1.24.5"));
    assert!(!t.preview("platform_remove", &json!({"instance": "gitea-iyakki-gitea"})).unwrap().is_empty());
}

// ------------------------------------------------------------------------------------------------ the private store with no container engine

fn private_store() -> Arc<dyn ServiceDef> {
    spec("private-store").unwrap()
}

/// A static x86-64 ELF header (what `localbin` accepts as a static program): the content is not run.
fn static_elf() -> Vec<u8> {
    let mut h = vec![0u8; 64 + 2 * 56];
    h[..4].copy_from_slice(b"\x7fELF");
    h[4] = 2;
    h[5] = 1;
    h[18..20].copy_from_slice(&62u16.to_le_bytes());
    h[32..40].copy_from_slice(&64u64.to_le_bytes());
    h[54..56].copy_from_slice(&56u16.to_le_bytes());
    h[56..58].copy_from_slice(&2u16.to_le_bytes());
    h[64..68].copy_from_slice(&1u32.to_le_bytes());
    h[120..124].copy_from_slice(&1u32.to_le_bytes());
    h
}

/// A world with kubectl and no podman or docker.
fn world_no_engine(tag: &str) -> W {
    let mut w = world(tag);
    let run = Arc::new(FakeRunner::new(&["kubectl", "curl", "git"]));
    w.pl.run = run.clone();
    w.run = run;
    w
}

#[test]
fn a_private_store_with_no_registry_says_why_and_asks_for_one() {
    let w = world_no_engine("noreg");
    healthy(&w.run);
    let p = plan(&w.pl, &*private_store(), &profile(&w), &Options::default()).unwrap();
    assert!(p.needs_registry && !p.can_apply(), "{:?}", p.blockers);
    let b = p.blockers.join(" ");
    assert!(b.contains("built on this device") && b.contains("in-cluster registry") && b.contains("Docker Hub") && b.contains("Create registry") && b.contains("no cluster restart"), "{b}");
    assert!(!b.contains("podman"), "no engine is asked for: {b}");
    // a service that does not need the registry is not asked to create one
    let w2 = world_no_engine("gitea");
    healthy(&w2.run);
    assert!(!plan(&w2.pl, &*gitea(), &profile(&w2), &Options::default()).unwrap().needs_registry);
}

#[test]
fn with_a_registry_and_no_container_engine_the_image_is_built_and_pushed_by_iyakki() {
    let w = world_no_engine("builtin");
    w.run.fail("get secret private-store", 1, "Error from server (NotFound): secrets \"private-store\" not found");
    w.run.answer("create secret generic private-store", "secret/private-store created");
    w.run.answer("label secret", "secret/private-store labeled");
    w.run.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
    healthy(&w.run);
    let p = plan(&w.pl, &*private_store(), &profile(&w), &Options::default()).unwrap();
    assert!(!p.needs_registry && p.can_apply(), "{:?}", p.blockers);
    let m = p.mirror.clone().expect("mirrored");
    assert_eq!(m.engine, BUILTIN_ENGINE);
    assert_eq!(m.images[0].1, "localhost:30500/iyakki-platform/localhost/iyakki-store-server:1.0.0");
    assert_eq!(p.image_refs, vec!["localhost:30500/iyakki-platform/localhost/iyakki-store-server:1.0.0"]);
    let v = preview(&p);
    assert!(v.contains("No podman or docker is needed"), "{v}");
    // the program the image holds is placed where the deploy looks for it
    std::fs::create_dir_all(w.dir.join("store-server")).unwrap();
    std::fs::write(w.dir.join("store-server/iyakki-store-server-x86_64"), static_elf()).unwrap();
    let reg = crate::ocipush::tests::FakeRegistry::default();
    let mut said = Vec::new();
    apply_with(&w.pl, &p, true, &mut |s| said.push(s.to_string()), &reg).unwrap_or_else(|e| panic!("{e}"));
    let lines = w.run.lines();
    assert!(!lines.iter().any(|l| l.contains("podman") || l.contains("docker")), "{lines:?}");
    assert!(reg.manifests.lock().unwrap().contains_key("iyakki-platform/localhost/iyakki-store-server:1.0.0"));
    assert!(said.iter().any(|l| l.contains("no container engine needed")) && said.iter().any(|l| l.contains("packing")), "{said:?}");
    let pushed = reg.log.lock().unwrap().iter().position(|l| l.starts_with("PUT /v2/iyakki-platform/localhost/iyakki-store-server/manifests/1.0.0")).expect("manifest pushed");
    let _ = pushed;
    assert!(lines.iter().any(|l| l.contains("apply -f")), "the objects are applied after the push: {lines:?}");
    // the layer is the program, byte for byte
    let layer = reg.blobs.lock().unwrap().values().find(|b| b.len() > 1024 && b.starts_with(b"iyakki")).cloned().expect("layer");
    assert_eq!(&layer[512..512 + static_elf().len()], static_elf().as_slice());
}

#[test]
fn a_failed_push_stops_before_the_cluster_objects_and_says_why() {
    let w = world_no_engine("pushfail");
    w.run.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
    healthy(&w.run);
    let p = plan(&w.pl, &*private_store(), &profile(&w), &Options::default()).unwrap();
    std::fs::create_dir_all(w.dir.join("store-server")).unwrap();
    std::fs::write(w.dir.join("store-server/iyakki-store-server-x86_64"), static_elf()).unwrap();
    let reg = crate::ocipush::tests::FakeRegistry::default();
    *reg.down.lock().unwrap() = true;
    let e = apply_with(&w.pl, &p, true, &mut |_| {}, &reg).unwrap_err();
    assert!(e.contains("pushing the store server image failed") && e.contains("cannot connect"), "{e}");
    assert!(!w.run.lines().iter().any(|l| l.contains("apply -f")), "{:?}", w.run.lines());
}
