//! Settings -> App events: which events each app listens to (F-SDK-188). One row per registration of the signed-in person's apps: the app,
//! the event, what it lets the app hear (the scope the app holds), how many events wait for it, and a switch. The person's switch goes through
//! the core (`cx.core.app_events().set_enabled`); an event switched off is neither queued nor delivered. Every change is audited by names.
//! Nothing here reads a payload: the page shows registrations and counts only.

use super::*;

/// One selectable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EventRow {
    pub app: String,
    pub event: String,
    pub filter: String,
    pub enabled: bool,
    pub gives: String,
    pub pending: usize,
}

impl SettingsApp {
    pub(super) fn appevents_rows(cx: &AppCx) -> Vec<EventRow> {
        let mut out = Vec::new();
        for (app, regs) in cx.core.app_events().apps(&cx.session.user) {
            for r in regs {
                out.push(EventRow { app: app.clone(), event: r.event, filter: r.filter, enabled: r.enabled, gives: r.gives, pending: r.pending });
            }
        }
        out
    }

    pub(super) fn appevents_len(cx: &AppCx) -> usize {
        Self::appevents_rows(cx).len()
    }

    pub(super) fn appevents_buttons(&self) -> Vec<Btn> {
        vec![("Turn off or on", ACCENT, 't'), ("Take its permission away", ERR, 'p'), ("Look again", TEXT, 'r')]
    }

    pub(super) fn handle_appevents(&mut self, k: &Key, cx: &AppCx) {
        let len = Self::appevents_len(cx);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Enter | Key::Char('t') => self.appevents_toggle(self.sel(), cx),
            Key::Char('p') => self.appevents_revoke(self.sel(), cx),
            _ => {}
        }
    }

    pub(super) fn appevents_sub(&mut self, row: usize, c: char, cx: &AppCx) {
        self.sel[self.tab] = row;
        if c == 't' {
            self.appevents_toggle(row, cx);
        } else if c == 'p' {
            self.appevents_revoke(row, cx);
        }
    }

    /// The person takes the permission this event needs away from the app: the registration ends and the app is told (`app.permissions`).
    fn appevents_revoke(&mut self, row: usize, cx: &AppCx) {
        let Some(r) = Self::appevents_rows(cx).get(row).cloned() else {
            self.fail("Nothing is chosen.");
            return;
        };
        let Some(scope) = cx.core.app_events().catalogue().into_iter().find(|e| e.name == r.event).map(|e| e.scope) else {
            self.fail("That event has no permission to take away.");
            return;
        };
        match cx.core.app_events().set_permission(&cx.session.user, &r.app, &scope, false, "person") {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "appevents.permission", &format!("{} {scope} taken away", r.app));
                self.ok(format!("{} can no longer use {scope}. It was told, and what it registered for with it has ended.", r.app));
            }
            Err(e) => self.fail(e),
        }
    }

    fn appevents_toggle(&mut self, row: usize, cx: &AppCx) {
        let Some(r) = Self::appevents_rows(cx).get(row).cloned() else {
            self.fail("Nothing is chosen.");
            return;
        };
        let on = !r.enabled;
        match cx.core.app_events().set_enabled(&cx.session.user, &r.app, &r.event, &r.filter, on) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "appevents.switch", &format!("{} {} {}", r.app, r.event, if on { "on" } else { "off" }));
                self.ok(if on { format!("{} hears {} again.", r.app, r.event) } else { format!("{} no longer hears {}. Nothing waits for it.", r.app, r.event) });
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_appevents(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::appevents_rows(cx);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 44.0 * s;
        let room = ((a.h / row_h) as usize).max(1);
        let start = sel.saturating_sub(room - 1);
        let mut y = a.y;
        if rows.is_empty() {
            let msg = g.fit(13.0 * s, "No app listens to events yet. An app registers for the events it needs; you see each one here and can switch it off.", Face::Sans, a.w);
            g.text(a.x, y + 4.0 * s, 13.0 * s, DIM, &msg, Face::Sans);
            return;
        }
        for (i, r) in rows.iter().enumerate().skip(start).take(room) {
            let rr = R::new(a.x, y, a.w, row_h - 4.0 * s);
            if i == sel {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 8.0 * s, rgb(30, 38, 68));
            }
            g.hit(rr, Event::Row(i), "row");
            let size = 12.5 * s;
            let label = if r.enabled { "Turn off" } else { "Turn on" };
            let w = g.text_width(size, label, Face::Bold) + size * 1.7;
            let right = rr.x + rr.w - 8.0 * s - w;
            g.button(right, rr.y + (rr.h - size * 2.1) / 2.0, size, label, if r.enabled { ERR } else { ACCENT }, Event::Sub(i as u8, Key::Char('t')));
            let text_w = (right - rr.x - 24.0 * s).max(40.0 * s);
            let what = if r.filter.is_empty() { r.event.clone() } else { format!("{} ({})", r.event, r.filter) };
            let t = g.fit(14.0 * s, &format!("{}: {}", r.app, what), Face::Bold, text_w);
            g.text(rr.x + 12.0 * s, rr.y + 4.0 * s, 14.0 * s, if r.enabled { TEXT } else { MUTED }, &t, Face::Bold);
            let state = if r.enabled { format!("{} waiting", r.pending) } else { "switched off".to_string() };
            let sub = g.fit(12.0 * s, &format!("{} · {state}", r.gives), Face::Sans, text_w);
            g.text(rr.x + 12.0 * s, rr.y + 23.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
            y += row_h;
        }
    }
}
