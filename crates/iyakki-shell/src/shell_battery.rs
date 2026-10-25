//! The shell's side of the battery indicator: reading the machine on the heartbeat, turning warnings into toasts and cards, the
//! popup's keys and the clean shutdown at the critical level. The decisions are `iyakki_agent::battery`'s (tested there); what
//! the chip and popup look like is `batterybar.rs`.

use super::*;
use crate::batterybar as bb;
use iyakki_agent::battery::Alert;

/// Seconds since the shell first asked: a clock that does not jump when the wall clock is set.
pub(crate) fn steady_secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs()
}

impl Shell {
    /// The heartbeat's work (about twice a second): the battery is read when it is due (every 10 s, every second while a
    /// countdown shows; the adapter's file every second).
    pub(super) fn battery_tick(&mut self) {
        self.battery_step(steady_secs());
    }

    /// One heartbeat at `now` seconds.
    pub(crate) fn battery_step(&mut self, now: u64) {
        self.battery.settings = self.env.services.settings.lock().map(|s| s.get().battery.clone()).unwrap_or_default();
        let before = self.battery.reading.clone();
        let was_counting = self.battery.counting_down();
        let alerts = self.battery.poll(now, false);
        if before != self.battery.reading || !alerts.is_empty() || was_counting || self.battery.counting_down() {
            self.stale = true;
        }
        self.battery_apply(alerts);
    }

    /// Toasts, cards and the shutdown for what the engine decided.
    pub(crate) fn battery_apply(&mut self, alerts: Vec<Alert>) {
        for a in alerts {
            match a {
                Alert::Low(level) => {
                    let t = bb::toast_text(level, &self.battery.reading);
                    self.say_info(t);
                    let _ = crate::cues::start(&self.env.services, &self.cloud, iyakki_agent::syssounds::Cue::Notify);
                }
                Alert::PlugInNow => {
                    self.battery.close();
                }
                Alert::CountdownStart => {
                    self.battery.close();
                    let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_else(|| "battery".to_string());
                    self.env.audit.record(&user, "battery.critical", &self.battery.reading.percent_text());
                }
                Alert::CountdownCancelled => self.say_info("Shutdown cancelled."),
                Alert::Shutdown => self.battery_shutdown(),
            }
            self.stale = true;
        }
    }

    /// The clean shutdown at the critical level: the same safe power-off the power menu uses (audited), only when this host can.
    fn battery_shutdown(&mut self) {
        if !self.env.host.can_power() {
            self.say("The battery is almost empty. Plug in the power adapter.");
            return;
        }
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_else(|| "battery".to_string());
        self.env.audit.record(&user, "power.off", "battery critical");
        self.env.host.power_off();
    }

    /// F4 or a click on the chip.
    pub(super) fn toggle_battery(&mut self) {
        if self.session.is_none() || !self.battery.shown() {
            return;
        }
        self.help = false;
        self.devpop.close();
        self.brand.close();
        self.installs.close();
        self.battery.toggle();
        self.stale = true;
    }

    /// Keys and clicks of the popup while it is open.
    pub(super) fn handle_battery(&mut self, ev: &Event) {
        match ev {
            Event::Key(Key::Esc) | Event::Tab(bb::CLOSE) | Event::Key(Key::F(bb::KEY)) => self.battery.close(),
            Event::Tab(bb::SETTINGS) | Event::Key(Key::Enter) => {
                self.battery.close();
                self.jump_to("settings", "tab:battery");
            }
            _ => {}
        }
        self.stale = true;
    }

    /// Cancel on the countdown card, or OK on the plug-in card.
    pub(super) fn battery_card_event(&mut self, ev: &Event) {
        match ev {
            Event::Tab(bb::CANCEL) => self.battery.cancel_countdown(),
            Event::Tab(bb::CARD_OK) => self.battery.dismiss_card(),
            // Esc does whichever card is showing
            _ => {
                if self.battery.counting_down() {
                    self.battery.cancel_countdown();
                } else {
                    self.battery.dismiss_card();
                }
            }
        }
        self.stale = true;
    }

    /// Look at a made-up /sys and read it now (tests).
    #[cfg(test)]
    pub(crate) fn battery_set_env_for_test(&mut self, root: &std::path::Path) {
        self.battery.set_env(iyakki_agent::battery::BatteryEnv::at(root));
        self.battery_step(0);
    }
}
