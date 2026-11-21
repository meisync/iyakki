//! Page scripts against a real browser: the same page, served from a loopback server, is loaded by headless Chrome and by our page
//! loader; the order in which blocking, deferred, inline and dynamically added scripts run, what they see of the page when they run and the
//! values they leave must be the same. Chrome is only the oracle. Skipped (with a message) when Chrome is missing.
//! (Async scripts race by design; the page with one only checks that it ran after the blocking ones.)

use iyakki_js::{run_page_with, Limits, SiteData};
use iyakki_web::{Fetch, Response, Url};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;

fn chrome() -> Option<String> {
    let mut names: Vec<String> = std::env::var("IYAKKI_CHROME").into_iter().collect();
    names.extend(["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map(String::from));
    names.into_iter().find(|n| Command::new(n).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
}

struct Files(HashMap<String, String>);
impl Fetch for Files {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let path = url.to_string().trim_start_matches("https://a.org").to_string();
        self.0.get(&path).map(|b| Response { status: 200, content_type: "text/javascript".into(), body: b.as_bytes().to_vec(), ..Response::default() }).ok_or_else(|| "404".to_string())
    }
}

/// A plain loopback HTTP server for a few files; returns the port.
fn serve(files: HashMap<String, (String, String)>, got: std::sync::mpsc::Sender<String>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let files = files.clone();
            let got = got.clone();
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                if let Some(q) = path.strip_prefix("/__log?") {
                    let _ = got.send(percent(q));
                }
                let (code, ct, body) = match files.get(&path) {
                    Some((ct, b)) => ("200 OK", ct.clone(), b.clone()),
                    None => ("404 Not Found", "text/plain".into(), "no".into()),
                };
                let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Type: {ct}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    port
}

fn percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What Chrome leaves in `window.__log` (it is copied to the title once the page has loaded).
fn in_chrome(html: &str, scripts: &HashMap<String, String>) -> String {
    let mut files: HashMap<String, (String, String)> = scripts.iter().map(|(k, v)| (k.clone(), ("text/javascript".to_string(), v.clone()))).collect();
    let probe = "<script>addEventListener('load',()=>setTimeout(()=>{new Image().src='/__log?'+encodeURIComponent((window.__log||[]).join(','))},500))</script>";
    files.insert("/".into(), ("text/html".into(), format!("<!doctype html><html><head>{probe}</head><body>{html}</body></html>")));
    let (tx, rx) = std::sync::mpsc::channel();
    let port = serve(files, tx);
    let d = std::env::temp_dir().join(format!("iyakki-js-chrome-{}-{port}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let mut child = Command::new(chrome().unwrap())
        .args(["--headless=new", "--no-sandbox", "--disable-gpu"])
        .arg(format!("--user-data-dir={}", d.display()))
        .arg(format!("http://127.0.0.1:{port}/"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("chrome runs");
    let got = rx.recv_timeout(std::time::Duration::from_secs(20));
    let _ = child.kill();
    let _ = child.wait();
    let _ = Command::new("pkill").args(["-f", &format!("user-data-dir={}", d.display())]).status();
    let _ = std::fs::remove_dir_all(&d);
    got.unwrap_or_else(|_| panic!("chrome did not finish the page"))
}

fn ours(html: &str, scripts: &HashMap<String, String>) -> String {
    let web = Arc::new(Files(scripts.clone()));
    let mut log = String::new();
    let page = format!("<!doctype html><html><head></head><body>{html}</body></html>");
    let res = run_page_with(&page, &Url::parse("https://a.org/").unwrap(), web, |_| true, &SiteData::default(), Limits::default(), |e| {
        log = e.eval("log", "(window.__log || []).join(',')").unwrap_or_default();
    });
    let _ = res;
    log
}

fn push(what: &str) -> String {
    format!("(window.__log = window.__log || []).push('{what}');")
}

#[test]
fn script_order_and_what_scripts_see_match_a_real_chrome() {
    if chrome().is_none() {
        eprintln!("skipped: no Chrome");
        return;
    }
    let scripts: HashMap<String, String> = [
        ("/a.js", push("a")),
        ("/b.js", push("b")),
        ("/d1.js", push("d1")),
        ("/d2.js", push("d2")),
        ("/y.js", push("y")),
        ("/seesp.js", "(window.__log = window.__log || []).push('p' + document.querySelectorAll('p').length);".to_string()),
        ("/m.js", format!("{}export {{}};", push("m"))),
        ("/adds.js", "var s = document.createElement('script'); s.src = '/dyn.js'; document.body.appendChild(s); (window.__log = window.__log || []).push('adds');".to_string()),
        ("/dyn.js", push("dyn")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let pages: &[(&str, &str)] = &[
        ("blocking in order", "<script src=/a.js></script><script src=/b.js></script><script>(window.__log = window.__log || []).push('inline');</script>"),
        ("defer after blocking and inline, in order", "<script defer src=/d1.js></script><script src=/a.js></script><script defer src=/d2.js></script><script>(window.__log = window.__log || []).push('inline');</script>"),
        ("an inline script with defer is blocking", "<script defer>(window.__log = window.__log || []).push('inline-defer');</script><script defer src=/d1.js></script><script>(window.__log = window.__log || []).push('inline');</script>"),
        ("a module runs after the blocking scripts", "<script type=module src=/m.js></script><script src=/a.js></script><script defer src=/d1.js></script>"),
        ("a blocking script sees only what is written above it", "<p>1</p><script src=/seesp.js></script><p>2</p><script src=/seesp.js></script><p>3</p><script defer src=/seesp.js></script>"),
        ("a script added by a script runs after the adder", "<script src=/adds.js></script><script src=/b.js></script>"),
        ("a script that fails does not stop the next", "<script src=/a.js></script><script>throw new Error('x')</script><script src=/b.js></script>"),
    ];
    let mut bad = Vec::new();
    for (name, html) in pages {
        let (a, c) = (ours(html, &scripts), in_chrome(html, &scripts));
        eprintln!("{name}: ours [{a}] chrome [{c}]");
        if a != c {
            bad.push(format!("{name}: ours [{a}] chrome [{c}]"));
        }
    }
    assert!(bad.is_empty(), "differences from Chrome:\n{}", bad.join("\n"));
    // async scripts race: only that it ran, after the blocking one
    let html = "<script async src=/y.js></script><script src=/a.js></script>";
    for (who, log) in [("ours", ours(html, &scripts)), ("chrome", in_chrome(html, &scripts))] {
        // an async script may run before the blocking one in a real browser (it depends on which file arrives first): only that both ran;
        // ours runs the blocking one first, deterministically
        assert!(log.contains('y') && log.contains('a') && (who == "chrome" || log.starts_with('a')), "{who}: [{log}]");
    }
}

#[test]
fn storage_cookies_and_indexeddb_give_the_same_answers_as_a_real_chrome() {
    if chrome().is_none() {
        eprintln!("skipped: no Chrome");
        return;
    }
    let log = "var L = (window.__log = window.__log || []);";
    let cases: &[(&str, String)] = &[
        (
            "localStorage",
            format!(
                "{log} localStorage.clear(); localStorage.setItem('b','2'); localStorage.setItem('a','1'); localStorage.setItem('n', 5); localStorage.setItem('a','11'); localStorage.removeItem('b'); \
                 L.push(localStorage.length, localStorage.getItem('a'), localStorage.getItem('zz'), typeof localStorage.n, localStorage.n, 'b' in localStorage, JSON.stringify(Object.keys(localStorage).sort()));"
            ),
        ),
        (
            "sessionStorage",
            format!("{log} sessionStorage.clear(); sessionStorage.setItem('k', {{a:1}}); L.push(sessionStorage.getItem('k'), sessionStorage.length); sessionStorage.clear(); L.push(sessionStorage.length);"),
        ),
        (
            "cookies",
            format!(
                "{log} document.cookie = 'a=1; path=/'; document.cookie = 'b=2; path=/'; L.push(document.cookie); \
                 document.cookie = 'b=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/'; L.push(document.cookie); document.cookie = 'a=3; path=/'; L.push(document.cookie); \
                 document.cookie = 'c=x y; path=/other'; L.push(document.cookie); document.cookie = 'd=1; max-age=0; path=/'; L.push(document.cookie);"
            ),
        ),
        (
            "indexedDB",
            format!(
                "{log} const rq = (r) => new Promise((ok, no) => {{ r.onsuccess = () => ok(r.result); r.onerror = () => no(r.error); }}); \
                 const all = (store, dir, range) => new Promise((ok) => {{ const out = []; const r = store.openCursor(range, dir); r.onsuccess = () => {{ const c = r.result; if (c) {{ out.push(c.value ? c.key : c.key); c.continue(); }} else ok(out); }}; }}); \
                 (async () => {{ \
                   const o = indexedDB.open('db1', 1); \
                   o.onupgradeneeded = () => {{ const s = o.result.createObjectStore('s', {{ keyPath: 'id' }}); s.createIndex('byname', 'name'); s.createIndex('uniq', 'u', {{ unique: true }}); }}; \
                   const db = await rq(o); \
                   let tx = db.transaction('s', 'readwrite'); let s = tx.objectStore('s'); \
                   for (const [id, name, u] of [[10, 'x', 1], [9, 'y', 2], ['b', 'x', 3], ['a', 'z', 4], [2.5, 'y', 5]]) s.put({{ id, name, u }}); \
                   await new Promise((ok) => (tx.oncomplete = ok)); \
                   tx = db.transaction('s', 'readonly'); s = tx.objectStore('s'); \
                   L.push('order:' + (await all(s, 'next')).join('|')); L.push('prev:' + (await all(s, 'prev')).join('|')); \
                   L.push('range:' + (await all(s, 'next', IDBKeyRange.bound(9, 'a'))).join('|')); \
                   L.push('get:' + (await rq(s.get(9))).name + ',' + (await rq(s.index('byname').get('z'))).id + ',' + (await rq(s.count()))); \
                   const t2 = db.transaction('s', 'readwrite'); const dup = t2.objectStore('s').put({{ id: 99, name: 'q', u: 1 }}); \
                   await new Promise((ok) => {{ dup.onerror = (e) => {{ L.push('dup:' + dup.error.name); }}; t2.onabort = () => {{ L.push('aborted'); ok(); }}; t2.oncomplete = () => ok(); }}); \
                   L.push('after:' + (await rq(db.transaction('s').objectStore('s').count()))); \
                 }})().catch((e) => L.push('ERR ' + e));"
            ),
        ),
    ];
    let mut bad = Vec::new();
    for (name, src) in cases {
        let scripts: HashMap<String, String> = [("/t.js".to_string(), src.clone())].into_iter().collect();
        let html = "<script src=/t.js></script>";
        let (a, c) = (ours(html, &scripts), in_chrome(html, &scripts));
        eprintln!("{name}: ours [{a}] chrome [{c}]");
        if a != c || a.is_empty() {
            bad.push(format!("{name}: ours [{a}] chrome [{c}]"));
        }
    }
    assert!(bad.is_empty(), "differences from Chrome:\n{}", bad.join("\n"));
}
