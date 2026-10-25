//! The AI hardware profile (docs/AI-HARDWARE.md): an honest answer to "can this machine run language models, and how big?", and the
//! one table that decides which AI tools of Store -> Setup are shown, recommended or hidden because of it.
//!
//! * **What counts is the memory a model can really use**, not the machine's headline numbers: dedicated video memory (minus a
//!   reserve for the screen and other programs) for a discrete card, the shared pool the driver exposes for an integrated GPU,
//!   unified memory (minus what macOS keeps) for Apple Silicon, system RAM minus a reserve for CPU-only inference.
//! * **Read-only files, nothing started.** Every function takes the root of the file system it reads (`/` on the machine, a folder
//!   of fake files in the tests): DMI (`/sys/class/dmi/id`), the device tree of Asahi Linux (`/proc/device-tree/model`),
//!   `/proc/meminfo`, the PCI display devices (`gpu::detect`) with amdgpu's `mem_info_vram_total` and `mem_info_gtt_total`, Intel's
//!   `lmem_total_bytes`, and the PCI windows as a last guess. A missing or garbled file is "unknown", never a crash.
//! * **Unknown is not small.** A GPU whose memory size cannot be learned is shown and never hides anything.
//! * **Nothing is ever impossible**: [`visibility`] turns "Hidden" into "Not recommended, with the reason" when the administrator's
//!   "Show all AI tools" setting is on.

use crate::gpu::{self, Vendor};
use crate::quickinstall::Item;
use std::path::Path;

/// A GPU (or Apple chip) is "LLM capable" with at least this much usable memory (GB): 7B at 4 bits needs about 5 GB plus context.
pub const MIN_LLM_GB: f64 = 6.0;
/// Memory kept free on a discrete card for the framebuffer, the compositor and other programs (GB).
pub const DISCRETE_RESERVE_GB: f64 = 1.5;
/// Memory kept for the system when the model runs on the CPU or in shared memory (GB); the same 4 GB as `modelhost::fits`.
pub const SYSTEM_RESERVE_GB: f64 = 4.0;
/// Cache for the conversation, added to a model's size when asking if it fits (GB).
pub const CONTEXT_GB: f64 = 1.0;
/// Apple Silicon: macOS lets the GPU use about two thirds of the memory up to 36 GB and three quarters above (the default of
/// Metal's recommended working set; the owner can raise it with a system setting, which IYAKKI does not assume).
pub const APPLE_SMALL_SHARE: f64 = 2.0 / 3.0;
pub const APPLE_LARGE_SHARE: f64 = 0.75;
pub const APPLE_SMALL_LIMIT_GB: f64 = 36.0;
/// Models: (name, size in GB at 4 bits). The tiers follow them.
pub const MODELS: [(&str, f64); 5] = [("3B", 2.5), ("7B", 5.0), ("13B", 9.0), ("32B", 20.0), ("70B", 42.0)];

/// How big a model the machine runs comfortably.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    None,
    /// 7B models.
    Small,
    /// up to 13B.
    Medium,
    /// up to 32B.
    Large,
    /// 70B.
    Xl,
}

impl Tier {
    /// The tier of `usable` GB: the biggest model (plus [`CONTEXT_GB`]) that fits. Boundaries: 6, 10, 21 and 43 GB.
    pub fn of(usable: f64) -> Tier {
        let fits = |gb: f64| usable + 1e-9 >= gb + CONTEXT_GB;
        if fits(MODELS[4].1) {
            Tier::Xl
        } else if fits(MODELS[3].1) {
            Tier::Large
        } else if fits(MODELS[2].1) {
            Tier::Medium
        } else if fits(MODELS[1].1) {
            Tier::Small
        } else {
            Tier::None
        }
    }
    /// Plain words.
    pub fn words(self) -> &'static str {
        match self {
            Tier::None => "too little memory for models of useful size",
            Tier::Small => "runs 7B models comfortably",
            Tier::Medium => "runs 7B and 13B models comfortably",
            Tier::Large => "runs models up to 32B comfortably",
            Tier::Xl => "runs models up to 70B comfortably",
        }
    }
}

/// An Apple chip generation. `Unknown` is a newer Apple Silicon model id this table does not know yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gen {
    M1,
    M2,
    M3,
    M4,
    M5,
    M6,
    Unknown,
}

impl Gen {
    pub fn name(self) -> &'static str {
        match self {
            Gen::M1 => "M1",
            Gen::M2 => "M2",
            Gen::M3 => "M3",
            Gen::M4 => "M4",
            Gen::M5 => "M5",
            Gen::M6 => "M6",
            Gen::Unknown => "M-series",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipTier {
    Base,
    Pro,
    Max,
    Ultra,
}

impl ChipTier {
    pub fn name(self) -> &'static str {
        match self {
            ChipTier::Base => "",
            ChipTier::Pro => " Pro",
            ChipTier::Max => " Max",
            ChipTier::Ultra => " Ultra",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Class {
    AppleSilicon { gen: Gen, tier: Option<ChipTier>, unified_gb: f64 },
    /// `vram_gb` None: the size could not be learned.
    DiscreteGpu { vendor: Vendor, name: String, vram_gb: Option<f64> },
    IntegratedGpu { vendor: Vendor, name: String, carve_out_gb: f64, gtt_gb: f64 },
    CpuOnly { ram_gb: f64 },
}

/// One graphics device as the profile saw it.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuInfo {
    pub vendor: Vendor,
    pub name: String,
    pub integrated: bool,
    pub vram_gb: Option<f64>,
    pub carve_out_gb: f64,
    pub gtt_gb: f64,
    /// Memory a model can use on it (None: unknown size).
    pub usable_gb: Option<f64>,
    /// Where the size came from: "the driver", "the card's model", "the PCI window".
    pub source: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AiProfile {
    pub class: Class,
    /// Memory a model can use, GB (0 when unknown).
    pub llm_usable_gb: f64,
    pub tier: Tier,
    /// The system RAM, GB.
    pub ram_gb: f64,
    /// Every graphics device found (virtual ones left out), best first.
    pub gpus: Vec<GpuInfo>,
    /// A Mac on the network is set up as a model host (the bridge, docs/MAC-MODEL-HOST.md).
    pub mac_hosts: usize,
    /// The GPUs of the remote cluster the person works against (aiprofile_remote.rs), when there is one.
    pub remote: Option<crate::aiprofile_remote::RemoteProfile>,
    pub notes: Vec<String>,
}

// ------------------------------------------------------------------------------------------------ reading

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().trim_matches('\0').trim().to_string())
}

const GIB: f64 = (1u64 << 30) as f64;

/// The RAM in GB from `proc/meminfo` (0 when unreadable).
pub fn ram_gb(root: &Path) -> f64 {
    let t = read(&root.join("proc/meminfo")).unwrap_or_default();
    let kb = t.lines().find_map(|l| l.strip_prefix("MemTotal:")?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
    kb as f64 / (1024.0 * 1024.0)
}

/// Apple Silicon generation from a model identifier ("Mac14,2"). `None`: not an Apple Silicon model (an Intel Mac, a PC).
/// Known: M1 (MacBookAir10,1, MacBookPro17,1, MacBookPro18,x, Macmini9,1, iMac21,x, Mac13,x), M2 (Mac14,x), M3 (Mac15,x),
/// M4 (Mac16,x), M5 (Mac17,x), M6 (Mac18,x: expected by the numbering, unverified); any other `MacNN,x` with NN above 18 is
/// "a newer Apple Silicon" (`Gen::Unknown`).
pub fn apple_gen_of_model(model: &str) -> Option<(Gen, Option<ChipTier>)> {
    let m = model.trim();
    let (fam, ver) = m.split_at(m.find(|c: char| c.is_ascii_digit())?);
    let (major, minor) = ver.split_once(',')?;
    let (major, minor): (u32, u32) = (major.parse().ok()?, minor.parse().ok()?);
    let g = match fam {
        "MacBookAir" if major == 10 => (Gen::M1, Some(ChipTier::Base)),
        "MacBookAir" if major > 10 => (Gen::Unknown, None),
        "MacBookPro" if major == 17 => (Gen::M1, Some(ChipTier::Base)),
        "MacBookPro" if major == 18 => (Gen::M1, Some(if minor == 2 || minor == 4 { ChipTier::Max } else { ChipTier::Pro })),
        "MacBookPro" if major > 18 => (Gen::Unknown, None),
        "Macmini" if major == 9 => (Gen::M1, Some(ChipTier::Base)),
        "Macmini" if major > 9 => (Gen::Unknown, None),
        "iMac" if major == 21 => (Gen::M1, Some(ChipTier::Base)),
        "iMac" if major > 21 => (Gen::Unknown, None),
        "Mac" => match major {
            13 => (Gen::M1, Some(if minor == 2 { ChipTier::Ultra } else { ChipTier::Max })),
            14 => match minor {
                2 | 3 | 7 | 15 => (Gen::M2, Some(ChipTier::Base)),
                5 | 6 | 13 => (Gen::M2, Some(ChipTier::Max)),
                8 | 14 => (Gen::M2, Some(ChipTier::Ultra)),
                9 | 10 | 12 => (Gen::M2, Some(ChipTier::Pro)),
                _ => (Gen::M2, None),
            },
            15 => match minor {
                3 | 12 | 13 => (Gen::M3, Some(ChipTier::Base)),
                6 | 7 | 8 => (Gen::M3, Some(ChipTier::Pro)),
                9..=11 => (Gen::M3, Some(ChipTier::Max)),
                _ => (Gen::M3, None),
            },
            16 => (Gen::M4, None),
            17 => (Gen::M5, None),
            18 => (Gen::M6, None),
            n if n > 18 => (Gen::Unknown, None),
            _ => return None,
        },
        _ => return None,
    };
    Some(g)
}

/// Generation and tier from a device-tree model string ("Apple MacBook Pro (14-inch, M2 Pro, 2023)"), as Asahi Linux gives it.
pub fn apple_gen_of_text(text: &str) -> Option<(Gen, Option<ChipTier>)> {
    let t = text.trim();
    if !t.starts_with("Apple") {
        return None;
    }
    let bytes: Vec<char> = t.chars().collect();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == 'M' && bytes[i + 1].is_ascii_digit() && (i == 0 || !bytes[i - 1].is_alphanumeric()) {
            let gen = match bytes[i + 1] {
                '1' => Gen::M1,
                '2' => Gen::M2,
                '3' => Gen::M3,
                '4' => Gen::M4,
                '5' => Gen::M5,
                '6' => Gen::M6,
                _ => Gen::Unknown,
            };
            let rest: String = bytes[i + 2..].iter().collect();
            let word = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
            let tier = if word.starts_with("Pro") {
                ChipTier::Pro
            } else if word.starts_with("Max") {
                ChipTier::Max
            } else if word.starts_with("Ultra") {
                ChipTier::Ultra
            } else {
                ChipTier::Base
            };
            return Some((gen, Some(tier)));
        }
    }
    None
}

/// Video memory (GB) of NVIDIA cards by PCI device id: the proprietary and the nouveau driver tell no total through sysfs, so the card's
/// model says it. Only models with one size are listed; a card that is not here is "unknown size".
pub const NVIDIA_VRAM_GB: &[(u16, f64)] = &[
    (0x2684, 24.0),
    (0x2685, 24.0),
    (0x2702, 16.0),
    (0x2704, 16.0),
    (0x2705, 16.0),
    (0x2782, 12.0),
    (0x2783, 12.0),
    (0x2786, 12.0),
    (0x2803, 8.0),
    (0x2882, 8.0),
    (0x2204, 24.0),
    (0x2206, 10.0),
    (0x2208, 12.0),
    (0x2216, 10.0),
    (0x2484, 8.0),
    (0x2488, 8.0),
    (0x2503, 12.0),
    (0x2504, 12.0),
    (0x2487, 8.0),
    (0x1e04, 11.0),
    (0x1e07, 11.0),
    (0x1e82, 8.0),
    (0x1e87, 8.0),
    (0x1f02, 8.0),
    (0x1f07, 8.0),
    (0x1f08, 6.0),
    (0x1f06, 8.0),
    (0x2184, 6.0),
    (0x21c4, 6.0),
    (0x2182, 6.0),
    (0x1f82, 4.0),
    (0x1b80, 8.0),
    (0x1b06, 11.0),
    (0x1c03, 6.0),
    (0x1c81, 2.0),
    (0x1c82, 4.0),
    (0x26b1, 48.0),
    (0x2230, 48.0),
    (0x2236, 24.0),
];

/// The largest memory window of a PCI device in GB, from its `resource` file (lines of `start end flags` in hex). Resizable BAR makes
/// this the video memory; without it the window is 256 MB and tells nothing, so only 4 GB or more is believed.
fn pci_window_gb(dir: &Path) -> Option<f64> {
    let t = read(&dir.join("resource"))?;
    let best = t
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (s, e) = (u64::from_str_radix(f.next()?.trim_start_matches("0x"), 16).ok()?, u64::from_str_radix(f.next()?.trim_start_matches("0x"), 16).ok()?);
            (e >= s && s != 0).then(|| e - s + 1)
        })
        .max()?;
    let gb = best as f64 / GIB;
    (gb >= 4.0).then_some(gb)
}

fn num(p: &Path) -> Option<u64> {
    read(p)?.parse().ok()
}

/// AMD device ids of APUs (the graphics inside the processor), whose "video memory" is a slice of RAM.
const AMD_APU_IDS: &[u16] = &[0x15bf, 0x1681, 0x164e, 0x1900, 0x15d8, 0x15dd, 0x1636, 0x1638, 0x163f, 0x15e7, 0x13c0, 0x1506, 0x150e, 0x1114];

fn gpu_info(root: &Path, g: &gpu::Gpu, ram_gb: f64) -> GpuInfo {
    let dir = root.join("sys/bus/pci/devices").join(&g.pci);
    let mut name = g.name.clone();
    if !name.to_ascii_lowercase().contains(&g.vendor.name().to_ascii_lowercase()) {
        name = format!("{} {}", g.vendor.name(), name);
    }
    let mut info = GpuInfo { vendor: g.vendor, name, integrated: false, vram_gb: None, carve_out_gb: 0.0, gtt_gb: 0.0, usable_gb: None, source: "unknown" };
    match g.vendor {
        Vendor::Amd => {
            let vram = g.vram.or_else(|| num(&dir.join("mem_info_vram_total")));
            let gtt = num(&dir.join("mem_info_gtt_total"));
            let vram_gb = vram.map(|v| v as f64 / GIB);
            let gtt_gb = gtt.map(|v| v as f64 / GIB).unwrap_or(0.0);
            // an APU: by its id; or a driver that reports at most 1 GB of dedicated memory next to a bigger shared pool
            info.integrated = AMD_APU_IDS.contains(&g.device_id) || (vram_gb.is_some_and(|v| v <= 1.0) && gtt_gb > vram_gb.unwrap_or(0.0));
            if info.integrated {
                // the carve-out is a slice of the same RAM the shared pool maps: count the larger, once, and leave the system its share
                info.carve_out_gb = vram_gb.unwrap_or(0.0);
                info.gtt_gb = gtt_gb;
                let pool = info.carve_out_gb.max(gtt_gb);
                let cap = (ram_gb - SYSTEM_RESERVE_GB).max(0.0);
                info.usable_gb = (vram.is_some() || gtt.is_some()).then(|| pool.min(if ram_gb > 0.0 { cap } else { pool }));
                info.vram_gb = vram_gb;
                info.source = "the driver";
            } else if let Some(v) = vram_gb {
                info.vram_gb = Some(v);
                info.usable_gb = Some((v - DISCRETE_RESERVE_GB).max(0.0));
                info.source = "the driver";
            }
        }
        Vendor::Nvidia => {
            if let Some((_, v)) = NVIDIA_VRAM_GB.iter().find(|(d, _)| *d == g.device_id) {
                info.vram_gb = Some(*v);
                info.source = "the card's model";
            } else if let Some(v) = pci_window_gb(&dir) {
                info.vram_gb = Some(v);
                info.source = "the PCI window";
            }
            info.usable_gb = info.vram_gb.map(|v| (v - DISCRETE_RESERVE_GB).max(0.0));
        }
        Vendor::Intel => {
            let lmem = std::fs::read_dir(dir.join("drm")).into_iter().flatten().flatten().find_map(|e| num(&e.path().join("lmem_total_bytes"))).or_else(|| num(&dir.join("lmem_total_bytes")));
            if let Some(b) = lmem.filter(|b| *b > 0) {
                let v = b as f64 / GIB;
                info.vram_gb = Some(v);
                info.usable_gb = Some((v - DISCRETE_RESERVE_GB).max(0.0));
                info.source = "the driver";
            } else if g.name.contains("Arc") && g.name.chars().any(|c| c.is_ascii_digit()) && !g.name.contains("Meteor") && !g.name.contains("Graphics (") {
                // an Arc card whose driver tells no total: the PCI window, else unknown
                if let Some(v) = pci_window_gb(&dir) {
                    info.vram_gb = Some(v);
                    info.usable_gb = Some((v - DISCRETE_RESERVE_GB).max(0.0));
                    info.source = "the PCI window";
                }
            } else {
                // UHD, Iris Xe, Arc Graphics of a processor: shares RAM, and the kernel gives no pool to count: not used for models
                info.integrated = true;
                info.usable_gb = Some(0.0);
                info.source = "the graphics type";
            }
        }
        Vendor::Other(_) => {}
    }
    info
}

/// Read this machine under `root` (`/`).
pub fn detect(root: &Path) -> AiProfile {
    let ram = ram_gb(root);
    let model = read(&root.join("sys/class/dmi/id/product_name")).unwrap_or_default();
    let tree = read(&root.join("proc/device-tree/model")).unwrap_or_default();
    let mut notes = Vec::new();
    // Apple Silicon: the device tree of Asahi Linux names the chip; the DMI model id is the fallback
    let apple = apple_gen_of_text(&tree).or_else(|| apple_gen_of_model(&model));
    if let Some((gen, tier)) = apple {
        if gen == Gen::Unknown {
            notes.push("A newer Apple Silicon model than this table knows: the memory is read from the machine, the chip is not named.".into());
        }
        let share = if ram <= APPLE_SMALL_LIMIT_GB + 0.5 { APPLE_SMALL_SHARE } else { APPLE_LARGE_SHARE };
        let usable = ram * share;
        notes.push(format!("macOS lets the GPU use about {} of the unified memory by default.", if share < 0.7 { "two thirds" } else { "three quarters" }));
        return AiProfile { class: Class::AppleSilicon { gen, tier, unified_gb: ram }, llm_usable_gb: usable, tier: Tier::of(usable), ram_gb: ram, gpus: vec![], mac_hosts: 0, remote: None, notes };
    }
    let mut gpus: Vec<GpuInfo> = gpu::detect(root).iter().filter(|g| !g.is_virtual()).map(|g| gpu_info(root, g, ram)).collect();
    // best first: the most usable memory, a discrete card before a shared pool
    gpus.sort_by(|a, b| b.usable_gb.unwrap_or(-1.0).partial_cmp(&a.usable_gb.unwrap_or(-1.0)).unwrap_or(std::cmp::Ordering::Equal).then(a.integrated.cmp(&b.integrated)));
    if gpus.len() > 1 {
        notes.push(format!("{} graphics devices: the best one is counted, models are not spread over several.", gpus.len()));
    }
    // a GPU of unknown size is the "best" only when no GPU has a known size
    let best = gpus.first().cloned();
    if let Some(b) = &best {
        if b.usable_gb.is_none() {
            notes.push(format!("The memory size of {} is unknown (the driver does not say and the card is not in IYAKKI's table): nothing is hidden because of it.", b.name));
        } else if b.source == "the card's model" || b.source == "the PCI window" {
            notes.push(format!("The memory of {} comes from {}, not measured.", b.name, b.source));
        }
    }
    let (class, usable) = match best {
        Some(b) if b.integrated && b.usable_gb.unwrap_or(0.0) > 0.0 => {
            let u = b.usable_gb.unwrap_or(0.0);
            (Class::IntegratedGpu { vendor: b.vendor, name: b.name.clone(), carve_out_gb: b.carve_out_gb, gtt_gb: b.gtt_gb }, u)
        }
        Some(b) if !b.integrated => {
            let u = b.usable_gb.unwrap_or(0.0);
            (Class::DiscreteGpu { vendor: b.vendor, name: b.name.clone(), vram_gb: b.vram_gb }, u)
        }
        _ => (Class::CpuOnly { ram_gb: ram }, (ram - SYSTEM_RESERVE_GB).max(0.0)),
    };
    let tier = if matches!(class, Class::CpuOnly { .. }) { Tier::None } else { Tier::of(usable) };
    AiProfile { class, llm_usable_gb: usable, tier, ram_gb: ram, gpus, mac_hosts: 0, remote: None, notes }
}

impl AiProfile {
    /// Add the remote cluster's GPUs (aiprofile_remote.rs).
    pub fn with_remote(mut self, r: Option<crate::aiprofile_remote::RemoteProfile>) -> AiProfile {
        self.remote = r;
        self
    }
    /// The cluster has a GPU big enough for models (or one whose memory is unknown): a model server can be deployed there.
    pub fn remote_ok(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.capable() || r.unknown())
    }
    fn remote_words(&self) -> String {
        self.remote.as_ref().map(|r| format!("your cluster '{}' has {}{}", r.cluster, r.words(), if r.stale { " (last known: the cluster did not answer)" } else { "" })).unwrap_or_default()
    }
}

/// The same with the number of Mac model hosts the person set up (Hosts::list().len()).
pub fn detect_with_hosts(root: &Path, mac_hosts: usize) -> AiProfile {
    let mut p = detect(root);
    p.mac_hosts = mac_hosts;
    p
}

// ------------------------------------------------------------------------------------------------ words

fn gb(v: f64) -> String {
    if v >= 100.0 || (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as u64)
    } else {
        format!("{v:.1}")
    }
}

impl AiProfile {
    /// An LLM-capable machine: a GPU or Apple chip with at least [`MIN_LLM_GB`] usable. A GPU of unknown size is not "capable" but is not
    /// "incapable" either ([`AiProfile::unknown`]).
    pub fn capable(&self) -> bool {
        !matches!(self.class, Class::CpuOnly { .. }) && self.llm_usable_gb >= MIN_LLM_GB
    }

    /// A graphics device is there but its memory size is not known.
    pub fn unknown(&self) -> bool {
        matches!(self.class, Class::DiscreteGpu { vram_gb: None, .. })
    }

    /// The model sizes that fit ("7B", "13B"), 4-bit with [`CONTEXT_GB`] of cache.
    pub fn fits(&self) -> Vec<&'static str> {
        MODELS.iter().filter(|(_, s)| self.llm_usable_gb + 1e-9 >= s + CONTEXT_GB).map(|(n, _)| *n).collect()
    }

    /// "fits: 7B Q4, 13B Q4" or "fits: nothing of useful size".
    pub fn fits_line(&self) -> String {
        let f = self.fits();
        if f.is_empty() {
            "fits: no model of useful size".into()
        } else {
            format!("fits: {}", f.iter().map(|n| format!("{n} Q4")).collect::<Vec<_>>().join(", "))
        }
    }

    /// The machine in plain words.
    pub fn line(&self) -> String {
        match &self.class {
            Class::AppleSilicon { gen, tier, unified_gb } => format!("Apple {}{}, {} GB unified memory: about {} GB usable for models", gen.name(), tier.map_or("", |t| t.name()), gb(*unified_gb), gb(self.llm_usable_gb.floor())),
            Class::DiscreteGpu { name, vram_gb: Some(v), .. } => format!("{name}, {} GB: about {} GB usable for models", gb(*v), gb((self.llm_usable_gb * 2.0).floor() / 2.0)),
            Class::DiscreteGpu { name, vram_gb: None, .. } => format!("{name}: memory size unknown, so nothing is hidden because of it"),
            Class::IntegratedGpu { name, .. } => format!("{name} (shared): up to {} GB usable for models", gb((self.llm_usable_gb * 2.0).floor() / 2.0)),
            Class::CpuOnly { ram_gb } => {
                let intel = self.gpus.iter().find(|g| g.integrated);
                match intel {
                    Some(g) => format!("{} (not used for models): CPU only, {} GB RAM", g.name, gb((ram_gb * 10.0).round() / 10.0)),
                    None => format!("No GPU: CPU only, {} GB RAM", gb((ram_gb * 10.0).round() / 10.0)),
                }
            }
        }
    }

    /// [`line`](AiProfile::line) and what it means: "..., runs 7B models comfortably".
    pub fn summary(&self) -> String {
        let tail = match &self.class {
            Class::CpuOnly { .. } => "local models would run on the CPU and be slow".to_string(),
            Class::DiscreteGpu { vram_gb: None, .. } => "ask the card's maker or add its id to IYAKKI's table".to_string(),
            _ => self.tier.words().to_string(),
        };
        let mac = if self.mac_hosts > 0 { format!(". {} Mac model host{} set up", self.mac_hosts, if self.mac_hosts == 1 { "" } else { "s" }) } else { String::new() };
        let cl = if self.remote.is_some() { format!(". Cluster: {}", self.remote_words().trim_start_matches("your cluster ")) } else { String::new() };
        format!("{}; {tail}{mac}{cl}", self.line())
    }

    fn best_gpu_name(&self) -> String {
        self.gpus.first().map(|g| g.name.clone()).unwrap_or_default()
    }
}

// ------------------------------------------------------------------------------------------------ the show / hide table

/// What the profile says about one tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vis {
    Show,
    /// Shown, and the reason is that the cluster has the GPU this machine lacks ("your cluster 'x' has 2 x ...").
    ShownBecause(String),
    /// Shown with a star and the reason.
    Recommended(String),
    /// Shown, with the reason it is not a good idea here.
    NotRecommended(String),
    /// Not shown (unless "Show all AI tools" is on), with the reason.
    Hidden(String),
}

impl Vis {
    pub fn shown(&self) -> bool {
        !matches!(self, Vis::Hidden(_))
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Vis::Show => None,
            Vis::ShownBecause(r) | Vis::Recommended(r) | Vis::NotRecommended(r) | Vis::Hidden(r) => Some(r),
        }
    }
}

/// The single table: which tool is shown, recommended or hidden for this profile.
///
/// | Tool | Capable GPU / Apple chip | GPU under 6 GB | CPU only | Apple Silicon |
/// |---|---|---|---|---|
/// | Lemonade, Ollama | shown | hidden | hidden | shown |
/// | GPU for local models (Vulkan) | recommended | hidden | hidden | hidden (Metal, not Vulkan) |
/// | NVIDIA driver | shown when an NVIDIA GPU is the capable one | hidden | hidden | hidden |
/// | Mac's GPU (M-series host) | not recommended (it has its own) | recommended | recommended | hidden (it is the Mac) |
/// | Elasticsearch | not recommended under 6 GB RAM | | | |
/// | everything else (Kokoro and Indic-TTS run on the CPU) | shown | | | |
///
/// A GPU of unknown size hides nothing. `None` (no profile read yet) shows everything.
pub fn rule(p: &AiProfile, item: Item) -> Vis {
    let small = || {
        if matches!(p.class, Class::CpuOnly { .. }) {
            format!("this machine has no GPU with {} GB or more; local models would run on the CPU and be very slow", gb(MIN_LLM_GB))
        } else {
            format!("the best GPU here ({}) has under {} GB usable for models; local models would run on the CPU and be very slow", p.best_gpu_name(), gb(MIN_LLM_GB))
        }
    };
    let apple = matches!(p.class, Class::AppleSilicon { .. });
    match item {
        Item::Lemonade | Item::Ollama => {
            if p.capable() || p.unknown() {
                Vis::Show
            } else if p.remote_ok() {
                Vis::ShownBecause(p.remote_words())
            } else if apple {
                Vis::NotRecommended(format!("this Apple chip has about {} GB usable for models: only small ones (3B) fit", gb(p.llm_usable_gb.floor())))
            } else {
                Vis::Hidden(small())
            }
        }
        Item::GpuVulkan => {
            if apple {
                Vis::Hidden("this is an Apple Silicon Mac: it uses Metal, not a Vulkan runtime".into())
            } else if p.unknown() {
                Vis::Show
            } else if p.capable() {
                Vis::Recommended(format!("a GPU with {} GB usable for models was found: local models run much faster on it", gb((p.llm_usable_gb * 2.0).floor() / 2.0)))
            } else {
                Vis::Hidden(small())
            }
        }
        Item::GpuNvidia => {
            let nvidia_best = p.gpus.first().is_some_and(|g| g.vendor == Vendor::Nvidia);
            if apple {
                Vis::Hidden("this is an Apple Silicon Mac: there is no NVIDIA card".into())
            } else if !nvidia_best {
                Vis::Hidden("the best GPU here is not an NVIDIA one".into())
            } else if p.capable() || p.unknown() {
                Vis::Show
            } else {
                Vis::Hidden(small())
            }
        }
        Item::MacHost => {
            if apple {
                Vis::Hidden("this machine is the Mac: models run here directly".into())
            } else if p.capable() {
                Vis::NotRecommended(format!("this machine has its own GPU ({} GB usable for models); a Mac on your network is optional", gb((p.llm_usable_gb * 2.0).floor() / 2.0)))
            } else if p.unknown() {
                Vis::Show
            } else if p.remote_ok() {
                Vis::NotRecommended(p.remote_words())
            } else {
                Vis::Recommended("this machine has no GPU with 6 GB or more for models; the GPU of a Mac on your network can run them".into())
            }
        }
        Item::Elasticsearch if p.ram_gb > 0.0 && p.ram_gb * 1024.0 < item.min_ram_mb() as f64 => Vis::NotRecommended(format!("it needs about {} GB of memory and this machine has {:.1} GB", item.min_ram_mb().div_ceil(1024), p.ram_gb)),
        _ => Vis::Show,
    }
}

/// [`rule`] with the administrator's "Show all AI tools": a hidden tool is shown as "not recommended" with the same reason.
pub fn visibility(p: &AiProfile, item: Item, show_all: bool) -> Vis {
    match rule(p, item) {
        Vis::Hidden(r) if show_all => Vis::NotRecommended(r),
        v => v,
    }
}

/// The text for a hidden tool: "Hidden because ...".
pub fn hidden_text(reason: &str) -> String {
    format!("Hidden because {reason}.")
}

/// The AI tools hidden on this profile, each with its reason (what the "Show all AI tools" switch would bring back).
pub fn hidden_items(p: &AiProfile) -> Vec<(Item, String)> {
    Item::ALL.into_iter().filter_map(|i| if let Vis::Hidden(r) = rule(p, i) { Some((i, r)) } else { None }).collect()
}

static LOCAL: std::sync::OnceLock<AiProfile> = std::sync::OnceLock::new();

/// This machine's profile as the screens use it: the local hardware is read once per run (it does not change while IYAKKI runs),
/// the cluster's GPUs and the Mac hosts each time (cached values, never a wait).
pub fn this_machine(mac_hosts: usize) -> AiProfile {
    let mut p = LOCAL.get_or_init(|| detect(Path::new("/"))).clone();
    p.mac_hosts = mac_hosts;
    p.remote = crate::aiprofile_remote::current();
    p
}

#[cfg(test)]
#[path = "aiprofile_tests.rs"]
mod tests;
