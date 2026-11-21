//! The platform calls that report progress, over a real loopback socket against the reference server (F-SPLIT-03, D-SPLIT-03-APPS): the call starts
//! a job on the core, the UI's closure gets every progress line in order, the method's own answer comes at the end, and a secret the deploy made
//! to show once never crosses. Real transport, a fake platform, nothing waits for time.

use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::PlatformApi;
use iyakki_core_api::contract::RateLimit;
use iyakki_shared::devcluster::ClusterProfile;
use iyakki_shared::platform_types::{Applied, ClusterRef, Options, Plan, Reveal, Wait};
use iyakki_shared::privatemanage::{Action, JobArgs, Report, StoreState};
use iyakki_shared::wsexec::{Out, Runner, Sink};
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc};
use serde_json::json;

struct NoRunner;
impl Runner for NoRunner {
    fn run(&self, _: &Path, _: &[String], _: &[(String, String)], _: Duration, _: usize, _: Option<Sink>) -> Out {
        panic!("nothing runs a program here")
    }
    fn has(&self, _: &str) -> bool {
        false
    }
}

/// A platform that says what it was asked and does nothing else. `seen` keeps the arguments the core got.
#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<String>>,
}
impl PlatformApi for Fake {
    fn runner(&self) -> Arc<dyn Runner> {
        Arc::new(NoRunner)
    }
    fn private_store_state(&self, services: Option<Arc<dyn std::any::Any + Send + Sync>>, cluster: &ClusterRef, namespace: &str) -> Report {
        let who = services.and_then(|s| s.downcast::<String>().ok()).map(|s| (*s).clone());
        self.seen.lock().unwrap().push(format!("private_store_state {namespace} services={who:?}"));
        Report { state: StoreState::Broken { why: iyakki_shared::privatemanage::Why::NoKubectl, reason: "read by the core".into() }, cluster: cluster.clone(), namespace: namespace.to_string(), instance: String::new(), facts: Default::default(), record: None, elsewhere: None, source: None }
    }
    fn private_finish_local(&self, services: Arc<dyn std::any::Any + Send + Sync>, _: &Report, action: &Action, registry: Option<&ClusterProfile>, say: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        let who = services.downcast::<String>().map(|s| (*s).clone()).unwrap_or_default();
        self.seen.lock().unwrap().push(format!("private_finish_local {action:?} registry={} services={who}", registry.is_some()));
        say("removing the vault entry");
        say("removing the catalog entry");
        Ok(vec!["done".into()])
    }
    fn private_start_job(&self, services: Arc<dyn std::any::Any + Send + Sync>, launch: Option<Arc<dyn std::any::Any + Send + Sync>>, args: &JobArgs) -> Result<iyakki_shared::jobs::Job, String> {
        let who = services.downcast::<String>().map(|s| (*s).clone()).unwrap_or_default();
        self.seen.lock().unwrap().push(format!("private_start_job ns={} launch={} services={who}", args.namespace, launch.is_some()));
        Err("no job runner in this test".into())
    }
    fn apply(&self, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Applied, String> {
        self.seen.lock().unwrap().push(format!("apply {} approved={approved}", plan.service));
        if !approved {
            return Err("the plan was not approved".into());
        }
        for l in ["pulling the image", "creating the namespace", "waiting for the pod"] {
            progress(l);
        }
        Ok(Applied { done: vec!["deployed".into()], urls: Vec::new(), reveal: vec![Reveal { label: "Admin password".into(), vault: "uvault:ann".into(), value: zeroize::Zeroizing::new("s3cret-admin-pass".into()) }], ready: true, how_to_use: "open the address".into(), instance: "gitea-1".into() })
    }
    fn apply_with(&self, _: &Plan, _: bool, _: &mut dyn FnMut(&str), _: &dyn iyakki_shared::ocipush::RegistryHttp) -> Result<Applied, String> {
        panic!("the core's own apply runs: the UI's registry object never arrives")
    }
    fn plan_waiting(&self, service: &str, _: &ClusterProfile, given: &Options, wait: Wait, say: &mut dyn FnMut(&str)) -> Result<Plan, String> {
        self.seen.lock().unwrap().push(format!("plan_waiting {service} tries={} pause_ms={} opts={}", wait.tries, wait.pause.as_millis(), given.0.len()));
        say("asking the registry");
        Err("the registry does not answer".into())
    }
    fn private_repair(&self, _: &Report, _: &ClusterProfile, opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        progress("repairing the namespace");
        progress("repairing the secret");
        Ok(vec![format!("repaired with {} options", opts.0.len())])
    }
}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
fn machine(host: Host) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(Arc::new(host)), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread }
}
impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn profile() -> ClusterProfile {
    serde_json::from_value(json!({"name": "k3s", "kubeconfig": "/k", "context": "c", "server": "https://127.0.0.1:6443", "source": "Project", "exists": true, "readable": true, "tested": true, "reachable": true, "k3s": true, "local": true, "version": "v1", "can_i": "", "problem": "", "fix": ""})).unwrap()
}
fn plan() -> Plan {
    serde_json::from_value(json!({
        "service": "gitea", "title": "Gitea", "version": "1", "instance": "gitea-1",
        "cluster": {"name": "k3s", "kubeconfig": "/k", "context": "c", "server": "https://127.0.0.1:6443", "source": "Project"},
        "profile": serde_json::to_value(profile()).unwrap(), "namespace": "git", "create_namespace": true, "opts": {},
        "images": [], "image_refs": [], "mirror": null, "needs_registry": false, "manifest": "", "manifest_path": "/m.yaml", "secrets": [], "urls": [], "exposes": [],
        "warnings": [], "blockers": [], "notes": [], "existing": [], "storage_class": null, "network_policy": null, "how_to_use": "", "vault_prefix": "p", "notes_lint": [], "target_arch": "amd64"
    })).unwrap()
}

fn remote(m: &Machine) -> Arc<dyn PlatformApi> {
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap());
    match rpc.root("PlatformApi") {
        Some(RemoteAny::PlatformApi(p)) => p,
        _ => panic!("the trait is generated"),
    }
}

#[test]
fn a_deploy_runs_on_the_core_the_uis_closure_gets_every_line_and_the_secret_to_show_once_never_crosses() {
    let fake = Arc::new(Fake::default());
    let m = machine(Host::new().with_root(Obj::PlatformApi(fake.clone())));
    let platform = remote(&m);
    let mut lines = Vec::new();
    let done = platform.apply(&plan(), true, &mut |l| lines.push(l.to_string())).unwrap();
    assert_eq!(lines, ["pulling the image", "creating the namespace", "waiting for the pod"], "every line, in the order the core said it");
    assert_eq!((done.done.as_slice(), done.ready, done.instance.as_str(), done.how_to_use.as_str()), (&["deployed".to_string()][..], true, "gitea-1", "open the address"));
    // the generated password stays in the core (and in the vault the line names): the UI is told its name and where it was kept
    assert_eq!((done.reveal.len(), done.reveal[0].label.as_str(), done.reveal[0].vault.as_str(), done.reveal[0].value.as_str()), (1, "Admin password", "uvault:ann", ""));
    // a refusal of the core's own method arrives as it is, after no line
    let mut none = Vec::new();
    assert_eq!(platform.apply(&plan(), false, &mut |l| none.push(l.to_string())).unwrap_err(), "the plan was not approved");
    assert!(none.is_empty());
    // the UI's registry object cannot ride along: apply_with runs the core's own apply (the fake's apply_with panics if it were called)
    let mut via = Vec::new();
    struct Http;
    impl iyakki_shared::ocipush::RegistryHttp for Http {
        fn request(&self, _: &str, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_shared::ocipush::Reply, String> {
            panic!("the UI's transport is never used")
        }
    }
    assert!(platform.apply_with(&plan(), true, &mut |l| via.push(l.to_string()), &Http).is_ok());
    assert_eq!(via.len(), 3);
    assert_eq!(fake.seen.lock().unwrap().as_slice(), ["apply gitea approved=true", "apply gitea approved=false", "apply gitea approved=true"]);
}

#[test]
fn waiting_for_a_plan_and_repairing_the_private_store_report_progress_and_take_their_arguments_as_data() {
    let fake = Arc::new(Fake::default());
    let m = machine(Host::new().with_root(Obj::PlatformApi(fake.clone())));
    let platform = remote(&m);
    let mut said = Vec::new();
    let r = platform.plan_waiting("gitea", &profile(), &Options([("size".to_string(), "10".to_string())].into_iter().collect()), Wait { tries: 12, pause: Duration::from_secs(5) }, &mut |l| said.push(l.to_string()));
    assert_eq!((r.err().as_deref(), said.as_slice()), (Some("the registry does not answer"), &["asking the registry".to_string()][..]));
    assert_eq!(fake.seen.lock().unwrap().as_slice(), ["plan_waiting gitea tries=12 pause_ms=5000 opts=1"], "the wait crosses as tries and milliseconds");
    let report: Report = serde_json::from_value(json!({"state": "NotInstalled", "cluster": {"name": "k3s", "kubeconfig": "/k", "context": "c", "server": "s", "source": "Project"}, "namespace": "n", "instance": "i", "facts": serde_json::to_value(iyakki_shared::privatemanage::Facts::default()).unwrap(), "record": null, "elsewhere": null, "source": null})).unwrap_or_else(|e| panic!("{e}"));
    let mut repair = Vec::new();
    let out = platform.private_repair(&report, &profile(), &Options::default(), &mut |l| repair.push(l.to_string()));
    assert_eq!((out, repair.as_slice()), (Ok(vec!["repaired with 0 options".to_string()]), &["repairing the namespace".to_string(), "repairing the secret".to_string()][..]));
}

#[test]
fn a_lost_core_ends_the_job_with_the_reason_and_the_closure_hears_nothing() {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let Some(RemoteAny::PlatformApi(platform)) = rpc.root("PlatformApi") else { panic!("the trait is generated") };
    let mut heard = 0;
    assert!(platform.apply(&plan(), true, &mut |_| heard += 1).is_err());
    assert!(platform.plan_waiting("gitea", &profile(), &Options::default(), Wait { tries: 1, pause: Duration::ZERO }, &mut |_| heard += 1).is_err());
    assert_eq!(heard, 0);
}

fn cluster_ref() -> ClusterRef {
    serde_json::from_value(json!({"name": "k3s", "kubeconfig": "/k", "context": "c", "server": "s", "source": "Project"})).unwrap()
}

#[test]
fn the_three_calls_that_took_the_services_run_on_the_core_with_the_cores_own_services_and_never_the_uis_launch() {
    let fake = Arc::new(Fake::default());
    let m = machine(Host::new().with_root(Obj::PlatformApi(fake.clone())).with_services(Arc::new("core-services".to_string())));
    let platform = remote(&m);
    let ui_side: Arc<dyn std::any::Any + Send + Sync> = Arc::new("the UI's own object".to_string());
    // the state: the UI only says it had services; the core reads with its own
    let r = platform.private_store_state(Some(ui_side.clone()), &cluster_ref(), "store");
    assert!(matches!(r.state, StoreState::Broken { ref reason, .. } if reason == "read by the core"), "{:?}", r.state);
    let none = platform.private_store_state(None, &cluster_ref(), "store2");
    assert_eq!(none.namespace, "store2");
    // the device side: each line reaches the closure in order
    let mut said = Vec::new();
    let out = platform.private_finish_local(ui_side.clone(), &r, &Action::Reset, None, &mut |l| said.push(l.to_string()));
    assert_eq!((out, said.as_slice()), (Ok(vec!["done".to_string()]), &["removing the vault entry".to_string(), "removing the catalog entry".to_string()][..]));
    // the job: the UI's program to launch never arrives (the core runs its own), the answer is the method's
    let args: JobArgs = serde_json::from_value(json!({"namespace": "store", "kubeconfig": "/k", "context": "c", "reset": true, "delete_data": false, "wait_secs": 5, "force_finalizers": false})).unwrap_or_else(|e| panic!("{e}"));
    let err = platform.private_start_job(ui_side.clone(), Some(Arc::new(1u8)), &args).unwrap_err();
    assert_eq!(err, "no job runner in this test");
    assert_eq!(
        fake.seen.lock().unwrap().as_slice(),
        ["private_store_state store services=Some(\"core-services\")", "private_store_state store2 services=None", "private_finish_local Reset registry=false services=core-services", "private_start_job ns=store launch=false services=core-services"]
    );
}

#[test]
fn a_host_without_services_refuses_by_name_and_the_state_is_shown_as_unreachable() {
    let m = machine(Host::new().with_root(Obj::PlatformApi(Arc::new(Fake::default()))));
    let platform = remote(&m);
    let ui_side: Arc<dyn std::any::Any + Send + Sync> = Arc::new(0u8);
    let r = platform.private_store_state(Some(ui_side.clone()), &cluster_ref(), "store");
    assert!(matches!(r.state, StoreState::Broken { ref reason, .. } if reason.contains("no services of its own")), "{:?}", r.state);
    assert!(platform.private_finish_local(ui_side.clone(), &r, &Action::Reset, None, &mut |_| {}).unwrap_err().contains("no services of its own"));
    let args: JobArgs = serde_json::from_value(json!({"namespace": "n", "kubeconfig": "/k", "context": "c", "reset": false, "delete_data": false, "wait_secs": 5, "force_finalizers": false})).unwrap();
    assert!(platform.private_start_job(ui_side, None, &args).unwrap_err().contains("no services of its own"));
}
