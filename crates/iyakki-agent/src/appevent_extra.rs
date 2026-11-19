//! Further sources of the events apps hear (F-SDK-203): a USB device plugged or taken out, the screen shown to a client device or its
//! clipboard used (a sign for the person), the time zone the network put the machine in, and an app's own storage nearly full.
//! (An app asking to be woken at a time is the hub's own: `AppEvents::wake_at`, fired by the core's timer.)
//!
//! Same rules as `appevent_more`: a pure state machine per source takes what one look found, compares it with the look before and returns the
//! events; the first look only remembers; payloads are words and counts, never a serial number, an address, a place or a stored value.

use iyakki_core_api::appevents::{payload, Payload, Val, MAX_TEXT};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

fn word(s: &str) -> Val {
    Val::Text(s.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect())
}

// ------------------------------------------------------------------------------------------------ the signs: screen and clipboard

static SCREEN_CLIENTS: AtomicUsize = AtomicUsize::new(0);
static CLIPBOARD_USES: AtomicU64 = AtomicU64::new(0);

/// A client device started to see the screen (it signed in over the client link). Called by the client session.
pub fn screen_client_opened() {
    SCREEN_CLIENTS.fetch_add(1, Ordering::SeqCst);
}

/// That client device left.
pub fn screen_client_closed() {
    #[allow(deprecated)] // try_update is the new name; kept so older toolchains still build
    let _ = SCREEN_CLIENTS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
}

/// A client device used the machine's clipboard (put text in, or took text out, each with the person's own press).
pub fn clipboard_used() {
    CLIPBOARD_USES.fetch_add(1, Ordering::SeqCst);
}

/// The two counters as the next look will see them.
pub fn sharing_now() -> (usize, u64) {
    (SCREEN_CLIENTS.load(Ordering::SeqCst), CLIPBOARD_USES.load(Ordering::SeqCst))
}

// ------------------------------------------------------------------------------------------------ the facts

/// An app's own storage at one look: `(person, app)` to (bytes used, bytes allowed).
pub type Quotas = BTreeMap<(String, String), (u64, u64)>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtraFacts {
    /// The USB devices that are plugged in (hubs left out), by name.
    pub usb: BTreeSet<String>,
    pub screen_clients: usize,
    pub clipboard_uses: u64,
    /// Whether the person let the network choose the time zone, and the zone in force.
    pub zone_auto: bool,
    pub zone: String,
    pub quotas: Quotas,
}

/// The percent of the allowance in use, and the state it means: `near_full` from 90, `full` from 100, `ok` again under 80 (so a value at the
/// edge does not flap).
pub fn quota_state(used: u64, limit: u64, was: &str) -> (&'static str, i64) {
    let pct = if limit == 0 { 0 } else { (used.saturating_mul(100) / limit).min(1000) as i64 };
    let s = if pct >= 100 {
        "full"
    } else if pct >= 90 {
        "near_full"
    } else if pct < 80 || was == "ok" {
        // between 80 and 90 what was said before stands, so a value at the edge does not flap
        "ok"
    } else {
        "near_full"
    };
    (s, pct)
}

/// An event ready to emit: its name, the one person it is for (`None`: everybody) and its payload.
pub type Out = (&'static str, Option<String>, Payload);

#[derive(Default)]
pub struct ExtraWatch {
    last: Option<ExtraFacts>,
    said_quota: BTreeMap<(String, String), &'static str>,
}

impl ExtraWatch {
    /// The events between two looks (the first look only remembers).
    pub fn observe(&mut self, now: ExtraFacts) -> Vec<Out> {
        let mut out: Vec<Out> = Vec::new();
        if let Some(prev) = &self.last {
            for (change, set) in [("added", now.usb.difference(&prev.usb)), ("removed", prev.usb.difference(&now.usb))] {
                for d in set {
                    out.push(("usb.device", None, payload(&[("device", word(d)), ("change", word(change))])));
                }
            }
            if prev.screen_clients == 0 && now.screen_clients > 0 {
                out.push(("sharing.changed", None, payload(&[("kind", word("screen")), ("state", word("started"))])));
            }
            if prev.screen_clients > 0 && now.screen_clients == 0 {
                out.push(("sharing.changed", None, payload(&[("kind", word("screen")), ("state", word("stopped"))])));
            }
            if now.clipboard_uses > prev.clipboard_uses {
                out.push(("sharing.changed", None, payload(&[("kind", word("clipboard")), ("state", word("used"))])));
            }
            if now.zone_auto && !now.zone.is_empty() && (now.zone != prev.zone || !prev.zone_auto) && !prev.zone.is_empty() {
                out.push(("location.changed", None, payload(&[("zone", word(&now.zone))])));
            }
        }
        for (k, (used, limit)) in &now.quotas {
            let was = self.said_quota.get(k).copied().unwrap_or("ok");
            let (state, pct) = quota_state(*used, *limit, was);
            // an app that was already full when it first listened is told once: its first look is the state it starts from
            if state != was {
                out.push(("storage.quota", Some(k.0.clone()), payload(&[("state", word(state)), ("percent", Val::Int(pct))])));
            }
            self.said_quota.insert(k.clone(), state);
        }
        self.said_quota.retain(|k, _| now.quotas.contains_key(k));
        self.last = Some(now);
        out
    }
}

// ------------------------------------------------------------------------------------------------ the real readers

/// The USB devices under `<sys>/bus/usb/devices`: interfaces (`1-2:1.0`) and hubs (class 09) are left out; the name is the product text, or
/// `vendor:product` ids when there is none. A serial number is never read.
pub fn usb_devices(sys: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for e in std::fs::read_dir(sys.join("bus/usb/devices")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.contains(':') || n.starts_with("usb") {
            continue; // an interface, or a root hub
        }
        let read = |f: &str| std::fs::read_to_string(e.path().join(f)).unwrap_or_default().trim().to_string();
        if read("bDeviceClass") == "09" {
            continue;
        }
        let product = read("product");
        let ids = format!("{}:{}", read("idVendor"), read("idProduct"));
        out.insert(if product.is_empty() { ids } else { format!("{product} ({ids})") });
    }
    out
}

/// The bytes the files of one app's own storage take, from the folder's file list (`name`, `size`): only names that belong to the app count.
pub fn app_bytes(dir: &Path, app: &str) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        n == format!("{app}.json") || n.starts_with(&format!("{app}."))
    }).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// What one app may keep (the default of the SDK's storage port).
pub const APP_STORAGE_BYTES: u64 = 16 * 1024 * 1024;

#[cfg(test)]
#[path = "appevent_extra_tests.rs"]
mod tests;
