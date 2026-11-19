use super::*;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-gpu-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// One fake PCI display device under `root`.
fn pci(root: &Path, addr: &str, class: &str, vendor: &str, device: &str, driver: Option<&str>, nodes: &[&str]) {
    let d = root.join("sys/bus/pci/devices").join(addr);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("class"), format!("{class}\n")).unwrap();
    std::fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
    std::fs::write(d.join("device"), format!("{device}\n")).unwrap();
    if let Some(drv) = driver {
        let t = root.join("sys/bus/pci/drivers").join(drv);
        std::fs::create_dir_all(&t).unwrap();
        std::os::unix::fs::symlink(&t, d.join("driver")).unwrap();
    }
    for n in nodes {
        std::fs::create_dir_all(d.join("drm").join(n)).unwrap();
    }
}

#[test]
fn the_device_tables_name_the_cards_and_fall_back_to_the_family() {
    assert_eq!(device_name(NVIDIA, 0x2684), "GeForce RTX 4090");
    assert_eq!(device_name(NVIDIA, 0x1e04), "GeForce RTX 2080 Ti");
    assert_eq!(device_name(AMD, 0x15bf), "Radeon 780M (Phoenix)");
    assert_eq!(device_name(INTEL, 0x56a0), "Arc A770");
    assert_eq!(device_name(NVIDIA, 0x2999), "NVIDIA Blackwell GPU (10de:2999)", "an unknown id still names the family");
    assert_eq!(device_name(0x1af4, 0x1050), "virtio graphics (1af4:1050)");
    // the families by id range
    for (id, fam) in [(0x2684, "Ada Lovelace"), (0x2782, "Ada Lovelace"), (0x2204, "Ampere"), (0x2503, "Ampere"), (0x1e04, "Turing"), (0x1f82, "Turing"), (0x2184, "Turing"), (0x2182, "Turing"), (0x1b80, "Pascal"), (0x1d81, "Volta"), (0x13c2, "Maxwell"), (0x2900, "Blackwell"), (0x0100, "unknown architecture")] {
        assert_eq!(nvidia_family(id), fam, "{id:04x}");
    }
    // the open kernel modules and nouveau's GSP path: Turing and newer only
    assert!(nvidia_open_supported(0x2684) && nvidia_open_supported(0x1e04) && nvidia_open_supported(0x2900));
    assert!(!nvidia_open_supported(0x1b80) && !nvidia_open_supported(0x13c2) && !nvidia_open_supported(0x1d81));
    // every named NVIDIA card in the table is in a family the verdict knows
    for (id, n) in NVIDIA_NAMES {
        assert_ne!(nvidia_family(*id), "unknown architecture", "{n} {id:04x}");
    }
}

#[test]
fn detection_reads_sysfs_for_every_display_device_and_ignores_the_rest() {
    let r = tmp("detect");
    // an RTX 4090 on nouveau, an AMD iGPU on amdgpu with its video memory, an Intel card without a driver, a VM display, a network card
    pci(&r, "0000:01:00.0", "0x030000", "0x10de", "0x2684", Some("nouveau"), &["card0", "renderD128", "card0-DP-1"]);
    std::fs::write(r.join("sys/bus/pci/devices/0000:01:00.0/boot_vga"), "0\n").unwrap();
    pci(&r, "0000:65:00.0", "0x030000", "0x1002", "0x15bf", Some("amdgpu"), &["card1", "renderD129"]);
    std::fs::write(r.join("sys/bus/pci/devices/0000:65:00.0/boot_vga"), "1\n").unwrap();
    std::fs::write(r.join("sys/bus/pci/devices/0000:65:00.0/mem_info_vram_total"), "536870912\n").unwrap();
    pci(&r, "0000:00:02.0", "0x038000", "0x8086", "0x4680", None, &[]);
    pci(&r, "0000:00:03.0", "0x030000", "0x1234", "0x1111", Some("bochs-drm"), &["card2"]);
    pci(&r, "0000:02:00.0", "0x020000", "0x8086", "0x15f3", Some("igc"), &[]);
    let g = detect(&r);
    assert_eq!(g.iter().map(|g| g.pci.as_str()).collect::<Vec<_>>(), ["0000:00:02.0", "0000:00:03.0", "0000:01:00.0", "0000:65:00.0"], "sorted by address, the network card left out");
    let nv = g.iter().find(|g| g.is_nvidia()).unwrap();
    assert_eq!((nv.name.as_str(), nv.driver.as_deref(), nv.card.as_deref(), nv.render.as_deref()), ("GeForce RTX 4090", Some("nouveau"), Some("card0"), Some("renderD128")), "the connector entry card0-DP-1 is not the card");
    assert_eq!(nv.family(), "Ada Lovelace");
    let amd = g.iter().find(|g| g.vendor == Vendor::Amd).unwrap();
    assert!(amd.boot_vga);
    assert_eq!(amd.vram, Some(512 << 20));
    assert!(amd.line().contains("Radeon 780M") && amd.line().contains("amdgpu") && amd.line().contains("renderD129") && amd.line().contains("1 GB"), "{}", amd.line());
    assert_eq!(amd.render_path(&r), Some(r.join("dev/dri/renderD129")));
    assert!(g.iter().find(|g| g.pci == "0000:00:03.0").unwrap().is_virtual());
    assert_eq!(g.iter().find(|g| g.pci == "0000:00:02.0").unwrap().driver, None);
    assert!(has_nvidia(&g));
    assert!(!has_nvidia(&[amd.clone()]));
    assert!(detect(&tmp("empty")).is_empty(), "no sysfs, no GPUs, no panic");
    let _ = std::fs::remove_dir_all(&r);
}

fn gpu_of(vendor: u16, device: u16, driver: Option<&str>, render: bool) -> Gpu {
    Gpu { pci: "0000:01:00.0".into(), vendor_id: vendor, device_id: device, vendor: Vendor::from_id(vendor), name: device_name(vendor, device), driver: driver.map(Into::into), card: Some("card0".into()), render: render.then(|| "renderD128".to_string()), boot_vga: false, vram: None }
}

#[test]
fn the_vulkan_verdict_says_why_in_plain_words() {
    let v = |g: Gpu| vulkan_verdict(&g);
    assert!(v(gpu_of(AMD, 0x15bf, Some("amdgpu"), true)).0, "RADV");
    assert!(v(gpu_of(INTEL, 0x4680, Some("i915"), true)).0 && v(gpu_of(INTEL, 0x56a0, Some("xe"), true)).0, "ANV");
    let (ok, why) = v(gpu_of(NVIDIA, 0x2684, Some("nouveau"), true));
    assert!(ok && why.contains("NVK") && why.contains("GSP"), "{why}");
    assert!(v(gpu_of(NVIDIA, 0x2684, Some("nvidia"), true)).0);
    let (ok, why) = v(gpu_of(NVIDIA, 0x1b80, Some("nouveau"), true));
    assert!(!ok && why.contains("Turing"), "a Pascal card has no NVK: {why}");
    let (ok, why) = v(gpu_of(NVIDIA, 0x2684, None, false));
    assert!(!ok && why.contains("no render node"), "{why}");
    let (ok, why) = v(gpu_of(AMD, 0x67df, Some("radeon"), true));
    assert!(!ok && why.contains("amdgpu"), "an old AMD card on radeon: {why}");
    assert!(!v(gpu_of(0x1234, 0x1111, Some("bochs-drm"), true)).0, "a virtual adapter");
    let all = [gpu_of(AMD, 0x15bf, Some("amdgpu"), true), gpu_of(NVIDIA, 0x2684, None, false)];
    assert_eq!(vulkan_usable(&all).len(), 1);
}

#[test]
fn the_kernel_log_gives_missing_firmware_and_the_lines_about_the_cards() {
    let log = "\
[    1.1] usb 1-1: new device
[    3.2] nouveau 0000:01:00.0: NVIDIA AD102 (192000a1)
[    3.3] nouveau 0000:01:00.0: Direct firmware load for nvidia/ad102/gsp/gsp-535.113.01.bin failed with error -2
[    3.3] nouveau 0000:01:00.0: gsp ctor failed: -2
[    3.4] amdgpu 0000:65:00.0: amdgpu: Direct firmware load for amdgpu/gc_11_0_1_pfp.bin failed with error -2
[    3.4] nouveau 0000:01:00.0: Direct firmware load for nvidia/ad102/gsp/gsp-535.113.01.bin failed with error -2
[    9.0] eth0: link up";
    assert_eq!(missing_firmware(log), ["nvidia/ad102/gsp/gsp-535.113.01.bin", "amdgpu/gc_11_0_1_pfp.bin"], "each file once");
    let l = kernel_lines(log, &["0000:01:00.0"]);
    assert!(l.iter().any(|x| x.contains("gsp ctor failed")) && l.iter().any(|x| x.contains("Direct firmware load")));
    assert!(!l.iter().any(|x| x.contains("eth0") || x.contains("usb")));
    assert!(missing_firmware("[1] all quiet").is_empty());
}

#[test]
fn the_driver_state_reads_modules_and_the_images_module_folder() {
    let r = tmp("driver");
    std::fs::create_dir_all(r.join("proc")).unwrap();
    std::fs::write(r.join("proc/modules"), "nouveau 3321856 3 - Live 0x0000000000000000\nnvidia_uvm 4000000 0 - Live 0x0\nnvidia 25000000 1 nvidia_uvm, Live 0x0\n").unwrap();
    assert!(module_loaded(&r, "nouveau") && module_loaded(&r, "nvidia") && module_loaded(&r, "nvidia-uvm"), "a dash is an underscore");
    assert!(!module_loaded(&r, "amdgpu") && !module_loaded(&r, "nvidia_drm"), "prefixes do not count");
    assert_eq!(nvidia_module_path(&r), None);
    std::fs::create_dir_all(r.join("lib/modules/6.12.112/updates")).unwrap();
    std::fs::write(r.join("lib/modules/6.12.112/updates/nvidia.ko"), b"x").unwrap();
    assert_eq!(nvidia_module_path(&r), Some(r.join("lib/modules/6.12.112/updates/nvidia.ko")));
    let s = driver_state(&r, &[], "[1] nvidia 0000:01:00.0: Direct firmware load for nvidia/595.99.02/gsp_ga10x.bin failed with error -2");
    assert!(s.nouveau_loaded && s.nvidia_loaded && s.nvidia_uvm_loaded && s.nvidia_module_present);
    assert_eq!(s.missing_firmware, ["nvidia/595.99.02/gsp_ga10x.bin"]);
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn pods_that_ask_for_a_gpu_are_found_by_resource_or_cdi_annotation() {
    let json = r#"{"items":[
      {"metadata":{"namespace":"iyakki-ai","name":"ollama-gpu-1"},"status":{"phase":"Running"},"spec":{"containers":[{"resources":{"limits":{"nvidia.com/gpu":"1"}}}]}},
      {"metadata":{"namespace":"iyakki-ai","name":"lemonade-7"},"status":{"phase":"Running"},"spec":{"containers":[{"resources":{"limits":{"iyakki.dev/gpu":"1","memory":"2Gi"}}}]}},
      {"metadata":{"namespace":"iyakki-ai","name":"annotated","annotations":{"cdi.k8s.io/nvidia":"nvidia.com/gpu=0"}},"status":{"phase":"Running"},"spec":{"containers":[{}]}},
      {"metadata":{"namespace":"iyakki-ai","name":"old-smoke"},"status":{"phase":"Succeeded"},"spec":{"containers":[{"resources":{"limits":{"nvidia.com/gpu":1}}}]}},
      {"metadata":{"namespace":"kube-system","name":"coredns"},"status":{"phase":"Running"},"spec":{"containers":[{"resources":{"limits":{"memory":"170Mi"}}}]}},
      {"metadata":{"namespace":"x","name":"other-cdi","annotations":{"cdi.k8s.io/x":"vendor.com/camera=0"}},"status":{"phase":"Running"},"spec":{"containers":[{}]}}
    ]}"#;
    let p = pods_using_gpu(json);
    assert_eq!(p.len(), 3, "{p:?}");
    assert_eq!((p[0].namespace.as_str(), p[0].name.as_str(), p[0].how.as_str()), ("iyakki-ai", "ollama-gpu-1", "nvidia.com/gpu x1"));
    assert_eq!((p[1].name.as_str(), p[1].how.as_str()), ("lemonade-7", "iyakki.dev/gpu x1"));
    assert_eq!((p[2].name.as_str(), p[2].how.as_str()), ("annotated", "CDI nvidia.com/gpu=0"));
    assert!(pods_using_gpu("not json").is_empty() && pods_using_gpu("{}").is_empty());
}

struct FakeUp {
    models: String,
    chat: String,
    status: u16,
    seen: Mutex<Vec<(String, String)>>,
}

impl iyakki_proxy::Upstream for FakeUp {
    fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        self.seen.lock().unwrap().push((format!("{method} {url}"), String::from_utf8_lossy(body).into_owned()));
        if url.ends_with("/models") {
            return Ok(iyakki_proxy::UpstreamResponse { status: 200, body: self.models.clone().into_bytes() });
        }
        Ok(iyakki_proxy::UpstreamResponse { status: self.status, body: self.chat.clone().into_bytes() })
    }
}

#[test]
fn the_test_prompt_picks_a_downloaded_chat_model_and_reports_tokens_per_second() {
    let up = FakeUp {
        models: r#"{"data":[{"id":"nomic-embed","downloaded":true,"labels":["embeddings"]},{"id":"Qwen3-0.6B-GGUF","downloaded":false,"labels":[]},{"id":"Llama-3.2-1B-GGUF","downloaded":true,"labels":["hot"]}]}"#.into(),
        chat: r#"{"choices":[{"message":{"content":"The sky is blue because of Rayleigh scattering."}}],"usage":{"completion_tokens":96}}"#.into(),
        status: 200,
        seen: Mutex::new(Vec::new()),
    };
    let t = Mutex::new(vec![100.0, 102.0]);
    let now = || t.lock().unwrap().remove(0);
    let r = test_prompt(&up, "http://127.0.0.1:13305/api/v1", None, &now).unwrap();
    assert_eq!((r.model.as_str(), r.tokens), ("Llama-3.2-1B-GGUF", 96), "not the embeddings model, not the one that is not downloaded");
    assert!((r.tokens_per_second - 48.0).abs() < 1e-9, "{}", r.tokens_per_second);
    assert_eq!(r.line(), "48.0 tokens/s (96 tokens in 2.0 s, model Llama-3.2-1B-GGUF)");
    let seen = up.seen.lock().unwrap();
    assert_eq!(seen[1].0, "POST http://127.0.0.1:13305/api/v1/chat/completions");
    assert!(seen[1].1.contains("Llama-3.2-1B-GGUF") && seen[1].1.contains(TEST_PROMPT) && seen[1].1.contains("\"stream\":false"));
}

#[test]
fn the_test_prompt_prefers_the_servers_own_timing_and_reports_every_failure_plainly() {
    let ok = r#"{"choices":[{"message":{"content":"Blue."}}],"usage":{"completion_tokens":50},"timings":{"predicted_ms":500.0}}"#;
    let r = parse_test_answer(ok, "m", 9.0).unwrap();
    assert!((r.tokens_per_second - 100.0).abs() < 1e-9, "llama.cpp's own generation time wins over the wall clock");
    let words = parse_test_answer(r#"{"choices":[{"message":{"content":"one two three four five six seven eight nine ten"}}]}"#, "m", 1.0).unwrap();
    assert_eq!(words.tokens, 13, "without usage the words are counted, times 1.3");
    assert!(parse_test_answer("<html>", "m", 1.0).unwrap_err().contains("not JSON"));
    assert!(parse_test_answer(r#"{"error":{"message":"model not loaded"}}"#, "m", 1.0).unwrap_err().contains("model not loaded"));
    assert!(parse_test_answer(r#"{"choices":[{"message":{"content":""}}]}"#, "m", 1.0).unwrap_err().contains("no text"));
    let up = FakeUp { models: r#"{"data":[]}"#.into(), chat: String::new(), status: 200, seen: Mutex::new(Vec::new()) };
    assert!(test_prompt(&up, "http://x/v1", None, &|| 0.0).unwrap_err().contains("no downloaded chat model"));
    let up = FakeUp { models: String::new(), chat: "boom".into(), status: 500, seen: Mutex::new(Vec::new()) };
    assert!(test_prompt(&up, "http://x/v1", Some("m"), &|| 0.0).unwrap_err().contains("HTTP 500"));
}

#[test]
fn a_real_llama_cpp_answer_from_the_vulkan_pod_is_read_with_its_own_timing() {
    // the answer llama-server b11749 gave on k3s v1.37.1 with an AMD Radeon 780M (RADV) through iyakki.dev/gpu: tests/fixtures kept as it came
    let body = include_str!("fixtures/gpu/llama-chat-response.json");
    let r = parse_test_answer(body, "m", 9.9).unwrap();
    assert_eq!(r.tokens, 91);
    assert!((r.seconds - 0.183471).abs() < 1e-9, "the server's own generation time, not the wall clock: {}", r.seconds);
    assert!((r.tokens_per_second - 495.98).abs() < 0.5, "{}", r.tokens_per_second);
    assert!(r.reply.starts_with("\"Why is the sky blue?\""));
}

#[test]
fn the_last_test_result_is_kept_in_the_data_folder_for_the_page() {
    let d = tmp("last");
    assert_eq!(last_test(&d), None);
    save_last_test(&d, "48.0 tokens/s").unwrap();
    assert_eq!(last_test(&d).as_deref(), Some("48.0 tokens/s"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_report_page_lists_gpus_driver_pods_and_the_last_test() {
    let r = Report {
        gpus: vec![gpu_of(NVIDIA, 0x2684, Some("nouveau"), true), gpu_of(AMD, 0x15bf, Some("amdgpu"), true)],
        driver: DriverState { nouveau_loaded: true, nvidia_module_present: true, missing_firmware: vec!["nvidia/ad102/gsp/gsp-535.113.01.bin".into()], ..Default::default() },
        pods: vec![GpuPod { namespace: "iyakki-ai".into(), name: "lemonade-1".into(), how: "iyakki.dev/gpu x1".into() }],
        pods_note: String::new(),
        last_test: Some("48.0 tokens/s".into()),
        stage1: Some("RADV".into()),
        stage3: None,
    };
    let t: String = report_lines(&r).iter().map(|(_, l)| l.clone()).collect::<Vec<_>>().join("\n");
    for needle in ["GeForce RTX 4090", "Radeon 780M", "NVK", "Firmware the kernel could not load: nvidia/ad102", "Vulkan for local models: RADV", "NVIDIA driver (CUDA): not installed", "Pod iyakki-ai/lemonade-1", "Last test prompt: 48.0 tokens/s", "NVIDIA open modules: present"] {
        assert!(t.contains(needle), "missing `{needle}` in:\n{t}");
    }
    let none = report_lines(&Report::default());
    assert!(none[0].1.contains("No graphics device"));
    assert!(!report_lines(&Report { gpus: vec![gpu_of(AMD, 0x15bf, Some("amdgpu"), true)], ..Default::default() }).iter().any(|(_, l)| l.contains("NVIDIA driver (CUDA)")), "the NVIDIA line only with an NVIDIA GPU");
}
