// The known-bad site list through the real browser: a listed site is not asked, the person is told why, and taking it off the
// list opens it again. Included (include!) into the browser's test module.

use crate::apps::browser_badsites::{self as bsites, Pin as BadPin, TEST_LOCK as BAD_LOCK};
use iyakki_web::badsites::{BadSiteFetch, Guard, INTERSTITIAL_TITLE as BAD_TITLE};

fn ptext(a: &BrowserApp) -> String {
    let d = &a.page().unwrap().doc;
    let mut o = d.title.clone();
    for b in &d.blocks {
        if let Block::Text(t) = b {
            for sp in &t.spans {
                o.push(' ');
                o.push_str(&sp.text);
            }
        }
    }
    o
}

#[test]
fn a_listed_site_is_not_asked_a_page_explains_why_and_removing_it_opens_the_site() {
    let _turn = BAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let web = site(vec![("https://evil.example/", html("<html><head><title>Win a prize</title></head><body><p>Type your password</p></body></html>")), ("https://fine.example/", html("<html><head><title>Fine</title></head><body><p>All good</p></body></html>"))]);
    let mut cx = cx_with("badsites1", web.clone());
    cx.web = Arc::new(BadSiteFetch::new(web.clone(), Guard::global()));
    let _ = std::fs::remove_file(bsites::list_path(&cx));
    let _pin = BadPin::to(&cx);
    bsites::forget_loaded();
    bsites::apply(&cx);
    bsites::add(&cx, "evil.example", "phishing a bank").unwrap();
    let mut a = BrowserApp::new(&cx);
    a.open_url("login.evil.example", &cx);
    settle(&mut a, &cx);
    assert!(web.asked.lock().unwrap().is_empty(), "nothing was sent to a listed site: {:?}", web.asked.lock().unwrap());
    let shown = ptext(&a);
    assert!(shown.contains("known-bad site list") && shown.contains("phishing a bank"), "{shown}");
    assert!(!shown.contains("Type your password"));
    assert_eq!(a.page().unwrap().doc.title, BAD_TITLE);
    // another site is unaffected
    a.open_url("fine.example", &cx);
    settle(&mut a, &cx);
    assert!(ptext(&a).contains("All good"));
    // switched off, the listed site opens
    bsites::set_enabled(&cx, false);
    a.open_url("evil.example", &cx);
    settle(&mut a, &cx);
    assert!(ptext(&a).contains("Type your password"));
    bsites::set_enabled(&cx, true);
    // taken off the list, it opens again
    assert!(bsites::remove(&cx, "evil.example"));
    web.asked.lock().unwrap().clear();
    a.open_url("evil.example", &cx);
    settle(&mut a, &cx);
    assert!(ptext(&a).contains("Type your password"));
    assert!(web.asked.lock().unwrap().iter().any(|u| u.contains("evil.example")));
    let _ = std::fs::remove_file(bsites::list_path(&cx));
    bsites::forget_loaded();
}

#[test]
fn a_downloaded_list_adds_its_sites_and_a_failed_download_changes_nothing() {
    let _turn = BAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let web = site(vec![]);
    let cx = cx_with("badsites2", web);
    let path = bsites::list_path(&cx);
    let _ = std::fs::remove_file(&path);
    struct Src(Result<Vec<u8>, String>);
    impl iyakki_web::blocker_lists::ListSource for Src {
        fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
            self.0.clone()
        }
    }
    let wait = || {
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            if !bsites::busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the download never finished");
    };
    assert!(bsites::start_download(path.clone(), Arc::new(Src(Ok(b"# hosts\n0.0.0.0 a.example\nb.example  malware\nhttp://c.example/x.exe\n".to_vec()))), "https://lists.example/hosts".into()));
    wait();
    assert!(bsites::job_status().starts_with("Done: 2 sites read, 1 lines skipped"), "{}", bsites::job_status());
    let list = bsites::load_list(&cx);
    assert!(list.check_host("a.example").is_some() && list.check_host("x.b.example").is_some() && list.check_host("c.example").is_none());
    assert!(bsites::start_download(path.clone(), Arc::new(Src(Err("the server answered 500".into()))), "https://lists.example/hosts".into()));
    wait();
    assert!(bsites::job_status().contains("failed") && bsites::load_list(&cx).len() == 2, "{}", bsites::job_status());
    assert_eq!(bsites::clear(&cx), 2);
    assert!(bsites::load_list(&cx).is_empty());
}
