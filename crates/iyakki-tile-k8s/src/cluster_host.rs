//! What every screen that works against a cluster needs from the cluster switcher (docs/K3S-DEV.md, "Clusters"): the account's registry,
//! the rows with their status dots and GPU badges, a role-checked switch, and "Manage clusters...". The K8S tile's header, the AI IDE's
//! Kubernetes panel and deploy header and the Store -> Setup popup each own one [`ClusterHost`] and place the closed box where their
//! header has room; the open list is drawn last, over everything.
//!
//! * One cluster: nothing is drawn or clickable, and the screens behave as before.
//! * Switching goes through `clusters::switch` (or `switch_workspace` when the workspace has its own cluster), so the roles apply;
//!   every switch is audited (`cluster.switch`, naming the cluster).
//! * Never blocks drawing: the registry is read at most every two seconds, the GPU badges come from the cache that
//!   `clusters::refresh_gpus` fills on a thread (never in tests, which only see what they put there).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cluster_switch::{rows_for, ClusterSwitch, SwitchOut, SwitchRow};
use crate::gfx::Gfx;
use crate::ui::{Event, R};

use iyakki_cluster_model::clusters::{self, ClusterEntry, Dot, Registry, Status};
use iyakki_sec::policy::Role;

/// What the host does after the switcher handled an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostOut {
    None,
    /// The cluster is now this one (saved; the host re-reads what it shows).
    Switched(ClusterEntry),
    /// The switch was refused (role, or the cluster is gone): say why.
    Refused(String),
    /// "Manage clusters..." (the host asks the shell for Settings -> Clusters).
    Manage,
}

/// What the switcher asks of the machine: the person, the status of a cluster, the GPU look, the role-checked switch and the audit line. The shell
/// gives the real one (the fleet, the agent's cluster registry, the audit log); a fake gives fixed answers.
pub trait ClusterHostPort {
    fn data_dir(&self) -> std::path::PathBuf;
    fn user(&self) -> String;
    fn role(&self) -> Role;
    /// The status of a cluster on this machine (its health), as the fleet reports it.
    fn local_status(&self, e: &ClusterEntry) -> Status;
    /// A remote cluster's dot from the last GPU look (which is one `kubectl get nodes`): answered, answered long ago, or not checked.
    fn remote_status(&self, e: &ClusterEntry) -> Status;
    /// Start a look at the GPUs of the remote clusters (on a thread; never in tests, which only see what they put in the cache).
    fn refresh_gpus(&self, visible: &[ClusterEntry]);
    /// The summary of the GPUs last seen on a cluster (`2 x NVIDIA ... each`), if any.
    fn gpu_summary(&self, id: &str) -> Option<String>;
    /// The account's switch (role-checked by the registry): the new active cluster.
    fn switch_account(&self, id: &str) -> Result<ClusterEntry, String>;
    fn audit(&self, action: &str, detail: &str);
}

pub struct ClusterHost {
    pub sw: ClusterSwitch,
    rows: Vec<SwitchRow>,
    dir: std::path::PathBuf,
    user: String,
    role: Role,
    workspace: String,
    /// The workspace has its own cluster: a switch sets that instead of the account's active cluster.
    ws_override: bool,
    effective: Option<ClusterEntry>,
    loaded: Option<Instant>,
    port: Arc<dyn ClusterHostPort>,
}

/// The shell's request that opens Settings -> Clusters.
pub const MANAGE_REQUEST: (&str, &str) = ("settings", "tab:clusters");

impl ClusterHost {
    /// `base`: the event codes `base..base + 10 + rows` belong to this switcher. `workspace`: the AI IDE workspace (empty elsewhere).
    pub fn new(base: usize, port: Arc<dyn ClusterHostPort>, workspace: &str) -> ClusterHost {
        let mut h = ClusterHost { sw: ClusterSwitch::new(base), rows: vec![], dir: port.data_dir(), user: port.user(), role: port.role(), workspace: workspace.to_string(), ws_override: false, effective: None, loaded: None, port };
        h.reload(&|_| None);
        h
    }

    pub fn rows(&self) -> &[SwitchRow] {
        &self.rows
    }

    pub fn visible(&self) -> bool {
        ClusterSwitch::visible(&self.rows)
    }

    /// The cluster this screen works against right now (the workspace's override, else the account's active cluster).
    pub fn effective(&self) -> Option<&ClusterEntry> {
        self.effective.as_ref()
    }

    /// `cluster "gpu-box" (remote)` for the confirmation texts, even with one cluster.
    pub fn phrase(&self) -> String {
        self.effective.as_ref().map(|e| e.phrase()).unwrap_or_else(|| ClusterEntry::local_primary().phrase())
    }

    /// Read the registry again. `dot`: a host that knows a cluster's state better than the registry does (the K8S tile knows if the
    /// managed k3s runs) says it here; `None` leaves the default (a local cluster's health, a remote one's last GPU look).
    pub fn reload(&mut self, dot: &dyn Fn(&ClusterEntry) -> Option<Dot>) {
        self.loaded = Some(Instant::now());
        let reg = Registry::load_synced(&self.dir, &self.user);
        self.ws_override = !self.workspace.is_empty() && reg.workspace_active.contains_key(&self.workspace);
        self.effective = Some(reg.effective(&self.workspace));
        let port = &self.port;
        let visible: Vec<ClusterEntry> = reg.visible_to(self.role).into_iter().cloned().collect();
        if visible.iter().any(|e| !e.kind.is_local()) {
            port.refresh_gpus(&visible);
        }
        let status = |e: &ClusterEntry| -> Status {
            let mut st = if e.kind.is_local() { port.local_status(e) } else { port.remote_status(e) };
            if let Some(d) = dot(e) {
                st.dot = d;
            }
            st.gpu = port.gpu_summary(&e.id);
            st
        };
        self.rows = rows_for(&reg, self.role, &self.workspace, &status);
    }

    /// Re-read now and then (the file is small, but drawing must stay cheap).
    pub fn tick(&mut self, dot: &dyn Fn(&ClusterEntry) -> Option<Dot>) {
        if self.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.reload(dot);
        }
    }

    pub fn width(&self, g: &mut Gfx, s: f32) -> f32 {
        ClusterSwitch::width(g, s, &self.rows)
    }

    /// The closed box at (`x`, `y`), at most `max_w` wide; `None` (nothing drawn or clickable) with one cluster.
    pub fn draw(&mut self, g: &mut Gfx, x: f32, y: f32, max_w: f32, s: f32, accent: [u8; 4]) -> Option<R> {
        self.sw.draw(g, x, y, max_w, s, &self.rows, accent)
    }

    /// The open list: draw it last, inside `bounds`.
    pub fn draw_popup(&mut self, g: &mut Gfx, bounds: R, s: f32, accent: [u8; 4]) -> Option<R> {
        self.sw.draw_popup(g, bounds, s, &self.rows, accent)
    }

    /// The list is open and wants every key.
    pub fn owns_keys(&self) -> bool {
        self.sw.owns_keys()
    }

    /// The account's active cluster becomes `id` (role-checked; the caller says what it did) and the rows are read again.
    pub fn switch_account(&mut self, id: &str, dot: &dyn Fn(&ClusterEntry) -> Option<Dot>) -> Result<ClusterEntry, String> {
        let e = self.port.switch_account(id)?;
        self.reload(dot);
        Ok(e)
    }

    /// Feed an event of the switcher; does the switch itself (role-checked, audited) and reports it.
    pub fn handle(&mut self, ev: &Event, dot: &dyn Fn(&ClusterEntry) -> Option<Dot>) -> HostOut {
        match self.sw.handle(ev, &self.rows) {
            SwitchOut::None => HostOut::None,
            SwitchOut::Manage => HostOut::Manage,
            SwitchOut::Switch(id) => {
                let r = if self.ws_override { clusters::switch_workspace(&self.dir, &self.user, self.role, &self.workspace, Some(&id)) } else { self.port.switch_account(&id) };
                match r {
                    Ok(e) => {
                        self.port.audit("cluster.switch", &format!("{}{}", e.phrase(), if self.ws_override { format!(" (workspace {})", self.workspace) } else { String::new() }));
                        self.reload(dot);
                        HostOut::Switched(e)
                    }
                    Err(why) => HostOut::Refused(why),
                }
            }
        }
    }
}
