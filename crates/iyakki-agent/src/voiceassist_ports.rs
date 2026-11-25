//! The real ports of the My IYAKKI state machine (`voiceassist`): the microphone through the Voice tile's recorders, speech to text
//! through the Voice settings, the speaker through the one audio entry point, the chat agent through `agent::RunHandle`, and the
//! voice thread through the chat sessions. Plus the worker that runs the machine on its own thread so the screen never waits for
//! a speech engine.
//!
//! Nothing here keeps sound: the samples go from the recorder to the speech engine and are dropped. No file is opened in this module.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent::{RunEvent, RunHandle, Verdict};
use crate::chat::Message;
use crate::services::Services;
use crate::voiceassist::*;
use crate::voicedesk::{self as vd, Capture, Recorders};
pub use iyakki_shared::voiceassist_ports::{Cmd, Live};

// ------------------------------------------------------------------------------------------------ microphone

/// The microphone: the Voice tile's recorders and record command (one microphone choice for dictation, the recorder and this).
pub struct RecorderAudio {
    pub recorders: Arc<dyn Recorders>,
    pub svc: Arc<Services>,
    cap: Option<Box<dyn Capture>>,
    /// When the capture was opened, on the echo clock (see [`echo_ms`]).
    opened_ms: u64,
}

impl RecorderAudio {
    pub fn new(recorders: Arc<dyn Recorders>, svc: Arc<Services>) -> RecorderAudio {
        RecorderAudio { recorders, svc, cap: None, opened_ms: 0 }
    }
}

/// The clock the echo reference and the captures share (milliseconds since the first use).
pub fn echo_ms() -> u64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// What the assistant played lately (its own voice), for taking its echo out of a capture (Settings -> Voice, echo cancellation).
static ECHO_REF: std::sync::OnceLock<Mutex<crate::echocancel::Reference>> = std::sync::OnceLock::new();

fn echo_ref() -> std::sync::MutexGuard<'static, crate::echocancel::Reference> {
    ECHO_REF.get_or_init(|| Mutex::new(crate::echocancel::Reference::default())).lock().unwrap_or_else(|e| e.into_inner())
}

/// How long after the capture was opened its first sample is taken to be: the recorder program needs a moment to start, and the
/// filter only looks backwards, so the reference is not allowed to be later than the sound it belongs to.
pub const CAPTURE_START_MARGIN_MS: u64 = 80;

/// The capture with the assistant's own voice taken out, when the person switched that on and something was playing during it.
pub fn clean_for(svc: &Services, pcm: Vec<i16>, opened_ms: u64) -> Vec<i16> {
    if !svc.voice_settings().echo_cancel {
        return pcm;
    }
    // the mixer is asked to keep the speaker's whole output for the next minute; the capture is cleaned with it when it holds sound for the
    // capture (it holds the assistant's voice and every other app's), else with what the assistant itself played
    crate::echocancel::want_mixed_output(echo_ms());
    let start = opened_ms + CAPTURE_START_MARGIN_MS;
    let c = crate::echocancel::clean_capture_mixed(&pcm, start, &crate::echocancel::mixed_output());
    if c.touched {
        return c.pcm;
    }
    let r = echo_ref();
    let c = crate::echocancel::clean_capture(&pcm, start, &r);
    if c.touched { c.pcm } else { pcm }
}

/// Open the microphone the way the Voice tile does (the saved microphone choice, the record command, the named-format PCM of
/// `micpcm`): the one place that does it, shared by the assistant's push to talk and the wake listener (`wakelisten`).
pub fn open_capture(recorders: &dyn Recorders, svc: &Services, max_secs: usize) -> Result<Box<dyn Capture>, String> {
    if svc.voice_settings().echo_cancel {
        // the mixer keeps the speaker's whole output from now on, so this capture can be cleaned with everything that played
        crate::echocancel::want_mixed_output(echo_ms());
    }
    let mics = recorders.mics();
    let v = svc.voice_settings();
    let mic = vd::pick_mic(&mics, &v.mic);
    if mic.is_none() && mics.is_empty() {
        return Err(vd::no_mic_text_t2(&recorders.product(), vd::t2_mic_hidden(&vd::proc_root())));
    }
    let cmd = vd::record_cmd_for(&v.record_cmd, mic);
    recorders.start(&cmd, max_secs)
}

impl AudioIn for RecorderAudio {
    fn open(&mut self) -> Result<(), String> {
        if self.cap.is_some() {
            return Ok(());
        }
        self.cap = Some(open_capture(self.recorders.as_ref(), &self.svc, (MAX_LISTEN_MS / 1000 + 10) as usize)?);
        self.opened_ms = echo_ms();
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.cap.is_some()
    }
    fn level(&mut self) -> f32 {
        self.cap.as_ref().map_or(0.0, |c| c.level())
    }
    fn alive(&mut self) -> bool {
        self.cap.as_mut().is_some_and(|c| c.is_alive())
    }
    fn take(&mut self) -> Result<Vec<i16>, String> {
        match self.cap.take() {
            Some(c) => c.stop().map(|pcm| clean_for(&self.svc, pcm, self.opened_ms)),
            None => Err("The microphone is not open.".into()),
        }
    }
    fn discard(&mut self) {
        if let Some(c) = self.cap.take() {
            let _ = c.stop();
        }
    }
}

impl Drop for RecorderAudio {
    fn drop(&mut self) {
        self.discard();
    }
}

// ------------------------------------------------------------------------------------------------ speech to text

/// Speech to text with the engine of Settings -> Voice (else a Lemonade whisper model). Sound goes to another machine only when the
/// person already agreed to that machine in the Voice tile.
pub struct ServicesStt {
    pub svc: Arc<Services>,
}

impl ServicesStt {
    /// Hear `pcm` with the backend of `reg` that `voice.stt` selects (`voicebackend::Registry::selected`): the consent check and the
    /// call sit here in one place, the same place the `voice_backends` tool reads from.
    pub fn hear_with(&self, reg: &crate::voicebackend::Registry, pcm: &[i16]) -> Result<Heard, String> {
        let backend = reg
            .selected(self.svc.voice_settings().stt.as_ref())
            .filter(|b| b.configured())
            .ok_or_else(|| "No speech-to-text is set up (Settings -> Voice, or download a Whisper model in Settings -> Models).".to_string())?;
        let on_other_machine = match backend.engine() {
            Some(e) => vd::consent_text(&self.svc, e, true).is_some(),
            None => backend.needs_consent(),
        };
        if on_other_machine {
            return Err("The speech-to-text engine is on another machine you have not agreed to send your voice to yet: open the Voice tile and say yes there first.".into());
        }
        let text = vd::transcribe_backend(backend.as_ref(), pcm)?;
        // the engines used here do not report a confidence; the machine's short-transcript rule is the safety net
        Ok(Heard { text, confidence: None })
    }
}

impl Stt for ServicesStt {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String> {
        let reg = crate::voicebackend::Registry::from_services(&self.svc);
        self.hear_with(&reg, pcm)
    }
}

// ------------------------------------------------------------------------------------------------ speaker

struct Job {
    busy: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

/// The speaker: the chosen reading voice, played through the shared audio entry point (`Services::play_wav_output`) so the
/// system volume and the output choice apply. One sentence at a time on a helper thread.
/// The sentences of the voice assistant being spoken, for the end-of-session hook `voice-assistant-speaker` (D-SESSION-END).
pub static SPEAKER_STOPS: iyakki_shared::sessionend::StopSet = iyakki_shared::sessionend::StopSet::new();

pub struct SystemSpeaker {
    svc: Arc<Services>,
    job: Option<Job>,
}

impl SystemSpeaker {
    pub fn new(svc: Arc<Services>) -> SystemSpeaker {
        SystemSpeaker { svc, job: None }
    }

    fn engine(&self) -> Option<(crate::voice::SpeechEngine, Option<String>)> {
        let v = self.svc.voice_settings();
        if let Some(e) = v.tts.clone() {
            return Some((e, None));
        }
        vd::read_voices(&self.svc).into_iter().next().map(|r| (r.engine, r.voice))
    }
}

/// The sentence-to-WAV step of a cloud session's mouth: the chosen reading voice (the same choice and consent as [`SystemSpeaker`]).
pub fn cloud_synth(svc: Arc<Services>) -> Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync> {
    Arc::new(move |text: &str| {
        use crate::story::Synth;
        let me = SystemSpeaker::new(svc.clone());
        let (engine, voice) = me.engine().ok_or_else(|| "No reading voice is set up (Settings -> Voice).".to_string())?;
        if vd::consent_text(&svc, &engine, false).is_some() {
            return Err("The reading voice is on another machine you have not agreed to send text to yet.".into());
        }
        let speed = svc.voice_settings().speed_pct as f32 / 100.0;
        vd::ChoiceSynth { svc: svc.clone(), engine, opts: crate::voice::SpeakOpts { voice, speed, lang: None } }.synth(text)
    })
}

/// The cloud session's microphone and mouth, when a session listens and the switch is on: the device's sound is what the assistant hears (the
/// listener hands it out once) and the answer goes back over the pinned link. `None` otherwise: the machine's own microphone and speaker.
pub fn cloud_ports(svc: &Arc<Services>) -> Option<(Box<dyn AudioIn>, Box<dyn Speaker>)> {
    let l = svc.cloud_voice_listener()?;
    let ear = l.take_audio()?;
    let l2 = l.clone();
    Some((Box::new(ear), Box::new(crate::cloudvoice::CloudSpeaker::new(cloud_synth(svc.clone()), Arc::new(move || l2.return_target())))))
}

/// One sentence on its helper thread: made into sound, played, and cut short by the speaker's own stop or by the end of the session
/// (`ended`, the guard of [`SPEAKER_STOPS`]). `busy` goes false when it is over, whatever way it ended.
pub(crate) fn speak_sentence(synth: &dyn crate::story::Synth, sink: &dyn crate::story::Sink, text: &str, echo: bool, busy: &AtomicBool, stop: &AtomicBool, ended: &iyakki_shared::sessionend::StopGuard) {
    let done = || busy.store(false, Ordering::SeqCst);
    let Ok(wav) = synth.synth(text) else { return done() };
    if stop.load(Ordering::SeqCst) || ended.stopped() {
        return done();
    }
    let Ok(mut playing) = sink.play(&wav) else { return done() };
    // what is being played is the reference for taking its echo out of the next capture (only when the person switched that on)
    if echo {
        if let Ok(w) = crate::voice::parse_wav(&wav) {
            let pcm: Vec<f32> = crate::voice::to_mono_16k(&w).iter().map(|s| f32::from(*s) / 32768.0).collect();
            echo_ref().add(echo_ms(), pcm);
        }
    }
    while playing.is_playing() {
        if stop.load(Ordering::SeqCst) || ended.stopped() {
            playing.stop();
            echo_ref().stop_all(echo_ms());
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    done();
}

impl Speaker for SystemSpeaker {
    fn say(&mut self, text: &str) -> Result<(), String> {
        use crate::story::{Sink, Synth};
        let (engine, voice) = self.engine().ok_or_else(|| "No reading voice is set up (Settings -> Voice).".to_string())?;
        if vd::consent_text(&self.svc, &engine, false).is_some() {
            return Err("The reading voice is on another machine you have not agreed to send text to yet: open the Voice tile and say yes there first.".into());
        }
        let v = self.svc.voice_settings();
        let speed = v.speed_pct as f32 / 100.0;
        let opts = crate::voice::SpeakOpts { voice, speed, lang: None };
        let (synth, sink): (Box<dyn Synth>, Box<dyn Sink>) = match crate::hostengines::Spec::from_setting(&engine).filter(|_| crate::hostengines::speaks_directly(&engine)) {
            Some(spec) => (Box::new(crate::hostengines::DirectSynth), Box::new(crate::hostengines::DirectSink { spec, lang: None, speed })),
            None => (Box::new(vd::ChoiceSynth { svc: self.svc.clone(), engine, opts }), Box::new(crate::recordings::ServicesSink(self.svc.clone()))),
        };
        let (busy, stop) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
        if let Some(old) = self.job.take() {
            old.stop.store(true, Ordering::SeqCst);
        }
        self.job = Some(Job { busy: busy.clone(), stop: stop.clone() });
        let text = text.to_string();
        let echo = self.svc.voice_settings().echo_cancel;
        let ended = SPEAKER_STOPS.enter(|| {});
        std::thread::spawn(move || speak_sentence(&*synth, &*sink, &text, echo, &busy, &stop, &ended));
        Ok(())
    }
    fn busy(&mut self) -> bool {
        self.job.as_ref().is_some_and(|j| j.busy.load(Ordering::SeqCst))
    }
    fn stop(&mut self) {
        if let Some(j) = &self.job {
            j.stop.store(true, Ordering::SeqCst);
            echo_ref().stop_all(echo_ms());
        }
    }
    fn level(&self) -> f32 {
        // the mixer does not report its output level yet: while the assistant speaks it is taken as a steady, fairly loud voice, so the
        // echo gate sits high until a real reference exists (plan step 5)
        if self.job.as_ref().is_some_and(|j| j.busy.load(Ordering::SeqCst)) { 0.35 } else { 0.0 }
    }
    fn louder(&mut self) {
        let _ = self.svc.master_nudge(10);
    }
    fn quieter(&mut self) {
        let _ = self.svc.master_nudge(-10);
    }
}

// ------------------------------------------------------------------------------------------------ the chat agent

/// Starts one run of the chat agent for a history (the shell builds it from the person's tools, model and limits).
pub type StartRun = Box<dyn Fn(Vec<Message>) -> RunHandle + Send>;

/// The chat agent in a voice session: the same loop as the chat, with the spoken-style addition in the system prompt. Earlier turns of
/// the session are sent again, so a follow-up has its context.
pub struct RunAgent {
    start: StartRun,
    history: Vec<Message>,
    handle: Option<RunHandle>,
}

/// The most earlier messages kept for the next turn.
const KEEP_MESSAGES: usize = 24;

impl RunAgent {
    pub fn new(base_system: &str, start: StartRun) -> RunAgent {
        RunAgent { start, history: vec![Message::system(format!("{base_system}{SESSION_PROMPT}"))], handle: None }
    }
}

impl AgentPort for RunAgent {
    fn ask(&mut self, text: &str) {
        self.history.push(Message::user(text));
        let keep = self.history.len().saturating_sub(KEEP_MESSAGES).max(1);
        self.history.drain(1..keep);
        self.handle = Some((self.start)(self.history.clone()));
    }
    fn poll(&mut self) -> Option<AgentEvent> {
        let h = self.handle.as_ref()?;
        loop {
            match h.events.try_recv() {
                Ok(RunEvent::Approval { id, name, summary, .. }) => return Some(AgentEvent::Approval { id, name, summary }),
                Ok(RunEvent::Final { text }) => {
                    self.history.push(Message::assistant(text.clone()));
                    self.handle = None;
                    return Some(AgentEvent::Final(text));
                }
                Ok(RunEvent::Failed { error }) => {
                    self.handle = None;
                    return Some(AgentEvent::Failed(error));
                }
                Ok(RunEvent::Stopped { .. }) => {
                    self.handle = None;
                    return Some(AgentEvent::Stopped);
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
    fn decide(&mut self, id: &str, yes: bool) {
        if let Some(h) = &self.handle {
            h.decide(id, if yes { Verdict::Approve } else { Verdict::Deny });
        }
    }
    fn cancel(&mut self) {
        if let Some(h) = self.handle.take() {
            h.cancel();
        }
    }
}

// ------------------------------------------------------------------------------------------------ the voice thread

/// The voice conversation as one chat session titled [`THREAD_TITLE`], so nothing is lost when the person switches to the keyboard.
/// Text only.
pub struct SessionLog {
    pub svc: Arc<Services>,
    pub user: String,
    session: Option<crate::sessions::Session>,
}

impl SessionLog {
    pub fn new(svc: Arc<Services>, user: &str) -> SessionLog {
        SessionLog { svc, user: user.to_string(), session: None }
    }
}

impl TurnLog for SessionLog {
    fn record(&mut self, who: Who, text: &str) {
        if self.session.is_none() {
            // continue the newest voice thread of the day, else start one
            let found = self.svc.sessions.list(&self.user).into_iter().find(|m| m.title == THREAD_TITLE).and_then(|m| self.svc.sessions.load(&self.user, &m.id));
            self.session = found.or_else(|| {
                let mut s = self.svc.sessions.create(&self.user).ok()?;
                s.title = THREAD_TITLE.into();
                Some(s)
            });
        }
        let Some(s) = self.session.as_mut() else { return };
        s.messages.push(match who {
            Who::Person => Message::user(crate::voiceassist::spoken_turn(text)),
            Who::Assistant => Message::assistant(text),
        });
        let _ = self.svc.sessions.save(s);
    }
}

// ------------------------------------------------------------------------------------------------ the worker



/// The machine running on its own thread. Dropping the handle stops it and closes the microphone.
pub struct Worker {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// Offer the text of finished work (see [`Cmd::Wake`]).
    pub fn wake(&self, text: &str) {
        let _ = self.tx.send(Cmd::Wake(text.to_string()));
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Run `ports` on a thread. `live` is read about four times a second; `tick_ms` is the step (50 in the app, small in tests).
pub fn spawn(ports: Ports, cfg: Config, live: Box<dyn Fn() -> Live + Send>, tick_ms: u64) -> Worker {
    let (tx, rx): (Sender<Cmd>, Receiver<Cmd>) = channel();
    let l0 = live();
    let mut a = Assistant::new(ports, Config { pause_ms: l0.pause_ms, follow_ms: l0.follow_ms, spoken_approvals: l0.spoken_approvals, ..cfg }, l0.muted);
    let status = Arc::new(Mutex::new(a.status()));
    let st = status.clone();
    let wid = crate::voiceassist::new_worker_id();
    let join = std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut last_live = 0u64;
        // finished work waits here until the assistant is free; a newer text replaces an older one (the waker's text names everything)
        let mut waiting: Option<String> = None;
        loop {
            let now = t0.elapsed().as_millis() as u64 + 1;
            match rx.recv_timeout(Duration::from_millis(tick_ms)) {
                Ok(Cmd::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    a.stop();
                    crate::voiceassist::set_worker_glyph(wid, crate::voiceassist::MicGlyph::Off);
                    return;
                }
                Ok(Cmd::Tap) => a.tap(now),
                Ok(Cmd::Stop) => a.stop(),
                Ok(Cmd::Approve(id, yes)) => {
                    a.approve(&id, yes);
                }
                Ok(Cmd::Confirm(yes)) => {
                    a.confirm(yes);
                }
                Ok(Cmd::Wake(t)) => waiting = Some(t),
                Err(_) => {}
            }
            if now.saturating_sub(last_live) >= 250 {
                last_live = now;
                let l = live();
                if l.muted != a.is_muted() {
                    a.set_muted(l.muted);
                }
                let c = a.config_mut();
                c.follow_ms = l.follow_ms;
                c.pause_ms = l.pause_ms;
                c.spoken_approvals = l.spoken_approvals;
            }
            a.tick(now);
            if let Some(t) = waiting.take() {
                if !a.wake_turn(now, &t) {
                    waiting = Some(t);
                }
            }
            let now_status = a.status();
            crate::voiceassist::set_worker_glyph(wid, crate::voiceassist::glyph_of(&now_status));
            *st.lock().unwrap_or_else(|e| e.into_inner()) = now_status;
        }
    });
    Worker { tx, status, join: Some(join) }
}

#[cfg(test)]
#[path = "voiceassist_ports_tests.rs"]
mod tests;
