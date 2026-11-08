//! The two GPU steps of Quick install (Local models group) and the test prompt (F-GPU-01, F-GPU-03; docs/GPU-K3S.md).
//!
//! * [`run_vulkan`] ("GPU for local models (Vulkan)", stage 1): looks at the GPUs, waits for the cluster, makes the render nodes
//!   openable, deploys a generic device plugin that advertises them as `iyakki.dev/gpu`, runs a smoke pod that asks for one and lists the
//!   Vulkan devices, and gives Lemonade its GPU settings when it is deployed. No CDI and no restart of the cluster.
//! * [`run_nvidia`] ("NVIDIA GPU driver (CUDA)", stage 3): refuses early when the card or the image cannot do it, downloads the vendor
//!   files at the person's click, loads the open modules, makes the device nodes, writes the CDI spec, deploys the device plugin, waits for
//!   `nvidia.com/gpu` on the node and runs `nvidia-smi` in a pod.
//! * [`ensure_runtime`] puts back, after a start, what lives in RAM: the CDI specs, the module and the device nodes.
//! * [`run_test`] sends the test prompt and reports tokens per second.
//!
//! Everything goes through [`crate::quickinstall::StepEnv`] (the cluster, the clock, the log) and [`GpuEnv`] (files under a root, programs,
//! the download); the tests give both a fake.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::gpu;
use crate::gpu_nvidia as nv;
use crate::gpu_workloads as wl;
use crate::gpucdi;
use crate::quickinstall::{Item, StepEnv};
use crate::wsexec::Out;

/// What the GPU steps do to the machine besides the cluster.
pub trait GpuEnv {
    /// The file system root (`/`; a folder in tests).
    fn root(&self) -> PathBuf;
    /// The data folder (`/var/lib/iyakki`).
    fn data(&self) -> PathBuf;
    /// The managed k3s's `--data-dir`.
    fn k3s_data(&self) -> PathBuf;
    /// The kernel log.
    fn dmesg(&mut self) -> String;
    /// Run a program (modprobe, sh, mknod): argv, timeout.
    fn run(&mut self, argv: &[String], timeout: Duration) -> Out;
    /// Download, verify and keep the vendor `.run` at `layout.run()`; progress goes to the log.
    fn download_run(&mut self, layout: &nv::Layout) -> Result<(), String>;
    /// `kubectl` against the managed cluster.
    fn kubectl(&mut self, args: &[&str]) -> Out;
    /// Ask the supervisor to restart the managed cluster.
    fn request_restart(&mut self) -> Result<(), String>;
    /// NVIDIA's published checksum text for the download, when it can be fetched (`None`: not reachable; the pinned value still applies).
    fn published_checksum(&mut self) -> Option<String>;
}

fn gpu_env<'a>(env: &'a mut dyn StepEnv) -> Result<&'a mut dyn GpuEnv, String> {
    env.gpu().ok_or_else(|| "the GPU steps are not available here".to_string())
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn dev_dri(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root.join("dev/dri")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("renderD")).collect();
    v.sort();
    v
}

fn cdi_dir(root: &Path) -> PathBuf {
    root.join(gpucdi::SPEC_DIR.trim_start_matches('/'))
}

/// Wait until the cluster's containerd reads CDI specs (the drop-ins are written and k3s has rendered its configuration with them) and the
/// node is Ready again. Asks for a restart once when the drop-ins are new.
fn make_containerd_read_cdi(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    let (k3s_data, ready_cdi) = {
        let g = gpu_env(env)?;
        (g.k3s_data(), gpucdi::cluster_cdi_enabled(&g.k3s_data()))
    };
    let changed = gpucdi::ensure_dropins(&k3s_data)?;
    if ready_cdi && !changed {
        env.say("the cluster's containerd reads CDI specs already");
        return Ok(());
    }
    env.say(if changed { "containerd needs a small setting (CDI on): written as a drop-in in the cluster's data folder; the cluster restarts once" } else { "the cluster has not read the CDI setting yet; restarting it once" });
    gpu_env(env)?.request_restart()?;
    // a restart takes the API server away for a while: wait for the configuration to be rendered again, then for Ready
    let mut waited = Duration::ZERO;
    env.sleep(poll);
    loop {
        let (enabled, ready) = (gpucdi::cluster_cdi_enabled(&k3s_data), env.cluster_ready());
        if enabled && ready {
            return Ok(());
        }
        if waited >= wait {
            return Err(format!("the cluster did not come back with CDI on after {} minutes (is its containerd configuration in {} rendered with the drop-in? docs/GPU-K3S.md, troubleshooting)", wait.as_secs() / 60, gpucdi::containerd_dir(&k3s_data).display()));
        }
        env.sleep(poll);
        waited += poll;
    }
}

/// Create a smoke pod, wait for it to finish, read its log, delete it. Returns the log.
fn run_smoke_pod(env: &mut dyn StepEnv, g: wl::Gpu, timeout: Duration, poll: Duration) -> Result<String, String> {
    let (name, manifest) = wl::smoke_pod(g);
    let _ = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "delete", "pod", name, "--ignore-not-found", "--wait=true"]);
    env.kubectl_apply(name, &wl::namespace_and(&manifest))?;
    let mut waited = Duration::ZERO;
    let phase = loop {
        let out = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "get", "pod", name, "-o", "jsonpath={.status.phase}"]);
        let p = out.stdout.trim().to_string();
        if p == "Succeeded" || p == "Failed" {
            break p;
        }
        if waited >= timeout {
            let d = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "describe", "pod", name]);
            let tail: Vec<&str> = d.stdout.lines().rev().take(8).collect();
            let _ = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "delete", "pod", name, "--ignore-not-found", "--wait=false"]);
            return Err(format!("the smoke pod did not finish in {} minutes (phase {}); its last events: {}", timeout.as_secs() / 60, if p.is_empty() { "unknown" } else { &p }, tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
        }
        env.sleep(poll);
        waited += poll;
    };
    let log = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "logs", name]);
    let text = format!("{}{}", log.stdout, log.stderr);
    let _ = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "delete", "pod", name, "--ignore-not-found", "--wait=false"]);
    if phase == "Failed" && text.trim().is_empty() {
        return Err("the smoke pod failed and printed nothing".into());
    }
    Ok(text)
}

fn say_lines(env: &mut dyn StepEnv, text: &str, max: usize) {
    for l in text.lines().filter(|l| !l.trim().is_empty()).take(max) {
        env.say(&format!("  {}", l.trim()));
    }
}

/// Is a Lemonade Deployment in the cluster?
fn lemonade_deployed(env: &mut dyn StepEnv) -> bool {
    match gpu_env(env) {
        Ok(g) => g.kubectl(&["-n", wl::NAMESPACE, "get", "deployment", "lemonade", "-o", "name"]).code == Some(0),
        Err(_) => false,
    }
}

/// Is the Ollama Deployment of Quick install in the cluster?
fn ollama_deployed(env: &mut dyn StepEnv) -> bool {
    match gpu_env(env) {
        Ok(g) => g.kubectl(&["-n", wl::NAMESPACE, "get", "deployment", crate::quickinstall::ollama_cluster::NAME, "-o", "name"]).code == Some(0),
        Err(_) => false,
    }
}

/// Give the Ollama Deployment the GPU the same way Lemonade gets it (a patch; a failure is a note, not an error).
fn patch_ollama(env: &mut dyn StepEnv, g: wl::Gpu, gid: Option<u32>) {
    if !ollama_deployed(env) {
        return;
    }
    let Ok(ge) = gpu_env(env) else { return };
    let out = ge.kubectl(&["-n", wl::NAMESPACE, "patch", "deployment", crate::quickinstall::ollama_cluster::NAME, "--type", "strategic", "-p", &wl::ollama_patch(g, gid)]);
    if out.code == Some(0) {
        env.say(&format!("  Ollama got {} too; it restarts", g.resource()));
    } else {
        env.say(&format!("  note: Ollama could not be given the GPU: {}", out.combined().trim()));
    }
}

// ------------------------------------------------------------------------------------------------ Lemonade: GPU or CPU

/// Where Lemonade runs its models: on the GPU (the default, when a GPU step is installed) or on the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compute {
    Gpu,
    Cpu,
}

impl Compute {
    pub fn word(self) -> &'static str {
        match self {
            Compute::Gpu => "gpu",
            Compute::Cpu => "cpu",
        }
    }
    pub fn parse(s: &str) -> Option<Compute> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gpu" => Some(Compute::Gpu),
            "cpu" => Some(Compute::Cpu),
            _ => None,
        }
    }
}

fn compute_file(data: &Path) -> PathBuf {
    data.join("lemonade-compute")
}

/// The person's choice for Lemonade (a file on the data volume); with none made it is the GPU.
pub fn lemonade_compute(data: &Path) -> Compute {
    // a computer with its own profile has its own choice (a computer that never chose gets the default, not another computer's choice)
    match crate::hwprofile::extra_get(data, crate::hwprofile::EXTRA_GPU) {
        Some(serde_json::Value::String(w)) => return Compute::parse(&w).unwrap_or(Compute::Gpu),
        Some(_) => return Compute::Gpu,
        None => {}
    }
    std::fs::read_to_string(compute_file(data)).ok().and_then(|t| Compute::parse(&t)).unwrap_or(Compute::Gpu)
}

/// Keep the choice for the next start and the next install.
pub fn set_lemonade_compute(data: &Path, c: Compute) -> Result<(), String> {
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    crate::hwprofile::extra_set(data, crate::hwprofile::EXTRA_GPU, serde_json::Value::String(c.word().to_string()));
    std::fs::write(compute_file(data), format!("{}\n", c.word())).map_err(|e| e.to_string())
}

/// Put Lemonade's Deployment in line with the choice: with `Cpu` the GPU limit and backend are taken away; with `Gpu` it gets the path the cluster
/// advertises (NVIDIA first, else the Vulkan one). Says what it did; nothing to do when Lemonade is not deployed or no GPU step has been run.
pub fn apply_lemonade_compute(env: &mut dyn StepEnv, c: Compute) -> Result<String, String> {
    if !lemonade_deployed(env) {
        return Ok("Lemonade is not installed yet; the choice is kept for when it is".into());
    }
    let (root, nodes_json) = {
        let g = gpu_env(env)?;
        (g.root(), g.kubectl(&["get", "nodes", "-o", "json"]).stdout)
    };
    let patch = match c {
        Compute::Cpu => wl::lemonade_unpatch(),
        Compute::Gpu => {
            if wl::node_resource_count(&nodes_json, wl::NVIDIA_RESOURCE) > 0 {
                wl::lemonade_patch(wl::Gpu::Nvidia, None)
            } else if wl::node_resource_count(&nodes_json, wl::GPU_RESOURCE) > 0 {
                let nodes: Vec<String> = gpu::vulkan_usable(&gpu::detect(&root)).iter().filter_map(|g| g.render.clone()).collect();
                let (_, gid) = open_render_nodes(&root, &nodes);
                wl::lemonade_patch(wl::Gpu::Vulkan, gid)
            } else {
                return Ok("no GPU step is installed yet; Lemonade stays on the CPU until it is (Store -> Setup)".into());
            }
        }
    };
    let out = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "patch", "deployment", "lemonade", "--type", "strategic", "-p", &patch]);
    if out.code != Some(0) {
        return Err(format!("could not change where Lemonade runs: {}", out.combined().trim()));
    }
    Ok(match c {
        Compute::Gpu => "Lemonade uses the GPU; it restarts".into(),
        Compute::Cpu => "Lemonade uses the CPU; it restarts".into(),
    })
}

/// After Lemonade was (re)deployed: give it the GPU again when the person's choice is the GPU and a GPU step is installed (applying the
/// manifest again removes the patch). A note, never an error.
pub fn reapply_lemonade_gpu(env: &mut dyn StepEnv) {
    let Some(data) = env.gpu().map(|g| g.data()) else { return };
    if lemonade_compute(&data) != Compute::Gpu {
        return;
    }
    match apply_lemonade_compute(env, Compute::Gpu) {
        Ok(m) => env.say(&format!("  {m}")),
        Err(e) => env.say(&format!("  note: {e}")),
    }
}

// ------------------------------------------------------------------------------------------------ stage 1

/// Make the DRM render nodes openable by an unprivileged user (mode 0666, as render nodes are on most systems: the kernel checks the GPU
/// itself, a render node cannot change the display). Returns the nodes changed, and the group of the first (for pods that run as a user).
pub fn open_render_nodes(root: &Path, nodes: &[String]) -> (Vec<String>, Option<u32>) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut changed = Vec::new();
    let mut gid = None;
    for n in nodes {
        let p = root.join("dev/dri").join(n);
        let Ok(m) = std::fs::metadata(&p) else { continue };
        gid.get_or_insert(m.gid());
        if m.permissions().mode() & 0o006 != 0o006 && std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).is_ok() {
            changed.push(n.clone());
        }
    }
    (changed, gid)
}

/// "GPU for local models (Vulkan)".
pub fn run_vulkan(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    let (root, dmesg) = {
        let g = gpu_env(env)?;
        (g.root(), g.dmesg())
    };
    env.say(&format!("[0/6] looking at the graphics hardware (read-only: /sys/bus/pci/devices); licence: {}", Item::GpuVulkan.licence()));
    let gpus = gpu::detect(&root);
    if gpus.is_empty() {
        return Err("no graphics device found on this machine".into());
    }
    for g in &gpus {
        let (ok, why) = gpu::vulkan_verdict(g);
        env.say(&format!("  {} {}\n      {why}", if ok { "usable:" } else { "not usable:" }, g.line()));
    }
    let usable = gpu::vulkan_usable(&gpus);
    if usable.is_empty() {
        let missing = gpu::missing_firmware(&dmesg);
        let more = if missing.is_empty() { String::new() } else { format!(" The kernel could not load: {}.", missing.join(", ")) };
        return Err(format!("none of the GPUs can run Vulkan for pods yet (reasons above).{more}"));
    }
    env.say("[1/6] checking the cluster");
    crate::quickinstall::wait_for_cluster(env, wait, poll)?;
    let nodes: Vec<String> = usable.iter().filter_map(|g| g.render.clone()).collect();
    env.say(&format!("[2/6] making the render nodes openable by pods ({})", nodes.join(", ")));
    let (changed, gid) = open_render_nodes(&root, &nodes);
    if !changed.is_empty() {
        env.say(&format!("  mode 0666 now: {}", changed.join(", ")));
    }
    env.say(&format!("[3/6] deploying the generic device plugin (Apache-2.0, squat/generic-device-plugin; the cluster pulls it now): {} as {}", nodes.join(", "), wl::GPU_RESOURCE));
    let out = env.kubectl_apply("gpu-device-plugin", &wl::generic_plugin_manifest(&nodes))?;
    say_lines(env, &out, 4);
    let mut waited = Duration::ZERO;
    loop {
        let o = gpu_env(env)?.kubectl(&["get", "nodes", "-o", "json"]);
        let n = wl::node_resource_count(&o.stdout, wl::GPU_RESOURCE);
        if n > 0 {
            env.say(&format!("  the node reports {} = {n}", wl::GPU_RESOURCE));
            break;
        }
        if waited >= Duration::from_secs(600) {
            let logs = gpu_env(env)?.kubectl(&["-n", "kube-system", "logs", "daemonset/iyakki-gpu-device-plugin", "--tail=10"]);
            return Err(format!("the node does not report {} after 10 minutes. Plugin log: {}", wl::GPU_RESOURCE, logs.combined().lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ")));
        }
        env.sleep(poll);
        waited += poll;
    }
    env.say(&format!("[4/6] smoke test: an unprivileged pod asks for one GPU and lists the Vulkan devices it sees (the first run pulls {}, about 300 MB)", wl::LLAMA_VULKAN_IMAGE));
    let log = run_smoke_pod(env, wl::Gpu::Vulkan, Duration::from_secs(1500), poll)?;
    say_lines(env, &log, 10);
    let seen = wl::smoke_ok(wl::Gpu::Vulkan, &log)?;
    env.say(&format!("  the pod sees: {seen}"));
    env.say("[5/6] Lemonade");
    let data = gpu_env(env)?.data();
    if lemonade_deployed(env) && lemonade_compute(&data) == Compute::Cpu {
        env.say("  Lemonade is set to run on the CPU; it keeps that (change it in Settings -> Models or by chat)");
    } else if lemonade_deployed(env) {
        let out = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "patch", "deployment", "lemonade", "--type", "strategic", "-p", &wl::lemonade_patch(wl::Gpu::Vulkan, gid)]);
        if out.code != Some(0) {
            return Err(format!("could not give Lemonade the GPU: {}", out.combined().trim()));
        }
        env.say(&format!("  Lemonade got {} and the Vulkan backend; it restarts", wl::GPU_RESOURCE));
    } else {
        env.say("  Lemonade is not installed; install it from Store -> Setup and run this step again to give it the GPU");
    }
    patch_ollama(env, wl::Gpu::Vulkan, gid);
    let detail = format!("{} ({}); pods ask for {}: 1", usable[0].name, usable[0].render.clone().unwrap_or_default(), wl::GPU_RESOURCE);
    crate::quickinstall::mark_verified(env, Item::GpuVulkan, &detail)?;
    env.say(&format!("[6/6] done: {detail}"));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ stage 3

/// The order and the early refusals: what [`run_nvidia`] looks at first. Exposed for the page and for tests.
pub fn nvidia_facts(root: &Path, data: &Path) -> nv::Facts {
    let all = gpu::detect(root);
    nv::machine_facts(root, data, &all)
}

/// Load the modules, make the nodes. Returns the GPUs the driver found.
fn load_driver(env: &mut dyn StepEnv) -> Result<Vec<(u32, Option<String>)>, String> {
    let (root, data) = {
        let g = gpu_env(env)?;
        (g.root(), g.data())
    };
    let l = nv::Layout::new(&data);
    // the kernel looks for nvidia/<version>/gsp_*.bin under this path first
    let fw_param = root.join("sys/module/firmware_class/parameters/path");
    if fw_param.exists() {
        std::fs::write(&fw_param, format!("{}\n", l.firmware().display())).map_err(|e| format!("{}: {e}", fw_param.display()))?;
        env.say(&format!("  firmware search path: {}", l.firmware().display()));
    }
    for m in ["nvidia", "nvidia-uvm"] {
        let out = gpu_env(env)?.run(&nv::modprobe_argv(m), Duration::from_secs(120));
        if out.code != Some(0) {
            let dm = gpu_env(env)?.dmesg();
            let lines = gpu::kernel_lines(&dm, &[]);
            return Err(format!("modprobe {m} failed: {} {}", out.combined().trim(), lines.into_iter().rev().take(4).collect::<Vec<_>>().join(" | ")));
        }
        env.say(&format!("  loaded {m}"));
    }
    let saved = nv::allow_power_saving(&root);
    if !saved.is_empty() {
        env.say(&format!("  power saving on for {} (the card powers down when idle)", saved.join(", ")));
    }
    match nv::loaded_version(&root) {
        Some(v) if v == nv::NVIDIA_VERSION => env.say(&format!("  the loaded driver is {v}, as pinned")),
        Some(v) => return Err(format!("the loaded NVIDIA driver is {v}, not the pinned {}: the module in this image and the vendor files differ", nv::NVIDIA_VERSION)),
        None => return Err("/proc/driver/nvidia/version is not readable: the driver did not start".into()),
    }
    let found = nv::driver_gpus(&root);
    if found.is_empty() {
        let dm = gpu_env(env)?.dmesg();
        return Err(format!("the NVIDIA driver loaded but found no GPU. Kernel lines: {}", gpu::kernel_lines(&dm, &[]).into_iter().rev().take(5).collect::<Vec<_>>().join(" | ")));
    }
    let proc_devices = std::fs::read_to_string(root.join("proc/devices")).unwrap_or_default();
    let minors: Vec<u32> = found.iter().map(|(m, _)| *m).collect();
    for n in nv::nodes_to_make(&minors, &proc_devices) {
        let path = root.join(n.path.trim_start_matches('/'));
        if path.exists() {
            continue;
        }
        let out = gpu_env(env)?.run(&argv(&["mknod", "-m", "666", &path.display().to_string(), "c", &n.major.to_string(), &n.minor.to_string()]), Duration::from_secs(10));
        if out.code != Some(0) {
            return Err(format!("could not make {}: {}", path.display(), out.combined().trim()));
        }
        env.say(&format!("  made {} ({}:{})", n.path, n.major, n.minor));
    }
    Ok(found)
}

/// Write both CDI specs of the NVIDIA driver (the documented name and the device plugin's).
fn write_nvidia_specs(root: &Path, data: &Path, gpus: &[(u32, Option<String>)]) -> Result<Vec<PathBuf>, String> {
    let l = nv::Layout::new(data);
    let mut v = Vec::new();
    for kind in [gpucdi::NVIDIA_KIND, gpucdi::PLUGIN_KIND] {
        let spec = nv::cdi_spec(kind, root, &l, gpus)?;
        v.push(gpucdi::write_spec(&cdi_dir(root), &spec)?);
    }
    Ok(v)
}

/// "NVIDIA GPU driver (CUDA)".
pub fn run_nvidia(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    let (root, data) = {
        let g = gpu_env(env)?;
        (g.root(), g.data())
    };
    for l in nv::licence_notice().lines() {
        env.say(l);
    }
    let facts = nvidia_facts(&root, &data);
    let stage = nv::next_stage(&facts);
    env.say(&format!("[0/9] where this stands: {}", stage.words()));
    if stage.refuses() {
        return Err(stage.words().to_string());
    }
    let layout = nv::Layout::new(&data);
    env.say("[1/9] checking the cluster");
    crate::quickinstall::wait_for_cluster(env, wait, poll)?;
    // ---- the vendor files
    if layout.staged() {
        env.say(&format!("[2/9] the vendor files for {} are staged already ({})", nv::NVIDIA_VERSION, layout.stage().display()));
    } else {
        env.say(&format!("[2/9] downloading {} ({} MB) from download.nvidia.com; it is checked against the checksum pinned in IYAKKI and the one NVIDIA publishes", nv::run_file(), nv::RUN_BYTES >> 20));
        gpu_env(env)?.download_run(&layout)?;
        env.say("[3/9] download verified; unpacking it here (no installer script runs)");
        let _ = std::fs::remove_dir_all(layout.extracted());
        // the archive is zstd-compressed and NVIDIA's own unpacker carries a glibc `zstd` that cannot run on this system: unpack it natively,
        // and fall back to `sh <file> --extract-only` only when the file is not of that kind
        match nv::extract_makeself_zstd(&layout.run(), &layout.extracted()) {
            Ok(n) => env.say(&format!("  unpacked {n} files from the zstd archive")),
            Err(why) => {
                env.say(&format!("  native unpack not possible ({why}); trying `sh <file> --extract-only`"));
                let _ = std::fs::remove_dir_all(layout.extracted());
                let out = gpu_env(env)?.run(&argv(&["sh", &layout.run().display().to_string(), "--extract-only", "--target", &layout.extracted().display().to_string()]), Duration::from_secs(900));
                if out.code != Some(0) {
                    return Err(format!("unpacking failed (needs `sh`, `tail`, and a `zstd` or `xz` that runs here): {}", out.combined().trim().chars().take(300).collect::<String>()));
                }
            }
        }
        let s = nv::stage(&layout.extracted(), &layout)?;
        env.say(&format!("  staged {} libraries (+{} links), {} programs, {} firmware files, Vulkan ICD {} · {} MB in {}", s.libs, s.links, s.bins, s.firmware, if s.icd { "yes" } else { "no" }, s.bytes >> 20, layout.stage().display()));
        nv::clean_download(&layout);
        env.say("  the download and the unpacked tree were deleted (only the stage stays)");
    }
    // ---- the driver
    env.say("[4/9] loading the NVIDIA open modules and making the device nodes");
    let found = load_driver(env)?;
    for (m, u) in &found {
        env.say(&format!("  GPU minor {m}{}", u.as_ref().map(|u| format!(" · {u}")).unwrap_or_default()));
    }
    // ---- CDI
    env.say("[5/9] writing the CDI specs nvidia.com/gpu and k8s.device-plugin.nvidia.com/gpu");
    for p in write_nvidia_specs(&root, &data, &found)? {
        env.say(&format!("  {}", p.display()));
    }
    env.say("[6/9] making sure the cluster's containerd reads CDI specs");
    make_containerd_read_cdi(env, wait, poll)?;
    // ---- the plugin
    env.say(&format!("[7/9] deploying the NVIDIA device plugin (Apache-2.0, {}; the cluster pulls it now)", nv::PLUGIN_IMAGE));
    let manifest = nv::plugin_manifest(wl::KUBELET_DIR, &layout.stage().display().to_string());
    let out = env.kubectl_apply("nvidia-device-plugin", &manifest)?;
    say_lines(env, &out, 4);
    let mut waited = Duration::ZERO;
    loop {
        let out = gpu_env(env)?.kubectl(&["get", "nodes", "-o", "json"]);
        let n = wl::node_resource_count(&out.stdout, wl::NVIDIA_RESOURCE);
        if n > 0 {
            env.say(&format!("  the node reports nvidia.com/gpu = {n}"));
            break;
        }
        if waited >= Duration::from_secs(900) {
            let logs = gpu_env(env)?.kubectl(&["-n", "kube-system", "logs", "daemonset/nvidia-device-plugin", "--tail=15"]);
            return Err(format!("the node does not report nvidia.com/gpu after 15 minutes. Plugin log: {}", logs.combined().lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ")));
        }
        env.sleep(poll);
        waited += poll;
    }
    // ---- proof
    env.say(&format!("[8/9] smoke test: nvidia-smi in a pod ({}; the first run pulls it, about 200 MB)", nv::CUDA_BASE_IMAGE));
    let log = run_smoke_pod(env, wl::Gpu::Nvidia, Duration::from_secs(1200), poll)?;
    say_lines(env, &log, 14);
    let seen = wl::smoke_ok(wl::Gpu::Nvidia, &log)?;
    let data = gpu_env(env)?.data();
    if lemonade_deployed(env) && lemonade_compute(&data) == Compute::Cpu {
        env.say("  Lemonade is set to run on the CPU; it keeps that");
    } else if lemonade_deployed(env) {
        let out = gpu_env(env)?.kubectl(&["-n", wl::NAMESPACE, "patch", "deployment", "lemonade", "--type", "strategic", "-p", &wl::lemonade_patch(wl::Gpu::Nvidia, None)]);
        if out.code == Some(0) {
            env.say("  Lemonade got nvidia.com/gpu: 1 and the CUDA backend; it restarts");
        } else {
            env.say(&format!("  note: Lemonade could not be given the GPU: {}", out.combined().trim()));
        }
    }
    patch_ollama(env, wl::Gpu::Nvidia, None);
    let detail = format!("driver {} · {} · pods request nvidia.com/gpu", nv::NVIDIA_VERSION, seen);
    crate::quickinstall::mark_verified(env, Item::GpuNvidia, &detail)?;
    env.say(&format!("[9/9] done: {detail}"));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the NVIDIA card off and on

/// What the kernel says about each NVIDIA card's power, for the log: `0000:01:00.0 suspended`.
fn nvidia_power_state(root: &Path) -> String {
    gpu::detect(root).iter().filter(|g| g.is_nvidia()).map(|g| format!("{} {}", g.pci, std::fs::read_to_string(root.join("sys/bus/pci/devices").join(&g.pci).join("power/runtime_status")).map(|t| t.trim().to_string()).unwrap_or_else(|_| "unknown".into()))).collect::<Vec<_>>().join(", ")
}

/// Switch the NVIDIA card off (`sleep`) or on again. A laptop card idles at 20 W or more while the driver is loaded and anything (the device
/// plugin's health checks, a pod) keeps it open, and the driver's own power saving cannot then work. **Off**: stop the NVIDIA device plugin,
/// unload the driver (the file [`nv::sleep_marker`] keeps it off across a restart), and say what the kernel reports. **On**: remove the file, load
/// the driver and make the nodes and CDI specs, deploy the plugin. Pods that use the card must be stopped first: the driver does not unload while
/// one holds it, and then nothing is changed except the plugin, which is put back.
pub fn run_nvidia_power(env: &mut dyn StepEnv, sleep: bool, _wait: Duration, poll: Duration) -> Result<(), String> {
    let (root, data) = {
        let g = gpu_env(env)?;
        (g.root(), g.data())
    };
    let layout = nv::Layout::new(&data);
    if !layout.staged() {
        return Err("the NVIDIA driver is not installed: Setup > Hardware > NVIDIA GPU driver".into());
    }
    let marker = nv::sleep_marker(&data);
    let plugin = || nv::plugin_manifest(wl::KUBELET_DIR, &layout.stage().display().to_string());
    if sleep {
        env.say("[1/3] stopping the NVIDIA device plugin (its health checks keep the card awake)");
        let out = gpu_env(env)?.kubectl(&["-n", "kube-system", "delete", "daemonset", "nvidia-device-plugin", "--ignore-not-found", "--wait=true", "--timeout=60s"]);
        say_lines(env, &out.combined(), 3);
        env.say("[2/3] unloading the NVIDIA driver");
        let mut failed = None;
        for m in ["nvidia-uvm", "nvidia-drm", "nvidia-modeset", "nvidia"] {
            let o = gpu_env(env)?.run(&argv(&["modprobe", "-r", m]), Duration::from_secs(60));
            if o.code != Some(0) && failed.is_none() && gpu::module_loaded(&root, &m.replace('-', "_")) {
                failed = Some(format!("{m}: {}", o.combined().trim()));
            }
        }
        if let Some(why) = failed {
            // put the plugin back so nothing is left half done
            let _ = env.kubectl_apply("nvidia-device-plugin", &plugin());
            return Err(format!("the driver would not unload ({why}). Something still uses the card: stop pods that ask for nvidia.com/gpu (Lemonade with the CUDA backend is one) and try again"));
        }
        std::fs::create_dir_all(marker.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
        std::fs::write(&marker, "off\n").map_err(|e| format!("{}: {e}", marker.display()))?;
        env.sleep(poll);
        env.say(&format!("[3/3] done: the NVIDIA driver is unloaded and stays off after a restart. The kernel reports: {}", nvidia_power_state(&root)));
        return Ok(());
    }
    env.say("[1/3] loading the NVIDIA driver");
    let _ = std::fs::remove_file(&marker);
    let found = load_driver(env)?;
    env.say("[2/3] writing the CDI specs");
    write_nvidia_specs(&root, &data, &found)?;
    env.say("[3/3] deploying the NVIDIA device plugin");
    let out = env.kubectl_apply("nvidia-device-plugin", &plugin())?;
    say_lines(env, &out, 3);
    env.say("done: the node reports nvidia.com/gpu again in a minute or so");
    Ok(())
}

// ------------------------------------------------------------------------------------------------ after a start

/// After every start of IYAKKI the RAM holds nothing of what the GPU steps did: put the CDI specs back and, when the NVIDIA step ran,
/// load the driver again. Cheap when everything is there. Returns a line per thing done.
pub fn ensure_runtime(root: &Path, data: &Path, run: &mut dyn FnMut(&[String]) -> Out) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let marked = |it: Item| crate::quickinstall::marked(data, it).is_some();
    if marked(Item::GpuVulkan) {
        // the render nodes are made again at every start (devtmpfs); the device plugin in the cluster finds them by its own
        let (changed, _) = open_render_nodes(root, &dev_dri(root));
        if !changed.is_empty() {
            out.push(Ok(format!("GPU: render nodes made openable again ({})", changed.join(", "))));
        }
    }
    if marked(Item::GpuNvidia) && !nv::sleep_marker(data).exists() {
        let l = nv::Layout::new(data);
        // the kernel loads the nvidia module by itself a few seconds into the boot, long before this runs, and asks for the GSP firmware when the card
        // is first opened (by the device plugin or a pod): the search path must name the stage's firmware folder at every start, whether or not the
        // CDI specs and the modules need doing; without it "Direct firmware load for nvidia/<version>/gsp_ga10x.bin failed" and no pod can use the card
        if l.staged() {
            let fw_param = root.join("sys/module/firmware_class/parameters/path");
            let want = format!("{}\n", l.firmware().display());
            if fw_param.exists() && std::fs::read_to_string(&fw_param).map(|t| t != want).unwrap_or(true) && std::fs::write(&fw_param, &want).is_ok() {
                out.push(Ok(format!("GPU: firmware search path set to {}", l.firmware().display())));
            }
        }
        let have = cdi_dir(root).join(format!("{}.json", gpucdi::NVIDIA_KIND.replace('/', "-"))).is_file();
        if !have && l.staged() && !gpu::module_loaded(root, "nouveau") {
            let r = (|| -> Result<String, String> {
                let fw_param = root.join("sys/module/firmware_class/parameters/path");
                if fw_param.exists() {
                    let _ = std::fs::write(&fw_param, format!("{}\n", l.firmware().display()));
                }
                for m in ["nvidia", "nvidia-uvm"] {
                    let o = run(&nv::modprobe_argv(m));
                    if o.code != Some(0) {
                        return Err(format!("modprobe {m} failed: {}", o.combined().trim()));
                    }
                }
                let found = nv::driver_gpus(root);
                let proc_devices = std::fs::read_to_string(root.join("proc/devices")).unwrap_or_default();
                for n in nv::nodes_to_make(&found.iter().map(|(m, _)| *m).collect::<Vec<_>>(), &proc_devices) {
                    let path = root.join(n.path.trim_start_matches('/'));
                    if !path.exists() {
                        run(&argv(&["mknod", "-m", "666", &path.display().to_string(), "c", &n.major.to_string(), &n.minor.to_string()]));
                    }
                }
                write_nvidia_specs(root, data, &found).map(|p| format!("GPU: NVIDIA driver loaded again, wrote {} CDI spec(s)", p.len()))
            })();
            out.push(r);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ the test prompt

/// Where a model server for the test might answer: Lemonade's API and the GPU Ollama's loopback port.
pub fn test_endpoints(lemonade_bases: &[String]) -> Vec<String> {
    let mut v: Vec<String> = lemonade_bases.to_vec();
    v.push("http://127.0.0.1:11435/v1".into());
    v
}

/// `iyakki quick-install gpu-test`: send the test prompt to the first server that answers; print the result and keep it.
pub fn run_test(data: &Path, bases: &[String], up: &dyn iyakki_proxy::Upstream, now: &dyn Fn() -> f64, say: &mut dyn FnMut(&str)) -> Result<gpu::TestResult, String> {
    say(&format!("test prompt: \"{}\"", gpu::TEST_PROMPT));
    let mut last = String::from("no local model server answers (install Lemonade, or the GPU Ollama, and pull a chat model)");
    for b in bases {
        say(&format!("trying {b}"));
        match gpu::test_prompt(up, b, None, now) {
            Ok(r) => {
                say(&format!("reply: {}", r.reply.replace('\n', " ")));
                say(&r.line());
                gpu::save_last_test(data, &r.line())?;
                return Ok(r);
            }
            Err(e) => {
                say(&format!("  {e}"));
                last = e;
            }
        }
    }
    Err(last)
}

// ------------------------------------------------------------------------------------------------ the GPU page

/// What the page reads at once (files only): the GPUs, the driver state without the kernel log, the markers of the two steps and the last
/// test result. The pods and the kernel log come from [`refresh_async`].
pub fn quick_report(root: &Path, data: &Path) -> gpu::Report {
    let gpus = gpu::detect(root);
    gpu::Report {
        driver: gpu::driver_state(root, &gpus, ""),
        gpus,
        pods: Vec::new(),
        pods_note: "looking...".into(),
        last_test: gpu::last_test(data),
        stage1: crate::quickinstall::marked(data, Item::GpuVulkan),
        stage3: crate::quickinstall::marked(data, Item::GpuNvidia),
    }
}

/// The slow part of the report: the kernel log and the pods that use the GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Background {
    pub kernel_log: String,
    pub pods: Vec<gpu::GpuPod>,
    /// Why the pods could not be listed (no cluster, not Ready ...), empty when they could.
    pub pods_note: String,
}

static BACKGROUND: std::sync::Mutex<Option<Background>> = std::sync::Mutex::new(None);

/// Read the kernel log and ask the managed cluster for its pods, on a thread; [`take_background`] returns the answer once.
pub fn refresh_async(svc: std::sync::Arc<crate::services::Services>) {
    *BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let _ = std::thread::Builder::new().name("iyakki-gpu-refresh".into()).spawn(move || {
        let dm = crate::wsexec::Runner::run(&crate::wsexec::System, Path::new("/"), &["dmesg".to_string()], &[], Duration::from_secs(5), 4 << 20, None);
        let mut b = Background { kernel_log: dm.stdout, ..Default::default() };
        if svc.k3sdev.installed().is_some() {
            let o = svc.k3sdev.kubectl(&["get", "pods", "-A", "-o", "json"]);
            if o.code == Some(0) {
                b.pods = gpu::pods_using_gpu(&o.stdout);
            } else {
                b.pods_note = format!("cluster not answering: {}", o.combined().lines().last().unwrap_or("").trim());
            }
        } else {
            b.pods_note = "no managed cluster".into();
        }
        *BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(b);
    });
}

/// The answer of [`refresh_async`], once it is there.
pub fn take_background() -> Option<Background> {
    BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Merge the slow part into the report.
pub fn merge_background(r: &mut gpu::Report, b: &Background, root: &Path) {
    r.pods = b.pods.clone();
    r.pods_note = b.pods_note.clone();
    r.driver = gpu::driver_state(root, &r.gpus, &b.kernel_log);
}

#[cfg(test)]
#[path = "gpu_steps_tests.rs"]
mod tests;
