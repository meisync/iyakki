//! The cluster switcher of the Setup tab's popups (alpha.10 AP3, D-AP3-01): the view the tab draws and feeds events, with plain answers.
//! The real switcher is the shell's `ClusterHost` (the tile-k8s widget with the shell's registry and audit), behind `ShellSwitch` in
//! `crates/iyakki-apps/src/apps/store_port.rs`.

use iyakki_cluster_model::clusters::ClusterEntry;
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{Event, R};

/// What a switch event did.
#[derive(Clone, Debug, PartialEq)]
pub enum SwitchOut {
    None,
    /// The cluster is now this one (saved; the tab re-reads what it shows).
    Switched(ClusterEntry),
    /// The switch was refused (role, or the cluster is gone): say why.
    Refused(String),
    /// "Manage clusters..." (the tab asks the shell for Settings, Clusters).
    Manage,
}

pub trait ClusterSwitchView {
    /// Read the registry now and then (never slow).
    fn tick(&mut self);
    /// The cluster the popups deploy to.
    fn effective(&self) -> Option<ClusterEntry>;
    fn visible(&self) -> bool;
    fn width(&self, g: &mut Gfx, s: f32) -> f32;
    fn draw(&mut self, g: &mut Gfx, x: f32, y: f32, max_w: f32, s: f32, accent: [u8; 4]) -> Option<R>;
    fn draw_popup(&mut self, g: &mut Gfx, bounds: R, s: f32, accent: [u8; 4]);
    /// The switch holds the keys (its list is open).
    fn owns_keys(&self) -> bool;
    /// The cluster list is open.
    fn open(&self) -> bool;
    fn handle(&mut self, ev: &Event) -> SwitchOut;
}
