// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menu state machine. Takes input events, returns what to draw. No drawing code.
//!
//! Units are the front end's choice (pixels for a framebuffer, character cells for text); the layout says which.

use crate::item::Item;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left.
    pub x: u32,
    /// Top.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl Rect {
    /// Is the point inside?
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// Screen geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Screen width.
    pub width: u32,
    /// Screen height.
    pub height: u32,
    /// Space above the list for the title.
    pub header_h: u32,
    /// Space below the list for the countdown and notices.
    pub footer_h: u32,
    /// Height of one row (a big touch target on a framebuffer).
    pub row_h: u32,
    /// Left and right margin.
    pub margin: u32,
}

impl Layout {
    fn visible_rows(&self) -> usize {
        let room = self.height.saturating_sub(self.header_h + self.footer_h);
        ((room / self.row_h.max(1)) as usize).max(1)
    }
}

/// One listed row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Index into the item list given to the front end's start code.
    pub item: usize,
    /// Text.
    pub label: String,
    /// Reason shown when the row is greyed out and cannot be chosen.
    pub disabled: Option<String>,
}

/// Rows for the listed (not hidden) items.
pub fn rows_from_items(items: &[Item]) -> Vec<Row> {
    items
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.hidden)
        .map(|(n, i)| Row { item: n, label: i.title.clone(), disabled: i.disabled.map(|d| String::from(d.text())) })
        .collect()
}

/// Input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Enter.
    Enter,
    /// Escape.
    Esc,
    /// A key or touch that only woke the screen (it showed the hidden menu): the countdown stops, nothing else changes. Without it the first key
    /// of a quiet splash would reveal the rows while the countdown kept running and started the system a moment later.
    Wake,
    /// Touch or mouse press at a point.
    Tap {
        /// X.
        x: u32,
        /// Y.
        y: u32,
    },
    /// Time passed.
    Tick {
        /// Milliseconds.
        ms: u32,
    },
}

/// What the front end must do after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Redraw if needed, nothing else.
    None,
    /// Start this item (index as in `Row::item`).
    Boot(usize),
    /// The person pressed Escape.
    Cancel,
}

/// One line to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Text, with the reason added for a greyed row.
    pub text: String,
    /// Where it goes (also the tap target).
    pub rect: Rect,
    /// Highlighted.
    pub selected: bool,
    /// Greyed out.
    pub disabled: bool,
}

/// Everything to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Header text.
    pub header: String,
    /// Visible rows.
    pub lines: Vec<Line>,
    /// Countdown or notice.
    pub footer: Option<String>,
    /// Rows exist above.
    pub more_above: bool,
    /// Rows exist below.
    pub more_below: bool,
}

/// The menu.
#[derive(Debug, Clone)]
pub struct Menu {
    rows: Vec<Row>,
    layout: Layout,
    selected: usize,
    scroll: usize,
    remaining_ms: Option<u32>,
    header: String,
    notice: Option<String>,
    waiting: bool,
}

impl Menu {
    /// Create. `selected_item` is an item index (the default); `timeout_secs` starts a countdown.
    pub fn new(rows: Vec<Row>, layout: Layout, selected_item: usize, timeout_secs: Option<u32>, header: &str, notice: Option<&str>) -> Menu {
        let first_ok = rows.iter().position(|r| r.disabled.is_none()).unwrap_or(0);
        let selected = rows.iter().position(|r| r.item == selected_item && r.disabled.is_none()).unwrap_or(first_ok);
        let mut m = Menu {
            rows,
            layout,
            selected,
            scroll: 0,
            remaining_ms: timeout_secs.map(|s| s.saturating_mul(1000)),
            header: header.into(),
            notice: notice.map(Into::into),
            waiting: false,
        };
        m.keep_in_view();
        m
    }

    /// A menu that waits for the person (boot assessment said stop): no countdown at all, and `why` stays on the notice line
    /// whatever is pressed. Time never starts anything.
    pub fn new_waiting(rows: Vec<Row>, layout: Layout, selected_item: usize, header: &str, why: &str) -> Menu {
        let mut m = Menu::new(rows, layout, selected_item, None, header, Some(why));
        m.waiting = true;
        m
    }

    /// Does this menu wait for ever?
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    /// Item index of the selected row (`None` for an empty menu).
    pub fn selected_item(&self) -> Option<usize> {
        self.rows.get(self.selected).map(|r| r.item)
    }

    /// Is a countdown running?
    pub fn counting(&self) -> bool {
        self.remaining_ms.is_some()
    }

    fn enabled(&self, i: usize) -> bool {
        self.rows.get(i).is_some_and(|r| r.disabled.is_none())
    }

    fn keep_in_view(&mut self) {
        let vis = self.layout.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + vis {
            self.scroll = self.selected + 1 - vis;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(vis));
    }

    /// Nearest enabled row at or after `from` going `down` (or up), else the other way.
    fn nearest(&self, from: usize, down: bool) -> Option<usize> {
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let from = from.min(n - 1);
        let fwd: Option<usize> = if down { (from..n).find(|&i| self.enabled(i)) } else { (0..=from).rev().find(|&i| self.enabled(i)) };
        fwd.or_else(|| if down { (0..from).rev().find(|&i| self.enabled(i)) } else { (from + 1..n).find(|&i| self.enabled(i)) })
    }

    /// Feed one event.
    pub fn handle(&mut self, input: Input) -> Event {
        if let Input::Tick { ms } = input {
            if let Some(left) = self.remaining_ms {
                if left <= ms {
                    self.remaining_ms = None;
                    return self.selected_item().filter(|_| self.enabled(self.selected)).map_or(Event::None, Event::Boot);
                }
                self.remaining_ms = Some(left - ms);
            }
            return Event::None;
        }
        self.remaining_ms = None; // any other input stops the countdown
        let vis = self.layout.visible_rows();
        let n = self.rows.len();
        let sel = self.selected;
        match input {
            Input::Up if sel > 0 => self.selected = self.nearest(sel - 1, false).unwrap_or(sel),
            Input::Down if sel + 1 < n => self.selected = self.nearest(sel + 1, true).unwrap_or(sel),
            Input::PageUp => self.selected = self.nearest(sel.saturating_sub(vis), false).unwrap_or(sel),
            Input::PageDown => self.selected = self.nearest((sel + vis).min(n.saturating_sub(1)), true).unwrap_or(sel),
            Input::Home => self.selected = self.nearest(0, true).unwrap_or(sel),
            Input::End => self.selected = self.nearest(n.saturating_sub(1), false).unwrap_or(sel),
            Input::Enter if self.enabled(sel) => return Event::Boot(self.rows[sel].item),
            Input::Esc => return Event::Cancel,
            Input::Tap { x, y } => {
                let hit = self.view().lines.iter().position(|l| l.rect.contains(x, y));
                if let Some(k) = hit {
                    let row = self.scroll + k;
                    if self.enabled(row) {
                        self.selected = row;
                        return Event::Boot(self.rows[row].item);
                    }
                }
            }
            _ => {}
        }
        self.keep_in_view();
        Event::None
    }

    /// What to draw now.
    pub fn view(&self) -> View {
        let l = &self.layout;
        let vis = l.visible_rows();
        let end = (self.scroll + vis).min(self.rows.len());
        let lines = (self.scroll..end)
            .map(|i| {
                let r = &self.rows[i];
                Line {
                    text: match &r.disabled {
                        Some(why) => format!("{} ({})", r.label, why),
                        None => r.label.clone(),
                    },
                    rect: Rect {
                        x: l.margin,
                        y: l.header_h + (i - self.scroll) as u32 * l.row_h,
                        w: l.width.saturating_sub(2 * l.margin),
                        h: l.row_h,
                    },
                    selected: i == self.selected,
                    disabled: r.disabled.is_some(),
                }
            })
            .collect();
        let footer = match (self.remaining_ms, self.rows.get(self.selected)) {
            (Some(ms), Some(r)) => Some(format!("Starting {} in {} s", r.label, ms.div_ceil(1000))),
            _ => self.notice.clone(),
        };
        View { header: self.header.clone(), lines, footer, more_above: self.scroll > 0, more_below: end < self.rows.len() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(rows_visible: u32) -> Layout {
        Layout { width: 100, height: 10 + 20 + rows_visible * 10, header_h: 10, footer_h: 20, row_h: 10, margin: 5 }
    }

    fn rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| Row { item: i + 100, label: format!("r{i}"), disabled: None }).collect()
    }

    fn menu(n: usize, vis: u32, timeout: Option<u32>) -> Menu {
        Menu::new(rows(n), lay(vis), 100, timeout, "Boot", None)
    }

    #[test]
    fn keys_move_and_boot() {
        let mut m = menu(3, 5, None);
        assert_eq!(m.selected_item(), Some(100));
        assert_eq!(m.handle(Input::Up), Event::None);
        assert_eq!(m.selected_item(), Some(100));
        m.handle(Input::Down);
        m.handle(Input::Down);
        m.handle(Input::Down);
        assert_eq!(m.selected_item(), Some(102));
        m.handle(Input::Home);
        assert_eq!(m.selected_item(), Some(100));
        m.handle(Input::End);
        assert_eq!(m.selected_item(), Some(102));
        assert_eq!(m.handle(Input::Enter), Event::Boot(102));
        assert_eq!(m.handle(Input::Esc), Event::Cancel);
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        let mut m = menu(10, 3, None);
        for _ in 0..5 {
            m.handle(Input::Down);
        }
        let v = m.view();
        assert_eq!(v.lines.len(), 3);
        assert!(v.lines.iter().any(|l| l.selected));
        assert_eq!(v.lines[2].text, "r5");
        assert!(v.more_above && v.more_below);
        m.handle(Input::End);
        assert!(!m.view().more_below);
        m.handle(Input::PageUp);
        assert_eq!(m.selected_item(), Some(106));
        m.handle(Input::Home);
        let v = m.view();
        assert!(!v.more_above && v.more_below);
        assert_eq!(v.lines[0].rect, Rect { x: 5, y: 10, w: 90, h: 10 });
        assert_eq!(v.lines[1].rect.y, 20);
    }

    #[test]
    fn tap_boots_row_under_finger() {
        let mut m = menu(4, 3, None);
        assert_eq!(m.handle(Input::Tap { x: 50, y: 25 }), Event::Boot(101));
        assert_eq!(m.handle(Input::Tap { x: 2, y: 25 }), Event::None); // margin
        assert_eq!(m.handle(Input::Tap { x: 50, y: 5 }), Event::None); // header
        assert_eq!(m.handle(Input::Tap { x: 50, y: 45 }), Event::None); // footer
        // after scrolling the first visible row is row 1
        for _ in 0..3 {
            m.handle(Input::Down);
        }
        assert_eq!(m.handle(Input::Tap { x: 50, y: 10 }), Event::Boot(101));
    }

    #[test]
    fn disabled_rows_are_skipped_and_not_startable() {
        let mut r = rows(4);
        r[1].disabled = Some("nope".into());
        r[2].disabled = Some("nope".into());
        let mut m = Menu::new(r.clone(), lay(5), 101, None, "h", None);
        assert_eq!(m.selected_item(), Some(100)); // disabled default -> first enabled
        m.handle(Input::Down);
        assert_eq!(m.selected_item(), Some(103));
        m.handle(Input::Up);
        assert_eq!(m.selected_item(), Some(100));
        assert_eq!(m.handle(Input::Tap { x: 50, y: 20 }), Event::None);
        let v = m.view();
        assert!(v.lines[1].disabled && v.lines[1].text == "r1 (nope)");
        // everything disabled: Enter does nothing
        for x in r.iter_mut() {
            x.disabled = Some("x".into());
        }
        let mut m = Menu::new(r, lay(5), 100, None, "h", None);
        assert_eq!(m.handle(Input::Enter), Event::None);
        assert_eq!(m.handle(Input::Down), Event::None);
    }

    #[test]
    fn countdown_boots_and_input_cancels() {
        let mut m = menu(2, 5, Some(3));
        assert_eq!(m.view().footer.as_deref(), Some("Starting r0 in 3 s"));
        assert_eq!(m.handle(Input::Tick { ms: 1000 }), Event::None);
        assert_eq!(m.view().footer.as_deref(), Some("Starting r0 in 2 s"));
        assert_eq!(m.handle(Input::Tick { ms: 1500 }), Event::None);
        assert_eq!(m.view().footer.as_deref(), Some("Starting r0 in 1 s"));
        assert_eq!(m.handle(Input::Tick { ms: 500 }), Event::Boot(100));
        assert!(!m.counting());
        assert_eq!(m.handle(Input::Tick { ms: 10_000 }), Event::None);

        let mut m = menu(2, 5, Some(3));
        m.handle(Input::Down);
        assert!(!m.counting());
        assert_eq!(m.handle(Input::Tick { ms: 10_000 }), Event::None);
        assert_eq!(m.view().footer, None);

        // a wake (the key that only showed the hidden menu) stops the countdown and moves nothing
        let mut m = menu(2, 5, Some(3));
        let before = m.selected_item();
        assert_eq!(m.handle(Input::Wake), Event::None);
        assert!(!m.counting(), "the countdown is off");
        assert_eq!(m.selected_item(), before, "the choice did not move");
        assert_eq!(m.handle(Input::Tick { ms: 10_000 }), Event::None, "nothing starts by itself any more");
        assert_eq!(m.view().footer, None);
    }

    #[test]
    fn notice_shown_without_countdown() {
        let m = Menu::new(rows(1), lay(2), 100, None, "h", Some("reason text"));
        assert_eq!(m.view().footer.as_deref(), Some("reason text"));
    }

    #[test]
    fn waiting_menu_never_counts_down_and_keeps_notice() {
        let mut m = Menu::new_waiting(rows(3), lay(5), 100, "Boot", "both systems failed");
        assert!(m.waiting() && !m.counting());
        assert_eq!(m.view().footer.as_deref(), Some("both systems failed"));
        assert_eq!(m.handle(Input::Tick { ms: 3_600_000 }), Event::None);
        assert_eq!(m.view().footer.as_deref(), Some("both systems failed"));
        m.handle(Input::Down);
        assert_eq!(m.view().footer.as_deref(), Some("both systems failed"));
        assert_eq!(m.handle(Input::Tick { ms: u32::MAX }), Event::None);
        assert_eq!(m.handle(Input::Enter), Event::Boot(101)); // the person picks
        assert!(!menu(2, 5, None).waiting());
    }

    #[test]
    fn empty_menu_is_safe() {
        let mut m = Menu::new(Vec::new(), lay(2), 0, Some(1), "h", None);
        for i in [Input::Up, Input::Down, Input::PageUp, Input::PageDown, Input::Home, Input::End, Input::Enter, Input::Tap { x: 1, y: 11 }] {
            assert_eq!(m.handle(i), Event::None);
        }
        assert_eq!(m.selected_item(), None);
        assert!(m.view().lines.is_empty());
        let mut m = Menu::new(Vec::new(), lay(2), 0, Some(1), "h", None);
        assert_eq!(m.handle(Input::Tick { ms: 5000 }), Event::None);
    }

    #[test]
    fn tiny_screen_shows_one_row() {
        let l = Layout { width: 10, height: 5, header_h: 10, footer_h: 10, row_h: 10, margin: 0 };
        let mut m = Menu::new(rows(3), l, 100, None, "h", None);
        m.handle(Input::Down);
        assert_eq!(m.view().lines.len(), 1);
        assert!(m.view().lines[0].selected);
    }

    #[test]
    fn rows_skip_hidden_items() {
        use crate::item::{Action, Disabled};
        let mk = |id: &str, hidden, dis| Item {
            id: id.into(),
            title: id.into(),
            hidden,
            order: 0,
            default: false,
            hold: None,
            more: false,
            disabled: dis,
            action: Action::Linux(0),
        };
        let r = rows_from_items(&[mk("a", true, None), mk("b", false, Some(Disabled::UnsignedSecureBoot))]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].item, 1);
        assert!(r[0].disabled.is_some());
    }
}
