//! The services a person has deployed or connected: `installed-services-<user>.json` next to their other settings. It holds names,
//! cluster references, URLs and vault entry NAMES, never a value.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Options, UrlInfo};
use crate::devcluster::Source;

/// How to reach the cluster a service lives in (a reference; it is re-checked, never trusted, before anything is run).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterRef {
    pub name: String,
    pub kubeconfig: String,
    pub context: String,
    pub server: String,
    pub source: Source,
}

impl ClusterRef {
    pub fn of(p: &crate::devcluster::ClusterProfile) -> ClusterRef {
        ClusterRef { name: p.name.clone(), kubeconfig: p.kubeconfig.clone(), context: p.context.clone(), server: p.server.clone(), source: p.source }
    }
    pub fn line(&self) -> String {
        format!("{} ({}{})", if self.name.is_empty() { "cluster" } else { &self.name }, if self.server.is_empty() { "server unknown" } else { &self.server }, if self.context.is_empty() { String::new() } else { format!(", context {}", self.context) })
    }
}

/// A workspace connected to a git service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsLink {
    pub workspace: String,
    pub repo_url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    /// The instance: `<service>-<namespace>` (made unique across clusters).
    pub id: String,
    pub service: String,
    pub title: String,
    pub version: String,
    pub kind: String,
    pub cluster: Option<ClusterRef>,
    pub namespace: String,
    /// IYAKKI created the namespace (so Remove with data may delete it).
    pub created_namespace: bool,
    /// An existing service the person connected: IYAKKI deployed nothing and removes nothing.
    pub connected: bool,
    pub options: Options,
    pub urls: Vec<UrlInfo>,
    /// Vault entry names (`svc.<instance>.<key>`); the values are in the person's vault.
    pub vault: Vec<String>,
    pub installed_at: i64,
    pub updated_at: i64,
    pub workspaces: Vec<WsLink>,
    pub note: String,
}

impl Default for Installed {
    fn default() -> Self {
        Installed { id: String::new(), service: String::new(), title: String::new(), version: String::new(), kind: String::new(), cluster: None, namespace: String::new(), created_namespace: false, connected: false, options: Options::default(), urls: vec![], vault: vec![], installed_at: 0, updated_at: 0, workspaces: vec![], note: String::new() }
    }
}

impl Installed {
    pub fn web_url(&self) -> Option<&str> {
        self.urls.iter().find(|u| u.kind == "web").map(|u| u.url.as_str())
    }
    pub fn token_vault(&self) -> Option<&str> {
        self.vault.iter().find(|v| v.ends_with(".token")).map(String::as_str)
    }
}

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
