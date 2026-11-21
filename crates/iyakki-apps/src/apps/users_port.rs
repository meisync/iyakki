//! What the USERS tile asks of the rest of the system (the sixth tile moved onto the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`users.rs`) holds no account store, no `Services`, no `AppCx` and no audit log: it asks a `UsersPort`. The real port ([`CxPort`]) is the
//! only place that locks the store and hands it the acting session (the store re-checks the actor's permission on every change), wipes a person's
//! vault when their password is reset or the account is deleted, and writes the audit lines.

use std::path::PathBuf;

use iyakki_sec::policy::{PolicyOverride, Role};

use super::AppCx;
use crate::accounts::{AccountError, UserRecord};

pub trait UsersPort {
    /// The signed-in person (the actor of every change).
    fn me(&self) -> String;
    fn can_manage_users(&self) -> bool;
    fn users(&self) -> Vec<UserRecord>;
    fn create_user(&self, name: &str, password: &str, role: Role) -> Result<(), AccountError>;
    /// Reset another person's password. On success their personal vault is closed and wiped (the old password cannot open it, and an administrator must not read it).
    fn set_password(&self, name: &str, password: &str) -> Result<(), AccountError>;
    fn set_role(&self, name: &str, role: Role) -> Result<(), AccountError>;
    fn set_overrides(&self, name: &str, ov: PolicyOverride) -> Result<(), AccountError>;
    /// Delete an account. On success its personal vault is wiped.
    fn delete_user(&self, name: &str) -> Result<(), AccountError>;
    fn audit(&self, action: &str, subject: &str);
    /// The number of entries when the audit chain is intact, else the place where it breaks.
    fn audit_verify(&self) -> Result<usize, usize>;
    fn audit_recent(&self, n: usize) -> Vec<String>;
    /// Start a new audit log; the broken one is kept aside, never deleted. Returns where it was kept.
    fn audit_start_new_log(&self) -> Result<PathBuf, String>;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    fn store(&self) -> crate::accounts::UsersGuard {
        self.0.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl UsersPort for CxPort<'_> {
    fn me(&self) -> String {
        self.0.session.user.clone()
    }
    fn can_manage_users(&self) -> bool {
        self.0.session.policy.manage_users
    }
    fn users(&self) -> Vec<UserRecord> {
        self.0.store.lock().map(|s| s.list()).unwrap_or_default()
    }
    fn create_user(&self, name: &str, password: &str, role: Role) -> Result<(), AccountError> {
        self.store().create_user(&self.0.session, name, password, role)
    }
    fn set_password(&self, name: &str, password: &str) -> Result<(), AccountError> {
        let r = self.store().set_password(&self.0.session, name, None, password);
        if r.is_ok() {
            let _ = self.0.vault().reset_user_vault(name);
        }
        r
    }
    fn set_role(&self, name: &str, role: Role) -> Result<(), AccountError> {
        self.store().set_role(&self.0.session, name, role)
    }
    fn set_overrides(&self, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.store().set_overrides(&self.0.session, name, ov)
    }
    fn delete_user(&self, name: &str) -> Result<(), AccountError> {
        let r = self.store().delete_user(&self.0.session, name);
        if r.is_ok() {
            let _ = self.0.vault().reset_user_vault(name);
        }
        r
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn audit_verify(&self) -> Result<usize, usize> {
        self.0.audit.verify()
    }
    fn audit_recent(&self, n: usize) -> Vec<String> {
        self.0.audit.recent(n)
    }
    fn audit_start_new_log(&self) -> Result<PathBuf, String> {
        self.0.audit.start_new_log()
    }
}
