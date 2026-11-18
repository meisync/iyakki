//! The widget board on the machine page (LAUNCHER-PLAN slice 4, F-LAUNCH-08).
//!
//! Page 0 of the home screen shows the person's board (`iyakki_uikit::board`): Attention first (what needs a person, with the real
//! problem named), the approvals the assistant waits on, the machine's vitals, the runs going, the tokens of the last day, the network and
//! the groups of apps. Each widget reads only what the core says (`iyakki_core_api`: the apps' statuses, the sysinfo, the net, the usage
//! ledger). Edit mode (key `e`, or the Arrange button) puts a remove badge, a grip to drag, a size pill and an Add tile on the widgets;
//! every change is kept for the person, audited as `settings.display.board`, and can be made from the chat (`display_board_set`).

use super::*;
use crate::fx::with_alpha;
use iyakki_core_api::Os;
use iyakki_uikit::board::{self, Board, Kind, Size};

/// What the widgets show that comes from the machine, read at most every two seconds while the machine page is on screen.
#[derive(Clone, Default)]
pub(super) struct BoardFacts {
    pub vitals: Vec<String>,
    pub network: Vec<String>,
    pub tokens: Option<(u64, u64, u64)>,
    /// The single tool calls waiting for the person (asked of the core), for the approvals widget.
    pub waiting: Vec<iyakki_core_api::WaitingCall>,
    pub at: u64,
}

#[derive(Default)]
pub(super) struct BoardUi {
    pub board: Board,
    pub edit: bool,
    pub sel: usize,
    pub add_pick: usize,
    /// A grip pressed: the widget and where the pointer went down; `moved` once it travelled.
    pub drag: Option<(usize, (i32, i32))>,
    pub moved: bool,
    /// An edge handle pressed (the widget to resize); the pointer decides half or full when it is let go.
    pub resize: Option<usize>,
    /// The first row of widgets on screen: the board scrolls by whole rows when its widgets do not all fit.
    pub scroll: usize,
    pub facts: BoardFacts,
}

// ------------------------------------------------------------------------------------------------ pure pieces

/// `1.2k`, `3.4M`; small numbers as they are.
pub(crate) fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

fn uptime_words(secs: f64) -> String {
    let m = (secs / 60.0) as u64;
    match m {
        0..=59 => format!("{m} min"),
        60..=2879 => format!("{} h {} min", m / 60, m % 60),
        _ => format!("{} days", m / 1440),
    }
}

/// The lines of the machine widget, from what the core reports (a figure that cannot be read says so instead of showing a zero).
pub(crate) fn vitals_lines(os: &dyn Os) -> Vec<String> {
    let si = os.sysinfo();
    let mut v = Vec::new();
    v.push(match (si.loadavg(), si.cpu_count()) {
        (Ok(l), Ok(n)) => format!("Load {:.2} on {n} processor{}", l.one, if n == 1 { "" } else { "s" }),
        (Ok(l), Err(_)) => format!("Load {:.2}", l.one),
        _ => "Load: not readable here".to_string(),
    });
    v.push(match si.meminfo() {
        Ok(m) if m.total_kb > 0 => format!("Memory {:.1} of {:.1} GB in use", m.total_kb.saturating_sub(m.available_kb) as f64 / 1_048_576.0, m.total_kb as f64 / 1_048_576.0),
        _ => "Memory: not readable here".to_string(),
    });
    v.push(match si.uptime() {
        Ok(u) => format!("Up {}", uptime_words(u.up_secs)),
        _ => "Uptime: not readable here".to_string(),
    });
    v
}

/// The lines of the network widget: every address except the loopback.
pub(crate) fn network_lines(os: &dyn Os) -> Vec<String> {
    match os.net().ipv4_addresses() {
        Ok(a) => {
            let v: Vec<String> = a.into_iter().filter(|(_, ip)| !ip.starts_with("127.")).map(|(i, ip)| format!("{i}  {ip}")).collect();
            if v.is_empty() {
                vec!["No network address".to_string()]
            } else {
                v
            }
        }
        Err(_) => vec!["Network: not readable here".to_string()],
    }
}

/// The widgets' rows: full-width widgets take a row; half-width ones pair up two to a row when `two_cols`.
pub(crate) fn rows_of(two_cols: bool, items: &[(Size, f32)]) -> Vec<Vec<usize>> {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for (i, (size, _)) in items.iter().enumerate() {
        let wide = *size == Size::Full || !two_cols;
        match rows.last_mut() {
            Some(last) if !wide && last.len() == 1 && items[last[0]].0 == Size::Half => last.push(i),
            _ => rows.push(vec![i]),
        }
    }
    rows
}

/// Where the widgets go: `items` are the widgets' widths and wanted heights in order. Full-width widgets take a row; half-width ones pair
/// up two to a row when `two_cols` (otherwise every widget takes a row). A row is as tall as its tallest widget. A widget that does not fit
/// in `area` is `None`, and so is every one after it (the order is the person's, nothing is silently reordered).
#[allow(dead_code)]
pub(crate) fn place(area: R, two_cols: bool, gap: f32, items: &[(Size, f32)]) -> Vec<Option<R>> {
    place_from(area, two_cols, gap, items, 0)
}

/// [`place`] with the board scrolled: the rows before `first_row` are off the top (`None`), the rest fill the area from its top as before.
pub(crate) fn place_from(area: R, two_cols: bool, gap: f32, items: &[(Size, f32)], first_row: usize) -> Vec<Option<R>> {
    let rows = rows_of(two_cols, items);
    let col_w = ((area.w - gap) / 2.0).max(1.0);
    let bottom = area.y + area.h + 0.5;
    let (mut out, mut y, mut full) = (vec![None; items.len()], area.y, false);
    for row in rows.into_iter().skip(first_row) {
        let h = row.iter().map(|i| items[*i].1).fold(0.0f32, f32::max);
        if full || y + h > bottom {
            full = true;
            continue;
        }
        for (k, i) in row.iter().enumerate() {
            let wide = items[*i].0 == Size::Full || !two_cols;
            out[*i] = Some(if wide { R::new(area.x, y, area.w, h) } else { R::new(area.x + k as f32 * (col_w + gap), y, col_w, h) });
        }
        y += h + gap;
    }
    out
}

/// The furthest the board scrolls: the first row from which every row after it fits (0 when everything fits). A single row taller than
/// the area still ends the scrolling at that row.
pub(crate) fn scroll_max(area: R, two_cols: bool, gap: f32, items: &[(Size, f32)]) -> usize {
    let rows = rows_of(two_cols, items);
    let heights: Vec<f32> = rows.iter().map(|r| r.iter().map(|i| items[*i].1).fold(0.0f32, f32::max)).collect();
    let mut start = heights.len().saturating_sub(1);
    let mut used = heights.last().copied().unwrap_or(0.0);
    while start > 0 && used + gap + heights[start - 1] <= area.h + 0.5 {
        start -= 1;
        used += gap + heights[start];
    }
    if heights.is_empty() || (start == 0 && used > area.h + 0.5 && heights.len() == 1) {
        return 0;
    }
    start
}

/// The row a widget is in.
pub(crate) fn row_of(two_cols: bool, items: &[(Size, f32)], widget: usize) -> Option<usize> {
    rows_of(two_cols, items).iter().position(|r| r.contains(&widget))
}

/// The edge handle of a widget card in edit mode: a strip down the right edge, below the corner controls, wide enough for a thumb.
pub(crate) fn edge_handle(card: R, s: f32) -> R {
    let w = 16.0 * s;
    let top = card.y + 40.0 * s;
    R::new(card.x + card.w - w, top, w, (card.h - 40.0 * s).max(8.0 * s))
}

/// Half or full from where a dragged edge was let go: past 62% of the board's width is full width.
pub(crate) fn size_from_edge(area: R, x: f32) -> Size {
    if x >= area.x + area.w * 0.62 {
        Size::Full
    } else {
        Size::Half
    }
}

/// The controls of a widget card in edit mode: (grip, size pill, remove badge), each a comfortable touch target.
pub(crate) fn controls(card: R, s: f32) -> (R, R, R) {
    let d = 30.0 * s;
    let remove = R::new(card.x + card.w - d - 4.0 * s, card.y + 4.0 * s, d, d);
    let pill = R::new(remove.x - 52.0 * s - 4.0 * s, card.y + 4.0 * s, 52.0 * s, d);
    let grip = R::new(card.x + 4.0 * s, card.y + 4.0 * s, d, d);
    (grip, pill, remove)
}

// ---------------------------------------------------------------------------------------------------- the shell

impl Shell {
    /// Read this person's board at sign-in (and when a chat tool changed it).
    pub(super) fn load_board(&mut self, user: &str) {
        self.bd.board = board::load(self.env.data_dir(), user);
        self.bd.sel = self.bd.sel.min(self.bd.board.slots.len().saturating_sub(1));
    }

    pub(super) fn reset_board(&mut self) {
        self.bd = BoardUi::default();
    }

    /// Keep the board for this person and say so in the access log.
    fn board_changed(&mut self, what: &str) {
        if let Some(sess) = self.session.clone() {
            board::save(self.env.data_dir(), &sess.user, &self.bd.board);
            self.env.audit.record(&sess.user, "settings.display.board", what);
        }
        self.bd.sel = self.bd.sel.min(self.bd.board.slots.len().saturating_sub(1));
        mark_stale!(self);
    }

    /// Read the machine's figures for the widgets, at most every two seconds, and only while the machine page is the one on the screen.
    pub(super) fn refresh_board_facts(&mut self) {
        if !self.machine || !matches!(self.screen, Screen::Launcher) || self.bd.facts.at != 0 && self.tick.saturating_sub(self.bd.facts.at) < 60 {
            return;
        }
        let Some(cx) = self.cx() else { return };
        let mut f = BoardFacts { at: self.tick.max(1), ..Default::default() };
        if self.bd.board.has(Kind::Vitals) {
            f.vitals = vitals_lines(&*cx.os);
        }
        if self.bd.board.has(Kind::Network) {
            f.network = network_lines(&*cx.os);
        }
        if self.bd.board.has(Kind::Tokens) {
            f.tokens = self.core.app_status().tokens_24h(&cx.session.user);
        }
        if self.bd.board.has(Kind::Approvals) {
            f.waiting = self.core.app_status().waiting_calls(&cx.session.user);
        }
        if f.waiting != self.bd.facts.waiting || f.vitals != self.bd.facts.vitals || f.network != self.bd.facts.network || f.tokens != self.bd.facts.tokens {
            mark_stale!(self);
        }
        self.bd.facts = f;
    }

    /// The text lines of a widget that is a list of plain lines (the others draw their own rows).
    fn board_lines(&self, k: Kind) -> Vec<String> {
        let say = |app: PApp| self.status_of(app).map(|st| st.line.clone());
        match k {
            Kind::Vitals if self.bd.facts.vitals.is_empty() => vec!["Reading the machine...".to_string()],
            Kind::Vitals => self.bd.facts.vitals.clone(),
            Kind::Network if self.bd.facts.network.is_empty() => vec!["Reading the network...".to_string()],
            Kind::Network => self.bd.facts.network.clone(),
            Kind::Approvals if !self.bd.facts.waiting.is_empty() => {
                // each waiting call on its own line (the tool and what it will do), three at most, then how many more
                let w = &self.bd.facts.waiting;
                let mut v: Vec<String> = w.iter().take(3).map(|c| format!("{}: {}", c.tool, c.summary)).collect();
                if w.len() > 3 {
                    v.push(format!("and {} more", w.len() - 3));
                }
                v
            }
            Kind::Approvals => vec![say(PApp::Chat).filter(|_| self.status_of(PApp::Chat).is_some_and(|s| s.kind == iyakki_core_api::StatusKind::Attention)).unwrap_or_else(|| "None waiting".to_string())],
            Kind::Runs => vec![say(PApp::Runs).unwrap_or_else(|| "Nothing running".to_string())],
            Kind::Tokens => match self.bd.facts.tokens {
                Some((i, o, c)) => vec![format!("{} in, {} out", short_count(i), short_count(o)), format!("{} call{}", short_count(c), if c == 1 { "" } else { "s" })],
                None => vec!["Nothing used in the last 24 hours".to_string()],
            },
            Kind::Attention | Kind::Apps => Vec::new(),
        }
    }

    /// How tall a widget wants to be, in pixels at scale `s`.
    fn board_want(&self, k: Kind, s: f32, card_w: f32) -> f32 {
        let rows = match k {
            Kind::Attention => self.machine_attention().len().clamp(1, 6) as f32 * 48.0 + 6.0,
            Kind::Apps => self.apps_rows(card_w, s) as f32 * 24.0 + 6.0,
            other => self.board_lines(other).len().clamp(1, 4) as f32 * 20.0 + 6.0,
        };
        (26.0 + rows) * s
    }

    /// How many rows the Apps widget needs in a card `card_w` wide: two groups to a row when there is room, else one.
    fn apps_rows(&self, card_w: f32, s: f32) -> usize {
        let n = self.machine_groups().len().min(6);
        if card_w >= 300.0 * s {
            n.div_ceil(2)
        } else {
            n
        }
    }

    /// The board's area on the island: under the title and host line, above the footer hint.
    fn board_area(&self, isl: R, s: f32) -> R {
        let pad = 40.0 * s;
        let top = isl.y + 30.0 * s + 76.0 * s - 6.0 * s;
        let foot = if self.bd.edit { 30.0 * s } else { 16.0 * s };
        R::new(isl.x + pad, top, isl.w - 2.0 * pad, (isl.y + isl.h - foot - top).max(1.0))
    }

    /// The board's layout inputs on the island `isl`: the area for the widgets (the Add tile's room taken off), whether there are two columns,
    /// the gap, every widget's width and wanted height, and the Add tile (in edit mode, while something can still be added; it has its place
    /// at the bottom of the board's area, so it is always there to press).
    fn board_layout(&self, isl: R, s: f32) -> (R, bool, f32, Vec<(Size, f32)>, Option<R>) {
        let mut area = self.board_area(isl, s);
        let gap = 12.0 * s;
        let two = area.w >= 520.0 * s;
        let add = self.bd.edit && !self.bd.board.missing().is_empty();
        let tile = add.then(|| {
            let t = 40.0 * s;
            let r = R::new(area.x, area.y + (area.h - t).max(0.0), area.w, t);
            area.h = (area.h - t - gap).max(1.0);
            r
        });
        let half_w = if two { (area.w - gap) / 2.0 } else { area.w };
        let items: Vec<(Size, f32)> = self.bd.board.slots.iter().map(|sl| (sl.size, self.board_want(sl.kind, s, if sl.size == Size::Full || !two { area.w } else { half_w }))).collect();
        (area, two, gap, items, tile)
    }

    /// Every widget's card on the island `isl` (index into the board, rectangle), and the Add tile in edit mode. The board scrolls by whole rows
    /// (`bd.scroll`) when its widgets do not all fit.
    pub(super) fn board_places(&self, isl: R, s: f32) -> (Vec<(usize, R)>, Option<R>) {
        let (area, two, gap, items, tile) = self.board_layout(isl, s);
        let first = self.bd.scroll.min(scroll_max(area, two, gap, &items));
        let placed = place_from(area, two, gap, &items, first);
        let cards = placed.iter().enumerate().filter_map(|(i, r)| r.map(|r| (i, r))).collect();
        (cards, tile)
    }

    fn board_two_cols(&self, isl: R, s: f32) -> bool {
        self.board_area(isl, s).w >= 520.0 * s
    }

    /// How far the board can scroll (rows) as laid out now, and whether it has two columns.
    pub(super) fn board_scroll_range(&self, isl: R, s: f32) -> (bool, usize) {
        let (area, two, gap, items, _) = self.board_layout(isl, s);
        (two, scroll_max(area, two, gap, &items))
    }

    /// Scroll the board by `rows` (positive: down), within what it can; true when it moved.
    pub(super) fn board_scroll_by(&mut self, rows: i32) -> bool {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let (_, max) = self.board_scroll_range(self.board_island(), s);
        let now = self.bd.scroll.min(max);
        let to = (now as i64 + rows as i64).max(0).min(max as i64) as usize;
        let moved = to != self.bd.scroll;
        self.bd.scroll = to;
        if moved {
            mark_stale!(self);
        }
        moved
    }

    /// Bring the selected widget's row on screen (arranging by key walks past the edge of what fits).
    fn board_reveal(&mut self) {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let (area, two, gap, items, _) = self.board_layout(self.board_island(), s);
        let Some(row) = row_of(two, &items, self.bd.sel) else { return };
        let max = scroll_max(area, two, gap, &items);
        let mut first = self.bd.scroll.min(max).min(row);
        while first < max && place_from(area, two, gap, &items, first)[self.bd.sel].is_none() {
            first += 1;
        }
        self.bd.scroll = first;
    }

    /// The Arrange / Done button beside the host line.
    pub(super) fn board_button(&self, g: &mut Gfx, isl: R, s: f32) -> R {
        let label = if self.bd.edit { "Done" } else { "Arrange" };
        let bw = g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.7;
        R::new(isl.x + isl.w - 40.0 * s - bw, isl.y + 30.0 * s + 40.0 * s, bw, 28.0 * s)
    }

    pub(super) fn draw_board(&self, g: &mut Gfx, isl: R, s: f32) {
        let _p = iyakki_uikit::prof::scope("shell.draw_board");
        let (cards, add) = self.board_places(isl, s);
        let n = self.bd.board.slots.len();
        if n == 0 {
            let r = self.board_area(isl, s);
            let t = g.fit(15.0 * s, "The board is empty. Arrange adds widgets.", Face::Sans, r.w);
            g.text(r.x, r.y + 6.0 * s, 15.0 * s, MUTED, &t, Face::Sans);
        }
        for (i, r) in &cards {
            let slot = self.bd.board.slots[*i];
            let sel = self.bd.edit && *i == self.bd.sel;
            g.rrect(r.x, r.y, r.w, r.h, 16.0 * s, with_alpha(CARD, 200));
            if sel {
                g.rrect_outline(r.x, r.y, r.w, r.h, 16.0 * s, 2.0 * s * crate::theme::ring().max(1.0), crate::ui::ACCENT_MAIN);
            }
            let (grip, pill, remove) = controls(*r, s);
            let tx = r.x + 14.0 * s + if self.bd.edit { grip.w } else { 0.0 };
            let tw = r.w - (tx - r.x) - 10.0 * s - if self.bd.edit { pill.w + remove.w + 12.0 * s } else { 0.0 };
            let title = g.fit(12.0 * s, slot.kind.title(), Face::Bold, tw.max(20.0 * s));
            g.text(tx, r.y + 10.0 * s, 12.0 * s, MUTED, &title, Face::Bold);
            let body = R::new(r.x + 14.0 * s, r.y + 26.0 * s, r.w - 28.0 * s, (r.h - 30.0 * s).max(1.0));
            self.draw_widget_body(g, slot.kind, body, s);
            if self.bd.edit {
                let moving = self.bd.drag.is_some_and(|(d, _)| d == *i) && self.bd.moved;
                g.rrect(grip.x, grip.y, grip.w, grip.h, 8.0 * s, with_alpha(if moving { crate::ui::ACCENT_MAIN } else { CARD_HOVER }, 200));
                for k in 0..3 {
                    g.rect(grip.x + 8.0 * s, grip.y + 9.0 * s + k as f32 * 6.0 * s, grip.w - 16.0 * s, 2.0 * s, MUTED);
                }
                g.rrect(pill.x, pill.y, pill.w, pill.h, 8.0 * s, with_alpha(CARD_HOVER, 200));
                let word = if slot.size == Size::Full { "FULL" } else { "HALF" };
                let ww = g.text_width(11.0 * s, word, Face::Bold);
                g.text(pill.x + (pill.w - ww) / 2.0, pill.y + (pill.h - 11.0 * s) / 2.0 - 1.0 * s, 11.0 * s, TEXT, word, Face::Bold);
                g.circle(remove.x + remove.w / 2.0, remove.y + remove.h / 2.0, remove.w / 2.0 - 2.0 * s, with_alpha(ERR, 220));
                let (cx, cy, k) = (remove.x + remove.w / 2.0, remove.y + remove.h / 2.0, 5.0 * s);
                g.line(cx - k, cy - k, cx + k, cy + k, 2.2 * s, [255, 255, 255, 255]);
                g.line(cx - k, cy + k, cx + k, cy - k, 2.2 * s, [255, 255, 255, 255]);
                let name = slot.kind.title().to_lowercase();
                g.hit(grip, Event::MouseDown((grip.x + grip.w / 2.0) as i32, (grip.y + grip.h / 2.0) as i32), &format!("move widget {name}"));
                g.hit(pill, Event::MouseDown((pill.x + pill.w / 2.0) as i32, (pill.y + pill.h / 2.0) as i32), &format!("size of widget {name}"));
                g.hit(remove, Event::MouseDown((remove.x + remove.w / 2.0) as i32, (remove.y + remove.h / 2.0) as i32), &format!("remove widget {name}"));
                if self.board_two_cols(isl, s) {
                    let edge = edge_handle(*r, s);
                    let resizing = self.bd.resize == Some(*i);
                    g.rrect(edge.x + edge.w / 2.0 - 2.0 * s, edge.y + edge.h / 2.0 - 14.0 * s, 4.0 * s, 28.0 * s, 2.0 * s, if resizing { crate::ui::ACCENT_MAIN } else { MUTED });
                    g.hit(edge, Event::MouseDown((edge.x + edge.w / 2.0) as i32, (edge.y + edge.h / 2.0) as i32), &format!("resize widget {name}"));
                }
            }
        }
        if let Some(r) = add {
            let next = self.bd.board.missing()[self.bd.add_pick % self.bd.board.missing().len()];
            g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, with_alpha(CARD, 150));
            g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 1.5 * s, crate::ui::ACCENT_MAIN);
            let label = g.fit(14.0 * s, &format!("+ Add {}", next.title()), Face::Bold, r.w - 24.0 * s);
            g.text(r.x + 14.0 * s, r.y + (r.h - 14.0 * s) / 2.0 - 1.0 * s, 14.0 * s, TEXT, &label, Face::Bold);
            g.hit(r, Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32), "add a widget");
        }
        let first_shown = cards.iter().map(|(i, _)| *i).min().unwrap_or(0);
        let above = first_shown;
        let below = n.saturating_sub(cards.iter().map(|(i, _)| *i + 1).max().unwrap_or(0));
        if n > 0 && (above > 0 || below > 0) {
            let r = self.board_area(isl, s);
            let word = |k: usize| format!("{k} widget{}", if k == 1 { "" } else { "s" });
            let text = match (above > 0, below > 0) {
                (true, true) => format!("{} above and {} below: scroll to see them.", word(above), word(below)),
                (true, false) => format!("{} above: scroll up.", word(above)),
                _ => format!("{} more: scroll down.", word(below)),
            };
            let t = g.fit(12.0 * s, &text, Face::Sans, r.w);
            g.text(r.x, isl.y + isl.h - 22.0 * s - if self.bd.edit { 26.0 * s } else { 0.0 }, 12.0 * s, DIM, &t, Face::Sans);
        }
        if self.bd.edit {
            let hint = g.fit(12.0 * s, "Up Down choose · , . move · s size · x remove · a add · r start again · e or Esc done", Face::Sans, isl.w - 80.0 * s);
            g.text(isl.x + 40.0 * s, isl.y + isl.h - 24.0 * s, 12.0 * s, DIM, &hint, Face::Sans);
        }
        let b = self.board_button(g, isl, s);
        let label = if self.bd.edit { "Done" } else { "Arrange" };
        g.button(b.x, b.y, 13.0 * s, label, if self.bd.edit { crate::ui::ACCENT_MAIN } else { MUTED }, Event::MouseDown((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
    }

    fn draw_widget_body(&self, g: &mut Gfx, k: Kind, body: R, s: f32) {
        match k {
            Kind::Attention => {
                let rows = self.machine_attention();
                if rows.is_empty() {
                    let t = g.fit(15.0 * s, "Nothing needs you right now.", Face::Sans, body.w);
                    g.text(body.x, body.y + 6.0 * s, 15.0 * s, TEXT, &t, Face::Sans);
                }
                let mut y = body.y;
                for (name, detail, st) in rows.iter().take(6) {
                    if y + 44.0 * s > body.y + body.h + 4.0 * s {
                        break;
                    }
                    g.rrect(body.x, y, body.w, 42.0 * s, 12.0 * s, with_alpha(CARD, 200));
                    g.rrect(body.x + 8.0 * s, y + 8.0 * s, 5.0 * s, 26.0 * s, 2.5 * s, st.colour());
                    let t = g.fit(15.0 * s, name, Face::Bold, body.w - 36.0 * s);
                    g.text(body.x + 24.0 * s, y + 4.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
                    let d = g.fit(12.5 * s, detail, Face::Sans, body.w - 36.0 * s);
                    g.text(body.x + 24.0 * s, y + 22.0 * s, 12.5 * s, MUTED, &d, Face::Sans);
                    y += 48.0 * s;
                }
            }
            Kind::Apps => {
                let groups: Vec<(String, usize)> = self.machine_groups().into_iter().take(6).collect();
                let two = body.w + 28.0 * s >= 300.0 * s;
                let col_w = if two { (body.w - 16.0 * s) / 2.0 } else { body.w };
                for (i, (name, n)) in groups.iter().enumerate() {
                    let (col, row) = if two { (i % 2, i / 2) } else { (0, i) };
                    let (x, y) = (body.x + col as f32 * (col_w + 16.0 * s), body.y + row as f32 * 24.0 * s);
                    if y + 22.0 * s > body.y + body.h + 4.0 * s {
                        break;
                    }
                    let c = format!("{n}");
                    let cw = g.text_width(12.5 * s, &c, Face::Sans);
                    let t = g.fit(13.0 * s, name, Face::Bold, (col_w - cw - 10.0 * s).max(20.0 * s));
                    g.text(x, y + 1.0 * s, 13.0 * s, TEXT, &t, Face::Bold);
                    g.text(x + col_w - cw, y + 2.0 * s, 12.5 * s, MUTED, &c, Face::Sans);
                }
            }
            other => {
                let mut y = body.y;
                for line in self.board_lines(other).into_iter().take(4) {
                    let t = g.fit(14.0 * s, &line, Face::Sans, body.w);
                    g.text(body.x, y, 14.0 * s, TEXT, &t, Face::Sans);
                    y += 20.0 * s;
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------------- input

    fn board_island(&self) -> R {
        let (w, h) = self.size;
        launcher::machine_rect(w, h, scale_for(w, h))
    }

    /// Clicks and drags on the machine page's board. True when the event was the board's.
    pub(super) fn board_pointer(&mut self, ev: &Event) -> bool {
        if !self.machine || !matches!(self.screen, Screen::Launcher) {
            return false;
        }
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let isl = self.board_island();
        match ev {
            Event::MouseDown(x, y) => {
                let mut scratch = Gfx::new(8, 8);
                if self.board_button(&mut scratch, isl, s).contains(*x, *y) {
                    self.bd.edit = !self.bd.edit;
                    mark_stale!(self);
                    return true;
                }
                if !self.bd.edit {
                    return false;
                }
                let (cards, add) = self.board_places(isl, s);
                if add.is_some_and(|r| r.contains(*x, *y)) {
                    self.board_add();
                    return true;
                }
                for (i, r) in cards {
                    let (grip, pill, remove) = controls(r, s);
                    if remove.contains(*x, *y) {
                        self.bd.sel = i;
                        self.board_remove();
                        return true;
                    }
                    if pill.contains(*x, *y) {
                        self.bd.sel = i;
                        self.board_size();
                        return true;
                    }
                    if self.board_two_cols(isl, s) && edge_handle(r, s).contains(*x, *y) {
                        self.bd.sel = i;
                        self.bd.resize = Some(i);
                        mark_stale!(self);
                        return true;
                    }
                    if grip.contains(*x, *y) {
                        self.bd.sel = i;
                        self.bd.drag = Some((i, (*x, *y)));
                        self.bd.moved = false;
                        mark_stale!(self);
                        return true;
                    }
                    if r.contains(*x, *y) {
                        self.bd.sel = i;
                        mark_stale!(self);
                        return true;
                    }
                }
                false
            }
            Event::MouseMove(..) if self.bd.resize.is_some() => true,
            Event::MouseMove(x, y) => match self.bd.drag {
                Some((_, (px, py))) => {
                    if (x - px).abs() + (y - py).abs() > 8 && !self.bd.moved {
                        self.bd.moved = true;
                        mark_stale!(self);
                    }
                    true
                }
                None => false,
            },
            Event::MouseUp(x, _) if self.bd.resize.is_some() => {
                let i = self.bd.resize.take().unwrap_or(0);
                let want = size_from_edge(self.board_area(isl, s), *x as f32);
                if let Some(sl) = self.bd.board.slots.get(i).copied() {
                    if sl.size != want {
                        self.bd.sel = i;
                        self.bd.board.set_size(i, want);
                        self.board_changed(&format!("size {} {}", sl.kind.key(), want.key()));
                    }
                }
                mark_stale!(self);
                true
            }
            Event::MouseUp(x, y) => {
                let Some((from, _)) = self.bd.drag.take() else { return false };
                let moved = std::mem::take(&mut self.bd.moved);
                if moved {
                    let (cards, _) = self.board_places(isl, s);
                    if let Some((to, _)) = cards.into_iter().find(|(_, r)| r.contains(*x, *y)) {
                        if to != from {
                            self.board_move(from, to);
                        }
                    }
                }
                mark_stale!(self);
                true
            }
            _ => false,
        }
    }

    /// Keys on the machine page: `e` starts arranging; while arranging the board has the keyboard.
    pub(super) fn board_key(&mut self, k: &Key) -> bool {
        if !self.machine || !matches!(self.screen, Screen::Launcher) {
            return false;
        }
        if !self.bd.edit {
            if matches!(k, Key::Char('e')) {
                self.bd.edit = true;
                mark_stale!(self);
                return true;
            }
            return false;
        }
        let n = self.bd.board.slots.len();
        match k {
            Key::Up | Key::Left => self.bd.sel = self.bd.sel.saturating_sub(1),
            Key::Down | Key::Right => self.bd.sel = (self.bd.sel + 1).min(n.saturating_sub(1)),
            Key::Char(',') if self.bd.sel > 0 => self.board_move(self.bd.sel, self.bd.sel - 1),
            Key::Char('.') if self.bd.sel + 1 < n => self.board_move(self.bd.sel, self.bd.sel + 1),
            Key::Char('s') => self.board_size(),
            Key::Char('x') => self.board_remove(),
            Key::Char('a') => self.board_add(),
            Key::Char('r') => {
                self.bd.board = Board::default();
                self.board_changed("reset");
            }
            Key::Esc | Key::Char('e') | Key::Enter => self.bd.edit = false,
            _ => {}
        }
        self.board_reveal();
        mark_stale!(self);
        true
    }

    fn board_move(&mut self, from: usize, to: usize) {
        let name = self.bd.board.slots.get(from).map(|s| s.kind.key());
        if let (Some(name), Some(at)) = (name, self.bd.board.move_to(from, to)) {
            self.bd.sel = at;
            self.board_changed(&format!("move {name} to {}", at + 1));
        }
    }

    fn board_size(&mut self) {
        if let Some(sl) = self.bd.board.slots.get(self.bd.sel).copied() {
            self.bd.board.set_size(self.bd.sel, sl.size.other());
            self.board_changed(&format!("size {} {}", sl.kind.key(), sl.size.other().key()));
        }
    }

    fn board_remove(&mut self) {
        if let Some(sl) = self.bd.board.remove(self.bd.sel) {
            self.board_changed(&format!("remove {}", sl.kind.key()));
        }
    }

    fn board_add(&mut self) {
        let miss = self.bd.board.missing();
        if miss.is_empty() {
            return;
        }
        let k = miss[self.bd.add_pick % miss.len()];
        if self.bd.board.add(k) {
            self.bd.sel = self.bd.board.slots.len() - 1;
            self.board_changed(&format!("add {}", k.key()));
        }
    }
}

#[cfg(test)]
#[path = "shell_board_tests.rs"]
mod tests;
