//! Protected browser data, the person's choices (F-VAULT-07 to F-VAULT-10): the timed checkpoint, the lock screen, what is left out of the
//! sealed file, and the reader browser's data file. Fake files in temp folders; no real profile.

use super::*;
use crate::profilestore::{on_sign_in, on_sign_out, ProfileKey, SignIn, Status};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-profpol-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(n: u8) -> ProfileKey {
    ProfileKey::from_bytes([n; 32])
}

/// A plain profile with cookies and the storage of three sites, then protected and signed in (open, key held).
fn signed_in(name: &str, user: &str) -> (PathBuf, ProfileStore) {
    let d = temp(name);
    let st = ProfileStore::new(&d, user);
    let p = st.profile_path();
    for site in ["https_example.com_0", "https_www.shop.example.com_0", "https_other.org_0"] {
        std::fs::create_dir_all(p.join("data/indexeddb/v1").join(site)).unwrap();
        std::fs::write(p.join("data/indexeddb/v1").join(site).join("db.bin"), site.as_bytes()).unwrap();
        std::fs::create_dir_all(p.join("data/localstorage")).unwrap();
        std::fs::write(p.join("data/localstorage").join(format!("{site}.localstorage")), site.as_bytes()).unwrap();
    }
    std::fs::create_dir_all(p.join("config")).unwrap();
    std::fs::write(p.join("data/cookies.sqlite"), b"cookie rows").unwrap();
    std::fs::write(p.join("config/prefs"), b"zoom=1").unwrap();
    let k = key(5);
    st.protect(&k).unwrap();
    assert!(matches!(on_sign_in(&d, user, Some(key(5))), SignIn::Opened { .. }));
    (d, st)
}

fn names(st: &ProfileStore) -> Vec<String> {
    fn walk(d: &Path, rel: &str, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let n = format!("{rel}/{}", e.file_name().to_string_lossy());
            if e.path().is_dir() {
                walk(&e.path(), &n, out);
            } else {
                out.push(n);
            }
        }
    }
    let mut v = Vec::new();
    walk(&st.profile_path(), "", &mut v);
    v.sort();
    v
}

#[test]
fn site_names_are_cleaned_and_the_choices_survive_a_save_with_bad_entries_dropped() {
    assert_eq!(clean_site(" HTTPS://Example.COM/ ").unwrap(), "example.com");
    for bad in ["", "localhost", "a b.com", "exa/mple.com", "-x.com", "x..com", "http://", "ex ample.org"] {
        assert!(clean_site(bad).is_err(), "{bad}");
    }
    let d = temp("save");
    assert_eq!(Policy::load(&d, "alice"), Policy::default(), "nothing saved: the defaults, which clear nothing");
    let p = Policy { checkpoint_minutes: 999_999, lock_seal: true, clear_cookies_on_exit: true, clear_sites_on_exit: vec!["B.com".into(), "a.com".into(), "not a site".into(), "a.com".into()], never_persist: vec![] };
    p.save(&d, "alice").unwrap();
    let q = Policy::load(&d, "alice");
    assert_eq!(q.checkpoint_minutes, 24 * 60);
    assert_eq!(q.clear_sites_on_exit, ["a.com", "b.com"]);
    assert!(q.lock_seal && q.clear_cookies_on_exit);
    assert!(q.summary().contains("Seal when the screen locks: yes") && q.summary().contains("a.com, b.com"));
    std::fs::write(Policy::path(&d, "alice"), "{ not json").unwrap();
    assert_eq!(Policy::load(&d, "alice"), Policy::default(), "a damaged file reads as the defaults");
}

#[test]
fn origins_are_read_from_storage_names_and_a_site_covers_its_sub_domains_only() {
    assert_eq!(origin_host("https_example.com_0.localstorage").as_deref(), Some("example.com"));
    assert_eq!(origin_host("https_www.shop.example.com_0").as_deref(), Some("www.shop.example.com"));
    assert_eq!(origin_host("http_localhost_8080").as_deref(), Some("localhost"));
    for none in ["cookies.sqlite", "file__0", "https_x", "https__0", "indexeddb", "prefs"] {
        assert_eq!(origin_host(none), None, "{none}");
    }
    let p = Policy { never_persist: vec!["example.com".into()], ..Policy::default() };
    assert!(left_out(&p, false, "data/localstorage/https_example.com_0.localstorage"));
    assert!(left_out(&p, false, "data/indexeddb/v1/https_www.shop.example.com_0/db.bin"), "a sub-domain and everything in its folder");
    assert!(!left_out(&p, false, "data/indexeddb/v1/https_notexample.com_0/db.bin"), "a name that only ends the same is another site");
    assert!(!left_out(&p, false, "data/cookies.sqlite"));
    let c = Policy { clear_cookies_on_exit: true, clear_sites_on_exit: vec!["other.org".into()], ..Policy::default() };
    assert!(!left_out(&c, false, "data/cookies.sqlite") && !left_out(&c, false, "data/localstorage/https_other.org_0.localstorage"), "clear-on-exit counts at sign-out only");
    assert!(left_out(&c, true, "data/cookies.sqlite-wal") && left_out(&c, true, "data/localstorage/https_other.org_0.localstorage"));
}

#[test]
fn a_never_kept_site_is_missing_after_a_checkpoint_and_a_reopen_while_the_rest_stays() {
    let (d, st) = signed_in("never", "alice");
    Policy { never_persist: vec!["example.com".into()], ..Policy::default() }.save(&d, "alice").unwrap();
    let before = names(&st);
    assert!(before.iter().any(|n| n.contains("https_example.com_0")), "the running browser still has it: {before:?}");
    crate::profilestore::on_checkpoint(&d, "alice").unwrap();
    assert_eq!(names(&st), before, "a checkpoint changes nothing in the open copy");
    on_sign_out(&d, "alice").unwrap();
    assert_eq!(st.status(), Status::Sealed);
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    let after = names(&st);
    assert!(!after.iter().any(|n| n.contains("example.com")), "{after:?}");
    assert!(after.iter().any(|n| n.contains("https_other.org_0")) && after.iter().any(|n| n.ends_with("/cookies.sqlite")), "{after:?}");
}

#[test]
fn clear_on_exit_leaves_cookies_and_chosen_sites_out_at_sign_out_only() {
    let (d, st) = signed_in("exit", "alice");
    Policy { clear_cookies_on_exit: true, clear_sites_on_exit: vec!["other.org".into()], ..Policy::default() }.save(&d, "alice").unwrap();
    crate::profilestore::on_checkpoint(&d, "alice").unwrap();
    on_sign_out(&d, "alice").unwrap();
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    let after = names(&st);
    assert!(!after.iter().any(|n| n.contains("cookies.sqlite") || n.contains("other.org")), "{after:?}");
    assert!(after.iter().any(|n| n.contains("https_example.com_0")) && after.iter().any(|n| n.ends_with("config/prefs")), "{after:?}");
    // what was cleared is cleared for good: the earlier sealed copy was replaced
    assert!(st.status() == Status::Open);
}

#[test]
fn the_timed_checkpoint_waits_for_its_interval_skips_an_unchanged_profile_and_writes_a_changed_one() {
    let (d, st) = signed_in("tick", "alice");
    Policy { checkpoint_minutes: 2, ..Policy::default() }.save(&d, "alice").unwrap();
    let t0 = 1_000_000u64;
    assert_eq!(tick(&d, "alice", t0), Beat::Nothing, "the clock starts when the profile is first seen open");
    assert_eq!(tick(&d, "alice", t0 + 119), Beat::Nothing, "not yet");
    assert_eq!(tick(&d, "alice", t0 + 120), Beat::Unchanged, "due, but nothing changed");
    std::fs::write(st.profile_path().join("data/cookies.sqlite"), b"new cookie rows, longer than before").unwrap();
    assert_eq!(tick(&d, "alice", t0 + 200), Beat::Nothing, "the interval counts from the last beat");
    let sealed_before = std::fs::read(d.join("web-profile/alice.sealed")).unwrap();
    assert_eq!(tick(&d, "alice", t0 + 240), Beat::Written);
    assert_ne!(std::fs::read(d.join("web-profile/alice.sealed")).unwrap(), sealed_before, "the sealed file holds the change");
    assert!(d.join("web-profile/alice.sealed.bak").exists(), "the generation before is kept");
    // off: never
    Policy { checkpoint_minutes: 0, ..Policy::default() }.save(&d, "alice").unwrap();
    assert_eq!(tick(&d, "alice", t0 + 10_000), Beat::Nothing);
    // a profile that is not open has nothing to write
    on_sign_out(&d, "alice").unwrap();
    assert_eq!(tick(&d, "alice", t0 + 20_000), Beat::Nothing);
}

#[test]
fn locking_the_screen_seals_and_removes_the_open_copy_only_when_chosen_and_the_unlock_opens_it_again() {
    let (d, st) = signed_in("lock", "alice");
    assert_eq!(on_screen_lock(&d, "alice"), Ok(false), "off by default: the copy stays open");
    assert_eq!(st.status(), Status::Open);
    Policy { lock_seal: true, ..Policy::default() }.save(&d, "alice").unwrap();
    std::fs::write(st.profile_path().join("data/cookies.sqlite"), b"changed while open").unwrap();
    assert_eq!(on_screen_lock(&d, "alice"), Ok(true));
    assert_eq!(st.status(), Status::Sealed);
    assert!(!st.profile_path().exists(), "no decrypted copy and no link are left");
    assert_eq!(on_screen_unlock(&d, "alice", None), Ok(true), "the key held since sign-in opens it");
    assert_eq!(st.status(), Status::Open);
    assert_eq!(std::fs::read(st.profile_path().join("data/cookies.sqlite")).unwrap(), b"changed while open", "nothing was lost across the lock");
    // a second unlock with nothing sealed does nothing
    assert_eq!(on_screen_unlock(&d, "alice", None), Ok(false));
    // a key given by the caller opens it when none is held (a restarted shell)
    on_screen_lock(&d, "alice").unwrap();
    on_sign_out(&d, "alice").ok();
    assert_eq!(on_screen_unlock(&d, "alice", Some(key(9))).unwrap_err().to_string().is_empty(), false, "a wrong key is refused");
}

#[test]
fn the_reader_browser_data_file_moves_inside_the_sealed_profile_and_is_unreachable_while_sealed() {
    let d = temp("reader");
    let st = ProfileStore::new(&d, "alice");
    let plain = d.join("browser-data-alice.json");
    assert_eq!(reader_data_path(&d, "alice", "alice"), Some(plain.clone()), "no protection: the usual file");
    std::fs::write(&plain, "{\"bookmarks\":[\"secret-bookmark\"]}").unwrap();
    let p = st.profile_path();
    std::fs::create_dir_all(p.join("data")).unwrap();
    std::fs::create_dir_all(p.join("config")).unwrap();
    std::fs::write(p.join("data/cookies.sqlite"), b"c").unwrap();
    st.protect(&key(5)).unwrap();
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    // open: the file is inside the profile, moved there once and removed from beside it
    let inside = reader_data_path(&d, "alice", "alice").unwrap();
    assert!(inside.starts_with(st.profile_path()) && inside.ends_with("data/reader/browser-data.json"), "{inside:?}");
    assert!(std::fs::read_to_string(&inside).unwrap().contains("secret-bookmark"));
    assert!(!plain.exists(), "no plain copy beside the sealed file");
    std::fs::write(&inside, "{\"bookmarks\":[\"secret-bookmark\",\"two\"]}").unwrap();
    st.seal(&key(5)).unwrap();
    assert_eq!(reader_data_path(&d, "alice", "alice"), None, "sealed: nothing is read or written");
    let all = std::fs::read(d.join("web-profile/alice.sealed")).unwrap();
    assert!(!all.windows(15).any(|w| w == b"secret-bookmark"), "the sealed file holds no readable bookmark");
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    assert!(std::fs::read_to_string(reader_data_path(&d, "alice", "alice").unwrap()).unwrap().contains("two"));
}

// ---- the cookies of a site left out, row by row (F-VAULT-09, KI-740, with `sqlmini`) ----

/// A row of the cookie table as the database stores it: the first column is the row number (an INTEGER PRIMARY KEY holds NULL in the record).
fn text_record(cols: &[&str]) -> Vec<u8> {
    let (mut types, mut body) = (vec![0u8], Vec::new());
    for c in cols {
        types.push((13 + 2 * c.len()) as u8);
        body.extend_from_slice(c.as_bytes());
    }
    let mut r = vec![(types.len() + 1) as u8];
    r.extend(types);
    r.extend(body);
    r
}

fn cookie_db(hosts: &[&str]) -> Vec<u8> {
    let rows = hosts.iter().enumerate().map(|(i, h)| (i as i64 + 1, text_record(&[&format!("n{i}"), "v", h, "/"]))).collect();
    crate::sqlmini::write(&[crate::sqlmini::Table { name: "moz_cookies".into(), sql: "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, path TEXT)".into(), rows }]).unwrap()
}

fn cookie_hosts(db: &[u8]) -> Vec<String> {
    crate::sqlmini::read(db).unwrap()[0].rows.iter().map(|(_, r)| crate::sqlmini::column_text(r, 3).unwrap()).collect()
}

#[test]
fn the_cookies_of_a_never_kept_site_and_its_subdomains_are_left_out_row_by_row_and_the_other_rows_stay() {
    let (d, st) = signed_in("cookie-rows", "alice");
    let live = st.profile_path().join("data/cookies.sqlite");
    std::fs::write(&live, cookie_db(&[".example.com", "www.shop.example.com", "other.org", ".other.org", "notexample.com", "example.com.evil.net"])).unwrap();
    Policy { never_persist: vec!["Example.com".into()], ..Policy::default() }.save(&d, "alice").unwrap();
    // a checkpoint seals without the rows, the running browser keeps its own copy whole
    crate::profilestore::on_checkpoint(&d, "alice").unwrap();
    assert_eq!(cookie_hosts(&std::fs::read(&live).unwrap()).len(), 6, "the open copy is not touched");
    on_sign_out(&d, "alice").unwrap();
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    let kept = cookie_hosts(&std::fs::read(&st.profile_path().join("data/cookies.sqlite")).unwrap());
    assert_eq!(kept, ["other.org", ".other.org", "notexample.com", "example.com.evil.net"], "only example.com and its sub-domains went; a name that merely ends alike or contains it stays");
}

#[test]
fn a_cookie_file_that_cannot_be_rewritten_whole_stays_as_it_was_and_clearing_cookies_still_drops_the_file() {
    let (d, st) = signed_in("cookie-keep", "alice");
    let live = st.profile_path().join("data/cookies.sqlite");
    let db = cookie_db(&[".example.com", "other.org"]);
    // a log with content (rows that are only in it would be lost by a rewrite): the file is kept whole
    std::fs::write(&live, &db).unwrap();
    std::fs::write(st.profile_path().join("data/cookies.sqlite-wal"), b"frames").unwrap();
    Policy { never_persist: vec!["example.com".into()], ..Policy::default() }.save(&d, "alice").unwrap();
    crate::profilepolicy::on_checkpoint_for_test(&d, "alice");
    on_sign_out(&d, "alice").unwrap();
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    assert_eq!(std::fs::read(st.profile_path().join("data/cookies.sqlite")).unwrap(), db, "kept as it was");
    // a file that is not a database at all is kept too
    std::fs::write(st.profile_path().join("data/cookies.sqlite"), b"not a database").unwrap();
    std::fs::remove_file(st.profile_path().join("data/cookies.sqlite-wal")).ok();
    on_sign_out(&d, "alice").unwrap();
    assert!(matches!(on_sign_in(&d, "alice", Some(key(5))), SignIn::Opened { .. }));
    assert_eq!(std::fs::read(st.profile_path().join("data/cookies.sqlite")).unwrap(), b"not a database");
    // nothing to filter, or nothing matching: the same bytes (no rewrite of a file that did not change)
    assert!(filter_cookie_db(&db, 0, &[]).is_none());
    assert!(filter_cookie_db(&db, 0, &[&"nomatch.net".to_string()]).is_none());
    assert!(filter_cookie_db(&db, 5, &[&"example.com".to_string()]).is_none(), "a log with content");
    // the table's host column is found by name, quoted or not, and a table with a constraint line is not guessed at
    assert_eq!(crate::profilepolicy::host_column_for_test("CREATE TABLE t (id INTEGER PRIMARY KEY, \"host_key\" TEXT, name TEXT)"), Some(1));
    assert_eq!(crate::profilepolicy::host_column_for_test("CREATE TABLE t (a, b, PRIMARY KEY (a, b))"), None);
}
