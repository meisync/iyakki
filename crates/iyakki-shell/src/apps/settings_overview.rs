//! Settings -> Overview: the page Settings opens on. It lists what needs attention (no AI model chosen, firewall open, clock
//! wrong, Bluetooth firmware missing, data kept only in RAM), each with the button that opens the page that fixes it, then
//! the things people change most as one-tap buttons. Only pages this account may open are offered.

use iyakki_fw::engine::Status;

use super::*;
use crate::fx::AMBER;

/// One thing that needs attention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Attn {
    /// The page that fixes it.
    pub tab: Tab,
    pub title: String,
    pub why: String,
}

/// Things people look for most: (what they want to do, the page).
pub(super) const COMMON: [(&str, Tab); 9] = [
    ("Join WiFi", Tab::Network),
    ("Change my password", Tab::Account),
    ("Bigger text", Tab::Display),
    ("Choose the AI model", Tab::Models),
    ("Sound and volume", Tab::Sound),
    ("Time zone and clock", Tab::Time),
    ("Firewall", Tab::Firewall),
    ("Name of this computer", Tab::HostName),
    ("Where my data is kept", Tab::Storage),
];

impl SettingsApp {
    /// What needs attention right now, most important first. Only pages this account has are mentioned.
    pub(super) fn attention(&self, cx: &AppCx) -> Vec<Attn> {
        let has = |t: Tab| self.tabs.contains(&t);
        let mut out = Vec::new();
        if has(Tab::Firewall) && self.fw.writable {
            match self.fw.handle.lock().status() {
                Status::Off | Status::OffThisBoot => out.push(Attn { tab: Tab::Firewall, title: "The firewall is open".into(), why: "Nothing coming in from the network is blocked. Choose Standard.".into() }),
                Status::NotProtected(_) | Status::Unavailable(_) => out.push(Attn { tab: Tab::Firewall, title: "The firewall is not protecting this computer".into(), why: "The rules could not be applied. Open the page to see why.".into() }),
                Status::On | Status::Pending { .. } => {}
            }
        }
        if has(Tab::Models) && Self::svc(cx).settings.lock().unwrap().get().chat.is_none() {
            out.push(Attn { tab: Tab::Models, title: "No AI model is chosen".into(), why: "The assistant needs one to answer. Add a service or pick a model that runs here.".into() });
        }
        if has(Tab::Time) && iyakki_agent::clock::looks_wrong(iyakki_agent::clock::now_unix()) {
            out.push(Attn { tab: Tab::Time, title: "The clock looks wrong".into(), why: "This computer has not set its time. Ask the time server now.".into() });
        }
        if has(Tab::Bluetooth) && self.hw.bt_needs_firmware() {
            out.push(Attn { tab: Tab::Bluetooth, title: "Bluetooth needs firmware".into(), why: "The adapter is found but cannot work until its firmware is installed.".into() });
        }
        if has(Tab::SessionLog) {
            if let Some(u) = self.crash_note() {
                out.push(Attn { tab: Tab::SessionLog, title: format!("The last session ended unexpectedly ({})", iyakki_storage::crashlog::wall(u.started).split(' ').next().unwrap_or("")), why: "Open the log to see the last kernel messages before it stopped.".into() });
            }
        }
        if has(Tab::Updates) {
            let data = &self.storage.data_dir;
            if let Some(t) = iyakki_agent::devmode::update::interrupted_text(data) {
                out.push(Attn { tab: Tab::Updates, title: "A system update did not finish".into(), why: t });
            } else if iyakki_agent::devmode::centre::read_trial(data).is_some_and(|t| t.state == "fell-back") {
                out.push(Attn { tab: Tab::Updates, title: "The last update did not start, the saved version runs".into(), why: "Open System update to see what happened and what to do.".into() });
            }
        }
        if has(Tab::Storage) && self.storage.virtual_disks.is_none() && matches!(iyakki_storage::data_location(&self.storage.roots, &self.storage.data_dir), iyakki_storage::DataLocation::Memory) {
            out.push(Attn { tab: Tab::Storage, title: "Your data is kept in memory only".into(), why: "Accounts, settings and files are forgotten at power-off. Choose a disk to keep them.".into() });
        }
        out
    }

    pub(super) fn handle_overview(&mut self, k: &Key, cx: &AppCx) {
        let list = self.attention(cx);
        match k {
            Key::Up => self.move_sel(-1, list.len()),
            Key::Down => self.move_sel(1, list.len()),
            Key::Char('s') if cx.session.policy.allows_app(iyakki_sec::policy::App::Doctor) => self.hw.ask_open_item("doctor", "share"),
            Key::Enter => {
                if let Some(a) = list.get(self.sel[self.tab].min(list.len().saturating_sub(1))) {
                    self.go_to(a.tab);
                }
            }
            _ => {}
        }
    }

    /// Open a page by its tab, if this account has it.
    pub(super) fn go_to(&mut self, t: Tab) {
        if let Some(i) = self.tabs.iter().position(|x| *x == t) {
            self.tab = i;
            self.nav_after_edit();
        }
    }

    pub(super) fn render_overview(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let list = self.attention(cx);
        let bottom = a.y + a.h;
        let mut y = a.y;
        if list.is_empty() {
            let t = g.fit(18.0 * s, "Everything looks fine.", Face::Bold, a.w);
            g.text(a.x, y, 18.0 * s, OK, &t, Face::Bold);
            y += 28.0 * s;
            for l in g.wrap(14.0 * s, "Nothing needs your attention. Use the list on the left, or the search box, to find a setting.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, MUTED, &l, Face::Sans);
                y += 20.0 * s;
            }
        } else {
            let t = g.fit(18.0 * s, &format!("Needs attention ({})", list.len()), Face::Bold, a.w);
            g.text(a.x, y, 18.0 * s, AMBER, &t, Face::Bold);
            y += 30.0 * s;
            let rh = 60.0 * s;
            let sel = self.sel[self.tab].min(list.len() - 1);
            let room = (((bottom - y - 150.0 * s) / rh) as usize).max(1);
            let first = if sel < room { 0 } else { sel + 1 - room }.min(list.len().saturating_sub(room));
            for (i, at) in list.iter().enumerate().skip(first).take(room) {
                if y + rh > bottom + 0.5 {
                    break;
                }
                let bw = g.text_width(14.0 * s, "Fix it", Face::Bold) + 14.0 * s * 1.7;
                let why = g.fit(13.0 * s, &at.why, Face::Sans, a.w - 36.0 * s - bw - 24.0 * s);
                row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &at.title, &why, &[]);
                if let Some(ti) = self.tabs.iter().position(|x| *x == at.tab) {
                    let bh = 14.0 * s * 2.1;
                    g.button(a.x + a.w - bw - 14.0 * s, y + (rh - 6.0 * s - bh) / 2.0, 14.0 * s, "Fix it", ACCENT, Event::Tab(ti));
                }
                y += rh;
            }
        }
        // the usual places
        y += 12.0 * s;
        if y + 70.0 * s > bottom {
            return;
        }
        g.text(a.x, y, 15.0 * s, TEXT, "Common things", Face::Bold);
        y += 28.0 * s;
        let (mut bx, mut by) = (a.x, y);
        for (label, tab) in COMMON {
            let Some(ti) = self.tabs.iter().position(|x| *x == tab) else { continue };
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
            if bx > a.x && bx + bw > a.x + a.w {
                bx = a.x;
                by += 40.0 * s;
            }
            if by + 14.0 * s * 2.1 > bottom {
                break;
            }
            bx += g.button(bx, by, 14.0 * s, label, TEXT, Event::Tab(ti)) + 8.0 * s;
        }
        // a support bundle, previewed in DOCTOR before anything is saved
        if cx.session.policy.allows_app(iyakki_sec::policy::App::Doctor) {
            let label = "Share diagnostics";
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
            if bx > a.x && bx + bw > a.x + a.w {
                bx = a.x;
                by += 40.0 * s;
            }
            if by + 14.0 * s * 2.1 <= bottom {
                g.button(bx, by, 14.0 * s, label, ACCENT, Event::Key(Key::Char('s')));
            }
        }
    }
}
