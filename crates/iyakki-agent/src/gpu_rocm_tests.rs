use super::*;
use crate::gpu::{Gpu, Vendor};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn card(vendor: Vendor, device: u16, driver: Option<&str>, render: bool) -> Gpu {
    Gpu { pci: "0000:03:00.0".into(), vendor_id: 0x1002, device_id: device, vendor, name: format!("test card {device:04x}"), driver: driver.map(String::from), card: Some("card0".into()), render: render.then(|| "renderD128".to_string()), boot_vga: false, vram: None }
}

fn root(kfd: bool) -> std::path::PathBuf {
    // one folder per call: tests of this file run side by side in one process and must not clear each other's folder
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-rocm-{}-{}-{}", std::process::id(), kfd, N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("dev")).unwrap();
    if kfd {
        std::fs::write(d.join("dev/kfd"), "").unwrap();
    }
    d
}

#[test]
fn a_modern_amd_card_with_amdgpu_a_render_node_and_kfd_may_try_rocm_and_every_missing_piece_is_named() {
    let with = root(true);
    let g = card(Vendor::Amd, 0x744C, Some("amdgpu"), true);
    assert!(rocm_verdict(&g, &with).0, "{:?}", rocm_verdict(&g, &with));
    let without = root(false);
    let (ok, why) = rocm_verdict(&g, &without);
    assert!(!ok && why.contains("no /dev/kfd") && why.contains("stays on Vulkan"), "{why}");
    assert!(rocm_verdict(&card(Vendor::Nvidia, 0x2684, Some("nvidia"), true), &with).1.contains("not an AMD card"));
    assert!(rocm_verdict(&card(Vendor::Amd, 0x744C, Some("radeon"), true), &with).1.contains("needs amdgpu"));
    assert!(rocm_verdict(&card(Vendor::Amd, 0x744C, Some("amdgpu"), false), &with).1.contains("no render node"));
    assert_eq!(rocm_usable(&[g.clone(), card(Vendor::Intel, 0x56A0, Some("i915"), true)], &with).len(), 1);
}

#[test]
fn polaris_and_older_cards_stay_on_vulkan_even_with_kfd() {
    let with = root(true);
    for (dev, gen) in [(0x67DF, "Polaris"), (0x6995, "Polaris"), (0x67B0, "Hawaii"), (0x6938, "Tonga"), (0x7300, "Fiji")] {
        let (ok, why) = rocm_verdict(&card(Vendor::Amd, dev, Some("amdgpu"), true), &with);
        assert!(!ok && why.contains(gen) && why.contains("stays on Vulkan"), "{dev:04x}: {why}");
    }
    assert_eq!(rocm_dropped_generation(0x744C), None, "an RDNA3 id is not excluded");
    assert_eq!(rocm_dropped_generation(0x7480), None);
}

#[test]
fn the_plugin_and_the_smoke_pod_need_an_image_pinned_by_digest_and_say_what_they_ask_for() {
    let img = format!("docker.io/rocm/k8s-device-plugin@sha256:{DIGEST}");
    let m = amd_plugin_manifest(&img, "/var/lib/kubelet").unwrap();
    for want in ["kind: DaemonSet", "namespace: kube-system", &format!("image: {img}"), "path: /var/lib/kubelet/device-plugins", "drop: [\"ALL\"]", "allowPrivilegeEscalation: false", "app.kubernetes.io/managed-by: iyakki"] {
        assert!(m.contains(want), "{want}\n{m}");
    }
    assert!(!m.contains("privileged: true"), "the plugin is not privileged");
    for bad in ["docker.io/rocm/k8s-device-plugin:latest", "x@sha256:abc", "x@sha256:ZZZZ456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "a b@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"] {
        assert!(amd_plugin_manifest(bad, "/var/lib/kubelet").unwrap_err().contains("pinned by digest"), "{bad}");
        assert!(rocm_smoke_pod(bad, "iyakki-ai").is_err());
    }
    assert!(amd_plugin_manifest(&img, "relative/dir").is_err() && amd_plugin_manifest(&img, "/a/../b").is_err());
    let pod = rocm_smoke_pod(&format!("docker.io/rocm/rocm-terminal@sha256:{DIGEST}"), "iyakki-ai").unwrap();
    assert!(pod.contains("name: gpu-smoke-rocm") && pod.contains("amd.com/gpu: 1") && pod.contains("command: [\"rocminfo\"]") && pod.contains("namespace: iyakki-ai"), "{pod}");
    assert!(rocm_smoke_pod(&format!("x@sha256:{DIGEST}"), "Bad Name").is_err());
}

#[test]
fn the_smoke_log_must_show_a_gpu_target_not_just_the_cpu_agent() {
    let good = "ROCk module is loaded\nAgent 1\n  Name:                    AMD Ryzen 9 7940HS\n  Marketing Name:          AMD Ryzen 9 7940HS w/ Radeon 780M Graphics\nAgent 2\n  Name:                    gfx1103\n  Marketing Name:          AMD Radeon 780M\n";
    assert_eq!(rocm_smoke_ok(good).unwrap(), "AMD Radeon 780M (gfx1103)");
    assert_eq!(rocm_smoke_ok("Name: gfx90a\n").unwrap(), "a ROCm device (gfx90a)");
    assert!(rocm_smoke_ok("Name: gfx000\n").is_err());
    assert!(rocm_smoke_ok("ROCk module is not loaded\n").unwrap_err().contains("did not reach the pod"));
    let e = rocm_smoke_ok("Agent 1\n Name: AMD Ryzen 9\n").unwrap_err();
    assert!(e.contains("no GPU target"), "{e}");
}

#[test]
fn the_rocm_patches_give_the_card_and_the_unpatches_take_it_away() {
    let v: serde_json::Value = serde_json::from_str(&lemonade_rocm_patch()).unwrap();
    let c = &v["spec"]["template"]["spec"]["containers"][0];
    assert_eq!((c["name"].as_str(), c["resources"]["limits"]["amd.com/gpu"].as_i64(), c["env"][0]["value"].as_str()), (Some("lemonade"), Some(1), Some("rocm")));
    let o: serde_json::Value = serde_json::from_str(&ollama_rocm_patch(None).unwrap()).unwrap();
    assert!(o["spec"]["template"]["spec"]["containers"][0]["image"].is_null());
    let with = ollama_rocm_patch(Some(&format!("ollama/ollama:rocm@sha256:{DIGEST}"))).unwrap();
    assert!(with.contains("ollama/ollama:rocm@sha256:"));
    assert!(ollama_rocm_patch(Some("ollama/ollama:rocm")).is_err(), "a tag is not a pin");
    for u in [crate::gpu_workloads::lemonade_unpatch(), crate::gpu_workloads::ollama_unpatch()] {
        let u: serde_json::Value = serde_json::from_str(&u).unwrap();
        assert!(u["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"].as_object().unwrap().contains_key("amd.com/gpu"));
    }
}

#[test]
fn the_pins_this_build_carries_are_checked_and_an_unset_one_is_named() {
    use iyakki_shared::gpu_rocm::Pins;
    let none = Pins::shipped();
    assert!(Pins::is_pinned(&none.plugin) && Pins::is_pinned(&none.smoke) && none.ollama_image().is_some(), "the shipped pins are valid digests");
    assert!(Pins::shipped().check().is_ok());
    let e = Pins { plugin: String::new(), smoke: format!("a@sha256:{DIGEST}"), ollama: String::new() }.check().unwrap_err();
    assert!(e.contains("device plugin") && !e.contains("test image"), "{e}");
    assert!(Pins::is_pinned(&format!("x/y@sha256:{DIGEST}")) && !Pins::is_pinned("x/y:latest") && !Pins::is_pinned(&format!("@sha256:{DIGEST}")) && !Pins::is_pinned(&format!("x@sha256:{}", DIGEST.to_uppercase())));
    assert!(Pins { plugin: String::new(), smoke: String::new(), ollama: "x:rocm".into() }.ollama_image().is_none());
}
