//! The agent-side pieces of the TALES engine (F-SPLIT-03): the speaker process, the text-to-speech engine of the services and the
//! follower of an external agent's run. The rest of the engine is `iyakki_shared::story`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use iyakki_shared::story::*;
use iyakki_shared::speechtime::{Timed, TimingSource};
use crate::voice::{self, SpeakOpts};
use iyakki_wordtime as wordtime;
use iyakki_wordtime::{WordClock, WordTimeline};

impl Playing for voice::Player {
    fn is_playing(&mut self) -> bool {
        voice::Player::is_playing(self)
    }
    fn stop(&mut self) {
        voice::Player::stop(self)
    }
    fn played_ms(&mut self) -> Option<u64> {
        voice::Player::played_ms(self)
    }
}

/// The speaker command of Settings → Voice (`aplay -q` by default): the WAV goes to its standard input.
pub struct CommandSink {
    pub play_cmd: String,
}

impl Sink for CommandSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        Ok(Box::new(voice::Player::start_streamed(&self.play_cmd, wav)?))
    }
}

/// The text-to-speech engine of Settings → Voice, with the voice and speed picked on the page.
pub struct ServicesSynth {
    pub svc: Arc<crate::services::Services>,
    pub opts: SpeakOpts,
}

impl Synth for ServicesSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        self.svc.synthesize_timed(text, &self.opts).map(|(w, _)| w)
    }
    fn synth_timed(&self, text: &str) -> Result<(Vec<u8>, Option<Timed>), String> {
        self.svc.synthesize_timed(text, &self.opts)
    }
    fn align(&self, text: &str, wav: &[u8]) -> Option<Timed> {
        self.svc.align_speech(text, wav, self.opts.lang.as_deref())
    }
}


/// The story store of `user` (home folder and writable folders as the account has them).
pub fn open(policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> iyakki_shared::story::StoryStore {
    open_at(policy, user, data_dir, crate::scope::home_root(user, data_dir))
}

/// The store of `user` whose home folder is `home` (tests, and accounts whose home is not the usual one).
pub fn open_at(policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: std::path::PathBuf) -> iyakki_shared::story::StoryStore {
    let roots = crate::scope::file_roots_with_home(policy, user, data_dir, home.clone());
    iyakki_shared::story::StoryStore::open_with(policy, user, home, roots)
}

// ------------------------------------------------------------------------------------------------------------ the reader







struct St {
    gen: u64,
    chunks: Vec<Chunk>,
    wavs: Vec<Option<Arc<Vec<u8>>>>,
    envs: Vec<Vec<f32>>,
    secs: Vec<f32>,
    /// The silence at the start and at the end of each piece's sound (ms), measured from it: the words fill the speech between.
    edges: Vec<(f32, f32)>,
    /// Word times measured for each piece (the engine's own, or an alignment), when there are any.
    timed: Vec<Option<Timed>>,
    /// Where the word times of the clock come from.
    source: TimingSource,
    cur: usize,
    status: Status,
    paused: bool,
    /// Counts the pauses: a pause that is ended again before the playing thread looked still makes the piece start over.
    pause_seq: u64,
    /// More pieces may still be added (the story is being written).
    open: bool,
    /// When the piece that plays began, on the reader's time (milliseconds, see [`St::now_ms`]).
    started: Option<f32>,
    voice_key: String,
    synth: Option<Arc<dyn Synth>>,
    sink: Option<Arc<dyn Sink>>,
    /// A piece the engine could not make, and why: reading goes on until it gets there.
    fail: Option<(usize, String)>,
    /// Follows the word being said in the piece that plays (None when word highlighting is off or nothing plays).
    clock: Option<WordClock>,
    words_on: bool,
    /// Speaking speed used by the rate estimate of the word timing (1.0 = normal).
    speed: f32,
    epoch: Instant,
    /// Replaces the real clock (milliseconds) when set: tests move time themselves instead of waiting for it.
    time: Option<TimeSource>,
}

/// A source of time in milliseconds.
pub type TimeSource = Arc<dyn Fn() -> f32 + Send + Sync>;

impl St {
    fn now_ms(&self) -> f32 {
        match &self.time {
            Some(f) => f(),
            None => self.epoch.elapsed().as_secs_f32() * 1000.0,
        }
    }

    /// The clock for piece `i`: its measured word times when there are any, else fitted to its sound when the length is
    /// known, a rate estimate otherwise; and where the times come from.
    /// How the estimate was fitted to the sound of piece `i` (None when the times were measured or the length is unknown).
    fn fit_report(&self, i: usize) -> Option<wordtime::FitReport> {
        let c = self.chunks.get(i)?;
        let secs = self.secs.get(i).copied().filter(|s| *s > 0.0)?;
        if self.timed.get(i).is_some_and(|t| t.is_some()) {
            return None;
        }
        let (lead, trail) = self.edges.get(i).copied().unwrap_or((0.0, 0.0));
        let env = self.envs.get(i).map(Vec::as_slice).unwrap_or(&[]);
        Some(WordTimeline::fitted_pinned(&c.text, secs * 1000.0, lead, trail, env).1)
    }

    /// Which timeline the clock of piece `i` uses, in words, for the log.
    fn timeline_path(&self, i: usize) -> String {
        if let Some(Some(t)) = self.timed.get(i) {
            return format!("{} word times", t.source.label());
        }
        let secs = self.secs.get(i).copied().unwrap_or(0.0);
        if secs <= 0.0 {
            return "rate estimate: the length of the sound is unknown".into();
        }
        match self.fit_report(i) {
            Some(f) if f.pins > 0 => format!("pinned {} pause(s) to punctuation", f.pins),
            _ if self.envs.get(i).is_none_or(|e| e.is_empty()) => "plain fit: no loudness".into(),
            _ => "plain fit: no silences matched to punctuation".into(),
        }
    }

    fn clock_for(&self, i: usize) -> Option<(WordClock, TimingSource)> {
        if !self.words_on {
            return None;
        }
        let c = self.chunks.get(i)?;
        if let Some((tl, src)) = self.timed.get(i).cloned().flatten().and_then(|t| t.timeline(&c.text).map(|tl| (tl, t.source))) {
            return Some((WordClock::new(tl, wordtime::START_LATENCY_MS), src));
        }
        let secs = self.secs.get(i).copied().unwrap_or(0.0);
        let (lead, trail) = self.edges.get(i).copied().unwrap_or((0.0, 0.0));
        let env = self.envs.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let tl = if secs > 0.0 {
            let (tl, rep) = WordTimeline::fitted_pinned(&c.text, secs * 1000.0, lead, trail, env);
            log::debug!("reading: sentence {}: word timeline stretch factor {:.2}, {} pause(s) pinned to punctuation, worst estimated-versus-pinned boundary error {:.0} ms", i + 1, rep.stretch, rep.pins, rep.worst_boundary_error_ms);
            log::debug!("reading: sentence {} words: {}", i + 1, tl.debug_line(&c.text));
            tl
        } else {
            WordTimeline::estimate(&c.text, self.speed)
        };
        Some((WordClock::new(tl, wordtime::START_LATENCY_MS), TimingSource::Estimated))
    }
}

struct Shared {
    st: Mutex<St>,
    cv: Condvar,
}

/// Reads pieces aloud: while piece N plays, pieces N + 1 and N + 2 are being made. Cheap to clone; all clones are the same
/// reader. Stopping (or starting something else) ends the playing process within a few hundredths of a second.
#[derive(Clone)]
pub struct Reader {
    sh: Arc<Shared>,
}

/// How many pieces are made ahead of the one that plays.
pub const LOOKAHEAD: usize = 2;
const POLL: Duration = Duration::from_millis(15);

impl Default for Reader {
    fn default() -> Self {
        Reader::new()
    }
}

impl Reader {
    pub fn new() -> Reader {
        Reader {
            sh: Arc::new(Shared {
                st: Mutex::new(St { gen: 0, chunks: Vec::new(), wavs: Vec::new(), envs: Vec::new(), secs: Vec::new(), edges: Vec::new(), timed: Vec::new(), source: TimingSource::Estimated, cur: 0, status: Status::Idle, paused: false, pause_seq: 0, open: false, started: None, voice_key: String::new(), synth: None, sink: None, fail: None, clock: None, words_on: true, speed: 1.0, epoch: Instant::now(), time: None }),
                cv: Condvar::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, St> {
        self.sh.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start reading `chunks` from piece `from`. `open` says that more pieces will follow through [`Reader::append`] (the
    /// story is still being written). Sound already made for the same text with the same `voice_key` is kept, so a replay
    /// or a seek costs nothing; a different `voice_key` drops it.
    pub fn start(&self, chunks: Vec<Chunk>, from: usize, open: bool, voice_key: &str, synth: Arc<dyn Synth>, sink: Arc<dyn Sink>) {
        let gen = {
            let mut st = self.lock();
            st.gen += 1;
            let same_voice = st.voice_key == voice_key;
            let mut wavs = vec![None; chunks.len()];
            let mut envs = vec![Vec::new(); chunks.len()];
            let mut secs = vec![0.0; chunks.len()];
            let mut edges = vec![(0.0, 0.0); chunks.len()];
            let mut timed = vec![None; chunks.len()];
            if same_voice {
                for (i, c) in chunks.iter().enumerate() {
                    if st.chunks.get(i).is_some_and(|o| o.text == c.text) {
                        wavs[i] = st.wavs[i].clone();
                        envs[i] = std::mem::take(&mut st.envs[i]);
                        secs[i] = st.secs[i];
                        edges[i] = st.edges.get(i).copied().unwrap_or((0.0, 0.0));
                        timed[i] = st.timed.get(i).cloned().flatten();
                    }
                }
            }
            st.chunks = chunks;
            st.wavs = wavs;
            st.envs = envs;
            st.secs = secs;
            st.edges = edges;
            st.timed = timed;
            st.voice_key = voice_key.to_string();
            st.cur = from.min(st.chunks.len());
            st.status = Status::Buffering;
            st.paused = false;
            st.open = open;
            st.started = None;
            st.synth = Some(synth.clone());
            st.sink = Some(sink.clone());
            st.fail = None;
            st.clock = None;
            st.gen
        };
        self.sh.cv.notify_all();
        let (a, b) = (self.sh.clone(), self.sh.clone());
        let from = from;
        std::thread::spawn(move || synth_loop(a, gen, from, synth));
        std::thread::spawn(move || play_loop(b, gen, from, sink));
    }

    /// Add pieces at the end (more of the story was written).
    pub fn append(&self, more: Vec<Chunk>) {
        let mut st = self.lock();
        for c in more {
            st.chunks.push(c);
            st.wavs.push(None);
            st.envs.push(Vec::new());
            st.secs.push(0.0);
            st.edges.push((0.0, 0.0));
            st.timed.push(None);
        }
        drop(st);
        self.sh.cv.notify_all();
    }

    /// No more pieces will be added: the reader finishes after the last one.
    pub fn close_input(&self) {
        self.lock().open = false;
        self.sh.cv.notify_all();
    }

    /// Hold the sound. The piece that was playing starts again from its beginning on [`Reader::resume`].
    pub fn pause(&self) {
        let mut st = self.lock();
        if matches!(st.status, Status::Playing | Status::Buffering) {
            st.paused = true;
            st.pause_seq += 1;
            st.status = Status::Paused;
            let now = st.now_ms();
            if let Some(c) = st.clock.as_mut() {
                c.pause(now);
            }
        }
        drop(st);
        self.sh.cv.notify_all();
    }

    pub fn resume(&self) {
        let mut st = self.lock();
        if st.paused {
            st.paused = false;
            st.status = Status::Buffering;
        }
        drop(st);
        self.sh.cv.notify_all();
    }

    /// Stop at once (the speaker process is ended) and forget the position.
    pub fn stop(&self) {
        let mut st = self.lock();
        st.gen += 1;
        st.paused = false;
        st.started = None;
        st.clock = None;
        if !matches!(st.status, Status::Failed(_)) {
            st.status = Status::Idle;
        }
        drop(st);
        self.sh.cv.notify_all();
    }

    /// Carry on from piece `chunk` (a click on the progress bar, previous/next, replay).
    pub fn seek(&self, chunk: usize) {
        let (chunks, open, key, synth, sink) = {
            let st = self.lock();
            (st.chunks.clone(), st.open, st.voice_key.clone(), st.synth.clone(), st.sink.clone())
        };
        let (Some(synth), Some(sink)) = (synth, sink) else { return };
        let chunk = chunk.min(chunks.len().saturating_sub(1));
        self.start(chunks, chunk, open, &key, synth, sink);
    }

    pub fn is_active(&self) -> bool {
        matches!(self.lock().status, Status::Buffering | Status::Playing | Status::Paused)
    }

    /// Tell [`crate::soundhold`] that this reader holds the speaker while it reads aloud (the browser's "Stop it" dialog names it
    /// as TALES and stops it with [`Reader::stop`]). The latest reader announced is the one the dialog stops.
    pub fn announce_holder(&self) {
        let (a, b) = (self.clone(), self.clone());
        crate::soundhold::announce(
            "tales",
            Arc::new(move || matches!(a.status(), Status::Buffering | Status::Playing).then(|| "TALES is reading".to_string())),
            Arc::new(move || b.stop()),
        );
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.lock();
        let total = st.chunks.len();
        let cur = st.cur.min(total.saturating_sub(1));
        let weight = |i: usize| if st.secs[i] > 0.0 { st.secs[i] } else { st.chunks[i].text.chars().count() as f32 / 14.0 };
        let all: f32 = (0..total).map(weight).sum();
        let before: f32 = (0..cur).map(weight).sum();
        let into = match (&st.status, st.started) {
            (Status::Playing, Some(t)) if total > 0 => ((st.now_ms() - t).max(0.0) / 1000.0).min(weight(cur)),
            _ => 0.0,
        };
        let fraction = if st.status == Status::Done { 1.0 } else if all > 0.0 { ((before + into) / all).clamp(0.0, 1.0) } else { 0.0 };
        let c = st.chunks.get(cur);
        let now = st.now_ms();
        let word = match (&st.status, c, &st.clock) {
            (Status::Playing | Status::Paused, Some(c), Some(k)) => k.range(now).map(|(a, b)| (c.start + a, c.start + b)),
            _ => None,
        };
        let word_exact = word.is_some() && st.clock.as_ref().is_some_and(|k| k.mode(now) == wordtime::TimingMode::Exact);
        let timing = if word_exact { TimingSource::Voice } else { st.source };
        Snapshot { status: st.status.clone(), chunk: cur, unit: c.map_or(0, |c| c.unit), range: c.map_or((0, 0), |c| (c.start, c.end)), chunks: total, fraction, ready: st.wavs.iter().filter(|w| w.is_some()).count(), word, word_exact, timing }
    }

    /// Which timeline the word clock of the piece that plays uses (the same words as the "timeline:" part of the sentence log line).
    pub fn timeline_note(&self) -> String {
        let st = self.lock();
        st.timeline_path(st.cur)
    }

    /// The reader's time in milliseconds: the real clock, or the one given to [`Reader::set_time_source`].
    pub fn now_ms(&self) -> f32 {
        self.lock().now_ms()
    }

    /// Use `time` (milliseconds) instead of the real clock for the word timing and the progress: tests move it by hand.
    pub fn set_time_source(&self, time: TimeSource) {
        self.lock().time = Some(time);
    }

    /// Follow the word being said inside the piece (on by default). Takes effect at the next piece.
    pub fn set_words(&self, on: bool) {
        let mut st = self.lock();
        st.words_on = on;
        if !on {
            st.clock = None;
        }
    }

    /// The speaking speed (1.0 = normal) the word timing assumes when the length of the sound is not known.
    pub fn set_speed(&self, speed: f32) {
        self.lock().speed = if speed.is_finite() { speed.clamp(0.25, 4.0) } else { 1.0 };
    }

    /// How loud the sound is right now, 0 to 1, from the audio itself (0 unless something is playing).
    pub fn level(&self) -> f32 {
        let st = self.lock();
        let (Status::Playing, Some(t)) = (&st.status, st.started) else { return 0.0 };
        let env = st.envs.get(st.cur).map(Vec::as_slice).unwrap_or(&[]);
        if env.is_empty() {
            return 0.5; // sound of unknown shape
        }
        let k = ((st.now_ms() - t).max(0.0) / 20.0) as usize;
        env[k.min(env.len() - 1)..(k + 3).min(env.len())].iter().copied().fold(0.0, f32::max)
    }

    /// The sound made so far for every piece (for saving the audio).
    pub fn sounds(&self) -> (Vec<Chunk>, Vec<Option<Arc<Vec<u8>>>>) {
        let st = self.lock();
        (st.chunks.clone(), st.wavs.clone())
    }
}

fn synth_loop(sh: Arc<Shared>, gen: u64, from: usize, synth: Arc<dyn Synth>) {
    let mut i = from;
    loop {
        let text = {
            let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if st.gen != gen {
                    return;
                }
                if i >= st.chunks.len() {
                    if !st.open {
                        return;
                    }
                } else if st.wavs[i].is_some() {
                    i += 1;
                    continue;
                } else if i <= st.cur + LOOKAHEAD {
                    break st.chunks[i].text.clone();
                }
                st = sh.cv.wait_timeout(st, POLL * 4).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        let r = synth.synth_timed(&text);
        let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if st.gen != gen {
            return;
        }
        match r {
            Ok((w, timed)) => {
                let (env, secs) = envelope(&w);
                let w = Arc::new(w);
                let need_align = timed.is_none() && st.words_on;
                if i < st.wavs.len() {
                    st.wavs[i] = Some(w.clone());
                    st.edges[i] = speech_edges(&env, secs);
                    st.envs[i] = env;
                    st.secs[i] = secs;
                    st.timed[i] = timed;
                }
                let key = st.voice_key.clone();
                drop(st);
                sh.cv.notify_all();
                if need_align {
                    // the alignment runs beside the reading: the piece starts with estimated times and switches when it is done
                    let (sh2, synth2) = (sh.clone(), synth.clone());
                    let _ = std::thread::Builder::new().name("iyakki-align".into()).spawn(move || align_piece(sh2, synth2, i, text, w, key));
                }
                i += 1;
            }
            Err(e) => {
                // what is already made keeps playing; the failure shows when reading gets to this piece
                st.fail = Some((i, format!("Could not make the speech for part {} of {}: {e}", i + 1, st.chunks.len())));
                drop(st);
                sh.cv.notify_all();
                return;
            }
        }
    }
}

/// Ask for the word times of piece `i` after its sound was made, and use them (also for the piece that plays now: the lit
/// word moves to where the sound really is). Dropped when the piece's text or the voice changed meanwhile.
fn align_piece(sh: Arc<Shared>, synth: Arc<dyn Synth>, i: usize, text: String, wav: Arc<Vec<u8>>, key: String) {
    let Some(t) = synth.align(&text, &wav) else { return };
    let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
    if st.voice_key != key || !st.chunks.get(i).is_some_and(|c| c.text == text) || !st.wavs.get(i).is_some_and(|w| w.as_ref().is_some_and(|w| Arc::ptr_eq(w, &wav))) {
        return;
    }
    let Some(tl) = t.timeline(&text) else { return };
    let src = t.source;
    st.timed[i] = Some(t);
    if st.cur == i && matches!(st.status, Status::Playing | Status::Paused) {
        if let Some(k) = st.clock.as_mut() {
            k.set_timeline(tl);
            st.source = src;
            log::debug!("reading: part {}, word timing now {}", i + 1, src.label());
        }
    }
    drop(st);
    sh.cv.notify_all();
}

fn play_loop(sh: Arc<Shared>, gen: u64, from: usize, sink: Arc<dyn Sink>) {
    let mut i = from;
    loop {
        // wait for piece i to be ready
        let wav = {
            let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if st.gen != gen {
                    return;
                }
                if i >= st.chunks.len() {
                    if !st.open {
                        st.status = Status::Done;
                        st.started = None;
                        drop(st);
                        sh.cv.notify_all();
                        return;
                    }
                } else if let Some(w) = st.wavs[i].clone() {
                    break w;
                } else if let Some((at, msg)) = st.fail.clone().filter(|(at, _)| *at == i) {
                    let _ = at;
                    st.status = Status::Failed(msg);
                    st.gen += 1;
                    st.started = None;
                    drop(st);
                    sh.cv.notify_all();
                    return;
                }
                st.cur = i.min(st.chunks.len().saturating_sub(1));
                st.started = None;
                st.clock = None;
                if !st.paused {
                    st.status = Status::Buffering;
                }
                sh.cv.notify_all();
                st = sh.cv.wait_timeout(st, POLL * 4).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        // play it; a pause stops the sound and the piece is played again from its start on resume
        'again: loop {
            // The piece is handed to the sink while the lock is held, right after the generation was checked: a seek or stop
            // (which needs the lock to bump the generation) therefore comes strictly before or strictly after this piece
            // reaches the sink. Otherwise an old reading could send its piece to the page just after the new reading sent
            // its own, replacing it, and the new piece would never be reported as ended.
            let seq;
            let played = {
                let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                while st.paused && st.gen == gen {
                    st = sh.cv.wait_timeout(st, POLL * 4).unwrap_or_else(|e| e.into_inner()).0;
                }
                if st.gen != gen {
                    return;
                }
                st.cur = i;
                seq = st.pause_seq;
                st.status = Status::Playing;
                let now = st.now_ms();
                st.started = Some(now);
                let (clock, source) = st.clock_for(i).map_or((None, TimingSource::Estimated), |(k, s)| (Some(k), s));
                st.clock = clock;
                st.source = source;
                if let Some(k) = st.clock.as_mut() {
                    k.start(now);
                    log::debug!("reading: part {} of {}, word timing {}", i + 1, st.chunks.len(), source.label());
                }
                match sink.play(&wav) {
                    Ok(c) => Ok(c),
                    Err(e) => {
                        st.status = Status::Failed(format!("Could not play the sound: {e}"));
                        st.gen += 1;
                        Err(())
                    }
                }
            };
            sh.cv.notify_all();
            let Ok(mut child) = played else { return };
            let mut last_word: Option<(usize, usize)> = None;
            // (milliseconds heard at the last change, when that was)
            let mut follower = wordtime::HeardFollower::new();
            loop {
                let st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                if st.gen != gen {
                    drop(st);
                    child.stop();
                    return;
                }
                if st.paused || st.pause_seq != seq {
                    drop(st);
                    child.stop();
                    continue 'again;
                }
                let st = sh.cv.wait_timeout(st, POLL).unwrap_or_else(|e| e.into_inner()).0;
                drop(st);
                // the speaker says how much was heard: the words follow the sound, so a gap in the sound stops the highlight
                if let Some(p) = child.played_ms() {
                    let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                    let now = st.now_ms();
                    if st.gen == gen && !st.paused {
                        // Settings -> Sound "Highlight timing offset": positive lights the words later
                        let p = (p as f32 - crate::audiosink::highlight_offset_ms() as f32).max(0.0);
                        if let Some(k) = st.clock.as_mut() {
                            follower.poll(k, p, now);
                        }
                    }
                }
                // the voice itself says which word it is on (browser boundary events)
                if let Some(w) = child.spoken_word().filter(|w| Some(*w) != last_word) {
                    last_word = Some(w);
                    let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                    let now = st.now_ms();
                    if st.gen == gen {
                        if let Some(k) = st.clock.as_mut() {
                            let at = wordtime::map_index(w.0, w.1, k.timeline().len());
                            k.boundary(at, now);
                        }
                    }
                }
                if !child.is_playing() {
                    if child.error().is_none() {
                        let st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                        if st.gen == gen {
                            let (audio, at) = (st.secs.get(i).copied().unwrap_or(0.0) * 1000.0, st.clock.as_ref().map(|k| k.position(st.now_ms())));
                            let m = follower.stats();
                            let fit = st.fit_report(i);
                            let path = st.timeline_path(i);
                            if m.polls > 0 {
                                log::info!(
                                    "reading: sentence {} of {}: sound {:.0} ms, heard {:.0} ms, word clock at {} ms, clock behind the sound by at most {:.0} ms and ahead by at most {:.0} ms, longest gap between polls {:.0} ms, highlight offset {} ms, timeline: {}, word timeline stretch factor {}, worst estimated-versus-pinned boundary error {}",
                                    i + 1,
                                    st.chunks.len(),
                                    audio,
                                    m.last_played_ms,
                                    at.map_or("-".to_string(), |a| format!("{a:.0}")),
                                    m.max_behind_ms,
                                    m.max_ahead_ms,
                                    m.max_gap_ms,
                                    crate::audiosink::highlight_offset_ms(),
                                    path,
                                    fit.map_or("-".to_string(), |f| format!("{:.2}", f.stretch)),
                                    fit.map_or("-".to_string(), |f| format!("{:.0} ms ({} pinned)", f.worst_boundary_error_ms, f.pins))
                                );
                            }
                        }
                    }
                    if let Some(e) = child.error() {
                        let mut st = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                        if st.gen == gen {
                            st.status = Status::Failed(format!("Could not read part {} of {} aloud: {e}", i + 1, st.chunks.len()));
                            st.gen += 1;
                            st.started = None;
                        }
                        drop(st);
                        sh.cv.notify_all();
                        return;
                    }
                    break 'again;
                }
            }
        }
        i += 1;
        sh.cv.notify_all();
    }
}
