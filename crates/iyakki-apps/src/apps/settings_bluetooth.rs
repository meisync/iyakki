//! Settings: the Bluetooth page (lane ST-D, docs/development/SETTINGS-EXTRACTION-PLAN.md, section 3). The page itself is `bluetooth_page.rs`
//! (`BtPage`, which reads no session). Its two hooks into the shell take a `UserPort`: the heartbeat and the log of what the page did.

use super::*;
use super::super::settings_device_port::UserPort;

impl SettingsApp {
    /// The Bluetooth page's events: only an administrator reaches the page (a person who may not change settings gets the other handlers).
    /// Returns true when the page took the event.
    pub(super) fn bluetooth_event(&mut self, ev: &Event, p: &dyn UserPort) -> bool {
        if !p.is_admin() || (self.nav_focus && matches!(ev, Event::Key(_) | Event::Paste(_))) {
            return false;
        }
        if !self.bt.handle(ev) {
            return false;
        }
        self.bt_drain(p);
        true
    }

    /// A heartbeat on the Bluetooth page: the model reads the machine and the outcomes go to the system log.
    pub(super) fn bt_tick(&mut self, p: &dyn UserPort) {
        let ticks = self.ticks;
        let lines = self.bt.tick(ticks);
        for l in lines {
            p.audit("settings.bluetooth", &l);
        }
    }

    /// The lines an action of the Bluetooth page wrote (requests and outcomes) go to the system log.
    pub(super) fn bt_drain(&mut self, p: &dyn UserPort) {
        for l in self.bt.model.take_log() {
            p.audit("settings.bluetooth", &l);
        }
    }
}
