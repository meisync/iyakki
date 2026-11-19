//! What a person gives `iyakki-connect window`: the one-time link of an instance (`https://host/connect#token`, optionally with the
//! certificate's SHA-256 as `?pin=`), a bare address, or the name of an instance saved on this computer. Also the decision about the
//! instance's certificate, which is a pure function so its refusals are tested without a network.

use iyakki_link::cloudclient::Pin;

use crate::pins;

/// A link or address, read.
#[derive(Debug, PartialEq, Eq)]
pub struct LinkTarget {
    /// `host:port`, lower case, port explicit (the key the pin is stored under).
    pub addr: String,
    /// The one-time token (from the fragment), when the input was a link.
    pub token: Option<String>,
    /// The certificate pin the link carries, when it does.
    pub pin: Option<Pin>,
}

fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A token is what the instance made: hex. Anything with other characters is refused, because it is put into an address.
pub fn token_ok(t: &str) -> bool {
    (8..=200).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~'))
}

/// Read `input` as a link (`https://host[:port]/connect[?pin=..]#token`), `host[:port]/connect#token`, or a bare `host[:port]`.
pub fn parse_link(input: &str) -> Result<LinkTarget, String> {
    let input = input.trim();
    if input.to_ascii_lowercase().starts_with("http://") {
        return Err("a plain http:// address cannot be pinned or protected: use the https:// link".into());
    }
    let rest = match input.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
        Some(_) => return Err(format!("{input:?} is not an https link or an address")),
        None => input,
    };
    let (rest, frag) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f)),
        None => (rest, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((r, q)) => (r, q),
        None => (rest, ""),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("an address with a user name in it is not accepted".into());
    }
    if !matches!(path, "" | "/" | "/connect" | "/connect/") {
        return Err(format!("{path:?} is not the connect page of an instance (the link ends in /connect)"));
    }
    let addr = pins::key(authority)?;
    let token = match frag.filter(|f| !f.is_empty()) {
        None => None,
        Some(f) => {
            let t = pct_decode(f).ok_or("the link's token is not readable")?;
            if !token_ok(&t) {
                return Err("the link's token is not what an instance makes: copy the whole link again".into());
            }
            Some(t)
        }
    };
    let mut pin = None;
    for kv in query.split('&').filter(|x| !x.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if k == "pin" {
            let v = pct_decode(v).ok_or("the link's pin is not readable")?;
            if pin.is_some() {
                return Err("the link names two pins".into());
            }
            pin = Some(Pin::parse(&v).ok_or_else(|| format!("the link's pin {v:?} is not a SHA-256 fingerprint"))?);
        }
    }
    Ok(LinkTarget { addr, token, pin })
}

/// What to do with the certificate an instance showed.
#[derive(Debug, PartialEq, Eq)]
pub enum PinDecision {
    /// It is the pinned one.
    Use,
    /// First use, and the link named exactly this certificate: remember it.
    Remember,
    /// First use with nothing to compare: the person decides.
    Ask,
    /// Never continue; the text says why.
    Stop(String),
}

/// `stored`: the pin on this computer. `linked`: the pin in the link. `shown`: the certificate the instance showed now.
/// A pin that changed is a hard stop, and so is a link that disagrees with the stored pin or with the certificate shown.
pub fn decide_pin(addr: &str, stored: Option<Pin>, linked: Option<Pin>, shown: Pin) -> PinDecision {
    match (stored, linked) {
        (Some(s), Some(l)) if s != l => PinDecision::Stop(format!("STOP: the link names a certificate ({}) that is not the one pinned for {addr} ({}). Nothing was sent. If the instance really has a new certificate, `iyakki-connect unpin {addr}` and use the link again.", l.display(), s.display())),
        (Some(s), _) if s != shown => PinDecision::Stop(crate::changed(addr, s, shown)),
        (Some(_), _) => PinDecision::Use,
        (None, Some(l)) if l != shown => PinDecision::Stop(format!("STOP: {addr} shows a certificate ({}) that is not the one in your link ({}). Either the link was copied wrongly or this is not the instance. Nothing was sent or remembered.", shown.display(), l.display())),
        (None, Some(_)) => PinDecision::Remember,
        (None, None) => PinDecision::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn pin(n: u8) -> Pin {
        Pin([n; 32])
    }

    #[test]
    fn the_token_is_read_from_the_fragment_and_the_pin_from_the_query() {
        let l = parse_link(&format!("https://Mac.local:8443/connect?pin={}#{TOKEN}", pin(7).display().replace(':', "%3A"))).unwrap();
        assert_eq!(l, LinkTarget { addr: "mac.local:8443".into(), token: Some(TOKEN.into()), pin: Some(pin(7)) });
        // hex pin without colons, default port, no scheme
        let l = parse_link(&format!("example.org/connect?pin={}#{TOKEN}", pin(9).hex())).unwrap();
        assert_eq!((l.addr.as_str(), l.pin), ("example.org:443", Some(pin(9))));
        // an address alone has neither
        assert_eq!(parse_link("192.168.1.20:8443").unwrap(), LinkTarget { addr: "192.168.1.20:8443".into(), token: None, pin: None });
        assert_eq!(parse_link("https://box.example/connect").unwrap().token, None);
    }

    #[test]
    fn links_that_could_be_traps_are_refused() {
        for bad in [
            "http://box.local:8443/connect#0123456789abcdef",
            "ftp://box.local/connect",
            "https://user:pw@box.local/connect#0123456789abcdef",
            "https://box.local/other#0123456789abcdef",
            "https://box.local/connect#../../x",
            "https://box.local/connect#a b c d e f g h",
            "https://box.local/connect#short",
            "https://box.local/connect?pin=zz#0123456789abcdef",
            "https://box.local/connect?pin=ab",
            "https://box.local/connect#%zz",
            "",
            "https://",
        ] {
            assert!(parse_link(bad).is_err(), "{bad:?} must be refused");
        }
        let two = format!("https://b.local/connect?pin={}&pin={}#{TOKEN}", pin(1).hex(), pin(2).hex());
        assert!(parse_link(&two).is_err(), "two pins");
    }

    #[test]
    fn a_changed_pin_is_a_hard_stop_and_a_link_must_agree_with_what_is_shown() {
        let a = "box:443";
        assert_eq!(decide_pin(a, Some(pin(1)), None, pin(1)), PinDecision::Use);
        assert_eq!(decide_pin(a, Some(pin(1)), Some(pin(1)), pin(1)), PinDecision::Use);
        assert!(matches!(decide_pin(a, Some(pin(1)), None, pin(2)), PinDecision::Stop(m) if m.contains("changed")));
        // the link vouches for a certificate that is not the stored one: stop, even if the shown one equals the link's
        assert!(matches!(decide_pin(a, Some(pin(1)), Some(pin(2)), pin(2)), PinDecision::Stop(m) if m.contains("not the one pinned")));
        assert_eq!(decide_pin(a, None, Some(pin(3)), pin(3)), PinDecision::Remember);
        assert!(matches!(decide_pin(a, None, Some(pin(3)), pin(4)), PinDecision::Stop(m) if m.contains("not the one in your link")));
        assert_eq!(decide_pin(a, None, None, pin(5)), PinDecision::Ask);
    }
}
