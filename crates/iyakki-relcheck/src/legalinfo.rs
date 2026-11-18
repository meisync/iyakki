//! Completeness of a Buildroot `legal-info/` folder for the copyleft parts: every package in `manifest.csv` has its licence texts
//! (`licenses/<pkg>-<ver>/`), every GPL / LGPL / MPL package has its source (`sources/<pkg>-<ver>/`: the archive and the patches that
//! were applied), and every package that owns files in the image appears in the manifest at all.

use std::collections::BTreeMap;
use std::path::Path;

use crate::sbom::{self, Pkg};
use crate::spdx::{self, Class};

#[derive(Debug, Default)]
pub struct Report {
    pub dir: String,
    pub packages: usize,
    pub copyleft: usize,
    pub failures: Vec<String>,
}

fn has_files(d: &Path) -> bool {
    std::fs::read_dir(d).map(|mut r| r.next().is_some()).unwrap_or(false)
}

pub fn check(dir: &Path, file_list_owners: &BTreeMap<String, String>) -> Report {
    let mut r = Report { dir: dir.display().to_string(), ..Default::default() };
    let Ok(csv) = std::fs::read_to_string(dir.join("manifest.csv")) else {
        r.failures.push(format!("{}: no manifest.csv (run `make legal-info` in that Buildroot build)", dir.display()));
        return r;
    };
    let pkgs: Vec<Pkg> = sbom::parse_buildroot_manifest(&csv);
    r.packages = pkgs.len();
    for p in &pkgs {
        let nv = format!("{}-{}", p.name, p.version);
        let (class, _) = spdx::classify_list(&p.license);
        if !has_files(&dir.join("licenses").join(&nv)) {
            r.failures.push(format!("{nv}: no licence texts in licenses/{nv}"));
        }
        if class == Class::Copyleft {
            r.copyleft += 1;
            if !has_files(&dir.join("sources").join(&nv)) {
                r.failures.push(format!("{nv}: copyleft ({}) but no source in sources/{nv} -- the source offer cannot be met", p.license));
            }
        }
    }
    let known: std::collections::BTreeSet<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
    let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
    for pkg in file_list_owners.values() {
        if !known.contains(pkg.as_str()) {
            *missing.entry(pkg.as_str()).or_default() += 1;
        }
    }
    if !missing.is_empty() {
        let names: Vec<_> = missing.keys().take(40).copied().collect();
        r.failures.push(format!("{} package(s) that own files in the build are missing from manifest.csv, e.g. {}{}", missing.len(), names.join(", "), if missing.len() > 40 { ", ..." } else { "" }));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sources_texts_and_manifest_rows_fail() {
        let d = std::env::temp_dir().join(format!("iyakki-relcheck-legalinfo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["licenses/busybox-1.37.0", "licenses/zlib-1.3", "sources/zlib-1.3", "licenses/linux-6.12"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
            std::fs::write(d.join(p).join("COPYING"), b"x").unwrap();
        }
        std::fs::create_dir_all(d.join("sources/busybox-1.37.0")).unwrap();
        std::fs::write(d.join("manifest.csv"), "\"PACKAGE\",\"VERSION\",\"LICENSE\",\"LICENSE FILES\",\"SOURCE ARCHIVE\",\"SOURCE SITE\",\"DEPS\"\n\"busybox\",\"1.37.0\",\"GPL-2.0\",\"LICENSE\",\"b.tar\",\"http://x\",\"\"\n\"zlib\",\"1.3\",\"Zlib\",\"L\",\"z.tar\",\"http://x\",\"\"\n\"linux\",\"6.12\",\"GPL-2.0\",\"C\",\"l.tar\",\"http://x\",\"\"\n").unwrap();
        let mut owners = BTreeMap::new();
        owners.insert("bin/x".to_string(), "busybox".to_string());
        owners.insert("lib/y".to_string(), "wpewebkit".to_string());
        let r = check(&d, &owners);
        assert_eq!(r.packages, 3);
        assert!(r.failures.iter().any(|f| f.contains("busybox-1.37.0") && f.contains("no source")), "{:?}", r.failures);
        assert!(r.failures.iter().any(|f| f.contains("linux-6.12") && f.contains("no source")));
        assert!(r.failures.iter().any(|f| f.contains("wpewebkit")));
        assert_eq!(r.failures.len(), 3);
        assert_eq!(check(&d.join("nope"), &owners).failures.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
