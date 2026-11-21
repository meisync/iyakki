//! The signed list of a Debian archive (F-LINAPP-19): the `InRelease` file (clear-signed) or the pair `Release` and `Release.gpg`,
//! checked with the archive signing key the owner pins in `legal/debian-archive-pgp.asc`, read for the SHA-256 table of the archive's
//! files and for its validity window. The registry route of `linuxapps_dist` keeps the image digest as its pin; this is the second,
//! independent list: a file of the archive (a `Packages` index, a package) is accepted only when its hash is in a list whose signature
//! was good and which has not expired. The key is pinned since 2026-11-19 (Debian 12 and 13).
// SPDX-License-Identifier: Apache-2.0

use sha2::{Digest, Sha256};

/// The pinned Debian archive signing keys of Debian 12 and 13 (`legal/debian-archive-pgp.asc`: provenance and fingerprints are in its header,
/// read from ftp-master.debian.org and the debian-archive-keyring package on 2026-11-19; see docs/project/PIN-DECISIONS.md).
pub fn pinned_key() -> Option<&'static [u8]> {
    Some(include_bytes!("../../../legal/debian-archive-pgp.asc"))
}

/// A checked Release file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRelease {
    pub suite: String,
    pub codename: String,
    pub date: i64,
    pub valid_until: Option<i64>,
    /// `(sha256 hex, size, path relative to the suite folder)`.
    pub files: Vec<(String, u64, String)>,
}

impl SignedRelease {
    pub fn lookup(&self, path: &str) -> Option<&(String, u64, String)> {
        self.files.iter().find(|f| f.2 == path)
    }

    /// Is `bytes` exactly the file `path` of this list (size and SHA-256)?
    pub fn check_file(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let (hash, size, _) = self.lookup(path).ok_or_else(|| format!("{path} is not in the signed list"))?;
        if *size != bytes.len() as u64 {
            return Err(format!("{path} has {} bytes, the signed list says {size}", bytes.len()));
        }
        let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        if &got != hash {
            return Err(format!("{path} does not match the signed list (got {got}, listed {hash}): discarded"));
        }
        Ok(())
    }
}

/// `Sat, 09 Aug 2025 09:49:14 UTC` as seconds since 1970; `None` for any other shape.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = s.split_once(", ").map(|x| x.1).unwrap_or(s);
    let mut p = s.split_whitespace();
    let day: i64 = p.next()?.parse().ok()?;
    let mname = p.next()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| *m == mname)? as i64 + 1;
    let year: i64 = p.next()?.parse().ok()?;
    let mut t = p.next()?.split(':');
    let (h, m, sec): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    if !matches!(p.next(), Some("UTC") | Some("GMT") | Some("+0000")) || !(1..=31).contains(&day) || h > 23 || m > 59 || sec > 60 || year < 1970 {
        return None;
    }
    // days from civil (Howard Hinnant)
    let (y, mo) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + m * 60 + sec)
}

/// Reads the text of a Release file (not yet trusted by itself: call it after the signature check).
fn parse_release(text: &str) -> Result<SignedRelease, String> {
    let (mut suite, mut codename, mut date, mut valid_until) = (String::new(), String::new(), None, None);
    let mut files = Vec::new();
    let mut in_sha256 = false;
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix(' ') {
            if in_sha256 {
                let mut f = rest.split_whitespace();
                let (Some(h), Some(sz), Some(path), None) = (f.next(), f.next(), f.next(), f.next()) else { return Err("a line of the SHA256 table is not `hash size path`".into()) };
                let size: u64 = sz.parse().map_err(|_| "a size in the SHA256 table is not a number".to_string())?;
                if h.len() != 64 || !h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) || path.contains("..") || path.starts_with('/') {
                    return Err("a line of the SHA256 table is damaged".into());
                }
                files.push((h.to_string(), size, path.to_string()));
            }
            continue;
        }
        in_sha256 = false;
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        match k {
            "Suite" => suite = v.to_string(),
            "Codename" => codename = v.to_string(),
            "Date" => date = Some(parse_date(v).ok_or("the Date of the list is not readable")?),
            "Valid-Until" => valid_until = Some(parse_date(v).ok_or("the Valid-Until of the list is not readable")?),
            "SHA256" => in_sha256 = true,
            _ => {}
        }
    }
    if files.is_empty() {
        return Err("the list has no SHA256 table".into());
    }
    Ok(SignedRelease { suite, codename, date: date.ok_or("the list has no Date")?, valid_until, files })
}

fn window(r: SignedRelease, now: i64) -> Result<SignedRelease, String> {
    if r.date > now + 86400 {
        return Err("the signed list is dated in the future: the clock or the list is wrong".into());
    }
    if r.valid_until.is_some_and(|v| v < now) {
        return Err("the signed list has expired (a replayed old list is refused)".into());
    }
    Ok(r)
}

/// `Release` with its detached signature `Release.gpg`.
pub fn verify_release(key: &[u8], release: &[u8], sig: &[u8], now: i64) -> Result<SignedRelease, String> {
    iyakki_openpgp::verify_detached(key, sig, release).map_err(|e| format!("the signature of the archive's Release file is not good ({e})"))?;
    window(parse_release(&String::from_utf8_lossy(release))?, now)
}

/// The clear-signed `InRelease` file: the text and the signature block are taken apart, the text is put in the canonical form the
/// signature covers (dash escapes undone, trailing blanks cut, CR LF line ends, no line end after the last line) and checked.
pub fn verify_inrelease(key: &[u8], inrelease: &[u8], now: i64) -> Result<SignedRelease, String> {
    let text = std::str::from_utf8(inrelease).map_err(|_| "InRelease is not text")?;
    let body = text.strip_prefix("-----BEGIN PGP SIGNED MESSAGE-----").ok_or("InRelease is not clear-signed")?;
    let body = body.trim_start_matches(['\r', '\n']);
    // armor headers (Hash: ...) end at the first empty line
    let (_hdr, body) = body.split_once("\n\n").ok_or("InRelease has no text")?;
    let sig_at = body.find("\n-----BEGIN PGP SIGNATURE-----").ok_or("InRelease has no signature")?;
    let (msg, sig) = (&body[..sig_at], &body[sig_at + 1..]);
    if sig.matches("-----BEGIN PGP SIGNATURE-----").count() != 1 || msg.contains("\n-----BEGIN PGP") {
        return Err("InRelease has more than one signature block".into());
    }
    let plain: Vec<String> = msg.lines().map(|l| l.strip_prefix("- ").unwrap_or(l).trim_end_matches([' ', '\t', '\r']).to_string()).collect();
    let canonical = plain.join("\r\n");
    iyakki_openpgp::verify_detached(key, sig.as_bytes(), canonical.as_bytes()).map_err(|e| format!("the signature of the archive's InRelease file is not good ({e})"))?;
    window(parse_release(&plain.join("\n"))?, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = include_bytes!("../tests/fixtures/debian-release/test-key.asc");
    const OTHER: &[u8] = include_bytes!("../tests/fixtures/debian-release/other-key.asc");
    const INRELEASE: &[u8] = include_bytes!("../tests/fixtures/debian-release/InRelease");
    const RELEASE: &[u8] = include_bytes!("../tests/fixtures/debian-release/Release");
    const RELEASE_GPG: &[u8] = include_bytes!("../tests/fixtures/debian-release/Release.gpg");
    // 2025-08-10 12:00:00 UTC, inside the fixture's window
    const NOW: i64 = 1_754_827_200;

    #[test]
    fn dates_are_read_as_utc_seconds_and_other_shapes_are_refused() {
        assert_eq!(parse_date("Thu, 01 Jan 1970 00:00:00 UTC"), Some(0));
        assert_eq!(parse_date("Sat, 09 Aug 2025 09:49:14 UTC"), Some(1_754_732_954));
        assert_eq!(parse_date("Sat, 09 Aug 2025 09:49:14 +0200"), None);
        assert_eq!(parse_date("yesterday"), None);
        assert_eq!(parse_date("Sat, 32 Aug 2025 09:49:14 UTC"), None);
    }

    #[test]
    fn a_good_inrelease_and_a_good_release_pair_give_the_same_table() {
        let a = verify_inrelease(KEY, INRELEASE, NOW).unwrap();
        let b = verify_release(KEY, RELEASE, RELEASE_GPG, NOW).unwrap();
        assert_eq!(a, b);
        assert_eq!((a.suite.as_str(), a.codename.as_str()), ("stable", "trixie"));
        assert_eq!(a.files.len(), 2, "only the SHA256 table counts");
        assert_eq!(a.lookup("main/binary-amd64/Packages.xz").unwrap().1, 1234);
    }

    #[test]
    fn another_key_a_changed_text_or_a_changed_signature_is_refused() {
        assert!(verify_inrelease(OTHER, INRELEASE, NOW).is_err());
        assert!(verify_release(OTHER, RELEASE, RELEASE_GPG, NOW).is_err());
        let changed = String::from_utf8_lossy(INRELEASE).replace("1234", "1235");
        assert!(verify_inrelease(KEY, changed.as_bytes(), NOW).is_err());
        let mut rel = RELEASE.to_vec();
        rel[10] ^= 1;
        assert!(verify_release(KEY, &rel, RELEASE_GPG, NOW).is_err());
        assert!(verify_inrelease(KEY, b"just text", NOW).is_err());
        assert!(verify_inrelease(KEY, b"", NOW).is_err());
        // a text with a second signature block smuggled in
        let two = String::from_utf8_lossy(INRELEASE).replace("Label: Debian", "-----BEGIN PGP SIGNATURE-----\nLabel: Debian");
        assert!(verify_inrelease(KEY, two.as_bytes(), NOW).is_err());
    }

    #[test]
    fn an_expired_or_future_list_is_refused() {
        let valid_until = parse_date("Sat, 16 Aug 2025 09:49:14 UTC").unwrap();
        assert!(verify_inrelease(KEY, INRELEASE, valid_until + 1).unwrap_err().contains("expired"));
        assert!(verify_inrelease(KEY, INRELEASE, parse_date("Sat, 09 Aug 2025 09:49:14 UTC").unwrap() - 3 * 86400).unwrap_err().contains("future"));
    }

    #[test]
    fn a_file_is_accepted_only_with_the_listed_size_and_hash() {
        let r = verify_inrelease(KEY, INRELEASE, NOW).unwrap();
        assert!(r.check_file("main/binary-amd64/Packages.xz", b"x").unwrap_err().contains("bytes"));
        assert!(r.check_file("nothing", b"x").unwrap_err().contains("not in the signed list"));
        // build a list whose entry matches a small file
        let data = b"hello packages";
        let h: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
        let list = SignedRelease { files: vec![(h, data.len() as u64, "a/Packages".into())], ..r };
        assert!(list.check_file("a/Packages", data).is_ok());
        assert!(list.check_file("a/Packages", b"hello packagez").unwrap_err().contains("does not match"));
    }

    #[test]
    fn damaged_tables_are_refused() {
        for bad in ["Date: Sat, 09 Aug 2025 09:49:14 UTC\nSHA256:\n zz 1 a\n", "Date: Sat, 09 Aug 2025 09:49:14 UTC\nSHA256:\n 1 2\n", "Date: Sat, 09 Aug 2025 09:49:14 UTC\n", "SHA256:\n 1111111111111111111111111111111111111111111111111111111111111111 1 ../x\n"] {
            assert!(parse_release(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_pinned_file_holds_the_two_debian_archive_keys() {
        let keys = iyakki_openpgp::rsa_keys(pinned_key().unwrap()).unwrap();
        // two RSA-4096 primary keys and their two signing subkeys
        assert_eq!(keys.len(), 4, "bookworm and trixie, primary and signing subkey each");
        assert!(keys.iter().all(|(n, _)| n.len() == 512));
    }

    /// The real clear-signed list of the Debian stable suite as served on 2026-09-12 (Debian 13.7), checked with the pinned key.
    #[test]
    fn the_real_trixie_list_verifies_with_the_pinned_key_and_a_changed_byte_does_not() {
        let real: &[u8] = include_bytes!("../tests/fixtures/debian-release/trixie-InRelease-2026-09-12");
        let now = parse_date("Sat, 12 Sep 2026 09:00:00 UTC").unwrap();
        let r = verify_inrelease(pinned_key().unwrap(), real, now).unwrap();
        assert_eq!((r.suite.as_str(), r.codename.as_str()), ("stable", "trixie"));
        let mut bad = real.to_vec();
        let at = bad.windows(9).position(|w| w == b"Debian 13").unwrap_or(200);
        bad[at + 7] ^= 1;
        assert!(verify_inrelease(pinned_key().unwrap(), &bad, now).is_err());
    }
}
