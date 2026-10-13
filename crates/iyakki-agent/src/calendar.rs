//! Each person's calendar and alarms. Events have a time (or a whole day), a length, a repeat and an optional
//! reminder; alarms ring at a time of day, once or on chosen weekdays. The calendar app, the chat's tools and the
//! terminal all use this one store, so a plan the assistant makes appears in the app and an alarm set in the app can be
//! read by the assistant. Times are kept as instants and shown in the machine's time zone, so a daily 9:00 stays at 9:00
//! across daylight saving.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::clock::{civil, days_from_civil, Zone};

pub const MAX_EVENTS: usize = 2000;
pub const MAX_ALARMS: usize = 100;
const DAY: i64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
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
    pub fn parse(s: &str) -> Result<Repeat, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "once" | "no" => Ok(Repeat::None),
            "daily" | "day" | "everyday" => Ok(Repeat::Daily),
            "weekly" | "week" => Ok(Repeat::Weekly),
            "monthly" | "month" => Ok(Repeat::Monthly),
            "yearly" | "year" | "annual" | "annually" => Ok(Repeat::Yearly),
            o => Err(format!("repeat `{o}` is not one of: once, daily, weekly, monthly, yearly")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub title: String,
    /// The first start, as an instant (for an all-day event, local midnight of its date).
    pub start: i64,
    /// Length in minutes; 0 for a point in time. An all-day event lasts the day.
    pub minutes: u32,
    pub all_day: bool,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub notes: String,
    /// Who made it: `user`, or `assistant` when the chat planned it.
    #[serde(default)]
    pub by: String,
    /// Remind this many minutes before it starts (0 = when it starts); none = no reminder.
    #[serde(default)]
    pub remind: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alarm {
    pub id: String,
    pub label: String,
    pub hour: u8,
    pub minute: u8,
    /// Weekdays it rings on, bit 0 = Sunday … bit 6 = Saturday; 0 = once, at `once_at`.
    pub days: u8,
    #[serde(default)]
    pub once_at: Option<i64>,
    pub enabled: bool,
    #[serde(default)]
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
        let i = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().position(|d| p.starts_with(d)).ok_or_else(|| format!("`{p}` is not a weekday; use daily, weekdays, weekends, once, or names like mon,wed,fri"))?;
        m |= 1 << i;
    }
    Ok(m)
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlarmOccurrence {
    pub id: String,
    pub label: String,
    pub at: i64,
    pub days: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    Alarm { label: String, at: i64 },
    Reminder { title: String, starts: i64 },
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default)]
    alarms: Vec<Alarm>,
}

pub struct CalendarStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

fn days_in_month(y: i64, m: u32) -> u32 {
    let next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
    (next - days_from_civil(y, m, 1)) as u32
}

fn new_id() -> String {
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    r.iter().map(|b| format!("{b:02x}")).collect()
}

fn clean_text(s: &str, max: usize, what: &str) -> Result<String, String> {
    let t: String = s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect::<String>().trim().to_string();
    if t.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    Ok(t)
}

/// Starts of an event that fall in `[from, to)` (by their start time), repeats expanded.
pub fn starts_in(e: &Event, from: i64, to: i64, zone: &Zone) -> Vec<i64> {
    if to <= from {
        return vec![];
    }
    if e.repeat == Repeat::None {
        return if e.start >= from && e.start < to { vec![e.start] } else { vec![] };
    }
    let base = zone.local(e.start + if e.all_day { 3600 } else { 0 });
    let (by, bm, bd) = (base.year, base.month, base.day);
    let (h, mi) = if e.all_day { (0, 0) } else { (base.hour, base.minute) };
    let mut out = Vec::new();
    let at = |y: i64, m: u32, d: u32| zone.local_to_unix(y, m, d, h, mi, 0);
    let base_days = days_from_civil(by, bm, bd);
    match e.repeat {
        Repeat::Daily | Repeat::Weekly => {
            let step = if e.repeat == Repeat::Daily { 1 } else { 7 };
            let from_days = (from + 12 * 3600).div_euclid(DAY);
            let k0 = ((from_days - base_days).div_euclid(step) - 1).max(0);
            for k in k0..k0 + 20_000 {
                let (y, m, d) = civil(base_days + k * step);
                let t = at(y, m, d);
                if t >= to {
                    break;
                }
                if t >= from {
                    out.push(t);
                }
            }
        }
        Repeat::Monthly => {
            for k in 0..(12 * 100) {
                let (mut y, mut m) = (by, bm as i64 + k);
                y += (m - 1).div_euclid(12);
                m = (m - 1).rem_euclid(12) + 1;
                if bd > days_in_month(y, m as u32) {
                    continue;
                }
                let t = at(y, m as u32, bd);
                if t >= to {
                    break;
                }
                if t >= from && t >= e.start {
                    out.push(t);
                }
            }
        }
        Repeat::Yearly => {
            for k in 0..200 {
                let y = by + k;
                if bd > days_in_month(y, bm) {
                    continue;
                }
                let t = at(y, bm, bd);
                if t >= to {
                    break;
                }
                if t >= from && t >= e.start {
                    out.push(t);
                }
            }
        }
        Repeat::None => {}
    }
    out
}

fn alarm_times(a: &Alarm, from: i64, to: i64, zone: &Zone) -> Vec<i64> {
    if to <= from {
        return vec![];
    }
    if a.days == 0 {
        return a.once_at.filter(|t| *t >= from && *t < to).into_iter().collect();
    }
    let (first, last) = ((from - DAY).div_euclid(DAY), (to + DAY).div_euclid(DAY));
    let mut out = Vec::new();
    // local dates around the range
    let d0 = civil(days_from_civil(zone.local(from).year, zone.local(from).month, zone.local(from).day) - 1);
    let _ = (first, last, d0);
    let start_days = days_from_civil(zone.local(from).year, zone.local(from).month, zone.local(from).day) - 1;
    let end_days = days_from_civil(zone.local(to).year, zone.local(to).month, zone.local(to).day) + 1;
    for dd in start_days..=end_days.min(start_days + 800) {
        let (y, m, d) = civil(dd);
        let wd = (dd + 4).rem_euclid(7) as u8;
        if a.days & (1 << wd) == 0 {
            continue;
        }
        let t = zone.local_to_unix(y, m, d, a.hour as u32, a.minute as u32, 0);
        if t >= from && t < to {
            out.push(t);
        }
    }
    out
}

impl CalendarStore {
    pub fn new(dir: &std::path::Path) -> CalendarStore {
        CalendarStore { dir: dir.into(), lock: Mutex::new(()) }
    }

    fn path(&self, user: &str) -> Result<PathBuf, String> {
        if iyakki_sec::policy::valid_username(user) { Ok(self.dir.join(format!("{user}.json"))) } else { Err("invalid user".into()) }
    }

    fn load(&self, user: &str) -> Result<Data, String> {
        match std::fs::read(self.path(user)?) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("the calendar file is damaged: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Data::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, user: &str, d: &Data) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(user)?;
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&serde_json::to_vec_pretty(d).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_event(&self, user: &str, title: &str, start: i64, minutes: u32, all_day: bool, repeat: Repeat, notes: &str, by: &str, remind: Option<u32>) -> Result<Event, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let title = clean_text(title, 120, "the title")?;
        if title.is_empty() {
            return Err("an event needs a title".into());
        }
        if minutes > 60 * 24 * 31 {
            return Err("an event can last at most 31 days".into());
        }
        if remind.is_some_and(|r| r > 60 * 24 * 14) {
            return Err("a reminder can be at most 14 days before".into());
        }
        if !(0..4_102_444_800).contains(&start) {
            return Err("that date is out of range (1970 to 2100)".into());
        }
        let mut d = self.load(user)?;
        if d.events.len() >= MAX_EVENTS {
            return Err(format!("the calendar is full ({MAX_EVENTS} events); delete some"));
        }
        let e = Event { id: new_id(), title, start, minutes, all_day, repeat, notes: clean_text(notes, 1000, "the notes")?, by: by.into(), remind };
        d.events.push(e.clone());
        self.save(user, &d)?;
        Ok(e)
    }

    pub fn remove_event(&self, user: &str, id: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = self.load(user)?;
        let n = d.events.len();
        d.events.retain(|e| e.id != id);
        if d.events.len() == n {
            return Err(format!("no event with id {id}"));
        }
        self.save(user, &d)
    }

    pub fn add_alarm(&self, user: &str, label: &str, hour: u32, minute: u32, days: u8, now: i64, zone: &Zone, by: &str) -> Result<Alarm, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if hour > 23 || minute > 59 {
            return Err("the time must be between 00:00 and 23:59".into());
        }
        let mut d = self.load(user)?;
        if d.alarms.len() >= MAX_ALARMS {
            return Err(format!("too many alarms ({MAX_ALARMS}); delete some"));
        }
        let mut a = Alarm { id: new_id(), label: clean_text(label, 80, "the label")?, hour: hour as u8, minute: minute as u8, days: days & 0x7f, once_at: None, enabled: true, by: by.into() };
        if a.label.is_empty() {
            a.label = "Alarm".into();
        }
        if a.days == 0 {
            // the next time the clock shows that time
            let l = zone.local(now);
            let today = zone.local_to_unix(l.year, l.month, l.day, hour, minute, 0);
            a.once_at = Some(if today > now { today } else { let (y, m, dd) = civil(days_from_civil(l.year, l.month, l.day) + 1); zone.local_to_unix(y, m, dd, hour, minute, 0) });
        }
        d.alarms.push(a.clone());
        self.save(user, &d)?;
        Ok(a)
    }

    pub fn remove_alarm(&self, user: &str, id: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = self.load(user)?;
        let n = d.alarms.len();
        d.alarms.retain(|a| a.id != id);
        if d.alarms.len() == n {
            return Err(format!("no alarm with id {id}"));
        }
        self.save(user, &d)
    }

    /// Switches an alarm on or off; switching a one-time alarm on sets it to the next time the clock shows that time.
    pub fn set_alarm_enabled(&self, user: &str, id: &str, on: bool, now: i64, zone: &Zone) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = self.load(user)?;
        let a = d.alarms.iter_mut().find(|a| a.id == id).ok_or_else(|| format!("no alarm with id {id}"))?;
        a.enabled = on;
        if on && a.days == 0 {
            let l = zone.local(now);
            let today = zone.local_to_unix(l.year, l.month, l.day, a.hour as u32, a.minute as u32, 0);
            a.once_at = Some(if today > now { today } else { let (y, m, dd) = civil(days_from_civil(l.year, l.month, l.day) + 1); zone.local_to_unix(y, m, dd, a.hour as u32, a.minute as u32, 0) });
        }
        self.save(user, &d)
    }

    pub fn alarms(&self, user: &str) -> Vec<Alarm> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.load(user).map(|d| d.alarms).unwrap_or_default();
        v.sort_by_key(|a| (a.hour, a.minute, a.id.clone()));
        v
    }

    pub fn events(&self, user: &str) -> Vec<Event> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load(user).map(|d| d.events).unwrap_or_default()
    }

    /// Events (repeats expanded) that are on during `[from, to)`, soonest first.
    pub fn events_in(&self, user: &str, from: i64, to: i64, zone: &Zone) -> Vec<Occurrence> {
        let mut out = Vec::new();
        for e in self.events(user) {
            let len = if e.all_day { DAY } else { e.minutes as i64 * 60 };
            for s in starts_in(&e, from - len, to, zone) {
                let end = s + len;
                if end > from || (len == 0 && s >= from) {
                    out.push(Occurrence { id: e.id.clone(), title: e.title.clone(), start: s, end, all_day: e.all_day, repeat: e.repeat, notes: e.notes.clone(), by: e.by.clone(), remind: e.remind });
                }
            }
        }
        out.sort_by(|a, b| a.start.cmp(&b.start).then(a.title.cmp(&b.title)));
        out
    }

    /// When the alarms ring during `[from, to)` (switched-off ones included, marked).
    pub fn alarms_in(&self, user: &str, from: i64, to: i64, zone: &Zone) -> Vec<AlarmOccurrence> {
        let mut out = Vec::new();
        for a in self.alarms(user) {
            for t in alarm_times(&a, from, to, zone) {
                out.push(AlarmOccurrence { id: a.id.clone(), label: a.label.clone(), at: t, days: a.days_label(), enabled: a.enabled });
            }
        }
        out.sort_by_key(|a| a.at);
        out
    }

    /// The next time an enabled alarm rings after `now`.
    pub fn next_alarm(&self, user: &str, now: i64, zone: &Zone) -> Option<AlarmOccurrence> {
        self.alarms_in(user, now + 1, now + 8 * DAY, zone).into_iter().find(|a| a.enabled)
    }

    /// What became due in `(since, now]`: alarms ringing and event reminders. A one-time alarm that rang is switched off.
    pub fn take_due(&self, user: &str, since: i64, now: i64, zone: &Zone) -> Vec<Due> {
        if now <= since {
            return vec![];
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(mut d) = self.load(user) else { return vec![] };
        let mut out = Vec::new();
        let mut changed = false;
        for a in d.alarms.iter_mut().filter(|a| a.enabled) {
            for t in alarm_times(a, since + 1, now + 1, zone) {
                out.push(Due::Alarm { label: a.label.clone(), at: t });
                if a.days == 0 {
                    a.enabled = false;
                    changed = true;
                }
            }
        }
        for e in &d.events {
            let Some(r) = e.remind else { continue };
            let lead = r as i64 * 60;
            for s in starts_in(e, since + 1 + lead, now + 1 + lead, zone) {
                out.push(Due::Reminder { title: e.title.clone(), starts: s });
            }
        }
        if changed {
            let _ = self.save(user, &d);
        }
        out
    }
}

/// `2026-10-12 15:30`, `2026-10-12T15:30` or `2026-10-12` (a whole day), in the machine's time zone.
/// Returns the instant and whether it is a whole day.
pub fn parse_local(zone: &Zone, s: &str) -> Result<(i64, bool), String> {
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
        None | Some("") => Ok((zone.local_to_unix(y, m, d, 0, 0, 0), true)),
        Some(t) => {
            let (h, mi) = parse_hm(t).ok_or_else(bad)?;
            Ok((zone.local_to_unix(y, m, d, h, mi, 0), false))
        }
    }
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

/// The sound of an alarm: three rising beeps, twice, about four seconds.
pub fn alarm_wav() -> Vec<u8> {
    let mut pcm = Vec::new();
    for _ in 0..2 {
        for f in [880.0, 988.0, 1175.0] {
            pcm.extend(crate::voice::tone(f, 220));
            pcm.extend(vec![0i16; crate::voice::SAMPLE_RATE as usize * 130 / 1000]);
        }
        pcm.extend(vec![0i16; crate::voice::SAMPLE_RATE as usize * 600 / 1000]);
    }
    crate::voice::encode_wav(&pcm, crate::voice::SAMPLE_RATE)
}

const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MO: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `Mon 12 Oct 2026 15:30`.
pub fn when_text(zone: &Zone, t: i64, all_day: bool) -> String {
    let l = zone.local(t + if all_day { 3600 } else { 0 });
    if all_day { format!("{} {} {} {}", WD[l.weekday as usize], l.day, MO[l.month as usize - 1], l.year) } else { format!("{} {} {} {} {:02}:{:02}", WD[l.weekday as usize], l.day, MO[l.month as usize - 1], l.year, l.hour, l.minute) }
}

/// One line for an occurrence, as the assistant and the terminal show it.
pub fn occurrence_line(zone: &Zone, o: &Occurrence) -> String {
    let when = if o.all_day {
        format!("{} (all day)", when_text(zone, o.start, true))
    } else if o.end > o.start {
        let e = zone.local(o.end);
        format!("{} to {:02}:{:02}", when_text(zone, o.start, false), e.hour, e.minute)
    } else {
        when_text(zone, o.start, false)
    };
    format!("[{}] {}  {}{}{}{}", o.id, when, o.title, if o.repeat == Repeat::None { String::new() } else { format!("  (repeats {})", o.repeat.label()) }, o.remind.map_or(String::new(), |r| format!("  (reminder {r} min before)")), if o.notes.is_empty() { String::new() } else { format!("  — {}", o.notes.replace('\n', " ")) })
}

pub fn alarm_line(zone: &Zone, a: &Alarm, now: i64) -> String {
    let next = alarm_times(a, now, now + 8 * DAY, zone).first().map(|t| when_text(zone, *t, false));
    format!("[{}] {:02}:{:02}  {}  ({}{}){}", a.id, a.hour, a.minute, a.label, a.days_label(), if a.enabled { String::new() } else { ", off".into() }, if a.enabled { next.map_or(String::new(), |n| format!("  next: {n}")) } else { String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> CalendarStore {
        let d = std::env::temp_dir().join(format!("iyakki-cal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        CalendarStore::new(&d)
    }

    fn t(z: &Zone, y: i64, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        z.local_to_unix(y, m, d, h, mi, 0)
    }

    #[test]
    fn dates_and_times_are_read_leniently_but_checked() {
        let z = Zone::utc();
        assert_eq!(parse_local(&z, "2026-10-12 15:30").unwrap(), (t(&z, 2026, 10, 12, 15, 30), false));
        assert_eq!(parse_local(&z, "2026-10-12T07:05").unwrap().0, t(&z, 2026, 10, 12, 7, 5));
        assert_eq!(parse_local(&z, "2026-10-12").unwrap(), (t(&z, 2026, 10, 12, 0, 0), true));
        for bad in ["", "tomorrow", "2026-13-01", "2026-02-30", "2026-10-12 25:00", "2026-10-12 9", "10/12/2026", "1969-01-01", "2026-10-12 12:60"] {
            assert!(parse_local(&z, bad).is_err(), "{bad}");
        }
        assert_eq!((parse_hm("7:30"), parse_hm("07:30"), parse_hm("7:30pm"), parse_hm("7pm"), parse_hm("12am"), parse_hm("12pm"), parse_hm("18:05")), (Some((7, 30)), Some((7, 30)), Some((19, 30)), Some((19, 0)), Some((0, 0)), Some((12, 0)), Some((18, 5))));
        assert!(parse_hm("24:00").is_none() && parse_hm("7").is_none() && parse_hm("13pm").is_none() && parse_hm("x").is_none());
        assert_eq!((parse_days("daily"), parse_days("weekdays"), parse_days("weekends"), parse_days("once"), parse_days("mon,wed,fri"), parse_days("Sat Sun")), (Ok(0x7f), Ok(0b0111110), Ok(0b1000001), Ok(0), Ok(0b0101010), Ok(0b1000001)));
        assert!(parse_days("funday").is_err());
        assert!(Repeat::parse("weekly").is_ok() && Repeat::parse("sometimes").is_err());
    }

    #[test]
    fn events_repeat_daily_weekly_monthly_and_yearly_and_keep_their_clock_time_across_daylight_saving() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let s = store("repeat");
        let mk = |title: &str, y, m, d, h, mi, rep| s.add_event("amy", title, t(&z, y, m, d, h, mi), 30, false, rep, "", "user", None).unwrap();
        mk("standup", 2026, 3, 2, 9, 0, Repeat::Daily);
        mk("review", 2026, 3, 2, 14, 0, Repeat::Weekly);
        mk("rent", 2026, 1, 31, 8, 0, Repeat::Monthly);
        mk("birthday", 2024, 2, 29, 12, 0, Repeat::Yearly);
        mk("dentist", 2026, 3, 10, 16, 15, Repeat::None);
        let week = s.events_in("amy", t(&z, 2026, 3, 6, 0, 0), t(&z, 2026, 3, 13, 0, 0), &z);
        let standups: Vec<_> = week.iter().filter(|o| o.title == "standup").collect();
        assert_eq!(standups.len(), 7);
        assert!(standups.iter().all(|o| { let l = z.local(o.start); (l.hour, l.minute) == (9, 0) }), "9:00 stays 9:00 although the clocks went forward on 8 March");
        assert_eq!(standups[2].start - standups[1].start, 23 * 3600, "the short day");
        assert_eq!(week.iter().filter(|o| o.title == "review").count(), 1);
        assert_eq!(week.iter().filter(|o| o.title == "dentist").count(), 1);
        // monthly on the 31st skips short months
        let months = s.events_in("amy", t(&z, 2026, 1, 1, 0, 0), t(&z, 2026, 8, 1, 0, 0), &z);
        let rent: Vec<u32> = months.iter().filter(|o| o.title == "rent").map(|o| z.local(o.start).month).collect();
        assert_eq!(rent, vec![1, 3, 5, 7], "no 31st in February, April or June");
        // a leap-day birthday only in leap years
        let years = s.events_in("amy", t(&z, 2024, 1, 1, 0, 0), t(&z, 2029, 1, 1, 0, 0), &z);
        assert_eq!(years.iter().filter(|o| o.title == "birthday").map(|o| z.local(o.start).year).collect::<Vec<_>>(), vec![2024, 2028]);
        // nothing before the first date
        assert!(s.events_in("amy", t(&z, 2026, 2, 1, 0, 0), t(&z, 2026, 3, 1, 0, 0), &z).iter().all(|o| o.title == "rent"));
        // an event that began before the range and is still on shows up
        let long = s.add_event("amy", "conference", t(&z, 2026, 5, 4, 9, 0), 60 * 24 * 3, false, Repeat::None, "", "user", None).unwrap();
        assert!(s.events_in("amy", t(&z, 2026, 5, 6, 0, 0), t(&z, 2026, 5, 7, 0, 0), &z).iter().any(|o| o.id == long.id));
        assert!(!s.events_in("amy", t(&z, 2026, 5, 8, 0, 0), t(&z, 2026, 5, 9, 0, 0), &z).iter().any(|o| o.id == long.id));
    }

    #[test]
    fn all_day_events_cover_the_local_day_and_belong_to_one_person() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let s = store("allday");
        let (start, all_day) = parse_local(&z, "2026-10-12").unwrap();
        s.add_event("amy", "holiday", start, 0, all_day, Repeat::None, "bank holiday", "assistant", None).unwrap();
        let day = s.events_in("amy", t(&z, 2026, 10, 12, 0, 0), t(&z, 2026, 10, 13, 0, 0), &z);
        assert_eq!(day.len(), 1);
        assert!(day[0].all_day && day[0].by == "assistant");
        assert!(s.events_in("amy", t(&z, 2026, 10, 13, 0, 0), t(&z, 2026, 10, 14, 0, 0), &z).is_empty());
        assert!(s.events_in("bob", t(&z, 2026, 10, 12, 0, 0), t(&z, 2026, 10, 13, 0, 0), &z).is_empty(), "another person sees nothing");
        assert!(occurrence_line(&z, &day[0]).contains("(all day)") && occurrence_line(&z, &day[0]).contains("bank holiday"));
    }

    #[test]
    fn alarms_ring_once_or_on_chosen_days_and_a_one_time_alarm_switches_itself_off() {
        let z = Zone::utc();
        let s = store("alarms");
        let now = t(&z, 2026, 10, 12, 6, 0); // a Monday
        let once = s.add_alarm("amy", "tea", 6, 30, 0, now, &z, "user").unwrap();
        assert_eq!(once.once_at, Some(t(&z, 2026, 10, 12, 6, 30)), "the next 06:30 is this morning");
        let past = s.add_alarm("amy", "late", 5, 0, 0, now, &z, "user").unwrap();
        assert_eq!(past.once_at, Some(t(&z, 2026, 10, 13, 5, 0)), "05:00 has passed, so tomorrow");
        s.add_alarm("amy", "work", 7, 15, parse_days("weekdays").unwrap(), now, &z, "assistant").unwrap();
        assert!(s.add_alarm("amy", "x", 24, 0, 0, now, &z, "user").is_err());
        assert_eq!(s.next_alarm("amy", now, &z).unwrap().label, "tea");
        // the week's alarm schedule
        let week = s.alarms_in("amy", t(&z, 2026, 10, 12, 0, 0), t(&z, 2026, 10, 19, 0, 0), &z);
        assert_eq!(week.iter().filter(|a| a.label == "work").count(), 5, "Monday to Friday");
        assert_eq!(week.iter().filter(|a| a.label == "tea").count(), 1);
        // time passes: the one-time alarm rings once and then is off
        let due = s.take_due("amy", now, t(&z, 2026, 10, 12, 6, 31), &z);
        assert_eq!(due, vec![Due::Alarm { label: "tea".into(), at: t(&z, 2026, 10, 12, 6, 30) }]);
        assert!(s.take_due("amy", t(&z, 2026, 10, 12, 6, 31), t(&z, 2026, 10, 12, 6, 40), &z).is_empty());
        assert!(!s.alarms("amy").iter().find(|a| a.id == once.id).unwrap().enabled);
        // the weekday alarm rings on Monday at 07:15 and not on Saturday
        assert_eq!(s.take_due("amy", t(&z, 2026, 10, 12, 7, 0), t(&z, 2026, 10, 12, 7, 20), &z).len(), 1);
        assert!(s.take_due("amy", t(&z, 2026, 10, 17, 7, 0), t(&z, 2026, 10, 17, 7, 20), &z).is_empty());
        // switching a one-time alarm on again sets it for the next 06:30
        s.set_alarm_enabled("amy", &once.id, true, t(&z, 2026, 10, 12, 8, 0), &z).unwrap();
        assert_eq!(s.alarms("amy").iter().find(|a| a.id == once.id).unwrap().once_at, Some(t(&z, 2026, 10, 13, 6, 30)));
        s.set_alarm_enabled("amy", &once.id, false, now, &z).unwrap();
        assert!(s.next_alarm("amy", now, &z).is_some());
        assert!(!alarm_line(&z, &s.alarms("amy")[0], now).is_empty());
        s.remove_alarm("amy", &once.id).unwrap();
        assert!(s.remove_alarm("amy", &once.id).is_err());
    }

    #[test]
    fn reminders_become_due_before_an_event_starts_and_only_once() {
        let z = Zone::utc();
        let s = store("remind");
        s.add_event("amy", "call mum", t(&z, 2026, 10, 12, 18, 0), 15, false, Repeat::None, "", "user", Some(10)).unwrap();
        s.add_event("amy", "no reminder", t(&z, 2026, 10, 12, 18, 5), 15, false, Repeat::None, "", "user", None).unwrap();
        s.add_event("amy", "at the start", t(&z, 2026, 10, 12, 18, 30), 15, false, Repeat::None, "", "user", Some(0)).unwrap();
        s.add_event("amy", "daily", t(&z, 2026, 10, 1, 18, 2), 15, false, Repeat::Daily, "", "user", Some(2)).unwrap();
        let due = |a: (u32, u32), b: (u32, u32)| s.take_due("amy", t(&z, 2026, 10, 12, a.0, a.1), t(&z, 2026, 10, 12, b.0, b.1), &z);
        assert!(due((17, 0), (17, 49)).is_empty());
        let d = due((17, 49), (17, 51));
        assert_eq!(d, vec![Due::Reminder { title: "call mum".into(), starts: t(&z, 2026, 10, 12, 18, 0) }]);
        assert!(due((17, 51), (17, 59)).is_empty(), "it was not due again");
        assert_eq!(due((17, 59), (18, 1)).len(), 1, "the daily one, 2 minutes before 18:02");
        assert_eq!(due((18, 29), (18, 31)).len(), 1, "a reminder of 0 minutes comes when it starts");
    }

    #[test]
    fn the_alarm_sound_is_a_short_valid_wav() {
        let w = alarm_wav();
        assert_eq!(&w[..4], b"RIFF");
        assert!(w.len() > 100_000 && w.len() < 400_000, "{}", w.len());
        assert!(w[44..].iter().any(|b| *b != 0), "it is not silence");
    }

    #[test]
    fn limits_text_cleaning_files_and_damage_are_handled() {
        let z = Zone::utc();
        let s = store("limits");
        assert!(s.add_event("amy", "  ", 0, 0, false, Repeat::None, "", "user", None).is_err());
        assert!(s.add_event("amy", &"x".repeat(121), 0, 0, false, Repeat::None, "", "user", None).is_err());
        assert!(s.add_event("amy", "x", -5, 0, false, Repeat::None, "", "user", None).is_err());
        assert!(s.add_event("amy", "x", 0, 99_999_999, false, Repeat::None, "", "user", None).is_err());
        assert!(s.add_event("../evil", "x", 0, 0, false, Repeat::None, "", "user", None).is_err());
        let e = s.add_event("amy", "line\x07break\nnotes", 100, 0, false, Repeat::None, "a\x00b", "user", None).unwrap();
        assert!(!e.title.contains('\x07') && !e.notes.contains('\0'));
        assert!(s.remove_event("amy", "nope").is_err());
        s.remove_event("amy", &e.id).unwrap();
        // permissions and a damaged file
        use std::os::unix::fs::PermissionsExt;
        s.add_event("amy", "x", 100, 0, false, Repeat::None, "", "user", None).unwrap();
        let path = s.path("amy").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&path, b"{ nope").unwrap();
        assert!(s.add_event("amy", "y", 1, 0, false, Repeat::None, "", "user", None).unwrap_err().contains("damaged"));
        assert!(s.events("amy").is_empty() && s.take_due("amy", 0, 1000, &z).is_empty(), "reading a damaged file gives nothing, never a crash");
    }
}
