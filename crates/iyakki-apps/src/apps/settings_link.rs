//! The tick of the live WiFi line on the Network page (the work is `iyakki_shared::wifilink`).
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

impl super::SettingsApp {
    /// Called on every tick: reads the signal and runs the sign-in check on the timers while the Network page is open.
    pub(super) fn link_tick(&mut self, cx: &super::AppCx) {
        if self.tab() != super::Tab::Network {
            return;
        }
        let t0 = *self.link_t0.get_or_insert_with(std::time::Instant::now);
        let now = t0.elapsed().as_secs();
        let Some(iface) = self.wifi.iface() else { return };
        let w = self.wifi.wifi.clone();
        let link: iyakki_shared::wifilink::LinkJob = Arc::new(move || w.link_text(&iface));
        let probe: Option<iyakki_shared::wifilink::ProbeJob> = iyakki_shared::wifilink::probe_job(cx.data_dir());
        self.link.tick(now, Some(link), probe);
    }
}

