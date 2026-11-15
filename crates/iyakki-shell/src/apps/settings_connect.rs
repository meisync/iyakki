//! Settings -> Remote access -> "Connect a client" (EXPERIMENTAL, off by default; docs/development/CLIENT-SPLIT-DESIGN.md section 5).
//!
//! Administrators switch the session endpoint for clients on or off (`k`), make a one-time link (`l`, shown as text and a QR code, good for
//! a few minutes, used once), see the enrolled devices with when each was last seen, and revoke one (`x`, then its number). The endpoint
//! itself is in `iyakki-remote` (`clientsession.rs`); the switch, the links and the device list are `iyakki_agent::clientlink`, shared
//! with it, so a revoke here closes an open session within a second. Every change is audited (`settings.client_endpoint`,
//! `settings.client_link`, `settings.client_revoke`); the audit never holds the link or its token.

use super::*;
use iyakki_agent::clientlink as cl;
use iyakki_agent::clientmedia::{device_holds, permission_capability, set_permission, DEVICE_PERMISSIONS};

/// The link on show: the text, its QR code and when it stops working.
pub(super) struct Shown {
    url: String,
    qr: Option<iyakki_agent::qr::Qr>,
    expires: i64,
}

#[derive(Default)]
pub(super) struct ConnectPanel {
    shown: Option<Shown>,
    /// Whether the switch was on when the page was last drawn (for the buttons).
    on: bool,
    /// `x` was pressed: the next digit revokes that device.
    revoke_armed: bool,
    /// `d` was pressed: the next digit opens the permissions of that device.
    perm_armed: bool,
    /// The device whose permissions are on show: id and name.
    perm: Option<(String, String)>,
    /// What the permission buttons say (kept from the last draw).
    perm_buttons: Vec<(String, bool)>,
}

type Line = (String, f32, [u8; 4], Face);

fn now() -> i64 {
    iyakki_agent::clock::now_unix()
}

/// "never", "just now", "5 min ago", "3 h ago", "2 days ago".
pub(super) fn ago(now: i64, at: i64) -> String {
    if at <= 0 {
        return "never".into();
    }
    let d = (now - at).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

impl SettingsApp {
    pub(super) fn connect_buttons(&self) -> Vec<Btn> {
        if self.remote.connect.on {
            let mut v: Vec<Btn> = vec![("Stop client connections", ERR, 'k'), ("New one-time link", ACCENT, 'l')];
            if self.remote.connect.perm.is_some() {
                // green: on. The lines above say on or off in words.
                const LABELS: [&str; 5] = ["1 Sound", "2 Send clipboard", "3 Machine cameras", "4 Share its camera", "5 Share its microphone"];
                for (i, (_, on)) in self.remote.connect.perm_buttons.iter().enumerate().take(5) {
                    v.push((LABELS[i], if *on { OK } else { TEXT }, (b'1' + i as u8) as char));
                }
                v.push(("Back to the devices", TEXT, 'd'));
            } else {
                v.push(("Device permissions", TEXT, 'd'));
            }
            v
        } else {
            vec![("Connect a client (experimental)", TEXT, 'k')]
        }
    }

    /// Keys of the section; true when the key was one of them.
    pub(super) fn connect_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        let armed = std::mem::take(&mut self.remote.connect.revoke_armed);
        let perm_armed = std::mem::take(&mut self.remote.connect.perm_armed);
        // the permissions of one device are on show: 1 to 5 switch them, p goes back
        if self.remote.connect.perm.is_some() && cl::enabled(Self::svc(cx).dir()) {
            match k {
                Key::Char(c @ '1'..='5') => {
                    if self.admin(cx) {
                        self.connect_toggle(cx, *c as usize - '1' as usize);
                    }
                    return true;
                }
                Key::Char('d') => {
                    self.remote.connect.perm = None;
                    return true;
                }
                _ => {}
            }
        }
        match k {
            Key::Char('d') => {
                if self.admin(cx) {
                    self.remote.connect.perm_armed = true;
                    self.ok("Permissions of which device? Press its number (1 to 9) in the list below, or any other key to cancel.");
                }
            }
            Key::Char(c @ '1'..='9') if perm_armed => {
                if self.admin(cx) {
                    self.connect_open_perm(cx, *c as usize - '1' as usize);
                }
            }
            Key::Char('k') => {
                if self.admin(cx) {
                    let on = !cl::enabled(Self::svc(cx).dir());
                    self.connect_set(cx, on);
                }
            }
            Key::Char('l') => {
                if self.admin(cx) {
                    self.connect_link(cx);
                }
            }
            Key::Char('x') => {
                if self.admin(cx) {
                    self.remote.connect.revoke_armed = true;
                    self.ok("Revoke which device? Press its number (1 to 9) in the list below, or any other key to cancel.");
                }
            }
            Key::Char(c @ '1'..='9') if armed => {
                if self.admin(cx) {
                    self.connect_revoke(cx, *c as usize - '1' as usize);
                }
            }
            _ => return false,
        }
        true
    }

    fn connect_set(&mut self, cx: &AppCx, on: bool) {
        let svc = Self::svc(cx);
        match cl::set_enabled(svc.dir(), on) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.client_endpoint", if on { "on" } else { "off" });
                if on {
                    self.ok("Client connections (experimental) are on. Make a one-time link with l. Web access above must be on too, because clients come in through it.");
                } else {
                    self.remote.connect.shown = None;
                    self.ok("Client connections are off: the endpoint answers nothing and open client sessions end within a second.");
                }
            }
            Err(e) => self.fail(e),
        }
    }

    fn connect_link(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx);
        if !cl::enabled(svc.dir()) {
            return self.fail("Switch client connections on first (k).");
        }
        let at = now();
        match cl::create_token(svc.dir(), &cx.session.user, at, cl::TOKEN_TTL_SECS) {
            Ok(token) => {
                let base = wa::urls(&iyakki_agent::devname::label(svc.dir()), &wa::local_ips(), svc.web_access_port()).into_iter().next().unwrap_or_default();
                // the machine's own certificate is in the link, so the window program pins it without a question (a browser still shows its warning)
                let url = cl::link_url_pinned(&base, &token, iyakki_agent::devmode::cert_fingerprint(svc.dir()).as_deref());
                // the audit says that a link was made, never what it is
                cx.audit.record(&cx.session.user, "settings.client_link", &format!("one-time, {} min", cl::TOKEN_TTL_SECS / 60));
                self.remote.connect.shown = Some(Shown { qr: iyakki_agent::qr::encode(url.as_bytes()), url, expires: at + cl::TOKEN_TTL_SECS });
                self.ok("A one-time link is on the page. It works once and stops after a few minutes.");
            }
            Err(e) => self.fail(e),
        }
    }

    fn connect_open_perm(&mut self, cx: &AppCx, index: usize) {
        match cl::list_devices(Self::svc(cx).dir()).into_iter().filter(|d| d.active()).nth(index) {
            Some(d) => self.remote.connect.perm = Some((d.id, d.name)),
            None => self.fail("There is no device with that number."),
        }
    }

    /// Switch permission `i` (0 to 4 in [`DEVICE_PERMISSIONS`]) of the device on show: audited, effective at the device's next request.
    fn connect_toggle(&mut self, cx: &AppCx, i: usize) {
        let Some((id, name)) = self.remote.connect.perm.clone() else { return };
        let Some((perm, _, what)) = DEVICE_PERMISSIONS.get(i).copied() else { return };
        let svc = Self::svc(cx);
        let cap = permission_capability(perm).unwrap_or(perm);
        let on = !device_holds(svc.dir(), &id).iter().any(|c| c == cap);
        match set_permission(svc.dir(), &id, perm, on, &cx.session.user, now()) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "settings.client_permission", &format!("{perm} {} {name} {id}", if on { "on" } else { "off" }));
                if on {
                    self.ok(format!("{name} may now ask to {what}. Where it says asked every use, a card still comes up on the machine each time."));
                } else {
                    self.ok(format!("{name} may no longer {what}. It is refused from its next request."));
                }
            }
            Err(e) => self.fail(e),
        }
    }

    fn connect_revoke(&mut self, cx: &AppCx, index: usize) {
        let svc = Self::svc(cx);
        let Some(d) = cl::list_devices(svc.dir()).into_iter().filter(|d| d.active()).nth(index) else {
            return self.fail("There is no device with that number.");
        };
        match cl::revoke_device(svc.dir(), &d.id, now()) {
            Ok(true) => {
                cx.audit.record(&cx.session.user, "settings.client_revoke", &format!("{} {}", d.name, d.id));
                self.ok(format!("{} is revoked: it cannot connect again and its open session ends within a second.", d.name));
            }
            Ok(false) => self.fail("That device is revoked already."),
            Err(e) => self.fail(e),
        }
    }

    /// The lines of the section and where the QR code goes (the index of the line it sits above).
    pub(super) fn connect_lines(&mut self, cx: &AppCx, lines: &mut Vec<Line>) -> Option<usize> {
        let svc = Self::svc(cx);
        let on = cl::enabled(svc.dir());
        self.remote.connect.on = on;
        let at = now();
        if self.remote.connect.shown.as_ref().is_some_and(|s| s.expires <= at) {
            self.remote.connect.shown = None;
        }
        lines.push(("Connect a client (EXPERIMENTAL)".into(), 14.0, TEXT, Face::Bold));
        lines.push(("A browser page, a window program or a phone can show this desktop and send input. Off by default. A client is enrolled once with a one-time link, then proves itself with a key; the person still signs in with their password, and a lost device is revoked here.".into(), 13.0, MUTED, Face::Sans));
        lines.push((if on { "Client connections: on (experimental)".to_string() } else { "Client connections: off, the endpoint answers nothing".to_string() }, 13.0, if on { rgb(255, 196, 94) } else { OK }, Face::Bold));
        let mut qr_at = None;
        if on {
            if let Some(s) = &self.remote.connect.shown {
                lines.push((format!("One-time link, works once, {} min left (scan or type it on the new device):", ((s.expires - at) / 60 + 1).max(1)), 13.0, TEXT, Face::Sans));
                if s.qr.is_some() {
                    qr_at = Some(lines.len());
                }
                lines.push((s.url.clone(), 12.0, ACCENT, Face::Mono));
                lines.push(("Anyone who opens it in the next minutes can enrol a device, so show it only to the device you mean. The part after # never leaves the device that opens it.".into(), 12.0, DIM, Face::Sans));
            }
            let devices = cl::list_devices(svc.dir());
            if devices.is_empty() {
                lines.push(("No client is enrolled yet.".into(), 13.0, MUTED, Face::Sans));
            }
            let mut n = 0;
            for d in &devices {
                if d.active() {
                    n += 1;
                    lines.push((format!("{n}. {}   enrolled by {}, last seen {}", d.name, d.by, ago(at, d.last_seen)), 13.0, TEXT, Face::Sans));
                } else {
                    lines.push((format!("   {}   revoked", d.name), 12.0, DIM, Face::Sans));
                }
            }
            // the permissions of one device (EXPERIMENTAL media half): on means allowed to ask
            self.remote.connect.perm_buttons.clear();
            if let Some((id, name)) = self.remote.connect.perm.clone() {
                if devices.iter().any(|d| d.id == id && d.active()) {
                    let held = device_holds(svc.dir(), &id);
                    lines.push((format!("Permissions of {name} (press 1 to 5 to switch one, d to go back):"), 13.0, TEXT, Face::Bold));
                    for (i, (perm, cap, what)) in DEVICE_PERMISSIONS.iter().enumerate() {
                        let on = held.iter().any(|c| c == cap);
                        self.remote.connect.perm_buttons.push((perm.to_string(), on));
                        lines.push((format!("{}. {perm}: {}   may {what}", i + 1, if on { "on" } else { "off" }), 13.0, if on { rgb(255, 196, 94) } else { MUTED }, Face::Sans));
                    }
                    lines.push(("On means allowed to ask. The clipboard and the device's camera and microphone still ask on this machine at every use; sound and cameras of the machine do not ask again once on. A change counts from the device's next request.".into(), 12.0, DIM, Face::Sans));
                } else {
                    self.remote.connect.perm = None;
                }
            }
            lines.push(("k switches off, l makes a new one-time link, x then a number revokes a device, d then a number sets what a device may ask for.".into(), 12.0, DIM, Face::Sans));
        } else {
            lines.push(("k switches it on. Nothing in this section is reachable while it is off.".into(), 12.0, DIM, Face::Sans));
        }
        qr_at
    }

    /// Draw the QR code of the link above the line `y` is at; returns the height used.
    pub(super) fn connect_draw_qr(&self, g: &mut Gfx, x: f32, y: f32, s: f32) -> f32 {
        let Some(q) = self.remote.connect.shown.as_ref().and_then(|s| s.qr.as_ref()) else { return 0.0 };
        // black modules on a white square with the 4-module quiet zone, whole pixels so every edge stays sharp
        let m = (3.0 * s).round().max(2.0);
        let side = (q.size + 8) as f32 * m;
        let (x0, y0) = (x.round(), (y + 4.0 * s).round());
        g.rect(x0, y0, side, side, [255, 255, 255, 255]);
        for qy in 0..q.size {
            let mut qx = 0;
            while qx < q.size {
                if q.get(qx, qy) {
                    let start = qx;
                    while qx < q.size && q.get(qx, qy) {
                        qx += 1;
                    }
                    g.rect(x0 + (start + 4) as f32 * m, y0 + (qy + 4) as f32 * m, (qx - start) as f32 * m, m, [0, 0, 0, 255]);
                } else {
                    qx += 1;
                }
            }
        }
        side + 8.0 * s
    }
}

#[cfg(test)]
#[path = "settings_connect_tests.rs"]
mod tests;
