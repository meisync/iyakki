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

// ------------------------------------------------------------------------------------------------ microphone

/// The microphone: the Voice tile's recorders and record command (one microphone choice for dictation, the recorder and this).
pub struct RecorderAudio {
    pub recorders: Arc<dyn Recorders>,
    pub svc: Arc<Services>,
    cap: Option<Box<dyn Capture>>,
}

impl RecorderAudio {
    pub fn new(recorders: Arc<dyn Recorders>, svc: Arc<Services>) -> RecorderAudio {
        RecorderAudio { recorders, svc, cap: None }
    }
}

impl AudioIn for RecorderAudio {
    fn open(&mut self) -> Result<(), String> {
        if self.cap.is_some() {
            return Ok(());
        }
        let mics = self.recorders.mics();
        let v = self.svc.voice_settings();
        let mic = vd::pick_mic(&mics, &v.mic);
        if mic.is_none() && mics.is_empty() {
            return Err(vd::no_mic_text_t2(&self.recorders.product(), vd::t2_mic_hidden(&vd::proc_root())));
        }
        let cmd = vd::record_cmd_for(&v.record_cmd, mic);
        self.cap = Some(self.recorders.start(&cmd, (MAX_LISTEN_MS / 1000 + 10) as usize)?);
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
            Some(c) => c.stop(),
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

impl Stt for ServicesStt {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String> {
        let engine = vd::stt_engine(&self.svc).ok_or_else(|| "No speech-to-text is set up (Settings -> Voice, or download a Whisper model in Settings -> Models).".to_string())?;
        if vd::consent_text(&self.svc, &engine, true).is_some() {
            return Err("The speech-to-text engine is on another machine you have not agreed to send your voice to yet: open the Voice tile and say yes there first.".into());
        }
        let text = vd::transcribe(&self.svc, &engine, pcm)?;
        // the engines used here do not report a confidence; the machine's short-transcript rule is the safety net
        Ok(Heard { text, confidence: None })
    }
}

// ------------------------------------------------------------------------------------------------ speaker

struct Job {
    busy: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

/// The speaker: the chosen reading voice, played through the shared audio entry point (`Services::play_wav_output`) so the
/// system volume and the output choice apply. One sentence at a time on a helper thread.
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
        std::thread::spawn(move || {
            let done = || busy.store(false, Ordering::SeqCst);
            let Ok(wav) = synth.synth(&text) else { return done() };
            if stop.load(Ordering::SeqCst) {
                return done();
            }
            let Ok(mut playing) = sink.play(&wav) else { return done() };
            while playing.is_playing() {
                if stop.load(Ordering::SeqCst) {
                    playing.stop();
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            done();
        });
        Ok(())
    }
    fn busy(&mut self) -> bool {
        self.job.as_ref().is_some_and(|j| j.busy.load(Ordering::SeqCst))
    }
    fn stop(&mut self) {
        if let Some(j) = &self.job {
            j.stop.store(true, Ordering::SeqCst);
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
            Who::Person => Message::user(text),
            Who::Assistant => Message::assistant(text),
        });
        let _ = self.svc.sessions.save(s);
    }
}

// ------------------------------------------------------------------------------------------------ the worker

/// What the screen asks of the worker.
pub enum Cmd {
    Tap,
    Stop,
    /// A tap or a key on the approval card.
    Approve(String, bool),
    /// A tap or a key on "did you say ...".
    Confirm(bool),
    Quit,
}

/// The settings the worker follows (read about four times a second, so a change from the chat tool or Settings acts at once).
#[derive(Clone, Copy, Debug)]
pub struct Live {
    pub muted: bool,
    pub follow_ms: u64,
    pub pause_ms: u64,
}

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
    let mut a = Assistant::new(ports, Config { pause_ms: l0.pause_ms, follow_ms: l0.follow_ms, ..cfg }, l0.muted);
    let status = Arc::new(Mutex::new(a.status()));
    let st = status.clone();
    let join = std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut last_live = 0u64;
        loop {
            let now = t0.elapsed().as_millis() as u64 + 1;
            match rx.recv_timeout(Duration::from_millis(tick_ms)) {
                Ok(Cmd::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    a.stop();
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
            }
            a.tick(now);
            *st.lock().unwrap_or_else(|e| e.into_inner()) = a.status();
        }
    });
    Worker { tx, status, join: Some(join) }
}
