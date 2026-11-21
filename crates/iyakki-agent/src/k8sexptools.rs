//! The assistant's read-only tool for the cluster explorer's custom resources (F-K8S-26): `k8s_custom_resources` lists the custom resource
//! definitions of a cluster the person may see in the K8S tile, and the instances of one definition. It is the chat side of the tile's
//! Instances tab: the same registry of clusters (what the account may use), the same `k8sexp` and `k8scr` code, one read-only
//! `kubectl get` each, never a change. What a cluster prints is data: the tool is marked untrusted so it is never taken as an instruction.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::clusters::ClusterEntryExt;
use crate::k8s::{Cluster, Kube, Target};
use crate::k8scr::{list_instances, CrdInfo};
use crate::k8sexp::{self as kx, valid_label, valid_name, Kind, Scope};
pub use iyakki_shared::k8sexptools::{PROMPT_RULE};

pub const TOOL: &str = "k8s_custom_resources";
/// Applying a YAML manifest to a cluster the person sees in the K8S tile (F-K8S-26): asks every time, a server-side dry run first.
pub const APPLY: &str = "k8s_apply_yaml";
pub use iyakki_shared::k8sexptools::{scan_manifest, Obj, MAX_OBJECTS, MAX_YAML};

/// One cluster the account may use: its id and name, where kubectl looks, and the client to ask.
pub struct Cluster1 {
    pub id: String,
    pub name: String,
    pub target: Target,
    pub kube: Arc<dyn Kube>,
}

pub trait Backend: Send + Sync {
    /// The clusters this account may use, the managed one first.
    fn clusters(&self) -> Vec<Cluster1>;
    fn now(&self) -> u64;
    fn audit(&self, action: &str, detail: &str);
}

pub struct K8sExpTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
    /// A team's deploy rules around changes to a cluster bound to a team environment (F-TCFG-10); none for a person's own clusters.
    gate: Option<Arc<dyn crate::tenantconfig::clustergate::TeamGate>>,
}

impl K8sExpTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> K8sExpTools {
        K8sExpTools { inner, be, gate: None }
    }

    /// Changes to a cluster the gate knows go through the team's deploy rules (approval request, spend, audit).
    pub fn with_gate(mut self, gate: Arc<dyn crate::tenantconfig::clustergate::TeamGate>) -> K8sExpTools {
        self.gate = Some(gate);
        self
    }

    fn pick(&self, args: &Value) -> Result<Cluster1, String> {
        let mut all = self.be.clusters();
        if all.is_empty() {
            return Err("no cluster is set up for this account".into());
        }
        match args["cluster"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
            None => Ok(all.remove(0)),
            Some(c) => match all.iter().position(|e| e.id == c || e.name == c) {
                Some(i) => Ok(all.remove(i)),
                None => Err(format!("no cluster called `{c}` for this account (they are: {})", all.iter().map(|e| e.name.clone()).collect::<Vec<_>>().join(", "))),
            },
        }
    }

    fn apply_input(&self, args: &Value) -> Result<(String, Vec<Obj>, Option<String>), String> {
        let y = args["yaml"].as_str().filter(|y| !y.trim().is_empty()).ok_or("give the manifest as `yaml` (text)")?;
        let ns = args["namespace"].as_str().map(str::trim).filter(|d| !d.is_empty());
        if let Some(n) = ns {
            if !valid_label(n) {
                return Err(format!("`{n}` is not a namespace name"));
            }
        }
        Ok((y.to_string(), scan_manifest(y)?, ns.map(str::to_string)))
    }

    fn apply_card(&self, args: &Value) -> Result<String, String> {
        let c = self.pick(args)?;
        let (_, objs, ns) = self.apply_input(args)?;
        let mut s = format!("Apply a manifest to the cluster “{}” ({} object{}).\nIt is first checked by the cluster itself with a dry run; nothing is changed if that fails.\n", c.name, objs.len(), if objs.len() == 1 { "" } else { "s" });
        for o in &objs {
            s.push_str(&format!("• {} {}{}{}\n", o.kind, o.name, if o.cluster_wide() { String::new() } else if o.namespace.is_empty() { ns.as_deref().map(|n| format!(" in {n}")).unwrap_or_default() } else { format!(" in {}", o.namespace) }, if o.cluster_wide() { "  (CLUSTER-WIDE)" } else { "" }));
        }
        if objs.iter().any(Obj::cluster_wide) {
            s.push_str("Some of these objects are not inside one namespace: they change the whole cluster.\n");
        }
        s.push_str("Existing objects with these names are changed to match the manifest. Nothing is deleted.");
        Ok(s)
    }

    fn run_apply(&self, args: &Value) -> Result<String, String> {
        let c = self.pick(args)?;
        let (yaml, objs, ns) = self.apply_input(args)?;
        let mut a = c.target.base_args_secs(30);
        if let Some(n) = &ns {
            a.extend(["-n".into(), n.clone()]);
        }
        a.push("apply".into());
        let mut dry = a.clone();
        dry.push("--dry-run=server".into());
        let d = c.kube.apply_manifest(&dry, &yaml, 40);
        if !d.ok() {
            self.be.audit("k8s.apply_yaml", &format!("{} refused by the dry run", c.id));
            return Err(format!("the cluster did not accept the manifest, so nothing was changed:\n{}", d.stderr.lines().chain(d.stdout.lines()).take(12).collect::<Vec<_>>().join("\n")));
        }
        let mut real = |_approved_by: Option<&str>| -> Result<String, String> {
            let o = c.kube.apply_manifest(&a, &yaml, 60);
            if !o.ok() {
                self.be.audit("k8s.apply_yaml", &format!("{} failed", c.id));
                return Err(format!("the apply failed after the dry run passed (some objects may have changed):\n{}", o.stderr.lines().chain(o.stdout.lines()).take(12).collect::<Vec<_>>().join("\n")));
            }
            Ok(o.stdout)
        };
        let stdout = match self.gate.as_ref().and_then(|g| g.through(&c.id, &mut real)) {
            Some(r) => r?,
            None => real(None)?,
        };
        let o = iyakki_shared::wsexec::Out { stdout, ..Default::default() };
        self.be.audit("k8s.apply_yaml", &format!("{} {} object(s)", c.id, objs.len()));
        Ok(format!("Applied {} object{} to {}:\n{}", objs.len(), if objs.len() == 1 { "" } else { "s" }, c.name, o.stdout.lines().take(60).collect::<Vec<_>>().join("\n")))
    }

    fn run(&self, args: &Value) -> Result<String, String> {
        let c = self.pick(args)?;
        let now = self.be.now();
        let def = args["definition"].as_str().map(str::trim).filter(|d| !d.is_empty());
        let ns = args["namespace"].as_str().map(str::trim).filter(|d| !d.is_empty());
        if let Some(n) = ns {
            if !valid_label(n) {
                return Err(format!("`{n}` is not a namespace name"));
            }
        }
        let Some(def) = def else {
            let l = kx::list(&*c.kube, &c.target, &Scope::All, Kind::Crds, now);
            self.be.audit("k8s.custom_resources", &format!("{} definitions", c.id));
            return match l.cluster {
                Cluster::Ok if l.rows.is_empty() => Ok(format!("{}: no custom resource definitions.", c.name)),
                Cluster::Ok => Ok(format!("{}: {} definition{}\n{}", c.name, l.total, if l.total == 1 { "" } else { "s" }, l.rows.iter().map(|r| format!("{}  group {}  {}  kind {}", r.name, r.cells.get(1).map_or("", |x| x.as_str()), r.cells.get(2).map_or("", |x| x.as_str()), r.cells.get(3).map_or("", |x| x.as_str()))).collect::<Vec<_>>().join("\n"))),
                Cluster::KubectlMissing => Err("kubectl is not available on this computer".into()),
                Cluster::Unreachable(w) | Cluster::Forbidden(w) | Cluster::Failed(w) => Err(w),
            };
        };
        // the definition is named by its resource argument (`plural.group`); the cluster itself says what it is
        if !valid_name(def) {
            return Err(format!("`{def}` is not a definition name (such as widgets.example.com)"));
        }
        let name = {
            // a definition is `<plural>.<group>`: its own object name is the same text
            def.to_string()
        };
        let obj = kx::get_object(&*c.kube, &c.target, Kind::Crds, "", &name)?;
        let crd = CrdInfo::from_object(&obj).ok_or("that definition does not say its group and plural name, so its instances cannot be listed")?;
        let scope = ns.map_or(Scope::All, |n| Scope::Ns(n.to_string()));
        let l = list_instances(&*c.kube, &c.target, &crd, &scope, now)?;
        self.be.audit("k8s.custom_resources", &format!("{} {}", c.id, crd.resource()));
        if l.rows.is_empty() {
            return Ok(format!("{}: no instances of {}.", c.name, l.crd));
        }
        let mut out = format!("{}: {} instance{} of {}{}\n", c.name, l.total, if l.total == 1 { "" } else { "s" }, l.crd, if l.total > l.rows.len() { format!(" (the first {} are shown)", l.rows.len()) } else { String::new() });
        for r in &l.rows {
            out.push_str(&format!("{}{} · {} old{}\n", r.name, if r.namespace.is_empty() { String::new() } else { format!(" in {}", r.namespace) }, r.age, if r.status.is_empty() { String::new() } else { format!(" · {}", r.status) }));
        }
        Ok(out)
    }
}

impl Tools for K8sExpTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != TOOL);
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Read the custom resources of a cluster the person sees in the K8S tile: the definitions, or the instances of one definition (such as widgets.example.com). Read-only; what it prints came from the cluster.".into(), schema: json!({"type": "object", "properties": {"cluster": {"type": "string", "description": "A cluster's id or name; default: the first one."}, "definition": {"type": "string", "description": "A definition name such as widgets.example.com; leave out to list the definitions."}, "namespace": {"type": "string", "description": "One namespace; default: all."}}}) }, risk: Risk::Safe, untrusted: true });
        v.retain(|d| d.spec.name != APPLY);
        v.push(ToolDef { spec: ToolSpec { name: APPLY.into(), description: "Apply a YAML manifest to a cluster the person sees in the K8S tile. The person approves a card that lists every object (cluster-wide ones marked); the cluster dry-runs it first. Changes existing objects with the same names, never deletes.".into(), schema: json!({"type": "object", "properties": {"cluster": {"type": "string", "description": "A cluster's id or name; default: the first one."}, "yaml": {"type": "string", "description": "The manifest text (one or more documents separated by ---); at most 256 KiB and 50 objects. Never put secrets in it."}, "namespace": {"type": "string", "description": "The namespace for objects that name none."}}, "required": ["yaml"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == APPLY {
            return self.run_apply(args);
        }
        if name == TOOL { self.run(args) } else { self.inner.call(name, args) }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == APPLY {
            return match self.apply_input(args) {
                Ok((_, o, _)) => format!("apply a manifest of {} object{} to a cluster", o.len(), if o.len() == 1 { "" } else { "s" }),
                Err(e) => format!("{APPLY} (invalid: {e})"),
            };
        }
        if name == TOOL { format!("read the custom resources{}", args["definition"].as_str().map(|d| format!(" of {d}")).unwrap_or_default()) } else { self.inner.describe(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        (name == TOOL) || (name != APPLY && self.inner.pre_approved(name))
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == APPLY {
            return self.apply_card(args).ok();
        }
        if name == TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == APPLY {
            return self.apply_card(args).err();
        }
        if name == TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn approved_always(&self, name: &str) -> bool {
        name != TOOL && name != APPLY && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == APPLY { Some("changing a cluster asks every time".into()) } else if name == TOOL { None } else { self.inner.grant_blocker(name, args) }
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn correct_claims(&self, answer: &str) -> Option<String> {
        self.inner.correct_claims(answer)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: the cluster registry of the account (what the K8S tile lists) and kubectl through the same runners as the tile.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub role: iyakki_sec::policy::Role,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn clusters(&self) -> Vec<Cluster1> {
        let dir = self.svc.dir();
        let reg = crate::clusters::Registry::load_synced(dir, &self.user);
        let k3s = self.svc.k3sdev.clone();
        let podman = k3s.installed().is_some_and(|s| s.backend == crate::k3sdev::Backend::Podman);
        let native = k3s.installed().is_some_and(|s| s.backend == crate::k3sdev::Backend::Native);
        let run: Arc<dyn crate::wsexec::Runner> = Arc::new(crate::wsexec::System);
        let kube = |managed: bool| -> Arc<dyn Kube> {
            if managed && podman { Arc::new(crate::devcluster::PodKube { run: run.clone(), container: crate::k3sdev::POD_NAME.into() }) } else { Arc::new(crate::devcluster::RunnerKube { run: run.clone() }) }
        };
        let mut v = Vec::new();
        for e in reg.visible_to(self.role) {
            let (target, managed) = if e.is_primary() {
                (Target { kubeconfig: if native { k3s.layout().kubeconfig().display().to_string() } else { String::new() }, ..Default::default() }, true)
            } else if e.kind.is_local() {
                (Target { kubeconfig: e.kubeconfig_for(dir), ..Default::default() }, false)
            } else {
                (Target { kubeconfig: e.kubeconfig.clone(), context: e.context.clone(), ..Default::default() }, false)
            };
            v.push(Cluster1 { id: e.id.clone(), name: e.name.clone(), target, kube: kube(managed) });
        }
        v
    }
    fn now(&self) -> u64 {
        crate::clock::now_unix().max(0) as u64
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
#[path = "k8sexptools_tests.rs"]
mod tests;
