//! Running a page: parse it, give its scripts the plug-in modules they are allowed, run them in order, let the timers
//! and the load events play out, and hand back the page as it ended up.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_web::{CookieStore, Fetch, Url};

use crate::engine::{JsEngine, JsError, Limits, WebModule};
use crate::modules::console::ConsoleLine;
use crate::modules::storage::{KeyValueStore, MemoryStore};
use crate::modules::{run_timers, ConsoleModule, CookiesModule, DomModule, EncodingModule, EnvModule, FetchModule, IdbModule, IdbStore, StorageModule, TimersModule, UrlModule};

/// What the embedder keeps between pages: the site's `localStorage`, and the browser's cookies.
#[derive(Clone, Default)]
pub struct SiteData {
    pub local: Arc<Mutex<MemoryStore>>,
    pub cookies: Arc<Mutex<CookieStore>>,
    /// The site's `indexedDB` databases.
    pub idb: Arc<Mutex<IdbStore>>,
}

struct SharedStore(Arc<Mutex<MemoryStore>>);

impl KeyValueStore for SharedStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().ok()?.get(key)
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().map_err(|_| "storage is unavailable".to_string())?.set(key, value)
    }
    fn remove(&mut self, key: &str) {
        if let Ok(mut s) = self.0.lock() {
            s.remove(key);
        }
    }
    fn clear(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.clear();
        }
    }
    fn keys(&self) -> Vec<String> {
        self.0.lock().map(|s| s.keys()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageReport {
    /// Scripts that ran to the end.
    pub ran: u32,
    /// Scripts that threw (their messages are in `console`).
    pub failed: u32,
    /// Scripts not run: modules, other types, over the size limit.
    pub skipped: u32,
    pub console: Vec<ConsoleLine>,
    /// Where a script asked to navigate.
    pub navigation: Option<String>,
    /// Virtual milliseconds of timers that ran, and timers left over.
    pub timer_ms: u64,
    pub timers_left: u32,
    pub notes: Vec<String>,
}

impl PageReport {
    /// Lines for the developer view.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.notes.clone();
        out.extend(self.console.iter().map(|l| format!("{}: {}", l.level, l.text)));
        out
    }
}

pub struct PageResult {
    pub tree: iyakki_web::html::Tree,
    pub report: PageReport,
}

const MAX_SCRIPT: usize = 16 << 20;
const MAX_SCRIPTS: usize = 150;
/// Total time one page may spend in its scripts.
const TOTAL_WALL: Duration = Duration::from_secs(40);

/// Run the page. `allow` is the account's host policy; `data` carries storage and cookies between pages.
pub fn run_page(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits) -> PageResult {
    run_page_with(html, url, web, allow, data, limits, |_| {})
}

/// Like [`run_page`], and `after` gets the engine once the page has loaded and its timers have played out, to drive the finished
/// page further (the browser-automation tests evaluate `automation.js` there). The tree returned is the one after `after` ran.
pub fn run_page_with(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits, after: impl FnOnce(&mut JsEngine)) -> PageResult {
    match run_core(html, url, web, allow, data, limits, Vec::new(), after) {
        Ok((r, _engine)) => r,
        Err(r) => r,
    }
}

/// Like [`run_page_with`], with more modules plugged in after the standard ones (the embedder's own bridge), and the engine
/// handed back so the page can be kept alive and driven further ([`LivePage`]). `Err` is a page whose engine could not start.
#[allow(clippy::too_many_arguments)]
fn run_core(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits, extra: Vec<Box<dyn WebModule>>, after: impl FnOnce(&mut JsEngine)) -> Result<(PageResult, JsEngine), PageResult> {
    let t0 = Instant::now();
    let tree = iyakki_web::html::parse(html);
    let t_parse = t0.elapsed();
    let mut report = PageReport::default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // the page's import map (bare module names), if it has one
    let import_map: Option<String> = tree.descendants(0).into_iter().find(|&n| tree.tag(n) == Some("script") && tree.attr(n, "type").is_some_and(|t| t.trim().eq_ignore_ascii_case("importmap"))).map(|n| tree.text_of(n));
    let mut modules: Vec<Box<dyn WebModule>> = vec![
        Box::new(crate::modules::EsmModule::new(url.clone(), web.clone(), Box::new(allow.clone()), import_map.as_deref())),
        Box::new(ConsoleModule::new()),
        Box::new(TimersModule),
        Box::new(EncodingModule),
        Box::new(UrlModule),
        Box::new(EnvModule::new(url.clone())),
        Box::new(StorageModule::new(crate::modules::storage::Kind::Local, Box::new(SharedStore(data.local.clone())))),
        Box::new(StorageModule::new(crate::modules::storage::Kind::Session, Box::new(MemoryStore::default()))),
        Box::new(CookiesModule::new(data.cookies.clone(), url.clone(), now)),
        Box::new(FetchModule::new(url.clone(), web.clone(), Box::new(allow.clone())).with_cookies(data.cookies.clone(), now)),
        Box::new(DomModule::new(tree.clone())),
        // after the page object: it replaces the placeholder `indexedDB` and the `IDB*` classes defined there
        Box::new(IdbModule::new(data.idb.clone())),
    ];
    modules.extend(extra);
    let mut engine = match JsEngine::new(limits, modules) {
        Ok(e) => e,
        Err(e) => {
            report.notes.push(format!("scripts could not be started: {e}"));
            return Err(PageResult { tree, report });
        }
    };
    if std::env::var_os("IYAKKI_JS_PROFILE").is_some() {
        engine.profile_start();
    }
    let t_engine = t0.elapsed();
    let started = Instant::now();
    let scripts = engine.module::<DomModule>("dom").map(|d| d.pending_scripts()).unwrap_or_default();
    for (i, s) in scripts.iter().enumerate() {
        if started.elapsed() > TOTAL_WALL {
            report.notes.push("scripts stopped: the page took too long".into());
            report.skipped += (scripts.len() - i) as u32;
            break;
        }
        if i >= MAX_SCRIPTS {
            report.notes.push("scripts stopped: too many scripts".into());
            report.skipped += (scripts.len() - i) as u32;
            break;
        }
        let is_module = s.kind == "module";
        if s.nomodule || !(s.is_classic_js() || is_module) {
            // `nomodule` scripts are for browsers without modules; JSON, templates and import maps are data
            if !matches!(s.kind.as_str(), "importmap" | "application/json" | "application/ld+json" | "speculationrules") && !s.nomodule {
                report.skipped += 1;
            }
            continue;
        }
        if !engine.module::<DomModule>("dom").is_some_and(|d| d.claim(s.node)) {
            continue; // already run (a script that added another)
        }
        let (name, code) = match &s.src {
            Some(src) => match url.join(src).and_then(|u| fetch_script(web.as_ref(), &u, &allow)) {
                Ok(c) => (src.clone(), c),
                Err(e) => {
                    report.notes.push(format!("could not load {src}: {e}"));
                    report.failed += 1;
                    continue;
                }
            },
            None => ("inline script".to_string(), s.text.clone()),
        };
        if code.len() > MAX_SCRIPT {
            report.notes.push(format!("{name} was skipped (over {} MB)", MAX_SCRIPT >> 20));
            report.skipped += 1;
            continue;
        }
        let _ = engine.eval("current", &format!("__dom.setCurrent({})", s.node));
        let script_started = Instant::now();
        let outcome = if is_module {
            // an inline module has no address of its own: it is named after the page, so its imports resolve against it
            let mod_name = if s.src.is_some() { url.join(s.src.as_deref().unwrap_or("")).map(|u| u.to_string()).unwrap_or_else(|_| name.clone()) } else { format!("{}#module-{i}", url.without_fragment()) };
            engine.eval_module(&mod_name, &code)
        } else {
            engine.eval_cached(&name, &code).and_then(|_| engine.run_jobs().map(|_| ()))
        };
        if std::env::var_os("IYAKKI_JS_STATS").is_some() && script_started.elapsed() > Duration::from_millis(30) {
            eprintln!("JSSTATS script {:7.3} s  {} bytes  {}", script_started.elapsed().as_secs_f64(), code.len(), name.chars().take(110).collect::<String>());
        }
        let _ = engine.eval("current", "__dom.setCurrent(null)");
        match outcome {
            Ok(()) => report.ran += 1,
            Err(JsError::Exception(m)) => {
                report.failed += 1;
                // the message and the first frames of the stack, to tell what is missing
                let mut parts = m.lines();
                let first = parts.next().unwrap_or("").to_string();
                let frames: Vec<&str> = parts.take(if std::env::var_os("IYAKKI_JS_STATS").is_some() { 12 } else { 3 }).map(str::trim).collect();
                report.notes.push(if frames.is_empty() { format!("error in {name}: {first}") } else { format!("error in {name}: {first}  [{}]", frames.join(" | ")) });
            }
            Err(e) => {
                report.failed += 1;
                let place = if matches!(e, JsError::Timeout | JsError::OutOfFuel) { engine.last_stderr() } else { String::new() };
                report.notes.push(if place.is_empty() { format!("{name}: {e}") } else { format!("{name}: {e}\n{}", place.lines().take(8).collect::<Vec<_>>().join("\n")) });
                if matches!(e, JsError::Sandbox(_)) {
                    break; // the engine is not trustworthy after a trap
                }
            }
        }
    }
    let t_scripts = t0.elapsed();
    // the page has loaded: the events, then whatever the timers had waiting
    if engine.eval("load", "__dom.fireLoad()").and_then(|_| engine.run_jobs().map(|_| ())).is_err() {
        report.notes.push("the page's load handlers failed".into());
    }
    match run_timers(&mut engine, 5_000, 2_000) {
        Ok(t) => {
            report.timer_ms = t.virtual_ms;
            report.timers_left = t.pending;
        }
        Err(e) => report.notes.push(format!("timers stopped: {e}")),
    }
    after(&mut engine);
    if std::env::var_os("IYAKKI_JS_PROFILE").is_some() {
        eprintln!("JSPROF {}", engine.profile_report());
    }
    if std::env::var_os("IYAKKI_JS_STATS").is_some() {
        eprintln!("JSSTATS phases: parse {:.2} s, engine up {:.2} s, scripts {:.2} s, load+timers {:.2} s", t_parse.as_secs_f64(), (t_engine - t_parse).as_secs_f64(), (t_scripts - t_engine).as_secs_f64(), (t0.elapsed() - t_scripts).as_secs_f64());
        let all = engine.host_stats();
        let total: Duration = all.iter().map(|x| x.2).sum();
        eprintln!("JSSTATS host calls total {:.2} s in {} calls; script phase {:.2} s", total.as_secs_f64(), all.iter().map(|x| x.1).sum::<u64>(), started.elapsed().as_secs_f64());
        for (k, n, t) in all.iter().take(25) {
            eprintln!("JSSTATS   {k:40} {n:9} calls {:8.3} s", t.as_secs_f64());
        }
    }
    report.console = engine.module::<ConsoleModule>("console").map(|c| std::mem::take(&mut c.lines)).unwrap_or_default();
    report.navigation = engine.module::<EnvModule>("window").and_then(|e| e.navigation.clone());
    let tree = engine.module::<DomModule>("dom").map(|d| d.tree.clone()).unwrap_or(tree);
    Ok((PageResult { tree, report }, engine))
}

/// A page kept alive after it loaded: its engine stays, so clicks and typing reach its scripts and the page can be drawn again.
/// This is what an app that lives in a tile (not a page that is read once) runs on.
pub struct LivePage {
    engine: JsEngine,
    pub report: PageReport,
    pub url: Url,
}

impl LivePage {
    /// Load `html` at `url` with `extra` modules (the app's bridge) plugged in after the standard ones. `None`: the engine could not start.
    pub fn load(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits, extra: Vec<Box<dyn WebModule>>) -> Option<LivePage> {
        let (res, engine) = run_core(html, url, web, allow, data, limits, extra, |_| {}).ok()?;
        Some(LivePage { engine, report: res.report, url: url.clone() })
    }

    /// The page as it is now.
    pub fn tree(&mut self) -> iyakki_web::html::Tree {
        self.engine.module::<DomModule>("dom").map(|d| d.tree.clone()).unwrap_or_else(|| iyakki_web::html::parse(""))
    }

    /// Run a script in the page (tests and the app's host).
    pub fn eval(&mut self, name: &str, js: &str) -> Result<String, JsError> {
        let r = self.engine.eval(name, js)?;
        let _ = self.engine.run_jobs();
        Ok(r)
    }

    fn settle(&mut self) {
        let _ = run_timers(&mut self.engine, 50, 200);
    }

    /// Click the control made from this node of the tree.
    pub fn click(&mut self, node: usize) -> Result<(), JsError> {
        self.engine.eval("click", &format!("__dom.click({node})"))?;
        self.settle();
        Ok(())
    }

    /// Type this text into the field made from this node.
    pub fn input(&mut self, node: usize, value: &str) -> Result<(), JsError> {
        self.engine.eval("input", &format!("__dom.input({node}, {})", json_str(value)))?;
        self.settle();
        Ok(())
    }

    /// Check or uncheck a box or radio button.
    pub fn check(&mut self, node: usize, on: bool) -> Result<(), JsError> {
        self.engine.eval("check", &format!("__dom.check({node}, {on})"))?;
        self.settle();
        Ok(())
    }

    /// Let `ms` virtual milliseconds of timers pass (a clock in the page ticks).
    pub fn advance(&mut self, ms: u64) -> u32 {
        run_timers(&mut self.engine, ms, 200).map(|t| t.ran).unwrap_or(0)
    }

    /// Lines the page's scripts printed since the last call.
    pub fn take_console(&mut self) -> Vec<ConsoleLine> {
        self.engine.module::<ConsoleModule>("console").map(|c| std::mem::take(&mut c.lines)).unwrap_or_default()
    }

    /// A module plugged in at load, by name (the shell reads the bridge's outbox through it).
    pub fn module<T: 'static>(&mut self, name: &str) -> Option<&mut T> {
        self.engine.module::<T>(name)
    }

    /// The engine, for a host that needs to hand a result to the page (`iyakki.__resolve`).
    pub fn engine(&mut self) -> &mut JsEngine {
        &mut self.engine
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn fetch_script(web: &dyn Fetch, url: &Url, allow: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let mut cur = url.without_fragment();
    for _ in 0..4 {
        if !allow(&cur.host) {
            return Err(format!("your account may not open pages from {}", cur.host));
        }
        let r = web.get(&cur)?;
        if (300..400).contains(&r.status) {
            cur = cur.join(r.location.as_deref().ok_or("redirect with nowhere to go")?)?.without_fragment();
            continue;
        }
        if r.status >= 400 {
            return Err(format!("the server answered {}", r.status));
        }
        return Ok(String::from_utf8_lossy(&r.body).into_owned());
    }
    Err("too many redirects".into())
}

/// A renderer for [`iyakki_web::load_with`] that runs scripts.
pub fn renderer(web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: SiteData) -> impl Fn(&Url, &str) -> (iyakki_web::Doc, Vec<String>) {
    renderer_with(web, allow, data, |_, tree| iyakki_web::doc::build(tree))
}

/// Like [`renderer`], with the last step (the finished page's tree to what is shown) chosen by the caller, for
/// instance to apply style sheets.
pub fn renderer_with(web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: SiteData, build: impl Fn(&Url, &iyakki_web::html::Tree) -> iyakki_web::Doc) -> impl Fn(&Url, &str) -> (iyakki_web::Doc, Vec<String>) {
    move |url, html| {
        let r = run_page(html, url, web.clone(), allow.clone(), &data, Limits::default());
        let mut notes = r.report.lines();
        if r.report.ran + r.report.failed > 0 {
            notes.insert(0, format!("scripts: {} ran, {} failed, {} skipped", r.report.ran, r.report.failed, r.report.skipped));
        }
        (build(url, &r.tree), notes)
    }
}

/// Data for a browsing session: one `localStorage` per origin, and one cookie jar for everything.
#[derive(Default, Clone)]
pub struct Sites {
    local: Arc<Mutex<BTreeMap<String, Arc<Mutex<MemoryStore>>>>>,
    idb: Arc<Mutex<BTreeMap<String, Arc<Mutex<IdbStore>>>>>,
    pub cookies: Arc<Mutex<CookieStore>>,
}

fn key_of(url: &Url) -> String {
    format!("{}://{}:{}", if url.https { "https" } else { "http" }, url.host, url.port.unwrap_or(if url.https { 443 } else { 80 }))
}

impl Sites {
    pub fn for_origin(&self, url: &Url) -> SiteData {
        let key = key_of(url);
        let local = self.local.lock().map(|mut m| m.entry(key).or_default().clone()).unwrap_or_default();
        let idb = self.idb.lock().map(|mut m| m.entry(key_of(url)).or_default().clone()).unwrap_or_default();
        SiteData { local, idb, cookies: self.cookies.clone() }
    }

    /// The `localStorage` of every origin that holds something: `(origin, entries)`.
    pub fn snapshot_local(&self) -> Vec<(String, BTreeMap<String, String>)> {
        let Ok(m) = self.local.lock() else { return Vec::new() };
        m.iter().filter_map(|(o, s)| s.lock().ok().filter(|s| !s.0.is_empty()).map(|s| (o.clone(), s.0.clone()))).collect()
    }

    /// The `indexedDB` databases of every origin that has any: `(origin, json)`. Restore with [`Sites::restore_idb`].
    pub fn snapshot_idb(&self) -> Vec<(String, String)> {
        let Ok(m) = self.idb.lock() else { return Vec::new() };
        m.iter().filter_map(|(o, s)| s.lock().ok().filter(|s| !s.is_empty()).map(|s| (o.clone(), s.to_json()))).collect()
    }

    /// Put saved `indexedDB` databases back (the origin is written as `https://host:443`); false if the text is not valid.
    pub fn restore_idb(&self, origin: &str, json: &str) -> bool {
        let Some(saved) = IdbStore::from_json(json) else { return false };
        if let Ok(mut m) = self.idb.lock() {
            let slot = m.entry(origin.to_string()).or_default().clone();
            let mut guard = slot.lock();
            if let Ok(s) = guard.as_mut() {
                **s = saved;
            }
        }
        true
    }

    /// Put saved `localStorage` entries back (the origin is written as `https://host:443`).
    pub fn restore_local(&self, origin: &str, entries: BTreeMap<String, String>) {
        if let Ok(mut m) = self.local.lock() {
            let slot = m.entry(origin.to_string()).or_default().clone();
            let mut guard = slot.lock();
            if let Ok(s) = guard.as_mut() {
                s.0 = entries;
            }
        }
    }

    /// Forget everything stored for a host: its `localStorage` (all origins) and its cookies.
    pub fn forget_host(&self, host: &str) {
        if let Ok(mut m) = self.local.lock() {
            m.retain(|o, _| o.split("://").nth(1).and_then(|r| r.rsplit_once(':')).map(|(h, _)| h) != Some(host));
        }
        if let Ok(mut m) = self.idb.lock() {
            m.retain(|o, _| o.split("://").nth(1).and_then(|r| r.rsplit_once(':')).map(|(h, _)| h) != Some(host));
        }
        if let Ok(mut c) = self.cookies.lock() {
            c.clear_host(host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_web::{Block, Response};
    use std::collections::HashMap;

    struct Site(HashMap<String, Response>);
    impl Fetch for Site {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.0.get(&url.to_string()).cloned().ok_or_else(|| "connection refused".to_string())
        }
    }

    fn site(pages: Vec<(&str, &str, &str)>) -> Arc<dyn Fetch> {
        Arc::new(Site(pages.into_iter().map(|(u, ct, b)| (u.to_string(), Response { status: 200, content_type: ct.into(), body: b.as_bytes().to_vec(), ..Response::default() })).collect()))
    }

    fn texts(doc: &iyakki_web::Doc) -> Vec<String> {
        doc.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(format!("{}{}", if t.marker.is_empty() { String::new() } else { format!("{} ", t.marker) }, t.spans.iter().map(|s| s.text.as_str()).collect::<String>())) } else { None }).collect()
    }

    fn run(html: &str, web: Arc<dyn Fetch>) -> (iyakki_web::Doc, PageReport) {
        let r = run_page(html, &Url::parse("https://a.org/").unwrap(), web, |_| true, &SiteData::default(), Limits::default());
        (iyakki_web::doc::build(&r.tree), r.report)
    }

    #[test]
    fn scripts_fill_in_the_page_before_it_is_shown() {
        let (doc, r) = run("<h1 id=t>Loading…</h1><div id=app></div><script>document.getElementById('t').textContent = 'Ready'; const ul = document.createElement('ul'); for (const n of ['a', 'b']) { const li = document.createElement('li'); li.textContent = n; ul.appendChild(li); } document.getElementById('app').appendChild(ul);</script>", site(vec![]));
        assert_eq!(texts(&doc), vec!["Ready", "• a", "• b"]);
        assert_eq!((r.ran, r.failed), (1, 0));
    }

    #[test]
    fn external_scripts_timers_and_load_handlers_all_play_out() {
        let web = site(vec![("https://a.org/app.js", "text/javascript", "document.addEventListener('DOMContentLoaded', () => { document.getElementById('x').textContent = 'dcl'; }); setTimeout(() => { document.getElementById('y').textContent = 'timer'; }, 1500);")]);
        let (doc, r) = run("<p id=x>x</p><p id=y>y</p><script src=\"/app.js\"></script>", web);
        assert_eq!(texts(&doc), vec!["dcl", "timer"]);
        assert_eq!(r.timer_ms, 1500, "the wait took no real time");
    }

    #[test]
    fn data_loaded_with_fetch_ends_up_on_the_page() {
        let web = site(vec![("https://a.org/api/items", "application/json", "{\"items\": [\"alpha\", \"beta\"]}")]);
        let (doc, _) = run("<ul id=l></ul><script>fetch('/api/items').then(r => r.json()).then(d => { document.getElementById('l').innerHTML = d.items.map(i => '<li>' + i + '</li>').join(''); });</script>", web);
        assert_eq!(texts(&doc), vec!["• alpha", "• beta"]);
    }

    #[test]
    fn a_broken_script_does_not_take_the_page_down() {
        let (doc, r) = run("<p id=a>one</p><script>throw new Error('first fails')</script><script>document.getElementById('a').textContent = 'second ran'</script><script>this is not javascript</script><script type=\"module\">import x from './y.js'</script><script type=\"application/ld+json\">{\"a\":1}</script>", site(vec![]));
        assert_eq!(texts(&doc), vec!["second ran"]);
        assert_eq!((r.ran, r.failed, r.skipped), (1, 3, 0), "the module that imports a missing file fails, JSON is data");
        assert!(r.notes.iter().any(|n| n.contains("first fails")) && r.notes.iter().any(|n| n.contains("SyntaxError")) && r.notes.iter().any(|n| n.contains("connection refused") || n.contains("y.js")), "{:?}", r.notes);
    }

    #[test]
    fn a_runaway_script_is_stopped_and_the_rest_of_the_page_still_shows() {
        let started = Instant::now();
        let r = run_page("<p id=a>text</p><script>while (true) {}</script><script>document.getElementById('a').textContent = 'after'</script>", &Url::parse("https://a.org/").unwrap(), site(vec![]), |_| true, &SiteData::default(), Limits { wall: Duration::from_millis(300), fuel: 1_000_000_000, ..Limits::default() });
        let doc = iyakki_web::doc::build(&r.tree);
        assert_eq!(texts(&doc), vec!["after"]);
        assert!(r.report.notes.iter().any(|n| n.contains("ran too long")), "{:?}", r.report.notes);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn local_storage_can_be_saved_restored_and_forgotten_per_host() {
        let sites = Sites::default();
        let u = Url::parse("https://a.org/x").unwrap();
        sites.for_origin(&u).local.lock().unwrap().set("k", "v").unwrap();
        sites.for_origin(&Url::parse("https://b.org/").unwrap()).local.lock().unwrap().set("other", "1").unwrap();
        let snap = sites.snapshot_local();
        assert_eq!(snap.len(), 2);
        let fresh = Sites::default();
        for (o, e) in snap {
            fresh.restore_local(&o, e);
        }
        assert_eq!(fresh.for_origin(&u).local.lock().unwrap().get("k"), Some("v".into()));
        fresh.forget_host("a.org");
        assert_eq!(fresh.for_origin(&u).local.lock().unwrap().get("k"), None);
        assert_eq!(fresh.for_origin(&Url::parse("https://b.org/").unwrap()).local.lock().unwrap().get("other"), Some("1".into()));
    }

    #[test]
    fn pages_of_one_site_share_storage_and_cookies_but_other_sites_do_not() {
        let sites = Sites::default();
        let (u1, u2, other) = (Url::parse("https://a.org/one").unwrap(), Url::parse("https://a.org/two").unwrap(), Url::parse("https://b.org/").unwrap());
        let web = site(vec![]);
        let go = |u: &Url, script: &str| {
            let r = run_page(&format!("<p id=o></p><script>{script}</script>"), u, web.clone(), |_| true, &sites.for_origin(u), Limits::default());
            iyakki_web::doc::build(&r.tree)
        };
        go(&u1, "localStorage.visits = '1'; document.cookie = 'theme=dark; Max-Age=60';");
        let d = go(&u2, "document.getElementById('o').textContent = localStorage.visits + '|' + document.cookie;");
        assert_eq!(texts(&d), vec!["1|theme=dark"]);
        let d = go(&other, "document.getElementById('o').textContent = localStorage.visits + '|' + document.cookie;");
        assert_eq!(texts(&d), vec!["undefined|"]);
    }

    #[test]
    fn script_requests_obey_the_host_policy() {
        let web = site(vec![("https://evil.net/x.js", "text/javascript", "document.getElementById('a').textContent = 'evil ran'"), ("https://a.org/ok.js", "text/javascript", "document.getElementById('a').textContent = 'ok ran'")]);
        let r = run_page("<p id=a>text</p><script src=\"https://evil.net/x.js\"></script><script src=\"/ok.js\"></script>", &Url::parse("https://a.org/").unwrap(), web, |h| h == "a.org", &SiteData::default(), Limits::default());
        assert_eq!(texts(&iyakki_web::doc::build(&r.tree)), vec!["ok ran"]);
        assert!(r.report.notes.iter().any(|n| n.contains("may not open pages from evil.net")));
    }

    #[test]
    fn navigation_requests_are_reported() {
        let r = run_page("<script>location.href = '/elsewhere'</script>", &Url::parse("https://a.org/").unwrap(), site(vec![]), |_| true, &SiteData::default(), Limits::default());
        assert_eq!(r.report.navigation.as_deref(), Some("https://a.org/elsewhere"));
    }

    #[test]
    fn the_renderer_plugs_into_the_loader() {
        let web = site(vec![("https://a.org/", "text/html", "<title>T</title><p id=a>old</p><script>document.getElementById('a').textContent = 'new'</script>")]);
        let render = renderer(web.clone(), |_| true, SiteData::default());
        let l = iyakki_web::load_with(web.as_ref(), &Url::parse("https://a.org/").unwrap(), &|_| true, &render).unwrap();
        let iyakki_web::Loaded::Page { doc, notes, .. } = l else { panic!() };
        assert_eq!((doc.title.as_str(), texts(&doc)), ("T", vec!["new".to_string()]));
        assert!(notes[0].starts_with("scripts: 1 ran, 0 failed"), "{notes:?}");
    }

    #[test]
    fn es_modules_import_each_other_use_import_meta_and_dynamic_import_and_top_level_await() {
        let web = site(vec![
            ("https://a.org/lib/util.js", "text/javascript", "import { two } from './two.js'; export const one = 1; export default function add(a, b) { return a + b + two; }"),
            ("https://a.org/lib/two.js", "text/javascript", "export const two = 2; export const where = import.meta.url;"),
            ("https://a.org/lazy.js", "text/javascript", "export const lazy = 'loaded later';"),
            ("https://a.org/app.js", "text/javascript", "import add, { one } from '/lib/util.js'; import { where } from './lib/two.js'; document.getElementById('a').textContent = add(one, 10) + ' ' + where; const m = await import('./lazy.js'); document.getElementById('b').textContent = m.lazy;"),
        ]);
        let (doc, r) = run("<p id=a>-</p><p id=b>-</p><script type=module src=/app.js></script><script type=module>import { two } from '/lib/two.js'; document.getElementById('b').textContent += ' inline ' + two;</script><script nomodule>document.getElementById('a').textContent = 'nomodule ran';</script>", web);
        assert_eq!((r.ran, r.failed), (2, 0), "{:?}", r.notes);
        assert_eq!(texts(&doc), vec!["13 https://a.org/lib/two.js", "loaded later inline 2"], "{:?}", r.notes);
    }

    #[test]
    fn bare_names_need_an_import_map_and_module_errors_are_reported() {
        let web = site(vec![("https://a.org/vendor/lib.js", "text/javascript", "export const v = 'mapped';"), ("https://a.org/broken.js", "text/javascript", "export const = ;"), ("https://a.org/throws.js", "text/javascript", "throw new Error('module boom');")]);
        let (doc, r) = run("<p id=a>-</p><script type=importmap>{\"imports\":{\"lib\":\"/vendor/lib.js\",\"pkg/\":\"/vendor/\"}}</script><script type=module>import { v } from 'lib'; import * as p from 'pkg/lib.js'; document.getElementById('a').textContent = v + ' ' + p.v;</script><script type=module>import 'nothing-here';</script><script type=module src=/broken.js></script><script type=module src=/throws.js></script>", web);
        assert_eq!(texts(&doc), vec!["mapped mapped"]);
        assert_eq!((r.ran, r.failed), (1, 3), "{:?}", r.notes);
        let notes = r.notes.join("\n");
        assert!(notes.contains("no import map entry") && notes.contains("module boom"), "{notes}");
    }

    #[test]
    fn modules_obey_the_host_policy_and_current_script_is_set_while_a_script_runs() {
        let web = site(vec![("https://evil.net/m.js", "text/javascript", "export const x = 1;")]);
        let r = run_page("<p id=a>-</p><script type=module>import { x } from 'https://evil.net/m.js'; document.getElementById('a').textContent = x;</script>", &Url::parse("https://a.org/").unwrap(), web.clone(), |h| h == "a.org", &SiteData::default(), Limits::default());
        assert_eq!(r.report.failed, 1);
        assert!(r.report.notes.join(" ").contains("may not open pages from evil.net"), "{:?}", r.report.notes);
        let (doc, r2) = run("<p id=a>-</p><script id=me data-x=7>document.getElementById('a').textContent = document.currentScript.id + document.currentScript.getAttribute('data-x');</script><script>document.getElementById('a').textContent += ' ' + (document.currentScript !== null);</script>", web);
        assert_eq!(texts(&doc), vec!["me7 true"], "{:?}", r2.notes);
    }

    #[test]
    fn custom_elements_with_shadow_roots_and_slots_show_their_content() {
        let (doc, r) = run("<x-card>light words</x-card><p id=after>after</p><script>customElements.define('x-card', class extends HTMLElement { connectedCallback() { const r = this.attachShadow({ mode: 'open' }); r.innerHTML = '<h2>Card title</h2><div>shadow says <slot></slot></div>'; } });</script><script>const e = document.createElement('x-card'); e.textContent = 'made later'; document.body.appendChild(e);</script>", site(vec![]));
        assert_eq!(r.failed, 0, "{:?}", r.notes);
        assert_eq!(texts(&doc), vec!["Card title", "shadow says light words", "after", "Card title", "shadow says made later"]);
    }

    #[test]
    fn template_content_and_upgrades_and_attribute_callbacks() {
        let (doc, r) = run("<template id=t><li>one</li><li>two</li></template><ul id=l></ul><x-a data-v=1></x-a><script>document.getElementById('l').appendChild(document.importNode(document.getElementById('t').content, true)); class A extends HTMLElement { static get observedAttributes() { return ['data-v']; } attributeChangedCallback(n, o, v) { this.textContent = 'a:' + n + '=' + v; } } customElements.define('x-a', A); document.querySelector('x-a').setAttribute('data-v', '2'); customElements.whenDefined('x-a').then(() => { document.title = 'defined'; });</script>", site(vec![]));
        assert_eq!(r.failed, 0, "{:?}", r.notes);
        assert_eq!(texts(&doc), vec!["• one", "• two", "a:data-v=2"]);
    }
}
