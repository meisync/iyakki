//! Command-line tools that can be installed from the CLI tile: kubectl, k3s, helm, terraform, the GitHub, AWS, Google
//! Cloud and Azure command lines. Each is downloaded over HTTPS from its vendor into a folder of its own under the
//! system's data (no root needed, nothing outside it is touched), checked against the vendor's published checksum where
//! there is one, unpacked by our own code, and its program is linked into one folder that the chat's `run_command` and
//! the terminal look in first. Removing a tool deletes its folder and links.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_proxy::Streamer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::exec::Extra;

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
            _ => (MAX_DOWNLOAD, crate::archive::MAX_TOTAL),
        }
    }
}

/// For tests (here and in the shell): records `id` as installed with `script` as its program, without downloading anything.
#[doc(hidden)]
pub fn write_fake_installed(m: &CliManager, id: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let prog = m.pkg_dir(id).join("v1").join(id);
    std::fs::create_dir_all(prog.parent().unwrap()).unwrap();
    std::fs::write(&prog, script).unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(m.bin_dir()).unwrap();
    let link = m.bin_dir().join(id);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&prog, &link).unwrap();
    let mut all = m.load();
    all.retain(|i| i.id != id);
    all.push(Installed { id: id.into(), version: "v1".into(), reported: String::new(), at: 1, bins: vec![id.into()], verified: true, by: "root".into(), dir: "v1".into(), previous: None });
    m.store(&all).unwrap();
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
];

/// The SHA-256 of some bytes, as lower-case hex.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub fn def(id: &str) -> Option<&'static CliDef> {
    CATALOG.iter().find(|d| d.id == id)
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
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

pub struct CliManager {
    root: PathBuf,
    streamer: Mutex<Option<Arc<dyn Streamer>>>,
    jobs: Mutex<HashMap<String, Job>>,
    state_lock: Mutex<()>,
    python: Mutex<String>,
    /// Tests: a PGP key file used for `(id, key)` instead of the one pinned in legal/.
    pgp_override: Mutex<Option<(String, Vec<u8>)>>,
    /// The programs an npm install uses (tests use stand-ins).
    npm: Mutex<String>,
    node: Mutex<String>,
    /// Told when an install finished well (Settings → Agents and the model list look again).
    on_installed: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
    /// Programs that come with the system (see [`Source::System`]), looked for again every few seconds, not on every redraw.
    system: Mutex<HashMap<String, (std::time::Instant, SystemStatus)>>,
    /// Where system programs are looked for (tests point it at a folder of stand-ins); `None` is the process's `PATH`.
    search: Mutex<Option<String>>,
    /// A program installed first because another needs it (id → the id of the install it is part of): its progress shows on that install.
    parents: Mutex<HashMap<String, String>>,
    /// Tests: pretend to be this C library (`true` = musl) and processor (`x86_64`, `aarch64`) when picking which build to download.
    target: Mutex<Option<(bool, &'static str)>>,
    /// On: a tool that is on the machine already (put there by hand) counts as installed and installing it is a no-op that says so.
    /// Off in tests, where the machine's own programs must not matter; the shell turns it on.
    detect_present: Mutex<bool>,
}

/// What was found for a tool that comes with the system.
#[derive(Clone, Debug, PartialEq, Eq)]
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

pub(crate) fn arch() -> Result<(&'static str, &'static str, &'static str), String> {
    arch_of(std::env::consts::ARCH)
}

pub(crate) fn arch_of(machine: &str) -> Result<(&'static str, &'static str, &'static str), String> {
    match machine {
        "x86_64" => Ok(("amd64", "x86_64", "x86_64")),
        "aarch64" => Ok(("arm64", "aarch64", "arm")),
        a => Err(format!("these tools are not offered for this machine's processor ({a})")),
    }
}

pub(crate) fn host_ok(url: &str, hosts: &[&str]) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| format!("only https downloads are allowed ({url})"))?;
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if hosts.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) { Ok(()) } else { Err(format!("the download tried to go to {host}, which is not one of this tool's sources")) }
}

fn resolve(base: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.starts_with("https://") || loc.starts_with("http://") {
        return Some(loc.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split('/').next()?;
    if loc.starts_with('/') { Some(format!("{scheme}://{authority}{loc}")) } else { None }
}

/// The published SHA-256 of `file` in a [`Checksum::Fixed`] table (tests can stand in a value for a file they build themselves).
fn fixed_hash(table: &[(&str, &str)], file: &str) -> Option<String> {
    #[cfg(test)]
    if let Some((_, h)) = TEST_FIXED.lock().unwrap().iter().find(|(n, _)| n == file) {
        return Some(h.clone());
    }
    table.iter().find(|(n, _)| *n == file).map(|(_, h)| h.to_string())
}

#[cfg(test)]
static TEST_FIXED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The npm install argument: `package@version` for a pinned tool.
fn pinned_npm(d: &CliDef, pkg: &str) -> String {
    if d.pin.is_empty() { pkg.to_string() } else { format!("{pkg}@{}", d.pin) }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 + 0 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode_version(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c.to_string() } else { format!("%{:02X}", c as u32) }).collect()
}

/// The version named in the address a "latest" link redirects to.
pub fn version_from_location(loc: &str) -> Option<String> {
    let after = if let Some(i) = loc.find("/tag/") { &loc[i + 5..] } else if let Some(i) = loc.find("/download/") { &loc[i + 10..] } else { return None };
    let v = pct_decode(after.split(['/', '?', '#']).next()?);
    (!v.is_empty() && v.len() < 40 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))).then_some(v)
}

pub(crate) fn template(t: &str, v: &str) -> Result<String, String> {
    template_for(t, v, is_musl(), std::env::consts::ARCH)
}

/// [`template`] for a given C library and processor (`x86_64` or `aarch64`), so the choice of build is testable.
pub(crate) fn template_for(t: &str, v: &str, musl: bool, machine: &str) -> Result<String, String> {
    let (a, a2, a3) = arch_of(machine)?;
    let (node_dist, node_libc) = if musl { ("unofficial-builds.nodejs.org/download/release", "-musl") } else { ("nodejs.org/dist", "") };
    let node_arch = if a == "arm64" { "arm64" } else { "x64" };
    let t = t.replace("{libc_triple}", if musl { "musl" } else { "gnu" }).replace("{cplatform}", &format!("linux-{node_arch}{}", if musl { "-musl" } else { "" })).replace("{arch4}", node_arch).replace("{musl}", if musl { "musl" } else { "" });
    Ok(t.replace("{node_dist}", node_dist).replace("{node_libc}", node_libc).replace("{node_arch}", node_arch).replace("{ve}", &pct_encode_version(v)).replace("{vn}", v.trim_start_matches('v')).replace("{v7}", &v.trim_start_matches('v').replace('.', "")).replace("{arch7}", node_arch).replace("{v}", v).replace("{arduino}", if a == "arm64" { "Linux_ARM64" } else { "Linux_64bit" }).replace("{grpc}", if a == "arm64" { "arm64" } else { "x86_64" }).replace("{arch2}", a2).replace("{arch3}", a3).replace("{arch}", a).replace("{k3s_suffix}", if a == "arm64" { "-arm64" } else { "" }))
}

/// The C library of this machine is musl (the IYAKKI images, Alpine): programs built for glibc do not run.
pub(crate) fn is_musl() -> bool {
    Path::new(&format!("/lib/ld-musl-{}.so.1", std::env::consts::ARCH)).exists()
}

pub(crate) fn extract_hash(text: &str, file: &str) -> Option<String> {
    let valid = |h: &str| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit());
    let mut single = None;
    for l in text.lines() {
        let mut p = l.split_whitespace();
        let (Some(h), name) = (p.next(), p.next()) else { continue };
        if !valid(h) {
            continue;
        }
        match name {
            None => single = Some(h.to_ascii_lowercase()),
            Some(n) if n.trim_start_matches('*').rsplit('/').next() == Some(file) => return Some(h.to_ascii_lowercase()),
            _ => {}
        }
    }
    single
}

impl CliManager {
    pub fn new(root: &Path) -> CliManager {
        CliManager { root: root.into(), streamer: Mutex::new(None), jobs: Mutex::new(HashMap::new()), state_lock: Mutex::new(()), python: Mutex::new("python3".into()), pgp_override: Mutex::new(None), npm: Mutex::new("npm".into()), node: Mutex::new("node".into()), on_installed: Mutex::new(None), system: Mutex::new(HashMap::new()), search: Mutex::new(None), parents: Mutex::new(HashMap::new()), target: Mutex::new(None), detect_present: Mutex::new(false) }
    }

    pub fn set_streamer(&self, s: Arc<dyn Streamer>) {
        *self.streamer.lock().unwrap() = Some(s);
    }

    /// Count programs that are on the machine already as installed (see the field); the shell calls this once.
    pub fn set_detect_present(&self, on: bool) {
        *self.detect_present.lock().unwrap() = on;
    }

    /// Put the tools folder first on this process's search path, so everything IYAKKI starts afterwards (jobs, the terminal, agents,
    /// the cluster) finds what was installed from the catalogue without links in /usr/bin. Idempotent.
    pub fn export_path(&self) {
        let bin = self.bin_dir().display().to_string();
        let cur = std::env::var("PATH").unwrap_or_default();
        if !cur.split(':').any(|d| d == bin) {
            std::env::set_var("PATH", if cur.is_empty() { bin } else { format!("{bin}:{cur}") });
        }
    }

    /// The first program of `id` as found on the machine outside IYAKKI's records (the tools folder first, then the search path and
    /// the usual places), with the version it reports; `None` when nothing runnable is there or detection is off.
    pub fn present_elsewhere(&self, id: &str) -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        if !*self.detect_present.lock().unwrap() || self.is_installed(id).is_some() {
            return None;
        }
        let d = def(id)?;
        if matches!(d.source, Source::System) {
            return None;
        }
        let name = d.bins.first()?.0;
        let path = format!("{}:{}:/usr/local/bin:/usr/bin:/bin", self.bin_dir().display(), self.search_path());
        let found = path.split(':').filter(|p| !p.is_empty()).map(|p| Path::new(p).join(name)).find(|p| p.is_file() && p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0))?;
        let out = run_quiet(&found.display().to_string(), d.version_args, 20).ok()?;
        // the runtimes must be good enough for the tools that use them
        let program = found.display().to_string();
        match id {
            "nodejs" if out.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse::<u32>().ok()).is_none_or(|m| m < 22) => return None,
            "python" if run_quiet(&program, &["-c", "import venv, ensurepip"], 30).is_err() => return None,
            _ => {}
        }
        let line = pick_version_line(&out, id);
        Some(format!("{} ({})", if line.is_empty() { "version not reported".to_string() } else { line }, found.display()))
    }

    /// `None` when an installed tool's programs are in place and can start; otherwise plain words for the list ("installed but not
    /// runnable"): a link that leads nowhere, or a program built for a C library this system does not have.
    pub fn health(&self, id: &str) -> Option<String> {
        let d = def(id)?;
        let i = self.is_installed(id)?;
        for name in &i.bins {
            let link = self.bin_dir().join(name);
            if !link.exists() {
                return Some(format!("{} is recorded as installed but `{name}` is missing; installing it again repairs it", d.name));
            }
            if let Err(e) = runnable_check(&link) {
                return Some(format!("{} is installed but cannot run here: {e}", d.name));
            }
        }
        None
    }

    fn musl_now(&self) -> (bool, &'static str) {
        self.target.lock().unwrap().unwrap_or((is_musl(), std::env::consts::ARCH))
    }

    /// Pick downloads as a machine with this C library and processor would (tests only; the programs still run here).
    #[doc(hidden)]
    pub fn set_target(&self, musl: bool, machine: &'static str) {
        *self.target.lock().unwrap() = Some((musl, machine));
    }

    fn tpl(&self, t: &str, v: &str) -> Result<String, String> {
        match *self.target.lock().unwrap() {
            Some((musl, machine)) => template_for(t, v, musl, machine),
            None => template(t, v),
        }
    }

    /// The PGP key file for a tool's download signature: the one pinned in `legal/` (not yet present while it says `unset`), or a test's.
    fn pgp_key(&self, id: &str) -> Option<Vec<u8>> {
        if let Some((i, k)) = &*self.pgp_override.lock().unwrap() {
            return (i == id).then(|| k.clone());
        }
        pgp_key_file(id).map(|k| k.as_bytes().to_vec())
    }

    /// Tests: use this key file for `id`'s signature check.
    pub fn set_pgp_key(&self, id: &str, key: &[u8]) {
        *self.pgp_override.lock().unwrap() = Some((id.into(), key.to_vec()));
    }

    /// Which Python makes the environment for Python-packaged tools (tests use a stand-in).
    pub fn set_python(&self, p: &str) {
        *self.python.lock().unwrap() = p.into();
    }

    /// Which node and npm an npm install uses (tests use stand-ins).
    pub fn set_npm(&self, npm: &str, node: &str) {
        *self.npm.lock().unwrap() = npm.into();
        *self.node.lock().unwrap() = node.into();
    }

    /// Where programs that come with the system are looked for (tests use a folder of stand-ins).
    pub fn set_search_path(&self, p: Option<&str>) {
        *self.search.lock().unwrap() = p.map(str::to_string);
        self.system.lock().unwrap().clear();
    }

    /// The program search path used to find system tools.
    pub fn search_path(&self) -> String {
        self.search.lock().unwrap().clone().unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
    }

    /// What is on this machine for a catalogue entry whose source is [`Source::System`] (`None` for any other entry). Cached
    /// for a few seconds because the screen asks on every redraw.
    pub fn system_status(&self, id: &str) -> Option<SystemStatus> {
        let d = def(id)?;
        if !matches!(d.source, Source::System) {
            return None;
        }
        if let Some((at, st)) = self.system.lock().unwrap().get(id) {
            if at.elapsed().as_secs() < 20 {
                return Some(st.clone());
            }
        }
        let how_to = crate::ssh::install_hint(std::env::consts::OS);
        let st = match crate::ssh::detect(&self.search_path()) {
            Some(c) => SystemStatus { found: true, label: format!("{} {}", c.flavor.name(), c.version), path: c.path.display().to_string(), commands: c.commands(), how_to },
            None => SystemStatus { found: false, label: "not found on this machine".into(), path: String::new(), commands: vec![], how_to },
        };
        self.system.lock().unwrap().insert(id.to_string(), (std::time::Instant::now(), st.clone()));
        Some(st)
    }

    /// Forget what was found (after something was installed by hand, or in a test).
    pub fn recheck_system(&self) {
        self.system.lock().unwrap().clear();
    }

    /// Called with the tool's id after an install finished well.
    pub fn set_on_installed(&self, f: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.on_installed.lock().unwrap() = Some(f);
    }

    /// The folder an npm tool is installed into (its own `npm_config_prefix`).
    pub fn npm_prefix(&self, id: &str) -> PathBuf {
        // the version folder the install uses: the pin, or "latest" for a tool that follows the vendor
        self.pkg_dir(id).join(def(id).map(|d| d.pin).filter(|p| !p.is_empty()).unwrap_or("latest"))
    }

    /// The program to run for `name`: the one the manager was told to use, or (when it was not told) the catalogue's copy if one is
    /// installed, otherwise the bare name found through `PATH`.
    fn program(&self, configured: &Mutex<String>, name: &str) -> String {
        let c = configured.lock().unwrap().clone();
        let own = self.bin_dir().join(name);
        if c == name && own.exists() { own.display().to_string() } else { c }
    }

    fn npm_program(&self) -> String {
        self.program(&self.npm, "npm")
    }

    fn node_program(&self) -> String {
        self.program(&self.node, "node")
    }

    fn python_program(&self) -> String {
        self.program(&self.python, "python3")
    }

    /// Node's major version, or why npm tools cannot be installed.
    pub fn node_check(&self) -> Result<u32, String> {
        let (node, npm) = (self.node_program(), self.npm_program());
        let hint = "Node.js 22 or newer is best.";
        let out = run_quiet(&node, &["--version"], 20).map_err(|_| format!("Node.js is not installed: Store \u{2192} Setup \u{2192} Get started \u{2192} Node.js (or Install on its line in the CLI tile). {hint}"))?;
        let major: u32 = out.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).ok_or_else(|| format!("Node.js answered `{}`, which was not understood. {hint}", out.trim()))?;
        if major < 18 {
            return Err(format!("Node.js {} is too old (18 or newer is needed). {hint}", out.trim()));
        }
        run_quiet(&npm, &["--version"], 20).map_err(|_| format!("npm is not installed next to Node.js: Store \u{2192} Setup \u{2192} Get started \u{2192} Node.js. {hint}"))?;
        Ok(major)
    }

    /// Python 3 can make environments and has pip.
    pub fn python_check(&self) -> Result<(), String> {
        run_quiet(&self.python_program(), &["-c", "import venv, ensurepip"], 30).map(|_| ()).map_err(|e| if e.starts_with("could not start") { "Python 3 is not installed: Store \u{2192} Setup \u{2192} Get started \u{2192} Python 3 (or Install on its line in the CLI tile).".to_string() } else { format!("Python 3 is installed but cannot make environments with pip ({e}): Store \u{2192} Setup \u{2192} Get started \u{2192} Python 3.") })
    }

    /// The catalogue programs `d` needs and the machine does not have, in the order to install them. An older Node.js than 22 counts
    /// as missing (Claude Code and Copilot ask for it); a catalogue program that is installed already is never installed again here.
    pub fn missing_needs(&self, d: &CliDef) -> Vec<&'static str> {
        let have = |id: &str| self.is_installed(id).is_some();
        match d.source {
            Source::Npm(_) if !have("nodejs") && !self.node_check().is_ok_and(|m| m >= 22) => vec!["nodejs"],
            Source::Pip(_) if !have("python") && self.python_check().is_err() => vec!["python"],
            Source::Python(_) if !have("uv") => vec!["uv"],
            _ => vec![],
        }
    }

    /// What an install will do, in plain words with the exact command, shown before anything runs (`None`: a plain download
    /// from the vendor, described by the tile itself).
    pub fn install_plan(&self, id: &str) -> Option<String> {
        let d = def(id)?;
        let first = |need: &[&str]| -> String {
            let names: Vec<&str> = need.iter().filter_map(|n| def(n)).map(|n| n.name).collect();
            if names.is_empty() { String::new() } else { format!(" {} is not on this machine (or too old), so it is installed first from the CLI catalogue into {} (no sudo).", names.join(" and "), self.root.join("pkgs").display()) }
        };
        match d.source {
            Source::Npm(pkg) => {
                let node = match self.node_check() {
                    Ok(m) if m >= 22 => format!(" Node.js {m} was found."),
                    _ => first(&self.missing_needs(d)),
                };
                Some(format!("Run `npm install -g {}` with the install folder set to {} (npm_config_prefix for that one command only; no sudo, your shell files are untouched). npm downloads from {}.{node}{}", pinned_npm(d, pkg), self.npm_prefix(id).display(), d.hosts.join(", "), d.integrity_warning().map(|w| format!(" {w}")).unwrap_or_default()))
            }
            Source::Pip(pkg) => Some(format!("Create a Python environment in {} and run `pip install {}` in it (no sudo). pip downloads from {}.{}{}", self.pkg_dir(id).display(), if d.pin.is_empty() { pkg.to_string() } else { format!("{pkg}=={}", d.pin) }, d.hosts.join(", "), first(&self.missing_needs(d)), d.integrity_warning().map(|w| format!(" {w}")).unwrap_or_default())),
            Source::File(_) if d.id == "claude-code" => Some(format!("Download Anthropic's own build of Claude Code for this machine from downloads.claude.ai (no Node.js, no sudo): the version is read from Anthropic's release bucket, and the program is checked against the SHA-256 in Anthropic's release manifest before it is kept in {}. If that download cannot be fetched and Node.js is on the machine, `npm install -g @anthropic-ai/claude-code` is run instead (npm_config_prefix for that one command only).", self.pkg_dir(id).display())),
            Source::TarGz(_) if d.id == "github-copilot" => Some(format!("Download GitHub's own standalone build of the Copilot CLI for this machine from github.com/github/copilot-cli/releases (no Node.js, no sudo) and check it against the release's SHA256SUMS.txt before it is kept in {}.", self.pkg_dir(id).display())),
            Source::Setting(key) => Some(format!("Nothing can be downloaded yet: the vendor publishes no build that runs on IYAKKI's images and the address is the setting `{key}`, which is empty until you or the image maker give one (https only, with its SHA-256). Nothing is fetched until then.")),
            Source::Python(v) => Some(format!("Run `uv python install {v}` with the install folder set to {} (no sudo). uv downloads Python from github.com.{}{}", self.pkg_dir(id).display(), first(&self.missing_needs(d)), d.integrity_warning().map(|w| format!(" {w}")).unwrap_or_default())),
            _ => d.integrity_warning(),
        }
    }

    /// Where the signed downloads manifest keeps its last good copy and its check state.
    pub fn manifest_dir(&self) -> PathBuf {
        self.root.join("dlmanifest")
    }

    /// The folder holding a link to every installed program; the chat and the terminal look here first.
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn pkg_dir(&self, id: &str) -> PathBuf {
        self.root.join("pkgs").join(id)
    }

    pub fn installed(&self) -> Vec<Installed> {
        let _g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load()
    }

    fn load(&self) -> Vec<Installed> {
        std::fs::read(self.state_path()).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, Installed>>(&b).ok()).map(|m| m.into_values().collect()).unwrap_or_default()
    }

    fn store(&self, all: &[Installed]) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let map: BTreeMap<&str, &Installed> = all.iter().map(|i| (i.id.as_str(), i)).collect();
        let tmp = self.root.join("state.tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.state_path()).map_err(|e| e.to_string())
    }

    pub fn is_installed(&self, id: &str) -> Option<Installed> {
        self.installed().into_iter().find(|i| i.id == id)
    }

    /// The installed programs the assistant and the terminal may run, with the host each is checked for.
    pub fn extras(&self) -> Vec<Extra> {
        let mut v = Vec::new();
        for i in self.installed() {
            let Some(d) = def(&i.id) else { continue };
            for (name, _) in d.bins.iter().filter(|(n, _)| !n.ends_with("_completer")) {
                if i.bins.iter().any(|b| b == name) {
                    v.push(Extra { name: name.to_string(), host: d.net_host.map(str::to_string) });
                }
            }
        }
        v
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    pub fn running(&self) -> bool {
        self.jobs.lock().unwrap().values().any(|j| !j.finished && j.error.is_none())
    }

    fn set_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let parent = self.parents.lock().unwrap().get(id).cloned();
        let mut jobs = self.jobs.lock().unwrap();
        let seen = jobs.get_mut(id).map(|j| {
            f(j);
            (j.phase.clone(), j.done, j.total)
        });
        if let (Some(p), Some((phase, done, total))) = (parent, seen) {
            if let Some(pj) = jobs.get_mut(&p) {
                pj.phase = format!("first, {}: {phase}", def(id).map(|d| d.name).unwrap_or(id));
                (pj.done, pj.total) = (done, total);
            }
        }
    }

    /// Starts installing `id` (or reinstalling the latest version) and returns at once; `job` follows it.
    pub fn install(self: &Arc<Self>, id: &str, by: &str) -> Result<(), String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list: {}", CATALOG.iter().map(|d| d.id).collect::<Vec<_>>().join(", ")))?;
        if matches!(d.source, Source::System) {
            return Err(format!("{} is not downloaded: it comes with the system. {}", d.name, crate::ssh::install_hint(std::env::consts::OS)));
        }
        arch()?;
        let (musl, machine) = self.musl_now();
        if musl {
            if let Some(why) = musl_unavailable(id, machine) {
                return Err(format!("{} is not available on this image: {why}", d.name));
            }
        }
        let streamer = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up for downloads")?;
        // already on the machine (put there by hand): nothing to download, and the list says "installed" with its version
        if let Some(here) = self.present_elsewhere(id) {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.values().any(|j| !j.finished && j.error.is_none()) {
                return Err("another install is still running; wait for it to finish".into());
            }
            jobs.insert(id.into(), Job { id: id.into(), phase: format!("installed \u{b7} {here}"), done: 0, total: 0, finished: true, error: None });
            drop(jobs);
            if let Some(h) = self.on_installed.lock().unwrap().clone() {
                h(id);
            }
            return Ok(());
        }
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.values().any(|j| !j.finished && j.error.is_none()) {
                return Err("another install is still running; wait for it to finish".into());
            }
            jobs.insert(id.into(), Job { id: id.into(), phase: "starting".into(), done: 0, total: 0, finished: false, error: None });
        }
        let (me, by) = (self.clone(), by.to_string());
        std::thread::Builder::new().name("iyakki-cli-install".into()).spawn(move || {
            let r = me.run_install(d, &*streamer, &by);
            let ok = r.is_ok();
            me.set_job(d.id, |j| match r {
                Ok(()) => {
                    j.finished = true;
                    j.phase = "installed".into();
                }
                Err(e) => j.error = Some(e),
            });
            if ok {
                let hook = me.on_installed.lock().unwrap().clone();
                if let Some(h) = hook {
                    h(d.id);
                }
            }
        }).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn open_follow(&self, st: &dyn Streamer, url: &str, hosts: &[&str]) -> Result<(iyakki_proxy::StreamHead, String), String> {
        let mut url = url.to_string();
        for _ in 0..6 {
            host_ok(&url, hosts)?;
            let head = st.open(&url, &[]).map_err(|e| format!("could not reach {url}: {e}"))?;
            if (300..400).contains(&head.status) {
                let loc = head.header("location").ok_or("a redirect without a place to go")?;
                url = resolve(&url, loc).ok_or("an unusable redirect")?;
                continue;
            }
            if head.status != 200 {
                return Err(format!("{url} answered {}", head.status));
            }
            return Ok((head, url));
        }
        Err("too many redirects".into())
    }

    pub(crate) fn small(&self, st: &dyn Streamer, url: &str, hosts: &[&str]) -> Result<String, String> {
        self.small_max(st, url, hosts, 256 * 1024)
    }

    fn small_max(&self, st: &dyn Streamer, url: &str, hosts: &[&str], max: u64) -> Result<String, String> {
        let (head, _) = self.open_follow(st, url, hosts)?;
        let mut s = String::new();
        head.body.take(max).read_to_string(&mut s).map_err(|e| e.to_string())?;
        Ok(s)
    }

    pub(crate) fn latest_version(&self, d: &CliDef, st: &dyn Streamer) -> Result<String, String> {
        if !d.pin.is_empty() {
            return Ok(d.pin.to_string());
        }
        // a signed downloads manifest (docs/development/DOWNLOADS-MANIFEST.md) may name the version; only for the items wired to it so far
        if let Some(v) = crate::dlmanifest::version_override(&self.manifest_dir(), d.id) {
            return Ok(v);
        }
        match d.version {
            VersionFrom::Text(u) => {
                let v = self.small(st, u, d.hosts)?.trim().to_string();
                if v.is_empty() || v.len() > 40 || v.contains(char::is_whitespace) {
                    return Err("the version answer was not understood".into());
                }
                Ok(v)
            }
            VersionFrom::Json(u, ptr) => {
                let t = self.small(st, &self.tpl(u, "")?, d.hosts)?;
                let j: serde_json::Value = serde_json::from_str(&t).map_err(|_| "the version answer was not understood".to_string())?;
                j.pointer(ptr).and_then(|v| v.as_str()).map(str::to_string).filter(|v| !v.is_empty() && v.len() < 40).ok_or_else(|| "the version answer was not understood".into())
            }
            VersionFrom::Redirect(u) => {
                host_ok(u, d.hosts)?;
                let head = st.open(u, &[]).map_err(|e| format!("could not reach {u}: {e}"))?;
                let loc = head.header("location").ok_or("the latest-release link did not name a version")?;
                version_from_location(loc).ok_or_else(|| "the latest version could not be read from the release link".into())
            }
            VersionFrom::NodeLts(u) => {
                let t = self.small_max(st, &self.tpl(u, "")?, d.hosts, 2 << 20)?;
                let want = self.tpl("{cplatform}", "")?;
                node_lts_pick(&t, &want).ok_or_else(|| "the Node.js release list has no long-term-support build for this machine".into())
            }
            VersionFrom::Latest => Ok("latest".into()),
        }
    }

    fn run_install(&self, d: &'static CliDef, st: &dyn Streamer, by: &str) -> Result<(), String> {
        let fail = |e: String| -> String { e };
        for need in self.missing_needs(d) {
            let nd = def(need).ok_or_else(|| format!("`{need}` is not in the list"))?;
            self.install_dependency(nd, d, st, by)?;
        }
        self.set_job(d.id, |j| j.phase = "finding the latest version".into());
        // a vendor's native build whose address cannot even be asked for its version may still have the npm fallback
        let mut native_failed: Option<String> = None;
        let version = match self.latest_version(d, st) {
            Ok(v) => v,
            Err(e) if npm_fallback(d.id).is_some() && self.node_check().is_ok() => {
                native_failed = Some(e);
                "latest".to_string()
            }
            Err(e) => return Err(fail(e)),
        };
        let tmp = self.root.join("tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let pkg = self.pkg_dir(d.id);
        let old = self.is_installed(d.id);
        // folders left by an install that was cut short are not the current or the previous version: they go
        let keep: Vec<String> = old.iter().flat_map(|o| [self.dir_of(o)].into_iter().chain(o.previous.iter().map(|p| p.dir.clone()))).collect();
        if let Ok(rd) = std::fs::read_dir(&pkg) {
            for e in rd.flatten() {
                if !keep.contains(&e.file_name().to_string_lossy().into_owned()) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let target = self.fresh_dir(d.id, &version.replace('/', "_"));
        let mut verified = false;
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let result: Result<(), String> = (|| {
            if let Some(e) = &native_failed {
                return Err(e.clone());
            }
            match d.source {
                Source::Pip(pkgname) => {
                    self.set_job(d.id, |j| j.phase = format!("installing {pkgname} with pip (a few minutes)"));
                    let py = self.python_program();
                    run_quiet(&py, &["-m", "venv", &target.join("venv").display().to_string()], 120)?;
                    // DuckDB: only ready-made wheels (musl systems have no compiler), and its command is our small program
                    let mut args = vec!["install", "--no-input", "--disable-pip-version-check"];
                    if d.id == "duckdb" {
                        args.push("--only-binary=:all:");
                    }
                    // hash values in the signed manifest for this exact version: every file pip installs must match one (including dependencies)
                    let lock_file = target.join("requirements.lock");
                    let lock = if d.pin.is_empty() { None } else { crate::dlmanifest::pins::pip_lock(&self.manifest_dir(), d.id, pkgname, d.pin) };
                    let lock_arg = lock_file.display().to_string();
                    let spec = if d.pin.is_empty() { pkgname.to_string() } else { format!("{pkgname}=={}", d.pin) };
                    if let Some(text) = &lock {
                        std::fs::write(&lock_file, text).map_err(|e| e.to_string())?;
                        args.extend(["--require-hashes", "-r", lock_arg.as_str()]);
                        verified = true;
                    } else {
                        args.push(&spec);
                    }
                    run_quiet(&target.join("venv/bin/pip").display().to_string(), &args, 1800)?;
                    if d.id == "duckdb" {
                        write_duckdb_shim(&target)?;
                    }
                }
                Source::Python(spec) => {
                    self.set_job(d.id, |j| j.phase = format!("downloading Python {spec} with uv (a minute or two)"));
                    let uv = self.bin_dir().join("uv").display().to_string();
                    let (dir, shims, cache) = (target.display().to_string(), target.join("shims").display().to_string(), tmp.join("uv-cache").display().to_string());
                    // uv keeps everything in this tool's folder; its own `python3.12` link goes in `shims`, not in the person's ~/.local/bin
                    let env = [("UV_PYTHON_INSTALL_DIR", dir.as_str()), ("UV_PYTHON_BIN_DIR", shims.as_str()), ("UV_CACHE_DIR", cache.as_str()), ("UV_NO_PROGRESS", "1")];
                    run_quiet_env(&uv, &["python", "install", spec], &env, 900)?;
                }
                Source::System => return Err(format!("{} comes with the system and is not downloaded", d.name)),
                Source::Setting(key) => return Err(format!("{} has no download address yet: the OpenVPN project publishes source and distribution packages but no build that runs on IYAKKI's musl images, and IYAKKI does not invent one. Set `{key}` (an https address of a build you trust, with its SHA-256) to enable the download. Nothing was fetched.", d.name)),
                Source::Npm(pkgname) => self.npm_install(d, &pinned_npm(d, pkgname), &target, &tmp)?,
                Source::File(u) | Source::TarGz(u) | Source::Zip(u) | Source::TarZst(u) | Source::TarXz(u) => {
                    let url = self.tpl(u, &version)?;
                    let file = pct_decode(url.rsplit('/').next().unwrap_or("download"));
                    // a manifest names this machine's build before anything large is fetched
                    let manifest_want = match d.checksum {
                        Some(Checksum::Manifest(mu)) => {
                            self.set_job(d.id, |j| j.phase = "reading the release manifest".into());
                            let text = self.small(st, &self.tpl(mu, &version)?, d.hosts)?;
                            Some(manifest_checksum(&text, &self.tpl("{cplatform}", &version)?).ok_or("the vendor's release manifest has no entry for this machine")?)
                        }
                        _ => None,
                    };
                    self.set_job(d.id, |j| j.phase = "downloading".into());
                    let (head, final_url) = self.open_follow(st, &url, d.hosts)?;
                    let total = head.header("content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    let (max_download, max_unpacked) = d.limits();
                    if total > max_download {
                        return Err(format!("the download is {} MB, more than the {} MB allowed", total >> 20, max_download >> 20));
                    }
                    self.set_job(d.id, |j| j.total = total);
                    let dl = tmp.join(format!("{}.download", d.id));
                    let mut out = std::io::BufWriter::new(std::fs::File::create(&dl).map_err(|e| e.to_string())?);
                    let (mut hasher, mut body, mut got) = (Sha256::new(), head.body, 0u64);
                    let mut md5 = matches!(d.checksum, Some(Checksum::Md5(_))).then(crate::xz::Md5::new);
                    // a detached PGP signature (aws-cli): fetched before the large download, checked against the key pinned in legal/
                    let pgp_key = self.pgp_key(d.id);
                    let mut pgp = match &pgp_key {
                        Some(_) => {
                            self.set_job(d.id, |j| j.phase = "reading the vendor's signature".into());
                            let (sig, _) = self.open_follow(st, &format!("{url}.sig"), d.hosts)?;
                            let mut raw = Vec::new();
                            sig.body.take(64 * 1024).read_to_end(&mut raw).map_err(|e| format!("could not read the signature: {e}"))?;
                            Some(crate::openpgp::Hasher::start(&raw).map_err(|e| format!("the vendor's signature file cannot be used: {e}"))?)
                        }
                        None => None,
                    };
                    let mut buf = vec![0u8; 128 * 1024];
                    loop {
                        let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
                        if n == 0 {
                            break;
                        }
                        got += n as u64;
                        if got > max_download {
                            return Err("the download is larger than allowed".into());
                        }
                        hasher.update(&buf[..n]);
                        if let Some(m) = &mut md5 {
                            m.update(&buf[..n]);
                        }
                        if let Some(p) = &mut pgp {
                            p.update(&buf[..n]);
                        }
                        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                        self.set_job(d.id, |j| j.done = got);
                    }
                    out.flush().map_err(|e| e.to_string())?;
                    if total > 0 && got != total {
                        return Err(format!("the download stopped at {} of {} MB", got >> 20, total >> 20));
                    }
                    let got_hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                    if let Some(c) = d.checksum {
                        self.set_job(d.id, |j| j.phase = "checking the checksum".into());
                        let want = match (c, manifest_want) {
                            (Checksum::Single(u) | Checksum::Lines(u), _) => {
                                let text = self.small(st, &self.tpl(u, &version)?, d.hosts)?;
                                extract_hash(&text, &file).ok_or("the vendor's checksum file has no entry for this download")?
                            }
                            (Checksum::Fixed(table), _) => fixed_hash(table, &file).ok_or("this pinned release has no published checksum for this machine's build")?,
                            (Checksum::Md5(u), _) => {
                                let text = self.small(st, &self.tpl(u, &version)?, d.hosts)?;
                                let want = crate::xz::md5_from(&text, &file).ok_or("the vendor's checksum file has no entry for this download")?;
                                let got = md5.take().map(|m| m.hex()).unwrap_or_default();
                                if want != got {
                                    return Err(format!("the download does not match the vendor's checksum (expected {}…, got {}…); nothing was installed", &want[..12], &got[..12.min(got.len())]));
                                }
                                got_hash.clone()
                            }
                            (Checksum::Manifest(_), Some(w)) => w,
                            (Checksum::Manifest(_), None) => return Err("the vendor's release manifest was not read".into()),
                        };
                        if want != got_hash {
                            return Err(format!("the download does not match the vendor's checksum (expected {}…, got {}…); nothing was installed", &want[..12], &got_hash[..12]));
                        }
                        verified = true;
                    }
                    // our own SHA-256 in the signed manifest (for this exact version and file name) must match too
                    if let Some(want) = crate::dlmanifest::pins::sha256_for(&self.manifest_dir(), d.id, &version, &file) {
                        if want != got_hash {
                            return Err(format!("the download does not match the SHA-256 pinned by IYAKKI (expected {}…, got {}…); nothing was installed", &want[..12], &got_hash[..12]));
                        }
                        verified = true;
                    }
                    if let (Some(p), Some(key)) = (pgp.take(), &pgp_key) {
                        self.set_job(d.id, |j| j.phase = "checking the vendor's signature".into());
                        p.finish().and_then(|f| f.check(key)).map_err(|e| format!("the download does not match the vendor's PGP signature ({e}); nothing was installed"))?;
                        verified = true;
                    }
                    self.set_job(d.id, |j| j.phase = "unpacking".into());
                    let _ = final_url;
                    match d.source {
                        Source::File(_) => {
                            let dst = target.join(d.bins[0].1);
                            if let Some(parent) = dst.parent() {
                                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                            }
                            std::fs::copy(&dl, &dst).map_err(|e| e.to_string())?;
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
                        }
                        Source::TarGz(_) => {
                            crate::archive::untar_gz(&dl, &target, &tmp)?;
                        }
                        Source::Zip(_) => {
                            crate::archive::unzip(&dl, &target)?;
                        }
                        Source::TarZst(_) => {
                            crate::archive::untar_zst(&dl, &target, max_unpacked)?;
                        }
                        Source::TarXz(_) => {
                            crate::xz::untar_xz(&dl, &target, &tmp, max_unpacked)?;
                        }
                        Source::Pip(_) | Source::Npm(_) | Source::Python(_) | Source::System | Source::Setting(_) => {}
                    }
                    let _ = std::fs::remove_file(&dl);
                    if d.id == "nodejs" {
                        self.fit_node(d, st, &target, &tmp)?;
                    }
                }
            }
            Ok(())
        })();
        // Claude Code: the native build is the way; the npm package is the fallback, only when it could not be fetched (never after a
        // checksum mismatch) and a Node.js that npm can use is already on the machine
        let mut version = version;
        let result = match (result, npm_fallback(d.id)) {
            (Err(e), Some(pkg)) if !e.contains("checksum") && self.node_check().is_ok() => {
                let _ = std::fs::remove_file(tmp.join(format!("{}.download", d.id)));
                let _ = std::fs::remove_dir_all(&target);
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                self.set_job(d.id, |j| j.done = 0);
                verified = false;
                version = "latest".into();
                self.npm_install(d, pkg, &target, &tmp).map_err(|e2| format!("{e}; the npm fallback failed too: {e2}"))
            }
            (r, _) => r,
        };
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&target);
            let _ = std::fs::remove_file(tmp.join(format!("{}.download", d.id)));
            return Err(e);
        }
        // find the programs
        self.set_job(d.id, |j| j.phase = "checking the new version".into());
        let mut found: Vec<(&str, PathBuf)> = Vec::new();
        for (idx, (name, rel)) in d.bins.iter().enumerate() {
            match find_in(&target, rel) {
                Some(src) => found.push((name, src)),
                // only the first program has to be there; the others come and go between versions
                None if idx > 0 => {}
                None => {
                    let _ = std::fs::remove_dir_all(&target);
                    return Err(format!("the unpacked download has no `{rel}`"));
                }
            }
        }
        // a program built for another C library does not even start; say so in plain words and install nothing
        let said_version = if version == "latest" { String::new() } else { format!(" {version}") };
        for (_, src) in &found {
            if let Err(e) = runnable_check(src) {
                let _ = std::fs::remove_dir_all(&target);
                return Err(format!("{}{said_version} was downloaded but cannot run here: {e}", d.name));
            }
        }
        // the program says what it is. When a working version is already installed, the new one must run before it replaces
        // the old: a version that does not even start never takes over.
        let said = run_quiet(&found[0].1.display().to_string(), d.version_args, 60);
        if let (Err(e), Some(o)) = (&said, &old) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(format!("the new version does not start ({e}); {} {} stays installed", d.name, o.version));
        }
        // a vendor's own program that cannot even print its version is the wrong build for this machine: nothing is installed
        if let (Err(e), true) = (&said, matches!(d.id, "claude-code" | "github-copilot") || said.as_ref().is_err_and(|e| e.starts_with("could not start"))) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(format!("{} was downloaded and checked but does not start on this machine ({e}); nothing was installed", d.name));
        }
        let reported = said.map(|o| pick_version_line(&o, d.id)).unwrap_or_default();
        let shown = if version == "latest" { reported.split(|c: char| c.is_whitespace() || c == '/').find(|w| w.trim_start_matches('v').chars().next().is_some_and(|c| c.is_ascii_digit())).unwrap_or("latest").to_string() } else { version.clone() };
        let new_dir = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let previous = old.as_ref().map(|o| Previous { version: o.version.clone(), dir: self.dir_of(o), reported: o.reported.clone(), bins: o.bins.clone(), verified: o.verified, at: o.at });
        // the record first, then the links: a cut-off in between is mended by `repair_links` from the record
        {
            let _g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut all = self.load();
            all.retain(|i| i.id != d.id);
            all.push(Installed { id: d.id.into(), version: shown, reported, at: crate::clock::now_unix(), bins: found.iter().map(|f| f.0.to_string()).collect(), verified, by: by.into(), dir: new_dir.clone(), previous: previous.clone() });
            self.store(&all)?;
        }
        self.set_job(d.id, |j| j.phase = "linking".into());
        self.link_all(&found)?;
        // keep this version and the one before it; every other folder goes
        if let Ok(rd) = std::fs::read_dir(&pkg) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n != new_dir && previous.as_ref().is_none_or(|p| p.dir != n) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        Ok(())
    }

    /// Downloads an address an administrator typed (https only, redirects stay on its host) to `dest`; returns the SHA-256 of what came.
    /// Used by the browser add-on's mirror (`avaddon_install.rs`): the host is the administrator's own, so it is the only one allowed.
    pub fn fetch_admin_file(&self, url: &str, dest: &Path) -> Result<String, String> {
        let st = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up for downloads")?;
        let host = url.strip_prefix("https://").and_then(|r| r.split(['/', ':', '?', '#']).next()).unwrap_or("").to_ascii_lowercase();
        let (head, _) = self.open_follow(&*st, url, &[host.as_str()])?;
        if head.status != 200 {
            return Err(format!("the mirror answered {}", head.status));
        }
        const MAX: u64 = 64 << 20;
        let mut body = head.body.take(MAX + 1);
        let mut out = std::io::BufWriter::new(std::fs::File::create(dest).map_err(|e| e.to_string())?);
        let (mut h, mut buf, mut got) = (Sha256::new(), vec![0u8; 64 * 1024], 0u64);
        loop {
            let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > MAX {
                return Err("the download is larger than 64 MB".into());
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// `npm install -g` of one package into `target` (the child's `npm_config_prefix`), with the catalogue's Node.js first on the search path.
    fn npm_install(&self, d: &CliDef, pkgname: &str, target: &Path, tmp: &Path) -> Result<(), String> {
        self.set_job(d.id, |j| j.phase = format!("installing {pkgname} with npm (a minute or two)"));
        let npm = self.npm_program();
        let cache = tmp.join("npm-cache").display().to_string();
        let prefix = target.display().to_string();
        // npm's scripts start `node` through the shell's search path, which must find the catalogue's copy
        let path = format!("{}:{}", self.bin_dir().display(), std::env::var("PATH").unwrap_or_default());
        // the prefix and the cache reach the child only; the person's npm settings and shell files are not touched
        let env = [("npm_config_prefix", prefix.as_str()), ("npm_config_cache", cache.as_str()), ("npm_config_update_notifier", "false"), ("npm_config_fund", "false"), ("npm_config_audit", "false"), ("PATH", path.as_str())];
        run_quiet_env(&npm, &["install", "-g", pkgname], &env, 900).map(|_| ())
    }

    /// Installs `dep` (needed by `parent`) as a part of the parent's install: its progress shows on the parent's job, and the manager
    /// runs it from then on.
    fn install_dependency(&self, dep: &'static CliDef, parent: &CliDef, st: &dyn Streamer, by: &str) -> Result<(), String> {
        self.jobs.lock().unwrap().insert(dep.id.into(), Job { id: dep.id.into(), phase: "starting".into(), done: 0, total: 0, finished: false, error: None });
        self.parents.lock().unwrap().insert(dep.id.into(), parent.id.into());
        let r = self.run_install(dep, st, by);
        self.parents.lock().unwrap().remove(dep.id);
        self.set_job(dep.id, |j| match &r {
            Ok(()) => {
                j.finished = true;
                j.phase = "installed".into();
            }
            Err(e) => j.error = Some(e.clone()),
        });
        r.map_err(|e| format!("{} is needed first for {} and could not be installed: {e}", dep.name, parent.name))?;
        let own = |n: &str| self.bin_dir().join(n).display().to_string();
        match dep.id {
            "nodejs" => {
                *self.node.lock().unwrap() = own("node");
                *self.npm.lock().unwrap() = own("npm");
            }
            "python" => *self.python.lock().unwrap() = own("python3"),
            _ => {}
        }
        let hook = self.on_installed.lock().unwrap().clone();
        if let Some(h) = hook {
            h(dep.id);
        }
        Ok(())
    }

    /// Node's musl build needs `libstdc++.so.6`, which minimal images lack. When `node` will not start for that reason, the library is
    /// fetched from Alpine into the Node folder and `bin/node` becomes a launcher that points the loader at it.
    fn fit_node(&self, d: &CliDef, st: &dyn Streamer, target: &Path, tmp: &Path) -> Result<(), String> {
        let Some(node) = find_in(target, "*/bin/node") else { return Ok(()) };
        match run_quiet(&node.display().to_string(), &["--version"], 30) {
            Err(e) if e.contains("libstdc++") || e.contains("shared library") || e.contains("relocating") => {}
            // it starts, or fails for another reason, which the check after unpacking reports
            _ => return Ok(()),
        }
        self.set_job(d.id, |j| j.phase = "fetching libstdc++ from Alpine (Node.js needs it)".into());
        let lib = self.fetch_libstdcpp(st, tmp)?;
        let dir = target.join("libstdcpp");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::copy(&lib, dir.join("libstdc++.so.6")).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(tmp.join("libstdcpp-unpack"));
        let real = node.with_file_name("node.bin");
        std::fs::rename(&node, &real).map_err(|e| e.to_string())?;
        // the launcher finds its own folder through any link to it (the links in the tools folder), and the variable reaches node's children too
        let script = "#!/bin/sh\nd=$(dirname \"$(readlink -f \"$0\")\")\nexport LD_LIBRARY_PATH=\"$d/../../libstdcpp${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}\"\nexec \"$d/node.bin\" \"$@\"\n";
        std::fs::write(&node, script).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
    }

    /// The real `libstdc++.so.6.x` file out of Alpine's current stable `libstdc++` package, unpacked under `tmp`.
    fn fetch_libstdcpp(&self, st: &dyn Streamer, tmp: &Path) -> Result<PathBuf, String> {
        let hosts = ["alpinelinux.org"];
        let base = format!("https://dl-cdn.alpinelinux.org/alpine/latest-stable/main/{}/", std::env::consts::ARCH);
        let listing = self.small_max(st, &base, &hosts, 8 << 20)?;
        let name = pick_libstdcpp(&listing).ok_or("Alpine's package list has no libstdc++ for this processor")?;
        let (head, _) = self.open_follow(st, &format!("{base}{name}"), &hosts)?;
        let mut apk = Vec::new();
        head.body.take(16 << 20).read_to_end(&mut apk).map_err(|e| format!("the libstdc++ download broke off: {e}"))?;
        let members = crate::archive::gunzip_members(&apk, 64 << 20)?;
        let files = members.last().ok_or("the libstdc++ package is empty")?;
        let scratch = tmp.join("libstdcpp-unpack");
        let _ = std::fs::remove_dir_all(&scratch);
        crate::archive::untar_stream(std::io::Cursor::new(files), &scratch, 64 << 20)?;
        let found = std::fs::read_dir(scratch.join("usr/lib")).map_err(|e| e.to_string())?.flatten().find(|e| e.file_type().is_ok_and(|t| t.is_file()) && e.file_name().to_string_lossy().starts_with("libstdc++.so.6."));
        found.map(|e| e.path()).ok_or_else(|| "the libstdc++ package has no library in it".to_string())
    }

    /// Point the command links at these programs, each swapped in one rename so a command is never missing.
    fn link_all(&self, found: &[(&str, PathBuf)]) -> Result<(), String> {
        let bindir = self.bin_dir();
        std::fs::create_dir_all(&bindir).map_err(|e| e.to_string())?;
        for (name, src) in found {
            let tmp = bindir.join(format!(".{name}.new"));
            let _ = std::fs::remove_file(&tmp);
            std::os::unix::fs::symlink(src, &tmp).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, bindir.join(name)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The folder (under `pkgs/<id>`) a recorded version lives in.
    pub(crate) fn dir_of(&self, i: &Installed) -> String {
        if !i.dir.is_empty() {
            return i.dir.clone();
        }
        let v = i.version.replace('/', "_");
        if self.pkg_dir(&i.id).join(&v).is_dir() { v } else { "latest".into() }
    }

    /// A folder name for a new install of `base` that nothing uses yet (`latest`, then `latest.2`, ...): an update never
    /// writes into the folder of the version that is running.
    fn fresh_dir(&self, id: &str, base: &str) -> PathBuf {
        let pkg = self.pkg_dir(id);
        let first = pkg.join(base);
        if !first.exists() {
            return first;
        }
        (2..).map(|n| pkg.join(format!("{base}.{n}"))).find(|p| !p.exists()).unwrap_or(first)
    }

    /// Make the command links agree with the record (after a cut-off between writing the record and swapping the links).
    /// Returns what was mended.
    pub fn repair_links(&self, id: &str) -> Vec<String> {
        let Some(d) = def(id) else { return vec![] };
        let Some(i) = self.is_installed(id) else { return vec![] };
        let base = self.pkg_dir(id).join(self.dir_of(&i));
        let mut mended = Vec::new();
        for (name, rel) in d.bins.iter().filter(|(n, _)| i.bins.iter().any(|b| b == n)) {
            let Some(src) = find_in(&base, rel) else { continue };
            if std::fs::read_link(self.bin_dir().join(name)).ok().as_deref() != Some(src.as_path()) && self.link_all(&[(name, src)]).is_ok() {
                mended.push(name.to_string());
            }
        }
        mended
    }

    /// Every installed tool's links, mended (run when the store opens).
    pub fn repair_all(&self) -> Vec<String> {
        self.installed().iter().flat_map(|i| self.repair_links(&i.id)).collect()
    }

    /// Go back to the version that was installed before (kept for one generation). The two swap places, so going forward again is
    /// the same call. The older program must start before anything is changed.
    pub fn rollback(&self, id: &str) -> Result<String, String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list"))?;
        if self.running() {
            return Err("an install is running; wait for it to finish".into());
        }
        let _g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        let cur = all.iter().find(|i| i.id == id).cloned().ok_or_else(|| format!("{} is not installed", d.name))?;
        let prev = cur.previous.clone().ok_or_else(|| format!("{} has no earlier version kept to go back to", d.name))?;
        let base = self.pkg_dir(id).join(&prev.dir);
        let mut found: Vec<(&str, PathBuf)> = Vec::new();
        for (name, rel) in d.bins.iter().filter(|(n, _)| prev.bins.iter().any(|b| b == n)) {
            if let Some(src) = find_in(&base, rel) {
                found.push((name, src));
            }
        }
        if found.is_empty() {
            return Err(format!("the kept copy of {} {} is gone or damaged; nothing was changed", d.name, prev.version));
        }
        run_quiet(&found[0].1.display().to_string(), d.version_args, 60).map_err(|e| format!("the earlier version does not start ({e}); nothing was changed"))?;
        let swapped = Installed { version: prev.version.clone(), reported: prev.reported.clone(), bins: prev.bins.clone(), verified: prev.verified, at: prev.at, dir: prev.dir.clone(), previous: Some(Previous { version: cur.version.clone(), dir: self.dir_of(&cur), reported: cur.reported.clone(), bins: cur.bins.clone(), verified: cur.verified, at: cur.at }), ..cur.clone() };
        all.retain(|i| i.id != id);
        all.push(swapped);
        self.store(&all)?;
        self.link_all(&found)?;
        Ok(format!("{} is back at {} (it was {}).", d.name, prev.version, cur.version))
    }

    /// What the vendor publishes as the newest version (a network call: not on the screen's thread). Tools whose vendor
    /// publishes only "latest" have no number to compare, and say so.
    pub fn check_latest(&self, id: &str) -> Result<String, String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list"))?;
        if matches!(d.source, Source::System) {
            return Err(format!("{} comes with the system and is updated by its package manager, not from here.", d.name));
        }
        if !d.pin.is_empty() {
            // this release of IYAKKI installs one exact version (see `CliDef::pin`); a newer one arrives with a newer IYAKKI
            return Ok(d.pin.to_string());
        }
        if matches!(d.version, VersionFrom::Latest) {
            // no number from the vendor's own address, but the registry the tool installs from has one
            let lookup = match d.source {
                Source::Npm(_) | Source::Pip(_) | Source::Python(_) => true,
                _ => false,
            };
            if !lookup {
                return Err(format!("{} publishes only \"latest\", with no version number to compare, so IYAKKI cannot tell whether yours is behind. Updating installs the newest.", d.name));
            }
            let st = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up")?;
            let ok = |v: Option<String>| v.filter(|v| !v.is_empty() && v.len() < 40).ok_or_else(|| "the version answer was not understood".to_string());
            return match d.source {
                Source::Npm(pkg) => {
                    let t = self.small(&*st, &format!("https://registry.npmjs.org/{}/latest", pkg.replace('/', "%2F")), d.hosts)?;
                    ok(serde_json::from_str::<serde_json::Value>(&t).ok().and_then(|j| j.get("version").and_then(|v| v.as_str()).map(str::to_string)))
                }
                Source::Pip(pkg) => {
                    let t = self.small_max(&*st, &format!("https://pypi.org/pypi/{pkg}/json"), d.hosts, 16 << 20)?;
                    let j = serde_json::from_str::<serde_json::Value>(&t).ok();
                    let (musl, machine) = self.musl_now();
                    // DuckDB: on musl the newest version that has a ready-made musl wheel for this Python (pip's choice with --only-binary)
                    if pkg == "duckdb" && musl {
                        ok(j.and_then(|j| pypi_newest_with_wheel(&j, &format!("musllinux_1_2_{machine}"), "cp312")))
                    } else {
                        ok(j.and_then(|j| j.pointer("/info/version").and_then(|v| v.as_str()).map(str::to_string)))
                    }
                }
                Source::Python(series) => {
                    let t = self.small(&*st, "https://github.com/astral-sh/python-build-standalone/releases/latest/download/SHA256SUMS", d.hosts)?;
                    let (musl, machine) = self.target.lock().unwrap().unwrap_or((is_musl(), std::env::consts::ARCH));
                    ok(pbs_pick(&t, series, machine, musl))
                }
                _ => unreachable!(),
            };
        }
        let st = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up")?;
        self.latest_version(d, &*st)
    }

    /// What the tile offers for `id`: Install when it is not there, Upgrade when `latest` (from [`CliManager::check_latest`], a network
    /// call to make off the screen's thread) is newer than what is installed, otherwise nothing to do. Installing and upgrading are the
    /// same detached job ([`CliManager::install`]); a missing runtime (Node.js, Python, uv) is installed first by that job.
    pub fn offer(&self, id: &str, latest: Option<&str>) -> Offer {
        match (self.is_installed(id), latest) {
            (None, _) => self.present_elsewhere(id).map_or(Offer::Install, Offer::Elsewhere),
            (Some(i), Some(l)) if i.version != "latest" && version_is_newer(&i.version, l) => Offer::Upgrade(l.to_string()),
            _ => Offer::UpToDate,
        }
    }

    /// The ids an install of `id` runs, in order: the runtimes it still needs (Node.js, or uv then Python), then `id` itself.
    pub fn install_order(&self, id: &str) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        let Some(d) = def(id) else { return out };
        for n in self.missing_needs(d) {
            for x in self.install_order(n) {
                if !out.contains(&x) {
                    out.push(x);
                }
            }
        }
        out.push(d.id);
        out
    }

    /// The installed tools that stop working when `id` is removed (Node.js: the npm tools; Python: the pip tools; uv: nothing that
    /// runs, only Python updates).
    pub fn needed_by(&self, id: &str) -> Vec<&'static str> {
        self.installed().iter().filter_map(|i| def(&i.id)).filter(|d| runtime_for(d) == Some(id) && id != "uv").map(|d| d.id).collect()
    }

    /// A sentence to show before removing `id` when installed tools use it (`None`: nothing depends on it).
    pub fn uninstall_warning(&self, id: &str) -> Option<String> {
        let names: Vec<&str> = self.needed_by(id).into_iter().filter_map(def).map(|d| d.name).collect();
        let me = def(id)?;
        (!names.is_empty()).then(|| format!("{} is used by {}; they stop working if it is removed (installing one of them again brings {} back first).", me.name, names.join(", "), me.name))
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list"))?;
        if matches!(d.source, Source::System) {
            return Err(format!("{} comes with the system; there is nothing here to remove.", d.name));
        }
        if self.running() {
            return Err("an install is running; wait for it to finish".into());
        }
        let _g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        if !all.iter().any(|i| i.id == id) {
            return Err(format!("{} is not installed", d.name));
        }
        for (name, _) in d.bins {
            let _ = std::fs::remove_file(self.bin_dir().join(name));
        }
        let _ = std::fs::remove_dir_all(self.pkg_dir(id));
        all.retain(|i| i.id != id);
        self.jobs.lock().unwrap().remove(id);
        self.store(&all)
    }
}

/// The newest release in a Node.js `index.json` that belongs to a long-term-support line and lists `platform` (`linux-x64-musl`)
/// among its files. The list is newest first.
fn node_lts_pick(index: &str, platform: &str) -> Option<String> {
    let j: serde_json::Value = serde_json::from_str(index).ok()?;
    j.as_array()?.iter().find_map(|r| {
        let lts = r.get("lts").is_some_and(|l| l.as_str().is_some_and(|s| !s.is_empty()));
        let has = r.get("files").and_then(|f| f.as_array()).is_some_and(|f| f.iter().any(|x| x.as_str() == Some(platform)));
        let v = r.get("version")?.as_str()?;
        (lts && has && v.len() < 40 && v.starts_with('v')).then(|| v.to_string())
    })
}

/// The file name (as it is written in the link, `%2B` for `+`) of the `libstdc++` package in an Alpine directory listing; the newest if
/// several are there. `libstdc++-dev` and the like are not it.
fn pick_libstdcpp(listing: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for part in listing.split("href=\"").skip(1) {
        let Some(raw) = part.split('"').next() else { continue };
        let plain = raw.replace("%2B", "+");
        let Some(rest) = plain.strip_prefix("libstdc++-") else { continue };
        if rest.starts_with(|c: char| c.is_ascii_digit()) && rest.ends_with(".apk") && best.as_ref().is_none_or(|b| b.replace("%2B", "+") < plain) {
            best = Some(raw.to_string());
        }
    }
    best
}

/// Release programs that exist only for glibc: they are not offered on a musl system. (Python wheels, npm packages and static Go or
/// Rust programs are not in this list.)
fn musl_unavailable(id: &str, machine: &str) -> Option<&'static str> {
    match id {
        "aws-cli" => Some("Amazon publishes the AWS CLI v2 only built for glibc (its bundled programs need it)."),
        "gcloud" => Some("the Google Cloud SDK bundles a glibc Python and publishes no musl build."),
        "ollama" => Some("Ollama's release (program and libraries) is built only for glibc; there is no musl build."),
        "espflash" if machine != "x86_64" => Some("the project publishes a musl build for x86_64 only."),
        _ => None,
    }
}

/// The program interpreter an ELF file asks for (`/lib64/ld-linux-x86-64.so.2`), when it is a 64-bit little-endian ELF that has one.
pub(crate) fn elf_interpreter(path: &Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = [0u8; 64];
    f.read_exact(&mut h).ok()?;
    if &h[..4] != b"\x7fELF" || h[4] != 2 || h[5] != 1 {
        return None;
    }
    let (phoff, phentsize, phnum) = (u64::from_le_bytes(h[32..40].try_into().ok()?), u16::from_le_bytes(h[54..56].try_into().ok()?) as u64, u16::from_le_bytes(h[56..58].try_into().ok()?) as u64);
    for i in 0..phnum.min(64) {
        f.seek(SeekFrom::Start(phoff + i * phentsize)).ok()?;
        let mut ph = [0u8; 56];
        f.read_exact(&mut ph).ok()?;
        if u32::from_le_bytes(ph[0..4].try_into().ok()?) == 3 {
            let (off, size) = (u64::from_le_bytes(ph[8..16].try_into().ok()?), u64::from_le_bytes(ph[32..40].try_into().ok()?).min(512));
            f.seek(SeekFrom::Start(off)).ok()?;
            let mut buf = vec![0u8; size as usize];
            f.read_exact(&mut buf).ok()?;
            return Some(String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(buf.len())]).into_owned());
        }
    }
    None
}

/// Whether the program can be started at all on this system: an ELF file's loader must exist. The message says why in plain words
/// ("it is built for glibc; this system uses musl").
pub(crate) fn runnable_check(path: &Path) -> Result<(), String> {
    let Some(interp) = elf_interpreter(path) else { return Ok(()) };
    if Path::new(&interp).exists() {
        return Ok(());
    }
    if interp.contains("ld-linux") || interp.contains("/ld.so") {
        Err(if is_musl() { "it is built for glibc; this system uses musl.".to_string() } else { format!("it needs {interp}, which this system does not have.") })
    } else if interp.contains("ld-musl") {
        Err(format!("it is built for musl and needs {interp}, which this system does not have (it uses glibc)."))
    } else {
        Err(format!("it needs {interp}, which this system does not have."))
    }
}

/// The Amazon public key pinned in the repository for the AWS CLI's `.sig` file (`unset` until the owner adds it).
const AWS_CLI_PGP_KEY: &str = include_str!("../../../legal/aws-cli-pgp.asc");

fn pgp_key_file(id: &str) -> Option<&'static str> {
    let k = match id {
        "aws-cli" => AWS_CLI_PGP_KEY,
        _ => return None,
    };
    k.trim_start().starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----").then_some(k)
}

fn pgp_key_pinned(id: &str) -> bool {
    pgp_key_file(id).is_some()
}

/// DuckDB's `duckdb` command: a shell wrapper that runs our small Python program with the environment's Python.
fn write_duckdb_shim(target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let (py, bin) = (target.join("venv/bin/python"), target.join("bin"));
    if py.display().to_string().contains('\'') {
        return Err("the install folder has a quote in its name".into());
    }
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let shim = bin.join("duckdb.py");
    std::fs::write(&shim, include_str!("duckdb_shim.py")).map_err(|e| e.to_string())?;
    let wrapper = bin.join("duckdb");
    std::fs::write(&wrapper, format!("#!/bin/sh\nexec '{}' '{}' \"$@\"\n", py.display(), shim.display())).map_err(|e| e.to_string())?;
    for p in [&shim, &wrapper] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What the tile offers for a tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Not installed.
    Install,
    /// Installed, and this newer version exists.
    Upgrade(String),
    /// Installed, and no newer version is known.
    UpToDate,
    /// Not installed by IYAKKI but already on the machine (put there by hand): shown as installed, with the version it reports and where.
    Elsewhere(String),
}

/// The catalogue program a tool installs with: npm tools use Node.js, pip tools Python, a uv-managed Python uv.
pub fn runtime_for(d: &CliDef) -> Option<&'static str> {
    match d.source {
        Source::Npm(_) => Some("nodejs"),
        Source::Pip(_) => Some("python"),
        Source::Python(_) => Some("uv"),
        _ => None,
    }
}

/// `latest` is a newer version than `installed` (numbers compared part by part, a leading `v` ignored; anything else: different = newer).
pub fn version_is_newer(installed: &str, latest: &str) -> bool {
    let nums = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect() };
    let (a, b) = (nums(installed), nums(latest));
    if a.is_empty() || b.is_empty() {
        return installed.trim_start_matches('v') != latest.trim_start_matches('v');
    }
    let n = a.len().max(b.len());
    (0..n).map(|i| (b.get(i).copied().unwrap_or(0), a.get(i).copied().unwrap_or(0))).find(|(x, y)| x != y).is_some_and(|(x, y)| x > y)
}

/// The newest release in a PyPI `json` answer that has a wheel whose name contains both `platform` and `python_tag`.
fn pypi_newest_with_wheel(j: &serde_json::Value, platform: &str, python_tag: &str) -> Option<String> {
    let nums = |v: &str| -> Vec<u64> { v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect() };
    j.get("releases")?.as_object()?.iter().filter(|(v, files)| !v.contains(|c: char| c.is_ascii_alphabetic()) && files.as_array().is_some_and(|fs| fs.iter().any(|f| !f.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false) && f.get("filename").and_then(|n| n.as_str()).is_some_and(|n| n.ends_with(".whl") && n.contains(platform) && n.contains(python_tag))))).map(|(v, _)| v.clone()).max_by_key(|v| nums(v))
}

/// The newest `cpython-<series>.N+TAG-<machine>-unknown-linux-<libc>-install_only` in python-build-standalone's SHA256SUMS, as `<series>.N`.
fn pbs_pick(sums: &str, series: &str, machine: &str, musl: bool) -> Option<String> {
    let suffix = format!("-{machine}-unknown-linux-{}-install_only.tar.gz", if musl { "musl" } else { "gnu" });
    let prefix = format!("cpython-{series}.");
    sums.lines().filter_map(|l| l.split_whitespace().nth(1)).filter_map(|n| n.trim_start_matches('*').strip_prefix(prefix.as_str())?.strip_suffix(suffix.as_str())?.split('+').next()?.parse::<u32>().ok()).max().map(|n| format!("{series}.{n}"))
}

/// The npm package a tool falls back to when its native build could not be downloaded.
fn npm_fallback(id: &str) -> Option<&'static str> {
    (id == "claude-code").then_some("@anthropic-ai/claude-code")
}

/// `platforms.<platform>.checksum` of Anthropic's release manifest, when it is a SHA-256.
pub(crate) fn manifest_checksum(manifest: &str, platform: &str) -> Option<String> {
    let j: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let h = j.get("platforms")?.get(platform)?.get("checksum")?.as_str()?.to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// Where `rel` (which may start with `*/`, standing for the one top folder) is inside `base`.
fn find_in(base: &Path, rel: &str) -> Option<PathBuf> {
    let p = if let Some(rest) = rel.strip_prefix("*/") {
        let tops: Vec<PathBuf> = std::fs::read_dir(base).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        tops.iter().map(|t| t.join(rest)).find(|c| c.is_file())?
    } else {
        base.join(rel)
    };
    use std::os::unix::fs::PermissionsExt;
    (p.is_file() && p.metadata().ok()?.permissions().mode() & 0o111 != 0).then_some(p)
}

fn pick_version_line(out: &str, id: &str) -> String {
    let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let pick = if id == "azure-cli" { lines.iter().find(|l| l.starts_with("azure-cli")) } else { lines.first() };
    pick.map(|l| l.chars().take(120).collect()).unwrap_or_default()
}

/// Runs a program with a time limit and a cut-down environment; its output on success, or why it failed.
pub(crate) fn run_quiet(program: &str, args: &[&str], seconds: u64) -> Result<String, String> {
    run_quiet_env(program, args, &[], seconds)
}

/// [`run_quiet`] with some variables added for this one child.
fn run_quiet_env(program: &str, args: &[&str], env: &[(&str, &str)], seconds: u64) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for k in ["PATH", "LANG", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    // a check that only asks for a version runs in a throw-away home: a program that writes a settings file on its first start
    // (Copilot does) must not litter the person's home or `/`
    let scratch = std::env::temp_dir().join("iyakki-check-home");
    let _ = std::fs::create_dir_all(&scratch);
    cmd.env("HOME", &scratch);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // a program that was written a moment ago can be "busy" while another thread's fork still holds the file
    let mut child = None;
    for attempt in 0..8 {
        match cmd.spawn() {
            Ok(c) => {
                child = Some(c);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 7 => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("could not start {program}: {e}")),
        }
    }
    let mut child = child.ok_or_else(|| format!("could not start {program}"))?;
    let mut so = child.stdout.take().ok_or("no output pipe")?;
    let mut se = child.stderr.take().ok_or("no error pipe")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = so.by_ref().take(256 * 1024).read_to_string(&mut s);
        let _ = tx.send((true, s));
    });
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = se.by_ref().take(256 * 1024).read_to_string(&mut s);
        let _ = tx2.send((false, s));
    });
    let start = std::time::Instant::now();
    // the PID 1 reaper may take the exit status of this child: then the program has ended but its status is gone (`Gone`)
    let state = loop {
        match iyakki_core_api::child_status(&mut child) {
            iyakki_core_api::ChildState::Running if start.elapsed().as_secs() >= seconds => {
                let _ = child.kill();
                let _ = iyakki_core_api::child_wait(&mut child);
                return Err(format!("{program} did not finish within {seconds} seconds"));
            }
            iyakki_core_api::ChildState::Running => std::thread::sleep(std::time::Duration::from_millis(30)),
            ended => break ended,
        }
    };
    let (mut out, mut err) = (String::new(), String::new());
    while let Ok((is_out, s)) = rx.recv_timeout(std::time::Duration::from_millis(400)) {
        if is_out { out = s } else { err = s }
    }
    // a status that was taken is not a failure by itself: the output decides (an error line on stderr and nothing on stdout is one)
    let ok = state.success() || (matches!(state, iyakki_core_api::ChildState::Gone) && (!out.trim().is_empty() || err.trim().is_empty()));
    if ok { Ok(if out.trim().is_empty() { err } else { out }) } else { Err(format!("{} failed: {}", program.rsplit('/').next().unwrap_or(program), err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no message"))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{gzip, make_tar, make_zip, tmp};
    use iyakki_proxy::StreamHead;
    use std::io::Cursor;

    type Reply = (u16, Vec<(String, String)>, Vec<u8>);

    struct Fake {
        routes: Mutex<HashMap<String, Reply>>,
        asked: Mutex<Vec<String>>,
    }

    impl Streamer for Fake {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.asked.lock().unwrap().push(url.to_string());
            let r = self.routes.lock().unwrap().get(url).cloned().ok_or_else(|| format!("no route for {url}"))?;
            let mut headers = r.1.clone();
            headers.push(("content-length".into(), r.2.len().to_string()));
            Ok(StreamHead { status: r.0, headers, body: Box::new(Cursor::new(r.2)) })
        }
    }

    fn fake(routes: Vec<(String, Reply)>) -> Arc<Fake> {
        Arc::new(Fake { routes: Mutex::new(routes.into_iter().collect()), asked: Mutex::new(vec![]) })
    }

    fn ok(body: &[u8]) -> Reply {
        (200, vec![], body.to_vec())
    }

    fn sha(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    fn manager(tag: &str, st: Arc<Fake>) -> Arc<CliManager> {
        let d = tmp(tag);
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(st);
        // the tests do not depend on which C library the machine they run on has; the ones about musl say so
        m.set_target(false, std::env::consts::ARCH);
        m
    }

    fn wait(m: &CliManager, id: &str) -> Job {
        for _ in 0..500 {
            if let Some(j) = m.job(id) {
                if j.finished || j.error.is_some() {
                    return j;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the install never finished: {:?}", m.job(id));
    }

    #[test]
    fn the_catalogue_is_well_formed_https_only_and_covers_the_tools_people_ask_for() {
        let ids: Vec<&str> = CATALOG.iter().map(|d| d.id).collect();
        for want in ["kubectl", "k3s", "helm", "terraform", "gh", "aws-cli", "gcloud", "azure-cli", "arduino-cli", "esptool", "espflash", "claude-code", "github-copilot", "codex", "gemini-cli", "aider", "nodejs", "uv", "python"] {
            assert!(ids.contains(&want), "{want}");
        }
        let mut names = std::collections::HashSet::new();
        for d in CATALOG {
            assert!(!d.bins.is_empty() && !d.hosts.is_empty() && !d.about.is_empty());
            assert!(d.home.starts_with("https://"));
            for (n, _) in d.bins {
                assert!(names.insert(*n), "{n} is provided twice");
                assert!(!n.contains('/') && !n.starts_with('.') && !n.is_empty());
            }
            let urls: Vec<&str> = match d.source { Source::File(u) | Source::TarGz(u) | Source::Zip(u) | Source::TarZst(u) | Source::TarXz(u) => vec![u], Source::Pip(_) | Source::Npm(_) | Source::Python(_) | Source::System | Source::Setting(_) => vec![] };
            for u in urls.into_iter().chain(match d.checksum { Some(Checksum::Single(u)) | Some(Checksum::Lines(u)) | Some(Checksum::Manifest(u)) | Some(Checksum::Md5(u)) => Some(u), Some(Checksum::Fixed(_)) | None => None }) {
                let url = template(u, "v1.2.3").unwrap();
                assert!(host_ok(&url, d.hosts).is_ok(), "{} {url}", d.id);
                assert!(!url.contains('{'), "unfilled placeholder in {url}");
            }
        }
        assert_eq!(version_from_location("https://github.com/cli/cli/releases/tag/v2.60.0").as_deref(), Some("v2.60.0"));
        assert_eq!(version_from_location("https://github.com/k3s-io/k3s/releases/download/v1.31.1%2Bk3s1/k3s").as_deref(), Some("v1.31.1+k3s1"));
        assert_eq!(version_from_location("https://example.com/nothing"), None);
        assert!(host_ok("http://dl.k8s.io/x", &["k8s.io"]).is_err() && host_ok("https://evil.example/x", &["k8s.io"]).is_err() && host_ok("https://cdn.dl.k8s.io/x", &["k8s.io"]).is_ok() && host_ok("https://notk8s.io/x", &["k8s.io"]).is_err());
        assert_eq!(extract_hash(&format!("{} *kubectl\n", "a".repeat(64)), "kubectl").as_deref(), Some(&*"a".repeat(64)));
        assert_eq!(extract_hash(&format!("{}\n", "b".repeat(64)), "whatever").as_deref(), Some(&*"b".repeat(64)));
        assert_eq!(extract_hash(&format!("{}  other\n{}  gh.tar.gz\n", "1".repeat(64), "2".repeat(64)), "gh.tar.gz").as_deref(), Some(&*"2".repeat(64)));
        assert_eq!(extract_hash("nothing", "x"), None);
    }

    #[test]
    fn a_single_file_tool_is_downloaded_checked_linked_and_listed_and_then_removed() {
        let (arch, _, _) = arch().unwrap();
        let bin = b"#!/bin/sh\necho 'Client Version: v1.31.0'\n";
        let st = fake(vec![
            ("https://dl.k8s.io/release/stable.txt".into(), ok(b"v1.31.0\n")),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl"), ok(bin)),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl.sha256"), ok(sha(bin).as_bytes())),
        ]);
        let m = manager("single", st.clone());
        assert!(m.installed().is_empty() && m.extras().is_empty());
        m.install("kubectl", "root").unwrap();
        let j = wait(&m, "kubectl");
        assert!(j.error.is_none() && j.finished, "{j:?}");
        let i = m.is_installed("kubectl").unwrap();
        assert_eq!((i.version.as_str(), i.verified, i.by.as_str()), ("v1.31.0", true, "root"));
        assert!(i.reported.contains("Client Version: v1.31.0"), "{:?}", i.reported);
        let link = m.bin_dir().join("kubectl");
        assert!(link.exists() && std::fs::read_link(&link).is_ok());
        assert_eq!(m.extras(), vec![Extra { name: "kubectl".into(), host: None }]);
        // the tool can be run through the folder of installed tools
        let out = crate::exec::run_with(&m.extras(), "kubectl", &["version".to_string()], &std::env::temp_dir(), None, 10, Some(&m.bin_dir())).unwrap();
        assert!(out.contains("v1.31.0"), "{out}");
        assert!(crate::exec::run_with(&m.extras(), "helm", &[], &std::env::temp_dir(), None, 10, Some(&m.bin_dir())).unwrap_err().contains("not available"));
        // removal deletes the program, its folder and its entry
        m.remove("kubectl").unwrap();
        assert!(!link.exists() && !m.pkg_dir("kubectl").exists() && m.installed().is_empty());
        assert!(m.remove("kubectl").unwrap_err().contains("not installed") && m.remove("nope").is_err());
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_refused_and_leaves_nothing_behind() {
        let (arch, _, _) = arch().unwrap();
        let st = fake(vec![
            ("https://dl.k8s.io/release/stable.txt".into(), ok(b"v1.31.0")),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl"), ok(b"tampered program")),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl.sha256"), ok(sha(b"the real program").as_bytes())),
        ]);
        let m = manager("badsum", st);
        m.install("kubectl", "root").unwrap();
        let j = wait(&m, "kubectl");
        assert!(j.error.as_deref().unwrap_or("").contains("does not match the vendor's checksum"), "{j:?}");
        assert!(m.installed().is_empty() && !m.bin_dir().join("kubectl").exists());
        assert!(!m.pkg_dir("kubectl").join("v1.31.0").exists());
        // and can be tried again
        assert!(m.install("kubectl", "root").is_ok());
        wait(&m, "kubectl");
    }

    #[test]
    fn a_tar_gz_tool_with_a_top_folder_and_a_zip_tool_are_unpacked_and_linked_and_redirects_are_followed_within_their_hosts() {
        let (arch, a2, _) = arch().unwrap();
        // helm: tar.gz with linux-amd64/helm
        let helm_tar = gzip(&make_tar(&[("linux-amd64/", b'5', "", 0o755), ("linux-amd64/helm", b'0', "#!/bin/sh\necho 'v3.16.2'\n", 0o755), ("linux-amd64/LICENSE", b'0', "x", 0o644)]));
        let helm_url = format!("https://get.helm.sh/helm-v3.16.2-linux-{arch}.tar.gz");
        // terraform: a zip behind a redirect to the vendor's own CDN
        let tf_zip = make_zip(&[("terraform", "#!/bin/sh\necho 'Terraform v1.9.5'\n", 0o100755, true), ("LICENSE.txt", "mpl", 0o100644, false)]);
        let tf_name = format!("terraform_1.9.5_linux_{arch}.zip");
        let st = fake(vec![
            ("https://get.helm.sh/helm-latest-version".into(), ok(b"v3.16.2")),
            (helm_url.clone(), ok(&helm_tar)),
            (format!("{helm_url}.sha256sum"), ok(format!("{}  helm-v3.16.2-linux-{arch}.tar.gz\n", sha(&helm_tar)).as_bytes())),
            ("https://checkpoint-api.hashicorp.com/v1/check/terraform".into(), ok(b"{\"product\":\"terraform\",\"current_version\":\"1.9.5\"}")),
            (format!("https://releases.hashicorp.com/terraform/1.9.5/{tf_name}"), (302, vec![("location".into(), format!("https://cdn.releases.hashicorp.com/x/{tf_name}"))], vec![])),
            (format!("https://cdn.releases.hashicorp.com/x/{tf_name}"), ok(&tf_zip)),
            ("https://releases.hashicorp.com/terraform/1.9.5/terraform_1.9.5_SHA256SUMS".into(), ok(format!("{}  other.zip\n{}  {tf_name}\n", "0".repeat(64), sha(&tf_zip)).as_bytes())),
        ]);
        let m = manager("archives", st.clone());
        m.install("helm", "root").unwrap();
        let j = wait(&m, "helm");
        assert!(j.error.is_none(), "{j:?}");
        assert_eq!(m.is_installed("helm").unwrap().version, "v3.16.2");
        assert!(m.bin_dir().join("helm").exists());
        m.install("terraform", "root").unwrap();
        let j = wait(&m, "terraform");
        assert!(j.error.is_none(), "{j:?}");
        let t = m.is_installed("terraform").unwrap();
        assert!(t.verified && t.reported.contains("Terraform v1.9.5"), "{t:?}");
        let mut names: Vec<String> = m.extras().into_iter().map(|x| x.name).collect();
        names.sort();
        assert_eq!(names, vec!["helm", "terraform"]);
        // a redirect to a host that is not the vendor's is refused
        let st2 = fake(vec![
            ("https://get.helm.sh/helm-latest-version".into(), ok(b"v3.16.2")),
            (helm_url, (302, vec![("location".into(), "https://evil.example/helm.tar.gz".into())], vec![])),
        ]);
        let m2 = manager("redirect", st2);
        m2.install("helm", "root").unwrap();
        assert!(wait(&m2, "helm").error.unwrap().contains("not one of this tool's sources"));
        let _ = a2;
    }

    #[test]
    fn a_latest_redirect_names_the_version_and_a_tool_without_a_checksum_says_so() {
        let (arch, _, _) = arch().unwrap();
        let tar = gzip(&make_tar(&[("gh_2.60.0_linux_amd64/bin/gh", b'0', "#!/bin/sh\necho 'gh version 2.60.0'\n", 0o755)]));
        let name = format!("gh_2.60.0_linux_{arch}.tar.gz");
        let st = fake(vec![
            ("https://github.com/cli/cli/releases/latest".into(), (302, vec![("location".into(), "https://github.com/cli/cli/releases/tag/v2.60.0".into())], vec![])),
            (format!("https://github.com/cli/cli/releases/download/v2.60.0/{name}"), ok(&tar)),
            ("https://github.com/cli/cli/releases/download/v2.60.0/gh_2.60.0_checksums.txt".into(), ok(format!("{}  {name}\n", sha(&tar)).as_bytes())),
        ]);
        let m = manager("gh", st);
        m.install("gh", "root").unwrap();
        assert!(wait(&m, "gh").error.is_none());
        assert_eq!(m.is_installed("gh").unwrap().version, "v2.60.0");
        assert_eq!(m.extras(), vec![Extra { name: "gh".into(), host: Some("api.github.com".into()) }]);
        // aws: a zip with no published checksum, version read from the program
        let (_, a2, _) = super::arch().unwrap();
        let zip = make_zip(&[("aws/dist/aws", "#!/bin/sh\necho 'aws-cli/2.17.0 Python/3.12 Linux'\n", 0o100755, true), ("aws/dist/aws_completer", "#!/bin/sh\n", 0o100755, false)]);
        let st = fake(vec![(format!("https://awscli.amazonaws.com/awscli-exe-linux-{a2}-{}.zip", def("aws-cli").unwrap().pin), ok(&zip))]);
        let m = manager("aws", st);
        m.install("aws-cli", "root").unwrap();
        assert!(wait(&m, "aws-cli").error.is_none());
        let i = m.is_installed("aws-cli").unwrap();
        assert!(!i.verified, "no checksum was available");
        assert_eq!(i.version, def("aws-cli").unwrap().pin, "{i:?}");
        assert_eq!(m.extras().iter().find(|x| x.name == "aws").unwrap().host.as_deref(), Some("amazonaws.com"));
    }

    #[test]
    fn aws_cli_with_a_pinned_pgp_key_must_carry_a_valid_signature() {
        let key: &[u8] = include_bytes!("../tests/fixtures/openpgp/test-key.asc");
        let payload: &[u8] = include_bytes!("../tests/fixtures/openpgp/payload.bin");
        let good_sig: &[u8] = include_bytes!("../tests/fixtures/openpgp/payload.bin.sha256.sig");
        let (_, a2, _) = super::arch().unwrap();
        let url = format!("https://awscli.amazonaws.com/awscli-exe-linux-{a2}-{}.zip", def("aws-cli").unwrap().pin);
        // the signature belongs to another file: refused, nothing installed
        let mut other = payload.to_vec();
        other[0] ^= 1;
        let st = fake(vec![(url.clone(), ok(&other)), (format!("{url}.sig"), ok(good_sig))]);
        let m = manager("awspgp-bad", st);
        m.set_pgp_key("aws-cli", key);
        m.install("aws-cli", "root").unwrap();
        let j = wait(&m, "aws-cli");
        assert!(j.error.as_deref().is_some_and(|e| e.contains("PGP signature") && e.contains("nothing was installed")), "{j:?}");
        assert!(m.is_installed("aws-cli").is_none());
        // no signature file at all: refused too
        let st = fake(vec![(url.clone(), ok(payload))]);
        let m = manager("awspgp-none", st);
        m.set_pgp_key("aws-cli", key);
        m.install("aws-cli", "root").unwrap();
        assert!(wait(&m, "aws-cli").error.is_some());
        // the right file passes the signature check (and then fails later, because the stand-in is not a zip)
        let st = fake(vec![(url.clone(), ok(payload)), (format!("{url}.sig"), ok(good_sig))]);
        let m = manager("awspgp-good", st);
        m.set_pgp_key("aws-cli", key);
        m.install("aws-cli", "root").unwrap();
        let j = wait(&m, "aws-cli");
        assert!(!j.error.as_deref().unwrap_or("").contains("PGP"), "{j:?}");
    }

    #[test]
    fn the_arduino_and_esp_tools_install_with_optional_extra_programs_and_need_no_network_to_run() {
        let (arch, a2, _) = arch().unwrap();
        let ard = if arch == "arm64" { "Linux_ARM64" } else { "Linux_64bit" };
        // arduino-cli: a tar.gz whose program is at the top, with a checksum file named like the release's
        let tar = gzip(&make_tar(&[("arduino-cli", b'0', "#!/bin/sh\necho 'arduino-cli  Version: 1.5.1 Commit: x'\n", 0o755), ("LICENSE.txt", b'0', "gpl", 0o644)]));
        let name = format!("arduino-cli_1.5.1_{ard}.tar.gz");
        // espflash: a zip holding one program, pinned with the digest GitHub shows for the release asset
        let zip = make_zip(&[("espflash", "#!/bin/sh\necho 'espflash 4.6.0'\n", 0o100755, true)]);
        TEST_FIXED.lock().unwrap().push((format!("espflash-{a2}-unknown-linux-gnu.zip"), sha(&zip)));
        let st = fake(vec![
            ("https://github.com/arduino/arduino-cli/releases/latest".into(), (302, vec![("location".into(), "https://github.com/arduino/arduino-cli/releases/tag/v1.5.1".into())], vec![])),
            (format!("https://github.com/arduino/arduino-cli/releases/download/v1.5.1/{name}"), ok(&tar)),
            ("https://github.com/arduino/arduino-cli/releases/download/v1.5.1/1.5.1-checksums.txt".into(), ok(format!("{}  {name}\n", sha(&tar)).as_bytes())),
            (format!("https://github.com/esp-rs/espflash/releases/download/v4.6.0/espflash-{a2}-unknown-linux-gnu.zip"), ok(&zip)),
        ]);
        let m = manager("esp", st);
        m.install("arduino-cli", "root").unwrap();
        let j = wait(&m, "arduino-cli");
        assert!(j.error.is_none(), "{j:?}");
        assert!(m.is_installed("arduino-cli").unwrap().verified);
        m.install("espflash", "root").unwrap();
        let j = wait(&m, "espflash");
        assert!(j.error.is_none(), "{j:?}");
        assert!(m.is_installed("espflash").unwrap().verified, "checked against the digest GitHub publishes for the pinned release");
        // esptool from pip: only its first program is required; the others may be missing
        let d = tmp("esptoolpy");
        let py = d.join("python3");
        std::fs::write(&py, "#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then exit 0; fi\nenv=\"$3\"\nmkdir -p \"$env/bin\"\ncat > \"$env/bin/pip\" <<'EOF'\n#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '#!/bin/sh\\necho \"esptool v5.5.0\"\\n' > \"$d/esptool\"\nchmod +x \"$d/esptool\"\nEOF\nchmod +x \"$env/bin/pip\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&py, std::fs::Permissions::from_mode(0o755)).unwrap();
        m.set_python(&py.display().to_string());
        m.install("esptool", "root").unwrap();
        let j = wait(&m, "esptool");
        assert!(j.error.is_none(), "{j:?}");
        let e = m.is_installed("esptool").unwrap();
        assert_eq!((e.bins.as_slice(), e.version.as_str()), (&["esptool".to_string()][..], def("esptool").unwrap().pin), "{e:?}");
        // what the chat may run, and which network check each gets: the chip tools need none
        let mut x = m.extras();
        x.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(x, vec![Extra { name: "arduino-cli".into(), host: None }, Extra { name: "espflash".into(), host: Some("-".into()) }, Extra { name: "esptool".into(), host: Some("-".into()) }]);
        let out = crate::exec::run_with(&m.extras(), "espflash", &["board-info".to_string()], &std::env::temp_dir(), None, 10, Some(&m.bin_dir())).unwrap();
        assert!(out.contains("espflash 4.6.0"), "{out}");
    }

    /// Stand-in node and npm: npm records how it was started and "installs" a `claude` into its prefix.
    fn fake_npm(d: &Path, node_version: &str) -> (String, String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let record = d.join("npm-seen.txt");
        let node = d.join("node");
        std::fs::write(&node, format!("#!/bin/sh\necho {node_version}\n")).unwrap();
        let npm = d.join("npm");
        std::fs::write(&npm, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 10.0.0; exit 0; fi\n{{ echo \"ARGS $*\"; echo \"PREFIX=$npm_config_prefix\"; echo \"CACHE=$npm_config_cache\"; echo \"SUDO_USER=$SUDO_USER\"; }} > {rec}\nmkdir -p \"$npm_config_prefix/bin\"\nfor n in claude codex gemini copilot; do printf '#!/bin/sh\\necho \"2.1.5 (Claude Code)\"\\n' > \"$npm_config_prefix/bin/$n\"; chmod +x \"$npm_config_prefix/bin/$n\"; done\n", rec = record.display())).unwrap();
        for p in [&node, &npm] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (npm.display().to_string(), node.display().to_string(), record)
    }

    #[test]
    fn an_npm_tool_is_installed_into_its_own_prefix_with_the_command_shown_first_and_removed_again() {
        let d = tmp("npm");
        let (npm, node, record) = fake_npm(&d, "v22.4.0");
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_npm(&npm, &node);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        m.set_on_installed(Arc::new(move |id| s2.lock().unwrap().push(id.to_string())));
        // the plan names the exact command and the folder, before anything runs
        let plan = m.install_plan("codex").unwrap();
        assert!(plan.contains(&format!("`npm install -g @openai/codex@{}`", def("codex").unwrap().pin)) && plan.contains("npm_config_prefix") && plan.contains(&m.npm_prefix("codex").display().to_string()), "{plan}");
        assert!(plan.contains("no sudo") && plan.contains("registry.npmjs.org") && plan.contains("Node.js 22 was found"), "{plan}");
        assert!(!record.exists(), "showing the plan runs nothing");
        assert_eq!(m.install_plan("gemini-cli").unwrap().contains(&format!("`npm install -g @google/gemini-cli@{}`", def("gemini-cli").unwrap().pin)), true);
        assert!(!m.install_plan("github-copilot").unwrap().contains("npm install") && !m.install_plan("claude-code").unwrap().contains("Node.js is not on this machine"), "the native builds do not need Node.js");
        assert!(m.install_plan("kubectl").is_none(), "a plain download is described by the tile");
        std::env::remove_var("npm_config_prefix");
        m.install("codex", "root").unwrap();
        let j = wait(&m, "codex");
        assert!(j.error.is_none(), "{j:?}");
        // an argument array with no shell and no sudo, the prefix and cache only for the child
        let rec = std::fs::read_to_string(&record).unwrap();
        assert!(rec.contains(&format!("ARGS install -g @openai/codex@{}\n", def("codex").unwrap().pin)), "{rec}");
        assert!(rec.contains(&format!("PREFIX={}", m.npm_prefix("codex").display())) && rec.contains("CACHE=") && rec.contains("SUDO_USER=\n"), "{rec}");
        assert!(std::env::var("npm_config_prefix").is_err(), "this process's environment is untouched");
        // linked where the chat and the terminal look, version read from the program, hook told
        assert!(m.bin_dir().join("codex").exists());
        let i = m.is_installed("codex").unwrap();
        assert!(i.reported.contains("2.1.5") && i.version == def("codex").unwrap().pin, "{i:?}");
        assert!(m.extras().iter().any(|e| e.name == "codex" && e.host.as_deref() == Some("api.openai.com")));
        assert_eq!(*seen.lock().unwrap(), ["codex"]);
        // removing deletes the folder and the link
        m.remove("codex").unwrap();
        assert!(!m.bin_dir().join("codex").exists() && !m.npm_prefix("codex").exists() && m.is_installed("codex").is_none());
    }

    #[test]
    fn a_node_that_is_missing_or_too_old_is_installed_first_and_when_that_fails_nothing_is_left_behind() {
        let d = tmp("npm-nonode");
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_npm("/nonexistent/npm", "/nonexistent/node");
        let plan = m.install_plan("codex").unwrap();
        assert!(plan.contains("Node.js is not on this machine") && plan.contains("installed first from the CLI catalogue") && plan.contains("no sudo"), "{plan}");
        m.install("codex", "root").unwrap();
        let e = wait(&m, "codex").error.unwrap();
        assert!(e.contains("Node.js is needed first for OpenAI Codex CLI") && e.contains("no route"), "{e}");
        assert!(m.installed().is_empty() && !m.npm_prefix("codex").exists());
        // too old counts as missing (the vendors ask for 22)
        let (npm, node, record) = fake_npm(&d, "v16.20.0");
        m.set_npm(&npm, &node);
        m.install("gemini-cli", "root").unwrap();
        let e = wait(&m, "gemini-cli").error.unwrap();
        assert!(e.contains("Node.js is needed first for Gemini CLI"), "{e}");
        assert!(!record.exists());
        // npm itself failing leaves nothing behind and the error is reported
        let (_, node, _) = fake_npm(&d, "v22.0.0");
        let bad = d.join("badnpm");
        std::fs::write(&bad, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 10; exit 0; fi\necho 'npm ERR! network request failed' >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
        m.set_npm(&bad.display().to_string(), &node);
        m.install("codex", "root").unwrap();
        let j = wait(&m, "codex");
        assert!(j.error.as_deref().is_some_and(|e| e.contains("npm ERR! network request failed")), "{j:?}");
        assert!(m.is_installed("codex").is_none() && !m.npm_prefix("codex").exists());
    }

    #[test]
    fn a_python_package_tool_is_installed_into_its_own_environment_and_failures_are_reported() {
        let d = tmp("pip");
        let fake_py = d.join("python3");
        // a stand-in python that makes the environment and a pip that "installs" az
        std::fs::write(&fake_py, "#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then exit 0; fi\nenv=\"$3\"\nmkdir -p \"$env/bin\"\ncat > \"$env/bin/pip\" <<'EOF'\n#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '#!/bin/sh\\necho \"azure-cli                         2.64.0\"\\necho \"core 2.64.0\"\\n' > \"$d/az\"\nchmod +x \"$d/az\"\nEOF\nchmod +x \"$env/bin/pip\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_py, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_python(&fake_py.display().to_string());
        m.install("azure-cli", "root").unwrap();
        let j = wait(&m, "azure-cli");
        assert!(j.error.is_none(), "{j:?}");
        let i = m.is_installed("azure-cli").unwrap();
        assert!(i.reported.starts_with("azure-cli") && i.version == def("azure-cli").unwrap().pin, "{i:?}");
        assert!(m.bin_dir().join("az").exists());
        // no python anywhere and no way to fetch one: the catalogue's Python is tried first, and the failure says so; nothing is left behind
        let m2 = Arc::new(CliManager::new(&d.join("cli2")));
        m2.set_streamer(fake(vec![]));
        m2.set_python("/nonexistent/python3");
        m2.install("azure-cli", "root").unwrap();
        let j = wait(&m2, "azure-cli");
        assert!(j.error.unwrap().contains("Python 3 is needed first for Azure CLI"));
        assert!(m2.installed().is_empty());
        // no network set up, and unknown names
        let m3 = Arc::new(CliManager::new(&d.join("cli3")));
        assert!(m3.install("kubectl", "root").unwrap_err().contains("no network connection"));
        assert!(m3.install("nothing", "root").unwrap_err().contains("not in the list"));
    }

    /// Routes for the catalogue's Node.js: the version list, a tarball holding `node` and `npm` scripts, and the checksum file.
    /// The address of a download as a glibc machine of this processor would use it (what the tests' managers are told to be).
    fn tpl_glibc(t: &str, v: &str) -> Result<String, String> {
        template_for(t, v, false, std::env::consts::ARCH)
    }

    fn node_routes(node: &str, npm: &str) -> Vec<(String, Reply)> {
        let d = def("nodejs").unwrap();
        let (VersionFrom::NodeLts(vu), Source::TarGz(tu), Some(Checksum::Lines(cu))) = (d.version, d.source, d.checksum) else { panic!("the nodejs entry changed shape") };
        let plat = tpl_glibc("{cplatform}", "").unwrap();
        let url = tpl_glibc(tu, "v26.1.0").unwrap();
        let file = url.rsplit('/').next().unwrap().to_string();
        let top = file.trim_end_matches(".tar.gz").to_string();
        let tar = gzip(&make_tar(&[(&format!("{top}/"), b'5', "", 0o755), (&format!("{top}/bin/"), b'5', "", 0o755), (&format!("{top}/bin/node"), b'0', node, 0o755), (&format!("{top}/bin/npm"), b'0', npm, 0o755)]));
        vec![
            (tpl_glibc(vu, "").unwrap(), ok(format!(r#"[{{"version":"v27.0.0","lts":false,"files":["{plat}"]}},{{"version":"v26.1.0","lts":"Zed","files":["{plat}"]}},{{"version":"v25.0.0","lts":false,"files":["{plat}"]}}]"#).as_bytes())),
            (url, ok(&tar)),
            (tpl_glibc(cu, "v26.1.0").unwrap(), ok(format!("{}  {file}\n", sha(&tar)).as_bytes())),
        ]
    }

    #[test]
    fn a_missing_node_is_installed_from_the_catalogue_first_and_the_npm_tool_then_runs_with_it() {
        let d = tmp("npm-dep");
        let record = d.join("npm-seen.txt");
        let npm = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 11.0.0; exit 0; fi\n{{ echo \"ARGS $*\"; echo \"PATH=$PATH\"; }} > {rec}\nmkdir -p \"$npm_config_prefix/bin\"\nprintf '#!/bin/sh\\necho \"2.1.5 (Codex)\"\\n' > \"$npm_config_prefix/bin/codex\"\nchmod +x \"$npm_config_prefix/bin/codex\"\n", rec = record.display());
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_target(false, std::env::consts::ARCH);
        m.set_streamer(fake(node_routes("#!/bin/sh\necho v26.1.0\n", &npm)));
        m.set_npm("/nonexistent/npm", "/nonexistent/node");
        let told = Arc::new(Mutex::new(Vec::<String>::new()));
        let t2 = told.clone();
        m.set_on_installed(Arc::new(move |id| t2.lock().unwrap().push(id.to_string())));
        assert_eq!(m.install_order("codex"), ["nodejs", "codex"], "the order shown before anything runs");
        m.install("codex", "root").unwrap();
        let j = wait(&m, "codex");
        assert!(j.error.is_none(), "{j:?}");
        let n = m.is_installed("nodejs").unwrap();
        assert!(n.verified && n.version == "v26.1.0" && n.by == "root", "{n:?}");
        assert!(m.bin_dir().join("node").exists() && m.bin_dir().join("npm").exists());
        // npm ran with the catalogue's folder first on its search path, so its scripts find that `node`
        let rec = std::fs::read_to_string(&record).unwrap();
        assert!(rec.contains(&format!("ARGS install -g @openai/codex@{}\n", def("codex").unwrap().pin)) && rec.contains(&format!("PATH={}:", m.bin_dir().display())), "{rec}");
        assert!(m.is_installed("codex").is_some());
        assert_eq!(*told.lock().unwrap(), ["nodejs", "codex"]);
        assert!(m.install_plan("codex").unwrap().contains("Node.js 26 was found"));
        m.remove("nodejs").unwrap();
        assert!(!m.bin_dir().join("node").exists());
    }

    #[test]
    fn a_missing_python_is_installed_through_uv_first_and_the_pip_tool_then_runs_with_it() {
        let d = tmp("py-dep");
        let (_, a2, _) = arch().unwrap();
        let uvd = def("uv").unwrap();
        let (Source::TarGz(tu), Some(Checksum::Single(cu))) = (uvd.source, uvd.checksum) else { panic!("the uv entry changed shape") };
        // the Python uv will "download": a tree copied into UV_PYTHON_INSTALL_DIR, whose python3 makes environments
        let tree = d.join("fakepy");
        std::fs::create_dir_all(tree.join("bin")).unwrap();
        let py = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Python 3.12.1'; exit 0; fi\nif [ \"$1\" = \"-c\" ]; then exit 0; fi\nenv=\"$3\"\nmkdir -p \"$env/bin\"\ncat > \"$env/bin/pip\" <<'EOF'\n#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '#!/bin/sh\\necho \"esptool v5.5.0\"\\n' > \"$d/esptool\"\nchmod +x \"$d/esptool\"\nEOF\nchmod +x \"$env/bin/pip\"\n";
        use std::os::unix::fs::PermissionsExt;
        for n in ["python3", "python"] {
            std::fs::write(tree.join("bin").join(n), py).unwrap();
            std::fs::set_permissions(tree.join("bin").join(n), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let seen = d.join("uv-seen.txt");
        let uv = format!("#!/bin/sh\ncase \"$1 $2\" in\n  \"python install\") cp -r {tree} \"$UV_PYTHON_INSTALL_DIR/cpython-3.12.1-linux\"; echo \"$3|$UV_PYTHON_INSTALL_DIR|$UV_PYTHON_BIN_DIR\" > {seen};;\n  *) echo 'uv 0.13.1';;\nesac\n", tree = tree.display(), seen = seen.display());
        let top = format!("uv-{a2}-unknown-linux-musl");
        let tar = gzip(&make_tar(&[(&format!("{top}/"), b'5', "", 0o755), (&format!("{top}/uv"), b'0', &uv, 0o755), (&format!("{top}/uvx"), b'0', "#!/bin/sh\necho uvx\n", 0o755)]));
        let url = template(tu, "0.13.1").unwrap();
        let file = url.rsplit('/').next().unwrap().to_string();
        let st = fake(vec![
            ("https://github.com/astral-sh/uv/releases/latest".into(), (302, vec![("location".into(), "https://github.com/astral-sh/uv/releases/tag/0.13.1".into())], vec![])),
            (url, ok(&tar)),
            (template(cu, "0.13.1").unwrap(), ok(format!("{}  {file}\n", sha(&tar)).as_bytes())),
        ]);
        let m = manager("py-dep-m", st);
        m.set_python("/nonexistent/python3");
        assert!(m.install_plan("esptool").unwrap().contains("Python 3 is not on this machine"));
        m.install("esptool", "root").unwrap();
        let j = wait(&m, "esptool");
        assert!(j.error.is_none(), "{j:?}");
        assert!(m.is_installed("uv").unwrap().verified, "uv came first, checked against the published checksum");
        let p = m.is_installed("python").unwrap();
        assert_eq!(p.version, def("python").unwrap().pin, "{p:?}");
        assert!(m.bin_dir().join("python3").exists() && m.bin_dir().join("uv").exists());
        // uv was pointed at this tool's own folder only
        let rec = std::fs::read_to_string(&seen).unwrap();
        assert!(rec.starts_with("3.12.16|") && rec.contains("/pkgs/python/") && rec.trim_end().ends_with("/shims"), "{rec}");
        assert_eq!(m.is_installed("esptool").unwrap().version, def("esptool").unwrap().pin);
    }

    #[test]
    fn a_node_that_lacks_libstdcpp_gets_it_from_alpine_and_starts_through_a_launcher() {
        let listing = "<a href=\"libstdc++-dev-15.2.0-r5.apk\">x</a>\n<a href=\"libstdc%2B%2B-15.2.0-r4.apk\">y</a>\n<a href=\"libstdc%2B%2B-15.2.0-r5.apk\">z</a>\n<a href=\"libgcc-15.2.0-r5.apk\">w</a>\n";
        assert_eq!(pick_libstdcpp(listing).as_deref(), Some("libstdc%2B%2B-15.2.0-r5.apk"));
        assert_eq!(pick_libstdcpp("<a href=\"libgcc-1.apk\">"), None);
        let node = "#!/bin/sh\ncase \"$LD_LIBRARY_PATH\" in *libstdcpp*) echo v26.1.0;; *) echo 'Error loading shared library libstdc++.so.6: No such file or directory (needed by /x/node)' >&2; exit 127;; esac\n";
        let base = format!("https://dl-cdn.alpinelinux.org/alpine/latest-stable/main/{}/", std::env::consts::ARCH);
        // an .apk is three gzip members: signature, control, files
        let mut apk = gzip(&make_tar(&[(".SIGN.RSA.x", b'0', "sig", 0o644)]));
        apk.extend(gzip(&make_tar(&[(".PKGINFO", b'0', "pkgname = libstdc++", 0o644)])));
        apk.extend(gzip(&make_tar(&[("usr/", b'5', "", 0o755), ("usr/lib/", b'5', "", 0o755), ("usr/lib/libstdc++.so.6.0.34", b'0', "the library", 0o755), ("usr/lib/libstdc++.so.6", b'2', "libstdc++.so.6.0.34", 0o777)])));
        let mut routes = node_routes(node, "#!/bin/sh\necho 11.0.0\n");
        routes.push((base.clone(), ok(listing.as_bytes())));
        routes.push((format!("{base}libstdc%2B%2B-15.2.0-r5.apk"), ok(&apk)));
        let m = manager("node-libstdcpp", fake(routes));
        m.install("nodejs", "root").unwrap();
        let j = wait(&m, "nodejs");
        assert!(j.error.is_none(), "{j:?}");
        let i = m.is_installed("nodejs").unwrap();
        let dir = m.pkg_dir("nodejs").join(m.dir_of(&i));
        assert_eq!(std::fs::read_to_string(dir.join("libstdcpp/libstdc++.so.6")).unwrap(), "the library");
        assert!(i.reported.contains("v26.1.0"), "{i:?}");
        // through the link in the tools folder, as the terminal and npm's scripts start it
        assert_eq!(run_quiet(&m.bin_dir().join("node").display().to_string(), &["--version"], 20).unwrap().trim(), "v26.1.0");
        assert!(!m.root.join("tmp/libstdcpp-unpack").exists(), "the scratch folder is removed");
    }

    const CLAUDE_BASE: &str = "https://downloads.claude.ai/claude-code-releases";

    /// Anthropic's bucket: `latest`, a manifest naming every platform's SHA-256, and one program per platform (a script that says which
    /// build it is). `wrong` lists platforms whose manifest checksum is made wrong.
    fn claude_routes(version: &str, platforms: &[&str], wrong: &[&str]) -> Vec<(String, Reply)> {
        let mut routes = vec![(format!("{CLAUDE_BASE}/latest"), ok(version.as_bytes()))];
        let mut entries = Vec::new();
        for p in platforms {
            let prog = format!("#!/bin/sh\necho '{version} (Claude Code) [{p}]'\n");
            let sum = if wrong.contains(p) { "0".repeat(64) } else { sha(prog.as_bytes()) };
            entries.push(format!(r#""{p}":{{"binary":"claude","checksum":"{sum}","size":{}}}"#, prog.len()));
            routes.push((format!("{CLAUDE_BASE}/{version}/{p}/claude"), ok(prog.as_bytes())));
        }
        routes.push((format!("{CLAUDE_BASE}/{version}/manifest.json"), ok(format!(r#"{{"version":"{version}","platforms":{{{}}}}}"#, entries.join(",")).as_bytes())));
        routes
    }

    #[test]
    fn claude_code_comes_as_anthropics_native_build_picked_by_libc_and_processor_and_checked_against_the_manifest() {
        // the platform names the manifest uses: musl and glibc, x64 and arm64; only the right program is offered, so a wrong pick fails
        for (musl, machine, platform) in [(true, "x86_64", "linux-x64-musl"), (false, "x86_64", "linux-x64"), (true, "aarch64", "linux-arm64-musl"), (false, "aarch64", "linux-arm64")] {
            let m = manager(&format!("claude-{platform}"), fake(claude_routes("2.1.302", &[platform], &[])));
            m.set_target(musl, machine);
            m.set_npm("/nonexistent/npm", "/nonexistent/node");
            m.install("claude-code", "root").unwrap();
            let j = wait(&m, "claude-code");
            assert!(j.error.is_none(), "{platform}: {j:?}");
            let i = m.is_installed("claude-code").unwrap();
            assert!(i.verified && i.version == "2.1.302" && i.reported.contains(&format!("[{platform}]")), "{i:?}");
            assert_eq!(m.extras(), vec![Extra { name: "claude".into(), host: Some("api.anthropic.com".into()) }]);
            // run through the folder of installed tools, like the terminal and the chat do
            let out = run_quiet(&m.bin_dir().join("claude").display().to_string(), &["--version"], 10).unwrap();
            assert!(out.contains("(Claude Code)"), "{out}");
            // no Node.js was involved
            assert!(m.is_installed("nodejs").is_none());
        }
        // the plan: a plain download, no Node.js step
        let m = manager("claude-order", fake(vec![]));
        assert_eq!(m.install_order("claude-code"), ["claude-code"]);
        assert_eq!(m.install_order("github-copilot"), ["github-copilot"]);
        let plan = m.install_plan("claude-code").unwrap();
        assert!(plan.contains("downloads.claude.ai") && plan.contains("no Node.js") && plan.contains("SHA-256") && plan.contains("`npm install -g @anthropic-ai/claude-code`"), "{plan}");
        assert!(m.install_plan("github-copilot").unwrap().contains("SHA256SUMS.txt"));
    }

    #[test]
    fn claude_code_with_a_wrong_checksum_is_refused_even_when_node_is_there_to_fall_back_on() {
        let d = tmp("claude-bad");
        let (npm, node, record) = fake_npm(&d, "v24.1.0");
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(claude_routes("2.1.302", &["linux-x64-musl"], &["linux-x64-musl"])));
        m.set_target(true, "x86_64");
        m.set_npm(&npm, &node);
        m.install("claude-code", "root").unwrap();
        let e = wait(&m, "claude-code").error.unwrap();
        assert!(e.contains("does not match the vendor's checksum"), "{e}");
        assert!(m.installed().is_empty() && !m.bin_dir().join("claude").exists() && !m.pkg_dir("claude-code").join("2.1.302").exists());
        assert!(!record.exists(), "a mismatch is never answered with another source");
        // a manifest that does not list this machine
        let m = manager("claude-noplat", fake(claude_routes("2.1.302", &["linux-arm64"], &[])));
        m.set_target(true, "x86_64");
        m.set_npm("/nonexistent/npm", "/nonexistent/node");
        m.install("claude-code", "root").unwrap();
        assert!(wait(&m, "claude-code").error.unwrap().contains("no entry for this machine"));
    }

    #[test]
    fn claude_code_falls_back_to_npm_only_when_the_download_fails_and_a_node_is_present() {
        let d = tmp("claude-npm");
        let (npm, node, record) = fake_npm(&d, "v22.4.0");
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![])); // the bucket cannot be reached
        m.set_npm(&npm, &node);
        m.install("claude-code", "root").unwrap();
        let j = wait(&m, "claude-code");
        assert!(j.error.is_none(), "{j:?}");
        assert!(std::fs::read_to_string(&record).unwrap().contains("ARGS install -g @anthropic-ai/claude-code\n"));
        let i = m.is_installed("claude-code").unwrap();
        assert!(!i.verified && i.version == "2.1.5" && m.bin_dir().join("claude").exists(), "{i:?}");
        // no Node.js: the download's own error is reported and nothing is left
        let m2 = manager("claude-nonode", fake(vec![]));
        m2.set_npm("/nonexistent/npm", "/nonexistent/node");
        m2.install("claude-code", "root").unwrap();
        let e = wait(&m2, "claude-code").error.unwrap();
        assert!(e.contains("could not reach") && !e.contains("npm"), "{e}");
        assert!(m2.installed().is_empty());
    }

    #[test]
    fn claude_code_updates_by_the_same_path_offers_upgrade_and_is_removed_with_its_folder() {
        let st = fake(claude_routes("2.1.302", &["linux-x64-musl"], &[]));
        let m = manager("claude-update", st.clone());
        m.set_target(true, "x86_64");
        assert_eq!(m.offer("claude-code", Some("2.1.302")), Offer::Install);
        m.install("claude-code", "root").unwrap();
        assert!(wait(&m, "claude-code").error.is_none());
        assert_eq!(m.check_latest("claude-code").unwrap(), "2.1.302");
        assert_eq!(m.offer("claude-code", Some("2.1.302")), Offer::UpToDate);
        assert_eq!(m.offer("claude-code", None), Offer::UpToDate);
        // a newer release appears: Upgrade is offered, and installing again replaces it, keeping the old for one rollback
        for (k, v) in claude_routes("2.1.310", &["linux-x64-musl"], &[]) {
            st.routes.lock().unwrap().insert(k, v);
        }
        let newer = m.check_latest("claude-code").unwrap();
        assert_eq!(m.offer("claude-code", Some(&newer)), Offer::Upgrade("2.1.310".into()));
        m.install("claude-code", "root").unwrap();
        assert!(wait(&m, "claude-code").error.is_none());
        let i = m.is_installed("claude-code").unwrap();
        assert_eq!((i.version.as_str(), i.previous.as_ref().map(|p| p.version.as_str())), ("2.1.310", Some("2.1.302")));
        assert_eq!(m.offer("claude-code", Some("2.1.310")), Offer::UpToDate);
        m.remove("claude-code").unwrap();
        assert!(!m.pkg_dir("claude-code").exists() && !m.bin_dir().join("claude").exists());
    }

    #[test]
    fn copilot_is_githubs_standalone_build_picked_by_libc_and_processor_and_checked_against_sha256sums() {
        for (musl, machine, name) in [(true, "x86_64", "copilot-linuxmusl-x64.tar.gz"), (false, "x86_64", "copilot-linux-x64.tar.gz"), (true, "aarch64", "copilot-linuxmusl-arm64.tar.gz"), (false, "aarch64", "copilot-linux-arm64.tar.gz")] {
            let tar = gzip(&make_tar(&[("copilot", b'0', &format!("#!/bin/sh\necho 'GitHub Copilot CLI 1.0.98 [{name}]'\n"), 0o755)]));
            let base = "https://github.com/github/copilot-cli/releases";
            let st = fake(vec![
                (format!("{base}/latest"), (302, vec![("location".into(), format!("{base}/tag/v1.0.98"))], vec![])),
                (format!("{base}/download/v1.0.98/{name}"), ok(&tar)),
                (format!("{base}/download/v1.0.98/SHA256SUMS.txt"), ok(format!("{}  other.tar.gz\n{}  {name}\n", "1".repeat(64), sha(&tar)).as_bytes())),
            ]);
            let m = manager(&format!("copilot-{name}"), st);
            m.set_target(musl, machine);
            m.set_npm("/nonexistent/npm", "/nonexistent/node");
            m.install("github-copilot", "root").unwrap();
            let j = wait(&m, "github-copilot");
            assert!(j.error.is_none(), "{name}: {j:?}");
            let i = m.is_installed("github-copilot").unwrap();
            assert!(i.verified && i.version == "v1.0.98" && i.reported.contains(&format!("[{name}]")), "{i:?}");
            assert!(m.is_installed("nodejs").is_none(), "no Node.js for the standalone build");
        }
        // a build that does not match the published checksum is refused
        let tar = gzip(&make_tar(&[("copilot", b'0', "#!/bin/sh\necho x\n", 0o755)]));
        let base = "https://github.com/github/copilot-cli/releases";
        let st = fake(vec![
            (format!("{base}/latest"), (302, vec![("location".into(), format!("{base}/tag/v1.0.98"))], vec![])),
            (format!("{base}/download/v1.0.98/copilot-linuxmusl-x64.tar.gz"), ok(&tar)),
            (format!("{base}/download/v1.0.98/SHA256SUMS.txt"), ok(format!("{}  copilot-linuxmusl-x64.tar.gz\n", "2".repeat(64)).as_bytes())),
        ]);
        let m = manager("copilot-bad", st);
        m.set_target(true, "x86_64");
        m.install("github-copilot", "root").unwrap();
        assert!(wait(&m, "github-copilot").error.unwrap().contains("does not match the vendor's checksum"));
        assert!(m.installed().is_empty());
    }

    #[test]
    fn node_is_the_active_lts_from_the_official_site_or_the_musl_builds_and_verified_against_shasums() {
        let idx = r#"[{"version":"v26.12.0","lts":false,"files":["linux-x64","linux-x64-musl"]},{"version":"v24.23.0","lts":"Krypton","files":["linux-x64"]},{"version":"v24.22.0","lts":"Krypton","files":["linux-x64","linux-x64-musl","linux-arm64-musl"]},{"version":"v22.23.3","lts":"Jod","files":["linux-x64-musl"]}]"#;
        // not the Current line, and only a release that has a build for this machine
        assert_eq!(node_lts_pick(idx, "linux-x64-musl").as_deref(), Some("v24.22.0"));
        assert_eq!(node_lts_pick(idx, "linux-x64").as_deref(), Some("v24.23.0"));
        assert_eq!(node_lts_pick(idx, "linux-arm64"), None);
        assert_eq!(node_lts_pick("not json", "linux-x64"), None);
        // glibc downloads come from nodejs.org, musl ones from the unofficial builds; arm64 is named arm64
        assert_eq!(template_for("{cplatform}", "", true, "aarch64").unwrap(), "linux-arm64-musl");
        assert_eq!(template_for("https://{node_dist}/{v}/node-{v}-linux-{node_arch}{node_libc}.tar.gz", "v24.22.0", true, "x86_64").unwrap(), "https://unofficial-builds.nodejs.org/download/release/v24.22.0/node-v24.22.0-linux-x64-musl.tar.gz");
        assert_eq!(template_for("https://{node_dist}/{v}/node-{v}-linux-{node_arch}{node_libc}.tar.gz", "v24.22.0", false, "aarch64").unwrap(), "https://nodejs.org/dist/v24.22.0/node-v24.22.0-linux-arm64.tar.gz");
        // end to end for both libcs: the index, the tarball and SHASUMS256.txt of the matching host
        for musl in [true, false] {
            let d = def("nodejs").unwrap();
            let (Source::TarGz(tu), Some(Checksum::Lines(cu)), VersionFrom::NodeLts(vu)) = (d.source, d.checksum, d.version) else { panic!("shape") };
            let plat = template_for("{cplatform}", "", musl, "x86_64").unwrap();
            let url = template_for(tu, "v24.22.0", musl, "x86_64").unwrap();
            let file = url.rsplit('/').next().unwrap().to_string();
            let top = file.trim_end_matches(".tar.gz").to_string();
            let tar = gzip(&make_tar(&[(&format!("{top}/bin/node"), b'0', "#!/bin/sh\necho v24.22.0\n", 0o755), (&format!("{top}/bin/npm"), b'0', "#!/bin/sh\necho 11.0.0\n", 0o755)]));
            let st = fake(vec![
                (template_for(vu, "", musl, "x86_64").unwrap(), ok(format!(r#"[{{"version":"v26.0.0","lts":false,"files":["{plat}"]}},{{"version":"v24.22.0","lts":"Krypton","files":["{plat}"]}}]"#).as_bytes())),
                (url.clone(), ok(&tar)),
                (template_for(cu, "v24.22.0", musl, "x86_64").unwrap(), ok(format!("{}  {file}\n", sha(&tar)).as_bytes())),
            ]);
            let m = manager(&format!("node-lts-{musl}"), st.clone());
            m.set_target(musl, "x86_64");
            m.install("nodejs", "root").unwrap();
            let j = wait(&m, "nodejs");
            assert!(j.error.is_none(), "{musl}: {j:?}");
            let i = m.is_installed("nodejs").unwrap();
            assert!(i.verified && i.version == "v24.22.0", "{i:?}");
            assert!(st.asked.lock().unwrap().iter().any(|u| u.contains(if musl { "unofficial-builds.nodejs.org" } else { "://nodejs.org/dist" })));
            assert!(st.asked.lock().unwrap().iter().any(|u| u.ends_with("/SHASUMS256.txt")));
            // a tampered tarball is refused
            let bad = fake(vec![
                (template_for(vu, "", musl, "x86_64").unwrap(), ok(format!(r#"[{{"version":"v24.22.0","lts":"Krypton","files":["{plat}"]}}]"#).as_bytes())),
                (url, ok(&tar)),
                (template_for(cu, "v24.22.0", musl, "x86_64").unwrap(), ok(format!("{}  {file}\n", "a".repeat(64)).as_bytes())),
            ]);
            let m = manager(&format!("node-lts-bad-{musl}"), bad);
            m.set_target(musl, "x86_64");
            m.install("nodejs", "root").unwrap();
            assert!(wait(&m, "nodejs").error.unwrap().contains("does not match the vendor's checksum"));
            assert!(m.installed().is_empty());
        }
    }

    #[test]
    fn the_order_of_an_install_shows_the_runtimes_first_and_removing_a_runtime_warns_which_tools_need_it() {
        let m = manager("order", fake(vec![]));
        m.set_npm("/nonexistent/npm", "/nonexistent/node");
        m.set_python("/nonexistent/python3");
        assert_eq!(m.install_order("codex"), ["nodejs", "codex"]);
        assert_eq!(m.install_order("gemini-cli"), ["nodejs", "gemini-cli"]);
        assert_eq!(m.install_order("aider"), ["uv", "python", "aider"]);
        assert_eq!(m.install_order("python"), ["uv", "python"]);
        assert_eq!(m.install_order("nodejs"), ["nodejs"]);
        assert_eq!(m.install_order("claude-code"), ["claude-code"]);
        assert_eq!(m.install_order("kubectl"), ["kubectl"]);
        // nothing needs a runtime that nothing was installed with
        assert!(m.uninstall_warning("nodejs").is_none());
        write_fake_installed(&m, "nodejs", "#!/bin/sh\necho v24.1.0\n");
        write_fake_installed(&m, "codex", "#!/bin/sh\necho 1\n");
        write_fake_installed(&m, "gemini-cli", "#!/bin/sh\necho 1\n");
        write_fake_installed(&m, "claude-code", "#!/bin/sh\necho 1\n");
        write_fake_installed(&m, "python", "#!/bin/sh\necho Python 3.12.1\n");
        assert_eq!(m.install_order("codex"), ["codex"], "Node.js is there now");
        let w = m.uninstall_warning("nodejs").unwrap();
        assert!(w.contains("Node.js is used by") && w.contains("OpenAI Codex CLI") && w.contains("Gemini CLI") && !w.contains("Claude Code"), "{w}");
        assert!(m.uninstall_warning("python").is_none(), "no pip tool is installed");
        write_fake_installed(&m, "aider", "#!/bin/sh\necho aider 0.1\n");
        assert!(m.uninstall_warning("python").unwrap().contains("Aider"));
        assert!(m.uninstall_warning("claude-code").is_none() && m.uninstall_warning("kubectl").is_none());
        // removal itself still works (the screen asks first)
        m.remove("nodejs").unwrap();
        assert!(m.is_installed("nodejs").is_none());
    }

    #[test]
    fn upgrades_are_known_for_npm_and_pip_tools_and_python_and_compared_by_number() {
        assert!(version_is_newer("2.1.5", "2.1.10") && version_is_newer("v1.9.0", "v1.10.0") && version_is_newer("3.12.1", "3.12.15"));
        assert!(!version_is_newer("2.1.10", "2.1.5") && !version_is_newer("v24.22.0", "24.22.0") && !version_is_newer("1.2", "1.2.0"));
        assert!(version_is_newer("weird", "other") && !version_is_newer("same", "same"));
        let sums = format!("{}  cpython-3.12.9+20260101-x86_64-unknown-linux-musl-install_only.tar.gz\n{}  cpython-3.12.15+20261013-x86_64-unknown-linux-musl-install_only.tar.gz\n{}  cpython-3.13.9+20261013-x86_64-unknown-linux-musl-install_only.tar.gz\n{}  cpython-3.12.20+20261013-x86_64-unknown-linux-gnu-install_only.tar.gz\n", "1".repeat(64), "2".repeat(64), "3".repeat(64), "4".repeat(64));
        assert_eq!(pbs_pick(&sums, "3.12", "x86_64", true).as_deref(), Some("3.12.15"));
        assert_eq!(pbs_pick(&sums, "3.12", "x86_64", false).as_deref(), Some("3.12.20"));
        assert_eq!(pbs_pick(&sums, "3.12", "aarch64", true), None);
        assert_eq!(manifest_checksum(&format!(r#"{{"platforms":{{"linux-x64":{{"checksum":"{}"}},"bad":{{"checksum":"zz"}}}}}}"#, "A".repeat(64)), "linux-x64").as_deref(), Some(&*"a".repeat(64)));
        assert_eq!(manifest_checksum(r#"{"platforms":{"bad":{"checksum":"zz"}}}"#, "bad"), None);
        let st = fake(vec![
            ("https://registry.npmjs.org/@openai%2Fcodex/latest".into(), ok(br#"{"name":"@openai/codex","version":"0.99.0"}"#)),
            ("https://pypi.org/pypi/aider-chat/json".into(), ok(br#"{"info":{"version":"0.90.1"},"releases":{}}"#)),
            ("https://github.com/astral-sh/python-build-standalone/releases/latest/download/SHA256SUMS".into(), (302, vec![("location".into(), "https://github.com/astral-sh/python-build-standalone/releases/download/20261013/SHA256SUMS".into())], vec![])),
            ("https://github.com/astral-sh/python-build-standalone/releases/download/20261013/SHA256SUMS".into(), ok(sums.as_bytes())),
        ]);
        let m = manager("upgrades", st);
        m.set_target(true, "x86_64");
        // every npm, pip and Python tool is pinned in this release: the answer is the pin, not what the registry says
        for id in ["codex", "aider", "python"] {
            assert_eq!(m.check_latest(id).unwrap(), def(id).unwrap().pin);
        }
        write_fake_installed(&m, "codex", "#!/bin/sh\necho 1\n"); // recorded as version v1
        assert_eq!(m.offer("codex", Some("v2.0.0")), Offer::Upgrade("v2.0.0".into()));
        assert_eq!(m.offer("codex", Some("v1")), Offer::UpToDate);
        assert_eq!(m.offer("aider", Some("0.86.2")), Offer::Install);
    }


    /// A minimal 64-bit ELF header with one PT_INTERP naming `interp`.
    fn fake_elf(interp: &str) -> Vec<u8> {
        let mut b = vec![0u8; 64 + 56];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        let off = b.len() as u64;
        b[64..68].copy_from_slice(&3u32.to_le_bytes());
        b[72..80].copy_from_slice(&off.to_le_bytes());
        b[96..104].copy_from_slice(&((interp.len() + 1) as u64).to_le_bytes());
        b.extend_from_slice(interp.as_bytes());
        b.push(0);
        b
    }

    #[test]
    fn a_program_built_for_another_c_library_is_refused_with_a_plain_message_and_nothing_is_installed() {
        let d = tmp("elf");
        let glibc = d.join("glibc-prog");
        std::fs::write(&glibc, fake_elf("/nonexistent/lib64/ld-linux-x86-64.so.2")).unwrap();
        assert_eq!(elf_interpreter(&glibc).as_deref(), Some("/nonexistent/lib64/ld-linux-x86-64.so.2"));
        let e = runnable_check(&glibc).unwrap_err();
        assert!(e.contains("glibc") || e.contains("ld-linux"), "{e}");
        let ok_prog = d.join("sh-prog");
        std::fs::write(&ok_prog, fake_elf("/bin/sh")).unwrap();
        assert!(runnable_check(&ok_prog).is_ok());
        let script = d.join("script");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        assert!(elf_interpreter(&script).is_none() && runnable_check(&script).is_ok() && runnable_check(&d.join("missing")).is_ok());
        // a vendor file with a missing loader: the job fails, nothing is recorded or linked, and the old install (none) is untouched
        let (arch, _, _) = arch().unwrap();
        let bin = fake_elf("/nonexistent/lib64/ld-linux-x86-64.so.2");
        let st = fake(vec![
            ("https://dl.k8s.io/release/stable.txt".into(), ok(b"v1.31.0")),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl"), ok(&bin)),
            (format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl.sha256"), ok(sha(&bin).as_bytes())),
        ]);
        let m = manager("elf-glibc", st);
        m.install("kubectl", "root").unwrap();
        let e = wait(&m, "kubectl").error.unwrap();
        assert!(e.contains("kubectl v1.31.0 was downloaded but cannot run here"), "{e}");
        assert!(m.installed().is_empty() && !m.bin_dir().join("kubectl").exists() && !m.pkg_dir("kubectl").join("v1.31.0").exists());
        // an installed tool whose program went bad is reported as installed but not runnable
        write_fake_installed(&m, "helm", "#!/bin/sh\necho v3\n");
        assert!(m.health("helm").is_none());
        std::fs::write(m.pkg_dir("helm").join("v1").join("helm"), fake_elf("/nonexistent/lib64/ld-linux-x86-64.so.2")).unwrap();
        assert!(m.health("helm").unwrap().contains("installed but cannot run here"));
        std::fs::remove_file(m.pkg_dir("helm").join("v1").join("helm")).unwrap();
        assert!(m.health("helm").unwrap().contains("missing"));
        assert!(m.health("kubectl").is_none(), "not installed: nothing to report");
    }

    #[test]
    fn glibc_only_releases_are_not_offered_on_musl_and_espflash_takes_its_musl_build() {
        let m = manager("musl-gate", fake(vec![]));
        m.set_target(true, "x86_64");
        for id in ["aws-cli", "gcloud", "ollama"] {
            let e = m.install(id, "root").unwrap_err();
            assert!(e.contains("is not available on this image") && e.contains("glibc"), "{id}: {e}");
        }
        m.set_target(true, "aarch64");
        assert!(m.install("espflash", "root").unwrap_err().contains("x86_64 only"));
        assert_eq!(template_for("espflash-{arch2}-unknown-linux-{libc_triple}.zip", "", true, "x86_64").unwrap(), "espflash-x86_64-unknown-linux-musl.zip");
        assert_eq!(template_for("espflash-{arch2}-unknown-linux-{libc_triple}.zip", "", false, "aarch64").unwrap(), "espflash-aarch64-unknown-linux-gnu.zip");
        // glibc machines get everything
        let g = manager("glibc-gate", fake(vec![]));
        g.set_target(false, "x86_64");
        assert!(g.install("aws-cli", "root").is_ok(), "offered on glibc");
    }

    #[test]
    fn duckdb_is_installed_from_its_python_wheel_with_a_duckdb_command_and_says_which_version() {
        let d = tmp("duckdb");
        let pip_seen = d.join("pip-seen.txt");
        let fake_py = d.join("python3");
        // the stand-in python makes the environment with a `python` that reports the wheel's version, and a pip that records its arguments
        std::fs::write(&fake_py, format!("#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then exit 0; fi\nenv=\"$3\"\nmkdir -p \"$env/bin\"\nprintf '#!/bin/sh\\necho \"v1.3.2 via Python 3 (the newest DuckDB wheel built for musl)\"\\n' > \"$env/bin/python\"\nchmod +x \"$env/bin/python\"\nprintf '#!/bin/sh\\necho \"$*\" > {rec}\\n' > \"$env/bin/pip\"\nchmod +x \"$env/bin/pip\"\n", rec = pip_seen.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_py, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_python(&fake_py.display().to_string());
        m.set_target(true, "x86_64");
        m.install("duckdb", "root").unwrap();
        let j = wait(&m, "duckdb");
        assert!(j.error.is_none(), "{j:?}");
        assert!(std::fs::read_to_string(&pip_seen).unwrap().contains("--only-binary=:all: duckdb==1.3.2"));
        let i = m.is_installed("duckdb").unwrap();
        assert_eq!(i.version, "1.3.2");
        assert!(i.reported.contains("via Python 3") && i.reported.contains("newest DuckDB wheel built for musl"), "{i:?}");
        // the command in the tools folder is the wrapper around our program; extras keep the DASH tile's read-only network rule
        let link = m.bin_dir().join("duckdb");
        assert!(link.exists());
        let wrapper = std::fs::read_to_string(std::fs::canonicalize(&link).unwrap()).unwrap();
        assert!(wrapper.starts_with("#!/bin/sh\nexec '") && wrapper.contains("venv/bin/python") && wrapper.contains("duckdb.py"), "{wrapper}");
        assert_eq!(m.extras(), vec![Extra { name: "duckdb".into(), host: Some("-".into()) }]);
        // the newest wheel for musl, not the newest release
        let j: serde_json::Value = serde_json::from_str(r#"{"releases":{
            "1.5.6":[{"filename":"duckdb-1.5.6-cp312-cp312-manylinux_2_27_x86_64.whl"}],
            "1.4.0":[{"filename":"duckdb-1.4.0-cp312-cp312-musllinux_1_2_x86_64.whl","yanked":true}],
            "1.3.2":[{"filename":"duckdb-1.3.2-cp312-cp312-musllinux_1_2_x86_64.whl"},{"filename":"duckdb-1.3.2-cp313-cp313-musllinux_1_2_x86_64.whl"}],
            "1.10.0rc1":[{"filename":"duckdb-1.10.0rc1-cp312-cp312-musllinux_1_2_x86_64.whl"}],
            "1.2.9":[{"filename":"duckdb-1.2.9-cp312-cp312-musllinux_1_2_x86_64.whl"}]}}"#).unwrap();
        assert_eq!(pypi_newest_with_wheel(&j, "musllinux_1_2_x86_64", "cp312").as_deref(), Some("1.3.2"));
        assert_eq!(pypi_newest_with_wheel(&j, "musllinux_1_2_aarch64", "cp312"), None);
        let st = fake(vec![("https://pypi.org/pypi/duckdb/json".into(), ok(br#"{"info":{"version":"1.5.6"},"releases":{"1.5.6":[{"filename":"duckdb-1.5.6-cp312-cp312-manylinux_2_27_x86_64.whl"}],"1.3.2":[{"filename":"duckdb-1.3.2-cp312-cp312-musllinux_1_2_x86_64.whl"}]}}"#))]);
        let m2 = manager("duckdb-latest", st);
        m2.set_target(true, "x86_64");
        assert_eq!(m2.check_latest("duckdb").unwrap(), def("duckdb").unwrap().pin, "pinned to the newest wheel that exists for musl");
    }

    #[test]
    fn the_duckdb_command_keeps_the_calls_the_dash_tile_makes_working() {
        // run the real program with the system's python and a stand-in `duckdb` module (skipped without python3)
        let Ok(py) = run_quiet("python3", &["--version"], 10) else { return };
        let _ = py;
        let d = tmp("duckdb-shim");
        std::fs::write(d.join("duckdb.py"), "__version__ = '1.3.2'\nclass _Cur:\n    def __init__(s, sql):\n        s.sql = sql\n        s.description = [('a',), ('b',)] if sql.lower().startswith('select') else None\n    def fetchall(s):\n        return [(1, 'x'), (2, None)]\nclass _Con:\n    def execute(s, sql):\n        if 'boom' in sql:\n            raise RuntimeError('Catalog Error: boom')\n        return _Cur(sql)\ndef connect(db=':memory:', read_only=False):\n    open(__file__ + '.opened', 'w').write(db + ' ' + str(read_only))\n    return _Con()\n").unwrap();
        std::fs::write(d.join("shim.py"), include_str!("duckdb_shim.py")).unwrap();
        let shim = d.join("shim.py").display().to_string();
        let run = |args: &[&str]| -> Result<String, String> {
            let mut a = vec![shim.as_str()];
            a.extend_from_slice(args);
            // run_quiet clears the environment, so the stand-in module is found through its folder as the script's own directory
            run_quiet_env("python3", &a, &[("PYTHONPATH", &d.display().to_string())], 20)
        };
        // exactly what DASH sends
        let out = run(&["-json", "-batch", "-no-stdin", "-init", "/dev/null", "-readonly", "/data/x.db", "-c", "SELECT * FROM t"]).unwrap();
        assert_eq!(out.trim(), "[{\"a\": 1, \"b\": \"x\"},\n{\"a\": 2, \"b\": null}]");
        assert_eq!(std::fs::read_to_string(d.join("duckdb.py.opened")).unwrap(), "/data/x.db True");
        assert!(run(&["-version"]).unwrap().starts_with("v1.3.2 via Python 3"));
        assert_eq!(run(&["-csv", "-c", "select 1"]).unwrap(), "a,b\n1,x\n2,\n");
        assert_eq!(run(&["-json", "-c", "create table t(a int)"]).unwrap().trim(), "");
        let e = run(&["-c", "select boom"]).unwrap_err();
        assert!(e.contains("Catalog Error: boom"), "{e}");
        assert!(run(&["-nonsense"]).unwrap_err().contains("unknown option"));
    }

    #[test]
    fn a_tool_that_is_on_the_machine_already_counts_as_installed_and_installing_it_is_a_no_op() {
        let d = tmp("present");
        let sys = d.join("sysbin");
        std::fs::create_dir_all(&sys).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(sys.join("kubectl"), "#!/bin/sh\necho 'Client Version: v1.30.0'\n").unwrap();
        std::fs::set_permissions(sys.join("kubectl"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_search_path(Some(&sys.display().to_string()));
        // detection is off unless the shell turns it on: the machine's own programs do not matter to tests
        assert_eq!(m.offer("kubectl", None), Offer::Install);
        m.set_detect_present(true);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        m.set_on_installed(Arc::new(move |id| s2.lock().unwrap().push(id.to_string())));
        match m.offer("kubectl", None) {
            Offer::Elsewhere(v) => assert!(v.contains("v1.30.0") && v.contains("sysbin"), "{v}"),
            o => panic!("{o:?}"),
        }
        m.install("kubectl", "root").unwrap();
        let j = m.job("kubectl").unwrap();
        assert!(j.finished && j.error.is_none() && j.phase.starts_with("installed \u{b7} ") && j.phase.contains("v1.30.0"), "{j:?}");
        assert!(m.installed().is_empty(), "nothing was downloaded or recorded");
        assert_eq!(*seen.lock().unwrap(), ["kubectl"]);
        assert_eq!(m.offer("helm", None), Offer::Install);
        // the runtimes: a Node.js that is too old does not count
        std::fs::write(sys.join("node"), "#!/bin/sh\necho v16.1.0\n").unwrap();
        std::fs::set_permissions(sys.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(m.present_elsewhere("nodejs").is_none());
    }

    #[test]
    fn the_tools_folder_comes_first_on_the_search_path_and_a_missing_node_names_the_menu_not_a_package_manager() {
        let d = tmp("pathfirst");
        let m = Arc::new(CliManager::new(&d.join("cli")));
        let before = std::env::var("PATH").unwrap_or_default();
        m.export_path();
        let after = std::env::var("PATH").unwrap();
        assert!(after.starts_with(&format!("{}:", m.bin_dir().display())), "{after}");
        m.export_path();
        assert_eq!(std::env::var("PATH").unwrap(), after, "idempotent");
        std::env::set_var("PATH", before);
        m.set_npm("/nonexistent/npm", "/nonexistent/node");
        let e = m.node_check().unwrap_err();
        assert!(e.contains("Node.js is not installed: Store") && e.contains("Setup") && !e.contains("apt") && !e.contains("brew") && !e.contains("sudo"), "{e}");
        m.set_python("/nonexistent/python3");
        let e = m.python_check().unwrap_err();
        assert!(e.contains("Python 3 is not installed: Store") && !e.contains("apt") && !e.contains("brew"), "{e}");
    }

    #[test]
    fn ollama_comes_as_a_zstd_tar_with_its_libraries_beside_the_program_and_the_checksum_file_names_it_with_dot_slash() {
        let (arch, _, _) = arch().unwrap();
        let tar = crate::archive::tests::make_tar(&[
            ("bin/", b'5', "", 0o755),
            ("bin/ollama", b'0', "#!/bin/sh\necho 'ollama version is 0.40.3'\n", 0o755),
            ("lib/ollama/libggml.so", b'0', &"x".repeat(5000), 0o644),
        ]);
        let z = ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest);
        let name = format!("ollama-linux-{arch}.tar.zst");
        let st = fake(vec![
            ("https://github.com/ollama/ollama/releases/latest".into(), (302, vec![("location".into(), "https://github.com/ollama/ollama/releases/tag/v0.40.3".into())], vec![])),
            (format!("https://github.com/ollama/ollama/releases/download/v0.40.3/{name}"), ok(&z)),
            ("https://github.com/ollama/ollama/releases/download/v0.40.3/sha256sum.txt".into(), ok(format!("{}  ./{name}\n{}  ./other.tgz\n", sha(&z), "0".repeat(64)).as_bytes())),
        ]);
        let m = manager("ollama", st);
        m.install("ollama", "root").unwrap();
        let j = wait(&m, "ollama");
        assert!(j.error.is_none() && j.finished, "{j:?}");
        let i = m.is_installed("ollama").unwrap();
        assert_eq!((i.version.as_str(), i.verified), ("v0.40.3", true));
        assert!(i.reported.contains("0.40.3"), "{:?}", i.reported);
        // the libraries sit next to the program where it looks for them
        let real = std::fs::canonicalize(m.bin_dir().join("ollama")).unwrap();
        assert!(real.ends_with("bin/ollama") && real.parent().unwrap().parent().unwrap().join("lib/ollama/libggml.so").exists());
        assert_eq!(m.extras(), vec![Extra { name: "ollama".into(), host: Some("-".into()) }]);
        assert_eq!(def("ollama").unwrap().limits().0, 3 << 30);
        assert_eq!(def("kubectl").unwrap().limits().0, MAX_DOWNLOAD);
    }

    #[test]
    fn openvpn_has_no_built_in_address_and_the_click_explains_the_gap_and_fetches_nothing() {
        let d = def("openvpn").unwrap();
        assert!(matches!(d.source, Source::Setting("vpn.openvpn_download")), "the address is a setting, not a constant");
        assert!(d.license.contains("GPL-2.0") && d.note.contains("does not ship it") && d.note.contains("none is invented"));
        let st = Arc::new(Fake { routes: Mutex::new(HashMap::new()), asked: Mutex::new(vec![]) });
        let m = manager("openvpn", st.clone());
        let plan = m.install_plan("openvpn").unwrap();
        assert!(plan.contains("Nothing can be downloaded yet") && plan.contains("vpn.openvpn_download"), "{plan}");
        m.install("openvpn", "root").unwrap();
        let j = wait(&m, "openvpn");
        let e = j.error.unwrap();
        assert!(e.contains("no download address") && e.contains("does not invent") && e.contains("Nothing was fetched"), "{e}");
        assert!(st.asked.lock().unwrap().is_empty(), "no request was made");
        assert!(m.is_installed("openvpn").is_none());
    }

    #[test]
    fn ffmpeg_comes_as_a_tar_xz_checked_against_the_builders_md5_file() {
        let (arch, _, _) = arch().unwrap();
        let tar = crate::archive::tests::make_tar(&[
            ("ffmpeg-7.0.2-static/", b'5', "", 0o755),
            ("ffmpeg-7.0.2-static/ffmpeg", b'0', "#!/bin/sh\necho 'ffmpeg version 7.0.2-static'\n", 0o755),
            ("ffmpeg-7.0.2-static/ffprobe", b'0', "#!/bin/sh\necho 'ffprobe version 7.0.2-static'\n", 0o755),
            ("ffmpeg-7.0.2-static/manpages/ffmpeg.txt", b'0', &"x".repeat(70000), 0o644),
        ]);
        let xz = crate::xz::make_xz_stored(&tar);
        let name = format!("ffmpeg-release-{arch}-static.tar.xz");
        let md5 = {
            let mut m = crate::xz::Md5::new();
            m.update(&xz);
            m.hex()
        };
        let url = format!("https://johnvansickle.com/ffmpeg/releases/{name}");
        let st = fake(vec![(url.clone(), ok(&xz)), (format!("{url}.md5"), ok(format!("{md5}  {name}\n").as_bytes()))]);
        let m = manager("ffmpeg", st);
        m.install("ffmpeg", "root").unwrap();
        let j = wait(&m, "ffmpeg");
        assert!(j.error.is_none() && j.finished, "{j:?}");
        let i = m.is_installed("ffmpeg").unwrap();
        assert!(i.verified && i.reported.contains("7.0.2"), "{i:?}");
        assert!(m.bin_dir().join("ffmpeg").exists() && m.bin_dir().join("ffprobe").exists());
        assert_eq!(def("ffmpeg").unwrap().hosts, &["johnvansickle.com"]);

        // a wrong digest installs nothing
        let st = fake(vec![(url.clone(), ok(&xz)), (format!("{url}.md5"), ok(format!("{}  {name}\n", "0".repeat(32)).as_bytes()))]);
        let m = manager("ffmpeg-bad", st);
        m.install("ffmpeg", "root").unwrap();
        let j = wait(&m, "ffmpeg");
        assert!(j.error.as_deref().is_some_and(|e| e.contains("checksum")), "{j:?}");
        assert!(m.is_installed("ffmpeg").is_none());
    }

    #[test]
    fn seven_zip_comes_from_the_projects_release_page_as_a_tar_xz_and_is_listed_as_pinned_and_verified() {
        let d = def("7zip").unwrap();
        assert!(matches!(d.checksum, Some(Checksum::Fixed(_))) && d.bins == [("7zz", "7zzs")] && d.hosts.contains(&"github.com"));
        assert_eq!(tpl_glibc("{v7}", "25.01").unwrap(), "2501");
        let Source::TarXz(tu) = d.source else { panic!("the 7zip entry changed shape") };
        let url = tpl_glibc(tu, d.pin).unwrap();
        assert!(url.starts_with("https://github.com/ip7z/7zip/releases/download/26.05/7z2605-linux-"), "{url}");
        let tar = crate::archive::tests::make_tar(&[("7zz", b'0', "#!/bin/sh\necho '7zz dynamic'\n", 0o755), ("7zzs", b'0', "#!/bin/sh\necho '7-Zip (z) 25.01 (x64) static'\n", 0o755), ("License.txt", b'0', "LGPL", 0o644)]);
        let xz = crate::xz::make_xz_stored(&tar);
        TEST_FIXED.lock().unwrap().push((url.rsplit('/').next().unwrap().to_string(), sha(&xz)));
        let st = fake(vec![(url, ok(&xz))]);
        let m = manager("7zip", st);
        m.install("7zip", "root").unwrap();
        let j = wait(&m, "7zip");
        assert!(j.error.is_none() && j.finished, "{j:?}");
        let i = m.is_installed("7zip").unwrap();
        assert!(i.verified, "checked against the digest GitHub publishes for the pinned release");
        let link = m.bin_dir().join("7zz");
        assert!(link.exists(), "7zz is linked");
        assert!(std::fs::read_to_string(&link).unwrap().contains("static"), "it is the static build, 7zzs");
    }

    fn kubectl_routes(st: &Fake, version: &str, program: &str) {
        let (arch, _, _) = arch().unwrap();
        let bin = program.as_bytes();
        let mut r = st.routes.lock().unwrap();
        r.insert("https://dl.k8s.io/release/stable.txt".into(), ok(version.as_bytes()));
        r.insert(format!("https://dl.k8s.io/release/{version}/bin/linux/{arch}/kubectl"), ok(bin));
        r.insert(format!("https://dl.k8s.io/release/{version}/bin/linux/{arch}/kubectl.sha256"), ok(sha(bin).as_bytes()));
    }

    #[test]
    fn an_update_keeps_the_old_version_for_one_rollback_and_a_new_version_that_does_not_start_never_replaces_the_old() {
        let st = fake(vec![]);
        kubectl_routes(&st, "v1.31.0", "#!/bin/sh\necho 'Client Version: v1.31.0'\n");
        let m = manager("update", st.clone());
        m.install("kubectl", "root").unwrap();
        wait(&m, "kubectl");
        assert_eq!(m.check_latest("kubectl").unwrap(), "v1.31.0");
        let link = m.bin_dir().join("kubectl");
        let first = std::fs::read_link(&link).unwrap();
        // a new version that cannot start: the old one stays, linked and recorded
        kubectl_routes(&st, "v1.32.0", "#!/bin/sh\nexit 3\n");
        assert_eq!(m.check_latest("kubectl").unwrap(), "v1.32.0");
        m.install("kubectl", "root").unwrap();
        let j = wait(&m, "kubectl");
        assert!(j.error.as_deref().unwrap_or("").contains("does not start") && j.error.as_deref().unwrap_or("").contains("v1.31.0 stays installed"), "{j:?}");
        let i = m.is_installed("kubectl").unwrap();
        assert_eq!((i.version.as_str(), i.previous.is_none()), ("v1.31.0", true));
        assert_eq!(std::fs::read_link(&link).unwrap(), first);
        assert!(!m.pkg_dir("kubectl").join("v1.32.0").exists(), "nothing half-installed");
        // a good one takes over and the old one is kept
        kubectl_routes(&st, "v1.32.0", "#!/bin/sh\necho 'Client Version: v1.32.0'\n");
        m.install("kubectl", "root").unwrap();
        wait(&m, "kubectl");
        let i = m.is_installed("kubectl").unwrap();
        assert_eq!((i.version.as_str(), i.previous.as_ref().map(|p| p.version.as_str())), ("v1.32.0", Some("v1.31.0")));
        assert!(m.pkg_dir("kubectl").join("v1.31.0").is_dir() && m.pkg_dir("kubectl").join("v1.32.0").is_dir());
        assert_ne!(std::fs::read_link(&link).unwrap(), first);
        // rollback swaps them; forward again is the same call
        assert!(m.rollback("kubectl").unwrap().contains("back at v1.31.0"));
        assert_eq!(std::fs::read_link(&link).unwrap(), first);
        let i = m.is_installed("kubectl").unwrap();
        assert_eq!((i.version.as_str(), i.previous.as_ref().map(|p| p.version.as_str())), ("v1.31.0", Some("v1.32.0")));
        // a third version drops the oldest folder: one generation only
        kubectl_routes(&st, "v1.33.0", "#!/bin/sh\necho 'Client Version: v1.33.0'\n");
        m.install("kubectl", "root").unwrap();
        wait(&m, "kubectl");
        assert!(!m.pkg_dir("kubectl").join("v1.32.0").exists() && m.pkg_dir("kubectl").join("v1.31.0").is_dir() && m.pkg_dir("kubectl").join("v1.33.0").is_dir());
        // a cut-off between the record and the links is mended from the record
        std::os::unix::fs::symlink(&first, m.bin_dir().join(".tmp")).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&first, &link).unwrap();
        assert_eq!(m.repair_all(), vec!["kubectl".to_string()]);
        assert!(std::fs::read_link(&link).unwrap().to_string_lossy().contains("v1.33.0"));
        assert!(m.repair_all().is_empty());
        // nothing kept: nothing to go back to; removing deletes the kept version too
        m.remove("kubectl").unwrap();
        assert!(!m.pkg_dir("kubectl").exists() && m.rollback("kubectl").is_err());
        // a tool whose vendor publishes only "latest" says so
        assert!(m.check_latest("ffmpeg").unwrap_err().contains("only \"latest\""));
    }
}
