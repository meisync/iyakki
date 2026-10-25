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

use std::time::{Duration, Instant};

use crate::apps::cluster_switch::{rows_for, ClusterSwitch, SwitchOut, SwitchRow};
use crate::apps::AppCx;
use crate::gfx::Gfx;
use crate::ui::{Event, R};

use iyakki_agent::clusters::{self, ClusterEntry, Dot, Registry, Status};
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
    services: std::sync::Arc<iyakki_agent::services::Services>,
    audit: std::sync::Arc<crate::accounts::AuditLog>,
}

/// The shell's request that opens Settings -> Clusters.
pub const MANAGE_REQUEST: (&str, &str) = ("settings", "tab:clusters");

impl ClusterHost {
    /// `base`: the event codes `base..base + 10 + rows` belong to this switcher. `workspace`: the AI IDE workspace (empty elsewhere).
    pub fn new(base: usize, cx: &AppCx, workspace: &str) -> ClusterHost {
        let mut h = ClusterHost { sw: ClusterSwitch::new(base), rows: vec![], dir: cx.services.dir().to_path_buf(), user: cx.session.user.clone(), role: cx.session.role, workspace: workspace.to_string(), ws_override: false, effective: None, loaded: None, services: cx.services.clone(), audit: cx.audit.clone() };
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
        let fleet = self.services.fleet();
        let visible: Vec<ClusterEntry> = reg.visible_to(self.role).into_iter().cloned().collect();
        if !cfg!(test) && visible.iter().any(|e| !e.kind.is_local()) {
            let run: std::sync::Arc<dyn iyakki_agent::wsexec::Runner> = std::sync::Arc::new(iyakki_agent::wsexec::System);
            clusters::refresh_gpus(&run, &visible, true);
        }
        let status = |e: &ClusterEntry| -> Status {
            let mut st = if e.kind.is_local() { fleet.status(e, false) } else { Self::remote_status(e) };
            if let Some(d) = dot(e) {
                st.dot = d;
            }
            st.gpu = clusters::gpu_summary(&e.id);
            st
        };
        self.rows = rows_for(&reg, self.role, &self.workspace, &status);
    }

    /// A remote cluster's dot from the last GPU look (which is one `kubectl get nodes`): answered, answered long ago, or not checked.
    fn remote_status(e: &ClusterEntry) -> Status {
        let mut st = Status::unknown();
        if let Some(p) = iyakki_agent::aiprofile_remote::clusters_cache().get_key(&e.id) {
            st.dot = if p.stale { Dot::Stale } else { Dot::Running };
            st.last_error = p.error.clone();
        }
        st
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

    /// Feed an event of the switcher; does the switch itself (role-checked, audited) and reports it.
    pub fn handle(&mut self, ev: &Event, dot: &dyn Fn(&ClusterEntry) -> Option<Dot>) -> HostOut {
        match self.sw.handle(ev, &self.rows) {
            SwitchOut::None => HostOut::None,
            SwitchOut::Manage => HostOut::Manage,
            SwitchOut::Switch(id) => {
                let r = if self.ws_override { clusters::switch_workspace(&self.dir, &self.user, self.role, &self.workspace, Some(&id)) } else { clusters::switch(&self.dir, &self.dir, &self.user, self.role, &id) };
                match r {
                    Ok(e) => {
                        self.audit.record(&self.user, "cluster.switch", &format!("{}{}", e.phrase(), if self.ws_override { format!(" (workspace {})", self.workspace) } else { String::new() }));
                        self.reload(dot);
                        HostOut::Switched(e)
                    }
                    Err(why) => HostOut::Refused(why),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "cluster_host_tests.rs"]
mod tests;
