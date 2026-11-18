//! The CLI tile's interactive session (F-CLI-08): a login on a pseudo-terminal drawn in the tile. `TermView` holds the terminal engine
//! (`iyakki-vt`) and draws its screen as coloured text on a character grid; the keys of the tile go to the program, except F10 (close),
//! Shift+PageUp/PageDown and the wheel (scroll back). The engine sits behind a `Link`, so the tests drive it with a fake.

use iyakki_vt::{Cell, Color, Key as VKey, Link, Mods, Terminal, BOLD, DIM as VDIM, HIDDEN, REVERSE, UNDERLINE};

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, Nav, MUTED, R};

const FG: [u8; 4] = rgb(222, 226, 240);
const PANEL_BG: [u8; 4] = rgb(10, 13, 24);
const BANNER: [u8; 4] = rgb(255, 196, 94);

/// The 16 base colours (a dark-background terminal scheme).
const BASE: [[u8; 3]; 16] = [[0, 0, 0], [205, 49, 49], [13, 188, 121], [229, 229, 16], [36, 114, 200], [188, 63, 188], [17, 168, 205], [229, 229, 229], [102, 102, 102], [241, 76, 76], [35, 209, 139], [245, 245, 67], [59, 142, 234], [214, 112, 214], [41, 184, 219], [255, 255, 255]];

/// The colour of an index of the 256-colour palette.
pub fn palette(i: u8) -> [u8; 4] {
    let i = i as usize;
    if i < 16 {
        let c = BASE[i];
        return rgb(c[0], c[1], c[2]);
    }
    if i >= 232 {
        let v = (8 + (i - 232) * 10) as u8;
        return rgb(v, v, v);
    }
    let n = i - 16;
    let lvl = |x: usize| if x == 0 { 0u8 } else { (55 + x * 40) as u8 };
    rgb(lvl(n / 36), lvl(n / 6 % 6), lvl(n % 6))
}

fn color_of(c: Color, default: [u8; 4]) -> [u8; 4] {
    match c {
        Color::Default => default,
        Color::Indexed(i) => palette(i),
        Color::Rgb(r, g, b) => rgb(r, g, b),
    }
}

/// A key of the tile as the terminal engine knows it, with its modifiers. `None` for keys the terminal does not send.
pub fn map_key(k: &Key) -> Option<(VKey, Mods)> {
    let none = Mods::default();
    Some(match k {
        Key::Char(c) => (VKey::Char(*c), none),
        Key::Ctrl(c) => (VKey::Char(c.to_ascii_lowercase()), Mods { ctrl: true, shift: c.is_ascii_uppercase(), alt: false }),
        Key::Alt(c) => (VKey::Char(c.to_ascii_lowercase()), Mods { ctrl: false, shift: c.is_ascii_uppercase(), alt: true }),
        Key::Enter => (VKey::Enter, none),
        Key::Backspace => (VKey::Backspace, none),
        Key::Delete => (VKey::Delete, none),
        Key::Tab => (VKey::Tab, none),
        Key::BackTab => (VKey::BackTab, none),
        Key::Esc => (VKey::Esc, none),
        Key::Up => (VKey::Up, none),
        Key::Down => (VKey::Down, none),
        Key::Left => (VKey::Left, none),
        Key::Right => (VKey::Right, none),
        Key::Home => (VKey::Home, none),
        Key::End => (VKey::End, none),
        Key::PageUp => (VKey::PageUp, none),
        Key::PageDown => (VKey::PageDown, none),
        Key::F(n) => (VKey::F(*n), none),
        Key::Nav { nav, shift, ctrl } => {
            let k = match nav {
                Nav::Up => VKey::Up,
                Nav::Down => VKey::Down,
                Nav::Left => VKey::Left,
                Nav::Right => VKey::Right,
                Nav::Home => VKey::Home,
                Nav::End => VKey::End,
                Nav::PageUp => VKey::PageUp,
                Nav::PageDown => VKey::PageDown,
            };
            (k, Mods { ctrl: *ctrl, shift: *shift, alt: false })
        }
    })
}

/// What the keyboard did to the view.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Sent to the program or scrolled the view.
    Used,
    /// The person asked to close the session.
    Close,
    /// Not for the session.
    Ignored,
}

pub struct TermView {
    term: Terminal<Box<dyn Link>>,
    /// What the session is called in the title.
    pub title: String,
    /// Kept alive while the session runs (the private folder with the keys, for instance).
    _keep: Option<Box<dyn std::any::Any>>,
    /// Size of one character cell and of the grid at the last draw.
    cell: (f32, f32),
    pub grid: (usize, usize),
}

impl TermView {
    pub fn new(term: Terminal<Box<dyn Link>>, title: &str, keep: Option<Box<dyn std::any::Any>>) -> TermView {
        TermView { grid: (term.screen.cols(), term.screen.rows()), term, title: title.to_string(), _keep: keep, cell: (0.0, 0.0) }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn terminal(&mut self) -> &mut Terminal<Box<dyn Link>> {
        &mut self.term
    }

    /// The exit code once the program has ended and its last output is on the screen.
    pub fn ended(&mut self) -> Option<i32> {
        self.term.finished().then(|| self.term.exit_code().unwrap_or(-1))
    }

    /// One heartbeat: read what the program wrote.
    pub fn tick(&mut self) {
        // a few reads so a burst of output (a build, `cat` of a big file) catches up without waiting for the next heartbeat
        for _ in 0..4 {
            if !self.term.poll() {
                break;
            }
        }
    }

    pub fn close(&mut self) {
        self.term.close();
    }

    /// Keys, paste and the wheel. F10 closes; Shift+PageUp/PageDown and the wheel scroll the view back.
    pub fn event(&mut self, ev: &Event) -> Outcome {
        if self.ended().is_some() {
            // the program is gone: a key closes the view
            return match ev {
                Event::Key(Key::Enter) | Event::Key(Key::Esc) | Event::Key(Key::F(10)) => Outcome::Close,
                Event::Key(Key::PageUp) | Event::Scroll(_) | Event::Key(Key::Nav { .. }) => {
                    self.scroll(ev);
                    Outcome::Used
                }
                _ => Outcome::Ignored,
            };
        }
        match ev {
            Event::Key(Key::F(10)) => Outcome::Close,
            Event::Key(Key::Nav { nav: Nav::PageUp | Nav::PageDown, shift: true, .. }) | Event::Scroll(_) => {
                self.scroll(ev);
                Outcome::Used
            }
            Event::Paste(t) => {
                self.term.paste(t);
                Outcome::Used
            }
            Event::Key(k) => match map_key(k) {
                Some((vk, m)) => {
                    self.term.key(vk, m);
                    Outcome::Used
                }
                None => Outcome::Ignored,
            },
            _ => Outcome::Ignored,
        }
    }

    fn scroll(&mut self, ev: &Event) {
        let page = self.term.screen.rows().saturating_sub(1).max(1) as isize;
        match ev {
            Event::Scroll(d) => self.term.scroll_back(*d as isize * 3),
            Event::Key(Key::Nav { nav: Nav::PageUp, .. }) | Event::Key(Key::PageUp) => self.term.scroll_back(page),
            Event::Key(Key::Nav { nav: Nav::PageDown, .. }) => self.term.scroll_back(-page),
            _ => {}
        }
    }

    /// Draw the session in `area` (which it fills) and fit the terminal to it.
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let size = 15.0 * s;
        let cw = g.text_width(size, "M", Face::Mono).max(1.0);
        let lh = g.line_height(size, Face::Mono).max(1.0);
        self.cell = (cw, lh);
        crate::fx::panel(g, area, s, MUTED);
        let top = 30.0 * s;
        let inner = R::new(area.x + 12.0 * s, area.y + top, area.w - 24.0 * s, area.h - top - 10.0 * s);
        g.rect(inner.x, inner.y, inner.w, inner.h, PANEL_BG);
        g.text(area.x + 14.0 * s, area.y + 6.0 * s, 14.0 * s, MUTED, &format!("{}  ·  F10 closes · Shift+PgUp scrolls", if self.term.screen.title.is_empty() { self.title.clone() } else { format!("{} — {}", self.title, self.term.screen.title) }), Face::Sans);
        let cols = ((inner.w / cw).floor() as usize).clamp(10, 500);
        let rows = ((inner.h / lh).floor() as usize).clamp(3, 200);
        self.term.resize(cols, rows);
        self.grid = (cols, rows);
        let back = self.term.back;
        let ended = self.ended();
        let screen = &self.term.screen;
        for y in 0..screen.rows() {
            let row: Vec<Cell> = screen.view_row(back, y);
            let py = inner.y + y as f32 * lh;
            // backgrounds first (runs of one colour), then the text of runs that share their look
            let mut x = 0;
            while x < row.len() {
                let c = row[x];
                let (fg, bg) = look(&c);
                let mut end = x + 1;
                while end < row.len() && look(&row[end]) == (fg, bg) && row[end].attrs == c.attrs {
                    end += 1;
                }
                if let Some(b) = bg {
                    g.rect(inner.x + x as f32 * cw, py, (end - x) as f32 * cw, lh, b);
                }
                if c.attrs & HIDDEN == 0 {
                    let text: String = row[x..end].iter().filter(|c| !c.wide_tail).map(|c| c.ch).collect();
                    if !text.trim().is_empty() {
                        g.text(inner.x + x as f32 * cw, py, size, fg, &text, Face::Mono);
                    }
                }
                if c.attrs & UNDERLINE != 0 {
                    g.rect(inner.x + x as f32 * cw, py + lh - 2.0 * s, (end - x) as f32 * cw, s.max(1.0), fg);
                }
                x = end;
            }
        }
        // the cursor: a block that blinks, only on the live screen
        if back == 0 && screen.cursor_visible && (tick / 15) % 2 == 0 && screen.x < screen.cols() && screen.y < screen.rows() {
            let (cx, cy) = (inner.x + screen.x as f32 * cw, inner.y + screen.y as f32 * lh);
            g.rect(cx, cy, cw, lh, FG);
            let c = screen.cell(screen.x, screen.y);
            if c.ch != ' ' {
                g.text(cx, cy, size, PANEL_BG, &c.ch.to_string(), Face::Mono);
            }
        }
        if back > 0 {
            g.text(inner.x + inner.w - 190.0 * s, inner.y + 4.0 * s, 13.0 * s, BANNER, &format!("{back} lines back · type to return"), Face::Sans);
        }
        if let Some(code) = ended {
            let msg = format!("The session has ended (exit code {code}). Press Enter or F10 to close.");
            let w = g.text_width(14.0 * s, &msg, Face::Sans);
            g.rect(inner.x, inner.y + inner.h - 26.0 * s, inner.w.max(w + 20.0 * s), 26.0 * s, rgb(40, 30, 10));
            g.text(inner.x + 10.0 * s, inner.y + inner.h - 22.0 * s, 14.0 * s, BANNER, &msg, Face::Sans);
        }
        let w = g.button(area.x + area.w - 150.0 * s, area.y + 3.0 * s, 13.0 * s, "Close (F10)", MUTED, Event::Key(Key::F(10)));
        let _ = w;
    }

}

/// Colours of a cell as (text, background); a background of the default colour is not drawn. Reverse swaps them; bold lifts the
/// eight dark colours to their bright twins; dim darkens.
fn look(c: &Cell) -> ([u8; 4], Option<[u8; 4]>) {
    let mut fg = match c.fg {
        Color::Indexed(i) if i < 8 && c.attrs & BOLD != 0 => palette(i + 8),
        other => color_of(other, FG),
    };
    let mut bg = match c.bg {
        Color::Default => None,
        other => Some(color_of(other, PANEL_BG)),
    };
    if c.attrs & REVERSE != 0 {
        let b = bg.unwrap_or(PANEL_BG);
        bg = Some(fg);
        fg = b;
    }
    if c.attrs & VDIM != 0 {
        fg = [fg[0] / 2 + 20, fg[1] / 2 + 20, fg[2] / 2 + 20, fg[3]];
    }
    (fg, bg)
}
