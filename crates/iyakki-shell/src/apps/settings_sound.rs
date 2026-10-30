//! Settings → Sound: which output IYAKKI plays on (Automatic, or one device such as "HDMI: PDM-15T"), a test chime on the
//! chosen output, and IYAKKI's own volume and mute.
//!
//! The choice is the machine's (administrators) and is kept in the settings file; Automatic follows what is connected
//! (`iyakki_agent::audioout`). The volume is the output's own: a software volume (ALSA softvol, `iyakki_agent::softvol`)
//! when the output has no hardware volume (HDMI on Intel, the T2 Mac), which every program follows; else it is applied by
//! IYAKKI's players. Each output keeps its last volume. The
//! rows come from `devices::sound_rows`; the card mixers below them keep working as before.

use crate::safeclamp::SafeClamp;
use super::*;
use iyakki_agent::audioout;

impl SettingsApp {
    /// Keys of the Sound page about the output and IYAKKI's volume. True when the key was taken here.
    pub(super) fn sound_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        self.hw.set_sound(Self::svc(cx).sound_settings());
        let target = self.selected_target();
        match k {
            Key::Char('o') => self.sound_choose(cx),
            Key::Char('t') => self.sound_test(cx, target),
            Key::Char('A') => self.sound_advanced(),
            Key::Char('M') => self.sound_mixer(cx),
            Key::Char('W') => self.sound_web_mix(cx),
            Key::Enter | Key::Char(' ' | 'e') => match target {
                Some(Target::SoundChoice) => self.sound_choose(cx),
                Some(Target::SoundOut(key, _)) => self.sound_set_output(cx, &key),
                Some(Target::SoftVolume) => self.sound_volume(cx, None),
                Some(Target::SoundAdvanced) => self.sound_advanced(),
                Some(Target::SoundMixer) => self.sound_mixer(cx),
                Some(Target::SoundWebMix) => self.sound_web_mix(cx),
                Some(Target::SoundHighlightOffset) => self.sound_set_highlight_offset(cx, 0),
                Some(Target::SoundVolMode(card_id)) => self.sound_cycle_mode(cx, &card_id),
                Some(Target::SoundHwCheck(card, device, control, card_id)) => self.sound_hw_check(cx, card, device, control, card_id),
                Some(Target::SoundPattern(walk)) => self.sound_pattern(cx, walk),
                _ => return false,
            },
            // + − on the boost row move the boost
            Key::Char('+' | '=' | '-') if matches!(target, Some(Target::SoundBoost(_))) => {
                if let Some(Target::SoundBoost(card_id)) = target {
                    let cur = Self::svc(cx).sound_settings().boost_for(&card_id) as i32;
                    let next = (cur + if matches!(k, Key::Char('-')) { -1 } else { 1 }).clamp_safe(0, iyakki_agent::softvol::MAX_BOOST_DB as i32);
                    self.sound_set_boost(cx, &card_id, next as u32);
                }
            }
            // + − on the highlight offset row move it by 25 ms
            Key::Char('+' | '=' | '-') if matches!(target, Some(Target::SoundHighlightOffset)) => {
                let cur = Self::svc(cx).sound_settings().highlight_offset_ms;
                self.sound_set_highlight_offset(cx, cur + if matches!(k, Key::Char('-')) { -25 } else { 25 });
            }
            // + − m change IYAKKI's volume everywhere but on a card's own mixer row
            Key::Char('+' | '=' | '-' | 'm') if !matches!(target, Some(Target::Card(_))) => {
                let delta = match k {
                    Key::Char('+' | '=') => Some(5),
                    Key::Char('-') => Some(-5),
                    _ => None,
                };
                self.sound_volume(cx, delta)
            }
            _ => return false,
        }
        true
    }

    /// The list of outputs to choose from.
    fn sound_choose(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the sound output.");
        }
        let cur = Self::svc(cx).sound_settings().output;
        let mark = |on: bool| if on { "  ✓" } else { "" };
        let mut items = vec![format!("Automatic (follows what is connected){}", mark(cur.is_empty()))];
        items.extend(self.hw.outputs(self.ticks).iter().map(|o| format!("{} · {}  [{}]{}", o.label(), o.state(), o.key(), mark(o.key() == cur))));
        self.modal = Some(Modal::Pick(PickKind::SoundOutput, Picker::new("Sound output", items)));
    }

    /// A line of the list above was picked.
    pub(super) fn sound_pick(&mut self, cx: &AppCx, item: &str) {
        let key = if item.starts_with("Automatic") { String::new() } else { item.rsplit_once('[').and_then(|(_, r)| r.split_once(']')).map(|(k, _)| k.to_string()).unwrap_or_default() };
        if key.is_empty() && !item.starts_with("Automatic") {
            return;
        }
        self.sound_set_output(cx, &key);
    }

    fn sound_set_output(&mut self, cx: &AppCx, key: &str) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the sound output.");
        }
        match Self::svc(cx).update_sound(|s| s.output = key.to_string()) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.output", if key.is_empty() { "automatic" } else { key });
                self.hw.set_sound(now);
                self.hw.invalidate();
                let c = audioout::choose(&self.hw.outputs(self.ticks), key);
                let msg = match (&c.output, key.is_empty()) {
                    (Some(o), true) => format!("Sound output: Automatic, now {}. The next sound plays there.", o.label()),
                    (Some(o), false) => format!("Sound output: {}. The next sound plays there.", o.label()),
                    (None, _) => format!("Sound output: Automatic. {}", c.reason),
                };
                self.ok(msg);
            }
            Err(e) => self.fail(e),
        }
    }

    /// `Some(d)`: louder or quieter by `d` points (louder also unmutes); `None`: mute or unmute.
    fn sound_volume(&mut self, cx: &AppCx, delta: Option<i32>) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the volume.");
        }
        let r = Self::svc(cx).update_sound(|s| match delta {
            Some(d) => {
                s.volume_pct = (s.volume_pct as i32 + d).clamp(0, 100) as u32;
                if d > 0 {
                    s.muted = false;
                }
            }
            None => s.muted = !s.muted,
        });
        self.sound_said(cx, r);
    }

    /// A click on the IYAKKI volume slider's track.
    pub(super) fn sound_set_volume(&mut self, cx: &AppCx, v: u32) {
        if !self.admin(cx) {
            return;
        }
        let r = Self::svc(cx).update_sound(|s| {
            s.volume_pct = v.min(100);
            s.muted = false;
        });
        self.sound_said(cx, r);
    }

    fn sound_said(&mut self, cx: &AppCx, r: Result<audioout::SoundSettings, String>) {
        match r {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.volume", &if now.muted { "muted".to_string() } else { now.volume_pct.to_string() });
                self.ok(if now.muted { "IYAKKI sound muted.".to_string() } else { format!("IYAKKI volume {}%.", now.volume_pct) });
                self.hw.set_sound(now);
            }
            Err(e) => self.fail(e),
        }
    }

    /// `t`: a short gentle chime on the selected output, else on the one in use.
    fn sound_test(&mut self, cx: &AppCx, target: Option<Target>) {
        let svc = Self::svc(cx).clone();
        let prefs = svc.sound_settings();
        let outs = self.hw.outputs(self.ticks);
        let dev = match target {
            Some(Target::SoundOut(_, alsa)) => Some(alsa),
            _ => {
                let c = audioout::choose(&outs, &prefs.output);
                match c.output {
                    // a sound server chooses for itself in automatic mode
                    Some(_) if !c.chosen && self.hw.real_sound() && audioout::sound_server_running() => None,
                    Some(o) => Some(o.alsa()),
                    None if outs.is_empty() => None, // nothing listed (another OS, a sound server): the default device
                    None => return self.fail(c.reason),
                }
            }
        };
        cx.audit.record(&cx.session.user, "settings.sound.test", dev.as_deref().unwrap_or("default"));
        self.start("playing a test sound", move || JobOut::Msg(svc.test_sound(dev.as_deref())));
    }
}

/// Settings -> Sound: the mixer switch. The setting is the machine's, so only administrators change it; the agent applies it
/// (no app has a branch for it).
impl SettingsApp {
    pub(super) fn sound_mixer(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the sound mixer.");
        }
        let on = !Self::svc(cx).sound_settings().mixer;
        match Self::svc(cx).update_sound(|s| s.mixer = on) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.mixer", if on { "on" } else { "off" });
                self.ok(if on { "Sound mixer on: sounds from several apps can play together. It applies from the next sound." } else { "Sound mixer off: one app at a time, as before. It applies from the next sound." });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }
}

/// Settings -> Sound: the switch that sends the web engine's sound into the mixer (`iyakki_agent::webmix`). Administrators only.
impl SettingsApp {
    pub(super) fn sound_web_mix(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the browser's sound route.");
        }
        let on = !Self::svc(cx).sound_settings().web_mix;
        match Self::svc(cx).update_sound(|s| s.web_mix = on) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.web_mix", if on { "on" } else { "off" });
                self.ok(if !on {
                    "Browser sound through the mixer off: the browser opens the sound card itself again from the next web engine start."
                } else if now.mixer {
                    "Browser sound through the mixer on (experimental, untested on real hardware). It applies the next time the web engine starts."
                } else {
                    "Browser sound through the mixer on, but it does nothing until the mixer above is on."
                });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }
}

/// Settings -> Sound -> Advanced: the highlight timing offset (administrators; it is the machine's, like the output).
impl SettingsApp {
    pub(super) fn sound_set_highlight_offset(&mut self, cx: &AppCx, ms: i32) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the highlight timing offset.");
        }
        let ms = ms.clamp_safe(-500, 500);
        match Self::svc(cx).update_sound(|s| s.highlight_offset_ms = ms) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.highlight_offset", &ms.to_string());
                self.ok(format!("Highlight timing offset {ms:+} ms: {}.", if ms == 0 { "the lit word follows the sound as measured" } else if ms > 0 { "the lit word comes later" } else { "the lit word comes earlier" }));
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }
}

/// Settings -> Sound -> Advanced: boost, which volume control, the hardware self-check and the speaker tests.
impl SettingsApp {
    fn sound_advanced(&mut self) {
        let on = self.hw.toggle_advanced();
        self.hw.invalidate();
        self.ok(if on { "Advanced sound rows shown." } else { "Advanced sound rows hidden." });
    }

    pub(super) fn sound_set_boost(&mut self, cx: &AppCx, card_id: &str, db: u32) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the boost.");
        }
        let id = card_id.to_string();
        match Self::svc(cx).update_sound(|s| s.set_boost(&id, db)) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.boost", &format!("{card_id} {db}"));
                self.ok(if db == 0 { "Boost off.".to_string() } else { format!("Boost +{db} dB. Loud sound may distort. It applies from the next sound.") });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    fn sound_cycle_mode(&mut self, cx: &AppCx, card_id: &str) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the volume control.");
        }
        let id = card_id.to_string();
        let next = Self::svc(cx).sound_settings().volume_mode_for(card_id).next();
        match Self::svc(cx).update_sound(|s| s.set_volume_mode(&id, next)) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.volume_control", &format!("{card_id} {}", next.label()));
                self.ok(format!("Volume control: {}.", next.label()));
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    fn sound_hw_check(&mut self, cx: &AppCx, card: u32, device: u32, control: String, card_id: String) {
        if !self.admin(cx) {
            return self.fail("Only administrators can change the volume control.");
        }
        let svc = Self::svc(cx).clone();
        cx.audit.record(&cx.session.user, "settings.sound.hwcheck", &card_id);
        self.start("playing two tones", move || {
            let r = svc.hw_volume_check(card, device, &control);
            JobOut::HwCheck(card_id, r)
        });
    }

    /// The two tones were played: ask which one was quieter.
    pub(super) fn hw_check_done(&mut self, card_id: String, r: Result<String, String>) {
        match r {
            Ok(m) => {
                self.ok(m);
                self.modal = Some(Modal::Confirm(ConfirmKind::SoundHwWorks(card_id), "Did the second tone sound quieter than the first? Press y for Yes: the card's hardware volume works and is used. Press n for No: it does nothing, so IYAKKI uses its own software volume for this card. Esc leaves the setting as it is.".into()));
            }
            Err(e) => self.fail(e),
        }
    }

    /// The answer to "Did the second tone sound quieter?".
    pub(super) fn sound_hw_answer(&mut self, cx: &AppCx, kind: &ConfirmKind, quieter: bool) {
        let ConfirmKind::SoundHwWorks(card_id) = kind else { return };
        let (id, mode) = (card_id.clone(), if quieter { audioout::VolumeMode::Hardware } else { audioout::VolumeMode::Software });
        match Self::svc(cx).update_sound(|s| s.set_volume_mode(&id, mode)) {
            Ok(now) => {
                cx.audit.record(&cx.session.user, "settings.sound.volume_control", &format!("{card_id} {}", mode.label()));
                self.ok(if quieter { "The hardware volume is used for this card." } else { "This card's hardware volume does nothing: IYAKKI's software volume is used." });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    fn sound_pattern(&mut self, cx: &AppCx, walk: bool) {
        let svc = Self::svc(cx).clone();
        cx.audit.record(&cx.session.user, "settings.sound.pattern", if walk { "channel walk" } else { "stereo test" });
        self.start(if walk { "playing the channel walk" } else { "playing the stereo test" }, move || JobOut::Msg(svc.test_pattern(walk)));
    }
}
