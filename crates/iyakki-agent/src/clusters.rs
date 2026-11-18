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
pub use iyakki_shared::clusters::*;

fn now() -> i64 {
    crate::clock::now_unix()
}




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

/// the workspace (its override, else the account's active cluster); with one cluster exactly the old remembered choice, as before.
pub fn remembered_for(dir: &Path, user: &str, workspace: &str) -> Option<Choice> {
    let r = Registry::load(dir, user);
    if !r.has_choice() {
        return Choice::load(dir, user);
    }
    Some(choice_for(&r.effective(workspace), dir))
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
