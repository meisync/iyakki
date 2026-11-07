//! UUID v4 / v7 generation and the timestamp converter (epoch seconds or milliseconds, ISO 8601, "now").

use iyakki_agent::clock::{civil, days_from_civil, Zone};

/// Random bytes from the system (the agent's generator: a tile does not read the kernel's devices itself).
fn random<const N: usize>() -> [u8; N] {
    let hex = iyakki_agent::sharedfolder::random_hex(N);
    let mut b = [0u8; N];
    for (i, x) in b.iter_mut().enumerate() {
        *x = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    b
}

fn fmt(b: [u8; 16]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub fn uuid_v4() -> String {
    let mut b = random::<16>();
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    fmt(b)
}

/// RFC 9562 version 7: 48 bits of Unix milliseconds, then random bits.
pub fn uuid_v7(ms: u64) -> String {
    let mut b = random::<16>();
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = b[6] & 0x0f | 0x70;
    b[8] = b[8] & 0x3f | 0x80;
    fmt(b)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub fn gen(v7: bool, n: usize) -> Result<String, String> {
    Ok((0..n).map(|_| if v7 { uuid_v7(now_ms() as u64) } else { uuid_v4() }).collect::<Vec<_>>().join("\n"))
}

/// `2024-05-06T07:08:09.123Z` for Unix milliseconds.
pub fn iso_utc(ms: i64) -> String {
    let (s, f) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (y, m, d) = civil(s.div_euclid(86_400));
    let r = s.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{f:03}Z", r / 3600, r % 3600 / 60, r % 60)
}

/// The seconds form used by the JWT page.
pub fn iso_utc_s(s: i64) -> String {
    iso_utc(s * 1000).replace(".000Z", "Z")
}

/// A date and time with a zone, in the system zone.
pub fn local_text(ms: i64) -> String {
    let l = Zone::system().local(ms.div_euclid(1000));
    let sign = if l.offset < 0 { '-' } else { '+' };
    let o = l.offset.abs();
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {} (UTC{sign}{:02}:{:02})", l.year, l.month, l.day, l.hour, l.minute, l.second, l.abbrev, o / 3600, o % 3600 / 60)
}

/// ISO 8601 / RFC 3339 text to Unix milliseconds. No zone means UTC.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = match s.find(['T', 't', ' ']) {
        Some(p) => (&s[..p], s[p + 1..].trim()),
        None => (s, ""),
    };
    let mut dp = date.split('-');
    let (y, m, d): (i64, u32, u32) = (dp.next()?.parse().ok()?, dp.next()?.parse().ok()?, dp.next()?.parse().ok()?);
    if dp.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let (mut hh, mut mm, mut ss, mut ms, mut off) = (0i64, 0i64, 0i64, 0i64, 0i64);
    if !rest.is_empty() {
        let (clock, zone) = match rest.find(['Z', 'z', '+']).or_else(|| rest.rfind('-')) {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };
        let mut tp = clock.split(':');
        hh = tp.next()?.parse().ok()?;
        mm = tp.next().map_or(Some(0), |x| x.parse().ok())?;
        if let Some(sec) = tp.next() {
            let (w, f) = sec.split_once('.').unwrap_or((sec, ""));
            ss = w.parse().ok()?;
            if !f.is_empty() {
                ms = format!("{:0<3}", &f[..f.len().min(3)]).parse().ok()?;
            }
        }
        if hh > 23 || mm > 59 || ss > 60 {
            return None;
        }
        if let Some(z) = zone.strip_prefix(['+', '-']) {
            let (h, m2) = z.split_once(':').unwrap_or((z.get(..2)?, z.get(2..).unwrap_or("0")));
            let o = h.parse::<i64>().ok()? * 3600 + m2.parse::<i64>().ok()? * 60;
            off = if zone.starts_with('-') { -o } else { o };
        } else if !zone.is_empty() && !zone.eq_ignore_ascii_case("z") {
            return None;
        }
    }
    let secs = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss - off;
    Some(secs * 1000 + ms)
}

/// "5 minutes ago" / "in 2 days".
fn relative(delta_s: i64) -> String {
    let a = delta_s.abs();
    let (n, u) = match a {
        0..=59 => (a, "second"),
        60..=3599 => (a / 60, "minute"),
        3600..=86_399 => (a / 3600, "hour"),
        86_400..=2_591_999 => (a / 86_400, "day"),
        2_592_000..=31_535_999 => (a / 2_592_000, "month"),
        _ => (a / 31_536_000, "year"),
    };
    let s = if n == 1 { "" } else { "s" };
    if delta_s < 0 { format!("{n} {u}{s} ago") } else { format!("in {n} {u}{s}") }
}

/// The page action. `now_ms` is passed in so a test can fix the clock.
pub fn convert(input: &str, now: i64) -> Result<String, String> {
    let t = input.trim();
    let ms = if t.is_empty() || t.eq_ignore_ascii_case("now") {
        now
    } else if let Ok(n) = t.parse::<i64>() {
        // 11 digits or fewer is seconds (up to year 5138); more is milliseconds
        if n.abs() < 100_000_000_000 { n.checked_mul(1000).ok_or("Too large")? } else { n }
    } else if let Some(ms) = parse_iso(t) {
        ms
    } else {
        return Err("Type epoch seconds, epoch milliseconds, an ISO 8601 date like 2024-05-06T07:08:09Z, or now.".into());
    };
    Ok(format!(
        "Epoch seconds       {}\nEpoch milliseconds  {}\nUTC (ISO 8601)      {}\nLocal               {}\nRelative to now     {}",
        ms.div_euclid(1000),
        ms,
        iso_utc(ms),
        local_text(ms),
        relative((ms - now).div_euclid(1000))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_have_the_right_shape_version_and_variant() {
        for v7 in [false, true] {
            let u = if v7 { uuid_v7(1_700_000_000_000) } else { uuid_v4() };
            let p: Vec<&str> = u.split('-').collect();
            assert_eq!(p.iter().map(|x| x.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12], "{u}");
            assert!(u.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
            assert_eq!(p[2].as_bytes()[0], if v7 { b'7' } else { b'4' });
            assert!(matches!(p[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'), "{u}");
        }
        assert_ne!(uuid_v4(), uuid_v4());
        assert!(uuid_v7(1_700_000_000_000).starts_with("018bcfe5-6800-"), "48 bits of milliseconds first");
        assert_eq!(gen(false, 3).unwrap().lines().count(), 3);
    }

    #[test]
    fn iso_and_epoch_round_trip() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(parse_iso("2023-11-14T22:13:20.123Z"), Some(1_700_000_000_123));
        assert_eq!(parse_iso("2023-11-14T23:13:20+01:00"), Some(1_700_000_000_000));
        assert_eq!(parse_iso("2023-11-14 22:13:20"), Some(1_700_000_000_000));
        assert_eq!(parse_iso("2023-11-14"), Some(1_699_920_000_000));
        assert_eq!(parse_iso("2000-02-30x"), None);
        assert_eq!(parse_iso("2023-13-01"), None);
    }

    #[test]
    fn the_page_reads_seconds_milliseconds_iso_and_now() {
        let a = convert("1700000000", 1_700_000_100_000).unwrap();
        assert!(a.contains("2023-11-14T22:13:20.000Z") && a.contains("1 minute ago"), "{a}");
        let b = convert("1700000000000", 0).unwrap();
        assert!(b.contains("Epoch seconds       1700000000"), "{b}");
        let c = convert("2023-11-14T22:13:20Z", 0).unwrap();
        assert!(c.contains("1700000000000"), "{c}");
        assert!(convert("now", 86_400_000).unwrap().contains("1970-01-02T00:00:00.000Z"));
        assert!(convert("banana", 0).is_err());
    }
}
