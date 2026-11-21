//! Refreshing kept web pages by itself on a timer (F-RAG-17).
//!
//! Never by default: a collection is read again on a timer only after the person (or an administrator, for a shared collection) chose
//! an interval for THAT collection with `rag_refresh_schedule` (it asks every time). The choice is one small file in the data folder
//! (`ragrefresh.json`, mode 0600): collection namespace -> hours between refreshes and the time of the last one. A tick asks
//! "is anything due?" (cheap, reads the file) and, only then, does the same refresh `rag_add_page refresh` does: every kept page of the
//! collection (at most 20) read again with the account's own web access, its egress rules and its request budget, the changed ones
//! replaced. Nothing new is crawled or added. One audit line per run with the counts, never an address.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ragtools::RagToolsEnv;

/// The shortest and the longest interval a person may choose, in hours.
pub const MIN_HOURS: u64 = 1;
pub const MAX_HOURS: u64 = 24 * 30;
/// The most collections that may have a timer.
pub const MAX_SCHEDULES: usize = 20;
/// How often a tick looks at the file at all, in seconds.
pub const LOOK_EVERY_SECS: u64 = 600;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Entry {
    pub every_hours: u64,
    /// Unix seconds of the last timed run, 0 when none yet (the first run is one interval after the choice).
    pub last: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    schedules: BTreeMap<String, Entry>,
}

/// The clock, in unix seconds.
pub fn now_secs() -> u64 {
    crate::clock::now_unix().max(0) as u64
}

fn path(dir: &Path) -> PathBuf {
    dir.join("ragrefresh.json")
}

fn lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

fn load(dir: &Path) -> File {
    std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(dir: &Path, f: &File) -> Result<(), String> {
    use std::io::Write;
    let tmp = path(dir).with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(&tmp).and_then(|mut h| h.write_all(&bytes)).map_err(|e| format!("could not keep the choice: {e}"))?;
    std::fs::rename(&tmp, path(dir)).map_err(|e| format!("could not keep the choice: {e}"))
}

/// Choose how often the kept pages of the collection `ns` are read again; 0 (or [`never`]) removes the timer, which is the default.
/// `now` starts the first interval.
pub fn set(dir: &Path, ns: &str, every_hours: u64, now: u64) -> Result<(), String> {
    if every_hours != 0 && !(MIN_HOURS..=MAX_HOURS).contains(&every_hours) {
        return Err(format!("choose 0 (never) or between {MIN_HOURS} and {MAX_HOURS} hours"));
    }
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(dir);
    if every_hours == 0 {
        f.schedules.remove(ns);
    } else {
        if !f.schedules.contains_key(ns) && f.schedules.len() >= MAX_SCHEDULES {
            return Err(format!("at most {MAX_SCHEDULES} collections can have a timer; remove one first"));
        }
        f.schedules.insert(ns.to_string(), Entry { every_hours, last: now });
    }
    save(dir, &f)
}

/// The interval chosen for `ns`: 0 when none (never).
pub fn hours_of(dir: &Path, ns: &str) -> u64 {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    load(dir).schedules.get(ns).map(|e| e.every_hours).unwrap_or(0)
}

/// Every collection with a timer.
pub fn all(dir: &Path) -> Vec<(String, Entry)> {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    load(dir).schedules.into_iter().collect()
}

/// The collections whose interval has passed at `now`.
pub fn due(dir: &Path, now: u64) -> Vec<String> {
    all(dir).into_iter().filter(|(_, e)| e.every_hours > 0 && now.saturating_sub(e.last) >= e.every_hours * 3600).map(|(ns, _)| ns).collect()
}

fn mark(dir: &Path, ns: &str, now: u64) {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut f = load(dir);
    if let Some(e) = f.schedules.get_mut(ns) {
        e.last = now;
        let _ = save(dir, &f);
    }
}

/// One tick for a signed-in person: every due collection this person may see (`env.access`; a shared one only for an administrator) is
/// refreshed once. A failed run still counts as a run (the next try is one interval later, so a dead site is not hammered). Returns how
/// many collections were refreshed.
pub fn run_due(env: &RagToolsEnv, dir: &Path, now: u64) -> usize {
    let mut n = 0;
    for ns in due(dir, now) {
        let Some(public) = env.access.public(&ns) else { continue };
        if public.starts_with("shared") && !env.manage_shared {
            continue;
        }
        mark(dir, &ns, now);
        let (read, changed, _same, failed) = env.refresh_pages(&ns, &public);
        (env.audit)("rag.refresh_timer", &format!("{public}: {read} read, {changed} changed, {} failed", failed.len()));
        n += 1;
    }
    n
}

/// Called from the shell's heartbeat for the signed-in person: looks at the file at most every [`LOOK_EVERY_SECS`] and never runs two
/// rounds at once; a due round runs on its own thread. Returns whether one was started.
pub fn tick(svc: &std::sync::Arc<crate::services::Services>, user: &str, policy: &iyakki_sec::policy::Policy, audit: crate::gitkeys::AuditFn) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    static SLOTS: Mutex<Option<BTreeMap<String, (u64, Arc<AtomicBool>)>>> = Mutex::new(None);
    let now = now_secs();
    let dir = svc.dir().to_path_buf();
    let key = format!("{}:{user}", dir.display());
    let busy = {
        let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(Default::default);
        let slot = m.entry(key).or_insert((0, Arc::new(AtomicBool::new(false))));
        if slot.1.load(Ordering::SeqCst) || now.saturating_sub(slot.0) < LOOK_EVERY_SECS {
            return false;
        }
        slot.0 = now;
        if due(&dir, now).is_empty() {
            return false;
        }
        slot.1.store(true, Ordering::SeqCst);
        slot.1.clone()
    };
    let env = RagToolsEnv::for_refresh(svc, user, policy, audit);
    let spawned = std::thread::Builder::new().name("iyakki-rag-refresh".into()).spawn({
        let busy = busy.clone();
        move || {
            run_due(&env, &dir, now_secs());
            busy.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        busy.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-ragrefresh-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn nothing_has_a_timer_until_one_is_chosen_and_zero_removes_it() {
        let d = tmp("default");
        assert_eq!(hours_of(&d, "user/alice/web"), 0);
        assert!(due(&d, u64::MAX / 2).is_empty());
        set(&d, "user/alice/web", 24, 1000).unwrap();
        assert_eq!(hours_of(&d, "user/alice/web"), 24);
        set(&d, "user/alice/web", 0, 2000).unwrap();
        assert_eq!(hours_of(&d, "user/alice/web"), 0);
        assert!(all(&d).is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_interval_is_bounded_and_the_number_of_timers_is_capped() {
        let d = tmp("bounds");
        assert!(set(&d, "user/a/x", MAX_HOURS + 1, 0).is_err());
        assert!(set(&d, "user/a/x", MAX_HOURS, 0).is_ok());
        assert!(set(&d, "user/a/x", MIN_HOURS, 0).is_ok());
        for i in 1..MAX_SCHEDULES {
            set(&d, &format!("user/a/c{i}"), 5, 0).unwrap();
        }
        assert_eq!(all(&d).len(), MAX_SCHEDULES);
        assert!(set(&d, "user/a/one-too-many", 5, 0).is_err());
        assert!(set(&d, "user/a/c1", 6, 0).is_ok(), "changing an existing timer is allowed at the cap");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_collection_is_due_one_interval_after_the_choice_and_after_each_run() {
        let d = tmp("due");
        set(&d, "user/a/web", 2, 10_000).unwrap();
        assert!(due(&d, 10_000 + 7199).is_empty());
        assert_eq!(due(&d, 10_000 + 7200), vec!["user/a/web".to_string()]);
        mark(&d, "user/a/web", 20_000);
        assert!(due(&d, 20_000 + 7199).is_empty());
        assert_eq!(due(&d, 20_000 + 7200).len(), 1);
        // a clock that went backwards is not "due"
        assert!(due(&d, 5).is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_damaged_file_means_no_timers() {
        let d = tmp("damaged");
        std::fs::write(path(&d), b"{ not json").unwrap();
        assert!(all(&d).is_empty());
        set(&d, "user/a/web", 3, 0).unwrap();
        assert_eq!(hours_of(&d, "user/a/web"), 3);
        let _ = std::fs::remove_dir_all(d);
    }
}
