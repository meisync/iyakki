//! The browser's side of the inspector (F-BROW-41): the script engine behind the Developer tools prompt, and the tree behind the
//! Elements page. Data and rules are in `browser_inspect.rs`; drawing is in `browser_devpanel.rs`.
//!
//! * The engine is started lazily, on the first expression, from the page's own source (`Page::source`) with the same host rules as the
//!   page load had (the account's host policy, cookies as the person set them). It starts on a worker thread, because a page's own
//!   scripts run again when it starts and that can take seconds; expressions typed meanwhile wait for it. It belongs to one page: a
//!   new page forgets it, and so does closing the panel.
//! * Pages the FULL web engine shows are reached through the helper's `Script` message instead (F-BROX-16): the expression is wrapped,
//!   run in the page, and its text answer is shown here, cut at 4000 characters; the audit line still names the host only.
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
    /// Expressions sent to the full engine and not yet answered: (the helper's request id, when it was sent).
    pub(super) full_pending: Vec<(u32, std::time::Instant)>,
    /// The tree asked of the full engine and not yet answered (F-BROX-16): the helper's request id and when it was sent.
    pub(super) full_tree_pending: Option<(u32, std::time::Instant)>,
    /// The address of the page the full engine's tree on the Elements page was read from.
    pub(super) full_tree_url: Option<String>,
}

/// The longest answer from the engine's page that is shown (a hostile page can return anything).
const FULL_ANSWER_CHARS: usize = 4000;
/// How long the answer of the full engine is waited for.
const FULL_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

/// The script the full engine runs for one expression: the expression is passed as a string literal and evaluated indirectly (global scope,
/// as a console does), and the value comes back as text the way the reader's console shows it. Nothing of the person's page is changed.
fn full_console_script(expr: &str) -> String {
    let quoted = serde_json::to_string(expr).unwrap_or_else(|_| "\"\"".into());
    format!("(function(){{/*iy-console*/var s={quoted};var v=(0,eval)(s);{}return __show(v);}})()", FULL_SHOW)
}

const FULL_SHOW: &str = "function __show(v){try{if(v===undefined)return 'undefined';if(typeof v==='string')return JSON.stringify(v);if(typeof v==='function')return String(v).split('\\n')[0];if(v!==null&&typeof v==='object'){var j=JSON.stringify(v,function(k,x){return typeof x==='bigint'?x+'n':x});return j===undefined?String(v):j;}return String(v);}catch(e){return String(v);}}";

/// The script that reads the page tree out of the full engine (F-BROX-16, the Elements page). It returns ONE string: JSON of at most
/// [`FULL_TREE_NODES`] nodes, 14 levels deep, with text cut at 200 characters and only a short list of attributes (never `value`, and no
/// content of `script`, `style`, `textarea` or `select`, so nothing the person typed travels). The page is not changed.
const FULL_TREE_SCRIPT: &str = "(function(){/*iy-tree*/var N=0,MAX=1500,KEEP={id:1,'class':1,name:1,type:1,href:1,src:1,alt:1,title:1,role:1,'aria-label':1};function cut(s,n){s=String(s);return s.length>n?s.slice(0,n):s}function walk(e,d){if(N>=MAX||d>14)return null;N++;if(e.nodeType===3){var t=e.nodeValue;if(!t||!t.trim())return null;return{t:cut(t.replace(/\\s+/g,' '),200)}}if(e.nodeType!==1)return null;var o={n:String(e.tagName).toLowerCase()};var a=[];for(var i=0;i<e.attributes.length&&a.length<12;i++){var at=e.attributes[i];if(KEEP[at.name]===1)a.push([at.name,cut(at.value,200)])}if(a.length)o.a=a;if(/^(script|style|textarea|select|noscript)$/.test(o.n))return o;var c=[];for(var k=e.firstChild;k;k=k.nextSibling){var w=walk(k,d+1);if(w)c.push(w)}if(c.length)o.c=c;return o}var r=walk(document.documentElement,0);return JSON.stringify({r:r,x:N>=MAX})})()";

#[cfg(test)]
pub(super) const FULL_TREE_SCRIPT_FOR_TESTS: &str = FULL_TREE_SCRIPT;

#[cfg(test)]
pub(super) fn full_console_script_for_tests(expr: &str) -> String {
    full_console_script(expr)
}

/// The most nodes of the full engine's tree that are taken in, and the most bytes of its answer.
const FULL_TREE_NODES: usize = 1500;
const FULL_TREE_BYTES: usize = 512 * 1024;

/// The answer of [`FULL_TREE_SCRIPT`] (the helper's JSON text of the string it returned) as the inspector's tree. Everything is checked
/// again here: a hostile page can return anything, so the size, the depth and the length of each text are bounded on this side too.
pub(super) fn tree_from_full_answer(json: &str) -> Option<iyakki_web::html::Tree> {
    use iyakki_web::html::{Node, NodeKind, Tree, NONE};
    if json.len() > FULL_TREE_BYTES + 16 {
        return None;
    }
    let inner: String = serde_json::from_str(json.trim()).ok()?;
    if inner.len() > FULL_TREE_BYTES {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&inner).ok()?;
    let clean = |s: &str, n: usize| -> String { s.chars().filter(|c| !c.is_control()).take(n).collect() };
    let mut t = Tree { nodes: vec![Node { kind: NodeKind::Element { tag: "#root".into(), attrs: vec![] }, children: vec![], parent: NONE }], truncated: v["x"].as_bool().unwrap_or(false), free: vec![], free_set: Default::default(), reused: vec![] };
    let mut todo: Vec<(&serde_json::Value, usize, usize)> = vec![(v.get("r")?, 0, 0)];
    // depth first, children in order: the stack gets them reversed
    while let Some((j, parent, depth)) = todo.pop() {
        if t.nodes.len() > FULL_TREE_NODES || depth > 16 {
            t.truncated = true;
            continue;
        }
        let id = t.nodes.len();
        if let Some(text) = j.get("t").and_then(|x| x.as_str()) {
            t.nodes.push(Node { kind: NodeKind::Text(clean(text, 200)), children: vec![], parent });
        } else if let Some(tag) = j.get("n").and_then(|x| x.as_str()) {
            let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ':').take(32).collect();
            if tag.is_empty() {
                continue;
            }
            let attrs = j.get("a").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|p| Some((clean(p.get(0)?.as_str()?, 40), clean(p.get(1)?.as_str()?, 200)))).take(12).collect()).unwrap_or_default();
            t.nodes.push(Node { kind: NodeKind::Element { tag, attrs }, children: vec![], parent });
            if let Some(c) = j.get("c").and_then(|c| c.as_array()) {
                for k in c.iter().rev() {
                    todo.push((k, id, depth + 1));
                }
            }
        } else {
            continue;
        }
        t.nodes[parent].children.push(id);
    }
    (t.nodes.len() > 1).then_some(t)
}

/// What the engine's JSON text of a string result says: the string itself (the helper answers values as JSON), cut and without control characters.
fn full_answer_text(json: &str) -> String {
    let t = serde_json::from_str::<String>(json.trim()).unwrap_or_else(|_| json.to_string());
    t.chars().filter(|c| !c.is_control() || *c == '\n').take(FULL_ANSWER_CHARS).collect()
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
        if let Some(full) = self.full.as_mut() {
            // the full engine: one `Script` message, the answer comes back through `inspect_poll`
            let host = full.url.split("://").nth(1).and_then(|r| r.split(['/', ':', '?']).next()).unwrap_or("").to_string();
            cx.audit.record(&cx.session.user, "browser.devtools.eval", &host);
            if self.inspect.full_pending.len() >= 8 {
                self.dev.insp.push(Kind::Note, "Eight expressions are already waiting for the page; wait for them.");
                return;
            }
            let id = full.eval(&full_console_script(&expr), 5000);
            self.inspect.full_pending.push((id, std::time::Instant::now()));
            return;
        }
        let Some(page) = self.inspect_page() else {
            self.dev.insp.push(Kind::Note, "Open a page in the built-in reader first.");
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
        if let Some(full) = self.full.as_mut() {
            let mut left = Vec::new();
            for (id, at) in std::mem::take(&mut self.inspect.full_pending) {
                match full.take_script_result(id) {
                    Some((true, json)) => self.dev.insp.show_result(Ok(full_answer_text(&json))),
                    Some((false, msg)) => self.dev.insp.show_result(Err(full_answer_text(&msg))),
                    None if at.elapsed() > FULL_WAIT => self.dev.insp.show_result(Err("the page did not answer in time".into())),
                    None => {
                        left.push((id, at));
                        continue;
                    }
                }
                self.dev_dirty = true;
            }
            self.inspect.full_pending = left;
            if let Some((id, at)) = self.inspect.full_tree_pending {
                match full.take_script_result(id) {
                    Some((true, json)) => {
                        self.inspect.full_tree_pending = None;
                        match tree_from_full_answer(&json) {
                            Some(t) => {
                                self.inspect.full_tree_url = Some(full.url.clone());
                                self.dev.insp.set_tree(t, true);
                            }
                            None => self.dev.insp.push(Kind::Note, "The page's tree could not be read (too big or not a page)."),
                        }
                        self.dev_dirty = true;
                    }
                    Some((false, msg)) => {
                        self.inspect.full_tree_pending = None;
                        self.dev.insp.push(Kind::Err, format!("The page's tree could not be read: {}", full_answer_text(&msg)));
                        self.dev_dirty = true;
                    }
                    None if at.elapsed() > FULL_WAIT => {
                        self.inspect.full_tree_pending = None;
                        self.dev.insp.push(Kind::Note, "The page did not give its tree in time.");
                        self.dev_dirty = true;
                    }
                    None => {}
                }
            }
            // another page is open in the full engine: its tree is not the one shown
            if self.dev.tab == browser_devpanel::Tab::Elements && self.inspect.full_tree_pending.is_none() && self.inspect.full_tree_url.as_deref().is_some_and(|u| u != full.url) {
                self.inspect.full_tree_url = None;
                self.dev.insp.clear_tree();
                self.dev_dirty = true;
            }
        } else if !self.inspect.full_pending.is_empty() || self.inspect.full_tree_pending.is_some() {
            self.inspect.full_pending.clear();
            self.inspect.full_tree_pending = None;
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
    pub(super) fn dev_inspect(&mut self, cx: &AppCx) {
        if let Some(full) = self.full.as_mut() {
            // the full engine: one `Script` message that returns the tree as bounded JSON; the answer comes back through `inspect_poll`
            if self.inspect.full_tree_pending.is_none() {
                let host = full.url.split("://").nth(1).and_then(|r| r.split(['/', ':', '?']).next()).unwrap_or("").to_string();
                cx.audit.record(&cx.session.user, "browser.devtools.tree", &host);
                let id = full.eval(FULL_TREE_SCRIPT, 5000);
                self.inspect.full_tree_pending = Some((id, std::time::Instant::now()));
            }
            return;
        }
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
