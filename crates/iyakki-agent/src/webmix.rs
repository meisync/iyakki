//! The web engine's sound through IYAKKI's mixer (experimental, Settings -> Sound "Browser sound through the mixer", off by default).
//!
//! Why: the web engine (GStreamer `alsasink`) opens the sound card by itself, so with the mixer on it is NOT mixed: it either finds
//! the card held by the mixer's one `aplay` ("Device or resource busy") or, when it comes first, locks the mixer out.
//!
//! The route, with no program to start and no dependency: the engine gets its own ALSA configuration ([`conf_text`], the same way
//! [`crate::softvol::WEB_ALSA_CONF`] works) whose `default` PCM is ALSA's built-in `file` plugin writing raw 48 kHz stereo 16-bit
//! PCM into a named pipe (a FIFO next to the web profile). A thread of the agent ([`Tap`]) holds the pipe open for reading, and
//! hands what arrives to the mixer as a media source labelled "Browser": the mixer then ducks it for speech and alarms, applies the
//! master volume and mute, and is the one writer to the card. The pipe is also the clock: when it is full the engine's write
//! blocks, so the mixer's ring paces the page like a sound card would. When nothing arrives for [`IDLE_END`] the source ends and
//! the mixer can let the card sleep.
//!
//! NOT proven on hardware: that this alsa-lib build has the `file` plugin, that `alsasink` keeps its audio clock steady on a
//! `null`-backed PCM (video and sound may drift apart), and the latency. The setting is off; with it off nothing here runs.
//! The pipe, the conversion and the hand-over to the mixer are tested with a fake pipe writer and a fake sink.

use std::io::Read;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::mixer::{MixError, SourceKind, SourceWriter};
pub use iyakki_shared::webmix::{TapState};

/// The engine's `default` PCM when it is routed to the mixer.
pub const PCM: &str = "iyakkiwebmix";
/// What the pipe carries: raw S16_LE, [`RATE`] Hz, [`CHANNELS`] channels (the `plug` plugin in front of the file converts).
pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// No data for this long: the mixer source ends.
pub const IDLE_END: Duration = Duration::from_millis(400);
/// Size asked for the pipe itself (one page, 4 KB = 21 ms of 48 kHz stereo; the kernel default of 64 KB is 340 ms). The engine's
/// `file` plugin over a `null` slave has no clock of its own, so the pipe is the only thing that holds the page back: a deep pipe
/// is a deep delay between picture and sound.
pub const PIPE_BYTES: i32 = 4096;
/// What the tap reads at a time (5 ms), so little audio waits in the tap itself while the ring is full.
const READ_BYTES: usize = 1024;
/// No data for this long: whatever is still queued is thrown away (a pause or a stop must not play on). A stall shorter than this
/// plays out as before.
pub const IDLE_FLUSH: Duration = Duration::from_millis(100);
/// The name of the pipe next to the web profile.
pub const FIFO_NAME: &str = "web-sound.fifo";
/// How the mixer lists this source.
pub const LABEL: &str = "Browser";

/// The ALSA configuration text for the engine: ALSA's own file, then the pipe PCM and `default` set to it. `None` when the path
/// holds a character that could change the configuration (a quote, a newline, a backslash).
pub fn conf_text(fifo: &Path) -> Option<String> {
    let p = fifo.to_str()?;
    if p.is_empty() || p.chars().any(|c| c.is_control() || matches!(c, '"' | '\\' | '\'' | '#' | '{' | '}')) {
        return None;
    }
    Some(format!(
        "</usr/share/alsa/alsa.conf>\npcm.{PCM}file {{\n    type file\n    slave.pcm \"null\"\n    file \"{p}\"\n    format raw\n    perm 0600\n}}\npcm.{PCM} {{\n    type plug\n    slave {{\n        pcm \"{PCM}file\"\n        format S16_LE\n        rate {RATE}\n        channels {CHANNELS}\n    }}\n}}\npcm.!default {PCM}\n"
    ))
}

static WANTED: AtomicBool = AtomicBool::new(false);

/// `sound.web_mix` of Settings -> Sound (the agent applies it with the other sound settings).
pub fn set_wanted(on: bool) {
    if !crate::mixer::may_change_switches() {
        return;
    }
    WANTED.store(on, Ordering::SeqCst);
}

pub fn wanted() -> bool {
    WANTED.load(Ordering::SeqCst)
}

/// Whether the engine is to be routed to the mixer: the setting is on AND the mixer is on (without the mixer there is nothing to
/// route to).
pub fn active() -> bool {
    wanted() && crate::mixer::enabled()
}

/// What the Sound page says, honestly.
pub fn status_text(web_mix: bool, mixer_on: bool, tap: Option<TapState>) -> &'static str {
    match (web_mix, mixer_on, tap) {
        (false, _, _) => "off, route: direct to the sound card (not mixed; the radio and the browser exclude each other on a one-program-at-a-time card)",
        (true, false, _) => "on, but it does nothing while the mixer above is off: the browser opens the sound card by itself",
        (true, true, None) => "route: through the mixer once the web engine starts (open the Browser again to apply it)",
        (true, true, Some(TapState::Idle)) => "route: through the mixer (connected, the page is silent now)",
        (true, true, Some(TapState::Carrying)) => "route: through the mixer, mixed with the radio and other sounds (listed as Browser)",
        (true, true, Some(TapState::Refused)) => "on, but the mixer did not take the page's sound (the card is held by a program outside IYAKKI); the page is silent",
    }
}



/// Where a source for the page's sound comes from (the real one asks the global mixer, tests give a fake one).
pub type Opener = Arc<dyn Fn() -> Result<SourceWriter, MixError> + Send + Sync>;

/// Raw S16_LE bytes to samples; a trailing part of a frame stays in `carry` for the next call.
pub fn bytes_to_samples(carry: &mut Vec<u8>, bytes: &[u8]) -> Vec<i16> {
    carry.extend_from_slice(bytes);
    let frame = CHANNELS * 2;
    let whole = carry.len() / frame * frame;
    let out: Vec<i16> = carry[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    carry.drain(..whole);
    out
}

/// The thread that reads the pipe and feeds the mixer.
pub struct Tap {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    drop_now: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tap {
    /// Make the pipe (an existing pipe is reused, anything else at the path is an error) and start reading it.
    pub fn start(path: &Path, opener: Opener) -> Result<Tap, String> {
        make_fifo(path)?;
        // read AND write so the open never blocks and the pipe never reports "end" when the engine closes its side
        let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path).map_err(|e| format!("cannot open the sound pipe: {e}"))?;
        // SAFETY: fcntl on a pipe descriptor we own; the result is only a size (a refusal leaves the default, which still works)
        let got = unsafe { libc::fcntl(file.as_raw_fd(), crate::oscompat::F_SETPIPE_SZ, PIPE_BYTES) };
        if got <= 0 {
            log::info!("web sound: the pipe could not be made smaller; the delay will be larger");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(AtomicU8::new(TapState::Idle.code()));
        let drop_now = Arc::new(AtomicBool::new(false));
        let (s2, st2, d2) = (stop.clone(), state.clone(), drop_now.clone());
        let thread = std::thread::Builder::new().name("iyakki-webmix".into()).spawn(move || run(file, opener, s2, st2, d2)).map_err(|e| e.to_string())?;
        Ok(Tap { path: path.to_path_buf(), stop, state, drop_now, thread: Some(thread) })
    }

    /// Ask the tap to throw away what is queued (ring, pipe) at its next turn.
    pub fn drop_queued(&self) {
        self.drop_now.store(true, Ordering::SeqCst);
    }

    pub fn state(&self) -> TapState {
        TapState::from_code(self.state.load(Ordering::SeqCst))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn make_fifo(path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    if let Ok(m) = std::fs::symlink_metadata(path) {
        return if m.file_type().is_fifo() { Ok(()) } else { Err("the sound pipe's path holds something else".into()) };
    }
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| "the sound pipe's path is not valid".to_string())?;
    // SAFETY: a NUL-terminated path that lives through the call
    let r = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    if r == 0 { Ok(()) } else { Err(format!("cannot make the sound pipe: {}", std::io::Error::last_os_error())) }
}

fn wait_readable(fd: i32, ms: i32) -> bool {
    // SAFETY: zeroed pollfd is valid (the struct may have extra fields on some targets); one entry, a live fd
    let mut p: libc::pollfd = unsafe { std::mem::zeroed() };
    p.fd = fd;
    p.events = libc::POLLIN;
    let r = unsafe { libc::poll(&mut p, 1, ms) };
    r > 0 && p.revents & libc::POLLIN != 0
}

/// Throw away everything queued for the speaker: the source's ring, the half frame in `carry` and what waits in the pipe. The
/// source stays open (a page that starts again feeds the same one); the mixer lets the card sleep when nothing comes.
fn drop_all(file: &mut std::fs::File, carry: &mut Vec<u8>, src: &mut Option<SourceWriter>) {
    carry.clear();
    if let Some(s) = src.as_ref() {
        s.flush();
    }
    let mut junk = [0u8; 4096];
    while matches!(file.read(&mut junk), Ok(n) if n > 0) {}
}

/// The page stopped, paused or its tile was left: throw away the queued sound now (the tap does it within about 20 ms). Safe to
/// call when the route is off.
pub fn drop_queued() {
    if let Some(t) = TAP.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        t.drop_queued();
    }
}

fn run(mut file: std::fs::File, opener: Opener, stop: Arc<AtomicBool>, state: Arc<AtomicU8>, drop_now: Arc<AtomicBool>) {
    let fd = file.as_raw_fd();
    let mut buf = vec![0u8; READ_BYTES];
    let mut carry: Vec<u8> = Vec::new();
    let mut src: Option<SourceWriter> = None;
    let mut last = Instant::now();
    let set = |s: TapState| state.store(s.code(), Ordering::SeqCst);
    let mut flushed = true;
    while !stop.load(Ordering::SeqCst) {
        if drop_now.swap(false, Ordering::SeqCst) {
            drop_all(&mut file, &mut carry, &mut src);
            flushed = true;
            set(TapState::Idle);
            continue;
        }
        if !wait_readable(fd, 20) {
            if !flushed && last.elapsed() >= IDLE_FLUSH {
                // the page stopped or paused: what is still queued would be heard after it
                drop_all(&mut file, &mut carry, &mut src);
                flushed = true;
            }
            if src.is_some() && last.elapsed() >= IDLE_END {
                if let Some(s) = src.take() {
                    s.finish();
                }
                set(TapState::Idle);
            }
            continue;
        }
        let n = match file.read(&mut buf) {
            Ok(n) if n > 0 => n,
            _ => continue,
        };
        last = Instant::now();
        flushed = false;
        let pcm = bytes_to_samples(&mut carry, &buf[..n]);
        if pcm.is_empty() {
            continue;
        }
        if src.as_ref().is_some_and(|s| s.is_done()) {
            src = None;
        }
        if src.is_none() {
            match opener() {
                Ok(s) => src = Some(s),
                Err(e) => log::info!("web sound: the mixer did not take the page's sound ({e})"),
            }
        }
        match &src {
            Some(s) => match s.write_i16_wait(&pcm, Duration::from_millis(500)) {
                Ok(()) => set(TapState::Carrying),
                Err(e) => {
                    log::info!("web sound: {e}");
                    src = None;
                    set(TapState::Refused);
                }
            },
            None => {
                set(TapState::Refused);
                // drop it at the pace of real time, so the page's clock does not run away
                let ms = pcm.len() as u64 * 1000 / (RATE as u64 * CHANNELS as u64);
                std::thread::sleep(Duration::from_millis(ms.min(100)));
            }
        }
    }
    if let Some(s) = src.take() {
        s.finish();
    }
}

static TAP: Mutex<Option<Tap>> = Mutex::new(None);

/// The state of the process-wide tap (None: it was never started).
pub fn tap_state() -> Option<TapState> {
    TAP.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(Tap::state)
}

/// Start the process-wide tap on the global mixer, once per path.
pub fn ensure(path: &Path) -> bool {
    let mut g = TAP.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|t| t.path() == path) {
        return true;
    }
    *g = None;
    let opener: Opener = Arc::new(|| crate::mixer::global_if_enabled().ok_or(MixError::Disabled)?.open_live_source(SourceKind::Media, RATE, CHANNELS, LABEL));
    match Tap::start(path, opener) {
        Ok(t) => {
            *g = Some(t);
            true
        }
        Err(e) => {
            log::warn!("web sound: {e}");
            false
        }
    }
}

/// The environment for the web engine when it is routed to the mixer: writes the configuration next to the profile, makes sure
/// the tap is reading the pipe BEFORE the engine can open it, and returns `ALSA_CONFIG_PATH`. Empty when the route is off or
/// anything failed (the engine then keeps its usual route).
pub fn engine_env(data_dir: &Path) -> Vec<(String, String)> {
    engine_env_with(active(), data_dir, &ensure)
}

fn engine_env_with(on: bool, data_dir: &Path, start_tap: &dyn Fn(&Path) -> bool) -> Vec<(String, String)> {
    if !on {
        return Vec::new();
    }
    let fifo = data_dir.join(FIFO_NAME);
    let conf = data_dir.join("web-alsa-mix.conf");
    let Some(text) = conf_text(&fifo) else { return Vec::new() };
    if std::fs::read_to_string(&conf).ok().as_deref() != Some(text.as_str()) && std::fs::write(&conf, &text).is_err() {
        return Vec::new();
    }
    if !start_tap(&fifo) {
        return Vec::new();
    }
    // The engine's sound plugin (websink.c) opens the PCM called by IYAKKI_WEBSINK_DEVICE, and "default" is NOT safe here: the
    // included alsa.conf loads /etc/asound.conf AFTER this file (its @hooks run once the file is parsed), and that file's own
    // `pcm.!default` (the exclusive softvol over plughw on a T2 Mac) overrides ours. Proven on the Mac mini: with this file as
    // ALSA_CONFIG_PATH, `aplay -D default` still went to the card, `aplay -D iyakkiwebmix` went into the file plugin. So the sink
    // is told the name of our PCM, which nothing else defines.
    vec![("ALSA_CONFIG_PATH".to_string(), conf.to_string_lossy().into_owned()), ("IYAKKI_WEBSINK_DEVICE".to_string(), PCM.to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{FakeSink, Mixer, MixerCfg};
    use std::io::Write;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webmix-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_mixer() -> (Arc<Mixer>, FakeSink) {
        let sink = FakeSink::new();
        let cfg = MixerCfg { raise_priority: false, realtime: false, ..MixerCfg::default() };
        (Arc::new(Mixer::start(cfg, Box::new(sink.clone()))), sink)
    }

    fn opener_for(m: &Arc<Mixer>) -> Opener {
        let m = m.clone();
        Arc::new(move || m.open_live_source(SourceKind::Media, RATE, CHANNELS, LABEL))
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !f() {
            assert!(t0.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn pcm_bytes(n_frames: usize, v: i16) -> Vec<u8> {
        (0..n_frames * CHANNELS).flat_map(|_| v.to_le_bytes()).collect()
    }

    #[test]
    fn the_configuration_routes_default_to_a_file_pcm_on_the_pipe_and_refuses_odd_paths() {
        let t = conf_text(Path::new("/data/web-sound.fifo")).unwrap();
        assert!(t.starts_with("</usr/share/alsa/alsa.conf>\n"));
        assert!(t.contains("type file") && t.contains("file \"/data/web-sound.fifo\"") && t.contains("slave.pcm \"null\""));
        assert!(t.contains("rate 48000") && t.contains("channels 2") && t.contains("format S16_LE"));
        assert!(t.ends_with("pcm.!default iyakkiwebmix\n"));
        for bad in ["/a\"b", "/a\nb", "/a\\b", "/a#b", "/a{b", ""] {
            assert!(conf_text(Path::new(bad)).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn bytes_become_samples_and_a_split_frame_waits_for_the_rest() {
        let mut carry = Vec::new();
        let all = pcm_bytes(3, 1000);
        let a = bytes_to_samples(&mut carry, &all[..5]);
        assert_eq!(a, vec![1000, 1000]);
        assert_eq!(carry.len(), 1);
        let b = bytes_to_samples(&mut carry, &all[5..]);
        assert_eq!(b.len(), 4);
        assert!(carry.is_empty());
        assert_eq!(bytes_to_samples(&mut carry, &(-2i16).to_le_bytes()), Vec::<i16>::new());
    }

    #[test]
    fn what_the_engine_writes_into_the_pipe_reaches_the_sink_of_the_mixer_and_the_source_ends_when_it_stops() {
        let d = dir("flow");
        let (m, sink) = fake_mixer();
        let fifo = d.join(FIFO_NAME);
        let tap = Tap::start(&fifo, opener_for(&m)).unwrap();
        assert_eq!(tap.state(), TapState::Idle);
        // the engine's side: the file plugin opens the pipe for writing and writes raw PCM
        let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        for _ in 0..20 {
            w.write_all(&pcm_bytes(480, 8000)).unwrap();
        }
        let listed = std::cell::Cell::new(false);
        until("the tap to say it carries sound", || {
            listed.set(listed.get() || m.sources().iter().any(|s| s.label == LABEL && s.kind == SourceKind::Media));
            tap.state() == TapState::Carrying
        });
        assert!(listed.get(), "the mixer lists the page's sound as the Browser media source");
        until("the mixer to receive the page's sound", || sink.lock().samples.iter().filter(|s| **s != 0).count() > 4000);
        {
            let l = sink.lock();
            assert_eq!((l.rate, l.channels), (RATE, CHANNELS));
        }
        drop(w);
        until("the source to end after silence", || m.sources().is_empty() && tap.state() == TapState::Idle);
        drop(tap);
        m.shutdown();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_mixer_that_refuses_is_reported_and_the_pipe_keeps_draining_at_about_real_time() {
        let d = dir("refused");
        let fifo = d.join(FIFO_NAME);
        let opener: Opener = Arc::new(|| Err(MixError::Busy { holder: "a program outside IYAKKI".into() }));
        let tap = Tap::start(&fifo, opener).unwrap();
        let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        let t0 = Instant::now();
        // 10 x 43 ms of sound
        for _ in 0..10 {
            w.write_all(&pcm_bytes(2048, 500)).unwrap();
        }
        until("the refusal", || tap.state() == TapState::Refused);
        // all of it was taken out of the pipe (it did not block the writer) and not faster than real time
        until("the pipe to be drained", || {
            let mut left: libc::c_int = 0;
            // SAFETY: FIONREAD on a live fd with a valid int pointer
            let r = unsafe { libc::ioctl(w.as_raw_fd(), libc::FIONREAD as _, &mut left) };
            r == 0 && left == 0
        });
        assert!(t0.elapsed() >= Duration::from_millis(250), "drained in {:?}", t0.elapsed());
        drop(w);
        drop(tap);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_path_that_is_not_a_pipe_is_not_used_and_an_existing_pipe_is_reused() {
        let d = dir("path");
        let f = d.join("plain");
        std::fs::write(&f, b"x").unwrap();
        let opener: Opener = Arc::new(|| Err(MixError::Disabled));
        assert!(Tap::start(&f, opener.clone()).is_err());
        let p = d.join("pipe");
        drop(Tap::start(&p, opener.clone()).unwrap());
        assert!(std::fs::symlink_metadata(&p).unwrap().file_type().is_fifo());
        assert!(Tap::start(&p, opener).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_route_needs_to_be_on_and_a_running_tap_and_the_status_says_what_is_true() {
        let d = dir("env");
        let started = std::cell::Cell::new(0);
        let ok = |_: &Path| {
            started.set(started.get() + 1);
            true
        };
        assert!(engine_env_with(false, &d, &ok).is_empty());
        assert_eq!(started.get(), 0, "nothing starts while the route is off");
        // a directory that cannot be written, or a tap that cannot start: no environment, the engine keeps its usual route
        assert!(engine_env_with(true, Path::new("/nonexistent-dir"), &ok).is_empty());
        assert!(engine_env_with(true, &d, &|_| false).is_empty());
        let env = engine_env_with(true, &d, &ok);
        assert_eq!(started.get(), 1);
        assert_eq!(env.len(), 2);
        assert_eq!(env[0].0, "ALSA_CONFIG_PATH");
        assert_eq!(env[1], ("IYAKKI_WEBSINK_DEVICE".to_string(), "iyakkiwebmix".to_string()), "the sink names our PCM, not `default` that asound.conf overrides");
        assert_eq!(std::fs::read_to_string(d.join("web-alsa-mix.conf")).unwrap(), conf_text(&d.join(FIFO_NAME)).unwrap());
        let _ = std::fs::remove_dir_all(&d);

        assert!(status_text(false, true, None).starts_with("off"));
        assert!(status_text(true, false, None).contains("does nothing"));
        assert!(status_text(true, true, None).contains("through the mixer") && status_text(false, true, None).contains("direct"));
        assert!(status_text(true, true, Some(TapState::Refused)).contains("silent"));
        assert!(status_text(true, true, Some(TapState::Carrying)).contains("through the mixer"));
    }

    /// A mixer paced like a card (realtime), with the browser numbers of the default configuration (100 ms target).
    fn paced_mixer() -> (Arc<Mixer>, FakeSink, MixerCfg) {
        let sink = FakeSink::new();
        let cfg = MixerCfg { raise_priority: false, idle_ms: 20_000, ..MixerCfg::default() }.with_live_target_ms(crate::mixer::DEFAULT_LIVE_TARGET_MS);
        (Arc::new(Mixer::start(cfg, Box::new(sink.clone()))), sink, cfg)
    }

    #[test]
    fn the_page_sound_lags_by_less_than_the_bound_during_a_long_stream_with_a_page_that_writes_as_fast_as_the_pipe_allows() {
        let _rt = crate::mixer::rt_serial();
        // retried: the bound adds the scheduling delay measured during the run, but a host this busy can still starve the thread in a way the probe misses
        crate::mixer::retried_under_load(3, || {
            // The worst case for lag: a page that fills every queue (what alsasink over a clockless `null` slave does). Chunk k carries
            // the value 1000 + k; the time it was accepted by the pipe is noted, and the time the sink got it plus the device buffer
            // (the fake card plays a period when the mixer hands it over plus the lead, the real one after its 40 ms buffer) is
            // when it would be heard.
            let d = dir("lag");
            let (m, sink, cfg) = paced_mixer();
            let fifo = d.join(FIFO_NAME);
            let tap = Tap::start(&fifo, opener_for_live(&m)).unwrap();
            let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
            let t0 = crate::mixer::engine::now_us() / 1000;
            let mut accepted: Vec<u64> = Vec::new();
            let secs = 6;
            let chunks = secs * 100; // 10 ms each
            let mut sent = 0usize;
            // The mixer thread is a normal thread here (no real-time priority): on a loaded machine the scheduler can keep it off the CPU
            // for longer than the bound, and that delay is the machine's, not the pipeline's. A probe thread measures how late a 5 ms
            // sleep wakes up while the stream runs; the largest overshoot is added to the bound (about 0 to 1 ms on an idle machine).
            let probe_stop = Arc::new(AtomicBool::new(false));
            let probe = {
                let stop = probe_stop.clone();
                std::thread::spawn(move || {
                    let mut worst = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let t = std::time::Instant::now();
                        std::thread::sleep(Duration::from_millis(5));
                        worst = worst.max((t.elapsed().as_millis() as u64).saturating_sub(5));
                    }
                    worst
                })
            };
            // a real page is paced by its sink: keep roughly real time on average but never wait for the queues to drain
            while sent < chunks {
                w.write_all(&pcm_bytes(480, 1000 + sent as i16)).unwrap();
                accepted.push(crate::mixer::engine::now_us() / 1000);
                sent += 1;
                // the page can run at most 1 s ahead of the clock: sleeps when ahead, so the whole run takes about `secs` seconds
                let due = t0 + sent as u64 * 10;
                let ahead = due.saturating_sub(crate::mixer::engine::now_us() / 1000);
                if ahead > 1000 {
                    std::thread::sleep(Duration::from_millis(ahead - 1000));
                }
            }
            until("the mixer to receive the last chunk", || sink.lock().samples.iter().filter(|s| **s == 1000 + chunks as i16 - 1).count() > 0);
            let lead_ms = (cfg.live_lead_periods * 20) as u64;
            let l = sink.lock();
            let mut worst = 0u64;
            let mut count = 0;
            for k in 100..chunks {
                let v = 1000 + k as i16;
                let Some(pos) = l.samples.iter().position(|s| *s == v) else { continue };
                let Some(&(t_sink, _)) = l.stamps.iter().rev().find(|(_, at)| *at <= pos) else { continue };
                let lag = (t_sink + lead_ms).saturating_sub(accepted[k]);
                worst = worst.max(lag);
                count += 1;
            }
            assert!(count > chunks as usize / 2, "only {count} chunks found in the sink");
            drop(l);
            // the 100 ms target plus the 10 ms in which the test notes the time of a chunk
            eprintln!("worst page-sound lag over {count} chunks: {worst} ms");
            probe_stop.store(true, Ordering::Relaxed);
            let jitter = probe.join().unwrap();
            // The longest time the mixer thread itself went without writing a period (the pace is one period of 20 ms): while it is
            // off the CPU everything queues up by that much, which the probe above only sees in part.
            let stall = {
                let l = sink.lock();
                l.stamps.windows(2).map(|w| w[1].0.saturating_sub(w[0].0)).max().unwrap_or(0).saturating_sub(20)
            };
            eprintln!("worst scheduling overshoot of a 5 ms sleep during the run: {jitter} ms; longest pause of the mixer thread beyond its period: {stall} ms");
            assert!(worst <= crate::mixer::DEFAULT_LIVE_TARGET_MS as u64 + 10 + jitter + stall, "worst lag {worst} ms is above the bound (plus {jitter} ms of scheduling delay and {stall} ms of mixer-thread pause measured on this machine)");
            drop(w);
            drop(tap);
            m.shutdown();
            let _ = std::fs::remove_dir_all(&d);
        });
    }

    #[test]
    fn the_queue_is_empty_within_a_period_of_a_stop_and_a_pause_does_not_play_on() {
        let _rt = crate::mixer::rt_serial();
        // timing of a 20 ms period on a shared host: tried up to three times; a stop that does not drain fails every time
        crate::mixer::retried_under_load(3, || {
            let d = dir("stop");
            let (m, _sink, _cfg) = paced_mixer();
            let fifo = d.join(FIFO_NAME);
            let tap = Tap::start(&fifo, opener_for_live(&m)).unwrap();
            let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
            // fill every queue, then stop writing (a pause): the ring is flushed after IDLE_FLUSH, not played for seconds
            for _ in 0..30 {
                w.write_all(&pcm_bytes(480, 7000)).unwrap();
            }
            until("sound to be carried", || tap.state() == TapState::Carrying);
            let queued = || m.sources().iter().find(|s| s.label == LABEL).map(|s| s.buffered_ms).unwrap_or(0);
            let t_pause = Instant::now();
            until("the ring to be empty after the pause", || queued() == 0);
            let took = t_pause.elapsed();
            assert!(took <= IDLE_FLUSH + Duration::from_millis(60), "the queue took {took:?} to empty after the page stopped");

            // an explicit stop (the tile is left): within one 20 ms period (plus the tap's 20 ms poll), pipe included
            let sent = std::thread::spawn(move || {
                for _ in 0..8 {
                    if w.write_all(&pcm_bytes(480, 7000)).is_err() {
                        break;
                    }
                }
                w
            });
            let w = sent.join().unwrap();
            let t_stop = Instant::now();
            tap.drop_queued();
            until("the queues to be empty after a stop", || {
                let mut left: libc::c_int = 0;
                // SAFETY: FIONREAD on a live fd with a valid int pointer
                let r = unsafe { libc::ioctl(w.as_raw_fd(), libc::FIONREAD as _, &mut left) };
                queued() == 0 && r == 0 && left == 0
            });
            assert!(t_stop.elapsed() <= Duration::from_millis(60), "the stop took {:?}", t_stop.elapsed());
            drop(w);
            drop(tap);
            m.shutdown();
            let _ = std::fs::remove_dir_all(&d);
        });
    }

    fn opener_for_live(m: &Arc<Mixer>) -> Opener {
        let m = m.clone();
        Arc::new(move || m.open_live_source(SourceKind::Media, RATE, CHANNELS, LABEL))
    }

    #[test]
    fn the_pipe_is_one_page_not_the_kernel_default() {
        let d = dir("pipe");
        let fifo = d.join(FIFO_NAME);
        let opener: Opener = Arc::new(|| Err(MixError::Disabled));
        let tap = Tap::start(&fifo, opener).unwrap();
        let w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        // SAFETY: F_GETPIPE_SZ on a live pipe descriptor
        let size = unsafe { libc::fcntl(w.as_raw_fd(), crate::oscompat::F_GETPIPE_SZ) };
        assert!(size > 0 && size <= 8192, "pipe size {size}");
        drop(w);
        drop(tap);
        let _ = std::fs::remove_dir_all(&d);
    }
}
