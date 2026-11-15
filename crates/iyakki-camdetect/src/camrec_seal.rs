//! A ledger that finds recordings cut at the end or deleted (F-CAM-03).
//!
//! The segment file format finds changed, moved and swapped pictures, but a picture taken off the END of a file looks the same as a
//! power cut, and a deleted file leaves nothing behind. So when the recorder closes a file it also writes one line to a ledger beside
//! the index: the file, how many pictures it ended with, the time of the last one, and a MAC over those facts made with a key
//! derived from the camera's recording key. When retention (age, size) removes a file it writes a `gone` line the same way. Later:
//!
//! * a file with fewer pictures than its sealed line says, or one that is gone without a `gone` line, is reported;
//! * a ledger line with a wrong MAC (edited, or made by someone without the key) is reported, and its claims are not trusted;
//! * a file with no line at all is simply an unsealed file (the one being recorded now, or one cut by a power cut before it closed).
//!
//! Limits: someone who can delete the whole ledger is not caught (the ledger has no copy elsewhere), and a person who has the key
//! can forge lines. The ledger is for finding carelessness and simple tampering by someone who has the disk but not the vault.

use iyakki_volcrypt::mac::{ct_eq, hmac_sha256};

use crate::camrec_format::RecKey;

/// A line of the ledger, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// The file was closed with this many pictures, the last at `end_ms`.
    Sealed { rel: String, frames: u32, end_ms: u64 },
    /// Retention removed the file on purpose.
    Gone { rel: String },
}

/// What is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// The file has fewer pictures than it was closed with.
    Cut { rel: String, sealed: u32, found: u32 },
    /// The file is not there, and retention did not remove it.
    Deleted { rel: String },
    /// A ledger line whose MAC is wrong or which cannot be read (the line number counts from 1).
    BadLine { number: usize },
}

impl Finding {
    /// One sentence for a person.
    pub fn words(&self) -> String {
        match self {
            Finding::Cut { rel, sealed, found } => format!("the recording {rel} was closed with {sealed} pictures and now has {found}: the end was cut off"),
            Finding::Deleted { rel } => format!("the recording {rel} is missing and was not removed by the keeping rules"),
            Finding::BadLine { number } => format!("line {number} of the recordings ledger is damaged or was not written by this computer"),
        }
    }
}

fn mac_of(key: &RecKey, what: &str) -> String {
    // a key of its own for the ledger, so the MAC of a line can never be a MAC of a picture
    let k = hmac_sha256(key.as_bytes(), &[b"iyakki-camrec-ledger-v1"]);
    hmac_sha256(&k, &[what.as_bytes()]).iter().map(|b| format!("{b:02x}")).collect()
}

fn ok_rel(rel: &str) -> bool {
    !rel.is_empty() && rel.len() <= 128 && !rel.contains("..") && !rel.starts_with('/') && rel.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
}

/// The line to append when a file is closed.
pub fn sealed_line(key: &RecKey, rel: &str, frames: u32, end_ms: u64) -> String {
    let what = format!("sealed {rel} {frames} {end_ms}");
    format!("{what} {}\n", mac_of(key, &what))
}

/// The line to append when retention removes a file.
pub fn gone_line(key: &RecKey, rel: &str) -> String {
    let what = format!("gone {rel}");
    format!("{what} {}\n", mac_of(key, &what))
}

/// Read one line; `None` when it is not valid or its MAC is wrong.
pub fn parse_line(key: &RecKey, line: &str) -> Option<Line> {
    let (what, mac) = line.trim_end().rsplit_once(' ')?;
    if mac.len() != 64 || !ct_eq(mac.as_bytes(), mac_of(key, what).as_bytes()) {
        return None;
    }
    let mut p = what.split(' ');
    match p.next()? {
        "sealed" => {
            let (rel, frames, end_ms) = (p.next()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
            (ok_rel(rel) && p.next().is_none()).then(|| Line::Sealed { rel: rel.to_string(), frames, end_ms })
        }
        "gone" => {
            let rel = p.next()?;
            (ok_rel(rel) && p.next().is_none()).then(|| Line::Gone { rel: rel.to_string() })
        }
        _ => None,
    }
}

/// Check the ledger against the files. `frames_of(rel)` gives the number of pictures a file holds now, `None` when it is not there.
/// The last line about a file counts (a file is sealed once; a name used again after `gone` starts over).
pub fn verify(key: &RecKey, ledger: &str, frames_of: &dyn Fn(&str) -> Option<u32>) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut state: Vec<(String, Option<u32>)> = Vec::new(); // rel, sealed frames (None: gone)
    for (i, line) in ledger.lines().enumerate().take(200_000) {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(key, line) {
            None => out.push(Finding::BadLine { number: i + 1 }),
            Some(Line::Sealed { rel, frames, .. }) => match state.iter_mut().find(|s| s.0 == rel) {
                Some(s) => s.1 = Some(frames),
                None => state.push((rel, Some(frames))),
            },
            Some(Line::Gone { rel }) => match state.iter_mut().find(|s| s.0 == rel) {
                Some(s) => s.1 = None,
                None => state.push((rel, None)),
            },
        }
    }
    for (rel, sealed) in state {
        let Some(sealed) = sealed else { continue };
        match frames_of(&rel) {
            None => out.push(Finding::Deleted { rel }),
            Some(found) if found < sealed => out.push(Finding::Cut { rel, sealed, found }),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> RecKey {
        RecKey::from_bytes([n; 32])
    }

    #[test]
    fn lines_round_trip_and_a_changed_or_foreign_line_is_not_believed() {
        let k = key(1);
        let l = sealed_line(&k, "2026/11/13/101500.irec", 120, 1_700_000_000_000);
        assert_eq!(parse_line(&k, &l), Some(Line::Sealed { rel: "2026/11/13/101500.irec".into(), frames: 120, end_ms: 1_700_000_000_000 }));
        assert_eq!(parse_line(&k, &gone_line(&k, "a/b.irec")), Some(Line::Gone { rel: "a/b.irec".into() }));
        assert_eq!(parse_line(&key(2), &l), None, "another key");
        assert_eq!(parse_line(&k, &l.replace(" 120 ", " 99 ")), None, "a changed count");
        assert_eq!(parse_line(&k, &l.replace("2026", "2025")), None, "a changed name");
        assert_eq!(parse_line(&k, "sealed ../../etc/passwd 1 1 00"), None);
        let evil = {
            let what = "sealed /etc/x 1 1";
            format!("{what} {}", mac_of(&k, what))
        };
        assert_eq!(parse_line(&k, &evil), None, "even a correct MAC does not make a path outside the folder acceptable");
        assert_eq!(parse_line(&k, ""), None);
        assert_eq!(parse_line(&k, "sealed"), None);
    }

    #[test]
    fn a_cut_a_deletion_and_a_forged_line_are_found_and_honest_removals_are_not() {
        let k = key(7);
        let mut ledger = String::new();
        for (rel, n) in [("d/1.irec", 10u32), ("d/2.irec", 20), ("d/3.irec", 30), ("d/4.irec", 40)] {
            ledger.push_str(&sealed_line(&k, rel, n, 5));
        }
        ledger.push_str(&gone_line(&k, "d/4.irec"));
        ledger.push_str("sealed d/9.irec 1000 5 0000000000000000000000000000000000000000000000000000000000000000\n");
        let files = |rel: &str| match rel {
            "d/1.irec" => Some(10),
            "d/2.irec" => Some(12),
            _ => None,
        };
        let f = verify(&k, &ledger, &files);
        assert_eq!(f, vec![Finding::BadLine { number: 6 }, Finding::Cut { rel: "d/2.irec".into(), sealed: 20, found: 12 }, Finding::Deleted { rel: "d/3.irec".into() }]);
        for x in &f {
            assert!(!x.words().is_empty());
        }
        // d/4 was removed on purpose; d/1 is whole; an unsealed file is nobody's business
        assert!(verify(&k, "", &files).is_empty());
        // a file longer than sealed (appended after the seal) is not a cut
        assert!(verify(&k, &sealed_line(&k, "d/1.irec", 5, 1), &files).is_empty());
        // a name used again after it was removed starts over
        let again = format!("{}{}", gone_line(&k, "d/1.irec"), sealed_line(&k, "d/1.irec", 8, 9));
        assert!(verify(&k, &again, &files).is_empty());
        let again = format!("{}{}", gone_line(&k, "d/1.irec"), sealed_line(&k, "d/1.irec", 18, 9));
        assert_eq!(verify(&k, &again, &files), vec![Finding::Cut { rel: "d/1.irec".into(), sealed: 18, found: 10 }]);
    }

    #[test]
    fn hostile_ledgers_never_panic() {
        let k = key(3);
        let mut seed = 11u64;
        for _ in 0..300 {
            let mut s = String::new();
            for _ in 0..(seed % 120) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let a = b"sealed gone 0123456789abcdef/._ \n-";
                s.push(a[((seed >> 33) as usize) % a.len()] as char);
            }
            let _ = verify(&k, &s, &|_| Some(1));
        }
    }
}
