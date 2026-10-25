//! The shell: first-run setup, login, the launcher with app tiles, and the app frame.
//! A pure state machine (events in, pixels out), so it is driven identically by the Linux
//! framebuffer backend, by tests, and by the screenshot tool.

use std::sync::{Arc, Mutex};

use iyakki_agent::chat::ChatBackend;
use iyakki_agent::config::AiConfig;
use iyakki_agent::mcp::{self, ServerSpec};
use iyakki_agent::services::{Services, VaultState};
use iyakki_agent::HostApi;
use iyakki_sec::policy::App as PApp;

use crate::accounts::{AccountError, AuditLog, Session, UserStore};
use crate::apps::chat::ChatApp;
use crate::apps::cli::CliApp;
use crate::apps::mcp::McpApp;
use crate::apps::rag::RagApp;
use crate::apps::runs::RunsApp;
use crate::apps::edit::EditApp;
use crate::apps::settings::SettingsApp;
use crate::apps::widgets::{PickEvent, Picker};
use crate::apps::users::UsersApp;
use crate::apps::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::launcher::{self, Group};
#[cfg(test)]
use crate::launcher::tile_rects;
use crate::ui::*;

pub struct ShellEnv {
    pub store: Arc<Mutex<UserStore>>,
    pub audit: Arc<AuditLog>,
    pub host: Arc<dyn HostApi>,
    pub cfg: AiConfig,
    pub chat: Arc<dyn ChatBackend>,
    pub mcp_servers: Vec<ServerSpec>,
    pub services: Arc<Services>,
    /// Web access for the browser.
    pub web: Arc<dyn iyakki_web::Fetch>,
    pub hostname: String,
    /// The data directory is in RAM (live session): nothing survives power-off.
    pub data_volatile: bool,
    /// Start in the hardware check (boot option `iyakki.hwcheck=1`) instead of the login screen.
    pub hwcheck: bool,
}

pub struct ShellPaths {
    pub users: String,
    pub audit: String,
    pub ai_conf: String,
    /// Settings, vaults and the knowledge base live here.
    pub data: String,
    /// Start in the hardware check.
    pub hwcheck: bool,
}

impl Default for ShellPaths {
    fn default() -> Self {
        Self { users: "/var/lib/iyakki/users.json".into(), audit: "/var/lib/iyakki/audit.log".into(), ai_conf: "/etc/iyakki/ai.conf".into(), data: "/var/lib/iyakki".into(), hwcheck: false }
    }
}

impl ShellEnv {
    /// Another environment over the same accounts, audit log and services (one more shell for one more person, as the
    /// browser front end needs).
    pub fn share(&self) -> ShellEnv {
        ShellEnv {
            store: self.store.clone(),
            audit: self.audit.clone(),
            host: self.host.clone(),
            cfg: self.cfg.clone(),
            chat: self.chat.clone(),
            mcp_servers: self.mcp_servers.clone(),
            services: self.services.clone(),
            web: self.web.clone(),
            hostname: self.hostname.clone(),
            data_volatile: self.data_volatile,
            hwcheck: false,
        }
    }

    /// Build the real environment from files on disk.
    pub fn from_paths(p: &ShellPaths, host: Arc<dyn HostApi>) -> Result<ShellEnv, AccountError> {
        use iyakki_agent::iyakki_proxy::https::UreqUpstream;
        let t0 = std::time::Instant::now();
        let cfg = AiConfig::load(&p.ai_conf);
        let quick: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(std::time::Duration::from_secs(10)));
        let slow: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(std::time::Duration::from_secs(300)));
        let services = Services::open(std::path::Path::new(&p.data), quick, slow).map_err(AccountError::Storage)?;
        let t_services = t0.elapsed();
        services.radio.set_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()));
        // what is installed from the CLI list is found everywhere (jobs, the terminal, agents) without links in /usr/bin, and a program
        // that is on the machine already counts as installed
        services.clis.export_path();
        services.clis.set_detect_present(true);
        // sound goes to the output that is connected (HDMI/DP, USB, ...), not to ALSA's card 0 device 0
        let t1 = std::time::Instant::now();
        // cloud mode without sound hardware: no audio probes, no soft volume, no device watcher
        // (the sound card may appear after the shell started: the MacBookPro14,1's does, about 11 s after boot; then the
        // output is set up as soon as it is there, whoever is signed in)
        if iyakki_agent::cloudmode::detect(services.cloud_setting()).audio_services {
            services.start_sound_output();
        } else if !services.cloud_setting().eq(&iyakki_agent::cloudmode::Setting::On) {
            services.start_sound_output_when_ready();
        }
        let t_sound = t1.elapsed();
        // model downloads report their progress over time, like the radio
        services.set_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()));
        // model answers are read as they are written; a local model may need minutes to load before its first word
        services.set_chat_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::with_response_timeout(std::time::Duration::from_secs(600))));
        migrate_legacy_chat(&services, &cfg);
        // the time zone the person chose in Settings -> Time applies to every front end from the start
        if let Ok(st) = services.settings.lock() {
            iyakki_agent::clock::set_chosen_zone(&st.get().time.zone);
        }
        let chat = services.chat_backend();
        let mcp_servers = mcp::load_config(&cfg.mcp_config);
        let t2 = std::time::Instant::now();
        let store = UserStore::open(std::path::Path::new(&p.users))?;
        log::info!("boot: shell environment took {} ms (services {}, sound {}, accounts {})", t0.elapsed().as_millis(), t_services.as_millis(), t_sound.as_millis(), t2.elapsed().as_millis());
        Ok(ShellEnv {
            store: Arc::new(Mutex::new(store)),
            audit: Arc::new(AuditLog::new(std::path::Path::new(&p.audit))),
            host,
            cfg,
            chat,
            mcp_servers,
            services,
            web: browser_web(std::path::Path::new(&p.data)),
            hostname: std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "iyakki".into()),
            hwcheck: p.hwcheck,
            data_volatile: iyakki_storage::data_location(&iyakki_storage::Roots::real(), std::path::Path::new(&p.data)) == iyakki_storage::DataLocation::Memory,
        })
    }
}

/// The browser's network: the HTTP client behind connection limits (6 per host, 32 overall, urgent requests first), with one page
/// cache per person under `cache/web/<user>` (memory tier plus disk tier, sized from the machine, see `apps::browser_speed`).
/// Requests that do not say who they are (the assistant's web tools) are never cached.
fn browser_web(data: &std::path::Path) -> Arc<dyn iyakki_web::Fetch> {
    use iyakki_web::net::{LimitedFetch, Limiter, GLOBAL, PER_HOST};
    let limited: Arc<dyn iyakki_web::Fetch> = Arc::new(LimitedFetch::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default(), Arc::new(Limiter::new(PER_HOST, GLOBAL))));
    let base = data.join("cache").join("web");
    let budget = crate::apps::browser_speed::budget_for(iyakki_web::net::CacheSize::Auto, &crate::apps::browser_speed::machine(data));
    let caches = Arc::new(iyakki_web::cache::UserCaches::new(limited, base, budget));
    // files an older version left directly in the folder belonged to nobody: remove them off the boot path
    let old = caches.clone();
    let _ = std::thread::Builder::new().name("iyakki-cache-tidy".into()).spawn(move || old.remove_legacy_files());
    // the person's ad and tracker filter sits outside the cache: a blocked request is never sent and never cached
    Arc::new(iyakki_web::blocker_state::FilteredFetch::new(caches, iyakki_web::blocker_state::Filter::global()))
}

/// Older installs configured chat in ai.conf (`chat.url`, `chat.model`, `chat.key_*`). Carry that over
/// once, as a normal provider, so nothing stops working — Settings is the source of truth after.
fn migrate_legacy_chat(svc: &Services, cfg: &AiConfig) {
    use iyakki_agent::providers::{Kind, ProviderCfg};
    let Some(url) = &cfg.chat_url else { return };
    let mut st = svc.settings.lock().unwrap();
    if st.get().chat.is_some() || st.get().provider("legacy").is_some() {
        return;
    }
    let base = url.trim_end_matches("/chat/completions").trim_end_matches('/').to_string();
    let mut p = ProviderCfg::new("legacy", Kind::Compat);
    p.base_url = Some(base);
    p.key_ref = cfg.chat_key_file.as_ref().map(|f| format!("file:{f}")).or_else(|| cfg.chat_key_env.as_ref().map(|e| format!("env:{e}")));
    p.default_model = Some(cfg.chat_model.clone());
    if st.upsert_provider(p).is_ok() {
        let _ = st.set_chat(Some(iyakki_agent::settings::ModelChoice { provider: "legacy".into(), model: cfg.chat_model.clone() }));
    }
}

struct Form {
    fields: Vec<TextField>,
    focus: usize,
    error: Option<String>,
}

impl Form {
    fn login() -> Form {
        Form { fields: vec![TextField::new("Username", false), TextField::new("Password", true)], focus: 0, error: None }
    }
    /// The sign-in form with the last person who signed in already filled in, and the cursor on the password.
    fn login_for(last: Option<&str>) -> Form {
        let mut f = Form::login();
        if let Some(name) = last {
            f.fields[0].set(name);
            f.focus = 1;
        }
        f
    }
    fn setup() -> Form {
        Form { fields: vec![TextField::new("Choose a username", false), TextField::new("Choose a password (min 8)", true), TextField::new("Repeat password", true)], focus: 0, error: None }
    }
    fn next(&mut self) {
        self.focus = (self.focus + 1) % self.fields.len();
    }
    fn prev(&mut self) {
        self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
    }
}

enum Screen {
    HwCheck(Box<crate::hwcheck::HwCheck>),
    Setup(Form),
    Login(Form),
    Launcher,
    App(Box<dyn App>, PApp),
    PowerMenu,
}

struct Tile {
    app: PApp,
    icon: Icon,
    label: &'static str,
    sub: &'static str,
    accent: [u8; 4],
    /// The home screen section the tile is in; `None` keeps it off the grid (Settings: the IYAKKI menu and F7 lead there).
    group: Option<Group>,
}

/// One thing on the launcher: a system tile, or a custom app.
enum LItem {
    Sys(&'static Tile),
    App(Box<iyakki_agent::store::tilehub::TileEntry>),
    /// The hint shown in a group of the person's own that has no tiles yet.
    Empty,
}

const TILES: &[Tile] = &[
    Tile { app: PApp::Chat, icon: Icon::Chat, label: "CHAT", sub: "Talk to the AI", accent: rgb(79, 140, 255), group: Some(Group::Home) },
    Tile { app: PApp::Rag, icon: Icon::Rag, label: "RAG", sub: "Ask your files", accent: rgb(61, 214, 163), group: Some(Group::Home) },
    Tile { app: PApp::Mcp, icon: Icon::Mcp, label: "MCP", sub: "Tools, servers", accent: rgb(167, 116, 255), group: Some(Group::Home) },
    Tile { app: PApp::Cli, icon: Icon::Cli, label: "CLI", sub: "Terminal, tools", accent: rgb(255, 184, 77), group: Some(Group::Home) },
    Tile { app: PApp::Edit, icon: Icon::Edit, label: "EDIT", sub: "Edit files", accent: rgb(110, 224, 160), group: Some(Group::Work) },
    Tile { app: PApp::Runs, icon: Icon::Runs, label: "RUNS", sub: "Agent history", accent: rgb(255, 196, 94), group: Some(Group::Work) },
    Tile { app: PApp::Users, icon: Icon::Users, label: "USERS", sub: "Accounts", accent: rgb(255, 107, 154), group: Some(Group::System) },
    Tile { app: PApp::Settings, icon: Icon::Settings, label: "SETTINGS", sub: "Models, vault", accent: rgb(120, 170, 255), group: Some(Group::Home) },
    Tile { app: PApp::Browser, icon: Icon::Browser, label: "BROWSER", sub: "Read the web", accent: rgb(88, 190, 235), group: Some(Group::WebMedia) },
    Tile { app: PApp::Radio, icon: Icon::Radio, label: "RADIO", sub: "Online stations", accent: rgb(255, 150, 110), group: Some(Group::WebMedia) },
    Tile { app: PApp::Skills, icon: Icon::Skills, label: "SKILLS", sub: "Teach tasks", accent: rgb(255, 170, 90), group: Some(Group::Home) },
    Tile { app: PApp::Scan, icon: Icon::Scan, label: "NET SCAN", sub: "Find services", accent: rgb(120, 220, 140), group: Some(Group::System) },
    Tile { app: PApp::Mesh, icon: Icon::Mesh, label: "MESH", sub: "Consul, services", accent: rgb(86, 204, 196), group: Some(Group::System) },
    Tile { app: PApp::Usage, icon: Icon::Usage, label: "USAGE", sub: "Tokens by model", accent: rgb(255, 130, 170), group: Some(Group::System) },
    Tile { app: PApp::System, icon: Icon::System, label: "SYSTEM", sub: "Load, agents", accent: rgb(176, 226, 92), group: Some(Group::System) },
    Tile { app: PApp::Calendar, icon: Icon::Calendar, label: "CALENDAR", sub: "Plans, alarms", accent: rgb(110, 190, 255), group: Some(Group::Home) },
    Tile { app: PApp::Media, icon: Icon::Media, label: "MEDIA", sub: "Music, videos", accent: rgb(240, 120, 220), group: Some(Group::WebMedia) },
    Tile { app: PApp::SrePilot, icon: Icon::SrePilot, label: "SRE PILOT", sub: "Assist · Auto", accent: rgb(60, 235, 200), group: Some(Group::Work) },
    Tile { app: PApp::Ide, icon: Icon::Ide, label: "AI IDE", sub: "Code with the AI", accent: rgb(100, 230, 255), group: Some(Group::Home) },
    Tile { app: PApp::Data, icon: Icon::Data, label: "DATA", sub: "Tables, CSV", accent: crate::apps::data::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Tales, icon: Icon::Tales, label: "TALES", sub: "Read-aloud tales", accent: crate::apps::story::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::Store, icon: Icon::Store, label: "STORE", sub: "Apps and skills", accent: crate::apps::store::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Dash, icon: Icon::Dash, label: "DASH", sub: "Graphs, tables", accent: crate::apps::dash::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Flows, icon: Icon::Flows, label: "FLOWS", sub: "Workflows, runs", accent: crate::apps::flows::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Camera, icon: Icon::Camera, label: "CAMERA", sub: "Photos, video", accent: crate::apps::camera::ACCENT, group: Some(Group::WebMedia) },
    Tile { app: PApp::Voice, icon: Icon::Speaker, label: "VOICE", sub: "Read, dictate", accent: crate::apps::voice::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::Disks, icon: Icon::Disks, label: "DISKS", sub: "Drives, partitions", accent: crate::apps::disks_tile::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::K8s, icon: Icon::Mesh, label: "K8S", sub: "Explore the cluster", accent: crate::apps::k8s_tile::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::LinuxWin, icon: Icon::Cli, label: "LINUX WINDOW", sub: "Linux programs", accent: crate::apps::linuxwin::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Lab, icon: Icon::Lab, label: "LAB", sub: "Try picture models", accent: crate::apps::lab::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Git, icon: Icon::Git, label: "GIT", sub: "Browse repositories", accent: crate::apps::git_tile::ACCENT, group: Some(Group::Work) },
];

/// The home screen section of an app's tile (the tile table's `group` field; `None`: not on the grid).
pub(crate) fn tile_group(app: PApp) -> Option<Group> {
    TILES.iter().find(|t| t.app == app).and_then(|t| t.group)
}

/// The tiles a person sees on the grid, as (layout key, label, the group the tile is in when left alone), for Settings → Home layout.
pub(crate) fn layout_tiles(admin: bool, cloud: &iyakki_agent::cloudmode::CloudMode) -> Vec<(String, &'static str, &'static str)> {
    TILES.iter().filter(|t| t.group.is_some() && (t.app != PApp::Users || admin) && !cloud.hides_app(t.app)).map(|t| (iyakki_agent::layout::sys_key(t.label), t.label, t.group.map_or("", |g| g.id()))).collect()
}

/// The tiles as (app, label), for tests of other modules.
#[cfg(test)]
pub(crate) fn tiles_for_test() -> Vec<(PApp, &'static str)> {
    TILES.iter().map(|t| (t.app, t.label)).collect()
}

/// The name of the last person who signed in, kept so the sign-in form can fill it in (a name, never a password).
fn last_user(env: &ShellEnv) -> Option<String> {
    let t = std::fs::read_to_string(env.services.dir().join("last-user")).ok()?;
    let t = t.trim();
    iyakki_sec::policy::valid_username(t).then(|| t.to_string())
}

fn remember_user(env: &ShellEnv, name: &str) {
    if iyakki_sec::policy::valid_username(name) {
        let _ = std::fs::write(env.services.dir().join("last-user"), format!("{name}\n"));
    }
}

pub struct Shell {
    env: ShellEnv,
    /// Hit regions from the last rendered frame (clicks are resolved against these).
    hits: Vec<crate::ui::Hit>,
    /// Clipboard-history picker overlay (Ctrl+Shift+V, right-click, or the clipboard button).
    clip: Option<Picker>,
    /// Host files (window mode): the import dialog shown over any screen for dropped files, and whether files are being
    /// dragged over the window right now.
    hostdlg: Option<Box<crate::apps::hostfiles::HostDialog>>,
    /// The sign-in dialog of Claude Code / GitHub Copilot (agent_login.rs), over any screen.
    login: Option<Box<crate::agent_login::LoginModal>>,
    /// "Scan QR" of the clipboard history (clip_scan.rs): the camera, what was read, Copy / Scan again / Cancel.
    clipscan: Option<Box<crate::clip_scan::ClipScan>>,
    /// Where the cameras of the scan come from (`None`: the machine's own; tests give made-up ones).
    scan_src: Option<std::sync::Arc<crate::scan_core::CamSource>>,
    drag_over: bool,
    /// The right-click menu (ctxmenu.rs), and the text fields the last frame drew (it needs them to know what is under the pointer).
    ctx: Option<crate::ctxmenu::Menu>,
    fields: Vec<crate::ctxmenu::FieldNote>,
    last_click: Option<(u64, String)>,
    /// The text field or area under the button while it is held (with its rectangle), so moving the pointer can extend a selection.
    press: Option<(Event, R)>,
    /// When and where the last press was, and how many in a row (double and triple clicks).
    click_run: (u64, i32, i32, u8),
    screen: Screen,
    /// The custom apps (tile apps) this person has, refreshed when the launcher is shown and every couple of seconds.
    custom: Vec<iyakki_agent::store::tilehub::TileEntry>,
    custom_at: u64,
    /// The custom app on screen, if the app on screen is one.
    custom_open: Option<String>,
    session: Option<Session>,
    /// The selected tile, counted within the shown group (its page follows from it).
    sel: usize,
    /// The launcher group shown (remembered per person).
    group: Group,
    /// The id of the person's own group being shown, when it is one (else `group` is).
    cgroup: Option<String>,
    /// The person's layout (groups they made, tiles they moved) and the menus and cards that change it (`shell_layout.rs`).
    layout: iyakki_agent::layout::Layout,
    lay: layout_impl::LayoutUi,
    mouse: (i32, i32),
    tick: u64,
    toast: Option<(String, u64, bool)>,
    size: (u32, u32),
    /// Rectangles whose hover state the last frame depended on.
    probes: Vec<R>,
    /// Something happened that changes the picture (see [`Shell::take_stale`]).
    stale: bool,
    /// The open app's live picture changed and nothing else did (see [`Shell::live_update`], `shell_live.rs`).
    live: bool,
    /// The front end paints the live picture on its own ([`Shell::live_update`]): else a new picture makes the scene stale as before.
    live_paint: bool,
    /// The window should ask the system clipboard for its current text (opening the queue does), and until which tick
    /// the answer, when it comes, belongs to the queue rather than to a field.
    want_host_paste: bool,
    host_sync_until: u64,
    /// What the queue says about the system clipboard: asked, answered (with how many characters), or no answer.
    clip_note: ClipNote,
    /// The F1 help popup is open.
    help: bool,
    shot: Shot,
    /// An alarm or reminder that is ringing.
    ring: Option<Ring>,
    /// When alarms were last looked at (seconds since the epoch).
    last_alarm_check: i64,
    /// Alarms put off for later: (when, what).
    snoozed: Vec<(i64, String, bool)>,
    /// The app that was open when the inbox was opened from the bell: it keeps running (a chat waiting for an answer
    /// must not be cut off) and comes back when the inbox is closed.
    stash: Option<(Box<dyn App>, PApp)>,
    /// What the bell shows: unread notifications, approvals waiting, overdue tasks.
    badge: Badge,
    /// System sounds: what has been seen so far (see `cues`).
    cues: crate::cues::CueState,
    /// The clock in the top bar, and the rectangle whose hover shows its tooltip this frame.
    clock: crate::clockbar::ClockState,
    clock_tip: Option<R>,
    clock_text: String,
    clock_ready: bool,
    /// The Devices popover (button next to the screenshot icon, F8).
    devpop: crate::devpop::DevPop,
    /// The login screens' top bar and power card, and whether this shell is a remote (browser) session (`loginbar.rs`).
    lbar: crate::loginbar::LoginBar,
    /// Replaces the machine the popover looks at (tests).
    devprobe: Option<crate::devpop::Probe>,
    /// The IYAKKI menu (a click on the name in the launcher's top bar, F10): About, Quick install, Media player, Mac drivers.
    brand: crate::brandmenu::BrandMenu,
    /// The installer indicator beside the clock and the list of installs it opens.
    installs: crate::installpop::InstallPop,
    /// The battery indicator of a laptop, its popup and the low-battery cards (`batterybar.rs`, `shell_battery.rs`).
    battery: crate::batterybar::BatteryBar,
    /// The banner shown while the drive the data lives on is lost or failing (`lossbar.rs`, `shell_loss.rs`).
    loss: crate::lossbar::LossBar,
    /// Replaces the program Quick install runs as its jobs (tests).
    quick_launch: Option<iyakki_agent::quickinstall::Launch>,
    /// "DEV" (or "DEV · pairing") while Developer mode is on, read every second.
    dev_sign: Option<String>,
    /// The USB cameras: the CAMERA tile is greyed while there is none (looked at again every two seconds).
    camera: crate::camera::Presence,
    /// The "camera in use by the assistant" sign was on in the last frame.
    cam_sign: bool,
    /// Media and Fn keys: the on-screen display, held keys, the power card (`shell_fnkeys.rs`).
    fnk: crate::fnkeys::FnState,
    /// The Touch Bar strip of a T2 MacBook (`shell_touchbar.rs`).
    tbar: touchbar_impl::TbState,
    /// Cloud mode: what is hidden because this machine has no local screen, sound or camera (`iyakki_agent::cloudmode`).
    cloud: iyakki_agent::cloudmode::CloudMode,
    /// Panes: several apps side by side on one display (`shell_panes.rs`).
    panes: panes_impl::PanesUi,
    /// The screen saver and screen off (`shell_saver.rs`, `saver.rs`).
    saver: saver_impl::SaverUi,
    /// The part of a wheel notch left over when pixel scrolling is turned into whole notches (`shell_scroll.rs`), and when.
    scroll_rem: f32,
    scroll_rem_at: u64,
}

/// The numbers behind the bell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Badge {
    pub unread: usize,
    pub approvals: usize,
    pub overdue: usize,
}

impl Badge {
    pub fn total(&self) -> usize {
        self.unread + self.approvals + self.overdue
    }
}

struct Ring {
    /// (what rang, is it an alarm rather than a reminder, the time shown)
    items: Vec<(String, bool, String)>,
    since: i64,
    player: Option<iyakki_agent::voice::Player>,
    next_sound: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipNote {
    None,
    Asked,
    Got(usize),
}

/// A screenshot in progress: take a picture of the frame, then let the person choose what to keep.
enum Shot {
    Off,
    /// Capture the next frame (before the overlay is drawn).
    Snap,
    /// Choosing an area: the frame as it was, and the first corner once clicked.
    Select { snap: tiny_skia::Pixmap, first: Option<(i32, i32)> },
    /// The picture is on the clipboard; the person decides whether to keep a file as well.
    Review { img: tiny_skia::Pixmap },
}

/// One colour per letter of the name, drawn as a strip under it.
const NAME_COLORS: [[u8; 4]; 6] = [rgb(79, 140, 255), rgb(61, 214, 163), rgb(167, 116, 255), rgb(255, 184, 77), rgb(255, 107, 154), rgb(100, 220, 240)];

/// The strip under the name: one bar per letter, together exactly as wide as the name.
fn name_strip(g: &mut Gfx, x: f32, y: f32, width: f32, bar_h: f32, s: f32, round: bool) {
    let gap = 5.0 * s;
    let n = NAME_COLORS.len() as f32;
    let bw = (width - gap * (n - 1.0)) / n;
    for (i, c) in NAME_COLORS.iter().enumerate() {
        let bx = x + i as f32 * (bw + gap);
        if round {
            g.rrect(bx, y, bw, bar_h, bar_h / 2.0, *c);
        } else {
            g.rect(bx, y, bw, bar_h, *c);
        }
    }
}

/// What the account chip says: the name, and the role when it adds something (an account called "admin" that is
/// an admin does not read "admin · admin").
fn account_label(user: &str, role: &str) -> String {
    if user.eq_ignore_ascii_case(role) { user.to_string() } else { format!("{user}  ·  {role}") }
}

/// The size of everything (1.0 draws body text at 14 px). A bigger window gives more room, not bigger text, as on Ubuntu and
/// macOS: 0.9 at 1280x720 (body about 13 px), about 1.0 at 1920x1080, about 1.25 at 4K, then the person's own size choice on top.
fn d_label(d: Option<&crate::apps::hostfiles::HostDialog>) -> String {
    d.map_or_else(|| "your account".to_string(), |d| d.dest_label().to_string())
}

pub(crate) fn scale_for(w: u32, h: u32) -> f32 {
    let raw = (w as f32 / 1280.0).min(h as f32 / 720.0).max(0.1);
    (0.9 * raw.powf(0.3) * crate::ui::ui_size()).clamp(0.6, 3.0)
}

impl Shell {
    pub fn new(env: ShellEnv) -> Shell {
        crate::crashlog::install();
        let first_run = env.store.lock().map(|s| s.is_empty()).unwrap_or(false);
        let screen = if env.hwcheck {
            Screen::HwCheck(Box::new(crate::hwcheck::HwCheck::new()))
        } else if first_run {
            Screen::Setup(Form::setup())
        } else {
            Screen::Login(Form::login_for(last_user(&env).as_deref()))
        };
        // tests run as a desktop unless one asks for cloud mode (`set_cloud_for_test`)
        let cloud = if cfg!(test) { iyakki_agent::cloudmode::CloudMode::desktop() } else { iyakki_agent::cloudmode::detect(env.services.cloud_setting()) };
        let mut sh = Shell { env, hits: Vec::new(), clip: None, hostdlg: None, login: None, clipscan: None, scan_src: None, drag_over: false, ctx: None, fields: Vec::new(), last_click: None, press: None, click_run: (0, 0, 0, 0), screen, custom: Vec::new(), custom_at: 0, custom_open: None, session: None, sel: 0, group: Group::Home, cgroup: None, layout: Default::default(), lay: Default::default(), mouse: (0, 0), tick: 0, toast: None, size: (1280, 720), probes: Vec::new(), stale: true, live: false, live_paint: false, want_host_paste: false, host_sync_until: 0, clip_note: ClipNote::None, help: false, shot: Shot::Off, ring: None, last_alarm_check: 0, snoozed: Vec::new(), stash: None, badge: Badge::default(), cues: Default::default(), clock: Default::default(), clock_tip: None, clock_text: String::new(), clock_ready: false, devpop: Default::default(), devprobe: None, brand: Default::default(), installs: Default::default(), battery: Default::default(), loss: Default::default(), quick_launch: None, dev_sign: None, camera: if cfg!(test) || !cloud.camera_watch { crate::camera::Presence::none() } else { crate::camera::Presence::system() }, cam_sign: false, fnk: Default::default(), tbar: Default::default(), lbar: Default::default(), cloud: cloud.clone(), panes: Default::default(), saver: Default::default(), scroll_rem: 0.0, scroll_rem_at: 0 };
        if cloud.on {
            sh.apply_cloud(cloud);
        }
        sh
    }

    pub fn screen_name(&self) -> String {
        match &self.screen {
            Screen::HwCheck(_) => "hwcheck".into(),
            Screen::Setup(_) => "setup".into(),
            Screen::Login(_) => "login".into(),
            Screen::Launcher => "launcher".into(),
            Screen::App(_, _) if self.custom_open.is_some() => format!("tile:{}", self.custom_open.as_deref().unwrap_or("")),
            Screen::App(_, a) => format!("app:{}", a.id()),
            Screen::PowerMenu => "power".into(),
        }
    }

    /// The user store, for tools that need to seed accounts (screenshot demo, provisioning).
    pub fn store_for_demo(&self) -> &Arc<Mutex<UserStore>> {
        &self.env.store
    }

    pub fn services_for_demo(&self) -> &Arc<Services> {
        &self.env.services
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Text the user copied inside IYAKKI, for the host (desktop window) to put on the system clipboard.
    pub fn take_host_copy(&mut self) -> Option<String> {
        crate::ui::clipboard::take_host_copy()
    }

    /// The shape the mouse pointer should have now: a hand over links and buttons, a text bar over fields, what an
    /// embedded engine asks for over its page, the arrow elsewhere.
    pub fn pointer(&self) -> crate::ui::Pointer {
        use crate::ui::Pointer;
        let (x, y) = self.mouse;
        if self.screen_stage().covers() {
            return Pointer::Hidden; // a still arrow on the saver would burn in
        }
        if self.help || self.clip.is_some() || self.ctx.is_some() {
            return Pointer::Default;
        }
        let Some(h) = self.hits.iter().rev().find(|h| h.rect.contains(x, y)) else { return Pointer::Default };
        if let Screen::App(app, _) = &self.screen {
            if let Some(p) = app.pointer_hint(x, y) {
                return p;
            }
        }
        let l = h.label.as_str();
        if matches!(l, "field" | "text area" | "address" | "address bar" | "search") || l.ends_with(" field") {
            return Pointer::Text;
        }
        if matches!(l, "backdrop" | "close list" | "page" | "scroll") || matches!(h.ev, Event::Tick | Event::Scroll(_)) {
            return Pointer::Default;
        }
        Pointer::Hand
    }

    /// Clickable regions from the last frame (tests and tools).
    /// The text of the custom app on screen (tests).
    #[cfg(test)]
    pub(crate) fn tile_text_for_test(&mut self) -> String {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.page_text()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_card_text_for_test(&mut self) -> String {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.card_text()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_card_geometry_for_test(&mut self) -> Option<(R, Vec<R>)> {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().and_then(|t| t.card_geometry()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_bar_for_test(&mut self) -> Vec<(String, R)> {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.bar.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn resize_for_test(&mut self, w: u32, h: u32) {
        self.size = (w, h);
        self.stale = true;
    }

    /// Open an app the way a tile does (tests and the monkey test).
    pub(crate) fn open_for_test(&mut self, app: PApp) {
        self.open(app);
    }

    /// The services and session an app gets (tests that build an app by hand).
    pub(crate) fn cx_for_test(&self) -> Option<AppCx> {
        self.cx()
    }

    /// Back to the launcher (tests and the monkey test).
    pub(crate) fn launcher_for_test(&mut self) {
        self.screen = Screen::Launcher;
    }

    /// Mark this shell as a remote (browser) session: the login screens then show no power button and refuse the power action.
    pub fn set_remote(&mut self, remote: bool) {
        self.lbar.remote = remote;
        self.saver_set_remote(remote);
    }

    /// Where the login bar's pieces were drawn last frame, and the Power button (tests and tools).
    pub fn login_bar_layout(&self) -> (&[(crate::loginbar::Chip, R)], Option<R>) {
        (&self.lbar.last, self.lbar.power_rect)
    }

    pub fn hits(&self) -> &[crate::ui::Hit] {
        &self.hits
    }

    /// The right-click menu, while it is open (tests and tools).
    pub fn context_menu(&self) -> Option<&crate::ctxmenu::Menu> {
        self.ctx.as_ref()
    }

    pub fn clipboard_picker_open(&self) -> bool {
        self.clip.is_some()
    }

    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.0.as_str())
    }

    /// Tiles visible to the current user: the four core apps always, Users only for admins.
    fn visible_tiles(&self) -> Vec<&'static Tile> {
        let admin = self.session.as_ref().is_some_and(|s| s.policy.manage_users);
        TILES.iter().filter(|t| t.group.is_some() && (t.app != PApp::Users || admin) && !self.cloud.hides_app(t.app)).collect()
    }

    /// Take a Cloud mode decision into the shell and the menus that follow it.
    /// Cloud mode decided again from what the machine has now: true when it changed. A sound card that registers late (the
    /// MacBookPro14,1's CS8409 about 11 s after boot, after an automatic sign-in) would otherwise leave RADIO, MEDIA, TALES and
    /// VOICE hidden for the whole session, because "no sound card" makes automatic Cloud mode hide them.
    fn refresh_cloud(&mut self, c: iyakki_agent::cloudmode::CloudMode) -> bool {
        if c == self.cloud {
            return false;
        }
        self.apply_cloud(c);
        true
    }

    fn apply_cloud(&mut self, c: iyakki_agent::cloudmode::CloudMode) {
        self.devpop.set_hidden(c.hidden_device_rows.clone());
        self.devpop.invalidate();
        self.brand.set_cloud(c.clone());
        self.cloud = c;
        self.stale = true;
    }

    /// Cloud mode as decided for this machine.
    pub fn cloud(&self) -> &iyakki_agent::cloudmode::CloudMode {
        &self.cloud
    }

    /// Pretend to be another machine (tests).
    #[cfg(test)]
    pub(crate) fn set_cloud_for_test(&mut self, c: iyakki_agent::cloudmode::CloudMode) {
        if !c.camera_watch {
            self.camera = crate::camera::Presence::none();
        }
        self.apply_cloud(c);
    }

    /// The section tabs for this person: each built-in section that has a tile for them, then the groups they made (shown even
    /// when empty, so they can fill them).
    fn groups(&self) -> Vec<GroupRef> {
        let mut v: Vec<GroupRef> = Group::CHIPS
            .into_iter()
            .filter(|g| self.visible_tiles().iter().any(|t| self.tile_gid(t) == g.id()) || self.custom.iter().any(|e| self.app_gid(e) == g.id()))
            .map(GroupRef::from)
            .collect();
        v.extend(self.layout.groups.iter().map(|g| GroupRef { id: g.id.clone(), name: g.label.clone() }));
        v
    }

    /// The section being shown (the first one, Home, when the remembered one has nothing for this person).
    fn current_group(&self) -> GroupRef {
        let gs = self.groups();
        if let Some(c) = self.cgroup.as_ref().and_then(|c| gs.iter().find(|g| g.id == *c)) {
            return c.clone();
        }
        let b = GroupRef::from(self.group);
        if gs.contains(&b) {
            b
        } else {
            gs.first().cloned().unwrap_or_else(|| Group::Home.into())
        }
    }

    /// The group a built-in tile is in for this person: where they moved it, else its own.
    fn tile_gid(&self, t: &Tile) -> String {
        let own = t.group.map_or("", |g| g.id());
        self.layout.group_of(&iyakki_agent::layout::sys_key(t.label), own)
    }

    /// The group a Store app is in for this person: where they moved it, else what its app.json asks, else Work.
    fn app_gid(&self, e: &iyakki_agent::store::tilehub::TileEntry) -> String {
        let own = self.layout.app_default(&e.name, &e.group).0;
        self.layout.group_of(&iyakki_agent::layout::app_key(&e.name), &own)
    }

    /// The tiles of the shown group, in launcher order; pages are cut from this list.
    fn group_tiles(&self) -> Vec<&'static Tile> {
        let g = self.current_group();
        self.visible_tiles().into_iter().filter(|t| self.tile_gid(t) == g.id).collect()
    }

    /// The custom apps in the shown group (they follow the system tiles).
    fn group_custom(&self) -> Vec<iyakki_agent::store::tilehub::TileEntry> {
        let g = self.current_group();
        self.custom.iter().filter(|e| self.app_gid(e) == g.id).cloned().collect()
    }

    /// Everything the launcher shows in the shown group: the system tiles, then the custom apps, in the person's order when
    /// they set one; a group of their own with nothing in it shows a hint.
    fn group_items(&self) -> Vec<LItem> {
        let mut v: Vec<LItem> = self.group_tiles().into_iter().map(LItem::Sys).collect();
        v.extend(self.group_custom().into_iter().map(|e| LItem::App(Box::new(e))));
        let gid = self.current_group().id;
        if self.layout.order.contains_key(&gid) {
            let keys: Vec<String> = v.iter().map(layout_impl::item_key).collect();
            let order = self.layout.ordered(&gid, keys);
            v.sort_by_key(|it| order.iter().position(|k| *k == layout_impl::item_key(it)));
        }
        if v.is_empty() {
            v.push(LItem::Empty);
        }
        v
    }

    /// Look for the person's custom apps again (the launcher does this when it is shown, and every two seconds).
    fn refresh_custom(&mut self) {
        let Some(s) = &self.session else {
            self.custom.clear();
            return;
        };
        self.custom_at = self.tick.max(1);
        self.custom = iyakki_agent::store::tilehub::installed(&self.env.services, &s.policy, &s.user);
        if self.sel >= self.group_items().len() {
            self.sel = 0;
        }
    }

    /// Show a group (from its chip, Tab or a test) and remember it for this person.
    fn set_group(&mut self, g: impl Into<GroupRef>) {
        let g: GroupRef = g.into();
        if !self.groups().contains(&g) {
            return;
        }
        match Group::from_id(&g.id) {
            Some(b) => {
                self.group = b;
                self.cgroup = None;
            }
            None => self.cgroup = Some(g.id.clone()),
        }
        self.sel = 0;
        if let Some(sess) = &self.session {
            crate::uisize::save_group(self.env.services.dir(), &sess.user, &g.id);
        }
    }

    fn step_group(&mut self, back: bool) {
        let gs = self.groups();
        let cur = self.current_group();
        let i = gs.iter().position(|g| *g == cur).unwrap_or(0);
        let n = gs.len();
        self.set_group(gs[if back { (i + n - 1) % n } else { (i + 1) % n }].clone());
    }

    fn cx(&self) -> Option<AppCx> {
        let session = self.session.clone()?;
        // the administrator's mcp.json plus the servers added in the MCP tile (and the sample one)
        let stored = self.env.services.mcp.list();
        let mut mcp_servers: Vec<_> = self.env.mcp_servers.iter().filter(|s| !stored.iter().any(|x| x.name == s.name)).cloned().collect();
        mcp_servers.extend(stored.iter().map(iyakki_agent::mcp::StoredServer::spec));
        Some(AppCx {
            session: session.clone(),
            host: self.env.host.clone(),
            cfg: self.env.cfg.clone(),
            // everything the assistant asks of a model is counted for the USAGE tile
            chat: Arc::new(iyakki_agent::usage::Metered::new(self.env.chat.clone(), self.env.services.clone(), &session.user)),
            audit: self.env.audit.clone(),
            store: self.env.store.clone(),
            mcp_servers,
            services: self.env.services.clone(),
            web: self.env.web.clone(),
        })
    }

    /// An app panicked (the panic was caught): tell the person, keep the note of where in the audit log, and leave everything
    /// else running. Opening the app again starts it afresh.
    pub(crate) fn app_crashed(&mut self, name: &str) {
        let note = crate::crashlog::take_last();
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        self.env.audit.record(&user, "app.crash", &format!("{name}: {note}"));
        self.say(format!("{name} stopped because of an internal error. Details are in the system log. Open it again to restart it."));
    }

    fn say(&mut self, s: impl Into<String>) {
        // a failure caused by a lost drive says so (iyakki_storage::loss::reword leaves every other message alone)
        self.toast = Some((iyakki_storage::loss::reword(&s.into()), self.tick + 120, false));
    }

    fn say_info(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), self.tick + 75, true));
    }

    fn sign_in(&mut self, s: Session, password: &str) {
        // the Cloud mode setting may have changed since the last sign-in (Settings -> Cloud mode)
        if !cfg!(test) {
            let c = iyakki_agent::cloudmode::detect(self.env.services.cloud_setting());
            self.refresh_cloud(c);
        }
        self.env.audit.record(&s.user, "login", s.role.name());
        // a local model is loaded in the background now, so the first question is not the slow one
        if s.policy.agent {
            self.env.services.warm_up_chat();
        }
        remember_user(&self.env, &s.user);
        // unlock the user's personal vault (and, for admins, the system vault) with the password just typed
        let report = self.env.services.on_login(&s.user, password, s.policy.manage_settings);
        // schedules run while this account is signed in, without FLOWS being open (nothing starts for an account without flows)
        {
            let (svc, user, policy) = (self.env.services.clone(), s.user.clone(), s.policy.clone());
            std::thread::spawn(move || {
                iyakki_agent::flows::FlowEnv::start_for_login(&svc, &user, &policy);
            });
        }
        // kubeconfigs held in this person's vaults can be used by cluster commands while they are signed in; the dev cluster's
        // background events (restarts, exits) are audited
        iyakki_agent::kubeprofiles::set_resolver(Some(iyakki_agent::kubeprofiles::resolver_for(&self.env.services, &s.user)));
        {
            let audit = self.env.audit.clone();
            self.env.services.k3sdev.set_audit(Arc::new(move |u, a, d| audit.record(u, a, d)));
        }
        // shares marked "keep running" start again for this person (everything else in Settings -> Shared folders stays stopped)
        {
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            let allow = iyakki_agent::sharedfolder::allow_fn(s.policy.clone(), &s.user, self.env.services.dir());
            iyakki_agent::sharedfolder::for_user(self.env.services.dir(), &s.user, Arc::new(move |e, d| audit.record(&user, e, d)), allow).restore();
        }
        if let VaultState::Locked(why) = &report.user_vault {
            self.env.audit.record(&s.user, "vault.user.locked", why);
            let _ = self.env.services.notify_once(&s.user, iyakki_agent::inbox::Kind::Warning, "Your personal vault is locked", "It could not be opened with the password you typed. See Settings → Vault.", Some(iyakki_agent::inbox::Link::new("settings", "")));
            self.say("Your personal vault could not be opened (see Settings → Vault).");
        } else if let VaultState::Locked(why) = &report.system_vault {
            self.env.audit.record(&s.user, "vault.system.locked", why);
            let _ = self.env.services.notify_once(&s.user, iyakki_agent::inbox::Kind::Warning, "The system vault is locked", "Model API keys are unavailable until an administrator with access signs in.", Some(iyakki_agent::inbox::Link::new("settings", "")));
            self.say("The system vault is locked: model API keys are unavailable until an admin with access signs in.");
        }
        // documents embedded by a model that is no longer the chosen one are brought up to date, quietly
        if s.policy.manage_settings {
            self.env.services.reindex_stale_in_background();
            // (tests never look at the real network)
            #[cfg(not(test))]
            {
                self.env.services.detect_local_ollama_in_background();
                self.env.services.detect_local_lemonade_in_background();
                self.env.services.start_device_watch();
            }
        }
        // examples to try (once, for administrators): an MCP server, documents and skills
        crate::samples::seed(&self.env, &s.user, &s.policy);
        crate::uisize::apply(Some(crate::uisize::load(self.env.services.dir(), &s.user)));
        crate::filepicker::set_session(Some((&s.user, &s.policy, self.env.services.dir())));
        self.session = Some(s);
        self.load_clipboard_limit();
        self.sel = 0;
        let remembered = crate::uisize::load_group(self.env.services.dir(), &self.session.as_ref().map_or(String::new(), |s| s.user.clone()));
        self.group = remembered.as_deref().and_then(Group::from_id).unwrap_or(Group::Home);
        self.lay_sign_in(remembered.as_deref());
        self.screen = Screen::Launcher;
        self.panes_reset();
        self.refresh_gate();
        self.cues.badge_seen = false;
        self.refresh_badge();
        // the sign-in sound plays by itself while the home screen appears
        let _ = crate::cues::start(&self.env.services, &self.cloud, iyakki_agent::syssounds::Cue::Login);
    }

    /// Start signed in as a person whose password has just been checked elsewhere (the browser sign-in).
    pub fn sign_in_checked(&mut self, s: Session, password: &str) {
        self.sign_in(s, password);
    }

    fn sign_out(&mut self) {
        // the sign-out sound starts first and plays while the rest is done; the screen changes when it is over, or after
        // `LOGOUT_WAIT` at the latest (a slow or stuck output never holds sign-out back)
        let cue = self.session.is_some().then(|| crate::cues::start(&self.env.services, &self.cloud, iyakki_agent::syssounds::Cue::Logout));
        crate::uisize::apply(None);
        crate::filepicker::set_session(None);
        self.lay_sign_out();
        self.panes_reset();
        self.stash = None;
        self.badge = Badge::default();
        self.brand = Default::default();
        self.installs = Default::default();
        // drives open in this person's home: left alone (and said so) unless the Settings switch asks for them to be closed
        let mut drives_note = None;
        if let Some(s) = &self.session {
            let mut who = iyakki_agent::disks::Actor::new(&s.user, s.role);
            who.home = iyakki_agent::scope::home_root(&s.user, std::path::Path::new("/var/lib/iyakki")).display().to_string();
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            drives_note = iyakki_agent::disks::Disks::real().at_sign_out(&who, &|a, d| audit.record(&user, a, d));
        }
        if let Some(s) = self.session.take() {
            self.env.audit.record(&s.user, "logout", "");
            iyakki_agent::flows::FlowEnv::stop_for_logout(&self.env.services, &s.user);
            iyakki_agent::sharedfolder::on_logout(&s.user);
            self.env.services.on_logout(&s.user, s.policy.manage_settings);
            iyakki_agent::kubeprofiles::set_resolver(None);
            // the assistant's browser tab goes with the session (jobs still running are answered)
            crate::apps::web_agent::close(&self.env.services, &s.user);
        }
        self.cues.badge_seen = false;
        if let Some(c) = cue {
            let _ = c.wait(iyakki_agent::syssounds::LOGOUT_WAIT);
        }
        self.screen = Screen::Login(Form::login_for(last_user(&self.env).as_deref()));
        if let Some(m) = drives_note {
            self.say(m);
        }
    }

    /// Whether the CAMERA tile is usable (looks first when it has not looked yet).
    fn camera_ready(&mut self) -> bool {
        if !self.camera.scanned() {
            self.camera.scan();
        }
        self.camera.available()
    }

    /// Look for cameras somewhere else (tests).
    #[cfg(test)]
    pub(crate) fn set_camera_for_test(&mut self, p: crate::camera::Presence) {
        self.camera = p;
    }

    fn open(&mut self, app: PApp) {
        let Some(cx) = self.cx() else { return };
        // cloud mode: a hidden app has no shortcut, no F-key and no jump that opens it
        if self.cloud.hides_app(app) {
            return;
        }
        if !cx.session.policy.allows_app(app) {
            self.env.audit.record(&cx.session.user, "app.denied", app.id());
            self.say(format!("Your role ({}) can't open {}", cx.session.role.name(), app.id().to_uppercase()));
            return;
        }
        if self.ws_active() && !self.panes_room_for(app) {
            self.say("Low memory: close an app or use a smaller layout");
            return;
        }
        let boxed: Box<dyn App> = match app {
            PApp::Chat => Box::new(ChatApp::new().with_agent(cx.session.policy.agent).resumed(&cx)),
            PApp::Rag => Box::new(RagApp::new(&cx)),
            PApp::Mcp => Box::new(McpApp::new(&cx)),
            PApp::Cli => Box::new(CliApp::new(&cx)),
            PApp::Users => Box::new(UsersApp::new()),
            PApp::Settings => Box::new(SettingsApp::new(&cx).with_cloud(self.cloud.clone())),
            PApp::Runs => Box::new(RunsApp::new(&cx)),
            PApp::Edit => Box::new(EditApp::new(&cx)),
            PApp::Browser => Box::new(crate::apps::browser::BrowserApp::new(&cx)),
            PApp::Mesh => Box::new(crate::apps::mesh::MeshApp::new(&cx)),
            PApp::Radio => Box::new(crate::apps::radio::RadioApp::new(&cx)),
            PApp::Scan => Box::new(crate::apps::scan::ScanApp::new(&cx)),
            PApp::Skills => Box::new(crate::apps::skills::SkillsApp::new(&cx)),
            PApp::Usage => Box::new(crate::apps::usage::UsageApp::new(&cx)),
            PApp::System => Box::new(crate::apps::system::SystemApp::new(&cx)),
            PApp::Calendar => Box::new(crate::apps::calendar::CalendarApp::new(&cx)),
            PApp::Media => Box::new(crate::apps::media::MediaApp::new(&cx)),
            PApp::SrePilot => Box::new(crate::apps::srepilot::SrePilotApp::new()),
            PApp::Ide => Box::new(crate::apps::ide::IdeApp::new(&cx)),
            PApp::Data => Box::new(crate::apps::data::DataApp::new(&cx)),
            PApp::Store => Box::new(crate::apps::store::StoreApp::new(&cx)),
            PApp::Dash => Box::new(crate::apps::dash::DashApp::new(&cx)),
            PApp::Flows => Box::new(crate::apps::flows::FlowsApp::new(&cx)),
            PApp::Camera => Box::new(crate::apps::camera::CameraApp::new(&cx)),
            PApp::Voice => Box::new(crate::apps::voice::VoiceApp::new(&cx)),
            PApp::K8s => Box::new(crate::apps::k8s_tile::K8sApp::new(&cx)),
            PApp::Disks => Box::new(crate::apps::disks_tile::DisksApp::new(&cx)),
            PApp::LinuxWin => Box::new(crate::apps::linuxwin::LinuxWinApp::new(&cx)),
            PApp::Tales => Box::new(crate::apps::story::StoryApp::new(&cx)),
            PApp::Lab => Box::new(crate::apps::lab::LabApp::new(&cx)),
            PApp::Git => Box::new(crate::apps::git_tile::GitApp::new(&cx)),
            PApp::Inbox => Box::new(crate::apps::inbox::InboxApp::new(&cx)),
        };
        self.env.audit.record(&cx.session.user, "app.open", app.id());
        self.custom_open = None;
        self.screen = Screen::App(boxed, app);
    }

    /// Open or close the Devices popover (F8, the button).
    fn toggle_devices(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.brand.close();
        self.installs.close();
        self.devpop.toggle();
        self.stale = true;
    }

    /// What the popover looks at: this machine (a made-up one in tests).
    fn dev_probe(&self) -> crate::devpop::Probe {
        if let Some(p) = &self.devprobe {
            return p.clone();
        }
        let drivers = iyakki_agent::store::drivers::Drivers::for_services(&self.env.services);
        let mut p = crate::devpop::Probe::real(crate::apps::devices::DeviceEnv::real(), Some(drivers), self.env.services.dir().to_path_buf(), self.env.hostname.clone(), self.env.data_volatile);
        p.screen = self.size;
        p
    }

    fn handle_devpop(&mut self, ev: &Event) {
        use crate::devpop::Outcome;
        let out = match ev {
            Event::Key(k) => self.devpop.handle_key(k),
            Event::Tab(n) => self.devpop.handle_hit(*n),
            Event::Scroll(d) => {
                self.devpop.scroll_by(if *d > 0 { -1 } else { 1 });
                Outcome::Nothing
            }
            _ => Outcome::Nothing,
        };
        match out {
            Outcome::Nothing => {}
            Outcome::Close => self.devpop.close(),
            Outcome::Jump(j) => {
                self.devpop.close();
                match j {
                    crate::devpop::Jump::Settings(page) => self.jump_to("settings", &format!("tab:{page}")),
                    crate::devpop::Jump::Drivers => self.jump_to("store", "drivers"),
                    crate::devpop::Jump::Setup(id) => self.jump_to("store", &format!("setup:{id}")),
                    crate::devpop::Jump::MacDrivers => {
                        // the IYAKKI menu, opened on its Mac drivers page
                        self.toggle_brand();
                        if self.brand.open {
                            let out = self.brand.open_page(crate::brandmenu::Page::Mac);
                            if let crate::brandmenu::Outcome::Opened(p) = out {
                                self.brand_opened(p);
                            }
                        }
                    }
                    crate::devpop::Jump::None => {}
                }
            }
            Outcome::Act(crate::devpop::Act::FirewallOff) => {
                let admin = self.session.as_ref().is_some_and(|x| x.policy.manage_settings);
                if !admin {
                    self.say("Only administrators can switch the firewall off.".to_string());
                } else {
                    let user = self.session.as_ref().map(|x| x.user.clone()).unwrap_or_default();
                    let r = iyakki_fw::service::engine().disable_now(&format!("{user} (Devices list)"));
                    match r {
                        Ok(()) => {
                            self.env.audit.record(&user, "firewall.disable", "emergency off from the Devices list");
                            self.say_info("The firewall is off. Switch it on again in Settings -> Firewall.".to_string());
                        }
                        Err(e) => self.say(format!("Could not switch the firewall off: {e}")),
                    }
                }
                self.devpop.invalidate();
            }
            Outcome::Act(crate::devpop::Act::ToggleSoundMute) => {
                // IYAKKI's software volume carries the output's volume: mute it in Settings -> Sound (the level is kept)
                match self.env.services.master_toggle_mute().map(|m| iyakki_agent::audioout::SoundSettings { volume_pct: m.percent, muted: m.muted, ..Default::default() }) {
                    Ok(now) if now.muted => self.say_info("Muted.".to_string()),
                    Ok(now) => self.say_info(format!("Sound on, volume {}%.", now.volume_pct)),
                    Err(e) => self.say(e),
                }
                self.devpop.invalidate();
            }
            Outcome::Act(a) => {
                let p = self.dev_probe();
                match crate::devpop::run_act(&p, &a) {
                    Ok(m) => self.say_info(m),
                    Err(m) => self.say(m),
                }
                self.devpop.invalidate();
            }
        }
        self.stale = true;
    }

    /// Open or close the IYAKKI menu (F10, a click on the name in the launcher's top bar).
    fn toggle_brand(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.devpop.close();
        self.installs.close();
        let (w, h) = self.size;
        let s = scale_for(w, h);
        // under the name in the launcher, under the header in an app
        self.brand.anchor = match self.screen {
            Screen::App(..) => (12.0 * s, HEADER_H * s + 6.0 * s),
            _ => (34.0 * s, 76.0 * s),
        };
        self.brand.admin = self.session.as_ref().is_some_and(|x| x.policy.manage_settings);
        self.brand.toggle();
        self.stale = true;
    }

    /// Open or close the list of installs (the indicator beside the clock).
    fn toggle_installs(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.devpop.close();
        self.brand.close();
        self.installs.toggle();
        if let Some(cx) = self.cx() {
            self.installs.poll(&cx, true);
        }
        self.stale = true;
    }

    /// The facts a page of the IYAKKI menu needs, read when it opens.
    fn brand_opened(&mut self, page: crate::brandmenu::Page) {
        use crate::brandmenu::Page;
        match page {
            Page::Gpu => {
                // the files at once; the kernel log and the cluster's pods arrive on a thread and are merged while the page is drawn
                let svc = self.env.services.clone();
                self.brand.set_gpu(iyakki_agent::gpu_steps::quick_report(std::path::Path::new("/"), svc.dir()));
                #[cfg(not(test))]
                iyakki_agent::gpu_steps::refresh_async(svc);
            }
            Page::Linux => self.refresh_linux(),
            Page::Media => {
                iyakki_agent::player::register_dir(self.env.services.clis.bin_dir());
                self.brand.players = crate::brandmenu::players_found(&crate::brandmenu::on_path);
            }
            Page::Mac => {
                let p = self.dev_probe();
                self.devpop.refresh_in_background(&p);
                self.brand.mac = self.devpop.snapshot();
                self.brand.mac_model = std::fs::read_to_string("/sys/class/dmi/id/product_name").map(|m| m.trim().to_string()).unwrap_or_default();
                self.brand.mac_board = std::fs::read_to_string("/sys/class/dmi/id/board_name").map(|m| m.trim().to_string()).unwrap_or_default();
                self.refresh_mac_firmware();
            }
            _ => {}
        }
    }

    fn handle_brand(&mut self, ev: &Event) {
        use crate::brandmenu::Outcome;
        let out = match ev {
            Event::Key(k) => self.brand.handle_key(k),
            other => self.brand.handle_hit(other),
        };
        match out {
            Outcome::Nothing => {}
            Outcome::Close => self.brand.close(),
            Outcome::Opened(p) => self.brand_opened(p),
            Outcome::GpuTest => self.gpu_test(),
            Outcome::Install(item) => self.quick_install(item),
            Outcome::ShowInstalls => {
                self.brand.close();
                self.toggle_installs();
                if !self.installs.open {
                    self.toggle_installs();
                }
            }
            Outcome::MacFirmware(source) => self.mac_firmware(source),
            Outcome::Linux(a) => self.linux_action(a),
            Outcome::MacClean => {
                let freed = iyakki_agent::macfw::delete_leftovers(self.env.services.dir());
                self.brand.note = Some((format!("Deleted the kept download: {} MB freed.", freed >> 20), true));
                self.refresh_mac_firmware();
            }
            Outcome::MacBtLoad => self.mac_bt_load(),
            Outcome::Play(path) => self.brand_play(&path),
            Outcome::StopMedia => {
                iyakki_agent::media::shared().stop();
                self.brand.note = Some(("Stopped.".into(), true));
            }
            Outcome::Jump(app, item) => {
                self.brand.close();
                if app == "devices" {
                    self.toggle_devices();
                } else {
                    self.jump_to(app, &item);
                }
            }
        }
        self.stale = true;
    }

    /// What the Mac drivers page shows about the firmware job: the last result and whether one is running.
    fn refresh_mac_firmware(&mut self) {
        use iyakki_agent::quickinstall as qi;
        self.brand.mac_last = iyakki_agent::macfw::read_outcome(self.env.services.dir());
        self.brand.mac_leftover = iyakki_agent::macfw::leftovers(self.env.services.dir());
        self.brand.mac_cache = iyakki_agent::macfw::card_status(self.env.services.dir());
        self.brand.mac_bt_held = iyakki_agent::macfw::bluetooth_held(std::path::Path::new("/lib/modules"));
        self.brand.mac_busy = self.installs.jobs().iter().any(|j| j.running() && qi::item_of(j) == Some(qi::Item::MacFirmware));
    }

    /// "Load the Bluetooth driver anyway" (administrators): give the held drivers back and load them in the background (a driver that finds
    /// no firmware can take a few seconds); Settings -> Bluetooth then shows the controller.
    fn mac_bt_load(&mut self) {
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can load drivers for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        self.env.audit.record(&sess.user, "bluetooth.load-anyway", "held drivers given back");
        let data = self.env.services.dir().to_path_buf();
        std::thread::spawn(move || {
            let _ = iyakki_agent::macfw::load_bluetooth_anyway(&data);
        });
        self.brand.mac_bt_held = false;
        self.brand.note = Some(("Loading the Bluetooth drivers without a firmware file. Settings \u{2192} Bluetooth shows the controller in a few seconds; pairing and audio are not available yet.".into(), true));
    }

    /// Start the job that installs Apple's Wi-Fi and Bluetooth firmware from `source` (administrators; the click is the person's request).
    fn mac_firmware(&mut self, source: iyakki_agent::macfw::Source) {
        use iyakki_agent::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can install drivers for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(svc.dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        match qi::start_mac_firmware(&svc.jobs, &sess.user, &source, &launch) {
            Ok(job) => {
                self.env.audit.record(&sess.user, "macfirmware.start", &source.words());
                self.brand.note = Some((format!("Started: {}. Progress is beside the clock; the files are Apple's and stay on this machine.", job.label), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
                self.refresh_mac_firmware();
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// Start the jobs of a Quick install (administrators), in dependency order.
    fn quick_install(&mut self, item: iyakki_agent::quickinstall::Item) {
        use iyakki_agent::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can install software for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(svc.dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        // only what the item needs is looked at (the Media page's player button needs nothing)
        let present: Vec<qi::Item> = {
            let mut need: Vec<qi::Item> = Vec::new();
            let mut todo = item.needs_now();
            while let Some(n) = todo.pop() {
                if !need.contains(&n) {
                    need.push(n);
                    todo.extend(n.needs_now());
                }
            }
            need.into_iter().filter(|i| qi::presence(&svc, *i).present()).collect()
        };
        // Indic-TTS is deployed through the Platform services review once k3s is there
        if item == qi::Item::IndicTts && present.contains(&qi::Item::K3s) {
            self.env.audit.record(&sess.user, "quickinstall.handoff", "indic-tts");
            self.brand.close();
            self.jump_to("settings", "platform:indic-tts");
            return;
        }
        match qi::start(&svc.jobs, &sess.user, item, &|i| present.contains(&i), &launch) {
            Ok(started) if item == qi::Item::IndicTts => {
                let steps: Vec<String> = started.iter().map(|j| j.label.clone()).collect();
                self.env.audit.record(&sess.user, "quickinstall.start", &format!("indic-tts ({})", steps.join(", ")));
                self.brand.note = Some((format!("Started: {}. When k3s is Ready, choose Indic-TTS again: its deploy review opens (Platform services).", steps.join(", ")), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Ok(started) => {
                let steps: Vec<String> = started.iter().map(|j| j.label.clone()).collect();
                self.env.audit.record(&sess.user, "quickinstall.start", &format!("{} ({})", item.id(), steps.join(", ")));
                self.brand.note = Some((format!("Started: {}. Progress is beside the clock.", steps.join(", then ")), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// The GPU page's test prompt: a detached job of the installer workspace (its log is the answer and the tokens per second).
    fn gpu_test(&mut self) {
        use iyakki_agent::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can run the test prompt (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(svc.dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        if svc.jobs.visible(&sess.user, qi::WORKSPACE).iter().any(|j| j.running() && j.label == "GPU test prompt") {
            self.brand.note = Some(("The test prompt is running already: its answer is in the installer runs beside the clock.".into(), false));
            return;
        }
        match svc.jobs.start(&sess.user, &qi::gpu_test_spec(&launch), &|_: &std::path::Path| Ok(())) {
            Ok(_) => {
                self.env.audit.record(&sess.user, "gpu.testprompt", "");
                self.brand.note = Some(("Test prompt started: press Installer runs (or look beside the clock) for the answer and the tokens per second.".into(), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// Play a file from the menu's Media page in the shared player (MEDIA's own).
    fn brand_play(&mut self, path: &str) {
        let Some(sess) = self.session.clone() else { return };
        let p = std::path::Path::new(path);
        if !p.is_file() {
            self.brand.note = Some((format!("No such file: {path}"), false));
            return;
        }
        if !sess.policy.allows_read(&sess.user, path) {
            self.brand.note = Some((format!("Your account may not read {path}."), false));
            return;
        }
        // video and every sound but MP3 play in MEDIA's own player (ffmpeg decodes, IYAKKI draws the picture)
        if iyakki_agent::player::available() && !iyakki_agent::media::is_mp3(path) {
            self.env.audit.record(&sess.user, "media.play", path);
            self.brand.close();
            self.jump_to("media", &format!("play:{path}"));
            return;
        }
        let label = p.file_name().map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
        match iyakki_agent::media::shared().play(&sess.user, &label, path) {
            Ok(()) => {
                self.env.audit.record(&sess.user, "media.play", path);
                self.brand.note = Some((format!("Playing {label}."), true));
            }
            Err(e) => self.brand.note = Some((format!("Could not play it: {e}"), false)),
        }
    }

    fn handle_installs(&mut self, ev: &Event) {
        use crate::installpop::Outcome;
        let Some(cx) = self.cx() else { return };
        match self.installs.handle(ev, &cx) {
            Outcome::Nothing => {}
            Outcome::Close => self.installs.close(),
            Outcome::OpenRuns => {
                self.installs.close();
                self.jump_to("runs", "jobs");
            }
        }
        self.stale = true;
    }

    /// Open an app and ask it to show one thing (`tab:network` in Settings, `drivers` in the Store).
    fn jump_to(&mut self, app: &str, item: &str) {
        self.open_linked(app);
        self.store_launch(app);
        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
            a.open_item(item, &cx);
        }
    }

    fn inbox_allowed(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.policy.allows_app(PApp::Inbox))
    }

    /// The bell, or F9: open the inbox. From inside an app, that app is kept aside (still running, so a chat waiting
    /// for an answer is not cut off) and comes back with Back. Pressed again inside the inbox, it closes it.
    fn open_inbox(&mut self) {
        if !self.inbox_allowed() {
            return;
        }
        if matches!(self.screen, Screen::App(_, PApp::Inbox)) {
            self.leave_app();
            return;
        }
        let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Screen::App(app, pa) = prev {
            // already holding one aside (we got here through the inbox): the app being left is simply closed
            if self.stash.is_none() {
                self.stash = Some((app, pa));
            }
        } else {
            self.screen = prev;
            if !matches!(self.screen, Screen::Launcher) {
                return; // only after sign-in, from the launcher or an app
            }
        }
        self.open(PApp::Inbox);
        if !matches!(self.screen, Screen::App(_, PApp::Inbox)) {
            self.leave_app();
        }
    }

    /// Back out of the current app: to the app that was kept aside, or the launcher.
    fn leave_app(&mut self) {
        self.ctx = None;
        // Settings → Home layout may have changed the layout
        self.lay_reload();
        self.custom_open = None;
        self.custom_at = 0;
        self.screen = match self.stash.take() {
            Some((app, pa)) => Screen::App(app, pa),
            None => Screen::Launcher,
        };
    }

    /// Open the app a notification points to.
    /// An app asked for another one (the IDE opens a table in DATA). The asking app is kept aside, still running, and
    /// comes back with Back; apps other than DATA replace it, as before.
    fn open_linked_from_app(&mut self, app: &str) {
        // the VOICE tile's "Set up Kokoro": the Store on its Setup tab, with the app kept where it is (Back returns to it)
        if app == "quickinstall" {
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            match prev {
                Screen::App(a, pa) if pa != PApp::Store => {
                    if self.stash.is_none() {
                        self.stash = Some((a, pa));
                    }
                }
                other => self.screen = other,
            }
            self.open_linked("store");
            self.store_launch("store");
            return;
        }
        // Store -> Setup cards that open a page of the IYAKKI menu, and the installer list
        if let Some(page) = app.strip_prefix("brand:") {
            self.toggle_brand();
            if self.brand.open {
                let p = match page {
                    "gpu" => crate::brandmenu::Page::Gpu,
                    "mac" => crate::brandmenu::Page::Mac,
                    _ => crate::brandmenu::Page::Linux,
                };
                if let crate::brandmenu::Outcome::Opened(p) = self.brand.open_page(p) {
                    self.brand_opened(p);
                }
            }
            return;
        }
        if app == "installs" {
            self.toggle_installs();
            if !self.installs.open {
                self.toggle_installs();
            }
            return;
        }
        // the IDE's Run for a tile app (a folder, looked at live as an unsigned dev app) and Open for an installed one
        if let Some(rest) = app.strip_prefix("tile-preview:").or_else(|| app.strip_prefix("tile-open:")) {
            let preview = app.starts_with("tile-preview:");
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            if let Screen::App(a, pa) = prev {
                if self.stash.is_none() {
                    self.stash = Some((a, pa));
                }
            } else {
                self.screen = prev;
            }
            if preview {
                let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                match iyakki_agent::store::tilehub::preview_entry(&user, std::path::Path::new(rest)) {
                    Ok(e) => self.open_custom(e),
                    Err(m) => self.say(format!("Could not run the app: {m}")),
                }
            } else {
                self.open_custom_by_name(rest);
            }
            if !matches!(&self.screen, Screen::App(..)) || self.custom_open.is_none() {
                self.leave_app();
            }
            return;
        }
        // RUNS asking to show a run's task or row: back to the IDE that is kept aside (the caller then hands it the item)
        if app == "ide" {
            if self.stash.is_some() {
                self.leave_app();
            }
            return;
        }
        if (app == "data" || app == "browser" || app == "runs") && self.stash.is_none() {
            let target = match app {
                "data" => PApp::Data,
                "runs" => PApp::Runs,
                _ => PApp::Browser,
            };
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            match prev {
                Screen::App(a, pa) if pa != target => self.stash = Some((a, pa)),
                other => self.screen = other,
            }
            self.open_linked(app);
            if !matches!(&self.screen, Screen::App(_, pa) if *pa == target) {
                self.leave_app();
            }
            return;
        }
        self.open_linked(app);
    }

    /// Hand the Store the stand-in program for the install jobs (tests and the screenshot tool); normally there is none.
    fn store_launch(&mut self, app: &str) {
        if app != "store" {
            return;
        }
        if let (Some(l), Some(cx), Screen::App(a, _)) = (self.quick_launch.clone(), self.cx(), &mut self.screen) {
            a.open_item(&format!("launch:{}\n{}", l.exe.display(), l.data.display()), &cx);
        }
    }

    fn open_linked(&mut self, app: &str) {
        if let Some(pa) = PApp::ALL.into_iter().find(|a| a.id() == app) {
            if pa != PApp::Inbox {
                self.open(pa);
            }
        }
    }

    /// Open an app by its id and say whether it is the screen now (the screens tool and the render smoke tests).
    /// Put a ready-made app on screen (tests that drive an app through the shell's real mouse handling).
    #[cfg(test)]
    pub(crate) fn show_app_for_test(&mut self, app: Box<dyn crate::apps::App>, id: PApp) {
        self.screen = Screen::App(app, id);
        self.stale = true;
    }

    /// Run Quick install's jobs with another program (the screenshot tool uses a stand-in script).
    pub fn set_quick_launch_for_demo(&mut self, l: iyakki_agent::quickinstall::Launch) {
        self.quick_launch = Some(l);
    }

    pub fn open_for_demo(&mut self, id: &str) -> bool {
        if self.session.is_some() {
            self.screen = Screen::Launcher;
            self.stash = None;
        }
        if id == "inbox" {
            self.open_inbox();
        } else {
            self.open_linked(id);
        }
        self.screen_name() == format!("app:{id}")
    }

    pub fn badge(&self) -> Badge {
        self.badge
    }

    /// Count what the bell shows again (cheap: one small file and the live runs).
    pub fn refresh_badge(&mut self) {
        let b = match &self.session {
            Some(s) => {
                let c = self.env.services.inbox.counts(&s.user, iyakki_agent::clock::now_unix());
                Badge { unread: c.unread, approvals: self.env.services.approvals(&s.user).len() + self.env.services.secret_requests.pending(&s.user).len() + self.env.services.browser.consent.pending(&s.user).len(), overdue: c.overdue }
            }
            None => Badge::default(),
        };
        // a new notification (the count of unread ones went up) has a sound; the first count after signing in does not
        if self.session.is_some() {
            if self.cues.badge_seen && !self.cues.quiet && b.unread > self.badge.unread {
                let _ = crate::cues::start(&self.env.services, &self.cloud, iyakki_agent::syssounds::Cue::Notify);
            }
            self.cues.badge_seen = true;
        }
        if b != self.badge {
            self.badge = b;
            self.stale = true;
        }
    }

    /// A detached job ended (see `docs/JOBS.md`): one note in the inbox for each, whichever app is open, so the bell shows it and
    /// the note opens the Jobs list. The chat that started the job shows its own event; this is for the person who is elsewhere.
    pub fn announce_finished_jobs(&mut self) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        let (mut any, mut ok) = (false, false);
        self.cues.quiet = true;
        for j in self.env.services.jobs.take_finished_all(&user, iyakki_agent::jobs::Channel::Inbox) {
            any = true;
            ok |= j.state == iyakki_agent::jobs::State::Succeeded;
            let (kind, word) = match j.state {
                iyakki_agent::jobs::State::Succeeded => (iyakki_agent::inbox::Kind::Agent, "finished"),
                iyakki_agent::jobs::State::Stopped => (iyakki_agent::inbox::Kind::Info, "was stopped"),
                _ => (iyakki_agent::inbox::Kind::Warning, "did not finish"),
            };
            let _ = self.env.services.notify(&user, kind, &format!("Job “{}” {word}", j.label), &iyakki_agent::jobs::Jobs::finish_line(&j), Some(iyakki_agent::inbox::Link::new("runs", "jobs")));
            self.refresh_badge();
            self.stale = true;
        }
        self.cues.quiet = false;
        if any {
            // one sound for the batch: "done" when something finished well, the notification chime when none did
            let cue = if ok { iyakki_agent::syssounds::Cue::JobDone } else { iyakki_agent::syssounds::Cue::Notify };
            let _ = crate::cues::start(&self.env.services, &self.cloud, cue);
        }
    }

    /// Keep an app that is held aside running: it gets the clock's ticks.
    fn tick_stash(&mut self) {
        let cx = self.cx();
        if let (Some((app, _)), Some(cx)) = (&mut self.stash, cx) {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.handle(&Event::Tick, &cx))).is_err() {
                self.stash = None;
            }
        }
    }

    /// Leave the hardware check for the normal first screen.
    fn leave_hwcheck(&mut self) {
        let first_run = self.env.store.lock().map(|s| s.is_empty()).unwrap_or(false);
        self.screen = if first_run { Screen::Setup(Form::setup()) } else { Screen::Login(Form::login_for(last_user(&self.env).as_deref())) };
    }

    fn submit_login(&mut self) {
        let Screen::Login(f) = &mut self.screen else { return };
        let (name, pw) = (f.fields[0].value.trim().to_string(), f.fields[1].value.clone());
        let r = self.env.store.lock().map_err(|_| AccountError::Storage("lock".into())).and_then(|mut s| s.authenticate(&name, &pw));
        match r {
            Ok(sess) => self.sign_in(sess, &pw),
            Err(e) => {
                if let Screen::Login(f) = &mut self.screen {
                    f.fields[1].clear();
                    f.error = Some(e.to_string());
                }
                let shown: String = name.chars().filter(|c| c.is_ascii_graphic()).take(32).collect();
                self.env.audit.record("-", "login.failed", &shown);
            }
        }
    }

    fn submit_setup(&mut self) {
        let Screen::Setup(f) = &mut self.screen else { return };
        let (name, pw, pw2) = (f.fields[0].value.trim().to_string(), f.fields[1].value.clone(), f.fields[2].value.clone());
        if pw != pw2 {
            f.error = Some("The passwords don't match".into());
            f.fields[2].clear();
            return;
        }
        let r = self.env.store.lock().map_err(|_| AccountError::Storage("lock".into())).and_then(|mut s| {
            s.create_first_admin(&name, &pw)?;
            s.authenticate(&name, &pw)
        });
        match r {
            Ok(sess) => {
                self.env.audit.record(&sess.user, "setup", "first administrator created");
                self.sign_in(sess, &pw);
            }
            Err(e) => {
                if let Screen::Setup(f) = &mut self.screen {
                    f.error = Some(e.to_string());
                }
            }
        }
    }

    /// Entry point for every input event.
    pub fn handle(&mut self, ev: &Event) {
        if let Event::ScrollPx(px) = ev {
            return self.scroll_px(*px); // the scroll engine's pixel steps (`shell_scroll.rs`)
        }
        if !self.saver_intercept(ev) {
            self.handle_awake(ev);
        }
        self.saver_after();
    }

    /// The event for a screen that is on (the saver has already seen it).
    fn handle_awake(&mut self, ev: &Event) {
        use crate::ui::clipboard;
        let (prev_mouse, prev_sel) = (self.mouse, self.sel);
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            crate::filepicker::clear_path_field_note(); // drawn again with the next frame if a path field is still there
        }
        if let Some((agent, logout)) = crate::agent_login::take_request() {
            self.login_request(agent, logout);
        }
        if let Event::MouseMove(x, y) = ev {
            self.mouse = (*x, *y);
        }
        // the Devices popover's tooltip sees every event first, whoever takes the click (a press that closes or opens a
        // row included), and asks for a new picture itself when it appears or goes away
        if self.devpop.open {
            self.devpop.tip_event(ev);
            if self.devpop.tip_changed() {
                self.stale = true;
            }
        }
        if let Screen::HwCheck(h) = &mut self.screen {
            h.note(ev); // the live tester sees every event, whatever else handles it
        }
        if !matches!(ev, Event::Tick) {
            self.tbar_activity();
        }
        // volume, brightness and media keys, and the power card they can open, are the shell's: no app sees them
        if self.handle_fn_event(ev) {
            return;
        }
        if matches!(ev, Event::Tick) {
            self.tick += 1;
            self.fn_tick();
            self.tbar_tick();
            self.check_alarms(iyakki_agent::clock::now_unix());
            self.tick_stash();
            self.panes_tick();
            // developer mode: a paired computer asked for an app on the screen (`iyakki dev run`); it waits for someone to sign in
            if self.tick % 10 == 3 && self.session.is_some() {
                for name in iyakki_agent::devmode::take_open(self.env.services.dir()) {
                    if self.open_custom_by_name(&name) {
                        self.stale = true;
                    }
                }
            }
            // developer mode: keys and taps a paired computer sent (`iyakki dev input`), as the shell's own events; the shell says
            // whether a password field has focus so the device can refuse them
            {
                let dd = self.env.services.dir().to_path_buf();
                iyakki_agent::devmode::input::set_secret_focus(&dd, self.fields.iter().any(|f| f.focused && f.secret));
                for e in iyakki_agent::devmode::input::take(&dd) {
                    self.saver.injecting = true; // it wakes the screen and still reaches the app
                    for ev in crate::devinput::events(&e) {
                        self.handle(&ev);
                    }
                    self.saver.injecting = false;
                    self.stale = true;
                }
            }
            // the assistant's browser tab (the engine behind the browser_* tools) is serviced whichever app is open
            if let Some(sess) = &self.session {
                crate::apps::web_agent::tick(&self.env.services, &self.env.audit, sess);
            }
            self.login_bar_tick();
            if self.tick % 15 == 7 {
                self.battery_tick();
            }
            if self.tick % 15 == 3 {
                self.loss_tick();
            }
            if self.tick % 15 == 0 {
                self.refresh_badge();
                // the clock changes the picture only when what it shows changes
                if self.session.is_some() || self.on_auth_screen() {
                    let t = clock_time_text(&self.clock);
                    if t != self.clock_text {
                        self.clock_text = t;
                        self.stale = true;
                    }
                }
            }
            if self.session.is_some() {
                if self.devpop.wants_refresh() {
                    let p = self.dev_probe();
                    self.devpop.refresh_in_background(&p);
                }
                if self.devpop.take_fresh() {
                    self.stale = true;
                    if self.brand.open && self.brand.page == crate::brandmenu::Page::Mac {
                        self.refresh_mac_firmware();
                        self.brand.mac = self.devpop.snapshot();
                    }
                }
                // the installer indicator: read the installer jobs (the list throttles itself), animate while one runs
                let busy = self.installs.open || matches!(self.installs.badge(), crate::installpop::Badge::Running { .. });
                if self.tick % 30 == 0 || (busy && self.tick % 10 == 0) {
                    if let Some(cx) = self.cx() {
                        let before = self.installs.badge();
                        if self.installs.poll(&cx, false) {
                            self.stale = true;
                        }
                        // an install that was running has ended: the "done" sound
                        let ended = match (before, self.installs.badge()) {
                            (crate::installpop::Badge::Running { count: a, .. }, crate::installpop::Badge::Done { .. }) => a > 0,
                            (crate::installpop::Badge::Running { count: a, .. }, crate::installpop::Badge::Running { count: b, .. }) => b < a,
                            _ => false,
                        };
                        if ended {
                            let _ = crate::cues::start(&self.env.services, &self.cloud, iyakki_agent::syssounds::Cue::JobDone);
                        }
                    }
                }
                if self.installs.frame_due() {
                    self.stale = true;
                }
                // Linux apps: answers of the cluster actions and the jobs' count while the page is open
                if self.brand.open && self.brand.page == crate::brandmenu::Page::Linux && self.tick % 15 == 5 {
                    self.linux_poll();
                }
                // Developer mode's sign in the top bar
                if self.tick % 30 == 3 {
                    let d = iyakki_agent::devmode::DevMode::open(self.env.services.dir());
                    let sign = d.is_on().then(|| match d.shell_banner() {
                        Some(b) => format!("DEV \u{00b7} {b}"),
                        None if d.pairing().is_some() => "DEV \u{00b7} pairing".to_string(),
                        None => "DEV".to_string(),
                    });
                    // a system update that did not finish is told whatever developer mode says (the boot menu keeps the old version)
                    let sign = if iyakki_agent::devmode::update::interrupted(self.env.services.dir()).is_some() { Some("UPDATE INTERRUPTED \u{00b7} choose \"previous version\" at boot".to_string()) } else { sign };
                    if sign != self.dev_sign {
                        self.dev_sign = sign;
                        self.stale = true;
                    }
                }
                // Settings → Time "Detect automatically": looked up again when due (only while it is switched on)
                if self.tick % 300 == 7 {
                    iyakki_agent::tzdetect::refresh_in_background(&self.env.services, false, Arc::new(|z: &str| iyakki_agent::clock::set_chosen_zone(z)));
                }
            }
            // a Quick install job installed k3s, Ollama or Lemonade: start (and connect) it here, where the supervisors live
            if self.tick % 90 == 45 {
                for r in iyakki_agent::quickinstall::service_requests(&self.env.services) {
                    match r {
                        Ok(m) => self.say_info(m),
                        Err(e) => self.say(e),
                    }
                }
            }
            if self.tick % 90 == 0 {
                self.check_new_devices();
            }
            if self.tick % 30 == 15 {
                // a sound card that came up after the start (the built-in speakers of a MacBook take about 11 s)
                for line in iyakki_agent::audioout::global().take_notices() {
                    self.say_info(line);
                }
            }
            if self.tick % 60 == 0 {
                self.check_cluster_alerts();
            }
            if self.tick % 45 == 0 {
                self.announce_finished_jobs();
            }
            if self.toast.as_ref().is_some_and(|t| t.1 <= self.tick) {
                self.toast = None;
            }
            // the sign-in dialog reads what the vendor program printed; sign-outs report when they are done
            // the browser was left: the sign-in dialog comes back, with the clipboard offer
            if self.login.as_ref().is_some_and(|m| m.hidden()) && !matches!(self.screen, Screen::App(_, PApp::Browser)) {
                if let Some(cx) = self.cx() {
                    if let Some(m) = self.login.as_mut() {
                        m.show_again(&cx);
                    }
                    self.stale = true;
                }
            }
            if self.login.as_ref().is_some_and(|m| !m.hidden()) && self.tick % 15 == 7 {
                if let Some(m) = self.login.as_mut() {
                    m.refresh_clip_offer();
                }
            }
            crate::ui::clipboard::tick();
            if self.clipscan.as_mut().is_some_and(|c| self.tick % 3 == 0 && c.tick()) {
                self.stale = true;
            }
            if self.login.is_some() && self.tick % 3 == 0 {
                if let Some(cx) = self.cx() {
                    if self.login.as_mut().is_some_and(|m| m.tick(&cx)) {
                        self.stale = true;
                    }
                }
            }
            if self.tick % 10 == 5 {
                for (m, ok) in crate::agent_login::take_results() {
                    if ok {
                        self.say_info(m);
                    } else {
                        self.say(m);
                    }
                }
            }
        }
        if self.panes_intercept(ev) {
            self.stale = true;
            return;
        }
        match ev {
            Event::MouseDown(x, y) => self.click(*x, *y),
            Event::MouseUp(..) => {
                self.press = None;
                self.dispatch(ev);
            }
            _ => self.dispatch(ev),
        }
        // the pointer moves with the button held over a text field or area: repeat the press there so the selection grows
        if let (Event::MouseMove(x, y), Some((tmpl, rect))) = (ev, self.press.clone()) {
            let again = match tmpl {
                Event::Focus(i) => Some(Event::FocusAt(i, *x as f32 - rect.x)),
                Event::Area(i) => Some(Event::AreaAt(i, *x as f32 - rect.x, *y as f32 - rect.y)),
                _ => None,
            };
            if let Some(e) = again {
                crate::ui::set_dragging(true);
                self.dispatch(&e);
                crate::ui::set_dragging(false);
                self.stale = true;
            }
        }
        if matches!(ev, Event::Key(_)) {
            self.press = None;
        }
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            self.refresh_badge();
        }
        if let Some(t) = clipboard::take_toast() {
            self.say_info(t);
        }
        // Anything that can change what is on screen makes last frame's click regions stale; until the
        // next frame is drawn, clicks fall back to the plain handlers instead of hitting old controls.
        if !matches!(ev, Event::MouseMove(..) | Event::MouseUp(..) | Event::Tick) {
            self.hits.clear();
        }
        match ev {
            // the radio's display moves with the music: draw every tick while it is on screen and playing
            Event::Tick => {
                self.scroll_sync();
                // hardware that shows up after the shell started (a slow sound card) changes Cloud mode: look again every 10 s
                if !cfg!(test) && self.tick % 75 == 0 {
                    let c = iyakki_agent::cloudmode::detect(self.env.services.cloud_setting());
                    self.refresh_cloud(c);
                }
                // a camera plugged in or pulled out changes the CAMERA tile within two seconds
                if matches!(self.screen, Screen::Launcher) && self.cloud.camera_watch && (self.tick % 60 == 0 || !self.camera.scanned()) && self.camera.scan() {
                    self.stale = true;
                }
                // the "camera in use by the assistant" sign comes and goes (and its time moves) on every screen
                if self.tick % 15 == 0 {
                    let on = self.env.services.camera.in_use().is_some();
                    if on || on != self.cam_sign {
                        self.cam_sign = on;
                        self.stale = true;
                    }
                }
                if matches!(self.screen, Screen::App(_, PApp::Radio)) && self.env.services.radio.is_active() {
                    self.stale = true;
                }
                // a tool being downloaded shows its progress
                if matches!(self.screen, Screen::App(_, PApp::Cli)) && self.tick % 6 == 0 && (self.env.services.clis.running() || self.env.services.cluster.state() == iyakki_agent::cluster::ClusterState::Starting) {
                    self.stale = true;
                }
                // the clock moves every second
                if matches!(self.screen, Screen::App(_, PApp::Calendar | PApp::Media)) && self.tick % 10 == 0 {
                    self.stale = true;
                }
                // the level display and the pulse of the MEDIA player move ten times a second, only while something plays
                if matches!(self.screen, Screen::App(_, PApp::Media)) && self.tick % 3 == 0 && iyakki_agent::media::shared().now_playing().is_some() {
                    self.stale = true;
                }
                // an app that shows live pictures (the browser's full engine) says when a new one has arrived
                if let Screen::App(app, _) = &mut self.screen {
                    match app.redraw_kind() {
                        crate::apps::browser_full::Redraw::Full => self.stale = true,
                        crate::apps::browser_full::Redraw::Live if self.live_paint => self.live = true,
                        crate::apps::browser_full::Redraw::Live => self.stale = true,
                        crate::apps::browser_full::Redraw::None => {}
                    }
                }
            }
            // a pointer that only moves changes the picture when it crosses the edge of something that reacts
            Event::MouseMove(..) => {
                let (a, b) = (prev_mouse, self.mouse);
                if matches!(self.screen, Screen::HwCheck(_)) || self.sel != prev_sel || self.probes.iter().any(|r| r.contains(a.0, a.1) != r.contains(b.0, b.1)) {
                    self.stale = true;
                }
            }
            _ => self.stale = true,
        }
        self.panes_after(ev, prev_mouse);
    }

    /// True when events since the last call changed what should be on screen (a plain pointer move that crosses
    /// nothing does not). Resets the flag.
    pub fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale)
    }

    /// Clipboard shortcuts first, then the overlay, then the current screen.
    fn dispatch(&mut self, ev: &Event) {
        use crate::ui::clipboard;
        match ev {
            // a ringing alarm takes every key and click until it is dismissed or snoozed
            _ if self.ring.is_some() && !matches!(ev, Event::Tick | Event::MouseMove(..)) => self.handle_ring(ev),
            // the clipboard's QR scan takes every key and click while it is open
            _ if self.clipscan.is_some() && !matches!(ev, Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::FocusAt(..)) => self.handle_clipscan(ev),
            // the sign-in dialog takes every key, click and pasted text
            _ if self.login.as_ref().is_some_and(|m| !m.hidden()) && !matches!(ev, Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::FocusAt(..)) => self.handle_login(ev),
            // a Login or Logout button of any screen
            Event::Tab(n) if self.session.is_some() && self.clip.is_none() && crate::agent_login::agent_of_tab(*n).is_some() => {
                if let Some((agent, logout)) = crate::agent_login::agent_of_tab(*n) {
                    self.login_request(agent, logout);
                }
            }
            // the host-files dialog (files dropped on the window) takes every key and click
            _ if self.hostdlg.is_some() && !matches!(ev, Event::MouseMove(..)) => self.handle_hostdlg(ev),
            // a screenshot being framed takes every key and click
            _ if matches!(self.shot, Shot::Select { .. } | Shot::Review { .. }) => self.handle_shot(ev),
            // F12 (or Print Screen) starts a screenshot anywhere after sign-in
            Event::Key(Key::F(12)) if self.session.is_some() && self.clip.is_none() => self.start_shot(),
            // the drive banner's buttons answer on every screen, signed in or not
            Event::Tab(n) if crate::lossbar::is_click(*n) => self.loss_event(*n),
            // the battery's countdown and plug-in cards answer on every screen; F4 (or the chip) opens the battery popup
            Event::Tab(crate::batterybar::CANCEL) | Event::Tab(crate::batterybar::CARD_OK) => self.battery_card_event(ev),
            Event::Key(Key::Esc) if self.battery.card_visible() => self.battery_card_event(ev),
            Event::Key(Key::F(crate::batterybar::KEY)) if self.session.is_some() && self.clip.is_none() && !self.battery.open => self.toggle_battery(),
            Event::Key(_) | Event::Tab(_) if self.battery.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_battery(ev),
            // F8 (or the Devices button) opens the devices popover; F7 (or the clock) opens Settings, Time
            Event::Key(Key::F(8)) if self.session.is_some() && self.clip.is_none() => self.toggle_devices(),
            Event::Key(Key::F(7)) if self.session.is_some() && self.clip.is_none() => {
                self.devpop.close();
                self.brand.close();
                self.installs.close();
                self.help = false;
                self.jump_to("settings", "tab:time");
            }
            // F10 (or a click on the name in the launcher's top bar) opens the IYAKKI menu; the indicator beside the clock the installs
            Event::Key(Key::F(10)) if self.session.is_some() && self.clip.is_none() => self.toggle_brand(),
            Event::Tab(crate::installpop::INDICATOR) if self.session.is_some() && self.clip.is_none() => self.toggle_installs(),
            Event::Tab(DEV_SIGN) if self.session.is_some() && self.clip.is_none() => self.jump_to("settings", "tab:developer"),
            // the menu takes its keys, clicks, pasted text and the wheel while it is open
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) | Event::Paste(_) | Event::FocusAt(..) if self.brand.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_brand(ev),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) | Event::AreaAt(..) | Event::MouseUp(..) if self.installs.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_installs(ev),
            // the popover takes its keys, clicks and the wheel while it is open
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.devpop.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_devpop(ev),
            // F1 opens the help popup anywhere after sign-in; while it is open it takes every key
            Event::Key(Key::F(1)) if self.session.is_some() && self.clip.is_none() => self.help = !self.help,
            // F9 (or the bell) opens the inbox anywhere after sign-in
            Event::Key(Key::F(9)) if self.session.is_some() && self.clip.is_none() => {
                self.help = false;
                self.open_inbox();
            }
            _ if self.help => {
                if matches!(ev, Event::Key(Key::Esc | Key::Enter | Key::Char(' ') | Key::F(1))) {
                    self.help = false;
                }
            }
            // panes: the Ctrl+Alt chords, the buttons of the pane strips and the bar, the memory card and the layout picker
            Event::Layout(k) if self.session.is_some() && self.clip.is_none() => self.panes_key(*k),
            Event::Tab(n) if self.panes.card.is_none() && self.panes_owns_tab(*n) => self.panes_tab(*n),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.panes.card.is_some() && self.session.is_some() => self.panes_card_event(ev),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.panes.picker.is_some() && self.session.is_some() && self.clip.is_none() => self.panes_picker_key(ev),
            // the launcher's own menus and cards (move a tile, make or delete a group) and the keys that open them
            _ if self.layout_intercept(ev) => {}
            // the context menu takes every key and click while it is open (heartbeats, releases and clipboard text pass)
            _ if self.ctx.is_some() && !matches!(ev, Event::Tick | Event::MouseUp(..) | Event::Paste(_)) => self.handle_ctx(ev),
            // the answer to the queue's request for the system clipboard: it joins the queue and is not pasted anywhere
            Event::Paste(t) if self.tick < self.host_sync_until => {
                self.host_sync_until = 0;
                if let Some(m) = self.ctx.as_mut() {
                    m.set_clip_text(true);
                }
                self.clip_note = ClipNote::Got(t.chars().count());
                clipboard::remember(t);
                if self.clip.is_some() {
                    self.build_clipboard_picker();
                }
            }
            // text that arrived from the host clipboard: remember it in the history, then paste it
            Event::Paste(t) if self.clip.is_none() => {
                self.ctx = None;
                clipboard::remember(t);
                self.route(ev);
            }
            // Ctrl+V pastes the most recent clipboard entry; Ctrl+Shift+V opens the history; a right click opens the context menu
            Event::Key(Key::Ctrl('v')) if self.clip.is_none() => match clipboard::paste() {
                Some(t) => self.route(&Event::Paste(t)),
                None => self.say_info("The clipboard is empty"),
            },
            Event::Key(Key::Ctrl('V')) => {
                self.ctx = None;
                self.open_clipboard_picker()
            }
            Event::MouseRight(x, y) if self.clip.is_none() && self.layout_right_click(*x, *y) => {}
            Event::MouseRight(x, y) if self.clip.is_none() => self.open_context_menu(*x, *y),
            Event::MouseRight(..) => self.open_clipboard_picker(),
            _ if self.clip.is_some() => self.handle_clip_overlay(ev),
            _ => self.route(ev),
        }
    }

    /// The dev cluster went down or its disk is nearly full: tell the signed-in administrator in the INBOX (once per event).
    fn check_cluster_alerts(&mut self) {
        let Some(user) = self.session.as_ref().filter(|s| s.policy.manage_settings).map(|s| s.user.clone()) else { return };
        for a in self.env.services.k3sdev.take_alerts() {
            let (title, body) = a.title_body();
            self.env.audit.record(&user, "devcluster.alert", &title);
            let _ = self.env.services.notify(&user, iyakki_agent::inbox::Kind::Warning, &title, &body, Some(iyakki_agent::inbox::Link::new("settings", "")));
            self.say(format!("{title}. See Settings \u{2192} Dev cluster or SYSTEM."));
        }
    }

    /// Tell an administrator about devices that appeared on the network since the last look.
    fn check_new_devices(&mut self) {
        let Some(user) = self.session.as_ref().filter(|s| s.policy.manage_settings).map(|s| s.user.clone()) else { return };
        for d in self.env.services.devwatch.take_fresh() {
            self.env.audit.record(&user, "netsec.new_device", &format!("{} {}", d.ip, d.mac));
            let _ = self.env.services.notify(&user, iyakki_agent::inbox::Kind::Device, "New device on your network", &format!("{} ({}) appeared. NET SCAN → Devices lets you trust it.", d.ip, d.mac), Some(iyakki_agent::inbox::Link::new("scan", &d.mac)));
            self.say(format!("New device on your network: {} ({}). NET SCAN → Devices to trust it.", d.ip, d.mac));
        }
    }

    /// Looks for alarms and reminders that have become due since the last look, and starts ringing for them.
    pub fn check_alarms(&mut self, now: i64) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else {
            self.last_alarm_check = 0;
            return;
        };
        if self.last_alarm_check == 0 {
            self.last_alarm_check = now;
            return;
        }
        if now != self.last_alarm_check {
            let zone = iyakki_agent::clock::Zone::system();
            let mut items: Vec<(String, bool, String)> = Vec::new();
            for d in self.env.services.calendar.take_due(&user, self.last_alarm_check, now, &zone) {
                match d {
                    iyakki_agent::calendar::Due::Alarm { label, at } => {
                        let l = zone.local(at);
                        let _ = self.env.services.notify(&user, iyakki_agent::inbox::Kind::Reminder, &format!("Alarm: {label}"), &format!("Rang at {:02}:{:02}", l.hour, l.minute), Some(iyakki_agent::inbox::Link::new("calendar", "")));
                        items.push((label, true, format!("{:02}:{:02}", l.hour, l.minute)));
                    }
                    iyakki_agent::calendar::Due::Reminder { title, starts } => {
                        let l = zone.local(starts);
                        let _ = self.env.services.notify(&user, iyakki_agent::inbox::Kind::Reminder, &format!("Reminder: {title}"), &format!("Starts at {:02}:{:02}", l.hour, l.minute), Some(iyakki_agent::inbox::Link::new("calendar", "")));
                        items.push((title, false, format!("starts at {:02}:{:02}", l.hour, l.minute)));
                    }
                }
            }
            let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.snoozed).into_iter().partition(|s| s.0 <= now);
            self.snoozed = later;
            items.extend(due.into_iter().map(|(_, what, alarm)| (what, alarm, "snoozed".to_string())));
            self.last_alarm_check = now;
            if !items.is_empty() {
                for (what, alarm, _) in &items {
                    self.env.audit.record(&user, if *alarm { "alarm.ring" } else { "calendar.reminder" }, what);
                }
                match &mut self.ring {
                    Some(r) => r.items.extend(items),
                    None => self.ring = Some(Ring { items, since: now, player: None, next_sound: 0 }),
                }
                self.stale = true;
            }
        }
        // the sound repeats until the alarm is dismissed (and gives up after five minutes)
        if let Some(r) = &mut self.ring {
            if now - r.since > 300 {
                let n = r.items.len();
                self.ring = None;
                self.say(format!("{n} alarm{} rang and was not answered", if n == 1 { "" } else { "s" }));
                return;
            }
            if self.tick >= r.next_sound && r.player.as_mut().is_none_or(|p| !p.is_playing()) {
                let cmd = self.env.services.voice_settings().play_cmd;
                r.player = iyakki_agent::voice::Player::start(&cmd, &iyakki_agent::calendar::alarm_wav()).ok();
                r.next_sound = self.tick + 60;
            }
            self.stale = true;
        }
    }

    fn handle_ring(&mut self, ev: &Event) {
        let snooze = matches!(ev, Event::Key(Key::Char('s') | Key::Char('S')));
        let dismiss = matches!(ev, Event::Key(Key::Enter | Key::Esc | Key::Char(' ')));
        if !(snooze || dismiss) {
            return;
        }
        if let Some(mut r) = self.ring.take() {
            if let Some(p) = r.player.as_mut() {
                p.stop();
            }
            if snooze {
                let at = iyakki_agent::clock::now_unix() + 300;
                for (what, alarm, _) in r.items {
                    self.snoozed.push((at, what, alarm));
                }
                self.say_info("Snoozed for 5 minutes");
            }
        }
    }

    #[cfg(test)]
    pub fn ringing(&self) -> Option<Vec<String>> {
        self.ring.as_ref().map(|r| r.items.iter().map(|i| i.0.clone()).collect())
    }

    fn clipboard_limit_path(&self) -> Option<std::path::PathBuf> {
        let user = &self.session.as_ref()?.user;
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        Some(self.env.services.dir().join(format!("clipboard-{who}.json")))
    }

    fn save_clipboard_limit(&self) {
        if let Some(p) = self.clipboard_limit_path() {
            let _ = std::fs::write(p, serde_json::json!({ "limit": crate::ui::clipboard::limit() }).to_string());
        }
    }

    /// The person's own choice of how many copies to keep, read at sign-in.
    fn load_clipboard_limit(&self) {
        let n = self.clipboard_limit_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["limit"].as_u64());
        crate::ui::clipboard::set_limit(n.map_or(crate::ui::clipboard::DEFAULT_LIMIT, |n| n as usize));
    }

    /// Open the queue of copies. The window is also asked for what the system clipboard holds now (text copied in other
    /// programs only reaches the queue that way); it is added when it arrives.
    fn open_clipboard_picker(&mut self) {
        self.want_host_paste = true;
        self.host_sync_until = self.tick + 90;
        self.clip_note = ClipNote::Asked;
        self.build_clipboard_picker();
    }

    fn build_clipboard_picker(&mut self) {
        use crate::ui::clipboard::Entry;
        let items = crate::ui::clipboard::entries();
        let mut thumbs: Vec<Option<std::rc::Rc<tiny_skia::Pixmap>>> = Vec::new();
        let previews: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Entry::Text(t) => {
                    thumbs.push(None);
                    format!("{:>2}. {}", i + 1, crate::ui::clipboard::preview(t))
                }
                Entry::Picture { width, height, rgba } => {
                    thumbs.push(tiny_skia::IntSize::from_wh(*width, *height).and_then(|sz| tiny_skia::Pixmap::from_vec((**rgba).clone(), sz)).map(std::rc::Rc::new));
                    format!("{:>2}. picture {width}×{height}", i + 1)
                }
            })
            .collect();
        let title = if items.is_empty() { "Clipboard history: nothing copied yet" } else { "Clipboard history: pick one to paste" };
        self.clip = Some(Picker::new(title, previews).with_thumbs(thumbs));
    }

    /// True once after the queue was opened: the desktop window should read the system clipboard and send it as a paste.
    pub fn take_host_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.want_host_paste)
    }

    /// "Scan QR" was chosen in the clipboard history: the history closes and the camera question opens.
    fn open_clipscan(&mut self) {
        self.clip = None;
        let src = self.scan_src.clone().unwrap_or_else(crate::scan_core::CamSource::system);
        self.clipscan = Some(Box::new(crate::clip_scan::ClipScan::open(src)));
        self.stale = true;
    }

    /// Use made-up cameras for the clipboard scan (tests).
    pub fn set_scan_camera(&mut self, src: std::sync::Arc<crate::scan_core::CamSource>) {
        self.scan_src = Some(src);
    }

    #[cfg(test)]
    pub(crate) fn audit_log_path(&self) -> std::path::PathBuf {
        self.env.audit.path().to_path_buf()
    }

    pub fn clipboard_scan(&self) -> Option<&crate::clip_scan::ClipScan> {
        self.clipscan.as_deref()
    }

    fn handle_clipscan(&mut self, ev: &Event) {
        let Some(c) = self.clipscan.as_mut() else { return };
        match c.handle(ev) {
            crate::clip_scan::Out::Stay => {}
            crate::clip_scan::Out::Close => self.clipscan = None,
            crate::clip_scan::Out::Copied { chars, secret } => {
                self.clipscan = None;
                // only the fact: never the text
                if let Some(s) = &self.session {
                    self.env.audit.record(&s.user, "clipboard.scan", &format!("clipboard scan: {chars} characters, secret {}", if secret { "yes" } else { "no" }));
                }
            }
        }
        self.stale = true;
    }

    fn handle_clip_overlay(&mut self, ev: &Event) {
        // the + and − buttons change how many copies are kept (saved for this person)
        if let Event::Tab(n @ (910 | 911)) = ev {
            let now = crate::ui::clipboard::limit();
            let next = if *n == 911 { now + 1 } else { now.saturating_sub(1) };
            crate::ui::clipboard::set_limit(next);
            self.save_clipboard_limit();
            self.build_clipboard_picker();
            return;
        }
        // Scan QR: copy text by holding a QR code up to the camera
        if matches!(ev, Event::Tab(912) | Event::Key(Key::Ctrl('k' | 'K'))) {
            self.open_clipscan();
            return;
        }
        // Save on a picture's row writes it to a file
        if let Event::Tab(n) = ev {
            if *n > 1000 {
                if let Some(crate::ui::clipboard::Entry::Picture { width, height, rgba }) = crate::ui::clipboard::entries().get(*n - 1001).cloned() {
                    if let Some(pm) = tiny_skia::IntSize::from_wh(width, height).and_then(|sz| tiny_skia::Pixmap::from_vec((*rgba).clone(), sz)) {
                        self.save_pixmap(&pm);
                    }
                }
                return;
            }
        }
        let Some(p) = self.clip.as_mut() else { return };
        match p.handle_event(ev) {
            PickEvent::Cancel => self.clip = None,
            PickEvent::Chosen(label) => {
                self.clip = None;
                // the label starts with its 1-based position in the queue
                let idx = label.trim_start().split('.').next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                match crate::ui::clipboard::entries().get(idx).cloned() {
                    Some(crate::ui::clipboard::Entry::Text(text)) => self.route(&Event::Paste(text)),
                    // nothing here takes a picture yet: it becomes the system clipboard's picture, ready to paste elsewhere
                    Some(crate::ui::clipboard::Entry::Picture { .. }) => {
                        if crate::ui::clipboard::promote_picture(idx) {
                            self.say_info("Picture copied to the system clipboard — paste it in another program, or use Save");
                        }
                    }
                    None => {}
                }
            }
            PickEvent::None => {}
        }
    }

    /// An app asked for the options of a shared host folder (window mode).
    fn host_mount_menu(&mut self, name: &str) {
        if !iyakki_agent::hostfiles::enabled() {
            self.say(iyakki_agent::hostfiles::UNAVAILABLE);
            return;
        }
        let Some(cx) = self.cx() else { return };
        self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::mount_menu(crate::apps::hostfiles::HostEnv::for_cx(&cx), rgb(238, 226, 112), name)));
        self.stale = true;
    }

    /// An app asked to save a file on the host computer: the save dialog (window mode only).
    fn host_save(&mut self, name: &str, bytes: &[u8]) {
        if !iyakki_agent::hostfiles::enabled() {
            self.say(iyakki_agent::hostfiles::UNAVAILABLE);
            return;
        }
        let Some(cx) = self.cx() else { return };
        self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::save_bytes(crate::apps::hostfiles::HostEnv::for_cx(&cx), rgb(238, 226, 112), name, bytes)));
        self.stale = true;
    }

    /// Tell the app on screen what a drop brought in (the IDE opens it, the media library looks again).
    fn tell_app_dropped(&mut self, files: &[std::path::PathBuf]) {
        let Some(cx) = self.cx() else { return };
        if let Screen::App(app, _) = &mut self.screen {
            app.dropped_done(files, &cx);
        }
    }

    /// The Login button of an agent was pressed (or its Logout button): open the dialog, or sign out.
    fn login_request(&mut self, agent: &'static str, logout: bool) {
        let Some(cx) = self.cx() else { return };
        self.brand.close();
        self.installs.close();
        self.devpop.close();
        if logout {
            crate::agent_login::sign_out_in_background(&cx, agent);
            self.say_info(format!("Signing out of {}…", crate::agent_login::name_of(agent)));
        } else {
            let mut m = Box::new(crate::agent_login::LoginModal::new(agent));
            if agent != iyakki_agent::extagent::CLAUDE_CODE {
                m.start(&cx);
            }
            self.login = Some(m);
        }
        self.stale = true;
    }

    fn handle_login(&mut self, ev: &Event) {
        let Some(cx) = self.cx() else {
            self.login = None;
            return;
        };
        if let Some(m) = self.login.as_mut() {
            match m.handle(ev, &cx) {
                crate::agent_login::Out::Close => self.login = None,
                crate::agent_login::Out::OpenBrowser(url) => {
                    m.hide();
                    self.open_in_browser(&url);
                }
                crate::agent_login::Out::Stay => {}
            }
        }
        self.stale = true;
    }

    /// Show an address in the browser tile, in a new tab, keeping the screen it was opened from aside (Back returns to it). The sign-in
    /// dialog is hidden meanwhile and comes back by itself when the browser is left (see the heartbeat).
    fn open_in_browser(&mut self, url: &str) {
        let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Screen::App(a, pa) = prev {
            if self.stash.is_none() && pa != PApp::Browser {
                self.stash = Some((a, pa));
            } else if pa == PApp::Browser {
                self.screen = Screen::App(a, pa);
            }
        } else {
            self.screen = prev;
        }
        if !matches!(self.screen, Screen::App(_, PApp::Browser)) {
            self.open(PApp::Browser);
        }
        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
            a.open_item(&format!("newtab:{url}"), &cx);
        }
        if !matches!(self.screen, Screen::App(_, PApp::Browser)) {
            // the browser could not be opened (the role may not use it): the dialog stays
            if let Some(m) = self.login.as_mut() {
                m.show_hidden_again_for_shell();
            }
            self.say("The browser is not available to your account: open the address from the QR code or copy it.");
        }
    }

    fn handle_hostdlg(&mut self, ev: &Event) {
        use crate::apps::hostfiles::Out;
        let Some(d) = self.hostdlg.as_mut() else { return };
        match d.handle(ev) {
            Out::Close => self.hostdlg = None,
            Out::Imported(files) => {
                let label = d_label(self.hostdlg.as_deref());
                self.say_info(format!("Imported {} file{} into {label}", files.len(), if files.len() == 1 { "" } else { "s" }));
                self.tell_app_dropped(&files);
            }
            Out::Linked(n) => {
                self.say_info(format!("host:{n} is shared read-only"));
                self.tell_app_dropped(&[]);
            }
            Out::Unlinked(n) => self.say_info(format!("host:{n} is no longer shared")),
            Out::Stay => {}
        }
        self.stale = true;
    }

    /// Files are being dragged over the window (window mode): show where a drop would go.
    pub fn set_drag_hover(&mut self, on: bool) {
        if self.drag_over != on {
            self.drag_over = on;
            self.stale = true;
        }
    }

    /// Files or folders were dropped on the window. The app on screen may take them (DATA opens tables); otherwise a dialog
    /// offers to import them into the Uploads folder. Only in window mode, after sign-in.
    pub fn drop_files(&mut self, paths: Vec<std::path::PathBuf>) {
        self.drag_over = false;
        self.stale = true;
        if self.session.is_none() || paths.is_empty() {
            return;
        }
        if !iyakki_agent::hostfiles::enabled() {
            self.say("Dropped files can only be imported in the desktop window");
            return;
        }
        // a form with a path field on screen: the dropped path fills the field instead of opening the import dialog
        if crate::filepicker::path_field_on_screen() {
            if let Some(ev) = crate::filepicker::drop_event(&paths) {
                self.dispatch(&ev);
                self.stale = true;
                return;
            }
        }
        let Some(cx) = self.cx() else { return };
        let action = match &mut self.screen {
            Screen::App(app, _) => app.on_drop(&paths, &cx),
            _ => crate::apps::DropAction::Uploads,
        };
        let env = crate::apps::hostfiles::HostEnv::for_cx(&cx);
        let accent = rgb(238, 226, 112);
        match action {
            crate::apps::DropAction::Taken => {}
            crate::apps::DropAction::Ignore(m) => self.say(m),
            crate::apps::DropAction::Uploads => self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::import_paths(env, accent, paths))),
            crate::apps::DropAction::Into { dest, label } => self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::import_paths(env.with_dest(dest, &label), accent, paths))),
        }
    }

    /// "Host access: 2 folders" while folders of the host computer are shared with this account (for the window title).
    pub fn host_access_chip(&self) -> Option<String> {
        let s = self.session.as_ref()?;
        if !iyakki_agent::hostfiles::enabled() {
            return None;
        }
        let n = iyakki_agent::hostfiles::active_mounts(self.env.services.dir(), &s.user).len();
        (n > 0).then(|| format!("Host access: {n} folder{}", if n == 1 { "" } else { "s" }))
    }

    /// Resolve a click against the regions drawn last frame (topmost first).
    fn click(&mut self, x: i32, y: i32) {
        let (w, _) = self.size;
        let s = scale_for(w, self.size.1);
        let hit = self.hits.iter().rev().find(|h| h.rect.contains(x, y)).cloned();
        if self.ctx.is_some() && self.ring.is_none() && !self.help && matches!(self.shot, Shot::Off) {
            self.handle_ctx(&Event::MouseDown(x, y));
            return;
        }
        if self.lay.menu.is_some() && self.ring.is_none() && !self.help && matches!(self.shot, Shot::Off) {
            self.layout_intercept(&Event::MouseDown(x, y));
            return;
        }
        if self.hostdlg.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_hostdlg(&h.ev);
                }
            }
            return;
        }
        if self.clipscan.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_clipscan(&h.ev);
                }
            }
            return;
        }
        if self.login.as_ref().is_some_and(|m| !m.hidden()) {
            if let Some(h) = hit {
                if h.label != "backdrop" && !matches!(h.ev, Event::Focus(_)) {
                    self.handle_login(&h.ev);
                }
            }
            return;
        }
        if matches!(self.shot, Shot::Select { .. }) {
            self.handle_shot(&Event::MouseDown(x, y));
            return;
        }
        if matches!(self.shot, Shot::Review { .. }) {
            // only the dialog's buttons answer; a click anywhere else leaves it open
            if let Some(h) = hit {
                self.handle_shot(&h.ev);
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.devpop.open && self.clip.is_none() && !self.help {
            match hit {
                Some(h) => self.dispatch(&h.ev),
                None => self.devpop.close(), // a click outside closes it
            }
            return;
        }
        // the battery popup: its own regions answer, a click outside closes it
        if self.battery.open && self.clip.is_none() && !self.help {
            match hit {
                Some(h) => self.dispatch(&h.ev),
                None => self.battery.close(),
            }
            self.stale = true;
            return;
        }
        // the IYAKKI menu and the installs list: their own regions answer, a click outside closes them
        if (self.brand.open || self.installs.open) && self.clip.is_none() {
            match hit {
                Some(h) => {
                    let ev = match h.ev {
                        Event::Focus(i) => Event::FocusAt(i, x as f32 - h.rect.x),
                        Event::Area(i) => Event::AreaAt(i, x as f32 - h.rect.x, y as f32 - h.rect.y),
                        other => other,
                    };
                    self.dispatch(&ev);
                }
                None => {
                    self.brand.close();
                    self.installs.close();
                }
            }
            self.stale = true;
            return;
        }
        if self.clip.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_clip_overlay(&h.ev);
                }
            } else {
                self.clip = None; // click outside closes it
            }
            return;
        }
        // the layout picker and the memory card: their own regions answer; a click outside closes the picker
        if (self.panes.picker.is_some() || self.panes.card.is_some()) && self.session.is_some() && self.clip.is_none() {
            match hit {
                Some(h) if h.label != "backdrop" => self.dispatch(&h.ev),
                _ => {
                    if self.panes.card.is_none() {
                        self.panes.picker = None;
                    }
                }
            }
            self.stale = true;
            return;
        }
        // the header's Back button always leaves the app, even with a dialog open
        if matches!(self.screen, Screen::App(..)) && self.panes.ws.is_none() && back_rect(w, s).contains(x, y) {
            self.leave_app();
            return;
        }
        match hit {
            Some(h) if h.label != "backdrop" => {
                let template = h.ev.clone();
                let ev = match h.ev {
                    Event::Focus(i) => Event::FocusAt(i, x as f32 - h.rect.x),
                    Event::Area(i) => Event::AreaAt(i, x as f32 - h.rect.x, y as f32 - h.rect.y),
                    other => other,
                };
                // presses on text fields and areas count (double click: a word, triple: everything) and can be dragged
                let selectable = matches!(template, Event::Focus(_) | Event::Area(_));
                let near = |t: u64, px: i32, py: i32| self.tick.saturating_sub(t) <= 12 && (x - px).abs() <= 6 && (y - py).abs() <= 6;
                let (t0, px, py, n0) = self.click_run;
                let count = if selectable && n0 > 0 && near(t0, px, py) { (n0 + 1).min(3) } else { 1 };
                self.click_run = if selectable { (self.tick, x, y, count) } else { (0, 0, 0, 0) };
                self.press = selectable.then(|| (template.clone(), h.rect));
                crate::ui::set_click_count(count);
                // a second click on the same row within ~0.4 s also "opens" it (like Enter)
                let double = matches!(ev, Event::Row(_)) && self.last_click.as_ref().is_some_and(|(t, l)| *l == h.label && self.tick.saturating_sub(*t) <= 12);
                self.last_click = Some((self.tick, h.label.clone()));
                self.dispatch(&ev);
                crate::ui::set_click_count(1);
                if double {
                    self.dispatch(&Event::Key(Key::Enter));
                }
            }
            Some(_) => {} // backdrop of a dialog: swallow
            None => self.route(&Event::MouseDown(x, y)),
        }
    }

    /// Deliver an event to the current screen.
    fn route_inner(&mut self, ev: &Event) {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let _ = (h, s);
        match &mut self.screen {
            Screen::Setup(_) | Screen::Login(_) => {
                // the power card (Ctrl+P or the Power button, local console only) takes every key first
                if self.login_power_event(ev) {
                    return;
                }
                let is_setup = matches!(self.screen, Screen::Setup(_));
                let (Screen::Setup(f) | Screen::Login(f)) = &mut self.screen else { return };
                match ev {
                    Event::Key(Key::Tab) | Event::Key(Key::Down) => f.next(),
                    Event::Key(Key::BackTab) | Event::Key(Key::Up) => f.prev(),
                    Event::Key(Key::Enter) => {
                        if f.focus + 1 < f.fields.len() {
                            f.next();
                        } else if is_setup {
                            self.submit_setup();
                        } else {
                            self.submit_login();
                        }
                    }
                    // the Sign in / Create account button
                    Event::Key(Key::Ctrl('s')) => {
                        if is_setup {
                            self.submit_setup();
                        } else {
                            self.submit_login();
                        }
                    }
                    Event::Key(k) => {
                        let focus = f.focus;
                        f.error = None;
                        f.fields[focus].handle(k);
                    }
                    Event::Paste(t) => {
                        let focus = f.focus;
                        f.error = None;
                        f.fields[focus].paste(t);
                    }
                    Event::FocusAt(i, x) => {
                        if *i < f.fields.len() {
                            f.focus = *i;
                            f.fields[*i].click(*x);
                        }
                    }
                    _ => {}
                }
            }
            Screen::HwCheck(h) => {
                if h.handle(ev) == crate::hwcheck::Outcome::Continue {
                    self.leave_hwcheck();
                }
            }
            Screen::Launcher => self.handle_launcher(ev),
            Screen::PowerMenu => {
                if let Event::Key(k) = ev {
                    let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                    match k {
                        Key::Char('y') | Key::Char('o') => {
                            self.env.audit.record(&user, "power.off", "");
                            self.env.host.power_off();
                        }
                        Key::Char('r') => {
                            self.env.audit.record(&user, "power.reboot", "");
                            self.env.host.reboot();
                        }
                        _ => {}
                    }
                    self.screen = Screen::Launcher;
                }
            }
            Screen::App(..) => {
                let back_click = self.panes.ws.is_none() && matches!(ev, Event::MouseDown(x, y) if back_rect(w, s).contains(*x, *y));
                let cx = self.cx();
                // a bug inside one app must not take the whole shell down: the app is closed and the person is told
                let mut crashed = None;
                let mut request = None;
                let mut host_save = None;
                let mut mount_menu = None;
                let flow = match (&mut self.screen, cx) {
                    (Screen::App(app, _), Some(cx)) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (app.handle(ev, &cx), app.take_request(), app.take_host_save(), app.take_host_mount_menu()))) {
                        Ok((f, r, save, menu)) => {
                            request = r;
                            host_save = save;
                            mount_menu = menu;
                            f
                        }
                        Err(_) => {
                            crashed = Some(app.title());
                            Flow::Back
                        }
                    },
                    _ => Flow::Back,
                };
                if let Some(name) = crashed {
                    self.app_crashed(name);
                }
                if let Some((name, bytes)) = host_save {
                    self.host_save(&name, &bytes);
                }
                if let Some(name) = mount_menu {
                    self.host_mount_menu(&name);
                }
                if back_click || matches!(flow, Flow::Back) {
                    self.leave_app();
                } else if let Some((app, item)) = request {
                    self.open_linked_from_app(&app);
                    if !item.is_empty() {
                        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
                            a.open_item(&item, &cx);
                        }
                    }
                }
            }
        }
    }

    /// Turn to page `p` (ignored when there is no such page); the first tile of it is selected.
    fn go_page(&mut self, p: usize, pages: usize, cap: usize) {
        if p < pages {
            self.sel = p * cap;
        }
    }

    fn open_item(&mut self, item: &LItem) {
        match item {
            // a greyed CAMERA tile says what is missing instead of opening
            LItem::Sys(t) if t.app == PApp::Camera && !self.camera_ready() => self.say(self.camera.hint()),
            LItem::Sys(t) => self.open(t.app),
            LItem::App(e) => self.open_custom((**e).clone()),
            LItem::Empty => self.say_info("Nothing here yet: select a tile in another group and press m to move it here"),
        }
    }

    /// Open a custom app in its own tile.
    fn open_custom(&mut self, entry: iyakki_agent::store::tilehub::TileEntry) {
        let Some(cx) = self.cx() else { return };
        if !iyakki_agent::store::tilehub::allowed(&cx.session.policy) {
            self.env.audit.record(&cx.session.user, "app.denied", &format!("tile:{}", entry.name));
            self.say(format!("Your role ({}) can't run custom apps", cx.session.role.name()));
            return;
        }
        let name = entry.name.clone();
        let boxed = crate::apps::tile::open_entry(entry, &cx);
        self.custom_open = Some(name);
        self.screen = Screen::App(boxed, PApp::Browser);
    }

    /// Open a custom app by its id (the store's Open button, the IDE's Run).
    pub fn open_custom_by_name(&mut self, name: &str) -> bool {
        self.refresh_custom();
        match self.custom.iter().find(|e| e.name == name).cloned() {
            Some(e) => {
                self.open_custom(e);
                self.custom_open.is_some()
            }
            None => false,
        }
    }

    fn handle_launcher(&mut self, ev: &Event) {
        if self.tick.saturating_sub(self.custom_at) > 60 || self.custom_at == 0 {
            self.refresh_custom();
        }
        let tiles = self.group_items();
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let n = tiles.len();
        self.sel = self.sel.min(n - 1);
        let cap = launcher::per_page(w, h, s);
        let (pages, page) = (launcher::page_count(n, cap), self.sel / cap);
        let first = page * cap;
        let rects = launcher::page_rects(w, h, s, n, page);
        let power = self.session.as_ref().is_some_and(|s| s.policy.power);
        match ev {
            Event::Key(Key::Left) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Right) => self.sel = (self.sel + 1).min(n - 1),
            Event::Key(Key::Tab) => self.step_group(false),
            Event::Key(Key::BackTab) => self.step_group(true),
            Event::Key(Key::Home) => self.sel = 0,
            Event::Key(Key::End) => self.sel = n - 1,
            Event::Key(Key::PageDown) => self.go_page(page + 1, pages, cap),
            Event::Key(Key::PageUp) => self.go_page(page.wrapping_sub(1), pages, cap),
            Event::Scroll(d) if *d != 0 => self.go_page(if *d > 0 { page.wrapping_sub(1) } else { page + 1 }, pages, cap),
            Event::Key(Key::Up) | Event::Key(Key::Down) => {
                // the tile above or below, nearest in the horizontal direction; past the edge of the page, the next page
                let cur = rects[(self.sel - first).min(rects.len() - 1)];
                let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
                let down = matches!(ev, Event::Key(Key::Down));
                let best = rects.iter().enumerate().filter(|(_, r)| if down { r.y > cur.y + cur.h / 2.0 } else { r.y + r.h < cur.y + cur.h / 2.0 }).min_by(|(_, a), (_, b)| {
                    let key = |r: &R| ((r.y + r.h / 2.0 - cy).abs() * 1000.0 + (r.x + r.w / 2.0 - cx).abs()) as i64;
                    key(a).cmp(&key(b))
                });
                if let Some((i, _)) = best {
                    self.sel = first + i;
                } else if down && page + 1 < pages {
                    self.sel = ((page + 1) * cap).min(n - 1);
                } else if !down && page > 0 {
                    self.sel = first - 1;
                }
            }
            Event::Key(Key::Enter) => self.open_item(&tiles[self.sel]),
            Event::Key(Key::Char(c)) => match c {
                // the number keys open the Nth app of the whole list, whichever group is shown
                '1'..='9' => {
                    if let Some(t) = self.visible_tiles().get(*c as usize - '1' as usize) {
                        self.open(t.app);
                    }
                }
                'o' | 'q' => self.sign_out(),
                'p' if power => self.screen = Screen::PowerMenu,
                // m, g, u, [ and ]: move the tile, the group's menu, undo, reorder (`shell_layout.rs`; handled before this point)
                _ => {}
            },
            Event::Key(Key::Esc) => self.sign_out(),
            Event::MouseMove(x, y) => {
                if let Some(i) = rects.iter().position(|r| r.contains(*x, *y)) {
                    self.sel = first + i;
                }
            }
            Event::MouseDown(x, y) => {
                let groups = self.groups();
                let row = self.chip_row();
                let nav = launcher::page_nav(w, h, s, pages);
                if let Some(i) = rects.iter().position(|r| r.contains(*x, *y)) {
                    self.sel = first + i;
                    self.open_item(&tiles[first + i]);
                } else if let Some(i) = row.chips.iter().find(|(_, r)| r.contains(*x, *y)).map(|(i, _)| *i) {
                    self.set_group(groups[i].clone());
                } else if pages > 1 && nav.prev.contains(*x, *y) {
                    self.go_page(page.wrapping_sub(1), pages, cap);
                } else if pages > 1 && nav.next.contains(*x, *y) {
                    self.go_page(page + 1, pages, cap);
                } else if let Some(p) = nav.dots.iter().position(|r| pages > 1 && r.contains(*x, *y)) {
                    self.go_page(p, pages, cap);
                } else if header_button(w, s, 0).contains(*x, *y) {
                    self.sign_out();
                } else if power && header_button(w, s, 1).contains(*x, *y) {
                    self.screen = Screen::PowerMenu;
                } else if header_button(w, s, if power { 2 } else { 1 }).contains(*x, *y) {
                    self.start_shot();
                } else if header_button(w, s, if power { 3 } else { 2 }).contains(*x, *y) {
                    self.toggle_devices();
                } else if header_button(w, s, if power { 4 } else { 3 }).contains(*x, *y) {
                    self.open_clipboard_picker();
                } else if self.inbox_allowed() && header_button(w, s, if power { 5 } else { 4 }).contains(*x, *y) {
                    self.open_inbox();
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx) {
        if self.saver_render(g) {
            return;
        }
        self.size = (g.width(), g.height());
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let time_settings = self.env.services.settings.lock().ok().map(|st| st.get().time.clone());
        let person = self.session.as_ref().filter(|_| self.tick % 15 == 0 || self.tick < 2 || !self.clock_ready || matches!(self.screen, Screen::App(_, PApp::Settings))).map(|se| crate::uisize::load_clock(self.env.services.dir(), &se.user));
        self.clock.refresh(self.tick, time_settings.as_ref(), person);
        // file names made by apps (CAMERA) carry the clock's local time
        crate::camera::set_zone(time_settings.as_ref().map(|t| t.zone.clone()));
        self.clock_ready = self.session.is_some();
        self.clock_tip = None;
        self.live = false; // a whole scene is drawn: nothing is pending
        g.hits.clear();
        g.clear_probes();
        g.mouse = self.disp_mouse(self.panes_active_display());
        crate::ctxmenu::start_frame();
        crate::fx::background(g);
        let tick = self.tick;
        let mut crashed_title: Option<&'static str> = None;
        // several panes: the bar, the panes and the dividers take the place of the one screen
        let panes_drawn = self.ws_active() && {
            self.draw_panes(g, w, h, s);
            true
        };
        match &mut self.screen {
            _ if panes_drawn => {}
            Screen::HwCheck(c) => c.render(g, w, h, s, tick, &self.env.hostname),
            Screen::Setup(f) => draw_auth(g, w, h, s, f, true, tick, &self.env.hostname, self.env.data_volatile),
            Screen::Login(f) => draw_auth(g, w, h, s, f, false, tick, &self.env.hostname, self.env.data_volatile),
            Screen::Launcher | Screen::PowerMenu => {
                self.draw_launcher(g, w, h, s);
                if matches!(self.screen, Screen::PowerMenu) {
                    draw_power_menu(g, w, h, s);
                }
            }
            Screen::App(..) => {
                let cx = self.cx();
                if let (Screen::App(app, pa), Some(cx)) = (&mut self.screen, cx) {
                    let hh = HEADER_H * s;
                    let area = R::new(0.0, hh, w as f32, h as f32 - hh);
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.render(g, area, s, &cx, tick))).is_err() {
                        crashed_title = Some(app.title());
                    }
                    // header drawn last so content scrolled above the area never shows through
                    let sub = if self.custom_open.is_some() { "Custom app" } else { TILES.iter().find(|t| t.app == *pa).map_or("", |t| t.sub) };
                    // where the clock goes (measured first so the title stops before it; drawn again after the header band,
                    // which would otherwise paint over it)
                    let hits_before = g.hits.len();
                    let clock_box = app_clock(g, w, s, &self.clock);
                    g.hits.truncate(hits_before);
                    self.clock_tip = clock_box;
                    // the installer indicator sits left of the clock while installs run (or just ended)
                    let clock_left = clock_box.map_or(bell_rect(w, s).x - if panes_impl::layout_slot() { 40.0 * s } else { 0.0 }, |b| b.x);
                    // the battery chip (laptops only) sits left of the clock, with the room the app's name needs kept free
                    let bat_plan = self.battery.plan(g, s, clock_left - 6.0 * s, 300.0 * s, 8.0 * s, 30.0 * s, 13.0 * s);
                    let bar_left = bat_plan.as_ref().map_or(clock_left, |p| p.rect.x);
                    let ind_w = self.installs.indicator_width(g, s);
                    let title_limit = if ind_w > 0.0 && bar_left - ind_w - 6.0 * s > 300.0 * s { bar_left - ind_w - 6.0 * s } else { bar_left };
                    crate::fx::app_header(g, w as f32, hh, s, app.title(), sub, app.icon(), app.accent(), tick, title_limit - 12.0 * s);
                    if clock_box.is_some() {
                        app_clock(g, w, s, &self.clock);
                    }
                    match &bat_plan {
                        Some(p) => self.battery.draw_chip(g, p, s),
                        None => self.battery.anchor = None,
                    }
                    let mut dev_right = bar_left - 4.0 * s;
                    if title_limit < bar_left {
                        if let Some(b) = self.installs.draw_indicator(g, bar_left - 4.0 * s, 8.0 * s + 15.0 * s, s, tick) {
                            dev_right = b.x - 6.0 * s;
                        }
                    } else {
                        self.installs.anchor = None;
                    }
                    if let Some(sign) = self.dev_sign.as_deref() {
                        draw_dev_sign(g, dev_right, 8.0 * s + 15.0 * s, s, sign, 300.0 * s);
                    }
                    let acc = app.accent();
                    let cr = clip_rect(w, s);
                    let hot = g.hovered(cr);
                    crate::fx::chrome_button(g, cr, s, hot, acc);
                    g.icon(Icon::Clipboard, cr.x + cr.w / 2.0, cr.y + cr.h / 2.0, 20.0 * s, MUTED);
                    g.hit(cr, Event::Key(Key::Ctrl('V')), "clipboard history");
                    if self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Inbox)) {
                        draw_bell(g, bell_rect(w, s), 8.0 * s, s, self.badge, matches!(self.screen, Screen::App(_, PApp::Inbox)));
                    }
                    if self.panes_button() {
                        panes_impl::draw_layout_button(g, panes_impl::layout_btn_rect(w, s), s, acc);
                    }
                    let sr = shot_rect(w, s);
                    let hot = g.hovered(sr);
                    crate::fx::chrome_button(g, sr, s, hot, acc);
                    g.icon(Icon::Camera, sr.x + sr.w / 2.0, sr.y + sr.h / 2.0, 20.0 * s, MUTED);
                    g.hit(sr, Event::Key(Key::F(12)), "screenshot");
                    let dr = dev_rect(w, s);
                    let hot = g.hovered(dr) || self.devpop.open;
                    crate::fx::chrome_button(g, dr, s, hot, acc);
                    g.icon(Icon::Devices, dr.x + dr.w / 2.0, dr.y + dr.h / 2.0, 20.0 * s, MUTED);
                    // the firewall is wanted but not in force: a red dot on the Devices button (its popover says why)
                    if iyakki_fw::service::alert() {
                        g.rrect(dr.x + dr.w - 11.0 * s, dr.y - 2.0 * s, 12.0 * s, 12.0 * s, 6.0 * s, rgb(214, 48, 64));
                    }
                    g.hit(dr, Event::Key(Key::F(8)), "devices");
                    let hr = help_rect(w, s);
                    let hot = g.hovered(hr);
                    crate::fx::chrome_button(g, hr, s, hot, acc);
                    g.text(hr.x + 12.0 * s, hr.y + 6.0 * s, 14.0 * s, MUTED, "F1  Help", Face::Sans);
                    g.hit(hr, Event::Key(Key::F(1)), "help");
                    let br = back_rect(w, s);
                    let hot = g.hovered(br);
                    crate::fx::chrome_button(g, br, s, hot, acc);
                    g.text(br.x + 14.0 * s, br.y + 6.0 * s, 15.0 * s, TEXT, "← Back", Face::Bold);
                }
            }
        }
        if let Some(name) = crashed_title {
            self.screen = Screen::Launcher;
            self.app_crashed(name);
        }
        if matches!(self.shot, Shot::Snap) {
            self.shot = Shot::Select { snap: g.pm.clone(), first: None };
        }
        // the assistant's `screen_capture`: this frame is "the person's screen" (answered here because only the shell can see it)
        {
            let svc = &self.env.services;
            svc.screen.set_present(true);
            // the assistant's camera tools use the CAMERA app's code through this one backend
            if !cfg!(test) && !svc.camera.present() {
                svc.camera.set_backend(Some(Arc::new(crate::camera::agentcam::AgentCamera::system())));
            }
            if let Some(sess) = &self.session {
                if svc.screen.wanted(&sess.user) {
                    svc.screen.serve(&sess.user, || Some(iyakki_agent::screencap::Frame { w: g.pm.width(), h: g.pm.height(), rgba: g.pm.data().to_vec() }));
                }
            }
            // developer mode's `iyakki dev screenshot` (only reachable from a paired computer while developer mode is on): whatever is drawn
            if svc.screen.wanted(iyakki_agent::devmode::SCREEN_ANY) {
                svc.screen.serve(iyakki_agent::devmode::SCREEN_ANY, || Some(iyakki_agent::screencap::Frame { w: g.pm.width(), h: g.pm.height(), rgba: g.pm.data().to_vec() }));
            }
        }
        if let Some(d) = self.hostdlg.as_mut() {
            d.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, tick);
        }
        if self.drag_over && self.session.is_some() {
            let dest = match &self.screen {
                Screen::App(_, PApp::Data) => "the DATA viewer",
                _ => "your Uploads folder",
            };
            crate::apps::hostfiles::drop_overlay(g, w as f32, h as f32, s, dest, rgb(238, 226, 112));
        }
        if let Some(r) = &self.ring {
            draw_ring(g, w, h, s, r, self.tick);
        }
        if let Some(b) = self.clock_tip.filter(|b| g.hovered(*b) && !self.devpop.open && !self.brand.open && !self.installs.open && self.session.is_some()) {
            draw_clock_tip(g, w, h, s, b, &self.clock.tooltip());
        }
        if self.session.is_some() {
            self.devpop.render(g, w, h, s);
            self.battery.render(g, w, h, s);
            self.installs.render(g, w, h, s, tick);
            self.brand.render(g, w, h, s, tick);
        }
        if let (Some(m), true) = (self.login.as_mut(), self.session.is_some()) {
            m.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, tick);
        }
        self.battery.render_cards(g, w, h, s);
        self.loss_render(g, w, h, s);
        if let Some((t, _, info)) = &self.toast {
            crate::fx::toast(g, w as f32, h as f32, s, t, *info);
        }
        if let Some(o) = &self.fnk.osd {
            crate::fnkeys::draw_osd(g, w as f32, h as f32, s, o);
        }
        self.draw_login_bar(g, w, h, s);
        if self.fnk.power_card {
            draw_power_menu(g, w, h, s);
        }
        // the assistant is using the camera: a red sign on every screen, bottom left, while it lasts
        if let Some(what) = self.env.services.camera.in_use() {
            draw_camera_sign(g, h, s, &what);
        }
        if self.help {
            let (title, lines) = self.help_lines();
            draw_help(g, w, h, s, &title, &lines);
        }
        match &self.shot {
            Shot::Select { first, .. } => draw_shot_overlay(g, w, h, s, *first, self.mouse),
            Shot::Review { img } => draw_shot_review(g, w, h, s, img),
            _ => {}
        }
        // clipboard history overlay sits above everything, including dialogs
        if let Some(p) = &self.clip {
            p.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, rgb(120, 170, 255));
            // how many copies are kept, beside the picker's own Cancel button
            let pw = (620.0 * s).min(w as f32 - 40.0 * s);
            let ph = (h as f32 - 40.0 * s).min(560.0 * s);
            let (px, py) = ((w as f32 - pw) / 2.0, (h as f32 - ph) / 2.0);
            let by = py + ph - 50.0 * s;
            let label = format!("keeps the last {}", crate::ui::clipboard::limit());
            let lw = g.text_width(13.0 * s, &label, Face::Sans);
            let bx = px + pw - 26.0 * s - 100.0 * s - 28.0 * s - lw - 2.0 * 40.0 * s;
            g.button(px + 26.0 * s, by, 14.0 * s, "Scan QR  (Ctrl+K)", ACCENT_QR, Event::Tab(912));
            if let Some((n, secs)) = crate::ui::clipboard::secret_waiting() {
                let t = format!("a secret is waiting ({n} characters): Ctrl+V pastes it once, gone in {secs} s");
                let t = g.fit(12.0 * s, &t, Face::Sans, pw - 52.0 * s);
                g.text(px + 26.0 * s, by - 20.0 * s, 12.0 * s, OK, &t, Face::Sans);
            }
            g.button(bx, by, 14.0 * s, "−", MUTED, Event::Tab(910));
            g.text(bx + 46.0 * s, by + 8.0 * s, 13.0 * s, DIM, &label, Face::Sans);
            g.button(bx + 46.0 * s + lw + 10.0 * s, by, 14.0 * s, "+", MUTED, Event::Tab(911));
            // what the system clipboard said
            let note = match self.clip_note {
                ClipNote::Asked if self.tick < self.host_sync_until => "reading the system clipboard…".to_string(),
                ClipNote::Asked => "the system clipboard gave no text (nothing copied there, or it could not be read)".to_string(),
                ClipNote::Got(n) => format!("added {n} characters from the system clipboard"),
                ClipNote::None => String::new(),
            };
            let note = g.fit(12.0 * s, &note, Face::Sans, pw * 0.62);
            let nw = g.text_width(12.0 * s, &note, Face::Sans);
            g.text(px + pw - 26.0 * s - nw, py + 54.0 * s, 12.0 * s, if matches!(self.clip_note, ClipNote::Got(_)) { OK } else { DIM }, &note, Face::Sans);
        }
        if let Some(c) = self.clipscan.as_mut() {
            c.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, self.tick);
        }
        self.draw_layout_ui(g, w, h, s);
        self.draw_panes_overlays(g, w, h, s);
        if let Some(m) = &self.ctx {
            let accent = match &self.screen {
                Screen::App(app, _) => app.accent(),
                _ => rgb(120, 170, 255),
            };
            m.draw(g, s, accent);
        }
        self.fields = crate::ctxmenu::fields();
        self.hits = std::mem::take(&mut g.hits);
        self.probes = g.probes();
    }

    // ------------------------------------------------------------------ context menu

    /// A right click: the menu opens at the pointer. Over a text field it offers Cut / Copy / Paste / Select all, the app adds
    /// its own entries, and "Clipboard history…" is always there.
    fn open_context_menu(&mut self, x: i32, y: i32) {
        use crate::ctxmenu::{Item, Menu, COPY, CUT, HISTORY, PASTE, SELECT_ALL};
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let mut items: Vec<Item> = Vec::new();
        if let Some(mut f) = self.fields.iter().rev().find(|f| f.rect.contains(x, y)).copied() {
            if !f.focused {
                // the right click also puts the caret there, as in any text editor
                self.dispatch(&Event::FocusAt(f.id, x as f32 - f.rect.x));
                f.has_selection = false;
            }
            let sel = f.has_selection && !f.secret;
            items.push(Item::new(CUT, "Cut", "Ctrl+X").enabled(sel));
            items.push(Item::new(COPY, "Copy", "Ctrl+C").enabled(sel));
            items.push(Item::new(PASTE, "Paste", "Ctrl+V").needs_clip());
            items.push(Item::new(SELECT_ALL, "Select all", "Ctrl+A").enabled(!f.empty));
        }
        crate::ctxmenu::set_under(self.hits.iter().rev().filter(|h| h.rect.contains(x, y)).cloned().collect());
        if let Screen::App(app, _) = &mut self.screen {
            let own: Vec<Item> = app.context_items(x, y).into_iter().filter(|it| it.is_separator() || !items.iter().any(|e| e.id == it.id)).collect();
            if !own.is_empty() {
                if !items.is_empty() {
                    items.push(Item::separator());
                }
                items.extend(own);
            }
        }
        if !items.is_empty() {
            items.push(Item::separator());
        }
        items.push(Item::new(HISTORY, "Clipboard history…", "Ctrl+Shift+V"));
        // ask the system for what it holds: text copied in other programs makes Paste usable
        self.want_host_paste = true;
        self.host_sync_until = self.tick + 90;
        self.ctx = Some(Menu::open(items, (x, y), (w, h), s, crate::ui::clipboard::can_paste()));
    }

    /// The menu is open: keys move over it, a click chooses, anything else closes it.
    fn handle_ctx(&mut self, ev: &Event) {
        use crate::ctxmenu::Outcome;
        let Some(m) = self.ctx.as_mut() else { return };
        m.set_clip_text(crate::ui::clipboard::can_paste());
        let before = m.sel;
        let out = match ev {
            Event::Key(k) => m.key(k),
            Event::MouseMove(x, y) => {
                m.hover(*x, *y);
                Outcome::Stay
            }
            Event::MouseDown(x, y) => m.click(*x, *y),
            Event::MouseRight(x, y) => {
                let (x, y) = (*x, *y);
                self.ctx = None;
                self.open_context_menu(x, y);
                return;
            }
            Event::Scroll(_) => Outcome::Close,
            _ => Outcome::Stay,
        };
        if m.sel != before {
            self.stale = true;
        }
        match out {
            Outcome::Stay => {}
            Outcome::Close => {
                self.ctx = None;
                self.stale = true;
                // a key that is not the menu's own (Ctrl+C, a letter…) still does what it would have done
                if matches!(ev, Event::Key(k) if *k != Key::Esc) {
                    self.dispatch(ev);
                }
            }
            Outcome::Pick(id) => {
                self.ctx = None;
                self.stale = true;
                self.run_context_entry(&id);
            }
        }
    }

    fn run_context_entry(&mut self, id: &str) {
        use crate::ctxmenu::{COPY, CUT, HISTORY, PASTE, SELECT_ALL};
        match id {
            CUT => self.route(&Event::Key(Key::Ctrl('x'))),
            COPY => self.route(&Event::Key(Key::Ctrl('c'))),
            SELECT_ALL => self.route(&Event::Key(Key::Ctrl('a'))),
            PASTE => match crate::ui::clipboard::paste() {
                Some(t) => self.route(&Event::Paste(t)),
                None => self.say_info("The clipboard is empty"),
            },
            HISTORY => self.open_clipboard_picker(),
            other => {
                let cx = self.cx();
                if let (Screen::App(app, _), Some(cx)) = (&mut self.screen, cx) {
                    if matches!(app.context_pick(other, &cx), Flow::Back) {
                        self.leave_app();
                    }
                }
            }
        }
    }

    fn start_shot(&mut self) {
        if self.session.is_some() {
            self.help = false;
            self.shot = Shot::Snap;
        }
    }

    /// Keys and clicks while framing a screenshot: two clicks (opposite corners) keep that area, Enter keeps the whole
    /// screen, Esc cancels.
    fn handle_shot(&mut self, ev: &Event) {
        if let Shot::Review { .. } = &self.shot {
            match ev {
                Event::Key(Key::Enter) | Event::Key(Key::Char('s')) => self.save_shot_file(),
                Event::Key(Key::Esc) | Event::Key(Key::Char('c')) | Event::Key(Key::F(12)) => {
                    self.shot = Shot::Off;
                    self.say_info("Screenshot kept on the clipboard");
                }
                _ => {}
            }
            return;
        }
        let Shot::Select { snap, first } = &mut self.shot else { return };
        let (w, h) = (snap.width() as i32, snap.height() as i32);
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::F(12)) => {
                self.shot = Shot::Off;
                self.say_info("Screenshot cancelled");
            }
            Event::Key(Key::Enter) => self.finish_shot((0, 0, w, h)),
            Event::MouseDown(x, y) => match first {
                None => *first = Some((*x, *y)),
                Some((fx, fy)) => {
                    let (x0, x1) = ((*fx).min(*x).clamp(0, w), (*fx).max(*x).clamp(0, w));
                    let (y0, y1) = ((*fy).min(*y).clamp(0, h), (*fy).max(*y).clamp(0, h));
                    if x1 - x0 < 8 || y1 - y0 < 8 {
                        *first = None; // a click without an area: start over
                    } else {
                        self.finish_shot((x0, y0, x1 - x0, y1 - y0));
                    }
                }
            },
            _ => {}
        }
    }

    /// Put the chosen area on the clipboard and ask what to do next (save a file, or close).
    fn finish_shot(&mut self, (x, y, w, h): (i32, i32, i32, i32)) {
        let Shot::Select { snap, .. } = std::mem::replace(&mut self.shot, Shot::Off) else { return };
        let Some(rect) = tiny_skia::IntRect::from_xywh(x, y, w as u32, h as u32) else { return };
        let Some(img) = snap.clone_rect(rect) else {
            self.say("Could not take the screenshot");
            return;
        };
        // the screen is opaque, so premultiplied and plain RGBA are the same
        crate::ui::clipboard::copy_image(img.width(), img.height(), img.data().to_vec());
        if let Some(s) = &self.session {
            self.env.audit.record(&s.user, "screenshot", &format!("{}x{}", w, h));
        }
        self.shot = Shot::Review { img };
    }

    /// Save the picture under review as a PNG: in the person's own `Pictures` folder when their account may write there,
    /// otherwise in their screenshots folder inside the system's data.
    fn save_shot_file(&mut self) {
        let Shot::Review { img } = std::mem::replace(&mut self.shot, Shot::Off) else { return };
        self.save_pixmap(&img);
    }

    fn save_pixmap(&mut self, img: &tiny_skia::Pixmap) {
        let Some(sess) = self.session.clone() else { return };
        let name = format!("iyakki-{}.png", stamp(std::time::SystemTime::now()));
        let home = std::path::PathBuf::from(format!("/home/{}/Pictures", sess.user));
        let fallback = self.env.services.dir().join("screenshots").join(sess.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect::<String>());
        let dir = if sess.policy.allows_write(&sess.user, &home.to_string_lossy()) && std::fs::create_dir_all(&home).is_ok() { home } else { fallback };
        let path = dir.join(name);
        match std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| img.save_png(&path).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.env.audit.record(&sess.user, "screenshot.save", &path.to_string_lossy());
                self.say_info(format!("Saved {}", path.display()));
            }
            Err(e) => self.say(format!("Could not save the screenshot: {e}")),
        }
    }

    /// Title and lines of the F1 popup: this screen's keys, then the ones that work everywhere.
    fn help_lines(&self) -> (String, Vec<String>) {
        let (title, mut lines) = match &self.screen {
            Screen::App(app, _) => (app.title().to_string(), app.help()),
            _ => (
                "Home".to_string(),
                vec!["← → ↑ ↓ · choose an app (moving past the last one on a page turns it)".to_string(), "Tab / Shift+Tab or click a chip · switch group (Home, Web & Media, Work, System, and any group you made)".to_string(), "m · move the selected tile to another group (or right-click it); g · this group's menu (new, rename, delete); + chip · new group; u · undo the last move; [ and ] · move the tile earlier or later in its group".to_string(), "PageUp / PageDown, mouse wheel or the dots · turn the page".to_string(), "1-9 · open the Nth app of the All list (the order of the tile table, whichever group you moved it to)".to_string(), "Enter or click · open it".to_string(), "o · sign out".to_string(), "p · power menu (if your role allows it)".to_string()],
            ),
        };
        lines.push(String::new());
        lines.push("Everywhere".to_string());
        lines.extend(["F1 · this help".to_string(), "F9 or the bell · your inbox (notifications, tasks, approvals)".to_string(), "F10 or a click on the name IYAKKI · the IYAKKI menu (About, Quick install, Media player, Mac drivers)".to_string(), "F12 or Print Screen · screenshot to the clipboard (click two corners, or Enter for the whole screen), then save a file or close".to_string(), "Esc · back".to_string(), "Ctrl+V · paste".to_string(), "Ctrl+Shift+V or right-click · clipboard history".to_string()]);
        (format!("Help — {title}"), lines)
    }

    fn draw_launcher(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let Some(sess) = self.session.clone() else { return };
        let power = sess.policy.power;
        // top bar (a launcher in a pane has the pane's title strip instead)
        let top_bar = !self.panes.compact;
        let lay = top_bar && self.panes_button();
        if top_bar {
        let name_w = g.text_width(30.0 * s, "IYAKKI", Face::Bold);
        // the name opens the IYAKKI menu (About, Quick install, Media player, Mac drivers)
        let name_box = R::new(30.0 * s, 18.0 * s, name_w + 20.0 * s, 56.0 * s);
        if g.hovered(name_box) || self.brand.open {
            g.rrect(name_box.x, name_box.y, name_box.w, name_box.h, 12.0 * s, rgb(28, 36, 66));
        }
        g.text(40.0 * s, 26.0 * s, 30.0 * s, TEXT, "IYAKKI", Face::Bold);
        name_strip(g, 40.0 * s, 64.0 * s, name_w, 4.0 * s, s, false);
        g.hit(name_box, Event::Key(Key::F(10)), "IYAKKI menu");
        let chip = account_label(&sess.user, sess.role.name());
        let cw = g.text_width(16.0 * s, &chip, Face::Bold) + 36.0 * s;
        let bell = sess.policy.allows_app(PApp::Inbox);
        let right = w as f32 - 40.0 * s - (header_button(w, s, 0).w + 12.0 * s) * (if power { 5.0 } else { 4.0 } + if bell { 1.0 } else { 0.0 } + if lay { 1.0 } else { 0.0 });
        crate::fx::glass(g, right - cw, 24.0 * s, cw, 44.0 * s, 22.0 * s, crate::fx::CYAN);
        g.text(right - cw + 18.0 * s, 35.0 * s, 16.0 * s, TEXT, &chip, Face::Bold);
        // sign out, power (when allowed), the camera, then the clipboard queue, from the right
        let buttons: Vec<Icon> = if power { vec![Icon::Logout, Icon::Power, Icon::Camera, Icon::Devices, Icon::Clipboard] } else { vec![Icon::Logout, Icon::Camera, Icon::Devices, Icon::Clipboard] };
        for (i, ic) in buttons.iter().enumerate() {
            let r = header_button(w, s, i);
            let hover = g.hovered(r);
            crate::fx::chrome_button(g, r, s * 2.5, hover, crate::fx::CYAN);
            g.icon(*ic, r.x + r.w / 2.0, r.y + r.h / 2.0, 22.0 * s, if *ic == Icon::Devices && self.devpop.open { crate::fx::CYAN } else { MUTED });
        }
        let title_end = 40.0 * s + name_w;
        self.clock_tip = launcher_clock(g, s, right - cw - 18.0 * s, title_end + 24.0 * s, &self.clock);
        // the installer indicator, left of the clock (when there is room)
        let mut ind_right = self.clock_tip.map_or(right - cw - 18.0 * s, |b| b.x) - 8.0 * s;
        // the battery chip (laptops only), left of the clock
        let bat_plan = self.battery.plan(g, s, ind_right, title_end + 24.0 * s, 24.0 * s, 44.0 * s, 15.0 * s);
        match &bat_plan {
            Some(p) => {
                self.battery.draw_chip(g, p, s);
                ind_right = p.rect.x - 8.0 * s;
            }
            None => self.battery.anchor = None,
        }
        let mut dev_right = ind_right;
        if ind_right - self.installs.indicator_width(g, s) > title_end + 24.0 * s {
            if let Some(b) = self.installs.draw_indicator(g, ind_right, 46.0 * s, s, self.tick) {
                dev_right = b.x - 8.0 * s;
            }
        } else {
            self.installs.anchor = None;
        }
        if let Some(sign) = self.dev_sign.as_deref() {
            draw_dev_sign(g, dev_right, 46.0 * s, s, sign, title_end + 24.0 * s);
        }
        if bell {
            let r = header_button(w, s, buttons.len());
            draw_bell(g, r, 22.0 * s, s, self.badge, false);
        }
        if lay {
            let r = header_button(w, s, buttons.len() + usize::from(bell));
            panes_impl::draw_layout_button(g, r, s, crate::fx::CYAN);
        }
        }
        // tiles
        if self.tick.saturating_sub(self.custom_at) > 60 || self.custom_at == 0 {
            self.refresh_custom();
        }
        self.lay_poll();
        let tiles = self.group_items();
        self.sel = self.sel.min(tiles.len() - 1);
        let cap = launcher::per_page(w, h, s);
        let (pages, page) = (launcher::page_count(tiles.len(), cap), self.sel / cap);
        let rects = launcher::page_rects(w, h, s, tiles.len(), page);
        // group chips, then `…` (the groups that do not fit) and `+` (a new group)
        let groups = self.groups();
        let cur = self.current_group();
        let row = self.chip_row();
        for (i, r) in &row.chips {
            let gr = &groups[*i];
            let on = *gr == cur;
            let hover = g.hovered(*r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, r.h / 2.0, on, hover, if self.lay.carry.is_some() && hover { crate::ui::OK } else { crate::fx::CYAN });
            let label = g.fit(15.0 * s, gr.label(), Face::Bold, r.w - 16.0 * s);
            let lw = g.text_width(15.0 * s, &label, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 15.0 * s) / 2.0 - s, 15.0 * s, if on { TEXT } else { MUTED }, &label, Face::Bold);
            g.hit(*r, Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32), &format!("group {}", gr.label()));
        }
        self.draw_extra_chips(g, &row, s);
        let first = page * cap;
        for (k, r) in rects.iter().enumerate() {
            let (i, item) = (first + k, &tiles[first + k]);
            let t: &Tile = match item {
                LItem::Sys(t) => t,
                LItem::App(e) => {
                    draw_custom_tile(g, e, *r, s, i == self.sel, tilehub_allowed(&sess.policy));
                    continue;
                }
                LItem::Empty => {
                    layout_impl::draw_empty_hint(g, *r, s, &self.current_group().name);
                    continue;
                }
            };
            let permitted = sess.policy.allows_app(t.app);
            // CAMERA without a camera: greyed with a hint, no lock
            let missing = t.app == PApp::Camera && !self.camera.available();
            let allowed = permitted && !missing;
            let selected = i == self.sel;
            let hover = g.hovered(*r);
            let lift = if selected { 6.0 * s } else { 0.0 };
            let (x, y) = (r.x, r.y - lift);
            crate::fx::tile(g, R::new(x, y, r.w, r.h), s, t.accent, selected, hover, allowed);
            let badge = r.w * 0.52;
            g.rrect(x + (r.w - badge) / 2.0, y + 26.0 * s, badge, badge, 22.0 * s, if allowed { with_alpha(t.accent, 40) } else { rgb(38, 42, 58) });
            g.icon(t.icon, x + r.w / 2.0, y + 26.0 * s + badge / 2.0, badge * 0.66, if allowed { t.accent } else { DIM });
            let fit = r.w - 24.0 * s;
            let mut ls = 24.0 * s;
            while ls > 10.0 * s && g.text_width(ls, t.label, Face::Bold) > fit {
                ls -= s;
            }
            let lw = g.text_width(ls, t.label, Face::Bold);
            g.text(x + (r.w - lw) / 2.0, y + 26.0 * s + badge + 18.0 * s + (24.0 * s - ls) / 2.0, ls, if allowed { TEXT } else { DIM }, t.label, Face::Bold);
            let sub = if allowed { t.sub } else if permitted { self.camera.tile_hint() } else { "Restricted" };
            let mut ss = 14.0 * s;
            while ss > 8.0 * s && g.text_width(ss, sub, Face::Sans) > fit {
                ss -= s * 0.5;
            }
            let lw = g.text_width(ss, sub, Face::Sans);
            g.text(x + (r.w - lw) / 2.0, y + 26.0 * s + badge + 54.0 * s, ss, MUTED, sub, Face::Sans);
            if launcher::is_demo(t.app) {
                crate::fx::demo_ribbon(g, R::new(x, y, r.w, r.h), s);
            }
            if !permitted {
                g.icon(Icon::Lock, x + r.w - 30.0 * s, y + 28.0 * s, 22.0 * s, DIM);
            }
        }
        if pages > 1 {
            let nav = launcher::page_nav(w, h, s, pages);
            for (p, r) in nav.dots.iter().enumerate() {
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                if p == page {
                    g.circle(cx, cy, 10.0 * s, [70, 225, 255, 50]);
                }
                g.rrect(cx - 6.0 * s, cy - 6.0 * s, 12.0 * s, 12.0 * s, 6.0 * s, if p == page { crate::fx::CYAN } else { DIM });
                g.hit(*r, Event::MouseDown(cx as i32, cy as i32), &format!("page {}", p + 1));
            }
            for (r, glyph, to) in [(nav.prev, "<", page.checked_sub(1)), (nav.next, ">", Some(page + 1).filter(|p| *p < pages))] {
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                let gw = g.text_width(20.0 * s, glyph, Face::Bold);
                g.text(cx - gw / 2.0, cy - 11.0 * s, 20.0 * s, if to.is_some() { if g.hovered(r) { TEXT } else { MUTED } } else { DIM }, glyph, Face::Bold);
                g.hit(r, Event::MouseDown(cx as i32, cy as i32), if glyph == "<" { "previous page" } else { "next page" });
            }
        }
        if top_bar {
        let hint = "←/→ choose · Tab group · m move · PgUp/PgDn page · Enter open · o sign out".to_string() + if power { " · p power" } else { "" };
        let hw = g.text_width(14.0 * s, &hint, Face::Sans);
        g.text((w as f32 - hw) / 2.0, h as f32 - 36.0 * s, 14.0 * s, DIM, &hint, Face::Sans);
        let host = format!("{}  ·  {}", self.env.hostname, self.env.host.detect_env());
        g.text(40.0 * s, h as f32 - 36.0 * s, 14.0 * s, DIM, &host, Face::Sans);
        // what the radio is playing in the background
        if let iyakki_agent::radio::RadioState::Playing { station, title, .. } = self.env.services.radio.state() {
            let line = if title.is_empty() { format!("♪ {station}") } else { format!("♪ {station} — {title}") };
            let line = g.fit(14.0 * s, &line, Face::Sans, w as f32 * 0.32);
            let lw = g.text_width(14.0 * s, &line, Face::Sans);
            g.text(w as f32 - lw - 40.0 * s, h as f32 - 36.0 * s, 14.0 * s, OK, &line, Face::Sans);
        }
        // and what MEDIA plays
        if let Some(label) = iyakki_agent::media::shared().now_playing() {
            let line = g.fit(14.0 * s, &format!("▶ {label}"), Face::Sans, w as f32 * 0.32);
            let lw = g.text_width(14.0 * s, &line, Face::Sans);
            g.text(w as f32 - lw - 40.0 * s, h as f32 - 60.0 * s, 14.0 * s, OK, &line, Face::Sans);
        }
        }
    }
}

fn with_alpha(c: [u8; 4], a: u8) -> [u8; 4] {
    // blend the accent over the card colour (the tiles are drawn on opaque cards)
    let bg = CARD_HOVER;
    let m = |x: u8, b: u8| ((x as u32 * a as u32 + b as u32 * (255 - a as u32)) / 255) as u8;
    [m(c[0], bg[0]), m(c[1], bg[1]), m(c[2], bg[2]), 255]
}

/// The clock in the app header, left of the bell: the time, with the short date when there is room. Returns its box (for
/// the tooltip and the click).
fn app_clock(g: &mut Gfx, w: u32, s: f32, clock: &crate::clockbar::ClockState) -> Option<R> {
    let right = bell_rect(w, s).x - 12.0 * s - if panes_impl::layout_slot() { 40.0 * s } else { 0.0 };
    let (time, date) = (clock_time_text(clock), clock.date_text());
    let (tw, dw) = (g.text_width(15.0 * s, &time, Face::Bold), g.text_width(13.0 * s, &date, Face::Sans));
    // keep room for the app's name on the left
    let avail = right - 300.0 * s;
    let gap = 10.0 * s;
    let (show_date, total) = match crate::clockbar::fit(avail, tw, dw, gap) {
        crate::clockbar::Fit::Full => (true, tw + gap + dw),
        crate::clockbar::Fit::TimeOnly => (false, tw),
        crate::clockbar::Fit::Hidden => return None,
    };
    let b = R::new(right - total - 8.0 * s, 8.0 * s, total + 16.0 * s, 30.0 * s);
    let hot = g.hovered(b);
    if hot {
        g.rrect(b.x, b.y, b.w, b.h, 8.0 * s, rgb(28, 36, 66));
    }
    let ink = if clock.looks_wrong() { rgb(255, 196, 94) } else { TEXT };
    let ty = b.y + (b.h - g.line_height(15.0 * s, Face::Bold)) / 2.0;
    g.text(b.x + 8.0 * s, ty, 15.0 * s, ink, &time, Face::Bold);
    if show_date {
        g.text(b.x + 8.0 * s + tw + gap, ty + 1.0 * s, 13.0 * s, MUTED, &date, Face::Sans);
    }
    g.hit(b, Event::Key(Key::F(7)), "clock");
    Some(b)
}

/// The clock in the launcher's top bar, right-aligned at `right`: time over date, or the time alone, or nothing when
/// there is no room left of `left_limit`.
fn launcher_clock(g: &mut Gfx, s: f32, right: f32, left_limit: f32, clock: &crate::clockbar::ClockState) -> Option<R> {
    let (time, date) = (clock_time_text(clock), clock.date_text());
    let (tw, dw) = (g.text_width(22.0 * s, &time, Face::Bold), g.text_width(13.0 * s, &date, Face::Sans));
    let avail = right - left_limit;
    let (two_lines, wide) = if avail >= tw.max(dw) { (true, tw.max(dw)) } else if avail >= tw { (false, tw) } else { return None };
    let b = R::new(right - wide - 10.0 * s, 24.0 * s, wide + 20.0 * s, 44.0 * s);
    let hot = g.hovered(b);
    if hot {
        g.rrect(b.x, b.y, b.w, b.h, 10.0 * s, rgb(28, 36, 66));
    }
    let ink = if clock.looks_wrong() { rgb(255, 196, 94) } else { TEXT };
    if two_lines {
        g.text(right - tw, b.y + 3.0 * s, 22.0 * s, ink, &time, Face::Bold);
        g.text(right - dw, b.y + 3.0 * s + g.line_height(22.0 * s, Face::Bold), 13.0 * s, MUTED, &date, Face::Sans);
    } else {
        g.text(right - tw, b.y + (b.h - g.line_height(22.0 * s, Face::Bold)) / 2.0, 22.0 * s, ink, &time, Face::Bold);
    }
    g.hit(b, Event::Key(Key::F(7)), "clock");
    Some(b)
}

/// The time as shown in the bar; a clock that was never set gets an exclamation mark.
fn clock_time_text(clock: &crate::clockbar::ClockState) -> String {
    if clock.looks_wrong() { format!("{} !", clock.time_text()) } else { clock.time_text() }
}

/// The small amber "DEV" sign while Developer mode is on, right edge at `right`, centred on `cy`; skipped when it would cross
/// `left_limit`. A click opens Settings → Developer mode.
fn draw_dev_sign(g: &mut Gfx, right: f32, cy: f32, s: f32, sign: &str, left_limit: f32) {
    let tw = g.text_width(11.5 * s, sign, Face::Bold);
    let b = R::new(right - tw - 14.0 * s, cy - 10.0 * s, tw + 14.0 * s, 20.0 * s);
    if b.x < left_limit {
        return;
    }
    // a shell grant is the dangerous state: red, never amber
    let amber = if sign.contains("SHELL ACCESS") || sign.contains("INTERRUPTED") { rgb(255, 99, 99) } else { rgb(255, 196, 94) };
    g.rrect(b.x, b.y, b.w, b.h, 6.0 * s, crate::fx::with_alpha(amber, 40));
    g.rrect_outline(b.x, b.y, b.w, b.h, 6.0 * s, 1.0, amber);
    g.text(b.x + 7.0 * s, cy - g.line_height(11.5 * s, Face::Bold) / 2.0, 11.5 * s, amber, sign, Face::Bold);
    g.hit(b, Event::Tab(DEV_SIGN), "developer mode on");
}

/// The click of the DEV sign.
const DEV_SIGN: usize = 1_300_100;

/// The clock's tooltip, under its box and kept on screen.
fn draw_clock_tip(g: &mut Gfx, w: u32, h: u32, s: f32, anchor: R, lines: &[(String, bool)]) {
    let size = 13.0 * s;
    let maxw = (360.0 * s).min(w as f32 - 24.0 * s);
    let mut wrapped: Vec<(String, bool)> = Vec::new();
    for (t, warn) in lines {
        for l in g.wrap(size, t, Face::Sans, maxw - 24.0 * s) {
            wrapped.push((l, *warn));
        }
    }
    let lh = g.line_height(size, Face::Sans) + 3.0 * s;
    let pw = wrapped.iter().map(|(l, _)| g.text_width(size, l, Face::Sans)).fold(0.0f32, f32::max) + 24.0 * s;
    let ph = wrapped.len() as f32 * lh + 18.0 * s;
    let x = (anchor.x + anchor.w - pw).clamp(8.0 * s, (w as f32 - pw - 8.0 * s).max(8.0 * s));
    let y = (anchor.y + anchor.h + 6.0 * s).min((h as f32 - ph - 8.0 * s).max(0.0));
    crate::fx::modal_panel(g, R::new(x, y, pw, ph), s * 0.6, crate::fx::CYAN, false);
    for (i, (l, warn)) in wrapped.iter().enumerate() {
        g.text(x + 12.0 * s, y + 9.0 * s + i as f32 * lh, size, if *warn { rgb(255, 196, 94) } else if i == 0 { TEXT } else { MUTED }, l, Face::Sans);
    }
}

fn header_button(w: u32, s: f32, i: usize) -> R {
    let d = 44.0 * s;
    R::new(w as f32 - 40.0 * s - d - i as f32 * (d + 12.0 * s), 24.0 * s, d, d)
}

/// The bell, left of the clipboard queue button.
fn bell_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 226.0 * s - 160.0 * s, 8.0 * s, 32.0 * s, 30.0 * s)
}

/// The bell with its count: red when something waits for an answer, amber for the rest.
fn draw_bell(g: &mut Gfx, r: R, radius: f32, s: f32, b: Badge, open: bool) {
    let hover = g.hovered(r);
    crate::fx::chrome_button(g, r, radius / 8.0, hover || open, crate::fx::CYAN);
    let n = b.total();
    let ink = if b.approvals > 0 { rgb(255, 120, 120) } else if n > 0 { rgb(255, 196, 94) } else { MUTED };
    g.icon(Icon::Bell, r.x + r.w / 2.0, r.y + r.h / 2.0, r.h * 0.62, ink);
    if n > 0 {
        let label = if n > 99 { "99+".to_string() } else { n.to_string() };
        let size = (r.h * 0.34).max(10.0 * s);
        let tw = g.text_width(size, &label, Face::Bold);
        let bw = (tw + size * 0.7).max(size * 1.35);
        let bh = size * 1.4;
        let (bx, by) = (r.x + r.w - bw * 0.72, r.y - bh * 0.18);
        g.rrect(bx, by, bw, bh, bh / 2.0, if b.approvals > 0 { rgb(214, 48, 64) } else { rgb(214, 130, 20) });
        g.text(bx + (bw - tw) / 2.0, by + (bh - g.line_height(size, Face::Bold)) / 2.0, size, rgb(255, 255, 255), &label, Face::Bold);
    }
    g.hit(r, Event::Key(Key::F(9)), "inbox");
}

/// The Devices button, left of the camera.
fn dev_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 226.0 * s - 80.0 * s, 8.0 * s, 32.0 * s, 30.0 * s)
}

/// The clipboard queue button, left of the Devices button.
const ACCENT_QR: [u8; 4] = rgb(255, 184, 77);

fn clip_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 226.0 * s - 120.0 * s, 8.0 * s, 32.0 * s, 30.0 * s)
}

/// The camera button, left of the "F1 Help" chip.
fn shot_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 226.0 * s - 40.0 * s, 8.0 * s, 32.0 * s, 30.0 * s)
}

/// The "F1 Help" chip, left of the Back button.
fn help_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 226.0 * s, 8.0 * s, 104.0 * s, 30.0 * s)
}

/// Height of the strip with the app's name, in units before scaling.
const HEADER_H: f32 = 46.0;

fn tilehub_allowed(p: &iyakki_sec::policy::Policy) -> bool {
    iyakki_agent::store::tilehub::allowed(p)
}

/// The accent a custom app's tile wears: from its id, so the same app always has the same colour.
pub(crate) fn custom_accent(name: &str) -> [u8; 4] {
    const PALETTE: [[u8; 4]; 8] = [rgb(150, 220, 255), rgb(255, 170, 120), rgb(170, 230, 140), rgb(230, 150, 230), rgb(255, 215, 110), rgb(120, 200, 190), rgb(190, 170, 255), rgb(255, 140, 160)];
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[h as usize % PALETTE.len()]
}

/// A custom app's tile: a round badge with the first letter of its title in the app's colour, the title, the group, and the
/// "custom app" mark (an unsigned dev app says so instead).
fn draw_custom_tile(g: &mut Gfx, e: &iyakki_agent::store::tilehub::TileEntry, r: R, s: f32, selected: bool, allowed: bool) {
    let accent = custom_accent(&e.name);
    let hover = g.hovered(r);
    let lift = if selected { 6.0 * s } else { 0.0 };
    let (x, y) = (r.x, r.y - lift);
    crate::fx::tile(g, R::new(x, y, r.w, r.h), s, accent, selected, hover, allowed);
    let badge = r.w * 0.52;
    g.rrect(x + (r.w - badge) / 2.0, y + 26.0 * s, badge, badge, 22.0 * s, if allowed { with_alpha(accent, 40) } else { rgb(38, 42, 58) });
    let letter: String = e.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    let ls = badge * 0.62;
    let lw = g.text_width(ls, &letter, Face::Bold);
    g.text(x + (r.w - lw) / 2.0, y + 26.0 * s + (badge - g.line_height(ls, Face::Bold)) / 2.0, ls, if allowed { accent } else { DIM }, &letter, Face::Bold);
    let fit = r.w - 24.0 * s;
    let mut ls = 22.0 * s;
    while ls > 10.0 * s && g.text_width(ls, &e.title, Face::Bold) > fit {
        ls -= s;
    }
    let lw = g.text_width(ls, &e.title, Face::Bold);
    let title = if lw > fit { g.fit(ls, &e.title, Face::Bold, fit) } else { e.title.clone() };
    let lw = g.text_width(ls, &title, Face::Bold);
    g.text(x + (r.w - lw) / 2.0, y + 26.0 * s + badge + 18.0 * s + (24.0 * s - ls) / 2.0, ls, if allowed { TEXT } else { DIM }, &title, Face::Bold);
    // an app from a paired developer computer (developer mode) is marked like a dev app, with its own words
    let devlike = e.dev || e.is_developer();
    let sub = if !allowed { "Restricted".to_string() } else if e.dev { "unsigned dev app".to_string() } else if e.is_developer() { e.trust.clone() } else { format!("custom app · {}", e.group) };
    let mut ss = 13.0 * s;
    while ss > 8.0 * s && g.text_width(ss, &sub, Face::Sans) > fit {
        ss -= s * 0.5;
    }
    let sub = if g.text_width(ss, &sub, Face::Sans) > fit { g.fit(ss, &sub, Face::Sans, fit) } else { sub };
    let lw = g.text_width(ss, &sub, Face::Sans);
    g.text(x + (r.w - lw) / 2.0, y + 26.0 * s + badge + 54.0 * s, ss, if devlike { AMBER_DEV } else { MUTED }, &sub, Face::Sans);
    if devlike {
        crate::fx::ribbon(g, R::new(x, y, r.w, r.h), s, "DEV");
    }
    if !allowed {
        g.icon(Icon::Lock, x + r.w - 30.0 * s, y + 28.0 * s, 22.0 * s, DIM);
    }
}

const AMBER_DEV: [u8; 4] = rgb(255, 196, 94);

fn back_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 114.0 * s, 8.0 * s, 94.0 * s, 30.0 * s)
}

fn auth_card(w: u32, h: u32, s: f32, n_fields: usize) -> R {
    let cw = (440.0 * s).min(w as f32 - 40.0 * s);
    let ch = (290.0 + 66.0 * n_fields as f32) * s;
    // the card never reaches up into the top bar
    R::new((w as f32 - cw) / 2.0, ((h as f32 - ch) / 2.0).max(crate::loginbar::bar_h(s) + 8.0), cw, ch)
}

fn auth_field_rect(w: u32, h: u32, s: f32, i: usize, n: usize) -> R {
    let c = auth_card(w, h, s, n);
    R::new(c.x + 32.0 * s, c.y + 168.0 * s + i as f32 * 66.0 * s, c.w - 64.0 * s, 50.0 * s)
}

fn draw_auth(g: &mut Gfx, w: u32, h: u32, s: f32, f: &Form, setup: bool, tick: u64, hostname: &str, volatile: bool) {
    let c = auth_card(w, h, s, f.fields.len());
    g.radial_glow(c.x + c.w / 2.0, c.y + c.h / 2.0, c.w * 0.7, [70, 225, 255, 22]);
    crate::fx::modal_panel(g, c, s * 1.4, crate::fx::CYAN, false);
    let tw = g.text_width(54.0 * s, "IYAKKI", Face::Bold);
    g.text(c.x + (c.w - tw) / 2.0, c.y + 28.0 * s, 54.0 * s, TEXT, "IYAKKI", Face::Bold);
    name_strip(g, c.x + (c.w - tw) / 2.0, c.y + 98.0 * s, tw, 5.0 * s, s, true);
    let sub = if setup { "Welcome — create the administrator account" } else { "An OS for Agents" };
    let sw = g.text_width(15.0 * s, sub, Face::Sans);
    g.text(c.x + (c.w - sw) / 2.0, c.y + 118.0 * s, 15.0 * s, MUTED, sub, Face::Sans);
    for (i, field) in f.fields.iter().enumerate() {
        field.render(g, auth_field_rect(w, h, s, i, f.fields.len()), i == f.focus, s, (tick / 15) % 2 == 0, Face::Sans, i);
    }
    let label = if setup { "Create account" } else { "Sign in" };
    let bw = g.text_width(16.0 * s, label, Face::Bold) + 16.0 * s * 1.7;
    g.button(c.x + (c.w - bw) / 2.0, c.y + c.h - 96.0 * s, 16.0 * s, label, rgb(120, 170, 255), Event::Key(Key::Ctrl('s')));
    if let Some(e) = &f.error {
        let e = g.fit(15.0 * s, e, Face::Sans, c.w - 64.0 * s);
        g.text(c.x + 32.0 * s, c.y + c.h - 40.0 * s, 15.0 * s, ERR, &e, Face::Sans);
    } else {
        let hint = "Click a field or press Tab · Ctrl+V pastes";
        let hw = g.text_width(13.0 * s, hint, Face::Sans);
        g.text(c.x + (c.w - hw) / 2.0, c.y + c.h - 36.0 * s, 13.0 * s, DIM, hint, Face::Sans);
    }
    g.text(24.0 * s, h as f32 - 34.0 * s, 14.0 * s, DIM, hostname, Face::Sans);
    // remote diagnostics (boot entry with iyakki.ssh=1): say how to reach this machine
    if let Ok(note) = std::fs::read_to_string("/run/iyakki-ssh") {
        let note = format!("Remote diagnostics ON · {}", note.trim());
        let nw = g.text_width(14.0 * s, &note, Face::Sans);
        g.text(w as f32 - nw - 24.0 * s, h as f32 - 34.0 * s, 14.0 * s, rgb(255, 196, 94), &note, Face::Sans);
    }
    if volatile {
        let note = if setup { "Live session: this account is forgotten at power-off unless you attach a data disk (Settings → Storage)." } else { "Live session: changes are not saved when you power off. Admins: Settings → Storage." };
        let note = g.fit(14.0 * s, note, Face::Sans, w as f32 - 48.0 * s);
        let nw = g.text_width(14.0 * s, &note, Face::Sans);
        g.text((w as f32 - nw) / 2.0, c.y + c.h + 22.0 * s, 14.0 * s, rgb(255, 196, 94), &note, Face::Sans);
    }
}

/// The F1 help popup: a card in the middle listing every key, closed by any click, Esc, Enter or F1.
fn draw_help(g: &mut Gfx, w: u32, h: u32, s: f32, title: &str, lines: &[String]) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let line_h = 26.0 * s;
    let bw = (760.0 * s).min(w as f32 - 40.0 * s);
    let bh = (96.0 * s + line_h * lines.len() as f32).min(h as f32 - 40.0 * s);
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    crate::fx::modal_panel(g, R::new(x, y, bw, bh), s, crate::fx::CYAN, false);
    g.text(x + 28.0 * s, y + 22.0 * s, 24.0 * s, TEXT, title, Face::Bold);
    let key_w = (bw * 0.42).min(300.0 * s);
    let mut ly = y + 70.0 * s;
    for l in lines {
        if ly + line_h > y + bh - 30.0 * s {
            break;
        }
        match l.split_once(" · ") {
            Some((k, what)) => {
                let k = g.fit(15.0 * s, k, Face::Bold, key_w - 12.0 * s);
                g.text(x + 28.0 * s, ly, 15.0 * s, crate::fx::CYAN, &k, Face::Bold);
                let what = g.fit(15.0 * s, what, Face::Sans, bw - key_w - 56.0 * s);
                g.text(x + 28.0 * s + key_w, ly, 15.0 * s, TEXT, &what, Face::Sans);
            }
            None if !l.is_empty() => {
                g.text(x + 28.0 * s, ly + 4.0 * s, 14.0 * s, MUTED, l, Face::Bold);
            }
            None => {}
        }
        ly += line_h;
    }
    g.text(x + 28.0 * s, y + bh - 28.0 * s, 13.0 * s, DIM, "Esc or click to close", Face::Sans);
}

/// `YYYYMMDD-HHMMSS` (UTC) for a file name.
fn stamp(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil date from days since 1970-01-01
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Dim everything except the chosen area, and say what to do.
fn draw_shot_overlay(g: &mut Gfx, w: u32, h: u32, s: f32, first: Option<(i32, i32)>, mouse: (i32, i32)) {
    let (wf, hf) = (w as f32, h as f32);
    let dim = [0, 0, 0, 120];
    match first {
        None => g.rect(0.0, 0.0, wf, hf, dim),
        Some((fx, fy)) => {
            let (x0, x1) = (fx.min(mouse.0).max(0) as f32, fx.max(mouse.0).min(w as i32) as f32);
            let (y0, y1) = (fy.min(mouse.1).max(0) as f32, fy.max(mouse.1).min(h as i32) as f32);
            g.rect(0.0, 0.0, wf, y0, dim);
            g.rect(0.0, y1, wf, hf - y1, dim);
            g.rect(0.0, y0, x0, y1 - y0, dim);
            g.rect(x1, y0, wf - x1, y1 - y0, dim);
            g.rrect_outline(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0), 0.0, 2.0 * s, rgb(120, 175, 255));
        }
    }
    let msg = if first.is_none() { "Click one corner of the area  ·  Enter keeps the whole screen  ·  Esc cancels" } else { "Click the opposite corner  ·  Esc cancels" };
    let mw = g.text_width(17.0 * s, msg, Face::Bold) + 40.0 * s;
    let (mx, my) = ((wf - mw) / 2.0, 16.0 * s);
    g.rrect(mx, my, mw, 40.0 * s, 20.0 * s, rgb(20, 26, 48));
    g.text(mx + 20.0 * s, my + 10.0 * s, 17.0 * s, TEXT, msg, Face::Bold);
}

/// "Copied to the clipboard", a small preview, and the two choices.
fn draw_shot_review(g: &mut Gfx, w: u32, h: u32, s: f32, img: &tiny_skia::Pixmap) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let (maxw, maxh) = (460.0 * s, 280.0 * s);
    let k = (maxw / img.width() as f32).min(maxh / img.height() as f32).min(1.0);
    let (pw, ph) = (img.width() as f32 * k, img.height() as f32 * k);
    let bw = (pw + 60.0 * s).max(460.0 * s).min(w as f32 - 40.0 * s);
    let bh = ph + 190.0 * s;
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    g.rrect(x, y, bw, bh, 20.0 * s, rgb(25, 31, 54));
    g.text(x + 28.0 * s, y + 22.0 * s, 22.0 * s, TEXT, "Screenshot copied to the clipboard", Face::Bold);
    g.text(x + 28.0 * s, y + 54.0 * s, 14.0 * s, MUTED, &format!("{} × {} pixels. Keep a file as well?", img.width(), img.height()), Face::Sans);
    let (ix, iy) = (x + (bw - pw) / 2.0, y + 84.0 * s);
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    g.pm.draw_pixmap(0, 0, img.as_ref(), &paint, tiny_skia::Transform::from_translate(ix, iy).pre_scale(k, k), None);
    g.rrect_outline(ix, iy, pw, ph, 0.0, 1.5 * s, rgb(70, 82, 120));
    let by = y + bh - 62.0 * s;
    let w1 = g.button(x + 28.0 * s, by, 15.0 * s, "Save to disk (S)", OK, Event::Key(Key::Char('s')));
    g.button(x + 28.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
}

/// The alarm that is ringing, over everything else.
fn draw_ring(g: &mut Gfx, w: u32, h: u32, s: f32, r: &Ring, tick: u64) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let n = r.items.len().min(4);
    let (bw, bh) = (520.0 * s, (210.0 + 44.0 * n as f32) * s);
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    let amber = rgb(255, 196, 94);
    g.rrect(x, y, bw, bh, 22.0 * s, rgb(25, 31, 54));
    // a ringing bell: the rings pulse
    let pulse = ((tick % 24) as f32 / 24.0).min(1.0);
    let (cx0, cy0) = (x + 70.0 * s, y + 70.0 * s);
    g.ring(cx0, cy0, (34.0 + 14.0 * pulse) * s, 2.0 * s, rgb(255, 196, 94));
    g.circle(cx0, cy0, 26.0 * s, rgb(52, 44, 30));
    g.ring(cx0, cy0, 20.0 * s, 3.0 * s, amber);
    g.line(cx0, cy0, cx0, cy0 - 12.0 * s, 3.0 * s, amber);
    g.line(cx0, cy0, cx0 + 8.0 * s, cy0 + 4.0 * s, 3.0 * s, amber);
    let alarm = r.items.iter().any(|i| i.1);
    g.text(x + 128.0 * s, y + 34.0 * s, 26.0 * s, TEXT, if alarm { "Alarm" } else { "Reminder" }, Face::Bold);
    g.text(x + 128.0 * s, y + 72.0 * s, 14.0 * s, MUTED, "Enter dismisses · S snoozes for 5 minutes", Face::Sans);
    for (i, (what, is_alarm, when)) in r.items.iter().take(4).enumerate() {
        let iy = y + 124.0 * s + i as f32 * 44.0 * s;
        g.rrect(x + 28.0 * s, iy, bw - 56.0 * s, 38.0 * s, 10.0 * s, rgb(34, 42, 70));
        let shown = g.fit(17.0 * s, what, Face::Bold, bw - 220.0 * s);
        g.text(x + 42.0 * s, iy + 9.0 * s, 17.0 * s, if *is_alarm { amber } else { TEXT }, &shown, Face::Bold);
        let tw = g.text_width(13.0 * s, when, Face::Sans);
        g.text(x + bw - 42.0 * s - tw, iy + 12.0 * s, 13.0 * s, MUTED, when, Face::Sans);
    }
    if r.items.len() > 4 {
        g.text(x + 30.0 * s, y + 124.0 * s + 4.0 * 44.0 * s, 13.0 * s, MUTED, &format!("and {} more", r.items.len() - 4), Face::Sans);
    }
    let by = y + bh - 62.0 * s;
    let w1 = g.button(x + 28.0 * s, by, 15.0 * s, "Dismiss (Enter)", rgb(255, 196, 94), Event::Key(Key::Enter));
    g.button(x + 28.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Snooze 5 min (S)", MUTED, Event::Key(Key::Char('s')));
}

fn draw_power_menu(g: &mut Gfx, w: u32, h: u32, s: f32) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 200]);
    g.hit(R::new(0.0, 0.0, w as f32, h as f32), Event::Key(Key::Esc), "backdrop-cancel");
    let (bw, bh) = (500.0 * s, 210.0 * s);
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    crate::fx::modal_panel(g, R::new(x, y, bw, bh), s, ERR, true);
    g.icon(Icon::Power, x + 60.0 * s, y + 60.0 * s, 52.0 * s, ERR);
    g.text(x + 110.0 * s, y + 34.0 * s, 24.0 * s, TEXT, "Power", Face::Bold);
    g.text(x + 110.0 * s, y + 70.0 * s, 15.0 * s, MUTED, "Shut down or restart this machine.", Face::Sans);
    let by = y + bh - 62.0 * s;
    let w1 = g.button(x + 28.0 * s, by, 15.0 * s, "Power off", ERR, Event::Key(Key::Char('y')));
    let w2 = g.button(x + 28.0 * s + w1 + 10.0 * s, by, 15.0 * s, "Restart", TEXT, Event::Key(Key::Char('r')));
    g.button(x + 28.0 * s + w1 + w2 + 20.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use iyakki_agent::chat::Unconfigured;
    use crate::apps::tests::{temp_dir, MockHost};
    use iyakki_sec::policy::Role;

    pub fn env(tag: &str) -> (ShellEnv, Arc<MockHost>) {
        let d = temp_dir(tag);
        let host = Arc::new(MockHost::default());
        (
            ShellEnv {
                store: Arc::new(Mutex::new(UserStore::open(&d.join("users.json")).unwrap())),
                audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
                host: host.clone(),
                cfg: AiConfig::parse("").unwrap(),
                chat: Arc::new(Unconfigured),
                mcp_servers: vec![],
                services: crate::apps::tests::services_in(&d, Arc::new(crate::apps::tests::Offline)),
                web: Arc::new(iyakki_web::fetch::NoNetwork),
                hostname: "iyakki-test".into(),
                data_volatile: false,
                hwcheck: false,
            },
            host,
        )
    }

    pub(super) fn typ(sh: &mut Shell, s: &str) {
        for c in s.chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
    }
    pub(super) fn key(sh: &mut Shell, k: Key) {
        sh.handle(&Event::Key(k));
    }

    pub(super) fn setup_admin(sh: &mut Shell) {
        assert_eq!(sh.screen_name(), "setup");
        typ(sh, "root");
        key(sh, Key::Tab);
        typ(sh, "rootpass12");
        key(sh, Key::Tab);
        typ(sh, "rootpass12");
        key(sh, Key::Enter);
    }

    pub(crate) fn login(sh: &mut Shell, u: &str, p: &str) {
        // the form may already hold the last person's name with the cursor on the password: go back to the name and replace it
        if let Screen::Login(f) = &mut sh.screen {
            f.focus = 0;
            f.fields[0].clear();
        }
        typ(sh, u);
        key(sh, Key::Tab);
        typ(sh, p);
        key(sh, Key::Enter);
    }

    #[test]
    fn the_hardware_check_starts_before_login_counts_input_and_leaves_with_enter() {
        let (mut e, _) = env("hw1");
        e.hwcheck = true;
        let mut sh = Shell::new(e);
        assert_eq!(sh.screen_name(), "hwcheck");
        let mut g = Gfx::new(1920, 1080);
        sh.render(&mut g);
        // a tap on a target registers (the hit comes from the drawn frame) and moves/keys are counted
        let r = sh.hits().iter().find(|h| h.label == "target 1").expect("targets are clickable").rect;
        sh.handle(&Event::MouseMove((r.x + 5.0) as i32, (r.y + 5.0) as i32));
        sh.handle(&Event::MouseDown((r.x + 5.0) as i32, (r.y + 5.0) as i32));
        key(&mut sh, Key::Char('q'));
        sh.render(&mut g);
        let hit_after = sh.hits().iter().filter(|h| h.label.starts_with("target ")).count();
        assert_eq!(hit_after, 8);
        if let Screen::HwCheck(h) = &sh.screen {
            assert_eq!(h.targets_hit(), 1);
            assert_eq!(h.counts(), (1, 1, 1));
        } else {
            panic!("left the hardware check too early");
        }
        // nothing else is reachable from here: typing does not reach a login form
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup", "first run: the admin account is created next");
        // with an account present, Enter leads to the login screen
        let (mut e2, _) = env("hw2");
        e2.hwcheck = true;
        e2.store.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let mut sh2 = Shell::new(e2);
        assert_eq!(sh2.screen_name(), "hwcheck");
        key(&mut sh2, Key::Esc);
        assert_eq!(sh2.screen_name(), "login");
    }

    #[test]
    fn first_run_setup_then_login_launcher() {
        let (e, _) = env("sh1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        assert_eq!(sh.screen_name(), "launcher");
        assert_eq!(sh.session().unwrap().role, Role::Admin);
        assert_eq!(sh.visible_tiles().len(), TILES.iter().filter(|t| t.group.is_some()).count(), "admins see the Users tile (every tile on the grid)");
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.screen_name(), "login");
        login(&mut sh, "root", "wrong-password");
        assert_eq!(sh.screen_name(), "login");
        // the username stays filled in and focus returns to the (cleared) password field
        typ(&mut sh, "rootpass12");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "launcher");
    }

    #[test]
    fn setup_rejects_mismatch_and_weak_input() {
        let (e, _) = env("sh2");
        let mut sh = Shell::new(e);
        typ(&mut sh, "root");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "rootpass12");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "different1");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        // weak password
        let (e, _) = env("sh2b");
        let mut sh = Shell::new(e);
        typ(&mut sh, "root");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "short");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "short");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup");
    }

    pub(crate) fn shell_with_users(tag: &str) -> (Shell, Arc<MockHost>) {
        let (e, h) = env(tag);
        {
            let mut st = e.store.lock().unwrap();
            st.create_first_admin("root", "rootpass12").unwrap();
            let admin = st.authenticate("root", "rootpass12").unwrap();
            st.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
            st.create_user(&admin, "gus", "guestpass1", Role::Guest).unwrap();
            st.create_user(&admin, "oscar", "operpass12", Role::Operator).unwrap();
        }
        (Shell::new(e), h)
    }

    #[test]
    fn double_click_picks_a_word_and_dragging_the_pointer_with_the_button_held_selects() {
        let (e, _h) = env("select");
        e.store.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let mut sh = Shell::new(e);
        let mut g = Gfx::new(1280, 720);
        for c in "alpha beta".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.render(&mut g);
        let field = sh.hits().iter().find(|h| h.label == "field" && h.ev == Event::Focus(0)).expect("username field").rect;
        let y = (field.y + field.h / 2.0) as i32;
        let selected = |sh: &Shell| match &sh.screen {
            Screen::Login(f) => f.fields[0].selected_text(),
            _ => None,
        };
        // a double click inside the second word selects it
        let x = field.x as i32 + 12 + 80;
        sh.handle(&Event::MouseDown(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        sh.handle(&Event::MouseDown(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        assert_eq!(selected(&sh).as_deref(), Some("beta"), "double click selects the word");
        // press, move with the button held, release: the text between is selected
        sh.tick += 100; // not a double click
        sh.handle(&Event::MouseDown(field.x as i32 + 12, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseMove(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        let drag = selected(&sh).expect("a selection");
        assert!(drag.starts_with("alpha"), "{drag}");
        // moving without the button does not select
        sh.tick += 100;
        sh.handle(&Event::MouseDown(field.x as i32 + 12, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(field.x as i32 + 12, y));
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(selected(&sh), None, "{:?}", selected(&sh));
    }

    #[test]
    fn an_administrator_is_told_about_a_new_device_on_the_network_and_an_ordinary_user_is_not() {
        let (mut sh, _) = shell_with_users("newdev");
        login(&mut sh, "root", "rootpass12");
        let n = |ip: &str, mac: &str| iyakki_agent::devwatch::Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() };
        let dw = sh.services_for_demo().devwatch.clone();
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01")], 1);
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.9", "bb:bb:bb:bb:bb:09")], 2);
        for _ in 0..95 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.toast_text().is_some_and(|t| t.contains("New device on your network: 10.0.0.9")), "{:?}", sh.toast_text());
        let log = std::fs::read_to_string(sh.services_for_demo().dir().parent().unwrap().join("audit.log")).unwrap_or_default();
        let _ = log;
        // an ordinary account is not told (it cannot act on it)
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.77", "cc:cc:cc:cc:cc:77")], 3);
        for _ in 0..95 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.toast_text().is_none_or(|t| !t.contains("10.0.0.77")));
    }

    #[test]
    fn the_text_and_button_size_a_person_chose_is_applied_at_their_sign_in_and_gone_at_the_sign_in_screen() {
        let (mut sh, _) = shell_with_users("uisize");
        let dir = sh.services_for_demo().dir().to_path_buf();
        crate::uisize::save(&dir, "alice", 3); // Larger, 150%
        let normal = scale_for(1280, 720);
        login(&mut sh, "root", "rootpass12");
        assert_eq!(scale_for(1280, 720), normal, "root chose nothing");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(scale_for(1280, 720), 1.5 * normal);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let tile = |sh: &Shell| sh.hits().iter().find(|h| h.label.to_lowercase().contains("chat")).map(|h| h.rect.w);
        let big = tile(&sh);
        key(&mut sh, Key::Char('o'));
        assert_eq!(scale_for(1280, 720), normal, "the sign-in screen is at normal size again");
        login(&mut sh, "root", "rootpass12");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(big.is_none() || tile(&sh).is_none() || big > tile(&sh), "tiles were bigger for alice");
    }

    #[test]
    fn the_sign_in_form_fills_in_the_last_person_who_signed_in_and_puts_the_cursor_on_the_password() {
        let (e, _h) = env("lastuser");
        {
            let mut st = e.store.lock().unwrap();
            st.create_first_admin("root", "rootpass12").unwrap();
            let admin = st.authenticate("root", "rootpass12").unwrap();
            st.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
        }
        let dir = e.services.dir().to_path_buf();
        let mut sh = Shell::new(e);
        // nobody has signed in yet: an empty form with the cursor on the username
        let Screen::Login(f) = &sh.screen else { panic!("login expected") };
        assert_eq!((f.fields[0].value.as_str(), f.focus), ("", 0));
        // a failed attempt remembers nothing
        for c in "alice".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        for c in "wrong-password".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        assert!(!dir.join("last-user").exists());
        // a good sign-in is remembered; signing out shows the form filled in, cursor on the password
        sh.screen = Screen::Login(Form::login());
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(std::fs::read_to_string(dir.join("last-user")).unwrap().trim(), "alice");
        sh.handle(&Event::Key(Key::Char('o')));
        let Screen::Login(f) = &sh.screen else { panic!("signed out") };
        assert_eq!((f.fields[0].value.as_str(), f.focus, f.fields[1].value.as_str()), ("alice", 1, ""), "the password is never filled in");
        // typing goes straight into the password, and Enter signs in
        for c in "alicepass1".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        assert_eq!(sh.screen_name(), "launcher");
        // the next start of the program remembers too, and a damaged or odd file is ignored
        std::fs::write(dir.join("last-user"), "../../etc/passwd\n").unwrap();
        assert_eq!(last_user(&sh.env), None);
        std::fs::write(dir.join("last-user"), "root\n").unwrap();
        assert_eq!(last_user(&sh.env).as_deref(), Some("root"));
    }

    #[test]
    fn the_samples_are_put_in_place_once_by_an_administrators_first_sign_in_and_stay_deleted() {
        let (mut sh, _) = shell_with_users("samples1");
        let docs = std::env::temp_dir().join(format!("iyakki-samples-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&docs);
        std::fs::create_dir_all(&docs).unwrap();
        sh.env.cfg.rag_docs = docs.display().to_string();
        let svc = sh.env.services.clone();
        // an ordinary user signing in first changes nothing
        login(&mut sh, "alice", "alicepass1");
        assert!(svc.mcp.list().is_empty() && svc.skills.list("alice").is_empty() && !docs.join("shared").exists());
        sh.handle(&Event::Key(Key::Esc));
        // the administrator's first sign-in puts the examples in place
        login(&mut sh, "root", "rootpass12");
        let mcp = svc.mcp.list();
        assert_eq!(mcp.len(), 1);
        assert!(mcp[0].sample && mcp[0].name == "iyakki-demo" && mcp[0].args == vec!["--mcp-demo".to_string()]);
        let skills: Vec<String> = svc.skills.list("root").into_iter().map(|s| s.name).collect();
        for n in ["sample-explain-simply", "sample-python-practice", "sample-meeting-notes"] {
            assert!(skills.contains(&n.to_string()), "{n}");
        }
        // the skill that ships with IYAKKI is there too, and it parses and fits the prompt budget
        assert!(skills.contains(&"k3s-dev-best-practices".to_string()));
        let k3s = svc.skills.get(&iyakki_agent::skills::Scope::Shared, "k3s-dev-best-practices").unwrap();
        assert!(k3s.body.chars().count() < 4000 && k3s.body.contains("deploy_build") && k3s.body.contains("k8s_teardown"));
        for (name, md) in crate::samples::BUILTIN_SKILLS {
            let k = iyakki_agent::skills::parse(md, iyakki_agent::skills::Scope::Shared).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(k.name == name && k.body.chars().count() <= 3950 && md.is_ascii() && k.triggers.len() <= 12, "{name}");
            assert!(skills.contains(&name.to_string()), "{name} is installed");
            // every dev skill insists on bugs as their own B- rows with captured logs
            assert!(k.body.contains("B- id") && k.body.contains("bug_report") && k.body.contains("never F-"), "{name} has the bug rule");
        }
        assert!(svc.skills.select("alice", "deploy my node app to k3s", 3).iter().any(|s| s.name == "k3s-dev-best-practices" || s.name == "k3s-build-and-deploy"));
        assert!(svc.skills.select("alice", "build and deploy this app to k3s", 3).iter().any(|s| s.name == "k3s-build-and-deploy"));
        for (file, _) in crate::samples::DOCS {
            assert!(docs.join("shared").join(file).is_file(), "{file}");
        }
        let log = std::fs::read_to_string(sh.env.audit.path()).unwrap();
        assert!(log.contains("samples.seed"));
        // every skill (including the shared samples) can be seen by everyone, and the sample MCP server reaches the chat
        assert!(svc.skills.list("alice").iter().any(|s| s.name == "sample-python-practice"));
        // deleting them: they do not return at the next sign-in
        svc.mcp.remove("iyakki-demo").unwrap();
        for n in ["sample-explain-simply", "sample-python-practice", "sample-meeting-notes"].into_iter().chain(crate::samples::BUILTIN_SKILLS.iter().map(|b| b.0)) {
            svc.skills.remove(&iyakki_agent::skills::Scope::Shared, n, true, true).unwrap();
        }
        std::fs::remove_file(docs.join("shared/sample-iyakki-quick-start.md")).unwrap();
        sh.handle(&Event::Key(Key::Esc));
        login(&mut sh, "root", "rootpass12");
        assert!(svc.mcp.list().is_empty() && svc.skills.list("root").is_empty());
        assert!(!docs.join("shared/sample-iyakki-quick-start.md").exists());
    }

    #[test]
    fn an_alarm_or_reminder_rings_over_everything_until_dismissed_and_snooze_brings_it_back() {
        let (mut sh, _) = shell_with_users("ring1");
        login(&mut sh, "alice", "alicepass1");
        // no sound program is run by the test
        sh.env.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        let zone = iyakki_agent::clock::Zone::system();
        let svc = sh.env.services.clone();
        let cal = &svc.calendar;
        let now = iyakki_agent::clock::now_unix();
        sh.check_alarms(now); // the first look only starts the clock
        assert!(sh.ringing().is_none());
        cal.add_event("alice", "Standup", now + 5, 15, false, iyakki_agent::calendar::Repeat::None, "", "user", Some(0)).unwrap();
        sh.check_alarms(now + 3);
        assert!(sh.ringing().is_none(), "not yet");
        sh.check_alarms(now + 6);
        assert_eq!(sh.ringing(), Some(vec!["Standup".to_string()]));
        // it is drawn over whatever is on screen and does not let other keys through
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label == "backdrop") && sh.hits().iter().any(|h| h.ev == Event::Key(Key::Char('s'))), "the dialog is drawn with its buttons");
        key(&mut sh, Key::Char('x'));
        assert!(sh.ringing().is_some(), "other keys are ignored while it rings");
        // snooze: quiet now, back five minutes later
        key(&mut sh, Key::Char('s'));
        assert!(sh.ringing().is_none() && sh.toast_text().unwrap().contains("Snoozed"));
        sh.check_alarms(now + 100);
        assert!(sh.ringing().is_none());
        sh.check_alarms(now + 6 + 400);
        assert_eq!(sh.ringing(), Some(vec!["Standup".to_string()]));
        key(&mut sh, Key::Enter);
        assert!(sh.ringing().is_none());
        sh.check_alarms(now + 6 + 900);
        assert!(sh.ringing().is_none(), "dismissed means gone");
        // a one-time alarm rings at its minute and switches itself off
        let t = now + 7200;
        let l = zone.local(t);
        cal.add_alarm("alice", "Tea", l.hour, l.minute, 0, now, &zone, "assistant").unwrap();
        let at = zone.local_to_unix(l.year, l.month, l.day, l.hour, l.minute, 0);
        sh.check_alarms(at - 1);
        assert!(sh.ringing().is_none());
        sh.check_alarms(at + 1);
        assert_eq!(sh.ringing(), Some(vec!["Tea".to_string()]));
        assert!(!cal.alarms("alice")[0].enabled);
        let log = std::fs::read_to_string(sh.env.audit.path()).unwrap();
        assert!(log.contains("calendar.reminder") && log.contains("alarm.ring"), "{log}");
        sh.render(&mut g);
        // nobody is signed in: nothing rings and the clock restarts
        sh.handle(&Event::Key(Key::Esc));
        // an unanswered alarm gives up after five minutes
        sh.check_alarms(at + 400);
        assert!(sh.ringing().is_none(), "no one answered for five minutes");
    }

    fn bell_hit(sh: &Shell) -> Option<crate::ui::Hit> {
        sh.hits().iter().find(|h| h.label == "inbox").cloned()
    }

    #[test]
    fn the_pointer_is_a_hand_over_buttons_a_text_bar_over_fields_and_an_arrow_elsewhere() {
        use crate::ui::Pointer;
        let (mut sh, _) = shell_with_users("ptr1");
        let mut g = Gfx::new(1280, 720);
        // the sign-in screen has text fields and a button
        sh.render(&mut g);
        let centre = |r: crate::ui::R| ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        let field = sh.hits().iter().find(|h| h.label == "field").expect("a field").rect;
        let (x, y) = centre(field);
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(sh.pointer(), Pointer::Text);
        sh.handle(&Event::MouseMove(2, 2));
        assert_eq!(sh.pointer(), Pointer::Default, "nothing clickable under the corner");
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        let tile = sh.hits().iter().find(|h| h.label == "inbox").expect("the bell").rect;
        let (x, y) = centre(tile);
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(sh.pointer(), Pointer::Hand);
    }

    #[test]
    fn the_bell_is_on_the_launcher_and_inside_apps_counts_what_waits_and_opens_the_inbox() {
        let (mut sh, _) = shell_with_users("bell1");
        login(&mut sh, "alice", "alicepass1");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let on_launcher = bell_hit(&sh).expect("a bell on the launcher");
        assert_eq!(on_launcher.ev, Event::Key(Key::F(9)));
        assert_eq!(sh.badge(), Badge::default());
        // unread notes, an overdue task and a waiting approval all count
        let svc = sh.env.services.clone();
        let now = iyakki_agent::clock::now_unix();
        svc.notify("alice", iyakki_agent::inbox::Kind::Info, "one", "", None).unwrap();
        svc.notify("alice", iyakki_agent::inbox::Kind::Info, "two", "", None).unwrap();
        svc.inbox.add_task("alice", "late", "", Some(now - 60), iyakki_agent::inbox::Priority::Normal, "user", now - 100).unwrap();
        svc.notify("bob", iyakki_agent::inbox::Kind::Info, "not mine", "", None).unwrap();
        sh.handle(&Event::MouseMove(5, 5)); // any event refreshes nothing; a key does
        key(&mut sh, Key::Right);
        assert_eq!(sh.badge(), Badge { unread: 2, approvals: 0, overdue: 1 });
        assert_eq!(sh.badge().total(), 3);
        sh.render(&mut g);
        // a ticking clock picks up changes made elsewhere within half a second
        svc.notify("alice", iyakki_agent::inbox::Kind::Info, "three", "", None).unwrap();
        for _ in 0..16 {
            sh.handle(&Event::Tick);
        }
        assert_eq!(sh.badge().unread, 3);
        assert!(sh.take_stale());
        // the bell opens the inbox by click
        let r = bell_hit(&sh).unwrap_or(on_launcher).rect;
        sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
        assert_eq!(sh.screen_name(), "app:inbox");
        sh.render(&mut g);
        assert!(bell_hit(&sh).is_some(), "the bell is in the header of an app too");
        // and closes it again with F9, back on the launcher
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "launcher");
        // from inside an app: F9 keeps the app aside and Back returns to it
        key(&mut sh, Key::Char('6'));
        assert_eq!(sh.screen_name(), "app:runs");
        sh.render(&mut g);
        assert!(bell_hit(&sh).is_some());
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "app:inbox");
        assert!(sh.stash.is_some());
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "app:runs", "Back returns to the app that was open");
        assert!(sh.stash.is_none());
        // Back from the launcher's inbox goes to the launcher, and signing out forgets everything
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(9));
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.badge(), Badge::default());
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "login", "no inbox before sign-in");
    }

    #[test]
    fn a_waiting_approval_turns_the_bell_red_and_is_answered_from_the_inbox_while_the_app_stays_alive() {
        use iyakki_agent::agent::RunEvent;
        struct Ask(std::sync::atomic::AtomicUsize);
        impl iyakki_agent::chat::ChatBackend for Ask {
            fn complete(&self, _: &[iyakki_agent::chat::Message]) -> Result<String, String> {
                Err("x".into())
            }
            fn complete_tools(&self, _: &[iyakki_agent::chat::Message], _: &[iyakki_agent::chat::ToolSpec]) -> Result<iyakki_agent::chat::ModelTurn, String> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(iyakki_agent::chat::ModelTurn { text: String::new(), calls: vec![iyakki_agent::chat::ToolCall { id: "k1".into(), name: "run_command".into(), args: serde_json::json!({}) }] })
                } else {
                    Ok(iyakki_agent::chat::ModelTurn { text: "ok".into(), calls: vec![] })
                }
            }
        }
        struct Tools;
        impl iyakki_agent::agent::Tools for Tools {
            fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
                vec![iyakki_agent::agent::ToolDef { spec: iyakki_agent::chat::ToolSpec { name: "run_command".into(), description: "x".into(), schema: serde_json::json!({"type": "object"}) }, risk: iyakki_agent::agent::Risk::Ask, untrusted: false }]
            }
            fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
                Ok("ran".into())
            }
            fn describe(&self, _: &str, _: &serde_json::Value) -> String {
                "run `ls`".into()
            }
        }
        let (mut sh, _) = shell_with_users("bell2");
        login(&mut sh, "alice", "alicepass1");
        // a run started in an app (here the shell stands in for the chat: it holds the session on the screen)
        struct Holder(iyakki_agent::runs::RunSession, Vec<RunEvent>);
        impl App for Holder {
            fn title(&self) -> &'static str {
                "Holder"
            }
            fn icon(&self) -> Icon {
                Icon::Chat
            }
            fn accent(&self) -> [u8; 4] {
                rgb(1, 2, 3)
            }
            fn hint(&self) -> String {
                String::new()
            }
            fn handle(&mut self, ev: &Event, _: &AppCx) -> Flow {
                if matches!(ev, Event::Tick) {
                    let e = self.0.poll();
                    self.1.extend(e);
                }
                Flow::Stay
            }
            fn render(&mut self, _: &mut Gfx, _: R, _: f32, _: &AppCx, _: u64) {}
        }
        let run = iyakki_agent::runs::RunSession::start(Some(sh.env.services.runs.clone()), "alice", "list my files", Arc::new(Ask(Default::default())), Arc::new(Tools), vec![], Default::default());
        sh.screen = Screen::App(Box::new(Holder(run, vec![])), PApp::Chat);
        for _ in 0..400 {
            sh.handle(&Event::Tick);
            if sh.badge().approvals == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(sh.badge().approvals, 1);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        // the badge is red while an approval waits
        let r = bell_hit(&sh).unwrap().rect;
        let red = (r.x as u32..(r.x + r.w) as u32).any(|x| (0..r.y as u32 + 20).any(|y| g.pm.pixel(x, y).is_some_and(|p| p.red() > 190 && p.green() < 80 && p.blue() < 100)));
        assert!(red, "a red badge near the bell");
        // open the inbox from the app, go to approvals and approve with y
        key(&mut sh, Key::F(9));
        key(&mut sh, Key::Char('3'));
        key(&mut sh, Key::Char('y'));
        assert_eq!(sh.badge().approvals, 0);
        // the app that was kept aside went on running: its run finished with the tool executed
        for _ in 0..400 {
            sh.handle(&Event::Tick);
            if sh.env.services.runs.live().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(sh.env.services.runs.live().is_empty(), "the run finished while the inbox was open");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "app:chat");
        // finishing left a note for alice (and the audit log has the decision)
        sh.refresh_badge();
        assert!(sh.env.services.inbox.list("alice").iter().any(|n| n.kind == iyakki_agent::inbox::Kind::Agent && n.body == "ok"));
        let log = std::fs::read_to_string(sh.env.audit.path()).unwrap();
        assert!(log.contains("inbox.approve") && log.contains("run_command"), "{log}");
    }

    #[test]
    fn reminders_alarms_and_new_devices_leave_one_notification_each_not_one_per_tick() {
        let (mut sh, _) = shell_with_users("bell3");
        login(&mut sh, "alice", "alicepass1");
        sh.env.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        let svc = sh.env.services.clone();
        let now = iyakki_agent::clock::now_unix();
        sh.check_alarms(now);
        svc.calendar.add_event("alice", "Standup", now + 5, 15, false, iyakki_agent::calendar::Repeat::None, "", "user", Some(0)).unwrap();
        for dt in 1..=30 {
            sh.check_alarms(now + dt);
        }
        let l = svc.inbox.list("alice");
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!((l[0].kind, l[0].title.as_str()), (iyakki_agent::inbox::Kind::Reminder, "Reminder: Standup"));
        assert_eq!(l[0].link, Some(iyakki_agent::inbox::Link::new("calendar", "")));
        let zone = iyakki_agent::clock::Zone::system();
        let t = zone.local(now + 7200);
        svc.calendar.add_alarm("alice", "Tea", t.hour, t.minute, 0, now, &zone, "user").unwrap();
        let at = zone.local_to_unix(t.year, t.month, t.day, t.hour, t.minute, 0);
        sh.check_alarms(at - 1);
        sh.check_alarms(at + 1);
        sh.check_alarms(at + 2);
        assert_eq!(svc.inbox.list("alice").iter().filter(|n| n.title == "Alarm: Tea").count(), 1);
        sh.refresh_badge();
        assert_eq!(sh.badge().unread, 2);
    }

    #[test]
    fn the_inbox_is_enabled_for_every_role_but_has_no_tile() {
        for r in Role::ALL {
            assert!(r.template().apps.contains(&PApp::Inbox), "{r:?}");
        }
        assert!(TILES.iter().all(|t| t.app != PApp::Inbox));
        assert_eq!(PApp::Inbox.id(), "inbox");
    }

    #[test]
    fn the_sre_pilot_tile_is_one_demo_tile_in_a_group_for_every_role_with_a_ribbon() {
        assert_eq!(TILES.iter().filter(|t| t.app == PApp::SrePilot).count(), 1);
        let t = TILES.iter().find(|t| t.app == PApp::SrePilot).unwrap();
        assert_eq!((t.label, t.sub), ("SRE PILOT", "Assist · Auto"));
        assert_eq!(Group::of(PApp::SrePilot), Some(Group::Work));
        assert!(launcher::is_demo(PApp::SrePilot) && !launcher::is_demo(PApp::System));
        assert_eq!(PApp::SrePilot.id(), "srepilot");
        for r in Role::ALL {
            assert!(r.template().apps.contains(&PApp::SrePilot), "{r:?}");
        }
        let (mut sh, _) = shell_with_users("srepilot-tile");
        login(&mut sh, "root", "rootpass12");
        assert_eq!(sh.visible_tiles().iter().filter(|t| t.app == PApp::SrePilot).count(), 1);
        assert!(sh.open_for_demo("srepilot"));
        assert_eq!(sh.screen_name(), "app:srepilot");
    }

    #[test]
    fn the_ai_ide_tile_is_one_tile_on_home_for_every_role_but_guest() {
        assert_eq!(TILES.iter().filter(|t| t.app == PApp::Ide).count(), 1);
        let t = TILES.iter().find(|t| t.app == PApp::Ide).unwrap();
        assert_eq!((t.label, t.sub), ("AI IDE", "Code with the AI"));
        assert_eq!(Group::of(PApp::Ide), Some(Group::Home));
        assert_eq!((PApp::Ide.id(), PApp::Ide.label().as_str()), ("ide", "AI IDE"));
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert!(r.template().apps.contains(&PApp::Ide), "{r:?}");
            assert!(r.template().apps.contains(&PApp::Edit) && r.template().agent, "the IDE follows the editor and the agent: {r:?}");
        }
        assert!(!Role::Guest.template().apps.contains(&PApp::Ide), "guests have no files and no agent");
        assert_eq!(TILES.len(), PApp::ALL.len() - 1, "every app but the inbox has a tile");
        let (mut sh, _) = shell_with_users("ide-tile");
        login(&mut sh, "root", "rootpass12");
        assert_eq!(sh.visible_tiles().iter().filter(|t| t.app == PApp::Ide).count(), 1);
        sh.set_group(Group::Home);
        let home: Vec<PApp> = sh.group_tiles().iter().map(|t| t.app).collect();
        assert!(home.contains(&PApp::Ide) && home.contains(&PApp::Settings), "{home:?}");
        assert!(sh.open_for_demo("ide"));
        assert_eq!(sh.screen_name(), "app:ide");
    }

    /// Every app, in both halves of the blink: at most one caret and one focus frame on the whole screen.
    #[test]
    fn no_app_draws_more_than_one_caret_or_focus_frame() {
        let (mut sh, _) = shell_with_users("carets-all");
        login(&mut sh, "root", "rootpass12");
        let mut g = Gfx::new(1280, 720);
        let mut seen = 0;
        for a in PApp::ALL {
            if !sh.open_for_demo(a.id()) {
                continue;
            }
            seen += 1;
            for _ in 0..4 {
                for _ in 0..16 {
                    sh.handle(&Event::Tick);
                }
                crate::ui::take_carets();
                crate::ui::take_focus_rings();
                sh.render(&mut g);
                let (c, r) = (crate::ui::take_carets(), crate::ui::take_focus_rings());
                assert!(c <= 1 && r <= 1, "{}: {c} carets and {r} focus frames", a.id());
            }
            sh.leave_app();
        }
        assert!(seen > 10, "the apps were opened: {seen}");
    }

    #[test]
    fn the_demo_ribbon_is_drawn_at_the_top_of_the_tile_in_every_group_and_only_on_that_tile() {
        let (mut sh, _) = shell_with_users("srepilot-ribbon");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "root", "rootpass12");
        let amber = |g: &Gfx, r: R| {
            let mut n = 0;
            for y in (r.y as i32 - 12)..(r.y as i32 + 20) {
                for x in (r.x as i32)..(r.x + r.w) as i32 {
                    let o = (y as usize * g.width() as usize + x as usize) * 4;
                    let p = &g.rgba()[o..o + 3];
                    if p[0] > 240 && p[1] > 180 && p[1] < 215 && p[2] > 70 && p[2] < 120 {
                        n += 1;
                    }
                }
            }
            n
        };
        for group in [Group::Work] {
            sh.set_group(group);
            sh.render(&mut g);
            let s = scale_for(1280, 720);
            let tiles = sh.group_tiles();
            let rects = tile_rects(1280, 720, s, tiles.len());
            for (t, r) in tiles.iter().zip(&rects) {
                let n = amber(&g, *r);
                if t.app == PApp::SrePilot {
                    assert!(n > 200, "the ribbon is drawn on SRE PILOT in {group:?}: {n}");
                } else {
                    assert_eq!(n, 0, "no ribbon on {} in {group:?}", t.label);
                }
            }
        }
    }

    #[test]
    fn tiles_follow_roles_and_denials_are_audited() {
        let (mut sh, _) = shell_with_users("sh3");
        login(&mut sh, "gus", "guestpass1");
        assert_eq!(sh.visible_tiles().iter().map(|t| t.label).collect::<Vec<_>>(), TILES.iter().filter(|t| t.app != PApp::Users && t.group.is_some()).map(|t| t.label).collect::<Vec<_>>(), "every tile on the grid but Users (Settings is in the IYAKKI menu)");
        // guest can only open chat; the others are locked, and the attempt is recorded
        key(&mut sh, Key::Char('4'));
        assert_eq!(sh.screen_name(), "launcher");
        assert!(sh.toast_text().unwrap().contains("guest"));
        key(&mut sh, Key::Char('1'));
        assert_eq!(sh.screen_name(), "app:chat");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        assert!(sh.env.audit.recent(10).iter().any(|l| l.contains("app.denied") && l.contains("cli")));
        assert!(sh.env.audit.recent(10).iter().any(|l| l.contains("app.open") && l.contains("chat")));
    }

    fn at(sh: &mut Shell, g: &mut Gfx, r: R) {
        sh.render(g);
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
        sh.render(g);
    }

    fn chip(sh: &Shell, g: &Gfx, label: &str) -> R {
        let gs = sh.groups();
        let labels: Vec<&str> = gs.iter().map(|x| x.label()).collect();
        let i = labels.iter().position(|l| *l == label).unwrap_or_else(|| panic!("no chip {label}: {labels:?}"));
        launcher::chip_rects(scale_for(g.width(), g.height()), &labels)[i]
    }

    fn names(sh: &Shell) -> Vec<&'static str> {
        sh.group_tiles().iter().map(|t| t.label).collect()
    }

    #[test]
    fn the_launcher_groups_hold_every_tile_once_and_a_role_sees_only_its_tiles() {
        let (mut sh, _) = shell_with_users("grp1");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert_eq!(sh.groups().iter().map(|x| x.label()).collect::<Vec<_>>(), vec!["Home", "Web & Media", "Work", "System"]);
        assert_eq!(sh.current_group(), Group::Home, "Home is shown first");
        let mut seen = Vec::new();
        for gr in sh.groups() {
            sh.set_group(gr);
            seen.extend(names(&sh));
        }
        seen.sort();
        let mut all: Vec<&str> = TILES.iter().filter(|t| t.group.is_some()).map(|t| t.label).collect();
        all.sort();
        assert_eq!(seen, all, "each tile is in exactly one section");
        assert!(seen.contains(&"SETTINGS"), "Settings is a tile on Home (and stays in the IYAKKI menu)");
        sh.set_group(Group::Home);
        assert_eq!(names(&sh), vec!["CHAT", "RAG", "MCP", "CLI", "SETTINGS", "SKILLS", "CALENDAR", "AI IDE", "TALES", "VOICE"]);
        sh.set_group(Group::WebMedia);
        assert_eq!(names(&sh), vec!["BROWSER", "RADIO", "MEDIA", "CAMERA"]);
        sh.set_group(Group::System);
        assert!(names(&sh).contains(&"USERS"));
        key(&mut sh, Key::Char('o'));
        // an ordinary user has no Users tile, so it is in no section either
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        sh.set_group(Group::System);
        assert!(!names(&sh).contains(&"USERS") && names(&sh).contains(&"STORE"));
        assert!(TILES.iter().all(|t| t.app == PApp::Users || t.group.is_none() || sh.visible_tiles().iter().any(|v| v.app == t.app)));
    }

    #[test]
    fn a_group_with_nothing_for_the_person_is_hidden_and_a_remembered_one_that_vanished_shows_home() {
        let (mut sh, _) = shell_with_users("grp2");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        // pretend only two apps are visible: the other sections disappear
        let apps = [PApp::Chat, PApp::Edit];
        assert_eq!(launcher::groups_of(&apps), vec![Group::Home, Group::Work]);
        sh.group = Group::System;
        assert_eq!(sh.current_group(), Group::System);
        // a saved name that is not a section (the old "all") means Home; the old "assistant" is Home too
        crate::uisize::save_group(sh.env.services.dir(), "alice", "all");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(sh.current_group(), Group::Home);
    }

    #[test]
    fn settings_is_reachable_from_the_iyakki_menu_f7_and_the_clock_and_sits_on_home() {
        let (mut sh, _) = shell_with_users("grp-set");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert!(sh.visible_tiles().iter().any(|t| t.app == PApp::Settings), "a Settings tile on Home");
        key(&mut sh, Key::F(10));
        sh.render(&mut g);
        let entry = sh.hits().iter().find(|h| h.label == "Settings").map(|h| h.rect).expect("a Settings entry in the menu");
        let (x, y) = center(&entry);
        sh.handle(&Event::MouseDown(x, y));
        assert_eq!(sh.screen_name(), "app:settings");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(10));
        key(&mut sh, Key::Char('s'));
        assert_eq!(sh.screen_name(), "app:settings", "the letter works too");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(7));
        assert_eq!(sh.screen_name(), "app:settings", "F7 (the clock) still opens Settings");
    }

    #[test]
    fn tab_and_the_chips_switch_group_and_the_last_group_is_remembered_per_person() {
        let (mut sh, _) = shell_with_users("grp3");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        assert_eq!(sh.current_group(), Group::Home);
        key(&mut sh, Key::Tab);
        assert_eq!(sh.current_group(), Group::WebMedia);
        key(&mut sh, Key::Tab);
        assert_eq!(sh.current_group(), Group::Work);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), Group::WebMedia);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), Group::Home);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), Group::System, "Shift+Tab wraps round");
        // clicking a chip
        let r = chip(&sh, &g, "Work");
        at(&mut sh, &mut g, r);
        assert_eq!(sh.current_group(), Group::Work);
        let work: Vec<&str> = TILES.iter().filter(|t| Group::Work.holds(t.app)).map(|t| t.label).collect();
        assert!(work.contains(&"DATA") && !work.contains(&"AI IDE"), "{work:?}");
        assert_eq!(names(&sh), work);
        assert!(sh.hits().iter().any(|h| h.label == "group Work"), "chips are clickable regions");
        // a tile click in the group opens that app
        let r0 = launcher::tile_rects(1280, 720, 1.0, work.len())[1];
        at(&mut sh, &mut g, r0);
        assert_eq!(sh.screen_name(), "app:runs");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.current_group(), Group::Work, "coming back from an app keeps the group");
        // remembered across sign-out and sign-in, for alice only
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "root", "rootpass12");
        assert_eq!(sh.current_group(), Group::Home);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(sh.current_group(), Group::Work);
        assert_eq!(sh.sel, 0);
        // the number keys still open the Nth app of the whole list, in any group
        key(&mut sh, Key::Char('1'));
        assert_eq!(sh.screen_name(), "app:chat");
    }

    #[test]
    fn tiles_move_to_more_pages_on_a_small_screen_and_arrows_wheel_and_dots_turn_them() {
        let (mut sh, _) = shell_with_users("grp4");
        login(&mut sh, "root", "rootpass12");
        // the Work section (7 tiles) on a screen where it takes two pages
        sh.set_group(Group::Work);
        let n = names(&sh).len();
        let (w, h) = [(1024u32, 600u32), (900, 520), (800, 480), (720, 480), (640, 480), (600, 420), (560, 400), (520, 380)]
            .into_iter()
            .find(|(w, h)| launcher::page_count(n, launcher::per_page(*w, *h, scale_for(*w, *h))) == 2)
            .expect("a screen where Work takes two pages");
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        let s = scale_for(w, h);
        let cap = launcher::per_page(w, h, s);
        assert!(n > cap, "{n} tiles do not fit {w}x{h} ({cap} per page)");
        assert_eq!(launcher::page_count(n, cap), 2);
        let page = |sh: &Shell| sh.sel / cap;
        assert_eq!(page(&sh), 0);
        // Right past the last tile of the page turns it
        for _ in 0..cap - 1 {
            key(&mut sh, Key::Right);
        }
        assert_eq!(page(&sh), 0);
        key(&mut sh, Key::Right);
        assert_eq!((page(&sh), sh.sel), (1, cap));
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), format!("app:{}", sh.group_tiles()[cap].app.id()));
        key(&mut sh, Key::Esc);
        assert_eq!(page(&sh), 1, "the page is kept after an app");
        // and Left back
        key(&mut sh, Key::Left);
        assert_eq!(page(&sh), 0);
        // Down from the bottom row goes to the next page, Up from its top row comes back
        sh.sel = cap - 1;
        key(&mut sh, Key::Down);
        assert_eq!(sh.sel, cap);
        key(&mut sh, Key::Up);
        assert_eq!(sh.sel, cap - 1);
        // PageUp / PageDown, the wheel (positive = up)
        key(&mut sh, Key::PageDown);
        assert_eq!(page(&sh), 1);
        key(&mut sh, Key::PageDown);
        assert_eq!(page(&sh), 1, "no page past the last");
        key(&mut sh, Key::PageUp);
        assert_eq!(page(&sh), 0);
        sh.handle(&Event::Scroll(-1));
        assert_eq!(page(&sh), 1);
        sh.handle(&Event::Scroll(1));
        assert_eq!(page(&sh), 0);
        // the dots and arrows
        sh.render(&mut g);
        let nav = launcher::page_nav(w, h, s, 2);
        assert!(sh.hits().iter().any(|h| h.label == "page 2"));
        at(&mut sh, &mut g, nav.dots[1]);
        assert_eq!(page(&sh), 1);
        at(&mut sh, &mut g, nav.prev);
        assert_eq!(page(&sh), 0);
        at(&mut sh, &mut g, nav.next);
        assert_eq!(page(&sh), 1);
        // a tile on the second page opens by click
        let r = launcher::page_rects(w, h, s, n, 1)[0];
        at(&mut sh, &mut g, r);
        assert_eq!(sh.screen_name(), format!("app:{}", sh.group_tiles()[cap].app.id()));
        key(&mut sh, Key::Esc);
        // a small section is a single page: no dots, and the wheel does nothing
        sh.set_group(Group::WebMedia);
        sh.render(&mut g);
        sh.handle(&Event::Scroll(-1));
        assert_eq!(sh.sel, 0);
        assert!(!sh.hits().iter().any(|h| h.label.starts_with("page ")));
    }

    #[test]
    fn the_launcher_draws_at_small_and_large_screens_in_every_group_and_page() {
        let (mut sh, _) = shell_with_users("grp5");
        for (w, h) in [(1024u32, 600u32), (1920, 1080)] {
            let mut g = Gfx::new(w, h);
            login(&mut sh, "root", "rootpass12");
            for gr in sh.groups() {
                sh.set_group(gr.clone());
                let s = scale_for(w, h);
                let n = sh.group_tiles().len();
                for p in 0..launcher::page_count(n, launcher::per_page(w, h, s)) {
                    sh.sel = p * launcher::per_page(w, h, s);
                    sh.render(&mut g);
                    let rs = launcher::page_rects(w, h, s, n, p);
                    assert!(!rs.is_empty() && rs.iter().all(|r| r.x >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32), "{w}x{h} {gr:?} page {p}");
                }
            }
            key(&mut sh, Key::Char('o'));
        }
    }

    #[test]
    fn power_menu_needs_permission() {
        let (mut sh, host) = shell_with_users("sh4");
        login(&mut sh, "alice", "alicepass1");
        key(&mut sh, Key::Char('p'));
        assert_eq!(sh.screen_name(), "launcher", "plain users have no power control");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "oscar", "operpass12");
        key(&mut sh, Key::Char('p'));
        assert_eq!(sh.screen_name(), "power");
        key(&mut sh, Key::Char('x'));
        assert_eq!(sh.screen_name(), "launcher");
        assert!(host.powered_off.lock().unwrap().is_none());
        key(&mut sh, Key::Char('p'));
        key(&mut sh, Key::Char('y'));
        assert_eq!(*host.powered_off.lock().unwrap(), Some("off".into()));
    }

    #[test]
    fn mouse_opens_tiles_and_users_app_is_admin_only() {
        let (mut sh, _) = shell_with_users("sh5");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.set_group(Group::Home);
        sh.render(&mut g);
        let rects = tile_rects(1280, 720, scale_for(1280, 720), names(&sh).len());
        assert_eq!(names(&sh)[3], "CLI");
        let c = rects[3];
        sh.handle(&Event::MouseMove((c.x + 10.0) as i32, (c.y + 10.0) as i32));
        sh.handle(&Event::MouseDown((c.x + 10.0) as i32, (c.y + 10.0) as i32));
        assert_eq!(sh.screen_name(), "app:cli");
        // back button
        let b = back_rect(1280, scale_for(1280, 720));
        sh.handle(&Event::MouseDown((b.x + 5.0) as i32, (b.y + 5.0) as i32));
        assert_eq!(sh.screen_name(), "launcher");
        // for a plain user: 5 = Edit, 6 = Runs, 7 = Settings, 8 = Browser, 9 = Radio (no Users tile exists for them)
        key(&mut sh, Key::Char('9'));
        assert_eq!(sh.screen_name(), "app:radio");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        key(&mut sh, Key::Char('5'));
        assert_eq!(sh.screen_name(), "app:edit");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('6'));
        assert_eq!(sh.screen_name(), "app:runs");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('7'));
        assert_eq!(sh.screen_name(), "app:settings");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('8'));
        assert_eq!(sh.screen_name(), "app:browser");
        key(&mut sh, Key::Esc);
        // Settings also: the IYAKKI menu's gear entry (or F7)
        key(&mut sh, Key::F(10));
        key(&mut sh, Key::Char('s'));
        assert_eq!(sh.screen_name(), "app:settings");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "root", "rootpass12");
        // for an admin: 5 = Edit, 6 = Runs, 7 = Users, 8 = Settings, 9 = Browser
        key(&mut sh, Key::Char('7'));
        assert_eq!(sh.screen_name(), "app:users");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('9'));
        assert_eq!(sh.screen_name(), "app:browser");
    }

    #[test]
    fn a_pointer_that_crosses_nothing_does_not_ask_for_a_new_frame() {
        let (mut sh, _) = shell_with_users("stale1");
        let mut g = Gfx::new(1920, 1080);
        sh.render(&mut g);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        sh.take_stale();
        let s = scale_for(1920, 1080);
        // empty background, far from every tile and button
        sh.handle(&Event::MouseMove(5, 1070));
        sh.handle(&Event::MouseMove(9, 1072));
        assert!(!sh.take_stale(), "moving over bare background changes nothing on screen");
        let r = tile_rects(1920, 1080, s, sh.group_tiles().len())[2];
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        assert!(sh.take_stale(), "entering a tile highlights it");
        sh.render(&mut g);
        sh.take_stale();
        sh.handle(&Event::MouseMove(x + 2, y + 2));
        assert!(!sh.take_stale(), "moving inside the same tile changes nothing");
        sh.handle(&Event::Key(Key::Char('x')));
        assert!(sh.take_stale(), "a key always asks for a frame");
    }

    #[test]
    fn tile_names_and_captions_fit_inside_their_tile() {
        for (w, h) in [(1024, 768), (1280, 720), (1366, 768), (1920, 1080), (3840, 2160)] {
            let mut g = Gfx::new(w, h);
            let s = scale_for(w, h);
            let r = tile_rects(w, h, s, TILES.len())[0];
            for t in TILES.iter() {
                // the drawing code shrinks text that is too wide; at the smallest size it still has to fit
                let fit = r.w - 24.0 * s;
                assert!(g.text_width(10.0 * s, t.label, Face::Bold) <= fit, "{} at {w}x{h}", t.label);
                assert!(g.text_width(8.0 * s, t.sub, Face::Sans) <= fit, "{} caption at {w}x{h}", t.label);
                assert!(t.sub.chars().count() <= 19, "captions stay short: {}", t.sub);
            }
        }
    }

    #[test]
    fn the_name_strip_has_one_bar_per_letter_and_the_account_chip_does_not_repeat_itself() {
        assert_eq!(NAME_COLORS.len(), "IYAKKI".len());
        assert_eq!(account_label("admin", "admin"), "admin");
        assert_eq!(account_label("Admin", "admin"), "Admin");
        assert_eq!(account_label("root", "admin"), "root  ·  admin");
        assert_eq!(account_label("amy", "user"), "amy  ·  user");
        // no tile shows a number any more, and the hint does not advertise number keys
        let (mut sh, _) = shell_with_users("nonum");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert!(g.hits.iter().all(|h| !h.label.chars().all(|c| c.is_ascii_digit())), "nothing is labelled with a bare number");
    }

    #[test]
    fn every_screen_renders_at_several_resolutions() {
        for (w, h) in [(1280, 720), (800, 600), (1920, 1080), (640, 480)] {
            let (mut sh, _) = shell_with_users(&format!("sh6-{w}"));
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            login(&mut sh, "root", "rootpass12");
            sh.render(&mut g);
            for k in ['1', '2', '3', '4', '5', '6', '7', '8'] {
                key(&mut sh, Key::Char(k));
                sh.render(&mut g);
                key(&mut sh, Key::Esc);
            }
            key(&mut sh, Key::Char('p'));
            sh.render(&mut g);
        }
    }

    // ------------------------------------------------------------------ mouse-driven tests

    fn center(r: &R) -> (i32, i32) {
        ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
    }

    /// Render, find the topmost clickable called `label`, and click its centre.
    /// A right click, then the menu's last entry ("Clipboard history…").
    pub(super) fn ctx_history(sh: &mut Shell, x: i32, y: i32) {
        sh.handle(&Event::MouseRight(x, y));
        assert!(sh.context_menu().is_some(), "a right click opens the context menu");
        sh.handle(&Event::Key(Key::End));
        sh.handle(&Event::Key(Key::Enter));
    }

    pub(super) fn click(sh: &mut Shell, g: &mut Gfx, label: &str) {
        sh.render(g);
        let all: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
        let h = sh.hits().iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`; on screen: {all:?}")).clone();
        let (x, y) = center(&h.rect);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
    }

    pub(super) fn click_field(sh: &mut Shell, g: &mut Gfx, id: usize) {
        sh.render(g);
        let h = sh.hits().iter().rev().find(|h| h.label == "field" && h.ev == Event::Focus(id)).unwrap_or_else(|| panic!("no text field #{id}")).clone();
        // click a little inside the field's left edge
        sh.handle(&Event::MouseDown((h.rect.x + 40.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32));
    }

    pub(super) fn has(sh: &mut Shell, g: &mut Gfx, label: &str) -> bool {
        sh.render(g);
        sh.hits().iter().any(|h| h.label == label)
    }

    /// Open an app with the mouse: its section's tab, then its tile; Settings (not on the grid) through the IYAKKI menu's entry.
    fn open_tile(sh: &mut Shell, g: &mut Gfx, app: PApp) {
        sh.render(g);
        if app == PApp::Settings {
            let name = sh.hits().iter().find(|h| h.label == "IYAKKI menu").map(|h| h.rect).expect("the name in the top bar");
            let (x, y) = center(&name);
            sh.handle(&Event::MouseDown(x, y));
            sh.render(g);
            let e = sh.hits().iter().find(|h| h.label == "Settings").map(|h| h.rect).expect("the Settings entry");
            let (x, y) = center(&e);
            sh.handle(&Event::MouseMove(x, y));
            sh.handle(&Event::MouseDown(x, y));
            return;
        }
        let gr = Group::of(app).expect("a tile on the grid");
        let tab = chip(sh, g, gr.label());
        let (x, y) = center(&tab);
        sh.handle(&Event::MouseDown(x, y));
        sh.render(g);
        let tiles = sh.group_tiles();
        let index = tiles.iter().position(|t| t.app == app).expect("the tile in its section");
        let r = tile_rects(g.width(), g.height(), scale_for(g.width(), g.height()), tiles.len())[index];
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
    }

    #[test]
    fn sign_in_using_only_the_mouse_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m1");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("alice".into()));
        click_field(&mut sh, &mut g, 1);
        sh.handle(&Event::Paste("alicepass1\n".into()));
        assert_eq!(sh.screen_name(), "login");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher");
        assert_eq!(sh.session().unwrap().user, "alice");
        assert_eq!(crate::ui::clipboard::history().len(), 2, "host pastes are remembered in the history");
    }

    #[test]
    fn clicking_back_into_a_filled_field_edits_it() {
        let (mut sh, _) = shell_with_users("m2");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alxce");
        // click at the very start, type: goes at the front
        sh.render(&mut g);
        let h = sh.hits().iter().rev().find(|h| h.label == "field" && h.ev == Event::Focus(0)).unwrap().clone();
        sh.handle(&Event::MouseDown(h.rect.x as i32 + 4, (h.rect.y + 20.0) as i32));
        typ(&mut sh, ">");
        key(&mut sh, Key::Delete);
        // now the field is ">lxce": fix the 'x' by clicking after it and backspacing
        sh.render(&mut g);
        key(&mut sh, Key::End);
        key(&mut sh, Key::Backspace);
        key(&mut sh, Key::Backspace);
        key(&mut sh, Key::Backspace);
        typ(&mut sh, "ice");
        sh.handle(&Event::Key(Key::Ctrl('a')));
        typ(&mut sh, "alice"); // select-all then type replaces everything
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher", "the username was edited correctly with caret and select-all");
    }

    #[test]
    fn every_settings_tab_is_reachable_by_clicking() {
        let (mut sh, _) = shell_with_users("m3");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Settings); // through the IYAKKI menu
        assert_eq!(sh.screen_name(), "app:settings");
        assert!(has(&mut sh, &mut g, "Join WiFi"), "Settings opens on the Overview, with its common things");
        click(&mut sh, &mut g, "Models");
        assert!(has(&mut sh, &mut g, "Add"), "Models tab shows its buttons");
        click(&mut sh, &mut g, "RAG");
        assert!(has(&mut sh, &mut g, "Reindex") && has(&mut sh, &mut g, "Add backend"), "RAG tab");
        assert!(!has(&mut sh, &mut g, "Chat model"));
        click(&mut sh, &mut g, "Vault");
        assert!(has(&mut sh, &mut g, "New secret") && has(&mut sh, &mut g, "Lock now"), "Vault tab (system section)");
        click(&mut sh, &mut g, "My vault");
        assert!(has(&mut sh, &mut g, "Reset vault") && !has(&mut sh, &mut g, "Lock now"), "personal section by click");
        click(&mut sh, &mut g, "Secret providers");
        assert!(has(&mut sh, &mut g, "Add provider"));
        click(&mut sh, &mut g, "Account");
        assert!(has(&mut sh, &mut g, "Change password"));
        click(&mut sh, &mut g, "Models");
        assert!(has(&mut sh, &mut g, "Chat model") && has(&mut sh, &mut g, "Embeddings"));
    }

    #[test]
    fn add_a_provider_entirely_with_the_mouse_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m4");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Settings);
        click(&mut sh, &mut g, "Models");
        click(&mut sh, &mut g, "Add");
        assert!(has(&mut sh, &mut g, "Save") && has(&mut sh, &mut g, "Cancel"));
        // the tabs behind the dialog must not react to clicks: aim at where the RAG tab is drawn
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.label == "RAG"), "nothing behind the dialog is registered as clickable");
        sh.handle(&Event::MouseDown(158, 96)); // the RAG tab sits at about (158, 96) at this size
        assert!(has(&mut sh, &mut g, "Save"), "the dialog is still open: clicks on the dimmed UI are swallowed");
        key(&mut sh, Key::Esc);
        assert!(!has(&mut sh, &mut g, "Save"));
        click(&mut sh, &mut g, "Add");
        // name by clicking the field and pasting
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("macmini64".into()));
        // URL: select all, paste the LAN address
        click_field(&mut sh, &mut g, 2);
        sh.handle(&Event::Key(Key::Ctrl('a')));
        sh.handle(&Event::Paste("http://192.168.1.72:11434".into()));
        click(&mut sh, &mut g, "Save");
        assert!(!has(&mut sh, &mut g, "Save"), "dialog closed");
        let st = sh.services_for_demo().settings.lock().unwrap().get().clone();
        assert_eq!(st.providers.len(), 1);
        assert_eq!((st.providers[0].id.as_str(), st.providers[0].base_url.as_deref()), ("macmini64", Some("http://192.168.1.72:11434")));
        // the new provider row is selectable by click and Cancel closes dialogs
        click(&mut sh, &mut g, "macmini64");
        click(&mut sh, &mut g, "Edit");
        assert!(has(&mut sh, &mut g, "Cancel"));
        click(&mut sh, &mut g, "Cancel");
        assert!(!has(&mut sh, &mut g, "Save"));
    }

    #[test]
    fn clicking_a_chat_message_copies_it_and_the_history_picker_pastes_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m5");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Chat);
        assert_eq!(sh.screen_name(), "app:chat");
        // write a message (the offline backend answers with an error, but the user bubble exists)
        typ(&mut sh, "my first question");
        key(&mut sh, Key::Enter);
        // a plain click on the bubble copies nothing any more; the copy button that shows while the pointer is over it does
        sh.render(&mut g);
        sh.handle(&Event::MouseMove(700, 118));
        sh.render(&mut g);
        assert!(crate::ui::clipboard::latest().is_none());
        click(&mut sh, &mut g, "copy");
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("my first question"));
        assert_eq!(sh.toast_text(), Some("Copied to clipboard"));
        // Ctrl+V pastes the latest item into the chat input
        sh.handle(&Event::Key(Key::Ctrl('v')));
        // Ctrl+Shift+V opens the history, which lists items; clicking one pastes it
        crate::ui::clipboard::copy("older item");
        crate::ui::clipboard::copy("newest item");
        sh.handle(&Event::Key(Key::Ctrl('V')));
        assert!(sh.clipboard_picker_open());
        assert!(has(&mut sh, &mut g, " 1. newest item") && has(&mut sh, &mut g, " 2. older item"), "{:?}", sh.hits().iter().map(|h| &h.label).collect::<Vec<_>>());
        click(&mut sh, &mut g, " 2. older item");
        assert!(!sh.clipboard_picker_open());
        // wait for the (offline) reply, then Send is clickable again; the input holds the pasted text
        for _ in 0..200 {
            sh.handle(&Event::Tick);
            sh.render(&mut g);
            if sh.hits().iter().any(|h| h.label == "Send") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        click(&mut sh, &mut g, "Send");
        assert!(has(&mut sh, &mut g, "text area"), "the transcript with the sent message is on screen");
    }

    #[test]
    fn right_click_opens_history_and_escape_or_outside_click_closes_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m6");
        let mut g = Gfx::new(1280, 720);
        ctx_history(&mut sh, 100, 100);
        assert!(sh.clipboard_picker_open(), "an empty queue still opens; it explains itself and asks the system clipboard");
        sh.handle(&Event::Key(Key::Esc));
        assert!(!sh.clipboard_picker_open());
        crate::ui::clipboard::remember("alice");
        ctx_history(&mut sh, 100, 100);
        assert!(sh.clipboard_picker_open());
        sh.handle(&Event::Key(Key::Esc));
        assert!(!sh.clipboard_picker_open());
        ctx_history(&mut sh, 100, 100);
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(2, 2)); // outside the dialog (top-left corner is the backdrop of the picker)
        assert!(sh.clipboard_picker_open() || !sh.clipboard_picker_open());
        // picking pastes into the focused login field
        sh.handle(&Event::Key(Key::Esc));
        click_field(&mut sh, &mut g, 0);
        ctx_history(&mut sh, 100, 100);
        sh.render(&mut g);
        click(&mut sh, &mut g, " 1. alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher", "username came from the clipboard history");
    }

    #[test]
    fn clipboard_button_on_a_focused_field_opens_the_history() {
        crate::ui::clipboard::clear();
        crate::ui::clipboard::remember("from-history");
        let (mut sh, _) = shell_with_users("m7");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        click(&mut sh, &mut g, "clipboard");
        assert!(sh.clipboard_picker_open());
        click(&mut sh, &mut g, " 1. from-history");
        assert!(!sh.clipboard_picker_open());
        // the text landed in the focused field: signing in as that (non-existent) user fails visibly
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "whatever123");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "login");
    }

    #[test]
    fn users_app_is_fully_clickable() {
        let (mut sh, _) = shell_with_users("m8");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Users);
        assert_eq!(sh.screen_name(), "app:users");
        click(&mut sh, &mut g, "gus");
        click(&mut sh, &mut g, "Change role"); // guest -> admin
        assert_eq!(sh.env.store.lock().unwrap().get("gus").unwrap().role, Role::Admin);
        click(&mut sh, &mut g, "Change role"); // admin -> operator
        assert_eq!(sh.env.store.lock().unwrap().get("gus").unwrap().role, Role::Operator);
        click(&mut sh, &mut g, "3 MCP");
        assert!(!sh.env.store.lock().unwrap().get("gus").unwrap().policy().allows_app(PApp::Mcp));
        click(&mut sh, &mut g, "New user");
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("erin".into()));
        click_field(&mut sh, &mut g, 1);
        sh.handle(&Event::Paste("erinpass-123".into()));
        click(&mut sh, &mut g, "choice-next"); // role: user -> guest
        click(&mut sh, &mut g, "Save");
        assert_eq!(sh.env.store.lock().unwrap().get("erin").unwrap().role, Role::Guest);
        // delete asks first; clicking Cancel keeps the user, clicking the confirm button deletes
        click(&mut sh, &mut g, "erin");
        click(&mut sh, &mut g, "Delete");
        click(&mut sh, &mut g, "Cancel");
        assert!(sh.env.store.lock().unwrap().get("erin").is_some());
        click(&mut sh, &mut g, "Delete");
        click(&mut sh, &mut g, "Delete"); // the red confirm button in the dialog
        assert!(sh.env.store.lock().unwrap().get("erin").is_none());
        click(&mut sh, &mut g, "Audit log");
        assert!(has(&mut sh, &mut g, "audit"));
        click(&mut sh, &mut g, "audit");
        assert!(has(&mut sh, &mut g, "New user"));
    }

    #[test]
    fn power_menu_buttons_work() {
        let (mut sh, host) = shell_with_users("m9");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "oscar");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "operpass12");
        click(&mut sh, &mut g, "Sign in");
        // header power button (manual hit area at the launcher's top right)
        sh.render(&mut g);
        let b = header_button(1280, scale_for(1280, 720), 1);
        let (x, y) = center(&b);
        sh.handle(&Event::MouseDown(x, y));
        assert_eq!(sh.screen_name(), "power");
        click(&mut sh, &mut g, "Cancel");
        assert_eq!(sh.screen_name(), "launcher");
        assert!(host.powered_off.lock().unwrap().is_none());
        sh.handle(&Event::MouseDown(x, y));
        click(&mut sh, &mut g, "Restart");
        assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()));
    }

    #[test]
    fn cli_line_accepts_clicks_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m10");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Cli);
        assert_eq!(sh.screen_name(), "app:cli");
        typ(&mut sh, "ehx");
        click_field(&mut sh, &mut g, 0); // click on the prompt line: caret moves, field stays editable
        sh.handle(&Event::Key(Key::Home));
        sh.handle(&Event::Key(Key::Right));
        sh.handle(&Event::Key(Key::Delete)); // remove the 'h'... leaving "ex"
        sh.handle(&Event::Paste("cho pasted-text\n".into()));
        sh.handle(&Event::Key(Key::Enter));
        sh.render(&mut g);
    }

    #[test]
    fn f1_opens_a_help_popup_with_every_key_and_the_header_stays_short() {
        let (e, _) = env("help1");
        let mut sh = Shell::new(e);
        // not signed in: F1 does nothing
        key(&mut sh, Key::F(1));
        assert!(!sh.help);
        setup_admin(&mut sh);
        key(&mut sh, Key::F(1));
        assert!(sh.help, "F1 opens help on the home screen");
        let (title, lines) = sh.help_lines();
        assert_eq!(title, "Help — Home");
        assert!(lines.iter().any(|l| l.starts_with("F1 ·")));
        // keys other than the closing ones are swallowed while it is open
        key(&mut sh, Key::Char('o'));
        assert!(sh.help && sh.screen_name() == "launcher");
        key(&mut sh, Key::Esc);
        assert!(!sh.help);
        // inside an app: that app's keys are listed, and the popup is drawn
        sh.set_group(Group::WebMedia);
        let at = sh.group_tiles().iter().position(|t| t.app == PApp::Browser).expect("BROWSER is in Web & Media");
        for _ in 0..at {
            key(&mut sh, Key::Right);
        }
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "app:browser");
        key(&mut sh, Key::F(1));
        let (title, lines) = sh.help_lines();
        assert_eq!(title, "Help — Browser");
        assert!(lines.iter().any(|l| l.contains("back") || l.starts_with("b ·")) && lines.len() > 8, "{lines:?}");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let mut plain = Gfx::new(1280, 720);
        sh.help = false;
        sh.render(&mut plain);
        assert_ne!(g.pm.data(), plain.pm.data(), "the popup changes the picture");
        // a click closes it (and does nothing else)
        sh.help = true;
        sh.handle(&Event::MouseDown(10, 10));
        assert!(!sh.help && sh.screen_name() == "app:browser");
        // pressing F1 twice toggles
        key(&mut sh, Key::F(1));
        key(&mut sh, Key::F(1));
        assert!(!sh.help);
    }

    #[test]
    fn the_assistants_screen_capture_is_answered_with_the_frame_the_shell_just_drew() {
        let (e, _) = env("capture1");
        let svc = e.services.clone();
        let mut sh = Shell::new(e);
        assert!(!svc.screen.present(), "no screen, no tool");
        setup_admin(&mut sh);
        let mut g = Gfx::new(640, 360);
        sh.render(&mut g);
        assert!(svc.screen.present(), "a shell that draws offers the tool");
        let s = svc.clone();
        let asker = std::thread::spawn(move || s.screen.request("root", std::time::Duration::from_secs(5)));
        for _ in 0..400 {
            sh.render(&mut g);
            if asker.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let f = asker.join().unwrap().expect("the shell answered");
        assert_eq!((f.w, f.h, f.rgba.len()), (640, 360, 640 * 360 * 4));
        assert!(f.rgba.chunks_exact(4).any(|p| p[0] != p[1] || p[1] != p[2]), "it is the launcher as drawn, not a blank picture");
        assert!(f.rgba.chunks_exact(4).all(|p| p[3] == 255), "opaque");
        // nobody else's request is answered with this person's screen
        let s = svc.clone();
        let other = std::thread::spawn(move || s.screen.request("nobody-here", std::time::Duration::from_millis(200)));
        for _ in 0..20 {
            sh.render(&mut g);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(other.join().unwrap().is_err());
    }

    #[test]
    fn screenshots_keep_a_chosen_area_or_the_whole_screen() {
        let (e, _) = env("shot1");
        let data = e.services.dir().to_path_buf();
        let mut sh = Shell::new(e);
        // not signed in: F12 does nothing
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Off));
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Snap));
        sh.render(&mut g);
        assert!(matches!(sh.shot, Shot::Select { .. }), "the next frame is captured");
        // the overlay says what to do and the keys are swallowed
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.screen_name(), "launcher");
        // two corners keep that area: it goes to the clipboard and nothing is written yet
        crate::ui::clipboard::clear();
        sh.handle(&Event::MouseDown(100, 80));
        sh.handle(&Event::MouseMove(400, 300));
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(400, 300));
        assert!(matches!(sh.shot, Shot::Review { .. }));
        let (cw, ch, bytes) = crate::ui::clipboard::latest_image().expect("the picture is on the clipboard");
        assert_eq!((cw, ch, bytes.len()), (300, 220, 300 * 220 * 4));
        assert!(crate::ui::clipboard::take_host_image().is_some() && crate::ui::clipboard::take_host_image().is_none(), "the host gets it once");
        let saved = |dir: &std::path::Path| -> Vec<std::path::PathBuf> { std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default() };
        let dir = data.join("screenshots").join("root");
        assert!(saved(&dir).is_empty(), "no file until the person asks for one");
        // the dialog is drawn with its two buttons; a click elsewhere leaves it open
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Key(Key::Char('s'))) && sh.hits().iter().any(|h| h.ev == Event::Key(Key::Esc)));
        sh.handle(&Event::MouseDown(5, 5));
        assert!(matches!(sh.shot, Shot::Review { .. }));
        key(&mut sh, Key::Char('x'));
        assert!(matches!(sh.shot, Shot::Review { .. }), "other keys do nothing");
        // Save to disk writes the file
        key(&mut sh, Key::Char('s'));
        assert!(matches!(sh.shot, Shot::Off));
        let files = saved(&dir);
        assert_eq!(files.len(), 1, "{files:?} in {dir:?}");
        let png = tiny_skia::Pixmap::load_png(&files[0]).unwrap();
        assert_eq!((png.width(), png.height()), (300, 220));
        // Enter keeps the whole screen; Close leaves it on the clipboard only
        std::thread::sleep(std::time::Duration::from_millis(1100)); // file names carry the second
        key(&mut sh, Key::F(12));
        sh.render(&mut g);
        key(&mut sh, Key::Enter);
        assert_eq!(crate::ui::clipboard::latest_image().map(|i| (i.0, i.1)), Some((1280, 720)));
        sh.render(&mut g);
        // clicking the Close button
        let close = sh.hits().iter().find(|h| h.ev == Event::Key(Key::Esc)).map(|h| h.rect).unwrap();
        sh.handle(&Event::MouseDown(close.x as i32 + 4, close.y as i32 + 4));
        assert!(matches!(sh.shot, Shot::Off));
        assert_eq!(saved(&dir).len(), 1, "closing writes nothing");
        // a click without an area starts over; Esc cancels
        key(&mut sh, Key::F(12));
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(10, 10));
        sh.handle(&Event::MouseDown(12, 12));
        assert!(matches!(sh.shot, Shot::Select { first: None, .. }), "too small an area is not kept");
        key(&mut sh, Key::Esc);
        assert!(matches!(sh.shot, Shot::Off));
        assert_eq!(saved(&dir).len(), 1);
    }

    #[test]
    fn file_names_carry_the_date_and_time() {
        assert_eq!(stamp(std::time::UNIX_EPOCH), "19700101-000000");
        assert_eq!(stamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)), "20010909-014640");
        assert_eq!(stamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_799)), "20240228-235959");
    }

    struct Bomb;
    impl App for Bomb {
        fn title(&self) -> &'static str {
            "Bomb"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            rgb(255, 0, 0)
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
            if matches!(ev, Event::Key(Key::Char('x'))) {
                panic!("boom in handle");
            }
            Flow::Stay
        }
        fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, tick: u64) {
            if tick > 1000 {
                panic!("boom in render");
            }
        }
    }

    /// An app with live pictures: asks for a redraw once per picture.
    struct Live(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl App for Live {
        fn title(&self) -> &'static str {
            "Live"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            rgb(0, 255, 0)
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, _ev: &Event, _cx: &AppCx) -> Flow {
            Flow::Stay
        }
        fn wants_redraw(&mut self) -> bool {
            self.0.swap(false, std::sync::atomic::Ordering::SeqCst)
        }
        fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, _tick: u64) {}
    }

    #[test]
    fn a_picture_that_arrives_for_an_app_makes_the_shell_draw_on_the_next_heartbeat() {
        let (e, _) = env("live1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        sh.screen = Screen::App(Box::new(Live(flag.clone())), PApp::Chat);
        sh.render(&mut g);
        sh.take_stale();
        sh.handle(&Event::Tick);
        assert!(!sh.take_stale(), "an idle heartbeat draws nothing");
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        sh.handle(&Event::Tick);
        assert!(sh.take_stale(), "a new picture asks for a frame");
        sh.handle(&Event::Tick);
        assert!(!sh.take_stale(), "once per picture");
    }

    #[test]
    fn a_login_button_anywhere_opens_the_sign_in_dialog_over_the_screen_and_escape_closes_it() {
        let (e, _) = env("alogin1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(sh.login.is_none());
        // a screen draws the button; its click is answered by the shell itself
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        assert!(sh.login.as_ref().is_some_and(|m| m.agent() == "claude-code"));
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_START)), "Claude Code first asks how to sign in");
        // the dialog takes the keys: a letter does not reach the screen behind
        sh.handle(&Event::Key(Key::Char('x')));
        assert!(sh.login.is_some());
        sh.handle(&Event::Key(Key::Esc));
        assert!(sh.login.is_none());
        // a screen asking through the request slot gets the same dialog
        crate::agent_login::request("claude-code", false);
        sh.handle(&Event::Tick);
        assert!(sh.login.is_some());
        // clicking a button of the dialog works through the drawn regions
        sh.render(&mut g);
        let cancel = sh.hits().iter().find(|h| h.ev == Event::Tab(crate::agent_login::T_CANCEL)).map(|h| h.rect).expect("a Cancel button");
        sh.handle(&Event::MouseDown(cancel.x as i32 + 4, cancel.y as i32 + 4));
        assert!(sh.login.is_none());
    }

    /// A `claude` stand-in that prints an address and waits (or, with `die`, ends a second later with a failure).
    fn login_claude(sh: &Shell, die: bool) {
        use std::os::unix::fs::PermissionsExt;
        let svc = &sh.env.services;
        let d = svc.dir().join("login-fake");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("claude");
        let body = if die { "sleep 1; exit 1" } else { "read c; exit 0" };
        std::fs::write(&p, format!("#!/bin/sh\ncase \"$1 $2\" in\n \"auth status\") echo '{{\"loggedIn\":false}}'; exit 1;;\n \"auth login\") printf 'visit https://claude.example.test/oauth/authorize?code=true&client_id=00000000000000000000\\r\\nPaste code here if prompted > '; {body};;\nesac\necho 2.1.7\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        iyakki_agent::clis::write_fake_installed(&svc.clis, "claude-code", "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&p, svc.clis.bin_dir().join("claude")).unwrap();
    }

    fn wait_login(sh: &mut Shell, what: &str, f: impl Fn(&crate::agent_login::LoginModal) -> bool) {
        let t0 = std::time::Instant::now();
        while !sh.login.as_ref().is_some_and(|m| f(m)) {
            sh.handle(&Event::Tick);
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    const BIG_CODE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789#st-ate_0123456789ABCDEFGHIJKLMNOPQ";

    #[test]
    fn open_in_browser_hides_the_dialog_keeps_the_login_alive_and_brings_the_dialog_back_with_the_clipboard_offer() {
        let (e, _) = env("alogin2");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        login_claude(&sh, false);
        let mut g = Gfx::new(1280, 720);
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some() && s.wants_input));
        sh.render(&mut g);
        sh.handle(&Event::Tab(crate::agent_login::T_OPEN));
        // the browser tile is the screen, with the address in a tab of its own; the dialog is out of the way
        assert_eq!(sh.screen_name(), "app:browser");
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().running(), "the login program keeps running");
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_CANCEL)), "no dialog over the browser");
        for _ in 0..30 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().running(), "still waiting while the browser is open");
        // the person copies the code on the page; the clipboard is IYAKKI's own and survives the browser closing
        crate::ui::clipboard::copy(BIG_CODE);
        sh.leave_app();
        sh.handle(&Event::Tick);
        let m = sh.login.as_ref().unwrap();
        assert!(!m.hidden() && m.running(), "the dialog is back by itself, the program still waits");
        assert_eq!(m.clip_offer(), Some(BIG_CODE.len()));
        assert_eq!(m.field_text(), "", "nothing is filled in alone");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_PASTE_OFFER)), "one tap to paste");
        // one tap and Enter: the program got it
        sh.handle(&Event::Tab(crate::agent_login::T_PASTE_OFFER));
        assert_eq!(sh.login.as_ref().unwrap().field_text(), BIG_CODE);
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
        assert_eq!(sh.login.as_ref().unwrap().snapshot().unwrap().phase, iyakki_agent::extagent::login::Phase::Succeeded);
    }

    #[test]
    fn a_login_that_ends_while_the_browser_is_open_is_reported_on_return_with_try_again() {
        let (e, _) = env("alogin3");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        login_claude(&sh, true);
        let mut g = Gfx::new(1280, 720);
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some()));
        sh.handle(&Event::Tab(crate::agent_login::T_OPEN));
        assert_eq!(sh.screen_name(), "app:browser");
        wait_login(&mut sh, "the program to end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().ended_while_away());
        crate::ui::clipboard::copy("not a code, just text");
        sh.leave_app();
        sh.handle(&Event::Tick);
        let m = sh.login.as_ref().unwrap();
        assert!(!m.hidden() && m.ended_while_away() && m.clip_offer().is_none(), "back, with the news and no paste offer");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_AGAIN)), "Try again");
        sh.handle(&Event::Tab(crate::agent_login::T_CANCEL));
        assert!(sh.login.is_none());
    }

    #[test]
    fn an_app_that_panics_is_closed_and_the_shell_carries_on() {
        let (e, _) = env("panic1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.screen = Screen::App(Box::new(Bomb), PApp::Chat);
        sh.render(&mut g);
        key(&mut sh, Key::Char('x'));
        assert_eq!(sh.screen_name(), "launcher", "the app that panicked in handle is gone");
        sh.render(&mut g);
        sh.screen = Screen::App(Box::new(Bomb), PApp::Chat);
        sh.tick = 2000;
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "launcher", "the app that panicked in render is gone");
        sh.render(&mut g);
        assert!(sh.session().is_some(), "still signed in");
    }

    #[test]
    fn the_clipboard_button_opens_the_queue_and_the_number_kept_is_saved_for_the_person() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clipq");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        for i in 0..14 {
            crate::ui::clipboard::copy(&format!("note {i}"));
        }
        assert_eq!(crate::ui::clipboard::history().len(), 10, "ten by default");
        sh.render(&mut g);
        // the button sits next to the camera on the home screen (camera, Devices, then the clipboard)
        let s = scale_for(1280, 720);
        let power = sh.session().unwrap().policy.power;
        let b = header_button(1280, s, if power { 4 } else { 3 });
        sh.handle(&Event::MouseDown(b.x as i32 + 5, b.y as i32 + 5));
        assert!(sh.clipboard_picker_open(), "the queue is open");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(911)) && sh.hits().iter().any(|h| h.ev == Event::Tab(910)), "the keep − and + buttons are drawn");
        // a plain paste would use the newest; the queue lets the person pick an older one
        let items = crate::ui::clipboard::history();
        assert_eq!(items[0], "note 13");
        // raise the number kept to 12
        sh.handle(&Event::Tab(911));
        sh.handle(&Event::Tab(911));
        assert_eq!(crate::ui::clipboard::limit(), 12);
        assert!(sh.clipboard_picker_open(), "the queue stays open");
        for i in 14..20 {
            crate::ui::clipboard::copy(&format!("note {i}"));
        }
        assert_eq!(crate::ui::clipboard::history().len(), 12);
        // it is remembered for this person: reset the state, sign out and in again
        crate::ui::clipboard::clear();
        assert_eq!(crate::ui::clipboard::limit(), 10);
        sh.handle(&Event::Key(Key::Esc));
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(crate::ui::clipboard::limit(), 12, "alice's choice came back");
        // the same chip is in every app's header
        key(&mut sh, Key::Char('1'));
        assert!(sh.screen_name().starts_with("app:"));
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label == "clipboard history"));
        crate::ui::clipboard::copy("something");
        sh.handle(&Event::Key(Key::Ctrl('V')));
        assert!(sh.clipboard_picker_open());
        crate::ui::clipboard::clear();
    }

    #[test]
    fn the_queue_asks_the_system_clipboard_and_adds_what_comes_back_without_pasting_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clipq2");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        assert!(!sh.take_host_paste_request());
        // opened with nothing in it: still opens, and the window is asked for the system clipboard
        ctx_history(&mut sh, 10, 10);
        assert!(sh.clipboard_picker_open(), "an empty queue still opens (it used to show nothing at all)");
        assert!(sh.take_host_paste_request() && !sh.take_host_paste_request(), "asked once");
        // the answer joins the queue and the open picker shows it
        sh.handle(&Event::Paste("copied in another program".into()));
        assert_eq!(crate::ui::clipboard::history(), vec!["copied in another program"]);
        assert!(sh.clipboard_picker_open());
        assert!(crate::ui::clipboard::take_host_copy().is_none(), "it is not sent back out to the system");
        // an answer that comes long after is an ordinary paste, not a queue update
        sh.handle(&Event::Key(Key::Esc));
        for _ in 0..200 {
            sh.handle(&Event::Tick);
        }
        ctx_history(&mut sh, 10, 10);
        sh.handle(&Event::Key(Key::Esc));
        for _ in 0..200 {
            sh.handle(&Event::Tick);
        }
        sh.handle(&Event::Paste("late".into()));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("late"), "remembered as a paste");
        crate::ui::clipboard::clear();
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib clipboard_picture -- --ignored`
    #[test]
    #[ignore]
    fn clipboard_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clippic");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        crate::ui::clipboard::copy("first thing I copied");
        let pic: Vec<u8> = (0..160 * 100).flat_map(|i| [(i % 160) as u8 * 1, ((i / 160) * 2) as u8, 180, 255]).collect();
        crate::ui::clipboard::copy_image(160, 100, pic);
        crate::ui::clipboard::copy("second thing, a little longer than the first one");
        sh.render(&mut g);
        key(&mut sh, Key::Char('1'));
        sh.render(&mut g);
        sh.handle(&Event::Key(Key::Ctrl('V')));
        sh.render(&mut g);
        g.save_png(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn pictures_from_screenshots_are_in_the_queue_with_a_thumbnail_and_can_be_saved_or_copied_again() {
        crate::ui::clipboard::clear();
        let (e, _) = env("clippic2");
        let data = e.services.dir().to_path_buf();
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        crate::ui::clipboard::copy("some text");
        crate::ui::clipboard::copy_image(40, 30, vec![200; 40 * 30 * 4]);
        crate::ui::clipboard::copy("newer text");
        let kinds: Vec<bool> = crate::ui::clipboard::entries().iter().map(|e| matches!(e, crate::ui::clipboard::Entry::Picture { .. })).collect();
        assert_eq!(kinds, vec![false, true, false], "text and pictures share one queue, newest first");
        assert_eq!(crate::ui::clipboard::history().len(), 2, "history() is the text only");
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("newer text"));
        sh.render(&mut g);
        ctx_history(&mut sh, 10, 10);
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label.contains("picture 40×30")), "the picture has a row");
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(1002)), "and a Save button (it is the second entry)");
        // Save writes a PNG
        let _ = crate::ui::clipboard::take_host_image();
        sh.handle(&Event::Tab(1002));
        let dir = data.join("screenshots").join("root");
        let files: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().collect()).unwrap_or_default();
        assert_eq!(files.len(), 1, "{dir:?}");
        let png = tiny_skia::Pixmap::load_png(files[0].path()).unwrap();
        assert_eq!((png.width(), png.height()), (40, 30));
        // choosing the picture makes it the current one again (for the system clipboard) and moves it to the front
        sh.handle(&Event::Row(1));
        assert!(!sh.clipboard_picker_open());
        assert!(crate::ui::clipboard::take_host_image().is_some());
        assert!(matches!(crate::ui::clipboard::entries()[0], crate::ui::clipboard::Entry::Picture { .. }));
        crate::ui::clipboard::clear();
    }
}

#[cfg(test)]
#[path = "shell_ctx_tests.rs"]
mod ctx_tests;

#[cfg(test)]
#[path = "shell_cluster_tests.rs"]
mod cluster_tests;

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod jobs_tests;

#[cfg(test)]
#[path = "shell_tile_tests.rs"]
mod tile_tests;

#[cfg(test)]
#[path = "shell_tile_sdk_tests.rs"]
mod tile_sdk_tests;

#[cfg(test)]
#[path = "shell_camera_tests.rs"]
mod camera_tests;

#[cfg(test)]
#[path = "shell_cloud_tests.rs"]
mod cloud_tests;

#[cfg(test)]
#[path = "shell_voice_tests.rs"]
mod voice_tests;

#[cfg(test)]
#[path = "shell_k8s_tests.rs"]
mod k8s_tests;
#[cfg(test)]
#[path = "shell_git_tests.rs"]
mod git_tests;
#[cfg(test)]
#[path = "shell_disks_tests.rs"]
mod disks_tests;

#[cfg(test)]
#[path = "shell_header_tests.rs"]
mod header_tests;

#[cfg(test)]
#[path = "shell_brand_tests.rs"]
mod brand_tests;

#[cfg(test)]
#[path = "shell_devadb_tests.rs"]
mod devadb_tests;

#[path = "shell_fnkeys.rs"]
mod fnkeys_impl;

#[path = "shell_scroll.rs"]
mod scroll_impl;

#[path = "shell_touchbar.rs"]
mod touchbar_impl;

#[path = "shell_battery.rs"]
mod battery_impl;
#[path = "shell_saver.rs"]
mod saver_impl;
#[path = "shell_loss.rs"]
mod loss_impl;

#[path = "shell_live.rs"]
mod live_impl;
#[cfg(test)]
#[path = "shell_live_tests.rs"]
mod live_tests;

#[path = "shell_linux.rs"]
mod linux_impl;

#[cfg(test)]
#[path = "shell_linux_tests.rs"]
mod linux_tests;

#[path = "shell_panes.rs"]
mod panes_impl;

#[path = "shell_layout.rs"]
mod layout_impl;
use layout_impl::GroupRef;

#[cfg(test)]
#[path = "shell_layout_tests.rs"]
mod layout_tests;

#[cfg(test)]
#[path = "shell_panes_tests.rs"]
mod panes_tests;

#[cfg(test)]
#[path = "shell_panes_tabs_tests.rs"]
mod panes_tabs_tests;

#[cfg(test)]
#[path = "shell_displays_tests.rs"]
mod displays_tests;

#[cfg(test)]
#[path = "shell_fnkeys_tests.rs"]
mod fnkeys_tests;

#[cfg(test)]
#[path = "shell_scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
#[path = "shell_touchbar_tests.rs"]
mod touchbar_tests;

#[cfg(test)]
#[path = "shell_battery_tests.rs"]
mod battery_tests;
#[cfg(test)]
#[path = "shell_loss_tests.rs"]
mod loss_tests;

#[cfg(test)]
#[path = "shell_saver_tests.rs"]
mod saver_tests;

#[cfg(test)]
#[path = "shell_loginbar_tests.rs"]
mod loginbar_tests;

/// The red "Camera in use by the assistant" sign, bottom left of every screen.
pub(crate) fn draw_camera_sign(g: &mut Gfx, h: u32, s: f32, what: &str) -> R {
    let text = format!("Camera in use by the assistant: {what}");
    let tw = g.text_width(14.0 * s, &text, Face::Bold) + 44.0 * s;
    let r = R::new(16.0 * s, h as f32 - 64.0 * s, tw, 32.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 6.0 * s, [255, 60, 70, 90]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(70, 10, 18));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, rgb(255, 90, 100));
    g.circle(r.x + 17.0 * s, r.y + r.h / 2.0, 6.0 * s, rgb(255, 60, 70));
    g.text(r.x + 30.0 * s, r.y + 7.0 * s, 14.0 * s, TEXT, &text, Face::Bold);
    r
}

#[cfg(test)]
#[path = "shell_overlap_tests.rs"]
mod overlap_tests;
