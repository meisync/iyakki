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
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Runtimes, Group::Cluster, Group::AiCoding, Group::Voice, Group::LocalModels, Group::Media];

    pub fn title(self) -> &'static str {
        match self {
            Group::Runtimes => "Runtimes",
            Group::Cluster => "Cluster",
            Group::AiCoding => "AI coding",
            Group::Voice => "Voice",
            Group::LocalModels => "Local models",
            Group::Media => "Media",
        }
    }

    /// What the section says about choosing (the pairs are not exclusive: they run side by side).
    pub fn note(self) -> &'static str {
        match self {
            Group::Runtimes => "Node.js and Python 3: the coding agents and Python tools need them, and they are installed first when missing",
            Group::Cluster => "k3s is the base; DuckDB, Elasticsearch, Lemonade, Indic-TTS and the local image registry install it first when it is missing",
            Group::AiCoding => "coding agents for the terminal and the AI IDE (Node.js is installed first when it is missing); pick one or both",
            Group::Voice => "reading voices; pick one or both (Kokoro: English, through Lemonade, which it installs first; Indic-TTS: Indian languages, in k3s)",
            Group::LocalModels => "model servers on this machine, added to Models when they answer; pick one or both (Lemonade runs in k3s)",
            Group::Media => "the player for video and audio files (MEDIA tile, IYAKKI menu \u{2192} Media player); it needs nothing else",
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
    Lemonade,
    Ollama,
    Ffmpeg,
}

impl Item {
    pub const ALL: [Item; 13] = [Item::Node, Item::Python, Item::K3s, Item::DuckDb, Item::Elasticsearch, Item::Registry, Item::ClaudeCode, Item::Copilot, Item::Kokoro, Item::IndicTts, Item::Lemonade, Item::Ollama, Item::Ffmpeg];

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
            Item::Lemonade => "lemonade",
            Item::Ollama => "ollama",
            Item::Ffmpeg => "ffmpeg",
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
            Item::Lemonade => "Lemonade",
            Item::Ollama => "Ollama",
            Item::Ffmpeg => "Media player (ffmpeg)",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Item::Node | Item::Python => Group::Runtimes,
            Item::K3s | Item::DuckDb | Item::Elasticsearch | Item::Registry => Group::Cluster,
            Item::ClaudeCode | Item::Copilot => Group::AiCoding,
            Item::Kokoro | Item::IndicTts => Group::Voice,
            Item::Lemonade | Item::Ollama => Group::LocalModels,
            Item::Ffmpeg => Group::Media,
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
            Item::IndicTts => "Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi voices: the Indic voice server in k3s",
            Item::Lemonade => "Local model server (chat, embeddings, images, whisper, Kokoro): one Deployment in k3s (namespace iyakki-ai), reachable on this machine only (127.0.0.1:13305) and added to Models",
            Item::Ollama => "Run language models on this machine; started and added to Models as ollama-local",
            Item::Ffmpeg => "Plays video and audio in IYAKKI (MEDIA tile, camera recordings, Media player). Downloads a static ffmpeg build from its builder (johnvansickle.com), GPLv3, checked with its checksum; about 42 MB",
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
            Item::IndicTts => "code: MIT (AI4Bharat Indic-TTS); model weights: NO licence stated by AI4Bharat. Whether IYAKKI may use them is an open owner decision; nothing is bundled, you supply the model files",
            Item::Lemonade => "Apache-2.0; each model you pull has its own licence",
            Item::Ollama => "MIT; each model you pull has its own licence",
            Item::Ffmpeg => "GPLv3 (the static build you download from its builder; IYAKKI does not ship it). H.264 and AAC are patent-licensed formats: decoding them is your choice, as in the browser",
        }
    }

    /// What has to be there first.
    pub fn needs(self) -> &'static [Item] {
        match self {
            Item::DuckDb | Item::Elasticsearch | Item::IndicTts | Item::Lemonade | Item::Registry => &[Item::K3s],
            Item::Kokoro => &[Item::Lemonade],
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

    /// Installed by a detached job. Indic-TTS is not: it is deployed through the Platform services review (Settings \u{2192} Dev
    /// cluster), which plans, shows and asks before it builds the image and applies the objects.
    pub fn runs_as_job(self) -> bool {
        self != Item::IndicTts
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
    let data = svc.dir();
    match item {
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
}

pub fn gather_facts(svc: &crate::services::Services) -> Facts {
    let mut f = Facts::default();
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
    out
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
    /// The registry step was given a cluster of its own (a chosen cluster, not the managed k3s): it does not wait for k3s.
    fn registry_on_other_cluster(&mut self) -> bool {
        false
    }
    /// Make `kubectl` and `k3s` commands in the tool folder (the k3s program answers to both names), so terminals and agents can use
    /// them; returns a line to say, empty when there is nothing to say.
    fn link_k3s_tools(&mut self) -> Result<String, String> {
        Ok(String::new())
    }
    /// Create the local image registry in the cluster (`localreg.rs`), saying each step through [`StepEnv::say`]; returns the address
    /// the pods pull it at. Idempotent: a registry that is ready is only reported.
    fn create_registry(&mut self) -> Result<String, String> {
        Err("creating the registry is not available here".into())
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
fn wait_for_cluster(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
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
            // when Lemonade's own step has not finished (or failed), this step deploys it too; a failure stops this one at once
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
            env.say("[1/3] Lemonade answers; pulling kokoro-v1 (about 330 MB)");
            env.lemonade_pull("kokoro-v1")?;
            env.mark(Item::Kokoro, "kokoro-v1 in Lemonade")?;
            env.say("[2/3] kokoro-v1 is downloaded");
            env.say("[3/3] done: Settings \u{2192} Models \u{2192} lemonade-local \u{2192} kokoro-v1 \u{2192} Use as reading voice");
            Ok(())
        }
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
        RealEnv { data: data.to_path_buf(), user: user.into(), k3s, clis, streamer, registry: RegistryArgs::default() }
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

impl StepEnv for RealEnv {
    fn link_k3s_tools(&mut self) -> Result<String, String> {
        let l = link_k3s_tools(&self.clis.bin_dir(), &self.k3s.layout().bin())?;
        Ok(if l.is_empty() { String::new() } else { format!("linked {} in {} (terminals and agents can run kubectl; KUBECONFIG points at the dev cluster)", l.join(" and "), self.clis.bin_dir().display()) })
    }
    fn registry_on_other_cluster(&mut self) -> bool {
        self.registry.target.is_some()
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
    let Some(item) = args.first().and_then(|a| Item::parse(a)) else {
        eprintln!("usage: iyakki quick-install <k3s|duckdb|elasticsearch> [--data DIR] [--user NAME]");
        return 2;
    };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let user = flag("--user").or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "system".into());
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
    match run_step(item, &mut env, WAIT_FOR_CLUSTER, Duration::from_secs(5)) {
        Ok(()) => 0,
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
