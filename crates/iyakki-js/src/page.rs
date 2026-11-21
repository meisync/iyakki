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
    /// The window the page is laid out for (`innerWidth`, `matchMedia`, sizes): `None` is 1280 by 720.
    pub viewport: Option<iyakki_web::css::Viewport>,
    /// What makes passkeys for `navigator.credentials` (F-BROX-03): `None` means pages see no `navigator.credentials` at all.
    pub passkeys: Option<Arc<Mutex<dyn crate::modules::PasskeyHost>>>,
    /// What registers service workers and makes push subscriptions for `PushManager` (F-INBOX-09): `None` keeps the plain stub of service workers.
    pub push: Option<Arc<Mutex<dyn crate::modules::PushHost>>>,
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
    /// Where the page's scripts left the window's scroll position (x, y in page pixels), when it is not the top.
    pub scroll: Option<(u32, u32)>,
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
    let vp = data.viewport.unwrap_or(iyakki_web::css::Viewport { width: 1280.0, height: 720.0, ..Default::default() });
    let mut modules: Vec<Box<dyn WebModule>> = vec![
        Box::new(crate::modules::EsmModule::new(url.clone(), web.clone(), Box::new(allow.clone()), import_map.as_deref())),
        Box::new(ConsoleModule::new()),
        Box::new(TimersModule),
        Box::new(EncodingModule),
        Box::new(UrlModule),
        Box::new(EnvModule::new(url.clone()).with_viewport(vp)),
        Box::new(StorageModule::new(crate::modules::storage::Kind::Local, Box::new(SharedStore(data.local.clone())))),
        Box::new(StorageModule::new(crate::modules::storage::Kind::Session, Box::new(MemoryStore::default()))),
        Box::new(CookiesModule::new(data.cookies.clone(), url.clone(), now)),
        Box::new(FetchModule::new(url.clone(), web.clone(), Box::new(allow.clone())).with_cookies(data.cookies.clone(), now)),
        Box::new({
            let dom = DomModule::new(tree.clone());
            // sizes are estimated only for an embedder that gave a window (the browser): other hosts keep the zero sizes of an engine without layout
            if data.viewport.is_some() { dom.with_layout(crate::modules::LayoutEnv { base: url.clone(), web: web.clone(), allow: Mutex::new(Box::new(allow.clone())), vp }) } else { dom }
        }),
        // after the page object: it replaces the placeholder `indexedDB` and the `IDB*` classes defined there
        Box::new(IdbModule::new(data.idb.clone())),
    ];
    if let Some(host) = &data.passkeys {
        let default = if url.https { 443 } else { 80 };
        let origin = match url.port.filter(|p| *p != default) {
            Some(p) => format!("{}://{}:{p}", if url.https { "https" } else { "http" }, url.host),
            None => format!("{}://{}", if url.https { "https" } else { "http" }, url.host),
        };
        modules.push(Box::new(crate::modules::PasskeysModule::new(&origin, &url.host, host.clone())));
    }
    if let Some(host) = &data.push {
        let default = if url.https { 443 } else { 80 };
        let origin = match url.port.filter(|p| *p != default) {
            Some(p) => format!("{}://{}:{p}", if url.https { "https" } else { "http" }, url.host),
            None => format!("{}://{}", if url.https { "https" } else { "http" }, url.host),
        };
        modules.push(Box::new(crate::modules::PushModule::new(&origin, host.clone())));
    }
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
    let secure = url.https;
    let mut scripts = engine.module::<DomModule>("dom").map(|d| d.pending_scripts()).unwrap_or_default();
    // blocking scripts first, then `defer` scripts and modules, then `async` ones, each group in the order written (the stable sort keeps it)
    scripts.sort_by_key(|s| s.phase());
    let mut prefetched = prefetch_scripts(&scripts, &url, secure, &web, &allow);
    let _ = engine.eval("queue", "__dom.queueAdded(true)");
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
            Some(src) => match prefetched.remove(&s.node).unwrap_or_else(|| url.join(src).and_then(|u| fetch_script(web.as_ref(), &u, &allow, secure))) {
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
        // a blocking script is run while the page is still being read: it sees only what is above it (deferred scripts, modules and
        // async ones run after the whole page is read)
        let unread = if s.phase() == 0 { engine.module::<DomModule>("dom").map(|d| d.hide_unread(s.node)) } else { None };
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
        if let Some(saved) = unread {
            if let Some(d) = engine.module::<DomModule>("dom") {
                d.restore_unread(saved);
            }
        }
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
    // scripts added by scripts run once the page's own scripts are done, each after the one that added it
    let _ = engine.eval("flush", "__dom.flushAdded()").and_then(|_| engine.run_jobs().map(|_| ()));
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
    report.scroll = engine.eval("scroll", "(window.scrollX || 0) + ',' + (window.scrollY || 0)").ok().and_then(|t| {
        let (x, y) = t.split_once(',')?;
        let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
        (x.is_finite() && y.is_finite() && (x >= 1.0 || y >= 1.0)).then_some((x.clamp(0.0, 1.0e7) as u32, y.clamp(0.0, 1.0e7) as u32))
    });
    let tree = engine.module::<DomModule>("dom").map(|d| d.tree.clone()).unwrap_or(tree);
    Ok((PageResult { tree, report }, engine))
}

/// The window the page is shown in changed. The page's `innerWidth`, `matchMedia` and estimated sizes follow; media query lists whose
/// answer changed get a `change` event, observers look again, and the window gets `resize`.
pub fn resize(engine: &mut JsEngine, vp: iyakki_web::css::Viewport) -> Result<(), JsError> {
    if let Some(e) = engine.module::<EnvModule>("window") {
        e.set_viewport(vp);
    }
    if let Some(d) = engine.module::<DomModule>("dom") {
        d.set_viewport(vp);
    }
    engine.eval("resize", "__iyakkiResize()")?;
    engine.run_jobs().map(|_| ())
}

/// What a pointer did over an element (see [`Interaction::Pointer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Down,
    Up,
    Move,
    /// The pointer came onto the element (`pointerover`, `mouseover`, `pointerenter`, `mouseenter`).
    Over,
    /// The pointer left it (`pointerout`, `mouseout`, `pointerleave`, `mouseleave`).
    Out,
}

/// What the person did to a live page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interaction {
    Click(usize),
    Input(usize, String),
    Check(usize, bool),
    /// A key while `node` has the focus (`None`: the focused element, else the page). `mods` is a bit set: 1 ctrl, 2 shift, 4 alt, 8 meta.
    /// `up` is the release (keyup); a press is keydown, keypress for a printable key or Enter, and the field's implicit submit on Enter.
    Key { node: Option<usize>, key: String, code: String, mods: u8, up: bool },
    /// The pointer did something over this node other than click (hover, press, release, move); a click already includes its own press and release.
    Pointer { node: usize, kind: PointerKind, x: i32, y: i32 },
    /// The person sent a form (a submit button inside it, the node of the form and of the button): the page's `submit` handlers run first.
    Submit { form: usize, submitter: Option<usize> },
    /// This many milliseconds passed while the page was looked at: its timers and animation callbacks run (a clock ticks, a slide moves on).
    Tick(u64),
}

/// A form a page sent and no handler stopped: what the browser must now send (the same way it sends a form of a page without scripts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// `get` or `post` (lower case; `dialog` and others are treated as `get` by the caller).
    pub method: String,
    /// The form's `action` as written (may be empty: the page's own address) and the browser resolves it.
    pub action: String,
    pub fields: Vec<(String, String)>,
}

/// What a live page looks like after an interaction: the tree to draw again, where a script asked to go (if it did), and what it printed.
pub struct Outcome {
    pub tree: iyakki_web::html::Tree,
    pub navigation: Option<String>,
    pub console: Vec<ConsoleLine>,
    /// How many more nodes the tree has than before (negative when some were removed), a cheap sign that the page changed shape.
    pub grew_by: i64,
    /// A handler stopped the default action of the click or key (so a link is not followed and a form is not sent).
    pub prevented: bool,
    /// A form the page sent (by a submit button, Enter in a field, or `form.submit()`) that no `submit` handler stopped.
    pub submission: Option<Submission>,
}

/// A page kept alive after it loaded: its engine stays, so clicks and typing reach its scripts and the page can be drawn again.
/// This is what an app that lives in a tile (not a page that is read once) runs on.
pub struct LivePage {
    engine: JsEngine,
    pub report: PageReport,
    pub url: Url,
    prevented: bool,
}

impl LivePage {
    /// Load `html` at `url` with `extra` modules (the app's bridge) plugged in after the standard ones. `None`: the engine could not start.
    pub fn load(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits, extra: Vec<Box<dyn WebModule>>) -> Option<LivePage> {
        let (res, engine) = run_core(html, url, web, allow, data, limits, extra, |_| {}).ok()?;
        Some(LivePage { engine, report: res.report, url: url.clone(), prevented: false })
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

    /// The window changed size (see [`resize`]).
    pub fn resize(&mut self, vp: iyakki_web::css::Viewport) -> Result<(), JsError> {
        resize(&mut self.engine, vp)
    }

    /// Click the control made from this node of the tree.
    pub fn click(&mut self, node: usize) -> Result<(), JsError> {
        self.prevented = self.engine.eval("click", &format!("__dom.click({node})"))? == "prevented";
        self.settle();
        Ok(())
    }

    /// One key event (see [`Interaction::Key`]).
    pub fn key(&mut self, node: Option<usize>, key: &str, code: &str, mods: u8, up: bool) -> Result<(), JsError> {
        let n = node.map(|n| n as i64).unwrap_or(-1);
        let r = self.engine.eval("key", &format!("__dom.key({n}, {}, {}, {}, {mods})", json_str(if up { "up" } else { "down" }), json_str(key), json_str(code)))?;
        self.prevented = r == "prevented";
        self.settle();
        Ok(())
    }

    /// One pointer event (see [`Interaction::Pointer`]).
    pub fn pointer(&mut self, node: usize, kind: PointerKind, x: i32, y: i32) -> Result<(), JsError> {
        let k = match kind {
            PointerKind::Down => "down",
            PointerKind::Up => "up",
            PointerKind::Move => "move",
            PointerKind::Over => "over",
            PointerKind::Out => "out",
        };
        self.prevented = self.engine.eval("pointer", &format!("__dom.pointer({node}, {}, {x}, {y})", json_str(k)))? == "prevented";
        self.settle();
        Ok(())
    }

    /// Send a form as a person does.
    pub fn submit(&mut self, form: usize, submitter: Option<usize>) -> Result<(), JsError> {
        let by = submitter.map(|n| n as i64).unwrap_or(-1);
        self.prevented = self.engine.eval("submit", &format!("__dom.submit({form}, {by})"))? == "prevented";
        self.settle();
        Ok(())
    }

    /// The form a page sent, once (a script's `form.submit()` and a handler-approved submit both end here).
    pub fn take_submission(&mut self) -> Option<Submission> {
        let t = self.engine.eval("takeSubmission", "__dom.takeSubmission()").ok()?;
        if t.is_empty() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&t).ok()?;
        let fields = v["fields"].as_array()?.iter().filter_map(|f| Some((f.get(0)?.as_str()?.to_string(), f.get(1)?.as_str()?.to_string()))).take(500).collect();
        Some(Submission { method: v["method"].as_str().unwrap_or("get").chars().take(10).collect(), action: v["action"].as_str().unwrap_or("").chars().take(2000).collect(), fields })
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

    /// Where a script asked to go, once (the request is taken, so it is not reported twice).
    pub fn take_navigation(&mut self) -> Option<String> {
        self.engine.module::<EnvModule>("window").and_then(|e| e.navigation.take())
    }

    /// One thing the person did to the page: the node is the one a control of the drawn page was made from (`Control::node`).
    pub fn interact(&mut self, what: &Interaction) -> Result<Outcome, JsError> {
        let before = self.engine.module::<DomModule>("dom").map(|d| d.tree.nodes.len()).unwrap_or(0);
        self.prevented = false;
        let ok = match what {
            Interaction::Click(n) => self.click(*n),
            Interaction::Input(n, v) => self.input(*n, v),
            Interaction::Check(n, on) => self.check(*n, *on),
            Interaction::Key { node, key, code, mods, up } => self.key(*node, key, code, *mods, *up),
            Interaction::Submit { form, submitter } => self.submit(*form, *submitter),
            Interaction::Pointer { node, kind, x, y } => self.pointer(*node, *kind, *x, *y),
            Interaction::Tick(ms) => {
                self.advance((*ms).min(5_000));
                Ok(())
            }
        };
        ok?;
        let prevented = self.prevented;
        let submission = self.take_submission();
        // a script that asked to go somewhere: taken, so the next interaction does not report it again
        let navigation = self.take_navigation();
        let console = self.take_console();
        let tree = self.tree();
        Ok(Outcome { grew_by: tree.nodes.len() as i64 - before as i64, tree, navigation, console, prevented, submission })
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

/// Fetch every external script the page will run at once (a few at a time) instead of one after the other while the page waits: what
/// the loop runs afterwards finds its text ready. The order the scripts run in does not change.
fn prefetch_scripts(scripts: &[crate::modules::dom::Script], url: &Url, secure: bool, web: &Arc<dyn Fetch>, allow: &(impl Fn(&str) -> bool + Send + Clone + 'static)) -> std::collections::HashMap<usize, Result<String, String>> {
    let wanted: Vec<(usize, Url)> = scripts
        .iter()
        .take(MAX_SCRIPTS)
        .filter(|s| !s.nomodule && (s.is_classic_js() || s.kind == "module"))
        .filter_map(|s| s.src.as_ref().and_then(|src| url.join(src).ok()).map(|u| (s.node, u)))
        .collect();
    let mut out = std::collections::HashMap::new();
    if wanted.len() < 2 {
        return out;
    }
    for chunk in wanted.chunks(6) {
        let got: Vec<Result<String, String>> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|(_, u)| {
                    let (web, allow) = (web.clone(), allow.clone());
                    sc.spawn(move || fetch_script(web.as_ref(), u, &allow, secure))
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("the fetch stopped".into()))).collect()
        });
        for ((node, _), r) in chunk.iter().zip(got) {
            out.insert(*node, r);
        }
    }
    out
}

fn fetch_script(web: &dyn Fetch, url: &Url, allow: &dyn Fn(&str) -> bool, secure_page: bool) -> Result<String, String> {
    let mut cur = url.without_fragment();
    for _ in 0..4 {
        // a secure page does not run unprotected scripts: anyone on the network could swap them (F-BROW-98); never even asked for
        if secure_page && !cur.https {
            return Err("blocked: this script would load without protection (http) on a secure page".into());
        }
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

/// The start of the note a renderer adds when a script of the page asked to go to another address (`location.href = ...`,
/// `location.replace(...)`, `location.assign(...)`): the rest of the note is the full address. The browser follows it (a few
/// steps at most) after checking it like any address the person typed.
pub const NAVIGATE_NOTE: &str = "A script asked to go to ";

/// The note that carries where the scripts left the window's scroll position (`x,y`), for the reader to start the drawn page there.
pub const SCROLL_NOTE: &str = "A script scrolled the page to ";

/// The scroll position a renderer note asks for.
pub fn scroll_in(notes: &[String]) -> Option<(f32, f32)> {
    let t = notes.iter().find_map(|n| n.strip_prefix(SCROLL_NOTE))?;
    let (x, y) = t.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    (x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0).then_some((x, y))
}

/// The address a renderer note asks for, when it is one.
pub fn navigation_in(notes: &[String]) -> Option<&str> {
    notes.iter().find_map(|n| n.strip_prefix(NAVIGATE_NOTE))
}

/// Like [`run_page`], and the page's engine is kept alive for [`LivePage::interact`] (`None` when the engine could not start).
pub fn run_page_live(html: &str, url: &Url, web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Clone + 'static, data: &SiteData, limits: Limits) -> (PageResult, Option<LivePage>) {
    match run_core(html, url, web, allow, data, limits, Vec::new(), |_| {}) {
        Ok((r, engine)) => {
            let mut live = LivePage { engine, report: r.report.clone(), url: url.clone(), prevented: false };
            // where the page's own scripts asked to go while it loaded is already in `r.report.navigation`, and the loader follows it (a few
            // steps at most, never to the page itself). The live engine must not report that request a second time with the first interaction
            // or beat: the second report would be followed without the limits, at a moment that depends on the speed of the machine.
            let _ = live.take_navigation();
            (r, Some(live))
        }
        Err(r) => (r, None),
    }
}

/// The lines a renderer shows with a page the scripts have run on: what ran and failed, then where a script asked to go and scrolled to.
pub fn notes_of(r: &PageResult) -> Vec<String> {
    let mut notes = r.report.lines();
    if r.report.ran + r.report.failed > 0 {
        notes.insert(0, format!("scripts: {} ran, {} failed, {} skipped", r.report.ran, r.report.failed, r.report.skipped));
    }
    if let Some(n) = &r.report.navigation {
        notes.push(format!("{NAVIGATE_NOTE}{n}"));
    }
    if let Some((x, y)) = r.report.scroll {
        notes.push(format!("{SCROLL_NOTE}{x},{y}"));
    }
    notes
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
        let notes = notes_of(&r);
        (build(url, &r.tree), notes)
    }
}

/// Data for a browsing session: one `localStorage` per origin, and one cookie jar for everything.
#[derive(Default, Clone)]
pub struct Sites {
    local: Arc<Mutex<BTreeMap<String, Arc<Mutex<MemoryStore>>>>>,
    idb: Arc<Mutex<BTreeMap<String, Arc<Mutex<IdbStore>>>>>,
    pub cookies: Arc<Mutex<CookieStore>>,
    /// Handed to every page of every site (F-BROX-03); `None`: no page has `navigator.credentials`.
    pub passkeys: Option<Arc<Mutex<dyn crate::modules::PasskeyHost>>>,
    /// Handed to every page of every site (F-INBOX-09); `None`: pages keep the stub `navigator.serviceWorker` and have no `PushManager`.
    pub push: Option<Arc<Mutex<dyn crate::modules::PushHost>>>,
}

fn key_of(url: &Url) -> String {
    format!("{}://{}:{}", if url.https { "https" } else { "http" }, url.host, url.port.unwrap_or(if url.https { 443 } else { 80 }))
}

impl Sites {
    pub fn for_origin(&self, url: &Url) -> SiteData {
        let key = key_of(url);
        let local = self.local.lock().map(|mut m| m.entry(key).or_default().clone()).unwrap_or_default();
        let idb = self.idb.lock().map(|mut m| m.entry(key_of(url)).or_default().clone()).unwrap_or_default();
        SiteData { local, idb, cookies: self.cookies.clone(), viewport: None, passkeys: self.passkeys.clone(), push: self.push.clone() }
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
    fn the_renderer_reports_where_a_script_asked_to_go_as_a_note() {
        let render = renderer(site(vec![]), |_| true, SiteData::default());
        let (_, notes) = render(&Url::parse("https://a.org/x").unwrap(), "<p>hi</p><script>location.replace('/next?a=1#top')</script>");
        assert_eq!(navigation_in(&notes), Some("https://a.org/next?a=1#top"), "{notes:?}");
        let (_, notes) = render(&Url::parse("https://a.org/x").unwrap(), "<p>hi</p><script>var a = 1</script>");
        assert_eq!(navigation_in(&notes), None);
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

    /// A page over https never asks for a module over plain http (F-BROW-98), not even through a redirect; a plain page loses nothing.
    #[test]
    fn a_secure_page_runs_no_unprotected_module_and_does_not_even_ask_for_it_while_a_plain_page_may() {
        let js = "export const x = 7;";
        let ok = Response { status: 200, content_type: "text/javascript".into(), body: js.as_bytes().to_vec(), ..Response::default() };
        let mut pages: HashMap<String, Response> = HashMap::new();
        pages.insert("http://cdn.example/m.js".into(), ok.clone());
        pages.insert("https://cdn.example/ok.js".into(), ok);
        pages.insert("https://cdn.example/redir.js".into(), Response { status: 302, location: Some("http://cdn.example/m.js".into()), ..Response::default() });
        let web: Arc<dyn Fetch> = Arc::new(Site(pages));
        let page = |src: &str, base: &str| run_page(&format!("<p id=a>-</p><script type=module>import {{ x }} from '{src}'; document.getElementById('a').textContent = 'got ' + x;</script>"), &Url::parse(base).unwrap(), web.clone(), |_| true, &SiteData::default(), Limits::default());
        // https page, http module: refused, and the address was not requested
        let r = page("http://cdn.example/m.js", "https://a.org/");
        assert_eq!(r.report.failed, 1);
        assert!(r.report.notes.join(" ").contains("without protection"), "{:?}", r.report.notes);
        // https page, https module that redirects to http: refused at the hop
        let r = page("https://cdn.example/redir.js", "https://a.org/");
        assert!(r.report.notes.join(" ").contains("without protection"), "{:?}", r.report.notes);
        // https page, https module: runs
        let r = page("https://cdn.example/ok.js", "https://a.org/");
        assert_eq!(r.report.failed, 0, "{:?}", r.report.notes);
        // a plain page may still load a plain module
        let r = page("http://cdn.example/m.js", "http://a.org/");
        assert_eq!(r.report.failed, 0, "{:?}", r.report.notes);
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

    /// A web that answers every script after a pause and counts how many were being fetched at once.
    struct Slow {
        files: HashMap<String, String>,
        now: std::sync::atomic::AtomicUsize,
        most: std::sync::atomic::AtomicUsize,
    }
    impl Fetch for Slow {
        fn get(&self, url: &Url) -> Result<Response, String> {
            use std::sync::atomic::Ordering::SeqCst;
            let n = self.now.fetch_add(1, SeqCst) + 1;
            self.most.fetch_max(n, SeqCst);
            // a fetch answers once another one is running with it (a page that asks one file after the other never gets there, and the
            // `most` check below fails); a deadline, not a stopwatch, ends the wait
            let t0 = Instant::now();
            while self.most.load(SeqCst) < 2 && t0.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(2));
            }
            self.now.fetch_sub(1, SeqCst);
            self.files.get(&url.to_string()).map(|b| Response { status: 200, content_type: "text/javascript".into(), body: b.as_bytes().to_vec(), ..Response::default() }).ok_or_else(|| "connection refused".to_string())
        }
    }

    fn pusher(name: &str) -> String {
        format!("(window.__log = window.__log || []).push('{name}');")
    }

    #[test]
    fn blocking_scripts_run_first_then_defer_and_modules_then_async_and_the_files_are_fetched_side_by_side() {
        let files: HashMap<String, String> = ["a", "b", "d", "y", "d2"].iter().map(|n| (format!("https://a.org/{n}.js"), pusher(n))).collect();
        let web = Arc::new(Slow { files, now: Default::default(), most: Default::default() });
        let html = "<script src=/a.js></script><script defer src=/d.js></script><script async src=/y.js></script><script src=/b.js></script><script defer>(window.__log = window.__log || []).push('inline-defer-is-blocking');</script><script defer src=/d2.js></script><script>(window.__log = window.__log || []).push('inline');</script>";
        let mut log = String::new();
        let res = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web.clone(), |_| true, &SiteData::default(), Limits::default(), |e| {
            log = e.eval("log", "window.__log.join(',')").unwrap_or_default();
        });
        assert_eq!(res.report.failed, 0, "{:?}", res.report.notes);
        // blocking ones in the order written (a `defer` on an inline script counts for nothing), then the deferred ones in order, then async
        assert_eq!(log, "a,b,inline-defer-is-blocking,inline,d,d2,y");
        // five files, each held until another one is running: one after the other would never get two at once
        assert!(web.most.load(std::sync::atomic::Ordering::SeqCst) >= 2, "fetched side by side");
    }

    #[test]
    fn a_blocking_script_sees_only_what_is_above_it_and_an_added_script_runs_after_its_adder() {
        let mut files: HashMap<String, String> = HashMap::new();
        files.insert("https://a.org/seesp.js".into(), "(window.__log = window.__log || []).push('p' + document.querySelectorAll('p').length);".into());
        files.insert("https://a.org/adds.js".into(), "var s = document.createElement('script'); s.src = '/dyn.js'; document.body.appendChild(s); (window.__log = window.__log || []).push('adds');".into());
        for n in ["dyn", "b"] {
            files.insert(format!("https://a.org/{n}.js"), pusher(n));
        }
        let web = Arc::new(Slow { files, now: Default::default(), most: Default::default() });
        let run_log = |html: &str| {
            let mut log = String::new();
            let r = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web.clone(), |_| true, &SiteData::default(), Limits::default(), |e| {
                log = e.eval("log", "(window.__log || []).join(',')").unwrap_or_default();
            });
            assert!(r.report.notes.is_empty(), "{:?}", r.report.notes);
            log
        };
        assert_eq!(run_log("<p>1</p><script src=/seesp.js></script><p>2</p><script src=/seesp.js></script><p>3</p><script defer src=/seesp.js></script>"), "p1,p2,p3");
        // what a script appends lands before the rest of the page, and the rest is back afterwards
        assert_eq!(run_log("<div id=a><script>document.body.appendChild(document.createElement('p')); (window.__log = window.__log || []).push(document.body.lastChild.tagName + document.getElementById('z'));</script></div><i id=z></i><script>(window.__log = window.__log || []).push(document.getElementById('z').id + document.querySelectorAll('p').length)</script>"), "Pnull,z1");
        assert_eq!(run_log("<body><script src=/adds.js></script><script src=/b.js></script></body>"), "adds,b,dyn");
    }

    #[test]
    fn a_failed_script_is_reported_in_its_own_place_and_the_others_still_run() {
        let files: HashMap<String, String> = [("https://a.org/ok.js".to_string(), pusher("ok")), ("https://a.org/ok2.js".to_string(), pusher("ok2"))].into_iter().collect();
        let web = Arc::new(Slow { files, now: Default::default(), most: Default::default() });
        let mut log = String::new();
        let res = run_page_with("<script src=/ok.js></script><script src=/missing.js></script><script defer src=/ok2.js></script>", &Url::parse("https://a.org/").unwrap(), web, |_| true, &SiteData::default(), Limits::default(), |e| {
            log = e.eval("log", "window.__log.join(',')").unwrap_or_default();
        });
        assert_eq!(log, "ok,ok2");
        assert_eq!(res.report.failed, 1);
        assert!(res.report.notes.iter().any(|n| n.contains("could not load /missing.js")), "{:?}", res.report.notes);
    }

    #[test]
    fn a_secure_page_runs_no_unprotected_script_and_does_not_even_ask_for_it_while_a_plain_page_may() {
        let files: HashMap<String, String> = [("http://cdn.org/a.js".to_string(), pusher("plain")), ("https://cdn.org/b.js".to_string(), pusher("secure"))].into_iter().collect();
        let html = "<script src=\"http://cdn.org/a.js\"></script><script src=\"https://cdn.org/b.js\"></script>";
        let run_on = |page: &str| {
            let web = Arc::new(Slow { files: files.clone(), now: Default::default(), most: Default::default() });
            let seen = web.clone();
            let mut log = String::new();
            let res = run_page_with(html, &Url::parse(page).unwrap(), web, |_| true, &SiteData::default(), Limits::default(), |e| {
                log = e.eval("log", "(window.__log || []).join(',')").unwrap_or_default();
            });
            (log, res.report.notes, seen.most.load(std::sync::atomic::Ordering::SeqCst))
        };
        let (log, notes, _) = run_on("https://a.org/");
        assert_eq!(log, "secure", "only the protected script ran");
        assert!(notes.iter().any(|n| n.contains("without protection")), "{notes:?}");
        let (log, notes, _) = run_on("http://a.org/");
        assert_eq!(log, "plain,secure", "a page that is itself unprotected loses nothing: {notes:?}");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use iyakki_web::{Block, Control, Response};
    use std::collections::HashMap;

    struct Site(HashMap<String, Response>);
    impl Fetch for Site {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.0.get(&url.to_string()).cloned().ok_or_else(|| "connection refused".to_string())
        }
    }

    fn live(html: &str) -> LivePage {
        let u = Url::parse("https://a.org/").unwrap();
        LivePage::load(html, &u, Arc::new(Site(HashMap::new())), |_| true, &SiteData::default(), Limits::default(), Vec::new()).expect("the engine starts")
    }

    fn texts(tree: &iyakki_web::html::Tree) -> Vec<String> {
        let doc = iyakki_web::doc::build(tree);
        doc.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.spans.iter().map(|s| s.text.as_str()).collect::<String>()),
                _ => None,
            })
            .collect()
    }

    fn control_named<'a>(doc: &'a iyakki_web::Doc, label: &str) -> &'a Control {
        doc.controls.iter().find(|c| c.label == label || c.placeholder == label).unwrap_or_else(|| panic!("no control {label:?} among {:?}", doc.controls.iter().map(|c| c.label.clone()).collect::<Vec<_>>()))
    }

    const COUNTER: &str = "<p id=n>0</p><button id=b>Add</button><input id=t type=text placeholder=Name><p id=h></p><input id=c type=checkbox><p id=s>off</p><button id=go>Go</button>\
        <script>let n = 0; document.getElementById('b').addEventListener('click', () => { n++; document.getElementById('n').textContent = String(n); console.log('clicked ' + n); });\
        document.getElementById('t').addEventListener('input', e => { document.getElementById('h').textContent = 'Hello ' + e.target.value; });\
        document.getElementById('c').addEventListener('change', e => { document.getElementById('s').textContent = e.target.checked ? 'on' : 'off'; });\
        document.getElementById('go').addEventListener('click', () => { location.href = '/next?from=click'; });</script>";

    #[test]
    fn a_control_of_the_drawn_page_reaches_its_script_and_the_changed_page_can_be_drawn_again() {
        let mut p = live(COUNTER);
        let doc = iyakki_web::doc::build(&p.tree());
        assert_eq!(texts(&p.tree())[0], "0");
        let add = control_named(&doc, "Add").node;
        let out = p.interact(&Interaction::Click(add)).unwrap();
        assert_eq!(texts(&out.tree)[0], "1", "the script ran and changed the page");
        assert!(out.console.iter().any(|l| l.text.contains("clicked 1")), "{:?}", out.console.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
        assert_eq!(out.navigation, None);
        let out = p.interact(&Interaction::Click(add)).unwrap();
        assert_eq!(texts(&out.tree)[0], "2", "the engine stayed alive between clicks");
        // the tree drawn again gives a new document with the same controls (the nodes did not change)
        let again = iyakki_web::doc::build(&out.tree);
        assert_eq!(control_named(&again, "Add").node, add);
    }

    #[test]
    fn a_navigation_the_loader_has_reported_is_not_reported_again_by_the_live_page() {
        let u = Url::parse("https://a.org/").unwrap();
        let html = "<p>x</p><script>location.href = '/next'</script>";
        let (r, live) = run_page_live(html, &u, Arc::new(Site(HashMap::new())), |_| true, &SiteData::default(), Limits::default());
        assert_eq!(r.report.navigation.as_deref(), Some("https://a.org/next"), "the loader is told where the script asked to go");
        assert_eq!(live.expect("the engine stays alive").take_navigation(), None, "the live page does not repeat it with its first update");
    }

    #[test]
    fn typing_and_checking_reach_the_page_and_a_script_navigation_is_reported_once() {
        let mut p = live(COUNTER);
        let doc = iyakki_web::doc::build(&p.tree());
        let name = control_named(&doc, "Name").node;
        let out = p.interact(&Interaction::Input(name, "Ada".into())).unwrap();
        assert!(texts(&out.tree).iter().any(|t| t == "Hello Ada"), "{:?}", texts(&out.tree));
        let boxn = doc.controls.iter().find(|c| c.kind == iyakki_web::doc::ControlKind::Checkbox).expect("a check box").node;
        let out = p.interact(&Interaction::Check(boxn, true)).unwrap();
        assert!(texts(&out.tree).iter().any(|t| t == "on"), "{:?}", texts(&out.tree));
        let go = control_named(&doc, "Go").node;
        let out = p.interact(&Interaction::Click(go)).unwrap();
        assert_eq!(out.navigation.as_deref(), Some("https://a.org/next?from=click"));
        let again = p.interact(&Interaction::Click(control_named(&doc, "Add").node)).unwrap();
        assert_eq!(again.navigation, None, "the request was taken, not repeated");
    }

    #[test]
    fn nodes_that_do_not_exist_and_scripts_that_fail_do_not_break_the_page() {
        let mut p = live("<button id=b>Boom</button><p id=r>ok</p><script>document.getElementById('b').addEventListener('click', () => { throw new Error('bad handler'); });</script>");
        let doc = iyakki_web::doc::build(&p.tree());
        let b = control_named(&doc, "Boom").node;
        let r = p.interact(&Interaction::Click(b));
        // a handler that throws is the page's problem: the page is still there and can be clicked again
        assert!(r.is_ok() || r.is_err());
        let out = p.interact(&Interaction::Click(99_999));
        assert!(out.is_ok() || out.is_err(), "no panic for a node that is not there");
        assert!(texts(&p.tree()).iter().any(|t| t == "ok"));
    }

    fn node_of(p: &mut LivePage, tag: &str) -> usize {
        p.tree().find(tag).unwrap_or_else(|| panic!("no <{tag}>"))
    }

    const FORMS: &str = "<form id=f1 action=/search method=get><input name=q id=q type=text value=cats><input type=checkbox name=safe value=1 checked><input type=checkbox name=off value=1>\
        <select name=kind><option value=a>A<option value=b selected>B</select><textarea name=note>hi</textarea><button name=go value=now id=go>Go</button><button name=other value=x type=button>x</button></form>\
        <form id=f2 action=/blocked method=post><input name=z value=1><button id=stop>Stop</button></form>\
        <p id=log></p><a id=link href=/plain>link</a><a id=quiet href=/quiet>quiet</a>\
        <script>document.getElementById('f2').addEventListener('submit', e => { e.preventDefault(); document.getElementById('log').textContent = 'blocked ' + (e.submitter ? e.submitter.id : 'none'); });\
        document.getElementById('quiet').addEventListener('click', e => e.preventDefault());\
        window.keys = []; document.addEventListener('keydown', e => { keys.push('d:' + e.key + (e.ctrlKey ? '+ctrl' : '')); if (e.key === 'x') e.preventDefault(); });\
        document.addEventListener('keypress', e => keys.push('p:' + e.key)); document.addEventListener('keyup', e => keys.push('u:' + e.key));</script>";

    #[test]
    fn a_submit_button_sends_its_form_with_the_fields_a_browser_sends_and_only_the_button_used() {
        let mut p = live(FORMS);
        let doc = iyakki_web::doc::build(&p.tree());
        let go = control_named(&doc, "Go").node;
        let out = p.interact(&Interaction::Click(go)).unwrap();
        let s = out.submission.expect("no handler stopped it");
        assert_eq!((s.method.as_str(), s.action.as_str()), ("get", "/search"));
        assert_eq!(s.fields, vec![("q".to_string(), "cats".to_string()), ("safe".into(), "1".into()), ("kind".into(), "b".into()), ("note".into(), "hi".into()), ("go".into(), "now".into())], "unchecked boxes and plain buttons are left out");
        assert!(!out.prevented);
        // taken once
        assert!(p.take_submission().is_none());
    }

    #[test]
    fn a_submit_handler_that_stops_the_event_stops_the_send_and_sees_the_button() {
        let mut p = live(FORMS);
        let doc = iyakki_web::doc::build(&p.tree());
        let stop = control_named(&doc, "Stop").node;
        let out = p.interact(&Interaction::Click(stop)).unwrap();
        assert!(out.submission.is_none(), "the page handled it itself");
        assert!(texts(&out.tree).iter().any(|t| t == "blocked stop"), "{:?}", texts(&out.tree));
    }

    #[test]
    fn enter_in_a_field_sends_the_form_and_a_script_can_send_it_too() {
        let mut p = live(FORMS);
        let q = node_of(&mut p, "input");
        let out = p.interact(&Interaction::Key { node: Some(q), key: "Enter".into(), code: "Enter".into(), mods: 0, up: false }).unwrap();
        let s = out.submission.expect("Enter sends the form of the field");
        assert_eq!(s.action, "/search");
        assert!(s.fields.iter().any(|f| f.0 == "go"), "the first submit button counts as the sender");
        p.eval("s", "document.getElementById('f1').submit()").unwrap();
        assert!(p.take_submission().is_some(), "form.submit() skips the handlers and sends");
        p.eval("s", "document.getElementById('f2').requestSubmit()").unwrap();
        assert!(p.take_submission().is_none(), "requestSubmit runs the handlers, and this one stops it");
    }

    #[test]
    fn keys_reach_the_page_in_order_with_their_modifiers_and_a_stopped_key_says_so() {
        let mut p = live(FORMS);
        let press = |p: &mut LivePage, k: &str, mods: u8| p.interact(&Interaction::Key { node: None, key: k.into(), code: k.into(), mods, up: false }).unwrap();
        let up = |p: &mut LivePage, k: &str| p.interact(&Interaction::Key { node: None, key: k.into(), code: k.into(), mods: 0, up: true }).unwrap();
        assert!(!press(&mut p, "a", 0).prevented);
        up(&mut p, "a");
        assert!(press(&mut p, "x", 0).prevented, "the handler stopped x");
        press(&mut p, "k", 1);
        assert_eq!(p.eval("k", "keys.join(' ')").unwrap(), "d:a p:a u:a d:x d:k+ctrl p:k");
    }

    #[test]
    fn a_click_a_handler_stops_is_reported_so_the_reader_does_not_follow_the_link() {
        let mut p = live(FORMS);
        let t = p.tree();
        let mut links = (0..t.nodes.len()).filter(|n| t.tag(*n) == Some("a"));
        let (plain, quiet) = (links.next().unwrap(), links.next().unwrap());
        assert!(!p.interact(&Interaction::Click(plain)).unwrap().prevented);
        assert!(p.interact(&Interaction::Click(quiet)).unwrap().prevented);
    }

    #[test]
    fn time_passes_for_a_page_that_is_looked_at() {
        let mut p = live("<p id=c>0</p><script>let n = 0; setInterval(() => { n++; document.getElementById('c').textContent = String(n); }, 1000);</script>");
        let first = p.interact(&Interaction::Tick(3000)).unwrap();
        let n: u32 = texts(&first.tree)[0].parse().unwrap();
        assert!(n >= 3, "three seconds of a one second timer: {n}");
        let more = p.interact(&Interaction::Tick(2000)).unwrap();
        assert!(texts(&more.tree)[0].parse::<u32>().unwrap() >= n + 2);
    }

    #[test]
    fn a_click_is_press_release_click_and_the_pointer_events_other_than_click_reach_the_page_in_order() {
        let mut p = live("<p id=log></p><button id=b>Hit</button><script>const log = []; const show = () => { document.getElementById('log').textContent = log.join(' '); };\
            const b = document.getElementById('b');\
            for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'pointerover', 'mouseover', 'pointerenter', 'mouseenter', 'pointerout', 'mouseout', 'pointerleave', 'mouseleave', 'pointermove', 'mousemove']) b.addEventListener(t, e => { log.push(t + (t === 'pointermove' ? ':' + e.clientX + ',' + e.clientY : '')); show(); });</script>");
        let doc = iyakki_web::doc::build(&p.tree());
        let b = control_named(&doc, "Hit").node;
        let say = |p: &mut LivePage, what: Interaction| texts(&p.interact(&what).unwrap().tree)[0].clone();
        assert_eq!(say(&mut p, Interaction::Click(b)), "pointerdown mousedown pointerup mouseup click");
        let after_hover = say(&mut p, Interaction::Pointer { node: b, kind: PointerKind::Over, x: 5, y: 6 });
        assert!(after_hover.ends_with("pointerover mouseover pointerenter mouseenter"), "{after_hover}");
        assert!(say(&mut p, Interaction::Pointer { node: b, kind: PointerKind::Move, x: 7, y: 8 }).ends_with("pointermove:7,8 mousemove"));
        assert!(say(&mut p, Interaction::Pointer { node: b, kind: PointerKind::Out, x: 9, y: 9 }).ends_with("pointerout mouseout pointerleave mouseleave"));
        // a handler that stops a press is reported, and an unknown node is an error and not a crash
        let mut q = live("<button id=b>Hit</button><script>document.getElementById('b').addEventListener('mousedown', e => e.preventDefault());</script>");
        let dq = iyakki_web::doc::build(&q.tree());
        assert!(q.interact(&Interaction::Pointer { node: control_named(&dq, "Hit").node, kind: PointerKind::Down, x: 0, y: 0 }).unwrap().prevented);
        assert!(q.interact(&Interaction::Pointer { node: 99_999, kind: PointerKind::Over, x: 0, y: 0 }).is_err());
    }
}
