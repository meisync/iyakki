//! Time: cron expressions, intervals, one-shots, missed-run and overlap policies, the persistent scheduler state, and the next-run
//! preview. Pure: the clock is a number the caller passes in, so tests use a fake one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::retry::hash_parts;

// ------------------------------------------------------------------------------------------------ civil time

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// A time zone: the offset from UTC (seconds, east positive) at an instant.
pub trait Tz: Send + Sync {
    fn offset_secs(&self, unix: i64) -> i32;
}

pub struct FixedTz(pub i32);

impl Tz for FixedTz {
    fn offset_secs(&self, _unix: i64) -> i32 {
        self.0
    }
}

pub type TzResolver = Arc<dyn Fn(&str) -> Option<Arc<dyn Tz>> + Send + Sync>;

/// `UTC`, `Z`, `local` (UTC unless the host supplies its own resolver) and `+05:30` / `-0800`.
pub fn basic_tz(name: &str) -> Option<Arc<dyn Tz>> {
    let n = name.trim();
    if n.is_empty() || n.eq_ignore_ascii_case("utc") || n.eq_ignore_ascii_case("z") || n.eq_ignore_ascii_case("local") || n.eq_ignore_ascii_case("gmt") {
        return Some(Arc::new(FixedTz(0)));
    }
    let (sign, rest) = match n.as_bytes().first()? {
        b'+' => (1, &n[1..]),
        b'-' => (-1, &n[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 && digits.len() != 2 {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = if digits.len() == 4 { digits[2..].parse().ok()? } else { 0 };
    if h > 14 || m > 59 {
        return None;
    }
    Some(Arc::new(FixedTz(sign * (h * 3600 + m * 60))))
}

pub fn default_resolver() -> TzResolver {
    Arc::new(basic_tz)
}

/// The instant of a local civil time (the earlier one when it happens twice; the next valid one when it does not exist).
pub fn local_to_unix(tz: &dyn Tz, y: i64, m: u32, d: u32, hh: u32, mm: u32) -> i64 {
    let civil = days_from_civil(y, m, d) * 86400 + hh as i64 * 3600 + mm as i64 * 60;
    let o1 = tz.offset_secs(civil) as i64;
    let guess = civil - o1;
    let o2 = tz.offset_secs(guess) as i64;
    if o1 == o2 {
        guess
    } else {
        let g2 = civil - o2;
        if tz.offset_secs(g2) as i64 == o2 {
            g2
        } else {
            guess.max(g2)
        }
    }
}

// ------------------------------------------------------------------------------------------------ cron

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    dom: u32,
    months: u16,
    dow: u8,
    dom_star: bool,
    dow_star: bool,
}

fn field(s: &str, lo: u32, hi: u32, names: &[&str], name_base: u32) -> Result<(u64, bool), String> {
    let mut mask = 0u64;
    let mut star = false;
    for part in s.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, st)) => (r, st.parse::<u32>().map_err(|_| format!("`{st}` is not a step"))?),
            None => (part, 1),
        };
        if step == 0 {
            return Err("a step of 0".into());
        }
        let val = |t: &str| -> Result<u32, String> {
            if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(t)) {
                return Ok(i as u32 + name_base);
            }
            t.parse::<u32>().map_err(|_| format!("`{t}` is not a valid value"))
        };
        let (a, b) = if range == "*" || range == "?" {
            if step == 1 {
                star = true;
            }
            (lo, hi)
        } else if let Some((x, y)) = range.split_once('-') {
            (val(x)?, val(y)?)
        } else {
            let v = val(range)?;
            if part.contains('/') {
                (v, hi)
            } else {
                (v, v)
            }
        };
        let (a, b) = if hi == 7 && b == 7 && a == 7 { (0, 0) } else { (a, b) };
        if a < lo || b > hi || a > b {
            return Err(format!("`{part}` is outside {lo}-{hi}"));
        }
        let mut v = a;
        while v <= b {
            mask |= 1 << (if hi == 7 { v % 7 } else { v });
            v += step;
        }
    }
    Ok((mask, star))
}

impl Cron {
    pub fn parse(expr: &str) -> Result<Cron, String> {
        let e = expr.trim();
        let e = match e {
            "@yearly" | "@annually" => "0 0 1 1 *",
            "@monthly" => "0 0 1 * *",
            "@weekly" => "0 0 * * 0",
            "@daily" | "@midnight" => "0 0 * * *",
            "@hourly" => "0 * * * *",
            other => other,
        };
        let f: Vec<&str> = e.split_whitespace().collect();
        if f.len() != 5 {
            return Err(format!("a cron expression has five fields (minute hour day month weekday), this has {}", f.len()));
        }
        let (minutes, _) = field(f[0], 0, 59, &[], 0).map_err(|m| format!("minute: {m}"))?;
        let (hours, _) = field(f[1], 0, 23, &[], 0).map_err(|m| format!("hour: {m}"))?;
        let (dom, dom_star) = field(f[2], 1, 31, &[], 0).map_err(|m| format!("day of month: {m}"))?;
        let (months, _) = field(f[3], 1, 12, &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"], 1).map_err(|m| format!("month: {m}"))?;
        let (dow, dow_star) = field(f[4], 0, 7, &["sun", "mon", "tue", "wed", "thu", "fri", "sat"], 0).map_err(|m| format!("weekday: {m}"))?;
        Ok(Cron { minutes, hours: hours as u32, dom: dom as u32, months: months as u16, dow: dow as u8, dom_star, dow_star })
    }

    fn day_matches(&self, y: i64, m: u32, d: u32) -> bool {
        if self.months & (1 << m) == 0 {
            return false;
        }
        let wd = weekday(days_from_civil(y, m, d));
        let dom_ok = self.dom & (1 << d) != 0;
        let dow_ok = self.dow & (1 << wd) != 0;
        match (self.dom_star, self.dow_star) {
            (true, true) => true,
            (false, true) => dom_ok,
            (true, false) => dow_ok,
            (false, false) => dom_ok || dow_ok,
        }
    }

    /// The first matching instant strictly after `after` (unix seconds) in `tz`; None if none in the next 8 years.
    pub fn next_after(&self, after: i64, tz: &dyn Tz) -> Option<i64> {
        let local_day = (after + tz.offset_secs(after) as i64).div_euclid(86400);
        for dd in 0..(366 * 8) {
            let (y, m, d) = civil_from_days(local_day + dd);
            if !self.day_matches(y, m, d) {
                continue;
            }
            for h in 0..24u32 {
                if self.hours & (1 << h) == 0 {
                    continue;
                }
                for mi in 0..60u32 {
                    if self.minutes & (1 << mi) == 0 {
                        continue;
                    }
                    let t = local_to_unix(tz, y, m, d, h, mi);
                    if t > after {
                        return Some(t);
                    }
                }
            }
        }
        None
    }
}

// ------------------------------------------------------------------------------------------------ the scheduler

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Entry {
    #[serde(default)]
    pub next_ms: Option<i64>,
    #[serde(default)]
    pub last_fire_ms: i64,
    #[serde(default)]
    pub last_due_ms: i64,
    #[serde(default)]
    pub fired: u64,
    #[serde(default)]
    pub skipped: u64,
    #[serde(default)]
    pub seen: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct SchedState {
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
    #[serde(default)]
    pub paused_flows: Vec<String>,
}

/// One run the scheduler wants started.
#[derive(Clone, Debug, PartialEq)]
pub struct Fire {
    pub flow: String,
    pub trigger: String,
    /// The tick this is for.
    pub due_ms: i64,
    /// When to start (the tick plus its jitter).
    pub at_ms: i64,
    /// A tick that passed while nothing was watching.
    pub missed: bool,
    pub overlap: Overlap,
    pub inputs: serde_json::Value,
}

/// What the scheduler needs to know about the world at a tick.
pub struct TickEnv<'a> {
    /// A run of this flow is queued or going.
    pub busy: &'a dyn Fn(&str) -> bool,
    /// A fingerprint of a file or folder (None: not there).
    pub fingerprint: &'a dyn Fn(&str) -> Option<String>,
    /// Calendar event starts in `(since_ms, now_ms]` whose title contains the text.
    pub calendar: &'a dyn Fn(&str, i64, i64) -> Vec<i64>,
}

pub struct Scheduler {
    pub state: SchedState,
    path: Option<PathBuf>,
    tz: TzResolver,
    dirty: bool,
    last_tick_ms: i64,
}

/// A tick within this long of its time is "on time".
pub const GRACE_MS: i64 = 90_000;

fn key(flow: &str, trigger: &str) -> String {
    format!("{flow}/{trigger}")
}

impl Scheduler {
    pub fn new(path: Option<PathBuf>, tz: TzResolver) -> Scheduler {
        let state = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Scheduler { state, path, tz, dirty: false, last_tick_ms: 0 }
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(p) = &self.path {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, serde_json::to_string_pretty(&self.state).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
        self.dirty = false;
    }

    pub fn pause_flow(&mut self, flow: &str) {
        if !self.state.paused_flows.iter().any(|f| f == flow) {
            self.state.paused_flows.push(flow.to_string());
            self.dirty = true;
        }
    }

    pub fn resume_flow(&mut self, flow: &str) {
        self.state.paused_flows.retain(|f| f != flow);
        self.dirty = true;
    }

    pub fn is_paused(&self, flow: &str) -> bool {
        self.state.paused_flows.iter().any(|f| f == flow)
    }

    pub fn entry(&self, flow: &str, trigger: &str) -> Option<&Entry> {
        self.state.entries.get(&key(flow, trigger))
    }

    fn tz_of(&self, name: &str) -> Arc<dyn Tz> {
        (self.tz)(name).unwrap_or_else(|| Arc::new(FixedTz(0)))
    }

    /// The tick after `after_ms` for a timed trigger.
    pub fn next_tick(&self, t: &Trigger, after_ms: i64) -> Option<i64> {
        match &t.kind {
            TriggerKind::Cron { expr, tz } => Cron::parse(expr).ok()?.next_after(after_ms.div_euclid(1000), self.tz_of(tz).as_ref()).map(|s| s * 1000),
            TriggerKind::Interval { every_s, anchor_ms } => {
                let every = (*every_s).max(1) as i64 * 1000;
                let anchor = anchor_ms.unwrap_or(0);
                let n = (after_ms - anchor).div_euclid(every) + 1;
                Some(anchor + n * every)
            }
            TriggerKind::Once { at_ms } => (*at_ms > after_ms).then_some(*at_ms),
            _ => None,
        }
    }

    /// The next `n` fire times of every enabled timed trigger of a flow from `from_ms`, soonest first: `(trigger id, ms)`.
    pub fn next_runs(&self, flow: &Flow, from_ms: i64, n: usize) -> Vec<(String, i64)> {
        let mut all = Vec::new();
        for t in flow.triggers.iter().filter(|t| t.enabled && t.kind.is_timed()) {
            let mut at = from_ms;
            for _ in 0..n {
                match self.next_tick(t, at) {
                    Some(x) => {
                        all.push((t.id.clone(), x));
                        at = x;
                    }
                    None => break,
                }
            }
        }
        all.sort_by_key(|x| x.1);
        all.truncate(n);
        all
    }

    fn jitter(&self, t: &Trigger, flow: &str, due: i64) -> i64 {
        if t.jitter_s == 0 {
            return 0;
        }
        (hash_parts(&[flow, &t.id, &due.to_string()]) % (t.jitter_s * 1000)) as i64
    }

    /// What to start now. Call every few seconds (and once at start-up, which handles ticks missed while off).
    pub fn tick(&mut self, now_ms: i64, flows: &[Flow], env: &TickEnv) -> Vec<Fire> {
        let mut fires = Vec::new();
        let live: Vec<String> = flows.iter().flat_map(|f| f.triggers.iter().map(|t| key(&f.id, &t.id))).collect();
        let before = self.state.entries.len();
        self.state.entries.retain(|k, _| live.contains(k));
        if self.state.entries.len() != before {
            self.dirty = true;
        }
        for f in flows {
            let paused_flow = self.is_paused(&f.id);
            for t in f.triggers.iter().filter(|t| t.enabled) {
                let k = key(&f.id, &t.id);
                match &t.kind {
                    TriggerKind::Cron { .. } | TriggerKind::Interval { .. } | TriggerKind::Once { .. } => {
                        let known = self.state.entries.contains_key(&k);
                        let mut e = self.state.entries.get(&k).cloned().unwrap_or_default();
                        if e.done {
                            continue;
                        }
                        if !known {
                            // a new trigger starts from now: nothing is "missed" before it existed (a one-shot in the past never fires)
                            e.next_ms = match &t.kind {
                                TriggerKind::Once { at_ms } if *at_ms > now_ms => Some(*at_ms),
                                TriggerKind::Once { .. } => {
                                    e.done = true;
                                    None
                                }
                                _ => self.next_tick(t, now_ms),
                            };
                        }
                        if paused_flow || e.paused {
                            // while paused, ticks pass and are remembered as due; resuming applies the missed policy
                            self.state.entries.insert(k, e);
                            continue;
                        }
                        let mut due: Vec<i64> = Vec::new();
                        while let Some(n) = e.next_ms {
                            if n > now_ms || due.len() >= 1000 {
                                break;
                            }
                            due.push(n);
                            e.next_ms = if matches!(t.kind, TriggerKind::Once { .. }) { None } else { self.next_tick(t, n) };
                        }
                        if matches!(t.kind, TriggerKind::Once { .. }) && e.next_ms.is_none() {
                            e.done = true;
                        }
                        let chosen: Vec<(i64, bool)> = if due.is_empty() {
                            vec![]
                        } else {
                            let last = *due.last().unwrap();
                            let on_time = now_ms - last <= GRACE_MS && due.len() == 1;
                            if on_time {
                                vec![(last, false)]
                            } else {
                                match t.missed {
                                    Missed::Skip => {
                                        if now_ms - last <= GRACE_MS { vec![(last, false)] } else { vec![] }
                                    }
                                    Missed::RunOnce => vec![(last, true)],
                                    Missed::CatchUp { max } => {
                                        let take = (max.max(1) as usize).min(due.len());
                                        due[due.len() - take..].iter().map(|d| (*d, true)).collect()
                                    }
                                }
                            }
                        };
                        e.skipped += (due.len() - chosen.len().min(due.len())) as u64;
                        if !due.is_empty() {
                            e.last_due_ms = *due.last().unwrap();
                        }
                        let mut allowed_overlap = true;
                        if !chosen.is_empty() && t.overlap == Overlap::Skip && (env.busy)(&f.id) {
                            e.skipped += chosen.len() as u64;
                            allowed_overlap = false;
                        }
                        if allowed_overlap {
                            for (d, missed) in chosen {
                                e.fired += 1;
                                e.last_fire_ms = now_ms;
                                fires.push(Fire { flow: f.id.clone(), trigger: t.id.clone(), due_ms: d, at_ms: if missed { now_ms } else { d + self.jitter(t, &f.id, d) }, missed, overlap: t.overlap, inputs: t.inputs.clone() });
                            }
                        }
                        self.state.entries.insert(k, e);
                        self.dirty = true;
                    }
                    TriggerKind::FileChange { path } => {
                        let known = self.state.entries.contains_key(&k);
                        let mut e = self.state.entries.get(&k).cloned().unwrap_or_default();
                        let fp = (env.fingerprint)(path).unwrap_or_default();
                        if known && !paused_flow && !e.paused && e.seen != fp {
                            if t.overlap == Overlap::Skip && (env.busy)(&f.id) {
                                e.skipped += 1;
                            } else {
                                e.fired += 1;
                                e.last_fire_ms = now_ms;
                                fires.push(Fire { flow: f.id.clone(), trigger: t.id.clone(), due_ms: now_ms, at_ms: now_ms, missed: false, overlap: t.overlap, inputs: t.inputs.clone() });
                            }
                        }
                        if !known || e.seen != fp {
                            e.seen = fp;
                            self.dirty = true;
                        }
                        self.state.entries.insert(k, e);
                    }
                    TriggerKind::Calendar { matching } => {
                        let known = self.state.entries.contains_key(&k);
                        let mut e = self.state.entries.get(&k).cloned().unwrap_or_default();
                        let since = if known { e.last_due_ms.max(self.last_tick_ms.min(now_ms)) } else { now_ms };
                        if known && !paused_flow && !e.paused {
                            for at in (env.calendar)(matching, since, now_ms) {
                                if t.overlap == Overlap::Skip && (env.busy)(&f.id) {
                                    e.skipped += 1;
                                    continue;
                                }
                                e.fired += 1;
                                e.last_fire_ms = now_ms;
                                fires.push(Fire { flow: f.id.clone(), trigger: t.id.clone(), due_ms: at, at_ms: now_ms, missed: false, overlap: t.overlap, inputs: t.inputs.clone() });
                            }
                        }
                        e.last_due_ms = now_ms;
                        self.state.entries.insert(k, e);
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
        }
        self.last_tick_ms = now_ms;
        self.save();
        fires.sort_by_key(|f| (f.at_ms, f.flow.clone(), f.trigger.clone()));
        fires
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn env<'a>(busy: &'a dyn Fn(&str) -> bool) -> TickEnv<'a> {
        TickEnv { busy, fingerprint: &|_| None, calendar: &|_, _, _| vec![] }
    }

    fn ms(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        (days_from_civil(y, mo, d) * 86400 + h as i64 * 3600 + mi as i64 * 60) * 1000
    }

    fn flow_with(t: Trigger) -> Flow {
        let mut f = Flow::new("f", "F");
        f.triggers.push(t);
        f
    }

    #[test]
    fn civil_dates_round_trip() {
        for d in [-800_000i64, -1, 0, 1, 365, 10_957, 19_000, 25_000, 2_000_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn cron_fields_and_next() {
        let utc = FixedTz(0);
        let c = Cron::parse("*/15 9-10 * * mon-fri").unwrap();
        // Wednesday 2026-10-14 08:00 UTC: next is 09:00 the same day
        let t = ms(2026, 10, 14, 8, 0) / 1000;
        assert_eq!(c.next_after(t, &utc), Some(ms(2026, 10, 14, 9, 0) / 1000));
        assert_eq!(c.next_after(ms(2026, 10, 14, 9, 0) / 1000, &utc), Some(ms(2026, 10, 14, 9, 15) / 1000));
        // after Friday 10:45 the next is Monday 09:00
        assert_eq!(c.next_after(ms(2026, 10, 16, 10, 45) / 1000, &utc), Some(ms(2026, 10, 19, 9, 0) / 1000));
        assert_eq!(Cron::parse("@hourly").unwrap().next_after(ms(2026, 1, 1, 0, 30) / 1000, &utc), Some(ms(2026, 1, 1, 1, 0) / 1000));
        assert_eq!(Cron::parse("0 0 29 2 *").unwrap().next_after(ms(2026, 3, 1, 0, 0) / 1000, &utc), Some(ms(2028, 2, 29, 0, 0) / 1000));
        // day-of-month OR day-of-week when both are given
        let both = Cron::parse("0 12 1 * mon").unwrap();
        assert_eq!(both.next_after(ms(2026, 10, 14, 0, 0) / 1000, &utc), Some(ms(2026, 10, 19, 12, 0) / 1000));
        assert_eq!(Cron::parse("0 12 * * 7").unwrap(), Cron::parse("0 12 * * 0").unwrap());
        for bad in ["", "* * * *", "61 * * * *", "* 24 * * *", "*/0 * * * *", "a b c d e", "5-1 * * * *"] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cron_honours_time_zones() {
        let ist = basic_tz("+05:30").unwrap();
        let c = Cron::parse("30 9 * * *").unwrap();
        // 09:30 in +05:30 is 04:00 UTC
        assert_eq!(c.next_after(ms(2026, 10, 14, 0, 0) / 1000, ist.as_ref()), Some(ms(2026, 10, 14, 4, 0) / 1000));
        assert!(basic_tz("Mars/Base").is_none());
        assert!(basic_tz("+25:00").is_none());
        assert!(basic_tz("UTC").is_some());
    }

    struct Dst;
    impl Tz for Dst {
        // a zone that is +1 until 2026-03-29 01:00 UTC and +2 after (a spring-forward)
        fn offset_secs(&self, unix: i64) -> i32 {
            if unix < ms(2026, 3, 29, 1, 0) / 1000 { 3600 } else { 7200 }
        }
    }

    #[test]
    fn cron_across_a_clock_change() {
        let c = Cron::parse("30 2 * * *").unwrap();
        // 02:30 local does not exist on 2026-03-29 (02:00 jumps to 03:00): it still fires once that day, never twice, never skipped
        let a = c.next_after(ms(2026, 3, 28, 12, 0) / 1000, &Dst).unwrap();
        let b = c.next_after(a, &Dst).unwrap();
        assert!(a > ms(2026, 3, 28, 12, 0) / 1000 && b - a >= 20 * 3600 && b - a <= 26 * 3600, "{a} {b}");
        let cc = Cron::parse("0 12 * * *").unwrap();
        let x = cc.next_after(ms(2026, 3, 28, 12, 0) / 1000, &Dst).unwrap();
        assert_eq!(x, ms(2026, 3, 29, 10, 0) / 1000, "noon local after the change is 10:00 UTC");
    }

    #[test]
    fn interval_and_once_and_preview() {
        let s = Scheduler::new(None, default_resolver());
        let f = flow_with(Trigger::new("t", TriggerKind::Interval { every_s: 300, anchor_ms: None }));
        let r = s.next_runs(&f, 1000, 3);
        assert_eq!(r.iter().map(|x| x.1).collect::<Vec<_>>(), vec![300_000, 600_000, 900_000]);
        let g = flow_with(Trigger::new("t", TriggerKind::Once { at_ms: 5000 }));
        assert_eq!(s.next_runs(&g, 0, 5).len(), 1);
        assert!(s.next_runs(&g, 6000, 5).is_empty());
        let mut off = flow_with(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
        off.triggers[0].enabled = false;
        assert!(s.next_runs(&off, 0, 5).is_empty());
    }

    #[test]
    fn a_new_trigger_does_not_catch_up_on_the_past() {
        let mut s = Scheduler::new(None, default_resolver());
        let f = flow_with(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
        let idle = |_: &str| false;
        assert!(s.tick(10 * MIN + 5, &[f.clone()], &env(&idle)).is_empty(), "first sight: remembers, fires nothing");
        let fires = s.tick(11 * MIN + 2000, &[f.clone()], &env(&idle));
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].due_ms, 11 * MIN);
        assert!(!fires[0].missed);
    }

    #[test]
    fn missed_policies() {
        let idle = |_: &str| false;
        let mk = |m: Missed| {
            let mut t = Trigger::new("t", TriggerKind::Interval { every_s: 600, anchor_ms: None });
            t.missed = m;
            flow_with(t)
        };
        for (m, expect, flagged) in [(Missed::Skip, 0usize, false), (Missed::RunOnce, 1, true), (Missed::CatchUp { max: 3 }, 3, true), (Missed::CatchUp { max: 100 }, 5, true)] {
            let f = mk(m);
            let mut s = Scheduler::new(None, default_resolver());
            s.tick(MIN, &[f.clone()], &env(&idle));
            // off for 54 minutes: the ticks at 10, 20, 30, 40 and 50 minutes passed, and the last is older than the grace time
            let fires = s.tick(55 * MIN, &[f.clone()], &env(&idle));
            assert_eq!(fires.len(), expect, "{m:?}");
            assert!(fires.iter().all(|x| x.missed == flagged), "{m:?}");
            if m == (Missed::CatchUp { max: 3 }) {
                assert_eq!(fires.iter().map(|x| x.due_ms).collect::<Vec<_>>(), vec![30 * MIN, 40 * MIN, 50 * MIN], "the latest three, oldest first");
            }
        }
        // a tick the machine woke up just after is not "missed", whatever the policy
        let f = mk(Missed::Skip);
        let mut s = Scheduler::new(None, default_resolver());
        s.tick(MIN, &[f.clone()], &env(&idle));
        let fires = s.tick(10 * MIN + 30_000, &[f.clone()], &env(&idle));
        assert_eq!(fires.len(), 1);
        assert!(!fires[0].missed);
    }

    #[test]
    fn overlap_policies_and_pausing() {
        let busy = |_: &str| true;
        let idle = |_: &str| false;
        for (o, expect) in [(Overlap::Skip, 0usize), (Overlap::Queue, 1), (Overlap::Allow, 1)] {
            let mut t = Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None });
            t.overlap = o;
            let f = flow_with(t);
            let mut s = Scheduler::new(None, default_resolver());
            s.tick(MIN, &[f.clone()], &env(&idle));
            assert_eq!(s.tick(2 * MIN + 100, &[f.clone()], &env(&busy)).len(), expect, "{o:?}");
        }
        let f = flow_with(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
        let mut s = Scheduler::new(None, default_resolver());
        s.tick(MIN, &[f.clone()], &env(&idle));
        s.pause_flow("f");
        assert!(s.tick(5 * MIN, &[f.clone()], &env(&idle)).is_empty(), "paused");
        s.resume_flow("f");
        let fires = s.tick(5 * MIN + 1000, &[f.clone()], &env(&idle));
        assert_eq!(fires.len(), 1, "resuming applies the missed policy (run once)");
        assert!(fires[0].missed);
    }

    #[test]
    fn once_fires_once_and_state_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("flow-sched-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sched.json");
        let idle = |_: &str| false;
        let f = flow_with(Trigger::new("t", TriggerKind::Once { at_ms: 5 * MIN }));
        let mut s = Scheduler::new(Some(path.clone()), default_resolver());
        assert!(s.tick(MIN, &[f.clone()], &env(&idle)).is_empty());
        assert_eq!(s.tick(5 * MIN + 10, &[f.clone()], &env(&idle)).len(), 1);
        assert!(s.tick(6 * MIN, &[f.clone()], &env(&idle)).is_empty());
        // "restart": a new scheduler reads the file; the one-shot stays done
        let mut s2 = Scheduler::new(Some(path.clone()), default_resolver());
        assert!(s2.tick(7 * MIN, &[f.clone()], &env(&idle)).is_empty());
        assert!(s2.entry("f", "t").unwrap().done);
        // an interval's next tick is remembered across the restart, so the time off counts as missed
        let g = flow_with(Trigger::new("i", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
        let mut s3 = Scheduler::new(Some(path.clone()), default_resolver());
        s3.tick(10 * MIN, &[g.clone()], &env(&idle));
        let mut s4 = Scheduler::new(Some(path), default_resolver());
        let fires = s4.tick(20 * MIN + 5, &[g], &env(&idle));
        assert_eq!(fires.len(), 1);
        assert!(fires[0].missed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jitter_delays_within_bounds() {
        let idle = |_: &str| false;
        let mut t = Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None });
        t.jitter_s = 20;
        let f = flow_with(t);
        let mut s = Scheduler::new(None, default_resolver());
        s.tick(MIN, &[f.clone()], &env(&idle));
        let fires = s.tick(2 * MIN + 10, &[f.clone()], &env(&idle));
        assert_eq!(fires.len(), 1);
        let j = fires[0].at_ms - fires[0].due_ms;
        assert!((0..20_000).contains(&j), "{j}");
    }

    #[test]
    fn file_change_fires_on_a_new_fingerprint_only() {
        let idle = |_: &str| false;
        let f = flow_with(Trigger::new("t", TriggerKind::FileChange { path: "/x".into() }));
        let mut s = Scheduler::new(None, default_resolver());
        let cell = std::cell::RefCell::new("a".to_string());
        let fp = |_: &str| Some(cell.borrow().clone());
        let e = TickEnv { busy: &idle, fingerprint: &fp, calendar: &|_, _, _| vec![] };
        assert!(s.tick(1000, &[f.clone()], &e).is_empty(), "first look only records");
        assert!(s.tick(2000, &[f.clone()], &e).is_empty());
        *cell.borrow_mut() = "b".into();
        assert_eq!(s.tick(3000, &[f.clone()], &e).len(), 1);
        assert!(s.tick(4000, &[f.clone()], &e).is_empty());
    }
}
