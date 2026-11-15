//! A built-in time zone table for cron triggers (F-CLOUD-71): the zones people schedule in, with their CURRENT daylight saving rules as
//! POSIX TZ strings (the same text a zone file ends with), and an evaluator for those strings. It is used when the image has no zone files
//! (the shipped `FROM scratch` container); a system zone file, when there is one, wins because it also knows the past.
//!
//! What it does not know: rules before the ones in force now (a time stamp in 2005 gets today's rule), and zones not in the table (they
//! still fall back to UTC with the visible warning). The names are the IANA names; a few common old aliases are listed too.

use iyakki_flow::sched::Tz;

/// (zone name, POSIX TZ string).
pub const TABLE: &[(&str, &str)] = &[
    ("UTC", "UTC0"), ("Etc/UTC", "UTC0"), ("GMT", "GMT0"), ("Etc/GMT", "GMT0"),
    ("America/New_York", "EST5EDT,M3.2.0,M11.1.0"), ("US/Eastern", "EST5EDT,M3.2.0,M11.1.0"), ("America/Toronto", "EST5EDT,M3.2.0,M11.1.0"), ("America/Detroit", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Chicago", "CST6CDT,M3.2.0,M11.1.0"), ("US/Central", "CST6CDT,M3.2.0,M11.1.0"), ("America/Winnipeg", "CST6CDT,M3.2.0,M11.1.0"),
    ("America/Denver", "MST7MDT,M3.2.0,M11.1.0"), ("US/Mountain", "MST7MDT,M3.2.0,M11.1.0"), ("America/Edmonton", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Phoenix", "MST7"), ("America/Regina", "CST6"),
    ("America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0"), ("US/Pacific", "PST8PDT,M3.2.0,M11.1.0"), ("America/Vancouver", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"), ("Pacific/Honolulu", "HST10"), ("US/Hawaii", "HST10"),
    ("America/Halifax", "AST4ADT,M3.2.0,M11.1.0"), ("America/St_Johns", "NST3:30NDT,M3.2.0,M11.1.0"),
    ("America/Mexico_City", "CST6"), ("America/Guatemala", "CST6"), ("America/Costa_Rica", "CST6"), ("America/Panama", "EST5"), ("America/Jamaica", "EST5"),
    ("America/Havana", "CST5CDT,M3.2.0/0,M11.1.0/1"),
    ("America/Bogota", "<-05>5"), ("America/Lima", "<-05>5"), ("America/Caracas", "<-04>4"), ("America/La_Paz", "<-04>4"),
    ("America/Santiago", "<-04>4<-03>,M9.1.6/24,M4.1.6/24"), ("America/Sao_Paulo", "<-03>3"), ("America/Argentina/Buenos_Aires", "<-03>3"), ("America/Buenos_Aires", "<-03>3"),
    ("America/Montevideo", "<-03>3"), ("America/Asuncion", "<-03>3"),
    ("Europe/London", "GMT0BST,M3.5.0/1,M10.5.0"), ("Europe/Dublin", "IST-1GMT0,M10.5.0,M3.5.0/1"), ("Europe/Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"), ("Atlantic/Reykjavik", "GMT0"),
    ("Europe/Paris", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Madrid", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Rome", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Amsterdam", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Brussels", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Vienna", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Stockholm", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Oslo", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Copenhagen", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Prague", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Budapest", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Belgrade", "CET-1CEST,M3.5.0,M10.5.0/3"), ("Europe/Zagreb", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Kiev", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Bucharest", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Sofia", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Riga", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Vilnius", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Tallinn", "EET-2EEST,M3.5.0/3,M10.5.0/4"), ("Europe/Istanbul", "<+03>-3"), ("Europe/Moscow", "MSK-3"), ("Europe/Minsk", "<+03>-3"),
    ("Africa/Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"), ("Africa/Johannesburg", "SAST-2"), ("Africa/Lagos", "WAT-1"), ("Africa/Nairobi", "EAT-3"), ("Africa/Algiers", "CET-1"),
    ("Africa/Accra", "GMT0"), ("Africa/Addis_Ababa", "EAT-3"), ("Africa/Casablanca", "<+01>-1"),
    ("Asia/Dubai", "<+04>-4"), ("Asia/Riyadh", "<+03>-3"), ("Asia/Baghdad", "<+03>-3"), ("Asia/Tehran", "<+0330>-3:30"), ("Asia/Kabul", "<+0430>-4:30"),
    ("Asia/Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0"), ("Asia/Karachi", "PKT-5"), ("Asia/Tashkent", "<+05>-5"), ("Asia/Almaty", "<+05>-5"),
    ("Asia/Kolkata", "IST-5:30"), ("Asia/Calcutta", "IST-5:30"), ("Asia/Colombo", "<+0530>-5:30"), ("Asia/Kathmandu", "<+0545>-5:45"), ("Asia/Dhaka", "<+06>-6"), ("Asia/Yangon", "<+0630>-6:30"),
    ("Asia/Bangkok", "<+07>-7"), ("Asia/Ho_Chi_Minh", "<+07>-7"), ("Asia/Jakarta", "WIB-7"), ("Asia/Singapore", "<+08>-8"), ("Asia/Kuala_Lumpur", "<+08>-8"),
    ("Asia/Hong_Kong", "HKT-8"), ("Asia/Shanghai", "CST-8"), ("Asia/Taipei", "CST-8"), ("Asia/Manila", "PST-8"), ("Australia/Perth", "AWST-8"),
    ("Asia/Seoul", "KST-9"), ("Asia/Tokyo", "JST-9"), ("Australia/Darwin", "ACST-9:30"), ("Australia/Adelaide", "ACST-9:30ACDT,M10.1.0,M4.1.0/3"),
    ("Australia/Brisbane", "AEST-10"), ("Australia/Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"), ("Australia/Melbourne", "AEST-10AEDT,M10.1.0,M4.1.0/3"), ("Australia/Hobart", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Pacific/Guam", "ChST-10"), ("Pacific/Fiji", "<+12>-12"), ("Pacific/Auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3"), ("Pacific/Tongatapu", "<+13>-13"),
];

/// A parsed rule: `Mm.w.d`, `Jn` (1 to 365, February 29 never counted) or `n` (0 to 365, counted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Day {
    Month { m: u32, w: u32, d: u32 },
    Julian1(u32),
    Julian0(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    day: Day,
    /// Seconds after midnight of the wall clock then in force (may be negative or over 24 hours).
    time: i32,
}

/// A POSIX TZ string, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posix {
    /// Seconds EAST of UTC in standard time (the string's sign is the other way round).
    std: i32,
    dst: Option<(i32, Rule, Rule)>,
}

fn name(s: &mut &str) -> Option<()> {
    if let Some(r) = s.strip_prefix('<') {
        let end = r.find('>')?;
        if end == 0 || !r[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-') {
            return None;
        }
        *s = &r[end + 1..];
        return Some(());
    }
    let n = s.chars().take_while(char::is_ascii_alphabetic).count();
    if n < 3 {
        return None;
    }
    *s = &s[n..];
    Some(())
}

fn number(s: &mut &str) -> Option<u32> {
    let n = s.chars().take_while(char::is_ascii_digit).count();
    if n == 0 || n > 4 {
        return None;
    }
    let v = s[..n].parse().ok()?;
    *s = &s[n..];
    Some(v)
}

/// `[+-]hh[:mm[:ss]]` in seconds.
fn hms(s: &mut &str) -> Option<i32> {
    let neg = if let Some(r) = s.strip_prefix('-') {
        *s = r;
        true
    } else {
        if let Some(r) = s.strip_prefix('+') {
            *s = r;
        }
        false
    };
    let h = number(s)?;
    let (mut m, mut sec) = (0, 0);
    if let Some(r) = s.strip_prefix(':') {
        *s = r;
        m = number(s)?;
        if let Some(r) = s.strip_prefix(':') {
            *s = r;
            sec = number(s)?;
        }
    }
    if h > 167 || m > 59 || sec > 59 {
        return None;
    }
    let t = (h * 3600 + m * 60 + sec) as i32;
    Some(if neg { -t } else { t })
}

fn rule(s: &mut &str) -> Option<Rule> {
    let day = if let Some(r) = s.strip_prefix('M') {
        *s = r;
        let m = number(s)?;
        *s = s.strip_prefix('.')?;
        let w = number(s)?;
        *s = s.strip_prefix('.')?;
        let d = number(s)?;
        if !(1..=12).contains(&m) || !(1..=5).contains(&w) || d > 6 {
            return None;
        }
        Day::Month { m, w, d }
    } else if let Some(r) = s.strip_prefix('J') {
        *s = r;
        let n = number(s)?;
        if !(1..=365).contains(&n) {
            return None;
        }
        Day::Julian1(n)
    } else {
        let n = number(s)?;
        if n > 365 {
            return None;
        }
        Day::Julian0(n)
    };
    let time = if let Some(r) = s.strip_prefix('/') {
        *s = r;
        hms(s)?
    } else {
        7200
    };
    Some(Rule { day, time })
}

impl Posix {
    pub fn parse(text: &str) -> Option<Posix> {
        let mut s = text.trim();
        name(&mut s)?;
        let std = -hms(&mut s)?;
        if s.is_empty() {
            return Some(Posix { std, dst: None });
        }
        name(&mut s)?;
        let dst = if s.is_empty() || s.starts_with(',') { std + 3600 } else { -hms(&mut s)? };
        // without rules the US rule applies; every table entry spells its rules out, so a string without them is refused
        s = s.strip_prefix(',')?;
        let a = rule(&mut s)?;
        s = s.strip_prefix(',')?;
        let b = rule(&mut s)?;
        if !s.is_empty() {
            return None;
        }
        Some(Posix { std, dst: Some((dst, a, b)) })
    }

    /// Offset east of UTC, in seconds, at this instant.
    pub fn offset(&self, unix: i64) -> i32 {
        let Some((dst, start, end)) = &self.dst else { return self.std };
        let year = year_of(unix + i64::from(self.std));
        // the start is a wall-clock time in standard time, the end one in daylight time
        let s = day_start(year, start.day) + i64::from(start.time) - i64::from(self.std);
        let e = day_start(year, end.day) + i64::from(end.time) - i64::from(*dst);
        let in_dst = if s < e { unix >= s && unix < e } else { !(unix >= e && unix < s) };
        if in_dst { *dst } else { self.std }
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn year_of(secs: i64) -> i64 {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

/// Midnight (as seconds since the epoch, wall clock read as UTC) of the day a rule names in this year.
fn day_start(year: i64, d: Day) -> i64 {
    let days = match d {
        Day::Julian1(n) => {
            let n = i64::from(n);
            // J60 is always March 1: after February in a leap year, skip the 29th
            days_from_civil(year, 1, 1) + n - 1 + i64::from(is_leap(year) && n >= 60)
        }
        Day::Julian0(n) => days_from_civil(year, 1, 1) + i64::from(n),
        Day::Month { m, w, d } => {
            let first = days_from_civil(year, m, 1);
            // weekday of the first (0 = Sunday); 1970-01-01 was a Thursday
            let wd = (first + 4).rem_euclid(7) as u32;
            let mut day = 1 + (d + 7 - wd) % 7 + (w - 1) * 7;
            let len = [31, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m as usize - 1];
            while day > len {
                day -= 7;
            }
            first + i64::from(day) - 1
        }
    };
    days * 86_400
}

/// `Etc/GMT+5` is five hours WEST of UTC (the sign is the other way round, on purpose in the IANA names).
fn etc_gmt(name: &str) -> Option<Posix> {
    let r = name.strip_prefix("Etc/GMT")?;
    let (sign, n) = (r.chars().next()?, r.get(1..)?);
    let h: i32 = n.parse().ok()?;
    if !(1..=14).contains(&h) || n.starts_with('0') {
        return None;
    }
    match sign {
        '+' if h <= 12 => Some(Posix { std: -h * 3600, dst: None }),
        '-' => Some(Posix { std: h * 3600, dst: None }),
        _ => None,
    }
}

struct Builtin(Posix);
impl Tz for Builtin {
    fn offset_secs(&self, unix: i64) -> i32 {
        self.0.offset(unix)
    }
}

/// The zone of the table, ignoring the case of the name.
pub fn builtin(name: &str) -> Option<std::sync::Arc<dyn Tz>> {
    let n = name.trim();
    let p = TABLE.iter().find(|(z, _)| z.eq_ignore_ascii_case(n)).and_then(|(_, s)| Posix::parse(s)).or_else(|| etc_gmt(n))?;
    Some(std::sync::Arc::new(Builtin(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-01-01T00:00:00Z
    const Y2026: i64 = 1_767_225_600;
    fn at(y: i64, m: u32, d: u32, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
    }
    fn off(zone: &str, unix: i64) -> i32 {
        builtin(zone).unwrap_or_else(|| panic!("{zone}")).offset_secs(unix) / 60
    }

    #[test]
    fn the_calendar_arithmetic_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(at(2026, 1, 1, 0, 0), Y2026);
        assert_eq!(year_of(Y2026 - 1), 2025);
        assert_eq!(year_of(Y2026), 2026);
        assert_eq!(year_of(at(2024, 2, 29, 12, 0)), 2024);
        // 2026-03-08 is the second Sunday of March; 2026-11-01 the first of November
        assert_eq!(day_start(2026, Day::Month { m: 3, w: 2, d: 0 }), at(2026, 3, 8, 0, 0));
        assert_eq!(day_start(2026, Day::Month { m: 11, w: 1, d: 0 }), at(2026, 11, 1, 0, 0));
        // the "last" Sunday (week 5) of March 2026 is the 29th; of February 2024 (leap) the 25th for week 5 day 0
        assert_eq!(day_start(2026, Day::Month { m: 3, w: 5, d: 0 }), at(2026, 3, 29, 0, 0));
        assert_eq!(day_start(2024, Day::Month { m: 2, w: 5, d: 0 }), at(2024, 2, 25, 0, 0));
        assert_eq!(day_start(2024, Day::Julian1(60)), at(2024, 3, 1, 0, 0));
        assert_eq!(day_start(2024, Day::Julian0(60)), at(2024, 3, 1, 0, 0), "day 60 counting February 29");
        assert_eq!(day_start(2025, Day::Julian0(60)), at(2025, 3, 2, 0, 0));
        assert_eq!(day_start(2025, Day::Julian1(60)), at(2025, 3, 1, 0, 0));
    }

    #[test]
    fn new_york_changes_at_two_in_the_morning_local_time_on_the_right_sundays() {
        // 2026: spring forward 2026-03-08 02:00 EST = 07:00 UTC; fall back 2026-11-01 02:00 EDT = 06:00 UTC
        assert_eq!(off("America/New_York", at(2026, 3, 8, 6, 59)), -300);
        assert_eq!(off("America/New_York", at(2026, 3, 8, 7, 0)), -240);
        assert_eq!(off("America/New_York", at(2026, 7, 1, 12, 0)), -240);
        assert_eq!(off("America/New_York", at(2026, 11, 1, 5, 59)), -240);
        assert_eq!(off("America/New_York", at(2026, 11, 1, 6, 0)), -300);
        assert_eq!(off("America/New_York", at(2026, 12, 25, 12, 0)), -300);
        assert_eq!(off("us/pacific", at(2026, 7, 1, 12, 0)), -420, "names are not case sensitive");
    }

    #[test]
    fn the_zones_of_both_hemispheres_and_the_odd_offsets_are_right() {
        // Europe: last Sunday of March 01:00 UTC to the last Sunday of October 01:00 UTC
        assert_eq!(off("Europe/Berlin", at(2026, 3, 29, 0, 59)), 60);
        assert_eq!(off("Europe/Berlin", at(2026, 3, 29, 1, 0)), 120);
        assert_eq!(off("Europe/Berlin", at(2026, 10, 25, 0, 59)), 120);
        assert_eq!(off("Europe/Berlin", at(2026, 10, 25, 1, 0)), 60);
        assert_eq!(off("Europe/London", at(2026, 7, 1, 0, 0)), 60);
        assert_eq!(off("Europe/London", at(2026, 1, 1, 0, 0)), 0);
        // Ireland writes its winter time as the "daylight" one: the end comes before the start in the year, the answer is the same
        assert_eq!(off("Europe/Dublin", at(2026, 7, 1, 0, 0)), 60);
        assert_eq!(off("Europe/Dublin", at(2026, 1, 1, 0, 0)), 0);
        assert_eq!(off("Europe/Athens", at(2026, 7, 1, 0, 0)), 180);
        // southern hemisphere: summer time over New Year
        assert_eq!(off("Australia/Sydney", at(2026, 1, 1, 0, 0)), 660);
        assert_eq!(off("Australia/Sydney", at(2026, 7, 1, 0, 0)), 600);
        assert_eq!(off("Pacific/Auckland", at(2026, 1, 1, 0, 0)), 780);
        assert_eq!(off("Pacific/Auckland", at(2026, 7, 1, 0, 0)), 720);
        assert_eq!(off("America/Santiago", at(2026, 1, 1, 12, 0)), -180);
        assert_eq!(off("America/Santiago", at(2026, 7, 1, 12, 0)), -240);
        // no daylight saving, odd offsets
        assert_eq!(off("Asia/Kolkata", at(2026, 7, 1, 0, 0)), 330);
        assert_eq!(off("Asia/Kathmandu", at(2026, 7, 1, 0, 0)), 345);
        assert_eq!(off("Asia/Tehran", at(2026, 7, 1, 0, 0)), 210);
        assert_eq!(off("Asia/Tokyo", at(2026, 7, 1, 0, 0)), 540);
        assert_eq!(off("America/Phoenix", at(2026, 7, 1, 0, 0)), -420);
        assert_eq!(off("America/St_Johns", at(2026, 7, 1, 0, 0)), -150);
        assert_eq!(off("America/St_Johns", at(2026, 1, 1, 0, 0)), -210);
        // a rule with a time over 24 hours (Israel: 4th Thursday of March + 26 h = Friday 02:00) and one at 24:00 (Cairo, last Thursday of October)
        assert_eq!(off("Asia/Jerusalem", at(2026, 3, 26, 23, 59)), 120);
        assert_eq!(off("Asia/Jerusalem", at(2026, 3, 27, 0, 0)), 180);
        assert_eq!(off("Asia/Jerusalem", at(2026, 10, 24, 22, 59)), 180);
        assert_eq!(off("Asia/Jerusalem", at(2026, 10, 24, 23, 0)), 120);
        assert_eq!(off("Africa/Cairo", at(2026, 7, 1, 0, 0)), 180);
        assert_eq!(off("Africa/Cairo", at(2026, 1, 1, 0, 0)), 120);
    }

    #[test]
    fn etc_gmt_names_have_the_iana_sign_and_unknown_names_are_not_invented() {
        assert_eq!(off("Etc/GMT+5", Y2026), -300);
        assert_eq!(off("Etc/GMT-14", Y2026), 840);
        for bad in ["Etc/GMT+13", "Etc/GMT-15", "Etc/GMT+05", "Etc/GMT+", "Etc/GMT0", "Mars/Olympus", "", "America"] {
            assert!(builtin(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn every_table_entry_parses_and_the_parser_refuses_damaged_strings() {
        for (zone, s) in TABLE {
            assert!(Posix::parse(s).is_some(), "{zone}: {s}");
        }
        let mut names: Vec<&str> = TABLE.iter().map(|(z, _)| *z).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "a zone listed twice");
        for bad in ["", "EST", "EST5EDT", "EST5EDT,M3.2.0", "EST5EDT,M13.2.0,M11.1.0", "EST5EDT,M3.6.0,M11.1.0", "EST5EDT,M3.2.7,M11.1.0", "EST5EDT,M3.2.0,M11.1.0,x", "<>5", "<a b>5", "E5", "EST5:60", "EST999"] {
            assert!(Posix::parse(bad).is_none(), "{bad}");
        }
        let p = Posix::parse("EST5EDT,J60/-1,300/3:30:15").unwrap();
        assert_eq!(p.std, -18_000);
        assert_eq!(p.dst.unwrap().0, -14_400);
    }
}
