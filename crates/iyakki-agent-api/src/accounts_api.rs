//! The accounts as the interface that crosses the core socket (F-SPLIT-03, D-SPLIT-03-HANDLE). `AccountsApi` is generated from the store
//! (`dist/split/gen_store_api.py`); this file adds the two hand-written parts: the adapter that makes a served `AccountsApi` the source of a
//! [`UsersRef`], and [`AccountsGuarded`], what the core hands out as the root.

use std::sync::Arc;

use iyakki_accounts::{AccountError, Session, UserRecord, UserSource};
use iyakki_sec::policy::{PolicyOverride, Role};

pub use iyakki_accounts::{UsersGuard, UsersRef};

use crate::member_stores::AccountsApi;

/// A served `AccountsApi` as the source of a [`UsersRef`].
pub struct UsersSrc(pub Arc<dyn AccountsApi>);

impl UserSource for UsersSrc {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn list(&self) -> Vec<UserRecord> {
        self.0.list_public()
    }
    fn get(&self, name: &str) -> Option<UserRecord> {
        self.0.get_public(name)
    }
    fn authenticate(&self, name: &str, password: &str) -> Result<Session, AccountError> {
        self.0.authenticate(name, password)
    }
    fn resume_session(&self, name: &str) -> Option<Session> {
        self.0.resume_session(name)
    }
    fn create_first_admin(&self, name: &str, password: &str) -> Result<(), AccountError> {
        self.0.create_first_admin(name, password)
    }
    fn create_user(&self, actor: &Session, name: &str, password: &str, role: Role) -> Result<(), AccountError> {
        self.0.create_user(actor, name, password, role)
    }
    fn delete_user(&self, actor: &Session, name: &str) -> Result<(), AccountError> {
        self.0.delete_user(actor, name)
    }
    fn set_role(&self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        self.0.set_role(actor, name, role)
    }
    fn set_overrides(&self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.0.set_overrides(actor, name, ov)
    }
    fn set_password(&self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        self.0.set_password(actor, name, old, new)
    }
    fn refresh(&self) {
        self.0.refresh()
    }
}

/// The accounts the core serves: the same calls as the store, with one rule added. A `Session` that arrives over a connection is only a claim:
/// the core takes the actor's NAME, reads the actor's role and policy again from its own store, and ignores the role and policy in the `Session`
/// it was sent (an actor that does not exist is `Forbidden`). The list and the single account never carry the password hash (the store's own
/// `list_public` and `get_public`). `add_local` is not on the interface at all.
pub struct AccountsGuarded(pub Arc<dyn AccountsApi>);

impl AccountsGuarded {
    fn actor(&self, claimed: &Session) -> Result<Session, AccountError> {
        let rec = self.0.get_public(&claimed.user).ok_or(AccountError::Forbidden)?;
        Ok(Session { user: rec.name.clone(), role: rec.role, policy: rec.policy() })
    }
}

impl AccountsApi for AccountsGuarded {
    fn refresh(&self) {
        self.0.refresh()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn list_public(&self) -> Vec<UserRecord> {
        self.0.list_public()
    }
    fn get_public(&self, name: &str) -> Option<UserRecord> {
        self.0.get_public(name)
    }
    fn create_first_admin(&self, name: &str, pw: &str) -> Result<(), AccountError> {
        self.0.create_first_admin(name, pw)
    }
    fn resume_session(&self, name: &str) -> Option<Session> {
        self.0.resume_session(name)
    }
    fn authenticate(&self, name: &str, pw: &str) -> Result<Session, AccountError> {
        self.0.authenticate(name, pw)
    }
    fn create_user(&self, actor: &Session, name: &str, pw: &str, role: Role) -> Result<(), AccountError> {
        self.0.create_user(&self.actor(actor)?, name, pw, role)
    }
    fn delete_user(&self, actor: &Session, name: &str) -> Result<(), AccountError> {
        self.0.delete_user(&self.actor(actor)?, name)
    }
    fn set_role(&self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        self.0.set_role(&self.actor(actor)?, name, role)
    }
    fn set_overrides(&self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.0.set_overrides(&self.actor(actor)?, name, ov)
    }
    fn set_password(&self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        self.0.set_password(&self.actor(actor)?, name, old, new)
    }
}

/// A [`UsersRef`] (what the shell holds) as the served `AccountsApi`: the core serves the very accounts its own shell uses.
/// `list_public` and `get_public` are the reference's `list` and `get` (already without the hash).
pub struct UsersAsApi(pub UsersRef);

impl AccountsApi for UsersAsApi {
    fn refresh(&self) {
        self.0 .0.refresh()
    }
    fn is_empty(&self) -> bool {
        self.0 .0.is_empty()
    }
    fn list_public(&self) -> Vec<UserRecord> {
        self.0 .0.list()
    }
    fn get_public(&self, name: &str) -> Option<UserRecord> {
        self.0 .0.get(name)
    }
    fn create_first_admin(&self, name: &str, pw: &str) -> Result<(), AccountError> {
        self.0 .0.create_first_admin(name, pw)
    }
    fn resume_session(&self, name: &str) -> Option<Session> {
        self.0 .0.resume_session(name)
    }
    fn authenticate(&self, name: &str, pw: &str) -> Result<Session, AccountError> {
        self.0 .0.authenticate(name, pw)
    }
    fn create_user(&self, actor: &Session, name: &str, pw: &str, role: Role) -> Result<(), AccountError> {
        self.0 .0.create_user(actor, name, pw, role)
    }
    fn delete_user(&self, actor: &Session, name: &str) -> Result<(), AccountError> {
        self.0 .0.delete_user(actor, name)
    }
    fn set_role(&self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        self.0 .0.set_role(actor, name, role)
    }
    fn set_overrides(&self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.0 .0.set_overrides(actor, name, ov)
    }
    fn set_password(&self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        self.0 .0.set_password(actor, name, old, new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_accounts::UserStore;
    use std::sync::Mutex;

    fn users(tag: &str) -> (UsersRef, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-accapi-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (UsersRef::of_store(&Arc::new(Mutex::new(UserStore::open(&d.join("users.json")).unwrap()))), d)
    }

    #[test]
    fn a_users_ref_served_and_taken_back_is_the_same_accounts_and_the_guard_ignores_a_claimed_role() {
        let (u, d) = users("rt");
        u.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let served: Arc<dyn AccountsApi> = Arc::new(AccountsGuarded(Arc::new(UsersAsApi(u.clone()))));
        let back = UsersRef::new(Arc::new(UsersSrc(served)));
        let admin = back.lock().unwrap().authenticate("root", "rootpass12").unwrap();
        back.lock().unwrap().create_user(&admin, "gus", "guspass-123", Role::Guest).unwrap();
        assert_eq!(u.lock().unwrap().get("gus").unwrap().role, Role::Guest, "the change is in the original accounts");
        let forged = Session { user: "gus".into(), role: Role::Admin, policy: Role::Admin.template() };
        assert_eq!(back.lock().unwrap().set_role(&forged, "gus", Role::Admin), Err(AccountError::Forbidden));
        let ghost = Session { user: "ghost".into(), role: Role::Admin, policy: Role::Admin.template() };
        assert_eq!(back.lock().unwrap().delete_user(&ghost, "gus"), Err(AccountError::Forbidden));
        assert!(back.lock().unwrap().list().iter().all(|r| r.hash == "*"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
