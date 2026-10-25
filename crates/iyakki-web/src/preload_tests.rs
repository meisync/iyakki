use super::*;
use crate::net::{LimitedFetch, Limiter};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

/// A fake server with a fixed latency per request that records what was asked and when.
struct Site {
    delay: Duration,
    log: Mutex<Vec<(String, String, u8)>>,
    now: AtomicUsize,
    peak: AtomicUsize,
    fail: Mutex<Vec<String>>,
}

impl Site {
    fn new(ms: u64) -> Arc<Site> {
        Arc::new(Site { delay: Duration::from_millis(ms), log: Mutex::new(vec![]), now: AtomicUsize::new(0), peak: AtomicUsize::new(0), fail: Mutex::new(vec![]) })
    }
    fn count(&self, path: &str) -> usize {
        self.log.lock().unwrap().iter().filter(|(p, _, _)| p == path).count()
    }
}

impl Fetch for Arc<Site> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(n, Ordering::SeqCst);
        self.log.lock().unwrap().push((req.url.path.clone(), req.cookie.clone(), req.priority));
        std::thread::sleep(self.delay);
        self.now.fetch_sub(1, Ordering::SeqCst);
        if self.fail.lock().unwrap().contains(&req.url.path) {
            return Err("connection reset".into());
        }
        Ok(Response { status: 200, body: req.url.path.clone().into_bytes(), content_type: "text/plain".into(), ..Response::default() })
    }
}

fn page() -> Url {
    Url::parse("https://a.org/dir/page.html").unwrap()
}

fn allow_all() -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(|_| true)
}

fn hints(paths: &[(&str, Kind)]) -> Vec<Hint> {
    paths.iter().map(|(p, k)| Hint { url: Url::parse(&format!("https://a.org{p}")).unwrap(), kind: *k }).collect()
}

const ALL: Wants = Wants { styles: true, scripts: true, images: true };

#[test]
fn the_scanner_finds_sheets_scripts_and_pictures_in_the_order_the_page_wants_them() {
    let html = r##"<!doctype html><html><head>
        <base href="/assets/">
        <link rel="stylesheet" href="a.css">
        <link rel="preload" as="style" href="b.css">
        <link rel="preload" as="script" href="pre.js">
        <link rel="modulepreload" href="mod.js">
        <link rel="stylesheet" href="print.css" media="print">
        <link rel="alternate stylesheet" href="alt.css">
        <link rel="icon" href="icon.png">
        <script src="one.js"></script>
        <script async src="https://cdn.other.net/two.js"></script>
        <script type="module" src="three.mjs"></script>
        <script nomodule src="legacy.js"></script>
        <script type="application/ld+json" src="data.json"></script>
        <script src="http://insecure.example/x.js"></script>
        <script>inline()</script>
        </head><body>
        <img src="p1.jpg"><img src="data:image/png;base64,AAAA"><img src="p2.png" loading="lazy"><img src="p1.jpg"><img src="/abs/p3.gif">
        </body></html>"##;
    let h = scan(html, &page(), ALL);
    let got: Vec<String> = h.iter().map(|x| format!("{:?} {}", x.kind, x.url)).collect();
    assert_eq!(
        got,
        [
            "Style https://a.org/assets/a.css",
            "Style https://a.org/assets/b.css",
            "Script https://a.org/assets/pre.js",
            "Script https://a.org/assets/mod.js",
            "Script https://a.org/assets/one.js",
            "Script https://cdn.other.net/two.js",
            "Script https://a.org/assets/three.mjs",
            "Image https://a.org/assets/p1.jpg",
            "Image https://a.org/abs/p3.gif",
        ],
        "relative to the base, no print sheets, alternates, nomodule, data, lazy, duplicate or unprotected files"
    );
}

#[test]
fn the_scanner_only_wants_what_the_page_will_use() {
    let html = "<link rel=stylesheet href=a.css><script src=a.js></script><img src=a.png>";
    let only = |w: Wants| scan(html, &page(), w).iter().map(|h| h.kind).collect::<Vec<_>>();
    assert_eq!(only(Wants { styles: true, scripts: false, images: false }), [Kind::Style]);
    assert_eq!(only(Wants { styles: false, scripts: true, images: false }), [Kind::Script]);
    assert_eq!(only(Wants { styles: false, scripts: false, images: true }), [Kind::Image]);
    assert!(only(Wants { styles: false, scripts: false, images: false }).is_empty());
    // a page of a thousand pictures starts only a few
    let many: String = (0..1000).map(|i| format!("<img src=i{i}.png>")).collect();
    assert_eq!(scan(&many, &page(), ALL).len(), MAX_IMAGE_HINTS);
    // garbage does not panic
    assert!(scan("<link rel=stylesheet href='http://[::1'><script src='", &page(), ALL).len() <= 2);
}

#[test]
fn many_scripts_arrive_in_parallel_and_the_serial_reader_does_not_wait_for_each() {
    let site = Site::new(40);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 8);
    let list: Vec<(String, Kind)> = (0..16).map(|i| (format!("/s{i}.js"), Kind::Script)).collect();
    let h = hints(&list.iter().map(|(p, k)| (p.as_str(), *k)).collect::<Vec<_>>());
    let t = Instant::now();
    pre.start(&h);
    // the page asks for them one after the other, as its script loop does
    for (p, _) in &list {
        let r = pre.get(&Url::parse(&format!("https://a.org{p}")).unwrap()).unwrap();
        assert_eq!(r.body, p.as_bytes());
    }
    let took = t.elapsed();
    assert!(took < Duration::from_millis(16 * 40 / 3), "16 fetches of 40 ms took {took:?}: they did not overlap");
    assert!(site.peak.load(Ordering::SeqCst) >= 4);
    assert!(site.peak.load(Ordering::SeqCst) <= 8, "the workers limit holds");
    for i in 0..16 {
        assert_eq!(site.count(&format!("/s{i}.js")), 1, "each file was fetched once, not once early and once by the page");
    }
}

#[test]
fn a_request_for_something_in_flight_joins_it_instead_of_fetching_again() {
    let site = Site::new(80);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 2);
    pre.start(&hints(&[("/x.css", Kind::Style)]));
    std::thread::sleep(Duration::from_millis(20));
    let t = Instant::now();
    pre.get(&Url::parse("https://a.org/x.css").unwrap()).unwrap();
    assert!(t.elapsed() < Duration::from_millis(75), "the wait was the rest of the fetch, not a new one");
    assert_eq!(site.count("/x.css"), 1);
}

#[test]
fn a_request_for_something_still_queued_goes_at_once_and_the_queue_skips_it() {
    let site = Site::new(60);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 1);
    // one worker: the second and third wait their turn behind the first
    pre.start(&hints(&[("/a.js", Kind::Script), ("/b.js", Kind::Script), ("/c.js", Kind::Script)]));
    std::thread::sleep(Duration::from_millis(10));
    let t = Instant::now();
    pre.get(&Url::parse("https://a.org/c.js").unwrap()).unwrap();
    assert!(t.elapsed() < Duration::from_millis(110), "{:?}: it was not made to wait behind a.js and b.js", t.elapsed());
    pre.get(&Url::parse("https://a.org/a.js").unwrap()).unwrap();
    pre.get(&Url::parse("https://a.org/b.js").unwrap()).unwrap();
    assert_eq!((site.count("/a.js"), site.count("/b.js"), site.count("/c.js")), (1, 1, 1), "nothing twice");
}

#[test]
fn urgent_files_start_before_pictures_and_carry_their_priority() {
    let site = Site::new(30);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 1);
    pre.start(&hints(&[("/s.css", Kind::Style), ("/j.js", Kind::Script), ("/i1.png", Kind::Image)]));
    std::thread::sleep(Duration::from_millis(150));
    let log = site.log.lock().unwrap().clone();
    assert_eq!(log.iter().map(|(p, _, _)| p.as_str()).collect::<Vec<_>>(), ["/s.css", "/j.js", "/i1.png"]);
    assert_eq!(log.iter().map(|(_, _, p)| *p).collect::<Vec<_>>(), [net::BLOCKING, net::BLOCKING, net::IMAGE]);
}

#[test]
fn cookies_must_match_or_the_page_asks_again() {
    let site = Site::new(5);
    let jar = Arc::new(Mutex::new(CookieStore::new()));
    jar.lock().unwrap().store_from_header(&page(), "sid=1; Path=/", Preloader::now());
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), Some(jar.clone()), 2);
    pre.start(&hints(&[("/s.css", Kind::Style), ("/s.js", Kind::Script)]));
    std::thread::sleep(Duration::from_millis(40));
    // the sheet was fetched with the cookie, the script without (as the page's script loader does)
    let log = site.log.lock().unwrap().clone();
    assert!(log.iter().any(|(p, c, _)| p == "/s.css" && c == "sid=1"));
    assert!(log.iter().any(|(p, c, _)| p == "/s.js" && c.is_empty()));
    // the page asks for the sheet with the same cookie: served from the early answer
    let same = Request { cookie: "sid=1".into(), ..Request::get(&Url::parse("https://a.org/s.css").unwrap()) };
    pre.send(&same).unwrap();
    assert_eq!(site.count("/s.css"), 1);
    // a different cookie (changed meanwhile) is not served from it
    pre.start(&hints(&[("/t.css", Kind::Style)]));
    std::thread::sleep(Duration::from_millis(30));
    let other = Request { cookie: "sid=2".into(), ..Request::get(&Url::parse("https://a.org/t.css").unwrap()) };
    pre.send(&other).unwrap();
    assert_eq!(site.count("/t.css"), 2);
}

#[test]
fn hosts_the_account_may_not_open_are_never_asked() {
    let site = Site::new(1);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), Arc::new(|h| h == "a.org"), None, 2);
    let mut h = hints(&[("/ok.js", Kind::Script)]);
    h.push(Hint { url: Url::parse("https://tracker.example/t.js").unwrap(), kind: Kind::Script });
    pre.start(&h);
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(site.count("/ok.js"), 1);
    assert_eq!(site.count("/t.js"), 0);
}

#[test]
fn leaving_the_page_cancels_what_has_not_started_and_drops_what_arrives_late() {
    let site = Site::new(50);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 2);
    let many: Vec<(String, Kind)> = (0..20).map(|i| (format!("/n{i}.js"), Kind::Script)).collect();
    pre.start(&hints(&many.iter().map(|(p, k)| (p.as_str(), *k)).collect::<Vec<_>>()));
    std::thread::sleep(Duration::from_millis(20));
    pre.cancel();
    std::thread::sleep(Duration::from_millis(200));
    let started = site.log.lock().unwrap().len();
    assert!(started <= 4, "only what was already on the wire ran: {started}");
    assert_eq!(pre.pending(), (0, 0, 0), "nothing is kept");
    // a page that asks now just fetches for itself
    assert!(pre.get(&Url::parse("https://a.org/n19.js").unwrap()).is_ok());
    // and starting again after cancel does nothing
    pre.start(&hints(&[("/late.js", Kind::Script)]));
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(site.count("/late.js"), 0);
}

#[test]
fn dropping_the_preloader_stops_the_workers() {
    let site = Site::new(30);
    {
        let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 1);
        let many: Vec<(String, Kind)> = (0..10).map(|i| (format!("/d{i}.js"), Kind::Script)).collect();
        pre.start(&hints(&many.iter().map(|(p, k)| (p.as_str(), *k)).collect::<Vec<_>>()));
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(250));
    assert!(site.log.lock().unwrap().len() <= 2);
}

#[test]
fn a_failed_early_fetch_is_not_remembered_the_page_sees_the_real_answer() {
    let site = Site::new(5);
    site.fail.lock().unwrap().push("/bad.js".into());
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 2);
    pre.start(&hints(&[("/bad.js", Kind::Script)]));
    std::thread::sleep(Duration::from_millis(40));
    site.fail.lock().unwrap().clear();
    assert!(pre.get(&Url::parse("https://a.org/bad.js").unwrap()).is_ok(), "the page asked again and the server had recovered");
    assert_eq!(site.count("/bad.js"), 2);
}

#[test]
fn only_plain_gets_are_served_early() {
    let site = Site::new(1);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 2);
    pre.start(&hints(&[("/p.js", Kind::Script)]));
    std::thread::sleep(Duration::from_millis(30));
    let post = Request { method: "POST".into(), body: b"x=1".to_vec(), ..Request::get(&Url::parse("https://a.org/p.js").unwrap()) };
    // POST is refused by this fake's transport semantics only if it cares; the point is it bypasses the early answer
    let _ = pre.send(&post);
    assert_eq!(site.count("/p.js"), 2);
    // the early answer is still held and is used once; a second request goes out on its own
    pre.get(&Url::parse("https://a.org/p.js").unwrap()).unwrap();
    assert_eq!(site.count("/p.js"), 2);
    pre.get(&Url::parse("https://a.org/p.js").unwrap()).unwrap();
    assert_eq!(site.count("/p.js"), 3);
}

#[test]
fn no_deadlock_when_the_pools_are_exhausted() {
    // 1 connection overall, 1 worker, 40 early fetches, and a page that asks for them in reverse order from 4 threads
    let site = Site::new(1);
    let limited: Arc<dyn Fetch> = Arc::new(LimitedFetch::new(site.clone(), Arc::new(Limiter::new(1, 1))));
    let pre = Preloader::new(limited, &page(), allow_all(), None, 1);
    let list: Vec<(String, Kind)> = (0..40).map(|i| (format!("/e{i}.js"), Kind::Script)).collect();
    pre.start(&hints(&list.iter().map(|(p, k)| (p.as_str(), *k)).collect::<Vec<_>>()));
    let t = Instant::now();
    let hs: Vec<_> = (0..4)
        .map(|th| {
            let pre = pre.clone();
            std::thread::spawn(move || {
                for i in (0..40).rev().filter(|i| i % 4 == th) {
                    pre.get(&Url::parse(&format!("https://a.org/e{i}.js")).unwrap()).unwrap();
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert!(t.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_early_answers_held_are_capped() {
    struct Big;
    impl Fetch for Big {
        fn get(&self, _: &Url) -> Result<Response, String> {
            Ok(Response { status: 200, body: vec![0; 20 << 20], ..Response::default() })
        }
    }
    let pre = Preloader::new(Arc::new(Big), &page(), allow_all(), None, 4);
    let list: Vec<(String, Kind)> = (0..8).map(|i| (format!("/big{i}.js"), Kind::Script)).collect();
    pre.start(&hints(&list.iter().map(|(p, k)| (p.as_str(), *k)).collect::<Vec<_>>()));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(pre.pending().2 <= MAX_HELD, "held {}", pre.pending().2);
}

#[test]
fn finishing_the_page_lets_go_of_unused_scripts_but_keeps_pictures_for_later() {
    let site = Site::new(2);
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 4);
    pre.start(&hints(&[("/u.js", Kind::Script), ("/u.css", Kind::Style), ("/p.png", Kind::Image)]));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(pre.pending().0, 3);
    pre.release_unused();
    assert_eq!(pre.pending().0, 1);
    pre.get(&Url::parse("https://a.org/p.png").unwrap()).unwrap();
    assert_eq!(site.count("/p.png"), 1, "the picture was still there");
    assert_eq!(pre.pending(), (0, 0, 0));
}

#[test]
fn a_picture_that_failed_early_is_reported_once_without_asking_the_server_again() {
    let site = Site::new(2);
    site.fail.lock().unwrap().push("/gone.png".into());
    let pre = Preloader::new(Arc::new(site.clone()), &page(), allow_all(), None, 2);
    pre.start(&hints(&[("/gone.png", Kind::Image)]));
    std::thread::sleep(Duration::from_millis(40));
    assert!(pre.get(&Url::parse("https://a.org/gone.png").unwrap()).is_err());
    assert_eq!(site.count("/gone.png"), 1);
}
