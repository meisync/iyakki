//! Size limits for a cloud tenant's files. The tenant keeps ONE volume with a folder per namespace and person (`isolation: folders`, the
//! decision for the first version: it avoids a claim and a worker pod per namespace and keeps one set of checks: the path jail, the rights
//! and these limits). Because the folders share one disk, a person or a team must not be able to fill it for the others, so every write is
//! counted against two explicit limits from the tenant configuration item `storage.quota`:
//!
//! * `place_mb`: what ONE place may hold (a person's folder in a namespace, a namespace's shared folder, a person's personal folder);
//! * `tenant_mb`: what all places together may hold.
//!
//! A team may only lower them (the resolver keeps the smaller number). The size is counted by walking the folder without following links
//! (links are not counted and not entered); a folder with more than [`MAX_WALK`] entries is refused, never guessed (fail closed). A write
//! that would pass a limit is refused with 413 and nothing is written. Replacing a file counts only the difference.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// More entries than this in one count: refuse (the count must stay quick and cannot be trusted past it).
pub const MAX_WALK: u64 = 2_000_000;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageQuota {
    pub place_bytes: u64,
    pub tenant_bytes: u64,
}

impl Default for StorageQuota {
    fn default() -> StorageQuota {
        StorageQuota { place_bytes: 10 * 1024 * MIB, tenant_bytes: 100 * 1024 * MIB }
    }
}

impl StorageQuota {
    pub fn from_values(v: &BTreeMap<String, Value>) -> StorageQuota {
        let d = StorageQuota::default();
        let mb = |k: &str, dflt: u64| v.get(k).and_then(|x| x.as_u64()).map_or(dflt, |x| x.clamp(1, 100_000_000).saturating_mul(MIB));
        StorageQuota { place_bytes: mb("place_mb", d.place_bytes), tenant_bytes: mb("tenant_mb", d.tenant_bytes) }
    }

    /// The effective item of the tenant (or of a team), else the defaults. A configuration that cannot be read gives the defaults (which
    /// are limits too, never "no limit").
    pub fn load(data: &Path, team: Option<&str>) -> StorageQuota {
        use crate::tenantconfig::{resolve::resolve, schema, store::Store};
        let Some(spec) = schema::kind("storage.quota") else { return StorageQuota::default() };
        let Ok(cfg) = Store::new(data).load() else { return StorageQuota::default() };
        let entries = cfg.entries();
        let eff = resolve(spec, &cfg.layers(&entries, "storage.quota", team, "default"));
        if !eff.exists || !eff.enabled {
            return StorageQuota::default();
        }
        StorageQuota::from_values(&eff.values.into_iter().collect())
    }
}

/// Bytes held by regular files under `dir` (links are neither counted nor followed). A folder that is not there holds 0.
pub fn usage(dir: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    let mut seen = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("the size of the files could not be counted".into()),
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_WALK {
                return Err(format!("more than {MAX_WALK} files and folders: the size cannot be counted, so nothing more is accepted here"));
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                total = total.saturating_add(m.len());
            }
        }
    }
    Ok(total)
}

fn size_text(b: u64) -> String {
    if b >= 1024 * MIB { format!("{:.1} GiB", b as f64 / (1024.0 * MIB as f64)) } else { format!("{} MiB", b / MIB) }
}

/// May `incoming` more bytes be written into `place` (a place folder) inside `root` (all places)? `replaced` is the size of the file that the
/// write replaces (0 for a new file). Err: the refusal in words (the caller answers 413 and writes nothing).
pub fn check_write(q: &StorageQuota, root: &Path, place: &Path, incoming: u64, replaced: u64) -> Result<(), String> {
    let grow = incoming.saturating_sub(replaced);
    if grow == 0 {
        return Ok(());
    }
    let in_place = usage(place)?;
    if in_place.saturating_add(grow) > q.place_bytes {
        return Err(format!("this folder is larger than its limit would allow: it holds {} of {} (larger than the limit by {}); delete something first", size_text(in_place), size_text(q.place_bytes), size_text((in_place + grow).saturating_sub(q.place_bytes))));
    }
    let in_tenant = usage(root)?;
    if in_tenant.saturating_add(grow) > q.tenant_bytes {
        return Err(format!("the tenant's storage is full: it holds {} of {} (larger than the limit by {}); ask an administrator", size_text(in_tenant), size_text(q.tenant_bytes), size_text((in_tenant + grow).saturating_sub(q.tenant_bytes))));
    }
    Ok(())
}

/// After a clone (its size is not known before): is the place still within its limits? Err: the words; the caller removes the clone.
pub fn check_within(q: &StorageQuota, root: &Path, place: &Path) -> Result<(), String> {
    let p = usage(place)?;
    if p > q.place_bytes {
        return Err(format!("larger than the folder's limit ({} of {})", size_text(p), size_text(q.place_bytes)));
    }
    let t = usage(root)?;
    if t > q.tenant_bytes {
        return Err(format!("larger than the tenant's limit ({} of {})", size_text(t), size_text(q.tenant_bytes)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cloudquota-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn small() -> StorageQuota {
        StorageQuota { place_bytes: 100, tenant_bytes: 150 }
    }

    #[test]
    fn usage_counts_files_in_folders_and_ignores_links() {
        let d = dir("usage");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/one"), [0u8; 10]).unwrap();
        std::fs::write(d.join("a/b/two"), [0u8; 25]).unwrap();
        let outside = dir("usage-out");
        std::fs::write(outside.join("big"), [0u8; 9999]).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, d.join("a/link")).unwrap();
            std::os::unix::fs::symlink(outside.join("big"), d.join("a/flink")).unwrap();
        }
        assert_eq!(usage(&d).unwrap(), 35);
        assert_eq!(usage(&d.join("missing")).unwrap(), 0);
    }

    #[test]
    fn a_write_over_the_place_or_the_tenant_limit_is_refused_and_a_replacement_counts_the_difference() {
        let root = dir("limits");
        let (ann, bob) = (root.join("ns/web/people/ann"), root.join("ns/web/people/bob"));
        std::fs::create_dir_all(&ann).unwrap();
        std::fs::create_dir_all(&bob).unwrap();
        std::fs::write(ann.join("f"), [0u8; 60]).unwrap();
        let q = small();
        assert!(check_write(&q, &root, &ann, 40, 0).is_ok(), "exactly at the limit");
        let e = check_write(&q, &root, &ann, 41, 0).unwrap_err();
        assert!(e.contains("larger than") && e.contains("limit"), "{e}");
        assert!(check_write(&q, &root, &ann, 100, 60).is_ok(), "replacing a 60-byte file with 100 grows by 40");
        assert!(check_write(&q, &root, &ann, 50, 60).is_ok(), "a smaller file always fits");
        // bob has room in his own place but the tenant as a whole is full
        std::fs::write(bob.join("g"), [0u8; 80]).unwrap();
        let e = check_write(&q, &root, &bob, 11, 0).unwrap_err();
        assert!(e.contains("tenant"), "{e}");
        assert!(check_write(&q, &root, &bob, 10, 0).is_ok());
        // after a clone: within or not
        assert!(check_within(&q, &root, &ann).is_ok());
        std::fs::write(ann.join("h"), [0u8; 50]).unwrap();
        assert!(check_within(&q, &root, &ann).unwrap_err().contains("folder"));
    }

    #[test]
    fn the_limits_come_from_the_configuration_a_team_can_only_lower_them_and_a_bad_file_means_the_defaults() {
        let d = dir("cfg");
        assert_eq!(StorageQuota::load(&d, None), StorageQuota::default());
        let cfg = d.join("cloud/config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("tenant-config.json"), serde_json::json!({"items": [
            {"kind": "storage.quota", "name": "default", "enabled": true, "values": {"place_mb": 500, "tenant_mb": 4000}},
            {"kind": "storage.quota", "name": "default", "namespace": "web", "enabled": true, "values": {"place_mb": 900, "tenant_mb": 1000}}
        ]}).to_string()).unwrap();
        let q = StorageQuota::load(&d, None);
        assert_eq!((q.place_bytes, q.tenant_bytes), (500 * MIB, 4000 * MIB));
        let w = StorageQuota::load(&d, Some("web"));
        assert_eq!((w.place_bytes, w.tenant_bytes), (500 * MIB, 1000 * MIB), "a team may lower a limit, not raise it");
        std::fs::write(cfg.join("tenant-config.json"), "{ not json").unwrap();
        assert_eq!(StorageQuota::load(&d, None), StorageQuota::default());
        assert!(StorageQuota::default().place_bytes > 0, "the defaults are limits, never none");
    }
}
