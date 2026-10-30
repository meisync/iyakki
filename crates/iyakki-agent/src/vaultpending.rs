//! Passwords that wait for a vault.
//!
//! A Quick install makes a Secret in the cluster and leaves the shell to move the password into the person's vault. While the vault is
//! locked (it opens only with the password typed at sign-in, or Settings → Vault → Unlock) the move cannot happen, so it waits.
//! The password stays in the cluster Secret, and this module keeps
//! a list of WHERE to read it (namespace, Secret, key, vault name): never a value. Whenever the vault opens, the list is worked off
//! without installing anything again. Each outcome is written to the job's log, the audit and the inbox, in words, never with a value.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The Quick install item id (`clickhouse`, `matrix`, `graph-db`, `gitea`).
    pub item: String,
    pub user: String,
    /// "ClickHouse", "Matrix registration token": the words of the notices.
    pub what: String,
    pub namespace: String,
    pub secret: String,
    pub key: String,
    /// The vault entry to fill.
    pub vault: String,
    /// What DASH calls the source that uses it (empty for none).
    pub source: String,
}

fn file(data: &Path) -> PathBuf {
    crate::quickinstall::state_dir(data).join("pending-vault.json")
}

pub fn list(data: &Path) -> Vec<Pending> {
    std::fs::read_to_string(file(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(data: &Path, all: &[Pending]) -> Result<(), String> {
    std::fs::create_dir_all(crate::quickinstall::state_dir(data)).map_err(|e| e.to_string())?;
    let t = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(file(data), t).map_err(|e| e.to_string())
}

/// Add (or replace the same user and vault entry).
pub fn add(data: &Path, p: Pending) -> Result<(), String> {
    let mut all = list(data);
    all.retain(|x| !(x.user == p.user && x.vault == p.vault));
    all.push(p);
    save(data, &all)
}

pub fn remove(data: &Path, user: &str, vault: &str) -> Result<(), String> {
    let mut all = list(data);
    let n = all.len();
    all.retain(|x| !(x.user == user && x.vault == vault));
    if all.len() == n { Ok(()) } else { save(data, &all) }
}

/// The notice (toast, inbox, card note).
pub fn waiting_words(p: &Pending) -> String {
    format!("The {} password is waiting for your vault: sign in with your password or open Settings \u{2192} Vault \u{2192} Unlock", p.what)
}

/// The card's short note for an item ("" when nothing waits for it).
pub fn card_note(data: &Path, item: &str) -> String {
    if list(data).iter().any(|p| p.item == item) { "password waiting for your vault".into() } else { String::new() }
}

/// What the world gives the finisher (fakes in tests).
pub struct Hooks<'a> {
    pub unlocked: &'a dyn Fn(&str) -> bool,
    /// Read the Secret, store the value, check the entry is there: one plain line, or why not.
    pub move_one: &'a dyn Fn(&Pending) -> Result<String, String>,
    /// `(user, action, detail)`: the audit (never a value).
    pub audit: &'a dyn Fn(&str, &str, &str),
    /// `(pending, line)`: appended to the item's job log.
    pub joblog: &'a dyn Fn(&Pending, &str),
}

/// Work off every pending entry whose person's vault is open now; an entry is removed only after its move worked (a second call
/// finds nothing to do). One line per entry done.
pub fn finish_all(data: &Path, h: &Hooks) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    for p in list(data) {
        if !(h.unlocked)(&p.user) {
            continue;
        }
        match (h.move_one)(&p) {
            Ok(line) => {
                let _ = remove(data, &p.user, &p.vault);
                (h.joblog)(&p, &format!("[vault] stored {} in your vault (finished after sign-in)", p.vault));
                (h.audit)(&p.user, "quickinstall.vault.stored", &format!("{} {}", p.item, p.vault));
                log::info!("quick install vault handoff: {} stored as {}", p.item, p.vault);
                out.push(Ok(line));
            }
            Err(e) => log::warn!("quick install vault handoff for {} still waiting: {e}", p.item),
        }
    }
    out
}

/// A handoff gave up: remember where the password is, say so in the job log, the audit and the shell log; returns the notice text.
pub fn record_waiting(data: &Path, p: Pending, tries: u32, why: &str, h: &Hooks) -> String {
    let words = waiting_words(&p);
    let _ = add(data, p.clone());
    let line = if tries == 0 { format!("[vault] waiting for your vault: {why}; the password is waiting in the cluster Secret {} (namespace {})", p.secret, p.namespace) } else { format!("[vault] gave up after {tries} tries: {why}; the password is waiting in the cluster Secret {} (namespace {})", p.secret, p.namespace) };
    (h.joblog)(&p, &line);
    (h.audit)(&p.user, "quickinstall.vault.waiting", &format!("{} {}", p.item, p.vault));
    log::warn!("quick install vault handoff for {} gave up after {tries} tries: {why}", p.item);
    words
}

/// A handoff worked: say so in the job log, the audit and the shell log.
pub fn record_stored(data: &Path, p: &Pending, h: &Hooks) {
    let _ = remove(data, &p.user, &p.vault);
    (h.joblog)(p, &format!("[vault] stored {} in your vault", p.vault));
    (h.audit)(&p.user, "quickinstall.vault.stored", &format!("{} {}", p.item, p.vault));
    log::info!("quick install vault handoff: {} stored as {}", p.item, p.vault);
}

/// The move itself for a plain Secret key: read it (a closure over kubectl), store it (a closure over the vault), check the name is there.
pub fn move_secret(p: &Pending, read: &dyn Fn(&Pending) -> Result<Zeroizing<String>, String>, store: &dyn Fn(&str, &str, &str) -> Result<(), String>, has: &dyn Fn(&str, &str) -> bool) -> Result<String, String> {
    let v = read(p).map_err(|e| format!("could not read the Secret {}: {e}", p.secret))?;
    if v.trim().is_empty() {
        return Err(format!("the Secret {} holds no {}", p.secret, p.key));
    }
    store(&p.user, &p.vault, v.trim()).map_err(|e| format!("could not put it in your vault: {e}"))?;
    if !has(&p.user, &p.vault) {
        return Err(format!("{} is not in your vault after saving", p.vault));
    }
    Ok(format!("the {} password is in your vault as {}", p.what, p.vault))
}

#[cfg(test)]
#[path = "vaultpending_tests.rs"]
mod tests;
