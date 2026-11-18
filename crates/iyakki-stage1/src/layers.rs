//! Layered root (docs/development/LAYERED-ROOT-DESIGN.md): the system as several read-only squashfs layers (base, core, ui, optional packs)
//! stacked with overlayfs, described by a layer manifest (`layers.json`). Pure decisions only: parsing, the requires check, the mount plan.
//! The system calls are in `main.rs`; the update code of the agent uses the same types (one definition of the manifest).
//!
//! Additive rules (docs/development/CORE-COMPATIBILITY-DESIGN.md): unknown fields are ignored, a NEWER schema is refused by name (never guessed),
//! and the single `root.sfs` root keeps working when no manifest exists.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{valid_id, Volume};

/// The schema number this program reads.
pub const LAYER_SCHEMA: u32 = 1;
/// Layer manifest of an install's seed layers on the boot volume, and the layer files beside it.
pub const BOOT_MANIFEST: &str = "boot/layers.json";
pub const BOOT_LAYERS_DIR: &str = "boot/layers";
/// Layers an update put on the data volume; `current.json` is the set that starts, `previous.json` the set it replaced.
pub const DATA_LAYERS_DIR: &str = "iyakki/layers";
pub const MAX_LAYERS: usize = 12;
/// Where the first stage mounts each layer (`/layers/<name>`).
pub const MOUNT_BASE: &str = "/layers";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    /// `base`, `core`, `ui` or the name of an optional pack.
    pub name: String,
    /// `boot` or `data`: the volume the file is on.
    pub volume: String,
    /// Path of the squashfs file inside that volume (`boot/layers/base-<12 hex>.sfs`, `iyakki/layers/core-<12 hex>.sfs`).
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// Interface numbers this layer offers (`core_iface: "1"`). Values are dotted numbers.
    #[serde(default)]
    pub provides: BTreeMap<String, String>,
    /// Ranges of other layers' interface numbers it needs (`base_abi: "1..1"`; `"2.."` is 2 or more, `"..3"` is 3 or less).
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
    /// A missing optional layer is left out of the stack; a missing required one makes the whole set unusable.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LayerSet {
    pub schema: u32,
    /// The release the set belongs to (informational).
    #[serde(default)]
    pub release: String,
    /// Bottom first: `base`, then `core`, then `ui`, then packs. The top layer wins where two have the same path.
    pub layers: Vec<Layer>,
}

fn clean_rel(p: &str) -> bool {
    !p.is_empty() && p.len() <= 200 && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch)))
}

pub fn volume_of(s: &str) -> Option<Volume> {
    match s {
        "boot" => Some(Volume::Boot),
        "data" => Some(Volume::Data),
        _ => None,
    }
}

/// `1`, `6.12`, `6.12.3` as numbers; anything else is not a version.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    if s.is_empty() || s.len() > 40 {
        return None;
    }
    s.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

fn cmp_version(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// Is `have` inside `range` (`lo..hi`, either side may be empty, a single number means exactly that)?
pub fn in_range(have: &str, range: &str) -> Result<bool, String> {
    let h = parse_version(have).ok_or_else(|| format!("{have:?} is not a version"))?;
    let (lo, hi) = match range.split_once("..") {
        Some((l, r)) => (l, r),
        None => (range, range),
    };
    if lo.is_empty() && hi.is_empty() {
        return Err(format!("range {range:?} is empty"));
    }
    if !lo.is_empty() && cmp_version(&h, &parse_version(lo).ok_or_else(|| format!("range {range:?} is not valid"))?).is_lt() {
        return Ok(false);
    }
    if !hi.is_empty() && cmp_version(&h, &parse_version(hi).ok_or_else(|| format!("range {range:?} is not valid"))?).is_gt() {
        return Ok(false);
    }
    Ok(true)
}

impl Layer {
    /// The file name the layer must have: `<name>-<first 12 hex of the SHA-256>.sfs` (content-addressed: a changed layer is a new file).
    pub fn file_name(&self) -> String {
        format!("{}-{}.sfs", self.name, self.sha256.get(..12).unwrap_or(""))
    }
}

impl LayerSet {
    pub fn parse(text: &str) -> Result<LayerSet, String> {
        let set: LayerSet = serde_json::from_str(text).map_err(|e| format!("layer manifest: {e}"))?;
        set.check()?;
        Ok(set)
    }

    /// Structure only (names, paths, hashes); the requires check is [`LayerSet::check_requires`].
    pub fn check(&self) -> Result<(), String> {
        if self.schema == 0 || self.schema > LAYER_SCHEMA {
            return Err(format!("layer manifest schema {} is not understood (this program reads schema {LAYER_SCHEMA}); update the first stage first", self.schema));
        }
        if self.layers.is_empty() || self.layers.len() > MAX_LAYERS {
            return Err(format!("a layer set has 1 to {MAX_LAYERS} layers"));
        }
        let mut seen: Vec<&str> = Vec::new();
        for l in &self.layers {
            if !valid_id(&l.name) || l.name.len() > 24 || seen.contains(&l.name.as_str()) {
                return Err(format!("layer name {:?} is not valid or is listed twice", l.name));
            }
            seen.push(&l.name);
            if volume_of(&l.volume).is_none() {
                return Err(format!("layer {}: volume {:?} is not boot or data", l.name, l.volume));
            }
            let dir = if l.volume == "boot" { BOOT_LAYERS_DIR } else { DATA_LAYERS_DIR };
            if !clean_rel(&l.path) || l.path != format!("{dir}/{}", l.file_name()) {
                return Err(format!("layer {}: path {:?} must be {dir}/{}", l.name, l.path, l.file_name()));
            }
            if l.sha256.len() != 64 || !l.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("layer {} has no valid SHA-256", l.name));
            }
            if l.size == 0 {
                return Err(format!("layer {} has size 0", l.name));
            }
            for (k, v) in &l.provides {
                if parse_version(v).is_none() || !valid_id(k) {
                    return Err(format!("layer {}: provides {k}={v:?} is not valid", l.name));
                }
            }
            for (k, v) in &l.requires {
                if !valid_id(k) || in_range("0", v).is_err() {
                    return Err(format!("layer {}: requires {k}={v:?} is not valid", l.name));
                }
            }
        }
        Ok(())
    }

    /// Every `requires` of the layers in `used` must be met by a `provides` of another used layer (or by `extra`, e.g. the kernel release).
    /// Returns the problems in plain words (empty: consistent).
    pub fn check_requires(&self, used: &[&str], extra: &BTreeMap<String, String>) -> Vec<String> {
        let mut have: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
        for l in self.layers.iter().filter(|l| used.contains(&l.name.as_str())) {
            for (k, v) in &l.provides {
                have.insert(k, (v, &l.name));
            }
        }
        let mut out = Vec::new();
        for l in self.layers.iter().filter(|l| used.contains(&l.name.as_str())) {
            for (k, range) in &l.requires {
                let got = have.get(k.as_str()).map(|(v, _)| *v).or_else(|| extra.get(k).map(String::as_str));
                match got {
                    None => out.push(format!("layer {} needs {k} {range}, which no layer of this set provides", l.name)),
                    Some(v) => match in_range(v, range) {
                        Ok(true) => {}
                        Ok(false) => out.push(format!("layer {} needs {k} {range} but this set has {k} {v}", l.name)),
                        Err(e) => out.push(format!("layer {}: {e}", l.name)),
                    },
                }
            }
        }
        out
    }

    pub fn names(&self) -> Vec<&str> {
        self.layers.iter().map(|l| l.name.as_str()).collect()
    }

    /// The text written to disk (stable key order, trailing newline).
    pub fn to_text(&self) -> String {
        let mut t = serde_json::to_string_pretty(self).unwrap_or_default();
        t.push('\n');
        t
    }
}

/// One squashfs to mount.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct LayerMount {
    pub name: String,
    pub volume: Volume,
    pub path: String,
    pub size: u64,
    pub mountpoint: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MountPlan {
    pub mounts: Vec<LayerMount>,
    /// Overlay `lowerdir` value: top layer first.
    pub lowerdir: String,
}

impl MountPlan {
    pub fn names(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.name.as_str()).collect()
    }
    pub fn uses(&self, v: Volume) -> bool {
        self.mounts.iter().any(|m| m.volume == v)
    }
}

/// Decide what to mount for a set: every file must exist with exactly the promised size (a half-written layer is a different size); a
/// missing optional layer is left out; the requires of what is left must hold. `stat(volume, path)`: file size or None.
pub fn plan_set(set: &LayerSet, extra: &BTreeMap<String, String>, stat: &dyn Fn(Volume, &str) -> Option<u64>) -> Result<MountPlan, String> {
    set.check()?;
    let mut mounts = Vec::new();
    for l in &set.layers {
        let vol = volume_of(&l.volume).ok_or("bad volume")?;
        match stat(vol, &l.path) {
            Some(n) if n == l.size => mounts.push(LayerMount { name: l.name.clone(), volume: vol, path: l.path.clone(), size: l.size, mountpoint: format!("{MOUNT_BASE}/{}", l.name) }),
            Some(n) if !l.optional => return Err(format!("layer {} ({}) is {n} bytes, the manifest says {}: an unfinished update", l.name, l.path, l.size)),
            None if !l.optional => return Err(format!("layer {} ({}) is missing", l.name, l.path)),
            _ => {}
        }
    }
    let names: Vec<&str> = mounts.iter().map(|m| m.name.as_str()).collect();
    let problems = set.check_requires(&names, extra);
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let lowerdir = mounts.iter().rev().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>().join(":");
    Ok(MountPlan { mounts, lowerdir })
}

/// The manifests to try, best first: (volume, path, label). `previous_only`: the boot menu's previous-version entry (never the newest set).
pub fn manifest_sources(previous_only: bool) -> Vec<(Volume, String, &'static str)> {
    let mut v = Vec::new();
    if !previous_only {
        v.push((Volume::Data, format!("{DATA_LAYERS_DIR}/current.json"), "current layers"));
    }
    v.push((Volume::Data, format!("{DATA_LAYERS_DIR}/previous.json"), "previous layers"));
    v.push((Volume::Boot, BOOT_MANIFEST.to_string(), "seed layers"));
    v
}

/// One thing the first stage tries to start from.
#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    /// The layer manifests of this volume.
    Layers(Volume),
    /// A single root image or tree.
    Root(crate::Candidate),
}

/// The order of attempts: the data volume's layer sets (an update's), the data volume's single roots, the boot volume's seed layers, the boot
/// volume's seed `root.sfs`. A single-root update after a layered one therefore wins over the seed layers, and a layered update wins over an
/// older single root (the single-root update retires the data volume's layer pointers).
pub fn attempt_order(list: Vec<crate::Candidate>) -> Vec<Attempt> {
    let (data, boot): (Vec<_>, Vec<_>) = list.into_iter().partition(|c| c.volume == Volume::Data);
    let mut out = vec![Attempt::Layers(Volume::Data)];
    out.extend(data.into_iter().map(Attempt::Root));
    out.push(Attempt::Layers(Volume::Boot));
    out.extend(boot.into_iter().map(Attempt::Root));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(name: &str, vol: &str, fill: char, size: u64, provides: &[(&str, &str)], requires: &[(&str, &str)]) -> Layer {
        let sha: String = std::iter::repeat_n(fill, 64).collect();
        let dir = if vol == "boot" { BOOT_LAYERS_DIR } else { DATA_LAYERS_DIR };
        Layer {
            name: name.into(),
            volume: vol.into(),
            path: format!("{dir}/{name}-{}.sfs", &sha[..12]),
            sha256: sha,
            size,
            provides: provides.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            requires: requires.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            optional: false,
        }
    }

    fn set() -> LayerSet {
        LayerSet {
            schema: 1,
            release: "0.1.0".into(),
            layers: vec![
                layer("base", "boot", 'a', 300, &[("base_abi", "1")], &[]),
                layer("core", "data", 'b', 40, &[("core_iface", "1")], &[("base_abi", "1..1")]),
                layer("ui", "data", 'c', 5, &[("ui_iface", "1")], &[("core_iface", "1..2"), ("base_abi", "1..")]),
            ],
        }
    }

    fn stat_all(set: &LayerSet) -> impl Fn(Volume, &str) -> Option<u64> + '_ {
        move |v, p| set.layers.iter().find(|l| l.path == p && volume_of(&l.volume) == Some(v)).map(|l| l.size)
    }

    #[test]
    fn the_manifest_round_trips_and_ignores_unknown_fields() {
        let s = set();
        assert_eq!(LayerSet::parse(&s.to_text()).unwrap(), s);
        let mut v: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        v["future"] = serde_json::json!({"x": 1});
        v["layers"][0]["note"] = "hello".into();
        assert_eq!(LayerSet::parse(&v.to_string()).unwrap(), s);
    }

    #[test]
    fn a_newer_schema_and_bad_manifests_are_refused_by_name() {
        let mut v: serde_json::Value = serde_json::from_str(&set().to_text()).unwrap();
        v["schema"] = 2.into();
        assert!(LayerSet::parse(&v.to_string()).unwrap_err().contains("schema 2"));
        for bad in ["", "{}", "[]", r#"{"schema":1,"layers":[]}"#] {
            assert!(LayerSet::parse(bad).is_err(), "{bad}");
        }
        let mut s = set();
        s.layers[1].path = "iyakki/layers/../x.sfs".into();
        assert!(s.check().is_err());
        let mut s = set();
        s.layers[1].path = s.layers[1].path.replace("bbbb", "cccc");
        assert!(s.check().is_err(), "the file name must carry the hash");
        let mut s = set();
        s.layers[2].name = "core".into();
        assert!(s.check().is_err(), "duplicate name");
        let mut s = set();
        s.layers[0].sha256 = "zz".into();
        assert!(s.check().is_err());
        let mut s = set();
        s.layers[0].volume = "cd".into();
        assert!(s.check().is_err());
    }

    #[test]
    fn version_ranges() {
        assert!(in_range("1", "1..1").unwrap());
        assert!(in_range("2", "1..").unwrap());
        assert!(!in_range("0", "1..").unwrap());
        assert!(in_range("3", "..3").unwrap());
        assert!(!in_range("4", "..3").unwrap());
        assert!(in_range("6.12.3", "6.12..6.13").unwrap());
        assert!(in_range("6.12", "6.12").unwrap());
        assert!(!in_range("6.11", "6.12").unwrap());
        assert!(in_range("x", "1").is_err());
        assert!(in_range("1", "..").is_err());
        assert!(in_range("1", "a..b").is_err());
    }

    #[test]
    fn the_mount_plan_stacks_top_first() {
        let s = set();
        let p = plan_set(&s, &BTreeMap::new(), &stat_all(&s)).unwrap();
        assert_eq!(p.names(), ["base", "core", "ui"]);
        assert_eq!(p.lowerdir, "/layers/ui:/layers/core:/layers/base");
        assert!(p.uses(Volume::Boot) && p.uses(Volume::Data));
        assert_eq!(p.mounts[0].mountpoint, "/layers/base");
    }

    #[test]
    fn a_missing_or_short_layer_makes_the_set_unusable_unless_optional() {
        let s = set();
        let none = |_: Volume, _: &str| None;
        assert!(plan_set(&s, &BTreeMap::new(), &none).unwrap_err().contains("missing"));
        let short = |v: Volume, p: &str| stat_all(&s)(v, p).map(|n| n - 1);
        assert!(plan_set(&s, &BTreeMap::new(), &short).unwrap_err().contains("unfinished"));
        let mut s2 = set();
        s2.layers.push({
            let mut l = layer("pack1", "data", 'd', 9, &[], &[]);
            l.optional = true;
            l
        });
        let st = stat_all(&s2);
        let p = plan_set(&s2, &BTreeMap::new(), &st).unwrap();
        assert_eq!(p.names(), ["base", "core", "ui", "pack1"]);
        let without = |v: Volume, path: &str| if path.contains("pack1") { None } else { st(v, path) };
        assert_eq!(plan_set(&s2, &BTreeMap::new(), &without).unwrap().names(), ["base", "core", "ui"]);
    }

    #[test]
    fn requires_that_do_not_match_refuse_the_set() {
        let mut s = set();
        s.layers[0].provides.insert("base_abi".into(), "2".into());
        let e = plan_set(&s, &BTreeMap::new(), &stat_all(&s)).unwrap_err();
        assert!(e.contains("layer core needs base_abi 1..1 but this set has base_abi 2"), "{e}");
        let mut s = set();
        s.layers.remove(0);
        let e = plan_set(&s, &BTreeMap::new(), &stat_all(&s)).unwrap_err();
        assert!(e.contains("no layer of this set provides"), "{e}");
        // extra facts (the kernel release) satisfy a requires
        let mut s = set();
        s.layers[0].requires.insert("kernel".into(), "6.12..6.13".into());
        assert!(plan_set(&s, &BTreeMap::new(), &stat_all(&s)).is_err());
        let extra: BTreeMap<String, String> = [("kernel".to_string(), "6.12.112".to_string())].into();
        assert!(plan_set(&s, &extra, &stat_all(&s)).is_ok());
        let extra: BTreeMap<String, String> = [("kernel".to_string(), "6.14".to_string())].into();
        assert!(plan_set(&s, &extra, &stat_all(&s)).is_err());
    }

    #[test]
    fn data_layers_then_data_roots_then_seed_layers_then_the_seed_root() {
        use crate::{Candidate, Kind};
        let c = |v, p: &str| Candidate { volume: v, kind: Kind::Image, path: p.into(), size: None };
        let order = attempt_order(vec![c(Volume::Data, "iyakki/root/a/root.sfs"), c(Volume::Data, "iyakki/root/b/root.sfs"), c(Volume::Boot, "boot/root.sfs")]);
        assert_eq!(
            order,
            vec![
                Attempt::Layers(Volume::Data),
                Attempt::Root(c(Volume::Data, "iyakki/root/a/root.sfs")),
                Attempt::Root(c(Volume::Data, "iyakki/root/b/root.sfs")),
                Attempt::Layers(Volume::Boot),
                Attempt::Root(c(Volume::Boot, "boot/root.sfs")),
            ]
        );
        // no single roots at all: just the two layer places
        assert_eq!(attempt_order(vec![]), vec![Attempt::Layers(Volume::Data), Attempt::Layers(Volume::Boot)]);
    }

    #[test]
    fn sources_are_current_previous_seed_and_the_previous_entry_skips_current() {
        let a: Vec<_> = manifest_sources(false).into_iter().map(|s| s.1).collect();
        assert_eq!(a, ["iyakki/layers/current.json", "iyakki/layers/previous.json", "boot/layers.json"]);
        let b: Vec<_> = manifest_sources(true).into_iter().map(|s| s.1).collect();
        assert_eq!(b, ["iyakki/layers/previous.json", "boot/layers.json"]);
    }
}
