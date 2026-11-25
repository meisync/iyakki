//! Seconds since 1970 of an RFC 3339 UTC time (moved out of `iyakki_agent::k8s`; the agent re-exports it).

/// Seconds since 1970 of `2024-05-01T12:34:56Z` (UTC); 0 when it cannot be read.
pub fn parse_time(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let n = |a: usize, z: usize| s.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19)) else { return 0 };
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + h * 3600 + mi * 60 + se).max(0) as u64
}
