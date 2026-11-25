//! The tick of the live WiFi line on the Network page (the work is `iyakki_shared::wifilink`).
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::apps::settings::network_port::NetworkSettingsPort;

impl super::SettingsApp {
    /// Called on every tick: reads the signal and runs the sign-in check on the timers while the Network page is open.
    pub(super) fn link_tick(&mut self, p: &dyn NetworkSettingsPort) {
        if self.tab() != super::Tab::Network {
            return;
        }
        let t0 = *self.link_t0.get_or_insert_with(std::time::Instant::now);
        let now = t0.elapsed().as_secs();
        let Some(iface) = self.wifi.iface() else { return };
        let w = self.wifi.wifi.clone();
        let link: iyakki_shared::wifilink::LinkJob = Arc::new(move || w.link_text(&iface));
        let probe: Option<iyakki_shared::wifilink::ProbeJob> = iyakki_shared::wifilink::probe_job(p.data_root());
        self.link.tick(now, Some(link), probe);
    }

    /// The key `o`: the page asked for a sign-in (a hotel or cafe network) is opened in the Browser tile. Only an http or https address
    /// the sign-in check found is opened; with none, it says so and nothing opens.
    pub(super) fn open_sign_in(&mut self, _p: &dyn NetworkSettingsPort) {
        match self.link.sign_in_url().filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
            Some(u) => {
                crate::apps::browser::request_open(&u);
                self.hw.ask_shell("browser", u);
                self.ok("Opening the network's sign-in page in the Browser.".to_string());
            }
            None => self.fail("This network has not asked for a sign-in (or no check address is set up)."),
        }
    }

    /// The key `v`: with two or more WiFi adapters, use the next one for scanning and joining; remembered in `wifi-adapter.txt`.
    pub(super) fn use_next_adapter(&mut self, _p: &dyn NetworkSettingsPort) {
        let all = self.wifi.adapters();
        if all.len() < 2 {
            return self.fail("This computer has one WiFi adapter (or none): there is nothing to switch.");
        }
        let cur = self.wifi.iface();
        let next = all.iter().position(|a| Some(a) == cur.as_ref()).map_or(0, |i| (i + 1) % all.len());
        let name = all[next].clone();
        if let Err(e) = self.wifi.known.set_chosen_adapter(Some(&name)) {
            return self.fail(format!("The choice could not be kept: {e}"));
        }
        self.link = Default::default();
        self.ok(format!("WiFi adapter {name} is in use now."));
    }
}
