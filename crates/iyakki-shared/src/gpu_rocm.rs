//! Types moved out of `iyakki_agent::gpu_rocm` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



use std::path::Path;

use crate::gpu::{Gpu, Vendor};

pub const RESOURCE: &str = "amd.com/gpu";

/// PCI device ids of AMD generations ROCm no longer supports (Polaris, Hawaii, Tonga, Fiji and older GCN), by the kernel's amdgpu id table.
pub fn rocm_dropped_generation(device: u16) -> Option<&'static str> {
    Some(match device {
        0x67C0..=0x67FF | 0x6980..=0x699F | 0x694C..=0x694F => "Polaris",
        0x67A0..=0x67BF => "Hawaii",
        0x6920..=0x693F => "Tonga",
        0x7300 => "Fiji",
        0x6640..=0x665F | 0x6600..=0x663F => "Bonaire and Oland",
        0x6780..=0x679F => "Tahiti and Pitcairn",
        _ => return None,
    })
}

/// Stage 2: is ROCm worth trying on this card? `(ok, plain words)`. Needs the amdgpu driver, a render node and the compute interface
/// `/dev/kfd` under `root` (the kernel makes it when `CONFIG_HSA_AMD` is on and the card supports it).
pub fn rocm_verdict(g: &Gpu, root: &Path) -> (bool, String) {
    if g.vendor != Vendor::Amd {
        return (false, format!("{} is not an AMD card", g.name));
    }
    if g.is_virtual() {
        return (false, format!("{} is a virtual display adapter: no compute", g.name));
    }
    let drv = g.driver.as_deref().unwrap_or("");
    if drv != "amdgpu" {
        return (false, format!("AMD card driven by `{}`: ROCm needs amdgpu", if drv.is_empty() { "no driver" } else { drv }));
    }
    if g.render.is_none() {
        return (false, format!("{} has no render node: the kernel driver did not set it up", g.name));
    }
    if let Some(gen) = rocm_dropped_generation(g.device_id) {
        return (false, format!("{} is a {gen} card: ROCm no longer supports that generation, so it stays on Vulkan (stage 1)", g.name));
    }
    if !root.join("dev/kfd").exists() {
        return (false, format!("{} has no /dev/kfd: the kernel's compute interface did not start for it (the card or its firmware is not supported); it stays on Vulkan", g.name));
    }
    (true, format!("{}: amdgpu with the compute interface (/dev/kfd); ROCm can be tried and the smoke pod will say", g.name))
}

/// Cards stage 2 can try.
pub fn rocm_usable<'a>(gpus: &'a [Gpu], root: &Path) -> Vec<&'a Gpu> {
    gpus.iter().filter(|g| rocm_verdict(g, root).0).collect()
}

/// A card stage 2 may be tried on is on this machine (the Quick install entry is shown only then).
pub fn any_usable(gpus: &[Gpu]) -> bool {
    !rocm_usable(gpus, Path::new("/")).is_empty()
}

/// The images stage 2 needs, each pinned by digest (`name@sha256:<64 hex>`). **They are the owner's to choose** (ROCm images are large, change
/// often and need a real card to be tested on): empty means "not pinned yet", and the Quick install entry `gpu-rocm` then stops with a plain
/// message before it changes anything (fails closed, KI-782). Fill them in with the owner's chosen digests and nothing else needs to change.
///
/// Provenance (owner directive 2026-11-19, retrieved 2026-11-19 over HTTPS, no layers pulled): each digest is the `Docker-Content-Digest`
/// of the tag's manifest from the Docker Hub registry v2 API (anonymous token, `registry-1.docker.io/v2/<repo>/manifests/<tag>`), and the
/// sha256 of the manifest body was recomputed and matched. Tags chosen: the newest numbered (non-rc, non-labeller) release of the
/// device plugin; the newest `rocm-terminal` tag; the newest non-rc `-rocm` Ollama tag. A test on a real AMD card is still Manual 'needed'.
/// Device plugin: docker.io/rocm/k8s-device-plugin:1.31.0.12 (single-arch amd64 manifest).
pub const PLUGIN_IMAGE: &str = "docker.io/rocm/k8s-device-plugin@sha256:6437c74f6566e5d11c20c42369fca7194315943aea91f7bd6c98de1b19caf117";
/// Test image: docker.io/rocm/rocm-terminal:6.4 (single-arch amd64 manifest).
pub const SMOKE_IMAGE: &str = "docker.io/rocm/rocm-terminal@sha256:72b323c5d56c511ea75f7353d0fee04e637390dd4874d414020284627a658014";
/// The ROCm build of Ollama (optional: without it only Lemonade is given the card).
/// docker.io/ollama/ollama:0.40.16-rocm (OCI index, amd64 only).
pub const OLLAMA_IMAGE: &str = "docker.io/ollama/ollama@sha256:85b3c895222b1ddc9d63c40b20fc74f37a37593dcbbdfbf28910a925e43ead00";

/// The images a run uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins {
    pub plugin: String,
    pub smoke: String,
    pub ollama: String,
}

impl Pins {
    /// The pins this build carries.
    pub fn shipped() -> Pins {
        Pins { plugin: PLUGIN_IMAGE.into(), smoke: SMOKE_IMAGE.into(), ollama: OLLAMA_IMAGE.into() }
    }

    /// True for `name@sha256:<64 lower-case hex>`.
    pub fn is_pinned(image: &str) -> bool {
        image.rsplit_once("@sha256:").is_some_and(|(n, d)| !n.is_empty() && d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) && !image.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'')
    }

    /// The plain refusal when the plugin or the smoke image is not pinned (the Ollama image is optional).
    pub fn check(&self) -> Result<(), String> {
        let mut missing = Vec::new();
        if !Pins::is_pinned(&self.plugin) {
            missing.push("AMD's device plugin");
        }
        if !Pins::is_pinned(&self.smoke) {
            missing.push("the ROCm test image");
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("ROCm for pods cannot be installed yet: {} must be pinned by digest first (the owner chooses them after a test on a real AMD card; see docs/project/PIN-DECISIONS.md). Nothing was changed; local models keep using Vulkan.", missing.join(" and ")))
        }
    }

    /// The ROCm Ollama image, when one is pinned and valid.
    pub fn ollama_image(&self) -> Option<&str> {
        Some(self.ollama.as_str()).filter(|i| Pins::is_pinned(i))
    }
}
