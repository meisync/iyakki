//! The one-time-code book: entries, groups, the vault round trip (a fake vault and the real one), and that no secret leaks.

use std::cell::RefCell;
use std::sync::Arc;

use super::*;

const SECRET: &str = "JBSWY3DPEHPK3PXP";

fn new(issuer: &str, account: &str, group: &str) -> NewEntry {
    NewEntry { issuer: issuer.into(), account: account.into(), secret: Zeroizing::new(SECRET.into()), algo: Algo::Sha1, digits: 6, period: 30, group: group.into() }
}

#[derive(Default)]
struct Fake {
    text: RefCell<Option<String>>,
    locked: bool,
}

impl CodeVault for Fake {
    fn load(&self) -> Result<Book, VaultProblem> {
        if self.locked {
            return Err(VaultProblem::Locked);
        }
        match self.text.borrow().as_deref() {
            None => Ok(Book::default()),
            Some(t) => Book::parse(t).map_err(VaultProblem::Other),
        }
    }
    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        if self.locked {
            return Err(VaultProblem::Locked);
        }
        *self.text.borrow_mut() = Some(book.to_text().map_err(VaultProblem::Other)?.to_string());
        Ok(())
    }
}

#[test]
fn a_new_book_has_one_group_and_no_entries() {
    let b = Book::default();
    assert_eq!(b.groups, [DEFAULT_GROUP]);
    assert!(b.entries.is_empty());
}

#[test]
fn an_entry_is_added_normalised_and_gives_its_code() {
    let mut b = Book::default();
    let mut n = new("GitHub", "alice", "personal");
    n.secret = Zeroizing::new("jbsw y3dp ehpk 3pxp".into());
    let id = b.add(&n).unwrap();
    let e = b.entry(id).unwrap();
    assert_eq!((e.issuer.as_str(), e.account.as_str(), e.secret.as_str(), e.algo.as_str(), e.digits, e.period), ("GitHub", "alice", SECRET, "SHA1", 6, 30));
    assert_eq!(e.group, "Personal", "the group name is matched without regard to case");
    assert_eq!(e.title(), "GitHub (alice)");
    let key = totp::base32_decode(SECRET).unwrap();
    assert_eq!(e.code_at(1_800_000_000), Some(totp::totp(Algo::Sha1, &key, 1_800_000_000, 30, 6)));
    assert_eq!(e.code_at(1_800_000_000).unwrap().len(), 6);
}

#[test]
fn the_algorithm_digits_and_period_are_kept() {
    let mut b = Book::default();
    let mut n = new("Bank", "me", "Personal");
    n.algo = Algo::Sha512;
    n.digits = 8;
    n.period = 60;
    let id = b.add(&n).unwrap();
    let e = b.entry(id).unwrap();
    assert_eq!((e.algorithm(), e.digits, e.period), (Algo::Sha512, 8, 60));
    let key = totp::base32_decode(SECRET).unwrap();
    assert_eq!(e.code_at(100), Some(totp::totp(Algo::Sha512, &key, 100, 60, 8)));
}

#[test]
fn bad_entries_are_refused_in_plain_words() {
    let mut b = Book::default();
    let mut bad = new("", "", "Personal");
    assert!(b.add(&bad).unwrap_err().contains("service name or the account name"));
    bad = new("X", "", "Personal");
    bad.secret = Zeroizing::new("not base32!".into());
    assert!(b.add(&bad).is_err());
    bad = new("X", "", "Personal");
    bad.digits = 5;
    assert!(b.add(&bad).unwrap_err().contains("digits"));
    bad = new("X", "", "Personal");
    bad.period = 0;
    assert!(b.add(&bad).unwrap_err().contains("time step"));
    assert!(b.add(&new("X", "", "Nowhere")).unwrap_err().contains("does not exist"));
    assert!(b.add(&new(&"x".repeat(81), "", "Personal")).is_err());
    assert!(b.add(&new("a\nb", "", "Personal")).is_err());
    assert!(b.entries.is_empty());
    b.add(&new("X", "a", "Personal")).unwrap();
    assert!(b.add(&new("X", "a", "Personal")).unwrap_err().contains("already"));
    assert_eq!(b.entries.len(), 1);
}

#[test]
fn a_book_holds_at_most_two_hundred_codes() {
    let mut b = Book::default();
    for i in 0..MAX_ENTRIES {
        b.add(&new("S", &format!("a{i}"), "Personal")).unwrap();
    }
    assert!(b.add(&new("S", "one more", "Personal")).unwrap_err().contains("At most"));
    // and a full book still fits the vault's size limit
    assert!(b.to_text().unwrap().len() < crate::vault::MAX_SECRET_BYTES);
}

#[test]
fn groups_are_created_renamed_and_deleted() {
    let mut b = Book::default();
    assert_eq!(b.add_group("  Work ").unwrap(), "Work");
    assert!(b.add_group("work").unwrap_err().contains("already"));
    assert!(b.add_group("").is_err());
    assert!(b.add_group(&"g".repeat(81)).is_err());
    let a = b.add(&new("Slack", "me", "Work")).unwrap();
    let c = b.add(&new("Mail", "me", "Personal")).unwrap();
    assert_eq!((b.count_in("Work"), b.count_in("Personal")), (1, 1));
    // rename moves the entries along
    assert_eq!(b.rename_group("Work", "Job").unwrap(), "Job");
    assert_eq!(b.entry(a).unwrap().group, "Job");
    assert!(b.rename_group("Job", "personal").unwrap_err().contains("already"));
    assert!(b.rename_group("Nope", "X").is_err());
    assert_eq!(b.rename_group("Job", "JOB").unwrap(), "JOB", "a new spelling of the same name is fine");
    // deleting keeps the codes: they go to the first group
    assert_eq!(b.delete_group("JOB").unwrap(), 1);
    assert_eq!(b.groups, ["Personal"]);
    assert_eq!(b.entry(a).unwrap().group, "Personal");
    assert_eq!(b.entry(c).unwrap().group, "Personal");
    assert!(b.delete_group("Personal").unwrap_err().contains("only group"));
    assert!(b.delete_group("Nope").is_err());
}

#[test]
fn groups_are_limited() {
    let mut b = Book::default();
    for i in 1..MAX_GROUPS {
        b.add_group(&format!("g{i}")).unwrap();
    }
    assert!(b.add_group("one more").unwrap_err().contains("At most"));
}

#[test]
fn an_entry_is_edited_moved_and_deleted() {
    let mut b = Book::default();
    b.add_group("Work").unwrap();
    let id = b.add(&new("Old", "me", "Personal")).unwrap();
    b.edit(id, " New ", "you").unwrap();
    assert_eq!((b.entry(id).unwrap().issuer.as_str(), b.entry(id).unwrap().account.as_str()), ("New", "you"));
    assert_eq!(b.entry(id).unwrap().secret, SECRET, "editing never touches the secret");
    assert!(b.edit(id, "", "").is_err());
    assert!(b.edit(999, "a", "b").is_err());
    b.move_to(id, "work").unwrap();
    assert_eq!(b.entry(id).unwrap().group, "Work");
    assert!(b.move_to(id, "Nowhere").is_err());
    assert!(b.move_to(999, "Work").is_err());
    let gone = b.delete(id).unwrap();
    assert_eq!(gone.issuer, "New");
    assert!(b.entry(id).is_none());
    assert!(b.delete(id).is_err());
}

#[test]
fn ids_are_not_reused_after_a_delete() {
    let mut b = Book::default();
    let a = b.add(&new("A", "", "Personal")).unwrap();
    b.delete(a).unwrap();
    let c = b.add(&new("B", "", "Personal")).unwrap();
    assert_ne!(a, c);
}

#[test]
fn the_book_round_trips_through_a_fake_vault_with_everything_in_it() {
    let v = Fake::default();
    assert_eq!(v.load().unwrap(), Book::default(), "nothing stored yet: a fresh book");
    let mut b = v.load().unwrap();
    b.add_group("Work").unwrap();
    let mut n = new("Example", "me@example.com", "Work");
    n.algo = Algo::Sha256;
    n.digits = 7;
    n.period = 45;
    b.add(&n).unwrap();
    v.save(&b).unwrap();
    let back = v.load().unwrap();
    assert_eq!(back, b);
    let e = &back.entries[0];
    assert_eq!((e.algo.as_str(), e.digits, e.period, e.group.as_str()), ("SHA256", 7, 45, "Work"));
    // the secret and the names are in the stored text (it is the vault that encrypts it)
    let stored = v.text.borrow().clone().unwrap();
    assert!(stored.contains(SECRET) && stored.contains("me@example.com"));
}

#[test]
fn a_locked_vault_is_reported_and_nothing_is_saved() {
    let v = Fake { locked: true, ..Fake::default() };
    assert_eq!(v.load().unwrap_err(), VaultProblem::Locked);
    assert_eq!(v.save(&Book::default()).unwrap_err(), VaultProblem::Locked);
    assert!(v.text.borrow().is_none());
}

#[test]
fn a_damaged_or_newer_stored_book_is_an_error_not_an_empty_book() {
    for bad in ["", "not json", "{}", "[1]", r#"{"v":2,"groups":["A"],"entries":[],"next":1}"#] {
        assert!(Book::parse(bad).is_err(), "{bad:?}");
    }
    let v = Fake::default();
    *v.text.borrow_mut() = Some("garbage".into());
    assert!(matches!(v.load().unwrap_err(), VaultProblem::Other(m) if !m.contains("garbage")));
}

#[test]
fn a_stored_book_with_a_missing_group_or_next_counter_is_repaired() {
    let t = format!(r#"{{"v":1,"groups":[],"entries":[{{"id":7,"issuer":"A","account":"","secret":"{SECRET}","algo":"SHA1","digits":6,"period":30,"group":"Gone"}}],"next":1}}"#);
    let mut b = Book::parse(&t).unwrap();
    assert_eq!(b.groups, [DEFAULT_GROUP]);
    assert_eq!(b.entries[0].group, DEFAULT_GROUP);
    let id = b.add(&new("B", "", "Personal")).unwrap();
    assert!(id > 7, "{id}");
}

#[test]
fn a_damaged_entry_gives_no_code_instead_of_a_wrong_one() {
    let mut b = Book::default();
    let id = b.add(&new("A", "", "Personal")).unwrap();
    b.entries[0].secret = "!!!".into();
    assert_eq!(b.entry(id).unwrap().code_at(1_800_000_000), None);
}

#[test]
fn debug_text_never_shows_a_secret() {
    let mut b = Book::default();
    b.add(&new("A", "me", "Personal")).unwrap();
    let d = format!("{b:?} {:?} {:?}", new("A", "me", "Personal"), b.entries[0]);
    assert!(!d.contains(SECRET), "{d}");
}

#[test]
fn from_a_link_the_entry_carries_the_links_settings() {
    let p = totp::parse_otpauth(&format!("otpauth://totp/Acme:zed?secret={SECRET}&algorithm=SHA512&digits=8&period=20")).unwrap();
    let mut b = Book::default();
    let id = b.add(&NewEntry::from_link(p, "Personal")).unwrap();
    let e = b.entry(id).unwrap();
    assert_eq!((e.issuer.as_str(), e.account.as_str(), e.algo.as_str(), e.digits, e.period), ("Acme", "zed", "SHA512", 8, 20));
}

#[test]
fn the_clock_check_follows_the_floor() {
    assert!(!clock_is_set(0));
    assert!(!clock_is_set(crate::clockfloor::COMPILED_FLOOR - 1));
    assert!(clock_is_set(crate::clockfloor::COMPILED_FLOOR));
    assert!(clock_is_set(crate::clock::now_unix()), "the machine running the tests has a set clock");
}

#[derive(Debug)]
struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in tests".into())
    }
}

#[test]
fn the_real_vault_keeps_the_book_locks_and_unlocks() {
    let d = std::env::temp_dir().join(format!("iyakki-authcodes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let v = ServicesVault { svc: svc.clone(), user: "amy".into() };
    // before sign-in the vault is not open
    assert_eq!(v.load().unwrap_err(), VaultProblem::Locked);
    svc.on_login("amy", "amy-pass-123", false);
    let mut b = v.load().unwrap();
    assert_eq!(b, Book::default());
    b.add(&new("Mail", "amy", "Personal")).unwrap();
    v.save(&b).unwrap();
    assert_eq!(v.load().unwrap(), b);
    // the vault holds it under its own name, and nobody else's vault sees it
    assert!(svc.user_names("amy").unwrap().contains(&VAULT_NAME.to_string()));
    let other = ServicesVault { svc: svc.clone(), user: "bob".into() };
    assert_eq!(other.load().unwrap_err(), VaultProblem::Locked);
    // the plain files on disk do not contain the secret
    let mut stack = vec![d.clone()];
    while let Some(p) = stack.pop() {
        for e in std::fs::read_dir(&p).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                assert!(!String::from_utf8_lossy(&bytes).contains(SECRET), "{}", path.display());
            }
        }
    }
    // locked again: reading and saving say so
    svc.lock_user_vault("amy");
    assert_eq!(v.load().unwrap_err(), VaultProblem::Locked);
    assert_eq!(v.save(&b).unwrap_err(), VaultProblem::Locked);
    // unlocking with the password brings the codes back
    svc.unlock_user_vault("amy", "amy-pass-123").unwrap();
    assert_eq!(v.load().unwrap(), b);
    let _ = std::fs::remove_dir_all(&d);
}
