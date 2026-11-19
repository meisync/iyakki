//! The one handle the UI program holds on the core (alpha.5 item 0a, docs/development/UI-BOUNDARY.md). The apps and the shell ask `cx.core`
//! (a `Arc<dyn Core>`) for what they used to take from `iyakki_agent::` directly; the device gives them `iyakki_agent::corehandle::AgentCore`
//! (backed by the agent in the same process today, a local-socket client when `iyakki-ui` becomes its own program), a test gives them
//! `iyakki_core_fake::FakeCore`. Each slice of the move adds one facet here; the ratchet `ui_agent_uses` counts what is left.

use std::any::Any;

/// What a sound is, which fixes its priority on the speaker: alarm > speech > notification > media > radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Radio,
    Media,
    Notification,
    Speech,
    Alarm,
}

/// The mixer's state as the Settings Sound page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MixerStats {
    /// The sound card is open and the mixer is feeding it.
    pub sink_open: bool,
    /// The long text of the numbers (sources, load, underruns, buffer).
    pub detail: String,
    /// The last problem the mixer met, if any.
    pub last_error: Option<String>,
}

/// The sound mixer: what is playing, its state, and a place to play a short sound.
pub trait Mixer: Send + Sync {
    /// The mixer's numbers; `None` while it is off or was never used.
    fn stats(&self) -> Option<MixerStats>;
    /// The sounds playing now and the person's level (percent) for each, one entry per label.
    fn active_sources(&self) -> Vec<(String, u32)>;
    /// Play a WAV on the speaker. The returned value keeps the sound alive: dropping it stops it.
    fn play_wav(&self, kind: SoundKind, label: &str, wav: &[u8]) -> Result<Box<dyn Any + Send>, String>;
}

/// What an app says about itself to the home screen (LAUNCHER-PLAN slice 5, F-LAUNCH-09): the chip and badge on its icon, the line
/// under its name and, when a person is needed, a row of the Attention list. An app with nothing to say has no status at all: the icon
/// then shows the app's own static line, never a made-up figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// Doing something now (a station playing, jobs running).
    Running,
    /// Something is waiting on the person (an approval, an overdue task, a drive problem).
    Attention,
    /// A one-time step is open (updates to install).
    Setup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppStatus {
    pub kind: StatusKind,
    /// The line under the icon's name ("Playing Radio X", "3 updates").
    pub line: String,
    /// The number on the icon's corner badge (0: none).
    pub count: usize,
    /// For `Attention` and `Setup` that need a person: the row's title and its detail on the machine page's Attention list.
    pub needs: Option<(String, String)>,
}

/// One tool call that waits for a person's answer: the tool's name and one line on what it will do. Never the arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingCall {
    pub tool: String,
    pub summary: String,
}

/// The apps' statuses for one person, asked through the core (the UI never reads the radio, the jobs or the inbox itself for this).
pub trait AppStatuses: Send + Sync {
    /// `(app id, status)` for every app that has something to say now, most urgent first.
    fn all(&self, user: &str) -> Vec<(String, AppStatus)>;
    /// What the person's assistant and tools used in the last 24 hours, from the usage ledger: `(tokens in, tokens out, calls)`. `None`
    /// when the ledger has nothing for them (the widget board then says so instead of showing a made-up zero).
    fn tokens_24h(&self, _user: &str) -> Option<(u64, u64, u64)> {
        None
    }
    /// The single tool calls waiting for this person's answer, oldest first (the approvals widget lists them). Empty when none wait or the
    /// core does not say.
    fn waiting_calls(&self, _user: &str) -> Vec<WaitingCall> {
        Vec::new()
    }
}

/// One screen with a DDC/CI channel, as the person sees it (F-KBD-08).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// The id the person chooses by (maker, product, serial of the screen's identity block); `None` when the screen did not answer.
    pub id: Option<String>,
    /// The name to show; the connector when the screen did not answer.
    pub label: String,
    pub connector: String,
    /// The brightness in percent, or the plain reason it cannot be read (many monitors do not answer DDC/CI).
    pub percent: Result<u8, String>,
    /// This is the screen the person chose; only that one is ever written to.
    pub chosen: bool,
}

/// External monitor brightness over DDC/CI. The core opens the bus; reading is free, writing needs the screen to be the chosen one.
pub trait Monitors: Send + Sync {
    /// Every screen with a DDC channel (reads only).
    fn list(&self) -> Vec<MonitorInfo>;
    /// Choose the screen with this id as the only one that may be written to (`None`: none). The id must belong to a screen that answers now.
    fn choose(&self, id: Option<&str>) -> Result<(), String>;
    /// Set the brightness (1 to 100) of the chosen screen; the value read back.
    fn set_brightness(&self, percent: u8) -> Result<u8, String>;
}

/// The monitors of a core that has no DDC/CI (a fake, or a machine without the I2C driver).
pub struct NoMonitors;

impl Monitors for NoMonitors {
    fn list(&self) -> Vec<MonitorInfo> {
        Vec::new()
    }
    fn choose(&self, _id: Option<&str>) -> Result<(), String> {
        Err("this machine has no screens that can be set over DDC/CI".into())
    }
    fn set_brightness(&self, _percent: u8) -> Result<u8, String> {
        Err("this machine has no screens that can be set over DDC/CI".into())
    }
}

/// What an SDK app keeps (docs/development/UI-SDK.md): text under short keys, one namespace per person and app, readable by that app only.
/// The core owns the file and the limits; the app never opens one.
pub trait AppData: Send + Sync {
    fn get(&self, user: &str, app: &str, key: &str) -> Option<String>;
    fn set(&self, user: &str, app: &str, key: &str, value: &str) -> Result<(), String>;
    fn remove(&self, user: &str, app: &str, key: &str) -> Result<(), String>;
    fn keys(&self, user: &str, app: &str) -> Vec<String>;
}

/// The person's own desktop choices an SDK app may change through the core: the pinned dock apps and the wallpaper.
pub trait Desktop: Send + Sync {
    /// The pinned apps; `None` when the person never chose (the Look's list applies).
    fn dock(&self, user: &str) -> Option<Vec<String>>;
    fn set_dock(&self, user: &str, items: &[String]) -> Result<(), String>;
    /// The wallpaper key; `None` when the person never chose.
    fn wallpaper(&self, user: &str) -> Option<String>;
    fn set_wallpaper(&self, user: &str, key: &str) -> Result<(), String>;
    /// A choice for one screen of this person (`None`: the person's own choice applies there). Screens are told apart by the id the
    /// arrangement gives them.
    fn screen_wallpaper(&self, _user: &str, _screen: u32) -> Option<String> {
        None
    }
    fn set_screen_wallpaper(&self, _user: &str, _screen: u32, _key: &str) -> Result<(), String> {
        Err("this core keeps no per-screen choices".into())
    }
    /// Where the dock sits on one screen: `bottom`, `left`, `right` or `off` (`None`: the Look's choice, off by default on a second screen).
    fn screen_dock(&self, _user: &str, _screen: u32) -> Option<String> {
        None
    }
    fn set_screen_dock(&self, _user: &str, _screen: u32, _position: &str) -> Result<(), String> {
        Err("this core keeps no per-screen choices".into())
    }
}

/// The socket a UI program outside the process uses (F-SPLIT-07): off by default, a unix socket on this machine only.
pub trait UiSocket: Send + Sync {
    /// The status in plain words (switch, scopes, account).
    fn status(&self) -> String;
    fn enabled(&self) -> bool;
    /// Switch it on or off (the caller is an administrator who chose to). The scopes and the account stay as they were; the answer is the
    /// new status.
    fn set_enabled(&self, on: bool) -> Result<String, String>;
}

/// A key of a connected braille display, as the screen reader needs it (F-A11Y-07).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrailleKey {
    PanLeft,
    PanRight,
    FocusPrev,
    FocusNext,
    /// The routing key above display cell `n` (from 0).
    Route(usize),
}

/// One connected braille display. It only moves cells out and keys in: what the cells say is the screen reader's business.
pub trait BrailleCells: Send {
    /// How many cells the display has.
    fn width(&self) -> usize;
    /// Show these cells (Unicode braille patterns; anything else is blank).
    fn put(&mut self, cells: &str) -> Result<(), String>;
    /// The keys pressed since the last call (never waits).
    fn keys(&mut self) -> Vec<BrailleKey>;
}

/// The door to a braille display the shell reaches through the core (F-A11Y-07): the shell never opens a device node itself.
pub trait BrailleLink: Send + Sync {
    /// Open the first display connected right now; `None` when there is none or it could not be opened.
    fn open(&self) -> Option<Box<dyn BrailleCells>>;
    /// Whether a display is connected, in plain words.
    fn status(&self) -> String;
}

/// Putting the person back at the desktop after the shell process died (F-SHELL-65): developer and test machines only.
/// The ticket holds no password and no key: the vault stays locked until the person types the password.
pub trait SessionResume: Send + Sync {
    /// Whether a ticket may be written and read now: developer mode is on AND the Settings switch is on.
    fn allowed(&self) -> bool;
    /// The Settings switch alone (off by default).
    fn switch_on(&self) -> bool;
    fn set_switch(&self, on: bool) -> Result<(), String>;
    /// Say who is signed in (called again every few seconds); does nothing when not allowed, and removes any ticket then.
    fn remember(&self, user: &str);
    /// The person to put back: a fresh ticket of this boot, only when allowed.
    fn who(&self) -> Option<String>;
    /// A sign-out: remove the ticket.
    fn forget(&self);
    /// The ids of the apps that are open, the one held aside first and the one on the screen last (F-SHELL-65): kept in the ticket the
    /// last `remember` wrote, with the ticket's rules (only when allowed; no content of an app, only its id). Does nothing by default.
    fn remember_apps(&self, _apps: &[String]) {}
    /// The app ids of the ticket to put back (empty: only the person and the home screen come back).
    fn apps(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The machine's settings, read as text by the UI (the first domain a UI program outside the process gets over the contract socket).
/// Writing stays with the typed settings contract and its scopes; nothing here changes a setting.
pub trait SettingsView: Send + Sync {
    /// The keys the machine offers, sorted.
    fn keys(&self) -> Vec<String>;
    /// One setting as text (`true`, `42`, or the text itself), or why it cannot be read (not allowed, unknown key, no connection).
    fn get(&self, key: &str) -> Result<String, String>;
}

/// Whether the person's vault is open and how many entries it holds (never a name or a value). The second domain of the UI socket.
pub trait VaultView: Send + Sync {
    /// `(locked, entries)` of the vault of `user`; over the UI socket the machine answers for the account the listener serves.
    fn status(&self, user: &str) -> Result<(bool, u32), String>;
}

/// The Markdown files of a person's own home folder, for the viewer's file list (F-SDK-31). Names are relative to the home folder; nothing
/// outside it is listed or read, links are not followed, and an app that asked to open a file never sees where it is.
pub trait HomeFiles: Send + Sync {
    /// Names (relative, sorted, at most 200) of the person's Markdown files.
    fn list_markdown(&self, user: &str) -> Vec<String>;
    /// The text of one file by the name `list_markdown` gave (at most 1 MB of text).
    fn read_markdown(&self, user: &str, name: &str) -> Result<String, String>;
}

/// The flows of a person, as the UI lists them (the third domain of the UI socket): `(id, name)`.
pub trait FlowsView: Send + Sync {
    /// `may_use` is the caller's own knowledge of the person's policy (may they use FLOWS at all); the core still checks its own rules. Over
    /// the UI socket the machine answers for the account the listener serves.
    fn list(&self, user: &str, may_use: bool) -> Result<Vec<(String, String)>, String>;
    /// Is the person's schedule runner going (FLOWS need not be open)? A core without flows says no.
    fn scheduler_running(&self, _user: &str) -> bool {
        false
    }
    /// Is a last workflow written down for the person (cheap: no engine is opened)?
    fn last_flow_recorded(&self, _user: &str) -> bool {
        false
    }
    /// Forget the last workflow.
    fn clear_last_flow(&self, _user: &str) {}
    /// The person signed out: their schedules stop.
    fn stop_for_logout(&self, _user: &str) {}
}

/// The app data and desktop of a core that keeps none (a fake without them, a preview).
pub struct NoUiState;

impl AppData for NoUiState {
    fn get(&self, _: &str, _: &str, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("this core keeps no app data".into())
    }
    fn remove(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("this core keeps no app data".into())
    }
    fn keys(&self, _: &str, _: &str) -> Vec<String> {
        Vec::new()
    }
}

impl BrailleLink for NoUiState {
    fn open(&self) -> Option<Box<dyn BrailleCells>> {
        None
    }
    fn status(&self) -> String {
        "This core reaches no braille display.".into()
    }
}

impl UiSocket for NoUiState {
    fn status(&self) -> String {
        "This core serves no UI socket.".into()
    }
    fn enabled(&self) -> bool {
        false
    }
    fn set_enabled(&self, _: bool) -> Result<String, String> {
        Err("this core serves no UI socket".into())
    }
}

impl SessionResume for NoUiState {
    fn allowed(&self) -> bool {
        false
    }
    fn switch_on(&self) -> bool {
        false
    }
    fn set_switch(&self, _: bool) -> Result<(), String> {
        Err("this core keeps no session ticket".into())
    }
    fn remember(&self, _: &str) {}
    fn who(&self) -> Option<String> {
        None
    }
    fn forget(&self) {}
}

impl SettingsView for NoUiState {
    fn keys(&self) -> Vec<String> {
        Vec::new()
    }
    fn get(&self, _: &str) -> Result<String, String> {
        Err("this core shows no settings".into())
    }
}

impl VaultView for NoUiState {
    fn status(&self, _: &str) -> Result<(bool, u32), String> {
        Err("this core shows no vault".into())
    }
}

impl HomeFiles for NoUiState {
    fn list_markdown(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn read_markdown(&self, _: &str, _: &str) -> Result<String, String> {
        Err("this core shows no files".into())
    }
}

impl FlowsView for NoUiState {
    fn list(&self, _: &str, _: bool) -> Result<Vec<(String, String)>, String> {
        Err("this core shows no flows".into())
    }
}

impl Desktop for NoUiState {
    fn dock(&self, _: &str) -> Option<Vec<String>> {
        None
    }
    fn set_dock(&self, _: &str, _: &[String]) -> Result<(), String> {
        Err("this core keeps no desktop choices".into())
    }
    fn wallpaper(&self, _: &str) -> Option<String> {
        None
    }
    fn set_wallpaper(&self, _: &str, _: &str) -> Result<(), String> {
        Err("this core keeps no desktop choices".into())
    }
}

/// Cloud mode as the core decides it (F-SPLIT-03): whether this machine hides what needs a local screen, sound or camera. The core knows the
/// administrator's setting, the boot parameter and the machine's hardware, so the UI only asks.
#[cfg(feature = "native")]
pub trait CloudModes: Send + Sync {
    /// The mode now (window mode, boot parameter, setting and `/sys` facts decided). The default is a desktop with everything shown.
    fn detect(&self) -> iyakki_cloudmode::CloudMode {
        iyakki_cloudmode::CloudMode::desktop()
    }
}

/// A core with nothing to say about cloud mode: always the desktop.
#[cfg(feature = "native")]
pub struct DesktopOnly;

#[cfg(feature = "native")]
impl CloudModes for DesktopOnly {}

/// What a person's files may be touched by the UI (F-SPLIT-03, domain `settings scope`): the checks against the person's policy, the
/// person's home folder and path resolving. The core decides; the UI asks. The defaults deny, so a core that does not serve the
/// facet can never let a path through by accident.
#[cfg(feature = "native")]
pub trait FileScope: Send + Sync {
    /// May this person read `p`? `Err` is the sentence to show.
    fn check_read(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, p: &std::path::Path) -> Result<(), String> {
        Err(format!("permission denied: {}", p.display()))
    }
    /// May this person write `p`?
    fn check_write(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, p: &std::path::Path) -> Result<(), String> {
        Err(format!("permission denied (write): {}", p.display()))
    }
    /// The person's home folder (a real path).
    fn home_root(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        data_dir.join("home").join(user)
    }
    /// The home folder for code that creates folders in it (never the real `$HOME` in tests).
    fn home_root_for_writes(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        self.home_root(user, data_dir)
    }
    /// The `HOME` a program started for the person must get; `None` when this is not the IYAKKI system.
    fn system_home(&self, _user: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// The folders the person's pickers and tools stay inside (home, workspaces, readable scopes, granted host folders). The default knows
    /// only the home folder.
    fn roots(&self, _policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> iyakki_filescope::FileRoots {
        let home = self.home_root(user, data_dir);
        self.roots_with_home(_policy, user, data_dir, home)
    }
    /// [`FileScope::roots`] with the home folder chosen by the caller.
    fn roots_with_home(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, _data_dir: &std::path::Path, home: std::path::PathBuf) -> iyakki_filescope::FileRoots {
        iyakki_filescope::FileRoots { roots: vec![home.clone()], home, mounts: Vec::new() }
    }
    /// Resolve `input` (absolute, or relative to `cwd`) to a canonical path.
    fn resolve(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        Err(format!("cannot resolve {input} (relative to {})", cwd.display()))
    }
    /// Like `resolve`, for a file about to be written.
    fn resolve_for_write(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        self.resolve(cwd, input)
    }
    /// Make the folder above `path`.
    fn ensure_parent(&self, _path: &std::path::Path) -> Result<(), String> {
        Err("the core does not serve file scope".into())
    }
    /// A sentence for an I/O error on `path`.
    fn explain_io(&self, e: &std::io::Error, path: &std::path::Path, action: &str) -> String {
        format!("cannot {action} {}: {e}", path.display())
    }
}

/// A core with no file scope: everything denied.
#[cfg(feature = "native")]
pub struct NoFileScope;

#[cfg(feature = "native")]
impl FileScope for NoFileScope {}

/// Facts about the machine the pages show, asked of the core (F-SPLIT-03, domain `settings sysinfo`). The UI never reads `/proc` or `/sys` and
/// never names the roots: a test gives a fake machine, the device gives the real one.
pub trait HostFacts: Send + Sync {
    /// The kernel command line (one line).
    fn cmdline(&self) -> String {
        String::new()
    }
    /// Installed and free memory.
    fn memory(&self) -> crate::sysinfo::Memory {
        crate::sysinfo::Memory::default()
    }
    /// Installed memory in whole kibibytes.
    fn mem_total_kb(&self) -> u64 {
        self.memory().total / 1024
    }
    /// The machine's model; empty when unknown.
    fn product_name(&self) -> String {
        String::new()
    }
    /// Seconds since boot.
    fn uptime_secs(&self) -> u64 {
        0
    }
    /// The user and group that own the running program.
    fn self_owner(&self) -> (u32, u32) {
        (0, 0)
    }
    /// Where a short-lived file shared with another process belongs (RAM-backed where there is one).
    fn shared_memory_dir(&self) -> std::path::PathBuf {
        std::env::temp_dir()
    }
    /// What the System page shows, read from the given roots (`SysRoots::real()` on the device, a made-up machine in a test).
    fn cpu_times(&self, _r: &crate::sysinfo::SysRoots) -> crate::sysinfo::CpuTimes {
        Default::default()
    }
    fn cpu_count(&self, _r: &crate::sysinfo::SysRoots) -> usize {
        1
    }
    fn memory_of(&self, _r: &crate::sysinfo::SysRoots) -> crate::sysinfo::Memory {
        Default::default()
    }
    fn load_average(&self, _r: &crate::sysinfo::SysRoots) -> (f32, f32, f32) {
        (0.0, 0.0, 0.0)
    }
    fn uptime_of(&self, _r: &crate::sysinfo::SysRoots) -> u64 {
        0
    }
    fn hottest_temp(&self, _r: &crate::sysinfo::SysRoots) -> Option<f32> {
        None
    }
    fn net_counters(&self, _r: &crate::sysinfo::SysRoots) -> Vec<crate::sysinfo::NetCounters> {
        Vec::new()
    }
    fn disk_counters(&self, _r: &crate::sysinfo::SysRoots) -> Vec<crate::sysinfo::DiskCounters> {
        Vec::new()
    }
    /// Stop (SIGTERM) or kill (SIGKILL) a process; process 1 and the program itself are refused.
    fn signal(&self, _pid: u32, _hard: bool) -> Result<(), String> {
        Err("this core cannot signal processes".into())
    }
    fn processes(&self, _r: &crate::sysinfo::SysRoots) -> Vec<crate::sysinfo::Proc> {
        Vec::new()
    }
    fn net_use(&self, _r: &crate::sysinfo::SysRoots, _pids: &[u32]) -> std::collections::HashMap<u32, crate::sysinfo::NetUse> {
        Default::default()
    }
    /// The folder the core reads process facts from (`/proc` on the device; a made-up machine in a test). Pure helpers take it as a parameter.
    fn proc_root(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("/proc")
    }
    /// The folder the core reads hardware facts from (`/sys` on the device).
    fn sys_root(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("/sys")
    }
}

/// A core with no facts about the machine: empty answers.
pub struct NoHostFacts;
impl HostFacts for NoHostFacts {}

/// Everything the UI may ask of the core, facet by facet.
pub trait Core: Send + Sync {
    /// Facts about the machine (cmdline, memory, model, uptime).
    fn host(&self) -> &dyn HostFacts {
        &NoHostFacts
    }
    /// The person's files: policy checks, home folder, path resolving (default: everything denied).
    #[cfg(feature = "native")]
    fn file_scope(&self) -> &dyn FileScope {
        &NoFileScope
    }
    /// Cloud mode (default: desktop).
    #[cfg(feature = "native")]
    fn cloud(&self) -> &dyn CloudModes {
        &DesktopOnly
    }
    /// The Markdown files of the person's home (F-SDK-31).
    fn home_files(&self) -> &dyn HomeFiles {
        &NoUiState
    }
    /// The person's flows (served over the UI socket as the third domain).
    fn flows_view(&self) -> &dyn FlowsView {
        &NoUiState
    }
    /// Whether the vault is open (served over the UI socket as the second domain).
    fn vault_view(&self) -> &dyn VaultView {
        &NoUiState
    }
    /// The machine's settings as text (served over the UI socket as the first domain, F-SPLIT-03).
    fn settings_view(&self) -> &dyn SettingsView {
        &NoUiState
    }
    /// Resuming the session after the shell process died (F-SHELL-65).
    fn session_resume(&self) -> &dyn SessionResume {
        &NoUiState
    }
    /// The socket of a UI program outside the process (F-SPLIT-07).
    fn ui_socket(&self) -> &dyn UiSocket {
        &NoUiState
    }
    /// A connected braille display (F-A11Y-07).
    fn braille(&self) -> &dyn BrailleLink {
        &NoUiState
    }
    /// What an SDK app keeps (UI-SDK storage port).
    fn app_data(&self) -> &dyn AppData {
        &NoUiState
    }
    /// The dock and wallpaper choices (UI-SDK desktop ports).
    fn desktop(&self) -> &dyn Desktop {
        &NoUiState
    }
    /// The person's own files as an app embeds a file browser (docs/development/UI-SDK.md): listing and reading text in the home folder.
    fn files(&self) -> &dyn crate::files::Files {
        &crate::files::NoFiles
    }
    /// The command line an app embeds as a terminal (docs/development/UI-SDK.md): sessions checked by the person's policy, approvals by the core.
    fn terminal(&self) -> &dyn crate::terminal::Terminal {
        &crate::terminal::NoTerminal
    }
    /// Embedded browser sessions for SDK apps (F-SDK-60): the core judges every address, loads the page, hands back a display list.
    fn browser(&self) -> &dyn crate::browsersession::BrowserSessions {
        &crate::browsersession::NoBrowser
    }
    /// The assistant an app embeds (docs/development/UI-SDK.md): sessions with app tools, approvals through the core, audit.
    fn assistant(&self) -> &dyn crate::assistant::Assistant {
        &crate::assistant::NoAssistant
    }
    /// What an app's event handler may do: leave a note, start a flow, ask for a chat tool (F-SDK-201).
    fn app_actions(&self) -> &dyn crate::appactions::AppActions {
        &crate::appactions::NoAppActions
    }
    /// Events delivered to apps, online and offline (F-SDK-188): registration per app, one bounded queue per app, scopes the person sees.
    fn app_events(&self) -> &dyn crate::appevents::AppEvents {
        &crate::appevents::NoAppEvents
    }
    /// Camera views an app embeds (F-SDK-160): the core lists the cameras the person may watch, asks them before a view starts, hands out frames.
    fn cameras(&self) -> &dyn crate::cameras::Cameras {
        &crate::cameras::NoCameras
    }
    /// The media player an app embeds (docs/development/UI-SDK.md, "Embedding audio and video"): decoding in the core, sound through the mixer.
    fn media(&self) -> &dyn crate::media::Media {
        &crate::media::NoMedia
    }
    /// External monitor brightness (F-KBD-08).
    fn monitors(&self) -> &dyn Monitors {
        &NoMonitors
    }
    /// The sound output: which output is chosen and why, the notices of the watcher (F-SPLIT-03).
    fn sound(&self) -> &dyn crate::soundctl::SoundControl {
        &crate::soundctl::NoSound
    }
    /// The short system sounds: sign-in, sign-out, a notification, a finished job.
    fn cues(&self) -> &dyn crate::soundctl::CuePlayer {
        &crate::soundctl::NoCues
    }
    fn mixer(&self) -> &dyn Mixer;
    /// What the apps say about themselves right now (the home screen's icon lines, chips and Attention list).
    fn app_status(&self) -> &dyn AppStatuses;
}
