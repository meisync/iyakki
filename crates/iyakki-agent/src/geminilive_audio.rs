//! Gemini's own voice for the live session (EXPERIMENTAL, `voice_out = gemini`; GEMINI-VOICE-DESIGN.md slice 4): playback of the model's
//! audio, barge-in, and the microphone's front end (echo cancellation and the gate that keeps the speaker from retriggering the model).
//!
//! * The audio the model sends (16-bit PCM, 24 kHz mono) is cut into short segments (the first after 300 ms, the next ones about
//!   800 ms each, the rest at the end of the turn) and each segment is played through the SAME shared audio path as every other
//!   sound ([`crate::story::Sink`]; the product's [`SpeechSink`] is `Services::play_wav_output` with the speech priority: the mixer
//!   ducks the radio and media, or the play command is used). There is no second playback route. The audio lives in memory only (the
//!   WAV of a segment is built in memory and handed to the sink; nothing is written to disk by this module).
//! * Before the first segment of a turn the speaker-hold rule applies ([`crate::soundhold`]): when another program holds the speaker
//!   a question is raised for the screen ([`AudioEvent::SpeakerHeld`]) and nothing plays until the person answers
//!   ([`AudioOut::answer_hold`]); a "no" drops the audio of that turn (the answer text stays on the screen).
//! * Barge-in ([`AudioOut::barge_in`]): the playing segment is stopped at once, the queue and the buffer are dropped, the echo
//!   reference is cut at that instant.
//! * [`MicFront`]: the microphone chunk goes through the echo canceller against what was played ([`crate::echocancel::StreamCleaner`]),
//!   and while the model is speaking it is sent to Google only when what is left is the person (a level gate with a short hold-over),
//!   so the speaker's own sound never reaches the model; the same gate raises the barge-in.
//! * The shared path has no stream for a growing sound, so segments are joined by overlap: the next segment is started [`OVERLAP_MS`]
//!   before the playing one ends, the end of the one fades out and the start of the other fades in, so the two add up to the same
//!   level and no gap is heard at the join (F-GEMVOICE-16). Heard on a speaker: not yet.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::echocancel::{resample_24k_to_16k, Reference, StreamCleaner};
use crate::geminilive::{End, EndReason, LiveEvent, LiveSession, Pcm, PcmSource};
use crate::story::{Playing, Sink};
use crate::voiceassist::TurnLog;

/// Sample rate of the model's voice.
pub const OUT_RATE: u32 = 24_000;
/// The first segment of a turn is started after this much audio has arrived (ms).
pub const FIRST_SEGMENT_MS: u64 = 300;
/// The following segments are about this long (ms).
pub const SEGMENT_MS: u64 = 800;
/// The next segment starts this long before the playing one ends; the end and the start fade over this time (ms).
pub const OVERLAP_MS: u64 = 60;
/// Audio waiting for the speaker question is kept up to this long (ms); more is dropped.
pub const HOLD_MAX_MS: u64 = 10_000;
/// The level (0 to 1, after the echo is taken out) above which the microphone carries a person.
pub const SPEECH_LEVEL: f32 = 0.15;
/// The person must be loud this long before it counts as a barge-in (ms).
pub const BARGE_MS: u64 = 120;
/// How much of the model's speech the canceller needs to have heard before the gate is at its normal strictness (ms).
pub const LEARN_MS: u64 = 1500;
/// After the person was last heard, microphone audio keeps flowing this long (ms) so the end of a word is not cut.
pub const HOLD_OVER_MS: u64 = 400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    /// Another program holds the speaker: this question goes to the screen (the person's answer: [`AudioOut::answer_hold`]).
    SpeakerHeld(String),
    /// A segment started playing.
    Started,
    /// Everything of the turn has been played.
    Done,
    /// A segment could not be played (the sink said why).
    Failed(String),
}

/// What a barge-in threw away (for the log and the tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    pub playing: bool,
    pub queued_ms: u64,
}

fn ms_of(bytes: usize) -> u64 {
    (bytes as u64 / 2) * 1000 / u64::from(OUT_RATE)
}

fn pcm_of(bytes: &[u8]) -> Vec<i16> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

pub type HeldFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

struct Cur {
    p: Box<dyn Playing>,
    /// When it started on the clock, and how long it is (ms): the next one starts a little before its end.
    start_ms: u64,
    len_ms: u64,
}

/// A linear fade in over the first `ms` milliseconds of 24 kHz sound.
fn fade_in(pcm: &mut [i16], ms: u64) {
    let n = ((OUT_RATE as u64 * ms / 1000) as usize).min(pcm.len());
    for (i, v) in pcm.iter_mut().take(n).enumerate() {
        *v = (f32::from(*v) * (i as f32 / n as f32)) as i16;
    }
}

/// A linear fade out over the last `ms` milliseconds.
fn fade_out(pcm: &mut [i16], ms: u64) {
    let n = ((OUT_RATE as u64 * ms / 1000) as usize).min(pcm.len());
    let len = pcm.len();
    for (i, v) in pcm[len - n..].iter_mut().enumerate() {
        *v = (f32::from(*v) * (1.0 - (i + 1) as f32 / n as f32)) as i16;
    }
}

/// The playback of the model's voice.
pub struct AudioOut {
    sink: Arc<dyn Sink>,
    reference: Arc<Mutex<Reference>>,
    held: HeldFn,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    pend: Vec<u8>,
    queue: VecDeque<Vec<u8>>,
    cur: Option<Cur>,
    /// The segment that was started early, while `cur` is still playing its last moments.
    next: Option<Cur>,
    turn_over: bool,
    in_turn: bool,
    hold: Hold,
    started_any: bool,
    /// After a barge-in the rest of the interrupted answer (already on its way) is not played.
    discard: bool,
    /// Milliseconds of sound played since the start (for the status line).
    pub played_ms: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hold {
    /// Not asked yet for this turn.
    Unasked,
    Asked,
    Allowed,
    /// The person said no: the audio of this turn is dropped.
    Refused,
}

impl AudioOut {
    /// `held`: when something else holds the speaker, the question to show (the product: `soundhold::conflict_for_new_player`);
    /// `clock`: milliseconds on the echo reference's clock.
    pub fn new(sink: Arc<dyn Sink>, reference: Arc<Mutex<Reference>>, held: HeldFn, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> AudioOut {
        AudioOut { sink, reference, held, clock, pend: Vec::new(), queue: VecDeque::new(), cur: None, next: None, turn_over: false, in_turn: false, hold: Hold::Unasked, started_any: false, discard: false, played_ms: 0 }
    }

    /// Audio of the model (24 kHz 16-bit PCM). Kept in memory until it is played.
    pub fn push(&mut self, pcm24: &[u8]) {
        if self.discard || self.hold == Hold::Refused || pcm24.len() % 2 != 0 {
            return;
        }
        if !self.in_turn {
            self.in_turn = true;
            self.turn_over = false;
            self.hold = Hold::Unasked;
            self.started_any = false;
        }
        self.pend.extend_from_slice(pcm24);
        let queued: u64 = self.queue.iter().map(|q| ms_of(q.len())).sum();
        if self.hold != Hold::Allowed && queued + ms_of(self.pend.len()) > HOLD_MAX_MS {
            // waiting for the speaker too long: the oldest sound goes
            self.queue.pop_front();
        }
        let want = if self.started_any || !self.queue.is_empty() { SEGMENT_MS } else { FIRST_SEGMENT_MS };
        if ms_of(self.pend.len()) >= want {
            self.cut();
        }
    }

    fn cut(&mut self) {
        if !self.pend.is_empty() {
            self.queue.push_back(std::mem::take(&mut self.pend));
        }
    }

    /// The model finished its turn: the rest is played.
    pub fn end_turn(&mut self) {
        self.cut();
        self.turn_over = true;
    }

    /// The person finished speaking: the next audio is a new answer and is played.
    pub fn accept_new_turn(&mut self) {
        self.discard = false;
    }

    /// The person's answer to the speaker question (the shell stopped the holder when it is yes).
    pub fn answer_hold(&mut self, yes: bool) {
        self.hold = if yes { Hold::Allowed } else { Hold::Refused };
        if !yes {
            self.queue.clear();
            self.pend.clear();
        }
    }

    pub fn playing(&mut self) -> bool {
        self.cur.as_mut().is_some_and(|c| c.p.is_playing()) || self.next.as_mut().is_some_and(|c| c.p.is_playing())
    }

    /// Sound is playing or waiting to play.
    pub fn busy(&mut self) -> bool {
        self.playing() || !self.queue.is_empty() || !self.pend.is_empty()
    }

    /// Barge-in: stop at once, drop everything not yet played, cut the echo reference at this instant.
    pub fn barge_in(&mut self) -> Dropped {
        let d = Dropped { playing: self.playing(), queued_ms: self.queue.iter().map(|q| ms_of(q.len())).sum::<u64>() + ms_of(self.pend.len()) };
        if let Some(mut c) = self.cur.take() {
            c.p.stop();
        }
        if let Some(mut c) = self.next.take() {
            c.p.stop();
        }
        self.queue.clear();
        self.pend.clear();
        self.reference.lock().unwrap_or_else(std::sync::PoisonError::into_inner).stop_all((self.clock)());
        // what the model still sends of this answer is not played either, until a new one is expected ([`AudioOut::accept_new_turn`])
        self.discard = true;
        self.in_turn = false;
        self.turn_over = false;
        self.started_any = false;
        self.hold = Hold::Unasked;
        d
    }

    /// Play one segment now: fades at its ends when it joins others, the echo reference, the event. `Err` is the sink's reason.
    fn start_segment(&mut self, seg: Vec<u8>, joined_before: bool) -> Result<Cur, String> {
        let mut pcm = pcm_of(&seg);
        // more is expected after this one (another segment queued, or the turn is not over): its end fades so the next can overlap it
        let more = !self.queue.is_empty() || !self.pend.is_empty() || !self.turn_over;
        if joined_before {
            fade_in(&mut pcm, OVERLAP_MS);
        }
        if more {
            fade_out(&mut pcm, OVERLAP_MS);
        }
        let wav = crate::voice::encode_wav(&pcm, OUT_RATE);
        let p = self.sink.play(&wav)?;
        let now = (self.clock)();
        self.reference.lock().unwrap_or_else(std::sync::PoisonError::into_inner).add(now, resample_24k_to_16k(&pcm));
        let len_ms = ms_of(seg.len());
        self.played_ms += len_ms;
        self.started_any = true;
        Ok(Cur { p, start_ms: now, len_ms })
    }

    /// Starts the next segment when the last one ended, and a little BEFORE it ends when another is ready, so the join has no gap. Called often.
    pub fn step(&mut self) -> Vec<AudioEvent> {
        let mut ev = Vec::new();
        if self.cur.as_mut().is_some_and(|c| !c.p.is_playing()) {
            if let Some(mut c) = self.cur.take() {
                if let Some(e) = c.p.error() {
                    ev.push(AudioEvent::Failed(e));
                }
            }
            // the one that was started early is the playing one now
            self.cur = self.next.take();
        }
        // an early start: the playing segment has OVERLAP_MS or less left and another one waits
        if self.hold == Hold::Allowed && self.next.is_none() && !self.queue.is_empty() {
            let now = (self.clock)();
            if self.cur.as_ref().is_some_and(|c| now + OVERLAP_MS >= c.start_ms + c.len_ms) {
                if let Some(seg) = self.queue.pop_front() {
                    match self.start_segment(seg, true) {
                        Ok(c) => self.next = Some(c),
                        Err(e) => {
                            self.queue.clear();
                            ev.push(AudioEvent::Failed(e));
                        }
                    }
                }
                return ev;
            }
        }
        if self.cur.is_none() {
            if self.queue.is_empty() && self.turn_over && !self.pend.is_empty() {
                self.cut();
            }
            if self.queue.is_empty() && self.turn_over && self.in_turn && self.pend.is_empty() {
                self.in_turn = false;
                if self.started_any {
                    ev.push(AudioEvent::Done);
                }
                return ev;
            }
            if self.queue.is_empty() {
                return ev;
            }
            match self.hold {
                Hold::Refused => {
                    self.queue.clear();
                    return ev;
                }
                Hold::Unasked => {
                    if let Some(q) = (self.held)() {
                        self.hold = Hold::Asked;
                        ev.push(AudioEvent::SpeakerHeld(q));
                        return ev;
                    }
                    self.hold = Hold::Allowed;
                }
                Hold::Asked => return ev,
                Hold::Allowed => {}
            }
            if let Some(seg) = self.queue.pop_front() {
                let joined = self.started_any;
                match self.start_segment(seg, joined) {
                    Ok(c) => {
                        self.cur = Some(c);
                        ev.push(AudioEvent::Started);
                    }
                    Err(e) => {
                        self.queue.clear();
                        ev.push(AudioEvent::Failed(e));
                    }
                }
            }
        }
        ev
    }
}

/// The product's sink: the shared audio entry point with the speech priority (the mixer ducks the radio and media). Nothing else.
pub struct SpeechSink(pub Arc<crate::services::Services>);

struct OnSpeaker(crate::audiosink::Output);

impl Playing for OnSpeaker {
    fn is_playing(&mut self) -> bool {
        self.0.is_playing()
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

impl Sink for SpeechSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        Ok(Box::new(OnSpeaker(self.0.play_wav_output(crate::mixer::SourceKind::Speech, "Gemini voice (experimental)", wav)?)))
    }
}

/// The product's speaker question: another program (or IYAKKI's own player) holds the speaker.
pub fn product_held() -> HeldFn {
    Arc::new(|| {
        let mixing = crate::audiosink::mixing();
        crate::soundhold::conflict_for_new_player("Gemini voice", mixing).map(|f| crate::soundhold::question(&f, "Gemini voice"))
    })
}

/// The product's way to end what holds the speaker (after the person said yes to the question): the same stop the radio uses.
pub fn product_stop_holder() -> Arc<dyn Fn() -> Result<String, String> + Send + Sync> {
    Arc::new(|| {
        let mixing = crate::audiosink::mixing();
        match crate::soundhold::conflict_for_new_player("Gemini voice", mixing) {
            Some(f) => crate::soundhold::global().stop(&f, &crate::quiet::System, std::path::Path::new(crate::quiet::ASOUND), std::time::Duration::from_secs(3)),
            None => Ok("Nothing holds the speaker now.".into()),
        }
    })
}

/// Lets other players ask this one to stop (the radio's "stop it so X can play"): announces the playback to the speaker-hold registry.
pub fn announce_to(reg: &crate::soundhold::Registry, busy: Arc<dyn Fn() -> bool + Send + Sync>, stop: Arc<dyn Fn() + Send + Sync>) {
    reg.announce("gemini-voice", Arc::new(move || busy().then(|| "The Gemini voice assistant is speaking".to_string())), stop);
}

// -------------------------------------------------------------------------------------------------------------- the microphone

/// What the microphone's front end decided for one chunk.
#[derive(Debug, PartialEq)]
pub struct Front {
    /// The chunk to send to Google (echo taken out), if any: while the model speaks, only what is the person.
    pub send: Option<Vec<u8>>,
    /// The person started talking over the model: stop the playback now.
    pub barge: bool,
    /// The level of what is left after the echo (0 to 1).
    pub level: f32,
    /// The person was speaking and has been quiet for the hold-over: their speech ended at this time (ms).
    pub speech_ended_at: Option<u64>,
}

pub struct MicFront {
    cleaner: StreamCleaner,
    voiced_ms: u64,
    last_voice: Option<u64>,
    was_speaking: bool,
    /// Milliseconds of the model's speech the filter has had to learn the room from.
    learned_ms: u64,
    /// Echo cancellation is part of the front end here whatever the My IYAKKI setting says: without it the model would hear itself.
    pub always_clean: bool,
}

impl MicFront {
    pub fn new(reference: Arc<Mutex<Reference>>) -> MicFront {
        MicFront { cleaner: StreamCleaner::new(reference), voiced_ms: 0, last_voice: None, was_speaking: false, learned_ms: 0, always_clean: true }
    }

    /// `pcm` is 16 kHz 16-bit little-endian PCM that ended at `now_ms`; `model_speaking`: the playback is on.
    pub fn step(&mut self, pcm: &[u8], now_ms: u64, model_speaking: bool) -> Front {
        let samples = pcm_of(pcm);
        if samples.is_empty() {
            return Front { send: None, barge: false, level: 0.0, speech_ended_at: None };
        }
        let len_ms = samples.len() as u64 * 1000 / 16_000;
        let (clean, level) = self.cleaner.process(&samples, now_ms);
        // while the filter is still learning the room (the first seconds of the model's voice) its left-over echo is louder: the gate
        // is twice as strict until it has had LEARN_MS of the model's speech
        let strict = if self.learned_ms < LEARN_MS { 2.0 } else { 1.0 };
        if model_speaking {
            self.learned_ms += len_ms;
        }
        let voice = level >= SPEECH_LEVEL * strict;
        let mut speech_ended_at = None;
        if voice {
            self.voiced_ms += len_ms;
            self.last_voice = Some(now_ms);
            self.was_speaking = true;
        } else {
            self.voiced_ms = 0;
            if self.was_speaking && self.last_voice.is_some_and(|t| now_ms.saturating_sub(t) >= HOLD_OVER_MS) {
                self.was_speaking = false;
                speech_ended_at = self.last_voice;
            }
        }
        let barge = model_speaking && self.voiced_ms >= (BARGE_MS as f32 * strict) as u64;
        let recent = self.last_voice.is_some_and(|t| now_ms.saturating_sub(t) <= HOLD_OVER_MS);
        let send = if !model_speaking || voice || recent { Some(clean.iter().flat_map(|s| s.to_le_bytes()).collect()) } else { None };
        Front { send, barge, level, speech_ended_at }
    }
}

/// Runs a live session with Gemini's own voice. The microphone goes through [`MicFront`], the model's audio through [`AudioOut`]; the
/// person talking over the model stops it at once. `now_ms` is the echo reference's clock. Returns the end of the session.
#[allow(clippy::too_many_arguments)]
pub fn run_gemini_voice(s: &mut LiveSession, mic: &mut dyn PcmSource, front: &mut MicFront, out: &mut AudioOut, log: &mut dyn TurnLog, now_ms: &dyn Fn() -> u64, on_event: &mut dyn FnMut(&AudioEvent), hold: &mut dyn FnMut() -> Option<bool>) -> End {
    let mut released = false;
    loop {
        if let Some(e) = s.ended() {
            return e.clone();
        }
        let speaking = out.busy();
        if !released {
            match mic.next(std::time::Duration::from_millis(20)) {
                Pcm::Chunk(c) => {
                    let f = front.step(&c, now_ms(), speaking);
                    if f.barge {
                        out.barge_in();
                        s.barge_in();
                    }
                    if f.speech_ended_at.is_some() {
                        s.mark_speech_end();
                        out.accept_new_turn();
                    }
                    if let Some(bytes) = f.send {
                        if let Err(e) = s.send_audio(&bytes) {
                            return e;
                        }
                    }
                }
                Pcm::Quiet => {}
                Pcm::Released => {
                    released = true;
                    s.mark_speech_end();
                    out.accept_new_turn();
                    if let Err(e) = s.end_turn() {
                        return e;
                    }
                }
                Pcm::Gone => return s.close(EndReason::Stopped, "The microphone is not available any more."),
            }
        }
        for ev in s.poll(std::time::Duration::from_millis(if released { 50 } else { 1 })) {
            match ev {
                LiveEvent::Audio(b) => out.push(&b),
                LiveEvent::TurnEnd => out.end_turn(),
                LiveEvent::Reply(t) => {
                    let heard = s.take_heard();
                    if !heard.trim().is_empty() {
                        log.record(crate::voiceassist::Who::Person, heard.trim());
                    }
                    log.record(crate::voiceassist::Who::Assistant, &t);
                }
                LiveEvent::Interrupted => {
                    out.barge_in();
                }
                LiveEvent::Ended(e) => return e,
                _ => {}
            }
        }
        if let Some(yes) = hold() {
            out.answer_hold(yes);
        }
        for a in out.step() {
            on_event(&a);
            if a == AudioEvent::Done && released {
                return s.close(EndReason::Finished, "Answered.");
            }
        }
    }
}
