//! Settings -> Alexa (Amazon's assistant behind the voice seam, optional, off by default; F-VOICE-43). The decisions are
//! `iyakki_shared::alexa`; the protocol is `iyakki_agent::avs` / `avs_h2`.
//!
//! * The owner's Amazon product (`alexa-avs.json`) is shown as set up or not. With none the switch and the card refuse in words
//!   (FAIL CLOSED: no client id, nothing is ever dialled).
//! * On or off is the machine's choice (administrators).
//! * The CONSENT CARD names what leaves the computer (the voice during a turn) and says Amazon's assistant is not IYAKKI's. "No" is the
//!   first choice, so Enter alone never agrees; only a person's key or tap on this page can say Yes (no assistant tool can).
//! * Everything but the yes is also a chat tool (`alexa_status`, `alexa_set`).

use super::*;
use iyakki_shared::alexa;

const NO: &str = "No: keep my voice on this computer";
const YES: &str = "Yes: allow sending my voice to Amazon during an Alexa turn";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AlexaRow {
    Status,
    Switch,
    Consent,
}

pub(super) const ROWS: [AlexaRow; 3] = [AlexaRow::Status, AlexaRow::Switch, AlexaRow::Consent];

impl SettingsApp {
    pub(super) fn alexa_rows(&self) -> &'static [AlexaRow] {
        &ROWS
    }

    pub(super) fn alexa_consent_form() -> Form {
        Form::new("Send my voice to Amazon?").with_note(&alexa::consent_card()).choice("Your answer (Left/Right to change, Enter to confirm)", &[NO, YES], 0)
    }

    pub(super) fn alexa_switch(&mut self, cx: &AppCx) {
        if !cx.session.policy.manage_settings {
            return self.fail("Only an administrator switches Alexa on or off.");
        }
        let on = !alexa::load(cx.data_dir()).enabled;
        match alexa::set_enabled(cx.data_dir(), on) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.alexa", if on { "switched on" } else { "switched off" });
                self.ok(if on { "Alexa is available. Nothing is sent to Amazon until you say yes (key C) and start a turn." } else { "Alexa is off." });
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn alexa_consent_key(&mut self, cx: &AppCx) {
        if !alexa::configured(cx.data_dir()) {
            return self.fail(alexa::NO_CLIENT_ID);
        }
        if alexa::consent_date(cx.data_dir(), &cx.session.user).is_some() {
            // taking a yes back is always safe: no question
            return match alexa::revoke(cx.data_dir(), &cx.session.user) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.alexa", "consent taken back");
                    self.ok("Your yes is taken back. Nothing goes to Amazon.");
                }
                Err(e) => self.fail(e),
            };
        }
        self.open_form(FormKind::AlexaConsent, Self::alexa_consent_form());
    }

    /// The person answered the card (only a person at this page gets here; no assistant tool leads to it).
    pub(super) fn alexa_consent_submit(&mut self, cx: &AppCx, form: &Form) {
        if form.choice_idx(0) != 1 {
            return self.ok("Understood: your voice stays on this computer.");
        }
        match alexa::grant(cx.data_dir(), &cx.session.user, &iyakki_clock::today()) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.alexa", "consent given for the voice");
                self.ok("Your yes is kept. Your voice goes to Amazon only during an Alexa turn you start. Press C to take it back.");
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn handle_alexa(&mut self, k: &Key, cx: &AppCx) {
        let len = ROWS.len();
        let sel = self.sel().min(len - 1);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Home => self.move_sel(-(len as i32), len),
            Key::End => self.move_sel(len as i32, len),
            Key::Enter => match ROWS[sel] {
                AlexaRow::Status => {}
                AlexaRow::Switch => self.alexa_switch(cx),
                AlexaRow::Consent => self.alexa_consent_key(cx),
            },
            Key::Char('o') => self.alexa_switch(cx),
            Key::Char('C') | Key::Char('c') => self.alexa_consent_key(cx),
            _ => {}
        }
    }

    pub(super) fn alexa_buttons(&self) -> Vec<Btn> {
        match ROWS[self.sel().min(ROWS.len() - 1)] {
            AlexaRow::Switch => vec![("On / off", ACCENT, 'o')],
            _ => vec![("Consent card", ACCENT, 'C')],
        }
    }

    pub(super) fn alexa_row_text(&self, cx: &AppCx, row: AlexaRow) -> (String, String) {
        let dir = cx.data_dir();
        match row {
            AlexaRow::Status => ("Status".into(), alexa::status_text(dir, &cx.session.user)),
            AlexaRow::Switch => (
                "Alexa on this computer".into(),
                if !alexa::configured(dir) {
                    "Not available: the owner has not put an Alexa Voice Service product (client id and product id) on this computer. Nothing is sent to Amazon.".into()
                } else if alexa::load(dir).enabled {
                    "On. Nothing is sent until you say yes (key C) and start a turn.".into()
                } else {
                    "Off (the default). Nothing is sent to Amazon.".into()
                },
            ),
            AlexaRow::Consent => (
                "Your yes to sending your voice".into(),
                match alexa::consent_date(dir, &cx.session.user) {
                    Some(d) => format!("Yes, on {d}. Your voice goes to Amazon only during a turn you start. Press C to take it back."),
                    None => "Not given: nothing is sent. Press C to read the card and answer it.".into(),
                },
            ),
        }
    }

    pub(super) fn render_alexa(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let sel = self.sel().min(ROWS.len() - 1);
        let rh = 62.0 * s;
        let mut y = a.y;
        let head = g.fit(14.0 * s, "Optional, off by default. Amazon's assistant is not IYAKKI's: it uses none of your tools or approvals.", Face::Bold, a.w);
        g.text(a.x, y, 14.0 * s, MUTED, &head, Face::Bold);
        y += 26.0 * s;
        for (i, row) in ROWS.iter().enumerate() {
            if i > 0 && y + rh > a.y + a.h {
                break;
            }
            let (title, detail) = self.alexa_row_text(cx, *row);
            let r = R::new(a.x, y, a.w, rh - 4.0 * s);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &title);
            let tw = r.w - 24.0 * s;
            let t = g.fit(15.0 * s, &title, Face::Bold, tw);
            g.text(r.x + 12.0 * s, y + 4.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
            let words: Vec<&str> = detail.split(' ').collect();
            let (mut l1, mut l2) = (String::new(), String::new());
            for w in words {
                let target = if l2.is_empty() && g.text_width(13.0 * s, &format!("{l1} {w}"), Face::Sans) <= tw { &mut l1 } else { &mut l2 };
                if !target.is_empty() {
                    target.push(' ');
                }
                target.push_str(w);
            }
            let l1 = g.fit(13.0 * s, l1.trim(), Face::Sans, tw);
            let l2 = g.fit(13.0 * s, l2.trim(), Face::Sans, tw);
            g.text(r.x + 12.0 * s, y + 24.0 * s, 13.0 * s, MUTED, &l1, Face::Sans);
            g.text(r.x + 12.0 * s, y + 40.0 * s, 13.0 * s, MUTED, &l2, Face::Sans);
            y += rh;
        }
    }
}
