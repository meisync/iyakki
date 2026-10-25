//! The shell's side of the drive banner: reading the watcher's state on the heartbeat, the buttons (Reconnect now, Restart, Check the
//! drive, Minimise) and writing the RAM event list to the audit log once the drive is back. What the card looks like is
//! `lossbar.rs`; the states and the wording are `iyakki_storage::loss`'s.

use super::*;
use iyakki_storage::loss;

impl Shell {
    /// The heartbeat's work (about twice a second): look at the published state, notice a change, flush the RAM events.
    pub(super) fn loss_tick(&mut self) {
        let snap = loss::snapshot();
        if self.loss.update(&snap, self.tick) {
            self.stale = true;
        }
        if snap.state.healthy() {
            // the audit log lives on the drive: what happened while it was away is written now, once
            for ev in loss::take_events_for_audit() {
                self.env.audit.record("system", "storage.event", &format!("uptime {} s: {}", ev.uptime, ev.text));
            }
        }
    }

    /// A click on one of the banner's buttons.
    pub(super) fn loss_event(&mut self, id: usize) {
        use crate::lossbar as lb;
        match id {
            lb::RECONNECT => {
                loss::request_reattach();
                self.say_info("Reconnecting the drive...");
            }
            lb::CHECK => {
                loss::request_check();
                self.say_info("Checking the drive (it only reads, nothing is changed)...");
            }
            lb::RESTART => {
                if self.env.host.can_power() {
                    self.env.host.reboot();
                } else {
                    self.say("Restarting is only possible on the IYAKKI system itself.");
                }
            }
            lb::MINIMISE => self.loss.minimised = true,
            lb::EXPAND => self.loss.minimised = false,
            lb::CARD_OK => self.loss.dismiss_card(),
            _ => {}
        }
        self.stale = true;
    }

    /// Draw the banner over everything (every screen, signed in or not).
    pub(super) fn loss_render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let snap = loss::snapshot();
        crate::lossbar::render(&mut self.loss, g, w, h, s, &snap);
    }
}
