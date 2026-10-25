use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_web::blocker::ResType;
use iyakki_web::blocker_lists::ListSource;
use std::collections::HashMap;

struct FakeServer(Mutex<HashMap<String, Result<Vec<u8>, String>>>);

impl FakeServer {
    fn with(pages: &[(&str, &str)]) -> FakeServer {
        FakeServer(Mutex::new(pages.iter().map(|(u, t)| (u.to_string(), Ok(t.as_bytes().to_vec()))).collect()))
    }
}

impl ListSource for FakeServer {
    fn get(&self, url: &str, _: usize) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().get(url).cloned().unwrap_or_else(|| Err("the server answered 404".into()))
    }
}

#[test]
fn blocking_is_on_by_default_and_choices_are_saved_per_person() {
    let cx = cx_with_admin_store("blk1");
    assert_eq!(load(&cx), BlockConfig::default());
    assert!(load(&cx).enabled && load(&cx).allow.is_empty());
    let mut c = load(&cx);
    c.enabled = false;
    c.allow.insert("news.org".into());
    save(&cx, &c);
    assert_eq!(load(&cx), c);
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert_eq!(load(&other), BlockConfig::default());
    std::fs::write(config_path(&cx), "{ nope").unwrap();
    assert_eq!(load(&cx), BlockConfig::default(), "a damaged file means the defaults");
    std::fs::write(config_path(&cx), r#"{"allow":["ok.org","bad host","../x","UP.org"],"enabled":"yes"}"#).unwrap();
    let d = load(&cx);
    assert_eq!(d.allow.iter().cloned().collect::<Vec<_>>(), vec!["ok.org".to_string(), "up.org".to_string()]);
    assert!(d.enabled, "a value of the wrong type is ignored");
}

#[test]
fn the_per_site_toggle_flips_and_is_listed() {
    let cx = cx_with_admin_store("blk2");
    assert!(toggle_allow(&cx, "News.org"));
    assert!(load(&cx).allow.contains("news.org"));
    assert!(!toggle_allow(&cx, "news.org"));
    assert!(load(&cx).allow.is_empty());
}

#[test]
fn the_standard_lists_are_fetched_only_by_a_job_and_then_block() {
    let cx = cx_with_admin_store("blk3");
    let store = store(&cx);
    assert!(store.list().is_empty(), "nothing is downloaded by itself");
    let srv = FakeServer::with(&[(KNOWN[0].url, "||ads.example.com^\n"), (KNOWN[1].url, "||track.example.net^$third-party\n##.ad\n")]);
    let msg = run_job(&store, &srv, &Job::DownloadStandard, 100);
    assert!(msg.contains("EasyList (1 rules)") && msg.contains("EasyPrivacy (2 rules)"), "{msg}");
    let lists = store.list();
    assert_eq!(lists.len(), 2);
    assert!(lists.iter().all(|m| m.hash.len() == 64 && m.licence.contains("GPL") && m.updated == 100 && m.url.starts_with("https://easylist.to/")));
    // a failing server for one list is reported, the other still lands
    let half = FakeServer::with(&[(KNOWN[0].url, "||other.example^\n")]);
    let msg = run_job(&store, &half, &Job::UpdateAll, 200);
    assert!(msg.contains("updated EasyList") && msg.contains("failed EasyPrivacy"), "{msg}");
    assert_eq!(store.list().iter().find(|m| m.id == "easylist").unwrap().updated, 200);
    assert_eq!(store.list().iter().find(|m| m.id == "easyprivacy").unwrap().updated, 100, "a failed update keeps the old list");
    assert!(run_job(&ListStore::open(temp_dir("blk3-empty")), &half, &Job::UpdateAll, 1).contains("no list"));
    assert!(run_job(&store, &srv, &Job::AddUrl("not a url".into()), 1).starts_with("not added"));
    let f = temp_dir("blk3-file").join("my.txt");
    std::fs::write(&f, "||mine.example^\n").unwrap();
    assert!(run_job(&store, &srv, &Job::AddFile(f), 5).starts_with("added my.txt"));
    let b = store.compile();
    assert!(b.is_blocked("https://mine.example/x.js", "https://a.org/", ResType::Script));
}

#[test]
fn apply_puts_the_persons_lists_into_the_shared_filter_and_honours_the_switch() {
    let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cx = cx_with_admin_store("blk4");
    let f = iyakki_web::blocker_state::Filter::global();
    let store = store(&cx);
    store.put("one", "One", "", "", b"||ads.blk4.example^\n", 1).unwrap();
    forget_loaded();
    apply(&cx);
    assert!(f.enabled());
    assert!(f.blocker().is_blocked("https://ads.blk4.example/x.js", "https://a.org/", ResType::Script));
    // switched off: the filter says so (the lists stay)
    save(&cx, &BlockConfig { enabled: false, ..BlockConfig::default() });
    apply(&cx);
    assert!(!f.enabled());
    let net = Arc::new(FakeNet);
    let ff = iyakki_web::blocker_state::FilteredFetch::new(net, f.clone());
    let req = iyakki_web::Request { referer: "https://a.org/".into(), priority: iyakki_web::net::BLOCKING, ..iyakki_web::Request::get(&iyakki_web::Url::parse("https://ads.blk4.example/x.js").unwrap()) };
    assert!(iyakki_web::Fetch::send(&ff, &req).is_ok(), "off: sent");
    save(&cx, &BlockConfig::default());
    apply(&cx);
    assert!(iyakki_web::Fetch::send(&ff, &req).is_err(), "on: blocked");
    // a list file that went missing leaves nothing blocked, never an error
    std::fs::remove_file(lists_dir(&cx).join("one.txt")).unwrap();
    forget_loaded();
    apply(&cx);
    assert!(iyakki_web::Fetch::send(&ff, &req).is_ok(), "fails open");
    store.remove("one");
}

struct FakeNet;
impl iyakki_web::Fetch for FakeNet {
    fn get(&self, _: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
        Ok(iyakki_web::Response { status: 200, ..Default::default() })
    }
}

#[test]
fn the_full_engine_gets_a_rule_list_only_when_there_are_rules_and_blocking_is_on() {
    let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cx = cx_with_admin_store("blk5");
    let profile = temp_dir("blk5-profile");
    std::fs::create_dir_all(&profile).unwrap();
    forget_loaded();
    assert!(prepare_full_engine(&cx, &profile).is_empty(), "no lists, no rule file");
    assert!(!profile.join("content-filter.json").exists());
    store(&cx).put("one", "One", "", "", b"||ads.blk5.example^\n##.ad\n", 1).unwrap();
    forget_loaded();
    let env = prepare_full_engine(&cx, &profile);
    assert_eq!(env.len(), 2, "{env:?}");
    let json = std::fs::read_to_string(profile.join("content-filter.json")).unwrap();
    assert!(json.contains("ads\\\\.blk5\\\\.example") && json.contains("css-display-none"));
    let id = std::fs::read_to_string(profile.join("content-filter.id")).unwrap();
    // the compiled copy WebKit made is thrown away only when the rules change
    std::fs::create_dir_all(profile.join("content-filter-store")).unwrap();
    prepare_full_engine(&cx, &profile);
    assert!(profile.join("content-filter-store").exists(), "same rules, same compiled copy");
    // allowing a site changes the rules
    let mut c = load(&cx);
    c.allow.insert("news.org".into());
    save(&cx, &c);
    prepare_full_engine(&cx, &profile);
    assert_ne!(std::fs::read_to_string(profile.join("content-filter.id")).unwrap(), id);
    assert!(!profile.join("content-filter-store").exists());
    assert!(std::fs::read_to_string(profile.join("content-filter.json")).unwrap().contains("*news.org"));
    // off: the files go
    c.enabled = false;
    save(&cx, &c);
    assert!(prepare_full_engine(&cx, &profile).is_empty());
    assert!(!profile.join("content-filter.json").exists() && !profile.join("content-filter.id").exists());
    store(&cx).remove("one");
}

#[test]
fn ages_are_plain_words() {
    assert_eq!(ago(100, 100), "just now");
    assert_eq!(ago(0, 120), "2 min ago");
    assert_eq!(ago(0, 7200), "2 h ago");
    assert_eq!(ago(0, 7 * 86400), "7 days ago");
    assert_eq!(ago(500, 100), "just now", "a clock that went back");
}

#[test]
fn a_second_job_is_refused_while_one_runs() {
    set_job("Working…");
    assert!(busy());
    assert!(!start_job(PathBuf::from("/nonexistent"), Arc::new(FakeServer::with(&[])), Job::UpdateAll));
    set_job("Done: x");
    assert!(!busy());
    set_job("");
}
