//! Settings -> This computer: one install (a data volume on an external disk) can start on several computers, and the settings that
//! belong to a COMPUTER (sound, display, keyboard and pointer, microphone, GPU, power) are kept apart from the person's
//! (`iyakki_agent::hwprofile`). The page names this computer, lists the others the install has seen, lets an administrator choose per
//! kind of setting whether it stays per computer or is shared by all, copy another computer's settings here, reset this computer's
//! settings, or forget another computer. The three changes that replace settings ask first.

use super::*;
use iyakki_agent::hwprofile::{self as hw, Category};

/// What an approval card is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CompAct {
    /// Take the settings of this computer (its id) over this one's.
    Copy(String),
    /// Put this computer's settings back to automatic.
    Reset,
    /// Delete the profile of this computer (its id).
    Forget(String),
    /// Startup disk: make IYAKKI's own boot entry, put it first, start it once, or put the old order back.
    StartRegister,
    StartFirst,
    StartNext,
    StartRevert,
    /// Startup disk: restarts through `iyakki dev reboot` start IYAKKI once (on or off).
    RebootInto(bool),
}

/// What the list of computers is choosing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompPick {
    Copy,
    Forget,
}

/// A heading no wider than the page (the shared one is 640 px wide at scale 1, which runs off a phone).
fn heading(g: &mut Gfx, a: R, y: f32, s: f32, title: &str) -> f32 {
    crate::fx::section_title(g, a.x, y, a.w, s, title, crate::fx::CYAN)
}

const KEYS: [(Category, char); 6] = [(Category::Sound, 's'), (Category::Display, 'd'), (Category::Keyboard, 'k'), (Category::Microphone, 'm'), (Category::Gpu, 'g'), (Category::Power, 'p')];

/// A calendar date (UTC) from seconds since the epoch.
fn date(t: i64) -> String {
    if t <= 0 {
        return "unknown".into();
    }
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y}-{m:02}-{d:02}")
}

impl SettingsApp {
    fn comp_get(cx: &AppCx) -> Option<hw::Computers> {
        hw::computers_of(Self::svc(cx))
    }

    pub(super) fn computer_buttons(&self) -> Vec<Btn> {
        vec![("Copy settings from…", TEXT, 'c'), ("Reset this computer", TEXT, 'r'), ("Forget a computer…", ERR, 'f'), ("Start IYAKKI first", TEXT, 'b'), ("Start IYAKKI next time", TEXT, 'n')]
    }

    fn comp_others(c: &hw::Computers) -> Vec<hw::Profile> {
        c.list().into_iter().filter(|p| p.id != c.me.id).collect()
    }

    pub(super) fn handle_computer(&mut self, k: &Key, cx: &AppCx) {
        let Key::Char(ch) = k else { return };
        if !"sdkmgpcrfebnvw".contains(*ch) || !self.admin(cx) {
            return;
        }
        // the startup disk keys work without a per-computer profile folder
        let start = match ch {
            'e' => Some((CompAct::StartRegister, "Make IYAKKI's own entry in this computer's boot menu for the volume it is running from? One firmware boot entry described \"IYAKKI\" is written; other systems' entries and the boot order stay as they are (it is added last).")),
            'b' => Some((CompAct::StartFirst, "Make IYAKKI start first on this computer? IYAKKI's entry goes first in the firmware's boot order (it is made when missing); the other entries keep their order and the old order is saved, `v` puts it back.")),
            'n' => Some((CompAct::StartNext, "Start IYAKKI at the next restart only? The firmware's one-time next boot is set to IYAKKI; the boot order does not change.")),
            'v' => Some((CompAct::StartRevert, "Put the earlier boot order back and cancel a pending one-time start? IYAKKI's entry stays in the boot menu.")),
            'w' => {
                let on = !Self::svc(cx).startup_reboot_into_iyakki();
                Some((CompAct::RebootInto(on), if on { "Switch on \"Restart into IYAKKI\"? A restart asked through `iyakki dev reboot` then sets the firmware's one-time next boot to IYAKKI first, so a dual-boot computer does not start the other system." } else { "Switch off \"Restart into IYAKKI\"? A restart through `iyakki dev reboot` is left to the firmware's own order." }))
            }
            _ => None,
        };
        if let Some((act, text)) = start {
            self.modal = Some(Modal::Confirm(ConfirmKind::Computer(act), text.to_string()));
            return;
        }
        let Some(c) = Self::comp_get(cx) else { return self.fail("This install keeps no per-computer settings here.") };
        if let Some((cat, _)) = KEYS.iter().find(|(_, key)| key == ch) {
            let shared = c.shared().contains(cat);
            match hw::set_sharing(Self::svc(cx), *cat, !shared) {
                Ok(m) => {
                    cx.audit.record(&cx.session.user, "settings.computer.sharing", &format!("{}={}", cat.key(), !shared));
                    self.ok(m)
                }
                Err(e) => self.fail(e),
            }
            return;
        }
        match ch {
            'r' => self.modal = Some(Modal::Confirm(ConfirmKind::Computer(CompAct::Reset), format!("Put the sound, display, keyboard, microphone, GPU and power settings of {} back to automatic? The settings of your other computers stay as they are.", c.me.label))),
            'c' | 'f' => {
                let others = Self::comp_others(&c);
                if others.is_empty() {
                    return self.fail("No other computer has used this install yet.");
                }
                let items: Vec<String> = others.iter().map(|p| format!("{} ({})", p.label, p.id)).collect();
                let (kind, title) = if *ch == 'c' { (CompPick::Copy, "Copy settings from") } else { (CompPick::Forget, "Forget computer") };
                self.modal = Some(Modal::Pick(PickKind::Computer(kind), Picker::new(title, items)));
            }
            _ => {}
        }
    }

    /// A computer was picked from the list: ask before copying from it or forgetting it.
    pub(super) fn computer_pick(&mut self, cx: &AppCx, kind: CompPick, item: &str) {
        let Some(c) = Self::comp_get(cx) else { return };
        let Some(id) = item.rsplit_once(" (").map(|(_, r)| r.trim_end_matches(')').to_string()) else { return };
        let Some(p) = Self::comp_others(&c).into_iter().find(|p| p.id == id) else { return };
        let (act, text) = match kind {
            CompPick::Copy => (CompAct::Copy(id), format!("Replace the sound, display, keyboard, microphone, GPU and power settings of {} with those of {}? Kinds you share between computers are not touched.", c.me.label, p.label)),
            CompPick::Forget => (CompAct::Forget(id), format!("Forget {}? Its saved settings are deleted; if it is used again it starts from automatic. Nothing of yours is deleted.", p.label)),
        };
        self.modal = Some(Modal::Confirm(ConfirmKind::Computer(act), text));
    }

    pub(super) fn computer_confirm(&mut self, cx: &AppCx, act: CompAct) {
        if !self.admin(cx) {
            return;
        }
        let svc = Self::svc(cx);
        let sd_env = || {
            let mut e = iyakki_agent::startupdisk::Env::real();
            e.backup = svc.dir().to_path_buf();
            e
        };
        let r = match &act {
            CompAct::Copy(id) => hw::copy_from(svc, id),
            CompAct::Reset => hw::reset(svc),
            CompAct::Forget(id) => hw::forget(svc, id),
            CompAct::StartRegister => iyakki_agent::startupdisk::register(&sd_env()),
            CompAct::StartFirst => iyakki_agent::startupdisk::first(&sd_env()),
            CompAct::StartNext => iyakki_agent::startupdisk::next(&sd_env()),
            CompAct::StartRevert => iyakki_agent::startupdisk::revert(&sd_env()),
            CompAct::RebootInto(on) => svc.set_startup_reboot_into_iyakki(*on).map(|()| if *on { "Restarts through `iyakki dev reboot` now start IYAKKI once.".to_string() } else { "Restarts through `iyakki dev reboot` are left to the firmware's own order.".to_string() }),
        };
        let what = match &act {
            CompAct::StartRegister => "startup disk: register".to_string(),
            CompAct::StartFirst => "startup disk: first".to_string(),
            CompAct::StartNext => "startup disk: next".to_string(),
            CompAct::StartRevert => "startup disk: revert".to_string(),
            CompAct::RebootInto(on) => format!("startup disk: restart into IYAKKI {}", if *on { "on" } else { "off" }),
            CompAct::Copy(id) => format!("copy {id}"),
            CompAct::Reset => "reset".to_string(),
            CompAct::Forget(id) => format!("forget {id}"),
        };
        cx.audit.record(&cx.session.user, "settings.computer", &what);
        match r {
            Ok(m) => self.ok(m),
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_computer(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let Some(c) = Self::comp_get(cx) else {
            let mut y = a.y + heading(g, a, a.y, s, "This computer");
            for l in g.wrap(15.0 * s, "No per-computer settings are kept here (a cloud instance, or the profile folder could not be opened).", Face::Sans, a.w) {
                g.text(a.x, y, 15.0 * s, MUTED, &l, Face::Sans);
                y += 22.0 * s;
            }
            return;
        };
        let admin = cx.session.policy.manage_settings;
        let limit = a.y + a.h;
        let mut y = a.y + heading(g, a, a.y, s, "This computer");
        let me = c.profile();
        let line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], size: f32, face: Face, text: &str| {
            for l in g.wrap(size * s, text, face, a.w) {
                if *y + size * s * 1.3 > limit {
                    return;
                }
                g.text(a.x, *y, size * s, col, &l, face);
                *y += size * s * 1.45;
            }
        };
        line(g, &mut y, TEXT, 16.0, Face::Bold, &c.me.label);
        if let Some(p) = &me {
            line(g, &mut y, MUTED, 13.0, Face::Sans, &format!("Started {} time{} on it · first seen {} · last seen {}", p.boots, if p.boots == 1 { "" } else { "s" }, date(p.first_seen), date(p.last_seen)));
        }
        y += 6.0 * s;
        // the kinds of settings: on = kept per computer. Compact two-column rows when the page is short.
        let shared = c.shared();
        let full_h = 52.0 * s;
        let want_full = KEYS.len() as f32 * (full_h + 6.0 * s);
        let room = (limit - y) * 0.62;
        let (two_col, rh) = if want_full <= room { (false, full_h) } else if a.w >= 520.0 * s { (true, 40.0 * s) } else { (false, 34.0 * s) };
        let colw = if two_col { (a.w - 10.0 * s) / 2.0 } else { a.w };
        let mut ry = y;
        for (i, (cat, key)) in KEYS.iter().enumerate() {
            let (col, row_i) = if two_col { (i % 2, i / 2) } else { (0, i) };
            let x = a.x + col as f32 * (colw + 10.0 * s);
            let yy = y + row_i as f32 * (rh + 5.0 * s);
            let r = R::new(x, yy, colw, rh);
            let per = !shared.contains(cat);
            let hot = admin && g.hovered(r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, false, hot, ACCENT);
            let title = format!("{}: {}", cat.label(), if per { "per computer" } else { "shared by all" });
            let title = g.fit(14.5 * s, &title, Face::Bold, r.w - 100.0 * s);
            g.text(r.x + 14.0 * s, r.y + (r.h - g.line_height(14.5 * s, Face::Bold)) / 2.0, 14.5 * s, if admin { TEXT } else { MUTED }, &title, Face::Bold);
            let (sw, sh) = (44.0 * s, 24.0 * s);
            crate::fx::switch(g, R::new(r.x + r.w - 14.0 * s - sw, r.y + (r.h - sh) / 2.0, sw, sh), per, admin, ACCENT, s);
            if admin {
                g.hit(r, Event::Key(Key::Char(*key)), &title);
            }
            ry = ry.max(yy + rh);
        }
        y = ry + 8.0 * s;
        if !admin {
            line(g, &mut y, MUTED, 13.0, Face::Sans, "Only administrators change these.");
        }
        line(g, &mut y, DIM, 12.5, Face::Sans, "On: this kind of setting is remembered separately for each computer. Off: one value for all of them. Models, voice engines, the store, the firewall, the time zone and accounts always go with you.");
        y += 6.0 * s;
        // the startup disk: what the firmware starts, and the keys that change it (each asks first)
        let sd_env = iyakki_agent::startupdisk::Env::real();
        let sd_line = iyakki_agent::startupdisk::status_line(&sd_env, Self::svc(cx).startup_reboot_into_iyakki());
        line(g, &mut y, TEXT, 14.0, Face::Bold, "Startup disk");
        line(g, &mut y, MUTED, 13.0, Face::Sans, &sd_line);
        if admin {
            line(g, &mut y, DIM, 12.5, Face::Sans, "e makes IYAKKI's own boot entry · b starts IYAKKI first · n starts it at the next restart only · v puts the old order back · w restarts from developer mode into IYAKKI (on or off). Each asks first.");
        }
        y += 6.0 * s;
        if y + 40.0 * s < limit {
            y += heading(g, a, y, s, "Computers this install has seen");
        }
        for p in c.list() {
            if y + 22.0 * s > limit {
                break;
            }
            let here = p.id == c.me.id;
            let t = format!("{}{} · last seen {}", p.label, if here { "  (this computer)" } else { "" }, date(p.last_seen));
            let t = g.fit(14.0 * s, &t, Face::Sans, a.w);
            g.text(a.x, y, 14.0 * s, if here { OK } else { TEXT }, &t, Face::Sans);
            y += 21.0 * s;
        }
    }
}
