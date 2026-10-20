//! The AI IDE's panes: where each one goes at any window size. Pure functions (no drawing), so the shell, the tests and the
//! saved layouts agree. Sizes in `Prefs` are in unscaled units (they are multiplied by the UI scale when laid out).
//!
//! Wide windows (1280x720 and up) show a workspace rail, the file tree, the editor with a bottom panel, and the chat side by
//! side. Narrow ones (about 1024x600) show one pane at a time with a strip of pane tabs.

use serde::{Deserialize, Serialize};

use crate::ui::R;

/// The panes the keyboard and the mouse can focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Tree,
    Main,
    Chat,
    Bottom,
}

impl Pane {
    pub const ALL: [Pane; 4] = [Pane::Tree, Pane::Main, Pane::Chat, Pane::Bottom];
    pub fn name(self) -> &'static str {
        match self {
            Pane::Tree => "Workspace",
            Pane::Main => "Editor",
            Pane::Chat => "Chat",
            Pane::Bottom => "Panel",
        }
    }
    pub fn code(self) -> u8 {
        match self {
            Pane::Tree => 1,
            Pane::Main => 2,
            Pane::Chat => 3,
            Pane::Bottom => 4,
        }
    }
    pub fn from_code(c: usize) -> Option<Pane> {
        Pane::ALL.into_iter().find(|p| p.code() as usize == c)
    }
    pub fn next(self) -> Pane {
        match self {
            Pane::Tree => Pane::Main,
            Pane::Main => Pane::Bottom,
            Pane::Bottom => Pane::Chat,
            Pane::Chat => Pane::Tree,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub sidebar_w: f32,
    pub chat_w: f32,
    pub bottom_h: f32,
    pub sidebar_open: bool,
    pub chat_open: bool,
    pub bottom_open: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sidebar_w: 236.0, chat_w: 400.0, bottom_h: 230.0, sidebar_open: true, chat_open: true, bottom_open: true }
    }
}

pub const MIN_SIDEBAR: f32 = 150.0;
pub const MIN_CHAT: f32 = 270.0;
pub const MIN_MAIN: f32 = 300.0;
pub const MIN_BOTTOM: f32 = 110.0;
pub const MIN_MAIN_H: f32 = 140.0;

/// Which divider a drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Div {
    /// Between the tree and the editor.
    Side,
    /// Between the editor and the chat.
    Chat,
    /// Between the editor and the bottom panel.
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub compact: bool,
    pub top: R,
    pub rail: R,
    pub sidebar: Option<R>,
    pub main: R,
    pub chat: Option<R>,
    pub bottom: Option<R>,
    pub status: R,
    /// The strip of pane tabs (compact windows only).
    pub pane_tabs: Option<R>,
    pub dividers: Vec<(Div, R)>,
}

/// Compact when the window is narrower than about 1100 px, or the text size leaves under 1100 units of width.
pub fn is_compact(area: R, s: f32) -> bool {
    area.w < 1100.0 || area.w / s < 1100.0
}

/// Lay the panes out in `area` (below the app header). `shown` is the pane a compact window shows.
pub fn compute(area: R, s: f32, p: &Prefs, shown: Pane) -> Layout {
    let compact = is_compact(area, s);
    let m = super::ide_chrome::cur();
    let top_h = m.top_h * s;
    let status_h = m.status_h * s;
    let rail_w = m.rail_w * s;
    let top = R::new(area.x, area.y, area.w, top_h);
    let status = R::new(area.x, area.y + area.h - status_h, area.w, status_h);
    let rail = R::new(area.x, area.y + top_h, rail_w, (area.h - top_h - status_h).max(0.0));
    let body = R::new(area.x + rail_w, area.y + top_h, (area.w - rail_w).max(0.0), (area.h - top_h - status_h).max(0.0));
    let div = m.divider * s;
    if compact {
        let tabs_h = m.tab_h * s;
        let pane_tabs = R::new(body.x, body.y, body.w, tabs_h);
        let content = R::new(body.x, body.y + tabs_h, body.w, (body.h - tabs_h).max(0.0));
        let nothing = R::new(content.x, content.y, 0.0, 0.0);
        let mut l = Layout { compact, top, rail, sidebar: None, main: nothing, chat: None, bottom: None, status, pane_tabs: Some(pane_tabs), dividers: vec![] };
        match shown {
            Pane::Tree => l.sidebar = Some(content),
            Pane::Chat => l.chat = Some(content),
            Pane::Bottom => l.bottom = Some(content),
            Pane::Main => l.main = content,
        }
        return l;
    }
    // widths: the editor keeps its minimum; the side panes give way first
    let mut sw = if p.sidebar_open { (p.sidebar_w * s).max(MIN_SIDEBAR * s) } else { 0.0 };
    let mut cw = if p.chat_open { (p.chat_w * s).max(MIN_CHAT * s) } else { 0.0 };
    let min_main = MIN_MAIN * s;
    let mut over = sw + cw + min_main - body.w;
    if over > 0.0 {
        let cut = over.min(cw - if p.chat_open { MIN_CHAT * s } else { 0.0 }).max(0.0);
        cw -= cut;
        over -= cut;
    }
    if over > 0.0 {
        let cut = over.min(sw - if p.sidebar_open { MIN_SIDEBAR * s } else { 0.0 }).max(0.0);
        sw -= cut;
    }
    let mut x = body.x;
    let sidebar = (sw > 0.0).then(|| R::new(x, body.y, sw - 2.0 * s, body.h));
    x += sw;
    let main_w = (body.w - sw - cw).max(0.0);
    let col = R::new(x, body.y, main_w, body.h);
    let chat = (cw > 0.0).then(|| R::new(body.x + body.w - cw + 2.0 * s, body.y, cw - 2.0 * s, body.h));
    let mut bh = if p.bottom_open { (p.bottom_h * s).max(MIN_BOTTOM * s) } else { 0.0 };
    bh = bh.min((col.h - MIN_MAIN_H * s).max(0.0));
    let (main, bottom) = if bh >= 60.0 * s {
        (R::new(col.x, col.y, col.w, col.h - bh), Some(R::new(col.x, col.y + col.h - bh + 2.0 * s, col.w, bh - 2.0 * s)))
    } else {
        (col, None)
    };
    let mut dividers = Vec::new();
    if let Some(sb) = &sidebar {
        dividers.push((Div::Side, R::new(sb.x + sb.w - div / 2.0 + 1.0 * s, body.y, div, body.h)));
    }
    if let Some(c) = &chat {
        dividers.push((Div::Chat, R::new(c.x - div / 2.0 - 1.0 * s, body.y, div, body.h)));
    }
    if let Some(b) = &bottom {
        dividers.push((Div::Bottom, R::new(col.x, b.y - div / 2.0 - 1.0 * s, col.w, div)));
    }
    Layout { compact, top, rail, sidebar, main, chat, bottom, status, pane_tabs: None, dividers }
}

impl Prefs {
    /// Move a divider so it sits at pixel `pos` (x for the vertical ones, y for the bottom one) in a window laid out in `area`.
    pub fn drag(&mut self, d: Div, pos: f32, area: R, s: f32) {
        let m = super::ide_chrome::cur();
        let rail_w = m.rail_w * s;
        let body_x = area.x + rail_w;
        let body_w = area.w - rail_w;
        match d {
            Div::Side => {
                let max = (body_w - if self.chat_open { self.chat_w * s } else { 0.0 } - MIN_MAIN * s).max(MIN_SIDEBAR * s);
                self.sidebar_w = ((pos - body_x).clamp(MIN_SIDEBAR * s, max)) / s;
            }
            Div::Chat => {
                let right = body_x + body_w;
                let max = (body_w - if self.sidebar_open { self.sidebar_w * s } else { 0.0 } - MIN_MAIN * s).max(MIN_CHAT * s);
                self.chat_w = ((right - pos).clamp(MIN_CHAT * s, max)) / s;
            }
            Div::Bottom => {
                let bottom = area.y + area.h - m.status_h * s;
                let max = (area.h - m.top_h * s - m.status_h * s - MIN_MAIN_H * s).max(MIN_BOTTOM * s);
                self.bottom_h = ((bottom - pos).clamp(MIN_BOTTOM * s, max)) / s;
            }
        }
    }

    /// Grow or shrink a pane by `delta` unscaled units (the keyboard way to resize).
    pub fn nudge(&mut self, pane: Pane, delta: f32) {
        match pane {
            Pane::Tree => self.sidebar_w = (self.sidebar_w + delta).clamp(MIN_SIDEBAR, 600.0),
            Pane::Chat => self.chat_w = (self.chat_w + delta).clamp(MIN_CHAT, 900.0),
            Pane::Bottom => self.bottom_h = (self.bottom_h + delta).clamp(MIN_BOTTOM, 700.0),
            Pane::Main => {}
        }
    }

    pub fn toggle(&mut self, pane: Pane) {
        match pane {
            Pane::Tree => self.sidebar_open = !self.sidebar_open,
            Pane::Chat => self.chat_open = !self.chat_open,
            Pane::Bottom => self.bottom_open = !self.bottom_open,
            Pane::Main => {}
        }
    }

    pub fn is_open(&self, pane: Pane) -> bool {
        match pane {
            Pane::Tree => self.sidebar_open,
            Pane::Chat => self.chat_open,
            Pane::Bottom => self.bottom_open,
            Pane::Main => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &R, b: &R) -> bool {
        a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
    }

    fn rects(l: &Layout) -> Vec<(&'static str, R)> {
        let mut v = vec![("top", l.top), ("rail", l.rail), ("main", l.main), ("status", l.status)];
        if let Some(r) = l.sidebar {
            v.push(("sidebar", r));
        }
        if let Some(r) = l.chat {
            v.push(("chat", r));
        }
        if let Some(r) = l.bottom {
            v.push(("bottom", r));
        }
        if let Some(r) = l.pane_tabs {
            v.push(("tabs", r));
        }
        v
    }

    #[test]
    fn the_three_sizes_lay_out_without_overlap_and_inside_the_window() {
        for (w, h, s) in [(1280.0f32, 720.0f32, 1.0f32), (1920.0, 1080.0, 1.5), (1024.0, 600.0, 0.8), (1100.0, 700.0, 1.0), (800.0, 480.0, 0.6)] {
            let area = R::new(0.0, 46.0 * s, w, h - 46.0 * s);
            for shown in Pane::ALL {
                let l = compute(area, s, &Prefs::default(), shown);
                let rs = rects(&l);
                for (n, r) in &rs {
                    assert!(r.w >= 0.0 && r.h >= 0.0 && r.x >= -0.5 && r.y >= area.y - 0.5 && r.x + r.w <= w + 0.5 && r.y + r.h <= h + 0.5, "{n} {r:?} at {w}x{h}");
                }
                for (i, (na, a)) in rs.iter().enumerate() {
                    for (nb, b) in &rs[i + 1..] {
                        assert!(!overlaps(a, b), "{na} {a:?} overlaps {nb} {b:?} at {w}x{h} showing {shown:?}");
                    }
                }
                if !l.compact {
                    assert!(l.main.w >= MIN_MAIN * s - 0.5, "the editor keeps room at {w}x{h}: {:?}", l.main);
                    assert!(l.sidebar.is_some() && l.chat.is_some() && l.bottom.is_some());
                } else {
                    assert!(l.pane_tabs.is_some() && (l.sidebar.is_some() as u8 + l.chat.is_some() as u8 + l.bottom.is_some() as u8) <= 1);
                }
            }
        }
        assert!(is_compact(R::new(0.0, 0.0, 1024.0, 600.0), 0.8));
        assert!(!is_compact(R::new(0.0, 0.0, 1280.0, 720.0), 1.0));
        assert!(!is_compact(R::new(0.0, 0.0, 1920.0, 1080.0), 1.5));
    }

    #[test]
    fn panes_collapse_and_keep_minimum_sizes() {
        let area = R::new(0.0, 46.0, 1280.0, 674.0);
        let m = super::super::ide_chrome::cur();
        let mut p = Prefs::default();
        p.sidebar_open = false;
        p.chat_open = false;
        p.bottom_open = false;
        let l = compute(area, 1.0, &p, Pane::Main);
        assert!(l.sidebar.is_none() && l.chat.is_none() && l.bottom.is_none() && l.dividers.is_empty());
        assert!((l.main.w - (1280.0 - m.rail_w)).abs() < 0.5);
        // huge preferences give way to the editor's minimum
        let big = Prefs { sidebar_w: 500.0, chat_w: 800.0, bottom_h: 900.0, ..Default::default() };
        let l = compute(area, 1.0, &big, Pane::Main);
        assert!(l.main.w >= MIN_MAIN - 0.5 && l.main.h >= MIN_MAIN_H - 0.5, "{:?}", l.main);
        assert!(l.sidebar.unwrap().w >= MIN_SIDEBAR - 3.0 && l.chat.unwrap().w >= MIN_CHAT - 3.0);
        assert_eq!(l.dividers.len(), 3);
    }

    #[test]
    fn dragging_and_nudging_resize_within_limits_and_toggling_works() {
        let area = R::new(0.0, 46.0, 1280.0, 674.0);
        let m = super::super::ide_chrome::cur();
        let mut p = Prefs::default();
        p.drag(Div::Side, 400.0, area, 1.0);
        assert!((p.sidebar_w - (400.0 - m.rail_w)).abs() < 0.5);
        p.drag(Div::Side, 5.0, area, 1.0);
        assert_eq!(p.sidebar_w, MIN_SIDEBAR);
        p.drag(Div::Chat, 900.0, area, 1.0);
        assert!((p.chat_w - 380.0).abs() < 0.5);
        p.drag(Div::Chat, 1275.0, area, 1.0);
        assert_eq!(p.chat_w, MIN_CHAT);
        p.drag(Div::Bottom, 400.0, area, 1.0);
        assert!((p.bottom_h - (720.0 - m.status_h - 400.0)).abs() < 0.5);
        p.drag(Div::Bottom, 100.0, area, 1.0);
        assert!(p.bottom_h <= 720.0 - m.top_h - m.status_h - MIN_MAIN_H + 0.5);
        let before = p.chat_w;
        p.nudge(Pane::Chat, 40.0);
        assert_eq!(p.chat_w, before + 40.0);
        p.nudge(Pane::Tree, -1000.0);
        assert_eq!(p.sidebar_w, MIN_SIDEBAR);
        assert!(p.is_open(Pane::Chat));
        p.toggle(Pane::Chat);
        assert!(!p.is_open(Pane::Chat));
        // the dividers sit on the pane edges
        let l = compute(area, 1.0, &Prefs::default(), Pane::Main);
        let side = l.dividers.iter().find(|d| d.0 == Div::Side).unwrap().1;
        assert!((side.x + side.w / 2.0 - l.sidebar.unwrap().x - l.sidebar.unwrap().w).abs() < 3.0);
        let json = serde_json::to_string(&Prefs::default()).unwrap();
        assert_eq!(serde_json::from_str::<Prefs>(&json).unwrap(), Prefs::default());
        assert_eq!(serde_json::from_str::<Prefs>("{}").unwrap(), Prefs::default(), "missing fields take their defaults");
    }
}
