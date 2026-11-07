//! The periodic update check (F-UPD-04): looks at every store source the person or administrator configured and, when the
//! administrator set one, the system update index, and keeps the answer for the page, the Overview and a badge.
//!
//! * It only asks addresses that were set: the store's own sources (the official catalog on the portal, a mirror, a folder or a stick)
//!   and `system_url`, which is empty until an administrator fills it in. There is no built-in address here.
//! * It only reports. It never downloads a package or a system update and never installs anything: the person clicks Update in the
//!   STORE, or installs a system pack from the System update page.
//! * An app update that asks for permissions the installed version did not have is marked, and is never applied without the
//!   person reading them (the install review already shows them; the mark makes the list say so before).
//! * Packages that are not in any catalog (a developer's local ones) never appear: only catalog entries are compared.
//! * [`due`] decides when the next check is: daily with a small stable jitter so a fleet does not call at the same second, never when
//!   it is off, and not on a metered link unless the person allows it. The shell does not know which links are metered yet, so it
//!   passes `None` and the page says so.
//!
//! The last answer is kept in `update-check.json` in the data folder; a check that cannot reach a source keeps the older answer for
//! that source's apps out of the list (it is reported as an error, not as "up to date").

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use iyakki_sec::policy::Policy;

use super::catalog::Fetch;
use super::install::Actor;
use super::{State, Store};
use crate::devmode::update::compare_versions;
use crate::services::Services;

pub const FILE: &str = "update-check.json";
pub const DAY: i64 = 86_400;
/// The system update index is a small file.
const MAX_INDEX: usize = 64 * 1024;

/// How often to look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// About once a day (a stable jitter of up to an hour is added).
    #[default]
    Daily,
    /// Only when the person presses Check now.
    Manual,
    /// Never, not even from the page.
    Off,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Daily => Mode::Manual,
            Mode::Manual => Mode::Off,
            Mode::Off => Mode::Daily,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Daily => "every day",
            Mode::Manual => "only when I press Check now",
            Mode::Off => "off",
        }
    }
}

/// The choices of Settings -> System update, kept in the settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckSettings {
    pub mode: Mode,
    /// Where the system update index is (an administrator sets it; empty = no system check).
    pub system_url: String,
    /// Look on a metered connection too.
    pub metered_ok: bool,
}

/// A usable index address: https, or http for a mirror inside a network; no spaces, no user name or password in it.
pub fn check_url(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(String::new());
    }
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).ok_or("the address starts with https:// (or http:// for a mirror inside your network)")?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains('@') || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("that is not an address (no spaces, no user name or password in it)".into());
    }
    Ok(s.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    System,
    App,
}

/// One thing that can be updated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub kind: Kind,
    /// The package kind and name, or `system`.
    pub id: String,
    pub title: String,
    pub from: String,
    pub to: String,
    /// The catalog or index it was found in.
    pub source: String,
    /// Permissions the new version asks for and the installed one did not (plain words).
    pub new_permissions: Vec<String>,
    pub note: String,
}

impl Update {
    /// An app update with new permissions must be read before it is applied; the check never applies anything.
    pub fn needs_review(&self) -> bool {
        !self.new_permissions.is_empty()
    }
}

/// The answer of the last check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub checked_at: i64,
    pub updates: Vec<Update>,
    /// (source, what went wrong)
    pub errors: Vec<(String, String)>,
    /// Sources and indexes that were asked.
    pub asked: Vec<String>,
}

impl Report {
    pub fn count(&self) -> usize {
        self.updates.len()
    }
    /// The words for a badge or the top bar: "3 updates", "1 update", or nothing.
    pub fn badge(&self) -> Option<String> {
        match self.count() {
            0 => None,
            1 => Some("1 update".into()),
            n => Some(format!("{n} updates")),
        }
    }
    pub fn load(dir: &Path) -> Option<Report> {
        serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let tmp: PathBuf = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
    }
}

/// The system update index: what an administrator's mirror (or the portal) says is the newest version.
#[derive(Debug, Deserialize)]
struct Index {
    version: String,
    #[serde(default)]
    variant: String,
    #[serde(default)]
    notes: String,
}

/// What `due` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Look now.
    Now,
    /// Seconds until the next look.
    Later(i64),
    /// The person chose Manual: only the button looks.
    Manual,
    Off,
    /// A metered connection and the person did not allow it.
    Metered,
}

/// A stable delay of 0 to 3599 seconds for this device and this day (FNV-1a of the device key and the day number).
pub fn jitter_secs(device: &str, day: i64) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in device.bytes().chain(day.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % 3600) as i64
}

/// Is a check due? `metered` is `None` when the machine cannot tell.
pub fn due(s: &CheckSettings, last: i64, now: i64, metered: Option<bool>, device: &str) -> Due {
    match s.mode {
        Mode::Off => return Due::Off,
        Mode::Manual => return Due::Manual,
        Mode::Daily => {}
    }
    if metered == Some(true) && !s.metered_ok {
        return Due::Metered;
    }
    if last <= 0 {
        return Due::Now;
    }
    let next = last + DAY + jitter_secs(device, last / DAY);
    if now >= next { Due::Now } else { Due::Later(next - now) }
}

/// Look at the sources. Slow (network): call it off the UI thread. `refresh` reads every store source and updates the cached
/// copies; the comparison is then done on what is cached, so a source that could not be reached is an error line, not silence.
#[allow(clippy::too_many_arguments)]
pub fn run(store: &Store, svc: &Services, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &CheckSettings, running: &str, variant: &str) -> Report {
    let mut rep = Report { checked_at: store.now(), ..Report::default() };
    if settings.mode == Mode::Off {
        return rep;
    }
    // 1. the apps: every enabled source, then the view the STORE shows
    let cfg = store.config();
    rep.asked.extend(cfg.sources.iter().filter(|s| s.enabled).map(|s| s.name.clone()));
    let refreshed = store.refresh(svc, fetch);
    rep.errors.extend(refreshed.errors);
    let actor = Actor::from_policy(user, policy);
    let loaded = store.local_catalogs(svc);
    let items = store.items(svc, &actor, &loaded);
    let installed = store.installed(svc, policy, user, &items);
    for it in &items {
        let State::Update(to) = &it.state else { continue };
        let now_perms: Vec<String> = it.entry.meta.permissions.iter().filter_map(|p| super::manifest::Permission::parse(p).ok()).map(|p| p.words(user)).collect();
        let had: Vec<&String> = installed.iter().find(|e| e.kind == it.entry.kind && e.name == it.entry.name).map(|e| e.permissions.iter().collect()).unwrap_or_default();
        let added: Vec<String> = now_perms.into_iter().filter(|p| !had.contains(&p)).collect();
        rep.updates.push(Update {
            kind: Kind::App,
            id: format!("{}:{}", it.entry.kind, it.entry.name),
            title: it.entry.title().to_string(),
            from: it.installed_version.clone().unwrap_or_default(),
            to: to.clone(),
            source: it.source_name.clone(),
            note: if added.is_empty() { String::new() } else { "asks for permissions the installed version did not".into() },
            new_permissions: added,
        });
    }
    // 2. the system: only the address an administrator set
    if !settings.system_url.is_empty() {
        rep.asked.push(settings.system_url.clone());
        match check_url(&settings.system_url).and_then(|u| fetch.get(&u, MAX_INDEX)) {
            Err(e) => rep.errors.push(("System update".into(), e)),
            Ok(b) => match serde_json::from_slice::<Index>(&b) {
                Err(_) => rep.errors.push(("System update".into(), "the answer is not an update index".into())),
                Ok(ix) => {
                    if !ix.variant.is_empty() && !variant.is_empty() && ix.variant != variant {
                        // an index for another build of IYAKKI says nothing about this one
                    } else if compare_versions(&ix.version, running) == std::cmp::Ordering::Greater {
                        rep.updates.push(Update { kind: Kind::System, id: "system".into(), title: "IYAKKI".into(), from: running.into(), to: ix.version, source: "System update".into(), new_permissions: Vec::new(), note: ix.notes.chars().take(300).collect() });
                    }
                }
            },
        }
    }
    rep
}

/// [`run`], and the answer is kept for the page.
#[allow(clippy::too_many_arguments)]
pub fn run_and_keep(store: &Store, svc: &Services, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &CheckSettings, running: &str, variant: &str) -> Report {
    let rep = run(store, svc, policy, user, fetch, settings, running, variant);
    let _ = rep.save(svc.dir());
    rep
}
