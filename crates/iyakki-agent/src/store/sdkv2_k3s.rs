//! The `iyakki.k3s` backend of the SDK 1.2 bridge (F-SDK-32): a cluster namespace of an app's own on the person's managed k3s, behind the bridge's
//! checks (`sdkv2.rs`: the app's `k3s` capability `read` or `manage`, the cluster rules, the plan shown to the person and asked every time).
//!
//! * **One namespace per app**, `app-<id>` (letters and digits, at most 63 characters), made on the first change with the label
//!   `iyakki.app=<id>`, a ResourceQuota (10 pods, 2 CPUs and 2 GiB of requests) and a default-deny ingress NetworkPolicy, so an app cannot fill
//!   the cluster and no other namespace reaches into it. Every kubectl call of this backend carries `-n app-<id>`; an app never names another.
//! * **The plan** is the deploy lint (`iyakki-lint::deploylint`, the same one the K8S tile uses before it applies anything): privileged
//!   containers, host paths and host networking, added capabilities, root users, cluster-scoped kinds, secrets with literal values and any
//!   namespace but the app's are refused (`refused`), the rest is listed (`lint`). What is applied is the lint's own text (its rewrites included),
//!   never the app's.
//! * **Reads** (`status`, `get`, `logs`) work on the app's namespace only. A cluster that is not running answers `Unavailable` in words that say how
//!   to start it; nothing is installed or started from here.
//! * The kubectl seam is [`KubePort`]; [`RealKube`] is the managed cluster's `K3sDev::kubectl_cap`, tests use a fake that records the calls.

use std::sync::Arc;

use serde_json::{json, Value};

use super::sdkv2_backends::{Backend, Who};
use super::tilesdk::{code, SdkError};

/// What the namespace of one app may hold (the quota the first change puts in place).
pub const QUOTA_PODS: u32 = 10;
pub const QUOTA_CPU: &str = "2";
pub const QUOTA_MEMORY: &str = "2Gi";

/// The most a `kubectl get` answer is read (the bridge cuts the items itself).
const READ_CAP: usize = 1_000_000;

pub trait KubePort: Send + Sync {
    /// Whether the managed cluster is up (its API answers).
    fn running(&self) -> bool;
    /// One kubectl call; the answer is its standard output, the error its plain explanation.
    fn kubectl(&self, args: &[String]) -> Result<String, String>;
    /// A manifest file for `kubectl apply -f` (the real port writes it under the data folder; the file is the person's, mode 600).
    fn write_manifest(&self, app: &str, text: &str) -> Result<String, String>;
    /// Whether an image was loaded into the local cluster (the lint then keeps it from being pulled).
    fn is_local_image(&self, image: &str) -> bool;
}

/// The namespace of an app: `app-` and its id with everything that is not a letter or digit turned into `-`, at most 63 characters, never ending in `-`.
pub fn namespace_of(app: &str) -> String {
    let id: String = app.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut ns = format!("app-{}", id.trim_matches('-'));
    ns.truncate(63);
    ns.trim_end_matches('-').to_string()
}

fn unavailable(why: &str) -> SdkError {
    SdkError::new(code::UNAVAILABLE, why.to_string()).with("fix", "Install and start the cluster from the Store, then try again")
}

fn failed(msg: String) -> SdkError {
    SdkError::new(code::FAILED, msg)
}

fn s<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

pub struct K3sEngine {
    pub kube: Arc<dyn KubePort>,
}

impl K3sEngine {
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn up(&self) -> Result<(), SdkError> {
        if self.kube.running() {
            Ok(())
        } else {
            Err(unavailable("the cluster is not running on this computer"))
        }
    }

    fn namespace_exists(&self, ns: &str) -> Result<bool, SdkError> {
        match self.kube.kubectl(&Self::args(&["get", "namespace", ns, "-o", "name"])) {
            Ok(_) => Ok(true),
            Err(e) if e.contains("NotFound") || e.contains("not found") => Ok(false),
            Err(e) => Err(failed(e)),
        }
    }

    /// The namespace with its label, quota and default-deny ingress, made when it is not there; only a change makes one.
    fn ensure(&self, app: &str, ns: &str) -> Result<(), SdkError> {
        if self.namespace_exists(ns)? {
            return Ok(());
        }
        let id: String = app.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(63).collect();
        let text = format!(
            "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {ns}\n  labels:\n    iyakki.app: \"{id}\"\n---\napiVersion: v1\nkind: ResourceQuota\nmetadata:\n  name: app-quota\n  namespace: {ns}\nspec:\n  hard:\n    pods: \"{QUOTA_PODS}\"\n    requests.cpu: \"{QUOTA_CPU}\"\n    requests.memory: {QUOTA_MEMORY}\n---\napiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: default-deny-ingress\n  namespace: {ns}\nspec:\n  podSelector: {{}}\n  policyTypes:\n    - Ingress\n"
        );
        let file = self.kube.write_manifest(&format!("{app}-namespace"), &text).map_err(failed)?;
        self.kube.kubectl(&Self::args(&["apply", "-f", &file])).map(|_| ()).map_err(failed)
    }

    fn lint(&self, ns: &str, yaml: &str) -> crate::deploylint::Lint {
        crate::deploylint::lint(yaml, ns, &|i| self.kube.is_local_image(i))
    }
}

impl Backend for K3sEngine {
    fn call(&self, who: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        let ns = namespace_of(&who.app);
        match op {
            "k3s.status" => {
                let ready = self.kube.running() && self.namespace_exists(&ns).unwrap_or(false);
                Ok(json!({"cluster": if self.kube.running() { "this computer" } else { "not running" }, "namespace": ns, "ready": ready}))
            }
            "k3s.get" => {
                self.up()?;
                if !self.namespace_exists(&ns)? {
                    return Ok(json!({"items": []}));
                }
                let mut args = vec!["get".to_string(), s(a, "kind").to_string()];
                if let Some(n) = a.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    args.push(n.to_string());
                }
                if let Some(sel) = a.get("selector").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    args.extend(["-l".to_string(), sel.to_string()]);
                }
                args.extend(["-n".to_string(), ns, "-o".to_string(), "json".to_string()]);
                let out = self.kube.kubectl(&args).map_err(failed)?;
                let v: Value = serde_json::from_str(&out).map_err(|_| failed("the cluster's answer was not readable".into()))?;
                // one named object is a list of one; a list keeps its items
                Ok(if v.get("items").is_some() { v } else { json!({"items": [v]}) })
            }
            "k3s.logs" => {
                self.up()?;
                let args = vec!["logs".to_string(), s(a, "pod").to_string(), "-n".to_string(), ns, format!("--tail={}", a.get("tail").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500))];
                let out = self.kube.kubectl(&args).map_err(failed)?;
                Ok(json!({"lines": out.lines().map(str::to_string).collect::<Vec<_>>()}))
            }
            "k3s.plan" => {
                // a plan needs no running cluster: it only reads the manifest with the cluster rules
                let l = self.lint(&ns, s(a, "yaml"));
                let objects: Vec<String> = l.objects.iter().map(|(k, n)| format!("{k} {n}")).collect();
                let plan = if l.ok() { format!("{} object(s) in {ns}: {}{}", objects.len(), objects.join(", "), if l.images.is_empty() { String::new() } else { format!("\nimages: {}", l.images.join(", ")) }) } else { "nothing would be applied".to_string() };
                let mut lint = l.warnings.clone();
                lint.extend(l.notes.iter().map(|n| format!("changed: {n}")));
                Ok(json!({"plan": plan, "lint": lint, "refused": l.errors}))
            }
            "k3s.apply" => {
                self.up()?;
                let l = self.lint(&ns, s(a, "yaml"));
                if !l.ok() {
                    return Err(SdkError::new(code::BLOCKED, format!("the cluster rules refuse this: {}", l.errors.iter().take(3).cloned().collect::<Vec<_>>().join("; "))));
                }
                self.ensure(&who.app, &ns)?;
                let file = self.kube.write_manifest(&who.app, &l.text).map_err(failed)?;
                self.kube.kubectl(&["apply".to_string(), "-f".to_string(), file, "-n".to_string(), ns]).map_err(failed)?;
                Ok(json!({"applied": l.objects.len()}))
            }
            "k3s.restart" => {
                self.up()?;
                let name = s(a, "name").to_string();
                // the workload kinds an app may restart, tried in turn: the first the namespace has by that name
                for kind in ["deployment", "statefulset", "daemonset"] {
                    let target = format!("{kind}/{name}");
                    match self.kube.kubectl(&["rollout".to_string(), "restart".to_string(), target, "-n".to_string(), ns.clone()]) {
                        Ok(_) => return Ok(json!({"restarted": name})),
                        Err(e) if e.contains("NotFound") || e.contains("not found") => continue,
                        Err(e) => return Err(failed(e)),
                    }
                }
                Err(SdkError::new(code::FAILED, format!("there is no workload called {name} in this app's part of the cluster")))
            }
            other => Err(failed(format!("the cluster has no call {other}"))),
        }
    }
}

/// The managed cluster of this computer (`K3sDev`): kubectl through its own door, a manifest file under the data folder.
pub struct RealKube {
    pub dev: Arc<crate::k3sdev::K3sDev>,
    pub dir: std::path::PathBuf,
}

impl KubePort for RealKube {
    fn running(&self) -> bool {
        self.dev.state() == crate::k3sdev::Svc::Running
    }
    fn kubectl(&self, args: &[String]) -> Result<String, String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = self.dev.kubectl_cap(&refs, READ_CAP);
        if o.ok() {
            Ok(o.stdout)
        } else {
            Err(crate::localreg::explain_kubectl(&o.combined()))
        }
    }
    fn write_manifest(&self, app: &str, text: &str) -> Result<String, String> {
        use std::os::unix::fs::PermissionsExt;
        let name: String = app.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(80).collect();
        let dir = self.dir.join("appk3s");
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make the folder for the manifest: {e}"))?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let file = dir.join(format!("{name}.yaml"));
        std::fs::write(&file, text).map_err(|e| format!("could not write the manifest: {e}"))?;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        Ok(file.display().to_string())
    }
    fn is_local_image(&self, _image: &str) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "sdkv2_k3s_tests.rs"]
mod tests;
