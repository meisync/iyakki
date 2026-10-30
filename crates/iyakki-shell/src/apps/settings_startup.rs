//! Settings -> Startup: every service of the machine with where its start stands (the startup registry, `iyakki_agent::startup`):
//! a coloured dot, the name, the word (ready, starting, pending, failed, stopped, off) and the step it is on or the reason it failed.
//! Administrators get Retry (cluster, models) and Open (the service's own page) for the chosen row; the page is for administrators.

use super::*;
use iyakki_agent::startup::{self, Row, StartupState};

pub(super) const ROW_H: f32 = 44.0;

/// (colour, word) of a state.
fn look(st: &StartupState) -> ([u8; 4], &'static str) {
    match st {
        StartupState::Ready => (OK, "ready"),
        StartupState::Starting { .. } => (crate::fx::AMBER, "starting"),
        StartupState::Pending => (crate::fx::AMBER, "pending"),
        StartupState::Failed(_) => (ERR, "failed"),
        StartupState::Stopped => (DIM, "stopped"),
        StartupState::Disabled => (DIM, "off"),
    }
}

/// The second line of a row.
pub(super) fn detail(r: &Row) -> String {
    match &r.state {
        StartupState::Failed(why) => why.clone(),
        StartupState::Ready => String::new(),
        StartupState::Stopped => "stopped (by choice or because nothing needs it)".into(),
        StartupState::Disabled => "not on this machine".into(),
        _ => r.step.clone(),
    }
}

impl SettingsApp {
    pub(super) fn startup_buttons(&self) -> Vec<Btn> {
        vec![("Retry", ACCENT, 'e'), ("Open its page", TEXT, 'o'), ("Look again", TEXT, 'r')]
    }

    fn startup_rows() -> Vec<Row> {
        startup::global().snapshot()
    }

    pub(super) fn handle_startup(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::startup_rows();
        let len = rows.len();
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Home => self.move_sel(-(len as i32), len),
            Key::End => self.move_sel(len as i32, len),
            Key::Char('r') => {
                iyakki_agent::startup_probe::poll(&cx.services, startup::global(), iyakki_fw::service::alert());
                self.ok("Looked again.");
            }
            Key::Char('e') | Key::Char('o') => {
                let Some(row) = rows.get(self.sel()) else { return };
                if !cx.session.policy.manage_settings {
                    self.fail("Only an administrator can do that.");
                    return;
                }
                if matches!(k, Key::Char('e')) {
                    if !row.can_retry || !row.state.failed() {
                        self.fail(format!("{} is not waiting for a retry.", row.name));
                        return;
                    }
                    cx.audit.record(&cx.session.user, "startup.retry", row.id);
                    let (svc, id, user) = (cx.services.clone(), row.id, cx.session.user.clone());
                    std::thread::spawn(move || {
                        let _ = iyakki_agent::startup_probe::retry(&svc, id, &user);
                    });
                    self.ok(format!("Starting {} again.", row.name));
                } else {
                    match row.goes.item() {
                        Some(item) => self.open_item(item, cx),
                        None => self.fail(format!("{} has no page of its own.", row.name)),
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_startup(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::startup_rows();
        let admin = cx.session.policy.manage_settings;
        let sum = startup::global().summary();
        let head = if sum.all_ready() { format!("All services are ready ({} running).", sum.ready) } else { format!("{} of {} services are not ready yet.", sum.rows.len(), sum.tracked) };
        g.text(a.x, a.y, 15.0 * s, if sum.all_ready() { OK } else { crate::fx::AMBER }, &head, Face::Bold);
        let mut y = a.y + 28.0 * s;
        let rh = ROW_H * s;
        let sel = self.sel().min(rows.len().saturating_sub(1));
        for (i, r) in rows.iter().enumerate() {
            if y + rh > a.y + a.h {
                break;
            }
            let (col, word) = look(&r.state);
            let hot = g.hovered(R::new(a.x, y, a.w, rh - 4.0 * s));
            crate::fx::row_at(g, a.x, y, a.w, rh - 4.0 * s, 10.0 * s, i == sel, hot, ACCENT);
            g.hit(R::new(a.x, y, a.w, rh - 4.0 * s), Event::Row(i), r.name);
            g.circle(a.x + 16.0 * s, y + (rh - 4.0 * s) / 2.0, 5.0 * s, col);
            let word_w = 84.0 * s;
            g.text(a.x + 32.0 * s, y + 4.0 * s, 15.0 * s, TEXT, r.name, Face::Bold);
            g.text(a.x + a.w - word_w, y + 4.0 * s, 14.0 * s, col, word, Face::Bold);
            let d = detail(r);
            let d = if admin || !r.state.failed() { d } else { "did not start".to_string() };
            let d = g.fit(13.0 * s, &d, Face::Sans, a.w - 32.0 * s - word_w - 12.0 * s);
            g.text(a.x + 32.0 * s, y + 24.0 * s, 13.0 * s, MUTED, &d, Face::Sans);
            if r.state.busy() && matches!(r.state, StartupState::Starting { .. }) {
                let t = format!("{} s", r.age);
                let tw = g.text_width(13.0 * s, &t, Face::Sans);
                g.text(a.x + a.w - tw - 4.0 * s, y + 24.0 * s, 13.0 * s, DIM, &t, Face::Sans);
            }
            y += rh;
        }
    }
}
