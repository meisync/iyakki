//! The real implementation of the facets of `iyakki_agent_api::facets_b3` (F-SPLIT-03, lane q-split-b3): each call is one function of the
//! agent module it replaces for the UI.

use std::path::Path;

use std::collections::BTreeSet;
use std::sync::Arc;

use iyakki_agent_api::{AgentFacetsB3, AuditLine, AvAddonFacet, BrowserSourceFacet, ClientMediaFacet, ComputerHooksFacet, ComputerPage, ProfileLockFacet, ServicesFacetB3, ToolWrapFacetB3};
use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::clientmedia::{AudioRoutes, Indicator};
use iyakki_shared::hwprofile::Category;
use iyakki_shared::startup::Registry;
use iyakki_shared::wizardtools::Backend as WizardBackend;

use crate::services::Services;
use iyakki_shared::avaddon::Addon;
use iyakki_shared::profilepolicy::Beat;

use crate::agentapi_gen::AgentApiImpl;

/// A shared VPN manager as the UI's trait object (`Manager::shared` hands out one per person).
#[derive(Clone)]
pub struct VpnHandle(pub Arc<crate::vpn_mgr::Manager>);

impl VpnHandle {
    pub fn arc(m: Arc<crate::vpn_mgr::Manager>) -> Arc<dyn iyakki_agent_api::VpnManager> {
        Arc::new(VpnHandle(m))
    }
}

impl iyakki_agent_api::VpnManager for VpnHandle {
    fn profiles(&self) -> Vec<iyakki_vpn::profile::Profile> {
        crate::vpn_mgr::Manager::profiles(&self.0)
    }
    fn get(&self, id: &str) -> Option<iyakki_vpn::profile::Profile> {
        crate::vpn_mgr::Manager::get(&self.0, id)
    }
    fn is_enabled(&self, id: &str) -> bool {
        crate::vpn_mgr::Manager::is_enabled(&self.0, id)
    }
    fn rows(&self) -> Vec<iyakki_shared::vpn_mgr::Row> {
        crate::vpn_mgr::Manager::rows(&self.0)
    }
    fn summary(&self) -> Option<iyakki_shared::vpn_mgr::Summary> {
        crate::vpn_mgr::Manager::summary(&self.0)
    }
    fn guard_line(&self) -> Option<String> {
        crate::vpn_mgr::Manager::guard_line(&self.0)
    }
    fn status(&self, id: &str) -> iyakki_shared::vpn_mgr::Status {
        crate::vpn_mgr::Manager::status(&self.0, id)
    }
    fn import(&self, name: &str, input: &[u8]) -> Result<iyakki_shared::vpn_mgr::Imported, String> {
        crate::vpn_mgr::Manager::import(&self.0, name, input)
    }
    fn set_settings(&self, id: &str, settings: iyakki_vpn::profile::Settings, now: u64) -> Result<iyakki_vpn::profile::Profile, String> {
        crate::vpn_mgr::Manager::set_settings(&self.0, id, settings, now)
    }
    fn set_enabled(&self, id: &str, on: bool, now: u64) -> Result<iyakki_vpn::profile::Profile, String> {
        crate::vpn_mgr::Manager::set_enabled(&self.0, id, on, now)
    }
    fn remove(&self, id: &str) -> Result<Vec<String>, String> {
        crate::vpn_mgr::Manager::remove(&self.0, id)
    }
    fn retry(&self, id: &str, now: u64) {
        crate::vpn_mgr::Manager::retry(&self.0, id, now)
    }
    fn tick(&self, now: u64) {
        crate::vpn_mgr::Manager::tick(&self.0, now)
    }
    fn ensure_supervised(&self) {
        self.0.ensure_supervised()
    }
    fn set_credentials(&self, id: &str, user: &str, pass: &str) -> Result<(), String> {
        self.0.set_credentials(id, user, pass)
    }
    fn has_credentials(&self, id: &str) -> bool {
        self.0.has_credentials(id)
    }
    fn helper_on(&self) -> Option<bool> {
        Some(self.0.helper_enabled())
    }
    fn set_helper(&self, on: bool) -> Result<(), String> {
        self.0.set_helper_enabled(on)
    }
}

pub struct RealProfileLock;

impl ProfileLockFacet for RealProfileLock {
    fn on_screen_lock(&self, data: &Path, user: &str) -> Result<bool, String> {
        crate::profilepolicy::on_screen_lock(data, user).map_err(|e| e.to_string())
    }
    fn on_screen_unlock(&self, data: &Path, user: &str, vault_key_hex: Option<&str>) -> Result<bool, String> {
        let key = vault_key_hex.and_then(crate::profilestore::ProfileKey::from_hex);
        crate::profilepolicy::on_screen_unlock(data, user, key).map_err(|e| e.to_string())
    }
    fn tick_in_background(&self, data: &Path, user: &str, now: u64, report: Box<dyn Fn(&Beat) + Send>) {
        crate::profilepolicy::tick_in_background(data, user, now, move |b| report(b));
    }
}

pub struct RealAvAddon;

impl AvAddonFacet for RealAvAddon {
    fn detect(&self, data: &Path) -> Addon {
        crate::avaddon::detect(data)
    }
}

pub struct RealClientMedia;

impl ClientMediaFacet for RealClientMedia {
    fn device_holds(&self, data: &Path, device_id: &str) -> Vec<String> {
        crate::clientmedia::device_holds(data, device_id)
    }
    fn set_permission(&self, data: &Path, device_id: &str, name: &str, on: bool, by: &str, now: i64) -> Result<&'static str, String> {
        crate::clientmedia::set_permission(data, device_id, name, on, by, now)
    }
    fn device_role(&self, data: &Path, device_id: &str) -> String {
        crate::clientmedia::role_of(data, device_id).to_string()
    }
    fn set_device_role(&self, data: &Path, device_id: &str, role: &str, by: &str, now: i64) -> Result<(), String> {
        crate::clientmedia::apply_role(data, device_id, role, by, now)
    }
    fn indicator(&self) -> Indicator {
        crate::clientmedia::indicator()
    }
    fn audio_routes(&self) -> Arc<AudioRoutes> {
        crate::clientmedia::audio_routes()
    }
}

pub struct RealBrowserSource;

impl BrowserSourceFacet for RealBrowserSource {
    fn browser_session_active(&self, user: &str) -> bool {
        crate::browsersource::browser_session(user).is_some()
    }
}

pub struct RealToolWraps;

impl ToolWrapFacetB3 for RealToolWraps {
    fn wrap_device_tools(&self, inner: Arc<dyn Tools>, data: &Path, user: &str, admin: bool, audit: AuditLine) -> Arc<dyn Tools> {
        use crate::clientmedia::devicetools::{DeviceTools, RealBackend};
        Arc::new(DeviceTools::new(inner, Arc::new(RealBackend { dir: data.to_path_buf(), user: user.to_string(), admin, audit })))
    }
    fn wrap_restart_tools(&self, inner: Arc<dyn Tools>, data: &Path, policy: &Policy, user: &str, audit: AuditLine) -> Arc<dyn Tools> {
        Arc::new(crate::devmode::restarttools::RestartTools::new(inner, crate::devmode::DevMode::open(data), policy.clone(), user, audit))
    }
    fn wrap_wizard_tools(&self, inner: Arc<dyn Tools>, backend: Arc<dyn WizardBackend>) -> Arc<dyn Tools> {
        Arc::new(crate::wizardtools::WizardTools::new(inner, backend))
    }
}

pub struct RealComputerHooks;

impl ComputerHooksFacet for RealComputerHooks {
    fn display_size_get(&self, data: &Path, user: &str) -> Option<Option<usize>> {
        crate::hwprofile::display_size_get(data, user)
    }
    fn display_size_set(&self, data: &Path, user: &str, level: usize) {
        crate::hwprofile::display_size_set(data, user, level)
    }
}

/// The facets that need the services of this install.
pub struct ServicesB3 {
    svc: Arc<Services>,
}

impl Services {
    /// The facets of the lane q-split-b3 over these services: what the UI holds instead of `iyakki_agent::startup_probe`, `hwprofile`, ...
    pub fn b3(self: &Arc<Self>) -> Arc<dyn ServicesFacetB3> {
        Arc::new(ServicesB3 { svc: self.clone() })
    }
}

impl ServicesFacetB3 for ServicesB3 {
    fn startup_poll(&self, reg: &Registry, firewall_alert: bool) {
        crate::startup_probe::poll(&self.svc, reg, firewall_alert)
    }
    fn startup_retry(&self, id: &str, user: &str) -> Result<String, String> {
        crate::startup_probe::retry(&self.svc, id, user)
    }
    fn kube_resolver(&self, user: Option<&str>) {
        match user {
            Some(u) => crate::kubeprofiles::set_resolver(Some(crate::kubeprofiles_svc::resolver_for(&self.svc, u))),
            None => crate::kubeprofiles::set_resolver(None),
        }
    }
    fn computer_attach(&self) -> bool {
        crate::hwprofile_svc::attach_real(&self.svc).is_some()
    }
    fn computer_announce(&self, admins: &[String]) -> bool {
        crate::hwprofile_svc::computers_of(&self.svc).is_some_and(|c| crate::hwprofile_svc::announce_new_computer(&self.svc, &c, admins))
    }
    fn computer_page(&self) -> Option<ComputerPage> {
        let c = crate::hwprofile_svc::computers_of(&self.svc)?;
        let all = c.list().into_iter().map(|p| (p.id, p.label, p.last_seen)).collect();
        let me_seen = c.profile().map(|p| (p.boots, p.first_seen, p.last_seen));
        Some(ComputerPage { me_id: c.me.id.clone(), me_label: c.me.label.clone(), shared: c.shared() as BTreeSet<Category>, me_seen, all })
    }
    fn computer_set_sharing(&self, cat: Category, shared: bool) -> Result<String, String> {
        crate::hwprofile_svc::set_sharing(&self.svc, cat, shared)
    }
    fn computer_copy_from(&self, name: &str) -> Result<String, String> {
        crate::hwprofile_svc::copy_from(&self.svc, name)
    }
    fn computer_reset(&self) -> Result<String, String> {
        crate::hwprofile_svc::reset(&self.svc)
    }
    fn computer_forget(&self, name: &str) -> Result<String, String> {
        crate::hwprofile_svc::forget(&self.svc, name)
    }
    fn wizard_backend(&self, user: &str, audit: AuditLine) -> Arc<dyn WizardBackend> {
        Arc::new(crate::wizardtools::RealBackend { svc: self.svc.clone(), user: user.to_string(), audit })
    }
    fn vpn_manager(&self, user: &str, audit: AuditLine) -> Arc<dyn iyakki_agent_api::VpnManager> {
        VpnHandle::arc(crate::vpn_mgr::Manager::shared(&self.svc, user, audit))
    }
    fn vpn_start_for_login(&self, user: &str, audit: AuditLine) {
        crate::vpn_mgr::start_for_login(&self.svc, user, audit)
    }
    fn vpn_on_logout(&self, user: &str) {
        crate::vpn_mgr::on_logout(&self.svc, user)
    }
    fn wrap_vpn_tools(&self, inner: Arc<dyn Tools>, user: &str, audit: AuditLine) -> Arc<dyn Tools> {
        Arc::new(crate::vpntools::VpnTools::new(inner, crate::vpn_mgr::Manager::shared(&self.svc, user, audit)))
    }
    fn wrap_git_work_tools(&self, inner: Arc<dyn Tools>, user: &str, policy: &Policy, audit: AuditLine) -> Arc<dyn Tools> {
        let env = crate::gitworktools::GitWorkEnv::for_services(&self.svc, user, policy, audit);
        Arc::new(crate::gitworktools::GitWorkTools::new(inner, env))
    }
}

impl AgentFacetsB3 for AgentApiImpl {
    fn client_media(&self) -> &dyn ClientMediaFacet {
        &RealClientMedia
    }
    fn tool_wraps_b3(&self) -> &dyn ToolWrapFacetB3 {
        &RealToolWraps
    }
    fn browser_source(&self) -> &dyn BrowserSourceFacet {
        &RealBrowserSource
    }
    fn computer_hooks(&self) -> &dyn ComputerHooksFacet {
        &RealComputerHooks
    }
    fn avaddon(&self) -> &dyn AvAddonFacet {
        &RealAvAddon
    }
    fn profile_lock(&self) -> &dyn ProfileLockFacet {
        &RealProfileLock
    }
}

#[cfg(test)]
#[path = "facets_b3_tests.rs"]
mod tests;
