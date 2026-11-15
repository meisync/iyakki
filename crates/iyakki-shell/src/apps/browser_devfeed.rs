//! The built-in reader's own network list and console notes, fed to the Developer tools buffers (`iyakki_agent::devtools`) so that the
//! chat agent's tools (`browser_failed_requests`, `browser_timings`, `browser_console`, F-BROW-66) and the panel's copy (F-BROW-68) work
//! for pages the reader shows, not only for pages in the full web engine.
//!
//! Only while something is collecting for this person (the Developer tools panel is open, or the switch for the chat agent is on):
//! otherwise the lines are skipped, exactly as the full engine's are. Everything is scrubbed (`scrub_url`, `scrub_text`) before it is
//! stored: no query strings, fragments, user info or token-like words, no bodies or headers.
//!
//! * a request that failed or answered 400 or more becomes a failed request;
//! * a request that took a second or more becomes a slow resource;
//! * the page load becomes one timing (the first answer, and the whole load; the reader has no separate DOMContentLoaded, so that is the same number);
//! * the page's script notes (`error in app.js: ...`, `could not load ...`, console `error` and `warn` lines, scripts stopped) become
//!   console errors and warnings, with the script's name as the source.

use super::*;
use iyakki_agent::devtools::{scrub_text, scrub_url, DevEvent, Level};

/// A request this slow (ms) is listed as a slow resource.
const SLOW_MS: u128 = 1000;

/// A note of the page as a console message: `(level, source, text)`, or `None` for notes that are not messages from the page.
pub(super) fn note_to_console(note: &str) -> Option<(Level, String, String)> {
    let n = note.trim();
    if let Some(rest) = n.strip_prefix("error in ") {
        let (name, msg) = rest.split_once(": ")?;
        return Some((Level::Error, name.trim().to_string(), msg.trim().to_string()));
    }
    if let Some(rest) = n.strip_prefix("could not load ") {
        let (name, msg) = rest.split_once(": ").unwrap_or((rest, "could not be loaded"));
        return Some((Level::Error, name.trim().to_string(), format!("could not load: {msg}")));
    }
    for (p, level) in [("error: ", Level::Error), ("warn: ", Level::Warn), ("warning: ", Level::Warn)] {
        if let Some(t) = n.strip_prefix(p) {
            return Some((level, String::new(), t.trim().to_string()));
        }
    }
    if n.starts_with("scripts stopped") || n.starts_with("timers stopped") || n.starts_with("The page's scripts left it empty") || n.contains("was skipped (over") {
        return Some((Level::Warn, String::new(), n.to_string()));
    }
    None
}

impl BrowserApp {
    /// Called on every heartbeat (cheap when nothing changed).
    pub(super) fn dev_feed_reader(&mut self, cx: &AppCx) {
        if self.full.is_some() || self.embedded {
            return;
        }
        let (dt, user) = (&cx.services.devtools, cx.session.user.as_str());
        let len = self.net_log.lock().map_or(0, |l| l.len());
        if !dt.collecting(user) {
            // nothing is kept while nothing collects: what is already in the list is skipped, as for the full engine
            self.dev_fed = len;
            return;
        }
        let url = self.page().map(|p| p.url.to_string());
        if let Some(u) = &url {
            if self.dev_page_fed.as_deref() != Some(u.as_str()) {
                self.dev_page_fed = Some(u.clone());
                dt.set_page(user, u);
            }
        }
        // requests since the last call
        if len < self.dev_fed {
            self.dev_fed = 0; // a new load started a new list
        }
        if len > self.dev_fed {
            let fresh: Vec<browser_devtools::NetLine> = self.net_log.lock().map(|l| l[self.dev_fed..].to_vec()).unwrap_or_default();
            for n in fresh {
                let ms = n.ms.min(u32::MAX as u128) as u32;
                let bad = n.status.is_none_or(|s| s >= 400);
                let addr = scrub_url(&n.url);
                if addr.is_empty() {
                    continue;
                }
                if bad {
                    dt.record(user, DevEvent::Failed { method: n.method.to_ascii_uppercase(), status: n.status.unwrap_or(0), ms, url: addr, error: scrub_text(&n.error) });
                } else if n.ms >= SLOW_MS {
                    dt.record(user, DevEvent::Slow { kind: "resource".into(), bytes: n.bytes as u64, ms, url: addr });
                }
            }
            self.dev_fed = len;
        }
        // the page: its load timing and its script notes, once
        if let (Some(u), Some(p)) = (url, self.page().cloned()) {
            if self.dev_notes_fed.as_deref() != Some(u.as_str()) && !self.is_loading() {
                self.dev_notes_fed = Some(u.clone());
                let first = self.net_log.lock().ok().and_then(|l| l.first().map(|n| n.ms.min(u32::MAX as u128) as u32)).unwrap_or(0);
                if self.load_ms > 0 {
                    dt.record(user, DevEvent::Page { response_ms: first, dcl_ms: self.load_ms.max(first), load_ms: self.load_ms.max(first), url: u });
                }
                for note in &p.notes {
                    if let Some((level, source, text)) = note_to_console(note) {
                        let source = if source.is_empty() { String::new() } else { scrub_url(&source) };
                        dt.record(user, DevEvent::Console { level, line: 0, col: 0, source, text: scrub_text(&text) });
                    }
                }
            }
        }
    }
}
