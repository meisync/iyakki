//! Types moved out of `iyakki_agent::clis` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What was found for a tool that comes with the system.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SystemStatus {
    pub found: bool,
    /// `OpenSSH 9.6p1`, `Dropbear 2026.91`, or why nothing was found.
    pub label: String,
    pub path: String,
    /// The commands of the tool that exist here.
    pub commands: Vec<String>,
    /// How to get it, for this operating system.
    pub how_to: String,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use sha2::{Digest, Sha256};

/// Largest download accepted.
pub const MAX_DOWNLOAD: u64 = 900 << 20;

#[derive(Clone, Copy, Debug)]
pub enum VersionFrom {
    /// The latest version is the answer of a text address (`v1.31.0`).
    Text(&'static str),
    /// …a field of a JSON answer.
    Json(&'static str, &'static str),
    /// …named in the address a "latest" link redirects to.
    Redirect(&'static str),
    /// The vendor publishes only "latest"; the version is read from the program afterwards.
    Latest,
    /// …the newest Node.js release of the long-term-support line (`lts` is a name, not `false`) that has a build for this machine, read
    /// from a Node.js `index.json` (24.x "Krypton" in October 2026; the next line is picked up when the project makes it LTS).
    NodeLts(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// One program file.
    File(&'static str),
    TarGz(&'static str),
    Zip(&'static str),
    /// A zstd-compressed tar, unpacked while it is decompressed (Ollama ships this way).
    TarZst(&'static str),
    /// An xz-compressed tar, unpacked by IYAKKI's own reader (`xz.rs`; the static ffmpeg builds ship this way).
    TarXz(&'static str),
    /// A Python package installed into its own environment (needs `python3` with `venv`; when there is none, the catalogue's Python is installed first).
    Pip(&'static str),
    /// An npm package installed with `npm install -g` into a folder of its own (the child's `npm_config_prefix`; needs node and npm; when
    /// Node.js 22 or newer is not on the machine, the catalogue's Node.js is installed first).
    Npm(&'static str),
    /// A Python (the given version, e.g. `3.12`) that `uv` downloads into the tool's own folder; needs the catalogue's `uv`.
    Python(&'static str),
    /// Already on the machine (OpenSSH on a computer, Dropbear's client on an IYAKKI image): detected, never downloaded, and
    /// updated by the system's own package manager.
    System,
    /// The download address is not built in: it is a setting (the key named here) that the person or the image maker gives, because
    /// the vendor publishes no build that runs on every IYAKKI image. Until it is set, installing stops with the reason and nothing
    /// is fetched (OpenVPN, `docs/development/VPN-CONNECTOR-PLAN.md`).
    Setting(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub enum Checksum {
    /// A file holding only the hash (or `hash  name`).
    Single(&'static str),
    /// A file of `hash  name` lines; the line for the downloaded file's name is used.
    Lines(&'static str),
    /// A JSON manifest with `platforms.<platform>.checksum` (Anthropic's `manifest.json`); the platform is the `{cplatform}` of the download.
    Manifest(&'static str),
    /// A file holding an MD5 digest (`md5  name`), the only digest some vendors publish (checked with IYAKKI's own MD5, `xz.rs`).
    Md5(&'static str),
    /// SHA-256 values read from the vendor's release metadata (GitHub's asset digests) when the pinned version was chosen:
    /// (name of the downloaded file, hex digest). Moves with `pin`.
    Fixed(&'static [(&'static str, &'static str)]),
}

pub struct CliDef {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub home: &'static str,
    /// The licence of the program, shown on its store entry and in the CLI tile (SPDX where there is one).
    pub license: &'static str,
    /// An exact version this release of IYAKKI installs (empty: the vendor's newest, checked at download when the vendor publishes a checksum).
    /// Set for every tool that has no vendor checksum, so a moving "latest" is never fetched unverified.
    pub pin: &'static str,
    pub version: VersionFrom,
    pub source: Source,
    pub checksum: Option<Checksum>,
    /// Programs to link: (command name, where it is inside the unpacked folder; `*/` matches the one top folder).
    pub bins: &'static [(&'static str, &'static str)],
    /// Hosts (and their sub-domains) the download may come from or be sent to.
    pub hosts: &'static [&'static str],
    /// The host the account's network policy is checked for when the chat runs it; none = any host.
    pub net_host: Option<&'static str>,
    /// The arguments that make the first program print its version.
    pub version_args: &'static [&'static str],
    pub note: &'static str,
}

impl CliDef {
    /// For a tool the vendor publishes no checksum for: what IYAKKI can and cannot prove about the download, shown on the item's screen.
    pub fn integrity_warning(&self) -> Option<String> {
        if self.checksum.is_some() || matches!(self.source, Source::System) {
            return None;
        }
        let why = match self.source {
            Source::Pip(_) => "PyPI lists a SHA-256 for each file, but this release carries no hash values for it (the signed downloads manifest can: `pip --require-hashes`), so pip runs without a hash lock and the package's own dependencies are not pinned",
            Source::Npm(_) => "npm checks the registry's integrity value, but the package's own dependencies are not pinned",
            Source::Python(_) => "uv checks each Python build against the SHA-256 table built into uv, which is the only check",
            _ => match self.id {
                "aws-cli" if pgp_key_pinned(self.id) => return None,
                "aws-cli" => "Amazon publishes only a PGP signature for this file, and the Amazon public key is not yet pinned in this IYAKKI release (legal/aws-cli-pgp.asc), so the signature is not checked",
                "gcloud" => "Google publishes no checksum file for this download",
                _ => "the vendor publishes no checksum for this download",
            },
        };
        Some(format!("Warning: IYAKKI cannot prove this download is the vendor's file: {why}. Version {} is pinned in this IYAKKI release and fetched over HTTPS only.", self.pin))
    }

    /// The most that may be downloaded and the most that may be unpacked from it: the same for every tool except the ones
    /// that carry large libraries (Ollama is a download of over a gigabyte).
    pub fn limits(&self) -> (u64, u64) {
        match self.id {
            "ollama" => (3 << 30, 12 << 30),
            _ => (MAX_DOWNLOAD, MAX_UNPACKED),
        }
    }
}

pub const CATALOG: &[CliDef] = &[
    CliDef {
        id: "kubectl",
        name: "kubectl",
        about: "Talk to a Kubernetes cluster: pods, deployments, logs.",
        home: "https://kubernetes.io/docs/reference/kubectl/",
        license: "Apache-2.0",
        pin: "",
        version: VersionFrom::Text("https://dl.k8s.io/release/stable.txt"),
        source: Source::File("https://dl.k8s.io/release/{v}/bin/linux/{arch}/kubectl"),
        checksum: Some(Checksum::Single("https://dl.k8s.io/release/{v}/bin/linux/{arch}/kubectl.sha256")),
        bins: &[("kubectl", "kubectl")],
        hosts: &["k8s.io"],
        net_host: None,
        version_args: &["version", "--client"],
        note: "Uses your kubeconfig (KUBECONFIG, or ~/.kube/config).",
    },
    CliDef {
        id: "k3s",
        name: "k3s",
        about: "Lightweight Kubernetes: the k3s program (it also carries kubectl as `k3s kubectl`).",
        home: "https://k3s.io",
        license: "Apache-2.0",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/k3s-io/k3s/releases/latest/download/k3s"),
        source: Source::File("https://github.com/k3s-io/k3s/releases/download/{ve}/k3s{k3s_suffix}"),
        checksum: Some(Checksum::Lines("https://github.com/k3s-io/k3s/releases/download/{ve}/sha256sum-{arch}.txt")),
        bins: &[("k3s", "k3s")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: None,
        version_args: &["--version"],
        note: "Only the program is installed. Running a cluster needs root and a service, which is not done from here.",
    },
    CliDef {
        id: "helm",
        name: "Helm",
        about: "The Kubernetes package manager: install and upgrade charts.",
        home: "https://helm.sh",
        license: "Apache-2.0",
        pin: "",
        version: VersionFrom::Text("https://get.helm.sh/helm-latest-version"),
        source: Source::TarGz("https://get.helm.sh/helm-{v}-linux-{arch}.tar.gz"),
        checksum: Some(Checksum::Single("https://get.helm.sh/helm-{v}-linux-{arch}.tar.gz.sha256sum")),
        bins: &[("helm", "*/helm")],
        hosts: &["helm.sh", "azureedge.net", "windows.net"],
        net_host: None,
        version_args: &["version", "--short"],
        note: "",
    },
    CliDef {
        id: "terraform",
        name: "Terraform",
        about: "Describe infrastructure as files and apply it to a cloud.",
        home: "https://developer.hashicorp.com/terraform",
        license: "BUSL-1.1 (source-available, not open source)",
        pin: "",
        version: VersionFrom::Json("https://checkpoint-api.hashicorp.com/v1/check/terraform", "/current_version"),
        source: Source::Zip("https://releases.hashicorp.com/terraform/{vn}/terraform_{vn}_linux_{arch}.zip"),
        checksum: Some(Checksum::Lines("https://releases.hashicorp.com/terraform/{vn}/terraform_{vn}_SHA256SUMS")),
        bins: &[("terraform", "terraform")],
        hosts: &["hashicorp.com"],
        net_host: None,
        version_args: &["version"],
        note: "Terraform has been under the Business Source Licence (BUSL-1.1) since version 1.6: source-available, not open source. IYAKKI does not ship it; you download it yourself from HashiCorp at your request, under HashiCorp's terms. OpenTofu (in this list) is the open-source fork that reads the same files.",
    },
    CliDef {
        id: "opentofu",
        name: "OpenTofu",
        about: "Describe infrastructure as files and apply it to a cloud (the open-source fork of Terraform; the `tofu` command).",
        home: "https://opentofu.org",
        license: "MPL-2.0",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/opentofu/opentofu/releases/latest"),
        source: Source::TarGz("https://github.com/opentofu/opentofu/releases/download/{v}/tofu_{vn}_linux_{arch}.tar.gz"),
        checksum: Some(Checksum::Lines("https://github.com/opentofu/opentofu/releases/download/{v}/tofu_{vn}_SHA256SUMS")),
        bins: &[("tofu", "tofu")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: None,
        version_args: &["version"],
        note: "Mozilla Public License 2.0 (a Linux Foundation project). Downloaded from the project's GitHub releases at your request and checked against its SHA256SUMS file; IYAKKI does not ship it.",
    },
    CliDef {
        id: "gh",
        name: "GitHub CLI",
        about: "Repositories, issues and pull requests from the command line.",
        home: "https://cli.github.com",
        license: "MIT",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/cli/cli/releases/latest"),
        source: Source::TarGz("https://github.com/cli/cli/releases/download/{v}/gh_{vn}_linux_{arch}.tar.gz"),
        checksum: Some(Checksum::Lines("https://github.com/cli/cli/releases/download/{v}/gh_{vn}_checksums.txt")),
        bins: &[("gh", "*/bin/gh")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("api.github.com"),
        version_args: &["--version"],
        note: "Sign in with `gh auth login` or set GH_TOKEN.",
    },
    CliDef {
        id: "aws-cli",
        name: "AWS CLI",
        about: "Amazon Web Services from the command line (the `aws` command).",
        home: "https://aws.amazon.com/cli/",
        license: "Apache-2.0",
        pin: "2.38.6",
        version: VersionFrom::Latest,
        source: Source::Zip("https://awscli.amazonaws.com/awscli-exe-linux-{arch2}-{v}.zip"),
        checksum: None,
        bins: &[("aws", "aws/dist/aws"), ("aws_completer", "aws/dist/aws_completer")],
        hosts: &["amazonaws.com"],
        net_host: Some("amazonaws.com"),
        version_args: &["--version"],
        note: "Amazon publishes no checksum for this download (it is over HTTPS from amazonaws.com). Credentials come from ~/.aws or AWS_* variables.",
    },
    CliDef {
        id: "gcloud",
        name: "Google Cloud CLI",
        about: "Google Cloud from the command line (`gcloud` and `gsutil`).",
        home: "https://cloud.google.com/sdk",
        license: "Apache-2.0 (Google Cloud terms apply to the services)",
        pin: "590.0.1",
        version: VersionFrom::Latest,
        source: Source::TarGz("https://dl.google.com/dl/cloudsdk/channels/rapid/downloads/google-cloud-cli-{v}-linux-{arch3}.tar.gz"),
        checksum: None,
        bins: &[("gcloud", "google-cloud-sdk/bin/gcloud"), ("gsutil", "google-cloud-sdk/bin/gsutil")],
        hosts: &["google.com", "googleapis.com"],
        net_host: Some("googleapis.com"),
        version_args: &["--version"],
        note: "Google publishes no checksum for this download (it is over HTTPS from dl.google.com). About 150 MB. Sign in with `gcloud auth login`.",
    },
    CliDef {
        id: "arduino-cli",
        name: "Arduino CLI",
        about: "Build and upload Arduino and ESP32/ESP8266 sketches: boards, libraries, compile, upload.",
        home: "https://arduino.github.io/arduino-cli/",
        license: "GPL-3.0",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/arduino/arduino-cli/releases/latest"),
        source: Source::TarGz("https://github.com/arduino/arduino-cli/releases/download/{v}/arduino-cli_{vn}_{arduino}.tar.gz"),
        checksum: Some(Checksum::Lines("https://github.com/arduino/arduino-cli/releases/download/{v}/{vn}-checksums.txt")),
        bins: &[("arduino-cli", "arduino-cli")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: None,
        version_args: &["version"],
        note: "Board packages and libraries (for example `arduino-cli core install esp32:esp32`) are downloaded by arduino-cli itself. Uploading needs permission to use the serial device (/dev/ttyUSB0, /dev/ttyACM0).",
    },
    CliDef {
        id: "esptool",
        name: "esptool (ESP32/ESP8266)",
        about: "Espressif's tool to flash firmware to, read and erase ESP32 and ESP8266 chips over USB (`esptool`, `espefuse`, `espsecure`).",
        home: "https://docs.espressif.com/projects/esptool/",
        license: "GPL-2.0-or-later",
        pin: "5.6.0",
        version: VersionFrom::Latest,
        source: Source::Pip("esptool"),
        checksum: None,
        bins: &[("esptool", "venv/bin/esptool"), ("espefuse", "venv/bin/espefuse"), ("espsecure", "venv/bin/espsecure")],
        hosts: &["pypi.org", "pythonhosted.org"],
        net_host: Some("-"),
        version_args: &["version"],
        note: "Installed from PyPI into its own Python environment, so `python3` with `venv` is needed. It only talks to the chip over USB; it needs permission to use the serial device (/dev/ttyUSB0, /dev/ttyACM0).",
    },
    CliDef {
        id: "espflash",
        name: "espflash (ESP32)",
        about: "A fast ESP32 flasher and serial monitor written in Rust (`espflash`).",
        home: "https://github.com/esp-rs/espflash",
        license: "MIT OR Apache-2.0",
        pin: "v4.6.0",
        version: VersionFrom::Redirect("https://github.com/esp-rs/espflash/releases/latest"),
        source: Source::Zip("https://github.com/esp-rs/espflash/releases/download/{v}/espflash-{arch2}-unknown-linux-{libc_triple}.zip"),
        checksum: Some(Checksum::Fixed(&[
            ("espflash-x86_64-unknown-linux-gnu.zip", "c4fcfa32ddc3155608f003a230e6f3c184eff9641ec9cfa8eb54f1b515d403e7"),
            ("espflash-x86_64-unknown-linux-musl.zip", "d515ee13ae44ba913c31a2ca58dc5fd72d3362994ee1d7487a16b289dfbb6e4a"),
            ("espflash-aarch64-unknown-linux-gnu.zip", "560c690ce28c7fe2b31709eeaf1fcc6717cfa832798b68287dddca1e3a3b65d6"),
        ])),
        bins: &[("espflash", "espflash")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "The project publishes no checksum for this download (it is over HTTPS from github.com); the musl build is used on musl systems (published for x86_64 only). It only talks to the chip over USB; it needs permission to use the serial device.",
    },
    CliDef {
        id: "azure-cli",
        name: "Azure CLI",
        about: "Microsoft Azure from the command line (the `az` command).",
        home: "https://learn.microsoft.com/cli/azure/",
        license: "MIT",
        pin: "2.91.0",
        version: VersionFrom::Latest,
        source: Source::Pip("azure-cli"),
        checksum: None,
        bins: &[("az", "venv/bin/az")],
        hosts: &["pypi.org", "pythonhosted.org"],
        net_host: Some("management.azure.com"),
        version_args: &["--version"],
        note: "Installed from PyPI into its own Python environment, so `python3` with `venv` is needed, and it takes a few minutes. Sign in with `az login`.",
    },
    CliDef {
        id: "nodejs",
        name: "Node.js",
        about: "The JavaScript runtime with npm (`node`, `npm`, `npx`). Installed by itself when an npm tool (Claude Code, Copilot CLI, Codex, Gemini CLI) needs it and the machine has no Node.js 22 or newer.",
        home: "https://nodejs.org",
        license: "MIT",
        pin: "",
        version: VersionFrom::NodeLts("https://{node_dist}/index.json"),
        source: Source::TarGz("https://{node_dist}/{v}/node-{v}-linux-{node_arch}{node_libc}.tar.gz"),
        checksum: Some(Checksum::Lines("https://{node_dist}/{v}/SHASUMS256.txt")),
        bins: &[("node", "*/bin/node"), ("npm", "*/bin/npm"), ("npx", "*/bin/npx")],
        hosts: &["nodejs.org", "alpinelinux.org"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "MIT licence. The newest release from nodejs.org, checked against its published checksum, kept in IYAKKI's data folder (no sudo). On a musl system (the IYAKKI images) the musl build from unofficial-builds.nodejs.org is used, and when the machine lacks `libstdc++` it is fetched once from Alpine's package repository (over HTTPS; Alpine publishes no separate checksum for that file) and `node` starts through a small launcher that points at it.",
    },
    CliDef {
        id: "uv",
        name: "uv",
        about: "A fast Python package and version manager (`uv`, `uvx`). It is how Python is installed here.",
        home: "https://docs.astral.sh/uv/",
        license: "MIT OR Apache-2.0",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/astral-sh/uv/releases/latest"),
        source: Source::TarGz("https://github.com/astral-sh/uv/releases/download/{v}/uv-{arch2}-unknown-linux-musl.tar.gz"),
        checksum: Some(Checksum::Single("https://github.com/astral-sh/uv/releases/download/{v}/uv-{arch2}-unknown-linux-musl.tar.gz.sha256")),
        bins: &[("uv", "*/uv"), ("uvx", "*/uvx")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "MIT or Apache-2.0. The statically linked build, so it runs on glibc and musl systems alike; the download is checked against the checksum file the project publishes.",
    },
    CliDef {
        id: "python",
        name: "Python 3",
        about: "Python 3 with venv and pip (`python3`, `python`). Installed by itself when a Python tool (Aider, esptool, the Azure CLI) needs it and the machine has none.",
        home: "https://www.python.org",
        license: "PSF-2.0",
        pin: "3.12.16",
        version: VersionFrom::Latest,
        source: Source::Python("3.12.16"),
        checksum: None,
        bins: &[("python3", "*/bin/python3"), ("python", "*/bin/python")],
        hosts: &["github.com", "githubusercontent.com", "pypi.org"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "PSF licence. Python 3.12 from the python-build-standalone project, downloaded by `uv` (installed first when missing) into IYAKKI's data folder, with no sudo and nothing outside it touched.",
    },
    CliDef {
        id: "claude-code",
        name: "Claude Code",
        about: "Anthropic's coding agent. Once installed and signed in, it can answer the AI IDE's chat (Settings → Agents).",
        home: "https://code.claude.com/docs/en/setup",
        license: "Proprietary (Anthropic terms)",
        pin: "",
        version: VersionFrom::Text("https://downloads.claude.ai/claude-code-releases/latest"),
        source: Source::File("https://downloads.claude.ai/claude-code-releases/{v}/{cplatform}/claude"),
        checksum: Some(Checksum::Manifest("https://downloads.claude.ai/claude-code-releases/{v}/manifest.json")),
        bins: &[("claude", "bin/claude")],
        hosts: &["claude.ai", "registry.npmjs.org"],
        net_host: Some("api.anthropic.com"),
        version_args: &["--version"],
        note: "Proprietary software from Anthropic: IYAKKI does not ship it; it is downloaded from Anthropic at your request. It is Anthropic's own native build for this machine (the musl build on IYAKKI, from downloads.claude.ai, the same files as Anthropic's installer), checked against the SHA-256 in Anthropic's release manifest and kept in this tool's own folder: no Node.js, no sudo, your shell files are untouched. If that download fails and Node.js is on the machine, the npm package is used instead. Sign in yourself with `claude auth login` in a terminal; IYAKKI never sees your password. On a musl system Anthropic asks for ripgrep installed and USE_BUILTIN_RIPGREP=0.",
    },
    CliDef {
        id: "github-copilot",
        name: "GitHub Copilot CLI",
        about: "GitHub's coding agent in the terminal. Once installed and signed in, it can answer the AI IDE's chat as a read-only advisor (Settings → Agents).",
        home: "https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli",
        license: "Proprietary (GitHub Copilot CLI licence)",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/github/copilot-cli/releases/latest"),
        source: Source::TarGz("https://github.com/github/copilot-cli/releases/download/{v}/copilot-linux{musl}-{arch4}.tar.gz"),
        checksum: Some(Checksum::Lines("https://github.com/github/copilot-cli/releases/download/{v}/SHA256SUMS.txt")),
        bins: &[("copilot", "copilot")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("api.github.com"),
        version_args: &["--version"],
        note: "Proprietary software from GitHub: IYAKKI does not ship it; the standalone program GitHub publishes (the musl build on IYAKKI; no Node.js needed) is downloaded from GitHub's releases at your request and checked against GitHub's SHA256SUMS.txt. It needs a GitHub Copilot subscription. Sign in with `copilot login`.",
    },
    CliDef {
        id: "codex",
        name: "OpenAI Codex CLI",
        about: "OpenAI's coding agent in the terminal. Use it from the terminal, or add it as a custom agent in Settings → Agents.",
        home: "https://github.com/openai/codex",
        license: "Apache-2.0",
        pin: "0.164.0",
        version: VersionFrom::Latest,
        source: Source::Npm("@openai/codex"),
        checksum: None,
        bins: &[("codex", "bin/codex")],
        hosts: &["registry.npmjs.org"],
        net_host: Some("api.openai.com"),
        version_args: &["--version"],
        note: "Installed with npm (Node.js from this catalogue is installed first when the machine has none) into this tool's own folder (no sudo). Run `codex` and choose Sign in, or use an API key.",
    },
    CliDef {
        id: "gemini-cli",
        name: "Gemini CLI",
        about: "Google's coding agent in the terminal. Use it from the terminal, or add it as a custom agent in Settings → Agents.",
        home: "https://github.com/google-gemini/gemini-cli",
        license: "Apache-2.0",
        pin: "0.65.0",
        version: VersionFrom::Latest,
        source: Source::Npm("@google/gemini-cli"),
        checksum: None,
        bins: &[("gemini", "bin/gemini")],
        hosts: &["registry.npmjs.org"],
        net_host: Some("generativelanguage.googleapis.com"),
        version_args: &["--version"],
        note: "Installed with npm (Node.js from this catalogue is installed first when the machine has none) into this tool's own folder (no sudo). Sign in with Google when it asks, or set GEMINI_API_KEY.",
    },
    CliDef {
        id: "aider",
        name: "Aider",
        about: "A coding assistant in the terminal that edits files in a git repository. Add it as a custom agent in Settings → Agents.",
        home: "https://aider.chat/docs/install.html",
        license: "Apache-2.0",
        pin: "0.86.2",
        version: VersionFrom::Latest,
        source: Source::Pip("aider-chat"),
        checksum: None,
        bins: &[("aider", "venv/bin/aider")],
        hosts: &["pypi.org", "pythonhosted.org"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "Installed from PyPI into its own Python environment (Python 3.9 to 3.12 per its documentation), which takes a few minutes. It talks to the model service you give it a key for.",
    },
    CliDef {
        id: "ollama",
        name: "Ollama",
        about: "Run language models on this machine's own processor or graphics card, and use them from the chat.",
        home: "https://ollama.com",
        license: "MIT",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/ollama/ollama/releases/latest"),
        source: Source::TarZst("https://github.com/ollama/ollama/releases/download/{v}/ollama-linux-{arch}.tar.zst"),
        checksum: Some(Checksum::Lines("https://github.com/ollama/ollama/releases/download/{v}/sha256sum.txt")),
        bins: &[("ollama", "bin/ollama")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "A download of over a gigabyte (it carries the graphics libraries) that unpacks to several. Start the server from here, then pull models from the chat or Models; they are kept in the data folder. A machine without enough memory or a graphics card will be slow.",
    },
    CliDef {
        id: "ffmpeg",
        name: "ffmpeg (media player)",
        about: "Decodes video and audio. IYAKKI's own player (MEDIA tile, IYAKKI menu \u{2192} Media player) uses it to play almost any video or sound file, including the camera's recordings (the `ffmpeg` and `ffprobe` commands).",
        home: "https://johnvansickle.com/ffmpeg/",
        license: "GPL-3.0 (static build)",
        pin: "",
        version: VersionFrom::Latest,
        source: Source::TarXz("https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-{arch}-static.tar.xz"),
        checksum: Some(Checksum::Md5("https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-{arch}-static.tar.xz.md5")),
        bins: &[("ffmpeg", "*/ffmpeg"), ("ffprobe", "*/ffprobe")],
        hosts: &["johnvansickle.com"],
        net_host: Some("-"),
        version_args: &["-version"],
        note: "GNU GPL version 3 (a static build, so it runs on the IYAKKI images). IYAKKI does not ship it: you download it here from johnvansickle.com, checked against the MD5 file the builder publishes next to it (about 42 MB on x86-64, 19 MB on ARM). H.264 and AAC are patent-licensed formats; decoding them is your choice and your licence matter, as in the web browser (docs/FULL-BROWSER.md). Unpacked it takes about 150 MB of the data folder. IYAKKI starts it only as a decoder with fixed arguments, never through a shell.",
    },
    CliDef {
        id: "7zip",
        name: "7-Zip (reads disk images)",
        about: "Opens archives and disk images (DMG, HFS+, APFS, ISO, ZIP, tar and more): the `7zz` command. IYAKKI uses it to read Apple's Wi-Fi and Bluetooth firmware out of a Mac's recovery image or a macOS disk image (IYAKKI menu \u{2192} Mac drivers).",
        home: "https://www.7-zip.org/",
        license: "LGPL-2.1+ with unRAR restriction, parts BSD",
        pin: "26.05",
        version: VersionFrom::Redirect("https://github.com/ip7z/7zip/releases/latest"),
        source: Source::TarXz("https://github.com/ip7z/7zip/releases/download/{v}/7z{v7}-linux-{arch7}.tar.xz"),
        checksum: Some(Checksum::Fixed(&[
            ("7z2605-linux-x64.tar.xz", "22cf82fcc6eb7474f1d5bb4f02c0f8ebd573a718089d6833dc30f8350a3b232e"),
            ("7z2605-linux-arm64.tar.xz", "0b71f6cfc34669b3bf69f2347efd3429f77020665003a13b9cdc53b784270421"),
            ("7z2605-linux-arm.tar.xz", "fcc6744c35f5af3dda0d57de379e885626b2c0fca2ac8a33410eba17d1d94954"),
            ("7z2605-linux-x86.tar.xz", "e8d8b762a4ee97fffa36a35b9831bbd0a7bb4645e5fafad839f9c22e297587a8"),
        ])),
        bins: &[("7zz", "7zzs")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["i"],
        note: "7-Zip by Igor Pavlov: GNU LGPL, with the unRAR restriction for RAR code and a BSD licence for some parts (see 7-zip.org/license.txt). IYAKKI does not ship it: you download the project's own static Linux build (7zzs, linked with no libraries, so it runs on the IYAKKI images) from the project's release page on GitHub when you ask for it (about 1.5 MB). The project publishes no checksum file for it, so the list shows it as not verified: the download is protected by HTTPS only. IYAKKI runs it only to list and unpack a disk image into a folder on the data volume, with fixed arguments, never through a shell, and never on a drive.",
    },
    CliDef {
        id: "duckdb",
        name: "DuckDB",
        about: "Run SQL on CSV and Parquet files or a database file (the `duckdb` command); the DASH tile uses it for its DuckDB source.",
        home: "https://duckdb.org",
        license: "MIT",
        pin: "1.3.2",
        version: VersionFrom::Latest,
        source: Source::Pip("duckdb"),
        checksum: None,
        bins: &[("duckdb", "bin/duckdb")],
        hosts: &["pypi.org", "pythonhosted.org"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "MIT licence. DuckDB's own command-line program is built for glibc and does not run on musl systems such as the IYAKKI images, so DuckDB is installed from its Python package (Python 3 from this list is installed first when missing; pip is told to use a ready-made wheel only) and IYAKKI adds a small `duckdb` command that runs SQL with it: `duckdb [-c SQL] [-json|-csv|-markdown] [file.db]`, or SQL on standard input. On musl pip picks the newest DuckDB that has a musl wheel, which can be older than the newest DuckDB (the list shows which). The DASH tile only runs read-only queries with it.",
    },
    CliDef {
        id: "grpcurl",
        name: "grpcurl",
        about: "Call gRPC services from the command line, with JSON in and out (the `grpcurl` command); the FLOWS tile's gRPC node uses it.",
        home: "https://github.com/fullstorydev/grpcurl",
        license: "MIT",
        pin: "",
        version: VersionFrom::Redirect("https://github.com/fullstorydev/grpcurl/releases/latest"),
        source: Source::TarGz("https://github.com/fullstorydev/grpcurl/releases/download/{v}/grpcurl_{vn}_linux_{grpc}.tar.gz"),
        checksum: Some(Checksum::Lines("https://github.com/fullstorydev/grpcurl/releases/download/{v}/grpcurl_{vn}_checksums.txt")),
        bins: &[("grpcurl", "grpcurl")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["-version"],
        note: "MIT licence. The download is checked against the checksum file the project publishes with each release. FLOWS runs it as an argument list, never through a shell.",
    },
    CliDef {
        id: "ssh",
        name: "SSH client (OpenSSH)",
        about: "Log in to other machines, run a command there, copy files (`ssh`, `scp`, `sftp`) and make keys (`ssh-keygen`). Host keys and private keys are kept by IYAKKI; see the Connections tab for saved hosts.",
        home: "https://www.openssh.com",
        license: "BSD / ISC (OpenSSH)",
        pin: "",
        version: VersionFrom::Latest,
        source: Source::System,
        checksum: None,
        bins: &[("ssh", "ssh"), ("scp", "scp"), ("sftp", "sftp"), ("ssh-keygen", "ssh-keygen")],
        hosts: &["openssh.com"],
        net_host: None,
        version_args: &["-V"],
        note: "Not downloaded: it is found on this machine. The IYAKKI images carry Dropbear's client (`dbclient`, also named `ssh`; BSD/MIT licence; it has `scp` but no `sftp`). New host keys are trusted only after you confirm the fingerprint; a changed key is refused. The assistant needs your approval for every call and can use only keys kept in your vault.",
    },
    CliDef {
        id: "openvpn",
        name: "OpenVPN (for .ovpn profiles)",
        about: "Connects the OpenVPN profiles you import in Settings -> Network -> VPN. IYAKKI starts it only with a fixed argument list (no scripts, routes and DNS applied by IYAKKI's own checks), never through a shell.",
        home: "https://openvpn.net/community/",
        license: "GPL-2.0 with OpenSSL exception",
        pin: "from-setting",
        version: VersionFrom::Latest,
        source: Source::Setting("vpn.openvpn_download"),
        checksum: None,
        bins: &[("openvpn", "openvpn")],
        hosts: &["openvpn.net"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "GNU GPL version 2: IYAKKI does not ship it; you download it here, after reading the licence, and it is checked against the SHA-256 you give with the address. The OpenVPN project publishes source tarballs and distribution packages but no build for IYAKKI's musl images, so no address is built in and none is invented: until `vpn.openvpn_download` is set, this entry explains the gap and fetches nothing. WireGuard profiles need no download.",
    },
    CliDef {
        id: "cryptsetup",
        name: "cryptsetup (to unlock LUKS drives)",
        about: "Unlocks LUKS encrypted drives for DISKS. IYAKKI starts it only with a fixed argument list, never through a shell, and gives it the password on its standard input (never on the command line).",
        home: "https://gitlab.com/cryptsetup/cryptsetup",
        license: "GPL-2.0 with OpenSSL exception (the library: LGPL-2.1)",
        pin: "from-setting",
        version: VersionFrom::Latest,
        source: Source::Setting("disks.cryptsetup_download"),
        checksum: None,
        bins: &[("cryptsetup", "cryptsetup")],
        hosts: &["gitlab.com", "kernel.org"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "GNU GPL: IYAKKI does not ship it; you download it here, after reading the licence, and it is checked against the SHA-256 you give with the address. The project publishes source tarballs but no build for IYAKKI's images, so no address is built in and none is invented: until `disks.cryptsetup_download` is set, this entry explains the gap and fetches nothing. A system package manager's cryptsetup is found too. See docs/development/DISKS-UNLOCK-HELPER.md.",
    },
    CliDef {
        id: "dislocker",
        name: "dislocker (to unlock BitLocker drives)",
        about: "Unlocks BitLocker drives (password or recovery key) for DISKS, read-only. IYAKKI starts it only with a fixed argument list, never through a shell, and gives it the key on its standard input.",
        home: "https://github.com/Aorimn/dislocker",
        license: "GPL-2.0",
        pin: "from-setting",
        version: VersionFrom::Latest,
        source: Source::Setting("disks.dislocker_download"),
        checksum: None,
        bins: &[("dislocker", "dislocker")],
        hosts: &["github.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "GNU GPL version 2: IYAKKI does not ship it; you download it here, after reading the licence, and it is checked against the SHA-256 you give with the address. It needs FUSE and `losetup` on the machine. No address is built in and none is invented: until `disks.dislocker_download` is set, this entry explains the gap and fetches nothing. See docs/development/DISKS-UNLOCK-HELPER.md.",
    },
    CliDef {
        id: "exfatprogs",
        name: "exfatprogs (to format drives as exFAT)",
        about: "Makes exFAT file systems for DISKS (`mkfs.exfat`). IYAKKI starts it only after you typed the drive's name and the countdown ended, with a fixed argument list, never through a shell.",
        home: "https://github.com/exfatprogs/exfatprogs",
        license: "GPL-2.0",
        pin: "from-setting",
        version: VersionFrom::Latest,
        source: Source::Setting("disks.exfatprogs_download"),
        checksum: None,
        bins: &[("mkfs.exfat", "mkfs.exfat")],
        hosts: &["github.com"],
        net_host: Some("-"),
        version_args: &["-V"],
        note: "GNU GPL version 2: IYAKKI does not ship it; you download it here, after reading the licence, and it is checked against the SHA-256 you give with the address. No address is built in and none is invented: until `disks.exfatprogs_download` is set, this entry explains the gap and fetches nothing. A system package manager's copy is found too. See docs/DISKS.md.",
    },
    CliDef {
        id: "ntfs-3g",
        name: "ntfs-3g (to format and resize NTFS drives)",
        about: "Makes NTFS file systems (`mkntfs`) and resizes them (`ntfsresize`) for DISKS. IYAKKI starts them only after you typed the drive's name and the countdown ended, with fixed argument lists, never through a shell.",
        home: "https://github.com/tuxera/ntfs-3g",
        license: "GPL-2.0",
        pin: "from-setting",
        version: VersionFrom::Latest,
        source: Source::Setting("disks.ntfs3g_download"),
        checksum: None,
        bins: &[("mkntfs", "mkntfs"), ("ntfsresize", "ntfsresize")],
        hosts: &["github.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "GNU GPL version 2 (the library part LGPL-2): IYAKKI does not ship it; you download it here, after reading the licence, and it is checked against the SHA-256 you give with the address. No address is built in and none is invented: until `disks.ntfs3g_download` is set, this entry explains the gap and fetches nothing. A system package manager's copy is found too. See docs/DISKS.md.",
    },
];

/// The SHA-256 of some bytes, as lower-case hex.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub fn def(id: &str) -> Option<&'static CliDef> {
    CATALOG.iter().find(|d| d.id == id)
}

/// The Amazon public key pinned in the repository for the AWS CLI's `.sig` file (`unset` until the owner adds it).
pub const AWS_CLI_PGP_KEY: &str = include_str!("../../../legal/aws-cli-pgp.asc");

pub fn pgp_key_file(id: &str) -> Option<&'static str> {
    let k = match id {
        "aws-cli" => AWS_CLI_PGP_KEY,
        _ => return None,
    };
    k.trim_start().starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----").then_some(k)
}

pub fn pgp_key_pinned(id: &str) -> bool {
    pgp_key_file(id).is_some()
}

/// The most that is unpacked from one download (the same number as `iyakki_archive::archive::MAX_TOTAL`; a test in the agent keeps them equal).
pub const MAX_UNPACKED: u64 = 2 << 30;

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub fn host_ok(url: &str, hosts: &[&str]) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| format!("only https downloads are allowed ({url})"))?;
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if hosts.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) { Ok(()) } else { Err(format!("the download tried to go to {host}, which is not one of this tool's sources")) }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub fn template(t: &str, v: &str) -> Result<String, String> {
    template_for(t, v, is_musl(), std::env::consts::ARCH)
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use std::path::Path;

/// [`template`] for a given C library and processor (`x86_64` or `aarch64`), so the choice of build is testable.
pub fn template_for(t: &str, v: &str, musl: bool, machine: &str) -> Result<String, String> {
    let (a, a2, a3) = arch_of(machine)?;
    let (node_dist, node_libc) = if musl { ("unofficial-builds.nodejs.org/download/release", "-musl") } else { ("nodejs.org/dist", "") };
    let node_arch = if a == "arm64" { "arm64" } else { "x64" };
    let t = t.replace("{libc_triple}", if musl { "musl" } else { "gnu" }).replace("{cplatform}", &format!("linux-{node_arch}{}", if musl { "-musl" } else { "" })).replace("{arch4}", node_arch).replace("{musl}", if musl { "musl" } else { "" });
    Ok(t.replace("{node_dist}", node_dist).replace("{node_libc}", node_libc).replace("{node_arch}", node_arch).replace("{ve}", &pct_encode_version(v)).replace("{vn}", v.trim_start_matches('v')).replace("{v7}", &v.trim_start_matches('v').replace('.', "")).replace("{arch7}", node_arch).replace("{v}", v).replace("{arduino}", if a == "arm64" { "Linux_ARM64" } else { "Linux_64bit" }).replace("{grpc}", if a == "arm64" { "arm64" } else { "x86_64" }).replace("{arch2}", a2).replace("{arch3}", a3).replace("{arch}", a).replace("{k3s_suffix}", if a == "arm64" { "-arm64" } else { "" }))
}

/// The C library of this machine is musl (the IYAKKI images, Alpine): programs built for glibc do not run.
pub fn is_musl() -> bool {
    Path::new(&format!("/lib/ld-musl-{}.so.1", std::env::consts::ARCH)).exists()
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub fn arch_of(machine: &str) -> Result<(&'static str, &'static str, &'static str), String> {
    match machine {
        "x86_64" => Ok(("amd64", "x86_64", "x86_64")),
        "aarch64" => Ok(("arm64", "aarch64", "arm")),
        a => Err(format!("these tools are not offered for this machine's processor ({a})")),
    }
}

pub fn pct_encode_version(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c.to_string() } else { format!("%{:02X}", c as u32) }).collect()
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    /// What the program itself reported (`--version`), first line.
    #[serde(default)]
    pub reported: String,
    pub at: i64,
    pub bins: Vec<String>,
    /// The download's checksum was compared with the vendor's published one.
    pub verified: bool,
    pub by: String,
    /// The folder under `pkgs/<id>` this version lives in (empty in records from before: worked out from the version).
    #[serde(default)]
    pub dir: String,
    /// The version that was installed before this one, kept for one generation so it can be rolled back to.
    #[serde(default)]
    pub previous: Option<Previous>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: String,
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

impl Job {
    pub fn describe(&self) -> String {
        match (&self.error, self.finished) {
            (Some(e), _) => format!("{}: failed: {e}", self.id),
            (None, true) => format!("{}: installed", self.id),
            _ if self.total > 0 => format!("{}: {} ({}%, {} of {} MB)", self.id, self.phase, self.done * 100 / self.total.max(1), self.done >> 20, self.total >> 20),
            _ if self.done > 0 => format!("{}: {} ({} MB)", self.id, self.phase, self.done >> 20),
            _ => format!("{}: {}", self.id, self.phase),
        }
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----

/// The one older version a tool keeps (its folder stays on disk until the next update replaces it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Previous {
    pub version: String,
    pub dir: String,
    #[serde(default)]
    pub reported: String,
    pub bins: Vec<String>,
    pub verified: bool,
    pub at: i64,
}
