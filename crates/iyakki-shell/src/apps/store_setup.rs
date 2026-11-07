//! STORE -> Setup: everything the old "Quick install" page of the IYAKKI menu did, as cards grouped by purpose (docs/STORE.md,
//! docs/USER-GUIDE.md). The engine is unchanged (`iyakki_agent::quickinstall`, the `iyakki quick-install ITEM` job runner); this file
//! is the screen: which cards this machine shows, what each says about itself, what the one button does, the search and filters,
//! "Recommended for this machine", "Install all recommended" with its review, the keys, the scrolling list.
//!
//! The name: "Setup". People come to the Store to get things onto the machine; the tab holds the system's own building blocks (a
//! cluster, runtimes, model servers, voices, coding agents, drivers), which are "setting the machine up", and the word fits on a
//! 360 px wide tab strip. "Essentials" suggested everything is needed; "Tools & services" is long and overlaps the CLI tile.
//!
//! Nothing here touches the network on the screen's thread: the facts (what is installed, what cannot run) are read on a thread of
//! their own and the card says "Checking..." until they arrive (tests give their own with `set_facts`).

use crate::safeclamp::SafeClamp;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iyakki_agent::jobs::Job;
use iyakki_agent::k3sdev::Svc;
use iyakki_agent::quickinstall::{self as qi, Facts, Item, Presence};

use super::{blink, AppCx, Flow};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const AMBER: [u8; 4] = fx::AMBER;
const SB: usize = 20_000;
pub const FILTER: usize = SB;
pub const INSTALL_ALL: usize = SB + 10;
pub const CONFIRM_YES: usize = SB + 11;
pub const CONFIRM_NO: usize = SB + 12;
pub const SEARCH: usize = SB + 13;
pub const BAR: usize = SB + 14;
/// The popup: close (also a click outside it), "install all in this category", the installer list, its own scroll bar, its panel.
pub const POP_CLOSE: usize = SB + 15;
pub const POP_ALL: usize = SB + 16;
pub const POP_LOG: usize = SB + 18;
pub const POP_BAR: usize = SB + 19;
pub const POP_PANEL: usize = SB + 17;
/// The category bar: 0 is All, then the categories in order.
pub const CAT: usize = SB + 20;
pub const PRIMARY: usize = SB + 200;
pub const DETAILS: usize = SB + 300;
pub const MORE: usize = SB + 400;
/// A tile: a click opens its popup.
pub const TILE: usize = SB + 500;
pub const LOGOUT: usize = SB + 600;
pub const DELETE: usize = SB + 700;
pub const STOP: usize = SB + 800;
/// The popup's "Build it here (k3s)" button of the browser audio and video card.
pub const BUILD: usize = SB + 760;
/// The popup's Remove button of the voice pack (Undo: the old speech engines come back, the models it pulled are deleted).
pub const REMOVE: usize = SB + 850;
/// The cluster switcher of the popup owns the codes from here (its list rows follow); "Show all AI tools" is the link under the filters.
pub const SWITCH: usize = SB + 900;
pub const SHOW_ALL_AI: usize = SB + 890;

#[path = "store_setup_view.rs"]
mod view;

#[path = "store_setup_remove.rs"]
pub mod remove;

// ------------------------------------------------------------------------------------------------ the machine

/// What the tab knows about this machine besides what is installed (read once when the tab opens; tests give their own).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Machine {
    /// Cloud mode: no hardware cards.
    pub cloud: bool,
    /// A desktop window of another computer: IYAKKI never installs a cluster there.
    pub window: bool,
    /// The DMI product name ("Macmini8,1"), empty when unknown.
    pub model: String,
    /// Graphics devices with a render node (virtual ones left out), and whether one is NVIDIA's.
    pub gpus: usize,
    pub nvidia: bool,
    /// Memory in megabytes, 0 when unknown.
    pub ram_mb: u64,
    /// The MacBook's FaceTime HD camera (PCI 14e4:1570) is there (`camfw::camera_in`).
    pub camera: bool,
    /// The restricted firmware packs whose hardware is here (`fwpack::present_in`).
    pub fw: Vec<Item>,
    /// The AI hardware profile (docs/AI-HARDWARE.md): what the AI tools are shown, recommended or hidden by. `None`: not read
    /// (tests, unknown): nothing is hidden.
    pub ai: Option<iyakki_agent::aiprofile::AiProfile>,
    /// Settings: "Show all AI tools".
    pub show_all_ai: bool,
    /// Whether a speech engine is set up (Settings \u{2192} Voice has a speech-to-text or a reading voice of its own). `None`: not read
    /// (tests, unknown): the voice pack is not recommended.
    pub voice_engines: Option<bool>,
}

impl Machine {
    pub fn is_mac(&self) -> bool {
        let m = self.model.as_str();
        m.starts_with("Mac") || m.starts_with("iMac") || m.starts_with("Xserve")
    }

    /// Read this machine (sysfs and proc: a few small reads).
    pub fn detect(cx: &AppCx) -> Machine {
        let gpus = iyakki_agent::gpu::detect(std::path::Path::new("/"));
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let kb = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:")?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
        Machine {
            cloud: iyakki_agent::cloudmode::detect(cx.services.cloud_setting()).on,
            window: iyakki_agent::webengine::host_session(),
            model: std::fs::read_to_string("/sys/class/dmi/id/product_name").map(|m| m.trim().to_string()).unwrap_or_default(),
            gpus: gpus.iter().filter(|g| !g.is_virtual()).count(),
            nvidia: iyakki_agent::gpu::has_nvidia(&gpus),
            ram_mb: kb / 1024,
            camera: iyakki_agent::camfw::camera_in(std::path::Path::new("/sys")),
            fw: Item::ALL
                .into_iter()
                .filter(|i| match i.fw_def() {
                    Some(d) => iyakki_agent::fwpack::present_in(std::path::Path::new("/sys"), d),
                    // the hardware firmware item is shown where a driver asked for a file that is missing, or when it is installed
                    None => *i == Item::FwHardware && (iyakki_agent::fwextra::installed(cx.services.dir()).is_some() || !iyakki_agent::fwextra_cli::plan_here(cx.services.dir()).is_empty()),
                })
                .collect(),
            ai: Some(iyakki_agent::aiprofile::this_machine(iyakki_agent::modelhost::Hosts::new(cx.services.dir()).list().len())),
            show_all_ai: cx.services.show_all_ai(),
            voice_engines: Some(Self::has_voice_engines(cx)),
        }
    }

    /// A speech engine is set in Settings \u{2192} Voice (listening or reading).
    fn has_voice_engines(cx: &AppCx) -> bool {
        let Ok(st) = cx.services.settings.lock() else { return false };
        let v = &st.get().voice;
        v.stt.is_some() || v.tts.is_some()
    }

    /// What the AI hardware profile says about this tool (Show when there is no profile, and in cloud mode, which keeps its own rules).
    pub fn ai_vis(&self, item: Item) -> iyakki_agent::aiprofile::Vis {
        match &self.ai {
            Some(p) if !self.cloud => iyakki_agent::aiprofile::visibility(p, item, self.show_all_ai),
            _ => iyakki_agent::aiprofile::Vis::Show,
        }
    }

    /// "Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models; runs ..." (the facts line), when the profile is known.
    pub fn ai_line(&self) -> Option<String> {
        self.ai.as_ref().map(|p| p.summary())
    }

    /// "3 AI tools are hidden because ..." with the way to bring them back; `None` when nothing is hidden.
    pub fn hidden_ai_note(&self) -> Option<String> {
        let p = self.ai.as_ref().filter(|_| !self.cloud && !self.show_all_ai)?;
        let hidden: Vec<_> = iyakki_agent::aiprofile::hidden_items(p).into_iter().filter(|(i, _)| self.base_shows(*i)).collect();
        let (_, reason) = hidden.first()?;
        Some(format!("{} AI tool{} hidden because {reason}. An administrator can switch on Show all AI tools in Settings, Cloud mode.", hidden.len(), if hidden.len() == 1 { " is" } else { "s are" }))
    }

    /// Does this machine show the card at all? Hardware cards need the hardware and are never shown in cloud mode.
    pub fn shows(&self, item: Item) -> bool {
        self.base_shows(item) && self.ai_vis(item).shown()
    }

    /// The hardware rules alone (what the machine has), before the AI hardware profile.
    fn base_shows(&self, item: Item) -> bool {
        match item {
            Item::GpuVulkan => !self.cloud && self.gpus > 0,
            Item::GpuNvidia => !self.cloud && self.nvidia,
            Item::MacFirmware => !self.cloud && self.is_mac(),
            Item::MacCamera => !self.cloud && self.camera,
            Item::FwHardware => !self.cloud && self.fw.contains(&item),
            _ if item.fw_def().is_some() => !self.cloud && self.fw.contains(&item),
            _ => true,
        }
    }

    /// Too little memory for this one (memory unknown: never).
    pub fn too_small(&self, item: Item) -> bool {
        self.ram_mb > 0 && item.min_ram_mb() > 0 && self.ram_mb < item.min_ram_mb()
    }

    /// The plain note for a heavy item on a small machine.
    pub fn heavy_note(&self, item: Item) -> Option<String> {
        use iyakki_agent::aiprofile::Vis;
        if self.too_small(item) {
            return Some(format!("Needs about {} GB of memory; this machine has {:.1} GB, so it is not recommended here.", item.min_ram_mb().div_ceil(1024), self.ram_mb as f64 / 1024.0));
        }
        match self.ai_vis(item) {
            Vis::NotRecommended(r) => Some(format!("Not recommended here: {r}.")),
            Vis::ShownBecause(r) => Some(format!("Shown because {r}.")),
            _ => None,
        }
    }

    /// Why this is recommended for this machine (None: not recommended). `present` says what is installed already.
    pub fn recommend(&self, item: Item, present: &dyn Fn(Item) -> bool) -> Option<&'static str> {
        if present(item) || !self.shows(item) || self.too_small(item) || matches!(self.ai_vis(item), iyakki_agent::aiprofile::Vis::NotRecommended(_)) {
            return None;
        }
        match item {
            Item::K3s if !self.window => Some("Needed by Lemonade, Kokoro, DuckDB and Elasticsearch"),
            Item::VoicePack if self.voice_engines == Some(false) && !self.window => Some("No speech engine is set up yet: this adds listening (Whisper tiny) and a reading voice (Kokoro, English) in one action"),
            Item::GpuVulkan => Some("A GPU was found: local models run much faster on it"),
            Item::GpuNvidia => Some("An NVIDIA GPU was found: its driver gives pods CUDA"),
            Item::MacFirmware => Some("This is a Mac: Wi-Fi and Bluetooth need Apple's firmware"),
            Item::MacHost if matches!(self.ai_vis(item), iyakki_agent::aiprofile::Vis::Recommended(_)) => Some("No GPU for models here: the GPU of a Mac on your network can run them"),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------------------------------------ categories

/// The categories of the tile page (the category bar, and the headings of the All view). Every item is in exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gk {
    GetStarted,
    Ai,
    Voices,
    Agents,
    Data,
    Media,
    Hardware,
    LinuxApps,
}

impl Gk {
    pub const ALL: [Gk; 8] = [Gk::GetStarted, Gk::Ai, Gk::Voices, Gk::Agents, Gk::Data, Gk::Media, Gk::Hardware, Gk::LinuxApps];

    pub fn title(self) -> &'static str {
        match self {
            Gk::GetStarted => "Get started",
            Gk::Ai => "AI servers and models",
            Gk::Voices => "Voices",
            Gk::Agents => "Coding agents",
            Gk::Data => "Data services",
            Gk::Media => "Media tools",
            Gk::Hardware => "Hardware",
            Gk::LinuxApps => "Linux apps",
        }
    }

    /// The short name on the category bar.
    pub fn short(self) -> &'static str {
        match self {
            Gk::GetStarted => "Get started",
            Gk::Ai => "AI and models",
            Gk::Voices => "Voices",
            Gk::Agents => "Coding agents",
            Gk::Data => "Data",
            Gk::Media => "Media",
            Gk::Hardware => "Hardware",
            Gk::LinuxApps => "Linux apps",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Gk::GetStarted => "Node.js, Python and the k3s cluster: the base for the rest",
            Gk::Ai => "model servers here, or the GPU of a Mac on your network",
            Gk::Voices => "reading voices: English (Kokoro) and Indian languages",
            Gk::Agents => "Claude Code and GitHub Copilot for the terminal and the AI IDE, and Gitea to keep your work versioned",
            Gk::Data => "DuckDB, Elasticsearch and the local image registry",
            Gk::Media => "the player for video and audio files",
            Gk::Hardware => "GPU for local models, Mac Wi-Fi, Bluetooth and camera firmware",
            Gk::LinuxApps => "Debian and Ubuntu programs in a sandbox",
        }
    }

    /// The colour of the category's badges: colours the other apps already use (no palette of its own).
    pub fn accent(self) -> [u8; 4] {
        match self {
            Gk::GetStarted => fx::CYAN,
            Gk::Ai => crate::apps::flows::ACCENT,
            Gk::Voices => fx::MAGENTA,
            Gk::Agents => AMBER,
            Gk::Data => crate::apps::data::ACCENT,
            Gk::Media => crate::apps::camera::ACCENT,
            Gk::Hardware => crate::apps::dash::ACCENT,
            Gk::LinuxApps => crate::apps::story::ACCENT,
        }
    }

    /// The group an item lives in.
    pub fn of(item: Item) -> Gk {
        match item {
            Item::Node | Item::Python | Item::K3s => Gk::GetStarted,
            Item::Lemonade | Item::Ollama | Item::MacHost => Gk::Ai,
            Item::Kokoro | Item::VoicePack => Gk::Voices,
            Item::ClaudeCode | Item::Copilot | Item::Gitea | Item::Matrix => Gk::Agents,
            Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::SeaweedFs | Item::RustFs | Item::Registry => Gk::Data,
            Item::Ffmpeg | Item::FfmpegLibs => Gk::Media,
            Item::GpuVulkan | Item::GpuNvidia | Item::MacFirmware | Item::MacCamera => Gk::Hardware,
            Item::LinuxApps | Item::LinuxWindows => Gk::LinuxApps,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek | Item::FwHardware => Gk::Hardware,
        }
    }
}

/// The badge picture of an item (the launcher's icons: the apps the item belongs to).
pub fn icon_of(item: Item) -> Icon {
    match item {
        Item::Node | Item::Python | Item::ClaudeCode | Item::Copilot => Icon::Cli,
        Item::K3s => Icon::Mesh,
        Item::DuckDb => Icon::Data,
        Item::Elasticsearch | Item::OpenSearch => Icon::Rag,
        Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::SeaweedFs | Item::RustFs => Icon::Data,
        Item::Gitea => Icon::Git,
        Item::Matrix => Icon::Chat,
        Item::Registry => Icon::Store,
        Item::Kokoro | Item::VoicePack => Icon::Speaker,
        Item::Lemonade | Item::Ollama => Icon::Chat,
        Item::MacHost | Item::MacFirmware => Icon::Devices,
        Item::GpuVulkan | Item::GpuNvidia => Icon::System,
        Item::Ffmpeg | Item::FfmpegLibs => Icon::Media,
        Item::MacCamera => Icon::Camera,
        Item::LinuxApps | Item::LinuxWindows => Icon::Ide,
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek | Item::FwHardware => Icon::Devices,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Installed,
    NotInstalled,
    Recommended,
    /// An upgrade is available.
    Updates,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Installed, Filter::NotInstalled, Filter::Recommended, Filter::Updates];
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Installed => "Installed",
            Filter::NotInstalled => "Not installed",
            Filter::Recommended => "Recommended",
            Filter::Updates => "Updates",
        }
    }
    fn next(self) -> Filter {
        Filter::ALL[(Filter::ALL.iter().position(|f| *f == self).unwrap_or(0) + 1) % Filter::ALL.len()]
    }
}

// ------------------------------------------------------------------------------------------------ cards

#[derive(Clone, Debug, PartialEq)]
pub enum CardState {
    Checking,
    Missing,
    /// A job is running; its progress when it reports one.
    Installing(Option<f32>),
    Failed,
    Installed,
    /// Installed by IYAKKI, cannot run here (the plain reason).
    NotRunnable(String),
    /// Installed earlier, what it needs to run does not answer now.
    Earlier,
    /// Found on the machine, not IYAKKI's.
    Elsewhere,
}

/// The state of a card from what is known: a running job is Installing; the newest job having failed is Failed only when the cluster
/// shows no healthy object for the item (a failed Retry or an old failed job never turns a running service into Failed).
pub fn base_state(running: Option<Option<f32>>, newest_job_failed: bool, checking: bool, healthy: bool, bad: Option<String>, pres: Option<&Presence>) -> CardState {
    if let Some(p) = running {
        CardState::Installing(p)
    } else if newest_job_failed && !healthy {
        CardState::Failed
    } else if checking {
        CardState::Checking
    } else if let (Some(m), Some(Presence::Installed(_))) = (bad, pres) {
        CardState::NotRunnable(m)
    } else {
        match pres {
            Some(Presence::Installed(_)) => CardState::Installed,
            Some(Presence::Earlier(_)) => CardState::Earlier,
            Some(Presence::Elsewhere(_)) => CardState::Elsewhere,
            _ => CardState::Missing,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    Install,
    Retry,
    Reinstall,
    Update,
    /// Install IYAKKI's own copy next to the one found (asked twice).
    OwnCopy,
    /// Indic-TTS with k3s there: the Platform services review.
    Deploy,
    Open,
    SignIn,
    Log,
    /// Start the managed dev cluster (k3s) through its supervisor.
    Start,
    /// Stop it (a second button of the popup, never the primary one).
    Stop,
    /// A restricted firmware pack is installed and the driver needs a restart to pick it up: asks first, then restarts (the store opens the confirm).
    Reboot,
}

impl Act {
    /// Changes the machine: administrators only.
    pub fn needs_admin(self) -> bool {
        matches!(self, Act::Install | Act::Retry | Act::Reinstall | Act::Update | Act::OwnCopy | Act::Start | Act::Stop | Act::Reboot)
    }
}

/// Everything a card says, worked out from the facts (no drawing).
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub item: Item,
    pub state: CardState,
    pub chip: String,
    pub act: Act,
    pub label: String,
    /// "Needed by ..." for a recommended card.
    pub reason: Option<&'static str>,
    /// "Installs k3s first".
    pub deps: Option<String>,
    /// Amber/red lines: heavy on this machine, not runnable, found elsewhere, used by other tools, desktop window.
    pub notes: Vec<(String, bool)>,
    /// The button cannot be pressed (desktop window and a cluster item).
    pub blocked: bool,
    /// Bytes of a half or finished download kept on the data partition (the camera firmware): the Delete download button.
    pub kept: u64,
    /// A service that is installed: is it up (None: not a service, or not known)?
    pub running: Option<bool>,
    /// The installed version when the engine knows one ("v1.31.4"), else empty.
    pub version: String,
    /// The newer version a lookup found ("1.38"), only for an installed item.
    pub update: Option<String>,
}

/// Where an installed item opens: (app id, item) as the shell understands a request.
pub fn open_target(item: Item) -> Option<(&'static str, &'static str)> {
    match item {
        Item::Node | Item::Python | Item::ClaudeCode | Item::Copilot => Some(("cli", "")),
        Item::DuckDb | Item::Prometheus | Item::ClickHouse | Item::GraphDb => Some(("data", "")),
        Item::Kokoro | Item::VoicePack => Some(("voice", "")),
        Item::Lemonade | Item::Ollama => Some(("settings", "modelhost:pc")),
        Item::Ffmpeg => Some(("media", "")),
        Item::FfmpegLibs => Some(("browser", "")),
        Item::MacHost => Some(("settings", "modelhost:mac")),
        Item::GpuVulkan | Item::GpuNvidia => Some(("brand:gpu", "")),
        Item::MacFirmware => Some(("brand:mac", "")),
        Item::MacCamera => Some(("camera", "")),
        Item::LinuxApps | Item::LinuxWindows => Some(("brand:linux", "")),
        Item::K3s => Some(("k8s", "")),
        // after the install: Settings, Notifications, with the Matrix account form open at the server's address
        Item::Matrix => Some(("settings", "matrix:add")),
        Item::Elasticsearch | Item::OpenSearch | Item::Registry | Item::Gitea | Item::SeaweedFs | Item::RustFs => None,
        // the restricted firmware packs have their own licence step (fwpack.rs); this page only says where they stand
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek | Item::FwHardware => None,
    }
}

/// "Where it goes", for the details.
pub fn where_text(item: Item) -> &'static str {
    match item {
        Item::Node | Item::Python | Item::DuckDb | Item::ClaudeCode | Item::Copilot | Item::Ollama | Item::Ffmpeg | Item::FfmpegLibs => "the tools folder on the data partition, linked into the command path (no sudo)",
        Item::K3s => "the data partition: the managed dev cluster",
        Item::Elasticsearch => "the cluster, namespace iyakki-data",
        Item::OpenSearch => "the cluster, namespace iyakki-data (ClusterIP only, port 9200, 5 GB volume, about 1.5 GB of memory)",
        Item::Prometheus => "the cluster, namespace iyakki-monitoring (ClusterIP only, 5 GB volume, read-only access to the cluster)",
        Item::ClickHouse => "the cluster, namespace iyakki-data (ClusterIP only, HTTP 8123 and native 9000, 5 GB volume, about 1 GB of memory; the password is in your vault as svc.clickhouse)",
        Item::GraphDb => "the cluster, namespace iyakki-data (ClusterIP only, 5 GB volume, about 500 MB of memory; the password is in your vault as svc.graphdb)",
        Item::Gitea => "the cluster, namespace iyakki-gitea (ClusterIP; reachable on this machine only at http://127.0.0.1:3300, 10 GB volume; the password and token are in your vault, and the Git connection \"Local Gitea\" is added)",
        Item::Matrix => "the cluster, namespace iyakki-matrix (ClusterIP only, 5 GB volume, about 512 MB of memory; registration needs a token that is in your vault as svc.matrix.registration-token; federation is off)",
        Item::SeaweedFs => "the cluster, namespace iyakki-objects (ClusterIP only, S3 port 8333, 20 GB volume, about 1 GB of memory; the keys are in your vault as svc.seaweedfs.access-key and svc.seaweedfs.secret-key; the connection record is s3.seaweedfs)",
        Item::RustFs => "the cluster, namespace iyakki-objects (ClusterIP only, S3 port 9000, 20 GB volume, about 1 GB of memory; the keys are in your vault as svc.rustfs.access-key and svc.rustfs.secret-key; the connection record is s3.rustfs)",
        Item::Registry => "the cluster, one pod with a volume (port 30500 on this machine only)",
        Item::Lemonade => "the cluster, namespace iyakki-ai, reachable on this machine only (127.0.0.1:13305)",
        Item::Kokoro => "inside Lemonade: the voice model is pulled into it",
        Item::VoicePack => "inside Lemonade in the cluster (both models are pulled into it, on the CPU); the engines are set in Settings \u{2192} Voice",
        Item::MacHost => "nothing is installed by IYAKKI; you set up the other computer",
        Item::GpuVulkan => "a small device plugin in the cluster; nothing on the system image changes",
        Item::GpuNvidia => "NVIDIA's libraries on the data partition and a device plugin in the cluster",
        Item::MacFirmware => "a driver pack on the data partition; Apple's files stay on this machine",
        Item::MacCamera => "a driver pack (mac-facetimehd) on the data partition; Apple's files are fetched from Apple's CDN at your click and stay on this machine",
        Item::LinuxApps => "a sandbox named debian on the data partition",
        Item::LinuxWindows => "a sandbox named gui on the data partition (graphical programs in an IYAKKI window)",
        Item::FwHardware => "a driver pack (fw-hardware) on the data partition that survives restarts; only the files this computer asks for, from the upstream linux-firmware release",
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => "a driver pack on the data partition that survives restarts; the files come from the upstream linux-firmware repository",
    }
}

/// Size words for the card: "about 45 MB · about a minute".
pub fn size_text(item: Item) -> String {
    let mb = item.download_mb();
    if item == Item::MacCamera {
        format!("about 17.6 MB · {}", item.time_text())
    } else if mb == 0 {
        format!("no download · {}", item.time_text())
    } else if mb >= 1000 {
        format!("about {:.1} GB · {}", mb as f64 / 1000.0, item.time_text())
    } else {
        format!("about {mb} MB · {}", item.time_text())
    }
}

// ------------------------------------------------------------------------------------------------ the pane

/// A category as listed now.
#[derive(Clone, Debug)]
pub struct SectionView {
    pub gk: Gk,
    /// The tiles of this category that match the search and the filter.
    pub items: Vec<Item>,
    /// All the tiles this category has on this machine, matching or not.
    pub total: usize,
}

pub struct Confirm {
    /// What would run, in order (dependencies first), and the targets asked for.
    pub steps: Vec<Item>,
    pub targets: Vec<Item>,
    /// The question: "Install 2 recommended items?".
    pub title: String,
    /// Which cluster the cluster items of the plan go to (`clusters::confirm_text`), when the plan has any.
    pub cluster: Option<String>,
    /// A longer review (the voice pack's: what is downloaded, where it runs, the licences with their texts, which the person reads before
    /// confirming); empty for the short plans. It scrolls with the arrow keys, Page keys and the wheel.
    pub review: Vec<String>,
    pub scroll: usize,
}

/// Does installing this item put something into a cluster (so the screen names the cluster and offers the switcher)? Ollama is not here: it
/// installs on this machine under its own supervisor.
pub fn deploys_to_cluster(item: Item) -> bool {
    matches!(item, Item::Registry | Item::Lemonade | Item::VoicePack | Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs)
}

pub struct Setup {
    pub facts: Option<Facts>,
    /// The facts were given (tests): do not read the machine.
    fixed: bool,
    slot: Arc<Mutex<Option<Facts>>>,
    pub latest: Vec<(Item, qi::Latest)>,
    latest_slot: Arc<Mutex<Option<Vec<(Item, qi::Latest)>>>>,
    pub machine: Machine,
    machine_fixed: bool,
    pub jobs: Vec<Job>,
    pub failed: Vec<Item>,
    /// The camera firmware job's last result (`camfw::read_outcome`) and the bytes of its kept download.
    pub camera_outcome: Option<iyakki_agent::camfw::Outcome>,
    pub camera_kept: u64,
    /// The Mac firmware job's last result and what it keeps on the data volume (`macfw::card_status`: kept images, "Retry (no new download)").
    pub mac_outcome: Option<iyakki_agent::macfw::Outcome>,
    pub mac_cache: String,
    /// The restricted firmware packs' last results (`fwpack::read_outcome`): the cards' notes and the Reboot button.
    pub fw_outcomes: Vec<(Item, iyakki_agent::fwpack::Outcome)>,
    /// Seconds the machine has been up (a result older than the start is not a pending restart); 0 when unknown.
    pub uptime: i64,
    /// The licence form of this pack was asked for (the Store opens it), and the restart confirm.
    pub fw_ask: Option<Item>,
    /// The hardware-firmware item: what the card shows (read by a worker), its last result, and the form was asked for.
    pub hw_view: Option<iyakki_agent::fwextra_cli::HwView>,
    hw_slot: Arc<Mutex<Option<iyakki_agent::fwextra_cli::HwView>>>,
    pub hw_outcome: Option<iyakki_agent::fwpack::Outcome>,
    pub hw_ask: bool,
    pub reboot_ask: bool,
    pub signin: Vec<(Item, crate::agent_login::State)>,
    pub query: TextField,
    pub searching: bool,
    pub filter: Filter,
    /// The category chosen on the bar (None: All).
    pub cat: Option<Gk>,
    /// Items whose popup shows the technical details.
    pub details: HashSet<Item>,
    /// The tile the keyboard is on.
    pub cursor: Item,
    /// The popup that is open.
    pub popup: Option<Item>,
    pub list: ScrollList,
    /// The popup's own scrolling body.
    pub pop_list: ScrollList,
    pub confirm: Option<Confirm>,
    /// The removal review card (`store_setup_remove.rs`), its result slot and the step it is on.
    pub rm: Option<remove::RemoveCard>,
    rm_slot: Arc<Mutex<Option<Result<String, String>>>>,
    rm_progress: Arc<Mutex<String>>,
    confirm_own: Option<Item>,
    pub note: Option<(String, bool)>,
    /// Replaces the program the jobs run (tests use a stand-in).
    pub launch: Option<qi::Launch>,
    pub request: Option<(String, String)>,
    /// What the cluster supervisor says now (read each frame: a lock and a copy); None until read, and in tests.
    pub live_k3s: Option<Svc>,
    /// The version map the "Build it here" route reads (None: the image's own; tests give one).
    pub av_map: Option<String>,
    ctl_slot: Arc<Mutex<Option<Result<String, String>>>>,
    /// Deletes a model of this machine's Lemonade (the voice pack's Remove); tests give a fake.
    pub lemonade_delete: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    started: bool,
    last_poll: u64,
    last_facts: u64,
    /// Where each tile is in the content (x, top, w, h), from the last drawing.
    pub spots: Vec<(Item, R)>,
    /// How many columns the last drawing had.
    pub cols: usize,
    /// The cursor moved: the next drawing scrolls it into view (not every frame, or the wheel could never leave it).
    follow: bool,
    /// The role may change the machine (the old page's rule: administrators).
    pub admin: bool,
    ticks: u64,
    /// The cluster switcher of the popups of the items that deploy to a cluster (hidden with one cluster), made when the tab is drawn.
    pub host: Option<crate::apps::cluster_host::ClusterHost>,
    /// The cluster the account works against now (`clusters::Registry::active_entry`), as the last drawing saw it.
    pub cluster: Option<iyakki_agent::clusters::ClusterEntry>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup::new()
    }
}

impl Setup {
    pub fn new() -> Setup {
        Setup {
            facts: None,
            fixed: false,
            slot: Default::default(),
            latest: vec![],
            latest_slot: Default::default(),
            machine: Machine::default(),
            machine_fixed: false,
            jobs: vec![],
            failed: vec![],
            camera_outcome: None,
            camera_kept: 0,
            mac_outcome: None,
            mac_cache: String::new(),
            fw_outcomes: Vec::new(),
            uptime: 0,
            fw_ask: None,
            hw_view: None,
            hw_slot: Default::default(),
            hw_outcome: None,
            hw_ask: false,
            reboot_ask: false,
            signin: vec![],
            query: TextField::new("/ search apps and tools", false),
            searching: false,
            filter: Filter::All,
            cat: None,
            details: HashSet::new(),
            cursor: Item::K3s,
            popup: None,
            list: ScrollList::default(),
            pop_list: ScrollList::default(),
            confirm: None,
            rm: None,
            rm_slot: Default::default(),
            rm_progress: Default::default(),
            confirm_own: None,
            note: None,
            launch: None,
            request: None,
            live_k3s: None,
            av_map: None,
            ctl_slot: Default::default(),
            lemonade_delete: Arc::new(iyakki_agent::voicepack::delete_local_model),
            started: false,
            last_poll: 0,
            last_facts: 0,
            spots: vec![],
            cols: 1,
            follow: false,
            admin: false,
            ticks: 0,
            host: None,
            cluster: None,
        }
    }

    /// Give the facts (tests, and the screen when the thread's answer arrives).
    pub fn set_facts(&mut self, f: Facts) {
        self.facts = Some(f);
        self.fixed = true;
    }

    pub fn set_machine(&mut self, m: Machine) {
        self.machine = m;
        self.machine_fixed = true;
    }

    /// The tab was opened (or came back): read the machine and start reading what is installed.
    pub fn enter(&mut self, cx: &AppCx) {
        self.admin = cx.session.policy.manage_settings;
        if !self.machine_fixed {
            self.machine = Machine::detect(cx);
        }
        self.refresh_facts(cx);
        self.refresh_jobs(cx);
        self.refresh_hw(cx);
        self.started = true;
    }

    fn refresh_facts(&mut self, cx: &AppCx) {
        if self.fixed {
            return;
        }
        // tests read it at once (what is installed on a made-up data folder); the screen asks a thread
        if cfg!(test) {
            self.facts = Some(qi::gather_facts(&cx.services));
            return;
        }
        let (svc, slot) = (cx.services.clone(), self.slot.clone());
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::thread::Builder::new().name("iyakki-setup-facts".into()).spawn(move || {
            let f = qi::gather_facts(&svc);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        });
        if self.latest.is_empty() {
            let (svc, slot) = (cx.services.clone(), self.latest_slot.clone());
            let _ = std::thread::Builder::new().name("iyakki-setup-latest".into()).spawn(move || {
                let r = qi::check_upgrades(&svc);
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            });
        }
    }

    /// Read what the hardware-firmware card shows (kernel log, devices, network) on a worker; tests and a fixed machine skip it.
    pub fn refresh_hw(&mut self, cx: &AppCx) {
        if self.machine_fixed || cfg!(test) || self.machine.cloud {
            return;
        }
        let (data, home, slot) = (cx.services.dir().to_path_buf(), std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("/root")), self.hw_slot.clone());
        let _ = std::thread::Builder::new().name("iyakki-setup-hwfw".into()).spawn(move || {
            let v = iyakki_agent::fwextra_cli::view_now(&data, &home);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        });
    }

    fn refresh_jobs(&mut self, cx: &AppCx) {
        self.hw_outcome = iyakki_agent::fwextra::read_outcome(cx.services.dir());
        self.jobs = qi::install_jobs(&cx.services.jobs, &cx.session.user);
        self.failed = qi::failed_items(&self.jobs);
        if self.machine.camera {
            self.camera_outcome = iyakki_agent::camfw::read_outcome(cx.services.dir());
            self.camera_kept = iyakki_agent::camfw::leftovers(cx.services.dir());
        }
        if self.machine.is_mac() {
            self.mac_outcome = iyakki_agent::macfw::read_outcome(cx.services.dir());
            self.mac_cache = iyakki_agent::macfw::card_status(cx.services.dir());
        }
        // the restricted firmware packs' results (a few small files) and how long the machine has been up
        self.fw_outcomes = self.machine.fw.iter().chain(Item::ALL.iter().filter(|i| i.fw_def().is_some_and(|d| iyakki_agent::fwpack::installed(cx.services.dir(), d).is_some()))).collect::<std::collections::BTreeSet<_>>().into_iter().filter_map(|i| Some((*i, iyakki_agent::fwpack::read_outcome(cx.services.dir(), i.fw_def()?)?))).collect();
        self.uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|t| t.split_whitespace().next()?.split('.').next()?.parse().ok()).unwrap_or(0);
        // who is signed in to the two coding agents (from the remembered status; no program runs here)
        let facts = self.facts.as_ref();
        let installed = |it: Item| facts.is_some_and(|f| f.presence.iter().any(|(x, p)| *x == it && matches!(p, Presence::Installed(_))));
        self.signin = [Item::ClaudeCode, Item::Copilot]
            .into_iter()
            .filter(|it| installed(*it))
            .filter_map(|it| {
                let agent = it.cli_id().and_then(crate::agent_login::agent_of_cli)?;
                Some((it, crate::agent_login::state(cx, agent, true)))
            })
            .collect();
    }

    /// A heartbeat: the jobs about every half second while one runs (else every few seconds), the facts when a thread answered or a job
    /// finished. True when the screen should be drawn again.
    pub fn tick(&mut self, cx: &AppCx) -> bool {
        self.ticks += 1;
        let mut changed = false;
        if let Some(f) = self.slot.lock().ok().and_then(|mut x| x.take()) {
            self.facts = Some(f);
            self.refresh_jobs(cx);
            changed = true;
        }
        if let Some(v) = self.hw_slot.lock().ok().and_then(|mut x| x.take()) {
            self.hw_view = Some(v);
            changed = true;
        }
        if let Some(l) = self.latest_slot.lock().ok().and_then(|mut x| x.take()) {
            self.latest = l;
            changed = true;
        }
        changed |= self.take_control_result();
        changed |= self.take_remove_result(cx);
        changed |= self.read_live(cx);
        let running = self.jobs.iter().any(|j| j.running());
        let every = if running { 15 } else { 90 };
        if self.ticks.saturating_sub(self.last_poll) >= every {
            self.last_poll = self.ticks;
            let before = (self.jobs.iter().map(|j| (j.id.clone(), j.state.clone(), j.progress.map(|p| p as i32))).collect::<Vec<_>>(), self.failed.clone());
            self.refresh_jobs(cx);
            let after = (self.jobs.iter().map(|j| (j.id.clone(), j.state.clone(), j.progress.map(|p| p as i32))).collect::<Vec<_>>(), self.failed.clone());
            changed |= before != after;
            // something finished (even while another job still runs, such as a long driver install): what is installed changed
            let one_finished = before.0.iter().any(|b| b.1 == iyakki_agent::jobs::State::Running && !self.jobs.iter().any(|j| j.id == b.0 && j.running()));
            if one_finished && self.ticks.saturating_sub(self.last_facts) > 5 {
                self.last_facts = self.ticks;
                self.fixed = false;
                // an upgrade may just have finished: ask the vendors again (in the background)
                self.latest.clear();
                self.refresh_facts(cx);
            }
        }
        changed
    }

    /// Read what the cluster supervisor says (a lock and a copy; never a program). Tests give the facts, so they keep what they set.
    /// True when it changed.
    fn read_live(&mut self, cx: &AppCx) -> bool {
        if self.fixed {
            return false;
        }
        let now = cx.services.k3sdev.state();
        let changed = self.live_k3s.as_ref() != Some(&now);
        self.live_k3s = Some(now);
        changed
    }

    pub fn wants_redraw(&self) -> bool {
        self.jobs.iter().any(|j| j.running()) && self.ticks % 15 == 0
    }

    // ---- what the cards say

    fn present(&self, item: Item) -> bool {
        self.facts.as_ref().is_some_and(|f| f.presence.iter().any(|(x, p)| *x == item && p.present()))
    }

    fn presence(&self, item: Item) -> Option<&Presence> {
        self.facts.as_ref()?.presence.iter().find(|(x, _)| *x == item).map(|(_, p)| p)
    }

    fn running_job(&self, item: Item) -> Option<&Job> {
        self.jobs.iter().find(|j| j.running() && qi::item_of(j) == Some(item))
    }

    pub fn recommended(&self, item: Item) -> Option<&'static str> {
        self.facts.as_ref()?;
        self.machine.recommend(item, &|i| self.present(i))
    }

    /// The cards this machine can show at all, in the engine's order.
    pub fn eligible(&self) -> Vec<Item> {
        Item::ALL.into_iter().filter(|i| self.machine.shows(*i)).collect()
    }

    pub fn card(&self, item: Item) -> Card {
        let pres = self.presence(item).cloned();
        let bad = self.facts.as_ref().and_then(|f| f.unhealthy.iter().find(|(x, _)| *x == item).map(|(_, m)| m.clone()));
        let healthy = self.facts.as_ref().is_some_and(|f| f.healthy.contains(&item));
        let state = base_state(self.running_job(item).map(|j| j.progress), self.failed.contains(&item), self.facts.is_none(), healthy, bad.clone(), pres.as_ref());
        let latest = self.latest.iter().find(|(x, _)| *x == item).map(|(_, l)| l);
        let signed_out = self.signin.iter().any(|(x, s)| *x == item && matches!(s, crate::agent_login::State::SignedOut));
        let window_block = self.machine.window && (item == Item::K3s || item.needs().contains(&Item::K3s));
        let svc_state = self.service_state(item, pres.as_ref());
        let running = svc_state.as_ref().map(|s| *s == Svc::Running);
        let version = match &pres {
            Some(Presence::Installed(d)) if item == Item::K3s || item.cli_id().is_some() => version_of(d),
            _ => String::new(),
        };
        let update = match (&state, latest) {
            (CardState::Installed, Some(qi::Latest::Newer(v))) => Some(v.trim_start_matches('v').to_string()),
            _ => None,
        };
        let chip = match &state {
            CardState::Checking => "Checking…".to_string(),
            CardState::Missing if item == Item::MacHost => "Guide".to_string(),
            CardState::Missing => "Not installed".to_string(),
            CardState::Installing(Some(p)) => format!("Installing {:.0} %", p.clamp(0.0, 100.0)),
            CardState::Installing(None) => "Installing…".to_string(),
            CardState::Failed => "Failed".to_string(),
            CardState::Installed if item == Item::MacHost => "Model host added".to_string(),
            CardState::Installed => match &svc_state {
                Some(Svc::Running) => "Running".to_string(),
                Some(Svc::Starting) => "Starting…".to_string(),
                Some(Svc::Stopping) => "Stopping…".to_string(),
                Some(_) => "Stopped".to_string(),
                None => "Installed".to_string(),
            },
            CardState::NotRunnable(_) => "Installed, not runnable".to_string(),
            CardState::Earlier => "Installed earlier, not running".to_string(),
            CardState::Elsewhere => "Found on this machine".to_string(),
        };
        let (act, label) = match &state {
            _ if item == Item::MacHost => (Act::Open, "Open guide".to_string()),
            // a restricted firmware pack (fwpack.rs): Install asks for the vendor's licence first (a form of the Store), then runs its own job
            _ if item == Item::FwHardware && matches!(state, CardState::Installed) && self.hw_reboot_pending() => (Act::Reboot, "Restart to finish".to_string()),
            _ if item == Item::FwHardware && matches!(state, CardState::Installed) => (Act::Reinstall, "Check again, add or remove".to_string()),
            _ if item == Item::FwHardware && matches!(state, CardState::Missing | CardState::Earlier | CardState::Checking) => (Act::Install, "See the licences and install".to_string()),
            _ if item.fw_def().is_some() && matches!(state, CardState::Installed) && self.fw_reboot_pending(item) => (Act::Reboot, "Reboot to finish".to_string()),
            _ if item.fw_def().is_some() && matches!(state, CardState::Installed) => (Act::Reinstall, "Reinstall".to_string()),
            _ if item.fw_def().is_some() && matches!(state, CardState::Missing | CardState::Earlier | CardState::Checking) => (Act::Install, "Read the licence and install".to_string()),
            CardState::Checking => (Act::None, "Install".to_string()),
            CardState::Installing(_) => (Act::Log, "View log".to_string()),
            CardState::Failed if item == Item::MacFirmware && !self.mac_cache.is_empty() => (Act::Retry, "Retry (no new download)".to_string()),
            CardState::Failed => (Act::Retry, "Retry".to_string()),
            CardState::Missing if item == Item::MacCamera => (Act::Install, "Install camera driver and firmware".to_string()),
            CardState::Missing => (Act::Install, "Install".to_string()),
            CardState::Installed if signed_out => (Act::SignIn, "Sign in".to_string()),
            CardState::Installed if matches!(svc_state, Some(Svc::Starting)) => (Act::None, "Starting…".to_string()),
            CardState::Installed if matches!(svc_state, Some(Svc::Stopping)) => (Act::None, "Stopping…".to_string()),
            CardState::Installed if item == Item::K3s && running == Some(false) => (Act::Start, "Start".to_string()),
            CardState::Installed if matches!(latest, Some(qi::Latest::Newer(_))) => match latest {
                Some(qi::Latest::Newer(v)) => (Act::Update, format!("Upgrade to {}", v.trim_start_matches('v'))),
                _ => (Act::None, String::new()),
            },
            CardState::Installed if item == Item::MacCamera => (Act::Open, "Open camera".to_string()),
            CardState::Installed => match open_target(item) {
                Some(_) => (Act::Open, "Open".to_string()),
                None => (Act::Reinstall, "Reinstall".to_string()),
            },
            CardState::NotRunnable(_) => (Act::Reinstall, "Reinstall (repair)".to_string()),
            CardState::Earlier => (Act::Reinstall, "Reinstall".to_string()),
            CardState::Elsewhere if item.cli_id().is_some() => (Act::OwnCopy, "Install own copy".to_string()),
            CardState::Elsewhere => (Act::None, "Found".to_string()),
        };
        // the line under the description
        let mut notes: Vec<(String, bool)> = Vec::new();
        if self.facts.as_ref().is_some_and(|f| f.waiting.contains(&item)) {
            notes.push(("password waiting for your vault: sign in with your password or open Settings \u{2192} Vault \u{2192} Unlock".to_string(), true));
        }
        if let Some(n) = self.machine.heavy_note(item) {
            notes.push((n, false));
        }
        if let CardState::NotRunnable(m) = &state {
            notes.push((format!("Installed but not runnable: {m}"), true));
        }
        if let (CardState::Elsewhere, Some(Presence::Elsewhere(d))) = (&state, &pres) {
            if item.cli_id().is_some() {
                notes.push((format!("Found on this machine, left as it is: {d}"), false));
            }
        }
        if item == Item::MacCamera {
            if let Some(o) = self.camera_outcome.as_ref().filter(|o| !o.headline.is_empty()) {
                if !matches!(state, CardState::Installing(_) | CardState::Checking) {
                    notes.push((o.headline.clone(), matches!(state, CardState::Failed) || (!o.installed && !matches!(state, CardState::Installed))));
                }
            }
        }
        if item == Item::MacFirmware && !matches!(state, CardState::Installing(_) | CardState::Checking) {
            if let Some(o) = self.mac_outcome.as_ref().filter(|o| !o.headline.is_empty()) {
                notes.push((o.headline.clone(), !o.installed));
            }
            if !self.mac_cache.is_empty() {
                notes.push((self.mac_cache.clone(), false));
            }
        }
        if let Some(def) = item.fw_def() {
            if let Some((_, o)) = self.fw_outcomes.iter().find(|(x, _)| *x == item).filter(|(_, o)| !o.headline.is_empty()) {
                if !matches!(state, CardState::Installing(_) | CardState::Checking) {
                    notes.push((o.headline.clone(), !o.installed && !matches!(state, CardState::Installed)));
                }
            }
            if !matches!(state, CardState::Installed) {
                notes.push((format!("Without it: {}", def.without), false));
            }
        }
        if item == Item::FwHardware {
            if let Some(o) = self.hw_outcome.as_ref().filter(|o| !o.headline.is_empty()) {
                if !matches!(state, CardState::Installing(_) | CardState::Checking) {
                    notes.push((o.headline.clone(), !o.installed));
                }
            }
            match &self.hw_view {
                Some(v) => {
                    notes.push((v.summary.clone(), false));
                    for (title, class) in &v.vendors {
                        notes.push((format!("{title}: {class}"), false));
                    }
                    for o in &v.offers {
                        notes.push((format!("{}{}", if o.ready { "Ready: " } else if o.needs_click { "On your click: " } else { "" }, o.line), false));
                    }
                }
                None => notes.push(("Reading which firmware files this computer asks for…".into(), false)),
            }
        }
        if let Some((_, w)) = self.facts.as_ref().and_then(|f| f.used_by.iter().find(|(x, _)| *x == item)) {
            notes.push((w.clone(), false));
        }
        if let Some((_, qi::Latest::Newer(v))) = self.latest.iter().find(|(x, l)| *x == item && matches!(l, qi::Latest::Newer(_))).map(|(i, l)| (i, l.clone())) {
            if matches!(state, CardState::Installed) {
                notes.push((format!("A newer version is available: {}", v.trim_start_matches('v')), false));
            }
        }
        if let Some((_, crate::agent_login::State::SignedIn(who))) = self.signin.iter().find(|(x, s)| *x == item && matches!(s, crate::agent_login::State::SignedIn(_))).map(|(i, s)| (i, s.clone())) {
            notes.push((format!("Signed in as {who}"), false));
        }
        if window_block {
            notes.push(("In a desktop window IYAKKI never installs a cluster: this computer manages its own. Use Store \u{2192} Setup on the IYAKKI machine itself.".into(), true));
        }
        // what it installs first
        let deps = if matches!(state, CardState::Missing | CardState::Failed | CardState::Earlier | CardState::Elsewhere | CardState::NotRunnable(_)) && item.runs_as_job() {
            let steps = qi::order(item, &|i| self.present(i));
            let runtimes = self.facts.as_ref().map(|f| f.runtimes.clone()).unwrap_or_default();
            let titles = qi::order_titles_with(&steps, &|i: Item| runtimes.iter().find(|(x, _)| *x == i).map(|(_, v)| v.clone()).unwrap_or_default());
            let mine = item.title().to_string();
            let before: Vec<String> = titles.into_iter().filter(|t| *t != mine).collect();
            (!before.is_empty()).then(|| format!("Installs {} first", before.join(", ")))
        } else {
            None
        };
        let kept = if item == Item::MacCamera && !matches!(state, CardState::Installing(_)) { self.camera_kept } else { 0 };
        Card { item, state, chip, act, label, reason: self.recommended(item), deps, notes, kept, blocked: window_block && matches!(act, Act::Install | Act::Retry | Act::Reinstall | Act::OwnCopy | Act::Start), running, version, update }
    }

    /// What a service says about itself: k3s from the supervisor (read live; else from the words the presence line carries), the
    /// services that answer or run in the cluster as up while installed, anything else None (a tool has no "running").
    fn service_state(&self, item: Item, pres: Option<&Presence>) -> Option<Svc> {
        let Some(Presence::Installed(d)) = pres else { return None };
        match item {
            Item::K3s => Some(self.live_k3s.clone().unwrap_or_else(|| {
                let d = d.to_lowercase();
                if d.contains("stopped") {
                    Svc::Stopped
                } else if d.contains("starting") {
                    Svc::Starting
                } else if d.contains("stopping") {
                    Svc::Stopping
                } else if d.contains("failed") {
                    Svc::Failed(String::new())
                } else {
                    Svc::Running
                }
            })),
            Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Registry | Item::Lemonade | Item::Kokoro => Some(Svc::Running),
            _ => None,
        }
    }

    /// The newer version a lookup found for an installed item (without the leading v).
    pub fn update_of(&self, item: Item) -> Option<String> {
        self.card(item).update
    }

    /// The items that need this one (the cards that say "Installs k3s first").
    pub fn needed_by(&self, item: Item) -> Vec<Item> {
        Item::ALL.into_iter().filter(|i| i.needs().contains(&item) && self.machine.shows(*i)).collect()
    }

    fn matches_query(&self, item: Item) -> bool {
        let q = self.query.value.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        let hay = format!("{} {} {} {} {}", item.title(), item.about(), item.id(), Gk::of(item).title(), item.licence()).to_lowercase();
        q.split_whitespace().all(|w| hay.contains(w))
    }

    fn matches_filter(&self, item: Item) -> bool {
        match self.filter {
            Filter::All => true,
            Filter::Installed => self.present(item),
            Filter::NotInstalled => !self.present(item),
            Filter::Recommended => self.recommended(item).is_some(),
            Filter::Updates => self.update_of(item).is_some(),
        }
    }

    fn matches_cat(&self, item: Item) -> bool {
        self.cat.is_none_or(|g| Gk::of(item) == g)
    }

    /// (matching, all) tile counts: "N of M shown".
    pub fn counts(&self) -> (usize, usize) {
        let all = self.eligible();
        (all.iter().filter(|i| self.matches_query(**i) && self.matches_filter(**i) && self.matches_cat(**i)).count(), all.len())
    }

    pub fn narrowed(&self) -> bool {
        !self.query.value.trim().is_empty() || self.filter != Filter::All || self.cat.is_some()
    }

    /// The categories as listed now, in order, each with the tiles that match the search, the filter and the category bar. A tile is in
    /// its own category only (recommended ones carry a star instead of moving), so nothing is ever hidden in a folded heading.
    pub fn sections(&self) -> Vec<SectionView> {
        let eligible = self.eligible();
        let mut v = Vec::new();
        for gk in Gk::ALL {
            if self.cat.is_some_and(|c| c != gk) {
                continue;
            }
            let all: Vec<Item> = eligible.iter().copied().filter(|i| Gk::of(*i) == gk).collect();
            if all.is_empty() {
                continue;
            }
            let items: Vec<Item> = all.iter().copied().filter(|i| self.matches_query(*i) && self.matches_filter(*i)).collect();
            if items.is_empty() && self.narrowed() {
                continue;
            }
            v.push(SectionView { gk, total: all.len(), items });
        }
        v
    }

    /// The categories this machine has tiles in (the category bar).
    pub fn categories(&self) -> Vec<Gk> {
        let eligible = self.eligible();
        Gk::ALL.into_iter().filter(|g| eligible.iter().any(|i| Gk::of(*i) == *g)).collect()
    }

    /// The tiles in the order the keyboard walks them.
    pub fn stops(&self) -> Vec<Item> {
        self.sections().into_iter().flat_map(|s| s.items).collect()
    }

    fn fix_cursor(&mut self) {
        let stops = self.stops();
        if !stops.contains(&self.cursor) {
            self.cursor = stops.first().copied().unwrap_or(Item::K3s);
        }
    }

    // ---- install all recommended, install all in a category

    /// The review for these targets: what would run, in order (dependencies first).
    fn plan_for(&self, targets: Vec<Item>, title: String) -> Confirm {
        let mut present: HashSet<Item> = self.facts.iter().flat_map(|f| f.presence.iter().filter(|(_, p)| p.present()).map(|(i, _)| *i)).collect();
        let review = if targets.contains(&Item::VoicePack) { iyakki_agent::voicepack::review_lines(present.contains(&Item::Lemonade)) } else { Vec::new() };
        let mut steps = Vec::new();
        for t in &targets {
            for s in qi::order(*t, &|i| present.contains(&i)) {
                if s.runs_as_job() && !steps.contains(&s) {
                    steps.push(s);
                }
                present.insert(s);
            }
        }
        let cluster = steps.iter().any(|s| deploys_to_cluster(*s)).then(|| self.deploy_phrase());
        Confirm { steps, targets, title, cluster, review, scroll: 0 }
    }

    /// The cluster the account works against now (`local` unless another was chosen), read from the registry.
    pub fn active_cluster(cx: &AppCx) -> iyakki_agent::clusters::ClusterEntry {
        iyakki_agent::clusters::Registry::load_synced(cx.services.dir(), &cx.session.user).active_entry()
    }

    /// `Deploy the cluster items to cluster "This machine" (local)`: the confirmation text of a plan that puts things into a cluster.
    fn deploy_phrase(&self) -> String {
        let e = self.cluster.clone().unwrap_or_else(iyakki_agent::clusters::ClusterEntry::local_primary);
        iyakki_agent::clusters::confirm_text("Deploy the cluster items", &e)
    }

    /// The confirmation line of one item that deploys to a cluster: `Deploy Lemonade to cluster "gpu-box" (remote)`; for a cluster that is
    /// not on this machine it also says what Setup can do there. `None` for the other items.
    pub fn deploy_text(&self, item: Item) -> Option<String> {
        if !deploys_to_cluster(item) {
            return None;
        }
        let e = self.cluster.clone().unwrap_or_else(iyakki_agent::clusters::ClusterEntry::local_primary);
        let mut t = iyakki_agent::clusters::confirm_text(&format!("Deploy {}", item.title()), &e);
        if !e.kind.is_local() {
            t.push_str(". Setup puts this into the cluster on this machine only: choose \"This machine\" above first");
        }
        Some(t)
    }

    /// Setup's installers run in this machine's own cluster; the Indic-TTS review (Platform services) follows the active one. Another
    /// cluster is refused with the way out (the other screens use it).
    fn cluster_refusal(cx: &AppCx, item: Item) -> Option<String> {
        if !deploys_to_cluster(item) {
            return None;
        }
        let e = Self::active_cluster(cx);
        (!e.kind.is_local()).then(|| format!("{} would go to the cluster on this machine, but your active cluster is {}. Setup installs into this machine's own cluster only: choose \"This machine\" in the cluster list of this window first (the K8S tile, the AI IDE and Platform services use {}).", item.title(), e.phrase(), e.phrase()))
    }

    /// What "Install all recommended" would install: the recommended items, and what each needs first, in order.
    pub fn plan_all(&self) -> Confirm {
        let targets: Vec<Item> = self.eligible().into_iter().filter(|i| self.recommended(*i).is_some() && i.runs_as_job() && self.running_job(*i).is_none()).collect();
        let title = format!("Install {} recommended item{}?", targets.len(), if targets.len() == 1 { "" } else { "s" });
        self.plan_for(targets, title)
    }

    /// The items of a category that "Install all in this category" would install: the ones not on the machine yet that IYAKKI can
    /// install here (a guide, a desktop window's cluster items and what is being installed are left out).
    pub fn category_targets(&self, gk: Gk) -> Vec<Item> {
        self.eligible()
            .into_iter()
            .filter(|i| Gk::of(*i) == gk && i.runs_as_job() && !self.present(*i) && self.running_job(*i).is_none() && !self.card(*i).blocked && self.facts.is_some())
            .collect()
    }

    pub fn plan_category(&self, gk: Gk) -> Confirm {
        let targets = self.category_targets(gk);
        let title = format!("Install {} in {}?", if targets.len() == 1 { "1 item".to_string() } else { format!("{} items", targets.len()) }, gk.title());
        self.plan_for(targets, title)
    }

    /// The review's lines: one per step with its size, then the total.
    pub fn confirm_lines(c: &Confirm) -> Vec<String> {
        let mut v: Vec<String> = c.steps.iter().enumerate().map(|(i, s)| format!("{}. {} · {}", i + 1, s.title(), size_text(*s))).collect();
        let total: u32 = c.steps.iter().map(|s| s.download_mb()).sum();
        if let Some(c) = &c.cluster {
            v.push(c.clone());
        }
        v.push(format!("Total download: about {}", if total >= 1000 { format!("{:.1} GB", total as f64 / 1000.0) } else { format!("{total} MB") }));
        v
    }

    fn start_all(&mut self, cx: &AppCx) {
        let Some(c) = self.confirm.take() else { return };
        // the consent text of "Build it here (k3s)" was read and confirmed: the install job takes the build route (avbuild.rs)
        if c.title == iyakki_agent::avbuild::CONFIRM_TITLE {
            if let Err(e) = iyakki_agent::avbuild::write_request(cx.services.dir()) {
                self.say(format!("Could not record the request: {e}"), false);
                return;
            }
        }
        let mut present: HashSet<Item> = self.facts.iter().flat_map(|f| f.presence.iter().filter(|(_, p)| p.present()).map(|(i, _)| *i)).collect();
        let mut started = 0usize;
        let mut err = None;
        for t in &c.targets {
            match self.start_one(cx, *t, &present, false) {
                Ok(n) => started += n,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
            for s in qi::order(*t, &|i| present.contains(&i)) {
                present.insert(s);
            }
        }
        self.refresh_jobs(cx);
        self.note = Some(match err {
            Some(e) if started == 0 => (e, false),
            Some(e) => (format!("{started} step(s) started; then: {e}"), false),
            None => (format!("Started {started} install step(s). Progress is on the tiles and beside the clock."), true),
        });
    }

    fn launch_for(&self, cx: &AppCx) -> Result<qi::Launch, String> {
        if let Some(l) = &self.launch {
            return Ok(l.clone());
        }
        qi::Launch::this_program(cx.services.dir())
    }

    /// Start the jobs for one target (and what it needs first). Returns how many steps started.
    fn start_one(&mut self, cx: &AppCx, item: Item, present: &HashSet<Item>, audit_note: bool) -> Result<usize, String> {
        if !cx.session.policy.manage_settings {
            return Err(format!("Only an administrator can install software for the whole machine (your role is {}).", cx.session.role.name()));
        }
        if item == Item::FwHardware {
            // the licence list comes first: the Store opens its form (vendors, licences, the way to get the files); nothing is fetched before that
            self.hw_ask = true;
            if audit_note {
                self.note = Some(("Read the licences in the form and choose I accept; nothing is downloaded or installed before that.".into(), true));
            }
            return Ok(0);
        }
        if item.fw_def().is_some() {
            // the licence comes first: the Store opens its form (vendor, licence summary, folder or stick, "I accept"); nothing is fetched before that
            self.fw_ask = Some(item);
            if audit_note {
                self.note = Some(("Read the vendor's licence in the form and choose I accept; nothing is downloaded or installed before that.".into(), true));
            }
            return Ok(0);
        }
        if let Some(why) = Self::cluster_refusal(cx, item) {
            return Err(why);
        }
        let launch = self.launch_for(cx)?;
        if item == Item::MacCamera {
            // its own job (the firmware from Apple's CDN), not a step of the installer's order
            let j = qi::start_camera_firmware(&cx.services.jobs, &cx.session.user, &launch)?;
            cx.audit.record(&cx.session.user, "quickinstall.start", &format!("{} ({})", item.id(), j.label));
            if audit_note {
                self.note = Some((format!("Started: {}. Apple's camera firmware (about 17.6 MB) is downloaded from Apple; progress is on the tile and beside the clock.", j.label), true));
            }
            return Ok(1);
        }
        let started = qi::start(&cx.services.jobs, &cx.session.user, item, &|i| present.contains(&i), &launch)?;
        let steps: Vec<String> = started.iter().map(|j| j.label.clone()).collect();
        let on = if steps.iter().any(|_| deploys_to_cluster(item)) { format!(" on {}", Self::active_cluster(cx).phrase()) } else { String::new() };
        cx.audit.record(&cx.session.user, "quickinstall.start", &format!("{} ({}){on}", item.id(), steps.join(", ")));
        if audit_note {
            self.note = Some((format!("Started: {}{}. Progress is on the tile and beside the clock.", steps.join(", then "), if on.is_empty() { String::new() } else { format!(" ({})", iyakki_agent::clusters::confirm_text(&format!("Deploy {}", item.title()), &Self::active_cluster(cx))) }), true));
        }
        Ok(started.len())
    }

    // ---- actions

    fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.note = Some((m.into(), ok));
    }

    /// The card's one button.
    pub fn press(&mut self, item: Item, cx: &AppCx) {
        let card = self.card(item);
        self.cursor = item;
        if card.blocked {
            let m = card.notes.iter().find(|n| n.1).map(|n| n.0.clone()).unwrap_or_default();
            self.say(m, false);
            return;
        }
        if card.act.needs_admin() && !self.admin {
            self.say(format!("Only an administrator can install software for the whole machine (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        if card.act != Act::OwnCopy {
            self.confirm_own = None;
        }
        match card.act {
            Act::None => {}
            Act::Log => self.request = Some(("installs".into(), String::new())),
            Act::Open => {
                if let Some((app, it)) = open_target(item) {
                    self.request = Some((app.into(), it.into()));
                    self.popup = None;
                }
            }
            Act::SignIn => {
                if let Some(agent) = item.cli_id().and_then(crate::agent_login::agent_of_cli) {
                    crate::agent_login::request(agent, false);
                }
            }
            Act::Deploy => {
                let on = Self::active_cluster(cx);
                cx.audit.record(&cx.session.user, "quickinstall.handoff", &format!("indic-tts: {}", iyakki_agent::clusters::confirm_text("Deploy Indic-TTS", &on)));
                self.request = Some(("settings".into(), "platform:indic-tts".into()));
                self.popup = None;
            }
            Act::OwnCopy if self.confirm_own != Some(item) => {
                self.confirm_own = Some(item);
                self.say(format!("{} is already on this machine and works; IYAKKI leaves it alone. Press the button again to put IYAKKI's own copy next to it (it replaces the links in the tools folder).", item.title()), false);
            }
            Act::Reboot => {
                // never without a click, and a second one: the Store asks "Restart this machine now?"
                self.reboot_ask = true;
            }
            Act::Start | Act::Stop => self.control(cx, item, card.act == Act::Start),
            // the voice pack is never started by one click: the review (sizes, where it runs, licence texts) comes first
            Act::Install | Act::Retry | Act::Reinstall | Act::Update | Act::OwnCopy if item == Item::VoicePack => {
                if let Some(why) = Self::cluster_refusal(cx, item) {
                    self.say(why, false);
                    return;
                }
                if self.running_job(item).is_some() {
                    self.say("The voice pack is being installed: its progress is beside the clock.", false);
                    return;
                }
                self.popup = None;
                self.confirm = Some(self.plan_for(vec![item], "Voice: Whisper tiny + Kokoro: read, then install".into()));
            }
            // the browser's add-on: the administrator reads what was found (or where to put the file) and confirms the licences first
            Act::Install | Act::Retry | Act::Reinstall | Act::Update | Act::OwnCopy if item == Item::FfmpegLibs => {
                if self.running_job(item).is_some() {
                    self.say("The browser add-on is being installed: its progress is beside the clock.", false);
                    return;
                }
                self.popup = None;
                let mut c = self.plan_for(vec![item], "Browser audio and video: read, then confirm".into());
                c.review = iyakki_agent::avaddon_install::plan(&iyakki_agent::avaddon_install::Places::here(cx.services.dir())).review;
                self.confirm = Some(c);
            }
            Act::Install | Act::Retry | Act::Reinstall | Act::Update | Act::OwnCopy => {
                self.confirm_own = None;
                let present: HashSet<Item> = self.facts.iter().flat_map(|f| f.presence.iter().filter(|(_, p)| p.present()).map(|(i, _)| *i)).collect();
                match self.start_one(cx, item, &present, true) {
                    Ok(_) => {}
                    Err(e) => self.say(e, false),
                }
                self.refresh_jobs(cx);
            }
        }
    }

    /// The result of this pack's last run says a restart is wanted and the machine has not started since.
    pub fn fw_reboot_pending(&self, item: Item) -> bool {
        let now = iyakki_agent::clock::now_unix();
        self.fw_outcomes.iter().any(|(x, o)| *x == item && iyakki_agent::fwpack::reboot_pending(o, now, self.uptime))
    }

    /// The hardware firmware's last run says a restart is wanted and the machine has not started since.
    pub fn hw_reboot_pending(&self) -> bool {
        let now = iyakki_agent::clock::now_unix();
        self.hw_outcome.as_ref().is_some_and(|o| iyakki_agent::fwpack::reboot_pending(o, now, self.uptime))
    }

    /// The hardware-firmware form was answered with "I accept": record the acceptance of exactly the list the person saw (audit line),
    /// then start the job with the way they chose. `source`: "download", "link", "folder" or "received".
    pub fn hardware_accept_and_start(&mut self, cx: &AppCx, source: &str, folder: Option<std::path::PathBuf>, with_nvidia: bool) -> Result<String, String> {
        if !cx.session.policy.manage_settings {
            return Err(format!("Only an administrator can install software for the whole machine (your role is {}).", cx.session.role.name()));
        }
        let launch = self.launch_for(cx)?;
        let from = match source {
            "folder" => {
                let f = folder.ok_or("Choose the folder or USB stick with the firmware files.")?;
                iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &f)?;
                if !f.is_dir() {
                    return Err(format!("{} is not a folder", f.display()));
                }
                Some(f)
            }
            "received" => Some(self.hw_view.as_ref().and_then(|v| v.staging.clone()).ok_or("No files have been received from the other computer yet.")?),
            _ => None,
        };
        let link = if source == "link" { Some(self.hw_view.as_ref().and_then(|v| v.link_iface.clone()).ok_or("No cable to another computer is connected.")?) } else { None };
        let with: Vec<String> = if with_nvidia { vec!["nvidia".into()] } else { Vec::new() };
        let plan = iyakki_agent::fwextra_cli::plan_here(&launch.data);
        let sel = plan.select(&with);
        if sel.files.is_empty() {
            return Err("No missing firmware file was found in the kernel log: nothing to install.".into());
        }
        iyakki_agent::fwextra::accept(&launch.data, &sel, &cx.session.user, iyakki_agent::clock::now_unix())?;
        cx.audit.record(&cx.session.user, "firmware.accept", &format!("fw-hardware {} file(s), list {}, linux-firmware {}", sel.unique().len(), &sel.digest(iyakki_agent::fwextra::manifest())[..16], iyakki_agent::fwextra::TAG));
        let opts = qi::HwFirmwareOpts { from, with, link };
        let j = qi::start_hardware_firmware(&cx.services.jobs, &cx.session.user, &opts, &launch)?;
        cx.audit.record(&cx.session.user, "quickinstall.start", &format!("{} ({})", Item::FwHardware.id(), j.label));
        self.refresh_jobs(cx);
        Ok(format!("Started: {}. {}; progress is on the tile and beside the clock.", j.label, match source {
            "folder" => "The files are read from the folder and checked",
            "received" => "The files the other computer sent are checked",
            "link" => "The other computer's internet is used through the cable for this job only, then removed",
            _ => "The files are downloaded from the upstream linux-firmware repository and checked",
        }))
    }

    /// Remove the extra firmware (the person chose it in the form): the driver pack and the kept download go; the audit log says so.
    pub fn hardware_remove(&mut self, cx: &AppCx) -> Result<String, String> {
        if !cx.session.policy.manage_settings {
            return Err(format!("Only an administrator can change the machine's firmware (your role is {}).", cx.session.role.name()));
        }
        let launch = self.launch_for(cx)?;
        iyakki_agent::fwextra::remove(&launch.data)?;
        cx.audit.record(&cx.session.user, "firmware.remove", "fw-hardware");
        self.hw_outcome = None;
        self.fixed = false;
        self.refresh_facts(cx);
        Ok("Removed the extra firmware (the driver pack and the downloaded pieces). Restart for drivers that already loaded it.".into())
    }

    /// The licence form was answered with "I accept": record it (the pack's record and the audit log), then start the pack's job, from a
    /// folder or stick when `from` is given. Returns the plain line for the note.
    pub fn firmware_accept_and_start(&mut self, cx: &AppCx, item: Item, from: Option<std::path::PathBuf>) -> Result<String, String> {
        if !cx.session.policy.manage_settings {
            return Err(format!("Only an administrator can install software for the whole machine (your role is {}).", cx.session.role.name()));
        }
        let def = item.fw_def().ok_or_else(|| format!("{} is not a firmware pack", item.title()))?;
        if let Some(f) = &from {
            iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, f)?;
            if !f.is_dir() {
                return Err(format!("{} is not a folder", f.display()));
            }
        }
        let launch = self.launch_for(cx)?;
        iyakki_agent::fwpack::accept(&launch.data, def, &cx.session.user, iyakki_agent::clock::now_unix())?;
        cx.audit.record(&cx.session.user, "firmware.accept", &format!("{} licence {} (sha256 {}) at linux-firmware {}", def.pack_id, def.licence_id, &iyakki_agent::fwpack::licence_digest(def)[..16], &iyakki_agent::fwpack::table().commit[..10]));
        let j = iyakki_agent::quickinstall::start_firmware_pack(&cx.services.jobs, &cx.session.user, item, from.as_deref(), &launch)?;
        cx.audit.record(&cx.session.user, "quickinstall.start", &format!("{} ({})", item.id(), j.label));
        self.refresh_jobs(cx);
        Ok(format!("Started: {}. {}; progress is on the tile and beside the clock.", j.label, if from.is_some() { "The files are read from the folder and checked" } else { "The files are downloaded from the upstream linux-firmware repository and checked" }))
    }

    /// The restart was confirmed: ask the system to restart (the same path as Settings and the CLI).
    pub fn reboot_now(&mut self, cx: &AppCx) -> Result<String, String> {
        if !cx.session.policy.manage_settings || !cx.session.policy.power {
            return Err(format!("Only an administrator can restart this machine (your role is {}).", cx.session.role.name()));
        }
        cx.audit.record(&cx.session.user, "power.reboot", "restricted firmware pack");
        cx.host.reboot();
        Ok("Restarting…".into())
    }

    /// The camera card's Delete download: the kept pieces of Apple's update go (the installed firmware stays).
    pub fn delete_download(&mut self, cx: &AppCx) {
        if !self.admin {
            self.say(format!("Only an administrator can delete the download (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        if self.running_job(Item::MacCamera).is_some() {
            self.say("The camera firmware is being installed: delete the download when it is done.", false);
            return;
        }
        let n = iyakki_agent::camfw::delete_leftovers(cx.services.dir());
        cx.audit.record(&cx.session.user, "quickinstall.camera.delete-download", &format!("{n} bytes"));
        self.camera_kept = iyakki_agent::camfw::leftovers(cx.services.dir());
        self.say(format!("Deleted the camera firmware download ({:.1} MB). The installed firmware is untouched.", n as f64 / 1e6), true);
    }

    /// The "Show all AI tools" link: the administrator's switch of Settings -> Cloud mode (key A there), audited the same way.
    pub fn show_all_ai_tools(&mut self, cx: &AppCx) {
        if !cx.session.policy.manage_settings {
            self.say(format!("Only an administrator can show all AI tools (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        match cx.services.set_show_all_ai(true) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.show_all_ai", "on");
                self.machine.show_all_ai = true;
                self.say("Show all AI tools: on. Tools this machine's hardware does not suit are listed as not recommended (Settings, Cloud mode switches it off again).", true);
                self.after_narrow();
            }
            Err(e) => self.say(e, false),
        }
    }

    pub fn ask_all(&mut self) {
        if !self.admin {
            self.say("Only an administrator can install software for the whole machine. Ask an administrator.", false);
            return;
        }
        let c = self.plan_all();
        if c.targets.is_empty() {
            self.say("Nothing recommended is missing on this machine.", true);
            return;
        }
        self.confirm = Some(c);
    }

    /// "Install all in this category": the same review as "Install all recommended", for the missing items of one category.
    pub fn ask_category(&mut self, gk: Gk) {
        if !self.admin {
            self.say("Only an administrator can install software for the whole machine. Ask an administrator.", false);
            return;
        }
        let c = self.plan_category(gk);
        if c.targets.is_empty() {
            self.say(format!("Nothing in {} is missing that IYAKKI can install here.", gk.title()), true);
            return;
        }
        self.confirm = Some(c);
    }

    /// Start or stop the managed dev cluster through its supervisor (k3s only: the other services run in the cluster). The supervisor may
    /// wait for the network, so the call runs on a thread of its own and the answer comes back to the next heartbeat.
    fn control(&mut self, cx: &AppCx, item: Item, start: bool) {
        if item != Item::K3s {
            return;
        }
        let (svc, user, slot) = (cx.services.clone(), cx.session.user.clone(), self.ctl_slot.clone());
        cx.audit.record(&cx.session.user, if start { "quickinstall.k3s.start" } else { "quickinstall.k3s.stop" }, "k3s");
        let run = move || {
            let r = if start { svc.k3sdev.start(&user).map(|_| "k3s is starting. The tile says Running when the node is ready.".to_string()) } else { svc.k3sdev.stop(&user).map(|_| "k3s was stopped.".to_string()) };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        };
        if cfg!(test) {
            run();
            self.take_control_result();
        } else {
            self.say(if start { "Starting k3s…" } else { "Stopping k3s…" }, true);
            if std::thread::Builder::new().name("iyakki-setup-k3s".into()).spawn(run).is_err() {
                self.say("Could not start the helper that talks to the cluster supervisor.", false);
            }
        }
    }

    /// The answer of a start or stop that ran on its thread: it becomes the note. True when there was one.
    fn take_control_result(&mut self) -> bool {
        let Some(r) = self.ctl_slot.lock().ok().and_then(|mut x| x.take()) else { return false };
        self.say(match &r { Ok(m) | Err(m) => m.clone() }, r.is_ok());
        true
    }

    /// The popup's Stop button (an administrator; the primary button of a running cluster is Open).
    /// "Build it here (k3s)" on the browser audio and video card: gate (k3s running, a map entry for this IYAKKI version), then the consent text with
    /// its button. Nothing starts before the button; the package is only built and copied to /local/packages (the usual review and confirm follow).
    pub fn build_here(&mut self, cx: &AppCx) {
        if !self.admin {
            self.say(format!("Only an administrator can build software for the whole machine (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        if self.running_job(Item::FfmpegLibs).is_some() {
            self.say("The browser add-on is being installed or built: its progress is beside the clock.", false);
            return;
        }
        if self.card(Item::K3s).running != Some(true) {
            self.say(iyakki_agent::avbuild::NO_K3S_TEXT, false);
            return;
        }
        let map = self.av_map.clone().unwrap_or_else(|| std::fs::read_to_string(std::path::Path::new(iyakki_agent::avbuild::BRIDGE_DIR).join("versions.map")).unwrap_or_default());
        match iyakki_agent::avbuild::lookup(&map, iyakki_agent::avbuild::IMAGE_VERSION) {
            Ok(entry) => {
                self.popup = None;
                let mut c = self.plan_for(vec![Item::FfmpegLibs], iyakki_agent::avbuild::CONFIRM_TITLE.into());
                c.review = iyakki_agent::avbuild::consent_text(&entry, std::env::consts::ARCH);
                self.confirm = Some(c);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub fn stop(&mut self, item: Item, cx: &AppCx) {
        if !self.admin {
            self.say(format!("Only an administrator can stop the cluster (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        self.control(cx, item, false);
    }

    /// The long review scrolls: arrows, Page keys, Home, End and the wheel. True when the event was a scroll.
    fn confirm_scroll(&mut self, ev: &Event) -> bool {
        let Some(c) = self.confirm.as_mut().filter(|c| !c.review.is_empty()) else { return false };
        let last = Self::confirm_lines(c).len() + 1 + c.review.len();
        let page = 12usize;
        c.scroll = match ev {
            Event::Key(Key::Up) => c.scroll.saturating_sub(1),
            Event::Key(Key::Down) => c.scroll + 1,
            Event::Key(Key::PageUp) => c.scroll.saturating_sub(page),
            Event::Key(Key::PageDown) => c.scroll + page,
            Event::Key(Key::Home) => 0,
            Event::Key(Key::End) => last,
            Event::Scroll(d) if *d < 0 => c.scroll.saturating_sub(3),
            Event::Scroll(_) => c.scroll + 3,
            _ => return false,
        }
        .min(last.saturating_sub(1));
        true
    }

    /// The popup's Remove button of the voice pack: the speech engines that were set before come back (when you have not changed them
    /// since) and the models the pack downloaded are deleted from Lemonade. Administrators only; audited.
    pub fn remove_voice_pack(&mut self, cx: &AppCx) {
        if !self.admin {
            self.say(format!("Only an administrator can remove the voice pack (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        if self.running_job(Item::VoicePack).is_some() {
            self.say("The voice pack is being installed: remove it when that is done.", false);
            return;
        }
        let (svc, slot, del) = (cx.services.clone(), self.ctl_slot.clone(), self.lemonade_delete.clone());
        cx.audit.record(&cx.session.user, "quickinstall.voicepack.remove", "voice-pack");
        let run = move || {
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(iyakki_agent::voicepack::remove(&svc, &|m: &str| del(m)));
        };
        if cfg!(test) {
            run();
            self.take_control_result();
        } else {
            self.say("Removing the voice pack\u{2026}", true);
            if std::thread::Builder::new().name("iyakki-voicepack-remove".into()).spawn(run).is_err() {
                self.say("Could not start the helper that removes the voice pack.", false);
            }
        }
    }

    /// Open the popup of a tile.
    pub fn open_popup(&mut self, item: Item) {
        if !self.machine.shows(item) {
            return;
        }
        self.cursor = item;
        self.popup = Some(item);
        self.pop_list.to_top();
        self.confirm_own = None;
    }

    fn move_cursor(&mut self, d: i32) {
        let stops = self.stops();
        if stops.is_empty() {
            return;
        }
        let at = stops.iter().position(|c| *c == self.cursor).unwrap_or(0) as i32;
        self.cursor = stops[(at + d).clamp_safe(0, stops.len() as i32 - 1) as usize];
        self.show_cursor();
    }

    /// Up or Down: the tile in the nearest row above or below whose middle is closest to this one's (rows are the drawn ones, so a new
    /// category heading is crossed naturally). Before the first drawing: one tile back or forward.
    fn move_row(&mut self, down: bool) {
        let Some((_, cur)) = self.spots.iter().find(|(i, _)| *i == self.cursor).copied() else {
            self.move_cursor(if down { 1 } else { -1 });
            return;
        };
        let mid = cur.x + cur.w / 2.0;
        let rows: Vec<&(Item, R)> = self.spots.iter().filter(|(_, r)| if down { r.y > cur.y + cur.h / 2.0 } else { r.y + r.h < cur.y + cur.h / 2.0 }).collect();
        let Some(row_y) = rows.iter().map(|(_, r)| r.y).reduce(|a, b| if down { a.min(b) } else { a.max(b) }) else { return };
        let best = rows.iter().filter(|(_, r)| (r.y - row_y).abs() < 1.0).min_by(|a, b| (a.1.x + a.1.w / 2.0 - mid).abs().total_cmp(&(b.1.x + b.1.w / 2.0 - mid).abs()));
        if let Some((it, _)) = best {
            self.cursor = *it;
            self.show_cursor();
        }
    }

    fn show_cursor(&mut self) {
        self.follow = true;
        if let Some((_, r)) = self.spots.iter().find(|(c, _)| *c == self.cursor).copied() {
            self.list.show(r.y, r.y + r.h);
        }
    }

    fn cycle_category(&mut self) {
        let cats = self.categories();
        self.cat = match self.cat {
            None => cats.first().copied(),
            Some(c) => cats.iter().position(|g| *g == c).and_then(|p| cats.get(p + 1)).copied(),
        };
        self.after_narrow();
    }

    // ---- events

    /// Keys and clicks of the tab. None: the Store handles it (section switching).
    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> Option<Flow> {
        self.admin = cx.session.policy.manage_settings;
        if let Event::Tick = ev {
            return Some(if self.tick(cx) { Flow::Stay } else { Flow::Stay });
        }
        // the removal card owns the keyboard
        if self.rm.is_some() {
            return self.remove_event(ev, cx);
        }
        // the review owns the keyboard
        if self.confirm.is_some() {
            if self.confirm_scroll(ev) {
                return Some(Flow::Stay);
            }
            match ev {
                Event::Key(Key::Char('y')) | Event::Key(Key::Enter) | Event::Tab(CONFIRM_YES) => self.start_all(cx),
                Event::Key(_) | Event::Tab(CONFIRM_NO) => self.confirm = None,
                Event::Tab(_) => {}
                _ => {}
            }
            return Some(Flow::Stay);
        }
        if let Some(item) = self.popup {
            return self.popup_event(item, ev, cx);
        }
        if self.list.event(ev, BAR) {
            return Some(Flow::Stay);
        }
        match ev {
            Event::Tab(INSTALL_ALL) => {
                self.ask_all();
                Some(Flow::Stay)
            }
            Event::Tab(SHOW_ALL_AI) => {
                self.show_all_ai_tools(cx);
                Some(Flow::Stay)
            }
            Event::Tab(n) if (FILTER..FILTER + Filter::ALL.len()).contains(n) => {
                self.filter = Filter::ALL[n - FILTER];
                self.after_narrow();
                Some(Flow::Stay)
            }
            Event::Tab(n) if (CAT..CAT + Gk::ALL.len() + 1).contains(n) => {
                self.cat = if *n == CAT { None } else { Gk::ALL.get(n - CAT - 1).copied() };
                self.after_narrow();
                Some(Flow::Stay)
            }
            Event::Tab(n) if (TILE..TILE + 32).contains(n) => {
                if let Some(it) = Item::ALL.get(n - TILE) {
                    self.open_popup(*it);
                }
                Some(Flow::Stay)
            }
            Event::Tab(n) if *n >= SB && *n < SB + 1000 => Some(Flow::Stay),
            Event::Focus(_) | Event::FocusAt(..) => {
                if let Event::FocusAt(SEARCH, x) = ev {
                    self.searching = true;
                    self.query.click(*x);
                    return Some(Flow::Stay);
                }
                None
            }
            Event::Paste(t) if self.searching => {
                self.query.paste(t);
                self.after_narrow();
                Some(Flow::Stay)
            }
            Event::Key(k) => self.key(k, cx),
            _ => None,
        }
    }

    /// Everything while a popup is open: it is modal, so keys and clicks mean the popup's buttons, and Esc or a click outside closes it.
    fn popup_event(&mut self, item: Item, ev: &Event, cx: &AppCx) -> Option<Flow> {
        if let Event::MouseMove(x, y) = ev {
            self.list.pointer = Some((*x, *y));
        }
        if self.pop_list.event(ev, POP_BAR) {
            return Some(Flow::Stay);
        }
        let idx = Item::ALL.iter().position(|i| *i == item).unwrap_or(0);
        // the cluster switcher of the popup: its codes, and every key while its list is open
        if deploys_to_cluster(item) {
            if let Some(h) = self.host.as_mut() {
                if matches!(ev, Event::Tab(n) if (SWITCH..SWITCH + 100).contains(n)) || (h.owns_keys() && matches!(ev, Event::Key(_))) {
                    use crate::apps::cluster_host::HostOut;
                    match h.handle(ev, &|_| None) {
                        HostOut::Switched(e) => {
                            self.cluster = Some(e.clone());
                            self.note = Some((format!("Working against {}.", e.phrase()), true));
                        }
                        HostOut::Refused(why) => self.note = Some((why, false)),
                        HostOut::Manage => {
                            self.request = Some((crate::apps::cluster_host::MANAGE_REQUEST.0.into(), crate::apps::cluster_host::MANAGE_REQUEST.1.into()));
                            self.popup = None;
                        }
                        HostOut::None => {}
                    }
                    return Some(Flow::Stay);
                }
            }
        }
        match ev {
            Event::Tab(POP_CLOSE) => self.popup = None,
            Event::Tab(POP_PANEL) => {}
            Event::Tab(POP_LOG) => self.request = Some(("installs".into(), String::new())),
            Event::Tab(POP_ALL) => self.ask_category(Gk::of(item)),
            Event::Tab(n) if *n == PRIMARY + idx => self.press(item, cx),
            Event::Tab(n) if *n == STOP + idx => self.stop(item, cx),
            Event::Tab(n) if *n == BUILD + idx && item == Item::FfmpegLibs => self.build_here(cx),
            Event::Tab(n) if *n == REMOVE + idx && item == Item::VoicePack => self.remove_voice_pack(cx),
            Event::Tab(n) if *n == remove::RM_OPEN + idx => self.open_remove(item, cx),
            Event::Tab(n) if *n == DETAILS + idx => {
                if !self.details.remove(&item) {
                    self.details.insert(item);
                }
            }
            Event::Tab(n) if *n == MORE + idx => {
                if let Some((app, it)) = open_target(item) {
                    self.request = Some((app.into(), it.into()));
                }
            }
            Event::Tab(n) if *n == LOGOUT + idx => {
                if let Some(agent) = item.cli_id().and_then(crate::agent_login::agent_of_cli) {
                    crate::agent_login::request(agent, true);
                }
            }
            Event::Tab(DELETE) => self.delete_download(cx),
            Event::Key(k) => match k {
                Key::Esc => self.popup = None,
                Key::Enter => self.press(item, cx),
                Key::Char('d') => {
                    if !self.details.remove(&item) {
                        self.details.insert(item);
                    }
                }
                Key::Char('l') => self.request = Some(("installs".into(), String::new())),
                Key::Char('r') if item != Item::VoicePack && self.can_remove(item) => self.open_remove(item, cx),
                Key::Char('a') => self.ask_category(Gk::of(item)),
                Key::Char('o') => {
                    if let Some((app, it)) = open_target(item) {
                        self.request = Some((app.into(), it.into()));
                    }
                }
                Key::Char('s') if item == Item::K3s && self.card(item).running == Some(true) => self.stop(item, cx),
                Key::PageUp | Key::PageDown | Key::Up | Key::Down => {
                    let k = match k {
                        Key::Up => Key::PageUp,
                        Key::Down => Key::PageDown,
                        other => other.clone(),
                    };
                    self.pop_list.key(&k);
                }
                _ => {}
            },
            _ => {}
        }
        Some(Flow::Stay)
    }

    fn after_narrow(&mut self) {
        self.list.to_top();
        self.fix_cursor();
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Option<Flow> {
        if self.searching {
            match k {
                Key::Enter | Key::Down | Key::Tab => self.searching = false,
                Key::Esc => {
                    if self.query.value.is_empty() {
                        self.searching = false;
                    } else {
                        self.query.clear();
                    }
                }
                other => {
                    self.query.handle(other);
                }
            }
            self.after_narrow();
            return Some(Flow::Stay);
        }
        // Home and End are the first and the last tile (the list follows); the page keys scroll
        match k {
            Key::Home => {
                self.cursor = self.stops().first().copied().unwrap_or(self.cursor);
                self.list.to_top();
                return Some(Flow::Stay);
            }
            Key::End => {
                self.cursor = self.stops().last().copied().unwrap_or(self.cursor);
                self.show_cursor();
                return Some(Flow::Stay);
            }
            _ => {}
        }
        if self.list.key(k) {
            return Some(Flow::Stay);
        }
        match k {
            Key::Char('/') => self.searching = true,
            Key::Char('f') => {
                self.filter = self.filter.next();
                self.after_narrow();
            }
            Key::Char('c') => self.cycle_category(),
            Key::Char('a') => self.ask_all(),
            Key::Esc => {
                if !self.query.value.is_empty() {
                    self.query.clear();
                    self.after_narrow();
                } else if self.filter != Filter::All {
                    self.filter = Filter::All;
                    self.after_narrow();
                } else if self.cat.is_some() {
                    self.cat = None;
                    self.after_narrow();
                } else if self.note.is_some() {
                    self.note = None;
                } else {
                    return Some(Flow::Back);
                }
            }
            Key::Left => self.move_cursor(-1),
            Key::Right => self.move_cursor(1),
            Key::Up => self.move_row(false),
            Key::Down => self.move_row(true),
            Key::Enter | Key::Char(' ') => {
                let it = self.cursor;
                self.open_popup(it);
            }
            Key::Char('i') => {
                let it = self.cursor;
                self.press(it, cx);
            }
            Key::Char('o') => {
                if let Some((app, it)) = open_target(self.cursor) {
                    self.request = Some((app.into(), it.into()));
                }
            }
            Key::Char('d') => {
                let it = self.cursor;
                self.details.insert(it);
                self.open_popup(it);
            }
            Key::Char('l') => self.request = Some(("installs".into(), String::new())),
            Key::Tab | Key::BackTab => return None,
            _ => return Some(Flow::Stay),
        }
        self.fix_cursor();
        Some(Flow::Stay)
    }

    /// Open on this tile's popup (another screen asked: the VOICE tile's "Set up Kokoro", the old menu's keys).
    pub fn focus_item(&mut self, item: Item) {
        self.query.clear();
        self.filter = Filter::All;
        self.cat = None;
        self.searching = false;
        self.cursor = item;
        self.show_cursor();
        self.open_popup(item);
    }

    pub fn hint(&self) -> String {
        if let Some(h) = self.remove_hint() {
            return h;
        }
        if self.confirm.is_some() {
            return "y or Enter install all · Esc cancel".into();
        }
        if self.popup.is_some() {
            return "Enter the main button · d details · a install all in this category · l installer runs · Esc close".into();
        }
        "arrows choose · Enter open a tile · i main button · / search · f filter · c category · a install all recommended · l installer runs · Tab section".into()
    }
}

/// The version in a presence line ("k3s v1.31.4+k3s1 · running", "version 0.5"): the first word that starts like one, with a leading v.
pub fn version_of(detail: &str) -> String {
    detail
        .split(|c: char| c.is_whitespace() || c == '·' || c == ',' || c == '(' || c == ')')
        .map(|w| w.trim())
        .find(|w| {
            let digit_first = w.trim_start_matches('v').chars().next().is_some_and(|c| c.is_ascii_digit());
            digit_first && !w.contains(':') && (w.starts_with('v') || w.contains('.'))
        })
        .map(|w| format!("v{}", w.trim_start_matches('v')))
        .unwrap_or_default()
}

/// The first sentence of the item's text, as the one-line purpose.
fn short_about(item: Item) -> &'static str {
    match item {
        Item::Node => "JavaScript runtime with npm: the coding agents and many tools need it.",
        Item::Python => "Python 3 with venv and pip, for Python tools.",
        Item::K3s => "A lightweight Kubernetes cluster on this machine: the base for databases and model servers.",
        Item::DuckDb => "SQL on CSV and Parquet files; the DASH tile uses it.",
        Item::Elasticsearch => "A search engine, one pod in the cluster. Source-available (Elastic License 2.0 or SSPL), not open source; pulled by the cluster, never shipped.",
        Item::OpenSearch => "An open-source search engine (OpenSearch, Apache-2.0), one pod in the cluster; added to DASH as a source.",
        Item::Prometheus => "Metrics for the cluster, kept 7 days; added to DASH as a source.",
        Item::ClickHouse => "An analytics database for DASH (ClickHouse, Apache-2.0); added to DASH as a source, reads only.",
        Item::GraphDb => "A graph database the assistant can ask about relations (Neo4j, GPL-3.0); added to DASH, reads only.",
        Item::Gitea => "Keeps your projects versioned: a git service on this machine (MIT); the GIT tile and the AI IDE use it as \"Local Gitea\".",
        Item::SeaweedFs => "S3-compatible object storage (SeaweedFS, Apache-2.0, mature): one pod in the cluster; keys in your vault.",
        Item::RustFs => "S3-compatible object storage in Rust (RustFS, Apache-2.0, young: alpha releases); pick it, SeaweedFS or both.",
        Item::Matrix => "A chat server of your own (Matrix, Apache-2.0): flows and the assistant can message you there; registration needs a token from your vault.",
        Item::Registry => "A place for images built on this device (the private store's server).",
        Item::ClaudeCode => "Anthropic's coding agent for the terminal and the AI IDE. Proprietary: downloaded from Anthropic, not shipped.",
        Item::Copilot => "GitHub's coding agent; needs a Copilot subscription. Proprietary: downloaded from GitHub, not shipped.",
        Item::Kokoro => "An English reading voice, run by Lemonade.",
        Item::VoicePack => "Listening (Whisper tiny) and an English reading voice (Kokoro) in one action, run by Lemonade and set up for you.",
        Item::Lemonade => "A local model server for chat, embeddings, images and speech.",
        Item::Ollama => "Run language models on this machine; added to Models when it answers.",
        Item::MacHost => "Use the GPU of a Mac (Apple Silicon) or another computer on your network. A guide: you set up that computer.",
        Item::GpuVulkan => "Let pods in the cluster use this machine's GPU through Vulkan.",
        Item::GpuNvidia => "NVIDIA's own driver, so pods can use CUDA.",
        Item::Ffmpeg => "Plays video and audio in IYAKKI (MEDIA tile, camera recordings).",
        Item::FfmpegLibs => "H.264 video and AAC sound in the BROWSER tile (TikTok and similar pages).",
        Item::MacFirmware => "Apple's Wi-Fi and Bluetooth firmware for a Mac, fetched for your own Mac at your request.",
        Item::MacCamera => "The built-in FaceTime HD camera of this MacBook: one click loads the camera driver (part of this image) and fetches Apple's firmware from Apple.",
        Item::LinuxApps => "Run Debian or Ubuntu programs in a sandbox of their own.",
        Item::LinuxWindows => "Show a graphical Linux program in an IYAKKI window (the LINUX WINDOW tile).",
        Item::FwHardware => "The vendor firmware this computer asks for and lacks (Wi-Fi, Bluetooth, graphics, network): listed with each licence, then downloaded at your click.",
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => item.fw_def().map_or("Firmware for hardware in this machine, downloaded at your click.", |d| d.about),
    }
}

fn state_color(c: &CardState) -> [u8; 4] {
    match c {
        CardState::Installed => OK,
        CardState::Installing(_) => super::ACCENT,
        CardState::Failed | CardState::NotRunnable(_) => ERR,
        CardState::Earlier | CardState::Elsewhere => AMBER,
        CardState::Missing | CardState::Checking => DIM,
    }
}

#[cfg(test)]
#[path = "store_setup_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "store_setup_ai_tests.rs"]
mod ai_tests;

#[cfg(test)]
#[path = "store_setup_cluster_tests.rs"]
mod cluster_tests;

#[cfg(test)]
#[path = "store_setup_voice_tests.rs"]
mod voice_tests;

#[cfg(test)]
#[path = "store_setup_avbuild_tests.rs"]
mod avbuild_tests;

#[cfg(test)]
#[path = "store_setup_remove_tests.rs"]
mod remove_tests;
