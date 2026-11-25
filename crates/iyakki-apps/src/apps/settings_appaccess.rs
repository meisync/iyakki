//! Settings -> Apps access: the device and system access apps hold (docs/development/CORE-ACCESS-MODEL.md sections 5 and 6b, slice 2).
//! One page: the list of what installed apps were allowed at install (app, capability, who approved it), a button to take one away, and below
//! it the access log of this computer (who, which app, what, which camera, when, allowed or refused). Administrators only. The log is kept on
//! this machine (`iyakki_shared::accessbroker`), holds no pictures and no content, and is bounded. A revoke applies at the broker's next call.
//! Built-in apps hold their access by a signed table and are not listed here.

use iyakki_shared::accessbroker::{self as ab, FileLog};

use super::*;
use super::apps_port::AppAccessPort;

/// One selectable row: an installation and one capability it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AccessRow {
    pub installation: String,
    pub app: String,
    pub version: String,
    pub who: String,
    pub capability: String,
    pub revoked: bool,
}

const LOG_LINES: usize = 40;

impl SettingsApp {
    pub(super) fn appaccess_rows(p: &dyn AppAccessPort) -> Vec<AccessRow> {
        let mut out = Vec::new();
        for h in ab::held_list(&p.data_dir()) {
            for c in &h.holds {
                out.push(AccessRow { installation: h.installation.clone(), app: h.app.clone(), version: h.version.clone(), who: h.who.clone(), capability: c.clone(), revoked: false });
            }
            for c in &h.revoked {
                out.push(AccessRow { installation: h.installation.clone(), app: h.app.clone(), version: h.version.clone(), who: h.who.clone(), capability: c.clone(), revoked: true });
            }
        }
        out
    }

    pub(super) fn appaccess_len(p: &dyn AppAccessPort) -> usize {
        Self::appaccess_rows(p).len()
    }

    pub(super) fn appaccess_buttons(&self) -> Vec<Btn> {
        vec![("Take it away", ERR, 'r'), ("Time limit…", TEXT, 'e'), ("Clear the log", TEXT, 'c')]
    }

    pub(super) fn handle_appaccess(&mut self, k: &Key, p: &dyn AppAccessPort) {
        let len = Self::appaccess_len(p);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('r') | Key::Delete => self.appaccess_revoke(self.sel(), p),
            Key::Char('e') => self.appaccess_limit_form(self.sel(), p),
            Key::Char('c') => self.appaccess_clear(p),
            _ => {}
        }
    }

    /// A chip on a row.
    pub(super) fn appaccess_sub(&mut self, row: usize, c: char, p: &dyn AppAccessPort) {
        self.sel[self.tab] = row;
        if c == 'r' {
            self.appaccess_revoke(row, p);
        }
    }

    fn appaccess_revoke(&mut self, row: usize, p: &dyn AppAccessPort) {
        if !self.admin(p) {
            return;
        }
        let Some(r) = Self::appaccess_rows(p).get(row).cloned() else {
            self.fail("Nothing is chosen.");
            return;
        };
        if r.revoked {
            self.ok(format!("{} was already taken away from {}.", r.capability, r.app));
            return;
        }
        match p.access_revoke(&r.installation, &r.capability) {
            Ok(true) => {
                p.audit("access.revoke", &format!("{} {} {} (approved by {})", r.app, r.version, r.capability, r.who));
                // the app is told what it may do now (and an events scope taken away ends its registration), whether it runs or not
                let owner = r.installation.split('/').nth(2).filter(|_| r.installation.starts_with("tile/user/")).unwrap_or(&p.user()).to_string();
                let _ = p.app_events().permissions_changed(&owner, &r.app, &[], &[r.capability.clone()], "administrator", false);
                self.ok(format!("{} can no longer use {}. It applies at its next try.", r.app, r.capability));
            }
            Ok(false) => self.ok(format!("{} was already taken away from {}.", r.capability, r.app)),
            Err(e) => self.fail(e),
        }
    }

    /// Opens the form that gives the chosen grant an end (F-SDK-217).
    fn appaccess_limit_form(&mut self, row: usize, p: &dyn AppAccessPort) {
        if !self.admin(p) {
            return;
        }
        let Some(r) = Self::appaccess_rows(p).get(row).cloned() else { return self.fail("Nothing is chosen.") };
        if r.revoked {
            return self.fail(format!("{} was taken away from {}: there is nothing to limit.", r.capability, r.app));
        }
        let f = Form::new(&format!("Time limit for {} on {}", r.capability, r.app)).text("Hours it may still use it (1 to 8784; empty: no limit)", "", "for example 24");
        self.open_form(FormKind::AppAccessLimit(r.installation, r.capability), f);
    }

    /// The form was submitted: the limit is written and the app is told by the core's own look at the next tick (`app.permissions`, cause `expiry`).
    pub(super) fn appaccess_limit_submit(&mut self, p: &dyn AppAccessPort, installation: &str, capability: &str, form: &Form) -> Result<String, String> {
        if !self.admin(p) {
            return Err("Only an administrator may change what apps may use.".into());
        }
        let text = form.value(0);
        let hours = match text.trim() {
            "" => None,
            t => Some(t.parse::<u32>().ok().filter(|h| (1..=8784).contains(h)).ok_or("Hours: a whole number from 1 to 8784, or empty for no limit.")?),
        };
        match p.access_limit(installation, capability, hours)? {
            true => {
                p.audit("access.limit", &format!("{installation} {capability} {}", hours.map_or("no limit".to_string(), |h| format!("{h} h"))));
                Ok(match hours {
                    Some(h) => format!("{capability} may be used for {h} more hour(s). It applies at the app's next try."),
                    None => format!("{capability} has no time limit now."),
                })
            }
            false => Err(format!("{capability} is not held now, so no limit was set.")),
        }
    }

    fn appaccess_clear(&mut self, p: &dyn AppAccessPort) {
        if !self.admin(p) {
            return;
        }
        FileLog::open(&p.data_dir()).clear();
        p.audit("access.log.clear", "the access log was emptied");
        self.ok("The access log is empty.");
    }

    pub(super) fn render_appaccess(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn AppAccessPort) {
        let rows = Self::appaccess_rows(p);
        let log = FileLog::open(&p.data_dir()).recent(LOG_LINES);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 44.0 * s;
        let top_h = (a.h * 0.5).max(row_h * 2.0);
        let room = ((top_h / row_h) as usize).max(1);
        let start = sel.saturating_sub(room - 1);
        let mut y = a.y;
        if rows.is_empty() {
            let msg = g.fit(13.0 * s, "No installed app has device access yet (default apps are not listed).", Face::Sans, a.w);
            g.text(a.x, y + 4.0 * s, 13.0 * s, DIM, &msg, Face::Sans);
            y += row_h;
        }
        for (i, r) in rows.iter().enumerate().skip(start).take(room) {
            let rr = R::new(a.x, y, a.w, row_h - 4.0 * s);
            if i == sel {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 8.0 * s, rgb(30, 38, 68));
            }
            g.hit(rr, Event::Row(i), "row");
            let size = 12.5 * s;
            let mut right = rr.x + rr.w - 8.0 * s;
            if r.revoked {
                let w = g.text_width(size, "Taken away", Face::Bold);
                right -= w;
                g.text(right, rr.y + (rr.h - size) / 2.0, size, ERR, "Taken away", Face::Bold);
            } else {
                let w = g.text_width(size, "Take away", Face::Bold) + size * 1.7;
                right -= w;
                g.button(right, rr.y + (rr.h - size * 2.1) / 2.0, size, "Take away", ERR, Event::Sub(i as u8, Key::Char('r')));
            }
            let text_w = (right - rr.x - 24.0 * s).max(40.0 * s);
            let t = g.fit(14.0 * s, &format!("{}: {}", r.app, r.capability), Face::Bold, text_w);
            g.text(rr.x + 12.0 * s, rr.y + 4.0 * s, 14.0 * s, if r.revoked { MUTED } else { TEXT }, &t, Face::Bold);
            let sub = g.fit(12.0 * s, &format!("version {} · approved by {}", r.version, r.who), Face::Sans, text_w);
            g.text(rr.x + 12.0 * s, rr.y + 23.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
            y += row_h;
        }
        // the log
        let ly = y + 8.0 * s;
        if ly + 24.0 * s > a.y + a.h {
            return; // no room under the list on a small window: the list matters more than the log
        }
        let head = g.fit(13.0 * s, "Who used what (kept on this computer only; no pictures, no content)", Face::Bold, a.w);
        g.text(a.x, ly, 13.0 * s, ACCENT, &head, Face::Bold);
        let line_h = 17.0 * s;
        let mut yy = ly + 22.0 * s;
        if log.is_empty() {
            g.text(a.x, yy, 12.5 * s, DIM, "Nothing has been used yet.", Face::Sans);
        }
        let zone = iyakki_clock::Zone::system_with(None);
        for e in log {
            if yy + line_h > a.y + a.h {
                break;
            }
            let when = iyakki_clock::format(&zone, e.at, "%d %b %H:%M");
            let verdict = if e.allowed { "allowed".to_string() } else { format!("refused ({})", e.reason) };
            let line = format!("{when}  {} · {} · {} {} · {}", e.person, e.app, e.capability, e.target, verdict);
            let line = g.fit(12.5 * s, &line, Face::Sans, a.w);
            g.text(a.x, yy, 12.5 * s, if e.allowed { MUTED } else { ERR }, &line, Face::Sans);
            yy += line_h;
        }
    }
}
