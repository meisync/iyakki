//! The fake port of the Apps and AI Settings pages (lane ST-B): one value that implements every group trait of `settings_apps_port.rs`
//! (and the model, secret and voice groups beside it), with everything in memory: the person, an administrator switch, the audit lines, a
//! data folder the test gives it, the scripted app events of `iyakki-core-fake`, skills, files and a manual clock. Nothing is read from the
//! machine and nothing sleeps.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use iyakki_core_api::appevents::AppEvents;
use iyakki_core_fake::FakeAppEvents;
use iyakki_shared::skills::{Scope, Skill};

use super::apps_port::*;
use super::super::hostfiles::HostEnv;
use super::super::settings_port::{PagePort, SettingsDataPort, SettingsWriteError};
use iyakki_shared::settings::{Settings, SettingsStore};
use iyakki_shared::settings_ref::SettingsRef;
use iyakki_shared::vault::VaultError;
use std::sync::Arc;

pub(crate) struct FakeB {
    pub admin: AtomicBool,
    pub user: String,
    pub dir: PathBuf,
    pub clock: iyakki_testkit::ManualClock,
    audit_lines: Mutex<Vec<(String, String)>>,
    pub events: FakeAppEvents,
    /// What `access_revoke` and `access_limit` were asked: (installation, capability, hours).
    pub access_calls: Mutex<Vec<(String, String, Option<u32>)>>,
    /// The next access call answers this refusal.
    pub access_refusal: Mutex<Option<String>>,
    /// The person may add and delete shared skills.
    pub manage_skills: AtomicBool,
    pub skills: Mutex<Vec<Skill>>,
    /// Files the person may read, by path; a path that is not here is refused.
    pub files: Mutex<HashMap<String, String>>,
    /// The settings file (a real store in the fake's own folder, so the helpers that take a handle work unchanged).
    pub store: Arc<Mutex<SettingsStore>>,
    /// The vault: (system, name) to value; locked, every write is refused.
    pub secrets: Mutex<HashMap<(bool, String), String>>,
    pub vault_locked: AtomicBool,
    /// What a provider's model list answers.
    pub model_list: Arc<Mutex<Result<Vec<String>, String>>>,
    /// The knowledge base the workspace history lives in (in memory).
    pub kb: Arc<Mutex<iyakki_memx::kb::KnowledgeBase>>,
    /// The Waiting settings of the person and the wakes left today.
    pub ledger: Arc<FakeLedger>,
    /// Documents are being re-embedded in the background, and what a backend test answers.
    pub reindexing: AtomicBool,
    pub backend_tests: Arc<Mutex<Vec<String>>>,
    pub backend_answer: Arc<Mutex<Result<String, String>>>,
    /// What the voice services answer and were asked.
    pub voice: Arc<FakeVoice>,
    /// The account may use the microphone and the speaker.
    pub voice_allowed: AtomicBool,
    /// What the Lemonade pages' services answer and were asked, the model clients, and the story store.
    pub lem: Arc<FakeLemonade>,
    pub ai: Arc<FakeAi>,
    /// The agent's facets, recording each call and answering with the default.
    pub agent_api: Arc<iyakki_agent_api::FakeAgentApi>,
    /// The agents' status cache and what the slow requests and the CLI install answer.
    pub agents: Arc<FakeAgents>,
    /// What the model-host port did and what it answers.
    pub mhost: Arc<FakeModelHosts>,
    /// The router panel the pages see.
    pub router: Arc<FakeRouter>,
    /// What the Gemini test request answers.
    pub ping_answer: Arc<Mutex<Result<String, String>>>,
    /// The settings are held busy: every change answers `Busy`.
    pub busy: AtomicBool,
}

impl FakeB {
    pub(crate) fn new(admin: bool, dir: PathBuf) -> FakeB {
        FakeB {
            admin: admin.into(),
            user: "amy".into(),
            clock: iyakki_testkit::ManualClock::new(1_000_000),
            audit_lines: Default::default(),
            events: FakeAppEvents::new(),
            access_calls: Default::default(),
            access_refusal: Default::default(),
            manage_skills: admin.into(),
            skills: Default::default(),
            files: Default::default(),
            store: Arc::new(Mutex::new(SettingsStore::open(&dir.join("settings.json")).expect("a fresh settings file"))),
            dir,
            secrets: Default::default(),
            vault_locked: false.into(),
            model_list: Arc::new(Mutex::new(Ok(vec!["small-7b".into(), "big-70b".into()]))),
            kb: Arc::new(Mutex::new(iyakki_memx::kb::KnowledgeBase::in_memory(iyakki_memx::kb::default_embedder()))),
            ledger: Arc::new(FakeLedger::default()),
            voice: Arc::new(FakeVoice { store: Mutex::new(None), calls: Default::default(), refusal: Default::default() }),
            voice_allowed: true.into(),
            lem: Arc::new(FakeLemonade::default()),
            ai: Arc::new(FakeAi::default()),
            agent_api: Default::default(),
            reindexing: false.into(),
            backend_tests: Default::default(),
            backend_answer: Arc::new(Mutex::new(Ok("answered".into()))),
            agents: Arc::new(FakeAgents::default()),
            mhost: Arc::new(FakeModelHosts::default()),
            router: Arc::new(FakeRouter::default()),
            ping_answer: Arc::new(Mutex::new(Ok("OK".into()))),
            busy: false.into(),
        }
        .with_voice_store()
    }
    fn with_voice_store(self) -> FakeB {
        *self.voice.store.lock().unwrap() = Some(self.store.clone());
        self
    }
    /// Every skill the fake holds (all scopes).
    pub(crate) fn skills_list_for_test(&self) -> Vec<Skill> {
        self.skills.lock().unwrap().clone()
    }
    /// The (action, subject) pairs the pages audited.
    pub(crate) fn audited(&self) -> Vec<(String, String)> {
        self.audit_lines.lock().unwrap().clone()
    }
}

impl PagePort for FakeB {
    fn is_admin(&self) -> bool {
        self.admin.load(Ordering::SeqCst)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit_lines.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }
}

impl SettingsDataPort for FakeB {
    fn settings_get(&self) -> Settings {
        SettingsRef::of_store(&self.store).lock().unwrap().get().clone()
    }
    fn settings_update(&self, f: &mut dyn FnMut(&mut Settings)) -> Result<(), SettingsWriteError> {
        if self.busy.load(Ordering::SeqCst) {
            return Err(SettingsWriteError::Busy);
        }
        SettingsRef::of_store(&self.store).lock().unwrap().update(|s| f(s)).map_err(|e| SettingsWriteError::Refused(e.to_string()))
    }
}

impl BasePort for FakeB {
    fn settings_ref(&self) -> SettingsRef {
        SettingsRef::of_store(&self.store)
    }
}

impl SecretsPort for FakeB {
    fn secret_has_ref(&self, reference: &str) -> bool {
        let name = reference.strip_prefix("vault:").unwrap_or(reference);
        self.secrets.lock().unwrap().keys().any(|(_, n)| n == name)
    }
    fn secret_set(&self, name: &str, value: &str, system: bool) -> Result<(), VaultError> {
        if self.vault_locked.load(Ordering::SeqCst) {
            return Err(VaultError::Locked);
        }
        self.secrets.lock().unwrap().insert((system, name.to_string()), value.to_string());
        Ok(())
    }
    fn secret_system_names(&self) -> Result<Vec<String>, VaultError> {
        if self.vault_locked.load(Ordering::SeqCst) {
            return Err(VaultError::Locked);
        }
        let mut v: Vec<String> = self.secrets.lock().unwrap().keys().filter(|(sys, _)| *sys).map(|(_, n)| n.clone()).collect();
        v.sort();
        Ok(v)
    }
}

struct FakePing(Arc<Mutex<Result<String, String>>>);
impl GeminiPing for FakePing {
    fn ping(&self, _model: &str, _key_ref: &str, _tier: iyakki_shared::gemini::Tier) -> Result<String, String> {
        self.0.lock().unwrap().clone()
    }
}

impl GeminiPort for FakeB {
    fn gemini_pinger(&self) -> Arc<dyn GeminiPing> {
        Arc::new(FakePing(self.ping_answer.clone()))
    }
}

impl UserPort for FakeB {
    fn user(&self) -> String {
        self.user.clone()
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        let all: Vec<String> = self.audit_lines.lock().unwrap().iter().map(|(a, s)| format!("1 {} {a} {s}", self.user)).collect();
        all[all.len().saturating_sub(n)..].to_vec()
    }
}

impl AppEventsPort for FakeB {
    fn app_events(&self) -> &dyn AppEvents {
        &self.events
    }
}

impl AppAccessPort for FakeB {
    fn access_revoke(&self, installation: &str, capability: &str) -> Result<bool, String> {
        if let Some(e) = self.access_refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.access_calls.lock().unwrap().push((installation.into(), capability.into(), None));
        Ok(true)
    }
    fn access_limit(&self, installation: &str, capability: &str, hours: Option<u32>) -> Result<bool, String> {
        if let Some(e) = self.access_refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.access_calls.lock().unwrap().push((installation.into(), capability.into(), hours));
        Ok(true)
    }
}

impl HostAccessPort for FakeB {
    fn host_env(&self) -> HostEnv {
        let lines = std::sync::Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        HostEnv { data_dir: self.dir.clone(), user: self.user.clone(), dest: self.dir.join("Uploads"), dest_label: "your Uploads folder".into(), audit: std::sync::Arc::new(move |a, d| lines.lock().unwrap().push((a.to_string(), d.to_string()))) }
    }
}

/// `name`, `description` and `triggers` from the front matter of a skill file, and the body after it.
fn parse_skill(md: &str, scope: Scope) -> Result<Skill, String> {
    let rest = md.strip_prefix("---\n").ok_or("a skill starts with a --- line")?;
    let (head, body) = rest.split_once("\n---\n").ok_or("the front matter has no end")?;
    let field = |k: &str| head.lines().find_map(|l| l.strip_prefix(&format!("{k}:"))).map(|v| v.trim().to_string()).unwrap_or_default();
    let name = field("name");
    if name.is_empty() {
        return Err("a skill needs a name".into());
    }
    let triggers = field("triggers").split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    Ok(Skill { scope, name, description: field("description"), triggers, body: body.trim().to_string() })
}

impl SkillsPort for FakeB {
    fn may_manage_skills(&self) -> bool {
        self.manage_skills.load(Ordering::SeqCst)
    }
    fn skills_list(&self) -> Vec<Skill> {
        self.skills.lock().unwrap().iter().filter(|s| s.scope == Scope::Shared || s.scope == Scope::User(self.user.clone())).cloned().collect()
    }
    fn skills_add(&self, scope: Scope, markdown: &str) -> Result<Skill, String> {
        if scope == Scope::Shared && !self.may_manage_skills() {
            return Err("you may not change shared skills".into());
        }
        let sk = parse_skill(markdown, scope)?;
        let mut all = self.skills.lock().unwrap();
        all.retain(|s| !(s.scope == sk.scope && s.name == sk.name));
        all.push(sk.clone());
        Ok(sk)
    }
    fn skill_file_read(&self, path: &str) -> Result<String, String> {
        self.files.lock().unwrap().get(path.trim()).cloned().ok_or_else(|| format!("{path}: you may not read this"))
    }
}

struct FakeFetcher(Arc<Mutex<Result<Vec<String>, String>>>);
impl ModelFetcher for FakeFetcher {
    fn fetch(&self, _provider: &str) -> Result<Vec<String>, String> {
        self.0.lock().unwrap().clone()
    }
}

impl ModelOptsPort for FakeB {
    fn model_fetcher(&self) -> Arc<dyn ModelFetcher> {
        Arc::new(FakeFetcher(self.model_list.clone()))
    }
}

/// A scripted router panel: what it knows, what it was asked, and the refusal the next change answers.
#[derive(Default)]
pub(crate) struct FakeRouter {
    pub profiles: Mutex<Vec<iyakki_shared::router_registry::Profile>>,
    pub strategies: Mutex<Vec<(String, usize)>>,
    pub asked: Mutex<Vec<(String, serde_json::Value)>>,
    pub refusal: Mutex<Option<String>>,
    pub progress: Mutex<Option<String>>,
}

impl FakeRouter {
    /// The default budget as the four form values (tokens, calls, tries, rounds).
    pub(crate) fn config_for_test(&self) -> (String, String, String, String) {
        let b = iyakki_agent_api::RouterPanel::config(self).budget;
        (b.max_tokens.to_string(), b.max_calls.to_string(), b.max_attempts.to_string(), b.max_escalations.to_string())
    }
    fn note(&self, what: &str, args: &serde_json::Value) -> Result<String, String> {
        if let Some(e) = self.refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.asked.lock().unwrap().push((what.to_string(), args.clone()));
        Ok(format!("{what} done"))
    }
}

impl iyakki_agent_api::RouterPanel for FakeRouter {
    fn profiles(&self) -> Vec<iyakki_shared::router_registry::Profile> {
        self.profiles.lock().unwrap().clone()
    }
    fn strategies(&self) -> Vec<(String, usize)> {
        self.strategies.lock().unwrap().clone()
    }
    fn progress(&self) -> Option<String> {
        self.progress.lock().unwrap().clone()
    }
    fn set(&self, args: &serde_json::Value) -> Result<String, String> {
        self.note("set", args)
    }
    fn strategy_set(&self, args: &serde_json::Value) -> Result<String, String> {
        self.note("strategy_set", args)
    }
    fn strategy_remove(&self, args: &serde_json::Value) -> Result<String, String> {
        self.note("strategy_remove", args)
    }
    fn strategy_add(&self, args: &serde_json::Value) -> Result<String, String> {
        self.note("strategy_add", args)
    }
    fn cancel(&self) -> Result<String, String> {
        self.note("cancel", &serde_json::Value::Null)
    }
}

impl RouterPort for FakeB {
    fn router_panel(&self) -> Arc<dyn iyakki_agent_api::RouterPanel> {
        self.router.clone()
    }
    fn router_js_valid_name(&self, name: &str) -> bool {
        !name.is_empty() && name != "default" && name.len() <= 32 && name.starts_with(|c: char| c.is_ascii_lowercase()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    }
    fn home_root_for_writes(&self) -> PathBuf {
        self.dir.join("home")
    }
}

/// The Waiting ledger of one person: the settings and the wakes left.
#[derive(Default)]
pub(crate) struct FakeLedger {
    pub settings: Mutex<iyakki_shared::pending::Settings>,
    pub wakes_left: std::sync::atomic::AtomicU32,
}

impl iyakki_agent_api::pending::AgentPending for FakeLedger {
    fn pending_settings(&self, _user: &str) -> iyakki_shared::pending::Settings {
        self.settings.lock().unwrap().clone()
    }
    fn pending_set_settings(&self, _user: &str, s: iyakki_shared::pending::Settings) {
        *self.settings.lock().unwrap() = s;
    }
    fn pending_wakes_left(&self, _user: &str) -> u32 {
        self.wakes_left.load(Ordering::SeqCst)
    }
}

impl AssistantPort for FakeB {
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.kb.clone()
    }
    fn pending_ledger(&self) -> Arc<dyn iyakki_agent_api::pending::AgentPending> {
        self.ledger.clone()
    }
    fn pendingplus_settings_rows(&self, s: &iyakki_shared::pending::Settings, wakes_left: u32) -> Vec<iyakki_shared::pendingplus::Row> {
        iyakki_agent::pendingplus::settings_rows(s, wakes_left)
    }
    fn pendingplus_apply_setting(&self, s: &mut iyakki_shared::pending::Settings, key: &str, value: &str) -> Result<(), String> {
        iyakki_agent::pendingplus::apply_setting(s, key, value)
    }
}

/// The model-host port: the hosts it kept or removed, the kubectl it was asked, and the key of a host.
#[derive(Default)]
pub(crate) struct FakeModelHosts {
    pub removed: Mutex<Vec<String>>,
    pub kubectl: Mutex<Vec<Vec<String>>>,
    pub keys: Mutex<HashMap<String, String>>,
    pub remove_refusal: Mutex<Option<String>>,
}

impl ModelHostOps for FakeModelHosts {
    fn add(&self, n: &iyakki_shared::modelhost::NewHost, _report: &iyakki_shared::modelhost::TestReport) -> Result<iyakki_shared::modelhost::Host, String> {
        Err(format!("the fake keeps nothing ({})", n.nickname))
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        if let Some(e) = self.remove_refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.removed.lock().unwrap().push(id.to_string());
        Ok(())
    }
}

impl iyakki_shared::modelhost::Kube for FakeModelHosts {
    fn kubectl(&self, args: &[&str]) -> iyakki_shared::wsexec::Out {
        self.kubectl.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
        iyakki_shared::wsexec::Out { code: Some(0), stdout: "created".into(), ..Default::default() }
    }
}

impl ModelHostPort for FakeB {
    fn modelhost_ops(&self) -> Arc<dyn ModelHostOps> {
        self.mhost.clone()
    }
    fn modelhost_kube(&self) -> Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync> {
        self.mhost.clone()
    }
    fn modelhost_key(&self, host_id: &str) -> Option<String> {
        self.mhost.keys.lock().unwrap().get(host_id).cloned()
    }
}

/// The agents' status cache (scripted), the install plan and the slow requests.
#[derive(Default)]
pub(crate) struct FakeAgents {
    pub status: Mutex<HashMap<String, iyakki_extagentx::Status>>,
    pub models: Mutex<HashMap<String, Vec<iyakki_extagentx::ModelInfo>>>,
    pub refreshed: Mutex<u32>,
    pub installed: Mutex<Vec<String>>,
    pub install_plan: Mutex<Option<String>>,
    pub install_refusal: Mutex<Option<String>>,
    pub tests: Mutex<Vec<(String, Option<String>)>>,
    pub test_answer: Mutex<Option<Result<String, String>>>,
    pub login: Mutex<Option<crate::agent_login::State>>,
}

impl iyakki_agent_api::ExtStatusApi for FakeAgents {
    fn set_models(&self, id: &str, list: Vec<iyakki_extagentx::ModelInfo>) {
        self.models.lock().unwrap().insert(id.into(), list);
    }
    fn set_models_at(&self, id: &str, list: Vec<iyakki_extagentx::ModelInfo>, _at: u64) {
        self.set_models(id, list);
    }
    fn cli_models(&self, id: &str) -> Option<Vec<iyakki_extagentx::ModelInfo>> {
        self.models.lock().unwrap().get(id).cloned()
    }
    fn get(&self, id: &str) -> Option<iyakki_extagentx::Status> {
        self.status.lock().unwrap().get(id).cloned()
    }
    fn set(&self, id: &str, s: iyakki_extagentx::Status) {
        self.status.lock().unwrap().insert(id.into(), s);
    }
    fn busy(&self) -> bool {
        false
    }
    fn invalidate(&self) {}
    fn refresh(&self, _cfg: iyakki_shared::extagentcfg::AgentsCfg) {
        *self.refreshed.lock().unwrap() += 1;
    }
    fn reset(&self, _cfg: iyakki_shared::extagentcfg::AgentsCfg) {}
    fn recheck(&self, _cfg: iyakki_shared::extagentcfg::AgentsCfg, _id: &str) {}
    fn recheck_after(&self, _cfg: iyakki_shared::extagentcfg::AgentsCfg, _id: &str, _min: std::time::Duration) {}
    fn refresh_one(&self, _cfg: iyakki_shared::extagentcfg::AgentsCfg, _id: &str) {}
}

impl AgentTester for FakeAgents {
    fn test(&self, agent: Arc<dyn iyakki_shared::extagent::ExternalAgent>, key_ref: Option<String>) -> Result<String, String> {
        self.tests.lock().unwrap().push((agent.id(), key_ref));
        self.test_answer.lock().unwrap().clone().unwrap_or_else(|| Ok(format!("{} answered OK", agent.label())))
    }
    fn check_models(&self, provider: &str) -> Result<String, String> {
        Ok(format!("{provider}: 3 models usable"))
    }
}

impl AgentsPort for FakeB {
    fn agents_cfg(&self) -> iyakki_shared::extagentcfg::AgentsCfg {
        self.settings_get().agents
    }
    fn ext_status(&self) -> Arc<dyn iyakki_agent_api::ExtStatusApi> {
        self.agents.clone()
    }
    fn cli_install_plan(&self, _cli: &str) -> Option<String> {
        self.agents.install_plan.lock().unwrap().clone()
    }
    fn cli_install(&self, cli: &str) -> Result<(), String> {
        if let Some(e) = self.agents.install_refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.agents.installed.lock().unwrap().push(cli.to_string());
        Ok(())
    }
    fn agent_tester(&self) -> Arc<dyn AgentTester> {
        self.agents.clone()
    }
    fn agent_login_state(&self, _agent: &str, _installed: bool) -> crate::agent_login::State {
        self.agents.login.lock().unwrap().clone().unwrap_or(crate::agent_login::State::SignedOut)
    }
}

struct FakeBackendTester(Arc<Mutex<Vec<String>>>, Arc<Mutex<Result<String, String>>>);
impl RagBackendTester for FakeBackendTester {
    fn test(&self, backend: iyakki_shared::rag_ext::ExternalRag) -> Result<String, String> {
        self.0.lock().unwrap().push(backend.id().to_string());
        self.1.lock().unwrap().clone()
    }
}

impl RagPort for FakeB {
    fn kb(&self) -> Arc<dyn iyakki_agent_api::member_stores::KnowledgeBaseApi> {
        Arc::new(self.kb.clone())
    }
    fn rag_docs_dir(&self) -> PathBuf {
        self.dir.join("docs")
    }
    fn kb_is_reindexing(&self) -> bool {
        self.reindexing.load(Ordering::SeqCst)
    }
    fn rag_backend_tester(&self) -> Arc<dyn RagBackendTester> {
        Arc::new(FakeBackendTester(self.backend_tests.clone(), self.backend_answer.clone()))
    }
}

/// The services calls of the Lemonade pages: the model lists, what was asked (by name) and the next refusal.
#[derive(Default)]
pub(crate) struct FakeLemonade {
    pub models: Mutex<HashMap<String, Vec<iyakki_shared::lemonade::Model>>>,
    pub calls: Mutex<Vec<String>>,
    pub refusal: Mutex<Option<String>>,
    pub allowed_servers: Mutex<Vec<String>>,
    pub reindex_started: AtomicBool,
}

impl FakeLemonade {
    pub(crate) fn called(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn note(&self, what: String) -> Result<(), String> {
        if let Some(e) = self.refusal.lock().unwrap().take() {
            return Err(e);
        }
        self.calls.lock().unwrap().push(what);
        Ok(())
    }
}

impl LemonadeOps for FakeLemonade {
    fn lemonade_cached(&self, provider: &str) -> Vec<iyakki_shared::lemonade::Model> {
        self.models.lock().unwrap().get(provider).cloned().unwrap_or_default()
    }
    fn refresh_lemonade(&self, provider: &str) -> Result<Vec<iyakki_shared::lemonade::Model>, String> {
        self.note(format!("refresh {provider}"))?;
        Ok(self.lemonade_cached(provider))
    }
    fn detect_local_servers(&self) -> Vec<String> {
        vec!["Ollama on 11434".into()]
    }
    fn collection_review(&self, _provider: &str, collection: &str) -> Result<iyakki_shared::omni::Review, String> {
        Err(format!("{collection} is not scripted"))
    }
    fn parts_undo_available(&self) -> Option<(String, String)> {
        None
    }
    fn undo_all_parts(&self, _store: &iyakki_shared::story::StoryStore) -> Result<String, String> {
        Err("no story store in the fake".into())
    }
    fn use_lemonade_voice(&self, provider: &str, model: &str, voice: Option<&str>) -> Result<String, String> {
        self.note(format!("voice {provider}/{model} {}", voice.unwrap_or("")))?;
        Ok("Voice kept.".into())
    }
    fn use_lemonade_stt(&self, provider: &str, model: &str) -> Result<String, String> {
        self.note(format!("stt {provider}/{model}"))?;
        Ok("Speech-to-text kept.".into())
    }
    fn speech_server_allowed(&self, url: &str) -> bool {
        self.allowed_servers.lock().unwrap().iter().any(|u| u == url)
    }
    fn allow_speech_server(&self, url: &str) -> Result<(), String> {
        self.note(format!("allow {url}"))?;
        self.allowed_servers.lock().unwrap().push(url.to_string());
        Ok(())
    }
    fn reindex_stale_in_background(&self) -> bool {
        self.reindex_started.store(true, Ordering::SeqCst);
        true
    }
    fn lemonade_load(&self, provider: &str, model: &str, load: bool) -> Result<String, String> {
        self.note(format!("{} {provider}/{model}", if load { "load" } else { "unload" }))?;
        Ok(format!("{model} {}", if load { "loaded" } else { "unloaded" }))
    }
    fn pull_model_for(&self, user: &str, provider: &str, model: &str) -> Result<(), String> {
        self.note(format!("pull {user} {provider}/{model}"))
    }
    fn install_missing_parts(&self, provider: &str, collection: &str) -> Result<Vec<String>, String> {
        self.note(format!("install parts {provider}/{collection}"))?;
        Ok(vec!["part-a".into()])
    }
    fn use_all_parts(&self, _provider: &str, _collection: &str, _store: &iyakki_shared::story::StoryStore) -> Result<String, String> {
        Err("no story store in the fake".into())
    }
    fn pull_status(&self) -> Vec<iyakki_shared::services::PullStatus> {
        Vec::new()
    }
    fn lemonade_compute_backend(&self, _user: &str) -> Arc<dyn iyakki_agent_api::ComputeBackend> {
        Arc::new(FakeCompute(Mutex::new(iyakki_shared::gpu_steps::Compute::Gpu)))
    }
    fn vision_candidates(&self) -> Vec<iyakki_shared::visionmodel::Cand> {
        Vec::new()
    }
    fn vision_model(&self, _own: Option<&iyakki_shared::settings::ModelChoice>) -> Result<iyakki_shared::visionmodel::Pick, iyakki_shared::visionmodel::VisionError> {
        Err(iyakki_shared::visionmodel::VisionError::NoneFound("no vision model in the fake".into()))
    }
}

struct FakeCompute(Mutex<iyakki_shared::gpu_steps::Compute>);
impl iyakki_agent_api::ComputeBackend for FakeCompute {
    fn current(&self) -> iyakki_shared::gpu_steps::Compute {
        *self.0.lock().unwrap()
    }
    fn set(&self, c: iyakki_shared::gpu_steps::Compute) -> Result<String, String> {
        *self.0.lock().unwrap() = c;
        Ok(format!("now {}", c.word()))
    }
}

/// The detection worker: the settings store and the vault of the fake, a short HTTP client that refuses.
struct FakeDetect {
    store: Arc<Mutex<SettingsStore>>,
}
impl DetectOps for FakeDetect {
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        struct Refuse;
        impl iyakki_proxy::Upstream for Refuse {
            fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("no network in the fake".into())
            }
        }
        Arc::new(Refuse)
    }
    fn secret_system_set(&self, _name: &str, _value: &str) -> Result<(), VaultError> {
        Ok(())
    }
    fn provider_upsert(&self, cfg: iyakki_shared::providers::ProviderCfg) -> Result<(), String> {
        SettingsRef::of_store(&self.store).lock().unwrap().upsert_provider(cfg).map_err(|e| e.to_string())
    }
}

/// The model clients: what a check, a test and one chat turn answer.
#[derive(Default)]
pub(crate) struct FakeAi {
    pub check_refusal: Mutex<Option<String>>,
    pub chat_answer: Mutex<Option<Result<String, String>>>,
    pub chats: Mutex<Vec<(String, String)>>,
    pub tts: Arc<FakeHostTts>,
}

impl iyakki_agent_api::AiRead for FakeAi {
    fn host_tts(&self) -> Arc<dyn iyakki_agent_api::HostTtsApi> {
        self.tts.clone()
    }
    fn ext_session(&self, _chat: &str, _agent: &str) -> Option<String> {
        None
    }
    fn ext_session_set(&self, _chat: &str, _agent: &str, _id: &str) {}
    fn usage_record(&self, _user: &str, _provider: &str, _model: &str, _input: u64, _output: u64) {}
    fn provider_check(&self, provider: &str) -> Result<(), String> {
        match self.check_refusal.lock().unwrap().clone() {
            Some(e) => Err(format!("{provider}: {e}")),
            None => Ok(()),
        }
    }
    fn provider_test(&self, provider: &str) -> Result<iyakki_shared::providers::TestReport, String> {
        Err(format!("{provider}: the fake does not talk to a server"))
    }
    fn provider_chat(&self, provider: &str, model: &str, _history: &[iyakki_shared::chat::Message]) -> Result<String, String> {
        self.chats.lock().unwrap().push((provider.to_string(), model.to_string()));
        self.chat_answer.lock().unwrap().clone().unwrap_or_else(|| Ok("OK".to_string()))
    }
    fn image_models(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn picture_choices(&self, _found: &[(String, String)]) -> Vec<iyakki_shared::storyillus::Cand> {
        Vec::new()
    }
    fn unavailable_models(&self, _provider: &str) -> Vec<String> {
        Vec::new()
    }
}

/// The speech engines of the computer: nothing is found, and what was asked is counted.
#[derive(Default)]
pub(crate) struct FakeHostTts {
    pub enabled: AtomicBool,
    pub scanning: AtomicBool,
    pub rechecked: std::sync::atomic::AtomicUsize,
    pub ensured: std::sync::atomic::AtomicUsize,
}

impl iyakki_agent_api::HostTtsApi for FakeHostTts {
    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    fn configure(&self, _env: iyakki_shared::hostengines::Env) {}
    fn set_opener(&self, _opener: Option<Vec<String>>) {}
    fn ensure(&self) {
        self.ensured.fetch_add(1, Ordering::SeqCst);
    }
    fn recheck(&self) {
        self.rechecked.fetch_add(1, Ordering::SeqCst);
        self.scanning.store(true, Ordering::SeqCst);
    }
    fn report(&self) -> Option<Arc<iyakki_shared::hostengines::Report>> {
        None
    }
    fn scanning(&self) -> bool {
        self.scanning.load(Ordering::SeqCst)
    }
    fn scan_now(&self) -> Arc<iyakki_shared::hostengines::Report> {
        Arc::new(iyakki_shared::hostengines::Report::empty())
    }
    fn helper_open(&self, _hv: Arc<iyakki_shared::hostvoice::HostVoice>, _opener: Option<&[String]>) -> Result<String, String> {
        Err("no browser here".into())
    }
    fn helper_state(&self) -> Option<iyakki_shared::voicehelper::HelperState> {
        None
    }
    fn helper_stop(&self) {}
    fn auto_engine(&self, _lang: Option<&str>) -> Option<iyakki_shared::voice::SpeechEngine> {
        None
    }
}

impl AgentPort for FakeB {
    fn agent(&self) -> &dyn iyakki_agent_api::AgentApi {
        &*self.agent_api
    }
    fn agent_arc(&self) -> Arc<dyn iyakki_agent_api::AgentApi> {
        self.agent_api.clone()
    }
    fn ai(&self) -> Arc<dyn iyakki_agent_api::AiRead> {
        self.ai.clone()
    }
}

impl LemonadePort for FakeB {
    fn lemonade(&self) -> Arc<dyn LemonadeOps> {
        self.lem.clone()
    }
    fn detect_ops(&self) -> Arc<dyn DetectOps> {
        Arc::new(FakeDetect { store: self.store.clone() })
    }
    fn story_store(&self) -> Option<iyakki_shared::story::StoryStore> {
        None
    }
    fn plain_error(&self) -> Arc<dyn Fn(&str, &str) -> String + Send + Sync> {
        Arc::new(|provider, raw| format!("{provider} says: {raw}"))
    }
}

/// The voice services: the settings of the voice (from the fake's settings file), what was asked, and the next refusal.
pub(crate) struct FakeVoice {
    store: Mutex<Option<Arc<Mutex<SettingsStore>>>>,
    pub calls: Mutex<Vec<String>>,
    pub refusal: Mutex<Option<String>>,
}

impl FakeVoice {
    fn settings(&self) -> iyakki_shared::voice::VoiceSettings {
        let store = self.store.lock().unwrap().clone().expect("the fake's store");
        let g = SettingsRef::of_store(&store).lock().unwrap().get().voice.clone();
        g
    }
    fn change(&self, f: impl FnOnce(&mut iyakki_shared::voice::VoiceSettings)) -> Result<(), String> {
        let store = self.store.lock().unwrap().clone().expect("the fake's store");
        let mut f = Some(f);
        SettingsRef::of_store(&store).lock().unwrap().update(|s| {
            if let Some(f) = f.take() {
                f(&mut s.voice)
            }
        }).map_err(|e| e.to_string())
    }
    fn refused(&self) -> Result<(), String> {
        match self.refusal.lock().unwrap().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl VoiceOps for FakeVoice {
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.settings()
    }
    fn transcribe_samples(&self, pcm: &[i16]) -> Result<String, String> {
        self.refused()?;
        Ok(format!("{} samples heard", pcm.len()))
    }
    fn synthesize_with_opts(&self, _text: &str, _opts: &iyakki_shared::voice::SpeakOpts) -> Result<Vec<u8>, String> {
        self.refused()?;
        Ok(Vec::new())
    }
    fn speech_aligner(&self) -> Option<iyakki_shared::voice::SpeechEngine> {
        None
    }
    fn wake_set_sensitivity(&self, sens: u8) -> Result<(), String> {
        self.refused()?;
        self.calls.lock().unwrap().push(format!("sensitivity {sens}"));
        self.change(|v| v.my_wake_sens = sens)
    }
    fn wake_set_opt_in(&self, on: bool) -> Result<bool, String> {
        self.refused()?;
        self.calls.lock().unwrap().push(format!("opt-in {on}"));
        self.change(|v| v.my_wake = on)?;
        Ok(on)
    }
    fn wake_no_model_text(&self) -> Option<String> {
        None
    }
    fn wake_stats_line(&self) -> String {
        "no wake stats in the fake".into()
    }
}

impl VoicePort for FakeB {
    fn voice_ops(&self) -> Arc<dyn VoiceOps> {
        self.voice.clone()
    }
    fn may_use_voice(&self) -> bool {
        self.voice_allowed.load(Ordering::SeqCst)
    }
    fn dict_fix_set(&self, on: bool) -> Result<(), String> {
        self.voice.change(|v| v.fix_wording = on)
    }
}
