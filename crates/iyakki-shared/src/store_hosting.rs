// The official root key and the hosted official catalog, the parts that need no secret (F-APPSTORE-14).
//
// Three checks the project runs before it publishes, none of which ever sees a private key:
// * [`roots_line`] turns a root's PUBLIC key and window into the line for `official-roots.txt` (refusing anything that
//   is not a valid public key or window), and [`roots_problems`] audits the whole file: unreadable lines, the same key
//   twice, windows that are empty or longer than ten years, a build with no root valid now.
// * [`check_hosting`] audits the folder that will be uploaded (`catalog.json` and the packages next to it) against the
//   pinned roots exactly as a device will: the catalog is signed by a pinned root (not merely by someone), its number is
//   higher than the one published before, it is not about to expire, and every package it names is there with the
//   size and SHA-256 it lists.
// * The signing itself (`Catalog::sign`, `iyakki pkg catalog-add`) is done on the offline machine that holds the key.
//
// Nothing here generates or stores the official key: the owner makes it offline (docs/STORE.md section 10), and until
// its public half is in `official-roots.txt` the device says so honestly.

use std::path::Path;

use crate::store_catalog::{sha256_hex, Catalog, CatalogSigner};
use crate::store_trust::Roots;
use crate::pkg::unhex;

const TEN_YEARS: i64 = 10 * 365 * 86_400;
const WARN_EXPIRY: i64 = 14 * 86_400;

/// One line for `official-roots.txt` from a root's public key (64 hex characters).
pub fn roots_line(id: &str, public_key: &str, valid_from: i64, valid_until: i64) -> Result<String, String> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("the root's id may use letters, digits, - and _ (up to 40)".into());
    }
    let b = unhex::<32>(public_key).ok_or("the public key must be 64 hexadecimal characters")?;
    ed25519_dalek::VerifyingKey::from_bytes(&b).map_err(|_| "that is not a valid ed25519 public key".to_string())?;
    if valid_from < 0 || valid_until <= valid_from {
        return Err("the window must end after it starts".into());
    }
    if valid_until - valid_from > TEN_YEARS {
        return Err("a root's window may be at most ten years: rotate instead".into());
    }
    Ok(format!("{id} {} {valid_from} {valid_until}", public_key.to_ascii_lowercase()))
}

/// Everything wrong with the text of an `official-roots.txt` (empty list when it is sound). An EMPTY file is not an error
/// here: it is the honest state before the key exists. `now` is used for the "a root is valid now" note.
pub fn roots_problems(text: &str, now: i64) -> Vec<String> {
    let mut out = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut any = false;
    let mut valid_now = false;
    for (n, l) in text.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        any = true;
        let f: Vec<&str> = l.split_whitespace().collect();
        let (Some(id), Some(key), Some(from), Some(until)) = (f.first(), f.get(1), f.get(2).and_then(|x| x.parse::<i64>().ok()), f.get(3).and_then(|x| x.parse::<i64>().ok())) else {
            out.push(format!("line {}: needs id, public key, valid from, valid until", n + 1));
            continue;
        };
        if f.len() != 4 {
            out.push(format!("line {}: extra words after the window", n + 1));
        }
        if let Err(e) = roots_line(id, key, from, until) {
            out.push(format!("line {}: {e}", n + 1));
            continue;
        }
        if !keys.insert(key.to_ascii_lowercase()) {
            out.push(format!("line {}: the same key is listed twice", n + 1));
        }
        if !ids.insert(id.to_string()) {
            out.push(format!("line {}: the id {id} is used twice", n + 1));
        }
        valid_now |= from <= now && now <= until;
    }
    if any && !valid_now {
        out.push("no listed root is valid now: devices built from this file would show nothing as Official".into());
    }
    out
}

/// What [`check_hosting`] found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostReport {
    pub catalog_id: String,
    pub serial: u64,
    pub entries: usize,
    pub packages_checked: usize,
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

impl HostReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Audit the folder that will be uploaded as the official catalog. `previous_serial` is the number of the catalog
/// published last (0 for the first one).
pub fn check_hosting(dir: &Path, roots: &Roots, previous_serial: u64, now: i64) -> HostReport {
    let mut r = HostReport::default();
    let text = match std::fs::read_to_string(dir.join("catalog.json")) {
        Ok(t) => t,
        Err(_) => {
            r.problems.push("there is no catalog.json in the folder".into());
            return r;
        }
    };
    if roots.is_empty() {
        r.problems.push("no root key is pinned: nothing can be checked as Official yet".into());
    }
    let (c, check): (Catalog, _) = match Catalog::parse(&text, roots, &[], now, true) {
        Ok(x) => x,
        Err(e) => {
            r.problems.push(e.to_string());
            return r;
        }
    };
    r.catalog_id = c.id.clone();
    r.serial = c.serial;
    r.entries = c.entries.len();
    match &check.signer {
        CatalogSigner::Official(_) => {}
        other => r.problems.push(format!("the catalog must be signed by a pinned root key, but it is {}", other.label())),
    }
    r.warnings.extend(check.warnings.iter().cloned());
    if check.expired {
        r.problems.push("the catalog has already expired".into());
    } else if c.expires != 0 && c.expires - now < WARN_EXPIRY {
        r.warnings.push("the catalog expires within 14 days: sign a new one soon".into());
    }
    if c.serial <= previous_serial {
        r.problems.push(format!("the catalog number {} must be higher than the published one ({previous_serial}): devices refuse a lower or equal number", c.serial));
    }
    // Entries dropped by the parser (bad names, a package without hash) are a publishing mistake, not something to hide.
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let listed = raw.get("entries").and_then(|e| e.as_array()).map(|a| a.len()).unwrap_or(0);
    if listed != c.entries.len() {
        r.problems.push(format!("{} of {} entries are not valid and devices would ignore them", listed - c.entries.len(), listed));
    }
    for e in &c.entries {
        if e.url.is_empty() {
            continue;
        }
        if e.url.starts_with("http://") || e.url.starts_with("https://") {
            r.warnings.push(format!("{}: the package is hosted elsewhere ({}): not checked here", e.id, e.url.split('/').nth(2).unwrap_or("")));
            continue;
        }
        match std::fs::read(dir.join(&e.url)) {
            Err(_) => r.problems.push(format!("{}: the package file {} is missing", e.id, e.url)),
            Ok(b) => {
                r.packages_checked += 1;
                if b.len() as u64 != e.size {
                    r.problems.push(format!("{}: the file is {} bytes but the catalog says {}", e.id, b.len(), e.size));
                }
                if sha256_hex(&b) != e.sha256 {
                    r.problems.push(format!("{}: the file's SHA-256 differs from the catalog's", e.id));
                }
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use crate::store_catalog::tests::{catalog, entry};
    use super::*;
    use crate::pkg::{new_signing_key, public_key_hex};

    const NOW: i64 = 1_800_000_000;
    const YEAR: i64 = 365 * 86_400;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-host-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn publish(d: &Path, key: &ed25519_dalek::SigningKey, serial: u64, expires: i64, pkg: &[u8], listed: &[u8]) {
        std::fs::write(d.join("alpha.iyp"), pkg).unwrap();
        let mut c = catalog("official", serial, vec![entry("alpha", listed, "alpha.iyp")]);
        c.expires = expires;
        c.sign(key);
        std::fs::write(d.join("catalog.json"), c.to_json()).unwrap();
    }

    #[test]
    fn the_shipped_file_is_sound_and_still_empty_until_the_owner_makes_the_key() {
        let text = include_str!("../../iyakki-agent/src/store/official-roots.txt");
        assert!(roots_problems(text, NOW).is_empty());
        assert!(Roots::parse(text).is_empty(), "the real official key is never generated by the code or the tests");
    }

    #[test]
    fn a_generated_test_root_makes_a_line_a_device_accepts_as_official() {
        let root = new_signing_key().unwrap();
        let line = roots_line("root-test", &public_key_hex(&root), NOW - YEAR, NOW + 5 * YEAR).unwrap();
        assert!(roots_problems(&line, NOW).is_empty());
        let roots = Roots::parse(&line);
        assert!(roots.valid_at(&public_key_hex(&root), NOW).is_some());
        assert!(roots.valid_at(&public_key_hex(&root), NOW + 6 * YEAR).is_none());
        // and the catalog a device reads from that folder says Official
        let d = dir("ok");
        publish(&d, &root, 3, NOW + YEAR, b"pkg", b"pkg");
        let rep = check_hosting(&d, &roots, 2, NOW);
        assert!(rep.ok(), "{rep:?}");
        assert_eq!((rep.serial, rep.entries, rep.packages_checked), (3, 1, 1));
        let l = crate::store_catalog::read_folder("official", &d, &roots, &[], NOW).unwrap();
        assert!(matches!(l.check.signer, CatalogSigner::Official(ref id) if id == "root-test"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn bad_roots_lines_are_named() {
        let k = public_key_hex(&new_signing_key().unwrap());
        assert!(roots_line("r", "zz", 0, 10).is_err());
        assert!(roots_line("r", &"0".repeat(63), 0, 10).is_err());
        assert!(roots_line("bad id", &k, 0, 10).is_err());
        assert!(roots_line("r", &k, 10, 10).is_err());
        assert!(roots_line("r", &k, 0, 11 * YEAR).is_err());
        let two = format!("{}\n{}\n", roots_line("a", &k, 1, 100).unwrap(), roots_line("a", &k, 1, 100).unwrap());
        let p = roots_problems(&two, 50);
        assert!(p.iter().any(|x| x.contains("twice")), "{p:?}");
        assert!(roots_problems("r nothex 1 2", 1).iter().any(|x| x.contains("line 1")));
        assert!(roots_problems("r", 1)[0].contains("needs id"));
        let expired = roots_line("a", &k, 1, 100).unwrap();
        assert!(roots_problems(&expired, 500).iter().any(|x| x.contains("no listed root is valid now")));
    }

    #[test]
    fn hosting_refuses_every_way_the_folder_could_be_wrong() {
        let (root, other) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r", &root);
        let has = |rep: &HostReport, w: &str| rep.problems.iter().any(|p| p.contains(w));
        // signed by a key that is not pinned
        let d = dir("stranger");
        publish(&d, &other, 5, NOW + YEAR, b"pkg", b"pkg");
        assert!(has(&check_hosting(&d, &roots, 1, NOW), "pinned root"));
        // not higher than the published number
        publish(&d, &root, 5, NOW + YEAR, b"pkg", b"pkg");
        assert!(has(&check_hosting(&d, &roots, 5, NOW), "must be higher"));
        assert!(check_hosting(&d, &roots, 4, NOW).ok());
        // expired and about to expire
        publish(&d, &root, 6, NOW - 10, b"pkg", b"pkg");
        assert!(has(&check_hosting(&d, &roots, 1, NOW), "expired"));
        publish(&d, &root, 6, NOW + 86_400, b"pkg", b"pkg");
        let r = check_hosting(&d, &roots, 1, NOW);
        assert!(r.ok() && r.warnings.iter().any(|w| w.contains("14 days")));
        // package differs from the listing
        publish(&d, &root, 7, NOW + YEAR, b"changed!", b"pkg");
        let r = check_hosting(&d, &roots, 1, NOW);
        assert!(has(&r, "SHA-256") && has(&r, "bytes"));
        // package missing
        std::fs::remove_file(d.join("alpha.iyp")).unwrap();
        assert!(has(&check_hosting(&d, &roots, 1, NOW), "missing"));
        // unsigned, tampered, no file, no roots
        std::fs::write(d.join("catalog.json"), catalog("official", 9, vec![]).to_json()).unwrap();
        assert!(!check_hosting(&d, &roots, 1, NOW).ok());
        publish(&d, &root, 8, NOW + YEAR, b"pkg", b"pkg");
        let t = std::fs::read_to_string(d.join("catalog.json")).unwrap().replace("\"serial\": 8", "\"serial\": 99");
        std::fs::write(d.join("catalog.json"), t).unwrap();
        assert!(has(&check_hosting(&d, &roots, 1, NOW), "bad signature"));
        assert!(has(&check_hosting(&dir("empty"), &roots, 0, NOW), "no catalog.json"));
        publish(&d, &root, 8, NOW + YEAR, b"pkg", b"pkg");
        assert!(has(&check_hosting(&d, &Roots::default(), 0, NOW), "no root key is pinned"));
        let _ = std::fs::remove_dir_all(d);
    }
}
