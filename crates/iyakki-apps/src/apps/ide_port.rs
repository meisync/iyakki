//! What the IDE app asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12; alpha.10 step P4).
//!
//! The IDE files name no `AppCx` for the things below: they ask an [`IdePort`] who is signed in, what that person's policy and role allow, where
//! the data folder is, to write an audit line (or to be handed a sink that does), the core, and the stores and facets the IDE panes read (the
//! browser state, the Git kit, the CLI tools, the store, the task manager, the knowledge base). The real port ([`CxIde`]) is the one place that reads
//! these from the shell's context; the fake ([`FakeIdePort`]) keeps them in memory with a manual clock, so an IDE pane is tested without a shell.
//!
//! Growing it: a new need of an IDE file becomes a method here, never a new `cx.` in an IDE file. What is left to move is listed in
//! docs/development/CHAT-IDE-PORTS-PLAN.md.

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use iyakki_agent_api::{BrowserJobsApi, BrowserStateApi, CliApi, GitKit, RunApi, SettingsRef, StoreFacets, TasksFacet};
use iyakki_shared::extagentcfg::AgentsCfg;
use iyakki_core_api::Core;
use iyakki_sec::policy::{Policy, Role};

use super::AppCx;

/// A line sink for the audit log: `(action, subject)`, already bound to the signed-in person.
pub type AuditSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub trait IdePort {
    /// The signed-in person.
    fn user(&self) -> &str;
    /// What that person's policy allows.
    fn policy(&self) -> &Policy;
    /// The role the person signed in with.
    fn role(&self) -> Role;
    /// The person's data folder of this install.
    fn data_dir(&self) -> &Path;
    /// One line in the audit log, for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// A sink that writes audit lines for the signed-in person (for code that runs on another thread).
    fn audit_sink(&self) -> AuditSink;
    /// The newest audit lines (at most `n`).
    fn audit_recent(&self, n: usize) -> Vec<String>;
    /// The core (devices, files, browser), the way every tile reaches the machine.
    fn core(&self) -> Arc<dyn Core>;
    /// The settings of the install.
    fn settings(&self) -> SettingsRef;
    /// Seconds since the epoch, from the shell's clock (a manual clock in the fake).
    fn now_secs(&self) -> u64;
    // ---- the facets the IDE panes read ----
    /// The browser state the IDE's preview and browser pane share with the Browser app.
    fn browser(&self) -> Arc<dyn BrowserStateApi>;
    /// The browser's background jobs (downloads and the like).
    fn browser_jobs(&self) -> Arc<dyn BrowserJobsApi>;
    /// The Git kit (clone, status, commit, push over the person's keys).
    fn git_kit(&self) -> Arc<dyn GitKit>;
    /// The command-line tools the person's policy lets the IDE start.
    fn clis(&self) -> Arc<dyn CliApi>;
    /// The store facets (tiles, keys, publish).
    fn storeapi(&self) -> Arc<dyn StoreFacets>;
    /// The task manager of the workspaces.
    fn tasks_facet(&self) -> Arc<dyn TasksFacet>;
    /// The saved run logs of the person.
    fn runs(&self) -> Arc<dyn RunApi>;
    /// The knowledge base the workspace histories are kept in.
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource>;
    /// The upstream the quick web fetches of the Publish flow go through (the person's host policy applies).
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream>;
    /// The external agents the person may choose, as configured.
    fn agents_cfg(&self) -> AgentsCfg;
}

/// The real port, over the shell's context. A cheap view (copy it freely); the reads live as long as the context, not as long as the view.
#[derive(Clone, Copy)]
pub struct CxIde<'a>(pub &'a AppCx);

impl<'a> CxIde<'a> {
    pub fn user(&self) -> &'a str {
        &self.0.session.user
    }
    pub fn policy(&self) -> &'a Policy {
        &self.0.session.policy
    }
    pub fn role(&self) -> Role {
        self.0.session.role
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
    pub fn browser(&self) -> Arc<dyn BrowserStateApi> {
        self.0.browser()
    }
    pub fn browser_jobs(&self) -> Arc<dyn BrowserJobsApi> {
        self.0.browser_jobs()
    }
    pub fn git_kit(&self) -> Arc<dyn GitKit> {
        self.0.git_kit()
    }
    pub fn clis(&self) -> Arc<dyn CliApi> {
        self.0.clis()
    }
    pub fn storeapi(&self) -> Arc<dyn StoreFacets> {
        self.0.storeapi()
    }
    pub fn tasks_facet(&self) -> Arc<dyn TasksFacet> {
        self.0.tasks_facet()
    }
    pub fn runs(&self) -> Arc<dyn RunApi> {
        self.0.runs()
    }
    pub fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.0.kb_source()
    }
    pub fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        self.0.quick_upstream()
    }
    pub fn agents_cfg(&self) -> AgentsCfg {
        self.0.agents_cfg()
    }
}

impl IdePort for CxIde<'_> {
    fn user(&self) -> &str {
        CxIde::user(self)
    }
    fn policy(&self) -> &Policy {
        CxIde::policy(self)
    }
    fn role(&self) -> Role {
        CxIde::role(self)
    }
    fn data_dir(&self) -> &Path {
        CxIde::data_dir(self)
    }
    fn audit(&self, action: &str, subject: &str) {
        CxIde::audit(self, action, subject)
    }
    fn audit_sink(&self) -> AuditSink {
        CxIde::audit_sink(self)
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        CxIde::audit_recent(self, n)
    }
    fn core(&self) -> Arc<dyn Core> {
        CxIde::core(self)
    }
    fn settings(&self) -> SettingsRef {
        CxIde::settings(self)
    }
    fn now_secs(&self) -> u64 {
        CxIde::now_secs(self)
    }
    fn browser(&self) -> Arc<dyn BrowserStateApi> {
        CxIde::browser(self)
    }
    fn browser_jobs(&self) -> Arc<dyn BrowserJobsApi> {
        CxIde::browser_jobs(self)
    }
    fn git_kit(&self) -> Arc<dyn GitKit> {
        CxIde::git_kit(self)
    }
    fn clis(&self) -> Arc<dyn CliApi> {
        CxIde::clis(self)
    }
    fn storeapi(&self) -> Arc<dyn StoreFacets> {
        CxIde::storeapi(self)
    }
    fn tasks_facet(&self) -> Arc<dyn TasksFacet> {
        CxIde::tasks_facet(self)
    }
    fn runs(&self) -> Arc<dyn RunApi> {
        CxIde::runs(self)
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        CxIde::kb_source(self)
    }
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        CxIde::quick_upstream(self)
    }
    fn agents_cfg(&self) -> AgentsCfg {
        CxIde::agents_cfg(self)
    }
}

/// A port in memory for tests: fixed person, policy and role, an audit list, a manual clock, the core and settings it is given. The facets are given
/// with the `with_` builders; asking for one that was not given stops the test with a sentence saying which (a fake never falls back to the real thing).
#[cfg(test)]
pub struct FakeIdePort {
    pub user: String,
    pub policy: Policy,
    pub role: Role,
    pub dir: PathBuf,
    pub core: Arc<dyn Core>,
    pub settings: SettingsRef,
    browser: Option<Arc<dyn BrowserStateApi>>,
    jobs: Option<Arc<dyn BrowserJobsApi>>,
    git: Option<Arc<dyn GitKit>>,
    clis: Option<Arc<dyn CliApi>>,
    store: Option<Arc<dyn StoreFacets>>,
    tasks: Option<Arc<dyn TasksFacet>>,
    runs: Option<Arc<dyn RunApi>>,
    kb: Option<Arc<dyn iyakki_memx::kb::KbSource>>,
    upstream: Option<Arc<dyn iyakki_proxy::Upstream>>,
    pub agents: AgentsCfg,
    audit: Arc<Mutex<Vec<(String, String, String)>>>,
    clock: AtomicU64,
}

#[cfg(test)]
impl FakeIdePort {
    pub fn new(user: &str, role: Role, dir: &Path, core: Arc<dyn Core>, settings: SettingsRef) -> FakeIdePort {
        FakeIdePort { user: user.into(), policy: role.template(), role, dir: dir.into(), core, settings, browser: None, jobs: None, git: None, clis: None, store: None, tasks: None, runs: None, kb: None, upstream: None, agents: AgentsCfg::default(), audit: Arc::new(Mutex::new(Vec::new())), clock: AtomicU64::new(1_700_000_000) }
    }
    pub fn with_browser(mut self, b: Arc<dyn BrowserStateApi>) -> FakeIdePort {
        self.browser = Some(b);
        self
    }
    pub fn with_browser_jobs(mut self, b: Arc<dyn BrowserJobsApi>) -> FakeIdePort {
        self.jobs = Some(b);
        self
    }
    pub fn with_git_kit(mut self, g: Arc<dyn GitKit>) -> FakeIdePort {
        self.git = Some(g);
        self
    }
    pub fn with_clis(mut self, c: Arc<dyn CliApi>) -> FakeIdePort {
        self.clis = Some(c);
        self
    }
    pub fn with_storeapi(mut self, s: Arc<dyn StoreFacets>) -> FakeIdePort {
        self.store = Some(s);
        self
    }
    pub fn with_tasks_facet(mut self, t: Arc<dyn TasksFacet>) -> FakeIdePort {
        self.tasks = Some(t);
        self
    }
    pub fn with_runs(mut self, r: Arc<dyn RunApi>) -> FakeIdePort {
        self.runs = Some(r);
        self
    }
    pub fn with_kb_source(mut self, k: Arc<dyn iyakki_memx::kb::KbSource>) -> FakeIdePort {
        self.kb = Some(k);
        self
    }
    pub fn with_quick_upstream(mut self, u: Arc<dyn iyakki_proxy::Upstream>) -> FakeIdePort {
        self.upstream = Some(u);
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
impl IdePort for FakeIdePort {
    fn user(&self) -> &str {
        &self.user
    }
    fn policy(&self) -> &Policy {
        &self.policy
    }
    fn role(&self) -> Role {
        self.role
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
    fn browser(&self) -> Arc<dyn BrowserStateApi> {
        self.browser.clone().expect("FakeIdePort: give it a browser state with with_browser")
    }
    fn browser_jobs(&self) -> Arc<dyn BrowserJobsApi> {
        self.jobs.clone().expect("FakeIdePort: give it browser jobs with with_browser_jobs")
    }
    fn git_kit(&self) -> Arc<dyn GitKit> {
        self.git.clone().expect("FakeIdePort: give it a Git kit with with_git_kit")
    }
    fn clis(&self) -> Arc<dyn CliApi> {
        self.clis.clone().expect("FakeIdePort: give it CLI tools with with_clis")
    }
    fn storeapi(&self) -> Arc<dyn StoreFacets> {
        self.store.clone().expect("FakeIdePort: give it store facets with with_storeapi")
    }
    fn tasks_facet(&self) -> Arc<dyn TasksFacet> {
        self.tasks.clone().expect("FakeIdePort: give it a task manager with with_tasks_facet")
    }
    fn runs(&self) -> Arc<dyn RunApi> {
        self.runs.clone().expect("FakeIdePort: give it run logs with with_runs")
    }
    fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.kb.clone().expect("FakeIdePort: give it a knowledge base with with_kb_source")
    }
    fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        self.upstream.clone().expect("FakeIdePort: give it an upstream with with_quick_upstream")
    }
    fn agents_cfg(&self) -> AgentsCfg {
        self.agents.clone()
    }
}

#[cfg(test)]
#[path = "ide_port_tests.rs"]
mod tests;
