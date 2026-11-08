use super::*;
use crate::gpucdi::tests::tmp;
use crate::quickinstall::DepState;
use std::sync::Mutex;

/// A machine and a cluster that do what the real ones would, in a folder.
struct Fake {
    root: PathBuf,
    data: PathBuf,
    k3s_data: PathBuf,
    lines: Vec<String>,
    runs: Vec<String>,
    kubectl: Vec<String>,
    applied: Vec<(String, String)>,
    downloads: usize,
    restarts: usize,
    marks: Vec<(Item, String)>,
    /// What the smoke pod prints.
    pod_log: String,
    pod_phase: String,
    lemonade: bool,
    /// The node reports this many GPUs after this many looks.
    node_gpus: u32,
    node_after: usize,
    node_looks: usize,
    /// modprobe nvidia puts this version in /proc/driver/nvidia/version.
    driver_version: String,
    slept: Duration,
    ready: bool,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let root = tmp(tag);
        let data = root.join("var/lib/iyakki");
        let k3s_data = data.join("k3s-dev/data");
        std::fs::create_dir_all(root.join("proc")).unwrap();
        std::fs::write(root.join("proc/modules"), "").unwrap();
        Fake { root, data, k3s_data, lines: vec![], runs: vec![], kubectl: vec![], applied: vec![], downloads: 0, restarts: 0, marks: vec![], pod_log: String::new(), pod_phase: "Succeeded".into(), lemonade: false, node_gpus: 1, node_after: 0, node_looks: 0, driver_version: nv::NVIDIA_VERSION.into(), slept: Duration::ZERO, ready: true }
    }
    fn pci(&self, addr: &str, vendor: &str, device: &str, driver: Option<&str>, render: Option<&str>) {
        let d = self.root.join("sys/bus/pci/devices").join(addr);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("class"), "0x030000\n").unwrap();
        std::fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(d.join("device"), format!("{device}\n")).unwrap();
        if let Some(drv) = driver {
            let t = self.root.join("sys/bus/pci/drivers").join(drv);
            std::fs::create_dir_all(&t).unwrap();
            std::os::unix::fs::symlink(&t, d.join("driver")).unwrap();
        }
        if let Some(r) = render {
            std::fs::create_dir_all(d.join("drm").join(r)).unwrap();
            std::fs::create_dir_all(self.root.join("dev/dri")).unwrap();
            std::fs::write(self.root.join("dev/dri").join(r), b"").unwrap();
        }
    }
    fn amd(&self) {
        self.pci("0000:65:00.0", "0x1002", "0x15bf", Some("amdgpu"), Some("renderD128"));
    }
    fn nvidia(&self, driver: Option<&str>) {
        self.pci("0000:01:00.0", "0x10de", "0x2684", driver, Some("renderD129"));
        std::fs::create_dir_all(self.root.join("lib/modules/6.12.112/updates")).unwrap();
        std::fs::write(self.root.join("lib/modules/6.12.112/updates/nvidia.ko"), b"x").unwrap();
    }
    fn said(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
    fn smi(&self) -> String {
        format!("| NVIDIA-SMI {v}   Driver Version: {v}   CUDA Version: 13.3 |\n|   0  NVIDIA GeForce RTX 4090   Off |\n", v = nv::NVIDIA_VERSION)
    }
}

fn out(code: i32, stdout: &str) -> Out {
    Out { code: Some(code), stdout: stdout.into(), stderr: String::new(), timed_out: false, truncated: false, missing: false, status_lost: false }
}

impl GpuEnv for Fake {
    fn root(&self) -> PathBuf {
        self.root.clone()
    }
    fn data(&self) -> PathBuf {
        self.data.clone()
    }
    fn k3s_data(&self) -> PathBuf {
        self.k3s_data.clone()
    }
    fn dmesg(&mut self) -> String {
        String::new()
    }
    fn run(&mut self, argv: &[String], _t: Duration) -> Out {
        self.runs.push(argv.join(" "));
        match argv[0].as_str() {
            "modprobe" if argv[1] == "-r" => {
                let m = argv[2].replace('-', "_");
                let t = std::fs::read_to_string(self.root.join("proc/modules")).unwrap();
                std::fs::write(self.root.join("proc/modules"), t.lines().filter(|l| !l.starts_with(&format!("{m} "))).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
                out(0, "")
            }
            "modprobe" => {
                let m = argv[1].replace('-', "_");
                let mut t = std::fs::read_to_string(self.root.join("proc/modules")).unwrap();
                t.push_str(&format!("{m} 1 0 - Live 0x0\n"));
                std::fs::write(self.root.join("proc/modules"), t).unwrap();
                if m == "nvidia" {
                    let p = self.root.join("proc/driver/nvidia");
                    std::fs::create_dir_all(p.join("gpus/0000:01:00.0")).unwrap();
                    std::fs::write(p.join("version"), format!("NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  {}  Release Build\n", self.driver_version)).unwrap();
                    std::fs::write(p.join("gpus/0000:01:00.0/information"), "GPU UUID: \t GPU-abc\nDevice Minor: \t 0\n").unwrap();
                    std::fs::write(self.root.join("proc/devices"), "Character devices:\n195 nvidia\n504 nvidia-uvm\n").unwrap();
                }
                out(0, "")
            }
            "mknod" => {
                std::fs::create_dir_all(PathBuf::from(&argv[3]).parent().unwrap()).unwrap();
                std::fs::write(&argv[3], b"").unwrap();
                out(0, "")
            }
            "sh" => {
                let target = PathBuf::from(argv.last().unwrap());
                crate::gpu_nvidia::tests::fake_package(&target, nv::NVIDIA_VERSION);
                out(0, "")
            }
            other => out(127, &format!("{other}: not found")),
        }
    }
    fn download_run(&mut self, l: &nv::Layout) -> Result<(), String> {
        self.downloads += 1;
        std::fs::create_dir_all(&l.root).unwrap();
        std::fs::write(l.run(), b"#!/bin/sh\n").unwrap();
        Ok(())
    }
    fn kubectl(&mut self, args: &[&str]) -> Out {
        let line = args.join(" ");
        self.kubectl.push(line.clone());
        if line.contains("get pod") && line.contains("jsonpath") {
            return out(0, &self.pod_phase.clone());
        }
        if line.contains(" logs ") && line.contains("gpu-smoke") {
            return out(0, &self.pod_log.clone());
        }
        // Ollama is not in the cluster in these tests (its GPU patch has its own tests in quickinstall::ollama_cluster_tests)
        if line.contains("get deployment ollama") {
            return out(1, "");
        }
        if line.contains("get deployment lemonade") {
            return out(if self.lemonade { 0 } else { 1 }, "deployment.apps/lemonade");
        }
        if line.contains("get nodes") {
            self.node_looks += 1;
            let n = if self.node_looks > self.node_after { self.node_gpus } else { 0 };
            return out(0, &format!("{{\"items\":[{{\"status\":{{\"allocatable\":{{\"nvidia.com/gpu\":\"{n}\",\"iyakki.dev/gpu\":\"{n}\"}}}}}}]}}"));
        }
        out(0, "")
    }
    fn request_restart(&mut self) -> Result<(), String> {
        self.restarts += 1;
        // k3s starts again and renders its configuration with the drop-ins
        let d = crate::gpucdi::containerd_dir(&self.k3s_data);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.toml"), "version = 3\nimports = [\"/x/config-v3.toml.d/*.toml\"]\n[plugins.'io.containerd.cri.v1.runtime']\n  enable_cdi = true\n").unwrap();
        Ok(())
    }
    fn published_checksum(&mut self) -> Option<String> {
        None
    }
}

impl StepEnv for Fake {
    fn gpu(&mut self) -> Option<&mut dyn GpuEnv> {
        Some(self)
    }
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, _d: Item) -> DepState {
        DepState::Ready
    }
    fn k3s_present(&mut self) -> bool {
        true
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Ok(String::new())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        self.ready
    }
    fn install_cli(&mut self, _id: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        self.applied.push((name.into(), manifest.into()));
        Ok(format!("{name} configured"))
    }
    fn rollout(&mut self, _n: &str, _name: &str, _t: Duration) -> Result<(), String> {
        Ok(())
    }
    fn sleep(&mut self, d: Duration) {
        self.slept += d;
    }
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        crate::quickinstall::mark_done(&self.data, item, detail)?;
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, _w: &str) -> Result<(), String> {
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        false
    }
    fn lemonade_pull(&mut self, _m: &str) -> Result<(), String> {
        Ok(())
    }
}

const W: Duration = Duration::from_secs(60);
const P: Duration = Duration::from_secs(5);

fn vulkan_log() -> String {
    "ggml_vulkan: Found 1 Vulkan devices:\nVulkan0: AMD Radeon 780M Graphics (RADV PHOENIX) (radv) | uma: 1\n".into()
}

// ---- stage 1

#[test]
fn stage_1_deploys_the_device_plugin_tests_with_an_unprivileged_pod_and_gives_lemonade_the_gpu() {
    let mut f = Fake::new("s1-ok");
    f.amd();
    f.lemonade = true;
    f.pod_log = vulkan_log();
    f.node_after = 1;
    run_vulkan(&mut f, W, P).unwrap();
    // the render node is openable by pods
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(f.root.join("dev/dri/renderD128")).unwrap().permissions().mode() & 0o777, 0o666);
    }
    // the device plugin, applied first, with the render node as a group; no restart of the cluster, no CDI
    assert_eq!(f.applied[0].0, "gpu-device-plugin");
    assert!(f.applied[0].1.contains("path: /dev/dri/renderD128") && f.applied[0].1.contains("--domain=iyakki.dev"));
    assert_eq!(f.restarts, 0, "stage 1 does not restart the cluster");
    assert!(!f.root.join("var/run/cdi").exists(), "stage 1 writes no CDI spec");
    assert!(!f.k3s_data.join("agent").exists(), "and no containerd drop-in");
    assert!(f.node_looks >= 2, "it waited until the node reported iyakki.dev/gpu");
    // the smoke pod: namespace + pod applied, the log read, the pod deleted again
    let (name, manifest) = &f.applied[1];
    assert_eq!(name, "gpu-smoke-vulkan");
    assert!(manifest.contains("kind: Namespace") && manifest.contains("iyakki.dev/gpu: 1"));
    assert!(f.kubectl.iter().any(|k| k.contains("logs gpu-smoke-vulkan")) && f.kubectl.iter().any(|k| k.contains("delete pod gpu-smoke-vulkan")));
    // Lemonade
    let patch = f.kubectl.iter().find(|k| k.contains("patch deployment lemonade")).expect("Lemonade is patched");
    assert!(patch.contains("iyakki.dev/gpu") && patch.contains("vulkan") && patch.contains("supplementalGroups"), "{patch}");
    // the end
    assert!(f.said("[0/6]") && f.said("[6/6] done") && f.said("Radeon 780M") && f.said("usable:"));
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::GpuVulkan);
    assert!(f.marks[0].1.contains("renderD128") && f.marks[0].1.contains("iyakki.dev/gpu"), "{:?}", f.marks);
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn stage_1_without_lemonade_says_so_and_does_not_patch() {
    let mut f = Fake::new("s1-nolem");
    f.amd();
    f.pod_log = vulkan_log();
    run_vulkan(&mut f, W, P).unwrap();
    assert!(f.said("Lemonade is not installed"), "no Lemonade, no patch: said, not an error");
    assert!(!f.kubectl.iter().any(|k| k.contains("patch deployment")));
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn stage_1_stops_with_the_reason_when_no_gpu_can_do_vulkan() {
    let mut f = Fake::new("s1-none");
    assert!(run_vulkan(&mut f, W, P).unwrap_err().contains("no graphics device"));
    assert!(f.applied.is_empty() && f.restarts == 0, "nothing was changed");
    let mut f = Fake::new("s1-nodrv");
    f.pci("0000:01:00.0", "0x10de", "0x2684", None, None);
    let e = run_vulkan(&mut f, W, P).unwrap_err();
    assert!(e.contains("none of the GPUs can run Vulkan"), "{e}");
    assert!(f.said("not usable:") && f.said("no render node"));
    assert!(f.applied.is_empty());
}

#[test]
fn stage_1_fails_when_the_pod_only_sees_the_software_renderer_or_nothing() {
    let mut f = Fake::new("s1-soft");
    f.amd();
    f.pod_log = "Vulkan0: llvmpipe (LLVM 15.0.7, 256 bits) (cpu)\n".into();
    let e = run_vulkan(&mut f, W, P).unwrap_err();
    assert!(e.contains("software renderer"), "{e}");
    assert!(f.marks.is_empty(), "a failed test does not mark the step done");
    let mut f = Fake::new("s1-none-seen");
    f.amd();
    f.pod_log = "Available devices:\n  (none)\n".into();
    assert!(run_vulkan(&mut f, W, P).unwrap_err().contains("no Vulkan device"));
}

#[test]
fn stage_1_gives_up_with_the_plugins_log_when_the_node_never_shows_the_resource() {
    let mut f = Fake::new("s1-node");
    f.amd();
    f.node_gpus = 0;
    let e = run_vulkan(&mut f, W, P).unwrap_err();
    assert!(e.contains("does not report iyakki.dev/gpu"), "{e}");
    assert!(f.kubectl.iter().any(|k| k.contains("logs daemonset/iyakki-gpu-device-plugin")));
    assert!(f.marks.is_empty());
}

#[test]
fn stage_1_gives_up_with_the_pods_events_when_the_pod_never_finishes() {
    let mut f = Fake::new("s1-stuck");
    f.amd();
    f.pod_phase = "Pending".into();
    let e = run_vulkan(&mut f, Duration::from_secs(60), P).unwrap_err();
    assert!(e.contains("did not finish") && e.contains("Pending"), "{e}");
    assert!(f.kubectl.iter().any(|k| k.contains("describe pod")));
    assert!(f.slept >= Duration::from_secs(1500), "it waited the whole time first: {:?}", f.slept);
}

// ---- stage 3

fn nvidia_ready_machine(tag: &str) -> Fake {
    let f = Fake::new(tag);
    f.nvidia(None);
    f
}

#[test]
fn stage_3_runs_every_step_in_order_and_ends_with_a_pod_that_runs_nvidia_smi() {
    let mut f = nvidia_ready_machine("s3-ok");
    f.lemonade = true;
    f.pod_log = f.smi();
    f.node_after = 2;
    run_nvidia(&mut f, W, P).unwrap();
    for step in ["[0/9]", "[1/9]", "[2/9] downloading", "[3/9] download verified", "[4/9]", "[5/9]", "[6/9]", "[7/9]", "[8/9]", "[9/9] done"] {
        assert!(f.said(step), "no `{step}` in {:?}", f.lines);
    }
    // the licence notice is the first thing in the log
    assert!(f.lines[0].contains(&format!("version {}", nv::NVIDIA_VERSION)) && f.lines.iter().any(|l| l.contains("accept NVIDIA's licence")));
    assert_eq!(f.downloads, 1);
    // the order of the programs run: unpack, load, load, make nodes
    let kinds: Vec<&str> = f.runs.iter().map(|r| r.split_whitespace().next().unwrap()).collect();
    assert_eq!(kinds, ["sh", "modprobe", "modprobe", "mknod", "mknod", "mknod", "mknod"], "{:?}", f.runs);
    assert!(f.runs[0].contains("--extract-only"), "{}", f.runs[0]);
    assert!(f.runs[1].ends_with("nvidia NVreg_DynamicPowerManagement=0x02") && f.runs[2].ends_with("nvidia-uvm"));
    assert!(f.runs.iter().any(|r| r.contains("/dev/nvidiactl c 195 255")) && f.runs.iter().any(|r| r.contains("/dev/nvidia-uvm c 504 0")));
    // both CDI specs
    assert!(f.root.join("var/run/cdi/nvidia.com-gpu.json").is_file() && f.root.join("var/run/cdi/k8s.device-plugin.nvidia.com-gpu.json").is_file());
    // the plugin, applied after the CDI spec and the containerd setting
    assert_eq!(f.restarts, 1);
    assert_eq!(f.applied[0].0, "nvidia-device-plugin");
    assert!(f.applied[0].1.contains("path: /var/lib/kubelet/device-plugins"));
    assert_eq!(f.applied[1].0, "gpu-smoke-nvidia");
    // the node was polled until it reported the GPU
    assert!(f.node_looks >= 3);
    // Lemonade got the resource
    assert!(f.kubectl.iter().any(|k| k.contains("patch deployment lemonade") && k.contains("nvidia.com/gpu")));
    // cleaned up, marked
    let l = nv::Layout::new(&f.data);
    assert!(l.staged() && !l.run().exists() && !l.extracted().exists());
    assert_eq!(f.marks[0].0, Item::GpuNvidia);
    assert!(f.marks[0].1.contains(nv::NVIDIA_VERSION) && f.marks[0].1.contains("RTX 4090"), "{:?}", f.marks);
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn stage_3_refuses_before_downloading_anything() {
    // no NVIDIA GPU
    let mut f = Fake::new("s3-nogpu");
    f.amd();
    assert!(run_nvidia(&mut f, W, P).unwrap_err().contains("no NVIDIA GPU"));
    assert_eq!(f.downloads, 0);
    // a Pascal card
    let mut f = Fake::new("s3-pascal");
    f.pci("0000:01:00.0", "0x10de", "0x1b80", None, None);
    assert!(run_nvidia(&mut f, W, P).unwrap_err().contains("older than Turing"));
    assert_eq!(f.downloads, 0);
    // the image has no modules
    let mut f = Fake::new("s3-nomod");
    f.pci("0000:01:00.0", "0x10de", "0x2684", None, None);
    assert!(run_nvidia(&mut f, W, P).unwrap_err().contains("no NVIDIA kernel modules"));
    assert_eq!(f.downloads, 0);
    // nouveau holds the card
    let mut f = Fake::new("s3-nouveau");
    f.nvidia(Some("nouveau"));
    std::fs::write(f.root.join("proc/modules"), "nouveau 1 0 - Live 0x0\n").unwrap();
    let e = run_nvidia(&mut f, W, P).unwrap_err();
    assert!(e.contains("modprobe.blacklist=nouveau"), "{e}");
    assert_eq!(f.downloads, 0);
    assert!(f.runs.is_empty() && f.applied.is_empty() && f.restarts == 0, "nothing was touched");
}

#[test]
fn stage_3_does_not_download_again_when_the_stage_is_there() {
    let mut f = nvidia_ready_machine("s3-staged");
    let l = nv::Layout::new(&f.data);
    crate::gpu_nvidia::tests::fake_package(&l.extracted(), nv::NVIDIA_VERSION);
    nv::stage(&l.extracted(), &l).unwrap();
    f.pod_log = f.smi();
    run_nvidia(&mut f, W, P).unwrap();
    assert_eq!(f.downloads, 0);
    assert!(f.said("staged already") && !f.runs.iter().any(|r| r.starts_with("sh ")));
}

#[test]
fn stage_3_stops_when_the_loaded_driver_is_not_the_pinned_version() {
    let mut f = nvidia_ready_machine("s3-version");
    f.driver_version = "550.54.14".into();
    let e = run_nvidia(&mut f, W, P).unwrap_err();
    assert!(e.contains("550.54.14") && e.contains("not the pinned"), "{e}");
    assert!(f.applied.is_empty(), "no plugin on a mismatching driver");
}

#[test]
fn stage_3_reports_the_plugin_log_when_the_node_never_shows_the_gpu() {
    let mut f = nvidia_ready_machine("s3-node");
    f.node_gpus = 0;
    let e = run_nvidia(&mut f, W, P).unwrap_err();
    assert!(e.contains("does not report nvidia.com/gpu"), "{e}");
    assert!(f.kubectl.iter().any(|k| k.contains("logs daemonset/nvidia-device-plugin")));
    assert!(f.marks.is_empty());
}

#[test]
fn stage_3_fails_loudly_when_nvidia_smi_in_the_pod_does_not_work() {
    let mut f = nvidia_ready_machine("s3-smi");
    f.pod_log = "Failed to initialize NVML: Driver/library version mismatch".into();
    let e = run_nvidia(&mut f, W, P).unwrap_err();
    assert!(e.contains("did not answer"), "{e}");
    assert!(f.marks.is_empty());
}

// ---- after a start

#[test]
fn after_a_start_the_specs_and_the_driver_come_back_from_the_markers() {
    let mut f = nvidia_ready_machine("restore");
    f.amd();
    // nothing marked: nothing done
    let mut run = |a: &[String]| f.run(a, P);
    assert!(ensure_runtime(&PathBuf::from("/nonexistent"), &PathBuf::from("/nonexistent"), &mut |_a: &[String]| out(0, "")).is_empty());
    let _ = &mut run;
    // the Vulkan step ran before: the render node (made again by devtmpfs at this start) is opened for pods again
    let (root, data) = (f.root.clone(), f.data.clone());
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("dev/dri/renderD128"), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    crate::quickinstall::mark_done(&data, Item::GpuVulkan, "x").unwrap();
    let r = ensure_runtime(&root, &data, &mut |_a: &[String]| out(0, ""));
    assert_eq!(r.len(), 1);
    assert!(r[0].as_ref().unwrap().contains("renderD128"), "{r:?}");
    assert!(ensure_runtime(&root, &data, &mut |_a: &[String]| out(0, "")).is_empty(), "open already: nothing to do");
    // the NVIDIA step ran before: stage there, driver not loaded, spec gone
    let l = nv::Layout::new(&data);
    crate::gpu_nvidia::tests::fake_package(&l.extracted(), nv::NVIDIA_VERSION);
    nv::stage(&l.extracted(), &l).unwrap();
    crate::quickinstall::mark_done(&data, Item::GpuNvidia, "x").unwrap();
    let calls = Mutex::new(Vec::new());
    let root2 = root.clone();
    let r = ensure_runtime(&root, &data, &mut |a: &[String]| {
        calls.lock().unwrap().push(a.join(" "));
        // the real modprobe fills /proc; the fake does it for the one module that matters
        if a[1] == "nvidia" {
            let p = root2.join("proc/driver/nvidia/gpus/0000:01:00.0");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("information"), "Device Minor: 0\n").unwrap();
            std::fs::create_dir_all(root2.join("dev")).unwrap();
            std::fs::write(root2.join("proc/devices"), "Character devices:\n504 nvidia-uvm\n").unwrap();
            if a[0] == "modprobe" {
                std::fs::write(root2.join("dev/nvidiactl"), b"").unwrap();
            }
        }
        out(0, "")
    });
    assert!(r.iter().all(|x| x.is_ok()), "{r:?}");
    let c = calls.lock().unwrap();
    assert!(c.iter().any(|x| x == "modprobe nvidia NVreg_DynamicPowerManagement=0x02") && c.iter().any(|x| x == "modprobe nvidia-uvm"));
    assert!(root.join("var/run/cdi/nvidia.com-gpu.json").is_file());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_nvidia_driver_is_not_loaded_again_while_nouveau_holds_the_card() {
    let f = nvidia_ready_machine("restore-nouveau");
    let l = nv::Layout::new(&f.data);
    crate::gpu_nvidia::tests::fake_package(&l.extracted(), nv::NVIDIA_VERSION);
    nv::stage(&l.extracted(), &l).unwrap();
    crate::quickinstall::mark_done(&f.data, Item::GpuNvidia, "x").unwrap();
    std::fs::write(f.root.join("proc/modules"), "nouveau 1 0 - Live 0x0\n").unwrap();
    let r = ensure_runtime(&f.root, &f.data, &mut |_a: &[String]| panic!("no program may run"));
    assert!(r.is_empty());
    let _ = std::fs::remove_dir_all(&f.root);
}

// ---- the test prompt job

struct Up(Mutex<Vec<String>>);
impl iyakki_proxy::Upstream for Up {
    fn request(&self, _m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        self.0.lock().unwrap().push(url.into());
        if url.starts_with("http://127.0.0.1:13305") {
            return Err("connection refused".into());
        }
        if url.ends_with("/models") {
            return Ok(iyakki_proxy::UpstreamResponse { status: 200, body: br#"{"data":[{"id":"llama3.2:1b"}]}"#.to_vec() });
        }
        Ok(iyakki_proxy::UpstreamResponse { status: 200, body: br#"{"choices":[{"message":{"content":"Rayleigh scattering."}}],"usage":{"completion_tokens":60}}"#.to_vec() })
    }
}

#[test]
fn the_test_job_tries_each_server_and_keeps_the_result() {
    let d = tmp("testjob");
    let t = Mutex::new(vec![10.0, 12.0]);
    let mut lines = Vec::new();
    let bases = test_endpoints(&["http://127.0.0.1:13305/api/v1".to_string()]);
    assert_eq!(bases.last().map(String::as_str), Some("http://127.0.0.1:11435/v1"), "the GPU Ollama is the second place");
    let r = run_test(&d, &bases, &Up(Mutex::new(vec![])), &|| t.lock().unwrap().remove(0), &mut |l| lines.push(l.to_string())).unwrap();
    assert_eq!(r.model, "llama3.2:1b");
    assert!((r.tokens_per_second - 30.0).abs() < 1e-9);
    assert!(lines.iter().any(|l| l.contains("connection refused")), "{lines:?}");
    assert!(lines.last().unwrap().contains("30.0 tokens/s"));
    assert_eq!(crate::gpu::last_test(&d).as_deref(), Some("30.0 tokens/s (60 tokens in 2.0 s, model llama3.2:1b)"));
    // nobody answers
    let e = run_test(&d, &["http://127.0.0.1:13305/api/v1".to_string()], &Up(Mutex::new(vec![])), &|| 0.0, &mut |_| {}).unwrap_err();
    assert!(e.contains("connection refused"));
    let _ = std::fs::remove_dir_all(&d);
}

// ---- the Quick install entries

#[test]
fn the_gpu_entries_are_in_local_models_after_k3s_and_the_nvidia_one_hides_without_an_nvidia_gpu() {
    use crate::quickinstall::{items_of, order, Group};
    assert_eq!(items_of(Group::LocalModels), [Item::Lemonade, Item::Ollama, Item::MacHost, Item::GpuVulkan, Item::GpuNvidia]);
    assert_eq!(Item::GpuVulkan.title(), "GPU for local models (Vulkan)");
    assert_eq!(Item::GpuNvidia.title(), "NVIDIA GPU driver (CUDA)");
    assert_eq!(order(Item::GpuVulkan, &|_| false), [Item::K3s, Item::GpuVulkan]);
    assert_eq!(order(Item::GpuNvidia, &|i| i == Item::K3s), [Item::GpuNvidia]);
    assert!(Item::GpuVulkan.runs_as_job() && Item::GpuNvidia.runs_as_job());
    assert_eq!(Item::parse("gpu-nvidia"), Some(Item::GpuNvidia));
    // the review text shows the licence first and what is downloaded with its size
    assert!(Item::GpuNvidia.about().contains("about 400 MB") && Item::GpuNvidia.about().contains("download.nvidia.com"));
    assert!(Item::GpuNvidia.licence().contains("proprietary") && Item::GpuNvidia.licence().contains("MIT / GPL-2.0"));
    let amd = crate::gpu::Gpu { pci: "0000:65:00.0".into(), vendor_id: 0x1002, device_id: 0x15bf, vendor: crate::gpu::Vendor::Amd, name: "x".into(), driver: None, card: None, render: None, boot_vga: false, vram: None };
    let nvd = crate::gpu::Gpu { vendor_id: 0x10de, vendor: crate::gpu::Vendor::Nvidia, ..amd.clone() };
    assert_eq!(crate::quickinstall::hidden_items(&[amd.clone()]), [Item::GpuNvidia]);
    assert_eq!(crate::quickinstall::hidden_items(&[]), [Item::GpuNvidia]);
    assert!(crate::quickinstall::hidden_items(&[amd, nvd]).is_empty());
}

#[test]
fn the_steps_run_as_detached_jobs_of_the_installer_workspace_with_logs() {
    let l = crate::quickinstall::Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    let s = crate::quickinstall::spec(Item::GpuNvidia, 2, 2, &l);
    assert_eq!(s.workspace, crate::quickinstall::WORKSPACE, "the top bar's installer indicator counts this workspace");
    assert_eq!(s.label, "Install NVIDIA GPU driver (CUDA) (2/2, after k3s)");
    assert_eq!(s.cmd, crate::jobs::Cmd::Argv(vec!["/usr/bin/iyakki".into(), "quick-install".into(), "gpu-nvidia".into(), "--data".into(), "/var/lib/iyakki".into()]));
}

#[test]
fn the_nvidia_card_can_be_switched_off_and_on_and_stays_off_across_a_start() {
    let mut f = nvidia_ready_machine("s3-sleep");
    f.pod_log = f.smi();
    run_nvidia(&mut f, W, P).unwrap();
    f.runs.clear();
    f.kubectl.clear();
    f.applied.clear();
    f.lines.clear();
    run_nvidia_power(&mut f, true, W, P).unwrap();
    // the plugin goes first, then the modules come out (the main one last), and a file keeps it that way
    assert!(f.kubectl.iter().any(|k| k.contains("delete daemonset nvidia-device-plugin")), "{:?}", f.kubectl);
    assert_eq!(f.runs, ["modprobe -r nvidia-uvm", "modprobe -r nvidia-drm", "modprobe -r nvidia-modeset", "modprobe -r nvidia"]);
    assert!(nv::sleep_marker(&f.data).is_file());
    assert!(f.said("[3/3] done") && f.said("0000:01:00.0"), "{:?}", f.lines);
    // a start does not load the driver again while it is off
    let mut loaded = Vec::new();
    let r = ensure_runtime(&f.root, &f.data, &mut |a| {
        loaded.push(a.join(" "));
        out(0, "")
    });
    assert!(r.is_empty() && loaded.is_empty(), "{r:?} {loaded:?}");
    // on again: the file goes, the driver loads, the plugin is deployed
    f.runs.clear();
    f.lines.clear();
    run_nvidia_power(&mut f, false, W, P).unwrap();
    assert!(!nv::sleep_marker(&f.data).exists());
    assert!(f.runs.iter().any(|r| r.starts_with("modprobe nvidia NVreg_")), "{:?}", f.runs);
    assert!(f.applied.iter().any(|(n, _)| n == "nvidia-device-plugin"));
}

#[test]
fn switching_the_nvidia_card_off_before_the_driver_is_installed_is_refused() {
    let mut f = nvidia_ready_machine("s3-sleep-none");
    let e = run_nvidia_power(&mut f, true, W, P).unwrap_err();
    assert!(e.contains("not installed"), "{e}");
}

#[test]
fn the_firmware_search_path_is_set_at_every_start_even_when_the_cdi_specs_are_already_there() {
    // the kernel loads the nvidia module by itself early in the boot; the GSP firmware is asked for when the card is first opened
    let mut f = nvidia_ready_machine("fwpath");
    f.pod_log = f.smi();
    run_nvidia(&mut f, W, P).unwrap();
    let param = f.root.join("sys/module/firmware_class/parameters/path");
    std::fs::create_dir_all(param.parent().unwrap()).unwrap();
    std::fs::write(&param, "\n").unwrap();
    assert!(f.root.join("var/run/cdi/nvidia.com-gpu.json").is_file(), "the specs are there, as after a restart that kept them");
    let r = ensure_runtime(&f.root, &f.data, &mut |_a: &[String]| out(0, ""));
    let want = format!("{}\n", nv::Layout::new(&f.data).firmware().display());
    assert_eq!(std::fs::read_to_string(&param).unwrap(), want, "{r:?}");
    assert!(r.iter().any(|x| x.as_ref().is_ok_and(|l| l.contains("firmware search path"))), "{r:?}");
    // and nothing is written when it is right already
    assert!(ensure_runtime(&f.root, &f.data, &mut |_a: &[String]| out(0, "")).iter().all(|x| !x.as_ref().is_ok_and(|l| l.contains("firmware search path"))));
}

#[test]
fn lemonade_defaults_to_the_gpu_and_a_cpu_choice_is_kept_and_respected() {
    let mut f = Fake::new("compute-choice");
    f.amd();
    f.lemonade = true;
    f.pod_log = vulkan_log();
    f.node_after = 1;
    assert_eq!(lemonade_compute(&f.data), Compute::Gpu, "with nothing chosen it is the GPU");
    set_lemonade_compute(&f.data, Compute::Cpu).unwrap();
    assert_eq!(lemonade_compute(&f.data), Compute::Cpu);
    run_vulkan(&mut f, W, P).unwrap();
    assert!(!f.kubectl.iter().any(|k| k.contains("patch deployment lemonade")), "the GPU step leaves a CPU Lemonade alone");
    assert!(f.said("runs on the CPU") || f.said("run on the CPU"));
    // the choice applied now: GPU patches, CPU takes it away
    f.kubectl.clear();
    let m = apply_lemonade_compute(&mut f, Compute::Gpu).unwrap();
    assert!(m.contains("GPU") && f.kubectl.iter().any(|k| k.contains("patch deployment lemonade") && k.contains("nvidia.com/gpu")), "{m} {:?}", f.kubectl);
    f.kubectl.clear();
    let m = apply_lemonade_compute(&mut f, Compute::Cpu).unwrap();
    assert!(m.contains("CPU") && f.kubectl.iter().any(|k| k.contains("patch deployment lemonade") && k.contains("null")), "{m} {:?}", f.kubectl);
    let _ = std::fs::remove_dir_all(&f.root);
}

#[test]
fn a_lemonade_deployed_after_the_gpu_step_gets_the_gpu_again_unless_the_cpu_was_chosen() {
    let mut f = Fake::new("compute-reapply");
    f.amd();
    f.lemonade = true;
    f.node_after = 0;
    reapply_lemonade_gpu(&mut f);
    assert!(f.kubectl.iter().any(|k| k.contains("patch deployment lemonade")), "{:?}", f.kubectl);
    f.kubectl.clear();
    set_lemonade_compute(&f.data, Compute::Cpu).unwrap();
    reapply_lemonade_gpu(&mut f);
    assert!(!f.kubectl.iter().any(|k| k.contains("patch deployment lemonade")));
    // no GPU step installed: nothing to give, said plainly
    f.node_gpus = 0;
    let m = apply_lemonade_compute(&mut f, Compute::Gpu).unwrap();
    assert!(m.contains("no GPU step"), "{m}");
    let _ = std::fs::remove_dir_all(&f.root);
}
