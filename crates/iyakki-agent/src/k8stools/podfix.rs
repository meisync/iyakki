//! Why a pod cannot start, in words the model can act on: `CreateContainerConfigError` (runAsNonRoot with an image that runs as
//! root, a missing ConfigMap or Secret key, an invalid mount) and an `ImagePullBackOff` for an image name nobody built or loaded.
//! Also the static checks `k8s_apply` runs before applying (Dockerfile user, a stale second Deployment, unknown image names) and the
//! one-line `Next:` that `deploy_status` ends with when pods are not ready.

use std::collections::BTreeMap;

use serde_json::Value;

use super::K8sTools;

const USER_FIX: &str = "add `USER 10001` (a number, not a name) to the Dockerfile after installing, `chown` the app files to it, rebuild with deploy_build (a new tag), deploy_load, update the image in the manifest and k8s_apply; or set `runAsUser: 10001` in the manifest's securityContext";

/// Every `image "X"` named in event or describe text.
fn images_in(text: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for key in ["image \"", "image `"] {
        let mut from = 0;
        while let Some(i) = text[from..].find(key) {
            let s = from + i + key.len();
            let img: String = text[s..].chars().take_while(|c| !matches!(c, '"' | '`' | ' ' | '\n')).collect();
            if !img.is_empty() && !v.contains(&img) {
                v.push(img);
            }
            from = s;
        }
    }
    v
}

/// Does the first path part look like a registry host (`localhost:5000`, `reg.lan`, `10.0.0.5:5000`)?
pub fn has_registry_host(image: &str) -> bool {
    let first = image.split('/').next().unwrap_or("");
    image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost")
}

/// The cause and fix for `text` (events / describe / a pod message), or None. `known(image)` says whether the workspace built or
/// loaded that image.
pub fn diagnose_pod(text: &str, known: &dyn Fn(&str) -> bool) -> Option<String> {
    let l = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    if l.contains("runasnonroot") && (l.contains("will run as root") || l.contains("non-numeric user") || l.contains("image has non-numeric")) {
        let why = if l.contains("non-numeric") { "the image's USER is a name, and Kubernetes can only check a number" } else { "the image runs as root (its Dockerfile has no USER)" };
        out.push(format!("The container cannot start: the manifest says `runAsNonRoot: true` but {why}. Fix: {USER_FIX}."));
    }
    if (l.contains("configmap") || l.contains("secret")) && (l.contains("not found") || l.contains("couldn't find key")) && (l.contains("createcontainerconfigerror") || l.contains("couldn't find key") || l.contains("error:")) {
        let line = text.lines().find(|x| { let t = x.to_lowercase(); (t.contains("configmap") || t.contains("secret")) && (t.contains("not found") || t.contains("couldn't find key")) }).unwrap_or("").trim();
        out.push(format!("The container cannot start because a ConfigMap or Secret it uses is missing or has no such key ({}). Fix: create it in the namespace (a ConfigMap in the manifest; a Secret from the vault with vault_request), or correct the name or key in the manifest's env / envFrom / volumes, then k8s_apply again.", super::cut(line, 200)));
    }
    if l.contains("invalid volumemount") || l.contains("failed to create containerd task") && l.contains("mount") || l.contains("volumemount") && l.contains("not found") {
        out.push("The container cannot start because of a volume mount: every volumeMounts entry needs a volume with the same name in `volumes`, a mountPath that is an absolute path, and (for a file) a subPath that exists. Fix the manifest and k8s_apply again.".into());
    }
    if l.contains("imagepullbackoff") || l.contains("errimagepull") || l.contains("failed to pull") || l.contains("back-off pulling") {
        let unknown: Vec<String> = images_in(text).into_iter().filter(|i| !has_registry_host(i) && !known(i)).collect();
        if !unknown.is_empty() {
            let many = unknown.len() > 1;
            out.push(format!("The image{} {} {} no registry address and {} never built or loaded by deploy_build / deploy_load in this workspace, so the node looks for {} on Docker Hub (docker.io/library) and finds nothing. Fix: build it with deploy_build, deploy_load it, and put the exact name deploy_load returned (it starts with the registry address) into the manifest; if a stale file in deploy/ names it, remove that file and delete the stale object with k8s_delete.", if many { "s" } else { "" }, unknown.join(", "), if many { "have" } else { "has" }, if many { "were" } else { "was" }, if many { "them" } else { "it" }));
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Label pairs (`app=web`) of a Deployment's `selector.matchLabels` in manifest text, per Deployment: (name, labels).
pub fn manifest_deployments(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut out = Vec::new();
    for doc in text.split("\n---") {
        if !doc.lines().any(|l| l.trim_end() == "kind: Deployment") {
            continue;
        }
        let mut name = String::new();
        let mut in_meta = false;
        let mut labels = BTreeMap::new();
        let mut in_labels = false;
        let mut indent = 0;
        for l in doc.lines() {
            let t = l.trim();
            let ind = l.len() - l.trim_start().len();
            if l.starts_with("metadata:") {
                in_meta = true;
            } else if in_meta && name.is_empty() && ind > 0 && t.starts_with("name:") {
                name = t[5..].trim().trim_matches(['"', '\'']).to_string();
                in_meta = false;
            }
            if t == "matchLabels:" {
                in_labels = true;
                indent = ind;
                continue;
            }
            if in_labels {
                if ind > indent && t.contains(':') {
                    let (k, v) = t.split_once(':').expect("checked");
                    labels.insert(k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string());
                } else if !t.is_empty() {
                    in_labels = false;
                }
            }
        }
        if !name.is_empty() {
            out.push((name, labels));
        }
    }
    out
}

impl K8sTools {
    /// Did the workspace build or load this image reference?
    fn image_known(&self, img: &str) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let bare = img.strip_prefix("localhost/").unwrap_or(img);
        st.loaded.iter().any(|(k, v)| k == img || v == img || k.strip_prefix("localhost/") == Some(bare)) || st.built.iter().any(|b| b == img || b.strip_prefix("localhost/") == Some(bare))
    }

    /// The cause and fix for pod startup errors in `text`.
    pub(super) fn pod_diagnosis(&self, text: &str) -> Option<String> {
        diagnose_pod(text, &|i| self.image_known(i))
    }

    /// Warnings before an apply: the Dockerfile's user against runAsNonRoot, a second Deployment for the same pods, image names
    /// nobody built or loaded.
    pub(super) fn apply_warnings(&self, text: &str, images: &[String]) -> Vec<String> {
        let mut w = Vec::new();
        let non_root = text.lines().any(|l| l.trim() == "runAsNonRoot: true");
        let has_uid = text.lines().any(|l| l.trim().starts_with("runAsUser:"));
        for img in images {
            let tail = img.rsplit('/').next().unwrap_or(img);
            let base = tail.split([':', '@']).next().unwrap_or(tail);
            let df = crate::dockerlint::find_dockerfile(&self.root, base.trim_end_matches("-image"));
            if non_root && !has_uid {
                if let Some(dfp) = &df {
                    if let Ok(t) = std::fs::read_to_string(dfp) {
                        let rel = dfp.strip_prefix(&self.root).unwrap_or(dfp).display().to_string();
                        match crate::dockerlint::final_user(&t) {
                            None => w.push(format!("{img}: the manifest sets runAsNonRoot: true but {rel} has no USER, so the image runs as root and the pod will fail with CreateContainerConfigError. Fix: {USER_FIX}.")),
                            Some(u) if !crate::dockerlint::numeric_nonroot(&u) => w.push(format!("{img}: the manifest sets runAsNonRoot: true but {rel} has `USER {u}` (root or a name), which Kubernetes cannot verify. Fix: {USER_FIX}.")),
                            _ => {}
                        }
                    }
                }
            }
            if !has_registry_host(img) && !self.image_known(img) && (df.is_some() || crate::dockerlint::find_dockerfile(&self.root, base).is_some()) {
                let known: Vec<String> = {
                    let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.loaded.values().cloned().chain(st.built.iter().cloned()).collect()
                };
                w.push(format!("{img}: this image name has no registry address and was not returned by deploy_build / deploy_load in this session (known: {}). If it is your app, build and load it and use the name deploy_load returns; a public image such as nginx is fine.", if known.is_empty() { "none yet".to_string() } else { known.join(", ") }));
            }
        }
        // a second Deployment in the namespace for the same pods (a stale manifest)
        let new = manifest_deployments(text);
        if !new.is_empty() {
            let o = self.kc().read(&["get", "deployments", "-n", &self.ns, "-o", "json"]);
            if o.ok() {
                if let Some(items) = serde_json::from_str::<Value>(&o.stdout).ok().and_then(|j| j["items"].as_array().cloned()) {
                    for it in items {
                        let name = it["metadata"]["name"].as_str().unwrap_or("").to_string();
                        let labels: BTreeMap<String, String> = it["spec"]["selector"]["matchLabels"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
                        for (nn, nl) in &new {
                            if *nn != name && !name.is_empty() && labels.iter().any(|(k, v)| nl.get(k) == Some(v)) {
                                let files = self.manifest_files_naming(&name);
                                w.push(format!("Deployment `{name}` already runs in {} with the same pod labels ({}) as `{nn}` that you are applying: two Deployments will fight over one app (a stale manifest?). If `{name}` is stale, delete it with k8s_delete kind=deployment name={name}{}.", self.ns, labels.iter().filter(|(k, v)| nl.get(*k) == Some(*v)).map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", "), if files.is_empty() { String::new() } else { format!(" and remove the stale file{} {}", if files.len() > 1 { "s" } else { "" }, files.join(", ")) }));
                            }
                        }
                    }
                }
            }
        }
        w
    }

    /// Workspace manifests (deploy/, k8s/) that mention a Deployment of that name.
    fn manifest_files_naming(&self, name: &str) -> Vec<String> {
        let mut v = Vec::new();
        for d in ["deploy", "k8s"] {
            if let Ok(rd) = std::fs::read_dir(self.root.join(d)) {
                for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "yaml" || x == "yml")) {
                    if std::fs::read_to_string(e.path()).is_ok_and(|t| manifest_deployments(&t).iter().any(|x| x.0 == name)) {
                        v.push(format!("{d}/{}", e.file_name().to_string_lossy()));
                    }
                }
            }
        }
        v.sort();
        v
    }

    /// One concrete action for the model when pods are not ready (`None`: everything is ready).
    pub(super) fn next_action(&self, snap: &crate::k8s::Snapshot, diagnosis: &str) -> Option<String> {
        let bad: Vec<&crate::k8s::Pod> = snap.pods.iter().filter(|p| p.phase != "Succeeded" && (p.ready.split('/').next() != p.ready.split('/').nth(1) || !p.reason.is_empty())).collect();
        let notready_w = snap.workloads.iter().any(|w| w.ready < w.desired);
        if bad.is_empty() && !notready_w {
            return None;
        }
        let d = diagnosis.to_lowercase();
        let pod = bad.first().map(|p| p.name.clone()).unwrap_or_default();
        let reason = bad.iter().map(|p| p.reason.as_str()).find(|r| !r.is_empty()).unwrap_or("");
        Some(if d.contains("runasnonroot") {
            "Next: add `USER 10001` to the Dockerfile (chown the app files), deploy_build with a new tag, deploy_load, put the returned image name in the manifest and k8s_apply; then deploy_status again.".to_string()
        } else if d.contains("never built or loaded") {
            "Next: deploy_build the app, deploy_load it, replace the image in the manifest with the exact name deploy_load returned (and delete the stale Deployment/file named in the Diagnosis), then k8s_apply.".to_string()
        } else if d.contains("configmap or secret") {
            "Next: create the missing ConfigMap/Secret (or fix its name/key in the manifest) and k8s_apply again.".to_string()
        } else {
            match reason {
                "ImagePullBackOff" | "ErrImagePull" | "InvalidImageName" => "Next: read the Diagnosis; check the image name is the one deploy_load returned and that the registry is reachable (registry_setup_help), fix the manifest and k8s_apply.".to_string(),
                "CrashLoopBackOff" | "Error" | "OOMKilled" => format!("Next: k8s_logs pod={pod} previous=true to see why it exits; fix the CMD, env, port or memory limit, rebuild with a new tag and redeploy."),
                "CreateContainerConfigError" | "CreateContainerError" => format!("Next: k8s_describe kind=pod name={pod} and read the container's error; fix the manifest's env, volumes or securityContext and k8s_apply."),
                _ if bad.iter().any(|p| p.phase == "Pending") => format!("Next: k8s_describe kind=pod name={pod}: Pending usually means the requests are larger than any node offers or the image is still pulling; lower the requests or wait and run deploy_status once more."),
                _ => "Next: wait a few seconds and run deploy_status again; if the pods stay not Ready, read k8s_events and k8s_logs.".to_string(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_errors_get_a_cause_and_a_fix() {
        let none = |_: &str| false;
        let root = diagnose_pod("Warning  Failed  kubelet  Error: container has runAsNonRoot and image will run as root (pod: \"a\", container: \"a\")", &none).unwrap();
        assert!(root.contains("runAsNonRoot: true") && root.contains("no USER") && root.contains("USER 10001") && root.contains("runAsUser: 10001") && root.contains("new tag") || root.contains("deploy_build"), "{root}");
        let named = diagnose_pod("Error: container has runAsNonRoot and image has non-numeric user (node), cannot verify user is non-root", &none).unwrap();
        assert!(named.contains("a name") && named.contains("USER 10001"), "{named}");
        let cm = diagnose_pod("Error: configmap \"app-config\" not found", &none).unwrap();
        assert!(cm.contains("ConfigMap or Secret") && cm.contains("app-config") && cm.contains("vault_request"), "{cm}");
        let key = diagnose_pod("Error: couldn't find key DB_URL in Secret shop-dev/db", &none).unwrap();
        assert!(key.contains("DB_URL"), "{key}");
        assert!(diagnose_pod("Error: failed to create containerd task: invalid VolumeMount x", &none).unwrap().contains("volumeMounts"));
        assert!(diagnose_pod("everything is fine", &none).is_none());
    }

    #[test]
    fn a_pull_error_for_a_name_nobody_built_says_so_but_loaded_and_registry_names_are_left_to_the_registry_diagnosis() {
        let text = "Warning Failed  Failed to pull image \"student-attendance-image\": rpc error: pull access denied\nWarning Failed Error: ErrImagePull\nNormal BackOff Back-off pulling image \"localhost:5000/student-attendance:abc\"";
        let d = diagnose_pod(text, &|_| false).unwrap();
        assert!(d.contains("student-attendance-image has no registry address") && d.contains("never built or loaded") && d.contains("Docker Hub") && d.contains("k8s_delete") && !d.contains("localhost:5000/student-attendance:abc has"), "{d}");
        assert!(diagnose_pod(text, &|i| i == "student-attendance-image").is_none(), "a loaded image is not blamed");
        assert!(diagnose_pod("Failed to pull image \"reg.lan:5000/a:1\" ImagePullBackOff", &|_| false).is_none());
        assert!(has_registry_host("localhost:30500/ns/web:1") && has_registry_host("reg.lan/a") && !has_registry_host("web:1") && !has_registry_host("library/web"));
    }

    #[test]
    fn deployments_and_their_selectors_are_read_from_a_manifest() {
        let m = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  labels:\n    app: web\nspec:\n  selector:\n    matchLabels:\n      app: web\n      tier: front\n  template:\n    metadata:\n      labels:\n        app: web\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: web\n";
        let d = manifest_deployments(m);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, "web");
        assert_eq!(d[0].1.get("app").map(String::as_str), Some("web"));
        assert_eq!(d[0].1.get("tier").map(String::as_str), Some("front"));
    }
}
