use crate::platform_types::ClusterRef;
use crate::platform_types::Installed;

pub const DEFAULT_NAMESPACE: &str = "iyakki-private-store";

/// The label of the jobs, and what `iyakki store-manage` is called.
pub const JOB_LABEL_RESET: &str = "Reset private store";

pub const JOB_LABEL_REMOVE: &str = "Remove private store";

/// What is wrong, when something is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    ClusterDown,
    NoKubectl,
    Terminating,
    NotOurs,
    CrashLoop,
    NoEndpoints,
    TokenLost,
    /// The cluster has the store but this device has no record of it.
    NoRecord,
    /// This device lists the store (record or STORE catalog entry) but the cluster has nothing.
    CatalogOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreState {
    NotInstalled,
    Healthy,
    /// The install stopped half-way: what is there and what is not, in plain words.
    Partial { present: Vec<String>, missing: Vec<String> },
    Broken { why: Why, reason: String },
}

/// What the cluster and this device said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub namespace: bool,
    pub terminating: bool,
    /// The namespace carries `managed-by=iyakki`.
    pub managed: bool,
    /// ... and `iyakki.dev/service=private-store` (IYAKKI made it for this store).
    pub made_for_store: bool,
    pub deployment: bool,
    pub desired: u32,
    pub ready: u32,
    pub service: bool,
    pub pvc: bool,
    pub secret: bool,
    pub endpoints: bool,
    /// A pod's waiting reason such as CrashLoopBackOff, empty when none.
    pub bad_pod: String,
    pub token: bool,
    pub seed: bool,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub state: StoreState,
    pub cluster: ClusterRef,
    pub namespace: String,
    pub instance: String,
    pub facts: Facts,
    /// The installed-services record of this store in THIS cluster.
    pub record: Option<Installed>,
    /// A record of a private store in another cluster exists (never touched here).
    pub elsewhere: Option<String>,
    /// The STORE catalog entry (id, name) added for this store's address.
    pub source: Option<(String, String)>,
}

/// Which buttons make sense.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    pub deploy: bool,
    pub repair: bool,
    pub reset: bool,
    pub remove: bool,
    pub start_cluster: bool,
}

impl Report {
    pub fn offers(&self) -> Offers {
        let exists = self.facts.namespace && self.facts.managed;
        // Reset deletes the namespace as a whole, which only a namespace IYAKKI made for the store allows
        let whole = self.facts.namespace && self.facts.made_for_store;
        match &self.state {
            StoreState::NotInstalled => Offers { deploy: true, ..Default::default() },
            StoreState::Healthy => Offers { reset: whole, remove: exists, ..Default::default() },
            StoreState::Partial { .. } => Offers { repair: true, reset: whole, remove: exists, ..Default::default() },
            StoreState::Broken { why, .. } => match why {
                Why::ClusterDown => Offers { start_cluster: true, ..Default::default() },
                Why::NoKubectl | Why::NotOurs => Offers::default(),
                Why::Terminating => Offers { reset: whole, remove: exists, ..Default::default() },
                Why::CatalogOnly => Offers { repair: true, reset: true, remove: true, ..Default::default() },
                Why::CrashLoop | Why::NoEndpoints | Why::TokenLost | Why::NoRecord => Offers { repair: true, reset: whole, remove: exists, ..Default::default() },
            },
        }
    }

    /// The word the person types to confirm anything destructive: the namespace.
    pub fn confirm_word(&self) -> &str {
        &self.namespace
    }

    pub fn headline(&self) -> String {
        match &self.state {
            StoreState::NotInstalled => "NOT INSTALLED".into(),
            StoreState::Healthy => "HEALTHY".into(),
            StoreState::Partial { .. } => "PARTLY INSTALLED".into(),
            StoreState::Broken { .. } => "BROKEN".into(),
        }
    }

    /// The screen's text: state, what exists, what is missing, what to do.
    pub fn text(&self) -> String {
        let mut t = format!("Private store in namespace {} on {}\nSTATE: {}\n\n", self.namespace, self.cluster.line(), self.headline());
        match &self.state {
            StoreState::NotInstalled => t.push_str("Nothing of the private store is in this cluster and this device has no record of one. Deploy it to begin.\n"),
            StoreState::Healthy => t.push_str("The namespace, the server, the address and the data volume are there, the server answers, and this device has its admin token. Nothing needs fixing.\n"),
            StoreState::Partial { present, missing } => {
                t.push_str("The install stopped half-way.\n");
                t.push_str(&format!("There: {}\n", if present.is_empty() { "nothing".to_string() } else { present.join(", ") }));
                t.push_str(&format!("Missing: {}\n", missing.join(", ")));
                t.push_str("\nRepair puts back only what is missing and keeps the data. Reset and reinstall starts again from nothing.\n");
            }
            StoreState::Broken { reason, .. } => {
                t.push_str(reason);
                t.push('\n');
            }
        }
        if let Some(e) = &self.elsewhere {
            t.push_str(&format!("\nNote: {e}\n"));
        }
        let o = self.offers();
        t.push_str("\nWHAT YOU CAN DO\n");
        let mut any = false;
        let mut line = |s: &str| {
            any = true;
            t.push_str(&format!("- {s}\n"));
        };
        if o.deploy {
            line("Deploy: continue to the deploy wizard.");
        }
        if o.repair {
            line("Repair: put back what is missing, keep your published apps.");
        }
        if o.reset {
            line("Reset and reinstall: delete the namespace, wait until it is gone, deploy again (the old data is deleted).");
        }
        if o.remove {
            line("Remove: delete what the store made; choose whether to keep the data volume (default: keep).");
        }
        if o.start_cluster {
            line("Start the cluster (Settings, Dev cluster), then look again.");
        }
        if !any {
            t.push_str("- Nothing from here; see the message above.\n");
        }
        t
    }
}

/// The pure part: the state from what was found.
pub fn classify(f: &Facts, record: bool, source: bool) -> StoreState {
    if !f.namespace {
        return if record || source {
            StoreState::Broken { why: Why::CatalogOnly, reason: "This device lists a private store (in its installed services or in STORE), but the cluster has no such namespace any more. It was deleted outside IYAKKI, or the cluster was rebuilt. Repair deploys it again (new data volume); Remove forgets the entries here.".into() }
        } else {
            StoreState::NotInstalled
        };
    }
    if f.terminating {
        return StoreState::Broken { why: Why::Terminating, reason: "The namespace is stuck in Terminating: Kubernetes is deleting it but something inside has not finished (a volume still in use, or an object with a finalizer). Nothing can be installed into it meanwhile. Reset and reinstall waits for it to go (with a time limit) and can clear the finalizers of what is left if you allow it.".into() };
    }
    if !f.managed {
        return StoreState::Broken { why: Why::NotOurs, reason: "A namespace with this name exists but IYAKKI does not manage it (it lacks the label app.kubernetes.io/managed-by=iyakki), so IYAKKI will not change or delete anything in it. Choose another namespace in the deploy options, or label it yourself if it really is the store's.".into() };
    }
    let mut present = vec!["the namespace".to_string()];
    let mut missing = Vec::new();
    for (ok, there, gone) in [(f.pvc, "the data volume (PVC)", "the data volume (PVC)"), (f.secret, "the admin token Secret", "the admin token Secret"), (f.deployment, "the server (Deployment)", "the server (Deployment)"), (f.service, "the address (Service)", "the address (Service)")] {
        if ok {
            present.push(there.to_string());
        } else {
            missing.push(gone.to_string());
        }
    }
    if !missing.is_empty() {
        return StoreState::Partial { present, missing };
    }
    if !f.bad_pod.is_empty() {
        return StoreState::Broken { why: Why::CrashLoop, reason: format!("The server pod cannot run: {}. Repair restarts the server and shows the cluster's events; if the image is missing from the in-cluster registry, deploy again so it is pushed.", f.bad_pod) };
    }
    if f.ready < f.desired.max(1) || !f.endpoints {
        return StoreState::Broken { why: Why::NoEndpoints, reason: "The server is deployed but nothing answers behind its address (no ready pod, no endpoints). If it was just deployed wait a minute and look again; otherwise Repair restarts it.".into() };
    }
    if !f.token {
        return StoreState::Broken { why: Why::TokenLost, reason: "The store is running but this device no longer has its admin token (the vault entry is gone), so you cannot publish or add publishers. Repair reads it back from the store's Secret in the cluster and puts it in your vault; nothing is regenerated.".into() };
    }
    if !record {
        return StoreState::Broken { why: Why::NoRecord, reason: "The store is running in the cluster but this device has no record of it (the installed-services entry is missing). Repair adopts it again without touching the data.".into() };
    }
    StoreState::Healthy
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveChoice {
    /// Keep the data volume (and the Secret, namespace and vault entries that go with it): the default.
    pub keep_data: bool,
    /// Also delete the in-cluster registry (separate from the store; default keep).
    pub remove_registry: bool,
}

impl Default for RemoveChoice {
    fn default() -> Self {
        RemoveChoice { keep_data: true, remove_registry: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Repair,
    Reset,
    Remove(RemoveChoice),
}

impl Action {
    pub fn destructive(&self) -> bool {
        !matches!(self, Action::Repair)
    }
}

/// The store's vault entries that IYAKKI made for this store, by name (never anything else).
pub fn store_vault_entries(rep: &Report) -> Vec<String> {
    let prefix = format!("svc.{}.", rep.instance);
    let (a, b) = vault_names(&rep.instance);
    let mut v: Vec<String> = rep.record.iter().flat_map(|r| r.vault.iter().cloned()).filter(|n| n.starts_with(&prefix)).collect();
    for n in [a, b] {
        if !v.contains(&n) {
            v.push(n);
        }
    }
    v.sort();
    v
}

/// Exactly what the action will do, to read before approving.
pub fn preview(rep: &Report, action: &Action) -> String {
    let ns = &rep.namespace;
    let mut s = String::new();
    match action {
        Action::Repair => {
            s.push_str(&format!("REPAIR the private store in namespace {ns} on {}\n\n", rep.cluster.line()));
            let steps = repair_steps(rep);
            if steps.is_empty() {
                s.push_str("Nothing needs to be put back.\n");
            }
            for (i, t) in steps.iter().enumerate() {
                s.push_str(&format!("{}. {t}\n", i + 1));
            }
            s.push_str("\nWhat is already there is not touched: the data volume, the Secret and the published apps stay as they are.\n");
        }
        Action::Reset => {
            s.push_str(&format!("RESET AND REINSTALL the private store in namespace {ns} on {}\n\nThis DELETES, then deploys again:\n", rep.cluster.line()));
            s.push_str(&delete_list(rep, false));
            s.push_str("\nThen it waits until the namespace is gone (up to 2 minutes; if it is stuck Terminating you are told why and can allow clearing the finalizers of what is left), and the deploy wizard continues from the options.\n");
            s.push_str("\nWARNING: the data volume and every published app on it are DELETED and cannot be recovered. The catalog key is made again, so every device has to add the store again. Use Repair to keep your apps.\n");
        }
        Action::Remove(c) => {
            s.push_str(&format!("REMOVE the private store in namespace {ns} on {}\n\nThis deletes:\n", rep.cluster.line()));
            s.push_str(&delete_list(rep, c.keep_data));
            if c.keep_data {
                s.push_str("\nKEPT: the data volume (your published apps), the Secret, the namespace and the vault entries. Repair brings the store back with its apps later. (Your choice: Ctrl+K switches to delete everything.)\n");
            } else {
                s.push_str("\nWARNING: delete everything. The data volume and every published app on it are DELETED and cannot be recovered. (Ctrl+K switches back to keep my published apps.)\n");
            }
            s.push_str(&format!("\nThe in-cluster image registry is a separate thing: {} (Ctrl+R switches).\n", if c.remove_registry { "it WILL be deleted too, with every image in it" } else { "it is KEPT" }));
        }
    }
    if action.destructive() {
        s.push_str(&format!("\nTo approve, type the namespace ({ns}) below.\n"));
    }
    s.push_str("\nThe publisher keys you trusted (STORE, Settings, Trust a publisher key) stay in every case.");
    s
}

/// The documents of a manifest (split at lines that are exactly `---`) whose `kind:` is in `kinds`.
pub fn only_kinds(manifest: &str, kinds: &[&str]) -> String {
    let mut docs: Vec<String> = Vec::new();
    let mut cur = String::new();
    for l in manifest.lines() {
        if l.trim_end() == "---" {
            docs.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(l);
            cur.push('\n');
        }
    }
    docs.push(cur);
    let kept: Vec<String> = docs
        .into_iter()
        .filter(|d| d.lines().find_map(|l| l.strip_prefix("kind: ")).is_some_and(|k| kinds.contains(&k.trim())))
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    kept.join("---\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobArgs {
    pub kubeconfig: String,
    pub context: String,
    pub namespace: String,
    /// Reset (delete the namespace) or Remove.
    pub reset: bool,
    pub delete_data: bool,
    pub wait_secs: u64,
    pub force_finalizers: bool,
}

impl JobArgs {
    pub fn argv(&self, exe: &str) -> Vec<String> {
        let mut a: Vec<String> = vec![exe.into(), "store-manage".into(), if self.reset { "reset" } else { "remove" }.into(), "--kubeconfig".into(), self.kubeconfig.clone()];
        if !self.context.is_empty() {
            a.extend(["--context".into(), self.context.clone()]);
        }
        a.extend(["--namespace".into(), self.namespace.clone(), "--wait".into(), self.wait_secs.to_string()]);
        if self.delete_data {
            a.push("--delete-data".into());
        }
        if self.force_finalizers {
            a.push("--force".into());
        }
        a
    }

    pub fn parse(args: &[String]) -> Result<JobArgs, String> {
        let mode = args.first().map(String::as_str).unwrap_or("");
        if !matches!(mode, "reset" | "remove") {
            return Err("usage: iyakki store-manage reset|remove --kubeconfig FILE [--context NAME] --namespace NS [--delete-data] [--wait SECONDS] [--force]".into());
        }
        let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
        let ns = flag("--namespace").unwrap_or_default();
        if !crate::platform_types::dns_label(&ns) {
            return Err(format!("`{ns}` is not a namespace name"));
        }
        Ok(JobArgs {
            kubeconfig: flag("--kubeconfig").unwrap_or_default(),
            context: flag("--context").unwrap_or_default(),
            namespace: ns,
            reset: mode == "reset",
            delete_data: mode == "reset" || args.iter().any(|a| a == "--delete-data"),
            wait_secs: flag("--wait").and_then(|w| w.parse().ok()).unwrap_or(120).clamp(10, 1800),
            force_finalizers: args.iter().any(|a| a == "--force"),
        })
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----

pub fn vault_names(instance: &str) -> (String, String) {
    (format!("svc.{instance}.admin-token"), format!("svc.{instance}.catalog-seed"))
}

pub fn delete_list(rep: &Report, keep_data: bool) -> String {
    let ns = &rep.namespace;
    let mut s = String::new();
    s.push_str(&format!("- in the cluster: the Deployment, Service and Ingress of the store{}\n", if rep.facts.namespace { "" } else { " (the namespace is already gone, so nothing)" }));
    if keep_data {
        return s;
    }
    s.push_str(&format!("- in the cluster: the data volume (PVC) {PVC}, the Secret {SECRET}, and the namespace {ns}\n"));
    let v = store_vault_entries(rep);
    s.push_str(&format!("- in your vault: {}\n", v.iter().map(|n| format!("uvault:{n}")).collect::<Vec<_>>().join(", ")));
    match &rep.source {
        Some((id, name)) => s.push_str(&format!("- in STORE: the catalog entry {name} ({id}) and its publish token uvault:store-token-{id}\n")),
        None => s.push_str("- in STORE: no catalog entry for this store\n"),
    }
    s.push_str("- on this device: the installed-services record of the store\n");
    s
}

pub fn repair_steps(rep: &Report) -> Vec<String> {
    let f = &rep.facts;
    let mut v = Vec::new();
    if matches!(&rep.state, StoreState::Broken { why: Why::CatalogOnly, .. }) {
        v.push("deploy the whole store again (the namespace is gone): namespace, Secret, data volume, server, address".to_string());
        return v;
    }
    if !f.secret && f.namespace {
        v.push("create the Secret with a NEW admin token (the old one cannot be read back); the catalog key changes if the data volume is reused, so devices add the store again".to_string());
    }
    if f.secret && !f.token {
        v.push("read the admin token back from the cluster's Secret into your vault".to_string());
    }
    for (ok, what) in [(f.pvc, "the data volume (PVC)"), (f.deployment, "the server (Deployment)"), (f.service, "the address (Service)")] {
        if !ok && f.namespace {
            v.push(format!("create {what}"));
        }
    }
    if !f.bad_pod.is_empty() || (f.deployment && !f.endpoints) {
        v.push("restart the server and wait until it is ready".to_string());
    }
    if rep.record.is_none() {
        v.push("record the store in your installed services again".to_string());
    }
    v
}

const PVC: &str = "private-store-data";
const SECRET: &str = "private-store";
