use super::*;

#[test]
fn the_vulkan_smoke_pod_asks_for_the_generic_resource_and_needs_no_privileges() {
    let m = vulkan_smoke_pod();
    assert!(m.contains("iyakki.dev/gpu: 1"), "{m}");
    assert!(m.contains(LLAMA_VULKAN_IMAGE) && m.contains("--list-devices") && m.contains("restartPolicy: Never"));
    assert!(m.contains("namespace: iyakki-ai") && m.contains("app.kubernetes.io/managed-by: iyakki"));
    assert!(!m.contains("privileged") && !m.contains("hostPath"), "the pod gets the device from the device plugin, not from privileges");
    assert!(!m.contains("cdi.k8s.io"), "a CDI annotation on a pod is ignored by kubelet (tried on k3s v1.37.1)");
}

#[test]
fn the_nvidia_smoke_pod_requests_the_extended_resource() {
    let m = nvidia_smoke_pod();
    assert!(m.contains("nvidia.com/gpu: 1") && m.contains(crate::gpu_nvidia::CUDA_BASE_IMAGE) && m.contains("command: [\"nvidia-smi\"]"));
    assert!(!m.contains("runtimeClassName"), "no NVIDIA runtime class: CDI does it");
}

#[test]
fn images_are_pinned_to_a_version_or_a_digest_not_latest() {
    for (img, what) in [(LLAMA_VULKAN_IMAGE, "llama.cpp vulkan"), (crate::gpu_nvidia::CUDA_BASE_IMAGE, "cuda base"), (crate::gpu_nvidia::LLAMA_CUDA_IMAGE, "llama.cpp cuda"), (crate::gpu_nvidia::OLLAMA_IMAGE, "ollama"), (crate::gpu_nvidia::PLUGIN_IMAGE, "nvidia device plugin"), (GENERIC_PLUGIN_IMAGE, "generic device plugin")] {
        let pinned = img.contains("@sha256:") && img.rsplit_once("@sha256:").is_some_and(|d| d.1.len() == 64) || img.rsplit_once(':').is_some_and(|t| !t.1.is_empty() && t.1 != "latest" && t.1.chars().any(|c| c.is_ascii_digit()) && !t.1.contains('/'));
        assert!(pinned, "{what}: {img}");
        assert!(!img.ends_with(":latest"), "{what}: {img}");
    }
}

#[test]
fn the_generic_plugin_lists_each_render_node_as_a_shared_group_under_our_domain() {
    let m = generic_plugin_manifest(&["renderD128".to_string(), "renderD129".to_string()]);
    assert!(m.contains(GENERIC_PLUGIN_IMAGE) && m.contains("--domain=iyakki.dev") && m.contains("name: gpu"));
    assert_eq!(m.matches("- path: /dev/dri/renderD").count(), 2);
    assert!(m.contains("path: /dev/dri/renderD128") && m.contains("path: /dev/dri/renderD129"));
    assert_eq!(m.matches(&format!("- count: {SHARE}")).count(), 2, "each node can be shared by several pods");
    assert!(m.contains("path: /var/lib/kubelet/device-plugins") && m.contains("mountPath: /var/lib/kubelet/device-plugins"), "the kubelet's plugin folder (k3s keeps it in /var/lib/kubelet)");
    assert!(m.contains("namespace: kube-system") && m.contains("kind: DaemonSet") && m.contains("app.kubernetes.io/managed-by: iyakki"));
    // the plugin itself needs /dev and a socket; the pods it serves do not need privileges
    assert!(m.contains("privileged: true"));
    // the device definition is a YAML block the plugin parses: the groups sit under `groups:` with a deeper indent
    let block: Vec<&str> = m.lines().skip_while(|l| !l.contains("name: gpu")).take(6).collect();
    assert!(block[1].trim() == "groups:" && block[2].trim().starts_with("- count"), "{block:?}");
}

#[test]
fn smoke_logs_are_judged_by_what_they_prove() {
    let vk = "ggml_vulkan: Found 1 Vulkan devices:\nVulkan0: AMD Radeon 780M Graphics (RADV PHOENIX) (radv) | uma: 1\nAvailable devices:\n  Vulkan0: AMD Radeon 780M Graphics (RADV PHOENIX) (23810 MiB, 23000 MiB free)\n";
    assert!(smoke_ok(Gpu::Vulkan, vk).unwrap().contains("Radeon 780M"));
    // what the real pod printed on k3s v1.37.1 with an AMD iGPU
    assert!(smoke_ok(Gpu::Vulkan, "Available devices:\n  Vulkan0: AMD Radeon 780M Graphics (RADV PHOENIX) (16718 MiB, 15582 MiB free)\n").is_ok());
    let soft = "Vulkan0: llvmpipe (LLVM 15.0.7, 256 bits) (cpu)";
    assert!(smoke_ok(Gpu::Vulkan, soft).unwrap_err().contains("software renderer"), "the GPU did not reach the container");
    assert!(smoke_ok(Gpu::Vulkan, "Available devices:\n  (none)\n").unwrap_err().contains("no Vulkan device"), "what the pod printed when the device was missing");
    let v = crate::gpu_nvidia::NVIDIA_VERSION;
    let smi = format!("+-----------------------------------------------------------------------------------------+\n| NVIDIA-SMI {v}              Driver Version: {v}      CUDA Version: 13.3     |\n|   0  NVIDIA GeForce RTX 4090        Off |   00000000:01:00.0  On |                  Off |\n");
    assert!(smoke_ok(Gpu::Nvidia, &smi).unwrap().contains("RTX 4090"));
    assert!(smoke_ok(Gpu::Nvidia, "| NVIDIA-SMI 550.00 |").unwrap_err().contains("not with driver"));
    assert!(smoke_ok(Gpu::Nvidia, "Failed to initialize NVML: Driver/library version mismatch").unwrap_err().contains("did not answer"));
}

#[test]
fn the_lemonade_patch_gives_the_gpu_and_the_unpatch_takes_it_away() {
    let v: serde_json::Value = serde_json::from_str(&lemonade_patch(Gpu::Vulkan, Some(990))).unwrap();
    let pod = &v["spec"]["template"]["spec"];
    assert_eq!(pod["containers"][0]["name"], "lemonade", "a strategic merge needs the container name");
    assert_eq!(pod["containers"][0]["resources"]["limits"]["iyakki.dev/gpu"], 1);
    assert_eq!(pod["containers"][0]["env"][0]["value"], "vulkan");
    assert_eq!(pod["securityContext"]["supplementalGroups"][0], 990, "the unprivileged user needs the render node's group");
    let no_group: serde_json::Value = serde_json::from_str(&lemonade_patch(Gpu::Vulkan, None)).unwrap();
    assert!(no_group["spec"]["template"]["spec"]["securityContext"].is_null());
    let n: serde_json::Value = serde_json::from_str(&lemonade_patch(Gpu::Nvidia, Some(5))).unwrap();
    let c = &n["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(c["resources"]["limits"]["nvidia.com/gpu"], 1);
    assert_eq!(c["env"][0]["value"], "cuda");
    assert!(n["spec"]["template"]["spec"]["securityContext"].is_null(), "no render group for CUDA");
    let u: serde_json::Value = serde_json::from_str(&lemonade_unpatch()).unwrap();
    let c = &u["spec"]["template"]["spec"]["containers"][0];
    assert!(c["resources"]["limits"]["iyakki.dev/gpu"].is_null() && c["resources"]["limits"]["nvidia.com/gpu"].is_null());
    assert_eq!(c["env"][0]["$patch"], "delete");
}

#[test]
fn the_ollama_variants_differ_in_how_they_get_the_gpu() {
    let n = ollama_gpu_manifest(Gpu::Nvidia);
    // limits sit beside requests (the same indentation), under resources
    let indent = |m: &str, key: &str| m.lines().filter(|l| l.trim() == key).last().map(|l| l.len() - l.trim_start().len());
    assert_eq!(indent(&n, "limits:"), indent(&n, "requests:"), "limits must be a sibling of requests");
    assert!(n.contains("nvidia.com/gpu: 1") && !n.contains("OLLAMA_VULKAN") && !n.contains("iyakki.dev/gpu"));
    let v = ollama_gpu_manifest(Gpu::Vulkan);
    assert!(v.contains("OLLAMA_VULKAN") && v.contains("iyakki.dev/gpu: 1") && !v.contains("nvidia.com/gpu") && !v.contains("cdi.k8s.io"));
    for m in [&n, &v] {
        assert!(m.contains(crate::gpu_nvidia::OLLAMA_IMAGE) && m.contains("hostIP: 127.0.0.1") && m.contains("hostPort: 11435"), "loopback only, away from a host Ollama on 11434");
        assert!(m.contains("kind: PersistentVolumeClaim") && m.contains("kind: Deployment") && m.contains("kind: Service"));
        assert_eq!(m.matches("\n---\n").count(), 2);
    }
}

#[test]
fn applying_a_pod_creates_the_namespace_first() {
    let m = namespace_and(&vulkan_smoke_pod());
    assert!(m.starts_with("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: iyakki-ai"));
    assert!(m.contains("\n---\napiVersion: v1\nkind: Pod"));
}

#[test]
fn node_resource_counts_come_from_the_node_list() {
    let j = "{\"items\":[{\"status\":{\"allocatable\":{\"iyakki.dev/gpu\":\"8\",\"nvidia.com/gpu\":\"1\"}}},{\"status\":{\"allocatable\":{}}}]}";
    assert_eq!(node_resource_count(j, GPU_RESOURCE), 8);
    assert_eq!(node_resource_count(j, NVIDIA_RESOURCE), 1);
    assert_eq!(node_resource_count("{\"items\":[]}", GPU_RESOURCE), 0);
    assert_eq!(node_resource_count("not json", GPU_RESOURCE), 0);
    assert_eq!(Gpu::Vulkan.resource(), "iyakki.dev/gpu");
    assert_eq!(Gpu::Nvidia.resource(), "nvidia.com/gpu");
}

/// With `GPU_DUMP_DIR=/some/folder` the manifests are written there, to be checked against a real (throw-away) cluster with
/// `kubectl apply --dry-run=server`; without it this test does nothing.
#[test]
fn the_manifests_can_be_dumped_for_a_real_cluster_check() {
    let Ok(dir) = std::env::var("GPU_DUMP_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let w = |n: &str, t: String| std::fs::write(dir.join(n), t).unwrap();
    w("vulkan-smoke.yaml", namespace_and(&vulkan_smoke_pod()));
    w("nvidia-smoke.yaml", namespace_and(&nvidia_smoke_pod()));
    w("ollama-vulkan.yaml", namespace_and(&ollama_gpu_manifest(Gpu::Vulkan)));
    w("ollama-nvidia.yaml", namespace_and(&ollama_gpu_manifest(Gpu::Nvidia)));
    w("generic-plugin.yaml", generic_plugin_manifest(&["renderD128".to_string()]));
    w("plugin.yaml", crate::gpu_nvidia::plugin_manifest(KUBELET_DIR, "/var/lib/iyakki/gpu/nvidia/595.99.02/stage"));
    w("lemonade-vulkan.patch.json", lemonade_patch(Gpu::Vulkan, Some(990)));
    w("lemonade-nvidia.patch.json", lemonade_patch(Gpu::Nvidia, None));
    w("lemonade-unpatch.json", lemonade_unpatch());
    w("lemonade.yaml", crate::quickinstall::lemonade_manifest());
}
