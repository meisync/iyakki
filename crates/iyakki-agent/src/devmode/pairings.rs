//! The device's side of the pairings: which computers are paired (age, last use, what each may do) and a plain "Remove this computer"
//! that says what it takes away. Settings, Developer mode and the chat tool (`devtools`) both use these, so they say the same thing.

use iyakki_sec::policy::Policy;

use super::sessions::{self, SessionInfo};
use super::DevMode;
pub use iyakki_shared::devmode_types::pairings::{Pairing};



impl DevMode {
    /// Every paired computer, oldest pairing first.
    pub fn pairings(&self) -> Vec<Pairing> {
        let auto = self.shell_auto_list();
        let restarts = self.restart_grant_list();
        let open = sessions::list(&self.data);
        let mut v: Vec<Pairing> = self
            .keys()
            .into_iter()
            .map(|k| Pairing {
                scopes: k.scope_list().iter().map(|s| s.id().to_string()).collect(),
                api_scopes: self.api_scopes(&k.name),
                core_scopes: self.core_scopes(&k.name),
                shell_auto_until: auto.iter().find(|a| a.computer == k.name).map(|a| a.until),
                restart_grant: restarts.iter().any(|r| r.computer == k.name),
                sessions: open.iter().filter(|s| s.computer == k.name).cloned().collect(),
                key_short: k.short(),
                name: k.name,
                account: k.account,
                added: k.added,
                last_used: k.last_used,
            })
            .collect();
        v.sort_by_key(|p| p.added);
        v
    }

    /// One pairing by name.
    pub fn paired_computer(&self, name: &str) -> Option<Pairing> {
        self.pairings().into_iter().find(|p| p.name == name)
    }

    /// Remove a paired computer (administrators, on the device) and say what went. The same as [`DevMode::revoke`], with the sentence.
    pub fn remove_pairing(&self, policy: &Policy, user: &str, name: &str) -> Result<String, String> {
        Self::need_admin(policy)?;
        let before = self.paired_computer(name).ok_or_else(|| format!("no paired computer called {name}"))?;
        self.revoke(policy, user, name)?;
        Ok(format!("Removed {name}: that took away {}. To use it again it must pair again with a new code.", before.takes_away()))
    }
}

/// "3 days ago" for the lists (the same wording as `iyakki dev devices`).
pub fn ago(now: i64, t: i64) -> String {
    super::cli_devices::ago(now, t)
}
