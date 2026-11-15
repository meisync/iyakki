//! Sign out by keyboard asks first. Control+Command+Q (Mac profile; Control+Option+Q too) or Control+Windows+Q (PC profile) and the
//! one keys on the Home screen (`o`, `q`, Esc) open a small card: "Sign out? Open apps close and the vault locks." Enter or `y` signs
//! out, Esc or `n` stays (a second Esc never signs out); the two buttons answer to the mouse and to a touch. The Sign out button of the
//! top bar stays immediate. The card is never shown on the sign-in screen, and the lock screen is above it (it takes every key first).
//!
//! After any sign-out the sign-in screen says "You signed out" for a few seconds (or until a key is typed), so it is not mistaken for a
//! lock: a lock shows the "Locked" card with a password field and the person is still signed in.

use crate::stale_stats::mark_stale;
use super::*;

pub(super) const YES: usize = 0x4D70;
pub(super) const NO: usize = 0x4D71;
/// How long the "You signed out" line stays, in ticks (about five seconds).
const NOTE_TICKS: u64 = 150;

#[derive(Default)]
pub(super) struct SignoutUi {
    pub card: bool,
    /// The button Enter would press only for the keyboard walk: 0 = Sign out, 1 = Stay. Enter and y always sign out.
    pub focus: usize,
    /// The "You signed out" line is shown until this tick (0: not shown).
    pub note_until: u64,
}

impl Shell {
    pub fn signout_card_open(&self) -> bool {
        self.signout.card
    }

    /// The "You signed out" line is showing on the sign-in screen.
    pub fn signed_out_note_shown(&self) -> bool {
        self.signout.note_until > self.tick && self.session.is_none()
    }

    /// Open the confirm card (a chord or a Home key). Nothing happens on the sign-in screen or while the screen is locked.
    pub(super) fn signout_ask(&mut self) {
        if self.session.is_none() || self.lockui.active || self.signout.card {
            return;
        }
        self.signout.card = true;
        self.signout.focus = 0;
        mark_stale!(self);
        iyakki_uikit::access::announce("Sign out? Open apps close and the vault locks. Enter signs out, Escape stays.");
    }

    pub(super) fn signout_started(&mut self, had_session: bool) {
        self.signout.card = false;
        self.signout.note_until = if had_session { self.tick + NOTE_TICKS } else { 0 };
        if had_session {
            iyakki_uikit::access::announce("You signed out");
        }
    }

    /// True when the card (or the note's timing) took the event.
    pub(super) fn signout_intercept(&mut self, ev: &Event) -> bool {
        if self.signout.note_until != 0 {
            let typed = matches!(ev, Event::Key(_) | Event::Paste(_) | Event::MouseDown(..));
            if self.tick >= self.signout.note_until || typed {
                self.signout.note_until = 0;
                mark_stale!(self);
            }
        }
        if !self.signout.card {
            return false;
        }
        if self.session.is_none() {
            self.signout.card = false;
            return false;
        }
        let answer = match ev {
            Event::Key(Key::Enter) | Event::Key(Key::Char('y')) => Some(true),
            Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => Some(false),
            Event::Tab(YES) => Some(true),
            Event::Tab(NO) => Some(false),
            Event::Key(Key::Left | Key::Right | Key::Tab | Key::BackTab | Key::Up | Key::Down) => {
                self.signout.focus = 1 - self.signout.focus;
                mark_stale!(self);
                return true;
            }
            Event::MouseDown(x, y) => {
                // a touch arrives as a press too: the buttons answer, a press anywhere else is ignored
                let hit = self.hits.iter().rev().find(|h| h.rect.contains(*x, *y)).map(|h| h.ev.clone());
                match hit {
                    Some(Event::Tab(YES)) => Some(true),
                    Some(Event::Tab(NO)) => Some(false),
                    _ => None,
                }
            }
            // the heartbeat, the pointer moving, the Fn keys (volume) go on
            Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::Fn(..) | Event::FocusAt(..) => return false,
            _ => None,
        };
        mark_stale!(self);
        match answer {
            Some(true) => {
                self.signout.card = false;
                self.sign_out();
            }
            Some(false) => self.signout.card = false,
            None => {}
        }
        true
    }

    pub(super) fn draw_signout(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if self.session.is_none() {
            self.draw_signed_out_note(g, w, h, s);
            return;
        }
        if !self.signout.card || self.lockui.active {
            return;
        }
        let area = R::new(0.0, 0.0, w as f32, h as f32);
        let p = crate::fx::modal(g, area, s, 520.0 * s, 250.0 * s, crate::fx::CYAN, false);
        g.text(p.x + 24.0 * s, p.y + 20.0 * s, 22.0 * s, TEXT, "Sign out?", Face::Bold);
        let body = "Open apps close and the vault locks.";
        for (i, line) in g.wrap(16.0 * s, body, Face::Sans, p.w - 48.0 * s).into_iter().enumerate() {
            g.text(p.x + 24.0 * s, p.y + 62.0 * s + i as f32 * 24.0 * s, 16.0 * s, MUTED, &line, Face::Sans);
        }
        let hint = g.fit(14.0 * s, "Enter or y signs out. Esc or n stays.", Face::Sans, p.w - 48.0 * s);
        g.text(p.x + 24.0 * s, p.y + 112.0 * s, 14.0 * s, DIM, &hint, Face::Sans);
        let bw = ((p.w - 24.0 * s * 2.0 - 12.0 * s) / 2.0).min(220.0 * s);
        let y = p.y + p.h - 64.0 * s;
        for (i, (label, id)) in [("Sign out", YES), ("Stay", NO)].into_iter().enumerate() {
            let r = R::new(p.x + p.w - 24.0 * s - bw * (2 - i) as f32 - 12.0 * s * (1 - i) as f32, y, bw, 44.0 * s);
            let hot = g.hovered(r) || self.signout.focus == i;
            let c = crate::fx::pill(g, r, s, crate::fx::CYAN, hot, true);
            let lw = g.text_width(16.0 * s, label, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 16.0 * s) / 2.0 - s, 16.0 * s, c, label, Face::Bold);
            g.hit(r, Event::Tab(id), label);
        }
    }

    fn draw_signed_out_note(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if !matches!(self.screen, Screen::Login(_)) || self.signout.note_until <= self.tick {
            return;
        }
        let size = 16.0 * s;
        let text = "You signed out";
        let tw = g.text_width(size, text, Face::Sans) + 40.0 * s;
        let r = R::new((w as f32 - tw) / 2.0, h as f32 - 96.0 * s, tw, 38.0 * s);
        g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(20, 26, 46));
        g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.2 * s, rgb(70, 82, 120));
        g.text(r.x + 20.0 * s, r.y + (r.h - size) / 2.0 - s, size, MUTED, text, Face::Sans);
    }
}
