//! The sources of the events apps hear (F-SDK-195 to 200, docs/sdk/events.md): what the machine's own services tell the app events engine.
//!
//! Two kinds, both ending in one call, `AppEvents::emit` on the engine of `Services::app_events`:
//!
//! * **Pushed.** A service that changes something says so at the place where it changes it: sign-in and sign-out, the vault locked or
//!   unlocked, an app installed, updated or removed, a backup finished. [`emit`] never fails the caller (a refused emit is a bug of the
//!   source and goes to the audit report), and builds the payload from words and counts only: never a password, a path's content, a
//!   setting's value or a person's file.
//! * **Pulled.** What has no single place to hook (a setting changed by a screen, a flow that ended, the network link, a folder, the
//!   person's theme, the clock) is looked at by [`SourcedEvents`] each time an app asks for its events (`start`, `poll`, at most once a
//!   second), with a pure state machine per source that compares with what it saw last time and emits the difference. The first look
//!   only remembers (nothing old is announced). A source that watches something expensive (a folder, the clock's ticks) looks only while
//!   somebody listens (`listeners`, `wanted`).
//!
//! Every pulled source takes its inputs as parameters (a listing, a link state, a clock, a file text), so a test needs no machine.

use iyakki_core_api::appevents::{self as ae, payload, AppEvent, AppEvents, EventInfo, Payload, RegInfo, ScopeCheck, SharedHub, Val};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------------------------------------ scopes

/// Who holds which `events.*` scope (a built-in app by the signed table, an installed app by the person's approval record), and which
/// folders an app may watch (the ones the core's own file listing lets it read).
pub struct AgentScopes {
    pub dir: PathBuf,
}

impl ScopeCheck for AgentScopes {
    fn holds(&self, _user: &str, app: &str, scope: &str) -> bool {
        let held = crate::accessbroker::held_list(&self.dir);
        // an installed app holds what the person approved for it and nothing by its name: a store app called "sample" is not the built-in one
        if held.iter().any(|h| h.app == app) {
            return held.iter().any(|h| h.app == app && h.holds.iter().any(|c| c == scope));
        }
        iyakki_core_api::appcaps::builtin_holds(app, scope)
    }
    fn held(&self, _user: &str, app: &str) -> Vec<String> {
        let held = crate::accessbroker::held_list(&self.dir);
        if held.iter().any(|h| h.app == app) {
            return held.iter().filter(|h| h.app == app).flat_map(|h| h.holds.clone()).collect();
        }
        // a built-in app holds what its row of the signed table gives and what every built-in holds
        iyakki_core_api::appcaps::CATALOGUE.iter().filter(|d| iyakki_core_api::appcaps::builtin_holds(app, d.name)).map(|d| d.name.to_string()).collect()
    }
    fn folder_allowed(&self, user: &str, _app: &str, folder: &str) -> bool {
        iyakki_core_local::homebrowse::list(&crate::scope::home_root_for_writes(user, &self.dir), folder).is_ok()
    }
}

// ------------------------------------------------------------------------------------------------ pushed

/// Tell the engine; a refusal means the source built a wrong payload, which is reported and never reaches the person's work.
pub fn emit(hub: &dyn AppEvents, name: &str, user: Option<&str>, p: Payload) {
    if let Err(e) = hub.emit(name, user, p) {
        iyakki_schema::report(format!("app event {name}: {e}"));
    }
}

fn word(s: &str) -> Val {
    Val::Text(s.into())
}

/// `state`: signed_in, signed_out, locked or unlocked.
pub fn session_changed(hub: &dyn AppEvents, user: &str, state: &str) {
    emit(hub, "session.changed", Some(user), payload(&[("state", word(state))]));
}

/// `state`: locked or unlocked.
pub fn vault_changed(hub: &dyn AppEvents, user: Option<&str>, state: &str) {
    emit(hub, "vault.changed", user, payload(&[("state", word(state))]));
}

/// `change`: installed, updated or removed. The app's id, never its data.
pub fn app_changed(hub: &dyn AppEvents, user: &str, app: &str, change: &str) {
    emit(hub, "app.installed", Some(user), payload(&[("app", word(app)), ("change", word(change))]));
}

/// `phase`: available, downloaded or applied.
pub fn update_changed(hub: &dyn AppEvents, component: &str, phase: &str) {
    emit(hub, "updates.changed", None, payload(&[("component", word(component)), ("phase", word(phase))]));
}

/// A backup ended; the name of the file is the only text.
pub fn backup_finished(hub: &dyn AppEvents, user: &str, name: &str, ok: bool) {
    emit(hub, "backup.finished", Some(user), payload(&[("name", word(name)), ("outcome", word(if ok { "finished" } else { "failed" }))]));
}

/// What an app may do changed from `was` to `now` (capability names): the app is told what was granted and taken away, whether it runs or
/// not, and the registrations of events it may no longer hear end. Nothing is told when nothing changed.
pub fn permissions_told(hub: &dyn AppEvents, user: &str, app: &str, was: &[String], now: &[String], by: &str, restart: bool) {
    let granted: Vec<String> = now.iter().filter(|n| !was.contains(n)).cloned().collect();
    let revoked: Vec<String> = was.iter().filter(|n| !now.contains(n)).cloned().collect();
    if granted.is_empty() && revoked.is_empty() {
        return;
    }
    if let Err(e) = hub.permissions_changed(user, app, &granted, &revoked, by, restart) {
        iyakki_schema::report(format!("app permissions {app}: {e}"));
    }
}

// ------------------------------------------------------------------------------------------------ pulled: settings, flows, link, prefs, clock, files

/// The setting keys that changed since the last call (the contract's own change log, so a change by a screen counts too).
pub trait SettingChanges: Send + Sync {
    fn take(&self) -> Vec<String>;
}

/// The settings of the machine as the person's Settings pages and `iyakki dev config set` change them: every leaf of the settings file, by its
/// dotted name (`time.hour12`), compared by a hash of its value so that no value is kept here or told to an app. The first look only remembers.
pub struct StoreChanges {
    store: Arc<Mutex<crate::settings::SettingsStore>>,
    last: Mutex<Option<BTreeMap<String, u64>>>,
}

impl StoreChanges {
    pub fn new(store: Arc<Mutex<crate::settings::SettingsStore>>) -> StoreChanges {
        StoreChanges { store, last: Mutex::new(None) }
    }
}

fn leaves(v: &serde_json::Value, prefix: &str, out: &mut BTreeMap<String, u64>) {
    use std::hash::{Hash, Hasher};
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                leaves(x, &if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, out);
            }
        }
        other => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            other.to_string().hash(&mut h);
            out.insert(prefix.to_string(), h.finish());
        }
    }
}

/// The names that differ between two sets of leaves (changed, added or removed).
pub fn changed_leaves(prev: &BTreeMap<String, u64>, now: &BTreeMap<String, u64>) -> Vec<String> {
    let mut out: Vec<String> = now.iter().filter(|(k, v)| prev.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
    out.extend(prev.keys().filter(|k| !now.contains_key(*k)).cloned());
    out
}

impl SettingChanges for StoreChanges {
    fn take(&self) -> Vec<String> {
        let mut now = BTreeMap::new();
        if let Ok(v) = serde_json::to_value(self.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get()) {
            leaves(&v, "", &mut now);
        }
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let out = last.as_ref().map(|p| changed_leaves(p, &now)).unwrap_or_default();
        *last = Some(now);
        out
    }
}

impl SettingChanges for crate::contract_local::CoreSettings {
    fn take(&self) -> Vec<String> {
        self.take_changes()
    }
}

/// A flow run that ended since the last look: `(flow name, ok)`. The first look only remembers the runs that exist.
#[derive(Default)]
pub struct FlowWatch {
    seen: BTreeMap<String, BTreeSet<String>>,
}

impl FlowWatch {
    /// `runs`: `(run id, flow name, ended, ok)`.
    pub fn observe(&mut self, user: &str, runs: &[(String, String, bool, bool)]) -> Vec<(String, bool)> {
        let first = !self.seen.contains_key(user);
        let seen = self.seen.entry(user.to_string()).or_default();
        let mut out = Vec::new();
        for (id, flow, ended, ok) in runs {
            if *ended && seen.insert(id.clone()) && !first {
                out.push((flow.clone(), *ok));
            }
        }
        out
    }
}

pub use iyakki_core_local::appevent_facts::link_state;

#[derive(Default)]
pub struct LinkWatch {
    last: Option<String>,
}

impl LinkWatch {
    /// The new state when it differs from the last look (never on the first look).
    pub fn observe(&mut self, state: &str) -> Option<String> {
        let changed = self.last.as_deref().is_some_and(|l| l != state);
        self.last = Some(state.to_string());
        changed.then(|| state.to_string())
    }
}

/// What changed in a person's `ui-<name>.json` and `access-<name>.json` between two looks: the words of `display.changed` and the
/// accessibility change.
pub fn prefs_changes(prev: &serde_json::Value, now: &serde_json::Value) -> Vec<(&'static str, &'static str)> {
    const FIELDS: [(&str, &str, &str); 4] = [("theme", "display", "theme"), ("size", "display", "font"), ("skin", "display", "skin"), ("scale", "display", "scale")];
    let mut out = Vec::new();
    for (field, kind, what) in FIELDS {
        if prev.get(field) != now.get(field) {
            out.push((kind, what));
        }
    }
    out
}

#[derive(Default)]
pub struct PrefsWatch {
    ui: BTreeMap<String, serde_json::Value>,
    access: BTreeMap<String, String>,
}

impl PrefsWatch {
    /// Look at the person's files; the first look only remembers.
    pub fn observe(&mut self, user: &str, ui: &str, access: &str) -> (Vec<(&'static str, &'static str)>, bool) {
        let now: serde_json::Value = serde_json::from_str(ui).unwrap_or(serde_json::Value::Null);
        let ch = match self.ui.insert(user.to_string(), now.clone()) {
            Some(prev) => prefs_changes(&prev, &now),
            None => Vec::new(),
        };
        let acc = match self.access.insert(user.to_string(), access.to_string()) {
            Some(prev) => prev != access,
            None => false,
        };
        (ch, acc)
    }
}

/// What the clock did between two looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Minute,
    Hour,
    Midnight,
    Zone,
    Jump,
}

impl Tick {
    pub fn event(self) -> &'static str {
        match self {
            Tick::Minute => "clock.minute",
            Tick::Hour => "clock.hour",
            Tick::Midnight => "clock.midnight",
            Tick::Zone | Tick::Jump => "clock.changed",
        }
    }
}

/// The ticks of the clock between two looks, and a change of the zone or a jump of the wall clock that the steady clock did not see. Pure:
/// the wall clock (seconds since 1970 and the zone's offset in seconds) and the steady clock (milliseconds) are passed in. The first look
/// only remembers.
#[derive(Default)]
pub struct ClockWatch {
    last: Option<(i64, u64, String)>,
}

impl ClockWatch {
    pub fn observe(&mut self, wall_s: i64, offset_s: i64, steady_ms: u64, zone: &str) -> Vec<Tick> {
        let mut out = Vec::new();
        if let Some((last_wall, last_steady, last_zone)) = self.last.clone() {
            let (a, b) = (last_wall + offset_s, wall_s + offset_s);
            let moved = wall_s - last_wall;
            let steady = (steady_ms.saturating_sub(last_steady) / 1000) as i64;
            if last_zone != zone {
                out.push(Tick::Zone);
            } else if (moved - steady).abs() > 5 {
                out.push(Tick::Jump);
            }
            if b.div_euclid(60) != a.div_euclid(60) {
                out.push(Tick::Minute);
            }
            if b.div_euclid(3600) != a.div_euclid(3600) {
                out.push(Tick::Hour);
            }
            if b.div_euclid(86_400) != a.div_euclid(86_400) {
                out.push(Tick::Midnight);
            }
        }
        self.last = Some((wall_s, steady_ms, zone.to_string()));
        out
    }
}

/// What the machine reports about power, memory and heat at one look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MachineFacts {
    /// A power adapter is connected; `None` on a machine that reports none.
    pub ac: Option<bool>,
    /// The charge in percent and whether the battery is running down; `None` without a battery.
    pub percent: Option<u32>,
    pub discharging: bool,
    /// `normal`, `high` or `critical`.
    pub memory: String,
    /// The count of times the processor slowed down because of heat, summed over the cores; only the growth matters.
    pub throttle: u64,
}

/// The charge at or under which a running-down battery is "low".
pub const LOW_PERCENT: u32 = 15;

/// The words of `memory.pressure` from the kernel's pressure file (`some avg10=N ...`): under 10 normal, under 40 high, else critical.
pub fn memory_level(text: &str) -> &'static str {
    let avg: f32 = text.lines().find(|l| l.starts_with("some")).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10="))).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    if avg < 10.0 { "normal" } else if avg < 40.0 { "high" } else { "critical" }
}

/// The processor's heat slow-down count, summed over the cores that report one.
pub fn throttle_total(sys: &Path) -> u64 {
    iyakki_core_local::machinefacts::throttle_total_in(sys)
}

#[derive(Default)]
pub struct MachineWatch {
    last: Option<MachineFacts>,
    low: bool,
    throttled: bool,
}

impl MachineWatch {
    /// The events between two looks. The first look only remembers. Quiet hours are not here: the machine has no such setting yet.
    pub fn observe(&mut self, now: MachineFacts) -> Vec<(&'static str, Payload)> {
        let mut out = Vec::new();
        let low_now = now.discharging && now.percent.is_some_and(|p| p <= LOW_PERCENT);
        if let Some(prev) = self.last.clone() {
            if let (Some(a), Some(b)) = (prev.ac, now.ac) {
                if a != b {
                    out.push(("power.source", payload(&[("source", word(if b { "ac" } else { "battery" }))])));
                }
            }
            if low_now && !self.low {
                out.push(("power.battery_low", payload(&[("percent", Val::Int(now.percent.unwrap_or(0) as i64))])));
            }
            if prev.memory != now.memory {
                out.push(("memory.pressure", payload(&[("level", word(&now.memory))])));
            }
            let throttled_now = now.throttle > prev.throttle;
            if throttled_now != self.throttled {
                out.push(("thermal.changed", payload(&[("state", word(if throttled_now { "throttled" } else { "normal" }))])));
            }
            self.throttled = throttled_now;
        }
        self.low = low_now;
        self.last = Some(now);
        out
    }
}

/// The devices and volumes at one look: names only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceFacts {
    /// Where block devices are mounted (the mount points).
    pub volumes: BTreeSet<String>,
    /// The sound cards by name.
    pub audio: BTreeSet<String>,
    /// The cameras by name.
    pub cameras: BTreeSet<String>,
}

/// The mount points of block devices in `/proc/mounts` text.
pub fn block_mounts(text: &str) -> BTreeSet<String> {
    iyakki_core_api::parse_mounts(text).into_iter().filter(|m| m.source.starts_with("/dev/")).map(|m| m.target).collect()
}

/// The sound card names in `/proc/asound/cards` text (` 0 [PCH            ]: HDA-Intel - HDA Intel PCH`).
pub fn sound_cards(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(|l| l.split_once("]:").map(|(_, rest)| rest.trim().to_string())).filter(|n| !n.is_empty()).collect()
}

fn short(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(ae::MAX_TEXT).collect()
}

#[derive(Default)]
pub struct DeviceWatch {
    last: Option<DeviceFacts>,
}

impl DeviceWatch {
    /// The events between two looks (the first look only remembers). A sound card is reported as a speaker (the kernel's card list does not
    /// say which way it goes) and a low-space warning needs a free-space read that this source does not make yet.
    pub fn observe(&mut self, now: DeviceFacts) -> Vec<(&'static str, Payload)> {
        let mut out = Vec::new();
        if let Some(prev) = &self.last {
            for v in now.volumes.difference(&prev.volumes) {
                out.push(("storage.volume", payload(&[("volume", word(&short(v))), ("change", word("mounted"))])));
            }
            for v in prev.volumes.difference(&now.volumes) {
                out.push(("storage.volume", payload(&[("volume", word(&short(v))), ("change", word("unmounted"))])));
            }
            for (change, set) in [("added", now.audio.difference(&prev.audio)), ("removed", prev.audio.difference(&now.audio))] {
                for d in set {
                    out.push(("audio.devices", payload(&[("kind", word("speaker")), ("change", word(change)), ("device", word(&short(d)))])));
                }
            }
            for (change, set) in [("added", now.cameras.difference(&prev.cameras)), ("removed", prev.cameras.difference(&now.cameras))] {
                for d in set {
                    out.push(("camera.devices", payload(&[("change", word(change)), ("device", word(&short(d)))])));
                }
            }
        }
        self.last = Some(now);
        out
    }
}

/// What is known about one person at one look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersonFacts {
    /// Unread notes that need the person.
    pub inbox: usize,
    /// A VPN profile is on and connected (`Some(true)`), on and not connected (`Some(false)`), or none is on (`None`).
    pub vpn: Option<bool>,
}

#[derive(Default)]
pub struct PersonWatch {
    last: BTreeMap<String, PersonFacts>,
}

impl PersonWatch {
    pub fn observe(&mut self, user: &str, now: PersonFacts) -> Vec<(&'static str, Payload)> {
        let mut out = Vec::new();
        if let Some(prev) = self.last.insert(user.to_string(), now.clone()) {
            if prev.inbox != now.inbox {
                out.push(("inbox.pending", payload(&[("count", Val::Int(now.inbox as i64))])));
            }
            if prev.vpn != now.vpn {
                if let Some(up) = now.vpn {
                    out.push(("vpn.changed", payload(&[("name", word("vpn")), ("state", word(if up { "up" } else { "down" }))])));
                } else if prev.vpn == Some(true) {
                    out.push(("vpn.changed", payload(&[("name", word("vpn")), ("state", word("down"))])));
                }
            }
        }
        out
    }
}

/// A folder as the core's file listing shows it: name, folder or not, size, modified time.
pub type Snapshot = BTreeMap<String, (bool, u64, Option<u64>)>;

pub fn snapshot(l: &iyakki_core_api::Listing) -> Snapshot {
    l.entries.iter().map(|e| (e.name.clone(), (e.is_dir, e.size, e.modified))).collect()
}

/// What changed in a folder between two looks: `(name, created | modified | removed)`.
pub fn folder_changes(prev: &Snapshot, now: &Snapshot) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (n, v) in now {
        match prev.get(n) {
            None => out.push((n.clone(), "created")),
            Some(p) if p != v => out.push((n.clone(), "modified")),
            _ => {}
        }
    }
    for n in prev.keys() {
        if !now.contains_key(n) {
            out.push((n.clone(), "removed"));
        }
    }
    out
}

/// One watched folder per `(person, folder)`.
#[derive(Default)]
pub struct FileWatch {
    seen: BTreeMap<(String, String), Snapshot>,
}

impl FileWatch {
    /// `now` is the listing the person's file policy allows (an `Err` is a folder that is gone or not allowed: nothing is announced, the
    /// memory is dropped so a folder that comes back is looked at afresh).
    pub fn observe(&mut self, user: &str, folder: &str, now: Result<Snapshot, String>) -> Vec<(String, &'static str)> {
        let k = (user.to_string(), folder.to_string());
        match now {
            Err(_) => {
                self.seen.remove(&k);
                Vec::new()
            }
            Ok(n) => match self.seen.insert(k, n.clone()) {
                Some(prev) => folder_changes(&prev, &n),
                None => Vec::new(),
            },
        }
    }
}

// ------------------------------------------------------------------------------------------------ the facet with its sources

/// What the pulled sources read from this machine (a test gives its own).
pub struct Inputs {
    pub settings: Arc<dyn SettingChanges>,
    /// `(run id, flow name, ended, ok)` of one person's flow runs; `None` when no engine runs for them.
    pub flows: Box<dyn Fn(&str) -> Option<Vec<(String, String, bool, bool)>> + Send + Sync>,
    pub link: Box<dyn Fn() -> &'static str + Send + Sync>,
    /// The text of `ui-<name>.json` and of `access-<name>.json`, empty when there is none.
    pub prefs: Box<dyn Fn(&str) -> (String, String) + Send + Sync>,
    /// Wall seconds, the zone's offset in seconds, steady milliseconds, the zone's name.
    pub clock: Box<dyn Fn() -> (i64, i64, u64, String) + Send + Sync>,
    pub list: Box<dyn Fn(&str, &str) -> Result<iyakki_core_api::Listing, String> + Send + Sync>,
    /// Where on this machine a person's watched folder is (the kernel file watch watches there; the events still come from `list`).
    pub folder_dir: Box<dyn Fn(&str, &str) -> std::path::PathBuf + Send + Sync>,
    /// Power, memory and heat of this machine.
    pub machine: Box<dyn Fn() -> MachineFacts + Send + Sync>,
    /// Volumes, sound cards and cameras.
    pub devices: Box<dyn Fn() -> DeviceFacts + Send + Sync>,
    /// The inbox and the VPN of one person.
    pub person: Box<dyn Fn(&str) -> PersonFacts + Send + Sync>,
    /// The further machine-wide facts (F-SDK-204 to 207, 211) and the further per-person counts.
    pub more: Box<dyn Fn() -> crate::appevent_more::MoreFacts + Send + Sync>,
    /// What each app effectively holds and the causes that explain a change (F-SDK-217).
    pub perm: Box<dyn Fn() -> crate::appevent_perm::PermFacts + Send + Sync>,
    pub more_person: Box<dyn Fn(&str) -> crate::appevent_more::MorePersonFacts + Send + Sync>,
    /// USB devices, the signs of sharing, the zone the network chose and the storage of the apps that listen for it (given as `(person, app)`).
    pub extra: Box<dyn Fn(&[(String, String)]) -> crate::appevent_extra::ExtraFacts + Send + Sync>,
    /// The earliest the sources look again, in steady milliseconds (a test passes its own clock).
    pub steady_ms: Box<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Default)]
struct State {
    flows: FlowWatch,
    link: LinkWatch,
    prefs: PrefsWatch,
    clock: ClockWatch,
    files: FileWatch,
    machine: MachineWatch,
    devices: DeviceWatch,
    persons: PersonWatch,
    more: crate::appevent_more::MoreWatch,
    more_persons: crate::appevent_more::MorePersonWatch,
    perm: crate::appevent_perm::PermWatch,
    extra: crate::appevent_extra::ExtraWatch,
    last_look: Option<u64>,
}

/// The engine of the machine plus its pulled sources: an app that starts or polls first makes the sources look (at most once a second).
pub struct SourcedEvents {
    hub: Arc<SharedHub>,
    inputs: Inputs,
    state: Mutex<State>,
    /// The kernel's watch of the folders somebody listens to; none where the system has no inotify.
    watch: Option<crate::fnotify::DirWatch>,
    /// A person may have no UI file; the people to look at are the ones who listen.
    min_gap_ms: u64,
}

impl SourcedEvents {
    pub fn new(hub: Arc<SharedHub>, inputs: Inputs) -> SourcedEvents {
        SourcedEvents { hub, inputs, state: Mutex::new(State::default()), watch: crate::fnotify::DirWatch::new().ok(), min_gap_ms: 1000 }
    }

    pub fn hub(&self) -> &Arc<SharedHub> {
        &self.hub
    }

    /// Make every source look now (tests; `start` and `poll` do it at most once a second).
    pub fn look(&self) {
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.look_locked(&mut st);
    }

    /// The core's own clock for the sources: look now if an app is registered (at most once a second), whether or not any app is open.
    /// Events that happen while an app is away are queued for it (late) by this look. A test calls it with its own steady clock.
    pub fn tick(&self) {
        // an app that asked to be woken at a time is told when the time comes, whether it runs or not (F-SDK-203)
        self.hub.fire_wakes();
        if self.hub.any_registrations() {
            self.look_due();
        }
    }

    /// Start the thread that calls [`SourcedEvents::tick`] once a second for as long as the object lives (it holds only a weak reference). Where the
    /// kernel can watch folders it waits on that watch instead of sleeping, so a change in a folder an app listens to is looked at the moment it
    /// happens (F-SDK-208) and not at the next second.
    pub fn spawn_ticker(this: &Arc<SourcedEvents>) {
        let weak = Arc::downgrade(this);
        let _ = std::thread::Builder::new().name("app-event-sources".into()).spawn(move || loop {
            let Some(s) = weak.upgrade() else { return };
            let changed = s.wait_a_second();
            s.tick();
            if changed {
                s.look_files_now();
            }
            drop(s);
        });
    }

    /// Wait up to a second; ends sooner (and says so) when a watched folder changed. The watched folders are those somebody listens to now.
    fn wait_a_second(&self) -> bool {
        let Some(w) = &self.watch else {
            std::thread::sleep(std::time::Duration::from_secs(1));
            return false;
        };
        if self.watch_listened_folders() == 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            return false;
        }
        w.wait(std::time::Duration::from_secs(1))
    }

    /// Point the kernel watch at the folders somebody listens to now; how many folders it watches (0 where there is no watch).
    pub(crate) fn watch_listened_folders(&self) -> usize {
        let Some(w) = &self.watch else { return 0 };
        let dirs: BTreeSet<std::path::PathBuf> = self.hub.listeners("files.changed").into_iter().map(|(u, _app, folder)| (self.inputs.folder_dir)(&u, &folder)).collect();
        w.set_dirs(&dirs)
    }

    /// Look at the watched folders now (a kernel watch said something changed there).
    pub fn look_files_now(&self) {
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.look_files(&mut st);
    }

    fn look_due(&self) {
        let now = (self.inputs.steady_ms)();
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if st.last_look.is_some_and(|l| now.saturating_sub(l) < self.min_gap_ms) {
            return;
        }
        st.last_look = Some(now);
        self.look_locked(&mut st);
    }

    fn look_locked(&self, st: &mut State) {
        let hub: &dyn AppEvents = &*self.hub;
        // settings: the keys, and the display words of the ones that are the screen's
        for key in self.inputs.settings.take() {
            let (n, p) = ae::from_setting_change(&key);
            emit(hub, n, None, p);
        }
        // flows that ended, for each person who listens
        let people: BTreeSet<String> = hub.listeners("flows.run").into_iter().map(|l| l.0).collect();
        for u in &people {
            if let Some(runs) = (self.inputs.flows)(u) {
                for (flow, ok) in st.flows.observe(u, &runs) {
                    let (n, p) = ae::from_flow_end(&flow, ok);
                    emit(hub, n, Some(u), p);
                }
            }
        }
        // the link
        if let Some(s) = st.link.observe((self.inputs.link)()) {
            emit(hub, "connectivity.changed", None, payload(&[("state", word(&s))]));
        }
        // the person's look and accessibility, for each person who listens to either
        let people: BTreeSet<String> = ["display.changed", "accessibility.changed"].iter().flat_map(|e| hub.listeners(e)).map(|l| l.0).collect();
        for u in &people {
            let (ui, acc) = (self.inputs.prefs)(u);
            let (ch, acc_changed) = st.prefs.observe(u, &ui, &acc);
            for (_, what) in ch {
                emit(hub, "display.changed", Some(u), payload(&[("what", word(what))]));
            }
            if acc_changed {
                emit(hub, "accessibility.changed", Some(u), payload(&[("what", word("settings"))]));
            }
        }
        // the clock: the ticks only while somebody listens, but the look is always remembered
        let (wall, off, steady, zone) = (self.inputs.clock)();
        for t in st.clock.observe(wall, off, steady, &zone) {
            let name = t.event();
            let wanted = !matches!(t, Tick::Minute | Tick::Hour) || hub.wanted(name);
            if wanted {
                let p = match t {
                    Tick::Zone => payload(&[("change", word("timezone"))]),
                    Tick::Jump => payload(&[("change", word("time_jump"))]),
                    _ => Payload::new(),
                };
                emit(hub, name, None, p);
            }
        }
        // power, memory and heat
        for (name, p) in st.machine.observe((self.inputs.machine)()) {
            emit(hub, name, None, p);
        }
        // volumes, sound cards and cameras
        for (name, p) in st.devices.observe((self.inputs.devices)()) {
            emit(hub, name, None, p);
        }
        // the inbox and the VPN, for each person who listens to either
        let people: BTreeSet<String> = ["inbox.pending", "vpn.changed"].iter().flat_map(|e| hub.listeners(e)).map(|l| l.0).collect();
        for u in &people {
            for (name, p) in st.persons.observe(u, (self.inputs.person)(u)) {
                emit(hub, name, Some(u), p);
            }
        }
        // the further machine-wide sources
        for (name, p) in st.more.observe((self.inputs.more)()) {
            emit(hub, name, None, p);
        }
        // USB, the signs of sharing, the zone from the network, and the apps' own storage
        let quota_apps: Vec<(String, String)> = hub.listeners("storage.quota").into_iter().map(|l| (l.0, l.1)).collect();
        for (name, who, p) in st.extra.observe((self.inputs.extra)(&quota_apps)) {
            emit(hub, name, who.as_deref(), p);
        }
        // what apps hold, when a policy, an end time or the store's list explains a change
        crate::appevent_perm::tell(hub, st.perm.observe((self.inputs.perm)()));
        // approvals, security counts and reminders, for each person who listens to one of them
        let people: BTreeSet<String> = ["approvals.pending", "security.alert", "reminder.due", "backup.finished", "transfer.finished"].iter().flat_map(|e| hub.listeners(e)).map(|l| l.0).collect();
        for u in &people {
            for (name, p) in st.more_persons.observe(u, (self.inputs.more_person)(u)) {
                emit(hub, name, Some(u), p);
            }
        }
        self.look_files(st);
    }
}

impl SourcedEvents {
    /// The folders somebody watches (through the person's own file policy): the difference to the last look, names only.
    fn look_files(&self, st: &mut State) {
        let hub: &dyn AppEvents = &*self.hub;
        // the folders somebody watches (through the person's own file policy)
        for (u, _app, folder) in hub.listeners("files.changed") {
            let now = (self.inputs.list)(&u, &folder).map(|l| snapshot(&l));
            for (name, change) in st.files.observe(&u, &folder, now) {
                emit(hub, "files.changed", Some(&u), payload(&[("folder", word(&folder)), ("name", word(&name)), ("change", word(change))]));
            }
        }
    }
}

impl AppEvents for SourcedEvents {
    fn catalogue(&self) -> Vec<EventInfo> {
        self.hub.catalogue()
    }
    fn list(&self, u: &str, a: &str) -> Vec<RegInfo> {
        self.hub.list(u, a)
    }
    fn apps(&self, u: &str) -> Vec<(String, Vec<RegInfo>)> {
        self.hub.apps(u)
    }
    fn register(&self, u: &str, a: &str, e: &str, f: &str) -> Result<(), String> {
        self.hub.register(u, a, e, f)
    }
    fn unregister(&self, u: &str, a: &str, e: &str, f: &str) -> Result<(), String> {
        self.hub.unregister(u, a, e, f)
    }
    fn set_enabled(&self, u: &str, a: &str, e: &str, f: &str, on: bool) -> Result<(), String> {
        self.hub.set_enabled(u, a, e, f, on)
    }
    fn start(&self, u: &str, a: &str) -> Vec<AppEvent> {
        self.look_due();
        self.hub.start(u, a)
    }
    fn stop(&self, u: &str, a: &str) {
        self.hub.stop(u, a)
    }
    fn poll(&self, u: &str, a: &str, max: usize) -> Vec<AppEvent> {
        self.look_due();
        self.hub.poll(u, a, max)
    }
    fn replay(&self, u: &str, a: &str, after: u64) -> Vec<AppEvent> {
        self.hub.replay(u, a, after)
    }
    fn permissions_changed(&self, u: &str, a: &str, g: &[String], r: &[String], by: &str, restart: bool) -> Result<usize, String> {
        self.hub.permissions_changed(u, a, g, r, by, restart)
    }
    fn set_permission(&self, u: &str, a: &str, sc: &str, allow: bool, by: &str) -> Result<usize, String> {
        self.hub.set_permission(u, a, sc, allow, by)
    }
    fn permissions(&self, u: &str, a: &str) -> (Vec<String>, Option<String>) {
        self.hub.permissions(u, a)
    }
    fn known_apps(&self, u: &str) -> Vec<String> {
        self.hub.known_apps(u)
    }
    fn requeue(&self, u: &str, a: &str, after: u64) -> Result<usize, String> {
        self.hub.requeue(u, a, after)
    }
    fn publish(&self, u: &str, a: &str, n: &str, p: Payload) -> Result<usize, String> {
        self.hub.publish(u, a, n, p)
    }
    fn emit(&self, n: &str, u: Option<&str>, p: Payload) -> Result<usize, String> {
        self.hub.emit(n, u, p)
    }
    fn listeners(&self, e: &str) -> Vec<(String, String, String)> {
        self.hub.listeners(e)
    }
    fn wanted(&self, n: &str) -> bool {
        self.hub.wanted(n)
    }
    fn set_flag(&self, f: &str, on: bool) {
        self.hub.set_flag(f, on)
    }
    fn wake_at(&self, u: &str, a: &str, at_ms: u64, tag: &str) -> Result<(), String> {
        self.hub.wake_at(u, a, at_ms, tag)
    }
    fn wake_cancel(&self, u: &str, a: &str, tag: &str) -> bool {
        self.hub.wake_cancel(u, a, tag)
    }
    fn wakes(&self, u: &str, a: &str) -> Vec<(u64, String)> {
        self.hub.wakes(u, a)
    }
    fn fire_wakes(&self) -> usize {
        self.hub.fire_wakes()
    }
    fn has_wakes(&self) -> bool {
        self.hub.has_wakes()
    }
    fn audit(&self) -> Vec<String> {
        self.hub.audit()
    }
}

/// The inputs of this machine: the settings contract's change log, the person's flows engine, the interface table, the person's UI and
/// accessibility files, the system clock, and the core's own file listing (the file policy of the person's home).
pub fn real_inputs(services: &Arc<crate::services::Services>) -> Inputs {
    let (svc, dir, dir2, dir3) = (services.clone(), services.dir().to_path_buf(), services.dir().to_path_buf(), services.dir().to_path_buf());
    let start = std::time::Instant::now();
    let start2 = start;
    let svc2 = services.clone();
    let svc3 = services.clone();
    let dir4 = services.dir().to_path_buf();
    let (dir_more, dir_feed) = (services.dir().to_path_buf(), services.dir().to_path_buf());
    let svc_models = services.clone();
    Inputs {
        settings: Arc::new(StoreChanges::new(services.settings.clone())),
        machine: Box::new(|| {
            let r = iyakki_battery::read(&iyakki_battery::BatteryEnv::real());
            let sys = crate::browserdev::sys_root();
            MachineFacts {
                ac: r.ac_online,
                percent: r.percent.map(|p| p.round() as u32),
                discharging: r.status == iyakki_battery::Status::Discharging,
                memory: memory_level(&iyakki_core_local::machinefacts::memory_pressure()).to_string(),
                throttle: throttle_total(&sys),
            }
        }),
        devices: Box::new(|| {
            let cams: BTreeSet<String> = iyakki_core_local::machinefacts::camera_names();
            DeviceFacts { volumes: block_mounts(&iyakki_core_local::storage::mounts_text()), audio: iyakki_core_local::devctl::cards(&iyakki_core_local::devctl::Roots::real()).into_iter().map(|c| if c.long_name.is_empty() { c.name } else { format!("{} - {}", c.name, c.long_name) }).filter(|n| !n.is_empty()).collect(), cameras: cams }
        }),
        more: Box::new(move || {
            use crate::{appevent_feed as f, appevent_more as m};
            let sys = crate::browserdev::sys_root();
            let now = crate::clock::now_unix();
            m::MoreFacts {
                suspends: m::suspend_count(&sys),
                shutdown_in: iyakki_core_local::machinefacts::shutdown_scheduled().and_then(|t| m::shutdown_in(&t, now)),
                free_percent: m::free_percents(&iyakki_core_local::storage::mounts_text()),
                disk_ok: m::disk_states(&sys),
                bluetooth: m::bluetooth_connected(&sys, std::path::Path::new(iyakki_core_local::devctl::bthid_sys::STORE_PATH)),
                printers: m::printers_and_scanners(&sys),
                resolution: m::screen_resolution(&sys),
                locale: m::machine_locale(&dir_more, &std::env::var("LC_ALL").unwrap_or_default(), &std::env::var("LANG").unwrap_or_default()),
                metered: crate::meteredlink::metered_now() == Some(true),
                captive: f::captive_now(),
                peers: f::paired_computers(&dir_more),
                quiet: f::quiet_now(&dir_more, now.max(0) as u64),
                wake_heard: crate::wakestats::load(&dir_more).heard,
                updates: f::update_phases(&dir_more, &f::outdated_items(&svc_models)),
                model: {
                    let ids: Vec<String> = iyakki_shared::modelhost::Hosts::new(&dir_more).list().into_iter().map(|h| h.id).collect();
                    f::model_host(&ids, &|id| svc_models.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().provider(id).and_then(|p| p.default_model.clone()))
                },
                certs: f::cert_days(&dir_more, now),
                ..Default::default()
            }
        }),
        extra: {
            let (svc4, dir5) = (services.clone(), services.dir().to_path_buf());
            Box::new(move |apps: &[(String, String)]| {
                use crate::appevent_extra as x;
                let (screen_clients, clipboard_uses) = x::sharing_now();
                let zone_auto = svc4.settings.lock().map(|s| s.get().time.zone_auto).unwrap_or(false);
                let quotas = apps
                    .iter()
                    .map(|(user, app)| {
                        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
                        ((user.clone(), app.clone()), (x::app_bytes(&dir5.join("appdata").join("tile-sdk").join(who), app), x::APP_STORAGE_BYTES))
                    })
                    .collect();
                x::ExtraFacts { usb: x::usb_devices(&crate::browserdev::sys_root()), screen_clients, clipboard_uses, zone_auto, zone: crate::clock::chosen_zone_name().unwrap_or_default(), quotas }
            })
        },
        perm: Box::new(move || crate::appevent_permsrc::facts_from_dir(&dir4, crate::clock::now_unix())),
        more_person: Box::new(move |user| {
            let approvals = svc3.runs.pending_approvals(Some(user)).len();
            let totals = crate::appevent_feed::audit_totals(&std::fs::read_to_string(dir_feed.join("audit.log")).unwrap_or_default());
            let now = crate::clock::now_unix();
            let (backups, backup_last) = crate::appevent_more::backups_made(&dir_feed, user);
            let (downloads, download_last) = crate::appevent_more::downloads_made(&dir_feed);
            let upcoming = svc3.calendar.events_in(user, now - 86_400, now + 86_400, &crate::clock::Zone::system());
            crate::appevent_more::MorePersonFacts { approvals, failed_signins: totals.failed_signins, new_paired_devices: totals.new_paired_devices, new_place_unlocks: crate::unlockplaces::new_unlocks(&dir_feed, user), reminders_due: crate::appevent_more::reminders_due_now(&upcoming, now), backups, backup_last, downloads, download_last, ..Default::default() }
        }),
        person: Box::new(move |user| {
            let inbox = svc2.inbox.counts(user, crate::clock::now_unix()).attention;
            let vpn = crate::vpn_mgr::Manager::existing(&svc2, user).and_then(|m| m.summary()).map(|s| matches!(s.light, crate::vpn_mgr::Light::Secure));
            PersonFacts { inbox, vpn }
        }),
        flows: Box::new(move |user| {
            let env = crate::flows::FlowEnv::existing(&svc, user)?;
            Some(env.engine.summaries().into_iter().map(|s| (s.id, s.flow_name, s.status.is_terminal(), s.status == iyakki_flow::model::RunStatus::Succeeded)).collect())
        }),
        link: Box::new(|| link_state(&crate::browserdev::sys_root())),
        prefs: Box::new(move |user| {
            let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
            (std::fs::read_to_string(dir.join(format!("ui-{who}.json"))).unwrap_or_default(), std::fs::read_to_string(dir.join(format!("access-{who}.json"))).unwrap_or_default())
        }),
        clock: Box::new(move || {
            let now = crate::clock::now_unix();
            let z = crate::clock::Zone::system();
            let off = z.local(now).offset as i64;
            (now, off, start.elapsed().as_millis() as u64, crate::clock::chosen_zone_name().unwrap_or_default())
        }),
        list: Box::new(move |user, folder| iyakki_core_local::homebrowse::list(&crate::scope::home_root_for_writes(user, &dir2), folder)),
        folder_dir: {
            let dir6 = services.dir().to_path_buf();
            Box::new(move |user, folder| crate::fnotify::folder_path(&crate::scope::home_root_for_writes(user, &dir6), folder))
        },
        steady_ms: Box::new(move || {
            let _ = &dir3;
            start2.elapsed().as_millis() as u64
        }),
    }
}

#[cfg(test)]
#[path = "appevent_sources_tests.rs"]
mod tests;
