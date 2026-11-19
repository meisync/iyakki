//! Building a v2 update pack (docs/development/LAYERED-ROOT-DESIGN.md): `iyakki dev update-pack IMAGE -o DIR [--against PREV]`.
//! The image's boot partition carries the seed layers (`boot/layers.json`, `boot/layers/*.sfs`). The pack keeps what the device cannot get
//! otherwise: a layer whose hash is the same as in PREV is `keep` (nothing carried), a changed layer is `delta` against PREV's layer when
//! PREV's file is at hand and the delta is clearly smaller, else `full`.

use std::path::{Path, PathBuf};

use iyakki_stage1::layers::{LayerSet, DATA_LAYERS_DIR};

use super::files;
use super::update::{find_ci, Manifest};
use super::update_layers::{PackLayer, PackLayers};

/// A delta is used when it is at most this fraction (percent) of the new layer.
pub const DELTA_MAX_PERCENT: u64 = 80;

/// What `--against` named: the layer set of the version the device is expected to have, and where its layer files may be.
pub struct Previous {
    pub set: LayerSet,
    pub dirs: Vec<PathBuf>,
}

fn set_of(path: &Path) -> Option<LayerSet> {
    let text = std::fs::read_to_string(path).ok()?;
    LayerSet::parse(&text).ok().or_else(|| serde_json::from_str::<Manifest>(&text).ok().and_then(|m| m.layers).map(|l| l.set))
}

/// PREV is a folder (a layers output folder with `layers.json`, or an earlier pack with `update.json`) or a manifest file.
pub fn load_previous(p: &Path) -> Result<Previous, String> {
    if p.is_file() {
        let set = set_of(p).ok_or_else(|| format!("{}: not a layer manifest or a v2 update.json", p.display()))?;
        return Ok(Previous { set, dirs: p.parent().map(|d| vec![d.to_path_buf()]).unwrap_or_default() });
    }
    for rel in ["layers.json", "layers/layers.json", "boot/layers.json", "update.json"] {
        if let Some(set) = set_of(&p.join(rel)) {
            let dirs = [".", "layers", "boot/layers"].iter().map(|d| p.join(d)).collect();
            return Ok(Previous { set, dirs });
        }
    }
    Err(format!("{}: no layers.json or v2 update.json to compare with", p.display()))
}

fn find_layer_file(dirs: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(file_name)).find(|p| p.is_file())
}

/// The path of `p` relative to the pack folder, with `/` separators (the spelling the extraction gave it).
fn rel_in(dir: &Path, p: &Path) -> Result<String, String> {
    let r = p.strip_prefix(dir).map_err(|_| format!("{} is outside {}", p.display(), dir.display()))?;
    Ok(r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}

/// Rewrite the payload of an extracted image folder `dir` into a v2 pack. `files`: (path, size, sha256) as extracted. Returns the file
/// list of the pack (layer files that are not carried are deleted from `dir`) and the layer section; `None` when the image has no layers.
pub fn layerize(dir: &Path, files_in: Vec<(String, u64, String)>, against: Option<&Previous>, out: &mut dyn FnMut(&str)) -> Result<(Vec<(String, u64, String)>, Option<PackLayers>), String> {
    let is_manifest = |p: &str| p.eq_ignore_ascii_case("boot/layers.json");
    let Some(mf) = files_in.iter().find(|f| is_manifest(&f.0)) else { return Ok((files_in, None)) };
    let mpath = find_ci(dir, &mf.0).ok_or("boot/layers.json is missing after the copy")?;
    let seed = LayerSet::parse(&std::fs::read_to_string(&mpath).map_err(|e| e.to_string())?)?;
    let mut keep: Vec<(String, u64, String)> = files_in.iter().filter(|f| !is_manifest(&f.0) && !super::update_layers::is_payload(&f.0)).cloned().collect();
    let mut layers = Vec::new();
    let mut items = Vec::new();
    for l in &seed.layers {
        let src = find_ci(dir, &l.path).ok_or_else(|| format!("layer {}: {} is not in the image", l.name, l.path))?;
        let fname = l.path.rsplit('/').next().unwrap_or("").to_string();
        let mut nl = l.clone();
        nl.volume = "data".into();
        nl.path = format!("{DATA_LAYERS_DIR}/{fname}");
        let prev = against.and_then(|a| a.set.layers.iter().find(|x| x.name == l.name).map(|x| (x, a)));
        let item = if prev.is_some_and(|(p, _)| p.sha256 == l.sha256) {
            let _ = std::fs::remove_file(&src);
            out(&format!("  layer {}: unchanged ({})", l.name, &l.sha256[..12]));
            PackLayer { name: l.name.clone(), how: "keep".into(), file: String::new(), delta_from: String::new() }
        } else {
            let mut how = None;
            if let Some((p, a)) = prev {
                if let Some(old) = find_layer_file(&a.dirs, p.path.rsplit('/').next().unwrap_or("")) {
                    if files::sha256_file(&old).is_ok_and(|h| h == p.sha256) && p.size <= iyakki_bindelta::bindelta::MAX_FILE && l.size <= iyakki_bindelta::bindelta::MAX_FILE {
                        let (ob, nb) = (std::fs::read(&old).map_err(|e| e.to_string())?, std::fs::read(&src).map_err(|e| e.to_string())?);
                        let d = iyakki_bindelta::bindelta::make_delta(&ob, &nb);
                        if (d.len() as u64) * 100 <= l.size * DELTA_MAX_PERCENT {
                            // beside the layer file in the extracted folder: the FAT volume's own spelling of `boot/layers` (BOOT/LAYERS) is kept
                            let dname = format!("{}-{}.from-{}.delta", l.name, &l.sha256[..12], &p.sha256[..12]);
                            let dp = src.with_file_name(&dname);
                            let rel = rel_in(dir, &dp)?;
                            std::fs::write(&dp, &d).map_err(|e| format!("{}: {e}", dp.display()))?;
                            keep.push((rel.clone(), d.len() as u64, files::sha256_file(&dp)?));
                            out(&format!("  layer {}: difference against {}, {} KB (the layer is {} MB)", l.name, &p.sha256[..12], d.len().div_ceil(1024), l.size >> 20));
                            how = Some(PackLayer { name: l.name.clone(), how: "delta".into(), file: rel, delta_from: p.sha256.clone() });
                            let _ = std::fs::remove_file(&src);
                        } else {
                            out(&format!("  layer {}: the difference is {} KB, not clearly smaller than the layer: sent whole", l.name, d.len().div_ceil(1024)));
                        }
                    }
                }
            }
            match how {
                Some(h) => h,
                None => {
                    // whole: the file stays where it is in the folder; its pack path is boot/layers/NAME-HASH.sfs
                    let rel = rel_in(dir, &src)?;
                    keep.push((rel.clone(), l.size, l.sha256.clone()));
                    out(&format!("  layer {}: whole, {} MB", l.name, l.size >> 20));
                    PackLayer { name: l.name.clone(), how: "full".into(), file: rel, delta_from: String::new() }
                }
            }
        };
        layers.push(nl);
        items.push(item);
    }
    let _ = std::fs::remove_file(&mpath);
    keep.sort();
    let set = LayerSet { schema: seed.schema, release: seed.release.clone(), layers };
    Ok((keep, Some(PackLayers { set, items })))
}
