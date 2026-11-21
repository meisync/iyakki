//! The Gemini voice assistant on every screen (EXPERIMENTAL, off by default; docs/GEMINI-PROVIDER.md): the start key (Super+V by
//! default, `FnKey::VoiceAssist`), a microphone button, Esc to end, the cards of the tools that ask (and the speaker question), and
//! the session line. The session itself runs on its own thread (`iyakki_apps::apps::gemvoice`, `iyakki_agent::geminilive_host`).
//!
//! * The red sign is not drawn here: the open microphone is in `clientmedia::indicator()`, which the shell's one red band already shows.
//! * A card is answered only by a press on its Allow or Deny (`Event::Tab`); nothing the person says can approve anything.
//! * Nothing starts unless the assistant is switched on (Settings -> Gemini voice), the person may use voice and is signed in. The
//!   session itself still needs the person's yes and a key; when it cannot start the one line it gives is shown here and the local
//!   voice answers instead.

use crate::stale_stats::mark_stale;
use super::*;
use iyakki_shared::geminilive_host::{Board, RunState, Running};

/// Click regions.
pub const ALLOW: usize = 1_361_001;
pub const DENY: usize = 1_361_002;
pub const STOP: usize = 1_361_003;
pub const MIC: usize = 1_361_004;
pub const DONE: usize = 1_361_005;

/// How long the start key is kept down, in ticks (about 30 a second), for its release to mean press-to-talk.
pub const HOLD_TICKS: u64 = 15;

pub fn is_click(n: usize) -> bool {
    (ALLOW..=DONE).contains(&n)
}

pub struct GvUi {
    pub board: Arc<Board>,
    pub running: Option<Running>,
    /// The last state shown, so a change is said once.
    shown: Option<RunState>,
    /// The cards on the board at the last look (a change redraws).
    cards: usize,
    /// Whether the key and the button are offered (read every couple of seconds).
    pub offered: bool,
    pub(super) looked: u64,
    /// The press of the start key that began a session: the tick it went down at. Its release after a hold hands the turn over.
    held: Option<u64>,
}

impl Default for GvUi {
    fn default() -> GvUi {
        GvUi { board: Board::new(), running: None, shown: None, cards: 0, offered: false, looked: 0, held: None }
    }
}

impl GvUi {
    pub fn open(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.finished())
    }
}

impl Shell {
    /// On every heartbeat: follow the settings (is the key offered, which key), tell the board whether the person is at the screen,
    /// say what the session did, redraw when a card comes or goes.
    pub(super) fn gv_tick(&mut self) {
        let Some(cx) = self.cx() else {
            self.gv.board.set_present(false);
            if self.gv.running.is_some() {
                // signed out: the session ends with the person
                iyakki_apps::agent().geminilive_request_stop();
            }
            return;
        };
        self.gv.board.set_present(!self.is_locked());
        if self.gv.looked == 0 || self.tick.saturating_sub(self.gv.looked) >= 60 {
            self.gv.looked = self.tick.max(1);
            let offered = iyakki_apps::apps::gemvoice::offered(&cx);
            if offered != self.gv.offered {
                self.gv.offered = offered;
                mark_stale!(self);
            }
            let name = iyakki_apps::apps::gemvoice::key_name(&cx);
            iyakki_apps::input::set_voice_chord(if offered { Some(name.as_str()) } else { None });
        }
        let current = self.gv.running.as_ref().map(|r| (r.state(), r.finished()));
        if let Some((st, finished)) = current {
            if self.gv.shown.as_ref() != Some(&st) {
                self.gv.shown = Some(st.clone());
                match &st {
                    RunState::Fallback(line) => self.say(line.clone()),
                    RunState::Ended(line) => self.say_info(line.clone()),
                    RunState::Open => self.say_info("Gemini voice (EXPERIMENTAL): the microphone is open. Esc ends it."),
                    RunState::Connecting => {}
                }
                mark_stale!(self);
            }
            if finished {
                self.gv.running = None;
                self.gv.board.clear();
                mark_stale!(self);
            }
        }
        let n = self.gv.board.pending().len();
        if n != self.gv.cards {
            self.gv.cards = n;
            mark_stale!(self);
        }
    }

    /// The start key went down. A tap starts (or ends) a session as it always did; the key kept down after it started one is press-to-talk:
    /// its release hands the turn over (`gv_release`).
    pub(super) fn gv_press(&mut self) {
        let was_open = self.gv.open();
        self.gv_toggle();
        self.gv.held = (!was_open && self.gv.open()).then_some(self.tick);
    }

    /// The start key was let go. Held for at least `HOLD_TICKS` (about half a second) after it started the session: "done talking", the turn
    /// is handed over and the session ends after the answer. A tap does nothing here: the session stays open until the key, the button or
    /// Esc ends it. Hands-free end of speech is a different row (the level gate).
    pub(super) fn gv_release(&mut self) {
        let Some(at) = self.gv.held.take() else { return };
        if self.tick.saturating_sub(at) >= HOLD_TICKS && self.gv.open() {
            self.gv_done();
        }
    }

    /// The start key or the microphone button: start a session, or end the open one.
    pub(super) fn gv_toggle(&mut self) {
        let Some(cx) = self.cx() else { return };
        if self.gv.open() {
            return self.gv_stop();
        }
        if !iyakki_apps::apps::gemvoice::offered(&cx) {
            return self.say_info("The Gemini voice assistant (EXPERIMENTAL) is off. Turn it on in Settings -> Gemini voice.");
        }
        if self.is_locked() {
            return;
        }
        self.gv.shown = None;
        self.gv.running = Some(iyakki_apps::apps::gemvoice::start(&cx, self.gv.board.clone()));
        self.say_info("Gemini voice (EXPERIMENTAL): connecting. Esc ends it.");
        mark_stale!(self);
    }

    /// The wake phrase was heard: when the person opted in to the hand-off (and the assistant is on), chime and open a session.
    /// Returns whether it took the wake (else the Voice tile gets it as before).
    pub(super) fn gv_wake_handoff(&mut self) -> bool {
        let Some(cx) = self.cx() else { return false };
        if self.gv.open() || !iyakki_apps::apps::gemvoice::offered(&cx) || !cx.agent.b0().gemini().voice_wake_handoff(cx.data_dir()) {
            return false;
        }
        let _ = crate::cues::start(&*self.core, &self.cloud, iyakki_core_api::soundtypes::Cue::Notify);
        self.env.audit.record(&cx.session.user, "voice.session", "opened by the wake phrase (EXPERIMENTAL hand-off)");
        self.gv_toggle();
        true
    }

    pub(super) fn gv_stop(&mut self) {
        if let Some(r) = &self.gv.running {
            r.stop();
        }
        mark_stale!(self);
    }

    /// "I am done talking": the turn is handed over and the session ends after the answer.
    pub(super) fn gv_done(&mut self) {
        if let Some(r) = &self.gv.running {
            r.finish_turn();
        }
    }

    pub(super) fn gv_event(&mut self, id: usize) {
        match id {
            MIC => self.gv_toggle(),
            STOP => self.gv_stop(),
            DONE => self.gv_done(),
            ALLOW | DENY => {
                let Some(card) = self.gv.board.pending().into_iter().next() else { return };
                self.gv.board.press(&card.id, id == ALLOW);
                if let Some(s) = &self.session {
                    self.env.audit.record(&s.user, "voice.card", &format!("{} {}", card.name, if id == ALLOW { "allowed" } else { "denied" }));
                }
            }
            _ => {}
        }
        mark_stale!(self);
    }
}

/// Draws the card of the oldest tool that waits (Allow, Deny), the session line (Done talking, Stop) and the microphone button.
/// `row` is the number of signs stacked at the bottom left (the card sits above them, centred).
/// `in_bar`: the top bar carries the microphone button (the corner one is left out then).
pub(crate) fn render(ui: &GvUi, g: &mut Gfx, w: u32, h: u32, s: f32, in_bar: bool) {
    let (wf, hf) = (w as f32, h as f32);
    let size = 14.0 * s;
    if let Some(card) = ui.board.pending().into_iter().next() {
        let cw = (wf - 32.0 * s).min(560.0 * s);
        let lines = g.wrap(size, &card.summary, Face::Sans, cw - 32.0 * s);
        let shown: Vec<&String> = lines.iter().take(4).collect();
        let lh = g.line_height(size, Face::Sans);
        let ch = 26.0 * s + 22.0 * s + lh * shown.len() as f32 + 22.0 * s + 40.0 * s;
        let r = R::new((wf - cw) / 2.0, 64.0 * s, cw, ch);
        g.hit(r, Event::Tick, "Gemini voice: a tool asks");
        g.rrect_glow(r.x, r.y, r.w, r.h, 14.0 * s, 10.0 * s, [255, 190, 60, 90]);
        g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, rgb(34, 30, 40));
        g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 1.6 * s, rgb(255, 190, 60));
        let head = g.fit(size, "Gemini voice (EXPERIMENTAL) asks. Only your press answers.", Face::Bold, cw - 32.0 * s);
        g.text(r.x + 16.0 * s, r.y + 12.0 * s, size, TEXT, &head, Face::Bold);
        let mut y = r.y + 12.0 * s + lh + 10.0 * s;
        for l in shown {
            g.text(r.x + 16.0 * s, y, size, MUTED, l, Face::Sans);
            y += lh;
        }
        let by = r.y + r.h - 16.0 * s - size * 2.1;
        let bw = g.button(r.x + 16.0 * s, by, size, "Allow", ACCENT_MAIN, Event::Tab(ALLOW));
        g.button(r.x + 16.0 * s + bw + 12.0 * s, by, size, "Deny", ERR, Event::Tab(DENY));
        return;
    }
    if ui.open() {
        let text = "Gemini voice (EXPERIMENTAL)";
        let tw = g.text_width(size, text, Face::Bold) + 24.0 * s;
        let bh = size * 2.1;
        let x = wf - 16.0 * s - tw - 2.0 * (g.text_width(size, "Done talking", Face::Bold) + size * 1.7) - 24.0 * s;
        let y = hf - 56.0 * s - bh;
        g.text(x.max(8.0 * s), y + (bh - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, text, Face::Bold);
        let bx = x.max(8.0 * s) + tw;
        let bw = g.button(bx, y, size, "Done talking", ACCENT_MAIN, Event::Tab(DONE));
        g.button(bx + bw + 12.0 * s, y, size, "Stop (Esc)", ERR, Event::Tab(STOP));
    } else if ui.offered && !in_bar {
        let label = "Mic";
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        g.button(wf - 16.0 * s - bw, hf - 56.0 * s - size * 2.1, size, label, MUTED, Event::Tab(MIC));
    }
}
