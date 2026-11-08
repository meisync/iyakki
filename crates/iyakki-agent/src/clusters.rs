//! The cluster registry: every cluster a person can work against, one list per account (docs/K3S-DEV.md, "Clusters").
//!
//! * **Entries.** `local` is the managed k3s IYAKKI has always had (`k3sdev`, same folder, ports and names). Extra local clusters
//!   are optional and named: each has its own folder, API port, kubelet ports, node name and address ranges
//!   (`k3sdev::Instance::extra`) and is started and stopped on its own by the same supervisor code. Remote entries are
//!   kubeconfig profiles of the kinds `remote`, `tunnel` and `cloud-cli`, with a kubeconfig path or a vault entry (`uvault:<name>`).
//! * **Active cluster.** One per account (`active`), with an optional override per workspace (`workspace_active`). Everything that
//!   needs "the cluster" (K8S tile, the IDE's K8s panel, Setup items, the assistant's cluster tools, the GPU profile) asks
//!   [`Registry::effective`] and shows the name with [`ClusterEntry::phrase`]. Choosing one never moves or deletes anything;
//!   [`choice_for`] gives the `devcluster::Choice`, so code that still reads the old "remembered choice" follows it.
//! * **Migration.** [`Registry::load`] builds the registry from the old files (`devcluster-<user>.json` the remembered choice,
//!   `devcluster-vault-<user>.json` the vault profiles) when `clusters-<user>.json` does not exist yet. Old files are never edited
//!   or removed by loading; [`Registry::save`] writes only the new file (and [`switch`] mirrors the choice into the old one).
//! * **Limits.** At most [`DEFAULT_MAX_LOCAL`] local clusters (`advanced` raises it to [`ADVANCED_MAX_LOCAL`]), and
//!   [`local_start_gate`] refuses a second running local k3s when the memory is not there, saying why.
//!
//! Nothing here starts a program except through [`Fleet`] (the k3s supervisor, one per local cluster) and [`probe`] (read-only
//! `kubectl` through the `Runner`), so tests use fakes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_sec::policy::Role;
use serde::{Deserialize, Serialize};

use crate::clis::CliManager;
use crate::devcluster::Choice;
use crate::k3sdev::{self, Instance, K3sDev, Svc};
use crate::wsexec::Runner;

/// The id of the managed local cluster that always exists.
pub const LOCAL_ID: &str = "local";
/// Local clusters (the built-in one included) a person may have by default.
pub const DEFAULT_MAX_LOCAL: usize = 2;
/// With "Advanced" on.
pub const ADVANCED_MAX_LOCAL: usize = 4;
/// Memory one more running k3s needs: its own (about 2 GiB) plus the machine's usual reserve (`memgate::MIN_FREE`, 512 MiB).
pub const LOCAL_K3S_NEED: u64 = 2 * (1 << 30) + 512 * (1 << 20);
/// A status older than this is "stale".
pub const STALE_AFTER_SECS: i64 = 180;

fn now() -> i64 {
    crate::clock::now_unix()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "remote")]
    Remote,
    #[serde(rename = "tunnel")]
    Tunnel,
    #[serde(rename = "cloud-cli")]
    CloudCli,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Local => "local",
            Kind::Remote => "remote",
            Kind::Tunnel => "tunnel",
            Kind::CloudCli => "cloud-cli",
        }
    }
    pub fn from_word(s: &str) -> Option<Kind> {
        match s.trim().to_lowercase().as_str() {
            "local" => Some(Kind::Local),
            "remote" => Some(Kind::Remote),
            "tunnel" => Some(Kind::Tunnel),
            "cloud-cli" | "cloud" => Some(Kind::CloudCli),
            _ => None,
        }
    }
    pub fn is_local(self) -> bool {
        self == Kind::Local
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterEntry {
    /// Stable, short, lowercase: `local`, `gpu-box`. Never changes when the entry is renamed.
    pub id: String,
    /// What the person sees.
    pub name: String,
    pub kind: Kind,
    /// A kubeconfig path or a vault reference (`uvault:<name>`); empty for local clusters (their own folder).
    pub kubeconfig: String,
    pub context: String,
    pub namespace: String,
    /// Extra local clusters: the slot that fixes the ports and address ranges (1, 2, ...). 0 for `local` and remote entries.
    pub slot: u16,
    /// Extra local clusters: start with IYAKKI. (`local` uses its own `state.json` setting.)
    pub autostart: bool,
    /// Users (not only administrators and operators) may see and switch to it.
    pub visible_to_users: bool,
    pub created_at: i64,
}

impl Default for ClusterEntry {
    fn default() -> ClusterEntry {
        ClusterEntry { id: String::new(), name: String::new(), kind: Kind::Remote, kubeconfig: String::new(), context: String::new(), namespace: String::new(), slot: 0, autostart: false, visible_to_users: true, created_at: 0 }
    }
}

impl ClusterEntry {
    pub fn local_primary() -> ClusterEntry {
        ClusterEntry { id: LOCAL_ID.into(), name: "This machine".into(), kind: Kind::Local, ..ClusterEntry::default() }
    }

    pub fn is_primary(&self) -> bool {
        self.id == LOCAL_ID
    }

    /// The managed-instance description of a local entry.
    pub fn instance(&self) -> Option<Instance> {
        if self.kind != Kind::Local {
            return None;
        }
        Some(if self.is_primary() { Instance::primary() } else { Instance::extra(&self.id, self.slot.max(1)) })
    }

    /// The kubeconfig a program is given: the managed folder's file for a local cluster, the reference for a remote one.
    pub fn kubeconfig_for(&self, data_dir: &Path) -> String {
        match self.instance() {
            Some(i) => k3sdev::Layout::for_instance(data_dir, &i).kubeconfig().display().to_string(),
            None => self.kubeconfig.clone(),
        }
    }

    pub fn in_vault(&self) -> bool {
        crate::kubeprofiles::is_vault_ref(&self.kubeconfig)
    }

    /// `cluster "gpu-box" (remote)` for confirmation texts.
    pub fn phrase(&self) -> String {
        format!("cluster \"{}\" ({})", self.name, self.kind.word())
    }

    /// A production-looking name: the assistant only reads (same rule as `devcluster::read_only_reason`).
    pub fn read_only_reason(&self) -> Option<String> {
        let t = format!("{} {} {}", self.name, self.context, self.id).to_lowercase();
        (t.contains("prod") && !t.contains("nonprod") && !t.contains("non-prod")).then(|| format!("the cluster \"{}\" looks like production, so the assistant only reads from it; deploy to a development cluster (switch cluster, or Settings \u{2192} Clusters)", self.name))
    }
}

/// `Deploy Lemonade to cluster "gpu-box" (remote)`: the text every action that changes a cluster shows.
pub fn confirm_text(action: &str, to: &ClusterEntry) -> String {
    format!("{action} to {}", to.phrase())
}

/// `Start cluster "gpu-box" (remote)`: for actions on the cluster itself.
pub fn confirm_text_on(action: &str, on: &ClusterEntry) -> String {
    format!("{action} {}", on.phrase())
}

/// `Remove Registry from cluster "gpu-box" (remote)`.
pub fn confirm_text_from(action: &str, from: &ClusterEntry) -> String {
    format!("{action} from {}", from.phrase())
}

// ------------------------------------------------------------------------------------------------ roles

/// Administrators and operators add, remove, rename, start and stop.
pub fn can_manage(role: Role) -> bool {
    matches!(role, Role::Admin | Role::Operator)
}

/// Users may switch among the clusters they can see; guests may not.
pub fn can_switch(role: Role) -> bool {
    !matches!(role, Role::Guest)
}

/// The sentence for a refused change (`None`: allowed).
pub fn manage_refusal(role: Role) -> Option<String> {
    (!can_manage(role)).then(|| format!("only an administrator or operator can change the clusters (you are {})", role.name()))
}

// ------------------------------------------------------------------------------------------------ the registry

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub version: u32,
    pub clusters: Vec<ClusterEntry>,
    pub active: String,
    /// Workspace name to cluster id (the AI IDE's per-workspace choice).
    pub workspace_active: BTreeMap<String, String>,
    /// "Advanced": raises the local maximum.
    pub advanced: bool,
}

impl Default for Registry {
    fn default() -> Registry {
        Registry { version: 1, clusters: vec![ClusterEntry::local_primary()], active: LOCAL_ID.into(), workspace_active: BTreeMap::new(), advanced: false }
    }
}

fn safe_user(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

fn file_of(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("clusters-{}.json", safe_user(user)))
}

/// A lowercase id from a name: letters and digits, single dashes, up to 16 characters.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s: String = s.chars().take(16).collect();
    s.trim_end_matches('-').to_string()
}

pub fn valid_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 || !n.chars().all(|c| c.is_ascii_alphanumeric() || " -_.".contains(c)) {
        return Err("a cluster name is letters, digits, spaces and - _ . (up to 40)".into());
    }
    Ok(())
}

impl Registry {
    /// The person's registry. Without a saved one it is built from the old settings (nothing is written).
    pub fn load(dir: &Path, user: &str) -> Registry {
        if let Some(r) = std::fs::read_to_string(file_of(dir, user)).ok().and_then(|t| serde_json::from_str::<Registry>(&t).ok()) {
            return r.repaired();
        }
        Registry::migrate(dir, user)
    }

    /// The registry, with vault profiles added on the old Dev cluster page after the registry was saved appended as remote entries (a
    /// saved registry is saved again when that adds one). Nothing else changes; an unsaved registry is migrated afresh each time.
    pub fn load_synced(dir: &Path, user: &str) -> Registry {
        let mut r = Registry::load(dir, user);
        if !Registry::saved(dir, user) {
            return r;
        }
        let mut added = false;
        for p in crate::kubeprofiles::vault_load(dir, user) {
            if !r.clusters.iter().any(|c| c.kubeconfig == p.reference) && r.add_remote(&p.name, Kind::Remote, &p.reference, "", "").is_ok() {
                added = true;
            }
        }
        if added {
            let _ = r.save(dir, user);
        }
        r
    }

    /// Whether the new file exists (false: the registry came from migration and is not saved yet).
    pub fn saved(dir: &Path, user: &str) -> bool {
        file_of(dir, user).is_file()
    }

    /// Build from the old files: `local` first, one remote entry per vault profile, and the remembered choice (a path that is not
    /// a managed cluster becomes an entry, and the active one).
    pub fn migrate(dir: &Path, user: &str) -> Registry {
        let mut r = Registry::default();
        for p in crate::kubeprofiles::vault_load(dir, user) {
            let _ = r.add_remote_inner(&p.name, Kind::Remote, &p.reference, "", "");
        }
        if let Some(c) = Choice::load(dir, user) {
            let found = r.clusters.iter().find(|e| !e.kubeconfig.is_empty() && e.kubeconfig == c.kubeconfig).map(|e| e.id.clone());
            let id = found.or_else(|| {
                let machine = crate::devcluster::machine_kubeconfig().map(|p| p.display().to_string());
                if machine.as_deref() == Some(c.kubeconfig.as_str()) || c.kubeconfig.starts_with("/etc/rancher/k3s/") || c.kubeconfig.ends_with("/k3s-dev/kubeconfig") {
                    return Some(LOCAL_ID.to_string());
                }
                let stem = Path::new(&c.kubeconfig).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let name = if stem.is_empty() || stem == "config" { "remembered".to_string() } else { stem };
                let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || " -_.".contains(*c)).collect();
                let name = if name.is_empty() { "remembered".to_string() } else { name };
                r.add_remote_inner(&name, Kind::Remote, &c.kubeconfig, &c.context, "").ok().map(|e| e.id)
            });
            if let Some(id) = id {
                r.active = id;
            }
        }
        r
    }

    /// Fix what a hand-edited or older file can get wrong: `local` present and first, ids unique, slots set, active and overrides
    /// pointing at entries that exist.
    fn repaired(mut self) -> Registry {
        if !self.clusters.iter().any(|c| c.id == LOCAL_ID) {
            self.clusters.insert(0, ClusterEntry::local_primary());
        }
        let mut seen: Vec<String> = Vec::new();
        self.clusters.retain(|c| {
            let ok = !c.id.is_empty() && !seen.contains(&c.id);
            if ok {
                seen.push(c.id.clone());
            }
            ok
        });
        if let Some(i) = self.clusters.iter().position(|c| c.id == LOCAL_ID) {
            let l = self.clusters.remove(i);
            self.clusters.insert(0, ClusterEntry { kind: Kind::Local, slot: 0, ..l });
        }
        let mut used: Vec<u16> = self.clusters.iter().filter(|c| c.kind.is_local() && !c.is_primary() && c.slot > 0).map(|c| c.slot).collect();
        for c in self.clusters.iter_mut().filter(|c| c.kind.is_local() && !c.is_primary() && c.slot == 0) {
            let s = (1..=60u16).find(|s| !used.contains(s)).unwrap_or(60);
            used.push(s);
            c.slot = s;
        }
        if !self.clusters.iter().any(|c| c.id == self.active) {
            self.active = LOCAL_ID.into();
        }
        let ids: Vec<String> = self.clusters.iter().map(|c| c.id.clone()).collect();
        self.workspace_active.retain(|_, v| ids.contains(v));
        self
    }

    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("clusters-{}.json.tmp", safe_user(user)));
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, file_of(dir, user)).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&ClusterEntry> {
        self.clusters.iter().find(|c| c.id == id)
    }

    pub fn find_name(&self, name: &str) -> Option<&ClusterEntry> {
        let n = name.trim().to_lowercase();
        self.clusters.iter().find(|c| c.name.to_lowercase() == n || c.id == n)
    }

    /// The account's active cluster (`local` when the saved one is gone).
    pub fn active_entry(&self) -> ClusterEntry {
        self.get(&self.active).or_else(|| self.get(LOCAL_ID)).cloned().unwrap_or_else(ClusterEntry::local_primary)
    }

    /// The cluster for a workspace: its override when it has one, else the account's active one.
    pub fn effective(&self, workspace: &str) -> ClusterEntry {
        self.workspace_active.get(workspace).and_then(|id| self.get(id)).cloned().unwrap_or_else(|| self.active_entry())
    }

    /// More than one cluster: only then do the switchers show.
    pub fn has_choice(&self) -> bool {
        self.clusters.len() > 1
    }

    /// What this role may see (guests none; users the entries marked visible).
    pub fn visible_to(&self, role: Role) -> Vec<&ClusterEntry> {
        match role {
            Role::Guest => vec![],
            Role::User => self.clusters.iter().filter(|c| c.visible_to_users || c.id == LOCAL_ID).collect(),
            _ => self.clusters.iter().collect(),
        }
    }

    pub fn local_count(&self) -> usize {
        self.clusters.iter().filter(|c| c.kind.is_local()).count()
    }

    pub fn max_local(&self) -> usize {
        if self.advanced { ADVANCED_MAX_LOCAL } else { DEFAULT_MAX_LOCAL }
    }

    fn unique_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "cluster".to_string() } else { base.to_string() };
        let base = if base == LOCAL_ID { "local-2".to_string() } else { base };
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            let suffix = format!("-{n}");
            let keep = 16usize.saturating_sub(suffix.len());
            id = format!("{}{}", base.chars().take(keep).collect::<String>().trim_end_matches('-'), suffix);
            n += 1;
        }
        id
    }

    fn name_taken(&self, name: &str) -> bool {
        let n = name.trim().to_lowercase();
        self.clusters.iter().any(|c| c.name.to_lowercase() == n)
    }

    /// A new extra local cluster (not installed yet: set it up from the Clusters page). Refused over the maximum.
    pub fn add_local(&mut self, name: &str) -> Result<ClusterEntry, String> {
        self.add_local_avoiding(name, &Taken::default())
    }

    /// [`Registry::add_local`] that also avoids the ids and slots the other accounts' extra local clusters use: local clusters are a
    /// resource of the machine (folder, ports), so two accounts must not get the same slot or id (see [`taken_by_others`]).
    pub fn add_local_avoiding(&mut self, name: &str, taken: &Taken) -> Result<ClusterEntry, String> {
        valid_name(name)?;
        if self.name_taken(name) {
            return Err(format!("a cluster named \"{}\" exists already", name.trim()));
        }
        if self.local_count() >= self.max_local() {
            return Err(format!("this account may have {} local clusters (the built-in one included); {}Every local cluster is a full k3s: about 2 GB of memory and one or more CPU cores each.", self.max_local(), if self.advanced { "remove one first. " } else { "remove one, or switch on Advanced to raise the limit. " }));
        }
        let mut id = slug(name);
        if !k3sdev::valid_id(&id) {
            id = format!("c-{id}");
            if !k3sdev::valid_id(&id) {
                id = "extra".into();
            }
        }
        let mut id = self.unique_id(&id);
        let mut n = 2;
        while taken.ids.contains(&id) {
            let suffix = format!("-{n}");
            id = format!("{}{}", id.chars().take(16usize.saturating_sub(suffix.len())).collect::<String>().trim_end_matches('-'), suffix);
            n += 1;
        }
        let slot = (1..=60u16).find(|s| !taken.slots.contains(s) && !self.clusters.iter().any(|c| c.kind.is_local() && c.slot == *s)).ok_or("no free slot for another local cluster")?;
        let e = ClusterEntry { id, name: name.trim().to_string(), kind: Kind::Local, slot, created_at: now(), ..ClusterEntry::default() };
        self.clusters.push(e.clone());
        Ok(e)
    }

    /// A remote cluster profile. The kubeconfig is a path or a vault entry; nothing is read or tested here (see [`probe`]).
    pub fn add_remote(&mut self, name: &str, kind: Kind, kubeconfig: &str, context: &str, namespace: &str) -> Result<ClusterEntry, String> {
        valid_name(name)?;
        if self.name_taken(name) {
            return Err(format!("a cluster named \"{}\" exists already", name.trim()));
        }
        self.add_remote_inner(name, kind, kubeconfig, context, namespace)
    }

    fn add_remote_inner(&mut self, name: &str, kind: Kind, kubeconfig: &str, context: &str, namespace: &str) -> Result<ClusterEntry, String> {
        if kind.is_local() {
            return Err("a remote cluster is of kind remote, tunnel or cloud-cli".into());
        }
        let kc = kubeconfig.trim();
        if kc.is_empty() {
            return Err("choose a kubeconfig file or a vault entry".into());
        }
        let reference = if let Some(entry) = kc.strip_prefix("uvault:").or_else(|| kc.strip_prefix("vault:")) {
            if !crate::vault::valid_secret_name(entry) {
                return Err("a vault entry name is lowercase letters, digits, . _ - (up to 64)".into());
            }
            if kc.starts_with("vault:") { format!("vault:{entry}") } else { format!("uvault:{entry}") }
        } else {
            kc.to_string()
        };
        let id = self.unique_id(&slug(name));
        let e = ClusterEntry { id, name: name.trim().to_string(), kind, kubeconfig: reference, context: context.trim().into(), namespace: namespace.trim().into(), created_at: now(), ..ClusterEntry::default() };
        self.clusters.push(e.clone());
        Ok(e)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        valid_name(name)?;
        if self.clusters.iter().any(|c| c.id != id && c.name.to_lowercase() == name.trim().to_lowercase()) {
            return Err(format!("a cluster named \"{}\" exists already", name.trim()));
        }
        let c = self.clusters.iter_mut().find(|c| c.id == id).ok_or("no such cluster")?;
        c.name = name.trim().to_string();
        Ok(())
    }

    pub fn set_autostart(&mut self, id: &str, on: bool) -> Result<(), String> {
        if id == LOCAL_ID {
            return Err("the built-in local cluster keeps its own Autostart setting (Settings \u{2192} Clusters \u{2192} This machine)".into());
        }
        let c = self.clusters.iter_mut().find(|c| c.id == id).ok_or("no such cluster")?;
        if !c.kind.is_local() {
            return Err("only a local cluster can start with IYAKKI; a remote cluster is not ours to start".into());
        }
        c.autostart = on;
        Ok(())
    }

    pub fn set_visible_to_users(&mut self, id: &str, on: bool) -> Result<(), String> {
        let c = self.clusters.iter_mut().find(|c| c.id == id).ok_or("no such cluster")?;
        c.visible_to_users = on || c.id == LOCAL_ID;
        Ok(())
    }

    /// Forget an entry. The built-in `local` cannot be forgotten (Uninstall removes its cluster). Removing deletes nothing here: the
    /// caller removes an extra cluster's folder (or keeps it) and, for a vault profile, the vault entry when asked. Returns the entry.
    pub fn remove(&mut self, id: &str) -> Result<ClusterEntry, String> {
        if id == LOCAL_ID {
            return Err("the built-in local cluster cannot be removed from the list; use Uninstall on its card to remove the cluster itself".into());
        }
        let i = self.clusters.iter().position(|c| c.id == id).ok_or("no such cluster")?;
        let e = self.clusters.remove(i);
        if self.active == id {
            self.active = LOCAL_ID.into();
        }
        self.workspace_active.retain(|_, v| v != id);
        Ok(e)
    }

    /// Make a cluster the account's active one. Nothing is moved or deleted.
    pub fn set_active(&mut self, id: &str) -> Result<(), String> {
        if self.get(id).is_none() {
            return Err("no such cluster".into());
        }
        self.active = id.into();
        Ok(())
    }

    /// The AI IDE's per-workspace choice (`None` removes it: the workspace follows the active cluster again).
    pub fn set_workspace(&mut self, workspace: &str, id: Option<&str>) -> Result<(), String> {
        let ws = workspace.trim();
        if ws.is_empty() {
            return Err("no workspace".into());
        }
        match id {
            Some(id) => {
                if self.get(id).is_none() {
                    return Err("no such cluster".into());
                }
                self.workspace_active.insert(ws.to_string(), id.to_string());
            }
            None => {
                self.workspace_active.remove(ws);
            }
        }
        Ok(())
    }
}

/// The ids and slots of extra local clusters other accounts have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub ids: Vec<String>,
    pub slots: Vec<u16>,
}

/// Every account's saved registry in `dir`: `(user file stem, registry)`.
fn all_registries(dir: &Path) -> Vec<(String, Registry)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<(String, Registry)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let user = n.strip_prefix("clusters-")?.strip_suffix(".json")?.to_string();
            let r = serde_json::from_str::<Registry>(&std::fs::read_to_string(e.path()).ok()?).ok()?.repaired();
            Some((user, r))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// What the other accounts' extra local clusters use (the account itself is left out).
pub fn taken_by_others(dir: &Path, user: &str) -> Taken {
    let me = safe_user(user);
    let mut t = Taken::default();
    for (u, r) in all_registries(dir) {
        if u == me {
            continue;
        }
        for c in r.clusters.iter().filter(|c| c.kind.is_local() && !c.is_primary()) {
            t.ids.push(c.id.clone());
            t.slots.push(c.slot);
        }
    }
    t
}

/// Add an extra local cluster for `user`, avoiding the other accounts' ids and slots, and save.
pub fn add_local_saved(dir: &Path, user: &str, role: Role, name: &str) -> Result<ClusterEntry, String> {
    if let Some(r) = manage_refusal(role) {
        return Err(r);
    }
    let mut r = Registry::load(dir, user);
    let e = r.add_local_avoiding(name, &taken_by_others(dir, user))?;
    r.save(dir, user)?;
    Ok(e)
}

/// Role-checked switch of the account's active cluster: saves, and mirrors the choice into the old remembered-choice file (so code
/// that still reads it follows). Users may switch among what they see; guests may not.
pub fn switch(dir: &Path, data_dir: &Path, user: &str, role: Role, id: &str) -> Result<ClusterEntry, String> {
    if !can_switch(role) {
        return Err("guests cannot switch clusters".into());
    }
    let mut r = Registry::load(dir, user);
    if !r.visible_to(role).iter().any(|c| c.id == id) {
        return Err("that cluster is not available to your account".into());
    }
    r.set_active(id)?;
    r.save(dir, user)?;
    let e = r.active_entry();
    mirror_choice(dir, data_dir, user, &e);
    Ok(e)
}

/// Role-checked switch of one workspace's cluster (`None`: follow the active cluster again).
pub fn switch_workspace(dir: &Path, user: &str, role: Role, workspace: &str, id: Option<&str>) -> Result<ClusterEntry, String> {
    if !can_switch(role) {
        return Err("guests cannot switch clusters".into());
    }
    let mut r = Registry::load(dir, user);
    if let Some(i) = id {
        if !r.visible_to(role).iter().any(|c| c.id == i) {
            return Err("that cluster is not available to your account".into());
        }
    }
    r.set_workspace(workspace, id)?;
    r.save(dir, user)?;
    Ok(r.effective(workspace))
}

/// What `devcluster::resolve` is given as the remembered choice for a workspace: with several clusters, the one that is effective for
/// the workspace (its override, else the account's active cluster); with one cluster exactly the old remembered choice, as before.
pub fn remembered_for(dir: &Path, user: &str, workspace: &str) -> Option<Choice> {
    let r = Registry::load(dir, user);
    if !r.has_choice() {
        return Choice::load(dir, user);
    }
    Some(choice_for(&r.effective(workspace), dir))
}

/// The cluster a workspace uses, for texts and audit lines (`cluster "gpu-box" (remote)`). Empty with one cluster: the caller then names
/// the cluster it resolved itself (a workspace's own `project.json` kubeconfig can differ from the registry's `local`).
pub fn phrase_for(dir: &Path, user: &str, workspace: &str) -> String {
    let r = Registry::load(dir, user);
    if r.has_choice() { r.effective(workspace).phrase() } else { String::new() }
}

/// Write the old remembered choice for an entry (`devcluster-<user>.json`).
pub fn mirror_choice(dir: &Path, data_dir: &Path, user: &str, e: &ClusterEntry) {
    let _ = choice_for(e, data_dir).save(dir, user);
}

/// The `Choice` (kubeconfig and context) of an entry.
pub fn choice_for(e: &ClusterEntry, data_dir: &Path) -> Choice {
    Choice { kubeconfig: e.kubeconfig_for(data_dir), context: e.context.clone() }
}

/// Forget a remote profile. Its vault entry (and the old vault-profile row) goes only when `delete_vault_entry` is set; a local
/// entry's folder is never touched here.
pub fn forget_remote(dir: &Path, user: &str, reg: &mut Registry, id: &str, delete_vault_entry: bool, delete_secret: &mut dyn FnMut(&str) -> Result<(), String>) -> Result<ClusterEntry, String> {
    let e = reg.get(id).cloned().ok_or("no such cluster")?;
    if e.kind.is_local() {
        return Err("this is a local cluster: remove it with the local removal (it asks about its data)".into());
    }
    reg.remove(id)?;
    if delete_vault_entry && e.in_vault() {
        let _ = crate::kubeprofiles::vault_remove(dir, user, &e.name);
        delete_secret(&e.kubeconfig)?;
    }
    Ok(e)
}

// ------------------------------------------------------------------------------------------------ memory gate

/// May another local k3s start? `running_other` are the local clusters already starting or running, `mem_available` is
/// `MemAvailable` in bytes (`None`: unknown). The first local cluster is never refused here (today's behaviour); a second
/// needs [`LOCAL_K3S_NEED`] free.
pub fn local_start_gate(running_other: usize, mem_available: Option<u64>) -> Result<(), String> {
    if running_other == 0 {
        return Ok(());
    }
    let gib = |b: u64| format!("{:.1} GB", b as f64 / (1u64 << 30) as f64);
    match mem_available {
        None => Err("another local cluster is running and this machine's free memory cannot be read, so a second k3s is not started (each one needs about 2 GB). Stop the other local cluster first.".into()),
        Some(m) if m < LOCAL_K3S_NEED => Err(format!("another local cluster is running and only {} of memory is free; a second k3s needs about {} free (its own 2 GB plus a reserve for the system). Stop the other local cluster first, or close programs.", gib(m), gib(LOCAL_K3S_NEED))),
        Some(_) => Ok(()),
    }
}

/// The warning shown when adding or starting a second local cluster.
pub const COST_WARNING: &str = "Every local cluster is a complete k3s: about 2 GB of memory and one or more CPU cores while it runs, and 3 GB of disk. Two on one machine are for trying things side by side; start the second only when you need it.";

/// The GPU summary of a cluster for the badge, from the AI hardware profile's cache of that cluster's nodes (`aiprofile_remote`).
/// Reads the cache only: it never runs anything and never blocks drawing. `None`: unknown, no GPU, or a cluster on this machine.
pub fn gpu_summary(cluster_id: &str) -> Option<String> {
    crate::aiprofile_remote::badge_of(&crate::aiprofile_remote::clusters_cache().get_key(cluster_id)?)
}

/// Look at the GPUs of the remote clusters whose last look is due (one read-only `kubectl get nodes -o json` each, with the same
/// back-off as the chosen cluster: 60 s after an answer, 30 s doubling to 10 min after a failure; a cluster that stops answering keeps
/// its last badge, marked 'last known'). Clusters on this machine are not asked (their GPUs are this machine's own). Returns how many
/// looks started. `background`: each look runs on its own thread; otherwise they run here (tests).
pub fn refresh_gpus(run: &Arc<dyn Runner>, entries: &[ClusterEntry], background: bool) -> usize {
    let cache = crate::aiprofile_remote::clusters_cache();
    let mut started = 0;
    for e in entries.iter().filter(|e| !e.kind.is_local()) {
        let at = now().max(0) as u64;
        if !cache.begin_key(&e.id, at) {
            continue;
        }
        started += 1;
        let (run, cache, e) = (run.clone(), cache.clone(), e.clone());
        let work = move || {
            let kc = crate::devcluster::Kc::new(run, &e.kubeconfig, &e.context);
            let o = kc.kubectl(&["get", "nodes", "-o", "json"], 6, 12);
            let t = now().max(0) as u64;
            if o.ok() {
                crate::aiprofile_remote::note_nodes(&cache, &e.id, &e.name, &o.stdout, t);
            } else {
                let why = o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or("no answer").trim().to_string();
                crate::aiprofile_remote::note_failure(&cache, &e.id, &e.name, &why, t);
            }
        };
        if background {
            std::thread::spawn(work);
        } else {
            work();
        }
    }
    started
}

// ------------------------------------------------------------------------------------------------ status

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Running,
    Stopped,
    Unreachable,
    Stale,
    Unknown,
}

impl Dot {
    pub fn word(self) -> &'static str {
        match self {
            Dot::Running => "running",
            Dot::Stopped => "stopped",
            Dot::Unreachable => "unreachable",
            Dot::Stale => "stale",
            Dot::Unknown => "not checked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub dot: Dot,
    pub version: String,
    pub nodes: usize,
    pub ready: usize,
    pub last_error: String,
    pub checked_at: i64,
    pub gpu: Option<String>,
}

impl Status {
    pub fn unknown() -> Status {
        Status { dot: Dot::Unknown, version: String::new(), nodes: 0, ready: 0, last_error: String::new(), checked_at: 0, gpu: None }
    }

    /// `running · v1.31.1+k3s1 · 1/1 nodes ready`
    pub fn line(&self) -> String {
        let mut s = self.dot.word().to_string();
        if !self.version.is_empty() {
            s.push_str(&format!(" \u{b7} {}", self.version));
        }
        if self.nodes > 0 {
            s.push_str(&format!(" \u{b7} {}/{} nodes ready", self.ready, self.nodes));
        }
        if !self.last_error.is_empty() {
            s.push_str(&format!(" \u{b7} {}", self.last_error));
        }
        s
    }

    /// A snapshot older than [`STALE_AFTER_SECS`] shows as stale (the last answer is kept).
    pub fn aged(mut self, at: i64) -> Status {
        if self.checked_at > 0 && at - self.checked_at > STALE_AFTER_SECS && matches!(self.dot, Dot::Running | Dot::Unreachable) {
            self.dot = Dot::Stale;
        }
        self
    }
}

/// Parse `kubectl get nodes -o json`: (nodes, ready, kubelet version of the first).
pub fn parse_nodes(json: &str) -> Option<(usize, usize, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = v.get("items")?.as_array()?;
    let ready = items.iter().filter(|n| n.pointer("/status/conditions").and_then(|c| c.as_array()).is_some_and(|c| c.iter().any(|x| x["type"] == "Ready" && x["status"] == "True"))).count();
    let ver = items.first().and_then(|n| n.pointer("/status/nodeInfo/kubeletVersion")).and_then(|s| s.as_str()).unwrap_or_default().to_string();
    Some((items.len(), ready, ver))
}

fn status_from(o: &crate::wsexec::Out, id: &str, name: &str) -> Status {
    let cache = crate::aiprofile_remote::clusters_cache();
    if o.ok() {
        crate::aiprofile_remote::note_nodes(&cache, id, name, &o.stdout, now().max(0) as u64);
    } else {
        let why = o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or("no answer").trim().to_string();
        crate::aiprofile_remote::note_failure(&cache, id, name, &why, now().max(0) as u64);
    }
    let mut st = Status { checked_at: now(), gpu: gpu_summary(id), ..Status::unknown() };
    if !o.ok() {
        st.dot = Dot::Unreachable;
        let msg = o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or("no answer").trim().to_string();
        st.last_error = msg.chars().take(160).collect();
        return st;
    }
    match parse_nodes(&o.stdout) {
        Some((n, r, v)) => {
            st.dot = Dot::Running;
            st.nodes = n;
            st.ready = r;
            st.version = v;
        }
        None => {
            st.dot = Dot::Unreachable;
            st.last_error = "the answer was not a node list".into();
        }
    }
    st
}

/// Test a remote cluster: a read-only `kubectl get nodes`. A vault reference goes through the runner's usual vault handling.
pub fn probe(run: &Arc<dyn Runner>, e: &ClusterEntry) -> Status {
    let kc = crate::devcluster::Kc::new(run.clone(), &e.kubeconfig, &e.context);
    let o = kc.kubectl(&["get", "nodes", "-o", "json"], 6, 12);
    status_from(&o, &e.id, &e.name)
}

// ------------------------------------------------------------------------------------------------ the local fleet

/// The managed local clusters of this process: the primary plus the extra ones, each a [`K3sDev`] with its own supervisor, start
/// gate (clock, network, requirements), watchdog and health.
pub struct Fleet {
    data_dir: PathBuf,
    clis: Arc<CliManager>,
    run: Arc<dyn Runner>,
    primary: Arc<K3sDev>,
    extras: Mutex<BTreeMap<String, Arc<K3sDev>>>,
    /// Tests: the free memory the gate sees (outer `None`: read `/proc/meminfo`).
    mem: Mutex<Option<Option<u64>>>,
}

impl Fleet {
    pub fn new(primary: Arc<K3sDev>, data_dir: &Path, clis: Arc<CliManager>, run: Arc<dyn Runner>) -> Fleet {
        Fleet { data_dir: data_dir.to_path_buf(), clis, run, primary, extras: Mutex::new(BTreeMap::new()), mem: Mutex::new(None) }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn assume_memory(&self, available: Option<u64>) {
        *self.mem.lock().unwrap_or_else(|e| e.into_inner()) = Some(available);
    }

    fn mem_available(&self) -> Option<u64> {
        if let Some(m) = *self.mem.lock().unwrap_or_else(|e| e.into_inner()) {
            return m;
        }
        crate::sysinfo::mem_available_bytes()
    }

    /// The supervisor of an entry (created on first use for an extra cluster). `None` for a remote entry.
    pub fn get(&self, e: &ClusterEntry) -> Option<Arc<K3sDev>> {
        let inst = e.instance()?;
        if inst.primary {
            return Some(self.primary.clone());
        }
        let mut m = self.extras.lock().unwrap_or_else(|x| x.into_inner());
        Some(
            m.entry(inst.id.clone())
                .or_insert_with(|| {
                    let d = K3sDev::new_instance(&self.data_dir, self.clis.clone(), self.run.clone(), inst);
                    d.inherit_hooks(&self.primary);
                    d
                })
                .clone(),
        )
    }

    /// Forget an extra supervisor (after removal).
    pub fn drop_instance(&self, id: &str) {
        self.extras.lock().unwrap_or_else(|x| x.into_inner()).remove(id);
    }

    fn busy(d: &K3sDev) -> bool {
        matches!(d.state(), Svc::Starting | Svc::Running | Svc::Stopping)
    }

    /// Local clusters of `reg` other than `id` that are starting, running or stopping.
    pub fn running_others(&self, reg: &Registry, id: &str) -> usize {
        reg.clusters.iter().filter(|c| c.kind.is_local() && c.id != id).filter_map(|c| self.get(c)).filter(|d| Fleet::busy(d)).count()
    }

    /// Start a local cluster: the memory gate first (only a second running one can be refused), then the same start as ever
    /// (requirements, clock and network gate, supervisor and watchdog of that instance).
    pub fn start(&self, reg: &Registry, id: &str, user: &str, role: Role) -> Result<(), String> {
        if let Some(r) = manage_refusal(role) {
            return Err(r);
        }
        let e = reg.get(id).ok_or("no such cluster")?;
        if !e.kind.is_local() {
            return Err(format!("{} is not on this machine: IYAKKI cannot start it", e.phrase()));
        }
        let d = self.get(e).ok_or("no such cluster")?;
        local_start_gate(self.running_others(reg, id), self.mem_available()).map_err(|m| format!("{}: {m}", e.phrase()))?;
        d.start(user)
    }

    pub fn stop(&self, reg: &Registry, id: &str, user: &str, role: Role) -> Result<(), String> {
        if let Some(r) = manage_refusal(role) {
            return Err(r);
        }
        let e = reg.get(id).ok_or("no such cluster")?;
        let d = self.get(e).ok_or_else(|| format!("{} is not on this machine: IYAKKI did not start it and does not stop it", e.phrase()))?;
        d.stop(user)
    }

    /// Start the extra clusters marked Autostart (the primary has its own `K3sDev::autostart`). Each goes through the memory gate;
    /// the failures are returned with the cluster's name.
    pub fn autostart_extras(&self, reg: &Registry) -> Vec<String> {
        let mut errs = Vec::new();
        for e in reg.clusters.iter().filter(|c| c.kind.is_local() && !c.is_primary() && c.autostart) {
            let Some(d) = self.get(e) else { continue };
            if d.installed().is_none() || d.state() != Svc::Stopped {
                continue;
            }
            if let Err(err) = local_start_gate(self.running_others(reg, &e.id), self.mem_available()).and_then(|_| d.start("system")) {
                errs.push(format!("{}: {err}", e.phrase()));
            }
        }
        errs
    }

    pub fn clis(&self) -> Arc<CliManager> {
        self.clis.clone()
    }

    /// At start of IYAKKI: start the extra local clusters any account marked Autostart (each cluster once, through the memory gate).
    /// The built-in one has its own `K3sDev::autostart`. Returns the failures, named.
    pub fn autostart_all(&self, dir: &Path) -> Vec<String> {
        let mut merged = Registry::default();
        for (_, r) in all_registries(dir) {
            for c in r.clusters.into_iter().filter(|c| c.kind.is_local() && !c.is_primary() && c.autostart) {
                if !merged.clusters.iter().any(|m| m.id == c.id) {
                    merged.clusters.push(c);
                }
            }
        }
        self.autostart_extras(&merged)
    }

    /// The status of an entry: a local one from its supervisor's health (no process is started), a remote one from [`probe`].
    pub fn status(&self, e: &ClusterEntry, test_remote: bool) -> Status {
        match self.get(e) {
            Some(d) => {
                let h = d.health();
                let dot = match h.state.as_str() {
                    "running" => Dot::Running,
                    "starting" => Dot::Unknown,
                    "failed" => Dot::Unreachable,
                    _ => Dot::Stopped,
                };
                let (nodes, ready) = match h.node_ready {
                    Some(true) => (1, 1),
                    Some(false) => (1, 0),
                    None => (0, 0),
                };
                Status { dot, version: h.version.clone(), nodes, ready, last_error: h.failed_reason.clone(), checked_at: h.checked_at, gpu: gpu_summary(&e.id) }
            }
            None if test_remote => probe(&self.run, e),
            None => Status { gpu: gpu_summary(&e.id), ..Status::unknown() },
        }
    }

    /// Ports of two instances must differ; a check for the Clusters page.
    pub fn port_clashes(reg: &Registry) -> Vec<String> {
        let mut seen: BTreeMap<u16, String> = BTreeMap::new();
        let mut out = Vec::new();
        for (c, i) in reg.clusters.iter().filter_map(|c| c.instance().map(|i| (c, i))) {
            for p in i.all_ports() {
                if let Some(other) = seen.insert(p, c.name.clone()) {
                    out.push(format!("port {p} is used by \"{other}\" and \"{}\"", c.name));
                }
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "clusters_tests.rs"]
mod tests;
