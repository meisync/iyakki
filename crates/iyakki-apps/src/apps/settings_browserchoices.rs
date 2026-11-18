//! Settings -> Browser -> "What is kept of protected browser data": the choices of `iyakki_agent::profilepolicy` as keys and buttons, the
//! same as the chat tools `browser_protection_choices` and `browser_protection_choices_set` (F-VAULT-07 to F-VAULT-09, KI-741).
//!
//! * `Z` cycles the timed write-back (off, 5, 10, 30, 60 minutes), `L` seals the data when the screen locks, `Y` leaves cookies out at
//!   sign-out, `B` edits the sites that are never kept and `D` the sites whose data is left out at sign-out (a site covers its sub-domains).
//! * They only act while "Protect browser data" is on; the page says so when it is off.
//! Every change is audited without site names.

use super::*;
use iyakki_shared::profilepolicy::{MAX_SITES};
use iyakki_shared::profilepolicy::{clean_site, Policy};

const STEPS: [u32; 5] = [0, 5, 10, 30, 60];

fn next_interval(now: u32) -> u32 {
    let i = STEPS.iter().position(|v| *v == now);
    match i {
        Some(i) => STEPS[(i + 1) % STEPS.len()],
        None => 10,
    }
}

fn list_text(v: &[String]) -> String {
    if v.is_empty() { "none".to_string() } else { v.join(", ") }
}

/// A list of sites as typed (commas, spaces or new lines between them): each one checked, at most [`MAX_SITES`].
pub fn parse_sites(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split([',', ' ', '\n', ';']).map(str::trim).filter(|w| !w.is_empty()) {
        let s = clean_site(w)?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.len() > MAX_SITES {
        return Err(format!("at most {MAX_SITES} sites"));
    }
    out.sort();
    Ok(out)
}

impl SettingsApp {
    pub(super) fn render_browser_choices(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let p = Policy::load(cx.data_dir(), &cx.session.user);
        let on = cx.agent.profile_store().status_of(&cx.services, &cx.session.user) != iyakki_shared::profilestore::Status::Off;
        let mut y = y0 + controls::section_title(g, a.x, y0, s, "What is kept of protected browser data");
        let note = if on { "" } else { " (acts once Protect browser data is on)" };
        let gap = 8.0 * s;
        // the write-back interval: one row with its button
        let bw = g.text_width(15.0 * s, "Change (Z)", Face::Bold) + 15.0 * s * 1.7;
        let iv = if p.checkpoint_minutes == 0 { "off".to_string() } else { format!("every {} minutes while it changes", p.checkpoint_minutes) };
        controls::info_row(g, R::new(a.x, y, a.w - bw - 10.0 * s, 52.0 * s), s, &format!("Write back: {iv}{note}"), "The open copy is sealed again at this interval, so a crash loses little.");
        g.button(a.x + a.w - bw, y + 6.0 * s, 15.0 * s, "Change (Z)", ACCENT, Event::Key(Key::Char('Z')));
        y += 52.0 * s + gap;
        for (title, help, val, key) in [("Seal when the screen locks", "The decrypted copy is removed from memory when you lock the screen and comes back when you unlock.", p.lock_seal, 'L'), ("Leave cookies out at sign-out", "Cookies are not written into the sealed file when you sign out (the sites ask you to sign in again).", p.clear_cookies_on_exit, 'Y')] {
            controls::switch_row(g, R::new(a.x, y, a.w, 56.0 * s), s, title, help, val, true, Event::Key(Key::Char(key)), ACCENT);
            y += 56.0 * s + gap;
        }
        for (title, list, key) in [("Never keep the data of these sites", &p.never_persist, 'B'), ("Leave the data of these sites out at sign-out", &p.clear_sites_on_exit, 'D')] {
            let label = format!("Edit ({key})");
            let bw = g.text_width(15.0 * s, &label, Face::Bold) + 15.0 * s * 1.7;
            controls::info_row(g, R::new(a.x, y, a.w - bw - 10.0 * s, 52.0 * s), s, title, &list_text(list));
            g.button(a.x + a.w - bw, y + 6.0 * s, 15.0 * s, &label, ACCENT, Event::Key(Key::Char(key)));
            y += 52.0 * s + gap;
        }
        y - y0
    }

    /// Z, L, Y, B and D of the Browser tab.
    pub(super) fn handle_browser_choices(&mut self, k: &Key, cx: &AppCx) -> bool {
        let user = &cx.session.user;
        let dir = cx.data_dir();
        let mut p = Policy::load(dir, user);
        let what = match k {
            Key::Char('Z') => {
                p.checkpoint_minutes = next_interval(p.checkpoint_minutes);
                if p.checkpoint_minutes == 0 { "Write back: off.".to_string() } else { format!("Write back every {} minutes.", p.checkpoint_minutes) }
            }
            Key::Char('L') => {
                p.lock_seal = !p.lock_seal;
                format!("Seal at the lock screen: {}.", if p.lock_seal { "on" } else { "off" })
            }
            Key::Char('Y') => {
                p.clear_cookies_on_exit = !p.clear_cookies_on_exit;
                format!("Cookies left out at sign-out: {}.", if p.clear_cookies_on_exit { "yes" } else { "no" })
            }
            Key::Char('B') => {
                self.open_form(FormKind::BrowserSites(false), Form::new("Sites never kept").text("Sites", &p.never_persist.join(", "), "example.com, shop.example.org (empty: none)"));
                return true;
            }
            Key::Char('D') => {
                self.open_form(FormKind::BrowserSites(true), Form::new("Sites left out at sign-out").text("Sites", &p.clear_sites_on_exit.join(", "), "example.com, shop.example.org (empty: none)"));
                return true;
            }
            _ => return false,
        };
        match p.save(dir, user) {
            Ok(()) => {
                cx.audit.record(user, "settings.browser.protection", "choices changed");
                self.ok(what);
            }
            Err(e) => self.fail(e),
        }
        true
    }

    pub(super) fn browser_sites_submit(&mut self, cx: &AppCx, clear_on_exit: bool, form: &Form) -> Result<String, String> {
        let list = parse_sites(&form.value(0))?;
        let (user, dir) = (&cx.session.user, cx.data_dir());
        let mut p = Policy::load(dir, user);
        if clear_on_exit { p.clear_sites_on_exit = list.clone() } else { p.never_persist = list.clone() }
        p.save(dir, user)?;
        cx.audit.record(user, "settings.browser.protection", "site list changed");
        Ok(if list.is_empty() { "The list is empty.".to_string() } else { format!("{} site(s) in the list.", list.len()) })
    }
}
