//! The revocation list: keys, publishers, packages and licences that were withdrawn. Signed by a root key, checked
//! offline, and only ever replaced by a list with a higher serial, so replaying an old list cannot bring anything back.

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::trust::{sign_doc, verify_doc, Roots};
use crate::pkg::{newer, public_key_hex};

pub const MAX_LIST_BYTES: usize = 512 * 1024;
/// A list issued longer ago than this is still enforced, but the store says it is old.
pub const FRESH_DAYS: i64 = 30;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RevEntry {
    pub id: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RevPackage {
    pub name: String,
    pub kind: String,
    pub sha256: String,
    /// Every version below this one is withdrawn.
    pub before: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RevList {
    pub v: u32,
    pub serial: u64,
    pub issued: i64,
    pub expires: i64,
    pub keys: Vec<RevEntry>,
    pub publishers: Vec<RevEntry>,
    pub packages: Vec<RevPackage>,
    pub licenses: Vec<RevEntry>,
    pub issuer: String,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevError {
    Malformed(String),
    UnknownIssuer,
    BadSignature,
    Rollback { have: u64, got: u64 },
}

impl std::fmt::Display for RevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevError::Malformed(m) => write!(f, "the revocation list cannot be read: {m}"),
            RevError::UnknownIssuer => write!(f, "the revocation list was not signed by an IYAKKI root key"),
            RevError::BadSignature => write!(f, "the revocation list has a bad signature"),
            RevError::Rollback { have, got } => write!(f, "this revocation list (number {got}) is older than the one already known (number {have})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// No list has ever been loaded.
    Missing,
    Fresh,
    /// Issued this many days ago.
    Old(i64),
}

/// The list in force.
#[derive(Clone, Debug, Default)]
pub struct Revocations {
    list: Option<RevList>,
}

impl Revocations {
    pub fn load(path: &Path) -> Revocations {
        let list = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<RevList>(&b).ok());
        Revocations { list }
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join("revocations.json")
    }

    pub fn serial(&self) -> u64 {
        self.list.as_ref().map_or(0, |l| l.serial)
    }

    pub fn current(&self) -> Option<&RevList> {
        self.list.as_ref()
    }

    pub fn freshness(&self, now: i64) -> Freshness {
        match &self.list {
            None => Freshness::Missing,
            Some(l) if now - l.issued > FRESH_DAYS * 86_400 => Freshness::Old((now - l.issued) / 86_400),
            Some(_) => Freshness::Fresh,
        }
    }

    /// Check a signed list and, if it is newer than the one in force, make it the one in force. The same serial is
    /// accepted (a mirror serving what we already have); a lower one is refused.
    pub fn accept(&mut self, text: &str, roots: &Roots, now: i64) -> Result<bool, RevError> {
        let l = Self::verify(text, roots, now)?;
        if l.serial < self.serial() {
            return Err(RevError::Rollback { have: self.serial(), got: l.serial });
        }
        let changed = l.serial > self.serial();
        self.list = Some(l);
        Ok(changed)
    }

    pub fn verify(text: &str, roots: &Roots, now: i64) -> Result<RevList, RevError> {
        if text.len() > MAX_LIST_BYTES {
            return Err(RevError::Malformed("too large".into()));
        }
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| RevError::Malformed(e.to_string()))?;
        let l: RevList = serde_json::from_value(v.clone()).map_err(|e| RevError::Malformed(e.to_string()))?;
        if l.v != 1 {
            return Err(RevError::Malformed("an unknown format version".into()));
        }
        if roots.valid_at(&l.issuer, now).is_none() {
            return Err(RevError::UnknownIssuer);
        }
        if !verify_doc("revocations", &v, &l.issuer, &l.sig) {
            return Err(RevError::BadSignature);
        }
        Ok(l)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        let Some(l) = &self.list else { return Ok(()) };
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&serde_json::to_vec_pretty(l).unwrap_or_default())).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn key_revoked(&self, key: &str) -> Option<String> {
        self.list.as_ref()?.keys.iter().find(|e| e.id == key).map(|e| e.reason.clone())
    }

    pub fn publisher_revoked(&self, id: &str) -> Option<String> {
        self.list.as_ref()?.publishers.iter().find(|e| e.id == id).map(|e| e.reason.clone())
    }

    pub fn license_revoked(&self, serial: &str) -> Option<String> {
        self.list.as_ref()?.licenses.iter().find(|e| e.id == serial).map(|e| e.reason.clone())
    }

    /// Why this exact package is withdrawn, if it is.
    pub fn package_revoked(&self, kind: &str, name: &str, version: &str, sha256: &str) -> Option<String> {
        self.list.as_ref()?.packages.iter().find_map(|p| {
            if p.name != name || (!p.kind.is_empty() && p.kind != kind) {
                return None;
            }
            let why = if p.reason.is_empty() { String::new() } else { format!(" ({})", p.reason) };
            if !p.sha256.is_empty() {
                return (p.sha256 == sha256).then(|| format!("this exact build of {name} was withdrawn{why}"));
            }
            if !p.before.is_empty() {
                return newer(&p.before, version).then(|| format!("versions of {name} before {} were withdrawn{why}", p.before));
            }
            Some(format!("{name} was withdrawn{why}"))
        })
    }

    /// A signed list for tests and for `iyakki pkg revoke-sign`: `packages` are `(name, before)` pairs.
    pub fn sign_for_tests(&self, root: &SigningKey, serial: u64, issued: i64, keys: Vec<String>, publishers: Vec<String>, packages: Vec<(String, Option<String>)>) -> String {
        sign_list(root, serial, issued, issued + 90 * 86_400, keys, publishers, packages, vec![])
    }
}

/// Make and sign a revocation list (JSON text).
#[allow(clippy::too_many_arguments)]
pub fn sign_list(root: &SigningKey, serial: u64, issued: i64, expires: i64, keys: Vec<String>, publishers: Vec<String>, packages: Vec<(String, Option<String>)>, licenses: Vec<String>) -> String {
    let entry = |id: String| RevEntry { id, reason: "revoked".into() };
    let mut l = RevList {
        v: 1,
        serial,
        issued,
        expires,
        keys: keys.into_iter().map(entry).collect(),
        publishers: publishers.into_iter().map(entry).collect(),
        packages: packages.into_iter().map(|(name, before)| RevPackage { name, before: before.unwrap_or_default(), reason: "revoked".into(), ..Default::default() }).collect(),
        licenses: licenses.into_iter().map(entry).collect(),
        issuer: public_key_hex(root),
        sig: String::new(),
    };
    l.sig = sign_doc("revocations", &serde_json::to_value(&l).unwrap_or_default(), root);
    serde_json::to_string_pretty(&l).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::new_signing_key;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_signed_list_is_accepted_and_only_a_root_can_sign_one() {
        let (root, other) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r", &root);
        let mut r = Revocations::default();
        assert_eq!(r.freshness(NOW), Freshness::Missing);
        let text = r.sign_for_tests(&root, 3, NOW, vec!["aa".repeat(32)], vec!["evil".into()], vec![("badpkg".into(), None)]);
        assert_eq!(r.accept(&text, &roots, NOW), Ok(true));
        assert_eq!(r.serial(), 3);
        assert!(r.key_revoked(&"aa".repeat(32)).is_some() && r.key_revoked("bb").is_none());
        assert!(r.publisher_revoked("evil").is_some());
        assert!(r.package_revoked("skill", "badpkg", "9.9.9", "x").unwrap().contains("withdrawn"));
        assert_eq!(r.freshness(NOW + 10 * 86_400), Freshness::Fresh);
        assert_eq!(r.freshness(NOW + 45 * 86_400), Freshness::Old(45));
        // signed by someone else
        let forged = r.sign_for_tests(&other, 9, NOW, vec![], vec![], vec![]);
        assert_eq!(Revocations::verify(&forged, &roots, NOW), Err(RevError::UnknownIssuer));
        // changed after signing
        let tampered = text.replace("evil", "good");
        assert_eq!(Revocations::verify(&tampered, &roots, NOW), Err(RevError::BadSignature));
        assert!(matches!(Revocations::verify("{", &roots, NOW), Err(RevError::Malformed(_))));
        assert!(matches!(Revocations::verify(&"x".repeat(MAX_LIST_BYTES + 1), &roots, NOW), Err(RevError::Malformed(_))));
    }

    #[test]
    fn an_older_list_cannot_replace_a_newer_one_and_the_same_one_changes_nothing() {
        let root = new_signing_key().unwrap();
        let roots = Roots::with("r", &root);
        let mut r = Revocations::default();
        let v5 = r.sign_for_tests(&root, 5, NOW, vec!["aa".repeat(32)], vec![], vec![]);
        let v4_empty = r.sign_for_tests(&root, 4, NOW, vec![], vec![], vec![]);
        r.accept(&v5, &roots, NOW).unwrap();
        assert_eq!(r.accept(&v4_empty, &roots, NOW), Err(RevError::Rollback { have: 5, got: 4 }));
        assert!(r.key_revoked(&"aa".repeat(32)).is_some(), "the replayed list brought nothing back");
        assert_eq!(r.accept(&v5, &roots, NOW), Ok(false));
        assert!(RevError::Rollback { have: 5, got: 4 }.to_string().contains("older"));
    }

    #[test]
    fn packages_are_withdrawn_by_exact_hash_by_version_floor_or_whole() {
        let root = new_signing_key().unwrap();
        let mut l = RevList { v: 1, serial: 1, issued: NOW, expires: NOW, ..Default::default() };
        l.packages = vec![
            RevPackage { name: "a".into(), sha256: "h1".into(), ..Default::default() },
            RevPackage { name: "b".into(), before: "2.0.0".into(), reason: "bug".into(), ..Default::default() },
            RevPackage { name: "c".into(), kind: "tool".into(), ..Default::default() },
        ];
        let r = Revocations { list: Some(l) };
        assert!(r.package_revoked("skill", "a", "1.0.0", "h1").is_some() && r.package_revoked("skill", "a", "1.0.0", "h2").is_none());
        assert!(r.package_revoked("skill", "b", "1.9.9", "").unwrap().contains("before 2.0.0 were withdrawn (bug)"));
        assert!(r.package_revoked("skill", "b", "2.0.0", "").is_none());
        assert!(r.package_revoked("tool", "c", "1.0.0", "").is_some() && r.package_revoked("skill", "c", "1.0.0", "").is_none());
        drop(root);
    }

    #[test]
    fn the_list_in_force_survives_a_restart() {
        let root = new_signing_key().unwrap();
        let roots = Roots::with("r", &root);
        let d = std::env::temp_dir().join(format!("iyakki-rev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = Revocations::path(&d);
        let mut r = Revocations::default();
        r.accept(&r.sign_for_tests(&root, 2, NOW, vec![], vec!["evil".into()], vec![]), &roots, NOW).unwrap();
        r.save(&p).unwrap();
        let again = Revocations::load(&p);
        assert_eq!(again.serial(), 2);
        assert!(again.publisher_revoked("evil").is_some());
        assert_eq!(Revocations::load(&d.join("none.json")).serial(), 0);
        let _ = std::fs::remove_dir_all(d);
    }
}
