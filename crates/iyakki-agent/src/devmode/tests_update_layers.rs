//! Update pack v2 (layered root) without a device: temp folders for the boot and data volumes. Building the pack (keep, delta, full), the
//! refusals before anything is staged, the order of the writes, and what a power cut after each step leaves (the old set or the new set,
//! never a mixture the first stage would refuse).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_stage1::layers::{self, Layer, LayerSet};
use iyakki_stage1::Volume;

use super::pack_layers::{layerize, load_previous};
use super::tests_update::{env, pseudo, put, rig, stage_all, Rig};
use super::update::*;
use super::update_layers::{self, FAULT_STEP};

/// The fault knob is global: the tests of this file that use it run one at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAULT_STEP.store(0, std::sync::atomic::Ordering::SeqCst);
    g
}

fn sha(b: &[u8]) -> String {
    sha256_hex(b)
}

/// One layer of a set. `vol`: where its file is (`boot` for a seed, `data` for an update).
fn layer(name: &str, bytes: &[u8], vol: &str, provides: &[(&str, &str)], requires: &[(&str, &str)]) -> Layer {
    let h = sha(bytes);
    let dir = if vol == "boot" { layers::BOOT_LAYERS_DIR } else { layers::DATA_LAYERS_DIR };
    Layer {
        name: name.into(),
        volume: vol.into(),
        path: format!("{dir}/{name}-{}.sfs", &h[..12]),
        sha256: h,
        size: bytes.len() as u64,
        provides: provides.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        requires: requires.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        optional: false,
    }
}

struct Bytes {
    base: Vec<u8>,
    core: Vec<u8>,
    ui: Vec<u8>,
}

/// Bytes with no repeating structure (a replaced layer: nothing for a delta to find).
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x >> 24) as u8 }).collect()
}

fn v1_bytes() -> Bytes {
    Bytes { base: pseudo(3 << 20, 21), core: pseudo(600_000, 22), ui: pseudo(200_000, 23) }
}

/// Core changed in a few places (what a rebuilt program looks like to a delta), ui replaced by different bytes, base the same.
fn v2_bytes() -> Bytes {
    let v = v1_bytes();
    let mut core = v.core.clone();
    for i in (1000..core.len()).step_by(50_000) {
        core[i] ^= 0x5a;
    }
    Bytes { base: v.base, core, ui: noise(210_000, 99) }
}

fn set_of(b: &Bytes, vol: &str) -> LayerSet {
    LayerSet {
        schema: 1,
        release: "t".into(),
        layers: vec![
            layer("base", &b.base, vol, &[("base_abi", "1")], &[]),
            layer("core", &b.core, vol, &[("core_iface", "1")], &[("base_abi", "1..1")]),
            layer("ui", &b.ui, vol, &[], &[("core_iface", "1..1")]),
        ],
    }
}

fn write_layers(root: &Path, b: &Bytes, set: &LayerSet) {
    for (l, bytes) in set.layers.iter().zip([&b.base, &b.core, &b.ui]) {
        put(root, &l.path, bytes);
    }
}

/// A folder as a build leaves it (`layers.json` and the squashfs files): what `--against` reads.
fn build_folder(r: &Rig, tag: &str, b: &Bytes) -> (PathBuf, LayerSet) {
    let d = r.dir.join(format!("build-{tag}"));
    let set = set_of(b, "boot");
    write_layers(&d, b, &set);
    put(&d, "layers.json", set.to_text().as_bytes());
    (d, set)
}

/// The extracted image folder `iyakki dev update-pack` starts from: the boot files of the new version plus the seed layers.
fn image_folder(r: &Rig, tag: &str, version: &str, b: &Bytes) -> (PathBuf, Vec<(String, u64, String)>) {
    let d = r.dir.join(format!("image-{tag}"));
    let _ = std::fs::remove_dir_all(&d);
    put(&d, "boot/bzImage", &pseudo((1 << 20) + 5000, 11));
    put(&d, "boot/initrd-lean", &pseudo(300_000, 12));
    put(&d, "boot/grub/grub.cfg", super::tests_update::board_cfg().as_bytes());
    put(&d, "EFI/BOOT/bootx64.efi", &pseudo(300_000, 3));
    put(&d, "boot/iyakki-release", format!("VERSION={version}\nVARIANT=macweb\nBUILD_DATE=2026-10-22\n").as_bytes());
    let set = set_of(b, "boot");
    write_layers(&d, b, &set);
    put(&d, "boot/layers.json", set.to_text().as_bytes());
    // the FAT volume spells 8.3-compatible names in capitals (mmd/mcopy): the folder reads back as BOOT/LAYERS, the manifest as LAYERS.JSON
    std::fs::rename(d.join("boot/layers"), d.join("boot/LAYERS")).unwrap();
    std::fs::rename(d.join("boot/layers.json"), d.join("boot/LAYERS.JSON")).unwrap();
    let list = list_boot(&d).into_iter().map(|(p, s)| (p.clone(), s, super::files::sha256_file(&d.join(&p)).unwrap())).collect();
    (d, list)
}

/// Build the v2 pack the way `pack()` does; returns the folder and manifest.
fn make_pack(r: &Rig, tag: &str, version: &str, b: &Bytes, against: Option<&Path>) -> (PathBuf, Manifest) {
    let (d, list) = image_folder(r, tag, version, b);
    let prev = against.map(|a| load_previous(a).unwrap());
    let (list, layers) = layerize(&d, list, prev.as_ref(), &mut |_| {}).unwrap();
    let mut m = build_manifest(&d, &list, None, None, "t.img").unwrap();
    m.format = FORMAT_LAYERS;
    m.layers = layers;
    check_manifest(&m).unwrap();
    write_manifest(&d, &m).unwrap();
    (d, m)
}

/// A device that boots the v1 seed set: layers on the boot volume, no `iyakki/layers` yet. The partition has room for them.
fn layered_device(tag: &str) -> (Rig, PathBuf, LayerSet) {
    let mut r = rig(tag);
    r.boot.capacity = 64 << 20;
    let b = v1_bytes();
    let set = set_of(&b, "boot");
    write_layers(&r.boot.root, &b, &set);
    put(&r.boot.root, "boot/layers.json", set.to_text().as_bytes());
    // the lean device starts from the small RAM disk
    let (folder, _) = build_folder(&r, "v1", &b);
    (r, folder, set)
}

fn free() -> u64 {
    500 << 20
}

/// What the first stage would start from this device now (the same manifest order and the same checks, without mounting).
fn boots_as(r: &Rig) -> Option<Vec<String>> {
    let data = r.dm.data_dir();
    let stat = |v: Volume, p: &str| {
        let root = if v == Volume::Boot { r.boot.root.clone() } else { data.to_path_buf() };
        std::fs::metadata(root.join(p)).ok().filter(|m| m.is_file()).map(|m| m.len())
    };
    for (vol, path, _) in layers::manifest_sources(false) {
        let root = if vol == Volume::Boot { r.boot.root.clone() } else { data.to_path_buf() };
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else { continue };
        let Ok(set) = LayerSet::parse(&text) else { continue };
        if let Ok(plan) = layers::plan_set(&set, &BTreeMap::new(), &stat) {
            return Some(plan.mounts.iter().map(|m| format!("{}:{}", m.name, set.layers.iter().find(|l| l.name == m.name).unwrap().sha256[..12].to_string())).collect());
        }
    }
    None
}

fn names(b: &Bytes) -> Vec<String> {
    set_of(b, "boot").layers.iter().map(|l| format!("{}:{}", l.name, &l.sha256[..12])).collect()
}

fn apply_pack(r: &Rig, dir: &Path, m: &Manifest) -> Result<serde_json::Value, String> {
    let f = free;
    let e = env(r, &f, None);
    let p = plan(&e, m, false)?;
    stage_all(&e, dir, m, &p.needs);
    apply(&e, m, false, "laptop")
}

#[test]
fn the_pack_keeps_unchanged_layers_sends_a_difference_for_a_rebuilt_one_and_the_whole_of_a_replaced_one() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-build");
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    assert_eq!(m.format, 2);
    let pl = m.layers.as_ref().unwrap();
    let how: Vec<(&str, &str)> = pl.items.iter().map(|i| (i.name.as_str(), i.how.as_str())).collect();
    assert_eq!(how, [("base", "keep"), ("core", "delta"), ("ui", "full")]);
    // only the changed layers travel: no base, and the core as a small difference
    let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).filter(|p| p.to_ascii_lowercase().starts_with("boot/layers/")).collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    let delta = m.files.iter().find(|f| f.path.ends_with(".delta")).unwrap();
    assert!(delta.size < 100_000, "a delta of {} bytes for a 600 KB layer", delta.size);
    assert!(!d.join("boot/layers.json").exists() && !d.join(&set_of(&v2_bytes(), "boot").layers[0].path).exists());
    verify_payload(&d, &m).unwrap();
    // the layer section round-trips through update.json (an old reader sees `format` 2 first)
    let back = read_manifest(&d).unwrap();
    assert_eq!(back, m);
    // without --against everything is whole
    let (_, m0) = make_pack(&r, "v2-all", "0.1.0-alpha.2", &v2_bytes(), None);
    assert!(m0.layers.unwrap().items.iter().all(|i| i.how == "full"));
}

#[test]
fn an_old_reader_of_format_1_refuses_a_v2_pack_by_name_and_a_v1_pack_is_unchanged() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-format");
    let (_, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    let text = serde_json::to_string(&m).unwrap();
    // what a device that only reads format 1 sees: the number first, then it stops
    let old: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(old["format"], 2);
    // this device refuses an unknown future format and a format/section mismatch
    let mut bad = m.clone();
    bad.format = 3;
    assert!(check_manifest(&bad).unwrap_err().contains("format 3 is not understood"));
    let mut bad = m.clone();
    bad.format = 1;
    assert!(check_manifest(&bad).unwrap_err().contains("must say format 2"));
    // a format 1 manifest is serialised without the `layers` key at all
    let (_, v1) = super::tests_update::payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    assert!(!serde_json::to_string(&v1).unwrap().contains("layers"));
    assert_eq!(v1.format, 1);
}

#[test]
fn a_difference_applies_to_the_seed_layers_and_the_set_switches_last() {
    let _g = lock();
    let (r, v1dir, seed) = layered_device("lay-apply");
    assert_eq!(boots_as(&r).unwrap(), names(&v1_bytes()), "before: the seed set starts");
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    let f = free;
    let e = env(&r, &f, None);
    let p = plan(&e, &m, false).unwrap();
    assert!(p.notes.iter().any(|n| n.contains("layer base: unchanged")) && p.notes.iter().any(|n| n.contains("layer core: difference")), "{:?}", p.notes);
    stage_all(&e, &d, &m, &p.needs);
    let v = apply(&e, &m, false, "laptop").unwrap();
    assert_eq!(v["system_image"], true);
    assert_eq!(boots_as(&r).unwrap(), names(&v2_bytes()));
    let data = r.dm.data_dir();
    let cur = LayerSet::parse(&std::fs::read_to_string(data.join("iyakki/layers/current.json")).unwrap()).unwrap();
    // base stays where it is (boot volume), the new layers are on the data volume
    assert_eq!(cur.layers.iter().map(|l| l.volume.as_str()).collect::<Vec<_>>(), ["boot", "data", "data"]);
    assert_eq!(cur.layers[0].path, seed.layers[0].path);
    for l in &cur.layers {
        let root = if l.volume == "boot" { &r.boot.root } else { data };
        assert_eq!(super::files::sha256_file(&root.join(&l.path)).unwrap(), l.sha256, "{}", l.name);
    }
    // the replaced set is kept as previous.json
    let prev = LayerSet::parse(&std::fs::read_to_string(data.join("iyakki/layers/previous.json")).unwrap()).unwrap();
    assert_eq!(prev, seed);
    // the boot menu's previous entry starts it (iyakki.root=previous)
    let cfg = std::fs::read_to_string(r.boot.root.join("boot/grub/grub.cfg")).unwrap();
    assert!(cfg.contains("iyakki.root=previous"));
    // the seed layers on the boot volume are untouched
    assert_eq!(super::files::sha256_file(&r.boot.root.join(&seed.layers[0].path)).unwrap(), seed.layers[0].sha256);
}

#[test]
fn a_second_update_moves_current_to_previous_and_removes_the_layer_files_no_set_names() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-second");
    let (d2, m2) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    apply_pack(&r, &d2, &m2).unwrap();
    let v2dir = {
        let (dir, _) = build_folder(&r, "v2", &v2_bytes());
        dir
    };
    let mut v3 = v2_bytes();
    v3.core[2000] ^= 0xff;
    v3.ui = noise(215_000, 55);
    let (d3, m3) = make_pack(&r, "v3", "0.1.0-alpha.3", &v3, Some(&v2dir));
    apply_pack(&r, &d3, &m3).unwrap();
    assert_eq!(boots_as(&r).unwrap(), names(&v3));
    let lay = r.dm.data_dir().join("iyakki/layers");
    let prev = LayerSet::parse(&std::fs::read_to_string(lay.join("previous.json")).unwrap()).unwrap();
    assert_eq!(prev.layers[1].sha256, set_of(&v2_bytes(), "boot").layers[1].sha256, "previous is the set that was replaced");
    let mut files: Vec<String> = std::fs::read_dir(&lay).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".sfs")).collect();
    files.sort();
    // current (core3, ui3) and previous (core2, ui2); the seed core and ui of version 1 are gone, nothing else is left over
    assert_eq!(files.len(), 4, "{files:?}");
    assert!(std::fs::read_dir(&lay).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".new")));
}

#[test]
fn a_difference_against_a_layer_the_device_does_not_have_is_refused_before_anything_is_staged() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-refuse-base");
    // the pack was made against a version whose core is another one
    let mut other = v1_bytes();
    other.core[10] ^= 1;
    let (other_dir, _) = build_folder(&r, "other", &other);
    let _ = v1dir;
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&other_dir));
    let f = free;
    let e = env(&r, &f, None);
    let err = plan(&e, &m, false).unwrap_err();
    assert!(err.contains("core layer in this update is a difference against") && err.contains("this device has"), "{err}");
    assert!(!r.dm.data_dir().join("boot-next").exists() && !r.dm.data_dir().join("iyakki/layers").exists());
    let _ = d;
    // a kept layer the device lacks
    let mut other2 = v1_bytes();
    other2.base[5] ^= 1;
    let (other2_dir, _) = build_folder(&r, "other2", &other2);
    let mut v = v2_bytes();
    v.base = other2.base.clone();
    let (_, m2) = make_pack(&r, "v2b", "0.1.0-alpha.2", &v, Some(&other2_dir));
    // base differs from the pack's keep: build a pack whose base is `keep` of other2 = same as v.base
    let err = plan(&env(&r, &f, None), &m2, false);
    assert!(err.is_err() || m2.layers.as_ref().unwrap().items[0].how != "keep");
}

#[test]
fn layers_whose_requires_do_not_fit_are_refused_and_nothing_is_changed() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-requires");
    let (d, mut m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    // the new ui needs a core interface 2 that no layer of the set provides
    m.layers.as_mut().unwrap().set.layers[2].requires.insert("core_iface".into(), "2..2".into());
    write_manifest(&d, &m).unwrap();
    let f = free;
    let err = plan(&env(&r, &f, None), &m, false).unwrap_err();
    assert!(err.contains("do not fit together") && err.contains("layer ui needs core_iface 2..2 but this set has core_iface 1"), "{err}");
    assert_eq!(boots_as(&r).unwrap(), names(&v1_bytes()));
    assert!(!r.dm.data_dir().join("iyakki/layers").exists());
}

#[test]
fn a_pack_that_keeps_a_layer_is_refused_on_an_install_without_layers() {
    let _g = lock();
    let r = rig("lay-single");
    let (v1dir, _) = build_folder(&r, "v1", &v1_bytes());
    let (_, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    let f = free;
    let err = plan(&env(&r, &f, None), &m, false).unwrap_err();
    assert!(err.contains("has no such layer") || err.contains("single-root"), "{err}");
}

#[test]
fn a_full_pack_turns_a_single_root_install_into_a_layered_one_and_keeps_the_old_root_as_previous() {
    let _g = lock();
    let r = rig("lay-first");
    let data = r.dm.data_dir().to_path_buf();
    put(&data, "iyakki/root/old-root/root.sfs", b"old root image");
    put(&data, "iyakki/root/current", b"old-root\nsize=14\n");
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), None);
    let v = apply_pack(&r, &d, &m).unwrap();
    assert_eq!(v["system_image"], true);
    assert_eq!(boots_as(&r).unwrap(), names(&v2_bytes()).into_iter().collect::<Vec<_>>());
    assert_eq!(std::fs::read_to_string(data.join("iyakki/root/previous")).unwrap(), "old-root\nsize=14\n", "the old root is the previous version");
}

#[test]
fn a_single_root_update_after_a_layered_one_retires_the_layer_pointers() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-retire");
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    apply_pack(&r, &d, &m).unwrap();
    let lay = r.dm.data_dir().join("iyakki/layers");
    assert!(lay.join("current.json").exists());
    update_layers::retire_pointers(r.dm.data_dir());
    assert!(!lay.join("current.json").exists() && lay.join("current.json.retired").exists() && lay.join("previous.json.retired").exists());
    // with the pointers retired the first stage falls to the seed layers on the boot volume
    assert_eq!(boots_as(&r).unwrap(), names(&v1_bytes()));
}

#[test]
fn a_power_cut_after_any_step_leaves_a_set_the_first_stage_can_start_and_the_update_can_be_repeated() {
    let _g = lock();
    for step in 1..=4u32 {
        let (r, v1dir, _) = layered_device(&format!("lay-cut{step}"));
        let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
        let f = free;
        let e = env(&r, &f, None);
        let p = plan(&e, &m, false).unwrap();
        stage_all(&e, &d, &m, &p.needs);
        FAULT_STEP.store(step, std::sync::atomic::Ordering::SeqCst);
        let err = apply(&e, &m, false, "laptop").unwrap_err();
        FAULT_STEP.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(err.contains("power cut at step"), "step {step}: {err}");
        let state = boots_as(&r).unwrap_or_else(|| panic!("step {step}: no set can start"));
        // before current.json is written the OLD set starts; after, the NEW one, whole
        let want = if step >= 4 { names(&v2_bytes()) } else { names(&v1_bytes()) };
        assert_eq!(state, want, "step {step}");
        // the kernel and menu were not replaced yet: nothing the cut left is half-new on the boot volume's layer files
        assert_eq!(super::files::sha256_file(&r.boot.root.join(&set_of(&v1_bytes(), "boot").layers[0].path)).unwrap(), set_of(&v1_bytes(), "boot").layers[0].sha256);
        // power is back: the same pack applies again (the person's computer sends what is missing)
        let e2 = env(&r, &f, None);
        let p2 = plan(&e2, &m, false).unwrap();
        stage_all(&e2, &d, &m, &p2.needs);
        apply(&e2, &m, false, "laptop").unwrap_or_else(|er| panic!("step {step}: repeat failed: {er}"));
        assert_eq!(boots_as(&r).unwrap(), names(&v2_bytes()), "step {step}");
        assert!(status(r.dm.data_dir())["interrupted"].is_null());
    }
}

#[test]
fn a_layer_that_is_damaged_on_the_way_is_never_renamed_into_place() {
    let _g = lock();
    let (r, v1dir, _) = layered_device("lay-damaged");
    let (d, m) = make_pack(&r, "v2", "0.1.0-alpha.2", &v2_bytes(), Some(&v1dir));
    let f = free;
    let e = env(&r, &f, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &d, &m, &p.needs);
    // a bit flips in the staged whole layer (the ui) after staging
    let ui = m.layers.as_ref().unwrap().items.iter().find(|i| i.name == "ui").unwrap().file.clone();
    let sp = r.dm.data_dir().join("boot-next").join(&ui);
    let mut b = std::fs::read(&sp).unwrap();
    b[100] ^= 1;
    std::fs::write(&sp, b).unwrap();
    let err = apply(&e, &m, false, "laptop").unwrap_err();
    assert!(err.contains("not staged completely"), "{err}");
    assert_eq!(boots_as(&r).unwrap(), names(&v1_bytes()));
    assert!(!r.dm.data_dir().join("iyakki/layers/current.json").exists());
}
