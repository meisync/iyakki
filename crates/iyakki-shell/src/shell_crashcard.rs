//! The shell's side of the crash card (`crashcard.rs`): a caught panic raises it, its buttons reopen the app or dismiss it, and it
//! goes away by itself after half a minute, when the person signs out or when they open anything else.

use crate::stale_stats::mark_stale;
use super::*;

impl Shell {
    /// An app panicked and was closed: say so on the card, with the way back to it.
    pub(super) fn raise_crash_card(&mut self, name: &str, app: Option<PApp>) {
        self.crash = Some(crate::crashcard::CrashCard::new(name, app, self.tick));
        mark_stale!(self);
    }

    /// The card's text, while it is up.
    pub fn crash_card_text(&self) -> Option<String> {
        self.crash.as_ref().map(|c| c.text())
    }

    /// The heartbeat: the card goes away by itself.
    pub(super) fn crash_card_tick(&mut self) {
        if self.crash.as_ref().is_some_and(|c| c.expired(self.tick)) {
            self.crash = None;
            mark_stale!(self);
        }
    }

    /// A press on one of the card's buttons.
    pub(super) fn crash_card_event(&mut self, id: usize) {
        let Some(card) = self.crash.take() else { return };
        if id == crate::crashcard::OPEN {
            if let Some(app) = card.app {
                self.env.audit.record(&self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default(), "app.reopen", app.id());
                self.go_home();
                self.open(app);
            }
        }
        mark_stale!(self);
    }
}
