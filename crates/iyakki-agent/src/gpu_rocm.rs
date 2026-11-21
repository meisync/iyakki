//! GPU for local models in k3s, stage 2, AMD compute (F-GPU-02; docs/GPU-K3S.md): the parts that do not need the card. The kernel side
//! (`CONFIG_HSA_AMD`, the KFD compute interface) is in `linux.fragment`; this file decides whether a card is worth offering ROCm for, and writes
//! the cluster objects for it: AMD's device plugin (`amd.com/gpu`), a smoke pod that runs `rocminfo`, and the check of its log. It talks to no
//! cluster and starts nothing.
//!
//! * **The images are the owner's to pin.** ROCm images are large and change often; this code takes the image reference as an argument and
//!   refuses one without `@sha256:<digest>`, the same rule as image apps. Until a digest is chosen and tested on a real card, the Quick install
//!   entry cannot be offered (KI-782).
//! * **Older cards stay on Vulkan (stage 1).** ROCm dropped the GCN generations; [`rocm_verdict`] says so for the ones it knows by PCI id
//!   (Polaris and older). A card it does not know is not excluded: the smoke pod is the proof that matters.


pub use iyakki_shared::gpu_rocm::{rocm_dropped_generation, rocm_usable, rocm_verdict, RESOURCE};


fn managed() -> &'static str {
    "app.kubernetes.io/managed-by: iyakki"
}

fn pinned(image: &str) -> Result<&str, String> {
    let ok = image.contains("@sha256:") && image.rsplit("@sha256:").next().is_some_and(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    if ok && !image.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') { Ok(image) } else { Err(format!("`{image}` is not an image pinned by digest (name@sha256:<64 hex digits>): a tag can be moved to other contents")) }
}

/// AMD's device plugin as a DaemonSet in `kube-system`: it finds the `/dev/kfd` and `/dev/dri` devices and advertises them as `amd.com/gpu`; a pod
/// that asks for `amd.com/gpu: 1` gets the nodes it needs and runs unprivileged. The plugin itself needs the kubelet's plugin folder and `/sys`.
pub fn amd_plugin_manifest(image: &str, kubelet_dir: &str) -> Result<String, String> {
    let image = pinned(image)?;
    if !kubelet_dir.starts_with('/') || kubelet_dir.contains("..") || kubelet_dir.contains(|c: char| c.is_whitespace()) {
        return Err("the kubelet folder is an absolute path".into());
    }
    Ok(format!(
        "apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: iyakki-amd-gpu-device-plugin
  namespace: kube-system
  labels:
    app.kubernetes.io/name: iyakki-amd-gpu-device-plugin
    {m}
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: iyakki-amd-gpu-device-plugin
  updateStrategy:
    type: RollingUpdate
  template:
    metadata:
      labels:
        app.kubernetes.io/name: iyakki-amd-gpu-device-plugin
        {m}
    spec:
      priorityClassName: system-node-critical
      nodeSelector:
        iyakki.dev/amd-compute: \"true\"
      containers:
        - name: amdgpu-dp
          image: {image}
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: [\"ALL\"]
          resources:
            requests:
              cpu: 20m
              memory: 20Mi
            limits:
              memory: 80Mi
          volumeMounts:
            - name: device-plugin
              mountPath: /var/lib/kubelet/device-plugins
            - name: sys
              mountPath: /sys
      volumes:
        - name: device-plugin
          hostPath:
            path: {kubelet}/device-plugins
        - name: sys
          hostPath:
            path: /sys
",
        m = managed(),
        image = image,
        kubelet = kubelet_dir
    ))
}

/// The smoke pod: one `amd.com/gpu`, runs `rocminfo` and exits. `namespace` is the AI namespace of the cluster.
pub fn rocm_smoke_pod(image: &str, namespace: &str) -> Result<String, String> {
    let image = pinned(image)?;
    if namespace.is_empty() || !namespace.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err("the namespace is lower-case letters, digits and dashes".into());
    }
    Ok(format!(
        "apiVersion: v1
kind: Pod
metadata:
  name: gpu-smoke-rocm
  namespace: {namespace}
  labels:
    {m}
spec:
  restartPolicy: Never
  containers:
    - name: rocminfo
      image: {image}
      command: [\"rocminfo\"]
      resources:
        limits:
          {res}: 1
          memory: 2Gi
",
        m = managed(),
        res = RESOURCE
    ))
}

/// What the log of the smoke pod has to say: a GPU agent whose name is a `gfx` target (the CPU agent is also listed by `rocminfo`).
pub fn rocm_smoke_ok(log: &str) -> Result<String, String> {
    let gfx = log.lines().map(str::trim).find_map(|l| l.strip_prefix("Name:").map(str::trim).filter(|n| n.starts_with("gfx") && n.len() > 3 && n[3..].bytes().all(|b| b.is_ascii_alphanumeric()) && *n != "gfx000"));
    match gfx {
        Some(t) => {
            let market = log.lines().map(str::trim).filter_map(|l| l.strip_prefix("Marketing Name:")).map(str::trim).find(|m| !m.is_empty() && !m.contains("CPU") && !m.contains("Ryzen") && !m.contains("EPYC") && !m.contains("Core"));
            Ok(match market {
                Some(m) => format!("{m} ({t})"),
                None => format!("a ROCm device ({t})"),
            })
        }
        None if log.contains("ROCk module") && log.contains("not loaded") => Err("the ROCm kernel module is not loaded in the container's view: /dev/kfd did not reach the pod".into()),
        None => Err(format!("rocminfo listed no GPU target. Its log: {}", log.lines().rev().take(6).collect::<Vec<_>>().join(" | "))),
    }
}

/// The label the device plugin's DaemonSet selects on (`iyakki.dev/amd-compute=true`): put on the node by the step, taken off by the uninstall.
pub const NODE_LABEL: &str = "iyakki.dev/amd-compute";

/// The patch that gives the Lemonade Deployment the card: `amd.com/gpu: 1` and the ROCm llama.cpp backend (Lemonade fetches its own engine).
pub fn lemonade_rocm_patch() -> String {
    serde_json::json!({ "spec": { "template": { "spec": { "containers": [{ "name": "lemonade", "env": [{ "name": "LEMONADE_LLAMACPP", "value": "rocm" }], "resources": { "limits": { RESOURCE: 1 } } }] } } } }).to_string()
}

/// The patch that gives the Ollama Deployment the card: `amd.com/gpu: 1` and, when the owner pinned one, the ROCm build of Ollama.
pub fn ollama_rocm_patch(image: Option<&str>) -> Result<String, String> {
    let mut c = serde_json::json!({ "name": "ollama", "resources": { "limits": { RESOURCE: 1 } } });
    if let Some(i) = image {
        c["image"] = serde_json::Value::String(pinned(i)?.to_string());
    }
    Ok(serde_json::json!({ "spec": { "template": { "spec": { "containers": [c] } } } }).to_string())
}

#[cfg(test)]
#[path = "gpu_rocm_tests.rs"]
mod tests;
