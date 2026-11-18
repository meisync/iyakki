//! Events delivered to apps, online and offline (docs/sdk/events.md; owner request 2026-11-17).
//!
//! The core already has one source of truth for what happens on the machine (the pushed-event log of the core contract, F-CORE-18, the
//! notification bus, the file watcher, the settings store). This module is the app-facing layer on top of them, not a second bus: a
//! service that learns something calls [`AppEvents::emit`] with a catalogue name and a typed payload; the core decides who hears it.
//!
//! * **Catalogue.** [`CATALOGUE`] lists every event: name, version, the scope an app must hold (the person sees it on the install card and in
//!   Settings), the fields of its payload and their types, whether it coalesces and its rate limit. A payload that does not match is refused
//!   (the emitting service has a bug); a field named like a secret is refused for custom events. Payloads never carry a secret, a password,
//!   a token or a file's content: settings events carry the key and never the value.
//! * **Registration.** An app registers per event (and, for settings and files, a prefix). It is kept per person and app, survives a restart
//!   and shows in Settings > Apps, where the person switches each one off ([`AppEvents::set_enabled`]).
//! * **Delivery.** One queue per app. While the app runs, events are handed out in order, each with a sequence number that only grows and a
//!   time; while it does not run (or the machine slept) they wait in the same queue and arrive at the next start, before the first frame,
//!   marked `late`. The queue is bounded by count and age; the oldest are dropped and counted in one `missed` marker that is delivered first.
//!   State events (network status, battery, display) coalesce: only the latest of its kind (and key) waits. A noisy event is rate limited.
//! * **Replay.** The last delivered events are kept, so an app that crashed before it handled them can ask again ([`AppEvents::replay`]).
//! * **Audit.** Every registration, refusal and delivery is logged by names and numbers, never by payload values.
//!
//! The engine ([`EventHub`]) is pure: the clock is passed in, the store and the scope check are traits. [`SharedHub`] puts it behind a lock
//! and a clock and is what the fake and the local core use, so the rules are the same everywhere.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

/// The version of this facet. Additive changes only.
pub const APP_EVENTS_FACET_VERSION: u32 = 1;
/// Most events waiting for one app; one more drops the oldest and counts it as missed.
pub const MAX_QUEUE: usize = 100;
/// The longest an event waits for an app that does not run (7 days).
pub const MAX_AGE_MS: u64 = 7 * 24 * 3600 * 1000;
/// Most delivered events kept for replay.
pub const MAX_HISTORY: usize = 50;
/// Most registrations of one app.
pub const MAX_REGISTRATIONS: usize = 40;
/// The longest a text field of a payload is (device and app text is cut, not trusted).
pub const MAX_TEXT: usize = 200;
/// Most fields of a custom event.
pub const MAX_CUSTOM_FIELDS: usize = 8;
/// The name of the marker that says events were dropped.
/// An app counts as running only while it polls: not polling for this long means it is away (a shown app polls every frame). An event that waited
/// longer than this before it was handed over is `late`, whatever the app said about stopping.
pub const AWAY_AFTER_MS: u64 = 3000;
pub const MISSED: &str = "missed";
pub const NOT_REGISTERED: &str = "this app has not registered for that event";
pub const ALWAYS_ON: &str = "an app always hears what it may do; the person can take the permissions away instead";
pub const NO_EVENTS: &str = "this core has no app events";

/// One field of a payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Val {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Val {
    pub fn text(&self) -> Option<&str> {
        if let Val::Text(s) = self { Some(s) } else { None }
    }
    pub fn int(&self) -> Option<i64> {
        if let Val::Int(n) = self { Some(*n) } else { None }
    }
}

pub type Payload = BTreeMap<String, Val>;

/// A payload from pairs of text (a test and a service helper).
pub fn payload(pairs: &[(&str, Val)]) -> Payload {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// The type of a payload field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Text,
    Bool,
    Int,
    /// One of these words.
    Word(&'static [&'static str]),
}

/// How an event waits for an app that is not listening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coalesce {
    /// Several events become one: the latest effective state, with the changes since the app last heard added up (only `app.permissions`).
    Merge,
    /// Every event is kept (up to the bounds).
    No,
    /// Only the latest of this event waits.
    Kind,
    /// Only the latest per value of this field waits.
    By(&'static str),
}

pub struct EventDef {
    pub name: &'static str,
    pub version: u32,
    /// The capability the app must hold ([`crate::appcaps::CATALOGUE`]).
    pub scope: &'static str,
    /// What the person reads in Settings.
    pub gives: &'static str,
    pub fields: &'static [(&'static str, Field)],
    pub coalesce: Coalesce,
    /// Most events of this name accepted per minute for one app (only for events that do not coalesce; 0 = no limit).
    pub per_min: u32,
    /// A registration may carry a prefix that is matched against this field (settings key, folder).
    pub filter_field: Option<&'static str>,
    /// The event exists only while this switch is on (the experimental wake phrase).
    pub needs_flag: Option<&'static str>,
    /// A timer: the core runs it only while somebody listens ([`EventHub::wanted`]).
    pub timer: bool,
    /// Heard by the app it is about without registering, without a scope, and the person cannot switch it off (`app.permissions`).
    pub implicit: bool,
}

use Coalesce as C;
use Field as F;

const fn ev(name: &'static str, scope: &'static str, gives: &'static str, fields: &'static [(&'static str, Field)], coalesce: Coalesce, per_min: u32) -> EventDef {
    EventDef { name, version: 1, scope, gives, fields, coalesce, per_min, filter_field: None, needs_flag: None, timer: false, implicit: false }
}

const fn implicit(mut d: EventDef) -> EventDef {
    d.implicit = true;
    d
}

const fn timer(mut d: EventDef) -> EventDef {
    d.timer = true;
    d
}
const fn filtered(mut d: EventDef, field: &'static str) -> EventDef {
    d.filter_field = Some(field);
    d
}
const fn flagged(mut d: EventDef, flag: &'static str) -> EventDef {
    d.needs_flag = Some(flag);
    d
}

/// The scopes of the catalogue, once each (they are also in the capability catalogue, with these words).
pub const SCOPES: &[(&str, &str)] = &[
    ("events.network", "hear about the network: online, offline, metered, a sign-in page, a VPN, paired computers"),
    ("events.power", "hear about power: battery or mains, low battery, sleep and wake, heat, memory pressure, quiet hours"),
    ("events.session", "hear that a person signed in or out, locked or unlocked the screen, and that the vault opened or closed"),
    ("events.security", "hear about security alerts: failed sign-ins, a new paired device, an unlock from a new place (never who or where)"),
    ("events.display", "hear about the screen: size, scale, theme, skin and font changes"),
    ("events.locale", "hear about the language and the accessibility settings"),
    ("events.devices", "hear about sound devices, cameras, printers, scanners, Bluetooth devices and disks being added, removed or unwell"),
    ("events.storage", "hear about volumes being mounted, unmounted or nearly full, and about backups"),
    ("events.clock", "hear about clock changes and, if it asks, a tick every minute or hour"),
    ("events.settings", "hear that a setting changed (its name, never its value)"),
    ("events.apps", "hear about apps coming to the front or going away, installed, updated or removed"),
    ("events.inbox", "hear that notifications and approvals are waiting (the number, never the text)"),
    ("events.updates", "hear that an update is available, downloaded or applied, and that a download or install finished"),
    ("events.flows", "hear that a flow or job finished or failed"),
    ("events.files", "hear that a file in a folder it may read changed (the name, never the content)"),
    ("events.models", "hear that the model host is ready, failed or changed model"),
    ("events.voice", "hear the wake phrase (experimental, only while the person switched it on)"),
    ("events.reminders", "hear that a reminder or a calendar entry is due"),
    ("events.custom", "hear the events other apps publish"),
    ("events.publish", "publish events of its own for other apps"),
];

const WORD_NET: &[&str] = &["online", "offline", "metered", "captive_portal"];
const WORD_SRC: &[&str] = &["ac", "battery"];
const WORD_SLEEP: &[&str] = &["sleep", "wake"];
const WORD_SESSION: &[&str] = &["signed_in", "signed_out", "locked", "unlocked"];
const WORD_LOCK: &[&str] = &["locked", "unlocked"];
const WORD_DISPLAY: &[&str] = &["resolution", "scale", "theme", "skin", "font"];
const WORD_DEV: &[&str] = &["added", "removed", "default_changed"];
const WORD_DEVKIND: &[&str] = &["speaker", "microphone"];
const WORD_VOL: &[&str] = &["mounted", "unmounted", "low_space"];
const WORD_CLOCK: &[&str] = &["timezone", "time_jump"];
const WORD_LIFE: &[&str] = &["foreground", "background"];
const WORD_INST: &[&str] = &["installed", "updated", "removed"];
const WORD_UPD: &[&str] = &["available", "downloaded", "applied"];
const WORD_RUN: &[&str] = &["finished", "failed"];
const WORD_FILE: &[&str] = &["created", "modified", "removed"];
const WORD_PEER: &[&str] = &["paired", "forgotten", "online", "offline"];
const WORD_UPDOWN: &[&str] = &["up", "down"];
const WORD_CONN: &[&str] = &["connected", "disconnected"];
const WORD_THERM: &[&str] = &["throttled", "normal"];
const WORD_MEM: &[&str] = &["normal", "high", "critical"];
const WORD_HEALTH: &[&str] = &["warning", "ok"];
const WORD_SEC: &[&str] = &["failed_signin", "new_paired_device", "vault_unlock_new_place"];
const WORD_XFER: &[&str] = &["download", "install"];
const WORD_MODEL: &[&str] = &["ready", "failed", "changed"];
const WORD_PRINT: &[&str] = &["printer", "scanner"];
const WORD_BY: &[&str] = &["person", "administrator", "policy", "store", "expiry"];
const WORD_ONOFF: &[&str] = &["on", "off"];

/// The catalogue, version 1. Names of the apps and devices in a payload are text the device or app supplied: show them, do not trust them.
pub const CATALOGUE: &[EventDef] = &[
    // connectivity
    ev("connectivity.changed", "events.network", "the network went online, offline, metered or needs a sign-in page", &[("state", F::Word(WORD_NET))], C::Kind, 0),
    ev("vpn.changed", "events.network", "a VPN came up or went down", &[("name", F::Text), ("state", F::Word(WORD_UPDOWN))], C::By("name"), 0),
    ev("peers.changed", "events.network", "a paired computer was paired, forgotten, came online or went offline", &[("device", F::Text), ("change", F::Word(WORD_PEER))], C::No, 30),
    ev("peers.battery", "events.network", "the battery of a paired device changed", &[("device", F::Text), ("percent", F::Int)], C::By("device"), 0),
    // what the app may do: told to the app it is about, always
    implicit(ev("app.permissions", "", "what this app may do changed (names of what was granted and taken away, what it holds now, who changed it, whether it must restart)", &[("app", F::Text), ("granted", F::Text), ("revoked", F::Text), ("dropped", F::Text), ("effective", F::Text), ("held", F::Int), ("by", F::Word(WORD_BY)), ("restart", F::Bool)], C::Merge, 0)),
    // power
    ev("power.source", "events.power", "the machine moved to battery or to mains", &[("source", F::Word(WORD_SRC))], C::Kind, 0),
    ev("power.battery_low", "events.power", "the battery is low", &[("percent", F::Int)], C::Kind, 0),
    ev("power.sleep", "events.power", "the machine goes to sleep or woke up", &[("phase", F::Word(WORD_SLEEP))], C::No, 20),
    ev("power.shutdown_pending", "events.power", "a shutdown or restart is coming", &[("in_seconds", F::Int)], C::Kind, 0),
    ev("thermal.changed", "events.power", "the machine is slowing down because it is hot, or stopped", &[("state", F::Word(WORD_THERM))], C::Kind, 0),
    ev("memory.pressure", "events.power", "memory is getting short", &[("level", F::Word(WORD_MEM))], C::Kind, 0),
    ev("quiet_hours.changed", "events.power", "quiet hours (do not disturb) went on or off", &[("state", F::Word(WORD_ONOFF))], C::Kind, 0),
    // session and security
    ev("session.changed", "events.session", "a person signed in or out, or the screen was locked or unlocked", &[("state", F::Word(WORD_SESSION))], C::No, 30),
    ev("vault.changed", "events.session", "the vault was locked or unlocked", &[("state", F::Word(WORD_LOCK))], C::Kind, 0),
    ev("security.alert", "events.security", "a security alert: a failed sign-in, a new paired device, an unlock from a new place (a count, never who or where)", &[("kind", F::Word(WORD_SEC)), ("count", F::Int)], C::By("kind"), 0),
    ev("cert.expiring", "events.security", "a certificate or key expires soon", &[("what", F::Text), ("days", F::Int)], C::By("what"), 0),
    // display, language
    ev("display.changed", "events.display", "the screen size, scale, theme, skin or font changed", &[("what", F::Word(WORD_DISPLAY))], C::By("what"), 0),
    ev("locale.changed", "events.locale", "the language changed", &[("locale", F::Text)], C::Kind, 0),
    ev("accessibility.changed", "events.locale", "an accessibility setting changed", &[("what", F::Text)], C::By("what"), 0),
    // devices
    ev("audio.devices", "events.devices", "a sound device was added or removed, or the default changed", &[("kind", F::Word(WORD_DEVKIND)), ("change", F::Word(WORD_DEV)), ("device", F::Text)], C::No, 30),
    ev("camera.devices", "events.devices", "a camera was added or removed, or the default changed", &[("change", F::Word(WORD_DEV)), ("device", F::Text)], C::No, 30),
    ev("bluetooth.device", "events.devices", "a Bluetooth device connected or disconnected", &[("device", F::Text), ("change", F::Word(WORD_CONN))], C::No, 30),
    ev("printer.changed", "events.devices", "a printer or scanner was added or removed", &[("kind", F::Word(WORD_PRINT)), ("device", F::Text), ("change", F::Word(WORD_DEV))], C::No, 30),
    ev("disk.health", "events.devices", "a disk reports a health warning, or recovered", &[("disk", F::Text), ("state", F::Word(WORD_HEALTH))], C::By("disk"), 0),
    // storage
    ev("storage.volume", "events.storage", "a volume was mounted or unmounted, or is nearly full", &[("volume", F::Text), ("change", F::Word(WORD_VOL))], C::No, 30),
    ev("backup.finished", "events.storage", "a backup finished or failed", &[("name", F::Text), ("outcome", F::Word(WORD_RUN))], C::No, 20),
    // clock
    ev("clock.changed", "events.clock", "the time zone changed or the clock jumped", &[("change", F::Word(WORD_CLOCK))], C::No, 10),
    ev("clock.midnight", "events.clock", "a new day began", &[], C::Kind, 0),
    timer(ev("clock.minute", "events.clock", "a tick every minute (the core runs it only while an app listens)", &[], C::Kind, 0)),
    timer(ev("clock.hour", "events.clock", "a tick every hour (the core runs it only while an app listens)", &[], C::Kind, 0)),
    timer(ev("reminder.due", "events.reminders", "a reminder or a calendar entry is due", &[("title", F::Text)], C::No, 30)),
    // settings
    filtered(ev("settings.changed", "events.settings", "a setting changed (its name, never its value)", &[("setting", F::Text)], C::By("setting"), 0), "setting"),
    // apps
    ev("app.lifecycle", "events.apps", "an app came to the front or went to the back", &[("app", F::Text), ("change", F::Word(WORD_LIFE))], C::By("app"), 0),
    ev("app.installed", "events.apps", "an app was installed, updated or removed", &[("app", F::Text), ("change", F::Word(WORD_INST))], C::No, 30),
    ev("inbox.pending", "events.inbox", "notifications are waiting (the number)", &[("count", F::Int)], C::Kind, 0),
    ev("approvals.pending", "events.inbox", "approvals are waiting for the person (the number)", &[("count", F::Int)], C::Kind, 0),
    // updates, transfers, flows, models
    ev("updates.changed", "events.updates", "an update is available, downloaded or applied", &[("component", F::Text), ("phase", F::Word(WORD_UPD))], C::No, 20),
    ev("transfer.finished", "events.updates", "a download or an install finished or failed", &[("kind", F::Word(WORD_XFER)), ("name", F::Text), ("outcome", F::Word(WORD_RUN))], C::No, 30),
    ev("flows.run", "events.flows", "a flow or job finished or failed", &[("flow", F::Text), ("outcome", F::Word(WORD_RUN))], C::No, 60),
    ev("models.host", "events.models", "the model host is ready, failed or changed model", &[("state", F::Word(WORD_MODEL)), ("model", F::Text)], C::Kind, 0),
    // files
    filtered(ev("files.changed", "events.files", "a file in a watched folder was created, changed or removed (its name, never its content)", &[("folder", F::Text), ("name", F::Text), ("change", F::Word(WORD_FILE))], C::By("name"), 0), "folder"),
    // voice, only while the experimental wake phrase is on
    flagged(ev("voice.wake_heard", "events.voice", "the wake phrase was heard (experimental)", &[], C::No, 12), "wake_word"),
];

/// A custom event name: `custom.<publisher>.<name>`.
pub fn custom_parts(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("custom.")?;
    let (ns, n) = rest.split_once('.')?;
    let ok = |s: &str| !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok(ns) && ok(n) { Some((ns, n)) } else { None }
}

pub fn lookup(name: &str) -> Option<&'static EventDef> {
    CATALOGUE.iter().find(|d| d.name == name)
}

/// What a custom event is, in the catalogue's terms: rate limited, every event kept.
const CUSTOM: EventDef = EventDef { name: "custom", version: 1, scope: "events.custom", gives: "events other apps publish", fields: &[], coalesce: Coalesce::No, per_min: 60, filter_field: None, needs_flag: None, timer: false, implicit: false };

fn def_of(name: &str) -> Option<&'static EventDef> {
    if custom_parts(name).is_some() { Some(&CUSTOM) } else { lookup(name) }
}

fn clean_text(s: &str) -> Result<(), String> {
    if s.len() > MAX_TEXT || s.chars().any(char::is_control) {
        return Err(format!("a text field is at most {MAX_TEXT} characters of plain text"));
    }
    Ok(())
}

const SECRET_WORDS: &[&str] = &["password", "passwd", "secret", "token", "key", "cookie", "auth", "credential", "pin", "phrase"];

/// A field name of a custom event must not look like a secret.
fn secret_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| n.split(|c: char| !c.is_ascii_alphanumeric()).any(|p| p == *w))
}

/// Check a payload against the catalogue entry (custom events: a few plain fields, none named like a secret).
pub fn check_payload(name: &str, p: &Payload) -> Result<(), String> {
    if custom_parts(name).is_some() {
        if p.len() > MAX_CUSTOM_FIELDS {
            return Err(format!("a custom event has at most {MAX_CUSTOM_FIELDS} fields"));
        }
        for (k, v) in p {
            if k.is_empty() || k.len() > 32 || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err("a field name is up to 32 letters, digits and _".into());
            }
            if secret_like(k) {
                return Err(format!("field {k:?} looks like a secret: events never carry secrets"));
            }
            if let Val::Text(t) = v {
                clean_text(t)?;
            }
        }
        return Ok(());
    }
    let d = lookup(name).ok_or_else(|| format!("there is no event {name:?}"))?;
    for (k, v) in p {
        let Some((_, f)) = d.fields.iter().find(|(n, _)| n == k) else {
            return Err(format!("{name}: no field {k:?}"));
        };
        let ok = match (f, v) {
            (F::Bool, Val::Bool(_)) | (F::Int, Val::Int(_)) => true,
            (F::Text, Val::Text(t)) => clean_text(t).is_ok(),
            (F::Word(ws), Val::Text(t)) => ws.contains(&t.as_str()),
            _ => false,
        };
        if !ok {
            return Err(format!("{name}: field {k:?} has the wrong type or an unknown word"));
        }
    }
    for (k, _) in d.fields {
        if !p.contains_key(*k) {
            return Err(format!("{name}: field {k:?} is missing"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ what apps see

/// One event as the app gets it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEvent {
    /// Per app, only grows, never reused (also across restarts).
    pub seq: u64,
    pub at_ms: u64,
    pub name: String,
    pub version: u32,
    /// It happened while the app was not running (or the machine slept): handle it as news that may be old.
    pub late: bool,
    /// Delivered before and handed out again by a replay.
    pub replayed: bool,
    pub payload: Payload,
}

impl AppEvent {
    pub fn is_missed(&self) -> bool {
        self.name == MISSED
    }
    /// For a `missed` marker: how many events were dropped.
    pub fn missed_count(&self) -> u64 {
        self.payload.get("count").and_then(Val::int).unwrap_or(0).max(0) as u64
    }
}

/// One registration as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegInfo {
    pub event: String,
    /// A settings key prefix or a folder, else empty.
    pub filter: String,
    /// The person's switch: off means nothing is queued or delivered.
    pub enabled: bool,
    pub scope: String,
    pub gives: String,
    /// How many events wait for the app now.
    pub pending: usize,
}

/// One catalogue entry as a tool or a pane lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventInfo {
    pub name: String,
    pub version: u32,
    pub scope: String,
    pub gives: String,
    pub fields: Vec<String>,
    pub coalesces: bool,
    pub timer: bool,
}

pub fn catalogue_info() -> Vec<EventInfo> {
    CATALOGUE
        .iter()
        .map(|d| EventInfo { name: d.name.into(), version: d.version, scope: d.scope.into(), gives: d.gives.into(), fields: d.fields.iter().map(|(n, _)| n.to_string()).collect(), coalesces: d.coalesce != Coalesce::No, timer: d.timer })
        .collect()
}

// ------------------------------------------------------------------------------------------------ the facet

/// What the core asks about an app's permissions (the capability records and the file policy).
pub trait ScopeCheck: Send + Sync {
    fn holds(&self, user: &str, app: &str, scope: &str) -> bool;
    /// Whether the app may read this folder (the file policy); events about a folder it may not read are never delivered.
    fn folder_allowed(&self, _user: &str, _app: &str, _folder: &str) -> bool {
        true
    }
    /// The names of everything the app holds now (capabilities and scopes), for the effective set an app is told.
    fn held(&self, _user: &str, _app: &str) -> Vec<String> {
        Vec::new()
    }
}

pub trait AppEvents: Send + Sync {
    fn catalogue(&self) -> Vec<EventInfo> {
        Vec::new()
    }
    fn list(&self, _user: &str, _app: &str) -> Vec<RegInfo> {
        Vec::new()
    }
    /// Every app of the person that has registrations (Settings > Apps).
    fn apps(&self, _user: &str) -> Vec<(String, Vec<RegInfo>)> {
        Vec::new()
    }
    fn register(&self, _user: &str, _app: &str, _event: &str, _filter: &str) -> Result<(), String> {
        Err(NO_EVENTS.into())
    }
    fn unregister(&self, _user: &str, _app: &str, _event: &str, _filter: &str) -> Result<(), String> {
        Err(NO_EVENTS.into())
    }
    /// The person's switch for one registration.
    fn set_enabled(&self, _user: &str, _app: &str, _event: &str, _filter: &str, _on: bool) -> Result<(), String> {
        Err(NO_EVENTS.into())
    }
    /// The app starts: everything that waited (a `missed` marker first, then the events, `late` as recorded), in order.
    fn start(&self, _user: &str, _app: &str) -> Vec<AppEvent> {
        Vec::new()
    }
    /// The app stops (or the machine sleeps): what happens from now on waits and is marked `late`.
    fn stop(&self, _user: &str, _app: &str) {}
    /// Events since the last call, in order.
    fn poll(&self, _user: &str, _app: &str, _max: usize) -> Vec<AppEvent> {
        Vec::new()
    }
    /// The delivered events after this sequence number (at most [`MAX_HISTORY`] are kept), marked `replayed`.
    fn replay(&self, _user: &str, _app: &str, _after_seq: u64) -> Vec<AppEvent> {
        Vec::new()
    }
    /// The person (or a tool for the person) asks that the delivered events after this number wait for the app again: they come with its next
    /// poll, marked `late` and `replayed`. Returns how many.
    fn requeue(&self, _user: &str, _app: &str, _after_seq: u64) -> Result<usize, String> {
        Err(NO_EVENTS.into())
    }
    /// What an app may do changed (a core service that changes it says so here): registrations of events the app may no longer hear are
    /// dropped, and the app is told, whether or not it runs, with the names granted and taken away, what it holds now, who changed it
    /// (`person`, `administrator`, `policy`, `store`, `expiry`) and whether it must restart. Returns how many apps were told.
    fn permissions_changed(&self, _user: &str, _app: &str, _granted: &[String], _revoked: &[String], _by: &str, _restart: bool) -> Result<usize, String> {
        Err(NO_EVENTS.into())
    }
    /// The person takes one permission away from an app (`allow` false) or gives it back (true), by name; the app is told at once.
    fn set_permission(&self, _user: &str, _app: &str, _scope: &str, _allow: bool, _by: &str) -> Result<usize, String> {
        Err(NO_EVENTS.into())
    }
    /// What the app holds now, and the last change told to it as one line (names only), for Settings and the chat tool.
    fn permissions(&self, _user: &str, _app: &str) -> (Vec<String>, Option<String>) {
        (Vec::new(), None)
    }
    /// The apps of the person that have an event state (a registration or a permission change).
    fn known_apps(&self, _user: &str) -> Vec<String> {
        Vec::new()
    }
    /// An app publishes a custom event `custom.<its own id>.<name>`; needs `events.publish`.
    fn publish(&self, _user: &str, _app: &str, _name: &str, _payload: Payload) -> Result<usize, String> {
        Err(NO_EVENTS.into())
    }
    /// A core service reports what happened (`user` = everybody). Returns how many apps it was queued for. An app-facing client cannot call this.
    fn emit(&self, _name: &str, _user: Option<&str>, _payload: Payload) -> Result<usize, String> {
        Err(NO_EVENTS.into())
    }
    /// Who listens for this event: `(person, app, filter)` of every enabled registration (a source that watches something, such as a folder,
    /// watches only what somebody listens to).
    fn listeners(&self, _event: &str) -> Vec<(String, String, String)> {
        Vec::new()
    }
    /// Whether anybody listens for this event (a timer runs only then).
    fn wanted(&self, _name: &str) -> bool {
        false
    }
    /// A switch the core's services set (for example `wake_word`).
    fn set_flag(&self, _flag: &str, _on: bool) {}
    /// The newest audit lines (names and numbers only).
    fn audit(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Default)]
pub struct NoAppEvents;
impl AppEvents for NoAppEvents {}

// ------------------------------------------------------------------------------------------------ from the core's own events

/// What the core's pushed events (F-CORE-18) become for apps: the app layer is a view of the same log, not a second bus. A setting
/// event keeps the key and drops the value (and the name of who changed it).
pub fn from_setting_change(key: &str) -> (&'static str, Payload) {
    ("settings.changed", payload(&[("setting", Val::Text(key.chars().take(MAX_TEXT).collect()))]))
}

/// A flow run that ended (`ok` false = failed or stopped); the flow's name, nothing of its steps or data.
pub fn from_flow_end(flow: &str, ok: bool) -> (&'static str, Payload) {
    ("flows.run", payload(&[("flow", Val::Text(flow.chars().take(MAX_TEXT).collect())), ("outcome", Val::Text(if ok { "finished" } else { "failed" }.into()))]))
}

/// The inbox and the approvals waiting: counts only.
pub fn from_inbox(notifications: usize, approvals: usize) -> [(&'static str, Payload); 2] {
    [("inbox.pending", payload(&[("count", Val::Int(notifications as i64))])), ("approvals.pending", payload(&[("count", Val::Int(approvals as i64))]))]
}

// ------------------------------------------------------------------------------------------------ the engine

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Reg {
    event: String,
    filter: String,
    enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Queued {
    name: String,
    at_ms: u64,
    late: bool,
    payload: Payload,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Missed {
    count: u64,
    first_ms: u64,
    last_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AppState {
    regs: Vec<Reg>,
    queue: VecDeque<Queued>,
    missed: Missed,
    next_seq: u64,
    history: VecDeque<AppEvent>,
    /// The last permission change told to the app (when, and what), for Settings and the chat tool.
    #[serde(default)]
    last_permissions: Option<(u64, Payload)>,
    /// Scopes the person took away from this app (a narrowing of what it holds; giving it back removes the name).
    #[serde(default)]
    denied: Vec<String>,
    #[serde(skip)]
    running: bool,
    /// When the app last started or polled (the app that stopped asking is away, whatever it said).
    #[serde(skip)]
    last_poll: u64,
    /// event name -> (window start, accepted in the window)
    #[serde(skip)]
    window: BTreeMap<String, (u64, u32)>,
}

/// Where the states are kept so they survive a restart: one text (JSON) per `user/app`.
pub trait EventStore: Send {
    fn load_all(&self) -> Vec<(String, String)>;
    fn save(&self, key: &str, json: &str);
}

#[derive(Default, Clone)]
pub struct MemStore(pub std::sync::Arc<Mutex<BTreeMap<String, String>>>);
impl EventStore for MemStore {
    fn load_all(&self) -> Vec<(String, String)> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
    fn save(&self, key: &str, json: &str) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key.into(), json.into());
    }
}

fn key(user: &str, app: &str) -> String {
    format!("{user}/{app}")
}

fn matches(def: &EventDef, r: &Reg, p: &Payload) -> bool {
    let Some(f) = def.filter_field else { return true };
    if r.filter.is_empty() {
        return true;
    }
    let Some(v) = p.get(f).and_then(Val::text) else { return false };
    if f == "folder" { v == r.filter || v.starts_with(&format!("{}/", r.filter.trim_end_matches('/'))) } else { v.starts_with(&r.filter) }
}

pub struct EventHub {
    states: BTreeMap<String, AppState>,
    store: Box<dyn EventStore>,
    scopes: Box<dyn ScopeCheck>,
    flags: Vec<String>,
    audit: VecDeque<String>,
}

impl EventHub {
    pub fn new(store: Box<dyn EventStore>, scopes: Box<dyn ScopeCheck>) -> EventHub {
        let mut states = BTreeMap::new();
        for (k, json) in store.load_all() {
            if let Ok(s) = serde_json::from_str::<AppState>(&json) {
                states.insert(k, s);
            }
        }
        EventHub { states, store, scopes, flags: Vec::new(), audit: VecDeque::new() }
    }

    fn log(&mut self, line: String) {
        self.audit.push_back(line);
        while self.audit.len() > 200 {
            self.audit.pop_front();
        }
    }
    pub fn audit(&self) -> Vec<String> {
        self.audit.iter().cloned().collect()
    }
    fn save(&self, k: &str) {
        if let Some(s) = self.states.get(k) {
            if let Ok(j) = serde_json::to_string(s) {
                self.store.save(k, &j);
            }
        }
    }
    pub fn set_flag(&mut self, flag: &str, on: bool) {
        self.flags.retain(|f| f != flag);
        if on {
            self.flags.push(flag.into());
        }
    }

    pub fn register(&mut self, user: &str, app: &str, event: &str, filter: &str) -> Result<(), String> {
        let refuse = |this: &mut EventHub, why: String| {
            this.log(format!("appevents.refuse {app}: register {event}: {why}"));
            Err(why)
        };
        let Some(d) = def_of(event) else {
            return refuse(self, format!("there is no event {event:?}: the catalogue lists them"));
        };
        if d.implicit {
            return Ok(()); // always heard: nothing to register and nothing the person could switch off
        }
        if !self.holds(user, app, d.scope) {
            return refuse(self, format!("{app} does not hold {}: the person grants it first ({})", d.scope, d.gives));
        }
        if let Some(fl) = d.needs_flag {
            if !self.flags.iter().any(|f| f == fl) {
                return refuse(self, format!("{event} is off: the person has not switched {fl} on"));
            }
        }
        if !filter.is_empty() {
            if d.filter_field.is_none() {
                return refuse(self, format!("{event} takes no prefix"));
            }
            if filter.len() > 200 || filter.chars().any(char::is_control) || filter.split('/').any(|s| s == "..") {
                return refuse(self, "the prefix is plain text without ..".into());
            }
            if d.filter_field == Some("folder") {
                if let Err(e) = crate::files::check_rel(filter) {
                    return refuse(self, e);
                }
            }
            if d.filter_field == Some("folder") && !self.scopes.folder_allowed(user, app, filter) {
                return refuse(self, "the file policy does not let this app read that folder".into());
            }
        } else if d.filter_field == Some("folder") {
            return refuse(self, "files.changed needs the folder to watch".into());
        }
        let k = key(user, app);
        let st = self.states.entry(k.clone()).or_default();
        if st.regs.iter().any(|r| r.event == event && r.filter == filter) {
            return Ok(());
        }
        if st.regs.len() >= MAX_REGISTRATIONS {
            return refuse(self, format!("at most {MAX_REGISTRATIONS} registrations for one app"));
        }
        st.regs.push(Reg { event: event.into(), filter: filter.into(), enabled: true });
        self.log(format!("appevents.register {app}: {event}"));
        self.save(&k);
        Ok(())
    }

    pub fn unregister(&mut self, user: &str, app: &str, event: &str, filter: &str) -> Result<(), String> {
        if def_of(event).is_some_and(|d| d.implicit) {
            return Err(ALWAYS_ON.into());
        }
        let k = key(user, app);
        let st = self.states.get_mut(&k).ok_or_else(|| NOT_REGISTERED.to_string())?;
        let n = st.regs.len();
        st.regs.retain(|r| !(r.event == event && r.filter == filter));
        if st.regs.len() == n {
            return Err(NOT_REGISTERED.into());
        }
        // what waited for it is of no use any more
        st.queue.retain(|q| q.name != event);
        self.log(format!("appevents.unregister {app}: {event}"));
        self.save(&k);
        Ok(())
    }

    pub fn set_enabled(&mut self, user: &str, app: &str, event: &str, filter: &str, on: bool) -> Result<(), String> {
        if def_of(event).is_some_and(|d| d.implicit) {
            return Err(ALWAYS_ON.into());
        }
        let k = key(user, app);
        let st = self.states.get_mut(&k).ok_or_else(|| NOT_REGISTERED.to_string())?;
        let r = st.regs.iter_mut().find(|r| r.event == event && r.filter == filter).ok_or_else(|| NOT_REGISTERED.to_string())?;
        r.enabled = on;
        if !on {
            st.queue.retain(|q| q.name != event);
        }
        self.log(format!("appevents.switch {app}: {event} {}", if on { "on" } else { "off" }));
        self.save(&k);
        Ok(())
    }

    pub fn list(&self, user: &str, app: &str) -> Vec<RegInfo> {
        let Some(st) = self.states.get(&key(user, app)) else { return Vec::new() };
        st.regs
            .iter()
            .map(|r| {
                let d = def_of(&r.event);
                RegInfo {
                    event: r.event.clone(),
                    filter: r.filter.clone(),
                    enabled: r.enabled,
                    scope: d.map(|d| d.scope.to_string()).unwrap_or_default(),
                    gives: d.map(|d| d.gives.to_string()).unwrap_or_default(),
                    pending: st.queue.iter().filter(|q| q.name == r.event).count(),
                }
            })
            .collect()
    }

    pub fn apps(&self, user: &str) -> Vec<(String, Vec<RegInfo>)> {
        let pre = format!("{user}/");
        self.states.keys().filter_map(|k| k.strip_prefix(&pre)).filter(|a| !self.states[&key(user, a)].regs.is_empty()).map(|a| (a.to_string(), self.list(user, a))).collect()
    }

    pub fn listeners(&self, event: &str) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (k, s) in &self.states {
            let Some((u, a)) = k.split_once('/') else { continue };
            for r in s.regs.iter().filter(|r| r.enabled && r.event == event) {
                out.push((u.to_string(), a.to_string(), r.filter.clone()));
            }
        }
        out
    }

    /// Whether any app has an enabled registration (a source that is looked at on a clock of its own looks only then).
    pub fn any_registrations(&self) -> bool {
        self.states.values().any(|s| s.regs.iter().any(|r| r.enabled))
    }

    pub fn wanted(&self, name: &str) -> bool {
        self.states.values().any(|s| s.regs.iter().any(|r| r.enabled && r.event == name))
    }

    pub fn set_running(&mut self, user: &str, app: &str, on: bool) {
        if let Some(s) = self.states.get_mut(&key(user, app)) {
            s.running = on;
        }
    }

    /// A service reports an event: it is queued for every app that registered, holds the scope and has not switched it off.
    pub fn emit(&mut self, name: &str, user: Option<&str>, p: Payload, now: u64) -> Result<usize, String> {
        check_payload(name, &p)?;
        let d = def_of(name).ok_or_else(|| format!("there is no event {name:?}"))?;
        if let Some(fl) = d.needs_flag {
            if !self.flags.iter().any(|f| f == fl) {
                return Ok(0);
            }
        }
        if d.implicit {
            return self.emit_implicit(d, name, user, p, now);
        }
        let targets: Vec<String> = self.states.iter().filter(|(k, s)| user.is_none_or(|u| k.starts_with(&format!("{u}/"))) && s.regs.iter().any(|r| r.enabled && r.event == name && matches(d, r, &p))).map(|(k, _)| k.clone()).collect();
        let mut n = 0;
        for k in targets {
            let (u, a) = k.split_once('/').map(|(u, a)| (u.to_string(), a.to_string())).unwrap_or_default();
            if !self.holds(&u, &a, d.scope) {
                self.log(format!("appevents.refuse {a}: {name}: scope {} no longer held", d.scope));
                continue;
            }
            if d.filter_field == Some("folder") {
                let folder = p.get("folder").and_then(Val::text).unwrap_or("");
                if !self.scopes.folder_allowed(&u, &a, folder) {
                    self.log(format!("appevents.refuse {a}: {name}: file policy"));
                    continue;
                }
            }
            if self.enqueue(&k, d, name, &p, now) {
                n += 1;
            }
            self.save(&k);
        }
        Ok(n)
    }

    /// An event about one app, for that app only: its state is made when it has none, no scope is needed and no registration.
    fn emit_implicit(&mut self, d: &EventDef, name: &str, user: Option<&str>, p: Payload, now: u64) -> Result<usize, String> {
        let app = p.get("app").and_then(Val::text).ok_or("app.permissions names its app")?.to_string();
        let mut keys: Vec<String> = self.states.keys().filter(|k| k.rsplit_once('/').is_some_and(|(u, a)| a == app && user.is_none_or(|x| x == u))).cloned().collect();
        if let Some(u) = user {
            let k = key(u, &app);
            if !keys.contains(&k) {
                self.states.entry(k.clone()).or_default();
                keys.push(k);
            }
        }
        let mut n = 0;
        for k in keys {
            if let Some(st) = self.states.get_mut(&k) {
                st.last_permissions = Some((now, p.clone()));
            }
            if self.enqueue(&k, d, name, &p, now) {
                n += 1;
            }
            self.save(&k);
        }
        Ok(n)
    }

    /// Queue one event for one app; false when it was dropped by the rate limit.
    fn enqueue(&mut self, k: &str, d: &EventDef, name: &str, p: &Payload, now: u64) -> bool {
        let app = k.split_once('/').map(|x| x.1).unwrap_or("").to_string();
        let st = self.states.get_mut(k).expect("target exists");
        let late = !(st.running && now.saturating_sub(st.last_poll) <= AWAY_AFTER_MS);
        let mut note = None;
        let mut accepted = true;
        let mut merged: Option<Payload> = None;
        if d.coalesce == Coalesce::No && d.per_min > 0 {
            let w = st.window.entry(name.to_string()).or_insert((now, 0));
            if now.saturating_sub(w.0) >= 60_000 {
                *w = (now, 0);
            }
            if w.1 >= d.per_min {
                add_missed(&mut st.missed, now);
                accepted = false;
                note = Some(format!("appevents.drop {app}: {name}: rate limit {} a minute", d.per_min));
            } else {
                w.1 += 1;
            }
        }
        if accepted {
            match d.coalesce {
                Coalesce::No => {}
                Coalesce::Merge => {
                    // the latest effective set; what was granted and taken away is added up (a change undone before the app heard it is not told)
                    if let Some(i) = st.queue.iter().position(|q| q.name == name) {
                        let old = st.queue.remove(i).map(|q| q.payload).unwrap_or_default();
                        merged = Some(merge_permissions(&old, p));
                    }
                }
                Coalesce::Kind => st.queue.retain(|q| q.name != name),
                Coalesce::By(f) => st.queue.retain(|q| !(q.name == name && q.payload.get(f) == p.get(f))),
            }
            st.queue.push_back(Queued { name: name.into(), at_ms: now, late, payload: merged.unwrap_or_else(|| p.clone()) });
            if st.queue.len() > MAX_QUEUE {
                st.queue.pop_front();
                add_missed(&mut st.missed, now);
                note = Some(format!("appevents.drop {app}: queue full, the oldest was dropped"));
            }
        }
        if let Some(n) = note {
            self.log(n);
        }
        accepted
    }

    fn drain(&mut self, user: &str, app: &str, max: usize, now: u64) -> Vec<AppEvent> {
        let k = key(user, app);
        let Some(st) = self.states.get_mut(&k) else { return Vec::new() };
        // age
        while st.queue.front().is_some_and(|q| now.saturating_sub(q.at_ms) > MAX_AGE_MS) {
            st.queue.pop_front();
            add_missed(&mut st.missed, now);
        }
        let mut out = Vec::new();
        if st.missed.count > 0 && max > 0 {
            st.next_seq += 1;
            let m = std::mem::take(&mut st.missed);
            let p = payload(&[("count", Val::Int(m.count as i64)), ("first_at_ms", Val::Int(m.first_ms as i64)), ("last_at_ms", Val::Int(m.last_ms as i64))]);
            out.push(AppEvent { seq: st.next_seq, at_ms: now, name: MISSED.into(), version: 1, late: true, replayed: false, payload: p });
        }
        while out.len() < max {
            let Some(q) = st.queue.pop_front() else { break };
            // a registration switched off or removed while the event waited
            if !def_of(&q.name).is_some_and(|d| d.implicit) && !st.regs.iter().any(|r| r.enabled && r.event == q.name) {
                continue;
            }
            st.next_seq += 1;
            let version = def_of(&q.name).map_or(1, |d| d.version);
            out.push(AppEvent { seq: st.next_seq, at_ms: q.at_ms, name: q.name, version, late: q.late || now.saturating_sub(q.at_ms) > AWAY_AFTER_MS, replayed: false, payload: q.payload });
        }
        for e in &out {
            st.history.push_back(e.clone());
        }
        while st.history.len() > MAX_HISTORY {
            st.history.pop_front();
        }
        let lines: Vec<String> = out.iter().map(|e| format!("appevents.deliver {app}: {} #{}{}", e.name, e.seq, if e.late { " late" } else { "" })).collect();
        for l in lines {
            self.log(l);
        }
        self.save(&k);
        out
    }

    pub fn start(&mut self, user: &str, app: &str, now: u64) -> Vec<AppEvent> {
        if let Some(s) = self.states.get_mut(&key(user, app)) {
            s.last_poll = now;
        }
        self.set_running(user, app, true);
        self.drain(user, app, MAX_QUEUE + 1, now)
    }
    pub fn poll(&mut self, user: &str, app: &str, max: usize, now: u64) -> Vec<AppEvent> {
        if let Some(s) = self.states.get_mut(&key(user, app)) {
            s.last_poll = now;
        }
        self.drain(user, app, max, now)
    }

    pub fn replay(&mut self, user: &str, app: &str, after: u64) -> Vec<AppEvent> {
        let out: Vec<AppEvent> = self.states.get(&key(user, app)).map(|s| s.history.iter().filter(|e| e.seq > after).cloned().map(|mut e| {
            e.replayed = true;
            e
        }).collect()).unwrap_or_default();
        self.log(format!("appevents.replay {app}: {} events", out.len()));
        out
    }

    /// Whether the app holds the scope: the core's records say so and the person has not taken it away.
    fn holds(&self, user: &str, app: &str, scope: &str) -> bool {
        self.scopes.holds(user, app, scope) && !self.states.get(&key(user, app)).is_some_and(|s| s.denied.iter().any(|d| d == scope))
    }

    /// What the app holds now: the core's records without what the person took away.
    fn held(&self, user: &str, app: &str) -> Vec<String> {
        let denied = self.states.get(&key(user, app)).map(|s| s.denied.clone()).unwrap_or_default();
        self.scopes.held(user, app).into_iter().filter(|n| !denied.contains(n)).collect()
    }

    /// The person takes a permission away from an app, or gives it back (it cannot give more than the core's records hold). The app is told
    /// at once, running or not, and the registrations that needed the scope end.
    pub fn set_permission(&mut self, user: &str, app: &str, scope: &str, allow: bool, by: &str, now: u64) -> Result<usize, String> {
        if crate::appcaps::lookup(scope).is_none() {
            return Err(format!("there is no permission called {scope:?}"));
        }
        let k = key(user, app);
        let st = self.states.entry(k.clone()).or_default();
        let had = st.denied.iter().any(|d| d == scope);
        if allow == !had {
            return Err(if allow { format!("{app} was not stopped from using {scope}") } else { format!("{app} is already stopped from using {scope}") });
        }
        if allow {
            st.denied.retain(|d| d != scope);
        } else {
            st.denied.push(scope.into());
        }
        self.save(&k);
        let one = vec![scope.to_string()];
        if allow {
            self.permissions_changed(user, app, &one, &[], by, false, now)
        } else {
            self.permissions_changed(user, app, &[], &one, by, false, now)
        }
    }

    /// Drop the registrations the app may not keep now: a scope it no longer holds, a folder it may no longer read. The names that went.
    pub fn prune(&mut self, user: &str, app: &str) -> Vec<String> {
        let k = key(user, app);
        let Some(st) = self.states.get(&k) else { return Vec::new() };
        let keep: Vec<bool> = st.regs.iter().map(|r| def_of(&r.event).is_some_and(|d| self.holds(user, app, d.scope) && (d.filter_field != Some("folder") || self.scopes.folder_allowed(user, app, &r.filter)))).collect();
        let st = self.states.get_mut(&k).expect("checked");
        let mut dropped = Vec::new();
        let mut i = 0;
        st.regs.retain(|r| {
            let kept = keep[i];
            i += 1;
            if !kept && !dropped.contains(&r.event) {
                dropped.push(r.event.clone());
            }
            kept
        });
        let names = dropped.clone();
        st.queue.retain(|q| !names.contains(&q.name));
        if !dropped.is_empty() {
            self.log(format!("appevents.drop {app}: {} registrations ended with the permission", dropped.len()));
            self.save(&k);
        }
        dropped
    }

    pub fn permissions_changed(&mut self, user: &str, app: &str, granted: &[String], revoked: &[String], by: &str, restart: bool, now: u64) -> Result<usize, String> {
        let dropped = self.prune(user, app);
        let mut held = self.held(user, app);
        held.sort();
        let p = payload(&[("app", Val::Text(app.chars().take(MAX_TEXT).collect())), ("granted", Val::Text(names_text(granted))), ("revoked", Val::Text(names_text(revoked))), ("dropped", Val::Text(names_text(&dropped))), ("effective", Val::Text(names_text(&held))), ("held", Val::Int(held.len() as i64)), ("by", Val::Text(by.into())), ("restart", Val::Bool(restart))]);
        self.log(format!("appevents.permissions {app}: {} granted, {} revoked, {} dropped, by {by}", granted.len(), revoked.len(), dropped.len()));
        self.emit("app.permissions", Some(user), p, now)
    }

    pub fn permissions(&self, user: &str, app: &str) -> (Vec<String>, Option<String>) {
        let mut held = self.held(user, app);
        held.sort();
        let last = self.states.get(&key(user, app)).and_then(|s| s.last_permissions.as_ref()).map(|(at, p)| {
            let t = |f: &str| p.get(f).and_then(Val::text).unwrap_or("").to_string();
            format!("at {at} by {}: granted [{}], taken away [{}], registrations dropped [{}]{}", t("by"), t("granted"), t("revoked"), t("dropped"), if matches!(p.get("restart"), Some(Val::Bool(true))) { ", restart needed" } else { "" })
        });
        (held, last)
    }

    pub fn known_apps(&self, user: &str) -> Vec<String> {
        let pre = format!("{user}/");
        self.states.iter().filter(|(k, s)| k.starts_with(&pre) && (!s.regs.is_empty() || s.last_permissions.is_some())).map(|(k, _)| k[pre.len()..].to_string()).collect()
    }

    pub fn requeue(&mut self, user: &str, app: &str, after: u64) -> Result<usize, String> {
        let k = key(user, app);
        let st = self.states.get_mut(&k).ok_or_else(|| NOT_REGISTERED.to_string())?;
        let again: Vec<Queued> = st.history.iter().filter(|e| e.seq > after && !e.is_missed() && st.regs.iter().any(|r| r.enabled && r.event == e.name)).map(|e| Queued { name: e.name.clone(), at_ms: e.at_ms, late: true, payload: e.payload.clone() }).collect();
        let n = again.len();
        // they go in front: they are older than what waits now
        for q in again.into_iter().rev() {
            st.queue.push_front(q);
        }
        while st.queue.len() > MAX_QUEUE {
            st.queue.pop_back();
        }
        self.log(format!("appevents.requeue {app}: {n} events"));
        self.save(&k);
        Ok(n)
    }

    pub fn publish(&mut self, user: &str, app: &str, name: &str, p: Payload, now: u64) -> Result<usize, String> {
        let refuse = |this: &mut EventHub, why: String| {
            this.log(format!("appevents.refuse {app}: publish {name}: {why}"));
            Err(why)
        };
        if !self.holds(user, app, "events.publish") {
            return refuse(self, format!("{app} does not hold events.publish"));
        }
        let Some((ns, _)) = custom_parts(name) else {
            return refuse(self, "a custom event is called custom.<your app>.<name>".into());
        };
        let own: String = app.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        if ns != own {
            return refuse(self, format!("an app publishes only under its own name: custom.{own}.<name>"));
        }
        if let Err(e) = check_payload(name, &p) {
            return refuse(self, e);
        }
        self.emit(name, Some(user), p, now)
    }
}

/// Names as one text of at most [`MAX_TEXT`] characters: cut at a comma with `+N` for what did not fit.
pub fn names_text(names: &[String]) -> String {
    let mut out = String::new();
    for (i, n) in names.iter().enumerate() {
        let next = if out.is_empty() { n.clone() } else { format!("{out},{n}") };
        if next.len() + 6 > MAX_TEXT && i + 1 < names.len() || next.len() > MAX_TEXT {
            return format!("{out},+{}", names.len() - i);
        }
        out = next;
    }
    out
}

fn split_names(t: Option<&Val>) -> BTreeSet<String> {
    t.and_then(Val::text).map(|s| s.split(',').filter(|x| !x.is_empty() && !x.starts_with('+')).map(str::to_string).collect()).unwrap_or_default()
}

/// Two permission changes as one: the newer effective set, the changes added up (granted then taken away is no change), the newer `by`,
/// a restart if either needs one.
pub fn merge_permissions(old: &Payload, new: &Payload) -> Payload {
    let (og, or, ng, nr) = (split_names(old.get("granted")), split_names(old.get("revoked")), split_names(new.get("granted")), split_names(new.get("revoked")));
    // what the app has not heard yet: a grant taken away again, or a revoke granted again, is no change for it
    let granted: BTreeSet<String> = og.difference(&nr).chain(ng.iter()).cloned().collect();
    let revoked: BTreeSet<String> = or.difference(&ng).chain(nr.difference(&og)).cloned().collect();
    let dropped: BTreeSet<String> = split_names(old.get("dropped")).union(&split_names(new.get("dropped"))).cloned().collect();
    let restart = matches!(old.get("restart"), Some(Val::Bool(true))) || matches!(new.get("restart"), Some(Val::Bool(true)));
    let mut out = new.clone();
    out.insert("granted".into(), Val::Text(names_text(&granted.into_iter().collect::<Vec<_>>())));
    out.insert("revoked".into(), Val::Text(names_text(&revoked.into_iter().collect::<Vec<_>>())));
    out.insert("dropped".into(), Val::Text(names_text(&dropped.into_iter().collect::<Vec<_>>())));
    out.insert("restart".into(), Val::Bool(restart));
    out
}

fn add_missed(m: &mut Missed, now: u64) {
    if m.count == 0 {
        m.first_ms = now;
    }
    m.last_ms = now;
    m.count += 1;
}

// ------------------------------------------------------------------------------------------------ the shared facet

/// The engine behind a lock and a clock: the fake core and the local core both use it, so their rules cannot drift.
pub struct SharedHub {
    hub: Mutex<EventHub>,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl SharedHub {
    pub fn new(store: Box<dyn EventStore>, scopes: Box<dyn ScopeCheck>, clock: Box<dyn Fn() -> u64 + Send + Sync>) -> SharedHub {
        SharedHub { hub: Mutex::new(EventHub::new(store, scopes)), clock }
    }
    /// Whether any app has an enabled registration.
    pub fn any_registrations(&self) -> bool {
        self.h().any_registrations()
    }
    fn h(&self) -> std::sync::MutexGuard<'_, EventHub> {
        self.hub.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn now(&self) -> u64 {
        (self.clock)()
    }
}

impl AppEvents for SharedHub {
    fn catalogue(&self) -> Vec<EventInfo> {
        catalogue_info()
    }
    fn list(&self, user: &str, app: &str) -> Vec<RegInfo> {
        self.h().list(user, app)
    }
    fn apps(&self, user: &str) -> Vec<(String, Vec<RegInfo>)> {
        self.h().apps(user)
    }
    fn register(&self, user: &str, app: &str, event: &str, filter: &str) -> Result<(), String> {
        self.h().register(user, app, event, filter)
    }
    fn unregister(&self, user: &str, app: &str, event: &str, filter: &str) -> Result<(), String> {
        self.h().unregister(user, app, event, filter)
    }
    fn set_enabled(&self, user: &str, app: &str, event: &str, filter: &str, on: bool) -> Result<(), String> {
        self.h().set_enabled(user, app, event, filter, on)
    }
    fn start(&self, user: &str, app: &str) -> Vec<AppEvent> {
        let now = self.now();
        self.h().start(user, app, now)
    }
    fn stop(&self, user: &str, app: &str) {
        self.h().set_running(user, app, false)
    }
    fn poll(&self, user: &str, app: &str, max: usize) -> Vec<AppEvent> {
        let now = self.now();
        self.h().poll(user, app, max, now)
    }
    fn replay(&self, user: &str, app: &str, after_seq: u64) -> Vec<AppEvent> {
        self.h().replay(user, app, after_seq)
    }
    fn requeue(&self, user: &str, app: &str, after_seq: u64) -> Result<usize, String> {
        self.h().requeue(user, app, after_seq)
    }
    fn permissions_changed(&self, user: &str, app: &str, granted: &[String], revoked: &[String], by: &str, restart: bool) -> Result<usize, String> {
        let now = self.now();
        self.h().permissions_changed(user, app, granted, revoked, by, restart, now)
    }
    fn set_permission(&self, user: &str, app: &str, scope: &str, allow: bool, by: &str) -> Result<usize, String> {
        let now = self.now();
        self.h().set_permission(user, app, scope, allow, by, now)
    }
    fn permissions(&self, user: &str, app: &str) -> (Vec<String>, Option<String>) {
        self.h().permissions(user, app)
    }
    fn known_apps(&self, user: &str) -> Vec<String> {
        self.h().known_apps(user)
    }
    fn publish(&self, user: &str, app: &str, name: &str, p: Payload) -> Result<usize, String> {
        let now = self.now();
        self.h().publish(user, app, name, p, now)
    }
    fn emit(&self, name: &str, user: Option<&str>, p: Payload) -> Result<usize, String> {
        let now = self.now();
        self.h().emit(name, user, p, now)
    }
    fn wanted(&self, name: &str) -> bool {
        self.h().wanted(name)
    }
    fn listeners(&self, event: &str) -> Vec<(String, String, String)> {
        self.h().listeners(event)
    }
    fn set_flag(&self, flag: &str, on: bool) {
        self.h().set_flag(flag, on)
    }
    fn audit(&self) -> Vec<String> {
        self.h().audit()
    }
}

/// A scope check from a closure (the local core wraps its capability records in one).
pub struct FnScopes<F: Fn(&str, &str, &str) -> bool + Send + Sync>(pub F);
impl<F: Fn(&str, &str, &str) -> bool + Send + Sync> ScopeCheck for FnScopes<F> {
    fn holds(&self, user: &str, app: &str, scope: &str) -> bool {
        (self.0)(user, app, scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    struct Scopes(Arc<Mutex<Vec<String>>>, Vec<String>);
    impl ScopeCheck for Scopes {
        fn holds(&self, _u: &str, app: &str, scope: &str) -> bool {
            self.0.lock().unwrap().iter().any(|s| s == &format!("{app}:{scope}"))
        }
        fn folder_allowed(&self, _u: &str, _a: &str, folder: &str) -> bool {
            self.1.iter().any(|f| folder.starts_with(f.as_str()))
        }
        fn held(&self, _u: &str, app: &str) -> Vec<String> {
            self.0.lock().unwrap().iter().filter_map(|s| s.strip_prefix(&format!("{app}:")).map(str::to_string)).collect()
        }
    }

    struct T {
        hub: SharedHub,
        clock: Arc<AtomicU64>,
        held: Arc<Mutex<Vec<String>>>,
        store: MemStore,
    }
    fn t() -> T {
        let clock = Arc::new(AtomicU64::new(1_000));
        let held = Arc::new(Mutex::new(Vec::new()));
        let store = MemStore::default();
        let c = clock.clone();
        let hub = SharedHub::new(Box::new(store.clone()), Box::new(Scopes(held.clone(), vec!["Docs".into()])), Box::new(move || c.load(Ordering::SeqCst)));
        T { hub, clock, held, store }
    }
    impl T {
        fn grant(&self, app: &str, scope: &str) {
            self.held.lock().unwrap().push(format!("{app}:{scope}"));
        }
        fn at(&self, ms: u64) {
            self.clock.store(ms, Ordering::SeqCst);
        }
        fn net(&self, s: &str) -> usize {
            self.hub.emit("connectivity.changed", None, payload(&[("state", Val::Text(s.into()))])).unwrap()
        }
    }

    #[test]
    fn the_catalogue_is_consistent() {
        let scopes: Vec<&str> = SCOPES.iter().map(|s| s.0).collect();
        let mut names = std::collections::BTreeSet::new();
        for d in CATALOGUE {
            assert!(names.insert(d.name), "{} twice", d.name);
            assert!(d.implicit || scopes.contains(&d.scope), "{}: unknown scope {}", d.name, d.scope);
            assert!(d.version >= 1 && !d.gives.is_empty());
            for (f, _) in d.fields {
                assert!(!secret_like(f), "{}: field {f} looks like a secret", d.name);
            }
            if let Coalesce::By(f) = d.coalesce {
                assert!(d.fields.iter().any(|x| x.0 == f), "{}: coalesce key", d.name);
            }
            if let Some(f) = d.filter_field {
                assert!(d.fields.iter().any(|x| x.0 == f), "{}: filter field", d.name);
            }
        }
        assert!(CATALOGUE.len() >= 40);
    }

    #[test]
    fn every_scope_is_a_capability_the_person_can_grant_and_every_event_has_one() {
        for (n, g) in SCOPES {
            let d = crate::appcaps::lookup(n).unwrap_or_else(|| panic!("{n} is not in the capability catalogue"));
            assert_eq!(&d.gives, g);
        }
    }

    #[test]
    fn the_guide_lists_every_event_and_every_scope() {
        let doc = include_str!("../../../docs/sdk/events.md");
        for d in CATALOGUE {
            assert!(doc.contains(&format!("`{}`", d.name)), "docs/sdk/events.md does not list {}", d.name);
        }
        for (n, _) in SCOPES {
            assert!(doc.contains(&format!("`{n}`")), "docs/sdk/events.md does not name {n}");
        }
    }

    #[test]
    fn events_arrive_in_order_with_growing_numbers_and_times() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        assert!(t.hub.start("p", "a").is_empty());
        for (i, d) in ["x", "y", "z"].iter().enumerate() {
            t.at(2_000 + i as u64);
            t.hub.emit("peers.changed", None, payload(&[("device", Val::Text((*d).into())), ("change", Val::Text("online".into()))])).unwrap();
        }
        let got = t.hub.poll("p", "a", 10);
        assert_eq!(got.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(got.iter().map(|e| e.at_ms).collect::<Vec<_>>(), [2000, 2001, 2002]);
        assert!(got.iter().all(|e| !e.late));
        assert!(t.hub.poll("p", "a", 10).is_empty());
    }

    #[test]
    fn an_app_hears_only_what_it_registered_and_holds_the_scope_for() {
        let t = t();
        t.grant("a", "events.network");
        let e = t.hub.register("p", "a", "power.source", "").unwrap_err();
        assert!(e.contains("events.power"), "{e}");
        assert!(t.hub.register("p", "a", "nope.nothing", "").unwrap_err().contains("no event"));
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.grant("b", "events.power");
        t.hub.register("p", "b", "power.source", "").unwrap();
        t.hub.start("p", "a");
        assert_eq!(t.net("online"), 1);
        assert_eq!(t.hub.emit("power.source", None, payload(&[("source", Val::Text("ac".into()))])).unwrap(), 1);
        assert_eq!(t.hub.poll("p", "a", 9).len(), 1);
        assert!(t.hub.audit().iter().any(|l| l.starts_with("appevents.refuse a: register power.source")));
        // a revoked scope stops delivery at once
        t.held.lock().unwrap().clear();
        assert_eq!(t.net("offline"), 0);
        assert!(t.hub.poll("p", "a", 9).is_empty());
    }

    #[test]
    fn events_while_the_app_is_away_wait_and_arrive_late_at_the_next_start() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        t.hub.start("p", "a");
        let peer = |d: &str| payload(&[("device", Val::Text(d.into())), ("change", Val::Text("paired".into()))]);
        t.hub.emit("peers.changed", None, peer("one")).unwrap();
        t.hub.stop("p", "a");
        t.at(5_000);
        t.hub.emit("peers.changed", None, peer("two")).unwrap();
        t.hub.emit("peers.changed", None, peer("three")).unwrap();
        let got = t.hub.start("p", "a");
        // "one" was queued while running but waited four seconds without a poll: late too (an event that waits past the grace is late)
        assert_eq!(got.iter().map(|e| (e.seq, e.late)).collect::<Vec<_>>(), [(1, true), (2, true), (3, true)]);
        t.hub.emit("peers.changed", None, peer("four")).unwrap();
        assert_eq!(t.hub.poll("p", "a", 9)[0].seq, 4);
    }

    #[test]
    fn an_app_that_stopped_polling_is_away_even_if_it_never_said_so() {
        // KI-1011 (candidate 3): the app was never told to stop (the person went Home), so what happened meanwhile was not marked late
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        t.at(10_000);
        t.hub.start("p", "a");
        t.at(11_000);
        assert!(t.hub.poll("p", "a", 9).is_empty());
        // polling stops here: no `stop` call. A second later it still counts as running.
        t.at(12_000);
        let peer = |d: &str| payload(&[("device", Val::Text(d.into())), ("change", Val::Text("online".into()))]);
        t.hub.emit("peers.changed", None, peer("soon")).unwrap();
        t.at(60_000);
        t.hub.emit("peers.changed", None, peer("away")).unwrap();
        let got = t.hub.start("p", "a");
        assert_eq!(got.iter().map(|e| (e.payload["device"].text().unwrap().to_string(), e.late)).collect::<Vec<_>>(), [("soon".to_string(), true), ("away".to_string(), true)], "both waited past the grace");
        // a polling app gets its events at once, not late
        t.at(60_500);
        t.hub.emit("peers.changed", None, peer("live")).unwrap();
        t.at(60_600);
        assert!(!t.hub.poll("p", "a", 9)[0].late);
    }

    #[test]
    fn the_queue_is_bounded_by_count_and_age_and_says_how_many_were_missed() {
        let t = t();
        t.grant("a", "events.storage");
        t.hub.register("p", "a", "storage.volume", "").unwrap();
        let vol = |i: usize| payload(&[("volume", Val::Text(format!("v{i}"))), ("change", Val::Text("mounted".into()))]);
        // the rate limit also applies: spread over minutes
        for i in 0..(MAX_QUEUE + 5) {
            t.at(1_000 + i as u64 * 61_000);
            t.hub.emit("storage.volume", None, vol(i)).unwrap();
        }
        let start_at = 1_000 + (MAX_QUEUE as u64 + 5) * 61_000;
        t.at(start_at);
        let got = t.hub.start("p", "a");
        assert!(got[0].is_missed());
        // 5 over the count cap; the oldest also grew older than the age cap? only if over 7 days
        let aged = (0..(MAX_QUEUE + 5)).filter(|i| start_at - (1_000 + *i as u64 * 61_000) > MAX_AGE_MS).count() as u64;
        assert_eq!(got[0].missed_count(), 5.max(aged), "{aged}");
        assert!(got.len() <= MAX_QUEUE + 1);
        assert!(got[1..].iter().all(|e| e.late));
        assert!(got.windows(2).all(|w| w[0].seq < w[1].seq));
        // age on its own
        t.hub.stop("p", "a");
        t.hub.emit("storage.volume", None, vol(0)).unwrap();
        t.at(start_at + MAX_AGE_MS + 1);
        let got = t.hub.start("p", "a");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].missed_count(), 1);
    }

    #[test]
    fn state_events_keep_only_the_latest_and_do_not_flood() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        for s in ["online", "offline", "metered", "online", "captive_portal"] {
            t.at(t.clock.load(Ordering::SeqCst) + 1);
            t.net(s);
        }
        let got = t.hub.start("p", "a");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload["state"], Val::Text("captive_portal".into()));
        assert!(got[0].late);
        // by key: the latest per VPN
        t.grant("a", "events.network");
        t.hub.register("p", "a", "vpn.changed", "").unwrap();
        let v = |n: &str, s: &str| payload(&[("name", Val::Text(n.into())), ("state", Val::Text(s.into()))]);
        for (n, s) in [("w", "up"), ("o", "up"), ("w", "down")] {
            t.hub.emit("vpn.changed", None, v(n, s)).unwrap();
        }
        let got = t.hub.poll("p", "a", 9);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].payload["state"], Val::Text("down".into()));
    }

    #[test]
    fn a_noisy_event_is_rate_limited_and_counted() {
        let t = t();
        t.grant("a", "events.flows");
        t.hub.register("p", "a", "flows.run", "").unwrap();
        t.hub.start("p", "a");
        let run = payload(&[("flow", Val::Text("f".into())), ("outcome", Val::Text("finished".into()))]);
        let sent = (0..80).filter(|_| t.hub.emit("flows.run", None, run.clone()).unwrap() == 1).count();
        assert_eq!(sent, 60);
        let got = t.hub.poll("p", "a", 200);
        assert!(got[0].is_missed() && got[0].missed_count() == 20);
        assert_eq!(got.len(), 61);
        // the next minute is open again
        t.at(1_000 + 60_001);
        assert_eq!(t.hub.emit("flows.run", None, run).unwrap(), 1);
    }

    #[test]
    fn payloads_are_checked_and_carry_no_secrets() {
        let t = t();
        let bad = payload(&[("state", Val::Text("sideways".into()))]);
        assert!(t.hub.emit("connectivity.changed", None, bad).is_err());
        assert!(t.hub.emit("connectivity.changed", None, Payload::new()).unwrap_err().contains("missing"));
        assert!(t.hub.emit("connectivity.changed", None, payload(&[("state", Val::Text("online".into())), ("ssid", Val::Text("x".into()))])).unwrap_err().contains("no field"));
        assert!(check_payload("custom.a.b", &payload(&[("api_key", Val::Text("x".into()))])).unwrap_err().contains("secret"));
        assert!(check_payload("custom.a.b", &payload(&[("Password", Val::Text("x".into()))])).is_err());
        assert!(check_payload("custom.a.b", &payload(&[("note", Val::Text("x".into()))])).is_ok());
        assert!(check_payload("custom.a.b", &payload(&[("note", Val::Text("x".repeat(300)))])).is_err());
        // a settings event has the key and nothing else
        let s = lookup("settings.changed").unwrap();
        assert_eq!(s.fields.len(), 1);
    }

    #[test]
    fn registrations_survive_a_restart_with_their_queue_and_sequence() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.hub.start("p", "a");
        t.net("online");
        assert_eq!(t.hub.poll("p", "a", 9)[0].seq, 1);
        t.hub.stop("p", "a");
        t.net("offline");
        // a new engine over the same store: the restart
        let held = t.held.clone();
        let again = SharedHub::new(Box::new(t.store.clone()), Box::new(Scopes(held, vec![])), Box::new(|| 9_000));
        assert_eq!(again.list("p", "a").len(), 1);
        let got = again.start("p", "a");
        assert_eq!((got[0].seq, got[0].late, got[0].payload["state"].clone()), (2, true, Val::Text("offline".into())));
    }

    #[test]
    fn the_person_switches_one_event_off_and_nothing_waits_for_it() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.net("online");
        assert_eq!(t.hub.list("p", "a")[0].pending, 1);
        t.hub.set_enabled("p", "a", "connectivity.changed", "", false).unwrap();
        assert_eq!(t.hub.list("p", "a")[0].pending, 0);
        assert_eq!(t.net("offline"), 0);
        assert!(!t.hub.wanted("connectivity.changed"));
        t.hub.set_enabled("p", "a", "connectivity.changed", "", true).unwrap();
        assert!(t.hub.wanted("connectivity.changed"));
        assert_eq!(t.hub.apps("p")[0].0, "a");
        t.hub.unregister("p", "a", "connectivity.changed", "").unwrap();
        assert!(t.hub.unregister("p", "a", "connectivity.changed", "").is_err());
    }

    #[test]
    fn replay_hands_out_the_delivered_events_again() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        t.hub.start("p", "a");
        for d in ["a", "b", "c"] {
            t.hub.emit("peers.changed", None, payload(&[("device", Val::Text(d.into())), ("change", Val::Text("online".into()))])).unwrap();
        }
        t.hub.poll("p", "a", 9);
        let r = t.hub.replay("p", "a", 1);
        assert_eq!(r.iter().map(|e| (e.seq, e.replayed)).collect::<Vec<_>>(), [(2, true), (3, true)]);
        assert!(t.hub.replay("p", "a", 3).is_empty());
    }

    #[test]
    fn a_requeue_puts_delivered_events_back_in_front_marked_late() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        t.hub.start("p", "a");
        for d in ["a", "b"] {
            t.hub.emit("peers.changed", None, payload(&[("device", Val::Text(d.into())), ("change", Val::Text("online".into()))])).unwrap();
        }
        t.hub.poll("p", "a", 9);
        t.hub.emit("peers.changed", None, payload(&[("device", Val::Text("c".into())), ("change", Val::Text("online".into()))])).unwrap();
        assert_eq!(t.hub.requeue("p", "a", 1).unwrap(), 1);
        let got = t.hub.poll("p", "a", 9);
        assert_eq!(got.iter().map(|e| e.payload["device"].text().unwrap().to_string()).collect::<Vec<_>>(), ["b", "c"]);
        assert!(got[0].late && got[0].seq == 3);
    }

    #[test]
    fn custom_events_need_the_publish_scope_and_the_publishers_own_name() {
        let t = t();
        t.grant("notes", "events.publish");
        t.grant("sync", "events.custom");
        t.hub.register("p", "sync", "custom.notes.saved", "").unwrap();
        let p = payload(&[("id", Val::Int(4))]);
        assert!(t.hub.publish("p", "other", "custom.other.x", p.clone()).unwrap_err().contains("events.publish"));
        assert!(t.hub.publish("p", "notes", "custom.sync.saved", p.clone()).unwrap_err().contains("own name"));
        assert!(t.hub.publish("p", "notes", "saved", p.clone()).is_err());
        assert!(t.hub.publish("p", "notes", "custom.notes.saved", payload(&[("token", Val::Text("x".into()))])).is_err());
        assert_eq!(t.hub.publish("p", "notes", "custom.notes.saved", p).unwrap(), 1);
        assert_eq!(t.hub.start("p", "sync")[0].name, "custom.notes.saved");
    }

    #[test]
    fn settings_and_file_events_match_their_prefix_and_the_file_policy() {
        let t = t();
        t.grant("a", "events.settings");
        t.grant("a", "events.files");
        t.hub.register("p", "a", "settings.changed", "display.").unwrap();
        assert!(t.hub.register("p", "a", "connectivity.changed", "x").is_err());
        assert!(t.hub.register("p", "a", "files.changed", "").is_err(), "a folder is needed");
        assert!(t.hub.register("p", "a", "files.changed", "Secret").unwrap_err().contains("file policy"));
        t.hub.register("p", "a", "files.changed", "Docs").unwrap();
        let s = |k: &str| payload(&[("setting", Val::Text(k.into()))]);
        assert_eq!(t.hub.emit("settings.changed", None, s("display.scale")).unwrap(), 1);
        assert_eq!(t.hub.emit("settings.changed", None, s("sound.volume")).unwrap(), 0);
        let f = |d: &str| payload(&[("folder", Val::Text(d.into())), ("name", Val::Text("a.txt".into())), ("change", Val::Text("modified".into()))]);
        assert_eq!(t.hub.emit("files.changed", None, f("Docs/sub")).unwrap(), 1);
        assert_eq!(t.hub.emit("files.changed", None, f("Other")).unwrap(), 0);
    }

    #[test]
    fn the_wake_phrase_event_exists_only_while_its_switch_is_on_and_timers_run_only_when_wanted() {
        let t = t();
        t.grant("a", "events.voice");
        t.grant("a", "events.clock");
        assert!(t.hub.register("p", "a", "voice.wake_heard", "").unwrap_err().contains("off"));
        t.hub.set_flag("wake_word", true);
        t.hub.register("p", "a", "voice.wake_heard", "").unwrap();
        assert_eq!(t.hub.emit("voice.wake_heard", None, Payload::new()).unwrap(), 1);
        t.hub.set_flag("wake_word", false);
        assert_eq!(t.hub.emit("voice.wake_heard", None, Payload::new()).unwrap(), 0);
        assert!(!t.hub.wanted("clock.minute"));
        t.hub.register("p", "a", "clock.minute", "").unwrap();
        assert!(t.hub.wanted("clock.minute"));
    }

    #[test]
    fn audit_lines_name_events_and_numbers_never_values() {
        let t = t();
        t.grant("a", "events.settings");
        t.hub.register("p", "a", "settings.changed", "").unwrap();
        t.hub.start("p", "a");
        t.hub.emit("settings.changed", None, payload(&[("setting", Val::Text("net.wifi.hunter2".into()))])).unwrap();
        t.hub.poll("p", "a", 3);
        let lines = t.hub.audit();
        assert!(lines.iter().any(|l| l == "appevents.deliver a: settings.changed #1"), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("hunter2")));
    }

    #[test]
    fn the_core_events_become_app_events_without_values() {
        let t = t();
        t.grant("a", "events.settings");
        t.grant("a", "events.flows");
        t.grant("a", "events.inbox");
        for e in ["settings.changed", "flows.run", "inbox.pending"] {
            t.hub.register("p", "a", e, "").unwrap();
        }
        let (n, p) = from_setting_change("sound.volume");
        assert_eq!(t.hub.emit(n, None, p).unwrap(), 1);
        let (n, p) = from_flow_end("backup", false);
        assert_eq!(t.hub.emit(n, None, p).unwrap(), 1);
        let [(n, p), _] = from_inbox(3, 1);
        assert_eq!(t.hub.emit(n, None, p).unwrap(), 1);
        assert_eq!(t.hub.start("p", "a").len(), 3);
    }

    #[test]
    fn listeners_name_who_hears_an_event() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "peers.changed", "").unwrap();
        assert_eq!(t.hub.listeners("peers.changed"), [("p".to_string(), "a".to_string(), String::new())]);
        t.hub.set_enabled("p", "a", "peers.changed", "", false).unwrap();
        assert!(t.hub.listeners("peers.changed").is_empty());
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_permission_change_reaches_the_app_it_is_about_without_a_registration_and_no_other() {
        let t = t();
        t.grant("a", "camera.view");
        t.grant("b", "events.network");
        t.hub.register("p", "b", "peers.changed", "").unwrap();
        t.hub.start("p", "b");
        assert_eq!(t.hub.permissions_changed("p", "a", &names(&["camera.view"]), &[], "person", false).unwrap(), 1);
        let got = t.hub.start("p", "a");
        assert_eq!(got.len(), 1);
        let e = &got[0];
        assert_eq!((e.name.as_str(), e.late), ("app.permissions", true));
        assert_eq!(e.payload["granted"], Val::Text("camera.view".into()));
        assert_eq!((e.payload["by"].clone(), e.payload["held"].clone(), e.payload["effective"].clone()), (Val::Text("person".into()), Val::Int(1), Val::Text("camera.view".into())));
        assert!(t.hub.poll("p", "b", 9).is_empty(), "another app hears nothing of it");
        // another person's app of the same name hears nothing either
        assert!(t.hub.start("q", "a").is_empty());
        assert!(t.hub.audit().iter().any(|l| l.starts_with("appevents.permissions a: 1 granted, 0 revoked, 0 dropped, by person")));
    }

    #[test]
    fn it_needs_no_scope_and_no_registration_and_the_person_cannot_switch_it_off() {
        let t = t();
        t.hub.register("p", "a", "app.permissions", "").unwrap();
        assert!(t.hub.list("p", "a").is_empty(), "nothing is registered: there is nothing to switch");
        assert_eq!(t.hub.set_enabled("p", "a", "app.permissions", "", false).unwrap_err(), ALWAYS_ON);
        assert_eq!(t.hub.unregister("p", "a", "app.permissions", "").unwrap_err(), ALWAYS_ON);
        t.hub.permissions_changed("p", "a", &[], &names(&["files.read"]), "administrator", true).unwrap();
        assert_eq!(t.hub.start("p", "a").len(), 1);
        assert_eq!(t.hub.known_apps("p"), ["a"]);
    }

    #[test]
    fn taking_an_events_scope_away_drops_that_registration_and_tells_the_app() {
        let t = t();
        t.grant("a", "events.network");
        t.grant("a", "events.power");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.hub.register("p", "a", "power.source", "").unwrap();
        t.hub.start("p", "a");
        t.held.lock().unwrap().retain(|s| s != "a:events.network");
        t.hub.permissions_changed("p", "a", &[], &names(&["events.network"]), "person", false).unwrap();
        let regs: Vec<String> = t.hub.list("p", "a").into_iter().map(|r| r.event).collect();
        assert_eq!(regs, ["power.source"]);
        let got = t.hub.poll("p", "a", 9);
        assert_eq!(got[0].payload["dropped"], Val::Text("connectivity.changed".into()));
        assert_eq!(got[0].payload["revoked"], Val::Text("events.network".into()));
        assert!(!got[0].late, "the app was running");
    }

    #[test]
    fn several_changes_before_the_app_listens_are_one_event_with_the_changes_added_up() {
        let t = t();
        t.grant("a", "x");
        t.hub.permissions_changed("p", "a", &names(&["camera.view", "files.read"]), &[], "person", false).unwrap();
        t.grant("a", "y");
        t.hub.permissions_changed("p", "a", &names(&["net.connect"]), &names(&["camera.view"]), "policy", true).unwrap();
        // camera.view was granted and taken away, and net.connect granted and taken away, before the app heard: neither is a change for it
        t.hub.permissions_changed("p", "a", &[], &names(&["net.connect"]), "store", false).unwrap();
        let got = t.hub.start("p", "a");
        assert_eq!(got.len(), 1, "one event, not three");
        let p = &got[0].payload;
        assert_eq!((p["granted"].clone(), p["revoked"].clone(), p["by"].clone(), p["restart"].clone()), (Val::Text("files.read".into()), Val::Text("".into()), Val::Text("store".into()), Val::Bool(true)));
        assert_eq!(p["held"], Val::Int(2), "the effective set is the latest");
    }

    #[test]
    fn a_change_made_while_the_device_was_off_arrives_after_the_restart_marked_late() {
        let t = t();
        t.hub.permissions_changed("p", "a", &names(&["camera.view"]), &[], "administrator", false).unwrap();
        let held = t.held.clone();
        let again = SharedHub::new(Box::new(t.store.clone()), Box::new(Scopes(held, vec![])), Box::new(|| 9_000));
        let got = again.start("p", "a");
        assert_eq!((got.len(), got[0].late, got[0].payload["by"].clone()), (1, true, Val::Text("administrator".into())));
        let (_, last) = again.permissions("p", "a");
        assert!(last.unwrap().contains("by administrator: granted [camera.view]"), "the last change is kept for Settings");
    }

    #[test]
    fn the_person_takes_a_permission_away_and_gives_it_back_and_the_app_hears_both() {
        let t = t();
        t.grant("a", "events.network");
        t.hub.register("p", "a", "connectivity.changed", "").unwrap();
        t.hub.start("p", "a");
        t.hub.poll("p", "a", 9);
        assert!(t.hub.set_permission("p", "a", "no.such.thing", false, "person").is_err());
        assert_eq!(t.hub.set_permission("p", "a", "events.network", false, "person").unwrap(), 1);
        assert!(t.hub.list("p", "a").is_empty(), "the registration ended with the permission");
        assert!(t.hub.register("p", "a", "connectivity.changed", "").unwrap_err().contains("events.network"));
        let got = t.hub.poll("p", "a", 9);
        assert_eq!((got[0].payload["revoked"].clone(), got[0].payload["by"].clone(), got[0].payload["held"].clone()), (Val::Text("events.network".into()), Val::Text("person".into()), Val::Int(0)));
        assert!(t.hub.set_permission("p", "a", "events.network", false, "person").unwrap_err().contains("already"));
        // it survives a restart, and giving it back does not give more than the core's records hold
        let again = SharedHub::new(Box::new(t.store.clone()), Box::new(Scopes(t.held.clone(), vec![])), Box::new(|| 9_000));
        assert!(again.register("p", "a", "connectivity.changed", "").is_err());
        again.set_permission("p", "a", "events.network", true, "person").unwrap();
        again.register("p", "a", "connectivity.changed", "").unwrap();
        assert!(again.set_permission("p", "a", "events.power", true, "person").is_err(), "it was never stopped from that");
    }

    #[test]
    fn a_long_set_of_names_is_cut_with_a_count_and_nothing_but_names_is_in_the_payload() {
        let many: Vec<String> = (0..60).map(|i| format!("capability.number{i}")).collect();
        let t = names_text(&many);
        assert!(t.len() <= MAX_TEXT && t.contains(",+"), "{t}");
        assert_eq!(names_text(&[]), "");
        assert!(lookup("app.permissions").unwrap().fields.iter().all(|(f, _)| !secret_like(f)));
    }

    #[test]
    fn a_core_with_no_app_events_refuses_with_a_sentence() {
        let n = NoAppEvents;
        assert_eq!(n.register("p", "a", "x", "").unwrap_err(), NO_EVENTS);
        assert!(n.poll("p", "a", 1).is_empty() && n.catalogue().is_empty());
    }
}
