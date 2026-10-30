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
    pipe_bytes: i32,
}

impl AplaySink {
    /// The real one: the device is whatever Settings -> Sound chose, looked at each time the sink opens.
    pub fn system() -> AplaySink {
        AplaySink { program: crate::player::find_program("aplay"), device: Box::new(|| crate::audioout::global().device()), child: None, label: String::new(), tuned: false, pipe_bytes: 0 }
    }

    /// The command for these parameters (public for tests).
    pub fn command(program: &str, device: Option<&str>, rate: u32, channels: usize) -> Vec<String> {
        Self::command_with(program, device, rate, channels, false)
    }

    /// The same, asking aplay for a 500 ms buffer in 50 ms periods when `tuned`.
    pub fn command_with(program: &str, device: Option<&str>, rate: u32, channels: usize, tuned: bool) -> Vec<String> {
        let (b, p) = (format!("--buffer-time={APLAY_BUFFER_US}"), format!("--period-time={APLAY_PERIOD_US}"));
        let mut v = vec![program, "-q"];
        if tuned {
            v.extend([b.as_str(), p.as_str()]);
        }
        let (r, c) = (rate.to_string(), channels.to_string());
        v.extend(["-t", "raw", "-f", "S16_LE", "-r", &r, "-c", &c, "-"]);
        crate::audioout::route_args(v.iter().map(|s| s.to_string()).collect(), device)
    }

    fn spawn(args: &[String]) -> Result<Child, String> {
        let mut child = Command::new(&args[0]).args(&args[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start aplay: {e}"))?;
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
        let got = unsafe { libc::fcntl(si.as_raw_fd(), libc::F_SETPIPE_SZ, PIPE_BYTES) };
        if got > 0 { got } else { unsafe { libc::fcntl(si.as_raw_fd(), libc::F_GETPIPE_SZ) }.max(0) }
    }
}

impl Sink for AplaySink {
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
        let Some(prog) = self.program.clone() else { return Err("no sound player was found (aplay is missing)".into()) };
        let dev = (self.device)();
        self.label = dev.clone().unwrap_or_else(|| "default".into());
        // the device chain is unchanged (plug > softvol > plughw); only aplay's buffer is asked for. A device that refuses those
        // sizes gets aplay's own defaults, the known-good path
        let (child, tuned) = match Self::spawn(&Self::command_with(&prog, dev.as_deref(), rate, channels, true)) {
            Ok(c) => (c, true),
            Err(first) => {
                log::info!("mixer: aplay with a 500 ms buffer did not start ({first}); trying its defaults");
                (Self::spawn(&Self::command(&prog, dev.as_deref(), rate, channels))?, false)
            }
        };
        self.tuned = tuned;
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

    fn detail(&self) -> String {
        format!("pipe {} KB, aplay {}", self.pipe_bytes / 1024, if self.tuned { "buffer 500 ms / period 50 ms" } else { "default buffer" })
    }

    fn latency_ms(&self) -> u32 {
        if self.child.is_none() {
            0
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
    fn name(&self) -> String {
        "fake".into()
    }
}
