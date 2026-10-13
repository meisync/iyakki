//! The boot hardware check: what the machine reports (display, keyboard, mouse, touch, network, …) plus a
//! live tester for the things a person has to verify with their hands: every key, every pointer move and
//! tap, and eight tap targets at the corners and edges of the screen (they catch a touch panel that is
//! offset or only covers part of a 4K display).
//!
//! It runs before login (kernel option `iyakki.hwcheck=1`, or the "hardware check" boot menu entry), reads
//! only `/proc` and `/sys`, writes nothing unless you press S, and leaves with Enter.

use std::collections::VecDeque;

use iyakki_hwinfo::{collect, Roots};

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

pub const TARGETS: usize = 8;
const ACCENT: [u8; 4] = rgb(110, 224, 160);
pub const REPORT_FILE: &str = "/var/lib/iyakki/hwreport.txt";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Continue,
}

pub struct HwCheck {
    roots: Roots,
    text: String,
    attention: usize,
    scroll: usize,
    hit: [bool; TARGETS],
    log: VecDeque<String>,
    pointer: (i32, i32),
    moves: u64,
    clicks: u64,
    keys: u64,
    status: Option<(String, bool)>,
    report_path: String,
}

impl Default for HwCheck {
    fn default() -> Self {
        Self::new()
    }
}

/// Positions of the eight tap targets as fractions of the screen: corners and edge midpoints.
fn target_rects(w: f32, h: f32, t: f32, m: f32) -> [R; TARGETS] {
    let (l, r, top, b) = (m, w - m - t, m, h - m - t);
    let (cx, cy) = ((w - t) / 2.0, (h - t) / 2.0);
    [R::new(l, top, t, t), R::new(cx, top, t, t), R::new(r, top, t, t), R::new(l, cy, t, t), R::new(r, cy, t, t), R::new(l, b, t, t), R::new(cx, b, t, t), R::new(r, b, t, t)]
}

fn describe(ev: &Event) -> Option<String> {
    Some(match ev {
        Event::Key(Key::Char(c)) => format!("key  '{c}'"),
        Event::Key(Key::Ctrl(c)) => format!("key  Ctrl+{c}"),
        Event::Key(Key::Nav { nav, shift, ctrl }) => format!("key  {nav:?}{}{}", if *shift { " +Shift" } else { "" }, if *ctrl { " +Ctrl" } else { "" }),
        Event::Key(k) => format!("key  {k:?}"),
        Event::MouseDown(x, y) => format!("tap/click at {x}, {y}"),
        Event::MouseRight(x, y) => format!("right-click at {x}, {y}"),
        Event::Scroll(d) => format!("scroll {d:+}"),
        Event::Paste(_) => "paste".into(),
        _ => return None,
    })
}

impl HwCheck {
    pub fn new() -> HwCheck {
        Self::with_roots(Roots::real(), REPORT_FILE.into())
    }

    pub fn with_roots(roots: Roots, report_path: String) -> HwCheck {
        let mut h = HwCheck { roots, text: String::new(), attention: 0, scroll: 0, hit: [false; TARGETS], log: VecDeque::new(), pointer: (0, 0), moves: 0, clicks: 0, keys: 0, status: None, report_path };
        h.refresh();
        h
    }

    pub fn refresh(&mut self) {
        let rep = collect(&self.roots);
        self.attention = rep.warnings.len();
        self.text = rep.to_text();
    }

    pub fn report_text(&self) -> &str {
        &self.text
    }
    pub fn targets_hit(&self) -> usize {
        self.hit.iter().filter(|h| **h).count()
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }
    pub fn counts(&self) -> (u64, u64, u64) {
        (self.keys, self.moves, self.clicks)
    }

    /// Record an input event for the live tester (called for every event, whatever else handles it).
    pub fn note(&mut self, ev: &Event) {
        match ev {
            Event::MouseMove(x, y) => {
                self.pointer = (*x, *y);
                self.moves += 1;
            }
            Event::MouseDown(x, y) | Event::MouseRight(x, y) => {
                self.pointer = (*x, *y);
                self.clicks += 1;
            }
            Event::Key(_) => self.keys += 1,
            _ => {}
        }
        if let Some(d) = describe(ev) {
            if self.log.len() >= 9 {
                self.log.pop_front();
            }
            self.log.push_back(d);
        }
    }

    pub fn handle(&mut self, ev: &Event) -> Outcome {
        match ev {
            Event::Row(i) if *i < TARGETS => self.hit[*i] = true,
            Event::Key(Key::Enter) | Event::Key(Key::Esc) => return Outcome::Continue,
            Event::Key(Key::Char('r')) => {
                self.refresh();
                self.status = Some(("Report refreshed.".into(), true));
            }
            Event::Key(Key::Char('s')) => {
                self.status = Some(match std::fs::write(&self.report_path, &self.text) {
                    Ok(()) => (format!("Saved to {}", self.report_path), true),
                    Err(e) => (format!("Could not save ({e}). Photograph the screen instead."), false),
                });
            }
            Event::Key(Key::Char('c')) => {
                self.hit = [false; TARGETS];
                self.status = Some(("Targets cleared.".into(), true));
            }
            Event::Key(Key::Down) => self.scroll += 3,
            Event::Key(Key::Up) => self.scroll = self.scroll.saturating_sub(3),
            Event::Key(Key::PageDown) => self.scroll += 15,
            Event::Key(Key::PageUp) => self.scroll = self.scroll.saturating_sub(15),
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).max(0) as usize,
            _ => {}
        }
        Outcome::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32, tick: u64, hostname: &str) {
        let (wf, hf) = (w as f32, h as f32);
        let t = 78.0 * s;
        let m = 10.0 * s;
        // tap targets last (on top), but their regions are registered first so panels never steal taps
        let rects = target_rects(wf, hf, t, m);
        let inner_x = t + m + 18.0 * s;
        let inner_w = wf - 2.0 * inner_x;
        let size = 15.0 * s;
        g.text(inner_x, m + 6.0 * s, 28.0 * s, TEXT, "IYAKKI hardware check", Face::Bold);
        let sub = format!("{}  ·  screen {}x{} (UI scale {:.2})", hostname, w, h, s);
        g.text(inner_x, m + 44.0 * s, 15.0 * s, MUTED, &sub, Face::Sans);
        let top = m + 78.0 * s;
        let bottom = hf - t - m - 12.0 * s;
        // report (left)
        let left_w = inner_w * 0.58;
        crate::fx::panel(g, R::new(inner_x, top, left_w, bottom - top), s, ACCENT);
        let cw = g.text_width(size * 0.92, "M", Face::Mono).max(1.0);
        let cols = (((left_w - 24.0 * s) / cw) as usize).max(20);
        let lh = g.line_height(size * 0.92, Face::Mono);
        let rows = (((bottom - top - 20.0 * s) / lh) as usize).max(1);
        let mut lines: Vec<(String, [u8; 4])> = Vec::new();
        let mut attention = true;
        for l in self.text.lines() {
            if l.is_empty() {
                attention = false;
            }
            let col = if l.starts_with("  !") { rgb(255, 196, 94) } else if !l.starts_with(' ') && !l.is_empty() { ACCENT } else { TEXT };
            let _ = attention;
            // wrap long lines
            let chars: Vec<char> = l.chars().collect();
            if chars.is_empty() {
                lines.push((String::new(), col));
            }
            for chunk in chars.chunks(cols) {
                lines.push((chunk.iter().collect(), col));
            }
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(rows));
        for (i, (l, col)) in lines.iter().skip(self.scroll).take(rows).enumerate() {
            g.text(inner_x + 12.0 * s, top + 10.0 * s + i as f32 * lh, size * 0.92, *col, l, Face::Mono);
        }
        if lines.len() > rows {
            let th = ((bottom - top) * rows as f32 / lines.len() as f32).max(24.0 * s);
            let ty = top + (bottom - top - th) * self.scroll as f32 / (lines.len() - rows).max(1) as f32;
            g.rrect(inner_x + left_w - 9.0 * s, ty, 5.0 * s, th, 2.5 * s, rgb(52, 62, 100));
        }
        // live tester (right)
        let rx = inner_x + left_w + 16.0 * s;
        let rw = inner_w - left_w - 16.0 * s;
        crate::fx::card(g, R::new(rx, top, rw, bottom - top), s, ACCENT, 0.0);
        let mut y = top + 14.0 * s;
        g.text(rx + 14.0 * s, y, 17.0 * s, ACCENT, "Try your devices", Face::Bold);
        y += 30.0 * s;
        g.text(rx + 14.0 * s, y, 14.0 * s, MUTED, "Type, move the mouse, tap or click anywhere.", Face::Sans);
        y += 22.0 * s;
        g.text(rx + 14.0 * s, y, 14.0 * s, MUTED, &format!("Tap the {} squares at the corners and edges: each turns green.", TARGETS), Face::Sans);
        y += 34.0 * s;
        g.text(rx + 14.0 * s, y, 15.0 * s, TEXT, &format!("pointer  {}, {}", self.pointer.0, self.pointer.1), Face::Mono);
        y += 22.0 * s;
        g.text(rx + 14.0 * s, y, 15.0 * s, TEXT, &format!("keys {}   moves {}   taps {}", self.keys, self.moves, self.clicks), Face::Mono);
        y += 22.0 * s;
        g.text(rx + 14.0 * s, y, 15.0 * s, if self.targets_hit() == TARGETS { OK } else { TEXT }, &format!("targets {} / {}", self.targets_hit(), TARGETS), Face::Mono);
        y += 34.0 * s;
        g.text(rx + 14.0 * s, y, 14.0 * s, DIM, "latest events", Face::Bold);
        y += 22.0 * s;
        for l in self.log.iter().rev() {
            if y + 18.0 * s > bottom - 100.0 * s {
                break;
            }
            g.text(rx + 14.0 * s, y, 14.0 * s, MUTED, l, Face::Mono);
            y += 19.0 * s;
        }
        // footer: the buttons that fit left of the bottom-centre target
        let fy = hf - t - m + 4.0 * s;
        let mut bx = inner_x;
        for (label, col, key) in [("Continue (Enter)", OK, Key::Enter), ("Save report (S)", ACCENT, Key::Char('s')), ("Refresh (R)", TEXT, Key::Char('r'))] {
            bx += g.button(bx, fy, 15.0 * s, label, col, Event::Key(key)) + 10.0 * s;
        }
        // right panel: clear button and status
        g.button(rx + 14.0 * s, bottom - 78.0 * s, 14.0 * s, "Clear targets (C)", MUTED, Event::Key(Key::Char('c')));
        let status_y = bottom - 30.0 * s;
        if let Some((msg, ok)) = &self.status {
            let msg = g.fit(14.0 * s, msg, Face::Sans, rw - 28.0 * s);
            g.text(rx + 14.0 * s, status_y, 14.0 * s, if *ok { OK } else { ERR }, &msg, Face::Sans);
        } else if self.attention > 0 {
            let msg = g.fit(14.0 * s, &format!("{} item(s) need attention (see the report)", self.attention), Face::Sans, rw - 28.0 * s);
            g.text(rx + 14.0 * s, status_y, 14.0 * s, rgb(255, 196, 94), &msg, Face::Sans);
        }
        for (i, r) in rects.iter().enumerate() {
            let done = self.hit[i];
            let hot = g.hovered(*r);
            if done {
                g.rrect_glow(r.x, r.y, r.w, r.h, 14.0 * s, 5.0 * s, [OK[0], OK[1], OK[2], 70]);
            }
            g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, if done { rgb(30, 100, 72) } else if hot { rgb(52, 62, 100) } else { rgb(30, 37, 62) });
            g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 2.0 * s, if done { OK } else if hot { ACCENT } else { rgb(90, 104, 150) });
            let label = if done { "✓".to_string() } else { (i + 1).to_string() };
            let lw = g.text_width(26.0 * s, &label, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 30.0 * s) / 2.0, 26.0 * s, TEXT, &label, Face::Bold);
            g.hit(*r, Event::Row(i), &format!("target {}", i + 1));
        }
        let _ = tick;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(tag: &str) -> HwCheck {
        let d = std::env::temp_dir().join(format!("iyakki-hwcheck-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sys")).unwrap();
        std::fs::create_dir_all(d.join("proc")).unwrap();
        HwCheck::with_roots(Roots { sys: d.join("sys"), proc: d.join("proc") }, d.join("report.txt").display().to_string())
    }

    #[test]
    fn the_report_flags_what_is_missing() {
        let h = check("flags");
        assert!(h.report_text().starts_with("ATTENTION") && h.report_text().contains("No keyboard"));
        assert_eq!(h.attention, 3);
    }

    #[test]
    fn events_are_counted_logged_and_capped() {
        let mut h = check("events");
        for i in 0..30 {
            h.note(&Event::Key(Key::Char((b'a' + (i % 26) as u8) as char)));
        }
        h.note(&Event::MouseMove(100, 200));
        h.note(&Event::MouseMove(300, 400));
        h.note(&Event::MouseDown(300, 400));
        h.note(&Event::Key(Key::Nav { nav: crate::ui::Nav::Left, shift: true, ctrl: false }));
        h.note(&Event::Tick);
        assert_eq!(h.counts(), (31, 2, 1));
        assert_eq!(h.pointer, (300, 400));
        assert_eq!(h.log.len(), 9, "the log keeps the latest nine lines");
        assert_eq!(h.log.back().unwrap(), "key  Left +Shift");
        assert!(h.log.iter().any(|l| l == "tap/click at 300, 400"));
        assert!(!h.log.iter().any(|l| l.contains("moves")), "pointer motion is counted, not logged line by line");
    }

    #[test]
    fn targets_keys_scrolling_saving_and_leaving() {
        let mut h = check("keys");
        assert_eq!(h.handle(&Event::Row(0)), Outcome::Stay);
        assert_eq!(h.handle(&Event::Row(7)), Outcome::Stay);
        assert_eq!(h.handle(&Event::Row(8)), Outcome::Stay);
        assert_eq!(h.targets_hit(), 2, "an out-of-range target is ignored");
        h.handle(&Event::Key(Key::Char('c')));
        assert_eq!(h.targets_hit(), 0);
        h.handle(&Event::Key(Key::Down));
        assert_eq!(h.scroll, 3);
        h.handle(&Event::Key(Key::PageUp));
        assert_eq!(h.scroll, 0);
        h.handle(&Event::Scroll(-2));
        assert_eq!(h.scroll, 6);
        h.handle(&Event::Key(Key::Char('s')));
        let (msg, ok) = h.status().unwrap().clone();
        assert!(ok && msg.starts_with("Saved to"), "{msg}");
        assert_eq!(std::fs::read_to_string(&h.report_path).unwrap(), h.report_text());
        h.report_path = "/nonexistent-dir/x.txt".into();
        h.handle(&Event::Key(Key::Char('s')));
        assert!(!h.status().unwrap().1 && h.status().unwrap().0.contains("Photograph"));
        h.handle(&Event::Key(Key::Char('r')));
        assert_eq!(h.status().unwrap().0, "Report refreshed.");
        assert_eq!(h.handle(&Event::Key(Key::Enter)), Outcome::Continue);
        assert_eq!(h.handle(&Event::Key(Key::Esc)), Outcome::Continue);
    }

    #[test]
    fn targets_cover_corners_and_edges_at_any_size() {
        for (w, h) in [(1280.0f32, 720.0f32), (1920.0, 1080.0), (3840.0, 2160.0)] {
            let t = 78.0 * (w / 1280.0);
            let r = target_rects(w, h, t, 10.0);
            assert_eq!(r.len(), 8);
            assert!(r.iter().all(|x| x.x >= 0.0 && x.y >= 0.0 && x.x + x.w <= w && x.y + x.h <= h), "inside the screen at {w}x{h}");
            assert_eq!((r[0].x, r[0].y), (10.0, 10.0), "top-left corner");
            assert_eq!((r[7].x + r[7].w, r[7].y + r[7].h), (w - 10.0, h - 10.0), "bottom-right corner");
        }
    }

    #[test]
    fn it_renders_at_phone_to_4k_sizes_with_clickable_targets() {
        let mut h = check("render");
        h.note(&Event::Key(Key::Char('x')));
        h.handle(&Event::Row(3));
        for (w, ht) in [(640u32, 480u32), (1280, 720), (1920, 1080), (3840, 2160)] {
            let s = (w as f32 / 1280.0).min(ht as f32 / 720.0).clamp(0.6, 3.0);
            let mut g = Gfx::new(w, ht);
            g.clear(rgb(0, 0, 0));
            h.render(&mut g, w, ht, s, 0, "iyakki-test");
            assert_eq!(g.hits.iter().filter(|x| x.label.starts_with("target ")).count(), 8, "{w}x{ht}");
            assert!(g.hits.iter().any(|x| x.label == "Continue (Enter)"));
        }
    }
}
