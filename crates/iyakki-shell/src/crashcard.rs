//! The card shown when an app stopped because of an internal error (F-SHELL-40): what happened in plain words, a button that opens
//! the app again, and one that dismisses the card. Only the app that failed is closed (the shell catches the panic); this card is how
//! the person gets it back with one press instead of finding its tile. It sits at the bottom of the screen above every app, is
//! dismissed by its button, by opening anything, or by itself after half a minute. The text and the geometry are pure so the tests can
//! check them at every size without a shell.

use crate::fx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, ERR, MUTED, R, TEXT};
use iyakki_sec::policy::App as PApp;

/// Click regions.
pub const OPEN: usize = 1_450_001;
pub const DISMISS: usize = 1_450_002;

/// How long the card stays when nobody answers (heartbeats, about 30 a second): half a minute.
pub const KEEP_TICKS: u64 = 900;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashCard {
    /// What the app calls itself (its title).
    pub name: String,
    /// The tile to open again; `None` for an app that has no tile (a custom app), which then only has Dismiss.
    pub app: Option<PApp>,
    /// The heartbeat at which the card goes away by itself.
    pub until: u64,
}

impl CrashCard {
    pub fn new(name: &str, app: Option<PApp>, tick: u64) -> CrashCard {
        CrashCard { name: name.to_string(), app, until: tick + KEEP_TICKS }
    }

    /// The words on the card.
    pub fn text(&self) -> String {
        format!("{} stopped because of an internal error. Details are in the system log. Open it again to restart it.", self.name)
    }

    pub fn expired(&self, tick: u64) -> bool {
        tick >= self.until
    }

    /// The card's rectangle and the wrapped lines of its text, for a screen of `w` x `h` at scale `s`.
    pub fn layout(&self, g: &mut Gfx, w: f32, h: f32, s: f32) -> (R, Vec<String>) {
        let cw = (w - 32.0 * s).min(640.0 * s).max(200.0 * s);
        let lines = g.wrap(15.0 * s, &self.text(), Face::Sans, cw - 32.0 * s);
        let ch = 24.0 * s + lines.len().min(3) as f32 * 20.0 * s + 14.0 * s + 15.0 * s * 2.1 + 14.0 * s;
        let card = R::new((w - cw) / 2.0, (h - ch - 24.0 * s).max(0.0), cw, ch);
        (card, lines)
    }

    pub fn render(&self, g: &mut Gfx, w: f32, h: f32, s: f32) {
        let (card, lines) = self.layout(g, w, h, s);
        fx::modal_panel(g, card, s, ERR, true);
        // the card swallows clicks on itself so nothing under it is pressed by accident
        g.hit(card, Event::Tick, "app stopped");
        let mut y = card.y + 14.0 * s;
        for l in lines.iter().take(3) {
            g.text(card.x + 16.0 * s, y, 15.0 * s, TEXT, l, Face::Sans);
            y += 20.0 * s;
        }
        let by = y + 8.0 * s;
        let mut bx = card.x + 16.0 * s;
        if self.app.is_some() {
            bx += g.button(bx, by, 15.0 * s, "Open again", fx::CYAN, Event::Tab(OPEN)) + 10.0 * s;
        }
        g.button(bx, by, 15.0 * s, "Dismiss", MUTED, Event::Tab(DISMISS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_says_what_stopped_and_offers_to_open_it_again_only_when_it_has_a_tile() {
        let mut g = Gfx::new(1280, 720);
        let c = CrashCard::new("Bomb", Some(PApp::Chat), 10);
        assert!(c.text().contains("Bomb stopped because of an internal error") && c.text().contains("Open it again"));
        c.render(&mut g, 1280.0, 720.0, 1.0);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(OPEN)) && g.hits.iter().any(|h| h.ev == Event::Tab(DISMISS)));
        let mut g = Gfx::new(1280, 720);
        CrashCard::new("Custom", None, 10).render(&mut g, 1280.0, 720.0, 1.0);
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(OPEN)) && g.hits.iter().any(|h| h.ev == Event::Tab(DISMISS)));
        assert!(!c.expired(10 + KEEP_TICKS - 1) && c.expired(10 + KEEP_TICKS));
    }

    #[test]
    fn the_card_and_its_buttons_stay_on_the_screen_at_four_sizes_with_a_long_name() {
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            let mut g = Gfx::new(w, h);
            let c = CrashCard::new("An app with a very long name that goes on and on and on for quite a while", Some(PApp::Cli), 0);
            c.render(&mut g, w as f32, h as f32, s);
            let screen = R::new(0.0, 0.0, w as f32, h as f32);
            for hit in g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(OPEN) | Event::Tab(DISMISS))) {
                assert!(hit.rect.x >= 0.0 && hit.rect.y >= 0.0 && hit.rect.x + hit.rect.w <= screen.w + 0.5 && hit.rect.y + hit.rect.h <= screen.h + 0.5, "{w}x{h}: {:?}", hit.rect);
            }
            let (card, _) = c.layout(&mut g, w as f32, h as f32, s);
            let (o, d) = (g.hits.iter().find(|x| x.ev == Event::Tab(OPEN)).unwrap().rect, g.hits.iter().find(|x| x.ev == Event::Tab(DISMISS)).unwrap().rect);
            assert!(o.x + o.w <= d.x + 0.5 || o.y + o.h <= d.y, "{w}x{h}: the buttons do not overlap");
            assert!(d.y + d.h <= card.y + card.h + 0.5, "{w}x{h}: the buttons are inside the card");
        }
    }
}
