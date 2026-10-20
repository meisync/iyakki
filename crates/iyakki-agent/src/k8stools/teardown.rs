//! Tear down the workspace's own dev environment: `k8s_stop_env` (stop = scale every Deployment and StatefulSet to 0, keeping the
//! previous counts in the annotation `iyakki.dev/replicas`; start = scale back), `k8s_teardown` (delete the namespace and
//! everything in it) and `k8s_registry_cleanup` (delete the images pushed for the workspace from the IYAKKI registry).
//!
//! Only the namespace IYAKKI created for this workspace: it must carry the labels `app.kubernetes.io/managed-by=iyakki` and
//! `iyakki.dev/workspace=<name>` (put on at the first apply). `default`, `kube-*`, `iyakki-registry` and anything unlabelled are
//! refused with the manual command. Never on a production-looking cluster or one that only came from the environment.

use std::time::Duration;

use serde_json::Value;

use super::{arg, first_line, K8sTools};

pub const LABEL_MANAGED: &str = "app.kubernetes.io/managed-by";
pub const LABEL_WORKSPACE: &str = "iyakki.dev/workspace";
pub const ANNOTATION_REPLICAS: &str = "iyakki.dev/replicas";

/// A Deployment or StatefulSet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Workload {
    kind: &'static str,
    name: String,
    replicas: u64,
    saved: Option<u64>,
}

/// Namespaces IYAKKI never stops, removes or deploys into by itself: `default`, `kube-*` and the registry's.
pub fn protected_namespace(ns: &str) -> bool {
    ns == "default" || ns.starts_with("kube-") || ns == crate::regsetup::IN_NS
}

impl K8sTools {
    /// `Err` with the plain explanation unless this is a namespace IYAKKI manages for this workspace.
    pub(super) fn managed_namespace(&self) -> Result<(), String> {
        let (ns, ws) = (&self.ns, &self.ws);
        let manual = format!("If you created `{ns}` yourself and want it gone, run it yourself: kubectl delete namespace {ns}   (or, to let IYAKKI manage it: kubectl label namespace {ns} {LABEL_MANAGED}=iyakki {LABEL_WORKSPACE}={ws})");
        if protected_namespace(ns) {
            return Err(format!("refused: `{ns}` is a protected namespace and is never stopped or removed by IYAKKI. {manual}"));
        }
        let o = self.kc().read(&["get", "namespace", ns, "-o", "json"]);
        if !o.ok() {
            let t = Self::out_text(&o);
            return Err(if t.contains("NotFound") || t.contains("not found") { format!("the namespace `{ns}` does not exist: there is nothing to stop or remove") } else { format!("could not read namespace {ns}: {}", first_line(&t)) });
        }
        let j: Value = serde_json::from_str(&o.stdout).map_err(|_| format!("could not read the labels of namespace {ns}"))?;
        let l = &j["metadata"]["labels"];
        if l[LABEL_MANAGED].as_str() == Some("iyakki") && l[LABEL_WORKSPACE].as_str() == Some(ws.as_str()) {
            Ok(())
        } else {
            Err(format!("refused: namespace `{ns}` is not one IYAKKI manages for the workspace \"{ws}\": it lacks the labels {LABEL_MANAGED}=iyakki and {LABEL_WORKSPACE}={ws} (IYAKKI puts them on a namespace it creates). {manual}"))
        }
    }

    fn teardown_guard(&self) -> Result<(), String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        self.managed_namespace()
    }

    fn workloads(&self) -> Result<Vec<Workload>, String> {
        let o = self.kc().read(&["get", "deployments,statefulsets", "-n", &self.ns, "-o", "json"]);
        if !o.ok() {
            return Err(super::cut(&Self::out_text(&o), 1500));
        }
        let j: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
        Ok(j["items"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| Workload {
                        kind: if i["kind"].as_str() == Some("StatefulSet") { "statefulset" } else { "deployment" },
                        name: i["metadata"]["name"].as_str().unwrap_or("").to_string(),
                        replicas: i["spec"]["replicas"].as_u64().unwrap_or(1),
                        saved: i["metadata"]["annotations"][ANNOTATION_REPLICAS].as_str().and_then(|s| s.parse().ok()),
                    })
                    .filter(|w| !w.name.is_empty())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Exactly what the namespace holds, for the approval card. Secrets by name only (their values are never read).
    pub(super) fn inventory(&self) -> Result<String, String> {
        let kc = self.kc();
        let o = kc.read(&["get", "deployments,statefulsets,services,ingresses,pods,persistentvolumeclaims,configmaps", "-n", &self.ns, "-o", "json"]);
        if !o.ok() {
            return Err(super::cut(&Self::out_text(&o), 1500));
        }
        let j: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
        let items = j["items"].as_array().cloned().unwrap_or_default();
        let of = |kind: &str| -> Vec<&Value> { items.iter().filter(|i| i["kind"].as_str() == Some(kind)).collect() };
        let name = |i: &Value| i["metadata"]["name"].as_str().unwrap_or("?").to_string();
        let mut s = format!("Namespace {} (workspace \"{}\", managed by IYAKKI) holds:\n", self.ns, self.ws);
        let mut line = |label: &str, v: Vec<String>| s.push_str(&format!("- {label} ({}): {}\n", v.len(), if v.is_empty() { "none".to_string() } else { v.join(", ") }));
        line("Deployments", of("Deployment").iter().map(|i| format!("{} (replicas {})", name(i), i["spec"]["replicas"].as_u64().unwrap_or(1))).collect());
        line("StatefulSets", of("StatefulSet").iter().map(|i| format!("{} (replicas {})", name(i), i["spec"]["replicas"].as_u64().unwrap_or(1))).collect());
        line("Services", of("Service").iter().map(|i| format!("{} ({})", name(i), i["spec"]["type"].as_str().unwrap_or("ClusterIP"))).collect());
        line("Ingresses", of("Ingress").iter().map(|i| name(i)).collect());
        line("Pods", of("Pod").iter().map(|i| format!("{} ({})", name(i), i["status"]["phase"].as_str().unwrap_or("?"))).collect());
        let pvcs = of("PersistentVolumeClaim");
        line("PersistentVolumeClaims", pvcs.iter().map(|i| format!("{} ({}, {})", name(i), i["spec"]["resources"]["requests"]["storage"].as_str().unwrap_or("? size"), i["status"]["phase"].as_str().unwrap_or("?"))).collect());
        line("ConfigMaps", of("ConfigMap").iter().map(|i| name(i)).filter(|n| n != "kube-root-ca.crt").collect());
        let sec = kc.read(&["get", "secrets", "-n", &self.ns, "-o", "name"]);
        let secrets: Vec<String> = if sec.ok() { sec.stdout.lines().map(|l| l.trim().trim_start_matches("secret/").to_string()).filter(|l| !l.is_empty()).collect() } else { vec![] };
        line("Secrets, names only", secrets);
        if !pvcs.is_empty() {
            s.push_str("\nWARNING: the data in the PersistentVolumeClaims above (databases, uploads, anything the pods wrote) is DELETED with the namespace and cannot be recovered.\n");
        }
        Ok(s.trim_end().to_string())
    }

    // ---------------------------------------------------------------------------------------------- stop / start

    pub(super) fn stop_env_preview(&self, args: &Value) -> String {
        if let Err(e) = self.teardown_guard() {
            return format!("this would be refused: {e}");
        }
        let start = arg(args, "action") == "start";
        match self.workloads() {
            Ok(w) if w.is_empty() => "No Deployments or StatefulSets in the namespace.".into(),
            Ok(w) => {
                let mut s = format!("{} in namespace {}:\n", if start { "START (scale back to the counts saved before the stop)" } else { "STOP (scale to 0; Services, ConfigMaps and PVC data stay)" }, self.ns);
                for x in w.into_iter().filter(|x| if start { x.replicas == 0 } else { x.replicas > 0 }) {
                    s.push_str(&format!("- {} {}: {} → {}\n", x.kind, x.name, x.replicas, if start { x.saved.unwrap_or(1).to_string() } else { "0".to_string() }));
                }
                s.trim_end().to_string()
            }
            Err(e) => format!("could not read the workloads: {e}"),
        }
    }

    pub(super) fn stop_env(&self, args: &Value) -> Result<String, String> {
        self.teardown_guard()?;
        let start = match arg(args, "action") {
            "" | "stop" => false,
            "start" => true,
            o => return Err(format!("`{o}` is not an action (stop or start)")),
        };
        let kc = self.kc();
        let ws = self.workloads()?;
        if ws.is_empty() {
            return Ok("No Deployments or StatefulSets in the namespace; nothing to do.".into());
        }
        let mut done = Vec::new();
        for w in &ws {
            let target = if start {
                if w.replicas > 0 {
                    continue;
                }
                w.saved.unwrap_or(1)
            } else {
                if w.replicas == 0 {
                    continue;
                }
                let n = w.replicas.to_string();
                let a = kc.kubectl(&["annotate", w.kind, &w.name, &format!("{ANNOTATION_REPLICAS}={n}"), "--overwrite", "-n", &self.ns], 8, 30);
                if !a.ok() {
                    return Err(format!("could not remember the replica count of {} {}: {}", w.kind, w.name, first_line(&Self::out_text(&a))));
                }
                0
            };
            let o = kc.kubectl(&["scale", w.kind, &w.name, &format!("--replicas={target}"), "-n", &self.ns], 8, 60);
            if !o.ok() {
                return Err(format!("could not scale {} {}: {}", w.kind, w.name, first_line(&Self::out_text(&o))));
            }
            done.push(format!("{} {}: {} → {target}", w.kind, w.name, w.replicas));
        }
        self.audit(if start { "start" } else { "stop" }, &format!("{} workloads in {}", done.len(), self.ns));
        if done.is_empty() {
            return Ok(if start { "Everything is already running." } else { "Everything is already stopped." }.into());
        }
        Ok(format!("{} namespace {}:\n- {}\n{}", if start { "Started" } else { "Stopped" }, self.ns, done.join("\n- "), if start { "The pods are starting: check with deploy_status." } else { "Services, ConfigMaps, Secrets and PVC data are kept; start it again with k8s_stop_env action=start." }))
    }

    // ---------------------------------------------------------------------------------------------- remove all

    pub(super) fn teardown_preview(&self) -> String {
        if let Err(e) = self.teardown_guard() {
            return format!("this would be refused: {e}");
        }
        match self.inventory() {
            Ok(i) => format!("REMOVE ALL: kubectl delete namespace {} (everything below goes, then IYAKKI waits for it to disappear). To confirm, the namespace name must be typed.\n\n{i}", self.ns),
            Err(e) => format!("could not list the namespace: {e}"),
        }
    }

    pub(super) fn teardown(&self, args: &Value) -> Result<String, String> {
        self.teardown_guard()?;
        if arg(args, "confirm") != self.ns {
            return Err(format!("not removed: the confirmation does not match. Ask the person to TYPE the namespace name `{}` (ask_user), then call k8s_teardown again with confirm set to what they typed. Nothing was changed.", self.ns));
        }
        let inventory = self.inventory().unwrap_or_default();
        let kc = self.kc();
        self.audit("teardown", &format!("delete namespace {}", self.ns));
        let d = kc.kubectl(&["delete", "namespace", &self.ns, "--wait=false"], 10, 60);
        if !d.ok() {
            return Err(format!("kubectl delete namespace failed: {}", first_line(&Self::out_text(&d))));
        }
        let w = kc.kubectl(&["wait", "--for=delete", &format!("namespace/{}", self.ns), "--timeout=120s"], 130, 140);
        let mut out = String::new();
        if w.ok() || Self::out_text(&w).contains("NotFound") {
            out.push_str(&format!("Namespace {} is gone, with everything in it.\n", self.ns));
        } else {
            out.push_str(&format!("Namespace {} is still Terminating after 120 s.\n{}", self.ns, self.leftovers()));
        }
        out.push_str(&format!("\nIt held:\n{}\n", inventory.lines().skip(1).collect::<Vec<_>>().join("\n")));
        out.push_str(&self.image_offer());
        Ok(out.trim_end().to_string())
    }

    /// Why a namespace stays Terminating, in plain words.
    fn leftovers(&self) -> String {
        let kc = self.kc();
        let mut s = String::new();
        let o = kc.read(&["get", "namespace", &self.ns, "-o", "json"]);
        if o.ok() {
            if let Ok(j) = serde_json::from_str::<Value>(&o.stdout) {
                for c in j["status"]["conditions"].as_array().cloned().unwrap_or_default() {
                    if c["status"].as_str() == Some("True") {
                        s.push_str(&format!("- {}: {}\n", c["type"].as_str().unwrap_or("?"), c["message"].as_str().unwrap_or("")));
                    }
                }
                if let Some(f) = j["spec"]["finalizers"].as_array().filter(|f| !f.is_empty()) {
                    s.push_str(&format!("- waiting on finalizers: {}\n", f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
                }
            }
        }
        let left = kc.read(&["get", "all", "-n", &self.ns, "-o", "name"]);
        if left.ok() && !left.stdout.trim().is_empty() {
            s.push_str(&format!("- still present: {}\n", left.stdout.lines().take(12).collect::<Vec<_>>().join(", ")));
        }
        s.push_str("What to check: `kubectl get namespace ");
        s.push_str(&self.ns);
        s.push_str(" -o yaml` (status.conditions and spec.finalizers), pods stuck Terminating on a node that is down, and a PVC whose volume cannot detach. IYAKKI does not remove finalizers; do not force it unless you know which one is stuck.\n");
        s
    }

    // ---------------------------------------------------------------------------------------------- the registry images

    fn registry_base(&self) -> Option<String> {
        let (push, _, kind) = self.saved_registry();
        (!push.is_empty() && kind != "none").then_some(push)
    }

    fn curl(&self, args: &[&str]) -> super::Out {
        let mut a: Vec<String> = vec!["curl".into(), "-sS".into(), "--max-time".into(), "8".into()];
        a.extend(args.iter().map(|s| s.to_string()));
        self.run.run(&std::env::temp_dir(), &a, &[], Duration::from_secs(15), 256 * 1024, None)
    }

    /// The repositories under `<namespace>/` in the registry with their tags.
    fn registry_images(&self) -> Result<Vec<(String, Vec<String>)>, String> {
        let reg = self.registry_base().ok_or("no registry is set up for this workspace")?;
        if !self.run.has("curl") {
            return Err("curl is not installed".into());
        }
        let c = self.curl(&["-f", &format!("http://{reg}/v2/_catalog?n=1000")]);
        if !c.ok() {
            return Err(format!("the registry {reg} did not answer: {}", first_line(&Self::out_text(&c))));
        }
        let j: Value = serde_json::from_str(&c.stdout).map_err(|_| "the registry's catalog was not readable".to_string())?;
        let prefix = format!("{}/", self.ns);
        let mut out = Vec::new();
        for r in j["repositories"].as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_str).filter(|r| r.starts_with(&prefix)) {
            let t = self.curl(&["-f", &format!("http://{reg}/v2/{r}/tags/list")]);
            let tags: Vec<String> = serde_json::from_str::<Value>(&t.stdout).ok().and_then(|j| j["tags"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())).unwrap_or_default();
            out.push((r.to_string(), tags));
        }
        Ok(out)
    }

    fn image_offer(&self) -> String {
        match self.registry_images() {
            Ok(v) if v.is_empty() => "\nNo images for this workspace are in the registry.".into(),
            Ok(v) => format!("\nImages pushed for this workspace stay in the registry: {}. Removing them is a separate, optional step that asks: k8s_registry_cleanup.", v.iter().map(|(r, t)| format!("{r} ({})", t.join(", "))).collect::<Vec<_>>().join("; ")),
            Err(_) => String::new(),
        }
    }

    pub(super) fn registry_cleanup_preview(&self) -> String {
        match self.registry_images() {
            Ok(v) if v.is_empty() => "No images for this workspace are in the registry.".into(),
            Ok(v) => format!("Delete these images from the registry {}:\n{}\nThe registry must allow deletion (REGISTRY_STORAGE_DELETE_ENABLED=true; the in-cluster registry IYAKKI sets up does). Disk space is only freed when the registry runs garbage collection.", self.registry_base().unwrap_or_default(), v.iter().map(|(r, t)| format!("- {r}: {}", t.join(", "))).collect::<Vec<_>>().join("\n")),
            Err(e) => format!("this would be refused: {e}"),
        }
    }

    pub(super) fn registry_cleanup(&self, _args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let reg = self.registry_base().ok_or("no registry is set up for this workspace")?;
        let images = self.registry_images()?;
        if images.is_empty() {
            return Ok("No images for this workspace are in the registry.".into());
        }
        let (mut deleted, mut notes) = (0, Vec::new());
        for (repo, tags) in &images {
            let mut digests: Vec<String> = Vec::new();
            for t in tags {
                let h = self.curl(&["-I", "-H", "Accept: application/vnd.docker.distribution.manifest.v2+json", "-H", "Accept: application/vnd.oci.image.manifest.v1+json", &format!("http://{reg}/v2/{repo}/manifests/{t}")]);
                if let Some(d) = h.stdout.lines().find_map(|l| l.to_lowercase().strip_prefix("docker-content-digest:").map(|d| d.trim().to_string())) {
                    if !digests.contains(&d) {
                        digests.push(d);
                    }
                }
            }
            for d in digests {
                let o = self.curl(&["-o", "/dev/null", "-w", "%{http_code}", "-X", "DELETE", &format!("http://{reg}/v2/{repo}/manifests/{d}")]);
                match o.stdout.trim() {
                    "202" => deleted += 1,
                    "405" => return Err(format!("the registry {reg} does not allow deleting images. To allow it, start the registry with REGISTRY_STORAGE_DELETE_ENABLED=true (the in-cluster registry from Registry… A has it on; a registry you run yourself needs the setting). IYAKKI does not change a registry's settings. Nothing was deleted.")),
                    code => notes.push(format!("{repo}@{d}: HTTP {code}")),
                }
            }
        }
        self.audit("registry_cleanup", &format!("{deleted} manifests under {}/", self.ns));
        Ok(format!("Deleted {deleted} image manifests under {}/ from {reg}.{}\nThe registry only frees the disk space when it runs garbage collection (`registry garbage-collect`).", self.ns, if notes.is_empty() { String::new() } else { format!("\nCould not delete: {}", notes.join("; ")) }))
    }
}
