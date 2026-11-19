//! The AUTH tile's data: one-time-code entries (issuer, account, secret, hash, digits, step, group) and the groups they sit in. All
//! of it, secrets and names alike, is ONE secret in the person's own vault (`auth.codes`, JSON), so nothing about the accounts is
//! on disk in the clear and the vault's password protects it all. The vault is unlocked by the person's sign-in password; when it
//! is locked the tile says so and shows nothing.
//!
//! [`Book`] is the pure model (add, edit, move, delete, groups, the code now); [`CodeVault`] is where it is kept: the real one is
//! [`ServicesVault`], tests use a fake. Nothing here logs and no message contains a secret or a code. The secrets are zeroized when
//! an entry is dropped.

use std::sync::Arc;

pub use iyakki_shared::authcodes::*;

/// The real place: the signed-in person's own vault.
pub struct ServicesVault {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl CodeVault for ServicesVault {
    fn load(&self) -> Result<Book, VaultProblem> {
        if !self.svc.user_vault_unlocked(&self.user) {
            return Err(VaultProblem::Locked);
        }
        match self.svc.user_get(&self.user, VAULT_NAME) {
            Ok(t) => Book::parse(&t).map_err(VaultProblem::Other),
            Err(iyakki_shared::vault::VaultError::NotFound(_)) => Ok(Book::default()),
            Err(iyakki_shared::vault::VaultError::Locked) => Err(VaultProblem::Locked),
            Err(e) => Err(VaultProblem::Other(e.to_string())),
        }
    }

    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        let text = book.to_text().map_err(VaultProblem::Other)?;
        self.svc.user_set(&self.user, VAULT_NAME, &text).map_err(|e| match e {
            iyakki_shared::vault::VaultError::Locked => VaultProblem::Locked,
            other => VaultProblem::Other(other.to_string()),
        })
    }
}


impl crate::services::Services {
    /// The signed-in person's own vault as the place their one-time codes are kept (`AuthPort::vault`).
    pub fn code_vault(self: &Arc<Self>, user: &str) -> Box<dyn CodeVault> {
        Box::new(ServicesVault { svc: self.clone(), user: user.to_string() })
    }
}

#[cfg(test)]
#[path = "authcodes_tests.rs"]
mod tests;
