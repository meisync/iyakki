//! The real implementation of the facets of lane b2 (`iyakki-agent-api/src/facets_b2.rs`): every method delegates to the agent module of the
//! same name. `AgentApiImpl` answers `AgentFacetsB2` from here.

use std::any::Any;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent_api::{AgentFacetsB2, ClusterFacet, DictFixFacet, GpuPageFacet, KeyboardAppleFacet, LemonadeComputeFacet, NtpFacet, OwnedProc, ProcOwnerFacet, ProfileStoreFacet, StartupDiskFacet, ToolWrapFacet, WebGraphicsFacet};
use iyakki_core_api::sysinfo::SysRoots;
use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::dictfix::{Context, Fixed, Target};
use iyakki_shared::keyboard::{AppleState, KeyboardSettings};
use iyakki_shared::ntp::Status as NtpStatus;
use iyakki_shared::profilestore::Status as ProfileStatus;
use iyakki_shared::webgraphics::{Mode, Pending, Status as GraphicsStatus};

use crate::agentapi_gen::AgentApiImpl;
use crate::services::Services;

/// The services handle the UI passes (`&cx.services`, an `&Arc<Services>`).
/// The services the UI passes: the real `Arc<Services>`, or the `Arc<dyn ServicesHandle>` the UI holds since the handle flip (the real one inside).
fn svc(a: &dyn Any) -> Option<std::borrow::Cow<'_, Arc<Services>>> {
    if let Some(s) = a.downcast_ref::<Arc<Services>>() {
        return Some(std::borrow::Cow::Borrowed(s));
    }
    let h = a.downcast_ref::<Arc<dyn iyakki_agent_api::ServicesHandle>>()?;
    h.clone().as_any_arc().downcast::<Services>().ok().map(std::borrow::Cow::Owned)
}

const NO_SVC: &str = "unavailable: no services handle";

pub struct RealProfileStore;

impl ProfileStoreFacet for RealProfileStore {
    fn status_of(&self, s: &dyn Any, user: &str) -> ProfileStatus {
        svc(s).map_or(ProfileStatus::Off, |s| crate::profilestore::status_of(&s, user))
    }
    fn protect_for(&self, s: &dyn Any, user: &str) -> Result<String, String> {
        svc(s).ok_or_else(|| NO_SVC.to_string()).and_then(|s| crate::profilestore::protect_for(&s, user))
    }
    fn unprotect_for(&self, s: &dyn Any, user: &str) -> Result<String, String> {
        svc(s).ok_or_else(|| NO_SVC.to_string()).and_then(|s| crate::profilestore::unprotect_for(&s, user))
    }
    fn reset_for(&self, s: &dyn Any, user: &str) -> String {
        svc(s).map_or_else(|| NO_SVC.to_string(), |s| crate::profilestore::reset_for(&s, user))
    }
    fn status(&self, data: &Path, user: &str) -> ProfileStatus {
        crate::profilestore::ProfileStore::new(data, user).status()
    }
    fn is_protected(&self, data: &Path, user: &str) -> bool {
        crate::profilestore::ProfileStore::new(data, user).is_protected()
    }
    fn set_tenant_carrier(&self, tenant: &str, c: Option<std::sync::Arc<dyn iyakki_agent_api::ProfileCarrier>>) {
        crate::profilecarrier::set(tenant, c)
    }
    fn tenant_carriers(&self) -> Vec<String> {
        crate::profilecarrier::names()
    }
    fn send_to_tenant(&self, data: &Path, user: &str, tenant: &str, now: u64) -> Result<u64, String> {
        crate::profilecarrier::send(data, user, tenant, now)
    }
    fn fetch_from_tenant(&self, data: &Path, user: &str, tenant: &str, key_hex: &str, now: u64) -> Result<(), String> {
        crate::profilecarrier::fetch(data, user, tenant, key_hex, now)
    }
    fn export_sealed(&self, data: &Path, user: &str, to: &Path, now: u64) -> Result<u64, String> {
        crate::profilestore::ProfileStore::new(data, user).export_sealed(to, &crate::profilestore::holder_id(data), now).map_err(|e| e.to_string())
    }
    fn import_sealed(&self, data: &Path, user: &str, from: &Path, key_hex: &str, now: u64) -> Result<(), String> {
        let key = crate::profilestore::ProfileKey::from_hex(key_hex).ok_or("the browser key is not valid")?;
        crate::profilestore::ProfileStore::new(data, user).import_sealed(from, &key, &crate::profilestore::holder_id(data), now).map_err(|e| e.to_string())
    }
    fn on_sign_out(&self, data: &Path, user: &str) -> Result<bool, String> {
        crate::profilestore::on_sign_out(data, user).map_err(|e| e.to_string())
    }
    fn on_screen_unlock(&self, data: &Path, user: &str, key_hex: Option<&str>) -> Result<bool, String> {
        let key = key_hex.and_then(crate::profilestore::ProfileKey::from_hex);
        crate::profilepolicy::on_screen_unlock(data, user, key).map_err(|e| e.to_string())
    }
}

pub struct RealNtp;

impl NtpFacet for RealNtp {
    fn status(&self) -> NtpStatus {
        crate::ntp::status()
    }
    fn set_clock(&self, unix: f64) -> Result<(), String> {
        crate::ntp::set_clock(unix)
    }
    fn sync_once(&self, servers: &[String], force_back: bool, timeout: Duration, apply: &dyn Fn(f64) -> Result<(), String>) -> NtpStatus {
        crate::ntp::sync_once(servers, force_back, timeout, apply)
    }
    fn reset_status(&self) {
        crate::ntp::reset_status()
    }
}

pub struct RealStartupDisk;

use iyakki_core_api::startupdisk::{StartupAction, StartupDisk};

impl StartupDiskFacet for RealStartupDisk {
    fn register(&self, backup: &Path) -> Result<String, String> {
        crate::startupdisk::local(Some(backup.to_path_buf())).apply(StartupAction::Register, true)
    }
    fn first(&self, backup: &Path) -> Result<String, String> {
        crate::startupdisk::local(Some(backup.to_path_buf())).apply(StartupAction::First, true)
    }
    fn next(&self, backup: &Path) -> Result<String, String> {
        crate::startupdisk::local(Some(backup.to_path_buf())).apply(StartupAction::Next, true)
    }
    fn revert(&self, backup: &Path) -> Result<String, String> {
        crate::startupdisk::local(Some(backup.to_path_buf())).apply(StartupAction::Revert, true)
    }
    fn status_line(&self, backup: &Path, reboot_setting: bool) -> String {
        crate::startupdisk::local(Some(backup.to_path_buf())).status_line(reboot_setting)
    }
}

pub struct RealProcOwner;

impl ProcOwnerFacet for RealProcOwner {
    fn mine(&self, roots: &SysRoots, who: &str, admin: bool) -> Vec<OwnedProc> {
        crate::procowner::mine(roots, &crate::procowner::claims(), who, admin)
    }
    fn stop(&self, roots: &SysRoots, who: &str, admin: bool, pid: u32, hard: bool) -> Result<String, String> {
        crate::procowner::stop(roots, &crate::procowner::claims(), who, admin, pid, hard)
    }
    fn as_user(&self, user: &str, f: &mut dyn FnMut()) {
        crate::procowner::as_user(user, f)
    }
}

pub struct RealKeyboardApple;

impl KeyboardAppleFacet for RealKeyboardApple {
    fn apple_state(&self, sys: &Path) -> AppleState {
        crate::keyboard::apple_state(sys)
    }
    fn apply_apple(&self, sys: &Path, k: &KeyboardSettings) -> Result<Vec<String>, String> {
        crate::keyboard::apply_apple(sys, k)
    }
    fn set_touchbar_override(&self, mode: Option<u32>) {
        crate::keyboard::set_touchbar_override(mode)
    }
    fn restore_touchbar_mode(&self, sys: &Path, k: &KeyboardSettings) -> usize {
        crate::keyboard::restore_touchbar_mode(sys, k)
    }
}

pub struct RealDictFix;

impl DictFixFacet for RealDictFix {
    fn target(&self, s: &dyn Any) -> Target {
        svc(s).map_or(Target::NoModel, |s| crate::dictfix::target(&s, None))
    }
    fn fix_wording(&self, s: &dyn Any, raw: &str, ctx: &Context) -> Fixed {
        match svc(s) {
            Some(s) => crate::dictfix::fix_wording(&s, raw, ctx),
            None => Fixed::kept(raw, iyakki_shared::dictfix::Reason::NoModel, NO_SVC),
        }
    }
    fn set_enabled(&self, s: &dyn Any, on: bool) -> Result<(), String> {
        svc(s).ok_or_else(|| NO_SVC.to_string()).and_then(|s| crate::dictfix::set_enabled(&s, on))
    }
}

pub struct RealWebGraphics;

impl WebGraphicsFacet for RealWebGraphics {
    fn status(&self, data: &Path) -> GraphicsStatus {
        crate::webgraphics::status(data)
    }
    fn load_mode(&self, data: &Path) -> (Mode, Option<String>) {
        crate::webgraphics::load_mode(data)
    }
    fn set_mode(&self, data: &Path, mode: Mode) -> Result<(), String> {
        crate::webgraphics::set_mode(data, mode)
    }
    fn try_hardware_again(&self, data: &Path, gpu: &str) -> Result<(), String> {
        crate::webgraphics::try_hardware_again(data, gpu)
    }
    fn take_pending(&self) -> Vec<Pending> {
        crate::webgraphics::take_pending()
    }
}

pub struct RealToolWraps;

impl ToolWrapFacet for RealToolWraps {
    fn linux_appimage_tools(&self, inner: Arc<dyn Tools>, s: Arc<dyn Any + Send + Sync>, policy: Policy, user: String, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        match Services::from_any(s) {
            Some(svc) => Arc::new(crate::linuxappimagetools::LinuxAppImageTools::new(inner, Arc::new(crate::linuxappimagetools::RealBackend { svc, policy, user, audit }))),
            None => inner,
        }
    }
}

pub struct RealClusters;

impl ClusterFacet for RealClusters {
    fn switch(&self, dir: &Path, data_dir: &Path, user: &str, role: iyakki_sec::policy::Role, id: &str) -> Result<iyakki_shared::clusters::ClusterEntry, String> {
        crate::clusters::switch(dir, data_dir, user, role, id)
    }
    fn gpu_summary(&self, cluster_id: &str) -> Option<String> {
        crate::clusters::gpu_summary(cluster_id)
    }
    fn refresh_gpus(&self, run: &Arc<dyn iyakki_shared::wsexec::Runner>, entries: &[iyakki_shared::clusters::ClusterEntry], background: bool) -> usize {
        crate::clusters::refresh_gpus(run, entries, background)
    }
}

pub struct RealGpuPage;

impl GpuPageFacet for RealGpuPage {
    fn quick_report(&self, root: &Path, data: &Path) -> iyakki_shared::gpu::Report {
        crate::gpu_steps::quick_report(root, data)
    }
    fn refresh_async(&self, svc: Arc<dyn Any + Send + Sync>) {
        if let Some(svc) = Services::from_any(svc) {
            crate::gpu_steps::refresh_async(svc);
        }
    }
}

pub struct RealLemonadeCompute;

impl LemonadeComputeFacet for RealLemonadeCompute {
    fn lemonade_compute(&self, data: &Path) -> iyakki_shared::gpu_steps::Compute {
        crate::gpu_steps::lemonade_compute(data)
    }
}

impl AgentFacetsB2 for AgentApiImpl {
    fn profile_store(&self) -> &dyn ProfileStoreFacet {
        &RealProfileStore
    }
    fn ntp(&self) -> &dyn NtpFacet {
        &RealNtp
    }
    fn startup_disk(&self) -> &dyn StartupDiskFacet {
        &RealStartupDisk
    }
    fn proc_owner(&self) -> &dyn ProcOwnerFacet {
        &RealProcOwner
    }
    fn keyboard_apple(&self) -> &dyn KeyboardAppleFacet {
        &RealKeyboardApple
    }
    fn dict_fix(&self) -> &dyn DictFixFacet {
        &RealDictFix
    }
    fn web_graphics(&self) -> &dyn WebGraphicsFacet {
        &RealWebGraphics
    }
    fn lemonade_compute(&self) -> &dyn LemonadeComputeFacet {
        &RealLemonadeCompute
    }
    fn tool_wraps(&self) -> &dyn ToolWrapFacet {
        &RealToolWraps
    }
    fn cluster_registry(&self) -> &dyn ClusterFacet {
        &RealClusters
    }
    fn gpu_page(&self) -> &dyn GpuPageFacet {
        &RealGpuPage
    }
}
