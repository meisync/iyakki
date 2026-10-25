// The browser's side of ad and tracker blocking: the counter, the list of what was blocked, A for "allow on this site", and the
// explanation page for a blocked page. Included (include!) into the browser's test module.

use crate::apps::browser_block::{forget_loaded, lists_dir, store, Pin, TEST_LOCK};
use iyakki_web::blocker_state::{Filter, FilteredFetch, INTERSTITIAL_TITLE};

fn with_filter(tag: &str, rules: &str, pages: Vec<(&str, Response)>) -> (AppCx, Arc<Site>, Pin) {
    let web = site(pages);
    let mut cx = cx_with(tag, web.clone());
    cx.web = Arc::new(FilteredFetch::new(web.clone(), Filter::global()));
    let _ = std::fs::remove_dir_all(lists_dir(&cx));
    store(&cx).put("test", "Test list", "", "", rules.as_bytes(), 1).unwrap();
    let pin = Pin::to(&cx);
    forget_loaded();
    browser_block::apply(&cx);
    (cx, web, pin)
}

fn wait_for(a: &mut BrowserApp, cx: &AppCx, what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened: {what}");
}

const PAGE: &str = "<html><head><title>News</title></head><body><p>Headlines</p><img src=\"https://ads.example.com/b.gif\" width=10 height=10><img src=\"https://cdn.news.org/ok.gif\" width=10 height=10></body></html>";

#[test]
fn blocked_requests_are_counted_listed_and_one_key_allows_the_site() {
    let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (cx, web, _pin) = with_filter("blkui1", "||ads.example.com^\n", vec![("https://news.org/", html(PAGE))]);
    let mut a = BrowserApp::new(&cx);
    a.open_url("news.org", &cx);
    settle(&mut a, &cx);
    let f = Filter::global();
    wait_for(&mut a, &cx, "the picture request was seen and blocked", || f.page_blocked().1 >= 1);
    assert_eq!(f.page_blocked().0, "news.org");
    assert!(!web.asked.lock().unwrap().iter().any(|u| u.contains("ads.example.com")), "a blocked request is never sent");
    let log = f.page_log();
    assert!(log[0].url.contains("ads.example.com") && log[0].rule == "||ads.example.com^");
    // the chip and the list draw
    let _ = frame(&mut a, &cx, 1280, 720);
    key(&mut a, &cx, Key::Char('B'));
    assert!(a.show_blocked);
    let _ = frame(&mut a, &cx, 1280, 720);
    let _ = frame(&mut a, &cx, 360, 740);
    key(&mut a, &cx, Key::Char('B'));
    assert!(!a.show_blocked);
    // A: allowed on this site, and the page loads again with its pictures
    web.asked.lock().unwrap().clear();
    key(&mut a, &cx, Key::Char('a'));
    assert_eq!(a.status_text(), Some("Ads and trackers are allowed on news.org."));
    assert!(browser_block::load(&cx).allow.contains("news.org"));
    settle(&mut a, &cx);
    wait_for(&mut a, &cx, "the picture was fetched", || web.asked.lock().unwrap().iter().any(|u| u.contains("ads.example.com")));
    assert_eq!(f.page_blocked().1, 0);
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("browser.block.allow") && l.contains("news.org")));
    // A again: blocking is back
    key(&mut a, &cx, Key::Char('a'));
    assert_eq!(a.status_text(), Some("Blocking is back on for news.org."));
    assert!(browser_block::load(&cx).allow.is_empty());
    store(&cx).remove("test");
    forget_loaded();
}

#[test]
fn a_blocked_page_shows_why_and_one_key_opens_it() {
    let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (cx, web, _pin) = with_filter("blkui2", "||bad.example^$document\n", vec![("https://bad.example/", html("<html><head><title>Real page</title></head><body><p>Real words</p></body></html>"))]);
    let mut a = BrowserApp::new(&cx);
    a.open_url("bad.example", &cx);
    settle(&mut a, &cx);
    assert_eq!(a.page().unwrap().doc.title, INTERSTITIAL_TITLE, "never a silent failure");
    assert!(web.asked.lock().unwrap().is_empty(), "the page was not requested");
    assert!(a.page().unwrap().doc.blocks.iter().any(|b| matches!(b, Block::Text(t) if t.spans.iter().any(|sp| sp.text.contains("||bad.example^$document")))), "the rule is named");
    key(&mut a, &cx, Key::Char('a'));
    settle(&mut a, &cx);
    assert_eq!(a.page().unwrap().doc.title, "Real page");
    assert_eq!(web.asked.lock().unwrap().len(), 1);
    store(&cx).remove("test");
    forget_loaded();
}

#[test]
fn with_blocking_off_nothing_is_blocked_and_a_says_so() {
    let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (cx, web, _pin) = with_filter("blkui3", "||ads.example.com^\n", vec![("https://news.org/", html(PAGE))]);
    browser_block::save(&cx, &browser_block::BlockConfig { enabled: false, ..Default::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("news.org", &cx);
    settle(&mut a, &cx);
    wait_for(&mut a, &cx, "the picture was fetched", || web.asked.lock().unwrap().iter().any(|u| u.contains("ads.example.com")));
    key(&mut a, &cx, Key::Char('a'));
    assert!(a.status_text().unwrap().contains("Blocking is off everywhere"));
    store(&cx).remove("test");
    forget_loaded();
    browser_block::apply(&cx);
}
