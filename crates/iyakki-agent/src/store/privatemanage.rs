//! Looking after the private store that IYAKKI deployed into a cluster: what state it is in (`private_store_state`), and the three ways
//! out of a half-finished or broken install: **Repair** (put back only what is missing, keep the data), **Reset and reinstall** (delete the
//! store's namespace, wait for it to be gone, deploy again) and **Remove** (delete what the store made; keep or delete the data volume).
//! See docs/STORE.md, section 13, "Removing or repairing the private store".
//!
//! * Detection only reads (`kubectl get`), and only in the cluster and namespace it is given: no other cluster is touched.
//! * The cluster side of Reset and Remove runs as a detached job (`iyakki store-manage ...`, [`run_job`]) so it survives the window and
//!   shows beside the clock; the vault, the STORE catalog entry and the installed-services record are cleaned by [`finish_local`] in the
//!   signed-in session afterwards (a detached program cannot open the person's vault).
//! * Every destructive step needs the namespace typed back ([`prepare`]), is idempotent, and works when only the namespace exists.
//! * kubectl is `kubectl` or, when the machine has none, `k3s kubectl` of the managed cluster (`wsexec` does the rewrite).

pub use iyakki_shared::privatemanage::{Action, DEFAULT_NAMESPACE, Facts, JOB_LABEL_REMOVE, JOB_LABEL_RESET, JobArgs, Offers, RemoveChoice, Report, StoreState, Why, classify, only_kinds, preview, store_vault_entries};
pub use iyakki_shared::privatemanage::{delete_list, repair_steps, vault_names};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::install::Actor;
use super::privatestore::ID;
use super::Store;
use crate::devcluster::Kc;
use crate::platform::{self, installed, ClusterRef, Installed, Options, Platform, LABEL_MANAGED, LABEL_SERVICE};
use crate::wsexec::Runner;

const DEPLOYMENT: &str = "private-store";
const SERVICE: &str = "private-store";
const PVC: &str = "private-store-data";
const SECRET: &str = "private-store";

// ------------------------------------------------------------------------------------------------ the state







fn not_found(t: &str) -> bool {
    t.contains("NotFound") || t.contains("not found")
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

fn cluster_key(c: &ClusterRef) -> String {
    format!("{}#{}", c.kubeconfig, c.context)
}

fn same_cluster(a: &ClusterRef, b: &ClusterRef) -> bool {
    a.kubeconfig == b.kubeconfig && a.context == b.context
}

/// The instance id the store has (or would get) in `cluster`/`ns`.
pub fn instance_of(pl: &Platform, cluster: &ClusterRef, ns: &str) -> (String, Option<Installed>, Option<String>) {
    let all = installed::load(&pl.dir, &pl.user);
    let stores: Vec<&Installed> = all.iter().filter(|i| i.service == ID && !i.connected).collect();
    let here = stores.iter().find(|i| i.namespace == ns && i.cluster.as_ref().is_some_and(|c| same_cluster(c, cluster))).map(|i| (*i).clone());
    let elsewhere = stores.iter().find(|i| !(i.namespace == ns && i.cluster.as_ref().is_some_and(|c| same_cluster(c, cluster)))).map(|i| format!("this device also has a private store record `{}` for {}; it is not touched here", i.id, i.cluster.as_ref().map(ClusterRef::line).unwrap_or_default()));
    let id = match &here {
        Some(r) => r.id.clone(),
        None => installed::instance_id(ID, ns, &cluster_key(cluster), &all),
    };
    (id, here, elsewhere)
}


const BAD_WAITING: [&str; 7] = ["CrashLoopBackOff", "ImagePullBackOff", "ErrImagePull", "CreateContainerConfigError", "RunContainerError", "InvalidImageName", "CreateContainerError"];

/// Read the facts of `ns` through `kc` (read-only). `Err` is `(why, plain words)` when the cluster cannot be read at all.
fn gather(pl: &Platform, kc: &Kc, ns: &str, instance: &str) -> Result<Facts, (Why, String)> {
    let mut f = Facts::default();
    let (tok, seed) = vault_names(instance);
    f.token = pl.vault.has(&tok);
    f.seed = pl.vault.has(&seed);
    let n = kc.read(&["get", "namespace", ns, "-o", "json"]);
    if !n.ok() {
        let t = n.combined();
        if !not_found(&t) {
            return Err((Why::ClusterDown, format!("The cluster does not answer: {}\nNothing was changed. Start the cluster (the managed k3s: Settings, Dev cluster, Start) or check that the chosen cluster is up and reachable from here, then look again.", crate::localreg::explain_kubectl(&t))));
        }
        return Ok(f);
    }
    f.namespace = true;
    let j: Value = serde_json::from_str(&n.stdout).unwrap_or(Value::Null);
    f.terminating = j["status"]["phase"].as_str() == Some("Terminating") || !j["metadata"]["deletionTimestamp"].is_null();
    f.managed = j["metadata"]["labels"][LABEL_MANAGED].as_str() == Some("iyakki");
    f.made_for_store = j["metadata"]["labels"][LABEL_SERVICE].as_str() == Some(ID);
    if f.terminating {
        return Ok(f);
    }
    let d = kc.read(&["get", "deployments,pods", "-n", ns, "-o", "json"]);
    if !d.ok() {
        return Err((Why::ClusterDown, format!("The cluster stopped answering while reading {ns}: {}", crate::localreg::explain_kubectl(&d.combined()))));
    }
    let j: Value = serde_json::from_str(&d.stdout).unwrap_or(Value::Null);
    for i in j["items"].as_array().cloned().unwrap_or_default() {
        let name = i["metadata"]["name"].as_str().unwrap_or("");
        match i["kind"].as_str() {
            Some("Deployment") if name == DEPLOYMENT => {
                f.deployment = true;
                f.desired = i["spec"]["replicas"].as_u64().unwrap_or(1) as u32;
                f.ready = i["status"]["readyReplicas"].as_u64().unwrap_or(0) as u32;
            }
            Some("Pod") if name.starts_with(DEPLOYMENT) => {
                for c in i["status"]["containerStatuses"].as_array().cloned().unwrap_or_default() {
                    if let Some(r) = c["state"]["waiting"]["reason"].as_str() {
                        if BAD_WAITING.contains(&r) && f.bad_pod.is_empty() {
                            f.bad_pod = r.to_string();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let o = kc.read(&["get", "services,persistentvolumeclaims,secrets", "-n", ns, "-o", "name"]);
    if o.ok() {
        for l in o.stdout.lines() {
            let l = l.trim();
            f.service |= l.ends_with(&format!("/{SERVICE}")) && l.starts_with("service");
            f.pvc |= l.ends_with(&format!("/{PVC}"));
            f.secret |= l.ends_with(&format!("/{SECRET}")) && l.starts_with("secret");
        }
    }
    if f.deployment {
        let e = kc.read(&["get", "endpoints", SERVICE, "-n", ns, "-o", "json"]);
        if e.ok() {
            let j: Value = serde_json::from_str(&e.stdout).unwrap_or(Value::Null);
            f.endpoints = j["subsets"].as_array().is_some_and(|s| s.iter().any(|x| x["addresses"].as_array().is_some_and(|a| !a.is_empty())));
        }
    }
    Ok(f)
}


/// What state is the private store in, in `namespace` of `cluster`? Reads only that cluster; works for the managed k3s (kubectl or
/// `k3s kubectl`) and any chosen context.
pub fn private_store_state(pl: &Platform, store: Option<&Store>, cluster: &ClusterRef, namespace: &str) -> Report {
    let (instance, record, elsewhere) = instance_of(pl, cluster, namespace);
    let source = record.as_ref().and_then(|r| store.and_then(|s| source_of(s, r)));
    let mut rep = Report { state: StoreState::NotInstalled, cluster: cluster.clone(), namespace: namespace.to_string(), instance: instance.clone(), facts: Facts::default(), record: record.clone(), elsewhere, source: source.clone() };
    if !pl.run.has("kubectl") {
        rep.state = StoreState::Broken { why: Why::NoKubectl, reason: "There is no kubectl on this machine, and the managed k3s cluster's own kubectl was not found either. Install the dev cluster (Store \u{2192} Setup, k3s), or install kubectl for another cluster, then look again. Nothing was changed.".into() };
        return rep;
    }
    let kc = platform::kc_of(pl, cluster);
    match gather(pl, &kc, namespace, &instance) {
        Ok(f) => {
            rep.state = classify(&f, record.is_some(), source.is_some());
            rep.facts = f;
        }
        Err((why, reason)) => rep.state = StoreState::Broken { why, reason },
    }
    rep
}

/// The STORE catalog entry added for this record's address.
fn source_of(store: &Store, rec: &Installed) -> Option<(String, String)> {
    let url = rec.urls.iter().find(|u| u.kind == "catalog").map(|u| u.url.clone())?;
    store.config().sources.iter().find(|s| s.location == url).map(|s| (s.id.clone(), s.name.clone()))
}

// ------------------------------------------------------------------------------------------------ actions and their previews









// ------------------------------------------------------------------------------------------------ repair (in this program)

/// Put back only what is missing. `profile` is the chosen cluster's profile (checked like a deploy); `opts` are the options of the
/// record, or the defaults. Idempotent: a second run with nothing missing changes nothing.
pub fn repair(pl: &Platform, rep: &Report, profile: &crate::devcluster::ClusterProfile, opts: &Options, http: &dyn crate::ocipush::RegistryHttp, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    if !rep.offers().repair {
        return Err(format!("nothing can be repaired in this state ({}). Nothing was changed.", rep.headline().to_lowercase()));
    }
    let def = platform::spec(ID).ok_or("the private store service is not registered")?;
    let mut given = opts.clone();
    given.set("namespace", &rep.namespace);
    let mut plan = platform::plan(pl, &*def, profile, &given)?;
    if !plan.blockers.is_empty() {
        return Err(format!("not repaired: {}", plan.blockers.join("; ")));
    }
    let kc = platform::kc_of(pl, &rep.cluster);
    let f = &rep.facts;
    let mut done = Vec::new();
    let full = matches!(&rep.state, StoreState::Broken { why: Why::CatalogOnly, .. });
    // 1. a token that only the cluster still has goes back into the vault (the value is never printed)
    if f.secret && (!f.token || !f.seed) && !full {
        progress("reading the admin token back from the cluster's Secret");
        let n = restore_from_secret(pl, &kc, rep)?;
        done.push(format!("put {n} vault entr{} back from the cluster's Secret", if n == 1 { "y" } else { "ies" }));
    }
    // 2. only the missing objects are applied
    if !full {
        let mut want: Vec<&str> = Vec::new();
        for (ok, kind) in [(f.pvc, "PersistentVolumeClaim"), (f.deployment, "Deployment"), (f.service, "Service")] {
            if !ok {
                want.push(kind);
            }
        }
        plan.manifest = only_kinds(&plan.manifest, &want);
        if f.deployment {
            // the image is in use already: nothing is built or pushed again
            plan.mirror = None;
        }
    }
    // 3. a server that is deployed but not answering is restarted
    if !full && (!f.bad_pod.is_empty() || (f.deployment && !f.endpoints)) {
        progress("restarting the server");
        let r = kc.kubectl(&["rollout", "restart", &format!("deployment/{DEPLOYMENT}"), "-n", &rep.namespace], 20, 40);
        if !r.ok() {
            return Err(format!("could not restart the server: {}", first_line(&r.combined())));
        }
        done.push("restarted the server".into());
    }
    let a = platform::apply_with(pl, &plan, true, progress, http)?;
    done.extend(a.done.clone());
    if !a.ready {
        return Err(format!("the pieces were put back but the server is not ready yet:\n- {}\nLook again in a minute; the cluster's events say why.", a.done.join("\n- ")));
    }
    pl.audit("store.repair", &format!("{}:{} ok", rep.namespace, rep.cluster.server));
    Ok(done)
}


fn restore_from_secret(pl: &Platform, kc: &Kc, rep: &Report) -> Result<usize, String> {
    let o = kc.kubectl(&["get", "secret", SECRET, "-n", &rep.namespace, "-o", "json"], 10, 20);
    if !o.ok() {
        return Err(format!("could not read the Secret {SECRET}: {}", first_line(&o.combined())));
    }
    let j: Value = serde_json::from_str(&o.stdout).map_err(|_| "the Secret could not be read".to_string())?;
    let (tok, seed) = vault_names(&rep.instance);
    let mut n = 0;
    for (key, name) in [("admin-token", tok), ("catalog-seed", seed)] {
        if pl.vault.has(&name) {
            continue;
        }
        let Some(b) = j["data"][key].as_str() else { continue };
        let bytes = crate::models::b64_decode(b).ok_or_else(|| format!("the Secret's {key} is not readable"))?;
        let v = zeroize::Zeroizing::new(String::from_utf8(bytes).map_err(|_| format!("the Secret's {key} is not text"))?);
        pl.vault.set(&name, v.trim())?;
        n += 1;
    }
    if n == 0 {
        return Err("the cluster's Secret does not hold the admin token either; use Reset and reinstall".into());
    }
    Ok(n)
}

// ------------------------------------------------------------------------------------------------ the detached job



/// `iyakki store-manage ...`: the cluster side of Reset and Remove, as the job runs it. Returns the exit code.
pub fn cli_main(args: &[String]) -> i32 {
    let a = match JobArgs::parse(args) {
        Ok(a) => a,
        Err(e) => {
            crate::qprintln!("error: {e}");
            return 2;
        }
    };
    let run: Arc<dyn Runner> = Arc::new(crate::wsexec::System);
    match run_job(&run, &a, &|d| std::thread::sleep(d), &mut |l| crate::qprintln!("{l}")) {
        Ok(()) => {
            crate::qprintln!("done");
            0
        }
        Err(e) => {
            crate::qprintln!("error: {e}");
            1
        }
    }
}

fn delete_args(kc: &Kc, args: &[&str], what: &str) -> Result<(), String> {
    let mut a: Vec<&str> = vec!["delete"];
    a.extend(args);
    a.extend(["--ignore-not-found", "--wait=true", "--timeout=120s"]);
    let o = kc.kubectl(&a, 130, 150);
    if o.ok() || not_found(&o.combined()) {
        Ok(())
    } else {
        Err(format!("deleting {what} failed: {}", crate::localreg::explain_kubectl(&o.combined())))
    }
}

/// The cluster side of Reset and Remove. Idempotent (a namespace that is gone is success), logs what it does through `say`, and touches
/// only the one namespace, only when IYAKKI manages it.
pub fn run_job(run: &Arc<dyn Runner>, a: &JobArgs, sleep: &dyn Fn(Duration), say: &mut dyn FnMut(&str)) -> Result<(), String> {
    if !run.has("kubectl") {
        return Err("there is no kubectl and no k3s kubectl on this machine; install the dev cluster first (Store \u{2192} Setup, k3s). Nothing was changed".into());
    }
    let kc = Kc::new(run.clone(), &a.kubeconfig, &a.context);
    let ns = a.namespace.as_str();
    if crate::k8stools::protected_namespace(ns) {
        return Err(format!("refused: `{ns}` is a protected namespace; nothing was changed"));
    }
    say(&format!("looking at namespace {ns}"));
    let n = kc.read(&["get", "namespace", ns, "-o", "json"]);
    if !n.ok() {
        let t = n.combined();
        if not_found(&t) {
            say(&format!("namespace {ns} is already gone; nothing to delete in the cluster"));
            return Ok(());
        }
        return Err(format!("the cluster does not answer, so nothing was changed: {}", crate::localreg::explain_kubectl(&t)));
    }
    let j: Value = serde_json::from_str(&n.stdout).unwrap_or(Value::Null);
    let terminating = j["status"]["phase"].as_str() == Some("Terminating");
    if j["metadata"]["labels"][LABEL_MANAGED].as_str() != Some("iyakki") {
        return Err(format!("refused: namespace `{ns}` is not one IYAKKI manages (it lacks {LABEL_MANAGED}=iyakki); nothing was changed"));
    }
    let made_for_store = j["metadata"]["labels"][LABEL_SERVICE].as_str() == Some(ID);
    if a.reset && !made_for_store && !terminating {
        return Err(format!("refused: namespace `{ns}` was not created by IYAKKI for the private store (it lacks {LABEL_SERVICE}={ID}), so it is never deleted as a whole. Use Remove to delete what the store made in it"));
    }
    if terminating {
        say("the namespace is already being deleted; waiting for it");
        return wait_gone(&kc, ns, a, sleep, say);
    }
    let sel = format!("{LABEL_SERVICE}={ID}");
    say("deleting the server, its address and its ingress");
    delete_args(&kc, &["deployments,statefulsets,services,ingresses,configmaps,networkpolicies,jobs", "-n", ns, "-l", &sel], "the server and its address")?;
    delete_args(&kc, &[&format!("deployment/{DEPLOYMENT}"), &format!("service/{SERVICE}"), "-n", ns], "the server and its address by name")?;
    if !a.delete_data {
        say("kept the data volume, the Secret and the namespace");
        return Ok(());
    }
    if a.reset || made_for_store {
        say(&format!("deleting namespace {ns} with its data volume and Secret"));
        let d = kc.kubectl(&["delete", "namespace", ns, "--wait=false", "--ignore-not-found"], 20, 60);
        if !d.ok() && !not_found(&d.combined()) {
            return Err(format!("could not delete the namespace {ns}: {}", crate::localreg::explain_kubectl(&d.combined())));
        }
        return wait_gone(&kc, ns, a, sleep, say);
    }
    // a namespace somebody else made: only what the store made in it
    say("deleting the data volume and the Secret (the namespace is not ours, so it stays)");
    delete_args(&kc, &["persistentvolumeclaims,secrets", "-n", ns, "-l", &sel], "the data volume and the Secret")?;
    delete_args(&kc, &[&format!("persistentvolumeclaim/{PVC}"), &format!("secret/{SECRET}"), "-n", ns], "the data volume and the Secret by name")?;
    Ok(())
}

fn wait_gone(kc: &Kc, ns: &str, a: &JobArgs, sleep: &dyn Fn(Duration), say: &mut dyn FnMut(&str)) -> Result<(), String> {
    const STEP: u64 = 2;
    let tries = (a.wait_secs / STEP).max(1);
    let mut forced = false;
    for i in 0..tries {
        let o = kc.read(&["get", "namespace", ns, "-o", "json"]);
        if !o.ok() {
            if not_found(&o.combined()) {
                say(&format!("namespace {ns} is gone"));
                return Ok(());
            }
            return Err(format!("the cluster stopped answering while waiting: {}", crate::localreg::explain_kubectl(&o.combined())));
        }
        if i % 5 == 0 {
            say(&format!("waiting for namespace {ns} to be gone ({}s of {}s)", i * STEP, a.wait_secs));
        }
        if a.force_finalizers && !forced && i * STEP >= a.wait_secs / 2 {
            forced = true;
            say("still there: clearing the finalizers of the objects left in it");
            clear_finalizers(kc, ns, say);
        }
        sleep(Duration::from_secs(STEP));
    }
    let left = kc.read(&["get", "all,persistentvolumeclaims", "-n", ns, "-o", "name"]);
    let objs: Vec<String> = left.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(8).collect();
    Err(format!(
        "namespace {ns} is still Terminating after {}s. Kubernetes deletes a namespace only when every object in it has finished deleting; the usual causes are a volume that is still attached or in use, or an object with a finalizer that nothing is handling.{} Nothing outside this namespace was touched. What to do: press Reset again to wait longer, or allow the finalizers of the leftovers to be cleared (the 'force' choice).{}",
        a.wait_secs,
        if objs.is_empty() { String::new() } else { format!(" Left in it: {}.", objs.join(", ")) },
        if a.force_finalizers { " The finalizers were cleared once and it still did not go: check the cluster's events (kubectl get events -n NAMESPACE)." } else { "" }
    ))
}

fn clear_finalizers(kc: &Kc, ns: &str, say: &mut dyn FnMut(&str)) {
    let o = kc.read(&["get", "persistentvolumeclaims,pods,deployments,replicasets,services", "-n", ns, "-o", "json"]);
    if !o.ok() {
        return;
    }
    let j: Value = serde_json::from_str(&o.stdout).unwrap_or(Value::Null);
    for i in j["items"].as_array().cloned().unwrap_or_default() {
        if i["metadata"]["finalizers"].as_array().is_some_and(|f| !f.is_empty()) {
            let kind = i["kind"].as_str().unwrap_or("").to_lowercase();
            let name = i["metadata"]["name"].as_str().unwrap_or("");
            if kind.is_empty() || name.is_empty() {
                continue;
            }
            let p = kc.kubectl(&["patch", &format!("{kind}/{name}"), "-n", ns, "--type=merge", "-p", r#"{"metadata":{"finalizers":null}}"#], 10, 20);
            say(&format!("{kind}/{name}: {}", if p.ok() { "finalizers cleared".to_string() } else { format!("not cleared ({})", first_line(&p.combined())) }));
        }
    }
}

// ------------------------------------------------------------------------------------------------ preparing and starting the job

/// Check an action before it starts: the typed namespace, that the state allows it, and the cluster guard (hidden, production-looking
/// and read-only clusters are refused; the cluster must still be the server it was). Returns the job's arguments for Reset/Remove.
pub fn prepare(pl: &Platform, rep: &Report, action: &Action, typed: &str, force_finalizers: bool) -> Result<Option<JobArgs>, String> {
    let o = rep.offers();
    let allowed = match action {
        Action::Repair => o.repair,
        Action::Reset => o.reset,
        Action::Remove(_) => o.remove,
    };
    if !allowed {
        return Err(format!("not possible in this state ({}). Nothing was changed.", rep.headline().to_lowercase()));
    }
    if action.destructive() && typed.trim() != rep.confirm_word() {
        return Err(format!("not done: type the namespace `{}` to confirm. Nothing was changed.", rep.confirm_word()));
    }
    platform::guard_write(pl, &rep.cluster)?;
    Ok(match action {
        Action::Repair => None,
        Action::Reset => Some(JobArgs { kubeconfig: rep.cluster.kubeconfig.clone(), context: rep.cluster.context.clone(), namespace: rep.namespace.clone(), reset: true, delete_data: true, wait_secs: 120, force_finalizers }),
        Action::Remove(c) => Some(JobArgs { kubeconfig: rep.cluster.kubeconfig.clone(), context: rep.cluster.context.clone(), namespace: rep.namespace.clone(), reset: false, delete_data: !c.keep_data, wait_secs: 120, force_finalizers }),
    })
}

/// The job for `args`, in the installer workspace so the top bar shows it.
pub fn job_spec(l: &crate::quickinstall::Launch, args: &JobArgs) -> crate::jobs::Spec {
    crate::jobs::Spec { label: if args.reset { JOB_LABEL_RESET } else { JOB_LABEL_REMOVE }.into(), cmd: crate::jobs::Cmd::Argv(args.argv(&l.exe.display().to_string())), cwd: l.data.clone(), workspace: crate::quickinstall::WORKSPACE.into() }
}

/// Start the job (one store job at a time).
pub fn start_job(jobs: &crate::jobs::Jobs, owner: &str, l: &crate::quickinstall::Launch, args: &JobArgs) -> Result<crate::jobs::Job, String> {
    if jobs.visible(owner, crate::quickinstall::WORKSPACE).iter().any(|j| j.running() && (j.label == JOB_LABEL_RESET || j.label == JOB_LABEL_REMOVE)) {
        return Err("a private store reset or removal is running already: its progress is beside the clock in the top bar".into());
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &job_spec(l, args), &|_: &std::path::Path| Ok(()))
}

// ------------------------------------------------------------------------------------------------ after the job: this device

/// What is cleaned on this device once the cluster side succeeded: the installed-services record always; for Reset and for Remove with
/// the data deleted also the vault entries this store made, its STORE catalog entry and that entry's publish token. Only this store's.
/// With the data kept the vault entries and the catalog entry stay (the key and the token belong to the data). Trusted publisher keys
/// are never touched. The in-cluster registry is deleted only when asked (`registry`).
pub fn finish_local(pl: &Platform, store: &Store, actor: &Actor, rep: &Report, action: &Action, registry: Option<(&crate::localreg::Target, &dyn crate::ocipush::RegistryHttp)>, say: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let wipe = match action {
        Action::Repair => return Ok(done),
        Action::Reset => true,
        Action::Remove(c) => !c.keep_data,
    };
    if wipe {
        for n in store_vault_entries(rep) {
            if pl.vault.has(&n) {
                let _ = pl.vault.delete(&n);
                done.push(format!("deleted vault entry uvault:{n}"));
            }
        }
        if let Some((sid, name)) = &rep.source {
            let tok = super::authtools::token_entry(sid);
            if pl.vault.has(&tok) {
                let _ = pl.vault.delete(&tok);
                done.push(format!("deleted vault entry uvault:{tok}"));
            }
            if store.config().publish_target == *sid {
                let _ = store.set_publish_target(actor, "");
            }
            match store.remove_source(actor, sid) {
                Ok(()) => done.push(format!("removed the STORE catalog entry {name}")),
                Err(e) if e.contains("no such catalog") => {}
                Err(e) => return Err(e),
            }
        }
    } else {
        done.push("kept the vault entries and the STORE catalog entry (they belong to the data you kept)".into());
    }
    if rep.record.is_some() {
        installed::remove(&pl.dir, &pl.user, &rep.instance)?;
        done.push("removed the store from your installed services".into());
    }
    if let (Action::Remove(c), Some((t, http))) = (action, registry) {
        if c.remove_registry {
            crate::localreg::remove(&pl.run, http, t, "iyakki-registry", say)?;
            crate::quickinstall::unmark(&pl.dir, crate::quickinstall::Item::Registry);
            done.push("deleted the in-cluster image registry".into());
        }
    }
    pl.audit("store.manage", &format!("{:?} {}:{} ok", action, rep.namespace, rep.cluster.server));
    Ok(done)
}

#[cfg(test)]
#[path = "privatemanage_tests.rs"]
mod tests;
