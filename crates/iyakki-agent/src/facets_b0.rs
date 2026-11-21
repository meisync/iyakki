//! The real implementation of `iyakki_agent_api::facets_b0` (F-SPLIT-03, bucket b0): each method delegates to the agent module of its name.

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{B0Facets, CheckupFacet, B0Services, GeminiFacet, IdeBrowserFacet, QuickInstallFacet, LinuxAppsFacet, McpFacet, MediaFacet, MediaPlay, OciPushFacet, ProvidersFacet, QuietFacet, TargetFacet};
use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::checkup::{Inventory, ScreenOut};
use iyakki_shared::disks::Actor;
use iyakki_shared::idebrowser::OpenRequest;
use iyakki_shared::jobs::Job;
use iyakki_shared::mcp::{ServerSpec, StoredServer};
use iyakki_shared::quickinstall_uninstall::{Opts, Plan};
use iyakki_shared::quickinstall_ollama::HostInfo;
use iyakki_shared::quickinstall::{HwFirmwareOpts, Facts, Item, Latest, Launch, Presence};
use iyakki_shared::media::{Check, QueueState};
use iyakki_shared::ocipush::RegistryHttp;
use iyakki_shared::quiet::Env as QuietEnv;
use iyakki_shared::target::{ChangePlan, Target};

use crate::services::Services;

/// The stateless facets of bucket b0 (`AgentFacets::b0`).
pub struct RealB0;

impl B0Facets for RealB0 {
    fn providers(&self) -> &dyn ProvidersFacet {
        self
    }
    fn quiet(&self) -> &dyn QuietFacet {
        self
    }
    fn target(&self) -> &dyn TargetFacet {
        self
    }
    fn idebrowser(&self) -> &dyn IdeBrowserFacet {
        self
    }
    fn ocipush(&self) -> &dyn OciPushFacet {
        self
    }
    fn linuxapps(&self) -> &dyn LinuxAppsFacet {
        self
    }
    fn media(&self) -> &dyn MediaFacet {
        self
    }
    fn gemini(&self) -> &dyn GeminiFacet {
        self
    }
    fn mcp(&self) -> &dyn McpFacet {
        self
    }
    fn checkup(&self) -> &dyn CheckupFacet {
        self
    }
    fn quickinstall(&self) -> &dyn QuickInstallFacet {
        self
    }
}

impl QuickInstallFacet for RealB0 {
    fn order_titles_with(&self, steps: &[Item], before: &dyn Fn(Item) -> Vec<&'static str>) -> Vec<String> {
        crate::quickinstall::order_titles_with(steps, before)
    }
    fn ollama_host_info(&self, data: &Path) -> HostInfo {
        crate::quickinstall::ollama_cluster::real_host_info(data)
    }
    fn browser_package_card(&self, data: &Path, upstream: Arc<dyn iyakki_proxy::Upstream>) -> Result<Vec<String>, String> {
        crate::browserpkg_tools::card_lines(data, upstream)
    }
}

impl CheckupFacet for RealB0 {
    fn inventory(&self, screens: Vec<ScreenOut>) -> Inventory {
        crate::checkup::read_inventory(screens)
    }
    fn tone_wav(&self) -> Vec<u8> {
        crate::checkup::tone_wav()
    }
}

impl GeminiFacet for RealB0 {
    fn voice_enabled(&self, data: &Path) -> bool {
        crate::gemini::load(data).voice.enabled
    }
    fn voice_key(&self, data: &Path) -> String {
        crate::gemini::load(data).voice.key
    }
    fn voice_wake_handoff(&self, data: &Path) -> bool {
        crate::gemini::load(data).voice.wake_handoff
    }
}

impl McpFacet for RealB0 {
    fn demo_spec(&self) -> ServerSpec {
        crate::mcp::demo_spec()
    }
    fn origin_of(&self, stored: &[StoredServer], name: &str) -> &'static str {
        crate::mcp::origin_of(stored, name)
    }
}

impl MediaFacet for RealB0 {
    fn player(&self) -> Arc<dyn MediaPlay> {
        crate::media::shared()
    }
}

impl MediaPlay for crate::media::MediaPlayer {
    fn plays_mp3_in_process(&self) -> bool {
        crate::media::MediaPlayer::plays_mp3_in_process(self)
    }
    fn play(&self, user: &str, label: &str, target: &str) -> Result<(), String> {
        crate::media::MediaPlayer::play(self, user, label, target)
    }
    fn set_queue(&self, user: &str, items: Vec<String>, pos: usize, active: bool) -> u64 {
        crate::media::MediaPlayer::set_queue(self, user, items, pos, active)
    }
    fn queue_state(&self) -> QueueState {
        crate::media::MediaPlayer::queue_state(self)
    }
    fn skip(&self, delta: i64) -> Option<String> {
        crate::media::MediaPlayer::skip(self, delta)
    }
    fn stop(&self) {
        crate::media::MediaPlayer::stop(self)
    }
    fn check(&self) -> Check {
        crate::media::MediaPlayer::check(self)
    }
    fn now_target(&self) -> Option<String> {
        crate::media::MediaPlayer::now_target(self)
    }
    fn elapsed(&self) -> Option<u64> {
        crate::media::MediaPlayer::elapsed(self)
    }
    fn now_playing(&self) -> Option<String> {
        crate::media::MediaPlayer::now_playing(self)
    }
}

impl ProvidersFacet for RealB0 {
    fn set_thinking_sink(&self, sink: Option<Box<dyn FnMut(&str)>>) {
        crate::providers::set_thinking_sink(sink)
    }
}

impl QuietFacet for RealB0 {
    fn system(&self) -> Arc<dyn QuietEnv> {
        Arc::new(crate::quiet::System)
    }
}

impl TargetFacet for RealB0 {
    fn plan_change(&self, root: &Path, name: &str, to: Target) -> ChangePlan {
        crate::target::plan_change(root, name, to)
    }
    fn apply_change(&self, root: &Path, name: &str, to: Target) -> Result<(ChangePlan, String), String> {
        crate::target::apply_change(root, name, to)
    }
}

impl IdeBrowserFacet for RealB0 {
    fn offered(&self, policy: &Policy) -> bool {
        crate::idebrowser::IdeBrowserTools::offered(policy)
    }
    fn take_requests(&self, user: &str, ws: &str) -> Vec<OpenRequest> {
        crate::idebrowser::take_requests(user, ws)
    }
}

impl OciPushFacet for RealB0 {
    fn http(&self) -> Arc<dyn RegistryHttp> {
        Arc::new(crate::ocipush::TcpHttp::default())
    }
}

impl LinuxAppsFacet for RealB0 {
    fn set_workspace_sandbox(&self, data: &Path, user: &str, exe: &Path, root: &Path, name: Option<&str>) -> Result<String, String> {
        let la = crate::linuxapps::LinuxApps::new(data, user, exe, Arc::new(crate::wsexec::System));
        crate::linuxapps::set_workspace_sandbox(&la, root, name)
    }
}

/// The facets of bucket b0 over one person's services.
pub struct ServicesB0 {
    svc: Arc<Services>,
}

impl Services {
    /// The b0 facets over these services (`cx.services.b0()`).
    pub fn b0(self: &Arc<Self>) -> Arc<dyn B0Services> {
        Arc::new(ServicesB0 { svc: self.clone() })
    }
}

impl B0Services for ServicesB0 {
    fn tz_refresh_in_background(&self, force: bool, on_change: Arc<dyn Fn(&str) + Send + Sync>) -> bool {
        crate::tzdetect::refresh_in_background(&self.svc, force, on_change)
    }
    fn ide_browser_tools(&self, inner: Arc<dyn Tools>, policy: Policy, user: &str, ws: &str, root: &Path) -> Arc<dyn Tools> {
        Arc::new(crate::idebrowser::IdeBrowserTools::new(inner, self.svc.clone(), policy, user, ws, root))
    }
    fn qi_presence(&self, item: Item) -> Presence {
        crate::quickinstall::presence(&self.svc, item)
    }
    fn qi_install_jobs(&self, owner: &str) -> Vec<Job> {
        crate::quickinstall::install_jobs(&self.svc.jobs, owner)
    }
    fn qi_start(&self, owner: &str, target: Item, present: &dyn Fn(Item) -> bool, launch: &Launch) -> Result<Vec<Job>, String> {
        crate::quickinstall::start(&self.svc.jobs, owner, target, present, launch)
    }
    fn qi_start_firmware_pack(&self, owner: &str, item: Item, from: Option<&Path>, launch: &Launch) -> Result<Job, String> {
        crate::quickinstall::start_firmware_pack(&self.svc.jobs, owner, item, from, launch)
    }
    fn qi_check_upgrades(&self) -> Vec<(Item, Latest)> {
        crate::quickinstall::check_upgrades(&self.svc)
    }
    fn qi_gather_facts(&self) -> Facts {
        crate::quickinstall::gather_facts(&self.svc)
    }
    fn qi_service_requests(&self) -> Vec<Result<String, String>> {
        crate::quickinstall::service_requests(&self.svc)
    }
    fn qi_uninstall_plan(&self, item: Item, user: &str) -> (Plan, Option<String>) {
        let ctx = crate::quickinstall::uninstall::real_ctx(&self.svc, user, item);
        (crate::quickinstall::uninstall::plan(item, &ctx), ctx.cli_warning.clone())
    }
    fn qi_uninstall(&self, item: Item, user: &str, opts: Opts, progress: Option<Arc<std::sync::Mutex<String>>>) -> Result<String, String> {
        crate::quickinstall::uninstall::uninstall_real(&self.svc, user, item, opts, progress)
    }
    fn qi_start_camera_firmware(&self, owner: &str, launch: &Launch) -> Result<Job, String> {
        crate::quickinstall::start_camera_firmware(&self.svc.jobs, owner, launch)
    }
    fn qi_start_mac_firmware(&self, owner: &str, source: &iyakki_shared::macfw::Source, launch: &Launch) -> Result<Job, String> {
        crate::quickinstall::start_mac_firmware(&self.svc.jobs, owner, source, launch)
    }
    fn qi_start_gpu_test(&self, owner: &str, launch: &Launch) -> Result<Job, String> {
        self.svc.jobs.start(owner, &crate::quickinstall::gpu_test_spec(launch), &|_: &Path| Ok(()))
    }
    fn qi_start_hardware_firmware(&self, owner: &str, opts: &HwFirmwareOpts, launch: &Launch) -> Result<Job, String> {
        crate::quickinstall::start_hardware_firmware(&self.svc.jobs, owner, opts, launch)
    }
    fn share_tools(&self, inner: Arc<dyn Tools>, admin: bool, policy: Policy, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        let dir = self.svc.dir();
        let allow = crate::sharedfolder::allow_fn_for(admin, policy, user, dir);
        let scope = crate::sharedfolder::scope_for(admin, user, dir);
        let a2 = audit.clone();
        let shares = crate::sharedfolder::for_user(dir, user, a2, allow, scope);
        let domains = admin.then(|| crate::domains::Manager::shared(&self.svc, user, audit));
        Arc::new(crate::sharetools::ShareTools::new(inner, Arc::new(crate::sharetools::RealApi { shares, svc: self.svc.clone(), user: user.to_string(), domains })))
    }
    fn shares_restore(&self, admin: bool, policy: Policy, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) {
        let dir = self.svc.dir();
        let allow = crate::sharedfolder::allow_fn_for(admin, policy, user, dir);
        let scope = crate::sharedfolder::scope_for(admin, user, dir);
        crate::sharedfolder::for_user(dir, user, audit, allow, scope).restore();
    }
    fn disks_format_tools(&self, inner: Arc<dyn Tools>, who: Actor, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        let engine = Arc::new(crate::disks::Disks::real());
        engine.set_tools(Arc::new(crate::disks_fmt::ProcTools::real(&self.svc.clis.bin_dir())));
        let dir = self.svc.dir().to_path_buf();
        Arc::new(crate::disksformattools::DisksFormatTools::new(inner, Arc::new(crate::disksformattools::RealBackend { engine, who, dir, audit })))
    }
}
