//! The Browser's custom request headers, per person: the rule file next to their other browser settings, the values in their personal
//! vault (`webhdr.<id>`), and `apply`, which hands the enabled rules to the transport before a page is requested. The rules, the
//! matching and the refusals are in `iyakki_agent::browserheaders`. The full web engine does not get these headers yet.

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

pub fn rules_path(cx: &AppCx) -> PathBuf {
    cx.services.dir().join(format!("browser-headers-{}.json", super::browser_config::who(cx)))
}

pub fn store(cx: &AppCx) -> Store {
    Store::new(&rules_path(cx), Arc::new(Vault { services: cx.services.clone(), user: cx.session.user.clone() }))
}

/// What `apply` last compiled: the rule file's text. A vault that was locked is tried again next time.
static LOADED: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// Give the transport this person's enabled rules. Cheap when nothing changed.
pub fn apply(cx: &AppCx) {
    let path = rules_path(cx);
    let sig = std::fs::read_to_string(&path).unwrap_or_default();
    if let Ok(l) = LOADED.lock() {
        if l.as_ref().is_some_and(|(p, s)| *p == path && *s == sig) {
            return;
        }
    }
    let (rules, skipped) = store(cx).compile();
    bh::activate(rules);
    if let Ok(mut l) = LOADED.lock() {
        *l = if skipped == 0 { Some((path, sig)) } else { None };
    }
}

/// Make the next `apply` read everything again (after a change in Settings).
pub fn refresh(cx: &AppCx) {
    if let Ok(mut l) = LOADED.lock() {
        *l = None;
    }
    apply(cx);
}
