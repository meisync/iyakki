//! The browser's side of the inspector (F-BROW-41): the script engine behind the Developer tools prompt, and the tree behind the
//! Elements page. Data and rules are in `browser_inspect.rs`; drawing is in `browser_devpanel.rs`.
//!
//! * The engine is started lazily, on the first expression, from the page's own source (`Page::source`) with the same host rules as the
//!   page load had (the account's host policy, cookies as the person set them). It starts on a worker thread, because a page's own
//!   scripts run again when it starts and that can take seconds; expressions typed meanwhile wait for it. It belongs to one page: a
//!   new page forgets it, and so does closing the panel.
//! * Only for pages the built-in reader shows and only where scripts are allowed for the site (Settings -> Browser, or `j`): turning
//!   scripts off for a site is a promise that nothing of the site's runs, and the inspector keeps it.
//! * Each call has a short time limit. What an expression returns is shown to the person and not stored anywhere else; the audit line
//!   names the host only, never the expression or its value.

use std::sync::mpsc::{channel, Receiver, TryRecvError};

use super::browser_inspect::Kind;
use super::*;

/// Defined once per engine: shows a value the way a console does (strings quoted, objects as JSON, functions by their first line).
const PRELUDE: &str = r#"globalThis.__iy_show = function (v) {
  try {
    if (v === undefined) return 'undefined';
    if (typeof v === 'string') return JSON.stringify(v);
    if (typeof v === 'function') return String(v).split('\n')[0];
    if (v !== null && typeof v === 'object') { const j = JSON.stringify(v, (k, x) => (typeof x === 'bigint' ? x + 'n' : x)); return j === undefined ? String(v) : j; }
    return String(v);
  } catch (e) { return String(v); }
};"#;

/// Wall-clock time one expression, or the start of the engine's page scripts, may take.
const CALL_WALL: std::time::Duration = std::time::Duration::from_secs(5);

/// The engine and what goes with it, per page.
#[derive(Default)]
pub(super) struct InspectHost {
    /// The page the engine (or the start in progress) belongs to.
    pub(super) page: Option<String>,
    pub(super) live: Option<iyakki_js::LivePage>,
    pub(super) boot: Option<Receiver<Option<iyakki_js::LivePage>>>,
    /// Expressions typed while the engine starts.
    pub(super) queue: Vec<String>,
    /// The page whose tree the Elements page shows.
    pub(super) tree_for: Option<String>,
}

impl BrowserApp {
    /// The page the inspector works on: one the reader shows (not a file, not the start page, not the full engine's).
    fn inspect_page(&self) -> Option<Arc<Page>> {
        if self.full.is_some() {
            return None;
        }
        self.page().cloned().filter(|p| !p.download && !p.media && p.ctype != STUB_TYPE && !p.source.is_empty())
    }

    /// Forget the engine (a new page, or the panel closed).
    pub(super) fn inspect_reset(&mut self) {
        let had_lines = !self.dev.insp.lines.is_empty();
        self.inspect = InspectHost::default();
        self.dev.insp.waiting = false;
        self.dev.insp.clear_tree();
        if had_lines && self.dev.open {
            self.dev.insp.push(Kind::Note, "-- a new page: the expressions above ran in the earlier one --");
        }
    }

    /// An expression from the prompt.
    pub(super) fn dev_eval(&mut self, expr: String, cx: &AppCx) {
        let Some(page) = self.inspect_page() else {
            let why = if self.full.is_some() { "The prompt works on pages the built-in reader shows; the full web engine has no safe way to run a script and bring a result back." } else { "Open a page in the built-in reader first." };
            self.dev.insp.push(Kind::Note, why);
            return;
        };
        if !self.cfg.scripts_for(&page.url.host) {
            self.dev.insp.push(Kind::Note, format!("Scripts are off for {}: press j on the page (or turn them on in Settings -> Browser), reload, then try again.", page.url.host));
            return;
        }
        // the host only: never the expression or what it returned
        cx.audit.record(&cx.session.user, "browser.devtools.eval", &page.url.host);
        let key = page.url.to_string();
        if self.inspect.page.as_deref() == Some(key.as_str()) && self.inspect.live.is_some() {
            self.inspect_run(&expr);
            return;
        }
        self.inspect.queue.push(expr);
        if self.inspect.boot.is_some() && self.inspect.page.as_deref() == Some(key.as_str()) {
            return; // already starting
        }
        let (tx, rx) = channel();
        let (html, url) = (page.source.clone(), page.url.clone());
        let web = cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone());
        let policy = cx.session.policy.clone();
        let mut data = self.sites.for_origin(&url);
        if !self.cfg.cookies {
            data.cookies = Arc::new(std::sync::Mutex::new(iyakki_web::CookieStore::new()));
        }
        let limits = iyakki_js::Limits { wall: CALL_WALL, ..iyakki_js::Limits::default() };
        let spawned = std::thread::Builder::new().name("inspector-start".into()).spawn(move || {
            let lp = iyakki_js::LivePage::load(&html, &url, web, move |h: &str| policy.allows_host(h), &data, limits, Vec::new());
            let _ = tx.send(lp);
        });
        match spawned {
            Ok(_) => {
                self.inspect.page = Some(key);
                self.inspect.live = None;
                self.inspect.boot = Some(rx);
                self.dev.insp.waiting = true;
            }
            Err(e) => {
                self.inspect.queue.clear();
                self.dev.insp.push(Kind::Err, format!("could not start the script engine: {e}"));
            }
        }
    }

    /// Run one expression in the live engine and show what came back and what it printed.
    fn inspect_run(&mut self, expr: &str) {
        let Some(lp) = self.inspect.live.as_mut() else { return };
        let quoted = serde_json::to_string(expr).unwrap_or_else(|_| "\"\"".into());
        let r = lp.eval("console", &format!("__iy_show((0, eval)({quoted}))"));
        for l in lp.take_console() {
            let kind = if l.level == "error" { Kind::Err } else { Kind::Log };
            let text = if matches!(l.level.as_str(), "log" | "info" | "debug") { l.text } else { format!("{}: {}", l.level, l.text) };
            self.dev.insp.push(kind, text);
        }
        let tree = lp.tree();
        self.dev.insp.show_result(r.map_err(|e| e.to_string()));
        self.dev.insp.set_tree(tree, true);
        self.inspect.tree_for = self.inspect.page.clone();
    }

    /// Every heartbeat: collect the started engine, run what waited, keep the Elements page on the page that is open.
    pub(super) fn inspect_poll(&mut self, cx: &AppCx) {
        if !self.dev.open {
            return;
        }
        if let Some(rx) = &self.inspect.boot {
            match rx.try_recv() {
                Ok(Some(mut lp)) => {
                    let _ = lp.eval("prelude", PRELUDE);
                    self.inspect.boot = None;
                    self.inspect.live = Some(lp);
                    self.dev.insp.waiting = false;
                    for e in std::mem::take(&mut self.inspect.queue) {
                        self.dev.insp.push(Kind::Note, format!("running: {e}"));
                        self.inspect_run(&e);
                    }
                    self.dev_dirty = true;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    self.inspect.boot = None;
                    self.inspect.queue.clear();
                    self.dev.insp.waiting = false;
                    self.dev.insp.push(Kind::Err, "the page's script engine could not start");
                    self.dev_dirty = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        // a different page is open now: its engine and tree are not this one's
        let now = self.page().map(|p| p.url.to_string());
        if self.inspect.page.is_some() && self.inspect.page != now && self.full.is_none() {
            self.inspect_reset();
            self.dev_dirty = true;
        }
        let shown = self.inspect_page().map(|p| p.url.to_string());
        if self.dev.tab == browser_devpanel::Tab::Elements && self.inspect.tree_for != shown && self.inspect.boot.is_none() {
            self.dev_inspect(cx);
            self.dev_dirty = true;
        }
    }

    /// The Elements page asked for the tree: the live page when the engine runs, else the page as the server sent it.
    pub(super) fn dev_inspect(&mut self, _cx: &AppCx) {
        let Some(page) = self.inspect_page() else {
            self.dev.insp.clear_tree();
            self.inspect.tree_for = None;
            return;
        };
        let key = page.url.to_string();
        if let (Some(lp), true) = (self.inspect.live.as_mut(), self.inspect.page.as_deref() == Some(key.as_str())) {
            let t = lp.tree();
            self.dev.insp.set_tree(t, true);
        } else {
            self.dev.insp.set_tree(iyakki_web::html::parse(&page.source), false);
        }
        self.inspect.tree_for = Some(key);
    }
}
