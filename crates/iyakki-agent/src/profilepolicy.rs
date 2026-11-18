//! What the person chose for their protected browser data beyond switching it on (`docs/development/BROWSER-PROFILE-PROTECTION.md`
//! sections 4.4, 4.6 and D6 to D10): a timed checkpoint (F-VAULT-07), sealing when the screen locks (F-VAULT-08), clearing chosen data
//! when the browser closes and keeping chosen sites out of the sealed file (F-VAULT-09), and the reader browser's data file inside the
//! sealed profile (F-VAULT-10).
//!
//! The choices live in `<data>/web-profile/<user>.policy.json` (site names are in it, no cookies and no keys; it is not sealed so the
//! sign-in hook can read it before the profile opens). All of it applies only to a protected profile.
//!
//! Site matching works on the origin folder names the engine's storage uses (`https_example.com_0.localstorage`, `.../https_example.com_0/`):
//! a site matches its own origins and those of its sub-domains. The rows of `cookies.sqlite` of a site left out are removed with the
//! in-house reader and writer of `sqlmini`; a cookie file it cannot rewrite whole is kept as it is (KI-740).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::profilestore::{ProfileError, ProfileStore};
pub use iyakki_shared::profilepolicy::{Beat, MAX_SITES};

pub use iyakki_shared::profilepolicy::{clean_site, Policy, DEFAULT_CHECKPOINT_MINUTES};

/// The host of an origin folder name of the engine's storage: `https_example.com_0.localstorage` and `https_example.com_0` give
/// `example.com`; `file__0` and anything else give None.
pub fn origin_host(component: &str) -> Option<String> {
    let stem = component.strip_suffix(".localstorage").or_else(|| component.strip_suffix(".sqlite")).or_else(|| component.strip_suffix(".db")).unwrap_or(component);
    let rest = stem.strip_prefix("https_").or_else(|| stem.strip_prefix("http_"))?;
    let (host, port) = rest.rsplit_once('_')?;
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn host_matches(host: &str, site: &str) -> bool {
    host == site || host.strip_suffix(site).is_some_and(|p| p.ends_with('.'))
}

fn is_cookie_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("cookies.sqlite") || n == "cookies.txt" || n.starts_with("cookies.db")
}

/// What a sealed copy leaves out: `exit` is true at sign-out (the clear-on-exit choices count too), false at a checkpoint or a lock.
/// `rel` is the path inside the profile (`data/localstorage/https_example.com_0.localstorage`); a folder that is left out takes everything in it along.
pub fn left_out(policy: &Policy, exit: bool, rel: &str) -> bool {
    let mut sites: Vec<&String> = policy.never_persist.iter().collect();
    if exit {
        sites.extend(policy.clear_sites_on_exit.iter());
    }
    for comp in rel.split('/') {
        if exit && policy.clear_cookies_on_exit && is_cookie_file(comp) {
            return true;
        }
        if let Some(h) = origin_host(comp) {
            if sites.iter().any(|s| host_matches(&h, s)) {
                return true;
            }
        }
    }
    false
}

/// The filter a [`ProfileStore`] seal uses for this person, or None when nothing is left out (the common case).
pub fn seal_filter(data: &Path, user: &str, exit: bool) -> Option<std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>> {
    let p = Policy::load(data, user);
    let any = !p.never_persist.is_empty() || (exit && (p.clear_cookies_on_exit || !p.clear_sites_on_exit.is_empty()));
    any.then(|| std::sync::Arc::new(move |rel: &str| left_out(&p, exit, rel)) as std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>)
}

/// The cookie rows of the sites left out, removed from a cookie database (`cookies.sqlite`): the database is read with [`crate::sqlmini`],
/// the rows whose host column names one of `sites` (or a sub-domain of it, with or without the leading dot) are dropped, and a complete
/// database is written. `None` when nothing has to change, or when the file is not one this can rewrite whole (an index, a view, a log
/// with content in it, a host column it cannot find): the caller then keeps the file as it is, which is what it did before this existed.
pub fn filter_cookie_db(db: &[u8], wal_len: u64, sites: &[&String]) -> Option<Vec<u8>> {
    if sites.is_empty() || wal_len > 0 {
        return None;
    }
    let mut tables = crate::sqlmini::read(db).ok()?;
    let mut changed = false;
    for t in tables.iter_mut() {
        let Some(col) = host_column(&t.sql) else { continue };
        let before = t.rows.len();
        t.rows.retain(|(_, rec)| {
            let host = crate::sqlmini::column_text(rec, col).unwrap_or_default().trim_start_matches('.').to_ascii_lowercase();
            !sites.iter().any(|s| host_matches(&host, s))
        });
        changed |= t.rows.len() != before;
    }
    if !changed {
        return None;
    }
    crate::sqlmini::write(&tables).ok()
}

/// The position of the host column in a `CREATE TABLE` text of a cookie table (`host`, `host_key` or `domain`).
fn host_column(sql: &str) -> Option<usize> {
    let open = sql.find('(')?;
    let close = sql.rfind(')')?;
    let mut depth = 0i32;
    let mut cols: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in sql[open + 1..close].chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                cols.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    cols.push(cur);
    let names: Vec<String> = cols.iter().map(|c| c.trim().split_whitespace().next().unwrap_or("").trim_matches(|ch| ch == '"' || ch == '`' || ch == '[' || ch == ']').to_ascii_lowercase()).collect();
    // a table constraint line is not a column
    if names.iter().any(|n| matches!(n.as_str(), "primary" | "unique" | "check" | "foreign" | "constraint")) {
        return None;
    }
    names.iter().position(|n| matches!(n.as_str(), "host" | "host_key" | "domain"))
}

#[cfg(test)]
pub(crate) fn host_column_for_test(sql: &str) -> Option<usize> {
    host_column(sql)
}

#[cfg(test)]
pub(crate) fn on_checkpoint_for_test(data: &Path, user: &str) {
    crate::profilestore::on_checkpoint(data, user).unwrap();
}

/// The rewrite a [`ProfileStore`] seal uses for this person (the cookies of the sites that are never kept, or cleared at exit), or None.
pub fn seal_rewrite(data: &Path, user: &str, exit: bool) -> Option<std::sync::Arc<dyn Fn(&str, &Path) -> Option<Vec<u8>> + Send + Sync>> {
    let p = Policy::load(data, user);
    let mut sites: Vec<String> = p.never_persist.clone();
    if exit {
        sites.extend(p.clear_sites_on_exit.iter().cloned());
    }
    if sites.is_empty() || (exit && p.clear_cookies_on_exit) {
        return None;
    }
    Some(std::sync::Arc::new(move |rel: &str, path: &Path| {
        let name = rel.rsplit('/').next().unwrap_or("");
        if name != "cookies.sqlite" {
            return None;
        }
        let wal = std::fs::metadata(path.with_file_name("cookies.sqlite-wal")).map(|m| m.len()).unwrap_or(0);
        let db = std::fs::read(path).ok()?;
        let refs: Vec<&String> = sites.iter().collect();
        filter_cookie_db(&db, wal, &refs)
    }))
}

// ------------------------------------------------------------------ the timed checkpoint (F-VAULT-07)

/// A cheap fingerprint of the open profile: the number of files, their sizes and the newest change time. Equal fingerprints
/// mean nothing changed, so the checkpoint is skipped (no rewrite of an unchanged profile).
pub fn fingerprint(dir: &Path) -> u64 {
    fn walk(d: &Path, h: &mut u64, depth: u32) {
        if depth > 40 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut v: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        v.sort_by_key(|e| e.file_name());
        for e in v {
            let Ok(md) = e.metadata() else { continue };
            let t = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
            *h = h.wrapping_mul(1_099_511_628_211).wrapping_add(md.len() ^ t.rotate_left(17) ^ e.file_name().len() as u64);
            if md.is_dir() {
                walk(&e.path(), h, depth + 1);
            }
        }
    }
    let mut h = 14_695_981_039_346_656_037u64;
    for top in ["data", "config"] {
        walk(&dir.join(top), &mut h, 0);
    }
    h
}

#[derive(Default)]
struct Timer {
    last_at: u64,
    last_print: Option<u64>,
    busy: bool,
}

static TIMERS: Mutex<Option<HashMap<PathBuf, Timer>>> = Mutex::new(None);


/// Called about once a second with the clock in seconds. Writes the open profile back when the person's interval has passed and
/// something changed. Runs on the caller's thread: use [`tick_in_background`] from the shell.
pub fn tick(data: &Path, user: &str, now: u64) -> Beat {
    let st = ProfileStore::new(data, user);
    if st.status() != crate::profilestore::Status::Open {
        return Beat::Nothing;
    }
    let policy = Policy::load(data, user);
    if policy.checkpoint_minutes == 0 {
        return Beat::Nothing;
    }
    let key = st.profile_path();
    let dir = std::fs::canonicalize(&key).unwrap_or(key.clone());
    {
        let mut g = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
        let t = g.get_or_insert_with(HashMap::new).entry(key.clone()).or_default();
        if t.last_at == 0 {
            t.last_at = now; // the clock starts when the profile is first seen open
            t.last_print = Some(fingerprint(&dir));
            return Beat::Nothing;
        }
        if t.busy || now.saturating_sub(t.last_at) < u64::from(policy.checkpoint_minutes) * 60 {
            return Beat::Nothing;
        }
        t.busy = true;
    }
    let print = fingerprint(&dir);
    let same = TIMERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)).and_then(|t| t.last_print) == Some(print);
    let r = if same { Ok(false) } else { crate::profilestore::on_checkpoint(data, user).map(|_| true) };
    let mut g = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
    let t = g.get_or_insert_with(HashMap::new).entry(key).or_default();
    t.busy = false;
    t.last_at = now;
    match r {
        Ok(true) => {
            t.last_print = Some(print);
            Beat::Written
        }
        Ok(false) => Beat::Unchanged,
        Err(e) => Beat::Failed(e.to_string()),
    }
}

/// The shell's call: the beat runs on its own thread (a big profile takes a while to write), results go to `report`.
pub fn tick_in_background(data: &Path, user: &str, now: u64, report: impl Fn(&Beat) + Send + 'static) {
    renew_claim_often(data, user, now);
    let (d, u) = (data.to_path_buf(), user.to_string());
    // a cheap look first so a thread is not started once a second for nothing
    let st = ProfileStore::new(data, user);
    if st.status() != crate::profilestore::Status::Open {
        return;
    }
    let minutes = Policy::load(data, user).checkpoint_minutes;
    if minutes == 0 {
        return;
    }
    let seen = TIMERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&st.profile_path()).map(|t| (t.last_at, t.busy)));
    if let Some((last, busy)) = seen {
        if busy || last == 0 || now.saturating_sub(last) < u64::from(minutes) * 60 {
            return;
        }
    }
    let _ = std::thread::Builder::new().name("profile-checkpoint".into()).spawn(move || {
        let b = tick(&d, &u, now);
        if matches!(b, Beat::Written | Beat::Failed(_)) {
            report(&b);
        }
    });
}

static LAST_RENEW: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

/// While the profile is open its session claim is renewed about every 40 seconds (it runs out after two minutes, so a crashed machine
/// does not hold the profile for long). One small file write; done on the caller's thread.
fn renew_claim_often(data: &Path, user: &str, now: u64) {
    let st = ProfileStore::new(data, user);
    let key = st.profile_path();
    {
        let mut g = LAST_RENEW.lock().unwrap_or_else(|e| e.into_inner());
        let last = g.get_or_insert_with(HashMap::new).entry(key).or_insert(0);
        if now.saturating_sub(*last) < 40 {
            return;
        }
        *last = now;
    }
    if st.status() == crate::profilestore::Status::Open {
        let _ = crate::profilestore::renew_claim(data, user, &crate::profilestore::holder_id(data), now);
    }
}

/// Forget the timer of a person (sign-out, lock).
pub fn timer_reset(data: &Path, user: &str) {
    let key = ProfileStore::new(data, user).profile_path();
    if let Some(m) = TIMERS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&key);
    }
}

// ------------------------------------------------------------------ the lock screen (F-VAULT-08)

/// The screen locks: when the person chose it, seal the profile and remove the decrypted copy. The key stays in memory (the same
/// lifetime as the vault's data key) so the unlock needs no second password. Returns true when it sealed.
pub fn on_screen_lock(data: &Path, user: &str) -> Result<bool, ProfileError> {
    if !Policy::load(data, user).lock_seal {
        return Ok(false);
    }
    let st = ProfileStore::new(data, user);
    if st.status() != crate::profilestore::Status::Open {
        return Ok(false);
    }
    timer_reset(data, user);
    crate::profilestore::seal_keep_key(data, user)
}

/// The password was right: open the sealed profile again, if the lock sealed it. `key` is the vault's profile key when the held one is gone.
pub fn on_screen_unlock(data: &Path, user: &str, key: Option<crate::profilestore::ProfileKey>) -> Result<bool, ProfileError> {
    if !Policy::load(data, user).lock_seal {
        return Ok(false);
    }
    let st = ProfileStore::new(data, user);
    if st.status() != crate::profilestore::Status::Sealed {
        return Ok(false);
    }
    crate::profilestore::reopen(data, user, key)
}

// ------------------------------------------------------------------ the reader browser's data (F-VAULT-10)

/// Where the reader browser keeps its data file for this person: the file name next to the data folder while protection is off, a file
/// inside the profile's `data` folder (so it is sealed with the cookies) while protection is on and the profile is open, and None while
/// it is sealed (nothing is read or written beside the sealed file). `file_user` is the name the plain file has always carried.
pub fn reader_data_path(data: &Path, user: &str, file_user: &str) -> Option<PathBuf> {
    let plain = data.join(format!("browser-data-{file_user}.json"));
    let st = ProfileStore::new(data, user);
    match st.status() {
        crate::profilestore::Status::Off => Some(plain),
        crate::profilestore::Status::Open => {
            let inside = st.profile_path().join("data").join("reader").join("browser-data.json");
            if plain.is_file() && !inside.exists() {
                // the plain file moves in once: copy, then remove it with the usual overwrite
                if let Some(d) = inside.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                if std::fs::copy(&plain, &inside).is_ok() {
                    crate::profilestore::secure_remove(&plain);
                }
            }
            Some(inside)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "profilepolicy_tests.rs"]
mod tests;
