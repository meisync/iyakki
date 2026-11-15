//! The Runs page as a tile (F-WEB-55). The list of the person's runs and, when one is opened, its lines (what was said, what the tools
//! answered, what is waiting). It only READS: stopping a run and answering a question stay on the Runs page and the approvals list
//! (a tile has no power over either, and its manifest asks for `runs.list` and `runs.get` only).
//!
//! * [`RunsTile`] is the whole app over a [`Source`]: events in, display list out. The guest (`guest.rs`, wasm32 only) gives it the host's
//!   `call` import as the source; the tests give it a fake.
//! * Ids of the hit regions: 1 Back, 2 Refresh, 3 Up, 4 Down, 100+n the nth visible row.
//! * The list is read again every three seconds of `Tick` time, and the open run every three seconds too (a run that goes on shows its
//!   new lines).
use iyakki_tile_abi::{line_height, measure_text, Color, DisplayList, Event, Op, Rect};
use serde_json::{json, Value};

#[cfg(target_arch = "wasm32")]
mod guest;

/// `tile.wasm`: the wasm32 build of this crate's guest (`cargo build --release --lib --target wasm32-unknown-unknown -p iyakki-tile-runs`);
/// `tests/wasm32_guest.rs` builds it again and checks that it behaves like the committed file.
#[cfg(not(target_arch = "wasm32"))]
pub const TILE_WASM: &[u8] = include_bytes!("../tile.wasm");

/// What the tile may ask the service for: the manifest lists exactly these.
pub const CAPS: [&str; 4] = ["runs.list", "runs.get", "runs.decide", "runs.stop"];

const REFRESH_MS: u32 = 3000;
const ROW_H: i32 = 30;
const TOP: i32 = 40;

/// The one thing the tile needs from its host: a JSON call into the `runs` domain.
pub trait Source {
    fn call(&mut self, method: &str, json: &str) -> Result<String, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub status: String,
    pub prompt: String,
}

/// A question of the open run that waits for the person: (call id, tool, what it will do).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub call: String,
    pub tool: String,
    pub summary: String,
}

enum Screen {
    List,
    Run { id: String, status: String, lines: Vec<String>, pending: Vec<Pending> },
}

pub struct RunsTile<S: Source> {
    src: S,
    size: (i32, i32),
    dark: bool,
    rows: Vec<Row>,
    screen: Screen,
    scroll: usize,
    error: String,
    since: u32,
}

fn clip(text: &str, width: i32, size: u32) -> String {
    if measure_text(0, size, text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        let mut t = out.clone();
        t.push(c);
        t.push_str("...");
        if measure_text(0, size, &t) > width {
            break;
        }
        out.push(c);
    }
    out.push_str("...");
    out
}

impl<S: Source> RunsTile<S> {
    pub fn new(src: S) -> Self {
        let mut t = RunsTile { src, size: (360, 480), dark: false, rows: vec![], screen: Screen::List, scroll: 0, error: String::new(), since: 0 };
        t.refresh();
        t
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    fn visible(&self) -> usize {
        (((self.size.1 - TOP) / ROW_H).max(1)) as usize
    }

    fn refresh(&mut self) {
        self.since = 0;
        let r = match &self.screen {
            Screen::List => self.src.call("list", "{}"),
            Screen::Run { id, .. } => self.src.call("get", &json!({"id": id}).to_string()),
        };
        match r.and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| "the answer was not readable".to_string())) {
            Ok(v) => {
                self.error.clear();
                match &mut self.screen {
                    Screen::List => {
                        self.rows = v["runs"]
                            .as_array()
                            .map(|a| a.iter().take(40).map(|r| Row { id: r["id"].as_str().unwrap_or("").into(), status: r["status"].as_str().unwrap_or("").into(), prompt: r["prompt"].as_str().unwrap_or("").into() }).collect())
                            .unwrap_or_default();
                        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
                    }
                    Screen::Run { status, lines, pending, .. } => {
                        *pending = v["pending"].as_array().map(|a| a.iter().take(5).map(|p| Pending { call: p["call"].as_str().unwrap_or("").into(), tool: p["tool"].as_str().unwrap_or("").into(), summary: p["summary"].as_str().unwrap_or("").chars().take(200).collect() }).filter(|p| !p.call.is_empty()).collect()).unwrap_or_default();
                        *status = v["status"].as_str().unwrap_or("").into();
                        *lines = v["lines"].as_array().map(|a| a.iter().take(60).map(|l| l.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
                    }
                }
            }
            Err(e) => self.error = clip(&e, 300, 12),
        }
    }

    fn open(&mut self, id: String) {
        self.screen = Screen::Run { id, status: String::new(), lines: vec![], pending: vec![] };
        self.scroll = 0;
        self.refresh();
    }

    /// Returns true when the picture changed.
    pub fn event(&mut self, e: &Event) -> bool {
        match e {
            Event::Resize { w, h, .. } => {
                self.size = (*w, *h);
                true
            }
            Event::Theme { dark, .. } => {
                self.dark = *dark;
                true
            }
            Event::Tick { dt_ms } => {
                self.since = self.since.saturating_add(*dt_ms);
                if self.since >= REFRESH_MS {
                    self.refresh();
                    return true;
                }
                false
            }
            Event::Hit { id: 1 } => {
                if matches!(self.screen, Screen::Run { .. }) {
                    self.screen = Screen::List;
                    self.scroll = 0;
                    self.refresh();
                }
                true
            }
            Event::Hit { id: 2 } => {
                self.refresh();
                true
            }
            Event::Hit { id: 3 } => {
                self.scroll = self.scroll.saturating_sub(self.visible().max(1));
                true
            }
            Event::Hit { id: 4 } => {
                let n = match &self.screen {
                    Screen::List => self.rows.len(),
                    Screen::Run { lines, .. } => lines.len(),
                };
                let step = self.visible().max(1);
                if self.scroll + step < n {
                    self.scroll += step;
                }
                true
            }
            Event::Hit { id: 9 } => {
                // Stop: the open run, only while it goes on; the person's own click
                let Screen::Run { id: run, status, .. } = &self.screen else { return false };
                if status != "running" {
                    return false;
                }
                let run = run.clone();
                match self.src.call("stop", &json!({"id": run}).to_string()) {
                    Ok(_) => {
                        self.error.clear();
                        self.refresh();
                    }
                    Err(e) => self.error = e.chars().take(200).collect(),
                }
                true
            }
            Event::Hit { id: id @ (7 | 8) } => {
                // the person's own click on Approve or Deny: the first question of the open run is answered, once
                let Screen::Run { id: run, pending, .. } = &self.screen else { return false };
                let Some(p) = pending.first().cloned() else { return false };
                let run = run.clone();
                let verdict = if *id == 7 { "approve" } else { "deny" };
                match self.src.call("decide", &json!({"run": run, "call": p.call, "verdict": verdict}).to_string()) {
                    Ok(_) => {
                        self.error.clear();
                        self.refresh();
                    }
                    Err(e) => self.error = e.chars().take(200).collect(),
                }
                true
            }
            Event::Hit { id } if *id >= 100 && matches!(self.screen, Screen::List) => {
                let i = self.scroll + (*id - 100) as usize;
                match self.rows.get(i).map(|r| r.id.clone()) {
                    Some(rid) => {
                        self.open(rid);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    pub fn frame(&self) -> DisplayList {
        let (w, h) = self.size;
        let (bg, fg, mute, bar) = if self.dark { (Color::rgb(24, 26, 30), Color::rgb(230, 232, 236), Color::rgb(150, 156, 166), Color::rgb(40, 44, 52)) } else { (Color::rgb(245, 245, 245), Color::rgb(30, 30, 30), Color::rgb(100, 100, 100), Color::rgb(225, 228, 232)) };
        let accent = Color::rgb(40, 110, 220);
        let text = |x: i32, y: i32, size: u32, c: Color, t: &str| Op::Text { x, y, face: 0, size, color: c, text: t.into() };
        let button = |l: &mut DisplayList, id: u32, r: Rect, label: &str| {
            l.push(Op::RoundRect { r, radius: 6, color: accent });
            let lw = measure_text(0, 13, label);
            l.push(Op::Text { x: r.x + (r.w - lw) / 2, y: r.y + (r.h - line_height(13)) / 2, face: 0, size: 13, color: Color::rgb(255, 255, 255), text: label.into() });
            l.push(Op::Hit { id, r });
        };
        let mut l = DisplayList::new();
        l.push(Op::Rect { r: Rect::new(0, 0, w, h), color: bg });
        l.push(Op::Rect { r: Rect::new(0, 0, w, 32), color: bar });
        let (n, title) = match &self.screen {
            Screen::List => (self.rows.len(), "RUNS".to_string()),
            Screen::Run { id, status, lines, .. } => (lines.len(), format!("{} {}", clip(id, 80, 14), status)),
        };
        let mut x = 8;
        if matches!(self.screen, Screen::Run { .. }) {
            button(&mut l, 1, Rect::new(x, 4, 52, 24), "Back");
            x += 60;
        }
        l.push(text(x, 8, 14, fg, &clip(&title, (w - x - if matches!(&self.screen, Screen::Run { status, .. } if status == "running") { 236 } else { 190 }).max(20), 14)));
        button(&mut l, 2, Rect::new(w - 84, 4, 76, 24), "Refresh");
        if matches!(&self.screen, Screen::Run { status, .. } if status == "running") {
            button(&mut l, 9, Rect::new(w - 216, 4, 40, 24), "Stop");
        }
        if n > self.visible() {
            button(&mut l, 3, Rect::new(w - 128, 4, 38, 24), "Up");
            button(&mut l, 4, Rect::new(w - 172, 4, 38, 24), "Down");
        }
        let mut y = TOP;
        if !self.error.is_empty() {
            l.push(text(8, y, 12, Color::rgb(200, 40, 40), &clip(&self.error, w - 16, 12)));
            y += 20;
        }
        match &self.screen {
            Screen::List => {
                if self.rows.is_empty() && self.error.is_empty() {
                    l.push(text(8, y, 13, mute, "No runs yet."));
                }
                for (k, r) in self.rows.iter().skip(self.scroll).take(self.visible()).enumerate() {
                    let ry = y + k as i32 * ROW_H;
                    let c = if r.status.contains("fail") || r.status.contains("error") { Color::rgb(200, 40, 40) } else if r.status == "running" { accent } else { Color::rgb(40, 150, 80) };
                    l.push(Op::RoundRect { r: Rect::new(8, ry + 2, 6, ROW_H - 8), radius: 2, color: c });
                    l.push(text(22, ry + 2, 13, fg, &clip(&r.prompt, w - 30, 13)));
                    l.push(text(22, ry + 15, 11, mute, &clip(&r.status, w - 30, 11)));
                    l.push(Op::Hit { id: 100 + k as u32, r: Rect::new(0, ry, w, ROW_H) });
                }
            }
            Screen::Run { lines, pending, .. } => {
                let room = if pending.is_empty() { 0 } else { 80 };
                let per = ((h - y - room) / 16).max(1) as usize;
                if let Some(p) = pending.first() {
                    l.push(Op::Rect { r: Rect::new(0, h - 78, w, 78), color: bar });
                    let head = if pending.len() > 1 { format!("asks (1 of {}): ", pending.len()) } else { "asks: ".to_string() };
                    l.push(text(8, h - 74, 12, Color::rgb(200, 130, 20), &clip(&format!("{head}{}", p.summary), w - 16, 12)));
                    button(&mut l, 7, Rect::new(8, h - 44, 100, 32), "Approve");
                    button(&mut l, 8, Rect::new(120, h - 44, 100, 32), "Deny");
                }
                for (k, t) in lines.iter().skip(self.scroll).take(per).enumerate() {
                    l.push(text(8, y + k as i32 * 16, 12, fg, &clip(t, w - 16, 12)));
                }
                if lines.is_empty() && self.error.is_empty() {
                    l.push(text(8, y, 13, mute, "Nothing recorded yet."));
                }
            }
        }
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, String)>,
        replies: VecDeque<Result<String, String>>,
    }

    impl Source for Fake {
        fn call(&mut self, m: &str, j: &str) -> Result<String, String> {
            self.calls.push((m.into(), j.into()));
            self.replies.pop_front().unwrap_or_else(|| Ok(r#"{"runs":[]}"#.into()))
        }
    }

    fn list_reply(n: usize) -> String {
        json!({"runs": (0..n).map(|i| json!({"id": format!("r{i}"), "status": if i == 0 { "running" } else { "done" }, "prompt": format!("task number {i}")})).collect::<Vec<_>>()}).to_string()
    }

    fn tile(replies: Vec<Result<String, String>>) -> RunsTile<Fake> {
        let f = Fake { replies: replies.into(), ..Default::default() };
        let mut t = RunsTile::new(f);
        t.event(&Event::Resize { w: 360, h: 300, scale_pct: 100 });
        t
    }

    fn texts(l: &DisplayList) -> Vec<String> {
        l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    fn hits(l: &DisplayList) -> Vec<u32> {
        l.ops.iter().filter_map(|o| if let Op::Hit { id, .. } = o { Some(*id) } else { None }).collect()
    }

    #[test]
    fn the_list_shows_each_run_with_its_status_and_a_hit_region_per_row() {
        let t = tile(vec![Ok(list_reply(3))]);
        let l = t.frame();
        let x = texts(&l);
        assert!(x.contains(&"RUNS".to_string()) && x.contains(&"task number 0".to_string()) && x.contains(&"running".to_string()) && x.contains(&"done".to_string()), "{x:?}");
        assert_eq!(hits(&l).into_iter().filter(|h| *h >= 100).collect::<Vec<_>>(), vec![100, 101, 102]);
        assert!(hits(&l).contains(&2), "the Refresh button");
        assert!(!hits(&l).contains(&3), "no scroll buttons for three rows");
    }

    #[test]
    fn opening_a_run_reads_it_and_back_returns_to_the_list() {
        let mut t = tile(vec![Ok(list_reply(2)), Ok(json!({"id": "r1", "status": "done", "lines": ["you: hello", "answer: it is 4"]}).to_string()), Ok(list_reply(2))]);
        assert!(t.event(&Event::Hit { id: 101 }));
        assert_eq!(t.source().calls.last().unwrap(), &("get".to_string(), r#"{"id":"r1"}"#.to_string()));
        let x = texts(&t.frame());
        assert!(x.iter().any(|s| s.contains("r1") && s.contains("done")) && x.contains(&"answer: it is 4".to_string()), "{x:?}");
        assert!(hits(&t.frame()).contains(&1), "a Back button");
        assert!(t.event(&Event::Hit { id: 1 }));
        assert!(texts(&t.frame()).contains(&"RUNS".to_string()));
        assert_eq!(t.source().calls.last().unwrap().0, "list");
        // a row that is not there is not a hit
        assert!(!t.event(&Event::Hit { id: 150 }));
    }

    #[test]
    fn the_open_run_and_the_list_are_read_again_every_three_seconds_of_tick_time_and_not_before() {
        let mut t = tile(vec![]);
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Tick { dt_ms: 2900 }));
        assert_eq!(t.source().calls.len(), n);
        assert!(t.event(&Event::Tick { dt_ms: 200 }));
        assert_eq!(t.source().calls.len(), n + 1);
        assert!(!t.event(&Event::Tick { dt_ms: 1000 }));
    }

    #[test]
    fn a_failed_call_is_said_in_a_line_and_the_old_list_stays() {
        let mut t = tile(vec![Ok(list_reply(2)), Err("the service did not answer".into())]);
        assert!(t.event(&Event::Hit { id: 2 }));
        let x = texts(&t.frame());
        assert!(x.contains(&"the service did not answer".to_string()) && x.contains(&"task number 1".to_string()), "{x:?}");
        // unreadable JSON is a sentence too, and it clears when the next call works
        let mut t = tile(vec![Ok("not json".into()), Ok(list_reply(1))]);
        assert!(t.error().contains("not readable"));
        t.event(&Event::Hit { id: 2 });
        assert_eq!(t.error(), "");
    }

    #[test]
    fn long_lists_scroll_by_a_page_inside_the_bounds_and_long_text_is_clipped_to_the_width() {
        let long = json!({"runs": (0..30).map(|i| json!({"id": format!("r{i}"), "status": "done", "prompt": format!("{i:02}{}", "x".repeat(200))})).collect::<Vec<_>>()}).to_string();
        let mut t = tile(vec![Ok(long)]);
        let l = t.frame();
        assert!(hits(&l).contains(&3) && hits(&l).contains(&4));
        for o in &l.ops {
            if let Op::Text { x, size, text, .. } = o {
                assert!(x + measure_text(0, *size, text) <= 360, "{text} is wider than the tile");
            }
        }
        let first = texts(&l);
        t.event(&Event::Hit { id: 4 });
        assert_ne!(texts(&t.frame()), first);
        for _ in 0..20 {
            t.event(&Event::Hit { id: 4 });
        }
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("29") || s.starts_with("28")), "the last page still shows the last rows");
        for _ in 0..20 {
            t.event(&Event::Hit { id: 3 });
        }
        assert_eq!(texts(&t.frame()), first);
    }

    #[test]
    fn hostile_answers_never_panic_and_the_dark_theme_changes_the_colours() {
        for bad in ["", "null", "[]", r#"{"runs":5}"#, r#"{"runs":[null,1,{"id":7}]}"#, "\u{0}\u{1}"] {
            let t = tile(vec![Ok(bad.into())]);
            let _ = t.frame().encode().unwrap();
        }
        let mut t = tile(vec![Ok(list_reply(1))]);
        let light = t.frame();
        t.event(&Event::Theme { dark: true, accent: Color::rgb(0, 0, 0), background: Color::rgb(0, 0, 0), foreground: Color::rgb(0, 0, 0) });
        assert_ne!(t.frame(), light);
        assert!(t.frame().encode().is_ok());
    }

    fn open_with_question(extra: Vec<Result<String, String>>) -> RunsTile<Fake> {
        let mut replies = vec![Ok(list_reply(2)), Ok(json!({"id": "r0", "status": "running", "lines": ["you: clean up"], "pending": [{"call": "c9", "tool": "run_command", "summary": "delete the old files"}, {"call": "c10", "tool": "web_fetch", "summary": "fetch a page"}]}).to_string())];
        replies.extend(extra);
        let f = Fake { replies: replies.into(), ..Default::default() };
        let mut t = RunsTile::new(f);
        t.event(&Event::Resize { w: 360, h: 300, scale_pct: 100 });
        t.event(&Event::Hit { id: 100 });
        t
    }

    #[test]
    fn a_question_of_the_open_run_is_shown_with_approve_and_deny_and_answered_by_a_click() {
        let mut t = open_with_question(vec![Ok(r#"{"ok":true}"#.into()), Ok(json!({"id": "r0", "status": "running", "lines": [], "pending": [{"call": "c10", "tool": "web_fetch", "summary": "fetch a page"}]}).to_string())]);
        let l = t.frame();
        let x = texts(&l);
        assert!(x.iter().any(|s| s.starts_with("asks (1 of 2): delete")), "{x:?}");
        assert!(hits(&l).contains(&7) && hits(&l).contains(&8));
        assert!(t.event(&Event::Hit { id: 7 }));
        let decide = t.source().calls.iter().find(|c| c.0 == "decide").unwrap().clone();
        assert_eq!(serde_json::from_str::<Value>(&decide.1).unwrap(), json!({"run": "r0", "call": "c9", "verdict": "approve"}));
        // the run is read again and the next question is the one shown
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("asks: fetch a page")), "{:?}", texts(&t.frame()));
    }

    #[test]
    fn deny_sends_a_no_a_failed_answer_is_said_and_with_no_question_the_buttons_do_nothing() {
        let mut t = open_with_question(vec![Ok(r#"{"ok":true}"#.into()), Ok(json!({"id": "r0", "status": "failed", "lines": [], "pending": []}).to_string())]);
        t.event(&Event::Hit { id: 8 });
        let decide = t.source().calls.iter().find(|c| c.0 == "decide").unwrap().clone();
        assert!(decide.1.contains("\"deny\""), "{}", decide.1);
        assert!(!hits(&t.frame()).contains(&7), "no question left: no buttons");
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Hit { id: 7 }) && !t.event(&Event::Hit { id: 8 }));
        assert_eq!(t.source().calls.len(), n, "nothing is sent without a question");
        let mut t = open_with_question(vec![Err("the service did not answer".into())]);
        t.event(&Event::Hit { id: 7 });
        assert_eq!(t.error(), "the service did not answer");
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("asks (1 of 2): delete")), "the question stays");
        // on the list screen there are no such buttons at all
        let mut t = tile(vec![Ok(list_reply(1))]);
        assert!(!t.event(&Event::Hit { id: 7 }));
    }

    #[test]
    fn stop_is_there_only_for_a_run_that_goes_on_and_asks_the_service_to_stop_that_run() {
        let mut t = tile(vec![Ok(list_reply(2)), Ok(json!({"id": "r0", "status": "running", "lines": ["you: long job"], "pending": []}).to_string())]);
        t.event(&Event::Hit { id: 100 });
        assert!(texts(&t.frame()).contains(&"Stop".to_string()) && hits(&t.frame()).contains(&9), "Stop while the open run goes on");
        let n = t.source().calls.len();
        t.source().replies.push_back(Ok(r#"{"ok":true}"#.into()));
        t.source().replies.push_back(Ok(json!({"id": "r0", "status": "stopped", "lines": ["stopped: you stopped it"], "pending": []}).to_string()));
        assert!(t.event(&Event::Hit { id: 9 }));
        assert_eq!(t.source().calls[n], ("stop".to_string(), r#"{"id":"r0"}"#.to_string()));
        let x = texts(&t.frame());
        assert!(x.contains(&"stopped: you stopped it".to_string()) && !x.contains(&"Stop".to_string()), "{x:?}");
        assert!(!t.event(&Event::Hit { id: 9 }), "a run that is over cannot be stopped again");
        // on the list there is no Stop, and a refusal is one red line
        let mut t = tile(vec![Ok(list_reply(1))]);
        assert!(!hits(&t.frame()).contains(&9) && !t.event(&Event::Hit { id: 9 }));
        let mut t = tile(vec![Ok(list_reply(2)), Ok(json!({"id": "r0", "status": "running", "lines": [], "pending": []}).to_string()), Err("no such run".into())]);
        t.event(&Event::Hit { id: 100 });
        t.event(&Event::Hit { id: 9 });
        assert_eq!(t.error, "no such run");
    }
}
