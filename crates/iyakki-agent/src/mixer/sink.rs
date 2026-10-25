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
}

/// `aplay -q -D <iyakki output> -t raw -f S16_LE -r RATE -c CHANNELS -`, fed through its standard input.
pub struct AplaySink {
    /// The program and its fixed arguments up to the format flags (the rate and channels are appended); the device is added by
    /// `audioout::route_args`.
    program: Option<String>,
    device: Box<dyn Fn() -> Option<String> + Send>,
    child: Option<Child>,
    label: String,
}

impl AplaySink {
    /// The real one: the device is whatever Settings -> Sound chose, looked at each time the sink opens.
    pub fn system() -> AplaySink {
        AplaySink { program: crate::player::find_program("aplay"), device: Box::new(|| crate::audioout::global().device()), child: None, label: String::new() }
    }

    /// The command for these parameters (public for tests).
    pub fn command(program: &str, device: Option<&str>, rate: u32, channels: usize) -> Vec<String> {
        let args: Vec<String> = [program, "-q", "-t", "raw", "-f", "S16_LE", "-r", &rate.to_string(), "-c", &channels.to_string(), "-"].iter().map(|s| s.to_string()).collect();
        crate::audioout::route_args(args, device)
    }
}

impl Sink for AplaySink {
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
        let Some(prog) = self.program.clone() else { return Err("no sound player was found (aplay is missing)".into()) };
        let dev = (self.device)();
        let args = Self::command(&prog, dev.as_deref(), rate, channels);
        self.label = dev.unwrap_or_else(|| "default".into());
        let mut child = Command::new(&args[0]).args(&args[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start aplay: {e}"))?;
        // an aplay that cannot open the device ends within a moment: wait a little to turn that into an error now
        std::thread::sleep(Duration::from_millis(60));
        if let Ok(Some(st)) = child.try_wait() {
            let why = crate::voice::player_failure(&mut child);
            return Err(format!("aplay ended with {st}{}", if why.is_empty() { String::new() } else { format!(": {why}") }));
        }
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
                match child.try_wait() {
                    Ok(Some(_)) => break,
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
