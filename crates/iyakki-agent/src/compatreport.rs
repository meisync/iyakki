//! The compat report on this machine (F-CORE-19, `iyakki_core_api::compat`): what `dev info`, Settings, the support bundle and the chat
//! tool `compat_report` show, and the counter of deprecated names that were used. Reading only; nothing here changes a setting.

use std::collections::BTreeMap;
use std::sync::Mutex;

use iyakki_core_api::compat::{Peer, Report};

fn seen_slot() -> &'static Mutex<BTreeMap<String, u64>> {
    static S: std::sync::OnceLock<Mutex<BTreeMap<String, u64>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// A deprecated name was used by a client, an app or a file: counted for the report, once per use. Call it from the alias path of a
/// deprecated capability, call, field or message (the alias test of the deprecation table proves the old name still works).
pub fn note_use(name: &str) {
    let mut m = seen_slot().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // bounded: a hostile client cannot grow it
    if m.len() < 200 || m.contains_key(name) {
        *m.entry(name.chars().take(80).collect()).or_insert(0) += 1;
    }
}

/// The deprecated names used since this start, with their counts.
pub fn seen() -> Vec<(String, u64)> {
    seen_slot().lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().map(|(k, v)| (k.clone(), *v)).collect()
}

/// Paired developer computers as peers: the name, what they speak (developer mode's TLS channel) and how long ago they were heard.
pub fn computers(keys: &[crate::devmode::DevKey], now: i64) -> Vec<Peer> {
    keys.iter().map(|k| Peer { kind: "computer".into(), name: k.name.chars().take(40).collect(), version: "developer mode".into(), idle_secs: (k.last_used > 0).then(|| (now - k.last_used).max(0) as u64) }).collect()
}

/// Live peers: the open sessions of a client device and the helper programs running now (a decoder, a converter). They come and go, so they
/// are kept in memory only, in a bounded table, and listed with how long they have been open.
fn live_slot() -> &'static Mutex<BTreeMap<u64, (String, String, String, i64)>> {
    static S: std::sync::OnceLock<Mutex<BTreeMap<u64, (String, String, String, i64)>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

static NEXT_LIVE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Something that is connected now: `kind` is `session` (a client device's open session) or `helper` (a program this machine runs for a
/// feature); `version` says what it speaks. Returns a guard: when it is dropped the peer is gone from the report.
pub fn note_live(kind: &str, name: &str, version: &str) -> LiveGuard {
    let id = NEXT_LIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut m = live_slot().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // bounded: a flood of sessions or helpers cannot grow the table
    if m.len() < 200 {
        m.insert(id, (kind.chars().take(12).collect(), name.chars().take(40).collect(), version.chars().take(60).collect(), crate::clock::now_unix()));
    }
    LiveGuard(id)
}

pub struct LiveGuard(u64);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        live_slot().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
    }
}

/// The live peers, oldest first, with how long each has been open.
pub fn live(now: i64) -> Vec<Peer> {
    live_slot().lock().unwrap_or_else(std::sync::PoisonError::into_inner).values().map(|(k, n, v, since)| Peer { kind: k.clone(), name: n.clone(), version: v.clone(), idle_secs: Some((now - since).max(0) as u64) }).collect()
}

/// The report of this machine.
/// Client devices (the browser or window clients paired with a one-time link, experimental) that are not revoked, as peers: the name, the
/// client protocol they speak and how long ago they were heard (`last_seen`, 0 when never).
pub fn client_devices(data: &std::path::Path, now: i64) -> Vec<Peer> {
    crate::clientlink::list_devices(data)
        .into_iter()
        .filter(|d| d.active())
        .map(|d| Peer { kind: "client".into(), name: d.name.chars().take(40).collect(), version: format!("client protocol {}", iyakki_core_api::client::PROTOCOL_VERSION), idle_secs: (d.last_seen > 0).then(|| (now - d.last_seen).max(0) as u64) })
        .collect()
}

/// Every peer an administrator sees: the paired developer computers, then the paired client devices.
pub fn peers(data: &std::path::Path, keys: &[crate::devmode::DevKey], now: i64) -> Vec<Peer> {
    let mut v = computers(keys, now);
    v.extend(client_devices(data, now));
    v.extend(live(now));
    v
}

pub fn report(peers: Vec<Peer>) -> Report {
    Report::new(env!("CARGO_PKG_VERSION"), peers, seen())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deprecated_name_that_is_used_is_counted_and_shown_with_the_report() {
        note_use("compat-test-name");
        note_use("compat-test-name");
        let n = seen().into_iter().find(|(k, _)| k == "compat-test-name").unwrap().1;
        assert!(n >= 2);
        let r = report(vec![]);
        assert!(r.deprecated_seen.iter().any(|(k, c)| k == "compat-test-name" && *c >= 2));
        assert!(r.lines(true).iter().any(|l| l.starts_with("deprecated name seen but not in the table: compat-test-name")));
        // a very long name is cut, and the table cannot grow without bound
        note_use(&"x".repeat(500));
        assert!(seen().iter().all(|(k, _)| k.chars().count() <= 80));
        for i in 0..400 {
            note_use(&format!("flood-{i}"));
        }
        assert!(seen().len() <= 201);
    }

    #[test]
    fn paired_computers_are_peers_with_how_long_ago_they_were_heard() {
        let k = |name: &str, last: i64| crate::devmode::DevKey { name: name.into(), key: "00".into(), account: "root".into(), added: 1, added_by: "root".into(), last_used: last, scopes: vec![] };
        let p = computers(&[k("laptop", 990), k("never-used", 0)], 1000);
        assert_eq!((p[0].name.as_str(), p[0].idle_secs), ("laptop", Some(10)));
        assert_eq!(p[1].idle_secs, None);
        assert!(report(p).lines(true).iter().any(|l| l.contains("peer computer laptop · developer mode · heard 10 s ago")));
    }

    #[test]
    fn paired_client_devices_are_peers_too_until_they_are_revoked() {
        let d = std::env::temp_dir().join(format!("iyakki-compat-clients-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let a = crate::clientlink::enrol_device(&d, "phone", &"ab".repeat(32), "admin", 900).unwrap();
        let b = crate::clientlink::enrol_device(&d, "tablet", &"cd".repeat(32), "admin", 950).unwrap();
        crate::clientlink::touch_device(&d, &a.id, 990);
        let k = crate::devmode::DevKey { name: "laptop".into(), key: "00".into(), account: "root".into(), added: 1, added_by: "root".into(), last_used: 995, scopes: vec![] };
        let lines = report(peers(&d, &[k], 1000)).lines(true);
        assert!(lines.iter().any(|l| l.contains("peer computer laptop")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("peer client phone · client protocol 1 · heard 10 s ago")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("peer client tablet")), "{lines:?}");
        assert!(crate::clientlink::revoke_device(&d, &b.id, 1001).unwrap());
        let lines = report(peers(&d, &[], 1002)).lines(true);
        assert!(lines.iter().any(|l| l.contains("peer client phone")) && !lines.iter().any(|l| l.contains("tablet")), "{lines:?}");
        // a user account sees no peers at all
        assert!(!report(vec![]).lines(false).iter().any(|l| l.contains("peer ")));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn open_sessions_and_running_helpers_are_listed_while_they_live_and_gone_when_they_end() {
        let d = crate::testdir::outside_project("compat-live", "x");
        let a = note_live("session", "phone", "client protocol 1");
        let b = note_live("helper", "probe decoder", "probe-helper");
        let lines = report(peers(&d, &[], crate::clock::now_unix())).lines(true);
        assert!(lines.iter().any(|l| l.contains("peer session phone · client protocol 1")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("peer helper probe decoder · probe-helper")), "{lines:?}");
        drop(a);
        let lines = report(peers(&d, &[], crate::clock::now_unix())).lines(true);
        assert!(!lines.iter().any(|l| l.contains("peer session phone")) && lines.iter().any(|l| l.contains("peer helper probe decoder")), "{lines:?}");
        drop(b);
        let lines = report(peers(&d, &[], crate::clock::now_unix())).lines(true);
        assert!(!lines.iter().any(|l| l.contains("probe decoder")), "{lines:?}");
        // a user account sees none of it
        let _c = note_live("session", "tablet", "x");
        assert!(!report(vec![]).lines(false).iter().any(|l| l.contains("tablet")));
    }
}
