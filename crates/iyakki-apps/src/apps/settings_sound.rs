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
use super::super::settings_device_port::SoundPort;
use iyakki_shared::audioout;

impl SettingsApp {
    /// Keys of the Sound page about the output and IYAKKI's volume. True when the key was taken here.
    pub(super) fn sound_key(&mut self, k: &Key, p: &dyn SoundPort) -> bool {
        self.hw.set_sound(p.sound_settings());
        self.hw.set_mic_choice(p.voice_settings().mic);
        let target = self.selected_target();
        match k {
            Key::Char('i') => self.sound_mic_next(p),
            Key::Char('r') => self.sound_mic_test(p),
            Key::Char('o') => self.sound_choose(p),
            Key::Char('t') => self.sound_test(p, target),
            Key::Char('A') => self.sound_advanced(),
            Key::Char('M') => self.sound_mixer(p),
            Key::Char('W') => self.sound_web_mix(p),
            Key::Char('B') => self.sound_bluetooth(p),
            Key::Enter | Key::Char(' ' | 'e') => match target {
                Some(Target::SoundChoice) => self.sound_choose(p),
                Some(Target::SoundOut(key, _)) => self.sound_set_output(p, &key),
                Some(Target::SoftVolume) => self.sound_volume(p, None),
                Some(Target::SoundAdvanced) => self.sound_advanced(),
                Some(Target::SoundMixer) => self.sound_mixer(p),
                Some(Target::SoundMic(id)) => self.sound_mic_set(p, &id),
                Some(Target::SoundMicTest) => self.sound_mic_test(p),
                Some(Target::SoundT2Mic) => self.sound_t2_mic(p),
                Some(Target::SoundWebMix) => self.sound_web_mix(p),
                Some(Target::SoundBluetooth) => self.sound_bluetooth(p),
                Some(Target::SoundHighlightOffset) => self.sound_set_highlight_offset(p, 0),
                Some(Target::SoundVolMode(card_id)) => self.sound_cycle_mode(p, &card_id),
                Some(Target::SoundHwCheck(card, device, control, card_id)) => self.sound_hw_check(p, card, device, control, card_id),
                Some(Target::SoundPattern(walk)) => self.sound_pattern(p, walk),
                _ => return false,
            },
            // + − on the boost row move the boost
            Key::Char('+' | '=' | '-') if matches!(target, Some(Target::SoundBoost(_))) => {
                if let Some(Target::SoundBoost(card_id)) = target {
                    let cur = p.sound_settings().boost_for(&card_id) as i32;
                    let next = (cur + if matches!(k, Key::Char('-')) { -1 } else { 1 }).clamp_safe(0, iyakki_shared::softvol::MAX_BOOST_DB as i32);
                    self.sound_set_boost(p, &card_id, next as u32);
                }
            }
            // + − on the highlight offset row move it by 25 ms
            Key::Char('+' | '=' | '-') if matches!(target, Some(Target::SoundHighlightOffset)) => {
                let cur = p.sound_settings().highlight_offset_ms;
                self.sound_set_highlight_offset(p, cur + if matches!(k, Key::Char('-')) { -25 } else { 25 });
            }
            // + − m change IYAKKI's volume everywhere but on a card's own mixer row
            Key::Char('+' | '=' | '-' | 'm') if !matches!(target, Some(Target::Card(_))) => {
                let delta = match k {
                    Key::Char('+' | '=') => Some(5),
                    Key::Char('-') => Some(-5),
                    _ => None,
                };
                self.sound_volume(p, delta)
            }
            _ => return false,
        }
        true
    }

    /// The list of outputs to choose from.
    fn sound_choose(&mut self, p: &dyn SoundPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the sound output.");
        }
        let cur = p.sound_settings().output;
        let mark = |on: bool| if on { "  ✓" } else { "" };
        let mut items = vec![format!("Automatic (follows what is connected){}", mark(cur.is_empty()))];
        items.extend(self.hw.outputs(self.ticks).iter().map(|o| format!("{} · {}  [{}]{}", o.label(), o.state(), o.key(), mark(o.key() == cur))));
        self.modal = Some(Modal::Pick(PickKind::SoundOutput, Picker::new("Sound output", items)));
    }

    /// A line of the list above was picked.
    pub(super) fn sound_pick(&mut self, p: &dyn SoundPort, item: &str) {
        let key = if item.starts_with("Automatic") { String::new() } else { item.rsplit_once('[').and_then(|(_, r)| r.split_once(']')).map(|(k, _)| k.to_string()).unwrap_or_default() };
        if key.is_empty() && !item.starts_with("Automatic") {
            return;
        }
        self.sound_set_output(p, &key);
    }

    fn sound_set_output(&mut self, p: &dyn SoundPort, key: &str) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the sound output.");
        }
        match p.update_sound(&mut |s| s.output = key.to_string()) {
            Ok(now) => {
                p.audit("settings.sound.output", if key.is_empty() { "automatic" } else { key });
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
    fn sound_volume(&mut self, p: &dyn SoundPort, delta: Option<i32>) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the volume.");
        }
        let r = p.update_sound(&mut |s| match delta {
            Some(d) => {
                s.volume_pct = (s.volume_pct as i32 + d).clamp(0, 100) as u32;
                if d > 0 {
                    s.muted = false;
                }
            }
            None => s.muted = !s.muted,
        });
        self.sound_said(p, r);
    }

    /// A click on the IYAKKI volume slider's track.
    pub(super) fn sound_set_volume(&mut self, p: &dyn SoundPort, v: u32) {
        if !self.admin(p) {
            return;
        }
        let r = p.update_sound(&mut |s| {
            s.volume_pct = v.min(100);
            s.muted = false;
        });
        self.sound_said(p, r);
    }

    fn sound_said(&mut self, p: &dyn SoundPort, r: Result<audioout::SoundSettings, String>) {
        match r {
            Ok(now) => {
                p.audit("settings.sound.volume", &if now.muted { "muted".to_string() } else { now.volume_pct.to_string() });
                self.ok(if now.muted { "IYAKKI sound muted.".to_string() } else { format!("IYAKKI volume {}%.", now.volume_pct) });
                self.hw.set_sound(now);
            }
            Err(e) => self.fail(e),
        }
    }

    /// `t`: a short gentle chime on the selected output, else on the one in use.
    fn sound_test(&mut self, p: &dyn SoundPort, target: Option<Target>) {
        let prefs = p.sound_settings();
        let outs = self.hw.outputs(self.ticks);
        let dev = match target {
            Some(Target::SoundOut(_, alsa)) => Some(alsa),
            _ => {
                let c = audioout::choose(&outs, &prefs.output);
                match c.output {
                    // a sound server chooses for itself in automatic mode
                    Some(_) if !c.chosen && self.hw.real_sound() && p.sound_server_running() => None,
                    Some(o) => Some(o.alsa()),
                    None if outs.is_empty() => None, // nothing listed (another OS, a sound server): the default device
                    None => return self.fail(c.reason),
                }
            }
        };
        p.audit("settings.sound.test", dev.as_deref().unwrap_or("default"));
        let job = p.test_sound_job(dev);
        self.start("playing a test sound", move || JobOut::Msg(job()));
    }
}

/// Settings -> Sound: the mixer switch. The setting is the machine's, so only administrators change it; the agent applies it
/// (no app has a branch for it).
impl SettingsApp {
    pub(super) fn sound_mixer(&mut self, p: &dyn SoundPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the sound mixer.");
        }
        let on = !p.sound_settings().mixer;
        match p.update_sound(&mut |s| s.mixer = on) {
            Ok(now) => {
                p.audit("settings.sound.mixer", if on { "on" } else { "off" });
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
    /// The Bluetooth row (F-NET-28): stop when sending, else ask on a card which pair gets all of the computer's sound (the only paired one, or the
    /// remembered one when several are paired). The core does the connecting through the same tools the assistant uses.
    pub(super) fn sound_bluetooth(&mut self, p: &dyn SoundPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can send the computer's sound to Bluetooth headphones.");
        }
        let view = p.bluetooth_sound_view();
        if view.active {
            return self.sound_bluetooth_stop(p);
        }
        let Some((_, name)) = view.sinks.first().filter(|_| view.sinks.len() == 1) else {
            return if view.sinks.is_empty() {
                self.fail("No headphones or speaker are paired: pair them in Settings -> Bluetooth first.")
            } else {
                // several are paired: the person picks one
                let items = view.sinks.iter().map(|(a, n)| format!("{n}  [{a}]")).collect();
                self.modal = Some(Modal::Pick(PickKind::BluetoothSink, Picker::new("Send the computer's sound to", items)));
            };
        };
        self.modal = Some(Modal::Confirm(ConfirmKind::BluetoothSound(String::new()), format!("Send all of this computer's sound to {name}? Its own speakers go silent until you stop it. Press y to start.")));
    }

    pub(super) fn sound_bluetooth_stop(&mut self, p: &dyn SoundPort) {
        match p.bluetooth_sound_stop() {
            Ok(m) => {
                p.audit("settings.sound.bluetooth", "stop");
                self.ok(m);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    /// The person approved: connect and send (waits for the set-up, which can take a few seconds).
    pub(super) fn sound_bluetooth_start(&mut self, p: &dyn SoundPort, address: &str) {
        match p.bluetooth_sound_start(address) {
            Ok(m) => {
                p.audit("settings.sound.bluetooth", "start");
                self.ok(m);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn sound_web_mix(&mut self, p: &dyn SoundPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the browser's sound route.");
        }
        let on = !p.sound_settings().web_mix;
        match p.update_sound(&mut |s| s.web_mix = on) {
            Ok(now) => {
                p.audit("settings.sound.web_mix", if on { "on" } else { "off" });
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
    pub(super) fn sound_set_highlight_offset(&mut self, p: &dyn SoundPort, ms: i32) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the highlight timing offset.");
        }
        let ms = ms.clamp_safe(-500, 500);
        match p.update_sound(&mut |s| s.highlight_offset_ms = ms) {
            Ok(now) => {
                p.audit("settings.sound.highlight_offset", &ms.to_string());
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

    pub(super) fn sound_set_boost(&mut self, p: &dyn SoundPort, card_id: &str, db: u32) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the boost.");
        }
        let id = card_id.to_string();
        match p.update_sound(&mut |s| s.set_boost(&id, db)) {
            Ok(now) => {
                p.audit("settings.sound.boost", &format!("{card_id} {db}"));
                self.ok(if db == 0 { "Boost off.".to_string() } else { format!("Boost +{db} dB. Loud sound may distort. It applies from the next sound.") });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    fn sound_cycle_mode(&mut self, p: &dyn SoundPort, card_id: &str) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the volume control.");
        }
        let id = card_id.to_string();
        let next = p.sound_settings().volume_mode_for(card_id).next();
        match p.update_sound(&mut |s| s.set_volume_mode(&id, next)) {
            Ok(now) => {
                p.audit("settings.sound.volume_control", &format!("{card_id} {}", next.label()));
                self.ok(format!("Volume control: {}.", next.label()));
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn sound_hw_check(&mut self, p: &dyn SoundPort, card: u32, device: u32, control: String, card_id: String) {
        if !self.admin(p) {
            return self.fail("Only administrators can change the volume control.");
        }
        p.audit("settings.sound.hwcheck", &card_id);
        let job = p.hw_volume_check_job(card, device, control);
        self.start("playing two tones", move || JobOut::HwCheck(card_id, job()));
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
    pub(super) fn sound_hw_answer(&mut self, p: &dyn SoundPort, kind: &ConfirmKind, quieter: bool) {
        let ConfirmKind::SoundHwWorks(card_id) = kind else { return };
        let (id, mode) = (card_id.clone(), if quieter { audioout::VolumeMode::Hardware } else { audioout::VolumeMode::Software });
        match p.update_sound(&mut |s| s.set_volume_mode(&id, mode)) {
            Ok(now) => {
                p.audit("settings.sound.volume_control", &format!("{card_id} {}", mode.label()));
                self.ok(if quieter { "The hardware volume is used for this card." } else { "This card's hardware volume does nothing: IYAKKI's software volume is used." });
                self.hw.set_sound(now);
                self.hw.invalidate();
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn sound_pattern(&mut self, p: &dyn SoundPort, walk: bool) {
        p.audit("settings.sound.pattern", if walk { "channel walk" } else { "stereo test" });
        let job = p.test_pattern_job(walk);
        self.start(if walk { "playing the channel walk" } else { "playing the stereo test" }, move || JobOut::Msg(job()));
    }
}

/// Settings -> Sound, Microphone: which input is used (one choice for dictation and for the web pages in the Browser) and a short listening test.
impl SettingsApp {
    fn sound_mics(p: &dyn SoundPort) -> Vec<iyakki_shared::voicedesk::Mic> {
        p.list_mics()
    }

    /// Use this input (empty: Automatic).
    /// Enter on the built-in microphone row: turning it on asks first (the T2 driver can freeze the Mac); turning it off is immediate.
    fn sound_t2_mic(&mut self, p: &dyn SoundPort) {
        if !self.admin(p) {
            return self.fail("Only administrators can change this.");
        }
        match p.t2_mic() {
            iyakki_shared::voicedesk::T2Mic::Absent => self.fail("This computer has no T2 sound chip."),
            iyakki_shared::voicedesk::T2Mic::On => self.sound_t2_mic_apply(p, false),
            iyakki_shared::voicedesk::T2Mic::Off => {
                self.modal = Some(Modal::Confirm(
                    ConfirmKind::T2Mic,
                    "Allow the Mac's built-in microphone until the next restart? The T2 chip's sound driver has frozen a whole Mac when it recorded; the unsaved work of the programs running would be lost, and the Mac needs a restart. Nothing is stored: after a restart it is off again. Press y to allow it, or any other key to leave it off.".into(),
                ));
            }
        }
    }

    pub(super) fn sound_t2_mic_apply(&mut self, p: &dyn SoundPort, on: bool) {
        if !self.admin(p) {
            return;
        }
        match p.set_t2_capture(on) {
            Ok(()) => {
                p.audit("settings.sound.t2mic", if on { "on" } else { "off" });
                self.hw.set_mic_choice(p.voice_settings().mic);
                self.ok(if on { "The built-in microphone is allowed until the next restart. It appears in the list above: choose it and press r to test." } else { "The built-in microphone is hidden again." });
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn sound_mic_set(&mut self, p: &dyn SoundPort, id: &str) {
        match p.set_voice_mic(id) {
            Ok(()) => {
                p.audit("settings.sound.mic", if id.is_empty() { "automatic" } else { id });
                self.hw.set_mic_choice(id.to_string());
                let name = Self::sound_mics(p).into_iter().find(|m| m.id == id).map(|m| m.label);
                self.ok(match name {
                    Some(n) => format!("Recording from {n} (also for web pages in the Browser)."),
                    None => "The microphone is chosen automatically.".to_string(),
                });
            }
            Err(e) => self.fail(format!("Could not save the choice: {e}")),
        }
    }

    /// `i`: Automatic, then each input in turn.
    fn sound_mic_next(&mut self, p: &dyn SoundPort) {
        let mics = Self::sound_mics(p);
        if mics.is_empty() {
            return self.fail("No microphone found: plug in a USB microphone or a camera that has one.");
        }
        let cur = p.voice_settings().mic;
        let next = match mics.iter().position(|m| m.id == cur) {
            None => mics[0].id.clone(),
            Some(i) if i + 1 < mics.len() => mics[i + 1].id.clone(),
            Some(_) => String::new(),
        };
        self.sound_mic_set(p, &next);
    }

    /// `r`: listen for three seconds and say how loud it was (what Settings -> Voice -> Test mic does, aimed at the chosen input).
    pub(super) fn sound_mic_test(&mut self, p: &dyn SoundPort) {
        let v = p.voice_settings();
        let mics = Self::sound_mics(p);
        let Some(m) = iyakki_shared::voicedesk::pick_mic(&mics, &v.mic) else {
            return self.fail("No microphone found: plug in a USB microphone or a camera that has one.");
        };
        let cmd = p.record_cmd_for(&v.record_cmd, Some(m));
        let label = m.label.clone();
        p.audit("settings.sound.mic_test", &m.id);
        let job = p.mic_test_job(label, cmd);
        self.start("listening to the microphone (speak now)", move || JobOut::Msg(job()));
    }

    /// The Sound page's keys: its own keys first (output, volume, mixer and the rest), then the look-again key and the rows' keys.
    pub(super) fn handle_sound(&mut self, k: &Key, p: &dyn SoundPort) {
        if self.sound_key(k, p) {
            return;
        }
        let len = self.dev_rows().len();
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('r') => {
                // the sound watcher's picture may be a few seconds old: look now, and say if the card is still starting
                let starting = if self.hw.real_sound() { p.refresh_starting() } else { None };
                self.hw.invalidate();
                match starting {
                    Some(line) => self.ok(format!("Looked again: {line}.")),
                    None => self.ok("Looked at the hardware again."),
                }
            }
            other => self.dev_act(DevTab::Sound, other, p),
        }
    }

    /// The Sound page: its rows and the help line under them.
    pub(super) fn render_sound(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn SoundPort) {
        self.hw.set_sound(p.sound_settings());
        let admin = p.is_admin();
        self.draw_dev_rows(g, a, s, a.y, admin, "o chooses the output (Automatic follows what is connected) · t plays a test chime · the IYAKKI volume is the one master volume: the volume keys, the Devices popover and the radio follow it · A advanced · M mixes sounds from several apps (experimental).");
    }
}
