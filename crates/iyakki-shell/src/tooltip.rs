//! A tooltip for lists whose rows are cut to the width of the tile: a small panel with the whole text of the row, so an address
//! that did not fit is still readable.
//!
//! One [`Tooltip`] lives in the app that draws the list. The app feeds it every event ([`Tooltip::event`]); while drawing, each
//! row that was cut offers its full text ([`Tooltip::offer`]) and the app ends the frame with [`Tooltip::draw`]. When it appears:
//!
//! * the pointer rests on a row for half a second (mouse),
//! * a row has been selected with the keyboard for 600 ms (the keyboard user also gets the text in the tile's detail line),
//! * a finger or button is held on a row for 600 ms (long press).
//!
//! It goes away when the pointer moves, on any click, release, scroll or key, and on Esc, and comes back only after the next rest.
//! It registers no click region, so it never takes a click or the focus. It is placed from the pointer (or the selected row),
//! wrapped to at most 360 px at scale 1, and kept inside the screen. [`fit_keep_ip`] is the matching way to cut a line: the part
//! before the address gives way first.

use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, Key, MUTED, R, TEXT};

/// Heartbeats (30 per second) a resting mouse pointer waits before the tooltip shows: 500 ms.
pub const POINTER_DELAY: u32 = 15;
/// Heartbeats a keyboard selection or a long press waits: 600 ms.
pub const HOLD_DELAY: u32 = 18;

#[derive(Clone, Copy, PartialEq, Debug)]
enum By {
    Pointer,
    Keyboard,
    /// A press that is still held (a finger, or the button).
    Press,
}

/// The row that asked for the tooltip this frame.
struct Offer {
    rect: R,
    text: String,
    mouse: (i32, i32),
}

pub struct Tooltip {
    by: By,
    /// Heartbeats since the last input that restarts the wait.
    idle: u32,
    /// Something was done in this tile at all (a tile that was just opened shows nothing by itself).
    active: bool,
    /// Put away by a click or Esc until the next movement or key.
    put_away: bool,
    pos: (i32, i32),
    offer: Option<Offer>,
    /// What the last event or heartbeat left visible, to say when the picture changed.
    was: bool,
    changed: bool,
    /// The text of the panel drawn last (None when the last frame drew none): what tests and the shell can read back.
    pub shown: Option<String>,
}

impl Default for Tooltip {
    fn default() -> Tooltip {
        Tooltip::new()
    }
}

impl Tooltip {
    pub fn new() -> Tooltip {
        Tooltip { by: By::Keyboard, idle: 0, active: false, put_away: false, pos: (-1, -1), offer: None, was: false, changed: false, shown: None }
    }

    fn delay(&self) -> u32 {
        if self.by == By::Pointer {
            POINTER_DELAY
        } else {
            HOLD_DELAY
        }
    }

    /// Whether the wait is over (and nothing put the tooltip away).
    pub fn ready(&self) -> bool {
        self.active && !self.put_away && self.idle >= self.delay()
    }

    fn sync(&mut self) {
        let now = self.ready();
        if now != self.was {
            self.was = now;
            self.changed = true;
        }
    }

    /// Feed every event of the tile: movement, clicks, keys and the heartbeat.
    pub fn event(&mut self, ev: &Event) {
        match ev {
            Event::Tick => self.idle = self.idle.saturating_add(1),
            Event::MouseMove(x, y) => {
                if self.pos != (*x, *y) {
                    self.pos = (*x, *y);
                    (self.by, self.idle, self.active, self.put_away) = (By::Pointer, 0, true, false);
                }
            }
            Event::MouseDown(x, y) => {
                // a press hides what is shown; held for long enough it shows the text again (a long press)
                self.pos = (*x, *y);
                (self.by, self.idle, self.active, self.put_away) = (By::Press, 0, true, false);
            }
            Event::MouseUp(..) => (self.by, self.idle, self.put_away) = (By::Pointer, 0, true),
            Event::Scroll(_) => (self.idle, self.put_away) = (0, true),
            Event::Key(Key::Esc) => (self.by, self.idle, self.active, self.put_away) = (By::Keyboard, 0, true, true),
            // a click that the shell turned into a row or tab choice
            Event::Row(_) | Event::Tab(_) => (self.by, self.idle, self.active, self.put_away) = (By::Pointer, 0, true, true),
            Event::Key(_) | Event::Sub(..) => (self.by, self.idle, self.active, self.put_away) = (By::Keyboard, 0, true, false),
            _ => {}
        }
        self.sync();
    }

    /// True once after the tooltip appeared or went away: the tile asks the shell for a new picture (`App::wants_redraw`).
    pub fn take_changed(&mut self) -> bool {
        self.sync();
        std::mem::take(&mut self.changed)
    }

    /// While drawing: this row was cut, `text` is all of it, `selected` says the keyboard selection is on it. Only the row the
    /// pointer (or the selection, when the keyboard was used last) is on is kept.
    pub fn offer(&mut self, g: &Gfx, rect: R, selected: bool, text: &str) {
        let hot = if self.by == By::Keyboard { selected } else { g.hovered(rect) };
        if hot && self.offer.is_none() {
            self.offer = Some(Offer { rect, text: text.to_string(), mouse: g.mouse });
        }
    }

    /// The end of the frame: draw the panel when it is due. Returns its rectangle. Drawn last, over the tile; it registers no
    /// click region, so clicks and the focus go to what is under it.
    pub fn draw(&mut self, g: &mut Gfx, s: f32) -> Option<R> {
        self.shown = None;
        let offer = self.offer.take()?;
        if !self.ready() || offer.text.trim().is_empty() {
            return None;
        }
        self.shown = Some(offer.text.clone());
        let (sw, sh) = (g.width() as f32, g.height() as f32);
        let size = 13.0 * s;
        let maxw = (360.0 * s).min(sw - 16.0 * s).max(40.0);
        let lines: Vec<String> = offer.text.lines().flat_map(|l| g.wrap(size, l, Face::Sans, maxw - 20.0 * s)).collect();
        let lh = g.line_height(size, Face::Sans) + 3.0 * s;
        let pw = (lines.iter().map(|l| g.text_width(size, l, Face::Sans)).fold(0.0f32, f32::max) + 20.0 * s).min(maxw);
        let ph = lines.len() as f32 * lh + 14.0 * s;
        let (ax, below, above) = if self.by == By::Keyboard {
            (offer.rect.x + 16.0 * s, offer.rect.y + offer.rect.h + 4.0 * s, offer.rect.y - ph - 4.0 * s)
        } else {
            let (mx, my) = (offer.mouse.0 as f32, offer.mouse.1 as f32);
            // under the pointer, and above it when there is no room (a finger hides what is under it: the panel clears it by a hand)
            (mx + 10.0 * s, my + 22.0 * s, my - ph - 14.0 * s)
        };
        let x = ax.clamp_safe(4.0 * s, sw - pw - 4.0 * s);
        let y = if below + ph + 4.0 * s <= sh { below } else { above }.clamp_safe(4.0 * s, sh - ph - 4.0 * s);
        let r = R::new(x, y, pw, ph);
        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, rgb(30, 36, 62));
        g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.0, fx::CYAN);
        for (i, l) in lines.iter().enumerate() {
            g.text(r.x + 10.0 * s, r.y + 7.0 * s + i as f32 * lh, size, if i == 0 { TEXT } else { MUTED }, l, Face::Sans);
        }
        Some(r)
    }
}

/// The byte range of the first IPv4 address in `text` (four numbers up to 255 with dots between, a `:port` allowed after it).
pub fn find_ip(text: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            let parts: Vec<&str> = text[i..j].trim_end_matches('.').split('.').collect();
            if parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok()) {
                let mut end = i + text[i..j].trim_end_matches('.').len();
                if b.get(end) == Some(&b':') && b.get(end + 1).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                    while end < b.len() && b[end].is_ascii_digit() {
                        end += 1;
                    }
                }
                return Some((i, end));
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// `text` cut to `max_w` with a trailing `…`, but with the IPv4 address in it kept whole: what comes before the address (the
/// name) gives way first, then the end. Text without an address is cut as [`Gfx::fit`] does.
pub fn fit_keep_ip(g: &mut Gfx, size: f32, text: &str, face: Face, max_w: f32) -> String {
    let plain = g.fit(size, text, face, max_w);
    let Some((a, b)) = find_ip(text) else { return plain };
    if plain.contains(&text[a..b]) {
        return plain;
    }
    let (head, tail) = (&text[..a], &text[a..]);
    let tail_w = g.text_width(size, tail, face);
    let ip_w = g.text_width(size, &text[a..b], face);
    if tail_w <= max_w {
        // the name takes what the rest leaves
        let room = (max_w - tail_w).max(0.0);
        if g.text_width(size, "…", face) < room {
            return format!("{}{tail}", g.fit(size, head, face, room));
        }
        return tail.to_string();
    }
    // the rest is too long as well: a short piece of the name, the address whole, then as much of the rest as fits
    let (ip, rest) = (&text[a..b], &text[b..]);
    let name_room = (max_w * 0.25).min((max_w - ip_w).max(0.0));
    let name = if g.text_width(size, "…", face) < name_room { g.fit(size, head, face, name_room) } else { String::new() };
    let left = max_w - g.text_width(size, &name, face) - ip_w;
    let end = if left > g.text_width(size, "…", face) { g.fit(size, rest, face, left) } else { String::new() };
    format!("{name}{ip}{end}")
}

#[cfg(test)]
#[path = "tooltip_tests.rs"]
mod tests;
