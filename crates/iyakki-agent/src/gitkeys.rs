//! The agent's part of `iyakki_shared::gitkeys` (the module moved down for F-SPLIT-03): the vault of the signed-in person as the secret store.

use std::sync::Arc;

use zeroize::Zeroizing;

pub use iyakki_shared::gitkeys::*;

/// The signed-in person's vault, through the services.
pub struct ServicesStore {
    svc: Arc<crate::services::Services>,
    user: String,
}

impl ServicesStore {
    pub fn new(svc: &Arc<crate::services::Services>, user: &str) -> ServicesStore {
        ServicesStore { svc: svc.clone(), user: user.into() }
    }
}

impl SecretStore for ServicesStore {
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        self.svc.user_get(&self.user, name).map_err(|e| format!("could not read `{name}` from your vault: {e}"))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.svc.user_set(&self.user, name, value).map_err(|e| format!("could not write `{name}` to your vault: {e}"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.svc.user_delete(&self.user, name).map_err(|e| format!("could not delete `{name}` from your vault: {e}"))
    }
    fn names(&self) -> Result<Vec<String>, String> {
        self.svc.user_names(&self.user).map_err(|e| format!("could not list your vault: {e}"))
    }
}

impl crate::services::Services {
    /// The git keys of the signed-in person: the same folder as the CLI tile's SSH settings, secrets in their vault.
    pub fn git_keys(self: &Arc<Self>, user: &str, audit: AuditFn, session: Arc<Session>) -> GitKeys {
        let env = crate::ssh::Env::for_services(self, user);
        GitKeys::new(&env.dir, Arc::new(ServicesStore::new(self, user)), audit, session)
    }
}

#[cfg(test)]
#[path = "gitkeys_tests.rs"]
mod tests;
