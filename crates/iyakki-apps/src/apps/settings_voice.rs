//! Settings: the voice page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::*;
use super::apps_port::{store_secret, VoicePort};

impl SettingsApp {
    pub(super) fn engine_form(p: &dyn VoicePort, stt: bool) -> Form {
        let cur = {
            let st = p.settings_get();
            if stt { st.voice.stt.clone() } else { st.voice.tts.clone() }
        };
        let (kind, target, model, voice_name, lang, has_key) = match &cur {
            Some(SpeechEngine::Http { url, model, voice, language, key_ref }) => (0, url.clone(), model.clone(), voice.clone().unwrap_or_default(), language.clone().unwrap_or_default(), key_ref.is_some()),
            Some(SpeechEngine::Command { command }) => (1, command.clone(), String::new(), String::new(), String::new(), false),
            Some(SpeechEngine::Browser) => (2, String::new(), String::new(), String::new(), String::new(), false),
            // an engine of this computer is chosen on the page itself (Engine, Voice); here it is a command that runs it
            Some(SpeechEngine::Host { .. }) => (0, "http://127.0.0.1:8000/v1/audio/speech".into(), String::new(), String::new(), String::new(), false),
            // Google Gemini is set up on Settings -> Models (key g); saving this form replaces it with the endpoint typed here
            Some(SpeechEngine::Gemini { .. }) => (0, "http://127.0.0.1:8080/v1/audio/transcriptions".into(), String::new(), String::new(), String::new(), false),
            None => (0, if stt { "http://127.0.0.1:8080/v1/audio/transcriptions".into() } else { "http://127.0.0.1:8000/v1/audio/speech".into() }, String::new(), String::new(), String::new(), false),
        };
        let hint = if stt { "whisper-cli -m model.bin -f {wav} -nt -np" } else { "piper -m voice.onnx -f {out}" };
        let f = Form::new(if stt { "Speech-to-text engine" } else { "Text-to-speech engine" })
            .choice("Type", if stt { &["HTTP endpoint (OpenAI-compatible)", "Local command"][..] } else { &["HTTP endpoint (OpenAI-compatible)", "Local command", BROWSER_CHOICE][..] }, if stt { kind.min(1) } else { kind })
            .text("URL or command", &target, hint)
            .text("Model (HTTP only)", &model, if stt { "whisper-1, tiny.en, …" } else { "tts-1, kokoro, …" })
            .secret("API key (optional, HTTP only)", if has_key { "leave empty to keep the current key" } else { "stored in the system vault" });
        if stt { f.text("Language hint (optional)", &lang, "en") } else { f.text("Voice (optional)", &voice_name, "alloy, nova, …") }
    }

    pub(super) fn save_speech_engine(&mut self, p: &dyn VoicePort, form: &Form, stt: bool) -> Result<(), String> {
        let (target, model, key, extra) = (form.value(1), form.value(2), form.raw(3), form.value(4));
        let key_name = if stt { "stt-key" } else { "tts-key" };
        let prev_key = {
            let st = p.settings_get();
            match if stt { &st.voice.stt } else { &st.voice.tts } {
                Some(SpeechEngine::Http { key_ref, .. }) => key_ref.clone(),
                _ => None,
            }
        };
        if !stt && form.choice_idx(0) == 2 {
            // nothing to configure: the voices come from the browser or window in use
            p.settings_update(&mut |st| st.voice.tts = Some(SpeechEngine::Browser)).map_err(|e| e.message())?;
            p.audit("settings.voice.tts", "browser");
            return Ok(());
        }
        let engine = if form.choice_idx(0) == 0 {
            let key_ref = if !key.is_empty() { Some(format!("vault:{key_name}")) } else { prev_key };
            let opt = |s: String| if s.is_empty() { None } else { Some(s) };
            SpeechEngine::Http { url: target, model, key_ref, voice: if stt { None } else { opt(extra.clone()) }, language: if stt { opt(extra) } else { None } }
        } else {
            SpeechEngine::Command { command: target }
        };
        engine.validate()?;
        if form.choice_idx(0) == 0 && !key.is_empty() {
            store_secret(p, key_name, &key, true)?;
        }
        p.settings_update(&mut |st| if stt { st.voice.stt = Some(engine.clone()) } else { st.voice.tts = Some(engine.clone()) }).map_err(|e| e.message())?;
        p.audit(if stt { "settings.voice.stt" } else { "settings.voice.tts" }, "");
        Ok(())
    }

    pub(super) fn handle_voice(&mut self, k: &Key, p: &dyn VoicePort) {
        let svc = p.voice_ops();
        let toggle = |p: &dyn VoicePort, f: &dyn Fn(&mut iyakki_shared::voice::VoiceSettings)| p.settings_update(&mut |s| f(&mut s.voice));
        match k {
            Key::Char('s') if self.admin(p) => {
                let f = Self::engine_form(p, true);
                self.open_form(FormKind::VoiceStt, f);
            }
            Key::Char('v') if self.admin(p) => {
                let f = Self::engine_form(p, false);
                self.open_form(FormKind::VoiceTts, f);
            }
            Key::Char('c') if self.admin(p) => {
                let v = svc.voice_settings();
                let f = Form::new("Microphone and speaker commands").text("Record command (raw 16 kHz mono S16_LE on stdout)", &v.record_cmd, "arecord -q -f S16_LE -r 16000 -c 1 -t raw").text("Play command (WAV on stdin)", &v.play_cmd, "aplay -q");
                self.open_form(FormKind::VoiceCmds, f);
            }
            Key::Char('a') if self.admin(p) => match toggle(p, &|v| v.auto_send = !v.auto_send) {
                Ok(()) => self.ok("Auto-send toggled."),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('u') if self.admin(p) => match toggle(p, &|v| v.auto_stop = !v.auto_stop) {
                Ok(()) => self.ok(if p.voice_ops().voice_settings().auto_stop { "Dictation now ends by itself after a pause." } else { "Dictation now ends only when you stop it." }),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('U') if self.admin(p) => self.vo_cycle_pause(p),
            Key::Char('K') if self.admin(p) => match toggle(p, &|v| v.keep_listening = !v.keep_listening) {
                Ok(()) => self.ok(if p.voice_ops().voice_settings().keep_listening { "Keep listening is on: after each pause the text is written and it listens again until Stop." } else { "Keep listening is off." }),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('W') if self.admin(p) => match p.dict_fix_set(!svc.voice_settings().fix_wording) {
                Ok(()) => {
                    let on = svc.voice_settings().fix_wording;
                    p.audit(iyakki_shared::dictfix::AUDIT, if on { "on" } else { "off" });
                    self.ok(if on { "Correct my wording is on: dictated text is tidied by the chat assistant before it lands in the box." } else { "Correct my wording is off." });
                }
                Err(e) => self.fail(e),
            },
            Key::Char('M') if self.admin(p) => self.vo_wake_toggle(p),
            Key::Char('N') if self.admin(p) => self.vo_wake_sens(p),
            Key::Char('T') if self.admin(p) => self.vo_wake_confirm_toggle(p),
            Key::Char('L') if self.admin(p) => self.vo_cloud_stream_toggle(p),
            Key::Char('A') if self.admin(p) => self.vo_spoken_toggle(p),
            Key::Char('E') if self.admin(p) => self.vo_echo_toggle(p),
            Key::Char('r') if self.admin(p) => match toggle(p, &|v| v.speak_replies = !v.speak_replies) {
                Ok(()) => self.ok("Read replies aloud toggled."),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('S') if self.admin(p) => match toggle(p, &|v| v.stt = None) {
                Ok(()) => self.ok("Speech-to-text cleared."),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('V') if self.admin(p) => match toggle(p, &|v| v.tts = None) {
                Ok(()) => self.ok("Text-to-speech cleared."),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('e') if self.admin(p) => self.vo_cycle_engine(p),
            Key::Char('g') if self.admin(p) => self.vo_cycle_voice(p),
            Key::Char('f') if self.admin(p) => self.vo_cycle_speed(p),
            Key::Char('h') if self.admin(p) => self.vo_cycle_highlight(p),
            Key::Char('w') if self.admin(p) => match toggle(p, &|v| v.align_words = !v.align_words) {
                Ok(()) => self.ok(if p.voice_ops().voice_settings().align_words { "Word timing: from the sound (speech-to-text matches the read words when a server is there)." } else { "Word timing: estimated." }),
                Err(e) => self.fail(e.message()),
            },
            Key::Char('l') => {
                self.vo_lang = (self.vo_lang + 1) % VO_LANGS.len();
                self.vo_scroll = 0.0;
            }
            Key::Char('k') => {
                p.audit("voice.recheck", "");
                if !p.host_tts().is_enabled() {
                    self.fail("Looking for speech engines is switched off in this build.");
                } else {
                    p.host_tts().recheck();
                    self.ok("Looking for speech engines on this computer…");
                }
            }
            Key::Char('o') => self.vo_helper_toggle(p),
            Key::Char(c @ '1'..='6') if self.admin(p) => self.vo_quick(p, (*c as u8 - b'1') as usize),
            Key::Char(c) if "mpty".contains(*c) => {
                if !p.may_use_voice() {
                    p.audit("voice.denied", "test");
                    self.fail("Voice is not enabled for your account.");
                    return;
                }
                let ms = self.test_record_ms;
                let v = svc.voice_settings();
                // aimed at the chosen (or first) microphone like the VOICE tile does: a bare `arecord` opens ALSA's default device, which a computer with no
                // default capture card (a Mac mini with only a USB camera's microphone) does not have ("audio open error: No such file or directory")
                let rec_cmd = {
                    let mics = p.agent().voicedesk_list_mics(&p.agent().voicedesk_proc_root());
                    p.agent().voicedesk_record_cmd_for(&v.record_cmd, iyakki_shared::voicedesk::pick_mic(&mics, &v.mic))
                };
                
                p.audit("voice.test", &c.to_string());
                match *c {
                    'm' => self.start("listening to the microphone", move || {
                        JobOut::Msg((|| {
                            let rec = crate::recorders().start(&rec_cmd, iyakki_shared::voicedesk::MAX_RECORD_SECS)?;
                            std::thread::sleep(Duration::from_millis(ms));
                            let pcm = rec.stop()?;
                            let pk = voice::peak(&pcm);
                            if pk < 150 {
                                Err(format!("Microphone opened but it is silent (peak {pk}). Check the input volume and mute switch."))
                            } else {
                                Ok(format!("Microphone works: {:.1} s captured, peak level {pk} of 32767.", pcm.len() as f32 / voice::SAMPLE_RATE as f32))
                            }
                        })())
                    }),
                    'p' => self.start("playing a test tone", move || {
                        JobOut::Msg((|| {
                            let mut pl = crate::players().play(&v.play_cmd, &voice::encode_wav(&voice::tone(660.0, 700), voice::SAMPLE_RATE))?;
                            pl.wait(Duration::from_secs(8));
                            Ok("Played a test tone through the speaker command.".to_string())
                        })())
                    }),
                    't' => self.start("say something…", move || {
                        JobOut::Msg((|| {
                            let rec = crate::recorders().start(&rec_cmd, iyakki_shared::voicedesk::MAX_RECORD_SECS)?;
                            std::thread::sleep(Duration::from_millis(ms * 2));
                            let pcm = rec.stop()?;
                            let text = svc.transcribe_samples(&pcm)?;
                            Ok(format!("Heard: “{text}”"))
                        })())
                    }),
                    _ => self.vo_test(p),
                }
            }
            _ => {}
        }
    }

    pub(super) fn vo_tag(&self) -> &'static str {
        hostvoice::lang_tag(VO_LANGS[self.vo_lang % VO_LANGS.len()])
    }

    /// The engine that reads: the chosen one, else the best found on this computer, else the browser voice of the screen.
    pub(super) fn vo_engine(&self, p: &dyn VoicePort) -> Option<SpeechEngine> {
        let svc = p.voice_ops();
        let v = svc.voice_settings();
        let auto = if v.tts.is_none() { p.host_tts().auto_engine(Some(self.vo_tag())) } else { None };
        iyakki_shared::storyvoice::effective_engine_auto(&v, &HostVoice::current(), auto.as_ref())
    }

    pub(super) fn vo_usable(p: &dyn VoicePort) -> Vec<hostengines::Engine> {
        p.host_tts().report().map(|r| r.engines.iter().filter(|e| e.usable()).cloned().collect()).unwrap_or_default()
    }

    pub(super) fn set_tts(&mut self, p: &dyn VoicePort, e: Option<SpeechEngine>, what: &str) {
        if let Some(e) = &e {
            if let Err(m) = e.validate() {
                self.fail(m);
                return;
            }
        }
        match p.settings_update(&mut |st| st.voice.tts = e.clone()) {
            Ok(()) => {
                p.audit("settings.voice.tts", what);
                self.ok(format!("Text to speech: {what}."));
            }
            Err(e) => self.fail(e.message()),
        }
    }

    /// Engine ▾: automatic (the best found), each usable engine of this computer, then the browser voice when the screen has one.
    pub(super) fn vo_cycle_engine(&mut self, p: &dyn VoicePort) {
        let mut seq: Vec<(Option<SpeechEngine>, String)> = vec![(None, "automatic".into())];
        for e in Self::vo_usable(p) {
            seq.push((Some(iyakki_shared::hostengines::engine_setting(&e)), e.kind.label().to_string()));
        }
        if HostVoice::current().is_ready() {
            seq.push((Some(SpeechEngine::Browser), "browser voice".into()));
        }
        let cur = p.voice_ops().voice_settings().tts;
        let same = |a: &Option<SpeechEngine>, b: &Option<SpeechEngine>| match (a, b) {
            (Some(SpeechEngine::Host { engine: x, .. }), Some(SpeechEngine::Host { engine: y, .. })) => x == y,
            (a, b) => a == b,
        };
        let at = seq.iter().position(|(e, _)| same(e, &cur)).unwrap_or(0);
        let (next, name) = seq[(at + 1) % seq.len()].clone();
        self.set_tts(p, next, &name);
    }

    /// The voices of the engine in use for the language shown: (id, name).
    pub(super) fn vo_voices(&self, p: &dyn VoicePort) -> Vec<(String, String)> {
        let Some(SpeechEngine::Host { engine, .. }) = self.vo_engine(p) else { return Vec::new() };
        let Some(report) = p.host_tts().report() else { return Vec::new() };
        let Some(e) = hostengines::Kind::from_id(&engine).and_then(|k| report.get(k)) else { return Vec::new() };
        let tag = self.vo_tag();
        let mut mine: Vec<&hostengines::HVoice> = e.voices.iter().filter(|v| v.fit(tag) > 0).collect();
        if mine.is_empty() {
            mine = e.voices.iter().collect();
        }
        mine.into_iter().take(60).map(|v| (v.id.clone(), v.name.clone())).collect()
    }

    pub(super) fn vo_cycle_voice(&mut self, p: &dyn VoicePort) {
        let Some(SpeechEngine::Host { engine, program, voice }) = self.vo_engine(p) else {
            self.fail("The voice is part of the engine's own settings; choose an engine of this computer (Engine) to pick its voices.");
            return;
        };
        let voices = self.vo_voices(p);
        if voices.is_empty() {
            self.fail("This engine lists no voices.");
            return;
        }
        let at = voice.as_ref().and_then(|cur| voices.iter().position(|(id, _)| id == cur));
        // automatic, then each voice
        let next = match at {
            None if voice.is_none() => Some(0),
            None => Some(0),
            Some(i) if i + 1 < voices.len() => Some(i + 1),
            Some(_) => None,
        };
        let (new_voice, name) = match next {
            Some(i) => (Some(voices[i].0.clone()), voices[i].1.clone()),
            None => (None, "automatic".to_string()),
        };
        self.set_tts(p, Some(SpeechEngine::Host { engine, program, voice: new_voice }), &format!("voice {name}"));
    }

    /// Sentence + word, sentence only, off: what is lit while text is read aloud (stories and chat).
    pub(super) fn vo_cycle_highlight(&mut self, p: &dyn VoicePort) {
        use iyakki_shared::voice::Highlight;
        let cur = p.voice_ops().voice_settings().highlight;
        let next = Highlight::ALL[(Highlight::ALL.iter().position(|h| *h == cur).unwrap_or(0) + 1) % Highlight::ALL.len()];
        match p.settings_update(&mut |st| st.voice.highlight = next) {
            Ok(()) => {
                p.audit("settings.voice.highlight", next.label());
                self.ok(format!("While reading aloud: highlight {}.", next.label().to_lowercase()));
            }
            Err(e) => self.fail(e.message()),
        }
    }

    /// The pause that ends a dictation: 1, 1.5, 2, 3 seconds, round again.
    pub(super) fn vo_cycle_pause(&mut self, p: &dyn VoicePort) {
        const PAUSES: [u32; 4] = [1000, 1500, 2000, 3000];
        let cur = p.voice_ops().voice_settings().pause_ms;
        let next = PAUSES.iter().copied().find(|pl| *pl > cur).unwrap_or(PAUSES[0]);
        match p.settings_update(&mut |st| st.voice.pause_ms = next) {
            Ok(()) => {
                p.audit("settings.voice.pause", &next.to_string());
                self.ok(format!("A pause of {:.1} s ends a dictation.", next as f32 / 1000.0));
            }
            Err(e) => self.fail(e.message()),
        }
    }

    pub(super) fn vo_cycle_speed(&mut self, p: &dyn VoicePort) {
        const SPEEDS: [u32; 5] = [75, 100, 125, 150, 200];
        let cur = p.voice_ops().voice_settings().speed_pct;
        let next = SPEEDS.iter().copied().find(|s| *s > cur).unwrap_or(SPEEDS[0]);
        match p.settings_update(&mut |st| st.voice.speed_pct = next) {
            Ok(()) => {
                p.audit("settings.voice.speed", &next.to_string());
                self.ok(format!("Speaking speed {next}%."));
            }
            Err(e) => self.fail(e.message()),
        }
    }

    /// One-click engine `i` of the providers set up under Models.
    pub(super) fn vo_quick(&mut self, p: &dyn VoicePort, i: usize) {
        let Some(q) = Self::vo_quick_list(p).get(i).cloned() else { return };
        self.set_tts(p, Some(q.engine), q.label.trim_start_matches("Use "));
    }

    /// Speech endpoints of the providers already set up under Models (OpenAI and compatible servers).
    pub(super) fn vo_quick_list(p: &dyn VoicePort) -> Vec<iyakki_shared::storyvoice::QuickEngine> {
        let providers = p.settings_get().providers.clone();
        p.agent().helpers_b7().quick_engines(&providers, "", &[]).into_iter().filter(|q| matches!(q.engine, SpeechEngine::Http { .. })).collect()
    }

    /// Test voice: say a sentence in the chosen language with the engine in use, and say what happened.
    pub(super) fn vo_test(&mut self, p: &dyn VoicePort) {
        let svc = p.voice_ops();
        let v = svc.voice_settings();
        let tag = self.vo_tag();
        let speed = v.speed_pct as f32 / 100.0;
        let hv = HostVoice::current();
        let agent = p.agent_arc();
        let Some(engine) = self.vo_engine(p) else {
            self.fail(if p.host_tts().is_enabled() && p.host_tts().report().is_none() {
                "Still looking for speech engines on this computer; try again in a moment."
            } else {
                "No speech engine is ready. Install one (see the list below), press Re-check, or open the voice helper in your browser."
            });
            return;
        };
        self.start("speaking", move || {
            JobOut::Msg((|| {
                let sample = hostengines::sample_sentence(tag);
                match &engine {
                    SpeechEngine::Host { engine: id, program, voice } => {
                        agent.host_engines().test_speak(id, program.as_deref(), voice.as_deref(), tag, speed, &v.play_cmd)
                    }
                    SpeechEngine::Browser => {
                        let id = hv.speak(sample, None, Some(tag), speed)?;
                        let t = std::time::Instant::now();
                        loop {
                            match hv.state(id) {
                                hostvoice::UttState::Ended => break,
                                hostvoice::UttState::Failed(m) => return Err(m),
                                _ if t.elapsed() > Duration::from_secs(30) => return Err("the browser did not finish speaking".into()),
                                _ => std::thread::sleep(Duration::from_millis(40)),
                            }
                        }
                        Ok(format!("Spoke a sentence in {} with the browser voice.", hostengines::lang_name(tag)))
                    }
                    other => {
                        let wav = svc.synthesize_with_opts(sample, &voice::SpeakOpts { voice: None, speed, lang: Some(tag.to_string()) })?;
                        let mut pl = crate::players().play(&v.play_cmd, &wav)?;
                        pl.wait(Duration::from_secs(20));
                        Ok(format!("Synthesised a sentence with {} and played it.", other.label()))
                    }
                }
            })())
        });
    }

    /// Open the voice helper (a tab of the default browser that speaks) or disconnect it.
    pub(super) fn vo_helper_toggle(&mut self, p: &dyn VoicePort) {
                if !p.may_use_voice() {
            p.audit("voice.denied", "helper");
            self.fail("Voice is not enabled for your account.");
            return;
        }
        if matches!(p.host_tts().helper_state(), Some(iyakki_shared::voicehelper::HelperState::Waiting | iyakki_shared::voicehelper::HelperState::Connected)) {
            p.host_tts().helper_stop();
            p.audit("voice.helper", "disconnected");
            self.ok("Voice helper disconnected.");
            return;
        }
        match p.host_tts().helper_open(HostVoice::current(), None) {
            Ok(_) => {
                p.audit("voice.helper", "opened");
                self.ok("Opening the voice helper in your browser. Click the button on that page once to allow sound.");
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_voice(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn VoicePort) {
        // everything is kept inside the page's room: when the controls take most of it (a narrow window, big text) the whole page scrolls
        g.clipped(a, |g| self.render_voice_inner(g, a, s, p));
    }

    pub(super) fn render_voice_inner(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn VoicePort) {
        let svc = p.voice_ops();
        let v = svc.voice_settings();
        let admin = p.is_admin();
        let hv = HostVoice::current();
        p.host_tts().ensure();
        let report = p.host_tts().report();
        let engine = self.vo_engine(p);
        let tag = self.vo_tag();
        let fs = 14.0 * s;
        let bs = 13.0 * s;
        // ---- the controls, always in view
        let (status, col) = match (&engine, p.agent().helpers_b7().tts_status(&v, p.may_use_voice(), &std::env::var("PATH").unwrap_or_default(), &hv, if v.tts.is_none() { p.host_tts().auto_engine(Some(tag)) } else { None }.as_ref())) {
            (_, st::TtsStatus::Ready { engine }) => (format!("Reading aloud with {engine}."), OK),
            (None, _) if p.host_tts().scanning() || (p.host_tts().is_enabled() && report.is_none()) => ("Looking for speech engines on this computer…".to_string(), MUTED),
            (_, other) => (p.agent().helpers_b7().tts_status_text(&other), ERR),
        };
        // when the controls left too little room for the lists below them, the controls scroll away with the lists
        let tight = self.vo_tight;
        let off = if tight { self.vo_scroll } else { 0.0 };
        let mut y = a.y - off;
        for l in g.wrap(15.0 * s, &status, Face::Bold, a.w).into_iter().take(2) {
            g.text(a.x, y, 15.0 * s, col, &l, Face::Bold);
            y += 20.0 * s;
        }
        y += 4.0 * s;
        let engine_name = match (&v.tts, &engine) {
            (Some(e), _) => e.label(),
            (None, Some(SpeechEngine::Browser)) => "automatic: browser voice".to_string(),
            (None, Some(e)) => format!("automatic: {}", e.label()),
            (None, None) => "automatic: none found".to_string(),
        };
        let voice_name = match &engine {
            Some(SpeechEngine::Host { voice: Some(id), .. }) => std::path::Path::new(id).file_stem().map_or(id.clone(), |x| x.to_string_lossy().to_string()),
            Some(SpeechEngine::Host { .. }) => "automatic".to_string(),
            _ => "set by the engine".to_string(),
        };
        let chips: Vec<(String, char)> = vec![
            (format!("Engine: {} ▾", g.fit(bs, &engine_name, Face::Bold, (a.w * 0.6).max(90.0 * s))), 'e'),
            (format!("Voice: {} ▾", g.fit(bs, &voice_name, Face::Bold, (a.w * 0.4).max(80.0 * s))), 'g'),
            (format!("Language: {} ▾", VO_LANGS[self.vo_lang % VO_LANGS.len()]), 'l'),
            (format!("Speed: {}% ▾", v.speed_pct), 'f'),
            (format!("Highlight: {} ▾", v.highlight.label()), 'h'),
            (format!("Word timing: {} ▾", if v.align_words { "from the sound" } else { "estimated" }), 'w'),
        ];
        y += vo_flow(g, a.x, y, a.w, bs, &chips, |c| if matches!(c, 'e' | 'g' | 'f' | 'h' | 'w') && !admin { DIM } else { ACCENT }, s);
        let helper = p.host_tts().helper_state();
        let helper_on = matches!(helper, Some(iyakki_shared::voicehelper::HelperState::Waiting | iyakki_shared::voicehelper::HelperState::Connected));
        let acts: Vec<(String, char)> = vec![
            ("Test voice".into(), 'y'),
            ("Re-check engines".into(), 'k'),
            (if helper_on { "Disconnect voice helper".into() } else { "Open voice helper in your browser".into() }, 'o'),
        ];
        y += vo_flow(g, a.x, y, a.w, bs, &acts, |c| if c == 'y' { ACCENT } else { TEXT }, s);
        let helper_line = match &helper {
            Some(iyakki_shared::voicehelper::HelperState::Connected) => "Voice helper connected: your browser is speaking for IYAKKI.".to_string(),
            Some(iyakki_shared::voicehelper::HelperState::Waiting) => "Voice helper: waiting for your browser to open the page (click the button on it once).".to_string(),
            Some(iyakki_shared::voicehelper::HelperState::Closed(why)) => format!("Voice helper closed: {why}"),
            None => "Voice helper: reads aloud with your web browser's voices when this window has none (nothing leaves this computer).".to_string(),
        };
        for l in g.wrap(12.0 * s, &helper_line, Face::Sans, a.w).into_iter().take(2) {
            g.text(a.x, y, 12.0 * s, if matches!(helper, Some(iyakki_shared::voicehelper::HelperState::Connected)) { OK } else { MUTED }, &l, Face::Sans);
            y += 17.0 * s;
        }
        y += 8.0 * s;
        // ---- the rest scrolls
        let rest = R::new(a.x, y, a.w, (a.y + a.h - y).max(40.0 * s));
        let controls_h = y + off - a.y;
        self.vo_tight = a.y + a.h - (y + off) < 160.0 * s;
        let mut yy = if tight { rest.y } else { rest.y - self.vo_scroll };
        let mut used = 0.0f32;
        let put = |g: &mut Gfx, yy: &mut f32, used: &mut f32, text: &str, size: f32, col: [u8; 4], face: Face, gap: f32| {
            for l in g.wrap(size, text, face, rest.w) {
                g.text(rest.x, *yy, size, col, &l, face);
                *yy += size * 1.45;
                *used += size * 1.45;
            }
            *yy += gap;
            *used += gap;
        };
        g.clipped(if tight { a } else { rest }, |g| {
            put(g, &mut yy, &mut used, "Speech engines found on this computer", 15.0 * s, TEXT, Face::Bold, 2.0 * s);
            match &report {
                None if p.host_tts().is_enabled() => put(g, &mut yy, &mut used, "Looking… (this runs once in the background)", fs, MUTED, Face::Sans, 6.0 * s),
                None => put(g, &mut yy, &mut used, "Looking for engines is off in this program.", fs, MUTED, Face::Sans, 6.0 * s),
                Some(r) => {
                    for (line, col, size) in vo_engine_lines(r, tag) {
                        put(g, &mut yy, &mut used, &line, size * s, col, Face::Sans, 3.0 * s);
                    }
                    put(g, &mut yy, &mut used, "", 4.0, DIM, Face::Sans, 4.0 * s);
                }
            }
            let quick = Self::vo_quick_list(p);
            if !quick.is_empty() {
                put(g, &mut yy, &mut used, "Speech from the providers you set up under Models (press the number)", fs, MUTED, Face::Sans, 2.0 * s);
                let items: Vec<(String, char)> = quick.iter().enumerate().take(6).map(|(i, q)| (format!("{} {}", i + 1, q.label), (b'1' + i as u8) as char)).collect();
                let h = vo_flow(g, rest.x, yy, rest.w, bs, &items, |_| if admin { ACCENT } else { DIM }, s);
                yy += h;
                used += h;
            }
            let timing = match (v.align_words, svc.speech_aligner()) {
                (false, _) => "estimated (turned off)".to_string(),
                (true, Some(SpeechEngine::Http { url, model, .. })) => format!("aligned with {model} on {} ({})", iyakki_sec::url_host(&url).unwrap_or_default(), p.agent().helpers_b4().locality_label(&url)),
                (true, _) => "estimated (no speech-to-text server on this computer or allowed)".to_string(),
            };
            let wake_rows = Self::vo_wake_rows(p);
            let lines: [(&str, String, [u8; 4]); 16] = [
                ("Speech-to-text", v.stt.as_ref().map_or("not set up".into(), |e| e.label()), if v.stt.is_some() { OK } else { MUTED }),
                ("Text-to-speech", match &v.tts {
                    Some(e) => e.label(),
                    None => engine.as_ref().map_or("not set up".into(), |e| format!("{} (automatic)", e.label())),
                }, if v.tts.is_some() || engine.is_some() { OK } else { MUTED }),
                ("Record command", v.record_cmd.clone(), TEXT),
                ("Play command", v.play_cmd.clone(), TEXT),
                ("Auto-send speech", if v.auto_send { "on — transcribed text is sent immediately".into() } else { "off — text lands in the input box so you can edit it".into() }, TEXT),
                ("Dictation ends on a pause", if v.auto_stop { format!("on — after {:.1} s of quiet following your speech (u switches off, U changes the length)", v.pause_ms as f32 / 1000.0) } else { "off — you stop it yourself (u switches on)".into() }, TEXT),
                ("Correct my wording", if v.fix_wording { "on — the chat assistant tidies dictated text before it lands in the box (W switches off)".into() } else { "off — dictated text is exactly what was heard (W switches on)".into() }, TEXT),
                ("Read replies aloud", if v.speak_replies { "on".into() } else { "off".into() }, TEXT),
                ("Highlight while reading", v.highlight.label().to_lowercase(), TEXT),
                ("Word timing", timing, TEXT),
                wake_rows[0].clone(),
                wake_rows[1].clone(),
                Self::vo_wake_confirm_row(p),
                Self::vo_cloud_stream_row(p),
                Self::vo_spoken_row(p),
                Self::vo_echo_row(p),
            ];
            yy += 8.0 * s;
            for (k, val, col) in lines.iter() {
                if *k == "Echo cancellation" && admin {
                    // the row is a switch for a finger too: a tap sends the key
                    g.hit(R::new(rest.x, yy - 3.0 * s, rest.w, 26.0 * s), Event::Key(Key::Char('E')), "echo cancellation");
                }
                g.text(rest.x, yy, fs, MUTED, k, Face::Sans);
                let val = g.fit(15.0 * s, val, Face::Mono, rest.w - 190.0 * s);
                g.text(rest.x + 180.0 * s, yy, 15.0 * s, *col, &val, Face::Mono);
                yy += 26.0 * s;
                used += 26.0 * s;
            }
            yy += 8.0 * s;
            let notes = [
                if p.may_use_voice() { "Your account may use the microphone and speaker." } else { "Voice is switched off for your account (an admin can enable it in Users)." },
                "Reading aloud works with no setup when this computer has a speech engine (espeak-ng, speech-dispatcher, piper, macOS say, Windows speech): the best one is used unless you choose another. docs/VOICE.md lists what to install.",
                "Speech-to-text and other engines: an OpenAI-compatible HTTP server (OpenAI, speaches, LocalAI, whisper.cpp server) or a local command (whisper-cli, piper).",
                "Audio never leaves the machine unless you point an engine at a remote server. Recordings are not stored.",
            ];
            for n in notes {
                put(g, &mut yy, &mut used, n, 13.0 * s, DIM, Face::Sans, 4.0 * s);
            }
        });
        self.vo_max = if tight { (controls_h + used + 60.0 * s - a.h).max(0.0) } else { (used + 60.0 * s - rest.h).max(0.0) };
        self.vo_scroll = self.vo_scroll.clamp_safe(0.0, self.vo_max);
    }
}
