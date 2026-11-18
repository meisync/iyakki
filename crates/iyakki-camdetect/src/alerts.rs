//! When a motion event may become a notice in the inbox. Pure code: the caller gives the time, the local minute of the day and a clock
//! formatter, and sends what comes back.
//!
//! No spam, by construction:
//!
//! * **At most `max_per_hour` notices per camera in any rolling hour** (default 4, never more than 30). When the limit is reached
//!   further events are still stored in the event list but make no notice; the next notice that is sent says how many were held back.
//! * **Bursts are grouped.** The first event sends one notice right away and opens a burst of `group_s` seconds (default 10 minutes).
//!   Events inside the burst send nothing; when the burst ends ONE grouped notice says how many more there were and between when and
//!   when (it counts against the hourly limit too).
//! * **Quiet hours** (for example `22:00-06:30`, may wrap midnight): nothing is sent; when they end ONE notice says how many events
//!   happened meanwhile.
//! * **A per-camera switch** (off by default). The thumbnail travels with a notice only when the person allowed it.
//!
//! The gate only decides. It does not look at pictures and does not send anything itself.

use std::collections::VecDeque;

pub const DEFAULT_PER_HOUR: u32 = 4;
pub const MAX_PER_HOUR: u32 = 30;
pub const DEFAULT_GROUP_S: u64 = 600;
const HOUR_MS: u64 = 3_600_000;

/// A daily window in minutes after midnight; `from > to` wraps over midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quiet {
    pub from_min: u16,
    pub to_min: u16,
}

impl Quiet {
    /// `22:00-06:30`.
    pub fn parse(t: &str) -> Result<Quiet, String> {
        let hm = |s: &str| -> Option<u16> {
            let (h, m) = s.trim().split_once(':')?;
            let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
            (h < 24 && m < 60).then_some(h * 60 + m)
        };
        let (a, b) = t.split_once('-').ok_or("quiet hours are written like 22:00-06:30")?;
        match (hm(a), hm(b)) {
            (Some(from_min), Some(to_min)) if from_min != to_min => Ok(Quiet { from_min, to_min }),
            (Some(_), Some(_)) => Err("quiet hours must start and end at different times".into()),
            _ => Err("quiet hours are written like 22:00-06:30 (24 hour clock)".into()),
        }
    }

    pub fn text(&self) -> String {
        format!("{:02}:{:02}-{:02}:{:02}", self.from_min / 60, self.from_min % 60, self.to_min / 60, self.to_min % 60)
    }

    pub fn contains(&self, minute: u16) -> bool {
        if self.from_min < self.to_min {
            minute >= self.from_min && minute < self.to_min
        } else {
            minute >= self.from_min || minute < self.to_min
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertCfg {
    pub enabled: bool,
    pub max_per_hour: u32,
    pub quiet: Option<Quiet>,
    /// The small picture may travel with a notice (to channels that can carry one).
    pub thumbnail: bool,
    pub group_s: u64,
}

impl Default for AlertCfg {
    fn default() -> AlertCfg {
        AlertCfg { enabled: false, max_per_hour: DEFAULT_PER_HOUR, quiet: None, thumbnail: false, group_s: DEFAULT_GROUP_S }
    }
}

/// One notice to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// How many events this notice stands for.
    pub count: u32,
    pub grouped: bool,
    /// Start time of the (first) event, for the link back to it.
    pub event_ms: u64,
    /// The thumbnail may be attached (the person allowed it, and this notice is about one event).
    pub thumbnail: bool,
}

struct Burst {
    end_ms: u64,
    first_ms: u64,
    last_ms: u64,
    count: u32,
}

#[derive(Default)]
pub struct Gate {
    sent: VecDeque<u64>,
    burst: Option<Burst>,
    quiet_held: Vec<u64>,
    held_back: u32,
    /// Events that made no notice because of the hourly limit (all time, for the status line).
    pub suppressed: u64,
}

impl Gate {
    pub fn new() -> Gate {
        Gate::default()
    }

    fn slot(&mut self, cfg: &AlertCfg, now: u64) -> bool {
        while self.sent.front().is_some_and(|t| now.saturating_sub(*t) >= HOUR_MS) {
            self.sent.pop_front();
        }
        (self.sent.len() as u32) < cfg.max_per_hour.clamp(1, MAX_PER_HOUR)
    }

    fn held_note(&mut self) -> String {
        if self.held_back == 0 {
            return String::new();
        }
        let n = std::mem::take(&mut self.held_back);
        format!(" {n} more {} held back to keep the number of notices down.", if n == 1 { "event was" } else { "events were" })
    }

    /// Notices that are due without a new event: a finished burst, the end of the quiet hours.
    pub fn tick(&mut self, cfg: &AlertCfg, now: u64, minute: u16, camera: &str, fmt: &dyn Fn(u64) -> String) -> Vec<Notice> {
        let mut out = Vec::new();
        if !cfg.enabled {
            self.burst = None;
            self.quiet_held.clear();
            return out;
        }
        if self.burst.as_ref().is_some_and(|b| now >= b.end_ms) {
            if let Some(b) = self.burst.take() {
                if b.count > 0 {
                    if self.slot(cfg, now) {
                        self.sent.push_back(now);
                        let note = self.held_note();
                        out.push(Notice {
                            title: format!("{} more motion {} at {camera}", b.count, if b.count == 1 { "event" } else { "events" }),
                            body: format!("Between {} and {}.{note} Open CAMERAS to see them.", fmt(b.first_ms), fmt(b.last_ms)),
                            count: b.count,
                            grouped: true,
                            event_ms: b.first_ms,
                            thumbnail: false,
                        });
                    } else {
                        self.suppressed += b.count as u64;
                        self.held_back += b.count;
                    }
                }
            }
        }
        if !self.quiet_held.is_empty() && !cfg.quiet.is_some_and(|q| q.contains(minute)) && self.slot(cfg, now) {
            let held = std::mem::take(&mut self.quiet_held);
            self.sent.push_back(now);
            let note = self.held_note();
            let n = held.len() as u32;
            out.push(Notice {
                title: format!("{n} motion {} at {camera} during quiet hours", if n == 1 { "event" } else { "events" }),
                body: format!("Between {} and {}.{note} Open CAMERAS to see them.", fmt(held[0]), fmt(*held.last().unwrap_or(&held[0]))),
                count: n,
                grouped: true,
                event_ms: held[0],
                thumbnail: false,
            });
        }
        out
    }

    /// A new event started at `start_ms` (the notice is decided now, while it is news).
    #[allow(clippy::too_many_arguments)]
    pub fn event(&mut self, cfg: &AlertCfg, now: u64, minute: u16, camera: &str, zone: &str, start_ms: u64, person: Option<bool>, fmt: &dyn Fn(u64) -> String) -> Vec<Notice> {
        let mut out = self.tick(cfg, now, minute, camera, fmt);
        if !cfg.enabled {
            return out;
        }
        if cfg.quiet.is_some_and(|q| q.contains(minute)) {
            self.quiet_held.push(start_ms);
            if self.quiet_held.len() > 1000 {
                self.quiet_held.remove(0);
            }
            return out;
        }
        if let Some(b) = self.burst.as_mut() {
            if b.count == 0 {
                b.first_ms = start_ms;
            }
            b.count += 1;
            b.last_ms = start_ms;
            return out;
        }
        let group_ms = cfg.group_s.clamp(10, 86_400) * 1000;
        if self.slot(cfg, now) {
            self.sent.push_back(now);
            let note = self.held_note();
            let what = if person == Some(true) { "Person" } else { "Motion" };
            let place = if zone.is_empty() || zone == super::motion::WHOLE { String::new() } else { format!(" ({zone})") };
            out.push(Notice { title: format!("{what} at {camera}"), body: format!("{}{place}.{note} Open CAMERAS to see it.", fmt(start_ms)), count: 1, grouped: false, event_ms: start_ms, thumbnail: cfg.thumbnail });
            self.burst = Some(Burst { end_ms: now + group_ms, first_ms: start_ms, last_ms: start_ms, count: 0 });
        } else {
            self.suppressed += 1;
            self.held_back += 1;
            // the burst still opens so a long run of events does not try again at every event
            self.burst = Some(Burst { end_ms: now + group_ms, first_ms: start_ms, last_ms: start_ms, count: 0 });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(ms: u64) -> String {
        format!("t{}", ms / 1000)
    }
    fn on() -> AlertCfg {
        AlertCfg { enabled: true, ..Default::default() }
    }
    const MIN: u64 = 60_000;

    #[test]
    fn nothing_is_sent_unless_the_switch_is_on() {
        let mut g = Gate::new();
        assert!(g.event(&AlertCfg::default(), 1000, 600, "Door", "", 1000, None, &fmt).is_empty());
        assert!(g.tick(&AlertCfg::default(), 99_000_000, 600, "Door", &fmt).is_empty());
    }

    #[test]
    fn the_first_event_sends_and_a_burst_becomes_one_grouped_notice() {
        let mut g = Gate::new();
        let cfg = on();
        let first = g.event(&cfg, 0, 600, "Door", "Porch", 0, None, &fmt);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].title, "Motion at Door");
        assert!(first[0].body.contains("(Porch)") && !first[0].grouped && !first[0].thumbnail);
        // 7 more inside ten minutes: silent
        for k in 1..=7u64 {
            assert!(g.event(&cfg, k * MIN, 600, "Door", "Porch", k * MIN, None, &fmt).is_empty());
        }
        assert!(g.tick(&cfg, 9 * MIN, 600, "Door", &fmt).is_empty(), "the burst is still open");
        let grouped = g.tick(&cfg, 10 * MIN, 600, "Door", &fmt);
        assert_eq!(grouped.len(), 1);
        assert_eq!((grouped[0].count, grouped[0].grouped), (7, true));
        assert_eq!(grouped[0].title, "7 more motion events at Door");
        assert!(grouped[0].body.contains("t60") && grouped[0].body.contains("t420"));
        assert!(g.tick(&cfg, 11 * MIN, 600, "Door", &fmt).is_empty(), "and it does not repeat");
    }

    #[test]
    fn the_hourly_limit_holds_and_the_next_notice_says_so() {
        let mut g = Gate::new();
        let cfg = AlertCfg { enabled: true, max_per_hour: 2, group_s: 10, ..Default::default() };
        let mut sent = 0;
        // one event a minute, each after the 10 s burst of the one before: 6 events in 6 minutes
        for k in 0..6u64 {
            sent += g.event(&cfg, k * MIN, 600, "Door", "", k * MIN, None, &fmt).len();
        }
        assert_eq!(sent, 2, "no more than 2 in the hour");
        assert_eq!(g.suppressed, 4);
        // an hour after the first one a slot is free: the next notice mentions what was held back
        let n = g.event(&cfg, 61 * MIN, 600, "Door", "", 61 * MIN, None, &fmt);
        assert_eq!(n.len(), 1);
        assert!(n[0].body.contains("4 more events were held back"), "{}", n[0].body);
    }

    #[test]
    fn quiet_hours_send_nothing_and_one_summary_when_they_end() {
        let mut g = Gate::new();
        let cfg = AlertCfg { enabled: true, quiet: Some(Quiet::parse("22:00-06:30").unwrap()), ..Default::default() };
        for k in 0..5u64 {
            assert!(g.event(&cfg, k * MIN, 23 * 60 + 10, "Door", "", k * MIN, None, &fmt).is_empty());
        }
        assert!(g.tick(&cfg, 6 * MIN, 2 * 60, "Door", &fmt).is_empty(), "still quiet at 02:00");
        let end = g.tick(&cfg, 7 * MIN, 6 * 60 + 30, "Door", &fmt);
        assert_eq!(end.len(), 1);
        assert_eq!((end[0].count, end[0].title.as_str()), (5, "5 motion events at Door during quiet hours"));
        assert!(g.tick(&cfg, 8 * MIN, 7 * 60, "Door", &fmt).is_empty());
        // an event after the quiet hours sends as usual
        assert_eq!(g.event(&cfg, 100 * MIN, 8 * 60, "Door", "", 100 * MIN, None, &fmt).len(), 1);
    }

    #[test]
    fn the_quiet_window_parses_wraps_and_refuses_nonsense() {
        let q = Quiet::parse("22:00-06:30").unwrap();
        assert!(q.contains(23 * 60) && q.contains(0) && q.contains(6 * 60 + 29));
        assert!(!q.contains(6 * 60 + 30) && !q.contains(12 * 60) && !q.contains(21 * 60 + 59));
        let day = Quiet::parse("09:00-17:00").unwrap();
        assert!(day.contains(10 * 60) && !day.contains(8 * 60) && !day.contains(17 * 60));
        assert_eq!(q.text(), "22:00-06:30");
        for bad in ["", "22:00", "25:00-06:00", "22:00-22:00", "a-b", "22:61-01:00"] {
            assert!(Quiet::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_thumbnail_travels_only_when_allowed_and_a_person_is_named_as_one() {
        let mut g = Gate::new();
        let n = g.event(&AlertCfg { thumbnail: true, ..on() }, 0, 600, "Door", "", 0, Some(true), &fmt);
        assert!(n[0].thumbnail && n[0].title == "Person at Door");
        let mut g = Gate::new();
        let n = g.event(&on(), 0, 600, "Door", "", 0, Some(false), &fmt);
        assert!(!n[0].thumbnail && n[0].title == "Motion at Door");
    }
}
