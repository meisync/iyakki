//! Flows on disk: a draft, immutable published versions, rollback, tags, diffs and the library of reusable nodes.
//!
//! ```text
//! <root>/<flow-id>/flow.json            the draft
//! <root>/<flow-id>/meta.json            published number, tags, the version list
//! <root>/<flow-id>/versions/v1.json ... immutable, monotonic, each with a content hash
//! <root>/_library/<node-id>/versions/vN.json   reusable nodes
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kinds::Registry;
use crate::model::*;

pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Where the engine finds flows (the store implements it; tests may use a map).
pub trait FlowSource: Send + Sync {
    fn get_published(&self, id: &str) -> Option<Flow>;
    fn get_version(&self, id: &str, version: u32) -> Option<Flow>;
    fn get_draft(&self, id: &str) -> Option<Flow>;
    /// The flows whose triggers are active: the latest published version of each.
    fn list_published(&self) -> Vec<Flow>;
    /// Make the agent's fix a new draft: replace `node` of the draft (or of a copy of the published version when there is no draft).
    fn save_draft_fix(&self, flow_id: &str, patched: &Flow, node_id: &str, who: &str, summary: &str, simple: bool) -> Result<u32, String>;
    /// Library nodes expanded into the flow.
    fn resolve(&self, flow: Flow) -> Flow {
        flow
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct VersionEntry {
    pub version: u32,
    #[serde(default)]
    pub label: String,
    pub author: String,
    pub at_ms: i64,
    #[serde(default)]
    pub changelog: String,
    pub hash: String,
    /// What the draft held when it was published (who changed what).
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default)]
    pub approved_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct FlowMeta {
    /// Layout of the flow's files (see `iyakki-schema`): 0 = alpha.1. A flow whose `meta.json` says a newer schema than
    /// [`SCHEMA`] knows is read but never written.
    #[serde(default)]
    pub schema: u32,
    /// Fields a newer release wrote: kept, and written back.
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
    #[serde(default)]
    pub published: u32,
    #[serde(default)]
    pub tags: BTreeMap<String, u32>,
    #[serde(default)]
    pub versions: Vec<VersionEntry>,
    /// Counts draft saves made by the agent's fixes (for "draft r3").
    #[serde(default)]
    pub draft_rev: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlowSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub published: u32,
    pub nodes: usize,
    pub triggers: usize,
    pub draft_changed: bool,
    pub agent_changed: bool,
    pub production: bool,
}

pub struct PublishReq<'a> {
    pub by: &'a str,
    pub label: &'a str,
    pub changelog: &'a str,
    /// A person looked at the agent's changes and agrees. Required when the draft holds changes by an agent, unless the flow's policy
    /// allows publishing simple fixes by themselves.
    pub approved_by: Option<&'a str>,
}

pub struct FlowStore {
    root: PathBuf,
    now: NowFn,
    reg: Registry,
    lock: Mutex<()>,
}

/// `meta.json` of a flow (versions, tags, which one is published). Schema 1 stamps the alpha.1 layout; the flow files themselves
/// (`flow.json`, `versions/vN.json`, `runs/<id>/run.json`) added fields with serde defaults only.
pub const SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "flow", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the alpha.1 layout", run: |_| {} }] };

/// A flow folder whose `meta.json` is from a newer release is never written (any of its files).
fn guard_newer(path: &Path) -> Result<(), String> {
    for up in [path.parent(), path.parent().and_then(|p| p.parent())].into_iter().flatten() {
        let m = up.join("meta.json");
        if m.is_file() {
            return match iyakki_schema::on_disk_is_newer(&SCHEMA, &m) {
                Some(n) => Err(iyakki_schema::newer_message(&SCHEMA, n)),
                None => Ok(()),
            };
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    guard_newer(path)?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("tmp{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

// ------------------------------------------------------------------------------------------------ diff

#[derive(Clone, Debug, PartialEq)]
pub struct FieldChange {
    pub path: String,
    pub old: Value,
    pub new: Value,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeDiff {
    pub id: String,
    pub changes: Vec<FieldChange>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct FlowDiff {
    pub nodes_added: Vec<String>,
    pub nodes_removed: Vec<String>,
    pub nodes_changed: Vec<NodeDiff>,
    pub edges_added: Vec<String>,
    pub edges_removed: Vec<String>,
    /// Name, description, triggers, defaults, inputs, outputs, vars.
    pub other: Vec<FieldChange>,
}

impl FlowDiff {
    pub fn is_empty(&self) -> bool {
        self.nodes_added.is_empty() && self.nodes_removed.is_empty() && self.nodes_changed.is_empty() && self.edges_added.is_empty() && self.edges_removed.is_empty() && self.other.is_empty()
    }
    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.nodes_added {
            s.push_str(&format!("+ node {n}\n"));
        }
        for n in &self.nodes_removed {
            s.push_str(&format!("- node {n}\n"));
        }
        for n in &self.nodes_changed {
            s.push_str(&format!("~ node {}\n", n.id));
            for c in &n.changes {
                s.push_str(&format!("    {}: {} -> {}\n", c.path, short(&c.old), short(&c.new)));
            }
        }
        for e in &self.edges_added {
            s.push_str(&format!("+ edge {e}\n"));
        }
        for e in &self.edges_removed {
            s.push_str(&format!("- edge {e}\n"));
        }
        for c in &self.other {
            s.push_str(&format!("~ {}: {} -> {}\n", c.path, short(&c.old), short(&c.new)));
        }
        if s.is_empty() {
            s.push_str("no differences\n");
        }
        s
    }
}

fn short(v: &Value) -> String {
    let t = match v {
        Value::Null => "(none)".to_string(),
        Value::String(s) => format!("\"{s}\""),
        o => o.to_string(),
    };
    if t.chars().count() > 70 {
        format!("{}...", t.chars().take(70).collect::<String>())
    } else {
        t
    }
}

pub fn diff_json(old: &Value, new: &Value, prefix: &str, out: &mut Vec<FieldChange>) {
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                diff_json(a.get(k).unwrap_or(&Value::Null), b.get(k).unwrap_or(&Value::Null), &p, out);
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff_json(x, y, &format!("{prefix}[{i}]"), out);
            }
        }
        (a, b) if a == b => {}
        (a, b) => out.push(FieldChange { path: prefix.to_string(), old: a.clone(), new: b.clone() }),
    }
}

/// A node as the diff sees it: no position, no badge.
fn node_json(n: &Node) -> Value {
    let mut v = serde_json::to_value(n).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        o.remove("pos");
        o.remove("modified_by");
    }
    v
}

pub fn diff(old: &Flow, new: &Flow) -> FlowDiff {
    let mut d = FlowDiff::default();
    for n in &new.nodes {
        match old.node(&n.id) {
            None => d.nodes_added.push(n.id.clone()),
            Some(o) => {
                let mut ch = Vec::new();
                diff_json(&node_json(o), &node_json(n), "", &mut ch);
                if !ch.is_empty() {
                    d.nodes_changed.push(NodeDiff { id: n.id.clone(), changes: ch });
                }
            }
        }
    }
    for n in &old.nodes {
        if new.node(&n.id).is_none() {
            d.nodes_removed.push(n.id.clone());
        }
    }
    let ek = |f: &Flow| -> Vec<String> { f.edges.iter().map(|e| if e.when.is_empty() { e.key() } else { format!("{} [{}]", e.key(), e.when) }).collect() };
    let (oe, ne) = (ek(old), ek(new));
    d.edges_added = ne.iter().filter(|e| !oe.contains(e)).cloned().collect();
    d.edges_removed = oe.iter().filter(|e| !ne.contains(e)).cloned().collect();
    let strip = |f: &Flow| {
        let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut v {
            for k in ["nodes", "edges", "meta"] {
                o.remove(k);
            }
        }
        v
    };
    diff_json(&strip(old), &strip(new), "", &mut d.other);
    d
}

// ------------------------------------------------------------------------------------------------ the store

impl FlowStore {
    pub fn new(root: &Path, now: NowFn) -> FlowStore {
        FlowStore { root: root.to_path_buf(), now, reg: Registry::builtin(), lock: Mutex::new(()) }
    }

    pub fn with_registry(mut self, reg: Registry) -> FlowStore {
        self.reg = reg;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("`{id}` is not a flow id (letters, digits, - and _)"));
        }
        Ok(self.root.join(id))
    }

    fn meta(&self, id: &str) -> FlowMeta {
        match self.dir(id) {
            Ok(d) if d.join("meta.json").is_file() => iyakki_schema::load_typed::<FlowMeta>(&SCHEMA, &d.join("meta.json"), iyakki_schema::now_secs()).1,
            _ => FlowMeta::default(),
        }
    }

    fn save_meta(&self, id: &str, m: &FlowMeta) -> Result<(), String> {
        iyakki_schema::save_typed(&SCHEMA, &self.dir(id)?.join("meta.json"), m, 0o644)
    }

    pub fn meta_of(&self, id: &str) -> FlowMeta {
        self.meta(id)
    }

    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.root).map(|rd| rd.flatten().filter(|e| e.path().join("flow.json").is_file() || e.path().join("meta.json").is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| valid_id(n)).collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn exists(&self, id: &str) -> bool {
        self.dir(id).is_ok_and(|d| d.join("flow.json").is_file())
    }

    pub fn create(&self, mut flow: Flow, who: &str) -> Result<Flow, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(&flow.id)?;
        if dir.join("flow.json").exists() {
            return Err(format!("there is already a flow `{}`", flow.id));
        }
        // who changed what in a draft that is being made (the agent's own new draft carries its badge) is kept
        let changes = std::mem::take(&mut flow.meta.modified_by);
        flow.meta = Meta { version: 0, base_version: 0, author: who.to_string(), created_ms: (self.now)(), modified_by: changes, ..Default::default() };
        write_atomic(&dir.join("flow.json"), &flow.to_json())?;
        self.save_meta(&flow.id, &FlowMeta::default())?;
        Ok(flow)
    }

    pub fn get_draft(&self, id: &str) -> Option<Flow> {
        read_json(&self.dir(id).ok()?.join("flow.json"))
    }

    /// Save the draft. `change` (who changed what) is added to the draft's list, which the "agent changed" badge reads.
    pub fn save_draft(&self, flow: &Flow, change: Option<Change>) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(&flow.id)?;
        if !dir.join("flow.json").exists() {
            return Err(format!("there is no flow `{}`", flow.id));
        }
        let mut f = flow.clone();
        if let Some(c) = change {
            f.meta.modified_by.push(c);
        }
        write_atomic(&dir.join("flow.json"), &f.to_json())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(id)?;
        if !dir.exists() {
            return Err(format!("there is no flow `{id}`"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
    }

    pub fn get_version(&self, id: &str, version: u32) -> Option<Flow> {
        let f: Flow = read_json(&self.dir(id).ok()?.join("versions").join(format!("v{version}.json")))?;
        Some(f)
    }

    pub fn published(&self, id: &str) -> Option<Flow> {
        let m = self.meta(id);
        if m.published == 0 {
            return None;
        }
        self.get_version(id, m.published)
    }

    pub fn versions(&self, id: &str) -> Vec<VersionEntry> {
        self.meta(id).versions
    }

    /// Does a published file still match its recorded hash?
    pub fn verify(&self, id: &str, version: u32) -> Result<(), String> {
        let f = self.get_version(id, version).ok_or_else(|| format!("version {version} does not exist"))?;
        let want = self.meta(id).versions.iter().find(|v| v.version == version).map(|v| v.hash.clone()).ok_or("the version is not in the index")?;
        if f.content_hash() == want {
            Ok(())
        } else {
            Err(format!("version {version} was changed on disk after it was published"))
        }
    }

    /// The draft differs from the published version (ignoring who touched it and where nodes sit).
    pub fn draft_changed(&self, id: &str) -> bool {
        match (self.get_draft(id), self.published(id)) {
            (Some(d), Some(p)) => !diff(&p, &d).is_empty(),
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub fn summaries(&self) -> Vec<FlowSummary> {
        self.ids()
            .into_iter()
            .filter_map(|id| {
                let d = self.get_draft(&id)?;
                let m = self.meta(&id);
                Some(FlowSummary { id: id.clone(), name: d.name.clone(), description: d.description.clone(), published: m.published, nodes: d.nodes.len(), triggers: d.triggers.len(), draft_changed: self.draft_changed(&id), agent_changed: d.meta.modified_by.iter().any(Change::by_agent), production: d.production })
            })
            .collect()
    }

    pub fn publish(&self, id: &str, req: &PublishReq) -> Result<u32, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(id)?;
        let mut draft: Flow = read_json(&dir.join("flow.json")).ok_or_else(|| format!("there is no flow `{id}`"))?;
        let issues = crate::validate::validate_flow(&self.expand_library(draft.clone()), &self.reg);
        let errs: Vec<String> = crate::validate::errors(&issues).iter().map(|i| format!("{}{}", i.node.as_ref().map(|n| format!("{n}: ")).unwrap_or_default(), i.msg)).collect();
        if !errs.is_empty() {
            return Err(format!("the flow has problems that stop publishing: {}", errs.join("; ")));
        }
        let mut meta = self.meta(id);
        let agent_changes: Vec<&Change> = draft.meta.modified_by.iter().filter(|c| c.by_agent()).collect();
        if !agent_changes.is_empty() && req.approved_by.is_none() {
            let auto = draft.defaults.heal.auto_publish_simple && agent_changes.iter().all(|c| c.simple) && !draft.production;
            if !auto {
                return Err(format!("the draft holds changes made by the agent ({}): a person has to approve publishing them", agent_changes.iter().map(|c| c.summary.clone()).collect::<Vec<_>>().join("; ")));
            }
        }
        if let Some(p) = self.published(id) {
            if diff(&p, &draft).is_empty() {
                return Err("nothing changed since the published version".into());
            }
        }
        let next = meta.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;
        let path = dir.join("versions").join(format!("v{next}.json"));
        if path.exists() {
            return Err(format!("version {next} already exists and is never overwritten"));
        }
        let changes = draft.meta.modified_by.clone();
        let who_approved = req.approved_by.unwrap_or("").to_string();
        draft.meta = Meta { version: next, base_version: next, label: req.label.to_string(), author: req.by.to_string(), created_ms: (self.now)(), changelog: req.changelog.to_string(), modified_by: vec![], hash: String::new() };
        for n in &mut draft.nodes {
            n.modified_by = None;
        }
        draft.meta.hash = draft.content_hash();
        write_atomic(&path, &draft.to_json())?;
        meta.published = next;
        meta.versions.push(VersionEntry { version: next, label: req.label.to_string(), author: req.by.to_string(), at_ms: (self.now)(), changelog: req.changelog.to_string(), hash: draft.meta.hash.clone(), changes, approved_by: who_approved });
        self.save_meta(id, &meta)?;
        write_atomic(&dir.join("flow.json"), &draft.to_json())?;
        Ok(next)
    }

    /// Publish an older version's content as a new version.
    pub fn rollback(&self, id: &str, to: u32, by: &str) -> Result<u32, String> {
        let old = self.get_version(id, to).ok_or_else(|| format!("version {to} does not exist"))?;
        self.verify(id, to)?;
        {
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let dir = self.dir(id)?;
            let mut f = old;
            f.meta.modified_by.clear();
            write_atomic(&dir.join("flow.json"), &f.to_json())?;
        }
        self.publish(id, &PublishReq { by, label: "", changelog: &format!("Rolled back to v{to}"), approved_by: Some(by) })
    }

    /// Remove one published version (its file and its entry). The current published version is never dropped here: publish or roll
    /// back to another one first. Runs keep their own copy of the flow, so they are not touched.
    pub fn drop_version(&self, id: &str, version: u32) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(id)?;
        let mut m = self.meta(id);
        if !m.versions.iter().any(|v| v.version == version) {
            return Err(format!("version {version} does not exist"));
        }
        if m.published == version {
            return Err(if m.versions.len() == 1 { format!("version {version} is the only version and the one that runs; it is not dropped (delete the flow instead)") } else { format!("version {version} is the current published version; publish or roll back to another version first") });
        }
        m.versions.retain(|v| v.version != version);
        m.tags.retain(|_, v| *v != version);
        self.save_meta(id, &m)?;
        let _ = std::fs::remove_file(dir.join("versions").join(format!("v{version}.json")));
        Ok(())
    }

    pub fn tag(&self, id: &str, name: &str, version: u32) -> Result<(), String> {
        if name.is_empty() || name.len() > 32 || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err("a tag is letters, digits, - _ . (at most 32)".into());
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.meta(id);
        if !m.versions.iter().any(|v| v.version == version) {
            return Err(format!("version {version} does not exist"));
        }
        m.tags.insert(name.to_string(), version);
        self.save_meta(id, &m)
    }

    pub fn diff_versions(&self, id: &str, a: u32, b: Option<u32>) -> Result<FlowDiff, String> {
        let old = self.get_version(id, a).ok_or_else(|| format!("version {a} does not exist"))?;
        let new = match b {
            Some(n) => self.get_version(id, n).ok_or_else(|| format!("version {n} does not exist"))?,
            None => self.get_draft(id).ok_or("there is no draft")?,
        };
        Ok(diff(&old, &new))
    }

    // ------------------------------------------------------------------------------------------ library

    fn lib_dir(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("`{id}` is not a library node id"));
        }
        Ok(self.root.join("_library").join(id))
    }

    /// Save a node into the library as its next version.
    pub fn lib_publish(&self, id: &str, node: &Node, by: &str) -> Result<u32, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.lib_dir(id)?;
        let next = self.lib_versions(id).last().copied().unwrap_or(0) + 1;
        let mut n = node.clone();
        n.lib = None;
        n.modified_by = None;
        n.id = id.to_string();
        write_atomic(&dir.join("versions").join(format!("v{next}.json")), &serde_json::to_string_pretty(&n).unwrap_or_default())?;
        write_atomic(&dir.join("by.json"), &serde_json::json!({"last_by": by, "at_ms": (self.now)()}).to_string())?;
        Ok(next)
    }

    pub fn lib_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.root.join("_library")).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| valid_id(n)).collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn lib_versions(&self, id: &str) -> Vec<u32> {
        let mut v: Vec<u32> = self.lib_dir(id).ok().and_then(|d| std::fs::read_dir(d.join("versions")).ok()).map(|rd| rd.flatten().filter_map(|e| e.file_name().to_string_lossy().strip_prefix('v')?.strip_suffix(".json")?.parse().ok()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn lib_get(&self, id: &str, version: u32) -> Option<Node> {
        read_json(&self.lib_dir(id).ok()?.join("versions").join(format!("v{version}.json")))
    }

    pub fn lib_latest(&self, id: &str) -> Option<(u32, Node)> {
        let v = *self.lib_versions(id).last()?;
        Some((v, self.lib_get(id, v)?))
    }

    /// Which flows use the library node, and at which version: `(flow id, version used, in the draft or the published version)`.
    pub fn lib_usage(&self, id: &str) -> Vec<(String, u32, &'static str)> {
        let mut out = Vec::new();
        for fid in self.ids() {
            for (which, f) in [("draft", self.get_draft(&fid)), ("published", self.published(&fid))] {
                if let Some(f) = f {
                    for n in &f.nodes {
                        if let Some(l) = &n.lib {
                            if l.id == id {
                                out.push((fid.clone(), l.version, which));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// The flow as the engine will see it (library references filled in), for checking a draft.
    pub fn resolve_for_check(&self, f: &Flow) -> Flow {
        self.expand_library(f.clone())
    }

    /// Replace library references with their definitions (the referenced version, or the latest for a node that tracks the latest).
    pub fn expand_library(&self, mut flow: Flow) -> Flow {
        for n in &mut flow.nodes {
            let Some(l) = n.lib.clone() else { continue };
            let want = match n.track {
                Track::Pinned => if l.version > 0 { Some(l.version) } else { self.lib_versions(&l.id).last().copied() },
                Track::LatestPublished => self.lib_versions(&l.id).last().copied(),
                Track::Compatible => {
                    let latest = self.lib_versions(&l.id).last().copied();
                    match (latest, self.lib_get(&l.id, l.version)) {
                        (Some(lv), Some(cur)) => match self.lib_get(&l.id, lv) {
                            Some(new) if compatible(&cur, &new, &self.reg) => Some(lv),
                            _ => Some(l.version),
                        },
                        (latest, _) => latest,
                    }
                }
            };
            if let Some(def) = want.and_then(|v| self.lib_get(&l.id, v).map(|d| (v, d))) {
                let (v, d) = def;
                n.kind = d.kind;
                n.config = d.config;
                n.retry = d.retry;
                n.timeout_ms = d.timeout_ms;
                n.runner = d.runner;
                n.heal = d.heal;
                n.on_error = d.on_error;
                n.lib = Some(LibRef { id: l.id, version: v });
            }
        }
        flow
    }
}

/// Same kind, same ports, same output names: a newer version may replace an older one under a running task without changing what
/// downstream nodes see.
pub fn compatible(old: &Node, new: &Node, reg: &Registry) -> bool {
    if old.kind != new.kind {
        return false;
    }
    if let Some(k) = reg.get(&old.kind) {
        if k.ports(old) != k.ports(new) {
            return false;
        }
    }
    output_signature(old) == output_signature(new)
}

/// The names a node's output has, from its configuration.
pub fn output_signature(n: &Node) -> String {
    let keys = |k: &str| -> Vec<String> { n.config.get(k).and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default() };
    let mut parts: Vec<String> = keys("extract").into_iter().map(|k| format!("x:{k}")).collect();
    parts.extend(keys("outputs").into_iter().map(|k| format!("o:{k}")));
    parts.push(format!("parse:{}", n.config.get("parse").and_then(Value::as_str).unwrap_or("")));
    if n.config.get("output_schema").is_some() {
        parts.push(format!("schema:{}", n.config["output_schema"]));
    }
    parts.sort();
    parts.join(",")
}

impl FlowSource for FlowStore {
    fn get_published(&self, id: &str) -> Option<Flow> {
        self.published(id).map(|f| self.expand_library(f))
    }
    fn get_version(&self, id: &str, version: u32) -> Option<Flow> {
        FlowStore::get_version(self, id, version).map(|f| self.expand_library(f))
    }
    fn get_draft(&self, id: &str) -> Option<Flow> {
        FlowStore::get_draft(self, id).map(|f| self.expand_library(f))
    }
    fn list_published(&self) -> Vec<Flow> {
        self.ids().into_iter().filter_map(|id| self.published(&id)).map(|f| self.expand_library(f)).collect()
    }
    fn resolve(&self, flow: Flow) -> Flow {
        self.expand_library(flow)
    }
    fn save_draft_fix(&self, flow_id: &str, patched: &Flow, node_id: &str, who: &str, summary: &str, simple: bool) -> Result<u32, String> {
        // the fix is applied to the node of the draft (or, when there is no draft change, of the published version) and nothing else
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.dir(flow_id)?;
        let mut draft: Flow = read_json(&dir.join("flow.json")).ok_or_else(|| format!("there is no flow `{flow_id}`"))?;
        let new_node = patched.node(node_id).cloned().ok_or_else(|| format!("no node `{node_id}` in the fixed flow"))?;
        let change = Change { who: who.to_string(), node: node_id.to_string(), summary: summary.to_string(), at_ms: (self.now)(), simple };
        match draft.node_mut(node_id) {
            Some(n) => {
                let pos = n.pos;
                *n = new_node;
                n.pos = pos;
                n.modified_by = Some(change.clone());
            }
            None => return Err(format!("the draft has no node `{node_id}` any more")),
        }
        draft.meta.modified_by.push(change);
        write_atomic(&dir.join("flow.json"), &draft.to_json())?;
        let mut m = self.meta(flow_id);
        m.draft_rev += 1;
        let rev = m.draft_rev;
        self.save_meta(flow_id, &m)?;
        Ok(rev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flow-store-{tag}-{}-{}", std::process::id(), crate::retry::hash_parts(&[tag, &format!("{:?}", std::thread::current().id())])));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn flow(id: &str) -> Flow {
        let mut f = Flow::new(id, "Check");
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("a", "http", json!({"url": "https://x.example/a", "extract": {"n": "count"}})));
        f.nodes.push(Node::new("e", "end", json!({})));
        f.edges.push(Edge::new("s", "a"));
        f.edges.push(Edge::new("a", "e"));
        f
    }

    fn store(tag: &str) -> (FlowStore, PathBuf, Arc<std::sync::atomic::AtomicI64>) {
        let d = tmp(tag);
        let t = Arc::new(std::sync::atomic::AtomicI64::new(1000));
        let t2 = t.clone();
        (FlowStore::new(&d, Arc::new(move || t2.fetch_add(1, std::sync::atomic::Ordering::SeqCst))), d, t)
    }

    fn req<'a>(by: &'a str, log: &'a str) -> PublishReq<'a> {
        PublishReq { by, label: "", changelog: log, approved_by: None }
    }

    #[test]
    fn create_edit_publish_and_never_overwrite() {
        let (s, d, _) = store("pub");
        let f = s.create(flow("chk"), "user:me").unwrap();
        assert_eq!(f.meta.version, 0);
        assert!(s.create(flow("chk"), "user:me").unwrap_err().contains("already"));
        assert!(s.create(flow("../x"), "u").is_err());
        assert!(s.published("chk").is_none());
        let v1 = s.publish("chk", &req("user:me", "first")).unwrap();
        assert_eq!(v1, 1);
        assert_eq!(s.published("chk").unwrap().meta.version, 1);
        assert!(s.verify("chk", 1).is_ok());
        assert!(s.publish("chk", &req("user:me", "again")).unwrap_err().contains("nothing changed"));
        let mut dr = s.get_draft("chk").unwrap();
        dr.node_mut("a").unwrap().timeout_ms = Some(900);
        s.save_draft(&dr, None).unwrap();
        assert!(s.draft_changed("chk"));
        let v2 = s.publish("chk", &PublishReq { by: "user:me", label: "1.1.0", changelog: "slower", approved_by: None }).unwrap();
        assert_eq!(v2, 2);
        assert_eq!(s.versions("chk").iter().map(|v| v.version).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(s.versions("chk")[1].label, "1.1.0");
        assert!(!s.draft_changed("chk"));
        // a published file edited on disk is detected
        let p = d.join("chk/versions/v1.json");
        let t = std::fs::read_to_string(&p).unwrap().replace("x.example/a", "evil.example/a");
        std::fs::write(&p, t).unwrap();
        assert!(s.verify("chk", 1).unwrap_err().contains("changed on disk"));
        assert!(s.rollback("chk", 1, "user:me").is_err(), "a tampered version cannot be rolled back to");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn publishing_checks_the_flow_and_agent_changes_need_a_person() {
        let (s, d, _) = store("agent");
        let mut bad = flow("bad");
        bad.edges.push(Edge::new("e", "s"));
        s.create(bad, "u").unwrap();
        assert!(s.publish("bad", &req("u", "x")).unwrap_err().contains("problems"));
        s.create(flow("g"), "u").unwrap();
        s.publish("g", &req("u", "v1")).unwrap();
        let mut dr = s.get_draft("g").unwrap();
        dr.node_mut("a").unwrap().timeout_ms = Some(5);
        s.save_draft(&dr, Some(Change { who: "agent:assistant".into(), node: "a".into(), summary: "timeout".into(), at_ms: 5, simple: true })).unwrap();
        assert!(s.get_draft("g").unwrap().meta.modified_by.iter().any(Change::by_agent));
        let e = s.publish("g", &req("agent:assistant", "fix")).unwrap_err();
        assert!(e.contains("approve"), "{e}");
        assert!(s.summaries().iter().any(|x| x.id == "g" && x.agent_changed && x.draft_changed));
        // the flow's own policy may allow publishing simple fixes
        let mut dr2 = s.get_draft("g").unwrap();
        dr2.defaults.heal.auto_publish_simple = true;
        s.save_draft(&dr2, None).unwrap();
        let v = s.publish("g", &req("agent:assistant", "fix")).unwrap();
        assert_eq!(v, 2);
        assert!(s.versions("g")[1].changes.iter().any(|c| c.by_agent()), "the history says the agent changed it");
        // a non-simple agent change still needs a person even then
        let mut dr3 = s.get_draft("g").unwrap();
        dr3.node_mut("a").unwrap().timeout_ms = Some(6);
        s.save_draft(&dr3, Some(Change { who: "agent:assistant".into(), node: "a".into(), summary: "other".into(), at_ms: 6, simple: false })).unwrap();
        assert!(s.publish("g", &req("agent:assistant", "x")).is_err());
        let ok = s.publish("g", &PublishReq { by: "user:me", label: "", changelog: "reviewed", approved_by: Some("user:me") }).unwrap();
        assert_eq!(ok, 3);
        assert_eq!(s.versions("g")[2].approved_by, "user:me");
        assert!(s.get_draft("g").unwrap().meta.modified_by.is_empty(), "the badge goes after publishing");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rollback_publishes_old_content_as_a_new_monotonic_version() {
        let (s, d, _) = store("rb");
        s.create(flow("r"), "u").unwrap();
        s.publish("r", &req("u", "v1")).unwrap();
        let mut dr = s.get_draft("r").unwrap();
        dr.node_mut("a").unwrap().config["url"] = json!("https://x.example/b");
        s.save_draft(&dr, None).unwrap();
        s.publish("r", &req("u", "v2")).unwrap();
        let v3 = s.rollback("r", 1, "user:me").unwrap();
        assert_eq!(v3, 3);
        assert_eq!(s.published("r").unwrap().node("a").unwrap().config["url"], "https://x.example/a");
        assert_eq!(s.versions("r")[2].changelog, "Rolled back to v1");
        assert_eq!(s.versions("r").iter().map(|v| v.version).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(s.rollback("r", 9, "u").is_err());
        s.tag("r", "stable", 1).unwrap();
        assert_eq!(s.meta_of("r").tags["stable"], 1);
        assert!(s.tag("r", "bad tag", 1).is_err() && s.tag("r", "x", 9).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn diffs_are_node_level_and_field_level() {
        let a = flow("d");
        let mut b = a.clone();
        b.node_mut("a").unwrap().config["url"] = json!("https://x.example/b");
        b.node_mut("a").unwrap().timeout_ms = Some(500);
        b.nodes.push(Node::new("n", "notify", json!({"title": "t"})));
        b.edges.push(Edge::new("a", "n"));
        b.nodes.retain(|n| n.id != "e");
        b.edges.retain(|e| e.to != "e");
        b.name = "Renamed".into();
        b.triggers.push(Trigger::new("t", TriggerKind::Manual));
        b.node_mut("s").unwrap().pos = Pos { x: 9.0, y: 9.0 };
        let d = diff(&a, &b);
        assert_eq!(d.nodes_added, vec!["n"]);
        assert_eq!(d.nodes_removed, vec!["e"]);
        assert_eq!(d.nodes_changed.len(), 1, "moving a node is not a change");
        let ch = &d.nodes_changed[0];
        assert_eq!(ch.id, "a");
        assert!(ch.changes.iter().any(|c| c.path == "config.url" && c.new == json!("https://x.example/b")));
        assert!(ch.changes.iter().any(|c| c.path == "timeout_ms"));
        assert_eq!(d.edges_added, vec!["a:out>n"]);
        assert_eq!(d.edges_removed, vec!["a:out>e"]);
        assert!(d.other.iter().any(|c| c.path == "name") && d.other.iter().any(|c| c.path.starts_with("triggers")));
        assert!(d.text().contains("~ node a") && d.text().contains("config.url"));
        assert!(diff(&a, &a).is_empty());
        assert_eq!(diff(&a, &a).text().trim(), "no differences");
        let (s, dir, _) = store("dv");
        s.create(a.clone(), "u").unwrap();
        s.publish("d", &req("u", "1")).unwrap();
        assert!(s.diff_versions("d", 1, None).unwrap().is_empty());
        assert!(s.diff_versions("d", 5, None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_library_has_versions_usage_and_tracking() {
        let (s, d, _) = store("lib");
        let lib_node = Node::new("ignored", "http", json!({"url": "https://x.example/v1", "extract": {"n": "count"}}));
        assert_eq!(s.lib_publish("ping", &lib_node, "u").unwrap(), 1);
        let mut v2 = lib_node.clone();
        v2.config["url"] = json!("https://x.example/v2");
        assert_eq!(s.lib_publish("ping", &v2, "u").unwrap(), 2);
        let mut v3 = lib_node.clone();
        v3.config["extract"] = json!({"other": "x"});
        assert_eq!(s.lib_publish("ping", &v3, "u").unwrap(), 3);
        assert_eq!(s.lib_ids(), vec!["ping"]);
        assert_eq!(s.lib_latest("ping").unwrap().0, 3);
        for (id, track) in [("pin", Track::Pinned), ("lat", Track::LatestPublished), ("cmp", Track::Compatible)] {
            let mut f = flow(id);
            f.nodes[1] = Node::new("a", "http", json!({}));
            f.nodes[1].lib = Some(LibRef { id: "ping".into(), version: 1 });
            f.nodes[1].track = track;
            s.create(f, "u").unwrap();
            s.publish(id, &req("u", "x")).unwrap();
        }
        let url_of = |id: &str| FlowSource::get_published(&s, id).unwrap().node("a").unwrap().config["url"].as_str().unwrap_or("").to_string();
        assert!(url_of("pin").ends_with("/v1"), "pinned stays");
        assert!(url_of("lat").ends_with("/v1") || FlowSource::get_published(&s, "lat").unwrap().node("a").unwrap().config.get("extract") == Some(&json!({"other": "x"})), "latest-published takes v3");
        assert_eq!(FlowSource::get_published(&s, "lat").unwrap().node("a").unwrap().lib.as_ref().unwrap().version, 3);
        assert_eq!(FlowSource::get_published(&s, "cmp").unwrap().node("a").unwrap().lib.as_ref().unwrap().version, 1, "v3 changes the outputs, so compatible stays on v1");
        let usage = s.lib_usage("ping");
        assert_eq!(usage.iter().filter(|u| u.2 == "published").count(), 3);
        assert!(usage.iter().all(|u| u.1 == 1));
        assert!(s.lib_get("../x", 1).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_heal_fix_goes_to_the_draft_with_a_badge() {
        let (s, d, _) = store("fix");
        s.create(flow("h"), "u").unwrap();
        s.publish("h", &req("u", "v1")).unwrap();
        let mut patched = FlowSource::get_published(&s, "h").unwrap();
        patched.node_mut("a").unwrap().timeout_ms = Some(9000);
        let rev = s.save_draft_fix("h", &patched, "a", "agent:assistant", "timeout 9000 ms", true).unwrap();
        assert_eq!(rev, 1);
        let dr = s.get_draft("h").unwrap();
        assert_eq!(dr.node("a").unwrap().timeout_ms, Some(9000));
        assert!(dr.node("a").unwrap().modified_by.as_ref().unwrap().by_agent());
        assert_eq!(s.published("h").unwrap().node("a").unwrap().timeout_ms, None, "the published version is untouched");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn compatibility_is_about_ports_and_output_names() {
        let reg = Registry::builtin();
        let a = Node::new("a", "http", json!({"extract": {"n": "x"}}));
        let mut b = a.clone();
        b.config["url"] = json!("https://changed");
        assert!(compatible(&a, &b, &reg));
        b.config["extract"] = json!({"m": "x"});
        assert!(!compatible(&a, &b, &reg));
        let c = Node::new("a", "cli", json!({}));
        assert!(!compatible(&a, &c, &reg));
        let sw1 = Node::new("s", "switch", json!({"cases": [{"equals": 1, "port": "one"}]}));
        let sw2 = Node::new("s", "switch", json!({"cases": [{"equals": 1, "port": "uno"}]}));
        assert!(!compatible(&sw1, &sw2, &reg), "different ports");
    }
}
