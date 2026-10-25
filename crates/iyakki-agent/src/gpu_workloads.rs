//! Cluster objects for local models on a GPU (F-GPU-01, F-GPU-03; docs/GPU-K3S.md): the generic device plugin of stage 1, smoke pods that
//! prove the GPU reaches a container, the settings Lemonade gets when a GPU is enabled, and an Ollama deployment with a GPU. All are plain
//! text for the managed cluster's `kubectl` (the Quick install steps apply them; nothing here talks to a cluster). Images are pinned and
//! pulled by the cluster at the person's click; IYAKKI ships none.
//!
//! How a pod gets the GPU:
//! * **Stage 1, any GPU with a render node (Vulkan through Mesa in the image)**: a generic device plugin (squat/generic-device-plugin,
//!   Apache-2.0) advertises the DRM render nodes as the extended resource `iyakki.dev/gpu`; a pod asks for `iyakki.dev/gpu: 1` and
//!   kubelet and containerd add the device node and its cgroup permission, so the pod runs unprivileged. (Tried on k3s v1.37.1 with an AMD
//!   Radeon 780M: the pod's `llama-server --list-devices` printed `Vulkan0: AMD Radeon 780M Graphics (RADV PHOENIX)`. A CDI annotation on
//!   the pod does not work without a device plugin: kubelet does not pass pod annotations to the runtime.)
//! * **Stage 3, NVIDIA vendor driver**: the pod asks for `nvidia.com/gpu: 1`; the NVIDIA device plugin allocates and answers with a CDI
//!   name; containerd resolves it in IYAKKI's spec (libraries, nodes, environment).

/// The namespace of the AI servers (shared with Lemonade).
pub const NAMESPACE: &str = crate::quickinstall::LEMONADE_NAMESPACE;
/// llama.cpp's Vulkan image (pinned build): Mesa's Vulkan drivers (RADV, ANV, NVK) and `llama-server`, whose `--list-devices` names the GPU.
pub const LLAMA_VULKAN_IMAGE: &str = "ghcr.io/ggml-org/llama.cpp:server-vulkan-b11749";
/// The generic device plugin (Apache-2.0), pinned by digest (the multi-arch index published on 2026-10-19).
pub const GENERIC_PLUGIN_IMAGE: &str = "ghcr.io/squat/generic-device-plugin@sha256:5c48f7a16cfb37c4e07731bfa384b9dba6e12d2b0fb4350d8a321d49f12ac494";
/// The extended resource of stage 1.
pub const GPU_RESOURCE: &str = "iyakki.dev/gpu";
/// The extended resource of NVIDIA's device plugin.
pub const NVIDIA_RESOURCE: &str = "nvidia.com/gpu";
/// k3s keeps the kubelet's files in `/var/lib/kubelet` whatever its `--data-dir` is (checked on v1.37.1).
pub const KUBELET_DIR: &str = "/var/lib/kubelet";
/// How many pods may share one GPU's render node (a render node can be opened by any number of processes).
pub const SHARE: u32 = 8;

/// Which GPU path a workload uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpu {
    /// Vulkan through the generic device plugin (any vendor).
    Vulkan,
    /// NVIDIA's driver: `nvidia.com/gpu` and CUDA.
    Nvidia,
}

impl Gpu {
    /// The extended resource a pod of this path asks for.
    pub fn resource(self) -> &'static str {
        match self {
            Gpu::Vulkan => GPU_RESOURCE,
            Gpu::Nvidia => NVIDIA_RESOURCE,
        }
    }
}

/// `manifest` preceded by the namespace object (applied again, it changes nothing): the AI namespace exists when Lemonade was installed, not before.
pub fn namespace_and(manifest: &str) -> String {
    format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NAMESPACE}\n  labels:\n    {}\n---\n{manifest}", managed())
}

fn managed() -> &'static str {
    "app.kubernetes.io/managed-by: iyakki"
}

/// The stage 1 device plugin as a DaemonSet in `kube-system`: one device group per render node (`/dev/dri/renderD128` ...), each
/// schedulable [`SHARE`] times, advertised as `iyakki.dev/gpu`. The plugin runs privileged (it has to see `/dev` and create its socket in
/// the kubelet's plugin folder); the pods that use the GPU do not.
pub fn generic_plugin_manifest(render_nodes: &[String]) -> String {
    let mut groups = String::new();
    for n in render_nodes {
        groups.push_str(&format!("                - count: {SHARE}\n                  paths:\n                    - path: /dev/dri/{n}\n"));
    }
    format!(
        "apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: iyakki-gpu-device-plugin
  namespace: kube-system
  labels:
    app.kubernetes.io/name: iyakki-gpu-device-plugin
    {m}
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: iyakki-gpu-device-plugin
  updateStrategy:
    type: RollingUpdate
  template:
    metadata:
      labels:
        app.kubernetes.io/name: iyakki-gpu-device-plugin
        {m}
    spec:
      priorityClassName: system-node-critical
      containers:
        - name: generic-device-plugin
          image: {img}
          args:
            - --domain=iyakki.dev
            - --device
            - |
              name: gpu
              groups:
{groups}          securityContext:
            privileged: true
          resources:
            requests:
              cpu: 20m
              memory: 10Mi
            limits:
              memory: 40Mi
          volumeMounts:
            - name: device-plugin
              mountPath: /var/lib/kubelet/device-plugins
            - name: dev
              mountPath: /dev
      volumes:
        - name: device-plugin
          hostPath:
            path: {kubelet}/device-plugins
            type: Directory
        - name: dev
          hostPath:
            path: /dev
",
        m = managed(),
        img = GENERIC_PLUGIN_IMAGE,
        kubelet = KUBELET_DIR,
        groups = groups
    )
}

/// Stage 1 smoke test: a pod that asks for one GPU and lists the Vulkan devices llama.cpp sees, then exits.
pub fn vulkan_smoke_pod() -> String {
    format!(
        "apiVersion: v1
kind: Pod
metadata:
  name: gpu-smoke-vulkan
  namespace: {ns}
  labels:
    {m}
spec:
  restartPolicy: Never
  containers:
    - name: list-devices
      image: {img}
      args: [\"--list-devices\"]
      resources:
        limits:
          {res}: 1
          memory: 2Gi
",
        ns = NAMESPACE,
        m = managed(),
        img = LLAMA_VULKAN_IMAGE,
        res = GPU_RESOURCE
    )
}

/// Stage 3 smoke test: `nvidia-smi` in NVIDIA's CUDA base image, one GPU requested.
pub fn nvidia_smoke_pod() -> String {
    format!(
        "apiVersion: v1
kind: Pod
metadata:
  name: gpu-smoke-nvidia
  namespace: {ns}
  labels:
    {m}
spec:
  restartPolicy: Never
  containers:
    - name: nvidia-smi
      image: {img}
      command: [\"nvidia-smi\"]
      resources:
        limits:
          {res}: 1
",
        ns = NAMESPACE,
        m = managed(),
        img = crate::gpu_nvidia::CUDA_BASE_IMAGE,
        res = NVIDIA_RESOURCE
    )
}

/// The pod a smoke test creates, by path.
pub fn smoke_pod(g: Gpu) -> (&'static str, String) {
    match g {
        Gpu::Vulkan => ("gpu-smoke-vulkan", vulkan_smoke_pod()),
        Gpu::Nvidia => ("gpu-smoke-nvidia", nvidia_smoke_pod()),
    }
}

/// What a smoke pod's log has to say for the step to call it good: the GPU's name for Vulkan (a line `Vulkan0: <name>`), the table header
/// of `nvidia-smi` for NVIDIA.
pub fn smoke_ok(g: Gpu, log: &str) -> Result<String, String> {
    match g {
        Gpu::Vulkan => {
            let dev = log.lines().map(str::trim).find(|l| l.starts_with("Vulkan") && l.contains(':'));
            match dev {
                Some(l) if !l.to_ascii_lowercase().contains("llvmpipe") => Ok(l.to_string()),
                Some(l) => Err(format!("only the software renderer was found ({l}): the GPU did not reach the container")),
                None => Err(format!("the pod listed no Vulkan device. Its log: {}", log.lines().rev().take(6).collect::<Vec<_>>().join(" | "))),
            }
        }
        Gpu::Nvidia => {
            if log.contains("NVIDIA-SMI") && log.contains(crate::gpu_nvidia::NVIDIA_VERSION) {
                let name = log.lines().find(|l| l.contains("NVIDIA GeForce") || l.contains("NVIDIA RTX") || l.contains("NVIDIA A") || l.contains("NVIDIA L")).map(|l| l.trim().trim_matches('|').trim().to_string());
                Ok(name.unwrap_or_else(|| format!("nvidia-smi answered (driver {})", crate::gpu_nvidia::NVIDIA_VERSION)))
            } else if log.contains("NVIDIA-SMI") {
                Err(format!("nvidia-smi answered, but not with driver {}: {}", crate::gpu_nvidia::NVIDIA_VERSION, log.lines().find(|l| l.contains("NVIDIA-SMI")).unwrap_or("").trim()))
            } else {
                Err(format!("nvidia-smi did not answer. Its log: {}", log.lines().rev().take(6).collect::<Vec<_>>().join(" | ")))
            }
        }
    }
}

/// The strategic-merge patch that gives the Lemonade Deployment (`quickinstall::lemonade_manifest`) its GPU: the resource limit of the path
/// and the llama.cpp backend (Vulkan or CUDA). `render_gid` is the group that owns the render node: the Lemonade image runs as an
/// unprivileged user (10001) that needs it to open `/dev/dri/renderD*` (Vulkan only).
pub fn lemonade_patch(g: Gpu, render_gid: Option<u32>) -> String {
    let backend = match g {
        Gpu::Vulkan => "vulkan",
        Gpu::Nvidia => "cuda",
    };
    let container = serde_json::json!({ "name": "lemonade", "env": [{ "name": "LEMONADE_LLAMACPP", "value": backend }], "resources": { "limits": { g.resource(): 1 } } });
    let mut pod = serde_json::json!({ "containers": [container] });
    if let (Gpu::Vulkan, Some(gid)) = (g, render_gid) {
        pod["securityContext"] = serde_json::json!({ "supplementalGroups": [gid] });
    }
    serde_json::json!({ "spec": { "template": { "spec": pod } } }).to_string()
}

/// The patch that takes the GPU away again (a `null` deletes the key; the env entry is deleted by name).
pub fn lemonade_unpatch() -> String {
    serde_json::json!({ "spec": { "template": { "spec": { "securityContext": { "supplementalGroups": null }, "containers": [{ "name": "lemonade", "env": [{ "name": "LEMONADE_LLAMACPP", "$patch": "delete" }], "resources": { "limits": { GPU_RESOURCE: null, NVIDIA_RESOURCE: null } } }] } } } }).to_string()
}

/// Ollama as a Deployment in `iyakki-ai` with a GPU: the image from Docker Hub, its models on a volume, a ClusterIP Service on 11434 and a
/// port on this machine's loopback only (11435, so a host Ollama on 11434 never clashes). Vulkan sets `OLLAMA_VULKAN=1`.
pub fn ollama_gpu_manifest(g: Gpu) -> String {
    let env = match g {
        Gpu::Vulkan => "\n            - name: OLLAMA_VULKAN\n              value: \"1\"",
        Gpu::Nvidia => "",
    };
    format!(
        "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: ollama-gpu-data
  namespace: {ns}
  labels:
    {m}
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 30Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ollama-gpu
  namespace: {ns}
  labels:
    app.kubernetes.io/name: ollama-gpu
    {m}
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: ollama-gpu
  template:
    metadata:
      labels:
        app.kubernetes.io/name: ollama-gpu
        {m}
    spec:
      containers:
        - name: ollama
          image: {img}
          env:
            - name: OLLAMA_HOST
              value: 0.0.0.0:11434{env}
          ports:
            - containerPort: 11434
              hostPort: 11435
              hostIP: 127.0.0.1
          resources:
            requests:
              memory: 2Gi
            limits:
              {res}: 1
          readinessProbe:
            httpGet:
              path: /
              port: 11434
            initialDelaySeconds: 5
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /root/.ollama
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: ollama-gpu-data
---
apiVersion: v1
kind: Service
metadata:
  name: ollama-gpu
  namespace: {ns}
  labels:
    {m}
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: ollama-gpu
  ports:
    - port: 11434
      targetPort: 11434
",
        ns = NAMESPACE,
        m = managed(),
        img = crate::gpu_nvidia::OLLAMA_IMAGE,
        env = env,
        res = g.resource()
    )
}

/// The names of the objects the GPU steps own, for the removal.
pub const OWNED: &[(&str, &str)] = &[("pod", "gpu-smoke-vulkan"), ("pod", "gpu-smoke-nvidia"), ("deployment", "ollama-gpu"), ("service", "ollama-gpu")];

/// Allocatable count of an extended resource on the nodes, from `kubectl get nodes -o json`.
pub fn node_resource_count(json: &str, resource: &str) -> u32 {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return 0 };
    v["items"].as_array().into_iter().flatten().filter_map(|n| n["status"]["allocatable"][resource].as_str().and_then(|s| s.parse::<u32>().ok())).sum()
}

#[cfg(test)]
#[path = "gpu_workloads_tests.rs"]
mod tests;
