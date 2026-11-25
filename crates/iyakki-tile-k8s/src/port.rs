//! What the K8S tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile names no `Services`, no session and no audit log: it asks a [`K8sPort`] for who is signed in, the audit line, the person's files
//! and file rules, the work ledger of an action, the cluster switcher and the cloud deploy kit. The cluster client, the managed cluster's state
//! and its Start stay the tile's `Rig` (tests replace it); the real rig comes from the shell's port (`crates/iyakki-apps/src/apps/k8s_port.rs`),
//! the only place that reads the session, the audit log, the core's files and the k3s service.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_cluster_model::cloudlaunch::{LaunchSpec, Plan};
use iyakki_k8s_model::k8sexp::WorkReport;
use iyakki_sec::policy::Role;

use crate::cluster_host::ClusterHost;

/// The deploy kit of "Launch cloud mode" (the real one is the agent's `DeployKit`): the plan, its preview words and the steps that apply it.
pub trait CloudKit: Send + Sync {
    /// The image a cloud tenant runs unless the person names another.
    fn cloud_default_image(&self) -> String;
    /// What launching cloud mode would do (and what blocks it), as a plan.
    fn cloud_plan(&self, l: &LaunchSpec) -> Plan;
    /// The words of the preview: exactly what would be applied.
    fn cloud_preview(&self, l: &LaunchSpec, p: &Plan) -> String;
    /// Applies the plan: `kubectl` runs one call, `push` pushes the image; the lines it said.
    fn cloud_execute(&self, l: &LaunchSpec, p: &Plan, work: &Path, kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String>;
    /// `iyakki cloud ...` (example, manifests, secret, reach ...): lines go to `out`.
    fn cloud_cli(&self, args: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String>;
}

/// A kit that does nothing: the default of a port without cloud mode (every plan is blocked, saying so).
pub struct NoCloudKit;

const NO_KIT: &str = "cloud mode is not available here";

impl CloudKit for NoCloudKit {
    fn cloud_default_image(&self) -> String {
        String::new()
    }
    fn cloud_plan(&self, _l: &LaunchSpec) -> Plan {
        Plan { blockers: vec![NO_KIT.into()], ..Default::default() }
    }
    fn cloud_preview(&self, _l: &LaunchSpec, _p: &Plan) -> String {
        NO_KIT.into()
    }
    fn cloud_execute(&self, _l: &LaunchSpec, _p: &Plan, _work: &Path, _kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, _push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        Err(NO_KIT.into())
    }
    fn cloud_cli(&self, _args: &[String], _out: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err(NO_KIT.into())
    }
}

pub trait K8sPort {
    /// The signed-in person's name.
    fn user(&self) -> String;
    fn role(&self) -> Role;
    /// The data folder the person's choices and the cluster registry are kept in.
    fn data_dir(&self) -> PathBuf;
    /// One audit line as the signed-in person.
    fn audit(&self, action: &str, detail: &str);
    /// A text file under the person's own file rules (the core's file facet), at most `max` bytes.
    fn read_text(&self, path: &str, max: usize) -> Result<String, String>;
    /// The person's home folder.
    fn home_root(&self) -> PathBuf;
    /// The person's policy lets them write `path`.
    fn allows_write(&self, path: &str) -> bool;
    /// Where an action's steps are written down for the work ledger, when there is one.
    fn work_report(&self) -> Option<Arc<dyn WorkReport + Send + Sync>>;
    /// The cluster switcher of this person (the event codes from `base`; `workspace` is empty outside the AI IDE).
    fn cluster_host(&self, base: usize, workspace: &str) -> ClusterHost;
    /// The deploy kit of "Launch cloud mode".
    fn cloud_kit(&self) -> Arc<dyn CloudKit> {
        Arc::new(NoCloudKit)
    }
    /// The window host takes a saved file (a dialog) instead of the tile writing it into the home folder.
    fn host_save_enabled(&self) -> bool {
        false
    }
    /// The person has AI IDE workspaces (the tile then offers the IDE hint).
    fn has_ide_workspaces(&self) -> bool {
        false
    }
    /// The port-forwards that run (the core's one list: the tile, the chat tool and `dev forward` share it).
    fn forwards(&self) -> Vec<crate::forwards::FwdRow> {
        Vec::new()
    }
    /// Start one. The core asks the person's card (`k8s.portforward`) before anything runs.
    fn start_forward(&self, _spec: &crate::forwards::FwdSpec) -> Result<crate::forwards::FwdRow, String> {
        Err("port-forwards are not available here".into())
    }
    /// Stop one by number; false when there is none.
    fn stop_forward(&self, _id: u32) -> bool {
        false
    }
    /// Tests: no waiting before a selection loads its details.
    fn instant(&self) -> bool {
        false
    }
}
