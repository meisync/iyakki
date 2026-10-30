//! What a person needs to see about a server certificate before trusting it: fingerprint, subject, issuer, validity, and
//! where the issuer's certificate can be fetched. A small reader for the parts of X.509 (DER) it takes; it checks no signatures.

use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    /// SHA-256 of the whole certificate, upper-case hex pairs separated by colons.
    pub sha256: String,
    pub subject: String,
    pub issuer: String,
    /// Seconds since 1970 (UTC).
    pub not_before: i64,
    pub not_after: i64,
    /// `http://` addresses from the "CA Issuers" entry of Authority Information Access.
    pub ca_issuers: Vec<String>,
    pub self_issued: bool,
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Upper-case hex without separators, for comparing fingerprints typed or stored in different styles.
pub fn norm_fingerprint(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_uppercase()).collect()
}

/// `AB:CD:EF:01 …` in groups of four bytes per line chunk, for reading aloud or comparing by eye.
pub fn fingerprint_groups(fp: &str) -> String {
    let n = norm_fingerprint(fp);
    n.as_bytes().chunks(8).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join(" ")
}

/// First 8 hex digits, for the audit trail.
pub fn short_fingerprint(fp: &str) -> String {
    norm_fingerprint(fp).chars().take(8).collect()
}

/// One DER element: (tag, content, what follows).
fn tlv(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *d.first()?;
    let l0 = *d.get(1)? as usize;
    let (len, at) = if l0 < 0x80 {
        (l0, 2)
    } else {
        let n = l0 & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = len << 8 | *d.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let end = at.checked_add(len)?;
    Some((tag, d.get(at..end)?, d.get(end..)?))
}

fn items(mut d: &[u8]) -> Vec<(u8, &[u8])> {
    let mut v = Vec::new();
    while let Some((t, c, rest)) = tlv(d) {
        v.push((t, c));
        d = rest;
    }
    v
}

const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_O: &[u8] = &[0x55, 0x04, 0x0a];
const OID_AIA: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];
const OID_CA_ISSUERS: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x02];

/// "CN, O" of a Name, or a placeholder.
fn name_text(name: &[u8]) -> String {
    let (mut cn, mut org) = (None, None);
    for (_, rdn) in items(name) {
        for (_, atv) in items(rdn) {
            let parts = items(atv);
            if let [(6, oid), (_, val)] = parts.as_slice() {
                let s: String = String::from_utf8_lossy(val).chars().filter(|c| !c.is_control()).take(100).collect();
                if *oid == OID_CN {
                    cn = Some(s);
                } else if *oid == OID_O {
                    org = Some(s);
                }
            }
        }
    }
    match (cn, org) {
        (Some(c), Some(o)) if c != o => format!("{c} ({o})"),
        (Some(c), _) => c,
        (None, Some(o)) => o,
        _ => "(no name)".into(),
    }
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// UTCTime (tag 23, `YYMMDDHHMMSSZ`) or GeneralizedTime (tag 24, `YYYYMMDDHHMMSSZ`).
fn time_value(tag: u8, v: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(v).ok()?;
    let s = s.strip_suffix('Z')?;
    let (year, rest) = match tag {
        23 if s.len() == 12 => {
            let yy: i64 = s[..2].parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &s[2..])
        }
        24 if s.len() == 14 => (s[..4].parse().ok()?, &s[4..]),
        _ => return None,
    };
    let p = |a: usize| rest.get(a..a + 2)?.parse::<i64>().ok();
    Some(days_from_civil(year, p(0)?, p(2)?) * 86400 + p(4)? * 3600 + p(6)? * 60 + p(8)?)
}

pub fn parse(der: &[u8]) -> Result<CertInfo, String> {
    let bad = || "that is not a readable certificate".to_string();
    let (t, cert, _) = tlv(der).ok_or_else(bad)?;
    if t != 0x30 {
        return Err(bad());
    }
    let (t, tbs, _) = tlv(cert).ok_or_else(bad)?;
    if t != 0x30 {
        return Err(bad());
    }
    let mut f = items(tbs);
    if f.first().map(|x| x.0) == Some(0xa0) {
        f.remove(0); // version
    }
    // serial, signature algorithm, issuer, validity, subject, public key
    if f.len() < 6 {
        return Err(bad());
    }
    let (raw_issuer, raw_subject) = (f[2].1, f[4].1);
    let validity = items(f[3].1);
    let (nb, na) = match validity.as_slice() {
        [(t1, v1), (t2, v2)] => (time_value(*t1, v1).ok_or_else(bad)?, time_value(*t2, v2).ok_or_else(bad)?),
        _ => return Err(bad()),
    };
    let mut ca_issuers = Vec::new();
    for (tag, ext) in f.iter().skip(6) {
        if *tag != 0xa3 {
            continue;
        }
        let list = tlv(ext).map(|x| x.1).unwrap_or(&[]);
        for (_, e) in items(list) {
            let parts = items(e);
            let value = parts.last().map(|x| x.1).unwrap_or(&[]);
            if parts.first().map(|x| x.1) != Some(OID_AIA) {
                continue;
            }
            // OCTET STRING wrapping SEQUENCE OF AccessDescription
            let seq = tlv(value).map(|x| x.1).unwrap_or(&[]);
            for (_, ad) in items(seq) {
                if let [(6, method), (0x86, uri)] = items(ad).as_slice() {
                    if *method == OID_CA_ISSUERS {
                        if let Ok(u) = std::str::from_utf8(uri) {
                            if u.starts_with("http://") && u.len() < 500 && u.chars().all(|c| c.is_ascii_graphic()) {
                                ca_issuers.push(u.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(CertInfo { sha256: fingerprint(der), subject: name_text(raw_subject), issuer: name_text(raw_issuer), not_before: nb, not_after: na, ca_issuers, self_issued: raw_issuer == raw_subject })
}

/// `YYYY-MM-DD` for a Unix time.
pub fn date_text(t: i64) -> String {
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip_and_fingerprints_are_normalised() {
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(date_text(days_from_civil(2026, 10, 25) * 86400 + 5), "2026-10-25");
        assert_eq!(time_value(23, b"260102030405Z"), Some(days_from_civil(2026, 1, 2) * 86400 + 3 * 3600 + 4 * 60 + 5));
        assert_eq!(time_value(24, b"20501231235959Z"), Some(days_from_civil(2050, 12, 31) * 86400 + 86399));
        assert_eq!(norm_fingerprint("ab:cd ef"), "ABCDEF");
        assert_eq!(fingerprint_groups("AB:CD:EF:01:23:45:67:89:AA"), "ABCDEF01 23456789 AA");
        assert_eq!(short_fingerprint("ab:cd:ef:01:23:45:67:89"), "ABCDEF01");
        assert!(parse(b"nonsense").is_err());
    }
}
