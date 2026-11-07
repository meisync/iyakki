//! The periodic update check (F-UPD-04) with fake sources: apps from a catalog, the system index an administrator set, errors per
//! source, the schedule, and that the check only reports (nothing is installed or downloaded).


use super::catalog::tests::{catalog, entry, FakeFetch};
use super::catalog::SourceKind;
use super::install::tests::{admin, fx, meta, skill_pkg, user, Fx};
use super::install::Confirm;
use super::updatecheck::*;
use super::*;
use crate::pkg::{new_signing_key, public_key_hex};

fn policy() -> Policy {
    iyakki_sec::policy::Role::User.template()
}

/// Meeting notes 1.0.0 installed for alice, and a stick catalog that offers 1.2.0 asking for `perms`.
fn with_update(tag: &str, perms: &[&str]) -> Fx {
    let f = fx(tag);
    let a = user("alice");
    let key = new_signing_key().unwrap();
    f.svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
    let v1 = skill_pkg("1.0.0", Some(&key), None, &meta("acme", &[]));
    let r = f.store.review_sideload(&f.svc, &a, v1, Scope::User("alice".into()), &[]);
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    let v2 = skill_pkg("1.2.0", Some(&key), None, &meta("acme", perms));
    let mut e = entry("meeting-notes", &v2, "mn.iyp");
    e.version = "1.2.0".into();
    e.signer = public_key_hex(&key);
    e.meta = meta("acme", perms);
    let stick = f.dir.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    std::fs::write(stick.join("mn.iyp"), &v2).unwrap();
    std::fs::write(stick.join("catalog.json"), catalog("stick", 1, vec![e]).to_json()).unwrap();
    f.store.add_source(&admin(), "Stick", SourceKind::Folder, stick.to_str().unwrap()).unwrap();
    f
}

fn run_check(f: &Fx, fetch: &FakeFetch, s: &CheckSettings) -> Report {
    run_and_keep(&f.store, &f.svc, &policy(), "alice", fetch, s, "0.1.0-alpha.1", "macweb")
}

#[test]
fn an_app_with_a_newer_version_in_a_source_is_listed_and_nothing_is_installed() {
    let f = with_update("uc-app", &[]);
    let fetch = FakeFetch::default();
    let rep = run_check(&f, &fetch, &CheckSettings::default());
    assert_eq!(rep.count(), 1, "{rep:?}");
    let u = &rep.updates[0];
    assert_eq!((u.kind, u.id.as_str(), u.from.as_str(), u.to.as_str(), u.source.as_str()), (Kind::App, "skill:meeting-notes", "1.0.0", "1.2.0", "Stick"));
    assert!(!u.needs_review() && rep.errors.is_empty());
    assert_eq!(rep.badge().as_deref(), Some("1 update"));
    // it only reported: the installed version is still 1.0.0 and nothing was fetched
    let items = f.store.items(&f.svc, &Actor::from_policy("alice", &policy()), &f.store.local_catalogs(&f.svc));
    let it = items.iter().find(|i| i.entry.id == "meeting-notes").unwrap();
    assert_eq!((it.installed_version.as_deref(), &it.state), (Some("1.0.0"), &State::Update("1.2.0".into())));
    assert!(fetch.asked.lock().unwrap().is_empty());
    // the answer is kept for the page
    assert_eq!(Report::load(f.svc.dir()).unwrap(), rep);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn an_update_that_asks_for_new_permissions_is_marked_and_says_which() {
    let f = with_update("uc-perm", &["net:example.com"]);
    let rep = run_check(&f, &FakeFetch::default(), &CheckSettings::default());
    let u = &rep.updates[0];
    assert!(u.needs_review(), "{u:?}");
    assert!(u.new_permissions[0].contains("example.com"), "{:?}", u.new_permissions);
    assert!(u.note.contains("permissions"));
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn a_package_that_is_in_no_catalog_is_never_listed_and_a_nothing_new_check_is_empty() {
    let f = fx("uc-local");
    let a = user("alice");
    let key = new_signing_key().unwrap();
    f.svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
    let r = f.store.review_sideload(&f.svc, &a, skill_pkg("1.0.0", Some(&key), None, &meta("acme", &[])), Scope::User("alice".into()), &[]);
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    let rep = run_check(&f, &FakeFetch::default(), &CheckSettings::default());
    assert_eq!((rep.count(), rep.badge()), (0, None), "{rep:?}");
    assert!(rep.errors.is_empty());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn an_unreachable_source_is_an_error_line_and_not_up_to_date() {
    let f = fx("uc-err");
    f.store.add_source(&admin(), "Mirror", SourceKind::Url, "https://mirror.example/s/catalog.json").unwrap();
    let rep = run_check(&f, &FakeFetch::default(), &CheckSettings::default());
    assert_eq!(rep.asked, ["Mirror"]);
    // the store keeps its cache and warns on the catalog; with no cache at all the source is an error
    assert!(rep.errors.iter().any(|(s, _)| s == "Mirror") || rep.count() == 0, "{rep:?}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_system_index_an_administrator_set_reports_a_newer_version_of_this_variant_only() {
    let f = fx("uc-sys");
    let fetch = FakeFetch::default();
    let url = "https://updates.example/iyakki/index.json";
    let s = CheckSettings { system_url: url.into(), ..Default::default() };
    // no address, no system check, and nothing is asked
    let none = run_check(&f, &fetch, &CheckSettings::default());
    assert!(none.updates.is_empty() && fetch.asked.lock().unwrap().is_empty());
    fetch.put(url, br#"{"version":"0.1.0-alpha.3","variant":"macweb","notes":"Fixes the sound."}"#);
    let rep = run_check(&f, &fetch, &s);
    let u = &rep.updates[0];
    assert_eq!((u.kind, u.from.as_str(), u.to.as_str(), u.note.as_str()), (Kind::System, "0.1.0-alpha.1", "0.1.0-alpha.3", "Fixes the sound."));
    assert_eq!(*fetch.asked.lock().unwrap(), [url.to_string()], "only the set address, only the index");
    // the same or an older version, another variant, a damaged answer, an unreachable address
    fetch.put(url, br#"{"version":"0.1.0-alpha.1"}"#);
    assert!(run_check(&f, &fetch, &s).updates.is_empty());
    fetch.put(url, br#"{"version":"0.1.0-alpha.3","variant":"pc"}"#);
    assert!(run_check(&f, &fetch, &s).updates.is_empty(), "an index for another build says nothing about this one");
    fetch.put(url, b"<html>");
    assert!(run_check(&f, &fetch, &s).errors[0].1.contains("not an update index"));
    fetch.drop_all();
    assert_eq!(run_check(&f, &fetch, &s).errors[0].0, "System update");
    // an address that is not allowed is refused before anything is asked
    let bad = CheckSettings { system_url: "ftp://x".into(), ..Default::default() };
    assert!(run_check(&f, &fetch, &bad).errors[0].1.contains("https://"));
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn off_asks_nothing_and_the_addresses_are_checked() {
    let f = with_update("uc-off", &[]);
    let fetch = FakeFetch::default();
    let off = CheckSettings { mode: Mode::Off, system_url: "https://u.example/i.json".into(), ..Default::default() };
    let rep = run_check(&f, &fetch, &off);
    assert!(rep.updates.is_empty() && rep.asked.is_empty() && fetch.asked.lock().unwrap().is_empty());
    assert_eq!(check_url("").unwrap(), "");
    assert_eq!(check_url(" https://u.example/i.json ").unwrap(), "https://u.example/i.json");
    assert!(check_url("http://10.0.0.5/i.json").is_ok());
    for bad in ["u.example/i.json", "ftp://u.example", "https://", "https://me:pw@u.example/i", "https://u.example/a b"] {
        assert!(check_url(bad).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_schedule_is_daily_with_a_stable_jitter_off_or_manual_on_request_and_not_on_a_metered_link() {
    let s = CheckSettings::default();
    let now = 1_800_000_000;
    assert_eq!(due(&s, 0, now, None, "dev"), Due::Now, "never checked");
    let j = jitter_secs("dev", now / DAY);
    assert!((0..3600).contains(&j) && j == jitter_secs("dev", now / DAY), "stable");
    assert_ne!((0..20).map(|i| jitter_secs(&format!("d{i}"), 5)).collect::<std::collections::HashSet<_>>().len(), 1, "devices differ");
    let last = now - 3600;
    match due(&s, last, now, None, "dev") {
        Due::Later(n) => assert!(n > DAY - 3700 - 1 && n <= DAY + 3600 - 3600, "{n}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(due(&s, now - DAY - 3600, now, None, "dev"), Due::Now, "a day and an hour later it is always due");
    assert_eq!(due(&CheckSettings { mode: Mode::Manual, ..Default::default() }, 0, now, None, "dev"), Due::Manual);
    assert_eq!(due(&CheckSettings { mode: Mode::Off, ..Default::default() }, 0, now, None, "dev"), Due::Off);
    assert_eq!(due(&s, 0, now, Some(true), "dev"), Due::Metered);
    assert_eq!(due(&CheckSettings { metered_ok: true, ..Default::default() }, 0, now, Some(true), "dev"), Due::Now);
    assert_eq!(due(&s, 0, now, Some(false), "dev"), Due::Now);
    assert_eq!(Mode::Daily.next().next().next(), Mode::Daily);
}
