//! Facet of lane alpha.6/split-a (F-SPLIT-03): the account tool chain of the UI (`iyakki-apps` `tool_chain.rs`) put every wrapper on the
//! chat's tools by naming a dozen agent modules. Here each contiguous run of wrappers is ONE method over the services of this install; the
//! default leaves `inner` untouched (a core that serves none of them). The real one is `iyakki_agent::facets_b4`.

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

use iyakki_sec::policy::{Policy, Role};
use iyakki_shared::agent::Tools;
use std::path::Path;
use iyakki_shared::store_publishflow::Preflight;
use iyakki_shared::store_tile::TestReport;
use crate::UNAVAILABLE_STORE;

/// The audit line of a wrapper: what happened, with whom.
pub type AuditLineB4 = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Who the chain is for, and how it audits.
#[derive(Clone)]
pub struct ChainEnv {
    pub user: String,
    pub role: Role,
    pub policy: Policy,
    /// `audit(user, what, detail)`.
    pub audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>,
    /// The last `n` audit lines (the DOCTOR report shows them).
    pub audit_recent: Arc<dyn Fn(usize) -> Vec<String> + Send + Sync>,
}

/// The accounts of the shell, for the SFTP server: `login(name, password)` and `lookup(name)` give the account's policy.
pub struct SftpAccounts {
    pub login: Arc<dyn Fn(&str, &str) -> Result<Policy, String> + Send + Sync>,
    pub lookup: Arc<dyn Fn(&str) -> Option<Policy> + Send + Sync>,
}

/// What the model router needs from the UI side; the hub and the person of the cameras travel type-erased (the agent downcasts them).
pub struct RouterParts {
    pub project_root: Option<PathBuf>,
    pub scope: String,
    pub camera_hub: Arc<dyn Any + Send + Sync>,
    pub camera_who: Box<dyn Any + Send + Sync>,
    pub os: Arc<dyn iyakki_core_api::Os>,
}

/// The wrapper runs of the account tool chain.
pub trait ServicesFacetB4: Send + Sync {
    /// SSH tunnels (administrators), domain names (administrators), the address bar's short words, the browser's graphics mode (administrators).
    fn wrap_net_browser_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
    /// The SFTP server's tools (administrators only).
    fn wrap_sftp_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv, _accounts: SftpAccounts) -> Arc<dyn Tools> {
        inner
    }
    /// Updates, image build, cluster explorer resources, Linux sandboxes.
    fn wrap_run_a_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
    /// Skills, MCP servers (`file_servers` are the ones of the MCP tile's file), Linux pods.
    fn wrap_run_b_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv, _file_servers: Vec<iyakki_shared::mcp::ServerSpec>) -> Arc<dyn Tools> {
        inner
    }
    /// The model router (experimental).
    fn wrap_router_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv, _parts: RouterParts) -> Arc<dyn Tools> {
        inner
    }
    /// DOCTOR, the log of the last session, the lock screen.
    fn wrap_status_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
    /// The encrypted home, accessibility (`voice_ready` says whether a voice is installed), the browser's accessibility.
    fn wrap_home_access_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
    /// Encrypted drives (not for guests) and format / resize (administrators): the actor's `home` and owner are resolved by the caller.
    fn wrap_disks_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv, _who: iyakki_shared::disks::Actor) -> Arc<dyn Tools> {
        inner
    }
    /// System components, shared builds (administrators), PDF files, the PDF-output switch, WASM tiles, object storage, model providers,
    /// Ollama, document retrieval, quick uninstall, Lemonade compute, this computer, dictation wording, My IYAKKI.
    fn wrap_system_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv, _pdf: PdfParts, _chat: Arc<dyn iyakki_shared::chat::ChatBackend>) -> Arc<dyn Tools> {
        inner
    }
    /// The shared vault's cryptography (administrators), the cloud tenant space, the tool-offer routing.
    fn wrap_vault_routing_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
    /// The Settings tool entries of the pages that live in the agent, first run (views, backup, notifications, screen saver, WASM tiles, startup,
    /// Gemini, Gemini voice, sound). Empty when `env` is not this install's.
    fn settings_entries_a(&self, _env: &iyakki_shared::settingstools::Env) -> Vec<iyakki_shared::settingstools::Entry> {
        Vec::new()
    }
    /// The second run (memory, Linux apps, access, machine, pod TLS, network, wifi).
    fn settings_entries_b(&self, _env: &iyakki_shared::settingstools::Env, _linux: iyakki_shared::settingstools_linux::Access, _wifi: WifiParts) -> Vec<iyakki_shared::settingstools::Entry> {
        Vec::new()
    }
    /// Remove the voice pack (`delete` removes one local model of Lemonade); the words for the person.
    fn voicepack_remove(&self, _delete: &dyn Fn(&str) -> Result<(), String>) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// The SSH tunnels of this person (one shared manager per person and install).
    fn ssh_tunnels(&self, _user: &str, _audit: AuditLineB4) -> Arc<dyn SshTunnels> {
        Arc::new(NoB4)
    }
    /// The tunnels that were on at the last sign-out start again when the person signs in.
    fn ssh_tunnels_start_for_login(&self, _user: &str, _audit: AuditLineB4) {}
    /// A person signed out: their tunnels end.
    fn ssh_tunnels_on_logout(&self, _user: &str) {}
    /// Start an Autopilot run toward `start.goal` over `tools`; `assist` is the agent's `assistrun::Assist` of the chat (type-erased).
    #[allow(clippy::too_many_arguments)]
    fn autopilot_start(&self, _user: &str, _chat: Arc<dyn iyakki_shared::chat::ChatBackend>, _tools: Arc<dyn Tools>, _base: Vec<iyakki_shared::chat::Message>, _limits: iyakki_shared::agent::RunLimits, _assist: Box<dyn Any + Send>, _start: ApStart) -> iyakki_shared::agent::RunHandle {
        let (tx, rx) = std::sync::mpsc::channel();
        let (dtx, _drx) = std::sync::mpsc::channel();
        let _ = tx.send(iyakki_shared::agent::RunEvent::Failed { error: UNAVAILABLE_B4.into() });
        iyakki_shared::agent::RunHandle::from_parts(rx, dtx)
    }
    /// The app store of this install: one handle for the Store tile, the AI IDE and the platform pages.
    fn store(&self) -> Option<Arc<dyn crate::StoreApi>> {
        None
    }
    /// The camera tools of the assistant around `inner`, over the installation's hub (`hub` is the agent's `cameras::Hub`, `who` its `cameras::Who`, both type-erased).
    fn camera_tools(&self, inner: Arc<dyn Tools>, _hub: Arc<dyn Any + Send + Sync>, _who: Box<dyn Any + Send>) -> Arc<dyn Tools> {
        inner
    }
    /// At sign-in: start supervising recordings and the motion watcher over `hub`.
    fn camera_resume(&self, _hub: Arc<dyn Any + Send + Sync>) {}
    /// The hub as the source of the Cameras tile for the person `who` is.
    fn camera_source(&self, _hub: Arc<dyn Any + Send + Sync>, _who: Box<dyn Any + Send>) -> Option<Arc<dyn iyakki_tile_api::cameras::CameraSource>> {
        None
    }
    /// The usage log of this install as the source of the Usage tile (and the tiles that show a token count).
    fn usage_source(&self) -> Option<Arc<dyn iyakki_tile_api::usage::UsageSource>> {
        None
    }
    /// `chat` counted in the usage log for `user` (what the shell wraps every chat backend in).
    fn metered_chat(&self, chat: Arc<dyn iyakki_shared::chat::ChatBackend>, _user: &str) -> Arc<dyn iyakki_shared::chat::ChatBackend> {
        chat
    }
    /// Start the voice assistant ("My IYAKKI") worker of this person: it listens when the microphone is open, answers with the chat's model and the
    /// person's tools (and the assistant's own tool for changing its settings, which only an administrator may use).
    #[allow(clippy::too_many_arguments)]
    fn voice_assistant_spawn(&self, _user: &str, _policy: Policy, _host: Arc<dyn iyakki_core_api::HostApi>, _mcp: Vec<iyakki_shared::mcp::ServerSpec>, _chat: Arc<dyn iyakki_shared::chat::ChatBackend>, _audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>, _recorders: Arc<dyn iyakki_shared::voicedesk::Recorders>) -> Option<Box<dyn MineWorker>> {
        None
    }
    /// Whether a reading voice exists (Settings -> Voice, or an engine found on this computer): without one the screen reader is silent.
    fn voice_ready(&self) -> bool {
        false
    }
    /// The installation's camera hub (the first call makes it; later calls return the same one). `usb` holds an `Arc<dyn UsbProvider>` of the agent (type-erased),
    /// `people` lists the accounts, `encode` makes a JPEG of RGBA pixels, `after_save` hands a saved file to its owner, `host_name` names this computer for the access log.
    #[allow(clippy::too_many_arguments)]
    fn camera_hub(&self, _usb: Box<dyn Any + Send>, _people: Arc<dyn Fn() -> Vec<String> + Send + Sync>, _encode: Arc<dyn Fn(u32, u32, &[u8]) -> Vec<u8> + Send + Sync>, _after_save: Arc<dyn Fn(&std::path::Path) + Send + Sync>, _audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>, _host_name: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
    /// The tools of an AI IDE workspace chat around `base` (the board, the project tests, the connected cluster's `k8s_*`/`deploy_*` and the platform services): the kit
    /// is made once per run so the chat can ask it for the cluster line of the system prompt before it wraps the tools.
    fn project_kit(&self, _spec: ProjectKitSpec) -> Option<Arc<dyn ProjectKit>> {
        None
    }
    /// The workspace's detached jobs around `inner`: the tools `job_start`, `job_status`, ... (the account must be offered them: `cli` is full).
    fn wrap_job_tools(&self, inner: Arc<dyn Tools>, _user: &str, _policy: Policy, _cwd: &std::path::Path, _workspace: Option<(&str, &std::path::Path)>) -> Arc<dyn Tools> {
        inner
    }
    /// A chat run: the model answers with `tools` over `history`, and the run is logged and listed like any other (`assist` is the agent's `assistrun::Assist`, type-erased).
    #[allow(clippy::too_many_arguments)]
    fn chat_run_start(&self, _user: &str, _prompt: &str, _chat: Arc<dyn iyakki_shared::chat::ChatBackend>, _tools: Arc<dyn Tools>, _history: Vec<iyakki_shared::chat::Message>, _limits: iyakki_shared::agent::RunLimits, _assist: Box<dyn Any + Send>) -> Option<Box<dyn ChatRun>> {
        None
    }
    /// A chat run around a run started elsewhere (an Autopilot run, an external agent), logged and listed like any other.
    fn chat_run_from_handle(&self, _user: &str, _prompt: &str, _handle: iyakki_shared::agent::RunHandle) -> Option<Box<dyn ChatRun>> {
        None
    }
    /// Bluetooth tools (administrators only).
    fn wrap_bluetooth_tools(&self, inner: Arc<dyn Tools>, _env: &ChainEnv) -> Arc<dyn Tools> {
        inner
    }
}

/// What the wifi page of the Settings tools needs from the shell.
pub struct WifiParts {
    pub ctl: iyakki_shared::settingstools_wifi::WifiCtl,
    pub read: iyakki_shared::settingstools_wifi::SecretReader,
}

/// Where a saved Autopilot run stands (`.iyakki/autopilot.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApSaved {
    pub status: String,
    pub goal: String,
    pub step: usize,
    pub reason: String,
}

/// What an Autopilot run needs from the chat.
pub struct ApStart {
    pub root: PathBuf,
    pub features: PathBuf,
    pub goal: String,
    pub resume: bool,
    pub interjection: Option<String>,
    pub session: String,
    /// The PROJECT STATE block, rebuilt at every step.
    pub state: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// `autopilot.*` audit lines.
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    /// An inbox item when the run finishes or pauses (title, text).
    pub notify: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    pub executor: Option<iyakki_shared::autopilot::Executor>,
    /// Record the steps in the workspace history: (workspace name, chat session id); `None`: no history.
    pub history: Option<(String, Option<String>)>,
}

/// What the PDF tools need from the UI side.
pub struct PdfParts {
    pub ext: Option<String>,
    pub web: Arc<dyn iyakki_web::Fetch>,
    pub downloads: PathBuf,
}

/// The fake: every method keeps the tools as they are.
pub struct NoB4;
impl ServicesFacetB4 for NoB4 {}

/// Small stateless helpers of agent modules the UI called by path (`providererr`, `speechtime`, `touchprobe`, `vaultpending`, `voicepack`,
/// `datatools`, `upnp_serve`); the defaults say "nothing here".
pub trait HelpersB4: Send + Sync {
    /// A provider's raw error in plain words (`providererr::plain_error`).
    fn provider_plain_error(&self, _provider: &str, raw: &str) -> String {
        raw.to_string()
    }
    /// Where the server at `url` is, in words (`speechtime::locality(url).label()`): "on this computer", "on your network", "on the internet".
    fn locality_label(&self, _url: &str) -> &'static str {
        "on the internet"
    }
    /// The host and port of a server URL as the audit line writes it (`speechtime::server_of`).
    fn speech_server_of(&self, url: &str) -> String {
        url.to_string()
    }
    /// The USB touch panels the kernel lists: their names.
    fn touch_panel_names(&self, _sys: &std::path::Path) -> Vec<String> {
        Vec::new()
    }
    /// Probe the touch screen at the given level; the words for the person.
    fn touch_probe(&self, _sys: &std::path::Path, _level: u8) -> String {
        UNAVAILABLE_B4.into()
    }
    /// Reset the touch screen now; the words for the person.
    fn touch_reset_now(&self, _uptime_secs: u64) -> String {
        UNAVAILABLE_B4.into()
    }
    /// Passwords the cluster waits for until this vault is unlocked: (user, what).
    fn vault_pending_list(&self, _data: &std::path::Path) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Delete a local speech model of Lemonade.
    fn voicepack_delete_local_model(&self, _model: &str) -> Result<(), String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// The assistant's tab context line of this person (open data tabs), if any.
    fn data_tab_context(&self, _user: &str) -> Option<String> {
        None
    }
    /// Where the saved Autopilot run of a workspace stands (`None`: nothing saved).
    fn autopilot_saved(&self, _state_path: &std::path::Path) -> Option<ApSaved> {
        None
    }
    // the Publish flow of the AI IDE without a store handle
    fn pf_preflight(&self, _dir: &Path) -> Preflight {
        Preflight::default()
    }
    fn pf_header_hint(&self, _dir: &Path) -> Option<String> {
        None
    }
    fn tile_run_tests(&self, _dir: &Path) -> Result<TestReport, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// What the red sign says: the cameras that record or are watched for motion all the time.
    fn camera_sign_names(&self, _data: &std::path::Path) -> Vec<String> {
        Vec::new()
    }
    /// The click on the red sign: stop every recording and motion watch of `user`; how many stopped.
    fn camera_stop_all_signs(&self, _data: &std::path::Path, _user: &str) -> usize {
        0
    }
    /// The system prompt part for an AI IDE workspace (its board, commands and, when a cluster is connected, the cluster line).
    fn workspace_prompt(&self, _name: &str, _root: &std::path::Path, _features_file: &str, _cluster: Option<(&iyakki_shared::devcluster::ClusterProfile, &str)>) -> String {
        String::new()
    }
    /// The skills a workspace's stack pins for this request.
    fn project_pinned_skills(&self, _root: &std::path::Path, _stack_override: &str, _request: &str) -> Vec<String> {
        Vec::new()
    }
    /// Start the alarm sound as the mixer's alarm kind (heard over a radio or a film; with the mixer off it goes to the play command).
    fn calendar_ring(&self, _play_cmd: &str) -> Result<Box<dyn iyakki_shared::voicedesk::Playback>, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// The sound outputs the kernel lists now.
    fn sound_outputs(&self) -> Vec<iyakki_shared::audioout::Output> {
        Vec::new()
    }
    /// Seconds since the machine started (a huge value when unknown).
    fn uptime_secs(&self) -> u64 {
        u64::MAX / 2
    }
    /// DOCTOR: the slow reads (names, ports, the network), for a thread of their own.
    fn doctor_slow_probe(&self, _input: iyakki_shared::doctor_gather::SlowIn) -> iyakki_shared::doctor_gather::SlowOut {
        iyakki_shared::doctor_gather::SlowOut::default()
    }
    /// The languages the radio search offers.
    fn radio_languages(&self) -> &'static [iyakki_tile_api::radio::Language] {
        &[]
    }
    /// Switch the media server on for `user`.
    fn upnp_start_for(&self, _data_dir: &std::path::Path, _user: &str, _audit: AuditLineB4) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// Casting (the Shares page buttons, `iyakki_agent::upnp_cast_desk`): look for players on the local network; their names.
    fn cast_find(&self) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// The items of the person's media server to cast: (id, title, is video).
    fn cast_items(&self, _user: &str) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    /// Play item `item` of the media server on the player `name` (from `cast_find`).
    fn cast_play(&self, _user: &str, _name: &str, _item: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    fn cast_stop(&self, _name: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// `action` is `pause`, `resume`, `by` (seconds from now, may be negative) or `to` (a second from the start).
    fn cast_move(&self, _name: &str, _action: &str, _secs: i64) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// One line of progress and the position in seconds when the player said one.
    fn cast_progress(&self, _name: &str) -> Result<(String, Option<u64>), String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// The data science toolkit as the Setup page shows it: a code (`none`, `current`, `upgrade`, `broken`) and a sentence.
    fn datasci_state(&self, _data_dir: &std::path::Path) -> (String, String) {
        ("none".into(), String::new())
    }
    /// "Verify toolkit": the self-test again in the person's Linux apps sandbox with no network; the sentence it saw.
    fn datasci_verify(&self, _data_dir: &std::path::Path, _user: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// Who follows the media server for changes: the sentence and the rows (peer, service, seconds left).
    fn upnp_subscribers(&self, _user: &str) -> (String, Vec<(String, String, i64)>) {
        (String::new(), Vec::new())
    }
}

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B4: &str = "unavailable: this core does not serve this call";

/// The accessor.
pub trait AgentFacetsB4: Send + Sync {
    fn helpers_b4(&self) -> &dyn HelpersB4 {
        &NoB4
    }
}
impl HelpersB4 for NoB4 {}
impl AgentFacetsB4 for crate::FakeAgentApi {}

/// The SSH tunnels of one person: what Settings -> Network -> SSH tunnels asks (the real one is `iyakki_agent::sshtunnel_mgr::Manager`).
pub trait SshTunnels: Send + Sync {
    fn tunnels(&self) -> Vec<iyakki_shared::sshtunnel::Tunnel> {
        Vec::new()
    }
    fn get(&self, _id: &str) -> Option<iyakki_shared::sshtunnel::Tunnel> {
        None
    }
    fn rows(&self) -> Vec<iyakki_shared::sshtunnel_mgr::Row> {
        Vec::new()
    }
    fn save(&self, _t: iyakki_shared::sshtunnel::Tunnel, _new_key: bool) -> Result<iyakki_shared::sshtunnel::Tunnel, String> {
        Err(UNAVAILABLE_B4.into())
    }
    fn set_enabled(&self, _id: &str, _on: bool) -> Result<iyakki_shared::sshtunnel::Tunnel, String> {
        Err(UNAVAILABLE_B4.into())
    }
    fn retry(&self, _id: &str) {}
    fn remove(&self, _id: &str) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_B4.into())
    }
    fn trust(&self, _host: &str, _port: u16, _fingerprint: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B4.into())
    }
    /// Look at the tunnels once (the page does it every two seconds while it is open).
    fn tick(&self, _now: u64) {}
    /// Start watching the enabled tunnels.
    fn ensure_supervised(&self) {}
    /// Try the route of one tunnel now.
    fn test(&self, _id: &str, _now: u64) -> iyakki_shared::sshtunnel_mgr::TestReport {
        iyakki_shared::sshtunnel_mgr::TestReport { ok: false, lines: vec![UNAVAILABLE_B4.into()], untrusted: Vec::new() }
    }
    /// The vault key of this name (public line, fingerprint).
    fn key_info(&self, _id: &str) -> Option<iyakki_shared::gitkeys::KeyInfo> {
        None
    }
    /// The names of the keys the vault holds.
    fn key_ids(&self) -> Vec<String> {
        Vec::new()
    }
}
impl SshTunnels for NoB4 {}

/// The running voice assistant worker, as the Voice tile drives it.
pub trait MineWorker: Send + Sync {
    fn send(&self, cmd: iyakki_shared::voiceassist_ports::Cmd);
    fn status(&self) -> iyakki_shared::voiceassist::Status;
    /// The wake phrase was heard: start listening, with what was said after it.
    fn wake(&self, text: &str);
}

/// The connected cluster of a workspace, for the project tools.
pub struct ProjectCluster {
    pub runner: Arc<dyn iyakki_shared::wsexec::Runner>,
    pub profile: iyakki_shared::devcluster::ClusterProfile,
    pub namespace: String,
    /// What the registry calls this cluster (`cluster "gpu-box" (remote)`); empty: the profile's own name.
    pub phrase: String,
}

/// What the project tools of an AI IDE chat need.
pub struct ProjectKitSpec {
    pub user: String,
    pub name: String,
    pub root: PathBuf,
    pub features: PathBuf,
    /// `(what, detail)` goes to the audit log, `what` already prefixed (`k8s.`, `board.`).
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// The chat session whose id names the workspace history; `None`: no session yet.
    pub session: Option<String>,
    pub cluster: Option<ProjectCluster>,
}

/// The project tools of one chat run.
pub trait ProjectKit: Send + Sync {
    /// "k3s · namespace shop-dev" when a cluster is connected.
    fn cluster_line(&self) -> Option<String>;
    /// The project state block of the system prompt (stack, board, bugs, deploy, last session).
    fn state(&self, stack_override: &str, test: &str, run: &str, build: &str) -> String;
    /// The tools around `base`; `platform` is the agent's `PlatformTools` (type-erased), offered with the cluster tools.
    fn wrap(&self, base: Arc<dyn Tools>, platform: Option<Arc<dyn Any + Send + Sync>>) -> Arc<dyn Tools>;
}

/// One run of a chat as the chat drives it (the real one is the agent's `RunSession`).
pub trait ChatRun: Send {
    fn id(&self) -> String;
    fn is_done(&self) -> bool;
    /// New events since the last call.
    fn poll(&mut self) -> Vec<iyakki_shared::agent::RunEvent>;
    fn decide(&mut self, call_id: &str, v: iyakki_shared::agent::Verdict);
    fn cancel(&self);
    /// Pause an Autopilot run (it keeps its place); a plain run just stops.
    fn pause(&self);
    /// Marks the run as part of a chat.
    fn in_session(self: Box<Self>, session: &str) -> Box<dyn ChatRun>;
}
