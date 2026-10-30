//! VOICE, RECORD: a voice recorder. Record / Stop saves a 16-bit WAV in the person's own `Recordings` folder; the list below
//! plays, renames and deletes (delete asks first). The microphone is the one dictation uses (`Rig::recorders`, one microphone
//! choice for both) and the speaker is the shared audio entry point; nothing is recorded before the person presses Record, and a
//! recording and a dictation never run together (the T2 microphone and speaker are single-use devices).

use crate::safeclamp::SafeClamp;
use std::path::PathBuf;

use iyakki_agent::recordings::{self as recs, Entry, ServicesSink};
use iyakki_agent::story::{Playing, Sink};
use iyakki_agent::voicedesk::{self as vd, Capture, MAX_RECORD_SECS};

use super::voice::{Area, VoiceApp};
use super::{AppCx, Flow};
use crate::ui::{Event, Key, TextField};

pub(super) const T_TAB_REC: usize = 3;
pub(super) const T_RR_REC: usize = 50;
pub(super) const T_RR_STOP: usize = 51;
pub(super) const T_RR_PLAY: usize = 52;
pub(super) const T_RR_PSTOP: usize = 53;
pub(super) const T_RR_RENAME: usize = 54;
pub(super) const T_RR_DELETE: usize = 55;
pub(super) const T_RR_YES: usize = 56;
pub(super) const T_RR_NO: usize = 57;
pub(super) const T_RR_SAVE_NAME: usize = 58;
pub(super) const T_RR_CANCEL_NAME: usize = 59;
/// Row `i` of the list is `T_RR_ROW + i`.
pub(super) const T_RR_ROW: usize = 1_000;
pub(super) const NAME_FIELD: usize = 62_006;

/// What the bottom of the page is asking.
pub(super) enum Ask {
    Nothing,
    /// The new name for this recording is being typed.
    Rename(String),
    /// Delete this recording? Waiting for yes or no.
    Delete(String),
}

/// The state of Record mode.
pub(super) struct Rec {
    pub cap: Option<Box<dyn Capture>>,
    pub level: f32,
    pub peak: f32,
    pub list: Vec<Entry>,
    pub sel: usize,
    /// First row shown.
    pub top: usize,
    /// The recording that is playing and its sound.
    pub playing: Option<(String, Box<dyn Playing>)>,
    pub ask: Ask,
    pub name: TextField,
    /// Where recordings go (None: the person's `Recordings` folder).
    pub dir: Option<PathBuf>,
    pub listed: bool,
}

impl Rec {
    pub fn new() -> Rec {
        let mut name = TextField::new("name of the recording", false);
        name.max = recs::MAX_NAME;
        Rec { cap: None, level: 0.0, peak: 0.0, list: Vec::new(), sel: 0, top: 0, playing: None, ask: Ask::Nothing, name, dir: None, listed: false }
    }
}

impl Drop for Rec {
    fn drop(&mut self) {
        if let Some((_, p)) = self.playing.as_mut() {
            p.stop();
        }
    }
}

impl VoiceApp {
    /// Keep recordings in `dir` instead of the person's `Recordings` folder (tests).
    pub fn with_recordings_dir(mut self, dir: PathBuf) -> VoiceApp {
        self.rec.dir = Some(dir);
        self
    }

    pub fn is_recording_to_file(&self) -> bool {
        self.rec.cap.is_some()
    }

    pub fn recordings(&self) -> &[Entry] {
        &self.rec.list
    }

    pub fn is_playing_recording(&self) -> bool {
        self.rec.playing.is_some()
    }

    pub(super) fn rec_dir(&self, cx: &AppCx) -> PathBuf {
        self.rec.dir.clone().unwrap_or_else(|| recs::folder(&iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir())))
    }

    pub(super) fn rec_refresh(&mut self, cx: &AppCx) {
        let keep = self.rec.list.get(self.rec.sel).map(|e| e.name.clone());
        self.rec.list = recs::list(&self.rec_dir(cx));
        self.rec.sel = keep.and_then(|n| self.rec.list.iter().position(|e| e.name == n)).unwrap_or(0).min(self.rec.list.len().saturating_sub(1));
        self.rec.listed = true;
    }

    fn selected(&self) -> Option<&Entry> {
        self.rec.list.get(self.rec.sel)
    }

    // ------------------------------------------------------------------------------------------------ recording

    pub(super) fn rec_start(&mut self, cx: &AppCx) {
        if self.rec.cap.is_some() {
            return;
        }
        if self.cap.is_some() || self.transcribing {
            self.say("Dictation is using the microphone. Stop it first.", false);
            return;
        }
        if !self.allowed(cx, "record") {
            return;
        }
        self.look(cx);
        if self.mics.is_empty() {
            self.say(vd::no_mic_text_t2(&self.product, vd::t2_mic_hidden(&iyakki_agent::voicedesk::proc_root())), false);
            return;
        }
        let dir = self.rec_dir(cx);
        if !cx.session.policy.allows_write(&cx.session.user, &dir.display().to_string()) {
            self.say(format!("Your account may not save in {}.", dir.display()), false);
            return;
        }
        self.stop_reading(); // do not record the voice that is reading
        self.rec_stop_playing();
        self.rec.ask = Ask::Nothing;
        let mic = match self.mic_i.checked_sub(1) {
            Some(i) => self.mics.get(i),
            None => vd::pick_mic(&self.mics, ""),
        };
        let cmd = vd::record_cmd_for(&cx.services.voice_settings().record_cmd, mic);
        match self.rig.recorders.start(&cmd, MAX_RECORD_SECS) {
            Ok(c) => {
                self.rec.cap = Some(c);
                self.rec.level = 0.0;
                self.rec.peak = 0.0;
                self.status = None;
                cx.audit.record(&cx.session.user, "voice.recorder.start", "");
            }
            Err(e) => self.say(format!("The microphone could not be opened: {e}"), false),
        }
        self.dirty = true;
    }

    /// Stop recording and keep what was said as a WAV.
    pub(super) fn rec_stop(&mut self, cx: &AppCx) {
        let Some(cap) = self.rec.cap.take() else { return };
        self.dirty = true;
        let pcm = match cap.stop() {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => return self.say("Nothing was recorded.", false),
            Err(e) => return self.say(e, false),
        };
        let dir = self.rec_dir(cx);
        let made = std::fs::create_dir_all(&dir);
        crate::camera::own::give_saved(&dir);
        let stem = format!("recording-{}", crate::camera::stamp());
        match made.map_err(|e| e.to_string()).and_then(|_| recs::save(&dir, &stem, &pcm, iyakki_agent::voice::SAMPLE_RATE)) {
            Ok(e) => {
                crate::camera::own::give_saved(&dir.join(e.file()));
                cx.audit.record(&cx.session.user, "voice.recorder.save", &e.name);
                let msg = format!("Saved {} ({}).", e.name, clock_of(e.secs));
                self.rec_refresh(cx);
                self.rec.sel = self.rec.list.iter().position(|x| x.name == e.name).unwrap_or(0);
                self.say(msg, true);
            }
            Err(e) => self.say(vd::reword_storage_error(&e), false),
        }
    }

    // ------------------------------------------------------------------------------------------------ playing

    pub(super) fn rec_stop_playing(&mut self) {
        if let Some((_, mut p)) = self.rec.playing.take() {
            p.stop();
        }
        self.dirty = true;
    }

    fn rec_play_toggle(&mut self, cx: &AppCx) {
        if self.rec.playing.is_some() {
            return self.rec_stop_playing();
        }
        let Some(name) = self.selected().map(|e| e.name.clone()) else {
            return self.say("There is no recording to play yet.", false);
        };
        if self.rec.cap.is_some() {
            return self.say("Stop recording first.", false);
        }
        if let Err(why) = (self.rig.output)() {
            log::info!("voice: no sound output: {why}");
            return self.say("No sound output: choose where sound plays in Settings -> Sound.", false);
        }
        let wav = match recs::read(&self.rec_dir(cx), &name) {
            Ok(w) => w,
            Err(e) => return self.say(e, false),
        };
        self.stop_reading();
        let sink: std::sync::Arc<dyn Sink> = self.rig.sink.clone().unwrap_or_else(|| std::sync::Arc::new(ServicesSink(cx.services.clone())));
        match sink.play(&wav) {
            Ok(p) => {
                self.rec.playing = Some((name.clone(), p));
                self.say(format!("Playing {name}."), true);
            }
            Err(e) => self.say(format!("It could not be played: {e}"), false),
        }
    }

    // ------------------------------------------------------------------------------------------------ rename and delete

    fn rec_ask_rename(&mut self) {
        let Some(name) = self.selected().map(|e| e.name.clone()) else { return };
        self.rec.name.set(&name);
        self.rec.ask = Ask::Rename(name);
        self.status = None;
    }

    fn rec_apply_rename(&mut self, cx: &AppCx) {
        let Ask::Rename(old) = std::mem::replace(&mut self.rec.ask, Ask::Nothing) else { return };
        let typed = self.rec.name.value.clone();
        match recs::rename(&self.rec_dir(cx), &old, &typed) {
            Ok(new) => {
                if let Some((n, _)) = self.rec.playing.as_mut() {
                    if *n == old {
                        *n = new.clone();
                    }
                }
                cx.audit.record(&cx.session.user, "voice.recorder.rename", &new);
                self.rec_refresh(cx);
                self.rec.sel = self.rec.list.iter().position(|e| e.name == new).unwrap_or(0);
                self.say(format!("Renamed to {new}."), true);
            }
            Err(e) => {
                // keep the box open so the name can be fixed
                self.rec.ask = Ask::Rename(old);
                self.say(e, false);
            }
        }
    }

    fn rec_ask_delete(&mut self) {
        if let Some(name) = self.selected().map(|e| e.name.clone()) {
            self.rec.ask = Ask::Delete(name);
            self.status = None;
        }
    }

    fn rec_apply_delete(&mut self, cx: &AppCx) {
        let Ask::Delete(name) = std::mem::replace(&mut self.rec.ask, Ask::Nothing) else { return };
        if self.rec.playing.as_ref().is_some_and(|(n, _)| *n == name) {
            self.rec_stop_playing();
        }
        match recs::delete(&self.rec_dir(cx), &name) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "voice.recorder.delete", &name);
                self.rec_refresh(cx);
                self.say(format!("Deleted {name}."), true);
            }
            Err(e) => {
                self.rec_refresh(cx);
                self.say(e, false);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ the heartbeat

    pub(super) fn rec_tick(&mut self, cx: &AppCx) {
        if self.area == Area::Record && !self.rec.listed {
            self.rec_refresh(cx);
        }
        if self.rec.cap.is_some() {
            let (alive, full, secs, level) = {
                let c = self.rec.cap.as_mut().expect("recording");
                (c.is_alive(), c.is_full(), c.elapsed().as_secs(), c.level())
            };
            self.rec.level = level.max(self.rec.level * 0.85);
            self.rec.peak = level.max(self.rec.peak * 0.97);
            if !alive {
                self.rec_stop(cx);
            } else if full || secs >= MAX_RECORD_SECS as u64 {
                self.rec_stop(cx);
                self.say("Reached the five minute limit; the recording was saved.", true);
            }
        } else {
            self.rec.level *= 0.8;
        }
        let mut ended = None;
        if let Some((name, p)) = self.rec.playing.as_mut() {
            if !p.is_playing() {
                ended = Some((name.clone(), p.error()));
            }
        }
        if let Some((name, err)) = ended {
            self.rec.playing = None;
            match err {
                Some(e) => self.say(e, false),
                None => self.say(format!("Finished playing {name}."), true),
            }
        }
    }

    pub(super) fn rec_animating(&self) -> bool {
        self.rec.cap.is_some() || self.rec.playing.is_some() || self.rec.level > 0.01
    }

    /// Leaving Record mode: a recording in progress is saved and playing stops.
    pub(super) fn rec_leave(&mut self, cx: &AppCx) {
        if self.rec.cap.is_some() {
            self.rec_stop(cx);
        }
        self.rec_stop_playing();
        self.rec.ask = Ask::Nothing;
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    fn rec_select(&mut self, i: usize) {
        if i < self.rec.list.len() {
            self.rec.sel = i;
            self.rec.ask = Ask::Nothing;
        }
    }

    fn rec_click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_RR_REC => self.rec_start(cx),
            T_RR_STOP => self.rec_stop(cx),
            T_RR_PLAY | T_RR_PSTOP => self.rec_play_toggle(cx),
            T_RR_RENAME => self.rec_ask_rename(),
            T_RR_DELETE => self.rec_ask_delete(),
            T_RR_YES => self.rec_apply_delete(cx),
            T_RR_NO | T_RR_CANCEL_NAME => self.rec.ask = Ask::Nothing,
            T_RR_SAVE_NAME => self.rec_apply_rename(cx),
            n if n >= T_RR_ROW => self.rec_select(n - T_RR_ROW),
            _ => {}
        }
    }

    /// Events of Record mode. None: not mine (tabs, the microphone picker, F-keys), the app handles it.
    pub(super) fn rec_handle(&mut self, ev: &Event, cx: &AppCx) -> Option<Flow> {
        let flow = match ev {
            Event::Tab(n) if *n >= T_RR_REC => {
                self.rec_click(*n, cx);
                Flow::Stay
            }
            Event::FocusAt(NAME_FIELD, x) => {
                self.rec.name.click(*x);
                Flow::Stay
            }
            Event::Paste(t) => {
                if matches!(self.rec.ask, Ask::Rename(_)) {
                    self.rec.name.paste(t);
                }
                Flow::Stay
            }
            Event::Scroll(d) => {
                let max = self.rec.list.len().saturating_sub(1) as i32;
                self.rec.top = (self.rec.top as i32 + d.signum()).clamp_safe(0, max) as usize;
                Flow::Stay
            }
            Event::Key(k) => return self.rec_key(k, cx),
            _ => return None,
        };
        self.dirty = true;
        Some(flow)
    }

    fn rec_key(&mut self, k: &Key, cx: &AppCx) -> Option<Flow> {
        if matches!(k, Key::F(_)) {
            return None;
        }
        self.dirty = true;
        // a question first
        match &self.rec.ask {
            Ask::Delete(_) => {
                match k {
                    Key::Char('y' | 'Y') => self.rec_apply_delete(cx),
                    Key::Esc | Key::Char('n' | 'N') => self.rec.ask = Ask::Nothing,
                    _ => {}
                }
                return Some(Flow::Stay);
            }
            Ask::Rename(_) => {
                match k {
                    Key::Enter => self.rec_apply_rename(cx),
                    Key::Esc => self.rec.ask = Ask::Nothing,
                    other => {
                        self.rec.name.handle(other);
                    }
                }
                return Some(Flow::Stay);
            }
            Ask::Nothing => {}
        }
        match k {
            Key::Esc => {
                if self.rec.cap.is_some() {
                    self.rec_stop(cx); // Stop keeps the recording
                } else if self.rec.playing.is_some() {
                    self.rec_stop_playing();
                } else {
                    return Some(Flow::Back);
                }
            }
            Key::Ctrl('\n' | '\r') | Key::Ctrl('p' | 'P') | Key::Char(' ') => {
                if self.rec.cap.is_some() {
                    self.rec_stop(cx)
                } else {
                    self.rec_start(cx)
                }
            }
            Key::Up => self.rec_select(self.rec.sel.saturating_sub(1)),
            Key::Down => self.rec_select(self.rec.sel + 1),
            Key::Home => self.rec_select(0),
            Key::End => self.rec_select(self.rec.list.len().saturating_sub(1)),
            Key::Enter | Key::Char('p') => self.rec_play_toggle(cx),
            Key::Char('n') => self.rec_ask_rename(),
            Key::Delete | Key::Char('x') => self.rec_ask_delete(),
            Key::Char('m') => return None,
            Key::Char('r') | Key::Char('d') => return None,
            _ => {}
        }
        Some(Flow::Stay)
    }
}

pub(super) fn clock_of(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
