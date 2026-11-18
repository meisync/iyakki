//! Settings -> Voice: the wake phrase switch ("My IYAKKI", opt-in, off by default) and its sensitivity.
//!
//! `M` switches the wake phrase on or off, `N` cycles low, normal and high. Only an administrator changes either (it decides when this
//! computer listens). Turning it on needs My IYAKKI to be on and a trained model file; without one the page says why and the switch
//! stays off. On and off are written to the audit log (never any sound). The listening itself is the shell's (`shell_wake.rs`).

use super::*;
use iyakki_agent_api::ServicesB1;
use iyakki_shared::wakelisten as wl;

impl SettingsApp {
    pub(super) fn vo_wake_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !cx.voice_settings().my_wake;
        match svc.wake_set_opt_in(on) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "wake-on" } else { "wake-off" });
                self.ok(if on {
                    "The wake phrase (experimental) is on: this computer listens for \"My IYAKKI\" on every screen while My IYAKKI is on and not muted. It may miss you or wake by mistake. A red sign shows while the microphone is open; sound stays in memory and is never saved or sent."
                } else {
                    "The wake phrase is off. Push to talk still works."
                });
            }
            Err(why) => self.fail(why),
        }
    }

    pub(super) fn vo_wake_sens(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let next = (cx.voice_settings().my_wake_sens.min(2) + 1) % 3;
        match svc.wake_set_sensitivity(next) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.assistant", "wake-sensitivity");
                self.ok(format!("Wake phrase sensitivity: {}.", wl::SENSITIVITY[next as usize]));
            }
            Err(e) => self.fail(e),
        }
    }

    /// `T`: the experimental second check of the wake phrase (the speech-to-text engine must hear the words too). Off by default.
    pub(super) fn vo_wake_confirm_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !cx.voice_settings().my_wake_confirm;
        let r = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_wake_confirm = on).map_err(|e| e.to_string());
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "wake-confirm-on" } else { "wake-confirm-off" });
                self.ok(if on {
                    "The second check of the wake phrase (experimental) is on: the short clip that made the spotter fire goes to your speech-to-text engine, stays in memory and is dropped at once; the wake is dropped when the words are not there. If the engine is on another machine you have not agreed to, the check is skipped and the wake is taken."
                } else {
                    "The second check of the wake phrase is off."
                });
            }
            Err(e) => self.fail(e),
        }
    }

    /// The row of the page for the second check and the counters.
    pub(super) fn vo_wake_confirm_row(cx: &AppCx) -> (&'static str, String, [u8; 4]) {
        let on = cx.voice_settings().my_wake_confirm;
        let counts = Self::svc(cx).wake_stats_line();
        let text = if on { format!("on (experimental): the speech-to-text engine must hear the words too (T switches off). {counts}") } else { format!("off (experimental): press T to let the speech-to-text engine check the words. {counts}") };
        ("Wake second check", text, if on { ACCENT } else { MUTED })
    }

    /// `L`: allow this device to send microphone sound to a cloud machine for My IYAKKI (F-VOICE-38). Off by default; a card asks in every session.
    pub(super) fn vo_cloud_stream_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !cx.voice_settings().my_cloud_stream;
        let r = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_cloud_stream = on).map_err(|e| e.to_string());
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.cloud_stream", if on { "allowed" } else { "off" });
                self.ok(if on { "Sending your microphone sound to a cloud machine is allowed. A card still asks in every session before any sound leaves this device; it travels over a secure link and is never kept." } else { "Sending your microphone sound to a cloud machine is off." });
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn vo_cloud_stream_row(cx: &AppCx) -> (&'static str, String, [u8; 4]) {
        let on = cx.voice_settings().my_cloud_stream;
        let text = if on { "allowed: a card asks in every session before microphone sound goes to a cloud machine (L switches off)" } else { "off: no microphone sound goes to a cloud machine (L allows it; a card still asks every session)" };
        ("Sound to a cloud machine", text.to_string(), if on { ACCENT } else { OK })
    }

    pub(super) fn vo_spoken_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !cx.voice_settings().spoken_approvals;
        let r = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.spoken_approvals = on).map_err(|e| e.to_string());
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "spoken-approvals-on" } else { "spoken-approvals-off" });
                self.ok(if on {
                    "Spoken approvals are on: in a voice session a spoken yes can approve an ordinary ask. Deleting, installing, sending, secrets, the camera, the screen and shells still need a tap or a key."
                } else {
                    "Spoken approvals are off: approvals in a voice session need a tap or a key."
                });
            }
            Err(e) => self.fail(e),
        }
    }

    /// `E`: take the assistant's own voice out of what the microphone heard while it spoke (F-VOICE-36). Off by default.
    pub(super) fn vo_echo_toggle(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        let on = !cx.voice_settings().echo_cancel;
        let r = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.echo_cancel = on).map_err(|e| e.to_string());
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "echo-cancel-on" } else { "echo-cancel-off" });
                self.ok(if on {
                    "Echo cancellation is on: what the microphone heard while the assistant spoke has the assistant's own voice taken out before it is understood. Experimental: it has not been tried on real speakers and microphones."
                } else {
                    "Echo cancellation is off."
                });
            }
            Err(e) => self.fail(e),
        }
    }

    /// The row of the page for echo cancellation.
    pub(super) fn vo_echo_row(cx: &AppCx) -> (&'static str, String, [u8; 4]) {
        let on = cx.voice_settings().echo_cancel;
        let text = if on {
            "on (experimental): the assistant's own voice is taken out of what the microphone heard while it spoke (E switches off)"
        } else {
            "off: press E to take the assistant's own voice out of what the microphone hears while it speaks (experimental)"
        };
        ("Echo cancellation", text.to_string(), if on { ACCENT } else { OK })
    }

    /// The row of the page for spoken approvals.
    pub(super) fn vo_spoken_row(cx: &AppCx) -> (&'static str, String, [u8; 4]) {
        let on = cx.voice_settings().spoken_approvals;
        let text = if on {
            "on: a spoken yes can approve an ordinary ask in a voice session; the hard-ask list always needs a tap or key (A switches off)"
        } else {
            "off: approvals in a voice session need a tap or key; a spoken yes does nothing (A switches on)"
        };
        ("Spoken approvals", text.to_string(), if on { ACCENT } else { OK })
    }

    /// The two rows of the page: the switch with its reason, and the sensitivity.
    pub(super) fn vo_wake_rows(cx: &AppCx) -> [(&'static str, String, [u8; 4]); 2] {
        let svc = Self::svc(cx);
        let v = cx.voice_settings();
        let model = svc.wake_no_model_text();
        let state = if v.my_wake {
            if !v.my_on {
                "on, but My IYAKKI is off, so nothing listens (M switches off)".to_string()
            } else if v.my_muted {
                "on, but the microphone is muted, so nothing listens (M switches off)".to_string()
            } else if let Some(why) = &model {
                format!("on, but {why} (M switches off)")
            } else {
                "on (experimental): the microphone listens for \"My IYAKKI\" on every screen, a red sign shows while it does; sound stays in memory (M switches off)".to_string()
            }
        } else if let Some(why) = model {
            format!("off: {why}")
        } else if !v.my_on {
            "off: turn on My IYAKKI first (Voice tile, Assistant tab), then press M".to_string()
        } else {
            "off (experimental, opt-in): press M to let this computer listen for \"My IYAKKI\"; not tested on real voices yet, it may miss you or wake by mistake".to_string()
        };
        [("Wake phrase (experimental)", state, if v.my_wake { OK } else { MUTED }), ("Wake sensitivity", format!("{} (N changes: low, normal, high)", wl::SENSITIVITY[v.my_wake_sens.min(2) as usize]), TEXT)]
    }
}
