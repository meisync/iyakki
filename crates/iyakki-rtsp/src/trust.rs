//! The trust choice for encrypted camera streams (`rtsps://`, `https://`; F-CAM-20). Cameras usually carry a self-signed certificate, so
//! the usual public-authority check fails for them. The person's choices are: **System** (the public authorities decide), or **Pin** (this
//! exact certificate, by its SHA-256 fingerprint, and no other). A certificate that fails the system check is never accepted on its own:
//! the verdict is `Ask`, which carries the fingerprint for a card ("Trust this camera's certificate?"); the answer becomes a `Pin`.
//! A pinned camera that shows a different certificate is refused in words, never asked again silently.
//! The handshake itself is done by the caller (the agent's TLS transport); this crate has no dependencies and only judges.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    System,
    Pin(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// The certificate is not trusted by the system: show this fingerprint and ask.
    Ask { fingerprint: String },
    Refuse(String),
}

/// The fingerprint in a form that compares: hex digits only, lower case (colons, spaces and a `SHA256:` or `sha-256` prefix removed).
pub fn normalize(fp: &str) -> String {
    let t = fp.trim();
    let t = t.strip_prefix("SHA256:").or_else(|| t.strip_prefix("sha256:")).or_else(|| t.strip_prefix("SHA-256:")).unwrap_or(t);
    t.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_lowercase()).collect()
}

/// `AB:CD:...` for display from 32 bytes.
pub fn display(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Judge a presented certificate. `fingerprint` is the SHA-256 of the leaf as hex (any common spelling); `system_ok` says the chain
/// and the host name passed the public-authority check.
pub fn judge(trust: &Trust, fingerprint: &str, system_ok: bool) -> Verdict {
    let fp = normalize(fingerprint);
    if fp.len() != 64 {
        return Verdict::Refuse("the camera's certificate could not be read".into());
    }
    match trust {
        Trust::System if system_ok => Verdict::Accept,
        Trust::System => Verdict::Ask { fingerprint: fp },
        Trust::Pin(p) if normalize(p) == fp => Verdict::Accept,
        Trust::Pin(_) => Verdict::Refuse("this camera now shows a different certificate than the one you trusted; it may have been replaced or it may not be your camera. Remove the saved trust to look at the new one.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";

    #[test]
    fn a_pin_accepts_only_its_own_certificate_in_any_spelling() {
        let p = Trust::Pin(FP.into());
        for spelling in [FP.to_string(), FP.replace(':', "").to_lowercase(), format!("SHA256:{}", FP.replace(':', ""))] {
            assert_eq!(judge(&p, &spelling, false), Verdict::Accept, "{spelling}");
        }
        let other = FP.replace("AB", "AC");
        assert!(matches!(judge(&p, &other, true), Verdict::Refuse(m) if m.contains("different certificate")), "a pin is never replaced silently, even by a valid chain");
    }

    #[test]
    fn the_system_check_accepts_a_proper_chain_and_asks_about_the_rest_with_the_fingerprint() {
        assert_eq!(judge(&Trust::System, FP, true), Verdict::Accept);
        assert_eq!(judge(&Trust::System, FP, false), Verdict::Ask { fingerprint: normalize(FP) });
        assert!(matches!(judge(&Trust::System, "12:34", true), Verdict::Refuse(_)));
        assert!(matches!(judge(&Trust::Pin(FP.into()), "", false), Verdict::Refuse(_)));
        assert_eq!(display(&[0xAB, 0x01]), "AB:01");
    }
}
