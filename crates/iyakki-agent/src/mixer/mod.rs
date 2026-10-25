//! The IYAKKI audio mixer: several sound sources share one sound card with low CPU, where ALSA's `dmix` spins a core (the T2
//! MacBook's `aaudio`, see `docs/development/AUDIO-MIXER.md`).
//!
//! * [`engine`]: the pure mix engine (sources, ring buffers, resampling, channel mapping, ducking, gain ramps, saturation).
//! * [`resample`]: the in-house windowed-sinc converter.
//! * [`sink`]: where the result goes (`aplay` on IYAKKI's output, or a fake in tests).
//! * this file: the [`Mixer`] service (one thread, paced at 20 ms periods, stops writing when idle), its statistics, the
//!   process-wide instance behind the `sound.mixer` setting and a helper that plays a WAV through it.
//!
//! Off by default. When it is off [`Mixer::open_source`] answers [`MixError::Disabled`] and every caller keeps its old path (one
//! program at a time, `Busy` for the SDK).

pub mod engine;
pub mod resample;
pub mod sink;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use engine::{Engine, EngineCfg, PeriodInfo, SourceKind, SourceWriter};
pub use sink::{AplaySink, FakeSink, Sink};

use engine::{SourceShared, Wake};

/// The most sources at once.
pub const MAX_SOURCES: usize = 16;
/// After the sink could not be opened, new sources are refused (`Busy`) for this long.
pub const OPEN_RETRY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixError {
    /// The mixer is switched off (Settings -> Sound): the caller keeps its one-program-at-a-time path.
    Disabled,
    /// The card could not be opened (another program holds it) or an exclusive source holds the mixer, or it is full of
    /// more important sources. `holder` is a label for the message.
    Busy { holder: String },
    Invalid(String),
    /// The mixer was shut down.
    Closed,
}

impl std::fmt::Display for MixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixError::Disabled => write!(f, "the sound mixer is off (Settings -> Sound)"),
            MixError::Busy { holder } => write!(f, "the speaker is in use by {holder}"),
            MixError::Invalid(e) => write!(f, "{e}"),
            MixError::Closed => write!(f, "the sound mixer stopped"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerCfg {
    pub engine: EngineCfg,
    /// No audio from any source for this long: the sink is closed so the card can sleep.
    pub idle_ms: u32,
    /// Periods written ahead of the clock (absorbs scheduling jitter; adds this much latency to ducking).
    pub lead_periods: u32,
    /// Sleep to the period clock. Tests turn this off to mix faster than real time.
    pub realtime: bool,
}

impl Default for MixerCfg {
    fn default() -> Self {
        MixerCfg { engine: EngineCfg::default(), idle_ms: 1500, lead_periods: 2, realtime: true }
    }
}

/// What the sound doctor and the Devices popover read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// The thread has sources to serve.
    pub running: bool,
    pub sink_open: bool,
    pub sink: String,
    pub sources: usize,
    pub periods: u64,
    pub underruns: u64,
    pub clipped: u64,
    /// Time spent mixing, as a share of the time the periods cover (percent of one core, mixing only).
    pub cpu_percent: f32,
    pub mix_us_last: u32,
    pub ducking: bool,
    pub peak: f32,
    pub sink_opens: u64,
    pub last_error: Option<String>,
    mix_ns: u64,
}

impl Stats {
    /// One line for the doctor / Devices popover.
    pub fn line(&self) -> String {
        let state = if self.sink_open { "playing" } else if self.running { "waiting" } else { "idle" };
        let mut s = format!("mixer {state}: {} source(s), {:.1}% of a core, {} underrun(s)", self.sources, self.cpu_percent, self.underruns);
        if self.ducking {
            s.push_str(", ducking");
        }
        if let Some(e) = &self.last_error {
            s.push_str(&format!("; last problem: {e}"));
        }
        s
    }
}

/// A source as the UI lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    pub id: u64,
    pub kind: SourceKind,
    pub label: String,
    pub rate: u32,
    pub channels: usize,
    pub buffered_ms: u32,
    pub gain: f32,
}

type MasterFn = Arc<dyn Fn() -> f32 + Send + Sync>;

struct Inner {
    cfg: MixerCfg,
    wake: Arc<Wake>,
    pending: Mutex<Vec<Arc<SourceShared>>>,
    registry: Mutex<Vec<Arc<SourceShared>>>,
    evictions: Mutex<Vec<u64>>,
    next_id: AtomicU64,
    shutdown: AtomicBool,
    enabled: AtomicBool,
    stats: Mutex<Stats>,
    master: Mutex<Option<MasterFn>>,
    master_fixed: AtomicU32,
    open_error: Mutex<Option<(Instant, String)>>,
}

pub struct Mixer {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Mixer {
    /// Start the mixer thread (it sleeps until the first source arrives).
    pub fn start(cfg: MixerCfg, sink: Box<dyn Sink>) -> Mixer {
        let inner = Arc::new(Inner {
            cfg,
            wake: Wake::new(),
            pending: Mutex::new(Vec::new()),
            registry: Mutex::new(Vec::new()),
            evictions: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
            enabled: AtomicBool::new(true),
            stats: Mutex::new(Stats::default()),
            master: Mutex::new(None),
            master_fixed: AtomicU32::new(1.0f32.to_bits()),
            open_error: Mutex::new(None),
        });
        let i2 = inner.clone();
        let handle = std::thread::Builder::new().name("iyakki-mixer".into()).spawn(move || run(i2, sink)).ok();
        Mixer { inner, thread: Mutex::new(handle) }
    }

    pub fn set_enabled(&self, on: bool) {
        self.inner.enabled.store(on, Ordering::SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::SeqCst)
    }

    /// The master volume as a fixed factor (0.0 to 1.0).
    pub fn set_master(&self, g: f32) {
        self.inner.master_fixed.store(g.clamp(0.0, 4.0).to_bits(), Ordering::SeqCst);
    }

    /// The master volume as a function read every period (the global mixer follows Settings -> Sound with it).
    pub fn set_master_fn(&self, f: impl Fn() -> f32 + Send + Sync + 'static) {
        *self.inner.master.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    pub fn open_source(&self, kind: SourceKind, rate: u32, channels: usize) -> Result<SourceWriter, MixError> {
        self.open_source_with(kind, rate, channels, kind.label(), false)
    }

    /// `label` names the holder in messages; `exclusive` refuses every other source except alarms while this one is open (and,
    /// at unity gain, same rate and channels, passes its samples through unchanged).
    pub fn open_source_with(&self, kind: SourceKind, rate: u32, channels: usize, label: &str, exclusive: bool) -> Result<SourceWriter, MixError> {
        let i = &self.inner;
        if i.shutdown.load(Ordering::SeqCst) {
            return Err(MixError::Closed);
        }
        if !self.enabled() {
            return Err(MixError::Disabled);
        }
        if !(8_000..=192_000).contains(&rate) || !(1..=8).contains(&channels) {
            return Err(MixError::Invalid(format!("{rate} Hz with {channels} channel(s) is not a supported sound format")));
        }
        if let Some((at, why)) = i.open_error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            if at.elapsed() < OPEN_RETRY {
                return Err(MixError::Busy { holder: why });
            }
        }
        let mut reg = i.registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.retain(|s| !s.is_gone());
        if let Some(x) = reg.iter().find(|s| s.exclusive) {
            if kind != SourceKind::Alarm {
                return Err(MixError::Busy { holder: x.label.clone() });
            }
        }
        if exclusive && !reg.is_empty() {
            return Err(MixError::Busy { holder: reg[0].label.clone() });
        }
        if reg.len() >= MAX_SOURCES {
            // the least important (oldest first) gives way to a more important one
            let victim = reg.iter().filter(|s| s.kind.priority() < kind.priority()).min_by_key(|s| (s.kind.priority(), s.id)).map(|s| (s.id, s.clone()));
            match victim {
                Some((id, s)) => {
                    s.mark_gone(true);
                    i.evictions.lock().unwrap_or_else(|e| e.into_inner()).push(id);
                    reg.retain(|x| x.id != id);
                }
                None => return Err(MixError::Busy { holder: "too many sounds at once".into() }),
            }
        }
        let id = i.next_id.fetch_add(1, Ordering::SeqCst);
        let shared = SourceShared::new(id, kind, label, rate, channels, exclusive, i.wake.clone());
        reg.push(shared.clone());
        drop(reg);
        i.pending.lock().unwrap_or_else(|e| e.into_inner()).push(shared.clone());
        i.wake.notify();
        Ok(SourceWriter { s: shared })
    }

    pub fn stats(&self) -> Stats {
        self.inner.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn sources(&self) -> Vec<SourceInfo> {
        let reg = self.inner.registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.iter()
            .filter(|s| !s.is_gone())
            .map(|s| SourceInfo {
                id: s.id,
                kind: s.kind,
                label: s.label.clone(),
                rate: s.rate,
                channels: s.channels,
                buffered_ms: s.buffered_ms(),
                gain: s.gain(),
            })
            .collect()
    }

    /// Play a WAV through the mixer and return when it has been mixed (or why it could not be).
    pub fn play_wav(&self, kind: SourceKind, wav: &[u8], label: &str) -> Result<(), MixError> {
        let w16 = crate::audioout::to_pcm16_wav(wav);
        let w = crate::voice::parse_wav(w16.as_deref().unwrap_or(wav)).map_err(MixError::Invalid)?;
        let src = self.open_source_with(kind, w.rate, w.channels as usize, label, false)?;
        let secs = w.samples.len() as f64 / (w.rate as f64 * w.channels as f64);
        src.write_i16_wait(&w.samples, Duration::from_secs_f64(secs + 3.0)).map_err(|e| MixError::Busy { holder: e })?;
        src.finish_and_wait(Duration::from_secs_f64(secs + 3.0));
        Ok(())
    }

    /// Stop the thread and let go of the device. Sources end.
    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.wake.cv.notify_all();
        let h = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = h {
            let _ = h.join();
        }
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn set_error(inner: &Inner, e: Option<String>) {
    *inner.open_error.lock().unwrap_or_else(|x| x.into_inner()) = e.clone().map(|e| (Instant::now(), e.clone()));
    inner.stats.lock().unwrap_or_else(|x| x.into_inner()).last_error = e;
}

fn run(inner: Arc<Inner>, mut sink: Box<dyn Sink>) {
    let cfg = inner.cfg;
    let mut engine = Engine::new(cfg.engine);
    let period = cfg.engine.period();
    let period_ms = period.as_secs_f32() * 1000.0;
    let mut out: Vec<i16> = Vec::with_capacity(cfg.engine.period_frames * cfg.engine.channels);
    let mut next = Instant::now();
    let mut idle_ms = 0.0f32;
    let mut retry_at: Option<Instant> = None;
    let nap = |inner: &Inner, d: Duration| {
        inner.wake.sleeping.store(true, Ordering::SeqCst);
        let g = inner.wake.m.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.shutdown.load(Ordering::SeqCst) && inner.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            let _ = inner.wake.cv.wait_timeout(g, d);
        }
        inner.wake.sleeping.store(false, Ordering::SeqCst);
    };
    while !inner.shutdown.load(Ordering::SeqCst) {
        for s in inner.pending.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            engine.add(s);
        }
        for id in inner.evictions.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            engine.evict(id);
        }
        if engine.source_count() == 0 {
            if sink.is_open() {
                sink.close();
            }
            {
                let mut st = inner.stats.lock().unwrap_or_else(|e| e.into_inner());
                st.running = false;
                st.sink_open = false;
                st.sources = 0;
                st.ducking = false;
            }
            nap(&inner, Duration::from_millis(250));
            next = Instant::now();
            idle_ms = 0.0;
            continue;
        }
        // the card must be open before audio is taken from the sources
        if !sink.is_open() && engine.wants_sink() {
            if retry_at.is_none_or(|t| Instant::now() >= t) {
                match sink.open(cfg.engine.rate, cfg.engine.channels) {
                    Ok(()) => {
                        retry_at = None;
                        next = Instant::now();
                        idle_ms = 0.0;
                        set_error(&inner, None);
                        inner.stats.lock().unwrap_or_else(|e| e.into_inner()).sink_opens += 1;
                    }
                    Err(e) => {
                        log::info!("mixer: cannot open the sound device: {e}");
                        retry_at = Some(Instant::now() + OPEN_RETRY);
                        set_error(&inner, Some(e));
                    }
                }
            }
            if !sink.is_open() {
                nap(&inner, Duration::from_millis(100));
                continue;
            }
        }
        let master = inner.master.lock().unwrap_or_else(|e| e.into_inner()).clone().map_or_else(|| f32::from_bits(inner.master_fixed.load(Ordering::SeqCst)), |f| f());
        let t0 = Instant::now();
        let info = engine.mix_period(master, &mut out);
        let spent = t0.elapsed();
        if info.supplying > 0 {
            idle_ms = 0.0;
        } else {
            idle_ms += period_ms;
        }
        {
            let mut st = inner.stats.lock().unwrap_or_else(|e| e.into_inner());
            st.running = true;
            st.sources = info.sources;
            st.periods = engine.periods;
            st.underruns = engine.underruns;
            st.clipped = engine.clipped;
            st.mix_ns += spent.as_nanos() as u64;
            st.mix_us_last = spent.as_micros() as u32;
            st.cpu_percent = (st.mix_ns as f64 / (st.periods.max(1) as f64 * period.as_nanos() as f64) * 100.0) as f32;
            st.ducking = info.ducking;
            st.peak = info.peak;
            st.sink_open = sink.is_open();
            st.sink = sink.name();
        }
        if sink.is_open() {
            if idle_ms >= cfg.idle_ms as f32 {
                // nothing to play for a while: let the card sleep (the sources stay registered)
                sink.close();
                inner.stats.lock().unwrap_or_else(|e| e.into_inner()).sink_open = false;
            } else if let Err(e) = sink.write(&out) {
                log::info!("mixer: the sound device stopped: {e}");
                sink.close();
                set_error(&inner, Some(e));
                retry_at = Some(Instant::now() + OPEN_RETRY);
            }
        }
        if !sink.is_open() {
            // nothing is playing: do not tick at the period rate
            nap(&inner, Duration::from_millis(50));
            next = Instant::now();
            continue;
        }
        if cfg.realtime {
            next += period;
            let now = Instant::now();
            let target = next.checked_sub(period * cfg.lead_periods).unwrap_or(next);
            if target > now {
                std::thread::sleep(target - now);
            } else if now > next + period * 4 {
                next = now; // fell behind (a long write): do not burst to catch up
            }
        }
    }
    sink.close();
    engine.clear();
    for s in inner.registry.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        s.mark_gone(false);
    }
}

// ------------------------------------------------------------------------------------------------ the process-wide mixer

static GLOBAL: OnceLock<Mixer> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The mixer of this computer (started on first use, on IYAKKI's output).
pub fn global() -> &'static Mixer {
    GLOBAL.get_or_init(|| {
        let m = Mixer::start(MixerCfg::default(), Box::new(AplaySink::system()));
        // the software volume lives in the `iyakki` PCM the mixer plays into; where players must scale themselves
        // (no softvol), the factor is applied here
        m.set_master_fn(|| crate::audioout::global().gain() as f32 / 100.0);
        m
    })
}

/// `sound.mixer` of Settings -> Sound.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if let Some(m) = GLOBAL.get() {
        m.set_enabled(on);
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// The global mixer when the setting is on, else None (callers then use their old path).
pub fn global_if_enabled() -> Option<&'static Mixer> {
    enabled().then(|| {
        let m = global();
        m.set_enabled(true);
        m
    })
}

/// The mixer's numbers for the Sound page: None while the setting is off or the mixer was never used.
pub fn stats_if_enabled() -> Option<Stats> {
    if enabled() { GLOBAL.get().map(Mixer::stats) } else { None }
}

impl crate::services::Services {
    /// A place on the speaker for a program: `Disabled` while the mixer is off (the caller says `Busy`/keeps its own player).
    pub fn open_audio_source(&self, kind: SourceKind, rate: u32, channels: usize, label: &str) -> Result<SourceWriter, MixError> {
        global_if_enabled().ok_or(MixError::Disabled)?.open_source_with(kind, rate, channels, label, false)
    }

    /// The mixer's numbers (None while it is off or was never used).
    pub fn mixer_stats(&self) -> Option<Stats> {
        stats_if_enabled()
    }
}
