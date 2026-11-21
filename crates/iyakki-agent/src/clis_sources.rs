//! Download addresses that the person (or the image maker) gives for a program whose vendor publishes no build for IYAKKI's images
//! (`Source::Setting`, F-VPN-06: OpenVPN). One small file, `sources.json`, in the CLI folder: key -> `<https address> <sha256>`.
//! Nothing is built in and nothing is invented. A missing, empty or damaged entry means no download (the click explains why and
//! fetches nothing); a download whose SHA-256 differs from the entry is thrown away.

use std::collections::BTreeMap;
use std::path::Path;

pub const FILE: &str = "sources.json";
const MAX_URL: usize = 512;

/// An address and the SHA-256 (lower-case hex) the file must have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub sha256: String,
}

impl Entry {
    /// `https://host/path <64 hex digits>`. Plain http, a user name in the address, control characters, a missing or short checksum
    /// are all refused with the reason.
    pub fn parse(text: &str) -> Result<Entry, String> {
        let mut it = text.split_whitespace();
        let (Some(url), Some(sum), None) = (it.next(), it.next(), it.next()) else {
            return Err("give the address and its SHA-256 separated by a space: `https://... <64 hex digits>`".into());
        };
        let rest = url.strip_prefix("https://").ok_or("the address must start with https:// (a download over plain http is never accepted)")?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if url.len() > MAX_URL || authority.is_empty() || authority.contains('@') || url.chars().any(|c| c.is_control() || c == '\\') {
            return Err("the address is not usable (empty host, a user name in it, or too long)".into());
        }
        let host = authority.rsplit_once(':').map_or(authority, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { authority });
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)) {
            return Err("the host name of the address is not usable".into());
        }
        let sum = sum.to_ascii_lowercase();
        let sum = sum.strip_prefix("sha256:").unwrap_or(&sum).to_string();
        if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("the checksum must be a SHA-256: 64 hex digits".into());
        }
        Ok(Entry { url: url.to_string(), sha256: sum })
    }

    pub fn host(&self) -> &str {
        let rest = self.url.strip_prefix("https://").unwrap_or(&self.url);
        let a = rest.split(['/', '?', '#']).next().unwrap_or("");
        a.rsplit_once(':').map_or(a, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { a })
    }

    /// The file name the address ends in, which says how it is unpacked.
    pub fn file_name(&self) -> String {
        let path = self.url.split(['?', '#']).next().unwrap_or("");
        path.rsplit('/').next().unwrap_or("download").to_string()
    }
}

fn load(root: &Path) -> BTreeMap<String, String> {
    std::fs::read(root.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The raw value of `key`, if any.
pub fn raw(root: &Path, key: &str) -> Option<String> {
    load(root).remove(key).filter(|v| !v.trim().is_empty())
}

/// The checked entry of `key`: `Err` says why the download cannot start (nothing set, or a damaged value).
pub fn get(root: &Path, key: &str) -> Result<Entry, String> {
    match raw(root, key) {
        None => Err(format!("`{key}` is empty")),
        Some(v) => Entry::parse(&v).map_err(|e| format!("`{key}` cannot be used: {e}")),
    }
}

/// Sets (or, with an empty value, clears) `key`. The value is checked first; a value that does not parse is never written.
pub fn set(root: &Path, key: &str, value: &str) -> Result<(), String> {
    if !crate::clis::setting_keys().contains(&key) {
        return Err(format!("`{key}` is not the address of any program in the list"));
    }
    let mut m = load(root);
    if value.trim().is_empty() {
        m.remove(key);
    } else {
        Entry::parse(value)?;
        m.insert(key.to_string(), value.trim().to_string());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let tmp = root.join(format!("{FILE}.new"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, root.join(FILE)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SUM: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn only_an_https_address_with_a_full_sha256_is_an_entry() {
        let e = Entry::parse(&format!("https://mirror.example:8443/o/openvpn-2.6.tar.gz {}", SUM.to_uppercase())).unwrap();
        assert_eq!((e.host(), e.file_name().as_str(), e.sha256.as_str()), ("mirror.example", "openvpn-2.6.tar.gz", SUM));
        assert_eq!(Entry::parse(&format!("https://a.example/x sha256:{SUM}")).unwrap().sha256, SUM);
        for bad in ["", "https://a.example/x", &format!("http://a.example/x {SUM}"), &format!("https://u:p@a.example/x {SUM}"), &format!("https:///x {SUM}"), "https://a.example/x abc", &format!("https://a.example/x {SUM} extra"), &format!("https://a b.example/x {SUM}"), &format!("https://a.example/\u{7}x {SUM}"), &format!("ftp://a.example/x {SUM}"), &format!("https://a.example/x {}", "g".repeat(64))] {
            assert!(Entry::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Entry::parse(&format!("https://a.example/{} {SUM}", "x".repeat(600))).is_err());
    }

    #[test]
    fn a_value_is_checked_before_it_is_written_and_an_empty_one_clears() {
        let d = std::env::temp_dir().join(format!("iy-src-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(get(&d, "vpn.openvpn_download").unwrap_err().contains("is empty"));
        assert!(set(&d, "vpn.openvpn_download", "http://x.example/a abc").is_err());
        assert!(!d.join(FILE).exists(), "a refused value writes nothing");
        assert!(set(&d, "no.such_key", &format!("https://a.example/o {SUM}")).is_err());
        set(&d, "vpn.openvpn_download", &format!("https://a.example/o {SUM}")).unwrap();
        assert_eq!(get(&d, "vpn.openvpn_download").unwrap().url, "https://a.example/o");
        // a file edited by hand into something unusable is a refusal, not a download
        std::fs::write(d.join(FILE), r#"{"vpn.openvpn_download": "https://a.example/o"}"#).unwrap();
        assert!(get(&d, "vpn.openvpn_download").unwrap_err().contains("cannot be used"));
        set(&d, "vpn.openvpn_download", "").unwrap();
        assert!(raw(&d, "vpn.openvpn_download").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
