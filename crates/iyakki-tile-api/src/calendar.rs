//! The calendar facade: the alarm and event shapes a CALENDAR tile draws, the source it reads and writes them through, and the small
//! pure helpers (civil dates, the time/day/date parsers) it needs. The agent's `calendar` and `clock` modules have the same shapes
//! and the same helpers; the shell converts between them, and a shell test keeps the two copies in agreement.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Repeat {
    pub const ALL: [Repeat; 5] = [Repeat::None, Repeat::Daily, Repeat::Weekly, Repeat::Monthly, Repeat::Yearly];
    pub fn label(self) -> &'static str {
        match self {
            Repeat::None => "once",
            Repeat::Daily => "daily",
            Repeat::Weekly => "weekly",
            Repeat::Monthly => "monthly",
            Repeat::Yearly => "yearly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alarm {
    pub id: String,
    pub label: String,
    pub hour: u8,
    pub minute: u8,
    /// Weekdays it rings on, bit 0 = Sunday ... bit 6 = Saturday; 0 = once.
    pub days: u8,
    pub enabled: bool,
    /// Who made it: `user`, or `assistant`.
    pub by: String,
}

impl Alarm {
    pub fn days_label(&self) -> String {
        const N: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        match self.days {
            0 => "once".into(),
            0x7f => "every day".into(),
            0b0111110 => "weekdays".into(),
            0b1000001 => "weekends".into(),
            m => (0..7).filter(|i| m & (1 << i) != 0).map(|i| N[i]).collect::<Vec<_>>().join(" "),
        }
    }
}

/// One event on one day (a repeating event gives one per day it falls on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub id: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub repeat: Repeat,
    pub notes: String,
    pub by: String,
    pub remind: Option<u32>,
}

/// One ring of an alarm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlarmOccurrence {
    pub id: String,
    pub label: String,
    pub at: i64,
    pub days: String,
    pub enabled: bool,
}

/// A local date and time (the fields the tile reads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    /// Seconds east of UTC, and the zone's short name such as `PST`.
    pub offset: i32,
    pub abbrev: String,
}

/// What was saved when an event was added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub title: String,
    pub start: i64,
}

/// Where the tile gets its events and alarms, and the machine's time zone. The shell implements it with the agent's calendar store
/// and zone; tests use a fake. Every call is for one person (`user`).
pub trait CalendarSource {
    /// Unix time to local calendar fields, in the machine's zone.
    fn local(&self, unix: i64) -> Local;
    /// Local calendar fields to unix time.
    fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64;
    fn events_in(&self, user: &str, from: i64, to: i64) -> Vec<Occurrence>;
    fn alarms_in(&self, user: &str, from: i64, to: i64) -> Vec<AlarmOccurrence>;
    #[allow(clippy::too_many_arguments)]
    fn add_event(&self, user: &str, title: &str, start: i64, minutes: u32, all_day: bool, repeat: Repeat, notes: &str, by: &str, remind: Option<u32>) -> Result<Added, String>;
    fn remove_event(&self, user: &str, id: &str) -> Result<(), String>;
    fn alarms(&self, user: &str) -> Vec<Alarm>;
    fn add_alarm(&self, user: &str, label: &str, hour: u32, minute: u32, days: u8, now: i64, by: &str) -> Result<Alarm, String>;
    fn remove_alarm(&self, user: &str, id: &str) -> Result<(), String>;
    fn set_alarm_enabled(&self, user: &str, id: &str, on: bool, now: i64) -> Result<(), String>;
    fn next_alarm(&self, user: &str, now: i64) -> Option<AlarmOccurrence>;
}

/// Seconds since 1970 now (the wall clock).
pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Days since 1970-01-01 -> (year, month 1-12, day 1-31).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    let next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
    (next - days_from_civil(y, m, 1)) as u32
}

/// `daily`, `weekdays`, `weekends`, `once`, or weekday names such as `mon,wed,fri`.
pub fn parse_days(s: &str) -> Result<u8, String> {
    let t = s.trim().to_ascii_lowercase();
    match t.as_str() {
        "" | "once" | "no" | "none" => return Ok(0),
        "daily" | "everyday" | "every day" | "all" => return Ok(0x7f),
        "weekdays" | "workdays" => return Ok(0b0111110),
        "weekends" | "weekend" => return Ok(0b1000001),
        _ => {}
    }
    let mut m = 0u8;
    for p in t.split([',', ' ', '/']).filter(|p| !p.is_empty()) {
        let i = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().position(|d| p.starts_with(d)).ok_or_else(|| if p.chars().all(|c| c.is_ascii_digit()) { format!("`{p}` is a number, but `days` takes weekdays, not how many days: use daily, weekdays, weekends, once, or names like mon,wed,fri") } else { format!("`{p}` is not a weekday; use daily, weekdays, weekends, once, or names like mon,wed,fri") })?;
        m |= 1 << i;
    }
    Ok(m)
}

/// `7:30`, `07:30`, `18:05`, `7:30pm`, `7pm`.
pub fn parse_hm(t: &str) -> Option<(u32, u32)> {
    let t = t.trim().to_ascii_lowercase();
    let (body, pm, am) = if let Some(b) = t.strip_suffix("pm") { (b.trim(), true, false) } else if let Some(b) = t.strip_suffix("am") { (b.trim(), false, true) } else { (t.as_str(), false, false) };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if pm || am => (body.parse::<u32>().ok()?, 0),
        None => return None,
    };
    if m > 59 {
        return None;
    }
    let h = if pm || am {
        if !(1..=12).contains(&h) {
            return None;
        }
        match (pm, h) {
            (true, 12) => 12,
            (true, h) => h + 12,
            (false, 12) => 0,
            (false, h) => h,
        }
    } else {
        h
    };
    (h < 24).then_some((h, m))
}

/// `2026-10-12 15:30`, `2026-10-12T15:30` or `2026-10-12` (a whole day), in the source's time zone.
/// Returns the instant and whether it is a whole day.
pub fn parse_local(src: &dyn CalendarSource, s: &str) -> Result<(i64, bool), String> {
    let s = s.trim();
    let bad = || format!("`{s}` is not a date: write 2026-10-12 15:30 (or 2026-10-12 for a whole day)");
    let (date, time) = match s.split_once([' ', 'T']) {
        Some((d, t)) => (d, Some(t.trim())),
        None => (s, None),
    };
    let mut dp = date.split('-');
    let (y, m, d) = (dp.next().and_then(|v| v.parse::<i64>().ok()).ok_or_else(bad)?, dp.next().and_then(|v| v.parse::<u32>().ok()).ok_or_else(bad)?, dp.next().and_then(|v| v.parse::<u32>().ok()).ok_or_else(bad)?);
    if dp.next().is_some() || !(1970..=2100).contains(&y) || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return Err(bad());
    }
    match time {
        None | Some("") => Ok((src.local_to_unix(y, m, d, 0, 0, 0), true)),
        Some(t) => {
            let (h, mi) = parse_hm(t).ok_or_else(bad)?;
            Ok((src.local_to_unix(y, m, d, h, mi, 0), false))
        }
    }
}

/// A calendar with nothing in it that refuses every change; for a context whose tile does not use the calendar.
pub struct NoCalendar;

impl CalendarSource for NoCalendar {
    fn local(&self, unix: i64) -> Local {
        let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400) as u32);
        let (year, month, day) = civil(days);
        Local { year, month, day, hour: secs / 3600, minute: secs % 3600 / 60, second: secs % 60, weekday: (days + 4).rem_euclid(7) as u32, offset: 0, abbrev: "UTC".into() }
    }
    fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        days_from_civil(y, m, d) * 86_400 + (hh * 3600 + mm * 60 + ss) as i64
    }
    fn events_in(&self, _: &str, _: i64, _: i64) -> Vec<Occurrence> {
        vec![]
    }
    fn alarms_in(&self, _: &str, _: i64, _: i64) -> Vec<AlarmOccurrence> {
        vec![]
    }
    fn add_event(&self, _: &str, _: &str, _: i64, _: u32, _: bool, _: Repeat, _: &str, _: &str, _: Option<u32>) -> Result<Added, String> {
        Err("no calendar".into())
    }
    fn remove_event(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn alarms(&self, _: &str) -> Vec<Alarm> {
        vec![]
    }
    fn add_alarm(&self, _: &str, _: &str, _: u32, _: u32, _: u8, _: i64, _: &str) -> Result<Alarm, String> {
        Err("no calendar".into())
    }
    fn remove_alarm(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn set_alarm_enabled(&self, _: &str, _: &str, _: bool, _: i64) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn next_alarm(&self, _: &str, _: i64) -> Option<AlarmOccurrence> {
        None
    }
}
