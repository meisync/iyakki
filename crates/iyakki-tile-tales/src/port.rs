//! What the TALES tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile names no `Services`, no session, no audit log and no facet: it asks a [`TalesPort`] for the signed-in person and what their
//! account allows, the story library and the PDF writer, the chat model and the picture models, the speech engines and the external coding
//! agents, and for one [`TalesWorker`] to hand to the threads it starts (a story being written, a picture being drawn, a link being fetched).
//! The real port is in the shell (`crates/iyakki-apps/src/apps/story_port.rs`), the only place that reads the session and calls the services;
//! a fake port in `fake_tests.rs` hands the tile in-memory answers.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_chat_model::ChatBackend;
use iyakki_chatview::model_picker::{Current, ModelList, TopRow};
use iyakki_provider_kind::lemonade::Model as LemonadeModel;
use iyakki_provider_kind::{ImageParams, ImageTrace, PullStatus};
use iyakki_story_kit::storyillus::Cand;
use iyakki_story_kit::{QuickEngine, Sink, Story, StoryOptions, StoryPdf, StoryStore, StoredPic, Synth};
use iyakki_story_model::{DirectVoice, LinkedPicture, ModelChoice};
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::R;
use iyakki_voice_model::hosttts::HostTtsApi;
use iyakki_voice_model::voice::{SpeakOpts, SpeechEngine, VoiceSettings};

/// The answer to one picture request of the image backends: the provider, the model, the bytes and the file kind, or why not; the trace of
/// what was sent.
pub type ImageOut = (Result<(String, String, Vec<u8>, &'static str), String>, ImageTrace);

/// A picture fetched from an address the person approved: the bytes, the file extension and whether it is a clip.
pub type LinkedPic = LinkedPicture;

/// What one run of an external coding agent needs, as plain data (the shell turns it into the agent's own run request: no tools, one step,
/// 15 minutes at most).
#[derive(Clone)]
pub struct AgentRunReq {
    /// The agent's id (`claude-code`, `copilot`, `custom:<id>`).
    pub agent: String,
    pub workspace: PathBuf,
    pub prompt: String,
    pub system: String,
    pub model: String,
    pub key: Option<Arc<zeroize::Zeroizing<String>>>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    pub on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
    pub exe: PathBuf,
}

/// An external coding agent as the chooser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEntry {
    pub id: String,
    pub label: String,
    pub chip: String,
    pub ready: bool,
}

/// What the tile's threads may ask (cloned into a thread; no shell context crosses). One per tile, from [`TalesPort::worker`].
pub trait TalesWorker: Send + Sync {
    fn provider_is_lemonade(&self, provider: &str) -> bool;
    /// One Lemonade model as the server lists it now (blocks).
    fn lemonade_live_model(&self, provider: &str, model: &str) -> Result<Option<LemonadeModel>, String>;
    /// The server's health and the models it holds (blocks).
    fn lemonade_health(&self, provider: &str) -> Result<(String, Vec<String>), String>;
    /// Draw one picture with the named (or first found) picture model (blocks).
    fn generate_image(&self, model: Option<(&str, &str)>, prompt: &str, size: &str, params: &ImageParams) -> ImageOut;
    /// Start the run of an external coding agent; the events arrive on the feed.
    fn start_agent(&self, req: AgentRunReq) -> Box<dyn iyakki_story_kit::RunFeed>;
    /// Fetch the picture at `url` through the person's own host rules (blocks).
    fn fetch_link(&self, url: &str, allow_video: bool) -> Result<LinkedPic, String>;
}

pub trait TalesPort {
    // ---- the person and their account
    /// The person manages the system (administrators): may set up reading aloud for everyone.
    fn is_admin(&self) -> bool;
    /// The account may run commands in full (external coding agents are allowed).
    fn may_run_commands(&self) -> bool;
    /// The account may use voice.
    fn voice_allowed(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// The audit line as a function a worker may keep (always for the signed-in person).
    fn audit_fn(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync>;
    /// A model call of this person counted in the usage log, as a function a worker may keep.
    fn usage_fn(&self) -> Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>;
    /// The clock, seconds since 1970 (what a story is saved under, the date of a PDF, the stamps of the picture log); a function, so a
    /// worker thread keeps a copy and a test gives a manual clock.
    fn clock(&self) -> Arc<dyn Fn() -> u64 + Send + Sync>;
    /// The data folder of this install.
    fn data_dir(&self) -> PathBuf;
    /// A failure message reworded when the drive it names is gone (the core knows which drives are there), as a function the tile keeps.
    fn rewording(&self) -> Arc<dyn Fn(&str) -> String + Send + Sync>;
    /// The machine's memory now (what a picture model needs is compared with it).
    fn memory(&self) -> Option<iyakki_sysinfo_model::Memory>;

    // ---- the library, the PDF and the links
    /// The person's story library.
    fn story_store(&self) -> StoryStore;
    /// The story as a PDF (A4), dated from `now` (seconds since 1970).
    fn story_pdf(&self, story: &Story, opts: &StoryOptions, pics: &[StoredPic], now: i64) -> Result<StoryPdf, String>;
    fn asks_for_pdf(&self, text: &str) -> bool;
    /// A sentence that gives a picture link for a scene: the scene (0 based) and the address.
    fn asks_for_link(&self, text: &str, scenes: usize) -> Option<(usize, String)>;
    /// A new story reader (it reads pieces aloud while the next ones are made).
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader>;

    // ---- the chat model and the settings the tile reads
    /// The chat model of the shell (what "same as chat" means when no choice is set).
    fn chat(&self) -> Arc<dyn ChatBackend>;
    fn chat_backend_for(&self, choice: ModelChoice) -> Arc<dyn ChatBackend>;
    /// The chat model the person chose in Settings.
    fn chat_choice(&self) -> Option<ModelChoice>;
    /// Every (provider, model) of the enabled providers.
    fn text_models(&self) -> Vec<(String, String)>;
    /// The speech engines Settings could be set to with one click, from the providers and what is installed (`path`, `voice_dirs`).
    fn quick_engines(&self, path: &str, voice_dirs: &[PathBuf]) -> Vec<QuickEngine>;
    /// Keep this speech engine in the voice settings.
    fn set_speech_engine(&self, engine: SpeechEngine) -> Result<(), String>;
    fn voice_settings(&self) -> VoiceSettings;
    /// The model chooser's list (the shared model picker) and what it needs to draw and to remember a typed model.
    fn model_list(&self, plain: Vec<(String, String)>, top: Option<TopRow>, agents: bool, current: Current, title: &str, hint: &str) -> Option<ModelList>;
    fn model_draw(&self, g: &mut Gfx, area: R, scale: f32, list: &mut ModelList);
    fn model_remember_custom(&self, agent: &str, model: &str);

    // ---- external coding agents
    /// The agent as the chooser lists it, with its state.
    fn agent_entry(&self, id: &str) -> Option<AgentEntry>;
    /// The agent is set up (Claude Code, Copilot or a configured custom one).
    fn agent_known(&self, id: &str) -> bool;
    /// The name the person knows the agent by.
    fn agent_label(&self, id: &str) -> Option<String>;
    /// The agent runs as an advisor here (read-only, text only).
    fn agent_advisor(&self, id: &str) -> bool;
    /// The agent can be used now: `Err` is what to fix. When nothing is known yet, a look at the agents starts in the background.
    fn agent_check(&self, id: &str) -> Result<(), String>;
    /// The name of a model of this agent as the chooser shows it (the id when unknown).
    fn agent_model_label(&self, id: &str, model: &str) -> String;
    /// The vault reference of the agent's own key (empty: none).
    fn agent_key_ref(&self, id: &str) -> String;
    /// The secret named in a vault reference.
    fn resolve_secret(&self, key_ref: &str) -> Result<zeroize::Zeroizing<String>, String>;
    /// An external agent used as a plain text model for pictures (one request in, one text out).
    fn scene_agent_chat(&self, req: &AgentRunReq) -> Option<Arc<dyn ChatBackend>>;

    // ---- picture models
    fn image_models(&self) -> Vec<(String, String)>;
    fn picture_choices(&self, found: &[(String, String)]) -> Vec<Cand>;
    /// The machine cannot run a picture model on a GPU (times are long).
    fn cpu_only(&self) -> bool;
    fn gpu_lemonade(&self) -> Option<String>;
    fn lemonade_images_missing(&self) -> Vec<LemonadeModel>;
    fn lemonade_model(&self, provider: &str, model: &str) -> Option<LemonadeModel>;
    fn provider_is_lemonade(&self, provider: &str) -> bool;
    fn pull_status(&self) -> Vec<PullStatus>;
    /// Ask the server to download a picture model for this person.
    fn pull_model(&self, provider: &str, model: &str) -> Result<(), String>;

    // ---- speech
    fn host_tts(&self) -> Arc<dyn HostTtsApi>;
    /// The engine of this computer that speaks by itself, when `engine` is one.
    fn direct_voice(&self, engine: &SpeechEngine) -> Option<Arc<dyn DirectVoice>>;
    /// The sink that hands a piece of text to a player command (the configured one, or the one found on this computer).
    fn player_sink(&self, configured: &str, path: &str) -> Arc<dyn Sink>;
    /// The synthesizer that reads a story with these options.
    fn reading_synth(&self, opts: SpeakOpts) -> Arc<dyn Synth>;
    /// Does this engine speak by itself (a sink that says the text, no sound file)?
    fn speaks_directly(&self, engine: &SpeechEngine) -> bool;
    /// "Save audio" works with this engine.
    fn can_save_audio(&self, engine: &SpeechEngine) -> bool;

    // ---- threads
    /// What the threads the tile starts may ask.
    fn worker(&self) -> Arc<dyn TalesWorker>;
}
