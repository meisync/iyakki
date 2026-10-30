//! Settings -> Network -> VPN (administrators): import a WireGuard or OpenVPN profile the person already has (from a file or from the
//! clipboard), list the profiles, enable or disable one, choose its mode (all traffic, or only what the filters match), set the kill
//! switch or the fail-closed option, delete one (after asking). The rules, the routes and the manager are in
//! `iyakki_agent::vpn_mgr` and the crate `iyakki-vpn`; the plan is `docs/development/VPN-CONNECTOR-PLAN.md`.
//!
//! IYAKKI provides no VPN service and no server list. Keys and passwords go straight into the vault and are never drawn: the clipboard
//! text is read by the manager and never shown, the file is read by path, a profile shows only its public side (server, public key).

use std::sync::Arc;

use iyakki_agent::vpn_mgr::{describe, ago, Manager, Row, State};
use iyakki_vpn::filter::FilterSet;
use iyakki_vpn::profile::{Mode, Settings};

use super::*;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct VpnPanel {
    /// Tests: a manager with a fake system layer instead of the real one.
    pub(super) mgr: Option<Arc<Manager>>,
    /// Tests: the clock (Unix seconds).
    pub(super) now: Option<u64>,
}

/// The forms of this page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VpnForm {
    /// Import the file named in the form.
    ImportFile,
    /// Import the text on the clipboard.
    ImportClip,
    /// Mode and filters of the profile with this id.
    Mode(String),
}

const MODES: [&str; 2] = ["FULL: everything goes through the VPN", "HYBRID: only what the filters match"];
const OFF_ON: [&str; 2] = ["Off", "On"];
const ON_OFF: [&str; 2] = ["On (matched destinations never go direct)", "Off"];
const MAX_FILE: u64 = 256 * 1024;

impl SettingsApp {
    fn vpn_now(&self) -> u64 {
        self.vpn.now.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }

    pub(super) fn vpn_mgr(&self, cx: &AppCx) -> Arc<Manager> {
        if let Some(m) = &self.vpn.mgr {
            return m.clone();
        }
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        Manager::shared(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }

    pub(super) fn vpn_len(&self, cx: &AppCx) -> usize {
        self.vpn_mgr(cx).profiles().len()
    }

    pub(super) fn vpn_buttons(&self) -> Vec<Btn> {
        vec![("Import file", ACCENT, 'i'), ("Import clipboard", ACCENT, 'p'), ("Enable / Disable", TEXT, 'x'), ("Mode and filters", TEXT, 'm'), ("Delete", ERR, 'd'), ("Retry", TEXT, 'r')]
    }

    /// The heartbeat: while the page is open the profiles are looked at every two seconds.
    pub(super) fn vpn_tick(&mut self, cx: &AppCx) {
        if self.tab() == Tab::Vpn && self.ticks % 20 == 0 {
            self.vpn_mgr(cx).tick(self.vpn_now());
        }
    }

    fn vpn_row(&self, cx: &AppCx) -> Option<Row> {
        let rows = self.vpn_mgr(cx).rows();
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        rows.into_iter().nth(sel)
    }

    pub(super) fn handle_vpn(&mut self, k: &Key, cx: &AppCx) {
        let mgr = self.vpn_mgr(cx);
        let len = mgr.profiles().len();
        let row = self.vpn_row(cx);
        let none = "Choose a VPN profile first (Up/Down).";
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('i') => {
                let f = Form::new("Import a VPN profile from a file")
                    .text("Name", "", "for example: Office")
                    .path("Profile file (.conf or .ovpn)", "", "the file your VPN provider or your own server gave you", crate::filepicker::PathKind::File, &["conf", "ovpn"]);
                self.open_form(FormKind::Vpn(VpnForm::ImportFile), f);
            }
            Key::Char('p') => {
                if !crate::ui::clipboard::can_paste() {
                    self.fail("Copy the profile text first (the whole .conf or .ovpn), then press p. Nothing is shown on screen.");
                } else {
                    self.open_form(FormKind::Vpn(VpnForm::ImportClip), Form::new("Import the VPN profile on the clipboard").text("Name", "", "for example: Office"));
                }
            }
            Key::Char('x') | Key::Enter => match row {
                Some(r) => {
                    let on = !r.enabled;
                    match mgr.set_enabled(&r.profile.id, on, self.vpn_now()) {
                        Ok(p) if on => {
                            mgr.ensure_supervised();
                            let st = mgr.status(&p.id);
                            match st.error {
                                Some(e) => self.fail(format!("{} is enabled but not connected: {e}", p.name)),
                                None => self.ok(format!("{} is enabled: it connects now and after every sign-in. The Firewall and your local network keep working.", p.name)),
                            }
                        }
                        Ok(p) => self.ok(format!("{} is disabled: it is disconnected, everything it set up is removed, and the profile is kept.", p.name)),
                        Err(e) => self.fail(e),
                    }
                }
                None => self.fail(none),
            },
            Key::Char('m') => match row {
                Some(r) => self.vpn_open_mode(&r),
                None => self.fail(none),
            },
            Key::Char('d') | Key::Delete => match row {
                Some(r) => {
                    let text = format!("Delete the VPN profile “{}”? It disconnects now, and the profile and its keys in your vault are removed. This cannot be undone.", r.profile.name);
                    self.modal = Some(Modal::Confirm(ConfirmKind::VpnDelete(r.profile.id), text));
                }
                None => self.fail(none),
            },
            Key::Char('r') => match row {
                Some(r) => {
                    mgr.retry(&r.profile.id, self.vpn_now());
                    self.ok(format!("Trying {} again.", r.profile.name));
                }
                None => self.fail(none),
            },
            _ => {}
        }
    }

    fn vpn_open_mode(&mut self, r: &Row) {
        let s = &r.profile.settings;
        let f = Form::new(&format!("Mode and filters: {}", r.profile.name))
            .choice("Mode", &MODES, if s.mode == Mode::Hybrid { 1 } else { 0 })
            .choice("FULL: kill switch (blocks the internet outside the tunnel if it drops; your local network keeps working)", &OFF_ON, usize::from(s.kill_switch))
            .choice("HYBRID: fail closed", &ON_OFF, usize::from(s.mode == Mode::Hybrid && !s.fail_closed))
            .text("HYBRID: filters, separated by spaces or commas", &s.filters.entries().join(", "), "*.corp.example.com, https://example.org/app, 203.0.113.0/24, !public.corp.example.com");
        self.open_form(FormKind::Vpn(VpnForm::Mode(r.profile.id.clone())), f);
    }

    pub(super) fn vpn_submit(&mut self, cx: &AppCx, kind: &VpnForm, form: &mut Form) -> Result<String, String> {
        let mgr = self.vpn_mgr(cx);
        match kind {
            VpnForm::ImportFile => {
                let path = form.value(1);
                if path.is_empty() {
                    return Err("Choose the profile file (Browse, or drop it here).".into());
                }
                let meta = std::fs::metadata(&path).map_err(|_| "That file cannot be read.".to_string())?;
                if !meta.is_file() || meta.len() > MAX_FILE {
                    return Err("That is not a profile file (a regular file under 256 KB).".into());
                }
                let data = std::fs::read(&path).map_err(|_| "That file cannot be read.".to_string())?;
                self.vpn_imported(&mgr, &form.value(0), &data)
            }
            VpnForm::ImportClip => {
                let text = crate::ui::clipboard::paste().ok_or("The clipboard is empty. Copy the profile text first.")?;
                let said = self.vpn_imported(&mgr, &form.value(0), text.as_bytes())?;
                // the keys must not stay in the clipboard history
                crate::ui::clipboard::forget(&text);
                Ok(format!("{said} The copied text was removed from the clipboard history; copy something else to replace the clipboard itself."))
            }
            VpnForm::Mode(id) => {
                let hybrid = form.choice_idx(0) == 1;
                let filters = FilterSet::parse(&form.value(3).replace(',', "\n").split_whitespace().collect::<Vec<_>>().join("\n")).map_err(|e| e.to_string())?;
                let s = if hybrid {
                    Settings { mode: Mode::Hybrid, kill_switch: false, fail_closed: form.choice_idx(2) == 0, filters, public_ip_lookup: false }
                } else {
                    if !filters.is_empty() {
                        return Err("Filters belong to HYBRID mode. FULL sends everything through the VPN: choose HYBRID, or clear the filters.".into());
                    }
                    Settings { mode: Mode::Full, kill_switch: form.choice_idx(1) == 1, fail_closed: false, filters, public_ip_lookup: false }
                };
                let p = mgr.set_settings(id, s, self.vpn_now())?;
                Ok(format!("{}: {}. {}", p.name, describe(&p), if mgr.is_enabled(id) { "It was reconnected with the new settings." } else { "It takes effect when you enable it." }))
            }
        }
    }

    fn vpn_imported(&mut self, mgr: &Manager, name: &str, data: &[u8]) -> Result<String, String> {
        let imp = mgr.import(name, data)?;
        let mut said = format!("Imported “{}”. Its keys are in your vault. It is disabled: press x to connect.", imp.profile.name);
        for w in &imp.warnings {
            said.push_str(&format!(" Note: {w}."));
        }
        Ok(said)
    }

    pub(super) fn vpn_delete(&mut self, cx: &AppCx, id: &str) {
        match self.vpn_mgr(cx).remove(id) {
            Ok(done) => {
                self.ok(format!("Deleted {}.", done.join(", ")));
                let len = self.vpn_len(cx);
                self.sel[self.tab] = self.sel[self.tab].min(len.saturating_sub(1));
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    pub(super) fn render_vpn(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mgr = self.vpn_mgr(cx);
        let rows = mgr.rows();
        let now = self.vpn_now();
        let mut y = a.y;
        super::sshtun_page::intro(g, a, s, &mut y, &["IYAKKI provides no VPN service. Import the profile file your VPN provider or your own server gave you (WireGuard now; OpenVPN profiles are kept, connecting needs the program you download yourself). Keys and passwords stay in this computer's vault; nothing is sent anywhere.", "FULL sends all traffic through the VPN. HYBRID sends only what your filters match (by name or address) and everything else goes direct. The kill switch is off unless you switch it on."], "Import a VPN profile you already have. FULL sends everything through it, HYBRID only what your filters match.", if rows.is_empty() { 40.0 * s } else { 128.0 * s + 60.0 * s + 16.0 * s });
        if rows.is_empty() {
            y += 6.0 * s;
            for w in g.wrap(14.0 * s, "No profiles yet. Press i to import one.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, DIM, &w, Face::Sans);
                y += 20.0 * s;
            }
            return;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        let detail_h = 128.0 * s;
        let rh = 60.0 * s;
        let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
        let heights = vec![rh; rows.len()];
        let first = self.win[self.tab].layout(sel, &heights, room);
        let fit = self.win[self.tab].fit();
        for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
            let p = &r.profile;
            let (state, col) = vpn_state(r, now);
            let mode = match p.settings.mode {
                Mode::Full => "FULL",
                Mode::Hybrid => "HYBRID",
            };
            let mut chips: Vec<(&str, [u8; 4])> = vec![(state.as_str(), col), (p.kind().as_str(), MUTED), (mode, MUTED)];
            if r.status.guard {
                chips.push(("guard on", crate::fx::AMBER));
            }
            let sub = format!("{} · {}", p.endpoint_label().unwrap_or_else(|| "no server".into()), if p.full_tunnel() { "profile routes all traffic" } else { "profile routes some networks" });
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &p.name, &sub, &chips);
            y += rh;
        }
        if rows.len() > fit {
            g.text(a.x + a.w - 120.0 * s, a.y + a.h - detail_h - 6.0 * s, 11.0 * s, DIM, &format!("{}-{} of {}", first + 1, (first + fit).min(rows.len()), rows.len()), Face::Sans);
        }
        // the chosen profile: status, why it is not up, what it does; never a key
        let r = &rows[sel];
        let top = a.y + a.h - detail_h;
        g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
        let mut lines: Vec<(String, [u8; 4])> = vec![(format!("Status: {}.", r.status.line(r.enabled, now)), TEXT)];
        if let Some(e) = &r.status.error {
            lines.push((e.clone(), if r.status.needs_you { ERR } else { crate::fx::AMBER }));
        }
        if let State::Connected { .. } = r.status.state {
            lines.push((format!("Received {} · sent {}{}.", bytes(r.status.rx), bytes(r.status.tx), r.status.last_handshake.map(|h| format!(" · last handshake {} ago", ago(now.saturating_sub(h.max(0) as u64)))).unwrap_or_default()), MUTED));
        }
        lines.push((describe(&r.profile), MUTED));
        match r.profile.settings.mode {
            Mode::Hybrid => {
                let fs = &r.profile.settings.filters;
                let ignored = fs.dns_ignored_url_rules();
                lines.push((
                    format!(
                        "{} filter rule{}{}. {}",
                        fs.len(),
                        if fs.len() == 1 { "" } else { "s" },
                        if ignored > 0 { format!(" ({ignored} match whole addresses and only apply where IYAKKI sees the address, such as the Browser)") } else { String::new() },
                        if r.profile.settings.fail_closed { "Fail closed: matched destinations are never sent direct, even if the tunnel is down." } else { "Fail closed is off: if the tunnel is down matched destinations go direct." }
                    ),
                    MUTED,
                ));
            }
            Mode::Full => lines.push((if r.profile.settings.kill_switch { "Kill switch on: if the tunnel drops, the internet outside it is blocked until it is back or you disable the profile. Your local network keeps working.".to_string() } else { "Kill switch off: if the tunnel drops, traffic goes out directly.".to_string() }, MUTED)),
        }
        for w in &r.status.warnings {
            lines.push((format!("Note: {w}."), crate::fx::AMBER));
        }
        let mut ty = top + 4.0 * s;
        'draw: for (l, col) in lines {
            for w in g.wrap(12.0 * s, &l, Face::Sans, a.w) {
                if ty + 16.0 * s > a.y + a.h {
                    break 'draw;
                }
                g.text(a.x, ty, 12.0 * s, col, &w, Face::Sans);
                ty += 16.0 * s;
            }
        }
    }
}

fn bytes(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{} KB", n / 1024),
        _ => format!("{} MB", n / 1_048_576),
    }
}

/// The chip of a row: the state in a word and its colour.
fn vpn_state(r: &Row, now: u64) -> (String, [u8; 4]) {
    let amber = crate::fx::AMBER;
    match (r.status.state, r.enabled) {
        (State::Connected { since }, _) => (format!("● connected {}", ago(now.saturating_sub(since))), OK),
        (State::Connecting, _) => ("● connecting".into(), amber),
        (State::Stopped, false) => ("○ disabled".into(), DIM),
        (State::Stopped, true) if r.status.needs_you => ("● needs you".into(), ERR),
        (State::Stopped, true) if r.status.error.is_some() => ("● retrying".into(), amber),
        (State::Stopped, true) => ("○ starting".into(), DIM),
    }
}
