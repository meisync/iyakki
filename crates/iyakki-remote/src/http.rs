//! Just enough HTTP/1.1 for the browser front end: read one request (with hard limits), write one response. Every
//! connection serves a single request (`Connection: close`) or becomes a WebSocket.

use std::collections::HashMap;

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_BODY: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parse {
    Incomplete,
    Done(Request),
    Bad(u16, &'static str),
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The value of a cookie in the `Cookie` header.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
    }

    pub fn wants_websocket(&self) -> bool {
        self.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
            && self.header("connection").is_some_and(|c| c.to_ascii_lowercase().contains("upgrade"))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() => {
                let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(b[i + 1]) << 4 | hex(b[i + 2]));
                i += 3;
                continue;
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse what has been received so far.
pub fn parse(buf: &[u8]) -> Parse {
    let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > MAX_HEAD { Parse::Bad(431, "request headers too large") } else { Parse::Incomplete };
    };
    if head_end > MAX_HEAD {
        return Parse::Bad(431, "request headers too large");
    }
    let Ok(head) = std::str::from_utf8(&buf[..head_end]) else { return Parse::Bad(400, "bad request") };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version), None) = (first.next(), first.next(), first.next(), first.next()) else {
        return Parse::Bad(400, "bad request line");
    };
    if !version.starts_with("HTTP/1.") {
        return Parse::Bad(505, "HTTP version not supported");
    }
    if !target.starts_with('/') || target.contains(['\0', '\\']) {
        return Parse::Bad(400, "bad path");
    }
    let mut headers = HashMap::new();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { return Parse::Bad(400, "bad header") };
        if headers.len() >= 64 {
            return Parse::Bad(431, "too many headers");
        }
        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
    }
    if headers.contains_key("transfer-encoding") {
        return Parse::Bad(501, "chunked requests are not supported");
    }
    let len = match headers.get("content-length") {
        None => 0,
        Some(v) => match v.parse::<usize>() {
            Ok(n) if n <= MAX_BODY => n,
            Ok(_) => return Parse::Bad(413, "request body too large"),
            Err(_) => return Parse::Bad(400, "bad content length"),
        },
    };
    let body_at = head_end + 4;
    if buf.len() < body_at + len {
        return Parse::Incomplete;
    }
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let query = q.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (percent_decode(k), percent_decode(v))).collect();
    Parse::Done(Request { method: method.to_ascii_uppercase(), path: percent_decode(path), query, headers, body: buf[body_at..body_at + len].to_vec() })
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        204 => "No Content",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response { status, headers: vec![("Content-Type".into(), content_type.into())], body: body.into() }
    }

    pub fn html(body: &str) -> Response {
        Response::new(200, "text/html; charset=utf-8", body)
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response::new(status, "application/json", v.to_string())
    }

    pub fn error(status: u16, msg: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": msg }))
    }

    pub fn header(mut self, k: &str, v: &str) -> Response {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n", self.status, reason(self.status), self.body.len()).into_bytes();
        // the page runs nothing from outside and may not be put in a frame
        for (k, v) in [
            ("X-Content-Type-Options", "nosniff"),
            ("X-Frame-Options", "DENY"),
            ("Referrer-Policy", "no-referrer"),
            ("Cache-Control", "no-store"),
            ("Content-Security-Policy", "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
        ] {
            if !self.headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(k)) {
                out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
            }
        }
        for (k, v) in &self.headers {
            // a header value can never carry a line break, whatever put it here
            let v: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
            out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str) -> Parse {
        parse(s.as_bytes())
    }

    #[test]
    fn a_request_is_read_with_its_query_cookies_and_body() {
        let p = req("POST /login?a=1&b=x%20y HTTP/1.1\r\nHost: h\r\nCookie: s=abc; t=2\r\nContent-Length: 4\r\n\r\nbody");
        let Parse::Done(r) = p else { panic!("{p:?}") };
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/login"));
        assert_eq!(r.query["a"], "1");
        assert_eq!(r.query["b"], "x y");
        assert_eq!(r.cookie("s"), Some("abc"));
        assert_eq!(r.cookie("t"), Some("2"));
        assert_eq!(r.cookie("u"), None);
        assert_eq!(r.body, b"body");
    }

    #[test]
    fn a_request_that_has_not_fully_arrived_is_incomplete() {
        assert_eq!(req("GET / HTTP/1.1\r\nHost: h\r\n"), Parse::Incomplete);
        assert_eq!(req("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc"), Parse::Incomplete);
    }

    #[test]
    fn limits_and_malformed_requests_are_refused() {
        assert!(matches!(req("GET / HTTP/2.0\r\n\r\n"), Parse::Bad(505, _)));
        assert!(matches!(req("GET nothing HTTP/1.1\r\n\r\n"), Parse::Bad(400, _)));
        assert!(matches!(req("GET /a\\b HTTP/1.1\r\n\r\n"), Parse::Bad(400, _)));
        assert!(matches!(req("GET / HTTP/1.1 extra\r\n\r\n"), Parse::Bad(400, _)));
        assert!(matches!(req("GET / HTTP/1.1\r\nbroken header\r\n\r\n"), Parse::Bad(400, _)));
        assert!(matches!(req("POST / HTTP/1.1\r\nContent-Length: 999999\r\n\r\n"), Parse::Bad(413, _)));
        assert!(matches!(req("POST / HTTP/1.1\r\nContent-Length: x\r\n\r\n"), Parse::Bad(400, _)));
        assert!(matches!(req("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"), Parse::Bad(501, _)));
        let long = format!("GET /{} HTTP/1.1", "a".repeat(MAX_HEAD));
        assert!(matches!(parse(long.as_bytes()), Parse::Bad(431, _)));
    }

    #[test]
    fn a_websocket_request_is_recognised() {
        let Parse::Done(r) = req("GET /ws HTTP/1.1\r\nUpgrade: WebSocket\r\nConnection: keep-alive, Upgrade\r\n\r\n") else { panic!() };
        assert!(r.wants_websocket());
        let Parse::Done(r) = req("GET /ws HTTP/1.1\r\n\r\n") else { panic!() };
        assert!(!r.wants_websocket());
    }

    #[test]
    fn responses_carry_safety_headers_and_no_line_breaks_in_values() {
        let r = Response::html("<p>hi</p>").header("Set-Cookie", "a=b\r\nInjected: yes");
        let text = String::from_utf8(r.to_bytes()).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 9\r\n"));
        assert!(text.contains("X-Frame-Options: DENY") && text.contains("nosniff") && text.contains("Content-Security-Policy"));
        assert!(!text.contains("\r\nInjected: yes"), "no header injection");
        assert!(text.ends_with("\r\n\r\n<p>hi</p>"));
    }
}
