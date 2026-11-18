//! The one implementation of browser actions by CSS selector, for QA: used by the FLOWS browser nodes (`flows/browser.rs`) on the
//! browser session each flow run owns, and available to any other caller that has a page engine behind a [`BrowserJobs`] queue.
//!
//! It sits on the same pieces as the assistant's browser tools (`browsertools::Drv`, the job queue, the `Servicer`, and the engines
//! behind it: the WebKit/WPE helper through `Headless`, or a test double), so there is exactly one driver. It does NOT decide who may
//! do what: host policy, approvals, budgets, secrets and the files it writes are the caller's (`flows/browser.rs`). What it does
//! guarantee: passwords and token-like fields are removed from DOM dumps and extracted values, typed text is never put in an error
//! or a log, and every wait has a deadline and a cancel check.
//!
//! The script that runs in the page is `browserqa.js`. Console messages and script errors are collected from the moment that script
//! is in the page (the helper has no hook that runs before the page's own scripts), so messages printed during the first moments of
//! a load can be missed; everything after is kept, with the address of the page it came from.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browserconsent::BrowserState;
use crate::browserjobs::{Op, Reply};
use crate::browsertools::{decode_json, key_to_sym, Drv};
use crate::qaimage::Image;

pub const QA_JS: &str = include_str!("browserqa.js");
const QA_VERSION: u32 = 1;
/// Most console messages kept for a session.
const MAX_CONSOLE: usize = 500;
/// Pictures taller than this many screens are cut.
const MAX_SCREENS: usize = 12;

pub type CancelFn<'a> = &'a dyn Fn() -> bool;

#[derive(Default)]
pub struct State {
    /// Selectors of the iframes later operations aim at, outermost first.
    frames: Vec<String>,
    console: Vec<Value>,
    viewport: (u32, u32),
}

/// What a QA session remembers between actions (the iframe it is aimed at, the console messages, the viewport). It belongs to
/// the session of a run, not to the short-lived [`QaSession`] value that acts.
#[derive(Default)]
pub struct QaState(Mutex<State>);

/// The QA driver of one page engine (identified by the key its jobs are queued under).
pub struct QaSession<'a> {
    st: &'a BrowserState,
    key: &'a str,
    state: &'a Mutex<State>,
}

fn clip(s: &str, n: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    if t.chars().count() <= n {
        t
    } else {
        format!("{}…", t.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

impl<'a> QaSession<'a> {
    pub fn new(st: &'a BrowserState, key: &'a str, state: &'a QaState) -> QaSession<'a> {
        QaSession { st, key, state: &state.0 }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn drv(&self) -> Drv<'a> {
        Drv::new(self.st, self.key)
    }

    /// One operation of the page script (installing the script when the page does not have it yet).
    pub fn qa(&self, name: &str, args: &Value) -> Result<Value, String> {
        let mut a = args.clone();
        if let Value::Object(m) = &mut a {
            let frames = self.lock().frames.clone();
            if !frames.is_empty() && !m.contains_key("frame") {
                m.insert("frame".into(), json!(frames));
            }
        }
        let d = self.drv();
        let call = format!("window.__iyakkiQA.run({}, {})", json!(name), json!(a.to_string()));
        let thin = format!("(function(){{var a=window.__iyakkiQA;if(!a||a.v!=={QA_VERSION})return 'NEED_BUNDLE';return {call};}})()");
        let mut raw = d.eval(&thin, 20_000)?;
        if raw.trim().trim_matches('"') == "NEED_BUNDLE" {
            d.eval(&format!("(function(){{{QA_JS}\n;return 'ok';}})()"), 20_000)?;
            raw = d.eval(&thin, 20_000)?;
        }
        let v = decode_json(&raw)?;
        if v["ok"].as_bool() == Some(true) {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("the page refused").to_string())
        }
    }

    /// Move the page's new console messages into the session's list.
    fn drain_console(&self) {
        if let Ok(v) = self.qa("console", &json!({})) {
            let url = self.drv().state().map(|s| s.url).unwrap_or_default();
            let mut g = self.lock();
            for m in v["messages"].as_array().cloned().unwrap_or_default() {
                if g.console.len() < MAX_CONSOLE {
                    let mut m = m;
                    m["url"] = json!(url);
                    g.console.push(m);
                }
            }
        }
    }

    pub fn console(&self) -> Vec<Value> {
        self.drain_console();
        self.lock().console.clone()
    }

    pub fn console_errors(&self) -> Vec<String> {
        self.console()
            .iter()
            .filter(|m| matches!(m["level"].as_str().unwrap_or(""), "error" | "exception" | "unhandledrejection"))
            .map(|m| format!("{}: {}", m["level"].as_str().unwrap_or(""), m["text"].as_str().unwrap_or("")))
            .collect()
    }

    fn sleep(&self, ms: u64, cancel: CancelFn) -> Result<(), String> {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if cancel() {
                return Err("cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(20).min(end.saturating_duration_since(Instant::now())));
        }
        Ok(())
    }

    fn poll(&self, what: &str, timeout: Duration, cancel: CancelFn, mut f: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
        let end = Instant::now() + timeout;
        loop {
            if cancel() {
                return Err("cancelled".into());
            }
            if f()? {
                return Ok(());
            }
            if Instant::now() >= end {
                return Err(format!("timed out after {:.1} s waiting for {what}", timeout.as_secs_f64()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    // ------------------------------------------------------------------------------------------ navigation

    /// Open an address and wait: `load` (finished loading), `idle` (and quiet) or `none`.
    pub fn open(&self, url: &str, wait: &str, timeout: Duration, cancel: CancelFn) -> Result<Value, String> {
        if !self.drv().state().map(|s| s.url.is_empty() || s.url.starts_with("about:")).unwrap_or(true) {
            self.drain_console();
        }
        let d = self.drv();
        self.lock().frames.clear();
        d.navigate(url)?;
        if wait != "none" {
            let start = Instant::now();
            std::thread::sleep(Duration::from_millis(80));
            self.poll("the page to load", timeout, cancel, || {
                let st = d.state()?;
                if let Some(why) = st.gone {
                    return Err(format!("the browser engine stopped ({why})"));
                }
                Ok(!st.loading && (start.elapsed() > Duration::from_millis(200) || st.url.as_str() != "about:blank" && !st.url.is_empty()))
            })?;
            if wait == "idle" {
                d.settle(timeout.saturating_sub(start.elapsed()).max(Duration::from_millis(500)));
            }
        }
        let st = d.state()?;
        if st.url.is_empty() || st.url.starts_with("about:") {
            return Err("the page could not be opened (the browser stayed on a blank page); the address may be wrong, unreachable, or not allowed".into());
        }
        let _ = self.qa("hook", &json!({}));
        let nav = self.qa("nav", &json!({})).unwrap_or_else(|_| json!({}));
        let st = d.state()?;
        Ok(json!({"url": st.url, "title": st.title, "status": nav["status"], "loaded": !st.loading}))
    }

    pub fn state(&self) -> Result<Value, String> {
        let st = self.drv().state()?;
        Ok(json!({"url": st.url, "title": st.title, "loading": st.loading}))
    }

    pub fn set_viewport(&self, w: u32, h: u32, cancel: CancelFn) -> Result<Value, String> {
        self.drv().simple(Op::Resize { w, h })?;
        self.lock().viewport = (w, h);
        self.sleep(350, cancel)?;
        let p = self.qa("probe", &json!({})).ok();
        Ok(json!({"width": w, "height": h, "inner_width": p.as_ref().map(|p| p["width"].clone()).unwrap_or(Value::Null), "inner_height": p.as_ref().map(|p| p["height"].clone()).unwrap_or(Value::Null)}))
    }

    pub fn viewport(&self) -> (u32, u32) {
        self.lock().viewport
    }

    // ------------------------------------------------------------------------------------------ pictures

    fn frame(&self) -> Result<Image, String> {
        match self.drv().job(Op::Screenshot, 15)? {
            Reply::Frame { w, h, rgba } => Image::new(w, h, rgba),
            r => Err(format!("unexpected answer {r:?}")),
        }
    }

    /// A picture of the page, of the element, or of the whole page (screens joined, at most 12).
    pub fn capture(&self, selector: &str, index: i64, full_page: bool, cancel: CancelFn) -> Result<Image, String> {
        if !selector.is_empty() {
            let r = self.qa("rect", &json!({"selector": selector, "index": index}))?;
            self.sleep(200, cancel)?;
            let img = self.frame()?;
            let vw = r["vw"].as_f64().unwrap_or(img.w as f64).max(1.0);
            let k = img.w as f64 / vw;
            let (x, y, w, h) = (r["x"].as_f64().unwrap_or(0.0) * k, r["y"].as_f64().unwrap_or(0.0) * k, r["w"].as_f64().unwrap_or(0.0) * k, r["h"].as_f64().unwrap_or(0.0) * k);
            return img.crop(x.round() as i64, y.round() as i64, w.ceil() as i64, h.ceil() as i64).ok_or_else(|| format!("\"{selector}\" is outside the visible page or has no size, so there is nothing to take a picture of"));
        }
        if !full_page {
            return self.frame();
        }
        let p = self.qa("probe", &json!({}))?;
        let (vh, total) = (p["height"].as_f64().unwrap_or(0.0).max(1.0), p["scroll_h"].as_f64().unwrap_or(0.0));
        if total <= vh + 1.0 {
            return self.frame();
        }
        let first = {
            self.qa("scrollpos", &json!({"y": 0}))?;
            self.sleep(150, cancel)?;
            self.frame()?
        };
        let k = first.w as f64 / p["width"].as_f64().unwrap_or(first.w as f64).max(1.0);
        let mut parts: Vec<Image> = vec![first.clone()];
        let screen_h = first.h as f64;
        let mut done_css = vh;
        while done_css < total - 0.5 && parts.len() < MAX_SCREENS {
            let y = done_css.min((total - vh).max(0.0));
            self.qa("scrollpos", &json!({"y": y}))?;
            self.sleep(180, cancel)?;
            let img = self.frame()?;
            let at = self.qa("probe", &json!({}))?["scroll_y"].as_f64().unwrap_or(y);
            // the screen shows the page from `at`; keep only the part not shown yet
            let new_from_css = done_css - at;
            let skip = (new_from_css * k).round().max(0.0) as i64;
            match img.crop(0, skip.min(img.h as i64 - 1), img.w as i64, img.h as i64) {
                Some(c) => parts.push(c),
                None => break,
            }
            done_css = at + screen_h / k;
        }
        let _ = self.qa("scrollpos", &json!({"y": 0}));
        Image::stack(&parts).ok_or_else(|| "the page's pictures did not fit together".to_string())
    }

    // ------------------------------------------------------------------------------------------ acting

    fn native_click(&self, selector: &str, index: i64, double: bool, cancel: CancelFn) -> Result<(), String> {
        let r = self.qa("rect", &json!({"selector": selector, "index": index}))?;
        let (x, y) = (r["cx"].as_i64().unwrap_or(0) as i32, r["cy"].as_i64().unwrap_or(0) as i32);
        let d = self.drv();
        d.simple(Op::Mouse { kind: 0, x, y })?;
        for _ in 0..if double { 2 } else { 1 } {
            d.simple(Op::Mouse { kind: 1, x, y })?;
            d.simple(Op::Mouse { kind: 2, x, y })?;
        }
        self.sleep(100, cancel)
    }

    pub fn click(&self, selector: &str, index: i64, double: bool, native: bool, cancel: CancelFn) -> Result<Value, String> {
        self.drain_console();
        if native {
            self.native_click(selector, index, double, cancel)?;
        } else {
            self.qa("click", &json!({"selector": selector, "index": index, "double": double}))?;
        }
        self.after(cancel);
        Ok(json!({"clicked": selector}))
    }

    /// Let the page react (navigation, scripts) before the next node.
    pub fn after(&self, cancel: CancelFn) {
        let _ = self.sleep(120, cancel);
        self.drv().settle(Duration::from_millis(2500));
    }

    pub fn type_text(&self, selector: &str, index: i64, text: &str, clear: bool, native: bool, enter: bool, cancel: CancelFn) -> Result<Value, String> {
        let d = self.drv();
        let r = if native {
            self.qa("focus", &json!({"selector": selector, "index": index}))?;
            if clear {
                d.simple(Op::Key { keysym: iyakki_webview::keys::from_char('a'), mods: iyakki_webview::proto::MOD_CTRL, text: String::new() })?;
                let (sym, t) = key_to_sym("Backspace").unwrap_or((0xff08, String::new()));
                d.simple(Op::Key { keysym: sym, mods: 0, text: t })?;
            }
            d.simple(Op::Text(text.to_string()))?;
            json!({"length": text.chars().count()})
        } else {
            self.qa("type", &json!({"selector": selector, "index": index, "text": text, "clear": clear}))?
        };
        if enter {
            self.qa("focus", &json!({"selector": selector, "index": index}))?;
            let (sym, t) = key_to_sym("Enter").unwrap_or((0xff0d, "\r".into()));
            d.simple(Op::Key { keysym: sym, mods: 0, text: t })?;
        }
        self.after(cancel);
        Ok(json!({"typed_into": selector, "length": r["length"], "secret_field": r["secret"], "submitted": enter}))
    }

    /// Fill a map of selector to value. `file_ok` is asked about the path given for a file input.
    pub fn fill(&self, fields: &serde_json::Map<String, Value>, file_ok: &dyn Fn(&str) -> Result<(), String>, cancel: CancelFn) -> Result<Value, String> {
        let mut done: Vec<Value> = Vec::new();
        for (sel, v) in fields {
            if cancel() {
                return Err("cancelled".into());
            }
            let r = self.qa("fill_one", &json!({"selector": sel, "value": v}));
            match r {
                Ok(r) => done.push(json!({"selector": sel, "kind": r["kind"], "secret": r["secret"]})),
                Err(e) if e == "FILE" => {
                    file_ok(v.as_str().unwrap_or(""))?;
                    return Err(format!("{sel} is a file input: the path is allowed, but this browser engine cannot attach files to a page yet, so the form cannot be filled this way"));
                }
                Err(e) => return Err(format!("{sel}: {e} (filled {} of {} fields before this)", done.len(), fields.len())),
            }
        }
        self.after(cancel);
        Ok(json!({"filled": done}))
    }

    pub fn submit(&self, selector: &str, index: i64, cancel: CancelFn) -> Result<Value, String> {
        self.drain_console();
        self.qa("submit", &json!({"selector": if selector.is_empty() { Value::Null } else { json!(selector) }, "index": index}))?;
        self.after(cancel);
        Ok(json!({"submitted": if selector.is_empty() { "the first form" } else { selector }}))
    }

    pub fn key(&self, key: &str, selector: &str, index: i64, cancel: CancelFn) -> Result<Value, String> {
        let (sym, text) = key_to_sym(key).ok_or_else(|| format!("unknown key \"{}\" (Enter, Escape, Tab, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, Space or one character)", clip(key, 20)))?;
        self.drain_console();
        if !selector.is_empty() {
            self.qa("focus", &json!({"selector": selector, "index": index}))?;
        }
        self.drv().simple(Op::Key { keysym: sym, mods: 0, text })?;
        self.after(cancel);
        Ok(json!({"pressed": key}))
    }

    pub fn hover(&self, selector: &str, index: i64, cancel: CancelFn) -> Result<Value, String> {
        self.qa("hover", &json!({"selector": selector, "index": index}))?;
        if let Ok(r) = self.qa("rect", &json!({"selector": selector, "index": index})) {
            let _ = self.drv().simple(Op::Mouse { kind: 0, x: r["cx"].as_i64().unwrap_or(0) as i32, y: r["cy"].as_i64().unwrap_or(0) as i32 });
        }
        self.sleep(150, cancel)?;
        Ok(json!({"hovered": selector}))
    }

    pub fn select(&self, selector: &str, index: i64, option: &str, by: &str, cancel: CancelFn) -> Result<Value, String> {
        let r = self.qa("select", &json!({"selector": selector, "index": index, "option": option, "by": by}))?;
        self.after(cancel);
        Ok(json!({"selected": r["selected"], "value": r["value"]}))
    }

    pub fn scroll(&self, args: &Value) -> Result<Value, String> {
        let r = self.qa("scroll", args)?;
        Ok(json!({"scroll_y": r["scroll_y"]}))
    }

    // ------------------------------------------------------------------------------------------ reading

    pub fn dom(&self, args: &Value, redact: &dyn Fn(&str) -> String) -> Result<Value, String> {
        let mut o = self.qa("dom", args)?;
        if args["console"].as_bool().unwrap_or(true) {
            let c = self.console();
            let skip = c.len().saturating_sub(200);
            o["console"] = Value::Array(c.into_iter().skip(skip).collect());
        }
        if let Value::Object(m) = &mut o {
            m.remove("ok");
        }
        // typed secrets the page echoes back
        Ok(redact_value(&o, redact))
    }

    pub fn extract(&self, args: &Value) -> Result<Value, String> {
        let mut o = self.qa("extract", args)?;
        if let Value::Object(m) = &mut o {
            m.remove("ok");
        }
        Ok(o)
    }

    pub fn run_js(&self, expression: &str, a: &Value, timeout: Duration) -> Result<Value, String> {
        // the whole script goes through the page script's `js` operation so that the result is plain JSON
        let call = format!("window.__iyakkiQA.run('js', {})", json!(json!({"expression": expression, "args": a, "frame": self.lock().frames}).to_string()));
        let thin = format!("(function(){{var a=window.__iyakkiQA;if(!a||a.v!=={QA_VERSION})return 'NEED_BUNDLE';return {call};}})()");
        let d = self.drv();
        let ms = timeout.as_millis().min(120_000) as u32;
        let mut raw = d.eval(&thin, ms)?;
        if raw.trim().trim_matches('"') == "NEED_BUNDLE" {
            d.eval(&format!("(function(){{{QA_JS}\n;return 'ok';}})()"), 20_000)?;
            raw = d.eval(&thin, ms)?;
        }
        let v = decode_json(&raw)?;
        if v["ok"].as_bool() == Some(true) {
            Ok(v["value"].clone())
        } else {
            Err(format!("the script failed: {}", v["error"].as_str().unwrap_or("")))
        }
    }

    pub fn set_storage(&self, kind: &str, name: &str, value: &str, path: &str) -> Result<Value, String> {
        let st = self.drv().state()?;
        if st.url.is_empty() || st.url.starts_with("about:") {
            return Err("open a page of the site first: cookies and storage belong to a site".into());
        }
        self.qa("storage", &json!({"kind": kind, "name": name, "value": value, "path": path}))?;
        Ok(json!({"set": kind, "name": name}))
    }

    // ------------------------------------------------------------------------------------------ frames

    pub fn frame_action(&self, action: &str, selector: &str) -> Result<Value, String> {
        match action {
            "frame" => {
                let mut f = self.lock().frames.clone();
                f.push(selector.to_string());
                // the check runs with the new path
                self.qa("frame_check", &json!({"frame": f}))?;
                self.lock().frames = f;
            }
            "parent" => {
                self.lock().frames.pop();
            }
            "main" => self.lock().frames.clear(),
            "tabs" => {
                let st = self.drv().state()?;
                return Ok(json!({"tabs": [{"index": 0, "url": st.url, "title": st.title, "current": true}], "note": "this engine has one tab; a link that opens a new window opens in this one"}));
            }
            other => return Err(format!("unknown action `{other}`")),
        }
        let frames = self.lock().frames.clone();
        Ok(json!({"frame": frames}))
    }

    pub fn downloads(&self) -> Result<Vec<(String, String)>, String> {
        match self.drv().job(Op::Downloads, 15)? {
            Reply::Json(j) => {
                let v: Value = serde_json::from_str(&j).map_err(|e| e.to_string())?;
                Ok(v.as_array().cloned().unwrap_or_default().iter().map(|x| (x["url"].as_str().unwrap_or("").to_string(), x["name"].as_str().unwrap_or("").to_string())).collect())
            }
            r => Err(format!("unexpected answer {r:?}")),
        }
    }

    // ------------------------------------------------------------------------------------------ checking

    /// One look at a check: `(passed, message)`. `expected` is text; `attribute` names an attribute.
    fn check_once(&self, check: &str, selector: &str, index: i64, expected: &str, attribute: &str) -> Result<(bool, String), String> {
        let want_n = || expected.trim().parse::<usize>().map_err(|_| format!("`count` needs a whole number, not \"{}\"", clip(expected, 30)));
        let st = || self.drv().state();
        let sel = format!("\"{selector}\"");
        Ok(match check {
            "url_equals" => {
                let u = st()?.url;
                (u == expected, if u == expected { format!("the address is {u}") } else { format!("expected the address to be {expected} but it is {u}") })
            }
            "url_contains" => {
                let u = st()?.url;
                (u.contains(expected), if u.contains(expected) { format!("the address contains \"{expected}\"") } else { format!("expected the address to contain \"{expected}\" but it is {u}") })
            }
            "title_equals" => {
                let t = st()?.title;
                (t == expected, if t == expected { format!("the title is \"{t}\"") } else { format!("expected the title to be \"{expected}\" but it is \"{t}\"") })
            }
            "title_contains" => {
                let t = st()?.title;
                (t.contains(expected), if t.contains(expected) { format!("the title contains \"{expected}\"") } else { format!("expected the title to contain \"{expected}\" but it is \"{t}\"") })
            }
            "no_console_errors" => {
                let e = self.console_errors();
                (e.is_empty(), if e.is_empty() { "no console errors".into() } else { format!("{} console error(s): {}", e.len(), clip(&e.iter().take(3).cloned().collect::<Vec<_>>().join(" | "), 400)) })
            }
            "page_contains" | "page_not_contains" => {
                let t = self.qa("page_text", &json!({}))?["text"].as_str().unwrap_or("").to_string();
                let has = t.contains(expected);
                if check == "page_contains" {
                    (has, if has { format!("the page shows \"{expected}\"") } else { format!("expected the page to show \"{expected}\" but it does not") })
                } else {
                    (!has, if !has { format!("the page does not show \"{expected}\"") } else { format!("expected the page not to show \"{expected}\" but it does") })
                }
            }
            _ => {
                let i = self.qa("info", &json!({"selector": selector, "index": index}))?;
                let exists = i["exists"].as_bool().unwrap_or(false);
                let count = i["count"].as_u64().unwrap_or(0) as usize;
                let text = i["text"].as_str().unwrap_or("").to_string();
                let need = |what: &str| -> Option<(bool, String)> { (!exists).then(|| (false, format!("expected {sel} to {what} but no element matches it"))) };
                match check {
                    "exists" => (exists, if exists { format!("{sel} exists ({count} match)") } else { format!("expected {sel} to exist but no element matches it") }),
                    "not_exists" => (!exists, if !exists { format!("{sel} does not exist") } else { format!("expected {sel} not to exist but {count} element(s) match it") }),
                    "count" => {
                        let n = want_n()?;
                        (count == n, if count == n { format!("{sel} matches {n} element(s)") } else { format!("expected {sel} to match {n} element(s) but it matches {count}") })
                    }
                    "text_equals" => need("have text").unwrap_or_else(|| (text == expected, if text == expected { format!("{sel} text is \"{}\"", clip(&text, 80)) } else { format!("expected the text of {sel} to equal \"{}\" but it is \"{}\"", clip(expected, 80), clip(&text, 80)) })),
                    "text_contains" => need("contain text").unwrap_or_else(|| (text.contains(expected), if text.contains(expected) { format!("{sel} text contains \"{}\"", clip(expected, 80)) } else { format!("expected the text of {sel} to contain \"{}\" but it is \"{}\"", clip(expected, 80), clip(&text, 120)) })),
                    "text_matches" => {
                        let re = iyakki_flow::minire::Re::new(expected).map_err(|e| format!("the pattern is not valid: {e}"))?;
                        need("match a pattern").unwrap_or_else(|| (re.is_match(&text), if re.is_match(&text) { format!("{sel} text matches /{expected}/") } else { format!("expected the text of {sel} to match /{expected}/ but it is \"{}\"", clip(&text, 120)) }))
                    }
                    "value_equals" => {
                        let v = i["value"].as_str().unwrap_or("").to_string();
                        need("have a value").unwrap_or_else(|| (v == expected, if v == expected { format!("{sel} value is \"{}\"", clip(&v, 60)) } else { format!("expected the value of {sel} to be \"{}\" but it is \"{}\"", clip(expected, 60), clip(&v, 60)) }))
                    }
                    "attr_equals" | "attr_contains" => {
                        let a = i["attrs"][attribute].as_str().map(str::to_string);
                        need("have an attribute").unwrap_or_else(|| match a {
                            None => (false, format!("expected {sel} to have the attribute {attribute} but it has none")),
                            Some(a) => {
                                let ok = if check == "attr_equals" { a == expected } else { a.contains(expected) };
                                (ok, if ok { format!("{sel} {attribute} {} \"{}\"", if check == "attr_equals" { "is" } else { "contains" }, clip(expected, 80)) } else { format!("expected {attribute} of {sel} to {} \"{}\" but it is \"{}\"", if check == "attr_equals" { "equal" } else { "contain" }, clip(expected, 80), clip(&a, 120)) })
                            }
                        })
                    }
                    "visible" | "hidden" => {
                        let vis = i["visible"].as_bool().unwrap_or(false);
                        if check == "visible" {
                            need("be visible").unwrap_or_else(|| (vis, if vis { format!("{sel} is visible") } else { format!("expected {sel} to be visible but it is hidden") }))
                        } else {
                            (!exists || !vis, if !exists || !vis { format!("{sel} is hidden") } else { format!("expected {sel} to be hidden but it is visible") })
                        }
                    }
                    "enabled" | "disabled" => {
                        let en = i["enabled"].as_bool().unwrap_or(false);
                        let want = check == "enabled";
                        need(check).unwrap_or_else(|| (en == want, if en == want { format!("{sel} is {check}") } else { format!("expected {sel} to be {check} but it is {}", if en { "enabled" } else { "disabled" }) }))
                    }
                    "checked" => {
                        let c = i["checked"].as_bool().unwrap_or(false);
                        need("be checked").unwrap_or_else(|| (c, if c { format!("{sel} is checked") } else { format!("expected {sel} to be checked but it is not") }))
                    }
                    other => return Err(format!("`{other}` is not a check")),
                }
            }
        })
    }

    /// Keep looking until the check holds or `wait` has passed; the message is the last look's.
    pub fn check(&self, check: &str, selector: &str, index: i64, expected: &str, attribute: &str, wait: Duration, cancel: CancelFn) -> Result<(bool, String), String> {
        let end = Instant::now() + wait;
        loop {
            if cancel() {
                return Err("cancelled".into());
            }
            let (ok, msg) = self.check_once(check, selector, index, expected, attribute)?;
            if ok || Instant::now() >= end {
                return Ok((ok, msg));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn wait_for(&self, what: &str, value: &str, ms: u64, timeout: Duration, cancel: CancelFn) -> Result<Value, String> {
        match what {
            "time" => {
                self.sleep(ms.min(timeout.as_millis() as u64 + 1000), cancel)?;
                Ok(json!({"waited_ms": ms}))
            }
            "selector" | "selector_gone" => {
                let gone = what == "selector_gone";
                let mut count = 0;
                self.poll(&format!("{} {value} {}", "the selector", if gone { "to go away" } else { "to appear" }), timeout, cancel, || {
                    count = self.qa("info", &json!({"selector": value}))?["count"].as_u64().unwrap_or(0);
                    Ok(if gone { count == 0 } else { count > 0 })
                })?;
                Ok(json!({"selector": value, "count": count}))
            }
            "text" | "text_gone" => {
                let gone = what == "text_gone";
                self.poll(&format!("the text \"{}\" {}", clip(value, 60), if gone { "to go away" } else { "to appear" }), timeout, cancel, || {
                    let t = self.qa("page_text", &json!({}))?["text"].as_str().unwrap_or("").to_string();
                    Ok(t.contains(value) != gone)
                })?;
                Ok(json!({"text": value}))
            }
            "url" => {
                self.poll(&format!("the address to contain \"{}\"", clip(value, 80)), timeout, cancel, || Ok(self.drv().state()?.url.contains(value)))?;
                Ok(json!({"url": self.drv().state()?.url}))
            }
            other => Err(format!("`{other}` is not something to wait for")),
        }
    }
}

fn redact_value(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_value(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), redact_value(x, f))).collect()),
        other => other.clone(),
    }
}
