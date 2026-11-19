//! Page-load timeline of the reader browser's own stack (network, parse, scripts, style, pictures), for one address.
//!
//!   cargo run --release -p iyakki-js --example browser_timing -- https://en.wikipedia.org/wiki/Rust_(programming_language)
//!   cargo run --release -p iyakki-js --example browser_timing -- --js --runs 5 https://news.ycombinator.com/
//!
//! Options: `--js` run the page's scripts (the browser does this only for sites the user allows), `--runs N` repeat (a
//! summary line with the median is printed), `--no-images`, `--cache DIR` (use the disk cache, default none: every run is
//! cold), `--warm` (keep the cache between runs), `--preload` (the preload scanner on, when built in), `--quiet`.
//! The page's own pictures are fetched with the same number of workers as the browser uses. Layout and painting happen in the
//! shell and are not part of this example (see docs/BROWSER-SPEED.md).
//! What is NOT measured here: DNS/TCP/TLS inside the HTTP client (it does not expose them): they are measured separately for
//! the document's host with the system resolver and a plain TCP connect, and "cold minus warm" request time estimates the
//! TLS handshake.

use std::collections::VecDeque;
use std::io::Write as _;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_js::{renderer_with, SiteData};
use iyakki_proxy::https::BrowserFetch;
use iyakki_web::cache::{ArcFetch, CachedFetch};
use iyakki_web::net::{LimitedFetch, Limiter, GLOBAL, PER_HOST};
use iyakki_web::netstats::{max_per_host, Entry, TimedFetch};
use iyakki_web::preload::{self, Preloader, Wants};
use iyakki_web::{Fetch, Loaded, Request, Url};

struct Opts {
    url: String,
    js: bool,
    runs: usize,
    images: bool,
    cache: Option<String>,
    warm: bool,
    preload: bool,
    quiet: bool,
    /// Milliseconds added to every request (simulates a slower link; the real network time comes on top).
    delay_ms: u64,
    /// Connections per host and overall (0: the browser's own limits).
    per_host: usize,
    /// Filter list files (comma separated): requests they name are blocked before they are sent, as in the browser.
    block: Vec<String>,
}

fn parse_args() -> Opts {
    let mut o = Opts { url: String::new(), js: false, runs: 1, images: true, cache: None, warm: false, preload: false, quiet: false, delay_ms: 0, per_host: 0, block: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--js" => o.js = true,
            "--runs" => o.runs = it.next().and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 50),
            "--no-images" => o.images = false,
            "--cache" => o.cache = it.next(),
            "--warm" => o.warm = true,
            "--preload" => o.preload = true,
            "--quiet" => o.quiet = true,
            "--delay-ms" => o.delay_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--block" => o.block = it.next().map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default(),
            "--per-host" => o.per_host = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            _ if a.starts_with("--") => {
                eprintln!("unknown option {a}");
                std::process::exit(2);
            }
            _ => o.url = a,
        }
    }
    if o.url.is_empty() {
        eprintln!("usage: browser_timing [--js] [--runs N] [--no-images] [--cache DIR] [--warm] [--preload] [--delay-ms N] [--per-host N] [--quiet] URL");
        std::process::exit(2);
    }
    o
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Default, Clone, Debug)]
struct Run {
    dns: f64,
    tcp: f64,
    doc_first: f64,
    doc_warm: f64,
    doc_bytes: usize,
    load_total: f64,
    parse: f64,
    js_total: f64,
    js_script_net: f64,
    style_total: f64,
    style_net: f64,
    usable: f64,
    images_done: f64,
    images_n: usize,
    images_bytes: usize,
    decode: f64,
    requests: usize,
    sub_net_wait_max: f64,
    max_conc_all: usize,
    max_conc_host: usize,
    cache_hits: u64,
    cache_misses: u64,
    preload_used: u64,
    preload_wasted: u64,
}

fn main() {
    let o = parse_args();
    let url = match Url::parse(&o.url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("bad address: {e}");
            std::process::exit(2);
        }
    };
    let mut runs = Vec::new();
    for n in 0..o.runs {
        if !o.quiet {
            println!("=== run {} of {}: {} {}", n + 1, o.runs, o.url, if o.js { "(scripts on)" } else { "(scripts off)" });
        }
        if let Some(dir) = &o.cache {
            if !o.warm || n == 0 {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        runs.push(one_run(&o, &url));
    }
    summary(&o, &runs);
}

fn one_run(o: &Opts, url: &Url) -> Run {
    let mut r = Run::default();
    let counters0 = iyakki_web::net::snapshot();
    // DNS and TCP for the document's host, with the system resolver (what the HTTP client uses too)
    let port = url.port.unwrap_or(if url.https { 443 } else { 80 });
    let t = Instant::now();
    let addrs: Vec<_> = (url.host.as_str(), port).to_socket_addrs().map(|i| i.collect()).unwrap_or_default();
    r.dns = ms(t.elapsed());
    if let Some(a) = addrs.first() {
        let t = Instant::now();
        let _ = std::net::TcpStream::connect_timeout(a, Duration::from_secs(10));
        r.tcp = ms(t.elapsed());
    }

    // the stack as the browser builds it: preload store > cache > connection limits > (timing) > (simulated delay) > HTTP client
    let base: Arc<dyn Fetch> = Arc::new(Delay(BrowserFetch::default(), o.delay_ms));
    let mut blocker_filter: Option<Arc<iyakki_web::blocker_state::Filter>> = None;
    let base: Arc<dyn Fetch> = if o.block.is_empty() {
        base
    } else {
        // the blocker as the browser has it: outermost, so a blocked request is never sent
        let texts: Vec<String> = o.block.iter().map(|f| std::fs::read_to_string(f).unwrap_or_default()).collect();
        let t = Instant::now();
        let b = iyakki_web::blocker::compile_lists(texts.iter().map(String::as_str));
        if !o.quiet {
            let st = b.stats();
            println!("blocker: {} blocking + {} exception + {} hiding rules ({} lines skipped, {} dropped), compiled in {:.0} ms, about {} MB", st.blocking, st.exceptions, st.cosmetic, st.skipped, st.dropped, ms(t.elapsed()), b.memory_estimate() >> 20);
        }
        let filter = iyakki_web::blocker_state::Filter::new();
        filter.set_blocker(b);
        filter.new_page(&url.host);
        blocker_filter = Some(filter.clone());
        Arc::new(iyakki_web::blocker_state::FilteredFetch::new(base, filter))
    };
    let timed = Arc::new(TimedFetch::new(ArcFetch(base)));
    let limiter = Arc::new(if o.per_host > 0 { Limiter::new(o.per_host, 32) } else { Limiter::new(PER_HOST, GLOBAL) });
    let limited: Arc<dyn Fetch> = Arc::new(LimitedFetch::new(ArcFetch(timed.clone()), limiter));
    let cache: Option<Arc<CachedFetch<ArcFetch>>> = o.cache.as_ref().map(|d| {
        let mem = iyakki_web::net::budget(iyakki_web::net::CacheSize::Auto, 8 << 30, 100 << 30);
        Arc::new(CachedFetch::with_budget(ArcFetch(limited.clone()), d, mem))
    });
    let below: Arc<dyn Fetch> = match &cache {
        Some(c) => c.clone(),
        None => limited.clone(),
    };
    let cookies_arc = Arc::new(Mutex::new(iyakki_web::CookieStore::new()));
    let pre = o.preload.then(|| Preloader::new(below.clone(), url, Arc::new(|_: &str| true), Some(cookies_arc.clone()), 8));
    let web: Arc<dyn Fetch> = match &pre {
        Some(p) => p.clone(),
        None => below.clone(),
    };

    // the document alone, twice: the second request finds the connection warm, so cold minus warm is connect + TLS
    let t = Instant::now();
    let first = web.get(url);
    r.doc_first = ms(t.elapsed());
    if let Ok(f) = &first {
        r.doc_bytes = f.body.len();
    }
    let t = Instant::now();
    let _ = below.get(url);
    r.doc_warm = ms(t.elapsed());

    // the page, as the browser loads it (this is the part the user waits for)
    let t_all = Instant::now();
    let allow = |_: &str| true;
    let vp = iyakki_web::css::Viewport::default();
    let cookies = &*cookies_arc;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let parse_t = Mutex::new(Duration::ZERO);
    let style_t = Mutex::new((Duration::ZERO, 0.0f64));
    let js_t = Mutex::new(Duration::ZERO);
    let web2 = web.clone();
    let timed2 = timed.clone();
    let sheet = |u: &Url| -> Option<String> {
        let (_, resp) = iyakki_web::fetch_following(web2.as_ref(), Request::get(u), &allow, Some(iyakki_web::Jar { store: cookies, now })).ok()?;
        (resp.status < 400 && resp.body.len() <= iyakki_web::style::MAX_SHEET_BYTES).then(|| String::from_utf8_lossy(&resp.body).into_owned())
    };
    let build = |u: &Url, tree: &iyakki_web::html::Tree| -> iyakki_web::Doc {
        let before: f64 = timed2.entries().iter().filter(|e| e.kind() == "css").map(|e| e.ms()).sum();
        let t = Instant::now();
        let d = iyakki_web::style::styled_doc(tree, u, &vp, &sheet);
        let after: f64 = timed2.entries().iter().filter(|e| e.kind() == "css").map(|e| e.ms()).sum();
        if let Ok(mut s) = style_t.lock() {
            s.0 += t.elapsed();
            s.1 += after - before;
        }
        d
    };
    let render = |u: &Url, html: &str| -> (iyakki_web::Doc, Vec<String>) {
        if let Some(p) = &pre {
            p.start(&preload::scan(html, u, Wants { styles: true, scripts: o.js, images: o.images }));
        }
        if o.js {
            let t = Instant::now();
            let data = SiteData::default();
            let out = renderer_with(web.clone(), allow, data, &build)(u, html);
            if let Ok(mut j) = js_t.lock() {
                *j += t.elapsed();
            }
            out
        } else {
            let t = Instant::now();
            let tree = iyakki_web::html::parse(html);
            if let Ok(mut p) = parse_t.lock() {
                *p += t.elapsed();
            }
            (build(u, &tree), Vec::new())
        }
    };
    let loaded = iyakki_web::load_request(web.as_ref(), Request::get(url), &allow, &render, Some(iyakki_web::Jar { store: cookies, now }));
    r.load_total = ms(t_all.elapsed());
    r.usable = r.load_total;
    r.parse = ms(*parse_t.lock().unwrap());
    r.js_total = ms(*js_t.lock().unwrap());
    {
        let s = style_t.lock().unwrap();
        r.style_total = ms(s.0);
        r.style_net = s.1;
    }
    r.js_script_net = timed.entries().iter().filter(|e| e.kind() == "js").map(|e| e.ms()).sum();

    let doc = match loaded {
        Ok(Loaded::Page { doc, status, notes, .. }) => {
            if !o.quiet {
                println!("page: status {status}, {} blocks, {} images, {} links, {} notes", doc.blocks.len(), doc.images.len(), doc.links.len(), notes.len());
                for n in notes.iter().take(3) {
                    println!("  note: {}", n.chars().take(150).collect::<String>());
                }
            }
            Some(doc)
        }
        Ok(Loaded::Unsupported { content_type, .. }) => {
            println!("not a page: {content_type}");
            None
        }
        Err(e) => {
            println!("load failed: {e}");
            None
        }
    };

    // pictures: the browser starts them after the page is shown, 4 workers
    if let (true, Some(doc)) = (o.images, &doc) {
        let t_img = Instant::now();
        let jobs: VecDeque<String> = doc.images.iter().take(60).map(|i| i.src.clone()).filter(|s| !s.starts_with("data:")).collect();
        r.images_n = jobs.len();
        let queue = Mutex::new(jobs);
        let bytes = Mutex::new((0usize, Duration::ZERO));
        std::thread::scope(|sc| {
            for _ in 0..4 {
                sc.spawn(|| loop {
                    let Some(src) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { return };
                    let Ok(u) = url.join(&src) else { continue };
                    let mut req = Request::get(&u);
                    req.referer = url.to_string();
                    let Ok((_, resp)) = iyakki_web::fetch_following(web.as_ref(), req, &allow, Some(iyakki_web::Jar { store: cookies, now })) else { continue };
                    let t = Instant::now();
                    let _ = iyakki_web::image::decode(&resp.body); // PNG is decoded by the shell's renderer, not measured here
                    if let Ok(mut b) = bytes.lock() {
                        b.0 += resp.body.len();
                        b.1 += t.elapsed();
                    }
                });
            }
        });
        let b = bytes.lock().unwrap();
        r.images_bytes = b.0;
        r.decode = ms(b.1);
        r.images_done = ms(t_img.elapsed());
    }

    let c1 = iyakki_web::net::snapshot();
    r.cache_hits = c1.mem_hits + c1.disk_hits - counters0.mem_hits - counters0.disk_hits + c1.revalidated - counters0.revalidated;
    r.cache_misses = c1.misses - counters0.misses;
    r.preload_used = c1.preload_used - counters0.preload_used;
    r.preload_wasted = c1.preload_wasted - counters0.preload_wasted;
    let entries = timed.entries();
    r.requests = entries.len();
    r.max_conc_all = timed.max_concurrency();
    r.max_conc_host = max_per_host(&entries).first().map(|x| x.1).unwrap_or(0);
    // how long the page waited for its slowest subresource after the document came
    r.sub_net_wait_max = entries.iter().skip(1).map(|e| e.ms()).fold(0.0, f64::max);
    if !o.quiet {
        timeline(&entries, &r, o);
    }
    if let Some(f) = &blocker_filter {
        let (blocked, checked) = f.totals();
        println!("BLOCKER blocked={blocked} of {checked} requests checked (the request count above still includes the blocked ones: the timing list sits outside the blocker)");
    }
    r
}

/// Adds a fixed wait to every request (a slower link).
struct Delay(BrowserFetch, u64);
impl Fetch for Delay {
    fn get(&self, url: &Url) -> Result<iyakki_web::Response, String> {
        self.send(&Request::get(url))
    }
    fn send(&self, req: &Request) -> Result<iyakki_web::Response, String> {
        if self.1 > 0 {
            std::thread::sleep(Duration::from_millis(self.1));
        }
        self.0.send(req)
    }
}

fn timeline(entries: &[Entry], r: &Run, o: &Opts) {
    let mut es: Vec<&Entry> = entries.iter().collect();
    es.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
    println!("\n-- request timeline (ms since start; document pair at the top is the cold/warm probe) --");
    for e in es.iter().take(if o.quiet { 0 } else { 70 }) {
        let u: String = e.url.chars().take(88).collect();
        println!("{:8.0} {:8.0} {:7.0} ms {:>8} B c{:<2} {:5} {:3} {}", e.start_ms, e.end_ms, e.ms(), e.bytes, e.host_concurrency, e.kind(), e.status, u);
    }
    if es.len() > 70 {
        println!("... {} more", es.len() - 70);
    }
    let mut by: std::collections::BTreeMap<&str, (usize, usize, f64)> = Default::default();
    for e in entries {
        let v = by.entry(e.kind()).or_default();
        v.0 += 1;
        v.1 += e.bytes;
        v.2 += e.ms();
    }
    println!("\n-- subresources by type (count, bytes, summed request time) --");
    for (k, (n, b, t)) in &by {
        println!("{k:6} {n:4}  {:8.1} KB  {t:8.0} ms", *b as f64 / 1024.0);
    }
    println!("\n-- per host: most requests in flight at once --");
    for (h, n) in max_per_host(entries).iter().take(8) {
        println!("{n:3}  {h}");
    }
    println!("\n-- phases (ms) --");
    println!("DNS (system resolver)              {:8.1}", r.dns);
    println!("TCP connect (first address)        {:8.1}", r.tcp);
    println!("document cold (DNS+TCP+TLS+TTFB+body) {:5.1}   warm connection {:8.1}   => connect+TLS about {:.1}", r.doc_first, r.doc_warm, (r.doc_first - r.doc_warm).max(0.0));
    println!("page load until usable             {:8.1}", r.usable);
    println!("  html parse (scripts off)         {:8.1}", r.parse);
    println!("  script phase total (js on)       {:8.1}   of which waiting for script files {:.1}", r.js_total, r.js_script_net);
    println!("  style (sheets+cascade+doc)       {:8.1}   of which waiting for sheets {:.1}", r.style_total, r.style_net);
    println!("pictures: {} fetched, {:.1} KB, done after {:.1}, decode CPU {:.1}", r.images_n, r.images_bytes as f64 / 1024.0, r.images_done, r.decode);
    println!("requests on the wire {} ; most in flight at once {} ; slowest subresource {:.0} ms", r.requests, r.max_conc_all, r.sub_net_wait_max);
    println!("cache: {} hits (memory, disk, revalidated), {} misses ; preload: {} used, {} wasted", r.cache_hits, r.cache_misses, r.preload_used, r.preload_wasted);
    let _ = std::io::stdout().flush();
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else {
        v[v.len() / 2]
    }
}

fn summary(o: &Opts, runs: &[Run]) {
    let med = |f: &dyn Fn(&Run) -> f64| median(runs.iter().map(f).collect());
    println!(
        "TIMING url={} js={} preload={} runs={} usable_ms={:.0} (min {:.0} max {:.0}) doc_cold_ms={:.0} doc_warm_ms={:.0} dns_ms={:.1} tcp_ms={:.1} style_ms={:.0} js_ms={:.0} images_ms={:.0} requests={} max_conc={} max_conc_host={} cache_hits={} preload_used={} delay_ms={}",
        o.url,
        o.js,
        o.preload,
        runs.len(),
        med(&|r| r.usable),
        runs.iter().map(|r| r.usable).fold(f64::MAX, f64::min),
        runs.iter().map(|r| r.usable).fold(0.0, f64::max),
        med(&|r| r.doc_first),
        med(&|r| r.doc_warm),
        med(&|r| r.dns),
        med(&|r| r.tcp),
        med(&|r| r.style_total),
        med(&|r| r.js_total),
        med(&|r| r.images_done),
        med(&|r| r.requests as f64),
        med(&|r| r.max_conc_all as f64),
        med(&|r| r.max_conc_host as f64),
        med(&|r| r.cache_hits as f64),
        med(&|r| r.preload_used as f64),
        o.delay_ms,
    );
    let _ = (o.cache.is_some(), runs.iter().map(|r| (r.cache_hits, r.cache_misses)).count());
}
