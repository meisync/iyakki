//! More sources of the events apps hear (F-SDK-204 to 207, 211): the ones that were catalogued and not yet fed.
//!
//! Same rules as `appevent_sources`: a pure state machine per source takes what one look found, compares it with the look before and
//! returns the events; the first look only remembers; payloads are words and counts, never a name of a person, an address or a value.
//! The real readers at the end read plain files of this machine (`/sys`, `/proc`, `/run`); a machine without the file says nothing.

use crate::appevent_sources::emit;
use iyakki_core_api::appevents::{payload, AppEvents, Payload, Val};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn word(s: &str) -> Val {
    Val::Text(s.chars().filter(|c| !c.is_control()).take(iyakki_core_api::appevents::MAX_TEXT).collect())
}

/// A paired computer as the last look found it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerFact {
    pub online: bool,
    pub battery: Option<u32>,
}

/// Everything the machine-wide sources look at once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoreFacts {
    /// The link is paid by the amount or is a tether (`metered`), or needs a sign-in page (`captive`).
    pub metered: bool,
    pub captive: bool,
    /// How many times the machine has gone to sleep and woke up (the kernel's count of good suspends).
    pub suspends: u64,
    /// Seconds until a shutdown or restart that is already scheduled.
    pub shutdown_in: Option<u32>,
    /// Quiet hours on or off; `None` when the machine has no such setting.
    pub quiet: Option<bool>,
    /// Paired computers by name.
    pub peers: BTreeMap<String, PeerFact>,
    /// Free space of each mounted volume in percent.
    pub free_percent: BTreeMap<String, u8>,
    /// Each disk and whether it reports itself well.
    pub disk_ok: BTreeMap<String, bool>,
    /// Connected Bluetooth devices by name.
    pub bluetooth: BTreeSet<String>,
    /// Printers and scanners: (`printer` or `scanner`, name).
    pub printers: BTreeSet<(String, String)>,
    pub locale: String,
    /// `WIDTHxHEIGHT` of the screen.
    pub resolution: String,
    /// The phase of each component's update: `available`, `downloaded` or `applied`.
    pub updates: BTreeMap<String, String>,
    /// The model host: `ready` or `failed`, and the model's name.
    pub model: (String, String),
    /// How many times the experimental wake phrase was heard since start.
    pub wake_heard: u64,
    /// Days left of each certificate or key.
    pub certs: BTreeMap<String, i64>,
}

/// The free space at or under which a volume is "low", and the free space over which it counts as recovered.
pub const LOW_SPACE: u8 = 5;
pub const RECOVERED_SPACE: u8 = 10;
/// The days at which a certificate or key is told once each.
pub const CERT_DAYS: [i64; 3] = [30, 7, 1];

#[derive(Default)]
pub struct MoreWatch {
    last: Option<MoreFacts>,
    low: BTreeSet<String>,
    /// For each certificate the smallest threshold already told.
    told: BTreeMap<String, i64>,
    shutdown_told: bool,
}

impl MoreWatch {
    /// The events between two looks (the first look only remembers; a value that was already wrong at the first look is remembered as told).
    pub fn observe(&mut self, now: MoreFacts) -> Vec<(&'static str, Payload)> {
        let mut out: Vec<(&'static str, Payload)> = Vec::new();
        let was_low = self.low.clone();
        for (v, p) in &now.free_percent {
            if *p <= LOW_SPACE {
                self.low.insert(v.clone());
            } else if *p >= RECOVERED_SPACE {
                self.low.remove(v);
            }
        }
        for (c, d) in &now.certs {
            let at = CERT_DAYS.iter().copied().filter(|t| d <= t).min();
            match at {
                None => {
                    self.told.remove(c);
                }
                Some(t) => {
                    let known = self.told.get(c).copied();
                    if known.is_none_or(|k| t < k) {
                        self.told.insert(c.clone(), t);
                        // an expiry that is close at the first look is still told once: nothing else would ever say it
                        out.push(("cert.expiring", payload(&[("what", word(c)), ("days", Val::Int((*d).max(0)))])));
                    }
                }
            }
        }
        self.told.retain(|c, _| now.certs.contains_key(c));
        let shutdown_now = now.shutdown_in.is_some();
        if shutdown_now && !self.shutdown_told {
            out.push(("power.shutdown_pending", payload(&[("in_seconds", Val::Int(now.shutdown_in.unwrap_or(0) as i64))])));
        }
        self.shutdown_told = shutdown_now;
        if let Some(prev) = self.last.take() {
            if now.captive && !prev.captive {
                out.push(("connectivity.changed", payload(&[("state", word("captive_portal"))])));
            } else if now.metered && !prev.metered && !now.captive {
                out.push(("connectivity.changed", payload(&[("state", word("metered"))])));
            } else if (prev.captive || prev.metered) && !now.captive && !now.metered {
                out.push(("connectivity.changed", payload(&[("state", word("online"))])));
            }
            if now.suspends > prev.suspends {
                out.push(("power.sleep", payload(&[("phase", word("wake"))])));
            }
            if let (Some(a), Some(b)) = (prev.quiet, now.quiet) {
                if a != b {
                    out.push(("quiet_hours.changed", payload(&[("state", word(if b { "on" } else { "off" }))])));
                }
            }
            for (n, p) in &now.peers {
                match prev.peers.get(n) {
                    None => out.push(("peers.changed", payload(&[("device", word(n)), ("change", word("paired"))]))),
                    Some(o) => {
                        if o.online != p.online {
                            out.push(("peers.changed", payload(&[("device", word(n)), ("change", word(if p.online { "online" } else { "offline" }))])));
                        }
                        if let (Some(b), true) = (p.battery, o.battery != p.battery) {
                            out.push(("peers.battery", payload(&[("device", word(n)), ("percent", Val::Int(b.min(100) as i64))])));
                        }
                    }
                }
            }
            for n in prev.peers.keys().filter(|n| !now.peers.contains_key(*n)) {
                out.push(("peers.changed", payload(&[("device", word(n)), ("change", word("forgotten"))])));
            }
            for (v, p) in &now.free_percent {
                // told when it falls to the low mark, and again only after it was over the recovered mark (a rise to 7 and a fall to 4 is the same low)
                if *p <= LOW_SPACE && !was_low.contains(v) {
                    out.push(("storage.volume", payload(&[("volume", word(v)), ("change", word("low_space"))])));
                }
            }
            for (d, ok) in &now.disk_ok {
                if prev.disk_ok.get(d).is_some_and(|o| o != ok) {
                    out.push(("disk.health", payload(&[("disk", word(d)), ("state", word(if *ok { "ok" } else { "warning" }))])));
                }
            }
            for (change, set) in [("connected", now.bluetooth.difference(&prev.bluetooth)), ("disconnected", prev.bluetooth.difference(&now.bluetooth))] {
                for d in set {
                    out.push(("bluetooth.device", payload(&[("device", word(d)), ("change", word(change))])));
                }
            }
            for (change, set) in [("added", now.printers.difference(&prev.printers)), ("removed", prev.printers.difference(&now.printers))] {
                for (k, d) in set {
                    out.push(("printer.changed", payload(&[("kind", word(k)), ("device", word(d)), ("change", word(change))])));
                }
            }
            if !now.locale.is_empty() && prev.locale != now.locale && !prev.locale.is_empty() {
                out.push(("locale.changed", payload(&[("locale", word(&now.locale))])));
            }
            if !now.resolution.is_empty() && prev.resolution != now.resolution && !prev.resolution.is_empty() {
                out.push(("display.changed", payload(&[("what", word("resolution"))])));
            }
            for (c, ph) in &now.updates {
                if prev.updates.get(c) != Some(ph) {
                    out.push(("updates.changed", payload(&[("component", word(c)), ("phase", word(ph))])));
                }
            }
            if prev.model != now.model && !now.model.0.is_empty() {
                let state = if !prev.model.1.is_empty() && prev.model.1 != now.model.1 && now.model.0 == "ready" { "changed" } else { now.model.0.as_str() };
                out.push(("models.host", payload(&[("state", word(state)), ("model", word(&now.model.1))])));
            }
            if now.wake_heard > prev.wake_heard {
                out.push(("voice.wake_heard", Payload::new()));
            }
        }
        self.last = Some(now);
        out
    }
}

/// What is known about one person's counts at one look.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MorePersonFacts {
    /// Tool calls waiting for the person's answer.
    pub approvals: usize,
    /// Running totals since the machine started: failed sign-ins, newly paired devices, vault unlocks from a new place.
    pub failed_signins: u64,
    pub new_paired_devices: u64,
    pub new_place_unlocks: u64,
    /// The titles of the reminders and calendar entries due now (they leave the list when they are done or snoozed).
    pub reminders_due: BTreeSet<String>,
    /// Backups this person made by hand or by the chat tool since the machine kept count, and the last one (file name, whether it worked).
    pub backups: u64,
    pub backup_last: (String, bool),
    /// Downloads the update centre page made since the machine kept count (a machine-wide note), and the last one (name, whether it worked).
    pub downloads: u64,
    pub download_last: (String, bool),
}

#[derive(Default)]
pub struct MorePersonWatch {
    last: BTreeMap<String, MorePersonFacts>,
}

impl MorePersonWatch {
    pub fn observe(&mut self, user: &str, now: MorePersonFacts) -> Vec<(&'static str, Payload)> {
        let mut out = Vec::new();
        if let Some(prev) = self.last.insert(user.to_string(), now.clone()) {
            if prev.approvals != now.approvals {
                out.push(("approvals.pending", payload(&[("count", Val::Int(now.approvals as i64))])));
            }
            for (kind, a, b) in [("failed_signin", prev.failed_signins, now.failed_signins), ("new_paired_device", prev.new_paired_devices, now.new_paired_devices), ("vault_unlock_new_place", prev.new_place_unlocks, now.new_place_unlocks)] {
                if b > a {
                    out.push(("security.alert", payload(&[("kind", word(kind)), ("count", Val::Int((b - a).min(i64::MAX as u64) as i64))])));
                }
            }
            if now.backups > prev.backups {
                out.push(("backup.finished", payload(&[("name", word(&now.backup_last.0)), ("outcome", word(if now.backup_last.1 { "finished" } else { "failed" }))])));
            }
            if now.downloads > prev.downloads {
                out.push(("transfer.finished", payload(&[("kind", word("download")), ("name", word(&now.download_last.0)), ("outcome", word(if now.download_last.1 { "finished" } else { "failed" }))])));
            }
            for t in now.reminders_due.difference(&prev.reminders_due) {
                out.push(("reminder.due", payload(&[("title", word(t))])));
            }
        }
        out
    }
}

/// The titles of the calendar entries that are due now (F-SDK-206): an occurrence is due from its reminder time (the start minus its
/// `remind` minutes; the start itself when it has none) until it ends, or for the first five minutes of one that has no length. At most 20
/// titles. Pure: the occurrences come from the calendar store for a window around `now`.
pub fn reminders_due_now(occurrences: &[iyakki_shared::calendar::Occurrence], now: i64) -> BTreeSet<String> {
    occurrences
        .iter()
        .filter(|o| {
            let from = o.start - i64::from(o.remind.unwrap_or(0)) * 60;
            let until = if o.end > o.start { o.end } else { o.start + 300 };
            from <= now && now < until
        })
        .map(|o| o.title.clone())
        .take(20)
        .collect()
}

/// The language the machine runs in (F-SDK-206, decision D-LOCALE-SOURCE): a one-line file `locale` in the data folder when the owner put
/// one there, else `LC_ALL`, else `LANG`. There is no per-person language setting in this release, so every person hears the same value.
pub fn machine_locale(dir: &Path, lc_all: &str, lang: &str) -> String {
    let own = std::fs::read_to_string(dir.join("locale")).unwrap_or_default();
    let pick = [own.trim(), lc_all.trim(), lang.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("");
    pick.chars().filter(|c| !c.is_control()).take(40).collect()
}

fn backup_note_path(dir: &Path, user: &str) -> Option<std::path::PathBuf> {
    iyakki_sec::policy::valid_username(user).then(|| dir.join(format!("backups-made-{user}.json")))
}

/// A backup was made by hand (the command line) or by the chat tool: keep a count and the last file name for the pulled source below
/// (F-SDK-211). Only the file's own name is kept, never where it went or what it holds. A scheduled backup tells the apps itself and does
/// not come here. Never fails the backup.
pub fn note_backup_made(dir: &Path, user: &str, file_name: &str, ok: bool) {
    let Some(p) = backup_note_path(dir, user) else { return };
    let (count, _) = backups_made(dir, user);
    let name: String = file_name.chars().filter(|c| !c.is_control()).take(80).collect();
    let _ = std::fs::write(p, serde_json::json!({"count": count + 1, "name": name, "ok": ok}).to_string());
}

fn download_note_path(dir: &Path) -> std::path::PathBuf {
    dir.join("downloads-made.json")
}

/// A download that no person's session started (the update centre page fetching a signed system update) ended: keep a count and the fixed
/// name for the pulled source (F-SDK-207). Machine-wide; never an address, never what was in the file. Never fails the download.
pub fn note_download_made(dir: &Path, name: &str, ok: bool) {
    let (count, _) = downloads_made(dir);
    let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
    let _ = std::fs::write(download_note_path(dir), serde_json::json!({"count": count + 1, "name": name, "ok": ok}).to_string());
}

/// The count of page-started downloads and the last one `(name, ok)`.
pub fn downloads_made(dir: &Path) -> (u64, (String, bool)) {
    let Some(v) = std::fs::read_to_string(download_note_path(dir)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return (0, (String::new(), false)) };
    (v["count"].as_u64().unwrap_or(0), (v["name"].as_str().unwrap_or("").to_string(), v["ok"].as_bool().unwrap_or(false)))
}

/// The count of hand-made backups and the last one `(name, ok)`.
pub fn backups_made(dir: &Path, user: &str) -> (u64, (String, bool)) {
    let Some(v) = backup_note_path(dir, user).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return (0, (String::new(), false)) };
    (v["count"].as_u64().unwrap_or(0), (v["name"].as_str().unwrap_or("").to_string(), v["ok"].as_bool().unwrap_or(false)))
}

// ------------------------------------------------------------------------------------------------ pushed

/// The machine is about to sleep (the last thing the service does before it asks for the suspend); the wake is found by the pulled source.
pub fn going_to_sleep(hub: &dyn AppEvents) {
    emit(hub, "power.sleep", None, payload(&[("phase", word("sleep"))]));
}

/// A download or an install ended: `kind` is `download` or `install`, `name` what it was.
pub fn transfer_finished(hub: &dyn AppEvents, user: &str, kind: &str, name: &str, ok: bool) {
    emit(hub, "transfer.finished", Some(user), payload(&[("kind", word(kind)), ("name", word(name)), ("outcome", word(if ok { "finished" } else { "failed" }))]));
}

// ------------------------------------------------------------------------------------------------ real readers (the kernel reads are the core's)

pub use iyakki_core_local::appevent_facts::{bluetooth_links, disk_states, free_percents, printers_and_scanners, screen_resolution, suspend_count};

/// The names of the connected Bluetooth devices (F-SDK-211): each connected address is looked up in the pairing store by its address and told
/// by the device's own name; a device the store does not know is told as `device` (an address is never put in an event). Sorted, no repeats.
pub fn bluetooth_names(connected: &[String], store: &iyakki_core_local::devctl::bthid::Store) -> BTreeSet<String> {
    connected.iter().map(|a| store.device(a).map(|k| k.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "device".to_string())).collect()
}

/// The connected Bluetooth devices by name. The addresses come from the kernel's management channel (needs the right to open it) and the
/// names from the pairing store; when the channel cannot be opened the kernel's own links are told by their handle, as before.
pub fn bluetooth_connected(sys: &Path, store_path: &Path) -> BTreeSet<String> {
    let links = bluetooth_links(sys);
    if links.is_empty() {
        return links;
    }
    let addresses = (|| {
        let sock = iyakki_core_local::devctl::btmgmt::ControlSocket::open().ok()?;
        let mut m = iyakki_core_local::devctl::btmgmt::Mgmt::new(sock);
        let mut all = Vec::new();
        for c in iyakki_core_local::devctl::btview::read_controllers(&mut m).ok()? {
            all.extend(m.connections(c.index).ok()?.into_iter().map(|(a, _)| a));
        }
        Some(all)
    })();
    match addresses {
        Some(a) => bluetooth_names(&a, &iyakki_core_local::devctl::bthid::Store::parse(&std::fs::read_to_string(store_path).unwrap_or_default())),
        None => links,
    }
}

/// Seconds until a scheduled shutdown (`/run/systemd/shutdown/scheduled` style: the first line `USEC=<epoch microseconds>`); none when absent.
pub fn shutdown_in(file_text: &str, now_unix: i64) -> Option<u32> {
    let usec: i64 = file_text.lines().find_map(|l| l.strip_prefix("USEC="))?.trim().parse().ok()?;
    Some(((usec / 1_000_000) - now_unix).clamp(0, u32::MAX as i64) as u32)
}

/// Days left of a certificate whose `notAfter` is the Unix time `not_after` (negative: expired).
pub fn days_left(not_after: i64, now_unix: i64) -> i64 {
    (not_after - now_unix).div_euclid(86_400)
}

#[cfg(test)]
#[path = "appevent_more_tests.rs"]
mod tests;
