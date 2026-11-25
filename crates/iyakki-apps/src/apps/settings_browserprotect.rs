//! Settings -> Browser -> "Protect browser data": cookies, local storage and IndexedDB of the full browser are kept encrypted with
//! a key held in the person's vault (`iyakki_agent::profilestore`, design in `docs/development/BROWSER-PROFILE-PROTECTION.md`).
//! Off by default. Key P switches it on (with a confirmation that says what a forgotten password means) or off; key X deletes the
//! browser data. Every change is audited without paths or keys.

use super::*;
use super::browser_port::BrowserSettingsPort;
use iyakki_shared::profilestore::{Status};

const WARN_ON: &str = "Protect your browser data? Cookies, sign-ins and site storage are encrypted with a key kept in your vault, which your login password opens, and are decrypted only in memory while you are signed in. Close the browser first. If you forget your password the protected data cannot be recovered: you can only reset it, and you will be signed out of every site. Files that were plain on the drive are overwritten with zeros and removed, which a flash drive may not honour.";
const WARN_OFF: &str = "Stop protecting your browser data? Cookies and sign-ins become plain files on the drive again, readable by anybody who can read the drive. Close the browser first.";
const WARN_RESET: &str = "Delete all browser data (cookies, sign-ins, site storage)? You will be signed out of every site. This cannot be undone.";

impl SettingsApp {
    pub(super) fn render_browser_protect(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, p: &dyn BrowserSettingsPort) -> f32 {
        let st = p.profile_status();
        let on = st != Status::Off;
        let help = format!("Encrypts cookies and site data with a key in your vault, opened by your login password; open only while you are signed in. Now: {}. Forgotten password: the data cannot be recovered, only reset.", st.text());
        let rh = 86.0 * s;
        if y0 + rh > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &format!("Protect browser data: {}. Press P to change.", if on { "on" } else { "off" }), Face::Sans);
            }
            return 22.0 * s;
        }
        controls::switch_row(g, R::new(a.x, y0, a.w, rh), s, "Protect browser data", &help, on, true, Event::Key(Key::Char('P')), ACCENT);
        rh + 10.0 * s
    }

    /// P and X of the Browser tab.
    pub(super) fn handle_browser_protect(&mut self, k: &Key, p: &dyn BrowserSettingsPort) -> bool {
        match k {
            Key::Char('P') => {
                let (kind, text) = match p.profile_status() {
                    Status::Off | Status::NeedsFinish => (ConfirmKind::BrowserProtect, WARN_ON),
                    Status::Sealed | Status::Open => (ConfirmKind::BrowserUnprotect, WARN_OFF),
                };
                self.modal = Some(Modal::Confirm(kind, text.into()));
                true
            }
            Key::Char('X') => {
                self.modal = Some(Modal::Confirm(ConfirmKind::BrowserReset, WARN_RESET.into()));
                true
            }
            _ => false,
        }
    }

    /// The answer y to one of the three cards.
    pub(super) fn bp_confirm(&mut self, p: &dyn BrowserSettingsPort, kind: &ConfirmKind) {
                let (audit, r) = match kind {
            ConfirmKind::BrowserProtect => ("Browser data protection: on", p.profile_protect()),
            ConfirmKind::BrowserUnprotect => ("Browser data protection: off", p.profile_unprotect()),
            _ => ("Browser data reset", Ok(p.profile_reset())),
        };
        match r {
            Ok(m) => {
                p.audit("settings.browser", audit);
                self.ok(m);
            }
            Err(e) => {
                p.audit("settings.browser", &format!("{audit} failed"));
                self.fail(e);
            }
        }
    }
}
