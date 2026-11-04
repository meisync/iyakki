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
#[cfg(test)]
mod stress_tests;

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
    /// Periods of mixed audio kept written ahead of the card (absorbs scheduling stalls; adds this much latency to ducking and
    /// to a new source). 15 periods are 300 ms.
    pub lead_periods: u32,
    /// Ask for a higher scheduling priority for the mixer thread (failure is ignored and noted in the status).
    pub raise_priority: bool,
    /// Sleep to the period clock. Tests turn this off to mix faster than real time.
    pub realtime: bool,
}

impl Default for MixerCfg {
    fn default() -> Self {
        MixerCfg { engine: EngineCfg::default(), idle_ms: 1500, lead_periods: 15, raise_priority: true, realtime: true }
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
    /// Times a source ran dry while sounding (network, decoder or TTS too slow). Silent and faded, never a click.
    pub underruns: u64,
    /// The same by kind (index = `SourceKind::priority()`: radio, media, notification, speech, alarm).
    pub starved: [u64; 5],
    /// Times the mixer thread came too late: the card had run out of mixed audio (a scheduling stall longer than the lead).
    pub sink_late: u64,
    /// The least queued time among the streams that are sounding, ms (how close the nearest underrun is).
    pub buffered_ms: u32,
    /// Mixed audio written ahead of the card, ms (by the mixer's clock).
    pub ahead_ms: u32,
    /// What the mixer thread itself used, percent of one core (mixing, resampling, writing).
    pub thread_cpu_percent: f32,
    /// How the thread was prioritised: "realtime 10", "nice -10", "normal (not permitted)".
    pub priority: String,
    /// Buffer sizes of the sink.
    pub sink_detail: String,
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
        if self.sink_late > 0 {
            s.push_str(&format!(", {} late period(s)", self.sink_late));
        }
        if self.ducking {
            s.push_str(", ducking");
        }
        if let Some(e) = &self.last_error {
            s.push_str(&format!("; last problem: {e}"));
        }
        s
    }
}

impl Stats {
    /// The longer text of Settings -> Sound: the line above with the numbers that tell a starved source from a late mixer.
    pub fn detail(&self) -> String {
        let k = |i: usize| self.starved[i];
        let mut s = format!(
            "{} source(s) · {:.1}% of a core · {} underrun(s) from sources (radio {}, media {}, speech {}, other {}) · {} late period(s) · {} ms buffered, {} ms ahead · mixer thread {:.1}% CPU",
            self.sources,
            self.cpu_percent,
            self.underruns,
            k(0),
            k(1),
            k(3),
            k(2) + k(4),
            self.sink_late,
            self.buffered_ms,
            self.ahead_ms,
            self.thread_cpu_percent
        );
        if !self.priority.is_empty() {
            s.push_str(&format!(" · priority {}", self.priority));
        }
        if !self.sink_detail.is_empty() {
            s.push_str(&format!(" · {}", self.sink_detail));
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

/// The thread's CPU time, ns.
fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes the timespec we pass
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
        return 0;
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Nice -10 and, if permitted, round-robin real time at a low priority, for the calling thread. Returns what worked.
fn raise_thread_priority() -> String {
    // SAFETY: plain scheduling calls on the calling thread
    unsafe {
        let mut what = String::from("normal (not permitted)");
        if libc::setpriority(libc::PRIO_PROCESS, 0, -10) == 0 {
            what = "nice -10".into();
        }
        let mut sp: libc::sched_param = std::mem::zeroed();
        sp.sched_priority = 10;
        if libc::sched_setscheduler(0, libc::SCHED_RR, &sp) == 0 {
            what = "realtime 10".into();
        }
        what
    }
}

fn run(inner: Arc<Inner>, mut sink: Box<dyn Sink>) {
    let cfg = inner.cfg;
    let priority = if cfg.raise_priority { raise_thread_priority() } else { "normal".into() };
    log::info!("mixer: thread priority {priority}");
    inner.stats.lock().unwrap_or_else(|e| e.into_inner()).priority = priority;
    let mut engine = Engine::new(cfg.engine);
    let period = cfg.engine.period();
    let period_ms = period.as_secs_f32() * 1000.0;
    let lead = period * cfg.lead_periods;
    let mut out: Vec<i16> = Vec::with_capacity(cfg.engine.period_frames * cfg.engine.channels);
    // the card's timeline: it began playing at `anchor` and has been given `written` of audio since
    let mut anchor = Instant::now();
    let mut written = Duration::ZERO;
    let mut late = 0u64;
    let mut cpu_win = (Instant::now(), thread_cpu_ns());
    let mut thread_cpu = 0.0f32;
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
            inner.wake.end_us.store(0, Ordering::SeqCst);
            {
                let mut st = inner.stats.lock().unwrap_or_else(|e| e.into_inner());
                st.running = false;
                st.sink_open = false;
                st.sources = 0;
                st.ducking = false;
                st.buffered_ms = 0;
                st.ahead_ms = 0;
            }
            nap(&inner, Duration::from_millis(250));
            idle_ms = 0.0;
            continue;
        }
        // the chosen output changed while sound plays (Settings → Sound, a monitor plugged in): close the card, the branch below opens
        // it again on the new device; the sources keep their queues, so what plays goes on after a short gap
        if sink.is_open() && sink.stale() {
            log::info!("mixer: the sound output changed; playing on the new one");
            sink.close();
            inner.wake.end_us.store(0, Ordering::SeqCst);
        }
        // the card must be open before audio is taken from the sources
        if !sink.is_open() && engine.wants_sink() {
            if retry_at.is_none_or(|t| Instant::now() >= t) {
                match sink.open(cfg.engine.rate, cfg.engine.channels) {
                    Ok(()) => {
                        retry_at = None;
                        // pre-roll: the whole lead is written at once as silence, so the card never starts empty and the sources
                        // (some may hold less than the lead) are drained at the pace of the clock from the first sound on
                        anchor = Instant::now();
                        written = Duration::ZERO;
                        if cfg.realtime {
                            let zeros = vec![0i16; cfg.engine.period_frames * cfg.engine.channels];
                            for _ in 0..cfg.lead_periods {
                                if sink.write(&zeros).is_err() {
                                    break;
                                }
                                written += period;
                            }
                        }
                        idle_ms = 0.0;
                        set_error(&inner, None);
                        let mut st = inner.stats.lock().unwrap_or_else(|e| e.into_inner());
                        st.sink_opens += 1;
                        st.sink_detail = sink.detail();
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
        // pace by what the card has in hand: stay `lead` ahead of it, no further (the sources are drained as late as possible,
        // so ducking and new sources are heard soon); a thread that was stalled mixes the periods it missed back to back
        if cfg.realtime && sink.is_open() {
            let elapsed = anchor.elapsed();
            if written > Duration::ZERO && elapsed > written {
                // the card ran out of mixed audio before this period was ready
                late += 1;
                if late <= 5 {
                    log::info!("mixer: late by {} ms (the mixer thread was not scheduled in time)", (elapsed - written).as_millis());
                }
                anchor = Instant::now().checked_sub(written).unwrap_or_else(Instant::now);
            } else if written > elapsed + lead {
                std::thread::sleep(written - elapsed - lead);
                if inner.shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
        let master = inner.master.lock().unwrap_or_else(|e| e.into_inner()).clone().map_or_else(|| f32::from_bits(inner.master_fixed.load(Ordering::SeqCst)), |f| f());
        // when the end of this period is heard: the mixer's own clock says `written + period` after the anchor, and the card starts only
        // once its buffer is full, so what it holds beyond the lead is a delay the clock does not know
        let card_extra_us = (sink.latency_ms() as u64 * 1000).saturating_sub(lead.as_micros() as u64);
        let end = if sink.is_open() { engine::now_us() + (written + period).saturating_sub(anchor.elapsed()).as_micros() as u64 + card_extra_us } else { 0 };
        inner.wake.end_us.store(end, Ordering::SeqCst);
        let t0 = Instant::now();
        let info = engine.mix_period(master, &mut out);
        let spent = t0.elapsed();
        if info.supplying > 0 {
            idle_ms = 0.0;
        } else {
            idle_ms += period_ms;
        }
        if sink.is_open() {
            if idle_ms >= cfg.idle_ms as f32 {
                // nothing to play for a while: let the card sleep (the sources stay registered)
                sink.close();
                inner.wake.end_us.store(0, Ordering::SeqCst);
            } else if let Err(e) = sink.write(&out) {
                log::info!("mixer: the sound device stopped: {e}");
                sink.close();
                inner.wake.end_us.store(0, Ordering::SeqCst);
                set_error(&inner, Some(e));
                retry_at = Some(Instant::now() + OPEN_RETRY);
            } else {
                written += period;
            }
        }
        if cpu_win.0.elapsed() >= Duration::from_secs(1) {
            let ns = thread_cpu_ns();
            thread_cpu = ((ns.saturating_sub(cpu_win.1)) as f64 / cpu_win.0.elapsed().as_nanos().max(1) as f64 * 100.0) as f32;
            cpu_win = (Instant::now(), ns);
        }
        let is_open = sink.is_open();
        let ahead = if is_open && cfg.realtime { written.saturating_sub(anchor.elapsed()).as_millis() as u32 } else { 0 };
        let name = sink.name();
        {
            let mut st = inner.stats.lock().unwrap_or_else(|e| e.into_inner());
            st.running = true;
            st.sources = info.sources;
            st.periods = engine.periods;
            st.underruns = engine.underruns;
            st.starved = engine.starved;
            st.sink_late = late;
            st.buffered_ms = engine.buffered_ms();
            st.ahead_ms = ahead;
            st.thread_cpu_percent = thread_cpu;
            st.clipped = engine.clipped;
            st.mix_ns += spent.as_nanos() as u64;
            st.mix_us_last = spent.as_micros() as u32;
            st.cpu_percent = (st.mix_ns as f64 / (st.periods.max(1) as f64 * period.as_nanos() as f64) * 100.0) as f32;
            st.ducking = info.ducking;
            st.peak = info.peak;
            st.sink_open = is_open;
            st.sink = name;
        }
        if !is_open {
            // nothing is playing: do not tick at the period rate
            nap(&inner, Duration::from_millis(50));
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
    // a test holding [`exclusive_route`] owns the switch: other tests' settings changes do not reach it
    if !may_change_switches() {
        return;
    }
    ENABLED.store(on, Ordering::SeqCst);
    if let Some(m) = GLOBAL.get() {
        m.set_enabled(on);
    }
}

pub fn enabled() -> bool {
    PINNED_OFF.load(Ordering::SeqCst) == 0 && ENABLED.load(Ordering::SeqCst)
}

// Test isolation. The switch is process-wide and any test that changes sound settings flips it, so tests that need a known
// route say so: many can pin it off together (players use the play command), or one can own it (and assert what it set).
// Nothing in the product calls these.
static ROUTE_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());
static PINNED_OFF: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static OWNER: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

/// False while another thread's test owns the sound switches ([`exclusive_route`]).
pub(crate) fn may_change_switches() -> bool {
    OWNER.lock().unwrap_or_else(|e| e.into_inner()).is_none_or(|o| o == std::thread::current().id())
}

/// While alive, the mixer is off for every thread, whatever other tests set: sounds go to the play command.
#[doc(hidden)]
pub struct CommandRoute(#[allow(dead_code)] std::sync::RwLockReadGuard<'static, ()>);

#[doc(hidden)]
pub fn pin_command_route() -> CommandRoute {
    let g = ROUTE_GATE.read().unwrap_or_else(|e| e.into_inner());
    PINNED_OFF.fetch_add(1, Ordering::SeqCst);
    CommandRoute(g)
}

impl Drop for CommandRoute {
    fn drop(&mut self) {
        PINNED_OFF.fetch_sub(1, Ordering::SeqCst);
    }
}

/// While alive, no pinned test runs and only this thread's [`set_enabled`] changes the switch.
#[doc(hidden)]
pub struct ExclusiveRoute(#[allow(dead_code)] std::sync::RwLockWriteGuard<'static, ()>);

#[doc(hidden)]
pub fn exclusive_route() -> ExclusiveRoute {
    let g = ROUTE_GATE.write().unwrap_or_else(|e| e.into_inner());
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
    ExclusiveRoute(g)
}

impl Drop for ExclusiveRoute {
    fn drop(&mut self) {
        *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
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
