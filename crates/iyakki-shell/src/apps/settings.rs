//! Settings: models (Ollama, OpenAI, Claude, Gemini, OpenRouter, …), RAG and external RAG,
//! vaults (local encrypted + external providers) and your account.
//!
//! Everyone sees the app; system-wide tabs are read-only unless the role has `manage_settings`
//! (admins). Every change is re-checked here, saved atomically, and written to the audit log —
//! by *name* only, never by value. Secrets typed here go straight into a vault: settings files
//! only ever hold references like `vault:openai-key`.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::providers::{valid_provider_id, Kind, ProviderCfg};
use iyakki_agent::rag_ext::{ExtClient, ExternalRag};
use iyakki_agent::services::Services;
use iyakki_agent::settings::{ExtEntry, ModelChoice};
use iyakki_agent::vault::{valid_secret_name, ProviderConfig as SecretProvider, VaultError};
use iyakki_agent::hostengines;
use iyakki_agent::hostvoice::{self, HostVoice};
use iyakki_agent::story as st;
use iyakki_agent::voice::{self, Player, Recorder, SpeechEngine};

use super::controls::{self, Slider};
use super::devices::{DevRow, DevTab, DevicesPane, DeviceEnv, Target};
use super::settings_nav as nav;
use super::widgets::{render_confirm, Form, FormEvent, PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::accounts::AccountError;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// The third kind of text-to-speech engine: the voices of the browser or desktop window the person is using.
const BROWSER_CHOICE: &str = "Browser voice (uses the voices of the browser or desktop you are using)";
const ACCENT: [u8; 4] = rgb(120, 170, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Models,
    Agents,
    Rag,
    Vault,
    Skills,
    Network,
    Bluetooth,
    Display,
    Sound,
    Browser,
    DevCluster,
    Storage,
    Memory,
    Time,
    HostName,
    Install,
    Voice,
    IndicVoices,
    SecretProviders,
    HostAccess,
    Developer,
    Account,
}

impl Tab {
    /// The heading this page sits under in the left list.
    fn group(self) -> &'static str {
        match self {
            Tab::Models | Tab::Agents | Tab::Rag | Tab::Skills | Tab::Voice | Tab::IndicVoices => "Assistant",
            Tab::Browser | Tab::HostAccess | Tab::DevCluster => "Apps",
            Tab::Vault | Tab::SecretProviders | Tab::Account => "Security",
            Tab::Network | Tab::Bluetooth | Tab::Display | Tab::Sound | Tab::Storage => "Devices",
            Tab::Memory | Tab::Install | Tab::Time | Tab::HostName | Tab::Developer => "System",
        }
    }

    /// Words that should find this page when searching.
    fn keywords(self) -> &'static str {
        match self {
            Tab::Models => "ai chat llm model provider openai claude anthropic gemini ollama openrouter api key endpoint",
            Tab::Agents => "claude code copilot github coding agent cli codex gemini aider anthropic external install login api key auto approval",
            Tab::Rag => "search knowledge documents retrieval chunk overlap results embedding index external qdrant chroma",
            Tab::Vault => "secrets keys passwords tokens encrypted lock",
            Tab::Skills => "instructions prompts reusable assistant behaviour",
            Tab::Network => "wifi wi-fi ethernet internet lan cable ip address connection wireless",
            Tab::Bluetooth => "bluetooth wireless pairing headphones keyboard mouse adapter",
            Tab::Display => "screen monitor brightness resolution backlight output hdmi",
            Tab::Sound => "audio volume speaker mute loud quiet card output hdmi displayport usb headphones test sound",
            Tab::Browser => "web internet browser javascript scripts pictures images styles css cookies privacy sites",
            Tab::DevCluster => "k3s kubernetes cluster kubectl kubeconfig deploy registry namespace podman profile context node bare metal in-cluster service account vault developer",
            Tab::Storage => "disk usb drive data persistence save space format",
            Tab::Memory => "ram zram swap swappiness compressed memory zstd lz4 zswap pressure out of memory oom",
            Tab::Time => "clock time date timezone time zone utc 12 24 hour am pm seconds ntp sntp network time server sync 1970 battery wrong",
            Tab::HostName => "host name hostname computer name rename network name dns mdns",
            Tab::Developer => "developer mode adb dev install sideload pair pairing remote deploy tile app laptop computer key revoke push watch live reload logs screenshot",
            Tab::Install => "install installer disk erase internal ssd nvme sata drive usb copy computer partition boot",
            Tab::Voice => "microphone speech voice stt tts speak listen recording",
            Tab::IndicVoices => "indic tts neural voice tamil telugu malayalam kannada hindi bengali marathi gujarati punjabi odia assamese ai4bharat fastpitch hifigan download model usb offline read aloud",
            Tab::SecretProviders => "vault external hashicorp provider secrets",
            Tab::HostAccess => "host files folders share import export drag drop computer home read-only link copy unplugged audit",
            Tab::Account => "password profile sign out user login",
        }
    }

    /// One line under the page's title.
    fn blurb(self) -> &'static str {
        match self {
            Tab::Models => "The AI services the assistant can talk to, and which one it uses.",
            Tab::Agents => "Claude Code, GitHub Copilot and other coding agents that can answer the chat next to the built-in models.",
            Tab::Rag => "How documents are split and searched when the assistant looks things up.",
            Tab::Vault => "Secrets such as API keys, kept encrypted.",
            Tab::Skills => "Reusable instructions the assistant can follow.",
            Tab::Network => "Ethernet and WiFi: what this computer has, and what is switched on.",
            Tab::Bluetooth => "The Bluetooth adapter, if there is one.",
            Tab::Display => "The screen in use, connected displays and brightness.",
            Tab::Sound => "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.",
            Tab::Browser => "What the web browser may do: run scripts, show pictures, apply styles, keep cookies.",
            Tab::DevCluster => "The clusters the AI IDE can deploy to, this machine's own k3s, and where images go.",
            Tab::Storage => "Where your accounts, settings and files are kept.",
            Tab::Memory => "RAM, compressed swap in memory (zram) and an optional swap file.",
            Tab::Time => "The clock in the top bar: time zone, 12 or 24 hours, and setting the time from the network.",
            Tab::HostName => "The name of this computer on the network.",
            Tab::Developer => "Let a paired computer install, run and watch apps here (iyakki dev). Off by default.",
            Tab::Install => "Put IYAKKI on this computer's disk, from the stick it is running from.",
            Tab::Voice => "Speech to text and text to speech.",
            Tab::IndicVoices => "Neural voices for Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi and more: install, remove, choose.",
            Tab::SecretProviders => "External vaults that hold secrets for you.",
            Tab::HostAccess => "Folders of the computer this window runs on that this account may use, and what it may do with them.",
            Tab::Account => "Your account on this computer.",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Models => "Models",
            Tab::Agents => "Agents",
            Tab::Rag => "RAG",
            Tab::Vault => "Vault",
            Tab::Skills => "Skills",
            Tab::Network => "Network",
            Tab::Bluetooth => "Bluetooth",
            Tab::Display => "Display",
            Tab::Sound => "Sound",
            Tab::Browser => "Browser",
            Tab::DevCluster => "Dev cluster",
            Tab::Storage => "Storage",
            Tab::Memory => "Memory",
            Tab::Time => "Time",
            Tab::HostName => "Host name",
            Tab::Developer => "Developer mode",
            Tab::Install => "Install to disk",
            Tab::Voice => "Voice",
            Tab::IndicVoices => "Indic voices",
            Tab::SecretProviders => "Secret providers",
            Tab::HostAccess => "Host access",
            Tab::Account => "Account",
        }
    }
}

#[path = "settings_agents.rs"]
mod agents;
#[path = "settings_lemonade.rs"]
mod lemonade_page;
#[path = "settings_devcluster.rs"]
mod devcluster;
#[path = "settings_host.rs"]
mod host_access;
#[path = "settings_memory.rs"]
mod memory;
#[path = "settings_sound.rs"]
mod sound;
#[path = "settings_time.rs"]
mod time;
#[path = "settings_install.rs"]
mod install_page;
#[path = "settings_indic.rs"]
mod indic_page;
#[path = "settings_developer.rs"]
mod developer_page;
pub use install_page::InstallEnv;

#[derive(Clone, Debug, PartialEq, Eq)]
enum FormKind {
    Provider(Option<String>),
    ProviderKey(String),
    External(Option<String>),
    NewSecret(bool),
    Keyfile,
    SecretProvider,
    Password,
    VoiceStt,
    VoiceTts,
    VoiceCmds,
    NewSkill,
    ImportSkill,
    TokenPrice,
    /// (disk name, the user must also type ERASE because the disk holds data)
    Persist(String, bool),
    /// Make a virtual disk (the size is asked).
    NewVirtualDisk,
    /// The passphrase of a WiFi network found by the scan (name, what it offers).
    WifiJoin(String, iyakki_devctl::wifi::Security),
    /// A WiFi network that does not announce its name.
    WifiHidden,
    /// Remove a virtual disk (its name must be typed).
    RemoveVirtualDisk(String),
    /// Settings → Agents: the model of an agent; a custom agent (new, or the id being edited).
    AgentModel(String),
    AgentCustom(Option<String>),
    /// Settings → Dev cluster.
    Dc(devcluster::DcForm),
    /// Settings → Memory: one of its settings.
    Mem(memory::MemField),
    /// Settings → Time: the time server; Settings → Host name: the new name.
    TimeServer,
    HostName,
    /// Settings → Install to disk: the questions and the typed confirmation for this disk.
    Install(String),
    /// Settings → Indic voices: install from a folder (the language), where the bundles are published, where the server listens.
    IndicFolder(String),
    IndicSource,
    IndicServer,
}

/// The extra last line of the provider form's Type choice: look at the address and use what answers.
const AUTO_TYPE: &str = "Auto-detect (Ollama, Lemonade or OpenAI-compatible)";

#[derive(Clone, Debug, PartialEq, Eq)]
enum PickKind {
    /// A time zone from the list in Settings → Time.
    Zone,
    Chat(String),
    Embed(String),
    /// A network from the WiFi scan (or one of the choices below the list).
    Wifi,
    /// A saved WiFi network to forget.
    WifiForget,
    /// The vault entry holding an external agent's API key.
    AgentKey(String),
    /// The model of an external agent (a line of its list, or "Custom model id…").
    AgentModel(String),
    /// Settings → Models: a model of a Lemonade provider, then what to do with it, then the reading voice.
    Lemonade(lemonade_page::LemPick),
    /// Settings → Network: the private MAC address mode for every adapter of this kind.
    MacMode(iyakki_devctl::LinkKind),
    /// Settings → Sound: Automatic or one output.
    SoundOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConfirmKind {
    Provider(String),
    External(String),
    Secret(bool, String),
    SecretProvider(String),
    ResetPersonal,
    Skill(iyakki_agent::skills::Scope, String),
    Reboot,
    AgentInstall(String),
    AgentAuto(String),
    AgentTest(String),
    AgentRemove(String),
    Dc(devcluster::DcAct),
    /// Settings → Memory: apply the draft to the running system.
    MemApply,
    /// Settings → Network: change this adapter's MAC address now (the connection drops for a moment).
    MacApply(String),
    /// Settings → Indic voices: download or delete a language (the approval card names the host and the size).
    Indic(indic_page::IndicAct),
    /// Settings → Models: pull (download) or load a Lemonade model (the approval card names the model and its size).
    Lemonade(lemonade_page::LemAct),
}

enum Modal {
    Form(FormKind, Form),
    Pick(PickKind, Picker),
    Confirm(ConfirmKind, String),
}

enum JobOut {
    Msg(Result<String, String>),
    Wifi(Result<Vec<iyakki_devctl::wifi::Ap>, String>),
    Models { provider: String, models: Vec<String>, ms: u64 },
    Dc(devcluster::DcOut),
}

struct Job {
    label: String,
    rx: Receiver<JobOut>,
}

/// What the Storage tab acts on. Real paths in production; tests point these at a fake machine.
#[derive(Clone)]
pub struct StorageEnv {
    pub roots: iyakki_storage::Roots,
    pub mounter: Arc<dyn iyakki_storage::Mounter>,
    /// Set when the disks are virtual (the desktop window): disks can be made and removed from the tab.
    pub virtual_disks: Option<iyakki_storage::virt::VirtualDisks>,
    pub mke2fs: std::path::PathBuf,
    pub scratch: std::path::PathBuf,
    /// The running data directory: copied onto the new disk so nothing is lost.
    pub data_dir: std::path::PathBuf,
}

/// What the WiFi part of the Network page acts on: the programs that scan and join, the saved networks, and the machine's
/// links. Real in production; tests point it at stand-ins.
#[derive(Clone)]
pub struct WifiEnv {
    pub wifi: Arc<iyakki_devctl::wifi::Wifi>,
    pub known: Arc<iyakki_devctl::wifi::KnownStore>,
    pub roots: iyakki_devctl::Roots,
}

impl WifiEnv {
    pub fn real() -> WifiEnv {
        #[cfg(test)]
        {
            // unit tests never scan the real air or touch the real files
            return WifiEnv { wifi: Arc::new(iyakki_devctl::wifi::Wifi { run_dir: "/nonexistent/run".into(), path: Some("/nonexistent/bin".into()) }), known: Arc::new(iyakki_devctl::wifi::KnownStore::new(std::path::Path::new("/nonexistent/wifi.json"))), roots: iyakki_devctl::Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() } };
        }
        #[allow(unreachable_code)]
        WifiEnv { wifi: Arc::new(iyakki_devctl::wifi::Wifi::real()), known: Arc::new(iyakki_devctl::wifi::KnownStore::new(std::path::Path::new("/var/lib/iyakki/wifi.json"))), roots: iyakki_devctl::Roots::real() }
    }

    /// The first WiFi adapter's interface name.
    pub fn iface(&self) -> Option<String> {
        iyakki_devctl::links(&self.roots).into_iter().find(|l| l.kind == iyakki_devctl::LinkKind::Wifi).map(|l| l.name)
    }
}

static VIRTUAL_STORAGE: std::sync::OnceLock<StorageEnv> = std::sync::OnceLock::new();

/// From now on the Storage tab shows only virtual disks kept in `dir` (sparse image files), never the machine's drives.
/// Called once at start by the desktop window; `data_dir` is what making a data disk copies onto it.
pub fn use_virtual_disks(dir: &std::path::Path, data_dir: &std::path::Path, mke2fs: Option<std::path::PathBuf>) -> Result<(), String> {
    let v = iyakki_storage::virt::VirtualDisks::open(dir)?;
    let mke2fs = mke2fs.or_else(|| ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists())).unwrap_or_else(|| "/sbin/mke2fs".into());
    let env = StorageEnv { roots: v.roots(), mounter: Arc::new(v.mounter()), mke2fs, scratch: dir.to_path_buf(), data_dir: data_dir.to_path_buf(), virtual_disks: Some(v) };
    VIRTUAL_STORAGE.set(env).map_err(|_| "virtual disks are already set up".to_string())
}

impl StorageEnv {
    pub fn real() -> StorageEnv {
        #[cfg(test)]
        {
            // unit tests never look at (or touch) the machine's real disks
            return StorageEnv { roots: iyakki_storage::Roots { sys_block: "/nonexistent/sys".into(), dev: "/nonexistent/dev".into(), mounts: "/nonexistent/mounts".into(), require_block: true }, mounter: Arc::new(iyakki_storage::SysMounter), mke2fs: "/nonexistent/mke2fs".into(), scratch: "/nonexistent".into(), data_dir: "/nonexistent/data".into(), virtual_disks: None };
        }
        #[allow(unreachable_code)]
        if let Some(v) = VIRTUAL_STORAGE.get() {
            return v.clone();
        }
        let mke2fs = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists()).unwrap_or_else(|| "/sbin/mke2fs".into());
        StorageEnv { roots: iyakki_storage::Roots::real(), mounter: Arc::new(iyakki_storage::SysMounter), mke2fs, scratch: "/run".into(), data_dir: "/var/lib/iyakki".into(), virtual_disks: None }
    }
}

const SLIDER_CHUNK: usize = 7001;
const SLIDER_OVERLAP: usize = 7002;
const SLIDER_TOPK: usize = 7003;
/// Sliders on the hardware pages are `SLIDER_HW + row`.
const SLIDER_HW: usize = 7100;

pub struct SettingsApp {
    tabs: Vec<Tab>,
    tab: usize,
    sel: [usize; 24],
    /// Network, Bluetooth, Display and Sound pages.
    hw: DevicesPane,
    /// Ticks since the app opened (the hardware pages refresh on a timer).
    ticks: u64,
    /// How long the mic tests listen (milliseconds); shortened by tests.
    pub test_record_ms: u64,
    /// Voice page: the language of the test sentence (index in `VO_LANGS`), and how far the lower part is scrolled.
    vo_lang: usize,
    vo_scroll: f32,
    vo_max: f32,
    /// Vault tab: 0 = system vault, 1 = personal vault.
    section: usize,
    /// May edit the skills shared with everyone (set from the policy at open).
    share_skills: bool,
    /// The search box over the list of pages, whether it has the keyboard, and how far the list is scrolled.
    nav_query: TextField,
    nav_focus: bool,
    nav_scroll: f32,
    /// Where the pointer is and where the list was drawn (the wheel scrolls the list when the pointer is over it).
    mouse: (i32, i32),
    nav_rect: R,
    nav_s: f32,
    /// Width of each slider's track as last drawn, by slider id (a click's x is turned into a value with it).
    slider_w: Vec<(usize, f32)>,
    storage: StorageEnv,
    wifi: WifiEnv,
    /// The networks the last scan found, in the order the picker shows them (`None`: the extra choices below the list).
    wifi_items: Vec<(String, Option<iyakki_devctl::wifi::Ap>)>,
    devices: Vec<iyakki_storage::BlockDev>,
    modal: Option<Modal>,
    job: Option<Job>,
    /// The Host access page's state.
    host: host_access::HostPanel,
    status: Option<(String, bool)>,
    /// Settings → Dev cluster.
    dc: devcluster::Dc,
    /// Settings → Memory.
    mem: memory::MemPanel,
    time: time::TimePanel,
    hn: time::NamePanel,
    /// Settings → Install to disk.
    inst: install_page::InstallPanel,
    /// Settings → Indic voices.
    indic: indic_page::IndicPanel,
    /// Settings → Developer mode.
    devel: developer_page::DevPanel,
}

fn hf_on() -> bool {
    iyakki_agent::hostfiles::enabled()
}

fn kind_labels() -> Vec<&'static str> {
    Kind::ALL.iter().map(|k| k.label()).chain([AUTO_TYPE]).collect()
}

impl SettingsApp {
    pub fn new(cx: &AppCx) -> SettingsApp {
        let admin = cx.session.policy.manage_settings;
        let mut tabs = vec![Tab::Models, Tab::Rag, Tab::Vault];
        if cx.session.policy.skills {
            tabs.push(Tab::Skills);
        }
        if admin {
            tabs.insert(1, Tab::Agents);
        }
        if admin {
            tabs.extend([Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound]);
            tabs.push(Tab::Storage);
            tabs.push(Tab::Memory);
            tabs.push(Tab::HostName);
            tabs.push(Tab::Developer);
        }
        if admin && !hf_on() && install_page::InstallEnv::real().available() {
            tabs.push(Tab::Install);
        }
        if admin || cx.session.policy.voice {
            tabs.push(Tab::Voice);
        }
        if admin {
            tabs.push(Tab::IndicVoices);
        }
        if admin {
            tabs.push(Tab::SecretProviders);
        }
        if Self::dc_visible(cx) {
            tabs.push(Tab::DevCluster);
        }
        tabs.push(Tab::Time);
        tabs.push(Tab::Browser);
        tabs.push(Tab::HostAccess);
        tabs.push(Tab::Account);
        let mut app = SettingsApp { tabs, tab: 0, sel: [0; 24], hw: DevicesPane::new(DeviceEnv::real()), ticks: 0, test_record_ms: 2000, vo_lang: 0, vo_scroll: 0.0, vo_max: 0.0, section: if admin { 0 } else { 1 }, share_skills: cx.session.policy.manage_skills, nav_query: TextField::new("Search settings", false), nav_focus: false, nav_scroll: 0.0, mouse: (0, 0), nav_rect: R::new(0.0, 0.0, 0.0, 0.0), nav_s: 1.0, slider_w: Vec::new(), storage: StorageEnv::real(), wifi: WifiEnv::real(), wifi_items: Vec::new(), devices: vec![], modal: None, job: None, host: Default::default(), status: None, dc: Default::default(), mem: Default::default(), time: Default::default(), hn: Default::default(), inst: Default::default(), indic: Default::default(), devel: Default::default() };
        app.refresh_devices();
        app
    }

    /// Replace the machine the hardware pages look at (tests).
    pub fn with_hardware(mut self, env: DeviceEnv) -> SettingsApp {
        self.hw = DevicesPane::new(env);
        self
    }

    /// Replace what the private MAC address setting reads and changes (tests).
    pub fn with_mac(mut self, mac: iyakki_devctl::mac::MacEnv) -> SettingsApp {
        self.hw.set_mac(mac);
        self
    }

    /// Replace what the WiFi part acts on (tests).
    pub fn with_wifi(mut self, env: WifiEnv) -> SettingsApp {
        self.wifi = env;
        self
    }

    /// Replace the storage environment (tests).
    pub fn with_storage(mut self, env: StorageEnv) -> SettingsApp {
        self.storage = env;
        self.refresh_devices();
        self
    }

    fn refresh_devices(&mut self) {
        if self.tabs.contains(&Tab::Storage) {
            self.devices = iyakki_storage::list(&self.storage.roots);
            self.sel[self.tabs.iter().position(|t| *t == Tab::Storage).unwrap_or(0)] = self.sel[self.tabs.iter().position(|t| *t == Tab::Storage).unwrap_or(0)].min(self.devices.len().saturating_sub(1));
        }
    }

    /// A click on a slider's track at `x` pixels from its left edge.
    fn slider_click(&mut self, id: usize, x: f32, cx: &AppCx) {
        let Some(w) = self.slider_w.iter().find(|(i, _)| *i == id).map(|(_, w)| *w) else { return };
        match id {
            SLIDER_CHUNK | SLIDER_OVERLAP | SLIDER_TOPK if self.admin(cx) => {
                let rag = Self::svc(cx).settings.lock().unwrap().get().rag.clone();
                let result = Self::svc(cx).settings.lock().unwrap().update(|st| match id {
                    SLIDER_CHUNK => {
                        st.rag.chunk_max = controls::value_at(x, w, 100, 4000, 100) as usize;
                        // the overlap must stay under half the chunk size: bring it down with it
                        let top = (st.rag.chunk_max / 2).saturating_sub(1);
                        st.rag.overlap = st.rag.overlap.min(top) / 20 * 20;
                    }
                    SLIDER_OVERLAP => st.rag.overlap = controls::value_at(x, w, 0, (rag.chunk_max / 2).saturating_sub(1) as i64, 20) as usize,
                    _ => st.rag.top_k = controls::value_at(x, w, 1, 50, 1) as usize,
                });
                match result {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "settings.rag", "");
                        self.ok("Saved.");
                    }
                    Err(e) => self.fail(e.to_string()),
                }
            }
            id if id >= SLIDER_HW => self.hw_slider(id, x, w, cx),
            _ => {}
        }
    }

    fn nav_items(&self) -> Vec<nav::Item> {
        self.tabs.iter().enumerate().map(|(i, t)| nav::Item { tab: i, label: t.label(), group: t.group(), keywords: t.keywords() }).collect()
    }

    /// The pages the search box lets through.
    pub fn nav_matches(&self) -> Vec<usize> {
        nav::entries(&self.nav_items(), &self.nav_query.value).iter().filter_map(|e| if let nav::Entry::Item { tab, .. } = e { Some(*tab) } else { None }).collect()
    }

    /// A key while the search box has the keyboard. Returns whether it was used.
    fn nav_key(&mut self, k: &Key) -> bool {
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        match k {
            Key::Esc => {
                if self.nav_query.value.is_empty() {
                    self.nav_focus = false;
                } else {
                    self.nav_query.set("");
                }
            }
            Key::Enter => {
                if let Some(t) = nav::first_tab(&entries) {
                    self.tab = t;
                }
                self.nav_focus = false;
            }
            Key::Down | Key::Up => {
                if let Some(t) = nav::step_tab(&entries, self.tab, if *k == Key::Down { 1 } else { -1 }) {
                    self.tab = t;
                }
            }
            Key::Tab => self.nav_focus = false,
            other => {
                self.nav_query.handle(other);
                self.nav_after_edit();
            }
        }
        self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &nav::entries(&self.nav_items(), &self.nav_query.value), self.tab, self.nav_rect.h, self.nav_s);
        true
    }

    /// The search text changed: keep the shown page among the matches (the first match becomes the page).
    fn nav_after_edit(&mut self) {
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        if !entries.iter().any(|e| matches!(e, nav::Entry::Item { tab, .. } if *tab == self.tab)) {
            if let Some(t) = nav::first_tab(&entries) {
                self.tab = t;
            }
        }
        self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &entries, self.tab, self.nav_rect.h, self.nav_s);
    }

    pub fn tab(&self) -> Tab {
        self.tabs[self.tab]
    }
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }
    fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    fn admin(&mut self, cx: &AppCx) -> bool {
        if cx.session.policy.manage_settings {
            return true;
        }
        cx.audit.record(&cx.session.user, "settings.denied", self.tab().label());
        self.fail("Only administrators can change this.");
        false
    }

    fn svc(cx: &AppCx) -> &Arc<Services> {
        &cx.services
    }

    fn sel(&self) -> usize {
        self.sel[self.tab]
    }

    fn move_sel(&mut self, delta: i32, len: usize) {
        let i = &mut self.sel[self.tab];
        *i = (*i as i32 + delta).clamp(0, len.saturating_sub(1) as i32) as usize;
    }

    // ------------------------------------------------------------------ data helpers

    fn providers(cx: &AppCx) -> Vec<ProviderCfg> {
        Self::svc(cx).settings.lock().unwrap().get().providers.clone()
    }

    fn externals(cx: &AppCx) -> Vec<ExtEntry> {
        Self::svc(cx).settings.lock().unwrap().get().external_rag.clone()
    }

    fn secret_providers(cx: &AppCx) -> Vec<(String, SecretProvider)> {
        Self::svc(cx).settings.lock().unwrap().get().secret_providers.clone().into_iter().collect()
    }

    fn secret_names(&self, cx: &AppCx) -> Result<Vec<String>, VaultError> {
        if self.section == 0 && cx.session.policy.manage_settings {
            Self::svc(cx).system_names()
        } else {
            Self::svc(cx).user_names(&cx.session.user)
        }
    }

    fn store_key(&mut self, cx: &AppCx, name: &str, value: &str, system: bool) -> Result<(), String> {
        let r = if system { Self::svc(cx).system_set(name, value) } else { Self::svc(cx).user_set(&cx.session.user, name, value) };
        r.map_err(|e| match e {
            VaultError::Locked if system => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            VaultError::Locked => "Your personal vault is locked (it could not be opened at sign-in).".to_string(),
            other => other.to_string(),
        })?;
        cx.audit.record(&cx.session.user, if system { "vault.system.set" } else { "vault.user.set" }, name);
        Ok(())
    }

    // ------------------------------------------------------------------ jobs

    fn start(&mut self, label: &str, f: impl FnOnce() -> JobOut + Send + 'static) {
        if self.job.is_some() {
            self.fail("Another operation is still running.");
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.job = Some(Job { label: label.into(), rx });
    }

    fn poll(&mut self, cx: &AppCx) {
        let Some(job) = &self.job else { return };
        let Ok(out) = job.rx.try_recv() else { return };
        self.job = None;
        match out {
            JobOut::Msg(Ok(m)) => {
                self.ok(m);
                self.mem_after_job();
                self.refresh_devices();
            }
            JobOut::Dc(out) => self.dc_job_done(cx, out),
            JobOut::Wifi(Err(e)) => self.fail(e),
            JobOut::Wifi(Ok(aps)) => self.show_wifi_picker(aps),
            JobOut::Msg(Err(e)) => {
                self.fail(e);
                self.mem_after_job();
                self.refresh_devices();
            }
            JobOut::Models { provider, models, ms } => {
                let n = models.len();
                let r = Self::svc(cx).settings.lock().unwrap().update(|s| {
                    if let Some(p) = s.providers.iter_mut().find(|p| p.id == provider) {
                        p.models = models;
                    }
                });
                match r {
                    Ok(()) => self.ok(format!("{provider}: connected in {ms} ms, {n} model(s) available")),
                    Err(e) => self.fail(e.to_string()),
                }
            }
        }
    }

    fn test_provider(&mut self, cx: &AppCx, id: &str) {
        let svc = Self::svc(cx).clone();
        let id = id.to_string();
        match svc.client_for(&id, true) {
            Err(e) => self.fail(e),
            Ok(_) => self.start(&format!("testing {id}"), move || match svc.client_for(&id, true).and_then(|c| c.test()) {
                Ok(r) => {
                    // a Lemonade server's typed list is kept too (pictures, voices, embeddings)
                    if svc.client_for(&id, true).is_ok_and(|c| c.cfg.kind == Kind::Lemonade) {
                        let _ = svc.refresh_lemonade(&id);
                    }
                    JobOut::Models { provider: id, models: r.models, ms: r.latency_ms }
                }
                Err(e) => JobOut::Msg(Err(format!("{id}: {e}"))),
            }),
        }
    }

    fn test_chat(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let Some(choice) = svc.settings.lock().unwrap().get().chat.clone() else {
            self.fail("Choose a chat model first (press m on a provider).");
            return;
        };
        self.start("asking the model", move || {
            let r = svc.client_for(&choice.provider, false).and_then(|c| c.chat(&choice.model, &[iyakki_agent::chat::Message::user("Reply with the single word: OK")]));
            JobOut::Msg(match r {
                Ok(t) => Ok(format!("{} · {} replied: {}", choice.provider, choice.model, t.chars().take(80).collect::<String>())),
                Err(e) => Err(format!("{}: {e}", choice.provider)),
            })
        });
    }

    // ------------------------------------------------------------------ forms

    fn provider_form(cx: &AppCx, editing: Option<&ProviderCfg>) -> Form {
        let kind = editing.map_or(0, |p| Kind::ALL.iter().position(|k| *k == p.kind).unwrap_or(0));
        let default_url = editing.and_then(|p| p.base_url.clone()).or_else(|| Kind::ALL[kind].default_base().map(str::to_string)).unwrap_or_default();
        let _ = cx;
        Form::new(if editing.is_some() { "Edit model provider" } else { "Add model provider" })
            .text("Name (lowercase id)", editing.map_or("", |p| p.id.as_str()), "e.g. local-ollama, openai, claude")
            .choice("Type", &kind_labels(), kind)
            .text("Base URL", &default_url, "http://localhost:11434")
            .secret("API key", if editing.is_some() { "leave empty to keep the current key" } else { "stored in the system vault, never in settings" })
            .text("Default model (optional)", editing.and_then(|p| p.default_model.as_deref()).unwrap_or(""), "pick one later with m")
    }

    fn save_provider(&mut self, cx: &AppCx, form: &Form, editing: Option<String>) -> Result<(), String> {
        let id = form.value(0);
        let Some(&kind) = Kind::ALL.get(form.choice_idx(1)) else { return self.save_provider_auto(cx, form, editing) };
        let url = form.value(2);
        let key = form.raw(3);
        let model = form.value(4);
        let existing = Self::providers(cx).into_iter().find(|p| Some(&p.id) == editing.as_ref());
        if editing.is_none() && Self::providers(cx).iter().any(|p| p.id == id) {
            return Err(format!("A provider named `{id}` already exists."));
        }
        let mut cfg = existing.clone().unwrap_or_else(|| ProviderCfg::new(&id, kind));
        if editing.is_none() {
            cfg.id = id.clone();
        }
        cfg.kind = kind;
        cfg.base_url = if url.is_empty() { None } else { Some(url) };
        cfg.default_model = if model.is_empty() { None } else { Some(model) };
        let key_name = format!("{}-key", cfg.id);
        if !key.is_empty() {
            cfg.key_ref = Some(format!("vault:{key_name}"));
        } else if existing.is_none() && !kind.needs_key() {
            cfg.key_ref = None;
        }
        cfg.validate()?; // before touching the vault: a rejected form stores nothing
        if !key.is_empty() {
            self.store_key(cx, &key_name, &key, true)?;
        }
        Self::svc(cx).settings.lock().unwrap().upsert_provider(cfg.clone()).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, "settings.provider.save", &format!("{} ({})", cfg.id, cfg.kind.label()));
        Ok(())
    }

    fn external_form(editing: Option<&ExtEntry>) -> Form {
        let (kind, id, url, coll, cred) = match editing.map(|e| &e.backend) {
            Some(ExternalRag::Qdrant { id, url, collection, key_ref }) => (0, id.as_str(), url.as_str(), collection.as_str(), key_ref.is_some()),
            Some(ExternalRag::Chroma { id, url, collection, token_ref, .. }) => (1, id.as_str(), url.as_str(), collection.as_str(), token_ref.is_some()),
            Some(ExternalRag::Http { id, url, key_ref }) => (2, id.as_str(), url.as_str(), "", key_ref.is_some()),
            None => (0, "", "", "", false),
        };
        Form::new(if editing.is_some() { "Edit external RAG" } else { "Add external RAG" })
            .text("Name (lowercase id)", id, "e.g. team-qdrant")
            .choice("Type", &["Qdrant", "Chroma", "HTTP search endpoint"], kind)
            .text("URL", url, "http://host:6333  /  http://host:8000  /  https://search.corp/api")
            .text("Collection (Qdrant / Chroma)", coll, "collection name")
            .secret("API key / token (optional)", if cred { "leave empty to keep the current credential" } else { "stored in the system vault" })
    }

    fn save_external(&mut self, cx: &AppCx, form: &Form, editing: Option<String>) -> Result<(), String> {
        let (id, url, coll, key) = (form.value(0), form.value(2), form.value(3), form.raw(4));
        let prev = Self::externals(cx).into_iter().find(|e| Some(e.backend.id()) == editing.as_deref());
        if editing.is_none() && Self::externals(cx).iter().any(|e| e.backend.id() == id) {
            return Err(format!("A backend named `{id}` already exists."));
        }
        let key_name = format!("{id}-key");
        let prev_ref = prev.as_ref().and_then(|p| match &p.backend {
            ExternalRag::Qdrant { key_ref, .. } | ExternalRag::Http { key_ref, .. } => key_ref.clone(),
            ExternalRag::Chroma { token_ref, .. } => token_ref.clone(),
        });
        let cred = if !key.is_empty() { Some(format!("vault:{key_name}")) } else { prev_ref };
        let backend = match form.choice_idx(1) {
            0 => ExternalRag::Qdrant { id: id.clone(), url, collection: coll, key_ref: cred },
            1 => ExternalRag::Chroma { id: id.clone(), url, tenant: "default_tenant".into(), database: "default_database".into(), collection: coll, token_ref: cred },
            _ => ExternalRag::Http { id: id.clone(), url, key_ref: cred },
        };
        backend.validate()?;
        if !key.is_empty() {
            self.store_key(cx, &key_name, &key, true)?;
        }
        Self::svc(cx).settings.lock().unwrap().upsert_external(ExtEntry { enabled: prev.map_or(true, |p| p.enabled), backend }).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, "settings.external.save", &id);
        Ok(())
    }

    fn secret_provider_form() -> Form {
        Form::new("Add external secret provider")
            .text("Alias (lowercase id)", "", "e.g. corp-vault — used as ext:corp-vault/path#field")
            .choice("Type", &["HashiCorp Vault / OpenBao (KV v2)", "1Password Connect"], 0)
            .text("Address", "", "https://vault.example.com:8200")
            .secret("Access token", "stored in the system vault")
            .text("KV mount (HashiCorp)", "secret", "secret")
            .text("Namespace (HashiCorp, optional)", "", "")
    }

    fn save_secret_provider(&mut self, cx: &AppCx, form: &Form) -> Result<(), String> {
        let (alias, addr, token, mount, ns) = (form.value(0), form.value(2), form.raw(3), form.value(4), form.value(5));
        if !valid_provider_id(&alias) {
            return Err("Alias: lowercase letters, digits, - and _; must start with a letter.".into());
        }
        if token.is_empty() {
            return Err("An access token is required.".into());
        }
        let key_name = format!("{alias}-token");
        let token_ref = format!("vault:{key_name}");
        let cfg = if form.choice_idx(1) == 0 {
            SecretProvider::Hashicorp { addr, mount: if mount.is_empty() { "secret".into() } else { mount }, namespace: if ns.is_empty() { None } else { Some(ns) }, token_ref }
        } else {
            SecretProvider::Onepassword { addr, token_ref }
        };
        // validate the combination before storing the token
        let mut probe = Self::svc(cx).settings.lock().unwrap().get().clone();
        probe.secret_providers.insert(alias.clone(), cfg.clone());
        probe.validate().map_err(|e| e.to_string())?;
        self.store_key(cx, &key_name, &token, true)?;
        Self::svc(cx).settings.lock().unwrap().upsert_secret_provider(&alias, cfg).map_err(|e| e.to_string())?;
        Self::svc(cx).refresh_secret_providers();
        cx.audit.record(&cx.session.user, "settings.secret_provider.save", &alias);
        Ok(())
    }

    // ------------------------------------------------------------------ modal handling

    fn handle_modal(&mut self, ev: &Event, cx: &AppCx) {
        let Some(modal) = self.modal.take() else { return };
        match modal {
            Modal::Form(kind, mut form) => match form.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Changed(1) if matches!(kind, FormKind::Provider(_)) => {
                    // switching type refreshes the URL default unless the user typed their own
                    let old_default: Vec<String> = Kind::ALL.iter().map(|k| k.default_base().unwrap_or("").to_string()).collect();
                    if let Some(k) = Kind::ALL.get(form.choice_idx(1)).filter(|_| old_default.contains(&form.value(2)) || form.value(2).is_empty()) {
                        form.set_value(2, k.default_base().unwrap_or(""));
                    }
                    self.modal = Some(Modal::Form(kind, form));
                }
                FormEvent::Submit => {
                    if let Some(next) = self.submit_form(cx, kind.clone(), &mut form) {
                        self.modal = Some(next);
                    }
                }
                _ => self.modal = Some(Modal::Form(kind, form)),
            },
            Modal::Pick(kind, mut picker) => match picker.handle_event(ev) {
                PickEvent::Cancel => {}
                PickEvent::Chosen(item) => self.apply_pick(cx, kind, &item),
                PickEvent::None => self.modal = Some(Modal::Pick(kind, picker)),
            },
            Modal::Confirm(kind, text) => match ev {
                Event::Key(Key::Char('y')) => self.apply_confirm(cx, kind),
                // Esc or any letter cancels; other input (clicks on the backdrop, arrows) keeps it open
                Event::Key(Key::Esc) | Event::Key(Key::Char(_)) => {}
                _ => self.modal = Some(Modal::Confirm(kind, text)),
            },
        }
    }

    /// Returns a modal to keep open (the form again, with an error) when saving failed.
    fn submit_form(&mut self, cx: &AppCx, kind: FormKind, form: &mut Form) -> Option<Modal> {
        let result: Result<String, String> = match &kind {
            FormKind::Provider(editing) => {
                if !self.admin(cx) {
                    return None;
                }
                self.save_provider(cx, form, editing.clone()).map(|_| format!("Saved provider `{}`.", if let Some(e) = editing { e.clone() } else { form.value(0) }))
            }
            FormKind::ProviderKey(id) => {
                if !self.admin(cx) {
                    return None;
                }
                let key = form.raw(0);
                if key.is_empty() {
                    Err("Enter the key.".into())
                } else {
                    let name = format!("{id}-key");
                    self.store_key(cx, &name, &key, true).and_then(|_| {
                        let id2 = id.clone();
                        Self::svc(cx)
                            .settings
                            .lock()
                            .unwrap()
                            .update(|s| {
                                if let Some(p) = s.providers.iter_mut().find(|p| p.id == id2) {
                                    p.key_ref = Some(format!("vault:{name}"));
                                }
                            })
                            .map_err(|e| e.to_string())
                    })
                    .map(|_| format!("Key for `{id}` stored in the system vault."))
                }
            }
            FormKind::External(editing) => {
                if !self.admin(cx) {
                    return None;
                }
                self.save_external(cx, form, editing.clone()).map(|_| "Saved external RAG backend.".to_string())
            }
            FormKind::NewSecret(system) => {
                let (name, value) = (form.value(0), form.raw(1));
                if *system && !self.admin(cx) {
                    return None;
                }
                if !valid_secret_name(&name) {
                    Err("Name: lowercase letters, digits, . _ - (max 64).".into())
                } else if value.is_empty() {
                    Err("Enter the secret value.".into())
                } else {
                    self.store_key(cx, &name, &value, *system).map(|_| format!("Stored `{name}`."))
                }
            }
            FormKind::Keyfile => {
                if !self.admin(cx) {
                    return None;
                }
                let path = form.value(0);
                let r = Self::svc(cx).enable_keyfile(&path);
                if r.is_ok() {
                    cx.audit.record(&cx.session.user, "vault.keyfile.enable", &path);
                }
                r.map(|_| format!("Key file created at {path}; the system vault will open at boot."))
            }
            FormKind::SecretProvider => {
                if !self.admin(cx) {
                    return None;
                }
                self.save_secret_provider(cx, form).map(|_| "Saved secret provider.".to_string())
            }
            FormKind::VoiceStt | FormKind::VoiceTts => {
                if !self.admin(cx) {
                    return None;
                }
                let stt = kind == FormKind::VoiceStt;
                self.save_speech_engine(cx, form, stt).map(|_| format!("Saved the {} engine.", if stt { "speech-to-text" } else { "text-to-speech" }))
            }
            FormKind::VoiceCmds => {
                if !self.admin(cx) {
                    return None;
                }
                let (rec, play) = (form.value(0), form.value(1));
                Self::svc(cx).settings.lock().unwrap().update(|st| {
                    st.voice.record_cmd = rec;
                    st.voice.play_cmd = play;
                }).map(|_| {
                    cx.audit.record(&cx.session.user, "settings.voice.commands", "");
                    "Saved the microphone and speaker commands.".to_string()
                }).map_err(|e| e.to_string())
            }
            FormKind::Persist(dev, needs_force) => {
                if !self.admin(cx) {
                    return None;
                }
                let typed = form.value(0);
                let erase_ok = !*needs_force || form.value(1).trim() == "ERASE";
                if typed.trim().trim_start_matches("/dev/") != dev.as_str() {
                    Err(format!("Type {dev} exactly to confirm."))
                } else if !erase_ok {
                    Err("This disk holds data. Type ERASE (capitals) to replace it.".to_string())
                } else {
                    let (env, dev, force) = (self.storage.clone(), dev.clone(), *needs_force);
                    cx.audit.record(&cx.session.user, "storage.persist", &dev);
                    self.start("preparing the data disk", move || {
                        JobOut::Msg(iyakki_storage::persist(&env.roots, env.mounter.as_ref(), &env.mke2fs, &env.scratch, &dev, force, Some(&env.data_dir)).map(|r| format!("{} is now the IYAKKI data disk ({} files copied). Reboot to start using it.", r.device, r.files_copied)))
                    });
                    Ok("Working… this takes a few seconds.".to_string())
                }
            }
            FormKind::WifiJoin(ssid, security) => {
                if !self.admin(cx) {
                    return None;
                }
                match iyakki_devctl::wifi::Known::new(ssid, &form.raw(0), *security, false, iyakki_agent::clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
            }
            FormKind::WifiHidden => {
                if !self.admin(cx) {
                    return None;
                }
                let (name, pass) = (form.value(0), form.raw(1));
                let sec = if pass.is_empty() { iyakki_devctl::wifi::Security::Open } else { iyakki_devctl::wifi::Security::Wpa2 };
                match iyakki_devctl::wifi::Known::new(name.trim(), &pass, sec, true, iyakki_agent::clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
            }
            FormKind::NewVirtualDisk => {
                if !self.admin(cx) {
                    return None;
                }
                let Some(v) = self.storage.virtual_disks.clone() else { return None };
                match iyakki_storage::virt::parse_size(&form.value(0)) {
                    None => Err("Write a size such as 8G or 512M.".to_string()),
                    Some(bytes) => v.add(bytes).map(|name| {
                        cx.audit.record(&cx.session.user, "storage.virtual.add", &name);
                        self.refresh_devices();
                        format!("Made the virtual disk {name} ({}).", iyakki_storage::human(bytes))
                    }),
                }
            }
            FormKind::RemoveVirtualDisk(name) => {
                if !self.admin(cx) {
                    return None;
                }
                let Some(v) = self.storage.virtual_disks.clone() else { return None };
                if form.value(0).trim().trim_start_matches("/dev/") != name.as_str() {
                    Err(format!("Type {name} exactly to confirm."))
                } else {
                    v.remove(name).map(|_| {
                        cx.audit.record(&cx.session.user, "storage.virtual.remove", name);
                        self.refresh_devices();
                        format!("Removed {name}.")
                    })
                }
            }
            FormKind::TokenPrice => {
                if !self.admin(cx) {
                    return None;
                }
                match form.value(0).trim().parse::<u32>() {
                    Err(_) => Err("enter a whole number of cents (0 or more)".to_string()),
                    Ok(v) => Self::svc(cx).settings.lock().unwrap().update(|s| s.token_price_cents_per_mtok = v).map(|_| {
                        cx.audit.record(&cx.session.user, "settings.token_price", &v.to_string());
                        format!("Agent token price: {v} cents per million tokens.")
                    }).map_err(|e| e.to_string()),
                }
            }
            FormKind::Dc(f) => self.dc_submit(cx, &f, form),
            FormKind::Mem(f) => self.mem_submit(*f, form),
            FormKind::TimeServer => self.time_submit(cx, form),
            FormKind::HostName => self.host_name_submit(cx, form),
            FormKind::Install(d) => self.inst_submit(cx, d, form),
            FormKind::IndicFolder(_) | FormKind::IndicSource | FormKind::IndicServer => self.indic_submit(cx, &kind, form),
            FormKind::AgentModel(_) | FormKind::AgentCustom(_) => match self.agents_submit(cx, &kind, form) {
                Some(r) => r,
                None => return None,
            },
            FormKind::NewSkill => {
                let md = format!("---\nname: {}\ndescription: {}\ntriggers: {}\n---\n{}\n", form.value(0), form.value(1), form.value(2), form.value(3));
                self.add_skill(cx, &md, form.choice_idx(4) == 1)
            }
            FormKind::ImportSkill => {
                let path = form.value(0);
                let shared = form.choice_idx(1) == 1;
                Self::read_skill_file(cx, &path).and_then(|md| self.add_skill(cx, &md, shared))
            }
            FormKind::Password => {
                let (old, new, again) = (form.raw(0), form.raw(1), form.raw(2));
                if new != again {
                    Err("The new passwords don't match.".into())
                } else {
                    let user = cx.session.user.clone();
                    let r = cx.store.lock().map_err(|_| "store busy".to_string()).and_then(|mut s| s.set_password(&cx.session, &user, Some(&old), &new).map_err(|e| match e {
                        AccountError::Forbidden => "The current password is wrong.".to_string(),
                        other => other.to_string(),
                    }));
                    if r.is_ok() {
                        Self::svc(cx).on_password_change(&user, &old, &new);
                        cx.audit.record(&user, "account.password", "");
                    }
                    r.map(|_| "Password changed. Your personal vault keeps working with the new one.".to_string())
                }
            }
        };
        match result {
            Ok(m) => {
                self.ok(m);
                None
            }
            Err(e) => {
                form.error = Some(e);
                Some(Modal::Form(kind, form.clone()))
            }
        }
    }

    fn apply_pick(&mut self, cx: &AppCx, kind: PickKind, item: &str) {
        if !self.admin(cx) {
            return;
        }
        if kind == PickKind::Zone {
            return self.time_pick(cx, item);
        }
        if self.agents_pick(cx, &kind, item) {
            return;
        }
        if kind == PickKind::SoundOutput {
            return self.sound_pick(cx, item);
        }
        if let PickKind::Lemonade(p) = kind {
            return self.lem_pick(cx, p, item);
        }
        match kind {
            PickKind::Wifi => {
                let chosen = self.wifi_items.iter().find(|(t, _)| t == item).cloned();
                match chosen {
                    Some((_, Some(ap))) => {
                        if !ap.security.joinable() {
                            return self.fail(format!("{} networks ({}) cannot be joined from here.", ap.security.label(), ap.ssid));
                        }
                        if ap.security == iyakki_devctl::wifi::Security::Open {
                            match iyakki_devctl::wifi::Known::new(&ap.ssid, "", ap.security, false, iyakki_agent::clock::now_unix()) {
                                Ok(k) => self.join_wifi(cx, k),
                                Err(e) => self.fail(e),
                            }
                        } else {
                            let f = Form::new(&format!("Join {} ({})", ap.ssid, ap.security.label())).secret("Passphrase", "8 to 63 characters; only the key made from it is saved");
                            self.open_form(FormKind::WifiJoin(ap.ssid.clone(), ap.security), f);
                        }
                    }
                    Some((t, None)) if t.starts_with("Hidden") => {
                        let f = Form::new("Join a hidden WiFi network").text("Network name", "", "exactly as it is spelled").secret("Passphrase", "empty if the network is open");
                        self.open_form(FormKind::WifiHidden, f);
                    }
                    Some((t, None)) if t.starts_with("Forget") => {
                        let names: Vec<String> = self.wifi.known.list().into_iter().map(|k| k.ssid).collect();
                        self.modal = Some(Modal::Pick(PickKind::WifiForget, Picker::new("Forget which network?", names)));
                    }
                    _ => {}
                }
                return;
            }
            PickKind::WifiForget => {
                match self.wifi.known.forget(item) {
                    Ok(true) => {
                        cx.audit.record(&cx.session.user, "settings.wifi.forget", item);
                        if let Some(iface) = self.wifi.iface() {
                            let _ = self.wifi.wifi.apply(&iface, &self.wifi.known.list());
                        }
                        self.ok(format!("Forgot {item}."));
                    }
                    Ok(false) => self.fail(format!("{item} was not saved.")),
                    Err(e) => self.fail(e),
                }
                return;
            }
            PickKind::MacMode(k) => {
                let Some(mode) = iyakki_devctl::mac::MacMode::ALL.into_iter().find(|m| m.label() == item) else { return };
                let mac = self.hw.mac().clone();
                let name = if k == iyakki_devctl::LinkKind::Wifi { "WiFi" } else { "Ethernet" };
                match mac.set_mode(Some(k), None, mode) {
                    Ok(_) => {
                        cx.audit.record(&cx.session.user, "settings.network.mac", &format!("{} {}", name.to_lowercase(), mode.key()));
                        self.hw.invalidate();
                        self.ok(format!("Private MAC address for {name}: {}. It applies on the next connection; press a to apply it now.", mode.key()));
                    }
                    Err(e) => self.fail(e),
                }
                return;
            }
            _ => {}
        }
        let (r, what, provider) = match &kind {
            PickKind::Zone => return,
            PickKind::Chat(p) => (
                Self::svc(cx).settings.lock().unwrap().update(|s| {
                    s.chat = Some(ModelChoice { provider: p.clone(), model: item.into() });
                    if let Some(pr) = s.providers.iter_mut().find(|x| x.id == *p) {
                        pr.default_model = Some(item.into());
                    }
                }),
                "chat",
                p,
            ),
            PickKind::Wifi | PickKind::WifiForget | PickKind::AgentKey(_) | PickKind::AgentModel(_) | PickKind::MacMode(_) | PickKind::Lemonade(_) | PickKind::SoundOutput => return,
            PickKind::Embed(p) => {
                let choice = if item.starts_with("(built-in") { None } else { Some(ModelChoice { provider: p.clone(), model: item.into() }) };
                (Self::svc(cx).settings.lock().unwrap().set_embedding(choice), "embedding", p)
            }
        };
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, &format!("settings.{what}_model"), &format!("{provider}/{item}"));
                let extra = if what == "embedding" && Self::svc(cx).reindex_stale_in_background() { " Existing documents are being re-embedded in the background." } else { "" };
                self.ok(format!("{what} model: {provider} · {item}.{extra}"));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    fn apply_confirm(&mut self, cx: &AppCx, kind: ConfirmKind) {
        if let ConfirmKind::Dc(a) = kind {
            return self.dc_confirm(cx, a);
        }
        if let ConfirmKind::MacApply(iface) = kind {
            return self.mac_apply_now(cx, iface);
        }
        if let ConfirmKind::Indic(act) = kind {
            return self.indic_confirm(cx, act);
        }
        if let ConfirmKind::Lemonade(act) = kind {
            return self.lem_confirm(cx, act);
        }
        if kind == ConfirmKind::MemApply {
            if self.admin(cx) && self.mem.writable {
                self.mem_apply(cx);
            }
            return;
        }
        if self.admin(cx) && self.agents_confirm(cx, &kind) {
            return;
        }
        let r: Result<String, String> = match &kind {
            ConfirmKind::Provider(id) if self.admin(cx) => Self::svc(cx).settings.lock().unwrap().remove_provider(id).map(|_| format!("Removed provider `{id}`.")).map_err(|e| e.to_string()),
            ConfirmKind::External(id) if self.admin(cx) => Self::svc(cx).settings.lock().unwrap().remove_external(id).map(|_| format!("Removed `{id}`.")).map_err(|e| e.to_string()),
            ConfirmKind::SecretProvider(a) if self.admin(cx) => {
                let r = Self::svc(cx).settings.lock().unwrap().remove_secret_provider(a).map(|_| format!("Removed `{a}`.")).map_err(|e| e.to_string());
                Self::svc(cx).refresh_secret_providers();
                r
            }
            ConfirmKind::Secret(system, name) => {
                if *system && !self.admin(cx) {
                    return;
                }
                let r = if *system { Self::svc(cx).system_delete(name) } else { Self::svc(cx).user_delete(&cx.session.user, name) };
                r.map(|_| format!("Deleted secret `{name}`.")).map_err(|e| e.to_string())
            }
            ConfirmKind::Reboot if self.admin(cx) && cx.session.policy.power => {
                cx.audit.record(&cx.session.user, "power.reboot", "storage");
                cx.host.reboot();
                Ok("Restarting…".to_string())
            }
            ConfirmKind::Skill(scope, name) => {
                let p = &cx.session.policy;
                Self::svc(cx).skills.remove(scope, name, p.manage_skills, p.skills).map(|_| {
                    cx.audit.record(&cx.session.user, "skills.delete", &format!("{}:{name}", scope.label()));
                    format!("Deleted skill `{name}`.")
                }).map_err(|e| e.to_string())
            }
            ConfirmKind::ResetPersonal => Self::svc(cx).reset_user_vault(&cx.session.user).map(|_| "Personal vault removed; a new empty one is created at your next sign-in.".to_string()),
            _ => return,
        };
        match r {
            Ok(m) => {
                cx.audit.record(&cx.session.user, "settings.delete", &m);
                self.ok(m);
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------ per-tab keys

    // ------------------------------------------------------------------ voice

    fn engine_form(cx: &AppCx, stt: bool) -> Form {
        let cur = {
            let st = Self::svc(cx).settings.lock().unwrap();
            if stt { st.get().voice.stt.clone() } else { st.get().voice.tts.clone() }
        };
        let (kind, target, model, voice_name, lang, has_key) = match &cur {
            Some(SpeechEngine::Http { url, model, voice, language, key_ref }) => (0, url.clone(), model.clone(), voice.clone().unwrap_or_default(), language.clone().unwrap_or_default(), key_ref.is_some()),
            Some(SpeechEngine::Command { command }) => (1, command.clone(), String::new(), String::new(), String::new(), false),
            Some(SpeechEngine::Browser) => (2, String::new(), String::new(), String::new(), String::new(), false),
            // an engine of this computer is chosen on the page itself (Engine, Voice); here it is a command that runs it
            Some(SpeechEngine::Host { .. }) => (0, "http://127.0.0.1:8000/v1/audio/speech".into(), String::new(), String::new(), String::new(), false),
            None => (0, if stt { "http://127.0.0.1:8080/v1/audio/transcriptions".into() } else { "http://127.0.0.1:8000/v1/audio/speech".into() }, String::new(), String::new(), String::new(), false),
        };
        let hint = if stt { "whisper-cli -m model.bin -f {wav} -nt -np" } else { "piper -m voice.onnx -f {out}" };
        let f = Form::new(if stt { "Speech-to-text engine" } else { "Text-to-speech engine" })
            .choice("Type", if stt { &["HTTP endpoint (OpenAI-compatible)", "Local command"][..] } else { &["HTTP endpoint (OpenAI-compatible)", "Local command", BROWSER_CHOICE][..] }, if stt { kind.min(1) } else { kind })
            .text("URL or command", &target, hint)
            .text("Model (HTTP only)", &model, if stt { "whisper-1, tiny.en, …" } else { "tts-1, kokoro, …" })
            .secret("API key (optional, HTTP only)", if has_key { "leave empty to keep the current key" } else { "stored in the system vault" });
        if stt { f.text("Language hint (optional)", &lang, "en") } else { f.text("Voice (optional)", &voice_name, "alloy, nova, …") }
    }

    fn save_speech_engine(&mut self, cx: &AppCx, form: &Form, stt: bool) -> Result<(), String> {
        let (target, model, key, extra) = (form.value(1), form.value(2), form.raw(3), form.value(4));
        let key_name = if stt { "stt-key" } else { "tts-key" };
        let prev_key = {
            let st = Self::svc(cx).settings.lock().unwrap();
            match if stt { &st.get().voice.stt } else { &st.get().voice.tts } {
                Some(SpeechEngine::Http { key_ref, .. }) => key_ref.clone(),
                _ => None,
            }
        };
        if !stt && form.choice_idx(0) == 2 {
            // nothing to configure: the voices come from the browser or window in use
            Self::svc(cx).settings.lock().unwrap().update(|st| st.voice.tts = Some(SpeechEngine::Browser)).map_err(|e| e.to_string())?;
            cx.audit.record(&cx.session.user, "settings.voice.tts", "browser");
            return Ok(());
        }
        let engine = if form.choice_idx(0) == 0 {
            let key_ref = if !key.is_empty() { Some(format!("vault:{key_name}")) } else { prev_key };
            let opt = |s: String| if s.is_empty() { None } else { Some(s) };
            SpeechEngine::Http { url: target, model, key_ref, voice: if stt { None } else { opt(extra.clone()) }, language: if stt { opt(extra) } else { None } }
        } else {
            SpeechEngine::Command { command: target }
        };
        engine.validate()?;
        if form.choice_idx(0) == 0 && !key.is_empty() {
            self.store_key(cx, key_name, &key, true)?;
        }
        Self::svc(cx).settings.lock().unwrap().update(|st| if stt { st.voice.stt = Some(engine) } else { st.voice.tts = Some(engine) }).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, if stt { "settings.voice.stt" } else { "settings.voice.tts" }, "");
        Ok(())
    }

    fn handle_voice(&mut self, k: &Key, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let toggle = |cx: &AppCx, f: &dyn Fn(&mut iyakki_agent::voice::VoiceSettings)| Self::svc(cx).settings.lock().unwrap().update(|s| f(&mut s.voice));
        match k {
            Key::Char('s') if self.admin(cx) => {
                let f = Self::engine_form(cx, true);
                self.open_form(FormKind::VoiceStt, f);
            }
            Key::Char('v') if self.admin(cx) => {
                let f = Self::engine_form(cx, false);
                self.open_form(FormKind::VoiceTts, f);
            }
            Key::Char('c') if self.admin(cx) => {
                let v = svc.voice_settings();
                let f = Form::new("Microphone and speaker commands").text("Record command (raw 16 kHz mono S16_LE on stdout)", &v.record_cmd, "arecord -q -f S16_LE -r 16000 -c 1 -t raw").text("Play command (WAV on stdin)", &v.play_cmd, "aplay -q");
                self.open_form(FormKind::VoiceCmds, f);
            }
            Key::Char('a') if self.admin(cx) => match toggle(cx, &|v| v.auto_send = !v.auto_send) {
                Ok(()) => self.ok("Auto-send toggled."),
                Err(e) => self.fail(e.to_string()),
            },
            Key::Char('r') if self.admin(cx) => match toggle(cx, &|v| v.speak_replies = !v.speak_replies) {
                Ok(()) => self.ok("Read replies aloud toggled."),
                Err(e) => self.fail(e.to_string()),
            },
            Key::Char('S') if self.admin(cx) => match toggle(cx, &|v| v.stt = None) {
                Ok(()) => self.ok("Speech-to-text cleared."),
                Err(e) => self.fail(e.to_string()),
            },
            Key::Char('V') if self.admin(cx) => match toggle(cx, &|v| v.tts = None) {
                Ok(()) => self.ok("Text-to-speech cleared."),
                Err(e) => self.fail(e.to_string()),
            },
            Key::Char('e') if self.admin(cx) => self.vo_cycle_engine(cx),
            Key::Char('g') if self.admin(cx) => self.vo_cycle_voice(cx),
            Key::Char('f') if self.admin(cx) => self.vo_cycle_speed(cx),
            Key::Char('h') if self.admin(cx) => self.vo_cycle_highlight(cx),
            Key::Char('w') if self.admin(cx) => match toggle(cx, &|v| v.align_words = !v.align_words) {
                Ok(()) => self.ok(if Self::svc(cx).voice_settings().align_words { "Word timing: from the sound (speech-to-text matches the read words when a server is there)." } else { "Word timing: estimated." }),
                Err(e) => self.fail(e.to_string()),
            },
            Key::Char('l') => {
                self.vo_lang = (self.vo_lang + 1) % VO_LANGS.len();
                self.vo_scroll = 0.0;
            }
            Key::Char('k') => {
                cx.audit.record(&cx.session.user, "voice.recheck", "");
                if !svc.host_tts.is_enabled() {
                    self.fail("Looking for speech engines is switched off in this build.");
                } else {
                    svc.host_tts.recheck();
                    self.ok("Looking for speech engines on this computer…");
                }
            }
            Key::Char('o') => self.vo_helper_toggle(cx),
            Key::Char(c @ '1'..='6') if self.admin(cx) => self.vo_quick(cx, (*c as u8 - b'1') as usize),
            Key::Char(c) if "mpty".contains(*c) => {
                if !cx.session.policy.voice {
                    cx.audit.record(&cx.session.user, "voice.denied", "test");
                    self.fail("Voice is not enabled for your account.");
                    return;
                }
                let ms = self.test_record_ms;
                let v = svc.voice_settings();
                let user = cx.session.user.clone();
                cx.audit.record(&user, "voice.test", &c.to_string());
                match *c {
                    'm' => self.start("listening to the microphone", move || {
                        JobOut::Msg((|| {
                            let rec = Recorder::start(&v.record_cmd)?;
                            std::thread::sleep(Duration::from_millis(ms));
                            let pcm = rec.stop()?;
                            let pk = voice::peak(&pcm);
                            if pk < 150 {
                                Err(format!("Microphone opened but it is silent (peak {pk}). Check the input volume and mute switch."))
                            } else {
                                Ok(format!("Microphone works: {:.1} s captured, peak level {pk} of 32767.", pcm.len() as f32 / voice::SAMPLE_RATE as f32))
                            }
                        })())
                    }),
                    'p' => self.start("playing a test tone", move || {
                        JobOut::Msg((|| {
                            let mut p = Player::start(&v.play_cmd, &voice::encode_wav(&voice::tone(660.0, 700), voice::SAMPLE_RATE))?;
                            p.wait(Duration::from_secs(8));
                            Ok("Played a test tone through the speaker command.".to_string())
                        })())
                    }),
                    't' => self.start("say something…", move || {
                        JobOut::Msg((|| {
                            let rec = Recorder::start(&v.record_cmd)?;
                            std::thread::sleep(Duration::from_millis(ms * 2));
                            let pcm = rec.stop()?;
                            let text = svc.transcribe_samples(&pcm)?;
                            Ok(format!("Heard: “{text}”"))
                        })())
                    }),
                    _ => self.vo_test(cx),
                }
            }
            _ => {}
        }
    }

    // ---- reading aloud: the engines of this computer, the voice, the speed, the test and the browser voice helper

    fn vo_tag(&self) -> &'static str {
        hostvoice::lang_tag(VO_LANGS[self.vo_lang % VO_LANGS.len()])
    }

    /// The engine that reads: the chosen one, else the best found on this computer, else the browser voice of the screen.
    fn vo_engine(&self, cx: &AppCx) -> Option<SpeechEngine> {
        let svc = Self::svc(cx);
        let v = svc.voice_settings();
        let auto = if v.tts.is_none() { svc.host_tts.auto_engine(Some(self.vo_tag())) } else { None };
        st::effective_engine_auto(&v, &HostVoice::current(), auto.as_ref())
    }

    fn vo_usable(cx: &AppCx) -> Vec<hostengines::Engine> {
        Self::svc(cx).host_tts.report().map(|r| r.engines.iter().filter(|e| e.usable()).cloned().collect()).unwrap_or_default()
    }

    fn set_tts(&mut self, cx: &AppCx, e: Option<SpeechEngine>, what: &str) {
        if let Some(e) = &e {
            if let Err(m) = e.validate() {
                self.fail(m);
                return;
            }
        }
        match Self::svc(cx).settings.lock().unwrap().update(|st| st.voice.tts = e) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.voice.tts", what);
                self.ok(format!("Text to speech: {what}."));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Engine ▾: automatic (the best found), each usable engine of this computer, then the browser voice when the screen has one.
    fn vo_cycle_engine(&mut self, cx: &AppCx) {
        let mut seq: Vec<(Option<SpeechEngine>, String)> = vec![(None, "automatic".into())];
        for e in Self::vo_usable(cx) {
            seq.push((Some(e.setting()), e.kind.label().to_string()));
        }
        if HostVoice::current().is_ready() {
            seq.push((Some(SpeechEngine::Browser), "browser voice".into()));
        }
        // the Indic neural voices (Settings → Indic voices installs them and says where their server is)
        let indic_url = Self::indic_engine(cx).map_or_else(|| iyakki_agent::indic::models::server(&iyakki_agent::indic::models::root(Self::svc(cx).dir())), |(u, _)| u);
        seq.push((Some(SpeechEngine::Host { engine: iyakki_agent::indic::ENGINE_ID.into(), program: Some(indic_url), voice: None }), hostengines::Kind::IndicTts.label().to_string()));
        let cur = Self::svc(cx).voice_settings().tts;
        let same = |a: &Option<SpeechEngine>, b: &Option<SpeechEngine>| match (a, b) {
            (Some(SpeechEngine::Host { engine: x, .. }), Some(SpeechEngine::Host { engine: y, .. })) => x == y,
            (a, b) => a == b,
        };
        let at = seq.iter().position(|(e, _)| same(e, &cur)).unwrap_or(0);
        let (next, name) = seq[(at + 1) % seq.len()].clone();
        self.set_tts(cx, next, &name);
    }

    /// The voices of the engine in use for the language shown: (id, name).
    fn vo_voices(&self, cx: &AppCx) -> Vec<(String, String)> {
        let Some(SpeechEngine::Host { engine, program, .. }) = self.vo_engine(cx) else { return Vec::new() };
        if engine == iyakki_agent::indic::ENGINE_ID {
            // the voices the Indic server has, for the language shown (its own gender choice: female, male)
            let tag = self.vo_tag();
            let url = program.filter(|p| !p.is_empty()).unwrap_or_else(|| iyakki_agent::indic::DEFAULT_URL.to_string());
            let voices = iyakki_agent::indic::cached_voices(&url).unwrap_or_default();
            let lang = iyakki_agent::indic::lang_code(tag);
            return voices.into_iter().filter(|v| lang.is_none_or(|l| v.lang == l)).map(|v| (v.gender.clone(), v.label())).collect();
        }
        let Some(report) = Self::svc(cx).host_tts.report() else { return Vec::new() };
        let Some(e) = hostengines::Kind::from_id(&engine).and_then(|k| report.get(k)) else { return Vec::new() };
        let tag = self.vo_tag();
        let mut mine: Vec<&hostengines::HVoice> = e.voices.iter().filter(|v| v.fit(tag) > 0).collect();
        if mine.is_empty() {
            mine = e.voices.iter().collect();
        }
        mine.into_iter().take(60).map(|v| (v.id.clone(), v.name.clone())).collect()
    }

    fn vo_cycle_voice(&mut self, cx: &AppCx) {
        let Some(SpeechEngine::Host { engine, program, voice }) = self.vo_engine(cx) else {
            self.fail("The voice is part of the engine's own settings; choose an engine of this computer (Engine) to pick its voices.");
            return;
        };
        let voices = self.vo_voices(cx);
        if voices.is_empty() {
            self.fail("This engine lists no voices.");
            return;
        }
        let at = voice.as_ref().and_then(|cur| voices.iter().position(|(id, _)| id == cur));
        // automatic, then each voice
        let next = match at {
            None if voice.is_none() => Some(0),
            None => Some(0),
            Some(i) if i + 1 < voices.len() => Some(i + 1),
            Some(_) => None,
        };
        let (new_voice, name) = match next {
            Some(i) => (Some(voices[i].0.clone()), voices[i].1.clone()),
            None => (None, "automatic".to_string()),
        };
        self.set_tts(cx, Some(SpeechEngine::Host { engine, program, voice: new_voice }), &format!("voice {name}"));
    }

    /// Sentence + word, sentence only, off: what is lit while text is read aloud (stories and chat).
    fn vo_cycle_highlight(&mut self, cx: &AppCx) {
        use iyakki_agent::voice::Highlight;
        let cur = Self::svc(cx).voice_settings().highlight;
        let next = Highlight::ALL[(Highlight::ALL.iter().position(|h| *h == cur).unwrap_or(0) + 1) % Highlight::ALL.len()];
        match Self::svc(cx).settings.lock().unwrap().update(|st| st.voice.highlight = next) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.voice.highlight", next.label());
                self.ok(format!("While reading aloud: highlight {}.", next.label().to_lowercase()));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    fn vo_cycle_speed(&mut self, cx: &AppCx) {
        const SPEEDS: [u32; 5] = [75, 100, 125, 150, 200];
        let cur = Self::svc(cx).voice_settings().speed_pct;
        let next = SPEEDS.iter().copied().find(|s| *s > cur).unwrap_or(SPEEDS[0]);
        match Self::svc(cx).settings.lock().unwrap().update(|st| st.voice.speed_pct = next) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.voice.speed", &next.to_string());
                self.ok(format!("Speaking speed {next}%."));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// One-click engine `i` of the providers set up under Models.
    fn vo_quick(&mut self, cx: &AppCx, i: usize) {
        let Some(q) = Self::vo_quick_list(cx).get(i).cloned() else { return };
        self.set_tts(cx, Some(q.engine), q.label.trim_start_matches("Use "));
    }

    /// Speech endpoints of the providers already set up under Models (OpenAI and compatible servers).
    fn vo_quick_list(cx: &AppCx) -> Vec<st::QuickEngine> {
        let providers = Self::svc(cx).settings.lock().map(|s| s.get().providers.clone()).unwrap_or_default();
        st::quick_engines(&providers, "", &[]).into_iter().filter(|q| matches!(q.engine, SpeechEngine::Http { .. })).collect()
    }

    /// Test voice: say a sentence in the chosen language with the engine in use, and say what happened.
    fn vo_test(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let v = svc.voice_settings();
        let tag = self.vo_tag();
        let speed = v.speed_pct as f32 / 100.0;
        let hv = HostVoice::current();
        let Some(engine) = self.vo_engine(cx) else {
            self.fail(if svc.host_tts.is_enabled() && svc.host_tts.report().is_none() {
                "Still looking for speech engines on this computer; try again in a moment."
            } else {
                "No speech engine is ready. Install one (see the list below), press Re-check, or open the voice helper in your browser."
            });
            return;
        };
        self.start("speaking", move || {
            JobOut::Msg((|| {
                let sample = hostengines::sample_sentence(tag);
                match &engine {
                    SpeechEngine::Host { engine: id, program, voice } => {
                        let spec = hostengines::Spec::from_parts(id, program.as_deref(), voice.as_deref())?;
                        hostengines::test_speak(&spec, tag, speed, &v.play_cmd)
                    }
                    SpeechEngine::Browser => {
                        let id = hv.speak(sample, None, Some(tag), speed)?;
                        let t = std::time::Instant::now();
                        loop {
                            match hv.state(id) {
                                hostvoice::UttState::Ended => break,
                                hostvoice::UttState::Failed(m) => return Err(m),
                                _ if t.elapsed() > Duration::from_secs(30) => return Err("the browser did not finish speaking".into()),
                                _ => std::thread::sleep(Duration::from_millis(40)),
                            }
                        }
                        Ok(format!("Spoke a sentence in {} with the browser voice.", hostengines::lang_name(tag)))
                    }
                    other => {
                        let wav = svc.synthesize_with_opts(sample, &voice::SpeakOpts { voice: None, speed, lang: Some(tag.to_string()) })?;
                        let mut p = Player::start(&v.play_cmd, &wav)?;
                        p.wait(Duration::from_secs(20));
                        Ok(format!("Synthesised a sentence with {} and played it.", other.label()))
                    }
                }
            })())
        });
    }

    /// Open the voice helper (a tab of the default browser that speaks) or disconnect it.
    fn vo_helper_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx);
        if !cx.session.policy.voice {
            cx.audit.record(&cx.session.user, "voice.denied", "helper");
            self.fail("Voice is not enabled for your account.");
            return;
        }
        if matches!(svc.host_tts.helper_state(), Some(iyakki_agent::voicehelper::HelperState::Waiting | iyakki_agent::voicehelper::HelperState::Connected)) {
            svc.host_tts.helper_stop();
            cx.audit.record(&cx.session.user, "voice.helper", "disconnected");
            self.ok("Voice helper disconnected.");
            return;
        }
        match svc.host_tts.helper_open(HostVoice::current(), None) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "voice.helper", "opened");
                self.ok("Opening the voice helper in your browser. Click the button on that page once to allow sound.");
            }
            Err(e) => self.fail(e),
        }
    }

    fn render_voice(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let svc = Self::svc(cx);
        let v = svc.voice_settings();
        let admin = cx.session.policy.manage_settings;
        let hv = HostVoice::current();
        svc.host_tts.ensure();
        let report = svc.host_tts.report();
        let engine = self.vo_engine(cx);
        let tag = self.vo_tag();
        let fs = 14.0 * s;
        let bs = 13.0 * s;
        // ---- the controls, always in view
        let (status, col) = match (&engine, st::tts_status_auto(&v, cx.session.policy.voice, &std::env::var("PATH").unwrap_or_default(), &hv, if v.tts.is_none() { svc.host_tts.auto_engine(Some(tag)) } else { None }.as_ref())) {
            (_, st::TtsStatus::Ready { engine }) => (format!("Reading aloud with {engine}."), OK),
            (None, _) if svc.host_tts.scanning() || (svc.host_tts.is_enabled() && report.is_none()) => ("Looking for speech engines on this computer…".to_string(), MUTED),
            (_, other) => (st::status_text(&other), ERR),
        };
        let mut y = a.y;
        for l in g.wrap(15.0 * s, &status, Face::Bold, a.w).into_iter().take(2) {
            g.text(a.x, y, 15.0 * s, col, &l, Face::Bold);
            y += 20.0 * s;
        }
        y += 4.0 * s;
        let engine_name = match (&v.tts, &engine) {
            (Some(e), _) => e.label(),
            (None, Some(SpeechEngine::Browser)) => "automatic: browser voice".to_string(),
            (None, Some(e)) => format!("automatic: {}", e.label()),
            (None, None) => "automatic: none found".to_string(),
        };
        let voice_name = match &engine {
            Some(SpeechEngine::Host { voice: Some(id), .. }) => std::path::Path::new(id).file_stem().map_or(id.clone(), |x| x.to_string_lossy().to_string()),
            Some(SpeechEngine::Host { .. }) => "automatic".to_string(),
            _ => "set by the engine".to_string(),
        };
        let chips: Vec<(String, char)> = vec![
            (format!("Engine: {} ▾", g.fit(bs, &engine_name, Face::Bold, (a.w * 0.6).max(90.0 * s))), 'e'),
            (format!("Voice: {} ▾", g.fit(bs, &voice_name, Face::Bold, (a.w * 0.4).max(80.0 * s))), 'g'),
            (format!("Language: {} ▾", VO_LANGS[self.vo_lang % VO_LANGS.len()]), 'l'),
            (format!("Speed: {}% ▾", v.speed_pct), 'f'),
            (format!("Highlight: {} ▾", v.highlight.label()), 'h'),
            (format!("Word timing: {} ▾", if v.align_words { "from the sound" } else { "estimated" }), 'w'),
        ];
        y += vo_flow(g, a.x, y, a.w, bs, &chips, |c| if matches!(c, 'e' | 'g' | 'f' | 'h' | 'w') && !admin { DIM } else { ACCENT }, s);
        let helper = svc.host_tts.helper_state();
        let helper_on = matches!(helper, Some(iyakki_agent::voicehelper::HelperState::Waiting | iyakki_agent::voicehelper::HelperState::Connected));
        let acts: Vec<(String, char)> = vec![
            ("Test voice".into(), 'y'),
            ("Re-check engines".into(), 'k'),
            (if helper_on { "Disconnect voice helper".into() } else { "Open voice helper in your browser".into() }, 'o'),
        ];
        y += vo_flow(g, a.x, y, a.w, bs, &acts, |c| if c == 'y' { ACCENT } else { TEXT }, s);
        let helper_line = match &helper {
            Some(iyakki_agent::voicehelper::HelperState::Connected) => "Voice helper connected: your browser is speaking for IYAKKI.".to_string(),
            Some(iyakki_agent::voicehelper::HelperState::Waiting) => "Voice helper: waiting for your browser to open the page (click the button on it once).".to_string(),
            Some(iyakki_agent::voicehelper::HelperState::Closed(why)) => format!("Voice helper closed: {why}"),
            None => "Voice helper: reads aloud with your web browser's voices when this window has none (nothing leaves this computer).".to_string(),
        };
        for l in g.wrap(12.0 * s, &helper_line, Face::Sans, a.w).into_iter().take(2) {
            g.text(a.x, y, 12.0 * s, if matches!(helper, Some(iyakki_agent::voicehelper::HelperState::Connected)) { OK } else { MUTED }, &l, Face::Sans);
            y += 17.0 * s;
        }
        y += 8.0 * s;
        // ---- the rest scrolls
        let rest = R::new(a.x, y, a.w, (a.y + a.h - y).max(40.0 * s));
        let mut yy = rest.y - self.vo_scroll;
        let mut used = 0.0f32;
        let put = |g: &mut Gfx, yy: &mut f32, used: &mut f32, text: &str, size: f32, col: [u8; 4], face: Face, gap: f32| {
            for l in g.wrap(size, text, face, rest.w) {
                g.text(rest.x, *yy, size, col, &l, face);
                *yy += size * 1.45;
                *used += size * 1.45;
            }
            *yy += gap;
            *used += gap;
        };
        g.clipped(rest, |g| {
            put(g, &mut yy, &mut used, "Speech engines found on this computer", 15.0 * s, TEXT, Face::Bold, 2.0 * s);
            match &report {
                None if svc.host_tts.is_enabled() => put(g, &mut yy, &mut used, "Looking… (this runs once in the background)", fs, MUTED, Face::Sans, 6.0 * s),
                None => put(g, &mut yy, &mut used, "Looking for engines is off in this program.", fs, MUTED, Face::Sans, 6.0 * s),
                Some(r) => {
                    for (line, col, size) in vo_engine_lines(r, tag) {
                        put(g, &mut yy, &mut used, &line, size * s, col, Face::Sans, 3.0 * s);
                    }
                    put(g, &mut yy, &mut used, "", 4.0, DIM, Face::Sans, 4.0 * s);
                }
            }
            let quick = Self::vo_quick_list(cx);
            if !quick.is_empty() {
                put(g, &mut yy, &mut used, "Speech from the providers you set up under Models (press the number)", fs, MUTED, Face::Sans, 2.0 * s);
                let items: Vec<(String, char)> = quick.iter().enumerate().take(6).map(|(i, q)| (format!("{} {}", i + 1, q.label), (b'1' + i as u8) as char)).collect();
                let h = vo_flow(g, rest.x, yy, rest.w, bs, &items, |_| if admin { ACCENT } else { DIM }, s);
                yy += h;
                used += h;
            }
            let timing = match (v.align_words, svc.speech_aligner()) {
                (false, _) => "estimated (turned off)".to_string(),
                (true, Some(SpeechEngine::Http { url, model, .. })) => format!("aligned with {model} on {} ({})", iyakki_sec::url_host(&url).unwrap_or_default(), iyakki_agent::speechtime::locality(&url).label()),
                (true, _) => "estimated (no speech-to-text server on this computer or allowed)".to_string(),
            };
            let lines: [(&str, String, [u8; 4]); 8] = [
                ("Speech-to-text", v.stt.as_ref().map_or("not set up".into(), |e| e.label()), if v.stt.is_some() { OK } else { MUTED }),
                ("Text-to-speech", match &v.tts {
                    Some(e) => e.label(),
                    None => engine.as_ref().map_or("not set up".into(), |e| format!("{} (automatic)", e.label())),
                }, if v.tts.is_some() || engine.is_some() { OK } else { MUTED }),
                ("Record command", v.record_cmd.clone(), TEXT),
                ("Play command", v.play_cmd.clone(), TEXT),
                ("Auto-send speech", if v.auto_send { "on — transcribed text is sent immediately".into() } else { "off — text lands in the input box so you can edit it".into() }, TEXT),
                ("Read replies aloud", if v.speak_replies { "on".into() } else { "off".into() }, TEXT),
                ("Highlight while reading", v.highlight.label().to_lowercase(), TEXT),
                ("Word timing", timing, TEXT),
            ];
            yy += 8.0 * s;
            for (k, val, col) in lines.iter() {
                g.text(rest.x, yy, fs, MUTED, k, Face::Sans);
                let val = g.fit(15.0 * s, val, Face::Mono, rest.w - 190.0 * s);
                g.text(rest.x + 180.0 * s, yy, 15.0 * s, *col, &val, Face::Mono);
                yy += 26.0 * s;
                used += 26.0 * s;
            }
            yy += 8.0 * s;
            let notes = [
                if cx.session.policy.voice { "Your account may use the microphone and speaker." } else { "Voice is switched off for your account (an admin can enable it in Users)." },
                "Reading aloud works with no setup when this computer has a speech engine (espeak-ng, speech-dispatcher, piper, macOS say, Windows speech): the best one is used unless you choose another. docs/VOICE.md lists what to install.",
                "Speech-to-text and other engines: an OpenAI-compatible HTTP server (OpenAI, speaches, LocalAI, whisper.cpp server) or a local command (whisper-cli, piper).",
                "Audio never leaves the machine unless you point an engine at a remote server. Recordings are not stored.",
            ];
            for n in notes {
                put(g, &mut yy, &mut used, n, 13.0 * s, DIM, Face::Sans, 4.0 * s);
            }
        });
        self.vo_max = (used + 60.0 * s - rest.h).max(0.0);
        self.vo_scroll = self.vo_scroll.clamp(0.0, self.vo_max);
    }

    /// Number of selectable rows on the current tab.
    fn list_len(&mut self, cx: &AppCx) -> usize {
        match self.tab() {
            Tab::Models => Self::providers(cx).len(),
            Tab::Agents => Self::agent_ids(cx).len(),
            Tab::Rag => Self::externals(cx).len(),
            Tab::Vault => self.secret_names(cx).map_or(0, |n| n.len()),
            Tab::SecretProviders => Self::secret_providers(cx).len(),
            Tab::Skills => Self::svc(cx).skills.list(&cx.session.user).len(),
            Tab::Storage => self.devices.len(),
            Tab::Network | Tab::Bluetooth | Tab::Display | Tab::Sound => self.dev_rows().len(),
            Tab::HostAccess => Self::host_rows(cx).len(),
            Tab::Developer => Self::devmode_keys(cx).len(),
            Tab::DevCluster => self.dc_len(),
            Tab::Install => self.inst_len(),
            Tab::IndicVoices => Self::indic_len(),
            Tab::Voice | Tab::Account | Tab::Browser | Tab::Memory | Tab::Time | Tab::HostName => 0,
        }
    }

    fn add_skill(&mut self, cx: &AppCx, markdown: &str, shared: bool) -> Result<String, String> {
        use iyakki_agent::skills::Scope;
        let p = &cx.session.policy;
        if shared && !p.manage_skills {
            cx.audit.record(&cx.session.user, "settings.denied", "skills.shared");
            return Err("You may not edit the shared skills.".into());
        }
        let scope = if shared { Scope::Shared } else { Scope::User(cx.session.user.clone()) };
        let sk = Self::svc(cx).skills.add(scope, markdown, p.manage_skills, p.skills).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, "skills.add", &format!("{}:{}", sk.scope.label(), sk.name));
        Ok(format!("Saved {} skill `{}`.", sk.scope.label(), sk.name))
    }

    /// Read a skill file, but only from where this user may read (symlinks resolved first).
    fn read_skill_file(cx: &AppCx, path: &str) -> Result<String, String> {
        let real = iyakki_agent::scope::resolve(std::path::Path::new("/"), path.trim())?;
        iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &real)?;
        let len = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?.len();
        if len > 2 * iyakki_agent::skills::MAX_BODY_BYTES as u64 {
            return Err(format!("{} is too large for a skill", real.display()));
        }
        std::fs::read_to_string(&real).map_err(|e| format!("{}: {e}", real.display()))
    }

    fn handle_storage(&mut self, k: &Key, cx: &AppCx) {
        match k {
            Key::Up => self.move_sel(-1, self.devices.len()),
            Key::Down => self.move_sel(1, self.devices.len()),
            Key::Char('r') => {
                self.refresh_devices();
                self.ok("Disks rescanned.");
            }
            Key::Char('b') if self.admin(cx) => {
                if cx.session.policy.power {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Reboot, "Restart this machine now?".into()));
                } else {
                    self.fail("Your account may not restart the machine.");
                }
            }
            Key::Char('n') if self.admin(cx) && self.storage.virtual_disks.is_some() => {
                self.open_form(FormKind::NewVirtualDisk, Form::new("New virtual disk").text("Size (for example 8G, 512M)", "8G", "8G"));
            }
            Key::Char('x') if self.admin(cx) && self.storage.virtual_disks.is_some() => {
                let Some(dev) = self.devices.get(self.sel()).cloned() else { return };
                if dev.parent.is_some() {
                    return self.fail("Pick the disk itself, not one of its partitions.");
                }
                let f = Form::new(&format!("Delete the virtual disk {} and everything on it?", dev.name)).text(&format!("Type {} to confirm", dev.name), "", "the image file is removed");
                self.open_form(FormKind::RemoveVirtualDisk(dev.name.clone()), f);
            }
            Key::Char('m') if self.admin(cx) => {
                let Some(dev) = self.devices.get(self.sel()).cloned() else { return };
                match iyakki_storage::eligibility(&dev, &self.devices) {
                    Err(why) => self.fail(format!("Cannot use it: {why}.")),
                    Ok(what) => {
                        let mut f = Form::new(&format!("Erase /dev/{} and use it for IYAKKI data?", dev.name)).text(&format!("Type {} to confirm", dev.name), "", "everything on this disk will be lost");
                        if let Some(w) = &what {
                            f = f.text(&format!("It holds {w}. Type ERASE"), "", "ERASE");
                        }
                        self.open_form(FormKind::Persist(dev.name.clone(), what.is_some()), f);
                    }
                }
            }
            _ => {}
        }
    }

    fn render_storage(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        use iyakki_storage::DataLocation;
        let loc = iyakki_storage::data_location(&self.storage.roots, &self.storage.data_dir);
        let (text, col) = match &loc {
            DataLocation::Disk(d) => (format!("Saved to disk ({d}). Your accounts, settings, vaults, documents and chats survive power-off."), OK),
            DataLocation::Memory => ("Kept in RAM. Everything is forgotten at power-off. Pick a disk below to keep your data.".to_string(), rgb(255, 196, 94)),
            DataLocation::Other(o) => (format!("Stored on a {o} volume."), MUTED),
        };
        for (i, l) in g.wrap(15.0 * s, &text, Face::Bold, a.w).iter().take(2).enumerate() {
            g.text(a.x, a.y + i as f32 * 21.0 * s, 15.0 * s, col, l, Face::Bold);
        }
        if let Some((free, total)) = iyakki_storage::space(&self.storage.data_dir) {
            g.text(a.x, a.y + 46.0 * s, 13.0 * s, MUTED, &format!("{} free of {}", iyakki_storage::human(free), iyakki_storage::human(total)), Face::Sans);
        }
        let mut y = a.y + 76.0 * s;
        if self.storage.virtual_disks.is_some() {
            let note = "Virtual disks: this window only shows disks it makes for itself (files in its own folder), so nothing here can touch your computer's drives.";
            for l in g.wrap(13.0 * s, note, Face::Sans, a.w).iter().take(2) {
                g.text(a.x, y, 13.0 * s, ACCENT, l, Face::Sans);
                y += 18.0 * s;
            }
            y += 6.0 * s;
        }
        if self.devices.is_empty() {
            let none = if self.storage.virtual_disks.is_some() { "No virtual disks yet. Press n to make one." } else { "No disks found. Attach a USB drive or disk and press r." };
            g.text(a.x, y, 15.0 * s, DIM, none, Face::Sans);
        }
        let _ = cx;
        for (i, d) in self.devices.iter().enumerate() {
            if y > a.y + a.h - 58.0 * s {
                break;
            }
            let indent = if d.parent.is_some() { 24.0 * s } else { 0.0 };
            let verdict = match iyakki_storage::eligibility(d, &self.devices) {
                Err(why) => why,
                Ok(None) => "empty — can be used".to_string(),
                Ok(Some(what)) => format!("holds {what}"),
            };
            let sub = format!("{} · {}", if d.model.is_empty() { d.content.describe() } else { format!("{} · {}", d.model, d.content.describe()) }, verdict);
            let size = d.human_size();
            let mut chips: Vec<(&str, [u8; 4])> = vec![(size.as_str(), MUTED)];
            if d.removable {
                chips.push(("removable", ACCENT));
            }
            if d.content == iyakki_storage::Content::Iyakki {
                chips.push(("IYAKKI data", OK));
            }
            if d.mounted_at.is_some() {
                chips.push(("in use", ERR));
            }
            row(g, i, a.x + indent, y, a.w - indent, 58.0 * s, s, i == self.sel(), &format!("/dev/{}", d.name), &sub, &chips);
            y += 64.0 * s;
        }
    }

    fn dev_tab(&self) -> Option<DevTab> {
        match self.tab() {
            Tab::Network => Some(DevTab::Network),
            Tab::Bluetooth => Some(DevTab::Bluetooth),
            Tab::Display => Some(DevTab::Display),
            Tab::Sound => Some(DevTab::Sound),
            _ => None,
        }
    }

    fn dev_rows(&mut self) -> Vec<DevRow> {
        let ticks = self.ticks;
        // a window on another computer: the host operating system owns the MAC addresses
        let host = self.storage.virtual_disks.is_some();
        self.hw.set_host_managed(host);
        self.dev_tab().map(|t| self.hw.rows(t, ticks)).unwrap_or_default()
    }

    fn selected_target(&mut self) -> Option<Target> {
        let sel = self.sel();
        self.dev_rows().get(sel).map(|r| r.target.clone())
    }

    /// The adapter the selected Network row belongs to.
    fn selected_adapter(&mut self) -> Option<(String, iyakki_devctl::LinkKind)> {
        match self.selected_target()? {
            Target::Ethernet(n) => Some((n, iyakki_devctl::LinkKind::Ethernet)),
            Target::Wifi(n, _) => Some((n, iyakki_devctl::LinkKind::Wifi)),
            Target::Mac(n, k) => Some((n, k)),
            _ => None,
        }
    }

    /// `m` on the Network page: choose the private MAC address mode for the kind of adapter that is selected.
    fn choose_mac_mode(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the network.");
        }
        if self.hw.mac().host_managed {
            return self.fail("The MAC address is managed by the host operating system here (this is a window on another computer).");
        }
        let Some((_, kind)) = self.selected_adapter() else { return self.fail("Select an Ethernet or WiFi row first.") };
        let title = format!("Private MAC address for {}", if kind == iyakki_devctl::LinkKind::Wifi { "WiFi" } else { "Ethernet" });
        let items = iyakki_devctl::mac::MacMode::ALL.iter().map(|m| m.label().to_string()).collect();
        self.modal = Some(Modal::Pick(PickKind::MacMode(kind), Picker::new(&title, items)));
    }

    /// `a` on the Network page: ask before changing the selected adapter's address now.
    fn ask_mac_apply(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the network.");
        }
        if self.hw.mac().host_managed {
            return self.fail("The MAC address is managed by the host operating system here (this is a window on another computer).");
        }
        let Some((iface, _)) = self.selected_adapter() else { return self.fail("Select an Ethernet or WiFi row first.") };
        let warn = if self.hw.mac().carries_session(&iface) { format!(" WARNING: {iface} carries the connection you are using now (SSH or the remote screen); it will drop and you may need to reconnect.") } else { String::new() };
        let text = format!("Change the MAC address of {iface} now? The link goes down for a few seconds and reconnects with the address the setting calls for.{warn} Press y to continue.");
        self.modal = Some(Modal::Confirm(ConfirmKind::MacApply(iface), text));
    }

    /// The `y` of the question above: change the address and reconnect, in the background.
    fn mac_apply_now(&mut self, cx: &AppCx, iface: String) {
        if !self.admin(cx) {
            return;
        }
        let mac = self.hw.mac().clone();
        let (w, known) = (self.wifi.wifi.clone(), self.wifi.known.clone());
        cx.audit.record(&cx.session.user, "settings.network.mac.apply", &iface);
        self.hw.invalidate();
        self.start("changing the MAC address", move || {
            JobOut::Msg((|| {
                let wifi = iyakki_devctl::links(&mac.roots).iter().any(|l| l.name == iface && l.kind == iyakki_devctl::LinkKind::Wifi);
                let ssid = if wifi { w.current_ssid(&iface) } else { None };
                let said = mac.apply_now(&iface, ssid.as_deref(), true)?;
                if wifi {
                    let eff = ssid.or_else(|| mac.preferred_ssid());
                    let mode = mac.config().mode_for(iyakki_devctl::LinkKind::Wifi, &iface, eff.as_deref());
                    w.apply_with(&iface, &known.list(), mode, true)?;
                } else {
                    w.dhcp(&iface, true)?;
                }
                Ok(format!("{said} Reconnecting."))
            })())
        });
    }

    /// `w` on the Network page: look for networks.
    fn scan_wifi(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the network.");
        }
        let Some(iface) = self.wifi.iface() else { return self.fail("This computer has no WiFi adapter.") };
        let w = self.wifi.wifi.clone();
        cx.audit.record(&cx.session.user, "settings.wifi.scan", &iface);
        self.start("looking for WiFi networks", move || JobOut::Wifi(w.scan(&iface)));
    }

    fn show_wifi_picker(&mut self, aps: Vec<iyakki_devctl::wifi::Ap>) {
        let known: Vec<String> = self.wifi.known.list().into_iter().map(|k| k.ssid).collect();
        let mut items: Vec<(String, Option<iyakki_devctl::wifi::Ap>)> = aps
            .into_iter()
            .map(|a| {
                let bars = "▂▄▆█".chars().take(a.bars()).collect::<String>();
                let tag = if a.connected { "  · connected" } else if known.contains(&a.ssid) { "  · saved" } else { "" };
                (format!("{}   {bars:<4}  {}  {}{tag}", a.ssid, a.security.label(), a.band()), Some(a))
            })
            .collect();
        items.push(("Hidden network…".into(), None));
        if !known.is_empty() {
            items.push(("Forget a saved network…".into(), None));
        }
        let titles = items.iter().map(|(t, _)| t.clone()).collect();
        self.wifi_items = items;
        self.modal = Some(Modal::Pick(PickKind::Wifi, Picker::new("Join a WiFi network", titles)));
    }

    /// Remember a network and make the machine use it, in the background.
    fn join_wifi(&mut self, cx: &AppCx, k: iyakki_devctl::wifi::Known) {
        let Some(iface) = self.wifi.iface() else { return self.fail("This computer has no WiFi adapter.") };
        let (w, store) = (self.wifi.wifi.clone(), self.wifi.known.clone());
        let mac = iyakki_devctl::mac::MacEnv { roots: self.wifi.roots.clone(), ..self.hw.mac().clone() };
        cx.audit.record(&cx.session.user, "settings.wifi.join", &k.ssid);
        let ssid = k.ssid.clone();
        self.start("joining the network", move || {
            JobOut::Msg((|| {
                store.add(k)?;
                // the private address for this network (when one is chosen) before anything is sent from the adapter
                let mode = mac.config().mode_for(iyakki_devctl::LinkKind::Wifi, &iface, Some(&ssid));
                let (mut changed, mut note) = (false, String::new());
                if !mac.host_managed && (mode.is_private() || mac.infos().iter().any(|i| i.iface == iface && i.private)) {
                    match mac.apply_now(&iface, Some(&ssid), true) {
                        Ok(m) => changed = !m.contains("already has"),
                        Err(e) => note = format!(" Private MAC address not applied ({e}); the adapter keeps the address it has."),
                    }
                }
                w.apply_with(&iface, &store.list(), mode, changed)?;
                Ok(format!("Joining {ssid}. It is remembered and tried first the next time.{note}"))
            })())
        });
    }

    /// Bigger (`u`) or smaller (`U`) text and buttons, kept for this person.
    fn change_ui_size(&mut self, bigger: bool, cx: &AppCx) {
        use crate::uisize::{apply, load, save, LEVELS};
        let cur = load(cx.services.dir(), &cx.session.user);
        let next = if bigger { (cur + 1).min(LEVELS.len() - 1) } else { cur.saturating_sub(1) };
        if next == cur {
            return self.ok(format!("Already the {} size.", if bigger { "largest" } else { "smallest" }));
        }
        save(cx.services.dir(), &cx.session.user, next);
        apply(Some(next));
        self.ok(format!("Text and buttons: {} ({}%).", LEVELS[next].0, (LEVELS[next].1 * 100.0).round()));
    }

    fn handle_hw(&mut self, k: &Key, cx: &AppCx) {
        let Some(tab) = self.dev_tab() else { return };
        if tab == DevTab::Sound && self.sound_key(k, cx) {
            return;
        }
        let len = self.dev_rows().len();
        match k {
            Key::Char('w') if tab == DevTab::Network => self.scan_wifi(cx),
            Key::Char('m') if tab == DevTab::Network => self.choose_mac_mode(cx),
            Key::Char('a') if tab == DevTab::Network => self.ask_mac_apply(cx),
            Key::Enter | Key::Char(' ') if tab == DevTab::Network && matches!(self.selected_target(), Some(Target::Mac(..))) => self.choose_mac_mode(cx),
            Key::Char('u') if tab == DevTab::Display => self.change_ui_size(true, cx),
            Key::Char('U') if tab == DevTab::Display => self.change_ui_size(false, cx),
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('r') => {
                self.hw.invalidate();
                self.ok("Looked at the hardware again.");
            }
            other => {
                if !self.admin(cx) {
                    return;
                }
                let (sel, ticks) = (self.sel(), self.ticks);
                if let Some(d) = self.hw.act(tab, sel, other, ticks) {
                    match d.result {
                        Ok(m) => {
                            if !d.audit.is_empty() {
                                cx.audit.record(&cx.session.user, &format!("settings.{}", self.tab().label().to_lowercase()), &d.audit);
                            }
                            self.ok(m);
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
        }
    }

    fn render_hw(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let note = match self.tab() {
            Tab::Network => "w joins a WiFi network. m chooses the private MAC address (off, per network, random): it applies on the next connection, a applies it now (reconnects). Off keeps DHCP reservations working.",
            Tab::Bluetooth => "Pairing keyboards, mice and speakers is planned; today you can see and switch the adapter.",
            Tab::Display => "The desktop uses the screen's best mode. Choosing another resolution needs direct display control, which is planned.",
            _ => "o chooses the output (Automatic follows what is connected) · t plays a test chime · the IYAKKI volume is applied by IYAKKI (HDMI has no hardware volume).",
        };
        let admin = cx.session.policy.manage_settings;
        if self.tab() == Tab::Sound {
            self.hw.set_sound(Self::svc(cx).sound_settings());
        }
        let rows = self.dev_rows();
        let mut y = a.y;
        if self.tab() == Tab::Display {
            y += self.render_ui_size_card(g, R::new(a.x, y, a.w, 74.0 * s), s, cx) + 10.0 * s;
        }
        // a small screen: rows above are left out until the selected one is on the page
        let step = |r: &DevRow| if r.slider.is_some() { 90.0 * s } else { 64.0 * s };
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let mut first = 0;
        while first < sel && y + rows[first..sel].iter().map(step).sum::<f32>() > a.y + a.h - 70.0 * s {
            first += 1;
        }
        for (i, r) in rows.iter().enumerate().skip(first) {
            if y > a.y + a.h - 70.0 * s {
                break;
            }
            if let Some(v) = r.slider {
                // a click on the card selects the row; the buttons and the track on top of it change the value
                let card = R::new(a.x, y, a.w, 82.0 * s);
                g.hit(card, Event::Row(i), &r.title);
                let track = controls::slider_row(
                    g,
                    card,
                    s,
                    &Slider { id: SLIDER_HW + i, title: &r.title, help: &r.sub, value: v.value, min: v.min, max: v.max, unit: v.unit, minus: Event::Sub(i as u8, Key::Char('-')), plus: Event::Sub(i as u8, Key::Char('+')), enabled: admin && r.actionable() },
                    ACCENT,
                );
                self.slider_w.push((SLIDER_HW + i, track.w));
                if i == self.sel() {
                    g.rrect_outline(card.x, card.y, card.w, card.h, 14.0 * s, 2.0 * s, ACCENT);
                }
                y += 90.0 * s;
            } else {
                let chips: Vec<(&str, [u8; 4])> = r.chips.iter().map(|c| (c.0.as_str(), c.1)).collect();
                row(g, i, a.x, y, a.w, 58.0 * s, s, i == self.sel(), &r.title, &r.sub, &chips);
                y += 64.0 * s;
            }
        }
        let note = g.fit(13.0 * s, note, Face::Sans, a.w);
        g.text(a.x, a.y + a.h - 22.0 * s, 13.0 * s, DIM, &note, Face::Sans);
    }

    /// A click on a hardware page's slider track.
    fn hw_slider(&mut self, id: usize, x: f32, w: f32, cx: &AppCx) {
        let row = id - SLIDER_HW;
        let Some(tab) = self.dev_tab() else { return };
        if tab == DevTab::Sound && matches!(self.dev_rows().get(row).map(|r| &r.target), Some(Target::SoftVolume)) {
            self.sel[self.tab] = row;
            return self.sound_set_volume(cx, controls::value_at(x, w, 0, 100, 5) as u32);
        }
        if !self.admin(cx) {
            return;
        }
        let Some(v) = self.dev_rows().get(row).and_then(|r| r.slider) else { return };
        self.sel[self.tab] = row;
        let value = controls::value_at(x, w, v.min, v.max, v.step);
        let ticks = self.ticks;
        if let Some(d) = self.hw.set_value(tab, row, value, ticks) {
            match d.result {
                Ok(m) => {
                    if !d.audit.is_empty() {
                        cx.audit.record(&cx.session.user, &format!("settings.{}", self.tab().label().to_lowercase()), &d.audit);
                    }
                    self.ok(m);
                }
                Err(e) => self.fail(e),
            }
        }
    }

    fn handle_browser(&mut self, k: &Key, cx: &AppCx) {
        let mut c = super::browser_config::load(cx);
        let what = match k {
            Key::Char('a') => {
                c.js_all = !c.js_all;
                if c.js_all {
                    iyakki_js::warm_up();
                }
                format!("Scripts on every site: {}.", if c.js_all { "on" } else { "off" })
            }
            Key::Char('i') => {
                c.images = !c.images;
                format!("Pictures: {}.", if c.images { "on" } else { "off" })
            }
            Key::Char('s') => {
                c.styles = !c.styles;
                format!("Style sheets: {}.", if c.styles { "on" } else { "off" })
            }
            Key::Char('c') => {
                c.cookies = !c.cookies;
                format!("Cookies: {}.", if c.cookies { "on" } else { "off" })
            }
            Key::Char('e') => {
                c.search = c.search.next();
                format!("Search engine: {}.", c.search.name())
            }
            Key::Char('f') => {
                c.js_sites.clear();
                "Forgot the sites allowed to run scripts.".to_string()
            }
            _ => return,
        };
        super::browser_config::save(cx, &c);
        cx.audit.record(&cx.session.user, "settings.browser", &what);
        self.ok(what);
    }

    fn render_browser(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let c = super::browser_config::load(cx);
        let (gap, rh) = (10.0 * s, 70.0 * s);
        let two = a.w > 820.0 * s;
        let cw = if two { (a.w - gap) / 2.0 } else { a.w };
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Pages");
        let rows: [(&str, &str, bool, char); 4] = [
            ("Run scripts on every site", "Pages can change themselves, load more content and answer clicks. Scripts run in a sandbox with time and memory limits. When this is off, scripts run only on the sites you allow (press j on a page).", c.js_all, 'a'),
            ("Show pictures", "PNG, JPEG and GIF pictures are fetched after the text.", c.images, 'i'),
            ("Apply style sheets", "Colours, sizes and layout from the page's own style sheets.", c.styles, 's'),
            ("Accept cookies", "Let sites remember you for this session (sign-ins, preferences). Off: none are sent or kept.", c.cookies, 'c'),
        ];
        for (i, (title, help, on, key)) in rows.into_iter().enumerate() {
            let x = if two { a.x + (i % 2) as f32 * (cw + gap) } else { a.x };
            controls::switch_row(g, R::new(x, y, cw, rh), s, title, help, on, true, Event::Key(Key::Char(key)), ACCENT);
            if !two || i % 2 == 1 {
                y += rh + gap;
            }
        }
        if two {
            // an even number of switches ends a row
        }
        y += 6.0 * s;
        y += controls::section_title(g, a.x, y, s, "Search");
        controls::info_row(g, R::new(a.x, y, a.w - 130.0 * s, 44.0 * s), s, c.search.name(), "words typed in the address bar are searched here (Google, Bing and DuckDuckGo ask for checks this browser cannot pass)");
        g.button(a.x + a.w - 120.0 * s, y + 6.0 * s, 15.0 * s, "Change (e)", ACCENT, Event::Key(Key::Char('e')));
        y += 54.0 * s;
        y += controls::section_title(g, a.x, y, s, "The assistant in your browser");
        {
            let p = &cx.session.policy;
            let (title, help) = match p.browser_agent {
                iyakki_sec::policy::BrowserAgent::Off => ("Off for your account".to_string(), "The assistant cannot use your browser. An administrator can change this in Users (press w).".to_string()),
                iyakki_sec::policy::BrowserAgent::ReadOnly => ("Read-only".to_string(), "The assistant can open pages in your browser, read them, take pictures and scroll, after you allow each site once. It cannot click or type. An administrator can allow more in Users (press w).".to_string()),
                iyakki_sec::policy::BrowserAgent::Interactive => ("Read, click and type, with your approval".to_string(), format!("The assistant asks you before it acts on a site: approve all actions on that site for the session, one by one, or deny. Payments, deletions and sign-in codes are always yours. At most {} actions and {} minutes per request.", p.browser_max_actions, p.browser_max_minutes)),
            };
            controls::info_row(g, R::new(a.x, y, a.w, 56.0 * s), s, &title, &help);
            y += 66.0 * s;
        }
        y += controls::section_title(g, a.x, y, s, "Sites allowed to run scripts");
        if c.js_sites.is_empty() {
            g.text(a.x + 4.0 * s, y, 14.0 * s, DIM, if c.js_all { "None needed: scripts run everywhere." } else { "None yet. Press j on a page in the browser to allow one." }, Face::Sans);
        }
        for h in c.js_sites.iter().take(8) {
            controls::info_row(g, R::new(a.x, y, a.w, 38.0 * s), s, h, "scripts allowed");
            y += 44.0 * s;
            if y > a.y + a.h - 40.0 * s {
                break;
            }
        }
    }

    fn handle_skills(&mut self, k: &Key, cx: &AppCx) {
        let list = Self::svc(cx).skills.list(&cx.session.user);
        let cur = list.get(self.sel()).cloned();
        match k {
            Key::Up => self.move_sel(-1, list.len()),
            Key::Down => self.move_sel(1, list.len()),
            Key::Char(c @ ('c' | 'C')) => {
                let shared = *c == 'C';
                match crate::ui::clipboard::latest() {
                    None => self.fail("The clipboard is empty. Copy a skill (Markdown with name/description at the top) first."),
                    Some(t) => match self.add_skill(cx, &t, shared) {
                        Ok(m) => self.ok(m),
                        Err(e) => self.fail(e),
                    },
                }
            }
            Key::Char('a') => {
                let mut f = Form::new("New skill").text("Name", "", "lowercase-with-dashes").text("Description", "", "when should the assistant use it?").text("Triggers", "", "words that suggest it, comma separated").text("Instructions", "", "what the assistant should do");
                if self.share_skills {
                    f = f.choice("Scope", &["Mine", "Shared with everyone"], 0);
                } else {
                    f = f.choice("Scope", &["Mine"], 0);
                }
                self.open_form(FormKind::NewSkill, f);
            }
            Key::Char('i') => {
                let mut f = Form::new("Import skill file").text("File path", "", "/path/to/skill.md");
                f = if self.share_skills { f.choice("Scope", &["Mine", "Shared with everyone"], 0) } else { f.choice("Scope", &["Mine"], 0) };
                self.open_form(FormKind::ImportSkill, f);
            }
            Key::Char('d') => {
                if let Some(sk) = cur {
                    let can = match sk.scope {
                        iyakki_agent::skills::Scope::Shared => cx.session.policy.manage_skills,
                        _ => true,
                    };
                    if can {
                        let text = format!("Delete the {} skill “{}”?", sk.scope.label(), sk.name);
                        self.modal = Some(Modal::Confirm(ConfirmKind::Skill(sk.scope, sk.name), text));
                    } else {
                        self.fail("Only operators and administrators can delete shared skills.");
                    }
                }
            }
            _ => {}
        }
    }

    fn render_skills(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let list = cx.services.skills.list(&cx.session.user);
        if list.is_empty() {
            g.text(a.x, a.y + 4.0 * s, 15.0 * s, DIM, "No skills yet. A skill is a short Markdown file with instructions the assistant follows when a request matches it.", Face::Sans);
            g.text(a.x, a.y + 28.0 * s, 15.0 * s, DIM, "Copy one and press c, or press a to write one here.", Face::Sans);
            return;
        }
        let preview_h = 96.0 * s;
        let mut y = a.y;
        for (i, sk) in list.iter().enumerate() {
            if y + 58.0 * s > a.y + a.h - preview_h {
                break;
            }
            let chips: Vec<(&str, [u8; 4])> = vec![(sk.scope.label(), if sk.scope == iyakki_agent::skills::Scope::Shared { rgb(255, 196, 94) } else { ACCENT })];
            row(g, i, a.x, y, a.w, 52.0 * s, s, i == self.sel(), &sk.name, &sk.description, &chips);
            y += 58.0 * s;
        }
        if let Some(sk) = list.get(self.sel()) {
            let py = a.y + a.h - preview_h + 6.0 * s;
            g.text(a.x, py, 13.0 * s, MUTED, &format!("triggers: {}", if sk.triggers.is_empty() { "(none)".to_string() } else { sk.triggers.join(", ") }), Face::Bold);
            for (n, l) in g.wrap(13.0 * s, &sk.body, Face::Mono, a.w).iter().take(4).enumerate() {
                g.text(a.x, py + 22.0 * s + n as f32 * 17.0 * s, 13.0 * s, TEXT, l, Face::Mono);
            }
        }
    }

    fn open_form(&mut self, kind: FormKind, form: Form) {
        self.modal = Some(Modal::Form(kind, form));
    }

    fn handle_models(&mut self, k: &Key, cx: &AppCx) {
        let provs = Self::providers(cx);
        let cur = provs.get(self.sel()).cloned();
        match k {
            Key::Up => self.move_sel(-1, provs.len()),
            Key::Down => self.move_sel(1, provs.len()),
            Key::Char('a') if self.admin(cx) => {
                let f = Self::provider_form(cx, None);
                self.open_form(FormKind::Provider(None), f);
            }
            Key::Char('p') if self.admin(cx) => {
                let cur_price = Self::svc(cx).settings.lock().unwrap().get().token_price_cents_per_mtok;
                let f = Form::new("Agent token price").text("US cents per million tokens", &cur_price.to_string(), "0 = free or untracked; used for agent cost budgets");
                self.open_form(FormKind::TokenPrice, f);
            }
            Key::Char('f') if self.admin(cx) => self.detect_servers(cx),
            Key::Char('l') if cur.as_ref().is_some_and(|p| p.kind == Kind::Lemonade) => {
                let id = cur.as_ref().expect("checked").id.clone();
                self.lem_open(cx, &id);
            }
            Key::Char(c) if cur.is_some() && "edkmbxtc".contains(*c) => {
                let p = cur.expect("checked");
                match *c {
                    't' => self.test_provider(cx, &p.id),
                    'c' => self.test_chat(cx),
                    _ if !self.admin(cx) => {}
                    'e' => {
                        let f = Self::provider_form(cx, Some(&p));
                        self.open_form(FormKind::Provider(Some(p.id.clone())), f);
                    }
                    'd' => self.modal = Some(Modal::Confirm(ConfirmKind::Provider(p.id.clone()), format!("Remove provider “{}”?", p.id))),
                    'k' => {
                        let f = Form::new(&format!("API key for {}", p.id)).secret("API key", "stored in the system vault");
                        self.open_form(FormKind::ProviderKey(p.id.clone()), f);
                    }
                    'x' => {
                        let id = p.id.clone();
                        let r = Self::svc(cx).settings.lock().unwrap().update(|s| {
                            if let Some(x) = s.providers.iter_mut().find(|x| x.id == id) {
                                x.enabled = !x.enabled;
                            }
                        });
                        match r {
                            Ok(()) => self.ok(format!("{} {}.", p.id, if p.enabled { "disabled" } else { "enabled" })),
                            Err(e) => self.fail(e.to_string()),
                        }
                    }
                    'b' if p.kind == Kind::Lemonade && !p.model_kinds.is_empty() => {
                        // a Lemonade server says which of its models make embeddings
                        let mut items: Vec<String> = p.model_kinds.iter().filter(|(_, k)| k.as_str() == "embeddings").map(|(m, _)| m.clone()).collect();
                        items.insert(0, "(built-in hashing embeddings)".into());
                        self.modal = Some(Modal::Pick(PickKind::Embed(p.id.clone()), Picker::new(&format!("Embedding model on {}", p.id), items)));
                    }
                    'm' | 'b' => {
                        if p.models.is_empty() {
                            self.fail("No model list yet: press t to test the connection and fetch it.");
                        } else {
                            let mut items = p.models.clone();
                            if *c == 'b' {
                                items.insert(0, "(built-in hashing embeddings)".into());
                            }
                            let title = if *c == 'm' { format!("Chat model on {}", p.id) } else { format!("Embedding model on {}", p.id) };
                            let kind = if *c == 'm' { PickKind::Chat(p.id.clone()) } else { PickKind::Embed(p.id.clone()) };
                            self.modal = Some(Modal::Pick(kind, Picker::new(&title, items)));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn handle_rag(&mut self, k: &Key, cx: &AppCx) {
        let exts = Self::externals(cx);
        let cur = exts.get(self.sel()).cloned();
        let tweak = |cx: &AppCx, f: &dyn Fn(&mut iyakki_agent::settings::RagSettings)| Self::svc(cx).settings.lock().unwrap().update(|s| f(&mut s.rag));
        let mut result: Option<Result<(), iyakki_agent::settings::SettingsError>> = None;
        match k {
            Key::Up => self.move_sel(-1, exts.len()),
            Key::Down => self.move_sel(1, exts.len()),
            Key::Char('[') if self.admin(cx) => result = Some(tweak(cx, &|r| r.top_k = r.top_k.saturating_sub(1).max(1))),
            Key::Char(']') if self.admin(cx) => result = Some(tweak(cx, &|r| r.top_k = (r.top_k + 1).min(50))),
            Key::Char('-') if self.admin(cx) => {
                result = Some(tweak(cx, &|r| {
                    r.chunk_max = r.chunk_max.saturating_sub(100).max(100);
                    r.overlap = r.overlap.min((r.chunk_max / 2).saturating_sub(1)) / 20 * 20;
                }))
            }
            Key::Char('=') if self.admin(cx) => result = Some(tweak(cx, &|r| r.chunk_max = (r.chunk_max + 100).min(4000))),
            Key::Char(',') if self.admin(cx) => result = Some(tweak(cx, &|r| r.overlap = r.overlap.saturating_sub(20))),
            Key::Char('.') if self.admin(cx) => result = Some(tweak(cx, &|r| r.overlap += 20)),
            Key::Char('u') if self.admin(cx) => result = Some(tweak(cx, &|r| r.use_external = !r.use_external)),
            Key::Char('a') if self.admin(cx) => {
                let f = Self::external_form(None);
                self.open_form(FormKind::External(None), f);
            }
            Key::Char('r') => self.reindex(cx),
            Key::Char('i') => self.ingest(cx),
            Key::Char(c) if cur.is_some() && "edtx".contains(*c) => {
                let e = cur.expect("checked");
                match *c {
                    't' => {
                        let svc = Self::svc(cx).clone();
                        let backend = e.backend.clone();
                        self.start("testing backend", move || {
                            let c = ExtClient::new(backend.clone(), svc.secrets.clone(), svc.quick_upstream());
                            JobOut::Msg(c.test().map(|m| format!("{}: {m}", backend.id())).map_err(|e| format!("{}: {e}", backend.id())))
                        });
                    }
                    _ if !self.admin(cx) => {}
                    'e' => {
                        let f = Self::external_form(Some(&e));
                        self.open_form(FormKind::External(Some(e.backend.id().into())), f);
                    }
                    'd' => self.modal = Some(Modal::Confirm(ConfirmKind::External(e.backend.id().into()), format!("Remove backend “{}”?", e.backend.id()))),
                    'x' => {
                        let mut n = e.clone();
                        n.enabled = !n.enabled;
                        result = Some(Self::svc(cx).settings.lock().unwrap().upsert_external(n));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if let Some(r) = result {
            match r {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.rag", "");
                    self.ok("Saved.");
                }
                Err(e) => self.fail(e.to_string()),
            }
        }
    }

    fn own_namespaces(cx: &AppCx) -> Vec<String> {
        if cx.session.policy.manage_settings {
            let mut v = Self::svc(cx).kb.lock().unwrap().namespaces();
            for n in ["shared"] {
                if !v.iter().any(|x| x == n) {
                    v.push(n.into());
                }
            }
            v
        } else {
            vec![format!("user/{}", cx.session.user)]
        }
    }

    fn reindex(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let nss = Self::own_namespaces(cx);
        self.start("re-embedding documents", move || {
            let mut kb = svc.kb.lock().unwrap();
            let mut done = 0;
            for ns in &nss {
                if kb.stats(ns).is_some() {
                    if let Err(e) = kb.reindex(ns) {
                        return JobOut::Msg(Err(format!("{ns}: {e:?}")));
                    }
                    done += 1;
                }
            }
            JobOut::Msg(Ok(format!("Re-embedded {done} namespace(s) with {}.", kb.embedder_id())))
        });
    }

    fn ingest(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let base = std::path::PathBuf::from(&cx.cfg.rag_docs);
        let (ns, dir) = if cx.session.policy.manage_settings { ("shared".to_string(), base.join("shared")) } else { (format!("user/{}", cx.session.user), base.join(&cx.session.user)) };
        self.start("indexing documents", move || {
            let (added, seen) = svc.kb.lock().unwrap().ingest_dir(&ns, &dir);
            JobOut::Msg(Ok(format!("{ns}: {added} new document(s) indexed ({seen} files scanned in {}).", dir.display())))
        });
    }

    fn handle_secret_providers(&mut self, k: &Key, cx: &AppCx) {
        let list = Self::secret_providers(cx);
        match k {
            Key::Up => self.move_sel(-1, list.len()),
            Key::Down => self.move_sel(1, list.len()),
            Key::Char('a') if self.admin(cx) => self.open_form(FormKind::SecretProvider, Self::secret_provider_form()),
            Key::Char('d') if self.admin(cx) => {
                if let Some((a, _)) = list.get(self.sel()) {
                    self.modal = Some(Modal::Confirm(ConfirmKind::SecretProvider(a.clone()), format!("Remove secret provider “{a}”?")));
                }
            }
            Key::Char('t') => {
                if let Some((a, _)) = list.get(self.sel()) {
                    let (svc, a) = (Self::svc(cx).clone(), a.clone());
                    self.start("testing provider", move || JobOut::Msg(svc.secrets.test_provider(&a).map(|_| format!("{a}: reachable, token accepted")).map_err(|e| format!("{a}: {e}"))));
                }
            }
            _ => {}
        }
    }

    fn handle_vault(&mut self, k: &Key, cx: &AppCx) {
        let admin = cx.session.policy.manage_settings;
        let names = self.secret_names(cx).unwrap_or_default();
        let system = self.section == 0 && admin;
        match k {
            Key::Tab if admin => {
                self.section = 1 - self.section;
                self.sel[self.tab] = 0;
            }
            Key::Up => self.move_sel(-1, names.len()),
            Key::Down => self.move_sel(1, names.len()),
            Key::Char('n') => {
                let title = if system { "New system secret" } else { "New personal secret" };
                let f = Form::new(title).text("Name", "", "lowercase, e.g. github-token").secret("Value", "never shown again");
                self.open_form(FormKind::NewSecret(system), f);
            }
            Key::Char('d') => {
                if let Some(n) = names.get(self.sel()) {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Secret(system, n.clone()), format!("Delete secret “{n}”? This cannot be undone.")));
                }
            }
            Key::Char('l') if system => {
                Self::svc(cx).lock_system();
                cx.audit.record(&cx.session.user, "vault.system.lock", "");
                self.ok("System vault locked. An admin with access unlocks it by signing in.");
            }
            Key::Char('f') if system => {
                let f = Form::new("Unattended unlock key file").text("Path for the new key file", &Self::svc(cx).dir().join("vault.key").display().to_string(), "absolute path");
                self.open_form(FormKind::Keyfile, f);
            }
            Key::Char('R') if !system => self.modal = Some(Modal::Confirm(ConfirmKind::ResetPersonal, "Delete your personal vault and all secrets in it?".into())),
            _ => {}
        }
    }

    fn handle_account(&mut self, k: &Key, cx: &AppCx) {
        match k {
            Key::Char('p') => {
                let f = Form::new("Change password").secret("Current password", "").secret("New password (min 8)", "").secret("Repeat new password", "");
                self.open_form(FormKind::Password, f);
            }
            Key::Char('u') => self.change_ui_size(true, cx),
            Key::Char('U') => self.change_ui_size(false, cx),
            _ => {}
        }
    }
}

impl App for SettingsApp {
    fn title(&self) -> &'static str {
        "Settings"
    }
    fn icon(&self) -> Icon {
        Icon::Settings
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.modal.is_some() {
            return "Esc cancel".into();
        }
        match self.tab() {
            Tab::Models => "a add · f detect Ollama/Lemonade · e edit · d delete · t test · m chat model · b embeddings · k key · x on/off · c test chat · l Lemonade models",
            Tab::Agents => "d detect · i install · l login · o logout · k API key · m model · a Auto · u use for new chats · t test · n add custom · e edit · x hide/show",
            Tab::Rag => "a add backend · t test · r reindex · i ingest documents · / find a page",
            Tab::Vault => "n new · d delete · Tab system/personal · l lock · f key file",
            Tab::Skills => "c add from clipboard · a quick create · i import file · d delete",
            Tab::Network => "Enter turn on/off · w join a WiFi network · r look again",
            Tab::Bluetooth => "Enter turn on/off · r look again",
            Tab::Display => "u bigger text · U smaller text · + brighter · − dimmer · r look again",
            Tab::Sound => "o output · t test sound · + louder · − quieter · m mute · Enter use the selected output · r look again",
            Tab::Browser => "a scripts everywhere · i pictures · s styles · c cookies · f forget the allowed sites",
            Tab::DevCluster => "t test · d my default · w workspace default · k next workspace · h hide/show · x disconnect · g registry · P platform services (Gitea, private store) · v vault profile · r look again · s set up · b/o/e start/stop/restart · l logs",
            Tab::Storage if self.storage.virtual_disks.is_some() => "n new virtual disk · x delete it · m make this disk the data disk (erases it) · r rescan",
            Tab::Storage => "m make this disk the data disk (erases it) · r rescan · b reboot",
            Tab::Memory => "z zram · c compressor · w swappiness · f swap file · x zswap · p preview · y apply now · v save for next boots · r read again",
            Tab::Time => "h 12/24-hour · s seconds · z time zone · u machine's own zone · n network time on/off · c time server · y ask the server now · b set the clock to the network time",
            Tab::HostName => "e change the name · y apply now and keep it · v save for next boots · r read again",
            Tab::Developer => "e on/off · h switch-off time · p pair a computer · c cancel the code · Up/Down choose · x revoke · l activity log",
            Tab::Install => "Up/Down choose a disk · i install (erases it) · p show the plan · t partition table (Auto/GPT/MBR) · r look again · b restart",
            Tab::Voice => "s speech-to-text · v text-to-speech · c commands · m mic test · p speaker test · t STT test · y test voice · e engine · g voice · l language · f speed · k re-check · o voice helper",
            Tab::IndicVoices => "Up/Down choose a language · d download (asks first) · x delete · u from a folder or USB stick · b where bundles are published · a voice server address · e use for reading on/off · g female/male · y test voice · c check the server · k cancel a download",
            Tab::SecretProviders => "a add · t test · d delete",
            Tab::HostAccess => "a share a folder · o options · m read-only/read-write · c convert to a copy · u unlink · i import files · l audit trail",
            Tab::Account => "p change password · u bigger text · U smaller text",
        }
        .into()
    }

    fn open_item(&mut self, item: &str, cx: &AppCx) {
        // `tab:network` and the like: another part of the shell (the Devices popover, the clock) asks for one page
        if let Some(name) = item.strip_prefix("tab:") {
            let want = match name {
                "network" => Some(Tab::Network),
                "bluetooth" => Some(Tab::Bluetooth),
                "display" => Some(Tab::Display),
                "sound" => Some(Tab::Sound),
                "storage" => Some(Tab::Storage),
                "time" => Some(Tab::Time),
                "hostname" => Some(Tab::HostName),
                "developer" => Some(Tab::Developer),
                _ => None,
            };
            if let Some(i) = want.and_then(|t| self.tabs.iter().position(|x| *x == t)) {
                self.tab = i;
            }
            return;
        }
        if item == "voice" {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::Voice) {
                self.tab = i;
                self.vo_scroll = 0.0;
                cx.services.host_tts.ensure();
            }
        }
        if let Some(id) = item.strip_prefix("platform:") {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::DevCluster) {
                self.tab = i;
                self.dc_open_platform(cx, Some(id));
            }
        }
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.ticks += 1;
            self.poll(cx);
            self.host_dialog_event(ev);
            if self.tab() == Tab::DevCluster {
                self.dc_overlay_event(ev, cx);
                self.dc_prepare(cx);
            }
            if self.dc_platform_open() {
                self.dc_platform_event(ev);
            }
            return Flow::Stay;
        }
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            self.dc_platform_event(ev);
            return Flow::Stay;
        }
        if self.tab() == Tab::DevCluster && self.dc_has_overlay() {
            self.dc_overlay_event(ev, cx);
            return Flow::Stay;
        }
        // a Host access dialog takes every key and click while it is open
        if self.host_dialog_event(ev) {
            return Flow::Stay;
        }
        if self.modal.is_some() {
            self.handle_modal(ev, cx);
            return Flow::Stay;
        }
        if self.tab() == Tab::Memory && self.memory_event(ev) {
            return Flow::Stay;
        }
        if self.tab() == Tab::Install && self.inst.plan.handle(ev) {
            return Flow::Stay;
        }
        match ev {
            Event::MouseMove(x, y) => self.mouse = (*x, *y),
            Event::Tab(i) => {
                self.tab = (*i).min(self.tabs.len() - 1);
                self.nav_focus = false;
            }
            Event::FocusAt(id, x) if *id == nav::SEARCH_ID => {
                self.nav_focus = true;
                self.nav_query.click(*x);
            }
            Event::Paste(t) if self.nav_focus => {
                self.nav_query.paste(t);
                self.nav_after_edit();
            }
            Event::AreaAt(id, x, _) => self.slider_click(*id, *x, cx),
            // a button on a slider card: select that row, then press the key on it
            // a model of the Lemonade panel on Settings → Models
            Event::Sub(i, Key::Char('L')) if self.tab() == Tab::Models => self.lem_chip(*i as usize, cx),
            Event::Sub(row, k) if self.dev_tab().is_some() => {
                self.sel[self.tab] = *row as usize;
                self.handle_hw(k, cx);
            }
            Event::Row(i) => {
                let len = self.list_len(cx);
                self.sel[self.tab] = (*i).min(len.saturating_sub(1));
            }
            Event::Scroll(d) if self.tab() == Tab::Voice && !self.nav_rect.contains(self.mouse.0, self.mouse.1) => {
                self.vo_scroll = (self.vo_scroll - *d as f32 * 40.0 * self.nav_s).clamp(0.0, self.vo_max);
            }
            Event::Scroll(d) => {
                if self.nav_rect.contains(self.mouse.0, self.mouse.1) {
                    let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
                    self.nav_scroll = nav::clamp_scroll(self.nav_scroll - *d as f32 * 48.0 * self.nav_s, &entries, self.nav_rect.h, self.nav_s);
                } else {
                    let len = self.list_len(cx);
                    self.move_sel(-*d, len);
                }
            }
            Event::Key(k) if self.nav_focus => {
                self.nav_key(k);
            }
            // find a page: Ctrl+F, or / when no page needs the key
            Event::Key(Key::Ctrl('f')) | Event::Key(Key::Char('/')) => self.nav_focus = true,
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Left) => self.tab = self.tab.saturating_sub(1),
            Event::Key(Key::Right) => self.tab = (self.tab + 1).min(self.tabs.len() - 1),
            Event::Key(other) => match self.tab() {
                Tab::Models => self.handle_models(other, cx),
                Tab::Agents => self.handle_agents(other, cx),
                Tab::Rag => self.handle_rag(other, cx),
                Tab::Vault => self.handle_vault(other, cx),
                Tab::Skills => self.handle_skills(other, cx),
                Tab::Storage => self.handle_storage(other, cx),
                Tab::Network | Tab::Bluetooth | Tab::Display | Tab::Sound => self.handle_hw(other, cx),
                Tab::Voice => self.handle_voice(other, cx),
                Tab::IndicVoices => self.handle_indic(other, cx),
                Tab::SecretProviders => self.handle_secret_providers(other, cx),
                Tab::Account => self.handle_account(other, cx),
                Tab::Browser => self.handle_browser(other, cx),
                Tab::DevCluster => self.handle_devcluster(other, cx),
                Tab::HostAccess => self.handle_host(other, cx),
                Tab::Memory => self.handle_memory(other, cx),
                Tab::Time => self.handle_time(other, cx),
                Tab::HostName => self.handle_host_name(other, cx),
                Tab::Developer => self.handle_developer(other, cx),
                Tab::Install => self.handle_install(other, cx),
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // left: search and the list of pages; right: the page
        let pad = 20.0 * s;
        let nav_w = if area.w > 900.0 * s { 260.0 * s } else { (area.w * 0.30).max(170.0 * s) };
        let nav_r = R::new(area.x + pad, area.y + 10.0 * s, nav_w, area.h - 20.0 * s);
        self.nav_rect = nav_r;
        self.nav_s = s;
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        self.nav_scroll = nav::clamp_scroll(self.nav_scroll, &entries, nav_r.h, s);
        nav::draw(g, nav_r, s, &entries, self.tab, self.nav_scroll, &self.nav_query, self.nav_focus, blink(tick), ACCENT);
        let x = nav_r.x + nav_r.w + 28.0 * s;
        let w = (area.x + area.w - pad - x).max(120.0 * s);
        let page = self.tab();
        let title = g.fit(28.0 * s, page.label(), Face::Bold, w);
        g.text(x, area.y + 14.0 * s, 28.0 * s, TEXT, &title, Face::Bold);
        let blurb = g.fit(14.0 * s, page.blurb(), Face::Sans, w);
        g.text(x, area.y + 14.0 * s + 38.0 * s, 14.0 * s, MUTED, &blurb, Face::Sans);
        let bar_h = 88.0 * s;
        let admin = cx.session.policy.manage_settings;
        let rows_of_buttons = self.button_rows(admin);
        let btn_h = rows_of_buttons as f32 * 40.0 * s;
        self.slider_w.clear();
        let body = R::new(x, area.y + bar_h, w, area.h - bar_h - 40.0 * s - btn_h);
        match self.tab() {
            Tab::Models => self.render_models(g, body, s, cx),
            Tab::Agents => self.render_agents(g, body, s, cx),
            Tab::Rag => self.render_rag(g, body, s, cx),
            Tab::Vault => self.render_vault(g, body, s, cx),
            Tab::Skills => self.render_skills(g, body, s, cx),
            Tab::Storage => self.render_storage(g, body, s, cx),
            Tab::Network | Tab::Bluetooth | Tab::Display | Tab::Sound => self.render_hw(g, body, s, cx),
            Tab::Voice => self.render_voice(g, body, s, cx),
            Tab::IndicVoices => self.render_indic(g, body, s, cx),
            Tab::SecretProviders => self.render_secret_providers(g, body, s, cx),
            Tab::Account => self.render_account(g, body, s, cx),
            Tab::Browser => self.render_browser(g, body, s, cx),
            Tab::DevCluster => self.render_devcluster(g, body, s, cx),
            Tab::HostAccess => self.render_host(g, body, s, cx),
            Tab::Memory => self.render_memory(g, body, s, cx),
            Tab::Time => self.render_time(g, body, s, cx),
            Tab::HostName => self.render_host_name(g, body, s, cx),
            Tab::Developer => self.render_developer(g, body, s, cx),
            Tab::Install => self.render_install(g, body, s),
        }
        self.render_buttons(g, x, area.y + area.h - 36.0 * s - btn_h + 4.0 * s, w, s, admin);
        // status line
        let y = area.y + area.h - 30.0 * s;
        if let Some(j) = &self.job {
            g.text(x, y, 15.0 * s, MUTED, &format!("{}{}", j.label, ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
        } else if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, y, w, s, m, *ok);
        }
        let modal_area = R::new(area.x, area.y, area.w, area.h);
        self.render_host_dialog(g, modal_area, s, tick);
        match &self.modal {
            Some(Modal::Form(_, f)) => f.render(g, modal_area, s, ACCENT, blink(tick)),
            Some(Modal::Pick(_, p)) => p.render(g, modal_area, s, ACCENT),
            Some(Modal::Confirm(ConfirmKind::Indic(act), text)) => indic_page::render_card(g, modal_area, s, act, text),
            Some(Modal::Confirm(_, text)) => render_confirm(g, modal_area, s, text, "Yes, do it"),
            None => {}
        }
    }
}

// ---------------------------------------------------------------------- rendering

type Btn = (&'static str, [u8; 4], char);

/// The languages the voice test and the voice list can be shown for.
const VO_LANGS: [&str; 5] = ["English", "Tamil", "Hindi", "Telugu", "Malayalam"];

/// What the page says about each engine: found ones with their voices and what they can do, then the unusable and missing ones
/// with why and how to get them. (text, colour, size at scale 1)
fn vo_engine_lines(r: &hostengines::Report, tag: &str) -> Vec<(String, [u8; 4], f32)> {
    let mut out = Vec::new();
    for e in &r.engines {
        let speaks = if e.speaks(tag) { format!("speaks {}", hostengines::lang_name(tag)) } else { format!("no {} voice", hostengines::lang_name(tag)) };
        match &e.problem {
            None => out.push((format!("● {} · {} voices · {speaks} · {}{}", e.kind.label(), e.voices.len(), if e.kind.can_file() { "can save audio" } else { "play only: Save audio is unavailable" }, if e.kind.can_speak() { "" } else { " · needs a sound player" }), OK, 14.0)),
            Some(why) => out.push((format!("○ {}: {why}", e.kind.label()), MUTED, 14.0)),
        }
    }
    for (k, why) in &r.missing {
        out.push((format!("○ {}: {why}", k.label()), DIM, 13.0));
    }
    out
}

/// Buttons that wrap; each sends its key. Returns the height used.
fn vo_flow(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, items: &[(String, char)], col: impl Fn(char) -> [u8; 4], s: f32) -> f32 {
    let (mut bx, mut by) = (x, y);
    for (label, key) in items {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if bx > x && bx + bw > x + w {
            bx = x;
            by += 36.0 * s;
        }
        bx += g.button(bx, by, size, label, col(*key), Event::Key(Key::Char(*key))) + 8.0 * s;
    }
    by + 36.0 * s - y
}

impl SettingsApp {
    fn buttons_for(&self, admin: bool) -> Vec<Btn> {
        let (acc, mut_, err) = (ACCENT, TEXT, ERR);
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            return vec![]; // Platform services draws its own buttons
        }
        match (self.tab(), admin) {
            (Tab::Models, true) => vec![("Add", acc, 'a'), ("Detect", acc, 'f'), ("Edit", mut_, 'e'), ("Delete", err, 'd'), ("Test", mut_, 't'), ("Chat model", mut_, 'm'), ("Embeddings", mut_, 'b'), ("Set key", mut_, 'k'), ("On/Off", mut_, 'x'), ("Agent price", mut_, 'p'), ("Test chat", mut_, 'c')],
            (Tab::Models, false) => vec![("Test", mut_, 't'), ("Test chat", mut_, 'c')],
            (Tab::Agents, _) => vec![("Detect", acc, 'd'), ("Install", mut_, 'i'), ("Login", OK, 'l'), ("Logout", mut_, 'o'), ("API key", mut_, 'k'), ("Model", mut_, 'm'), ("Auto", mut_, 'a'), ("Use for new chats", mut_, 'u'), ("Test", mut_, 't'), ("Hide/show", mut_, 'x'), ("Add custom", mut_, 'n'), ("Edit", mut_, 'e'), ("Remove", err, 'R')],
            (Tab::Rag, true) => vec![("Add backend", acc, 'a'), ("Edit", mut_, 'e'), ("Delete", err, 'd'), ("Test", mut_, 't'), ("On/Off", mut_, 'x'), ("Reindex", mut_, 'r'), ("Ingest docs", mut_, 'i')],
            (Tab::Rag, false) => vec![("Reindex", mut_, 'r'), ("Ingest my docs", acc, 'i'), ("Test backend", mut_, 't')],
            (Tab::Vault, true) if self.section == 0 => vec![("New secret", acc, 'n'), ("Delete", err, 'd'), ("Lock now", mut_, 'l'), ("Key file", mut_, 'f')],
            (Tab::Vault, _) => vec![("New secret", acc, 'n'), ("Delete", err, 'd'), ("Reset vault", mut_, 'R')],
            (Tab::Voice, true) => vec![
                ("Speech-to-text", acc, 's'), ("Text-to-speech", acc, 'v'), ("Commands", mut_, 'c'), ("Auto-send", mut_, 'a'), ("Speak replies", mut_, 'r'), ("Clear STT", err, 'S'), ("Clear TTS", err, 'V'),
                ("Test mic", mut_, 'm'), ("Test speaker", mut_, 'p'), ("Test STT", mut_, 't'),
            ],
            (Tab::Voice, false) => vec![("Test mic", mut_, 'm'), ("Test speaker", mut_, 'p'), ("Test STT", mut_, 't')],
            (Tab::IndicVoices, a) => self.indic_buttons(a),
            (Tab::Network, _) => vec![("Turn on/off", acc, 'e'), ("Join WiFi", acc, 'w'), ("Private MAC", mut_, 'm'), ("Apply MAC now", mut_, 'a'), ("Look again", mut_, 'r')],
            (Tab::Bluetooth, _) => vec![("Turn on/off", acc, 'e'), ("Look again", mut_, 'r')],
            (Tab::Display, _) => vec![("Brighter", acc, '+'), ("Dimmer", mut_, '-'), ("Look again", mut_, 'r')],
            (Tab::Sound, _) => vec![("Output", acc, 'o'), ("Test sound", acc, 't'), ("Louder", mut_, '+'), ("Quieter", mut_, '-'), ("Mute", mut_, 'm'), ("Look again", mut_, 'r')],
            (Tab::Storage, _) if self.storage.virtual_disks.is_some() => vec![("New virtual disk", acc, 'n'), ("Delete it", err, 'x'), ("Make data disk", ERR, 'm'), ("Rescan", TEXT, 'r')],
            (Tab::Storage, _) => vec![("Make data disk", ERR, 'm'), ("Rescan", TEXT, 'r'), ("Reboot", TEXT, 'b')],
            (Tab::Skills, _) => {
                let mut v = vec![("From clipboard", acc, 'c'), ("Quick create", mut_, 'a'), ("Import file", mut_, 'i'), ("Delete", err, 'd')];
                if self.share_skills {
                    v.insert(1, ("Share clipboard", mut_, 'C'));
                }
                v
            }
            (Tab::SecretProviders, _) => vec![("Add provider", acc, 'a'), ("Test", mut_, 't'), ("Delete", err, 'd')],
            (Tab::Account, _) => vec![("Change password", acc, 'p'), ("Bigger text", mut_, 'u'), ("Smaller text", mut_, 'U')],
            (Tab::Browser, _) => vec![("Forget allowed sites", err, 'f')],
            (Tab::DevCluster, _) => vec![("Test", acc, 't'), ("My default", mut_, 'd'), ("Workspace default", mut_, 'w'), ("Next workspace", mut_, 'k'), ("Hide/Show", mut_, 'h'), ("Disconnect", err, 'x'), ("Registry", mut_, 'g'), ("Platform services", acc, 'P'), ("Add vault profile", mut_, 'v'), ("Remove it", err, 'V'), ("Look again", mut_, 'r')],
            (Tab::HostAccess, _) if hf_on() => Self::host_buttons(),
            (Tab::HostAccess, _) => vec![],
            (Tab::Memory, _) => self.memory_buttons(),
            (Tab::Time, _) => self.time_buttons(),
            (Tab::HostName, _) => self.host_name_buttons(),
            (Tab::Developer, _) => self.developer_buttons(),
            (Tab::Install, _) => self.install_buttons(),
        }
    }

    /// Rows the button bar needs (buttons wrap).
    fn button_rows(&self, admin: bool) -> usize {
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            0
        } else if self.tab() == Tab::DevCluster {
            self.dc_button_rows()
        } else if (self.tab() == Tab::Voice && admin) || (self.tab() == Tab::IndicVoices && admin) || self.tab() == Tab::Agents || self.tab() == Tab::Memory || self.tab() == Tab::Time || self.tab() == Tab::Sound {
            2
        } else {
            1
        }
    }

    fn render_buttons(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, admin: bool) {
        let (mut bx, mut by) = (x, y);
        for (label, col, key) in self.buttons_for(admin) {
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
            if bx + bw > x + w {
                bx = x;
                by += 40.0 * s;
            }
            bx += g.button(bx, by, 14.0 * s, label, col, Event::Key(Key::Char(key))) + 8.0 * s;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn row(g: &mut Gfx, idx: usize, x: f32, y: f32, w: f32, h: f32, s: f32, selected: bool, title: &str, sub: &str, chips: &[(&str, [u8; 4])]) {
    let hot = g.hovered(R::new(x, y, w, h));
    crate::fx::row_at(g, x, y, w, h, 12.0 * s, selected, hot, ACCENT);
    g.hit(R::new(x, y, w, h), Event::Row(idx), title);
    if selected {
        g.rect(x, y + 10.0 * s, 4.0 * s, h - 20.0 * s, ACCENT);
    }
    g.text(x + 18.0 * s, y + 8.0 * s, 17.0 * s, TEXT, title, Face::Bold);
    let sub = g.fit(13.0 * s, sub, Face::Mono, w - 36.0 * s - 150.0 * s);
    g.text(x + 18.0 * s, y + 34.0 * s, 13.0 * s, MUTED, &sub, Face::Mono);
    let mut cx = x + w - 14.0 * s;
    for (label, col) in chips.iter().rev() {
        let cw = g.text_width(12.0 * s, label, Face::Bold) + 18.0 * s;
        cx -= cw;
        crate::fx::chip(g, cx, y + 10.0 * s, s, label, *col, false);
        cx -= 6.0 * s;
    }
}

impl SettingsApp {
    fn render_models(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let (chat, emb, provs) = {
            let st = cx.services.settings.lock().unwrap();
            (st.get().chat.clone(), st.get().embedding.clone(), st.get().providers.clone())
        };
        let admin = cx.session.policy.manage_settings;
        let fmt = |c: &Option<ModelChoice>, none: &str| c.as_ref().map_or(none.to_string(), |c| format!("{} · {}", c.provider, c.model));
        g.text(a.x, a.y, 15.0 * s, MUTED, "Chat model", Face::Sans);
        g.text(a.x + 150.0 * s, a.y, 15.0 * s, if chat.is_some() { OK } else { ERR }, &fmt(&chat, "none selected"), Face::Bold);
        g.text(a.x, a.y + 24.0 * s, 15.0 * s, MUTED, "Embeddings", Face::Sans);
        g.text(a.x + 150.0 * s, a.y + 24.0 * s, 15.0 * s, TEXT, &fmt(&emb, "built-in (hashing, no model needed)"), Face::Bold);
        let price = cx.services.settings.lock().unwrap().get().token_price_cents_per_mtok;
        g.text(a.x, a.y + 48.0 * s, 15.0 * s, MUTED, "Agent price", Face::Sans);
        g.text(a.x + 150.0 * s, a.y + 48.0 * s, 15.0 * s, TEXT, &if price == 0 { "not set (cost budgets are not enforced)".to_string() } else { format!("{price} cents per million tokens  [p]") }, Face::Bold);
        let mut y = a.y + 86.0 * s;
        if provs.is_empty() {
            let msg = if admin { "No model providers yet. Press a to add Ollama, OpenAI, Claude, Gemini, OpenRouter or any OpenAI-compatible server." } else { "No model providers configured. Ask an administrator." };
            crate::fx::empty_state(g, R::new(a.x, y, a.w, 220.0 * s), s, Icon::Settings, "No model providers yet", msg, ACCENT);
        }
        for (i, p) in provs.iter().enumerate() {
            if y > a.y + a.h - 70.0 * s {
                break;
            }
            let key_ok = match &p.key_ref {
                None => ("no key", DIM),
                Some(r) if r.starts_with("ext:") => ("key: provider", MUTED),
                Some(r) => {
                    if cx.services.secrets.resolve(r, None).is_ok() { ("key ✓", OK) } else { ("key ✗ locked/missing", ERR) }
                }
            };
            let url = if admin { p.base_url.clone().unwrap_or_else(|| p.kind.default_base().unwrap_or("").to_string()) } else { String::new() };
            let sub = format!("{}{}{} models", url, if url.is_empty() { "" } else { "  ·  " }, p.models.len());
            let active = chat.as_ref().is_some_and(|c| c.provider == p.id);
            let chips: Vec<(&str, [u8; 4])> = [Some((p.kind.label(), ACCENT)), Some(key_ok), if active { Some(("chat", OK)) } else { None }, if p.enabled { None } else { Some(("off", ERR)) }].into_iter().flatten().collect();
            let selected = i == self.sel();
            let rh = if selected { 108.0 * s } else { 62.0 * s };
            row(g, i, a.x, y, a.w, rh, s, selected, &p.id, &sub, &chips);
            if selected {
                // the actions of the chosen provider, right on its row
                let mut bx = a.x + 18.0 * s;
                let by = y + 62.0 * s;
                let mut acts: Vec<(&str, [u8; 4], char)> = if admin { vec![("Edit", TEXT, 'e'), ("Delete", ERR, 'd'), ("Models", ACCENT, 'm'), ("Test", TEXT, 't'), ("Key", TEXT, 'k'), (if p.enabled { "Turn off" } else { "Turn on" }, TEXT, 'x')] } else { vec![("Test", ACCENT, 't')] };
                if p.kind == Kind::Lemonade {
                    acts.insert(if admin { 2 } else { 1 }, ("All models", ACCENT, 'l'));
                }
                for (label, col, key) in acts {
                    let bw = g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.7;
                    if bx + bw > a.x + a.w - 12.0 * s {
                        break;
                    }
                    bx += g.button(bx, by, 13.0 * s, label, col, Event::Key(Key::Char(key))) + 8.0 * s;
                }
            }
            y += rh + 8.0 * s;
        }
        if let Some(p) = provs.get(self.sel()).filter(|p| p.kind == Kind::Lemonade) {
            // what this Lemonade server has, by type, with a quick action on each model
            let top = y + 4.0 * s;
            self.render_lemonade(g, R::new(a.x, top, a.w, (a.y + a.h - top).max(0.0)), s, cx, p);
        } else if !provs.is_empty() {
            g.text(a.x + 4.0 * s, y + 2.0 * s, 13.0 * s, MUTED, if admin { "Select a provider, then Delete / Edit / Models" } else { "Select a provider, then Test" }, Face::Sans);
        }
    }

    fn render_rag(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let admin = cx.session.policy.manage_settings;
        let (rag, exts) = {
            let st = cx.services.settings.lock().unwrap();
            (st.get().rag.clone(), st.get().external_rag.clone())
        };
        let (embed_id, ns_stats) = {
            let kb = cx.services.kb.lock().unwrap();
            let nss = if admin { kb.namespaces() } else { vec![format!("user/{}", cx.session.user), "shared".to_string()] };
            (kb.embedder_id(), nss.into_iter().filter_map(|n| kb.stats(&n).map(|st| (n, st))).collect::<Vec<_>>())
        };
        // sliders in two columns when there is room, one when there is not
        let can = admin;
        let two = a.w > 820.0 * s;
        let gap = 10.0 * s;
        let (cw, rh) = if two { ((a.w - gap) / 2.0, 82.0 * s) } else { (a.w, 82.0 * s) };
        let mut y = a.y;
        y += controls::section_title(g, a.x, y, s, "Retrieval");
        let max_overlap = (rag.chunk_max / 2).saturating_sub(1) as i64;
        let specs = [
            (SLIDER_CHUNK, "Chunk size", "Characters of a document in one searchable piece. Bigger keeps more context; smaller finds more exactly.", rag.chunk_max as i64, 100, 4000, "characters", '-', '='),
            (SLIDER_OVERLAP, "Overlap", "Characters shared by neighbouring pieces, so a sentence is not cut off at an edge.", rag.overlap as i64, 0, max_overlap, "characters", ',', '.'),
            (SLIDER_TOPK, "Results", "How many pieces come back for each question.", rag.top_k as i64, 1, 50, "per question", '[', ']'),
        ];
        let mut col = 0usize;
        let mut row_y = y;
        for (id, title, help, value, min, max, unit, minus, plus) in specs {
            let x = if two { a.x + col as f32 * (cw + gap) } else { a.x };
            let tr = controls::slider_row(g, R::new(x, row_y, cw, rh), s, &Slider { id, title, help, value, min, max, unit, minus: Event::Key(Key::Char(minus)), plus: Event::Key(Key::Char(plus)), enabled: can }, ACCENT);
            self.slider_w.push((id, tr.w));
            if two && col == 0 {
                col = 1;
            } else {
                col = 0;
                row_y += rh + gap;
            }
        }
        if col == 1 {
            // the odd one out: the switch sits beside it
            controls::switch_row(g, R::new(a.x + cw + gap, row_y, cw, rh), s, "Search external backends", "Also ask the backends listed below, not only the local documents.", rag.use_external, can, Event::Key(Key::Char('u')), ACCENT);
            row_y += rh + gap;
        } else {
            controls::switch_row(g, R::new(a.x, row_y, a.w, 66.0 * s), s, "Search external backends", "Also ask the backends listed below, not only the local documents.", rag.use_external, can, Event::Key(Key::Char('u')), ACCENT);
            row_y += 66.0 * s + gap;
        }
        let embed_note = if Self::svc(cx).is_reindexing() { format!("{embed_id} — updating documents in the background…") } else { embed_id.clone() };
        controls::info_row(g, R::new(a.x, row_y, a.w, 44.0 * s), s, "Embedding", &embed_note);
        let mut y = row_y + 44.0 * s + 16.0 * s;
        g.text(a.x, y, 14.0 * s, MUTED, "Local knowledge base (r reindex · i ingest documents folder)", Face::Bold);
        y += 24.0 * s;
        if ns_stats.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "empty — press i to index the documents folder", Face::Sans);
            y += 24.0 * s;
        }
        for (n, st) in ns_stats.iter().take(4) {
            let un = if st.unembedded > 0 { format!("  ({} without vectors: press r)", st.unembedded) } else { String::new() };
            g.text(a.x, y, 14.0 * s, TEXT, &format!("{n:<22}{} docs · {} chunks{un}", st.docs, st.chunks), Face::Mono);
            y += 22.0 * s;
        }
        y += 14.0 * s;
        g.text(a.x, y, 14.0 * s, MUTED, "External RAG backends", Face::Bold);
        y += 26.0 * s;
        if exts.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, if admin { "none — press a to add Qdrant, Chroma or a custom HTTP search service" } else { "none configured" }, Face::Sans);
        }
        for (i, e) in exts.iter().enumerate() {
            if y > a.y + a.h - 60.0 * s {
                break;
            }
            let url = if admin { match &e.backend { ExternalRag::Qdrant { url, collection, .. } | ExternalRag::Chroma { url, collection, .. } => format!("{url}  ·  {collection}"), ExternalRag::Http { url, .. } => url.clone() } } else { String::new() };
            let chips: Vec<(&str, [u8; 4])> = vec![(e.backend.label(), ACCENT), if e.enabled { ("on", OK) } else { ("off", ERR) }];
            row(g, i, a.x, y, a.w, 58.0 * s, s, i == self.sel(), e.backend.id(), &url, &chips);
            y += 66.0 * s;
        }
    }

    fn render_vault(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let admin = cx.session.policy.manage_settings;
        let svc = &cx.services;
        let system = self.section == 0 && admin;
        let (exists, unlocked) = svc.system_vault_state();
        let mut y = a.y;
        let pill = |g: &mut Gfx, x: f32, y: f32, label: &str, on: bool| {
            let w = g.text_width(14.0 * s, label, Face::Bold) + 28.0 * s;
            let hot = g.hovered(R::new(x, y, w, 30.0 * s));
            if on {
                g.rrect_glow(x, y, w, 30.0 * s, 15.0 * s, 4.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 80]);
                g.rrect(x, y, w, 30.0 * s, 15.0 * s, ACCENT);
            } else {
                crate::fx::row_at(g, x, y, w, 30.0 * s, 15.0 * s, false, hot, ACCENT);
            }
            if !on {
                g.hit(R::new(x, y, w, 30.0 * s), Event::Key(Key::Tab), label); // switches system <-> personal
            }
            g.text(x + 14.0 * s, y + 6.0 * s, 14.0 * s, if on { rgb(10, 14, 28) } else { MUTED }, label, Face::Bold);
            w + 8.0 * s
        };
        let mut px = a.x;
        if admin {
            px += pill(g, px, y, "System vault", system);
        }
        pill(g, px, y, "My vault", !system);
        y += 44.0 * s;
        if system {
            let st = if !exists { "not created yet (created when an admin first signs in)".to_string() } else if unlocked { "unlocked".to_string() } else { "LOCKED — sign in as an admin with access".to_string() };
            g.text(a.x, y, 15.0 * s, if unlocked { OK } else { ERR }, &format!("Status: {st}"), Face::Bold);
            y += 24.0 * s;
            let slots = svc.system_slots().join(", ");
            let slots = g.fit(13.0 * s, &format!("Key holders: {}", if slots.is_empty() { "-".into() } else { slots }), Face::Sans, a.w);
            g.text(a.x, y, 13.0 * s, MUTED, &slots, Face::Sans);
            y += 30.0 * s;
        } else {
            let un = svc.user_vault_unlocked(&cx.session.user);
            g.text(a.x, y, 15.0 * s, if un { OK } else { ERR }, &format!("Status: {}", if un { "unlocked with your sign-in password" } else { "LOCKED (press R to start a fresh one)" }), Face::Bold);
            y += 30.0 * s;
        }
        match self.secret_names(cx) {
            Err(e) => {
                g.text(a.x, y, 15.0 * s, ERR, &e.to_string(), Face::Sans);
            }
            Ok(names) => {
                if names.is_empty() {
                    g.text(a.x, y, 15.0 * s, DIM, "No secrets yet. Press n to add one. Values are never displayed after saving.", Face::Sans);
                }
                for (i, n) in names.iter().enumerate() {
                    if y > a.y + a.h - 60.0 * s {
                        break;
                    }
                    row(g, i, a.x, y, a.w, 48.0 * s, s, i == self.sel(), n, &format!("reference:  {}:{n}", if system { "vault" } else { "uvault" }), &[("••••••••", DIM)]);
                    y += 56.0 * s;
                }
            }
        }
    }

    fn render_secret_providers(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let list = Self::secret_providers(cx);
        g.text(a.x, a.y, 14.0 * s, MUTED, "External vaults. Use their secrets anywhere a reference is accepted:  ext:<alias>/<path>#<field>", Face::Sans);
        let mut y = a.y + 34.0 * s;
        if list.is_empty() {
            g.text(a.x, y, 15.0 * s, DIM, "None configured. Press a to connect HashiCorp Vault / OpenBao or 1Password Connect.", Face::Sans);
        }
        for (i, (alias, cfg)) in list.iter().enumerate() {
            let (label, addr) = match cfg {
                SecretProvider::Hashicorp { addr, mount, namespace, .. } => ("HashiCorp/OpenBao", format!("{addr}  ·  mount {mount}{}", namespace.as_ref().map(|n| format!("  ·  ns {n}")).unwrap_or_default())),
                SecretProvider::Onepassword { addr, .. } => ("1Password Connect", addr.clone()),
            };
            row(g, i, a.x, y, a.w, 58.0 * s, s, i == self.sel(), alias, &addr, &[(label, ACCENT)]);
            y += 66.0 * s;
        }
    }

    /// The size of text and buttons, for whoever is signed in (every account has it on the Account page, administrators also
    /// on Display). Returns the height used.
    fn render_ui_size_card(&mut self, g: &mut Gfx, card: R, s: f32, cx: &AppCx) -> f32 {
        let cur = crate::uisize::load(cx.services.dir(), &cx.session.user);
        crate::fx::glass(g, card.x, card.y, card.w, card.h, 14.0 * s, ACCENT);
        g.text(card.x + 18.0 * s, card.y + 10.0 * s, 16.0 * s, TEXT, "Text and button size", Face::Bold);
        g.text(card.x + 18.0 * s, card.y + 34.0 * s, 13.0 * s, MUTED, "Applies to you at every sign-in. u makes it bigger, Shift+U smaller.", Face::Sans);
        let mut bx = card.x + card.w - 18.0 * s;
        for (i, (name, f)) in crate::uisize::LEVELS.iter().enumerate().rev() {
            let label = format!("{name} {}%", (f * 100.0).round());
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.7;
            bx -= w;
            let ev = if i > cur { Event::Key(Key::Char('u')) } else { Event::Key(Key::Char('U')) };
            if i == cur {
                g.button_off(bx, card.y + 20.0 * s, 13.0 * s, &label);
            } else {
                g.button(bx, card.y + 20.0 * s, 13.0 * s, &label, MUTED, ev);
            }
            bx -= 6.0 * s;
        }
        card.h
    }

    fn render_account(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let p = &cx.session.policy;
        let lines = [
            format!("User        {}", cx.session.user),
            format!("Role        {}", cx.session.role.name()),
            format!("Compute     {} instructions per run", p.max_fuel),
            format!("Memory      {} MiB per task", p.max_memory_mb),
            format!("Storage     {} MiB", p.vfs_quota_mb),
            format!("Apps        {}", p.apps.iter().map(|x| x.id()).collect::<Vec<_>>().join(", ")),
            String::new(),
            "Press p to change your password. Your personal vault is re-keyed automatically.".into(),
            "If an administrator resets your password, your personal vault cannot be recovered (by design).".into(),
        ];
        for (i, l) in lines.iter().enumerate() {
            let l = g.fit(15.0 * s, l, Face::Mono, a.w);
            g.text(a.x, a.y + i as f32 * 26.0 * s, 15.0 * s, if i < 6 { TEXT } else { MUTED }, &l, Face::Mono);
        }
        self.render_ui_size_card(g, R::new(a.x, a.y + lines.len() as f32 * 26.0 * s + 14.0 * s, a.w, 74.0 * s), s, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
    use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
    use iyakki_sec::policy::Role;
    use std::sync::Mutex;

    type Reply = Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>;
    struct Mock {
        calls: Mutex<Vec<(String, String)>>,
        reply: Reply,
    }
    impl Upstream for Mock {
        fn request(&self, m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((m.into(), url.into()));
            let (status, body) = (self.reply)(m, url);
            Ok(UpstreamResponse { status, body: body.into_bytes() })
        }
    }

    fn ollama_like(m: &str, url: &str) -> (u16, String) {
        match (m, url) {
            ("GET", u) if u.ends_with("/v1/models") => (200, r#"{"data":[{"id":"qwen3:32b"},{"id":"llama3.2:3b"}]}"#.into()),
            ("POST", u) if u.ends_with("/chat/completions") => (200, r#"{"choices":[{"message":{"content":"OK"}}]}"#.into()),
            ("POST", u) if u == "http://rag.corp/search" => (200, r#"{"results":[{"text":"x"}]}"#.into()),
            ("GET", u) if u.ends_with("/v1/auth/token/lookup-self") => (200, "{}".into()),
            _ => (404, String::new()),
        }
    }

    /// Admin session with a real Services over a mock network; the system vault is unlocked.
    fn admin(tag: &str) -> (AppCx, std::path::PathBuf) {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Mock { calls: Default::default(), reply: Box::new(ollama_like) }));
        cx.services.on_login("root", "rootpass12", true);
        (cx, d)
    }

    fn user(tag: &str) -> AppCx {
        let (mut cx, _) = admin(tag);
        cx.session.user = "amy".into();
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.services.on_login("amy", "amypass-123", false);
        cx
    }

    fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }
    fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }
    fn settle(a: &mut SettingsApp, cx: &AppCx) {
        for _ in 0..500 {
            a.handle(&Event::Tick, cx);
            if !a.is_busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("job did not finish");
    }
    fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
        while a.tab() != t {
            key(a, cx, Key::Right);
        }
    }
    fn status(a: &SettingsApp) -> String {
        a.status().map(|s| s.0.clone()).unwrap_or_default()
    }

    #[test]
    fn admin_adds_ollama_tests_it_and_picks_a_model() {
        let (cx, _) = admin("set1");
        let mut a = SettingsApp::new(&cx);
        key(&mut a, &cx, Key::Char('a'));
        assert!(a.has_modal());
        typ(&mut a, &cx, "local");
        for _ in 0..5 {
            key(&mut a, &cx, Key::Enter); // id → type → url → key → model → submit
        }
        assert!(!a.has_modal(), "{}", status(&a));
        let p = cx.services.settings.lock().unwrap().get().providers.clone();
        assert_eq!((p.len(), p[0].id.as_str(), p[0].kind), (1, "local", Kind::Ollama));
        assert_eq!(p[0].base_url.as_deref(), Some("http://localhost:11434"));
        // test connection fetches and caches the model list
        key(&mut a, &cx, Key::Char('t'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("2 model(s)"), "{}", status(&a));
        assert_eq!(cx.services.settings.lock().unwrap().get().providers[0].models, vec!["llama3.2:3b", "qwen3:32b"]);
        // choose the chat model by filtering the picker
        key(&mut a, &cx, Key::Char('m'));
        typ(&mut a, &cx, "qwen");
        key(&mut a, &cx, Key::Enter);
        let chat = cx.services.settings.lock().unwrap().get().chat.clone().unwrap();
        assert_eq!((chat.provider.as_str(), chat.model.as_str()), ("local", "qwen3:32b"));
        // and the chat app's live backend now answers through it
        key(&mut a, &cx, Key::Char('c'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("replied: OK"), "{}", status(&a));
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.chat_model") && l.contains("local/qwen3:32b")));
    }

    #[test]
    fn api_keys_go_to_the_vault_never_to_settings_or_logs() {
        let (cx, d) = admin("set2");
        let mut a = SettingsApp::new(&cx);
        key(&mut a, &cx, Key::Char('a'));
        typ(&mut a, &cx, "openai");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Right); // Ollama → OpenAI: URL default follows
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "sk-super-secret-value");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal(), "{:?}", a.status());
        let st = cx.services.settings.lock().unwrap().get().clone();
        assert_eq!(st.providers[0].base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(st.providers[0].key_ref.as_deref(), Some("vault:openai-key"));
        assert_eq!(&*cx.services.secrets.resolve("vault:openai-key", None).unwrap(), "sk-super-secret-value");
        let settings_file = std::fs::read_to_string(cx.services.dir().join("settings.json")).unwrap();
        assert!(!settings_file.contains("sk-super-secret-value"));
        let audit = std::fs::read_to_string(d.parent().unwrap().join(format!("iyakki-shell-{}-set2-x/audit.log", std::process::id()))).unwrap_or_default();
        assert!(!audit.contains("sk-super-secret-value"));
        assert!(cx.audit.recent(50).iter().all(|l| !l.contains("sk-super-secret-value")));
        // the vault file itself is encrypted
        let vault = std::fs::read_to_string(cx.services.dir().join("vault/system.afv")).unwrap();
        assert!(!vault.contains("sk-super-secret-value"));
    }

    #[test]
    fn a_rejected_form_stores_nothing() {
        let (cx, _) = admin("set3");
        let mut a = SettingsApp::new(&cx);
        // OpenAI needs a key: submitting without one is refused and the form stays open
        key(&mut a, &cx, Key::Char('a'));
        typ(&mut a, &cx, "openai");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Right);
        for _ in 0..4 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(a.has_modal());
        assert!(cx.services.settings.lock().unwrap().get().providers.is_empty());
        // a locked system vault makes saving a key fail cleanly (no half-saved provider)
        cx.services.lock_system();
        key(&mut a, &cx, Key::Tab); // wrap focus
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "sk-abc");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal());
        assert!(cx.services.settings.lock().unwrap().get().providers.is_empty(), "no provider without its key");
        // Esc closes the dialog
        key(&mut a, &cx, Key::Esc);
        assert!(!a.has_modal());
    }

    #[test]
    fn host_access_says_why_it_is_unavailable_and_in_window_mode_lists_mounts_with_actions_and_the_audit_trail() {
        let _g = iyakki_agent::hostfiles::test_guard();
        let (cx, _) = admin("hostacc");
        let mut a = SettingsApp::new(&cx);
        assert!(a.tabs().contains(&Tab::HostAccess));
        // not window mode: a plain explanation, no actions
        iyakki_agent::hostfiles::set_enabled(false);
        goto(&mut a, &cx, Tab::HostAccess);
        let g = draw(&mut a, &cx, 1280, 960);
        assert!(!g.hits.iter().any(|h| h.label == "Add folder…"), "no actions off window mode");
        assert!(SettingsApp::host_rows(&cx).is_empty());
        // window mode
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-hostacc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Music/Rock")).unwrap();
        std::fs::write(base.join("Music/Rock/a.mp3"), vec![0u8; 2048]).unwrap();
        iyakki_agent::hostfiles::set_enabled(true);
        iyakki_agent::hostfiles::set_host_home(Some(base.clone()));
        let g = draw(&mut a, &cx, 1280, 960);
        for want in ["Add folder…", "Options…", "Change mode", "Convert to copy", "Unlink", "Import files…", "Audit trail"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        let mut m = iyakki_agent::hostfiles::HostMounts::load(cx.services.dir(), &cx.session.user);
        m.add(&base, "~/Music", None, false).unwrap();
        let g = draw(&mut a, &cx, 1280, 960);
        assert!(g.hits.iter().any(|h| h.label == "host:Music" && matches!(h.ev, Event::Row(0))), "the folder is a clickable row");
        // change mode (audited), open the options, and the audit trail
        key(&mut a, &cx, Key::Char('m'));
        assert!(SettingsApp::host_rows(&cx)[0].0.write, "now read-write");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("host.mount.mode")));
        key(&mut a, &cx, Key::Char('m'));
        assert!(!SettingsApp::host_rows(&cx)[0].0.write);
        // the first use explains itself; once agreed the options open
        key(&mut a, &cx, Key::Char('o'));
        let g = draw(&mut a, &cx, 1280, 960);
        assert!(g.hits.iter().any(|h| h.label == "I understand, continue"), "the explanation comes first");
        key(&mut a, &cx, Key::Esc);
        std::fs::write(cx.services.dir().join(format!("host-consent-{}", cx.session.user)), "agreed\n").unwrap();
        key(&mut a, &cx, Key::Char('o'));
        let g = draw(&mut a, &cx, 1280, 960);
        assert!(g.hits.iter().any(|h| h.label.contains("Stop sharing")) && g.hits.iter().any(|h| h.label.contains("Convert to a copy")), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('l'));
        let g = draw(&mut a, &cx, 1280, 960);
        assert!(!g.hits.is_empty());
        // unplugged: listed with its status
        std::fs::rename(base.join("Music"), base.join("Music-away")).unwrap();
        let rows = SettingsApp::host_rows(&cx);
        assert_eq!((rows.len(), rows[0].1), (1, false), "listed, not available");
        key(&mut a, &cx, Key::Char('l'));
        let _ = draw(&mut a, &cx, 1280, 960);
        iyakki_agent::hostfiles::set_host_home(None);
        iyakki_agent::hostfiles::set_enabled(false);
    }


    #[test]
    fn non_admins_cannot_change_system_settings_and_it_is_audited() {
        let cx = user("set4");
        let mut a = SettingsApp::new(&cx);
        assert_eq!(a.tabs(), &[Tab::Models, Tab::Rag, Tab::Vault, Tab::Skills, Tab::Voice, Tab::Time, Tab::Browser, Tab::HostAccess, Tab::Account], "no secret-provider tab for users; everyone has Time");
        for k in ['a', 'e', 'd', 'k', 'x'] {
            key(&mut a, &cx, Key::Char(k));
            assert!(!a.has_modal(), "{k}");
        }
        assert_eq!(a.status().unwrap().1, false);
        assert!(status(&a).contains("administrators"));
        goto(&mut a, &cx, Tab::Rag);
        key(&mut a, &cx, Key::Char(']'));
        key(&mut a, &cx, Key::Char('a'));
        assert!(!a.has_modal());
        assert_eq!(cx.services.settings.lock().unwrap().get().rag.top_k, 5, "unchanged");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
        // even a hand-built admin form submitted by a non-admin is refused at save time
        let mut forced = SettingsApp::new(&cx);
        let form = SettingsApp::provider_form(&cx, None).text("x", "", "");
        forced.modal = Some(Modal::Form(FormKind::Provider(None), form));
        key(&mut forced, &cx, Key::Enter);
        assert!(cx.services.settings.lock().unwrap().get().providers.is_empty());
    }

    #[test]
    fn personal_vault_is_private_to_the_user() {
        let cx = user("set5");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Vault);
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "github-token");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "ghp_personal_secret");
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal(), "{:?}", a.status());
        assert_eq!(cx.services.user_names("amy").unwrap(), vec!["github-token"]);
        // the system vault is not reachable from a user session at all
        assert!(cx.services.system_names().is_err() || !cx.session.policy.manage_settings);
        key(&mut a, &cx, Key::Char('l')); // lock is a system-vault action: ignored for users
        assert!(cx.services.user_vault_unlocked("amy"));
        // delete with confirmation
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('n')); // anything but y cancels
        assert_eq!(cx.services.user_names("amy").unwrap().len(), 1);
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(cx.services.user_names("amy").unwrap().is_empty());
    }

    #[test]
    fn system_vault_secrets_lock_and_keyfile() {
        let (cx, d) = admin("set6");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Vault);
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "slack-webhook");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "https://hooks.example/abc");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(cx.services.system_names().unwrap(), vec!["slack-webhook"]);
        // invalid names are rejected in the form
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "Bad Name");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "v");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "form stays open with the reason");
        key(&mut a, &cx, Key::Esc);
        // key file
        key(&mut a, &cx, Key::Char('f'));
        for _ in 0..80 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, &d.join("unattended.key").display().to_string());
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal(), "{:?}", a.status());
        assert!(d.join("unattended.key").exists());
        // lock now
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(cx.services.system_vault_state(), (true, false));
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
    }

    #[test]
    fn rag_tuning_external_backends_ingest_and_reindex() {
        let (mut cx, d) = admin("set7");
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::write(docs.join("shared/a.md"), "Deploy keys live in the vault under prod.").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Rag);
        key(&mut a, &cx, Key::Char(']'));
        key(&mut a, &cx, Key::Char('='));
        key(&mut a, &cx, Key::Char('.'));
        key(&mut a, &cx, Key::Char('u'));
        let r = cx.services.settings.lock().unwrap().get().rag.clone();
        assert_eq!((r.top_k, r.chunk_max, r.overlap, r.use_external), (6, 800, 120, false));
        key(&mut a, &cx, Key::Char('u'));
        // add a generic HTTP backend
        key(&mut a, &cx, Key::Char('a'));
        typ(&mut a, &cx, "corp");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Right);
        key(&mut a, &cx, Key::Right); // → HTTP
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "http://rag.corp/search");
        for _ in 0..3 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(!a.has_modal(), "{:?}", a.status());
        assert_eq!(cx.services.settings.lock().unwrap().get().external_rag.len(), 1);
        key(&mut a, &cx, Key::Char('t'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("endpoint responded"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('x'));
        assert!(!cx.services.settings.lock().unwrap().get().external_rag[0].enabled);
        // ingest the shared folder, then reindex
        key(&mut a, &cx, Key::Char('i'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("1 new document"), "{}", status(&a));
        assert_eq!(cx.services.kb.lock().unwrap().stats("shared").unwrap().docs, 1);
        key(&mut a, &cx, Key::Char('i'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("0 new document"), "idempotent: {}", status(&a));
        key(&mut a, &cx, Key::Char('r'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("Re-embedded"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(cx.services.settings.lock().unwrap().get().external_rag.is_empty());
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
    }

    #[test]
    fn external_secret_provider_roundtrip() {
        let (cx, _) = admin("set8");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::SecretProviders);
        key(&mut a, &cx, Key::Char('a'));
        typ(&mut a, &cx, "corp");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter); // type: HashiCorp (default)
        typ(&mut a, &cx, "http://bao:8200");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "s.root-token");
        for _ in 0..3 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(!a.has_modal(), "{:?}", a.status());
        assert_eq!(&*cx.services.secrets.resolve("vault:corp-token", None).unwrap(), "s.root-token");
        key(&mut a, &cx, Key::Char('t'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("reachable"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(cx.services.settings.lock().unwrap().get().secret_providers.is_empty());
    }

    #[test]
    fn password_change_rekeys_the_personal_vault() {
        let (cx, _) = admin("set9");
        // root's account exists in the store with password rootpass12
        cx.services.user_set("root", "tok", "personal").unwrap();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Account);
        key(&mut a, &cx, Key::Char('p'));
        typ(&mut a, &cx, "wrong-old-pw");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "brand-new-pw1");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "brand-new-pw1");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "wrong current password keeps the form open");
        // fix the first field and retry (focus is on the last field; one Tab wraps to the first)
        key(&mut a, &cx, Key::Tab);
        for _ in 0..20 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "rootpass12");
        for _ in 0..2 {
            key(&mut a, &cx, Key::Enter);
        }
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal(), "{:?}", a.status());
        assert!(cx.store.lock().unwrap().authenticate("root", "brand-new-pw1").is_ok());
        cx.services.on_logout("root", true);
        assert_eq!(cx.services.on_login("root", "brand-new-pw1", true).user_vault, iyakki_agent::services::VaultState::Unlocked);
        assert_eq!(cx.services.user_names("root").unwrap(), vec!["tok"], "personal secrets survive the change");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("account.password")));
    }

    #[test]
    fn mismatched_new_passwords_are_rejected() {
        let (cx, _) = admin("set10");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Account);
        key(&mut a, &cx, Key::Char('p'));
        typ(&mut a, &cx, "rootpass12");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "new-password-1");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "new-password-2");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal());
        assert!(cx.store.lock().unwrap().authenticate("root", "rootpass12").is_ok(), "nothing changed");
    }

    #[test]
    fn every_tab_renders_for_admin_and_user_at_several_sizes() {
        for (w, h) in [(1280u32, 720u32), (800, 600), (640, 480)] {
            for admin_view in [true, false] {
                let (cx, _) = admin(&format!("set-r-{w}-{admin_view}"));
                let cx = if admin_view { cx } else { user(&format!("set-ru-{w}")) };
                let mut a = SettingsApp::new(&cx);
                let mut g = Gfx::new(w, h);
                for _ in 0..a.tabs().len() {
                    g.clear(rgb(0, 0, 0));
                    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).max(0.6), &cx, 3);
                    key(&mut a, &cx, Key::Right);
                }
            }
        }
    }

    // ------------------------------------------------------------------ voice tab

    fn fake_audio(d: &std::path::Path, silent: bool) -> iyakki_agent::voice::VoiceSettings {
        use iyakki_agent::voice::{encode_wav, tone};
        let pcm: Vec<u8> = if silent { vec![0u8; 32_000] } else { (0..16_000).flat_map(|i| (((i as f32 * 0.2).sin() * 9000.0) as i16).to_le_bytes()).collect() };
        std::fs::write(d.join("mic.raw"), pcm).unwrap();
        std::fs::write(d.join("canned.wav"), encode_wav(&tone(500.0, 200), 16_000)).unwrap();
        std::fs::write(d.join("whisper.sh"), "#!/bin/sh\necho 'hello from the test'\n").unwrap();
        let mut v = iyakki_agent::voice::VoiceSettings::default();
        v.record_cmd = format!("sh -c 'cat {}; sleep 30'", d.join("mic.raw").display());
        v.play_cmd = format!("sh -c 'cat > {}'", d.join("played.wav").display());
        v.stt = Some(SpeechEngine::Command { command: format!("sh {} {{wav}}", d.join("whisper.sh").display()) });
        v.tts = Some(SpeechEngine::Command { command: format!("sh -c 'cat >/dev/null; cat {}'", d.join("canned.wav").display()) });
        v
    }

    #[test]
    fn browser_voice_is_the_third_kind_of_text_to_speech_and_never_a_speech_to_text_kind() {
        let (cx, _) = admin("sv-bv");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        // Type -> Right, Right: the browser voice; the other fields are not needed
        key(&mut a, &cx, Key::Char('v'));
        key(&mut a, &cx, Key::Right);
        key(&mut a, &cx, Key::Right);
        for _ in 0..5 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(!a.has_modal(), "{:?}", a.status());
        assert_eq!(cx.services.voice_settings().tts, Some(SpeechEngine::Browser));
        // the form shows it chosen the next time, and the screen names it
        key(&mut a, &cx, Key::Char('v'));
        let again = SettingsApp::engine_form(&cx, false);
        assert_eq!(again.choice_idx(0), 2);
        key(&mut a, &cx, Key::Esc);
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
        // speech-to-text has two kinds only
        let stt = SettingsApp::engine_form(&cx, true);
        assert_eq!(stt.choice_idx(0), 0);
        // with nothing chosen, a browser that can speak is named as the default
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = None).unwrap();
        let hv = iyakki_agent::hostvoice::HostVoice::current();
        hv.connect(iyakki_agent::hostvoice::Front::Browser);
        hv.event(iyakki_agent::hostvoice::FromClient::Voices { supported: true, voices: vec![] });
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    }

    #[test]
    fn voice_tab_visibility_follows_the_role() {
        let (cx, _) = admin("sv0");
        assert!(SettingsApp::new(&cx).tabs().contains(&Tab::Voice));
        let ucx = user("sv0u");
        assert!(SettingsApp::new(&ucx).tabs().contains(&Tab::Voice), "users with voice may test their hardware");
        let mut gcx = user("sv0g");
        gcx.session.policy.voice = false;
        assert!(!SettingsApp::new(&gcx).tabs().contains(&Tab::Voice), "no voice permission, no tab");
    }

    #[test]
    fn admin_configures_engines_and_the_key_goes_to_the_vault() {
        let (cx, _) = admin("sv1");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        // HTTP speech-to-text with an API key
        key(&mut a, &cx, Key::Char('s'));
        key(&mut a, &cx, Key::Enter); // type: HTTP (default) -> URL
        key(&mut a, &cx, Key::Ctrl('a'));
        typ(&mut a, &cx, "http://192.168.1.9:8080/v1/audio/transcriptions");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "whisper-large");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "sk-voice-secret");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "en");
        key(&mut a, &cx, Key::Enter);
        assert!(!a.has_modal(), "{:?}", a.status());
        let v = cx.services.voice_settings();
        match v.stt.unwrap() {
            SpeechEngine::Http { url, model, key_ref, language, .. } => {
                assert_eq!((url.as_str(), model.as_str(), key_ref.as_deref(), language.as_deref()), ("http://192.168.1.9:8080/v1/audio/transcriptions", "whisper-large", Some("vault:stt-key"), Some("en")));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(&*cx.services.secrets.resolve("vault:stt-key", None).unwrap(), "sk-voice-secret");
        assert!(!std::fs::read_to_string(cx.services.dir().join("settings.json")).unwrap().contains("sk-voice-secret"));
        // local command text-to-speech
        key(&mut a, &cx, Key::Char('v'));
        key(&mut a, &cx, Key::Right); // Local command
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Ctrl('a'));
        typ(&mut a, &cx, "piper -m voice.onnx -f {out}");
        for _ in 0..4 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(!a.has_modal(), "{:?}", a.status());
        assert_eq!(cx.services.voice_settings().tts, Some(SpeechEngine::Command { command: "piper -m voice.onnx -f {out}".into() }));
        // invalid input keeps the dialog open
        key(&mut a, &cx, Key::Char('s'));
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Ctrl('a'));
        typ(&mut a, &cx, "ftp://nope");
        for _ in 0..4 {
            key(&mut a, &cx, Key::Enter);
        }
        assert!(a.has_modal(), "bad URL is rejected");
        key(&mut a, &cx, Key::Esc);
        // toggles and clearing
        key(&mut a, &cx, Key::Char('a'));
        key(&mut a, &cx, Key::Char('r'));
        let v = cx.services.voice_settings();
        assert!(v.auto_send && v.speak_replies);
        key(&mut a, &cx, Key::Char('S'));
        key(&mut a, &cx, Key::Char('V'));
        let v = cx.services.voice_settings();
        assert!(v.stt.is_none() && v.tts.is_none());
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    }

    #[test]
    fn hardware_tests_report_what_they_find() {
        let (cx, d) = admin("sv2");
        let dir = d.join("audio");
        std::fs::create_dir_all(&dir).unwrap();
        cx.services.settings.lock().unwrap().update(|s| s.voice = fake_audio(&dir, false)).unwrap();
        let mut a = SettingsApp::new(&cx);
        a.test_record_ms = 300;
        goto(&mut a, &cx, Tab::Voice);
        key(&mut a, &cx, Key::Char('m'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("Microphone works") && status(&a).contains("peak level"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('p'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("test tone"), "{}", status(&a));
        assert!(std::fs::read(dir.join("played.wav")).unwrap().starts_with(b"RIFF"), "a real WAV reached the speaker command");
        key(&mut a, &cx, Key::Char('t'));
        settle(&mut a, &cx);
        assert_eq!(status(&a), "Heard: “hello from the test”");
        let _ = std::fs::remove_file(dir.join("played.wav"));
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("Synthesised"), "{}", status(&a));
        assert!(dir.join("played.wav").exists());
        // a silent microphone is reported as silent
        cx.services.settings.lock().unwrap().update(|s| s.voice = fake_audio(&dir, true)).unwrap();
        key(&mut a, &cx, Key::Char('m'));
        settle(&mut a, &cx);
        assert_eq!(a.status().unwrap().1, false);
        assert!(status(&a).contains("silent"), "{}", status(&a));
        // a missing tool names the fix
        cx.services.settings.lock().unwrap().update(|s| s.voice.record_cmd = "/no/such/recorder".into()).unwrap();
        key(&mut a, &cx, Key::Char('m'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("cannot start") && status(&a).contains("Settings"), "{}", status(&a));
    }

    #[test]
    fn users_can_test_but_not_change_voice_settings() {
        let cx = user("sv3");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        for k in ['s', 'v', 'c', 'a', 'r', 'S', 'V'] {
            key(&mut a, &cx, Key::Char(k));
            assert!(!a.has_modal(), "{k}");
        }
        assert!(status(&a).contains("administrators"));
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
        // testing needs the voice permission (and is audited)
        key(&mut a, &cx, Key::Char('p'));
        assert!(a.is_busy() || status(&a).contains("Played") || a.status().is_some());
        settle(&mut a, &cx);
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("voice.test")));
        let mut no_voice = user("sv3b");
        no_voice.session.policy.voice = false;
        let mut b = SettingsApp::new(&no_voice);
        b.tab = 0;
        b.tabs.push(Tab::Voice); // even if the tab were reached, the tests refuse
        b.tab = b.tabs.len() - 1;
        key(&mut b, &no_voice, Key::Char('m'));
        assert!(!b.is_busy() && status(&b).contains("not enabled"));
        assert!(no_voice.audit.recent(10).iter().any(|l| l.contains("voice.denied")));
    }

    // ------------------------------------------------------------------ skills tab

    const SKILL_MD: &str = "---\nname: meeting-notes\ndescription: Summarise meeting notes\ntriggers: meeting, minutes\n---\nList decisions first.\n";

    #[test]
    fn users_keep_personal_skills_from_clipboard_form_and_file() {
        let cx = user("skills-user");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Skills);
        // from the clipboard (newlines preserved)
        crate::ui::clipboard::copy(SKILL_MD);
        key(&mut a, &cx, Key::Char('c'));
        assert!(status(&a).contains("Saved mine skill `meeting-notes`"), "{}", status(&a));
        let l = cx.services.skills.list("amy");
        assert_eq!((l.len(), l[0].body.as_str()), (1, "List decisions first."));
        assert_eq!(a.list_len(&cx), 1);
        // quick-create form (a user has no Scope choice beyond "Mine")
        key(&mut a, &cx, Key::Char('a'));
        typ(&mut a, &cx, "code-review");
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "Review a diff for bugs");
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "review, diff");
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "Look for bugs before style.");
        // Ctrl+S submits
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(!a.has_modal(), "{}", status(&a));
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::User("amy".into()), "code-review").is_some());
        // import from a file inside the user's read scope; outside it is refused
        let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-skill-import-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("tone.md"), SKILL_MD.replace("meeting-notes", "tone")).unwrap();
        let mut cx2 = cx.clone();
        cx2.session.policy.fs_read = vec![home.display().to_string()];
        let mut b = SettingsApp::new(&cx2);
        goto(&mut b, &cx2, Tab::Skills);
        key(&mut b, &cx2, Key::Char('i'));
        typ(&mut b, &cx2, &home.join("tone.md").display().to_string());
        b.handle(&Event::Key(Key::Ctrl('s')), &cx2);
        assert!(cx2.services.skills.get(&iyakki_agent::skills::Scope::User("amy".into()), "tone").is_some(), "{}", status(&b));
        key(&mut b, &cx2, Key::Char('i'));
        typ(&mut b, &cx2, "/etc/passwd");
        b.handle(&Event::Key(Key::Ctrl('s')), &cx2);
        assert!(b.has_modal(), "a refused import keeps the form open");
        assert!(cx2.audit.recent(20).iter().any(|l| l.contains("skills.add")));
    }

    #[test]
    fn shared_skills_need_the_permission_and_deleting_asks_first() {
        let cx = user("skills-shared");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Skills);
        crate::ui::clipboard::copy(SKILL_MD);
        key(&mut a, &cx, Key::Char('C'));
        assert!(cx.services.skills.list_scope(&iyakki_agent::skills::Scope::Shared).is_empty(), "plain users cannot add shared skills");
        assert!(!status(&a).starts_with("Saved") && a.status().unwrap().1 == false);
        // a forged form that asks for the shared scope is refused too
        assert!(SettingsApp::add_skill(&mut a, &cx, SKILL_MD, true).is_err());
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.denied")));
        // operators can; users then see it but cannot delete it
        let mut op = cx.clone();
        op.session.role = Role::Operator;
        op.session.policy = Role::Operator.template();
        let mut o = SettingsApp::new(&op);
        goto(&mut o, &op, Tab::Skills);
        crate::ui::clipboard::copy(SKILL_MD);
        key(&mut o, &op, Key::Char('C'));
        assert_eq!(cx.services.skills.list_scope(&iyakki_agent::skills::Scope::Shared).len(), 1, "{}", status(&o));
        let mut a2 = SettingsApp::new(&cx);
        goto(&mut a2, &cx, Tab::Skills);
        assert_eq!(a2.list_len(&cx), 1);
        key(&mut a2, &cx, Key::Char('d'));
        assert!(!a2.has_modal() && status(&a2).contains("Only operators"));
        // the operator confirms before deleting
        key(&mut o, &op, Key::Char('d'));
        assert!(o.has_modal());
        key(&mut o, &op, Key::Char('n'));
        assert_eq!(cx.services.skills.list_scope(&iyakki_agent::skills::Scope::Shared).len(), 1, "declined");
        key(&mut o, &op, Key::Char('d'));
        key(&mut o, &op, Key::Char('y'));
        assert!(cx.services.skills.list_scope(&iyakki_agent::skills::Scope::Shared).is_empty());
    }

    #[test]
    fn skills_tab_follows_the_permission_and_renders() {
        let mut cx = user("skills-vis");
        cx.session.policy.skills = false;
        assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Skills));
        let cx = user("skills-vis2");
        cx.services.skills.add(iyakki_agent::skills::Scope::User("amy".into()), SKILL_MD, false, true).unwrap();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Skills);
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "meeting-notes"), "the skill row is clickable");
    }

    #[test]
    fn admin_sets_the_token_price_and_bad_values_are_rejected() {
        let (cx, _) = admin("set-price");
        let mut a = SettingsApp::new(&cx);
        key(&mut a, &cx, Key::Char('p'));
        assert!(a.has_modal());
        typ(&mut a, &cx, "250");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(cx.services.settings.lock().unwrap().get().token_price_cents_per_mtok, 250);
        assert!(cx.audit.recent(5).iter().any(|l| l.contains("settings.token_price") && l.contains("250")));
        key(&mut a, &cx, Key::Char('p'));
        for _ in 0..3 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "abc");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "a non-number keeps the form open");
        assert_eq!(cx.services.settings.lock().unwrap().get().token_price_cents_per_mtok, 250);
        key(&mut a, &cx, Key::Esc);
        // users cannot
        let ucx = user("set-price-u");
        let mut u = SettingsApp::new(&ucx);
        key(&mut u, &ucx, Key::Char('p'));
        assert!(!u.has_modal());
        assert_eq!(ucx.services.settings.lock().unwrap().get().token_price_cents_per_mtok, 0);
    }

    // ------------------------------------------------------------------ storage tab

    struct DirMounter;
    impl iyakki_storage::Mounter for DirMounter {
        fn mount(&self, _dev: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
            std::fs::create_dir_all(target).map_err(|e| e.to_string())
        }
        fn unmount(&self, _target: &std::path::Path) -> Result<(), String> {
            Ok(())
        }
    }

    /// A fake machine: sda = the boot medium, vdb = empty disk, vdc = a disk with a filesystem on it.
    fn machine(tag: &str) -> Option<(StorageEnv, std::path::PathBuf)> {
        let mke2fs = ["/usr/sbin/mke2fs", "/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists())?;
        let root = std::env::temp_dir().join(format!("iyakki-ui-storage-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/block")).unwrap();
        std::fs::create_dir_all(root.join("dev")).unwrap();
        std::fs::write(root.join("mounts"), "rootfs / rootfs rw 0 0\n").unwrap();
        for (name, extra) in [("sda", Some((0x8001u64, &b"CD001"[..]))), ("vdb", None), ("vdc", Some((0u64, &b"precious"[..])))] {
            let d = root.join("sys/block").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("size"), "32768\n").unwrap();
            std::fs::write(d.join("removable"), "0\n").unwrap();
            let node = root.join("dev").join(name);
            std::fs::File::create(&node).unwrap().set_len(16 * 1024 * 1024).unwrap();
            if let Some((off, b)) = extra {
                use std::os::unix::fs::FileExt;
                std::fs::OpenOptions::new().write(true).open(&node).unwrap().write_all_at(b, off).unwrap();
            }
        }
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("users.json"), "{}").unwrap();
        let env = StorageEnv { roots: iyakki_storage::Roots { sys_block: root.join("sys/block"), dev: root.join("dev"), mounts: root.join("mounts"), require_block: false }, mounter: Arc::new(DirMounter), mke2fs, scratch: root.clone(), data_dir: data, virtual_disks: None };
        Some((env, root))
    }

    fn select_disk(a: &mut SettingsApp, cx: &AppCx, name: &str) {
        let i = a.devices.iter().position(|d| d.name == name).unwrap();
        a.handle(&Event::Row(i), cx);
    }

    #[test]
    fn admin_makes_an_empty_disk_the_data_disk_after_confirming() {
        let Some((env, root)) = machine("empty") else { return eprintln!("mke2fs missing; skipping") };
        let (cx, _) = admin("storage1");
        let mut a = SettingsApp::new(&cx).with_storage(env);
        assert!(a.tabs().contains(&Tab::Storage));
        goto(&mut a, &cx, Tab::Storage);
        assert_eq!(a.list_len(&cx), 3);
        select_disk(&mut a, &cx, "vdb");
        key(&mut a, &cx, Key::Char('m'));
        assert!(a.has_modal());
        typ(&mut a, &cx, "vda");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "a wrong name keeps the confirmation open");
        assert_eq!(iyakki_storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_storage::Content::Empty, "nothing happened yet");
        for _ in 0..3 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "vdb");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(status(&a).contains("vdb is now the IYAKKI data disk") && status(&a).contains("Reboot"), "{}", status(&a));
        assert_eq!(iyakki_storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_storage::Content::Iyakki);
        assert!(a.devices.iter().find(|d| d.name == "vdb").unwrap().content == iyakki_storage::Content::Iyakki, "the list refreshed");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("storage.persist") && l.contains("vdb")));
    }

    #[test]
    fn in_the_window_disks_are_virtual_made_and_deleted_from_the_storage_tab() {
        let Some((_, _)) = machine("virt-probe") else { return eprintln!("mke2fs missing; skipping") };
        let root = std::env::temp_dir().join(format!("iyakki-ui-virtual-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let v = iyakki_storage::virt::VirtualDisks::open(&root).unwrap();
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("users.json"), "{}").unwrap();
        let env = StorageEnv { roots: v.roots(), mounter: Arc::new(v.mounter()), mke2fs: "/usr/sbin/mke2fs".into(), scratch: root.clone(), data_dir: data, virtual_disks: Some(v) };
        let (cx, _) = admin("storage-virtual");
        let mut a = SettingsApp::new(&cx).with_storage(env);
        goto(&mut a, &cx, Tab::Storage);
        assert_eq!(a.list_len(&cx), 0, "no real drive is listed");
        key(&mut a, &cx, Key::Char('n'));
        assert!(a.has_modal());
        for _ in 0..2 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "banana");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "a size that is not a size is refused and the question stays");
        for _ in 0..6 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "128M");
        key(&mut a, &cx, Key::Enter);
        assert!(status(&a).contains("Made the virtual disk vda"), "{}", status(&a));
        assert_eq!(a.list_len(&cx), 1);
        assert!(root.join("dev/vda").exists());
        // the disk can become the data disk with the same confirmation as a real one
        select_disk(&mut a, &cx, "vda");
        key(&mut a, &cx, Key::Char('m'));
        typ(&mut a, &cx, "vda");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(status(&a).contains("vda is now the IYAKKI data disk"), "{}", status(&a));
        // and be deleted again, by typing its name
        key(&mut a, &cx, Key::Char('x'));
        typ(&mut a, &cx, "vdb");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "the wrong name keeps the disk");
        for _ in 0..3 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "vda");
        key(&mut a, &cx, Key::Enter);
        assert!(status(&a).contains("Removed vda"), "{}", status(&a));
        assert_eq!(a.list_len(&cx), 0);
        assert!(!root.join("dev/vda").exists());
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("storage.virtual.add")) && cx.audit.recent(10).iter().any(|l| l.contains("storage.virtual.remove")));
    }

    #[test]
    fn storage_refuses_the_boot_medium_and_needs_erase_for_disks_with_data() {
        let Some((env, root)) = machine("guard") else { return eprintln!("mke2fs missing; skipping") };
        let (cx, _) = admin("storage2");
        let mut a = SettingsApp::new(&cx).with_storage(env);
        goto(&mut a, &cx, Tab::Storage);
        select_disk(&mut a, &cx, "sda");
        key(&mut a, &cx, Key::Char('m'));
        assert!(!a.has_modal() && status(&a).contains("booted from"), "{}", status(&a));
        // a disk with data asks for a second, explicit confirmation
        select_disk(&mut a, &cx, "vdc");
        key(&mut a, &cx, Key::Char('m'));
        assert!(a.has_modal());
        typ(&mut a, &cx, "vdc");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "erase");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "lower-case erase is not enough");
        assert_eq!(iyakki_storage::probe(&root.join("dev/vdc")).unwrap(), iyakki_storage::Content::Other("data of unknown type".into()));
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "ERASE");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert_eq!(iyakki_storage::probe(&root.join("dev/vdc")).unwrap(), iyakki_storage::Content::Iyakki);
        assert_eq!(iyakki_storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_storage::Content::Empty, "other disks untouched");
    }

    #[test]
    fn storage_is_admin_only_and_renders() {
        let ucx = user("storage3");
        assert!(!SettingsApp::new(&ucx).tabs().contains(&Tab::Storage));
        // a hand-built request from a non-admin is refused at save time
        let mut forced = SettingsApp::new(&ucx);
        let f = Form::new("x").text("Type vdb", "vdb", "");
        forced.modal = Some(Modal::Form(FormKind::Persist("vdb".into(), false), f));
        key(&mut forced, &ucx, Key::Enter);
        assert!(!forced.is_busy());
        let (cx, _) = admin("storage4");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Storage);
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
        if let Some((env, _)) = machine("render") {
            let mut b = SettingsApp::new(&cx).with_storage(env);
            goto(&mut b, &cx, Tab::Storage);
            g.clear(rgb(0, 0, 0));
            b.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
            assert!(g.hits.iter().any(|h| h.label == "/dev/vdb"), "disks are clickable rows");
        }
    }

    // ---------------------------------------------------------------- hardware pages

    struct FakeCtl(Mutex<Vec<String>>);
    impl iyakki_devctl::Control for FakeCtl {
        fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("{n} {up}"));
            Ok(())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
            None
        }
        fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
            Err("no".into())
        }
        fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
            Err("no".into())
        }
        fn dhcp(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn fake_machine(tag: &str) -> (DeviceEnv, Arc<FakeCtl>) {
        let d = temp_dir(&format!("{tag}-hw"));
        let put = |rel: &str, c: &str| {
            let p = d.join("sys").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        put("class/net/eno1/type", "1\n");
        put("class/net/eno1/flags", "0x1003\n");
        put("class/net/eno1/carrier", "1\n");
        put("class/net/eno1/device/x", "");
        let ctl = Arc::new(FakeCtl(Mutex::new(vec![])));
        (DeviceEnv { roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl: ctl.clone() }, ctl)
    }

    #[test]
    fn the_hardware_pages_are_admin_only() {
        let ucx = user("hw1");
        let a = SettingsApp::new(&ucx);
        for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
            assert!(!a.tabs().contains(&t), "{t:?} is for administrators");
        }
        let (cx, _) = admin("hw2");
        let a = SettingsApp::new(&cx);
        for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
            assert!(a.tabs().contains(&t), "{t:?}");
        }
    }

    #[test]
    fn enter_on_the_network_page_switches_the_link_and_is_audited() {
        let (cx, _) = admin("hw3");
        let (env, ctl) = fake_machine("hw3");
        let mut a = SettingsApp::new(&cx).with_hardware(env);
        goto(&mut a, &cx, Tab::Network);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(status(&a), "Ethernet eno1 turned off.");
        assert_eq!(ctl.0.lock().unwrap().clone(), vec!["eno1 false"]);
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.network") && l.contains("ethernet eno1 off")), "{:?}", cx.audit.recent(10));
        // a missing WiFi adapter is listed, and pressing Enter on it only explains
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert!(status(&a).contains("nothing to switch"), "{}", status(&a));
    }

    #[test]
    fn a_non_admin_who_forces_the_page_cannot_switch_anything() {
        let ucx = user("hw4");
        let (env, ctl) = fake_machine("hw4");
        let mut a = SettingsApp::new(&ucx).with_hardware(env);
        a.tabs.push(Tab::Network); // a hand-built request, as the storage test does
        a.tab = a.tabs.len() - 1;
        key(&mut a, &ucx, Key::Enter);
        assert!(ctl.0.lock().unwrap().is_empty(), "the actuator was never called");
    }

    #[test]
    fn the_hardware_pages_render_with_clickable_rows_and_all_tabs_fit() {
        let (cx, _) = admin("hw5");
        let (env, _) = fake_machine("hw5");
        let mut a = SettingsApp::new(&cx).with_hardware(env);
        // tall enough for the whole list of pages (it grows with every page that is added; Developer mode made it 1080)
        let mut g = Gfx::new(1280, 1080);
        for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
            goto(&mut a, &cx, t);
            g.clear(rgb(0, 0, 0));
            g.hits.clear();
            a.render(&mut g, R::new(0.0, 72.0, 1280.0, 1008.0), 1.0, &cx, 0);
            assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(0))), "{t:?} rows are clickable");
            assert!(g.hits.iter().any(|h| h.label == "Look again"), "{t:?} has its buttons");
        }
        // every tab can be clicked even when the bar wraps onto a second line
        for (i, _) in a.tabs().to_vec().iter().enumerate() {
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(i)), "tab {i} is on screen");
        }
    }

    // ---------------------------------------------------------------- reading aloud: engines of this computer (docs/VOICE.md)

    /// Fake engines on a temp PATH: espeak-ng lists three voices, logs stdin and arguments, speaks for a moment and writes a
    /// WAV for -w; spd-say lists two voices; piper is there without a voice file.
    fn host_fakes(d: &std::path::Path, which: &[&str]) {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("voices.txt"), "Pty Language Age/Gender VoiceName File Other\n 5 en-us --/M English_(America) gmw/en-US\n 5 ta --/M Tamil inc/ta\n 5 hi --/M Hindi inc/hi\n").unwrap();
        std::fs::write(d.join("tone.wav"), iyakki_agent::voice::encode_wav(&iyakki_agent::voice::tone(440.0, 100), 16_000)).unwrap();
        let mk = |name: &str, body: &str| {
            let p = d.join(name);
            std::fs::write(&p, format!("#!/bin/sh\nD=$(dirname \"$0\")\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        };
        for w in which {
            match *w {
                "espeak-ng" => mk("espeak-ng", "case \"$1\" in --voices) cat \"$D/voices.txt\"; exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nout=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -w ] && out=\"$a\"; prev=\"$a\"; done\nif [ -n \"$out\" ]; then cat \"$D/tone.wav\" > \"$out\"; else sleep 0.1; fi"),
                "spd-say" => mk("spd-say", "case \"$1\" in -L) printf '  NAME LANGUAGE VARIANT\\n English en-US none\\n Tamil ta none\\n'; exit 0;; -C) exit 0;; esac\ncat >> \"$D/said.log\"\necho \"spd $@\" >> \"$D/args.log\"\nsleep 0.1"),
                "piper" => mk("piper", "exit 0"),
                other => panic!("no fake {other}"),
            }
        }
    }

    fn host_env(d: &std::path::Path) -> iyakki_agent::hostengines::Env {
        iyakki_agent::hostengines::Env { path: d.to_string_lossy().to_string(), os: iyakki_agent::hostengines::Os::Linux, piper_dirs: Vec::new() }
    }

    fn eng_dir(tag: &str) -> std::path::PathBuf {
        let d = temp_dir(tag);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_voice_page_finds_engines_with_no_setup_and_says_why_others_are_unavailable() {
        let (cx, _) = admin("sv-host1");
        let d = eng_dir("sv-host1-eng");
        host_fakes(&d, &["espeak-ng", "spd-say", "piper"]);
        cx.services.host_tts.configure(host_env(&d));
        let report = cx.services.host_tts.scan_now();
        assert_eq!(cx.services.voice_settings().tts, None, "nothing was set");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        match a.vo_engine(&cx) {
            Some(SpeechEngine::Host { engine, .. }) => assert_eq!(engine, "espeak-ng", "the best usable engine is used: piper has no voice file"),
            other => panic!("{other:?}"),
        }
        let lines = vo_engine_lines(&report, "ta-IN");
        let text: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
        assert!(text.iter().any(|l| l.starts_with("● espeak-ng") && l.contains("speaks Tamil") && l.contains("can save audio")), "{text:?}");
        assert!(text.iter().any(|l| l.starts_with("● speech-dispatcher") && l.contains("play only: Save audio is unavailable")), "{text:?}");
        assert!(text.iter().any(|l| l.starts_with("○ piper") && l.contains("no voice file was found")), "missing voice files are named: {text:?}");
        assert!(text.iter().any(|l| l.starts_with("○ festival") && l.contains("not found") && l.contains("install festival")), "a missing program says how to get it: {text:?}");
        assert!(vo_engine_lines(&report, "te-IN").iter().any(|l| l.0.starts_with("● espeak-ng") && l.0.contains("no Telugu voice")), "no voice for the language is said");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = draw(&mut a, &cx, w, h);
            let what = format!("voice page at {w}x{h}");
            let mine = |l: &str| ["Test voice", "Re-check engines", "Open voice helper in your browser", "Engine: ", "Voice: ", "Language: ", "Speed: "].iter().any(|p| l.starts_with(p));
            for hit in g.hits.iter().filter(|h| mine(&h.label)) {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{what}: {:?} is outside the window", (&hit.label, r.x, r.y, r.w, r.h));
            }
            for needed in ["Test voice", "Re-check engines", "Open voice helper in your browser"] {
                assert!(g.hits.iter().any(|h| h.label == needed), "{what}: {needed} is there: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
            }
            for prefix in ["Engine: ", "Voice: ", "Language: ", "Speed: "] {
                assert!(g.hits.iter().any(|h| h.label.starts_with(prefix)), "{what}: the {prefix} chooser is there");
            }
            // the controls are above the bottom button bar and do not sit on each other
            let btns: Vec<&crate::ui::Hit> = g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char(c)) if "eglfkoy".contains(c)) && ["Test voice", "Re-check engines", "Open voice helper in your browser"].iter().chain(["Engine: ", "Voice: ", "Language: ", "Speed: "].iter()).any(|p| h.label.starts_with(p))).collect();
            for (i, x) in btns.iter().enumerate() {
                for y in &btns[i + 1..] {
                    let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                    let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                    assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} and {:?} overlap", x.label, y.label);
                }
            }
            // and none sits on the bottom button bar
            for x in &btns {
                for y in g.hits.iter().filter(|y| matches!(y.ev, Event::Key(Key::Char(_))) && !mine(&y.label)) {
                    let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                    let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                    assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} sits on the bar button {:?}", x.label, y.label);
                }
            }
            // clicking each control does what its key does
            let test = g.hits.iter().find(|h| h.label == "Test voice").unwrap();
            assert_eq!(test.ev, Event::Key(Key::Char('y')));
        }
    }

    #[test]
    fn the_highlight_choice_cycles_sentence_and_word_sentence_only_off_and_is_saved() {
        use iyakki_agent::voice::Highlight;
        let (cx, _) = admin("sv-highlight");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        assert_eq!(cx.services.voice_settings().highlight, Highlight::SentenceAndWord, "sentence + word is the default");
        key(&mut a, &cx, Key::Char('h'));
        assert_eq!(cx.services.voice_settings().highlight, Highlight::Sentence);
        assert!(status(&a).contains("sentence only"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('h'));
        assert_eq!(cx.services.voice_settings().highlight, Highlight::Off);
        key(&mut a, &cx, Key::Char('h'));
        assert_eq!(cx.services.voice_settings().highlight, Highlight::SentenceAndWord);
        // an old settings file without the field loads with the default
        let v: iyakki_agent::voice::VoiceSettings = serde_json::from_str("{\"speed_pct\": 120}").unwrap();
        assert_eq!(v.highlight, Highlight::SentenceAndWord);
        assert_eq!(serde_json::from_str::<iyakki_agent::voice::VoiceSettings>("{\"highlight\":\"sentence\"}").unwrap().highlight, Highlight::Sentence);
        // word timing from the sound is on by default (also for an old settings file) and toggles with w
        assert!(v.align_words && v.remote_ok.is_empty());
        key(&mut a, &cx, Key::Char('w'));
        assert!(!cx.services.voice_settings().align_words && status(&a).contains("estimated"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('w'));
        assert!(cx.services.voice_settings().align_words);
    }

    #[test]
    fn test_voice_speaks_a_sentence_in_the_chosen_language_with_the_chosen_voice_and_speed() {
        let (cx, _) = admin("sv-host2");
        let d = eng_dir("sv-host2-eng");
        host_fakes(&d, &["espeak-ng"]);
        cx.services.host_tts.configure(host_env(&d));
        cx.services.host_tts.scan_now();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        // English first
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("Spoke a sentence in English with espeak-ng"), "{}", status(&a));
        let said = std::fs::read_to_string(d.join("said.log")).unwrap();
        assert!(said.contains("This is the IYAKKI voice test."), "{said}");
        // Tamil: the Tamil voice, and a Tamil sentence
        key(&mut a, &cx, Key::Char('l'));
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("in Tamil") && status(&a).contains("voice ta"), "{}", status(&a));
        let args = std::fs::read_to_string(d.join("args.log")).unwrap();
        assert!(args.lines().last().unwrap().contains("-v ta "), "{args}");
        assert!(std::fs::read_to_string(d.join("said.log")).unwrap().contains("வணக்கம்"), "the sentence is in Tamil");
        // speed: 125 percent is 218 words per minute
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(cx.services.voice_settings().speed_pct, 125);
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert!(std::fs::read_to_string(d.join("args.log")).unwrap().lines().last().unwrap().contains("-s 219"), "{}", std::fs::read_to_string(d.join("args.log")).unwrap());
        // a language without a voice is said, and the default voice is used
        key(&mut a, &cx, Key::Char('l')); // Hindi
        key(&mut a, &cx, Key::Char('l')); // Telugu
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert!(status(&a).contains("no Telugu voice") && status(&a).contains("default voice"), "{}", status(&a));
        // a voice chosen by name is kept for every language
        key(&mut a, &cx, Key::Char('g'));
        match cx.services.voice_settings().tts {
            Some(SpeechEngine::Host { engine, voice: Some(v), .. }) => assert_eq!((engine.as_str(), v.as_str()), ("espeak-ng", "en-us")),
            other => panic!("{other:?}"),
        }
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("voice.test") && log.contains("settings.voice.tts") && log.contains("settings.voice.speed"), "{log}");
    }

    #[test]
    fn engine_choice_cycles_through_automatic_the_found_engines_and_the_browser_and_is_for_admins() {
        let (cx, _) = admin("sv-host3");
        let d = eng_dir("sv-host3-eng");
        host_fakes(&d, &["espeak-ng", "spd-say"]);
        cx.services.host_tts.configure(host_env(&d));
        cx.services.host_tts.scan_now();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        let hv = HostVoice::current();
        hv.connect(iyakki_agent::hostvoice::Front::Browser);
        hv.event(iyakki_agent::hostvoice::FromClient::Voices { supported: true, voices: vec![] });
        let id = |cx: &AppCx| match cx.services.voice_settings().tts {
            Some(SpeechEngine::Host { engine, .. }) => engine,
            Some(SpeechEngine::Browser) => "browser".to_string(),
            Some(other) => other.label(),
            None => "automatic".to_string(),
        };
        let mut seen = vec![id(&cx)];
        for _ in 0..5 {
            key(&mut a, &cx, Key::Char('e'));
            seen.push(id(&cx));
        }
        assert_eq!(seen, ["automatic", "espeak-ng", "spd-say", "browser", "indic-tts", "automatic"], "the Indic neural voices are the last choice before automatic");
        // with a configured engine, it wins over what was found
        let own = SpeechEngine::Command { command: "sh -c 'cat > /dev/null'".into() };
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = Some(own.clone())).unwrap();
        assert_eq!(a.vo_engine(&cx), Some(own));
        // the default order when nothing is set: engine of this computer, then the browser voice, then nothing
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = None).unwrap();
        assert!(matches!(a.vo_engine(&cx), Some(SpeechEngine::Host { .. })));
        std::fs::remove_file(d.join("espeak-ng")).unwrap();
        std::fs::remove_file(d.join("spd-say")).unwrap();
        cx.services.host_tts.scan_now();
        assert_eq!(a.vo_engine(&cx), Some(SpeechEngine::Browser), "no engine here, the screen can speak");
        hv.disconnect();
        assert_eq!(a.vo_engine(&cx), None);
        key(&mut a, &cx, Key::Char('y'));
        assert!(status(&a).contains("No speech engine is ready") && status(&a).contains("voice helper"), "{}", status(&a));
        // people who are not administrators can test but not change
        let ucx = user("sv-host3u");
        ucx.services.host_tts.configure(host_env(&d));
        let mut u = SettingsApp::new(&ucx);
        goto(&mut u, &ucx, Tab::Voice);
        key(&mut u, &ucx, Key::Char('e'));
        assert!(status(&u).contains("Only administrators"), "{}", status(&u));
        key(&mut u, &ucx, Key::Char('f'));
        assert_eq!(ucx.services.voice_settings().speed_pct, 100);
    }

    #[test]
    fn re_check_finds_an_engine_installed_meanwhile() {
        let (cx, _) = admin("sv-host4");
        let d = eng_dir("sv-host4-eng");
        cx.services.host_tts.configure(host_env(&d));
        cx.services.host_tts.scan_now();
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        assert_eq!(a.vo_engine(&cx), None);
        host_fakes(&d, &["espeak-ng"]);
        key(&mut a, &cx, Key::Char('k'));
        for _ in 0..300 {
            if !cx.services.host_tts.scanning() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(a.vo_engine(&cx), Some(SpeechEngine::Host { ref engine, .. }) if engine == "espeak-ng"));
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("voice.recheck"));
    }

    #[test]
    fn the_browser_voice_helper_is_opened_and_disconnected_from_the_voice_page() {
        let (cx, _) = admin("sv-host5");
        let d = eng_dir("sv-host5-eng");
        let opener = d.join("fakeopen");
        std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > {}/opened\n", d.display())).unwrap();
        std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        cx.services.host_tts.configure(host_env(&d));
        cx.services.host_tts.set_opener(Some(vec![opener.to_string_lossy().to_string()]));
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Voice);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Open voice helper in your browser"));
        key(&mut a, &cx, Key::Char('o'));
        assert!(status(&a).contains("Opening the voice helper"), "{}", status(&a));
        assert_eq!(cx.services.host_tts.helper_state(), Some(iyakki_agent::voicehelper::HelperState::Waiting));
        let url = (0..200).find_map(|_| { std::thread::sleep(std::time::Duration::from_millis(20)); std::fs::read_to_string(d.join("opened")).ok().filter(|u| !u.is_empty()) }).expect("the browser was asked to open it");
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Disconnect voice helper"), "the button turns into Disconnect");
        // a second press disconnects: the listener is closed
        key(&mut a, &cx, Key::Char('o'));
        assert!(status(&a).contains("disconnected"), "{}", status(&a));
        assert_eq!(cx.services.host_tts.helper_state(), None);
        let addr = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let closed = (0..200).any(|_| { std::thread::sleep(std::time::Duration::from_millis(20)); std::net::TcpStream::connect(&addr).is_err() });
        assert!(closed, "the helper's port is closed");
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("voice.helper"), "{log}");
        // without the voice permission it is refused
        let mut gcx = user("sv-host5g");
        gcx.session.policy.voice = false;
        gcx.services.host_tts.configure(host_env(&d));
        let mut g2 = SettingsApp::new(&gcx);
        g2.tab = 0;
        g2.vo_helper_toggle(&gcx);
        assert!(status(&g2).contains("not enabled"), "{}", status(&g2));
        assert_eq!(gcx.services.host_tts.helper_state(), None);
    }

    #[test]
    fn settings_opens_on_the_voice_page_when_the_story_asks() {
        let (cx, _) = admin("sv-host6");
        let mut a = SettingsApp::new(&cx);
        assert_ne!(a.tab(), Tab::Voice);
        a.open_item("voice", &cx);
        assert_eq!(a.tab(), Tab::Voice);
    }

    // ---------------------------------------------------------------- the left list, search and sliders

    fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
        g
    }

    #[test]
    fn typing_in_the_search_box_narrows_the_pages_and_the_first_match_becomes_the_page() {
        let (cx, _) = admin("nav1");
        let mut a = SettingsApp::new(&cx);
        let all = a.nav_matches().len();
        assert_eq!(all, a.tabs().len());
        key(&mut a, &cx, Key::Ctrl('f'));
        assert!(a.nav_focus, "Ctrl+F goes to the search box");
        typ(&mut a, &cx, "wifi");
        assert_eq!(a.nav_matches().len(), 1);
        assert_eq!(a.tab(), Tab::Network, "the one match is shown at once");
        // keys go to the search box while it has the keyboard: 'a' does not open a form on the Models page
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.nav_query.value, "", "Esc clears the text first");
        assert!(a.nav_focus);
        key(&mut a, &cx, Key::Esc);
        assert!(!a.nav_focus, "and then leaves the box");
        // words in any order, keywords and group names
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "volume");
        assert_eq!(a.tab(), Tab::Sound);
        key(&mut a, &cx, Key::Esc);
        typ(&mut a, &cx, "devices");
        assert!(a.nav_matches().len() >= 4, "a group name finds its pages");
        key(&mut a, &cx, Key::Down);
        let second = a.tab();
        key(&mut a, &cx, Key::Enter);
        assert!(!a.nav_focus);
        assert_eq!(a.tab(), nav_first(&a).map_or(second, |_| a.tab()));
        // pasting works in the box
        let mut b = SettingsApp::new(&cx);
        key(&mut b, &cx, Key::Ctrl('f'));
        b.handle(&Event::Paste("bluetooth".into()), &cx);
        assert_eq!(b.tab(), Tab::Bluetooth);
        // nothing found: the page stays as it was and the list says so
        let mut c = SettingsApp::new(&cx);
        key(&mut c, &cx, Key::Ctrl('f'));
        typ(&mut c, &cx, "zzzzzz");
        assert!(c.nav_matches().is_empty());
        let g = draw(&mut c, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(_))));
    }

    fn nav_first(a: &SettingsApp) -> Option<usize> {
        a.nav_matches().first().copied()
    }

    #[test]
    fn the_list_is_clickable_scrolls_with_the_wheel_over_it_and_a_page_stays_in_view() {
        let (cx, _) = admin("nav2");
        let mut a = SettingsApp::new(&cx);
        // tall enough for every page of an administrator's list (it grows as pages are added)
        let g = draw(&mut a, &cx, 1280, 1080);
        for i in 0..a.tabs().len() {
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(i)), "page {i} is in the list");
        }
        assert!(g.hits.iter().any(|h| h.ev == Event::Focus(nav::SEARCH_ID)));
        // a short window: the list is longer than the box and scrolls
        let last = *a.nav_matches().last().unwrap();
        let g = draw(&mut a, &cx, 800, 300);
        assert_eq!(a.nav_scroll, 0.0);
        let shown: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Tab(t) = h.ev { Some(t) } else { None }).collect();
        assert!(shown.contains(&0) && !shown.contains(&last), "the end of the list is out of the box: shown {shown:?}, last {last}, nav {:?}", a.nav_rect);
        let inside = ((a.nav_rect.x + 20.0) as i32, (a.nav_rect.y + 100.0) as i32);
        a.handle(&Event::MouseMove(inside.0, inside.1), &cx);
        a.handle(&Event::Scroll(-4), &cx);
        assert!(a.nav_scroll > 0.0, "the wheel over the list scrolls the list: {}", a.nav_scroll);
        a.handle(&Event::Scroll(-100), &cx);
        let g = draw(&mut a, &cx, 800, 300);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(last)), "scrolled to the end, the last page is in the box");
        let max = a.nav_scroll;
        a.handle(&Event::Scroll(-100), &cx);
        assert_eq!(a.nav_scroll, max, "it stops at the end");
        for _ in 0..20 {
            a.handle(&Event::Scroll(100), &cx);
        }
        assert_eq!(a.nav_scroll, 0.0, "and at the start");
        // the wheel elsewhere moves the selection in the page, not the list
        a.handle(&Event::MouseMove(700, 300), &cx);
        a.handle(&Event::Scroll(-1), &cx);
        assert_eq!(a.nav_scroll, 0.0);
        // choosing a page near the end by search keeps it visible
        key(&mut a, &cx, Key::Ctrl('f'));
        typ(&mut a, &cx, "account");
        let g = draw(&mut a, &cx, 800, 300);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(a.tab)), "the chosen page is in the box");
    }

    #[test]
    fn a_click_on_a_rag_slider_sets_that_value_through_the_settings_checks_and_is_audited() {
        let (cx, _) = admin("sld1");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Rag);
        let _ = draw(&mut a, &cx, 1280, 720);
        let w = |id: usize| a.slider_w.iter().find(|(i, _)| *i == id).map(|(_, w)| *w).expect("slider drawn");
        let (wc, wo, wt) = (w(SLIDER_CHUNK), w(SLIDER_OVERLAP), w(SLIDER_TOPK));
        let rag = |cx: &AppCx| cx.services.settings.lock().unwrap().get().rag.clone();
        a.handle(&Event::AreaAt(SLIDER_TOPK, wt * 0.5, 5.0), &cx);
        assert_eq!(rag(&cx).top_k, 26, "halfway along 1..50");
        a.handle(&Event::AreaAt(SLIDER_CHUNK, wc, 5.0), &cx);
        assert_eq!(rag(&cx).chunk_max, 4000, "the far end");
        a.handle(&Event::AreaAt(SLIDER_CHUNK, 0.0, 5.0), &cx);
        assert_eq!(rag(&cx).chunk_max, 100, "the near end");
        // overlap must stay under half the chunk size: with chunk 100 the slider's range is 0..49, and an overlap above
        // that is refused by the settings checks, never saved
        a.handle(&Event::AreaAt(SLIDER_OVERLAP, wo, 5.0), &cx);
        let r = rag(&cx);
        assert!(r.overlap < r.chunk_max / 2, "{r:?}");
        a.handle(&Event::AreaAt(SLIDER_CHUNK, wc * 0.5, 5.0), &cx);
        a.handle(&Event::AreaAt(SLIDER_OVERLAP, wo * 0.5, 5.0), &cx);
        let r = rag(&cx);
        assert_eq!((r.chunk_max, r.overlap % 20), (2100, 0), "values snap to their step: {r:?}");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.rag")));
        // the buttons on the card do what they always did
        let before = rag(&cx).top_k;
        key(&mut a, &cx, Key::Char(']'));
        assert_eq!(rag(&cx).top_k, before + 1);
        // a click on a switch's key toggles external search
        let ext = rag(&cx).use_external;
        key(&mut a, &cx, Key::Char('u'));
        assert_eq!(rag(&cx).use_external, !ext);
    }

    #[test]
    fn the_sliders_are_not_clickable_for_someone_who_may_not_change_settings() {
        let ucx = user("sld2");
        let mut a = SettingsApp::new(&ucx);
        goto(&mut a, &ucx, Tab::Rag);
        let g = draw(&mut a, &ucx, 1280, 720);
        assert!(!g.hits.iter().any(|h| h.label == "slider" || h.label == "increase" || h.label == "decrease"), "nothing to click");
        let before = ucx.services.settings.lock().unwrap().get().rag.clone();
        a.handle(&Event::AreaAt(SLIDER_TOPK, 10.0, 5.0), &ucx);
        a.slider_w.push((SLIDER_TOPK, 100.0));
        a.handle(&Event::AreaAt(SLIDER_TOPK, 10.0, 5.0), &ucx);
        assert_eq!(ucx.services.settings.lock().unwrap().get().rag, before, "a forged click changes nothing");
    }

    #[test]
    fn brightness_and_volume_have_sliders_whose_buttons_and_track_work() {
        let (cx, _) = admin("sld3");
        let d = temp_dir("sld3-hw");
        let put = |rel: &str, c: &str| {
            let p = d.join("sys").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        put("class/backlight/bl0/max_brightness", "100\n");
        put("class/backlight/bl0/brightness", "50\n");
        put("class/graphics/fb0/virtual_size", "1280,720\n");
        let ctl = Arc::new(FakeCtl(Mutex::new(vec![])));
        let env = DeviceEnv { roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl };
        let mut a = SettingsApp::new(&cx).with_hardware(env);
        goto(&mut a, &cx, Tab::Display);
        let g = draw(&mut a, &cx, 1280, 720);
        let row = a.dev_rows().iter().position(|r| r.slider.is_some()).expect("a brightness slider");
        assert!(g.hits.iter().any(|h| h.ev == Event::Area(SLIDER_HW + row)), "its track takes clicks");
        let bl = |d: &std::path::Path| std::fs::read_to_string(d.join("sys/class/backlight/bl0/brightness")).unwrap().trim().to_string();
        a.handle(&Event::Sub(row as u8, Key::Char('+')), &cx);
        assert_eq!(bl(&d), "60", "the + button on the card steps by ten");
        let w = a.slider_w.iter().find(|(i, _)| *i == SLIDER_HW + row).unwrap().1;
        a.handle(&Event::AreaAt(SLIDER_HW + row, w * 0.8, 5.0), &cx);
        assert_eq!(bl(&d), "80", "a click on the track sets that brightness");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("brightness bl0 80")));
        a.handle(&Event::AreaAt(SLIDER_HW + row, 0.0, 5.0), &cx);
        assert_eq!(bl(&d), "5", "never fully dark");
    }

    #[test]
    fn the_browser_page_has_switches_that_are_saved_for_the_user_and_audited() {
        let (cx, _) = admin("brs1");
        let mut a = SettingsApp::new(&cx);
        assert!(a.tabs().contains(&Tab::Browser));
        assert!(a.nav_matches().contains(&a.tabs().iter().position(|t| *t == Tab::Browser).unwrap()));
        goto(&mut a, &cx, Tab::Browser);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char('a' | 'i' | 's' | 'c')))).count(), 4, "four switches");
        let load = |cx: &AppCx| super::super::browser_config::load(cx);
        assert!(!load(&cx).js_all && load(&cx).images && load(&cx).styles && load(&cx).cookies);
        key(&mut a, &cx, Key::Char('a'));
        key(&mut a, &cx, Key::Char('i'));
        key(&mut a, &cx, Key::Char('s'));
        key(&mut a, &cx, Key::Char('c'));
        let c = load(&cx);
        assert!(c.js_all && !c.images && !c.styles && !c.cookies);
        assert_eq!(status(&a), "Cookies: off.");
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Scripts on every site: on")));
        // forgetting the allowed sites
        let mut c2 = load(&cx);
        c2.js_sites.insert("a.org".into());
        super::super::browser_config::save(&cx, &c2);
        key(&mut a, &cx, Key::Char('f'));
        assert!(load(&cx).js_sites.is_empty());
        // another user is not affected
        let mut other = cx.clone();
        other.session.user = "amy".into();
        assert!(!load(&other).js_all);
        // everyone can use the page (it is their own browser), also a plain user
        let ucx = user("brs2");
        assert!(SettingsApp::new(&ucx).tabs().contains(&Tab::Browser));
        let _ = draw(&mut a, &cx, 640, 480);
    }

    #[test]
    fn the_display_tab_makes_text_and_buttons_bigger_or_smaller_for_the_person_and_keeps_it() {
        let (cx, _) = admin("uisize1");
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, Tab::Display);
        let normal = crate::ui::ui_size();
        key(&mut a, &cx, Key::Char('u'));
        assert_eq!(crate::uisize::load(cx.services.dir(), "root"), crate::uisize::NORMAL + 1);
        assert_eq!(crate::ui::ui_size(), 1.25);
        assert!(status(&a).contains("Large (125%)"), "{}", status(&a));
        for _ in 0..5 {
            key(&mut a, &cx, Key::Char('u'));
        }
        assert_eq!(crate::ui::ui_size(), 2.0);
        assert!(status(&a).contains("largest"), "{}", status(&a));
        key(&mut a, &cx, Key::Char('U'));
        assert_eq!(crate::ui::ui_size(), 1.5);
        // the page draws the choices as buttons, the current one off
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.label.starts_with("Huge")) && g.hits.iter().any(|h| h.label.starts_with("Small")));
        assert!(!g.hits.iter().any(|h| h.label.starts_with("Larger")), "the current one is not a button");
        crate::ui::set_ui_size(normal);
        // an ordinary account can change its own size too
        let mut cx2 = cx.clone();
        cx2.session.user = "bea".into();
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        let mut b = SettingsApp::new(&cx2);
        assert!(!b.tabs().contains(&Tab::Display), "the hardware pages are for administrators");
        goto(&mut b, &cx2, Tab::Account);
        key(&mut b, &cx2, Key::Char('U'));
        let mut g2 = Gfx::new(1280, 720);
        g2.clear(rgb(9, 11, 20));
        b.render(&mut g2, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx2, 0);
        assert!(g2.hits.iter().any(|h| h.label.starts_with("Normal")), "the Account page offers the sizes");
        assert_eq!(crate::uisize::load(cx2.services.dir(), "bea"), 0);
        assert_eq!(crate::uisize::load(cx2.services.dir(), "root"), 4 - 1, "root's own choice is separate");
        crate::ui::set_ui_size(1.0);
    }

    // ------------------------------------------------------------------ WiFi

    /// A machine with a WiFi adapter `wlan0` and stand-in `iw`, `ip`, `wpa_cli`, `wpa_supplicant` and `udhcpc`.
    fn wifi_world(tag: &str) -> (WifiEnv, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iyakki-ui-wifi-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let net = d.join("sys/class/net/wlan0");
        std::fs::create_dir_all(net.join("device")).unwrap();
        std::fs::create_dir_all(net.join("wireless")).unwrap();
        std::fs::write(net.join("type"), "1\n").unwrap();
        std::fs::create_dir_all(d.join("proc")).unwrap();
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let rec = d.join("rec.log");
        let script = |name: &str, body: &str| {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", rec.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script("ip", "exit 0");
        script("iw", "printf 'BSS aa:bb:cc:00:00:01(on wlan0)\\n\\tfreq: 2437.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -50.00 dBm\\n\\tSSID: Seen\\n\\tRSN:\\t * Version: 1\\n\\t\\t * Authentication suites: PSK\\nBSS aa:bb:cc:00:00:02(on wlan0)\\n\\tfreq: 2412.0\\n\\tcapability: ESS (0x0401)\\n\\tsignal: -70.00 dBm\\n\\tSSID: Cafe\\n'");
        script("wpa_cli", "exit 1");
        script("wpa_supplicant", "exit 0");
        script("udhcpc", "exit 0");
        let env = WifiEnv {
            wifi: Arc::new(iyakki_devctl::wifi::Wifi { run_dir: d.join("run"), path: Some(bin) }),
            known: Arc::new(iyakki_devctl::wifi::KnownStore::new(&d.join("wifi.json"))),
            roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") },
        };
        (env, d)
    }

    #[test]
    fn an_administrator_scans_picks_a_network_types_the_passphrase_and_it_is_joined_and_remembered() {
        let (env, d) = wifi_world("join");
        let (cx, _) = admin("wifi1");
        let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
        goto(&mut a, &cx, Tab::Network);
        key(&mut a, &cx, Key::Char('w'));
        settle(&mut a, &cx);
        assert!(a.has_modal(), "the list of networks is shown: {}", status(&a));
        let items: Vec<String> = a.wifi_items.iter().map(|(t, _)| t.clone()).collect();
        assert!(items[0].starts_with("Seen") && items[0].contains("WPA2") && items[0].contains("2.4 GHz"), "{items:?}");
        assert!(items[1].starts_with("Cafe") && items[1].contains("open"));
        assert_eq!(items[2], "Hidden network…");
        assert!(!items.iter().any(|t| t.starts_with("Forget")), "nothing is saved yet");
        key(&mut a, &cx, Key::Enter); // the first one
        assert!(a.has_modal());
        typ(&mut a, &cx, "short");
        key(&mut a, &cx, Key::Enter);
        assert!(a.has_modal(), "a passphrase that is too short keeps the question open");
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "password1");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(status(&a).contains("Joining Seen"), "{}", status(&a));
        let known = env.known.list();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].ssid, "Seen");
        assert!(known[0].passphrase.is_none() && known[0].psk.is_some(), "only the derived key is kept");
        let log = std::fs::read_to_string(d.join("rec.log")).unwrap();
        assert!(log.contains("iw dev wlan0 scan") && log.contains("wpa_supplicant -B -i wlan0") && log.contains("udhcpc"), "{log}");
        assert!(std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("ssid=\"Seen\""));
        assert!(!std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("password1"));
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.wifi.join") && l.contains("Seen")));
        assert!(!std::fs::read_to_string(cx.audit.path()).unwrap().contains("password1"), "the passphrase is never logged");
        // an open network is joined without a question; the saved one is marked and can be forgotten
        key(&mut a, &cx, Key::Char('w'));
        settle(&mut a, &cx);
        assert!(a.wifi_items[0].0.contains("saved") && a.wifi_items.iter().any(|(t, _)| t.starts_with("Forget")));
        typ(&mut a, &cx, "Cafe");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(status(&a).contains("Joining Cafe"), "{}", status(&a));
        assert_eq!(env.known.list().iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), vec!["Cafe", "Seen"]);
        key(&mut a, &cx, Key::Char('w'));
        settle(&mut a, &cx);
        typ(&mut a, &cx, "Forget");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "Seen");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(env.known.list().iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), vec!["Cafe"]);
        assert!(status(&a).contains("Forgot Seen"), "{}", status(&a));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_hidden_network_a_missing_adapter_and_an_ordinary_account_are_handled() {
        let (env, d) = wifi_world("hidden");
        let (cx, _) = admin("wifi2");
        let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
        goto(&mut a, &cx, Tab::Network);
        key(&mut a, &cx, Key::Char('w'));
        settle(&mut a, &cx);
        typ(&mut a, &cx, "Hidden");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "Secret Net");
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "hiddenpass1");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        let k = env.known.list();
        assert_eq!((k[0].ssid.as_str(), k[0].hidden), ("Secret Net", true));
        assert!(std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("scan_ssid=1"));
        // no adapter: said plainly
        let mut b = SettingsApp::new(&cx).with_wifi(WifiEnv { roots: iyakki_devctl::Roots { sys: d.join("nothing"), proc_: d.join("nothing") }, ..env.clone() });
        goto(&mut b, &cx, Tab::Network);
        key(&mut b, &cx, Key::Char('w'));
        assert!(status(&b).contains("no WiFi adapter"), "{}", status(&b));
        // an ordinary account has no Network page at all
        let mut cx2 = cx.clone();
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        assert!(!SettingsApp::new(&cx2).tabs().contains(&Tab::Network));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
#[path = "settings_agents_tests.rs"]
mod agents_tests;
#[cfg(test)]
#[path = "settings_memory_tests.rs"]
mod memory_tests;
#[cfg(test)]
#[path = "settings_time_tests.rs"]
mod time_tests;
#[cfg(test)]
#[path = "settings_install_tests.rs"]
mod install_tests;
#[cfg(test)]
#[path = "settings_mac_tests.rs"]
mod mac_tests;
#[cfg(test)]
#[path = "settings_indic_tests.rs"]
mod indic_tests;
#[cfg(test)]
#[path = "settings_lemonade_tests.rs"]
mod lemonade_tests;
#[cfg(test)]
#[path = "settings_sound_tests.rs"]
mod sound_tests;
#[cfg(test)]
#[path = "settings_developer_tests.rs"]
mod developer_tests;
