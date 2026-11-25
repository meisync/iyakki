//! The K8S tile (alpha.8 E7): pick a cluster and a namespace and explore it (`docs/K8S-TILE.md`), with the cluster switcher it shares with the AI IDE
//! and the Launch cloud mode card. A crate that depends on the toolkit (`iyakki-uikit`) and the pure leaf crates `iyakki-k8s-model` (the explorer's
//! engine), `iyakki-cluster-model` (the registry and the cloud plan) and `iyakki-sec` (the role) only. Everything it needs from the rest of the
//! system is [`port::K8sPort`] (with the cloud deploy kit [`port::CloudKit`]) and [`cluster_host::ClusterHostPort`]; the shell adapter
//! (`crates/iyakki-apps/src/apps/k8s_tile.rs`) and the real ports (`k8s_port.rs`, `cluster_host.rs`) are the only code that names the services.

pub use iyakki_uikit::ctxmenu;
pub(crate) use iyakki_uikit::{fx, gfx, ide_chrome, safeclamp, ui, widgets};

pub mod cloud;
pub mod cluster_host;
pub mod cluster_switch;
pub mod forwards;
pub mod port;
pub mod tile;
pub mod ui_draw;

/// What a screen's event does to the shell: stay, or go back to the home screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

pub use forwards::{FwdRow, FwdSpec};
pub use port::{CloudKit, K8sPort, NoCloudKit};
pub use tile::*;

#[cfg(test)]
mod fake_tests;
