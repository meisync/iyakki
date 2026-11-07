//! The files of a tenant's configuration, in `<data_dir>/cloud/config/`:
//!
//! * `tenant-config.json`: the operator's declarative document (same schema as the API's; e.g. a Kubernetes ConfigMap mounted
//!   read-only). Read on every use, never written here. An item with `"managed": true` cannot be changed through the API.
//! * `tenant-config.local.json`: what the API edited, `{schema, version, items}`. Written atomically.
//! * `versions/NNNNNN.json`: a snapshot of the local document per change, the last [`MAX_VERSIONS`] kept.
//! * `usage.json`: per-person counters for shared agents.
//!
//! Neither document can hold a secret value: the schema accepts only vault references (`schema::check_value`).

use super::resolve::{Layer, LayerKind};
use super::{short_hash, Doc, Item, Key, MAX_ITEMS, MAX_VERSIONS, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionInfo {
    pub version: u64,
    pub at: u64,
    pub by: String,
    pub hash: String,
    pub summary: String,
    pub items: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotFile {
    #[serde(flatten)]
    info: VersionInfo,
    doc: Doc,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub last_rotated: Option<u64>,
    /// (when, ok, HTTP status) of the last connection test. Never a body, never a key.
    #[serde(default)]
    pub test: Option<(u64, bool, u16)>,
}

pub fn meta_key(kind: &str, ns: Option<&str>, name: &str) -> String {
    format!("{kind}|{}|{name}", ns.unwrap_or("-"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    File,
    Api,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::File => "file",
            Source::Api => "api",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub item: Item,
    pub source: Source,
    /// Both documents have this key and the operator's wins.
    pub shadows_local: bool,
}

/// Both documents, read once. Everything a decision needs.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub file: Doc,
    pub local: Vec<Item>,
    pub version: u64,
    pub file_hash: String,
    /// The operator's file could not be read; its items count as absent (the safe reading), and this says why.
    pub file_error: Option<String>,
}

impl Config {
    pub fn entries(&self) -> Vec<Entry> {
        let mut keys: BTreeMap<Key, Entry> = BTreeMap::new();
        for it in &self.local {
            keys.insert(it.key(), Entry { item: it.clone(), source: Source::Api, shadows_local: false });
        }
        for it in &self.file.items {
            let k = it.key();
            match keys.get_mut(&k) {
                Some(e) if it.managed => *e = Entry { item: it.clone(), source: Source::File, shadows_local: true },
                Some(_) => {} // an unmanaged file item is a default: the API's item overrides it
                None => {
                    keys.insert(k, Entry { item: it.clone(), source: Source::File, shadows_local: false });
                }
            }
        }
        keys.into_values().collect()
    }

    pub fn find(&self, kind: &str, ns: Option<&str>, name: &str) -> Option<Entry> {
        self.entries().into_iter().find(|e| e.item.kind == kind && e.item.namespace.as_deref() == ns && e.item.name == name)
    }

    /// Is this key held by the operator's file as managed?
    pub fn is_managed(&self, kind: &str, ns: Option<&str>, name: &str) -> bool {
        self.file.items.iter().any(|i| i.managed && i.kind == kind && i.namespace.as_deref() == ns && i.name == name)
    }

    /// The layers that decide (kind, team, name): the tenant-level entry and the team's own.
    pub fn layers<'a>(&self, entries: &'a [Entry], kind: &str, team: Option<&str>, name: &str) -> Vec<Layer<'a>> {
        let mut out = Vec::new();
        for e in entries.iter().filter(|e| e.item.kind == kind && e.item.name == name) {
            let ns = e.item.namespace.as_deref();
            if ns.is_none() || ns == team {
                let kind = if e.item.managed && e.source == Source::File { LayerKind::Operator } else if ns.is_none() { LayerKind::Tenant } else { LayerKind::Team };
                out.push(Layer { kind, item: &e.item });
            }
        }
        out
    }

    /// All items as they stand, for export (config only).
    pub fn all_items(&self) -> Vec<Item> {
        self.entries().into_iter().map(|e| e.item).collect()
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { root: data_dir.join("cloud").join("config") }
    }
    pub fn file_path(&self) -> PathBuf {
        self.root.join("tenant-config.json")
    }
    fn local_path(&self) -> PathBuf {
        self.root.join("tenant-config.local.json")
    }
    fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    fn read_local(&self) -> Result<Local, String> {
        match std::fs::read_to_string(self.local_path()) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("tenant-config.local.json: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Local::default()),
            Err(e) => Err(format!("tenant-config.local.json: {e}")),
        }
    }

    /// Both documents. A broken API document is an error (nothing is guessed); a broken operator file leaves no operator items and says so.
    pub fn load(&self) -> Result<Config, String> {
        let l = self.read_local()?;
        let (file, file_hash, file_error) = match std::fs::read_to_string(self.file_path()) {
            Ok(t) => match serde_json::from_str::<Doc>(&t) {
                Ok(d) if d.items.len() <= MAX_ITEMS => (d, short_hash(t.as_bytes()), None),
                Ok(_) => (Doc::default(), String::new(), Some("tenant-config.json: too many items".to_string())),
                Err(e) => (Doc::default(), String::new(), Some(format!("tenant-config.json: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Doc::default(), String::new(), None),
            Err(e) => (Doc::default(), String::new(), Some(format!("tenant-config.json: {e}"))),
        };
        Ok(Config { file, local: l.items, version: l.version, file_hash, file_error })
    }

    /// Change the API document under a lock: `f` gets the current items and the operator's file and edits the items. Saved as the next
    /// version only when `f` succeeded. Returns the new version and the hash of the document.
    pub fn commit(&self, by: &str, summary: &str, f: impl FnOnce(&mut Vec<Item>, &Config) -> Result<(), String>) -> Result<(u64, String), String> {
        let _g = lock();
        let cfg = self.load()?;
        let mut items = cfg.local.clone();
        f(&mut items, &cfg)?;
        if items.len() > MAX_ITEMS {
            return Err(format!("at most {MAX_ITEMS} items"));
        }
        items.sort_by_key(|i| i.key());
        let version = cfg.version + 1;
        let doc = Doc { schema: SCHEMA_VERSION, items: items.clone() };
        let hash = short_hash(serde_json::to_string(&doc).unwrap_or_default().as_bytes());
        let info = VersionInfo { version, at: super::now_secs(), by: by.chars().take(64).collect(), hash: hash.clone(), summary: summary.chars().filter(|c| !c.is_control()).take(200).collect(), items: items.len() };
        let snap = SnapshotFile { info, doc };
        let put = |p: &Path, v: &dyn erased::Ser| iyakki_schema::write_atomic(p, v.pretty().as_bytes(), 0o600);
        put(&self.versions_dir().join(format!("{version:06}.json")), &snap)?;
        put(&self.local_path(), &Local { schema: SCHEMA_VERSION, version, items })?;
        self.prune();
        Ok((version, hash))
    }

    fn prune(&self) {
        let mut names: Vec<String> = std::fs::read_dir(self.versions_dir()).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".json")).collect();
        names.sort();
        while names.len() > MAX_VERSIONS {
            let n = names.remove(0);
            let _ = std::fs::remove_file(self.versions_dir().join(n));
        }
    }

    /// Newest first.
    pub fn versions(&self) -> Vec<VersionInfo> {
        let mut v: Vec<VersionInfo> = std::fs::read_dir(self.versions_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str::<SnapshotFile>(&t).ok())
            .map(|s| s.info)
            .collect();
        v.sort_by(|a, b| b.version.cmp(&a.version));
        v
    }

    pub fn version_doc(&self, n: u64) -> Result<Doc, String> {
        let t = std::fs::read_to_string(self.versions_dir().join(format!("{n:06}.json"))).map_err(|_| format!("version {n} is not kept (the last {MAX_VERSIONS} are)"))?;
        serde_json::from_str::<SnapshotFile>(&t).map(|s| s.doc).map_err(|e| format!("version {n}: {e}"))
    }

    // ---------------------------------------------------------------------------------------------------- meta

    /// Facts about an item that are not settings: when its key was last set, the last connection test (status only).
    pub fn meta(&self) -> BTreeMap<String, Meta> {
        std::fs::read_to_string(self.root.join("meta.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn update_meta(&self, key: &str, f: impl FnOnce(&mut Meta)) {
        let _g = lock();
        let mut m = self.meta();
        f(m.entry(key.to_string()).or_default());
        let _ = iyakki_schema::write_atomic(&self.root.join("meta.json"), serde_json::to_string(&m).unwrap_or_default().as_bytes(), 0o600);
    }

    // ---------------------------------------------------------------------------------------------------- usage

    /// Count one use of a shared integration by `user` in `scope` (a team, or `-`). Returns (the person's uses today, everybody's uses
    /// today in the scope, the scope's uses in the current 30-day period) after counting, or `Err` with the same numbers unchanged when a
    /// limit would be passed.
    #[allow(clippy::too_many_arguments)]
    pub fn count_use(&self, kind: &str, scope: &str, user: &str, now: u64, per_person: Option<u64>, per_tenant: Option<u64>, per_month: Option<u64>) -> Result<(u64, u64, u64), (u64, u64, u64)> {
        let _g = lock();
        let path = self.root.join("usage.json");
        let day = now / 86_400;
        let period = day / 30 * 30;
        let mut m: BTreeMap<String, u64> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        m.retain(|k, _| k.split('|').nth(1).and_then(|d| d.parse::<u64>().ok()).is_some_and(|d| d + 31 >= day));
        let (pk, tk, mk) = (format!("{kind}|{day}|u:{scope}:{user}"), format!("{kind}|{day}|all:{scope}"), format!("{kind}|{period}|m:{scope}"));
        let (p, t, mo) = (m.get(&pk).copied().unwrap_or(0), m.get(&tk).copied().unwrap_or(0), m.get(&mk).copied().unwrap_or(0));
        if per_person.is_some_and(|l| p >= l) || per_tenant.is_some_and(|l| t >= l) || per_month.is_some_and(|l| mo >= l) {
            return Err((p, t, mo));
        }
        m.insert(pk, p + 1);
        m.insert(tk, t + 1);
        m.insert(mk, mo + 1);
        let _ = iyakki_schema::write_atomic(&path, serde_json::to_string(&m).unwrap_or_default().as_bytes(), 0o600);
        Ok((p + 1, t + 1, mo + 1))
    }
}

/// A tiny object-safe "pretty JSON" so `commit` can write two different types with one closure.
mod erased {
    pub trait Ser {
        fn pretty(&self) -> String;
    }
    impl<T: serde::Serialize> Ser for T {
        fn pretty(&self) -> String {
            serde_json::to_string_pretty(self).unwrap_or_default()
        }
    }
}
