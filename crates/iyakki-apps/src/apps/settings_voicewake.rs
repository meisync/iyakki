//! Settings -> Voice: the wake phrase switch ("My IYAKKI", opt-in, off by default) and its sensitivity.
//!
//! `M` switches the wake phrase on or off, `N` cycles low, normal and high. Only an administrator changes either (it decides when this
//! computer listens). Turning it on needs My IYAKKI to be on and a trained model file; without one the page says why and the switch
//! stays off. On and off are written to the audit log (never any sound). The listening itself is the shell's (`shell_wake.rs`).

use super::*;
use super::apps_port::VoicePort;
use iyakki_shared::wakelisten as wl;

impl SettingsApp {
    pub(super) fn vo_wake_toggle(&mut self, p: &dyn VoicePort) {
        let svc = p.voice_ops();
        let on = !p.voice_ops().voice_settings().my_wake;
        match svc.wake_set_opt_in(on) {
            Ok(_) => {
                p.audit("voice.assistant", if on { "wake-on" } else { "wake-off" });
                self.ok(if on {
                    "The wake phrase (experimental) is on: this computer listens for \"My IYAKKI\" on every screen while My IYAKKI is on and not muted. It may miss you or wake by mistake. A red sign shows while the microphone is open; sound stays in memory and is never saved or sent."
                } else {
                    "The wake phrase is off. Push to talk still works."
                });
            }
            Err(why) => self.fail(why),
        }
    }

    pub(super) fn vo_wake_sens(&mut self, p: &dyn VoicePort) {
        let svc = p.voice_ops();
        let next = (p.voice_ops().voice_settings().my_wake_sens.min(2) + 1) % 3;
        match svc.wake_set_sensitivity(next) {
            Ok(()) => {
                p.audit("voice.assistant", "wake-sensitivity");
                self.ok(format!("Wake phrase sensitivity: {}.", wl::SENSITIVITY[next as usize]));
            }
            Err(e) => self.fail(e),
        }
    }

    /// `T`: the experimental second check of the wake phrase (the speech-to-text engine must hear the words too). Off by default.
    pub(super) fn vo_wake_confirm_toggle(&mut self, p: &dyn VoicePort) {
        let _svc = p.voice_ops().clone();
        let on = !p.voice_ops().voice_settings().my_wake_confirm;
        let r = p.settings_update(&mut |s| s.voice.my_wake_confirm = on).map_err(|e| e.message());
        match r {
            Ok(()) => {
                p.audit("voice.assistant", if on { "wake-confirm-on" } else { "wake-confirm-off" });
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
    pub(super) fn vo_wake_confirm_row(p: &dyn VoicePort) -> (&'static str, String, [u8; 4]) {
        let on = p.voice_ops().voice_settings().my_wake_confirm;
        let counts = p.voice_ops().wake_stats_line();
        let text = if on { format!("on (experimental): the speech-to-text engine must hear the words too (T switches off). {counts}") } else { format!("off (experimental): press T to let the speech-to-text engine check the words. {counts}") };
        ("Wake second check", text, if on { ACCENT } else { MUTED })
    }

    /// `L`: allow this device to send microphone sound to a cloud machine for My IYAKKI (F-VOICE-38). Off by default; a card asks in every session.
    pub(super) fn vo_cloud_stream_toggle(&mut self, p: &dyn VoicePort) {
        let _svc = p.voice_ops().clone();
        let on = !p.voice_ops().voice_settings().my_cloud_stream;
        let r = p.settings_update(&mut |s| s.voice.my_cloud_stream = on).map_err(|e| e.message());
        match r {
            Ok(()) => {
                p.audit("voice.cloud_stream", if on { "allowed" } else { "off" });
                self.ok(if on { "Sending your microphone sound to a cloud machine is allowed. A card still asks in every session before any sound leaves this device; it travels over a secure link and is never kept." } else { "Sending your microphone sound to a cloud machine is off." });
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn vo_cloud_stream_row(p: &dyn VoicePort) -> (&'static str, String, [u8; 4]) {
        let on = p.voice_ops().voice_settings().my_cloud_stream;
        let text = if on { "allowed: a card asks in every session before microphone sound goes to a cloud machine (L switches off)" } else { "off: no microphone sound goes to a cloud machine (L allows it; a card still asks every session)" };
        ("Sound to a cloud machine", text.to_string(), if on { ACCENT } else { OK })
    }

    pub(super) fn vo_spoken_toggle(&mut self, p: &dyn VoicePort) {
        let _svc = p.voice_ops().clone();
        let on = !p.voice_ops().voice_settings().spoken_approvals;
        let r = p.settings_update(&mut |s| s.voice.spoken_approvals = on).map_err(|e| e.message());
        match r {
            Ok(()) => {
                p.audit("voice.assistant", if on { "spoken-approvals-on" } else { "spoken-approvals-off" });
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
    pub(super) fn vo_echo_toggle(&mut self, p: &dyn VoicePort) {
        let _svc = p.voice_ops().clone();
        let on = !p.voice_ops().voice_settings().echo_cancel;
        let r = p.settings_update(&mut |s| s.voice.echo_cancel = on).map_err(|e| e.message());
        match r {
            Ok(()) => {
                p.audit("voice.assistant", if on { "echo-cancel-on" } else { "echo-cancel-off" });
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
    pub(super) fn vo_echo_row(p: &dyn VoicePort) -> (&'static str, String, [u8; 4]) {
        let on = p.voice_ops().voice_settings().echo_cancel;
        let text = if on {
            "on (experimental): the assistant's own voice is taken out of what the microphone heard while it spoke (E switches off)"
        } else {
            "off: press E to take the assistant's own voice out of what the microphone hears while it speaks (experimental)"
        };
        ("Echo cancellation", text.to_string(), if on { ACCENT } else { OK })
    }

    /// The row of the page for spoken approvals.
    pub(super) fn vo_spoken_row(p: &dyn VoicePort) -> (&'static str, String, [u8; 4]) {
        let on = p.voice_ops().voice_settings().spoken_approvals;
        let text = if on {
            "on: a spoken yes can approve an ordinary ask in a voice session; the hard-ask list always needs a tap or key (A switches off)"
        } else {
            "off: approvals in a voice session need a tap or key; a spoken yes does nothing (A switches on)"
        };
        ("Spoken approvals", text.to_string(), if on { ACCENT } else { OK })
    }

    /// The state text while the phrase is on and nothing else stops it. `paused` is `wakeshare::why_paused()` (F-VOICE-60,
    /// D-WAKE-SHARED-MIC): the listener gives way while a page or the Voice tile has the microphone, and Settings says why.
    pub(super) fn vo_wake_listening_text(paused: Option<&str>) -> String {
        match paused {
            Some(why) => format!("on (experimental), now paused: {} It listens again by itself (M switches off)", why.trim_start_matches("Wake phrase paused: ")),
            None => "on (experimental): the microphone listens for \"My IYAKKI\" on every screen, a red sign shows while it does; sound stays in memory (M switches off)".to_string(),
        }
    }

    /// The two rows of the page: the switch with its reason, and the sensitivity.
    pub(super) fn vo_wake_rows(p: &dyn VoicePort) -> [(&'static str, String, [u8; 4]); 2] {
        let svc = p.voice_ops();
        let v = p.voice_ops().voice_settings();
        let model = svc.wake_no_model_text();
        let state = if v.my_wake {
            if !v.my_on {
                "on, but My IYAKKI is off, so nothing listens (M switches off)".to_string()
            } else if v.my_muted {
                "on, but the microphone is muted, so nothing listens (M switches off)".to_string()
            } else if let Some(why) = &model {
                format!("on, but {why} (M switches off)")
            } else {
                Self::vo_wake_listening_text(iyakki_shared::wakeshare::why_paused())
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

#[cfg(test)]
mod wake_paused_tests {
    use super::*;
    type S = SettingsApp;
    #[test]
    fn the_wake_row_says_why_the_listener_is_paused_and_that_it_comes_back() {
        let t = S::vo_wake_listening_text(Some("Wake phrase paused: a page is using the microphone."));
        assert!(t.contains("now paused: a page is using the microphone.") && t.contains("listens again by itself"), "{t}");
        assert!(!t.contains("Wake phrase paused"), "{t}");
        assert!(S::vo_wake_listening_text(None).contains("a red sign shows"));
    }
}
