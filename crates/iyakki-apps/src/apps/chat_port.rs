//! What the CHAT app asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12; alpha.10 step P4).
//!
//! The chat files name no `AppCx`, no session and no audit log for the things below: they ask a [`ChatPort`] who is signed in, what that person's
//! policy allows, where the person's data folder is, to write an audit line, the core, and the settings. The real port ([`CxChat`]) is the one
//! place that reads these from the shell's context; the fake ([`FakeChatPort`]) holds them in memory with a manual clock, so a chat tool is tested
//! without a shell and without sleeping.
//!
//! Growing it: a new need of a chat file becomes a method here (the real port answers from the context, the fake from memory), never a new
//! `cx.` in a chat file. What is left to move is listed in docs/development/CHAT-IDE-PORTS-PLAN.md.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use iyakki_agent_api::{ExtStatusApi, SecretRequestsApi, SessionApi, SettingsRef, VaultRead};
use iyakki_shared::extagentcfg::AgentsCfg;
use iyakki_shared::voice::VoiceSettings;
use iyakki_core_api::Core;
use iyakki_sec::policy::Policy;

use super::AppCx;

/// A line sink for the audit log: `(action, subject)`, already bound to the signed-in person.
pub type AuditSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// A port that may be moved into a spawned run or a tool handler (the 40 places that used to clone the shell's context).
pub type ChatPortRef = Arc<dyn ChatPort>;

/// The agent's pure helper functions (the generated facade), kept apart from the port: a file that needs only helpers asks this trait, and a fake
/// of it is the facade's own fake. (CHAT-IDE-PORTS-PLAN.md step 2.)
pub trait AgentHelpers {
    fn agent(&self) -> Arc<dyn iyakki_agent_api::AgentApi>;
}

impl AgentHelpers for AppCx {
    fn agent(&self) -> Arc<dyn iyakki_agent_api::AgentApi> {
        self.agent.clone()
    }
}

/// The person and the data folder, the two things the browser's small stores read from a context. The shell's context and every port answer it, so a
/// store function that asks `&dyn UserData` is called with either one.
pub trait UserData: AgentHelpers {
    fn acting_user(&self) -> &str;
    fn data_root(&self) -> &Path;
    fn acting_policy(&self) -> &Policy;
    /// One audit line for the signed-in person.
    fn record_audit(&self, action: &str, subject: &str);
    fn core_handle(&self) -> Arc<dyn Core>;
    fn vault_handle(&self) -> Arc<dyn VaultRead>;
    /// The web access of this person (their own proxy/policy when they have one).
    fn web_for_user(&self) -> Arc<dyn iyakki_web::Fetch>;
}

impl UserData for AppCx {
    fn acting_user(&self) -> &str {
        &self.session.user
    }
    fn data_root(&self) -> &Path {
        self.data_dir()
    }
    fn acting_policy(&self) -> &Policy {
        &self.session.policy
    }
    fn record_audit(&self, action: &str, subject: &str) {
        self.audit.record(&self.session.user, action, subject);
    }
    fn core_handle(&self) -> Arc<dyn Core> {
        self.core.clone()
    }
    fn vault_handle(&self) -> Arc<dyn VaultRead> {
        self.vault()
    }
    fn web_for_user(&self) -> Arc<dyn iyakki_web::Fetch> {
        self.web.for_user(&self.session.user).unwrap_or_else(|| self.web.clone())
    }
}

pub trait ChatPort: AgentHelpers + UserData + Send + Sync {
    /// A copy of this port that can be moved to another thread (the real one owns a cheap clone of the context; the fake shares its audit list).
    fn shared(&self) -> ChatPortRef;
    /// The signed-in person.
    fn user(&self) -> &str;
    /// What that person's policy allows.
    fn policy(&self) -> &Policy;
    /// The person's data folder of this install.
    fn data_dir(&self) -> &Path;
    /// One line in the audit log, for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// A sink that writes audit lines for the signed-in person (for code that runs on another thread).
    fn audit_sink(&self) -> AuditSink;
    /// The newest audit lines (at most `n`).
    fn audit_recent(&self, n: usize) -> Vec<String>;
    /// The core (devices, files, browser, flows, app events), the way every tile reaches the machine.
    fn core(&self) -> Arc<dyn Core>;
    /// The settings of the install.
    fn settings(&self) -> SettingsRef;
    /// Seconds since the epoch, from the shell's clock (a manual clock in the fake).
    fn now_secs(&self) -> u64;
    // ---- the person's chats and what the chat reads about the install (slice 2) ----
    /// The store of the person's saved chats.
    fn sessions(&self) -> Arc<dyn SessionApi>;
    /// The working folder of one chat (its files), created when missing.
    fn session_workdir(&self, session_id: &str) -> Result<PathBuf, String>;
    /// How many files one chat has in its working folder.
    fn session_files(&self, session_id: &str) -> usize;
    /// The person's vault (names and values are read and written only through it).
    fn vault(&self) -> Arc<dyn VaultRead>;
    /// The voice settings of the install.
    fn voice_settings(&self) -> VoiceSettings;
    /// The external agents the person may choose, as configured.
    fn agents_cfg(&self) -> AgentsCfg;
    /// What the install knows about each external agent (installed, ready, models).
    fn ext_status(&self) -> Arc<dyn ExtStatusApi>;
    /// The requests for a secret the assistant has made and the person has not answered.
    fn secret_requests(&self) -> Arc<dyn SecretRequestsApi>;
    /// The knowledge base as the source of the workspace history.
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource>;
}

/// The real port, over the shell's context. It is a cheap view (copy it freely); the reads below live as long as the context, not as long as the view,
/// so `let user = CxChat(cx).user();` is fine.
#[derive(Clone, Copy)]
pub struct CxChat<'a>(pub &'a AppCx);

/// The real port that owns its context (cheap: every handle in it is shared). `CxChat::shared` makes one; it is what a spawned run holds.
#[derive(Clone)]
pub struct OwnedChat(pub AppCx);

macro_rules! delegate_chat {
    ($t:ty, $s:ident => $v:expr) => {
        impl AgentHelpers for $t {
            fn agent(&$s) -> Arc<dyn iyakki_agent_api::AgentApi> {
                $v.0.agent.clone()
            }
        }
        impl UserData for $t {
            fn acting_user(&$s) -> &str {
                &$v.0.session.user
            }
            fn data_root(&$s) -> &Path {
                $v.0.data_dir()
            }
            fn acting_policy(&$s) -> &Policy {
                &$v.0.session.policy
            }
            fn record_audit(&$s, action: &str, subject: &str) {
                $v.0.audit.record(&$v.0.session.user, action, subject);
            }
            fn core_handle(&$s) -> Arc<dyn Core> {
                $v.0.core.clone()
            }
            fn vault_handle(&$s) -> Arc<dyn VaultRead> {
                $v.0.vault()
            }
            fn web_for_user(&$s) -> Arc<dyn iyakki_web::Fetch> {
                <AppCx as UserData>::web_for_user(&$v.0)
            }
        }
    };
}
delegate_chat!(CxChat<'_>, self => self);
delegate_chat!(OwnedChat, self => self);

impl ChatPort for OwnedChat {
    fn shared(&self) -> ChatPortRef {
        Arc::new(self.clone())
    }
    fn user(&self) -> &str {
        CxChat(&self.0).user()
    }
    fn policy(&self) -> &Policy {
        CxChat(&self.0).policy()
    }
    fn data_dir(&self) -> &Path {
        CxChat(&self.0).data_dir()
    }
    fn audit(&self, action: &str, subject: &str) {
        CxChat(&self.0).audit(action, subject)
    }
    fn audit_sink(&self) -> AuditSink {
        CxChat(&self.0).audit_sink()
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        CxChat(&self.0).audit_recent(n)
    }
    fn core(&self) -> Arc<dyn Core> {
        CxChat(&self.0).core()
    }
    fn settings(&self) -> SettingsRef {
        CxChat(&self.0).settings()
    }
    fn now_secs(&self) -> u64 {
        CxChat(&self.0).now_secs()
    }
    fn sessions(&self) -> Arc<dyn SessionApi> {
        CxChat(&self.0).sessions()
    }
    fn session_workdir(&self, session_id: &str) -> Result<PathBuf, String> {
        CxChat(&self.0).session_workdir(session_id)
    }
    fn session_files(&self, session_id: &str) -> usize {
        CxChat(&self.0).session_files(session_id)
    }
    fn vault(&self) -> Arc<dyn VaultRead> {
        CxChat(&self.0).vault()
    }
    fn voice_settings(&self) -> VoiceSettings {
        CxChat(&self.0).voice_settings()
    }
    fn agents_cfg(&self) -> AgentsCfg {
        CxChat(&self.0).agents_cfg()
    }
    fn ext_status(&self) -> Arc<dyn ExtStatusApi> {
        CxChat(&self.0).ext_status()
    }
    fn secret_requests(&self) -> Arc<dyn SecretRequestsApi> {
        CxChat(&self.0).secret_requests()
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.0.kb_source()
    }
}

impl<'a> CxChat<'a> {
    pub fn user(&self) -> &'a str {
        &self.0.session.user
    }
    pub fn policy(&self) -> &'a Policy {
        &self.0.session.policy
    }
    pub fn data_dir(&self) -> &'a Path {
        self.0.data_dir()
    }
    pub fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    pub fn audit_sink(&self) -> AuditSink {
        let (audit, user) = (self.0.audit.clone(), self.0.session.user.clone());
        Arc::new(move |a, d| audit.record(&user, a, d))
    }
    pub fn audit_recent(&self, n: usize) -> Vec<String> {
        self.0.audit.recent(n)
    }
    pub fn core(&self) -> Arc<dyn Core> {
        self.0.core.clone()
    }
    pub fn settings(&self) -> SettingsRef {
        self.0.settings()
    }
    pub fn now_secs(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
    pub fn sessions(&self) -> Arc<dyn SessionApi> {
        self.0.sessions()
    }
    pub fn session_workdir(&self, session_id: &str) -> Result<PathBuf, String> {
        self.0.session_workdir(&self.0.session.user, session_id)
    }
    pub fn session_files(&self, session_id: &str) -> usize {
        self.0.session_files(&self.0.session.user, session_id)
    }
    pub fn vault(&self) -> Arc<dyn VaultRead> {
        self.0.vault()
    }
    pub fn voice_settings(&self) -> VoiceSettings {
        self.0.voice_settings()
    }
    pub fn agents_cfg(&self) -> AgentsCfg {
        self.0.agents_cfg()
    }
    pub fn ext_status(&self) -> Arc<dyn ExtStatusApi> {
        self.0.ext_status()
    }
    pub fn secret_requests(&self) -> Arc<dyn SecretRequestsApi> {
        self.0.secret_requests()
    }
}

impl ChatPort for CxChat<'_> {
    fn shared(&self) -> ChatPortRef {
        Arc::new(OwnedChat(self.0.clone()))
    }
    fn user(&self) -> &str {
        CxChat::user(self)
    }
    fn policy(&self) -> &Policy {
        CxChat::policy(self)
    }
    fn data_dir(&self) -> &Path {
        CxChat::data_dir(self)
    }
    fn audit(&self, action: &str, subject: &str) {
        CxChat::audit(self, action, subject)
    }
    fn audit_sink(&self) -> AuditSink {
        CxChat::audit_sink(self)
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        CxChat::audit_recent(self, n)
    }
    fn core(&self) -> Arc<dyn Core> {
        CxChat::core(self)
    }
    fn settings(&self) -> SettingsRef {
        CxChat::settings(self)
    }
    fn now_secs(&self) -> u64 {
        CxChat::now_secs(self)
    }
    fn sessions(&self) -> Arc<dyn SessionApi> {
        CxChat::sessions(self)
    }
    fn session_workdir(&self, session_id: &str) -> Result<PathBuf, String> {
        CxChat::session_workdir(self, session_id)
    }
    fn session_files(&self, session_id: &str) -> usize {
        CxChat::session_files(self, session_id)
    }
    fn vault(&self) -> Arc<dyn VaultRead> {
        CxChat::vault(self)
    }
    fn voice_settings(&self) -> VoiceSettings {
        CxChat::voice_settings(self)
    }
    fn agents_cfg(&self) -> AgentsCfg {
        CxChat::agents_cfg(self)
    }
    fn ext_status(&self) -> Arc<dyn ExtStatusApi> {
        CxChat::ext_status(self)
    }
    fn secret_requests(&self) -> Arc<dyn SecretRequestsApi> {
        CxChat::secret_requests(self)
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.0.kb_source()
    }
}

/// A port in memory for tests: fixed person and policy, an audit list, a manual clock, the core and settings it is given. The facets of slice 2 are
/// given with the `with_` builders; asking for one that was not given stops the test with a sentence saying which (a fake never falls back to the
/// real thing).
#[cfg(test)]
pub struct FakeChatPort {
    pub user: String,
    pub policy: Policy,
    pub dir: PathBuf,
    pub core: Arc<dyn Core>,
    pub settings: SettingsRef,
    pub voice: VoiceSettings,
    pub agents: AgentsCfg,
    sessions: Option<Arc<dyn SessionApi>>,
    vault: Option<Arc<dyn VaultRead>>,
    ext: Option<Arc<dyn ExtStatusApi>>,
    secrets: Option<Arc<dyn SecretRequestsApi>>,
    audit: Arc<Mutex<Vec<(String, String, String)>>>,
    clock: Arc<AtomicU64>,
    agent: Option<Arc<dyn iyakki_agent_api::AgentApi>>,
    web: Option<Arc<dyn iyakki_web::Fetch>>,
    kb: Option<Arc<dyn iyakki_memx::kb::KbSource>>,
}

#[cfg(test)]
impl Clone for FakeChatPort {
    /// A copy shares the audit list and the manual clock, so what a spawned run audits shows in the test's own handle.
    fn clone(&self) -> FakeChatPort {
        FakeChatPort { user: self.user.clone(), policy: self.policy.clone(), dir: self.dir.clone(), core: self.core.clone(), settings: self.settings.clone(), voice: self.voice.clone(), agents: self.agents.clone(), sessions: self.sessions.clone(), vault: self.vault.clone(), ext: self.ext.clone(), secrets: self.secrets.clone(), audit: self.audit.clone(), clock: self.clock.clone(), agent: self.agent.clone(), web: self.web.clone(), kb: self.kb.clone() }
    }
}

#[cfg(test)]
impl AgentHelpers for FakeChatPort {
    fn agent(&self) -> Arc<dyn iyakki_agent_api::AgentApi> {
        self.agent.clone().expect("FakeChatPort: give it the agent facade with with_agent")
    }
}

#[cfg(test)]
impl UserData for FakeChatPort {
    fn acting_user(&self) -> &str {
        &self.user
    }
    fn data_root(&self) -> &Path {
        &self.dir
    }
    fn acting_policy(&self) -> &Policy {
        &self.policy
    }
    fn record_audit(&self, action: &str, subject: &str) {
        ChatPort::audit(self, action, subject)
    }
    fn core_handle(&self) -> Arc<dyn Core> {
        self.core.clone()
    }
    fn vault_handle(&self) -> Arc<dyn VaultRead> {
        ChatPort::vault(self)
    }
    fn web_for_user(&self) -> Arc<dyn iyakki_web::Fetch> {
        self.web.clone().expect("FakeChatPort: give it web access with with_web")
    }
}

#[cfg(test)]
impl FakeChatPort {
    pub fn new(user: &str, policy: Policy, dir: &Path, core: Arc<dyn Core>, settings: SettingsRef) -> FakeChatPort {
        FakeChatPort { user: user.into(), policy, dir: dir.into(), core, settings, voice: VoiceSettings::default(), agents: AgentsCfg::default(), sessions: None, vault: None, ext: None, secrets: None, audit: Arc::new(Mutex::new(Vec::new())), clock: Arc::new(AtomicU64::new(1_700_000_000)), agent: None, web: None, kb: None }
    }
    pub fn with_sessions(mut self, s: Arc<dyn SessionApi>) -> FakeChatPort {
        self.sessions = Some(s);
        self
    }
    pub fn with_vault(mut self, v: Arc<dyn VaultRead>) -> FakeChatPort {
        self.vault = Some(v);
        self
    }
    pub fn with_ext_status(mut self, e: Arc<dyn ExtStatusApi>) -> FakeChatPort {
        self.ext = Some(e);
        self
    }
    pub fn with_secret_requests(mut self, r: Arc<dyn SecretRequestsApi>) -> FakeChatPort {
        self.secrets = Some(r);
        self
    }
    pub fn with_web(mut self, w: Arc<dyn iyakki_web::Fetch>) -> FakeChatPort {
        self.web = Some(w);
        self
    }
    pub fn with_kb(mut self, k: Arc<dyn iyakki_memx::kb::KbSource>) -> FakeChatPort {
        self.kb = Some(k);
        self
    }
    pub fn with_agent(mut self, a: Arc<dyn iyakki_agent_api::AgentApi>) -> FakeChatPort {
        self.agent = Some(a);
        self
    }
    /// Move the manual clock forward.
    pub fn advance(&self, secs: u64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
    /// The audit lines as `(person, action, subject)`.
    pub fn audited(&self) -> Vec<(String, String, String)> {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
impl ChatPort for FakeChatPort {
    fn shared(&self) -> ChatPortRef {
        Arc::new(self.clone())
    }
    fn user(&self) -> &str {
        &self.user
    }
    fn policy(&self) -> &Policy {
        &self.policy
    }
    fn data_dir(&self) -> &Path {
        &self.dir
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).push((self.user.clone(), action.into(), subject.into()));
    }
    fn audit_sink(&self) -> AuditSink {
        let (log, user) = (self.audit.clone(), self.user.clone());
        Arc::new(move |a, d| log.lock().unwrap_or_else(|e| e.into_inner()).push((user.clone(), a.into(), d.into())))
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        let a = self.audit.lock().unwrap_or_else(|e| e.into_inner());
        let from = a.len().saturating_sub(n);
        a[from..].iter().map(|(u, act, s)| format!("{u} {act} {s}")).collect()
    }
    fn core(&self) -> Arc<dyn Core> {
        self.core.clone()
    }
    fn settings(&self) -> SettingsRef {
        self.settings.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn sessions(&self) -> Arc<dyn SessionApi> {
        self.sessions.clone().expect("FakeChatPort: give it sessions with with_sessions")
    }
    fn session_workdir(&self, session_id: &str) -> Result<PathBuf, String> {
        let d = self.dir.join("sessions").join(session_id);
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        Ok(d)
    }
    fn session_files(&self, session_id: &str) -> usize {
        std::fs::read_dir(self.dir.join("sessions").join(session_id)).map(|d| d.flatten().count()).unwrap_or(0)
    }
    fn vault(&self) -> Arc<dyn VaultRead> {
        self.vault.clone().expect("FakeChatPort: give it a vault with with_vault")
    }
    fn voice_settings(&self) -> VoiceSettings {
        self.voice.clone()
    }
    fn agents_cfg(&self) -> AgentsCfg {
        self.agents.clone()
    }
    fn ext_status(&self) -> Arc<dyn ExtStatusApi> {
        self.ext.clone().expect("FakeChatPort: give it an external-agent status with with_ext_status")
    }
    fn secret_requests(&self) -> Arc<dyn SecretRequestsApi> {
        self.secrets.clone().expect("FakeChatPort: give it secret requests with with_secret_requests")
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.kb.clone().expect("FakeChatPort: give it a knowledge base with with_kb")
    }
}

#[cfg(test)]
#[path = "chat_port_tests.rs"]
mod tests;
