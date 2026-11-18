//! Hand-written facets of alpha.6 lane split-b (F-SPLIT-03): host probes the UI asked of the agent's `voice`, `voicedesk`, `scope`,
//! `webmix`, `webengine`, `cloudmode`, `audioout`, `audiosink`, `mediadl`, `btaudiotools` and `sharing::notice`. [`AgentFacetsS6B`] is a
//! supertrait of `AgentApi` (the accessor); the real implementation is `iyakki_agent::facets_s6b`, and [`NoS6B`] (the defaults) is the fake.

use std::path::{Path, PathBuf};

use iyakki_sec::policy::Policy;
use iyakki_shared::audioout::Output;
use iyakki_shared::mediaindex::ItemKind;
use iyakki_shared::voicedesk::T2Mic;
use iyakki_shared::webmix::TapState;

/// Sound probes: the loudness of a recording, the T2 microphone, the sound cards, the browser sound route.
pub trait SoundProbeFacet: Send + Sync {
    /// The loudest sample of a recording (0 for none).
    fn voice_peak(&self, _pcm: &[i16]) -> i16 {
        0
    }
    /// What the T2 microphone of this computer is doing (read from `proc_root`, `/proc` on a real machine).
    fn t2_mic(&self, _proc_root: &Path) -> T2Mic {
        T2Mic::Absent
    }
    /// The outputs the sound cards under `proc_root` offer.
    fn audio_outputs_at(&self, _proc_root: &Path) -> Vec<Output> {
        Vec::new()
    }
    /// The card number of a sound card that is still starting (None: none is).
    fn audio_card_starting(&self, _proc_root: &Path, _sys: &Path) -> Option<u32> {
        None
    }
    /// What holds the sound cards now (the lookup of who owns a stream runs here, in the agent).
    fn sounddoctor_scan(&self, _asound: &Path, _proc_root: &Path, _helper: u32, _own_pid: u32, _playing: bool) -> iyakki_shared::sounddoctor::Obs {
        iyakki_shared::sounddoctor::Obs { playing: false, engine: iyakki_shared::sounddoctor::Engine::Closed, holder: None }
    }
    fn sounddoctor_is_engine(&self, _proc_root: &Path, _helper: u32, _pid: u32) -> bool {
        false
    }
    /// The words of the "Browser sound" row for the settings and the tap state.
    fn web_mix_status_text(&self, _web_mix: bool, _mixer_on: bool, _tap: Option<TapState>) -> &'static str {
        "unavailable: this core does not serve this call"
    }
    /// Is the sound mixer the route sound takes now?
    fn audio_mixing(&self) -> bool {
        false
    }
    /// The configuration of the in-house player on this machine (None: no decoder program is installed).
    fn player_system_config(&self) -> Option<PlayerConfig> {
        None
    }
    /// Start playing a file with that configuration.
    fn player_start(&self, _cfg: &PlayerConfig, _file: &str, _start: f64, _area: (u32, u32)) -> Result<Arc<dyn MediaPlayer>, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    /// What plays and records sound on this machine (None: this core has none).
    fn system_players(&self) -> Option<Arc<dyn iyakki_shared::voicedesk::Players>> {
        None
    }
    fn system_recorders(&self) -> Option<Arc<dyn iyakki_shared::voicedesk::Recorders>> {
        None
    }
    /// Teach the Bluetooth speaker output which command plays sound.
    fn bluetooth_audio_install(&self, _play_cmd: Box<dyn Fn() -> String + Send + Sync>) {}
}

/// The web engine and the places the person's files live.
pub trait HostProbeFacet: Send + Sync {
    /// Why the web engine is not offered on this system (None: nothing to say).
    fn webengine_platform_note(&self) -> Option<&'static str> {
        None
    }
    /// Is a web engine offered (embedded or installed)?
    fn webengine_offered(&self) -> bool {
        false
    }
    /// The helper program that renders pages for the shell.
    fn webengine_embedded_helper(&self) -> Option<PathBuf> {
        None
    }
    /// The shell runs the engine as the host (not as a guest).
    fn webengine_set_host_mode(&self, _on: bool) {}
    /// The home folder to write into for a person (the data folder under test).
    fn home_root_for_writes(&self, _user: &str, data_dir: &Path) -> PathBuf {
        data_dir.join("home")
    }
    /// The folder a download of this kind goes to by default, within what the policy allows.
    fn media_planned_folder(&self, _folders: &[String], _policy: &Policy, _user: &str, _kind: Option<ItemKind>) -> Option<PathBuf> {
        None
    }
    /// The Gitea servers installed on the person's cluster, for "Connect Git".
    fn cluster_gitea(&self, _dir: &Path, _user: &str) -> Vec<iyakki_shared::gitconn::ClusterGitea> {
        Vec::new()
    }
    /// What this machine can do about power (read from the folders of `env`).
    fn power_caps(&self, _env: &iyakki_shared::power::PowerEnv) -> iyakki_shared::power::Caps {
        iyakki_shared::power::Caps::default()
    }
    fn power_current(&self, _env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::Profile> {
        None
    }
    fn power_set_profile(&self, _env: &iyakki_shared::power::PowerEnv, _p: iyakki_shared::power::Profile) -> Result<usize, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn power_apply_saved(&self, _env: &iyakki_shared::power::PowerEnv, _s: &iyakki_shared::power::PowerSettings) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_fan_card(&self, _env: &iyakki_shared::power::PowerEnv, _dir: &Path) -> Option<iyakki_shared::power::FanCard> {
        None
    }
    fn power_fan_toggle(&self, _env: &iyakki_shared::power::PowerEnv, _dir: &Path) -> Result<bool, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn power_lid_state(&self, _proc_root: &Path) -> Option<iyakki_shared::power::Lid> {
        None
    }
    fn power_external_display(&self, _sys: &Path) -> bool {
        false
    }
    /// The sharing backend of this machine (the files of the sharing service and `/sys`).
    fn sharing_backend(&self, _data: &Path) -> Option<Arc<dyn iyakki_shared::sharing::control::Backend>> {
        None
    }
    /// The shared folders of one person (the manager is made on first use and kept).
    fn shared_folders(&self, _data: &Path, _user: &str, _admin: bool, _policy: &iyakki_sec::policy::Policy, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn SharedFolderMgr> {
        Arc::new(NoS6B)
    }
    /// How many shares run now, in every manager.
    fn shares_running_total(&self) -> usize {
        0
    }
    /// Stop every running share; how many ran.
    fn shares_stop_everything(&self) -> usize {
        0
    }
    /// This machine's addresses on the local network, for the share links.
    fn lan_addresses(&self) -> Vec<String> {
        Vec::new()
    }
    /// A random string of hex digits (`bytes` bytes long).
    fn random_hex(&self, _bytes: usize) -> String {
        String::new()
    }
    /// Developer mode of the device whose data folder is given.
    fn devmode(&self, _data: &Path) -> Arc<dyn crate::DevModeApi> {
        Arc::new(crate::NoDev)
    }
    /// Is developer mode on in this data folder?
    fn devmode_is_on(&self, _data: &Path) -> bool {
        false
    }
    /// The update centre over a data folder.
    fn update_centre(&self, _data: &Path) -> Arc<dyn UpdateCentre> {
        Arc::new(NoS6B)
    }
    /// The kernel's boot id now.
    fn boot_id_now(&self) -> String {
        boot_id_none()
    }
    /// Is the share notice switched on in this data folder?
    fn sharing_notice_is_on(&self, _data: &Path) -> bool {
        false
    }
}

use std::sync::Arc;

use iyakki_flowsx::spec::SharedOpen;
use iyakki_shared::agent::Tools;
use iyakki_shared::browserconsent::{Answer, ConsentRequest};

/// The FLOWS environment of one person as the CHAT tile sees it (the engine and the store stay in the agent): the rules and the picture
/// of what is open for the system prompt, the newest failure, the flow tools around a tool set.
pub trait FlowChatEnv: Send + Sync {
    /// The rules of working on workflows (a system prompt part).
    fn prompt_rules(&self) -> &'static str {
        ""
    }
    /// A compact, redacted picture of what is open (flow, run, node) with the newest failure.
    fn context(&self, _open: &SharedOpen) -> String {
        String::new()
    }
    /// The name of a draft flow.
    fn draft_name(&self, _flow: &str) -> Option<String> {
        None
    }
    /// The newest failed run of a flow.
    fn newest_failed_run(&self, _flow: &str) -> Option<String> {
        None
    }
    /// Did the last run of this flow fail?
    fn last_run_failed(&self, _flow: &str) -> bool {
        false
    }
    /// The flow last open (id, name).
    fn last_flow(&self) -> Option<(String, String)> {
        None
    }
    /// The flow tools (flow_get, flow_edit, ...) around `inner`.
    fn wrap_tools(&self, inner: Arc<dyn Tools>, _open: SharedOpen) -> Arc<dyn Tools> {
        inner
    }
}

/// The FLOWS engine for the people of this machine: open a person's environment, answer a flow's permission request, start the flows
/// that run for a person at sign-in.
pub trait FlowsFacet: Send + Sync {
    /// The environment of a person (None-like error: this core has no flows).
    fn open(&self, _user: &str, _policy: &Policy) -> Result<Arc<dyn FlowChatEnv>, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    /// The person's answer to a permission request of a flow: carried out by the flows engine.
    fn answer_permission(&self, _user: &str, _policy: &Policy, _req: &ConsentRequest, _answer: Answer) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    /// Remember the flow a person last had open (the chat's sticky chip reads it).
    fn set_last_flow(&self, _user: &str, _flow: &str, _name: &str) {}
    /// A person signed in: start their flows' triggers. Whether anything started.
    fn start_for_login(&self, _user: &str, _policy: &Policy) -> bool {
        false
    }
}

impl FlowsFacet for NoS6B {}

/// The voice desk over the person's settings and providers: the voices to read with, the engine that listens, where text and sound go,
/// and the reading itself.
pub trait VoiceDeskFacet: Send + Sync {
    /// The reading voices that can be used now.
    fn read_voices(&self) -> Vec<iyakki_shared::voicedesk::ReadVoice> {
        Vec::new()
    }
    /// The engine that turns the microphone's sound into text (None: none is set).
    fn stt_engine(&self) -> Option<iyakki_shared::voice::SpeechEngine> {
        None
    }
    /// The line that says where the text or sound goes.
    fn where_text(&self, _engine: &iyakki_shared::voice::SpeechEngine, _what: &str) -> String {
        String::new()
    }
    /// The card shown before text or sound goes to a machine the person has not agreed to yet (None: nothing to ask).
    fn consent_text(&self, _engine: &iyakki_shared::voice::SpeechEngine, _listening: bool) -> Option<(iyakki_shared::voicedesk::Dest, String)> {
        None
    }
    /// Sound to text with the engine.
    fn transcribe(&self, _engine: &iyakki_shared::voice::SpeechEngine, _pcm: &[i16]) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    /// The pieces to read: one per sentence.
    fn pieces(&self, _text: &str) -> Vec<iyakki_shared::story::Chunk> {
        Vec::new()
    }
    /// Plays the sound of the pieces with the play command (a child process per piece).
    fn command_sink(&self, _play_cmd: String) -> Option<Arc<dyn iyakki_shared::story::Sink>> {
        None
    }
    /// Makes the sound of one piece with the chosen voice and speed.
    fn choice_synth(&self, _engine: iyakki_shared::voice::SpeechEngine, _opts: iyakki_shared::voice::SpeakOpts) -> Option<Arc<dyn iyakki_shared::story::Synth>> {
        None
    }
}

impl VoiceDeskFacet for NoS6B {}

/// What a run with an external coding agent needs, as plain data: the agent is named by its id (the core looks it up in the person's
/// settings), everything else is what the caller chose for this one run.
pub struct ExtRunSpec {
    pub agent: String,
    pub workspace: PathBuf,
    pub prompt: String,
    /// The PROJECT STATE, skills and rules; built on the run's own thread.
    pub system: Arc<dyn Fn() -> String + Send + Sync>,
    pub model: String,
    /// The vendor's session id from the last message of this chat.
    pub resume: Option<String>,
    pub tools: Arc<dyn Tools>,
    pub limits: iyakki_shared::agent::RunLimits,
    pub allow_auto: bool,
    pub key: Option<zeroize::Zeroizing<String>>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// IYAKKI's own program, started by the agent as the MCP server.
    pub exe: PathBuf,
    pub on_session: Arc<dyn Fn(&str) + Send + Sync>,
    /// (agent label, model, input tokens, output tokens) as the vendor reported them.
    pub on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
    /// Story content only: every tool request is denied and the IDE rules are not added.
    pub no_tools: bool,
}

/// The external coding agents (Claude Code, GitHub Copilot, custom ones): the list of them and the start of a run.
pub trait ExtAgentFacet: Send + Sync {
    /// The agents as the chooser lists them, with their state.
    fn entries(&self) -> Vec<iyakki_extagentx::Entry> {
        Vec::new()
    }
    /// Start a run; the events arrive on the handle (a run that cannot start ends with a failure event).
    fn start(&self, _spec: ExtRunSpec) -> iyakki_shared::agent::RunHandle {
        let (tx, rx) = std::sync::mpsc::channel();
        let (dtx, _drx) = std::sync::mpsc::channel();
        let _ = tx.send(iyakki_shared::agent::RunEvent::Failed { error: "unavailable: this core does not serve this call".into() });
        iyakki_shared::agent::RunHandle::from_parts(rx, dtx)
    }
}

impl ExtAgentFacet for NoS6B {}

/// How far an update pack can be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateTrust {
    /// The signature matches the release key.
    Signed,
    /// No signature (or this build has no release key to check it with).
    Unsigned,
    /// A signature that does not match: never installed.
    Bad(String),
}

/// One update pack found on a drive or in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFound {
    pub dir: PathBuf,
    pub version: String,
    pub variant: String,
    pub trust: UpdateTrust,
}

/// The versions on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateVersions {
    /// The version running now, and the version whose files are on the boot partition (differs after an update, before the restart).
    pub running: String,
    pub on_disk: String,
    pub variant: String,
    pub previous: Option<String>,
    /// A sentence when an update was interrupted.
    pub interrupted: Option<String>,
}

/// What installing a pack would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePreview {
    pub from: String,
    pub to: String,
    pub variant: String,
    /// Reasons that need the person's yes: other variant, older version, no signature.
    pub confirm: Vec<String>,
    /// Plain lines for the page.
    pub lines: Vec<String>,
}

/// The system update centre of this device: where the packs are, what installing one does, install and roll back.
pub trait UpdateCentre: Send + Sync {
    /// The data folder this centre works on.
    fn data(&self) -> PathBuf {
        PathBuf::new()
    }
    fn versions(&self) -> UpdateVersions {
        UpdateVersions { running: String::new(), on_disk: String::new(), variant: String::new(), previous: None, interrupted: None }
    }
    /// The packs under `root` (signed ones checked against the release key), newest first.
    fn find_packs(&self, _root: &Path) -> Vec<UpdateFound> {
        Vec::new()
    }
    /// Open a signed update file (a bundle) into the staging folder; the pack is listed by `find_packs` afterwards.
    fn open_file(&self, _path: &Path) -> Result<UpdateFound, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    fn preview(&self, _pack: &UpdateFound) -> Result<UpdatePreview, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    fn install(&self, _pack: &UpdateFound, _confirmed: bool, _who: &str) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    fn rollback(&self, _who: &str) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    /// A sentence about how the last start went after an update (None: nothing to say).
    fn after_boot(&self, _running: &str, _boot_id: &str) -> Option<String> {
        None
    }
}

/// The kernel's boot id now.
pub fn boot_id_none() -> String {
    String::new()
}

impl UpdateCentre for NoS6B {}

/// The domain names of this device (Settings -> Domains): the routes, the listener, the certificates.
pub trait DomainsMgr: Send + Sync {
    fn settings(&self) -> iyakki_shared::domains::ListenerCfg {
        iyakki_shared::domains::ListenerCfg::default()
    }
    fn set_listener(&self, _c: iyakki_shared::domains::ListenerChange) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn status(&self) -> iyakki_shared::domains::Status {
        iyakki_shared::domains::Status { enabled: false, running: false, bind: String::new(), ports: Vec::new(), firewall: false, firewall_until: 0, firewall_ports: Vec::new(), names: Vec::new(), last_errors: Vec::new(), refused: Vec::new() }
    }
    fn routes(&self) -> Vec<iyakki_shared::cloudroute::Route> {
        Vec::new()
    }
    fn save_route(&self, _replace: Option<(&str, &str)>, _route: iyakki_shared::cloudroute::Route, _namespace: Option<&str>) -> Result<(), String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn remove_route(&self, _host: &str, _path: &str) -> Result<iyakki_shared::cloudroute::Route, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    /// One look at the certificates and the listener; the lines to tell the person.
    fn tick(&self) -> Vec<String> {
        Vec::new()
    }
    fn namespace_of(&self, _r: &iyakki_shared::cloudroute::Route) -> Option<String> {
        None
    }
    fn make_trial_cert(&self, _host: &str) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn certs(&self) -> Result<Vec<iyakki_shared::domains::CertRow>, String> {
        Ok(Vec::new())
    }
    fn import_cert(&self, _name: &str, _host: &str, _chain_pem: &str, _key_pem: &str) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn cert_state(&self, _r: &iyakki_shared::cloudroute::Route) -> Option<String> {
        None
    }
    fn tunnel_choices(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn own(&self) -> Vec<iyakki_shared::domains::Own> {
        Vec::new()
    }
    fn discover(&self) -> Result<Vec<iyakki_shared::domains::Found>, String> {
        Ok(Vec::new())
    }
}

impl DomainsMgr for NoS6B {}

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_S6B: &str = "unavailable: this core does not serve this call";

/// A video or audio file playing (the in-house player over the decoder program); polled by the view.
pub trait MediaPlayer: Send + Sync {
    fn info(&self) -> iyakki_shared::player::Info;
    fn status(&self) -> iyakki_shared::player::Status;
    /// The newest picture the view has not seen (`seen` is the number of the last one it drew).
    fn frame(&self, seen: u64) -> Option<Arc<iyakki_shared::player::Frame>>;
    fn pause(&self);
    fn resume(&self);
    fn toggle_pause(&self);
    fn seek_to(&self, secs: f64);
    fn seek_by(&self, delta: f64);
    fn set_area(&self, area: (u32, u32));
    fn stop(&self);
}

/// How the player starts its processes, as an opaque value only the real core makes and reads.
pub type PlayerConfig = Arc<dyn std::any::Any + Send + Sync>;

/// A sign-in that runs (the vendor's login program in a terminal of its own), polled for what it shows.
pub trait LoginHandle: Send + Sync {
    fn snapshot(&self) -> iyakki_extagentx::login::Snapshot;
    fn cancel(&self);
    /// Type a line (the code a browser showed) into the program.
    fn send_line(&self, text: &str);
}

/// Signing in to the external coding agents from the screen, and the phone relay that carries the code from a phone to this device.
pub trait ExtLoginFacet: Send + Sync {
    /// The home folder the vendor's program signs in under for this person.
    fn login_home(&self, _user: &str, data: &Path) -> PathBuf {
        data.to_path_buf()
    }
    /// Start the program; the handle tells how it goes.
    fn start(&self, _spec: &iyakki_extagentx::login::Spec) -> Option<Arc<dyn LoginHandle>> {
        None
    }
    /// Sign out of Claude Code (Copilot signs out inside its own program).
    fn logout(&self, _agent: &str, _program: &str, _home: &Path) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    /// Open the phone relay's page for one sign-in; the token to put in its address.
    fn relay_open(&self, _label: &str) -> String {
        String::new()
    }
    /// The port (and whether it is https) the web front end serves the relay on (None: it cannot be served here).
    fn relay_port(&self) -> Option<(u16, bool)> {
        None
    }
    fn relay_take_submitted(&self) -> Option<String> {
        None
    }
    fn relay_rearm(&self) {}
    fn relay_finish(&self) {}
}

impl ExtLoginFacet for NoS6B {}

/// The shared folders (a link to a folder of this device) of one person.
pub trait SharedFolderMgr: Send + Sync {
    fn create(&self, _d: &iyakki_shared::sharedfolder::Draft) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn create_package_drop(&self, _folder: &Path, _password: &str) -> Result<String, String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn update(&self, _id: &str, _d: &iyakki_shared::sharedfolder::Draft) -> Result<(), String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn delete(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn start(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn stop(&self, _id: &str) {}
    fn new_token(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_S6B.into())
    }
    fn tick(&self) {}
    fn list(&self) -> Vec<iyakki_shared::sharedfolder::ShareInfo> {
        Vec::new()
    }
    fn get(&self, _id: &str) -> Option<iyakki_shared::sharedfolder::ShareCfg> {
        None
    }
}

impl SharedFolderMgr for NoS6B {}

/// The fake for every facet of this file: all defaults.
pub struct NoS6B;
impl SoundProbeFacet for NoS6B {}
impl HostProbeFacet for NoS6B {}

/// The accessors, one per facet. Every accessor answers with the fake by default.
pub trait AgentFacetsS6B: Send + Sync {
    fn sound_probe(&self) -> &dyn SoundProbeFacet {
        &NoS6B
    }
    fn host_probe(&self) -> &dyn HostProbeFacet {
        &NoS6B
    }
    /// Signing in to the external coding agents.
    fn ext_login(&self) -> &dyn ExtLoginFacet {
        &NoS6B
    }
}

impl AgentFacetsS6B for crate::FakeAgentApi {}

// The handle of the browser's media session over the real session (moved here with the module: the trait is this crate's).
impl crate::BrowserMediaHandle for iyakki_shared::browsermedia::BrowserMedia {
    fn request(&self, kind: iyakki_shared::browsermedia::Kind) -> iyakki_shared::browsermedia::Requested {
        iyakki_shared::browsermedia::BrowserMedia::request(self, kind)
    }
    fn is_on(&self, kind: iyakki_shared::browsermedia::Kind) -> bool {
        iyakki_shared::browsermedia::BrowserMedia::is_on(self, kind)
    }
    fn camera(&self) -> Option<Vec<u8>> {
        iyakki_shared::browsermedia::BrowserMedia::camera(self)
    }
    fn camera_frames(&self) -> u64 {
        iyakki_shared::browsermedia::BrowserMedia::camera_frames(self)
    }
    fn policy_camera(&self) -> bool {
        self.policy().camera
    }
    fn indicator_text(&self) -> Option<String> {
        iyakki_shared::browsermedia::BrowserMedia::indicator_text(self)
    }
    fn stop_capture(&self) -> bool {
        iyakki_shared::browsermedia::BrowserMedia::stop_capture(self)
    }
    fn cloud_sources(&self) -> iyakki_cloudmode::BrowserSources {
        self.sources().cloud()
    }
}
