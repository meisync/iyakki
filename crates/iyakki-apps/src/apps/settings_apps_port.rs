//! What the Settings pages of the Apps and AI group ask of the rest of the system (lane ST-B, docs/development/SETTINGS-EXTRACTION-PLAN.md).
//!
//! Small traits, one per kind of thing, each naming only what a page calls. They build on [`PagePort`] (administrator or not, the audit line,
//! the data folder, the clock) and add [`UserPort`] (who is signed in, the recent audit lines). The real implementation is `impl X for AppCx`
//! below, the one place of this group that reads the session, the core, the agent and the services; the fake is in
//! `settings_b_fakes_tests.rs`. A page takes `p: &dyn ThePortItUses`, never the shell context.

use iyakki_core_api::appevents::AppEvents;
use iyakki_shared::skills::{Scope, Skill};

use super::super::hostfiles::HostEnv;
use super::super::settings_port::{PagePort, SettingsDataPort};
use super::super::AppCx;

/// Who is signed in, and what the audit log said lately.
pub(crate) trait UserPort: PagePort {
    /// The signed-in person's account name.
    fn user(&self) -> String;
    /// The last `n` audit lines, newest last.
    fn audit_recent(&self, n: usize) -> Vec<String>;
}

impl UserPort for AppCx {
    fn user(&self) -> String {
        self.session.user.clone()
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        self.audit.recent(n)
    }
}

/// Settings -> App events: the core's event registrations of the person's apps.
pub(crate) trait AppEventsPort: UserPort {
    /// The core's app events (registrations, switches, permissions).
    fn app_events(&self) -> &dyn AppEvents;
}

impl AppEventsPort for AppCx {
    fn app_events(&self) -> &dyn AppEvents {
        self.core.app_events()
    }
}

/// Settings -> Apps access: what installed apps were allowed, taking it away, limiting it, and telling the app.
pub(crate) trait AppAccessPort: AppEventsPort {
    /// Take one capability from one installation (the broker applies it at its next call). `Ok(false)`: it did not hold it.
    fn access_revoke(&self, installation: &str, capability: &str) -> Result<bool, String>;
    /// Limit one capability to `hours` (or none). `Ok(false)`: no such grant.
    fn access_limit(&self, installation: &str, capability: &str, hours: Option<u32>) -> Result<bool, String>;
}

impl AppAccessPort for AppCx {
    fn access_revoke(&self, installation: &str, capability: &str) -> Result<bool, String> {
        self.agent.helpers_b6().access_revoke(AppCx::data_dir(self), installation, capability)
    }
    fn access_limit(&self, installation: &str, capability: &str, hours: Option<u32>) -> Result<bool, String> {
        self.agent.helpers_b6().access_limit(AppCx::data_dir(self), installation, capability, hours)
    }
}

/// Settings -> Host access: the environment the host-folder dialogs work in.
pub(crate) trait HostAccessPort: UserPort {
    /// What the host-folder dialogs need (the data folder, the person, where imports go, the audit sink).
    fn host_env(&self) -> HostEnv;
}

impl HostAccessPort for AppCx {
    fn host_env(&self) -> HostEnv {
        super::super::hostfiles::env_for_cx(self)
    }
}

/// Settings -> Skills: the person's and the shared skills, and reading a skill file only from where the person may read.
pub(crate) trait SkillsPort: UserPort {
    /// The person may add and delete the shared skills.
    fn may_manage_skills(&self) -> bool;
    /// The person's skills and the shared ones.
    fn skills_list(&self) -> Vec<Skill>;
    /// Add a skill from Markdown. The caller has checked who may add shared skills.
    fn skills_add(&self, scope: Scope, markdown: &str) -> Result<Skill, String>;
    /// The text of a skill file, read only from where this person may read.
    fn skill_file_read(&self, path: &str) -> Result<String, String>;
}

impl SkillsPort for AppCx {
    fn may_manage_skills(&self) -> bool {
        self.session.policy.manage_skills
    }
    fn skills_list(&self) -> Vec<Skill> {
        self.skills().list(&self.session.user)
    }
    fn skills_add(&self, scope: Scope, markdown: &str) -> Result<Skill, String> {
        let pol = &self.session.policy;
        self.skills().add(scope, markdown, pol.manage_skills, pol.skills).map_err(|e| e.to_string())
    }
    fn skill_file_read(&self, path: &str) -> Result<String, String> {
        let real = crate::file_scope().resolve(std::path::Path::new("/"), path.trim())?;
        self.core.file_scope().check_read(&self.session.policy, &self.session.user, &real)?;
        let len = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?.len();
        if len > 2 * iyakki_shared::skills::MAX_BODY_BYTES as u64 {
            return Err(format!("{} is too large for a skill", real.display()));
        }
        std::fs::read_to_string(&real).map_err(|e| format!("{}: {e}", real.display()))
    }
}

/// The machine's settings file and the person, for the pages that change both (the settings, the audit line, the data folder, who is signed in).
pub(crate) trait BasePort: SettingsDataPort + UserPort {
    /// The settings as a handle (for the helpers in `iyakki_shared` that take one and change several things at once).
    fn settings_ref(&self) -> iyakki_shared::settings_ref::SettingsRef;
}

impl BasePort for AppCx {
    fn settings_ref(&self) -> iyakki_shared::settings_ref::SettingsRef {
        self.settings()
    }
}

/// The vault and the secret references, for the pages that keep a key.
pub(crate) trait SecretsPort: UserPort {
    /// The reference (`vault:name`) names a secret that exists and can be read.
    fn secret_has_ref(&self, reference: &str) -> bool;
    /// Put a secret into the system vault (`system`) or the person's own vault; the raw answer of the vault.
    fn secret_set(&self, name: &str, value: &str, system: bool) -> Result<(), iyakki_shared::vault::VaultError>;
    /// The names of the system vault's secrets.
    fn secret_system_names(&self) -> Result<Vec<String>, iyakki_shared::vault::VaultError>;
}

impl SecretsPort for AppCx {
    fn secret_has_ref(&self, reference: &str) -> bool {
        self.secrets_api().has_ref(reference)
    }
    fn secret_set(&self, name: &str, value: &str, system: bool) -> Result<(), iyakki_shared::vault::VaultError> {
        if system {
            self.vault().system_set(name, value)
        } else {
            self.vault().user_set(&self.session.user, name, value)
        }
    }
    fn secret_system_names(&self) -> Result<Vec<String>, iyakki_shared::vault::VaultError> {
        self.vault().system_names()
    }
}

/// Keep a secret the way every page does: the vault's refusal in words, and one audit line by name only (never the value).
pub(crate) fn store_secret(p: &dyn SecretsPort, name: &str, value: &str, system: bool) -> Result<(), String> {
    use iyakki_shared::vault::VaultError;
    p.secret_set(name, value, system).map_err(|e| match e {
        VaultError::Locked if system => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
        VaultError::Locked => "Your personal vault is locked (it could not be opened at sign-in).".to_string(),
        other => other.to_string(),
    })?;
    p.audit(if system { "vault.system.set" } else { "vault.user.set" }, name);
    Ok(())
}

/// One tiny request to Google Gemini, run on a worker thread (the page keeps this, never the port itself).
pub(crate) trait GeminiPing: Send + Sync {
    fn ping(&self, model: &str, key_ref: &str, tier: iyakki_shared::gemini::Tier) -> Result<String, String>;
}

/// Settings -> Models, Google Gemini card.
pub(crate) trait GeminiPort: BasePort + SecretsPort {
    /// The sender of the test request.
    fn gemini_pinger(&self) -> std::sync::Arc<dyn GeminiPing>;
}

struct RealGeminiPing(std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl GeminiPing for RealGeminiPing {
    fn ping(&self, model: &str, key_ref: &str, tier: iyakki_shared::gemini::Tier) -> Result<String, String> {
        iyakki_shared::gemini::ping(model, key_ref, tier, &self.0.secrets(), self.0.slow_upstream().as_ref())
    }
}

impl GeminiPort for AppCx {
    fn gemini_pinger(&self) -> std::sync::Arc<dyn GeminiPing> {
        std::sync::Arc::new(RealGeminiPing(self.services.clone()))
    }
}

/// Ask a provider for its model list (a network call, so it runs on a worker thread).
pub(crate) trait ModelFetcher: Send + Sync {
    fn fetch(&self, provider: &str) -> Result<Vec<String>, String>;
}

struct RealModelFetcher(std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl ModelFetcher for RealModelFetcher {
    fn fetch(&self, provider: &str) -> Result<Vec<String>, String> {
        self.0.fetch_provider_models(provider)
    }
}

/// Settings -> Models, the options of one model: the settings file, the person, and the provider's model list.
pub(crate) trait ModelOptsPort: BasePort {
    /// The fetcher of a provider's model list.
    fn model_fetcher(&self) -> std::sync::Arc<dyn ModelFetcher>;
}

impl ModelOptsPort for AppCx {
    fn model_fetcher(&self) -> std::sync::Arc<dyn ModelFetcher> {
        std::sync::Arc::new(RealModelFetcher(self.services.clone()))
    }
}

/// Settings -> Model router: the router's panel (profiles, strategies, the run), the name rule of a strategy script, and where the person writes.
pub(crate) trait RouterPort: UserPort {
    /// The router panel of this person (every change is audited by the panel itself, as the chat tool's is).
    fn router_panel(&self) -> std::sync::Arc<dyn iyakki_agent_api::RouterPanel>;
    /// A strategy script's name is allowed.
    fn router_js_valid_name(&self, name: &str) -> bool;
    /// The folder the person's own files live in (a script is read only from inside it).
    fn home_root_for_writes(&self) -> std::path::PathBuf;
}

impl RouterPort for AppCx {
    fn router_panel(&self) -> std::sync::Arc<dyn iyakki_agent_api::RouterPanel> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.services.router_panel(&self.session.user, std::sync::Arc::new(move |a, d| audit.record(&user, a, d)), self.os.router())
    }
    fn router_js_valid_name(&self, name: &str) -> bool {
        self.agent.router_js_valid_name(name)
    }
    fn home_root_for_writes(&self) -> std::path::PathBuf {
        self.core.file_scope().home_root_for_writes(&self.session.user, AppCx::data_dir(self))
    }
}

/// Settings -> Assistant: the work history of the workspaces (the knowledge base), the Waiting ledger and its rules.
pub(crate) trait AssistantPort: BasePort {
    /// Where the history of the person's workspaces is kept.
    fn kb_source(&self) -> std::sync::Arc<dyn iyakki_memx::kb::KbSource>;
    /// The pending-work ledger (the person's Waiting settings and wakes left).
    fn pending_ledger(&self) -> std::sync::Arc<dyn iyakki_agent_api::pending::AgentPending>;
    /// The rows of the Waiting settings for these settings.
    fn pendingplus_settings_rows(&self, s: &iyakki_shared::pending::Settings, wakes_left: u32) -> Vec<iyakki_shared::pendingplus::Row>;
    /// Change one Waiting setting by its key (checked); the words of a refusal.
    fn pendingplus_apply_setting(&self, s: &mut iyakki_shared::pending::Settings, key: &str, value: &str) -> Result<(), String>;
}

impl AssistantPort for AppCx {
    fn kb_source(&self) -> std::sync::Arc<dyn iyakki_memx::kb::KbSource> {
        AppCx::kb_source(self)
    }
    fn pending_ledger(&self) -> std::sync::Arc<dyn iyakki_agent_api::pending::AgentPending> {
        self.pendingapi()
    }
    fn pendingplus_settings_rows(&self, s: &iyakki_shared::pending::Settings, wakes_left: u32) -> Vec<iyakki_shared::pendingplus::Row> {
        self.agent.pendingplus_settings_rows(s, wakes_left)
    }
    fn pendingplus_apply_setting(&self, s: &mut iyakki_shared::pending::Settings, key: &str, value: &str) -> Result<(), String> {
        self.agent.pendingplus_apply_setting(s, key, value)
    }
}

/// Keep a tested model host in Models, or take it out (a job: the vault and the settings are written).
pub(crate) trait ModelHostOps: Send + Sync {
    fn add(&self, n: &iyakki_shared::modelhost::NewHost, report: &iyakki_shared::modelhost::TestReport) -> Result<iyakki_shared::modelhost::Host, String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}

struct RealModelHostOps(std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl ModelHostOps for RealModelHostOps {
    fn add(&self, n: &iyakki_shared::modelhost::NewHost, report: &iyakki_shared::modelhost::TestReport) -> Result<iyakki_shared::modelhost::Host, String> {
        self.0.modelhost_add(n, report)
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.0.modelhost_remove(id)
    }
}

/// Settings -> Models -> Model host on your network.
pub(crate) trait ModelHostPort: BasePort {
    /// What keeps or removes a host in Models.
    fn modelhost_ops(&self) -> std::sync::Arc<dyn ModelHostOps>;
    /// The cluster the host's Service is published in.
    fn modelhost_kube(&self) -> std::sync::Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync>;
    /// The key of a host, read from the vault (`vault:<id>-key`); none when it has no key or the vault is locked.
    fn modelhost_key(&self, host_id: &str) -> Option<String>;
}

impl ModelHostPort for AppCx {
    fn modelhost_ops(&self) -> std::sync::Arc<dyn ModelHostOps> {
        std::sync::Arc::new(RealModelHostOps(self.services.clone()))
    }
    fn modelhost_kube(&self) -> std::sync::Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync> {
        self.k3sdev().modelhost_kube()
    }
    fn modelhost_key(&self, host_id: &str) -> Option<String> {
        self.secrets().resolve(&format!("vault:{host_id}-key"), None).ok().map(|k| k.to_string())
    }
}

/// The two slow requests of the Agents page (a test of an agent, a check of a key's models): they run on a worker thread.
pub(crate) trait AgentTester: Send + Sync {
    /// The version, the sign-in check, then one tiny request through the same machinery a chat uses. `key_ref` names the key to pass, if any.
    fn test(&self, agent: std::sync::Arc<dyn iyakki_shared::extagent::ExternalAgent>, key_ref: Option<String>) -> Result<String, String>;
    /// Which models a provider's key can use, in words.
    fn check_models(&self, provider: &str) -> Result<String, String>;
}

struct RealAgentTester(std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl AgentTester for RealAgentTester {
    fn test(&self, agent: std::sync::Arc<dyn iyakki_shared::extagent::ExternalAgent>, key_ref: Option<String>) -> Result<String, String> {
        let svc = &self.0;
        let key = key_ref.and_then(|r| svc.secrets().resolve(&r, None).ok());
        test_agent(&*svc.ext_status(), svc.extagent_facet(), agent, &std::env::temp_dir(), key)
    }
    fn check_models(&self, provider: &str) -> Result<String, String> {
        self.0.check_models(provider, None).map(|r| r.words())
    }
}

/// Settings -> Agents: the agents' configuration and status, the CLI tile's install, the vault names, and the slow requests.
pub(crate) trait AgentsPort: BasePort + SecretsPort {
    /// The agents' configuration as the chat reads it (the settings with the paths filled in).
    fn agents_cfg(&self) -> iyakki_shared::extagentcfg::AgentsCfg;
    /// What was found about each agent (installed, version, signed in) and the models its program lists.
    fn ext_status(&self) -> std::sync::Arc<dyn iyakki_agent_api::ExtStatusApi>;
    /// What installing this program from the CLI tile would run, in words; none when it cannot be installed from here.
    fn cli_install_plan(&self, cli: &str) -> Option<String>;
    /// Start the install in the CLI tile (progress is there).
    fn cli_install(&self, cli: &str) -> Result<(), String>;
    /// The sender of the slow requests.
    fn agent_tester(&self) -> std::sync::Arc<dyn AgentTester>;
    /// Whether the person is signed in to the vendor program (the vendor's own status; never a password).
    fn agent_login_state(&self, agent: &str, installed: bool) -> crate::agent_login::State;
}

impl AgentsPort for AppCx {
    fn agents_cfg(&self) -> iyakki_shared::extagentcfg::AgentsCfg {
        AppCx::agents_cfg(self)
    }
    fn ext_status(&self) -> std::sync::Arc<dyn iyakki_agent_api::ExtStatusApi> {
        AppCx::ext_status(self)
    }
    fn cli_install_plan(&self, cli: &str) -> Option<String> {
        self.services.clis().install_plan(cli)
    }
    fn cli_install(&self, cli: &str) -> Result<(), String> {
        self.services.clis().install(cli, &self.session.user)
    }
    fn agent_tester(&self) -> std::sync::Arc<dyn AgentTester> {
        std::sync::Arc::new(RealAgentTester(self.services.clone()))
    }
    fn agent_login_state(&self, agent: &str, installed: bool) -> crate::agent_login::State {
        crate::agent_login::state(self, agent, installed)
    }
}

/// The test button: the version, the sign-in status, then one tiny request through the same machinery a chat uses.
fn test_agent(status: &dyn iyakki_agent_api::ExtStatusApi, runner: std::sync::Arc<dyn iyakki_agent_api::ExtAgentFacet>, agent: std::sync::Arc<dyn iyakki_shared::extagent::ExternalAgent>, ws: &std::path::Path, key: Option<zeroize::Zeroizing<String>>) -> Result<String, String> {
    let st = iyakki_shared::extagent::detect(agent.as_ref(), crate::agent().exec().system().as_ref(), ws);
    status.set(&agent.id(), st.clone());
    if !st.ready() {
        return Err(st.fix);
    }
    let req = iyakki_agent_api::ExtRunSpec {
        agent: agent.id(),
        workspace: ws.to_path_buf(),
        prompt: "Reply with the single word OK.".into(),
        system: std::sync::Arc::new(String::new),
        model: agent.cfg_model(),
        resume: None,
        tools: std::sync::Arc::new(iyakki_shared::agent::NoTools),
        limits: iyakki_shared::agent::RunLimits { max_steps: 1, max_seconds: 90, ..Default::default() },
        allow_auto: false,
        key,
        audit: std::sync::Arc::new(|_, _| {}),
        exe: std::env::current_exe().unwrap_or_default(),
        on_session: std::sync::Arc::new(|_| {}),
        on_usage: std::sync::Arc::new(|_, _, _, _| {}),
        no_tools: false,
    };
    let h = runner.start(req);
    while let Ok(e) = h.events.recv_timeout(std::time::Duration::from_secs(120)) {
        match e {
            iyakki_shared::agent::RunEvent::Final { text } => return Ok(format!("{} {}: answered “{}”.", agent.label(), st.version, text.trim().chars().take(40).collect::<String>())),
            iyakki_shared::agent::RunEvent::Failed { error } => return Err(error),
            iyakki_shared::agent::RunEvent::Stopped { reason } => return Err(reason),
            _ => {}
        }
    }
    Err("No answer within two minutes.".into())
}

/// Test an external RAG backend (a network call, so it runs on a worker thread).
pub(crate) trait RagBackendTester: Send + Sync {
    fn test(&self, backend: iyakki_shared::rag_ext::ExternalRag) -> Result<String, String>;
}

struct RealRagBackendTester(std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl RagBackendTester for RealRagBackendTester {
    fn test(&self, backend: iyakki_shared::rag_ext::ExternalRag) -> Result<String, String> {
        iyakki_shared::rag_ext::ExtClient::new(backend, self.0.secrets().clone(), self.0.quick_upstream()).test()
    }
}

/// Settings -> Documents (RAG): the knowledge base, where the person's documents are, and the backend test.
pub(crate) trait RagPort: BasePort {
    /// The knowledge base (namespaces, statistics, re-embedding, indexing a folder).
    fn kb(&self) -> std::sync::Arc<dyn iyakki_agent_api::member_stores::KnowledgeBaseApi>;
    /// The folder the documents to index are kept in (one sub-folder per person, and `shared`).
    fn rag_docs_dir(&self) -> std::path::PathBuf;
    /// Documents are being re-embedded in the background right now.
    fn kb_is_reindexing(&self) -> bool;
    /// The tester of an external backend.
    fn rag_backend_tester(&self) -> std::sync::Arc<dyn RagBackendTester>;
}

impl RagPort for AppCx {
    fn kb(&self) -> std::sync::Arc<dyn iyakki_agent_api::member_stores::KnowledgeBaseApi> {
        AppCx::kb(self)
    }
    fn rag_docs_dir(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.cfg.rag_docs)
    }
    fn kb_is_reindexing(&self) -> bool {
        self.services.is_reindexing()
    }
    fn rag_backend_tester(&self) -> std::sync::Arc<dyn RagBackendTester> {
        std::sync::Arc::new(RealRagBackendTester(self.services.clone()))
    }
}

/// One macro lists a group of services calls once and makes the trait, the real forwarder over the services handle and nothing else: the fake
/// is written by hand in `settings_b_fakes_tests.rs` (so a test says what each call answers).
macro_rules! service_ops {
    ($(#[$m:meta])* $tr:ident, $real:ident { $( $(#[$fm:meta])* fn $name:ident(&self $(, $arg:ident : $ty:ty)*) -> $ret:ty; )* }) => {
        $(#[$m])*
        pub(crate) trait $tr: Send + Sync {
            $( $(#[$fm])* fn $name(&self $(, $arg: $ty)*) -> $ret; )*
        }
        pub(crate) struct $real(pub(crate) std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);
        impl $tr for $real {
            $( fn $name(&self $(, $arg: $ty)*) -> $ret { self.0.$name($($arg),*) } )*
        }
    };
}

use iyakki_shared::lemonade::Model as LemModel;
use iyakki_shared::story::StoryStore;

service_ops! {
    /// What the Models pages ask of the services: the Lemonade servers (models, load, pull, collections), the voice and speech choices they
    /// lead to, the vision candidates and the local servers. A page clones the handle into a worker thread when a call is slow.
    LemonadeOps, RealLemonadeOps {
        fn lemonade_cached(&self, provider: &str) -> Vec<LemModel>;
        fn refresh_lemonade(&self, provider: &str) -> Result<Vec<LemModel>, String>;
        fn detect_local_servers(&self) -> Vec<String>;
        fn collection_review(&self, provider: &str, collection: &str) -> Result<iyakki_shared::omni::Review, String>;
        fn parts_undo_available(&self) -> Option<(String, String)>;
        fn undo_all_parts(&self, store: &StoryStore) -> Result<String, String>;
        fn use_lemonade_voice(&self, provider: &str, model: &str, voice: Option<&str>) -> Result<String, String>;
        fn use_lemonade_stt(&self, provider: &str, model: &str) -> Result<String, String>;
        fn speech_server_allowed(&self, url: &str) -> bool;
        fn allow_speech_server(&self, url: &str) -> Result<(), String>;
        fn reindex_stale_in_background(&self) -> bool;
        fn lemonade_load(&self, provider: &str, model: &str, load: bool) -> Result<String, String>;
        fn pull_model_for(&self, user: &str, provider: &str, model: &str) -> Result<(), String>;
        fn install_missing_parts(&self, provider: &str, collection: &str) -> Result<Vec<String>, String>;
        fn use_all_parts(&self, provider: &str, collection: &str, store: &StoryStore) -> Result<String, String>;
        fn pull_status(&self) -> Vec<iyakki_shared::services::PullStatus>;
        fn lemonade_compute_backend(&self, user: &str) -> std::sync::Arc<dyn iyakki_agent_api::ComputeBackend>;
        fn vision_candidates(&self) -> Vec<iyakki_shared::visionmodel::Cand>;
        fn vision_model(&self, own: Option<&iyakki_shared::settings::ModelChoice>) -> Result<iyakki_shared::visionmodel::Pick, iyakki_shared::visionmodel::VisionError>;
    }
}

/// The part of the Lemonade pages that is not one services call: the speech and the provider that a detection saves, run on a worker.
pub(crate) trait DetectOps: Send + Sync {
    /// The quick HTTP client for a short GET (the server type is asked with it).
    fn quick_upstream(&self) -> std::sync::Arc<dyn iyakki_proxy::Upstream>;
    /// Put a secret into the system vault.
    fn secret_system_set(&self, name: &str, value: &str) -> Result<(), iyakki_shared::vault::VaultError>;
    /// Save a provider into the settings (checked like every change).
    fn provider_upsert(&self, cfg: iyakki_shared::providers::ProviderCfg) -> Result<(), String>;
}

pub(crate) struct RealDetectOps(pub(crate) std::sync::Arc<dyn iyakki_agent_api::ServicesHandle>);

impl DetectOps for RealDetectOps {
    fn quick_upstream(&self) -> std::sync::Arc<dyn iyakki_proxy::Upstream> {
        self.0.quick_upstream()
    }
    fn secret_system_set(&self, name: &str, value: &str) -> Result<(), iyakki_shared::vault::VaultError> {
        self.0.vault_facet().system_set(name, value)
    }
    fn provider_upsert(&self, cfg: iyakki_shared::providers::ProviderCfg) -> Result<(), String> {
        self.0.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).upsert_provider(cfg).map_err(|e| e.to_string())
    }
}

/// Settings -> Models, the Lemonade pages and the provider forms: the services behind them, the agent's facets, the story store and the AI client.
/// The agent's facets and the AI client, for the pages that ask for the words of a place, the machine's profile, the voice desk and so on.
pub(crate) trait AgentPort: BasePort {
    /// The agent's facets (a fake answers with the default and records the call).
    fn agent(&self) -> &dyn iyakki_agent_api::AgentApi;
    /// The same, as a handle a worker thread can hold.
    fn agent_arc(&self) -> std::sync::Arc<dyn iyakki_agent_api::AgentApi>;
    /// The AI client (provider checks, tests and one chat turn, the speech engines); cloned into a worker.
    fn ai(&self) -> std::sync::Arc<dyn iyakki_agent_api::AiRead>;
    /// The speech engines of this computer.
    fn host_tts(&self) -> std::sync::Arc<dyn iyakki_agent_api::HostTtsApi> {
        self.ai().host_tts()
    }
}

impl AgentPort for AppCx {
    fn agent(&self) -> &dyn iyakki_agent_api::AgentApi {
        &*self.agent
    }
    fn agent_arc(&self) -> std::sync::Arc<dyn iyakki_agent_api::AgentApi> {
        self.agent.clone()
    }
    fn ai(&self) -> std::sync::Arc<dyn iyakki_agent_api::AiRead> {
        AppCx::ai(self)
    }
}

pub(crate) trait LemonadePort: AgentPort + SecretsPort {
    /// The services calls of the Models pages.
    fn lemonade(&self) -> std::sync::Arc<dyn LemonadeOps>;
    /// The detection and saving of a server found on the network (worker side).
    fn detect_ops(&self) -> std::sync::Arc<dyn DetectOps>;
    /// The person's story store (TALES), when this core serves it.
    fn story_store(&self) -> Option<iyakki_shared::story::StoryStore>;
    /// A provider's raw error put into plain words (a function a worker can hold).
    fn plain_error(&self) -> std::sync::Arc<dyn Fn(&str, &str) -> String + Send + Sync>;
}

impl LemonadePort for AppCx {
    fn lemonade(&self) -> std::sync::Arc<dyn LemonadeOps> {
        std::sync::Arc::new(RealLemonadeOps(self.services.clone()))
    }
    fn detect_ops(&self) -> std::sync::Arc<dyn DetectOps> {
        std::sync::Arc::new(RealDetectOps(self.services.clone()))
    }
    fn story_store(&self) -> Option<iyakki_shared::story::StoryStore> {
        self.b5().story_store(&self.session.policy, &self.session.user, AppCx::data_dir(self))
    }
    fn plain_error(&self) -> std::sync::Arc<dyn Fn(&str, &str) -> String + Send + Sync> {
        let agent = self.agent.clone();
        std::sync::Arc::new(move |provider, raw| agent.helpers_b4().provider_plain_error(provider, raw))
    }
}

/// Settings -> Models as a whole: every group the page opens (the Lemonade pages, the Gemini card, the model options, the model host).
pub(crate) trait ModelsPort: LemonadePort + GeminiPort + ModelOptsPort + ModelHostPort {}
impl<T: LemonadePort + GeminiPort + ModelOptsPort + ModelHostPort + ?Sized> ModelsPort for T {}

service_ops! {
    /// What the Voice pages ask of the services: the voice settings as they are now, the speech engines' work, and the wake phrase.
    VoiceOps, RealVoiceOps {
        fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings;
        fn transcribe_samples(&self, pcm: &[i16]) -> Result<String, String>;
        fn synthesize_with_opts(&self, text: &str, opts: &iyakki_shared::voice::SpeakOpts) -> Result<Vec<u8>, String>;
        fn speech_aligner(&self) -> Option<iyakki_shared::voice::SpeechEngine>;
        fn wake_set_sensitivity(&self, sens: u8) -> Result<(), String>;
        fn wake_set_opt_in(&self, on: bool) -> Result<bool, String>;
        fn wake_no_model_text(&self) -> Option<String>;
        fn wake_stats_line(&self) -> String;
    }
}

/// Settings -> Voice and the wake phrase: the services behind them, the microphone and speaker permission, the wording fixer.
pub(crate) trait VoicePort: AgentPort + SecretsPort {
    /// The voice services (a page clones the handle into a worker when a call is slow).
    fn voice_ops(&self) -> std::sync::Arc<dyn VoiceOps>;
    /// The account may use the microphone and the speaker (set per person under Users).
    fn may_use_voice(&self) -> bool;
    /// Switch the "correct my wording" step on or off (the dictation fixer of the agent).
    fn dict_fix_set(&self, on: bool) -> Result<(), String>;
}

impl VoicePort for AppCx {
    fn voice_ops(&self) -> std::sync::Arc<dyn VoiceOps> {
        std::sync::Arc::new(RealVoiceOps(self.services.clone()))
    }
    fn may_use_voice(&self) -> bool {
        self.session.policy.voice
    }
    fn dict_fix_set(&self, on: bool) -> Result<(), String> {
        self.agent.dict_fix().set_enabled(&self.services, on)
    }
}
