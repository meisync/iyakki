//! The Gemini voice session as the shell runs it (EXPERIMENTAL): one manual session on its own thread, the person's screen as a board
//! of cards, and the one state the shell draws. The shell brings the real parts (the microphone through the Voice tile's capture path
//! `wakelisten::Mic`, the shared sink, the local voice, the chat's tool set of the account); this file wires them to
//! `geminilive` / `geminilive_tools` / `geminilive_audio` and holds the rules:
//!
//! * [`Board`] is the [`Screen`] of the tool dispatcher: a tool that asks becomes a card on it; the shell draws the card (Allow, Deny)
//!   and only the person's press ([`Board::press`]) answers. The speaker question ("Radio is playing. Stop it so Gemini can speak?")
//!   is a card of the same board, with the same buttons.
//! * One session at a time. The manual start key opens it (hands free: Gemini hears when the person stops talking); [`Running::finish_turn`]
//!   ("I am done talking") hands the turn over and ends the session after the answer; [`Running::stop`] / Esc / `request_stop` end it
//!   at once and close the microphone.
//! * Anything that cannot start comes back as one line ([`RunState::Fallback`]) and the local voice answers instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent::Tools;
use crate::geminilive::{self, EndReason, LiveDeps, Pcm, PcmSource, ToolHandler, VoiceOut};
use crate::geminilive_audio::{run_gemini_voice, AudioEvent, AudioOut, HeldFn, MicFront};
use crate::geminilive_tools::{AuditFn, Card, Dispatcher, Opts, Screen};
use crate::story::Sink;
use crate::voiceassist::{Speaker, TurnLog};
use crate::wakelisten::Mic;
pub use iyakki_shared::geminilive_host::{RunState};

pub use iyakki_shared::geminilive_host::{Board, Running, SPEAKER_CARD};

/// What a session needs from the outside.
pub struct Parts {
    pub live: LiveDeps,
    /// The chat's tool set of the account; `None`: no tools in this session.
    pub tools: Option<Arc<dyn Tools>>,
    pub board: Arc<Board>,
    pub audit: AuditFn,
    /// The chat's Auto mode switch (never covers the always-ask tools).
    pub auto: Arc<dyn Fn() -> bool + Send + Sync>,
    pub mic: Box<dyn Mic>,
    pub sink: Arc<dyn Sink>,
    pub speaker: Box<dyn Speaker>,
    pub log: Box<dyn TurnLog>,
    pub held: HeldFn,
    pub stop_holder: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    /// Milliseconds on the echo reference's clock.
    pub echo_clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

struct MicPcm {
    mic: Box<dyn Mic>,
    done: Arc<AtomicBool>,
}

impl PcmSource for MicPcm {
    fn next(&mut self, wait: Duration) -> Pcm {
        if self.done.swap(false, Ordering::SeqCst) {
            return Pcm::Released;
        }
        std::thread::sleep(wait.min(Duration::from_millis(30)));
        if !self.mic.alive() {
            return Pcm::Gone;
        }
        let s = self.mic.drain();
        if s.is_empty() { Pcm::Quiet } else { Pcm::Chunk(s.iter().flat_map(|v| v.to_le_bytes()).collect()) }
    }
}

fn geminilive_stop() {
    geminilive::request_stop();
}

fn set(state: &Arc<Mutex<RunState>>, s: RunState) {
    *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = s;
}

/// Starts a session on its own thread. Nothing is sent before the consent gate says yes (inside [`geminilive::start_with`]); the
/// microphone is opened only after the session is open, and closed when it ends.
pub fn spawn(parts: Parts) -> Running {
    let state = Arc::new(Mutex::new(RunState::Connecting));
    let done = Arc::new(AtomicBool::new(false));
    let (st, dn) = (state.clone(), done.clone());
    let gen0 = geminilive::stop_generation();
    let join = std::thread::Builder::new().name("iyakki-gemini-voice".into()).spawn(move || body(parts, &st, dn, gen0)).ok();
    if join.is_none() {
        set(&state, RunState::Fallback(geminilive::fallback_line("the session could not start (no thread).")));
    }
    Running::new(state, done, join, Arc::new(geminilive_stop))
}

fn body(parts: Parts, state: &Arc<Mutex<RunState>>, done: Arc<AtomicBool>, gen0: u64) {
    let Parts { live, tools, board, audit, auto, mut mic, sink, mut speaker, mut log, held, stop_holder, echo_clock } = parts;
    let (handler, decls): (Option<Box<dyn ToolHandler>>, Vec<serde_json::Value>) = match tools {
        Some(t) => {
            let opts = Opts { auto, now_ms: live.now_ms.clone(), approval_ms: crate::geminilive_tools::APPROVAL_MS, result_chars: crate::geminilive_tools::RESULT_CHARS, max_calls: crate::geminilive_tools::MAX_CALLS };
            let d = Dispatcher::new(t, board.clone(), audit.clone(), opts);
            let decls = d.catalogue().decls.clone();
            (Some(Box::new(d)), decls)
        }
        None => (None, Vec::new()),
    };
    let mut session = match geminilive::start_with_gen(&live, handler, &decls, gen0) {
        Ok(s) => s,
        Err(line) => {
            audit("voice.session", &format!("not started: {}", line.chars().take(160).collect::<String>()));
            set(state, RunState::Fallback(line));
            return;
        }
    };
    if let Err(e) = mic.open() {
        let end = session.close(EndReason::Failed, &format!("The microphone could not be opened ({e})."));
        audit("voice.session", "microphone could not be opened");
        set(state, RunState::Fallback(geminilive::fallback_line(&end.line)));
        return;
    }
    audit("voice.session", "started (EXPERIMENTAL; the microphone is open)");
    set(state, RunState::Open);
    let mut src = MicPcm { mic, done };
    let voice_out = session.cfg().voice_out;
    let end = if voice_out == VoiceOut::Gemini {
        let reference = Arc::new(Mutex::new(crate::echocancel::Reference::default()));
        let mut front = MicFront::new(reference.clone());
        let mut out = AudioOut::new(sink, reference, held, echo_clock.clone());
        let b2 = board.clone();
        let b3 = board.clone();
        // other players can see that Gemini speaks and ask it to stop ("stop it so the radio can play"): the same registry the radio uses
        let speaking = Arc::new(AtomicBool::new(false));
        let (sp1, sp2) = (speaking.clone(), speaking.clone());
        crate::geminilive_audio::announce_to(&crate::soundhold::global(), Arc::new(move || sp1.load(Ordering::SeqCst)), Arc::new(geminilive_stop));
        let stop2 = stop_holder.clone();
        run_gemini_voice(
            &mut session,
            &mut src,
            &mut front,
            &mut out,
            log.as_mut(),
            &*echo_clock,
            &mut |a| {
                match a {
                    AudioEvent::Started => sp2.store(true, Ordering::SeqCst),
                    AudioEvent::Done | AudioEvent::Failed(_) => sp2.store(false, Ordering::SeqCst),
                    AudioEvent::SpeakerHeld(_) => {}
                }
                if let AudioEvent::SpeakerHeld(q) = a {
                    b2.ask(Card { id: SPEAKER_CARD.into(), name: "Gemini voice".into(), summary: q.clone(), detail: String::new() });
                }
            },
            &mut || {
                let yes = b3.answer(SPEAKER_CARD)?;
                b3.withdraw(SPEAKER_CARD);
                if yes {
                    let _ = stop2();
                }
                Some(yes)
            },
        )
    } else {
        let _ = (&sink, &held, &stop_holder);
        geminilive::run(&mut session, &mut src, speaker.as_mut(), log.as_mut())
    };
    crate::soundhold::global().forget("gemini-voice");
    audit("voice.session", &format!("ended: {}", end.line.chars().take(160).collect::<String>()));
    drop(session);
    src.mic.close();
    board.clear();
    set(state, RunState::Ended(end.line));
}

/// Whether the session ended in a way that the person should hear the local voice instead (it could not start).
pub fn is_fallback(s: &RunState) -> Option<&str> {
    match s {
        RunState::Fallback(l) => Some(l),
        _ => None,
    }
}
