//! The store domain as narrow, hand-written facets (F-SPLIT-03, UI-BOUNDARY.md): what the UI asks of the store and the update check, without
//! `Services`, without file paths and without the agent's types. Every method has a default that answers "nothing" or "unavailable", so a core
//! (or a test) that serves only one facet compiles and behaves.
//!
//! * [`StoreHub`]: the tile apps of the launcher (listing, the unsigned dev apps of developer mode, the WASM switch).
//! * [`StoreUpdates`]: the periodic update check (its last answer, whether one is due, running one) and the setup wizards an install queued.
//! * [`StoreCatalog`]: small questions about the store's own files (the private store card, the words of a permission, the stores on sticks, the
//!   start of a new project).
//! * [`StoreDrivers`]: the drivers of this machine (what the machine is, what is on offer, what is installed, a pack made from a folder or a file).
//! * [`StoreComponents`]: the system components of the pinned private stores (channel, last check, review, the approved install).
//!
//! The data types are the pure ones in `iyakki-shared` (`store_tilehub`, `store_updatecheck`, `store_compapply`, `store_wizardhook`).
//! The real implementation is `iyakki_agent::StoreFacetsImpl`; [`FakeStoreFacets`] is the fake: canned answers, every call recorded.
//! An install or an update of components is never done by the facet on its own: [`StoreComponents::components_apply`] takes the review the
//! person was shown and binds the approval to exactly that review's digest.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_drivers::{apply, Machine, Row};

use iyakki_sec::policy::Policy;
use iyakki_shared::store_compapply::{ApplyResult, Review, StoreLine};
use iyakki_shared::store_private::StatusInfo;
use iyakki_shared::store_tilehub::TileEntry;
use iyakki_shared::store_updatecheck::{CheckSettings, Report};
use iyakki_shared::store_wizardhook::Pending;

/// The answer of a call this core does not serve.
pub const UNAVAILABLE: &str = "unavailable: this core does not serve this call";

/// The tile apps of the launcher.
pub trait StoreHub: Send + Sync {
    /// The tile apps this person sees: installed for them, installed for everyone and, when added, the dev apps.
    fn tiles(&self, policy: &Policy, user: &str) -> Vec<TileEntry> {
        let _ = (policy, user);
        Vec::new()
    }
    /// An app folder to look at right now without adding it to the launcher (the IDE's Run).
    fn preview_tile(&self, user: &str, dir: &Path) -> Result<TileEntry, String> {
        let _ = (user, dir);
        Err(UNAVAILABLE.into())
    }
    /// Developer mode: add the app in a workspace folder as an unsigned dev app (administrators only; refused otherwise).
    fn add_dev_tile(&self, policy: &Policy, user: &str, dir: &Path) -> Result<TileEntry, String> {
        let _ = (policy, user, dir);
        Err(UNAVAILABLE.into())
    }
    fn remove_dev_tile(&self, user: &str, name: &str) -> Result<(), String> {
        let _ = (user, name);
        Err(UNAVAILABLE.into())
    }
    /// Is the experimental WASM tile switch on (off by default)?
    fn wasm_tiles_enabled(&self) -> bool {
        false
    }
}

/// A setup wizard an install or an update queued, with what it will ask and run in one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizardLine {
    pub pending: Pending,
    /// The wizard's outline when the installed app still has it, otherwise empty.
    pub outline: String,
}

/// The update check and the wizards the installs queued.
pub trait StoreUpdates: Send + Sync {
    /// The answer of the last update check (kept in the data folder).
    fn update_report(&self) -> Option<Report> {
        None
    }
    /// Is a check due now (the mode, the last check, a metered link, the device's own delay)?
    fn update_check_due(&self, settings: &CheckSettings) -> bool {
        let _ = settings;
        false
    }
    /// Look at the sources now and keep the answer. Slow (network): call it off the UI thread. It reports only; it never installs.
    fn update_check_run(&self, policy: &Policy, user: &str, settings: &CheckSettings, running: &str, variant: &str) -> Report {
        let _ = (policy, user, settings, running, variant);
        Report::default()
    }
    /// (name, announcement address) of every enabled store that was added with its key checked.
    fn update_announce_stores(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The setup wizards queued for this person, each with its one-line outline.
    fn pending_wizards(&self, user: &str) -> Vec<WizardLine> {
        let _ = user;
        Vec::new()
    }
}

/// The review of the system components of one store, bound to the channel it was made for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentReview {
    pub store: String,
    pub channel: String,
    pub review: Review,
}

/// The system components of the pinned private stores.
pub trait StoreComponents: Send + Sync {
    /// The channel this computer takes components from (`stable`, `beta` or `nightly`).
    fn components_channel(&self) -> String {
        "stable".into()
    }
    fn components_set_channel(&self, channel: &str) -> Result<(), String> {
        let _ = channel;
        Err(UNAVAILABLE.into())
    }
    /// Ask every pinned store how many parts are newer and keep the answer for the page. Reads signed lists only; slow (network).
    fn components_check_all(&self) -> Vec<StoreLine> {
        Vec::new()
    }
    /// The lines of the last check, one per store, in plain words.
    fn components_check_lines(&self) -> Vec<String> {
        Vec::new()
    }
    /// The last component install as the page shows it.
    fn components_last_result(&self) -> Option<ApplyResult> {
        None
    }
    /// Read the signed list of the first pinned store that answers and make the review (what changes, sizes, restart). Nothing is
    /// downloaded or written. Slow (network): call it off the UI thread.
    fn components_review(&self, channel: &str) -> Result<ComponentReview, String> {
        let _ = channel;
        Err(UNAVAILABLE.into())
    }
    /// Install exactly the reviewed parts: the approval is made from this review and `user`'s press, so a changed plan is refused. Returns the
    /// plain text of what was done.
    fn components_apply(&self, review: &ComponentReview, user: &str) -> Result<String, String> {
        let _ = (review, user);
        Err(UNAVAILABLE.into())
    }
}


/// The most a driver file the person points at may hold (a pack made from a file is refused above it).
pub const MAX_DRIVER_FILE: u64 = 16 * 1024 * 1024;

/// The drivers of this machine (docs/MAC-HARDWARE.md): what the machine is, what the store offers for it, what is installed, and the pack
/// text the normal install review takes for a folder or a file. The install itself goes through that review, never through this facet.
pub trait StoreDrivers: Send + Sync {
    fn machine(&self) -> Machine {
        Machine::default()
    }
    /// The "Drivers for this machine" list (empty for a machine none of the entries applies to).
    fn rows(&self) -> Vec<Row> {
        Vec::new()
    }
    fn installed(&self) -> Vec<apply::Record> {
        Vec::new()
    }
    /// Is the installed driver `id` set aside (turned off)?
    fn is_disabled(&self, id: &str) -> bool {
        let _ = id;
        false
    }
    /// The pack text and the names of the files taken from `dir` for driver `id` (the caller checked that the person may read `dir`).
    fn folder_package(&self, id: &str, dir: &Path, now: i64) -> Result<(String, Vec<String>), String> {
        let _ = (id, dir, now);
        Err(UNAVAILABLE.into())
    }
    /// The pack text and the notice for the FaceTime camera driver file `driver_file` (read by the caller, at most [`MAX_DRIVER_FILE`] bytes).
    fn facetime_package(&self, driver_file: &[u8], accepted: bool, now: i64) -> Result<(String, String), String> {
        let _ = (driver_file, accepted, now);
        Err(UNAVAILABLE.into())
    }
}

/// A handle that has no machine to read.
#[derive(Default)]
pub struct NoDrivers;
impl StoreDrivers for NoDrivers {}

/// Where the UI gets the drivers handle of this machine.
pub trait StoreDriverSource: Send + Sync {
    fn drivers(&self) -> Arc<dyn StoreDrivers> {
        Arc::new(NoDrivers)
    }
}

/// Small questions about the store's own files and rules.
pub trait StoreCatalog: Send + Sync {
    /// What the Private store card shows for this person (the deployed instance, the catalog added for it, the default publish target).
    fn private_store_status(&self, user: &str) -> StatusInfo {
        let _ = user;
        StatusInfo::default()
    }
    /// The words of one store permission (`tile:storage`, `net:host`, `vault:name` ...) for this person, or `None` when it is not a permission.
    fn permission_words(&self, permission: &str, user: &str) -> Option<String> {
        let _ = (permission, user);
        None
    }
    /// The folders on mounted USB sticks that hold a store catalog.
    fn stick_stores(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Write a starting project (`kind`: skill, mcp, tile ...) named `name` into `dir` inside `root`. The folder must be new or empty.
    fn scaffold_project(&self, kind: &str, name: &str, root: &Path, dir: &str) -> Result<String, String> {
        let _ = (kind, name, root, dir);
        Err(UNAVAILABLE.into())
    }
    /// The `version` a project folder declares (empty when it declares none).
    fn project_version(&self, dir: &Path) -> String {
        let _ = dir;
        String::new()
    }
}

/// One handle for the facets (the UI keeps one).
pub trait StoreFacets: StoreHub + StoreUpdates + StoreComponents + StoreDriverSource + StoreCatalog {}
impl<T: StoreHub + StoreUpdates + StoreComponents + StoreDriverSource + StoreCatalog> StoreFacets for T {}

/// A handle that answers the default of every call.
#[derive(Default)]
pub struct NoStoreFacets;
impl StoreHub for NoStoreFacets {}
impl StoreUpdates for NoStoreFacets {}
impl StoreComponents for NoStoreFacets {}
impl StoreDriverSource for NoStoreFacets {}
impl StoreCatalog for NoStoreFacets {}

/// The fake: canned answers, every call recorded by name.
#[derive(Default)]
pub struct FakeStoreFacets {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    calls: Vec<String>,
    tiles: Vec<TileEntry>,
    wasm: bool,
    report: Option<Report>,
    due: bool,
    stores: Vec<(String, String)>,
    wizards: Vec<WizardLine>,
    channel: String,
    check_lines: Vec<String>,
    last_result: Option<ApplyResult>,
    review: Option<ComponentReview>,
    applied: Vec<String>,
    drivers: Option<Arc<dyn StoreDrivers>>,
    private: StatusInfo,
    sticks: Vec<PathBuf>,
    scaffolded: Vec<String>,
}

impl FakeStoreFacets {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn note(&self, c: &str) {
        self.st().calls.push(c.to_string());
    }
    pub fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
    }
    pub fn set_tiles(&self, t: Vec<TileEntry>) {
        self.st().tiles = t;
    }
    pub fn set_wasm(&self, on: bool) {
        self.st().wasm = on;
    }
    pub fn set_report(&self, r: Option<Report>) {
        self.st().report = r;
    }
    pub fn set_due(&self, d: bool) {
        self.st().due = d;
    }
    pub fn set_stores(&self, s: Vec<(String, String)>) {
        self.st().stores = s;
    }
    pub fn set_wizards(&self, w: Vec<WizardLine>) {
        self.st().wizards = w;
    }
    pub fn set_check_lines(&self, l: Vec<String>) {
        self.st().check_lines = l;
    }
    pub fn set_last_result(&self, r: Option<ApplyResult>) {
        self.st().last_result = r;
    }
    pub fn set_review(&self, r: Option<ComponentReview>) {
        self.st().review = r;
    }
    pub fn set_private_status(&self, s: StatusInfo) {
        self.st().private = s;
    }
    pub fn set_sticks(&self, s: Vec<PathBuf>) {
        self.st().sticks = s;
    }
    /// `kind name` of every project the fake wrote, in order.
    pub fn scaffolded(&self) -> Vec<String> {
        self.st().scaffolded.clone()
    }
    /// The drivers handle the UI gets (default: none).
    pub fn set_drivers(&self, d: Arc<dyn StoreDrivers>) {
        self.st().drivers = Some(d);
    }
    /// The digests of the reviews that were applied, in order.
    pub fn applied(&self) -> Vec<String> {
        self.st().applied.clone()
    }
}

impl StoreHub for FakeStoreFacets {
    fn tiles(&self, _policy: &Policy, user: &str) -> Vec<TileEntry> {
        self.note(&format!("tiles({user})"));
        self.st().tiles.clone()
    }
    fn preview_tile(&self, user: &str, dir: &Path) -> Result<TileEntry, String> {
        self.note(&format!("preview_tile({user}, {})", dir.display()));
        self.st().tiles.first().cloned().ok_or_else(|| "no such app".to_string())
    }
    fn add_dev_tile(&self, policy: &Policy, user: &str, dir: &Path) -> Result<TileEntry, String> {
        self.note(&format!("add_dev_tile({user}, {})", dir.display()));
        if !policy.manage_settings {
            return Err("Only an administrator can add an unsigned dev app.".into());
        }
        self.st().tiles.first().cloned().ok_or_else(|| "no such app".to_string())
    }
    fn remove_dev_tile(&self, user: &str, name: &str) -> Result<(), String> {
        self.note(&format!("remove_dev_tile({user}, {name})"));
        let mut s = self.st();
        let n = s.tiles.len();
        s.tiles.retain(|t| !(t.dev && t.name == name));
        if s.tiles.len() == n { Err(format!("{name} is not one of your dev apps")) } else { Ok(()) }
    }
    fn wasm_tiles_enabled(&self) -> bool {
        self.note("wasm_tiles_enabled");
        self.st().wasm
    }
}

impl StoreUpdates for FakeStoreFacets {
    fn update_report(&self) -> Option<Report> {
        self.note("update_report");
        self.st().report.clone()
    }
    fn update_check_due(&self, _settings: &CheckSettings) -> bool {
        self.note("update_check_due");
        self.st().due
    }
    fn update_check_run(&self, _policy: &Policy, user: &str, _settings: &CheckSettings, running: &str, _variant: &str) -> Report {
        self.note(&format!("update_check_run({user}, {running})"));
        let mut s = self.st();
        s.due = false;
        s.report.clone().unwrap_or_default()
    }
    fn update_announce_stores(&self) -> Vec<(String, String)> {
        self.note("update_announce_stores");
        self.st().stores.clone()
    }
    fn pending_wizards(&self, user: &str) -> Vec<WizardLine> {
        self.note(&format!("pending_wizards({user})"));
        self.st().wizards.clone()
    }
}

impl StoreComponents for FakeStoreFacets {
    fn components_channel(&self) -> String {
        self.note("components_channel");
        let c = self.st().channel.clone();
        if c.is_empty() { "stable".into() } else { c }
    }
    fn components_set_channel(&self, channel: &str) -> Result<(), String> {
        self.note(&format!("components_set_channel({channel})"));
        if !["stable", "beta", "nightly"].contains(&channel) {
            return Err(format!("`{channel}` is not a channel this computer can follow"));
        }
        self.st().channel = channel.into();
        Ok(())
    }
    fn components_check_all(&self) -> Vec<StoreLine> {
        self.note("components_check_all");
        Vec::new()
    }
    fn components_check_lines(&self) -> Vec<String> {
        self.note("components_check_lines");
        self.st().check_lines.clone()
    }
    fn components_last_result(&self) -> Option<ApplyResult> {
        self.note("components_last_result");
        self.st().last_result.clone()
    }
    fn components_review(&self, channel: &str) -> Result<ComponentReview, String> {
        self.note(&format!("components_review({channel})"));
        self.st().review.clone().ok_or_else(|| "No store with a checked key is added yet.".to_string())
    }
    fn components_apply(&self, review: &ComponentReview, user: &str) -> Result<String, String> {
        self.note(&format!("components_apply({user})"));
        let mut s = self.st();
        // the approval is bound to exactly the review that was shown
        match &s.review {
            Some(r) if r.review.digest == review.review.digest => {
                s.applied.push(review.review.digest.clone());
                Ok(format!("Installed {} part(s) from {}.", review.review.parts, review.store))
            }
            _ => Err("The plan changed since it was reviewed; nothing was installed.".into()),
        }
    }
}

impl StoreDriverSource for FakeStoreFacets {
    fn drivers(&self) -> Arc<dyn StoreDrivers> {
        self.note("drivers");
        self.st().drivers.clone().unwrap_or_else(|| Arc::new(NoDrivers))
    }
}

impl StoreCatalog for FakeStoreFacets {
    fn private_store_status(&self, user: &str) -> StatusInfo {
        self.note(&format!("private_store_status({user})"));
        self.st().private.clone()
    }
    fn permission_words(&self, permission: &str, user: &str) -> Option<String> {
        self.note(&format!("permission_words({permission}, {user})"));
        permission.strip_prefix("tile:").map(|t| format!("tile permission {t}"))
    }
    fn stick_stores(&self) -> Vec<PathBuf> {
        self.note("stick_stores");
        self.st().sticks.clone()
    }
    fn scaffold_project(&self, kind: &str, name: &str, _root: &Path, _dir: &str) -> Result<String, String> {
        self.note(&format!("scaffold_project({kind}, {name})"));
        if !["skill", "mcp", "tile"].contains(&kind) {
            return Err(format!("unknown kind {kind:?}: skill, mcp, tile"));
        }
        self.st().scaffolded.push(format!("{kind} {name}"));
        Ok(format!("wrote {name}"))
    }
}
