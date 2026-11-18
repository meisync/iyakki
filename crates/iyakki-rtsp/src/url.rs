//! `rtsp://[user[:password]@]host[:port]/path?query` and `http://...`. Credentials inside an address are split off; displays and logs use
//! [`Url::display`] / [`strip_credentials`], which never contain them.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Rtsp,
    Http,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    /// Starts with `/`; includes the query.
    pub path: String,
    pub user: Option<String>,
    pub password: Option<String>,
}

pub const MAX_URL: usize = 2048;

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse(text: &str) -> Result<Url, String> {
    let text = text.trim();
    if text.len() > MAX_URL {
        return Err("that address is too long".into());
    }
    if text.chars().any(|c| c.is_control() || c == ' ') {
        return Err("an address has no spaces or control characters".into());
    }
    let lower = text.to_ascii_lowercase();
    let (scheme, rest, dport) = if lower.starts_with("rtsp://") {
        (Scheme::Rtsp, &text[7..], 554)
    } else if lower.starts_with("http://") {
        (Scheme::Http, &text[7..], 80)
    } else if lower.starts_with("https://") || lower.starts_with("rtsps://") {
        return Err("encrypted (https / rtsps) camera streams are not supported yet; use rtsp:// or http:// on your own network".into());
    } else {
        return Err("a camera address starts with rtsp:// or http://".into());
    };
    let (auth_host, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    let (userinfo, hostport) = match auth_host.rfind('@') {
        Some(i) => (Some(&auth_host[..i]), &auth_host[i + 1..]),
        None => (None, auth_host),
    };
    let (user, password) = match userinfo {
        None => (None, None),
        Some(u) => match u.split_once(':') {
            Some((a, b)) => (Some(unescape(a)), Some(unescape(b))),
            None => (Some(unescape(u)), None),
        },
    };
    let (host, port) = if let Some(r) = hostport.strip_prefix('[') {
        let (h, after) = r.split_once(']').ok_or("an IPv6 address needs a closing ]")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse::<u16>().map_err(|_| "the port is not a number")?,
            None if after.is_empty() => dport,
            None => return Err("unexpected text after the IPv6 address".into()),
        };
        (h.to_string(), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "the port is not a number")?),
            None => (hostport.to_string(), dport),
        }
    };
    if host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')) {
        return Err("the host name in that address is not valid".into());
    }
    if port == 0 {
        return Err("the port must not be 0".into());
    }
    Ok(Url { scheme, host, port, path, user, password })
}

impl Url {
    /// `rtsp://host:port/path`, never with a user name or password.
    pub fn display(&self) -> String {
        let h = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{}://{}:{}{}", if self.scheme == Scheme::Rtsp { "rtsp" } else { "http" }, h, self.port, self.path)
    }
    /// The address without the default port (what goes into a request line).
    pub fn request_uri(&self) -> String {
        let h = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let dp = if self.scheme == Scheme::Rtsp { 554 } else { 80 };
        let sch = if self.scheme == Scheme::Rtsp { "rtsp" } else { "http" };
        if self.port == dp { format!("{sch}://{h}{}", self.path) } else { format!("{sch}://{h}:{}{}", self.port, self.path) }
    }
    pub fn credentials(&self) -> Option<(String, String)> {
        self.user.clone().map(|u| (u, self.password.clone().unwrap_or_default()))
    }
}

/// The text with any `user:password@` removed, for displays, logs and error messages. Works on text that is not a valid address too.
pub fn strip_credentials(text: &str) -> String {
    let Some(i) = text.find("://") else { return text.to_string() };
    let (scheme, rest) = text.split_at(i + 3);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    match rest[..end].rfind('@') {
        Some(at) => format!("{scheme}{}", &rest[at + 1..]),
        None => text.to_string(),
    }
}

/// Every place in `text` that looks like an address with credentials is cleaned (for an error message that quotes a server).
pub fn scrub(text: &str) -> String {
    text.split(' ').map(|w| if w.contains("://") && w.contains('@') { strip_credentials(w) } else { w.to_string() }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split_into_parts_and_credentials_are_decoded() {
        let u = parse("rtsp://admin:p%40ss@192.168.1.20:8554/stream1?x=1").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("192.168.1.20", 8554, "/stream1?x=1"));
        assert_eq!(u.credentials(), Some(("admin".into(), "p@ss".into())));
        assert_eq!(u.display(), "rtsp://192.168.1.20:8554/stream1?x=1");
        let d = parse("rtsp://cam.lan").unwrap();
        assert_eq!((d.port, d.path.as_str(), d.credentials()), (554, "/", None));
        assert_eq!(parse("http://[fe80::1]:81/video").unwrap().host, "fe80::1");
        assert_eq!(parse("RTSP://h?q=1").unwrap().path, "/?q=1");
    }

    #[test]
    fn bad_addresses_are_refused_in_words() {
        for bad in ["", "ftp://x", "rtsp://", "rtsp://:554/", "rtsp://h:99999/", "rtsp://h:0/", "rtsp://h /x", "rtsp://[::1/x", "rtsp://ho st/", "rtsp://h/\u{0}"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse("https://cam/").unwrap_err().contains("not supported"));
        assert!(parse(&format!("rtsp://h/{}", "a".repeat(3000))).is_err());
    }

    #[test]
    fn credentials_never_survive_display_stripping_or_scrubbing() {
        assert_eq!(strip_credentials("rtsp://admin:secret@10.0.0.5/live"), "rtsp://10.0.0.5/live");
        assert_eq!(strip_credentials("rtsp://admin@10.0.0.5/live?u=a@b"), "rtsp://10.0.0.5/live?u=a@b");
        assert_eq!(strip_credentials("rtsp://10.0.0.5/a@b"), "rtsp://10.0.0.5/a@b");
        assert_eq!(strip_credentials("not an address"), "not an address");
        assert_eq!(strip_credentials("rtsp://a:b@c:d@host/x"), "rtsp://host/x", "the last @ ends the credentials");
        assert_eq!(scrub("cannot reach rtsp://u:pw@h/x now"), "cannot reach rtsp://h/x now");
        assert!(!parse("rtsp://u:pw@h/x").unwrap().display().contains("pw"));
    }
}
