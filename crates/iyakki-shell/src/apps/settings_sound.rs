//! Settings → Sound: which output IYAKKI plays on (Automatic, or one device such as "HDMI: PDM-15T"), a test chime on the
//! chosen output, and IYAKKI's own volume and mute.
//!
//! The choice is the machine's (administrators) and is kept in the settings file; Automatic follows what is connected
//! (`iyakki_agent::audioout`). The volume is the output's own: a software volume (ALSA softvol, `iyakki_agent::softvol`)
//! when the output has no hardware volume (HDMI on Intel, the T2 Mac), which every program follows; else it is applied by
//! IYAKKI's players. Each output keeps its last volume. The
//! rows come from `devices::sound_rows`; the card mixers below them keep working as before.

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
            Key::Enter | Key::Char(' ' | 'e') => match target {
                Some(Target::SoundChoice) => self.sound_choose(cx),
                Some(Target::SoundOut(key, _)) => self.sound_set_output(cx, &key),
                Some(Target::SoftVolume) => self.sound_volume(cx, None),
                _ => return false,
            },
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
