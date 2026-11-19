//! Facets of alpha.6 lane split-c (F-SPLIT-03): the stateful objects the UI used to build from agent modules. The real
//! implementation is `iyakki_agent::facets_b6`; the defaults here are the fake (a core that serves none of them).

use std::any::Any;
use iyakki_sec::policy::Policy;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use iyakki_shared::agent::Tools;
use iyakki_shared::geminilive_host::{Board, RunState, Running};

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B6: &str = "unavailable: this core does not serve this call";

/// What the shell brings to a Gemini live voice session (EXPERIMENTAL): the person, the tools, the board of cards, the microphone,
/// the local voice. The connection, the key, the sound and the speaker question are the core's.
pub struct LiveSpec {
    pub user: String,
    /// The chat's tool set of the account; `None`: no tools in this session.
    pub tools: Option<Arc<dyn Tools>>,
    pub board: Arc<Board>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// The chat's Auto mode switch (never covers the always-ask tools).
    pub auto: Arc<dyn Fn() -> bool + Send + Sync>,
    pub mic: Box<dyn iyakki_shared::wakelisten::Mic>,
    pub speaker: Box<dyn iyakki_shared::voiceassist::Speaker>,
    pub log: Box<dyn iyakki_shared::voiceassist::TurnLog>,
    /// Tests only: replace the real connection (`Arc<dyn LiveConnector>` of the core, type-erased), the sound sink and the key.
    pub connector: Option<Box<dyn Any + Send>>,
    pub sink: Option<Arc<dyn iyakki_shared::story::Sink>>,
    pub key: Option<String>,
}

/// The Gemini live voice session of this install (the real one is `iyakki_agent::facets_b6::GeminiLiveReal`).
pub trait GeminiLive: Send + Sync {
    /// Starts a session on its own thread; returns at once, the state is read from the handle.
    fn spawn(&self, _spec: LiveSpec) -> Running {
        Running::new(Arc::new(Mutex::new(RunState::Fallback(UNAVAILABLE_B6.into()))), Arc::new(AtomicBool::new(false)), None, Arc::new(|| {}))
    }
}

/// The fake: starts nothing.
pub struct NoGeminiLive;
impl GeminiLive for NoGeminiLive {}

/// What needs the process's services; implemented for `iyakki_agent::services::Services` (`self: &Arc<Self>`).
pub trait ServicesB6 {
    /// The Gemini live voice session.
    fn gemini_live(self: &Arc<Self>) -> Arc<dyn GeminiLive> {
        Arc::new(NoGeminiLive)
    }
    /// The model router of `user`: `audit(what, detail)`, `gate` is the core's say over a strategy change and a run.
    fn router_panel(self: &Arc<Self>, _user: &str, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>, _gate: Arc<dyn iyakki_core_api::router::Router>) -> Arc<dyn RouterPanel> {
        Arc::new(NoRouterPanel)
    }
    /// Tests: the panel over a backend the test made (`Arc<dyn iyakki_agent::routertools::Backend>`, type-erased); `None` when it is not one.
    fn router_panel_over(self: &Arc<Self>, _backend: Box<dyn Any + Send>) -> Option<Arc<dyn RouterPanel>> {
        None
    }
    /// The person's remote files manager: `local(text, write)` is the person's file rules for a path on this computer (the real path, or why not),
    /// `downloads` the folder downloads go to by default.
    fn remote_kit(self: &Arc<Self>, _user: &str, _data: &Path, _downloads: &str, _local: Arc<dyn Fn(&str, bool) -> Result<PathBuf, String> + Send + Sync>, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn RemoteKit> {
        Arc::new(NoRemoteKit)
    }
    /// The core contract of this install (settings, vault, flows) for a local caller.
    fn contract_local(self: &Arc<Self>) -> Arc<dyn ContractLocal> {
        Arc::new(NoContractLocal)
    }
    /// The terminal of `user` (skills, store, toollog and the SSH commands included).
    fn terminal(self: &Arc<Self>, _user: &str, _policy: Policy, _host: Arc<dyn iyakki_core_api::HostApi>) -> Box<dyn Terminal> {
        Box::new(NoTerminal)
    }
    /// The documents search of `user`.
    fn rag_kit(self: &Arc<Self>, _user: &str) -> Arc<dyn RagKit> {
        Arc::new(NoRagKit)
    }
    /// The SFTP file server of this install, with the shell's accounts as the login check.
    fn sftp_server(self: &Arc<Self>, _data: &Path, _login: SftpLogin, _lookup: SftpLookup, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn SftpServer> {
        Arc::new(NoSftpServer)
    }
    /// The lines of the SFTP status for the Remote access page.
    fn sftp_status_lines(self: &Arc<Self>, _data: &Path) -> Vec<String> {
        Vec::new()
    }
    /// The Git engine (programs, keys, batches of jobs).
    fn git_kit(self: &Arc<Self>) -> Arc<dyn GitKit> {
        Arc::new(NoGitKit)
    }
    /// The recordings' sound and the revising assistant.
    fn speech_kit(self: &Arc<Self>) -> Arc<dyn SpeechKit> {
        Arc::new(NoSpeechKit)
    }
}

/// A sound sink that plays nothing (a core without sound).
pub struct NoSink;
impl iyakki_shared::story::Sink for NoSink {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn iyakki_shared::story::Playing>, String> {
        Err(UNAVAILABLE_B6.into())
    }
}

/// Speech of this install: the sound a recording plays through, and the assistant that revises a dictated text (the real one is
/// `iyakki_agent::facets_b6::SpeechKitReal`).
pub trait SpeechKit: Send + Sync {
    /// Plays WAV bytes of a recording through the one way sound reaches the speaker (the mixer, or the play command of Settings -> Voice).
    fn recording_sink(&self) -> Arc<dyn iyakki_shared::story::Sink> {
        Arc::new(NoSink)
    }
    /// The model a revision would use and whether it may be used now.
    fn revise_target(&self, _ctx: &iyakki_shared::revise::Context) -> iyakki_shared::dictfix::Target {
        iyakki_shared::dictfix::Target::NoModel
    }
    /// Revise `text` with the person's chat model (blocks up to the revise timeout: call it from a worker thread).
    fn revise(&self, text: &str, _kind: &iyakki_shared::revise::Kind, _ctx: &iyakki_shared::revise::Context) -> iyakki_shared::revise::Proposal {
        iyakki_shared::revise::Proposal::kept(text, UNAVAILABLE_B6)
    }
}

/// The fake: no sound, no model.
pub struct NoSpeechKit;
impl SpeechKit for NoSpeechKit {}

/// The deploy and registry steps (the real one is `iyakki_agent::facets_b6::DeployKitReal`): what the K8S panel and Settings -> Dev cluster
/// run against the person's cluster after the plan (pure, in `iyakki_shared::regsetup`) was shown and approved.
pub trait DeployKit: Send + Sync {
    /// The registry strip's status: the saved choice, plus whether `GET /v2/` answers from this machine.
    fn reg_status(&self, c: &iyakki_shared::workspace::K8sConfig, _run: &dyn iyakki_shared::wsexec::Runner) -> iyakki_shared::regsetup::RegStatus {
        iyakki_shared::regsetup::RegStatus::saved(c)
    }
    /// What the cluster says that the in-cluster registry plan depends on (StorageClass, taken NodePorts ...).
    fn reg_probe(&self, _run: &dyn iyakki_shared::wsexec::Runner, _k: &iyakki_shared::regsetup::KubeCtx) -> iyakki_shared::regsetup::Facts {
        iyakki_shared::regsetup::Facts::default()
    }
    /// Carries out an approved registry plan (`approved` false: refused).
    fn reg_apply(&self, _p: &iyakki_shared::regsetup::SetupPlan, _run: &dyn iyakki_shared::wsexec::Runner, _approved: bool) -> Result<iyakki_shared::regsetup::Applied, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// The cluster of a profile as the local image registry steps address it.
    fn localreg_target(&self, p: &iyakki_shared::devcluster::ClusterProfile) -> iyakki_shared::localreg::Target {
        iyakki_shared::localreg::Target { name: p.name.clone(), kubeconfig: p.kubeconfig.clone(), context: p.context.clone(), node_host: String::new() }
    }
    /// What exists of the local image registry now (read-only kubectl, and `GET /v2/_catalog` against the registry).
    fn localreg_inspect(&self, _run: &Arc<dyn iyakki_shared::wsexec::Runner>, _t: &iyakki_shared::localreg::Target) -> iyakki_shared::localreg::State {
        iyakki_shared::localreg::State::Unknown(UNAVAILABLE_B6.into())
    }
    /// Whether the cluster can hold the registry (answers, a node is Ready, a StorageClass).
    fn localreg_check_dependency(&self, _run: &Arc<dyn iyakki_shared::wsexec::Runner>, _t: &iyakki_shared::localreg::Target) -> iyakki_shared::localreg::Dependency {
        iyakki_shared::localreg::Dependency::Missing(UNAVAILABLE_B6.into())
    }
    /// Removes the local image registry; `typed` is what the person typed to confirm.
    fn localreg_remove(&self, _run: &Arc<dyn iyakki_shared::wsexec::Runner>, _t: &iyakki_shared::localreg::Target, _typed: &str, _say: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// The image a cloud tenant runs unless the person names another.
    fn cloud_default_image(&self) -> String {
        String::new()
    }
    /// What launching cloud mode would do (and what blocks it), as a plan.
    fn cloud_plan(&self, _l: &iyakki_shared::cloudlaunch::LaunchSpec) -> iyakki_shared::cloudlaunch::Plan {
        iyakki_shared::cloudlaunch::Plan { blockers: vec![UNAVAILABLE_B6.into()], ..Default::default() }
    }
    /// The words of the preview: exactly what would be applied.
    fn cloud_preview(&self, _l: &iyakki_shared::cloudlaunch::LaunchSpec, _p: &iyakki_shared::cloudlaunch::Plan) -> String {
        UNAVAILABLE_B6.into()
    }
    /// Applies the plan: `kubectl` runs one call, `push` pushes the image; the lines it said.
    fn cloud_execute(&self, _l: &iyakki_shared::cloudlaunch::LaunchSpec, _p: &iyakki_shared::cloudlaunch::Plan, _work: &Path, _kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, _push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// `iyakki cloud ...` (example, manifests, secret, reach ...): lines go to `out`.
    fn cloud_cli(&self, _args: &[String], _out: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err(UNAVAILABLE_B6.into())
    }
}

/// The fake: nothing is applied.
pub struct NoDeployKit;
impl DeployKit for NoDeployKit {}

/// The model router of one person as Settings -> Model router sees it (the real one is `iyakki_agent::facets_b6::RouterPanelReal`, over
/// the same backend the assistant's `router_*` tools use, so a change here is the same code as the tool).
pub trait RouterPanel: Send + Sync {
    fn config(&self) -> iyakki_shared::router::RouterConfig {
        iyakki_shared::router::RouterConfig::default()
    }
    /// The models the router knows (providers, with the person's corrections applied).
    fn profiles(&self) -> Vec<iyakki_shared::router_registry::Profile> {
        Vec::new()
    }
    /// The strategy scripts the person added: (name, bytes).
    fn strategies(&self) -> Vec<(String, usize)> {
        Vec::new()
    }
    /// The run going on, in words; `None`: no run.
    fn progress(&self) -> Option<String> {
        None
    }
    /// The summary of the latest run.
    fn last_run(&self) -> Option<String> {
        None
    }
    /// `router_set` (on/off, role, cost, zone, budget ...): the arguments of the tool.
    fn set(&self, _args: &serde_json::Value) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
    fn strategy_set(&self, _args: &serde_json::Value) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
    fn strategy_remove(&self, _args: &serde_json::Value) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// A script by name and text: tried on a sample first, kept only when it passes.
    fn strategy_add(&self, _args: &serde_json::Value) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// Cancels the run (after the model call that is running).
    fn cancel(&self) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
}

/// The fake: no router.
pub struct NoRouterPanel;
impl RouterPanel for NoRouterPanel {}

/// The person's remote files (SFTP and FTPS) manager: the tile's source (`RemoteSource`) and the assistant's tools over the same sessions.
pub trait RemoteKit: iyakki_tile_api::remote::RemoteSource + Send + Sync {
    /// The assistant's `remote_*` tools around `inner` (listing is free; connecting, moving and changing ask).
    fn wrap_tools(&self, inner: Arc<dyn Tools>) -> Arc<dyn Tools> {
        inner
    }
}

/// The fake: nothing is connected.
pub struct NoRemoteKit;
impl iyakki_tile_api::remote::RemoteSource for NoRemoteKit {
    fn connections(&self) -> Vec<iyakki_tile_api::remote::Conn> {
        Vec::new()
    }
    fn save(&self, c: &iyakki_tile_api::remote::Conn, p: Option<&str>, r: Option<&str>) -> Result<(), String> {
        iyakki_tile_api::remote::NoRemote.save(c, p, r)
    }
    fn remove(&self, n: &str) -> Result<(), String> {
        iyakki_tile_api::remote::NoRemote.remove(n)
    }
    fn key_names(&self) -> Vec<String> {
        Vec::new()
    }
    fn submit(&self, j: iyakki_tile_api::remote::Job) -> u64 {
        iyakki_tile_api::remote::NoRemote.submit(j)
    }
    fn take(&self, t: u64) -> Option<iyakki_tile_api::remote::JobResult> {
        iyakki_tile_api::remote::NoRemote.take(t)
    }
    fn trust(&self, n: &str, f: &str) -> Result<(), String> {
        iyakki_tile_api::remote::NoRemote.trust(n, f)
    }
    fn forget_host(&self, n: &str) -> Result<(), String> {
        iyakki_tile_api::remote::NoRemote.forget_host(n)
    }
    fn close(&self, s: u64) {
        iyakki_tile_api::remote::NoRemote.close(s)
    }
    fn start_transfer(&self, r: &iyakki_tile_api::remote::TransferReq) -> Result<u64, String> {
        iyakki_tile_api::remote::NoRemote.start_transfer(r)
    }
    fn transfers(&self) -> Vec<iyakki_tile_api::remote::TransferInfo> {
        Vec::new()
    }
    fn cancel_transfer(&self, id: u64) {
        iyakki_tile_api::remote::NoRemote.cancel_transfer(id)
    }
    fn retry_transfer(&self, id: u64) -> Result<u64, String> {
        iyakki_tile_api::remote::NoRemote.retry_transfer(id)
    }
    fn clear_finished(&self) {}
    fn download_dir(&self) -> String {
        String::new()
    }
}
impl RemoteKit for NoRemoteKit {}

/// What is wrong with a line of a git request, by line number (1-based), before anything runs.
pub type GitLineErrors = Vec<(usize, String)>;

/// A running batch of clone, fetch, pull or push jobs (the real one is `iyakki_agent::facets_b6::GitBatchReal`): what the Git tile and the
/// AI IDE read and press while it runs. Nothing here blocks.
pub trait GitBatch: Send {
    fn items(&self) -> Vec<iyakki_shared::gitjobs::Item>;
    fn phase(&self) -> iyakki_shared::gitjobs::Phase;
    /// Hosts whose key the person has to look at.
    fn prompts(&self) -> Vec<iyakki_shared::gitjobs::HostPrompt>;
    fn done(&self) -> usize;
    fn failed(&self) -> usize;
    fn pending(&self) -> bool;
    fn running_count(&self) -> usize;
    /// Does the work that is due (probe answers, queued items, finished jobs, project.json for cloned repositories).
    fn tick(&mut self);
    /// The person compared the fingerprint and trusts the host.
    fn trust_host(&mut self, host: &str, port: u16) -> Result<(), String>;
    fn refuse_host(&mut self, host: &str, port: u16);
    /// Makes a failed item run again.
    fn retry(&mut self, item: usize) -> bool;
    /// True once after entries were written into project.json.
    fn take_changed(&mut self) -> bool;
    /// The key service's folder (tests and the audit).
    fn agent_dir(&self) -> Option<PathBuf>;
    /// Tests: put an item into a state.
    fn set_state(&mut self, item: usize, state: iyakki_shared::gitjobs::ItemState);
}

/// The Git engine of this install: the programs, the person's keys and the batches (the real one is `iyakki_agent::facets_b6::GitKitReal`).
pub trait GitKit: Send + Sync {
    /// Looks for git in `search` and says plainly when it is missing.
    fn check_git(&self, _search: &str) -> iyakki_shared::gitjobs::GitCheck {
        iyakki_shared::gitjobs::GitCheck::Missing { message: UNAVAILABLE_B6.into() }
    }
    /// The git and ssh programs of this system.
    fn detect_tools(&self, _search: &str) -> Result<iyakki_shared::gitjobs::Tools, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// Asks the SSH client what a server presents.
    fn real_probe(&self, _search: &str) -> iyakki_shared::gitjobs::Probe {
        std::sync::Arc::new(|_, _| Err(UNAVAILABLE_B6.into()))
    }
    /// The one way a person's clone, fetch, pull or push is set up: keys, programs and the write check of the account.
    fn user_ctx(&self, _a: iyakki_shared::gitjobs::CtxFor) -> Result<iyakki_shared::gitjobs::Ctx, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// `@instance/owner/repo` of a platform Gitea; `None`: not such a reference.
    fn expand_instance(&self, _dir: &Path, _user: &str, _text: &str) -> Option<Result<iyakki_shared::gitjobs::InstanceRef, String>> {
        None
    }
    /// What would be lost by removing a clone, in words (empty: nothing).
    fn removal_risks(&self, _git: &Path, _dir: &Path) -> String {
        String::new()
    }
    fn remove_repo(&self, _root: &Path, _folder: &str, _typed: &str) -> Result<(), String> {
        Err(UNAVAILABLE_B6.into())
    }
    fn repo_status(&self, _git: &Path, _dir: &Path) -> Result<iyakki_shared::gitjobs::RepoStatus, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// Checks every line and the folders; nothing starts and no network is used.
    fn plan_clone(&self, _ctx: iyakki_shared::gitjobs::Ctx, _specs: &[iyakki_shared::gitjobs::CloneSpec]) -> Result<Box<dyn GitBatch>, GitLineErrors> {
        Err(vec![(0, UNAVAILABLE_B6.into())])
    }
    fn plan_update(&self, _ctx: iyakki_shared::gitjobs::Ctx, _repos: &[iyakki_shared::gitsite::RepoEntry], _op: iyakki_shared::gitjobs::Op) -> Result<Box<dyn GitBatch>, GitLineErrors> {
        Err(vec![(0, UNAVAILABLE_B6.into())])
    }
    fn plan_push(&self, _ctx: iyakki_shared::gitjobs::Ctx, _url: &str, _folder: &str, _branch: &str) -> Result<Box<dyn GitBatch>, GitLineErrors> {
        Err(vec![(0, UNAVAILABLE_B6.into())])
    }
}

/// The fake: no git.
pub struct NoGitKit;
impl GitKit for NoGitKit {}

/// The core contract served over this install's services (the real one is `iyakki_agent::facets_b6::ContractLocalReal`): the in-process
/// side of the contract the tiles call with a local `CallContext`. Same traits, guards and audit lines as the remote transport.
pub trait ContractLocal: Send + Sync {
    fn settings(&self) -> Option<&dyn iyakki_core_api::contract::Settings> {
        None
    }
    fn vault(&self) -> Option<&dyn iyakki_core_api::contract::Vault> {
        None
    }
    /// The flows of the account whose environment the caller holds (`Arc<iyakki_agent::flows::FlowEnv>`, type-erased).
    fn flows_for_env(&self, _env: Arc<dyn Any + Send + Sync>) -> Option<Arc<dyn iyakki_core_api::contract::Flows>> {
        None
    }
}

/// The fake: nothing is served.
pub struct NoContractLocal;
impl ContractLocal for NoContractLocal {}

/// One SSH command of the terminal, owning what it needs so it can run on a thread of its own.
pub trait TerminalCall: Send {
    /// The program name and the arguments as typed (never shown in the audit log: they can hold host names and users).
    fn command(&self) -> &str;
    /// Runs it (blocks up to the command's limit); lines go to `sink` as they come.
    fn run(&self, sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String>;
}

/// The CLI tile's terminal of one person (the real one is `iyakki_agent::cli::Cli`): every command goes through the person's policy.
pub trait Terminal: Send {
    fn prompt(&self) -> String {
        String::from("$ ")
    }
    /// Runs one line.
    fn exec(&mut self, _line: &str) -> iyakki_shared::cli::CliOut {
        iyakki_shared::cli::CliOut { text: UNAVAILABLE_B6.into(), action: iyakki_shared::cli::Action::None }
    }
    /// An SSH command that is to run on a thread of its own (`None`: not one).
    fn ssh_call(&self, _line: &str) -> Option<Box<dyn TerminalCall>> {
        None
    }
    /// The account may run every command (not only the restricted set).
    fn full(&self) -> bool {
        false
    }
}

/// The fake: a terminal that runs nothing.
pub struct NoTerminal;
impl Terminal for NoTerminal {}

/// The documents search of one person as the RAG tile and Settings use it (the real one is `iyakki_agent::facets_b6::RagKitReal`).
pub trait RagKit: Send + Sync {
    /// Runs the bounded research loop (plan, search, grade, rewrite, cite) over the account's collections and keeps the ledger of the run.
    /// Blocks: call it from a worker thread.
    fn research(&self, _question: &str, _k: usize, _model: Option<&dyn iyakki_shared::chat::ChatBackend>) -> iyakki_shared::agenticrag::Ledger {
        let b = iyakki_shared::agenticrag::Budget::default();
        iyakki_shared::agenticrag::Ledger::new("", &[], b, String::new())
    }
    /// The last research runs of the person, newest first.
    fn runs(&self) -> Vec<iyakki_shared::agenticrag::Ledger> {
        Vec::new()
    }
}

/// The fake: no documents.
pub struct NoRagKit;
impl RagKit for NoRagKit {}

/// The SFTP file server of this install (the real one is `iyakki_agent::sftp::Sftp` behind `facets_b6::SftpReal`): what Settings -> Remote
/// access shows and changes. Every change is audited by the caller.
pub trait SftpServer: Send + Sync {
    fn dir(&self) -> PathBuf {
        PathBuf::new()
    }
    fn config(&self) -> iyakki_shared::sftp::Config {
        iyakki_shared::sftp::Config::default()
    }
    fn status(&self) -> iyakki_shared::sftp::Status {
        iyakki_shared::sftp::Status { running: false, port: 0, until: 0, by: String::new(), config: self.config(), host_fingerprint: String::new(), host_fingerprint_ec: String::new(), host_fingerprint_rsa: String::new(), keys: Vec::new(), addresses: Vec::new() }
    }
    /// Why it cannot be started now (`None`: nothing in the way).
    fn problem(&self) -> Option<String> {
        Some(UNAVAILABLE_B6.into())
    }
    fn enable(&self, _by: &str) -> Result<iyakki_shared::sftp::Status, String> {
        Err(UNAVAILABLE_B6.into())
    }
    fn disable(&self, _by: &str) -> bool {
        false
    }
    /// Changes the saved settings with `f` (checked, then saved); the new settings.
    fn configure_dyn(&self, _f: &mut dyn FnMut(&mut iyakki_shared::sftp::Config) -> Result<(), String>) -> Result<iyakki_shared::sftp::Config, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// Adds a public key line for an account; its fingerprint.
    fn add_key(&self, _user: &str, _line: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B6.into())
    }
    fn remove_key(&self, _user: &str, _fingerprint: &str) -> Result<usize, String> {
        Err(UNAVAILABLE_B6.into())
    }
}

impl dyn SftpServer {
    /// [`SftpServer::configure_dyn`] with a closure that is called once.
    pub fn configure(&self, f: impl FnOnce(&mut iyakki_shared::sftp::Config) -> Result<(), String>) -> Result<iyakki_shared::sftp::Config, String> {
        let mut f = Some(f);
        self.configure_dyn(&mut |c| (f.take().expect("called once"))(c))
    }
}

/// The fake: no server.
pub struct NoSftpServer;
impl SftpServer for NoSftpServer {}

/// What the SFTP login check asks of the shell's accounts.
pub type SftpLogin = Arc<dyn Fn(&str, &str) -> Result<Policy, String> + Send + Sync>;
pub type SftpLookup = Arc<dyn Fn(&str) -> Option<Policy> + Send + Sync>;

/// A plan of the browser add-on install as the Store card reads it.
pub struct AvaddonPlan {
    /// Something is supplied (a package, a local ffmpeg, a mirror); `false`: the default is to build here.
    pub has_source: bool,
    /// What the administrator reads first.
    pub review: Vec<String>,
}

/// Stateless helpers of lane split-c (the real one is `iyakki_agent::facets_b6::RealHelpersB6`).
pub trait HelpersB6: Send + Sync {
    /// The Git connections' own HTTP(S) client (rustls, the token only in memory, a pinned certificate when the person chose one).
    /// The default reaches no server.
    fn git_http(&self) -> Arc<dyn iyakki_shared::gitconn::GitHttp> {
        Arc::new(iyakki_shared::gitconn::FakeGitHttp::default())
    }
    /// The privacy zone a provider runs in (this computer, the local network, the internet).
    fn router_zone(&self, _cfg: &iyakki_shared::providers::ProviderCfg) -> iyakki_shared::router_registry::Zone {
        iyakki_shared::router_registry::Zone::Internet
    }
    /// Takes a capability away from an installation (Settings, Apps access). `Ok(false)`: it did not hold it. The caller writes the audit line.
    fn access_revoke(&self, _data: &Path, _installation: &str, _capability: &str) -> Result<bool, String> {
        Err(UNAVAILABLE_B6.into())
    }
    /// Starts a check of the signed download list in the background (`force`: now, whatever the time since the last one). False: not started.
    fn dl_fetch_start(&self, _dir: &Path, _force: bool) -> bool {
        false
    }
    /// The audit lines the check made since the last call (`action`, `detail`).
    fn dl_fetch_take_audit(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// What the last check says.
    fn dl_fetch_status(&self, _dir: &Path) -> iyakki_shared::dlmanifest_fetch::CheckStatus {
        iyakki_shared::dlmanifest_fetch::CheckStatus::default()
    }
    /// The installation's camera hub as frames for a client (EXPERIMENTAL): `who(person)` makes the hub's `Who` of a person, both type-erased.
    fn camera_feed(&self, _hub: Arc<dyn Any + Send + Sync>, _who: Arc<dyn Fn(&str) -> Box<dyn Any + Send> + Send + Sync>) -> Option<Arc<dyn iyakki_shared::clientmedia::CameraFeed>> {
        None
    }
    /// What installing the browser audio and video add-on would do, and the review the administrator reads first.
    fn avaddon_plan(&self, _data: &Path) -> AvaddonPlan {
        AvaddonPlan { has_source: false, review: Vec::new() }
    }
    /// The deploy and registry steps.
    fn deploy_kit(&self) -> Arc<dyn DeployKit> {
        Arc::new(NoDeployKit)
    }
}

/// The default helpers.
pub struct NoB6;
impl HelpersB6 for NoB6 {}

/// The accessor (a supertrait of `AgentFacets`).
pub trait AgentFacetsB6: Send + Sync {
    fn helpers_b6(&self) -> &dyn HelpersB6 {
        &NoB6
    }
}
impl AgentFacetsB6 for crate::FakeAgentApi {}

#[cfg(test)]
#[path = "facets_b6_tests.rs"]
mod tests;
