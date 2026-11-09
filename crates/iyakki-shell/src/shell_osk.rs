//! The shell's side of the on-screen keyboard (`osk.rs`): draws it over everything, takes the presses that land on it, and hands its
//! keys to the screen as the keys of a physical keyboard.

use super::*;
use crate::osk::{Act, Cap, Osk, Shift};

/// `Event::Tab` ids of the keyboard's hit regions (the keyboard takes its own presses; the ids only name them for tests and
/// `iyakki dev state`).
pub(super) const KEY: usize = 9300;
pub(super) const BUTTON: usize = 9301;

impl Shell {
    /// The field that owns the keyboard in the frame just drawn.
    fn osk_focus(&self) -> Option<usize> {
        self.fields.iter().rev().find(|f| f.focused).map(|f| f.id)
    }

    /// How far the sign-in card moves up so the field being typed in is not under the keyboard.
    pub(super) fn osk_auth_lift(&mut self, w: u32, h: u32, s: f32) -> f32 {
        let focus = self.osk_focus();
        if !self.osk.update(focus) {
            return 0.0;
        }
        let (focus, n) = match &self.screen {
            _ if self.lockui.active => (0, 1),
            Screen::Login(f) | Screen::Setup(f) => (f.focus, f.fields.len()),
            _ => return 0.0,
        };
        let field = auth_field_rect(w, h, s, focus, n);
        let kb = Osk::area(w as f32, h as f32, s);
        (field.y + field.h + 14.0 * s - kb.y).max(0.0)
    }

    /// Draw the keyboard, or the small button that opens it, last so nothing covers it.
    pub(super) fn draw_osk(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let focus = self.osk_focus();
        let show = self.osk.update(focus);
        crate::osk::set_touch_mode(self.osk.touch);
        let (wf, hf) = (w as f32, h as f32);
        if !show {
            self.osk.set_frame(None, Vec::new());
            let button = self.osk.wants_button(focus, false).then(|| Osk::button_rect(wf, hf, s));
            self.osk.button = button;
            if let Some(b) = button {
                let hot = g.hovered(b);
                g.rrect(b.x, b.y, b.w, b.h, b.h / 4.0, if hot { rgb(60, 72, 110) } else { rgb(34, 42, 70) });
                g.rrect_outline(b.x, b.y, b.w, b.h, b.h / 4.0, 1.5, rgb(90, 104, 150));
                // a keyboard: a frame of three rows of keys
                let (kx, ky, kw, kh) = (b.x + b.w * 0.2, b.y + b.h * 0.32, b.w * 0.6, b.h * 0.36);
                for row in 0..3 {
                    for col in 0..if row == 1 { 5 } else { 4 } {
                        let n = if row == 1 { 5.0 } else { 4.0 };
                        let (cw, ch) = (kw / n, kh / 3.0);
                        g.rect(kx + col as f32 * cw + 1.0, ky + row as f32 * ch + 1.0, cw - 2.0, ch - 2.0, TEXT);
                    }
                }
                g.hit(b, Event::Tab(BUTTON), "keyboard");
            }
            return;
        }
        self.osk.button = None;
        let area = Osk::area(wf, hf, s);
        let caps = self.osk.layout(area);
        // the backing: dark, a little see-through, with a thin line on top
        g.rrect(area.x, area.y, area.w, area.h, 14.0 * s, rgb(12, 15, 28));
        g.rrect_outline(area.x, area.y, area.w, area.h, 14.0 * s, 1.0, rgb(52, 62, 96));
        for c in &caps {
            let r = c.rect;
            let hot = g.hovered(r);
            let base = if c.accent { rgb(46, 92, 170) } else if matches!(c.act, Act::Char(_)) { rgb(34, 42, 70) } else { rgb(24, 30, 52) };
            let lit = matches!((&c.act, self.osk.shift), (Act::Shift, Shift::Once | Shift::Lock));
            g.rrect(r.x, r.y, r.w, r.h, r.h * 0.18, if hot { rgb(70, 88, 140) } else if lit { rgb(52, 84, 160) } else { base });
            draw_cap(g, c, self.osk.shift);
            g.hit(r, Event::Tab(KEY), &format!("key {}", c.label));
        }
        self.osk.set_frame(Some(area), caps);
    }

    /// Presses on the keyboard are its own; physical keys switch it off. Returns whether the event is used up.
    pub(super) fn osk_intercept(&mut self, ev: &Event) -> bool {
        match ev {
            Event::MouseDown(x, y) => {
                if crate::osk::take_touch() {
                    self.osk.touched(self.tick);
                    crate::osk::set_touch_mode(true);
                    self.stale = true;
                }
                if let Some(b) = self.osk.button {
                    if b.contains(*x, *y) {
                        self.osk.manual = true;
                        self.osk.dismissed = false;
                        self.stale = true;
                        return true;
                    }
                }
                match self.osk.press(*x, *y, self.tick) {
                    None => false,
                    Some(key) => {
                        self.stale = true;
                        if let Some(k) = key {
                            self.osk_type(&k);
                        }
                        true
                    }
                }
            }
            Event::MouseUp(..) => self.osk.release(),
            Event::Tick => {
                if let Some(k) = self.osk.repeat(self.tick) {
                    self.osk_type(&k);
                }
                false
            }
            Event::Key(_) if !self.osk.typing => {
                self.osk.physical_key(self.tick);
                crate::osk::set_touch_mode(self.osk.touch);
                false
            }
            _ => false,
        }
    }

    /// Deliver a key of the keyboard as a physical one would arrive.
    fn osk_type(&mut self, ev: &Event) {
        self.osk.typing = true;
        self.handle_awake(ev);
        self.osk.typing = false;
        self.stale = true;
    }
}

/// The face of one key: its character, or a small drawing for the keys without one.
fn draw_cap(g: &mut Gfx, c: &Cap, shift: Shift) {
    let r = c.rect;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let u = r.h * 0.2;
    let ink = TEXT;
    match c.label.as_str() {
        "shift" | "caps" => {
            g.polygon(&[(cx, cy - u * 1.3), (cx + u * 1.3, cy), (cx + u * 0.55, cy), (cx + u * 0.55, cy + u * 1.1), (cx - u * 0.55, cy + u * 1.1), (cx - u * 0.55, cy), (cx - u * 1.3, cy)], if shift == Shift::Off { ink } else { rgb(255, 255, 255) });
            if c.label == "caps" {
                g.rect(cx - u * 0.55, cy + u * 1.35, u * 1.1, u * 0.3, rgb(255, 255, 255));
            }
        }
        "backspace" => {
            g.polygon(&[(cx - u * 1.6, cy), (cx - u * 0.6, cy - u * 1.1), (cx + u * 1.6, cy - u * 1.1), (cx + u * 1.6, cy + u * 1.1), (cx - u * 0.6, cy + u * 1.1)], ink);
            g.line(cx - u * 0.2, cy - u * 0.5, cx + u * 0.9, cy + u * 0.5, 2.0, rgb(24, 30, 52));
            g.line(cx - u * 0.2, cy + u * 0.5, cx + u * 0.9, cy - u * 0.5, 2.0, rgb(24, 30, 52));
        }
        "left" => g.polygon(&[(cx - u, cy), (cx + u * 0.8, cy - u * 1.1), (cx + u * 0.8, cy + u * 1.1)], ink),
        "right" => g.polygon(&[(cx + u, cy), (cx - u * 0.8, cy - u * 1.1), (cx - u * 0.8, cy + u * 1.1)], ink),
        "enter" => {
            g.line(cx + u * 1.2, cy - u * 1.1, cx + u * 1.2, cy + u * 0.4, 3.0, ink);
            g.line(cx + u * 1.2, cy + u * 0.4, cx - u * 0.9, cy + u * 0.4, 3.0, ink);
            g.polygon(&[(cx - u * 1.6, cy + u * 0.4), (cx - u * 0.6, cy - u * 0.4), (cx - u * 0.6, cy + u * 1.2)], ink);
        }
        "hide" => {
            g.line(cx - u * 1.1, cy - u * 0.4, cx, cy + u * 0.7, 3.0, ink);
            g.line(cx, cy + u * 0.7, cx + u * 1.1, cy - u * 0.4, 3.0, ink);
        }
        label => {
            let size = if label.chars().count() > 1 { r.h * 0.3 } else { r.h * 0.44 };
            let colour = if label == "space" { DIM } else { ink };
            let tw = g.text_width(size, label, Face::Sans);
            let lh = g.line_height(size, Face::Sans);
            g.text(cx - tw / 2.0, cy - lh / 2.0, size, colour, label, Face::Sans);
        }
    }
}
