//! `DevCluster` and `FleetApi` (iyakki-agent-api) over the supervisor of a managed k3s cluster and the fleet that holds them.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{DevCluster, FleetApi};
use iyakki_shared::clusters::{ClusterEntry, Registry, Status};
use iyakki_shared::k3sdev::{Backend, Health, Instance, Layout, Plan, State, Svc};

use crate::clusters::Fleet;
use crate::k3sdev::K3sDev;
use crate::wsexec::Out;

impl DevCluster for K3sDev {
    fn layout(&self) -> &Layout {
        K3sDev::layout(self)
    }
    fn installed(&self) -> Option<State> {
        K3sDev::installed(self)
    }
    fn state(&self) -> Svc {
        K3sDev::state(self)
    }
    fn health(&self) -> Health {
        K3sDev::health(self)
    }
    fn install(&self, plan: &Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String> {
        K3sDev::install(self, plan, st, user)
    }
    fn uninstall(&self, typed: &str, user: &str) -> Result<(), String> {
        K3sDev::uninstall(self, typed, user)
    }
    fn stop(&self, user: &str) -> Result<(), String> {
        K3sDev::stop(self, user)
    }
    fn start(self: Arc<Self>, user: &str) -> Result<(), String> {
        K3sDev::start(&self, user)
    }
    fn restart(self: Arc<Self>, user: &str) -> Result<(), String> {
        K3sDev::restart(&self, user)
    }
    fn upgrade(self: Arc<Self>, plan: &Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String> {
        K3sDev::upgrade(&self, plan, st, user)
    }
    fn upgrade_plan(&self, st: &dyn iyakki_proxy::Streamer) -> Result<Option<Plan>, String> {
        K3sDev::upgrade_plan(self, st)
    }
    fn reset(&self, typed: &str, user: &str) -> Result<(), String> {
        K3sDev::reset(self, typed, user)
    }
    fn refresh_health(&self) -> Health {
        K3sDev::refresh_health(self)
    }
    fn situation(&self) -> iyakki_shared::k3sdev::Situation {
        K3sDev::situation(self)
    }
    fn describe(&self, plan: &Plan) -> String {
        K3sDev::describe(self, plan)
    }
    fn log_tail(&self, lines: usize) -> String {
        K3sDev::log_tail(self, lines)
    }
    fn take_alerts(&self) -> Vec<iyakki_shared::k3sdev::Alert> {
        K3sDev::take_alerts(self)
    }
    fn set_audit(&self, f: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) {
        K3sDev::set_audit(self, f)
    }
    fn set_fixed_node_ip(&self, on: bool, user: &str) -> Result<(), String> {
        K3sDev::set_fixed_node_ip(self, on, user)
    }
    fn fixed_node_ip_enabled(&self) -> bool {
        K3sDev::fixed_node_ip_enabled(self)
    }
    fn set_autostart(&self, on: bool, user: &str) -> Result<(), String> {
        K3sDev::set_autostart(self, on, user)
    }
    fn kubectl(&self, args: &[&str]) -> Out {
        K3sDev::kubectl(self, args)
    }
    fn kubectl_reader(self: Arc<Self>) -> Arc<dyn iyakki_shared::kubectl::Kubectl> {
        self
    }
    fn modelhost_kube(self: Arc<Self>) -> Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync> {
        self
    }
    fn kubectl_cap(&self, args: &[&str], cap: usize) -> Out {
        K3sDev::kubectl_cap(self, args, cap)
    }
}

impl FleetApi for Fleet {
    fn get(&self, e: &ClusterEntry) -> Option<Arc<dyn DevCluster>> {
        Fleet::get(self, e).map(|d| d as Arc<dyn DevCluster>)
    }
    fn status(&self, e: &ClusterEntry, test_remote: bool) -> Status {
        Fleet::status(self, e, test_remote)
    }
    fn start(&self, reg: &Registry, id: &str, user: &str, role: iyakki_sec::policy::Role) -> Result<(), String> {
        Fleet::start(self, reg, id, user, role)
    }
    fn stop(&self, reg: &Registry, id: &str, user: &str, role: iyakki_sec::policy::Role) -> Result<(), String> {
        Fleet::stop(self, reg, id, user, role)
    }
    fn drop_instance(&self, id: &str) {
        Fleet::drop_instance(self, id)
    }
    fn data_dir(&self) -> PathBuf {
        Fleet::data_dir(self).to_path_buf()
    }
    fn lookup(&self, st: &dyn iyakki_proxy::Streamer, layout: &Layout, backend: Backend, registry: &str, inst: Option<&Instance>) -> Result<Plan, String> {
        match inst {
            Some(i) => crate::k3sdev::lookup_for(&self.clis(), st, layout, backend, registry, i),
            None => crate::k3sdev::lookup(&self.clis(), st, layout, backend, registry),
        }
    }
}

impl iyakki_agent_api::OllamaApi for crate::ollama::OllamaServer {
    fn state(&self) -> iyakki_shared::ollama::OllamaState {
        crate::ollama::OllamaServer::state(self)
    }
    fn url(&self) -> String {
        crate::ollama::OllamaServer::url(self)
    }
    fn models_dir(&self) -> PathBuf {
        crate::ollama::OllamaServer::models_dir(self)
    }
    fn start(self: Arc<Self>) -> Result<(), String> {
        crate::ollama::OllamaServer::start(&self)
    }
    fn stop(&self) -> Result<(), String> {
        crate::ollama::OllamaServer::stop(self)
    }
    fn set_port(&self, port: u16) {
        crate::ollama::OllamaServer::set_port(self, port)
    }
}

impl iyakki_agent_api::SandboxCluster for crate::cluster::Cluster {
    fn state(&self) -> iyakki_shared::cluster::ClusterState {
        crate::cluster::Cluster::state(self)
    }
    fn requirements(&self) -> Vec<String> {
        crate::cluster::Cluster::requirements(self)
    }
    fn start(self: Arc<Self>) -> Result<(), String> {
        crate::cluster::Cluster::start(&self)
    }
    fn stop(&self) -> Result<(), String> {
        crate::cluster::Cluster::stop(self)
    }
    fn sandbox_run_io(&self, language_name: &str, code: &str, stdin: &str, timeout: u64, network: bool, cancel: &std::sync::atomic::AtomicBool) -> Result<iyakki_shared::cluster::SandboxResult, String> {
        crate::cluster::Cluster::sandbox_run_io(self, language_name, code, stdin, timeout, network, cancel)
    }
}

impl iyakki_agent_api::CliApi for crate::clis::CliManager {
    fn export_path(&self) {
        crate::clis::CliManager::export_path(self)
    }
    fn set_detect_present(&self, on: bool) {
        crate::clis::CliManager::set_detect_present(self, on)
    }
    fn installed(&self) -> Vec<iyakki_shared::clis::Installed> {
        crate::clis::CliManager::installed(self)
    }
    fn is_installed(&self, id: &str) -> Option<iyakki_shared::clis::Installed> {
        crate::clis::CliManager::is_installed(self, id)
    }
    fn system_status(&self, id: &str) -> Option<iyakki_shared::clis::SystemStatus> {
        crate::clis::CliManager::system_status(self, id)
    }
    fn job(&self, id: &str) -> Option<iyakki_shared::clis::Job> {
        crate::clis::CliManager::job(self, id)
    }
    fn running(&self) -> bool {
        crate::clis::CliManager::running(self)
    }
    fn install_plan(&self, id: &str) -> Option<String> {
        crate::clis::CliManager::install_plan(self, id)
    }
    fn manifest_dir(&self) -> PathBuf {
        crate::clis::CliManager::manifest_dir(self)
    }
    fn bin_dir(&self) -> PathBuf {
        crate::clis::CliManager::bin_dir(self)
    }
    fn search_path(&self) -> String {
        crate::clis::CliManager::search_path(self)
    }
    fn install(self: Arc<Self>, id: &str, by: &str) -> Result<(), String> {
        crate::clis::CliManager::install(&self, id, by)
    }
    fn rollback(&self, id: &str) -> Result<String, String> {
        crate::clis::CliManager::rollback(self, id)
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        crate::clis::CliManager::remove(self, id)
    }
}
