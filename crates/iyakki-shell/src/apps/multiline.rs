//! The multi-line input of the composers (the TALES idea box, the chat and the AI IDE's chat): several lines that wrap, a caret,
//! a selection, a height that grows from a few lines to a larger most and then scrolls inside the box (wheel, arrows, typing),
//! and a limit on the characters that is never reached silently: a notice with the counts says what was left out.

use std::collections::HashMap;

use super::blink;
use crate::fx;
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, Key, Nav, DIM, ERR, MUTED, R, TEXT};

/// The most characters of one chat message (or one message to the AI IDE's assistant) that can be typed or pasted.
pub const CHAT_MAX: usize = 20_000;

/// Lines the box shows when it is empty, and the most it grows to before it scrolls (the chat in the full window may take more).
pub const MIN_ROWS: usize = 3;
pub const MAX_ROWS: usize = 10;
pub const MAX_ROWS_WIDE: usize = 12;

/// `12345` as "12,345".
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub struct MultiLine {
    pub text: String,
    pub cur: usize,
    pub anchor: Option<usize>,
    /// The first line shown (a visual line).
    pub top: usize,
    pub lay: Vec<(String, usize)>,
    key: (i32, i32, usize),
    /// The wrapped lines match the text (false right after an edit, until the next draw).
    pub fresh: bool,
    /// The lines the box shows when the text is shorter, and the most it shows before it scrolls.
    pub min_rows: usize,
    pub max_rows: usize,
    /// The most characters the box takes.
    pub max: usize,
    /// Enter makes a new line (the story box; the chat sends with Enter and has its own key for a new line).
    pub enter_newline: bool,
    /// The last typing, paste or text that did not fit: (characters offered in all, the limit). Shown as a notice.
    pub cut: Option<(usize, usize)>,
    /// The box scrolls to keep the caret in view (false after the wheel moved it away, until the next edit or caret move).
    follow: bool,
    /// (padding, line height) of the last draw, to map a press back to a character.
    pub geo: Option<(f32, f32)>,
    /// The box as last drawn and whether the pointer was over it.
    pub rect: R,
    hover: bool,
    /// The wrapped lines of one logical line, kept while the size and width stay (typing re-wraps only the line it changed).
    wrapped: HashMap<String, Vec<String>>,
    wrapped_for: (i32, i32),
    /// The width of the first 0, 1, 2… characters of the visible lines: (visual line, widths).
    adv: Vec<(usize, Vec<f32>)>,
    /// The height of the box alone (without the notice) at the last measure.
    pub box_h: f32,
}

impl MultiLine {
    pub fn new(max: usize) -> MultiLine {
        MultiLine {
            text: String::new(),
            cur: 0,
            anchor: None,
            top: 0,
            lay: Vec::new(),
            key: (0, 0, usize::MAX),
            fresh: false,
            min_rows: MIN_ROWS,
            max_rows: MAX_ROWS,
            max,
            enter_newline: true,
            cut: None,
            follow: true,
            geo: None,
            rect: R::new(0.0, 0.0, 0.0, 0.0),
            hover: false,
            wrapped: HashMap::new(),
            wrapped_for: (0, 0),
            adv: Vec::new(),
            box_h: 0.0,
        }
    }
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    fn byte(&self, ci: usize) -> usize {
        self.text.char_indices().nth(ci).map_or(self.text.len(), |(b, _)| b)
    }
    /// Text as the box keeps it: no control characters (but line breaks), tabs as spaces.
    fn clean(s: &str) -> String {
        s.replace("\r\n", "\n").replace('\r', "\n").chars().flat_map(|c| if c == '\t' { vec![' '; 4] } else { vec![c] }).filter(|c| !c.is_control() || *c == '\n').collect()
    }
    /// Replace the text (a draft, a suggestion). What does not fit is left out and the notice says so.
    pub fn set(&mut self, t: &str) {
        let all = Self::clean(t);
        let n = all.chars().count();
        self.text = all.chars().take(self.max).collect();
        self.cut = (n > self.max).then_some((n, self.max));
        self.cur = self.len();
        self.anchor = None;
        self.top = 0;
        self.key.2 = usize::MAX;
        self.fresh = false;
        self.follow = true;
    }
    /// The text, and the box is empty again.
    pub fn take(&mut self) -> String {
        self.cur = 0;
        self.anchor = None;
        self.top = 0;
        self.cut = None;
        self.fresh = false;
        self.follow = true;
        self.key.2 = usize::MAX;
        std::mem::take(&mut self.text)
    }
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cur).then(|| (a.min(self.cur), a.max(self.cur)))
    }
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        Some(self.text.chars().skip(a).take(b - a).collect())
    }
    pub fn is_all_selected(&self) -> bool {
        self.selection() == Some((0, self.len())) && !self.text.is_empty()
    }
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cur = self.len();
    }
    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else { return false };
        let (ba, bb) = (self.byte(a), self.byte(b));
        self.text.replace_range(ba..bb, "");
        self.cur = a;
        self.anchor = None;
        self.fresh = false;
        self.follow = true;
        self.refit();
        true
    }
    /// The notice goes away once the text is short enough again.
    fn refit(&mut self) {
        if self.len() < self.max {
            self.cut = None;
        }
    }
    pub fn insert(&mut self, s: &str) -> bool {
        self.delete_selection();
        let all = Self::clean(s);
        let offered = all.chars().count();
        let room = self.max.saturating_sub(self.len());
        let ins: String = all.chars().take(room).collect();
        self.cut = (offered > room).then_some((self.len() + offered, self.max));
        if ins.is_empty() {
            return false;
        }
        let at = self.byte(self.cur);
        self.text.insert_str(at, &ins);
        self.cur += ins.chars().count();
        self.anchor = None;
        self.fresh = false;
        self.follow = true;
        true
    }
    /// Pasted text goes in at the caret (or over the selection), line breaks kept.
    pub fn paste(&mut self, s: &str) -> bool {
        self.insert(s)
    }
    pub fn move_to(&mut self, p: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cur);
        } else {
            self.anchor = None;
        }
        self.cur = p.min(self.len());
        self.follow = true;
    }
    pub fn line_of(&self, idx: usize) -> usize {
        self.lay.iter().rposition(|(_, s)| *s <= idx).unwrap_or(0)
    }
    /// Start and end (in characters) of the line, ending at a line break, that holds `idx`.
    fn logical_line(&self, idx: usize) -> (usize, usize) {
        let chars: Vec<char> = self.text.chars().collect();
        let idx = idx.min(chars.len());
        let start = chars[..idx].iter().rposition(|c| *c == '\n').map_or(0, |p| p + 1);
        let end = chars[idx..].iter().position(|c| *c == '\n').map_or(chars.len(), |p| idx + p);
        (start, end)
    }
    fn home_pos(&self) -> usize {
        if self.fresh {
            self.lay.get(self.line_of(self.cur)).map_or(0, |l| l.1)
        } else {
            self.logical_line(self.cur).0
        }
    }
    fn end_pos(&self) -> usize {
        if self.fresh {
            self.lay.get(self.line_of(self.cur)).map_or(self.len(), |(t, s)| s + t.chars().count())
        } else {
            self.logical_line(self.cur).1
        }
    }
    /// The position `d` lines up (negative) or down, in the same column (lines as drawn when they are known, else as typed).
    fn vertical(&self, d: i32) -> usize {
        if !self.fresh {
            let (s, e) = self.logical_line(self.cur);
            let col = self.cur - s;
            return if d < 0 {
                if s == 0 {
                    0
                } else {
                    let (ps, pe) = self.logical_line(s - 1);
                    ps + col.min(pe - ps)
                }
            } else if e >= self.len() {
                self.len()
            } else {
                let (ns, ne) = self.logical_line(e + 1);
                ns + col.min(ne - ns)
            };
        }
        if self.lay.is_empty() {
            return self.cur;
        }
        let l = self.line_of(self.cur);
        let col = self.cur - self.lay[l].1;
        let t = l as i32 + d;
        if t < 0 {
            return 0;
        }
        if t as usize >= self.lay.len() {
            return self.len();
        }
        let (text, start) = &self.lay[t as usize];
        start + col.min(text.chars().count())
    }
    /// Handle a key; true when the text changed.
    pub fn key(&mut self, k: &Key) -> bool {
        match k {
            Key::Char(c) if !c.is_control() => self.insert(&c.to_string()),
            Key::Enter if self.enter_newline => self.insert("\n"),
            Key::Backspace => {
                if self.delete_selection() {
                    return true;
                }
                if self.cur == 0 {
                    return false;
                }
                let (a, b) = (self.byte(self.cur - 1), self.byte(self.cur));
                self.text.replace_range(a..b, "");
                self.cur -= 1;
                self.fresh = false;
                self.follow = true;
                self.refit();
                true
            }
            Key::Delete => {
                if self.delete_selection() {
                    return true;
                }
                if self.cur >= self.len() {
                    return false;
                }
                let (a, b) = (self.byte(self.cur), self.byte(self.cur + 1));
                self.text.replace_range(a..b, "");
                self.fresh = false;
                self.follow = true;
                self.refit();
                true
            }
            Key::Left => {
                let to = self.selection().map_or(self.cur.saturating_sub(1), |s| s.0);
                self.move_to(to, false);
                false
            }
            Key::Right => {
                let to = self.selection().map_or(self.cur + 1, |s| s.1);
                self.move_to(to, false);
                false
            }
            Key::Up => {
                let t = self.vertical(-1);
                self.move_to(t, false);
                false
            }
            Key::Down => {
                let t = self.vertical(1);
                self.move_to(t, false);
                false
            }
            Key::PageUp | Key::PageDown if self.lay.len() > self.min_rows => {
                let step = self.max_rows.max(2) as i32 - 1;
                let t = self.vertical(if matches!(k, Key::PageUp) { -step } else { step });
                self.move_to(t, false);
                false
            }
            Key::Home => {
                let s = self.home_pos();
                self.move_to(s, false);
                false
            }
            Key::End => {
                let e = self.end_pos();
                self.move_to(e, false);
                false
            }
            Key::Nav { nav, shift, .. } => {
                let step = self.max_rows.max(2) as i32 - 1;
                let to = match nav {
                    Nav::Left => self.cur.saturating_sub(1),
                    Nav::Right => self.cur + 1,
                    Nav::Up => self.vertical(-1),
                    Nav::Down => self.vertical(1),
                    Nav::PageUp => self.vertical(-step),
                    Nav::PageDown => self.vertical(step),
                    Nav::Home => self.home_pos(),
                    Nav::End => self.end_pos(),
                };
                self.move_to(to, *shift);
                false
            }
            Key::Ctrl('a') | Key::Ctrl('A') => {
                self.select_all();
                false
            }
            Key::Ctrl('c') | Key::Ctrl('C') | Key::Ctrl('x') | Key::Ctrl('X') => {
                let t = self.selected_text().unwrap_or_else(|| self.text.clone());
                if !t.is_empty() {
                    crate::ui::clipboard::copy(&t);
                }
                if matches!(k, Key::Ctrl('x') | Key::Ctrl('X')) {
                    if self.selection().is_some() {
                        return self.delete_selection();
                    }
                    let had = !self.text.is_empty();
                    self.text.clear();
                    self.cur = 0;
                    self.fresh = false;
                    self.cut = None;
                    return had;
                }
                false
            }
            _ => false,
        }
    }
    /// Wrap the text to `width` (again only when the text, the size or the width changed).
    fn layout(&mut self, g: &mut Gfx, size: f32, width: f32) {
        let key = ((size * 100.0) as i32, width as i32, self.text.len() * 31 + self.text.bytes().map(|b| b as usize).sum::<usize>());
        if key == self.key {
            self.fresh = true;
            return;
        }
        self.key = key;
        self.fresh = true;
        if self.wrapped_for != (key.0, key.1) || self.wrapped.len() > 4000 {
            self.wrapped.clear();
            self.wrapped_for = (key.0, key.1);
        }
        self.lay.clear();
        let mut base = 0usize;
        for logical in self.text.split('\n') {
            let chars: Vec<char> = logical.chars().collect();
            let wrapped = if logical.is_empty() {
                vec![String::new()]
            } else if let Some(w) = self.wrapped.get(logical) {
                w.clone()
            } else {
                let w = g.wrap(size, logical, Face::Sans, width);
                self.wrapped.insert(logical.to_string(), w.clone());
                w
            };
            let mut pos = 0usize;
            for l in wrapped {
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                let n = l.chars().count();
                self.lay.push((l, base + pos));
                pos += n;
            }
            base += chars.len() + 1;
        }
        if self.lay.is_empty() {
            self.lay.push((String::new(), 0));
        }
        self.adv.clear();
    }
    /// How many lines the box shows: `min_rows` at least, growing with the text up to `max_rows`.
    pub fn rows(&self) -> usize {
        let min = self.min_rows.max(1);
        self.lay.len().clamp(min, self.max_rows.max(min))
    }
    /// The position nearest to a press `x` px from the left of the text and on visual line `line`.
    pub fn pos_at(&self, x: f32, line: usize) -> usize {
        let l = line.min(self.lay.len().saturating_sub(1));
        let Some((_, start)) = self.lay.get(l) else { return 0 };
        let Some((_, adv)) = self.adv.iter().find(|(i, _)| *i == l) else { return *start };
        let k = adv.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |(k, _)| k);
        start + k
    }
    /// A press at (`x`, `y`) px from the top left of the box (as drawn): the caret goes there (a drag extends the selection,
    /// a triple click selects all).
    pub fn press(&mut self, x: f32, y: f32) {
        let Some((pad, lh)) = self.geo else { return };
        let line = self.top + ((y - pad).max(0.0) / lh.max(1.0)) as usize;
        let pos = self.pos_at(x - pad, line);
        if crate::ui::dragging() {
            self.move_to(pos, true);
        } else if crate::ui::click_count() >= 3 {
            self.select_all();
            self.follow = true;
        } else {
            self.anchor = Some(pos);
            self.cur = pos;
            self.follow = true;
        }
    }
    /// The wheel over the box moves its lines (two per notch; positive is up). True when the box took it.
    pub fn wheel(&mut self, d: i32) -> bool {
        let rows = self.rows();
        if !self.hover || self.lay.len() <= rows {
            return false;
        }
        let max_top = self.lay.len() - rows;
        self.top = (self.top as i32 - d * 2).clamp(0, max_top as i32) as usize;
        self.follow = false;
        true
    }
    /// What to tell the person under the box: that something was left out (with the counts), or how full the box is.
    pub fn notice(&self) -> Option<(String, bool)> {
        if let Some((offered, max)) = self.cut {
            return Some((format!("Too long: the first {} of {} characters were kept, the rest was left out.", thousands(max), thousands(offered)), true));
        }
        (self.len() * 10 >= self.max * 9).then(|| (format!("{} of {} characters", thousands(self.len()), thousands(self.max)), false))
    }
    /// Wrap for a box `w` wide and return its height (box and notice) without drawing anything.
    pub fn measure(&mut self, g: &mut Gfx, w: f32, size: f32, pad: f32, min_h: f32, s: f32, clip_button: bool) -> f32 {
        let room = if clip_button { 30.0 * s } else { 0.0 };
        self.layout(g, size, (w - 2.0 * pad - room).max(10.0));
        let lh = g.line_height(size, Face::Sans);
        self.box_h = (self.rows() as f32 * lh + 2.0 * pad).max(min_h);
        self.box_h + self.notice_h(s)
    }
    fn notice_h(&self, s: f32) -> f32 {
        if self.notice().is_some() {
            16.0 * s
        } else {
            0.0
        }
    }
    /// Draw the box with its top left at (`x`, `y`) and `w` wide (when `draw`), register its press area and return its height
    /// with the notice line under it.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, size: f32, pad: f32, min_h: f32, focused: bool, tick: u64, placeholder: &str, accent: [u8; 4], area: usize, label: &str, clip_button: bool, draw: bool) -> f32 {
        let total = self.measure(g, w, size, pad, min_h, s, clip_button);
        if !draw {
            return total;
        }
        let h = self.box_h;
        let lh = g.line_height(size, Face::Sans);
        let rows = self.rows();
        let r = R::new(x, y, w, h);
        self.rect = r;
        self.hover = g.hovered(r);
        #[cfg(test)]
        let legacy = crate::ui::legacy_on();
        #[cfg(not(test))]
        let legacy = false;
        let focused = focused && (legacy || crate::ui::input_focus_allowed());
        fx::field_frame(g, r, s, focused, accent);
        if focused {
            crate::ui::note_focus_ring();
        }
        // keep the caret line in view (unless the wheel moved the lines away)
        let cl = self.line_of(self.cur);
        if self.follow {
            if cl < self.top {
                self.top = cl;
            } else if cl >= self.top + rows {
                self.top = cl + 1 - rows;
            }
        }
        self.top = self.top.min(self.lay.len().saturating_sub(rows));
        let top = self.top;
        g.hit(r, Event::Area(area), label);
        self.geo = Some((pad, lh));
        crate::ctxmenu::note_field(crate::ctxmenu::FieldNote { rect: r, id: area, focused, has_selection: self.selection().is_some(), empty: self.text.is_empty(), secret: false });
        let text_w = w - 2.0 * pad - if clip_button && focused { 30.0 * s } else { 0.0 };
        if self.text.is_empty() && !focused {
            let lines: Vec<String> = if rows == 1 { vec![g.fit(size, placeholder, Face::Sans, text_w)] } else { g.wrap(size, placeholder, Face::Sans, text_w) };
            for (i, l) in lines.iter().take(rows).enumerate() {
                g.text(x + pad, y + pad + i as f32 * lh, size, DIM, l, Face::Sans);
            }
            self.adv.clear();
            self.notice_line(g, x, y + h, w, s);
            return total;
        }
        // the widths of the characters of the lines on show (for the caret and for a press)
        self.adv = (top..(top + rows).min(self.lay.len()))
            .map(|i| {
                let chars: Vec<char> = self.lay[i].0.chars().collect();
                let mut v = Vec::with_capacity(chars.len() + 1);
                let mut prefix = String::new();
                v.push(0.0);
                for c in &chars {
                    prefix.push(*c);
                    v.push(g.text_width(size, &prefix, Face::Sans));
                }
                (i, v)
            })
            .collect();
        let chars: Vec<char> = self.text.chars().collect();
        let sel = self.selection();
        for (i, (line, start)) in self.lay.iter().enumerate().skip(top).take(rows) {
            let ly = y + pad + (i - top) as f32 * lh;
            let n = line.chars().count();
            if let Some((a, b)) = sel {
                let (lo, hi) = (a.max(*start), b.min(start + n));
                if lo < hi {
                    let pre: String = chars[*start..lo].iter().collect();
                    let mid: String = chars[lo..hi].iter().collect();
                    let px = x + pad + g.text_width(size, &pre, Face::Sans);
                    let mw = g.text_width(size, &mid, Face::Sans);
                    g.rrect(px, ly, mw.max(3.0), lh, 3.0, fx::with_alpha(accent, 90));
                }
            }
            g.text(x + pad, ly, size, TEXT, line, Face::Sans);
        }
        if self.lay.len() > rows {
            // a thin bar at the right edge shows that there is more above or below
            let (bh, bx) = (h - 2.0 * 4.0 * s, x + w - 5.0 * s);
            let (th, ty) = (bh * rows as f32 / self.lay.len() as f32, bh * top as f32 / self.lay.len() as f32);
            g.rrect(bx, y + 4.0 * s + ty, 2.5 * s, th.max(8.0 * s), 1.2 * s, fx::with_alpha(MUTED, 150));
        }
        if focused && blink(tick) {
            let l = cl.max(top).min(top + rows - 1);
            let start = self.lay.get(l).map_or(0, |l| l.1);
            let col = self.cur.saturating_sub(start);
            let cx = x + pad + self.adv.iter().find(|(i, _)| *i == l).and_then(|(_, a)| a.get(col).or(a.last())).copied().unwrap_or(0.0);
            let cy = y + pad + (l - top) as f32 * lh;
            g.rect(cx, cy, 2.0 * s.max(1.0), lh, accent);
            crate::ui::note_caret();
        }
        if clip_button && focused {
            // clipboard button: opens the clipboard history picker
            let (bx, by, bs) = (x + w - 28.0 * s - 4.0 * s, y + 6.0 * s, 22.0 * s);
            let hot = g.hovered(R::new(bx, by, bs, bs));
            g.rrect(bx, by, bs, bs, 6.0 * s, if hot { crate::gfx::rgb(60, 72, 110) } else { crate::gfx::rgb(34, 42, 70) });
            g.icon(Icon::Clipboard, bx + bs / 2.0, by + bs / 2.0, 14.0 * s, MUTED);
            g.hit(R::new(bx, by, bs, bs), Event::Key(Key::Ctrl('V')), "clipboard");
        }
        self.notice_line(g, x, y + h, w, s);
        total
    }
    fn notice_line(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) {
        if let Some((t, bad)) = self.notice() {
            let size = 12.5 * s;
            let t = g.fit(size, &t, Face::Sans, w - 8.0 * s);
            g.text(x + 4.0 * s, y + 2.0 * s, size, if bad { ERR } else { DIM }, &t, Face::Sans);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!((thousands(0), thousands(999), thousands(1000), thousands(20_000), thousands(1_234_567)), ("0".into(), "999".into(), "1,000".into(), "20,000".into(), "1,234,567".into()));
    }

    #[test]
    fn what_does_not_fit_is_cut_with_a_notice_with_the_counts() {
        let mut m = MultiLine::new(100);
        assert!(m.insert(&"a".repeat(60)));
        assert!(m.notice().is_none());
        // a paste that does not fit: the part that fits is kept
        assert!(m.insert(&"b".repeat(80)));
        assert_eq!(m.len(), 100);
        let (n, bad) = m.notice().unwrap();
        assert!(bad && n.contains("100 of 140"), "{n}");
        // typing at the limit is refused and still says why
        assert!(!m.key(&Key::Char('x')));
        assert!(m.notice().unwrap().1);
        // deleting makes room: the notice goes
        m.key(&Key::Backspace);
        assert!(m.cut.is_none());
        // set with too much
        m.set(&"c".repeat(150));
        assert_eq!(m.len(), 100);
        assert!(m.notice().unwrap().0.contains("100 of 150"));
        m.set("short");
        assert!(m.notice().is_none());
        assert_eq!(m.take(), "short");
    }

    #[test]
    fn line_breaks_are_kept_tabs_become_spaces_and_control_characters_go() {
        let mut m = MultiLine::new(1000);
        m.paste("a\r\nb\tc\u{7}\nd");
        assert_eq!(m.text, "a\nb    c\nd");
        m.enter_newline = false;
        assert!(!m.key(&Key::Enter), "Enter is the owner's: it sends");
        m.enter_newline = true;
        assert!(m.key(&Key::Enter));
    }
}
