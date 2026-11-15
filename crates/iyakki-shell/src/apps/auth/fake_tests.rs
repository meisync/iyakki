//! The AUTH tile driven through a fake [`AuthPort`]: no services, no session and no audit file. It shows that every audit line comes through the
//! port and that the vault is the one the port gives (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use iyakki_agent::totp::Algo;

#[derive(Clone, Default)]
struct MemVault(Rc<RefCell<Option<String>>>);

impl CodeVault for MemVault {
    fn load(&self) -> Result<Book, VaultProblem> {
        match self.0.borrow().as_deref() {
            None => Ok(Book::default()),
            Some(t) => Book::parse(t).map_err(VaultProblem::Other),
        }
    }
    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        *self.0.borrow_mut() = Some(book.to_text().map_err(VaultProblem::Other)?.to_string());
        Ok(())
    }
}

struct FakePort {
    vault: MemVault,
    audit: RefCell<Vec<(String, String)>>,
}

impl AuthPort for FakePort {
    fn vault(&self) -> Box<dyn CodeVault> {
        Box::new(self.vault.clone())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.to_string(), subject.to_string()));
    }
}

fn port_with(entries: &[(&str, &str)]) -> FakePort {
    let vault = MemVault::default();
    let mut b = Book::default();
    for (issuer, account) in entries {
        b.add(&NewEntry::typed(issuer, account, "JBSWY3DPEHPK3PXP", (Algo::Sha1, 6, 30), "Personal")).unwrap();
    }
    vault.save(&b).unwrap();
    FakePort { vault, audit: RefCell::new(Vec::new()) }
}

#[test]
fn the_app_reads_the_vault_the_port_gives() {
    let port = port_with(&[("GitHub", "alice")]);
    let app = AuthApp::new(&port);
    assert_eq!(app.book.entries.len(), 1);
    assert!(port.audit.borrow().is_empty(), "opening writes no audit line");
}

#[test]
fn deleting_a_code_asks_first_then_deletes_and_audits_through_the_port_without_the_secret() {
    let port = port_with(&[("GitHub", "alice"), ("Slack", "bob")]);
    let mut app = AuthApp::new(&port);
    app.handle_with(&Event::Key(Key::Delete), &port);
    assert!(matches!(app.mode, Mode::DelEntry(_)));
    app.handle_with(&Event::Key(Key::Char('x')), &port);
    assert!(matches!(app.mode, Mode::DelEntry(_)), "another key does not confirm");
    assert!(port.audit.borrow().is_empty());
    app.handle_with(&Event::Key(Key::Char('y')), &port);
    assert_eq!(port.vault.load().unwrap().entries.len(), 1);
    let log = port.audit.borrow().clone();
    assert_eq!(log, vec![("auth.delete".to_string(), "issuer=GitHub".to_string())]);
    assert!(!format!("{log:?}").contains("JBSWY3DPEHPK3PXP"));
}

#[test]
fn escape_cancels_a_delete_and_nothing_is_audited() {
    let port = port_with(&[("GitHub", "alice")]);
    let mut app = AuthApp::new(&port);
    app.handle_with(&Event::Key(Key::Delete), &port);
    app.handle_with(&Event::Key(Key::Esc), &port);
    assert_eq!(port.vault.load().unwrap().entries.len(), 1);
    assert!(port.audit.borrow().is_empty());
}
