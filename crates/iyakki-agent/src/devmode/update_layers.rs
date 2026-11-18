//! Update pack v2: layered root images (docs/development/LAYERED-ROOT-DESIGN.md). A v2 pack (`format` 2) describes the layer set the device
//! will have and carries only the layers that changed, whole or as a delta against the layer the device already has. The device checks the
//! whole plan (hashes, `requires`, the bases of deltas, room) before anything is staged, writes each changed layer as `NAME-HASH.sfs.new`,
//! checks it, renames it, and switches `iyakki/layers/current.json` LAST (the previous set becomes `previous.json`).
//! Pure planning is separate from the file work so both are tested without a device.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iyakki_stage1::layers::{self, Layer, LayerSet, DATA_LAYERS_DIR};
use iyakki_stage1::Volume;
use serde::{Deserialize, Serialize};

use super::files;
use super::update::{find_ci, FileEntry};

/// Pack files under this folder are layer payloads (whole layers or deltas); they are staged on the data volume and never written to the boot volume.
pub const PAYLOAD_DIR: &str = "boot/layers/";

pub fn is_payload(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with(PAYLOAD_DIR)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PackLayer {
    pub name: String,
    /// `keep` (the device must already have this layer), `full` (the squashfs is in the pack) or `delta` (a difference against `delta_from`).
    pub how: String,
    /// Path of the payload in the pack (`full`, `delta`).
    #[serde(default)]
    pub file: String,
    /// SHA-256 of the layer the delta is made against (`delta`).
    #[serde(default)]
    pub delta_from: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PackLayers {
    /// The set the device has after the update. In the pack every layer is written as `volume: data`; a `keep` layer is looked up on the device.
    pub set: LayerSet,
    pub items: Vec<PackLayer>,
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The pack's layer section on its own terms (no device): consistent with the file list, nothing stray under `boot/layers/`.
pub fn check_pack(pl: &PackLayers, files: &[FileEntry]) -> Result<(), String> {
    pl.set.check()?;
    if pl.items.len() != pl.set.layers.len() || pl.items.iter().zip(&pl.set.layers).any(|(i, l)| i.name != l.name) {
        return Err("the layer list of the update does not match its layer set".into());
    }
    let mut used: Vec<&str> = Vec::new();
    for (it, l) in pl.items.iter().zip(&pl.set.layers) {
        if l.volume != "data" {
            return Err(format!("layer {}: an update writes its layers to the data volume", l.name));
        }
        match it.how.as_str() {
            "keep" => {
                if !it.file.is_empty() {
                    return Err(format!("layer {} is kept but names a file", l.name));
                }
            }
            "full" | "delta" => {
                let f = files.iter().find(|f| f.path == it.file).ok_or_else(|| format!("layer {}: {:?} is not in the update's file list", l.name, it.file))?;
                if !is_payload(&f.path) {
                    return Err(format!("layer {}: {} is not under {PAYLOAD_DIR}", l.name, f.path));
                }
                if it.how == "full" && (f.sha256 != l.sha256 || f.size != l.size) {
                    return Err(format!("layer {}: the file in the update is not the layer the set promises", l.name));
                }
                if it.how == "delta" && !hex64(&it.delta_from) {
                    return Err(format!("layer {}: the delta has no valid base hash", l.name));
                }
                used.push(&it.file);
            }
            other => return Err(format!("layer {}: {other:?} is not keep, full or delta", l.name)),
        }
    }
    if let Some(f) = files.iter().find(|f| is_payload(&f.path) && !used.contains(&f.path.as_str())) {
        return Err(format!("{} is in the update but no layer uses it", f.path));
    }
    Ok(())
}

/// The layer set the device starts from now: `iyakki/layers/current.json` when it is valid, else the seed `boot/layers.json`. None: a single-root install.
pub fn device_set(data: &Path, boot_root: &Path) -> Option<LayerSet> {
    let from_data = std::fs::read_to_string(data.join(DATA_LAYERS_DIR).join("current.json")).ok().and_then(|t| LayerSet::parse(&t).ok());
    from_data.or_else(|| find_ci(boot_root, layers::BOOT_MANIFEST).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| LayerSet::parse(&t).ok()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Full,
    /// Against the layer the device has (volume, path, sha256).
    Delta { old_volume: Volume, old_path: String, old_sha256: String },
}

/// One layer file to write on the data volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub name: String,
    pub kind: Kind,
    /// The payload in the staging folder.
    pub staged: String,
    pub staged_size: u64,
    /// Final path on the data volume (`iyakki/layers/NAME-HASH.sfs`).
    pub dst: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerPlan {
    pub final_set: LayerSet,
    pub ops: Vec<Op>,
    pub notes: Vec<String>,
}

impl LayerPlan {
    /// Pack files that must be staged (the payloads of the layers the device does not have yet).
    pub fn payload_needed(&self) -> Vec<String> {
        self.ops.iter().map(|o| o.staged.clone()).collect()
    }
    /// Bytes the data volume needs for this plan: the staged payloads plus the results of deltas (a full layer is renamed, not copied).
    pub fn data_bytes(&self) -> u64 {
        self.ops.iter().map(|o| o.staged_size + if o.kind == Kind::Full { 0 } else { o.size }).sum()
    }
    /// Does the update change which layers start?
    pub fn changes(&self, cur: Option<&LayerSet>) -> bool {
        !self.ops.is_empty() || cur.is_none_or(|c| c.layers.len() != self.final_set.layers.len() || c.layers.iter().zip(&self.final_set.layers).any(|(a, b)| a.name != b.name || a.sha256 != b.sha256))
    }
}

fn short(h: &str) -> &str {
    h.get(..12).unwrap_or(h)
}

/// Decide what an update does to a device whose current set is `cur` (None: a single-root install). Refuses, in plain words and before any
/// byte is staged, a pack that needs a layer the device does not have or whose `requires` the resulting set cannot meet.
/// `extra`: facts the requires may name besides the layers (the running kernel release).
pub fn plan(pl: &PackLayers, files: &[FileEntry], cur: Option<&LayerSet>, extra: &BTreeMap<String, String>) -> Result<LayerPlan, String> {
    check_pack(pl, files)?;
    let mut out_layers: Vec<Layer> = Vec::new();
    let mut ops = Vec::new();
    let mut notes = Vec::new();
    for (it, l) in pl.items.iter().zip(&pl.set.layers) {
        let have = cur.and_then(|c| c.layers.iter().find(|x| x.name == l.name));
        let have_same = have.filter(|h| h.sha256 == l.sha256 && h.size == l.size);
        if it.how == "keep" || have_same.is_some() {
            let Some(h) = have_same else {
                return Err(match have {
                    Some(h) => format!("this update keeps the {} layer {} but this device has {}: it was made as a difference against another version. Update to the version it was made against first, or use a full pack", l.name, short(&l.sha256), short(&h.sha256)),
                    None => format!("this update keeps the {} layer {} but this device has no such layer (a single-root or older install): use a full pack", l.name, short(&l.sha256)),
                });
            };
            // keep the layer where it is (the seed base stays on the boot volume), with this pack's interface numbers
            let mut k = h.clone();
            k.provides = l.provides.clone();
            k.requires = l.requires.clone();
            k.optional = l.optional;
            out_layers.push(k);
            notes.push(format!("layer {}: unchanged", l.name));
            continue;
        }
        let f = files.iter().find(|f| f.path == it.file).ok_or("internal: payload missing")?;
        let kind = if it.how == "full" {
            notes.push(format!("layer {}: whole, {} MB", l.name, f.size.div_ceil(1 << 20)));
            Kind::Full
        } else {
            let h = have.filter(|h| h.sha256 == it.delta_from).ok_or_else(|| match have {
                Some(h) => format!("the {} layer in this update is a difference against {} but this device has {}: update to the version it was made against first, or use a full pack", l.name, short(&it.delta_from), short(&h.sha256)),
                None => format!("the {} layer in this update is a difference against {} but this device has no such layer: use a full pack", l.name, short(&it.delta_from)),
            })?;
            notes.push(format!("layer {}: difference, {} KB for a {} MB layer", l.name, f.size.div_ceil(1024), l.size.div_ceil(1 << 20)));
            Kind::Delta { old_volume: layers::volume_of(&h.volume).ok_or("bad volume")?, old_path: h.path.clone(), old_sha256: h.sha256.clone() }
        };
        ops.push(Op { name: l.name.clone(), kind, staged: f.path.clone(), staged_size: f.size, dst: l.path.clone(), sha256: l.sha256.clone(), size: l.size });
        out_layers.push(l.clone());
    }
    let final_set = LayerSet { schema: layers::LAYER_SCHEMA, release: pl.set.release.clone(), layers: out_layers };
    final_set.check()?;
    let names: Vec<&str> = final_set.names();
    let problems = final_set.check_requires(&names, extra);
    if !problems.is_empty() {
        return Err(format!("the layers of this update do not fit together: {}. Nothing was changed", problems.join("; ")));
    }
    Ok(LayerPlan { final_set, ops, notes })
}

// -------------------------------------------------------------------------------------------- applying

/// Test knob: stop (an error, as a power cut would) at this step of [`install`]. Steps: 1 after a layer's `.new` file is written, 2 after it is
/// renamed, 3 after `previous.json`, 4 after `current.json` (before the cleanup).
#[cfg(test)]
pub(super) static FAULT_STEP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
fn fault(step: u32) -> Result<(), String> {
    if FAULT_STEP.load(std::sync::atomic::Ordering::SeqCst) == step {
        return Err(format!("(test) power cut at step {step}"));
    }
    Ok(())
}
#[cfg(not(test))]
fn fault(_step: u32) -> Result<(), String> {
    Ok(())
}

fn fsync_file(p: &Path) -> Result<(), String> {
    std::fs::File::open(p).and_then(|f| f.sync_all()).map_err(|e| format!("{}: {e}", p.display()))
}

fn fsync_dir(p: &Path) {
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.sync_all();
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_file_name(format!("{}.new", path.file_name().unwrap_or_default().to_string_lossy()));
    let mut w = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    w.write_all(text.as_bytes()).and_then(|_| w.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(d) = path.parent() {
        fsync_dir(d);
    }
    Ok(())
}

/// Write the changed layers and switch to the new set. `boot_root`: the boot volume as a folder (the seed layers are read from it);
/// `staging`: where the payloads were staged (the same volume as `data`, so a whole layer is renamed, not copied).
/// Order, each step safe to stop after: layer files (`.new`, check, rename), `previous.json`, `current.json` LAST, cleanup.
pub fn install(data: &Path, boot_root: &Path, staging: &Path, cur: Option<&LayerSet>, lp: &LayerPlan) -> Result<(), String> {
    let dir = data.join(DATA_LAYERS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for op in &lp.ops {
        let dst: PathBuf = data.join(&op.dst);
        let new = dst.with_file_name(format!("{}.new", dst.file_name().unwrap_or_default().to_string_lossy()));
        let _ = std::fs::remove_file(&new);
        let src = staging.join(&op.staged);
        match &op.kind {
            Kind::Full => {
                if std::fs::rename(&src, &new).is_err() {
                    std::fs::copy(&src, &new).map_err(|e| format!("{}: {e}", new.display()))?;
                }
            }
            Kind::Delta { old_volume, old_path, .. } => {
                let old = match old_volume {
                    Volume::Boot => find_ci(boot_root, old_path).ok_or_else(|| format!("the {} layer on the boot volume is missing", op.name))?,
                    Volume::Data => data.join(old_path),
                };
                iyakki_bindelta::bindelta::apply_delta_files(&old, &src, &new).map_err(|e| format!("the {} layer could not be made from the difference: {e}", op.name))?;
            }
        }
        fsync_file(&new)?;
        let ok = std::fs::metadata(&new).is_ok_and(|m| m.len() == op.size) && files::sha256_file(&new).is_ok_and(|h| h == op.sha256);
        if !ok {
            let _ = std::fs::remove_file(&new);
            return Err(format!("the {} layer does not match its SHA-256 after it was written: nothing was changed", op.name));
        }
        fault(1)?;
        std::fs::rename(&new, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        fsync_dir(&dir);
        fault(2)?;
    }
    // the set being replaced is kept as previous.json (the boot menu's previous-version entry starts it)
    if let Some(c) = cur {
        write_atomic(&dir.join("previous.json"), &c.to_text())?;
    } else if let Ok(old) = std::fs::read_to_string(data.join(iyakki_stage1::ROOTS_DIR).join("current")) {
        // a single-root install becoming layered: its root stays startable as the "previous" single root
        let _ = std::fs::create_dir_all(data.join(iyakki_stage1::ROOTS_DIR));
        write_atomic(&data.join(iyakki_stage1::ROOTS_DIR).join("previous"), &old)?;
    }
    fault(3)?;
    write_atomic(&dir.join("current.json"), &lp.final_set.to_text())?;
    fault(4)?;
    // layer files of the data volume that neither set names any more
    let mut keep: Vec<String> = Vec::new();
    for n in ["current.json", "previous.json"] {
        if let Some(s) = std::fs::read_to_string(dir.join(n)).ok().and_then(|t| LayerSet::parse(&t).ok()) {
            keep.extend(s.layers.iter().filter(|l| l.volume == "data").map(|l| l.path.rsplit('/').next().unwrap_or("").to_string()));
        }
    }
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if (name.ends_with(".sfs") || name.ends_with(".new") || name.ends_with(".applying")) && !keep.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

/// A single-root update after a layered one: the layer pointers are retired so the newer root is not shadowed by an older layer set.
pub fn retire_pointers(data: &Path) {
    let dir = data.join(DATA_LAYERS_DIR);
    for n in ["current.json", "previous.json"] {
        let p = dir.join(n);
        if p.exists() {
            let _ = std::fs::rename(&p, dir.join(format!("{n}.retired")));
        }
    }
    fsync_dir(&dir);
}
