//! The inbox notice and the badge count for "updates available" (F-UPD-04).
//!
//! * ONE grouped notice per person for everything that is waiting: apps and skills, driver packs and the system, e.g.
//!   "Updates available: 3 apps, system 1107h", with up to three names in its text. It is kept under one key
//!   ([`KEY`]), so a later check rewrites the same entry (same id) instead of adding another; a new version of anything makes it
//!   unread again; when the set only shrinks (something was installed) the text changes and the person's reading state stays;
//!   when nothing is pending any more it is removed. Dismissing it is honoured: an unchanged set is not announced again.
//! * What was announced is remembered in `update-announced.json` (pairs of `id@version`), so a source that is briefly unreachable
//!   does not make the same update look new when it comes back.
//! * The notice carries titles and versions only (no addresses, no secrets) and never installs anything: opening it shows the
//!   Store's updates page, or Settings -> System update when only the system is waiting.
//! * [`announce_system_update`] is the hook for the planned component manifest: it records "version X is out" so the badge, the
//!   System update line and the notice show it, and the next check keeps it until the running version has caught up.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::updatecheck::{Kind, Report, Update};
use crate::devmode::update::compare_versions;
use crate::inbox::{Kind as NoteKind, Link};
use crate::services::Services;

pub const FILE: &str = "update-announced.json";
/// The key of the one grouped notice.
pub const KEY: &str = "store-updates";
pub const SOURCE: &str = "store";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Memory {
    /// `id@version` of everything announced and still pending.
    announced: Vec<String>,
    /// A system version the component manifest (or anything else) announced, kept until the running version has caught up.
    extra_version: String,
    extra_notes: String,
}

fn load(dir: &Path) -> Memory {
    std::fs::read(dir.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(dir: &Path, m: &Memory) {
    let tmp = dir.join(format!("{FILE}.tmp"));
    if std::fs::write(&tmp, serde_json::to_vec_pretty(m).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(FILE));
    }
}

/// What a call did to the notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Posted,
    /// The same entry was rewritten.
    Updated,
    Removed,
}

fn pair(u: &Update) -> String {
    format!("{}@{}", u.id, u.to)
}

/// The grouped words: the title, the text and where it opens.
pub fn summary(updates: &[Update]) -> (String, String, Link) {
    let apps = updates.iter().filter(|u| u.kind == Kind::App && !u.driver).count();
    let drivers = updates.iter().filter(|u| u.kind == Kind::App && u.driver).count();
    let system = updates.iter().find(|u| u.kind == Kind::System);
    let mut parts = Vec::new();
    if apps > 0 {
        parts.push(format!("{apps} app{}", if apps == 1 { "" } else { "s" }));
    }
    if drivers > 0 {
        parts.push(format!("{drivers} driver{}", if drivers == 1 { "" } else { "s" }));
    }
    if let Some(s) = system {
        parts.push(format!("system {}", s.to));
    }
    let title = format!("Updates available: {}", parts.join(", "));
    let named: Vec<&Update> = updates.iter().filter(|u| u.kind == Kind::App).collect();
    let mut lines = Vec::new();
    if !named.is_empty() {
        let mut names: Vec<String> = named.iter().take(3).map(|u| format!("{} {}", u.title, u.to)).collect();
        if named.len() > 3 {
            names.push(format!("and {} more", named.len() - 3));
        }
        lines.push(format!("In the store: {}.", names.join(", ")));
    }
    if let Some(s) = system {
        lines.push(format!("IYAKKI {} is out (this is {}). Install it from Settings -> System update.{}", s.to, s.from, if s.note.is_empty() { String::new() } else { format!(" {}", s.note) }));
    }
    lines.push("Nothing is installed by itself: open this to look first.".into());
    let link = if named.is_empty() { Link::new("settings", "tab:updates") } else { Link::new("store", "updates") };
    (title, lines.join("\n"), link)
}

fn running_version() -> String {
    let rel = crate::relinfo::ReleaseInfo::parse(&std::fs::read_to_string(crate::relinfo::RELEASE_FILE).unwrap_or_default());
    if rel.version.is_empty() { crate::relinfo::VERSION.to_string() } else { rel.version }
}

/// Add the announced system version to `rep` (when the check itself did not find a newer one), or forget it when the running
/// version has caught up. The check calls this before it keeps its answer, so the badge and the page count it.
pub fn merge_extra(rep: &mut Report, dir: &Path, running: &str) {
    let mut m = load(dir);
    if m.extra_version.is_empty() {
        return;
    }
    if compare_versions(&m.extra_version, running) != std::cmp::Ordering::Greater {
        m.extra_version.clear();
        m.extra_notes.clear();
        save(dir, &m);
        return;
    }
    if rep.updates.iter().any(|u| u.kind == Kind::System && compare_versions(&u.to, &m.extra_version) != std::cmp::Ordering::Less) {
        return;
    }
    rep.updates.retain(|u| u.kind != Kind::System);
    rep.updates.push(Update { kind: Kind::System, id: "system".into(), title: "IYAKKI".into(), from: running.into(), to: m.extra_version.clone(), source: "Announcement".into(), new_permissions: Vec::new(), note: m.extra_notes.clone(), driver: false });
}

/// Make the notice match what is pending. Call it after a check kept its answer.
pub fn announce_report(svc: &Services, user: &str, rep: &Report) -> Outcome {
    let dir = svc.dir();
    let mut mem = load(dir);
    let mut now: Vec<String> = rep.updates.iter().map(pair).collect();
    now.sort();
    now.dedup();
    let complete = rep.errors.is_empty();
    if now.is_empty() {
        if !complete {
            // a source could not be reached: that is not "nothing is pending"
            return Outcome::Nothing;
        }
        let removed = svc.inbox.remove_keyed(user, KEY).unwrap_or(false);
        if !mem.announced.is_empty() {
            mem.announced.clear();
            save(dir, &mem);
        }
        return if removed { Outcome::Removed } else { Outcome::Nothing };
    }
    let fresh = now.iter().any(|p| !mem.announced.contains(p));
    let mut known = mem.announced.clone();
    known.sort();
    let changed = fresh || (complete && now != known);
    if !changed {
        return Outcome::Nothing;
    }
    let existed = svc.inbox.list(user).iter().any(|n| n.group == format!("keyed:{KEY}"));
    let (title, body, link) = summary(&rep.updates);
    if svc.inbox.upsert_keyed(user, KEY, NoteKind::Info, &title, &body, SOURCE, Some(link), fresh, crate::clock::now_unix()).is_err() {
        return Outcome::Nothing;
    }
    if complete {
        mem.announced = now;
    } else {
        mem.announced.extend(now);
        mem.announced.sort();
        mem.announced.dedup();
    }
    save(dir, &mem);
    if existed { Outcome::Updated } else { Outcome::Posted }
}

/// The hook for the component manifest (and anything else that learns of a newer IYAKKI): record "version X is out" and show
/// it in the badge, on the System update page and in the grouped notice. Does nothing when X is not newer than what runs.
pub fn announce_system_update(svc: &Services, user: &str, version: &str, notes: &str) -> Outcome {
    announce_system_update_for(svc, user, version, notes, &running_version())
}

/// [`announce_system_update`] with the running version given (tests).
pub fn announce_system_update_for(svc: &Services, user: &str, version: &str, notes: &str, running: &str) -> Outcome {
    let version = version.trim();
    let dir = svc.dir();
    if version.is_empty() || compare_versions(version, running) != std::cmp::Ordering::Greater {
        return Outcome::Nothing;
    }
    let mut mem = load(dir);
    mem.extra_version = version.chars().take(40).collect();
    mem.extra_notes = notes.chars().take(300).collect();
    save(dir, &mem);
    let mut rep = Report::load(dir).unwrap_or_default();
    merge_extra(&mut rep, dir, running);
    let _ = rep.save(dir);
    announce_report(svc, user, &rep)
}

/// After an install: rewrite the person's grouped notice for what is still pending (or remove it), without marking it unread. Does
/// nothing when the person has no such notice.
pub fn refresh_existing(svc: &Services, user: &str) {
    if !svc.inbox.list(user).iter().any(|n| n.group == format!("keyed:{KEY}")) {
        return;
    }
    let mut rep = Report::load(svc.dir()).unwrap_or_default();
    rep.errors.clear();
    announce_report(svc, user, &rep);
}
