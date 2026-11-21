//! The optional kernel-module pack on the device (F-PKG-11): which optional module a piece of hardware needs, and putting
//! exactly those modules back, checked against the manifest the image builder wrote (`modsplit::compress_tree`).
//!
//! The image keeps two small files, `optional-pack/manifest.txt` (`module <size> <sha256> <path>`) and `optional-pack/optional.map`
//! (`alias <pattern> <path>` and `dep <path> <dep path>...` lines, written before the modules left). From a device's modalias
//! strings [`needed`] names the optional modules (and the optional ones they need) that are not in the tree; [`install`] takes
//! their bytes from an injected `fetch`, checks size and SHA-256 against the manifest, writes each file whole (temporary name,
//! then rename) and runs `depmod` through a trait. Nothing is fetched or run here by itself; a path that leaves the module tree
//! is refused; a file that does not match the manifest is never written.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub release: String,
    pub modules: BTreeMap<String, Item>,
}

/// A module path inside the tree: relative, no `..`, ends in `.ko`.
pub fn safe_path(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.contains('\\') && !p.contains('\0') && p.ends_with(".ko") && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut m = Manifest::default();
        for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let f: Vec<&str> = l.splitn(4, ' ').collect();
            match f.as_slice() {
                ["release", r] => m.release = (*r).to_string(),
                ["module", size, sha, path] => {
                    let size = size.parse::<u64>().map_err(|_| format!("bad size in: {l}"))?;
                    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || !safe_path(path) {
                        return Err(format!("bad manifest line: {l}"));
                    }
                    m.modules.insert((*path).to_string(), Item { size, sha256: sha.to_ascii_lowercase() });
                }
                _ => return Err(format!("bad manifest line: {l}")),
            }
        }
        if m.release.is_empty() {
            return Err("the manifest names no kernel release".into());
        }
        Ok(m)
    }
}

/// The module name the kernel uses for a file path (`-` and `_` are the same name).
pub fn module_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.trim_end_matches(".gz").trim_end_matches(".ko").replace('-', "_")
}

/// The `optional.map` text for a split: the aliases of the optional modules (from a `modules.alias`) and their dependencies.
pub fn map_text(optional: &BTreeSet<String>, dep: &BTreeMap<String, Vec<String>>, alias_text: &str) -> String {
    let by_name: BTreeMap<String, &String> = optional.iter().map(|p| (module_name(p), p)).collect();
    let mut out = String::new();
    for l in alias_text.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if let ["alias", pat, name] = f.as_slice() {
            if let Some(p) = by_name.get(&name.replace('-', "_")) {
                out.push_str(&format!("alias {pat} {p}\n"));
            }
        }
    }
    for p in optional {
        let ds: Vec<&String> = dep.get(p).map(|d| d.iter().filter(|x| optional.contains(*x)).collect()).unwrap_or_default();
        if !ds.is_empty() {
            out.push_str(&format!("dep {p} {}\n", ds.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")));
        }
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct Map {
    pub aliases: Vec<(String, String)>,
    pub deps: BTreeMap<String, Vec<String>>,
}

impl Map {
    pub fn parse(text: &str) -> Map {
        let mut m = Map::default();
        for l in text.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            match f.as_slice() {
                ["alias", pat, p] => m.aliases.push(((*pat).to_string(), (*p).to_string())),
                ["dep", p, rest @ ..] => {
                    m.deps.insert((*p).to_string(), rest.iter().map(|s| (*s).to_string()).collect());
                }
                _ => {}
            }
        }
        m
    }
}

/// Shell-style match with `*` and `?` (what `modules.alias` patterns use; bracket sets are matched literally).
pub fn glob(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(si) = star {
            i = si + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// The optional modules (with the optional ones they need) that the given device aliases call for and that are not in `have`.
pub fn needed(manifest: &Manifest, map: &Map, modaliases: &[String], have: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut want: BTreeSet<String> = BTreeSet::new();
    for (pat, p) in &map.aliases {
        if manifest.modules.contains_key(p) && modaliases.iter().any(|a| glob(pat, a)) {
            want.insert(p.clone());
        }
    }
    let mut stack: Vec<String> = want.iter().cloned().collect();
    while let Some(p) = stack.pop() {
        for d in map.deps.get(&p).into_iter().flatten() {
            if manifest.modules.contains_key(d) && want.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    want.into_iter().filter(|p| !have(p)).collect()
}

/// The two files the image keeps, read from `root` (the `lib/modules/RELEASE` folder): `Ok(None)` when this image has no optional pack
/// (the modules were never split out).
pub fn read_pack(root: &Path) -> Result<Option<(Manifest, Map)>, String> {
    let dir = root.join("optional-pack");
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.txt")) else { return Ok(None) };
    let manifest = Manifest::parse(&text)?;
    let map = Map::parse(&std::fs::read_to_string(dir.join("optional.map")).map_err(|e| format!("optional-pack/optional.map: {e}"))?);
    Ok(Some((manifest, map)))
}

/// The optional modules this machine's devices call for and that are not in the tree yet. `Ok(vec![])` also for an image without a pack.
pub fn needed_here(root: &Path, modaliases: &[String]) -> Result<Vec<String>, String> {
    let Some((m, map)) = read_pack(root)? else { return Ok(Vec::new()) };
    Ok(needed(&m, &map, modaliases, &|p| root.join(p).exists()))
}

/// The id of exactly this list (path, size and checksum of each file): an approval counts for this list only.
pub fn list_id(manifest: &Manifest, wanted: &[String]) -> String {
    let mut h = Sha256::new();
    for p in wanted {
        if let Some(i) = manifest.modules.get(p) {
            h.update(format!("{p} {} {}\n", i.size, i.sha256));
        }
    }
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

/// The address of one module under the person's base address. Only `https`, no user name, query or fragment, and the module path
/// must be a safe path of the manifest; anything else is refused (the address is never taken from the pack itself).
pub fn module_url(base: &str, path: &str) -> Result<String, String> {
    let rest = base.strip_prefix("https://").ok_or("the download address must start with https://")?;
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() || host.contains('@') || base.contains(char::is_whitespace) || base.contains(['?', '#', '\\']) || base.chars().any(char::is_control) {
        return Err("the download address is not a plain https address (no user name, query or fragment)".into());
    }
    if !safe_path(path) {
        return Err(format!("{path} is not a path inside the module folder"));
    }
    Ok(format!("{}/{path}", base.trim_end_matches('/')))
}

/// Runs `depmod` for the release (the real one calls the computer's own tool).
pub trait Depmod {
    fn run(&self, root: &Path, release: &str) -> Result<(), String>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    pub written: Vec<String>,
}

/// Put the wanted modules into `root` (the `lib/modules/RELEASE` folder). Everything is fetched and checked before the first
/// file is written, so a bad file leaves the tree untouched.
pub fn install(root: &Path, manifest: &Manifest, wanted: &[String], fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>, depmod: &dyn Depmod) -> Result<Installed, String> {
    let mut got: Vec<(&String, Vec<u8>)> = Vec::new();
    for p in wanted {
        let item = manifest.modules.get(p).ok_or_else(|| format!("{p} is not in the optional pack"))?;
        if !safe_path(p) {
            return Err(format!("{p} is not a path inside the module folder"));
        }
        let b = fetch(p).map_err(|e| format!("{p}: {e}"))?;
        if b.len() as u64 != item.size {
            return Err(format!("{p}: {} bytes arrived, the manifest says {}; nothing was written", b.len(), item.size));
        }
        let h: String = Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect();
        if h != item.sha256 {
            return Err(format!("{p}: the checksum does not match the manifest; nothing was written"));
        }
        got.push((p, b));
    }
    let mut written = Vec::new();
    for (p, b) in got {
        let dst = root.join(p);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = dst.with_extension("ko.part");
        std::fs::write(&tmp, &b).map_err(|e| format!("{p}: {e}"))?;
        std::fs::rename(&tmp, &dst).map_err(|e| format!("{p}: {e}"))?;
        written.push(p.clone());
    }
    if !written.is_empty() {
        depmod.run(root, &manifest.release).map_err(|e| format!("the modules were written but depmod failed: {e}"))?;
    }
    Ok(Installed { written })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_must_be_plain_https_and_the_list_id_follows_the_list() {
        assert_eq!(module_url("https://packs.example.org/iyakki/", "kernel/a.ko").unwrap(), "https://packs.example.org/iyakki/kernel/a.ko");
        for bad in ["http://x.org", "ftp://x.org", "https://u@x.org", "https://x.org/?a=1", "https://x.org/#f", "https://", "x.org", "https://x.org/a b"] {
            assert!(module_url(bad, "kernel/a.ko").is_err(), "{bad}");
        }
        assert!(module_url("https://x.org", "../a.ko").is_err() && module_url("https://x.org", "/a.ko").is_err());
        let (m, ..) = manifest();
        let all: Vec<String> = m.modules.keys().cloned().collect();
        assert_ne!(list_id(&m, &all), list_id(&m, &all[..1]));
        assert_eq!(list_id(&m, &all), list_id(&m, &all));
    }

    #[test]
    fn a_tree_without_a_pack_needs_nothing_and_one_with_a_pack_names_what_the_devices_call_for() {
        let d = std::env::temp_dir().join(format!("iyakki-modpack-here-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("optional-pack")).unwrap();
        assert_eq!(needed_here(&d, &["usb:v1234p5678d0001".into()]).unwrap(), Vec::<String>::new());
        let (m, ..) = manifest();
        let t = format!("release 6.1.0\n{}", m.modules.iter().map(|(p, i)| format!("module {} {} {p}\n", i.size, i.sha256)).collect::<String>());
        std::fs::write(d.join("optional-pack/manifest.txt"), t).unwrap();
        std::fs::write(d.join("optional-pack/optional.map"), "alias usb:v1234p5678* kernel/drivers/media/dvb-usb/dvbu.ko\n").unwrap();
        assert_eq!(needed_here(&d, &["usb:v1234p5678d0001".into()]).unwrap(), ["kernel/drivers/media/dvb-usb/dvbu.ko"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    use std::cell::RefCell;

    struct D(RefCell<u32>);
    impl Depmod for D {
        fn run(&self, _: &Path, _: &str) -> Result<(), String> {
            *self.0.borrow_mut() += 1;
            Ok(())
        }
    }

    fn manifest() -> (Manifest, Vec<u8>, Vec<u8>) {
        let (a, b) = (b"AAAA".to_vec(), b"BBBBBB".to_vec());
        let hx = |x: &[u8]| Sha256::digest(x).iter().map(|c| format!("{c:02x}")).collect::<String>();
        let t = format!("release 6.1.0\nmodule {} {} kernel/drivers/media/dvb-usb/dvbu.ko\nmodule {} {} kernel/drivers/media/dvb-core/dvb.ko\n", a.len(), hx(&a), b.len(), hx(&b));
        (Manifest::parse(&t).unwrap(), a, b)
    }

    #[test]
    fn the_manifest_is_read_and_bad_lines_and_paths_are_refused() {
        let (m, ..) = manifest();
        assert_eq!(m.release, "6.1.0");
        assert_eq!(m.modules.len(), 2);
        assert!(Manifest::parse("release 1\nmodule 1 abc kernel/a.ko\n").is_err());
        let h = "0".repeat(64);
        assert!(Manifest::parse(&format!("release 1\nmodule 1 {h} ../etc/x.ko\n")).is_err());
        assert!(Manifest::parse(&format!("release 1\nmodule 1 {h} /abs/x.ko\n")).is_err());
        assert!(Manifest::parse(&format!("module 1 {h} kernel/a.ko\n")).is_err());
    }

    #[test]
    fn the_map_is_made_from_the_aliases_and_deps_and_read_back_and_globs_match() {
        let dvbu = "kernel/drivers/media/dvb-usb/dvbu.ko".to_string();
        let dvb = "kernel/drivers/media/dvb-core/dvb.ko".to_string();
        let opt: BTreeSet<String> = [dvbu.clone(), dvb.clone()].into();
        let dep = crate::modsplit::parse_dep(&format!("{dvbu}: {dvb} kernel/lib/x.ko\n{dvb}:\n"));
        let text = map_text(&opt, &dep, "alias usb:v1234p5678* dvbu\nalias pci:v1* ext4\nalias usb:vAAAA* dvb\n");
        assert!(text.contains(&format!("alias usb:v1234p5678* {dvbu}")) && !text.contains("ext4"), "{text}");
        assert!(text.contains(&format!("dep {dvbu} {dvb}")) && !text.contains("lib/x.ko"));
        let m = Map::parse(&text);
        assert_eq!(m.aliases.len(), 2);
        assert!(glob("usb:v1234p5678*", "usb:v1234p5678d0001") && !glob("usb:v1234p5678*", "usb:v1234p9999") && glob("a?c", "abc") && glob("*", ""));
    }

    #[test]
    fn hardware_asks_for_its_module_and_the_optional_ones_that_module_needs_and_not_what_is_there() {
        let (m, ..) = manifest();
        let map = Map::parse("alias usb:v1234p5678* kernel/drivers/media/dvb-usb/dvbu.ko\ndep kernel/drivers/media/dvb-usb/dvbu.ko kernel/drivers/media/dvb-core/dvb.ko\n");
        let n = needed(&m, &map, &["usb:v1234p5678d0001".into()], &|_| false);
        assert_eq!(n, ["kernel/drivers/media/dvb-core/dvb.ko", "kernel/drivers/media/dvb-usb/dvbu.ko"]);
        assert!(needed(&m, &map, &["usb:v0000p0000".into()], &|_| false).is_empty());
        assert_eq!(needed(&m, &map, &["usb:v1234p5678".into()], &|p| p.ends_with("dvb.ko")), ["kernel/drivers/media/dvb-usb/dvbu.ko"]);
    }

    #[test]
    fn modules_are_written_whole_after_every_check_and_a_bad_file_leaves_the_tree_alone() {
        let (m, a, b) = manifest();
        let d = std::env::temp_dir().join(format!("iyakki-modpack-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let wanted: Vec<String> = m.modules.keys().cloned().collect();
        let dm = D(RefCell::new(0));
        let (a2, b2) = (a.clone(), b.clone());
        let good = move |p: &str| Ok(if p.ends_with("dvbu.ko") { a2.clone() } else { b2.clone() });
        // the second file is damaged: nothing is written, depmod is not run
        let bad = |p: &str| Ok(if p.ends_with("dvbu.ko") { a.clone() } else { b"XXXXXX".to_vec() });
        assert!(install(&d, &m, &wanted, &bad, &dm).unwrap_err().contains("checksum"));
        assert!(!d.join("kernel").exists() && *dm.0.borrow() == 0);
        let short = |_: &str| Ok(vec![1u8]);
        assert!(install(&d, &m, &wanted, &short, &dm).unwrap_err().contains("manifest says"));
        let r = install(&d, &m, &wanted, &good, &dm).unwrap();
        assert_eq!(r.written.len(), 2);
        assert_eq!(std::fs::read(d.join("kernel/drivers/media/dvb-core/dvb.ko")).unwrap(), b);
        assert_eq!(*dm.0.borrow(), 1);
        assert!(install(&d, &m, &["kernel/other.ko".into()], &good, &dm).unwrap_err().contains("not in the optional pack"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
