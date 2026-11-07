//! The mix engine: pure, no threads, no clock. [`Engine::mix_period`] takes one period from every source, converts it to the
//! engine's rate and channel count, applies per-source gain, ducking and master gain with linear ramps across the period, adds
//! the sources in f32 and saturates to 16-bit. The thread that drives it (and paces it) is in `mod.rs`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::resample::Resampler;

/// What a source is, which also fixes its priority: alarm > speech > notification > media > radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Radio,
    Media,
    Notification,
    Speech,
    Alarm,
}

impl SourceKind {
    pub fn priority(self) -> u8 {
        match self {
            SourceKind::Radio => 0,
            SourceKind::Media => 1,
            SourceKind::Notification => 2,
            SourceKind::Speech => 3,
            SourceKind::Alarm => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Radio => "radio",
            SourceKind::Media => "media",
            SourceKind::Notification => "notification",
            SourceKind::Speech => "speech",
            SourceKind::Alarm => "alarm",
        }
    }

    /// The factor sources of a LOWER priority are brought to while a source of this kind is sounding (1.0: it does not duck).
    pub fn duck_to(self) -> f32 {
        match self {
            SourceKind::Alarm => 0.10,        // -20 dB
            SourceKind::Speech => 0.25,       // -12 dB
            SourceKind::Notification => 0.40, // -8 dB
            SourceKind::Media | SourceKind::Radio => 1.0,
        }
    }
}

/// Engine settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineCfg {
    /// Output rate, Hz.
    pub rate: u32,
    /// Output channels (the sink's).
    pub channels: usize,
    /// Frames in a period (960 = 20 ms at 48 kHz).
    pub period_frames: usize,
    /// A notification or alarm starts sounding once it has this much queued (or has ended); also where it resumes after running dry.
    pub prebuffer_ms: u32,
    /// Radio and media: queued before the source starts mixing (network and decoder jitter budget) ...
    pub media_prefill_ms: u32,
    /// ... and queued again before it resumes after it ran dry (it is silent, with a short fade, until then).
    pub media_resume_ms: u32,
    /// Speech: the same two numbers.
    pub speech_prefill_ms: u32,
    pub speech_resume_ms: u32,
    /// How long a source keeps ducking others after its last audible sample (bridges the gaps between sentences).
    pub hang_ms: u32,
    /// Ducking goes down in this time ...
    pub attack_ms: u32,
    /// ... and comes back up in this.
    pub release_ms: u32,
}

impl Default for EngineCfg {
    fn default() -> Self {
        EngineCfg { rate: 48_000, channels: 2, period_frames: 960, prebuffer_ms: 40, media_prefill_ms: 800, media_resume_ms: 400, speech_prefill_ms: 250, speech_resume_ms: 250, hang_ms: 300, attack_ms: 30, release_ms: 400 }
    }
}

impl EngineCfg {
    pub fn period(&self) -> Duration {
        Duration::from_micros(self.period_frames as u64 * 1_000_000 / self.rate.max(1) as u64)
    }
    /// Every source starts at once (tests, and callers that do their own buffering).
    pub fn no_prefill(mut self) -> EngineCfg {
        self.prebuffer_ms = 0;
        self.media_prefill_ms = 0;
        self.media_resume_ms = 0;
        self.speech_prefill_ms = 0;
        self.speech_resume_ms = 0;
        self
    }
    /// (first start, restart after running dry) in ms for a kind of source.
    pub fn prefill_ms(&self, kind: SourceKind) -> (u32, u32) {
        match kind {
            SourceKind::Radio | SourceKind::Media => (self.media_prefill_ms, self.media_resume_ms),
            SourceKind::Speech => (self.speech_prefill_ms, self.speech_resume_ms),
            SourceKind::Notification | SourceKind::Alarm => (self.prebuffer_ms, self.prebuffer_ms),
        }
    }
    fn period_ms(&self) -> f32 {
        self.period_frames as f32 * 1000.0 / self.rate.max(1) as f32
    }
}

/// Below this a source counts as silent (about -80 dBFS).
pub const AUDIBLE: f32 = 1e-4;
/// Milliseconds of audio a notification or alarm may queue before a write waits.
pub const RING_MS: u32 = 500;

/// How much a source of this kind may queue before a write waits (a live stream needs room for its jitter buffer plus slack).
pub fn ring_ms(kind: SourceKind) -> u32 {
    match kind {
        SourceKind::Radio | SourceKind::Media => 6_000,
        SourceKind::Speech => 2_000,
        SourceKind::Notification | SourceKind::Alarm => RING_MS,
    }
}

/// A source that runs dry fades out, and fades in again when it resumes, over this many frames (about 5 ms at 48 kHz).
const FADE_FRAMES: usize = 240;

/// Microseconds since the first call (a clock all mixer parts share).
pub(crate) fn now_us() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_micros() as u64
}

pub(crate) struct Wake {
    /// When the period being mixed will have been played, on [`now_us`] (0: the card is closed).
    pub end_us: AtomicU64,
    pub sleeping: AtomicBool,
    pub m: Mutex<()>,
    pub cv: Condvar,
}

impl Wake {
    pub fn new() -> Arc<Wake> {
        Arc::new(Wake { end_us: AtomicU64::new(0), sleeping: AtomicBool::new(false), m: Mutex::new(()), cv: Condvar::new() })
    }
    pub fn notify(&self) {
        if self.sleeping.load(Ordering::SeqCst) {
            self.cv.notify_all();
        }
    }
}

struct Ring {
    data: VecDeque<f32>,
    cap: usize,
}

/// What a [`SourceWriter`] and the engine share.
pub(crate) struct SourceShared {
    pub id: u64,
    pub kind: SourceKind,
    pub label: String,
    pub rate: u32,
    pub channels: usize,
    pub exclusive: bool,
    ring: Mutex<Ring>,
    space: Condvar,
    gain: AtomicU32,
    /// The person's level for this kind of sound (Settings / the speaker dropdown), 0.0 to 1.0; multiplies `gain`, which the player
    /// itself sets for fades and its own volume.
    level: AtomicU32,
    paused: AtomicBool,
    /// The writer is finished (or gone): play what is queued, then the source ends.
    closed: AtomicBool,
    /// The engine dropped the source (ended, or evicted for a higher-priority one).
    gone: AtomicBool,
    evicted: AtomicBool,
    epoch: AtomicU64,
    underruns: AtomicU64,
    /// Microseconds of this source that were mixed (at the output rate, so a stall of the source does not count).
    supplied_us: AtomicU64,
    /// When the last audio of this source reaches the speaker, on [`now_us`].
    heard_end_us: AtomicU64,
    pub frames_in: AtomicU64,
    pub wake: Arc<Wake>,
}

impl SourceShared {
    pub(crate) fn new(id: u64, kind: SourceKind, label: &str, rate: u32, channels: usize, exclusive: bool, wake: Arc<Wake>) -> Arc<SourceShared> {
        let cap = (rate as usize * channels * ring_ms(kind) as usize / 1000).max(channels * 64);
        Arc::new(SourceShared {
            id,
            kind,
            label: label.to_string(),
            rate,
            channels,
            exclusive,
            ring: Mutex::new(Ring { data: VecDeque::new(), cap }),
            space: Condvar::new(),
            gain: AtomicU32::new(1.0f32.to_bits()),
            level: AtomicU32::new(1.0f32.to_bits()),
            paused: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            gone: AtomicBool::new(false),
            evicted: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            supplied_us: AtomicU64::new(0),
            heard_end_us: AtomicU64::new(0),
            frames_in: AtomicU64::new(0),
            wake,
        })
    }

    fn queued_samples(&self) -> usize {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).data.len()
    }

    pub(crate) fn buffered_ms(&self) -> u32 {
        (self.queued_samples() / self.channels * 1000 / self.rate.max(1) as usize) as u32
    }

    /// How much of this source has been heard, ms: what was mixed minus what is still on its way to the speaker (the mixer's lead
    /// and the pipe). It stops when the source runs dry, and is 0 until the first sound is audible.
    pub(crate) fn played_ms(&self) -> u64 {
        let in_flight = self.heard_end_us.load(Ordering::SeqCst).saturating_sub(now_us());
        self.supplied_us.load(Ordering::SeqCst).saturating_sub(in_flight) / 1000
    }

    pub(crate) fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::SeqCst))
    }

    /// The gain the mix applies: the player's own gain times the person's level for this source.
    pub(crate) fn mix_gain(&self) -> f32 {
        self.gain() * self.level()
    }

    pub(crate) fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::SeqCst))
    }

    pub(crate) fn set_level(&self, l: f32) {
        self.level.store(l.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }

    pub(crate) fn mark_gone(&self, evicted: bool) {
        self.evicted.store(evicted, Ordering::SeqCst);
        self.gone.store(true, Ordering::SeqCst);
        self.space.notify_all();
    }

    pub(crate) fn is_gone(&self) -> bool {
        self.gone.load(Ordering::SeqCst)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Take up to `max` samples (a multiple of the channel count) from the ring.
    fn take(&self, max: usize, out: &mut Vec<f32>) -> usize {
        let mut r = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let n = max.min(r.data.len()) / self.channels * self.channels;
        out.extend(r.data.drain(..n));
        drop(r);
        if n > 0 {
            self.space.notify_all();
        }
        n
    }
}

/// The handle a program writes its audio to: interleaved samples at the rate and channel count it opened with.
pub struct SourceWriter {
    pub(crate) s: Arc<SourceShared>,
}

impl SourceWriter {
    pub fn id(&self) -> u64 {
        self.s.id
    }
    /// Another handle to the same source (a second thread feeds it while the first controls it). Dropping either finishes it.
    pub fn clone_handle(&self) -> SourceWriter {
        SourceWriter { s: self.s.clone() }
    }
    pub fn kind(&self) -> SourceKind {
        self.s.kind
    }

    /// Queue as many samples as fit without waiting; returns how many were taken (0 when the source ended or was evicted).
    pub fn write_f32(&self, pcm: &[f32]) -> usize {
        if self.s.is_gone() || self.s.is_closed() {
            return 0;
        }
        // the lock is held for at most about 90 ms of audio at a time, so the mixer thread never waits long for it
        let step = 8192 / self.s.channels * self.s.channels;
        let mut done = 0;
        while done < pcm.len() {
            let mut r = self.s.ring.lock().unwrap_or_else(|e| e.into_inner());
            let n = (r.cap - r.data.len()).min(pcm.len() - done).min(step) / self.s.channels * self.s.channels;
            if n == 0 {
                break;
            }
            r.data.extend(pcm[done..done + n].iter().copied());
            drop(r);
            done += n;
        }
        let n = done;
        self.s.frames_in.fetch_add((n / self.s.channels) as u64, Ordering::Relaxed);
        self.s.wake.notify();
        n
    }

    pub fn write_i16(&self, pcm: &[i16]) -> usize {
        let f: Vec<f32> = pcm.iter().map(|&x| x as f32 / 32768.0).collect();
        self.write_f32(&f)
    }

    /// Queue everything, waiting for room (this is what paces a decoder to the speaker). `Err` when the source was evicted or the
    /// time ran out.
    pub fn write_i16_wait(&self, pcm: &[i16], max: Duration) -> Result<(), String> {
        let f: Vec<f32> = pcm.iter().map(|&x| x as f32 / 32768.0).collect();
        let end = std::time::Instant::now() + max;
        let mut done = 0;
        while done < f.len() {
            if self.s.is_gone() {
                return Err(if self.s.evicted.load(Ordering::SeqCst) { "a more important sound took this place".into() } else { "the mixer stopped".into() });
            }
            done += self.write_f32(&f[done..]);
            if done >= f.len() {
                break;
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("the mixer did not take the audio in time".into());
            }
            let g = self.s.ring.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self.s.space.wait_timeout(g, left.min(Duration::from_millis(50)));
        }
        Ok(())
    }

    /// The source's own volume, 0.0 to 4.0 (ramped over a period, so no click).
    pub fn set_gain(&self, g: f32) {
        self.s.gain.store(g.clamp(0.0, 4.0).to_bits(), Ordering::SeqCst);
    }
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.s.gain.load(Ordering::SeqCst))
    }
    /// Pause: nothing is taken from the queue and the source does not duck others.
    pub fn set_paused(&self, p: bool) {
        self.s.paused.store(p, Ordering::SeqCst);
    }
    /// Throw away what is queued (stop, seek).
    pub fn flush(&self) {
        self.s.epoch.fetch_add(1, Ordering::SeqCst);
        self.s.ring.lock().unwrap_or_else(|e| e.into_inner()).data.clear();
        self.s.space.notify_all();
    }
    /// No more audio will come: what is queued plays, then the source ends. Also done when the writer is dropped.
    pub fn finish(&self) {
        self.s.closed.store(true, Ordering::SeqCst);
        self.s.wake.notify();
    }
    /// Milliseconds of this source that have been heard (not just handed over or mixed): the lead of the mixer and the pipe
    /// are subtracted, and it stands still while the source is starved. Drives word highlighting.
    pub fn played_ms(&self) -> u64 {
        self.s.played_ms()
    }
    /// Milliseconds queued.
    pub fn buffered_ms(&self) -> u32 {
        (self.s.queued_samples() / self.s.channels * 1000 / self.s.rate.max(1) as usize) as u32
    }
    /// Whether the engine has played everything and dropped the source.
    pub fn is_done(&self) -> bool {
        self.s.is_gone()
    }
    /// Finish and wait until the last sample was mixed (at most `max`). True: done.
    pub fn finish_and_wait(&self, max: Duration) -> bool {
        self.finish();
        let end = std::time::Instant::now() + max;
        while !self.s.is_gone() {
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
    /// Feed the source from a reader that yields interleaved little-endian 16-bit samples at the rate and channel count the
    /// source was opened with (a decoder's standard output: the in-house MP3 decoder, or the optional ffmpeg helper). A thread
    /// reads, waits for room (so the decoder is paced by the speaker) and finishes the source at end of data. Returns the
    /// thread; it ends with the bytes read, or the reason it stopped.
    pub fn feed_reader<R: std::io::Read + Send + 'static>(self, mut r: R) -> std::thread::JoinHandle<Result<usize, String>> {
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            let (mut total, mut odd): (usize, Option<u8>) = (0, None);
            loop {
                let n = r.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    self.finish();
                    return Ok(total);
                }
                total += n;
                let mut bytes: Vec<u8> = odd.take().into_iter().chain(buf[..n].iter().copied()).collect();
                if bytes.len() % 2 == 1 {
                    odd = bytes.pop();
                }
                let pcm: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                self.write_i16_wait(&pcm, Duration::from_secs(30))?;
            }
        })
    }

    pub fn underruns(&self) -> u64 {
        self.s.underruns.load(Ordering::Relaxed)
    }
}

impl Drop for SourceWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Convert channel layouts: mono is copied to all, more than two channels fold down to stereo (even channels left, odd right,
/// averaged), stereo fans out the way `softvol::fan_route` does (left on the even channels, right on the odd).
pub fn map_channels(inp: &[f32], in_ch: usize, out_ch: usize, out: &mut Vec<f32>) {
    if in_ch == out_ch {
        out.extend_from_slice(inp);
        return;
    }
    for f in inp.chunks_exact(in_ch) {
        let (l, r) = if in_ch == 1 {
            (f[0], f[0])
        } else {
            let (mut ls, mut ln, mut rs, mut rn) = (0.0, 0.0, 0.0, 0.0);
            for (i, v) in f.iter().enumerate() {
                if i % 2 == 0 {
                    ls += v;
                    ln += 1.0;
                } else {
                    rs += v;
                    rn += 1.0;
                }
            }
            (ls / ln, if rn > 0.0 { rs / rn } else { ls / ln })
        };
        match out_ch {
            1 => out.push(if in_ch == 1 { f[0] } else { 0.5 * (l + r) }),
            n => (0..n).for_each(|c| out.push(if c % 2 == 0 { l } else { r })),
        }
    }
}

struct Slot {
    s: Arc<SourceShared>,
    rs: Option<Resampler>,
    /// Converted frames (engine rate and channels) not yet mixed.
    ready: Vec<f32>,
    scratch: Vec<f32>,
    started: bool,
    gain_prev: f32,
    duck: f32,
    duck_prev: f32,
    hang_ms: f32,
    epoch: u64,
    underruns: u64,
    /// The source ran dry and waits for its resume level; it fades in when it sounds again.
    resuming: bool,
    fade_in: bool,
}

/// One period's result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeriodInfo {
    /// Sources that supplied at least one frame this period.
    pub supplying: usize,
    /// Sources registered after the period.
    pub sources: usize,
    pub clipped: u64,
    pub underruns: u64,
    /// Some source is ducking the others.
    pub ducking: bool,
    /// The loudest output sample, 0.0 to 1.0.
    pub peak: f32,
}

pub struct Engine {
    pub cfg: EngineCfg,
    slots: Vec<Slot>,
    master_prev: f32,
    acc: Vec<f32>,
    pub underruns: u64,
    /// Underruns by source kind (index = `SourceKind::priority()`).
    pub starved: [u64; 5],
    pub clipped: u64,
    pub periods: u64,
}

impl Engine {
    pub fn new(cfg: EngineCfg) -> Engine {
        Engine { cfg, slots: Vec::new(), master_prev: 1.0, acc: Vec::new(), underruns: 0, starved: [0; 5], clipped: 0, periods: 0 }
    }

    pub(crate) fn add(&mut self, s: Arc<SourceShared>) {
        let rs = (s.rate != self.cfg.rate).then(|| Resampler::new(s.rate, self.cfg.rate, s.channels));
        let epoch = s.epoch.load(Ordering::SeqCst);
        self.slots.push(Slot { s, rs, ready: Vec::new(), scratch: Vec::new(), started: false, gain_prev: 1.0, duck: 1.0, duck_prev: 1.0, hang_ms: 0.0, epoch, underruns: 0, resuming: false, fade_in: false });
        if let Some(sl) = self.slots.last_mut() {
            sl.gain_prev = sl.s.mix_gain();
        }
    }

    pub fn source_count(&self) -> usize {
        self.slots.len()
    }

    /// Per source: id, kind, label, current duck factor.
    pub fn source_info(&self) -> Vec<(u64, SourceKind, String, f32, u64)> {
        self.slots.iter().map(|s| (s.s.id, s.s.kind, s.s.label.clone(), s.duck, s.underruns)).collect()
    }

    /// Mix one period into `out` (interleaved i16, `period_frames * channels` samples). `master` is the factor for everything.
    pub fn mix_period(&mut self, master: f32, out: &mut Vec<i16>) -> PeriodInfo {
        let cfg = self.cfg;
        let (n, ch) = (cfg.period_frames, cfg.channels);
        self.periods += 1;
        self.acc.clear();
        self.acc.resize(n * ch, 0.0);
        let mut info = PeriodInfo::default();
        let pre_frames = |s: &SourceShared, resuming: bool| {
            let (first, again) = cfg.prefill_ms(s.kind);
            (s.rate as usize * (if resuming { again } else { first }) as usize / 1000).max(1) * s.channels
        };
        // 1. pull
        let mut supplied = vec![0usize; self.slots.len()];
        let mut audible = vec![false; self.slots.len()];
        for (i, sl) in self.slots.iter_mut().enumerate() {
            let ep = sl.s.epoch.load(Ordering::SeqCst);
            if ep != sl.epoch {
                sl.epoch = ep;
                sl.ready.clear();
                sl.started = false;
                sl.resuming = false;
                if sl.rs.is_some() {
                    sl.rs = Some(Resampler::new(sl.s.rate, cfg.rate, sl.s.channels));
                }
            }
            if sl.s.paused.load(Ordering::SeqCst) {
                continue;
            }
            if !sl.started {
                if sl.s.queued_samples() >= pre_frames(&sl.s, sl.resuming) || (sl.s.is_closed() && sl.s.queued_samples() > 0) || !sl.ready.is_empty() {
                    sl.started = true;
                    sl.fade_in = std::mem::take(&mut sl.resuming);
                } else {
                    continue;
                }
            }
            while sl.ready.len() < n * ch {
                let need_out = n - sl.ready.len() / ch;
                let want = ((need_out as u64 * sl.s.rate as u64 / cfg.rate as u64) as usize + 4) * sl.s.channels;
                sl.scratch.clear();
                if sl.s.take(want, &mut sl.scratch) == 0 {
                    break;
                }
                match sl.rs.as_mut() {
                    Some(rs) => {
                        let mut conv = Vec::with_capacity(need_out * sl.s.channels);
                        rs.process(&sl.scratch, &mut conv);
                        map_channels(&conv, sl.s.channels, ch, &mut sl.ready);
                    }
                    None => map_channels(&sl.scratch, sl.s.channels, ch, &mut sl.ready),
                }
            }
            let got = (sl.ready.len() / ch).min(n);
            supplied[i] = got;
            if got > 0 {
                sl.s.heard_end_us.store(sl.s.wake.end_us.load(Ordering::SeqCst), Ordering::SeqCst);
            }
            sl.s.supplied_us.fetch_add(got as u64 * 1_000_000 / cfg.rate.max(1) as u64, Ordering::SeqCst);
            if got > 0 && sl.fade_in {
                sl.fade_in = false;
                let k = got.min(FADE_FRAMES);
                for f in 0..k {
                    let g = (f as f32 + 1.0) / k as f32;
                    sl.ready[f * ch..(f + 1) * ch].iter_mut().for_each(|v| *v *= g);
                }
            }
            if got > 0 {
                info.supplying += 1;
                let peak = sl.ready[..got * ch].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                audible[i] = peak > AUDIBLE;
            }
            if got < n && !sl.s.is_closed() && sl.started {
                // a source that stopped mid-stream: late data, counted once per period
                sl.underruns += 1;
                sl.s.underruns.fetch_add(1, Ordering::Relaxed);
                info.underruns += 1;
                self.starved[sl.s.kind.priority() as usize] += 1;
                sl.started = false; // refill to the resume level before it sounds again
                sl.resuming = true;
                // what is left of the stream fades out instead of stopping with a click
                let k = got.min(FADE_FRAMES);
                for f in 0..k {
                    let g = 1.0 - (f as f32 + 1.0) / k as f32;
                    let at = (got - k + f) * ch;
                    sl.ready[at..at + ch].iter_mut().for_each(|v| *v *= g);
                }
            }
        }
        // 2. who is sounding (with a hang so speech does not pump between sentences)
        let ms = cfg.period_ms();
        for (sl, a) in self.slots.iter_mut().zip(&audible) {
            if *a {
                sl.hang_ms = cfg.hang_ms as f32;
            } else if sl.hang_ms > 0.0 {
                sl.hang_ms = (sl.hang_ms - ms).max(0.0);
            }
        }
        let sounding: Vec<(u8, f32)> = self.slots.iter().filter(|s| s.hang_ms > 0.0 && !s.s.paused.load(Ordering::SeqCst)).map(|s| (s.s.kind.priority(), s.s.kind.duck_to())).collect();
        // 3. gain, ducking, add
        let step_down = ms / cfg.attack_ms.max(1) as f32;
        let step_up = ms / cfg.release_ms.max(1) as f32;
        for (i, sl) in self.slots.iter_mut().enumerate() {
            let prio = sl.s.kind.priority();
            let target = sounding.iter().filter(|(p, _)| *p > prio).map(|(_, d)| *d).fold(1.0f32, f32::min);
            sl.duck_prev = sl.duck;
            sl.duck = if target < sl.duck { (sl.duck - step_down).max(target) } else { (sl.duck + step_up).min(target) };
            if sl.duck < 0.999 {
                info.ducking = true;
            }
            let g_now = sl.s.mix_gain();
            let (g0, g1) = (sl.gain_prev * sl.duck_prev, g_now * sl.duck);
            sl.gain_prev = g_now;
            let got = supplied[i];
            for f in 0..got {
                let g = if g0 == g1 { g1 } else { g0 + (g1 - g0) * (f as f32 + 1.0) / n as f32 };
                for c in 0..ch {
                    self.acc[f * ch + c] += sl.ready[f * ch + c] * g;
                }
            }
            sl.ready.drain(..got * ch);
        }
        // 4. master, saturate
        let (m0, m1) = (self.master_prev, master);
        self.master_prev = master;
        out.clear();
        out.reserve(n * ch);
        let mut peak = 0.0f32;
        for f in 0..n {
            let m = if m0 == m1 { m1 } else { m0 + (m1 - m0) * (f as f32 + 1.0) / n as f32 };
            for c in 0..ch {
                let v = self.acc[f * ch + c] * m;
                if v > 1.0 || v < -1.0 {
                    info.clipped += 1;
                }
                let v = v.clamp(-1.0, 1.0);
                peak = peak.max(v.abs());
                out.push((v * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
            }
        }
        info.peak = peak;
        self.clipped += info.clipped;
        self.underruns += info.underruns;
        // 5. forget the ended
        // (a short tone keeps ducking the others for its hang after its last sample, so the slot stays until that is over)
        self.slots.retain(|sl| {
            let over = sl.s.is_gone() || (sl.s.is_closed() && sl.ready.is_empty() && sl.s.queued_samples() == 0);
            if over {
                sl.s.mark_gone(false);
            }
            !over || sl.hang_ms > 0.0
        });
        info.sources = self.slots.len();
        info
    }

    /// Drop a source at once (eviction).
    pub(crate) fn evict(&mut self, id: u64) {
        if let Some(p) = self.slots.iter().position(|s| s.s.id == id) {
            self.slots.remove(p).s.mark_gone(true);
        }
    }

    /// Drop every source (shutdown).
    pub(crate) fn clear(&mut self) {
        for sl in self.slots.drain(..) {
            sl.s.mark_gone(false);
        }
    }
}

impl Engine {
    /// Some source has audio waiting (so the sink must be open before the next period is mixed).
    pub fn wants_sink(&self) -> bool {
        let cfg = self.cfg;
        self.slots.iter().any(|s| {
            if s.s.paused.load(Ordering::SeqCst) {
                return false;
            }
            let q = s.s.queued_samples();
            if !s.ready.is_empty() || (s.started && q > 0) {
                return true;
            }
            // a source still filling its jitter buffer does not open the card (it would be reopened and closed again and again)
            let (first, again) = cfg.prefill_ms(s.s.kind);
            let need = (s.s.rate as usize * (if s.resuming { again } else { first }) as usize / 1000).max(1) * s.s.channels;
            q >= need || (s.s.is_closed() && q > 0)
        })
    }

    /// The least queued time (ms) among the live streams that are sounding: how close the nearest underrun is. 0 with none.
    pub fn buffered_ms(&self) -> u32 {
        self.slots.iter().filter(|s| s.started && !s.s.is_closed()).map(|s| s.s.buffered_ms()).min().unwrap_or(0)
    }
}
