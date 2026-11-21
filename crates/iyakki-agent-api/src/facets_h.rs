//! The handle on the person's services (alpha.6 split H, the finish of the UI/agent split): everything the UI still asked of `Services`,
//! as one object-safe trait. The UI holds `Arc<dyn ServicesHandle>` (`AppCx::services`, `ShellEnv::services`); the real implementation is
//! `iyakki_agent::facets_h` over `Services`, and `iyakki-compose` builds it (`iyakki_compose::open_services`).
//! A fake default must never be what production gets (KI-1010): `is_real` is asserted by the composition root, `Shell::new` and the sweep tests.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::*;
use iyakki_core_api::fleet::Fleet;
mod recording;
pub use recording::RecordingServices;
use iyakki_shared::chat::{ChatBackend, Message};
use iyakki_shared::settings::ModelChoice;
use iyakki_shared::gitkeys::AuditFn;
use iyakki_proxy::Upstream;
use iyakki_shared::agent::Tools;
use iyakki_shared::authcodes::CodeVault;
use iyakki_shared::context::{ContextInfo, ContextOptions};
use iyakki_shared::gitkeys::{GitKeys, Session};
use iyakki_shared::kb::Hit;
use iyakki_shared::services::{LoginReport, PullStatus, SessionDeleted};
use iyakki_shared::story::Synth;
use iyakki_shared::jobs::Job;
use iyakki_shared::quickinstall::{Launch, RegistryArgs};
use iyakki_core_api::soundtypes::{Cue, CueOut};
use iyakki_shared::modelhost::{Host, NewHost, TestReport};
use iyakki_shared::visionmodel::Cand;
use iyakki_shared::syssounds::SystemSounds;
use iyakki_shared::story::StoryStore;
use iyakki_shared::voice::SpeechEngine;
use iyakki_shared::visionmodel::{Pick, VisionError};
use iyakki_shared::voice::{self, SpeakOpts, VoiceSettings};
use iyakki_shared::webtools::PageSnapshot;

/// Where Lemonade runs (CPU or GPU): what Settings reads and sets.
pub trait ComputeBackend: Send + Sync {
    fn current(&self) -> iyakki_shared::gpu_steps::Compute;
    /// Keep the choice and apply it to the cluster; the plain result line.
    fn set(&self, c: iyakki_shared::gpu_steps::Compute) -> Result<String, String>;
}

pub trait ServicesHandle: Send + Sync {
    /// The real implementation (the agent's `Services`) answers true; a fake answers false.
    fn is_real(&self) -> bool {
        false
    }
    /// The object itself, for the composition root and for tests that need the concrete type.
    fn as_any(&self) -> &dyn Any;
    /// The same object as an `Arc<dyn Any>` (downcast with `Arc::downcast`).
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
    fn agents_cfg(&self) -> iyakki_shared::extagent::AgentsCfg;
    fn agent_vault_token(&self, user: &str, agent: &str) -> Option<(&'static str, zeroize::Zeroizing<String>)>;
    fn ai(&self) -> Arc<dyn AiRead>;
    fn allow_speech_server(&self, url: &str) -> Result<(), String>;
    fn approvals(&self, user: &str) -> Vec<iyakki_shared::runs::PendingApproval>;
    fn b0(&self) -> Arc<dyn B0Services>;
    fn b3(&self) -> Arc<dyn ServicesFacetB3>;
    fn b4(&self) -> Arc<dyn ServicesFacetB4>;
    fn b5(&self) -> Arc<dyn ServicesFacetB5>;
    fn chat_backend_for(&self, choice: ModelChoice) -> Arc<dyn ChatBackend>;
    fn chat_context(&self, user: &str, session_id: Option<&str>, base_system: &str, history: &[Message], user_msg: &str, opts: &ContextOptions) -> (Vec<Message>, ContextInfo);
    fn cloud_stream_card(&self) -> Option<String>;
    fn cloud_stream_press(&self, yes: bool);
    fn cloud_stream_prompt_rule(&self) -> &'static str;
    fn cloud_stream_sync(&self);
    fn code_vault(&self, user: &str) -> Box<dyn CodeVault>;
    fn contract_local(&self) -> Arc<dyn crate::ContractLocal>;
    fn decide_approval(&self, user: &str, run_id: &str, call_id: &str, approve: bool) -> Result<iyakki_shared::runs::PendingApproval, String>;
    fn delete_session(&self, user: &str, id: &str) -> Result<SessionDeleted, String>;
    fn dev_deploy(&self) -> Arc<dyn crate::DevDeployFacet>;
    fn dir(&self) -> &Path;
    fn domains_facet(&self, user: &str, audit: AuditFn) -> Arc<dyn crate::DomainsMgr>;
    fn effective_tts(&self, lang: Option<&str>) -> Option<voice::SpeechEngine>;
    fn extagent_facet(&self) -> Arc<dyn ExtAgentFacet>;
    fn fleet_accept_label(&self, w: &str) -> String;
    fn fleet_attach_workspace(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, mgr: Arc<dyn crate::TaskMgr>, root: &std::path::Path, test: &str);
    fn fleet_context_conflicts(&self) -> Vec<String>;
    fn fleet_context_settle(&self, copy_id: &str, keep: &str) -> Result<String, String>;
    fn fleet_member_chips(&self) -> Vec<(String, bool)>;
    fn fleet_prompt_rule(&self) -> &'static str;
    fn flows_facet(&self) -> Arc<dyn FlowsFacet>;
    /// The account's flows environment as the FLOWS tile uses it (its jobs only: the engine, the store and the host of the real one never cross).
    fn flows_jobs(&self, user: &str, policy: &iyakki_sec::policy::Policy) -> Option<Arc<dyn crate::FlowsJobs>>;
    fn gemini_live(&self) -> Arc<dyn GeminiLive>;
    fn generate_image_traced(&self, model: Option<(&str, &str)>, prompt: &str, size: &str, p: &iyakki_shared::providers::ImageParams) -> (Result<(String, String, Vec<u8>, &'static str), String>, iyakki_shared::providers::ImageTrace);
    fn git_keys(&self, user: &str, audit: AuditFn, session: Arc<Session>) -> GitKeys;
    fn gpu_lemonade(&self) -> Option<String>;
    fn lemonade_cached(&self, provider: &str) -> Vec<iyakki_shared::lemonade::Model>;
    fn lemonade_health(&self, provider: &str) -> Result<(String, Vec<String>), String>;
    fn lemonade_images_missing(&self) -> Vec<iyakki_shared::lemonade::Model>;
    fn lemonade_live_model(&self, provider: &str, model: &str) -> Result<Option<iyakki_shared::lemonade::Model>, String>;
    fn lemonade_model(&self, provider: &str, model: &str) -> Option<iyakki_shared::lemonade::Model>;
    fn matrix(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn MatrixService>;
    fn media_index(&self, user: &str) -> Arc<Mutex<iyakki_shared::mediaindex::MediaIndex>>;
    fn note_fleet_press(&self, tool: &str);
    fn page_snapshot_of(&self, user: &str) -> Option<PageSnapshot>;
    fn pending_facet(&self) -> Arc<dyn crate::pending::AgentPending>;
    fn platform_real(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn PlatformApi>;
    fn platform_with(&self, user: &str, run: Arc<dyn iyakki_shared::wsexec::Runner>, env: iyakki_shared::devcluster::Env, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, tmp: PathBuf) -> Arc<dyn PlatformApi>;
    fn provider_is_lemonade(&self, provider: &str) -> bool;
    fn pull_model_for(&self, user: &str, provider: &str, model: &str) -> Result<(), String>;
    fn pull_status(&self) -> Vec<PullStatus>;
    fn quick_upstream(&self) -> Arc<dyn Upstream>;
    fn rag_kit(&self, user: &str) -> Arc<dyn crate::RagKit>;
    fn reading_synth(&self, opts: SpeakOpts) -> Arc<dyn Synth>;
    fn refresh_lemonade(&self, provider: &str) -> Result<Vec<iyakki_shared::lemonade::Model>, String>;
    fn remote_kit(&self, user: &str, data: &std::path::Path, downloads: &str, local: Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn crate::RemoteKit>;
    fn router_panel(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, gate: Arc<dyn iyakki_core_api::router::Router>) -> Arc<dyn crate::RouterPanel>;
    fn router_plan_rows(&self, user: &str) -> Vec<(String, String, Vec<String>, &'static str, Option<String>)>;
    fn search(&self, namespaces: &[&str], query: &str, k: usize) -> (Vec<Hit>, Vec<String>);
    fn session_files(&self, user: &str, id: &str) -> usize;
    fn session_workdir(&self, user: &str, session_id: &str) -> Result<PathBuf, String>;
    fn set_page_snapshot(&self, user: &str, snap: Option<PageSnapshot>);
    fn set_show_all_ai(&self, v: bool) -> Result<(), String>;
    fn set_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>);
    fn set_voice_mic(&self, id: &str) -> Result<(), String>;
    fn sftp_server(&self, data: &std::path::Path, login: crate::SftpLogin, lookup: crate::SftpLookup, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn crate::SftpServer>;
    fn sftp_status_lines(&self, data: &std::path::Path) -> Vec<String>;
    // ---- the file scope of the core (what a person's files, pickers and tools may reach): asked of the core so a UI program does not read the machine itself
    /// May `user` (with `policy`) read `p`? `Err` is the sentence to show.
    fn scope_check_read(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String>;
    /// May `user` write `p`?
    fn scope_check_write(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String>;
    /// The person's home folder (a real path).
    fn scope_home_root(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf;
    /// The home folder for code that creates folders in it.
    fn scope_home_root_for_writes(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf;
    /// The `HOME` a program started for the person must get; `None` when this is not the IYAKKI system.
    fn scope_system_home(&self, user: &str) -> Option<std::path::PathBuf>;
    /// The folders the person's pickers and tools stay inside.
    fn scope_roots(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> iyakki_core_api::FileRoots;
    /// [`ServicesHandle::scope_roots`] with the home folder chosen by the caller.
    fn scope_roots_with_home(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: std::path::PathBuf) -> iyakki_core_api::FileRoots;
    /// Resolve `input` (absolute, or relative to `cwd`) to a canonical path.
    fn scope_resolve(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String>;
    /// Like `scope_resolve`, for a file about to be written.
    fn scope_resolve_for_write(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String>;
    /// Make the folder above `path`.
    fn scope_ensure_parent(&self, path: &std::path::Path) -> Result<(), String>;
    fn show_all_ai(&self) -> bool;
    fn sound_settings(&self) -> iyakki_shared::audioout::SoundSettings;
    fn speech_kit(&self) -> Arc<dyn SpeechKit>;
    fn store_facets(&self) -> Arc<dyn crate::StoreFacets>;
    fn streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>>;
    fn system_slots(&self) -> Vec<String>;
    fn system_vault_state(&self) -> (bool, bool);
    fn tasks_facet(&self) -> Arc<dyn TasksFacet>;
    fn terminal(&self, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>) -> Box<dyn crate::Terminal>;
    fn transcribe_samples(&self, pcm: &[i16]) -> Result<String, String>;
    fn user_vault_unlocked(&self, user: &str) -> bool;
    fn vault_facet(&self) -> Arc<dyn VaultRead>;
    fn vision_model(&self, own: Option<&ModelChoice>) -> Result<Pick, VisionError>;
    fn voicedesk_facet(&self) -> Arc<dyn crate::VoiceDeskFacet>;
    fn voice_settings(&self) -> VoiceSettings;
    fn wake_mark(&self, kind: &str) -> Result<(), String>;
    fn wake_stats_line(&self) -> String;
    fn wrap_cloud_stream_tools(&self, inner: Arc<dyn Tools>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, can_change: bool) -> Arc<dyn Tools>;
    fn wrap_fleet_tools(&self, inner: Arc<dyn Tools>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools>;
    // ---- the members, as accessors (the same names as the fields of Services)
    /// The system settings with the shape the old `Arc<Mutex<SettingsStore>>` had (`lock()` gives a guard); a snapshot, see [`SettingsRef`].
    fn settings(&self) -> SettingsRef {
        SettingsRef::new(Arc::new(SettingsSrc(self.settings_api())))
    }
    /// The system settings as the interface that can cross the core socket.
    fn settings_api(&self) -> Arc<dyn SettingsApi>;
    fn inbox(&self) -> Arc<dyn crate::member_stores::InboxApi>;
    fn calendar(&self) -> Arc<dyn crate::member_stores::CalendarApi>;
    fn study(&self) -> Arc<dyn crate::member_stores::StudyApi>;
    /// The study plans that touch the calendar (add, remove, delete with their events); the core holds both stores.
    fn study_calendar(&self) -> Arc<dyn crate::study_calendar::StudyCalendarApi>;
    fn secrets(&self) -> &iyakki_shared::vault::Secrets;
    /// The secrets as the interface that can cross the core socket: whether a reference resolves, the provider list and the provider test. It
    /// never hands out a plain value (D-SPLIT-03-SECRETS); a screen that needs only "key present" asks here instead of [`ServicesHandle::secrets`].
    fn secrets_api(&self) -> Arc<dyn crate::member_stores::SecretsApi> {
        Arc::new(self.secrets().clone())
    }
    fn kb(&self) -> Arc<dyn crate::member_stores::KnowledgeBaseApi>;
    /// The knowledge base as the source the workspace history reads and writes.
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        Arc::new(crate::KbHandle(self.kb()))
    }
    fn devwatch(&self) -> Arc<dyn crate::member_stores::DevWatchApi>;
    fn browser(&self) -> Arc<dyn crate::member_stores::BrowserStateApi>;
    /// The questions the assistant asks about the person's browser (look, act, step, hand over, permission) and their answers.
    fn browser_consent(&self) -> Arc<dyn crate::member_stores::ConsentApi>;
    /// The queue between the assistant's browser tools and the page engine the shell services.
    fn browser_jobs(&self) -> Arc<dyn crate::member_stores::BrowserJobsApi>;
    fn screen(&self) -> Arc<dyn crate::member_stores::ScreenCaptureApi>;
    fn camera(&self) -> Arc<dyn crate::member_stores::CameraHubApi>;
    fn devtools(&self) -> Arc<dyn crate::member_stores::DevToolsApi>;
    fn ext_status(&self) -> Arc<dyn crate::member_stores::ExtStatusApi>;
    fn secret_requests(&self) -> Arc<dyn crate::member_stores::SecretRequestsApi>;
    fn usage(&self) -> Arc<dyn crate::member_stores::UsageLogApi>;
    fn sessions(&self) -> Arc<dyn SessionApi>;
    fn skills(&self) -> Arc<dyn SkillApi>;
    fn runs(&self) -> Arc<dyn RunApi>;
    fn packages(&self) -> Arc<dyn PackageApi>;
    fn mcp(&self) -> &dyn McpApi;
    fn jobs(&self) -> Arc<dyn JobApi>;
    fn radio(&self) -> Arc<dyn RadioApi>;
    fn clis(&self) -> Arc<dyn CliApi>;
    fn ollama(&self) -> Arc<dyn OllamaApi>;
    fn cluster(&self) -> Arc<dyn SandboxCluster>;
    fn k3sdev(&self) -> Arc<dyn DevCluster>;
    fn fleet(&self) -> Arc<dyn FleetApi>;
    fn git_kit(&self) -> Arc<dyn crate::GitKit>;
    fn qi_start_registry(&self, owner: &str, a: &RegistryArgs, k3s_present: bool, launch: &Launch) -> Result<Vec<Job>, String>;
    fn av_notify_stale(&self, user: &str);
    fn msg_mirror_tick(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> bool;
    fn rag_refresh_tick(&self, user: &str, policy: &iyakki_sec::policy::Policy, audit: iyakki_shared::gitkeys::AuditFn) -> bool;
    fn msg_offer_tools(&self, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools>;
    fn page_passkeys(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<std::sync::Mutex<dyn crate::PagePasskeyHost>>>;
    fn page_push(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<std::sync::Mutex<dyn crate::PagePushHost>>>;
    fn msg_prompt_rules(&self, user: &str) -> String;
    fn ssh_host(&self, user: &str) -> Arc<dyn crate::SshHost>;
    fn ssh_session_tools(&self, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools>;
    fn system_speaker(&self) -> Box<dyn iyakki_shared::voiceassist::Speaker>;
    fn user_tools(&self, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>, mcp: Vec<iyakki_shared::mcp::ServerSpec>, request: Option<&str>, cwd: Option<&Path>) -> Arc<dyn iyakki_shared::agent::Tools>;
    fn voice_turn_log(&self, user: &str) -> Box<dyn iyakki_shared::voiceassist::TurnLog>;
    fn wake_capture_mic(&self) -> Box<dyn iyakki_shared::wakelisten::Mic>;
    fn lesson_host(&self) -> Arc<dyn iyakki_shared::studylesson::LessonHost>;
    fn dash_host(&self) -> Arc<dyn iyakki_dash::tools::DashHost>;
    /// What the fleet page shows (the status without asking the network).
    fn fleet_view(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Result<iyakki_shared::fleetview::Status, String>;
    /// Do what a confirmed fleet card says.
    fn fleet_act(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, act: iyakki_shared::fleetview::FleetAct) -> Result<String, String>;
    fn announce_name_on(&self) -> bool;
    fn check_models(&self, provider_id: &str, cap: Option<usize>) -> Result<iyakki_shared::modelavail::Report, String>;
    fn clear_vision_cache(&self);
    fn cloud_setting(&self) -> iyakki_shared::cloudmode::Setting;
    fn collection_review(&self, provider: &str, collection: &str) -> Result<iyakki_shared::omni::Review, String>;
    fn cue_out(&self) -> Arc<dyn CueOut>;
    fn detect_local_servers(&self) -> Vec<String>;
    fn enable_keyfile(&self, path: &str) -> Result<(), String>;
    fn fetch_provider_models(&self, provider: &str) -> Result<Vec<String>, String>;
    fn hw_volume_check(&self, card: u32, device: u32, control: &str) -> Result<String, String>;
    fn install_missing_parts(&self, provider: &str, collection: &str) -> Result<Vec<String>, String>;
    fn is_reindexing(&self) -> bool;
    fn lemonade_load(&self, provider: &str, model: &str, load: bool) -> Result<String, String>;
    fn lock_system(&self);
    fn modelhost_add(&self, n: &NewHost, report: &TestReport) -> Result<Host, String>;
    fn modelhost_remove(&self, id: &str) -> Result<(), String>;
    fn on_password_change(&self, user: &str, old: &str, new: &str);
    fn parts_undo_available(&self) -> Option<(String, String)>;
    fn preview_cue(&self, cue: Cue, out: Arc<dyn CueOut>) -> Result<String, String>;
    fn refresh_secret_providers(&self);
    fn reindex_stale_in_background(&self) -> bool;
    fn set_announce_name(&self, v: Option<bool>) -> Result<(), String>;
    fn set_cloud_setting(&self, v: iyakki_shared::cloudmode::Setting) -> Result<(), String>;
    fn set_startup_reboot_into_iyakki(&self, v: bool) -> Result<(), String>;
    fn set_web_access(&self, on: bool) -> Result<(), String>;
    fn set_web_access_port(&self, port: u16) -> Result<(), String>;
    fn slow_upstream(&self) -> Arc<dyn Upstream>;
    fn speech_aligner(&self) -> Option<SpeechEngine>;
    fn speech_server_allowed(&self, url: &str) -> bool;
    fn startup_reboot_into_iyakki(&self) -> bool;
    fn synthesize_with_opts(&self, text: &str, opts: &voice::SpeakOpts) -> Result<Vec<u8>, String>;
    fn system_sounds(&self) -> SystemSounds;
    fn test_pattern(&self, walk: bool) -> Result<String, String>;
    fn test_sound(&self, device: Option<&str>) -> Result<String, String>;
    fn undo_all_parts(&self, store: &StoryStore) -> Result<String, String>;
    fn update_sound_with(&self, f: &mut dyn FnMut(&mut iyakki_shared::audioout::SoundSettings)) -> Result<iyakki_shared::audioout::SoundSettings, String>;
    fn update_system_sounds_with(&self, f: &mut dyn FnMut(&mut SystemSounds)) -> Result<SystemSounds, String>;
    fn use_all_parts(&self, provider: &str, collection: &str, store: &StoryStore) -> Result<String, String>;
    fn use_lemonade_stt(&self, provider: &str, model: &str) -> Result<String, String>;
    fn use_lemonade_voice(&self, provider: &str, model: &str, voice: Option<&str>) -> Result<String, String>;
    fn vision_candidates(&self) -> Vec<Cand>;
    fn wake_no_model_text(&self) -> Option<String>;
    fn wake_set_opt_in(&self, on: bool) -> Result<bool, String>;
    fn wake_set_sensitivity(&self, sens: u8) -> Result<(), String>;
    fn web_access_on(&self) -> bool;
    fn web_access_port(&self) -> u16;
    fn host_tts(&self) -> Arc<dyn HostTtsApi>;
    /// The Lemonade compute switch (CPU or GPU) of `user`.
    fn lemonade_compute_backend(&self, user: &str) -> Arc<dyn ComputeBackend>;
    fn detect_local_lemonade_in_background(&self);
    fn detect_local_ollama_in_background(&self);
    fn domains_login(&self, user: &str, audit: AuditFn);
    fn domains_logout(&self, user: &str);
    fn install_bt_remote(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>);
    fn install_pending_judge(&self);
    fn lock_user_vault(&self, user: &str);
    fn master_toggle_mute(&self) -> Result<iyakki_shared::audioout::Master, String>;
    fn on_login(&self, user: &str, password: &str, is_admin: bool) -> LoginReport;
    fn on_logout(&self, user: &str, is_admin: bool);
    fn on_resume(&self, user: &str, is_admin: bool) -> LoginReport;
    fn sharing_notice_tick(&self, user: &str, admin: bool) -> bool;
    fn start_device_watch(&self);
    fn unlock_user_vault(&self, user: &str, password: &str) -> Result<(), String>;
    fn wake_spawn(&self, listener: Option<crate::WakeListenerBox>) -> Result<Box<dyn crate::WakeHandle>, String>;
    fn warm_up_chat(&self);
    fn chat_backend(&self) -> Arc<dyn ChatBackend>;
    fn set_chat_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>);
    fn start_sound_output(&self);
    fn start_sound_output_when_ready(&self);
    fn router_panel_over(&self, backend: Box<dyn std::any::Any + Send>) -> Option<Arc<dyn crate::RouterPanel>>;
    // GEN-END
}

impl dyn ServicesHandle {
    /// Change the sound settings and save them (the closure runs once).
    pub fn update_sound<F: FnOnce(&mut iyakki_shared::audioout::SoundSettings)>(&self, f: F) -> Result<iyakki_shared::audioout::SoundSettings, String> {
        let mut f = Some(f);
        self.update_sound_with(&mut |s| {
            if let Some(f) = f.take() {
                f(s)
            }
        })
    }

    /// Change the system sounds and save them (the closure runs once).
    pub fn update_system_sounds<F: FnOnce(&mut SystemSounds)>(&self, f: F) -> Result<SystemSounds, String> {
        let mut f = Some(f);
        self.update_system_sounds_with(&mut |s| {
            if let Some(f) = f.take() {
                f(s)
            }
        })
    }
}
