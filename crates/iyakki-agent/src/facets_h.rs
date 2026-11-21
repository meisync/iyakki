//! The real `ServicesHandle` (alpha.6 split H): the UI holds `Arc<dyn ServicesHandle>`, this is what answers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_agent_api::*;
use iyakki_core_api::fleet::Fleet;
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

use crate::services::Services;

impl Services {
    /// This object as the `Arc` it was opened in.
    pub(crate) fn arc(&self) -> Arc<Services> {
        self.me.upgrade().expect("Services is only built by Services::open, which keeps it in an Arc")
    }
}

impl Services {
    /// The `Services` inside an `Arc<dyn Any>` a facet was handed: the concrete `Arc<Services>` (what `ServicesHandle::as_any_arc` gives), or the handle itself
    /// wrapped (`Arc::new(handle)`). `None` for anything else. Every facet that takes the services as `Any` goes through this, so a UI that holds the
    /// handle (not the concrete type) never gets "no services handle" (the class of bug of the 2026-11-18 gate).
    pub fn from_any(a: Arc<dyn std::any::Any + Send + Sync>) -> Option<Arc<Services>> {
        match a.downcast::<Services>() {
            Ok(s) => Some(s),
            Err(a) => a.downcast::<Arc<dyn ServicesHandle>>().ok().and_then(|h| h.as_any().is::<Services>().then(|| Services::from_handle(&h))),
        }
    }

    /// The `Services` behind a handle (the composition root, the host programs and tests that need the concrete type).
    pub fn from_handle(h: &Arc<dyn ServicesHandle>) -> Arc<Services> {
        h.clone().as_any_arc().downcast::<Services>().expect("the services handle is the agent's Services")
    }
}

/// For tests (and the composition root): the concrete `Services` behind a handle, so a test can call what the UI never needs.
pub trait RealServices {
    fn real(&self) -> &Services;
}

impl RealServices for dyn ServicesHandle {
    fn real(&self) -> &Services {
        self.as_any().downcast_ref::<Services>().expect("the services handle is the agent's Services")
    }
}

impl ServicesHandle for Services {
    fn is_real(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }
    fn agents_cfg(&self) -> iyakki_shared::extagent::AgentsCfg {
        Services::agents_cfg(self)
    }
    fn agent_vault_token(&self, user: &str, agent: &str) -> Option<(&'static str, zeroize::Zeroizing<String>)> {
        Services::agent_vault_token(self, user, agent)
    }
    fn ai(&self) -> Arc<dyn AiRead> {
        self.arc().ai()
    }
    fn allow_speech_server(&self, url: &str) -> Result<(), String> {
        Services::allow_speech_server(self, url)
    }
    fn approvals(&self, user: &str) -> Vec<iyakki_shared::runs::PendingApproval> {
        Services::approvals(self, user)
    }
    fn b0(&self) -> Arc<dyn B0Services> {
        self.arc().b0()
    }
    fn b3(&self) -> Arc<dyn ServicesFacetB3> {
        self.arc().b3()
    }
    fn b4(&self) -> Arc<dyn ServicesFacetB4> {
        self.arc().b4()
    }
    fn b5(&self) -> Arc<dyn ServicesFacetB5> {
        self.arc().b5()
    }
    fn chat_backend_for(&self, choice: ModelChoice) -> Arc<dyn ChatBackend> {
        self.arc().chat_backend_for(choice)
    }
    fn chat_context(&self, user: &str, session_id: Option<&str>, base_system: &str, history: &[Message], user_msg: &str, opts: &ContextOptions) -> (Vec<Message>, ContextInfo) {
        iyakki_agent_api::ServicesB7::chat_context(self, user, session_id, base_system, history, user_msg, opts)
    }
    fn cloud_stream_card(&self) -> Option<String> {
        Services::cloud_stream_card(self)
    }
    fn cloud_stream_press(&self, yes: bool) {
        Services::cloud_stream_press(self, yes)
    }
    fn cloud_stream_prompt_rule(&self) -> &'static str {
        Services::cloud_stream_prompt_rule(self)
    }
    fn cloud_stream_sync(&self) {
        Services::cloud_stream_sync(self)
    }
    fn code_vault(&self, user: &str) -> Box<dyn CodeVault> {
        self.arc().code_vault(user)
    }
    fn contract_local(&self) -> Arc<dyn iyakki_agent_api::ContractLocal> {
        iyakki_agent_api::ServicesB6::contract_local(&self.arc())
    }
    fn decide_approval(&self, user: &str, run_id: &str, call_id: &str, approve: bool) -> Result<iyakki_shared::runs::PendingApproval, String> {
        Services::decide_approval(self, user, run_id, call_id, approve)
    }
    fn delete_session(&self, user: &str, id: &str) -> Result<SessionDeleted, String> {
        Services::delete_session(self, user, id)
    }
    fn dev_deploy(&self) -> Arc<dyn iyakki_agent_api::DevDeployFacet> {
        self.arc().dev_deploy()
    }
    fn dir(&self) -> &Path {
        Services::dir(self)
    }
    fn domains_facet(&self, user: &str, audit: AuditFn) -> Arc<dyn iyakki_agent_api::DomainsMgr> {
        self.arc().domains_facet(user, audit)
    }
    fn effective_tts(&self, lang: Option<&str>) -> Option<voice::SpeechEngine> {
        Services::effective_tts(self, lang)
    }
    fn extagent_facet(&self) -> Arc<dyn ExtAgentFacet> {
        self.arc().extagent_facet()
    }
    fn fleet_accept_label(&self, w: &str) -> String {
        Services::fleet_accept_label(self, w)
    }
    fn fleet_attach_workspace(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, mgr: Arc<dyn iyakki_agent_api::TaskMgr>, root: &std::path::Path, test: &str) {
        self.arc().fleet_attach_workspace(os, gate, audit, mgr, root, test)
    }
    fn fleet_context_conflicts(&self) -> Vec<String> {
        self.arc().fleet_context_conflicts()
    }
    fn fleet_context_settle(&self, copy_id: &str, keep: &str) -> Result<String, String> {
        self.arc().fleet_context_settle(copy_id, keep)
    }
    fn fleet_member_chips(&self) -> Vec<(String, bool)> {
        self.arc().fleet_member_chips()
    }
    fn fleet_prompt_rule(&self) -> &'static str {
        Services::fleet_prompt_rule(self)
    }
    fn flows_facet(&self) -> Arc<dyn FlowsFacet> {
        self.arc().flows_facet()
    }
    fn flows_jobs(&self, user: &str, policy: &iyakki_sec::policy::Policy) -> Option<Arc<dyn iyakki_agent_api::FlowsJobs>> {
        iyakki_agent_api::ServicesB7::flows_panel(&self.arc(), user, policy).map(|p| p as Arc<dyn iyakki_agent_api::FlowsJobs>)
    }
    fn gemini_live(&self) -> Arc<dyn GeminiLive> {
        iyakki_agent_api::ServicesB6::gemini_live(&self.arc())
    }
    fn generate_image_traced(&self, model: Option<(&str, &str)>, prompt: &str, size: &str, p: &iyakki_shared::providers::ImageParams) -> (Result<(String, String, Vec<u8>, &'static str), String>, iyakki_shared::providers::ImageTrace) {
        Services::generate_image_traced(self, model, prompt, size, p)
    }
    fn git_keys(&self, user: &str, audit: AuditFn, session: Arc<Session>) -> GitKeys {
        self.arc().git_keys(user, audit, session)
    }
    fn gpu_lemonade(&self) -> Option<String> {
        Services::gpu_lemonade(self)
    }
    fn lemonade_cached(&self, provider: &str) -> Vec<iyakki_shared::lemonade::Model> {
        Services::lemonade_cached(self, provider)
    }
    fn lemonade_health(&self, provider: &str) -> Result<(String, Vec<String>), String> {
        Services::lemonade_health(self, provider)
    }
    fn lemonade_images_missing(&self) -> Vec<iyakki_shared::lemonade::Model> {
        Services::lemonade_images_missing(self)
    }
    fn lemonade_live_model(&self, provider: &str, model: &str) -> Result<Option<iyakki_shared::lemonade::Model>, String> {
        Services::lemonade_live_model(self, provider, model)
    }
    fn lemonade_model(&self, provider: &str, model: &str) -> Option<iyakki_shared::lemonade::Model> {
        Services::lemonade_model(self, provider, model)
    }
    fn matrix(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn MatrixService> {
        iyakki_agent_api::ServicesB1::matrix(&self.arc(), user, audit)
    }
    fn media_index(&self, user: &str) -> Arc<Mutex<iyakki_shared::mediaindex::MediaIndex>> {
        self.arc().media_index(user)
    }
    fn note_fleet_press(&self, tool: &str) {
        Services::note_fleet_press(self, tool)
    }
    fn page_snapshot_of(&self, user: &str) -> Option<PageSnapshot> {
        Services::page_snapshot_of(self, user)
    }
    fn pending_facet(&self) -> Arc<dyn iyakki_agent_api::pending::AgentPending> {
        self.arc().pending_facet()
    }
    fn platform_real(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn PlatformApi> {
        self.arc().platform_real(user, audit)
    }
    fn platform_with(&self, user: &str, run: Arc<dyn iyakki_shared::wsexec::Runner>, env: iyakki_shared::devcluster::Env, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, tmp: PathBuf) -> Arc<dyn PlatformApi> {
        self.arc().platform_with(user, run, env, audit, tmp)
    }
    fn provider_is_lemonade(&self, provider: &str) -> bool {
        Services::provider_is_lemonade(self, provider)
    }
    fn pull_model_for(&self, user: &str, provider: &str, model: &str) -> Result<(), String> {
        self.arc().pull_model_for(user, provider, model)
    }
    fn pull_status(&self) -> Vec<PullStatus> {
        Services::pull_status(self)
    }
    fn quick_upstream(&self) -> Arc<dyn Upstream> {
        Services::quick_upstream(self)
    }
    fn rag_kit(&self, user: &str) -> Arc<dyn iyakki_agent_api::RagKit> {
        iyakki_agent_api::ServicesB6::rag_kit(&self.arc(), user)
    }
    fn reading_synth(&self, opts: SpeakOpts) -> Arc<dyn Synth> {
        iyakki_agent_api::ServicesB7::reading_synth(&self.arc(), opts)
    }
    fn refresh_lemonade(&self, provider: &str) -> Result<Vec<iyakki_shared::lemonade::Model>, String> {
        Services::refresh_lemonade(self, provider)
    }
    fn remote_kit(&self, user: &str, data: &std::path::Path, downloads: &str, local: Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::RemoteKit> {
        iyakki_agent_api::ServicesB6::remote_kit(&self.arc(), user, data, downloads, local, audit)
    }
    fn router_panel(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, gate: Arc<dyn iyakki_core_api::router::Router>) -> Arc<dyn iyakki_agent_api::RouterPanel> {
        iyakki_agent_api::ServicesB6::router_panel(&self.arc(), user, audit, gate)
    }
    fn router_plan_rows(&self, user: &str) -> Vec<(String, String, Vec<String>, &'static str, Option<String>)> {
        Services::router_plan_rows(self, user)
    }
    fn search(&self, namespaces: &[&str], query: &str, k: usize) -> (Vec<Hit>, Vec<String>) {
        Services::search(self, namespaces, query, k)
    }
    fn session_files(&self, user: &str, id: &str) -> usize {
        Services::session_files(self, user, id)
    }
    fn session_workdir(&self, user: &str, session_id: &str) -> Result<PathBuf, String> {
        Services::session_workdir(self, user, session_id)
    }
    fn set_page_snapshot(&self, user: &str, snap: Option<PageSnapshot>) {
        Services::set_page_snapshot(self, user, snap)
    }
    fn set_show_all_ai(&self, v: bool) -> Result<(), String> {
        Services::set_show_all_ai(self, v)
    }
    fn set_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        Services::set_streamer(self, s)
    }
    fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        Services::set_voice_mic(self, id)
    }
    fn sftp_server(&self, data: &std::path::Path, login: iyakki_agent_api::SftpLogin, lookup: iyakki_agent_api::SftpLookup, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::SftpServer> {
        iyakki_agent_api::ServicesB6::sftp_server(&self.arc(), data, login, lookup, audit)
    }
    fn sftp_status_lines(&self, data: &std::path::Path) -> Vec<String> {
        iyakki_agent_api::ServicesB6::sftp_status_lines(&self.arc(), data)
    }
    fn scope_check_read(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        crate::scope::check_read(policy, user, p)
    }
    fn scope_check_write(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        crate::scope::check_write(policy, user, p)
    }
    fn scope_home_root(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        crate::scope::home_root(user, data_dir)
    }
    fn scope_home_root_for_writes(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        crate::scope::home_root_for_writes(user, data_dir)
    }
    fn scope_system_home(&self, user: &str) -> Option<std::path::PathBuf> {
        crate::scope::system_home(user)
    }
    fn scope_roots(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> iyakki_core_api::FileRoots {
        crate::scope::file_roots(policy, user, data_dir)
    }
    fn scope_roots_with_home(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: std::path::PathBuf) -> iyakki_core_api::FileRoots {
        crate::scope::file_roots_with_home(policy, user, data_dir, home)
    }
    fn scope_resolve(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        crate::scope::resolve(cwd, input)
    }
    fn scope_resolve_for_write(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        crate::scope::resolve_for_write(cwd, input)
    }
    fn scope_ensure_parent(&self, path: &std::path::Path) -> Result<(), String> {
        crate::scope::ensure_parent(path)
    }
    fn show_all_ai(&self) -> bool {
        Services::show_all_ai(self)
    }
    fn sound_settings(&self) -> iyakki_shared::audioout::SoundSettings {
        Services::sound_settings(self)
    }
    fn speech_kit(&self) -> Arc<dyn SpeechKit> {
        iyakki_agent_api::ServicesB6::speech_kit(&self.arc())
    }
    fn store_facets(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        self.arc().store_facets()
    }
    fn streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>> {
        Services::streamer(self)
    }
    fn system_slots(&self) -> Vec<String> {
        Services::system_slots(self)
    }
    fn system_vault_state(&self) -> (bool, bool) {
        Services::system_vault_state(self)
    }
    fn tasks_facet(&self) -> Arc<dyn TasksFacet> {
        self.arc().tasks_facet()
    }
    fn terminal(&self, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>) -> Box<dyn iyakki_agent_api::Terminal> {
        iyakki_agent_api::ServicesB6::terminal(&self.arc(), user, policy, host)
    }
    fn transcribe_samples(&self, pcm: &[i16]) -> Result<String, String> {
        Services::transcribe_samples(self, pcm)
    }
    fn user_vault_unlocked(&self, user: &str) -> bool {
        Services::user_vault_unlocked(self, user)
    }
    fn vault_facet(&self) -> Arc<dyn VaultRead> {
        self.arc().vault_facet()
    }
    fn vision_model(&self, own: Option<&ModelChoice>) -> Result<Pick, VisionError> {
        Services::vision_model(self, own)
    }
    fn voicedesk_facet(&self) -> Arc<dyn iyakki_agent_api::VoiceDeskFacet> {
        self.arc().voicedesk_facet()
    }
    fn voice_settings(&self) -> VoiceSettings {
        Services::voice_settings(self)
    }
    fn wake_mark(&self, kind: &str) -> Result<(), String> {
        Services::wake_mark(self, kind)
    }
    fn wake_stats_line(&self) -> String {
        Services::wake_stats_line(self)
    }
    fn wrap_cloud_stream_tools(&self, inner: Arc<dyn Tools>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, can_change: bool) -> Arc<dyn Tools> {
        self.arc().wrap_cloud_stream_tools(inner, audit, can_change)
    }
    fn wrap_fleet_tools(&self, inner: Arc<dyn Tools>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        self.arc().wrap_fleet_tools(inner, os, gate, audit)
    }
    fn settings_api(&self) -> Arc<dyn iyakki_agent_api::SettingsApi> {
        Arc::new(self.settings.clone())
    }
    fn settings(&self) -> iyakki_agent_api::SettingsRef {
        iyakki_agent_api::SettingsRef::new(Arc::new(iyakki_agent_api::SettingsSrc(self.settings_api())))
    }
    fn inbox(&self) -> Arc<dyn iyakki_agent_api::InboxApi> {
        self.inbox.clone()
    }
    fn calendar(&self) -> Arc<dyn iyakki_agent_api::CalendarApi> {
        self.calendar.clone()
    }
    fn study(&self) -> Arc<dyn iyakki_agent_api::StudyApi> {
        self.study.clone()
    }
    fn study_calendar(&self) -> Arc<dyn iyakki_agent_api::StudyCalendarApi> {
        Arc::new(iyakki_agent_api::StudyOverCalendar { study: self.study.clone(), calendar: self.calendar.clone() })
    }
    fn secrets(&self) -> &iyakki_shared::vault::Secrets {
        &self.secrets
    }
    fn secrets_api(&self) -> Arc<dyn iyakki_agent_api::member_stores::SecretsApi> {
        Arc::new(self.secrets.clone())
    }
    fn kb(&self) -> Arc<dyn iyakki_agent_api::KnowledgeBaseApi> {
        Arc::new(self.kb.clone())
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        Arc::new(iyakki_agent_api::KbHandle(self.kb()))
    }
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi> {
        self.devwatch.clone()
    }
    fn browser(&self) -> Arc<dyn iyakki_agent_api::BrowserStateApi> {
        self.browser.clone()
    }
    fn browser_consent(&self) -> Arc<dyn iyakki_agent_api::ConsentApi> {
        self.browser.consent.clone()
    }
    fn browser_jobs(&self) -> Arc<dyn iyakki_agent_api::BrowserJobsApi> {
        self.browser.jobs.clone()
    }
    fn screen(&self) -> Arc<dyn iyakki_agent_api::ScreenCaptureApi> {
        self.screen.clone()
    }
    fn camera(&self) -> Arc<dyn iyakki_agent_api::CameraHubApi> {
        self.camera.clone()
    }
    fn devtools(&self) -> Arc<dyn iyakki_agent_api::DevToolsApi> {
        self.devtools.clone()
    }
    fn ext_status(&self) -> Arc<dyn iyakki_agent_api::ExtStatusApi> {
        Arc::new(self.ext_status.clone())
    }
    fn secret_requests(&self) -> Arc<dyn iyakki_agent_api::SecretRequestsApi> {
        self.secret_requests.clone()
    }
    fn usage(&self) -> Arc<dyn iyakki_agent_api::UsageLogApi> {
        self.usage.clone()
    }
    fn sessions(&self) -> Arc<dyn SessionApi> {
        self.sessions.clone()
    }
    fn skills(&self) -> Arc<dyn SkillApi> {
        self.skills.clone()
    }
    fn runs(&self) -> Arc<dyn RunApi> {
        self.runs.clone()
    }
    fn packages(&self) -> Arc<dyn PackageApi> {
        self.packages.clone()
    }
    fn mcp(&self) -> &dyn McpApi {
        &self.mcp
    }
    fn jobs(&self) -> Arc<dyn JobApi> {
        Arc::new(self.jobs.clone())
    }
    fn radio(&self) -> Arc<dyn RadioApi> {
        Arc::new(self.radio.clone())
    }
    fn clis(&self) -> Arc<dyn CliApi> {
        self.clis.clone()
    }
    fn ollama(&self) -> Arc<dyn OllamaApi> {
        self.ollama.clone()
    }
    fn cluster(&self) -> Arc<dyn SandboxCluster> {
        self.cluster.clone()
    }
    fn k3sdev(&self) -> Arc<dyn DevCluster> {
        self.k3sdev.clone()
    }
    fn fleet(&self) -> Arc<dyn FleetApi> {
        Services::fleet(self)
    }
    fn git_kit(&self) -> Arc<dyn iyakki_agent_api::GitKit> {
        iyakki_agent_api::ServicesB6::git_kit(&self.arc())
    }
    fn qi_start_registry(&self, owner: &str, a: &RegistryArgs, k3s_present: bool, launch: &Launch) -> Result<Vec<Job>, String> {
        iyakki_agent_api::ServicesB8::qi_start_registry(self, owner, a, k3s_present, launch)
    }
    fn av_notify_stale(&self, user: &str) {
        iyakki_agent_api::ServicesB1::av_notify_stale(&self.arc(), user)
    }
    fn msg_mirror_tick(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> bool {
        iyakki_agent_api::ServicesB1::msg_mirror_tick(&self.arc(), user, audit)
    }
    fn rag_refresh_tick(&self, user: &str, policy: &iyakki_sec::policy::Policy, audit: iyakki_shared::gitkeys::AuditFn) -> bool {
        iyakki_agent_api::ServicesB1::rag_refresh_tick(&self.arc(), user, policy, audit)
    }
    fn msg_offer_tools(&self, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        iyakki_agent_api::ServicesB1::msg_offer_tools(&self.arc(), tools, user, audit)
    }
    fn page_passkeys(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<std::sync::Mutex<dyn iyakki_agent_api::PagePasskeyHost>>> {
        iyakki_agent_api::ServicesB1::page_passkeys(&self.arc(), user, audit)
    }
    fn page_push(&self, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<std::sync::Mutex<dyn iyakki_agent_api::PagePushHost>>> {
        iyakki_agent_api::ServicesB1::page_push(&self.arc(), user, audit)
    }
    fn msg_prompt_rules(&self, user: &str) -> String {
        iyakki_agent_api::ServicesB1::msg_prompt_rules(&self.arc(), user)
    }
    fn ssh_host(&self, user: &str) -> Arc<dyn iyakki_agent_api::SshHost> {
        iyakki_agent_api::ServicesB1::ssh_host(&self.arc(), user)
    }
    fn ssh_session_tools(&self, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        iyakki_agent_api::ServicesB1::ssh_session_tools(&self.arc(), tools, user, allows_host, audit)
    }
    fn system_speaker(&self) -> Box<dyn iyakki_shared::voiceassist::Speaker> {
        iyakki_agent_api::ServicesB1::system_speaker(&self.arc())
    }
    fn user_tools(&self, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>, mcp: Vec<iyakki_shared::mcp::ServerSpec>, request: Option<&str>, cwd: Option<&Path>) -> Arc<dyn iyakki_shared::agent::Tools> {
        iyakki_agent_api::ServicesB1::user_tools(&self.arc(), user, policy, host, mcp, request, cwd)
    }
    fn voice_turn_log(&self, user: &str) -> Box<dyn iyakki_shared::voiceassist::TurnLog> {
        iyakki_agent_api::ServicesB1::voice_turn_log(&self.arc(), user)
    }
    fn wake_capture_mic(&self) -> Box<dyn iyakki_shared::wakelisten::Mic> {
        iyakki_agent_api::ServicesB1::wake_capture_mic(&self.arc())
    }
    fn lesson_host(&self) -> Arc<dyn iyakki_shared::studylesson::LessonHost> {
        self.arc()
    }
    fn dash_host(&self) -> Arc<dyn iyakki_dash::tools::DashHost> {
        self.arc()
    }
    fn fleet_view(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Result<iyakki_shared::fleetview::Status, String> {
        self.arc().with_fleet(os, gate, audit, |api| api.view())?
    }
    fn fleet_act(&self, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, act: iyakki_shared::fleetview::FleetAct) -> Result<String, String> {
        use iyakki_shared::fleetview::FleetAct;
        self.arc()
            .with_fleet(os, gate, audit, |api| match act {
                FleetAct::Create(n, me) => api.create(&n, &me),
                FleetAct::Offer(f) => api.offer(&f),
                FleetAct::Join(a, c, me) => api.join(&a, &c, &me),
                FleetAct::Approve(r) => api.approve(&r),
                FleetAct::Refuse(r) => api.reject(&r),
                FleetAct::Accept(t) => api.accept_offer(&t),
                FleetAct::Drop(t) => api.drop_offer(&t),
                FleetAct::Remove(f, m) => api.remove(&f, &m),
                FleetAct::Leave(f) => api.leave(&f),
                FleetAct::Admin(f, m, on) => api.admin(&f, &m, on),
                FleetAct::Mode(m) => api.set_accept_word(&m),
                FleetAct::Trust(f, m, on) => api.trust(&f, &m, on),
                FleetAct::Decide(id, yes) => api.decide(&id, yes),
                FleetAct::Tls(on) => api.use_tls(on),
                FleetAct::Address(f, m, a) => api.address(&f, &m, &a),
                FleetAct::Relay(on) => api.use_relay(on),
                FleetAct::Integrate(f, send) => api.integrate(&f, send),
                FleetAct::Finalize(f) => api.finalize(&f),
            })?
    }
    fn announce_name_on(&self) -> bool {
        Services::announce_name_on(self)
    }
    fn check_models(&self, provider_id: &str, cap: Option<usize>) -> Result<iyakki_shared::modelavail::Report, String> {
        Services::check_models(self, provider_id, cap)
    }
    fn clear_vision_cache(&self) {
        Services::clear_vision_cache(self)
    }
    fn cloud_setting(&self) -> iyakki_shared::cloudmode::Setting {
        Services::cloud_setting(self)
    }
    fn collection_review(&self, provider: &str, collection: &str) -> Result<iyakki_shared::omni::Review, String> {
        Services::collection_review(self, provider, collection)
    }
    fn cue_out(&self) -> Arc<dyn CueOut> {
        Services::cue_out(self)
    }
    fn detect_local_servers(&self) -> Vec<String> {
        Services::detect_local_servers(self)
    }
    fn enable_keyfile(&self, path: &str) -> Result<(), String> {
        Services::enable_keyfile(self, path)
    }
    fn fetch_provider_models(&self, provider: &str) -> Result<Vec<String>, String> {
        Services::fetch_provider_models(self, provider)
    }
    fn hw_volume_check(&self, card: u32, device: u32, control: &str) -> Result<String, String> {
        Services::hw_volume_check(self, card, device, control)
    }
    fn install_missing_parts(&self, provider: &str, collection: &str) -> Result<Vec<String>, String> {
        self.arc().install_missing_parts(provider, collection)
    }
    fn is_reindexing(&self) -> bool {
        Services::is_reindexing(self)
    }
    fn lemonade_load(&self, provider: &str, model: &str, load: bool) -> Result<String, String> {
        Services::lemonade_load(self, provider, model, load)
    }
    fn lock_system(&self) {
        Services::lock_system(self)
    }
    fn modelhost_add(&self, n: &NewHost, report: &TestReport) -> Result<Host, String> {
        iyakki_agent_api::ServicesB7::modelhost_add(self, n, report)
    }
    fn modelhost_remove(&self, id: &str) -> Result<(), String> {
        iyakki_agent_api::ServicesB7::modelhost_remove(self, id)
    }
    fn on_password_change(&self, user: &str, old: &str, new: &str) {
        Services::on_password_change(self, user, old, new)
    }
    fn parts_undo_available(&self) -> Option<(String, String)> {
        Services::parts_undo_available(self)
    }
    fn preview_cue(&self, cue: Cue, out: Arc<dyn CueOut>) -> Result<String, String> {
        Services::preview_cue(self, cue, out)
    }
    fn refresh_secret_providers(&self) {
        Services::refresh_secret_providers(self)
    }
    fn reindex_stale_in_background(&self) -> bool {
        self.arc().reindex_stale_in_background()
    }
    fn set_announce_name(&self, v: Option<bool>) -> Result<(), String> {
        Services::set_announce_name(self, v)
    }
    fn set_cloud_setting(&self, v: iyakki_shared::cloudmode::Setting) -> Result<(), String> {
        Services::set_cloud_setting(self, v)
    }
    fn set_startup_reboot_into_iyakki(&self, v: bool) -> Result<(), String> {
        Services::set_startup_reboot_into_iyakki(self, v)
    }
    fn set_web_access(&self, on: bool) -> Result<(), String> {
        Services::set_web_access(self, on)
    }
    fn set_web_access_port(&self, port: u16) -> Result<(), String> {
        Services::set_web_access_port(self, port)
    }
    fn slow_upstream(&self) -> Arc<dyn Upstream> {
        Services::slow_upstream(self)
    }
    fn speech_aligner(&self) -> Option<SpeechEngine> {
        Services::speech_aligner(self)
    }
    fn speech_server_allowed(&self, url: &str) -> bool {
        Services::speech_server_allowed(self, url)
    }
    fn startup_reboot_into_iyakki(&self) -> bool {
        Services::startup_reboot_into_iyakki(self)
    }
    fn synthesize_with_opts(&self, text: &str, opts: &voice::SpeakOpts) -> Result<Vec<u8>, String> {
        Services::synthesize_with_opts(self, text, opts)
    }
    fn system_sounds(&self) -> SystemSounds {
        Services::system_sounds(self)
    }
    fn test_pattern(&self, walk: bool) -> Result<String, String> {
        Services::test_pattern(self, walk)
    }
    fn test_sound(&self, device: Option<&str>) -> Result<String, String> {
        Services::test_sound(self, device)
    }
    fn undo_all_parts(&self, store: &StoryStore) -> Result<String, String> {
        Services::undo_all_parts(self, store)
    }
    fn update_sound_with(&self, f: &mut dyn FnMut(&mut iyakki_shared::audioout::SoundSettings)) -> Result<iyakki_shared::audioout::SoundSettings, String> {
        Services::update_sound(self, |s| f(s))
    }
    fn update_system_sounds_with(&self, f: &mut dyn FnMut(&mut SystemSounds)) -> Result<SystemSounds, String> {
        Services::update_system_sounds(self, |s| f(s))
    }
    fn use_all_parts(&self, provider: &str, collection: &str, store: &StoryStore) -> Result<String, String> {
        Services::use_all_parts(self, provider, collection, store)
    }
    fn use_lemonade_stt(&self, provider: &str, model: &str) -> Result<String, String> {
        Services::use_lemonade_stt(self, provider, model)
    }
    fn use_lemonade_voice(&self, provider: &str, model: &str, voice: Option<&str>) -> Result<String, String> {
        Services::use_lemonade_voice(self, provider, model, voice)
    }
    fn vision_candidates(&self) -> Vec<Cand> {
        Services::vision_candidates(self)
    }
    fn wake_no_model_text(&self) -> Option<String> {
        iyakki_agent_api::ServicesB1::wake_no_model_text(&self.arc())
    }
    fn wake_set_opt_in(&self, on: bool) -> Result<bool, String> {
        iyakki_agent_api::ServicesB1::wake_set_opt_in(&self.arc(), on)
    }
    fn wake_set_sensitivity(&self, sens: u8) -> Result<(), String> {
        iyakki_agent_api::ServicesB1::wake_set_sensitivity(&self.arc(), sens)
    }
    fn web_access_on(&self) -> bool {
        Services::web_access_on(self)
    }
    fn web_access_port(&self) -> u16 {
        Services::web_access_port(self)
    }
    fn host_tts(&self) -> Arc<dyn HostTtsApi> {
        Arc::new(crate::facets_g::HostTtsHandle(self.host_tts.clone()))
    }
    fn lemonade_compute_backend(&self, user: &str) -> Arc<dyn ComputeBackend> {
        Arc::new(crate::lemonadecomputetools::RealBackend { svc: self.arc(), user: user.to_string() })
    }
    fn detect_local_lemonade_in_background(&self) {
        self.arc().detect_local_lemonade_in_background()
    }
    fn detect_local_ollama_in_background(&self) {
        self.arc().detect_local_ollama_in_background()
    }
    fn domains_login(&self, user: &str, audit: AuditFn) {
        self.arc().domains_login(user, audit)
    }
    fn domains_logout(&self, user: &str) {
        self.arc().domains_logout(user)
    }
    fn install_bt_remote(&self, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) {
        self.arc().install_bt_remote(user, audit)
    }
    fn install_pending_judge(&self) {
        self.arc().install_pending_judge()
    }
    fn lock_user_vault(&self, user: &str) {
        Services::lock_user_vault(self, user)
    }
    fn master_toggle_mute(&self) -> Result<iyakki_shared::audioout::Master, String> {
        Services::master_toggle_mute(self)
    }
    fn on_login(&self, user: &str, password: &str, is_admin: bool) -> LoginReport {
        Services::on_login(self, user, password, is_admin)
    }
    fn on_logout(&self, user: &str, is_admin: bool) {
        Services::on_logout(self, user, is_admin)
    }
    fn on_resume(&self, user: &str, is_admin: bool) -> LoginReport {
        Services::on_resume(self, user, is_admin)
    }
    fn sharing_notice_tick(&self, user: &str, admin: bool) -> bool {
        self.arc().sharing_notice_tick(user, admin)
    }
    fn start_device_watch(&self) {
        self.arc().start_device_watch()
    }
    fn unlock_user_vault(&self, user: &str, password: &str) -> Result<(), String> {
        Services::unlock_user_vault(self, user, password)
    }
    fn wake_spawn(&self, listener: Option<iyakki_agent_api::WakeListenerBox>) -> Result<Box<dyn iyakki_agent_api::WakeHandle>, String> {
        iyakki_agent_api::ServicesB1::wake_spawn(&self.arc(), listener)
    }
    fn warm_up_chat(&self) {
        self.arc().warm_up_chat()
    }
    fn chat_backend(&self) -> Arc<dyn ChatBackend> {
        self.arc().chat_backend()
    }
    fn set_chat_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        Services::set_chat_streamer(self, s)
    }
    fn start_sound_output(&self) {
        Services::start_sound_output(self)
    }
    fn start_sound_output_when_ready(&self) {
        Services::start_sound_output_when_ready(self)
    }
    fn router_panel_over(&self, backend: Box<dyn std::any::Any + Send>) -> Option<Arc<dyn iyakki_agent_api::RouterPanel>> {
        iyakki_agent_api::ServicesB6::router_panel_over(&self.arc(), backend)
    }
    // GEN-END
}
