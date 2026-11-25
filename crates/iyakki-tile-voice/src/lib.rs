//! VOICE: read text aloud with any reading voice that is set up, and dictate (speech to text) into a text box that can be
//! copied. Two areas: READ ALOUD (a text box to paste or type in, a voice and speed, play / pause / stop with the sentence being read
//! lit) and DICTATE (record with a clear indicator, a level meter and a five minute limit; the transcript is editable).
//!
//! The logic is `iyakki_agent::voicedesk`; this file is the state and the keys, `voice_ui.rs` draws it. Text and sound go to another
//! machine only after the person said yes to that machine (the same decision Settings -> Models stores). Recording starts only from
//! a click or key of the person, and the sound is kept in memory only: it is never written to disk.

use iyakki_uikit::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_voice_model::dictfix;
use iyakki_story_model::{Sink, Status, Synth};
pub type Reader = std::sync::Arc<dyn iyakki_story_model::StoryReader>;
use iyakki_voice_model::voice::SpeakOpts;
use iyakki_voice_model::voicedesk::{Mic, MAX_RECORD_SECS, MAX_TEXT_CHARS};
use iyakki_voice_model::voicedesk::{Capture, Recorders};
use iyakki_voice_model::voicedesk::{self as vd, Dest, ReadVoice};

use iyakki_uikit::multiline::MultiLine;
use crate::voice_mine::{Mine, T_TAB_MINE};
use crate::voice_rec::{Rec, T_TAB_REC};
use crate::port::VoicePort;
use iyakki_uikit::gfx::Icon;
use iyakki_uikit::ui::{Event, Key, TextField};

pub mod port;
pub mod voice_mine;
pub mod voice_rec;
pub mod voice_rec_ui;
pub mod voice_ui;
pub use port::Fixer;

/// What a key or a click did to the screen: stay, or go back to the apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}


pub const ACCENT: [u8; 4] = [60, 220, 235, 255];

// the click codes (Event::Tab)
pub const T_TAB_READ: usize = 1;
pub const T_TAB_DICT: usize = 2;
pub const T_PLAY: usize = 10;
pub const T_PAUSE: usize = 11;
pub const T_STOP: usize = 12;
pub const T_SEL: usize = 13;
pub const T_PASTE: usize = 14;
pub const T_LOAD: usize = 15;
pub const T_CLEAR: usize = 16;
/// Browse next to the box that opens a text file.
pub const T_BROWSE: usize = 49_921;
pub const T_VOICE: usize = 17;
pub const T_SPEED_DN: usize = 18;
pub const T_SPEED_UP: usize = 19;
pub const T_SETUP: usize = 20;
pub const T_SOUND: usize = 21;
pub const T_REC: usize = 30;
pub const T_REC_STOP: usize = 31;
pub const T_COPY: usize = 32;
pub const T_COPY_CLEAR: usize = 33;
pub const T_READ_BACK: usize = 34;
pub const T_MIC: usize = 35;
pub const T_SETTINGS_VOICE: usize = 36;
pub const T_T_CLEAR: usize = 37;
pub const T_KEEP: usize = 38;
pub const T_FIX: usize = 39;
pub const T_FIX_ORIG: usize = 42;
pub const T_YES: usize = 40;
pub const T_NO: usize = 41;

// the areas that take presses
pub const TEXT_AREA: usize = 62_001;
pub const TRANS_AREA: usize = 62_002;
pub const SPEED_AREA: usize = 62_003;
pub const PROGRESS_AREA: usize = 62_004;
pub const PATH_FIELD: usize = 62_005;

/// Speed moves in steps of this much.
pub const SPEED_STEP: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Read,
    Dictate,
    /// The voice recorder: Record / Stop saves a WAV; the list plays, renames and deletes.
    Record,
    /// My IYAKKI, the push-to-talk voice assistant (F5).
    Mine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    None,
    Text,
    Transcript,
    Path,
    /// The Assistant pane's instruction line.
    Assist,
}

/// What a yes on the consent card goes on to do.
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    Play { selection: bool },
    Record,
    /// Dictated text would go to the chat model on another machine (Correct my wording); a no records without the correction.
    RecordFix,
    /// Text would go to the chat model on another machine (the Assistant pane); a no sends nothing.
    Revise,
    /// One spoken instruction for the Assistant pane (the sound goes to the speech engine on another machine).
    RecordSay,
}

pub enum Modal {
    /// Text or sound would go to a machine the person has not agreed to yet.
    Consent { text: String, dest: Dest, then: After },
    VoicePick(iyakki_uikit::widgets::Picker),
    MicPick(iyakki_uikit::widgets::Picker),
    /// The language Translate goes into.
    LangPick(iyakki_uikit::widgets::Picker),
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

/// What the speech worker hands back: the speech engine's text, or why there is none. (The wording correction comes later, per chunk.)
pub type SttDone = Result<String, String>;

/// Empty results in a row with Keep listening on before it stops by itself.
pub const EMPTY_LIMIT: u32 = 5;
/// The one line when it stops because only noise was heard.
pub const NOISE_STOP: &str = "Stopped listening: I kept hearing noise but no words. Check the microphone level or press Record to start again";

/// A reading in progress or just finished.
pub struct Job {
    /// The text that is read (all of it, or the selection).
    pub text: String,
    /// Where it starts in the box (characters): the selection's start.
    pub base: usize,
}

impl Drop for VoiceApp {
    fn drop(&mut self) {
        // leaving the tile ends its recording: the wake listener may have the microphone back
        (self.tile_rec)(false);
    }
}

pub struct VoiceApp {
    pub area: Area,
    pub text: MultiLine,
    pub transcript: MultiLine,
    pub path: TextField,
    pub path_pick: iyakki_uikit::pathpick::PathPick,
    pub focus: Focus,
    pub voices: Vec<ReadVoice>,
    pub voice_i: usize,
    pub speed: f32,
    pub reader: Reader,
    pub job: Option<Job>,
    /// A line under the controls: (text, ok).
    pub status: Option<(String, bool)>,
    pub modal: Option<Modal>,
    pub rig: Rig,
    /// Tells the wake listener whether this tile records (it gives the microphone up while it does).
    pub tile_rec: Arc<dyn Fn(bool) + Send + Sync>,
    // dictation
    pub mics: Vec<Mic>,
    /// 0 is the default microphone, n the nth of `mics`.
    pub mic_i: usize,
    /// `mic_i` has been set from the saved choice.
    pub mic_seeded: bool,
    pub product: String,
    pub cap: Option<Box<dyn Capture>>,
    pub level: f32,
    pub peak: f32,
    /// Ends a dictation by itself after a pause in the talking.
    pub pause: Option<iyakki_voice_model::voicedesk::PauseDetector>,
    /// The last dictation ended on a pause (not by the person's Stop): with Keep listening on, the next one starts by itself.
    pub ended_on_pause: bool,
    pub transcribing: bool,
    pub stt_rx: Option<Receiver<SttDone>>,
    /// The last chunks of dictated text and their corrections (Correct my wording, in the background; see `voice_fix.rs`).
    pub fixes: Vec<fix::Chunk>,
    pub fix_serial: u64,
    pub fix_tx: std::sync::mpsc::Sender<(u64, dictfix::Fixed)>,
    pub fix_rx: Receiver<(u64, dictfix::Fixed)>,
    /// Why the last correction kept the raw words; stays on the Correct-my-wording row until the next answer or Clear.
    pub fix_note: Option<String>,
    /// Empty speech-to-text results in a row (Keep listening stops after [`EMPTY_LIMIT`]).
    pub empty_streak: u32,
    /// The person said no to sending text to the assistant's machine: no correction until they switch it off and on.
    pub fix_declined: bool,
    /// The Assistant pane beside the transcript.
    pub assist: assist::Assist,
    /// The recorder (Record mode).
    pub rec: Rec,
    /// My IYAKKI (the Assistant tab).
    pub mine: Mine,
    pub request: Option<(String, String)>,
    pub ticks: u64,
    pub dirty: bool,
    /// The last geometry drawn, for tests and presses: the speed slider's track.
    pub speed_rect: iyakki_uikit::ui::R,
    pub progress_rect: iyakki_uikit::ui::R,
    /// The last time the voices and microphones were looked at (ticks).
    pub looked: u64,
}

pub fn new_box() -> MultiLine {
    let mut m = MultiLine::new(MAX_TEXT_CHARS);
    m.min_rows = 3;
    m.max_rows = 30;
    m
}

/// The reading voice and the sound of a core that serves neither to this program (the UI over the socket, D-SPLIT-03-QUIET): every piece fails in words
/// and nothing is played.
pub struct NotServedVoice;

impl Synth for NotServedVoice {
    fn synth(&self, _text: &str) -> Result<Vec<u8>, String> {
        Err("The reading voice is not served over the UI socket yet.".into())
    }
}

impl Sink for NotServedVoice {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn iyakki_story_model::Playing>, String> {
        Err("Sound for reading is not served over the UI socket yet.".into())
    }
}

impl VoiceApp {
    /// The tile over any port (a fake in tests).
    pub fn with_port(cx: &dyn VoicePort, rig: Rig) -> VoiceApp {
        let fixch = fix::fix_channel();
        let mut a = VoiceApp {
            area: Area::Read,
            text: new_box(),
            transcript: new_box(),
            path: TextField::new("path of a text file", false),
            path_pick: Default::default(),
            focus: Focus::Text,
            voices: Vec::new(),
            voice_i: 0,
            speed: 1.0,
            reader: cx.new_reader(),
            job: None,
            status: None,
            modal: None,
            rig,
            tile_rec: cx.tile_recording(),
            mics: Vec::new(),
            mic_i: 0,
            mic_seeded: false,
            product: String::new(),
            cap: None,
            level: 0.0,
            peak: 0.0,
            pause: None,
            ended_on_pause: false,
            transcribing: false,
            stt_rx: None,
            fixes: Vec::new(),
            fix_serial: 0,
            fix_tx: fixch.0,
            fix_rx: fixch.1,
            fix_note: None,
            empty_streak: 0,
            fix_declined: false,
            assist: assist::Assist::new(),
            rec: Rec::new(),
            mine: Mine::new(),
            request: None,
            ticks: 0,
            dirty: true,
            speed_rect: iyakki_uikit::ui::R::new(0.0, 0.0, 0.0, 0.0),
            progress_rect: iyakki_uikit::ui::R::new(0.0, 0.0, 0.0, 0.0),
            looked: 0,
        };
        a.path.max = 1024;
        a.look(cx);
        a
    }

    // ------------------------------------------------------------------------------------------------ what exists

    /// Look at the voices and the microphones again (every two seconds while the app is open).
    pub fn look(&mut self, cx: &dyn VoicePort) {
        let keep = self.voices.get(self.voice_i).map(|v| v.id.clone());
        self.voices = cx.voicedesk().read_voices();
        self.voice_i = keep.and_then(|id| self.voices.iter().position(|v| v.id == id)).unwrap_or(0);
        self.mics = self.rig.recorders.mics();
        if !self.mic_seeded {
            // the choice is kept in the voice settings and shared with the Browser (one microphone for both)
            self.mic_seeded = true;
            let saved = cx.voice_settings().mic;
            self.mic_i = if saved.is_empty() { 0 } else { self.mics.iter().position(|m| m.id == saved).map_or(0, |i| i + 1) };
        }
        if self.mic_i > self.mics.len() {
            self.mic_i = 0;
        }
        self.product = self.rig.recorders.product();
        self.looked = self.ticks;
    }

    pub fn voice(&self) -> Option<&ReadVoice> {
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

    pub fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
        self.dirty = true;
    }

    pub fn allowed(&mut self, cx: &dyn VoicePort, what: &str) -> bool {
        if cx.voice_allowed() {
            return true;
        }
        cx.record("voice.denied", what);
        self.say("Voice is not enabled for your account.", false);
        false
    }

    // ------------------------------------------------------------------------------------------------ reading

    /// What is read: the selection or all of the box, with where it starts.
    pub fn read_source(&self, selection: bool) -> (String, usize) {
        if selection {
            if let (Some((a, _)), Some(t)) = (self.text.selection(), self.text.selected_text()) {
                return (t, a);
            }
            return (String::new(), 0);
        }
        (self.text.text.clone(), 0)
    }

    pub fn play(&mut self, cx: &dyn VoicePort, selection: bool) {
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
        if let Some((dest, text)) = cx.voicedesk().consent_text(&voice.engine, false) {
            self.modal = Some(Modal::Consent { text, dest, then: After::Play { selection } });
            return;
        }
        self.begin(cx, text, base, voice);
    }

    pub fn plan(&self, cx: &dyn VoicePort, voice: &ReadVoice) -> (Arc<dyn Synth>, Arc<dyn Sink>) {
        let opts = SpeakOpts { voice: voice.voice.clone(), speed: self.speed, lang: None };
        let v = cx.voice_settings();
        if cx.speaks_directly(&voice.engine) {
            if let Some(d) = cx.direct_voice(&voice.engine) {
                let sink: Arc<dyn Sink> = self.rig.sink.clone().unwrap_or_else(|| d.sink(None, None, self.speed));
                return (d.synth(), sink);
            }
        }
        let path = std::env::var("PATH").unwrap_or_default();
        let play_cmd = cx.player_command(&v.play_cmd, &path).unwrap_or(v.play_cmd.clone());
        let sink: Arc<dyn Sink> = self.rig.sink.clone().unwrap_or_else(|| cx.voicedesk().command_sink(play_cmd).unwrap_or_else(|| Arc::new(NotServedVoice)));
        (cx.voicedesk().choice_synth(voice.engine.clone(), opts).unwrap_or_else(|| Arc::new(NotServedVoice)), sink)
    }

    pub fn begin(&mut self, cx: &dyn VoicePort, text: String, base: usize, voice: ReadVoice) {
        let pieces = cx.voicedesk().pieces(&text);
        if pieces.is_empty() {
            self.say("There is nothing to say in this text (only addresses or marks).", false);
            return;
        }
        let (synth, sink) = self.plan(cx, &voice);
        self.reader.set_speed(self.speed);
        self.reader.start(pieces, 0, false, &vd::voice_key(&voice, self.speed), synth, sink);
        cx.record("voice.read", &format!("{} characters with {}", text.chars().count(), voice.id));
        self.job = Some(Job { text, base });
        self.focus = Focus::None;
        self.status = None;
        self.dirty = true;
    }

    pub fn pause_toggle(&mut self) {
        match self.reader.status() {
            Status::Playing | Status::Buffering => self.reader.pause(),
            Status::Paused => self.reader.resume(),
            _ => {}
        }
        self.dirty = true;
    }

    pub fn stop_reading(&mut self) {
        self.reader.stop();
        self.job = None;
        self.text.anchor = None;
        self.dirty = true;
    }

    /// The voice or the speed changed while reading: carry on from this sentence with the new one.
    pub fn restart_here(&mut self, cx: &dyn VoicePort) {
        let (Some(job), true) = (self.job.as_ref(), self.reader.is_active()) else { return };
        let at = self.reader.snapshot().chunk;
        let was_paused = self.reader.status() == Status::Paused;
        let (text, base) = (job.text.clone(), job.base);
        let Some(voice) = self.voice().cloned() else { return };
        let pieces = cx.voicedesk().pieces(&text);
        let (synth, sink) = self.plan(cx, &voice);
        self.reader.set_speed(self.speed);
        self.reader.start(pieces, at, false, &vd::voice_key(&voice, self.speed), synth, sink);
        if was_paused {
            self.reader.pause();
        }
        self.job = Some(Job { text, base });
    }

    pub fn set_speed(&mut self, cx: &dyn VoicePort, v: f32) {
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

    pub fn pick_voice(&mut self, cx: &dyn VoicePort, label: &str) {
        if let Some(i) = self.voices.iter().position(|v| v.label == label) {
            self.voice_i = i;
            self.restart_here(cx);
            self.dirty = true;
        }
    }

    // ------------------------------------------------------------------------------------------------ dictating

    pub fn start_record(&mut self, cx: &dyn VoicePort) {
        self.start_record_for(cx, false);
    }

    /// Start recording; `say`: it is one spoken instruction for the Assistant pane, not dictation.
    pub fn start_record_for(&mut self, cx: &dyn VoicePort, say: bool) {
        if self.cap.is_some() || self.transcribing {
            return;
        }
        if self.rec.cap.is_some() {
            self.say("The recorder is using the microphone. Stop it first.", false);
            return;
        }
        if !self.allowed(cx, "record") {
            return;
        }
        self.look(cx);
        if self.mics.is_empty() {
            self.say(cx.no_mic_text(&self.product, vd::t2_mic_hidden(cx.proc_root().as_path())), false);
            return;
        }
        let Some(engine) = cx.voicedesk().stt_engine() else {
            self.say("No speech-to-text is set up. Install Whisper with Settings -> Models (Lemonade) or set one in Settings -> Voice.", false);
            return;
        };
        if let Some((dest, text)) = cx.voicedesk().consent_text(&engine, true) {
            self.modal = Some(Modal::Consent { text, dest, then: if say { After::RecordSay } else { After::Record } });
            return;
        }
        if !say && cx.voice_settings().fix_wording && !self.fix_declined {
            if let dictfix::Target::NeedsConsent(_, server, text) = cx.dict_target() {
                let address = server.split_once("://").map_or(server.clone(), |(_, r)| r.to_string());
                let place = iyakki_speechtime::locality(&server);
                self.modal = Some(Modal::Consent { text, dest: Dest { name: "the assistant".into(), address, server, place }, then: After::RecordFix });
                return;
            }
        }
        self.record_now(cx, say);
    }

    pub fn record_now(&mut self, cx: &dyn VoicePort, say: bool) {
        self.stop_reading(); // do not record the voice that is reading
        // automatic is the first microphone found (never the playback-only `default` device)
        let mic = match self.mic_i.checked_sub(1) {
            Some(i) => self.mics.get(i),
            None => iyakki_voice_model::voicedesk::pick_mic(&self.mics, ""),
        };
        let cmd = cx.record_cmd_for(&cx.voice_settings().record_cmd, mic);
        match self.rig.recorders.start(&cmd, MAX_RECORD_SECS) {
            Ok(c) => {
                self.cap = Some(c);
                self.assist.say_mode = say;
                self.pause = { let v = cx.voice_settings(); iyakki_voice_model::voicedesk::PauseDetector::from_auto(v.auto_stop, v.pause_ms) };
                self.ended_on_pause = false;
                self.level = 0.0;
                self.peak = 0.0;
                self.status = None;
                cx.record("voice.record", "");
                self.focus = Focus::None;
            }
            Err(e) => self.say(format!("The microphone could not be opened: {e}"), false),
        }
        self.dirty = true;
    }

    /// Stop recording and turn what was said into text, or throw it away.
    pub fn stop_record(&mut self, cx: &dyn VoicePort, keep: bool) {
        let Some(cap) = self.cap.take() else { return };
        if !keep {
            drop(cap);
            self.assist.say_mode = false;
            self.say("Recording cancelled. Nothing was kept.", true);
            return;
        }
        let Some(engine) = cx.voicedesk().stt_engine() else {
            self.say("No speech-to-text is set up.", false);
            return;
        };
        let desk = cx.voicedesk();
        let (tx, rx) = channel();
        // the worker only turns the sound into text; Correct my wording runs after the raw words are in the box (voice_fix.rs)
        std::thread::spawn(move || {
            let r = cap.stop().and_then(|pcm| desk.transcribe(&engine, &pcm));
            let _ = tx.send(r);
        });
        self.stt_rx = Some(rx);
        self.transcribing = true;
        self.dirty = true;
    }

    pub fn finish_transcript(&mut self, cx: &dyn VoicePort, r: SttDone) {
        if self.assist.say_mode {
            self.finish_instruction(r);
            return;
        }
        self.transcribing = false;
        self.stt_rx = None;
        match r {
            Ok(raw) => {
                self.empty_streak = 0;
                let fix = cx.voice_settings().fix_wording && !self.fix_declined;
                self.add_chunk(cx, raw, fix);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub fn copy_transcript(&mut self, clear: bool) {
        let t = self.transcript.selected_text().filter(|_| !clear).unwrap_or_else(|| self.transcript.text.clone());
        if t.trim().is_empty() {
            self.say("There is no transcript to copy yet.", false);
            return;
        }
        iyakki_uikit::ui::clipboard::copy(&t);
        if clear {
            self.transcript.take();
            self.fixes_clear();
            self.assist_reset();
            self.say(format!("Copied {} characters and cleared the box.", t.chars().count()), true);
        } else {
            self.say(format!("Copied {} characters to the clipboard.", t.chars().count()), true);
        }
    }

    pub fn read_back(&mut self) {
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

    pub fn on_tick(&mut self, cx: &dyn VoicePort) {
        self.ticks += 1;
        // the wake listener gives the microphone up while this tile records (F-VOICE-60)
        (self.tile_rec)(self.cap.is_some() || self.rec.cap.is_some());
        self.mine_tick(cx);
        if self.ticks - self.looked >= 60 && self.cap.is_none() && self.rec.cap.is_none() && !self.reader.is_active() && self.modal.is_none() {
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
            let (alive, full, secs, level, ms) = {
                let c = self.cap.as_mut().expect("recording");
                (c.is_alive(), c.is_full(), c.elapsed().as_secs(), c.level(), c.elapsed().as_millis() as u64)
            };
            self.level = level.max(self.level * 0.85);
            self.peak = level.max(self.peak * 0.97);
            if alive && self.pause.as_mut().is_some_and(|p| p.tick(level, ms)) {
                // the person stopped talking: end it and start turning it into text, as if they had pressed stop
                self.ended_on_pause = true;
                self.stop_record(cx, true);
            } else if !alive {
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
                let dictating = !self.assist.say_mode;
                let keep = cx.voice_settings().keep_listening && self.area == Area::Dictate;
                let on_pause = std::mem::take(&mut self.ended_on_pause);
                // noise that ended a recording and gave no words: with Keep listening, listen again without a word (a red line each time
                // is noise too), and stop with one plain line after EMPTY_LIMIT in a row
                let empty = dictating && keep && on_pause && r.as_ref().err().is_some_and(|e| vd::is_no_words(e));
                if empty {
                    self.transcribing = false;
                    self.stt_rx = None;
                    self.empty_streak += 1;
                    if self.empty_streak >= EMPTY_LIMIT {
                        self.empty_streak = 0;
                        self.say(NOISE_STOP, false);
                    } else {
                        self.start_record(cx);
                    }
                } else {
                    let ok = r.is_ok() && dictating;
                    if dictating && !(keep && on_pause) {
                        self.empty_streak = 0;
                    }
                    self.finish_transcript(cx, r);
                    // Keep listening: the dictation ended on a pause and its raw text is in the box, so listen again at once, without
                    // waiting for the correction (never after a failure or the person's Stop, so a broken microphone cannot loop)
                    if ok && on_pause && keep {
                        self.start_record(cx);
                    }
                }
            }
        }
        self.fix_tick(cx);
        self.assist_tick();
        self.rec_tick(cx);
    }

    pub fn animating(&self) -> bool {
        self.cap.is_some() || self.transcribing || self.pending_fixes() > 0 || self.assist.working || self.reader.is_active() || self.level > 0.01 || self.rec_animating() || self.mine_active()
    }

    // ------------------------------------------------------------------------------------------------ files

    pub fn load_file(&mut self, cx: &dyn VoicePort) {
        let input = self.path.value.trim().to_string();
        self.focus = Focus::Text;
        if input.is_empty() {
            self.say("Type the path of a text file.", false);
            return;
        }
        let roots = cx.file_roots();
        let r = roots.resolve(&roots.home, &input).and_then(|real| {
            cx.check_read(&real)?;
            let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
            if meta.len() > 2_000_000 {
                return Err("That file is too large to read aloud (over 2 MB).".into());
            }
            let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
            let t = String::from_utf8(bytes).map_err(|_| "That file is not text.".to_string())?;
            cx.record("voice.load", &real.to_string_lossy());
            Ok(t)
        });
        match r {
            Ok(t) => {
                self.text.set(&t);
                self.path.set("");
                let n = self.text.len();
                match self.text.cut {
                    Some(_) => self.say(format!("Loaded the first {} characters; the rest was left out.", iyakki_uikit::multiline::thousands(n)), false),
                    None => self.say(format!("Loaded {} characters.", iyakki_uikit::multiline::thousands(n)), true),
                }
            }
            Err(e) => self.say(e, false),
        }
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    pub fn click(&mut self, n: usize, cx: &dyn VoicePort) {
        match n {
            T_TAB_READ => self.switch(Area::Read, cx),
            T_TAB_DICT => self.switch(Area::Dictate, cx),
            T_TAB_REC => self.switch(Area::Record, cx),
            T_TAB_MINE => self.switch(Area::Mine, cx),
            70..=82 => self.mine_click(n, cx),
            T_PLAY => self.play(cx, false),
            T_PAUSE => self.pause_toggle(),
            T_STOP => self.stop_reading(),
            T_SEL => self.play(cx, true),
            T_PASTE => {
                if let Some(t) = iyakki_uikit::ui::clipboard::latest() {
                    self.paste(&t);
                } else {
                    self.say("The clipboard has no text.", false);
                }
            }
            T_LOAD => {
                self.focus = Focus::Path;
            }
            T_BROWSE => {
                self.focus = Focus::Path;
                let start = self.path.value.clone();
                self.path_pick.open(iyakki_uikit::filepicker::PathKind::File, &["txt", "md", "text"], "Choose a text file", &start);
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
                self.modal = Some(Modal::VoicePick(iyakki_uikit::widgets::Picker::new("Reading voice", items)));
            }
            T_SPEED_DN => self.set_speed(cx, self.speed - SPEED_STEP),
            T_SPEED_UP => self.set_speed(cx, self.speed + SPEED_STEP),
            T_SETUP => self.request = Some(("quickinstall".into(), "kokoro".into())),
            T_SOUND => self.request = Some(("settings".into(), "tab:sound".into())),
            T_SETTINGS_VOICE => self.request = Some(("settings".into(), "voice".into())),
            T_REC => self.start_record(cx),
            T_REC_STOP => {
                self.ended_on_pause = false; // the person's Stop ends it for good
                self.stop_record(cx, true)
            }
            T_KEEP => {
                let on = !cx.voice_settings().keep_listening;
                match cx.update_voice(&mut |v| v.keep_listening = on) {
                    Ok(()) => {
                        cx.record("settings.voice.keep_listening", if on { "on" } else { "off" });
                        self.say(if on { "Keep listening is on: after each pause the text is written and it listens again until you press Stop." } else { "Keep listening is off: each dictation ends on its pause." }, true);
                    }
                    Err(e) => self.say(e.to_string(), false),
                }
                self.dirty = true;
            }
            T_FIX => {
                let on = !cx.voice_settings().fix_wording;
                match cx.dict_set_enabled(on) {
                    Ok(()) => {
                        cx.record(dictfix::AUDIT, if on { "on" } else { "off" });
                        self.fix_declined = false;
                        let msg = match (on, cx.dict_target()) {
                            (false, _) => "Correct my wording is off: the transcript is exactly what was heard.".to_string(),
                            (true, dictfix::Target::NoModel) => "Correct my wording is on, but no chat model is set up, so it does nothing yet. Pick one in Settings -> Models.".to_string(),
                            (true, _) => "Correct my wording is on: each part is tidied by the assistant before it lands in the box.".to_string(),
                        };
                        self.say(msg, true);
                    }
                    Err(e) => self.say(e, false),
                }
                self.dirty = true;
            }
            T_FIX_ORIG => self.toggle_original(),
            T_COPY => self.copy_transcript(false),
            T_COPY_CLEAR => self.copy_transcript(true),
            T_READ_BACK => self.read_back(),
            n if self.assist_click(n, cx) => {}
            T_T_CLEAR => {
                self.transcript.take();
                self.fixes_clear();
                self.assist_reset();
                self.focus = Focus::Transcript;
            }
            T_MIC => {
                self.look(cx);
                let mut items = vec!["Automatic".to_string()];
                items.extend(self.mics.iter().map(|m| m.label.clone()));
                self.modal = Some(Modal::MicPick(iyakki_uikit::widgets::Picker::new("Microphone", items)));
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub fn switch(&mut self, a: Area, cx: &dyn VoicePort) {
        if self.area != a {
            if self.area == Area::Record {
                self.rec_leave(cx);
            }
            if a == Area::Record {
                self.rec_refresh(cx);
            }
            self.area = a;
            self.status = None;
            self.focus = if a == Area::Read { Focus::Text } else { Focus::None };
            self.dirty = true;
        }
    }

    pub fn paste(&mut self, t: &str) {
        match self.focus {
            Focus::Path => {
                self.path.paste(t);
            }
            Focus::Transcript => {
                self.transcript.insert(t);
            }
            Focus::Assist => {
                self.assist.input.paste(t);
            }
            Focus::Text | Focus::None if self.area == Area::Read => {
                self.stop_reading();
                self.text.insert(t);
                self.focus = Focus::Text;
                let n = self.text.len();
                if self.text.cut.is_none() {
                    self.say(format!("Pasted. {} characters ready to read.", iyakki_uikit::multiline::thousands(n)), true);
                }
            }
            _ => {
                self.transcript.insert(t);
                self.focus = Focus::Transcript;
            }
        }
        self.dirty = true;
    }

    pub fn consent_yes(&mut self, cx: &dyn VoicePort) {
        let Some(Modal::Consent { dest, then, .. }) = self.modal.take() else { return };
        cx.record("voice.remote_ok", &dest.server);
        if let Err(e) = cx.allow_speech_server(&dest.server) {
            self.say(e, false);
            return;
        }
        match then {
            After::Play { selection } => self.play(cx, selection),
            After::Record => self.start_record(cx),
            After::RecordFix => self.start_record(cx),
            After::RecordSay => self.start_record_for(cx, true),
            After::Revise => self.assist_resume(cx),
        }
    }

    pub fn modal_event(&mut self, ev: &Event, cx: &dyn VoicePort) {
        match self.modal.take() {
            Some(Modal::Consent { text, dest, then }) => {
                let yes = matches!(ev, Event::Key(Key::Char('y' | 'Y')) | Event::Tab(T_YES));
                let no = matches!(ev, Event::Key(Key::Esc | Key::Char('n' | 'N')) | Event::Tab(T_NO));
                let fix = matches!(then, After::RecordFix);
                let revise = matches!(then, After::Revise);
                self.modal = Some(Modal::Consent { text, dest, then });
                if yes {
                    self.consent_yes(cx);
                } else if no && fix {
                    // no to the assistant's machine: dictate as before, with your words as heard
                    self.modal = None;
                    self.fix_declined = true;
                    self.start_record(cx);
                    self.say("Nothing was sent to the assistant. Your words stay as heard; switch Correct my wording off and on to be asked again.", true);
                } else if no && revise {
                    self.modal = None;
                    self.assist_declined();
                } else if no {
                    self.modal = None;
                    self.say("Nothing was sent. Pick a voice on this computer, or say yes to use that one.", true);
                }
            }
            Some(Modal::VoicePick(mut p)) => match p.handle_event(ev) {
                iyakki_uikit::widgets::PickEvent::Chosen(label) => self.pick_voice(cx, &label),
                iyakki_uikit::widgets::PickEvent::Cancel => {}
                iyakki_uikit::widgets::PickEvent::None => self.modal = Some(Modal::VoicePick(p)),
            },
            Some(Modal::MicPick(mut p)) => match p.handle_event(ev) {
                iyakki_uikit::widgets::PickEvent::Chosen(label) => {
                    self.mic_i = if label == "Automatic" { 0 } else { self.mics.iter().position(|m| m.label == label).map_or(0, |i| i + 1) };
                    let id = self.mic_i.checked_sub(1).and_then(|i| self.mics.get(i)).map(|m| m.id.clone()).unwrap_or_default();
                    if let Err(e) = cx.set_voice_mic(&id) {
                        self.say(format!("The microphone choice could not be saved: {e}"), false);
                    }
                }
                iyakki_uikit::widgets::PickEvent::Cancel => {}
                iyakki_uikit::widgets::PickEvent::None => self.modal = Some(Modal::MicPick(p)),
            },
            Some(Modal::LangPick(mut p)) => match p.handle_event(ev) {
                iyakki_uikit::widgets::PickEvent::Chosen(label) => {
                    if let Some(i) = self.assist.langs.iter().position(|l| *l == label) {
                        self.assist.lang_i = i;
                        self.assist_run(cx, iyakki_voice_model::revise::Kind::Translate(label));
                    }
                }
                iyakki_uikit::widgets::PickEvent::Cancel => {}
                iyakki_uikit::widgets::PickEvent::None => self.modal = Some(Modal::LangPick(p)),
            },
            None => {}
        }
        self.dirty = true;
    }

    pub fn boxed(&mut self) -> Option<&mut MultiLine> {
        match self.focus {
            Focus::Text => Some(&mut self.text),
            Focus::Transcript => Some(&mut self.transcript),
            _ => None,
        }
    }

    pub fn key(&mut self, k: &Key, cx: &dyn VoicePort) -> Flow {
        // keys that work everywhere
        match k {
            Key::F(2) => {
                self.switch(Area::Read, cx);
                return Flow::Stay;
            }
            Key::F(3) => {
                self.switch(Area::Dictate, cx);
                return Flow::Stay;
            }
            Key::F(4) => {
                self.switch(Area::Record, cx);
                return Flow::Stay;
            }
            Key::F(5) => {
                self.switch(Area::Mine, cx);
                return Flow::Stay;
            }
            Key::Ctrl('\n' | '\r') | Key::Ctrl('p' | 'P') => {
                match (self.area, self.cap.is_some()) {
                    (Area::Dictate, true) => self.stop_record(cx, true),
                    (Area::Dictate, false) => self.start_record(cx),
                    (Area::Record | Area::Mine, _) => {}
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
        if self.focus == Focus::Assist {
            self.assist_key(k, cx);
            self.dirty = true;
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
                        iyakki_uikit::ui::clipboard::copy(&t);
                    }
                    return Flow::Stay;
                }
                Key::Ctrl('x' | 'X') => {
                    if let Some(t) = b.selected_text() {
                        iyakki_uikit::ui::clipboard::copy(&t);
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
            (Key::Char('r'), _) => self.switch(Area::Read, cx),
            (Key::Char('d'), _) => self.switch(Area::Dictate, cx),
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
            (Key::Char('m'), Area::Dictate | Area::Record) => self.click(T_MIC, cx),
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

impl VoiceApp {
    pub fn title(&self) -> &'static str {
        "Voice"
    }

    pub fn icon(&self) -> Icon {
        Icon::Speaker
    }

    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    pub fn hint(&self) -> String {
        match self.area {
            Area::Read => "F2 Read aloud · F3 Dictate · F4 Record · Ctrl+Enter play or pause · Ctrl+O open a file · Esc stop or back".into(),
            Area::Dictate => "F2 Read aloud · F3 Dictate · F4 Record · Ctrl+Enter record or stop · Esc cancel the recording or go back".into(),
            Area::Mine => "F2 Read aloud · F3 Dictate · F4 Record · F5 Assistant · Space talk · S or Esc stop · M mute · Y/N answer a card".into(),
            Area::Record => "F2 Read aloud · F3 Dictate · F4 Record · Space record or stop · Up/Down choose · Enter play · n rename · Delete delete · Esc stop or back".into(),
        }
    }

    pub fn help(&self) -> Vec<String> {
        [
            "F2 / F3: Read aloud / Dictate (or r / d when no box has the keyboard)",
            "Read aloud: paste with Ctrl+V or type; Ctrl+Enter or Space plays and pauses; Esc stops; Ctrl+O loads a text file",
            "Select some text and press Read selection to read only that; click the progress bar to jump",
            "v choose the voice · + and - change the speed (when the voice has one)",
            "Dictate: Ctrl+Enter or Enter starts and stops recording (never without your click); Esc cancels and keeps nothing",
            "Dictate, Assistant pane: press a button (Shorter, Formal, Translate...) or type an instruction and press Enter; it shows a proposal and changes nothing until Apply; Undo or Ctrl+Z in the pane steps back; select part of the transcript to change only that part",
            "Dictate: c copy the transcript · x copy and clear · b read it back · m choose the microphone",
            "F4: Record. Space or Ctrl+Enter starts and stops (never without your press); the recording is saved as a WAV in your Recordings folder",
            "F5: Assistant (My IYAKKI, push to talk): Space or Enter talks and ends your request, S or Esc stops, M mutes, Y or N answers an approval card or a did-you-say question; risky tools can only be approved by a tap or key, never by voice",
            "Record: Up/Down choose a recording · Enter or p plays and stops it · n renames (Enter saves) · Delete or x deletes after you say y · m microphone · Esc stops",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    pub fn wants_redraw(&mut self) -> bool {
        if self.animating() {
            self.ticks % 3 == 0
        } else {
            std::mem::take(&mut self.dirty)
        }
    }

    pub fn handle_with(&mut self, ev: &Event, cx: &dyn VoicePort) -> Flow {
        if matches!(ev, Event::Tick) {
            self.on_tick(cx);
            return Flow::Stay;
        }
        // the file picker, when Browse opened it, takes every event until it answers
        match self.path_pick.event(ev) {
            iyakki_uikit::pathpick::Picked::NotOpen => {}
            iyakki_uikit::pathpick::Picked::Taken => return Flow::Stay,
            iyakki_uikit::pathpick::Picked::Chosen(p) => {
                self.path.set(&p.display().to_string());
                self.focus = Focus::Path;
                return Flow::Stay;
            }
        }
        if self.modal.is_some() {
            self.modal_event(ev, cx);
            return Flow::Stay;
        }
        if self.area == Area::Record {
            if let Some(f) = self.rec_handle(ev, cx) {
                return f;
            }
        }
        if self.area == Area::Mine {
            if let Event::Key(k) = ev {
                if let Some(f) = self.mine_key(k, cx) {
                    return f;
                }
            }
        }
        match ev {
            Event::Key(k) => return self.key(k, cx),
            Event::Tab(n) => {
                if matches!(*n, T_PASTE | T_LOAD | T_CLEAR | T_BROWSE) || *n == T_T_CLEAR {
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
            Event::FocusAt(assist::ASSIST_FIELD, x) => {
                self.focus = Focus::Assist;
                self.assist.input.click(*x);
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

    pub fn render_with(&mut self, g: &mut iyakki_uikit::gfx::Gfx, area: iyakki_uikit::ui::R, s: f32, cx: &dyn VoicePort, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}

#[path = "voice_assist.rs"]
pub mod assist;

#[path = "voice_fix.rs"]
pub mod fix;

#[cfg(test)]
mod fake_tests;
