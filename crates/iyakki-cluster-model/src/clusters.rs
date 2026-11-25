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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        for p in crate::profiles::vault_load(dir, user) {
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
        for p in crate::profiles::vault_load(dir, user) {
            let _ = r.add_remote_inner(&p.name, Kind::Remote, &p.reference, "", "");
        }
        if let Some(c) = crate::choice::Choice::load(dir, user) {
            let found = r.clusters.iter().find(|e| !e.kubeconfig.is_empty() && e.kubeconfig == c.kubeconfig).map(|e| e.id.clone());
            let id = found.or_else(|| {
                let machine = crate::choice::machine_kubeconfig().map(|p| p.display().to_string());
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
        if !crate::ids::valid_k3s_id(&id) {
            id = format!("c-{id}");
            if !crate::ids::valid_k3s_id(&id) {
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
            if !crate::ids::valid_secret_name(entry) {
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
    if delete_vault_entry && crate::profiles::is_vault_ref(&e.kubeconfig) {
        let _ = crate::profiles::vault_remove(dir, user, &e.name);
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

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;
    use std::collections::BTreeMap;

    fn entry(id: &str, name: &str, kind: Kind) -> ClusterEntry {
        ClusterEntry { id: id.into(), name: name.into(), kind, ..ClusterEntry::default() }
    }

    fn registry_with(clusters: Vec<ClusterEntry>) -> Registry {
        Registry { clusters, ..Registry::default() }
    }

    fn status(dot: Dot, checked_at: i64) -> Status {
        Status { dot, checked_at, ..Status::unknown() }
    }

    #[test]
    fn kind_words_parse_ignoring_case_and_spaces_and_cloud_is_an_alias_for_cloud_cli() {
        assert_eq!(Kind::from_word("  CLOUD "), Some(Kind::CloudCli));
        assert_eq!(Kind::from_word("Tunnel"), Some(Kind::Tunnel));
        assert_eq!(Kind::from_word("bogus"), None);
    }

    #[test]
    fn every_kind_word_parses_back_to_its_kind() {
        for k in [Kind::Local, Kind::Remote, Kind::Tunnel, Kind::CloudCli] {
            assert_eq!(Kind::from_word(k.word()), Some(k));
        }
    }

    #[test]
    fn only_the_local_kind_counts_as_local() {
        assert!(Kind::Local.is_local());
        assert!(!Kind::Remote.is_local() && !Kind::Tunnel.is_local() && !Kind::CloudCli.is_local());
    }

    #[test]
    fn dot_words_are_the_words_the_status_shows() {
        assert_eq!(Dot::Running.word(), "running");
        assert_eq!(Dot::Stopped.word(), "stopped");
        assert_eq!(Dot::Unreachable.word(), "unreachable");
        assert_eq!(Dot::Stale.word(), "stale");
        assert_eq!(Dot::Unknown.word(), "not checked");
    }

    #[test]
    fn a_status_line_joins_the_dot_version_and_node_count_with_middle_dots() {
        let s = Status { dot: Dot::Running, version: "v1.31.1+k3s1".into(), nodes: 1, ready: 1, ..Status::unknown() };
        assert_eq!(s.line(), "running \u{b7} v1.31.1+k3s1 \u{b7} 1/1 nodes ready");
    }

    #[test]
    fn a_status_line_skips_missing_version_and_nodes_and_appends_the_last_error() {
        let s = Status { dot: Dot::Unreachable, last_error: "timeout".into(), ..Status::unknown() };
        assert_eq!(s.line(), "unreachable \u{b7} timeout");
    }

    #[test]
    fn the_line_of_an_unknown_status_is_only_not_checked() {
        assert_eq!(Status::unknown().line(), "not checked");
    }

    #[test]
    fn an_old_running_or_unreachable_snapshot_shows_as_stale_only_after_the_limit() {
        assert_eq!(status(Dot::Running, 1000).aged(1000 + STALE_AFTER_SECS + 1).dot, Dot::Stale);
        assert_eq!(status(Dot::Running, 1000).aged(1000 + STALE_AFTER_SECS).dot, Dot::Running);
        assert_eq!(status(Dot::Unreachable, 1000).aged(1000 + STALE_AFTER_SECS + 1).dot, Dot::Stale);
    }

    #[test]
    fn a_stopped_unknown_or_never_checked_snapshot_does_not_age_into_stale() {
        assert_eq!(status(Dot::Stopped, 1000).aged(99_999).dot, Dot::Stopped);
        assert_eq!(status(Dot::Unknown, 1000).aged(99_999).dot, Dot::Unknown);
        assert_eq!(status(Dot::Running, 0).aged(99_999).dot, Dot::Running);
    }

    #[test]
    fn the_built_in_cluster_is_local_named_this_machine_and_is_the_only_primary() {
        let p = ClusterEntry::local_primary();
        assert_eq!((p.id.as_str(), p.name.as_str(), p.kind), ("local", "This machine", Kind::Local));
        assert!(p.is_primary());
        assert!(!entry("gpu", "GPU", Kind::Remote).is_primary());
    }

    #[test]
    fn a_phrase_names_the_cluster_and_its_kind() {
        let e = ClusterEntry { name: "gpu-box".into(), kind: Kind::Remote, ..ClusterEntry::default() };
        assert_eq!(e.phrase(), "cluster \"gpu-box\" (remote)");
    }

    #[test]
    fn the_confirmation_texts_put_the_action_and_the_cluster_together_in_three_shapes() {
        let e = ClusterEntry { name: "gpu-box".into(), kind: Kind::Remote, ..ClusterEntry::default() };
        assert_eq!(confirm_text("Deploy Lemonade", &e), "Deploy Lemonade to cluster \"gpu-box\" (remote)");
        assert_eq!(confirm_text_on("Start", &e), "Start cluster \"gpu-box\" (remote)");
        assert_eq!(confirm_text_from("Remove Registry", &e), "Remove Registry from cluster \"gpu-box\" (remote)");
    }

    #[test]
    fn a_production_looking_name_is_read_only_but_nonprod_names_are_not() {
        let prod = ClusterEntry { name: "Prod Cluster".into(), ..ClusterEntry::default() };
        let reason = prod.read_only_reason().unwrap();
        assert!(reason.contains("looks like production") && reason.contains("\"Prod Cluster\""));
        let nonprod = ClusterEntry { name: "gpu-box".into(), context: "nonprod-east".into(), ..ClusterEntry::default() };
        assert_eq!(nonprod.read_only_reason(), None);
        let dashed = ClusterEntry { name: "staging".into(), id: "non-prod-2".into(), ..ClusterEntry::default() };
        assert_eq!(dashed.read_only_reason(), None);
    }

    #[test]
    fn only_administrators_and_operators_can_manage_clusters() {
        assert!(can_manage(Role::Admin) && can_manage(Role::Operator));
        assert!(!can_manage(Role::User) && !can_manage(Role::Guest));
    }

    #[test]
    fn everyone_but_a_guest_can_switch_clusters() {
        assert!(can_switch(Role::Admin) && can_switch(Role::Operator) && can_switch(Role::User));
        assert!(!can_switch(Role::Guest));
    }

    #[test]
    fn a_refused_management_change_names_the_role_and_an_allowed_one_has_no_refusal() {
        assert_eq!(manage_refusal(Role::Admin), None);
        assert_eq!(manage_refusal(Role::User), Some("only an administrator or operator can change the clusters (you are user)".to_string()));
        assert!(manage_refusal(Role::Guest).unwrap().ends_with("(you are guest)"));
    }

    #[test]
    fn safe_user_keeps_only_letters_digits_dash_and_underscore() {
        assert_eq!(safe_user("admin.team@x/../y_1-"), "adminteamxy_1-");
    }

    #[test]
    fn the_registry_file_is_clusters_with_the_safe_user_name_in_the_folder() {
        assert_eq!(file_of(Path::new("/d"), "admin.team"), PathBuf::from("/d/clusters-adminteam.json"));
    }

    #[test]
    fn a_slug_is_a_lower_case_dashed_id_of_at_most_sixteen_characters() {
        assert_eq!(slug("  GPU Box #2!  "), "gpu-box-2");
        assert_eq!(slug("a--b"), "a-b");
        assert_eq!(slug("abcdefghijklmnopqrstuvwxyz"), "abcdefghijklmnop");
        assert_eq!(slug("abcdefghijklmno pq"), "abcdefghijklmno");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn a_cluster_name_is_letters_digits_spaces_and_marks_up_to_forty_characters() {
        assert!(valid_name("Lab Box_2.0-x").is_ok());
        assert!(valid_name(&"a".repeat(40)).is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("   ").is_err());
        assert!(valid_name(&"a".repeat(41)).is_err());
        assert!(valid_name("bad/name").unwrap_err().starts_with("a cluster name is letters"));
    }

    #[test]
    fn a_new_registry_holds_only_the_built_in_cluster_and_it_is_active() {
        let r = Registry::default();
        assert_eq!(r.version, 1);
        assert_eq!(r.clusters.len(), 1);
        assert_eq!(r.clusters[0].id, "local");
        assert_eq!(r.active, "local");
        assert!(!r.advanced);
        assert_eq!((r.local_count(), r.max_local(), r.has_choice()), (1, DEFAULT_MAX_LOCAL, false));
    }

    #[test]
    fn advanced_raises_the_local_cluster_maximum() {
        let r = Registry { advanced: true, ..Registry::default() };
        assert_eq!(r.max_local(), ADVANCED_MAX_LOCAL);
        assert_eq!(ADVANCED_MAX_LOCAL, 4);
    }

    #[test]
    fn get_matches_an_id_exactly_and_find_name_matches_a_name_or_an_id_ignoring_case() {
        let r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu-box", "GPU Box", Kind::Remote)]);
        assert!(r.get("gpu-box").is_some());
        assert!(r.get("GPU Box").is_none());
        assert_eq!(r.find_name("  gpu box ").map(|c| c.id.as_str()), Some("gpu-box"));
        assert_eq!(r.find_name("GPU-BOX").map(|c| c.id.as_str()), Some("gpu-box"));
        assert!(r.find_name("nope").is_none());
    }

    #[test]
    fn the_active_cluster_falls_back_to_the_built_in_one_when_the_saved_one_is_gone() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu-box", "GPU Box", Kind::Remote)]);
        r.active = "gpu-box".into();
        assert_eq!(r.active_entry().id, "gpu-box");
        r.active = "ghost".into();
        assert_eq!(r.active_entry().id, "local");
    }

    #[test]
    fn a_workspace_override_applies_only_while_its_cluster_exists() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu-box", "GPU Box", Kind::Remote)]);
        r.workspace_active.insert("ws1".into(), "gpu-box".into());
        r.workspace_active.insert("ws2".into(), "gone".into());
        assert_eq!(r.effective("ws1").id, "gpu-box");
        assert_eq!(r.effective("other").id, "local");
        assert_eq!(r.effective("ws2").id, "local");
    }

    #[test]
    fn the_switchers_show_only_when_there_is_more_than_one_cluster() {
        let mut r = Registry::default();
        assert!(!r.has_choice());
        r.clusters.push(entry("gpu-box", "GPU Box", Kind::Remote));
        assert!(r.has_choice());
    }

    #[test]
    fn visible_clusters_depend_on_the_role_guests_see_none_and_users_only_the_shared_ones() {
        let mut hidden = entry("hidden", "Hidden", Kind::Remote);
        hidden.visible_to_users = false;
        let r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu-box", "GPU Box", Kind::Remote), hidden]);
        assert_eq!(r.visible_to(Role::Admin).len(), 3);
        assert_eq!(r.visible_to(Role::Operator).len(), 3);
        let user: Vec<&str> = r.visible_to(Role::User).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(user, vec!["local", "gpu-box"]);
        assert!(r.visible_to(Role::Guest).is_empty());
    }

    #[test]
    fn local_count_includes_the_built_in_cluster_and_a_full_limit_refuses_another_local_one() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("lab", "Lab", Kind::Local), entry("gpu", "GPU", Kind::Remote)]);
        assert_eq!(r.local_count(), 2);
        let err = r.add_local("Another").unwrap_err();
        assert!(err.starts_with("this account may have 2 local clusters"));
        assert!(err.contains("switch on Advanced"));
    }

    #[test]
    fn with_advanced_on_a_full_advanced_limit_says_to_remove_one_first() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("a", "A", Kind::Local), entry("b", "B", Kind::Local), entry("c", "C", Kind::Local)]);
        r.advanced = true;
        assert_eq!(r.local_count(), 4);
        assert!(r.add_local("Another").unwrap_err().contains("remove one first."));
    }

    #[test]
    fn adding_a_local_cluster_refuses_a_name_in_use_or_an_invalid_name() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("lab", "Lab", Kind::Local)]);
        assert!(r.add_local("lab").unwrap_err().contains("exists already"));
        assert!(r.add_local("bad/name").unwrap_err().starts_with("a cluster name is letters"));
    }

    #[test]
    fn adding_a_remote_cluster_refuses_a_local_kind_an_empty_kubeconfig_and_a_bad_vault_name() {
        let mut r = Registry::default();
        assert!(r.add_remote_inner("Lab", Kind::Local, "/k", "", "").unwrap_err().starts_with("a remote cluster is of kind"));
        assert_eq!(r.add_remote_inner("Lab", Kind::Remote, "   ", "", "").unwrap_err(), "choose a kubeconfig file or a vault entry");
        assert!(r.add_remote_inner("Lab", Kind::Remote, "vault:Bad Name", "", "").unwrap_err().starts_with("a vault entry name is lowercase"));
        assert_eq!(r.clusters.len(), 1);
    }

    #[test]
    fn adding_a_remote_cluster_refuses_a_name_in_use_and_an_invalid_name() {
        let mut r = Registry::default();
        assert!(r.add_remote("this machine", Kind::Remote, "/k", "", "").unwrap_err().contains("exists already"));
        assert!(r.add_remote("bad/name", Kind::Remote, "/k", "", "").unwrap_err().starts_with("a cluster name is letters"));
    }

    #[test]
    fn renaming_keeps_the_id_and_refuses_a_name_another_cluster_has() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu-box", "GPU Box", Kind::Remote), entry("other", "Other", Kind::Remote)]);
        assert_eq!(r.rename("gpu-box", "GPU Lab"), Ok(()));
        assert_eq!(r.get("gpu-box").map(|c| c.name.as_str()), Some("GPU Lab"));
        assert_eq!(r.rename("gpu-box", "gpu lab"), Ok(()));
        assert!(r.rename("gpu-box", "other").unwrap_err().contains("exists already"));
        assert_eq!(r.rename("ghost", "Fine").unwrap_err(), "no such cluster");
    }

    #[test]
    fn autostart_is_refused_for_the_built_in_and_remote_clusters_and_allowed_for_extra_local_ones() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("lab", "Lab", Kind::Local), entry("gpu", "GPU", Kind::Remote)]);
        assert!(r.set_autostart("local", true).unwrap_err().contains("keeps its own Autostart setting"));
        assert!(r.set_autostart("gpu", true).unwrap_err().starts_with("only a local cluster can start"));
        assert_eq!(r.set_autostart("ghost", true), Err("no such cluster".to_string()));
        assert_eq!(r.set_autostart("lab", true), Ok(()));
        assert!(r.get("lab").unwrap().autostart);
    }

    #[test]
    fn the_built_in_cluster_always_stays_visible_to_users() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu", "GPU", Kind::Remote)]);
        assert_eq!(r.set_visible_to_users("gpu", false), Ok(()));
        assert!(!r.get("gpu").unwrap().visible_to_users);
        assert_eq!(r.set_visible_to_users("local", false), Ok(()));
        assert!(r.get("local").unwrap().visible_to_users);
        assert_eq!(r.set_visible_to_users("ghost", true), Err("no such cluster".to_string()));
    }

    #[test]
    fn removing_a_cluster_moves_an_active_choice_back_to_the_built_in_one_and_drops_its_workspaces() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu", "GPU", Kind::Remote)]);
        r.active = "gpu".into();
        r.workspace_active = BTreeMap::from([("ws".to_string(), "gpu".to_string()), ("ws2".to_string(), "local".to_string())]);
        assert_eq!(r.remove("gpu").map(|e| e.id), Ok("gpu".to_string()));
        assert_eq!(r.active, "local");
        assert!(r.workspace_active.get("ws").is_none());
        assert_eq!(r.workspace_active.get("ws2").map(String::as_str), Some("local"));
    }

    #[test]
    fn the_built_in_cluster_cannot_be_removed_from_the_list_and_unknown_ids_are_refused() {
        let mut r = Registry::default();
        assert!(r.remove("local").unwrap_err().contains("cannot be removed"));
        assert_eq!(r.remove("ghost").unwrap_err(), "no such cluster");
    }

    #[test]
    fn the_active_cluster_must_be_a_known_one() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu", "GPU", Kind::Remote)]);
        assert_eq!(r.set_active("gpu"), Ok(()));
        assert_eq!(r.active, "gpu");
        assert_eq!(r.set_active("ghost"), Err("no such cluster".to_string()));
        assert_eq!(r.active, "gpu");
    }

    #[test]
    fn a_workspace_choice_trims_its_name_and_none_removes_it() {
        let mut r = registry_with(vec![ClusterEntry::local_primary(), entry("gpu", "GPU", Kind::Remote)]);
        assert_eq!(r.set_workspace("  ws3 ", Some("gpu")), Ok(()));
        assert_eq!(r.workspace_active.get("ws3").map(String::as_str), Some("gpu"));
        assert_eq!(r.set_workspace("ws3", None), Ok(()));
        assert!(r.workspace_active.is_empty());
        assert_eq!(r.set_workspace("  ", Some("gpu")), Err("no workspace".to_string()));
        assert_eq!(r.set_workspace("ws4", Some("ghost")), Err("no such cluster".to_string()));
    }

    #[test]
    fn repair_restores_the_built_in_cluster_drops_duplicates_and_blank_ids_and_fixes_the_active_choice() {
        let r = Registry {
            version: 1,
            clusters: vec![entry("gpu", "GPU", Kind::Remote), entry("gpu", "Dup", Kind::Remote), entry("lab", "Lab", Kind::Local), entry("", "Blank", Kind::Remote)],
            active: "ghost".into(),
            workspace_active: BTreeMap::from([("ws".to_string(), "ghost".to_string()), ("ws2".to_string(), "gpu".to_string())]),
            advanced: false,
        }
        .repaired();
        let ids: Vec<&str> = r.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["local", "gpu", "lab"]);
        assert_eq!(r.clusters[1].name, "GPU");
        assert_eq!(r.clusters[0].kind, Kind::Local);
        assert_eq!(r.clusters[2].slot, 1);
        assert_eq!(r.active, "local");
        assert_eq!(r.workspace_active.keys().map(String::as_str).collect::<Vec<_>>(), vec!["ws2"]);
    }

    #[test]
    fn repair_keeps_a_saved_slot_and_gives_a_missing_one_the_first_free_slot() {
        let mut lab = entry("lab", "Lab", Kind::Local);
        lab.slot = 2;
        let r = registry_with(vec![lab, entry("lab2", "Lab 2", Kind::Local)]).repaired();
        assert_eq!(r.get("lab").unwrap().slot, 2);
        assert_eq!(r.get("lab2").unwrap().slot, 1);
    }

    #[test]
    fn forgetting_the_built_in_or_an_unknown_cluster_is_refused_without_touching_anything() {
        let mut r = Registry::default();
        let res = forget_remote(Path::new("/nonexistent"), "admin", &mut r, "local", false, &mut |_: &str| -> Result<(), String> { Ok(()) });
        assert!(res.unwrap_err().starts_with("this is a local cluster"));
        let res = forget_remote(Path::new("/nonexistent"), "admin", &mut r, "ghost", false, &mut |_: &str| -> Result<(), String> { Ok(()) });
        assert_eq!(res.unwrap_err(), "no such cluster");
        assert_eq!(r.clusters.len(), 1);
    }

    #[test]
    fn forgetting_a_remote_cluster_with_a_plain_kubeconfig_path_removes_it_and_touches_no_secret() {
        let mut gpu = entry("gpu", "GPU", Kind::Remote);
        gpu.kubeconfig = "/etc/kube/gpu.yaml".into();
        let mut r = registry_with(vec![ClusterEntry::local_primary(), gpu]);
        let mut calls = 0;
        let res = forget_remote(Path::new("/nonexistent"), "admin", &mut r, "gpu", true, &mut |_: &str| -> Result<(), String> {
            calls += 1;
            Ok(())
        });
        assert_eq!(res.map(|e| e.id), Ok("gpu".to_string()));
        assert!(r.get("gpu").is_none());
        assert_eq!(calls, 0);
    }

    #[test]
    fn a_second_local_cluster_is_never_allowed_without_free_memory_but_the_first_always_is() {
        assert_eq!(local_start_gate(0, None), Ok(()));
        assert!(local_start_gate(1, None).unwrap_err().contains("cannot be read"));
        assert!(local_start_gate(1, Some(LOCAL_K3S_NEED - 1)).unwrap_err().contains("of memory is free"));
        assert_eq!(local_start_gate(1, Some(LOCAL_K3S_NEED)), Ok(()));
        assert_eq!(local_start_gate(2, Some(u64::MAX)), Ok(()));
    }

    #[test]
    fn the_memory_gate_quotes_free_memory_and_the_need_in_gigabytes() {
        let err = local_start_gate(1, Some(1 << 30)).unwrap_err();
        assert!(err.contains("only 1.0 GB of memory is free"));
        assert!(err.contains("about 2.5 GB free"));
    }

    #[test]
    fn the_cluster_limits_and_the_memory_need_are_the_documented_values() {
        assert_eq!(DEFAULT_MAX_LOCAL, 2);
        assert_eq!(ADVANCED_MAX_LOCAL, 4);
        assert_eq!(LOCAL_K3S_NEED, 2_684_354_560);
        assert_eq!(STALE_AFTER_SECS, 180);
    }
}
