//! The real platform behind `iyakki_agent_api::PlatformApi` (F-SPLIT-03): each call is one function of the engine over this `Platform`.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{PlatformApi, ServiceInfo};
use iyakki_shared::devcluster::ClusterProfile;
use iyakki_shared::platform_types::*;

use super::{spec, specs, Platform};

/// A platform as the UI's handle.
pub struct Handle(pub Arc<Platform>);

impl Platform {
    /// This platform as the UI's handle (`Arc<dyn PlatformApi>`).
    pub fn into_api(self) -> Arc<dyn PlatformApi> {
        Arc::new(Handle(Arc::new(self)))
    }
}

fn info(d: &dyn super::ServiceDef) -> ServiceInfo {
    ServiceInfo { id: d.id().to_string(), title: d.title(), kind: d.kind().to_string(), card: d.card(), description: d.description(), backup_hint: d.backup_hint(), local_binary: d.local_binary() }
}

impl PlatformApi for Handle {
    fn dir(&self) -> PathBuf {
        self.0.dir.clone()
    }
    fn user(&self) -> String {
        self.0.user.clone()
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.0.audit)(action, detail)
    }
    fn vault_get(&self, name: &str) -> Result<zeroize::Zeroizing<String>, String> {
        self.0.vault.get(name)
    }
    fn runner(&self) -> Arc<dyn iyakki_shared::wsexec::Runner> {
        self.0.run.clone()
    }
    fn with_live(&self, live: iyakki_shared::wsexec::Sink) -> Option<Arc<dyn PlatformApi>> {
        Some(self.0.with_live(live).into_api())
    }
    fn private_repair(&self, rep: &iyakki_shared::privatemanage::Report, profile: &ClusterProfile, opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        crate::store::privatemanage::repair(&self.0, rep, profile, opts, &crate::ocipush::TcpHttp::default(), progress)
    }
    fn private_prepare(&self, rep: &iyakki_shared::privatemanage::Report, action: &iyakki_shared::privatemanage::Action, typed: &str, force_finalizers: bool) -> Result<Option<iyakki_shared::privatemanage::JobArgs>, String> {
        crate::store::privatemanage::prepare(&self.0, rep, action, typed, force_finalizers)
    }
    fn private_store_state(&self, services: Option<Arc<dyn std::any::Any + Send + Sync>>, cluster: &ClusterRef, namespace: &str) -> iyakki_shared::privatemanage::Report {
        let store = services.and_then(crate::services::Services::from_any).map(|s| crate::store::Store::for_services(&s));
        crate::store::privatemanage::private_store_state(&self.0, store.as_ref(), cluster, namespace)
    }
    fn private_finish_local(&self, services: Arc<dyn std::any::Any + Send + Sync>, rep: &iyakki_shared::privatemanage::Report, action: &iyakki_shared::privatemanage::Action, registry: Option<&ClusterProfile>, say: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        let svc = crate::services::Services::from_any(services).ok_or_else(|| "the services of this install are not the agent's".to_string())?;
        let store = crate::store::Store::for_services(&svc);
        let http = crate::ocipush::TcpHttp::default();
        let target = registry.map(crate::localreg::target_of);
        let reg = target.as_ref().map(|t| (t, &http as &dyn crate::ocipush::RegistryHttp));
        crate::store::privatemanage::finish_local(&self.0, &store, &iyakki_shared::store_install::Actor::system(), rep, action, reg, say)
    }
    fn private_start_job(&self, services: Arc<dyn std::any::Any + Send + Sync>, launch: Option<Arc<dyn std::any::Any + Send + Sync>>, args: &iyakki_shared::privatemanage::JobArgs) -> Result<iyakki_shared::jobs::Job, String> {
        let svc = crate::services::Services::from_any(services).ok_or_else(|| "the services of this install are not the agent's".to_string())?;
        let launch = match launch.and_then(|l| l.downcast::<crate::quickinstall::Launch>().ok()) {
            Some(l) => (*l).clone(),
            None => crate::quickinstall::Launch::this_program(svc.dir())?,
        };
        crate::store::privatemanage::start_job(&svc.jobs, &self.0.user, &launch, args)
    }
    fn tools_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        Arc::new(super::tools::PlatformTools::new(self.0.clone()))
    }
    fn services(&self) -> Vec<ServiceInfo> {
        specs().iter().map(|d| info(d.as_ref())).collect()
    }
    fn service(&self, id: &str) -> Option<ServiceInfo> {
        spec(id).map(|d| info(d.as_ref()))
    }
    fn how_to_use(&self, service: &str, urls: &[UrlInfo], o: &Options) -> String {
        spec(service).map(|d| d.how_to_use(urls, o)).unwrap_or_default()
    }
    fn upgrade_path(&self, service: &str, current: &str) -> Vec<String> {
        spec(service).map(|d| d.upgrade_path(current)).unwrap_or_default()
    }
    fn list_profiles(&self) -> Vec<ListedProfile> {
        super::engine::list_profiles(&self.0)
    }
    fn guard_write(&self, c: &ClusterRef) -> Result<ClusterProfile, String> {
        super::engine::guard_write(&self.0, c)
    }
    fn profile_by_id(&self, id: &str) -> Result<ClusterProfile, String> {
        super::engine::profile_by_id(&self.0, id)
    }
    fn plan_waiting(&self, service: &str, profile: &ClusterProfile, given: &Options, wait: Wait, say: &mut dyn FnMut(&str)) -> Result<Plan, String> {
        let Some(def) = spec(service) else { return Err(format!("unknown service {service}")) };
        super::engine::plan_waiting(&self.0, def.as_ref(), profile, given, wait, say)
    }
    fn plan(&self, service: &str, profile: &ClusterProfile, given: &Options) -> Result<Plan, String> {
        let Some(def) = spec(service) else { return Err(format!("unknown service {service}")) };
        super::engine::plan(&self.0, def.as_ref(), profile, given)
    }
    fn apply(&self, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Applied, String> {
        super::engine::apply(&self.0, plan, approved, progress)
    }
    fn apply_with(&self, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str), http: &dyn iyakki_shared::ocipush::RegistryHttp) -> Result<Applied, String> {
        super::engine::apply_with(&self.0, plan, approved, progress, http)
    }
    fn status(&self, inst: &Installed, check_url: bool) -> Status {
        super::engine::status(&self.0, inst, check_url)
    }
    fn power(&self, inst: &Installed, start: bool, approved: bool) -> Result<Vec<String>, String> {
        super::engine::power(&self.0, inst, start, approved)
    }
    fn plan_upgrade(&self, inst: &Installed, to: Option<&str>, backup_first: bool) -> Result<UpgradePlan, String> {
        super::engine::plan_upgrade(&self.0, inst, to, backup_first)
    }
    fn upgrade(&self, u: &UpgradePlan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
        super::engine::upgrade(&self.0, u, approved, progress)
    }
    fn backup(&self, inst: &Installed, copy_local: bool, approved: bool) -> Result<BackupResult, String> {
        super::engine::backup(&self.0, inst, copy_local, approved)
    }
    fn remove(&self, r: &RemovePlan, confirm: &str, approved: bool) -> Result<Vec<String>, String> {
        super::engine::remove(&self.0, r, confirm, approved)
    }
    fn plan_connect(&self, inst: &Installed, ws: &WsInfo, o: &ConnectOpts) -> Result<ConnectPlan, String> {
        super::connect::plan_connect(&self.0, inst, ws, o)
    }
    fn connect(&self, p: &ConnectPlan, approved: bool) -> Result<Connected, String> {
        super::connect::connect(&self.0, p, approved)
    }
    fn plan_push(&self, inst: &Installed, ws: &WsInfo) -> Result<PushPlan, String> {
        super::connect::plan_push(&self.0, inst, ws)
    }
    fn push(&self, p: &PushPlan, approved: bool) -> Result<String, String> {
        super::connect::push(&self.0, p, approved)
    }
    fn plan_pull_request(&self, inst: &Installed, ws: &WsInfo, base: &str, title: &str, body: &str) -> Result<PrPlan, String> {
        super::connect::plan_pull_request(&self.0, inst, ws, base, title, body)
    }
    fn create_pull_request(&self, p: &PrPlan, approved: bool) -> Result<String, String> {
        super::connect::create_pull_request(&self.0, p, approved)
    }
    fn connect_existing(&self, url: &str, token: &str, cluster: Option<&ClusterRef>, namespace: &str, approved: bool) -> Result<Installed, String> {
        super::gitea::connect_existing(&self.0, url, token, cluster, namespace, approved)
    }
    fn preview(&self, p: &Plan) -> String {
        super::engine::preview(p)
    }
    fn preview_power(&self, inst: &Installed, start: bool) -> String {
        super::engine::preview_power(inst, start)
    }
    fn preview_upgrade(&self, u: &UpgradePlan) -> String {
        super::engine::preview_upgrade(u)
    }
    fn preview_backup(&self, inst: &Installed, copy_local: bool) -> String {
        super::engine::preview_backup(inst, copy_local)
    }
    fn plan_remove(&self, inst: &Installed, delete_data: bool) -> RemovePlan {
        super::engine::plan_remove(inst, delete_data)
    }
    fn preview_remove(&self, r: &RemovePlan) -> String {
        super::engine::preview_remove(r)
    }
    fn preview_connect(&self, p: &ConnectPlan) -> String {
        super::connect::preview_connect(p)
    }
    fn preview_push(&self, p: &PushPlan) -> String {
        super::connect::preview_push(p)
    }
    fn preview_pull_request(&self, p: &PrPlan) -> String {
        super::connect::preview_pull_request(p)
    }
    fn validate_options(&self, service: &str, given: &Options) -> Result<Options, String> {
        let Some(def) = spec(service) else { return Err(format!("unknown service {service}")) };
        super::validate_options(def.as_ref(), given)
    }
    fn common_options(&self, service: &str) -> Vec<OptionDef> {
        let Some(def) = spec(service) else { return Vec::new() };
        super::common_options(def.as_ref())
    }
}

impl crate::services::Services {
    /// The platform of one person on this machine with the given command runner, kubeconfig places and RAM folder (`platform_for` of the panel).
    pub fn platform_with(self: &Arc<Self>, user: &str, run: Arc<dyn iyakki_shared::wsexec::Runner>, env: iyakki_shared::devcluster::Env, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, tmp: PathBuf) -> Arc<dyn PlatformApi> {
        Platform {
            run,
            dir: self.dir().to_path_buf(),
            user: user.to_string(),
            vault: Arc::new(super::secrets::ServicesVault { svc: self.clone(), user: user.to_string() }),
            audit,
            env,
            tmp,
            http: Arc::new(super::connect::NetHttp::default()),
            gen: Arc::new(super::secrets::random_secret),
            live: None,
        }
        .into_api()
    }

    /// The real platform of one person (the system runner, their vault, the real kubeconfig places).
    pub fn platform_real(self: &Arc<Self>, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn PlatformApi> {
        Platform::real(self, user, audit).into_api()
    }
}
