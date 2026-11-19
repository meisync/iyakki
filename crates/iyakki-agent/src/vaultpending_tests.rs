use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-vp-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn p() -> Pending {
    crate::quickinstall::pending_of(crate::quickinstall::Item::ClickHouse, "admin")
}

/// A fake vault (locked or not) and a fake cluster Secret.
struct World {
    locked: Cell<bool>,
    vault: RefCell<BTreeMap<String, String>>,
    audit: RefCell<Vec<String>>,
    joblog: RefCell<Vec<String>>,
    reads: Cell<u32>,
}

impl World {
    fn hooks<'a>(&'a self, mv: &'a dyn Fn(&Pending) -> Result<String, String>, unlocked: &'a dyn Fn(&str) -> bool, audit: &'a dyn Fn(&str, &str, &str), joblog: &'a dyn Fn(&Pending, &str)) -> Hooks<'a> {
        Hooks { unlocked, move_one: mv, audit, joblog }
    }
}

#[test]
fn a_give_up_leaves_the_log_line_the_audit_line_and_a_pending_entry_without_the_password_and_unlocking_finishes_it_once() {
    let d = tmp("flow");
    let w = World { locked: Cell::new(true), vault: RefCell::new(BTreeMap::new()), audit: RefCell::new(vec![]), joblog: RefCell::new(vec![]), reads: Cell::new(0) };
    let mv = |q: &Pending| -> Result<String, String> {
        w.reads.set(w.reads.get() + 1);
        move_secret(q, &|_| Ok(Zeroizing::new("s3cret-value".to_string())), &|_, n, v| if w.locked.get() { Err("vault is locked".into()) } else { w.vault.borrow_mut().insert(n.into(), v.into()); Ok(()) }, &|_, n| w.vault.borrow().contains_key(n))
    };
    let unlocked = |_: &str| !w.locked.get();
    let audit = |u: &str, a: &str, det: &str| w.audit.borrow_mut().push(format!("{u} {a} {det}"));
    let joblog = |_: &Pending, l: &str| w.joblog.borrow_mut().push(l.to_string());
    let h = w.hooks(&mv, &unlocked, &audit, &joblog);

    // the handoff gave up on a locked vault
    let words = record_waiting(&d, p(), 40, "could not put the password in your vault: vault is locked", &h);
    assert_eq!(words, "The ClickHouse password is waiting for your vault: sign in with your password or open Settings \u{2192} Vault \u{2192} Unlock");
    assert_eq!(list(&d), vec![p()]);
    assert_eq!(card_note(&d, "clickhouse"), "password waiting for your vault");
    assert!(w.joblog.borrow()[0].starts_with("[vault] gave up after 40 tries: ") && w.joblog.borrow()[0].contains("waiting in the cluster Secret"), "{:?}", w.joblog.borrow());
    assert_eq!(w.audit.borrow()[0], format!("admin quickinstall.vault.waiting clickhouse {}", p().vault));
    // the file holds only where to read it
    let on_disk = std::fs::read_to_string(file(&d)).unwrap();
    assert!(!on_disk.contains("s3cret-value") && on_disk.contains("clickhouse-auth") || on_disk.contains(&p().secret), "{on_disk}");

    // still locked: nothing happens, nothing is read
    assert!(finish_all(&d, &h).is_empty() && w.reads.get() == 0);
    // unlocked: the pending entry is completed, verified, removed
    w.locked.set(false);
    let done = finish_all(&d, &h);
    assert_eq!(done.len(), 1);
    assert!(done[0].as_ref().unwrap().contains("is in your vault as"), "{done:?}");
    assert_eq!(w.vault.borrow().get(&p().vault).map(String::as_str), Some("s3cret-value"));
    assert!(list(&d).is_empty() && card_note(&d, "clickhouse").is_empty());
    assert!(w.joblog.borrow().iter().any(|l| l.starts_with("[vault] stored ")) && w.audit.borrow().iter().any(|l| l.contains("quickinstall.vault.stored")));
    // idempotent
    assert!(finish_all(&d, &h).is_empty() && w.reads.get() == 1);
    // no secret value anywhere in the log or audit lines
    assert!(w.joblog.borrow().iter().chain(w.audit.borrow().iter()).all(|l| !l.contains("s3cret-value")));
}

#[test]
fn a_move_that_cannot_read_the_secret_stays_pending_and_the_vault_is_verified_after_saving() {
    let d = tmp("keep");
    add(&d, p()).unwrap();
    add(&d, p()).unwrap();
    assert_eq!(list(&d).len(), 1, "the same entry is not listed twice");
    let w = World { locked: Cell::new(false), vault: RefCell::new(BTreeMap::new()), audit: RefCell::new(vec![]), joblog: RefCell::new(vec![]), reads: Cell::new(0) };
    let mv = |q: &Pending| move_secret(q, &|_| Err("the cluster is down".into()), &|_, _, _| Ok(()), &|_, _| true);
    let unlocked = |_: &str| true;
    let audit = |_: &str, _: &str, _: &str| {};
    let joblog = |_: &Pending, _: &str| {};
    assert!(finish_all(&d, &w.hooks(&mv, &unlocked, &audit, &joblog)).is_empty());
    assert_eq!(list(&d).len(), 1);
    // a store that says yes but leaves nothing behind is not a success
    let lie = |q: &Pending| move_secret(q, &|_| Ok(Zeroizing::new("x".to_string())), &|_, _, _| Ok(()), &|_, _| false);
    assert!(lie(&p()).unwrap_err().contains("is not in your vault after saving"));
}

#[test]
fn the_pending_items_are_named_in_the_card_and_each_item_has_a_source() {
    use crate::quickinstall::{pending_of, Item};
    for (it, vault) in [(Item::GraphDb, crate::quickinstall::GRAPH_VAULT_NAME), (Item::ClickHouse, crate::quickinstall::clickhouse::VAULT_NAME), (Item::Matrix, crate::quickinstall::matrix::VAULT_NAME)] {
        let q = pending_of(it, "admin");
        assert_eq!((q.item.as_str(), q.vault.as_str()), (it.id(), vault));
        assert!(waiting_words(&q).contains("Settings \u{2192} Vault \u{2192} Unlock"));
    }
}

#[test]
fn a_hand_off_with_a_locked_vault_waits_at_once_in_words_and_runs_when_the_password_opened_the_vault() {
    struct Never;
    impl iyakki_proxy::Upstream for Never {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("no network in tests".into())
        }
    }
    let d = tmp("locked-start");
    let up: std::sync::Arc<dyn iyakki_proxy::Upstream> = std::sync::Arc::new(Never);
    let svc = crate::services::Services::open(&d.join("svc"), up.clone(), up).unwrap();
    let q = crate::quickinstall::pending_of(crate::quickinstall::Item::ClickHouse, "admin");
    // nobody typed a password: the vault is locked, the 40 tries are not spent, the person is told what to do
    let r = crate::quickinstall::wait_for_unlock(&svc, &q).expect("a locked vault waits");
    let words = r.unwrap_err();
    assert!(words.contains("password is waiting for your vault") && words.contains("sign in with your password") && words.contains("Settings \u{2192} Vault \u{2192} Unlock"), "{words}");
    assert_eq!(list(svc.dir()), vec![q.clone()], "where to read the password is kept, never the password");
    assert!(svc.inbox.list("admin").iter().any(|n| n.title == "The ClickHouse password is waiting for your vault"));
    // the sign-in with the password opens the vault: the hand-off runs as before
    svc.on_login("admin", "admin-pass-123", true);
    assert!(crate::quickinstall::wait_for_unlock(&svc, &q).is_none());
    let _ = std::fs::remove_dir_all(&d);
}
