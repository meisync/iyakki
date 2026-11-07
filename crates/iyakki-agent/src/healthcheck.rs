//! `iyakki healthcheck [URL]`: a container's HEALTHCHECK without curl. Asks `GET /healthz` (default `https://127.0.0.1:8443/healthz`, or
//! `IYAKKI_HEALTH_URL`) and exits 0 on a 200. The certificate is not checked: the address is this machine's own loopback.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub const DEFAULT_URL: &str = "https://127.0.0.1:8443/healthz";

pub fn parse_url(u: &str) -> Result<Target, String> {
    let (tls, rest) = if let Some(r) = u.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = u.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("{u}: expected http:// or https://"));
    };
    let (hp, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/healthz"),
    };
    let (host, port) = match hp.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(':') && !p.contains(']') => (h.trim_matches(|c| c == '[' || c == ']'), p.parse::<u16>().map_err(|_| format!("{u}: bad port"))?),
        _ => (hp.trim_matches(|c| c == '[' || c == ']'), if tls { 443 } else { 80 }),
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
        return Err(format!("{u}: bad host"));
    }
    Ok(Target { tls, host: host.into(), port, path: path.into() })
}

/// The status code of an HTTP reply.
pub fn status_of(reply: &[u8]) -> Option<u16> {
    let head = std::str::from_utf8(&reply[..reply.len().min(64)]).ok()?;
    head.strip_prefix("HTTP/1.")?.split_whitespace().nth(1)?.parse().ok()
}

pub fn check(t: &Target) -> Result<u16, String> {
    let timeout = Duration::from_secs(4);
    if t.tls {
        let r = crate::devmode::client::https(&t.host, t.port, None, "GET", &t.path, &[], b"", timeout)?;
        return Ok(r.status);
    }
    let mut s = TcpStream::connect((t.host.as_str(), t.port)).map_err(|e| format!("{}:{}: {e}", t.host, t.port))?;
    let _ = s.set_read_timeout(Some(timeout));
    s.write_all(format!("GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", t.path, t.host).as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let _ = s.take(4096).read_to_end(&mut buf);
    status_of(&buf).ok_or_else(|| "no HTTP reply".to_string())
}

/// The command: returns the exit code.
pub fn cli_main(a: &[String], env_url: Option<String>, out: &mut dyn FnMut(&str)) -> i32 {
    let url = a.first().cloned().or(env_url).unwrap_or_else(|| DEFAULT_URL.to_string());
    match parse_url(&url).and_then(|t| check(&t)) {
        Ok(200) => 0,
        Ok(c) => {
            out(&format!("unhealthy: {url} answered {c}"));
            1
        }
        Err(e) => {
            out(&format!("unhealthy: {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_split() {
        assert_eq!(parse_url(DEFAULT_URL).unwrap(), Target { tls: true, host: "127.0.0.1".into(), port: 8443, path: "/healthz".into() });
        assert_eq!(parse_url("http://localhost").unwrap().port, 80);
        assert_eq!(parse_url("https://[::1]:9/x").unwrap(), Target { tls: true, host: "::1".into(), port: 9, path: "/x".into() });
        assert!(parse_url("ftp://x").is_err() && parse_url("http://:80/").is_err() && parse_url("http://a:99999/").is_err());
    }

    #[test]
    fn status_lines_are_read() {
        assert_eq!(status_of(b"HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(status_of(b"HTTP/1.0 503 x"), Some(503));
        assert_eq!(status_of(b"nope"), None);
    }

    #[test]
    fn a_plain_listener_answering_200_is_healthy_and_a_closed_port_is_not() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 256];
            let _ = s.read(&mut b);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
        });
        let mut said = Vec::new();
        assert_eq!(cli_main(&[format!("http://127.0.0.1:{port}/healthz")], None, &mut |s| said.push(s.to_string())), 0, "{said:?}");
        h.join().unwrap();
        assert_eq!(cli_main(&[format!("http://127.0.0.1:{port}/healthz")], None, &mut |_| {}), 1);
    }
}
