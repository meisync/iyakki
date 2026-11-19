//! The platform services (deploy, upgrade, back up, remove, connect a workspace) as the UI uses them: one trait over the agent's engine, so
//! the UI holds `Arc<dyn PlatformApi>` and never names `Platform`. The data is in `iyakki_shared::platform_types`.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_shared::devcluster::ClusterProfile;
use iyakki_shared::platform_types::*;

pub const UNAVAILABLE_PLATFORM: &str = "unavailable: this core does not serve this call";

/// What the UI shows of one service.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub card: String,
    pub description: String,
    pub backup_hint: String,
    pub local_binary: Option<LocalBinary>,
}

/// The platform services of one person on one machine.
pub trait PlatformApi: Send + Sync {
    /// The folder of the person's settings and records.
    fn dir(&self) -> PathBuf {
        PathBuf::new()
    }
    fn user(&self) -> String {
        String::new()
    }
    /// One audit line of the platform (`platform.<action>`).
    fn audit(&self, _action: &str, _detail: &str) {}
    /// A secret of the person's vault by name (the platform keeps its admin passwords there).
    fn vault_get(&self, _name: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    /// The command runner of this platform (the system, or a fake in tests).
    fn runner(&self) -> Arc<dyn iyakki_shared::wsexec::Runner>;
    /// The same platform with live output going to `live`.
    fn with_live(&self, _live: iyakki_shared::wsexec::Sink) -> Option<Arc<dyn PlatformApi>> {
        None
    }
    /// The platform tools of the chat (an opaque handle only the real core can use).
    fn tools_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        Arc::new(())
    }
    /// Repair the private store: what the report says is wrong is put right (no data is deleted); the lines say what was done.
    fn private_repair(&self, _rep: &iyakki_shared::privatemanage::Report, _profile: &ClusterProfile, _opts: &Options, _progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    /// Check a reset or removal and make the arguments of its job (`None`: nothing to run).
    fn private_prepare(&self, _rep: &iyakki_shared::privatemanage::Report, _action: &iyakki_shared::privatemanage::Action, _typed: &str, _force_finalizers: bool) -> Result<Option<iyakki_shared::privatemanage::JobArgs>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    /// What state the private store is in (read only). `services` is the install's services (type-erased) when there are any.
    fn private_store_state(&self, _services: Option<Arc<dyn std::any::Any + Send + Sync>>, _cluster: &ClusterRef, _namespace: &str) -> iyakki_shared::privatemanage::Report;
    /// The device side of a reset or removal, after the cluster side ran (`registry`: also delete the local registry of that cluster).
    fn private_finish_local(&self, _services: Arc<dyn std::any::Any + Send + Sync>, _rep: &iyakki_shared::privatemanage::Report, _action: &iyakki_shared::privatemanage::Action, _registry: Option<&ClusterProfile>, _say: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    /// Start the background job of a reset or removal. `launch` is the program to run (`None`: this program); `services` the install's services.
    fn private_start_job(&self, _services: Arc<dyn std::any::Any + Send + Sync>, _launch: Option<Arc<dyn std::any::Any + Send + Sync>>, _args: &iyakki_shared::privatemanage::JobArgs) -> Result<iyakki_shared::jobs::Job, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    /// Every service that can be deployed.
    fn services(&self) -> Vec<ServiceInfo> {
        Vec::new()
    }
    fn service(&self, _id: &str) -> Option<ServiceInfo> {
        None
    }
    fn how_to_use(&self, _service: &str, _urls: &[UrlInfo], _o: &Options) -> String {
        String::new()
    }
    fn upgrade_path(&self, _service: &str, _current: &str) -> Vec<String> {
        Vec::new()
    }
    fn list_profiles(&self) -> Vec<ListedProfile> {
        Vec::new()
    }
    fn guard_write(&self, _c: &ClusterRef) -> Result<ClusterProfile, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn profile_by_id(&self, _id: &str) -> Result<ClusterProfile, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan_waiting(&self, _service: &str, _profile: &ClusterProfile, _given: &Options, _wait: Wait, _say: &mut dyn FnMut(&str)) -> Result<Plan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan(&self, _service: &str, _profile: &ClusterProfile, _given: &Options) -> Result<Plan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn apply(&self, _plan: &Plan, _approved: bool, _progress: &mut dyn FnMut(&str)) -> Result<Applied, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn apply_with(&self, _plan: &Plan, _approved: bool, _progress: &mut dyn FnMut(&str), _http: &dyn iyakki_shared::ocipush::RegistryHttp) -> Result<Applied, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn status(&self, _inst: &Installed, _check_url: bool) -> Status {
        Status { state: SvcState::Unreachable, ready: 0, desired: 0, pods: Vec::new(), live_images: Vec::new(), notes: Vec::new(), reachable: None }
    }
    fn power(&self, _inst: &Installed, _start: bool, _approved: bool) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan_upgrade(&self, _inst: &Installed, _to: Option<&str>, _backup_first: bool) -> Result<UpgradePlan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn upgrade(&self, _u: &UpgradePlan, _approved: bool, _progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn backup(&self, _inst: &Installed, _copy_local: bool, _approved: bool) -> Result<BackupResult, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn remove(&self, _r: &RemovePlan, _confirm: &str, _approved: bool) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan_connect(&self, _inst: &Installed, _ws: &WsInfo, _o: &ConnectOpts) -> Result<ConnectPlan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn connect(&self, _p: &ConnectPlan, _approved: bool) -> Result<Connected, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan_push(&self, _inst: &Installed, _ws: &WsInfo) -> Result<PushPlan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn push(&self, _p: &PushPlan, _approved: bool) -> Result<String, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn plan_pull_request(&self, _inst: &Installed, _ws: &WsInfo, _base: &str, _title: &str, _body: &str) -> Result<PrPlan, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn create_pull_request(&self, _p: &PrPlan, _approved: bool) -> Result<String, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn connect_existing(&self, _url: &str, _token: &str, _cluster: Option<&ClusterRef>, _namespace: &str, _approved: bool) -> Result<Installed, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn preview(&self, _p: &Plan) -> String {
        String::new()
    }
    fn preview_power(&self, _inst: &Installed, _start: bool) -> String {
        String::new()
    }
    fn preview_upgrade(&self, _u: &UpgradePlan) -> String {
        String::new()
    }
    fn preview_backup(&self, _inst: &Installed, _copy_local: bool) -> String {
        String::new()
    }
    fn plan_remove(&self, inst: &Installed, delete_data: bool) -> RemovePlan {
        RemovePlan { inst: inst.clone(), delete_data }
    }
    fn preview_remove(&self, _r: &RemovePlan) -> String {
        String::new()
    }
    fn preview_connect(&self, _p: &ConnectPlan) -> String {
        String::new()
    }
    fn preview_push(&self, _p: &PushPlan) -> String {
        String::new()
    }
    fn preview_pull_request(&self, _p: &PrPlan) -> String {
        String::new()
    }
    fn validate_options(&self, _service: &str, _given: &Options) -> Result<Options, String> {
        Err(UNAVAILABLE_PLATFORM.into())
    }
    fn common_options(&self, _service: &str) -> Vec<OptionDef> {
        Vec::new()
    }
}
