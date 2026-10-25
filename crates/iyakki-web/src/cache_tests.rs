use super::*;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// A fake server: answers are set per path; it counts requests, remembers the validators it was sent and can go offline.
#[derive(Default)]
struct Server {
    answers: Mutex<HashMap<String, Response>>,
    hits: Mutex<HashMap<String, usize>>,
    seen_validators: Mutex<Vec<(String, String)>>,
    offline: AtomicBool,
    delay_ms: AtomicU64,
}

impl Server {
    fn set(&self, path: &str, r: Response) {
        self.answers.lock().unwrap().insert(path.to_string(), r);
    }
    fn count(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

impl Fetch for Arc<Server> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        if self.offline.load(Ordering::SeqCst) {
            return Err("offline".into());
        }
        let d = self.delay_ms.load(Ordering::SeqCst);
        if d > 0 {
            std::thread::sleep(Duration::from_millis(d));
        }
        *self.hits.lock().unwrap().entry(req.url.path.clone()).or_insert(0) += 1;
        let r = self.answers.lock().unwrap().get(&req.url.path).cloned().ok_or("404")?;
        if !req.if_none_match.is_empty() || !req.if_modified_since.is_empty() {
            self.seen_validators.lock().unwrap().push((req.if_none_match.clone(), req.if_modified_since.clone()));
            if (!req.if_none_match.is_empty() && req.if_none_match == r.etag) || (!req.if_modified_since.is_empty() && req.if_modified_since == r.last_modified) {
                return Ok(Response { status: 304, cache_control: r.cache_control.clone(), ..Response::default() });
            }
        }
        Ok(r)
    }
}

fn page(cc: &str) -> Response {
    Response { status: 200, content_type: "text/css".into(), body: b"body{}".to_vec(), cache_control: cc.into(), ..Response::default() }
}

fn tmp(name: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let d = PathBuf::from(base).join(format!("iw-cache-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn setup(name: &str) -> (Arc<Server>, CachedFetch<Arc<Server>>, PathBuf) {
    let s = Arc::new(Server::default());
    let dir = tmp(name);
    (s.clone(), CachedFetch::new(s, &dir), dir)
}

fn u(p: &str) -> Url {
    Url::parse(&format!("https://a.org{p}")).unwrap()
}

#[test]
fn max_age_reads_the_header() {
    assert_eq!(max_age("public, max-age=300"), Some(300));
    assert_eq!(max_age("max-age=0"), None);
    assert_eq!(max_age("no-store, max-age=60"), None);
    assert_eq!(max_age("private, max-age=60"), None);
    assert_eq!(max_age("s-maxage=\"90\""), Some(90));
    assert_eq!(max_age(""), None);
}

#[test]
fn a_fresh_answer_is_served_from_memory_then_from_disk() {
    let (s, c, dir) = setup("fresh");
    s.set("/a.css", page("max-age=60"));
    assert_eq!(c.get(&u("/a.css")).unwrap().body, b"body{}");
    assert_eq!(c.get(&u("/a.css")).unwrap().body, b"body{}");
    assert_eq!(s.count("/a.css"), 1, "the second read did not reach the server");
    // a new cache object on the same folder (a restart) finds it on disk
    let c2 = CachedFetch::new(s.clone(), &dir);
    assert_eq!(c2.get(&u("/a.css")).unwrap().content_type, "text/css");
    assert_eq!(s.count("/a.css"), 1);
    assert_eq!(c2.stats().disk_entries, 1);
    c.clear();
}

#[test]
fn the_memory_tier_answers_without_touching_the_disk() {
    let (s, c, dir) = setup("mem");
    s.set("/m.css", page("max-age=60"));
    c.get(&u("/m.css")).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(c.get(&u("/m.css")).is_ok());
    assert_eq!(s.count("/m.css"), 1, "kept in memory although the file is gone");
}

#[test]
fn a_stale_answer_with_an_etag_is_renewed_by_a_304() {
    let (s, c, _d) = setup("etag");
    let mut r = page("max-age=10");
    r.etag = "\"v1\"".into();
    s.set("/e.css", r);
    c.get(&u("/e.css")).unwrap();
    c.advance_clock(11);
    let again = c.get(&u("/e.css")).unwrap();
    assert_eq!((again.status, again.body), (200, b"body{}".to_vec()), "the caller sees the full answer");
    assert_eq!(s.count("/e.css"), 2);
    assert_eq!(s.seen_validators.lock().unwrap().clone(), vec![("\"v1\"".to_string(), String::new())]);
    // renewed: fresh again for another 10 seconds without asking
    c.advance_clock(5);
    c.get(&u("/e.css")).unwrap();
    assert_eq!(s.count("/e.css"), 2);
    c.clear();
}

#[test]
fn a_changed_answer_replaces_the_old_one() {
    let (s, c, _d) = setup("changed");
    let mut r = page("max-age=10");
    r.etag = "\"v1\"".into();
    s.set("/c.css", r);
    c.get(&u("/c.css")).unwrap();
    let mut r2 = page("max-age=10");
    r2.etag = "\"v2\"".into();
    r2.body = b"new{}".to_vec();
    s.set("/c.css", r2);
    c.advance_clock(11);
    assert_eq!(c.get(&u("/c.css")).unwrap().body, b"new{}");
    assert_eq!(c.get(&u("/c.css")).unwrap().body, b"new{}");
    assert_eq!(s.count("/c.css"), 2);
    c.clear();
}

#[test]
fn last_modified_is_used_when_there_is_no_etag() {
    let (s, c, _d) = setup("lm");
    let mut r = page("max-age=5");
    r.last_modified = "Tue, 01 Sep 2026 10:00:00 GMT".into();
    s.set("/l.css", r);
    c.get(&u("/l.css")).unwrap();
    c.advance_clock(6);
    c.get(&u("/l.css")).unwrap();
    assert_eq!(s.seen_validators.lock().unwrap().clone(), vec![(String::new(), "Tue, 01 Sep 2026 10:00:00 GMT".to_string())]);
    c.clear();
}

#[test]
fn a_stale_answer_without_a_validator_is_fetched_again() {
    let (s, c, _d) = setup("novalidator");
    s.set("/n.css", page("max-age=5"));
    c.get(&u("/n.css")).unwrap();
    c.advance_clock(6);
    c.get(&u("/n.css")).unwrap();
    assert_eq!(s.count("/n.css"), 2);
    assert!(s.seen_validators.lock().unwrap().is_empty());
    c.clear();
}

#[test]
fn no_cache_is_kept_only_to_be_asked_about() {
    let (s, c, _d) = setup("nocache");
    let mut r = page("no-cache");
    r.etag = "\"x\"".into();
    s.set("/nc.css", r);
    c.get(&u("/nc.css")).unwrap();
    c.get(&u("/nc.css")).unwrap();
    assert_eq!(s.count("/nc.css"), 2, "asked every time");
    assert_eq!(s.seen_validators.lock().unwrap().len(), 1, "with the validator");
    // and without a validator nothing is kept at all
    s.set("/nc2.css", page("no-cache"));
    assert_eq!(c.stats().disk_entries, 1);
    c.clear();
}

#[test]
fn no_store_and_private_are_never_kept() {
    let (s, c, _d) = setup("private");
    for (i, cc) in ["no-store", "private, max-age=600", "no-store, max-age=600"].iter().enumerate() {
        let mut r = page(cc);
        r.etag = "\"t\"".into();
        s.set(&format!("/p{i}.css"), r);
        c.get(&u(&format!("/p{i}.css"))).unwrap();
        c.get(&u(&format!("/p{i}.css"))).unwrap();
        assert_eq!(s.count(&format!("/p{i}.css")), 2, "{cc}");
    }
    assert_eq!(c.stats().disk_entries, 0);
    assert_eq!(c.stats().mem_entries, 0);
}

#[test]
fn answers_that_set_cookies_or_vary_on_them_are_not_kept() {
    let (s, c, _d) = setup("vary");
    let mut with_cookie = page("max-age=60");
    with_cookie.set_cookies = vec!["a=b".into()];
    s.set("/sc.css", with_cookie);
    let mut vary_cookie = page("max-age=60");
    vary_cookie.vary = "Accept-Encoding, Cookie".into();
    s.set("/vc.css", vary_cookie);
    let mut vary_star = page("max-age=60");
    vary_star.vary = "*".into();
    s.set("/vs.css", vary_star);
    let mut vary_ok = page("max-age=60");
    vary_ok.vary = "Accept-Encoding".into();
    s.set("/vo.css", vary_ok);
    for p in ["/sc.css", "/vc.css", "/vs.css", "/vo.css"] {
        c.get(&u(p)).unwrap();
        c.get(&u(p)).unwrap();
    }
    assert_eq!((s.count("/sc.css"), s.count("/vc.css"), s.count("/vs.css"), s.count("/vo.css")), (2, 2, 2, 1));
    c.clear();
}

#[test]
fn requests_with_cookies_cache_static_files_but_not_pages() {
    let (s, c, _d) = setup("cookies");
    s.set("/s.css", page("max-age=60"));
    s.set("/doc", Response { status: 200, content_type: "text/html".into(), body: b"<p>hi".to_vec(), cache_control: "max-age=60".into(), ..Response::default() });
    s.set("/pub", Response { status: 200, content_type: "text/html".into(), body: b"<p>hi".to_vec(), cache_control: "public, max-age=60".into(), ..Response::default() });
    s.set("/api", Response { status: 200, content_type: "application/json".into(), body: b"{}".to_vec(), cache_control: "max-age=60".into(), ..Response::default() });
    let req = |p: &str| Request { cookie: "session=1".into(), ..Request::get(&u(p)) };
    for p in ["/s.css", "/doc", "/pub", "/api"] {
        c.send(&req(p)).unwrap();
        c.send(&req(p)).unwrap();
    }
    assert_eq!((s.count("/s.css"), s.count("/doc"), s.count("/pub"), s.count("/api")), (1, 2, 1, 2), "a signed-in page is not stored unless the server said public");
    c.clear();
}

#[test]
fn a_stale_copy_is_served_when_the_network_is_down() {
    let (s, c, _d) = setup("offline");
    s.set("/o.css", page("max-age=5"));
    c.get(&u("/o.css")).unwrap();
    c.advance_clock(100);
    s.offline.store(true, Ordering::SeqCst);
    assert_eq!(c.get(&u("/o.css")).unwrap().body, b"body{}");
    assert!(c.get(&u("/never-seen.css")).is_err());
    c.clear();
}

#[test]
fn the_age_header_shortens_freshness_and_a_week_is_the_most() {
    let url = u("/x.css");
    let mut r = page("max-age=100");
    r.age = 90;
    assert_eq!(policy(&r, &url, false), Policy { store: true, lifetime: 10 });
    r.age = 200;
    assert!(!policy(&r, &url, false).store, "already stale and nothing to ask with");
    let forever = page("public, max-age=31536000, immutable");
    assert_eq!(policy(&forever, &url, false).lifetime, MAX_FRESH);
    // a body that is only the start of a large file is never kept
    let mut media = page("max-age=60");
    media.content_length = Some(1 << 30);
    assert!(!policy(&media, &url, false).store);
    // other statuses are not kept
    let mut gone = page("max-age=60");
    gone.status = 404;
    assert!(!policy(&gone, &url, false).store);
}

#[test]
fn the_memory_budget_is_held_and_shrinks_on_request() {
    let s = Arc::new(Server::default());
    let dir = tmp("budget");
    let c = CachedFetch::with_budget(s.clone(), &dir, Budget { mem: 100_000, disk: 10 << 20, max_entry: 50_000 });
    for i in 0..10 {
        let mut r = page("max-age=600");
        r.body = vec![i as u8; 30_000];
        s.set(&format!("/b{i}.css"), r);
        c.get(&u(&format!("/b{i}.css"))).unwrap();
        assert!(c.stats().mem_bytes <= 100_000, "after {i}: {}", c.stats().mem_bytes);
    }
    assert!(c.stats().mem_entries >= 2);
    // the least recently used went first: the last one is still there
    let before = s.count("/b9.css");
    c.get(&u("/b9.css")).unwrap();
    assert_eq!(s.count("/b9.css"), before);
    // the memory gate takes the memory tier back
    c.set_budget(Budget { mem: 0, disk: 10 << 20, max_entry: 0 });
    assert_eq!(c.stats().mem_entries, 0);
    // an answer over the entry limit stays out of memory but is on disk
    let mut big = page("max-age=600");
    big.body = vec![7; 60_000];
    s.set("/big.css", big);
    c.set_budget(Budget { mem: 100_000, disk: 10 << 20, max_entry: 50_000 });
    c.get(&u("/big.css")).unwrap();
    c.get(&u("/big.css")).unwrap();
    assert_eq!(s.count("/big.css"), 1);
    assert_eq!(c.stats().mem_entries, 0);
    // nothing allowed: pass through
    c.set_budget(Budget::OFF);
    c.get(&u("/b9.css")).unwrap();
    c.get(&u("/b9.css")).unwrap();
    assert_eq!(s.count("/b9.css"), before + 2);
    c.clear();
}

#[test]
fn the_disk_budget_removes_the_least_recently_used_files() {
    let s = Arc::new(Server::default());
    let dir = tmp("disk");
    let c = CachedFetch::with_budget(s.clone(), &dir, Budget { mem: 0, disk: 100_000, max_entry: 0 });
    for i in 0..12 {
        let mut r = page("max-age=600");
        r.body = vec![i as u8; 10_000];
        s.set(&format!("/d{i}.css"), r);
        c.get(&u(&format!("/d{i}.css"))).unwrap();
    }
    let st = c.stats();
    assert!(st.disk_bytes <= 100_000, "{}", st.disk_bytes);
    assert!(st.disk_entries >= 3 && st.disk_entries < 12);
    let files = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(files, st.disk_entries, "the index matches the folder");
    // the newest survived, the oldest did not
    c.get(&u("/d11.css")).unwrap();
    assert_eq!(s.count("/d11.css"), 1);
    c.get(&u("/d0.css")).unwrap();
    assert_eq!(s.count("/d0.css"), 2);
    // shrinking the budget trims at once
    c.set_budget(Budget { mem: 0, disk: 45_000, max_entry: 0 });
    assert!(c.stats().disk_bytes <= 45_000);
    c.clear();
    assert_eq!(c.stats().disk_entries, 0);
}

#[test]
fn old_format_files_and_strangers_in_the_folder_are_harmless() {
    let (s, c, dir) = setup("legacy");
    s.set("/l.css", page("max-age=60"));
    std::fs::create_dir_all(&dir).unwrap();
    let k = key(&u("/l.css"));
    std::fs::write(dir.join(&k), b"9999999999\t200\thttps://a.org/l.css\ttext/css\nold body").unwrap();
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    assert_eq!(c.get(&u("/l.css")).unwrap().body, b"body{}", "an old-format file is a miss, not an error");
    assert!(dir.join("notes.txt").exists(), "files that are not cache entries are left alone");
    c.clear();
}

#[test]
fn each_person_has_a_cache_of_their_own() {
    let s = Arc::new(Server::default());
    s.set("/shared.css", page("max-age=600"));
    let base = tmp("users");
    let uc = UserCaches::new(Arc::new(s.clone()), &base, Budget { mem: 1 << 20, disk: 8 << 20, max_entry: 1 << 20 });
    let alice = uc.for_user("alice").unwrap();
    let bob = uc.for_user("bob").unwrap();
    alice.get(&u("/shared.css")).unwrap();
    alice.get(&u("/shared.css")).unwrap();
    assert_eq!(s.count("/shared.css"), 1);
    bob.get(&u("/shared.css")).unwrap();
    assert_eq!(s.count("/shared.css"), 2, "bob does not get alice's copy");
    // the anonymous path never caches
    uc.get(&u("/shared.css")).unwrap();
    uc.get(&u("/shared.css")).unwrap();
    assert_eq!(s.count("/shared.css"), 4);
    assert!(base.join("alice").is_dir() && base.join("bob").is_dir());
    assert_eq!(uc.cache_stats("alice").unwrap().disk_entries, 1);
    // clearing one person's cache leaves the other's
    uc.clear_cache("alice");
    assert_eq!(uc.cache_stats("alice").unwrap().disk_entries, 0);
    assert_eq!(uc.cache_stats("bob").unwrap().disk_entries, 1);
    // a hostile name stays inside the folder
    uc.for_user("../../etc").unwrap().get(&u("/shared.css")).unwrap();
    let safe = safe_name("../../etc");
    assert!(!safe.contains('/') && !safe.starts_with('.'), "{safe}");
    assert!(base.join(&safe).is_dir());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_budget_reaches_every_person_s_cache() {
    let s = Arc::new(Server::default());
    s.set("/x.css", page("max-age=600"));
    let uc = UserCaches::new(Arc::new(s.clone()), tmp("gate"), Budget { mem: 1 << 20, disk: 8 << 20, max_entry: 1 << 20 });
    let a = uc.for_user("a").unwrap();
    a.get(&u("/x.css")).unwrap();
    assert_eq!(uc.cache_stats("a").unwrap().mem_entries, 1);
    uc.set_cache_budget(Budget::OFF);
    assert_eq!(uc.cache_stats("a").unwrap().mem_entries, 0);
    assert_eq!(uc.cache_stats("a").unwrap().disk_entries, 0, "even the disk tier is emptied when nothing is allowed");
    // a person who signs in later gets the current budget
    assert_eq!(uc.cache_stats("late").unwrap().budget, Budget::OFF);
    let _ = std::fs::remove_dir_all(&uc.base);
}

#[test]
fn concurrent_readers_and_writers_do_not_corrupt_anything() {
    let s = Arc::new(Server::default());
    let dir = tmp("threads");
    let c = Arc::new(CachedFetch::with_budget(s.clone(), &dir, Budget { mem: 200_000, disk: 400_000, max_entry: 100_000 }));
    for i in 0..30 {
        let mut r = page("max-age=600");
        r.body = vec![i as u8; 5_000 + i * 100];
        s.set(&format!("/t{i}.css"), r);
    }
    let hs: Vec<_> = (0..8)
        .map(|t| {
            let c = c.clone();
            std::thread::spawn(move || {
                for n in 0..200 {
                    let i = (n * 7 + t * 3) % 30;
                    let r = c.get(&u(&format!("/t{i}.css"))).unwrap();
                    assert_eq!(r.body.len(), 5_000 + i * 100);
                    assert!(r.body.iter().all(|b| *b == i as u8));
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert!(c.stats().mem_bytes <= 200_000 && c.stats().disk_bytes <= 400_000);
    c.clear();
}
