//! Settings -> Remote access (administrators): the button "Serve this desktop to browsers". While it is on, this desktop can be opened
//! from a browser on another computer (HTTPS only, signing in with this machine's accounts). The page shows the addresses to open, the
//! certificate fingerprint to compare on the first visit, what it exposes, and refuses to switch on while no administrator account
//! with a password exists. The listener and the firewall rule are in `iyakki_remote::webswitch`; the setting is
//! `iyakki_shared::webaccess`. Off by default; every change is audited (`settings.web_access`).

use super::*;
use crate::apps::settings::network_port::NetworkSettingsPort;
use iyakki_shared::webaccess as wa;

// the SFTP server's switches, keys and card (F-FILE-06)
#[path = "settings_sftp.rs"]
pub(crate) mod sftp_page;

// the machines a flow may run on (F-FLOW-16)
#[path = "settings_machines.rs"]
pub(crate) mod machines_page;

// "Connect a client" (experimental): its own file, hooked in below at the buttons, the keys and the page
#[path = "settings_connect.rs"]
mod connect_page;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct RemotePanel {
    /// Whether the switch was on when the page was last drawn (for the button).
    on: bool,
    pub(super) scroll: f32,
    pub(super) max: f32,
    connect: connect_page::ConnectPanel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RemoteForm {
    Port,
}

impl SettingsApp {
    pub(super) fn remote_buttons(&self) -> Vec<Btn> {
        let mut v = if self.remote.on {
            vec![("Stop serving this desktop", ERR, 'e'), ("Change port", TEXT, 'p')]
        } else {
            vec![("Serve this desktop to browsers", ACCENT, 'e'), ("Change port", TEXT, 'p')]
        };
        v.extend(self.connect_buttons());
        v.extend(self.sftp_buttons());
        v.extend(self.machine_buttons());
        v
    }


    /// Is there an administrator who can sign in with a password?
    fn remote_has_admin(p: &dyn NetworkSettingsPort) -> bool {
        p.has_admin_password()
    }

    pub(super) fn remote_set(&mut self, p: &dyn NetworkSettingsPort, on: bool) {
        let port = p.web_access_port();
        if on && !Self::remote_has_admin(p) {
            self.fail("Not switched on: this machine has no administrator account with a password, so nobody could sign in safely. Make one in Settings, Account first.");
            return;
        }
        match p.set_web_access(on) {
            Ok(()) => {
                p.audit("settings.web_access", &format!("{} (port {port}, HTTPS)", if on { "on" } else { "off" }));
                if on {
                    let first = wa::urls(&iyakki_shared::devname::label(p.data_root()), &Self::remote_ips(), port).into_iter().next().unwrap_or_default();
                    self.ok(format!("Serving this desktop to browsers: open {first}. {} Press e again to stop.", wa::EXPOSES));
                } else {
                    self.ok("No longer serving this desktop to browsers: the port is closed within a few seconds and open sessions end.");
                }
            }
            Err(e) => self.fail(e),
        }
    }

    fn remote_ips() -> Vec<String> {
        wa::local_ips()
    }

    pub(super) fn handle_remote(&mut self, k: &Key, p: &dyn NetworkSettingsPort) {
        if self.connect_key(k, p) || self.sftp_key(k, p) || self.machine_key(k, p) {
            return;
        }
        match k {
            Key::Up => self.remote.scroll = (self.remote.scroll - 40.0).max(0.0),
            Key::Down => self.remote.scroll = (self.remote.scroll + 40.0).min(self.remote.max),
            Key::PageUp => self.remote.scroll = (self.remote.scroll - 300.0).max(0.0),
            Key::PageDown => self.remote.scroll = (self.remote.scroll + 300.0).min(self.remote.max),
            Key::Home => self.remote.scroll = 0.0,
            Key::End => self.remote.scroll = self.remote.max,
            Key::Char('e') | Key::Enter => {
                if self.admin(p) {
                    let on = !p.web_access_on();
                    self.remote_set(p, on);
                }
            }
            Key::Char('p') => {
                if self.admin(p) {
                    let cur = p.web_access_port();
                    self.open_form(FormKind::Remote(RemoteForm::Port), Form::new("Port for browsers (HTTPS)").text("Port", &cur.to_string(), "8443 (the default); 1024 to 65535"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn remote_scroll_by(&mut self, px: f32) {
        self.remote.scroll = (self.remote.scroll + px).clamp_safe(0.0, self.remote.max);
    }

    pub(super) fn remote_submit(&mut self, p: &dyn NetworkSettingsPort, _k: &RemoteForm, form: &mut Form) -> Result<String, String> {
        let port = wa::parse_port(&form.value(0))?;
        p.set_web_access_port(port)?;
        p.audit("settings.web_access.port", &wa::effective_port(port).to_string());
        Ok(format!("The port for browsers is {}.{}", wa::effective_port(port), if p.web_access_on() { " It follows within a few seconds (the listener is opened again)." } else { " It is used when you switch the service on." }))
    }

    pub(super) fn render_remote(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn NetworkSettingsPort) {
        let on = p.web_access_on();
        self.remote.on = on;
        let port = p.web_access_port();
        let st = wa::read_status(p.data_root());
        let listening = st.listening;
        let has_admin = Self::remote_has_admin(p);
        let (word, col) = if listening {
            (st.line(on), rgb(255, 196, 94))
        } else if !st.error.is_empty() {
            (st.line(on), ERR)
        } else if on {
            (st.line(on), rgb(255, 196, 94))
        } else {
            ("Off: nothing listens, no browser can reach this desktop".to_string(), OK)
        };
        let mut lines: Vec<(String, f32, [u8; 4], Face)> = Vec::new();
        lines.push(("Use this desktop from a browser".into(), 15.0, TEXT, Face::Bold));
        lines.push(("Switch this on to open this desktop in a browser on another computer or tablet: it shows the same screen and takes keys, mouse and touch. Off by default; always encrypted (HTTPS), never plain HTTP.".into(), 13.0, MUTED, Face::Sans));
        lines.push((format!("Remote access: {word}"), 16.0, col, Face::Bold));
        lines.push((format!("Port {port} (HTTPS only){}", if on && !listening && st.error.is_empty() { ", opening" } else { "" }), 13.0, MUTED, Face::Sans));
        if !has_admin {
            lines.push(("There is no administrator account with a password on this machine, so it cannot be switched on. Make one in Settings, Account.".into(), 13.0, ERR, Face::Sans));
        }
        if on || listening {
            lines.push(("Open one of these in the browser of the other computer".into(), 14.0, TEXT, Face::Bold));
            let shown = if st.port != 0 { st.port } else { port };
            for u in wa::urls(&iyakki_shared::devname::label(p.data_root()), &Self::remote_ips(), shown) {
                lines.push((u, 13.0, ACCENT, Face::Mono));
            }
            lines.push(("The name ends in .local and works on most computers and phones on the same network; if yours cannot find it, use an address.".into(), 12.0, DIM, Face::Sans));
            lines.push(("Certificate fingerprint (SHA-256). The browser warns once, because the certificate is made by this machine; compare this with what it shows before you continue.".into(), 13.0, MUTED, Face::Sans));
            match p.agent().devmode().cert_fingerprint(p.data_root()) {
                Some(fp) => {
                    let parts: Vec<&str> = fp.split(':').collect();
                    for chunk in parts.chunks(8) {
                        lines.push((chunk.join(":"), 13.0, TEXT, Face::Mono));
                    }
                }
                None => lines.push(("(the certificate is being made: look again in a moment)".into(), 13.0, DIM, Face::Sans)),
            }
        }
        self.sftp_lines(p, &mut lines);
        self.machine_lines(p, &mut lines);
        let qr_at = self.connect_lines(p, &mut lines);
        lines.push(("What this exposes".into(), 14.0, TEXT, Face::Bold));
        lines.push((wa::EXPOSES.into(), 13.0, rgb(255, 196, 94), Face::Sans));
        lines.push(("Turn it off when you are done. The firewall port is opened for the local networks only while it is on, and closed when it is off.".into(), 12.0, DIM, Face::Sans));
        if !self.admin_now(p) {
            lines.push(("Only administrators can change this.".into(), 13.0, MUTED, Face::Sans));
        }
        let mut y = a.y - self.remote.scroll;
        let mut used = 0.0f32;
        g.clipped(a, |g| {
            for (li, (t, size, c, face)) in lines.iter().enumerate() {
                let sz = size * s;
                if *size >= 14.0 && face == &Face::Bold {
                    y += 6.0 * s;
                    used += 6.0 * s;
                }
                if qr_at == Some(li) {
                    let h = self.connect_draw_qr(g, a.x, y, s);
                    y += h;
                    used += h;
                }
                for w in g.wrap(sz, t, *face, a.w) {
                    g.text(a.x, y, sz, *c, &w, *face);
                    y += sz * 1.45;
                    used += sz * 1.45;
                }
            }
        });
        self.remote.max = (used + 12.0 * s - a.h).max(0.0);
        self.remote.scroll = self.remote.scroll.clamp_safe(0.0, self.remote.max);
    }

    fn admin_now(&self, p: &dyn NetworkSettingsPort) -> bool {
        p.is_admin()
    }
}
