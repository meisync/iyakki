//! Settings -> Voice assistant (Gemini, experimental): the live voice session of `iyakki_shared::geminilive` (docs/GEMINI-PROVIDER.md,
//! docs/development/GEMINI-VOICE-DESIGN.md). EXPERIMENTAL and off by default.
//!
//! * On or off (administrators; the machine's choice, kept in `gemini.json`). Turning it on sends nothing: a session also needs the
//!   person's own yes (below) and a key in the vault (Settings -> Models, key g).
//! * The CONSENT CARD names what leaves the computer (the open microphone during a session and the results of tools) and is a choice
//!   with "No" first, so Enter alone never agrees; only a person's key or tap on this page can say Yes (no assistant tool can).
//! * Who speaks (the local voice reads Gemini's text, or Gemini's own voice), the session caps, the start key's name, the opt-in wake
//!   phrase hand-off, the status with the delay log and the limit meter, and a Stop button that closes the microphone at once.
//! * Everything here is also a chat tool (`gemini_voice_status`, `gemini_voice_set`, `gemini_voice_stop`).

use super::*;
use iyakki_shared::gemini::{ConsentState};
use iyakki_shared::gemini::{self};
use iyakki_shared::geminilive::{LiveChange, VoiceOut};
use iyakki_shared::geminilive::{self};

const NO: &str = "No: keep my voice on this computer";
const YES: &str = "Yes: allow sending my microphone to Google while a session is open";

/// The shortcut names the key row steps through (any valid name can be set by the chat tool).
pub(super) const KEYS: [&str; 5] = ["Super+V", "Super+G", "Ctrl+Alt+V", "Ctrl+Alt+G", "Super+Shift+V"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GvRow {
    Status,
    Switch,
    Consent,
    VoiceOut,
    SessionLen,
    Quiet,
    DayCap,
    SpokenCap,
    Shortcut,
    Wake,
    Stop,
}

pub(super) const ROWS: [GvRow; 11] = [GvRow::Status, GvRow::Switch, GvRow::Consent, GvRow::VoiceOut, GvRow::SessionLen, GvRow::Quiet, GvRow::DayCap, GvRow::SpokenCap, GvRow::Shortcut, GvRow::Wake, GvRow::Stop];

fn step(v: u32, up: bool, steps: &[u32]) -> u32 {
    if up { steps.iter().copied().find(|s| *s > v).unwrap_or(*steps.last().unwrap_or(&v)) } else { steps.iter().rev().copied().find(|s| *s < v).unwrap_or(steps[0]) }
}

const LEN_STEPS: [u32; 7] = [30, 60, 120, 300, 600, 900, 900];
const QUIET_STEPS: [u32; 7] = [5, 10, 20, 30, 60, 120, 300];
const DAY_STEPS: [u32; 8] = [60, 300, 600, 1800, 3600, 7200, 18_000, 36_000];
const SPOKEN_STEPS: [u32; 7] = [100, 200, 400, 600, 900, 1400, 2000];

impl SettingsApp {
    fn gv_cfg(cx: &AppCx) -> geminilive::LiveCfg {
        gemini::load(cx.data_dir()).voice
    }

    pub(super) fn gv_rows(&self) -> &'static [GvRow] {
        &ROWS
    }

    fn gv_apply(&mut self, cx: &AppCx, ch: LiveChange, said: String) {
        if !cx.session.policy.manage_settings {
            return self.fail("Only an administrator changes the Gemini voice assistant.");
        }
        match geminilive::apply(cx.data_dir(), &ch) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "settings.gemini_voice", &said);
                self.ok(said);
            }
            Err(e) => self.fail(e),
        }
    }

    /// The card, "No" first: only a person on this page can answer it.
    pub(super) fn gv_consent_form(cx: &AppCx) -> Form {
        let tier = gemini::load(cx.data_dir()).tier;
        Form::new("Send my microphone to Google? (EXPERIMENTAL)").with_note(&geminilive::consent_card_live(tier)).choice("Your answer (Left/Right to change, Enter to confirm)", &[NO, YES], 0)
    }

    pub(super) fn gv_consent_key(&mut self, cx: &AppCx) {
        match gemini::live_consent_state(cx.data_dir(), &cx.session.user) {
            ConsentState::Given { date } => {
                self.modal = Some(Modal::Confirm(ConfirmKind::GeminiRevoke, format!("You said yes on {date}. Take your yes back? Nothing goes to Google after that: not the microphone, not speech-to-text, not the assistant's messages.")));
            }
            _ => self.open_form(FormKind::GeminiLiveConsent, Self::gv_consent_form(cx)),
        }
    }

    /// The person answered the card (only a person at this page gets here; no assistant tool leads to it).
    pub(super) fn gv_consent_submit(&mut self, cx: &AppCx, form: &Form) {
        if form.choice_idx(0) != 1 {
            return self.ok("Understood: your microphone stays on this computer.");
        }
        match gemini::grant_live_on(cx.data_dir(), &cx.session.user, &iyakki_clock::today()) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.gemini_voice", &format!("consent given for the microphone ({} tier)", gemini::load(cx.data_dir()).tier.label()));
                self.ok("Your yes is kept. A session sends your microphone to Google only while it is open (red sign). You can take it back here at any time.");
            }
            Err(e) => self.fail(e),
        }
    }

    fn gv_act(&mut self, cx: &AppCx, row: GvRow, dir: i32) {
        let c = Self::gv_cfg(cx);
        let up = dir >= 0;
        match row {
            GvRow::Status => {}
            GvRow::Switch => {
                let on = !c.enabled;
                self.gv_apply(cx, LiveChange { enabled: Some(on), ..Default::default() }, if on { "The Gemini voice assistant (EXPERIMENTAL) is available. Nothing is sent until you say yes (key C) and start a session.".into() } else { "The Gemini voice assistant is off.".into() });
            }
            GvRow::Consent => self.gv_consent_key(cx),
            GvRow::VoiceOut => {
                let next = if c.voice_out == VoiceOut::Local { VoiceOut::Gemini } else { VoiceOut::Local };
                self.gv_apply(cx, LiveChange { voice_out: Some(next), ..Default::default() }, format!("Who speaks: {}.", next.label()));
            }
            GvRow::SessionLen => {
                let v = step(c.max_session_secs, up, &LEN_STEPS);
                self.gv_apply(cx, LiveChange { max_session_secs: Some(v), ..Default::default() }, format!("A session ends after {v} seconds."));
            }
            GvRow::Quiet => {
                let v = step(c.idle_secs, up, &QUIET_STEPS);
                self.gv_apply(cx, LiveChange { idle_secs: Some(v), ..Default::default() }, format!("The microphone closes after {v} seconds of quiet."));
            }
            GvRow::DayCap => {
                let v = step(c.day_cap_secs, up, &DAY_STEPS);
                self.gv_apply(cx, LiveChange { day_cap_secs: Some(v), ..Default::default() }, format!("Your daily cap of microphone time sent to Google is {v} seconds."));
            }
            GvRow::SpokenCap => {
                let v = step(c.spoken_cap_chars, up, &SPOKEN_STEPS);
                self.gv_apply(cx, LiveChange { spoken_cap_chars: Some(v), ..Default::default() }, format!("At most {v} characters of a reply are read aloud."));
            }
            GvRow::Shortcut => {
                let i = KEYS.iter().position(|k| *k == c.key).map_or(0, |i| if up { (i + 1) % KEYS.len() } else { (i + KEYS.len() - 1) % KEYS.len() });
                self.gv_apply(cx, LiveChange { key: Some(KEYS[i].to_string()), ..Default::default() }, format!("The start key is {}.", KEYS[i]));
            }
            GvRow::Wake => {
                let on = !c.wake_handoff;
                self.gv_apply(cx, LiveChange { wake_handoff: Some(on), ..Default::default() }, if on { "The wake phrase (experimental, opt-in) may open a Gemini voice session. Nothing is sent before the phrase is heard on this computer; the wake phrase itself must be on (Settings -> Voice).".into() } else { "The wake phrase no longer opens a Gemini voice session.".into() });
            }
            GvRow::Stop => self.gv_stop(cx),
        }
    }

    /// Stop now: every open session ends and the microphone closes. Free for everyone (a plain person may always stop their own microphone).
    pub(super) fn gv_stop(&mut self, cx: &AppCx) {
        let was = geminilive::request_stop();
        cx.audit.record(&cx.session.user, "settings.gemini_voice", if was { "stopped an open session" } else { "stop asked, none was open" });
        self.ok(if was { "The Gemini voice session is being closed: the microphone is shut and nothing more is sent." } else { "No Gemini voice session was open. Nothing is being sent." });
    }

    pub(super) fn handle_geminivoice(&mut self, k: &Key, cx: &AppCx) {
        let len = ROWS.len();
        let sel = self.sel().min(len - 1);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Home => self.move_sel(-(len as i32), len),
            Key::End => self.move_sel(len as i32, len),
            Key::Enter => self.gv_act(cx, ROWS[sel], 1),
            Key::Char('+') | Key::Right => self.gv_act(cx, ROWS[sel], 1),
            Key::Char('-') | Key::Left => self.gv_act(cx, ROWS[sel], -1),
            Key::Char('C') | Key::Char('c') => self.gv_consent_key(cx),
            Key::Char('v') => self.gv_act(cx, GvRow::VoiceOut, 1),
            Key::Char('k') => self.gv_act(cx, GvRow::Shortcut, 1),
            Key::Char('w') => self.gv_act(cx, GvRow::Wake, 1),
            Key::Char('o') => self.gv_act(cx, GvRow::Switch, 1),
            Key::Char('x') | Key::Char('X') => self.gv_stop(cx),
            _ => {}
        }
    }

    pub(super) fn geminivoice_buttons(&self) -> Vec<Btn> {
        let row = ROWS[self.sel().min(ROWS.len() - 1)];
        let mut b: Vec<Btn> = match row {
            GvRow::Switch => vec![("On / off", ACCENT, 'o')],
            GvRow::Consent => vec![("Consent card", ACCENT, 'C')],
            GvRow::VoiceOut => vec![("Who speaks", ACCENT, 'v')],
            GvRow::Shortcut => vec![("Next key", ACCENT, 'k')],
            GvRow::Wake => vec![("Wake hand-off on / off", ACCENT, 'w')],
            GvRow::SessionLen | GvRow::Quiet | GvRow::DayCap | GvRow::SpokenCap => vec![("More (+)", ACCENT, '+'), ("Less (-)", MUTED, '-')],
            GvRow::Status | GvRow::Stop => vec![("Consent card", MUTED, 'C')],
        };
        b.push(("Stop now", crate::ui::ERR, 'x'));
        b
    }

    pub(super) fn gv_row_text(&self, cx: &AppCx, row: GvRow) -> (String, String) {
        let c = Self::gv_cfg(cx);
        let today = iyakki_clock::today();
        let key_present = cx.secrets().resolve(gemini::KEY_REF, None).is_ok();
        match row {
            GvRow::Status => {
                let live = if geminilive::mic_open() { "A session is open: the microphone is open (red sign)." } else { "No session is open." };
                ("Status (EXPERIMENTAL)".into(), format!("{live} {} {}", geminilive::latency_text(), geminilive::meter_text(&c, &today)))
            }
            GvRow::Switch => ("Gemini voice assistant (EXPERIMENTAL)".into(), if c.enabled { format!("On. Start it with {} or the microphone button. Key: {}.", c.key, if key_present { "stored" } else { "not stored (Settings -> Models, key g)" }) } else { "Off (the default). Nothing is sent to Google.".into() }),
            GvRow::Consent => ("Your yes to sending the microphone".into(), match gemini::live_consent_state(cx.data_dir(), &cx.session.user) {
                ConsentState::Given { date } => format!("Yes, on {date}. The microphone goes to Google only while a session is open. Press C to take it back."),
                ConsentState::OtherTier { date, .. } => format!("You said yes on {date} for the other tier: the question is asked again. Press C."),
                ConsentState::Missing => "Not given: nothing is sent. Press C to read the card and answer it.".into(),
            }),
            GvRow::VoiceOut => ("Who speaks".into(), c.voice_out.label().to_string()),
            GvRow::SessionLen => ("Session length".into(), format!("A session ends after {} seconds.", c.max_session_secs)),
            GvRow::Quiet => ("Quiet time".into(), format!("The microphone closes after {} seconds without speech.", c.idle_secs)),
            GvRow::DayCap => ("Daily cap of microphone time".into(), format!("{} seconds a day sent to Google (your own cap; Google's free limit is told plainly when reached).", c.day_cap_secs)),
            GvRow::SpokenCap => ("Spoken reply".into(), format!("At most {} characters are read aloud; the rest stays on the screen.", c.spoken_cap_chars)),
            GvRow::Shortcut => ("Start key".into(), format!("{} starts a session; Esc ends it. Press K for the next name.", c.key)),
            GvRow::Wake => ("Wake phrase hand-off (experimental, opt-in)".into(), if c.wake_handoff { "On: the local wake phrase may open a session. Nothing is sent before it is heard.".into() } else { "Off (the default).".into() }),
            GvRow::Stop => ("Stop now".into(), "Closes the microphone at once and ends any open session.".into()),
        }
    }

    pub(super) fn render_geminivoice(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let sel = self.sel().min(ROWS.len() - 1);
        let rh = 62.0 * s;
        let mut y = a.y;
        let head = g.fit(14.0 * s, "EXPERIMENTAL: off by default. While a session is open your microphone goes to Google.", Face::Bold, a.w);
        g.text(a.x, y, 14.0 * s, MUTED, &head, Face::Bold);
        y += 26.0 * s;
        // the rows that fit; the window follows the selected row, and one row is always drawn (the largest text size on a small window)
        let fit = (((a.y + a.h - y) / rh) as usize).max(1);
        let first = if sel >= fit { sel + 1 - fit } else { 0 };
        for (i, row) in ROWS.iter().enumerate().skip(first) {
            if i > first && y + rh > a.y + a.h {
                break;
            }
            let (title, detail) = self.gv_row_text(cx, *row);
            let r = R::new(a.x, y, a.w, rh - 4.0 * s);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &title);
            let tw = r.w - 24.0 * s;
            let t = g.fit(15.0 * s, &title, Face::Bold, tw);
            g.text(r.x + 12.0 * s, y + 4.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
            // the detail may be long: two lines at most, each fitted
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
