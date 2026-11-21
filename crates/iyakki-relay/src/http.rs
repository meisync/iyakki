//! The three routes, spoken by hand over any `Read + Write` (a TLS stream in the program, an in-memory pair in the tests).
//!
//!   POST /v1/relay/post             body: one encoded envelope (Content-Length required)    204, or 4xx with a plain sentence
//!   GET  /v1/relay/fetch/{mailbox}                                                          200 and a JSON list of hex envelopes
//!   GET  /healthz                                                                           200 `ok`
//!
//! HTTP/1.1 with keep-alive; no chunked bodies, no upgrade, no query; headers over 8 KiB, bodies over 64 KiB and requests that stall are cut.

use std::io::{Read, Write};
use std::sync::Mutex;

use crate::store::{Refusal, Store, MAX_ENVELOPE_BYTES};
use crate::Clock;

const MAX_HEAD: usize = 8 * 1024;
const MAX_HEADERS: usize = 32;
/// Requests served on one connection before it is closed.
pub const MAX_REQUESTS_PER_CONN: usize = 1000;

/// The relay as the connection handler sees it.
pub struct Relay {
    pub store: Mutex<Store>,
    pub clock: Box<dyn Clock>,
    /// Write one line per event (kinds and sizes only; never a body). Set to a no-op to be quiet.
    pub log: Box<dyn Fn(&str) + Send + Sync>,
}

impl Relay {
    pub fn new(store: Store, clock: Box<dyn Clock>, log: Box<dyn Fn(&str) + Send + Sync>) -> Relay {
        Relay { store: Mutex::new(store), clock, log }
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub close: bool,
}

fn text(status: u16, s: &str) -> Response {
    Response { status, content_type: "text/plain; charset=utf-8", body: format!("{s}\n").into_bytes(), close: false }
}

fn refused(r: Refusal) -> Response {
    text(r.status, r.why)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        411 => "Length Required",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Route one request. `peer` is the network address without its port.
pub fn route(relay: &Relay, peer: &str, method: &str, path: &str, body: &[u8]) -> Response {
    let now = relay.clock.now_ms();
    match (method, path) {
        ("GET", "/healthz") => text(200, "ok"),
        ("POST", "/v1/relay/post") => {
            let r = relay.store().post(peer, body, now);
            match r {
                Ok(()) => {
                    (relay.log)(&format!("post {} bytes", body.len()));
                    Response { status: 204, content_type: "text/plain", body: vec![], close: false }
                }
                Err(e) => {
                    (relay.log)(&format!("post refused {} ({})", e.status, e.why));
                    refused(e)
                }
            }
        }
        ("GET", p) if p.starts_with("/v1/relay/fetch/") => {
            let mailbox = &p["/v1/relay/fetch/".len()..];
            let r = relay.store().fetch(peer, mailbox, now);
            match r {
                Ok(list) => {
                    if !list.is_empty() {
                        (relay.log)(&format!("fetch handed over {}", list.len()));
                    }
                    let items: Vec<String> = list.iter().map(|b| format!("\"{}\"", hex(b))).collect();
                    Response { status: 200, content_type: "application/json", body: format!("[{}]", items.join(",")).into_bytes(), close: false }
                }
                Err(e) => {
                    (relay.log)(&format!("fetch refused {} ({})", e.status, e.why));
                    refused(e)
                }
            }
        }
        (_, p) if p == "/healthz" || p == "/v1/relay/post" || p.starts_with("/v1/relay/fetch/") => text(405, "that method is not served here"),
        _ => text(404, "there is nothing here"),
    }
}

/// Serve one connection until it closes, errs, stalls or reaches [`MAX_REQUESTS_PER_CONN`].
pub fn serve_connection<S: Read + Write>(s: &mut S, relay: &Relay, peer: &str) {
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..MAX_REQUESTS_PER_CONN {
        let req = match read_request(s, &mut buf) {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(resp) => {
                let _ = write_response(s, &resp, true);
                return;
            }
        };
        let mut resp = route(relay, peer, &req.method, &req.path, &req.body);
        resp.close |= req.close;
        let close = resp.close;
        if write_response(s, &resp, close).is_err() || close {
            return;
        }
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
    close: bool,
}

fn fail(status: u16, why: &str) -> Response {
    let mut r = text(status, why);
    r.close = true;
    r
}

/// Read one request from `s`, keeping bytes of the next one in `buf`. `Ok(None)`: the peer closed between requests.
fn read_request<S: Read>(s: &mut S, buf: &mut Vec<u8>) -> Result<Option<Request>, Response> {
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(fail(431, "the request head is too large"));
        }
        let mut chunk = [0u8; 4096];
        match s.read(&mut chunk) {
            Ok(0) => return if buf.is_empty() { Ok(None) } else { Err(fail(400, "the request ended early")) },
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return if buf.is_empty() { Ok(None) } else { Err(fail(408, "the request took too long")) };
            }
            Err(_) => return Ok(None),
        }
    };
    if end > MAX_HEAD {
        return Err(fail(431, "the request head is too large"));
    }
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| fail(400, "the request head is not text"))?.to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let (method, path, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method.is_empty() || !path.starts_with('/') || parts.next().is_some() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(fail(400, "that is not an HTTP/1.1 request"));
    }
    if path.contains(['?', '#']) {
        return Err(fail(400, "no query or fragment is used here"));
    }
    let mut length: Option<usize> = None;
    let mut close = version == "HTTP/1.0";
    for (n, l) in lines.enumerate() {
        if n >= MAX_HEADERS {
            return Err(fail(431, "too many headers"));
        }
        let Some((k, v)) = l.split_once(':') else { return Err(fail(400, "a header line has no colon")) };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "content-length" => {
                if length.is_some() {
                    return Err(fail(400, "two content lengths"));
                }
                length = Some(v.parse::<usize>().map_err(|_| fail(400, "the content length is not a number"))?);
            }
            "transfer-encoding" => return Err(fail(501, "chunked bodies are not accepted; send a Content-Length")),
            "connection" => {
                let v = v.to_ascii_lowercase();
                if v.contains("close") {
                    close = true;
                } else if v.contains("keep-alive") {
                    close = false;
                }
            }
            _ => {}
        }
    }
    let want = match (method, length) {
        ("POST", None) => return Err(fail(411, "a POST needs a Content-Length")),
        (_, Some(n)) => n,
        (_, None) => 0,
    };
    if want > MAX_ENVELOPE_BYTES {
        return Err(fail(413, "the envelope is over 64 KiB"));
    }
    let start = end + 4;
    while buf.len() < start + want {
        let mut chunk = [0u8; 8192];
        match s.read(&mut chunk) {
            Ok(0) => return Err(fail(400, "the body ended early")),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(fail(408, "the request took too long")),
            Err(_) => return Ok(None),
        }
    }
    let body = buf[start..start + want].to_vec();
    buf.drain(..start + want);
    Ok(Some(Request { method: method.to_string(), path: path.to_string(), body, close }))
}

fn write_response<S: Write>(s: &mut S, r: &Response, close: bool) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n", r.status, reason(r.status), r.body.len());
    if r.status != 204 {
        out.push_str(&format!("Content-Type: {}\r\n", r.content_type));
    }
    out.push_str(if close { "Connection: close\r\n\r\n" } else { "Connection: keep-alive\r\n\r\n" });
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&r.body);
    s.write_all(&bytes)?;
    s.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::ManualClock;
    use std::io::Cursor;

    const T0: u64 = 1_800_000_000_000;

    /// A stream that reads from a script and records what is written.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn relay(clock: &ManualClock) -> Relay {
        Relay::new(Store::default(), Box::new(clock.clone()), Box::new(|_| {}))
    }

    fn talk(r: &Relay, input: Vec<u8>) -> String {
        let mut d = Duplex { input: Cursor::new(input), output: vec![] };
        serve_connection(&mut d, r, "10.0.0.9");
        String::from_utf8_lossy(&d.output).to_string()
    }

    fn post_req(body: &str) -> Vec<u8> {
        format!("POST /v1/relay/post HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    fn env(seq: u64) -> String {
        format!(r#"{{"v":1,"mailbox":"mb","from":"f","seq":{seq},"sent_ms":{T0},"ttl_ms":60000,"kind":"msg","body":"ab"}}"#)
    }

    #[test]
    fn post_then_fetch_on_one_keep_alive_connection_and_once_only() {
        let clock = ManualClock::at(T0);
        let r = relay(&clock);
        let mut input = post_req(&env(1));
        input.extend(post_req(&env(2)));
        input.extend(b"GET /v1/relay/fetch/mb HTTP/1.1\r\nHost: x\r\n\r\n");
        input.extend(b"GET /v1/relay/fetch/mb HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        let out = talk(&r, input);
        assert_eq!(out.matches("HTTP/1.1 204").count(), 2, "{out}");
        assert_eq!(out.matches("HTTP/1.1 200").count(), 2);
        assert!(out.contains("Content-Type: application/json"));
        // hex of the first envelope is in the first fetch and the second fetch is empty
        let first = out.find("[\"").unwrap();
        assert!(out[first..].contains("7b2276223a31"), "hex of the envelope");
        assert!(out.trim_end().ends_with("[]"));
        assert!(out.contains("Connection: close"));
    }

    #[test]
    fn expiry_follows_the_injected_clock() {
        let clock = ManualClock::at(T0);
        let r = relay(&clock);
        talk(&r, post_req(&env(1)));
        clock.advance(60_000);
        let out = talk(&r, b"GET /v1/relay/fetch/mb HTTP/1.1\r\n\r\n".to_vec());
        assert!(out.ends_with("[]"), "{out}");
        assert_eq!(r.store().expired, 1);
    }

    #[test]
    fn bad_requests_get_a_status_and_a_sentence() {
        let clock = ManualClock::at(T0);
        let r = relay(&clock);
        assert!(talk(&r, b"POST /v1/relay/post HTTP/1.1\r\n\r\n".to_vec()).starts_with("HTTP/1.1 411"));
        assert!(talk(&r, b"POST /v1/relay/post HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec()).starts_with("HTTP/1.1 501"));
        let big = format!("POST /v1/relay/post HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_ENVELOPE_BYTES + 1);
        assert!(talk(&r, big.into_bytes()).starts_with("HTTP/1.1 413"));
        assert!(talk(&r, b"GET /nope HTTP/1.1\r\n\r\n".to_vec()).starts_with("HTTP/1.1 404"));
        assert!(talk(&r, b"PUT /v1/relay/post HTTP/1.1\r\n\r\n".to_vec()).starts_with("HTTP/1.1 405"));
        assert!(talk(&r, b"GET /v1/relay/fetch/a?b=c HTTP/1.1\r\n\r\n".to_vec()).starts_with("HTTP/1.1 400"));
        assert!(talk(&r, b"GET /v1/relay/fetch/a%20b HTTP/1.1\r\n\r\n".to_vec()).starts_with("HTTP/1.1 400"));
        assert!(talk(&r, b"nonsense\r\n\r\n".to_vec()).starts_with("HTTP/1.1 400"));
        assert!(talk(&r, post_req("not an envelope")).starts_with("HTTP/1.1 400"));
        let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(9000));
        assert!(talk(&r, long.into_bytes()).starts_with("HTTP/1.1 431"));
        assert!(talk(&r, b"GET /healthz HTTP/1.1\r\n\r\n".to_vec()).contains("ok"));
        // a body cut short
        assert!(talk(&r, b"POST /v1/relay/post HTTP/1.1\r\nContent-Length: 50\r\n\r\nshort".to_vec()).starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn a_post_over_the_depth_is_refused_with_429_and_nothing_logs_a_body() {
        let clock = ManualClock::at(T0);
        let lines = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        let r = Relay::new(
            Store::new(crate::store::Limits { mailbox_depth: 1, ..Default::default() }),
            Box::new(clock.clone()),
            Box::new(move |s| l2.lock().unwrap().push(s.to_string())),
        );
        assert!(talk(&r, post_req(&env(1))).starts_with("HTTP/1.1 204"));
        assert!(talk(&r, post_req(&env(2))).starts_with("HTTP/1.1 429"));
        let log = lines.lock().unwrap().join("\n");
        assert!(log.contains("post refused 429") && !log.contains("\"ab\"") && !log.contains("mb"));
    }
}
