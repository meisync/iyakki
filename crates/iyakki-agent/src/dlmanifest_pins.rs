//! Our own SHA-256 values in the downloads manifest (`checksum.type = "sha256"`), used where the vendor publishes no checksum, or only a weak one
//! (ffmpeg's MD5): a pinned file is compared with the value at download time, and Python packages are installed with `pip --require-hashes`
//! from a requirements file made out of the values. Nothing here invents a value: with no `sha256` entry for the item and version, every
//! function answers `None` and the download behaves as before. Filling the values is the owner's step (`./run.sh dlmanifest pin ...`,
//! docs/development/DOWNLOADS-MANIFEST.md section 10).

use std::path::Path;

use super::{Checksum, FileSum, Store};

fn entry(s: &Store, id: &str, version: &str) -> Option<Vec<FileSum>> {
    let it = s.active().items.into_iter().find(|i| i.id == format!("cli:{id}"))?;
    if it.version.is_empty() || it.version != version {
        return None;
    }
    match it.checksum {
        Some(Checksum::Sha256 { files, .. }) if !files.is_empty() => Some(files),
        _ => None,
    }
}

/// The pinned SHA-256 of `file` for tool `id` at exactly `version`.
pub fn sha256_for(dir: &Path, id: &str, version: &str, file: &str) -> Option<String> {
    sha256_with(&Store::new(dir), id, version, file)
}

pub fn sha256_with(s: &Store, id: &str, version: &str, file: &str) -> Option<String> {
    entry(s, id, version)?.into_iter().find(|f| f.file == file).map(|f| f.sha256)
}

/// (normalised project name, version) from a wheel (`name-1.2-py3-none-any.whl`) or source archive (`name-1.2.tar.gz`) file name.
pub fn project_of(file: &str) -> Option<(String, String)> {
    let (name, ver) = if let Some(w) = file.strip_suffix(".whl") {
        let mut p = w.split('-');
        (p.next()?, p.next()?)
    } else {
        let stem = file.strip_suffix(".tar.gz").or_else(|| file.strip_suffix(".zip"))?;
        stem.rsplit_once('-')?
    };
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '!'));
    (ok(name) && ok(ver)).then(|| (name.to_ascii_lowercase().replace(['_', '.'], "-"), ver.to_string()))
}

/// The content of a `pip install --require-hashes -r` file for tool `id` at `version`, or `None` when the manifest has no hashes for it.
/// The tool's own package must be in the list at that version; every dependency has to be listed too (pip refuses the install otherwise,
/// which is the point: nothing unpinned gets in).
pub fn pip_lock(dir: &Path, id: &str, package: &str, version: &str) -> Option<String> {
    pip_lock_with(&Store::new(dir), id, package, version)
}

pub fn pip_lock_with(s: &Store, id: &str, package: &str, version: &str) -> Option<String> {
    let files = entry(s, id, version)?;
    let mut groups: Vec<((String, String), Vec<String>)> = Vec::new();
    for f in &files {
        let key = project_of(&f.file)?;
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push(f.sha256.clone()),
            None => groups.push((key, vec![f.sha256.clone()])),
        }
    }
    let own = package.to_ascii_lowercase().replace(['_', '.'], "-");
    if !groups.iter().any(|g| g.0 .0 == own && g.0 .1 == version) {
        return None;
    }
    let mut out = String::new();
    for ((name, ver), hashes) in groups {
        out.push_str(&format!("{name}=={ver}"));
        for h in hashes {
            out.push_str(&format!(" \\\n    --hash=sha256:{h}"));
        }
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dlmanifest::{bundled, Item, Kind, Manifest};

    fn store_with(id: &str, version: &str, files: &[(&str, &str)]) -> Store {
        let mut b: Manifest = bundled();
        b.items.retain(|i| i.id != format!("cli:{id}"));
        b.items.push(Item {
            id: format!("cli:{id}"),
            covers: vec![],
            licence: "MIT".into(),
            proprietary: false,
            downloaded_not_shipped: true,
            kind: Kind::NoneVendor,
            version: version.into(),
            url: String::new(),
            hosts: vec!["pypi.org".into()],
            checksum: Some(Checksum::Sha256 { files: files.iter().map(|(f, h)| FileSum { file: (*f).into(), sha256: (*h).into() }).collect(), date: "2026-01-01".into(), source: "test".into() }),
        });
        let d = std::env::temp_dir().join(format!("iyakki-pins-{}-{id}", std::process::id()));
        Store::with(&d, None, b)
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn file_names_give_project_and_version() {
        assert_eq!(project_of("duckdb-1.3.2-cp312-cp312-musllinux_1_2_x86_64.whl"), Some(("duckdb".into(), "1.3.2".into())));
        assert_eq!(project_of("Py_Serial-3.5-py2.py3-none-any.whl"), Some(("py-serial".into(), "3.5".into())));
        assert_eq!(project_of("reedsolo-1.7.0.tar.gz"), Some(("reedsolo".into(), "1.7.0".into())));
        assert_eq!(project_of("notes.txt"), None);
    }

    #[test]
    fn a_pin_applies_only_to_its_exact_version_and_file() {
        let s = store_with("ffmpeg", "7.0.2", &[("ffmpeg-7.0.2-amd64-static.tar.xz", A)]);
        assert_eq!(sha256_with(&s, "ffmpeg", "7.0.2", "ffmpeg-7.0.2-amd64-static.tar.xz").as_deref(), Some(A));
        assert_eq!(sha256_with(&s, "ffmpeg", "7.1", "ffmpeg-7.0.2-amd64-static.tar.xz"), None);
        assert_eq!(sha256_with(&s, "ffmpeg", "7.0.2", "other.tar.xz"), None);
        assert_eq!(sha256_with(&s, "kubectl", "7.0.2", "x"), None);
    }

    #[test]
    fn the_pip_lock_lists_every_file_hash_and_needs_the_tools_own_package() {
        let s = store_with("esptool", "5.6.0", &[("esptool-5.6.0-py3-none-any.whl", A), ("bitstring-4.2.3-py3-none-any.whl", B), ("bitstring-4.2.3.tar.gz", A)]);
        let lock = pip_lock_with(&s, "esptool", "esptool", "5.6.0").unwrap();
        assert!(lock.contains("esptool==5.6.0 \\\n    --hash=sha256:aaaa"), "{lock}");
        assert!(lock.contains("bitstring==4.2.3 \\\n    --hash=sha256:bbbb"), "{lock}");
        assert_eq!(lock.matches("bitstring==").count(), 1, "one line per project, two hashes");
        let only_dep = store_with("esptool", "5.6.0", &[("bitstring-4.2.3-py3-none-any.whl", B)]);
        assert_eq!(pip_lock_with(&only_dep, "esptool", "esptool", "5.6.0"), None);
        assert_eq!(pip_lock_with(&s, "esptool", "esptool", "5.5.0"), None);
    }

    #[test]
    fn the_bundled_manifest_has_no_invented_values() {
        // until the owner fills them in, nothing is pinned and every download behaves as before
        for id in ["ffmpeg", "duckdb", "esptool"] {
            let it = bundled().items.into_iter().find(|i| i.id == format!("cli:{id}")).unwrap();
            if let Some(Checksum::Sha256 { files, date, source }) = it.checksum {
                assert!(!date.is_empty() && !source.is_empty() && !files.is_empty());
            }
        }
    }
}
