//! `iyakki dev state` and `iyakki dev input click "LABEL"` (scope `debug`, and `input` for the click): what the shell is showing, as
//! bounded plain text, and where the buttons are, so a paired computer can look at the screen without a picture and press a button by
//! its name.
//!
//! The shell hands over a frame only while a request is waiting (a few seconds after the last one: [`request`], [`WANT_SECS`]) and
//! only when developer mode is on (the route checks that before it asks). The raw frame ([`Raw`]) is made safe HERE, in [`build`],
//! by rules that do not depend on the shell:
//!
//! * text inside a field flagged secret, and text made of mask characters only, becomes `[hidden]`; a text that looks like a credential
//!   ([`crate::credguard`]) is hidden too;
//! * in private apps ([`PRIVATE_APPS`]: chat, inbox, terminal, browser, editors...) no text is given at all, only the names of the
//!   buttons: a message body or a page is the person's;
//! * a button over a secret field has no name and cannot be clicked from a computer;
//! * at most [`MAX_TEXTS`] lines of at most [`MAX_LINE`] characters and [`MAX_HITS`] buttons.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const MAX_TEXTS: usize = 400;
pub const MAX_LINE: usize = 160;
pub const MAX_HITS: usize = 300;
/// The shell draws with text capture on for this long after a request.
pub const WANT_SECS: u64 = 5;
/// A heartbeat older than this means no screen is being drawn.
pub const ALIVE_SECS: u64 = 5;
/// A snapshot this young answers a request without waiting for a new frame.
pub const FRESH_MS: u128 = 1000;

/// Apps whose text belongs to the person (messages, pages, terminal output, documents): their text is not given, only button names.
pub const PRIVATE_APPS: [&str; 10] = ["Chat", "Inbox", "CLI", "Browser", "Edit", "AI IDE", "Tales", "Data", "Voice", "Users"];

#[derive(Clone, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn hits(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn center(&self) -> (i32, i32) {
        ((self.x + self.w / 2.0) as i32, (self.y + self.h / 2.0) as i32)
    }
}

/// A text field drawn this frame.
#[derive(Clone, Debug)]
pub struct RawField {
    pub rect: Rect,
    pub focused: bool,
    pub secret: bool,
}

/// What the shell saw, as it saw it.
#[derive(Clone, Debug, Default)]
pub struct Raw {
    /// "Home", "Sign in", or the open app's title.
    pub app: String,
    /// The page or tab of the app, when it has one ("Developer mode").
    pub page: String,
    pub size: (u32, u32),
    /// Every string drawn: left, top, text.
    pub texts: Vec<(f32, f32, String)>,
    /// Click regions in drawing order (later = on top): the name and the rectangle.
    pub hits: Vec<(String, Rect)>,
    pub fields: Vec<RawField>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub x: i32,
    pub y: i32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Over a secret field: nameless and not clickable from a computer.
    pub secret: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub app: String,
    pub page: String,
    pub size: (u32, u32),
    /// The focused element: the label next to the focused field, "a password field", or empty.
    pub focused: String,
    pub focused_secret: bool,
    pub lines: Vec<Line>,
    /// The text was left out because the app is private.
    pub text_hidden: bool,
    pub buttons: Vec<Button>,
    /// Something was cut by a limit.
    pub cut: bool,
}

fn masked(t: &str) -> bool {
    let n = t.chars().count();
    n >= 3 && t.chars().all(|c| "•●*·∗▪■".contains(c))
}

/// Make a raw frame safe to hand to a computer.
pub fn build(raw: &Raw) -> Snapshot {
    let secret_rects: Vec<&Rect> = raw.fields.iter().filter(|f| f.secret).map(|f| &f.rect).collect();
    let private = PRIVATE_APPS.contains(&raw.app.as_str());
    let in_secret = |x: f32, y: f32, w: f32| secret_rects.iter().any(|r| r.contains(x + 1.0, y + 1.0) || r.contains(x + w / 2.0, y + 1.0));
    let mut cut = false;
    let mut lines: Vec<Line> = Vec::new();
    if !private {
        let mut texts: Vec<&(f32, f32, String)> = raw.texts.iter().filter(|t| !t.2.trim().is_empty()).collect();
        // reading order: rows (to the nearest 6 px), then left to right
        texts.sort_by(|a, b| ((a.1 / 6.0).round() as i64, a.0 as i64).cmp(&((b.1 / 6.0).round() as i64, b.0 as i64)));
        for (x, y, t) in texts {
            let t = t.trim();
            let shown = if in_secret(*x, *y, 0.0) || masked(t) {
                "[hidden]".to_string()
            } else if crate::credguard::detect(t).is_some() {
                "[hidden: looks like a credential]".to_string()
            } else {
                t.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect()
            };
            if lines.last().is_some_and(|l| l.text == shown && l.y == *y as i32 && l.x == *x as i32) {
                continue;
            }
            if lines.len() >= MAX_TEXTS {
                cut = true;
                break;
            }
            lines.push(Line { x: *x as i32, y: *y as i32, text: shown });
        }
    }
    let mut buttons: Vec<Button> = Vec::new();
    for (label, r) in raw.hits.iter().filter(|(l, r)| !l.trim().is_empty() && r.w > 0.0 && r.h > 0.0) {
        let secret = secret_rects.iter().any(|s| s.hits(r));
        let label = if secret {
            "[secret field]".to_string()
        } else if crate::credguard::detect(label).is_some() {
            "[hidden: looks like a credential]".to_string()
        } else {
            label.chars().filter(|c| !c.is_control()).take(MAX_LINE).collect()
        };
        if buttons.len() >= MAX_HITS {
            cut = true;
            break;
        }
        buttons.push(Button { label, x: r.x as i32, y: r.y as i32, w: r.w as i32, h: r.h as i32, secret });
    }
    // the focused element
    let focused_field = raw.fields.iter().find(|f| f.focused);
    let (focused, focused_secret) = match focused_field {
        None => (String::new(), false),
        Some(f) if f.secret => ("a password field".to_string(), true),
        Some(f) => {
            // the nearest text on the same row to its left, else just above it
            let (fx, fy) = (f.rect.x, f.rect.y);
            let best = raw
                .texts
                .iter()
                .filter(|t| !t.2.trim().is_empty() && (t.1 - fy).abs() < f.rect.h && t.0 < fx)
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .or_else(|| raw.texts.iter().filter(|t| !t.2.trim().is_empty() && t.1 < fy && fy - t.1 < 48.0 && t.0 <= fx + 8.0).min_by(|a, b| (fy - a.1).partial_cmp(&(fy - b.1)).unwrap_or(std::cmp::Ordering::Equal)));
            (best.filter(|_| !private).map(|t| t.2.trim().chars().take(80).collect()).unwrap_or_else(|| "a text field".to_string()), false)
        }
    };
    Snapshot { app: raw.app.clone(), page: raw.page.clone(), size: raw.size, focused, focused_secret, lines, text_hidden: private, buttons, cut }
}

impl Snapshot {
    /// Plain text for people and assistants.
    pub fn text(&self) -> String {
        let mut s = format!("app: {}\n", self.app);
        if !self.page.is_empty() {
            s.push_str(&format!("page: {}\n", self.page));
        }
        s.push_str(&format!("screen: {}x{}\nfocused: {}\n", self.size.0, self.size.1, if self.focused.is_empty() { "(nothing)" } else { &self.focused }));
        if self.text_hidden {
            s.push_str("text: hidden (a private app: messages, pages and documents are the person's); button names follow\n");
        } else {
            s.push_str("text:\n");
            for l in &self.lines {
                s.push_str(&format!("  {:>4},{:<4} {}\n", l.x, l.y, l.text));
            }
        }
        s.push_str("buttons (use `iyakki dev input click \"LABEL\"`):\n");
        for b in &self.buttons {
            s.push_str(&format!("  {:>4},{:<4} {}x{}  {}{}\n", b.x, b.y, b.w, b.h, b.label, if b.secret { "  (not clickable)" } else { "" }));
        }
        if self.cut {
            s.push_str("[cut: more than the limits of lines or buttons]\n");
        }
        s
    }

    pub fn json(&self) -> Value {
        json!({
            "app": self.app, "page": self.page, "screen": [self.size.0, self.size.1], "focused": self.focused, "focused_secret": self.focused_secret,
            "text_hidden": self.text_hidden, "cut": self.cut,
            "lines": self.lines.iter().map(|l| json!({ "x": l.x, "y": l.y, "text": l.text })).collect::<Vec<_>>(),
            "buttons": self.buttons.iter().map(|b| json!({ "label": b.label, "x": b.x, "y": b.y, "w": b.w, "h": b.h, "secret": b.secret })).collect::<Vec<_>>(),
            "text": self.text(),
        })
    }

    /// The point to tap for the button called `label`: an exact name (any case) first, else the one button whose name contains it. The
    /// button must not be over a secret field and must not be covered by another one at its centre.
    pub fn target(&self, label: &str) -> Result<(i32, i32, String), String> {
        let want = label.trim().to_lowercase();
        if want.is_empty() {
            return Err("which button? iyakki dev input click \"LABEL\"".into());
        }
        let ok = |b: &&Button| b.w > 0 && b.h > 0;
        let mut found: Vec<&Button> = self.buttons.iter().filter(ok).filter(|b| b.label.to_lowercase() == want).collect();
        if found.is_empty() {
            found = self.buttons.iter().filter(ok).filter(|b| b.label.to_lowercase().contains(&want)).collect();
        }
        // the same name drawn twice in the same place is one button
        found.dedup_by(|a, b| a.label == b.label && a.x == b.x && a.y == b.y);
        let b = match found.as_slice() {
            [] => return Err(format!("no button called {label:?} on the screen now; `iyakki dev state` lists them")),
            [b] => *b,
            many => {
                let names: Vec<String> = many.iter().take(6).map(|b| format!("{:?}", b.label)).collect();
                return Err(format!("{} buttons match {label:?}: {}: give more of the name", many.len(), names.join(", ")));
            }
        };
        if b.secret {
            return Err("that button is over a secret field: it cannot be clicked from a computer".into());
        }
        let (cx, cy) = Rect { x: b.x as f32, y: b.y as f32, w: b.w as f32, h: b.h as f32 }.center();
        // covered? the last button (drawn on top) that contains the centre must be this one
        if let Some(top) = self.buttons.iter().rev().filter(ok).find(|o| cx >= o.x && cx < o.x + o.w && cy >= o.y && cy < o.y + o.h) {
            if top != b && top.label != b.label {
                return Err(format!("{:?} is covered by {:?} at that place", b.label, top.label));
            }
            if top.secret {
                return Err("that place is over a secret field: it cannot be clicked from a computer".into());
            }
        }
        Ok((cx, cy, b.label.clone()))
    }
}

// -------------------------------------------------------------------------------------------- the hand-off to the shell

#[derive(Default)]
struct Slot {
    want_until: Option<Instant>,
    alive: Option<Instant>,
    snap: Option<(Snapshot, Instant)>,
    fresh: bool,
}

fn slots() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Slot>> {
    static S: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    S.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The shell side, every beat: "a screen is being drawn here".
pub fn heartbeat(data_dir: &Path) {
    slots().entry(data_dir.to_path_buf()).or_default().alive = Some(Instant::now());
}

pub fn alive(data_dir: &Path) -> bool {
    slots().get(data_dir).and_then(|s| s.alive).is_some_and(|t| t.elapsed() < Duration::from_secs(ALIVE_SECS))
}

/// The shell side: should this frame be drawn with text capture on?
pub fn wanted(data_dir: &Path) -> bool {
    slots().get(data_dir).and_then(|s| s.want_until).is_some_and(|t| Instant::now() < t)
}

/// The shell side: a captured frame.
pub fn publish(data_dir: &Path, raw: &Raw) {
    let snap = build(raw);
    let mut g = slots();
    let s = g.entry(data_dir.to_path_buf()).or_default();
    s.snap = Some((snap, Instant::now()));
    s.fresh = true;
}

/// The listener side: ask for a frame and wait up to `wait_ms` for it (a snapshot younger than [`FRESH_MS`] answers at once). `Err`
/// when no screen is being drawn.
pub fn fetch(data_dir: &Path, wait_ms: u64) -> Result<Snapshot, String> {
    if !alive(data_dir) {
        return Err("no screen is being drawn on the device right now (is anyone signed in? the shell may be asleep)".into());
    }
    {
        let mut g = slots();
        let s = g.entry(data_dir.to_path_buf()).or_default();
        s.want_until = Some(Instant::now() + Duration::from_secs(WANT_SECS));
        if let Some((snap, at)) = &s.snap {
            if at.elapsed().as_millis() < FRESH_MS {
                return Ok(snap.clone());
            }
        }
        s.fresh = false;
    }
    let end = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        {
            let g = slots();
            if let Some(s) = g.get(data_dir) {
                if s.fresh {
                    if let Some((snap, _)) = &s.snap {
                        return Ok(snap.clone());
                    }
                }
            }
        }
        if Instant::now() >= end {
            return Err("the screen did not answer in time; try again".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Forget everything (developer mode went off): no frame is kept.
pub fn clear(data_dir: &Path) {
    slots().remove(data_dir);
}
