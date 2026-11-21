//! The accounts behind a small trait (F-SPLIT-03, UI-RPC-DESIGN.md, D-SPLIT-03-HANDLE): the shell and the apps hold a [`UsersRef`], so the same code
//! runs over the store in this process and over a connection to the core.
//!
//! The old code held `Arc<Mutex<UserStore>>` and wrote `store.lock().unwrap().authenticate(..)`. [`UsersRef`] keeps that shape (`lock()` gives a
//! [`UsersGuard`] with the same method names), but the guard holds no lock: every call is one call of the source, which locks for as long as it
//! needs. Differences from the store itself, all on purpose: `list` and `get` answer owned records with the password HASH blanked (the hash
//! never leaves the store), and nothing borrows from the store, so nothing needs the lock to stay held.

use std::sync::{Arc, Mutex, PoisonError};

use iyakki_sec::policy::{PolicyOverride, Role};

use crate::{AccountError, Session, UserRecord, UserStore};

/// What the shell and the apps ask of the accounts.
pub trait UserSource: Send + Sync {
    fn is_empty(&self) -> bool;
    /// Every account, the password hash blanked.
    fn list(&self) -> Vec<UserRecord>;
    /// One account, the password hash blanked.
    fn get(&self, name: &str) -> Option<UserRecord>;
    fn authenticate(&self, name: &str, password: &str) -> Result<Session, AccountError>;
    /// The session of a person put back at the desktop without the password (F-SHELL-65; the caller holds the fresh ticket).
    fn resume_session(&self, name: &str) -> Option<Session>;
    fn create_first_admin(&self, name: &str, password: &str) -> Result<(), AccountError>;
    fn create_user(&self, actor: &Session, name: &str, password: &str, role: Role) -> Result<(), AccountError>;
    fn delete_user(&self, actor: &Session, name: &str) -> Result<(), AccountError>;
    fn set_role(&self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError>;
    fn set_overrides(&self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError>;
    fn set_password(&self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError>;
    /// Read the accounts again if something else changed the file.
    fn refresh(&self);
}

fn locked(s: &Mutex<UserStore>) -> std::sync::MutexGuard<'_, UserStore> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

impl UserSource for Arc<Mutex<UserStore>> {
    fn is_empty(&self) -> bool {
        locked(self).is_empty()
    }
    fn list(&self) -> Vec<UserRecord> {
        locked(self).list_public()
    }
    fn get(&self, name: &str) -> Option<UserRecord> {
        locked(self).get_public(name)
    }
    fn authenticate(&self, name: &str, password: &str) -> Result<Session, AccountError> {
        locked(self).authenticate(name, password)
    }
    fn resume_session(&self, name: &str) -> Option<Session> {
        locked(self).resume_session(name)
    }
    fn create_first_admin(&self, name: &str, password: &str) -> Result<(), AccountError> {
        locked(self).create_first_admin(name, password)
    }
    fn create_user(&self, actor: &Session, name: &str, password: &str, role: Role) -> Result<(), AccountError> {
        locked(self).create_user(actor, name, password, role)
    }
    fn delete_user(&self, actor: &Session, name: &str) -> Result<(), AccountError> {
        locked(self).delete_user(actor, name)
    }
    fn set_role(&self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        locked(self).set_role(actor, name, role)
    }
    fn set_overrides(&self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        locked(self).set_overrides(actor, name, ov)
    }
    fn set_password(&self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        locked(self).set_password(actor, name, old, new)
    }
    fn refresh(&self) {
        locked(self).refresh()
    }
}

/// The accounts of a services handle, of the core's socket, or of one store in this process ([`UsersRef::of_store`]); cheap to clone.
#[derive(Clone)]
pub struct UsersRef(pub Arc<dyn UserSource>);

impl UsersRef {
    pub fn new(src: Arc<dyn UserSource>) -> UsersRef {
        UsersRef(src)
    }

    /// A store of this process.
    pub fn of_store(store: &Arc<Mutex<UserStore>>) -> UsersRef {
        UsersRef(Arc::new(store.clone()))
    }

    /// The accounts, in the shape of the old `Mutex::lock` (the `Result` is never an error; no lock is held).
    pub fn lock(&self) -> Result<UsersGuard, PoisonError<UsersGuard>> {
        Ok(UsersGuard(self.0.clone()))
    }
}

/// The accounts with the method names and receivers the store had.
pub struct UsersGuard(Arc<dyn UserSource>);

impl UsersGuard {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn list(&self) -> Vec<UserRecord> {
        self.0.list()
    }
    pub fn get(&self, name: &str) -> Option<UserRecord> {
        self.0.get(name)
    }
    pub fn authenticate(&mut self, name: &str, password: &str) -> Result<Session, AccountError> {
        self.0.authenticate(name, password)
    }
    pub fn resume_session(&mut self, name: &str) -> Option<Session> {
        self.0.resume_session(name)
    }
    pub fn create_first_admin(&mut self, name: &str, password: &str) -> Result<(), AccountError> {
        self.0.create_first_admin(name, password)
    }
    pub fn create_user(&mut self, actor: &Session, name: &str, password: &str, role: Role) -> Result<(), AccountError> {
        self.0.create_user(actor, name, password, role)
    }
    pub fn delete_user(&mut self, actor: &Session, name: &str) -> Result<(), AccountError> {
        self.0.delete_user(actor, name)
    }
    pub fn set_role(&mut self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        self.0.set_role(actor, name, role)
    }
    pub fn set_overrides(&mut self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.0.set_overrides(actor, name, ov)
    }
    pub fn set_password(&mut self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        self.0.set_password(actor, name, old, new)
    }
    pub fn refresh(&mut self) {
        self.0.refresh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn store(tag: &str) -> Arc<Mutex<UserStore>> {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("iyakki-usersref-{}-{}-{}", std::process::id(), tag, N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Arc::new(Mutex::new(UserStore::open(&d.join("users.json")).unwrap()))
    }

    #[test]
    fn a_guard_runs_the_whole_account_life_over_the_store_and_the_hash_is_never_handed_out() {
        let st = store("life");
        let r = UsersRef::of_store(&st);
        assert!(r.lock().unwrap().is_empty());
        r.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        assert_eq!(r.lock().unwrap().create_first_admin("x", "xxxxxxxx12"), Err(AccountError::Forbidden), "only while nobody exists");
        let admin = r.lock().unwrap().authenticate("root", "rootpass12").unwrap();
        assert_eq!(admin.role, Role::Admin);
        assert_eq!(r.lock().unwrap().authenticate("root", "wrong-pass"), Err(AccountError::InvalidCredentials));
        let mut g = r.lock().unwrap();
        g.create_user(&admin, "dave", "davepass12", Role::User).unwrap();
        g.set_role(&admin, "dave", Role::Guest).unwrap();
        g.set_password(&admin, "dave", None, "newpass-123").unwrap();
        assert!(g.authenticate("dave", "newpass-123").is_ok());
        let dave = g.get("dave").unwrap();
        assert_eq!((dave.role, dave.hash.as_str()), (Role::Guest, "*"), "the record is there, the hash is blanked to a mark that says a password exists");
        assert!(g.list().iter().all(|u| u.hash == "*") && g.list().len() == 2);
        assert!(st.lock().unwrap().get("dave").unwrap().hash.starts_with("$argon2"), "the store itself still has it");
        g.delete_user(&admin, "dave").unwrap();
        assert!(g.get("dave").is_none());
        assert_eq!(g.delete_user(&admin, "root"), Err(AccountError::LastAdmin));
        assert_eq!(g.resume_session("root").unwrap().user, "root");
        assert!(g.resume_session("nobody").is_none());
    }

    #[test]
    fn a_guest_session_cannot_change_accounts_and_a_poisoned_store_still_answers() {
        let st = store("poison");
        let r = UsersRef::of_store(&st);
        r.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let guest = Session { user: "g".into(), role: Role::Guest, policy: Role::Guest.template() };
        assert_eq!(r.lock().unwrap().create_user(&guest, "z", "zzzzzzzz12", Role::User), Err(AccountError::Forbidden));
        let st2 = st.clone();
        let _ = std::thread::spawn(move || {
            let _held = st2.lock().unwrap(); // poison-ok: the test makes the lock poisoned on purpose
            panic!("poison the accounts lock");
        })
        .join();
        assert!(st.lock().is_err(), "the lock is poisoned");
        assert_eq!(r.lock().unwrap().list().len(), 1, "the reference recovers like the callers did");
    }

    #[test]
    fn the_session_and_the_error_survive_the_wire() {
        let s = Session { user: "root".into(), role: Role::Admin, policy: Role::Admin.template() };
        let back: Session = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        for e in [AccountError::Throttled(7), AccountError::LastAdmin, AccountError::Storage("x".into())] {
            let back: AccountError = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
            assert_eq!(back, e);
        }
    }
}
