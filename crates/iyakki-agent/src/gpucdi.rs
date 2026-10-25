//! Container Device Interface (CDI) specs, written in-house (F-GPU-01, F-GPU-03; docs/GPU-K3S.md).
//!
//! CDI (<https://github.com/cncf-tags/container-device-interface>, an open specification) is how a container runtime learns which device
//! nodes, library mounts and environment variables a "device" such as `nvidia.com/gpu=0` stands for. containerd reads the JSON specs in
//! `/etc/cdi` and `/var/run/cdi` and applies the edits to a container when a device plugin names the device in its allocation answer
//! (the NVIDIA device plugin does it for `nvidia.com/gpu: 1`). A pod annotation alone is not enough: kubelet does not hand pod annotations
//! to the runtime as container annotations (tried on k3s v1.37: the annotation was ignored), so CDI needs a device plugin, and stage 1
//! (any vendor, Vulkan) uses a generic device plugin that hands out the DRM render nodes without CDI (`gpu_workloads.rs`). Nothing runs on the host and no hook is used:
//! the NVIDIA container toolkit's runtime hook is a glibc program and IYAKKI's host is musl, so this writer produces plain edits only.
//!
//! * [`Spec`] is the document (version 0.6.0), [`Spec::to_json`] its deterministic text (keys sorted, two-space indent) so the tests can
//!   compare with golden files.
//! * [`write_spec`] puts a spec in a spec directory atomically (a temporary name, then rename: containerd watches the directory).
//! * [`dropin_files`] are the containerd configuration drop-ins that switch CDI on in the k3s the cluster step manages. k3s renders its
//!   containerd configuration with `imports` of `config.toml.d/*.toml` (containerd 1.7 style, version 2) and `config-v3.toml.d/*.toml`
//!   (containerd 2.x, version 3); a drop-in survives k3s restarts and upgrades, unlike editing the generated file.

use std::path::{Path, PathBuf};

/// The CDI specification version the documents declare (hooks are not used, so any version from 0.5.0 would do).
pub const CDI_VERSION: &str = "0.6.0";
/// Stage 3: the NVIDIA vendor driver, named as the toolkit and the documentation name it.
pub const NVIDIA_KIND: &str = "nvidia.com/gpu";
/// The same devices under the name the NVIDIA device plugin returns for an allocation when it works with CDI annotations
/// (`k8s.device-plugin.nvidia.com/gpu=<id>`; checked on the real machine, docs/GPU-K3S.md step 14).
pub const PLUGIN_KIND: &str = "k8s.device-plugin.nvidia.com/gpu";
/// Where specs for containerd go: the volatile one (a RAM directory, written again at every start).
pub const SPEC_DIR: &str = "/var/run/cdi";

/// A device node the container gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceNode {
    /// Path inside the container.
    pub path: String,
    /// Path on the host when it differs.
    pub host_path: Option<String>,
    /// `c` or `b`; absent: the runtime reads the host node.
    pub kind: Option<char>,
    pub major: Option<u32>,
    pub minor: Option<u32>,
    pub file_mode: Option<u32>,
    pub permissions: Option<String>,
}

impl DeviceNode {
    /// A node of the host given as it is, readable and writable by every user of the container.
    pub fn open(path: &str) -> DeviceNode {
        DeviceNode { path: path.into(), host_path: None, kind: None, major: None, minor: None, file_mode: Some(0o666), permissions: Some("rw".into()) }
    }
}

/// A bind mount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub host_path: String,
    pub container_path: String,
    pub options: Vec<String>,
}

impl Mount {
    /// A read-only bind mount.
    pub fn ro(host: &str, container: &str) -> Mount {
        Mount { host_path: host.into(), container_path: container.into(), options: vec!["ro".into(), "nosuid".into(), "nodev".into(), "bind".into()] }
    }
}

/// Container edits: what the runtime adds to the container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edits {
    pub env: Vec<String>,
    pub device_nodes: Vec<DeviceNode>,
    pub mounts: Vec<Mount>,
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.device_nodes.is_empty() && self.mounts.is_empty()
    }
    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if !self.env.is_empty() {
            o.insert("env".into(), serde_json::json!(self.env));
        }
        if !self.device_nodes.is_empty() {
            let nodes: Vec<serde_json::Value> = self
                .device_nodes
                .iter()
                .map(|n| {
                    let mut m = serde_json::Map::new();
                    m.insert("path".into(), n.path.clone().into());
                    if let Some(h) = &n.host_path {
                        m.insert("hostPath".into(), h.clone().into());
                    }
                    if let Some(k) = n.kind {
                        m.insert("type".into(), k.to_string().into());
                    }
                    if let Some(v) = n.major {
                        m.insert("major".into(), v.into());
                    }
                    if let Some(v) = n.minor {
                        m.insert("minor".into(), v.into());
                    }
                    if let Some(v) = n.file_mode {
                        m.insert("fileMode".into(), v.into());
                    }
                    if let Some(p) = &n.permissions {
                        m.insert("permissions".into(), p.clone().into());
                    }
                    m.into()
                })
                .collect();
            o.insert("deviceNodes".into(), nodes.into());
        }
        if !self.mounts.is_empty() {
            let ms: Vec<serde_json::Value> = self.mounts.iter().map(|m| serde_json::json!({ "hostPath": m.host_path, "containerPath": m.container_path, "options": m.options })).collect();
            o.insert("mounts".into(), ms.into());
        }
        o.into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub edits: Edits,
}

/// A CDI document: `kind` is `vendor.com/class`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub kind: String,
    pub devices: Vec<Device>,
    /// Edits applied whenever any device of the spec is requested.
    pub common: Edits,
}

impl Spec {
    /// The deterministic text: pretty JSON with sorted keys, a final newline.
    pub fn to_json(&self) -> String {
        let devices: Vec<serde_json::Value> = self.devices.iter().map(|d| serde_json::json!({ "name": d.name, "containerEdits": d.edits.to_json() })).collect();
        let mut doc = serde_json::Map::new();
        doc.insert("cdiVersion".into(), CDI_VERSION.into());
        doc.insert("kind".into(), self.kind.clone().into());
        doc.insert("devices".into(), devices.into());
        if !self.common.is_empty() {
            doc.insert("containerEdits".into(), self.common.to_json());
        }
        let mut s = serde_json::to_string_pretty(&serde_json::Value::Object(doc)).unwrap_or_default();
        s.push('\n');
        s
    }

    /// `vendor` and `class` of the kind.
    pub fn vendor_class(&self) -> Option<(&str, &str)> {
        self.kind.split_once('/')
    }

    /// The file name containerd users give it: `nvidia.com-gpu.json`.
    pub fn file_name(&self) -> String {
        format!("{}.json", self.kind.replace('/', "-"))
    }

    /// The rules of the specification that matter here: a valid kind, at least one device, unique valid names, absolute paths.
    pub fn validate(&self) -> Result<(), String> {
        let (vendor, class) = self.vendor_class().ok_or("the kind has no `/`")?;
        let ok_word = |w: &str| !w.is_empty() && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !w.ends_with(['-', '_', '.']);
        if !ok_word(vendor) || !vendor.contains('.') || !ok_word(class) {
            return Err(format!("`{}` is not a valid CDI kind (vendor.domain/class)", self.kind));
        }
        if self.devices.is_empty() {
            return Err("a CDI spec needs at least one device".into());
        }
        let mut seen: Vec<&str> = Vec::new();
        for d in &self.devices {
            if d.name.is_empty() || !d.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
                return Err(format!("`{}` is not a valid device name", d.name));
            }
            if seen.contains(&d.name.as_str()) {
                return Err(format!("device `{}` is listed twice", d.name));
            }
            seen.push(&d.name);
        }
        let all = self.devices.iter().map(|d| &d.edits).chain(std::iter::once(&self.common));
        for e in all {
            for n in &e.device_nodes {
                if !n.path.starts_with('/') {
                    return Err(format!("device node path `{}` is not absolute", n.path));
                }
            }
            for m in &e.mounts {
                if !m.host_path.starts_with('/') || !m.container_path.starts_with('/') {
                    return Err(format!("mount `{}` -> `{}` has a path that is not absolute", m.host_path, m.container_path));
                }
            }
        }
        Ok(())
    }
}

/// Write `spec` into `dir` (created) as `<kind>.json`, through a temporary name and a rename. Returns the path.
pub fn write_spec(dir: &Path, spec: &Spec) -> Result<PathBuf, String> {
    spec.validate()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join(spec.file_name());
    let tmp = dir.join(format!(".{}.tmp", spec.file_name()));
    std::fs::write(&tmp, spec.to_json()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target)
}

/// Remove the specs of a kind from `dir` (no error when absent).
pub fn remove_spec(dir: &Path, kind: &str) {
    let _ = std::fs::remove_file(dir.join(format!("{}.json", kind.replace('/', "-"))));
}

/// Every spec file of `dir`: `(file name, kind, device names)`, unreadable files skipped.
pub fn list_specs(dir: &Path) -> Vec<(String, String, Vec<String>)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.ends_with(".json") || n.starts_with('.') {
            continue;
        }
        let Some(j) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let kind = j["kind"].as_str().unwrap_or("").to_string();
        let names = j["devices"].as_array().into_iter().flatten().filter_map(|d| d["name"].as_str().map(str::to_string)).collect();
        v.push((n, kind, names));
    }
    v.sort();
    v
}

// ------------------------------------------------------------------------------------------------ containerd (k3s)

/// The drop-in file name.
pub const DROPIN_NAME: &str = "iyakki-cdi.toml";

/// The two drop-ins under `<k3s data dir>/agent/etc/containerd/`: `(relative path, text)`. Version 2 for containerd 1.7 (and for a k3s
/// that still renders the old template), version 3 for containerd 2.x. containerd 2 has CDI on by default; the drop-in only makes
/// that explicit and fixes the directories, so a k3s that changes its default does not change this.
pub fn dropin_files() -> Vec<(String, String)> {
    let dirs = "[\"/etc/cdi\", \"/var/run/cdi\"]";
    vec![
        (
            format!("config.toml.d/{DROPIN_NAME}"),
            format!("# written by IYAKKI (GPU for local models): containerd 1.7 style configuration, CDI on\n[plugins.\"io.containerd.grpc.v1.cri\"]\n  enable_cdi = true\n  cdi_spec_dirs = {dirs}\n"),
        ),
        (
            format!("config-v3.toml.d/{DROPIN_NAME}"),
            format!("# written by IYAKKI (GPU for local models): containerd 2.x style configuration, CDI on\n[plugins.'io.containerd.cri.v1.runtime']\n  enable_cdi = true\n  cdi_spec_dirs = {dirs}\n"),
        ),
    ]
}

/// The folder the drop-ins go under for a k3s whose `--data-dir` is `k3s_data`.
pub fn containerd_dir(k3s_data: &Path) -> PathBuf {
    k3s_data.join("agent/etc/containerd")
}

/// Write the drop-ins; `true` when something changed (k3s then has to restart to read them).
pub fn ensure_dropins(k3s_data: &Path) -> Result<bool, String> {
    let base = containerd_dir(k3s_data);
    let mut changed = false;
    for (rel, text) in dropin_files() {
        let p = base.join(&rel);
        if std::fs::read_to_string(&p).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        std::fs::create_dir_all(p.parent().unwrap_or(&base)).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        changed = true;
    }
    Ok(changed)
}

/// Remove the drop-ins.
pub fn remove_dropins(k3s_data: &Path) {
    let base = containerd_dir(k3s_data);
    for (rel, _) in dropin_files() {
        let _ = std::fs::remove_file(base.join(rel));
    }
}

/// What k3s's rendered containerd configuration (`<data>/agent/etc/containerd/config.toml`) says about CDI: the drop-in must be imported
/// and the key must be there. `None` when the file does not exist yet (k3s was not started with the drop-ins).
pub fn cdi_enabled_in(rendered: &str) -> bool {
    let imports_dropin = rendered.lines().any(|l| l.trim_start().starts_with("imports") && (l.contains("config.toml.d") || l.contains("config-v3.toml.d")));
    let explicit = rendered.lines().any(|l| l.replace(' ', "").starts_with("enable_cdi=true"));
    let v3 = rendered.lines().any(|l| l.replace(' ', "") == "version=3");
    explicit || (v3 && imports_dropin)
}

/// Is CDI usable by the cluster whose k3s data folder is `k3s_data`: the generated configuration exists and enables it.
pub fn cluster_cdi_enabled(k3s_data: &Path) -> bool {
    let cfg = containerd_dir(k3s_data).join("config.toml");
    let v3 = containerd_dir(k3s_data).join("config-v3.toml");
    [cfg, v3].iter().any(|p| std::fs::read_to_string(p).is_ok_and(|t| cdi_enabled_in(&t) || (t.lines().any(|l| l.replace(' ', "") == "version=3") && containerd_dir(k3s_data).join(format!("config-v3.toml.d/{DROPIN_NAME}")).exists())))
}

#[cfg(test)]
#[path = "gpucdi_tests.rs"]
pub(crate) mod tests;
