//! Types moved out of `iyakki_agent::quickinstall` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::Path;
use std::path::PathBuf;

use crate::jobs::Job;

/// The jobs workspace of installer jobs: the top bar's indicator shows the jobs of this workspace.
pub const WORKSPACE: &str = "iyakki-install";

/// Where an item stands on this machine.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            Presence::Earlier(d) if d.starts_with(LAST_FAILED) => format!("installed earlier; {d}"),
            Presence::Earlier(d) if d.is_empty() => "installed earlier, not running".into(),
            Presence::Earlier(d) => format!("installed earlier, not running · {d}"),
            Presence::Elsewhere(d) => format!("present (not IYAKKI's) · {d}"),
        }
    }
}

/// The words that start a presence detail when the newest attempt failed after an earlier install.
pub const LAST_FAILED: &str = "the last attempt failed: ";

/// How a step's job is started: the program (this one, `iyakki`; a script in tests), the data folder it works on.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

pub mod opensearch {
    pub const ABOUT: &str = "A search engine you can use freely (OpenSearch, Apache-2.0): one single-node pod in the cluster (namespace iyakki-data, ClusterIP only, port 9200, 5 GB volume, about 1.5 GB of memory), no sign-in because nothing outside the cluster can reach it. DASH gets it as the source \"Cluster OpenSearch\", through its Elasticsearch connector (the calls are the same)";
    pub const LICENCE: &str = "OpenSearch is Apache-2.0 licensed (checked in the LICENSE.txt of the pinned release 3.9.0). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";
}

pub mod clickhouse {
    pub const ABOUT: &str = "An analytics database for DASH and the assistant (ClickHouse): one pod in the cluster (namespace iyakki-data, ClusterIP only, HTTP 8123 and native 9000, 5 GB volume, about 1 GB of memory), with a password generated for you and kept in your vault. It is added to DASH as the source \"Cluster ClickHouse\"; a ClickHouse source you added by hand at localhost then offers \"Use cluster address\"";
    pub const LICENCE: &str = "ClickHouse is Apache-2.0 licensed. It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";
}

pub mod objstore {
    /// Where the object stores go.
    pub const NS: &str = "iyakki-objects";
    pub const SEAWEEDFS_ABOUT: &str = "S3-compatible object storage you can rely on (SeaweedFS, mature): one pod in the cluster (namespace iyakki-objects, ClusterIP only, S3 port 8333, 20 GB volume, about 1 GB of memory), with an access key and a secret key generated for you and kept in your vault, and a bucket named iyakki made on the first start. Pick this one, or RustFS, or both";
    pub const RUSTFS_ABOUT: &str = "S3-compatible object storage written in Rust (RustFS, young: its releases are still alpha, so use it for trying and for data you can lose): one pod in the cluster (namespace iyakki-objects, ClusterIP only, S3 port 9000, 20 GB volume, about 1 GB of memory), with an access key and a secret key generated for you and kept in your vault. The first client makes the bucket. Pick this one, or SeaweedFS, or both";
    pub const SEAWEEDFS_LICENCE: &str = "SeaweedFS is Apache-2.0 licensed. It runs as its own pod and IYAKKI only talks to it over the S3 protocol; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";
    pub const RUSTFS_LICENCE: &str = "RustFS is Apache-2.0 licensed (the name RustFS is its owner's trademark). It runs as its own pod and IYAKKI only talks to it over the S3 protocol; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it";
}

/// The removal plan of an item (`iyakki_agent::quickinstall::uninstall` makes it from the machine).
pub use crate::quickinstall_uninstall as uninstall;

/// What each cluster item may use (CPU and memory requests and limits, the idle profile) and the review's memory warning.
#[path = "quickinstall_resources.rs"]
pub mod resources;

/// Ollama as a pod of the cluster: what the Setup page needs of it without the agent (the rest is `iyakki_agent::quickinstall::ollama_cluster`).
pub use crate::quickinstall_ollama as ollama_cluster;

/// What the IDE offers about the Local Gitea of Quick install (constants of `iyakki_agent::quickinstall::gitea`, F-SPLIT-03).
pub mod gitea {
    pub const NAMESPACE: &str = "iyakki-gitea";
    pub const OFFER_LABEL: &str = "Put this project under version control on Local Gitea";
    /// The name of the Git connection the install adds (`iyakki_agent::quickinstall::gitea::CONNECTION_NAME` is this).
    pub const CONNECTION_NAME: &str = "Local Gitea";
    pub const NOT_INSTALLED: &str = "Gitea is not installed yet. Open Store \u{2192} Setup, install \"Gitea (versioning for your work)\", then press this again.";

    /// What the AI IDE's Git panel shows for a project.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Offer {
        /// The project has a remote already: nothing is offered.
        None,
        /// No remote and Gitea is not installed (no connection yet): the plain message that names Store \u{2192} Setup.
        Install(String),
        /// No remote and the connection exists: its id (the flow starts on it).
        Ready(String),
    }

    pub fn offer(has_remote: bool, conns: &[crate::gitconn::Connection]) -> Offer {
        use crate::gitconn::Kind;
        if has_remote {
            return Offer::None;
        }
        match conns.iter().find(|c| !c.cluster.is_empty() && c.kind == Kind::Gitea && c.name.starts_with(CONNECTION_NAME)) {
            Some(c) => Offer::Ready(c.id.clone()),
            None => Offer::Install(NOT_INSTALLED.to_string()),
        }
    }
}

/// Where the Matrix homeserver of Quick install is reached (constant of `iyakki_agent::quickinstall::matrix`).
pub mod matrix {
    pub const NS: &str = "iyakki-matrix";
    pub const ABOUT: &str = "A chat server of your own (Tuwunel, a Matrix homeserver): one pod in the cluster (namespace iyakki-matrix, ClusterIP only, 5 GB volume, about 512 MB of memory). Registration is closed to anyone without a token that is generated for you and kept in your vault, and the server talks to no other server (federation off). Register an account with any Matrix client, then Settings, Notifications, Matrix account connects IYAKKI to it so flows and the assistant can message you";
    pub const LICENCE: &str = "Tuwunel is Apache-2.0 licensed (checked in the LICENSE file of the official repository matrix-construct/tuwunel and in the image label org.opencontainers.image.licenses of v1.9.4). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from ghcr.io when you click Install, IYAKKI does not ship it";
    pub const HOMESERVER: &str = "cluster://iyakki-matrix/matrix:8008";
}

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
            Group::Cluster => "k3s is the base; DuckDB, Elasticsearch, Lemonade and the local image registry install it first when it is missing; object storage: pick one or both (SeaweedFS is the dependable one, RustFS the Rust-native and young one)",
            Group::AiCoding => "coding agents for the terminal and the AI IDE (Node.js is installed first when it is missing); pick one or both",
            Group::Voice => "reading voices; pick one or both (Kokoro: English, through Lemonade, which it installs first); or Voice: Whisper tiny + Kokoro, which also adds listening and sets both up",
            Group::LocalModels => "model servers on this machine, added to Models when they answer; pick one or both (both run in k3s; an Ollama installed earlier on the host can be moved in); or use the GPU of a Mac on your network",
            Group::Media => "the player for video and audio files (MEDIA tile, IYAKKI menu \u{2192} Media player); it needs nothing else",
            Group::Linux => "Debian and Ubuntu programs (apt) in a sandbox of their own, with a network switch and only the folders you mount; downloaded from the distribution, never shipped (Store \u{2192} Setup \u{2192} Linux apps has the page)",
            Group::Mac => "T2 Macs only: the Wi-Fi and Bluetooth firmware is Apple's, so it is never shipped; it is read from your own Mac or from Apple's recovery download, when you ask (IYAKKI menu \u{2192} Mac drivers has more ways)",
            Group::Firmware => "firmware that IYAKKI is not sure it may redistribute is not in the images: shown only when your hardware needs it; you read the vendor's licence, accept it, and it is downloaded from the upstream linux-firmware repository (or read from a folder or stick), checked and installed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Item {
    Node,
    Python,
    K3s,
    DuckDb,
    Elasticsearch,
    /// OpenSearch (Apache-2.0) in the managed cluster: the open-source search engine next to Elasticsearch; one unprivileged pod,
    /// ClusterIP only; DASH gets it through its Elasticsearch connector (the `opensearch` submodule).
    OpenSearch,
    /// Prometheus (metrics) in the managed cluster: one Deployment with read-only RBAC, ClusterIP only; DASH gets a source for it.
    Prometheus,
    /// A graph database for the AI (Neo4j Community, GPL-3.0) in the managed cluster: one unprivileged pod, ClusterIP only, a generated
    /// password kept in a Secret and in the vault; DASH gets a Cypher source for it, read-only.
    GraphDb,
    /// ClickHouse (Apache-2.0) in the managed cluster for DASH: one pod, ClusterIP only, a generated password kept in a Secret and in
    /// the vault; DASH gets the shared source `clickhouse-cluster` (the `clickhouse` submodule).
    ClickHouse,
    /// Gitea (MIT) in the managed cluster to keep your work versioned: one unprivileged rootless pod, SQLite on a volume, published on
    /// this machine's loopback only. Reuses the Platform services' Gitea objects; the administrator password and an API token go to the
    /// person's vault and a Git connection "Local Gitea" is added (the `gitea` submodule).
    Gitea,
    /// Tuwunel (Apache-2.0), a small Matrix homeserver, in the managed cluster for chat of your own: its own namespace, ClusterIP only,
    /// registration closed without a generated token that goes to the vault, federation off (the `matrix` submodule).
    Matrix,
    /// SeaweedFS (Apache-2.0): S3-compatible object storage in the managed cluster, the dependable choice; ClusterIP only, keys generated into the
    /// person's vault (the `objstore` submodule).
    SeaweedFs,
    /// RustFS (Apache-2.0): the Rust-native S3-compatible object storage, young (alpha releases); same rules as SeaweedFS.
    RustFs,
    Registry,
    ClaudeCode,
    Copilot,
    Kokoro,
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
    /// Stage 2 for AMD: ROCm compute in pods (`amd.com/gpu`), AMD's device plugin and a `rocminfo` smoke pod; fails closed until the owner pins the images (gpu_rocm.rs).
    GpuRocm,
    Ffmpeg,
    /// The audio and video add-on of the browser (H.264, AAC, MP3): FFmpeg's shared libraries, LGPL-only, built for the image's musl, put in
    /// `<data>/addons/ffmpeg` where `avaddon.rs` looks. Installed by the CLI catalogue entry `ffmpeg-libs` (clis.rs) at the person's click.
    FfmpegLibs,
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
    /// MediaTek Wi-Fi and Bluetooth firmware (the pack `fw-mediatek`, same rules as the others): shown only where a MediaTek card is.
    FwMediatek,
    /// "Hardware firmware" (fwextra.rs): the vendor firmware files this computer asks for and lacks, from the pinned linux-firmware release,
    /// at the person's click ([`start_hardware_firmware`]). The images carry none of them. Shown on every computer.
    FwHardware,
    /// Python with scikit-learn, NumPy, SciPy and OpenCV in the Linux sandbox's no-network runner, downloaded at the click from the vendors'
    /// pinned files ([`crate::datasci`]); fails closed until the owner fills the pin table (F-FLOW-31).
    DataScience,
    /// The full web engine of the browser as an optional package (F-BROW-40): fetched from the store the person pinned, checked against its
    /// key, size and checksum, kept in the data folder (`browserpkg.rs`). The card of the install is read first; removing it leaves the image's own engine.
    BrowserPackage,
    /// The video-site extractor (yt-dlp, Unlicense), a program fetched from its vendor at the click and checked against the SHA-256 in `legal/downloads.txt`
    /// (F-MEDIA-12); nothing is shipped. Installed as `<data>/tools/yt-dlp`.
    YtDlp,
}

impl Item {
    pub const ALL: [Item; 39] = [Item::Node, Item::Python, Item::K3s, Item::DuckDb, Item::Elasticsearch, Item::OpenSearch, Item::Prometheus, Item::ClickHouse, Item::GraphDb, Item::Gitea, Item::Matrix, Item::SeaweedFs, Item::RustFs, Item::Registry, Item::ClaudeCode, Item::Copilot, Item::Kokoro, Item::VoicePack, Item::Lemonade, Item::Ollama, Item::MacHost, Item::GpuVulkan, Item::GpuNvidia, Item::GpuRocm, Item::Ffmpeg, Item::FfmpegLibs, Item::MacFirmware, Item::MacCamera, Item::LinuxApps, Item::LinuxWindows, Item::FwQualcommBt, Item::FwRealtekNet, Item::FwBroadcomNet, Item::FwAtheros9170, Item::FwMediatek, Item::FwHardware, Item::DataScience, Item::YtDlp, Item::BrowserPackage];

    pub fn id(self) -> &'static str {
        match self {
            Item::Node => "nodejs",
            Item::Python => "python",
            Item::K3s => "k3s",
            Item::DuckDb => "duckdb",
            Item::Elasticsearch => "elasticsearch",
            Item::OpenSearch => "opensearch",
            Item::Prometheus => "prometheus",
            Item::GraphDb => "graph-db",
            Item::ClickHouse => "clickhouse",
            Item::Gitea => "gitea",
            Item::Matrix => "matrix",
            Item::SeaweedFs => "seaweedfs",
            Item::RustFs => "rustfs",
            Item::Registry => "registry",
            Item::ClaudeCode => "claude-code",
            Item::Copilot => "github-copilot",
            Item::Kokoro => "kokoro",
            Item::VoicePack => "voice-pack",
            Item::Lemonade => "lemonade",
            Item::Ollama => "ollama",
            Item::MacHost => "mac-host",
            Item::GpuVulkan => "gpu-vulkan",
            Item::GpuNvidia => "gpu-nvidia",
            Item::GpuRocm => "gpu-rocm",
            Item::Ffmpeg => "ffmpeg",
            Item::FfmpegLibs => crate::avaddon::INSTALL_ITEM,
            Item::MacFirmware => "mac-firmware",
            Item::MacCamera => CAMERA_JOB,
            Item::LinuxApps => "linux-apps",
            Item::LinuxWindows => "linux-windows",
            Item::FwQualcommBt => "fw-qualcomm-bt",
            Item::FwRealtekNet => "fw-realtek-net",
            Item::FwBroadcomNet => "fw-broadcom-net",
            Item::FwAtheros9170 => "fw-atheros-carl9170",
            Item::FwMediatek => "fw-mediatek",
            Item::FwHardware => crate::fwextra::PACK_ID,
            Item::DataScience => "datasci",
            Item::BrowserPackage => "browser-package",
            Item::YtDlp => "ytdlp",
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
            Item::OpenSearch => "OpenSearch (search engine, open source)",
            Item::Prometheus => "Prometheus (metrics)",
            Item::GraphDb => "Graph database (for AI)",
            Item::ClickHouse => "ClickHouse (analytics database)",
            Item::Gitea => "Gitea (versioning for your work)",
            Item::Matrix => "Matrix server (chat of your own)",
            Item::SeaweedFs => "SeaweedFS (object storage, S3)",
            Item::RustFs => "RustFS (object storage, S3, Rust, young)",
            Item::Registry => "Local image registry",
            Item::ClaudeCode => "Claude Code",
            Item::Copilot => "GitHub Copilot CLI",
            Item::Kokoro => "Kokoro",
            Item::VoicePack => "Voice: Whisper tiny + Kokoro",
            Item::Lemonade => "Lemonade",
            Item::Ollama => "Ollama",
            Item::MacHost => "Use a Mac's GPU (Apple Silicon)",
            Item::GpuVulkan => "GPU for local models (Vulkan)",
            Item::GpuNvidia => "NVIDIA GPU driver (CUDA)",
            Item::GpuRocm => "AMD GPU compute (ROCm)",
            Item::Ffmpeg => "Media player (ffmpeg)",
            Item::FfmpegLibs => "Browser audio and video (H.264, AAC)",
            Item::MacFirmware => "Mac Wi-Fi and Bluetooth firmware (from your Mac)",
            Item::MacCamera => "MacBook FaceTime camera: driver and firmware (firmware from Apple)",
            Item::LinuxApps => "Linux apps (Debian/Ubuntu)",
            Item::LinuxWindows => "Linux windows (graphical programs)",
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => self.fw_def().map_or("Firmware", |d| d.title),
            Item::FwHardware => "Hardware firmware (Wi-Fi, Bluetooth, graphics, ...)",
            Item::DataScience => "Data science toolkit: scikit-learn, NumPy, SciPy, OpenCV",
            Item::BrowserPackage => "Full web engine for the browser (optional package)",
            Item::YtDlp => "Video-site downloader (yt-dlp)",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Item::Node | Item::Python => Group::Runtimes,
            Item::K3s | Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Registry => Group::Cluster,
            Item::ClaudeCode | Item::Copilot => Group::AiCoding,
            Item::Kokoro | Item::VoicePack => Group::Voice,
            Item::Lemonade | Item::Ollama | Item::MacHost | Item::GpuVulkan | Item::GpuNvidia | Item::GpuRocm => Group::LocalModels,
            Item::Ffmpeg | Item::FfmpegLibs | Item::BrowserPackage | Item::YtDlp => Group::Media,
            Item::MacFirmware | Item::MacCamera => Group::Mac,
            Item::LinuxApps | Item::LinuxWindows | Item::DataScience => Group::Linux,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek | Item::FwHardware => Group::Firmware,
        }
    }

    /// One line for the menu.
    pub fn about(self) -> &'static str {
        match self {
            Item::DataScience => crate::datasci::ABOUT,
            Item::BrowserPackage => "The engine that runs pages the built-in reader cannot (a full web engine as a separate program). Not in the base image: fetched at your click from the store you pinned (a store source with its key in Store, Settings), checked against that key, its size and checksum, run in a small private environment, and allowed the network and sound only. The card shows the version and size first; Remove takes it away and the image's own engine, if any, stays",
            Item::YtDlp => "Saves one video from a page address you give (https only) into your video folder and the media index. Downloads the yt-dlp program from its vendor (github.com/yt-dlp/yt-dlp), checked with the checksum IYAKKI pins; about 3 MB. It runs only when you ask, with no plug-ins and no command execution",
            Item::Node => "JavaScript runtime with npm (node, npm, npx): from nodejs.org, checksum-verified, kept in the data folder (no sudo)",
            Item::Python => "Python 3 with venv and pip (python3), installed through uv into the data folder (no sudo)",
            Item::K3s => "Lightweight Kubernetes on this machine: the managed dev cluster, inside the data partition, checksum-verified",
            Item::DuckDb => "SQL on CSV and Parquet files: the duckdb command (DASH uses it), set up together with the cluster",
            Item::Elasticsearch => "Search engine: one single-node Deployment in the cluster (namespace iyakki-data, port 9200 inside the cluster). Elasticsearch is source-available, not open source: the cluster pulls the image from Elastic when you click Install, IYAKKI does not ship it. OpenSearch (Apache-2.0, in this list) is the open-source choice",
            Item::OpenSearch => opensearch::ABOUT,
            Item::Prometheus => "Metrics for the cluster: one Prometheus pod (namespace iyakki-monitoring, ClusterIP only, 7 days kept, 5 GB volume) that scrapes the nodes and the pods that ask for it, with read-only access to the cluster. It is added to DASH as a metrics source",
            Item::ClickHouse => clickhouse::ABOUT,
            Item::Matrix => matrix::ABOUT,
            Item::SeaweedFs => objstore::SEAWEEDFS_ABOUT,
            Item::RustFs => objstore::RUSTFS_ABOUT,
            Item::Gitea => "Keeps your projects versioned on this machine: a Gitea git service (rootless, one pod in the cluster, namespace iyakki-gitea, 10 GB volume, about 300 MB of memory) that this machine reaches at http://127.0.0.1:3300 (no other computer can). A first administrator with a generated password and an API token are kept in your vault, and a Git connection called \"Local Gitea\" is added, so the GIT tile and the AI IDE (Git, Connect Git, Put this project under version control) use it at once",
            Item::GraphDb => "A graph database the AI can query (Neo4j Community): one pod in the cluster (namespace iyakki-data, ClusterIP only, 5 GB volume, about 500 MB of memory), with a password generated for you and kept in your vault. It is added to DASH as a read-only Cypher source, so the assistant can ask it for relations between things instead of reading everything",
            Item::Registry => "Where images built on this device go (the private store's server): one registry:2 pod with a volume in the cluster, created in one step, no cluster restart. It has no sign-in, so it listens on this machine only (127.0.0.1:30500): no other computer can reach it",
            Item::ClaudeCode => "Anthropic's coding agent (the claude command); the AI IDE can use it once you sign in (Settings \u{2192} Agents). Proprietary software: downloaded from Anthropic at your request, never shipped by IYAKKI",
            Item::Copilot => "GitHub's coding agent (the copilot command); needs a Copilot subscription and `copilot login`. Proprietary software: downloaded from GitHub at your request, never shipped by IYAKKI",
            Item::Kokoro => "English neural reading voice (kokoro-v1, 82M), pulled into Lemonade; Lemonade (and k3s) are installed first when missing",
            Item::VoicePack => "Listen and speak with one action: Whisper tiny (speech to text, any language it knows) and Kokoro (reading voice, English only), pulled into the Lemonade of this machine (Lemonade and k3s are installed first when missing), set as the engines of Settings \u{2192} Voice and tested. Review first: sizes, licences and what runs where. Remove puts the settings back",
            Item::Lemonade => "Local model server (chat, embeddings, images, whisper, Kokoro): one Deployment in k3s (namespace iyakki-ai), reachable on this machine only (127.0.0.1:13305) and added to Models",
            Item::Ollama => ollama_cluster::ABOUT,
            Item::MacHost => "A guide, not an install: use the GPU of a Mac with Apple Silicon (Metal) or of another computer on your network. Shows what to install ON that computer (you do it), tests the connection with tokens per second, adds it to Models and can publish it to the cluster",
            Item::GpuVulkan => "Let pods in k3s use this machine's GPU through Vulkan (AMD, Intel, NVIDIA via nouveau): deploys a small device plugin, tests it with an unprivileged pod and gives Lemonade the GPU. Nothing is downloaded by IYAKKI; the cluster pulls the plugin and a llama.cpp image (about 300 MB)",
            Item::GpuRocm => "AMD's ROCm for pods (shown when a supported AMD card with the compute interface is found; older Polaris cards stay on Vulkan): deploys AMD's device plugin so pods can ask for amd.com/gpu and tests it with rocminfo. The images are pulled by your cluster and must be pinned by digest first; until then this entry stops with a plain message and changes nothing",
            Item::GpuNvidia => "NVIDIA's own driver for CUDA in pods (shown when an NVIDIA GPU is found; Turing or newer): downloads NVIDIA's user-space libraries and GSP firmware for you from download.nvidia.com (about 400 MB, checksum-verified, never part of an image), loads the open kernel modules of this image, deploys NVIDIA's device plugin so pods can ask for nvidia.com/gpu, and tests it with nvidia-smi",
            Item::MacFirmware => "Downloads Apple's recovery image for this Mac model from Apple (about 1 GB, resumable; for a 2017 MacBook Pro it is the macOS 11 image, the one that keeps the Bluetooth patch), reads the Wi-Fi and Bluetooth firmware out of it with 7-Zip, installs it as a driver pack and loads the drivers. A downloaded image is kept until the files are installed: Retry reads it again and never downloads it twice. The files are Apple's: they stay on this machine, are never uploaded and are not part of IYAKKI. IYAKKI menu \u{2192} Mac drivers also reads them from a folder, a stick or a disk image",
            Item::MacCamera => "MacBook FaceTime HD camera (Broadcom 14e4:1570): downloads Apple's camera firmware from Apple's update server when you click (about 17.6 MB, only the needed parts, resumable, checked), builds the firmware and sensor files, installs them as a driver pack and loads the camera driver module that is already in the Mac image (no compiler and no restart needed). Apple's files stay on this machine, are never uploaded and are not part of IYAKKI",
            Item::LinuxWindows => "Show a graphical Linux program in an IYAKKI window (LINUX WINDOW tile): makes a Debian 12 sandbox named gui and installs, with apt, TigerVNC's X server (it draws into memory), a one-window manager, fonts, software OpenGL and the xterm test program (about 250 MB, never part of an image). No graphics card and no sound in the window",
            Item::LinuxApps => "Run Debian or Ubuntu programs: downloads the official Debian 12 base for you (about 30 MB, checksum-verified, never part of an image) into a sandbox named debian with the network on and no folders mounted; apt installs packages inside it. The Store \u{2192} Setup \u{2192} Linux apps (Open its page) adds Ubuntu, more sandboxes, mounted folders, image apps and the terminal",
            Item::Ffmpeg => "Plays video and audio in IYAKKI (MEDIA tile, camera recordings, Media player). Downloads a static ffmpeg build from its builder (johnvansickle.com), GPLv3, checked with its checksum; about 42 MB",
            Item::FfmpegLibs => "Lets the BROWSER tile play H.264 video and AAC sound (TikTok and similar pages). Built here from FFmpeg's LGPL sources, or supplied by you (a file in /local/packages, a mirror inside your organisation, or an ffmpeg you installed that passes the checks); IYAKKI neither ships nor hosts them; with nothing supplied, Install downloads the vendors' sources and builds them on this machine in an isolated pod (k3s must be running). You read the licences first; the browser engine uses them at its next start. Nothing of FFmpeg is in the image",
            Item::FwHardware => "The vendor firmware this computer asks for and does not have (Wi-Fi, Bluetooth, graphics, network cards): IYAKKI images do not carry it. The card lists exactly which files your hardware needs and what each vendor's licence says, then downloads only those from the upstream linux-firmware release (kernel.org, a fixed release, every file checked), installs them as a driver pack that survives restarts, and reloads the drivers where that is safe; Restart appears only when needed. A Wi-Fi chip without its firmware needs a wired or USB-C connection, a USB stick or another computer to get it.",
            Item::FwQualcommBt => "Bluetooth patch files for newer Qualcomm laptop chips (QCA6390, WCN3950, WCN3988, WCN7850, QCA6698): 20 files, about 1 MB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwRealtekNet => "Firmware for PCIe Realtek Ethernet chips (RTL8111/8168, RTL8125 and relatives): 30 small files, about 140 KB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwBroadcomNet => "Firmware for Broadcom wired network chips (Tigon3 tg3, NetXtreme II bnx2, NetXtreme II 10 Gigabit bnx2x): 21 files, about 3 MB. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwAtheros9170 => "Firmware for old Atheros AR9170 USB Wi-Fi sticks (carl9170 driver): one file, 13 KB. GPL-2.0 or later; the source is the carl9170fw project. Not in the images (IYAKKI is not sure it may redistribute them): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
            Item::FwMediatek => "Firmware for MediaTek Wi-Fi and Bluetooth cards (MT7921, MT7922, MT7925 and the older MT76xx, common in recent laptops): 28 files, about 9 MB. Not in the images (MediaTek's licence is not an open-source licence): you read and accept the vendor's licence, then the files are downloaded from the upstream linux-firmware repository at a fixed release, each checked for size and SHA-256, installed as a driver pack that survives restarts, and the Wi-Fi driver is loaded again; Reboot to finish appears only when the driver needs it. Also installable from a folder or USB stick.",
        }
    }

    /// The licences, as the review shows them before Install (program, and model weights where there are any).
    pub fn licence(self) -> &'static str {
        match self {
            Item::DataScience => crate::datasci::LICENCE,
            Item::BrowserPackage => "as the package's own description says (shown on the card before you install; nothing is installed without it)",
            Item::YtDlp => "Unlicense (public domain; the program you download from its vendor, IYAKKI does not ship it). Saving a video may need the right-holder's permission: that is your responsibility",
            Item::Node => "MIT (Node.js; npm is Artistic-2.0)",
            Item::Python => "PSF licence (Python); uv is MIT or Apache-2.0",
            Item::K3s => "Apache-2.0",
            Item::DuckDb => "MIT",
            Item::Registry => "Apache-2.0 (CNCF Distribution registry:2). The cluster pulls the image from Docker Hub when you click Create; IYAKKI does not ship it",
            Item::Prometheus => "Apache-2.0 (Prometheus). The cluster pulls the image from quay.io when you click Install; IYAKKI does not ship it",
            Item::ClickHouse => clickhouse::LICENCE,
            Item::Matrix => matrix::LICENCE,
            Item::SeaweedFs => objstore::SEAWEEDFS_LICENCE,
            Item::RustFs => objstore::RUSTFS_LICENCE,
            Item::Gitea => "Gitea is MIT licensed (checked in the LICENSE file of the pinned release 1.24.5; its dependencies keep their own licences). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it",
            Item::GraphDb => "Neo4j Community Edition is GPL-3.0 (free software with copyleft; commercial editions exist and are not used). It runs as its own pod and IYAKKI only talks to it over HTTP; the cluster pulls the image from Docker Hub when you click Install, IYAKKI does not ship it",
            Item::Elasticsearch => "Elastic License 2.0 or SSPL-1.0 (your choice; the pinned release 8.15.3 predates Elastic's AGPL-3.0 option, per its LICENSE.txt). Source-available, not open source. The cluster pulls the image from Elastic's registry when you click Install and runs it for you; it is downloaded by you, never shipped by IYAKKI",
            Item::OpenSearch => opensearch::LICENCE,
            Item::ClaudeCode => "proprietary (Anthropic's commercial terms); your own Anthropic account",
            Item::Copilot => "proprietary (GitHub's terms); needs a GitHub Copilot subscription",
            Item::Kokoro => "IYAKKI provides integration code only. Lemonade (Apache-2.0) pulls the Kokoro model at your click under the model's own licence, which Lemonade or the model's page states; IYAKKI ships no weights and gives no guarantee about them",
            Item::VoicePack => crate::voicepack::LICENCE_SHORT,
            Item::Lemonade => "Apache-2.0 (the Lemonade server image, pulled by the cluster at your click, not shipped); models and engines that Lemonade or you download have their own licences, outside IYAKKI",
            Item::Ollama => ollama_cluster::LICENCE,
            Item::MacHost => "nothing is installed by IYAKKI. On the Mac you choose: Ollama (MIT), llama.cpp (MIT), MLX-LM (MIT) or LM Studio (free to use, proprietary)",
            Item::GpuVulkan => "Mesa (MIT) and llama.cpp (MIT) in the pod image; the device plugin is squat/generic-device-plugin (Apache-2.0), pulled by the cluster; IYAKKI ships no GPU driver code of its own",
            Item::GpuRocm => "kernel driver (amdgpu, KFD): GPL-2.0 in the image. Device plugin: Apache-2.0. ROCm user space in the container image: MIT and other permissive licences per AMD's image, pulled by your cluster at your click, never shipped",
            Item::GpuNvidia => "kernel modules: MIT / GPL-2.0 (NVIDIA open GPU kernel modules, in the image). User-space libraries, nvidia-smi and GSP firmware: NVIDIA's proprietary driver licence, downloaded by you from NVIDIA, not shipped. Device plugin: Apache-2.0. Pressing Install accepts NVIDIA's licence for the downloaded files",
            Item::MacFirmware => "Apple's firmware files: Apple's own licence, no redistribution, so IYAKKI never ships them; they are fetched for your own Mac at your request. 7-Zip (the reader): GNU LGPL with the unRAR restriction, downloaded from its project when needed",
            Item::MacCamera => "driver: the open facetimehd driver (GPL-2.0, in the Mac image). Firmware and sensor files: Apple's own licence, no redistribution, so IYAKKI never ships them; they are fetched from Apple's CDN for your own Mac at your click",
            Item::LinuxWindows => "Debian packages, each with its own licence (TigerVNC: GPL-2.0, matchbox: GPL-2.0, Mesa: MIT, xterm: MIT, fonts: free licences), downloaded by apt inside the sandbox at your click, never shipped. \"Debian\" is a registered trademark of Software in the Public Interest, Inc.; IYAKKI does not rebrand it and is not endorsed by it",
            Item::LinuxApps => "Debian and Ubuntu are free software; every package keeps its own licence (/usr/share/doc/<package>/copyright inside the sandbox). Downloaded from the distribution, not shipped. \"Ubuntu\" and \"Debian\" are trademarks of their owners; IYAKKI does not rebrand them",
            Item::Ffmpeg => "GPLv3 (the static build you download from its builder; IYAKKI does not ship it). H.264 and AAC are patent-licensed formats: decoding them is your choice, as in the browser",
            Item::FfmpegLibs => "LGPL-2.1 or later (the libraries you supply: IYAKKI does not ship it and does not host it; the licence texts and the source offer travel with them and you read the archive's NOTICE and the source offer before you confirm). H.264 and AAC are patent-licensed formats in some countries: decoding them is your choice and responsibility",
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => self.fw_def().map_or("", |d| d.licence_note),
            Item::FwHardware => "Various per file: each vendor's own licence (listed from WHENCE before you click, texts kept next to the files). Free to redistribute with notice for most; some are restricted to the vendor's hardware (MediaTek, Marvell, Qualcomm, Broadcom, NVIDIA). Not in the images: downloaded for you from the upstream linux-firmware repository, never shipped.",
        }
    }

    /// What has to be there first.
    pub fn needs(self) -> &'static [Item] {
        match self {
            Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Lemonade | Item::Ollama | Item::Registry | Item::GpuVulkan | Item::GpuNvidia | Item::GpuRocm => &[Item::K3s],
            Item::Kokoro | Item::VoicePack => &[Item::Lemonade],
            Item::DataScience => &[Item::LinuxApps],
            Item::YtDlp => &[],
            _ => &[],
        }
    }

    /// Everything that has to be there first: [`Item::needs`], plus the runtime the catalogue says the tool installs with (an npm
    /// tool needs Node.js, a pip tool Python 3), so the order shows "Node.js → Copilot" like "k3s → DuckDB".
    pub fn needs_now(self) -> Vec<Item> {
        let mut v: Vec<Item> = self.needs().to_vec();
        // a pip tool needs Python 3, an npm tool Node.js (the CLI catalogue says which tools those are; a test in the agent keeps these two in step with it)
        if self.pip_tool() {
            v.push(Item::Python);
        }
        if self.npm_tool() {
            v.push(Item::Node);
        }
        v
    }

    /// The CLI tools the catalogue installs with npm (`iyakki_agent::clis`: `Source::Npm`); none today.
    pub fn npm_tool(self) -> bool {
        false
    }

    /// The CLI tools the catalogue installs with pip (`Source::Pip`).
    pub fn pip_tool(self) -> bool {
        matches!(self, Item::DuckDb)
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
            Item::OpenSearch => 1000,
            Item::Prometheus => 120,
            Item::GraphDb => 190,
            Item::ClickHouse => 220,
            Item::Gitea => 110,
            Item::Matrix => 36,
            Item::SeaweedFs => 150,
            Item::RustFs => 100,
            Item::Registry => 25,
            Item::ClaudeCode => 60,
            Item::Copilot => 40,
            Item::Kokoro => 350,
            Item::VoicePack => crate::voicepack::WHISPER_MB + crate::voicepack::KOKORO_MB,
            Item::Lemonade => 700,
            Item::Ollama => 1500,
            Item::MacHost => 0,
            Item::GpuVulkan => 300,
            Item::GpuNvidia => 400,
            Item::GpuRocm => 6000,
            Item::Ffmpeg => 42,
            Item::FfmpegLibs => 0,
            Item::MacFirmware => 1500,
            Item::MacCamera => 18,
            Item::LinuxApps => 30,
            Item::LinuxWindows => 250,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwAtheros9170 => 1,
            Item::FwMediatek => 9,
            Item::FwBroadcomNet => 3,
            Item::FwHardware => 5,
            Item::DataScience => crate::datasci::total_mb(),
            Item::BrowserPackage => 90,
            Item::YtDlp => 3,
        }
    }

    /// About how long it takes on a normal connection, in words.
    pub fn time_text(self) -> &'static str {
        match self {
            Item::MacHost => "a few minutes (your steps)",
            Item::Elasticsearch | Item::OpenSearch | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Lemonade | Item::Ollama | Item::Kokoro | Item::VoicePack | Item::MacFirmware | Item::GpuNvidia | Item::GpuRocm | Item::GpuVulkan => "5 to 15 minutes",
            Item::K3s => "2 to 5 minutes",
            Item::DataScience => "3 to 8 minutes",
            Item::BrowserPackage => "a minute or two",
            Item::YtDlp => "under a minute",
            _ => "about a minute",
        }
    }

    /// A server that keeps running once installed (the Setup tab then says Running, not just Installed).
    pub fn is_service(self) -> bool {
        matches!(self, Item::K3s | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Registry | Item::Lemonade | Item::Ollama)
    }

    /// Hardware entries: shown only on a machine that has the hardware, and never in cloud mode.
    pub fn is_hardware(self) -> bool {
        matches!(self, Item::GpuVulkan | Item::GpuNvidia | Item::GpuRocm | Item::MacFirmware | Item::MacCamera | Item::FwHardware) || self.fw_def().is_some()
    }

    /// Memory (megabytes) a machine should have before this is recommended; 0 = light.
    pub fn min_ram_mb(self) -> u64 {
        match self {
            Item::Elasticsearch => 6000,
            Item::OpenSearch => 3000,
            Item::GraphDb => 3000,
            Item::ClickHouse => 3000,
            Item::Gitea => 1500,
            Item::Matrix => 1500,
            Item::SeaweedFs | Item::RustFs => 2000,
            Item::Lemonade | Item::Ollama | Item::Kokoro | Item::VoicePack | Item::GpuVulkan | Item::GpuNvidia | Item::GpuRocm => 7500,
            Item::DataScience => 1500,
            Item::YtDlp => 100,
            _ => 0,
        }
    }

    /// Installed by a detached job (the Mac host and the firmware items have their own steps).
    pub fn runs_as_job(self) -> bool {
        !matches!(self, Item::MacHost | Item::MacCamera | Item::FwHardware) && self.fw_def().is_none()
    }

    /// The restricted firmware pack this item installs (fwpack.rs), when it is one.
    pub fn fw_def(self) -> Option<&'static crate::fwpack::Def> {
        match self {
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => crate::fwpack::def_for_pack(self.id()),
            _ => None,
        }
    }
}

/// Whether a newer version exists for an installed item.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// "1. k3s → 2. Elasticsearch" (what the menu shows before Install).
pub fn order_text(steps: &[Item]) -> String {
    titles_text(&steps.iter().map(|s| s.title().to_string()).collect::<Vec<_>>())
}

/// "1. Node.js → 2. GitHub Copilot CLI".
pub fn titles_text(titles: &[String]) -> String {
    titles.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("  \u{2192}  ")
}

/// What the menu needs to know about this machine, read off the screen's thread (it starts programs for their versions): where
/// each item stands, which installed tools cannot run, what the catalogue installs under each, and who depends on a runtime.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Facts {
    pub presence: Vec<(Item, Presence)>,
    /// Installed by IYAKKI but not runnable (the plain words of `CliManager::health`): offered Reinstall.
    pub unhealthy: Vec<(Item, String)>,
    /// `CliManager::install_order` minus the item itself: the runtimes (uv, Python, Node.js) installed before it.
    #[serde(deserialize_with = "crate::serde_static::keyed_words")]
    pub runtimes: Vec<(Item, Vec<&'static str>)>,
    /// A runtime used by installed tools (`CliManager::uninstall_warning`).
    pub used_by: Vec<(Item, String)>,
    /// Rows the menu does not show on this machine (the NVIDIA driver entry when no NVIDIA GPU is found).
    pub hidden: Vec<Item>,
    /// Cluster items whose Deployment has all its replicas ready right now (read from the cluster, not from a job): such an item is
    /// installed and running whatever an older (or a repeated) job said.
    pub healthy: Vec<Item>,
    /// Items whose password waits for the person's vault ([`crate::vaultpending`]).
    pub waiting: Vec<Item>,
    /// Items whose newest attempt failed after an earlier install (the `<item>.failed` record), with the reason (its cause class first).
    /// An item that is healthy in the cluster right now is not listed.
    pub failures: Vec<(Item, String)>,
}

/// The reason of a failed last attempt, when the presence carries it ([`LAST_FAILED`]).
pub fn failure_reason(p: &Presence) -> Option<&str> {
    match p {
        Presence::Earlier(d) => d.strip_prefix(LAST_FAILED),
        _ => None,
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

/// The folder of this module's own files (`<data>/quick-install`).
pub fn marker(data: &Path, item: Item) -> PathBuf {
    state_dir(data).join(format!("{}.done", item.id()))
}

pub fn failed_path(data: &Path, item: Item) -> PathBuf {
    state_dir(data).join(format!("{}.failed", item.id()))
}

/// Forget that a step failed.
pub fn clear_failure(data: &Path, item: Item) {
    let _ = std::fs::remove_file(failed_path(data, item));
}

/// Forget that the registry step finished (after the registry was removed).
pub fn unmark(data: &Path, item: Item) {
    let _ = std::fs::remove_file(marker(data, item));
    clear_failure(data, item);
}

/// The registry step's arguments (the managed cluster when `target` is `None`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegistryArgs {
    pub target: Option<crate::localreg::Target>,
    pub size_gi: u32,
    pub node_port: u16,
}

pub fn state_dir(data: &Path) -> PathBuf {
    data.join("quick-install")
}

/// Where the Elasticsearch Deployment goes.
pub const NAMESPACE: &str = "iyakki-data";

/// Where the Prometheus Deployment goes (its own namespace: the read-only cluster role belongs to it alone).
pub const PROM_NAMESPACE: &str = "iyakki-monitoring";

/// Where the Lemonade Deployment goes.
pub const LEMONADE_NAMESPACE: &str = "iyakki-ai";

/// The word after `quick-install` of the camera firmware job.
pub const CAMERA_JOB: &str = "mac-camera";

/// How the hardware-firmware job gets its files (fwextra.rs).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HwFirmwareOpts {
    /// A folder or stick (a `firmware-extra` folder, an unpacked release, the release tarball, or the staging folder of the developer link).
    pub from: Option<PathBuf>,
    /// Separate licence groups the person included (`nvidia`).
    pub with: Vec<String>,
    /// Use the other computer's internet over this cable interface for the job only (fwextra_net.rs).
    pub link: Option<String>,
}
