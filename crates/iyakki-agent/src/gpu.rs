//! GPUs for local models (F-GPU-01, F-GPU-03; docs/GPU-K3S.md): what graphics hardware this machine has and what state its driver is in.
//!
//! * **Detection is read-only sysfs** (`/sys/bus/pci/devices`, with the DRM nodes each device owns, `/dev/dri`) and table-driven:
//!   vendor ids to names and the device ids of the cards people own to marketing names, in this file, no `lspci`, no pci.ids. Every
//!   function takes the root of the file system it reads (`/` on the machine, a folder of fake files in the tests).
//! * **The verdict for stage 1 (Vulkan)**: Mesa brings the Vulkan driver inside the pod image, so the host needs only a kernel
//!   driver bound to the card and a render node: AMD (amdgpu, RADV), Intel (i915 or xe, ANV), NVIDIA Turing and newer through nouveau
//!   (NVK, needs the GSP firmware). A card that has none of those gets the reason in plain words.
//! * **What uses it**: pods that ask for the GPU through the extended-resource name or the CDI annotation ([`pods_using_gpu`]).
//! * **Test prompt**: one chat request to the local model server, with the answer's tokens per second ([`test_prompt`]).
//!
//! Nothing here starts a process. The kernel log is passed in as text; the cluster is asked through the caller's `kubectl`.

use std::path::{Path, PathBuf};

/// PCI vendor ids.
pub const NVIDIA: u16 = 0x10de;
pub const AMD: u16 = 0x1002;
pub const INTEL: u16 = 0x8086;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    /// Virtual or other (virtio, VMware, Bochs, ASPEED ...).
    Other(u16),
}

impl Vendor {
    pub fn from_id(id: u16) -> Vendor {
        match id {
            NVIDIA => Vendor::Nvidia,
            AMD | 0x1022 => Vendor::Amd,
            INTEL => Vendor::Intel,
            o => Vendor::Other(o),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other(0x1af4) => "virtio",
            Vendor::Other(0x15ad) => "VMware",
            Vendor::Other(0x1234) => "QEMU",
            Vendor::Other(0x1a03) => "ASPEED",
            Vendor::Other(0x1b36) => "Red Hat QXL",
            Vendor::Other(_) => "other",
        }
    }
}

/// Device ids of cards people own, per vendor: `(device id, name)`. Not a full pci.ids: a card that is not here is shown by its
/// family (from the id range) and its raw ids, which is enough to ask for the id to be added.
pub const NVIDIA_NAMES: &[(u16, &str)] = &[
    (0x2684, "GeForce RTX 4090"),
    (0x2685, "GeForce RTX 4090 D"),
    (0x2702, "GeForce RTX 4080 SUPER"),
    (0x2704, "GeForce RTX 4080"),
    (0x2705, "GeForce RTX 4070 Ti SUPER"),
    (0x2782, "GeForce RTX 4070 Ti"),
    (0x2783, "GeForce RTX 4070 SUPER"),
    (0x2786, "GeForce RTX 4070"),
    (0x2803, "GeForce RTX 4060 Ti"),
    (0x2882, "GeForce RTX 4060"),
    (0x2204, "GeForce RTX 3090"),
    (0x2206, "GeForce RTX 3080"),
    (0x2208, "GeForce RTX 3080 Ti"),
    (0x2216, "GeForce RTX 3080 LHR"),
    (0x2484, "GeForce RTX 3070"),
    (0x2488, "GeForce RTX 3070 LHR"),
    (0x2503, "GeForce RTX 3060"),
    (0x2504, "GeForce RTX 3060 LHR"),
    (0x2487, "GeForce RTX 3060 (8 GB)"),
    (0x1e04, "GeForce RTX 2080 Ti"),
    (0x1e07, "GeForce RTX 2080 Ti Rev. A"),
    (0x1e82, "GeForce RTX 2080"),
    (0x1e87, "GeForce RTX 2080 Rev. A"),
    (0x1f02, "GeForce RTX 2070"),
    (0x1f07, "GeForce RTX 2070 Rev. A"),
    (0x1f08, "GeForce RTX 2060"),
    (0x1f06, "GeForce RTX 2060 SUPER"),
    (0x2184, "GeForce GTX 1660"),
    (0x21c4, "GeForce GTX 1660 SUPER"),
    (0x2182, "GeForce GTX 1660 Ti"),
    (0x1f82, "GeForce GTX 1650"),
    (0x1b80, "GeForce GTX 1080"),
    (0x1b06, "GeForce GTX 1080 Ti"),
    (0x1c03, "GeForce GTX 1060 6GB"),
    (0x1c81, "GeForce GTX 1050"),
    (0x1c82, "GeForce GTX 1050 Ti"),
    (0x26b1, "RTX 6000 Ada Generation"),
    (0x2230, "RTX A6000"),
    (0x2236, "A10"),
];
pub const AMD_NAMES: &[(u16, &str)] = &[
    (0x744c, "Radeon RX 7900 XT/XTX"),
    (0x7480, "Radeon RX 7600"),
    (0x747e, "Radeon RX 7800 XT"),
    (0x73bf, "Radeon RX 6800/6800 XT/6900 XT"),
    (0x73df, "Radeon RX 6700 XT"),
    (0x73ff, "Radeon RX 6600"),
    (0x731f, "Radeon RX 5700/5700 XT"),
    (0x687f, "Radeon RX Vega 56/64"),
    (0x67df, "Radeon RX 480/580"),
    (0x15bf, "Radeon 780M (Phoenix)"),
    (0x1681, "Radeon 680M (Rembrandt)"),
    (0x164e, "Radeon Graphics (Raphael)"),
    (0x1900, "Radeon 890M"),
    (0x15d8, "Radeon Vega (Raven Ridge)"),
];
pub const INTEL_NAMES: &[(u16, &str)] = &[
    (0x56a0, "Arc A770"),
    (0x56a1, "Arc A750"),
    (0x5690, "Arc A770M"),
    (0xe20b, "Arc B580"),
    (0x4680, "UHD Graphics 770"),
    (0x4626, "UHD Graphics (Alder Lake-P)"),
    (0x9a49, "Iris Xe Graphics (Tiger Lake)"),
    (0xa7a0, "Iris Xe Graphics (Raptor Lake-P)"),
    (0x7d55, "Arc Graphics (Meteor Lake)"),
    (0x3e92, "UHD Graphics 630"),
    (0x5917, "UHD Graphics 620"),
];

/// The NVIDIA architecture of a device id, from the id ranges the vendor uses.
pub fn nvidia_family(device: u16) -> &'static str {
    match device {
        0x2900..=0x2fff => "Blackwell",
        0x2680..=0x28ff => "Ada Lovelace",
        0x2300..=0x233f => "Hopper",
        0x2200..=0x25ff => "Ampere",
        0x1e00..=0x1fff | 0x2180..=0x21ff => "Turing",
        0x1d81..=0x1dbf => "Volta",
        0x1b00..=0x1d7f => "Pascal",
        0x1340..=0x17ff => "Maxwell",
        0x0f00..=0x13bf => "Kepler",
        _ => "unknown architecture",
    }
}

/// Turing and newer: what NVIDIA's open kernel modules and nouveau's GSP path (NVK) support.
pub fn nvidia_open_supported(device: u16) -> bool {
    matches!(nvidia_family(device), "Turing" | "Ampere" | "Ada Lovelace" | "Hopper" | "Blackwell")
}

/// A marketing name from the tables, or a description built from the family.
pub fn device_name(vendor: u16, device: u16) -> String {
    let table = match vendor {
        NVIDIA => NVIDIA_NAMES,
        AMD => AMD_NAMES,
        INTEL => INTEL_NAMES,
        _ => &[],
    };
    if let Some((_, n)) = table.iter().find(|(d, _)| *d == device) {
        return (*n).to_string();
    }
    match vendor {
        NVIDIA => format!("NVIDIA {} GPU ({vendor:04x}:{device:04x})", nvidia_family(device)),
        _ => format!("{} graphics ({vendor:04x}:{device:04x})", Vendor::from_id(vendor).name()),
    }
}

/// One graphics device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    /// PCI address, `0000:01:00.0`.
    pub pci: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub vendor: Vendor,
    pub name: String,
    /// The kernel driver bound to it (`amdgpu`, `nouveau`, `nvidia`, `i915`, `xe` ...), if any.
    pub driver: Option<String>,
    /// `card0` and `renderD128` when the DRM subsystem made nodes for it.
    pub card: Option<String>,
    pub render: Option<String>,
    pub boot_vga: bool,
    /// Video memory in bytes when the driver tells (amdgpu's `mem_info_vram_total`).
    pub vram: Option<u64>,
}

impl Gpu {
    pub fn is_nvidia(&self) -> bool {
        self.vendor == Vendor::Nvidia
    }
    pub fn family(&self) -> &'static str {
        if self.is_nvidia() { nvidia_family(self.device_id) } else { "" }
    }
    /// `/dev/dri/renderD128` under `root`.
    pub fn render_path(&self, root: &Path) -> Option<PathBuf> {
        self.render.as_ref().map(|r| root.join("dev/dri").join(r))
    }
    /// A virtual adapter (a VM's display, a BMC): no compute to offer.
    pub fn is_virtual(&self) -> bool {
        matches!(self.vendor, Vendor::Other(_))
    }
    /// One line: name, address, driver, node.
    pub fn line(&self) -> String {
        let drv = self.driver.as_deref().unwrap_or("no driver");
        let node = self.render.as_deref().unwrap_or("no render node");
        let vram = self.vram.map(|v| format!(" · {} GB", (v + (1 << 29)) >> 30)).unwrap_or_default();
        format!("{} · {} · {drv} · {node}{vram}", self.name, self.pci)
    }
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// All display-class PCI devices under `root` (`/sys/bus/pci/devices`), sorted by address, with their DRM nodes.
pub fn detect(root: &Path) -> Vec<Gpu> {
    let dir = root.join("sys/bus/pci/devices");
    let mut out = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for n in names {
        let d = dir.join(&n);
        let Some(class) = read(&d.join("class")).and_then(|c| hex(&c)) else { continue };
        // 0x0300 VGA controller, 0x0302 3D controller, 0x0380 other display controller
        if class >> 8 != 0x0300 && class >> 8 != 0x0302 && class >> 8 != 0x0380 {
            continue;
        }
        let (Some(v), Some(dev)) = (read(&d.join("vendor")).and_then(|c| hex(&c)), read(&d.join("device")).and_then(|c| hex(&c))) else { continue };
        let (v, dev) = (v as u16, dev as u16);
        let driver = std::fs::read_link(d.join("driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
        let (mut card, mut render) = (None, None);
        for e in std::fs::read_dir(d.join("drm")).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if f.starts_with("renderD") {
                render = Some(f);
            } else if f.starts_with("card") && !f.contains('-') {
                card = Some(f);
            }
        }
        out.push(Gpu {
            pci: n,
            vendor_id: v,
            device_id: dev,
            vendor: Vendor::from_id(v),
            name: device_name(v, dev),
            driver,
            card,
            render,
            boot_vga: read(&d.join("boot_vga")).as_deref() == Some("1"),
            vram: read(&d.join("mem_info_vram_total")).and_then(|s| s.parse().ok()),
        });
    }
    out
}

/// An NVIDIA GPU is on the machine (the NVIDIA driver entry of Quick install is shown only then).
pub fn has_nvidia(gpus: &[Gpu]) -> bool {
    gpus.iter().any(|g| g.is_nvidia())
}

/// Stage 1: can Vulkan work on this card through Mesa in a pod? `(ok, plain words)`.
pub fn vulkan_verdict(g: &Gpu) -> (bool, String) {
    let drv = g.driver.as_deref().unwrap_or("");
    if g.is_virtual() {
        return (false, format!("{} is a virtual display adapter: no compute", g.name));
    }
    if g.render.is_none() {
        return (false, format!("{} has no render node (/dev/dri/renderD*): the kernel driver ({}) did not set it up", g.name, if drv.is_empty() { "none bound" } else { drv }));
    }
    match (g.vendor, drv) {
        (Vendor::Amd, "amdgpu") => (true, "AMD amdgpu: Mesa RADV in the pod".into()),
        (Vendor::Amd, d) => (false, format!("AMD card driven by `{}`: RADV needs amdgpu (GCN 1.2 and newer)", if d.is_empty() { "no driver" } else { d })),
        (Vendor::Intel, "i915" | "xe") => (true, format!("Intel {drv}: Mesa ANV in the pod")),
        (Vendor::Intel, d) => (false, format!("Intel card driven by `{}`: ANV needs i915 or xe", if d.is_empty() { "no driver" } else { d })),
        (Vendor::Nvidia, "nouveau") if nvidia_open_supported(g.device_id) => (true, format!("NVIDIA {} through nouveau: Mesa NVK in the pod (needs the GSP firmware; slower than the vendor driver)", g.family())),
        (Vendor::Nvidia, "nouveau") => (false, format!("NVIDIA {}: nouveau's Vulkan (NVK) needs Turing or newer", g.family())),
        (Vendor::Nvidia, "nvidia") => (true, "NVIDIA vendor driver loaded: CUDA (stage 3) and the vendor Vulkan ICD".into()),
        (Vendor::Nvidia, _) => (false, "NVIDIA card without a driver: nouveau did not bind (check the GSP firmware lines below)".into()),
        _ => (false, "no supported driver".into()),
    }
}

/// The GPUs stage 1 can use.
pub fn vulkan_usable(gpus: &[Gpu]) -> Vec<&Gpu> {
    gpus.iter().filter(|g| vulkan_verdict(g).0).collect()
}

/// Is this module name loaded (`/proc/modules`)?
pub fn module_loaded(root: &Path, name: &str) -> bool {
    let n = name.replace('-', "_");
    read(&root.join("proc/modules")).is_some_and(|t| t.lines().any(|l| l.split_whitespace().next().is_some_and(|m| m == n)))
}

/// The kernel log lines about firmware and about one PCI device (or the GPU drivers in general): what to show under the GPU list.
pub fn kernel_lines(dmesg: &str, pci: &[&str]) -> Vec<String> {
    let words = ["nouveau", "nvidia", "amdgpu", "i915", "[drm", "drm:", "NVRM", "xe "];
    dmesg
        .lines()
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            let about_fw = low.contains("firmware") || low.contains("gsp");
            let ours = pci.iter().any(|p| l.contains(p)) || words.iter().any(|w| l.contains(w));
            ours && (about_fw || low.contains("error") || low.contains("failed") || low.contains("nvrm"))
        })
        .map(|l| l.trim().to_string())
        .collect()
}

/// Firmware the kernel could not load, from its log: file names.
pub fn missing_firmware(dmesg: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for l in dmesg.lines() {
        // "Direct firmware load for nvidia/ad102/gsp/gsp-535.113.01.bin failed with error -2"
        if let Some(i) = l.find("Direct firmware load for ") {
            let rest = &l[i + 25..];
            if let Some(f) = rest.split(" failed").next() {
                let f = f.trim().to_string();
                if !f.is_empty() && !v.contains(&f) {
                    v.push(f);
                }
            }
        }
    }
    v
}

/// What the page shows about the driver of the whole machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriverState {
    pub nouveau_loaded: bool,
    pub amdgpu_loaded: bool,
    pub nvidia_loaded: bool,
    pub nvidia_uvm_loaded: bool,
    /// `nvidia.ko` is in /lib/modules (the image was built with the open modules).
    pub nvidia_module_present: bool,
    pub missing_firmware: Vec<String>,
    pub kernel_lines: Vec<String>,
}

pub fn driver_state(root: &Path, gpus: &[Gpu], dmesg: &str) -> DriverState {
    let pci: Vec<&str> = gpus.iter().map(|g| g.pci.as_str()).collect();
    DriverState {
        nouveau_loaded: module_loaded(root, "nouveau"),
        amdgpu_loaded: module_loaded(root, "amdgpu"),
        nvidia_loaded: module_loaded(root, "nvidia"),
        nvidia_uvm_loaded: module_loaded(root, "nvidia_uvm"),
        nvidia_module_present: nvidia_module_path(root).is_some(),
        missing_firmware: missing_firmware(dmesg),
        kernel_lines: kernel_lines(dmesg, &pci).into_iter().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect(),
    }
}

/// `/lib/modules/<kernel>/updates/nvidia.ko` (or `kernel/drivers/video/nvidia.ko`) under `root`, when the image carries the open modules.
pub fn nvidia_module_path(root: &Path) -> Option<PathBuf> {
    for k in std::fs::read_dir(root.join("lib/modules")).into_iter().flatten().flatten() {
        for sub in ["updates", "extra", "kernel/drivers/video"] {
            let p = k.path().join(sub).join("nvidia.ko");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// One pod that uses the GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuPod {
    pub namespace: String,
    pub name: String,
    /// `nvidia.com/gpu x1` or `iyakki.dev/gpu x1`.
    pub how: String,
}

/// Pods that ask for a GPU, from `kubectl get pods -A -o json`: the extended resource `iyakki.dev/gpu` (stage 1), `nvidia.com/gpu` (stage 3)
/// or `amd.com/gpu` in a container's limits, or a `cdi.k8s.io/*` annotation naming a GPU device.
pub fn pods_using_gpu(json: &str) -> Vec<GpuPod> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    for p in v["items"].as_array().into_iter().flatten() {
        let phase = p["status"]["phase"].as_str().unwrap_or("");
        if phase == "Succeeded" || phase == "Failed" {
            continue;
        }
        let (ns, name) = (p["metadata"]["namespace"].as_str().unwrap_or(""), p["metadata"]["name"].as_str().unwrap_or(""));
        let mut how: Vec<String> = Vec::new();
        for c in p["spec"]["containers"].as_array().into_iter().flatten() {
            for key in ["nvidia.com/gpu", "iyakki.dev/gpu", "amd.com/gpu"] {
                if let Some(n) = c["resources"]["limits"][key].as_str().map(str::to_string).or_else(|| c["resources"]["limits"][key].as_u64().map(|n| n.to_string())) {
                    how.push(format!("{key} x{n}"));
                }
            }
        }
        if let Some(a) = p["metadata"]["annotations"].as_object() {
            for (k, val) in a {
                if k.starts_with("cdi.k8s.io/") {
                    if let Some(s) = val.as_str().filter(|s| s.contains("gpu")) {
                        how.push(format!("CDI {s}"));
                    }
                }
            }
        }
        if !how.is_empty() {
            how.dedup();
            out.push(GpuPod { namespace: ns.into(), name: name.into(), how: how.join(", ") });
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ the test prompt

/// The prompt the test sends: short, so a slow CPU finishes, long enough to measure.
pub const TEST_PROMPT: &str = "Write two sentences about why the sky is blue.";

/// What a test prompt measured.
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub model: String,
    pub tokens: u64,
    pub seconds: f64,
    pub tokens_per_second: f64,
    pub reply: String,
}

impl TestResult {
    pub fn line(&self) -> String {
        format!("{:.1} tokens/s ({} tokens in {:.1} s, model {})", self.tokens_per_second, self.tokens, self.seconds, self.model)
    }
}

/// The request body of the test: one non-streaming chat turn, a few dozen tokens.
pub fn test_request(model: &str) -> String {
    serde_json::json!({ "model": model, "messages": [{ "role": "user", "content": TEST_PROMPT }], "max_tokens": 96, "temperature": 0, "stream": false }).to_string()
}

/// Read the answer of an OpenAI-style chat completion: tokens from `usage.completion_tokens` (else a count of the words, times 1.3),
/// `seconds` measured by the caller around the request.
pub fn parse_test_answer(body: &str, model: &str, seconds: f64) -> Result<TestResult, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("the answer is not JSON: {e}"))?;
    if let Some(e) = v["error"]["message"].as_str().or_else(|| v["error"].as_str()) {
        return Err(format!("the server said: {e}"));
    }
    let reply = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if reply.is_empty() {
        return Err("the server answered with no text".into());
    }
    let tokens = v["usage"]["completion_tokens"].as_u64().filter(|t| *t > 0).unwrap_or_else(|| ((reply.split_whitespace().count() as f64) * 1.3).round() as u64);
    // a server that measures its own generation time (llama.cpp style `timings`) is more exact than the wall clock
    let secs = v["timings"]["predicted_ms"].as_f64().map(|ms| ms / 1000.0).filter(|s| *s > 0.0).unwrap_or(seconds);
    if secs <= 0.0 {
        return Err("no time measured".into());
    }
    Ok(TestResult { model: model.into(), tokens, seconds: secs, tokens_per_second: tokens as f64 / secs, reply })
}

/// Run the test against a server at `base` (`http://127.0.0.1:13305/api/v1`): choose the first downloaded chat model unless one is
/// named, send the prompt, time it. `up` is the HTTP client (a fake in tests); `now` the clock in seconds.
pub fn test_prompt(up: &dyn iyakki_proxy::Upstream, base: &str, model: Option<&str>, now: &dyn Fn() -> f64) -> Result<TestResult, String> {
    let model = match model {
        Some(m) => m.to_string(),
        None => {
            let r = up.request("GET", &format!("{base}/models"), &[], &[])?;
            if r.status / 100 != 2 {
                return Err(format!("the model list answered HTTP {}", r.status));
            }
            let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|e| e.to_string())?;
            let list = v["data"].as_array().cloned().unwrap_or_default();
            let pick = list
                .iter()
                .find(|m| m["downloaded"].as_bool().unwrap_or(true) && !m["labels"].as_array().is_some_and(|l| l.iter().any(|x| matches!(x.as_str(), Some("embeddings" | "reranking" | "tts" | "audio" | "image" | "speech")))))
                .and_then(|m| m["id"].as_str())
                .ok_or("the server has no downloaded chat model: pull one in Settings \u{2192} Models first")?;
            pick.to_string()
        }
    };
    let t0 = now();
    let r = up.request("POST", &format!("{base}/chat/completions"), &[("Content-Type".into(), "application/json".into())], test_request(&model).as_bytes())?;
    let secs = now() - t0;
    let body = String::from_utf8_lossy(&r.body).into_owned();
    if r.status / 100 != 2 {
        return Err(format!("the server answered HTTP {}: {}", r.status, body.chars().take(160).collect::<String>()));
    }
    parse_test_answer(&body, &model, secs)
}

/// Where the last test result is kept (`<data>/gpu/last-test.txt`, one line).
pub fn last_test_file(data: &Path) -> PathBuf {
    data.join("gpu/last-test.txt")
}

pub fn save_last_test(data: &Path, line: &str) -> Result<(), String> {
    let p = last_test_file(data);
    std::fs::create_dir_all(p.parent().unwrap_or(data)).map_err(|e| e.to_string())?;
    std::fs::write(&p, format!("{}\n{line}\n", crate::clock::now_unix())).map_err(|e| e.to_string())
}

pub fn last_test(data: &Path) -> Option<String> {
    std::fs::read_to_string(last_test_file(data)).ok().and_then(|t| t.lines().nth(1).map(str::to_string))
}

// ------------------------------------------------------------------------------------------------ the report

/// Everything the GPU page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub gpus: Vec<Gpu>,
    pub driver: DriverState,
    pub pods: Vec<GpuPod>,
    /// Why the pods could not be listed (no cluster, not answering), empty when they could.
    pub pods_note: String,
    pub last_test: Option<String>,
    /// Stage 1 / stage 3 as installed (the markers of Quick install).
    pub stage1: Option<String>,
    pub stage3: Option<String>,
}

/// The page's text, line by line: `(level, text)` with level 0 normal, 1 good, 2 warning, 3 bad.
pub fn report_lines(r: &Report) -> Vec<(u8, String)> {
    let mut v: Vec<(u8, String)> = Vec::new();
    if r.gpus.is_empty() {
        v.push((3, "No graphics device found (nothing under /sys/bus/pci/devices has a display class).".into()));
    }
    for g in &r.gpus {
        let (ok, why) = vulkan_verdict(g);
        v.push((if ok { 1 } else if g.is_virtual() { 0 } else { 2 }, g.line()));
        v.push((0, format!("    {why}")));
    }
    if r.driver.nvidia_module_present {
        v.push((if r.driver.nvidia_loaded { 1 } else { 0 }, format!("NVIDIA open modules: present in this image · nvidia {} · nvidia_uvm {}", if r.driver.nvidia_loaded { "loaded" } else { "not loaded" }, if r.driver.nvidia_uvm_loaded { "loaded" } else { "not loaded" })));
    } else if has_nvidia(&r.gpus) {
        v.push((2, "NVIDIA open modules: not in this image (the PC image built with IYAKKI_NVIDIA_OPEN=1 carries them); nouveau drives the card".into()));
    }
    for f in &r.driver.missing_firmware {
        v.push((3, format!("Firmware the kernel could not load: {f}")));
    }
    for l in &r.driver.kernel_lines {
        v.push((0, format!("  kernel: {l}")));
    }
    v.push((if r.stage1.is_some() { 1 } else { 0 }, format!("Vulkan for local models: {}", r.stage1.clone().unwrap_or_else(|| "not installed (Store \u{2192} Setup \u{2192} Hardware)".into()))));
    if has_nvidia(&r.gpus) {
        v.push((if r.stage3.is_some() { 1 } else { 0 }, format!("NVIDIA driver (CUDA): {}", r.stage3.clone().unwrap_or_else(|| "not installed (Store \u{2192} Setup \u{2192} Hardware)".into()))));
    }
    if r.pods.is_empty() {
        v.push((0, if r.pods_note.is_empty() { "Pods using the GPU: none".to_string() } else { format!("Pods using the GPU: none ({})", r.pods_note) }));
    } else {
        for p in &r.pods {
            v.push((0, format!("Pod {}/{} uses the GPU ({})", p.namespace, p.name, p.how)));
        }
    }
    if let Some(t) = &r.last_test {
        v.push((1, format!("Last test prompt: {t}")));
    }
    v
}

#[cfg(test)]
#[path = "gpu_tests.rs"]
mod tests;
