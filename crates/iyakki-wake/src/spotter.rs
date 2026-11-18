//! The streaming wake phrase spotter and the trait the voice assistant uses. Everything here is memory only: a ring buffer of three
//! seconds of samples that is overwritten, a queue of feature frames, a few counters. This file touches no file and no network
//! (a test reads the source to prove it).
//!
//! Cost control, in order: (1) the energy gate looks at the loudness of every 10 ms hop (a handful of multiplications); (2) only while
//! the gate is open, and for 1.2 s after, are log-mel frames computed; (3) only then, every 100 ms, is the network run on the last 1.5 s;
//! (4) the scores are smoothed over three runs, compared with the person's threshold and followed by a cool-down.
//!
//! Privacy rules, each a test: nothing is heard while the spotter is not armed or is muted (the samples are dropped on arrival, the
//! buffers are wiped when it stops listening); `indicator()` is true exactly while the microphone is open to the spotter, so a screen
//! never shows "off" while it listens.

use crate::features::{normalise_window, LogMel, FRAME, HOP, N_MEL, SAMPLE_RATE, WINDOW_FRAMES};
use crate::model::{Net, Workspace};
use std::collections::VecDeque;

/// Three seconds of samples are kept, in memory only.
pub const RING_SAMPLES: usize = 3 * SAMPLE_RATE;
const SCORE_EVERY_HOPS: u64 = 10;
const HOLD_HOPS: u64 = 120;
const GATE_RATIO: f32 = 3.0;
const GATE_MIN_RMS: f32 = 0.003;
const SMOOTH: usize = 3;

pub const DEFAULT_THRESHOLD: f32 = 0.85;
pub const DEFAULT_COOLDOWN_MS: u64 = 2500;

/// What the spotter found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeEvent {
    /// The smoothed probability that the phrase was just said (0 to 1).
    pub confidence: f32,
    /// Stream time since the spotter was armed, in milliseconds.
    pub at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeState {
    /// Not armed: nothing listens.
    Off,
    /// Armed and not muted: the microphone is open to the spotter.
    Listening,
    /// Armed but muted: nothing listens until unmuted.
    Muted,
}

/// What the voice assistant uses. Rules every implementation keeps (and the tests check for the real one and the fake one):
///
/// * `feed` returns nothing and keeps nothing unless the state is `Listening`.
/// * Muting or disarming wipes everything buffered at once; unmuting starts from nothing.
/// * `indicator()` is true exactly while `state() == Listening`. The top bar and the Voice tile draw it, and never draw "off" while it is true.
/// * The default is `Off`: opting in is the person's act (`voice.my_iyakki_wake`, off by default), through a hard-ask tool or the tile.
pub trait WakeSpotter {
    fn state(&self) -> WakeState;
    fn arm(&mut self);
    fn disarm(&mut self);
    fn set_muted(&mut self, muted: bool);
    /// 16 kHz mono samples in, wake events out (usually none).
    fn feed(&mut self, samples: &[f32]) -> Vec<WakeEvent>;
    fn indicator(&self) -> bool {
        self.state() == WakeState::Listening
    }
}

/// Counters for the status line and the person's miss/false-wake feedback; no audio.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub hops: u64,
    pub gate_open_hops: u64,
    pub windows_scored: u64,
    pub wakes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub threshold: f32,
    pub cooldown_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { threshold: DEFAULT_THRESHOLD, cooldown_ms: DEFAULT_COOLDOWN_MS }
    }
}

/// The part that turns samples into smoothed scores: ring, energy gate, features, network. No threshold, no mute.
pub struct Scorer {
    net: Net,
    ws: Workspace,
    lm: LogMel,
    ring: Vec<f32>,
    write: usize,
    total: u64,
    hop_acc: Vec<f32>,
    recent_rms: [f32; 3],
    floor: f32,
    hops: u64,
    hold_until: u64,
    frames: VecDeque<[f32; N_MEL]>,
    last_frame: Option<u64>,
    window: Vec<f32>,
    recent_scores: [f32; SMOOTH],
    pub stats: Stats,
}

/// One scoring step.
#[derive(Clone, Copy, Debug)]
pub struct Score {
    pub at_ms: u64,
    pub smoothed: f32,
}

impl Scorer {
    pub fn new(net: Net) -> Scorer {
        Scorer {
            net,
            ws: Workspace::default(),
            lm: LogMel::new(),
            ring: vec![0.0; RING_SAMPLES],
            write: 0,
            total: 0,
            hop_acc: Vec::with_capacity(HOP),
            recent_rms: [0.0; 3],
            floor: 0.0,
            hops: 0,
            hold_until: 0,
            frames: VecDeque::with_capacity(WINDOW_FRAMES + 1),
            last_frame: None,
            window: vec![0.0; WINDOW_FRAMES * N_MEL],
            recent_scores: [0.0; SMOOTH],
            stats: Stats::default(),
        }
    }

    /// Forget all audio and state (ring zeroed, queues emptied).
    pub fn wipe(&mut self) {
        self.ring.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.total = 0;
        self.hop_acc.clear();
        self.recent_rms = [0.0; 3];
        self.floor = 0.0;
        self.hops = 0;
        self.hold_until = 0;
        self.frames.clear();
        self.last_frame = None;
        self.window.iter_mut().for_each(|v| *v = 0.0);
        self.recent_scores = [0.0; SMOOTH];
    }

    pub fn now_ms(&self) -> u64 {
        self.total * 1000 / SAMPLE_RATE as u64
    }

    /// The last `ms` of audio, copied out for a second-stage check. The caller must use it at once and drop it (never keep or save it).
    pub fn recent_audio(&self, ms: usize) -> Vec<f32> {
        let n = (ms * SAMPLE_RATE / 1000).min(RING_SAMPLES).min(self.total as usize);
        self.copy_back(n, 0)
    }

    /// `n` samples ending `skip` samples before the newest one.
    fn copy_back(&self, n: usize, skip: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        let end = self.write + RING_SAMPLES - skip;
        for i in 0..n {
            out.push(self.ring[(end + RING_SAMPLES - n + i) % RING_SAMPLES]);
        }
        out
    }

    fn push(&mut self, s: f32) {
        self.ring[self.write] = s;
        self.write = (self.write + 1) % RING_SAMPLES;
        self.total += 1;
    }

    /// Feed samples; calls `on_score` for every scoring step.
    pub fn feed(&mut self, samples: &[f32], on_score: &mut dyn FnMut(Score)) {
        for &s in samples {
            self.push(s);
            self.hop_acc.push(s);
            if self.hop_acc.len() == HOP {
                let rms = (self.hop_acc.iter().map(|v| v * v).sum::<f32>() / HOP as f32).sqrt();
                self.hop_acc.clear();
                self.on_hop(rms, on_score);
            }
        }
    }

    fn on_hop(&mut self, rms: f32, on_score: &mut dyn FnMut(Score)) {
        self.hops += 1;
        self.stats.hops += 1;
        self.recent_rms = [self.recent_rms[1], self.recent_rms[2], rms];
        let short = self.recent_rms.iter().sum::<f32>() / 3.0;
        if self.floor == 0.0 {
            self.floor = rms.max(1e-5);
        } else if rms < self.floor {
            self.floor = 0.8 * self.floor + 0.2 * rms;
        } else {
            self.floor = self.floor * 1.002 + 1e-7;
        }
        if short > (self.floor * GATE_RATIO).max(GATE_MIN_RMS) {
            self.hold_until = self.hops + HOLD_HOPS;
        }
        if self.hops >= self.hold_until {
            self.frames.clear();
            self.last_frame = None;
            self.recent_scores = [0.0; SMOOTH];
            return;
        }
        self.stats.gate_open_hops += 1;
        self.catch_up_features();
        if self.hops % SCORE_EVERY_HOPS == 0 && self.frames.len() == WINDOW_FRAMES {
            for (i, f) in self.frames.iter().enumerate() {
                self.window[i * N_MEL..(i + 1) * N_MEL].copy_from_slice(f);
            }
            normalise_window(&mut self.window);
            let p = self.net.score(&self.window, &mut self.ws);
            self.stats.windows_scored += 1;
            self.recent_scores.rotate_left(1);
            self.recent_scores[SMOOTH - 1] = p;
            let smoothed = self.recent_scores.iter().sum::<f32>() / SMOOTH as f32;
            on_score(Score { at_ms: self.now_ms(), smoothed });
        }
    }

    /// Compute the log-mel frames that are due (the first time after the gate opens, the last 1.5 s from the ring).
    fn catch_up_features(&mut self) {
        if (self.total as usize) < FRAME {
            return;
        }
        let latest = (self.total as usize - FRAME) / HOP;
        let start = match self.last_frame {
            Some(l) => l as usize + 1,
            None => latest.saturating_sub(WINDOW_FRAMES - 1),
        };
        for f in start..=latest {
            // samples [f*HOP - 1, f*HOP + FRAME) ; skip = how far the frame's end is behind the newest sample
            let end = f * HOP + FRAME;
            let skip = self.total as usize - end;
            if skip + FRAME + 1 > RING_SAMPLES {
                continue;
            }
            let buf = self.copy_back(FRAME + 1, skip);
            let v = self.lm.frame(&buf[1..], if f == 0 { 0.0 } else { buf[0] });
            self.frames.push_back(v);
            if self.frames.len() > WINDOW_FRAMES {
                self.frames.pop_front();
            }
            self.last_frame = Some(f as u64);
        }
    }
}

/// The real spotter: the scorer plus the person's threshold, the cool-down and the mute/arm rules.
pub struct Spotter {
    scorer: Scorer,
    pub settings: Settings,
    state: WakeState,
    muted: bool,
    quiet_until_ms: u64,
}

impl Spotter {
    pub fn new(net: Net, settings: Settings) -> Spotter {
        Spotter { scorer: Scorer::new(net), settings, state: WakeState::Off, muted: false, quiet_until_ms: 0 }
    }

    pub fn stats(&self) -> Stats {
        self.scorer.stats
    }

    /// The last `ms` of audio for the second-stage check, only while listening (empty otherwise). Use at once, drop, never save.
    pub fn recent_audio_for_confirm(&self, ms: usize) -> Vec<f32> {
        if self.state == WakeState::Listening { self.scorer.recent_audio(ms) } else { Vec::new() }
    }

    fn update_state(&mut self, armed: bool) {
        let new = if !armed { WakeState::Off } else if self.muted { WakeState::Muted } else { WakeState::Listening };
        if new != WakeState::Listening {
            self.scorer.wipe();
            self.quiet_until_ms = 0;
        }
        self.state = new;
    }
}

impl WakeSpotter for Spotter {
    fn state(&self) -> WakeState {
        self.state
    }
    fn arm(&mut self) {
        self.update_state(true);
    }
    fn disarm(&mut self) {
        self.update_state(false);
    }
    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        let armed = self.state != WakeState::Off;
        self.update_state(armed);
    }
    fn feed(&mut self, samples: &[f32]) -> Vec<WakeEvent> {
        if self.state != WakeState::Listening {
            return Vec::new();
        }
        let mut events = Vec::new();
        let (thr, cool) = (self.settings.threshold, self.settings.cooldown_ms);
        let quiet = &mut self.quiet_until_ms;
        let mut fired = false;
        let mut stat_wakes = 0;
        self.scorer.feed(samples, &mut |sc: Score| {
            if sc.smoothed >= thr && sc.at_ms >= *quiet {
                events.push(WakeEvent { confidence: sc.smoothed, at_ms: sc.at_ms });
                *quiet = sc.at_ms + cool;
                fired = true;
                stat_wakes += 1;
            }
        });
        self.scorer.stats.wakes += stat_wakes;
        if fired {
            // a fresh start after a wake: the next one must be a new utterance
            self.scorer.recent_scores = [0.0; SMOOTH];
        }
        events
    }
}

/// A stand-in for tests of the voice assistant and the tile: it follows the same arm/mute/indicator rules and "hears" a wake when told.
#[derive(Default)]
pub struct FakeSpotter {
    state_armed: bool,
    muted: bool,
    pending: Vec<f32>,
    pub heard_samples: usize,
}

impl FakeSpotter {
    /// The next `feed` of at least one sample returns a wake with this confidence (only if listening).
    pub fn will_hear_wake(&mut self, confidence: f32) {
        self.pending.push(confidence);
    }
}

impl WakeSpotter for FakeSpotter {
    fn state(&self) -> WakeState {
        if !self.state_armed { WakeState::Off } else if self.muted { WakeState::Muted } else { WakeState::Listening }
    }
    fn arm(&mut self) {
        self.state_armed = true;
    }
    fn disarm(&mut self) {
        self.state_armed = false;
        self.pending.clear();
    }
    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if muted {
            self.pending.clear();
        }
    }
    fn feed(&mut self, samples: &[f32]) -> Vec<WakeEvent> {
        if self.state() != WakeState::Listening || samples.is_empty() {
            return Vec::new();
        }
        self.heard_samples += samples.len();
        self.pending.drain(..).map(|c| WakeEvent { confidence: c, at_ms: 0 }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(n: usize, level: f32) -> Vec<f32> {
        let mut x = 12345u32;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                ((x >> 8) as f32 / (1 << 24) as f32 - 0.5) * 2.0 * level
            })
            .collect()
    }

    /// A network that says "phrase" when the window is loud in the top mel bins relative to the bottom (any structure will do here).
    fn always_yes() -> Net {
        let mut n = Net::zeros();
        let last = Net::param_count();
        // dense layer: output 1 bias large positive
        n.params[last - 1] = 9.0;
        n
    }

    #[test]
    fn silence_costs_no_features_and_no_network_runs() {
        let mut s = Spotter::new(always_yes(), Settings::default());
        s.arm();
        let ev = s.feed(&quiet(SAMPLE_RATE * 10, 0.0005));
        assert!(ev.is_empty());
        let st = s.stats();
        assert_eq!(st.windows_scored, 0);
        assert!(st.gate_open_hops < 30, "the gate stays shut on steady noise: {}", st.gate_open_hops);
        assert_eq!(st.hops, 1000);
    }

    #[test]
    fn a_loud_burst_opens_the_gate_scores_windows_and_fires_once_then_cools_down() {
        let mut s = Spotter::new(always_yes(), Settings { threshold: 0.8, cooldown_ms: 2500 });
        s.arm();
        let mut x = quiet(SAMPLE_RATE * 2, 0.0005);
        x.extend(quiet(SAMPLE_RATE, 0.3));
        x.extend(quiet(SAMPLE_RATE * 2, 0.0005));
        let ev = s.feed(&x);
        assert!(s.stats().windows_scored > 3);
        assert_eq!(ev.len(), 1, "one wake, the cool-down holds the rest: {ev:?}");
        assert!(ev[0].confidence > 0.99 && ev[0].at_ms >= 2000);
        // after the cool-down another burst fires again
        let mut y = quiet(SAMPLE_RATE * 3, 0.0005);
        y.extend(quiet(SAMPLE_RATE, 0.3));
        y.extend(quiet(SAMPLE_RATE, 0.0005));
        assert_eq!(s.feed(&y).len(), 1);
        assert_eq!(s.stats().wakes, 2);
    }

    #[test]
    fn a_threshold_above_the_score_never_fires() {
        let mut s = Spotter::new(Net::zeros(), Settings { threshold: 0.6, cooldown_ms: 100 });
        s.arm();
        let mut x = quiet(SAMPLE_RATE * 2, 0.0005);
        x.extend(quiet(SAMPLE_RATE * 2, 0.3));
        assert!(s.feed(&x).is_empty(), "an undecided network (0.5) stays below 0.6");
        assert!(s.stats().windows_scored > 0);
    }

    #[test]
    fn never_listens_while_off_or_muted_and_wipes_when_it_stops() {
        let mut s = Spotter::new(always_yes(), Settings::default());
        let loud = quiet(SAMPLE_RATE * 4, 0.3);
        assert_eq!(s.state(), WakeState::Off);
        assert!(!s.indicator());
        assert!(s.feed(&loud).is_empty());
        assert_eq!(s.stats().hops, 0, "off: not even looked at");
        s.arm();
        assert!(s.indicator() && s.state() == WakeState::Listening);
        s.feed(&quiet(SAMPLE_RATE, 0.2));
        assert!(!s.recent_audio_for_confirm(500).is_empty());
        s.set_muted(true);
        assert_eq!(s.state(), WakeState::Muted);
        assert!(!s.indicator(), "muted: the microphone is not open to the spotter");
        let hops = s.stats().hops;
        assert!(s.feed(&loud).is_empty());
        assert_eq!(s.stats().hops, hops, "muted: nothing processed");
        assert!(s.scorer.ring.iter().all(|v| *v == 0.0), "the ring is wiped when muted");
        assert!(s.recent_audio_for_confirm(500).is_empty());
        s.set_muted(false);
        assert!(s.indicator(), "unmuting while armed listens again");
        s.disarm();
        assert!(!s.indicator() && s.state() == WakeState::Off);
        s.set_muted(true);
        s.set_muted(false);
        assert_eq!(s.state(), WakeState::Off, "muting and unmuting never arms it");
    }

    #[test]
    fn the_ring_keeps_three_seconds_and_nothing_more() {
        let mut s = Spotter::new(Net::zeros(), Settings::default());
        s.arm();
        let x: Vec<f32> = (0..SAMPLE_RATE * 5).map(|i| (i % 1000) as f32 / 10_000.0).collect();
        s.feed(&x);
        assert_eq!(s.scorer.ring.len(), RING_SAMPLES);
        let last = s.scorer.recent_audio(1000);
        assert_eq!(last.len(), 16_000);
        assert_eq!(&last[..], &x[x.len() - 16_000..]);
        assert_eq!(s.scorer.recent_audio(10_000).len(), RING_SAMPLES, "asking for more gives the ring, not more");
    }

    #[test]
    fn the_fake_spotter_keeps_the_same_rules() {
        let mut f = FakeSpotter::default();
        f.will_hear_wake(0.9);
        assert!(f.feed(&[0.1]).is_empty(), "off: nothing");
        f.arm();
        f.will_hear_wake(0.9);
        f.set_muted(true);
        assert!(f.feed(&[0.1]).is_empty() && !f.indicator());
        f.set_muted(false);
        assert!(f.feed(&[0.1]).is_empty(), "the muted time's pending wake was dropped");
        f.will_hear_wake(0.9);
        assert_eq!(f.feed(&[0.1]).len(), 1);
        assert!(f.indicator());
    }

    #[test]
    fn the_runtime_sources_touch_no_file_and_no_network() {
        for (name, src) in [("spotter", include_str!("spotter.rs")), ("features", include_str!("features.rs")), ("model", include_str!("model.rs")), ("fft", include_str!("fft.rs"))] {
            let runtime = src.split("#[cfg(test)]").next().unwrap();
            for bad in ["std::fs", "File::", "TcpStream", "UdpSocket", "std::net", "OpenOptions", "Command::new"] {
                assert!(!runtime.contains(bad), "{name} must not use {bad}");
            }
        }
    }
}
