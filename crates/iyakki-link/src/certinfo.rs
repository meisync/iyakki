//! A small reader of the few certificate fields the connector needs (validity and DNS names), with no X509 library.

/// What a certificate says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    pub not_before: i64,
    pub not_after: i64,
    /// The `dNSName` entries of the Subject Alternative Name.
    pub names: Vec<String>,
}

struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    /// The next element: (tag, content).
    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let b = self.0;
        let tag = *b.first()?;
        let l0 = *b.get(1)?;
        let (len, head) = if l0 < 0x80 {
            (l0 as usize, 2)
        } else {
            let n = (l0 & 0x7f) as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let mut v = 0usize;
            for i in 0..n {
                v = (v << 8) | *b.get(2 + i)? as usize;
            }
            (v, 2 + n)
        };
        let body = b.get(head..head.checked_add(len)?)?;
        self.0 = &b[head + len..];
        Some((tag, body))
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn der_time(tag: u8, s: &[u8]) -> Option<i64> {
    let t = std::str::from_utf8(s).ok()?;
    let t = t.strip_suffix('Z')?;
    let (year, rest) = match tag {
        0x17 => {
            let yy: i64 = t.get(..2)?.parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, t.get(2..)?)
        }
        0x18 => (t.get(..4)?.parse().ok()?, t.get(4..)?),
        _ => return None,
    };
    let n = |a: usize| rest.get(a..a + 2)?.parse::<i64>().ok();
    Some(days_from_civil(year, n(0)?, n(2)?) * 86400 + n(4)? * 3600 + n(6)? * 60 + n(8).unwrap_or(0))
}

/// Read the validity and the names out of one DER certificate. `None` when it is not a certificate this reader understands.
pub fn read_cert(der: &[u8]) -> Option<CertInfo> {
    let (_, cert) = Der(der).next()?;
    let (_, tbs) = Der(cert).next()?;
    let mut f = Der(tbs);
    let (mut tag, mut body) = f.next()?;
    if tag == 0xa0 {
        (tag, body) = f.next()?; // serial
    }
    let _ = (tag, body);
    f.next()?; // signature algorithm
    f.next()?; // issuer
    let (_, validity) = f.next()?;
    let mut v = Der(validity);
    let (t1, b1) = v.next()?;
    let (t2, b2) = v.next()?;
    let (not_before, not_after) = (der_time(t1, b1)?, der_time(t2, b2)?);
    f.next()?; // subject
    f.next()?; // public key
    let mut names = Vec::new();
    for (tag, body) in std::iter::from_fn(|| f.next()) {
        if tag != 0xa3 {
            continue;
        }
        let (_, exts) = Der(body).next()?;
        let mut e = Der(exts);
        while let Some((_, ext)) = e.next() {
            let mut x = Der(ext);
            let Some((0x06, oid)) = x.next() else { continue };
            if oid != [0x55, 0x1d, 0x11] {
                continue;
            }
            let mut rest = x.next()?;
            if rest.0 == 0x01 {
                rest = x.next()?; // critical flag
            }
            let (_, names_seq) = Der(rest.1).next()?;
            let mut n = Der(names_seq);
            while let Some((t, b)) = n.next() {
                if t == 0x82 {
                    names.push(String::from_utf8_lossy(b).to_ascii_lowercase());
                }
            }
        }
    }
    Some(CertInfo { not_before, not_after, names })
}

/// A certificate name covers a host: equal, or `*.x` for one label under `x`.
pub fn name_covers(name: &str, host: &str) -> bool {
    let (name, host) = (name.to_ascii_lowercase(), host.to_ascii_lowercase());
    match name.strip_prefix("*.") {
        Some(d) => host.strip_suffix(d).is_some_and(|l| l.len() > 1 && l.ends_with('.') && !l[..l.len() - 1].contains('.')),
        None => name == host,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calendar_is_right() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(der_time(0x17, b"700101000000Z"), Some(0));
        assert_eq!(der_time(0x18, b"20270101000000Z"), Some(1_798_761_600));
    }

    #[test]
    fn names_follow_the_wildcard_rule_and_garbage_is_refused() {
        assert!(name_covers("*.example.org", "a.example.org") && !name_covers("*.example.org", "example.org"));
        assert!(read_cert(b"").is_none() && read_cert(&[0x30, 0x03, 1, 2]).is_none());
    }
}
