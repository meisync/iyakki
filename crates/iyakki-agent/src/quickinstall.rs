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
/// The Elasticsearch image, by tag AND digest (the manifest list of 8.15.3 from docker.elastic.co, read from the registry API on 2026-10-27).
pub const ES_IMAGE: &str = "docker.elastic.co/elasticsearch/elasticsearch:8.15.3@sha256:01c1732062b4a846c5ca687b0094b89bad0bfed00c6d71626db32fb8f3131a78";
/// Where the Prometheus Deployment goes (its own namespace: the read-only cluster role belongs to it alone).
pub const PROM_NAMESPACE: &str = "iyakki-monitoring";
/// The Prometheus image, by tag AND digest (the vendor's registry, quay.io; the digest is the manifest list of the tag v3.5.0, read from the
/// registry API on 2026-10-27). Change the tag and the digest here only, and verify the digest against the registry.
pub const PROM_IMAGE: &str = "quay.io/prometheus/prometheus:v3.5.0@sha256:63805ebb8d2b3920190daf1cb14a60871b16fd38bed42b857a3182bc621f4996";
/// The id of the DASH source the Prometheus step registers.
pub const PROM_SOURCE_ID: &str = "prometheus-cluster";
/// The DASH source of the in-cluster Elasticsearch.
pub const ES_SOURCE_ID: &str = "elasticsearch-cluster";
/// The graph database image (Neo4j Community, GPL-3.0; pinned by tag AND digest, the digest is the index of both architectures, verified
/// against Docker Hub on 2026-10-26, docs/development/GRAPH-DB-CHOICE.md). Change the tag and the digest here only.
pub const GRAPH_IMAGE: &str = "docker.io/library/neo4j:2026.10.1-community@sha256:f238979f3c80da6f383f2e4fe91be4dc3580d7ee3b7edec83d0b8783c53b74a4";
/// The id of the DASH source the graph database step registers.
pub const GRAPH_SOURCE_ID: &str = "graphdb-cluster";
/// The graph database as DASH and the AI reach it (a Service of the cluster, looked up at every connect).
pub const GRAPH_DASH_URL: &str = "cluster://iyakki-data/graphdb:7474";
/// The Kubernetes Secret that holds the administrator password for the pod (keys `NEO4J_AUTH` and `password`).
pub const GRAPH_SECRET: &str = "graphdb-auth";
/// The vault entry (the person's own vault) that holds the same password for DASH.
pub const GRAPH_VAULT_NAME: &str = "svc.graphdb";
/// The user name of the administrator (the only one Neo4j Community has).
pub const GRAPH_USER: &str = "neo4j";
/// How DASH names the in-cluster Services: looked up in the cluster at every connect, so a new cluster IP never goes stale.
pub const PROM_DASH_URL: &str = "cluster://iyakki-monitoring/prometheus:9090";
pub const ES_DASH_URL: &str = "cluster://iyakki-data/elasticsearch:9200";
/// Where the Lemonade Deployment goes.
pub const LEMONADE_NAMESPACE: &str = "iyakki-ai";
/// The Lemonade server image, by tag AND digest (the OCI index of v2026.42.1 from ghcr.io, read from the registry API on 2026-10-27; the project
/// publishes amd64 only).
pub const LEMONADE_IMAGE: &str = "ghcr.io/lemonade-sdk/lemonade-server:v2026.42.1@sha256:bd4b89738ca57a90ce3276ddf9270c317d369dc4bc4eded94568d37b88c2bc0d";
/// How long a step that needs the cluster waits for it to be installed and Ready.
pub const WAIT_FOR_CLUSTER: Duration = Duration::from_secs(25 * 60);
/// How long the k3s step waits, after starting the cluster, for it to be Ready before it calls the install failed.
pub const K3S_READY_WAIT: Duration = Duration::from_secs(10 * 60);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
}

impl Item {
    pub const ALL: [Item; 35] = [Item::Node, Item::Python, Item::K3s, Item::DuckDb, Item::Elasticsearch, Item::OpenSearch, Item::Prometheus, Item::ClickHouse, Item::GraphDb, Item::Gitea, Item::Matrix, Item::SeaweedFs, Item::RustFs, Item::Registry, Item::ClaudeCode, Item::Copilot, Item::Kokoro, Item::VoicePack, Item::Lemonade, Item::Ollama, Item::MacHost, Item::GpuVulkan, Item::GpuNvidia, Item::Ffmpeg, Item::FfmpegLibs, Item::MacFirmware, Item::MacCamera, Item::LinuxApps, Item::LinuxWindows, Item::FwQualcommBt, Item::FwRealtekNet, Item::FwBroadcomNet, Item::FwAtheros9170, Item::FwMediatek, Item::FwHardware];

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
            Item::Ffmpeg => "Media player (ffmpeg)",
            Item::FfmpegLibs => "Browser audio and video (H.264, AAC)",
            Item::MacFirmware => "Mac Wi-Fi and Bluetooth firmware (from your Mac)",
            Item::MacCamera => "MacBook FaceTime camera: driver and firmware (firmware from Apple)",
            Item::LinuxApps => "Linux apps (Debian/Ubuntu)",
            Item::LinuxWindows => "Linux windows (graphical programs)",
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => self.fw_def().map_or("Firmware", |d| d.title),
            Item::FwHardware => "Hardware firmware (Wi-Fi, Bluetooth, graphics, ...)",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Item::Node | Item::Python => Group::Runtimes,
            Item::K3s | Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Registry => Group::Cluster,
            Item::ClaudeCode | Item::Copilot => Group::AiCoding,
            Item::Kokoro | Item::VoicePack => Group::Voice,
            Item::Lemonade | Item::Ollama | Item::MacHost | Item::GpuVulkan | Item::GpuNvidia => Group::LocalModels,
            Item::Ffmpeg | Item::FfmpegLibs => Group::Media,
            Item::MacFirmware | Item::MacCamera => Group::Mac,
            Item::LinuxApps | Item::LinuxWindows => Group::Linux,
            Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek | Item::FwHardware => Group::Firmware,
        }
    }

    /// One line for the menu.
    pub fn about(self) -> &'static str {
        match self {
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
            Item::DuckDb | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Lemonade | Item::Ollama | Item::Registry | Item::GpuVulkan | Item::GpuNvidia => &[Item::K3s],
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
        }
    }

    /// About how long it takes on a normal connection, in words.
    pub fn time_text(self) -> &'static str {
        match self {
            Item::MacHost => "a few minutes (your steps)",
            Item::Elasticsearch | Item::OpenSearch | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Lemonade | Item::Ollama | Item::Kokoro | Item::VoicePack | Item::MacFirmware | Item::GpuNvidia | Item::GpuVulkan => "5 to 15 minutes",
            Item::K3s => "2 to 5 minutes",
            _ => "about a minute",
        }
    }

    /// A server that keeps running once installed (the Setup tab then says Running, not just Installed).
    pub fn is_service(self) -> bool {
        matches!(self, Item::K3s | Item::Elasticsearch | Item::OpenSearch | Item::Prometheus | Item::ClickHouse | Item::GraphDb | Item::Gitea | Item::Matrix | Item::SeaweedFs | Item::RustFs | Item::Registry | Item::Lemonade | Item::Ollama)
    }

    /// Hardware entries: shown only on a machine that has the hardware, and never in cloud mode.
    pub fn is_hardware(self) -> bool {
        matches!(self, Item::GpuVulkan | Item::GpuNvidia | Item::MacFirmware | Item::MacCamera | Item::FwHardware) || self.fw_def().is_some()
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
            Item::Lemonade | Item::Ollama | Item::Kokoro | Item::VoicePack | Item::GpuVulkan | Item::GpuNvidia => 7500,
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
            Presence::Earlier(d) if d.starts_with(LAST_FAILED) => format!("installed earlier; {d}"),
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

/// Remember that a step finished (what the job's last line said). A success also clears the record of an earlier failed attempt.
pub fn mark_done(data: &Path, item: Item, detail: &str) -> Result<(), String> {
    mark_done_note(data, item, detail, "")
}

/// [`mark_done`] with what the check before the marker saw (a third line; [`marked`] still returns the detail only).
pub fn mark_done_note(data: &Path, item: Item, detail: &str, note: &str) -> Result<(), String> {
    std::fs::create_dir_all(state_dir(data)).map_err(|e| e.to_string())?;
    let note_line = if note.is_empty() { String::new() } else { format!("{note}\n") };
    std::fs::write(marker(data, item), format!("{}\n{detail}\n{note_line}", crate::clock::now_unix())).map_err(|e| e.to_string())?;
    clear_failure(data, item);
    Ok(())
}

/// What the independent check saw before the marker was written, when the marker has it.
pub fn marked_note(data: &Path, item: Item) -> Option<String> {
    std::fs::read_to_string(marker(data, item)).ok().and_then(|t| t.lines().nth(2).map(|l| l.trim().to_string())).filter(|l| !l.is_empty())
}

/// Check the thing the person asked for ([`StepEnv::verify`]) and only then write the marker. A failed check fails the step ("Verify
/// failed: ...") and writes no marker; what the check saw is kept in the marker beside the detail.
pub fn mark_verified(env: &mut dyn StepEnv, item: Item, detail: &str) -> Result<(), String> {
    let note = env.verify(item, detail).map_err(|e| format!("Verify failed: {}. {} is not marked as installed", e.trim().trim_end_matches('.'), item.title()))?;
    if !note.is_empty() {
        env.say(&format!("  checked: {note}"));
    }
    env.set_verified_note(&note);
    env.mark(item, detail)
}

// ------------------------------------------------------------------------------------------------ the last attempt failed

fn failed_path(data: &Path, item: Item) -> PathBuf {
    state_dir(data).join(format!("{}.failed", item.id()))
}

/// Remember that the newest attempt failed or timed out (the reason with its cause class first, and when). Cleared by a success.
pub fn record_failure(data: &Path, item: Item, reason: &str) {
    let _ = std::fs::create_dir_all(state_dir(data));
    let one_line = reason.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    let _ = std::fs::write(failed_path(data, item), format!("{}\n{one_line}\n", crate::clock::now_unix()));
}

/// The newest failed attempt, `(when, reason)`, if no success has come after it.
pub fn last_failure(data: &Path, item: Item) -> Option<(i64, String)> {
    let t = std::fs::read_to_string(failed_path(data, item)).ok()?;
    let mut l = t.lines();
    let at = l.next()?.trim().parse().ok()?;
    Some((at, l.next().unwrap_or("").trim().to_string()))
}

pub fn clear_failure(data: &Path, item: Item) {
    let _ = std::fs::remove_file(failed_path(data, item));
}

/// The words that start a presence detail when the newest attempt failed after an earlier install.
pub const LAST_FAILED: &str = "the last attempt failed: ";

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
    let now = presence_plain_with(svc, item, lemonade);
    // an old install is still there, but the newest attempt (a Retry, an update) failed: say so; a success clears the record
    match (&now, last_failure(svc.dir(), item)) {
        (Presence::Installed(_) | Presence::Earlier(_), Some((_, why))) => Presence::Earlier(format!("{LAST_FAILED}{why}")),
        _ => now,
    }
}

/// [`presence_with`] without the record of a failed last attempt (what is there, whatever the newest attempt did).
pub fn presence_plain_with(svc: &crate::services::Services, item: Item, lemonade: &dyn Fn() -> Option<Vec<crate::lemonade::Model>>) -> Presence {
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
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => match item.fw_def().and_then(|d| crate::fwpack::installed(data, d)) {
            Some(r) => Presence::Installed(format!("{} files, {}", r.files.iter().filter(|f| !f.starts_with("licences/")).count(), r.version)),
            None => Presence::Missing,
        },
        Item::FwHardware => match crate::fwextra::installed(data) {
            Some(r) => Presence::Installed(format!("{} files, {}", r.files.iter().filter(|f| !f.starts_with("licences/")).count(), r.version)),
            None => Presence::Missing,
        },
        Item::MacCamera => match crate::camfw::read_outcome(data) {
            Some(o) if o.installed => Presence::Installed(if o.firmware_version.is_empty() { o.headline } else { format!("firmware {}", o.firmware_version) }),
            _ => Presence::Missing,
        },
        Item::Gitea => gitea::presence(data),
        Item::Ollama => {
            let host = svc.clis.installed().into_iter().find(|i| i.id == "ollama").map(|i| i.version);
            ollama_cluster::presence(data, host, svc.clis.present_elsewhere("ollama"), &crate::ollama::probe)
        }
        Item::FfmpegLibs => crate::avaddon_install::installed_note(data).map_or(Presence::Missing, Presence::Installed),
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
        // the pod runs a pinned image: there is no program version to compare once it runs in the cluster
        Item::Ollama if marked(svc.dir(), Item::Ollama).is_some_and(|d| ollama_cluster::is_cluster_detail(&d)) => None,
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
    /// Cluster items whose Deployment has all its replicas ready right now (read from the cluster, not from a job): such an item is
    /// installed and running whatever an older (or a repeated) job said.
    pub healthy: Vec<Item>,
    /// Items whose password waits for the person's vault ([`crate::vaultpending`]).
    pub waiting: Vec<Item>,
    /// Items whose newest attempt failed after an earlier install (the `<item>.failed` record), with the reason (its cause class first).
    /// An item that is healthy in the cluster right now is not listed.
    pub failures: Vec<(Item, String)>,
}

/// The Deployment each cluster item runs as: `(namespace, name)`.
pub fn cluster_object(item: Item) -> Option<(&'static str, &'static str)> {
    Some(match item {
        Item::Elasticsearch => (NAMESPACE, "elasticsearch"),
        Item::GraphDb => (NAMESPACE, "graphdb"),
        Item::Prometheus => (PROM_NAMESPACE, "prometheus"),
        Item::OpenSearch => (opensearch::NS, "opensearch"),
        Item::ClickHouse => (clickhouse::NS, "clickhouse"),
        Item::Matrix => (matrix::NS, "matrix"),
        Item::SeaweedFs => (objstore::NS, "seaweedfs"),
        Item::RustFs => (objstore::NS, "rustfs"),
        Item::Gitea => (gitea::NAMESPACE, "gitea"),
        Item::Ollama => (LEMONADE_NAMESPACE, "ollama"),
        _ => return None,
    })
}

/// The cluster items that `kubectl get deployments -A -o json` shows with every replica ready (and at least one wanted).
pub fn healthy_from(deployments_json: &str) -> Vec<Item> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(deployments_json) else { return Vec::new() };
    let Some(items) = v["items"].as_array() else { return Vec::new() };
    Item::ALL
        .into_iter()
        .filter(|it| {
            let Some((ns, name)) = cluster_object(*it) else { return false };
            items.iter().any(|d| {
                d["metadata"]["namespace"].as_str() == Some(ns) && d["metadata"]["name"].as_str() == Some(name) && {
                    let want = d["spec"]["replicas"].as_u64().unwrap_or(1);
                    want > 0 && d["status"]["readyReplicas"].as_u64().unwrap_or(0) >= want
                }
            })
        })
        .collect()
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
    f.waiting = crate::vaultpending::list(svc.dir()).iter().filter_map(|p| Item::parse(&p.item)).collect();
    // the cluster's own word on the items that run in it (one read, only while the cluster answers)
    if svc.k3sdev.state() == crate::k3sdev::Svc::Running {
        let _ = uninstall::retain_all(svc);
        let o = svc.k3sdev.kubectl(&["get", "deployments", "-A", "-o", "json"]);
        if o.code == Some(0) {
            f.healthy = healthy_from(&o.stdout);
            for it in &f.healthy {
                if let Some((_, p)) = f.presence.iter_mut().find(|(x, _)| x == it) {
                    // healthy right now: a failed last attempt (a Retry, an update) does not make it "installed earlier"
                    if matches!(p, Presence::Earlier(d) if d.starts_with(LAST_FAILED)) {
                        *p = presence_plain_with(svc, *it, &crate::lemonade::probe_local_models);
                    }
                    if *p == Presence::Missing {
                        *p = Presence::Installed("running in the cluster".into());
                    }
                }
            }
        }
    }
    f.failures = f.presence.iter().filter_map(|(it, p)| failure_reason(p).map(|r| (*it, r.to_string()))).collect();
    f
}

/// The reason of a failed last attempt, when the presence carries it ([`LAST_FAILED`]).
pub fn failure_reason(p: &Presence) -> Option<&str> {
    match p {
        Presence::Earlier(d) => d.strip_prefix(LAST_FAILED),
        _ => None,
    }
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

/// How the hardware-firmware job gets its files (fwextra.rs).
#[derive(Clone, Debug, Default)]
pub struct HwFirmwareOpts {
    /// A folder or stick (a `firmware-extra` folder, an unpacked release, the release tarball, or the staging folder of the developer link).
    pub from: Option<PathBuf>,
    /// Separate licence groups the person included (`nvidia`).
    pub with: Vec<String>,
    /// Use the other computer's internet over this cable interface for the job only (fwextra_net.rs).
    pub link: Option<String>,
}

/// The job of "Hardware firmware": `iyakki quick-install fw-hardware --data D [--from FOLDER] [--with nvidia] [--link IFACE]`.
pub fn hardware_firmware_spec(l: &Launch, o: &HwFirmwareOpts) -> Spec {
    let mut argv = vec![l.exe.display().to_string(), "quick-install".into(), Item::FwHardware.id().into(), "--data".into(), l.data.display().to_string()];
    if let Some(f) = &o.from {
        argv.push("--from".into());
        argv.push(f.display().to_string());
    }
    for w in &o.with {
        argv.push("--with".into());
        argv.push(w.clone());
    }
    if let Some(i) = &o.link {
        argv.push("--link".into());
        argv.push(i.clone());
    }
    let how = if o.from.is_some() { " (from a folder)" } else if o.link.is_some() { " (over the cable)" } else { "" };
    Spec { label: format!("Hardware firmware{how}"), cmd: Cmd::Argv(argv), cwd: l.data.clone(), workspace: WORKSPACE.into() }
}

/// Start the hardware-firmware job. Refuses when the licence list the job will use is not accepted, or one is running already.
pub fn start_hardware_firmware(jobs: &Jobs, owner: &str, o: &HwFirmwareOpts, l: &Launch) -> Result<Job, String> {
    if jobs.visible(owner, WORKSPACE).iter().any(|j| j.running() && item_of(j) == Some(Item::FwHardware)) {
        return Err("Hardware firmware is being installed already: its progress is beside the clock in the top bar".into());
    }
    std::fs::create_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    jobs.start(owner, &hardware_firmware_spec(l, o), &|_: &Path| Ok(()))
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

/// What the graph database's password handoff thread found out (drained by [`service_requests`]).
static GRAPH_DONE: std::sync::Mutex<Vec<Result<String, String>>> = std::sync::Mutex::new(Vec::new());

/// Called by the shell every few seconds: start and connect what Quick install jobs asked for. k3s starts under the cluster's
/// supervisor; Ollama under its own (and becomes `ollama-local` in Models when it answers); Lemonade already runs in k3s, so it is only
/// added to Models as `lemonade-local` once it answers. Returns one line per thing done.
pub fn service_requests(svc: &std::sync::Arc<crate::services::Services>) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    if let Some(r) = service_start_request(svc) {
        out.push(r.map(|_| "k3s is installed and starting (Settings \u{2192} Dev cluster shows it)".to_string()).map_err(|e| format!("k3s was installed but did not start: {e}")));
    }
    if take_after(svc.dir(), ollama_cluster::REQ_CONNECT).is_some() {
        // Ollama runs in the cluster: it joins Models as ollama-local once it answers on its loopback port; when the person moved a host
        // install over, the host server is stopped only after that (nothing is deleted)
        let stop_host = take_after(svc.dir(), ollama_cluster::REQ_STOP_HOST).is_some();
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-ollama-connect".into()).spawn(move || {
            for _ in 0..90 {
                if crate::ollama::probe(ollama_cluster::LOCAL_PORT).is_some() {
                    me.use_cluster_ollama(&format!("http://127.0.0.1:{}", ollama_cluster::LOCAL_PORT));
                    if stop_host {
                        let _ = me.ollama.stop();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        out.push(Ok(if stop_host { "Ollama is running in the cluster; Models (ollama-local) points at it and the host Ollama is stopped once it answers".to_string() } else { "Ollama is running in the cluster; it joins Models as ollama-local when it answers".to_string() }));
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
    // the graph database was installed: its password goes from the cluster's Secret into the person's vault (on a thread: kubectl and the vault wait)
    if let Some(user) = take_after(svc.dir(), "graph-vault") {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-graph-vault".into()).spawn(move || {
            if let Some(r) = wait_for_unlock(&me, &pending_of(Item::GraphDb, &user)) {
                GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                return;
            }
            let read = || -> Result<zeroize::Zeroizing<String>, String> {
                let out = me.k3sdev.kubectl(&["-n", NAMESPACE, "get", "secret", GRAPH_SECRET, "-o", "go-template={{index .data \"password\" | base64decode}}"]);
                let pw = zeroize::Zeroizing::new(out.stdout.clone());
                if out.code == Some(0) { Ok(pw) } else { Err(out.stderr.trim().lines().last().unwrap_or("kubectl failed").to_string()) }
            };
            let store = |pw: &str| me.user_set(&user, GRAPH_VAULT_NAME, pw).map_err(|e| e.to_string());
            let r = graph_vault_handoff(&read, &store, &|d| std::thread::sleep(d), 40, Duration::from_secs(3));
            let r = settle_handoff(&me, pending_of(Item::GraphDb, &user), 40, r);
            GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        });
    }
    // ClickHouse was installed: its password goes from the cluster's Secret into the person's vault (on a thread, like the graph database's)
    if let Some(user) = take_after(svc.dir(), clickhouse::REQUEST) {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-clickhouse-vault".into()).spawn(move || {
            if let Some(r) = wait_for_unlock(&me, &pending_of(Item::ClickHouse, &user)) {
                GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                return;
            }
            let read = || -> Result<zeroize::Zeroizing<String>, String> {
                let out = me.k3sdev.kubectl(&["-n", NAMESPACE, "get", "secret", clickhouse::SECRET, "-o", "go-template={{index .data \"password\" | base64decode}}"]);
                let pw = zeroize::Zeroizing::new(out.stdout.clone());
                if out.code == Some(0) { Ok(pw) } else { Err(out.stderr.trim().lines().last().unwrap_or("kubectl failed").to_string()) }
            };
            let store = |pw: &str| me.user_set(&user, clickhouse::VAULT_NAME, pw).map_err(|e| e.to_string());
            let r = vault_handoff(&clickhouse::handoff(), &read, &store, &|d| std::thread::sleep(d), 40, Duration::from_secs(3));
            let r = settle_handoff(&me, pending_of(Item::ClickHouse, &user), 40, r);
            GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        });
    }
    // Gitea was installed: its password and API token go from the cluster's Secrets into the person's vault and the Git connection
    // "Local Gitea" is added (on a thread: kubectl and the vault wait)
    if let Some(user) = take_after(svc.dir(), gitea::REQUEST) {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-gitea-vault".into()).spawn(move || {
            if let Some(r) = wait_for_unlock(&me, &pending_of(Item::Gitea, &user)) {
                GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                return;
            }
            let r = gitea::finish(&me, &user);
            let r = settle_handoff(&me, pending_of(Item::Gitea, &user), 40, r);
            GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        });
    }
    // an object store was installed: its two keys go from the cluster's Secret into the person's vault, with the typed record s3.<name>
    for e in objstore::ENGINES {
        if let Some(user) = take_after(svc.dir(), e.request) {
            let me = svc.clone();
            let _ = std::thread::Builder::new().name(format!("iyakki-{}-vault", e.name)).spawn(move || {
                if let Some(r) = wait_for_unlock(&me, &pending_of(e.item, &user)) {
                    GRAPH_DONE.lock().unwrap_or_else(|x| x.into_inner()).push(r);
                    return;
                }
                let r = objstore::finish_real(&me, &user, e);
                let r = settle_handoff(&me, pending_of(e.item, &user), 40, r);
                GRAPH_DONE.lock().unwrap_or_else(|x| x.into_inner()).push(r);
            });
        }
    }
    // the Matrix server was installed: its registration token goes from the cluster's Secret into the person's vault (on a thread)
    if let Some(user) = take_after(svc.dir(), matrix::REQUEST) {
        let me = svc.clone();
        let _ = std::thread::Builder::new().name("iyakki-matrix-vault".into()).spawn(move || {
            if let Some(r) = wait_for_unlock(&me, &pending_of(Item::Matrix, &user)) {
                GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                return;
            }
            let read = || -> Result<zeroize::Zeroizing<String>, String> {
                let out = me.k3sdev.kubectl(&["-n", matrix::NS, "get", "secret", matrix::SECRET, "-o", "go-template={{index .data \"token\" | base64decode}}"]);
                let t = zeroize::Zeroizing::new(out.stdout.clone());
                if out.code == Some(0) { Ok(t) } else { Err(out.stderr.trim().lines().last().unwrap_or("kubectl failed").to_string()) }
            };
            let store = |t: &str| me.user_set(&user, matrix::VAULT_NAME, t).map_err(|e| e.to_string());
            let r = matrix::handoff(&read, &store, &|d| std::thread::sleep(d), 40, Duration::from_secs(3));
            let r = settle_handoff(&me, pending_of(Item::Matrix, &user), 40, r);
            GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        });
    }
    // passwords that waited for a locked vault: work them off as soon as the person's vault is open (no new install)
    pending_vault_retry(svc);
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
    out.extend(std::mem::take(&mut *GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner())));
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

/// Where an item's password is read from, for the pending list.
pub fn pending_of(item: Item, user: &str) -> crate::vaultpending::Pending {
    use crate::vaultpending::Pending;
    let p = |what: &str, ns: &str, secret: &str, key: &str, vault: &str, source: &str| Pending { item: item.id().into(), user: user.into(), what: what.into(), namespace: ns.into(), secret: secret.into(), key: key.into(), vault: vault.into(), source: source.into() };
    match item {
        Item::GraphDb => p("graph database", NAMESPACE, GRAPH_SECRET, "password", GRAPH_VAULT_NAME, "Cluster graph database"),
        Item::ClickHouse => p("ClickHouse", NAMESPACE, clickhouse::SECRET, "password", clickhouse::VAULT_NAME, "Cluster ClickHouse"),
        Item::SeaweedFs | Item::RustFs => p(&format!("{} keys", objstore::engine_of(item).map_or("object storage", |e| e.title)), objstore::NS, &objstore::engine_of(item).map(|e| e.secret()).unwrap_or_default(), "secret-key", &objstore::engine_of(item).map(|e| e.secret_vault()).unwrap_or_default(), ""),
        Item::Matrix => p("Matrix registration token", matrix::NS, matrix::SECRET, "token", matrix::VAULT_NAME, ""),
        _ => p("Gitea", gitea::NAMESPACE, crate::platform::gitea::ADMIN_SECRET, "password", "svc.gitea.admin-password", "Git connection \"Local Gitea\""),
    }
}

/// Run `f` with the real hooks of this machine (kubectl, the person's vault, the audit, the job log, the inbox).
fn with_real_hooks<R>(svc: &std::sync::Arc<crate::services::Services>, f: impl FnOnce(&crate::vaultpending::Hooks) -> R) -> R {
    let unlocked = |u: &str| svc.user_vault_unlocked(u);
    let move_one = |p: &crate::vaultpending::Pending| -> Result<String, String> {
        if p.item == Item::Gitea.id() {
            return gitea::finish(svc, &p.user);
        }
        if let Some(e) = objstore::engine_named(&p.item) {
            return objstore::finish_real(svc, &p.user, e);
        }
        let read = |p: &crate::vaultpending::Pending| -> Result<zeroize::Zeroizing<String>, String> {
            let out = svc.k3sdev.kubectl(&["-n", &p.namespace, "get", "secret", &p.secret, "-o", &format!("go-template={{{{index .data \"{}\" | base64decode}}}}", p.key)]);
            let v = zeroize::Zeroizing::new(out.stdout.clone());
            if out.code == Some(0) { Ok(v) } else { Err(out.stderr.trim().lines().last().unwrap_or("kubectl failed").to_string()) }
        };
        let store = |u: &str, n: &str, v: &str| svc.user_set(u, n, v).map_err(|e| e.to_string());
        let has = |u: &str, n: &str| svc.user_names(u).is_ok_and(|x| x.iter().any(|y| y == n));
        crate::vaultpending::move_secret(p, &read, &store, &has)
    };
    let audit = |u: &str, a: &str, d: &str| svc.k3sdev.audit_line(u, a, d);
    let joblog = |p: &crate::vaultpending::Pending, line: &str| {
        let newest = svc.jobs.visible_all(&p.user).into_iter().filter(|j| item_of(j).is_some_and(|i| i.id() == p.item)).max_by_key(|j| j.started_ms);
        if let Some(j) = newest {
            let _ = svc.jobs.append_log(&p.user, &j.id, line);
        }
    };
    f(&crate::vaultpending::Hooks { unlocked: &unlocked, move_one: &move_one, audit: &audit, joblog: &joblog })
}

/// The end of a handoff thread: success is logged; a give-up is remembered (where the password is, never the password), logged in the
/// job's log, the audit and the shell log, left in the inbox, and the toast says what waits and what to do.
pub fn settle_handoff(svc: &std::sync::Arc<crate::services::Services>, p: crate::vaultpending::Pending, tries: u32, r: Result<String, String>) -> Result<String, String> {
    with_real_hooks(svc, |h| match r {
        Ok(m) => {
            crate::vaultpending::record_stored(svc.dir(), &p, h);
            Ok(m)
        }
        Err(e) => {
            let words = crate::vaultpending::record_waiting(svc.dir(), p.clone(), tries, &e, h);
            let title = format!("The {} password is waiting for your vault", p.what);
            if !svc.inbox.list(&p.user).iter().any(|n| !n.read && n.title == title) {
                let _ = svc.inbox.add(&p.user, crate::inbox::Kind::Warning, &title, &format!("{words}. The service works; its password is safe in the cluster Secret {} and moves into your vault on its own once the vault is open.", p.secret), "quick install", None, crate::clock::now_unix());
            }
            Err(words)
        }
    })
}

/// The person's vault is locked when a hand-off would start: there is nothing to retry (the vault opens only with the password typed at
/// sign-in or Settings → Vault → Unlock), so the 40 tries are skipped. The wait is recorded at once, in words, and the
/// pending list moves the password into the vault as soon as it opens. `None` means the vault is open and the hand-off runs.
pub(crate) fn wait_for_unlock(svc: &std::sync::Arc<crate::services::Services>, p: &crate::vaultpending::Pending) -> Option<Result<String, String>> {
    if svc.user_vault_unlocked(&p.user) {
        return None;
    }
    Some(settle_handoff(svc, p.clone(), 0, Err("your vault is locked: it opens when you sign in with your password, or with Settings \u{2192} Vault \u{2192} Unlock".into())))
}

/// Work off the pending passwords whose vault is open now (on a thread; one at a time). Lines go to the shell like any other result.
fn pending_vault_retry(svc: &std::sync::Arc<crate::services::Services>) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let waiting = crate::vaultpending::list(svc.dir());
    if waiting.is_empty() || !waiting.iter().any(|p| svc.user_vault_unlocked(&p.user)) || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let me = svc.clone();
    let _ = std::thread::Builder::new().name("iyakki-vault-pending".into()).spawn(move || {
        let done = with_real_hooks(&me, |h| crate::vaultpending::finish_all(me.dir(), h));
        GRAPH_DONE.lock().unwrap_or_else(|e| e.into_inner()).extend(done);
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });
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
    /// Install the browser's audio and video add-on from what the administrator supplied (`avaddon_install.rs`); what was installed.
    fn install_av_addon(&mut self) -> Result<String, String> {
        Err("the audio and video add-on cannot be installed here".into())
    }
    /// Build the browser add-on in an isolated k3s Job (`avbuild.rs`) and put the package in /local/packages; it is NOT installed.
    fn build_av_addon(&mut self) -> Result<String, String> {
        Err("building the audio and video add-on is not available here".into())
    }
    /// The administrator asked for the build route (the Store wrote the request after the consent text); true once per request.
    fn av_build_requested(&mut self) -> bool {
        false
    }
    /// `kubectl apply` a manifest (kept as `<name>.yaml` in the module's folder); returns what kubectl printed.
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String>;
    /// Wait for a Deployment's rollout.
    fn rollout(&mut self, namespace: &str, name: &str, timeout: Duration) -> Result<(), String>;
    fn sleep(&mut self, d: Duration);
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String>;
    /// An independent check of the thing the person asked for, run BEFORE the marker is written (`mark_verified`): a CLI answers
    /// `--version`, a deployment is Ready and its Service has an address, a model is listed by Lemonade, the add-on's libraries pass
    /// their check. `Ok(note)`: what was seen (kept in the marker beside the detail); `Err`: the step fails ("Verify failed: ...") and
    /// no marker is written. The default checks nothing (an item without a cheap independent check keeps its own step's check).
    fn verify(&mut self, _item: Item, _detail: &str) -> Result<String, String> {
        Ok(String::new())
    }
    /// What the check saw, handed to the next [`StepEnv::mark`] (the real one keeps it in the marker; a fake may ignore it).
    fn set_verified_note(&mut self, _note: &str) {}
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
    /// Add (or replace, by id) a DASH source (the in-cluster Prometheus and Elasticsearch).
    fn register_dash_source(&mut self, _cfg: crate::dash::connector::SourceConfig) -> Result<(), String> {
        Ok(())
    }
    /// Create the Secret with the graph database's administrator password in [`NAMESPACE`] when it is not there yet (the password is
    /// generated here, written to a 0600 file for the one `kubectl create secret` and removed at once, and shown nowhere). `Ok(true)`:
    /// created now; `Ok(false)`: it was there and is kept (the database only reads the password the first time it starts).
    fn graph_secret(&mut self) -> Result<bool, String> {
        Err("creating the graph database's Secret is not available here".into())
    }
    /// Create the Secret with ClickHouse's `default` user password in [`NAMESPACE`] when it is not there yet (same rules as
    /// [`StepEnv::graph_secret`]: generated here, via a 0600 file, never shown). `Ok(true)`: created now; `Ok(false)`: kept.
    fn clickhouse_secret(&mut self) -> Result<bool, String> {
        Err("creating ClickHouse's Secret is not available here".into())
    }
    /// Create the Secret with the Matrix server's registration token in its namespace when it is not there yet (generated here, via a 0600
    /// file, never shown). `Ok(true)`: created now; `Ok(false)`: kept.
    fn matrix_secret(&mut self) -> Result<bool, String> {
        Err("creating the Matrix server's Secret is not available here".into())
    }
    /// Create the Secret with an object store's access key and secret key (and its identity file) in [`objstore::NS`] when it is not there
    /// yet (generated here, via 0600 files, never shown). `Ok(true)`: created now; `Ok(false)`: kept.
    fn objstore_secret(&mut self, _e: &objstore::Engine) -> Result<bool, String> {
        Err("creating the object store's Secret is not available here".into())
    }
    // ---- Ollama in the cluster (`ollama_cluster`)
    /// The choice the person made on the review card for an Ollama on the host (taken once); `None`: a plain install.
    fn ollama_mode(&mut self) -> Option<ollama_cluster::Mode> {
        None
    }
    /// The host Ollama's models folder, its size, and the free room on the data partition.
    fn ollama_host(&mut self) -> ollama_cluster::HostInfo {
        ollama_cluster::HostInfo::default()
    }
    /// Copy the host models folder into the cluster Ollama's volume (a copy; the host folder is not changed); a plain result line.
    fn ollama_copy(&mut self, _dir: &Path) -> Result<String, String> {
        Err("copying the models into the cluster is not available here".into())
    }
    /// The model names the Ollama of the cluster (`cluster` true, on its loopback port) or of the host (port 11434) lists.
    fn ollama_models(&mut self, _cluster: bool) -> Result<Vec<String>, String> {
        Err("asking Ollama is not available here".into())
    }
    /// The image id (with its digest) the cluster pulled for a Deployment's first container; `None` where it cannot be read.
    fn image_id(&mut self, _ns: &str, _deploy: &str) -> Option<String> {
        None
    }
    // ---- Gitea (the `gitea` submodule): nothing here ever returns, prints or stores a password or token except into a Secret
    /// The managed cluster as a platform service record names it.
    fn cluster_ref(&mut self) -> Result<crate::platform::installed::ClusterRef, String> {
        Err("naming the cluster is not available here".into())
    }
    /// The person's installed platform services (`installed-services-<user>.json`).
    fn service_records(&mut self) -> Vec<crate::platform::installed::Installed> {
        vec![]
    }
    fn save_service(&mut self, _rec: crate::platform::installed::Installed) -> Result<(), String> {
        Err("recording the service is not available here".into())
    }
    /// Nothing on this machine listens on that loopback port.
    fn port_free(&mut self, _port: u16) -> bool {
        true
    }
    /// A Gitea that is in the cluster (Deployment `gitea` and PVC `gitea-data` in the Gitea namespace) whatever the records say.
    fn live_gitea(&mut self) -> Option<gitea::LiveGitea> {
        None
    }
    /// The HTTP status of a GET (no credentials) from this machine.
    fn http_status(&mut self, _url: &str) -> Result<u16, String> {
        Err("asking an address is not available here".into())
    }
    /// Create the Secret `name` in `ns` with a generated password under `key` (and the labels of the platform instance) when it is not
    /// there yet; `Ok(true)` created now, `Ok(false)` kept. The password is shown nowhere.
    fn generated_secret(&mut self, _ns: &str, _name: &str, _key: &str, _instance: &str) -> Result<bool, String> {
        Err("creating a Secret is not available here".into())
    }
    /// Put a value into a Secret (replacing it) through a 0600 file; the value is shown nowhere.
    fn put_secret(&mut self, _ns: &str, _name: &str, _key: &str, _value: &str, _instance: &str) -> Result<(), String> {
        Err("creating a Secret is not available here".into())
    }
    /// `kubectl exec deploy/<deploy> -c <container> -- sh -c <script> [arg0]`: its output, or the plain reason it failed.
    fn pod_exec(&mut self, _ns: &str, _deploy: &str, _container: &str, _script: &str, _arg0: &str) -> Result<String, String> {
        Err("running a command in a pod is not available here".into())
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
              cpu: 250m
              memory: 1Gi
            limits:
              cpu: \"2\"
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

/// The namespace of the data services, labelled as IYAKKI's (applied before the Secret is made; the same text as in the Elasticsearch objects).
pub fn data_namespace_manifest() -> String {
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
",
        ns = NAMESPACE
    )
}

/// The graph database objects: a volume claim, one unprivileged Deployment and a ClusterIP Service. The password is NOT here: the pod reads
/// it from the Secret [`GRAPH_SECRET`] by reference. Non-root (7474), read-only root file system (the writable places are the volume and
/// `emptyDir`s), no capabilities, no privilege escalation, no service account token, no hostPath; telemetry off; the memory is cut to
/// 256 MB heap and 64 MB page cache, about 490 MiB measured idle, limit 1 GiB.
pub fn graphdb_manifest() -> String {
    format!(
        "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: graphdb-data
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
  name: graphdb
  namespace: {ns}
  labels:
    app.kubernetes.io/name: graphdb
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: graphdb
  template:
    metadata:
      labels:
        app.kubernetes.io/name: graphdb
        app.kubernetes.io/managed-by: iyakki
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 7474
        runAsGroup: 7474
        fsGroup: 7474
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: graphdb
          image: {img}
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: [ALL]
          env:
            - name: NEO4J_AUTH
              valueFrom:
                secretKeyRef:
                  name: {secret}
                  key: NEO4J_AUTH
            - name: NEO4J_server_memory_heap_initial__size
              value: 128m
            - name: NEO4J_server_memory_heap_max__size
              value: 256m
            - name: NEO4J_server_memory_pagecache_size
              value: 64m
            - name: NEO4J_dbms_usage__report_enabled
              value: \"false\"
          ports:
            - containerPort: 7474
            - containerPort: 7687
          resources:
            requests:
              cpu: 100m
              memory: 640Mi
            limits:
              cpu: \"2\"
              memory: 1Gi
          readinessProbe:
            httpGet:
              path: /
              port: 7474
            initialDelaySeconds: 20
            periodSeconds: 10
          volumeMounts:
            - name: data
              mountPath: /data
            - name: logs
              mountPath: /logs
            - name: tmp
              mountPath: /tmp
            - name: conf
              mountPath: /var/lib/neo4j/conf
            - name: run
              mountPath: /var/lib/neo4j/run
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: graphdb-data
        - name: logs
          emptyDir: {{}}
        - name: tmp
          emptyDir: {{}}
        - name: conf
          emptyDir: {{}}
        - name: run
          emptyDir: {{}}
---
apiVersion: v1
kind: Service
metadata:
  name: graphdb
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: graphdb
  ports:
    - name: http
      port: 7474
      targetPort: 7474
    - name: bolt
      port: 7687
      targetPort: 7687
",
        ns = NAMESPACE,
        img = GRAPH_IMAGE,
        secret = GRAPH_SECRET
    )
}

/// The DASH source of the in-cluster graph database: Cypher over the Query API at its stable cluster address, basic sign-in as
/// [`GRAPH_USER`] with the password taken from the person's vault entry [`GRAPH_VAULT_NAME`] (a reference, never the value).
pub fn graphdb_source() -> crate::dash::connector::SourceConfig {
    let mut values = std::collections::BTreeMap::new();
    values.insert("url".to_string(), GRAPH_DASH_URL.to_string());
    values.insert("auth".to_string(), "basic".to_string());
    values.insert("username".to_string(), GRAPH_USER.to_string());
    values.insert("database".to_string(), "neo4j".to_string());
    let mut secrets = std::collections::BTreeMap::new();
    secrets.insert("password".to_string(), format!("uvault:{GRAPH_VAULT_NAME}"));
    crate::dash::connector::SourceConfig { id: GRAPH_SOURCE_ID.into(), kind: "neo4j".into(), name: "Cluster graph database".into(), values, secrets }
}

/// A new password: 24 characters of letters and digits, drawn from the system's random source without bias.
pub fn new_graph_password() -> Result<zeroize::Zeroizing<String>, String> {
    const A: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = zeroize::Zeroizing::new(String::new());
    while out.len() < 24 {
        let mut b = zeroize::Zeroizing::new([0u8; 32]);
        iyakki_sec::random_fill(&mut b[..]).map_err(|e| format!("no random numbers for the password: {e}"))?;
        for x in b.iter() {
            if *x < 248 && out.len() < 24 {
                out.push(A[(*x % 62) as usize] as char);
            }
        }
    }
    Ok(out)
}

/// The shell's half of the password handoff: the job made the Secret and left a request; the shell, which holds the person's unlocked
/// vault, reads the password from the cluster and stores it as [`GRAPH_VAULT_NAME`]. Tries up to `tries` times, `wait` apart (the vault may
/// be locked for a moment, the cluster busy). Returns a plain line; neither line holds the password.
pub fn graph_vault_handoff(read: &dyn Fn() -> Result<zeroize::Zeroizing<String>, String>, store: &dyn Fn(&str) -> Result<(), String>, sleep: &dyn Fn(Duration), tries: u32, wait: Duration) -> Result<String, String> {
    vault_handoff(&VaultNames { what: "graph database", vault_name: GRAPH_VAULT_NAME, source: "Cluster graph database", secret: GRAPH_SECRET }, read, store, sleep, tries, wait)
}

/// What a password handoff names in its two result lines.
pub struct VaultNames {
    pub what: &'static str,
    pub vault_name: &'static str,
    pub source: &'static str,
    pub secret: &'static str,
}

/// The handoff of one service (see [`graph_vault_handoff`]).
pub fn vault_handoff(h: &VaultNames, read: &dyn Fn() -> Result<zeroize::Zeroizing<String>, String>, store: &dyn Fn(&str) -> Result<(), String>, sleep: &dyn Fn(Duration), tries: u32, wait: Duration) -> Result<String, String> {
    let mut last = String::from("not tried");
    for n in 0..tries.max(1) {
        if n > 0 {
            sleep(wait);
        }
        let pw = match read() {
            Ok(p) if !p.trim().is_empty() => p,
            Ok(_) => {
                last = "the Secret holds no password".into();
                continue;
            }
            Err(e) => {
                last = format!("could not read the password from the cluster: {e}");
                continue;
            }
        };
        match store(pw.trim()) {
            Ok(()) => return Ok(format!("the {} password is in your vault as {}; DASH uses it for the source \"{}\"", h.what, h.vault_name, h.source)),
            Err(e) => last = format!("could not put the password in your vault: {e}"),
        }
    }
    Err(format!("{last}. The database works; unlock your vault (sign in again) and press Install on the {} again, or read the password with kubectl -n {NAMESPACE} get secret {} and save it as {}", h.what, h.secret, h.vault_name))
}

/// The Prometheus objects: a namespace, a service account with a read-only cluster role, the scrape configuration, a volume claim, one
/// unprivileged Deployment (read-only root file system, no capabilities, limits set) and a ClusterIP Service. No hostPath, no NodePort.
pub fn prometheus_manifest() -> String {
    format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: prometheus
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: iyakki-prometheus
  labels:
    app.kubernetes.io/managed-by: iyakki
rules:
  - apiGroups: [\"\"]
    resources: [nodes, nodes/metrics, services, endpoints, pods]
    verbs: [get, list, watch]
  - nonResourceURLs: [/metrics]
    verbs: [get]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: iyakki-prometheus
  labels:
    app.kubernetes.io/managed-by: iyakki
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: iyakki-prometheus
subjects:
  - kind: ServiceAccount
    name: prometheus
    namespace: {ns}
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: prometheus-config
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
data:
  prometheus.yml: |
    global:
      scrape_interval: 30s
      evaluation_interval: 30s
    scrape_configs:
      - job_name: prometheus
        static_configs:
          - targets: [\"localhost:9090\"]
      - job_name: kubelet
        scheme: https
        authorization:
          credentials_file: /var/run/secrets/kubernetes.io/serviceaccount/token
        tls_config:
          ca_file: /var/run/secrets/kubernetes.io/serviceaccount/ca.crt
          insecure_skip_verify: true
        kubernetes_sd_configs:
          - role: node
      - job_name: cadvisor
        scheme: https
        metrics_path: /metrics/cadvisor
        authorization:
          credentials_file: /var/run/secrets/kubernetes.io/serviceaccount/token
        tls_config:
          ca_file: /var/run/secrets/kubernetes.io/serviceaccount/ca.crt
          insecure_skip_verify: true
        kubernetes_sd_configs:
          - role: node
      - job_name: pods
        kubernetes_sd_configs:
          - role: pod
        relabel_configs:
          - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_scrape]
            action: keep
            regex: \"true\"
          - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_path]
            action: replace
            target_label: __metrics_path__
            regex: (.+)
          - source_labels: [__address__, __meta_kubernetes_pod_annotation_prometheus_io_port]
            action: replace
            regex: ([^:]+)(?::\\d+)?;(\\d+)
            replacement: $1:$2
            target_label: __address__
          - source_labels: [__meta_kubernetes_namespace]
            target_label: namespace
          - source_labels: [__meta_kubernetes_pod_name]
            target_label: pod
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: prometheus-data
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
  name: prometheus
  namespace: {ns}
  labels:
    app.kubernetes.io/name: prometheus
    app.kubernetes.io/managed-by: iyakki
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      app.kubernetes.io/name: prometheus
  template:
    metadata:
      labels:
        app.kubernetes.io/name: prometheus
        app.kubernetes.io/managed-by: iyakki
    spec:
      serviceAccountName: prometheus
      securityContext:
        runAsNonRoot: true
        runAsUser: 65534
        runAsGroup: 65534
        fsGroup: 65534
      containers:
        - name: prometheus
          image: {img}
          args:
            - --config.file=/etc/prometheus/prometheus.yml
            - --storage.tsdb.path=/prometheus
            - --storage.tsdb.retention.time=7d
            - --storage.tsdb.retention.size=4GB
            - --web.listen-address=:9090
          ports:
            - containerPort: 9090
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: [ALL]
          resources:
            requests:
              cpu: 100m
              memory: 256Mi
            limits:
              cpu: 500m
              memory: 768Mi
          readinessProbe:
            httpGet:
              path: /-/ready
              port: 9090
            initialDelaySeconds: 10
            periodSeconds: 10
          volumeMounts:
            - name: config
              mountPath: /etc/prometheus
              readOnly: true
            - name: data
              mountPath: /prometheus
      volumes:
        - name: config
          configMap:
            name: prometheus-config
        - name: data
          persistentVolumeClaim:
            claimName: prometheus-data
---
apiVersion: v1
kind: Service
metadata:
  name: prometheus
  namespace: {ns}
  labels:
    app.kubernetes.io/managed-by: iyakki
spec:
  type: ClusterIP
  selector:
    app.kubernetes.io/name: prometheus
  ports:
    - port: 9090
      targetPort: 9090
",
        ns = PROM_NAMESPACE,
        img = PROM_IMAGE
    )
}

/// The DASH source of the in-cluster Prometheus, by its stable cluster address.
pub fn prometheus_source() -> crate::dash::connector::SourceConfig {
    let mut values = std::collections::BTreeMap::new();
    values.insert("url".to_string(), PROM_DASH_URL.to_string());
    crate::dash::connector::SourceConfig { id: PROM_SOURCE_ID.into(), kind: "prometheus".into(), name: "Cluster Prometheus".into(), values, secrets: Default::default() }
}

/// The DASH source of the in-cluster Elasticsearch (single node, no sign-in), by its stable cluster address.
pub fn elasticsearch_source() -> crate::dash::connector::SourceConfig {
    let mut values = std::collections::BTreeMap::new();
    values.insert("url".to_string(), ES_DASH_URL.to_string());
    crate::dash::connector::SourceConfig { id: ES_SOURCE_ID.into(), kind: "elasticsearch".into(), name: "Cluster Elasticsearch".into(), values, secrets: Default::default() }
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
              cpu: 500m
              memory: 1Gi
            limits:
              cpu: \"6\"
              memory: 16Gi
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
    // applying the manifest again removes the GPU settings a GPU step gave it: put them back (unless the person chose the CPU)
    crate::gpu_steps::reapply_lemonade_gpu(env);
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
                env.say("[1/3] k3s is installed already (IYAKKI's dev cluster)");
            } else {
                env.say("[0/3] looking up the latest k3s release and its checksum");
                let what = env.install_k3s()?;
                env.say(&format!("[1/3] installed: {what}"));
            }
            match env.link_k3s_tools() {
                Ok(l) if !l.is_empty() => env.say(&l),
                Ok(_) => {}
                Err(e) => env.say(&format!("note: {e}")),
            }
            env.request_k3s_start()?;
            env.say("[2/3] asked IYAKKI to start the cluster (Settings \u{2192} Dev cluster shows it; it also starts with IYAKKI from now on)");
            // installed is not running: the item is only marked once the cluster really answers as Ready
            env.say("[3/3] waiting for the cluster to be Ready");
            wait_for_cluster(env, wait.min(K3S_READY_WAIT), poll).map_err(|_| format!("k3s was installed, but the cluster did not become Ready within {} minutes; it is not marked as installed. See Settings \u{2192} Dev cluster for why it did not start, then press Retry", K3S_READY_WAIT.as_secs() / 60))?;
            mark_verified(env, Item::K3s, "installed by Quick install")?;
            Ok(())
        }
        Item::DuckDb => {
            env.say("[0/3] DuckDB is installed with k3s: checking the cluster first");
            wait_for_cluster(env, wait, poll)?;
            env.say("[1/3] k3s is Ready; installing the duckdb program from the CLI catalogue");
            let v = env.install_cli("duckdb")?;
            env.say(&format!("[2/3] duckdb {v} installed (the CLI tile lists it)"));
            mark_verified(env, Item::DuckDb, &format!("version {v}"))?;
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
            let detail = format!("http://elasticsearch.{NAMESPACE}.svc:9200 inside the cluster; DASH source \"Cluster Elasticsearch\" at {ES_DASH_URL}");
            match env.register_dash_source(elasticsearch_source()) {
                Ok(()) => env.say(&format!("  added to DASH as the source \"Cluster Elasticsearch\" ({ES_DASH_URL}: IYAKKI looks the Service up each time, nothing listens on localhost)")),
                Err(e) => env.say(&format!("  could not add it to DASH ({e}); add an Elasticsearch source with {ES_DASH_URL} yourself")),
            }
            mark_verified(env, Item::Elasticsearch, &detail)?;
            env.say(&format!("[4/4] done: {detail} (kubectl -n {NAMESPACE} port-forward svc/elasticsearch 9200 to reach it from a program of your own)"));
            Ok(())
        }
        Item::Prometheus => {
            env.say(&format!("[0/5] Prometheus runs in k3s: checking the cluster first (licence: {})", item.licence()));
            wait_for_cluster(env, wait, poll)?;
            env.say(&format!("[1/5] k3s is Ready; applying the Prometheus objects in namespace {PROM_NAMESPACE} (read-only access to the cluster, ClusterIP only)"));
            let out = env.kubectl_apply("prometheus", &prometheus_manifest())?;
            for l in out.lines().filter(|l| !l.trim().is_empty()).take(12) {
                env.say(&format!("  {}", l.trim()));
            }
            env.say(&format!("[2/5] waiting for the pod (the first start pulls {PROM_IMAGE} from quay.io, about 120 MB)"));
            env.rollout(PROM_NAMESPACE, "prometheus", Duration::from_secs(600))?;
            env.say("[3/5] Prometheus is running");
            let in_cluster = format!("http://prometheus.{PROM_NAMESPACE}.svc:9090");
            let detail = match env.register_dash_source(prometheus_source()) {
                Ok(()) => {
                    env.say(&format!("[4/5] added to DASH as the source \"Cluster Prometheus\" ({PROM_DASH_URL}: IYAKKI looks the Service up each time)"));
                    format!("{in_cluster} inside the cluster; DASH source at {PROM_DASH_URL}")
                }
                Err(e) => {
                    env.say(&format!("[4/5] could not add it to DASH ({e}); add a Prometheus source with {PROM_DASH_URL} yourself"));
                    format!("{in_cluster} inside the cluster")
                }
            };
            mark_verified(env, Item::Prometheus, &detail)?;
            env.say(&format!("[5/5] done: {detail}"));
            Ok(())
        }
        Item::GraphDb => {
            env.say(&format!("[0/7] the graph database runs in k3s: checking the cluster first (licence: {})", item.licence()));
            wait_for_cluster(env, wait, poll)?;
            env.say(&format!("[1/7] k3s is Ready; making the namespace {NAMESPACE}"));
            let out = env.kubectl_apply("data-namespace", &data_namespace_manifest())?;
            for l in out.lines().filter(|l| !l.trim().is_empty()).take(4) {
                env.say(&format!("  {}", l.trim()));
            }
            match env.graph_secret()? {
                true => env.say(&format!("[2/7] made the administrator password and the Secret {GRAPH_SECRET} (the password is not shown; it goes into your vault as {GRAPH_VAULT_NAME})")),
                false => env.say(&format!("[2/7] the Secret {GRAPH_SECRET} is there already; its password is kept")),
            }
            env.say("[3/7] applying the graph database objects (unprivileged, ClusterIP only, 5 GB volume)");
            let out = env.kubectl_apply("graphdb", &graphdb_manifest())?;
            for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
                env.say(&format!("  {}", l.trim()));
            }
            env.say("[4/7] waiting for the pod (the first start pulls the Neo4j image, about 190 MB)");
            env.rollout(NAMESPACE, "graphdb", Duration::from_secs(900))?;
            env.say("[5/7] the graph database is running");
            let in_cluster = format!("http://graphdb.{NAMESPACE}.svc:7474");
            let detail = match env.register_dash_source(graphdb_source()) {
                Ok(()) => {
                    env.say(&format!("[6/7] added to DASH as the source \"Cluster graph database\" ({GRAPH_DASH_URL}; every query is a read transaction)"));
                    format!("{in_cluster} inside the cluster; DASH source at {GRAPH_DASH_URL}")
                }
                Err(e) => {
                    env.say(&format!("[6/7] could not add it to DASH ({e}); add a Neo4j source with {GRAPH_DASH_URL} yourself"));
                    format!("{in_cluster} inside the cluster")
                }
            };
            env.request_after("graph-vault")?;
            mark_verified(env, Item::GraphDb, &detail)?;
            env.say(&format!("[7/7] done: {detail}. IYAKKI is putting the password into your vault as {GRAPH_VAULT_NAME}"));
            Ok(())
        }
        Item::ClickHouse => clickhouse::run(env, wait, poll),
        Item::OpenSearch => opensearch::run(env, wait, poll),
        Item::Gitea => gitea::run(env, wait, poll),
        Item::Matrix => matrix::run(env, wait, poll),
        Item::SeaweedFs => objstore::run(&objstore::SEAWEEDFS, env, wait, poll),
        Item::RustFs => objstore::run(&objstore::RUSTFS, env, wait, poll),
        Item::Ollama => ollama_cluster::run(env, wait, poll),
        Item::Node | Item::Python | Item::ClaudeCode | Item::Copilot | Item::Ffmpeg => {
            let id = item.cli_id().unwrap_or_default();
            let server = false;
            let total = if server { 3 } else { 2 };
            env.say(&format!("[0/{total}] installing {} from the CLI catalogue (licence: {})", item.title(), item.licence()));
            let v = env.install_cli(id)?;
            env.say(&format!("[1/{total}] {} {v} installed (the CLI tile lists it)", item.title()));
            if server {
                env.request_after(id)?;
                env.say(&format!("[2/{total}] asked IYAKKI to start it and add it to Settings \u{2192} Models"));
            }
            mark_verified(env, item, &format!("version {v}"))?;
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
        Item::FfmpegLibs => {
            if env.av_build_requested() {
                // built here (k3s): the package goes to /local/packages and the person installs it through the usual review and confirm
                let what = env.build_av_addon()?;
                env.say(&what);
                return Ok(());
            }
            env.say("[0/2] checking the add-on libraries you supplied (a package in /local/packages, a mirror, or your installed ffmpeg)");
            let what = env.install_av_addon()?;
            env.say(&format!("[1/2] installed: {what}"));
            mark_verified(env, item, &what)?;
            env.say("[2/2] done: the BROWSER tile starts its engine again and plays H.264 and AAC (no FFmpeg is in the image)");
            Ok(())
        }
        Item::Lemonade => {
            env.say(&format!("[0/4] Lemonade runs in k3s: checking the cluster first (licence: {})", item.licence()));
            deploy_lemonade(env, wait, poll)?;
            env.say("[1/4] the Lemonade pod is ready");
            env.request_after("lemonade")?;
            env.say("[2/4] asked IYAKKI to add it to Settings \u{2192} Models");
            let detail = format!("http://127.0.0.1:{} on this machine; lemonade.{LEMONADE_NAMESPACE}.svc inside the cluster", crate::lemonade::DEFAULT_PORT);
            mark_verified(env, Item::Lemonade, &detail)?;
            env.say(&format!("[3/4] done: {detail}"));
            env.say("[4/4] models are pulled later from Settings \u{2192} Models (kept in the cluster's volume, 30 GB claimed)");
            Ok(())
        }
        Item::Kokoro => {
            env.say(&format!("[0/3] Kokoro reads through Lemonade: making sure Lemonade runs in k3s (licence: {})", item.licence()));
            ensure_lemonade(env, wait, poll)?;
            env.say("[1/3] Lemonade answers; pulling kokoro-v1 (about 330 MB)");
            env.lemonade_pull("kokoro-v1")?;
            mark_verified(env, Item::Kokoro, "kokoro-v1 in Lemonade")?;
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
            mark_verified(env, Item::Registry, &format!("pods pull {pull}"))?;
            env.say(&format!("done: the registry is ready; the private store deploy now pushes its image there (pods pull {pull})"));
            Ok(())
        }
        Item::GpuVulkan => crate::gpu_steps::run_vulkan(env, wait, poll),
        Item::GpuNvidia => crate::gpu_steps::run_nvidia(env, wait, poll),
        Item::MacFirmware => {
            let line = env.mac_firmware()?;
            mark_verified(env, Item::MacFirmware, &line)?;
            Ok(())
        }
        Item::FwHardware => Err("the hardware firmware has its own job: `iyakki quick-install fw-hardware`".into()),
        Item::MacCamera => Err("the camera firmware has its own job: `iyakki quick-install mac-camera`".into()),
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => Err(format!("this firmware pack has its own job: `iyakki quick-install {}`", item.id())),
        Item::MacHost => Err("the Mac model host is a guide: open it from Store \u{2192} Setup or Settings \u{2192} Models".into()),
        Item::LinuxApps => {
            env.say(crate::linuxapps_dist::LICENCE_NOTE);
            let line = env.linux_apps()?;
            mark_verified(env, Item::LinuxApps, &line)?;
            Ok(())
        }
        Item::LinuxWindows => {
            env.say(crate::linuxwin::NOTE);
            let line = env.linux_windows()?;
            mark_verified(env, Item::LinuxWindows, &line)?;
            Ok(())
        }
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
    /// What the check before the marker saw (written as the marker's third line by the next `mark`).
    verified_note: String,
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
        let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = iyakki_proxy::https::download_streamer();
        clis.set_streamer(streamer.clone());
        let k3s = crate::k3sdev::K3sDev::new(data, clis.clone(), std::sync::Arc::new(crate::wsexec::System));
        RealEnv { data: data.to_path_buf(), user: user.into(), k3s, clis, streamer, registry: RegistryArgs::default(), mac_source: crate::macfw::Source::Download, verified_note: String::new() }
    }

    /// `kubectl create secret generic` from 0600 files in a 0700 folder (one per `(key, value)`), removed at once, then the labels. The
    /// values are in no argument, no log and no manifest; `redact` are scrubbed from any error text.
    fn secret_from_values(&mut self, ns: &str, name: &str, values: &[(&str, &str)], labels: &[(&str, &str)], redact: &[&str]) -> Result<(), String> {
        // one folder per call: installs run side by side, and each removes its own folder when it ends
        static SECRET_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = state_dir(&self.data).join("secret-tmp").join(format!("{}-{}", std::process::id(), SECRET_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let scrub = |t: String| redact.iter().fold(t, |t, r| if r.is_empty() { t } else { t.replace(r, "[redacted]") });
        let result = (|| {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            let mut froms: Vec<String> = Vec::new();
            for (i, (key, value)) in values.iter().enumerate() {
                let p = dir.join(format!("v{i}"));
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                f.write_all(value.as_bytes()).map_err(|e| e.to_string())?;
                froms.push(format!("--from-file={key}={}", p.display()));
            }
            let mut args: Vec<&str> = vec!["-n", ns, "create", "secret", "generic", name];
            args.extend(froms.iter().map(String::as_str));
            let out = self.k3s.kubectl(&args);
            if out.code != Some(0) {
                return Err(format!("kubectl create secret failed: {}", scrub(out.combined()).trim()));
            }
            if !labels.is_empty() {
                let l: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
                let mut la: Vec<&str> = vec!["-n", ns, "label", "secret", name, "--overwrite"];
                la.extend(l.iter().map(String::as_str));
                let out = self.k3s.kubectl(&la);
                if out.code != Some(0) {
                    return Err(format!("created the Secret {name} but could not label it: {}", out.combined().trim()));
                }
            }
            Ok(())
        })();
        // the files hold the values: gone whatever happened
        let _ = std::fs::remove_dir_all(&dir);
        result
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
            Some(p) => crate::qprintln!("NVIDIA publishes the checksum {p}"),
            None => crate::qprintln!("note: NVIDIA's checksum file could not be fetched; the checksum pinned in IYAKKI still has to match"),
        }
        let mut last = u64::MAX;
        crate::gpu_nvidia::download_run(&*self.streamer, &l.run(), published.as_deref(), &mut |d, t| {
            let pct = if t > 0 { d * 100 / t } else { 0 };
            if pct != last {
                crate::qprintln!("downloading {} MB of {} MB ({pct}%)", d >> 20, t >> 20);
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
        let s = la.create(&req, &*self.streamer, &|_, _| Ok(()), &mut |l| crate::qprintln!("{l}"))?;
        Ok(format!("sandbox {} ({}) is ready", s.name, s.title()))
    }
    fn linux_windows(&mut self) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        let la = crate::linuxapps::LinuxApps::new(&self.data, &self.user, &exe, std::sync::Arc::new(crate::wsexec::System));
        let streamer = self.streamer.clone();
        crate::linuxapps::stream(|sink| crate::linuxwin::install(&la, &*streamer, Some(sink), &mut |l| crate::qprintln!("{l}")))
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
                            crate::qprintln!("{} {pct}%", p.phase);
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
    fn av_build_requested(&mut self) -> bool {
        crate::avbuild::take_request(&self.data)
    }
    fn build_av_addon(&mut self) -> Result<String, String> {
        let k3s = self.k3s.clone();
        let kube = move |a: &[&str]| {
            let o = k3s.kubectl(a);
            (o.code == Some(0), o.combined())
        };
        let map = std::fs::read_to_string(std::path::Path::new(crate::avbuild::BRIDGE_DIR).join("versions.map")).unwrap_or_default();
        let lines = std::cell::RefCell::new(Vec::<String>::new());
        let (sleep, say) = (|d: Duration| std::thread::sleep(d), |l: &str| lines.borrow_mut().push(l.to_string()));
        let cx = crate::avbuild::RunCx { data: &self.data, bridge: std::path::Path::new(crate::avbuild::BRIDGE_DIR), map_text: &map, version: crate::avbuild::IMAGE_VERSION, arch: std::env::consts::ARCH, kubectl: &kube, sleep: &sleep, say: &say, max_polls: 760 };
        let r = crate::avbuild::build(&cx);
        let progress = lines.borrow().join("\n");
        r.map(|m| format!("{progress}\n{m}"))
    }
    fn install_av_addon(&mut self) -> Result<String, String> {
        let places = crate::avaddon_install::Places::here(&self.data);
        let clis = self.clis.clone();
        crate::avaddon_install::run(&places, true, &|url, file| clis.fetch_admin_file(url, file))
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
    fn register_dash_source(&mut self, cfg: crate::dash::connector::SourceConfig) -> Result<(), String> {
        crate::dash::store::Sources::open(&self.data, &self.user).save(cfg)
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        let dir = state_dir(&self.data);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{name}.yaml"));
        let k3s = &self.k3s;
        let kube = |a: &[&str]| {
            let o = k3s.kubectl(a);
            crate::pvcguard::KOut { ok: o.code == Some(0), stdout: o.stdout.clone(), text: o.combined() }
        };
        crate::pvcguard::guarded_apply(manifest, None, &kube, &|m: &str| {
            std::fs::write(&file, m).map_err(|e| e.to_string())?;
            let out = k3s.kubectl(&["apply", "-f", &file.display().to_string()]);
            if out.code == Some(0) { Ok(out.stdout) } else { Err(format!("kubectl apply failed: {}", out.combined().trim())) }
        })
    }
    fn graph_secret(&mut self) -> Result<bool, String> {
        let there = self.k3s.kubectl(&["-n", NAMESPACE, "get", "secret", GRAPH_SECRET, "-o", "name"]);
        if there.code == Some(0) {
            return Ok(false);
        }
        let pw = new_graph_password()?;
        let auth = zeroize::Zeroizing::new(format!("{GRAPH_USER}/{}", pw.as_str()));
        self.secret_from_values(NAMESPACE, GRAPH_SECRET, &[("NEO4J_AUTH", &auth), ("password", &pw)], &[], &[pw.as_str()])?;
        Ok(true)
    }
    fn clickhouse_secret(&mut self) -> Result<bool, String> {
        let there = self.k3s.kubectl(&["-n", NAMESPACE, "get", "secret", clickhouse::SECRET, "-o", "name"]);
        if there.code == Some(0) {
            return Ok(false);
        }
        let pw = new_graph_password()?;
        self.secret_from_values(NAMESPACE, clickhouse::SECRET, &[("CLICKHOUSE_PASSWORD", &pw), ("password", &pw)], &[], &[pw.as_str()])?;
        Ok(true)
    }
    fn objstore_secret(&mut self, e: &objstore::Engine) -> Result<bool, String> {
        let there = self.k3s.kubectl(&["-n", objstore::NS, "get", "secret", &e.secret(), "-o", "name"]);
        if there.code == Some(0) {
            return Ok(false);
        }
        let (a, s) = objstore::new_keys()?;
        let vals = e.secret_values(&a, &s);
        let refs: Vec<(&str, &str)> = vals.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.secret_from_values(objstore::NS, &e.secret(), &refs, &[], &[a.as_str(), s.as_str(), vals.last().map(|v| v.1.as_str()).unwrap_or("")])?;
        Ok(true)
    }
    fn image_id(&mut self, ns: &str, deploy: &str) -> Option<String> {
        let o = self.k3s.kubectl(&["-n", ns, "get", "pods", "-l", &format!("app.kubernetes.io/name={deploy}"), "-o", "jsonpath={.items[0].status.containerStatuses[0].imageID}"]);
        let t = o.stdout.trim().to_string();
        if o.code == Some(0) && !t.is_empty() { Some(t) } else { None }
    }
    fn matrix_secret(&mut self) -> Result<bool, String> {
        let there = self.k3s.kubectl(&["-n", matrix::NS, "get", "secret", matrix::SECRET, "-o", "name"]);
        if there.code == Some(0) {
            return Ok(false);
        }
        let t = new_graph_password()?;
        self.secret_from_values(matrix::NS, matrix::SECRET, &[("TUWUNEL_REGISTRATION_TOKEN", &t), ("token", &t)], &[], &[t.as_str()])?;
        Ok(true)
    }
    fn cluster_ref(&mut self) -> Result<crate::platform::installed::ClusterRef, String> {
        let run: std::sync::Arc<dyn crate::wsexec::Runner> = std::sync::Arc::new(crate::wsexec::System);
        let p = crate::devcluster::inspect(&run, &self.k3s.layout().kubeconfig(), crate::devcluster::Source::K3s, "default", false);
        let mut c = crate::platform::installed::ClusterRef::of(&p);
        if c.name.is_empty() {
            c.name = "k3s".into();
        }
        Ok(c)
    }
    fn service_records(&mut self) -> Vec<crate::platform::installed::Installed> {
        crate::platform::installed::load(&self.data, &self.user)
    }
    fn save_service(&mut self, rec: crate::platform::installed::Installed) -> Result<(), String> {
        crate::platform::installed::upsert(&self.data, &self.user, rec)
    }
    fn port_free(&mut self, port: u16) -> bool {
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
    fn live_gitea(&mut self) -> Option<gitea::LiveGitea> {
        let ns = gitea::NAMESPACE;
        let there = |kind: &str, name: &str| self.k3s.kubectl(&["-n", ns, "get", kind, name, "-o", "name"]).code == Some(0);
        if !there("deployment", "gitea") || !there("pvc", "gitea-data") {
            return None;
        }
        let svc = |name: &str| {
            let o = self.k3s.kubectl(&["-n", ns, "get", "service", name, "-o", "jsonpath={.spec.type} {.spec.ports[*].nodePort}"]);
            if o.code != Some(0) {
                return (String::new(), 0u16);
            }
            let mut w = o.stdout.split_whitespace();
            (w.next().unwrap_or("").to_string(), w.next().and_then(|p| p.parse().ok()).unwrap_or(0))
        };
        let (http_type, http_node_port) = svc("gitea-http");
        let (_, ssh_node_port) = svc("gitea-ssh");
        Some(gitea::LiveGitea { exposure: http_type, http_node_port, ssh_node_port })
    }
    fn http_status(&mut self, url: &str) -> Result<u16, String> {
        let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(5));
        iyakki_proxy::Upstream::request(&up, "GET", url, &[], &[]).map(|r| r.status)
    }
    fn generated_secret(&mut self, ns: &str, name: &str, key: &str, instance: &str) -> Result<bool, String> {
        let there = self.k3s.kubectl(&["-n", ns, "get", "secret", name, "-o", "name"]);
        if there.code == Some(0) {
            return Ok(false);
        }
        let pw = new_graph_password()?;
        self.secret_from_values(ns, name, &[(key, &pw)], &[("iyakki.dev/instance", instance)], &[pw.as_str()])?;
        Ok(true)
    }
    fn put_secret(&mut self, ns: &str, name: &str, key: &str, value: &str, instance: &str) -> Result<(), String> {
        let _ = self.k3s.kubectl(&["-n", ns, "delete", "secret", name, "--ignore-not-found"]);
        self.secret_from_values(ns, name, &[(key, value)], &[("iyakki.dev/instance", instance)], &[value])
    }
    fn pod_exec(&mut self, ns: &str, deploy: &str, container: &str, script: &str, arg0: &str) -> Result<String, String> {
        let target = format!("deploy/{deploy}");
        let mut args: Vec<&str> = vec!["-n", ns, "exec", &target, "-c", container, "--", "sh", "-c", script];
        if !arg0.is_empty() {
            args.push(arg0);
        }
        let out = self.k3s.kubectl(&args);
        if out.code == Some(0) { Ok(out.stdout) } else { Err(out.combined().trim().to_string()) }
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
        let note = std::mem::take(&mut self.verified_note);
        mark_done_note(&self.data, item, detail, &note)
    }
    fn set_verified_note(&mut self, note: &str) {
        self.verified_note = note.to_string();
    }
    fn verify(&mut self, item: Item, detail: &str) -> Result<String, String> {
        // a program of the CLI catalogue: it runs and says its version
        // (Ollama in the cluster is a Deployment below; Ollama on the host is a program like the others)
        if let Some(id) = item.cli_id().filter(|_| item != Item::Ollama || !ollama_cluster::is_cluster_detail(detail)) {
            return self.clis.verify_runs(id).map(|l| format!("verified: `{id}` ran and said \"{l}\""));
        }
        // a Deployment: all its pods Ready and its Service has an address behind it (the pods' readiness probes ask the program itself)
        if let Some((ns, deploy, svc)) = verify::target(item) {
            let d = self.k3s.kubectl(&["-n", ns, "get", "deployment", deploy, "-o", "jsonpath={.spec.replicas} {.status.readyReplicas}"]);
            if d.code != Some(0) {
                return Err(format!("{deploy} is not in namespace {ns}: {}", d.combined().trim()));
            }
            let (_, pods) = verify::ready_from(&d.stdout).map_err(|e| format!("{deploy}: {e}"))?;
            let e = self.k3s.kubectl(&["-n", ns, "get", "endpoints", svc, "-o", "jsonpath={.subsets[*].addresses[*].ip}"]);
            let n = verify::endpoints_from(e.code == Some(0), &e.stdout).map_err(|e| format!("{svc}: {e}"))?;
            return Ok(verify::deployment_note(deploy, svc, pods, n));
        }
        match item {
            // a model: Lemonade itself lists it as downloaded
            Item::Kokoro => {
                let have = self.lemonade_models()?;
                if have.iter().any(|m| m == "kokoro-v1") { Ok("verified: Lemonade lists kokoro-v1 as downloaded".into()) } else { Err("Lemonade does not list kokoro-v1 as downloaded".into()) }
            }
            // the add-on: its libraries pass the check again where they are installed, and the source is named
            Item::FfmpegLibs => crate::avaddon_install::verify_installed(&self.data, &crate::avaddon_install::Places::here(&self.data).sonames, detail),
            // a GPU: the node still reports the resource pods ask for
            Item::GpuVulkan | Item::GpuNvidia => {
                let res = if item == Item::GpuVulkan { crate::gpu_workloads::GPU_RESOURCE } else { crate::gpu_workloads::NVIDIA_RESOURCE };
                let o = self.k3s.kubectl(&["get", "nodes", "-o", "json"]);
                let n = crate::gpu_workloads::node_resource_count(&o.stdout, res);
                if n > 0 { Ok(format!("verified: the node reports {res} = {n}")) } else { Err(format!("the node does not report {res}")) }
            }
            _ => Ok(String::new()),
        }
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
    fn ollama_mode(&mut self) -> Option<ollama_cluster::Mode> {
        ollama_cluster::take_mode(&self.data)
    }
    fn ollama_host(&mut self) -> ollama_cluster::HostInfo {
        ollama_cluster::real_host_info(&self.data)
    }
    fn ollama_copy(&mut self, dir: &Path) -> Result<String, String> {
        let sel = format!("app.kubernetes.io/name={}", ollama_cluster::NAME);
        let pod = self.k3s.kubectl(&["-n", ollama_cluster::NS, "get", "pods", "-l", &sel, "-o", "jsonpath={.items[0].metadata.name}"]);
        let pod = pod.stdout.trim().to_string();
        if pod.is_empty() {
            return Err("the Ollama pod was not found, so nothing was copied".into());
        }
        // kubectl cp packs the folder itself and unpacks it with the pod's tar; the host folder is only read
        let from = format!("{}/.", dir.display());
        let to = format!("{}/{pod}:/data/models", ollama_cluster::NS);
        let out = self.k3s.kubectl(&["cp", &from, &to, "-c", ollama_cluster::NAME]);
        if out.code == Some(0) { Ok("the models were copied".into()) } else { Err(format!("the copy failed ({}); the host Ollama and its models were not touched", out.combined().trim().lines().last().unwrap_or("kubectl cp failed"))) }
    }
    fn ollama_models(&mut self, cluster: bool) -> Result<Vec<String>, String> {
        let port = if cluster { ollama_cluster::LOCAL_PORT } else { crate::ollama::DEFAULT_PORT };
        crate::ollama::list_models(port)
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

/// The longest one install step may run before its job gives up (the step's own waits are shorter; this catches what hangs without a word).
pub fn step_deadline(item: Item) -> Duration {
    match item {
        Item::GpuNvidia | Item::FfmpegLibs | Item::LinuxWindows | Item::LinuxApps | Item::Lemonade | Item::Kokoro | Item::VoicePack | Item::Ollama | Item::MacFirmware => Duration::from_secs(4 * 3600),
        _ => Duration::from_secs(2 * 3600),
    }
}

/// A timer that calls `on_expire` with the reason when the step is still running at its deadline; dropping it (the step ended) cancels it.
pub struct Watchdog {
    done: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl Watchdog {
    pub fn start(what: &str, deadline: Duration, on_expire: impl FnOnce(String) + Send + 'static) -> Watchdog {
        let done = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let d2 = done.clone();
        let msg = format!("{what} timed out: it ran for {} minutes without finishing (it is not marked as installed; see the log for the last step, then press Retry)", deadline.as_secs() / 60);
        std::thread::spawn(move || {
            let (lock, cv) = &*d2;
            let g = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (g, _) = cv.wait_timeout_while(g, deadline, |finished| !*finished).unwrap_or_else(|e| e.into_inner());
            if !*g {
                drop(g);
                on_expire(msg);
            }
        });
        Watchdog { done }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let (lock, cv) = &*self.done;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

/// Forget that the registry step finished (after the registry was removed).
pub fn unmark(data: &Path, item: Item) {
    let _ = std::fs::remove_file(marker(data, item));
    clear_failure(data, item);
}

/// `iyakki quick-install <k3s|duckdb|elasticsearch> [--data DIR] [--user NAME]`: one step, as the job runs it. Returns the exit code.
pub fn cli_main(args: &[String]) -> i32 {
    // the GPU page's test prompt: not an installable item
    if args.first().is_some_and(|a| a == "gpu-test") {
        return gpu_test_main(args);
    }
    // the NVIDIA card off and on (power saving): not an installable item
    if args.first().is_some_and(|a| a == "gpu-nvidia-sleep" || a == "gpu-nvidia-wake") {
        let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
        let user = flag("--user").or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "system".into());
        let mut env = RealEnv::new(&data, &user);
        return match crate::gpu_steps::run_nvidia_power(&mut env, args[0] == "gpu-nvidia-sleep", WAIT_FOR_CLUSTER, Duration::from_secs(5)) {
            Ok(()) => 0,
            Err(e) => {
                crate::qprintln!("error: {e}");
                1
            }
        };
    }
    // passwords waiting for the vault: say which (names only). The running shell saves them by itself as soon as the person's vault is
    // open (sign in with the password, or Settings > Vault > Unlock); this process holds no vault.
    if args.first().is_some_and(|a| a == "--finish-vault") {
        let data = PathBuf::from(args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "/var/lib/iyakki".into()));
        let waiting = crate::vaultpending::list(&data);
        if waiting.is_empty() {
            crate::qprintln!("no password is waiting for a vault");
        }
        for p in &waiting {
            crate::qprintln!("{} (for {}): {}; it will be saved as {} from the cluster Secret {}", p.item, p.user, crate::vaultpending::waiting_words(p), p.vault, p.secret);
        }
        return 0;
    }
    // the camera firmware: not an installable item either
    if args.first().is_some_and(|a| a == CAMERA_JOB) {
        return camera_firmware_main(args);
    }
    let Some(item) = args.first().and_then(|a| Item::parse(a)) else {
        crate::qeprintln!("usage: iyakki quick-install <k3s|duckdb|elasticsearch|clickhouse|graph-db|gitea|matrix> [--data DIR] [--user NAME]");
        return 2;
    };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let user = flag("--user").or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "system".into());
    // a step that neither finishes nor fails ends by itself: exit 124, "timed out", never marked as installed
    let watchdog_data = data.clone();
    let _watchdog = Watchdog::start(item.title(), step_deadline(item), move |why| {
        failed_exit(&watchdog_data, item, &why);
        std::process::exit(124);
    });
    if item.fw_def().is_some() {
        return firmware_pack_main(args, item, &data, &user);
    }
    if item == Item::FwHardware {
        return crate::fwextra_cli::hardware_main(args, &data, &user);
    }
    let mut env = RealEnv::new(&data, &user);
    if item == Item::Registry {
        env.registry = match registry_args(args) {
            Ok(a) => a,
            Err(e) => {
                crate::qprintln!("error: {e}");
                return 2;
            }
        };
    }
    if item == Item::MacFirmware {
        env.mac_source = match crate::macfw::Source::from_args(args) {
            Ok(s) => s,
            Err(e) => {
                crate::qprintln!("error: {e}");
                return 2;
            }
        };
    }
    match run_step(item, &mut env, WAIT_FOR_CLUSTER, Duration::from_secs(5)) {
        Ok(()) => {
            // the data must outlive its claim: set the bound volumes to Retain (idempotent; the page refresh does it too)
            let claims = uninstall::claims_of(item);
            if !claims.is_empty() {
                let mut k = |a: &[&str]| {
                    let o = env.k3s.kubectl(a);
                    if o.code == Some(0) { Ok(o.stdout) } else { Err(o.combined()) }
                };
                for pv in uninstall::retain_claims(&mut k, &claims) {
                    crate::qprintln!("the volume {pv} is set to Retain: its data survives the claim");
                }
            }
            clear_failure(&data, item);
            0
        }
        Err(e) => failed_exit(&data, item, &e),
    }
}

/// A step ended with an error (or timed out): print the failure line with its cause class first ("Network problem: ...") as the job's
/// last line, and remember it so a card cannot say Installed from an older marker. Returns the exit code 1.
pub fn failed_exit(data: &Path, item: Item, err: &str) -> i32 {
    let line = quickinstall_cause::failure_line(err);
    crate::qprintln!("error: {line}");
    record_failure(data, item, &line);
    1
}

/// `iyakki quick-install fw-<name> [--data DIR] [--from FOLDER] [--accept-licence] [--force-hardware] [--user NAME]`: a restricted firmware
/// pack, its log is what the person reads. `--accept-licence` prints the vendor's licence note and records the acceptance for `--user`
/// (the Setup tile does the same with a click, and writes the audit log line); without an acceptance the job installs nothing.
fn firmware_pack_main(args: &[String], item: Item, data: &Path, user: &str) -> i32 {
    let Some(def) = item.fw_def() else { return 2 };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--accept-licence") {
        crate::qprintln!("licence of {}: {}", def.title, def.licence_note);
        if let Err(e) = crate::fwpack::accept(data, def, &format!("cli:{user}"), crate::clock::now_unix()) {
            crate::qprintln!("error: {e}");
            return 1;
        }
        crate::qprintln!("accepted by cli:{user} (recorded in {})", crate::fwpack::work_dir(data, def).display());
    }
    let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = iyakki_proxy::https::download_streamer();
    let mut env = crate::fwpack::RealEnv::new(data, streamer);
    let force = args.iter().any(|a| a == "--force-hardware");
    let r = match flag("--from") {
        Some(f) => crate::fwpack::run_folder(&mut env, data, def, force, Path::new(&f)),
        None => crate::fwpack::run(&mut env, data, def, force),
    };
    match r {
        Ok(_) => {
            clear_failure(data, item);
            0
        }
        Err(e) => failed_exit(data, item, &e),
    }
}

/// `iyakki quick-install mac-camera [--data DIR]`: the camera firmware job, its log is what the person reads.
fn camera_firmware_main(args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let streamer: std::sync::Arc<dyn iyakki_proxy::Streamer> = iyakki_proxy::https::download_streamer();
    let mut env = crate::camfw::RealEnv::new(&data, streamer);
    match crate::camfw::run(&mut env, &data) {
        Ok(_) => {
            clear_failure(&data, Item::MacCamera);
            0
        }
        Err(e) => failed_exit(&data, Item::MacCamera, &e),
    }
}

/// `iyakki quick-install gpu-test [--data DIR]`: the test prompt of the GPU page, as a job (its log is the result).
fn gpu_test_main(args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let bases: Vec<String> = crate::gpu_steps::test_endpoints(&crate::lemonade::local_ports().iter().filter_map(|p| crate::lemonade::probe_local(*p).map(|b| b.0)).collect::<Vec<_>>());
    let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(600));
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    match crate::gpu_steps::run_test(&data, &bases, &up, &now, &mut |l| crate::qprintln!("{l}")) {
        Ok(_) => 0,
        Err(e) => {
            crate::qprintln!("error: {e}");
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

#[cfg(test)]
#[path = "quickinstall_prometheus_tests.rs"]
mod prometheus_tests;

#[cfg(test)]
#[path = "quickinstall_graph_tests.rs"]
mod graph_tests;

/// The ClickHouse item: one pod and a ClusterIP Service in `iyakki-data`, a generated password, the shared DASH source `clickhouse-cluster`.
#[path = "quickinstall_clickhouse.rs"]
pub mod clickhouse;
/// The OpenSearch item: one pod and a ClusterIP Service in `iyakki-data`, the shared DASH source `opensearch-cluster` (Elasticsearch connector).
#[path = "quickinstall_opensearch.rs"]
pub mod opensearch;
#[cfg(test)]
#[path = "quickinstall_opensearch_tests.rs"]
mod opensearch_tests;
/// The object storage items (SeaweedFS and RustFS): S3 in the cluster, keys in the vault, a typed connection record.
#[path = "quickinstall_objstore.rs"]
pub mod objstore;

#[path = "quickinstall_uninstall.rs"]
pub mod uninstall;
#[cfg(test)]
#[path = "quickinstall_objstore_tests.rs"]
mod objstore_tests;
/// The Matrix server item: Tuwunel in its own namespace, a generated registration token for the vault, federation off.
#[path = "quickinstall_matrix.rs"]
pub mod matrix;
#[cfg(test)]
#[path = "quickinstall_matrix_tests.rs"]
mod matrix_tests;

#[cfg(test)]
#[path = "quickinstall_clickhouse_tests.rs"]
mod clickhouse_tests;

/// What each cluster item may use (CPU and memory requests and limits, the idle profile) and the review's memory warning.
#[path = "quickinstall_resources.rs"]
pub mod resources;

/// Ollama as a pod in the cluster (like Lemonade), the move of a host install, and where Ollama runs.
#[path = "quickinstall_ollama.rs"]
pub mod ollama_cluster;

#[cfg(test)]
#[path = "quickinstall_ollama_tests.rs"]
mod ollama_cluster_tests;

#[cfg(test)]
#[path = "quickinstall_resources_tests.rs"]
mod resources_tests;

/// The Gitea item: Platform services' Gitea objects deployed by a Quick install job, with the vault handoff and the Git connection.
#[path = "quickinstall_gitea.rs"]
pub mod gitea;

#[cfg(test)]
#[path = "quickinstall_gitea_tests.rs"]
mod gitea_tests;

/// The cause class of a failed step (network problem, timed out, disk full, licence needed, dependency failed, cluster not ready, verify failed).
#[path = "quickinstall_cause.rs"]
pub mod quickinstall_cause;

#[cfg(test)]
#[path = "quickinstall_cause_tests.rs"]
mod cause_tests;

#[cfg(test)]
#[path = "quickinstall_verify_tests.rs"]
mod verify_tests;

/// Which Deployment and Service an item is, and the plain readings of the checks that run before a marker is written.
#[path = "quickinstall_verify.rs"]
pub mod verify;
