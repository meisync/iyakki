//! Types moved out of `iyakki_agent::clusters` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// A status older than this is "stale".
pub const STALE_AFTER_SECS: i64 = 180;

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

/// The id of the cluster on this machine.
pub const LOCAL_ID: &str = "local";

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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iyakki_sec::policy::Role;

/// The id of the managed local cluster that always exists.
/// Local clusters (the built-in one included) a person may have by default.
pub const DEFAULT_MAX_LOCAL: usize = 2;
/// With "Advanced" on.
pub const ADVANCED_MAX_LOCAL: usize = 4;
/// Memory one more running k3s needs: its own (about 2 GiB) plus the machine's usual reserve (`memgate::MIN_FREE`, 512 MiB).
pub const LOCAL_K3S_NEED: u64 = 2 * (1 << 30) + 512 * (1 << 20);

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

pub fn safe_user(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

pub fn file_of(dir: &Path, user: &str) -> PathBuf {
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
        if let Some(c) = crate::devcluster::Choice::load(dir, user) {
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
    pub fn repaired(mut self) -> Registry {
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
        if !crate::k3sdev::valid_id(&id) {
            id = format!("c-{id}");
            if !crate::k3sdev::valid_id(&id) {
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
        let e = ClusterEntry { id, name: name.trim().to_string(), kind: Kind::Local, slot, created_at: iyakki_clock::now_unix(), ..ClusterEntry::default() };
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

    pub fn add_remote_inner(&mut self, name: &str, kind: Kind, kubeconfig: &str, context: &str, namespace: &str) -> Result<ClusterEntry, String> {
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
        let e = ClusterEntry { id, name: name.trim().to_string(), kind, kubeconfig: reference, context: context.trim().into(), namespace: namespace.trim().into(), created_at: iyakki_clock::now_unix(), ..ClusterEntry::default() };
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
pub fn all_registries(dir: &Path) -> Vec<(String, Registry)> {
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
/// The cluster a workspace uses, for texts and audit lines (`cluster "gpu-box" (remote)`). Empty with one cluster: the caller then names
/// the cluster it resolved itself (a workspace's own `project.json` kubeconfig can differ from the registry's `local`).
pub fn phrase_for(dir: &Path, user: &str, workspace: &str) -> String {
    let r = Registry::load(dir, user);
    if r.has_choice() { r.effective(workspace).phrase() } else { String::new() }
}

/// entry's folder is never touched here.
pub fn forget_remote(dir: &Path, user: &str, reg: &mut Registry, id: &str, delete_vault_entry: bool, delete_secret: &mut dyn FnMut(&str) -> Result<(), String>) -> Result<ClusterEntry, String> {
    let e = reg.get(id).cloned().ok_or("no such cluster")?;
    if e.kind.is_local() {
        return Err("this is a local cluster: remove it with the local removal (it asks about its data)".into());
    }
    reg.remove(id)?;
    if delete_vault_entry && crate::kubeprofiles::is_vault_ref(&e.kubeconfig) {
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

/// What a [`ClusterEntry`] needs from the machine (the managed k3s layout, the vault): kept in the agent.
pub trait ClusterEntryExt {
    fn instance(&self) -> Option<crate::k3sdev::Instance>;
    fn kubeconfig_for(&self, data_dir: &Path) -> String;
    fn in_vault(&self) -> bool;
}

impl ClusterEntryExt for ClusterEntry {
    fn instance(&self) -> Option<crate::k3sdev::Instance> {
        if self.kind != Kind::Local {
            return None;
        }
        Some(if self.is_primary() { crate::k3sdev::Instance::primary() } else { crate::k3sdev::Instance::extra(&self.id, self.slot.max(1)) })
    }

    /// The kubeconfig a program is given: the managed folder's file for a local cluster, the reference for a remote one.
    fn kubeconfig_for(&self, data_dir: &Path) -> String {
        match self.instance() {
            Some(i) => crate::k3sdev::Layout::for_instance(data_dir, &i).kubeconfig().display().to_string(),
            None => self.kubeconfig.clone(),
        }
    }

    fn in_vault(&self) -> bool {
        crate::kubeprofiles::is_vault_ref(&self.kubeconfig)
    }
}
