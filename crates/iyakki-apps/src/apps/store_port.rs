//! What the STORE tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The Store is moved in steps: the files that name no shell context ask a [`StorePort`] for the signed-in person, their rights, the audit line,
//! the data folder, the agent facade, the quick-install facet, the core and the few helpers that still take the shell context (the cluster
//! switcher, the coding agents' sign-in state). The real port ([`CxPort`]) is the only place that reads the session and calls the services; a
//! fake hands the tile in-memory answers (`store_setup_fake_tests.rs`). Step 1 (alpha.9 lane P3): `store_setup.rs`, `store_setup_view.rs` and
//! `store_setup_remove.rs` (the SETUP section). `store.rs`, `store_manage.rs`, `store_rating.rs` and `store_drivers.rs` follow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent_api::{AgentApi, B0Services, DevCluster, SettingsRef};
use iyakki_agent_api::StoreApi;
use iyakki_core_api::Core;

use super::AppCx;

pub trait StorePort {
    /// The signed-in person.
    fn user(&self) -> String;
    /// The name of the person's role (for the lines that say why a change is refused).
    fn role_name(&self) -> String;
    /// The person manages the system settings (administrators).
    fn manage_settings(&self) -> bool;
    /// The person may restart the machine.
    fn can_power(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn record(&self, action: &str, detail: &str);
    /// The person's policy (what the catalog, the lifecycle and the removal check).
    fn policy(&self) -> iyakki_sec::policy::Policy;
    /// The audit log, for a worker thread that records when it is done.
    fn recorder(&self) -> Arc<dyn iyakki_tile_store::engine::AuditSink>;
    /// The data folder of this install.
    fn data_dir(&self) -> &Path;
    /// The person may read this path (the core's file scope).
    fn check_read(&self, path: &Path) -> Result<(), String>;

    /// The narrow store facets (the update report, the drivers, the private store, the permission words).
    fn facets(&self) -> Arc<dyn iyakki_agent_api::StoreFacets>;
    /// The machine as the core reads it, for the Drivers chip.
    fn drivers_read(&self) -> Arc<dyn iyakki_tile_store::drivers::DriversView>;
    /// The AI hardware profile of this machine (the Setup tab reads what the AI tools are shown, recommended or hidden by).
    fn ai_profile(&self) -> Arc<dyn iyakki_tile_store::ai::AiProfilePort>;
    /// The hardware facts the Setup tab reads (graphics, camera, Mac firmware outcomes, voice pack helpers).
    fn setup_hw(&self) -> Arc<dyn iyakki_tile_store::setup_hw::SetupHwPort>;
    /// The services and add-on side of the Setup tab (the managed k3s node, the add-on's build route, the firmware card).
    fn setup_svc(&self) -> Arc<dyn iyakki_tile_store::setup_svc::SetupSvcPort>;
    /// The host facts the Setup tab reads (memory, product name, the sysfs root, the uptime) and whether cloud mode is on.
    fn machine_facts(&self) -> iyakki_tile_store::shell::HostFacts;
    fn cloud_on(&self) -> bool;
    /// A speech engine is set in Settings, Voice (listening or reading).
    fn voice_engines_set(&self) -> bool;
    /// The Setup tab's background work (the package card read, the voice pack removal).
    fn shell_jobs(&self) -> Arc<dyn iyakki_tile_store::shell::ShellJobs>;
    /// What the store screen reads from the rest of the system (the System update, the private store, the sticks, the update check,
    /// the pending licence).
    fn facts(&self) -> Arc<dyn iyakki_tile_store::facts::StoreFacts>;
    /// The drivers' install side (the packs for a folder or a stick, the camera firmware).
    fn drivers_install(&self) -> Arc<dyn iyakki_tile_store::facts::DriversInstall>;
    /// The shell's test switch (the tests run the screen at once, without threads).
    fn testing(&self) -> bool;
    /// The quick-install facet: facts, jobs, removal, the upgrade check.
    fn b0(&self) -> Arc<dyn B0Services>;
    /// The same services in mirror types (what the Setup page calls).
    fn qi(&self) -> Arc<dyn iyakki_tile_api::store_qi::QuickInstall>;
    /// The second facet of the services (the voice pack removal).
    fn b4(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB4>;
    /// The agent facade (firmware, the AI profile, the add-on build, the helpers).
    fn agent(&self) -> Arc<dyn AgentApi>;
    /// The core (the host, the cloud probe).
    fn core(&self) -> Arc<dyn Core>;
    /// Restart the machine (the Setup card "Reboot to finish").
    fn reboot_machine(&self);
    /// The system settings.
    fn settings(&self) -> SettingsRef;
    /// The development cluster's state.
    fn k3sdev(&self) -> Arc<dyn DevCluster>;
    /// The network reader the quick install uses (tests give an offline one).
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream>;
    /// Whether all AI tools are shown, and the switch.
    fn show_all_ai(&self) -> bool;
    fn set_show_all_ai(&self, on: bool) -> Result<(), String>;

    /// The publisher keys the person trusts (key, label).
    fn trusted_keys(&self) -> Vec<(String, String)>;

    /// The cluster switcher of this person on this screen (event codes `base..base + 10 + rows`).
    fn cluster_host(&self, base: usize, workspace: &str) -> Box<dyn iyakki_tile_store::switch::ClusterSwitchView>;
    /// The request the switcher's "Manage clusters..." makes (the shell opens Settings, Clusters).
    fn manage_request(&self) -> (String, String);
    /// Whether the coding agent is signed in (from the remembered status; no program runs).
    fn agent_login_state(&self, agent: &str, installed: bool) -> iyakki_tile_store::shell::LoginState;
    /// The coding agent of a CLI id (the shell's sign-in table), `None` for a tool that is not one.
    fn agent_of_cli(&self, cli: &str) -> Option<&'static str>;
    /// Ask the shell to start (or end) the sign-in of this coding agent.
    fn login_request(&self, agent: &'static str, logout: bool);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl StorePort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn role_name(&self) -> String {
        self.0.session.role.name().to_string()
    }
    fn manage_settings(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn can_power(&self) -> bool {
        self.0.session.policy.power
    }
    fn record(&self, action: &str, detail: &str) {
        self.0.audit.record(&self.0.session.user, action, detail);
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        self.0.session.policy.clone()
    }
    fn recorder(&self) -> Arc<dyn iyakki_tile_store::engine::AuditSink> {
        Arc::new(AgentAudit(self.0.audit.clone()))
    }
    fn facets(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        self.0.storeapi()
    }
    fn drivers_read(&self) -> Arc<dyn iyakki_tile_store::drivers::DriversView> {
        Arc::new(CoreDrivers(self.0.os.drivers()))
    }
    fn ai_profile(&self) -> Arc<dyn iyakki_tile_store::ai::AiProfilePort> {
        Arc::new(AgentAi(self.0.agent.ai_profile().machine(self.0.data_dir())))
    }
    fn setup_hw(&self) -> Arc<dyn iyakki_tile_store::setup_hw::SetupHwPort> {
        Arc::new(AgentSetupHw(self.0.agent.clone()))
    }
    fn setup_svc(&self) -> Arc<dyn iyakki_tile_store::setup_svc::SetupSvcPort> {
        Arc::new(AgentSetupSvc { agent: self.0.agent.clone(), dev: self.0.k3sdev() })
    }
    fn testing(&self) -> bool {
        crate::TESTING
    }
    fn facts(&self) -> Arc<dyn iyakki_tile_store::facts::StoreFacts> {
        Arc::new(AgentFacts { facets: self.0.storeapi(), core: self.0.core.clone(), settings: self.0.settings() })
    }
    fn drivers_install(&self) -> Arc<dyn iyakki_tile_store::facts::DriversInstall> {
        Arc::new(AgentDriversInstall(self.0.storeapi().drivers()))
    }
    fn machine_facts(&self) -> iyakki_tile_store::shell::HostFacts {
        let c = self.0.core.clone();
        let h = c.host();
        iyakki_tile_store::shell::HostFacts { mem_total_kb: h.mem_total_kb(), product_name: h.product_name(), sys_root: h.sys_root(), uptime_secs: h.uptime_secs() }
    }
    fn cloud_on(&self) -> bool {
        self.0.core.clone().cloud().detect().on
    }
    fn voice_engines_set(&self) -> bool {
        let Ok(st) = self.0.settings().lock() else { return false };
        let v = &st.get().voice;
        v.stt.is_some() || v.tts.is_some()
    }
    fn shell_jobs(&self) -> Arc<dyn iyakki_tile_store::shell::ShellJobs> {
        Arc::new(AgentJobs { agent: self.0.agent.clone(), b4: self.0.b4(), up: self.0.quick_upstream() })
    }
    fn data_dir(&self) -> &Path {
        self.0.data_dir()
    }
    fn check_read(&self, path: &Path) -> Result<(), String> {
        self.0.core.file_scope().check_read(&self.0.session.policy, &self.0.session.user, path)
    }
    fn b0(&self) -> Arc<dyn B0Services> {
        self.0.b0()
    }
    fn qi(&self) -> Arc<dyn iyakki_tile_api::store_qi::QuickInstall> {
        install_table();
        Arc::new(RealQi { b0: self.0.b0(), agent: self.0.agent.clone() })
    }
    fn b4(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB4> {
        self.0.b4()
    }
    fn agent(&self) -> Arc<dyn AgentApi> {
        self.0.agent.clone()
    }
    fn core(&self) -> Arc<dyn Core> {
        self.0.core.clone()
    }
    fn reboot_machine(&self) {
        self.0.host.reboot();
    }
    fn settings(&self) -> SettingsRef {
        self.0.settings()
    }
    fn k3sdev(&self) -> Arc<dyn DevCluster> {
        self.0.k3sdev()
    }
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        self.0.quick_upstream()
    }
    fn show_all_ai(&self) -> bool {
        self.0.services.show_all_ai()
    }
    fn set_show_all_ai(&self, on: bool) -> Result<(), String> {
        self.0.services.set_show_all_ai(on)
    }
    fn trusted_keys(&self) -> Vec<(String, String)> {
        self.0.services.packages().trusted_keys()
    }
    fn cluster_host(&self, base: usize, workspace: &str) -> Box<dyn iyakki_tile_store::switch::ClusterSwitchView> {
        Box::new(ShellSwitch(super::cluster_host::new(base, self.0, workspace)))
    }
    fn manage_request(&self) -> (String, String) {
        (super::cluster_host::MANAGE_REQUEST.0.into(), super::cluster_host::MANAGE_REQUEST.1.into())
    }
    fn agent_login_state(&self, agent: &str, installed: bool) -> iyakki_tile_store::shell::LoginState {
        use iyakki_tile_store::shell::LoginState as L;
        match crate::agent_login::state(self.0, agent, installed) {
            crate::agent_login::State::NotInstalled => L::NotInstalled,
            crate::agent_login::State::Checking => L::Checking,
            crate::agent_login::State::SignedOut => L::SignedOut,
            crate::agent_login::State::SignedIn(who) => L::SignedIn(who),
            crate::agent_login::State::Unknown(why) => L::Unknown(why),
        }
    }
    fn agent_of_cli(&self, cli: &str) -> Option<&'static str> {
        crate::agent_login::agent_of_cli(cli)
    }
    fn login_request(&self, agent: &'static str, logout: bool) {
        crate::agent_login::request(agent, logout)
    }
}

// ---- The cluster switcher (alpha.10 AP3, D-AP3-01): the shell's ClusterHost (the tile-k8s widget with the shell's registry and audit) seen
// through the tile's `ClusterSwitchView`.

/// The shell's cluster switcher.
pub struct ShellSwitch(pub super::cluster_host::ClusterHost);

impl iyakki_tile_store::switch::ClusterSwitchView for ShellSwitch {
    fn tick(&mut self) {
        self.0.tick(&|_| None)
    }
    fn effective(&self) -> Option<iyakki_cluster_model::clusters::ClusterEntry> {
        self.0.effective().cloned()
    }
    fn visible(&self) -> bool {
        self.0.visible()
    }
    fn width(&self, g: &mut iyakki_uikit::gfx::Gfx, s: f32) -> f32 {
        self.0.width(g, s)
    }
    fn draw(&mut self, g: &mut iyakki_uikit::gfx::Gfx, x: f32, y: f32, max_w: f32, s: f32, accent: [u8; 4]) -> Option<iyakki_uikit::ui::R> {
        self.0.draw(g, x, y, max_w, s, accent)
    }
    fn draw_popup(&mut self, g: &mut iyakki_uikit::gfx::Gfx, bounds: iyakki_uikit::ui::R, s: f32, accent: [u8; 4]) {
        self.0.draw_popup(g, bounds, s, accent);
    }
    fn owns_keys(&self) -> bool {
        self.0.owns_keys()
    }
    fn open(&self) -> bool {
        self.0.sw.open
    }
    fn handle(&mut self, ev: &iyakki_uikit::ui::Event) -> iyakki_tile_store::switch::SwitchOut {
        use iyakki_tile_k8s::cluster_host::HostOut as H;
        use iyakki_tile_store::switch::SwitchOut as S;
        match self.0.handle(ev, &|_| None) {
            H::None => S::None,
            H::Switched(e) => S::Switched(e),
            H::Refused(why) => S::Refused(why),
            H::Manage => S::Manage,
        }
    }
}

// ---- What the store screen reads (alpha.10 AP3): the agent's store facets, the core's pending licence and the settings' update check.

/// The agent's store facets, the core's app data and the settings.
pub struct AgentFacts {
    pub facets: Arc<dyn iyakki_agent_api::StoreFacets>,
    pub core: Arc<dyn Core>,
    pub settings: SettingsRef,
}

impl iyakki_tile_store::facts::StoreFacts for AgentFacts {
    fn system_update_out(&self) -> Option<(String, String)> {
        use iyakki_store_model::store_updatecheck::Kind;
        self.facets.update_report().and_then(|r| r.updates.into_iter().find(|u| u.kind == Kind::System)).map(|u| (u.to.to_string(), u.from.to_string()))
    }
    fn private_store(&self, user: &str) -> iyakki_store_model::store_private::StatusInfo {
        self.facets.private_store_status(user)
    }
    fn stick_stores(&self) -> Vec<PathBuf> {
        self.facets.stick_stores()
    }
    fn update_check_daily(&self) -> Option<bool> {
        self.settings.lock().map(|s| s.get().update_check.mode == iyakki_store_model::store_updatecheck::Mode::Daily).ok()
    }
    fn set_update_check_daily(&self, daily: bool) -> bool {
        use iyakki_store_model::store_updatecheck::Mode;
        self.settings.lock().map(|mut s| s.update(|s| s.update_check.mode = if daily { Mode::Daily } else { Mode::Manual }).is_ok()).unwrap_or(false)
    }
    fn pending_licence(&self, user: &str, app: &str, key: &str) -> Option<String> {
        self.core.app_data().get(user, app, key)
    }
    fn clear_pending_licence(&self, user: &str, app: &str, key: &str) {
        let _ = self.core.app_data().remove(user, app, key);
    }
    fn permission_words(&self, permission: &str, user: &str) -> Option<String> {
        self.facets.permission_words(permission, user)
    }
    fn update_check_inputs(&self) -> (iyakki_store_model::store_updatecheck::CheckSettings, String, String) {
        let settings = self.settings.lock().map(|s| s.get().update_check.clone()).unwrap_or_default();
        let rel = iyakki_shared::relinfo::ReleaseInfo::parse(&std::fs::read_to_string(iyakki_shared::relinfo::RELEASE_FILE).unwrap_or_default());
        let running = if rel.version.is_empty() { iyakki_shared::relinfo::VERSION.to_string() } else { rel.version };
        (settings, running, rel.variant)
    }
    fn driver_title(&self, id: &str) -> Option<String> {
        iyakki_core_api::drivers::known::by_id(id).map(|k| k.title.to_string())
    }
}

/// The agent's drivers install side.
pub struct AgentDriversInstall(pub Arc<dyn iyakki_agent_api::StoreDrivers>);

impl iyakki_tile_store::facts::DriversInstall for AgentDriversInstall {
    fn folder_package(&self, id: &str, dir: &Path, now: i64) -> Result<(String, Vec<String>), String> {
        self.0.folder_package(id, dir, now)
    }
    fn facetime_package(&self, driver_file: &[u8], accepted: bool, now: i64) -> Result<(String, String), String> {
        self.0.facetime_package(driver_file, accepted, now)
    }
    fn title_of(&self, id: &str) -> Option<String> {
        iyakki_core_api::drivers::known::by_id(id).map(|k| k.title.to_string())
    }
}

// ---- The Setup tab's background work (alpha.10 AP3, D-AP3-01): the package card read and the voice pack removal, on the threads the screen starts.

/// The agent's handles the Setup tab's threads use.
pub struct AgentJobs {
    pub agent: Arc<dyn AgentApi>,
    pub b4: Arc<dyn iyakki_agent_api::ServicesFacetB4>,
    pub up: Arc<dyn iyakki_proxy::Upstream>,
}

impl iyakki_tile_store::shell::ShellJobs for AgentJobs {
    fn browser_package_card(&self, data: &Path) -> Result<Vec<String>, String> {
        self.agent.b0().quickinstall().browser_package_card(data, self.up.clone())
    }
    fn voicepack_remove(&self, delete: &dyn Fn(&str) -> Result<(), String>) -> Result<String, String> {
        self.b4.voicepack_remove(delete)
    }
}

// ---- The Setup tab's services (alpha.10 AP3, D-AP3-02): the managed k3s node, the add-on build route and the firmware card, seen through
// the tile's `SetupSvcPort`.

/// The agent's services for the Setup tab: the agent facade and the managed development cluster.
pub struct AgentSetupSvc {
    pub agent: Arc<dyn AgentApi>,
    pub dev: Arc<dyn DevCluster>,
}

impl iyakki_tile_store::setup_svc::SetupSvcPort for AgentSetupSvc {
    fn k3s_state(&self) -> iyakki_tile_store::setup_svc::K3s {
        use iyakki_shared::k3sdev::Svc as S;
        use iyakki_tile_store::setup_svc::K3s as K;
        match self.dev.state() {
            S::NotInstalled => K::NotInstalled,
            S::Stopped => K::Stopped,
            S::Starting => K::Starting,
            S::Running => K::Running,
            S::Stopping => K::Stopping,
            S::Failed(why) => K::Failed(why),
        }
    }
    fn k3s_start(&self, user: &str) -> Result<(), String> {
        Arc::clone(&self.dev).start(user)
    }
    fn k3s_stop(&self, user: &str) -> Result<(), String> {
        self.dev.stop(user)
    }
    fn av_map_text(&self) -> String {
        std::fs::read_to_string(Path::new(iyakki_shared::avbuild::BRIDGE_DIR).join("versions.map")).unwrap_or_default()
    }
    fn av_consent(&self, map_text: &str) -> Result<Vec<String>, String> {
        self.agent.av_build().consent(map_text)
    }
    fn av_write_request(&self, data: &Path) -> Result<(), String> {
        self.agent.avbuild_write_request(data)
    }
    fn av_confirm_title(&self) -> String {
        iyakki_shared::avbuild::CONFIRM_TITLE.to_string()
    }
    fn av_no_k3s_text(&self) -> String {
        iyakki_shared::avbuild::NO_K3S_TEXT.to_string()
    }
    fn avaddon_plan(&self, data: &Path) -> iyakki_tile_store::setup_svc::AddonPlan {
        let p = self.agent.helpers_b6().avaddon_plan(data);
        iyakki_tile_store::setup_svc::AddonPlan { has_source: p.has_source, review: p.review }
    }
    fn datasci_state(&self, data: &Path) -> (String, String) {
        self.agent.helpers_b4().datasci_state(data)
    }
    fn datasci_verify(&self, data: &Path, user: &str) -> Result<String, String> {
        self.agent.helpers_b4().datasci_verify(data, user)
    }
    fn hw_view_now(&self, data: &Path, home: &Path) -> iyakki_tile_store::setup_svc::HwStatus {
        let v = self.agent.hw_firmware().view_now(data, home);
        iyakki_tile_store::setup_svc::HwStatus {
            summary: v.summary,
            vendors: v.vendors,
            offers: v.offers.into_iter().map(|o| iyakki_tile_store::setup_svc::HwOffer { ready: o.ready, needs_click: o.needs_click, line: o.line }).collect(),
            staging: v.staging,
            stick: v.stick,
            link_iface: v.link_iface,
        }
    }
    fn hw_accept(&self, data: &Path, with: &[String], user: &str, now: u64) -> Result<iyakki_tile_store::setup_svc::HwAccepted, String> {
        self.agent.hw_firmware().accept(data, with, user, now).map(|a| iyakki_tile_store::setup_svc::HwAccepted { files: a.files, digest16: a.digest16 })
    }
}

// ---- The Setup tab's hardware facts (alpha.10 AP3, D-AP3-02, D-AP3-03 (2) and (3)): the agent's answers and the graphics read, seen through
// the tile's `SetupHwPort`.

/// The agent's hardware answers for the Setup tab.
pub struct AgentSetupHw(pub Arc<dyn iyakki_agent_api::AgentApi>);

impl iyakki_tile_store::setup_hw::SetupHwPort for AgentSetupHw {
    fn gpu_facts(&self) -> iyakki_tile_store::setup_hw::GpuFacts {
        let gpus = iyakki_shared::gpu::detect(Path::new("/"));
        iyakki_tile_store::setup_hw::GpuFacts {
            count: gpus.iter().filter(|g| !g.is_virtual()).count(),
            nvidia: iyakki_shared::gpu::has_nvidia(&gpus),
            amd_rocm: iyakki_shared::gpu_rocm::any_usable(&gpus),
        }
    }
    fn camera_in(&self, sys_root: &Path) -> bool {
        self.0.camfw_camera_in(sys_root)
    }
    fn firmware_plan_empty(&self, data: &Path) -> bool {
        self.0.hw_firmware().plan_is_empty(data)
    }
    fn window_session(&self) -> bool {
        self.0.webengine_host_session()
    }
    fn camera_outcome(&self, data: &Path) -> Option<iyakki_tile_store::setup_hw::FwNote> {
        self.0.camfw_read_outcome(data).map(|o| iyakki_tile_store::setup_hw::FwNote { headline: o.headline, installed: o.installed })
    }
    fn camera_leftovers(&self, data: &Path) -> u64 {
        self.0.camfw_leftovers(data)
    }
    fn camera_delete_leftovers(&self, data: &Path) -> u64 {
        self.0.camfw_delete_leftovers(data)
    }
    fn mac_outcome(&self, data: &Path) -> Option<iyakki_tile_store::setup_hw::FwNote> {
        self.0.macfw_read_outcome(data).map(|o| iyakki_tile_store::setup_hw::FwNote { headline: o.headline, installed: o.installed })
    }
    fn mac_card_status(&self, data: &Path) -> String {
        self.0.macfw_card_status(data)
    }
    fn voicepack_review_lines(&self, lemonade_there: bool) -> Vec<String> {
        self.0.voicepack_review_lines(lemonade_there)
    }
    fn voicepack_delete_model(&self, model: &str) -> Result<(), String> {
        self.0.helpers_b4().voicepack_delete_local_model(model)
    }
}

// ---- The AI hardware profile (alpha.10 AP3, D-AP3-03 (1)): the agent's profile of this machine, seen through the tile's port.

/// The agent's AI profile of this machine.
#[derive(Debug)]
pub struct AgentAi(pub Arc<dyn iyakki_agent_api::AiMachine>);

impl iyakki_tile_store::ai::AiProfilePort for AgentAi {
    fn summary(&self) -> String {
        self.0.summary()
    }
    fn visibility(&self, item_id: &str, show_all: bool) -> iyakki_tile_store::ai::Vis {
        use iyakki_shared::aiprofile::Vis as A;
        use iyakki_tile_store::ai::Vis as V;
        match self.0.visibility(item_id, show_all) {
            A::Show => V::Show,
            A::ShownBecause(r) => V::ShownBecause(r),
            A::Recommended(r) => V::Recommended(r),
            A::NotRecommended(r) => V::NotRecommended(r),
            A::Hidden(r) => V::Hidden(r),
        }
    }
    fn hidden_items(&self) -> Vec<(String, String)> {
        self.0.hidden_items()
    }
}

// ---- The Drivers chip's read side (alpha.10 AP3, D-AP3-02): the core's driver read as the tile's plain view. The words come from the
// catalogue's own labels here, so the tile copies none of them.

/// The core's driver read, seen through the STORE tile's `DriversView`.
pub struct CoreDrivers(pub Arc<dyn iyakki_core_api::drivers::Drivers>);

fn availability_to_view(a: iyakki_core_api::drivers::Availability) -> iyakki_tile_store::drivers::Availability {
    use iyakki_core_api::drivers::Availability as A;
    use iyakki_tile_store::drivers::Availability as V;
    match a {
        A::Standard => V::Standard,
        A::AfterPack => V::AfterPack,
        A::NeedsMacImage => V::NeedsMacImage,
        A::LocalBuild => V::LocalBuild,
        A::NotSupported => V::NotSupported,
    }
}

fn delivery_to_view(d: iyakki_core_api::drivers::Delivery) -> iyakki_tile_store::drivers::Delivery {
    use iyakki_core_api::drivers::Delivery as D;
    use iyakki_tile_store::drivers::Delivery as V;
    match d {
        D::Builtin => V::Builtin,
        D::Folder => V::Folder,
        D::Extract => V::Extract,
        D::Info => V::Info,
    }
}

fn status_to_view(s: iyakki_core_api::drivers::Status) -> iyakki_tile_store::drivers::Status {
    use iyakki_core_api::drivers::Status as S;
    use iyakki_tile_store::drivers::Status as V;
    match s {
        S::Working => V::Working,
        S::FirmwareMissing => V::FirmwareMissing,
        S::NeedsReload => V::NeedsReload,
        S::NoDriver => V::NoDriver,
    }
}

fn known_to_view(k: &iyakki_core_api::drivers::known::Known) -> iyakki_tile_store::drivers::KnownInfo {
    iyakki_tile_store::drivers::KnownInfo {
        id: k.id.into(),
        title: k.title.into(),
        summary: k.summary.into(),
        why: k.why.into(),
        supply: k.supply.into(),
        guide: k.guide.into(),
        license: k.license.into(),
        availability: availability_to_view(k.availability),
        availability_word: k.availability.label().into(),
        delivery: delivery_to_view(k.delivery),
    }
}

impl iyakki_tile_store::drivers::DriversView for CoreDrivers {
    fn machine(&self) -> iyakki_tile_store::drivers::MachineInfo {
        let m = self.0.machine();
        iyakki_tile_store::drivers::MachineInfo { vendor: m.vendor, product: m.product }
    }
    fn rows(&self) -> Vec<iyakki_tile_store::drivers::RowInfo> {
        self.0
            .rows()
            .into_iter()
            .map(|r| iyakki_tile_store::drivers::RowInfo {
                known: known_to_view(r.known),
                status: status_to_view(r.status),
                status_word: r.status.label().into(),
                detail: r.detail,
                devices: r.devices,
            })
            .collect()
    }
    fn installed(&self) -> Vec<iyakki_tile_store::drivers::PackInfo> {
        self.0
            .installed()
            .into_iter()
            .map(|r| iyakki_tile_store::drivers::PackInfo { id: r.id, why: r.why, license: r.license, files: r.files, source: r.source, cmdline: r.cmdline })
            .collect()
    }
    fn is_disabled(&self, id: &str) -> bool {
        self.0.is_disabled(id)
    }
    fn facetime_notice(&self) -> String {
        iyakki_core_api::drivers::facetime::NOTICE.to_string()
    }
}

// ---- The audit log (alpha.10 AP3): the agent's log, seen through the tile's `AuditSink`.

/// The agent's audit log.
pub struct AgentAudit(pub Arc<dyn iyakki_agent_api::AuditApi>);

impl iyakki_tile_store::engine::AuditSink for AgentAudit {
    fn record(&self, user: &str, action: &str, detail: &str) {
        self.0.record(user, action, detail)
    }
}

// ---- The forwarding wrapper (alpha.10 AP3): the STORE screens call `iyakki_tile_store::engine::StoreEngine`; this wrapper answers each call
// with the agent's `StoreApi` of the same name. The bodies only forward (and the approval note keeps its one rule: the approval for this
// app version, with grants); no answer is changed.

/// The agent's store handle, seen through the STORE tile's engine trait.
pub struct AgentStore(pub Arc<dyn iyakki_agent_api::StoreApi>);

impl iyakki_tile_store::engine::StoreEngine for AgentStore {
    fn approval_audit(&self, installation: &str, app_version: &str) -> Option<String> {
        self.0.approval(installation).filter(|a| a.app_version == app_version && !a.grants.is_empty()).map(|a| a.audit_detail())
    }
    fn installed_version(&self, kind: &str, name: &str, scope: &iyakki_shared::skills::Scope) -> Option<(String, bool)> {
        self.0.installed_version(kind, name, scope)
    }
    fn is_disabled(&self, kind: &str, name: &str, scope: &iyakki_shared::skills::Scope) -> bool {
        self.0.is_disabled(kind, name, scope)
    }
    fn install(&self, actor: &iyakki_shared::store_install::Actor, review: &iyakki_shared::store_install::Review, confirm: iyakki_shared::store_install::Confirm) -> Result<iyakki_shared::store_install::Record, iyakki_shared::store_install::InstallError> {
        self.0.install(actor, review, confirm)
    }
    fn remove(&self, actor: &iyakki_shared::store_install::Actor, kind: &str, name: &str, scope: &iyakki_shared::skills::Scope) -> Result<(), String> {
        self.0.remove(actor, kind, name, scope)
    }
    fn disable(&self, actor: &iyakki_shared::store_install::Actor, kind: &str, name: &str, scope: &iyakki_shared::skills::Scope) -> Result<(), String> {
        self.0.disable(actor, kind, name, scope)
    }
    fn enable(&self, actor: &iyakki_shared::store_install::Actor, kind: &str, name: &str, scope: &iyakki_shared::skills::Scope) -> Result<iyakki_shared::store_install::Record, iyakki_shared::store_install::InstallError> {
        self.0.enable(actor, kind, name, scope)
    }
    fn enforce_revocations(&self) -> Vec<String> {
        self.0.enforce_revocations()
    }
    fn recover(&self) -> Vec<String> {
        self.0.recover()
    }
    fn add_license(&self, actor: &iyakki_shared::store_install::Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<iyakki_shared::store_license::Token, String> {
        self.0.add_license(actor, text, signer_of)
    }
    fn checkout_url(&self, app: &str, pricing: &iyakki_shared::store_manifest::Pricing) -> Result<String, String> {
        self.0.checkout_url(app, pricing)
    }
    fn remove_installed(&self, policy: &iyakki_sec::policy::Policy, user: &str, e: &iyakki_shared::store_installed::Entry) -> Result<String, String> {
        self.0.remove_installed(policy, user, e)
    }
    fn toggle_installed(&self, policy: &iyakki_sec::policy::Policy, user: &str, e: &iyakki_shared::store_installed::Entry) -> Result<String, String> {
        self.0.toggle_installed(policy, user, e)
    }
    fn pin(&self, key: &str, version: &str) -> Result<(), String> {
        self.0.pin(key, version)
    }
    fn unpin(&self, key: &str) -> Result<(), String> {
        self.0.unpin(key)
    }
    fn lifecycle(&self, policy: &iyakki_sec::policy::Policy, user: &str, loaded: &[iyakki_shared::store_catalog::Loaded], ctx: &iyakki_shared::store_lifecycle::Ctx) -> Vec<iyakki_shared::store_lifecycle::Life> {
        self.0.lifecycle(policy, user, loaded, ctx)
    }
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)> {
        self.0.check_cli_latest()
    }
    fn plan_update(&self, actor: &iyakki_shared::store_install::Actor, life: &iyakki_shared::store_lifecycle::Life, loaded: &[iyakki_shared::store_catalog::Loaded], fetch: &dyn iyakki_shared::store_catalog::Fetch) -> Result<iyakki_shared::store_lifecycle::UpdatePlan, iyakki_shared::store_lifecycle::LifeError> {
        self.0.plan_update(actor, life, loaded, fetch)
    }
    fn apply_update(&self, actor: &iyakki_shared::store_install::Actor, plan: &iyakki_shared::store_lifecycle::UpdatePlan, confirm: iyakki_shared::store_install::Confirm, audit: &dyn Fn(&str, &str)) -> Result<String, iyakki_shared::store_lifecycle::LifeError> {
        self.0.apply_update(actor, plan, confirm, audit)
    }
    fn rollback(&self, actor: &iyakki_shared::store_install::Actor, life: &iyakki_shared::store_lifecycle::Life, audit: &dyn Fn(&str, &str)) -> Result<String, iyakki_shared::store_lifecycle::LifeError> {
        self.0.rollback(actor, life, audit)
    }
    fn plan_remove(&self, user: &str, lives: &[iyakki_shared::store_lifecycle::Life], life: &iyakki_shared::store_lifecycle::Life) -> iyakki_shared::store_lifecycle::RemovePlan {
        self.0.plan_remove(user, lives, life)
    }
    fn remove_item(&self, policy: &iyakki_sec::policy::Policy, user: &str, lives: &[iyakki_shared::store_lifecycle::Life], life: &iyakki_shared::store_lifecycle::Life, opts: &iyakki_shared::store_lifecycle::RemoveOpts, audit: &dyn Fn(&str, &str)) -> Result<String, iyakki_shared::store_lifecycle::LifeError> {
        self.0.remove_item(policy, user, lives, life, opts, audit)
    }
    fn update_all_preview(&self, lives: &[iyakki_shared::store_lifecycle::Life]) -> Vec<(String, String, bool)> {
        self.0.update_all_preview(lives)
    }
    fn update_all(&self, policy: &iyakki_sec::policy::Policy, user: &str, loaded: &[iyakki_shared::store_catalog::Loaded], fetch: &dyn iyakki_shared::store_catalog::Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> iyakki_shared::store_lifecycle::Report {
        self.0.update_all(policy, user, loaded, fetch, audit, progress)
    }
    fn dir(&self) -> &std::path::Path {
        self.0.dir()
    }
    fn now(&self) -> i64 {
        self.0.now()
    }
    fn roots(&self) -> iyakki_shared::store_trust::Roots {
        self.0.roots()
    }
    fn revocations(&self) -> iyakki_shared::store_revocation::Revocations {
        self.0.revocations()
    }
    fn config(&self) -> iyakki_shared::store_catalog::Config {
        self.0.config()
    }
    fn add_source(&self, actor: &iyakki_shared::store_install::Actor, name: &str, kind: iyakki_shared::store_catalog::SourceKind, location: &str) -> Result<String, String> {
        self.0.add_source(actor, name, kind, location)
    }
    fn official_store_url(&self) -> Option<&'static str> {
        self.0.official_store_url()
    }
    fn probe_private(&self, fetch: &dyn iyakki_shared::store_catalog::Fetch, typed: &str) -> Result<iyakki_shared::store_core::Probe, String> {
        self.0.probe_private(fetch, typed)
    }
    fn pin_probed(&self, actor: &iyakki_shared::store_install::Actor, name: &str, p: &iyakki_shared::store_core::Probe, confirmed: &str) -> Result<String, String> {
        self.0.pin_probed(actor, name, p, confirmed)
    }
    fn set_publish_target(&self, actor: &iyakki_shared::store_install::Actor, id: &str) -> Result<(), String> {
        self.0.set_publish_target(actor, id)
    }
    fn remove_source(&self, actor: &iyakki_shared::store_install::Actor, id: &str) -> Result<(), String> {
        self.0.remove_source(actor, id)
    }
    fn toggle_source(&self, actor: &iyakki_shared::store_install::Actor, id: &str) -> Result<bool, String> {
        self.0.toggle_source(actor, id)
    }
    fn set_catalog_enabled(&self, actor: &iyakki_shared::store_install::Actor, on: bool) -> Result<(), String> {
        self.0.set_catalog_enabled(actor, on)
    }
    fn catalog_visible(&self) -> bool {
        self.0.catalog_visible()
    }
    fn trust_key(&self, actor: &iyakki_shared::store_install::Actor, key: &str, label: &str) -> Result<(), String> {
        self.0.trust_key(actor, key, label)
    }
    fn local_catalogs(&self) -> Vec<iyakki_shared::store_catalog::Loaded> {
        self.0.local_catalogs()
    }
    fn refresh(&self, fetch: &dyn iyakki_shared::store_catalog::Fetch) -> iyakki_shared::store_core::Refresh {
        self.0.refresh(fetch)
    }
    fn items(&self, actor: &iyakki_shared::store_install::Actor, loaded: &[iyakki_shared::store_catalog::Loaded]) -> Vec<iyakki_shared::store_core::Item> {
        self.0.items(actor, loaded)
    }
    fn known_names(&self, items: &[iyakki_shared::store_core::Item]) -> Vec<(String, String)> {
        self.0.known_names(items)
    }
    fn review_item(&self, actor: &iyakki_shared::store_install::Actor, item: &iyakki_shared::store_core::Item, loaded: &[iyakki_shared::store_catalog::Loaded], fetch: &dyn iyakki_shared::store_catalog::Fetch, scope: iyakki_shared::skills::Scope, known: &[(String, String)]) -> Result<iyakki_shared::store_install::Review, String> {
        self.0.review_item(actor, item, loaded, fetch, scope, known)
    }
    fn review_sideload(&self, actor: &iyakki_shared::store_install::Actor, bytes: Vec<u8>, scope: iyakki_shared::skills::Scope, known: &[(String, String)]) -> iyakki_shared::store_install::Review {
        self.0.review_sideload(actor, bytes, scope, known)
    }
    fn update_check_run(&self, policy: &iyakki_sec::policy::Policy, user: &str, fetch: &dyn iyakki_shared::store_catalog::Fetch, settings: &iyakki_shared::store_updatecheck::CheckSettings, running: &str, variant: &str) {
        self.0.update_check_run(policy, user, fetch, settings, running, variant)
    }
}

// ---- The mirrors of group 1 (D-Q4-01): the quick-install item, its facts and its job state. `iyakki_tile_api::store_qi` has the plain
// copies; the conversions live here, the only place that names both sides. The matches have no catch-all arm, so a new variant on either
// side does not compile until it is mapped.

use iyakki_shared::jobs as rj;
use iyakki_shared::quickinstall as rq;
use iyakki_tile_api::store_qi as m;

macro_rules! item_map {
    ($($v:ident),* $(,)?) => {
        pub fn item_to_mirror(i: rq::Item) -> m::Item {
            match i { $(rq::Item::$v => m::Item::$v),* }
        }
        pub fn item_from_mirror(i: m::Item) -> rq::Item {
            match i { $(m::Item::$v => rq::Item::$v),* }
        }
    };
}

item_map!(
    Node, Python, K3s, DuckDb, Elasticsearch, OpenSearch, Prometheus, ClickHouse, GraphDb, Gitea, Matrix, SeaweedFs, RustFs, Registry, ClaudeCode, Copilot, Kokoro,
    VoicePack, Lemonade, Ollama, MacHost, GpuVulkan, GpuNvidia, GpuRocm, Ffmpeg, FfmpegLibs, MacFirmware, MacCamera, LinuxApps, LinuxWindows, FwQualcommBt,
    FwRealtekNet, FwBroadcomNet, FwAtheros9170, FwMediatek, FwHardware, DataScience, YtDlp, BrowserPackage,
);

pub fn group_to_mirror(g: rq::Group) -> m::Group {
    match g {
        rq::Group::Runtimes => m::Group::Runtimes,
        rq::Group::Cluster => m::Group::Cluster,
        rq::Group::AiCoding => m::Group::AiCoding,
        rq::Group::Voice => m::Group::Voice,
        rq::Group::LocalModels => m::Group::LocalModels,
        rq::Group::Media => m::Group::Media,
        rq::Group::Linux => m::Group::Linux,
        rq::Group::Mac => m::Group::Mac,
        rq::Group::Firmware => m::Group::Firmware,
    }
}

pub fn group_from_mirror(g: m::Group) -> rq::Group {
    match g {
        m::Group::Runtimes => rq::Group::Runtimes,
        m::Group::Cluster => rq::Group::Cluster,
        m::Group::AiCoding => rq::Group::AiCoding,
        m::Group::Voice => rq::Group::Voice,
        m::Group::LocalModels => rq::Group::LocalModels,
        m::Group::Media => rq::Group::Media,
        m::Group::Linux => rq::Group::Linux,
        m::Group::Mac => rq::Group::Mac,
        m::Group::Firmware => rq::Group::Firmware,
    }
}

/// What the real item answers about itself, as the plain table row the tile holds.
pub fn item_info(i: rq::Item) -> m::ItemInfo {
    m::ItemInfo {
        item: item_to_mirror(i),
        id: i.id().to_string(),
        title: i.title().to_string(),
        about: i.about().to_string(),
        licence: i.licence().to_string(),
        group: group_to_mirror(i.group()),
        needs: i.needs().iter().map(|n| item_to_mirror(*n)).collect(),
        download_mb: i.download_mb(),
        time_text: i.time_text().to_string(),
        min_ram_mb: i.min_ram_mb(),
        is_service: i.is_service(),
        is_hardware: i.is_hardware(),
        runs_as_job: i.runs_as_job(),
        npm_tool: i.npm_tool(),
        pip_tool: i.pip_tool(),
        cli_id: i.cli_id().map(|s| s.to_string()),
        needs_now: i.needs_now().into_iter().map(item_to_mirror).collect(),
        fw: i.fw_def().map(fw_def_to_mirror),
        res_lines: rq::resources::of(i).map(|r| r.lines()).unwrap_or_default(),
    }
}

/// The whole table, in the real order.
pub fn item_table() -> Vec<m::ItemInfo> {
    rq::Item::ALL.iter().map(|i| item_info(*i)).collect()
}

pub fn presence_to_mirror(p: &rq::Presence) -> m::Presence {
    match p {
        rq::Presence::Missing => m::Presence::Missing,
        rq::Presence::Installed(d) => m::Presence::Installed(d.clone()),
        rq::Presence::Earlier(d) => m::Presence::Earlier(d.clone()),
        rq::Presence::Elsewhere(d) => m::Presence::Elsewhere(d.clone()),
    }
}

pub fn presence_from_mirror(p: &m::Presence) -> rq::Presence {
    match p {
        m::Presence::Missing => rq::Presence::Missing,
        m::Presence::Installed(d) => rq::Presence::Installed(d.clone()),
        m::Presence::Earlier(d) => rq::Presence::Earlier(d.clone()),
        m::Presence::Elsewhere(d) => rq::Presence::Elsewhere(d.clone()),
    }
}

pub fn latest_to_mirror(l: &rq::Latest) -> m::Latest {
    match l {
        rq::Latest::Newer(v) => m::Latest::Newer(v.clone()),
        rq::Latest::Current(v) => m::Latest::Current(v.clone()),
        rq::Latest::Unknown(v) => m::Latest::Unknown(v.clone()),
        rq::Latest::NotApplicable => m::Latest::NotApplicable,
    }
}

pub fn latest_from_mirror(l: &m::Latest) -> rq::Latest {
    match l {
        m::Latest::Newer(v) => rq::Latest::Newer(v.clone()),
        m::Latest::Current(v) => rq::Latest::Current(v.clone()),
        m::Latest::Unknown(v) => rq::Latest::Unknown(v.clone()),
        m::Latest::NotApplicable => rq::Latest::NotApplicable,
    }
}

pub fn job_state_to_mirror(s: &rj::State) -> m::JobState {
    match s {
        rj::State::Running => m::JobState::Running,
        rj::State::Succeeded => m::JobState::Succeeded,
        rj::State::Failed(c) => m::JobState::Failed(*c),
        rj::State::Stopped => m::JobState::Stopped,
        rj::State::Lost => m::JobState::Lost,
    }
}

pub fn job_state_from_mirror(s: &m::JobState) -> rj::State {
    match s {
        m::JobState::Running => rj::State::Running,
        m::JobState::Succeeded => rj::State::Succeeded,
        m::JobState::Failed(c) => rj::State::Failed(*c),
        m::JobState::Stopped => rj::State::Stopped,
        m::JobState::Lost => rj::State::Lost,
    }
}

pub fn job_to_mirror(j: &rj::Job) -> m::Job {
    m::Job {
        id: j.id.clone(),
        label: j.label.clone(),
        command: j.command.clone(),
        cwd: j.cwd.clone(),
        owner: j.owner.clone(),
        workspace: j.workspace.clone(),
        pid: j.pid,
        state: job_state_to_mirror(&j.state),
        started_ms: j.started_ms,
        ended_ms: j.ended_ms,
        progress: j.progress,
        last_line: j.last_line.clone(),
        last_active_ms: j.last_active_ms,
        dismissed: j.dismissed,
    }
}

pub fn job_from_mirror(j: &m::Job) -> rj::Job {
    rj::Job {
        id: j.id.clone(),
        label: j.label.clone(),
        command: j.command.clone(),
        cwd: j.cwd.clone(),
        owner: j.owner.clone(),
        workspace: j.workspace.clone(),
        pid: j.pid,
        state: job_state_from_mirror(&j.state),
        started_ms: j.started_ms,
        ended_ms: j.ended_ms,
        progress: j.progress,
        last_line: j.last_line.clone(),
        last_active_ms: j.last_active_ms,
        dismissed: j.dismissed,
    }
}

pub fn launch_to_mirror(l: &rq::Launch) -> m::Launch {
    m::Launch { exe: l.exe.clone(), data: l.data.clone() }
}

pub fn launch_from_mirror(l: &m::Launch) -> rq::Launch {
    rq::Launch { exe: l.exe.clone(), data: l.data.clone() }
}

pub fn hw_opts_to_mirror(o: &rq::HwFirmwareOpts) -> m::HwFirmwareOpts {
    m::HwFirmwareOpts { from: o.from.clone(), with: o.with.clone(), link: o.link.clone() }
}

pub fn hw_opts_from_mirror(o: &m::HwFirmwareOpts) -> rq::HwFirmwareOpts {
    rq::HwFirmwareOpts { from: o.from.clone(), with: o.with.clone(), link: o.link.clone() }
}

pub fn facts_to_mirror(f: &rq::Facts) -> m::Facts {
    m::Facts {
        presence: f.presence.iter().map(|(i, p)| (item_to_mirror(*i), presence_to_mirror(p))).collect(),
        unhealthy: f.unhealthy.iter().map(|(i, w)| (item_to_mirror(*i), w.clone())).collect(),
        runtimes: f.runtimes.iter().map(|(i, r)| (item_to_mirror(*i), r.iter().map(|s| s.to_string()).collect())).collect(),
        used_by: f.used_by.iter().map(|(i, w)| (item_to_mirror(*i), w.clone())).collect(),
        hidden: f.hidden.iter().map(|i| item_to_mirror(*i)).collect(),
        healthy: f.healthy.iter().map(|i| item_to_mirror(*i)).collect(),
        waiting: f.waiting.iter().map(|i| item_to_mirror(*i)).collect(),
        failures: f.failures.iter().map(|(i, w)| (item_to_mirror(*i), w.clone())).collect(),
    }
}

/// Mirror to real. The runtime ids are `&'static str` in the real facts: each distinct word is interned once for the life of the process.
pub fn facts_from_mirror(f: &m::Facts) -> rq::Facts {
    rq::Facts {
        presence: f.presence.iter().map(|(i, p)| (item_from_mirror(*i), presence_from_mirror(p))).collect(),
        unhealthy: f.unhealthy.iter().map(|(i, w)| (item_from_mirror(*i), w.clone())).collect(),
        runtimes: f.runtimes.iter().map(|(i, r)| (item_from_mirror(*i), r.iter().map(|s| iyakki_shared::serde_static::intern(s)).collect())).collect(),
        used_by: f.used_by.iter().map(|(i, w)| (item_from_mirror(*i), w.clone())).collect(),
        hidden: f.hidden.iter().map(|i| item_from_mirror(*i)).collect(),
        healthy: f.healthy.iter().map(|i| item_from_mirror(*i)).collect(),
        waiting: f.waiting.iter().map(|i| item_from_mirror(*i)).collect(),
        failures: f.failures.iter().map(|(i, w)| (item_from_mirror(*i), w.clone())).collect(),
    }
}

pub fn fw_outcome_to_mirror(o: &iyakki_shared::fwpack::Outcome) -> m::FwOutcome {
    m::FwOutcome { at: o.at, installed: o.installed, files: o.files, source: o.source.clone(), reboot_needed: o.reboot_needed, headline: o.headline.clone(), lines: o.lines.clone() }
}

pub fn fw_outcome_from_mirror(o: &m::FwOutcome) -> iyakki_shared::fwpack::Outcome {
    iyakki_shared::fwpack::Outcome { at: o.at, installed: o.installed, files: o.files, source: o.source.clone(), reboot_needed: o.reboot_needed, headline: o.headline.clone(), lines: o.lines.clone() }
}

fn fw_def_to_mirror(d: &iyakki_shared::fwpack::Def) -> m::FwDef {
    m::FwDef { pack_id: d.pack_id.into(), title: d.title.into(), vendor: d.vendor.into(), licence_note: d.licence_note.into(), licence_id: d.licence_id.into(), without: d.without.into(), about: d.about.into() }
}

fn un_review_to_mirror(r: iyakki_shared::quickinstall::uninstall::Review) -> m::UnReview {
    m::UnReview { title: r.title, goes: r.goes, stays: r.stays, data: r.data, data_available: r.data_available, data_required: r.data_required, space: r.space, space_if_ticked: r.space_if_ticked, notes: r.notes }
}

pub fn un_plan_to_mirror(p: &iyakki_shared::quickinstall::uninstall::Plan) -> m::UnPlan {
    m::UnPlan { item: item_to_mirror(p.item), data_requires: p.data_requires, space_text: p.space_text(), review_kept: un_review_to_mirror(p.review(false)), review_deleted: un_review_to_mirror(p.review(true)) }
}

fn oc_mode(m_: m::ollama_cluster::Mode) -> rq::ollama_cluster::Mode {
    match m_ {
        m::ollama_cluster::Mode::Move => rq::ollama_cluster::Mode::Move,
        m::ollama_cluster::Mode::Fresh => rq::ollama_cluster::Mode::Fresh,
    }
}

fn oc_host_to_mirror(h: rq::ollama_cluster::HostInfo) -> m::ollama_cluster::HostInfo {
    m::ollama_cluster::HostInfo { models_dir: h.models_dir, models_bytes: h.models_bytes, free_bytes: h.free_bytes }
}

fn oc_host_from_mirror(h: &m::ollama_cluster::HostInfo) -> rq::ollama_cluster::HostInfo {
    rq::ollama_cluster::HostInfo { models_dir: h.models_dir.clone(), models_bytes: h.models_bytes, free_bytes: h.free_bytes }
}

fn oc_decision_to_mirror(d: rq::ollama_cluster::Decision) -> m::ollama_cluster::Decision {
    match d {
        rq::ollama_cluster::Decision::Copy => m::ollama_cluster::Decision::Copy,
        rq::ollama_cluster::Decision::TooLarge => m::ollama_cluster::Decision::TooLarge,
        rq::ollama_cluster::Decision::NoRoom => m::ollama_cluster::Decision::NoRoom,
    }
}

fn oc_decision_from_mirror(d: m::ollama_cluster::Decision) -> rq::ollama_cluster::Decision {
    match d {
        m::ollama_cluster::Decision::Copy => rq::ollama_cluster::Decision::Copy,
        m::ollama_cluster::Decision::TooLarge => rq::ollama_cluster::Decision::TooLarge,
        m::ollama_cluster::Decision::NoRoom => rq::ollama_cluster::Decision::NoRoom,
    }
}

/// Give the tile the table of what the real items answer (once; the first call wins).
pub fn install_table() {
    m::set_table(item_table());
}

/// The real quick-install services in mirror types: the agent's facets underneath, the conversions here.
pub struct RealQi {
    pub b0: Arc<dyn B0Services>,
    pub agent: Arc<dyn AgentApi>,
}

impl m::QuickInstall for RealQi {
    fn gather_facts(&self) -> m::Facts {
        facts_to_mirror(&self.b0.qi_gather_facts())
    }
    fn install_jobs(&self, owner: &str) -> Vec<m::Job> {
        self.b0.qi_install_jobs(owner).iter().map(job_to_mirror).collect()
    }
    fn check_upgrades(&self) -> Vec<(m::Item, m::Latest)> {
        self.b0.qi_check_upgrades().iter().map(|(i, l)| (item_to_mirror(*i), latest_to_mirror(l))).collect()
    }
    fn start(&self, owner: &str, target: m::Item, present: &dyn Fn(m::Item) -> bool, launch: &m::Launch) -> Result<Vec<m::Job>, String> {
        let jobs = self.b0.qi_start(owner, item_from_mirror(target), &|i| present(item_to_mirror(i)), &launch_from_mirror(launch))?;
        Ok(jobs.iter().map(job_to_mirror).collect())
    }
    fn start_firmware_pack(&self, owner: &str, item: m::Item, from: Option<&Path>, launch: &m::Launch) -> Result<m::Job, String> {
        self.b0.qi_start_firmware_pack(owner, item_from_mirror(item), from, &launch_from_mirror(launch)).map(|j| job_to_mirror(&j))
    }
    fn start_camera_firmware(&self, owner: &str, launch: &m::Launch) -> Result<m::Job, String> {
        self.b0.qi_start_camera_firmware(owner, &launch_from_mirror(launch)).map(|j| job_to_mirror(&j))
    }
    fn start_hardware_firmware(&self, owner: &str, opts: &m::HwFirmwareOpts, launch: &m::Launch) -> Result<m::Job, String> {
        self.b0.qi_start_hardware_firmware(owner, &hw_opts_from_mirror(opts), &launch_from_mirror(launch)).map(|j| job_to_mirror(&j))
    }
    fn uninstall_plan(&self, item: m::Item, user: &str) -> (m::UnPlan, Option<String>) {
        let (plan, warn) = self.b0.qi_uninstall_plan(item_from_mirror(item), user);
        (un_plan_to_mirror(&plan), warn)
    }
    fn uninstall(&self, item: m::Item, user: &str, opts: m::UnOpts, progress: Option<Arc<std::sync::Mutex<String>>>) -> Result<String, String> {
        self.b0.qi_uninstall(item_from_mirror(item), user, rq::uninstall::Opts { delete_data: opts.delete_data, with_dependants: opts.with_dependants }, progress)
    }
    fn dependency_lines(&self, item: m::Item, installed: &dyn Fn(m::Item) -> bool) -> (Vec<m::Item>, Vec<String>) {
        let (deps, warn) = rq::uninstall::dependency_lines(item_from_mirror(item), &|i| installed(item_to_mirror(i)));
        (deps.into_iter().map(item_to_mirror).collect(), warn)
    }
    fn order_titles_with(&self, steps: &[m::Item], before: &dyn Fn(m::Item) -> Vec<String>) -> Vec<String> {
        let real: Vec<rq::Item> = steps.iter().map(|i| item_from_mirror(*i)).collect();
        self.agent.b0().quickinstall().order_titles_with(&real, &|i| before(item_to_mirror(i)).iter().map(|s| iyakki_shared::serde_static::intern(s)).collect())
    }
    fn total_warning(&self, steps: &[m::Item], installed: &[m::Item], ram_mb: u64) -> Option<String> {
        let steps: Vec<rq::Item> = steps.iter().map(|i| item_from_mirror(*i)).collect();
        let installed: Vec<rq::Item> = installed.iter().map(|i| item_from_mirror(*i)).collect();
        rq::resources::total_warning(&steps, &installed, ram_mb)
    }
    fn ollama_host_info(&self, data: &Path) -> m::ollama_cluster::HostInfo {
        oc_host_to_mirror(self.agent.b0().quickinstall().ollama_host_info(data))
    }
    fn ollama_decide(&self, h: &m::ollama_cluster::HostInfo) -> m::ollama_cluster::Decision {
        oc_decision_to_mirror(rq::ollama_cluster::decide(&oc_host_from_mirror(h)))
    }
    fn ollama_review_lines(&self, mode: m::ollama_cluster::Mode, h: &m::ollama_cluster::HostInfo, d: m::ollama_cluster::Decision) -> Vec<String> {
        rq::ollama_cluster::review_lines(oc_mode(mode), &oc_host_from_mirror(h), oc_decision_from_mirror(d))
    }
    fn ollama_write_mode(&self, data: &Path, mode: m::ollama_cluster::Mode) -> Result<(), String> {
        rq::ollama_cluster::write_mode(data, oc_mode(mode))
    }
    fn fw_present(&self, sys_root: &Path, item: m::Item) -> bool {
        item_from_mirror(item).fw_def().is_some_and(|d| iyakki_shared::fwpack::present_in(sys_root, d))
    }
    fn fw_installed(&self, data: &Path, item: m::Item) -> bool {
        item_from_mirror(item).fw_def().is_some_and(|d| iyakki_shared::fwpack::installed(data, d).is_some())
    }
    fn fw_outcome(&self, data: &Path, item: m::Item) -> Option<m::FwOutcome> {
        item_from_mirror(item).fw_def().and_then(|d| iyakki_shared::fwpack::read_outcome(data, d)).map(|o| fw_outcome_to_mirror(&o))
    }
    fn fw_accept(&self, data: &Path, item: m::Item, by: &str, now: i64) -> Result<(), String> {
        let def = item_from_mirror(item).fw_def().ok_or_else(|| format!("{} is not a firmware pack", item_from_mirror(item).title()))?;
        iyakki_shared::fwpack::accept(data, def, by, now)
    }
    fn fw_accepted(&self, data: &Path, item: m::Item) -> bool {
        item_from_mirror(item).fw_def().is_some_and(|d| iyakki_shared::fwpack::accepted(data, d).is_some())
    }
    fn fw_licence_digest(&self, item: m::Item) -> String {
        item_from_mirror(item).fw_def().map(iyakki_shared::fwpack::licence_digest).unwrap_or_default()
    }
    fn fw_commit(&self) -> String {
        iyakki_shared::fwpack::table().commit.clone()
    }
    fn hw_installed(&self, data: &Path) -> bool {
        iyakki_shared::fwextra::installed(data).is_some()
    }
    fn hw_outcome(&self, data: &Path) -> Option<m::FwOutcome> {
        iyakki_shared::fwextra::read_outcome(data).map(|o| fw_outcome_to_mirror(&o))
    }
    fn hw_remove(&self, data: &Path) -> Result<(), String> {
        iyakki_shared::fwextra::remove(data).map(|_| ())
    }
    fn hw_tag(&self) -> String {
        iyakki_shared::fwextra::TAG.to_string()
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn every_item_round_trips_and_keeps_its_place() {
        assert_eq!(rq::Item::ALL.len(), m::Item::ALL.len());
        for (i, (r, mi)) in rq::Item::ALL.iter().zip(m::Item::ALL.iter()).enumerate() {
            assert_eq!(item_to_mirror(*r), *mi, "item {i}: same place in both lists");
            assert_eq!(item_from_mirror(*mi), *r);
        }
    }

    #[test]
    fn every_group_round_trips() {
        for g in rq::Group::ALL {
            assert_eq!(group_from_mirror(group_to_mirror(g)), g);
        }
        assert_eq!(m::Group::ALL.len(), rq::Group::ALL.len());
        for (a, b) in rq::Group::ALL.iter().zip(m::Group::ALL.iter()) {
            assert_eq!(group_to_mirror(*a), *b);
        }
    }

    #[test]
    fn the_table_answers_what_the_real_item_answers() {
        let t = item_table();
        assert_eq!(t.len(), 39);
        for (row, r) in t.iter().zip(rq::Item::ALL.iter()) {
            assert_eq!(item_from_mirror(row.item), *r);
            assert_eq!(row.id, r.id());
            assert_eq!(row.title, r.title());
            assert_eq!(row.about, r.about());
            assert_eq!(row.licence, r.licence());
            assert_eq!(group_from_mirror(row.group), r.group());
            assert_eq!(row.needs.iter().map(|n| item_from_mirror(*n)).collect::<Vec<_>>(), r.needs().to_vec());
            assert_eq!((row.download_mb, row.min_ram_mb), (r.download_mb(), r.min_ram_mb()));
            assert_eq!(row.time_text, r.time_text());
            assert_eq!((row.is_service, row.is_hardware, row.runs_as_job, row.npm_tool, row.pip_tool), (r.is_service(), r.is_hardware(), r.runs_as_job(), r.npm_tool(), r.pip_tool()));
            assert_eq!(row.cli_id.as_deref(), r.cli_id());
        }
    }

    #[test]
    fn presence_and_latest_round_trip() {
        for p in [rq::Presence::Missing, rq::Presence::Installed("1.2".into()), rq::Presence::Earlier("x".into()), rq::Presence::Elsewhere("y".into())] {
            assert_eq!(presence_from_mirror(&presence_to_mirror(&p)), p);
        }
        for l in [rq::Latest::Newer("v2".into()), rq::Latest::Current("v1".into()), rq::Latest::Unknown("why".into()), rq::Latest::NotApplicable] {
            assert_eq!(latest_from_mirror(&latest_to_mirror(&l)), l);
        }
    }

    #[test]
    fn jobs_and_their_states_round_trip() {
        for s in [rj::State::Running, rj::State::Succeeded, rj::State::Failed(124), rj::State::Failed(1), rj::State::Stopped, rj::State::Lost] {
            assert_eq!(job_state_from_mirror(&job_state_to_mirror(&s)), s);
        }
        let j = rj::Job {
            id: "j7".into(),
            label: "Install Node.js".into(),
            command: "iyakki quick-install nodejs".into(),
            cwd: "/data".into(),
            owner: "admin".into(),
            workspace: rq::WORKSPACE.into(),
            pid: 42,
            state: rj::State::Failed(2),
            started_ms: 10,
            ended_ms: 20,
            progress: Some(12.5),
            last_line: "boom".into(),
            last_active_ms: 15,
            dismissed: true,
        };
        let back = job_from_mirror(&job_to_mirror(&j));
        assert_eq!(back, j);
        assert!(!job_to_mirror(&j).running());
        let mut running = j.clone();
        running.state = rj::State::Running;
        assert!(job_to_mirror(&running).running());
    }

    #[test]
    fn launch_and_hardware_options_round_trip() {
        let l = rq::Launch { exe: "/usr/bin/iyakki".into(), data: "/data".into() };
        let back = launch_from_mirror(&launch_to_mirror(&l));
        assert_eq!((back.exe, back.data), (l.exe, l.data));
        let o = rq::HwFirmwareOpts { from: Some("/stick/fw".into()), with: vec!["nvidia".into()], link: Some("eth1".into()) };
        let back = hw_opts_from_mirror(&hw_opts_to_mirror(&o));
        assert_eq!((back.from, back.with, back.link), (o.from, o.with, o.link));
        let d = hw_opts_from_mirror(&hw_opts_to_mirror(&rq::HwFirmwareOpts::default()));
        assert!(d.from.is_none() && d.with.is_empty() && d.link.is_none());
    }

    #[test]
    fn facts_round_trip() {
        let f = rq::Facts {
            presence: vec![(rq::Item::Node, rq::Presence::Installed("20".into())), (rq::Item::K3s, rq::Presence::Missing)],
            unhealthy: vec![(rq::Item::Copilot, "cannot run".into())],
            runtimes: vec![(rq::Item::Copilot, vec!["nodejs"]), (rq::Item::Python, vec![])],
            used_by: vec![(rq::Item::Node, "Copilot".into())],
            hidden: vec![rq::Item::GpuNvidia],
            healthy: vec![rq::Item::Elasticsearch],
            waiting: vec![rq::Item::ClickHouse],
            failures: vec![(rq::Item::Ollama, "network".into())],
        };
        let back = facts_from_mirror(&facts_to_mirror(&f));
        assert_eq!(back.presence, f.presence);
        assert_eq!(back.unhealthy, f.unhealthy);
        assert_eq!(back.runtimes, f.runtimes);
        assert_eq!(back.used_by, f.used_by);
        assert_eq!((back.hidden, back.healthy, back.waiting, back.failures), (f.hidden, f.healthy, f.waiting, f.failures));
        let empty = facts_to_mirror(&rq::Facts::default());
        assert_eq!(empty, m::Facts::default());
    }

    #[test]
    fn the_table_rows_carry_the_order_the_firmware_and_the_pod_words() {
        install_table();
        for r in rq::Item::ALL {
            let mi = item_to_mirror(r);
            assert_eq!(mi.needs_now().into_iter().map(item_from_mirror).collect::<Vec<_>>(), r.needs_now(), "{}", r.id());
            assert_eq!(mi.id(), r.id());
            assert_eq!(mi.title(), r.title());
            assert_eq!(m::Item::parse(r.id()), Some(mi));
            assert_eq!(mi.fw_def().map(|d| d.pack_id.as_str()), r.fw_def().map(|d| d.pack_id));
            if let Some(d) = r.fw_def() {
                let f = mi.fw_def().unwrap();
                assert_eq!((f.title.as_str(), f.vendor.as_str(), f.licence_note.as_str(), f.licence_id.as_str(), f.without.as_str(), f.about.as_str()), (d.title, d.vendor, d.licence_note, d.licence_id, d.without, d.about));
            }
            assert_eq!(mi.info().res_lines, rq::resources::of(r).map(|x| x.lines()).unwrap_or_default());
        }
        assert_eq!(m::Item::parse("no-such-item"), None);
    }

    #[test]
    fn the_install_order_and_the_job_helpers_agree_with_the_real_ones() {
        install_table();
        let present_sets: [&[rq::Item]; 3] = [&[], &[rq::Item::K3s], &[rq::Item::K3s, rq::Item::Node, rq::Item::Python, rq::Item::Lemonade]];
        for target in rq::Item::ALL {
            for set in present_sets {
                let real = rq::order(target, &|i| set.contains(&i));
                let mirror = m::order(item_to_mirror(target), &|i| set.contains(&item_from_mirror(i)));
                assert_eq!(mirror.iter().copied().map(item_from_mirror).collect::<Vec<_>>(), real, "{}", target.id());
                assert_eq!(m::order_text(&mirror), rq::order_text(&real));
            }
        }
        assert_eq!(m::titles_text(&["a".to_string(), "b".to_string()]), rq::titles_text(&["a".to_string(), "b".to_string()]));
        assert_eq!(m::LAST_FAILED, rq::LAST_FAILED);
        assert_eq!(m::WORKSPACE, rq::WORKSPACE);
        for g in rq::Group::ALL {
            assert_eq!(m::items_of(group_to_mirror(g)).into_iter().map(item_from_mirror).collect::<Vec<_>>(), rq::items_of(g));
        }
        let failed = format!("{}network", rq::LAST_FAILED);
        for p in [rq::Presence::Missing, rq::Presence::Installed("1".into()), rq::Presence::Installed(String::new()), rq::Presence::Earlier(failed.clone()), rq::Presence::Earlier(String::new()), rq::Presence::Earlier("x".into()), rq::Presence::Elsewhere("y".into())] {
            let mp = presence_to_mirror(&p);
            assert_eq!(mp.word(), p.word());
            assert_eq!(mp.present(), p.present());
            assert_eq!(m::failure_reason(&mp), rq::failure_reason(&p));
        }
        let job = |id: &str, item: rq::Item, state: rj::State, at: u64| rj::Job {
            id: id.into(),
            label: item.title().into(),
            command: format!("/usr/bin/iyakki quick-install {} --data /d", item.id()),
            cwd: "/d".into(),
            owner: "u".into(),
            workspace: rq::WORKSPACE.into(),
            pid: 1,
            state,
            started_ms: at,
            ended_ms: 0,
            progress: None,
            last_line: String::new(),
            last_active_ms: 0,
            dismissed: false,
        };
        let jobs = vec![
            job("a", rq::Item::Node, rj::State::Failed(1), 1),
            job("b", rq::Item::Node, rj::State::Succeeded, 2),
            job("c", rq::Item::K3s, rj::State::Failed(2), 3),
            job("d", rq::Item::Python, rj::State::Running, 4),
            job("e", rq::Item::Python, rj::State::Lost, 3),
            job("f", rq::Item::DuckDb, rj::State::Stopped, 5),
        ];
        let mj: Vec<m::Job> = jobs.iter().map(job_to_mirror).collect();
        assert_eq!(m::failed_items(&mj).into_iter().map(item_from_mirror).collect::<Vec<_>>(), rq::failed_items(&jobs));
        for (a, b) in jobs.iter().zip(&mj) {
            assert_eq!(m::item_of(b).map(item_from_mirror), rq::item_of(a));
        }
        assert_eq!(m::item_of_command("sh -c ls"), None);
    }

    #[test]
    fn the_firmware_outcome_and_the_reboot_rule_round_trip() {
        let o = iyakki_shared::fwpack::Outcome { at: 100, installed: true, files: 3, source: "download".into(), reboot_needed: true, headline: "h".into(), lines: vec!["l".into()] };
        assert_eq!(fw_outcome_from_mirror(&fw_outcome_to_mirror(&o)), o);
        for (now, up) in [(150, 100), (150, 30), (100, 0), (100, 500)] {
            assert_eq!(fw_outcome_to_mirror(&o).reboot_pending(now, up), iyakki_shared::fwpack::reboot_pending(&o, now, up));
        }
        let not_done = iyakki_shared::fwpack::Outcome { installed: false, ..o.clone() };
        assert_eq!(fw_outcome_to_mirror(&not_done).reboot_pending(150, 100), iyakki_shared::fwpack::reboot_pending(&not_done, 150, 100));
    }

    #[test]
    fn the_ollama_words_and_the_removal_reviews_are_the_real_ones() {
        use rq::ollama_cluster as oc;
        assert_eq!((m::ollama_cluster::CLUSTER_WORD, m::ollama_cluster::HOST_WORD, m::ollama_cluster::MOVE_TITLE, m::ollama_cluster::FRESH_TITLE), (oc::CLUSTER_WORD, oc::HOST_WORD, oc::MOVE_TITLE, oc::FRESH_TITLE));
        for d in ["Ollama runs in the cluster", "Ollama runs on this machine's host", ""] {
            assert_eq!(m::ollama_cluster::is_cluster_detail(d), oc::is_cluster_detail(d));
        }
        let h = oc::HostInfo { models_dir: "/m".into(), models_bytes: 5 << 30, free_bytes: Some(100 << 30) };
        assert_eq!(oc_host_from_mirror(&oc_host_to_mirror(h.clone())), h);
        for d in [oc::Decision::Copy, oc::Decision::TooLarge, oc::Decision::NoRoom] {
            assert_eq!(oc_decision_from_mirror(oc_decision_to_mirror(d)), d);
        }
        // the removal plan: the review of the real plan, kept and deleted
        let plan = rq::uninstall::Plan { item: rq::Item::Elasticsearch, steps: vec![], data_requires: true, always_stays: vec!["the image".into()], claim_gb: 5, data_bytes: 3 << 20, notes: vec!["n".into()] };
        let mp = un_plan_to_mirror(&plan);
        assert_eq!(item_from_mirror(mp.item), plan.item);
        assert_eq!(mp.space_text(), plan.space_text());
        assert!(mp.data_requires);
        assert_eq!(mp.review(false).title, plan.review(false).title);
        assert_eq!(mp.review(true).space, plan.review(true).space);
        assert_eq!(mp.review(false).stays, plan.review(false).stays);
    }
}

#[cfg(test)]
mod engine_tests {
    //! The forwarding wrapper answers what the agent's handle answers: a double that overrides four calls and leaves the rest to the
    //! agent's defaults ("nothing here"), so each forwarded answer is checked against the double, not against a copy of the default.
    use std::sync::Arc;

    use iyakki_agent_api::store_api::{StorePublish, UNAVAILABLE_STORE};
    use iyakki_agent_api::StoreApi;
    use iyakki_shared::store_install::{Actor, Review};
    use iyakki_shared::store_lifecycle::{Life, RemovePlan};
    use iyakki_tile_store::engine::StoreEngine;

    use super::AgentStore;

    struct Double;
    impl StorePublish for Double {}
    impl StoreApi for Double {
        fn plan_remove(&self, _user: &str, _lives: &[Life], _life: &Life) -> RemovePlan {
            unreachable!("the engine test does not plan a removal")
        }
        fn review_sideload(&self, _actor: &Actor, _bytes: Vec<u8>, _scope: iyakki_shared::skills::Scope, _known: &[(String, String)]) -> Review {
            unreachable!("the engine test does not review a file")
        }
        fn now(&self) -> i64 {
            42
        }
        fn official_store_url(&self) -> Option<&'static str> {
            Some("https://store.example/")
        }
        fn catalog_visible(&self) -> bool {
            true
        }
    }

    #[test]
    fn the_overridden_answers_come_through_the_wrapper() {
        let e = AgentStore(Arc::new(Double));
        assert_eq!(e.now(), 42);
        assert_eq!(e.official_store_url(), Some("https://store.example/"));
        assert!(e.catalog_visible());
    }

    #[test]
    fn a_call_the_double_leaves_to_the_agent_default_gives_the_agent_default_answer() {
        let e = AgentStore(Arc::new(Double));
        assert_eq!(e.approval_audit("app.one", "1.0"), None);
        let err = e.enable(&Actor::system(), "app", "one", &iyakki_shared::skills::Scope::Shared).unwrap_err();
        assert!(err.to_string().contains(UNAVAILABLE_STORE), "{err}");
        assert_eq!(e.pin("app.one", "1.0"), Err(UNAVAILABLE_STORE.to_string()));
    }
}

/// The app store of this install (the agent core always serves it): the handle the other tiles use for the publish and keys flows.
pub(crate) fn store_of(cx: &AppCx) -> Arc<dyn StoreApi> {
    store_of_port(&CxPort(cx))
}

pub(crate) fn store_of_port(cx: &dyn StorePort) -> Arc<dyn StoreApi> {
    cx.b4().store().expect("the agent core serves the app store")
}
