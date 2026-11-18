//! The services a person has deployed or connected: `installed-services-<user>.json` next to their other settings. It holds names,
//! cluster references, URLs and vault entry NAMES, never a value.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform_types::{Options, UrlInfo};
use crate::devcluster::Source;
pub use crate::platform_types::{ClusterRef, WsLink, Installed};







fn path(dir: &Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    dir.join(format!("installed-services-{safe}.json"))
}

pub fn load(dir: &Path, user: &str) -> Vec<Installed> {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(dir: &Path, user: &str, v: &[Installed]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(path(dir, user), serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn find(dir: &Path, user: &str, id: &str) -> Option<Installed> {
    load(dir, user).into_iter().find(|i| i.id == id)
}

/// Add or replace the record with the same id.
pub fn upsert(dir: &Path, user: &str, rec: Installed) -> Result<(), String> {
    let mut v = load(dir, user);
    v.retain(|i| i.id != rec.id);
    v.push(rec);
    save(dir, user, &v)
}

pub fn remove(dir: &Path, user: &str, id: &str) -> Result<(), String> {
    let mut v = load(dir, user);
    v.retain(|i| i.id != id);
    save(dir, user, &v)
}

/// A unique instance id for a service in a namespace of a cluster: `gitea-iyakki-gitea`, with a short cluster tag when the same
/// pair already exists on another cluster.
pub fn instance_id(service: &str, ns: &str, cluster_key: &str, existing: &[Installed]) -> String {
    let base = format!("{service}-{ns}");
    let key = |i: &Installed| i.cluster.as_ref().map(|c| format!("{}#{}", c.kubeconfig, c.context)).unwrap_or_default();
    if existing.iter().all(|i| i.id != base || key(i) == cluster_key) {
        return base.chars().take(40).collect();
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in cluster_key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{}-{:04x}", base.chars().take(35).collect::<String>(), h & 0xffff)
}
