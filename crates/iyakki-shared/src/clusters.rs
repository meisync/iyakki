//! The cluster registry: the data and its rules live in `iyakki-cluster-model` (alpha.8 E7) and are re-exported here under the old path; what a
//! cluster entry needs from the machine (the managed k3s layout, the vault) stays here.

use std::path::Path;

pub use iyakki_cluster_model::clusters::*;

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
