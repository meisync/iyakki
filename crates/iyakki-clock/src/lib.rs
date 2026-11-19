//! The time of day, in the machine's own time zone, so the assistant can say what day it is and `date` works.
//! The zone comes from `TZ` or `/etc/localtime` (the standard compiled zone files, read by our own parser, including
//! the rule at the end of the file that covers years after the last listed change). Without one the time is UTC.
//! The person can choose a zone in Settings -> Time: that name wins over `TZ` and `/etc/localtime`, and is found in
//! the zone files when the machine has them, else in the small built-in table of common zones ([`BUILTIN_ZONES`]),
//! so the image needs no tzdata package.

use std::path::Path;

const DAY: i64 = 86_400;
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

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
    pub yday: u32,
    pub offset: i32,
    pub abbrev: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TType {
    off: i32,
    dst: bool,
    abbrev: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    month: u32,
    week: u32,
    wday: u32,
    secs: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Posix {
    std: TType,
    dst: Option<(TType, Rule, Rule)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zone {
    trans: Vec<(i64, usize)>,
    types: Vec<TType>,
    footer: Option<Posix>,
}

/// Days since 1970-01-01 → (year, month 1-12, day 1-31).
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

fn parse_offset(s: &str) -> Option<(i32, &str)> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'-' => (-1, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (1, s),
    };
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == ':')).unwrap_or(rest.len());
    let mut parts = rest[..end].split(':').map(|p| p.parse::<i32>().ok());
    let h = parts.next()??;
    let m = parts.next().unwrap_or(Some(0))?;
    let sec = parts.next().unwrap_or(Some(0))?;
    Some((sign * (h * 3600 + m * 60 + sec), &rest[end..]))
}

fn parse_name(s: &str) -> Option<(String, &str)> {
    if let Some(r) = s.strip_prefix('<') {
        let e = r.find('>')?;
        return Some((r[..e].to_string(), &r[e + 1..]));
    }
    let e = s.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(s.len());
    (e >= 3).then(|| (s[..e].to_string(), &s[e..]))
}

fn parse_rule(s: &str) -> Option<Rule> {
    let (spec, time) = s.split_once('/').map_or((s, None), |(a, b)| (a, Some(b)));
    let spec = spec.strip_prefix('M')?;
    let mut p = spec.split('.').map(|x| x.parse::<u32>().ok());
    let (month, week, wday) = (p.next()??, p.next()??, p.next()??);
    if !(1..=12).contains(&month) || !(1..=5).contains(&week) || wday > 6 {
        return None;
    }
    let secs = match time {
        Some(t) => parse_offset(t)?.0,
        None => 7200,
    };
    Some(Rule { month, week, wday, secs })
}

fn parse_posix(s: &str) -> Option<Posix> {
    let (sname, rest) = parse_name(s)?;
    let (soff, rest) = parse_offset(rest)?;
    let std = TType { off: -soff, dst: false, abbrev: sname };
    if rest.is_empty() {
        return Some(Posix { std, dst: None });
    }
    let (dname, rest) = parse_name(rest)?;
    let (doff, rest) = if rest.starts_with(',') || rest.is_empty() { (std.off + 3600, rest) } else { parse_offset(rest).map(|(o, r)| (-o, r))? };
    let dst = TType { off: doff, dst: true, abbrev: dname };
    let mut rules = rest.strip_prefix(',')?.split(',');
    let start = parse_rule(rules.next()?)?;
    let end = parse_rule(rules.next()?)?;
    Some(Posix { std, dst: Some((dst, start, end)) })
}

/// The UTC instant a rule fires in `year`, given the offset in force just before it.
fn rule_instant(r: &Rule, year: i64, before: i32) -> i64 {
    let first = days_from_civil(year, r.month, 1);
    let first_wd = ((first + 4).rem_euclid(7)) as u32;
    let mut day = first + ((r.wday + 7 - first_wd) % 7) as i64 + (r.week as i64 - 1) * 7;
    let next_month = if r.month == 12 { days_from_civil(year + 1, 1, 1) } else { days_from_civil(year, r.month + 1, 1) };
    while day >= next_month {
        day -= 7;
    }
    day * DAY + r.secs as i64 - before as i64
}

impl Posix {
    fn at(&self, t: i64) -> TType {
        let Some((dst, start, end)) = &self.dst else { return self.std.clone() };
        let year = civil((t + self.std.off as i64).div_euclid(DAY)).0;
        let s = rule_instant(start, year, self.std.off);
        let e = rule_instant(end, year, dst.off);
        let in_dst = if s < e { t >= s && t < e } else { t >= s || t < e };
        if in_dst { dst.clone() } else { self.std.clone() }
    }
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

impl Zone {
    pub fn utc() -> Zone {
        Zone { trans: vec![], types: vec![TType { off: 0, dst: false, abbrev: "UTC".into() }], footer: None }
    }

    /// A compiled zone file (TZif, version 2 or later preferred).
    pub fn parse(b: &[u8]) -> Option<Zone> {
        if b.get(..4)? != b"TZif" {
            return None;
        }
        let version = *b.get(4)?;
        let counts = |at: usize| -> Option<[usize; 6]> {
            let mut c = [0usize; 6];
            for (i, v) in c.iter_mut().enumerate() {
                *v = be32(b, at + 20 + i * 4)? as usize;
            }
            Some(c)
        };
        let c1 = counts(0)?;
        let (isut, isstd, leap, timecnt, typecnt, charcnt) = (c1[0], c1[1], c1[2], c1[3], c1[4], c1[5]);
        let v1len = timecnt * 4 + timecnt + typecnt * 6 + charcnt + leap * 8 + isstd + isut;
        let (base, wide) = if version >= b'2' { (44 + v1len, true) } else { (0, false) };
        let c = if wide { counts(base)? } else { c1 };
        let (isut, isstd, leap, timecnt, typecnt, charcnt) = (c[0], c[1], c[2], c[3], c[4], c[5]);
        let tsz = if wide { 8 } else { 4 };
        let mut p = base + 44;
        let mut times = Vec::new();
        for i in 0..timecnt {
            let at = p + i * tsz;
            times.push(if wide { i64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?) } else { be32(b, at)? as i32 as i64 });
        }
        p += timecnt * tsz;
        let idx = b.get(p..p + timecnt)?.to_vec();
        p += timecnt;
        let tt = b.get(p..p + typecnt * 6)?;
        p += typecnt * 6;
        let chars = b.get(p..p + charcnt)?;
        p += charcnt + leap * (tsz + 4) + isstd + isut;
        let types: Vec<TType> = (0..typecnt)
            .map(|i| {
                let off = i32::from_be_bytes(tt[i * 6..i * 6 + 4].try_into().unwrap_or([0; 4]));
                let a = tt[i * 6 + 5] as usize;
                let end = chars[a.min(chars.len())..].iter().position(|&c| c == 0).map_or(chars.len(), |e| a + e);
                TType { off, dst: tt[i * 6 + 4] != 0, abbrev: String::from_utf8_lossy(&chars[a.min(chars.len())..end.max(a.min(chars.len()))]).into_owned() }
            })
            .collect();
        if types.is_empty() {
            return None;
        }
        let trans: Vec<(i64, usize)> = times.into_iter().zip(idx).map(|(t, i)| (t, (i as usize).min(types.len() - 1))).collect();
        let footer = if wide { b.get(p..).and_then(|f| std::str::from_utf8(f).ok()).and_then(|f| f.trim().lines().next().map(str::to_string)).and_then(|f| parse_posix(&f)) } else { None };
        Some(Zone { trans, types, footer })
    }

    /// A zone by its usual name ("Europe/Paris"): from the zone files when present, else the built-in table.
    pub fn named(name: &str) -> Option<Zone> {
        let name = name.trim();
        if name.is_empty() || name.contains("..") || name.starts_with('/') {
            return None;
        }
        if let Some(z) = std::fs::read(Path::new(&zoneinfo_dir()).join(name)).ok().and_then(|b| Zone::parse(&b)) {
            return Some(z);
        }
        BUILTIN_ZONES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|(_, rule)| Zone::from_posix(rule))
    }

    pub fn from_posix(s: &str) -> Option<Zone> {
        let p = parse_posix(s)?;
        Some(Zone { trans: vec![], types: vec![p.std.clone()], footer: Some(p) })
    }

    /// This machine's zone: `TZ` (a zone name or a rule), else `/etc/localtime`, else UTC.
    pub fn system() -> Zone {
        Zone::system_with(chosen_zone_name())
    }

    /// [`Zone::system`] with the person's choice given instead of read from the process-wide one (so it can be tested).
    pub fn system_with(chosen: Option<String>) -> Zone {
        if let Some(z) = chosen.and_then(|n| Zone::named(&n)) {
            return z;
        }
        if let Ok(tz) = std::env::var("TZ") {
            let name = tz.trim_start_matches(':');
            if !name.is_empty() && !name.contains("..") {
                if let Ok(b) = std::fs::read(Path::new("/usr/share/zoneinfo").join(name.trim_start_matches('/'))) {
                    if let Some(z) = Zone::parse(&b) {
                        return z;
                    }
                }
                if let Some(z) = Zone::from_posix(name) {
                    return z;
                }
            }
            if tz == "UTC" || tz.is_empty() {
                return Zone::utc();
            }
        }
        std::fs::read("/etc/localtime").ok().and_then(|b| Zone::parse(&b)).unwrap_or_else(Zone::utc)
    }

    fn at(&self, t: i64) -> TType {
        let after_last = self.trans.last().is_none_or(|(last, _)| t >= *last);
        if after_last {
            if let Some(f) = &self.footer {
                return f.at(t);
            }
        }
        match self.trans.partition_point(|(s, _)| *s <= t) {
            0 => self.types.iter().find(|x| !x.dst).unwrap_or(&self.types[0]).clone(),
            n => self.types[self.trans[n - 1].1].clone(),
        }
    }

    /// The UTC instant of a local date and time. In the hour that does not exist (clocks going forward) the later
    /// reading is used; in the hour that happens twice, the first.
    pub fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        let naive = days_from_civil(y, m, d) * DAY + hh as i64 * 3600 + mm as i64 * 60 + ss as i64;
        let mut t = naive - self.local(naive).offset as i64;
        for _ in 0..2 {
            t = naive - self.local(t).offset as i64;
        }
        // the offset before the change is the right one when both readings exist
        let before = naive - self.local(t - 4 * 3600).offset as i64;
        if self.local(before).hour == hh && self.local(before).minute == mm && self.local(before).day == d { before.min(t) } else { t }
    }

    pub fn local(&self, unix: i64) -> Local {
        let tt = self.at(unix);
        let l = unix + tt.off as i64;
        let days = l.div_euclid(DAY);
        let secs = l.rem_euclid(DAY) as u32;
        let (year, month, day) = civil(days);
        Local { year, month, day, hour: secs / 3600, minute: secs % 3600 / 60, second: secs % 60, weekday: (days + 4).rem_euclid(7) as u32, yday: (days - days_from_civil(year, 1, 1)) as u32 + 1, offset: tt.off, abbrev: tt.abbrev }
    }
}

/// The folder of compiled zone files (`IYAKKI_ZONEINFO` for tests; else the standard place).
fn zoneinfo_dir() -> String {
    std::env::var("IYAKKI_ZONEINFO").unwrap_or_else(|_| "/usr/share/zoneinfo".into())
}

static CHOSEN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The zone the person chose in Settings -> Time (applied at start and when it is changed); `""` clears it.
pub fn set_chosen_zone(name: &str) {
    if let Ok(mut c) = CHOSEN.lock() {
        *c = (!name.trim().is_empty()).then(|| name.trim().to_string());
    }
}

pub fn chosen_zone_name() -> Option<String> {
    CHOSEN.lock().ok().and_then(|c| c.clone())
}

/// Common zones with their POSIX rules, for machines without zone files. Rules are the current ones (they change
/// rarely; a machine with zone files uses those instead).
pub const BUILTIN_ZONES: &[(&str, &str)] = &[
    ("UTC", "UTC0"),
    ("Atlantic/Reykjavik", "GMT0"),
    ("Europe/London", "GMT0BST,M3.5.0/1,M10.5.0"),
    ("Europe/Dublin", "GMT0IST,M3.5.0/1,M10.5.0"),
    ("Europe/Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    ("Europe/Paris", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Madrid", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Rome", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Amsterdam", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Brussels", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Vienna", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Stockholm", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Oslo", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Copenhagen", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Prague", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Budapest", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Bucharest", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Istanbul", "<+03>-3"),
    ("Europe/Moscow", "MSK-3"),
    ("Africa/Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"),
    ("Africa/Lagos", "WAT-1"),
    ("Africa/Johannesburg", "SAST-2"),
    ("Africa/Nairobi", "EAT-3"),
    ("Asia/Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0"),
    ("Asia/Riyadh", "<+03>-3"),
    ("Asia/Tehran", "<+0330>-3:30"),
    ("Asia/Dubai", "<+04>-4"),
    ("Asia/Karachi", "PKT-5"),
    ("Asia/Kolkata", "IST-5:30"),
    ("Asia/Colombo", "<+0530>-5:30"),
    ("Asia/Kathmandu", "<+0545>-5:45"),
    ("Asia/Dhaka", "<+06>-6"),
    ("Asia/Bangkok", "<+07>-7"),
    ("Asia/Jakarta", "WIB-7"),
    ("Asia/Shanghai", "CST-8"),
    ("Asia/Hong_Kong", "HKT-8"),
    ("Asia/Singapore", "<+08>-8"),
    ("Asia/Manila", "PST-8"),
    ("Asia/Taipei", "CST-8"),
    ("Asia/Tokyo", "JST-9"),
    ("Asia/Seoul", "KST-9"),
    ("Australia/Perth", "AWST-8"),
    ("Australia/Darwin", "ACST-9:30"),
    ("Australia/Adelaide", "ACST-9:30ACDT,M10.1.0,M4.1.0/3"),
    ("Australia/Brisbane", "AEST-10"),
    ("Australia/Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Australia/Melbourne", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Pacific/Auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3"),
    ("Pacific/Honolulu", "HST10"),
    ("America/Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Vancouver", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Denver", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Phoenix", "MST7"),
    ("America/Chicago", "CST6CDT,M3.2.0,M11.1.0"),
    ("America/Mexico_City", "CST6"),
    ("America/New_York", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Toronto", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Halifax", "AST4ADT,M3.2.0,M11.1.0"),
    ("America/St_Johns", "NST3:30NDT,M3.2.0,M11.1.0"),
    ("America/Bogota", "<-05>5"),
    ("America/Lima", "<-05>5"),
    ("America/Santiago", "<-04>4<-03>,M9.1.6/24,M4.1.6/24"),
    ("America/Sao_Paulo", "<-03>3"),
    ("America/Argentina/Buenos_Aires", "<-03>3"),
];

/// Does the machine have its own zone files (`zone1970.tab` and the compiled zones)? Otherwise the built-in table is used.
pub fn has_zone_files() -> bool {
    ["zone1970.tab", "zone.tab"].iter().any(|f| std::fs::read_to_string(Path::new(&zoneinfo_dir()).join(f)).is_ok_and(|t| t.lines().filter(|l| !l.starts_with('#')).count() >= 20))
}

/// The zones to offer: every one in the zone files' index when the machine has them, else the built-in table. Sorted.
pub fn zone_names() -> Vec<String> {
    // `zone1970.tab`, else the older `zone.tab` (both list the zone in the third column)
    let mut v: Vec<String> = ["zone1970.tab", "zone.tab"]
        .iter()
        .find_map(|f| std::fs::read_to_string(Path::new(&zoneinfo_dir()).join(f)).ok())
        .map(|t| t.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(2)).map(str::to_string).collect())
        .unwrap_or_default();
    if v.len() < 20 {
        v = BUILTIN_ZONES.iter().map(|(n, _)| n.to_string()).collect();
    }
    if !v.iter().any(|n| n == "UTC") {
        v.push("UTC".into());
    }
    v.sort();
    v.dedup();
    v
}

/// The earliest date a working clock can show: the machine was built after it. A clock before this (a Mac mini
/// without its clock battery starts at 1970) has not been set.
pub const EARLIEST_SANE: i64 = 1_704_067_200; // 2024-01-01

pub fn looks_wrong(unix: i64) -> bool {
    unix < EARLIEST_SANE
}

/// The time for the top bar: `14:05`, `2:05 PM`, with seconds when asked.
pub fn bar_time(z: &Zone, unix: i64, hour12: bool, seconds: bool) -> String {
    let l = z.local(unix);
    let secs = if seconds { format!(":{:02}", l.second) } else { String::new() };
    if hour12 {
        format!("{}:{:02}{secs} {}", if l.hour % 12 == 0 { 12 } else { l.hour % 12 }, l.minute, if l.hour < 12 { "AM" } else { "PM" })
    } else {
        format!("{:02}:{:02}{secs}", l.hour, l.minute)
    }
}

/// The date for the top bar: `Sat 17 Oct`, or with `long` the full `Saturday, 17 October 2026`.
pub fn bar_date(z: &Zone, unix: i64, long: bool) -> String {
    let l = z.local(unix);
    if long {
        format!("{}, {} {} {}", WEEKDAYS[l.weekday as usize], l.day, MONTHS[l.month as usize - 1], l.year)
    } else {
        format!("{} {} {}", &WEEKDAYS[l.weekday as usize][..3], l.day, &MONTHS[l.month as usize - 1][..3])
    }
}

/// Today's date as `2026-10-17` (UTC), for change logs.
pub fn today() -> String {
    let (y, m, d) = civil(now_unix().div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn offset_text(o: i32, colon: bool) -> String {
    let a = o.abs();
    format!("{}{:02}{}{:02}", if o < 0 { '-' } else { '+' }, a / 3600, if colon { ":" } else { "" }, a % 3600 / 60)
}

/// The sentence the assistant is given and the `now` tool returns: `Sunday, 11 October 2026, 14:03:09 PDT (UTC-07:00)`.
pub fn describe(z: &Zone, unix: i64) -> String {
    let l = z.local(unix);
    format!("{}, {} {} {}, {:02}:{:02}:{:02} {} (UTC{}), week day number {}, day {} of the year, unix time {}", WEEKDAYS[l.weekday as usize], l.day, MONTHS[l.month as usize - 1], l.year, l.hour, l.minute, l.second, l.abbrev, offset_text(l.offset, true), l.weekday, l.yday, unix)
}

/// `date +FORMAT`: the usual `%` codes.
pub fn format(z: &Zone, unix: i64, fmt: &str) -> String {
    let l = z.local(unix);
    let h12 = if l.hour % 12 == 0 { 12 } else { l.hour % 12 };
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&l.year.to_string()),
            Some('y') => out.push_str(&format!("{:02}", l.year.rem_euclid(100))),
            Some('m') => out.push_str(&format!("{:02}", l.month)),
            Some('d') => out.push_str(&format!("{:02}", l.day)),
            Some('e') => out.push_str(&format!("{:2}", l.day)),
            Some('H') => out.push_str(&format!("{:02}", l.hour)),
            Some('I') => out.push_str(&format!("{h12:02}")),
            Some('M') => out.push_str(&format!("{:02}", l.minute)),
            Some('S') => out.push_str(&format!("{:02}", l.second)),
            Some('p') => out.push_str(if l.hour < 12 { "AM" } else { "PM" }),
            Some('A') => out.push_str(WEEKDAYS[l.weekday as usize]),
            Some('a') => out.push_str(&WEEKDAYS[l.weekday as usize][..3]),
            Some('B') => out.push_str(MONTHS[l.month as usize - 1]),
            Some('b') | Some('h') => out.push_str(&MONTHS[l.month as usize - 1][..3]),
            Some('j') => out.push_str(&format!("{:03}", l.yday)),
            Some('u') => out.push_str(&(if l.weekday == 0 { 7 } else { l.weekday }).to_string()),
            Some('w') => out.push_str(&l.weekday.to_string()),
            Some('Z') => out.push_str(&l.abbrev),
            Some('z') => out.push_str(&offset_text(l.offset, false)),
            Some('s') => out.push_str(&unix.to_string()),
            Some('F') => out.push_str(&format!("{}-{:02}-{:02}", l.year, l.month, l.day)),
            Some('T') => out.push_str(&format!("{:02}:{:02}:{:02}", l.hour, l.minute, l.second)),
            Some('R') => out.push_str(&format!("{:02}:{:02}", l.hour, l.minute)),
            Some('D') => out.push_str(&format!("{:02}/{:02}/{:02}", l.month, l.day, l.year.rem_euclid(100))),
            Some('c') => out.push_str(&format!("{} {} {:2} {:02}:{:02}:{:02} {}", &WEEKDAYS[l.weekday as usize][..3], &MONTHS[l.month as usize - 1][..3], l.day, l.hour, l.minute, l.second, l.year)),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip_and_known_days_are_right() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil(days_from_civil(2026, 10, 11)), (2026, 10, 11));
        for d in (-800_000..800_000).step_by(997) {
            let (y, m, dd) = civil(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        let z = Zone::utc();
        assert_eq!(z.local(0).weekday, 4, "1970-01-01 was a Thursday");
        assert_eq!(format(&z, 1_000_000_000, "%F %T %A %j"), "2001-09-09 01:46:40 Sunday 252");
        // 2026-10-11 is a Sunday
        assert_eq!(z.local(days_from_civil(2026, 10, 11) * DAY + 3600).weekday, 0);
    }

    #[test]
    fn a_posix_rule_gives_daylight_saving_in_the_right_weeks() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let at = |y: i64, m: u32, d: u32, h: i64| days_from_civil(y, m, d) * DAY + h * 3600;
        assert_eq!(z.local(at(2026, 7, 4, 12)).abbrev, "PDT");
        assert_eq!(z.local(at(2026, 7, 4, 12)).offset, -7 * 3600);
        assert_eq!(z.local(at(2026, 1, 4, 12)).abbrev, "PST");
        // DST starts Sunday 8 March 2026 at 02:00 local = 10:00 UTC
        assert_eq!(z.local(at(2026, 3, 8, 9)).abbrev, "PST");
        assert_eq!(z.local(at(2026, 3, 8, 10)).abbrev, "PDT");
        // and ends Sunday 1 November 2026 at 02:00 PDT = 09:00 UTC
        assert_eq!(z.local(at(2026, 11, 1, 8)).abbrev, "PDT");
        assert_eq!(z.local(at(2026, 11, 1, 9)).abbrev, "PST");
        let ist = Zone::from_posix("<+0530>-5:30").unwrap();
        let l = ist.local(at(2026, 10, 11, 0));
        assert_eq!((l.hour, l.minute, l.offset), (5, 30, 19_800));
        assert!(Zone::from_posix("nonsense").is_none() && Zone::from_posix("").is_none());
    }

    #[test]
    fn this_machines_zone_file_is_read_when_there_is_one() {
        let Ok(b) = std::fs::read("/usr/share/zoneinfo/America/Los_Angeles") else { return };
        let z = Zone::parse(&b).expect("a real zone file parses");
        let at = |y: i64, m: u32, d: u32, h: i64| days_from_civil(y, m, d) * DAY + h * 3600;
        assert_eq!(z.local(at(2026, 7, 4, 12)).abbrev, "PDT");
        assert_eq!(z.local(at(2026, 1, 4, 12)).abbrev, "PST");
        assert_eq!(z.local(at(2050, 7, 4, 12)).abbrev, "PDT", "years after the last listed change use the rule at the end of the file");
        assert_eq!(z.local(at(1999, 12, 31, 12)).offset, -8 * 3600);
        let kol = std::fs::read("/usr/share/zoneinfo/Asia/Kolkata").ok().and_then(|b| Zone::parse(&b));
        if let Some(k) = kol {
            assert_eq!(k.local(at(2026, 10, 11, 0)).offset, 19_800);
        }
        assert!(Zone::parse(b"not a zone").is_none() && Zone::parse(b"TZif").is_none());
    }

    #[test]
    fn a_local_time_converts_back_to_the_instant_across_daylight_saving() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        // winter and summer
        assert_eq!(z.local_to_unix(2026, 1, 15, 9, 30, 0), days_from_civil(2026, 1, 15) * DAY + 17 * 3600 + 30 * 60);
        assert_eq!(z.local_to_unix(2026, 7, 4, 9, 30, 0), days_from_civil(2026, 7, 4) * DAY + 16 * 3600 + 30 * 60);
        // every local minute of a few days round-trips, including the day clocks go forward and back
        for (y, m, d) in [(2026, 3, 8), (2026, 3, 9), (2026, 11, 1), (2026, 11, 2)] {
            for hh in 0..24u32 {
                let t = z.local_to_unix(y, m, d, hh, 15, 0);
                let l = z.local(t);
                if (y, m, d, hh) == (2026, 3, 8, 2) {
                    assert_eq!((l.hour, l.minute), (3, 15), "02:15 does not exist on 8 March; it lands in the hour after");
                } else {
                    assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (y, m, d, hh, 15), "{y}-{m}-{d} {hh}");
                }
            }
        }
        // 01:30 on 1 November happens twice; the first one (PDT) is used
        assert_eq!(z.local(z.local_to_unix(2026, 11, 1, 1, 30, 0)).abbrev, "PDT");
        assert_eq!(Zone::utc().local_to_unix(1970, 1, 2, 0, 0, 0), 86_400);
    }

    #[test]
    fn date_formats_and_the_sentence_for_the_assistant() {
        let z = Zone::utc();
        let t = days_from_civil(2026, 10, 11) * DAY + 14 * 3600 + 3 * 60 + 9;
        assert_eq!(format(&z, t, "%A, %d %B %Y %I:%M %p %Z %z %% %q"), "Sunday, 11 October 2026 02:03 PM UTC +0000 % %q");
        assert_eq!(format(&z, t, "%a %b %e %y %H:%M:%S %j %u %w"), "Sun Oct 11 26 14:03:09 284 7 0");
        let d = describe(&z, t);
        assert!(d.starts_with("Sunday, 11 October 2026, 14:03:09 UTC (UTC+00:00)"), "{d}");
    }

    #[test]
    fn the_bar_shows_24_or_12_hour_time_with_optional_seconds_and_a_short_or_long_date() {
        let z = Zone::utc();
        let at = |h: i64, m: i64| days_from_civil(2026, 10, 17) * DAY + h * 3600 + m * 60 + 9;
        assert_eq!(bar_time(&z, at(7, 5), false, false), "07:05");
        assert_eq!(bar_time(&z, at(7, 5), false, true), "07:05:09");
        assert_eq!(bar_time(&z, at(0, 5), true, false), "12:05 AM");
        assert_eq!(bar_time(&z, at(12, 0), true, false), "12:00 PM");
        assert_eq!(bar_time(&z, at(15, 30), true, true), "3:30:09 PM");
        assert_eq!(bar_date(&z, at(7, 5), false), "Sat 17 Oct");
        assert_eq!(bar_date(&z, at(7, 5), true), "Saturday, 17 October 2026");
    }

    #[test]
    fn every_built_in_zone_parses_and_gives_the_right_local_time_and_daylight_saving() {
        let at = |y: i64, m: u32, d: u32, h: i64| days_from_civil(y, m, d) * DAY + h * 3600;
        for (name, rule) in BUILTIN_ZONES {
            assert!(Zone::from_posix(rule).is_some(), "{name}: {rule}");
        }
        std::env::set_var("IYAKKI_ZONEINFO", "/nonexistent");
        let tz = |n: &str| Zone::named(n).unwrap_or_else(|| panic!("{n}"));
        assert_eq!(tz("Asia/Kolkata").local(at(2026, 10, 17, 0)).hour, 5);
        assert_eq!(tz("Asia/Kolkata").local(at(2026, 10, 17, 0)).minute, 30);
        assert_eq!(tz("Asia/Kathmandu").local(at(2026, 10, 17, 0)).minute, 45);
        assert_eq!(tz("America/New_York").local(at(2026, 7, 1, 12)).hour, 8, "EDT");
        assert_eq!(tz("America/New_York").local(at(2026, 1, 1, 12)).hour, 7, "EST");
        assert_eq!(tz("Europe/London").local(at(2026, 7, 1, 12)).hour, 13, "BST");
        assert_eq!(tz("Europe/Berlin").local(at(2026, 3, 29, 0) + 3600).hour, 3, "clocks went forward at 01:00 UTC on 29 March");
        assert_eq!(tz("Australia/Sydney").local(at(2026, 1, 1, 0)).hour, 11, "southern summer");
        assert_eq!(tz("Australia/Sydney").local(at(2026, 7, 1, 0)).hour, 10);
        assert_eq!(tz("America/Santiago").local(at(2026, 1, 1, 12)).hour, 9, "-03 in summer");
        assert_eq!(tz("Asia/Tokyo").local(at(2026, 10, 17, 0)).hour, 9);
        assert!(Zone::named("Mars/Olympus").is_none() && Zone::named("../etc/passwd").is_none() && Zone::named("").is_none());
        assert!(zone_names().len() >= BUILTIN_ZONES.len() && zone_names().contains(&"Europe/Paris".to_string()));
        std::env::remove_var("IYAKKI_ZONEINFO");
    }

    #[test]
    fn a_chosen_zone_wins_and_a_clock_in_1970_looks_wrong() {
        std::env::set_var("IYAKKI_ZONEINFO", "/nonexistent");
        assert_eq!(Zone::system_with(Some("Asia/Tokyo".into())).local(0).hour, 9);
        assert_eq!(Zone::system_with(Some("Nowhere/Land".into())).local(0).hour, Zone::system_with(None).local(0).hour, "an unknown choice falls back to the machine's zone");
        std::env::remove_var("IYAKKI_ZONEINFO");
        assert!(looks_wrong(0) && looks_wrong(1_700_000_000) && !looks_wrong(1_790_000_000));
    }
}
