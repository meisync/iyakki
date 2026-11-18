//! The USERS tile driven through a fake [`UsersPort`]: a real account store in a scratch folder (the store is the rule keeper, so it stays real),
//! but no `Services`, no `AppCx` and no audit file. The port records the audit lines and the vault wipes, which is what the tile owes the system.

use std::cell::RefCell;
use std::sync::Mutex;

use super::*;
use crate::accounts::{Session, UserRecord, UserStore};
use crate::apps::tests::temp_dir;

struct FakePort {
    store: Mutex<UserStore>,
    session: Session,
    audit: RefCell<Vec<(String, String)>>,
    wiped: RefCell<Vec<String>>,
}

fn port(tag: &str, role: Role) -> FakePort {
    let d = temp_dir(tag);
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    FakePort { store: Mutex::new(store), session: Session { user: "root".into(), role, policy: role.template() }, audit: RefCell::new(Vec::new()), wiped: RefCell::new(Vec::new()) }
}

impl UsersPort for FakePort {
    fn me(&self) -> String {
        self.session.user.clone()
    }
    fn can_manage_users(&self) -> bool {
        self.session.policy.manage_users
    }
    fn users(&self) -> Vec<UserRecord> {
        self.store.lock().unwrap().list().to_vec()
    }
    fn create_user(&self, name: &str, password: &str, role: Role) -> Result<(), AccountError> {
        self.store.lock().unwrap().create_user(&self.session, name, password, role)
    }
    fn set_password(&self, name: &str, password: &str) -> Result<(), AccountError> {
        let r = self.store.lock().unwrap().set_password(&self.session, name, None, password);
        if r.is_ok() {
            self.wiped.borrow_mut().push(name.to_string());
        }
        r
    }
    fn set_role(&self, name: &str, role: Role) -> Result<(), AccountError> {
        self.store.lock().unwrap().set_role(&self.session, name, role)
    }
    fn set_overrides(&self, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        self.store.lock().unwrap().set_overrides(&self.session, name, ov)
    }
    fn delete_user(&self, name: &str) -> Result<(), AccountError> {
        let r = self.store.lock().unwrap().delete_user(&self.session, name);
        if r.is_ok() {
            self.wiped.borrow_mut().push(name.to_string());
        }
        r
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.to_string(), subject.to_string()));
    }
    fn audit_verify(&self) -> Result<usize, usize> {
        Ok(0)
    }
    fn audit_recent(&self, _n: usize) -> Vec<String> {
        Vec::new()
    }
    fn audit_start_new_log(&self) -> Result<std::path::PathBuf, String> {
        Err("not in this test".into())
    }
}

fn keys(app: &mut UsersApp, p: &FakePort, s: &str) {
    for c in s.chars() {
        app.handle_with(&Event::Key(Key::Char(c)), p);
    }
}

fn key(app: &mut UsersApp, p: &FakePort, k: Key) {
    app.handle_with(&Event::Key(k), p);
}

fn make_carol(app: &mut UsersApp, p: &FakePort) {
    key(app, p, Key::Char('n'));
    keys(app, p, "carol");
    key(app, p, Key::Tab);
    keys(app, p, "carolpass1");
    key(app, p, Key::Tab);
    key(app, p, Key::Enter);
}

#[test]
fn a_new_user_is_made_and_audited_through_the_port_without_the_password() {
    let p = port("users-fake1", Role::Admin);
    let mut app = UsersApp::new();
    make_carol(&mut app, &p);
    assert_eq!(app.mode(), &Mode::List);
    assert!(p.users().iter().any(|u| u.name == "carol"));
    let log = p.audit.borrow().clone();
    assert!(log.iter().any(|(a, s)| a == "user.create" && s.starts_with("carol")), "{log:?}");
    assert!(!format!("{log:?}").contains("carolpass1"));
    assert!(p.wiped.borrow().is_empty());
}

#[test]
fn a_password_reset_and_a_delete_wipe_the_vault_through_the_port() {
    let p = port("users-fake2", Role::Admin);
    let mut app = UsersApp::new();
    make_carol(&mut app, &p);
    let at = p.users().iter().position(|u| u.name == "carol").unwrap();
    for _ in 0..at {
        key(&mut app, &p, Key::Down);
    }
    key(&mut app, &p, Key::Char('p'));
    keys(&mut app, &p, "newpass123");
    key(&mut app, &p, Key::Enter);
    assert_eq!(*p.wiped.borrow(), vec!["carol".to_string()]);
    assert!(p.audit.borrow().iter().any(|(a, s)| a == "user.password" && s == "carol"));
    key(&mut app, &p, Key::Char('d'));
    key(&mut app, &p, Key::Char('y'));
    assert!(!p.users().iter().any(|u| u.name == "carol"));
    assert_eq!(*p.wiped.borrow(), vec!["carol".to_string(), "carol".to_string()]);
}

#[test]
fn the_last_administrator_cannot_be_deleted_and_the_failure_is_audited_and_wipes_nothing() {
    let p = port("users-fake3", Role::Admin);
    let mut app = UsersApp::new();
    key(&mut app, &p, Key::Char('d'));
    key(&mut app, &p, Key::Char('y'));
    assert_eq!(p.users().len(), 1);
    assert!(p.wiped.borrow().is_empty());
    assert!(p.audit.borrow().iter().any(|(a, _)| a == "user.delete.failed"), "{:?}", p.audit.borrow());
}
