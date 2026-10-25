//! What a sound device really accepts, learned from the device instead of from a table of models.
//!
//! `aplay -D hw:C,D --dump-hw-params /dev/zero` prints the hardware parameter space of a PCM (before any format is chosen):
//!
//! ```text
//! ACCESS:  MMAP_INTERLEAVED RW_INTERLEAVED
//! FORMAT:  S24_LE S32_LE
//! CHANNELS: [2 4]
//! RATE: 44100
//! PERIOD_SIZE: [8 262144]
//! BUFFER_SIZE: [16 524288]
//! ```
//!
//! [`parse`] turns that text into [`Constraints`] (a single number is a fixed value, `[a b]` is a range, a bare list is a
//! set of values; anything else is ignored, nothing here panics on garbled output), and [`Constraints::mix`] picks the
//! parameters of the `dmix` that [`crate::softvol`] writes: 16-bit when the device takes it (the long-standing default),
//! else 32-bit, 24-bit in 32, or packed 24-bit; stereo, or the device's own minimum (four channels on the MacBookPro14,1,
//! which get the stereo signal on every channel through a `route` plugin); 48 kHz, or the only rate the device has.
//!
//! The question is asked once per device and the answer kept ([`Prober`]): once IYAKKI's `dmix` holds the hardware device,
//! `hw:C,D` is busy and cannot be asked again. A failed look (the device is busy or not ready yet) is not kept as an
//! answer, only remembered for a few seconds so `aplay` is not started on every refresh.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::softvol::{Mix, MIX_RATE};

/// An inclusive range of whole numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub lo: u32,
    pub hi: u32,
}

impl Range {
    fn has(&self, v: u32) -> bool {
        self.lo <= v && v <= self.hi
    }
}

/// What the device accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Constraints {
    /// Sample formats by ALSA name (`S16_LE`, `S32_LE`, ...).
    pub formats: Vec<String>,
    pub channels: Vec<Range>,
    pub rates: Vec<Range>,
    pub period_size: Vec<Range>,
    pub buffer_size: Vec<Range>,
}

/// The formats a dmix can be given, best first: the 16-bit default, then the ones 24-bit-only hardware takes.
pub const FORMAT_ORDER: [&str; 4] = ["S16_LE", "S32_LE", "S24_LE", "S24_3LE"];

/// The most numbers read from one line (a garbled line cannot make this slow or large).
const MAX_ITEMS: usize = 64;

fn num(t: &str) -> Option<i64> {
    t.trim_matches(|c: char| !c.is_ascii_digit()).parse::<i64>().ok().filter(|v| (0..=u32::MAX as i64).contains(v))
}

/// `2`, `44100`, `[2 4]`, `(8 262144]`, `44100 48000`, or `[8000 192000]` as a list of ranges.
fn ranges(v: &str) -> Vec<Range> {
    let v = v.trim();
    if v.starts_with('[') || v.starts_with('(') {
        let nums: Vec<i64> = v.split_whitespace().take(MAX_ITEMS).filter_map(num).collect();
        if nums.len() != 2 {
            return Vec::new();
        }
        // an open end excludes the number itself
        let lo = nums[0] + i64::from(v.starts_with('('));
        let hi = nums[1] - i64::from(v.ends_with(')'));
        if lo > hi {
            return Vec::new();
        }
        return vec![Range { lo: lo as u32, hi: hi as u32 }];
    }
    v.split_whitespace().take(MAX_ITEMS).filter_map(num).map(|n| Range { lo: n as u32, hi: n as u32 }).collect()
}

/// Read the output of `aplay --dump-hw-params`. Unknown lines are skipped; a line that cannot be read counts as absent.
pub fn parse(text: &str) -> Constraints {
    let mut c = Constraints::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else { continue };
        match key.trim() {
            "FORMAT" => {
                c.formats = val.split_whitespace().take(MAX_ITEMS).filter(|f| f.len() <= 16 && f.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')).map(str::to_string).collect()
            }
            "CHANNELS" => c.channels = ranges(val),
            "RATE" => c.rates = ranges(val),
            "PERIOD_SIZE" => c.period_size = ranges(val),
            "BUFFER_SIZE" => c.buffer_size = ranges(val),
            _ => {}
        }
    }
    c
}

fn takes(set: &[Range], v: u32) -> bool {
    set.iter().any(|r| r.has(v))
}

impl Constraints {
    /// Enough was read to choose from (a format, a channel count and a rate).
    pub fn usable(&self) -> bool {
        !self.formats.is_empty() && !self.channels.is_empty() && !self.rates.is_empty()
    }

    /// The device takes this format.
    pub fn has_format(&self, f: &str) -> bool {
        self.formats.iter().any(|x| x == f)
    }

    /// The channel count to open the device with: 2 when it takes 2, else its smallest.
    pub fn pick_channels(&self) -> Option<u32> {
        if takes(&self.channels, 2) {
            return Some(2);
        }
        let lo = self.channels.iter().map(|r| r.lo).filter(|&n| n >= 1).min()?;
        (lo <= 8).then_some(lo)
    }

    /// The only rate the device has, if it has one.
    pub fn fixed_rate(&self) -> Option<u32> {
        let first = self.rates.first()?;
        (first.lo == first.hi && self.rates.iter().all(|r| r == first)).then_some(first.lo)
    }

    /// The only rate when there is one; 48 kHz when the device takes it; else the nearest one.
    pub fn pick_rate(&self) -> Option<u32> {
        if let Some(r) = self.fixed_rate() {
            return Some(r);
        }
        if takes(&self.rates, MIX_RATE) {
            return Some(MIX_RATE);
        }
        self.rates.iter().flat_map(|r| [r.lo, r.hi]).filter(|v| *v > 0).min_by_key(|v| v.abs_diff(MIX_RATE))
    }

    /// Periods of 1024 frames and a buffer of 8192 fit (otherwise the device's own sizes are left alone). Not known
    /// (neither line was read): no.
    pub fn takes_sized(&self) -> bool {
        let known = !self.period_size.is_empty() || !self.buffer_size.is_empty();
        known && (self.period_size.is_empty() || takes(&self.period_size, 1024)) && (self.buffer_size.is_empty() || takes(&self.buffer_size, 8192))
    }

    /// The dmix parameters for this device, None when the text did not say enough.
    pub fn mix(&self) -> Option<Mix> {
        if !self.usable() {
            return None;
        }
        let format = FORMAT_ORDER.iter().find(|f| self.has_format(f)).copied()?;
        let plain = Mix { format, channels: self.pick_channels()?, sized: self.takes_sized(), rate: self.pick_rate()?, probed: false };
        // the long-standing case (16-bit stereo at 48 kHz): exactly what was always written
        if plain == Mix::STEREO {
            return Some(Mix::STEREO);
        }
        Some(Mix { probed: true, ..plain })
    }
}

/// Runs `aplay --dump-hw-params` for a card and device: its output (both streams), or why not.
pub type ProbeFn = Box<dyn Fn(u32, u32) -> Result<String, String> + Send + Sync>;

/// How long a failed look is not repeated.
pub const RETRY_AFTER: Duration = Duration::from_secs(15);

enum Seen {
    Known(Constraints),
    Failed(Instant),
}

/// Asks each device once and remembers the answer for the session.
pub struct Prober {
    run: ProbeFn,
    seen: Mutex<HashMap<(u32, u32), Seen>>,
    retry: Duration,
}

impl Prober {
    pub fn new(run: impl Fn(u32, u32) -> Result<String, String> + Send + Sync + 'static) -> Prober {
        Prober { run: Box::new(run), seen: Mutex::new(HashMap::new()), retry: RETRY_AFTER }
    }

    /// The real `aplay`.
    pub fn system() -> Prober {
        Prober::new(aplay_dump)
    }

    /// How soon a failed look is repeated (tests).
    pub fn with_retry(mut self, d: Duration) -> Prober {
        self.retry = d;
        self
    }

    /// What the device accepts, asking it the first time.
    pub fn constraints(&self, card: u32, device: u32) -> Option<Constraints> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        match seen.get(&(card, device)) {
            Some(Seen::Known(c)) => return Some(c.clone()),
            Some(Seen::Failed(at)) if at.elapsed() < self.retry => return None,
            _ => {}
        }
        match (self.run)(card, device).map(|t| parse(&t)) {
            Ok(c) if c.usable() => {
                log::info!("sound: card {card} device {device} accepts formats {:?}, channels {:?}, rates {:?}", c.formats, c.channels, c.rates);
                seen.insert((card, device), Seen::Known(c.clone()));
                Some(c)
            }
            Ok(_) => {
                log::info!("sound: card {card} device {device}: the parameter list could not be read");
                seen.insert((card, device), Seen::Failed(Instant::now()));
                None
            }
            Err(e) => {
                log::info!("sound: could not ask card {card} device {device} what it accepts: {e}");
                seen.insert((card, device), Seen::Failed(Instant::now()));
                None
            }
        }
    }

    /// The dmix parameters for the device (None: not known, the caller keeps its default).
    pub fn mix(&self, card: u32, device: u32) -> Option<Mix> {
        self.constraints(card, device)?.mix()
    }

    /// Forget what was learned about a card (it went away and may come back as something else).
    pub fn forget(&self, card: u32) {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).retain(|(c, _), _| *c != card);
    }
}

/// `aplay -D hw:C,D --dump-hw-params /dev/zero`: the report goes to stderr; both streams are returned. Ends after 3 s.
pub fn aplay_dump(card: u32, device: u32) -> Result<String, String> {
    use std::io::Read;
    let mut child = std::process::Command::new("aplay")
        .args(["-D", &format!("hw:{card},{device}"), "--dump-hw-params", "/dev/zero"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("aplay: {e}"))?;
    let (so, se) = (child.stdout.take(), child.stderr.take());
    let end = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("aplay did not finish".into());
            }
        }
    };
    // each stream is a screenful at most; read what is there, up to 64 KiB
    let mut text = String::new();
    let mut buf = Vec::new();
    if let Some(r) = so {
        let _ = r.take(64 * 1024).read_to_end(&mut buf);
    }
    text.push_str(&String::from_utf8_lossy(&buf));
    text.push('\n');
    buf.clear();
    if let Some(r) = se {
        let _ = r.take(64 * 1024).read_to_end(&mut buf);
    }
    text.push_str(&String::from_utf8_lossy(&buf));
    if text.contains("FORMAT:") {
        Ok(text)
    } else {
        Err(format!("aplay ended with {status}: {}", text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()))
    }
}

#[cfg(test)]
#[path = "soundprobe_tests.rs"]
mod tests;
