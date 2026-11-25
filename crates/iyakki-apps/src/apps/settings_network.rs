//! Settings: the network page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::*;
use crate::apps::settings::network_port::NetworkSettingsPort;

impl SettingsApp {
    /// `m` on the Network page: choose the private MAC address mode for the kind of adapter that is selected.
    pub(super) fn choose_mac_mode(&mut self, p: &dyn NetworkSettingsPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the network.");
        }
        if self.hw.mac().host_managed {
            return self.fail("The MAC address is managed by the host operating system here (this is a window on another computer).");
        }
        let Some((_, kind)) = self.selected_adapter() else { return self.fail("Select an Ethernet or WiFi row first.") };
        let title = format!("Private MAC address for {}", if kind == iyakki_core_api::device_types::devctl::LinkKind::Wifi { "WiFi" } else { "Ethernet" });
        let items = iyakki_core_api::device_types::devctl::mac::MacMode::ALL.iter().map(|m| m.label().to_string()).collect();
        self.modal = Some(Modal::Pick(PickKind::MacMode(kind), Picker::new(&title, items)));
    }

    /// `a` on the Network page: ask before changing the selected adapter's address now.
    pub(super) fn ask_mac_apply(&mut self, p: &dyn NetworkSettingsPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the network.");
        }
        if self.hw.mac().host_managed {
            return self.fail("The MAC address is managed by the host operating system here (this is a window on another computer).");
        }
        let Some((iface, _)) = self.selected_adapter() else { return self.fail("Select an Ethernet or WiFi row first.") };
        let warn = if self.hw.mac().carries_session(&iface) { format!(" WARNING: {iface} carries the connection you are using now (SSH or the remote screen); it will drop and you may need to reconnect.") } else { String::new() };
        let text = format!("Change the MAC address of {iface} now? The link goes down for a few seconds and reconnects with the address the setting calls for.{warn} Press y to continue.");
        self.modal = Some(Modal::Confirm(ConfirmKind::MacApply(iface), text));
    }

    /// The `y` of the question above: change the address and reconnect, in the background.
    pub(super) fn mac_apply_now(&mut self, p: &dyn NetworkSettingsPort, iface: String) {
        if !self.admin(p) {
            return;
        }
        let mac = self.hw.mac().clone();
        let (w, known) = (self.wifi.wifi.clone(), self.wifi.known.clone());
        p.audit("settings.network.mac.apply", &iface);
        self.hw.invalidate();
        self.start("changing the MAC address", move || {
            JobOut::Msg((|| {
                let wifi = iyakki_core_api::device_types::devctl::links(&mac.roots).iter().any(|l| l.name == iface && l.kind == iyakki_core_api::device_types::devctl::LinkKind::Wifi);
                let ssid = if wifi { w.current_ssid(&iface) } else { None };
                let said = mac.apply_now(&iface, ssid.as_deref(), true)?;
                if wifi {
                    let eff = ssid.or_else(|| mac.preferred_ssid());
                    let mode = mac.config().mode_for(iyakki_core_api::device_types::devctl::LinkKind::Wifi, &iface, eff.as_deref());
                    w.apply_with(&iface, &known.list(), mode, true)?;
                } else {
                    w.dhcp(&iface, true)?;
                }
                Ok(format!("{said} Reconnecting."))
            })())
        });
    }

    /// `w` on the Network page: look for networks.
    pub(super) fn scan_wifi(&mut self, p: &dyn NetworkSettingsPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the network.");
        }
        let Some(iface) = self.wifi.iface() else { return self.fail(if iyakki_core_api::device_types::devctl::broadcom_wifi_without_driver(&self.wifi.roots) { iyakki_core_api::device_types::devctl::BROADCOM_NO_DRIVER_TEXT } else { "This computer has no WiFi adapter." }) };
        let w = self.wifi.wifi.clone();
        p.audit("settings.wifi.scan", &iface);
        self.start("looking for WiFi networks", move || JobOut::Wifi(w.scan(&iface)));
    }

    pub(super) fn show_wifi_picker(&mut self, aps: Vec<iyakki_core_api::device_types::devctl::wifi::Ap>) {
        let known: Vec<String> = self.wifi.known.list().into_iter().map(|k| k.ssid).collect();
        let mut items: Vec<(String, Option<iyakki_core_api::device_types::devctl::wifi::Ap>)> = aps
            .into_iter()
            .map(|a| {
                let bars = "▂▄▆█".chars().take(a.bars()).collect::<String>();
                let tag = if a.connected { "  · connected" } else if known.contains(&a.ssid) { "  · saved" } else { "" };
                (format!("{}   {bars:<4}  {}  {}{tag}", a.ssid, a.security.label(), a.band()), Some(a))
            })
            .collect();
        items.push(("Hidden network…".into(), None));
        if !known.is_empty() {
            items.push(("Forget a saved network…".into(), None));
        }
        let titles = items.iter().map(|(t, _)| t.clone()).collect();
        self.wifi_items = items;
        self.modal = Some(Modal::Pick(PickKind::Wifi, Picker::new("Join a WiFi network", titles)));
    }

    /// Remember a network and make the machine use it, in the background.
    pub(super) fn join_wifi(&mut self, p: &dyn NetworkSettingsPort, k: iyakki_core_api::device_types::devctl::wifi::Known) {
        let Some(iface) = self.wifi.iface() else { return self.fail(if iyakki_core_api::device_types::devctl::broadcom_wifi_without_driver(&self.wifi.roots) { iyakki_core_api::device_types::devctl::BROADCOM_NO_DRIVER_TEXT } else { "This computer has no WiFi adapter." }) };
        let (w, store) = (self.wifi.wifi.clone(), self.wifi.known.clone());
        let mac = iyakki_core_api::device_types::devctl::mac::MacEnv { roots: self.wifi.roots.clone(), ..self.hw.mac().clone() };
        p.audit("settings.wifi.join", &k.ssid);
        let ssid = k.ssid.clone();
        let _ = ssid;
        self.start("joining the network", move || JobOut::Msg(iyakki_core_api::device_types::devctl::wifi::join_now(&w, &store, &mac, &iface, k)));
    }
}
