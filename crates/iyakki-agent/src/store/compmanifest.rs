//! The signed component manifest of a private store: `GET <store>/v1/components.json` (docs/development/COMPONENT-UPDATES-PLAN.md).
//!
//! A component is one separately updated part of the system: `kernel`, `boot`, `base`, `core`, `kernel-layer` (apps stay packages). Each
//! entry names the artifact by its SHA-256 (the blob is `GET /v1/blobs/<sha256>`), its version, the ranges of the other components it
//! needs (`requires`), the channel and, for the versions still supported, a delta from an older full artifact. The store signs the whole
//! document with the catalog key (purpose `components`, the same helper the catalog, the withdrawn list and the system announcement use);
//! a device accepts it only when it names exactly the key that was pinned when the store was added.
//!
//! This file holds what the server and the device share: the entry, the ranges, the signing and checking of the document, and the
//! retention policy (which old artifacts a cleanup may remove).

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::trust::{sign_doc, verify_doc};
use crate::devmode::update::compare_versions;
use crate::pkg::public_key_hex;

pub const PATH: &str = "/v1/components.json";
pub const PURPOSE: &str = "components";
/// The components a store of this version takes (the plan's table; apps stay packages).
pub const COMPONENTS: [&str; 5] = ["kernel", "boot", "base", "core", "kernel-layer"];
pub const MAX_DELTAS: usize = 8;
pub const MAX_REQUIRES: usize = 8;
/// The most bytes read for the document.
pub const MAX_DOC: usize = 2 << 20;

/// A delta from an older full artifact (`from_sha256`) to the entry's own artifact; `sha256` and `size` are those of the delta blob.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Delta {
    pub from_sha256: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub component: String,
    pub version: String,
    pub sha256: String,
    pub size: u64,
    /// Other components (or `abi`-like names) and the range each must be in: `{"kernel": ">=6.12"}`.
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
    pub channel: String,
    #[serde(default)]
    pub built_from: String,
    #[serde(default)]
    pub delta: Vec<Delta>,
    /// Where the artifact goes: `boot/bzImage` for a part of the boot partition, an absolute path for a file of the running system.
    #[serde(default)]
    pub install_path: String,
    #[serde(default)]
    pub added: i64,
}

/// An entry as the server keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub entry: Entry,
    pub owner: String,
    #[serde(default)]
    pub yanked: Option<String>,
    /// The files were removed by a cleanup; only the withdrawal notice stays.
    #[serde(default)]
    pub purged: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Yank {
    pub component: String,
    pub version: String,
    pub sha256: String,
    pub channel: String,
    pub reason: String,
}

// ------------------------------------------------------------------------------------------------ names and versions

pub fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_component_name(s: &str) -> bool {
    (2..=24).contains(&s.len()) && s.starts_with(|c: char| c.is_ascii_lowercase()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn valid_channel(s: &str) -> bool {
    (2..=24).contains(&s.len()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A version: digits and dots, an optional `-pre` part (`1107`, `6.12.5`, `0.1.0-alpha.3`).
pub fn valid_version(v: &str) -> bool {
    let (core, pre) = v.split_once('-').unwrap_or((v, "x"));
    !v.is_empty() && v.len() <= 32 && !core.is_empty() && core.split('.').all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit())) && pre.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// A path an artifact goes to: the boot partition (`boot/...`, `EFI/...`) or an absolute path of the running system; plain names only.
pub fn valid_install_path(p: &str) -> bool {
    let plain = |c: &str| !c.is_empty() && c.len() <= 100 && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "._-+~@".contains(ch));
    if let Some(rest) = p.strip_prefix('/') {
        let c: Vec<&str> = rest.split('/').collect();
        return p.len() <= 300 && c.len() <= 12 && c.iter().all(|x| plain(x));
    }
    crate::devmode::update::valid_path(p)
}

/// The default install path of a component (none for `kernel-layer`, which has to say).
pub fn default_install_path(component: &str) -> Option<&'static str> {
    match component {
        "kernel" => Some("boot/bzImage"),
        "boot" => Some("boot/initrd-lean"),
        "base" => Some("boot/root.sfs"),
        "core" => Some("/usr/bin/iyakki"),
        _ => None,
    }
}

/// What an update of this component needs from the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// Nothing runs differently until the services are started again (the core and the kernel layer swap a program).
    Services,
    /// The machine restarts (kernel, boot, base).
    Machine,
}

pub fn restart_of(component: &str) -> Restart {
    match component {
        "core" | "kernel-layer" => Restart::Services,
        _ => Restart::Machine,
    }
}

// ------------------------------------------------------------------------------------------------ ranges

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

/// A version range: comparators joined by commas or spaces (`>=6.12, <7`), `6.12..6.13` (from 6.12, before 6.13), `1107` (exactly), `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    parts: Vec<(Op, String)>,
    text: String,
}

impl Range {
    pub fn parse(s: &str) -> Result<Range, String> {
        let t = s.trim();
        let bad = |why: &str| format!("the range {t:?} is not understood: {why} (examples: >=6.12, 6.12..6.13, <2, 1107, *)");
        if t.is_empty() || t.len() > 60 {
            return Err(bad("it is empty or too long"));
        }
        if t == "*" {
            return Ok(Range { parts: vec![], text: t.into() });
        }
        let mut parts = vec![];
        for tok in t.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()) {
            if let Some((a, b)) = tok.split_once("..") {
                if !valid_version(a) || !valid_version(b) {
                    return Err(bad("a..b needs two versions"));
                }
                parts.push((Op::Ge, a.to_string()));
                parts.push((Op::Lt, b.to_string()));
                continue;
            }
            let (op, v) = if let Some(v) = tok.strip_prefix(">=") {
                (Op::Ge, v)
            } else if let Some(v) = tok.strip_prefix("<=") {
                (Op::Le, v)
            } else if let Some(v) = tok.strip_prefix('>') {
                (Op::Gt, v)
            } else if let Some(v) = tok.strip_prefix('<') {
                (Op::Lt, v)
            } else if let Some(v) = tok.strip_prefix('=') {
                (Op::Eq, v)
            } else {
                (Op::Eq, tok)
            };
            if !valid_version(v) {
                return Err(bad(&format!("{v:?} is not a version")));
            }
            parts.push((op, v.to_string()));
        }
        if parts.is_empty() {
            return Err(bad("nothing to compare"));
        }
        Ok(Range { parts, text: t.into() })
    }

    pub fn matches(&self, version: &str) -> bool {
        use std::cmp::Ordering::*;
        self.parts.iter().all(|(op, v)| {
            let c = compare_versions(version, v);
            match op {
                Op::Ge => c != Less,
                Op::Gt => c == Greater,
                Op::Le => c != Greater,
                Op::Lt => c == Less,
                Op::Eq => c == Equal,
            }
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The `requires` of an entry on its own terms: a component name needs a version range, any other name (`abi`) an exact text.
pub fn check_requires(r: &BTreeMap<String, String>, own: &str) -> Result<(), String> {
    if r.len() > MAX_REQUIRES {
        return Err(format!("requires lists at most {MAX_REQUIRES} names"));
    }
    for (k, v) in r {
        if !valid_component_name(k) {
            return Err(format!("requires: {k:?} is not a name (lowercase letters, digits, dashes)"));
        }
        if k == own {
            return Err(format!("requires: {own} cannot require itself"));
        }
        if COMPONENTS.contains(&k.as_str()) {
            Range::parse(v).map_err(|e| format!("requires {k}: {e}"))?;
        } else if v.trim().is_empty() || v.len() > 40 || v.chars().any(char::is_control) {
            return Err(format!("requires {k}: give the exact value (1 to 40 characters)"));
        }
    }
    Ok(())
}

/// An entry on its own terms (the structure, not the blobs).
pub fn check_entry(e: &Entry) -> Result<(), String> {
    if !valid_component_name(&e.component) {
        return Err(format!("{:?} is not a component name", e.component));
    }
    if !valid_version(&e.version) {
        return Err(format!("{} {:?}: not a version (digits and dots, optional -pre)", e.component, e.version));
    }
    if !valid_channel(&e.channel) {
        return Err(format!("{} {}: the channel is 2 to 24 lowercase letters, digits, dashes", e.component, e.version));
    }
    if !is_sha(&e.sha256) || e.size == 0 {
        return Err(format!("{} {}: needs the SHA-256 and the size of its artifact", e.component, e.version));
    }
    if e.built_from.chars().count() > 120 || e.built_from.chars().any(char::is_control) {
        return Err("built_from is at most 120 characters".into());
    }
    if !e.install_path.is_empty() && !valid_install_path(&e.install_path) {
        return Err(format!("{} {}: install_path {:?} is not a path an update may write", e.component, e.version, e.install_path));
    }
    if e.delta.len() > MAX_DELTAS {
        return Err(format!("at most {MAX_DELTAS} deltas per version"));
    }
    let mut seen = BTreeSet::new();
    for d in &e.delta {
        if !is_sha(&d.from_sha256) || !is_sha(&d.sha256) || d.size == 0 {
            return Err(format!("{} {}: a delta needs from_sha256, sha256 and size", e.component, e.version));
        }
        if d.from_sha256 == e.sha256 || !seen.insert(d.from_sha256.clone()) {
            return Err(format!("{} {}: a delta from itself or two deltas from the same version", e.component, e.version));
        }
    }
    check_requires(&e.requires, &e.component)
}

// ------------------------------------------------------------------------------------------------ the document

/// The document the store serves: only live entries, plus the withdrawn ones so a device that saw them refuses them.
pub fn build_doc(store_id: &str, serial: u64, issued: i64, records: &[Record], key: &SigningKey) -> String {
    let mut live: Vec<&Entry> = records.iter().filter(|r| r.yanked.is_none() && !r.purged).map(|r| &r.entry).collect();
    live.sort_by(|a, b| a.component.cmp(&b.component).then(a.channel.cmp(&b.channel)).then_with(|| compare_versions(&a.version, &b.version)));
    let yanked: Vec<Yank> = records.iter().filter_map(|r| r.yanked.as_ref().map(|why| Yank { component: r.entry.component.clone(), version: r.entry.version.clone(), sha256: r.entry.sha256.clone(), channel: r.entry.channel.clone(), reason: why.clone() })).collect();
    let mut doc = json!({"v": 1, "id": store_id, "serial": serial, "issued": issued, "components": live, "yanked": yanked, "key": public_key_hex(key)});
    let sig = sign_doc(PURPOSE, &doc, key);
    doc["sig"] = Value::String(sig);
    doc.to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub id: String,
    pub serial: u64,
    pub issued: i64,
    pub entries: Vec<Entry>,
    pub yanked: Vec<Yank>,
    pub key: String,
}

impl Doc {
    pub fn is_yanked(&self, e: &Entry) -> bool {
        self.yanked.iter().any(|y| y.component == e.component && y.sha256 == e.sha256 && y.channel == e.channel)
    }

    /// The newest live entry of `component` in `channel`.
    pub fn latest(&self, component: &str, channel: &str) -> Option<&Entry> {
        self.entries.iter().filter(|e| e.component == component && e.channel == channel && !self.is_yanked(e)).max_by(|a, b| compare_versions(&a.version, &b.version))
    }
}

/// Device side: accept the document only when it is signed by `pin` (the key pinned for that store); every entry is checked too.
pub fn verify(body: &[u8], pin: &str) -> Result<Doc, String> {
    const REFUSED: &str = "the store's component list is not signed by the key you confirmed: it is refused";
    if body.len() > MAX_DOC {
        return Err("the store's component list is too large: refused".into());
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| "the answer is not a component list".to_string())?;
    let (key, sig) = (v["key"].as_str().unwrap_or(""), v["sig"].as_str().unwrap_or(""));
    if pin.is_empty() || !key.eq_ignore_ascii_case(pin) || sig.is_empty() || !verify_doc(PURPOSE, &v, key, sig) {
        return Err(REFUSED.into());
    }
    let entries: Vec<Entry> = serde_json::from_value(v["components"].clone()).map_err(|e| format!("the component list is damaged: {e}"))?;
    let yanked: Vec<Yank> = serde_json::from_value(v["yanked"].clone()).unwrap_or_default();
    for e in &entries {
        check_entry(e).map_err(|e| format!("the component list has a bad entry: {e}"))?;
    }
    Ok(Doc { id: v["id"].as_str().unwrap_or("").to_string(), serial: v["serial"].as_u64().unwrap_or(0), issued: v["issued"].as_i64().unwrap_or(0), entries, yanked, key: key.to_ascii_lowercase() })
}

/// `<store>/v1/components.json` for a catalog address (`.../catalog.json` or a base address).
pub fn url_for_catalog(catalog_location: &str) -> String {
    let t = catalog_location.trim().trim_end_matches('/');
    format!("{}{PATH}", t.strip_suffix("/catalog.json").unwrap_or(t))
}

pub fn base_of(catalog_location: &str) -> String {
    let t = catalog_location.trim().trim_end_matches('/');
    t.strip_suffix("/catalog.json").unwrap_or(t).to_string()
}

// ------------------------------------------------------------------------------------------------ retention

/// What a cleanup would do for a policy ("keep the newest `keep` full artifacts per component and channel, and the deltas between them").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionPlan {
    /// `(component, channel, version)` of live entries that fall out of the newest `keep`.
    pub drop_entries: Vec<(String, String, String)>,
    /// `(component, version, delta sha256)`: deltas of entries that stay, from a version that goes.
    pub drop_deltas: Vec<(String, String, String)>,
    /// Withdrawn entries whose files are removed (the notice stays).
    pub purge_yanked: Vec<(String, String, String)>,
    /// Blobs no remaining entry or delta refers to.
    pub remove_blobs: Vec<String>,
    /// Bytes those blobs hold.
    pub bytes: u64,
}

impl RetentionPlan {
    pub fn is_empty(&self) -> bool {
        self.drop_entries.is_empty() && self.drop_deltas.is_empty() && self.purge_yanked.is_empty() && self.remove_blobs.is_empty()
    }
}

/// Pure: what to remove for `keep` newest live artifacts per component and channel. Withdrawn entries do not count towards `keep`; their
/// files go (the withdrawal notice stays so devices keep refusing the version). A blob also used by a remaining entry (a promotion to another
/// channel is a copy of the entry) is never removed.
pub fn plan_retention(records: &[Record], keep: usize) -> RetentionPlan {
    let keep = keep.max(1);
    let mut plan = RetentionPlan::default();
    let mut groups: BTreeMap<(&str, &str), Vec<&Record>> = BTreeMap::new();
    for r in records.iter().filter(|r| r.yanked.is_none() && !r.purged) {
        groups.entry((r.entry.component.as_str(), r.entry.channel.as_str())).or_default().push(r);
    }
    let mut kept: Vec<&Record> = vec![];
    for (_, mut g) in groups {
        g.sort_by(|a, b| compare_versions(&b.entry.version, &a.entry.version));
        for (i, r) in g.into_iter().enumerate() {
            if i < keep {
                kept.push(r);
            } else {
                plan.drop_entries.push((r.entry.component.clone(), r.entry.channel.clone(), r.entry.version.clone()));
            }
        }
    }
    for r in records.iter().filter(|r| r.yanked.is_some() && !r.purged) {
        plan.purge_yanked.push((r.entry.component.clone(), r.entry.channel.clone(), r.entry.version.clone()));
    }
    let mut sizes: BTreeMap<&str, u64> = BTreeMap::new();
    let mut before: BTreeSet<&str> = BTreeSet::new();
    for r in records.iter().filter(|r| !r.purged) {
        sizes.insert(&r.entry.sha256, r.entry.size);
        before.insert(&r.entry.sha256);
        for d in &r.entry.delta {
            sizes.insert(&d.sha256, d.size);
            before.insert(&d.sha256);
        }
    }
    let mut after: BTreeSet<&str> = BTreeSet::new();
    for r in &kept {
        after.insert(&r.entry.sha256);
    }
    for r in &kept {
        for d in &r.entry.delta {
            // a delta stays when the version it starts from stays (in any channel)
            if kept.iter().any(|k| k.entry.component == r.entry.component && k.entry.sha256 == d.from_sha256) {
                after.insert(&d.sha256);
            } else {
                plan.drop_deltas.push((r.entry.component.clone(), r.entry.version.clone(), d.sha256.clone()));
            }
        }
    }
    for h in before.difference(&after) {
        plan.remove_blobs.push(h.to_string());
        plan.bytes += sizes.get(h).copied().unwrap_or(0);
    }
    plan
}

/// Apply a plan to the records (the files are removed by the caller). Returns the records after.
pub fn apply_retention(records: &[Record], plan: &RetentionPlan) -> Vec<Record> {
    let mut out = vec![];
    for r in records {
        let id = (r.entry.component.clone(), r.entry.channel.clone(), r.entry.version.clone());
        if plan.drop_entries.contains(&id) && r.yanked.is_none() {
            continue;
        }
        let mut r = r.clone();
        if plan.purge_yanked.contains(&id) && r.yanked.is_some() {
            r.purged = true;
            r.entry.delta.clear();
        }
        r.entry.delta.retain(|d| !plan.drop_deltas.iter().any(|x| x.0 == r.entry.component && x.1 == r.entry.version && x.2 == d.sha256));
        out.push(r);
    }
    out
}
