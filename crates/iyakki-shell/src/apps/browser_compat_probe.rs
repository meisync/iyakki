//! The compat harness' second half: which web APIs the scripts of real sites name that the in-house engine does not have.
//!
//! `probe_many` (browser.rs) reports script errors; but a page that tests for an API in a `try` or an `if` is silent. This test
//! loads each address, keeps every script the page fetched, asks the engine which of the names in `assets/compat-apis.txt` are
//! missing, and counts in how many of the sites' scripts each missing name appears as a word. The result is a ranked table.
//!
//! `IYAKKI_URLS=/path/urls.txt cargo test -p iyakki-shell --lib probe_apis -- --ignored --nocapture`
//! (`IYAKKI_APIS_SAVE=dir` also keeps the scripts so a later run need not fetch again; `IYAKKI_APIS_FROM=dir` reads them.)

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use iyakki_web::fetch::{Fetch, Response};
use iyakki_web::Url;

const CATALOG: &str = include_str!("../../assets/compat-apis.txt");

/// A transport that answers from pages saved earlier.
struct Saved(HashMap<String, (String, String)>);

impl Fetch for Saved {
    fn get(&self, url: &Url) -> Result<Response, String> {
        match self.0.get(&url.to_string()) {
            Some((ct, b)) => Ok(Response { status: 200, content_type: ct.clone(), body: b.clone().into_bytes(), ..Response::default() }),
            None => Err("not saved".into()),
        }
    }
}

/// A transport that remembers what the page's scripts and documents were.
struct Rec {
    inner: Arc<dyn Fetch>,
    seen: Mutex<Vec<String>>,
    named: Mutex<Vec<(String, String, String)>>,
}

impl Fetch for Rec {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let r = self.inner.get(url)?;
        let ct = r.content_type.to_ascii_lowercase();
        if ct.contains("javascript") || ct.contains("ecmascript") || ct.contains("css") || ct.contains("html") || url.to_string().contains(".js") {
            let text = String::from_utf8_lossy(&r.body).into_owned();
            if let Ok(mut n) = self.named.lock() {
                n.push((url.to_string(), text.clone(), ct.clone()));
            }
            if let Ok(mut s) = self.seen.lock() {
                s.push(text);
            }
        }
        Ok(r)
    }
}

/// The catalog names the engine lacks, found by asking the engine itself.
pub fn missing_names(names: &[String]) -> Vec<String> {
    let url = Url::parse("https://a.org/").unwrap();
    let web: Arc<dyn Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
    let list = serde_json::to_string(names).unwrap();
    let out = Mutex::new(String::new());
    let js = format!(
        "(function(){{ const miss = []; for (const p of {list}) {{ try {{ const parts = p.split('.'); let o = globalThis; for (let i = 0; i < parts.length; i++) {{ const k = parts[i]; if (!(k in Object(o))) throw 0; if (i + 1 < parts.length) o = o[k]; }} }} catch (e) {{ miss.push(p); }} }} return JSON.stringify(miss); }})()"
    );
    iyakki_js::run_page_with("<body></body>", &url, web, |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default(), |e| {
        *out.lock().unwrap() = e.eval("probe", &js).unwrap_or_else(|_| "[]".into());
    });
    let s = out.lock().unwrap().clone();
    serde_json::from_str(&s).unwrap_or_default()
}

/// The source text around the first frame of an error note (`... (file.js:LINE:COL) ...`), when the file was fetched.
fn call_site(note: &str, files: &[(String, String, String)]) -> Option<String> {
    let at = note.find("[at ")?;
    let frame = &note[at..];
    let open = frame.find('(')?;
    let close = frame[open..].find(')')? + open;
    let loc = &frame[open + 1..close];
    let mut parts = loc.rsplitn(3, ':');
    let col: usize = parts.next()?.parse().ok()?;
    let line: usize = parts.next()?.parse().ok()?;
    let file = parts.next()?;
    let (_, body, _) = files.iter().find(|(u, _, _)| u.ends_with(file) || file.ends_with(u.rsplit('/').next().unwrap_or("?")))?;
    let l = body.lines().nth(line.checked_sub(1)?)?;
    let from = col.saturating_sub(70);
    let chars: Vec<char> = l.chars().collect();
    let end = (col + 30).min(chars.len());
    Some(chars[from.min(end)..end].iter().collect())
}

/// Is `name` used in `src`? A bare name is looked for as a word; a member (`.name`) after a dot, so that `show` or `files` do
/// not count every time the word appears.
fn mentions(src: &str, name: &str, member: bool) -> bool {
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(i) = src[from..].find(name) {
        let s = from + i;
        let e = s + name.len();
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
        let before_ok = if member { s > 0 && b[s - 1] == b'.' } else { s == 0 || !word(b[s - 1]) };
        if before_ok && (e >= b.len() || !word(b[e])) {
            return true;
        }
        from = s + 1;
    }
    false
}

#[test]
#[ignore]
fn probe_apis() {
    let Ok(file) = std::env::var("IYAKKI_URLS") else { return };
    let names: Vec<String> = CATALOG.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
    let missing = missing_names(&names);
    eprintln!("catalog {} names, {} missing from the engine", names.len(), missing.len());
    let mut sites = 0usize;
    let mut hits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Ok(dir) = std::env::var("IYAKKI_APIS_REPLAY") {
        // load the saved pages again with no network: the script errors of the engine as it is now
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        files.sort();
        let (mut ran, mut failed) = (0u32, 0u32);
        for f in files {
            let pages: Vec<(String, String, String)> = serde_json::from_str(&std::fs::read_to_string(&f).unwrap_or_default()).unwrap_or_default();
            let Some((page_url, html, _)) = pages.first().cloned() else { continue };
            let Ok(u) = Url::parse(&page_url) else { continue };
            let web: Arc<dyn Fetch> = Arc::new(Saved(pages.iter().map(|(u, b, c)| (u.clone(), (c.clone(), b.clone()))).collect()));
            let data = iyakki_js::SiteData { viewport: Some(iyakki_web::css::Viewport { width: 1280.0, height: 720.0, ..Default::default() }), ..iyakki_js::SiteData::default() };
            let res = iyakki_js::run_page(&html, &u, web, |_| true, &data, iyakki_js::Limits::default());
            ran += res.report.ran;
            failed += res.report.failed;
            eprintln!("PAGE\t{page_url}\tran {} failed {}", res.report.ran, res.report.failed);
            for note in res.report.notes.iter().filter(|n| n.starts_with("error in")) {
                eprintln!("ERR\t{page_url}\t{}", note.chars().take(200).collect::<String>());
            }
        }
        eprintln!("TOTAL\tran {ran} failed {failed}");
        return;
    }
    if let Ok(dir) = std::env::var("IYAKKI_APIS_FROM") {
        // replay: the scripts saved by an earlier run, no network
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            sites += 1;
            for m in &missing {
                let last = m.rsplit('.').next().unwrap_or(m);
                if mentions(&text, last, m.contains('.')) {
                    hits.entry(m.clone()).or_default().push(f.display().to_string());
                }
            }
        }
        print_ranking(sites, hits);
        return;
    }
    let web: Arc<dyn Fetch> = Arc::new(iyakki_agent::iyakki_proxy::https::BrowserFetch::default());
    let save = std::env::var("IYAKKI_APIS_SAVE").ok();
    for url in std::fs::read_to_string(file).unwrap().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Ok(u) = Url::parse(url) else { continue };
        let rec = Arc::new(Rec { inner: web.clone(), seen: Mutex::new(Vec::new()), named: Mutex::new(Vec::new()) });
        let (fetch, jar) = (rec.clone() as Arc<dyn Fetch>, std::sync::Mutex::new(iyakki_web::CookieStore::new()));
        let allow = |_: &str| true;
        let r = iyakki_web::fetch_following(fetch.as_ref(), iyakki_web::Request::get(&u), &allow, Some(iyakki_web::Jar { store: &jar, now: 0 }));
        let Ok((req, resp)) = r else { eprintln!("{url}: could not load"); continue };
        let html = String::from_utf8_lossy(&resp.body).into_owned();
        let res = iyakki_js::run_page(&html, &req.url, fetch, allow, &iyakki_js::SiteData::default(), iyakki_js::Limits::default());
        for note in res.report.notes.iter().filter(|n| n.starts_with("error in")) {
            eprintln!("ERR\t{url}\t{}", note.chars().take(160).collect::<String>());
            if let Some(site) = call_site(note, &rec.named.lock().unwrap()) {
                eprintln!("SITE\t{url}\t{site}");
            }
        }
        let mut all = vec![html];
        all.extend(rec.seen.lock().unwrap().iter().cloned());
        if let Some(dir) = &save {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(format!("{dir}/{sites}.txt"), all.join("\n/*--*/\n"));
            // the same pages with their addresses, so a later run can load them again with no network (IYAKKI_APIS_REPLAY)
            let mut pages = vec![(req.url.to_string(), String::from_utf8_lossy(&resp.body).into_owned(), resp.content_type.clone())];
            pages.extend(rec.named.lock().unwrap().iter().cloned());
            let _ = std::fs::write(format!("{dir}/{sites}.json"), serde_json::to_string(&pages).unwrap_or_default());
        }
        sites += 1;
        for m in &missing {
            let last = m.rsplit('.').next().unwrap_or(m);
            if all.iter().any(|s| mentions(s, last, m.contains('.'))) {
                hits.entry(m.clone()).or_default().push(url.to_string());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    print_ranking(sites, hits);
}

fn print_ranking(sites: usize, hits: BTreeMap<String, Vec<String>>) {
    let mut ranked: Vec<_> = hits.into_iter().collect();
    ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    eprintln!("API\tsites_of_{sites}\tname");
    for (name, urls) in ranked {
        eprintln!("API\t{}\t{}", urls.len(), name);
    }
}
