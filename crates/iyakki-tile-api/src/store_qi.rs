//! The STORE facade, group 1 (D-Q4-01): the quick-install item, its facts and its job state as plain mirrors. The shared module
//! `iyakki_shared::quickinstall` has the real types and every word the menu shows; the shell converts between them (`apps/store_port.rs`).
//! The words are not copied: [`ItemInfo`] carries what the real item answered, so the tile asks the port once and holds the table.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

macro_rules! items {
    ($($v:ident),* $(,)?) => {
        /// What the Setup page installs (same variants, same order as the real item).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Item {
            $($v),*
        }
        impl Item {
            pub const ALL: [Item; 39] = [$(Item::$v),*];
        }
    };
}

items!(
    Node, Python, K3s, DuckDb, Elasticsearch, OpenSearch, Prometheus, ClickHouse, GraphDb, Gitea, Matrix, SeaweedFs, RustFs, Registry, ClaudeCode, Copilot, Kokoro,
    VoicePack, Lemonade, Ollama, MacHost, GpuVulkan, GpuNvidia, GpuRocm, Ffmpeg, FfmpegLibs, MacFirmware, MacCamera, LinuxApps, LinuxWindows, FwQualcommBt,
    FwRealtekNet, FwBroadcomNet, FwAtheros9170, FwMediatek, FwHardware, DataScience, YtDlp, BrowserPackage,
);

/// The sections of the Setup page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Runtimes,
    Cluster,
    AiCoding,
    Voice,
    LocalModels,
    Media,
    Linux,
    Mac,
    Firmware,
}

impl Group {
    pub const ALL: [Group; 9] = [Group::Runtimes, Group::Cluster, Group::AiCoding, Group::Voice, Group::LocalModels, Group::Media, Group::Linux, Group::Mac, Group::Firmware];
}

/// What the real item answers about itself (its words and rules), asked once per item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInfo {
    pub item: Item,
    /// The word after `quick-install` of its job and the name of its marker files.
    pub id: String,
    pub title: String,
    pub about: String,
    pub licence: String,
    pub group: Group,
    /// The items it needs first (not the ones already there: the port asks `needs_now`).
    pub needs: Vec<Item>,
    pub download_mb: u32,
    pub time_text: String,
    pub min_ram_mb: u64,
    pub is_service: bool,
    pub is_hardware: bool,
    pub runs_as_job: bool,
    pub npm_tool: bool,
    pub pip_tool: bool,
    /// The id of its entry in the CLI catalogue, when it has one.
    pub cli_id: Option<String>,
    /// `needs` plus the runtime the CLI catalogue says the tool installs with (what the install order uses).
    pub needs_now: Vec<Item>,
    /// The restricted firmware pack of this item, when it is one.
    pub fw: Option<FwDef>,
    /// What the item's pod asks for and may use, as the popup's label and text lines (empty: no pod of the table).
    pub res_lines: Vec<(String, String)>,
}

/// The words of a restricted firmware pack (the real definition also holds the device ids and the module list; those stay in the port).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwDef {
    pub pack_id: String,
    pub title: String,
    pub vendor: String,
    pub licence_note: String,
    pub licence_id: String,
    pub without: String,
    pub about: String,
}

/// The plain result of a firmware run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FwOutcome {
    pub at: i64,
    pub installed: bool,
    pub files: usize,
    pub source: String,
    pub reboot_needed: bool,
    pub headline: String,
    pub lines: Vec<String>,
}

impl FwOutcome {
    /// A restart is still wanted: the run said so and the machine has not started since (`uptime` in seconds).
    pub fn reboot_pending(&self, now: i64, uptime: i64) -> bool {
        self.installed && self.reboot_needed && self.at > now - uptime
    }
}

static TABLE: OnceLock<Vec<ItemInfo>> = OnceLock::new();

/// Give the tile the table of what the real items answer. The shell calls it once, before the first screen; the first call wins and
/// later calls (every test builds its own app) change nothing, because the table is the same for every caller.
pub fn set_table(rows: Vec<ItemInfo>) {
    let _ = TABLE.set(rows);
}

impl Item {
    /// The table row of this item. Panics when the shell did not give the table (a programming error, caught by every test).
    pub fn info(self) -> &'static ItemInfo {
        let t = TABLE.get().expect("store_qi::set_table was not called");
        &t[Item::ALL.iter().position(|i| *i == self).expect("every item is in ALL")]
    }
    pub fn id(self) -> &'static str {
        &self.info().id
    }
    pub fn parse(s: &str) -> Option<Item> {
        Item::ALL.into_iter().find(|i| i.id() == s)
    }
    pub fn title(self) -> &'static str {
        &self.info().title
    }
    pub fn about(self) -> &'static str {
        &self.info().about
    }
    pub fn licence(self) -> &'static str {
        &self.info().licence
    }
    pub fn group(self) -> Group {
        self.info().group
    }
    pub fn needs(self) -> &'static [Item] {
        &self.info().needs
    }
    pub fn needs_now(self) -> Vec<Item> {
        self.info().needs_now.clone()
    }
    pub fn download_mb(self) -> u32 {
        self.info().download_mb
    }
    pub fn time_text(self) -> &'static str {
        &self.info().time_text
    }
    pub fn min_ram_mb(self) -> u64 {
        self.info().min_ram_mb
    }
    pub fn is_service(self) -> bool {
        self.info().is_service
    }
    pub fn is_hardware(self) -> bool {
        self.info().is_hardware
    }
    pub fn runs_as_job(self) -> bool {
        self.info().runs_as_job
    }
    pub fn npm_tool(self) -> bool {
        self.info().npm_tool
    }
    pub fn pip_tool(self) -> bool {
        self.info().pip_tool
    }
    pub fn cli_id(self) -> Option<&'static str> {
        self.info().cli_id.as_deref()
    }
    pub fn fw_def(self) -> Option<&'static FwDef> {
        self.info().fw.as_ref()
    }
}

/// The items of a section, in the menu's order.
pub fn items_of(g: Group) -> Vec<Item> {
    Item::ALL.into_iter().filter(|i| i.group() == g).collect()
}

/// "1. k3s \u{2192} 2. Elasticsearch".
pub fn titles_text(titles: &[String]) -> String {
    titles.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("  \u{2192}  ")
}

pub fn order_text(steps: &[Item]) -> String {
    titles_text(&steps.iter().map(|s| s.title().to_string()).collect::<Vec<_>>())
}

/// The steps for installing `target`, in dependency order (what it needs and is not there yet, then `target` itself).
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

/// The words that start a presence detail when the newest attempt failed after an earlier install.
pub const LAST_FAILED: &str = "the last attempt failed: ";

/// The jobs workspace of installer jobs.
pub const WORKSPACE: &str = "iyakki-install";

/// The reason of a failed last attempt, when the presence carries it.
pub fn failure_reason(p: &Presence) -> Option<&str> {
    match p {
        Presence::Earlier(d) => d.strip_prefix(LAST_FAILED),
        _ => None,
    }
}

/// Which item a job's command installs.
pub fn item_of_command(command: &str) -> Option<Item> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let i = words.iter().position(|w| *w == "quick-install")?;
    words.get(i + 1).and_then(|w| Item::parse(w))
}

pub fn item_of(job: &Job) -> Option<Item> {
    item_of_command(&job.command)
}

/// The items whose newest install job did not succeed (failed, stopped or lost).
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
        if j.state != JobState::Succeeded {
            failed.push(i);
        }
    }
    failed
}

/// Where an item stands on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    Missing,
    Installed(String),
    Earlier(String),
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
            Presence::Installed(d) => format!("installed \u{b7} {d}"),
            Presence::Earlier(d) if d.starts_with(LAST_FAILED) => format!("installed earlier; {d}"),
            Presence::Earlier(d) if d.is_empty() => "installed earlier, not running".into(),
            Presence::Earlier(d) => format!("installed earlier, not running \u{b7} {d}"),
            Presence::Elsewhere(d) => format!("present (not IYAKKI's) \u{b7} {d}"),
        }
    }
}

/// Whether a newer version exists for an installed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Latest {
    Newer(String),
    Current(String),
    Unknown(String),
    NotApplicable,
}

/// How a job stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Succeeded,
    /// Ended with this exit code.
    Failed(i32),
    Stopped,
    Lost,
}

/// An installer job as the Setup page sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub owner: String,
    pub workspace: String,
    pub pid: u32,
    pub state: JobState,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub progress: Option<f32>,
    pub last_line: String,
    pub last_active_ms: u64,
    pub dismissed: bool,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == JobState::Running
    }
    pub fn quiet_secs(&self, now_ms: u64) -> u64 {
        if !self.running() || self.last_active_ms == 0 {
            return 0;
        }
        now_ms.saturating_sub(self.last_active_ms) / 1000
    }
    pub fn elapsed_secs(&self, now_ms: u64) -> u64 {
        let end = if self.ended_ms > 0 { self.ended_ms } else { now_ms };
        end.saturating_sub(self.started_ms) / 1000
    }
}

impl JobState {
    pub fn finished(&self) -> bool {
        *self != JobState::Running
    }
}

/// How a step's job is started: the program and the data folder it works on.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// How the hardware-firmware job gets its files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HwFirmwareOpts {
    pub from: Option<PathBuf>,
    pub with: Vec<String>,
    pub link: Option<String>,
}

/// What the Setup page needs to know about this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub presence: Vec<(Item, Presence)>,
    pub unhealthy: Vec<(Item, String)>,
    /// The runtimes installed before the item (their ids).
    pub runtimes: Vec<(Item, Vec<String>)>,
    pub used_by: Vec<(Item, String)>,
    pub hidden: Vec<Item>,
    pub healthy: Vec<Item>,
    pub waiting: Vec<Item>,
    pub failures: Vec<(Item, String)>,
}

// ---- Moving an Ollama from the host into the cluster (mirrors of `quickinstall::ollama_cluster`; the decision and the words stay in the port)

pub mod ollama_cluster {
    use std::path::PathBuf;

    pub const CLUSTER_WORD: &str = "in the cluster";
    pub const HOST_WORD: &str = "Ollama runs on this machine's host";
    pub const MOVE_TITLE: &str = "Move Ollama into the cluster?";
    pub const FRESH_TITLE: &str = "Start Ollama fresh in the cluster?";

    pub fn is_cluster_detail(d: &str) -> bool {
        d.contains(CLUSTER_WORD)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mode {
        Move,
        Fresh,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct HostInfo {
        pub models_dir: PathBuf,
        pub models_bytes: u64,
        pub free_bytes: Option<u64>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Decision {
        Copy,
        TooLarge,
        NoRoom,
    }
}

// ---- Removing an item (mirrors of `quickinstall::uninstall`; the real plan keeps its steps, the tile gets both reviews)

/// A removal as the card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct UnReview {
    pub title: String,
    pub goes: Vec<String>,
    pub stays: Vec<String>,
    pub data: Vec<String>,
    pub data_available: bool,
    pub data_required: bool,
    pub space: Option<String>,
    pub space_if_ticked: String,
    pub notes: Vec<String>,
}

/// What removing an item does. The review depends only on the data tick, so the port hands both.
#[derive(Clone, Debug, PartialEq)]
pub struct UnPlan {
    pub item: Item,
    pub data_requires: bool,
    pub space_text: String,
    /// The review with the data kept.
    pub review_kept: UnReview,
    /// The review with the data deleted too.
    pub review_deleted: UnReview,
}

impl UnPlan {
    pub fn review(&self, delete_data: bool) -> &UnReview {
        if delete_data {
            &self.review_deleted
        } else {
            &self.review_kept
        }
    }
    pub fn space_text(&self) -> String {
        self.space_text.clone()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnOpts {
    pub delete_data: bool,
    pub with_dependants: bool,
}

/// The quick-install services of the person (the installer jobs, the facts, the removal), in mirror types. The shell implements it over
/// the agent's facet (`apps/store_port.rs`); a test of the tile gives a fake. `Send + Sync`: the screen hands it to worker threads.
pub trait QuickInstall: Send + Sync {
    fn gather_facts(&self) -> Facts;
    fn install_jobs(&self, owner: &str) -> Vec<Job>;
    fn check_upgrades(&self) -> Vec<(Item, Latest)>;
    /// Start the item and what it needs that `present` says is missing.
    fn start(&self, owner: &str, target: Item, present: &dyn Fn(Item) -> bool, launch: &Launch) -> Result<Vec<Job>, String>;
    fn start_firmware_pack(&self, owner: &str, item: Item, from: Option<&Path>, launch: &Launch) -> Result<Job, String>;
    fn start_camera_firmware(&self, owner: &str, launch: &Launch) -> Result<Job, String>;
    fn start_hardware_firmware(&self, owner: &str, opts: &HwFirmwareOpts, launch: &Launch) -> Result<Job, String>;
    /// The plan to remove an item, and the warning about the CLI catalogue's runtimes, if any.
    fn uninstall_plan(&self, item: Item, user: &str) -> (UnPlan, Option<String>);
    fn uninstall(&self, item: Item, user: &str, opts: UnOpts, progress: Option<Arc<Mutex<String>>>) -> Result<String, String>;
    /// Installed items that need `item`, and the plain warnings about the ones that only use it.
    fn dependency_lines(&self, item: Item, installed: &dyn Fn(Item) -> bool) -> (Vec<Item>, Vec<String>);
    /// The titles of the steps, with the runtimes the CLI catalogue installs before each (`before` gives their ids).
    fn order_titles_with(&self, steps: &[Item], before: &dyn Fn(Item) -> Vec<String>) -> Vec<String>;
    /// The memory warning for a plan (`None` when there is nothing to say).
    fn total_warning(&self, steps: &[Item], installed: &[Item], ram_mb: u64) -> Option<String>;
    fn ollama_host_info(&self, data: &Path) -> ollama_cluster::HostInfo;
    fn ollama_decide(&self, h: &ollama_cluster::HostInfo) -> ollama_cluster::Decision;
    fn ollama_review_lines(&self, mode: ollama_cluster::Mode, h: &ollama_cluster::HostInfo, d: ollama_cluster::Decision) -> Vec<String>;
    fn ollama_write_mode(&self, data: &Path, mode: ollama_cluster::Mode) -> Result<(), String>;
    // ---- restricted firmware packs
    /// The hardware of the pack is on this machine (`sys_root` is the host's /sys).
    fn fw_present(&self, sys_root: &Path, item: Item) -> bool;
    fn fw_installed(&self, data: &Path, item: Item) -> bool;
    fn fw_outcome(&self, data: &Path, item: Item) -> Option<FwOutcome>;
    /// The person accepted the pack's licence: record it.
    fn fw_accept(&self, data: &Path, item: Item, by: &str, now: i64) -> Result<(), String>;
    /// The accepted licence is on record for the current pin.
    fn fw_accepted(&self, data: &Path, item: Item) -> bool;
    /// The digest of the licence texts and the linux-firmware commit of the pin (for the audit line).
    fn fw_licence_digest(&self, item: Item) -> String;
    fn fw_commit(&self) -> String;
    // ---- the hardware firmware item (`fwextra`)
    fn hw_installed(&self, data: &Path) -> bool;
    fn hw_outcome(&self, data: &Path) -> Option<FwOutcome>;
    fn hw_remove(&self, data: &Path) -> Result<(), String>;
    /// The linux-firmware tag the hardware firmware is pinned to.
    fn hw_tag(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_item_list_is_complete_and_in_order() {
        assert_eq!(Item::ALL.len(), 39);
        let mut sorted = Item::ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, Item::ALL.to_vec(), "ALL is in variant order");
        sorted.dedup();
        assert_eq!(sorted.len(), 39);
    }

    #[test]
    fn a_running_job_runs_and_a_finished_one_does_not() {
        let mut j = Job {
            id: "j1".into(),
            label: "l".into(),
            command: "c".into(),
            cwd: String::new(),
            owner: "u".into(),
            workspace: "w".into(),
            pid: 1,
            state: JobState::Running,
            started_ms: 0,
            ended_ms: 0,
            progress: None,
            last_line: String::new(),
            last_active_ms: 0,
            dismissed: false,
        };
        assert!(j.running());
        j.state = JobState::Failed(3);
        assert!(!j.running());
    }
}
