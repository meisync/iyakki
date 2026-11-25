//! The real implementation of the facets of `iyakki_agent_api::facets_s6b` (F-SPLIT-03, alpha.6 lane split-b): each call is one function of
//! the agent module it replaces for the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent_api::{AgentFacetsS6B, HostProbeFacet, SoundProbeFacet};
use iyakki_sec::policy::Policy;
use iyakki_shared::audioout::Output;
use iyakki_shared::mediaindex::ItemKind;
use iyakki_shared::voicedesk::T2Mic;
use iyakki_shared::webmix::TapState;

use crate::agentapi_gen::AgentApiImpl;

struct RealSoundProbe;
impl SoundProbeFacet for RealSoundProbe {
    fn voice_peak(&self, pcm: &[i16]) -> i16 {
        crate::voice::peak(pcm)
    }
    fn t2_mic(&self, proc_root: &Path) -> T2Mic {
        crate::voicedesk::t2_mic(proc_root)
    }
    fn audio_outputs_at(&self, proc_root: &Path) -> Vec<Output> {
        crate::audioout::outputs_at(proc_root)
    }
    fn audio_card_starting(&self, proc_root: &Path, sys: &Path) -> Option<u32> {
        crate::audioout::card_starting(proc_root, sys)
    }
    fn web_mix_status_text(&self, web_mix: bool, mix_enabled: bool, tap: Option<TapState>) -> &'static str {
        crate::webmix::status_text(web_mix, mix_enabled, tap)
    }
    fn player_system_config(&self) -> Option<iyakki_agent_api::PlayerConfig> {
        crate::player::Config::system().map(|c| Arc::new(c) as iyakki_agent_api::PlayerConfig)
    }
    fn player_start(&self, cfg: &iyakki_agent_api::PlayerConfig, file: &str, start: f64, area: (u32, u32)) -> Result<Arc<dyn iyakki_agent_api::MediaPlayer>, String> {
        let c = cfg.clone().downcast::<crate::player::Config>().map_err(|_| "not this machine's player configuration".to_string())?;
        let player = Arc::new(crate::player::Player::start((*c).clone(), file, start, area)?);
        // remembered weakly, so that the headphones leaving pause it (F-LIFE-14)
        let p: Arc<dyn crate::routepause::Pausable> = player.clone();
        crate::routepause::global().add(&p);
        Ok(player)
    }
    fn system_players(&self) -> Option<Arc<dyn iyakki_shared::voicedesk::Players>> {
        Some(Arc::new(crate::voicedesk::SystemPlayers))
    }
    fn system_recorders(&self) -> Option<Arc<dyn iyakki_shared::voicedesk::Recorders>> {
        Some(Arc::new(crate::voicedesk::SystemRecorders))
    }
    fn sounddoctor_scan_via(&self, route: &dyn iyakki_core_api::AudioRoute, helper: u32, own_pid: u32, playing: bool) -> iyakki_shared::sounddoctor::Obs {
        crate::sounddoctor::scan_via(route, helper, own_pid, playing)
    }
    fn sounddoctor_is_engine_via(&self, route: &dyn iyakki_core_api::AudioRoute, helper: u32, pid: u32) -> bool {
        crate::sounddoctor::is_engine_via(route, helper, pid)
    }
    fn system_audio_route(&self) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> {
        std::sync::Arc::new(iyakki_core_local::audioroute::LocalAudioRoute::system())
    }
    fn audio_mixing(&self) -> bool {
        crate::audiosink::mixing()
    }
    fn bluetooth_audio_install(&self, play_cmd: Box<dyn Fn() -> String + Send + Sync>) {
        crate::btaudiotools::install(move || play_cmd())
    }
    fn bluetooth_sound_view(&self) -> iyakki_agent_api::BtSoundView {
        let t = crate::btsound::view_tools();
        iyakki_agent_api::BtSoundView { active: crate::btsound::active(&t), line: t.sound_line(), sinks: t.sinks() }
    }
    fn bluetooth_sound_start(&self, data: &Path, address: &str) -> Result<String, String> {
        crate::btsound::start(data, address)
    }
    fn bluetooth_sound_stop(&self, data: &Path) -> Result<String, String> {
        crate::btsound::stop(data)
    }
}

struct RealExtLogin;
impl iyakki_agent_api::ExtLoginFacet for RealExtLogin {
    fn login_home(&self, user: &str, data: &Path) -> PathBuf {
        crate::scope::home_root(user, data)
    }
    fn start(&self, spec: &iyakki_extagentx::login::Spec) -> Option<Arc<dyn iyakki_agent_api::LoginHandle>> {
        Some(Arc::new(crate::extagent::login::LoginSession::start(spec)))
    }
    fn logout(&self, agent: &str, program: &str, home: &Path) -> Result<String, String> {
        crate::extagent::login::logout(agent, program, home)
    }
    fn relay_open(&self, label: &str) -> String {
        crate::extagent::relay::open(label)
    }
    fn relay_port(&self) -> Option<(u16, bool)> {
        crate::extagent::relay::port()
    }
    fn relay_take_submitted(&self) -> Option<String> {
        crate::extagent::relay::take_submitted()
    }
    fn relay_rearm(&self) {
        crate::extagent::relay::rearm()
    }
    fn relay_finish(&self) {
        crate::extagent::relay::finish()
    }
}

impl iyakki_agent_api::LoginHandle for crate::extagent::login::LoginSession {
    fn snapshot(&self) -> iyakki_extagentx::login::Snapshot {
        crate::extagent::login::LoginSession::snapshot(self)
    }
    fn cancel(&self) {
        crate::extagent::login::LoginSession::cancel(self)
    }
    fn send_line(&self, text: &str) {
        crate::extagent::login::LoginSession::send_line(self, text)
    }
}

struct RealHostProbe;
impl HostProbeFacet for RealHostProbe {
    fn webengine_platform_note(&self) -> Option<&'static str> {
        crate::webengine::platform_note()
    }
    fn webengine_offered(&self) -> bool {
        crate::webengine::offered()
    }
    fn webengine_embedded_helper(&self) -> Option<PathBuf> {
        crate::webengine::embedded_helper()
    }
    fn webengine_set_host_mode(&self, on: bool) {
        crate::webengine::set_host_mode(on)
    }
    fn home_root_for_writes(&self, user: &str, data_dir: &Path) -> PathBuf {
        crate::scope::home_root_for_writes(user, data_dir)
    }
    fn media_planned_folder(&self, folders: &[String], policy: &Policy, user: &str, kind: Option<ItemKind>) -> Option<PathBuf> {
        crate::mediadl::planned_folder(folders, policy, user, kind)
    }
    fn cluster_gitea(&self, dir: &Path, user: &str) -> Vec<iyakki_shared::gitconn::ClusterGitea> {
        crate::gitconn::cluster_gitea(dir, user)
    }
    fn devmode(&self, data: &Path) -> Arc<dyn iyakki_agent_api::DevModeApi> {
        crate::devmode::facet::open(data)
    }
    fn devmode_is_on(&self, data: &Path) -> bool {
        crate::devmode::is_on(data)
    }
    fn update_centre(&self, data: &Path) -> Arc<dyn iyakki_agent_api::UpdateCentre> {
        crate::devmode::centre::Centre::real(data).into_facet()
    }
    fn boot_id_now(&self) -> String {
        crate::devmode::centre::boot_id_now()
    }
    fn power_caps(&self, env: &iyakki_shared::power::PowerEnv) -> iyakki_shared::power::Caps {
        crate::power::caps(env)
    }
    fn power_current(&self, env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::Profile> {
        crate::power::current(env)
    }
    fn power_set_profile(&self, env: &iyakki_shared::power::PowerEnv, p: iyakki_shared::power::Profile) -> Result<usize, String> {
        crate::power::set_profile(env, p)
    }
    fn power_apply_saved(&self, env: &iyakki_shared::power::PowerEnv, s: &iyakki_shared::power::PowerSettings) -> Result<String, String> {
        crate::power::apply_saved(env, s)
    }
    fn power_fan_card(&self, env: &iyakki_shared::power::PowerEnv, dir: &Path) -> Option<iyakki_shared::power::FanCard> {
        crate::power::fan_card(env, dir)
    }
    fn power_fan_toggle(&self, env: &iyakki_shared::power::PowerEnv, dir: &Path) -> Result<bool, String> {
        crate::power::fan_toggle(env, dir)
    }
    fn power_lid_state(&self, proc_root: &Path) -> Option<iyakki_shared::power::Lid> {
        crate::power::lid_state(proc_root)
    }
    fn power_external_display(&self, sys: &Path) -> bool {
        crate::power::external_display(sys)
    }
    fn sharing_backend(&self, data: &Path) -> Option<Arc<dyn iyakki_shared::sharing::control::Backend>> {
        Some(Arc::new(crate::sharing::control::FileBackend { data: data.to_path_buf() }))
    }
    fn shared_folders(&self, data: &Path, user: &str, admin: bool, policy: &Policy, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::SharedFolderMgr> {
        let allow = crate::sharedfolder::allow_fn_for(admin, policy.clone(), user, data);
        let scope = crate::sharedfolder::scope_for(admin, user, data);
        crate::sharedfolder::for_user(data, user, audit, allow, scope)
    }
    fn shares_running_total(&self) -> usize {
        crate::sharedfolder::running_total()
    }
    fn shares_stop_everything(&self) -> usize {
        crate::sharedfolder::stop_everything()
    }
    fn lan_addresses(&self) -> Vec<String> {
        crate::sharedfolder::lan_addresses()
    }
    fn random_hex(&self, bytes: usize) -> String {
        crate::sharedfolder::random_hex(bytes)
    }
    fn sharing_notice_is_on(&self, data: &Path) -> bool {
        crate::sharing::notice::is_on(data)
    }
}

impl AgentFacetsS6B for AgentApiImpl {
    fn sound_probe(&self) -> &dyn SoundProbeFacet {
        &RealSoundProbe
    }
    fn ext_login(&self) -> &dyn iyakki_agent_api::ExtLoginFacet {
        &RealExtLogin
    }
    fn host_probe(&self) -> &dyn HostProbeFacet {
        &RealHostProbe
    }
}

impl crate::services::Services {
    /// The volume and play keys of a connected phone or headset act on the player while somebody is signed in (F-NET-27).
    pub fn install_bt_remote(self: &Arc<Self>, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) {
        crate::btaudiotools::install_remote(self, user, move |who, what| audit(who, what));
    }

    /// Model-judged checks of waiting work (`wait_for` with `judge:`) ask the person's chat model (F-AI-16).
    pub fn install_pending_judge(self: &Arc<Self>) {
        crate::pendingplus::set_judge(Some(Arc::new(crate::pendingplus::ChatJudge::new(self.chat_backend()))));
    }

    /// One look at the sharing notice of a person; whether the badge changed.
    pub fn sharing_notice_tick(self: &Arc<Self>, user: &str, admin: bool) -> bool {
        crate::sharing::notice::tick(self, self.dir(), user, admin)
    }
}

impl iyakki_agent_api::MediaPlayer for crate::player::Player {
    fn info(&self) -> iyakki_shared::player::Info {
        crate::player::Player::info(self).clone()
    }
    fn status(&self) -> iyakki_shared::player::Status {
        crate::player::Player::status(self)
    }
    fn frame(&self, seen: u64) -> Option<Arc<iyakki_shared::player::Frame>> {
        crate::player::Player::frame(self, seen)
    }
    fn pause(&self) {
        crate::player::Player::pause(self)
    }
    fn resume(&self) {
        crate::player::Player::resume(self)
    }
    fn toggle_pause(&self) {
        crate::player::Player::toggle_pause(self)
    }
    fn seek_to(&self, secs: f64) {
        crate::player::Player::seek_to(self, secs)
    }
    fn seek_by(&self, delta: f64) {
        crate::player::Player::seek_by(self, delta)
    }
    fn set_area(&self, area: (u32, u32)) {
        crate::player::Player::set_area(self, area)
    }
    fn stop(&self) {
        crate::player::Player::stop(self)
    }
}
