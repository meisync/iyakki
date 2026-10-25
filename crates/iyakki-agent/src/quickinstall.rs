//! Quick install, from the IYAKKI menu in the top bar: **k3s**, and two things that run with it, **DuckDB** and **Elasticsearch**.
//!
//! * **Dependency order.** DuckDB and Elasticsearch need k3s: asking for either installs k3s first when it is not there yet
//!   ([`order`]), and the menu shows the order before anything starts ("1. k3s  2. Elasticsearch").
//! * **Detached jobs.** Every step is a job of [`crate::jobs`] in the workspace [`WORKSPACE`], so it survives the menu, the session
//!   and a restart of the shell, its output is a log, and the top bar's installer indicator counts it. The job runs this program
//!   again as `iyakki quick-install <k3s|duckdb|elasticsearch> --data <folder>` ([`cli_main`]), which prints `[n/m]` steps the
//!   jobs engine reads as progress.
//! * **The existing machinery.** k3s is the managed dev cluster of `k3sdev.rs` (download checked against the vendor's checksum,
//!   everything inside the data partition, refused in a desktop window or when another cluster exists); once it is installed the job
//!   leaves a start request that the shell's own supervisor picks up ([`take_start_request`]). DuckDB is the CLI tile's catalogue
//!   entry (`clis.rs`). Elasticsearch is one single-node Deployment applied with `k3s kubectl` into the namespace [`NAMESPACE`].
//! * **Tests touch nothing real.** [`run_step`] talks to the world only through [`StepEnv`]; the tests give it a fake. The job
//!   launcher takes the program to run ([`Launch::exe`]), so the tests start a small script instead of `iyakki`.
//!
//! No process is started here except through the jobs engine and the runner `k3sdev` is given (BOUNDARY.md).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::jobs::{Cmd, Job, Jobs, Spec};

/// The jobs workspace of installer jobs: the top bar's indicator shows the jobs of this workspace.
pub const WORKSPACE: &str = "iyakki-install";
/// Where the Elasticsearch Deployment goes.
pub const NAMESPACE: &str = "iyakki-data";
/// The Elasticsearch image (pinned).
pub const ES_IMAGE: &str = "docker.elastic.co/elasticsearch/elasticsearch:8.15.3";
/// Where the Lemonade Deployment goes.
pub const LEMONADE_NAMESPACE: &str = "iyakki-ai";
/// The Lemonade server image (pinned; the project publishes amd64 only).
pub const LEMONADE_IMAGE: &str = "ghcr.io/lemonade-sdk/lemonade-server:v2026.42.1";
/// How long a step that needs the cluster waits for it to be installed and Ready.
pub const WAIT_FOR_CLUSTER: Duration = Duration::from_secs(25 * 60);

/// The sections of the Quick install page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Runtimes,
    Cluster,
    AiCoding,
    Voice,
    LocalModels,
    Media,
    /// Firmware for Macs (Apple's Wi-Fi and Bluetooth files, read from the person's own Mac or Apple's recovery download).
    /// Debian and Ubuntu programs in a sandbox (linuxapps.rs, docs/LINUX-APPS.md).
    Linux,
    Mac,
    /// Restricted firmware packs (fwpack.rs): firmware IYAKKI is not sure it may ship, downloaded for the person at their click.
    Firmware,
}

impl Group {
    pub const ALL: [Group; 9] = [Group::Runtimes, Group::Cluster, Group::AiCoding, Group::Voice, Group::LocalModels, Group::Media, Group::Linux, Group::Mac, Group::Firmware];

    pub fn title(self) -> &'static str {
        match self {
            Group::Runtimes => "Runtimes",
            Group::Cluster => "Cluster",
            Group::AiCoding => "AI coding",
            Group::Voice => "Voice",
            Group::LocalModels => "Local models",
            Group::Media => "Media",
            Group::Linux => "Linux apps",
            Group::Mac => "Mac drivers",
            Group::Firmware => "Hardware firmware",
        }
    }

    /// What the section says about choosing (the pairs are not exclusive: they run side by side).
    pub fn note(self) -> &'static str {
        match self {
            Group::Runtimes => "Node.js and Python 3: the coding agents and Python tools need them, and they are installed first when missing",
            Group::Cluster => "k3s is the base; DuckDB, Elasticsearch, Lemonade, Indic-TTS and the local image registry install it first when it is missing",
            Group::AiCoding => "coding agents for the terminal and the AI IDE (Node.js is installed first when it is missing); pick one or both",
            Group::Voice => "reading voices; pick one or both (Kokoro: English, through Lemonade, which it installs first; Indic-TTS: Indian languages, in k3s); or Voice: Whisper tiny + Kokoro, which also adds listening and sets both up",
            Group::LocalModels => "model servers on this machine, added to Models when they answer; pick one or both (Lemonade runs in k3s); or use the GPU of a Mac on your network",
            Group::Media => "the player for video and audio files (MEDIA tile, IYAKKI menu \u{2192} Media player); it needs nothing else",
            Group::Linux => "Debian and Ubuntu programs (apt) in a sandbox of their own, with a network switch and only the folders you mount; downloaded from the distribution, never shipped (Store \u{2192} Setup \u{2192} Linux apps has the page)",
            Group::Mac => "T2 Macs only: the Wi-Fi and Bluetooth firmware is Apple's, so it is never shipped; it is read from your own Mac or from Apple's recovery download, when you ask (IYAKKI menu \u{2192} Mac drivers has more ways)",
            Group::Firmware => "firmware that IYAKKI is not sure it may redistribute is not in the images: shown only when your hardware needs it; you read the vendor's licence, accept it, and it is downloaded from the upstream linux-firmware repository (or read from a folder or stick), checked and installed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    Node,
    Python,
    K3s,
    DuckDb,
    Elasticsearch,
    Registry,
    ClaudeCode,
    Copilot,
    Kokoro,
    IndicTts,
    /// Voice in one click: Whisper tiny (speech to text) and Kokoro (text to speech), both through the machine's Lemonade, then set as
    /// the engines of Settings \u{2192} Voice and tested ([`crate::voicepack`]).
    VoicePack,
    Lemonade,
    Ollama,
    /// Not an install: a guide to using the GPU of a Mac (or another computer) on the network as a model host (modelhost.rs).
    MacHost,
    /// Stage 1 of the GPU for local models: Vulkan through Mesa in the pod image, any GPU with a render node (gpu_steps.rs).
    GpuVulkan,
    /// Stage 3: NVIDIA's vendor driver (CUDA): open modules from the image, vendor files downloaded at the click (gpu_nvidia.rs).
    GpuNvidia,
    Ffmpeg,
    /// Apple's Wi-Fi and Bluetooth firmware for a T2 Mac, installed as driver packs (macfw.rs).
    MacFirmware,
    /// A first Debian sandbox named `debian` (linuxapps.rs); the Store \u{2192} Setup \u{2192} Linux apps (Open its page) makes more (Ubuntu, other names, folders).
    LinuxApps,
    /// Graphical programs of the Linux apps sandbox in an IYAKKI window: a sandbox named `gui` with TigerVNC's X server, a window manager and a test
    /// program, installed by apt at the click (linuxwin.rs); the LINUX WINDOW tile shows them.
    LinuxWindows,
    /// The FaceTime HD camera of a MacBook: Apple's firmware is fetched from Apple's CDN on the click (camfw.rs). Started by
    /// [`start_camera_firmware`], not by [`start`]; shown only where the camera (14e4:1570) is.
    MacCamera,
    /// Restricted firmware packs (fwpack.rs): Qualcomm Bluetooth, Realtek and Broadcom network, Atheros AR9170. Each has its own job
    /// ([`start_firmware_pack`]), is shown only where its hardware is, and needs the person to accept the vendor's licence first.
    FwQualcommBt,
    FwRealtekNet,
    FwBroadcomNet,
    FwAtheros9170,
}

impl Item {
    pub const ALL: [Item; 25] = [Item::Node, Item::Python, Item::K3s, Item::DuckDb, Item::Elasticsearch, Item::Registry, Item::ClaudeCode, Item::Copilot, Item::Kokoro, Item::IndicTts, Item::VoicePack, Item::Lemonade, Item::Ollama, Item::MacHost, Item::GpuVulkan, Item::GpuNvidia, Item::Ffmpeg, Item::MacFirmware, Item::MacCamera, Item::LinuxApps, Item::LinuxWindows, Item::FwQualcommBt, Item::FwRealtekNet, Item::FwBroadcomNet, Item::FwAtheros9170];

    pub fn id(self) -> &'static str {
        match self {
            Item::Node => "nodejs",
            Item::Python => "python",
            Item::K3s => "k3s",
            Item::DuckDb => "duckdb",
            Item::Elasticsearch => "elasticsearch",
            Item::Registry => "registry",
            Item::ClaudeCode => "claude-code",
            Item::Copilot => "github-copilot",
            Item::Kokoro => "kokoro",
            Item::IndicTts => "indic-tts",
            Item::VoicePack => "voice-pack",
            Item::Lemonade => "lemonade",
            Item::Ollama => "ollama",
            Item::MacHost => "mac-host",
            Item::GpuVulkan => "gpu-vulkan",
            Item::GpuNvidia => "gpu-nvidia",
            Item::Ffmpeg => "ffmpeg",
            Item::MacFirmware => "mac-firmware",
            Item::MacCamera => CAMERA_JOB,
            Item::LinuxApps => "linux-apps",
            Item::LinuxWindows => "linux-windows",
            Item::FwQualcommBt => "fw-qualcomm-bt",
            Item::FwRealtekNet => "fw-realtek-net",
            Item::FwBroadcomNet => "fw-broadcom-net",
            Item::FwAtheros9170 => "fw-atheros-carl9170",
        }
    }

    pub fn parse(s: &str) -> Option<Item> {
        Item::ALL.into_iter().find(|i| i.id() == s.trim().to_ascii_lowercase())
    }

    pub fn title(self) -> &'static str {
        match self {
            Item::Node => "Node.js",
            Item::Python => "Python 3",
            Item::K3s => "k3s",
            Item::DuckDb => "DuckDB",
            Item::Elasticsearch => "Elasticsearch",
            Item::Registry => "Local image registry",
            Item::ClaudeCode => "Claude Code",
            Item::Copilot => "GitHub Copilot CLI",
            Item::Kokoro => "Kokoro",
            Item::IndicTts => "Indic-TTS",
            Item::VoicePack => "Voice: Whisper tiny + Kokoro",
            Item::Lemonade => "Lemonade",
            Item::Ollama => "Ollama",
            Item::MacHost => "Use a Mac's GPU (Apple Silicon)",
            Item::GpuVulkan => "GPU for local models (Vulkan)",
            Item::GpuNvidia => "NVIDIA GPU driver (CUDA)",
            Item::Ffmpeg => "Media player (ffmpeg)",
            Item::MacFirmware => "Mac Wi-Fi and Bluetooth firmware (from your Mac)",
            Item::MacCamera => "MacBook FaceTime camera: driver and firmware (firmware from Apple)",
            Item::LinuxApps => "Linux apps (Debian/Ubuntu)",
            Item::LinuxWindows => "Linux windows (graphical programs)",
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => self.fw_def().map_or("Firmware", |d| d.title),
        }
    }

    pub fn group(self) -> Group {
        match self {
            Item::Node | Item::Python => Group::Runtimes,
            Item::K3s | Item::DuckDb | Item::Elasticsearch | Item::Registry => Group::Cluster,
            Item::ClaudeCode | Item::Copilot => Group::AiCoding,
            Item::Kokoro | Item::IndicTts | Item::VoicePack => Group::Voice,
            Item::Lemonade | Item::Ollama | Item::MacHost | Item::GpuVulkan | Item::GpuNvidia => Group::LocalModels,
            Item::Ffmpeg => Group::Media,
            Item::MacFirmware | Item::MacCamera => Group::Mac,
            Item::LinuxApps | Item::LinuxWindows => Group::Linux,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => Group::Firmware,
        }
    }

    /// One line for the menu.
    pub fn about(self) -> &'static str {
        match self {
            Item::Node => "JavaScript runtime with npm (node, npm, npx): from nodejs.org, checksum-verified, kept in the data folder (no sudo)",
            Item::Python => "Python 3 with venv and pip (python3), installed through uv into the data folder (no sudo)",
            Item::K3s => "Lightweight Kubernetes on this machine: the managed dev cluster, inside the data partition, checksum-verified",
            Item::DuckDb => "SQL on CSV and Parquet files: the duckdb command (DASH uses it), set up together with the cluster",
            Item::Elasticsearch => "Search engine: one single-node Deployment in the cluster (namespace iyakki-data, port 9200 inside the cluster)",
            Item::Registry => "Where images built on this device go (the private store's server): one registry:2 pod with a volume in the cluster, created in one step, no cluster restart. Anyone who can reach this machine's port 30500 can push to it",
            Item::ClaudeCode => "Anthropic's coding agent (the claude command); the AI IDE can use it once you sign in (Settings \u{2192} Agents)",
            Item::Copilot => "GitHub's coding agent (the copilot command); needs a Copilot subscription and `copilot login`",
            Item::Kokoro => "English neural reading voice (kokoro-v1, 82M), pulled into Lemonade; Lemonade (and k3s) are installed first when missing",
            Item::VoicePack => "Listen and speak with one action: Whisper tiny (speech to text, any language it knows) and Kokoro (reading voice, English only), pulled into the Lemonade of this machine (Lemonade and k3s are installed first when missing), set as the engines of Settings \u{2192} Voice and tested. Review first: sizes, licences and what runs where. Remove puts the settings back",
            Item::IndicTts => "Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi voices: the Indic voice server in k3s",
            Item::Lemonade => "Local model server (chat, embeddings, images, whisper, Kokoro): one Deployment in k3s (namespace iyakki-ai), reachable on this machine only (127.0.0.1:13305) and added to Models",
            Item::Ollama => "Run language models on this machine; started and added to Models as ollama-local",
            Item::MacHost => "A guide, not an install: use the GPU of a Mac with Apple Silicon (Metal) or of another computer on your network. Shows what to install ON that computer (you do it), tests the connection with tokens per second, adds it to Models and can publish it to the cluster",
            Item::GpuVulkan => "Let pods in k3s use this machine's GPU through Vulkan (AMD, Intel, NVIDIA via nouveau): deploys a small device plugin, tests it with an unprivileged pod and gives Lemonade the GPU. Nothing is downloaded by IYAKKI; the cluster pulls the plugin and a llama.cpp image (about 300 MB)",
            Item::GpuNvidia => "NVIDIA's own driver for CUDA in pods (shown when an NVIDIA GPU is found; Turing or newer): downloads NVIDIA's user-space libraries and GSP firmware for you from download.nvidia.com (about 400 MB, checksum-verified, never part of an image), loads the open kernel modules of this image, deploys NVIDIA's device plugin so pods can ask for nvidia.com/gpu, and tests it with nvidia-smi",
            Item::MacFirmware => "Downloads Apple's recovery image for this Mac model from Apple (about 1 GB, resumable; for a 2017 MacBook Pro it is the macOS 11 image, the one that keeps the Bluetooth patch), reads the Wi-Fi and Bluetooth firmware out of it with 7-Zip, installs it as a driver pack and loads the drivers. A downloaded image is kept until the files are installed: Retry reads it again and never downloads it twice. The files are Apple's: they stay on this machine, are never uploaded and are not part of IYAKKI. IYAKKI menu \u{2192} Mac drivers also reads them from a folder, a stick or a disk image",
            Item::MacCamera => "MacBook FaceTime HD camera (Broadcom 14e4:1570): downloads Apple's camera firmware from Apple's update server when you click (about 17.6 MB, only the needed parts, resumable, checked), builds the firmware and sensor files, installs them as a driver pack and loads the camera driver module that is already in the Mac image (no compiler and no restart needed). Apple's files stay on this machine, are never uploaded and are not part of IYAKKI",
            Item::LinuxWindows => "Show a graphical Linux program in an IYAKKI window (LINUX WINDOW tile): makes a Debian 12 sandbox named gui and installs, with apt, TigerVNC's X server (it draws into memory), a one-window manager, fonts, software OpenGL and the xterm test program (about 250 MB, never part of an image). No graphics card and no sound in the window",
            Item::LinuxApps => "Run Debian or Ubuntu programs: downloads the official Debian 12 base for you (about 30 MB, checksum-verified, never part of an image) into a sandbox named debian with the network on and no folders mounted; apt installs packages inside it. The Store \u{2192} Setup \u{2192} Linux apps (Open its page) adds Ubuntu, more sandboxes, mounted folders, image apps and the terminal",
            Item::Ffmpeg => "Plays video and audio in IYAKKI (MEDIA tile, camera recordings, Media player). Downloads a static ffmpeg build from its builder (johnvansickle.com), GPLv3, checked with its checksum; about 42 MB",
            Item::FwQualcommBt => "Bluetooth patch files for newer Qualcomm laptop chips (QCA6390, WCN3950, WCN3988, WCN7850, QCA6698): 20 files, about 1 MB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwRealtekNet => "Firmware for PCIe Realtek Ethernet chips (RTL8111/8168, RTL8125 and relatives): 30 small files, about 140 KB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwBroadcomNet => "Firmware for Broadcom wired network chips (Tigon3 tg3, NetXtreme II bnx2, NetXtreme II 10 Gigabit bnx2x): 21 files, about 3 MB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwAtheros9170 => "Firmware for old Atheros AR9170 USB Wi-Fi sticks (carl9170 driver): one file, 13 KB. GPL-2.0 or later; the source is the carl9170fw project. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
        }
    }

    /// The licences, as the review shows them before Install (program, and model weights where there are any).
    pub fn licence(self) -> &'static str {
        match self {
            Item::Node => "MIT (Node.js; npm is Artistic-2.0)",
            Item::Python => "PSF licence (Python); uv is MIT or Apache-2.0",
            Item::K3s => "Apache-2.0",
            Item::DuckDb => "MIT",
            Item::Registry => "Apache-2.0 (CNCF Distribution registry:2). The cluster pulls the image from Docker Hub when you click Create; IYAKKI does not ship it",
            Item::Elasticsearch => "Elastic License 2.0 or SSPL (the vendor's choice for this image; free to run yourself, not OSI open source)",
            Item::ClaudeCode => "proprietary (Anthropic's commercial terms); your own Anthropic account",
            Item::Copilot => "proprietary (GitHub's terms); needs a GitHub Copilot subscription",
            Item::Kokoro => "Kokoro-82M weights: Apache-2.0 (hexgrad/Kokoro-82M); Lemonade: Apache-2.0",
            Item::VoicePack => crate::voicepack::LICENCE_SHORT,
            Item::IndicTts => "code: MIT (AI4Bharat Indic-TTS); model weights: NO licence stated by AI4Bharat. Whether IYAKKI may use them is an open owner decision; nothing is bundled, you supply the model files",
            Item::Lemonade => "Apache-2.0; each model you pull has its own licence",
            Item::Ollama => "MIT; each model you pull has its own licence",
            Item::MacHost => "nothing is installed by IYAKKI. On the Mac you choose: Ollama (MIT), llama.cpp (MIT), MLX-LM (MIT) or LM Studio (free to use, proprietary)",
            Item::GpuVulkan => "Mesa (MIT) and llama.cpp (MIT) in the pod image; the device plugin is squat/generic-device-plugin (Apache-2.0), pulled by the cluster; IYAKKI ships no GPU driver code of its own",
            Item::GpuNvidia => "kernel modules: MIT / GPL-2.0 (NVIDIA open GPU kernel modules, in the image). User-space libraries, nvidia-smi and GSP firmware: NVIDIA's proprietary driver licence, downloaded by you from NVIDIA, not shipped. Device plugin: Apache-2.0. Pressing Install accepts NVIDIA's licence for the downloaded files",
            Item::MacFirmware => "Apple's firmware files: Apple's own licence, no redistribution, so IYAKKI never ships them; they are fetched for your own Mac at your request. 7-Zip (the reader): GNU LGPL with the unRAR restriction, downloaded from its project when needed",
            Item::MacCamera => "driver: the open facetimehd driver (GPL-2.0, in the Mac image). Firmware and sensor files: Apple's own licence, no redistribution, so IYAKKI never ships them; they are fetched from Apple's CDN for your own Mac at your click",
            Item::LinuxWindows => "Debian packages, each with its own licence (TigerVNC: GPL-2.0, matchbox: GPL-2.0, Mesa: MIT, xterm: MIT, fonts: free licences), downloaded by apt inside the sandbox at your click, never shipped. \"Debian\" is a registered trademark of Software in the Public Interest, Inc.; IYAKKI does not rebrand it and is not endorsed by it",
            Item::LinuxApps => "Debian and Ubuntu are free software; every package keeps its own licence (/usr/share/doc/<package>/copyright inside the sandbox). Downloaded from the distribution, not shipped. \"Ubuntu\" and \"Debian\" are trademarks of their owners; IYAKKI does not rebrand them",
            Item::Ffmpeg => "GPLv3 (the static build you download from its builder; IYAKKI does not ship it). H.264 and AAC are patent-licensed formats: decoding them is your choice, as in the browser",
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => self.fw_def().map_or("", |d| d.licence_note),
        }
    }

    /// What has to be there first.
    pub fn needs(self) -> &'static [Item] {
        match self {
            Item::DuckDb | Item::Elasticsearch | Item::IndicTts | Item::Lemonade | Item::Registry | Item::GpuVulkan | Item::GpuNvidia => &[Item::K3s],
            Item::Kokoro | Item::VoicePack => &[Item::Lemonade],
            _ => &[],
        }
    }

    /// Everything that has to be there first: [`Item::needs`], plus the runtime the catalogue says the tool installs with (an npm
    /// tool needs Node.js, a pip tool Python 3), so the order shows "Node.js → Copilot" like "k3s → DuckDB".
    pub fn needs_now(self) -> Vec<Item> {
        let mut v: Vec<Item> = self.needs().to_vec();
        if let Some(d) = self.cli_id().and_then(crate::clis::def) {
            match d.source {
                crate::clis::Source::Npm(_) => v.push(Item::Node),
                crate::clis::Source::Pip(_) => v.push(Item::Python),
                _ => {}
            }
        }
        v
    }

    /// The CLI catalogue entry this item installs (`clis.rs`), when it is one.
    pub fn cli_id(self) -> Option<&'static str> {
        match self {
            Item::Node => Some("nodejs"),
            Item::Python => Some("python"),
            Item::DuckDb => Some("duckdb"),
            Item::ClaudeCode => Some("claude-code"),
            Item::Copilot => Some("github-copilot"),
            Item::Ollama => Some("ollama"),
            Item::Ffmpeg => Some("ffmpeg"),
            _ => None,
        }
    }

    /// About how much is downloaded (megabytes), for the Store's Setup tab and its "Install all recommended" review. Honest
    /// round numbers from the item texts above; 0 where nothing is downloaded by IYAKKI (a guide, a deploy review).
    pub fn download_mb(self) -> u32 {
        match self {
            Item::Node => 45,
            Item::Python => 40,
            Item::K3s => 70,
            Item::DuckDb => 20,
            Item::Elasticsearch => 600,
            Item::Registry => 25,
            Item::ClaudeCode => 60,
            Item::Copilot => 40,
            Item::Kokoro => 350,
            Item::VoicePack => crate::voicepack::WHISPER_MB + crate::voicepack::KOKORO_MB,
            Item::IndicTts => 0,
            Item::Lemonade => 700,
            Item::Ollama => 300,
            Item::MacHost => 0,
            Item::GpuVulkan => 300,
            Item::GpuNvidia => 400,
            Item::Ffmpeg => 42,
            Item::MacFirmware => 1500,
            Item::MacCamera => 18,
            Item::LinuxApps => 30,
            Item::LinuxWindows => 250,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwAtheros9170 => 1,
            Item::FwBroadcomNet => 3,
        }
    }

    /// About how long it takes on a normal connection, in words.
    pub fn time_text(self) -> &'static str {
        match self {
            Item::MacHost => "a few minutes (your steps)",
            Item::IndicTts => "review first, then minutes",
            Item::Elasticsearch | Item::Lemonade | Item::Kokoro | Item::VoicePack | Item::MacFirmware | Item::GpuNvidia | Item::GpuVulkan => "5 to 15 minutes",
            Item::K3s | Item::Ollama => "2 to 5 minutes",
            _ => "about a minute",
        }
    }

    /// A server that keeps running once installed (the Setup tab then says Running, not just Installed).
    pub fn is_service(self) -> bool {
        matches!(self, Item::K3s | Item::Elasticsearch | Item::Registry | Item::Lemonade | Item::Ollama | Item::IndicTts)
    }

    /// Hardware entries: shown only on a machine that has the hardware, and never in cloud mode.
    pub fn is_hardware(self) -> bool {
        matches!(self, Item::GpuVulkan | Item::GpuNvidia | Item::MacFirmware | Item::MacCamera) || self.fw_def().is_some()
    }

    /// Memory (megabytes) a machine should have before this is recommended; 0 = light.
    pub fn min_ram_mb(self) -> u64 {
        match self {
            Item::Elasticsearch => 6000,
            Item::Lemonade | Item::Ollama | Item::Kokoro | Item::VoicePack | Item::IndicTts | Item::GpuVulkan | Item::GpuNvidia => 7500,
            _ => 0,
        }
    }

    /// Installed by a detached job. Indic-TTS is not: it is deployed through the Platform services review (Settings \u{2192} Dev
    /// cluster), which plans, shows and asks before it builds the image and applies the objects.
    pub fn runs_as_job(self) -> bool {
        !matches!(self, Item::IndicTts | Item::MacHost | Item::MacCamera) && self.fw_def().is_none()
    }

    /// The restricted firmware pack this item installs (fwpack.rs), when it is one.
    pub fn fw_def(self) -> Option<&'static crate::fwpack::Def> {
        match self {
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => crate::fwpack::def_for_pack(self.id()),
            _ => None,
        }
    }
}

/// The items of a section, in the menu's order.
pub fn items_of(g: Group) -> Vec<Item> {
    Item::ALL.into_iter().filter(|i| i.group() == g).collect()
}

/// The steps for installing `target`, in dependency order: what it needs and is not there yet, then `target` itself (always,
/// so an installed item can be installed again to repair it).
pub fn order(target: Item, present: &dyn Fn(Item) -> bool) -> Vec<Item> {
    fn visit(i: Item, target: Item, present: &dyn Fn(Item) -> bool, out: &mut Vec<Item>) {
        for n in i.needs_now() {
            visit(n, target, present, out);
        }
        if (i == target || !present(i)) && !out.contains(&i) {
            out.push(i);
        }
    }
    let mut out = Vec::new();
    visit(target, target, present, &mut out);
    out
}

/// The order as the menu shows it, with the runtimes the CLI catalogue installs underneath a step: `before(item)` gives the ids
/// the catalogue runs before that step's own (from `CliManager::install_order`: "uv" then "python" for a pip tool on a machine
/// with neither), named as the catalogue names them, once each.
pub fn order_titles_with(steps: &[Item], before: &dyn Fn(Item) -> Vec<&'static str>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut add = |t: String| {
        if !v.contains(&t) {
            v.push(t);
        }
    };
    for s in steps {
        for id in before(*s) {
            if let Some(d) = crate::clis::def(id) {
                add(d.name.to_string());
            }
        }
        add(s.title().to_string());
    }
    v
}

/// "1. k3s → 2. Elasticsearch" (what the menu shows before Install).
pub fn order_text(steps: &[Item]) -> String {
    titles_text(&steps.iter().map(|s| s.title().to_string()).collect::<Vec<_>>())
}

/// "1. Node.js → 2. GitHub Copilot CLI".
pub fn titles_text(titles: &[String]) -> String {
    titles.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("  \u{2192}  ")
}

// ------------------------------------------------------------------------------------------------ what is there

/// Where an item stands on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    Missing,
    /// Installed (by IYAKKI): a short detail (version, place).
    Installed(String),
    /// Installed earlier (its marker is there), but what it needs to run does not answer now, so it cannot be checked.
    Earlier(String),
    /// There, but not IYAKKI's (another cluster on the machine): it counts as present and is never replaced.
    Elsewhere(String),
}

impl Presence {
    pub fn present(&self) -> bool {
        !matches!(self, Presence::Missing)
    }
    pub fn word(&self) -> String {
        match self {
            Presence::Missing => "not installed".into(),
            Presence::Installed(d) if d.is_empty() => "installed".into(),
            Presence::Installed(d) => format!("installed · {d}"),
            Presence::Earlier(d) if d.is_empty() => "installed earlier, not running".into(),
            Presence::Earlier(d) => format!("installed earlier, not running · {d}"),
            Presence::Elsewhere(d) => format!("present (not IYAKKI's) · {d}"),
        }
    }
}

/// The folder of this module's own files (`<data>/quick-install`).
pub fn state_dir(data: &Path) -> PathBuf {
    data.join("quick-install")
}

fn marker(data: &Path, item: Item) -> PathBuf {
    state_dir(data).join(format!("{}.done", item.id()))
}

/// Remember that a step finished (what the job's last line said).
pub fn mark_done(data: &Path, item: Item, detail: &str) -> Result<(), String> {
    std::fs::create_dir_all(state_dir(data)).map_err(|e| e.to_string())?;
    std::fs::write(marker(data, item), format!("{}\n{detail}\n", crate::clock::now_unix())).map_err(|e| e.to_string())
}

/// The marker a finished step left, with its detail.
pub fn marked(data: &Path, item: Item) -> Option<String> {
    std::fs::read_to_string(marker(data, item)).ok().map(|t| t.lines().nth(1).unwrap_or("").trim().to_string())
}

/// Where each item stands, from the services (k3s: the managed cluster, or another one on the machine; DuckDB: the CLI tile;
/// Elasticsearch: the marker its job leaves). Reads files and `/proc` only; call it when the menu opens, not every frame.
pub fn presence(svc: &crate::services::Services, item: Item) -> Presence {
    presence_with(svc, item, &crate::lemonade::probe_local_models)
}

/// [`presence`] with the Lemonade check given (`None` = nothing answers on 127.0.0.1:13305; else its downloaded models).
/// Lemonade and Kokoro are decided by that answer, with the marker only as the fallback when nothing answers ("installed
/// earlier, not running"): a marker can outlive a cluster or volume reset, so it never counts as "installed" by itself.
pub fn presence_with(svc: &crate::services::Services, item: Item, lemonade: &dyn Fn() -> Option<Vec<crate::lemonade::Model>>) -> Presence {
    let data = svc.dir();
    match item {
        Item::Lemonade | Item::Kokoro => {
            let earlier = marked(data, item);
            match lemonade() {
                Some(models) if item == Item::Lemonade => Presence::Installed(format!("answering on 127.0.0.1:{} ({} model(s))", crate::lemonade::DEFAULT_PORT, models.iter().filter(|m| m.downloaded).count())),
                Some(models) => match models.iter().find(|m| m.id == "kokoro-v1" && m.downloaded) {
                    Some(_) => Presence::Installed("kokoro-v1 downloaded in Lemonade".into()),
                    None => Presence::Missing,
                },
                None => earlier.map_or(Presence::Missing, Presence::Earlier),
            }
        }
        Item::VoicePack => {
            let earlier = marked(data, item);
            match lemonade() {
                Some(models) => {
                    let has = |id: &str| models.iter().any(|m| m.id == id && m.downloaded);
                    if has(crate::voicepack::WHISPER_MODEL) && has(crate::voicepack::KOKORO_MODEL) { Presence::Installed(format!("{} and {} downloaded in Lemonade", crate::voicepack::WHISPER_MODEL, crate::voicepack::KOKORO_MODEL)) } else { Presence::Missing }
                }
                None => earlier.map_or(Presence::Missing, Presence::Earlier),
            }
        }
        Item::K3s => {
            if let Some(st) = svc.k3sdev.installed() {
                return Presence::Installed(format!("{} {} · {}", st.backend.label(), st.version, svc.k3sdev.state().word()));
            }
            let found = svc.k3sdev.found_elsewhere();
            match found.first() {
                Some(f) => Presence::Elsewhere(f.clone()),
                None => Presence::Missing,
            }
        }
        Item::LinuxWindows => {
            let n = crate::linuxwin::count_ready(data);
            if n == 0 { Presence::Missing } else { Presence::Installed("Debian 12 sandbox gui with the display server".into()) }
        }
        Item::LinuxApps => {
            let n = crate::linuxapps::count_all(data);
            if n == 0 { Presence::Missing } else { Presence::Installed(format!("{n} sandbox(es)")) }
        }
        Item::MacHost => {
            let n = crate::modelhost::Hosts::new(data).list().len();
            if n == 0 { Presence::Missing } else { Presence::Installed(format!("{n} model host(s) in Models")) }
        }
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => match item.fw_def().and_then(|d| crate::fwpack::installed(data, d)) {
            Some(r) => Presence::Installed(format!("{} files, {}", r.files.iter().filter(|f| !f.starts_with("licences/")).count(), r.version)),
            None => Presence::Missing,
        },
        Item::MacCamera => match crate::camfw::read_outcome(data) {
            Some(o) if o.installed => Presence::Installed(if o.firmware_version.is_empty() { o.headline } else { format!("firmware {}", o.firmware_version) }),
            _ => Presence::Missing,
        },
        Item::IndicTts => {
            // the Platform services keep their instances in `installed-services-<user>.json`
            let deployed = std::fs::read_dir(data).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("installed-services-") && std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("\"indic-tts\"")));
            if deployed { Presence::Installed("deployed in the cluster (Platform services)".into()) } else { Presence::Missing }
        }
        other => match other.cli_id() {
            // installed by IYAKKI (its record), else put on the machine by hand (the tools folder, the search path): shown as
            // installed with the version it reports and where, and left alone (never replaced unless asked twice)
            Some(id) => match svc.clis.installed().into_iter().find(|i| i.id == id) {
                Some(i) => Presence::Installed(format!("version {}", i.version)),
                None => svc.clis.present_elsewhere(id).map_or(Presence::Missing, Presence::Elsewhere),
            },
            None => match marked(data, other) {
                Some(d) => Presence::Installed(d),
                None => Presence::Missing,
            },
        },
    }
}

// ------------------------------------------------------------------------------------------------ upgrades

/// Whether a newer version exists for an installed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Latest {
    /// The vendor publishes this newer version.
    Newer(String),
    /// Installed is the newest published one.
    Current(String),
    /// Cannot tell (the vendor publishes only "latest", or the lookup failed): the reason.
    Unknown(String),
    /// Nothing to compare (pinned images, deployments).
    NotApplicable,
}

/// Version texts compared the way the catalogue writes them: a leading `v` and `+`/`-` before a build suffix do not matter.
pub fn same_version(a: &str, b: &str) -> bool {
    let n = |v: &str| v.trim().trim_start_matches('v').replace('+', "-");
    !n(a).is_empty() && n(a) == n(b)
}

/// What the lookup of the newest version says about an installed `version`.
pub fn compare(installed: &str, latest: Result<String, String>) -> Latest {
    match latest {
        Ok(l) if same_version(installed, &l) => Latest::Current(l),
        Ok(l) if crate::clis::version_is_newer(installed, &l) => Latest::Newer(l),
        Ok(l) => Latest::Current(l),
        Err(e) => Latest::Unknown(e),
    }
}

/// The installed version of an item that has one to compare (k3s, and the catalogue programs).
pub fn installed_version(svc: &crate::services::Services, item: Item) -> Option<String> {
    match item {
        Item::K3s => svc.k3sdev.installed().map(|s| s.version).filter(|v| !v.is_empty()),
        other => svc.clis.is_installed(other.cli_id()?).map(|i| i.version),
    }
}

/// For every item that is installed and has a version: is a newer one published? `fetch(id)` asks the vendor (a network call;
/// the shell runs this on a thread of its own, tests give a fake).
pub fn upgrades_with(installed: &[(Item, Option<String>)], fetch: &dyn Fn(&str) -> Result<String, String>) -> Vec<(Item, Latest)> {
    installed
        .iter()
        .map(|(item, ver)| {
            let l = match (ver, if *item == Item::K3s { Some("k3s") } else { item.cli_id() }) {
                (Some(v), Some(id)) => compare(v, fetch(id)),
                _ => Latest::NotApplicable,
            };
            (*item, l)
        })
        .collect()
}

/// The real lookup: this machine's installed versions against the vendors' newest (network).
pub fn check_upgrades(svc: &crate::services::Services) -> Vec<(Item, Latest)> {
    let inst: Vec<(Item, Option<String>)> = Item::ALL.iter().map(|i| (*i, installed_version(svc, *i))).collect();
    upgrades_with(&inst, &|id| match svc.clis.check_latest(id) {
        // the catalogue's own verdict (what its tile offers) for catalogue programs; k3s is compared by version
        Ok(l) if id != "k3s" => match svc.clis.offer(id, Some(&l)) {
            crate::clis::Offer::Upgrade(v) => Ok(v),
            _ => Ok(installed_version_of(svc, id).unwrap_or(l)),
        },
        other => other,
    })
}

fn installed_version_of(svc: &crate::services::Services, id: &str) -> Option<String> {
    svc.clis.is_installed(id).map(|i| i.version)
}

/// What the menu needs to know about this machine, read off the screen's thread (it starts programs for their versions): where
/// each item stands, which installed tools cannot run, what the catalogue installs under each, and who depends on a runtime.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub presence: Vec<(Item, Presence)>,
    /// Installed by IYAKKI but not runnable (the plain words of `CliManager::health`): offered Reinstall.
    pub unhealthy: Vec<(Item, String)>,
    /// `CliManager::install_order` minus the item itself: the runtimes (uv, Python, Node.js) installed before it.
    pub runtimes: Vec<(Item, Vec<&'static str>)>,
    /// A runtime used by installed tools (`CliManager::uninstall_warning`).
    pub used_by: Vec<(Item, String)>,
    /// Rows the menu does not show on this machine (the NVIDIA driver entry when no NVIDIA GPU is found).
    pub hidden: Vec<Item>,
}

pub fn gather_facts(svc: &crate::services::Services) -> Facts {
    let mut f = Facts::default();
    f.hidden = hidden_items(&crate::gpu::detect(Path::new("/")));
    for it in Item::ALL {
        f.presence.push((it, presence(svc, it)));
        if let Some(id) = it.cli_id() {
            if let Some(h) = svc.clis.health(id) {
                f.unhealthy.push((it, h));
            }
            let before: Vec<&'static str> = svc.clis.install_order(id).into_iter().filter(|x| *x != id).collect();
            if !before.is_empty() {
                f.runtimes.push((it, before));
            }
            if let Some(w) = svc.clis.uninstall_warning(id) {
                f.used_by.push((it, w));
            }
        }
    }
    f
}

/// The rows not shown for these GPUs: the NVIDIA driver entry needs an NVIDIA GPU.
pub fn hidden_items(gpus: &[crate::gpu::Gpu]) -> Vec<Item> {
    if crate::gpu::has_nvidia(gpus) { Vec::new() } else { vec![Item::GpuNvidia] }
}

/// What the item's button says: Install, Upgrade to a newer version, or (nothing newer / unknown) Update or Reinstall.
pub fn action_label(item: Item, presence: &Presence, latest: Option<&Latest>) -> String {
    let t = item.title();
    match presence {
        // found on the machine (put there by hand): installing IYAKKI's own copy is a choice, asked twice by the menu
        Presence::Elsewhere(_) if item.cli_id().is_some() => format!("Install {t} (own copy)"),
        Presence::Installed(_) => match latest {
            Some(Latest::Newer(v)) => format!("Upgrade {t} to {}", v.trim_start_matches('v')),
            Some(Latest::Current(_)) => format!("Reinstall {t}"),
            _ if item.cli_id().is_some() || item == Item::K3s => format!("Update {t}"),
            _ => format!("Reinstall {t}"),
        },
        _ => format!("Install {t}"),
    }
}

/// The state line of a row: what is installed, and what a lookup said about newer versions.
pub fn state_text(presence: &Presence, latest: Option<&Latest>) -> String {
    let base = presence.word();
    match (presence, latest) {
        (Presence::Installed(_), Some(Latest::Newer(v))) => format!("{base} · upgrade available: {}", v.trim_start_matches('v')),
        (Presence::Installed(_), Some(Latest::Current(_))) => format!("{base} · up to date"),
        _ => base,
    }
}

// ------------------------------------------------------------------------------------------------ starting the jobs

/// How a step's job is started: the program (this one, `iyakki`; a script in tests), the data folder it works on.
#[derive(Clone, Debug)]
pub struct Launch {
    pub exe: PathBuf,
    pub data: PathBuf,
}

impl Launch {
    /// This program, on this data folder.
    pub fn this_program(data: &Path) -> Result<Launch, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        Ok(Launch { exe, data: data.to_path_buf() })
    }
}

/// The job label of a step: "Install k3s (1/2)", "Install Elasticsearch (2/2, after k3s)".
pub fn label(item: Item, step: usize, total: usize) -> String {
    label_with("Install", item, step, total)
}

/// [`label`] with another verb ("Upgrade" for something that is installed already).
pub fn label_with(verb: &str, item: Item, step: usize, total: usize) -> String {
    let after: Vec<&str> = item.needs_now().iter().map(|n| n.title()).collect();
    if after.is_empty() || step == 1 {
        format!("{verb} {} ({step}/{total})", item.title())
    } else {
        format!("{verb} {} ({step}/{total}, after {})", item.title(), after.join(", "))
    }
}

/// The job of one step.
pub fn spec(item: Item, step: usize, total: usize, l: &Launch) -> Spec {
    spec_with("Install", item, step, total, l)
}

/// [`spec`] with the verb of the label.
pub fn spec_with(verb: &str, item: Item, step: usize, total: usize, l: &Launch) -> Spec {
    Spec {
        label: label_with(verb, item, step, total),
        cmd: Cmd::Argv(vec![l.exe.display().to_string(), "quick-install".into(), item.id().into(), "--data".into(), l.data.display().to_string()]),
        cwd: l.data.clone(),
        workspace: WORKSPACE.into(),
    }
}

/// The job of the Mac firmware step with a source other than Apple's download (a folder, a stick or a disk image the person picked):
/// `iyakki quick-install mac-firmware --data D --source folder --path P`.
pub fn mac_firmware_spec(l: &Launch, source: &crate::macfw::Source) -> Spec {
    let mut argv: Vec<String> = vec![l.exe.display().to_string(), "quick-install".into(), Item::MacFirmware.id().into(), "--data".into(), l.data.display().to_string()];
    argv.extend(source.args());
    Spec { label: format!("Install Mac firmware ({})", source.words()), cmd: Cmd::Argv(argv), cwd: l.data.clone(), workspace: WORKSPACE.into() }
}

/// Start the Mac firmware job for `source`. Refuses when one is running already.
pub fn start_mac_firmware(jobs: &Jobs, owner: &str, source: &crate::macfw::Source, l: &Launch) -> Result<Job, String> {
    if jobs.visible(owner, WORKSPACE).iter().any(|j| j.running() && item_of(j) == Some(Item::MacFirmware)) {
        return Err("The Mac firmware is being installed already: its progress is beside the clock in the top bar".into());
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &mac_firmware_spec(l, source), &|_: &Path| Ok(()))
}

/// The job of the camera firmware (FaceTime HD camera of a MacBook, camfw.rs): `iyakki quick-install mac-camera --data D`. Not an [`Item`]
/// (the Setup tab lists items; this one is started from the Mac drivers page and by hand over SSH).
pub fn camera_firmware_spec(l: &Launch) -> Spec {
    Spec {
        label: "Camera firmware (FaceTime HD)".into(),
        cmd: Cmd::Argv(vec![l.exe.display().to_string(), "quick-install".into(), CAMERA_JOB.into(), "--data".into(), l.data.display().to_string()]),
        cwd: l.data.clone(),
        workspace: WORKSPACE.into(),
    }
}

/// The word after `quick-install` of the camera firmware job.
pub const CAMERA_JOB: &str = "mac-camera";

/// Is this job the camera firmware job?
pub fn is_camera_job(job: &Job) -> bool {
    let words: Vec<&str> = job.command.split_whitespace().collect();
    words.iter().position(|w| *w == "quick-install").and_then(|i| words.get(i + 1)) == Some(&CAMERA_JOB)
}

/// Start the camera firmware job. Refuses when one is running already.
pub fn start_camera_firmware(jobs: &Jobs, owner: &str, l: &Launch) -> Result<Job, String> {
    if jobs.visible(owner, WORKSPACE).iter().any(|j| j.running() && is_camera_job(j)) {
        return Err("The camera firmware is being installed already: its progress is beside the clock in the top bar".into());
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &camera_firmware_spec(l), &|_: &Path| Ok(()))
}

/// The job of a restricted firmware pack (fwpack.rs): `iyakki quick-install fw-<name> --data D [--from FOLDER]`. Not a step of the
/// installer's order: the licence has to be accepted first ([`crate::fwpack::accept`]), and the job refuses without it.
pub fn firmware_pack_spec(l: &Launch, item: Item, from: Option<&Path>) -> Spec {
    let mut argv = vec![l.exe.display().to_string(), "quick-install".into(), item.id().into(), "--data".into(), l.data.display().to_string()];
    if let Some(f) = from {
        argv.push("--from".into());
        argv.push(f.display().to_string());
    }
    Spec { label: format!("{}{}", item.title(), if from.is_some() { " (from a folder)" } else { "" }), cmd: Cmd::Argv(argv), cwd: l.data.clone(), workspace: WORKSPACE.into() }
}

/// Start a firmware pack job. Refuses when the item is not a pack, the licence is not accepted, or one is running already.
pub fn start_firmware_pack(jobs: &Jobs, owner: &str, item: Item, from: Option<&Path>, l: &Launch) -> Result<Job, String> {
    let Some(def) = item.fw_def() else { return Err(format!("{} is not a firmware pack", item.title())) };
    if crate::fwpack::accepted(&l.data, def).is_none() {
        return Err(format!("Read and accept the vendor's licence first ({}); nothing is downloaded or installed before that.", def.title));
    }
    if jobs.visible(owner, WORKSPACE).iter().any(|j| j.running() && item_of(j) == Some(item)) {
        return Err(format!("{} is being installed already: its progress is beside the clock in the top bar", def.title));
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &firmware_pack_spec(l, item, from), &|_: &Path| Ok(()))
}

/// The job of the GPU page's test prompt: `iyakki quick-install gpu-test`. Its log is the result (tokens per second).
pub fn gpu_test_spec(l: &Launch) -> Spec {
    Spec {
        label: "GPU test prompt".into(),
        cmd: Cmd::Argv(vec![l.exe.display().to_string(), "quick-install".into(), "gpu-test".into(), "--data".into(), l.data.display().to_string()]),
        cwd: l.data.clone(),
        workspace: WORKSPACE.into(),
    }
}

/// Which item a job of [`WORKSPACE`] installs (from its command).
pub fn item_of(job: &Job) -> Option<Item> {
    let words: Vec<&str> = job.command.split_whitespace().collect();
    let i = words.iter().position(|w| *w == "quick-install")?;
    words.get(i + 1).and_then(|w| Item::parse(w))
}

/// The items whose newest install job did not succeed (failed, stopped or lost): the menu offers Retry on them.
pub fn failed_items(jobs: &[Job]) -> Vec<Item> {
    let mut seen: Vec<Item> = Vec::new();
    let mut failed: Vec<Item> = Vec::new();
    let mut sorted: Vec<&Job> = jobs.iter().collect();
    sorted.sort_by_key(|j| std::cmp::Reverse(j.started_ms));
    for j in sorted {
        let Some(i) = item_of(j) else { continue };
        if seen.contains(&i) || j.running() {
            seen.push(i);
            continue;
        }
        seen.push(i);
        if j.state != crate::jobs::State::Succeeded {
            failed.push(i);
        }
    }
    failed
}

/// The installer jobs of a person (running first, newest first), not dismissed.
pub fn install_jobs(jobs: &Jobs, owner: &str) -> Vec<Job> {
    let mut v = jobs.visible(owner, WORKSPACE);
    v.sort_by_key(|j| !j.running());
    v
}

/// Start the jobs that install `target` (and what it needs first). Refuses when an item of the order is being installed already.
pub fn start(jobs: &Jobs, owner: &str, target: Item, present: &dyn Fn(Item) -> bool, l: &Launch) -> Result<Vec<Job>, String> {
    let steps: Vec<Item> = order(target, present).into_iter().filter(|i| i.runs_as_job()).collect();
    if steps.is_empty() {
        return Ok(Vec::new());
    }
    let busy: Vec<Item> = jobs.visible(owner, WORKSPACE).iter().filter(|j| j.running()).filter_map(item_of).collect();
    if let Some(b) = steps.iter().find(|s| busy.contains(s)) {
        return Err(format!("{} is being installed already: its progress is beside the clock in the top bar", b.title()));
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    let total = steps.len();
    let mut started = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        // something installed already is upgraded (installed again from the newest release), the rest installed
        let verb = if present(*s) { "Upgrade" } else { "Install" };
        match jobs.start(owner, &spec_with(verb, *s, i + 1, total, l), &|_: &Path| Ok(())) {
            Ok(j) => started.push(j),
            Err(e) => {
                // what started keeps running; a later step that could not start is said plainly
                return if started.is_empty() { Err(e) } else { Err(format!("{} started, {} did not: {e}", started.len(), s.title())) };
            }
        }
    }
    Ok(started)
}

// ------------------------------------------------------------------------------------------------ the k3s start handshake

fn start_request(data: &Path) -> PathBuf {
    after_file(data, "k3s")
}

fn after_file(data: &Path, what: &str) -> PathBuf {
    state_dir(data).join(format!("after-{what}"))
}

/// A job finished installing a server (`k3s`, `ollama`, `lemonade`): the shell starts it and connects it ([`service_requests`]).
pub fn request_after(data: &Path, what: &str, user: &str) -> Result<(), String> {
    std::fs::create_dir_all(state_dir(data)).map_err(|e| e.to_string())?;
    std::fs::write(after_file(data, what), user).map_err(|e| e.to_string())
}

/// A waiting request for `what`, taken (removed) at once: `Some(user)` once.
pub fn take_after(data: &Path, what: &str) -> Option<String> {
    let p = after_file(data, what);
    let who = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(if who.trim().is_empty() { "system".to_string() } else { who.trim().to_string() })
}

/// Called by the shell every few seconds: start and connect what Quick install jobs asked for. k3s starts under the cluster's
/// supervisor; Ollama under its own (and becomes `ollama-local` in Models when it answers); Lemonade already runs in k3s, so it is only
/// added to Models as `lemonade-local` once it answers. Returns one line per thing done.
pub fn service_requests(svc: &std::sync::Arc<crate::services::Services>) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    if let Some(r) = service_start_request(svc) {
        out.push(r.map(|_| "k3s is installed and starting (Settings \u{2192} Dev cluster shows it)".to_string()).map_err(|e| format!("k3s was installed but did not start: {e}")));
    }
    if take_after(svc.dir(), "ollama").is_some() {
        out.push(match svc.ollama.start() {
            Ok(()) => Ok("Ollama is installed and starting; it joins Models as ollama-local when it answers".into()),
            Err(e) if e.contains("already running") => Ok("Ollama is installed (already running)".into()),
            Err(e) => Err(format!("Ollama was installed but did not start: {e}")),
        });
    }
    if take_after(svc.dir(), "lemonade").is_some() {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-lemonade-connect".into()).spawn(move || {
            for _ in 0..90 {
                if let Some((base, version)) = crate::lemonade::probe_local(crate::lemonade::DEFAULT_PORT) {
                    me.use_local_lemonade(&base, version.as_deref());
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        out.push(Ok("Lemonade is running in k3s; it joins Models as lemonade-local when it answers".into()));
    }
    // Kokoro was pulled into Lemonade: make it the reading voice (TALES, chat replies) when the person has not chosen one
    if take_after(svc.dir(), "kokoro").is_some() {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-kokoro-voice".into()).spawn(move || {
            for _ in 0..90 {
                if configure_kokoro_voice(&me) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        out.push(Ok("Kokoro is installed in Lemonade; it becomes the reading voice for TALES and chat replies unless you chose another voice".into()));
    }
    // the voice pack was pulled and tested: set Whisper tiny and Kokoro as the engines of Settings \u{2192} Voice (the old ones are remembered for Undo)
    if take_after(svc.dir(), "voice-pack").is_some() {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-voice-pack".into()).spawn(move || {
            for _ in 0..90 {
                if crate::voicepack::configure_when_ready(&me) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        out.push(Ok("Whisper tiny and Kokoro are installed in Lemonade; they become the speech to text and reading voice of Settings \u{2192} Voice (Remove in the tile's details undoes it)".into()));
    }
    // the GPU steps: the cluster restarts once so containerd reads the CDI setting; after a start of IYAKKI the RAM holds nothing of what
    // they did (CDI specs, the NVIDIA modules and device nodes), so it is put back, once per start for the driver
    if let Some(user) = take_after(svc.dir(), "k3s-restart") {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-gpu-restart".into()).spawn(move || {
            let _ = me.k3sdev.restart(&user);
        });
        out.push(Ok("k3s restarts once so its containerd reads the CDI setting (GPU for local models)".into()));
    }
    // put the GPU steps' RAM state back (render-node modes, NVIDIA modules, device nodes, CDI specs) on a thread: loading a module can take
    // a while, and this runs where the screen is drawn
    static GPU_RESTORE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static GPU_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static GPU_DONE: std::sync::Mutex<Vec<Result<String, String>>> = std::sync::Mutex::new(Vec::new());
    out.extend(std::mem::take(&mut *GPU_DONE.lock().unwrap_or_else(|e| e.into_inner())));
    if GPU_RESTORE.load(std::sync::atomic::Ordering::Relaxed) < 3 && (marked(svc.dir(), Item::GpuVulkan).is_some() || marked(svc.dir(), Item::GpuNvidia).is_some()) && !GPU_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let data = svc.dir().to_path_buf();
        let _ = std::thread::Builder::new().name("iyakki-gpu-restore".into()).spawn(move || {
            let r = crate::gpu_steps::ensure_runtime(Path::new("/"), &data, &mut |a: &[String]| crate::wsexec::Runner::run(&crate::wsexec::System, Path::new("/"), a, &[], Duration::from_secs(120), 64 * 1024, None));
            if !r.is_empty() {
                GPU_RESTORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                GPU_DONE.lock().unwrap_or_else(|e| e.into_inner()).extend(r);
            }
            GPU_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    out
}

/// Make Kokoro the reading voice once the local Lemonade answers and lists `kokoro-v1` as downloaded: the Lemonade server joins
/// Models (as `lemonade-local`) when it is not there yet, its model list is read (so the reading-voice fallback and Settings see the
/// model), and when no reading voice is set (Settings \u{2192} Voice) `kokoro-v1` becomes it. A voice the person chose is never replaced.
/// Returns true when it is settled (done, or nothing to do), false to try again.
pub fn configure_kokoro_voice(svc: &std::sync::Arc<crate::services::Services>) -> bool {
    let ports = crate::lemonade::local_ports_override().map(|o| o.1).unwrap_or_else(|| crate::lemonade::local_ports().to_vec());
    let Some((base, version)) = ports.into_iter().find_map(crate::lemonade::probe_local) else { return false };
    svc.use_local_lemonade(&base, version.as_deref());
    let want = crate::lemonade::api_base(&base);
    let id = {
        let st = svc.settings.lock().unwrap();
        st.get().providers.iter().find(|p| p.kind == crate::providers::Kind::Lemonade && p.base_url.as_deref().is_some_and(|u| crate::lemonade::api_base(u) == want)).map(|p| p.id.clone())
    };
    let Some(id) = id else { return false };
    match svc.refresh_lemonade(&id) {
        Ok(list) if list.iter().any(|m| m.id == "kokoro-v1" && m.downloaded) => {}
        _ => return false,
    }
    let chosen = svc.settings.lock().unwrap().get().voice.tts.is_some();
    if !chosen {
        let _ = svc.use_lemonade_voice(&id, "kokoro-v1", None);
    }
    true
}

/// The k3s job leaves this after installing: the process that owns the cluster's supervisor (the shell) starts it.
pub fn request_start(data: &Path, user: &str) -> Result<(), String> {
    request_after(data, "k3s", user)
}

/// A start request that is waiting, taken (removed) at once: `Some(user)` once.
pub fn take_start_request(data: &Path) -> Option<String> {
    let _ = start_request(data);
    take_after(data, "k3s")
}

/// Called by the shell every few seconds: start the managed cluster when a quick-install job asked for it. Returns what happened.
pub fn service_start_request(svc: &std::sync::Arc<crate::services::Services>) -> Option<Result<(), String>> {
    let user = take_start_request(svc.dir())?;
    if matches!(svc.k3sdev.state(), crate::k3sdev::Svc::Starting | crate::k3sdev::Svc::Running) {
        return Some(Ok(()));
    }
    Some(svc.k3sdev.start(&user))
}

// ------------------------------------------------------------------------------------------------ running a step (the job)

/// Everything a step does to the world. [`RealEnv`] is this machine; tests give a fake.
pub trait StepEnv {
    /// One line of output (the job's log).
    fn say(&mut self, line: &str);
    /// How the install of something this step needs stands: ready on the machine, still being installed, or its own job failed
    /// first (with the reason: the last line of its log).
    fn dep_state(&mut self, dep: Item) -> DepState;
    /// A cluster is installed (IYAKKI's, or another one on the machine).
    fn k3s_present(&mut self) -> bool;
    /// Another cluster (not IYAKKI's) is on the machine: its description.
    fn k3s_elsewhere(&mut self) -> Option<String>;
    /// Look up and install the managed cluster (download, checksum, state), saying how far it is.
    fn install_k3s(&mut self) -> Result<String, String>;
    /// Ask the supervisor to start it.
    fn request_k3s_start(&mut self) -> Result<(), String>;
    /// The cluster answers and its node is Ready.
    fn cluster_ready(&mut self) -> bool;
    /// Install a CLI catalogue program (DuckDB), waiting for it; returns its version.
    fn install_cli(&mut self, id: &str) -> Result<String, String>;
    /// `kubectl apply` a manifest (kept as `<name>.yaml` in the module's folder); returns what kubectl printed.
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String>;
    /// Wait for a Deployment's rollout.
    fn rollout(&mut self, namespace: &str, name: &str, timeout: Duration) -> Result<(), String>;
    fn sleep(&mut self, d: Duration);
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String>;
    /// Ask the shell to start and connect a server once it is installed (`ollama`, `lemonade`).
    fn request_after(&mut self, what: &str) -> Result<(), String>;
    /// A Lemonade server answers on this machine.
    fn lemonade_ready(&mut self) -> bool;
    /// Pull a model into the local Lemonade server.
    fn lemonade_pull(&mut self, model: &str) -> Result<(), String>;
    /// The ids of the models Lemonade has downloaded now (the voice pack pulls only what is missing).
    fn lemonade_models(&mut self) -> Result<Vec<String>, String> {
        Err("reading Lemonade's models is not available here".into())
    }
    /// Delete a model from Lemonade (taking back what a failed voice pack pulled).
    fn lemonade_delete(&mut self, _model: &str) -> Result<(), String> {
        Err("deleting a Lemonade model is not available here".into())
    }
    /// The voice pack's self-test (Kokoro speaks, Whisper tiny listens): the plain pass line, or the plain fail line.
    fn voice_selftest(&mut self) -> Result<String, String> {
        Err("the voice self-test is not available here".into())
    }
    /// Remember the models the voice pack pulled (so Remove deletes only those).
    fn record_pulled(&mut self, _models: &[String]) -> Result<(), String> {
        Ok(())
    }
    /// The registry step was given a cluster of its own (a chosen cluster, not the managed k3s): it does not wait for k3s.
    fn registry_on_other_cluster(&mut self) -> bool {
        false
    }
    /// Make `kubectl` and `k3s` commands in the tool folder (the k3s program answers to both names), so terminals and agents can use
    /// them; returns a line to say, empty when there is nothing to say.
    fn link_k3s_tools(&mut self) -> Result<String, String> {
        Ok(String::new())
    }
    /// The machine side of the GPU steps (files under a root, programs, the vendor download); `None` where there is none.
    fn gpu(&mut self) -> Option<&mut dyn crate::gpu_steps::GpuEnv> {
        None
    }
    /// Create the local image registry in the cluster (`localreg.rs`), saying each step through [`StepEnv::say`]; returns the address
    /// the pods pull it at. Idempotent: a registry that is ready is only reported.
    fn create_registry(&mut self) -> Result<String, String> {
        Err("creating the registry is not available here".into())
    }
    /// Install Apple's Wi-Fi and Bluetooth firmware for this T2 Mac from the job's source (`macfw.rs`); returns the plain result line.
    fn mac_firmware(&mut self) -> Result<String, String> {
        Err("installing the Mac firmware is not available here".into())
    }
    /// Make the first Debian sandbox (`linuxapps.rs`); returns the plain result line.
    fn linux_apps(&mut self) -> Result<String, String> {
        Err("making a Linux apps sandbox is not available here".into())
    }
    /// Make the `gui` sandbox and install the display server in it (`linuxwin.rs`); returns the plain result line.
    fn linux_windows(&mut self) -> Result<String, String> {
        Err("installing Linux windows is not available here".into())
    }
}

/// The Elasticsearch objects: a namespace labelled as IYAKKI's, a volume claim, one Deployment and a ClusterIP Service.
pub fn elasticsearch_manifest() -> String {
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: elasticsearch-data
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 5Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: elasticsearch
  namespace: {ns}
  labels:
    app.kubernetes.io/name: elasticsearch
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: elasticsearch
  template:
    metadata:
      labels:
        app.kubernetes.io/name: elasticsearch
        app.kubernetes.io/managed-by: iyakki
    spec:
      securityContext:
        fsGroup: 1000
      containers:
        - name: elasticsearch
          image: {img}
          env:
            - name: discovery.type
              value: single-node
            - name: xpack.security.enabled
              value: \"false\"
            - name: ES_JAVA_OPTS
              value: -Xms512m -Xmx512m
          ports:
            - containerPort: 9200
          resources:
            requests:
              memory: 1Gi
            limits:
              memory: 1536Mi
          readinessProbe:
            httpGet:
              path: /_cluster/health?local=true
              port: 9200
            initialDelaySeconds: 20
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /usr/share/elasticsearch/data
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: elasticsearch-data
---
apiVersion: v1
kind: Service
metadata:
  name: elasticsearch
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: elasticsearch
  ports:
    - port: 9200
      targetPort: 9200
",
        ns = NAMESPACE,
        img = ES_IMAGE
    )
}

/// The Lemonade objects: a namespace labelled as IYAKKI's, a volume claim, one Deployment and a ClusterIP Service. The port is published on
/// this machine's loopback only (the API has no login), and an init container hands the volume to the image's unprivileged user (10001).
pub fn lemonade_manifest() -> String {
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: lemonade-data
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 30Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: lemonade
  namespace: {ns}
  labels:
    app.kubernetes.io/name: lemonade
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: lemonade
  template:
    metadata:
      labels:
        app.kubernetes.io/name: lemonade
        app.kubernetes.io/managed-by: iyakki
    spec:
      initContainers:
        - name: own-the-volume
          image: {img}
          command: [sh, -c, \"mkdir -p /data/huggingface /data/llama /data/cache /data/config && chown -R 10001:10001 /data\"]
          securityContext:
            runAsUser: 0
          volumeMounts:
            - name: data
              mountPath: /data
      containers:
        - name: lemonade
          image: {img}
          ports:
            - containerPort: {port}
              hostPort: {port}
              hostIP: 127.0.0.1
          resources:
            requests:
              memory: 1Gi
          readinessProbe:
            httpGet:
              path: /live
              port: {port}
            initialDelaySeconds: 5
            periodSeconds: 10
          volumeMounts:
            - name: data
              subPath: huggingface
              mountPath: /opt/lemonade/.cache/huggingface
            - name: data
              subPath: llama
              mountPath: /opt/lemonade/llama
            - name: data
              subPath: cache
              mountPath: /opt/lemonade/.cache/lemonade
            - name: data
              subPath: config
              mountPath: /opt/lemonade/.config/lemonade
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: lemonade-data
---
apiVersion: v1
kind: Service
metadata:
  name: lemonade
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: lemonade
  ports:
    - port: {port}
      targetPort: {port}
",
        ns = LEMONADE_NAMESPACE,
        img = LEMONADE_IMAGE,
        port = crate::lemonade::DEFAULT_PORT
    )
}

/// Deploy Lemonade (the cluster must be Ready) and wait for its pod; the same for the Lemonade step and for Kokoro, which needs it. Applying
/// again changes nothing, so both can run it.
fn deploy_lemonade(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    wait_for_cluster(env, wait, poll)?;
    let out = env.kubectl_apply("lemonade", &lemonade_manifest())?;
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
    env.say(&format!("waiting for the pod (the first start pulls {LEMONADE_IMAGE}, about 1 GB)"));
    env.rollout(LEMONADE_NAMESPACE, "lemonade", Duration::from_secs(1200))
}

/// Make sure Lemonade answers on this machine: when its own step has not finished (or failed), deploy it here too (applying again changes
/// nothing); a failure of its own job stops this step at once. Kokoro and the voice pack both start with it.
pub(crate) fn ensure_lemonade(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    if !env.lemonade_ready() {
        deploy_lemonade(env, wait, poll)?;
        env.request_after("lemonade")?;
    }
    env.say("waiting for the Lemonade server");
    let mut waited = Duration::ZERO;
    while !env.lemonade_ready() {
        fail_if_dep_failed(env, Item::Lemonade)?;
        if waited >= wait {
            return Err(format!("Lemonade did not answer on port {} after {} minutes; see the Lemonade job's log", crate::lemonade::DEFAULT_PORT, wait.as_secs() / 60));
        }
        env.sleep(poll);
        waited += poll;
    }
    Ok(())
}

/// How a step's prerequisite stands (see [`StepEnv::dep_state`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepState {
    Ready,
    Pending,
    /// Its own install failed (or was stopped or lost): the reason.
    Failed(String),
}

/// The plain message of a step whose prerequisite failed first.
pub fn dep_failed_message(dep: Item, why: &str) -> String {
    let why = if why.trim().is_empty() { "no reason in its log" } else { why.trim() };
    format!("{} failed first, so this step stopped without waiting. See its log in the installer runs: {why}", dep.title())
}

/// Stop at once when a prerequisite's own install has failed.
fn fail_if_dep_failed(env: &mut dyn StepEnv, dep: Item) -> Result<(), String> {
    match env.dep_state(dep) {
        DepState::Failed(why) => Err(dep_failed_message(dep, &why)),
        _ => Ok(()),
    }
}

/// Wait until a runtime (Node.js, Python 3) a tool needs is on the machine; fail at once when its install failed.
fn wait_for_runtime(env: &mut dyn StepEnv, dep: Item, wait: Duration, poll: Duration) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    let mut said = false;
    loop {
        match env.dep_state(dep) {
            DepState::Ready => return Ok(()),
            DepState::Failed(why) => return Err(dep_failed_message(dep, &why)),
            DepState::Pending => {}
        }
        if waited >= wait {
            return Err(format!("{} was not installed after {} minutes; see its job's log", dep.title(), wait.as_secs() / 60));
        }
        if !said {
            env.say(&format!("waiting for {} to be installed first", dep.title()));
            said = true;
        }
        env.sleep(poll);
        waited += poll;
    }
}

/// Wait until the cluster is installed and Ready (a step that needs it). Says how long it has waited about every 30 s. Stops at
/// once when k3s's own install failed.
pub(crate) fn wait_for_cluster(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    let mut said = Duration::ZERO;
    loop {
        if env.k3s_present() && env.cluster_ready() {
            return Ok(());
        }
        fail_if_dep_failed(env, Item::K3s)?;
        if waited >= wait {
            return Err(format!("k3s was not installed and Ready after {} minutes; see the k3s job's log and Settings \u{2192} Dev cluster", wait.as_secs() / 60));
        }
        if waited >= said {
            env.say(&format!("waiting for k3s to be installed and Ready ({}s)", waited.as_secs()));
            said = waited + Duration::from_secs(30);
        }
        env.sleep(poll);
        waited += poll;
    }
}

/// Do one step. `wait` is how long a step that needs the cluster waits for it; `poll` how often it looks.
pub fn run_step(item: Item, env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    // a prerequisite that already failed ends this step now, before any waiting (the job exits and the list shows why)
    for dep in item.needs_now() {
        fail_if_dep_failed(env, dep)?;
    }
    // the runtimes a tool needs (Node.js, Python 3) are installed by their own steps first: wait for them
    for dep in item.needs_now().into_iter().filter(|d| matches!(d, Item::Node | Item::Python)) {
        wait_for_runtime(env, dep, wait, poll)?;
    }
    match item {
        Item::K3s => {
            if let Some(other) = env.k3s_elsewhere() {
                env.say(&format!("[1/1] a cluster is already on this machine ({other}); it is used as it is, nothing was installed"));
                return Ok(());
            }
            if env.k3s_present() {
                env.say("[1/2] k3s is installed already (IYAKKI's dev cluster)");
            } else {
                env.say("[0/2] looking up the latest k3s release and its checksum");
                let what = env.install_k3s()?;
                env.say(&format!("[1/2] installed: {what}"));
            }
            match env.link_k3s_tools() {
                Ok(l) if !l.is_empty() => env.say(&l),
                Ok(_) => {}
                Err(e) => env.say(&format!("note: {e}")),
            }
            env.request_k3s_start()?;
            env.say("[2/2] asked IYAKKI to start the cluster (Settings \u{2192} Dev cluster shows it; it also starts with IYAKKI from now on)");
            env.mark(Item::K3s, "installed by Quick install")?;
            Ok(())
        }
        Item::DuckDb => {
            env.say("[0/3] DuckDB is installed with k3s: checking the cluster first");
            wait_for_cluster(env, wait, poll)?;
            env.say("[1/3] k3s is Ready; installing the duckdb program from the CLI catalogue");
            let v = env.install_cli("duckdb")?;
            env.say(&format!("[2/3] duckdb {v} installed (the CLI tile lists it)"));
            env.mark(Item::DuckDb, &format!("version {v}"))?;
            env.say("[3/3] done: run `duckdb` in the CLI tile, or add a DuckDB source in DASH");
            Ok(())
        }
        Item::Elasticsearch => {
            env.say("[0/4] Elasticsearch runs in k3s: checking the cluster first");
            wait_for_cluster(env, wait, poll)?;
            env.say(&format!("[1/4] k3s is Ready; applying the Elasticsearch objects in namespace {NAMESPACE}"));
            let out = env.kubectl_apply("elasticsearch", &elasticsearch_manifest())?;
            for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
                env.say(&format!("  {}", l.trim()));
            }
            env.say(&format!("[2/4] waiting for the pod (the first start pulls {ES_IMAGE}, about 600 MB)"));
            env.rollout(NAMESPACE, "elasticsearch", Duration::from_secs(900))?;
            env.say("[3/4] Elasticsearch is running");
            let detail = format!("http://elasticsearch.{NAMESPACE}.svc:9200 inside the cluster");
            env.mark(Item::Elasticsearch, &detail)?;
            env.say(&format!("[4/4] done: {detail} (kubectl -n {NAMESPACE} port-forward svc/elasticsearch 9200 to reach it from here)"));
            Ok(())
        }
        Item::Node | Item::Python | Item::ClaudeCode | Item::Copilot | Item::Ollama | Item::Ffmpeg => {
            let id = item.cli_id().unwrap_or_default();
            let server = item == Item::Ollama;
            let total = if server { 3 } else { 2 };
            env.say(&format!("[0/{total}] installing {} from the CLI catalogue (licence: {})", item.title(), item.licence()));
            let v = env.install_cli(id)?;
            env.say(&format!("[1/{total}] {} {v} installed (the CLI tile lists it)", item.title()));
            if server {
                env.request_after(id)?;
                env.say(&format!("[2/{total}] asked IYAKKI to start it and add it to Settings \u{2192} Models"));
            }
            env.mark(item, &format!("version {v}"))?;
            env.say(&format!("[{total}/{total}] done{}", match item {
                Item::ClaudeCode => ": press Login next to it (here, in the CLI list or in Settings \u{2192} Agents; it runs `claude auth login`), then choose it in Settings \u{2192} Agents",
                Item::Copilot => ": press Login next to it (here, in the CLI list or in Settings \u{2192} Agents; it runs `copilot login`)",
                Item::Node => ": node, npm and npx are in the CLI tile's PATH",
                Item::Python => ": python3 is in the CLI tile's PATH (Python tools use it)",
                Item::Ffmpeg => ": MEDIA and the IYAKKI menu \u{2192} Media player play video and sound with it now",
                _ => "",
            }));
            Ok(())
        }
        Item::Lemonade => {
            env.say(&format!("[0/4] Lemonade runs in k3s: checking the cluster first (licence: {})", item.licence()));
            deploy_lemonade(env, wait, poll)?;
            env.say("[1/4] the Lemonade pod is ready");
            env.request_after("lemonade")?;
            env.say("[2/4] asked IYAKKI to add it to Settings \u{2192} Models");
            let detail = format!("http://127.0.0.1:{} on this machine; lemonade.{LEMONADE_NAMESPACE}.svc inside the cluster", crate::lemonade::DEFAULT_PORT);
            env.mark(Item::Lemonade, &detail)?;
            env.say(&format!("[3/4] done: {detail}"));
            env.say("[4/4] models are pulled later from Settings \u{2192} Models (kept in the cluster's volume, 30 GB claimed)");
            Ok(())
        }
        Item::Kokoro => {
            env.say(&format!("[0/3] Kokoro reads through Lemonade: making sure Lemonade runs in k3s (licence: {})", item.licence()));
            ensure_lemonade(env, wait, poll)?;
            env.say("[1/3] Lemonade answers; pulling kokoro-v1 (about 330 MB)");
            env.lemonade_pull("kokoro-v1")?;
            env.mark(Item::Kokoro, "kokoro-v1 in Lemonade")?;
            env.say("[2/3] kokoro-v1 is downloaded; asking IYAKKI to make it the reading voice (unless you already chose another one)");
            env.request_after("kokoro")?;
            env.say("[3/3] done: Kokoro reads TALES and chat replies. Change it in Settings \u{2192} Voice, or Settings \u{2192} Models \u{2192} lemonade-local \u{2192} kokoro-v1 \u{2192} Use as reading voice");
            Ok(())
        }
        Item::VoicePack => crate::voicepack::run(env, wait, poll),
        Item::Registry => {
            env.say(&format!("the local image registry runs in the cluster: checking it first (licence: {})", item.licence()));
            if !env.registry_on_other_cluster() {
                wait_for_cluster(env, wait, poll)?;
            }
            let pull = env.create_registry()?;
            env.mark(Item::Registry, &format!("pods pull {pull}"))?;
            env.say(&format!("done: the registry is ready; the private store deploy now pushes its image there (pods pull {pull})"));
            Ok(())
        }
        Item::GpuVulkan => crate::gpu_steps::run_vulkan(env, wait, poll),
        Item::GpuNvidia => crate::gpu_steps::run_nvidia(env, wait, poll),
        Item::MacFirmware => {
            let line = env.mac_firmware()?;
            env.mark(Item::MacFirmware, &line)?;
            Ok(())
        }
        Item::MacCamera => Err("the camera firmware has its own job: `iyakki quick-install mac-camera`".into()),
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 => Err(format!("this firmware pack has its own job: `iyakki quick-install {}`", item.id())),
        Item::MacHost => Err("the Mac model host is a guide: open it from Store \u{2192} Setup or Settings \u{2192} Models".into()),
        Item::LinuxApps => {
            env.say(crate::linuxapps_dist::LICENCE_NOTE);
            let line = env.linux_apps()?;
            env.mark(Item::LinuxApps, &line)?;
            Ok(())
        }
        Item::LinuxWindows => {
            env.say(crate::linuxwin::NOTE);
            let line = env.linux_windows()?;
            env.mark(Item::LinuxWindows, &line)?;
            Ok(())
        }
        Item::IndicTts => Err("Indic-TTS is deployed through the Platform services review (Settings \u{2192} Dev cluster \u{2192} Platform services), not by a job".into()),
    }
}

/// This machine: the managed cluster of `k3sdev.rs`, the CLI catalogue, the real clock. Prints to stdout (the job's log).
pub struct RealEnv {
    data: PathBuf,
    user: String,
    k3s: std::sync::Arc<crate::k3sdev::K3sDev>,
    clis: std::sync::Arc<crate::clis::CliManager>,
    streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
    /// The registry step's own cluster (`--kubeconfig`), the volume size (`--size`) and NodePort (`--port`).
    registry: RegistryArgs,
    /// Where the Mac firmware step reads Apple's files from (`--source`, `--path`).
    mac_source: crate::macfw::Source,
}

/// What `iyakki quick-install registry` was told besides the data folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryArgs {
    pub target: Option<crate::localreg::Target>,
    pub size_gi: u32,
    pub node_port: u16,
}

impl RealEnv {
    pub fn new(data: &Path, user: &str) -> RealEnv {
        let clis = std::sync::Arc::new(crate::clis::CliManager::new(&data.join("cli")));
        let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = std::sync::Arc::new(iyakki_proxy::https::UreqStreamer::default());
        clis.set_streamer(streamer.clone());
        let k3s = crate::k3sdev::K3sDev::new(data, clis.clone(), std::sync::Arc::new(crate::wsexec::System));
        RealEnv { data: data.to_path_buf(), user: user.into(), k3s, clis, streamer, registry: RegistryArgs::default(), mac_source: crate::macfw::Source::Download }
    }
}

/// How a prerequisite stands from the installer jobs: ready on the machine, else its newest job (running: pending; succeeded: ready;
/// failed, stopped or lost: failed with the last line of its log), else (no job known) pending.
pub fn dep_state_from(jobs: &[Job], dep: Item, ready_now: bool) -> DepState {
    if ready_now {
        return DepState::Ready;
    }
    match jobs.iter().filter(|j| item_of(j) == Some(dep)).max_by_key(|j| j.started_ms) {
        Some(j) if j.running() => DepState::Pending,
        Some(j) if j.state == crate::jobs::State::Succeeded => DepState::Ready,
        Some(j) => DepState::Failed(if j.last_line.is_empty() { j.state.label() } else { j.last_line.clone() }),
        None => DepState::Pending,
    }
}

/// Link `kubectl` and `k3s` in `bin_dir` to the k3s program (one multi-call program). A name that is already there is left alone when
/// it is something else (a real kubectl from the CLI catalogue); a link of ours is renewed. Returns what was linked.
pub fn link_k3s_tools(bin_dir: &Path, k3s: &Path) -> Result<Vec<String>, String> {
    if !k3s.is_file() {
        return Err(format!("{} does not exist, so kubectl could not be linked to it", k3s.display()));
    }
    std::fs::create_dir_all(bin_dir).map_err(|e| format!("{}: {e}", bin_dir.display()))?;
    let mut done = Vec::new();
    for name in ["kubectl", "k3s"] {
        let link = bin_dir.join(name);
        match std::fs::symlink_metadata(&link) {
            Ok(m) if m.file_type().is_symlink() && std::fs::read_link(&link).is_ok_and(|t| t == k3s) => continue,
            Ok(m) if m.file_type().is_symlink() && std::fs::read_link(&link).is_ok_and(|t| t.ends_with("k3s-dev/bin/k3s")) => {
                std::fs::remove_file(&link).map_err(|e| format!("{}: {e}", link.display()))?;
            }
            Ok(_) => continue,
            Err(_) => {}
        }
        std::os::unix::fs::symlink(k3s, &link).map_err(|e| format!("{}: {e}", link.display()))?;
        done.push(name.to_string());
    }
    Ok(done)
}

/// The machine side of the GPU steps on this machine.
impl crate::gpu_steps::GpuEnv for RealEnv {
    fn root(&self) -> PathBuf {
        PathBuf::from("/")
    }
    fn data(&self) -> PathBuf {
        self.data.clone()
    }
    fn k3s_data(&self) -> PathBuf {
        self.k3s.layout().data()
    }
    fn dmesg(&mut self) -> String {
        let o = crate::wsexec::Runner::run(&crate::wsexec::System, Path::new("/"), &["dmesg".to_string()], &[], Duration::from_secs(10), 2 << 20, None);
        o.stdout
    }
    fn run(&mut self, argv: &[String], timeout: Duration) -> crate::wsexec::Out {
        crate::wsexec::Runner::run(&crate::wsexec::System, Path::new("/"), argv, &[], timeout, 256 * 1024, None)
    }
    fn published_checksum(&mut self) -> Option<String> {
        use std::io::Read;
        let head = crate::gpu_nvidia::open_run(&*self.streamer, &crate::gpu_nvidia::run_sha_url()).ok()?;
        let mut t = String::new();
        head.body.take(4096).read_to_string(&mut t).ok()?;
        crate::gpu_nvidia::published_sha(&t, &crate::gpu_nvidia::run_file())
    }
    fn download_run(&mut self, l: &crate::gpu_nvidia::Layout) -> Result<(), String> {
        let published = crate::gpu_steps::GpuEnv::published_checksum(self);
        match &published {
            Some(p) => println!("NVIDIA publishes the checksum {p}"),
            None => println!("note: NVIDIA's checksum file could not be fetched; the checksum pinned in IYAKKI still has to match"),
        }
        let mut last = u64::MAX;
        crate::gpu_nvidia::download_run(&*self.streamer, &l.run(), published.as_deref(), &mut |d, t| {
            let pct = if t > 0 { d * 100 / t } else { 0 };
            if pct != last {
                println!("downloading {} MB of {} MB ({pct}%)", d >> 20, t >> 20);
                last = pct;
            }
        })
    }
    fn kubectl(&mut self, args: &[&str]) -> crate::wsexec::Out {
        self.k3s.kubectl(args)
    }
    fn request_restart(&mut self) -> Result<(), String> {
        request_after(&self.data, "k3s-restart", &self.user)
    }
}

impl StepEnv for RealEnv {
    fn gpu(&mut self) -> Option<&mut dyn crate::gpu_steps::GpuEnv> {
        Some(self)
    }
    fn link_k3s_tools(&mut self) -> Result<String, String> {
        let l = link_k3s_tools(&self.clis.bin_dir(), &self.k3s.layout().bin())?;
        Ok(if l.is_empty() { String::new() } else { format!("linked {} in {} (terminals and agents can run kubectl; KUBECONFIG points at the dev cluster)", l.join(" and "), self.clis.bin_dir().display()) })
    }
    fn registry_on_other_cluster(&mut self) -> bool {
        self.registry.target.is_some()
    }
    fn linux_apps(&mut self) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        let la = crate::linuxapps::LinuxApps::new(&self.data, &self.user, &exe, std::sync::Arc::new(crate::wsexec::System));
        if la.get("debian").is_ok() {
            return Ok("the sandbox debian exists already".into());
        }
        let req = crate::linuxapps::CreateReq { name: "debian".into(), release: "debian-12".into(), network: true, mounts: vec![] };
        let s = la.create(&req, &*self.streamer, &|_, _| Ok(()), &mut |l| println!("{l}"))?;
        Ok(format!("sandbox {} ({}) is ready", s.name, s.title()))
    }
    fn linux_windows(&mut self) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        let la = crate::linuxapps::LinuxApps::new(&self.data, &self.user, &exe, std::sync::Arc::new(crate::wsexec::System));
        let streamer = self.streamer.clone();
        crate::linuxapps::stream(|sink| crate::linuxwin::install(&la, &*streamer, Some(sink), &mut |l| println!("{l}")))
    }
    fn mac_firmware(&mut self) -> Result<String, String> {
        let mut env = crate::macfw::RealEnv::new(&self.data, &self.user, self.clis.clone(), self.streamer.clone());
        let source = self.mac_source.clone();
        crate::macfw::run(&mut env, &self.data, &source).map(|o| o.headline)
    }
    fn create_registry(&mut self) -> Result<String, String> {
        let system: std::sync::Arc<dyn crate::wsexec::Runner> = std::sync::Arc::new(crate::wsexec::System);
        let (run, target): (std::sync::Arc<dyn crate::wsexec::Runner>, crate::localreg::Target) = match self.registry.target.clone() {
            Some(t) => (system, t),
            None => (
                std::sync::Arc::new(crate::localreg::K3sKubectl { inner: system, k3s: self.k3s.layout().bin() }),
                crate::localreg::Target { name: "iyakki-dev".into(), kubeconfig: self.k3s.layout().kubeconfig().display().to_string(), context: String::new(), node_host: "localhost".into() },
            ),
        };
        let mut o = crate::localreg::Opts::new(state_dir(&self.data).join("registry"));
        if self.registry.size_gi > 0 {
            o.size_gi = self.registry.size_gi;
        }
        o.node_port = self.registry.node_port;
        let info = crate::localreg::create(&run, &crate::ocipush::TcpHttp::default(), &target, &o, &mut |l| self.say(l))?;
        Ok(info.pull)
    }
    fn dep_state(&mut self, dep: Item) -> DepState {
        let ready = match dep {
            Item::K3s => self.k3s.installed().is_some() || !self.k3s.found_elsewhere().is_empty(),
            Item::Node => self.clis.is_installed("nodejs").is_some() || self.clis.node_check().is_ok_and(|m| m >= 22),
            Item::Python => self.clis.is_installed("python").is_some() || self.clis.python_check().is_ok(),
            Item::Lemonade => self.lemonade_ready(),
            other => marked(&self.data, other).is_some(),
        };
        let jobs = crate::jobs::Jobs::new(&self.data.join("jobs")).visible(&self.user, WORKSPACE);
        dep_state_from(&jobs, dep, ready)
    }
    fn say(&mut self, line: &str) {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    fn k3s_present(&mut self) -> bool {
        self.k3s.installed().is_some()
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        if self.k3s.installed().is_some() {
            return None;
        }
        self.k3s.found_elsewhere().into_iter().next()
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        let l = self.k3s.layout().clone();
        let plan = crate::k3sdev::lookup(&self.clis, &*self.streamer, &l, crate::k3sdev::Backend::Native, "in-cluster")?;
        self.say(&format!("version {} from {}", plan.version, plan.url));
        self.say(&format!("expected SHA-256 {}", plan.expected_sha));
        // the download reports how far it is; print it about every 5 %
        let (k3s, done) = (self.k3s.clone(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let watcher = {
            let (k3s, done) = (k3s.clone(), done.clone());
            std::thread::spawn(move || {
                let mut last = u64::MAX;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let p = k3s.progress();
                    if p.total > 0 {
                        let pct = p.done * 100 / p.total;
                        if last == u64::MAX || pct >= last + 5 {
                            println!("{} {pct}%", p.phase);
                            last = pct;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            })
        };
        let r = self.k3s.install(&plan, &*self.streamer, &self.user);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watcher.join();
        r.map(|_| format!("k3s {} at {}", plan.version, plan.install_path.display()))
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        request_start(&self.data, &self.user)
    }
    fn cluster_ready(&mut self) -> bool {
        self.k3s.kubectl(&["get", "nodes", "-o", "jsonpath={.items[*].status.conditions[?(@.type=='Ready')].status}"]).stdout.contains("True")
    }
    fn install_cli(&mut self, id: &str) -> Result<String, String> {
        if let Some(i) = self.clis.installed().into_iter().find(|i| i.id == id) {
            return Ok(i.version);
        }
        self.clis.install(id, &self.user)?;
        let mut last = String::new();
        loop {
            let Some(j) = self.clis.job(id) else { return Err("the install did not start".into()) };
            if let Some(e) = j.error {
                return Err(e);
            }
            if j.finished {
                break;
            }
            let line = if j.total > 0 { format!("{} {}%", j.phase, j.done * 100 / j.total) } else { j.phase.clone() };
            if line != last {
                self.say(&line);
                last = line;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(self.clis.installed().into_iter().find(|i| i.id == id).map(|i| i.version).unwrap_or_default())
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        let dir = state_dir(&self.data);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{name}.yaml"));
        std::fs::write(&file, manifest).map_err(|e| e.to_string())?;
        let out = self.k3s.kubectl(&["apply", "-f", &file.display().to_string()]);
        if out.code == Some(0) { Ok(out.stdout) } else { Err(format!("kubectl apply failed: {}", out.combined().trim())) }
    }
    fn rollout(&mut self, namespace: &str, name: &str, timeout: Duration) -> Result<(), String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let out = self.k3s.kubectl(&["-n", namespace, "rollout", "status", &format!("deployment/{name}"), "--timeout=15s"]);
            if out.code == Some(0) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("{name} did not become ready in {} minutes: {}", timeout.as_secs() / 60, out.combined().lines().last().unwrap_or("").trim()));
            }
            let pods = self.k3s.kubectl(&["-n", namespace, "get", "pods", "--no-headers"]);
            if let Some(l) = pods.stdout.lines().next() {
                self.say(&format!("  pod: {}", l.split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        }
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        mark_done(&self.data, item, detail)
    }
    fn request_after(&mut self, what: &str) -> Result<(), String> {
        request_after(&self.data, what, &self.user)
    }
    fn lemonade_ready(&mut self) -> bool {
        crate::lemonade::local_ports().iter().any(|p| crate::lemonade::probe_local(*p).is_some())
    }
    fn lemonade_models(&mut self) -> Result<Vec<String>, String> {
        crate::lemonade::probe_local_models().map(|m| m.into_iter().filter(|m| m.downloaded).map(|m| m.id).collect()).ok_or_else(|| "Lemonade does not answer".to_string())
    }
    fn lemonade_delete(&mut self, model: &str) -> Result<(), String> {
        crate::voicepack::delete_local_model(model)
    }
    fn voice_selftest(&mut self) -> Result<String, String> {
        let base = crate::lemonade::local_ports().iter().find_map(|p| crate::lemonade::probe_local(*p)).map(|b| b.0).ok_or("Self-test failed: Lemonade does not answer")?;
        crate::voicepack::selftest(&base, std::sync::Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300))))
    }
    fn record_pulled(&mut self, models: &[String]) -> Result<(), String> {
        crate::voicepack::record_pulled(&self.data, models)
    }
    fn lemonade_pull(&mut self, model: &str) -> Result<(), String> {
        let base = crate::lemonade::local_ports().iter().find_map(|p| crate::lemonade::probe_local(*p)).map(|b| b.0).ok_or("Lemonade does not answer")?;
        let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(3600));
        let body = serde_json::json!({ "model_name": model }).to_string();
        let r = iyakki_proxy::Upstream::request(&up, "POST", &format!("{base}/pull"), &[("Content-Type".into(), "application/json".into())], body.as_bytes())?;
        if r.status / 100 == 2 { Ok(()) } else { Err(format!("Lemonade answered HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(200).collect::<String>())) }
    }
}

/// The registry step's flags: `--kubeconfig FILE [--context NAME] [--node-host HOST] [--cluster-name NAME]` for a cluster other than the
/// managed k3s, `--size GI` (the volume), `--port NODEPORT`.
pub fn registry_args(args: &[String]) -> Result<RegistryArgs, String> {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let num = |name: &str| -> Result<u32, String> { flag(name).map(|v| v.parse::<u32>().map_err(|_| format!("{name} wants a number, not `{v}`"))).transpose().map(|v| v.unwrap_or(0)) };
    let target = flag("--kubeconfig").map(|k| crate::localreg::Target { name: flag("--cluster-name").unwrap_or_default(), kubeconfig: k, context: flag("--context").unwrap_or_default(), node_host: flag("--node-host").unwrap_or_else(|| "localhost".into()) });
    let port = num("--port")?;
    if port > u16::MAX as u32 {
        return Err(format!("--port {port} is not a port"));
    }
    Ok(RegistryArgs { target, size_gi: num("--size")?, node_port: port as u16 })
}

/// The job that creates the registry in a chosen cluster (not the managed k3s): `iyakki quick-install registry --kubeconfig ...`.
/// The managed cluster goes through [`start`] with [`Item::Registry`] instead, which installs k3s first when it is missing.
pub fn registry_spec(l: &Launch, a: &RegistryArgs) -> Spec {
    let mut argv: Vec<String> = vec![l.exe.display().to_string(), "quick-install".into(), Item::Registry.id().into(), "--data".into(), l.data.display().to_string()];
    if let Some(t) = &a.target {
        argv.extend(["--kubeconfig".into(), t.kubeconfig.clone()]);
        if !t.context.is_empty() {
            argv.extend(["--context".into(), t.context.clone()]);
        }
        argv.extend(["--node-host".into(), t.node_host.clone()]);
        if !t.name.is_empty() {
            argv.extend(["--cluster-name".into(), t.name.clone()]);
        }
    }
    if a.size_gi > 0 {
        argv.extend(["--size".into(), a.size_gi.to_string()]);
    }
    if a.node_port > 0 {
        argv.extend(["--port".into(), a.node_port.to_string()]);
    }
    Spec { label: label(Item::Registry, 1, 1), cmd: Cmd::Argv(argv), cwd: l.data.clone(), workspace: WORKSPACE.into() }
}

/// Start the registry job for `a` (the managed cluster when no target is given: k3s is installed first when missing, as for any
/// item). Refuses when a registry job is running already.
pub fn start_registry(jobs: &Jobs, owner: &str, a: &RegistryArgs, k3s_present: bool, l: &Launch) -> Result<Vec<Job>, String> {
    if jobs.visible(owner, WORKSPACE).iter().any(|j| j.running() && item_of(j) == Some(Item::Registry)) {
        return Err("The registry is being created already: its progress is beside the clock in the top bar".into());
    }
    if a.target.is_none() {
        // the managed cluster: the usual order (k3s first when missing); size and port ride on the registry job
        let steps = order(Item::Registry, &|i| i == Item::K3s && k3s_present);
        std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
        let total = steps.len();
        let mut started = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            let mut spec = if *s == Item::Registry { registry_spec(l, a) } else { spec_with("Install", *s, i + 1, total, l) };
            spec.label = label_with("Install", *s, i + 1, total);
            match jobs.start(owner, &spec, &|_: &Path| Ok(())) {
                Ok(j) => started.push(j),
                Err(e) => return if started.is_empty() { Err(e) } else { Err(format!("{} started, {} did not: {e}", started.len(), s.title())) },
            }
        }
        return Ok(started);
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &registry_spec(l, a), &|_: &Path| Ok(())).map(|j| vec![j])
}

/// Forget that the registry step finished (after the registry was removed).
pub fn unmark(data: &Path, item: Item) {
    let _ = std::fs::remove_file(marker(data, item));
}

/// `iyakki quick-install <k3s|duckdb|elasticsearch> [--data DIR] [--user NAME]`: one step, as the job runs it. Returns the exit code.
pub fn cli_main(args: &[String]) -> i32 {
    // the GPU page's test prompt: not an installable item
    if args.first().is_some_and(|a| a == "gpu-test") {
        return gpu_test_main(args);
    }
    // the camera firmware: not an installable item either
    if args.first().is_some_and(|a| a == CAMERA_JOB) {
        return camera_firmware_main(args);
    }
    let Some(item) = args.first().and_then(|a| Item::parse(a)) else {
        eprintln!("usage: iyakki quick-install <k3s|duckdb|elasticsearch> [--data DIR] [--user NAME]");
        return 2;
    };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let user = flag("--user").or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "system".into());
    if item.fw_def().is_some() {
        return firmware_pack_main(args, item, &data, &user);
    }
    let mut env = RealEnv::new(&data, &user);
    if item == Item::Registry {
        env.registry = match registry_args(args) {
            Ok(a) => a,
            Err(e) => {
                println!("error: {e}");
                return 2;
            }
        };
    }
    if item == Item::MacFirmware {
        env.mac_source = match crate::macfw::Source::from_args(args) {
            Ok(s) => s,
            Err(e) => {
                println!("error: {e}");
                return 2;
            }
        };
    }
    match run_step(item, &mut env, WAIT_FOR_CLUSTER, Duration::from_secs(5)) {
        Ok(()) => 0,
        Err(e) => {
            println!("error: {e}");
            1
        }
    }
}

/// `iyakki quick-install fw-<name> [--data DIR] [--from FOLDER] [--accept-licence] [--force-hardware] [--user NAME]`: a restricted firmware
/// pack, its log is what the person reads. `--accept-licence` prints the vendor's licence note and records the acceptance for `--user`
/// (the Setup tile does the same with a click, and writes the audit log line); without an acceptance the job installs nothing.
fn firmware_pack_main(args: &[String], item: Item, data: &Path, user: &str) -> i32 {
    let Some(def) = item.fw_def() else { return 2 };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--accept-licence") {
        println!("licence of {}: {}", def.title, def.licence_note);
        if let Err(e) = crate::fwpack::accept(data, def, &format!("cli:{user}"), crate::clock::now_unix()) {
            println!("error: {e}");
            return 1;
        }
        println!("accepted by cli:{user} (recorded in {})", crate::fwpack::work_dir(data, def).display());
    }
    let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = std::sync::Arc::new(iyakki_proxy::https::UreqStreamer::default());
    let mut env = crate::fwpack::RealEnv::new(data, streamer);
    let force = args.iter().any(|a| a == "--force-hardware");
    let r = match flag("--from") {
        Some(f) => crate::fwpack::run_folder(&mut env, data, def, force, Path::new(&f)),
        None => crate::fwpack::run(&mut env, data, def, force),
    };
    match r {
        Ok(_) => 0,
        Err(e) => {
            println!("error: {e}");
            1
        }
    }
}

/// `iyakki quick-install mac-camera [--data DIR]`: the camera firmware job, its log is what the person reads.
fn camera_firmware_main(args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = std::sync::Arc::new(iyakki_proxy::https::UreqStreamer::default());
    let mut env = crate::camfw::RealEnv::new(&data, streamer);
    match crate::camfw::run(&mut env, &data) {
        Ok(_) => 0,
        Err(e) => {
            println!("error: {e}");
            1
        }
    }
}

/// `iyakki quick-install gpu-test [--data DIR]`: the test prompt of the GPU page, as a job (its log is the result).
fn gpu_test_main(args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let bases: Vec<String> = crate::gpu_steps::test_endpoints(&crate::lemonade::local_ports().iter().filter_map(|p| crate::lemonade::probe_local(*p).map(|b| b.0)).collect::<Vec<_>>());
    let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(600));
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    match crate::gpu_steps::run_test(&data, &bases, &up, &now, &mut |l| println!("{l}")) {
        Ok(_) => 0,
        Err(e) => {
            println!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
#[path = "quickinstall_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "quickinstall_registry_tests.rs"]
mod registry_tests;
