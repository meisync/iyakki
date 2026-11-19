//! The wake phrase "My IYAKKI" on every screen: the shell owns the listener (`iyakki_agent::wakelisten`), so it listens whatever app is
//! open, not only while the Voice tile is. Strictly opt-in (Settings -> Voice, or the chat tool with its approval card); the listener
//! exists only while the person is signed in, may use voice, is on this machine's own microphone (not a browser's), and the saved
//! settings say wake phrase on, My IYAKKI on, not muted, with a usable model file.
//!
//! * The red sign (bottom left, stacked with the camera and recording signs) is drawn on every screen exactly while the microphone is
//!   open to the spotter; a click on it switches the wake phrase off (and audits it).
//! * While the Voice tile is open it has the microphone (push to talk), and while the screen is locked nothing listens: the listener
//!   is paused and has closed the device.
//! * A detection chimes (the Notify cue), then opens the Voice tile on the Assistant tab and taps Talk, so the existing session runs with its follow-up window
//!   rules unchanged. The audit log gets one line for the detection (no sound, no text).

use crate::stale_stats::mark_stale;
use super::*;
use iyakki_agent_api::{WakeHandle, WakeListenerBox};

pub(super) type ListenerMaker = Arc<dyn Fn() -> WakeListenerBox + Send + Sync>;

#[derive(Default)]
pub(super) struct WakeUi {
    handle: Option<Box<dyn WakeHandle>>,
    /// Tick of the last time a missing or damaged model file was looked at again.
    looked: u64,
    /// The microphone is open to the spotter (what the sign shows).
    pub sign: bool,
    /// Tests: a listener with a fake microphone and spotter instead of the machine's.
    pub listener_maker: Option<ListenerMaker>,
    /// Why the wake phrase is on but the microphone is not open (shown by the status line and the log), empty when fine.
    pub note: String,
}

impl Shell {
    /// Called on every heartbeat: start or stop the listener to match the settings, tell it what the screen needs, collect a detection.
    pub(super) fn wake_tick(&mut self) {
        let eligible = match &self.session {
            Some(s) if s.policy.voice && !iyakki_apps::agent().browser_source().browser_session_active(&s.user) => {
                let v = self.env.voice_settings();
                v.my_wake && v.my_on
            }
            _ => false,
        };
        if !eligible {
            if self.wake.handle.take().is_some() || self.wake.sign {
                self.wake.sign = false;
                self.wake.note.clear();
                mark_stale!(self);
            }
            return;
        }
        if self.wake.handle.is_none() && (self.wake.looked == 0 || self.tick.saturating_sub(self.wake.looked) >= 75) {
            self.wake.looked = self.tick.max(1);
            match self.env.services.wake_spawn(self.wake.listener_maker.as_ref().map(|m| m())) {
                Ok(h) => {
                    self.wake.handle = Some(h);
                    self.wake.note.clear();
                }
                Err(text) => self.wake.note = text,
            }
        }
        let paused = matches!(self.screen, Screen::App(_, PApp::Voice)) || self.is_locked();
        let Some(h) = &self.wake.handle else { return };
        h.set_paused(paused);
        let on = h.indicator();
        if on != self.wake.sign {
            self.wake.sign = on;
            mark_stale!(self);
        }
        let note = h.note();
        if note != self.wake.note {
            self.wake.note = note;
        }
        if h.take_wake() {
            if let Some(s) = &self.session {
                self.env.audit.record(&s.user, "voice.wake", "heard");
            }
            self.wake.sign = false;
            // the opt-in hand-off to the Gemini voice assistant (EXPERIMENTAL): the local spotter heard the phrase and closed its
            // microphone; a chime plays and the session opens (nothing was sent before this point)
            if self.gv_wake_handoff() {
                mark_stale!(self);
                return;
            }
            // a short chime says "I heard you" (the Notify cue, the person's own System sounds choices apply); it ends before the microphone opens again
            crate::cues::chime_and_wait(&*self.core, &self.cloud);
            iyakki_apps::agent().wake_request_session();
            if !matches!(self.screen, Screen::App(_, PApp::Voice)) {
                self.open(PApp::Voice);
                if !matches!(self.screen, Screen::App(_, PApp::Voice)) {
                    let _ = iyakki_apps::agent().wakelisten_take_session_request(); // the tile could not open (role, cloud mode, memory): no stale tap later
                }
            }
            mark_stale!(self);
        }
    }

    /// The sign's click: switch the wake phrase off.
    pub(super) fn wake_sign_click(&mut self) {
        if let Some(s) = &self.session {
            let _ = self.env.services.wake_set_opt_in(false);
            self.env.audit.record(&s.user, "voice.assistant", "wake-off");
        }
        self.wake.handle = None;
        self.wake.sign = false;
        mark_stale!(self);
    }
}

/// The red sign: "Listening for My IYAKKI" with the microphone open. Returns its rectangle (the click target).
pub(crate) fn draw_wake_sign(g: &mut Gfx, h: u32, s: f32, rows_below: usize) -> R {
    let text = "Microphone open: listening for \"My IYAKKI\". Click to switch off.";
    let tw = g.text_width(15.0 * s, text, Face::Bold) + 48.0 * s;
    let y = h as f32 - 64.0 * s - rows_below as f32 * 44.0 * s;
    let r = R::new(16.0 * s, y, tw, 34.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 8.0 * s, [255, 40, 50, 130]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(110, 12, 22));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 2.0 * s, rgb(255, 90, 100));
    g.circle(r.x + 18.0 * s, r.y + r.h / 2.0, 7.0 * s, rgb(255, 60, 70));
    g.text(r.x + 34.0 * s, r.y + 8.0 * s, 15.0 * s, TEXT, text, Face::Bold);
    r
}
