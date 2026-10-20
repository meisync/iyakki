// #[cfg(test)] -- this whole file is test code (privatemanage.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! The private store's state, Repair, Reset and Remove with fakes: a scripted kubectl, an in-memory vault, temp folders. Nothing here
//! touches a cluster, a drive or the network.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::devcluster::{Choice, Env, Source};
use crate::platform::connect::FakeHttp;
use crate::platform::{MemVault, UrlInfo, VaultApi};
use crate::store::catalog::SourceKind;
use crate::wsexec::{FakeRunner, Out};

const NS: &str = "iyakki-private-store";
const INSTANCE: &str = "private-store-iyakki-private-store";
const OURS: &str = r#"{"metadata":{"name":"iyakki-private-store","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/service":"private-store"}},"status":{"phase":"Active"}}"#;
const TERMINATING: &str = r#"{"metadata":{"name":"iyakki-private-store","deletionTimestamp":"2026-01-01T00:00:00Z","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/service":"private-store"}},"status":{"phase":"Terminating"}}"#;
const FOREIGN: &str = r#"{"metadata":{"name":"iyakki-private-store","labels":{}},"status":{"phase":"Active"}}"#;
const NOT_FOUND: &str = "Error from server (NotFound): namespaces \"iyakki-private-store\" not found";
const REFUSED: &str = "The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port? connection refused";
const TOKEN: &str = "TOKENVALUE0123456789TOKENVALUE0123456789";

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-pm-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What the scripted cluster holds.
#[derive(Clone, Default)]
struct Sc {
    /// None: the namespace is not found; Some(json): it is.
    ns: Option<&'static str>,
    down: bool,
    /// (desired, ready)
    dep: Option<(u32, u32)>,
    bad_pod: Option<&'static str>,
    svc: bool,
    pvc: bool,
    secret: bool,
    endpoints: bool,
}

impl Sc {
    fn full() -> Sc {
        Sc { ns: Some(OURS), dep: Some((1, 1)), svc: true, pvc: true, secret: true, endpoints: true, ..Default::default() }
    }
    fn only_ns() -> Sc {
        Sc { ns: Some(OURS), ..Default::default() }
    }
}

fn script(r: &FakeRunner, sc: &Sc) {
    if sc.down {
        r.fail("get namespace", 1, REFUSED);
        return;
    }
    match sc.ns {
        None => {
            r.fail("get namespace", 1, NOT_FOUND);
            return;
        }
        Some(j) => {
            r.answer("get namespace iyakki-private-store", j);
        }
    }
    let mut items = String::new();
    if let Some((d, rd)) = sc.dep {
        items.push_str(&format!(r#"{{"kind":"Deployment","metadata":{{"name":"private-store"}},"spec":{{"replicas":{d}}},"status":{{"readyReplicas":{rd}}}}}"#));
        let waiting = sc.bad_pod.map_or(String::new(), |w| format!(r#","containerStatuses":[{{"state":{{"waiting":{{"reason":"{w}"}}}}}}]"#));
        items.push_str(&format!(r#",{{"kind":"Pod","metadata":{{"name":"private-store-abc"}},"status":{{"phase":"Running"{waiting}}}}}"#));
    }
    r.answer("get deployments,pods -n iyakki-private-store", &format!(r#"{{"items":[{items}]}}"#));
    let mut names = String::new();
    for (on, n) in [(sc.svc, "service/private-store"), (sc.pvc, "persistentvolumeclaim/private-store-data"), (sc.secret, "secret/private-store")] {
        if on {
            names.push_str(n);
            names.push('\n');
        }
    }
    r.answer("get services,persistentvolumeclaims,secrets -n iyakki-private-store", &names);
    r.answer("get endpoints private-store", &if sc.endpoints { r#"{"subsets":[{"addresses":[{"ip":"10.1.0.5"}]}]}"#.to_string() } else { r#"{"subsets":[]}"#.to_string() });
}

struct W {
    pl: Platform,
    run: Arc<FakeRunner>,
    vault: Arc<MemVault>,
    dir: std::path::PathBuf,
    cluster: ClusterRef,
    store: Store,
}

fn world(tag: &str, sc: &Sc) -> W {
    let dir = tmp(tag);
    let kubeconfig = dir.join("kubeconfig.yaml");
    std::fs::write(&kubeconfig, "apiVersion: v1\nkind: Config\n").unwrap();
    let kubeconfig = kubeconfig.display().to_string();
    Choice { kubeconfig: kubeconfig.clone(), context: String::new() }.save(&dir, "alice").unwrap();
    std::fs::create_dir_all(dir.join("ram")).unwrap();
    let run = Arc::new(FakeRunner::new(&["kubectl", "curl"]));
    script(&run, sc);
    let vault = Arc::new(MemVault::default());
    let pl = Platform {
        run: run.clone(),
        dir: dir.clone(),
        user: "alice".into(),
        vault: vault.clone(),
        audit: Arc::new(|_, _| {}),
        env: Env { home: dir.clone(), ..Default::default() },
        tmp: dir.join("ram"),
        http: Arc::new(FakeHttp::default()),
        gen: Arc::new(|n| format!("{:G<n$}", "GENERATED")),
        live: None,
    };
    let cluster = ClusterRef { name: "k3s".into(), kubeconfig, context: String::new(), server: "https://127.0.0.1:6443".into(), source: Source::K3s };
    let store = Store::open(&dir.join("store"));
    W { pl, run, vault, dir, cluster, store }
}

fn record(w: &W) -> Installed {
    Installed {
        id: INSTANCE.into(),
        service: ID.into(),
        title: "Private store".into(),
        version: "1.0.0".into(),
        kind: "store".into(),
        cluster: Some(w.cluster.clone()),
        namespace: NS.into(),
        created_namespace: true,
        urls: vec![UrlInfo { label: "Catalog".into(), url: "http://10.0.0.5:30300/catalog.json".into(), kind: "catalog".into() }, UrlInfo { label: "Private store".into(), url: "http://10.0.0.5:30300".into(), kind: "web".into() }],
        vault: vec![format!("svc.{INSTANCE}.admin-token"), format!("svc.{INSTANCE}.catalog-seed"), format!("svc.{INSTANCE}.publisher-bob")],
        ..Default::default()
    }
}

fn with_record(w: &W) {
    installed::upsert(&w.dir, "alice", record(w)).unwrap();
}

fn with_vault(w: &W) {
    w.vault.set(&format!("svc.{INSTANCE}.admin-token"), TOKEN).unwrap();
    w.vault.set(&format!("svc.{INSTANCE}.catalog-seed"), "seedvalue").unwrap();
}

fn state(w: &W) -> StoreState {
    private_store_state(&w.pl, Some(&w.store), &w.cluster, NS).state
}

fn broken(w: &W) -> (Why, String) {
    match state(w) {
        StoreState::Broken { why, reason } => (why, reason),
        s => panic!("not broken: {s:?}"),
    }
}

fn writes(r: &FakeRunner) -> Vec<String> {
    r.lines().into_iter().filter(|l| ["create ", "apply ", "label ", "delete ", "patch ", "rollout restart", " scale "].iter().any(|k| l.contains(k))).collect()
}

// ------------------------------------------------------------------------------------------------ the states

#[test]
fn every_state_is_detected_in_plain_words() {
    // nothing anywhere
    let w = world("s-none", &Sc::default());
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert_eq!(r.state, StoreState::NotInstalled);
    assert!(r.offers().deploy && !r.offers().repair && !r.offers().remove && r.text().contains("NOT INSTALLED"));
    // healthy: namespace, deployment ready, service, token known, recorded
    let w = world("s-ok", &Sc::full());
    with_record(&w);
    with_vault(&w);
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert_eq!(r.state, StoreState::Healthy, "{}", r.text());
    assert!(r.offers().reset && r.offers().remove && !r.offers().repair && !r.offers().deploy);
    // only the namespace exists
    let w = world("s-ns", &Sc::only_ns());
    match state(&w) {
        StoreState::Partial { present, missing } => {
            assert_eq!(present, vec!["the namespace"]);
            assert_eq!(missing.len(), 4, "{missing:?}");
            assert!(missing.iter().any(|m| m.contains("Deployment")) && missing.iter().any(|m| m.contains("PVC")));
        }
        s => panic!("{s:?}"),
    }
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(r.offers().repair && r.offers().reset && r.offers().remove, "a namespace alone can be repaired, reset or removed");
    assert!(r.text().contains("Missing:") && r.text().contains("Repair puts back only what is missing"));
    // the volume and the Secret exist, the server does not (the install stopped before the deployment)
    let w = world("s-nodep", &Sc { pvc: true, secret: true, ..Sc::only_ns() });
    match state(&w) {
        StoreState::Partial { present, missing } => {
            assert!(present.iter().any(|p| p.contains("PVC")) && present.iter().any(|p| p.contains("Secret")), "{present:?}");
            assert_eq!(missing.len(), 2, "{missing:?}");
        }
        s => panic!("{s:?}"),
    }
    // crash loop
    let w = world("s-crash", &Sc { bad_pod: Some("CrashLoopBackOff"), dep: Some((1, 0)), ..Sc::full() });
    let (why, reason) = broken(&w);
    assert_eq!(why, Why::CrashLoop);
    assert!(reason.contains("CrashLoopBackOff"));
    // deployed but nothing answers
    let w = world("s-noend", &Sc { dep: Some((1, 0)), endpoints: false, ..Sc::full() });
    assert_eq!(broken(&w).0, Why::NoEndpoints);
    // the token is gone from the vault
    let w = world("s-token", &Sc::full());
    with_record(&w);
    let (why, reason) = broken(&w);
    assert_eq!(why, Why::TokenLost);
    assert!(reason.contains("admin token") && reason.contains("Repair"));
    // namespace stuck Terminating
    let w = world("s-term", &Sc { ns: Some(TERMINATING), ..Default::default() });
    let (why, reason) = broken(&w);
    assert_eq!(why, Why::Terminating);
    assert!(reason.contains("stuck in Terminating") && reason.contains("finalizer"));
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(r.offers().reset && !r.offers().repair && !r.offers().deploy);
    // catalog entry (record) present but the cluster object is gone
    let w = world("s-catonly", &Sc::default());
    with_record(&w);
    let (why, reason) = broken(&w);
    assert_eq!(why, Why::CatalogOnly);
    assert!(reason.contains("no such namespace"));
    assert!(private_store_state(&w.pl, Some(&w.store), &w.cluster, NS).offers().repair);
    // ... and a STORE catalog entry alone is also "listed but gone"
    let w = world("s-srcnly", &Sc::default());
    with_record(&w);
    w.store.add_source(&Actor::system(), "Mine", SourceKind::Url, "http://10.0.0.5:30300/catalog.json").unwrap();
    assert_eq!(private_store_state(&w.pl, Some(&w.store), &w.cluster, NS).source.map(|s| s.1), Some("Mine".to_string()));
    // cluster object present but no record on this device
    let w = world("s-norec", &Sc::full());
    with_vault(&w);
    let (why, reason) = broken(&w);
    assert_eq!(why, Why::NoRecord);
    assert!(reason.contains("no record"));
    // a namespace that is not IYAKKI's
    let w = world("s-foreign", &Sc { ns: Some(FOREIGN), ..Default::default() });
    let (why, _) = broken(&w);
    assert_eq!(why, Why::NotOurs);
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(!r.offers().reset && !r.offers().remove && !r.offers().repair);
}

#[test]
fn a_cluster_that_is_down_or_a_machine_with_no_kubectl_is_explained_and_nothing_is_changed() {
    let w = world("down", &Sc { down: true, ..Default::default() });
    with_record(&w);
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    let StoreState::Broken { why, reason } = &r.state else { panic!("{:?}", r.state) };
    assert_eq!(*why, Why::ClusterDown);
    assert!(reason.contains("does not answer") && reason.contains("Start the cluster") && reason.contains("Nothing was changed"), "{reason}");
    assert!(r.offers().start_cluster && !r.offers().reset && !r.offers().repair && !r.offers().remove, "a record must not make a down cluster look like a missing store");
    assert!(writes(&w.run).is_empty());
    // no kubectl (and no managed k3s): said at once, no command is run
    let mut w = world("nokubectl", &Sc::full());
    let run = Arc::new(FakeRunner::new(&["curl"]));
    w.pl.run = run.clone();
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    let StoreState::Broken { why, reason } = &r.state else { panic!() };
    assert_eq!(*why, Why::NoKubectl);
    assert!(reason.contains("no kubectl") && reason.contains("k3s"), "{reason}");
    assert!(run.calls().is_empty());
    let a = JobArgs { kubeconfig: "/k".into(), context: String::new(), namespace: NS.into(), reset: true, delete_data: true, wait_secs: 20, force_finalizers: false };
    let run: Arc<dyn Runner> = run;
    assert!(run_job(&run, &a, &|_| {}, &mut |_| {}).unwrap_err().contains("no kubectl"));
}

#[test]
fn detection_only_reads_and_only_in_the_selected_cluster() {
    let w = world("scope", &Sc::full());
    // a record of a store in ANOTHER cluster must not be mistaken for this one, and its cluster is never contacted
    let mut other = record(&w);
    other.id = "private-store-iyakki-private-store-ab12".into();
    other.cluster = Some(ClusterRef { name: "lab".into(), kubeconfig: "/home/x/lab.yaml".into(), context: "lab".into(), server: "https://10.9.9.9:6443".into(), source: Source::UserKube });
    installed::upsert(&w.dir, "alice", other).unwrap();
    with_vault(&w);
    let r = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(matches!(r.state, StoreState::Broken { why: Why::NoRecord, .. }), "{:?}", r.state);
    assert!(r.record.is_none() && r.elsewhere.as_ref().is_some_and(|e| e.contains("not touched")), "{:?}", r.elsewhere);
    let lines = w.run.lines();
    assert!(!lines.is_empty());
    for l in &lines {
        assert!(l.contains(&format!("--kubeconfig {}", w.cluster.kubeconfig)) && !l.contains("lab.yaml") && !l.contains("--context lab"), "{l}");
    }
    assert!(writes(&w.run).is_empty(), "detection never writes: {lines:?}");
    // a chosen context is passed on
    let mut c = w.cluster.clone();
    c.context = "work".into();
    let w2 = world("scope2", &Sc::full());
    let _ = private_store_state(&w2.pl, None, &c, NS);
    assert!(w2.run.lines().iter().all(|l| l.contains("--context work")));
}

// ------------------------------------------------------------------------------------------------ a scripted cluster that changes

type Rule = Box<dyn Fn(&str, usize) -> Option<Out> + Send + Sync>;

/// A runner whose answers can depend on how often a command was seen (a namespace that takes a while to go).
struct Seq {
    rule: Rule,
    calls: Mutex<Vec<String>>,
    programs: Vec<&'static str>,
}

impl Seq {
    fn new(programs: &[&'static str], rule: Rule) -> Arc<Seq> {
        Arc::new(Seq { rule, calls: Mutex::new(vec![]), programs: programs.to_vec() })
    }
    fn lines(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|l| l.replace(" --request-timeout=8s", "").replace(" --request-timeout=130s", "")).collect()
    }
}

impl Runner for Seq {
    fn run(&self, _cwd: &std::path::Path, argv: &[String], _env: &[(String, String)], _t: Duration, _cap: usize, _s: Option<crate::wsexec::Sink>) -> Out {
        let line = argv.join(" ");
        let n = self.calls.lock().unwrap().iter().filter(|l| **l == line).count();
        self.calls.lock().unwrap().push(line.clone());
        (self.rule)(&line, n).unwrap_or_else(|| Out { code: Some(0), stdout: String::new(), ..Default::default() })
    }
    fn has(&self, p: &str) -> bool {
        self.programs.contains(&p)
    }
}

fn ok(s: &str) -> Out {
    Out { code: Some(0), stdout: s.into(), ..Default::default() }
}

fn bad(s: &str) -> Out {
    Out { code: Some(1), stderr: s.into(), ..Default::default() }
}

fn args(reset: bool, delete_data: bool, force: bool) -> JobArgs {
    JobArgs { kubeconfig: "/kc".into(), context: String::new(), namespace: NS.into(), reset, delete_data, wait_secs: 20, force_finalizers: force }
}

fn pos(lines: &[String], what: &str) -> usize {
    lines.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("no `{what}` in {lines:#?}"))
}

#[test]
fn reset_deletes_the_workloads_then_the_namespace_in_that_order_and_waits_until_it_is_gone() {
    // the namespace shows Terminating twice, then it is gone
    let gets = Arc::new(AtomicUsize::new(0));
    let g2 = gets.clone();
    let run = Seq::new(&["kubectl"], Box::new(move |l, _| {
        if l.contains("get namespace") {
            let n = g2.fetch_add(1, Ordering::SeqCst);
            return Some(match n {
                0 => ok(OURS),
                1 | 2 => ok(TERMINATING),
                _ => bad(NOT_FOUND),
            });
        }
        None
    }));
    let slept = Arc::new(AtomicUsize::new(0));
    let s2 = slept.clone();
    let r: Arc<dyn Runner> = run.clone();
    let mut said = Vec::new();
    run_job(&r, &args(true, true, false), &move |_| { s2.fetch_add(1, Ordering::SeqCst); }, &mut |l| said.push(l.to_string())).unwrap_or_else(|e| panic!("{e}"));
    let lines = run.lines();
    let first_get = pos(&lines, "get namespace");
    let wl = pos(&lines, "delete deployments,statefulsets,services,ingresses");
    let by_name = pos(&lines, "delete deployment/private-store service/private-store");
    let ns = pos(&lines, "delete namespace iyakki-private-store --wait=false");
    assert!(first_get < wl && wl < by_name && by_name < ns, "{lines:#?}");
    assert!(lines.iter().all(|l| !l.contains("delete persistentvolumeclaims")), "the namespace takes the volume with it: {lines:#?}");
    assert!(lines[ns..].iter().filter(|l| l.contains("get namespace")).count() >= 3, "it waits by looking again: {lines:#?}");
    assert_eq!(slept.load(Ordering::SeqCst), 2, "slept between the looks");
    assert!(said.iter().any(|l| l.contains("waiting for namespace")) && said.last().unwrap().contains("is gone"), "{said:?}");
    // every command is limited to the one namespace and the kubeconfig given
    assert!(lines.iter().all(|l| l.contains("--kubeconfig /kc")));
    // run it again: the namespace is gone, which is success and changes nothing
    let run2 = Seq::new(&["kubectl"], Box::new(|l, _| l.contains("get namespace").then(|| bad(NOT_FOUND))));
    let r2: Arc<dyn Runner> = run2.clone();
    let mut said = Vec::new();
    run_job(&r2, &args(true, true, false), &|_| {}, &mut |l| said.push(l.to_string())).unwrap();
    assert!(run2.lines().iter().all(|l| !l.contains("delete")), "{:?}", run2.lines());
    assert!(said[1].contains("already gone"));
}

#[test]
fn a_namespace_stuck_terminating_says_why_and_the_force_choice_clears_only_finalizers_in_that_namespace() {
    let ns_always = |l: &str| l.contains("get namespace iyakki-private-store").then(|| ok(TERMINATING));
    let left = r#"{"items":[{"kind":"PersistentVolumeClaim","metadata":{"name":"private-store-data","finalizers":["kubernetes.io/pvc-protection"]}},{"kind":"Service","metadata":{"name":"private-store"}}]}"#;
    let run = Seq::new(&["kubectl"], Box::new(move |l, _| {
        if l.contains("get persistentvolumeclaims,pods") {
            return Some(ok(left));
        }
        if l.contains("get all,persistentvolumeclaims") {
            return Some(ok("persistentvolumeclaim/private-store-data\n"));
        }
        ns_always(l)
    }));
    let r: Arc<dyn Runner> = run.clone();
    let e = run_job(&r, &args(true, true, false), &|_| {}, &mut |_| {}).unwrap_err();
    assert!(e.contains("still Terminating") && e.contains("finalizer") && e.contains("press Reset again") && e.contains("force") && e.contains("private-store-data") && e.contains("Nothing outside this namespace"), "{e}");
    assert!(run.lines().iter().all(|l| !l.contains("patch")), "finalizers are cleared only when allowed");
    // allowed
    run.calls.lock().unwrap().clear();
    let e = run_job(&r, &args(true, true, true), &|_| {}, &mut |_| {}).unwrap_err();
    let lines = run.lines();
    let patches: Vec<&String> = lines.iter().filter(|l| l.contains("patch")).collect();
    assert_eq!(patches.len(), 1, "{lines:#?}");
    assert!(patches[0].contains("persistentvolumeclaim/private-store-data -n iyakki-private-store") && patches[0].contains("finalizers"), "{}", patches[0]);
    assert!(e.contains("cleared once"), "{e}");
    // a namespace that was already Terminating is only waited for, not deleted again
    assert!(lines.iter().all(|l| !l.contains("delete ")));
}

#[test]
fn remove_keeps_or_deletes_the_data_volume_as_chosen_and_a_foreign_namespace_is_never_deleted() {
    let mk = |ns: &'static str, gone_after: usize| {
        let gets = Arc::new(AtomicUsize::new(0));
        Seq::new(&["kubectl"], Box::new(move |l, _| {
            if l.contains("get namespace") {
                let n = gets.fetch_add(1, Ordering::SeqCst);
                return Some(if n > gone_after { bad(NOT_FOUND) } else { ok(ns) });
            }
            None
        }))
    };
    // keep my published apps (default)
    let run = mk(OURS, 99);
    let r: Arc<dyn Runner> = run.clone();
    run_job(&r, &args(false, false, false), &|_| {}, &mut |_| {}).unwrap();
    let l = run.lines();
    assert!(l.iter().any(|x| x.contains("delete deployments,statefulsets,services,ingresses")));
    assert!(l.iter().all(|x| !x.contains("persistentvolumeclaim") && !x.contains("delete namespace") && !x.contains("secrets")), "{l:#?}");
    // delete everything
    let run = mk(OURS, 1);
    let r: Arc<dyn Runner> = run.clone();
    run_job(&r, &args(false, true, false), &|_| {}, &mut |_| {}).unwrap();
    let l = run.lines();
    assert!(pos(&l, "delete deployments") < pos(&l, "delete namespace iyakki-private-store"), "{l:#?}");
    // delete everything in a namespace IYAKKI manages but did not make for the store: its volume and Secret by label and name, never the namespace
    let managed_only = r#"{"metadata":{"labels":{"app.kubernetes.io/managed-by":"iyakki"}},"status":{"phase":"Active"}}"#;
    let run = mk(managed_only, 99);
    let r: Arc<dyn Runner> = run.clone();
    run_job(&r, &args(false, true, false), &|_| {}, &mut |_| {}).unwrap();
    let l = run.lines();
    assert!(l.iter().any(|x| x.contains("delete persistentvolumeclaims,secrets -n iyakki-private-store -l iyakki.dev/service=private-store")), "{l:#?}");
    assert!(l.iter().all(|x| !x.contains("delete namespace")));
    // reset needs a namespace made for the store
    let e = run_job(&(mk(managed_only, 99) as Arc<dyn Runner>), &args(true, true, false), &|_| {}, &mut |_| {}).unwrap_err();
    assert!(e.contains("never deleted as a whole"), "{e}");
    // a namespace nobody labelled is refused outright and nothing is deleted
    let run = mk(FOREIGN, 99);
    let r: Arc<dyn Runner> = run.clone();
    let e = run_job(&r, &args(false, true, false), &|_| {}, &mut |_| {}).unwrap_err();
    assert!(e.contains("not one IYAKKI manages"), "{e}");
    assert!(run.lines().iter().all(|x| !x.contains("delete")));
    // a protected namespace is refused before any command
    let mut a = args(false, true, false);
    a.namespace = "kube-system".into();
    let run = mk(OURS, 99);
    let r: Arc<dyn Runner> = run.clone();
    assert!(run_job(&r, &a, &|_| {}, &mut |_| {}).unwrap_err().contains("protected"));
    assert!(run.lines().is_empty());
    // a cluster that stops answering is a plain message
    let run = Seq::new(&["kubectl"], Box::new(|_, _| Some(bad(REFUSED))));
    let r: Arc<dyn Runner> = run.clone();
    let e = run_job(&r, &args(true, true, false), &|_| {}, &mut |_| {}).unwrap_err();
    assert!(e.contains("does not answer") && e.contains("nothing was changed"), "{e}");
}

#[test]
fn the_job_arguments_round_trip_and_bad_ones_are_refused() {
    let a = args(false, true, true);
    let argv = a.argv("/usr/bin/iyakki");
    assert_eq!(&argv[..3], &["/usr/bin/iyakki", "store-manage", "remove"]);
    assert_eq!(JobArgs::parse(&argv[2..]).unwrap(), a);
    let r = args(true, true, false);
    assert_eq!(JobArgs::parse(&r.argv("x")[2..]).unwrap(), r);
    assert!(JobArgs::parse(&["nuke".into()]).is_err());
    assert!(JobArgs::parse(&["reset".into(), "--namespace".into(), "Bad Name".into()]).is_err());
    // the job lands in the installer workspace so the top bar shows it
    let l = crate::quickinstall::Launch { exe: "/usr/bin/iyakki".into(), data: tmp("spec") };
    let s = job_spec(&l, &a);
    assert_eq!((s.label.as_str(), s.workspace.as_str()), (JOB_LABEL_REMOVE, crate::quickinstall::WORKSPACE));
    assert_eq!(job_spec(&l, &r).label, JOB_LABEL_RESET);
}

// ------------------------------------------------------------------------------------------------ approval

#[test]
fn anything_destructive_needs_the_namespace_typed_and_a_state_that_allows_it() {
    let w = world("typed", &Sc::only_ns());
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    for a in [Action::Reset, Action::Remove(RemoveChoice::default())] {
        for typed in ["", "yes", "iyakki-private", "private-store"] {
            let e = prepare(&w.pl, &rep, &a, typed, false).unwrap_err();
            assert!(e.contains("type the namespace `iyakki-private-store`") && e.contains("Nothing was changed"), "{e}");
        }
    }
    // the right word passes the typed check; the cluster guard then runs (the fake cluster is not a real one, so it may refuse for its own reasons)
    let r = prepare(&w.pl, &rep, &Action::Reset, NS, false);
    if let Err(e) = &r {
        assert!(!e.contains("type the namespace"), "{e}");
    }
    // Repair is not destructive: no typing
    assert!(!Action::Repair.destructive());
    // a state that does not allow it is refused first
    let w = world("typed2", &Sc { ns: Some(FOREIGN), ..Default::default() });
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(prepare(&w.pl, &rep, &Action::Remove(RemoveChoice::default()), NS, false).unwrap_err().contains("not possible in this state"));
    assert!(writes(&w.run).is_empty());
}

#[test]
fn the_previews_say_exactly_what_goes_and_what_stays() {
    let w = world("preview", &Sc::full());
    with_record(&w);
    with_vault(&w);
    w.store.add_source(&Actor::system(), "Mine", SourceKind::Url, "http://10.0.0.5:30300/catalog.json").unwrap();
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(rep.source.is_some());
    let keep = preview(&rep, &Action::Remove(RemoveChoice::default()));
    assert!(keep.contains("KEPT: the data volume") && keep.contains("registry is a separate thing: it is KEPT") && keep.contains("type the namespace (iyakki-private-store)"), "{keep}");
    assert!(!keep.contains("WARNING") && !keep.contains("uvault:"), "{keep}");
    let all = preview(&rep, &Action::Remove(RemoveChoice { keep_data: false, remove_registry: true }));
    for want in ["data volume (PVC) private-store-data", "the Secret private-store", "namespace iyakki-private-store", "uvault:svc.private-store-iyakki-private-store.admin-token", "uvault:svc.private-store-iyakki-private-store.publisher-bob", "catalog entry Mine", "store-token-", "installed-services record", "WILL be deleted", "WARNING", "publisher keys you trusted"] {
        assert!(all.contains(want), "missing `{want}`:\n{all}");
    }
    let reset = preview(&rep, &Action::Reset);
    assert!(reset.contains("RESET AND REINSTALL") && reset.contains("waits until the namespace is gone") && reset.contains("add the store again") && reset.contains("DELETED"), "{reset}");
    let rp = preview(&private_store_state(&world("pv2", &Sc { pvc: true, secret: true, ..Sc::only_ns() }).pl, None, &world("pv3", &Sc::default()).cluster, NS), &Action::Repair);
    assert!(rp.contains("REPAIR"), "{rp}");
}

// ------------------------------------------------------------------------------------------------ this device after the job

#[test]
fn after_a_job_only_this_stores_vault_entries_catalog_entry_and_record_go() {
    let mk = |tag: &str| {
        let w = world(tag, &Sc::full());
        with_record(&w);
        with_vault(&w);
        w.vault.set(&format!("svc.{INSTANCE}.publisher-bob"), "bobtoken").unwrap();
        // another store of the person's, with its own catalog entry and tokens: never touched
        w.vault.set("svc.private-store-other-ns.admin-token", "other").unwrap();
        w.vault.set("store-token-theirs", "theirs").unwrap();
        w.vault.set("unrelated", "x").unwrap();
        let mine = w.store.add_source(&Actor::system(), "Mine", SourceKind::Url, "http://10.0.0.5:30300/catalog.json").unwrap();
        let theirs = w.store.add_source(&Actor::system(), "Theirs", SourceKind::Url, "http://10.9.9.9:30300/catalog.json").unwrap();
        w.vault.set(&format!("store-token-{mine}"), "mine").unwrap();
        w.store.set_publish_target(&Actor::system(), &mine).unwrap();
        (w, mine, theirs)
    };
    // delete everything
    let (w, mine, theirs) = mk("fin-all");
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert_eq!(rep.source.as_ref().map(|s| s.0.clone()), Some(mine.clone()));
    let done = finish_local(&w.pl, &w.store, &Actor::system(), &rep, &Action::Remove(RemoveChoice { keep_data: false, remove_registry: false }), None, &mut |_| {}).unwrap();
    assert!(done.iter().any(|d| d.contains("removed the STORE catalog entry Mine")), "{done:?}");
    for gone in [format!("svc.{INSTANCE}.admin-token"), format!("svc.{INSTANCE}.catalog-seed"), format!("svc.{INSTANCE}.publisher-bob"), format!("store-token-{mine}")] {
        assert!(!w.vault.has(&gone), "{gone} should be deleted");
    }
    for stays in ["svc.private-store-other-ns.admin-token", "store-token-theirs", "unrelated"] {
        assert!(w.vault.has(stays), "{stays} belongs to something else");
    }
    let cfg = w.store.config();
    assert_eq!(cfg.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec![theirs.clone()]);
    assert!(cfg.publish_target.is_empty());
    assert!(installed::load(&w.dir, "alice").is_empty());
    // keep my data: the record goes, the vault entries and the catalog entry stay (so Repair re-attaches the store)
    let (w, mine, _) = mk("fin-keep");
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    let done = finish_local(&w.pl, &w.store, &Actor::system(), &rep, &Action::Remove(RemoveChoice::default()), None, &mut |_| {}).unwrap();
    assert!(done.iter().any(|d| d.contains("kept the vault entries")), "{done:?}");
    assert!(w.vault.has(&format!("svc.{INSTANCE}.admin-token")) && w.vault.has(&format!("store-token-{mine}")));
    assert_eq!(w.store.config().sources.len(), 2);
    assert!(installed::load(&w.dir, "alice").is_empty());
    // reset wipes like delete-everything (the key is made again)
    let (w, _, theirs) = mk("fin-reset");
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    finish_local(&w.pl, &w.store, &Actor::system(), &rep, &Action::Reset, None, &mut |_| {}).unwrap();
    assert!(!w.vault.has(&format!("svc.{INSTANCE}.admin-token")) && w.vault.has("store-token-theirs"));
    assert_eq!(w.store.config().sources.len(), 1);
    assert_eq!(w.store.config().sources[0].id, theirs);
    // a second run changes nothing and does not fail
    finish_local(&w.pl, &w.store, &Actor::system(), &rep, &Action::Reset, None, &mut |_| {}).unwrap();
    assert_eq!(w.store.config().sources.len(), 1);
}

#[test]
fn the_registry_is_deleted_only_when_asked() {
    use crate::localreg::tests::Cluster;
    use crate::localreg::{Opts, Target, Wait};
    let reg_http = crate::ocipush::tests::FakeRegistry::default();
    let target = Target { name: "k3s".into(), kubeconfig: "/data/k3s-dev/kubeconfig".into(), context: String::new(), node_host: "localhost".into() };
    for (ask, expect) in [(false, false), (true, true)] {
        let w = world(&format!("reg{ask}"), &Sc::full());
        with_record(&w);
        let c = Cluster::healthy();
        let rt: Arc<dyn Runner> = c.clone();
        crate::localreg::create(&rt, &reg_http, &target, &Opts { size_gi: 10, node_port: 0, wait: Wait::NONE, work_dir: w.dir.clone() }, &mut |_| {}).unwrap();
        c.calls.lock().unwrap().clear();
        let rec = record(&w);
        let (store, cluster) = (w.store, w.cluster);
        let mut pl = w.pl;
        pl.run = rt;
        let rep = Report { state: StoreState::Healthy, cluster, namespace: NS.into(), instance: INSTANCE.into(), facts: Facts { namespace: true, managed: true, ..Default::default() }, record: Some(rec), elsewhere: None, source: None };
        let done = finish_local(&pl, &store, &Actor::system(), &rep, &Action::Remove(RemoveChoice { keep_data: true, remove_registry: ask }), Some((&target, &reg_http)), &mut |_| {}).unwrap();
        assert_eq!(c.did("delete namespace iyakki-registry"), expect, "{:?}", c.lines());
        assert_eq!(done.iter().any(|d| d.contains("image registry")), expect);
    }
}

// ------------------------------------------------------------------------------------------------ repair

/// The answers `platform::plan` needs from a healthy cluster, after the scenario's own.
fn plan_answers(r: &FakeRunner) {
    r.answer("config current-context", "default");
    r.answer("config view --minify", "https://127.0.0.1:6443");
    r.answer("version -o json", r#"{"serverVersion":{"gitVersion":"v1.30.0+k3s1"}}"#);
    r.answer("auth can-i", "Resources  Non-Resource URLs  Resource Names  Verbs\npods  []  []  [get list]\n");
    r.answer("get nodes -o json", r#"{"items":[{"metadata":{"name":"n1"},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64"},"allocatable":{"pods":"110"}}}]}"#);
    r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#);
    r.answer("get services -A", "default web NodePort 30300\n");
    r.answer("get deployments,statefulsets,daemonsets -A", "kube-system coredns rancher/mirrored-coredns-coredns:1.10\n");
    r.answer("get pods -A", "pod/a\n");
    r.answer("get services -n", "");
    r.answer("get service registry", r#"{"spec":{"ports":[{"nodePort":30500}]}}"#);
    r.answer("create secret generic", "secret/private-store created");
    r.answer("label secret", "secret/private-store labeled");
    r.answer("apply -f", "created");
    r.answer("rollout status", "deployment \"private-store\" successfully rolled out");
    r.answer("rollout restart", "restarted");
    r.answer("config", "default");
}

fn profile_of(w: &W) -> crate::devcluster::ClusterProfile {
    let l = platform::list_profiles(&w.pl);
    assert_eq!(l.len(), 1, "{l:?}");
    assert!(l[0].refusal.is_none(), "{:?}", l[0].refusal);
    l[0].profile.clone()
}

fn applied_manifest(w: &W) -> String {
    let line = w.run.lines().into_iter().find(|l| l.contains(" apply -f ")).expect("something was applied");
    let path = line.split(" apply -f ").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn repair_puts_back_only_what_is_missing_and_keeps_the_data_and_the_secret() {
    // the server and the volume and the Secret exist; the address (Service) is missing
    let sc = Sc { svc: false, ..Sc::full() };
    let w = world("rep-svc", &sc);
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_record(&w);
    with_vault(&w);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(matches!(rep.state, StoreState::Partial { .. }), "{:?}", rep.state);
    let p = preview(&rep, &Action::Repair);
    assert!(p.contains("create the address (Service)") && !p.contains("create the data volume") && !p.contains("create the server"), "{p}");
    let reg = crate::ocipush::tests::FakeRegistry::default();
    let mut said = Vec::new();
    let done = repair(&w.pl, &rep, &profile_of(&w), &record(&w).options, &reg, &mut |l| said.push(l.to_string())).unwrap_or_else(|e| panic!("{e}"));
    let m = applied_manifest(&w);
    assert!(m.contains("kind: Service") && !m.contains("kind: Deployment") && !m.contains("kind: PersistentVolumeClaim"), "only the missing piece is applied:\n{m}");
    let l = w.run.lines();
    assert!(l.iter().all(|x| !x.contains("create secret") && !x.contains("delete ") && !x.contains("rollout restart")), "{l:#?}");
    assert!(reg.manifests.lock().unwrap().is_empty(), "no image is pushed when the server exists");
    assert!(done.iter().any(|d| d.contains("kept the existing Secret")), "{done:?}");
    assert!(installed::find(&w.dir, "alice", INSTANCE).is_some());
    assert!(w.vault.has(&format!("svc.{INSTANCE}.admin-token")));
    // the volume and the address are both missing
    let w = world("rep-pvc", &Sc { svc: false, pvc: false, ..Sc::full() });
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_record(&w);
    with_vault(&w);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    repair(&w.pl, &rep, &profile_of(&w), &record(&w).options, &reg, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let m = applied_manifest(&w);
    assert!(m.contains("kind: Service") && m.contains("kind: PersistentVolumeClaim") && !m.contains("kind: Deployment"), "{m}");
}

#[test]
fn repair_of_a_lost_token_reads_it_back_from_the_secret_and_never_prints_it() {
    let w = world("rep-token", &Sc::full());
    let b64 = |s: &str| {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let b = s.as_bytes();
        let mut o = String::new();
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() { o.push(T[(n >> (18 - 6 * i) & 63) as usize] as char) } else { o.push('=') }
            }
        }
        o
    };
    w.run.answer("get secret private-store -n iyakki-private-store -o json", &format!(r#"{{"data":{{"admin-token":"{}","catalog-seed":"{}"}}}}"#, b64(TOKEN), b64("seedvalue")));
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_record(&w);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(matches!(rep.state, StoreState::Broken { why: Why::TokenLost, .. }));
    assert!(preview(&rep, &Action::Repair).contains("read the admin token back"));
    let reg = crate::ocipush::tests::FakeRegistry::default();
    let done = repair(&w.pl, &rep, &profile_of(&w), &record(&w).options, &reg, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(&*w.vault.get(&format!("svc.{INSTANCE}.admin-token")).unwrap(), TOKEN);
    assert!(done.iter().any(|d| d.contains("put 2 vault entries back")), "{done:?}");
    let all = format!("{done:?} {:?}", w.run.calls().iter().map(|c| format!("{:?}{:?}", c.argv, c.env)).collect::<Vec<_>>());
    assert!(!all.contains(TOKEN), "the token must not appear in any command or message");
    // nothing was regenerated or re-created
    assert!(w.run.lines().iter().all(|l| !l.contains("create secret")));
    assert_eq!(state(&w), StoreState::Healthy);
}

#[test]
fn repair_restarts_a_crashing_server_and_adopts_a_store_that_has_no_record() {
    let w = world("rep-crash", &Sc { bad_pod: Some("CrashLoopBackOff"), dep: Some((1, 0)), ..Sc::full() });
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_record(&w);
    with_vault(&w);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    let reg = crate::ocipush::tests::FakeRegistry::default();
    repair(&w.pl, &rep, &profile_of(&w), &record(&w).options, &reg, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let l = w.run.lines();
    assert!(l.iter().any(|x| x.contains("rollout restart deployment/private-store -n iyakki-private-store")), "{l:#?}");
    assert!(l.iter().all(|x| !x.contains(" apply -f ")), "nothing was missing, so nothing is applied: {l:#?}");
    // no record: it is written again, the cluster objects are not touched
    let w = world("rep-norec", &Sc::full());
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_vault(&w);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(matches!(rep.state, StoreState::Broken { why: Why::NoRecord, .. }));
    repair(&w.pl, &rep, &profile_of(&w), &Options::default(), &reg, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert!(installed::find(&w.dir, "alice", INSTANCE).is_some_and(|r| r.namespace == NS));
    assert!(w.run.lines().iter().all(|x| !x.contains(" apply -f ") && !x.contains("delete ")));
    // a state that cannot be repaired says so
    let w = world("rep-none", &Sc { ns: Some(TERMINATING), ..Default::default() });
    plan_answers(&w.run);
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(repair(&w.pl, &rep, &profile_of(&w), &Options::default(), &reg, &mut |_| {}).unwrap_err().contains("nothing can be repaired"));
}

#[test]
fn the_filter_keeps_only_the_named_kinds_of_a_manifest() {
    let m = "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: a\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: b\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: c\n";
    let s = only_kinds(m, &["Service"]);
    assert!(s.contains("kind: Service") && !s.contains("Deployment") && !s.contains("Persistent"));
    let two = only_kinds(m, &["Deployment", "Service"]);
    assert!(two.contains("---\n") && two.contains("Deployment") && two.contains("Service"));
    assert_eq!(only_kinds(m, &[]), "");
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

#[test]
fn repair_of_a_missing_server_pushes_the_image_and_applies_only_the_deployment() {
    // the volume, the Secret and the address survived; the server (and so the image's use) did not
    let w = world("rep-dep", &Sc { dep: None, ..Sc::full() });
    w.run.answer("get secret private-store -n iyakki-private-store -o name", "secret/private-store");
    plan_answers(&w.run);
    with_record(&w);
    with_vault(&w);
    std::fs::create_dir_all(w.dir.join("store-server")).unwrap();
    std::fs::write(w.dir.join("store-server/iyakki-store-server-x86_64"), static_elf()).unwrap();
    let rep = private_store_state(&w.pl, Some(&w.store), &w.cluster, NS);
    assert!(matches!(&rep.state, StoreState::Partial { missing, .. } if missing.len() == 1 && missing[0].contains("Deployment")), "{:?}", rep.state);
    let reg = crate::ocipush::tests::FakeRegistry::default();
    repair(&w.pl, &rep, &profile_of(&w), &record(&w).options, &reg, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let m = applied_manifest(&w);
    assert!(m.contains("kind: Deployment") && !m.contains("kind: Service") && !m.contains("PersistentVolumeClaim"), "{m}");
    assert!(!reg.manifests.lock().unwrap().is_empty(), "the image is pushed again for the new server");
    assert!(w.run.lines().iter().all(|x| !x.contains("create secret") && !x.contains("delete ")));
}
