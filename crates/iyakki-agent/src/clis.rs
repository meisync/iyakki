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
}

#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// One program file.
    File(&'static str),
    TarGz(&'static str),
    Zip(&'static str),
    /// A zstd-compressed tar, unpacked while it is decompressed (Ollama ships this way).
    TarZst(&'static str),
    /// A Python package installed into its own environment (needs `python3` with `venv`).
    Pip(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub enum Checksum {
    /// A file holding only the hash (or `hash  name`).
    Single(&'static str),
    /// A file of `hash  name` lines; the line for the downloaded file's name is used.
    Lines(&'static str),
}

pub struct CliDef {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub home: &'static str,
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
    all.push(Installed { id: id.into(), version: "v1".into(), reported: String::new(), at: 1, bins: vec![id.into()], verified: true, by: "root".into() });
    m.store(&all).unwrap();
}

pub const CATALOG: &[CliDef] = &[
    CliDef {
        id: "kubectl",
        name: "kubectl",
        about: "Talk to a Kubernetes cluster: pods, deployments, logs.",
        home: "https://kubernetes.io/docs/reference/kubectl/",
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
        version: VersionFrom::Json("https://checkpoint-api.hashicorp.com/v1/check/terraform", "/current_version"),
        source: Source::Zip("https://releases.hashicorp.com/terraform/{vn}/terraform_{vn}_linux_{arch}.zip"),
        checksum: Some(Checksum::Lines("https://releases.hashicorp.com/terraform/{vn}/terraform_{vn}_SHA256SUMS")),
        bins: &[("terraform", "terraform")],
        hosts: &["hashicorp.com"],
        net_host: None,
        version_args: &["version"],
        note: "",
    },
    CliDef {
        id: "gh",
        name: "GitHub CLI",
        about: "Repositories, issues and pull requests from the command line.",
        home: "https://cli.github.com",
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
        version: VersionFrom::Latest,
        source: Source::Zip("https://awscli.amazonaws.com/awscli-exe-linux-{arch2}.zip"),
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
        version: VersionFrom::Latest,
        source: Source::TarGz("https://dl.google.com/dl/cloudsdk/channels/rapid/downloads/google-cloud-cli-linux-{arch3}.tar.gz"),
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
        version: VersionFrom::Redirect("https://github.com/esp-rs/espflash/releases/latest"),
        source: Source::Zip("https://github.com/esp-rs/espflash/releases/download/{v}/espflash-{arch2}-unknown-linux-gnu.zip"),
        checksum: None,
        bins: &[("espflash", "espflash")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "The project publishes no checksum for this download (it is over HTTPS from github.com). It only talks to the chip over USB; it needs permission to use the serial device.",
    },
    CliDef {
        id: "azure-cli",
        name: "Azure CLI",
        about: "Microsoft Azure from the command line (the `az` command).",
        home: "https://learn.microsoft.com/cli/azure/",
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
        id: "ollama",
        name: "Ollama",
        about: "Run language models on this machine's own processor or graphics card, and use them from the chat.",
        home: "https://ollama.com",
        version: VersionFrom::Redirect("https://github.com/ollama/ollama/releases/latest"),
        source: Source::TarZst("https://github.com/ollama/ollama/releases/download/{v}/ollama-linux-{arch}.tar.zst"),
        checksum: Some(Checksum::Lines("https://github.com/ollama/ollama/releases/download/{v}/sha256sum.txt")),
        bins: &[("ollama", "bin/ollama")],
        hosts: &["github.com", "githubusercontent.com"],
        net_host: Some("-"),
        version_args: &["--version"],
        note: "A download of over a gigabyte (it carries the graphics libraries) that unpacks to several. Start the server from here, then pull models from the chat or Models; they are kept in the data folder. A machine without enough memory or a graphics card will be slow.",
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
}

fn arch() -> Result<(&'static str, &'static str, &'static str), String> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(("amd64", "x86_64", "x86_64")),
        "aarch64" => Ok(("arm64", "aarch64", "arm")),
        a => Err(format!("these tools are not offered for this machine's processor ({a})")),
    }
}

fn host_ok(url: &str, hosts: &[&str]) -> Result<(), String> {
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

fn template(t: &str, v: &str) -> Result<String, String> {
    let (a, a2, a3) = arch()?;
    Ok(t.replace("{ve}", &pct_encode_version(v)).replace("{vn}", v.trim_start_matches('v')).replace("{v}", v).replace("{arduino}", if a == "arm64" { "Linux_ARM64" } else { "Linux_64bit" }).replace("{arch2}", a2).replace("{arch3}", a3).replace("{arch}", a).replace("{k3s_suffix}", if a == "arm64" { "-arm64" } else { "" }))
}

fn extract_hash(text: &str, file: &str) -> Option<String> {
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
        CliManager { root: root.into(), streamer: Mutex::new(None), jobs: Mutex::new(HashMap::new()), state_lock: Mutex::new(()), python: Mutex::new("python3".into()) }
    }

    pub fn set_streamer(&self, s: Arc<dyn Streamer>) {
        *self.streamer.lock().unwrap() = Some(s);
    }

    /// Which Python makes the environment for Python-packaged tools (tests use a stand-in).
    pub fn set_python(&self, p: &str) {
        *self.python.lock().unwrap() = p.into();
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
        if let Some(j) = self.jobs.lock().unwrap().get_mut(id) {
            f(j);
        }
    }

    /// Starts installing `id` (or reinstalling the latest version) and returns at once; `job` follows it.
    pub fn install(self: &Arc<Self>, id: &str, by: &str) -> Result<(), String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list: {}", CATALOG.iter().map(|d| d.id).collect::<Vec<_>>().join(", ")))?;
        arch()?;
        let streamer = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up for downloads")?;
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
            me.set_job(d.id, |j| match r {
                Ok(()) => {
                    j.finished = true;
                    j.phase = "installed".into();
                }
                Err(e) => j.error = Some(e),
            });
        }).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn open_follow(&self, st: &dyn Streamer, url: &str, hosts: &[&str]) -> Result<(iyakki_proxy::StreamHead, String), String> {
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

    fn small(&self, st: &dyn Streamer, url: &str, hosts: &[&str]) -> Result<String, String> {
        let (head, _) = self.open_follow(st, url, hosts)?;
        let mut s = String::new();
        head.body.take(256 * 1024).read_to_string(&mut s).map_err(|e| e.to_string())?;
        Ok(s)
    }

    fn latest_version(&self, d: &CliDef, st: &dyn Streamer) -> Result<String, String> {
        match d.version {
            VersionFrom::Text(u) => {
                let v = self.small(st, u, d.hosts)?.trim().to_string();
                if v.is_empty() || v.len() > 40 || v.contains(char::is_whitespace) {
                    return Err("the version answer was not understood".into());
                }
                Ok(v)
            }
            VersionFrom::Json(u, ptr) => {
                let t = self.small(st, u, d.hosts)?;
                let j: serde_json::Value = serde_json::from_str(&t).map_err(|_| "the version answer was not understood".to_string())?;
                j.pointer(ptr).and_then(|v| v.as_str()).map(str::to_string).filter(|v| !v.is_empty() && v.len() < 40).ok_or_else(|| "the version answer was not understood".into())
            }
            VersionFrom::Redirect(u) => {
                host_ok(u, d.hosts)?;
                let head = st.open(u, &[]).map_err(|e| format!("could not reach {u}: {e}"))?;
                let loc = head.header("location").ok_or("the latest-release link did not name a version")?;
                version_from_location(loc).ok_or_else(|| "the latest version could not be read from the release link".into())
            }
            VersionFrom::Latest => Ok("latest".into()),
        }
    }

    fn run_install(&self, d: &'static CliDef, st: &dyn Streamer, by: &str) -> Result<(), String> {
        let fail = |e: String| -> String { e };
        self.set_job(d.id, |j| j.phase = "finding the latest version".into());
        let version = self.latest_version(d, st).map_err(fail)?;
        let tmp = self.root.join("tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let pkg = self.pkg_dir(d.id);
        let target = pkg.join(version.replace('/', "_"));
        let mut verified = false;
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let result: Result<(), String> = (|| {
            match d.source {
                Source::Pip(pkgname) => {
                    self.set_job(d.id, |j| j.phase = format!("installing {pkgname} with pip (a few minutes)"));
                    let py = self.python.lock().unwrap().clone();
                    run_quiet(&py, &["-m", "venv", &target.join("venv").display().to_string()], 120)?;
                    run_quiet(&target.join("venv/bin/pip").display().to_string(), &["install", "--no-input", "--disable-pip-version-check", pkgname], 1800)?;
                }
                Source::File(u) | Source::TarGz(u) | Source::Zip(u) | Source::TarZst(u) => {
                    let url = template(u, &version)?;
                    let file = url.rsplit('/').next().unwrap_or("download").to_string();
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
                        let cu = template(match c { Checksum::Single(u) | Checksum::Lines(u) => u }, &version)?;
                        let text = self.small(st, &cu, d.hosts)?;
                        let want = extract_hash(&text, &file).ok_or("the vendor's checksum file has no entry for this download")?;
                        if want != got_hash {
                            return Err(format!("the download does not match the vendor's checksum (expected {}…, got {}…); nothing was installed", &want[..12], &got_hash[..12]));
                        }
                        verified = true;
                    }
                    self.set_job(d.id, |j| j.phase = "unpacking".into());
                    let _ = final_url;
                    match d.source {
                        Source::File(_) => {
                            let dst = target.join(d.bins[0].1);
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
                        Source::Pip(_) => {}
                    }
                    let _ = std::fs::remove_file(&dl);
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&target);
            let _ = std::fs::remove_file(tmp.join(format!("{}.download", d.id)));
            return Err(e);
        }
        // link the programs
        self.set_job(d.id, |j| j.phase = "linking".into());
        let bindir = self.bin_dir();
        std::fs::create_dir_all(&bindir).map_err(|e| e.to_string())?;
        let mut linked = Vec::new();
        for (idx, (name, rel)) in d.bins.iter().enumerate() {
            let src = match find_in(&target, rel) {
                Some(s) => s,
                None => {
                    // only the first program has to be there; the others come and go between versions
                    if idx > 0 {
                        continue;
                    }
                    let _ = std::fs::remove_dir_all(&target);
                    return Err(format!("the unpacked download has no `{rel}`"));
                }
            };
            let link = bindir.join(name);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&src, &link).map_err(|e| e.to_string())?;
            linked.push(name.to_string());
        }
        // the program says what it is
        let first = d.bins[0].0;
        let reported = run_quiet(&bindir.join(first).display().to_string(), d.version_args, 60).map(|o| pick_version_line(&o, d.id)).unwrap_or_default();
        let shown = if version == "latest" { reported.split(|c: char| c.is_whitespace() || c == '/').find(|w| w.trim_start_matches('v').chars().next().is_some_and(|c| c.is_ascii_digit())).unwrap_or("latest").to_string() } else { version.clone() };
        // remove other versions of this tool
        if let Ok(rd) = std::fs::read_dir(&pkg) {
            for e in rd.flatten() {
                if e.path() != target {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let _g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.load();
        all.retain(|i| i.id != d.id);
        all.push(Installed { id: d.id.into(), version: shown, reported, at: crate::clock::now_unix(), bins: linked, verified, by: by.into() });
        self.store(&all)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let d = def(id).ok_or_else(|| format!("`{id}` is not in the list"))?;
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
fn run_quiet(program: &str, args: &[&str], seconds: u64) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for k in ["PATH", "HOME", "LANG", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed().as_secs() >= seconds => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} did not finish within {seconds} seconds"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(30)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (mut out, mut err) = (String::new(), String::new());
    while let Ok((is_out, s)) = rx.recv_timeout(std::time::Duration::from_millis(400)) {
        if is_out { out = s } else { err = s }
    }
    if status.success() { Ok(if out.trim().is_empty() { err } else { out }) } else { Err(format!("{} failed: {}", program.rsplit('/').next().unwrap_or(program), err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no message"))) }
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
        for want in ["kubectl", "k3s", "helm", "terraform", "gh", "aws-cli", "gcloud", "azure-cli", "arduino-cli", "esptool", "espflash"] {
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
            let urls: Vec<&str> = match d.source { Source::File(u) | Source::TarGz(u) | Source::Zip(u) | Source::TarZst(u) => vec![u], Source::Pip(_) => vec![] };
            for u in urls.into_iter().chain(match d.checksum { Some(Checksum::Single(u)) | Some(Checksum::Lines(u)) => Some(u), None => None }) {
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
        let st = fake(vec![(format!("https://awscli.amazonaws.com/awscli-exe-linux-{a2}.zip"), ok(&zip))]);
        let m = manager("aws", st);
        m.install("aws-cli", "root").unwrap();
        assert!(wait(&m, "aws-cli").error.is_none());
        let i = m.is_installed("aws-cli").unwrap();
        assert!(!i.verified, "no checksum was available");
        assert_eq!(i.version, "2.17.0", "{i:?}");
        assert_eq!(m.extras().iter().find(|x| x.name == "aws").unwrap().host.as_deref(), Some("amazonaws.com"));
    }

    #[test]
    fn the_arduino_and_esp_tools_install_with_optional_extra_programs_and_need_no_network_to_run() {
        let (arch, a2, _) = arch().unwrap();
        let ard = if arch == "arm64" { "Linux_ARM64" } else { "Linux_64bit" };
        // arduino-cli: a tar.gz whose program is at the top, with a checksum file named like the release's
        let tar = gzip(&make_tar(&[("arduino-cli", b'0', "#!/bin/sh\necho 'arduino-cli  Version: 1.5.1 Commit: x'\n", 0o755), ("LICENSE.txt", b'0', "gpl", 0o644)]));
        let name = format!("arduino-cli_1.5.1_{ard}.tar.gz");
        // espflash: a zip holding one program, no checksum
        let zip = make_zip(&[("espflash", "#!/bin/sh\necho 'espflash 4.6.0'\n", 0o100755, true)]);
        let st = fake(vec![
            ("https://github.com/arduino/arduino-cli/releases/latest".into(), (302, vec![("location".into(), "https://github.com/arduino/arduino-cli/releases/tag/v1.5.1".into())], vec![])),
            (format!("https://github.com/arduino/arduino-cli/releases/download/v1.5.1/{name}"), ok(&tar)),
            ("https://github.com/arduino/arduino-cli/releases/download/v1.5.1/1.5.1-checksums.txt".into(), ok(format!("{}  {name}\n", sha(&tar)).as_bytes())),
            ("https://github.com/esp-rs/espflash/releases/latest".into(), (302, vec![("location".into(), "https://github.com/esp-rs/espflash/releases/tag/v4.6.0".into())], vec![])),
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
        assert!(!m.is_installed("espflash").unwrap().verified, "no checksum is published for it");
        // esptool from pip: only its first program is required; the others may be missing
        let d = tmp("esptoolpy");
        let py = d.join("python3");
        std::fs::write(&py, "#!/bin/sh\nenv=\"$3\"\nmkdir -p \"$env/bin\"\ncat > \"$env/bin/pip\" <<'EOF'\n#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '#!/bin/sh\\necho \"esptool v5.5.0\"\\n' > \"$d/esptool\"\nchmod +x \"$d/esptool\"\nEOF\nchmod +x \"$env/bin/pip\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&py, std::fs::Permissions::from_mode(0o755)).unwrap();
        m.set_python(&py.display().to_string());
        m.install("esptool", "root").unwrap();
        let j = wait(&m, "esptool");
        assert!(j.error.is_none(), "{j:?}");
        let e = m.is_installed("esptool").unwrap();
        assert_eq!((e.bins.as_slice(), e.version.as_str()), (&["esptool".to_string()][..], "v5.5.0"), "{e:?}");
        // what the chat may run, and which network check each gets: the chip tools need none
        let mut x = m.extras();
        x.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(x, vec![Extra { name: "arduino-cli".into(), host: None }, Extra { name: "espflash".into(), host: Some("-".into()) }, Extra { name: "esptool".into(), host: Some("-".into()) }]);
        let out = crate::exec::run_with(&m.extras(), "espflash", &["board-info".to_string()], &std::env::temp_dir(), None, 10, Some(&m.bin_dir())).unwrap();
        assert!(out.contains("espflash 4.6.0"), "{out}");
    }

    #[test]
    fn a_python_package_tool_is_installed_into_its_own_environment_and_failures_are_reported() {
        let d = tmp("pip");
        let fake_py = d.join("python3");
        // a stand-in python that makes the environment and a pip that "installs" az
        std::fs::write(&fake_py, "#!/bin/sh\nenv=\"$3\"\nmkdir -p \"$env/bin\"\ncat > \"$env/bin/pip\" <<'EOF'\n#!/bin/sh\nd=\"$(dirname \"$0\")\"\nprintf '#!/bin/sh\\necho \"azure-cli                         2.64.0\"\\necho \"core 2.64.0\"\\n' > \"$d/az\"\nchmod +x \"$d/az\"\nEOF\nchmod +x \"$env/bin/pip\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_py, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Arc::new(CliManager::new(&d.join("cli")));
        m.set_streamer(fake(vec![]));
        m.set_python(&fake_py.display().to_string());
        m.install("azure-cli", "root").unwrap();
        let j = wait(&m, "azure-cli");
        assert!(j.error.is_none(), "{j:?}");
        let i = m.is_installed("azure-cli").unwrap();
        assert!(i.reported.starts_with("azure-cli") && i.version == "2.64.0", "{i:?}");
        assert!(m.bin_dir().join("az").exists());
        // no python: a clear message and nothing left behind
        let m2 = Arc::new(CliManager::new(&d.join("cli2")));
        m2.set_streamer(fake(vec![]));
        m2.set_python("/nonexistent/python3");
        m2.install("azure-cli", "root").unwrap();
        let j = wait(&m2, "azure-cli");
        assert!(j.error.unwrap().contains("could not start"));
        assert!(m2.installed().is_empty());
        // no network set up, and unknown names
        let m3 = Arc::new(CliManager::new(&d.join("cli3")));
        assert!(m3.install("kubectl", "root").unwrap_err().contains("no network connection"));
        assert!(m3.install("nothing", "root").unwrap_err().contains("not in the list"));
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
}
