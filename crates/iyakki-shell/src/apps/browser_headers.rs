//! The Browser's custom request headers, per person: the rule file next to their other browser settings, the values in their personal
//! vault (`webhdr.<id>`), and `apply`, which hands the enabled rules to the transport before a page is requested. The rules, the
//! matching and the refusals are in `iyakki_agent::browserheaders`. The full web engine gets the same rules as the first built-in browser
//! extension (`iyakki_agent::browserext`): `sync_full` hands its helper the current extension snapshot when it changed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_agent::browserheaders::{self as bh, Store, Values};

use super::AppCx;

struct Vault {
    services: Arc<iyakki_agent::services::Services>,
    user: String,
}

fn name(id: &str) -> String {
    format!("webhdr.{id}")
}

impl Values for Vault {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match self.services.user_get(&self.user, &name(id)) {
            Ok(v) => Ok(Some(v.to_string())),
            Err(iyakki_agent::vault::VaultError::NotFound(_)) => Ok(None),
            Err(_) => Err("Your personal vault is locked; sign in with your password.".into()),
        }
    }
    fn set(&self, id: &str, value: &str) -> Result<(), String> {
        self.services.user_set(&self.user, &name(id), value).map_err(|_| "Your personal vault is locked; sign in with your password.".to_string())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        match self.services.user_delete(&self.user, &name(id)) {
            Ok(()) | Err(iyakki_agent::vault::VaultError::NotFound(_)) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The file of one person's rules (the Settings page and the assistant's tools read and write this one).
pub fn rules_path_for(dir: &std::path::Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("browser-headers-{who}.json"))
}

pub fn rules_path(cx: &AppCx) -> PathBuf {
    rules_path_for(cx.services.dir(), &cx.session.user)
}

/// The values are in the person's personal vault.
pub fn vault_values(services: &Arc<iyakki_agent::services::Services>, user: &str) -> Arc<dyn Values> {
    Arc::new(Vault { services: services.clone(), user: user.to_string() })
}

pub fn store(cx: &AppCx) -> Store {
    Store::new(&rules_path(cx), vault_values(&cx.services, &cx.session.user))
}

/// What `apply` last compiled: the rule file's text. A vault that was locked is tried again next time.
static LOADED: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// True when the rule file the transport last got is another text now (and one was loaded before): pages kept in the cache were
/// fetched with the old rules, so they must be asked for again.
pub(super) fn rules_changed(prev: Option<&(PathBuf, String)>, path: &std::path::Path, sig: &str, forced: bool) -> bool {
    prev.is_some_and(|(p, s)| p == path && (forced || s != sig))
}

/// Give the transport this person's enabled rules. Cheap when nothing changed. When the rules changed since the last time, the
/// person's page cache is emptied once, so a page already kept is requested again with the new rules (F-BROW-79).
pub fn apply(cx: &AppCx) {
    let path = rules_path(cx);
    let sig = std::fs::read_to_string(&path).unwrap_or_default();
    let changed = {
        let l = LOADED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let forced = FORCED.swap(false, std::sync::atomic::Ordering::SeqCst);
        if !forced && l.as_ref().is_some_and(|(p, s)| *p == path && *s == sig) {
            return;
        }
        rules_changed(l.as_ref(), &path, &sig, forced)
    };
    let (rules, skipped) = store(cx).compile();
    bh::activate(rules);
    if changed {
        cx.web.clear_cache(&cx.session.user);
    }
    *LOADED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = if skipped == 0 { Some((path, sig)) } else { None };
}

/// Set by `refresh`: the next `apply` reads everything again even when the rule file is the same text (a value in the vault changed).
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make the next `apply` read everything again (after a change in Settings).
pub fn refresh(cx: &AppCx) {
    FORCED.store(true, std::sync::atomic::Ordering::SeqCst);
    apply(cx);
}

/// Before the full web engine loads a page: make sure the person's rules are the active ones and the engine holds the current extension
/// snapshot (a no-op when it already does). The snapshot holds the header values: it goes only over the helper socket.
pub fn sync_full(cx: &AppCx, f: &mut super::browser_full::Full) {
    apply(cx);
    f.sync_extensions();
}
