//! Facets of bucket b1 of the UI/agent split (F-SPLIT-03, alpha.5): narrow, hand-written traits for the agent modules `aiprofile`, `avbuild`,
//! `fwextra_cli` and the others listed below, each with "unavailable"/empty defaults so a core (or a test) that serves one compiles and
//! behaves. The real implementations are in `iyakki-agent` (`facets_b1.rs`); the fakes here are the default impls or small recording structs.
//! [`AgentFacetsB1`] is a supertrait of `AgentApi`, so the UI reaches them as `cx.agent.ai_profile()` and so on.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_shared::aiprofile::Vis;

/// The answer of a call this core does not serve.
pub const B1_UNAVAILABLE: &str = "unavailable: this core does not serve this call";

// ------------------------------------------------------------------------------------------------ aiprofile

/// What the AI hardware profile of this machine says (the agent's `aiprofile::AiProfile`, kept behind this trait).
pub trait AiMachine: Send + Sync + std::fmt::Debug {
    /// "Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models; ..."
    fn summary(&self) -> String {
        String::new()
    }
    /// A GPU (or memory) big enough for local models was found.
    fn capable(&self) -> bool {
        false
    }
    /// The size of the best GPU could not be learned (nothing is hidden then).
    fn unknown(&self) -> bool {
        true
    }
    /// "fits: 7B Q4, 13B Q4" (what local models would suit this machine).
    fn fits_line(&self) -> String {
        String::new()
    }
    /// What the profile says about one install item (`item_id` is the item's id, e.g. "ollama"); `show_all` is the administrator's "Show all AI tools".
    fn visibility(&self, _item_id: &str, _show_all: bool) -> Vis {
        Vis::Show
    }
    /// The AI tools hidden on this profile, each with its reason: (item id, reason).
    fn hidden_items(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The tier of this machine in words ("small", "capable", ...), as the Settings page shows it next to the fits line.
    fn tier_words(&self) -> String {
        String::new()
    }
}

impl PartialEq for dyn AiMachine {
    fn eq(&self, other: &Self) -> bool {
        self.summary() == other.summary() && self.capable() == other.capable() && self.unknown() == other.unknown()
    }
}

/// A made-up machine for tests: canned answers.
#[derive(Clone, Debug, Default)]
pub struct FakeAiMachine {
    pub summary: String,
    pub capable: bool,
    pub unknown: bool,
    pub fits: String,
    /// (item id, visibility) rows; an item that is not listed is shown.
    pub vis: Vec<(String, Vis)>,
}

impl AiMachine for FakeAiMachine {
    fn summary(&self) -> String {
        self.summary.clone()
    }
    fn capable(&self) -> bool {
        self.capable
    }
    fn unknown(&self) -> bool {
        self.unknown
    }
    fn fits_line(&self) -> String {
        self.fits.clone()
    }
    fn visibility(&self, item_id: &str, show_all: bool) -> Vis {
        match self.vis.iter().find(|(i, _)| i == item_id).map(|(_, v)| v.clone()) {
            Some(Vis::Hidden(r)) if show_all => Vis::NotRecommended(r),
            Some(v) => v,
            None => Vis::Show,
        }
    }
    fn hidden_items(&self) -> Vec<(String, String)> {
        self.vis.iter().filter_map(|(i, v)| if let Vis::Hidden(r) = v { Some((i.clone(), r.clone())) } else { None }).collect()
    }
}

pub trait AiProfileFacet: Send + Sync {
    /// This machine's profile; `data` is the data folder (the Mac model hosts set up there are counted).
    fn machine(&self, _data: &Path) -> Arc<dyn AiMachine> {
        Arc::new(FakeAiMachine { unknown: true, ..Default::default() })
    }
    /// Can this machine run local models (the picture time estimates assume the CPU when not)?
    fn local_capable(&self) -> bool {
        false
    }
    /// This machine's profile without the Mac model hosts (cheap: the hardware is read once per run); the Settings page asks every frame.
    fn this_machine(&self) -> Arc<dyn AiMachine> {
        Arc::new(FakeAiMachine { unknown: true, ..Default::default() })
    }
}

// ------------------------------------------------------------------------------------------------ avbuild

pub trait AvBuildFacet: Send + Sync {
    /// The consent text for building the browser add-on here, from the map text (`versions.map`) and this image's version; `Err` when the map has no entry for it.
    fn consent(&self, _map_text: &str) -> Result<Vec<String>, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// What the tenant says about the add-on's decoders for this person (`media.codecs`): `None` when there is no tenant setting (the
    /// person's own choice stands), `Some(false)` when they are denied, `Some(true)` when allowed.
    fn codecs_policy(&self, _user: &str) -> Option<bool> {
        None
    }
}

// ------------------------------------------------------------------------------------------------ fwextra_cli

/// One way the hardware-firmware files can come in, as the card says it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HwOffer {
    pub ready: bool,
    pub needs_click: bool,
    pub line: String,
}

/// Everything the hardware-firmware Setup card shows before the click.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HwView {
    pub lines: Vec<String>,
    pub summary: String,
    /// (vendor group, licence class label)
    pub vendors: Vec<(String, String)>,
    pub files: usize,
    pub bytes: u64,
    pub has_nvidia: bool,
    pub offers: Vec<HwOffer>,
    /// The staging folder of the developer-mode link when it holds firmware files.
    pub staging: Option<PathBuf>,
    /// A stick or folder that looks like firmware.
    pub stick: Option<PathBuf>,
    /// The cable interface that could share the other computer's internet.
    pub link_iface: Option<String>,
}

/// What `HwFirmwareFacet::accept` recorded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HwAccepted {
    /// Distinct files in the accepted list.
    pub files: usize,
    /// The first 16 characters of the list's digest (the audit line).
    pub digest16: String,
}

pub trait HwFirmwareFacet: Send + Sync {
    /// Does the kernel log (and the requests kept earlier) ask for no firmware file?
    fn plan_is_empty(&self, _data: &Path) -> bool {
        true
    }
    /// The person accepted the licences: record exactly the list of files this computer asks for (plus `with`, e.g. "nvidia") and return what was recorded;
    /// `Err` when no file is missing or the record could not be written.
    fn accept(&self, _data: &Path, _with: &[String], _user: &str, _now: u64) -> Result<HwAccepted, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Read the kernel log, the devices and the network (blocks a few seconds: call it from a worker).
    fn view_now(&self, _data: &Path, _home: &Path) -> HwView {
        HwView::default()
    }
}

// ------------------------------------------------------------------------------------------------ contextintent

/// How to take a change of the project context back: the UI keeps it and hands it back; only the agent knows what is inside.
#[derive(Clone)]
pub struct ContextUndo(pub Arc<dyn std::any::Any + Send + Sync>);

impl std::fmt::Debug for ContextUndo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContextUndo")
    }
}

impl PartialEq for ContextUndo {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a project-context instruction did.
#[derive(Clone, Debug, Default)]
pub struct ContextDone {
    /// What to say in the chat, exactly what happened.
    pub reply: String,
    pub undo: Option<ContextUndo>,
    /// The entry the change was about (for "pin that" next).
    pub touched: Option<String>,
    /// Several entries fit: the ids offered, in the order of the numbered options in `reply`.
    pub choices: Vec<String>,
    /// Nothing was done and the message should go on to the model as it is (a credential).
    pub refused: bool,
}

pub trait ContextIntentFacet: Send + Sync {
    /// Is the text an instruction to change the project context?
    fn detect(&self, _text: &str) -> Option<iyakki_shared::contextintent::Detected> {
        None
    }
    fn apply(&self, _root: &Path, _person: &str, _d: &iyakki_shared::contextintent::Detected, _last: Option<&str>, _now: i64) -> ContextDone {
        ContextDone { reply: B1_UNAVAILABLE.into(), refused: false, ..Default::default() }
    }
    /// The numbered answer to "which one?" (`pick` 0: cancel).
    fn apply_choice(&self, _root: &Path, _person: &str, _d: &iyakki_shared::contextintent::Detected, _ids: &[String], _pick: usize, _now: i64) -> ContextDone {
        ContextDone { reply: B1_UNAVAILABLE.into(), ..Default::default() }
    }
    fn undo(&self, _root: &Path, _u: &ContextUndo) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

// ------------------------------------------------------------------------------------------------ browserheaders

/// What a secret header value is shown as.
pub const HEADER_MASK: &str = "********";

/// Where header values are kept: the person's vault in the shell, memory in tests.
pub trait HeaderValues: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, value: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// The custom request header rules of one person: a small file of everything but the values, and the values behind [`HeaderValues`].
pub trait HeaderRuleStore: Send + Sync {
    fn list(&self) -> Vec<iyakki_shared::browserheaders::HeaderRule> {
        Vec::new()
    }
    fn add(&self, _r: iyakki_shared::browserheaders::HeaderRule, _value: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn update(&self, _id: &str, _r: iyakki_shared::browserheaders::HeaderRule, _value: Option<&str>) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn set_enabled(&self, _id: &str, _on: bool) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn delete(&self, _id: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Make the enabled rules (with their values) the ones the browser's transport applies; how many enabled rules were left out (a locked vault, a bad rule).
    fn activate(&self) -> usize {
        0
    }
}

pub trait BrowserHeadersFacet: Send + Sync {
    /// The rules kept in the file `path`, the values behind `values`.
    fn open(&self, _path: &Path, _values: Arc<dyn HeaderValues>) -> Arc<dyn HeaderRuleStore> {
        Arc::new(NoRules)
    }
    /// Names (never values) of the active rules that apply to an address.
    fn active_here(&self, _url: &iyakki_web::Url) -> Vec<String> {
        Vec::new()
    }
    /// The function the transport wrapper asks for each request.
    fn provider(&self) -> iyakki_web::headerfetch::Provider {
        Arc::new(|_u: &iyakki_web::Url| Vec::new())
    }
}

/// A store with no rules, whatever is asked.
pub struct NoRules;
impl HeaderRuleStore for NoRules {}

// ------------------------------------------------------------------------------------------------ browser hardware

/// The hardware of the web browser a person is in (microphone, camera, clipboard, sound out), as the shell and the camera see it.
pub trait BrowserMediaHandle: Send + Sync {
    /// Ask the page to switch a source on (the person clicks Allow there).
    fn request(&self, _kind: iyakki_shared::browsermedia::Kind) -> iyakki_shared::browsermedia::Requested {
        iyakki_shared::browsermedia::Requested::Denied
    }
    fn is_on(&self, _kind: iyakki_shared::browsermedia::Kind) -> bool {
        false
    }
    /// The last camera frame (a JPEG).
    fn camera(&self) -> Option<Vec<u8>> {
        None
    }
    fn camera_frames(&self) -> u64 {
        0
    }
    /// The role or tenant policy lets the page switch the camera on.
    fn policy_camera(&self) -> bool {
        false
    }
    /// The sign's words while the browser's microphone or camera is on.
    fn indicator_text(&self) -> Option<String> {
        None
    }
    /// Switch the microphone and the camera off; true when one was on.
    fn stop_capture(&self) -> bool {
        false
    }
    /// What the connected browser supplies, for Cloud mode.
    fn cloud_sources(&self) -> iyakki_cloudmode::BrowserSources {
        Default::default()
    }
}

// ------------------------------------------------------------------------------------------------ wake phrase

/// A wake phrase listener running on its own thread; dropping it stops it and closes the microphone.
pub trait WakeHandle: Send {
    /// The microphone is open to the spotter right now.
    fn indicator(&self) -> bool {
        false
    }
    /// A wake phrase was heard since the last call.
    fn take_wake(&self) -> bool {
        false
    }
    /// Something else needs the microphone (the Voice tile) or must not hear (the lock screen).
    fn set_paused(&self, _paused: bool) {}
    fn note(&self) -> String {
        String::new()
    }
}

/// A listener made elsewhere (a test gives one with a fake microphone and spotter); only the agent knows what is inside.
pub struct WakeListenerBox(pub Box<dyn std::any::Any + Send>);

// ------------------------------------------------------------------------------------------------ ssh

/// The SSH client of this computer for one person: saved connections, trusted host keys, the person's keys. Created by `ServicesB1::ssh_host`.
pub trait SshHost: Send + Sync {
    fn connections(&self) -> Vec<iyakki_shared::ssh::Connection> {
        Vec::new()
    }
    /// Add a connection, or (with `replacing`) change the one called that.
    fn save_connection(&self, _c: iyakki_shared::ssh::Connection, _replacing: Option<&str>) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn remove_connection(&self, _name: &str) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// The host keys trusted for `host:port`.
    fn trusted_keys(&self, _host: &str, _port: u16) -> Vec<iyakki_shared::ssh::HostKey> {
        Vec::new()
    }
    fn trust(&self, _host: &str, _port: u16, _keys: &[iyakki_shared::ssh::HostKey]) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Forget the trusted keys of `host:port`; how many went.
    fn forget(&self, _host: &str, _port: u16) -> Result<usize, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Is an SSH client installed on this computer?
    fn has_client(&self) -> bool {
        false
    }
    /// Read the host keys `host:port` presents (network; call it from a worker); the error says how to get a client when there is none.
    fn probe(&self, _host: &str, _port: u16) -> Result<Vec<iyakki_shared::ssh::HostKey>, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Run `ssh args` for the person (a quick check or the saved command) and return its output; `sink` is told about output as it arrives.
    fn run(&self, _args: &[String], _cwd: &Path, _seconds: u64, _sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// A login on a pseudo-terminal of `cols` x `rows`: the link and what must be kept alive while the session runs.
    fn interactive(&self, _args: &[String], _cwd: &Path, _cols: usize, _rows: usize) -> Result<(Box<dyn iyakki_vt::Link>, Box<dyn std::any::Any + Send>), String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Make a key pair named `name` (a vault name `ssh.<x>`); `store` keeps the private key in the vault; the public key comes back.
    fn generate_key(&self, _name: &str, _comment: &str, _store: &dyn Fn(&str, &str) -> Result<(), String>) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// The public key of the vault key `name`, when it can be read.
    fn public_key(&self, _name: &str) -> Option<String> {
        None
    }
}

/// An SSH host with nothing in it.
pub struct NoSsh;
impl SshHost for NoSsh {}

// ------------------------------------------------------------------------------------------------ host speech engines

/// A speech engine of this computer that speaks by itself (espeak, say, ...): its text-to-sound stage is empty and its sink speaks.
pub trait DirectVoice: Send + Sync {
    /// The synth of this engine (it makes no sound file; the sink speaks the text).
    fn synth(&self) -> Arc<dyn iyakki_shared::story::Synth>;
    /// The sink that speaks; `voice` replaces the engine's own voice when given.
    fn sink(&self, voice: Option<&str>, lang: Option<&str>, speed: f32) -> Arc<dyn iyakki_shared::story::Sink>;
}

pub trait HostEnginesFacet: Send + Sync {
    /// The engine `engine` (id, program, voice of the setting) as a direct voice; `None` when it does not speak by itself or is not found here.
    fn direct(&self, _engine: &str, _program: Option<&str>, _voice: Option<&str>) -> Option<Arc<dyn DirectVoice>> {
        None
    }
    /// The Test button: say a sentence of `lang` with the engine; the plain words of the result.
    fn test_speak(&self, _engine: &str, _program: Option<&str>, _voice: Option<&str>, _lang: &str, _speed: f32, _play_cmd: &str) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

// ------------------------------------------------------------------------------------------------ story reader

/// Reads pieces aloud: while piece N plays, the next ones are made. All clones of the `Arc` are the same reader.
pub trait StoryReader: Send + Sync {
    /// Start reading `chunks` from piece `from`; `open`: more pieces follow through `append` (the story is still being written).
    fn start(&self, _chunks: Vec<iyakki_shared::story::Chunk>, _from: usize, _open: bool, _voice_key: &str, _synth: Arc<dyn iyakki_shared::story::Synth>, _sink: Arc<dyn iyakki_shared::story::Sink>) {}
    fn append(&self, _more: Vec<iyakki_shared::story::Chunk>) {}
    fn close_input(&self) {}
    fn pause(&self) {}
    fn resume(&self) {}
    fn stop(&self) {}
    fn seek(&self, _chunk: usize) {}
    fn is_active(&self) -> bool {
        false
    }
    fn announce_holder(&self) {}
    fn status(&self) -> iyakki_shared::story::Status {
        iyakki_shared::story::Status::Idle
    }
    fn snapshot(&self) -> iyakki_shared::story::Snapshot {
        iyakki_shared::story::Snapshot { status: iyakki_shared::story::Status::Idle, chunk: 0, unit: 0, range: (0, 0), chunks: 0, fraction: 0.0, ready: 0, word: None, word_exact: false, timing: Default::default() }
    }
    fn timeline_note(&self) -> String {
        String::new()
    }
    fn now_ms(&self) -> f32 {
        0.0
    }
    /// Replace the real clock (milliseconds): tests move time themselves.
    fn set_time_source(&self, _time: Arc<dyn Fn() -> f32 + Send + Sync>) {}
    fn set_words(&self, _on: bool) {}
    fn set_speed(&self, _speed: f32) {}
    fn level(&self) -> f32 {
        0.0
    }
    fn sounds(&self) -> (Vec<iyakki_shared::story::Chunk>, Vec<Option<Arc<Vec<u8>>>>) {
        (Vec::new(), Vec::new())
    }
}

/// A reader that reads nothing.
pub struct NoReader;
impl StoryReader for NoReader {}

pub trait StoryFacet: Send + Sync {
    fn new_reader(&self) -> Arc<dyn StoryReader> {
        Arc::new(NoReader)
    }
}

// ------------------------------------------------------------------------------------------------ scene (pictures from links in a reply)

/// A picture accepted from a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPicture {
    pub ext: String,
    pub bytes: Vec<u8>,
}

pub trait SceneFacet: Send + Sync {
    /// The writer's reply asks the person to fetch a picture: the scene (0-based) and the address. `None` for any other message.
    fn asks_for_link(&self, _text: &str, _scenes: usize) -> Option<(usize, String)> {
        None
    }
    /// The host part of an address, for the question and the audit line.
    fn host_of(&self, url: &str) -> String {
        url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("").to_string()
    }
    /// Fetch the picture at `url` (only after the person said yes to that address); `allow` is the account's own host rule.
    fn fetch_link(&self, _url: &str, _web: &dyn iyakki_web::Fetch, _allow: &dyn Fn(&str) -> bool, _allow_video: bool) -> Result<LinkedPicture, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

// ------------------------------------------------------------------------------------------------ k8s

/// A background feed of one namespace: a thread polls `kubectl` (read only) while the view is visible and keeps the last answer here.
pub trait KubeFeed: Send + Sync {
    fn set_visible(&self, _on: bool) {}
    fn latest(&self) -> Option<Arc<iyakki_shared::k8s::Snapshot>> {
        None
    }
    fn fetches(&self) -> u64 {
        0
    }
    /// Fetch now: a wait between retries ends at once.
    fn refresh(&self) {}
    fn status(&self) -> iyakki_shared::k8sretry::Status {
        iyakki_shared::k8sretry::Status::default()
    }
    fn set_target(&self, _t: iyakki_shared::k8s::Target) {}
    fn target(&self) -> iyakki_shared::k8s::Target {
        iyakki_shared::k8s::Target::default()
    }
    /// Follow the log of (pod, container); `None` stops following.
    fn follow(&self, _pod: Option<(String, String)>) {}
    fn logs(&self) -> Option<(String, Result<Vec<String>, String>)> {
        None
    }
    fn request_detail(&self, _pod: &str) {}
    fn detail(&self) -> Option<(String, Result<iyakki_shared::k8s::PodDetail, String>)> {
        None
    }
}

/// A feed that never has anything.
pub struct NoFeed;
impl KubeFeed for NoFeed {}

impl KubeFeed for iyakki_shared::k8s::Feed {
    fn set_visible(&self, on: bool) {
        iyakki_shared::k8s::Feed::set_visible(self, on)
    }
    fn latest(&self) -> Option<Arc<iyakki_shared::k8s::Snapshot>> {
        iyakki_shared::k8s::Feed::latest(self)
    }
    fn fetches(&self) -> u64 {
        iyakki_shared::k8s::Feed::fetches(self)
    }
    fn refresh(&self) {
        iyakki_shared::k8s::Feed::refresh(self)
    }
    fn status(&self) -> iyakki_shared::k8sretry::Status {
        iyakki_shared::k8s::Feed::status(self)
    }
    fn set_target(&self, t: iyakki_shared::k8s::Target) {
        iyakki_shared::k8s::Feed::set_target(self, t)
    }
    fn target(&self) -> iyakki_shared::k8s::Target {
        iyakki_shared::k8s::Feed::target(self)
    }
    fn follow(&self, pod: Option<(String, String)>) {
        iyakki_shared::k8s::Feed::follow(self, pod)
    }
    fn logs(&self) -> Option<(String, Result<Vec<String>, String>)> {
        iyakki_shared::k8s::Feed::logs(self)
    }
    fn request_detail(&self, pod: &str) {
        iyakki_shared::k8s::Feed::request_detail(self, pod)
    }
    fn detail(&self) -> Option<(String, Result<iyakki_shared::k8s::PodDetail, String>)> {
        iyakki_shared::k8s::Feed::detail(self)
    }
}

pub trait KubeFeedFacet: Send + Sync {
    /// Start a feed over `kube` for `target`; `interval` between snapshots, `log_interval` between log reads.
    fn start(&self, _kube: Arc<dyn iyakki_shared::k8s::Kube>, _target: iyakki_shared::k8s::Target, _interval: std::time::Duration, _log_interval: std::time::Duration) -> Box<dyn KubeFeed> {
        Box::new(NoFeed)
    }
}

// ------------------------------------------------------------------------------------------------ services-bound facets (avbuild, msg)

/// A messaging account as the Settings page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatrixAccount {
    pub id: String,
    pub label: String,
    /// `@name:server`, filled in by the sign-in.
    pub user_id: String,
    pub homeserver: String,
}

/// A room the person named.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatrixRoom {
    pub name: String,
    /// Copied into the INBOX while the account's copy is on?
    pub mirror: bool,
}

/// How to sign in to a homeserver.
#[derive(Clone)]
pub enum MatrixAuth {
    Password { user: String, password: String },
    Token(String),
}

/// The Matrix account of one person (the connector the assistant and the flows message through).
pub trait MatrixService: Send + Sync {
    fn accounts(&self) -> Vec<MatrixAccount> {
        Vec::new()
    }
    fn rooms(&self, _account: &str) -> Vec<MatrixRoom> {
        Vec::new()
    }
    fn mirror_on(&self, _account: &str) -> bool {
        false
    }
    fn set_mirror(&self, _account: &str, _on: bool) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn set_room_mirror(&self, _account: &str, _room: &str, _on: bool) -> Result<MatrixRoom, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Unread copied messages per saved room: (room name, count).
    fn unread_by_room(&self, _account: &str) -> Vec<(String, usize)> {
        Vec::new()
    }
    /// Sign in (network; call it from a worker) and keep the account; the token goes into the person's vault.
    fn add(&self, _label: &str, _homeserver: &str, _auth: MatrixAuth, _room_id: &str) -> Result<MatrixAccount, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Send a test message to the account's room; the plain words for the result.
    fn test_send(&self, _account: &str) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn remove(&self, _account: &str) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
}

/// A registry in which nobody holds anything and nothing can be stopped.
pub struct NoHolds;
impl iyakki_shared::soundhold::HoldRegistry for NoHolds {
    fn announce(&self, _key: &str, _holds: iyakki_shared::soundhold::Holds, _stop: iyakki_shared::soundhold::StopFn) {}
    fn forget(&self, _key: &str) {}
    fn holds(&self, _key: &str) -> Option<String> {
        None
    }
    fn find(&self, _wanter: &str, _outside: Option<&iyakki_shared::sounddoctor::Holder>, _ours: &dyn Fn(u32) -> bool) -> Option<iyakki_shared::soundhold::Found> {
        None
    }
    fn conflict_for_new_player(&self, _wanter: &str, _mixing: bool) -> Option<iyakki_shared::soundhold::Found> {
        None
    }
    fn stop(&self, _found: &iyakki_shared::soundhold::Found, _env: &dyn iyakki_shared::quiet::Env, _asound: &Path, _wait: std::time::Duration) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

/// A microphone that is never there.
pub struct NoMic;
impl iyakki_shared::wakelisten::Mic for NoMic {
    fn open(&mut self) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn is_open(&self) -> bool {
        false
    }
    fn drain(&mut self) -> Vec<i16> {
        Vec::new()
    }
    fn alive(&mut self) -> bool {
        false
    }
    fn close(&mut self) {}
}

/// A speaker that cannot speak.
pub struct NoSpeaker;
impl iyakki_shared::voiceassist::Speaker for NoSpeaker {
    fn say(&mut self, _text: &str) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    fn busy(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
    fn level(&self) -> f32 {
        0.0
    }
    fn louder(&mut self) {}
    fn quieter(&mut self) {}
}

/// A log that keeps nothing.
pub struct NoLog;
impl iyakki_shared::voiceassist::TurnLog for NoLog {
    fn record(&mut self, _who: iyakki_shared::voiceassist::Who, _text: &str) {}
}

/// A speech recogniser that hears nothing.
pub struct NoStt;
impl iyakki_shared::voiceassist::Stt for NoStt {
    fn hear(&mut self, _pcm: &[i16]) -> Result<iyakki_shared::voiceassist::Heard, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

/// Tools that offer nothing.
pub struct NoTools;
impl iyakki_shared::agent::Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        Vec::new()
    }
    fn call(&self, name: &str, _args: &serde_json::Value) -> Result<String, String> {
        Err(format!("unknown tool {name}"))
    }
    fn describe(&self, name: &str, _args: &serde_json::Value) -> String {
        name.to_string()
    }
}

/// A messaging service with no account that refuses everything.
pub struct NoMatrix;
impl MatrixService for NoMatrix {}

/// What needs the process's services (the data folder, the vault, the inbox). Implemented for `iyakki_agent::services::Services` (the methods take `self: &Arc<Self>`); the UI calls
/// the methods on `cx.services` with this trait in scope. Every method has an "unavailable" default.
pub trait ServicesB1 {
    /// At an administrator's sign-in: the inbox note when the browser add-on built here no longer matches this image.
    fn av_notify_stale(self: &Arc<Self>, _user: &str) {}
    /// The heartbeat of the messaging copy into the INBOX; true when something new was copied.
    fn msg_mirror_tick(self: &Arc<Self>, _user: &str, _audit: iyakki_shared::gitkeys::AuditFn) -> bool {
        false
    }
    /// The chat's tools for messaging (once an account exists), Web Push and passkeys, wrapped around `tools`.
    fn msg_offer_tools(self: &Arc<Self>, tools: Arc<dyn iyakki_shared::agent::Tools>, _user: &str, _audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        tools
    }
    /// The prompt text for those tools.
    fn msg_prompt_rules(self: &Arc<Self>, _user: &str) -> String {
        String::new()
    }
    /// The account's own tools (files, notes, tasks, radio, search ...): `request` is the person's message (for the tools that look at it), `cwd` the working folder.
    fn user_tools(self: &Arc<Self>, _user: &str, _policy: iyakki_sec::policy::Policy, _host: Arc<dyn iyakki_core_api::HostApi>, _mcp: Vec<iyakki_shared::mcp::ServerSpec>, _request: Option<&str>, _cwd: Option<&Path>) -> Arc<dyn iyakki_shared::agent::Tools> {
        Arc::new(NoTools)
    }
    /// The reading voice of Settings -> Voice as a speaker (the screen reader, My IYAKKI).
    fn system_speaker(self: &Arc<Self>) -> Box<dyn iyakki_shared::voiceassist::Speaker> {
        Box::new(NoSpeaker)
    }
    /// The thread log of the voice conversation of one person (what was said, as text).
    fn voice_turn_log(self: &Arc<Self>, _user: &str) -> Box<dyn iyakki_shared::voiceassist::TurnLog> {
        Box::new(NoLog)
    }
    /// Speech to text with the engine of Settings -> Voice.
    fn voice_stt(self: &Arc<Self>) -> Box<dyn iyakki_shared::voiceassist::Stt> {
        Box::new(NoStt)
    }
    /// The SSH client of `user` (saved connections, trusted keys, logins).
    fn ssh_host(self: &Arc<Self>, _user: &str) -> Arc<dyn SshHost> {
        Arc::new(NoSsh)
    }
    /// The chat's interactive SSH login tools (the list is free, a login asks every time) wrapped around `tools`.
    fn ssh_session_tools(self: &Arc<Self>, tools: Arc<dyn iyakki_shared::agent::Tools>, _user: &str, _allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>, _audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        tools
    }
    /// Save the wake phrase opt-in (turning it on needs a usable model); whether the saved value changed.
    fn wake_set_opt_in(self: &Arc<Self>, _on: bool) -> Result<bool, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Save the wake phrase sensitivity (0 low, 1 normal, 2 high).
    fn wake_set_sensitivity(self: &Arc<Self>, _sens: u8) -> Result<(), String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// Why the wake phrase has no usable model (`None`: there is one).
    fn wake_no_model_text(self: &Arc<Self>) -> Option<String> {
        Some(B1_UNAVAILABLE.into())
    }
    /// Start the listener (with the given one instead of the real microphone when `listener` is set); `Err` says why there is no model.
    fn wake_spawn(self: &Arc<Self>, _listener: Option<WakeListenerBox>) -> Result<Box<dyn WakeHandle>, String> {
        Err(B1_UNAVAILABLE.into())
    }
    /// The microphone of the Voice tile's capture path as a wake listener's port.
    fn wake_capture_mic(self: &Arc<Self>) -> Box<dyn iyakki_shared::wakelisten::Mic> {
        Box::new(NoMic)
    }
    fn matrix(self: &Arc<Self>, _user: &str, _audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn MatrixService> {
        Arc::new(NoMatrix)
    }
}

// ------------------------------------------------------------------------------------------------ the accessors

struct NoB1;
impl AiProfileFacet for NoB1 {}
impl AvBuildFacet for NoB1 {}
impl HwFirmwareFacet for NoB1 {}
impl ContextIntentFacet for NoB1 {}
impl BrowserHeadersFacet for NoB1 {}
impl KubeFeedFacet for NoB1 {}
impl SceneFacet for NoB1 {}
impl StoryFacet for NoB1 {}
impl HostEnginesFacet for NoB1 {}

/// The facets of bucket b1, one accessor each; every accessor has an "unavailable" default.
pub trait AgentFacetsB1: Send + Sync {
    fn ai_profile(&self) -> &dyn AiProfileFacet {
        &NoB1
    }
    fn av_build(&self) -> &dyn AvBuildFacet {
        &NoB1
    }
    fn hw_firmware(&self) -> &dyn HwFirmwareFacet {
        &NoB1
    }
    fn context_intent(&self) -> &dyn ContextIntentFacet {
        &NoB1
    }
    fn browser_headers(&self) -> &dyn BrowserHeadersFacet {
        &NoB1
    }
    fn kube_feed(&self) -> &dyn KubeFeedFacet {
        &NoB1
    }
    fn scene(&self) -> &dyn SceneFacet {
        &NoB1
    }
    fn story(&self) -> &dyn StoryFacet {
        &NoB1
    }
    /// The browser hardware of the web browser `user` is in, when they are in one.
    fn browser_media_for_user(&self, _user: &str) -> Option<Arc<dyn BrowserMediaHandle>> {
        None
    }
    /// A wake phrase was heard: ask for the My IYAKKI voice session (the Voice tile that opens takes it).
    fn wake_request_session(&self) {}
    /// The process-wide registry of the players that hold the sound card (the radio, the browser, the reader ...).
    fn sound_hold_registry(&self) -> Arc<dyn iyakki_shared::soundhold::HoldRegistry> {
        Arc::new(NoHolds)
    }
    fn host_engines(&self) -> &dyn HostEnginesFacet {
        &NoB1
    }
    /// The glyph the top bar shows for the microphone of My IYAKKI: the strongest of the running assistants.
    fn mic_glyph(&self) -> iyakki_shared::voiceassist::MicGlyph {
        iyakki_shared::voiceassist::MicGlyph::Off
    }
    /// Check a homeserver address (a cluster address is checked too) and return it in its clean form.
    fn matrix_check_homeserver(&self, _addr: &str) -> Result<String, String> {
        Err(B1_UNAVAILABLE.into())
    }
}

impl AgentFacetsB1 for crate::FakeAgentApi {}

/// A small recording fake for tests of the UI that wants to see what was asked.
#[derive(Default)]
pub struct RecordingB1 {
    pub calls: Mutex<Vec<String>>,
}
impl RecordingB1 {
    pub fn note(&self, s: &str) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(s.to_string());
    }
}
impl AvBuildFacet for RecordingB1 {
    fn consent(&self, map_text: &str) -> Result<Vec<String>, String> {
        self.note(&format!("av.consent {}", map_text.len()));
        Ok(vec!["consent".into()])
    }
}
