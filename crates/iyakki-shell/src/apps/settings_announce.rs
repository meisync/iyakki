//! Settings → Network → "Announce this device on the local network" (`iyakki_agent::devname`): the device answers to a random name
//! (`iyakki-<id>.local`) so a developer computer finds it again after its address changed. On by default, off by default in cloud
//! mode; the firewall rule "device name (mDNS)" exists only while it is on. Administrators only; every change is audited.

use super::*;

/// What the switch reveals, said before and after the person changes it.
pub(super) const REVEALS: &str = "It tells other computers on this network a random name (not yours), that this is an IYAKKI device, and that developer mode or the web service is on. It never tells who you are.";

impl SettingsApp {
    /// `n` on the Network page: switch the announcement on or off.
    pub(super) fn toggle_announce(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        let svc = Self::svc(cx);
        let next = !svc.announce_name_on();
        match svc.set_announce_name(Some(next)) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.announce_name", if next { "on" } else { "off" });
                let name = iyakki_agent::devname::label(svc.dir());
                self.ok(if next { format!("This device is announced as {name}.local on the local network. {REVEALS}") } else { format!("This device is no longer announced ({name}.local stops answering). Paired computers need its address.") });
            }
            Err(e) => self.fail(e),
        }
    }

    /// The short line of the Network page: the switch and the name in use.
    pub(super) fn announce_line(&self, cx: &AppCx) -> String {
        let svc = Self::svc(cx);
        if svc.announce_name_on() {
            let st = iyakki_agent::devname::read_status(svc.dir());
            format!("n: device name announcing is ON ({}). {REVEALS}", if st.on && !st.host.is_empty() { st.host } else { format!("{}.local", iyakki_agent::devname::label(svc.dir())) })
        } else {
            "n: device name announcing is OFF (Announce this device on the local network).".to_string()
        }
    }
}
