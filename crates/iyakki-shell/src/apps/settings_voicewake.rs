//! Settings -> Voice: the wake phrase switch ("My IYAKKI", opt-in, off by default) and its sensitivity.
//!
//! `M` switches the wake phrase on or off, `N` cycles low, normal and high. Only an administrator changes either (it decides when this
//! computer listens). Turning it on needs My IYAKKI to be on and a trained model file; without one the page says why and the switch
//! stays off. On and off are written to the audit log (never any sound). The listening itself is the shell's (`shell_wake.rs`).

use super::*;
use iyakki_agent::wakelisten as wl;

impl SettingsApp {
    pub(super) fn vo_wake_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !svc.voice_settings().my_wake;
        match wl::set_opt_in(&svc, on) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "wake-on" } else { "wake-off" });
                self.ok(if on {
                    "The wake phrase is on: this computer listens for \"My IYAKKI\" on every screen while My IYAKKI is on and not muted. A red sign shows while the microphone is open; sound stays in memory and is never saved or sent."
                } else {
                    "The wake phrase is off. Push to talk still works."
                });
            }
            Err(why) => self.fail(why),
        }
    }

    pub(super) fn vo_wake_sens(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let next = (svc.voice_settings().my_wake_sens.min(2) + 1) % 3;
        match wl::set_sensitivity(&svc, next) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.assistant", "wake-sensitivity");
                self.ok(format!("Wake phrase sensitivity: {}.", wl::SENSITIVITY[next as usize]));
            }
            Err(e) => self.fail(e),
        }
    }

    /// The two rows of the page: the switch with its reason, and the sensitivity.
    pub(super) fn vo_wake_rows(cx: &AppCx) -> [(&'static str, String, [u8; 4]); 2] {
        let svc = Self::svc(cx);
        let v = svc.voice_settings();
        let model = wl::no_model_text(&wl::load_model(svc.dir()));
        let state = if v.my_wake {
            if !v.my_on {
                "on, but My IYAKKI is off, so nothing listens (M switches off)".to_string()
            } else if v.my_muted {
                "on, but the microphone is muted, so nothing listens (M switches off)".to_string()
            } else if let Some(why) = &model {
                format!("on, but {why} (M switches off)")
            } else {
                "on: the microphone listens for \"My IYAKKI\" on every screen, a red sign shows while it does; sound stays in memory (M switches off)".to_string()
            }
        } else if let Some(why) = model {
            format!("off: {why}")
        } else if !v.my_on {
            "off: turn on My IYAKKI first (Voice tile, Assistant tab), then press M".to_string()
        } else {
            "off (opt-in): press M to let this computer listen for \"My IYAKKI\"; not tested on real voices yet".to_string()
        };
        [("Wake phrase", state, if v.my_wake { OK } else { MUTED }), ("Wake sensitivity", format!("{} (N changes: low, normal, high)", wl::SENSITIVITY[v.my_wake_sens.min(2) as usize]), TEXT)]
    }
}
