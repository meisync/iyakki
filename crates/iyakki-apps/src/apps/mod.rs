//! The apps behind the launcher tiles. Each app gets the signed-in user's [`Session`] (and so
//! their policy) through [`AppCx`]; none of them can reach beyond what that policy allows.

use std::sync::{Arc, Mutex};

use iyakki_shared::chat::ChatBackend;
use iyakki_shared::aiconfig::AiConfig;
use iyakki_shared::mcp::ServerSpec;
use iyakki_core_api::HostApi;

use crate::accounts::{AuditLog, Session, UserStore};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, R, TEXT};

/// The tiles on the home grid, as (layout key, label, the group the tile is in when left alone), for Settings -> Home layout. The tile
/// table belongs to the shell; it registers this function once at start (an app never depends on the shell).
pub type LayoutTilesFn = fn(bool, &iyakki_cloudmode::CloudMode) -> Vec<(String, &'static str, &'static str)>;
static LAYOUT_TILES: std::sync::OnceLock<LayoutTilesFn> = std::sync::OnceLock::new();

/// The apps a person may pin to the dock, as (app id, tile label), in tile order: the shell's tile table filtered by the person's role.
pub type PinnableFn = fn(&iyakki_sec::policy::Policy) -> Vec<(&'static str, &'static str)>;
static PINNABLE: std::sync::OnceLock<PinnableFn> = std::sync::OnceLock::new();

pub fn set_pinnable(f: PinnableFn) {
    let _ = PINNABLE.set(f);
}

pub fn pinnable(policy: &iyakki_sec::policy::Policy) -> Vec<(&'static str, &'static str)> {
    if let Some(f) = PINNABLE.get() {
        return f(policy);
    }
    // this crate's own tests have no shell: a small made-up table
    vec![("chat", "CHAT"), ("browser", "BROWSER"), ("ide", "AI IDE"), ("edit", "EDIT"), ("cli", "CLI"), ("system", "SYSTEM"), ("radio", "RADIO"), ("calendar", "CALENDAR")]
        .into_iter()
        .filter(|(id, _)| iyakki_sec::policy::App::ALL.iter().any(|a| a.id() == *id && policy.allows_app(*a)))
        .collect()
}

pub fn set_layout_tiles(f: LayoutTilesFn) {
    let _ = LAYOUT_TILES.set(f);
}

pub fn layout_tiles(admin: bool, cloud: &iyakki_cloudmode::CloudMode) -> Vec<(String, &'static str, &'static str)> {
    if let Some(f) = LAYOUT_TILES.get() {
        return f(admin, cloud);
    }
    // this crate's own tests have no shell: a small made-up table in the shape the shell's tile table gives (CHAT, and USERS for administrators)
    #[cfg(test)]
    {
        let mut v = vec![(iyakki_shared::layout::sys_key("CHAT"), "CHAT", "home")];
        if admin {
            v.push((iyakki_shared::layout::sys_key("USERS"), "USERS", "system"));
        }
        let _ = cloud;
        return v;
    }
    #[cfg(not(test))]
    {
        let _ = (admin, cloud);
        Vec::new()
    }
}

pub mod chat;
pub mod chat_view;
pub mod chat_browser_data;
pub mod chat_camvision;
pub mod chat_embedbrowser;
pub mod chat_appevents;
pub mod events_port;
pub mod chat_embedcameras;
pub mod chat_embedmedia;
pub mod chat_hostname;
pub mod chat_themes;

/// The accounts as the core's source of a person's policy (for the embedded terminal, which checks every line against it).
pub struct StorePolicies(pub std::sync::Arc<std::sync::Mutex<iyakki_accounts::UserStore>>);

impl iyakki_core_api::PolicySource for StorePolicies {
    fn policy_of(&self, user: &str) -> Option<iyakki_sec::policy::Policy> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(user).map(|r| r.policy())
    }
}
pub mod chat_monitor;
pub mod chat_resume;
pub mod markdown;
pub mod download_open;
pub mod chat_settings;
pub mod gemvoice;
pub mod tool_chain;
pub mod chat_settings_assistant;
pub(crate) mod model_picker;
pub mod chatpic;
pub mod browser;
pub mod browser_assist;
pub mod browser_av;
pub mod browser_badsites;
pub mod browser_block;
pub mod browser_live;
pub mod browser_chrome;
pub mod browser_groups;
pub mod browser_headers;
pub mod browser_config;
pub mod browser_dark;
pub mod browser_data;
pub mod browser_forms;
pub mod browser_stream;
pub mod browser_thumbs;
pub mod browser_upload;
pub mod browser_full;
pub mod browser_media;
pub mod web_agent;
pub mod browser_consent;
pub mod browser_devpanel;
pub mod browser_inspect;
pub mod browser_devtools;
pub mod browser_report;
pub mod browser_images;
pub mod browser_sound;
pub mod soundask;
pub mod browser_speed;
pub mod browser_typo;
pub mod browser_logins;
pub mod browser_persist;
pub mod pdfview;
pub mod browser_layout;
pub mod cli;
pub use iyakki_uikit::controls;
pub mod dash;
pub mod dash_charts;
pub mod dash_query;
pub mod dash_repair;
pub mod flows;
pub mod flows_assist;
pub mod flows_board;
pub mod flows_canvas;
pub mod flows_defaults;
pub mod flows_fail;
pub mod flows_hook;
pub mod flows_icons;
pub mod flows_tip;
pub mod flows_keys;
pub mod flows_paint;
pub mod flows_props;
pub mod flows_run;
pub mod flows_view;
pub mod data;
pub mod data_port;
pub mod dropattach;
pub mod hostfiles;
pub mod mcp;
pub mod mcp_port;
pub mod media;
pub mod media_port;
pub mod mediaplay;
pub mod multiline;
pub use iyakki_uikit::neon;
pub mod mesh;
pub mod radio;
pub mod sample;
pub mod sdk_browser;
pub mod sdk_media;
pub mod assistant_port;
pub mod cameras_port;
pub mod radio_port;
pub mod rag;
pub mod runs;
pub mod scan;
pub mod skills;
pub mod skills_port;
pub mod store;
pub(crate) mod store_drivers;
pub mod usage;
pub mod voice;
mod voice_mine;
mod voice_rec;
mod voice_rec_ui;
mod voice_ui;
pub mod lab;
mod lab_pic;
mod lab_ui;
pub mod story;
pub mod system;
pub mod calendar;
pub mod camera;
pub mod camera_port;
pub use iyakki_uikit::card;
pub mod tile;
pub mod tile_wasm;
pub mod inbox;
pub mod jobs_ui;
pub mod edit;
pub mod edit_preview;
pub mod board;
pub mod component;
pub mod ide;
pub mod ide_browser;
pub mod ide_bottom;
pub mod ide_router;
pub use iyakki_uikit::ide_chrome;
pub mod ide_deploy;
pub mod ide_gitview;
pub mod ide_k8s;
pub mod k8s_tile;
pub mod git_tile;
pub mod learn;
pub mod devtools;
pub mod auth;
pub mod remotefiles;
pub mod cameras;
mod learn_lesson;
mod learn_ui;
mod git_tile_repo;
mod git_tile_repo_ui;
mod git_tile_ui;
pub mod disks_tile;
mod disks_tile_ui;
pub mod checkup;
mod checkup_ui;
pub mod doctor;
pub(crate) mod doctor_facts;
mod doctor_ui;
pub mod cluster_host;
pub mod cluster_switch;
pub mod clusters_panel;
pub mod linuxwin;
mod k8s_cloud;
mod k8s_tile_ui;
pub mod platform_services;
pub mod ide_git;
pub mod ide_git_ui;
pub mod git_wizard;
pub mod ide_git_connect;
pub mod ide_notes;
pub mod ide_tasks;
pub mod embed;
pub mod ide_preview;
pub mod ide_tile;
pub mod ide_tasks_run;
pub mod ide_layout;
pub mod ide_tree;
pub mod ide_ui;
pub mod bluetooth_page;
pub mod devices;
pub mod settings;
pub mod settings_nav;
pub mod users;
pub mod users_port;
pub use iyakki_uikit::widgets;

/// What an app asks the shell to do with files dropped on the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// Offer to import them into the Uploads folder (the default).
    Uploads,
    /// Offer to import them into this folder (named by `label` in the dialog); the app is told what arrived.
    Into { dest: std::path::PathBuf, label: String },
    /// The app handled them itself.
    Taken,
    /// Nothing sensible to do here: say so.
    Ignore(String),
}

pub enum Flow {
    Stay,
    Back,
}

/// Everything an app may use. Cloning is cheap (shared handles).
#[derive(Clone)]
pub struct AppCx {
    pub session: Session,
    pub host: Arc<dyn HostApi>,
    pub cfg: AiConfig,
    pub chat: Arc<dyn ChatBackend>,
    pub audit: Arc<AuditLog>,
    pub store: Arc<Mutex<UserStore>>,
    pub mcp_servers: Vec<ServerSpec>,
    pub services: Arc<dyn iyakki_agent_api::ServicesHandle>,
    /// Web access for the browser (checked against the user's host policy by the browser itself).
    pub web: Arc<dyn iyakki_web::Fetch>,
    /// The machine, for what a tile used to read from `/proc` and `/sys` itself (UI/OS split, F-ARCH-05).
    pub os: Arc<dyn iyakki_core_api::Os>,
    /// The core, for what the UI used to take from the agent directly (the mixer first; docs/development/UI-BOUNDARY.md).
    pub core: Arc<dyn iyakki_core_api::Core>,
    /// The agent's functions the UI calls, as a generated facade (dist/split/gen_facade.py): a test gives `FakeAgentApi`.
    pub agent: Arc<dyn iyakki_agent_api::AgentApi>,
    /// A handle on the store facets that replaces the real one (a test gives `FakeStoreFacets`); `None`: the real one over `services` (see [`AppCx::storeapi`]).
    pub store_facets: Option<Arc<dyn iyakki_agent_api::StoreFacets>>,
}

impl AppCx {
    /// The data folder of this install (what the UI used to read as `services.dir()`).
    pub fn data_dir(&self) -> &std::path::Path {
        self.services.dir()
    }

    /// The agent's facets over the person's services, one accessor each (alpha.6 split F: the UI asks the context, not `services`, so the day the
    /// context holds the facets itself no app changes).
    pub fn b0(&self) -> Arc<dyn iyakki_agent_api::B0Services> {
        self.services.b0()
    }

    pub fn b3(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB3> {
        self.services.b3()
    }

    pub fn b4(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB4> {
        self.services.b4()
    }

    pub fn b5(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB5> {
        self.services.b5()
    }

    pub fn voicedesk_facet(&self) -> Arc<dyn iyakki_agent_api::VoiceDeskFacet> {
        self.services.voicedesk_facet()
    }

    pub fn extagent_facet(&self) -> Arc<dyn iyakki_agent_api::ExtAgentFacet> {
        self.services.extagent_facet()
    }

    pub fn tasks_facet(&self) -> Arc<dyn iyakki_agent_api::TasksFacet> {
        self.services.tasks_facet()
    }

    pub fn flows_facet(&self) -> Arc<dyn iyakki_agent_api::FlowsFacet> {
        self.services.flows_facet()
    }

    /// The device watch (the things plugged in and the ones the person trusts).
    pub fn devwatch(&self) -> &Arc<iyakki_shared::devwatch::DeviceWatch> {
        &self.services.devwatch()
    }

    /// The browser state (the person's consent, the jobs the shell serves) and the screen and camera hubs.
    pub fn browser(&self) -> &iyakki_shared::browserconsent::BrowserState {
        &self.services.browser()
    }

    pub fn screen(&self) -> &iyakki_shared::screencap::ScreenCapture {
        &self.services.screen()
    }

    pub fn camera(&self) -> &iyakki_shared::camtools::CameraHub {
        &self.services.camera()
    }

    /// The chat sessions, skills, runs, MCP servers, skill packages and background jobs of the person (stores that stay in the agent).
    pub fn sessions(&self) -> Arc<dyn iyakki_agent_api::SessionApi> {
        self.services.sessions()
    }

    pub fn skills(&self) -> Arc<dyn iyakki_agent_api::SkillApi> {
        self.services.skills()
    }

    pub fn runs(&self) -> Arc<dyn iyakki_agent_api::RunApi> {
        self.services.runs()
    }

    pub fn mcp(&self) -> &dyn iyakki_agent_api::McpApi {
        self.services.mcp()
    }

    pub fn packages(&self) -> Arc<dyn iyakki_agent_api::PackageApi> {
        self.services.packages()
    }

    pub fn jobs(&self) -> Arc<dyn iyakki_agent_api::JobApi> {
        self.services.jobs()
    }

    /// The radio player.
    pub fn radio(&self) -> Arc<dyn iyakki_agent_api::RadioApi> {
        self.services.radio()
    }

    /// The developer tools (the browser's console and network log).
    pub fn devtools(&self) -> &iyakki_shared::devtools::DevTools {
        &self.services.devtools()
    }

    // --- the small reads of the person's services that no store owns (piece 10): one accessor each, so the day `services` goes only these bodies change.
    pub fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        self.services.quick_upstream()
    }

    pub fn page_snapshot_of(&self, user: &str) -> Option<iyakki_shared::webtools::PageSnapshot> {
        self.services.page_snapshot_of(user)
    }

    pub fn set_page_snapshot(&self, user: &str, snap: Option<iyakki_shared::webtools::PageSnapshot>) {
        self.services.set_page_snapshot(user, snap)
    }

    pub fn set_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        self.services.set_streamer(s)
    }

    pub fn streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>> {
        self.services.streamer()
    }

    pub fn session_workdir(&self, user: &str, session_id: &str) -> Result<std::path::PathBuf, String> {
        self.services.session_workdir(user, session_id)
    }

    pub fn session_files(&self, user: &str, id: &str) -> usize {
        self.services.session_files(user, id)
    }

    pub fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        self.services.set_voice_mic(id)
    }

    pub fn chat_backend_for(&self, choice: iyakki_shared::settings::ModelChoice) -> Arc<dyn iyakki_shared::chat::ChatBackend> {
        self.services.chat_backend_for(choice)
    }

    /// What the platform panels need from the services, as handles (so the panels name no `Services`).
    pub fn platform_handles(&self) -> iyakki_agent_api::PlatformHandles {
        let svc = self.services.clone();
        iyakki_agent_api::PlatformHandles {
            dir: self.services.dir().to_path_buf(),
            k3s: self.services.k3sdev(),
            jobs: self.services.jobs(),
            any: self.services.clone().as_any_arc(),
            start_registry: Arc::new(move |owner, a, k3s_present, launch| svc.qi_start_registry(owner, a, k3s_present, launch)),
        }
    }

    /// The downloaded command line tools.
    pub fn clis(&self) -> Arc<dyn iyakki_agent_api::CliApi> {
        self.services.clis()
    }

    /// The Ollama server of the CLI page.
    pub fn ollama(&self) -> Arc<dyn iyakki_agent_api::OllamaApi> {
        self.services.ollama()
    }

    /// The sandbox cluster of the CLI page.
    pub fn sandbox_cluster(&self) -> Arc<dyn iyakki_agent_api::SandboxCluster> {
        self.services.cluster()
    }

    /// The managed local clusters (the primary and the extra ones).
    pub fn fleet(&self) -> Arc<dyn iyakki_agent_api::FleetApi> {
        self.services.fleet()
    }

    /// The managed dev cluster (k3s) of this computer.
    pub fn k3sdev(&self) -> Arc<dyn iyakki_agent_api::DevCluster> {
        self.services.k3sdev()
    }

    pub fn git_kit(&self) -> Arc<dyn iyakki_agent_api::GitKit> {
        self.services.git_kit()
    }

    /// The stores whose type is already shared data (no agent type): the settings, the inbox, the calendar, the study plans, the secrets, the knowledge base.
    pub fn settings(&self) -> &Arc<Mutex<iyakki_shared::settings::SettingsStore>> {
        &self.services.settings()
    }

    pub fn inbox(&self) -> &iyakki_shared::inbox::Inbox {
        &self.services.inbox()
    }

    pub fn calendar(&self) -> &iyakki_shared::calendar::CalendarStore {
        &self.services.calendar()
    }

    pub fn study(&self) -> &iyakki_shared::study::StudyStore {
        &self.services.study()
    }

    pub fn secrets(&self) -> &iyakki_shared::vault::Secrets {
        &self.services.secrets()
    }

    pub fn kb(&self) -> &Arc<Mutex<iyakki_memx::kb::KnowledgeBase>> {
        &self.services.kb()
    }

    /// The voice and sound settings, a copy of what the settings hold.
    pub fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.services.voice_settings()
    }

    pub fn sound_settings(&self) -> iyakki_core_api::soundtypes::SoundSettings {
        self.services.sound_settings()
    }

    pub fn agents_cfg(&self) -> iyakki_shared::extagentcfg::AgentsCfg {
        self.services.agents_cfg()
    }

    /// What the last look at each external coding agent found (shared data type).
    pub fn ext_status(&self) -> &Arc<iyakki_shared::extagent::StatusCache> {
        &self.services.ext_status()
    }

    // ---- split G: the model and speech side
    /// The speech engines of this computer (detection, the browser voice helper).
    pub fn host_tts(&self) -> Arc<dyn iyakki_agent_api::HostTtsApi> {
        self.ai().host_tts()
    }

    /// The model side: providers, picture models, vendor sessions.
    pub fn ai(&self) -> Arc<dyn iyakki_agent_api::AiRead> {
        self.services.ai()
    }

    /// The vaults: the system vault, each person's vault, the answers to secret requests.
    pub fn vault(&self) -> Arc<dyn iyakki_agent_api::VaultRead> {
        self.services.vault_facet()
    }

    /// Put a note into the person's inbox.
    pub fn notify(&self, user: &str, kind: iyakki_shared::inbox::Kind, title: &str, body: &str, link: Option<iyakki_shared::inbox::Link>) -> Result<(), String> {
        let source = link.as_ref().map(|l| l.app.clone()).unwrap_or_else(|| "system".into());
        self.inbox().add(user, kind, title, body, &source, link, iyakki_clock::now_unix()).map(|_| ())
    }

    /// The secrets the assistant asked the person for and that wait for an answer (shared data type).
    pub fn secret_requests(&self) -> &iyakki_shared::secretreq::SecretRequests {
        &self.services.secret_requests()
    }

    /// The token usage log (shared data type, a file in the data folder).
    pub fn usage(&self) -> &iyakki_shared::usagelog::UsageLog {
        &self.services.usage()
    }

    /// The store, the update check and the system components as narrow facets (iyakki-agent-api `store`): the handle a test gave, or the real one over
    /// this context's services (so a test that swaps `services` still reads the same data folder).
    pub fn pendingapi(&self) -> Arc<dyn iyakki_agent_api::pending::AgentPending> {
        self.services.pending_facet()
    }

    pub fn storeapi(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        self.store_facets.clone().unwrap_or_else(|| self.services.store_facets())
    }
}

pub trait App {
    fn title(&self) -> &'static str;
    /// The page or tab that is showing, when the app has them (developer mode's `iyakki dev state` tells it).
    fn page(&self) -> String {
        String::new()
    }
    /// The title of a dialog or question drawn over this tile right now (a file picker, a form, a confirmation), or `None`. The screen
    /// reader says it once when it opens (F-A11Y-02).
    fn dialog_title(&self) -> Option<String> {
        None
    }
    fn icon(&self) -> Icon;
    fn accent(&self) -> [u8; 4];
    /// One-line key help shown in the header.
    fn hint(&self) -> String;
    /// Every key and click this screen understands, one per line, for the F1 help popup. By default the header
    /// hint split into its parts.
    fn help(&self) -> Vec<String> {
        self.hint().split(" · ").map(str::to_string).collect()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow;
    /// The screen is about to lock and the person's protected browser data is sealed with it: an app that holds a web engine on that data
    /// lets it go first (F-VAULT-08, KI-742). The default does nothing.
    fn on_lock(&mut self, _cx: &AppCx) {}
    /// Asked when the person clicks Back: false keeps the app open (it may be asking something first, and then returns
    /// `Flow::Back` itself when the answer is yes).
    fn leave_guard(&mut self, _cx: &AppCx) -> bool {
        true
    }
    /// A smooth scroll step of `px` pixels of a nominal screen (positive = up; [`crate::scrolleng::NOTCH_PX`] is one wheel click) while
    /// nothing is drawn over this app. True when something under the pointer scrolled by it; false (the default) makes the shell send
    /// whole [`Event::Scroll`] notches instead, which every app understands.
    fn scroll_px(&mut self, _px: i32, _cx: &AppCx) -> bool {
        false
    }
    /// The app wants F12 for itself (the browser's Developer tools) instead of the shell's screenshot key. The shell checks this
    /// before it starts a screenshot; Print Screen still takes one. The default says no.
    fn claims_f12(&self) -> bool {
        false
    }
    /// The app shows another computer's program and wants Alt+key as a key of its own (`Key::Alt`), not as the plain letter. Default: no.
    fn wants_alt(&self) -> bool {
        false
    }
    /// The app draws its own smooth pictures and wants the wheel's raw clicks: the shell's wheel steps are not eased for it. No tile does
    /// now (the browser's engine takes the shell's eased steps as precise deltas, so it feels like the other tiles).
    fn scroll_raw(&self) -> bool {
        false
    }
    /// A media key (play/pause, next, previous, stop) pressed while this app is open: true when the app took it. The shell does
    /// the rest (the radio keeps playing in the background, so it answers the keys when no app does).
    fn media_key(&mut self, _k: crate::fnkeys::FnKey, _cx: &AppCx) -> bool {
        false
    }
    /// An app the shell should open next (the INBOX opens the app a notification points to): its id and the item in it.
    fn take_request(&mut self) -> Option<(String, String)> {
        None
    }
    /// Something the person is watching or listening to goes on in this app (a story being read aloud): the screen stays on
    /// (`shell_saver.rs`). The default says no.
    fn keeps_screen_on(&self) -> bool {
        false
    }
    /// What this app says about itself while it is open (F-LAUNCH-09): the chip and badge on its icon, the line under its name and, when a
    /// person is needed, a row on the machine page. The core says the same for an app that is not open (`iyakki_core_api::AppStatuses`);
    /// when both exist the open app's own word wins. The default says nothing: the icon then shows the app's static line.
    fn status(&self, _cx: &AppCx) -> Option<iyakki_core_api::AppStatus> {
        None
    }
    /// For the Touch Bar's AI strip: (an agent run is going, a tool call waits for a decision). Only the chat knows.
    fn touchbar_state(&self) -> (bool, bool) {
        (false, false)
    }
    /// This app wants another app opened on one thing (`settings`, `tab:models`): taken by the shell once.
    fn take_jump(&mut self) -> Option<(String, String)> {
        None
    }
    /// The shell opened this app because another one asked for a particular thing in it (`platform:private-store` in Settings).
    fn open_item(&mut self, _item: &str, _cx: &AppCx) {}
    /// Something changed on screen without a key or click (a frame arrived from an embedded engine): the shell draws again.
    /// Asked after every heartbeat; true once per change.
    fn wants_redraw(&mut self) -> bool {
        false
    }
    /// Like [`App::wants_redraw`], but says how much: an app whose live picture is a rectangle of the screen (the browser's full
    /// engine) answers `Live` when only that picture changed, so the shell can paint just it ([`App::live_draw`]) instead of the
    /// whole scene. The default is `Full` whenever `wants_redraw` says so.
    fn redraw_kind(&mut self) -> browser_full::Redraw {
        if self.wants_redraw() {
            browser_full::Redraw::Full
        } else {
            browser_full::Redraw::None
        }
    }
    /// Paint only the changed live picture over the scene drawn last (after `redraw_kind` said `Live`, and only when the shell
    /// knows nothing else on screen changed or overlaps it). Returns the screen rectangle `(x, y, w, h)` written; `None` makes
    /// the shell draw the whole scene instead.
    fn live_draw(&mut self, _g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        None
    }
    /// The rectangles `(x, y, w, h)` the last [`App::live_draw`] painted, when it painted several small ones (the screen write
    /// then copies only these, not whole rows). Empty: just the one rectangle `live_draw` returned.
    fn live_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        Vec::new()
    }
    /// How rarely (ms) the shell has to draw the whole scene while this app paints live; the shell draws it at least every 100 ms
    /// otherwise. An app that checks itself what else changed on screen may ask for longer.
    fn live_full_every_ms(&self) -> u64 {
        100
    }
    /// How often (ms) the shell has to draw the whole scene while nothing in this app moves (no overlay is open, the pointer is
    /// below the header). The default is the shell's 100 ms safety net; an app whose screen only changes on events may ask for more.
    fn idle_redraw_ms(&self) -> u64 {
        100
    }
    /// True while the app wants to be asked between the 33 ms heartbeats whether something changed (the word being read aloud):
    /// the backend then wakes every few ms and calls [`App::poll_redraw_due`], so a change is on screen within a few ms.
    fn wants_fast_poll(&self) -> bool {
        false
    }
    /// Asked between heartbeats while [`App::wants_fast_poll`]: true once when something changed that needs a frame now.
    fn poll_redraw_due(&mut self) -> bool {
        false
    }
    /// The pointer shape the app wants at (x, y) when the shell's own hit regions have no opinion (an embedded engine
    /// draws its own page and knows what is under the pointer).
    fn pointer_hint(&self, _x: i32, _y: i32) -> Option<crate::ui::Pointer> {
        None
    }
    /// Files or folders dropped on the window (window mode only): what this app wants done with them.
    fn on_drop(&mut self, _paths: &[std::path::PathBuf], _cx: &AppCx) -> DropAction {
        DropAction::Uploads
    }
    /// A file the app wants saved on the host computer (window mode): its suggested name and bytes. The shell shows the save
    /// dialog (rooted at the host home or a writable shared folder; nothing is replaced silently).
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        None
    }
    /// The app asks for the options of one shared host folder (convert to a copy, stop sharing, ...): its name.
    fn take_host_mount_menu(&mut self) -> Option<String> {
        None
    }
    /// The files a drop brought in (copied into the destination the app asked for), or empty after a folder was shared.
    fn dropped_done(&mut self, _files: &[std::path::PathBuf], _cx: &AppCx) {}
    /// Entries the app adds to the right-click menu at (x, y) (the shell adds Cut / Copy / Paste / Select all over a text
    /// field and always ends with "Clipboard history…"). Entries with the ids `copy`, `cut`, `paste` and `selall` are
    /// carried out by the shell as Ctrl+C / X / V / A sent to the app; any other id comes back through [`App::context_pick`].
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        Vec::new()
    }
    /// An entry the app added to the context menu was chosen.
    fn context_pick(&mut self, _id: &str, _cx: &AppCx) -> Flow {
        Flow::Stay
    }
    /// What the tile holds after being handed an item by another tile: EDIT the open files, MEDIA the queue and its place, DATA the
    /// table path (tests of the hand-over at shell level; every other app: nothing).
    #[cfg(any(test, feature = "testing"))]
    fn probe(&self) -> String {
        String::new()
    }
    /// The address of the list link that has the keyboard focus (browser tests).
    #[cfg(any(test, feature = "testing"))]
    fn probe_link(&self) -> Option<String> {
        None
    }
    /// A custom app answers with itself so the shell's tests can look inside (every other app: nothing).
    #[cfg(any(test, feature = "testing"))]
    fn as_any_tile(&mut self) -> Option<&mut tile::TileApp> {
        None
    }
    /// A `wasm` tile answers with the words it drew, or why it does not run (every other app: nothing).
    #[cfg(any(test, feature = "testing"))]
    fn wasm_tile_text(&mut self) -> String {
        String::new()
    }
    /// Draw into `area` (below the header). `tick` advances ~30 times a second.
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64);
}

/// Scrollback of coloured text lines, wrapped to the area width. `scroll` counts visual lines
/// from the bottom; the newest content is shown by default.
///
/// The text can be selected with the pointer (press and drag, double click a word, triple click an entry, Shift+click to
/// extend) and copied; the owner passes its events to [`ScrollLog::handle`] first.
pub struct ScrollLog {
    pub lines: Vec<(String, [u8; 4])>,
    pub scroll: usize,
    /// Selection over what was drawn last.
    pub pane: crate::textsel::Pane,
    lh: f32,
    /// Keep each entry on one line (cut with an ellipsis) instead of wrapping it.
    pub nowrap: bool,
}

/// The id of the area a scroll log registers for presses (`Event::Area`).
pub const LOG_AREA: usize = 61_100;

impl Default for ScrollLog {
    fn default() -> Self {
        ScrollLog { lines: Vec::new(), scroll: 0, pane: crate::textsel::Pane::new(LOG_AREA), lh: 16.0, nowrap: false }
    }
}

impl ScrollLog {
    pub fn push(&mut self, text: impl Into<String>, color: [u8; 4]) {
        self.lines.push((text.into(), color));
        self.scroll = 0;
        if self.lines.len() > 2000 {
            self.lines.drain(..500);
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
        self.pane.clear();
    }

    /// Presses, drags and Ctrl+C on the text; true when the event was used.
    pub fn handle(&mut self, ev: &Event) -> bool {
        self.pane.handle(ev)
    }

    /// One heartbeat: a drag held above or below the text scrolls it.
    pub fn on_tick(&mut self) {
        let a = self.pane.auto_scroll();
        if a != 0.0 {
            self.scroll_by((a / self.lh.max(1.0)).round() as i32 + a.signum() as i32);
            self.pane.refresh_drag();
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).max(0) as usize;
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, size: f32, face: Face) {
        let lh = g.line_height(size, face);
        let mut visual: Vec<([u8; 4], String)> = Vec::new();
        let mut entry_of: Vec<u32> = Vec::new();
        for (n, (t, c)) in self.lines.iter().enumerate() {
            if self.nowrap {
                visual.push((*c, g.fit(size, t, face, area.w)));
                entry_of.push(n as u32);
                continue;
            }
            for l in g.wrap(size, t, face, area.w) {
                visual.push((*c, l));
                entry_of.push(n as u32);
            }
        }
        let fit = (area.h / lh).floor().max(1.0) as usize;
        let max_scroll = visual.len().saturating_sub(fit);
        self.scroll = self.scroll.min(max_scroll);
        let end = visual.len() - self.scroll;
        let start = end.saturating_sub(fit);
        for (i, (c, l)) in visual[start..end].iter().enumerate() {
            g.text(area.x, area.y + i as f32 * lh, size, if *c == [0, 0, 0, 0] { TEXT } else { *c }, l, face);
        }
        // the selectable text: every line, those out of view without geometry
        self.lh = lh;
        self.pane.begin(area);
        for (i, (_, l)) in visual.iter().enumerate() {
            self.pane.line(g, area.x, area.y + (i as f32 - start as f32) * lh, lh, size, face, l, 0, entry_of[i]);
        }
        self.pane.end(g, crate::fx::CYAN);
    }
}

pub use iyakki_uikit::blink::{blink, caret_allowed, with_caret};

#[cfg(test)]
mod browser_compat_probe;
#[cfg(test)]
mod learn_tests;
#[cfg(test)]
mod disks_tile_tests;
#[cfg(test)]
mod checkup_tests;
#[cfg(test)]
mod doctor_tests;
#[cfg(test)]
mod linuxwin_tests;
#[cfg(test)]
mod ide_tests;
#[cfg(any(test, feature = "testing"))]
pub(crate) mod ide_fakeprobe_tests;
#[cfg(test)]
mod jobs_screens_tests;
#[cfg(test)]
mod dash_tests;
#[cfg(test)]
mod flows_tests;
#[cfg(test)]
pub(crate) mod lemonade_fake;
#[cfg(test)]
pub(crate) mod pdf_live_check;
#[cfg(test)]
pub(crate) mod tile_harness;

#[cfg(any(test, feature = "testing"))]
pub mod tests {
    #[cfg(test)]
    use crate::agent_handles_b2::real::RealServices as _;
    use super::*;
    use iyakki_sec::policy::{Policy, Role};

    #[derive(Default)]
    pub struct MockHost {
        pub powered_off: Mutex<Option<String>>,
        /// Whether the host says it can sleep (tests set it) and how many times `sleep` was called.
        pub sleepable: Mutex<bool>,
        pub slept: Mutex<u32>,
    }
    impl HostApi for MockHost {
        fn detect_env(&self) -> String {
            "baremetal".into()
        }
        fn net_status(&self) -> String {
            "eth0 Ethernet".into()
        }
        fn run_wasm(&self, _u: &str, _p: &Policy, _w: &[u8], _e: &str) -> Result<String, String> {
            Ok("ok".into())
        }
        fn power_off(&self) {
            *self.powered_off.lock().unwrap() = Some("off".into());
        }
        fn reboot(&self) {
            *self.powered_off.lock().unwrap() = Some("reboot".into());
        }
        fn can_sleep(&self) -> bool {
            *self.sleepable.lock().unwrap()
        }
        fn sleep(&self) {
            *self.slept.lock().unwrap() += 1;
        }
    }

    pub fn temp_dir(tag: &str) -> std::path::PathBuf {
        crate::testmem::start();
        // one folder per call (parallel tests ask for the same tags) and a short path (sockets live below it)
        (crate::testkit::kit().expect("install a test kit (iyakki_apps::testkit::install) before the first test helper").short_dir)("iyakki-shell", tag)
    }

    /// An admin session (user "tester") with a store containing only that admin.
    /// Upstream that always fails: tests that need network behaviour install their own.
    pub struct Offline;
    impl iyakki_proxy::Upstream for Offline {
        fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    pub fn services_in(d: &std::path::Path, up: Arc<dyn iyakki_proxy::Upstream>) -> Arc<dyn iyakki_agent_api::ServicesHandle> {
        no_real_agents_on_path();
        (crate::testkit::kit().expect("install a test kit before the first test helper").open_services)(&d.join("svc"), up.clone(), up).unwrap()
    }

    /// Hermetic tests: a developer PC that has the real `claude` or `copilot` on its PATH must not change what a test sees (the
    /// sign-in refresh would find the real program and its real account). Once per test process, a folder with two stand-ins that
    /// fail at once ("not installed", as on a clean machine) goes first on the PATH. A test that wants an agent sets its own command
    /// or puts a fake in the CLI tile's folder; those are used by their full path.
    fn no_real_agents_on_path() {
        use std::os::unix::fs::PermissionsExt;
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // the agent crate is not compiled as a test here, so its `cfg!(test)` guard is off: say it with the variable instead, or
            // a developer's real ~/Flows (a flow named A) shows up in the shell's screens and tests could write to the real home
            std::env::set_var("IYAKKI_NO_HOST_HOME", "1");
            let d = std::env::temp_dir().join(format!("iyakki-test-no-agents-{}", std::process::id()));
            if std::fs::create_dir_all(&d).is_err() {
                return;
            }
            for n in ["claude", "copilot"] {
                let f = d.join(n);
                if std::fs::write(&f, "#!/bin/sh\nexit 127\n").is_ok() {
                    let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
                }
            }
            let mut dirs = vec![d];
            dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
            if let Ok(p) = std::env::join_paths(dirs) {
                std::env::set_var("PATH", p);
            }
        });
    }

    pub fn cx_with_admin_store(tag: &str) -> AppCx {
        let d = temp_dir(tag);
        let mut store = UserStore::open(&d.join("users.json")).unwrap();
        store.create_first_admin("root", "rootpass12").unwrap();
        let services = services_in(&d, Arc::new(Offline));
        let store = Arc::new(Mutex::new(store));
        AppCx {
            session: Session { user: "root".into(), role: Role::Admin, policy: Role::Admin.template() },
            host: Arc::new(MockHost::default()),
            cfg: AiConfig::parse("").unwrap(),
            chat: Arc::new(iyakki_shared::chat::Unconfigured),
            audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
            store: store.clone(),
            mcp_servers: vec![],
            // the real handle over the test's services: the existing tests still see the process-wide mixer
            core: (crate::testkit::kit().expect("install a test kit").core)(&services, Arc::new(StorePolicies(store))),
            agent: (crate::testkit::kit().expect("install a test kit").agent)(),
            store_facets: None,
            services,
            web: Arc::new(iyakki_web::fetch::NoNetwork),
            os: Arc::new(iyakki_core_fake::FakeOs::new()),
        }
    }

    /// The read accessors of the context (alpha.6 split F) reach the real stores of the services, not a copy or a default: the same object, and a write
    /// through one is read through the other.
    #[test]
    fn the_context_read_accessors_are_the_real_stores_not_defaults() {
        let cx = cx_with_admin_store("cx-read-facets");
        // split H: the handle the context holds is the agent's real `Services` (never a fake default: KI-1010), and the accessors of the context read the
        // same objects through it as the concrete type does
        assert!(cx.services.is_real(), "AppCx::services is not the real implementation");
        let real = cx.services.real(); // panics unless the handle is the agent's Services

        assert!(std::ptr::eq(cx.settings().as_ref(), real.settings.as_ref()), "settings are not the real store");
        assert!(std::ptr::eq(cx.kb().as_ref(), real.kb.as_ref()), "the knowledge base is not the real store");
        assert!(std::ptr::eq(cx.devwatch().as_ref(), real.devwatch.as_ref()), "the device watch is not the real one");
        assert!(std::ptr::eq(cx.browser(), &real.browser) && std::ptr::eq(cx.screen(), &real.screen) && std::ptr::eq(cx.camera(), &real.camera));
        assert!(std::ptr::eq(cx.devtools(), &real.devtools));
        assert!(std::ptr::eq(cx.services.dir(), real.dir()), "the data folder is not the real one");
        let (a, b): (Arc<dyn iyakki_agent_api::SessionApi>, Arc<dyn iyakki_agent_api::SessionApi>) = (cx.sessions(), real.sessions.clone());
        assert!(std::ptr::eq(Arc::as_ptr(&a) as *const u8, Arc::as_ptr(&b) as *const u8), "the sessions are not the real store");
        assert!(std::ptr::eq(cx.inbox(), cx.services.inbox()));
        assert!(std::ptr::eq(cx.calendar(), cx.services.calendar()));
        assert!(std::ptr::eq(cx.study(), cx.services.study()));
        assert!(std::ptr::eq(cx.secrets(), cx.services.secrets()));
        assert!(Arc::ptr_eq(cx.settings(), &cx.services.settings()));
        assert!(Arc::ptr_eq(cx.kb(), &cx.services.kb()));
        assert!(Arc::ptr_eq(cx.ext_status(), &cx.services.ext_status()));
        cx.settings().lock().unwrap().update(|s| s.voice.mic = "sweep-mic".into()).unwrap();
        assert_eq!(cx.services.voice_settings().mic, "sweep-mic");
        assert_eq!(cx.voice_settings().mic, "sweep-mic");
        // split G: the usage log and the secret requests are the same objects; the facets write through to the real stores
        assert!(std::ptr::eq(cx.usage(), cx.services.usage()));
        assert!(std::ptr::eq(cx.secret_requests(), cx.services.secret_requests()));
        cx.ai().usage_record("root", "p", "m", 3, 4);
        assert_eq!(cx.services.usage().entries(Some("root")).iter().map(|e| e.input + e.output).sum::<u64>(), 7);
        cx.ai().ext_session_set("chat1", "claude-code", "vendor-9");
        assert_eq!(cx.services.real().ext_sessions.get("chat1", "claude-code").as_deref(), Some("vendor-9"));
        assert!(!cx.host_tts().is_enabled() || cx.services.host_tts().is_enabled());
        cx.host_tts().configure(iyakki_shared::hostengines::Env { path: String::new(), os: iyakki_shared::hostengines::Os::Linux, piper_dirs: vec![] });
        assert!(cx.services.host_tts().is_enabled(), "the speech-engine facet is the real HostTts");
        assert_eq!(cx.vault().user_vault_unlocked("nobody"), cx.services.user_vault_unlocked("nobody"));
        assert!(matches!(cx.vault().system_names(), Err(iyakki_shared::vault::VaultError::Locked)) == cx.services.real().system_names().is_err());
    }

    #[test]
    fn a_tile_reads_the_machine_through_cx_os_and_a_test_gets_the_fake_one() {
        let cx = cx_with_admin_store("cx-os");
        assert_eq!(cx.os.sysinfo().product_name().unwrap(), "Test Machine");
        assert_eq!(cx.os.sysinfo().cpu_count().unwrap(), 4);
    }

    /// Point HOME at a fresh temporary folder for the whole test process, so tests that open the FLOWS environment of an account
    /// (`FlowEnv::open`) keep their flows out of the developer's own home. Safe to call from any test.
    pub fn isolated_home() {
        static HOME: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            let d = std::env::temp_dir().join(format!("iyakki-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&d);
            std::env::set_var("HOME", &d);
        });
    }

    pub fn cx_with_backend(tag: &str, backend: impl ChatBackend + 'static) -> (AppCx, std::path::PathBuf) {
        let mut cx = cx_with_admin_store(tag);
        cx.session.user = "tester".into();
        cx.chat = Arc::new(backend);
        let d = temp_dir(&format!("{tag}-x"));
        cx.audit = Arc::new(AuditLog::new(&d.join("audit.log")));
        (cx, d)
    }
}


/// The engine of Settings -> Voice as a direct voice (an engine of this computer that speaks by itself), `None` for any other.
pub(crate) fn direct_voice(agent: &dyn iyakki_agent_api::AgentApi, e: &iyakki_shared::voice::SpeechEngine) -> Option<std::sync::Arc<dyn iyakki_agent_api::DirectVoice>> {
    match e {
        iyakki_shared::voice::SpeechEngine::Host { engine, program, voice } => agent.host_engines().direct(engine, program.as_deref(), voice.as_deref()),
        _ => None,
    }
}

/// The engine speaks by itself (no sound file is made).
pub(crate) fn speaks_directly(e: &iyakki_shared::voice::SpeechEngine) -> bool {
    matches!(e, iyakki_shared::voice::SpeechEngine::Host { engine, .. } if iyakki_shared::hostengines::Kind::from_id(engine).is_some_and(iyakki_shared::hostengines::Kind::can_speak))
}

/// "Save audio" works with this engine.
pub(crate) fn can_save_audio(e: &iyakki_shared::voice::SpeechEngine) -> bool {
    match e {
        iyakki_shared::voice::SpeechEngine::Host { engine, .. } => iyakki_shared::hostengines::Kind::from_id(engine).is_some_and(iyakki_shared::hostengines::Kind::can_file),
        iyakki_shared::voice::SpeechEngine::Browser => false,
        _ => true,
    }
}
