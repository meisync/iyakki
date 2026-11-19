//! HTTP/2 through an HTTP proxy (F-BROX-22): the `CONNECT` tunnel (RFC 9110 section 9.3.6). The proxy is asked for a tunnel to
//! `host:port`; after its 2xx answer the socket carries the TLS handshake and then HTTP/2 as if it were direct. Only the answer's head is
//! read, byte by byte, so not one byte of the tunnel is taken. The proxy's own sign-in (Basic) is sent only when the person gave one; the
//! proxy is never trusted with the page: the TLS certificate is still checked end to end.

use std::io::{Read, Write};

#[derive(Debug, PartialEq, Eq)]
pub enum ProxyError {
    /// The proxy wants a user name and password (407) and none, or a wrong one, was given.
    AuthRequired,
    Refused(u16),
    Bad(String),
    Io(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyError::AuthRequired => write!(f, "the proxy asks for a user name and password"),
            ProxyError::Refused(c) => write!(f, "the proxy refused to connect ({c})"),
            ProxyError::Bad(w) => write!(f, "{w}"),
            ProxyError::Io(w) => write!(f, "connection problem: {w}"),
        }
    }
}

const MAX_HEAD: usize = 16 * 1024;

fn b64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in d.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

/// The request line and headers of the tunnel request. Host names with anything but letters, digits, dots, dashes and colons (an IPv6
/// address in brackets is written as given) are refused so a name cannot add a header.
pub fn connect_request(host: &str, port: u16, creds: Option<(&str, &str)>) -> Result<Vec<u8>, ProxyError> {
    let plain = host.trim_matches(['[', ']']);
    if plain.is_empty() || plain.len() > 253 || !plain.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_')) || port == 0 {
        return Err(ProxyError::Bad("that host name cannot go through a proxy".into()));
    }
    let target = if plain.contains(':') { format!("[{plain}]:{port}") } else { format!("{plain}:{port}") };
    let mut r = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Some((u, p)) = creds {
        if u.contains(['\r', '\n']) || p.contains(['\r', '\n']) {
            return Err(ProxyError::Bad("the proxy sign-in has a line break in it".into()));
        }
        r.push_str(&format!("Proxy-Authorization: Basic {}\r\n", b64(format!("{u}:{p}").as_bytes())));
    }
    r.push_str("\r\n");
    Ok(r.into_bytes())
}

/// Ask the proxy on `s` for a tunnel; on `Ok` the socket is the tunnel (nothing of it was read).
pub fn open_tunnel<S: Read + Write>(s: &mut S, host: &str, port: u16, creds: Option<(&str, &str)>) -> Result<(), ProxyError> {
    s.write_all(&connect_request(host, port, creds)?).map_err(|e| ProxyError::Io(e.to_string()))?;
    s.flush().map_err(|e| ProxyError::Io(e.to_string()))?;
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err(ProxyError::Bad("the proxy's answer is too long".into()));
        }
        match s.read(&mut b) {
            Ok(0) => return Err(ProxyError::Bad("the proxy closed the connection".into())),
            Ok(_) => head.push(b[0]),
            Err(e) => return Err(ProxyError::Io(e.to_string())),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let code: u16 = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or_else(|| ProxyError::Bad("the proxy's answer is not HTTP".into()))?;
    match code {
        200..=299 => Ok(()),
        407 => Err(ProxyError::AuthRequired),
        c => Err(ProxyError::Refused(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A socket that reads from one buffer and records what was written.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        out: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.out.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn duplex(answer: &str) -> Duplex {
        Duplex { input: Cursor::new(answer.as_bytes().to_vec()), out: Vec::new() }
    }

    #[test]
    fn the_request_names_the_target_and_signs_in_only_when_given_a_sign_in() {
        let r = String::from_utf8(connect_request("example.org", 443, None).unwrap()).unwrap();
        assert!(r.starts_with("CONNECT example.org:443 HTTP/1.1\r\nHost: example.org:443\r\n") && r.ends_with("\r\n\r\n") && !r.contains("Authorization"));
        let r = String::from_utf8(connect_request("[::1]", 8443, Some(("ada", "pw"))).unwrap()).unwrap();
        assert!(r.starts_with("CONNECT [::1]:8443 HTTP/1.1") && r.contains("Proxy-Authorization: Basic YWRhOnB3\r\n"), "{r}");
        for bad in ["a b", "a\r\nX: y", "", "a/b"] {
            assert!(connect_request(bad, 443, None).is_err(), "{bad:?}");
        }
        assert!(connect_request("h", 0, None).is_err());
        assert!(connect_request("h", 443, Some(("a\r\nX: 1", "p"))).is_err());
    }

    #[test]
    fn a_2xx_answer_opens_the_tunnel_and_not_one_byte_of_it_is_read() {
        let mut d = duplex("HTTP/1.1 200 Connection established\r\nVia: p\r\n\r\nTUNNEL-BYTES");
        open_tunnel(&mut d, "example.org", 443, None).unwrap();
        let mut rest = String::new();
        d.input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "TUNNEL-BYTES");
        assert!(String::from_utf8_lossy(&d.out).starts_with("CONNECT example.org:443"));
    }

    #[test]
    fn refusals_a_missing_sign_in_and_junk_are_told_in_words() {
        assert_eq!(open_tunnel(&mut duplex("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"), "h", 443, None), Err(ProxyError::AuthRequired));
        assert_eq!(open_tunnel(&mut duplex("HTTP/1.1 403 Forbidden\r\n\r\n"), "h", 443, None), Err(ProxyError::Refused(403)));
        assert!(matches!(open_tunnel(&mut duplex("SSH-2.0-x\r\n\r\n"), "h", 443, None), Err(ProxyError::Bad(_))));
        assert!(matches!(open_tunnel(&mut duplex("HTTP/1.1 200 OK\r\n"), "h", 443, None), Err(ProxyError::Bad(_))), "closed before the end of the head");
        let long = format!("HTTP/1.1 200 OK\r\n{}", "X: y\r\n".repeat(5000));
        assert!(matches!(open_tunnel(&mut duplex(&long), "h", 443, None), Err(ProxyError::Bad(m)) if m.contains("too long")));
    }
}
