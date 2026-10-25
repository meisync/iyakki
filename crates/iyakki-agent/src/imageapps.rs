//! Image apps: ready-made container-image apps the person can run in the dev cluster (`docs/LINUX-APPS.md`, `docs/BUILD-YOUR-OWN-IMAGE-APP.md`).
//!
//! A catalog entry is a small JSON file (`image-apps/*.json` in the repository, `<data>/image-apps/*.json` for the person's own):
//! name, description, the image **pinned by digest**, ports, environment, volumes, licence. This replaces the app-store idea the owner
//! dropped on 2026-10-21. Nothing is downloaded by IYAKKI here: "Run" writes a Deployment and a Service for the namespace
//! `iyakki-apps` and hands them to the dev cluster's managed `kubectl`; the cluster pulls the image.
//!
//! * A reference without `@sha256:<64 hex>` is refused when the file is read (a tag can move; a digest cannot).
//! * A digest of all zeros is a placeholder: the entry is listed but cannot run until `dist/release/pin-image-apps.sh` has filled the real one.
//! * The pods get no service account token, no extra privileges, all capabilities dropped, the runtime's default seccomp profile and
//!   resource limits.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::wsexec::Out;

pub const NAMESPACE: &str = "iyakki-apps";

/// The entries that come with IYAKKI (examples; the files are in `image-apps/` of the repository).
pub const EXAMPLES: [(&str, &str); 3] = [
    ("busybox-httpd.json", include_str!("../../../image-apps/busybox-httpd.json")),
    ("python-http-server.json", include_str!("../../../image-apps/python-http-server.json")),
    ("whoami.json", include_str!("../../../image-apps/whoami.json")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A container image run by the dev cluster.
    Container,
    /// A plain root file system (tarball) run in the Linux apps sandbox runtime: parsed and shown; running it is planned.
    Rootfs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub path: String,
    #[serde(default = "default_vol_mb")]
    pub size_mb: u32,
}

fn default_vol_mb() -> u32 {
    64
}

fn default_kind() -> Kind {
    Kind::Container
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageApp {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Shown on the card as an example (not a recommendation).
    #[serde(default)]
    pub example: bool,
    #[serde(default = "default_kind")]
    pub kind: Kind,
    /// `registry/repo[:tag]@sha256:<64 hex>`
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub ports: Vec<Port>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    pub licence: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub run_as_user: Option<u32>,
    #[serde(default)]
    pub memory_mb: Option<u32>,
    #[serde(default)]
    pub cpu_milli: Option<u32>,
    /// `kind: rootfs`: where the tarball is and its SHA-256.
    #[serde(default)]
    pub rootfs_url: String,
    #[serde(default)]
    pub rootfs_sha256: String,
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// An image reference must end in `@sha256:<64 hex>`; a tag alone is refused.
pub fn validate_image_ref(r: &str) -> Result<(), String> {
    if r.is_empty() || r.len() > 300 || !r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@')) {
        return Err(format!("`{r}` is not an image reference"));
    }
    let Some((name, digest)) = r.rsplit_once('@') else {
        return Err(format!("`{r}` has no digest: pin the image as name@sha256:<64 hex> (a tag can change under you)"));
    };
    if !digest.strip_prefix("sha256:").is_some_and(hex64) {
        return Err(format!("`{r}`: the digest must be sha256: followed by 64 lower-case hex digits"));
    }
    if name.is_empty() || name.starts_with(['/', '-', ':']) || name.ends_with(['/', ':']) || name.contains("//") || name.contains("..") {
        return Err(format!("`{r}` has an unusable name"));
    }
    Ok(())
}

impl ImageApp {
    /// The digest part of the image reference.
    pub fn digest(&self) -> &str {
        self.image.rsplit_once('@').map_or("", |x| x.1)
    }

    /// False while the digest is the all-zero placeholder of an example not pinned yet.
    pub fn pinned(&self) -> bool {
        self.kind == Kind::Container && validate_image_ref(&self.image).is_ok() && self.digest().strip_prefix("sha256:").is_some_and(|h| h.chars().any(|c| c != '0'))
    }

    pub fn validate(&self) -> Result<(), String> {
        let bad = |m: &str| Err(format!("{}: {m}", if self.id.is_empty() { "an image app" } else { &self.id }));
        if self.id.is_empty() || self.id.len() > 40 || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || self.id.starts_with('-') || self.id.ends_with('-') {
            return bad("the id must be lower-case letters, digits and -");
        }
        if self.name.trim().is_empty() || self.name.len() > 80 || self.description.len() > 600 {
            return bad("the name (up to 80) or description (up to 600 characters) is missing or too long");
        }
        if self.licence.trim().is_empty() || self.licence.len() > 200 {
            return bad("the licence is required (for example \"Apache-2.0\")");
        }
        match self.kind {
            Kind::Container => validate_image_ref(&self.image)?,
            Kind::Rootfs => {
                if !self.rootfs_url.starts_with("https://") || !hex64(&self.rootfs_sha256) {
                    return bad("a rootfs entry needs an https rootfs_url and its rootfs_sha256");
                }
            }
        }
        if self.ports.len() > 5 || self.ports.iter().any(|p| p.port == 0 || p.name.is_empty() || p.name.len() > 15 || !p.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')) {
            return bad("up to 5 ports, each with a short lower-case name and a number from 1 to 65535");
        }
        if self.env.len() > 20 || self.env.iter().any(|e| e.name.is_empty() || e.name.len() > 64 || !e.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || e.name.starts_with(|c: char| c.is_ascii_digit()) || e.value.len() > 512 || e.value.contains('\0')) {
            return bad("up to 20 environment variables with plain names");
        }
        if self.volumes.len() > 3 || self.volumes.iter().any(|v| v.name.is_empty() || v.name.len() > 30 || !v.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || !v.path.starts_with('/') || v.path.contains("..") || v.path.contains('\0') || v.path.len() > 200 || v.size_mb == 0 || v.size_mb > 4096) {
            return bad("up to 3 volumes with an absolute path and a size of 1 to 4096 MB");
        }
        if self.command.len() > 16 || self.args.len() > 32 || self.command.iter().chain(&self.args).any(|a| a.len() > 400 || a.contains('\0')) {
            return bad("the command or its arguments are too long");
        }
        if self.homepage.len() > 200 || self.memory_mb.is_some_and(|m| m == 0 || m > 8192) || self.cpu_milli.is_some_and(|c| c == 0 || c > 4000) {
            return bad("the homepage or the resource limits are out of range");
        }
        Ok(())
    }
}

/// Read one catalog file's text.
pub fn parse(json: &str) -> Result<ImageApp, String> {
    let app: ImageApp = serde_json::from_str(json).map_err(|e| format!("not a valid image app file: {e}"))?;
    app.validate()?;
    Ok(app)
}

/// The folders searched for the person's own entries.
pub fn dirs(data: &Path) -> Vec<PathBuf> {
    vec![data.join("image-apps"), PathBuf::from("/usr/share/iyakki/image-apps")]
}

/// All entries: the examples, then the files of the folders. A file that does not pass is reported, not listed.
pub fn catalog(dirs: &[PathBuf]) -> (Vec<ImageApp>, Vec<String>) {
    let (mut apps, mut problems): (Vec<ImageApp>, Vec<String>) = (Vec::new(), Vec::new());
    for (file, text) in EXAMPLES {
        match parse(text) {
            Ok(a) => apps.push(a),
            Err(e) => problems.push(format!("{file}: {e}")),
        }
    }
    for d in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::metadata(&f) {
                Ok(m) if m.len() > 64 * 1024 => problems.push(format!("{name}: larger than 64 KB")),
                _ => match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| parse(&t)) {
                    Ok(a) if apps.iter().any(|x| x.id == a.id) => problems.push(format!("{name}: the id {} is listed already", a.id)),
                    Ok(a) => apps.push(a),
                    Err(e) => problems.push(format!("{name}: {e}")),
                },
            }
        }
    }
    (apps, problems)
}

fn labels(app: &ImageApp) -> Value {
    json!({"app.kubernetes.io/name": app.id, "app.kubernetes.io/managed-by": "iyakki", "iyakki.dev/image-app": app.id})
}

/// The Namespace, Deployment and Service to apply, as JSON documents in a `List`.
pub fn manifests(app: &ImageApp) -> Result<Value, String> {
    app.validate()?;
    if app.kind != Kind::Container {
        return Err(format!("{} is a root file system, not a container image: running those from the catalog is planned (import your own tarball into a sandbox instead)", app.id));
    }
    if !app.pinned() {
        return Err(format!("{} is not pinned yet: its image digest is a placeholder. Run dist/release/pin-image-apps.sh, or put the real digest in the file", app.id));
    }
    let mb = app.memory_mb.unwrap_or(256);
    let cpu = app.cpu_milli.unwrap_or(500);
    let mut container = json!({
        "name": "app",
        "image": app.image,
        "imagePullPolicy": "IfNotPresent",
        "securityContext": {"allowPrivilegeEscalation": false, "privileged": false, "capabilities": {"drop": ["ALL"]}, "readOnlyRootFilesystem": !app.volumes.is_empty()},
        "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": format!("{cpu}m"), "memory": format!("{mb}Mi")}},
    });
    // a read-only root needs somewhere to write: the volumes (and /tmp)
    let mut volumes: Vec<Value> = app.volumes.iter().map(|v| json!({"name": v.name, "emptyDir": {"sizeLimit": format!("{}Mi", v.size_mb)}})).collect();
    let mut mounts: Vec<Value> = app.volumes.iter().map(|v| json!({"name": v.name, "mountPath": v.path})).collect();
    if container["securityContext"]["readOnlyRootFilesystem"] == json!(true) {
        volumes.push(json!({"name": "tmp", "emptyDir": {"sizeLimit": "16Mi"}}));
        mounts.push(json!({"name": "tmp", "mountPath": "/tmp"}));
    }
    container["volumeMounts"] = Value::Array(mounts);
    if !app.command.is_empty() {
        container["command"] = json!(app.command);
    }
    if !app.args.is_empty() {
        container["args"] = json!(app.args);
    }
    if !app.ports.is_empty() {
        container["ports"] = Value::Array(app.ports.iter().map(|p| json!({"name": p.name, "containerPort": p.port})).collect());
    }
    if !app.env.is_empty() {
        container["env"] = Value::Array(app.env.iter().map(|e| json!({"name": e.name, "value": e.value})).collect());
    }
    let mut pod_sec = json!({"seccompProfile": {"type": "RuntimeDefault"}});
    if let Some(u) = app.run_as_user {
        pod_sec["runAsNonRoot"] = json!(u != 0);
        pod_sec["runAsUser"] = json!(u);
    }
    let l = labels(app);
    let mut items = vec![
        json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE, "labels": {"app.kubernetes.io/managed-by": "iyakki", "pod-security.kubernetes.io/warn": "restricted"}}}),
        json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": app.id, "namespace": NAMESPACE, "labels": l},
            "spec": {
                "replicas": 1,
                "selector": {"matchLabels": {"iyakki.dev/image-app": app.id}},
                "template": {
                    "metadata": {"labels": l},
                    "spec": {"automountServiceAccountToken": false, "enableServiceLinks": false, "securityContext": pod_sec, "containers": [container], "volumes": volumes}
                }
            }
        }),
    ];
    if !app.ports.is_empty() {
        items.push(json!({
            "apiVersion": "v1", "kind": "Service",
            "metadata": {"name": app.id, "namespace": NAMESPACE, "labels": l},
            "spec": {"type": "NodePort", "selector": {"iyakki.dev/image-app": app.id}, "ports": app.ports.iter().map(|p| json!({"name": p.name, "port": p.port, "targetPort": p.port})).collect::<Vec<_>>()}
        }));
    }
    Ok(json!({"apiVersion": "v1", "kind": "List", "items": items}))
}

/// Where the manifest of an app is written for `kubectl apply -f`.
pub fn manifest_path(data: &Path, id: &str) -> PathBuf {
    data.join(crate::linuxapps::DIR).join("image-apps").join(format!("{id}.json"))
}

/// What `kubectl` is asked: the managed cluster's, as `(arguments) -> output`.
pub type Kubectl<'a> = &'a dyn Fn(&[String]) -> Out;

fn words(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Deploy the app (apply its manifests). Returns a sentence for the page.
pub fn run(app: &ImageApp, data: &Path, kubectl: Kubectl) -> Result<String, String> {
    let m = manifests(app)?;
    let path = manifest_path(data, &app.id);
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    std::fs::write(&path, serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))?;
    let o = kubectl(&words(&["apply", "-f", &path.display().to_string()]));
    if !o.ok() {
        return Err(format!("the cluster refused it: {}", o.combined().trim()));
    }
    Ok(format!("{} was applied to the namespace {NAMESPACE}; the cluster pulls {} (pinned by digest) and starts it", app.name, app.image))
}

/// Remove the app's Deployment and Service.
pub fn stop(app: &ImageApp, kubectl: Kubectl) -> Result<String, String> {
    if app.id.is_empty() || !app.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("not an app id".into());
    }
    let o = kubectl(&words(&["delete", "deployment,service", "-n", NAMESPACE, "-l", &format!("iyakki.dev/image-app={}", app.id), "--ignore-not-found"]));
    if o.ok() { Ok(format!("{} was removed from the cluster", app.name)) } else { Err(format!("could not remove it: {}", o.combined().trim())) }
}

/// A line about the app's state (`running on port 31456`, `not running`).
pub fn status(app: &ImageApp, kubectl: Kubectl) -> String {
    let d = kubectl(&words(&["get", "deployment", &app.id, "-n", NAMESPACE, "-o", "jsonpath={.status.readyReplicas}"]));
    if !d.ok() {
        return "not running".into();
    }
    let ready = d.stdout.trim().parse::<u32>().unwrap_or(0);
    let port = kubectl(&words(&["get", "service", &app.id, "-n", NAMESPACE, "-o", "jsonpath={.spec.ports[0].nodePort}"])).stdout.trim().to_string();
    let at = if port.is_empty() { String::new() } else { format!(", open http://<this machine>:{port}") };
    if ready > 0 { format!("running{at}") } else { format!("starting (the image is being pulled){at}") }
}
