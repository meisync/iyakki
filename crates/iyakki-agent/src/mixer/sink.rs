//! Where the mixed audio goes: the card (through `aplay` on IYAKKI's output, the mechanism the radio and the cues use) or, in
//! tests, a recording fake.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub trait Sink: Send {
    /// Claim the device for 16-bit interleaved audio at this rate and channel count.
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String>;
    /// Write one period (blocks as long as the device needs).
    fn write(&mut self, pcm: &[i16]) -> Result<(), String>;
    /// Let go of the device (so the card can sleep and other programs can use it).
    fn close(&mut self);
    fn is_open(&self) -> bool;
    /// For the doctor and the Devices popover.
    fn name(&self) -> String;
    /// Buffer sizes in use, for the status line (empty: nothing to say).
    fn detail(&self) -> String {
        String::new()
    }
    /// How much audio the device itself holds before the speaker when it is full, ms (0: nothing to say). The device does not
    /// start before it is full, so a sample written at the mixer's time `t` is heard at `t` plus what this holds beyond the lead.
    fn latency_ms(&self) -> u32 {
        0
    }
    /// Ask for a small device buffer `(buffer_ms, period_ms)` at the next open (None: the normal one). The mixer sets it while a
    /// live source (the browser) is open, because the device does not start before its buffer is full and holds that much
    /// before the speaker.
    fn set_low_latency(&mut self, _profile: Option<(u32, u32)>) {}
    /// The profile the open device was opened with (None: normal, or closed).
    fn low_latency(&self) -> Option<(u32, u32)> {
        None
    }
    /// Close at once and throw away what the device still holds (no play-out). Defaults to `close`.
    fn abort(&mut self) {
        self.close();
    }
    /// The sound output chosen in Settings → Sound (or the one in use after a plug) changed since this sink was opened: the mixer
    /// closes it and opens it again on the new device, so sound that is playing moves at once and does not stay on the old one.
    fn stale(&self) -> bool {
        false
    }
}

/// The pipe to aplay holds at least this much (the default 64 KB is 340 ms of 48 kHz stereo, hardly more than the mixer's lead).
pub const PIPE_BYTES: i32 = 256 * 1024;
/// aplay's own ring buffer and period, in microseconds. The period is inside what the T2's patched driver takes (256 to 4096 frames).
pub const APLAY_BUFFER_US: u32 = 500_000;
pub const APLAY_PERIOD_US: u32 = 50_000;
/// What the T2's card holds with aplay's own defaults (347 ms, measured earlier), when the 500 ms request is refused.
pub const DEFAULT_BUFFER_MS: u32 = 347;

/// `aplay -q -D <iyakki output> -t raw -f S16_LE -r RATE -c CHANNELS -`, fed through its standard input.
pub struct AplaySink {
    /// The program and its fixed arguments up to the format flags (the rate and channels are appended); the device is added by
    /// `audioout::route_args`.
    program: Option<String>,
    device: Box<dyn Fn() -> Option<String> + Send>,
    child: Option<Child>,
    label: String,
    /// `--buffer-time`/`--period-time` were accepted by the device (else aplay runs with its own defaults).
    tuned: bool,
    /// Wanted at the next open, and the one the open child has (buffer ms, period ms).
    want_low: Option<(u32, u32)>,
    open_low: Option<(u32, u32)>,
    pipe_bytes: i32,
    /// The count of changes of the chosen output (`audioout::Selector::generation`) and the value it had when this sink opened.
    generation: fn() -> u64,
    opened_gen: u64,
}

impl AplaySink {
    /// The real one: the device is whatever Settings -> Sound chose, looked at each time the sink opens.
    pub fn system() -> AplaySink {
        AplaySink { program: crate::player::find_program("aplay"), device: Box::new(|| crate::audioout::global().device()), child: None, label: String::new(), tuned: false, want_low: None, open_low: None, pipe_bytes: 0, generation: || crate::audioout::global().generation(), opened_gen: 0 }
    }

    /// The command for these parameters (public for tests).
    pub fn command(program: &str, device: Option<&str>, rate: u32, channels: usize) -> Vec<String> {
        Self::command_with(program, device, rate, channels, false)
    }

    /// The same, asking aplay for a 500 ms buffer in 50 ms periods when `tuned`.
    pub fn command_with(program: &str, device: Option<&str>, rate: u32, channels: usize, tuned: bool) -> Vec<String> {
        Self::command_sized(program, device, rate, channels, tuned.then_some((APLAY_BUFFER_US / 1000, APLAY_PERIOD_US / 1000)))
    }

    /// The same with an explicit `(buffer ms, period ms)`; None leaves aplay's defaults.
    pub fn command_sized(program: &str, device: Option<&str>, rate: u32, channels: usize, size: Option<(u32, u32)>) -> Vec<String> {
        let tuned = size.is_some();
        let (bm, pm) = size.unwrap_or((APLAY_BUFFER_US / 1000, APLAY_PERIOD_US / 1000));
        let (b, p) = (format!("--buffer-time={}", bm * 1000), format!("--period-time={}", pm * 1000));
        let mut v = vec![program, "-q"];
        if tuned {
            v.extend([b.as_str(), p.as_str()]);
        }
        let (r, c) = (rate.to_string(), channels.to_string());
        v.extend(["-t", "raw", "-f", "S16_LE", "-r", &r, "-c", &c, "-"]);
        crate::audioout::route_args(v.iter().map(|s| s.to_string()).collect(), device)
    }

    fn spawn(args: &[String]) -> Result<Child, String> {
        let args = crate::voice::sound_player_argv(args);
        let mut cmd = Command::new(&args[0]);
        cmd.args(&args[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: prctl(PR_SET_PDEATHSIG) is async-signal-safe. The sink opens on the mixer's own long-lived thread, so the
            // player does not outlive the process (a test run that is cut off, a crash) and keep the card held.
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start aplay: {e}"))?;
        // an aplay that cannot open the device ends within a moment: wait a little to turn that into an error now
        std::thread::sleep(Duration::from_millis(60));
        let st = iyakki_core_api::child_status(&mut child);
        if st.is_over() {
            let why = crate::voice::player_failure(&mut child);
            return Err(format!("aplay ended with {}{}", st.describe(), if why.is_empty() { String::new() } else { format!(": {why}") }));
        }
        Ok(child)
    }

    /// A bigger pipe, so the mixer's lead fits and a write blocks only when the card is really behind. Returns the size got.
    fn grow_pipe(child: &Child) -> i32 {
        use std::os::fd::AsRawFd;
        let Some(si) = child.stdin.as_ref() else { return 0 };
        // SAFETY: fcntl on a pipe descriptor we own; the result is only a size
        let got = unsafe { libc::fcntl(si.as_raw_fd(), crate::oscompat::F_SETPIPE_SZ, PIPE_BYTES) };
        if got > 0 { got } else { unsafe { libc::fcntl(si.as_raw_fd(), crate::oscompat::F_GETPIPE_SZ) }.max(0) }
    }
}

impl Sink for AplaySink {
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
        let Some(prog) = self.program.clone() else { return Err("no sound player was found (aplay is missing)".into()) };
        self.opened_gen = (self.generation)(); // before the device is read: a change in between shows as stale at once
        let dev = (self.device)();
        self.label = dev.clone().unwrap_or_else(|| "default".into());
        // the device chain is unchanged (plug > softvol > plughw); only aplay's buffer is asked for. A device that refuses those
        // sizes gets aplay's own defaults, the known-good path
        let wanted = self.want_low;
        let size = wanted.unwrap_or((APLAY_BUFFER_US / 1000, APLAY_PERIOD_US / 1000));
        let (child, tuned) = match Self::spawn(&Self::command_sized(&prog, dev.as_deref(), rate, channels, Some(size))) {
            Ok(c) => (c, true),
            Err(first) => {
                log::info!("mixer: aplay with a 500 ms buffer did not start ({first}); trying its defaults");
                (Self::spawn(&Self::command(&prog, dev.as_deref(), rate, channels))?, false)
            }
        };
        self.tuned = tuned;
        self.open_low = if tuned { wanted } else { None };
        self.pipe_bytes = Self::grow_pipe(&child);
        self.child = Some(child);
        Ok(())
    }

    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else { return Err("the sound device is not open".into()) };
        let Some(si) = child.stdin.as_mut() else { return Err("the sound device is not open".into()) };
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        si.write_all(&bytes).map_err(|e| {
            format!("aplay stopped: {e}")
        })
    }

    fn close(&mut self) {
        self.open_low = None;
        if let Some(mut child) = self.child.take() {
            // closing the input lets aplay play out what it has and end by itself; killed if it takes too long
            drop(child.stdin.take());
            let end = Instant::now() + Duration::from_millis(600);
            loop {
                match iyakki_core_api::child_status(&mut child) {
                    s if s.is_over() => break,
                    _ if Instant::now() >= end => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }

    fn is_open(&self) -> bool {
        self.child.is_some()
    }

    fn set_low_latency(&mut self, profile: Option<(u32, u32)>) {
        self.want_low = profile;
    }

    fn low_latency(&self) -> Option<(u32, u32)> {
        self.open_low
    }

    fn abort(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.open_low = None;
    }

    fn stale(&self) -> bool {
        self.child.is_some() && (self.generation)() != self.opened_gen
    }

    fn detail(&self) -> String {
        match (self.tuned, self.open_low) {
            (_, Some((b, p))) => format!("pipe {} KB, aplay low latency, buffer {b} ms / period {p} ms", self.pipe_bytes / 1024),
            (true, None) => format!("pipe {} KB, aplay buffer 500 ms / period 50 ms", self.pipe_bytes / 1024),
            _ => format!("pipe {} KB, aplay default buffer", self.pipe_bytes / 1024),
        }
    }

    fn latency_ms(&self) -> u32 {
        if self.child.is_none() {
            0
        } else if let Some((b, _)) = self.open_low {
            b
        } else if self.tuned {
            APLAY_BUFFER_US / 1000
        } else {
            DEFAULT_BUFFER_MS
        }
    }

    fn name(&self) -> String {
        format!("aplay on {}", if self.label.is_empty() { "the IYAKKI output" } else { &self.label })
    }
}

impl Drop for AplaySink {
    fn drop(&mut self) {
        self.close();
    }
}

/// What a [`FakeSink`] saw.
#[derive(Default)]
pub struct FakeLog {
    pub samples: Vec<i16>,
    pub opens: u32,
    pub closes: u32,
    pub writes: u32,
    pub rate: u32,
    pub channels: usize,
    pub fail_open: Option<String>,
    pub open: bool,
    /// Tests: the chosen output changed since the last open (cleared by the next open).
    pub stale: bool,
    /// Low-latency profile asked for / held by the open device, and how many times a device was aborted (queued audio dropped).
    pub want_low: Option<(u32, u32)>,
    pub open_low: Option<(u32, u32)>,
    pub aborts: u32,
    /// Index into `samples` of each write, with its time (ms since the first call of the process clock): for lag measurements.
    pub stamps: Vec<(u64, usize)>,
}

#[derive(Clone, Default)]
pub struct FakeSink {
    pub log: Arc<Mutex<FakeLog>>,
}

impl FakeSink {
    pub fn new() -> FakeSink {
        FakeSink::default()
    }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeLog> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Sink for FakeSink {
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
        let mut l = self.lock();
        if let Some(e) = l.fail_open.clone() {
            return Err(e);
        }
        l.opens += 1;
        l.open = true;
        l.stale = false;
        l.open_low = l.want_low;
        l.rate = rate;
        l.channels = channels;
        Ok(())
    }
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        let mut l = self.lock();
        if !l.open {
            return Err("not open".into());
        }
        l.writes += 1;
        let at = l.samples.len();
        l.stamps.push((crate::mixer::engine::now_us() / 1000, at));
        l.samples.extend_from_slice(pcm);
        Ok(())
    }
    fn close(&mut self) {
        let mut l = self.lock();
        if l.open {
            l.closes += 1;
            l.open = false;
        }
    }
    fn is_open(&self) -> bool {
        self.lock().open
    }
    fn stale(&self) -> bool {
        let l = self.lock();
        l.open && l.stale
    }
    fn set_low_latency(&mut self, profile: Option<(u32, u32)>) {
        self.lock().want_low = profile;
    }
    fn low_latency(&self) -> Option<(u32, u32)> {
        let l = self.lock();
        if l.open { l.open_low } else { None }
    }
    fn abort(&mut self) {
        let mut l = self.lock();
        if l.open {
            l.aborts += 1;
            l.closes += 1;
            l.open = false;
            l.open_low = None;
        }
    }
    fn name(&self) -> String {
        "fake".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::test_procs::{gone, pid_from, script};

    /// A sink whose player is a script that ignores SIGTERM and never reads its input (a hung aplay).
    fn hung(tag: &str) -> (AplaySink, std::path::PathBuf) {
        let (prog, pidfile) = script(tag, "trap '' TERM\nexec sleep 1000");
        let s = AplaySink { program: Some(prog), device: Box::new(|| None), child: None, label: String::new(), tuned: false, want_low: None, open_low: None, pipe_bytes: 0, generation: || 0, opened_gen: 0 };
        (s, pidfile)
    }

    /// Opens, trying again when another test's fork held the fresh script open for writing ("text file busy").
    fn open_retry(s: &mut AplaySink) {
        for _ in 0..50 {
            match s.open(48_000, 2) {
                Err(e) if e.contains("busy") => std::thread::sleep(Duration::from_millis(20)),
                r => return r.unwrap(),
            }
        }
        panic!("the script stayed busy");
    }

    #[test]
    fn a_hung_player_is_killed_and_reaped_by_close_abort_and_drop() {
        let mut ends: Vec<(&str, Box<dyn Fn(AplaySink)>)> = Vec::new();
        ends.push(("close", Box::new(|mut s| s.close())));
        ends.push(("abort", Box::new(|mut s| s.abort())));
        ends.push(("drop", Box::new(drop)));
        for (how, end) in ends {
            let (mut s, pidfile) = hung(how);
            open_retry(&mut s);
            let pid = pid_from(&pidfile);
            assert!(!gone(pid, Duration::ZERO), "{how}: the player runs before the end");
            end(s);
            assert!(gone(pid, Duration::from_secs(3)), "{how}: the player is gone and reaped");
        }
    }

    #[test]
    fn a_player_is_gone_when_the_owner_thread_panics() {
        let (mut s, pidfile) = hung("panic");
        open_retry(&mut s);
        let pid = pid_from(&pidfile);
        let t = std::thread::spawn(move || {
            let _owned = s;
            panic!("the mixer thread died");
        });
        assert!(t.join().is_err());
        assert!(gone(pid, Duration::from_secs(3)), "unwinding dropped the sink, which ended the player");
    }

    #[test]
    fn a_test_run_never_starts_the_real_aplay() {
        let mut s = AplaySink { program: Some("/usr/bin/aplay".into()), device: Box::new(|| None), child: None, label: String::new(), tuned: false, want_low: None, open_low: None, pipe_bytes: 0, generation: || 0, opened_gen: 0 };
        s.open(48_000, 2).unwrap();
        let pid = s.child.as_ref().unwrap().id();
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        assert_ne!(comm.trim(), "aplay", "the sink must use the stand-in under test");
        drop(s);
        assert!(gone(pid, Duration::from_secs(3)));
    }
}
