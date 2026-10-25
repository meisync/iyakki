//! VOICE: read text aloud with any reading voice that is set up, and dictate (speech to text) into a text box that can be
//! copied. Two areas: READ ALOUD (a text box to paste or type in, a voice and speed, play / pause / stop with the sentence being read
//! lit) and DICTATE (record with a clear indicator, a level meter and a five minute limit; the transcript is editable).
//!
//! The logic is `iyakki_agent::voicedesk`; this file is the state and the keys, `voice_ui.rs` draws it. Text and sound go to another
//! machine only after the person said yes to that machine (the same decision Settings -> Models stores). Recording starts only from
//! a click or key of the person, and the sound is kept in memory only: it is never written to disk.

use crate::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_agent::story::{self as st, Reader, Sink, Status, Synth};
use iyakki_agent::voice::SpeakOpts;
use iyakki_agent::voicedesk::{self as vd, Capture, Dest, Mic, ReadVoice, Recorders, MAX_RECORD_SECS, MAX_TEXT_CHARS};

use super::multiline::MultiLine;
use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::ui::{Event, Key, TextField};

pub const ACCENT: [u8; 4] = [60, 220, 235, 255];

// the click codes (Event::Tab)
pub(super) const T_TAB_READ: usize = 1;
pub(super) const T_TAB_DICT: usize = 2;
pub(super) const T_PLAY: usize = 10;
pub(super) const T_PAUSE: usize = 11;
pub(super) const T_STOP: usize = 12;
pub(super) const T_SEL: usize = 13;
pub(super) const T_PASTE: usize = 14;
pub(super) const T_LOAD: usize = 15;
pub(super) const T_CLEAR: usize = 16;
pub(super) const T_VOICE: usize = 17;
pub(super) const T_SPEED_DN: usize = 18;
pub(super) const T_SPEED_UP: usize = 19;
pub(super) const T_SETUP: usize = 20;
pub(super) const T_SOUND: usize = 21;
pub(super) const T_REC: usize = 30;
pub(super) const T_REC_STOP: usize = 31;
pub(super) const T_COPY: usize = 32;
pub(super) const T_COPY_CLEAR: usize = 33;
pub(super) const T_READ_BACK: usize = 34;
pub(super) const T_MIC: usize = 35;
pub(super) const T_SETTINGS_VOICE: usize = 36;
pub(super) const T_T_CLEAR: usize = 37;
pub(super) const T_YES: usize = 40;
pub(super) const T_NO: usize = 41;

// the areas that take presses
pub(super) const TEXT_AREA: usize = 62_001;
pub(super) const TRANS_AREA: usize = 62_002;
pub(super) const SPEED_AREA: usize = 62_003;
pub(super) const PROGRESS_AREA: usize = 62_004;
pub(super) const PATH_FIELD: usize = 62_005;

/// Speed moves in steps of this much.
const SPEED_STEP: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Read,
    Dictate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    None,
    Text,
    Transcript,
    Path,
}

/// What a yes on the consent card goes on to do.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum After {
    Play { selection: bool },
    Record,
}

pub(super) enum Modal {
    /// Text or sound would go to a machine the person has not agreed to yet.
    Consent { text: String, dest: Dest, then: After },
    VoicePick(super::widgets::Picker),
    MicPick(super::widgets::Picker),
}

/// The things the app uses that tests replace: the recorders, where the sound goes and whether a sound output exists.
#[derive(Clone)]
pub struct Rig {
    pub recorders: Arc<dyn Recorders>,
    /// Where sound goes; None: the play command of Settings -> Voice (or the engine itself, for engines that speak by themselves).
    pub sink: Option<Arc<dyn Sink>>,
    /// Whether a sound output exists (Err: none).
    pub output: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

impl Rig {
    pub fn system() -> Rig {
        Rig {
            recorders: Arc::new(vd::SystemRecorders),
            sink: None,
            output: Arc::new(|| {
                use iyakki_agent::syssounds::CueOut;
                iyakki_agent::syssounds::SystemOut { play_cmd: String::new() }.ready()
            }),
        }
    }
}

/// A reading in progress or just finished.
pub(super) struct Job {
    /// The text that is read (all of it, or the selection).
    pub text: String,
    /// Where it starts in the box (characters): the selection's start.
    pub base: usize,
}

pub struct VoiceApp {
    pub(super) area: Area,
    pub(super) text: MultiLine,
    pub(super) transcript: MultiLine,
    pub(super) path: TextField,
    pub(super) focus: Focus,
    pub(super) voices: Vec<ReadVoice>,
    pub(super) voice_i: usize,
    pub(super) speed: f32,
    pub(super) reader: Reader,
    pub(super) job: Option<Job>,
    /// A line under the controls: (text, ok).
    pub(super) status: Option<(String, bool)>,
    pub(super) modal: Option<Modal>,
    pub(super) rig: Rig,
    // dictation
    pub(super) mics: Vec<Mic>,
    /// 0 is the default microphone, n the nth of `mics`.
    pub(super) mic_i: usize,
    pub(super) product: String,
    pub(super) cap: Option<Box<dyn Capture>>,
    pub(super) level: f32,
    pub(super) peak: f32,
    pub(super) transcribing: bool,
    pub(super) stt_rx: Option<Receiver<Result<String, String>>>,
    pub(super) request: Option<(String, String)>,
    pub(super) ticks: u64,
    pub(super) dirty: bool,
    /// The last geometry drawn, for tests and presses: the speed slider's track.
    pub(super) speed_rect: crate::ui::R,
    pub(super) progress_rect: crate::ui::R,
    /// The last time the voices and microphones were looked at (ticks).
    looked: u64,
}

fn new_box() -> MultiLine {
    let mut m = MultiLine::new(MAX_TEXT_CHARS);
    m.min_rows = 3;
    m.max_rows = 30;
    m
}

impl VoiceApp {
    pub fn new(cx: &AppCx) -> VoiceApp {
        VoiceApp::with_rig(cx, Rig::system())
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> VoiceApp {
        let mut a = VoiceApp {
            area: Area::Read,
            text: new_box(),
            transcript: new_box(),
            path: TextField::new("path of a text file", false),
            focus: Focus::Text,
            voices: Vec::new(),
            voice_i: 0,
            speed: 1.0,
            reader: Reader::new(),
            job: None,
            status: None,
            modal: None,
            rig,
            mics: Vec::new(),
            mic_i: 0,
            product: String::new(),
            cap: None,
            level: 0.0,
            peak: 0.0,
            transcribing: false,
            stt_rx: None,
            request: None,
            ticks: 0,
            dirty: true,
            speed_rect: crate::ui::R::new(0.0, 0.0, 0.0, 0.0),
            progress_rect: crate::ui::R::new(0.0, 0.0, 0.0, 0.0),
            looked: 0,
        };
        a.path.max = 1024;
        a.look(cx);
        a
    }

    // ------------------------------------------------------------------------------------------------ what exists

    /// Look at the voices and the microphones again (every two seconds while the app is open).
    pub(super) fn look(&mut self, cx: &AppCx) {
        let keep = self.voices.get(self.voice_i).map(|v| v.id.clone());
        self.voices = vd::read_voices(&cx.services);
        self.voice_i = keep.and_then(|id| self.voices.iter().position(|v| v.id == id)).unwrap_or(0);
        self.mics = self.rig.recorders.mics();
        if self.mic_i > self.mics.len() {
            self.mic_i = 0;
        }
        self.product = self.rig.recorders.product();
        self.looked = self.ticks;
    }

    pub(super) fn voice(&self) -> Option<&ReadVoice> {
        self.voices.get(self.voice_i)
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn is_reading(&self) -> bool {
        self.reader.is_active()
    }

    pub fn is_recording(&self) -> bool {
        self.cap.is_some()
    }

    pub fn status_text(&self) -> Option<(&str, bool)> {
        self.status.as_ref().map(|(t, ok)| (t.as_str(), *ok))
    }

    pub(super) fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
        self.dirty = true;
    }

    pub(super) fn allowed(&mut self, cx: &AppCx, what: &str) -> bool {
        if cx.session.policy.voice {
            return true;
        }
        cx.audit.record(&cx.session.user, "voice.denied", what);
        self.say("Voice is not enabled for your account.", false);
        false
    }

    // ------------------------------------------------------------------------------------------------ reading

    /// What is read: the selection or all of the box, with where it starts.
    fn read_source(&self, selection: bool) -> (String, usize) {
        if selection {
            if let (Some((a, _)), Some(t)) = (self.text.selection(), self.text.selected_text()) {
                return (t, a);
            }
            return (String::new(), 0);
        }
        (self.text.text.clone(), 0)
    }

    pub(super) fn play(&mut self, cx: &AppCx, selection: bool) {
        if self.reader.is_active() {
            return;
        }
        if !self.allowed(cx, "read") {
            return;
        }
        let (text, base) = self.read_source(selection);
        if text.trim().is_empty() {
            self.say(if selection { "Select some of the text first, then press Read selection." } else { "Nothing to read yet. Paste or type some text." }, false);
            return;
        }
        self.look(cx);
        let Some(voice) = self.voice().cloned() else {
            self.say("No reading voice is set up yet.", false);
            return;
        };
        if let Err(why) = (self.rig.output)() {
            log::info!("voice: no sound output: {why}");
            self.say("No sound output: choose where sound plays in Settings -> Sound.", false);
            return;
        }
        if let Some((dest, text)) = vd::consent_text(&cx.services, &voice.engine, false) {
            self.modal = Some(Modal::Consent { text, dest, then: After::Play { selection } });
            return;
        }
        self.begin(cx, text, base, voice);
    }

    fn plan(&self, cx: &AppCx, voice: &ReadVoice) -> (Arc<dyn Synth>, Arc<dyn Sink>) {
        let opts = SpeakOpts { voice: voice.voice.clone(), speed: self.speed, lang: None };
        let v = cx.services.voice_settings();
        if iyakki_agent::hostengines::speaks_directly(&voice.engine) {
            if let Some(spec) = iyakki_agent::hostengines::Spec::from_setting(&voice.engine) {
                let sink: Arc<dyn Sink> = self.rig.sink.clone().unwrap_or_else(|| Arc::new(iyakki_agent::hostengines::DirectSink { spec, lang: None, speed: self.speed }));
                return (Arc::new(iyakki_agent::hostengines::DirectSynth), sink);
            }
        }
        let path = std::env::var("PATH").unwrap_or_default();
        let play_cmd = iyakki_agent::hostengines::player_command(&v.play_cmd, &path).unwrap_or(v.play_cmd.clone());
        let sink: Arc<dyn Sink> = self.rig.sink.clone().unwrap_or_else(|| Arc::new(st::CommandSink { play_cmd }));
        (Arc::new(vd::ChoiceSynth { svc: cx.services.clone(), engine: voice.engine.clone(), opts }), sink)
    }

    fn begin(&mut self, cx: &AppCx, text: String, base: usize, voice: ReadVoice) {
        let pieces = vd::pieces(&text);
        if pieces.is_empty() {
            self.say("There is nothing to say in this text (only addresses or marks).", false);
            return;
        }
        let (synth, sink) = self.plan(cx, &voice);
        self.reader.set_speed(self.speed);
        self.reader.start(pieces, 0, false, &vd::voice_key(&voice, self.speed), synth, sink);
        cx.audit.record(&cx.session.user, "voice.read", &format!("{} characters with {}", text.chars().count(), voice.id));
        self.job = Some(Job { text, base });
        self.focus = Focus::None;
        self.status = None;
        self.dirty = true;
    }

    pub(super) fn pause_toggle(&mut self) {
        match self.reader.status() {
            Status::Playing | Status::Buffering => self.reader.pause(),
            Status::Paused => self.reader.resume(),
            _ => {}
        }
        self.dirty = true;
    }

    pub(super) fn stop_reading(&mut self) {
        self.reader.stop();
        self.job = None;
        self.text.anchor = None;
        self.dirty = true;
    }

    /// The voice or the speed changed while reading: carry on from this sentence with the new one.
    fn restart_here(&mut self, cx: &AppCx) {
        let (Some(job), true) = (self.job.as_ref(), self.reader.is_active()) else { return };
        let at = self.reader.snapshot().chunk;
        let was_paused = self.reader.status() == Status::Paused;
        let (text, base) = (job.text.clone(), job.base);
        let Some(voice) = self.voice().cloned() else { return };
        let pieces = vd::pieces(&text);
        let (synth, sink) = self.plan(cx, &voice);
        self.reader.set_speed(self.speed);
        self.reader.start(pieces, at, false, &vd::voice_key(&voice, self.speed), synth, sink);
        if was_paused {
            self.reader.pause();
        }
        self.job = Some(Job { text, base });
    }

    pub(super) fn set_speed(&mut self, cx: &AppCx, v: f32) {
        let takes = self.voice().is_none_or(ReadVoice::takes_speed);
        if !takes {
            self.say("This voice has no speed control.", false);
            return;
        }
        let v = ((v / SPEED_STEP).round() * SPEED_STEP).clamp_safe(vd::SPEED_MIN, vd::SPEED_MAX);
        if (v - self.speed).abs() > 0.001 {
            self.speed = v;
            self.restart_here(cx);
            self.dirty = true;
        }
    }

    fn pick_voice(&mut self, cx: &AppCx, label: &str) {
        if let Some(i) = self.voices.iter().position(|v| v.label == label) {
            self.voice_i = i;
            self.restart_here(cx);
            self.dirty = true;
        }
    }

    // ------------------------------------------------------------------------------------------------ dictating

    pub(super) fn start_record(&mut self, cx: &AppCx) {
        if self.cap.is_some() || self.transcribing {
            return;
        }
        if !self.allowed(cx, "record") {
            return;
        }
        self.look(cx);
        if self.mics.is_empty() {
            self.say(vd::no_mic_text_t2(&self.product, vd::t2_mic_hidden(std::path::Path::new("/proc"))), false);
            return;
        }
        let Some(engine) = vd::stt_engine(&cx.services) else {
            self.say("No speech-to-text is set up. Install Whisper with Settings -> Models (Lemonade) or set one in Settings -> Voice.", false);
            return;
        };
        if let Some((dest, text)) = vd::consent_text(&cx.services, &engine, true) {
            self.modal = Some(Modal::Consent { text, dest, then: After::Record });
            return;
        }
        self.record_now(cx);
    }

    fn record_now(&mut self, cx: &AppCx) {
        self.stop_reading(); // do not record the voice that is reading
        let mic = self.mic_i.checked_sub(1).and_then(|i| self.mics.get(i));
        let cmd = vd::record_cmd_for(&cx.services.voice_settings().record_cmd, mic);
        match self.rig.recorders.start(&cmd, MAX_RECORD_SECS) {
            Ok(c) => {
                self.cap = Some(c);
                self.level = 0.0;
                self.peak = 0.0;
                self.status = None;
                cx.audit.record(&cx.session.user, "voice.record", "");
                self.focus = Focus::None;
            }
            Err(e) => self.say(format!("The microphone could not be opened: {e}"), false),
        }
        self.dirty = true;
    }

    /// Stop recording and turn what was said into text, or throw it away.
    pub(super) fn stop_record(&mut self, cx: &AppCx, keep: bool) {
        let Some(cap) = self.cap.take() else { return };
        if !keep {
            drop(cap);
            self.say("Recording cancelled. Nothing was kept.", true);
            return;
        }
        let Some(engine) = vd::stt_engine(&cx.services) else {
            self.say("No speech-to-text is set up.", false);
            return;
        };
        let svc = cx.services.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(cap.stop().and_then(|pcm| vd::transcribe(&svc, &engine, &pcm)));
        });
        self.stt_rx = Some(rx);
        self.transcribing = true;
        self.dirty = true;
    }

    fn finish_transcript(&mut self, r: Result<String, String>) {
        self.transcribing = false;
        self.stt_rx = None;
        match r {
            Ok(t) => {
                if !self.transcript.text.is_empty() && !self.transcript.text.ends_with('\n') {
                    self.transcript.move_to(self.transcript.len(), false);
                    self.transcript.insert("\n");
                }
                self.transcript.move_to(self.transcript.len(), false);
                self.transcript.insert(&t);
                self.say(format!("Transcript ready ({} characters). You can edit it.", t.chars().count()), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn copy_transcript(&mut self, clear: bool) {
        let t = self.transcript.selected_text().filter(|_| !clear).unwrap_or_else(|| self.transcript.text.clone());
        if t.trim().is_empty() {
            self.say("There is no transcript to copy yet.", false);
            return;
        }
        crate::ui::clipboard::copy(&t);
        if clear {
            self.transcript.take();
            self.say(format!("Copied {} characters and cleared the box.", t.chars().count()), true);
        } else {
            self.say(format!("Copied {} characters to the clipboard.", t.chars().count()), true);
        }
    }

    fn read_back(&mut self) {
        if self.transcript.text.trim().is_empty() {
            self.say("There is no transcript to read yet.", false);
            return;
        }
        let t = self.transcript.text.clone();
        self.text.set(&t);
        self.area = Area::Read;
        self.focus = Focus::None;
        self.status = None;
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ the heartbeat

    fn on_tick(&mut self, cx: &AppCx) {
        self.ticks += 1;
        if self.ticks - self.looked >= 60 && self.cap.is_none() && !self.reader.is_active() && self.modal.is_none() {
            self.look(cx);
            self.dirty = true;
        }
        // the sentence being read is lit in the box
        if let Some(job) = &self.job {
            let snap = self.reader.snapshot();
            match snap.status {
                Status::Playing | Status::Buffering | Status::Paused => {
                    let (a, b) = (job.base + snap.range.0, job.base + snap.range.1);
                    if self.text.selection() != Some((a, b)) {
                        self.text.anchor = Some(a);
                        self.text.move_to(b, true);
                    }
                }
                Status::Done => {
                    self.text.anchor = None;
                    self.job = None;
                    self.say("Finished reading.", true);
                }
                Status::Failed(e) => {
                    self.text.anchor = None;
                    self.job = None;
                    self.say(e, false);
                }
                Status::Idle => {}
            }
        }
        // the microphone
        if self.cap.is_some() {
            let (alive, full, secs, level) = {
                let c = self.cap.as_mut().expect("recording");
                (c.is_alive(), c.is_full(), c.elapsed().as_secs(), c.level())
            };
            self.level = level.max(self.level * 0.85);
            self.peak = level.max(self.peak * 0.97);
            if !alive {
                // the program ended by itself (no device, busy): what it said is the error, anything it did capture is used
                self.stop_record(cx, true);
            } else if full || secs >= MAX_RECORD_SECS as u64 {
                self.stop_record(cx, true);
                self.say("Reached the five minute limit; turning what was said into text.", true);
            }
        } else {
            self.level *= 0.8;
        }
        if let Some(rx) = &self.stt_rx {
            if let Ok(r) = rx.try_recv() {
                self.finish_transcript(r);
            }
        }
    }

    pub(super) fn animating(&self) -> bool {
        self.cap.is_some() || self.transcribing || self.reader.is_active() || self.level > 0.01
    }

    // ------------------------------------------------------------------------------------------------ files

    fn load_file(&mut self, cx: &AppCx) {
        let input = self.path.value.trim().to_string();
        self.focus = Focus::Text;
        if input.is_empty() {
            self.say("Type the path of a text file.", false);
            return;
        }
        let roots = iyakki_agent::scope::FileRoots::new(&cx.session.policy, &cx.session.user, cx.services.dir());
        let r = roots.resolve(&roots.home, &input).and_then(|real| {
            iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &real)?;
            let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
            if meta.len() > 2_000_000 {
                return Err("That file is too large to read aloud (over 2 MB).".into());
            }
            let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
            let t = String::from_utf8(bytes).map_err(|_| "That file is not text.".to_string())?;
            cx.audit.record(&cx.session.user, "voice.load", &real.to_string_lossy());
            Ok(t)
        });
        match r {
            Ok(t) => {
                self.text.set(&t);
                self.path.set("");
                let n = self.text.len();
                match self.text.cut {
                    Some(_) => self.say(format!("Loaded the first {} characters; the rest was left out.", super::multiline::thousands(n)), false),
                    None => self.say(format!("Loaded {} characters.", super::multiline::thousands(n)), true),
                }
            }
            Err(e) => self.say(e, false),
        }
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    fn click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_TAB_READ => self.switch(Area::Read),
            T_TAB_DICT => self.switch(Area::Dictate),
            T_PLAY => self.play(cx, false),
            T_PAUSE => self.pause_toggle(),
            T_STOP => self.stop_reading(),
            T_SEL => self.play(cx, true),
            T_PASTE => {
                if let Some(t) = crate::ui::clipboard::latest() {
                    self.paste(&t);
                } else {
                    self.say("The clipboard has no text.", false);
                }
            }
            T_LOAD => {
                self.focus = Focus::Path;
            }
            T_CLEAR => {
                self.stop_reading();
                self.text.take();
                self.status = None;
                self.focus = Focus::Text;
            }
            T_VOICE => {
                self.look(cx);
                let items = self.voices.iter().map(|v| v.label.clone()).collect();
                self.modal = Some(Modal::VoicePick(super::widgets::Picker::new("Reading voice", items)));
            }
            T_SPEED_DN => self.set_speed(cx, self.speed - SPEED_STEP),
            T_SPEED_UP => self.set_speed(cx, self.speed + SPEED_STEP),
            T_SETUP => self.request = Some(("quickinstall".into(), "kokoro".into())),
            T_SOUND => self.request = Some(("settings".into(), "tab:sound".into())),
            T_SETTINGS_VOICE => self.request = Some(("settings".into(), "voice".into())),
            T_REC => self.start_record(cx),
            T_REC_STOP => self.stop_record(cx, true),
            T_COPY => self.copy_transcript(false),
            T_COPY_CLEAR => self.copy_transcript(true),
            T_READ_BACK => self.read_back(),
            T_T_CLEAR => {
                self.transcript.take();
                self.focus = Focus::Transcript;
            }
            T_MIC => {
                self.look(cx);
                let mut items = vec!["Default microphone".to_string()];
                items.extend(self.mics.iter().map(|m| m.label.clone()));
                self.modal = Some(Modal::MicPick(super::widgets::Picker::new("Microphone", items)));
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn switch(&mut self, a: Area) {
        if self.area != a {
            self.area = a;
            self.status = None;
            self.focus = if a == Area::Read { Focus::Text } else { Focus::None };
        }
    }

    fn paste(&mut self, t: &str) {
        match self.focus {
            Focus::Path => {
                self.path.paste(t);
            }
            Focus::Transcript => {
                self.transcript.insert(t);
            }
            Focus::Text | Focus::None if self.area == Area::Read => {
                self.stop_reading();
                self.text.insert(t);
                self.focus = Focus::Text;
                let n = self.text.len();
                if self.text.cut.is_none() {
                    self.say(format!("Pasted. {} characters ready to read.", super::multiline::thousands(n)), true);
                }
            }
            _ => {
                self.transcript.insert(t);
                self.focus = Focus::Transcript;
            }
        }
        self.dirty = true;
    }

    fn consent_yes(&mut self, cx: &AppCx) {
        let Some(Modal::Consent { dest, then, .. }) = self.modal.take() else { return };
        cx.audit.record(&cx.session.user, "voice.remote_ok", &dest.server);
        if let Err(e) = cx.services.allow_speech_server(&dest.server) {
            self.say(e, false);
            return;
        }
        match then {
            After::Play { selection } => self.play(cx, selection),
            After::Record => self.start_record(cx),
        }
    }

    fn modal_event(&mut self, ev: &Event, cx: &AppCx) {
        match self.modal.take() {
            Some(Modal::Consent { text, dest, then }) => {
                let yes = matches!(ev, Event::Key(Key::Char('y' | 'Y')) | Event::Tab(T_YES));
                let no = matches!(ev, Event::Key(Key::Esc | Key::Char('n' | 'N')) | Event::Tab(T_NO));
                self.modal = Some(Modal::Consent { text, dest, then });
                if yes {
                    self.consent_yes(cx);
                } else if no {
                    self.modal = None;
                    self.say("Nothing was sent. Pick a voice on this computer, or say yes to use that one.", true);
                }
            }
            Some(Modal::VoicePick(mut p)) => match p.handle_event(ev) {
                super::widgets::PickEvent::Chosen(label) => self.pick_voice(cx, &label),
                super::widgets::PickEvent::Cancel => {}
                super::widgets::PickEvent::None => self.modal = Some(Modal::VoicePick(p)),
            },
            Some(Modal::MicPick(mut p)) => match p.handle_event(ev) {
                super::widgets::PickEvent::Chosen(label) => {
                    self.mic_i = if label == "Default microphone" { 0 } else { self.mics.iter().position(|m| m.label == label).map_or(0, |i| i + 1) };
                }
                super::widgets::PickEvent::Cancel => {}
                super::widgets::PickEvent::None => self.modal = Some(Modal::MicPick(p)),
            },
            None => {}
        }
        self.dirty = true;
    }

    fn boxed(&mut self) -> Option<&mut MultiLine> {
        match self.focus {
            Focus::Text => Some(&mut self.text),
            Focus::Transcript => Some(&mut self.transcript),
            _ => None,
        }
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        // keys that work everywhere
        match k {
            Key::F(2) => {
                self.switch(Area::Read);
                return Flow::Stay;
            }
            Key::F(3) => {
                self.switch(Area::Dictate);
                return Flow::Stay;
            }
            Key::Ctrl('\n' | '\r') | Key::Ctrl('p' | 'P') => {
                match (self.area, self.cap.is_some()) {
                    (Area::Dictate, true) => self.stop_record(cx, true),
                    (Area::Dictate, false) => self.start_record(cx),
                    (Area::Read, _) if self.reader.is_active() => self.pause_toggle(),
                    (Area::Read, _) => self.play(cx, false),
                }
                return Flow::Stay;
            }
            Key::Esc => {
                if self.cap.is_some() {
                    self.stop_record(cx, false);
                } else if self.reader.is_active() {
                    self.stop_reading();
                } else if self.focus == Focus::Path {
                    self.path.set("");
                    self.focus = Focus::Text;
                } else if self.focus != Focus::None {
                    self.focus = Focus::None;
                } else {
                    return Flow::Back;
                }
                return Flow::Stay;
            }
            Key::Ctrl('o' | 'O') if self.area == Area::Read => {
                self.focus = Focus::Path;
                return Flow::Stay;
            }
            _ => {}
        }
        if self.focus == Focus::Path {
            match k {
                Key::Enter => self.load_file(cx),
                other => {
                    self.path.handle(other);
                }
            }
            return Flow::Stay;
        }
        // copy, cut and select all in the boxes
        if let Some(b) = self.boxed() {
            match k {
                Key::Ctrl('a' | 'A') => {
                    b.select_all();
                    return Flow::Stay;
                }
                Key::Ctrl('c' | 'C') => {
                    if let Some(t) = b.selected_text() {
                        crate::ui::clipboard::copy(&t);
                    }
                    return Flow::Stay;
                }
                Key::Ctrl('x' | 'X') => {
                    if let Some(t) = b.selected_text() {
                        crate::ui::clipboard::copy(&t);
                        b.key(&Key::Delete);
                    }
                    return Flow::Stay;
                }
                _ => {}
            }
            let changed = b.key(k);
            if changed && self.focus == Focus::Text {
                self.stop_reading();
            }
            return Flow::Stay;
        }
        // no box has the keyboard: single keys are shortcuts
        match (k, self.area) {
            (Key::Char('r'), _) => self.switch(Area::Read),
            (Key::Char('d'), _) => self.switch(Area::Dictate),
            (Key::Char(' '), Area::Read) => {
                if self.reader.is_active() {
                    self.pause_toggle();
                } else {
                    self.play(cx, false);
                }
            }
            (Key::Char('s'), Area::Read) => self.stop_reading(),
            (Key::Char('v'), Area::Read) => self.click(T_VOICE, cx),
            (Key::Char('+' | '='), Area::Read) => self.click(T_SPEED_UP, cx),
            (Key::Char('-'), Area::Read) => self.click(T_SPEED_DN, cx),
            (Key::Char('e'), Area::Read) | (Key::Enter | Key::Tab, Area::Read) => self.focus = Focus::Text,
            (Key::Char(' ') | Key::Enter, Area::Dictate) => {
                if self.cap.is_some() {
                    self.stop_record(cx, true);
                } else {
                    self.start_record(cx);
                }
            }
            (Key::Char('m'), Area::Dictate) => self.click(T_MIC, cx),
            (Key::Char('c'), Area::Dictate) => self.copy_transcript(false),
            (Key::Char('x'), Area::Dictate) => self.copy_transcript(true),
            (Key::Char('b'), Area::Dictate) => self.read_back(),
            (Key::Char('e') | Key::Tab, Area::Dictate) => self.focus = Focus::Transcript,
            _ => {}
        }
        self.dirty = true;
        Flow::Stay
    }
}

impl App for VoiceApp {
    fn title(&self) -> &'static str {
        "Voice"
    }

    fn icon(&self) -> Icon {
        Icon::Speaker
    }

    fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    fn hint(&self) -> String {
        match self.area {
            Area::Read => "F2 Read aloud · F3 Dictate · Ctrl+Enter play or pause · Ctrl+O open a file · Esc stop or back".into(),
            Area::Dictate => "F2 Read aloud · F3 Dictate · Ctrl+Enter record or stop · Esc cancel the recording or go back".into(),
        }
    }

    fn help(&self) -> Vec<String> {
        [
            "F2 / F3: Read aloud / Dictate (or r / d when no box has the keyboard)",
            "Read aloud: paste with Ctrl+V or type; Ctrl+Enter or Space plays and pauses; Esc stops; Ctrl+O loads a text file",
            "Select some text and press Read selection to read only that; click the progress bar to jump",
            "v choose the voice · + and - change the speed (when the voice has one)",
            "Dictate: Ctrl+Enter or Enter starts and stops recording (never without your click); Esc cancels and keeps nothing",
            "Dictate: c copy the transcript · x copy and clear · b read it back · m choose the microphone",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    fn wants_redraw(&mut self) -> bool {
        if self.animating() {
            self.ticks % 3 == 0
        } else {
            std::mem::take(&mut self.dirty)
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.on_tick(cx);
            return Flow::Stay;
        }
        if self.modal.is_some() {
            self.modal_event(ev, cx);
            return Flow::Stay;
        }
        match ev {
            Event::Key(k) => return self.key(k, cx),
            Event::Tab(n) => {
                if matches!(*n, T_PASTE | T_LOAD | T_CLEAR) || *n == T_T_CLEAR {
                    // these keep or take the keyboard themselves
                } else if self.focus == Focus::Path {
                    self.focus = Focus::Text;
                }
                self.click(*n, cx);
            }
            Event::Paste(t) => self.paste(t),
            Event::AreaAt(TEXT_AREA, x, y) => {
                self.focus = Focus::Text;
                self.text.press(*x, *y);
            }
            Event::AreaAt(TRANS_AREA, x, y) => {
                self.focus = Focus::Transcript;
                self.transcript.press(*x, *y);
            }
            Event::AreaAt(SPEED_AREA, x, _) => {
                let r = self.speed_rect;
                let f = (*x / r.w.max(1.0)).clamp(0.0, 1.0);
                self.set_speed(cx, vd::SPEED_MIN + f * (vd::SPEED_MAX - vd::SPEED_MIN));
            }
            Event::AreaAt(PROGRESS_AREA, x, _) => {
                let r = self.progress_rect;
                let n = self.reader.snapshot().chunks;
                if self.reader.is_active() && n > 0 {
                    let f = (*x / r.w.max(1.0)).clamp(0.0, 1.0);
                    self.reader.seek((f * n as f32) as usize);
                }
            }
            Event::FocusAt(PATH_FIELD, x) => {
                self.focus = Focus::Path;
                self.path.click(*x);
            }
            Event::Scroll(d) => {
                let _ = if self.area == Area::Read { self.text.wheel(*d) } else { self.transcript.wheel(*d) };
            }
            _ => {}
        }
        self.dirty = true;
        Flow::Stay
    }

    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, cx: &AppCx, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;
