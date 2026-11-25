//! `FakeCore`: the UI's handle on the core, in memory (`iyakki_core_api::Core`). Canned mixer numbers in, every played sound recorded.

use iyakki_core_api::{AppData, Desktop, HomeFiles, SessionResume, UiSocket, AppStatus, AppStatuses, Core, Mixer, MixerStats, MonitorInfo, Monitors, SoundKind};
use std::any::Any;
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeCore {
    mixer: FakeMixer,
    sound: crate::sound::FakeSound,
    cues: crate::sound::FakeCues,
    statuses: FakeStatuses,
    monitors: FakeMonitors,
    ui_state: FakeUiState,
    ui_socket: FakeUiSocket,
    braille: FakeBraille,
    resume: FakeResume,
    file_tree: crate::files::FakeFileTree,
    terminal: crate::terminal::FakeTerminal,
    cloud: FakeCloud,
    browser: crate::fakebrowser::FakeBrowser,
    assistant: crate::assistant::FakeAssistant,
    cameras: crate::cameras::FakeCameras,
    app_events: crate::appevents::FakeAppEvents,
    app_actions: crate::appactions::FakeAppActions,
    scope: FakeFileScope,
    host: FakeHost,
    media: crate::media::FakeMedia,
    files: FakeFiles,
    route: std::sync::Arc<crate::audioroute::FakeAudioRoute>,
    route_at: Mutex<Option<RouteAt>>,
}

/// Makes the route over given sound roots (a test over a made-up tree on disk brings its own reader).
type RouteAt = std::sync::Arc<dyn Fn(&std::path::Path, &std::path::Path) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> + Send + Sync>;

/// Machine facts in memory: whatever a test sets, empty otherwise.
#[derive(Default)]
pub struct FakeHost(Mutex<HostCanned>);

#[derive(Clone, Default)]
struct HostCanned {
    cmdline: String,
    memory: iyakki_core_api::sysinfo::Memory,
    product: String,
    uptime: u64,
    proc_root: Option<std::path::PathBuf>,
    sys_root: Option<std::path::PathBuf>,
}

impl FakeHost {
    pub fn set_cmdline(&self, c: &str) {
        lk(&self.0).cmdline = c.into();
    }
    pub fn set_memory(&self, m: iyakki_core_api::sysinfo::Memory) {
        lk(&self.0).memory = m;
    }
    pub fn set_product(&self, p: &str) {
        lk(&self.0).product = p.into();
    }
    pub fn set_uptime(&self, s: u64) {
        lk(&self.0).uptime = s;
    }
    pub fn set_roots(&self, proc_root: std::path::PathBuf, sys_root: std::path::PathBuf) {
        let mut c = lk(&self.0);
        c.proc_root = Some(proc_root);
        c.sys_root = Some(sys_root);
    }
}

impl iyakki_core_api::HostFacts for FakeHost {
    fn cmdline(&self) -> String {
        lk(&self.0).cmdline.clone()
    }
    fn memory(&self) -> iyakki_core_api::sysinfo::Memory {
        lk(&self.0).memory
    }
    fn product_name(&self) -> String {
        lk(&self.0).product.clone()
    }
    fn uptime_secs(&self) -> u64 {
        lk(&self.0).uptime
    }
    fn proc_root(&self) -> std::path::PathBuf {
        lk(&self.0).proc_root.clone().unwrap_or_else(|| "/proc".into())
    }
    fn sys_root(&self) -> std::path::PathBuf {
        lk(&self.0).sys_root.clone().unwrap_or_else(|| "/sys".into())
    }
}

/// File scope in memory: the person's policy decides (as in the real core), the home folder is `<data>/home/<user>`, and every question is recorded.
#[derive(Default)]
pub struct FakeFileScope {
    asked: Mutex<Vec<String>>,
}

impl FakeFileScope {
    /// `check_read(user, path)` / `check_write(user, path)` lines in the order asked.
    pub fn asked(&self) -> Vec<String> {
        lk(&self.asked).clone()
    }
}

impl iyakki_core_api::FileScope for FakeFileScope {
    fn check_read(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        lk(&self.asked).push(format!("check_read({user},{})", p.display()));
        if policy.allows_read(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied: {}", p.display())) }
    }
    fn check_write(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        lk(&self.asked).push(format!("check_write({user},{})", p.display()));
        if policy.allows_write(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied (write): {}", p.display())) }
    }
    fn resolve(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        let j = if input.starts_with('/') { std::path::PathBuf::from(input) } else { cwd.join(input) };
        Ok(j.canonicalize().unwrap_or(j))
    }
    fn ensure_parent(&self, path: &std::path::Path) -> Result<(), String> {
        match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => std::fs::create_dir_all(p).map_err(|e| e.to_string()),
            _ => Ok(()),
        }
    }
}


/// Cloud mode in memory: the desktop until a test sets another mode.
#[derive(Default)]
pub struct FakeCloud(Mutex<Option<iyakki_cloudmode::CloudMode>>);

impl FakeCloud {
    pub fn set_for_test(&self, c: iyakki_cloudmode::CloudMode) {
        *lk(&self.0) = Some(c);
    }
}

impl iyakki_core_api::CloudModes for FakeCloud {
    fn detect(&self) -> iyakki_cloudmode::CloudMode {
        lk(&self.0).clone().unwrap_or_else(iyakki_cloudmode::CloudMode::desktop)
    }
}


/// The session ticket in memory: `dev` is whether developer mode is on (the test sets it).
#[derive(Default)]
pub struct FakeResume {
    pub dev: std::sync::atomic::AtomicBool,
    switch: std::sync::atomic::AtomicBool,
    who: Mutex<Option<String>>,
    apps: Mutex<Vec<String>>,
    items: Mutex<Vec<(String, String)>>,
    /// somebody is signed in and no sign-out came (the marker of F-LIFE-16)
    open: std::sync::atomic::AtomicBool,
}

impl FakeResume {
    /// The shell "died" without a sign-out: the marker stays for the next `take_unexpected_end`.
    pub fn set_open_for_test(&self, on: bool) {
        self.open.store(on, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn set_switch_for_test(&self, on: bool) {
        self.switch.store(on, std::sync::atomic::Ordering::SeqCst);
    }
    /// A ticket written by an earlier shell (whatever the switch says; `who` still needs both switches).
    pub fn set_who_for_test(&self, user: &str) {
        *lk(&self.who) = Some(user.to_string());
    }
    pub fn clear_for_test(&self) {
        *lk(&self.who) = None;
    }
}

impl SessionResume for FakeResume {
    fn allowed(&self) -> bool {
        self.dev.load(std::sync::atomic::Ordering::SeqCst) && self.switch_on()
    }
    fn switch_on(&self) -> bool {
        self.switch.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_switch(&self, on: bool) -> Result<(), String> {
        self.switch.store(on, std::sync::atomic::Ordering::SeqCst);
        if !on {
            self.forget();
        }
        Ok(())
    }
    fn take_unexpected_end(&self) -> bool {
        self.open.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
    fn remember(&self, user: &str) {
        self.open.store(true, std::sync::atomic::Ordering::SeqCst);
        *lk(&self.who) = self.allowed().then(|| user.to_string());
    }
    fn who(&self) -> Option<String> {
        if self.allowed() { lk(&self.who).clone() } else { None }
    }
    fn forget(&self) {
        self.open.store(false, std::sync::atomic::Ordering::SeqCst);
        *lk(&self.who) = None;
        lk(&self.apps).clear();
    }
    fn remember_apps(&self, apps: &[String]) {
        if self.allowed() && lk(&self.who).is_some() {
            *lk(&self.apps) = apps.to_vec();
        }
    }
    fn apps(&self) -> Vec<String> {
        if self.allowed() { lk(&self.apps).clone() } else { Vec::new() }
    }
    fn remember_items(&self, items: &[(String, String)]) {
        if self.allowed() && lk(&self.who).is_some() {
            *lk(&self.items) = items.to_vec();
        }
    }
    fn items(&self) -> Vec<(String, String)> {
        if self.allowed() { lk(&self.items).clone() } else { Vec::new() }
    }
}

/// Files of made-up homes: `(user, name) -> text`.
#[derive(Default)]
pub struct FakeFiles {
    files: Mutex<std::collections::BTreeMap<(String, String), String>>,
}

impl FakeFiles {
    pub fn put(&self, user: &str, name: &str, text: &str) {
        lk(&self.files).insert((user.into(), name.into()), text.into());
    }
}

impl HomeFiles for FakeFiles {
    fn list_markdown(&self, user: &str) -> Vec<String> {
        lk(&self.files).keys().filter(|k| k.0 == user).map(|k| k.1.clone()).collect()
    }
    fn read_markdown(&self, user: &str, name: &str) -> Result<String, String> {
        lk(&self.files).get(&(user.to_string(), name.to_string())).cloned().ok_or_else(|| "that is not a Markdown file of your home folder".to_string())
    }
}

/// A braille display in memory: `connect(width)` plugs one in; what was put on it and the keys a test presses are shared with the handle
/// the shell gets.
#[derive(Default)]
pub struct FakeBraille(std::sync::Arc<Mutex<BrailleState>>);

#[derive(Default)]
struct BrailleState {
    width: usize,
    shown: Vec<String>,
    keys: Vec<iyakki_core_api::BrailleKey>,
    opened: usize,
}

impl FakeBraille {
    pub fn connect(&self, width: usize) {
        lk(&self.0).width = width;
    }
    pub fn disconnect(&self) {
        lk(&self.0).width = 0;
    }
    pub fn press(&self, k: iyakki_core_api::BrailleKey) {
        lk(&self.0).keys.push(k);
    }
    /// Everything put on the display so far, oldest first.
    pub fn shown(&self) -> Vec<String> {
        lk(&self.0).shown.clone()
    }
    pub fn opened(&self) -> usize {
        lk(&self.0).opened
    }
}

struct FakeCells(std::sync::Arc<Mutex<BrailleState>>);

impl iyakki_core_api::BrailleCells for FakeCells {
    fn width(&self) -> usize {
        lk(&self.0).width
    }
    fn put(&mut self, cells: &str) -> Result<(), String> {
        lk(&self.0).shown.push(cells.to_string());
        Ok(())
    }
    fn keys(&mut self) -> Vec<iyakki_core_api::BrailleKey> {
        std::mem::take(&mut lk(&self.0).keys)
    }
}

impl iyakki_core_api::BrailleLink for FakeBraille {
    fn open(&self) -> Option<Box<dyn iyakki_core_api::BrailleCells>> {
        let mut st = lk(&self.0);
        if st.width == 0 {
            return None;
        }
        st.opened += 1;
        Some(Box::new(FakeCells(self.0.clone())))
    }
    fn status(&self) -> String {
        match lk(&self.0).width {
            0 => "No braille display is connected.".into(),
            w => format!("A braille display with {w} cells is connected."),
        }
    }
}

/// The UI socket switch in memory.
#[derive(Default)]
pub struct FakeUiSocket {
    on: std::sync::atomic::AtomicBool,
}

impl UiSocket for FakeUiSocket {
    fn status(&self) -> String {
        format!("The UI socket is {}.", if self.enabled() { "on" } else { "off (the default)" })
    }
    fn enabled(&self) -> bool {
        self.on.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_enabled(&self, on: bool) -> Result<String, String> {
        self.on.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(self.status())
    }
}

/// App data and desktop choices in memory.
#[derive(Default)]
pub struct FakeUiState {
    data: Mutex<std::collections::BTreeMap<(String, String, String), String>>,
    dock: Mutex<std::collections::BTreeMap<String, Vec<String>>>,
    wallpaper: Mutex<std::collections::BTreeMap<String, String>>,
    screens: Mutex<std::collections::BTreeMap<(String, u32, String), String>>,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl AppData for FakeUiState {
    fn get(&self, user: &str, app: &str, key: &str) -> Option<String> {
        lk(&self.data).get(&(user.into(), app.into(), key.into())).cloned()
    }
    fn set(&self, user: &str, app: &str, key: &str, value: &str) -> Result<(), String> {
        lk(&self.data).insert((user.into(), app.into(), key.into()), value.into());
        Ok(())
    }
    fn remove(&self, user: &str, app: &str, key: &str) -> Result<(), String> {
        lk(&self.data).remove(&(user.into(), app.into(), key.into()));
        Ok(())
    }
    fn keys(&self, user: &str, app: &str) -> Vec<String> {
        lk(&self.data).keys().filter(|k| k.0 == user && k.1 == app).map(|k| k.2.clone()).collect()
    }
}

impl Desktop for FakeUiState {
    fn dock(&self, user: &str) -> Option<Vec<String>> {
        lk(&self.dock).get(user).cloned()
    }
    fn set_dock(&self, user: &str, items: &[String]) -> Result<(), String> {
        lk(&self.dock).insert(user.into(), items.to_vec());
        Ok(())
    }
    fn wallpaper(&self, user: &str) -> Option<String> {
        lk(&self.wallpaper).get(user).cloned()
    }
    fn set_wallpaper(&self, user: &str, key: &str) -> Result<(), String> {
        lk(&self.wallpaper).insert(user.into(), key.into());
        Ok(())
    }
    fn screen_wallpaper(&self, user: &str, screen: u32) -> Option<String> {
        lk(&self.screens).get(&(user.to_string(), screen, "wallpaper".to_string())).cloned()
    }
    fn set_screen_wallpaper(&self, user: &str, screen: u32, key: &str) -> Result<(), String> {
        lk(&self.screens).insert((user.into(), screen, "wallpaper".into()), key.into());
        Ok(())
    }
    fn screen_dock(&self, user: &str, screen: u32) -> Option<String> {
        lk(&self.screens).get(&(user.to_string(), screen, "dock".to_string())).cloned()
    }
    fn set_screen_dock(&self, user: &str, screen: u32, position: &str) -> Result<(), String> {
        lk(&self.screens).insert((user.into(), screen, "dock".into()), position.into());
        Ok(())
    }
}

/// Made-up screens: `(id, label, percent)`. Setting writes only the chosen one, like the real core.
#[derive(Default)]
pub struct FakeMonitors {
    screens: Mutex<Vec<(String, String, u8)>>,
    chosen: Mutex<Option<String>>,
}

impl FakeMonitors {
    pub fn set_screens(&self, v: &[(&str, &str, u8)]) {
        *self.screens.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = v.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), *c)).collect();
    }
    pub fn percent_of(&self, id: &str) -> Option<u8> {
        self.screens.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|s| s.0 == id).map(|s| s.2)
    }
}

impl Monitors for FakeMonitors {
    fn list(&self) -> Vec<MonitorInfo> {
        let chosen = self.chosen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        self.screens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .enumerate()
            .map(|(i, (id, label, p))| MonitorInfo { id: Some(id.clone()), label: label.clone(), connector: format!("fake-{i}"), percent: Ok(*p), chosen: chosen.as_deref() == Some(id.as_str()) })
            .collect()
    }
    fn choose(&self, id: Option<&str>) -> Result<(), String> {
        if let Some(i) = id {
            if !self.screens.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|s| s.0 == i) {
                return Err("no screen with that id answers now".into());
            }
        }
        *self.chosen.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = id.map(str::to_string);
        Ok(())
    }
    fn set_brightness(&self, percent: u8) -> Result<u8, String> {
        let chosen = self.chosen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().ok_or("no screen is chosen; choose one first")?;
        let p = percent.clamp(1, 100);
        let mut v = self.screens.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = v.iter_mut().find(|s| s.0 == chosen).ok_or("the chosen screen does not answer now")?;
        s.2 = p;
        Ok(p)
    }
}

/// The apps' statuses as the test says them.
#[derive(Default)]
pub struct FakeStatuses {
    all: Mutex<Vec<(String, AppStatus)>>,
    tokens: Mutex<Option<(u64, u64, u64)>>,
    waiting: Mutex<Vec<iyakki_core_api::WaitingCall>>,
}

impl FakeStatuses {
    pub fn set(&self, v: Vec<(String, AppStatus)>) {
        *self.all.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = v;
    }
    pub fn set_waiting(&self, w: Vec<iyakki_core_api::WaitingCall>) {
        *self.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = w;
    }
    pub fn set_tokens(&self, t: Option<(u64, u64, u64)>) {
        *self.tokens.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = t;
    }
}

impl AppStatuses for FakeStatuses {
    fn all(&self, _user: &str) -> Vec<(String, AppStatus)> {
        self.all.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn waiting_calls(&self, _user: &str) -> Vec<iyakki_core_api::WaitingCall> {
        self.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn tokens_24h(&self, _user: &str) -> Option<(u64, u64, u64)> {
        *self.tokens.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Default)]
pub struct FakeMixer {
    stats: Mutex<Option<MixerStats>>,
    sources: Mutex<Vec<(String, u32)>>,
    played: Mutex<Vec<(SoundKind, String, usize)>>,
}

impl FakeCore {
    /// The machine facts the fake answers with.
    pub fn host_fake(&self) -> &FakeHost {
        &self.host
    }
    /// The file-scope fake (what was asked).
    pub fn file_scope_for_test(&self) -> &FakeFileScope {
        &self.scope
    }
    pub fn new() -> FakeCore {
        FakeCore::default()
    }
    /// The made-up terminal of the embedded terminal.
    pub fn braille_fake(&self) -> &FakeBraille {
        &self.braille
    }
    pub fn terminal_fake(&self) -> &crate::terminal::FakeTerminal {
        &self.terminal
    }
    /// The made-up files of the embedded file browser.
    pub fn file_tree_fake(&self) -> &crate::files::FakeFileTree {
        &self.file_tree
    }
    /// The embedded assistant with its scripted model and audit lines.
    pub fn assistant_fake(&self) -> &crate::assistant::FakeAssistant {
        &self.assistant
    }
    /// The fake app actions (notes, flows, tool requests waiting for the person).
    pub fn app_actions_fake(&self) -> &crate::appactions::FakeAppActions {
        &self.app_actions
    }
    /// The scripted app events (the real engine with a test clock, scopes the test grants and a store in memory).
    pub fn app_events_fake(&self) -> &crate::appevents::FakeAppEvents {
        &self.app_events
    }
    /// The scripted camera views (cameras, frames, motion, the cards' effects and the audit lines).
    pub fn cameras_fake(&self) -> &crate::cameras::FakeCameras {
        &self.cameras
    }
    /// The embedded media player with its virtual clock and audit lines.
    pub fn media_fake(&self) -> &crate::media::FakeMedia {
        &self.media
    }
    /// The made-up sound output (state, outputs, notices).
    pub fn sound_fake(&self) -> &crate::sound::FakeSound {
        &self.sound
    }
    /// The made-up cue player (what was played).
    pub fn cues_fake(&self) -> &crate::sound::FakeCues {
        &self.cues
    }
    pub fn mixer_fake(&self) -> &FakeMixer {
        &self.mixer
    }
    pub fn statuses_fake(&self) -> &FakeStatuses {
        &self.statuses
    }
    /// The person the ticket names right now (None when not allowed).
    pub fn session_resume_who(&self) -> Option<String> {
        self.resume.who()
    }
    pub fn files_fake(&self) -> &FakeFiles {
        &self.files
    }
    pub fn resume_fake(&self) -> &FakeResume {
        &self.resume
    }
    pub fn monitors_fake(&self) -> &FakeMonitors {
        &self.monitors
    }
}

impl FakeMixer {
    pub fn set_stats(&self, s: Option<MixerStats>) {
        *self.stats.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = s;
    }
    pub fn set_sources(&self, s: Vec<(String, u32)>) {
        *self.sources.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = s;
    }
    /// Every sound played so far: kind, label, size of the WAV.
    pub fn played(&self) -> Vec<(SoundKind, String, usize)> {
        self.played.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl Mixer for FakeMixer {
    fn stats(&self) -> Option<MixerStats> {
        self.stats.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn active_sources(&self) -> Vec<(String, u32)> {
        self.sources.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn play_wav(&self, kind: SoundKind, label: &str, wav: &[u8]) -> Result<Box<dyn Any + Send>, String> {
        self.played.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((kind, label.to_string(), wav.len()));
        Ok(Box::new(()))
    }
}

impl FakeCore {
    /// Set the cloud mode the fake core answers with.
    pub fn cloud_for_test(&self) -> &FakeCloud {
        &self.cloud
    }
    /// The scripted browser sessions (pages, known-bad sites, the calls it saw).
    /// The made-up sound facts the core's `audio_route` answers from.
    pub fn audio_route_fake(&self) -> std::sync::Arc<crate::audioroute::FakeAudioRoute> {
        self.route.clone()
    }
    /// Answer `audio_route_at` with the route `f` makes over the roots (default: the made-up in-memory tree, whatever the roots).
    pub fn set_route_at(&self, f: impl Fn(&std::path::Path, &std::path::Path) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> + Send + Sync + 'static) {
        *self.route_at.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(f));
    }
    pub fn browser_fake(&self) -> &crate::fakebrowser::FakeBrowser {
        &self.browser
    }
}

impl Core for FakeCore {
    fn host(&self) -> &dyn iyakki_core_api::HostFacts {
        &self.host
    }
    fn file_scope(&self) -> &dyn iyakki_core_api::FileScope {
        &self.scope
    }
    fn cloud(&self) -> &dyn iyakki_core_api::CloudModes {
        &self.cloud
    }
    fn browser(&self) -> &dyn iyakki_core_api::BrowserSessions {
        &self.browser
    }
    fn mixer(&self) -> &dyn Mixer {
        &self.mixer
    }
    fn audio_route(&self) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> {
        self.route.clone()
    }
    fn audio_route_at(&self, proc_root: &std::path::Path, sys_root: &std::path::Path) -> std::sync::Arc<dyn iyakki_core_api::AudioRoute> {
        let f = self.route_at.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        match f {
            Some(f) => f(proc_root, sys_root),
            None => self.route.clone(),
        }
    }
    fn sound(&self) -> &dyn iyakki_core_api::SoundControl {
        &self.sound
    }
    fn cues(&self) -> &dyn iyakki_core_api::CuePlayer {
        &self.cues
    }
    fn app_status(&self) -> &dyn AppStatuses {
        &self.statuses
    }
    fn monitors(&self) -> &dyn Monitors {
        &self.monitors
    }
    fn ui_socket(&self) -> &dyn UiSocket {
        &self.ui_socket
    }
    fn braille(&self) -> &dyn iyakki_core_api::BrailleLink {
        &self.braille
    }
    fn session_resume(&self) -> &dyn SessionResume {
        &self.resume
    }
    fn files(&self) -> &dyn iyakki_core_api::Files {
        &self.file_tree
    }
    fn terminal(&self) -> &dyn iyakki_core_api::Terminal {
        &self.terminal
    }
    fn home_files(&self) -> &dyn HomeFiles {
        &self.files
    }
    fn app_data(&self) -> &dyn AppData {
        &self.ui_state
    }
    fn assistant(&self) -> &dyn iyakki_core_api::Assistant {
        &self.assistant
    }
    fn app_actions(&self) -> &dyn iyakki_core_api::AppActions {
        &self.app_actions
    }
    fn app_events(&self) -> &dyn iyakki_core_api::AppEvents {
        &self.app_events
    }
    fn cameras(&self) -> &dyn iyakki_core_api::Cameras {
        &self.cameras
    }
    fn media(&self) -> &dyn iyakki_core_api::Media {
        &self.media
    }
    fn desktop(&self) -> &dyn Desktop {
        &self.ui_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_answers_what_it_was_given_and_records_what_was_played() {
        let c = FakeCore::new();
        assert!(c.mixer().stats().is_none());
        c.mixer_fake().set_stats(Some(MixerStats { sink_open: true, detail: "2 source(s)".into(), last_error: None }));
        c.mixer_fake().set_sources(vec![("Radio".into(), 80)]);
        assert!(c.mixer().stats().unwrap().sink_open);
        assert_eq!(c.mixer().active_sources(), vec![("Radio".to_string(), 80)]);
        assert!(c.mixer().play_wav(SoundKind::Notification, "tone", &[0; 44]).is_ok());
        assert_eq!(c.mixer_fake().played(), vec![(SoundKind::Notification, "tone".to_string(), 44)]);
    }
}
