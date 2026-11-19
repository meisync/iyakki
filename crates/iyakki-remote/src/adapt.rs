//! Flow control and quality choice for one connection. Pure logic with the clock passed in (milliseconds), so it can be
//! tested with a simulated link.
//!
//! * **Backpressure**: at most [`WINDOW`] messages are unacknowledged. While the window is full nothing is produced, so
//!   nothing queues up; the next message is built from the newest picture (older ones are simply never sent).
//! * **Rate limit**: a minimum time between messages per level, except that a key or click lets the next message go at once.
//! * **Measurement**: the page acknowledges every message after drawing it. The time from sending to acknowledging gives
//!   the round trip; bytes over the time the link was busy give the throughput.
//! * **Level**: chosen from throughput and round trip (quickly down, slowly up), unless the person fixed one. "Data saver"
//!   keeps the level at Low or below.

use crate::codec::Level;
use std::collections::VecDeque;

pub const WINDOW: usize = 2;
const MIN_SAMPLE: usize = 3000;
const UP_AFTER_MS: u64 = 4000;
/// A message not acknowledged for this long means the link is stalling: go down a level.
const STALL_MS: u64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Fixed(Level),
}

struct Sent {
    seq: u32,
    at: u64,
    bytes: usize,
}

pub struct Controller {
    choice: Choice,
    saver: bool,
    auto_level: Level,
    in_flight: VecDeque<Sent>,
    last_send: Option<u64>,
    last_ack: Option<u64>,
    boost: bool,
    /// Bytes per second the link carries (smoothed), `None` until a message big enough has been acknowledged.
    bps: Option<f64>,
    rtt_ms: Option<f64>,
    rtt_min: Option<f64>,
    /// Round trip as the page measured it with its own pings (no queueing of pictures in it).
    rtt_ping: Option<f64>,
    better_since: Option<u64>,
    window_start: u64,
    window_bytes: usize,
    window_frames: usize,
    pub sent_bps: f64,
    pub sent_fps: f64,
}

impl Controller {
    pub fn new(now: u64) -> Controller {
        Controller {
            choice: Choice::Auto,
            saver: false,
            auto_level: Level::Balanced,
            in_flight: VecDeque::new(),
            last_send: None,
            last_ack: None,
            boost: false,
            bps: None,
            rtt_ms: None,
            rtt_min: None,
            rtt_ping: None,
            better_since: None,
            window_start: now,
            window_bytes: 0,
            window_frames: 0,
            sent_bps: 0.0,
            sent_fps: 0.0,
        }
    }

    pub fn set_choice(&mut self, c: Choice, saver: bool) {
        self.choice = c;
        self.saver = saver;
    }

    pub fn choice(&self) -> Choice {
        self.choice
    }

    pub fn saver(&self) -> bool {
        self.saver
    }

    /// The level in force now.
    pub fn level(&self) -> Level {
        let l = match self.choice {
            Choice::Fixed(l) => l,
            Choice::Auto => self.auto_level,
        };
        if self.saver {
            l.max(Level::Low)
        } else {
            l
        }
    }

    pub fn bps(&self) -> Option<f64> {
        self.bps
    }

    /// A round trip the page measured (ping to pong), in milliseconds; absurd values are ignored.
    pub fn on_rtt(&mut self, ms: f64) {
        if ms.is_finite() && (0.0..=60_000.0).contains(&ms) {
            self.rtt_ping = Some(self.rtt_ping.map_or(ms, |r| r * 0.6 + ms * 0.4));
        }
    }

    pub fn rtt_ms(&self) -> Option<f64> {
        self.rtt_ms
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// A key, click or scroll arrived: the next message may go at once.
    pub fn on_input(&mut self) {
        self.boost = true;
    }

    /// May a message be produced and sent now?
    pub fn can_send(&self, now: u64) -> bool {
        if self.in_flight.len() >= WINDOW {
            return false;
        }
        let gap = if self.boost { 8 } else { self.level().min_interval_ms() };
        self.last_send.is_none_or(|t| now.saturating_sub(t) >= gap)
    }

    /// The most bytes of tiles one message should carry: about a quarter second of the link, within 16 KiB and 1 MiB.
    pub fn budget(&self) -> usize {
        match self.bps {
            Some(b) => ((b * 0.25) as usize).clamp(16 * 1024, 1024 * 1024),
            None => 96 * 1024,
        }
    }

    pub fn on_send(&mut self, seq: u32, bytes: usize, now: u64) {
        self.in_flight.push_back(Sent { seq, at: now, bytes });
        self.last_send = Some(now);
        self.boost = false;
        self.window_bytes += bytes;
        self.window_frames += 1;
        if now.saturating_sub(self.window_start) >= 1000 {
            let secs = (now - self.window_start) as f64 / 1000.0;
            self.sent_bps = self.window_bytes as f64 / secs;
            self.sent_fps = self.window_frames as f64 / secs;
            self.window_start = now;
            self.window_bytes = 0;
            self.window_frames = 0;
        }
    }

    /// The page drew message `seq` (acknowledging a later one covers earlier ones; an unknown number is ignored).
    pub fn on_ack(&mut self, seq: u32, now: u64) {
        let Some(pos) = self.in_flight.iter().position(|s| s.seq == seq) else { return };
        for _ in 0..=pos {
            let s = self.in_flight.pop_front().expect("position found");
            let rtt = now.saturating_sub(s.at).max(1) as f64;
            self.rtt_ms = Some(self.rtt_ms.map_or(rtt, |r| r * 0.7 + rtt * 0.3));
            // a small message tells the round trip but not the speed; the best round trip seen is the link's, not the queue's
            if s.bytes < MIN_SAMPLE {
                self.rtt_min = Some(self.rtt_min.map_or(rtt, |m| m.min(rtt)));
            } else {
                let busy_from = self.last_ack.map_or(s.at, |a| a.max(s.at));
                // a message sent on an idle link also spent one round trip in flight, which is not transfer time
                let base = if busy_from == s.at { self.rtt_ping.or(self.rtt_min).unwrap_or(0.0) } else { 0.0 };
                let secs = (now.saturating_sub(busy_from) as f64 - base).max(4.0) / 1000.0;
                let sample = s.bytes as f64 / secs;
                self.bps = Some(self.bps.map_or(sample, |b| b * 0.6 + sample * 0.4));
            }
            self.last_ack = Some(now);
        }
        self.reevaluate(now);
    }

    /// Call every now and then (also when nothing was acknowledged): notices stalls, lets the level climb.
    pub fn tick(&mut self, now: u64) {
        if let Some(oldest) = self.in_flight.front() {
            if now.saturating_sub(oldest.at) > STALL_MS && self.auto_level != Level::VeryLow {
                self.auto_level = self.auto_level.lower();
                self.better_since = None;
                // do not judge the next one by the time spent stalling
                for s in self.in_flight.iter_mut() {
                    s.at = now;
                }
            }
        }
        self.reevaluate(now);
    }

    /// The level the measurements support.
    fn target(&self) -> Option<Level> {
        let bps = self.bps?;
        let rtt = self.rtt_ping.or(self.rtt_min).unwrap_or(0.0);
        let by_speed = if bps >= 2_000_000.0 {
            Level::Lossless
        } else if bps >= 375_000.0 {
            Level::Balanced
        } else if bps >= 100_000.0 {
            Level::Low
        } else {
            Level::VeryLow
        };
        // a slow round trip makes everything feel slow whatever the speed: send less
        let by_rtt = if rtt > 600.0 {
            Level::Low
        } else if rtt > 250.0 {
            Level::Balanced
        } else {
            Level::Lossless
        };
        Some(by_speed.max(by_rtt))
    }

    fn reevaluate(&mut self, now: u64) {
        let Some(t) = self.target() else { return };
        if t > self.auto_level {
            self.auto_level = t; // worse link: go down at once
            self.better_since = None;
        } else if t < self.auto_level {
            // better link: one step up after it has been better for a while
            match self.better_since {
                None => self.better_since = Some(now),
                Some(s) if now.saturating_sub(s) >= UP_AFTER_MS => {
                    self.auto_level = self.auto_level.higher();
                    self.better_since = None;
                }
                _ => {}
            }
        } else {
            self.better_since = None;
        }
    }

    /// The state for the page's statistics overlay.
    pub fn stats_json(&self) -> serde_json::Value {
        serde_json::json!({
            "t": "stats",
            "level": self.level().name(),
            "auto": self.choice == Choice::Auto,
            "saver": self.saver,
            "bps": self.bps.map(|b| b as u64),
            "rtt": self.rtt_ping.or(self.rtt_ms).map(|r| r as u64),
            "sent_bps": self.sent_bps as u64,
            "fps": (self.sent_fps * 10.0).round() / 10.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simulated link: `rate` bytes per second, `delay` ms each way. Returns when the page would acknowledge a message
    /// of `bytes` sent at `now` after the previous one finished.
    struct Link {
        rate: f64,
        delay: u64,
        free_at: u64,
    }

    impl Link {
        fn ack_time(&mut self, now: u64, bytes: usize) -> u64 {
            let start = self.free_at.max(now + self.delay);
            let done = start + (bytes as f64 / self.rate * 1000.0) as u64;
            self.free_at = done;
            done + self.delay
        }
    }

    /// Run the controller on a link for `secs` seconds with a source that always has `frame_bytes` to send. Returns
    /// (messages sent, level at the end, bytes sent).
    fn run(link: &mut Link, c: &mut Controller, secs: u64, frame_bytes: impl Fn(Level) -> usize) -> (usize, Level, usize) {
        let mut acks: Vec<(u64, u32)> = Vec::new();
        let (mut seq, mut sent, mut total) = (0u32, 0usize, 0usize);
        for now in 0..secs * 1000 {
            acks.retain(|&(t, s)| {
                if t <= now {
                    c.on_ack(s, now);
                    false
                } else {
                    true
                }
            });
            if now % 50 == 0 {
                c.tick(now);
            }
            if now % 1000 == 0 {
                c.on_rtt(2.0 * link.delay as f64);
            }
            if c.can_send(now) {
                seq += 1;
                let b = frame_bytes(c.level());
                acks.push((link.ack_time(now, b), seq));
                c.on_send(seq, b, now);
                sent += 1;
                total += b;
            }
        }
        for (t, s) in acks {
            c.on_ack(s, t.max(secs * 1000));
        }
        (sent, c.level(), total)
    }

    #[test]
    fn a_fast_link_keeps_the_best_level_and_a_slow_one_goes_down_without_queueing() {
        let size = |l: Level| [60_000, 30_000, 12_000, 4_000][l as usize];
        let mut c = Controller::new(0);
        let mut fast = Link { rate: 10_000_000.0, delay: 5, free_at: 0 };
        let (_, lvl, _) = run(&mut fast, &mut c, 30, size);
        assert_eq!(lvl, Level::Lossless, "plenty of bandwidth: full quality");

        let mut c = Controller::new(0);
        let mut slow = Link { rate: 40_000.0, delay: 150, free_at: 0 };
        let (sent, lvl, total) = run(&mut slow, &mut c, 30, size);
        assert_eq!(lvl, Level::VeryLow, "40 kB/s is a very low link");
        assert!(c.in_flight() <= WINDOW);
        // the sender never ran ahead of the link: bytes sent stay within what 30 s of the link can carry (plus the window)
        assert!(total as f64 <= 40_000.0 * 30.0 * 1.2 + (WINDOW * 60_000) as f64, "{total} bytes at {sent} messages");
    }

    #[test]
    fn the_level_climbs_back_slowly_when_the_link_recovers() {
        let size = |l: Level| [60_000, 30_000, 12_000, 4_000][l as usize];
        let mut c = Controller::new(0);
        let mut slow = Link { rate: 40_000.0, delay: 100, free_at: 0 };
        run(&mut slow, &mut c, 10, size);
        assert_eq!(c.level(), Level::VeryLow);
        // the link gets fast: it must not jump straight back up, but should get there within a minute
        let mut fast = Link { rate: 20_000_000.0, delay: 5, free_at: 0 };
        let mut now0 = 10_000u64;
        let mut seen = vec![c.level()];
        let mut acks: Vec<(u64, u32)> = Vec::new();
        for _ in 0..40 {
            for now in now0..now0 + 1000 {
                acks.retain(|&(t, s)| {
                    if t <= now {
                        c.on_ack(s, now);
                        false
                    } else {
                        true
                    }
                });
                if now % 50 == 0 {
                    c.tick(now);
                }
                if now % 1000 == 0 {
                    c.on_rtt(10.0);
                }
                if c.can_send(now) {
                    let b = size(c.level());
                    acks.push((fast.ack_time(now, b), (now + 1) as u32));
                    c.on_send((now + 1) as u32, b, now);
                }
            }
            now0 += 1000;
            if seen.last() != Some(&c.level()) {
                seen.push(c.level());
            }
        }
        assert!(seen.len() >= 3, "climbs one step at a time: {seen:?}");
        assert!(seen.windows(2).all(|w| w[1] < w[0]), "{seen:?}");
    }

    #[test]
    fn a_stalled_link_steps_down_and_data_saver_and_a_fixed_level_are_obeyed() {
        let mut c = Controller::new(0);
        c.on_send(1, 50_000, 0);
        c.tick(2500);
        assert_eq!(c.level(), Level::Low, "no acknowledgement for 2.5 s: one step down from Balanced");
        c.set_choice(Choice::Fixed(Level::Lossless), false);
        assert_eq!(c.level(), Level::Lossless);
        c.set_choice(Choice::Fixed(Level::Lossless), true);
        assert_eq!(c.level(), Level::Low, "data saver caps the level");
        c.set_choice(Choice::Fixed(Level::VeryLow), true);
        assert_eq!(c.level(), Level::VeryLow, "saver never raises it");
    }

    #[test]
    fn the_window_and_the_rate_limit_hold_messages_back_but_a_key_goes_through() {
        let mut c = Controller::new(0);
        c.set_choice(Choice::Fixed(Level::Low), false); // one message per 100 ms
        assert!(c.can_send(0));
        c.on_send(1, 1000, 0);
        assert!(!c.can_send(50), "too soon");
        assert!(c.can_send(100));
        c.on_input();
        assert!(!c.can_send(4), "even a key waits 8 ms");
        assert!(c.can_send(10), "a key does not wait for the rate limit");
        c.on_send(2, 1000, 10);
        assert!(!c.can_send(500), "two unacknowledged messages fill the window");
        c.on_ack(1, 520);
        assert!(c.can_send(521));
        c.on_ack(99, 600); // unknown numbers are ignored
        assert_eq!(c.in_flight(), 1);
        c.on_ack(2, 700);
        assert_eq!(c.in_flight(), 0);
        assert!(c.budget() >= 16 * 1024);
    }
}
