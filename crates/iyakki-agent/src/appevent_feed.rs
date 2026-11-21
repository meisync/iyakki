//! The real readers that feed the further event sources (F-SDK-204 to 207): what the audit log says about sign-ins and pairing, the
//! certificates the machine keeps, the update bundles waiting on the disk, and the wake phrase counter. Every function takes the text or
//! the folder it reads, so a test gives it a made-up one; a missing file says nothing.

use std::collections::BTreeMap;
use std::path::Path;

/// Running totals read from the audit log (a person's name never leaves this function: only counts do).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditTotals {
    /// `login.failed` lines.
    pub failed_signins: u64,
    /// `devicekey.enroll` lines: a new paired device.
    pub new_paired_devices: u64,
}

/// Count the audit log's JSON lines. Failed sign-ins carry no person (the name tried may not exist) and are machine-wide, as are pairings.
pub fn audit_totals(text: &str) -> AuditTotals {
    let mut t = AuditTotals::default();
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        match v.get("event").and_then(|e| e.as_str()) {
            Some("login.failed") => t.failed_signins += 1,
            Some("devicekey.enroll") => t.new_paired_devices += 1,
            _ => {}
        }
    }
    t
}

/// Days left of each certificate the machine keeps under `<dir>` (`tls/cert.pem`, `cert.pem` and `tls/*.crt`), keyed by the file's short name.
pub fn cert_days(dir: &Path, now_unix: i64) -> BTreeMap<String, i64> {
    let mut files = vec![dir.join("tls/cert.pem"), dir.join("cert.pem")];
    for e in std::fs::read_dir(dir.join("tls")).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "crt") {
            files.push(e.path());
        }
    }
    let mut out = BTreeMap::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let Some(not_after) = first_cert_not_after(&text) else { continue };
        let name = f.strip_prefix(dir).unwrap_or(&f).to_string_lossy().to_string();
        out.insert(name, crate::appevent_more::days_left(not_after, now_unix));
    }
    out
}

/// The `notAfter` of the first certificate block of a PEM text.
pub fn first_cert_not_after(pem: &str) -> Option<i64> {
    let body: String = pem.split("-----BEGIN CERTIFICATE-----").nth(1)?.split("-----END CERTIFICATE-----").next()?.lines().map(str::trim).collect();
    let der = iyakki_shared::b64::b64_decode(&body)?;
    iyakki_link::certinfo::read_cert(&der).map(|c| c.not_after)
}

/// Whether the network is asking for a sign-in, from the answer of the check the person already chose to run (nothing is sent here:
/// no check, no answer, no event).
pub fn captive_now() -> bool {
    iyakki_shared::wifilink::portal_seen() == Some(true)
}

/// The paired computers (the fleet's members other than this one) as the event watch sees them: online while the member's heartbeat is
/// fresh, and its battery is the one its heartbeat reports (none on a desktop). A member whose refusal is on record is left out.
pub fn peers_from_members(members: &[(String, String, bool, Option<u32>)]) -> BTreeMap<String, crate::appevent_more::PeerFact> {
    members.iter().filter(|(_, state, me, _)| !me && state != "refused" && state != "this computer").map(|(name, state, _, battery)| (name.chars().filter(|c| !c.is_control()).take(48).collect(), crate::appevent_more::PeerFact { online: state == "up", battery: *battery })).collect()
}

/// The fleet's members read from the hub of this data folder (nothing is made when no fleet was ever set up).
pub fn paired_computers(data: &Path) -> BTreeMap<String, crate::appevent_more::PeerFact> {
    if !data.join("fleet").is_dir() {
        return BTreeMap::new();
    }
    let Ok(hub) = crate::fleet_hub::Hub::open(data) else { return BTreeMap::new() };
    let rows: Vec<(String, String, bool, Option<u32>)> = hub.status().fleets.iter().flat_map(|f| f.members.iter().map(|m| (m.name.clone(), m.state.clone(), m.me, m.heartbeat.as_ref().and_then(|h| h.battery_pct)))).collect();
    peers_from_members(&rows)
}

/// Quiet hours as the machine has them: the outside-message policy of each person (`flows/<person>/channel-policy.json`). `None` when
/// nobody set quiet hours; `Some(true)` while any person's quiet hours cover `now`.
pub fn quiet_now(data: &Path, now: u64) -> Option<bool> {
    let mut any = false;
    let mut inside = false;
    for e in std::fs::read_dir(data.join("flows")).into_iter().flatten().flatten() {
        let book = iyakki_shared::flows_channel_policy::PolicyBook::new(&e.path());
        let p = book.policy();
        if p.quiet.is_some() {
            any = true;
            inside |= p.rules().in_quiet(now);
        }
    }
    any.then_some(inside)
}

/// The update phase of each component: `system` is `downloaded` while a checked bundle waits unpacked under the update centre's folder;
/// each installed item whose store entry is newer (`outdated`, the ids the store's own lifecycle list calls outdated) is `available`.
/// Nothing is sent when nothing is waiting. Only names go out, never a path or address.
pub fn update_phases(data: &Path, outdated: &[String]) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = outdated.iter().map(|id| (id.clone(), "available".to_string())).collect();
    let waiting = std::fs::read_dir(crate::sysupdate::staged_root(data)).into_iter().flatten().flatten().any(|e| e.path().is_dir());
    if waiting {
        out.insert("system".to_string(), "downloaded".to_string());
    }
    out
}

/// The installed items of every person (and the shared ones) that have a newer version in a store catalog already on the disk, by their
/// ids (`skill:notes`, `cli:kubectl`). Reads local catalogs only: no network call is made. A pinned item is not counted (its update is not offered).
pub fn outdated_items(svc: &crate::services::Services) -> Vec<String> {
    let store = crate::store::Store::for_services(svc);
    let loaded = store.local_catalogs(svc);
    let policy = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default());
    let mut users: Vec<String> = std::fs::read_dir(svc.dir().join("content/user")).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    users.sort();
    let mut ids = std::collections::BTreeSet::new();
    for u in users {
        for l in store.lifecycle(svc, &policy, &u, &loaded, &crate::store::lifecycle::Ctx::default()) {
            if l.can_update() {
                ids.insert(l.id.clone());
            }
        }
    }
    ids.into_iter().take(50).collect()
}

/// The model host as the event watch sees it: `(state, model)`. The hosts are the ones in the list of model hosts (`model-hosts.json`);
/// `default_model_of` gives the default model of that host's provider in Models (`None` when the provider is gone or has none). The
/// first host in the list that has a model is `ready` with that model's name; when hosts are listed and none has a model, the state is
/// `failed` with an empty name; with no host listed both are empty (nothing to tell). No network probe is made here: a host that is
/// reachable but whose model was removed is the only failure this sees.
pub fn model_host(host_ids: &[String], default_model_of: &dyn Fn(&str) -> Option<String>) -> (String, String) {
    if host_ids.is_empty() {
        return (String::new(), String::new());
    }
    for id in host_ids {
        if let Some(m) = default_model_of(id).filter(|m| !m.is_empty()) {
            return ("ready".to_string(), m);
        }
    }
    ("failed".to_string(), String::new())
}

#[cfg(test)]
#[path = "appevent_feed_tests.rs"]
mod tests;
