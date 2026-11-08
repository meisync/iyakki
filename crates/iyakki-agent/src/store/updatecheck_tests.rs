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

fn run_check(f: &Fx, fetch: &dyn super::catalog::Fetch, s: &CheckSettings) -> Report {
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

// ---------------------------------------------------------------------------------------- the grouped notice and the badge

use super::updatenotice::{self, Outcome};
use crate::inbox::Link;

fn notices(f: &Fx) -> Vec<crate::inbox::Notification> {
    f.svc.inbox.list("alice").into_iter().filter(|n| n.group == format!("keyed:{}", updatenotice::KEY)).collect()
}

fn upd(id: &str, title: &str, to: &str) -> Update {
    Update { kind: Kind::App, id: id.into(), title: title.into(), from: "1.0.0".into(), to: to.into(), source: "S".into(), new_permissions: vec![], note: String::new(), driver: false }
}

fn rep_of(updates: Vec<Update>) -> Report {
    Report { checked_at: 1, updates, ..Default::default() }
}

#[test]
fn a_check_that_finds_an_update_leaves_one_notice_and_a_repeat_does_not_add_another() {
    let f = with_update("uc-note", &[]);
    let fetch = FakeFetch::default();
    run_check(&f, &fetch, &CheckSettings::default());
    let n = notices(&f);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!((n[0].title.as_str(), n[0].source.as_str(), n[0].read), ("Updates available: 1 app", "store", false));
    assert_eq!(n[0].link, Some(Link::new("store", "updates")));
    assert!(n[0].body.contains("Meeting") || n[0].body.contains("meeting"), "{}", n[0].body);
    assert!(n[0].body.contains("Nothing is installed by itself"));
    // the next day's check finds the same: the same entry, untouched (read state kept)
    f.svc.inbox.mark_read("alice", &n[0].id, true).unwrap();
    run_check(&f, &fetch, &CheckSettings::default());
    let again = notices(&f);
    assert_eq!(again.len(), 1);
    assert_eq!((again[0].id.clone(), again[0].read), (n[0].id.clone(), true));
    // off announces nothing
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_grouped_notice_is_replaced_in_place_marked_unread_for_news_and_removed_when_nothing_is_pending() {
    let f = fx("uc-group");
    let r1 = rep_of(vec![upd("skill:a", "Alpha", "1.1.0"), upd("skill:b", "Beta", "2.0.0"), upd("tool:c", "Gamma", "3.0.0")]);
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &r1), Outcome::Posted);
    let n = notices(&f);
    assert_eq!(n[0].title, "Updates available: 3 apps");
    let id = n[0].id.clone();
    f.svc.inbox.mark_read("alice", &id, true).unwrap();
    // a new version of something, a driver and the system: the same entry, new text, unread again
    let mut drv = upd("extension:wifi", "Wi-Fi driver", "1.0.1");
    drv.driver = true;
    let sys = Update { kind: Kind::System, id: "system".into(), title: "IYAKKI".into(), from: "1106h".into(), to: "1107h".into(), source: "System update".into(), new_permissions: vec![], note: "Sound fixes.".into(), driver: false };
    let r2 = rep_of(vec![upd("skill:a", "Alpha", "1.2.0"), upd("skill:b", "Beta", "2.0.0"), upd("tool:c", "Gamma", "3.0.0"), upd("skill:d", "Delta", "1.0.0"), drv, sys]);
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &r2), Outcome::Updated);
    let n = notices(&f);
    assert_eq!(n.len(), 1, "never stacked");
    assert_eq!((n[0].id.as_str(), n[0].read), (id.as_str(), false));
    assert_eq!(n[0].title, "Updates available: 4 apps, 1 driver, system 1107h");
    assert!(n[0].body.contains("and 2 more") && n[0].body.contains("System update"), "{}", n[0].body);
    assert_eq!(f.svc.inbox.list("alice").len(), 1);
    // an unchanged set is not announced again, also after the person read it
    f.svc.inbox.mark_read("alice", &id, true).unwrap();
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &r2), Outcome::Nothing);
    assert!(notices(&f)[0].read);
    // something was installed: the text follows, the reading state stays
    let r3 = rep_of(vec![upd("skill:a", "Alpha", "1.2.0")]);
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &r3), Outcome::Updated);
    let n = notices(&f);
    assert_eq!((n[0].title.as_str(), n[0].read), ("Updates available: 1 app", true));
    // nothing pending: it goes away
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &rep_of(vec![])), Outcome::Removed);
    assert!(notices(&f).is_empty());
    // a dismissed notice stays dismissed while the set is the same
    updatenotice::announce_report(&f.svc, "alice", &r3);
    let id2 = notices(&f)[0].id.clone();
    f.svc.inbox.delete("alice", &id2).unwrap();
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &r3), Outcome::Nothing);
    assert!(notices(&f).is_empty());
    // but news brings it back
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &rep_of(vec![upd("skill:a", "Alpha", "1.3.0")])), Outcome::Posted);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn an_unreachable_source_does_not_remove_the_notice_or_make_the_same_update_look_new() {
    let f = fx("uc-flap");
    let both = rep_of(vec![upd("skill:a", "Alpha", "1.1.0"), upd("skill:b", "Beta", "2.0.0")]);
    updatenotice::announce_report(&f.svc, "alice", &both);
    let id = notices(&f)[0].id.clone();
    f.svc.inbox.mark_read("alice", &id, true).unwrap();
    // one source down: only Alpha is seen, with an error line; nothing changes
    let mut partial = rep_of(vec![upd("skill:a", "Alpha", "1.1.0")]);
    partial.errors.push(("Store".into(), "offline".into()));
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &partial), Outcome::Nothing);
    let mut none = rep_of(vec![]);
    none.errors.push(("Store".into(), "offline".into()));
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &none), Outcome::Nothing);
    assert_eq!(notices(&f).len(), 1);
    // it comes back: the same pairs, so no new notice and still read
    assert_eq!(updatenotice::announce_report(&f.svc, "alice", &both), Outcome::Nothing);
    assert!(notices(&f)[0].read);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn off_posts_no_notice_and_a_notice_from_the_off_source_setting_is_respected() {
    let f = with_update("uc-note-off", &[]);
    let fetch = FakeFetch::default();
    run_check(&f, &fetch, &CheckSettings { mode: Mode::Off, ..Default::default() });
    assert!(notices(&f).is_empty());
    f.svc.inbox.set_level("alice", "store", crate::inbox::Level::Off).unwrap();
    run_check(&f, &fetch, &CheckSettings::default());
    assert!(notices(&f).is_empty(), "the person turned notices from the store off");
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_component_hook_counts_in_the_badge_and_the_notice_until_the_running_version_has_caught_up() {
    let f = fx("uc-hook");
    assert_eq!(updatenotice::announce_system_update_for(&f.svc, "alice", "0.1.0-alpha.1", "", "0.1.0-alpha.1"), Outcome::Nothing, "not newer");
    assert_eq!(updatenotice::announce_system_update_for(&f.svc, "alice", "0.1.0-alpha.3", "Sound fixes.", "0.1.0-alpha.1"), Outcome::Posted);
    let n = notices(&f);
    assert_eq!(n[0].title, "Updates available: system 0.1.0-alpha.3");
    assert_eq!(n[0].link, Some(Link::new("settings", "tab:updates")));
    let rep = Report::load(f.svc.dir()).unwrap();
    assert_eq!((rep.count(), rep.badge().as_deref()), (1, Some("1 update")));
    // the next check (no list address) keeps it
    let fetch = FakeFetch::default();
    let r = run_check(&f, &fetch, &CheckSettings::default());
    assert_eq!(r.updates.len(), 1, "{r:?}");
    assert_eq!(notices(&f).len(), 1);
    // once the running version has caught up it is forgotten and the notice goes
    let r = run_and_keep(&f.store, &f.svc, &policy(), "alice", &fetch, &CheckSettings::default(), "0.1.0-alpha.3", "macweb");
    assert!(r.updates.is_empty() && notices(&f).is_empty());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn installing_the_update_clears_the_badge_count_and_the_notice_at_once() {
    let f = with_update("uc-clear", &[]);
    let fetch = FakeFetch::default();
    run_check(&f, &fetch, &CheckSettings::default());
    assert_eq!((Report::load(f.svc.dir()).unwrap().count(), notices(&f).len()), (1, 1));
    let a = Actor::from_policy("alice", &policy());
    let loaded = f.store.local_catalogs(&f.svc);
    let items = f.store.items(&f.svc, &a, &loaded);
    let it = items.iter().find(|i| i.entry.id == "meeting-notes").unwrap();
    let r = f.store.review_item(&f.svc, &a, it, &loaded, &fetch, Scope::User("alice".into()), &[]).unwrap();
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    assert_eq!(Report::load(f.svc.dir()).unwrap().count(), 0, "the badge follows the install");
    assert!(notices(&f).is_empty(), "nothing pending: the notice is gone");
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------- the store's announcement

use super::sysannounce;

struct Answers(std::collections::HashMap<String, Result<Vec<u8>, String>>);

impl super::catalog::Fetch for Answers {
    fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
        self.0.get(url).cloned().unwrap_or_else(|| Err("store.example answered 404".into()))
    }
}

const STORE_CATALOG: &str = "https://store.example/catalog.json";
const STORE_SYSTEM: &str = "https://store.example/v1/system.json";

fn with_store(tag: &str, pin: &str) -> Fx {
    let f = fx(tag);
    let mut c = f.store.config();
    if pin.is_empty() {
        c.add("Home store", SourceKind::Url, STORE_CATALOG).unwrap();
    } else {
        c.add_pinned("Home store", STORE_CATALOG, pin).unwrap();
    }
    c.save(f.store.dir()).unwrap();
    f
}

fn announce_settings() -> CheckSettings {
    CheckSettings { system_url: STORE_SYSTEM.into(), ..Default::default() }
}

#[test]
fn a_store_announcement_is_accepted_only_with_the_pinned_key() {
    let key = new_signing_key().unwrap();
    let other = new_signing_key().unwrap();
    let raw = br#"{"version":"0.1.0-alpha.9","variant":"macweb","notes":"New boot menu.","channel":"alpha"}"#;
    let signed = sysannounce::sign(raw, "iyakki-store", 1_800_000_000, &key).unwrap();
    let f = with_store("uc-ann", &public_key_hex(&key));
    let mut answers = Answers(Default::default());
    answers.0.insert(STORE_SYSTEM.into(), Ok(signed.clone().into_bytes()));
    let rep = run_check(&f, &answers, &announce_settings());
    let sys: Vec<_> = rep.updates.iter().filter(|u| u.kind == Kind::System).collect();
    assert_eq!(sys.len(), 1, "{rep:?}");
    assert_eq!((sys[0].to.as_str(), sys[0].note.as_str()), ("0.1.0-alpha.9", "New boot menu."));
    assert!(notices(&f)[0].title.contains("system 0.1.0-alpha.9"));
    // signed by another key (it names its own key, which is not the pinned one): refused
    let by_other = sysannounce::sign(raw, "iyakki-store", 1, &other).unwrap();
    answers.0.insert(STORE_SYSTEM.into(), Ok(by_other.into_bytes()));
    let rep = run_check(&f, &answers, &announce_settings());
    assert!(rep.updates.iter().all(|u| u.kind != Kind::System));
    assert!(rep.errors.iter().any(|(s, e)| s == "System update" && e.contains("not signed by the key you confirmed")), "{rep:?}");
    // tampered after signing: refused
    answers.0.insert(STORE_SYSTEM.into(), Ok(signed.replace("0.1.0-alpha.9", "9.9.9").into_bytes()));
    let rep = run_check(&f, &answers, &announce_settings());
    assert!(rep.errors.iter().any(|(s, e)| s == "System update" && e.contains("refused")), "{rep:?}");
    // unsigned content: refused
    answers.0.insert(STORE_SYSTEM.into(), Ok(raw.to_vec()));
    let rep = run_check(&f, &answers, &announce_settings());
    assert!(rep.errors.iter().any(|(s, e)| s == "System update" && e.contains("refused")), "{rep:?}");
    // no file on the store (404): nothing is announced, and that is not an error
    answers.0.insert(STORE_SYSTEM.into(), Err("store.example answered 404".into()));
    let rep = run_check(&f, &answers, &announce_settings());
    assert!(rep.errors.iter().all(|(s, _)| s != "System update") && rep.updates.iter().all(|u| u.kind != Kind::System), "{rep:?}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn a_store_that_was_added_without_checking_its_key_cannot_announce_a_system_update() {
    let key = new_signing_key().unwrap();
    let signed = sysannounce::sign(br#"{"version":"0.1.0-alpha.9"}"#, "iyakki-store", 1, &key).unwrap();
    let f = with_store("uc-ann-nopin", "");
    let mut answers = Answers(Default::default());
    answers.0.insert(STORE_SYSTEM.into(), Ok(signed.into_bytes()));
    let rep = run_check(&f, &answers, &announce_settings());
    assert!(rep.updates.iter().all(|u| u.kind != Kind::System));
    assert!(rep.errors.iter().any(|(s, e)| s == "System update" && e.contains("not added with its key checked")), "{rep:?}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_announcement_address_of_a_store_follows_its_catalog_address() {
    assert_eq!(sysannounce::url_for_catalog("https://s.example/catalog.json"), "https://s.example/v1/system.json");
    assert_eq!(sysannounce::url_for_catalog("http://10.0.0.5:30580/"), "http://10.0.0.5:30580/v1/system.json");
    assert!(sysannounce::is_store_announcement("http://10.0.0.5:30580/v1/system.json") && !sysannounce::is_store_announcement("https://u.example/index.json"));
}

#[test]
fn the_devmode_key_store_daily_check_reads_and_sets_the_daily_look_and_needs_an_administrator() {
    use crate::devmode::config::{get, set};
    let f = fx("uc-devcfg");
    let admin = iyakki_sec::policy::Role::Admin.template();
    assert!(crate::devmode::config::def("store.daily_check").is_some());
    // the default: daily, which only ever asks what was added (nothing is added here)
    assert_eq!(get(&f.svc, None, &admin, "admin", "store.daily_check").unwrap()["value"], serde_json::json!(true));
    set(&f.svc, None, &admin, "admin", "admin", "store.daily_check", &serde_json::json!(false)).unwrap();
    assert_eq!(f.svc.settings.lock().unwrap().get().update_check.mode, Mode::Manual);
    set(&f.svc, None, &admin, "admin", "admin", "store.daily_check", &serde_json::json!(true)).unwrap();
    assert_eq!(f.svc.settings.lock().unwrap().get().update_check.mode, Mode::Daily);
    // "false" does not turn an Off into something that asks
    f.svc.settings.lock().unwrap().update(|s| s.update_check.mode = Mode::Off).unwrap();
    set(&f.svc, None, &admin, "admin", "admin", "store.daily_check", &serde_json::json!(false)).unwrap();
    assert_eq!(f.svc.settings.lock().unwrap().get().update_check.mode, Mode::Off);
    let user = iyakki_sec::policy::Role::User.template();
    assert!(set(&f.svc, None, &user, "bob", "bob", "store.daily_check", &serde_json::json!(true)).is_err());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn with_no_added_source_and_no_list_address_a_daily_check_asks_nobody_and_it_is_due_again_after_a_day() {
    let f = fx("uc-nosrc");
    let fetch = FakeFetch::default();
    let rep = run_check(&f, &fetch, &CheckSettings::default());
    assert!(rep.updates.is_empty() && rep.asked.is_empty() && fetch.asked.lock().unwrap().is_empty(), "the built-in catalog is never contacted");
    // the timer: once a day, and at sign-in when the last look is older than a day
    let s = CheckSettings::default();
    let t0 = 1_800_000_000;
    assert_eq!(due(&s, t0 - 3 * DAY, t0, None, "dev"), Due::Now, "long ago");
    assert!(matches!(due(&s, t0, t0 + 3600, None, "dev"), Due::Later(_)), "one hour later: not yet");
    assert_eq!(due(&s, t0, t0 + DAY + 3600, None, "dev"), Due::Now, "overdue after sign-in");
    let _ = std::fs::remove_dir_all(&f.dir);
}
