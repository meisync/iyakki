//! The K8S tile's real port (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12): the only place that reads the session, the
//! audit log, the core's files, the k3s service and the agent's deploy kit for the tile (`iyakki_tile_k8s`, alpha.8 E7). The cluster client, the managed
//! cluster's state and its Start are the tile's `Rig`; the real rig comes from here.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_cluster_model::cloudlaunch::{LaunchSpec, Plan};
use iyakki_k8s_model::k8s::Kube;
use iyakki_k8s_model::k8sexp::WorkReport;
use iyakki_sec::policy::Role;
use iyakki_shared::clusters::ClusterEntryExt;
use iyakki_shared::k3sdev::Svc;
use iyakki_tile_k8s::cluster_host::ClusterHost;
use iyakki_tile_k8s::port::CloudKit;
pub use iyakki_tile_k8s::port::K8sPort;
use iyakki_tile_k8s::{ClusterEntry, FwdRow, FwdSpec, Managed, Rig};

use super::AppCx;

/// The agent's deploy kit, as the tile's cloud kit (the types are the same leaf types, so every call forwards).
struct AgentKit(Arc<dyn iyakki_agent_api::DeployKit>);

impl CloudKit for AgentKit {
    fn cloud_default_image(&self) -> String {
        self.0.cloud_default_image()
    }
    fn cloud_plan(&self, l: &LaunchSpec) -> Plan {
        self.0.cloud_plan(l)
    }
    fn cloud_preview(&self, l: &LaunchSpec, p: &Plan) -> String {
        self.0.cloud_preview(l, p)
    }
    fn cloud_execute(&self, l: &LaunchSpec, p: &Plan, work: &std::path::Path, kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        self.0.cloud_execute(l, p, work, kubectl, push)
    }
    fn cloud_cli(&self, args: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.0.cloud_cli(args, out)
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    /// The real rig: the cluster client of an entry, the managed cluster's state and its Start (the k3s service of this machine).
    pub fn rig(&self) -> Rig {
        if crate::TESTING {
            return Rig::offline();
        }
        let svc = self.0.k3sdev().clone();
        let (s1, s2) = (svc.clone(), svc.clone());
        Rig {
            kube_for: Arc::new(move |e: &ClusterEntry| -> Arc<dyn Kube> {
                let run: Arc<dyn iyakki_shared::wsexec::Runner> = crate::agent().exec().system();
                if e.managed && s1.installed().is_some_and(|s| s.backend == iyakki_shared::k3sdev::Backend::Podman) {
                    Arc::new(iyakki_shared::devcluster::PodKube { run, container: iyakki_shared::k3sdev::POD_NAME.into() })
                } else {
                    Arc::new(iyakki_shared::devcluster::RunnerKube { run })
                }
            }),
            managed: Arc::new(move || {
                let h = svc.health();
                let m = match svc.state() {
                    Svc::NotInstalled => Managed::NotSet,
                    Svc::Stopped => Managed::Stopped,
                    Svc::Starting => Managed::Starting { waiting_net: !h.note.is_empty() },
                    Svc::Running => Managed::Running,
                    Svc::Stopping => Managed::Stopping,
                    Svc::Failed(r) => Managed::Failed(r),
                };
                let native = svc.installed().is_some_and(|s| s.backend == iyakki_shared::k3sdev::Backend::Native);
                (m, native.then(|| svc.layout().kubeconfig()))
            }),
            start: Arc::new(move |user: &str| s2.clone().start(user)),
            local_kubeconfig: Arc::new(|dir, e| e.kubeconfig_for(dir)),
        }
    }
}

impl K8sPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn role(&self) -> Role {
        self.0.session.role
    }
    fn data_dir(&self) -> PathBuf {
        self.0.data_dir().to_path_buf()
    }
    fn audit(&self, action: &str, detail: &str) {
        self.0.audit.record(&self.0.session.user, action, detail);
    }
    fn read_text(&self, path: &str, max: usize) -> Result<String, String> {
        self.0.core.files().read_text(&self.0.session.user, path, max)
    }
    fn home_root(&self) -> PathBuf {
        self.0.core.file_scope().home_root(&self.0.session.user, self.0.data_dir())
    }
    fn allows_write(&self, path: &str) -> bool {
        self.0.session.policy.allows_write(&self.0.session.user, path)
    }
    fn work_report(&self) -> Option<Arc<dyn WorkReport + Send + Sync>> {
        self.0.b5().work_report(self.0.data_dir(), &self.0.session.user)
    }
    fn cluster_host(&self, base: usize, workspace: &str) -> ClusterHost {
        super::cluster_host::new(base, self.0, workspace)
    }
    fn cloud_kit(&self) -> Arc<dyn CloudKit> {
        Arc::new(AgentKit(crate::agent().helpers_b6().deploy_kit()))
    }
    fn host_save_enabled(&self) -> bool {
        iyakki_shared::hostfiles::enabled()
    }
    fn has_ide_workspaces(&self) -> bool {
        !iyakki_shared::workspace::Registry::load(self.0.data_dir(), &self.0.session.user).items.is_empty()
    }
    fn instant(&self) -> bool {
        crate::TESTING
    }
    fn forwards(&self) -> Vec<FwdRow> {
        self.0.os.port_forwards().list().iter().map(row_of).collect()
    }
    fn start_forward(&self, spec: &FwdSpec) -> Result<FwdRow, String> {
        let kind = iyakki_core_api::portforward::Target::parse(&spec.kind).ok_or("the kind is pod, svc or deployment")?;
        let req = iyakki_core_api::portforward::Request { kubeconfig: spec.kubeconfig.clone(), context: spec.context.clone(), namespace: spec.namespace.clone(), kind, name: spec.name.clone(), remote: spec.remote, local: spec.local };
        self.0.os.port_forwards().start(&req).map(|i| row_of(&i))
    }
    fn stop_forward(&self, id: u32) -> bool {
        self.0.os.port_forwards().stop(id)
    }
}

/// The core's forward as the tile's plain row.
fn row_of(i: &iyakki_core_api::portforward::Info) -> FwdRow {
    FwdRow { id: i.id, namespace: i.namespace.clone(), target: i.target.clone(), remote: i.remote, local: i.local }
}
