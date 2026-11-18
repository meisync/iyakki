//! HTTP/2 on the domain listener (`cloudlisten_h2serve.rs`). Loopback only. Three kinds of client: the real `curl` (an independent HTTP/2
//! implementation, nghttp2), a small hand-written client of this file for what curl will not do (tiny windows, floods, bad frames), and
//! a rustls client that negotiates `h2` by ALPN.

use super::*;
use std::collections::HashMap;
use std::io;
use crate::cloudlisten_h2::{self as f, ErrorCode, FrameType};
use iyakki_proxy::hpack::{Decoder, Encoder};

// ------------------------------------------------------------------------------------------------------------ a small client

struct H2<S: Read + Write> {
    s: S,
    enc: Encoder,
    dec: Decoder,
    buf: Vec<u8>,
}

#[derive(Default, Debug)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// How the stream ended: `None` when the connection did, "end" for END_STREAM, or the code of RST_STREAM.
    reset: Option<ErrorCode>,
    ended: bool,
}

impl<S: Read + Write> H2<S> {
    fn start(mut s: S, settings: &[(u16, u32)]) -> H2<S> {
        let mut p = Vec::new();
        for (id, v) in settings {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&v.to_be_bytes());
        }
        s.write_all(f::PREFACE).unwrap();
        let mut c = H2 { s, enc: Encoder::default(), dec: Decoder::new(4096), buf: Vec::new() };
        c.send(FrameType::Settings, 0, 0, &p);
        c
    }

    fn send(&mut self, ty: FrameType, flags: u8, stream: u32, payload: &[u8]) {
        // a server that has ended the connection may refuse more: the test then reads what it said
        let _ = self.s.write_all(&f::encode_frame(ty, flags, stream, payload).unwrap());
    }

    fn headers(&mut self, id: u32, method: &str, path: &str, host: &str, extra: &[(&str, &str)], end_stream: bool) {
        let mut list = vec![(":method".to_string(), method.to_string()), (":scheme".into(), "http".into()), (":path".into(), path.into()), (":authority".into(), host.into())];
        list.extend(extra.iter().map(|(n, v)| (n.to_string(), v.to_string())));
        let block = self.enc.encode(&list);
        self.send(FrameType::Headers, 0x4 | if end_stream { 0x1 } else { 0 }, id, &block);
    }

    fn get(&mut self, id: u32, host: &str, path: &str) {
        self.headers(id, "GET", path, host, &[], true);
    }

    /// The next frame, or None when the connection ended or nothing came within `wait`.
    fn frame(&mut self, wait: Duration) -> Option<(f::FrameHeader, Vec<u8>)> {
        let until = Instant::now() + wait;
        loop {
            if let Some(h) = f::parse_header(&self.buf) {
                let total = f::HEADER_LEN + h.len as usize;
                if self.buf.len() >= total {
                    let p = self.buf[f::HEADER_LEN..total].to_vec();
                    self.buf.drain(..total);
                    return Some((h, p));
                }
            }
            if Instant::now() > until {
                return None;
            }
            let mut tmp = [0u8; 65536];
            match self.s.read(&mut tmp) {
                Ok(0) => return None,
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(_) => return None,
            }
        }
    }

    /// Read until stream `id` ends or is reset (or the connection closes), answering SETTINGS and PING and giving windows back as data arrives.
    fn answer(&mut self, id: u32, give_windows: bool) -> Answer {
        let mut a = Answer::default();
        let mut block: Vec<u8> = Vec::new();
        while let Some((h, p)) = self.frame(Duration::from_secs(15)) {
            match h.ty {
                FrameType::Settings if h.flags & 1 == 0 => self.send(FrameType::Settings, 1, 0, &[]),
                FrameType::Ping if h.flags & 1 == 0 => self.send(FrameType::Ping, 1, 0, &p),
                FrameType::GoAway => {
                    a.reset = Some(ErrorCode::from_u32(u32::from_be_bytes([p[4], p[5], p[6], p[7]])));
                    return a;
                }
                FrameType::Headers | FrameType::Continuation if h.stream == id => {
                    let mut body = f::strip_padding(h.flags, &p).unwrap().to_vec();
                    if h.ty == FrameType::Headers && h.flags & 0x20 != 0 {
                        body.drain(..5);
                    }
                    block.extend(body);
                    if h.flags & 0x4 != 0 {
                        let list = self.dec.decode(&block).unwrap();
                        block.clear();
                        for (n, v) in list {
                            if n == ":status" {
                                a.status = v.parse().unwrap();
                            } else {
                                a.headers.push((n, v));
                            }
                        }
                    }
                    if h.flags & 0x1 != 0 {
                        a.ended = true;
                        return a;
                    }
                }
                FrameType::Data if h.stream == id => {
                    a.body.extend_from_slice(&p);
                    if give_windows && !p.is_empty() {
                        let inc = (p.len() as u32).to_be_bytes();
                        self.send(FrameType::WindowUpdate, 0, 0, &inc);
                        self.send(FrameType::WindowUpdate, 0, id, &inc);
                    }
                    if h.flags & 0x1 != 0 {
                        a.ended = true;
                        return a;
                    }
                }
                FrameType::RstStream if h.stream == id => {
                    a.reset = Some(ErrorCode::from_u32(u32::from_be_bytes([p[0], p[1], p[2], p[3]])));
                    return a;
                }
                _ => {}
            }
        }
        a
    }
}

fn h2(port: u16, settings: &[(u16, u32)]) -> H2<TcpStream> {
    let s = connect(port);
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    H2::start(s, settings)
}

fn hdr<'a>(a: &'a Answer, n: &str) -> Option<&'a str> {
    a.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
}

fn big_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

// ------------------------------------------------------------------------------------------------------------ the real curl

fn curl_with_http2() -> bool {
    std::process::Command::new("curl").arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).contains("HTTP2")).unwrap_or(false)
}

fn curl(args: &[&str]) -> (String, Vec<u8>) {
    let o = std::process::Command::new("curl").args(["-sS", "--max-time", "30"]).args(args).output().expect("curl runs");
    (String::from_utf8_lossy(&o.stderr).into_owned(), o.stdout)
}

#[test]
fn curl_speaks_http2_to_the_listener_and_gets_what_http1_gets() {
    if !curl_with_http2() {
        eprintln!("curl with HTTP/2 is not installed here: this test did not run");
        return;
    }
    let (a, b) = (upstream("a"), upstream("b"));
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p), route(A, "/api", b.port, p)]);
    let url = |path: &str| format!("http://127.0.0.1:{p}{path}");
    // a request and its head: HTTP/2, routed by the :authority (curl sends the Host as the authority)
    let (err, out) = curl(&["--http2-prior-knowledge", "-i", "-H", &format!("Host: {A}"), &url("/x?q=1")]);
    assert!(err.is_empty(), "{err}");
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(text.starts_with("HTTP/2 200"), "{text}");
    assert!(text.ends_with("name=a path=/x?q=1"), "{text}");
    // the same host with another path prefix goes to the other upstream
    let (_, out) = curl(&["--http2-prior-knowledge", "-H", &format!("Host: {A}"), &url("/api/v1")]);
    assert_eq!(String::from_utf8_lossy(&out), "name=b path=/api/v1");
    // the target saw an ordinary HTTP/1.1 request with the forwarding headers
    let head = saw(&a).to_lowercase();
    assert!(head.starts_with("get /x?q=1 http/1.1") && head.contains(&format!("host: {A}\r\n")) && head.contains("x-forwarded-for: 127.0.0.1") && head.contains("connection: close"), "{head}");
    // a chunked answer arrives whole
    let (_, out) = curl(&["--http2-prior-knowledge", "-H", &format!("Host: {A}"), &url("/chunked")]);
    assert_eq!(String::from_utf8_lossy(&out), "alphabetagamma");
    // an unknown host is the same 404 as on HTTP/1.1
    let (_, out) = curl(&["--http2-prior-knowledge", "-i", "-H", "Host: nobody.example.org", &url("/")]);
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/2 404"));
    // a body upload (200 000 bytes) and a 3 MB download that needs window updates both ways
    let dir = tmp("h2curl");
    let up: Vec<u8> = (0..200_000usize).map(|i| (i % 200) as u8).collect();
    std::fs::write(dir.join("up.bin"), &up).unwrap();
    let sum: u64 = up.iter().map(|b| *b as u64).sum();
    let (err, out) = curl(&["--http2-prior-knowledge", "-H", &format!("Host: {A}"), "--data-binary", &format!("@{}", dir.join("up.bin").display()), &url("/echo")]);
    assert!(err.is_empty(), "{err}");
    assert_eq!(String::from_utf8_lossy(&out), format!("len=200000 sum={sum}"));
    let (err, out) = curl(&["--http2-prior-knowledge", "-H", &format!("Host: {A}"), &url("/big")]);
    assert!(err.is_empty(), "{err}");
    assert_eq!(out.len(), 3_000_000);
    assert!(out == big_pattern(3_000_000), "the download is the bytes the target sent");
    // several requests at once on one connection
    let (err, out) = curl(&["--http2-prior-knowledge", "--parallel", "--parallel-immediate", "-H", &format!("Host: {A}"), &url("/one"), &url("/two"), &url("/three"), &url("/four")]);
    assert!(err.is_empty(), "{err}");
    let all = String::from_utf8_lossy(&out).into_owned();
    for want in ["name=a path=/one", "name=a path=/two", "name=a path=/three", "name=a path=/four"] {
        assert!(all.contains(want), "{all}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn curl_negotiates_h2_by_alpn_on_a_tls_route_and_the_answer_is_the_same() {
    if !curl_with_http2() {
        eprintln!("curl with HTTP/2 is not installed here: this test did not run");
        return;
    }
    let a = upstream("a");
    let dir = tmp("h2tls");
    let (chain, key) = self_signed(A).unwrap();
    write_cert(&dir, "appcert", &chain, &key);
    let p = free_port();
    let (o, _) = opts();
    let l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let (err, out) = curl(&["--http2", "-i", "-k", "--resolve", &format!("{A}:{p}:127.0.0.1"), &format!("https://{A}:{p}/secure")]);
    assert!(err.is_empty(), "{err}");
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(text.starts_with("HTTP/2 200") && text.ends_with("name=a path=/secure"), "{text}");
    assert!(saw(&a).to_lowercase().contains("x-forwarded-proto: https"));
    // a client that asks only for HTTP/1.1 still gets it
    let (_, out) = curl(&["--http1.1", "-i", "-k", "--resolve", &format!("{A}:{p}:127.0.0.1"), &format!("https://{A}:{p}/secure")]);
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 200"));
    drop(l);
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------------------------------------------------ the hand-written client

#[test]
fn a_rustls_client_that_asks_for_h2_gets_it_and_one_that_does_not_gets_http1() {
    let a = upstream("a");
    let dir = tmp("h2alpn");
    let (chain, key) = self_signed(A).unwrap();
    write_cert(&dir, "appcert", &chain, &key);
    let p = free_port();
    let (o, _) = opts();
    let _l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(chain.as_bytes()).unwrap()).unwrap();
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_root_certificates(roots).with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let mut tcp = connect(p);
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), ServerName::try_from(A.to_string()).unwrap()).unwrap();
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).unwrap();
    }
    assert_eq!(conn.alpn_protocol(), Some(b"h2".as_slice()), "the listener picks h2 when it is offered");
    tcp.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut c = H2::start(rustls::StreamOwned::new(conn, tcp), &[]);
    c.get(1, A, "/over-tls");
    let r = c.answer(1, true);
    assert_eq!((r.status, String::from_utf8_lossy(&r.body).into_owned()), (200, "name=a path=/over-tls".into()));
    assert!(saw(&a).to_lowercase().contains("x-forwarded-proto: https"));
    // a client that offers only http/1.1 is served HTTP/1.1
    let mut cfg1 = rustls::ClientConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_root_certificates({
        let mut r = rustls::RootCertStore::empty();
        r.add(CertificateDer::from_pem_slice(chain.as_bytes()).unwrap()).unwrap();
        r
    }).with_no_client_auth();
    cfg1.alpn_protocols = vec![b"http/1.1".to_vec()];
    let (resp, _) = tls_get(p, A, &Arc::new(cfg1), A, "/one").unwrap();
    assert_eq!(text(&resp), "name=a path=/one");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_stream_window_of_the_client_is_kept_and_a_window_update_lets_more_through() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let mut c = h2(p, &[(4, 1000)]); // SETTINGS_INITIAL_WINDOW_SIZE = 1000
    c.get(1, A, "/big");
    let mut got = 0usize;
    let mut head = false;
    while let Some((h, pl)) = c.frame(Duration::from_millis(700)) {
        match h.ty {
            FrameType::Settings if h.flags & 1 == 0 => c.send(FrameType::Settings, 1, 0, &[]),
            FrameType::Headers => head = true,
            FrameType::Data => {
                got += pl.len();
                let inc = (pl.len() as u32).to_be_bytes();
                c.send(FrameType::WindowUpdate, 0, 0, &inc); // the connection's window is kept open, the stream's is not
            }
            _ => {}
        }
    }
    assert!(head, "the head is not held back by the window");
    assert_eq!(got, 1000, "exactly the stream window, then the server waits");
    c.send(FrameType::WindowUpdate, 0, 1, &(5000u32).to_be_bytes());
    let mut more = 0usize;
    while let Some((h, pl)) = c.frame(Duration::from_millis(700)) {
        if h.ty == FrameType::Data {
            more += pl.len();
            c.send(FrameType::WindowUpdate, 0, 0, &(pl.len() as u32).to_be_bytes());
        }
    }
    assert_eq!(more, 5000, "exactly what the update allowed");
    // open it fully and the rest arrives, in order
    c.send(FrameType::WindowUpdate, 0, 1, &(2_000_000_000u32).to_be_bytes());
    let mut rest: Vec<u8> = Vec::new();
    while let Some((h, pl)) = c.frame(Duration::from_secs(10)) {
        if h.ty == FrameType::Data {
            rest.extend_from_slice(&pl);
            c.send(FrameType::WindowUpdate, 0, 0, &(pl.len() as u32).to_be_bytes());
            if h.flags & 1 != 0 {
                break;
            }
        }
    }
    assert_eq!(rest.len(), 3_000_000 - 6000);
    assert!(rest == big_pattern(3_000_000)[6000..], "no byte lost or repeated");
}

#[test]
fn many_streams_at_once_each_get_their_own_answer_and_head_has_no_body() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let mut c = h2(p, &[]);
    for i in 0..10u32 {
        c.get(1 + 2 * i, A, &format!("/s{i}"));
    }
    let mut want: Vec<u32> = (0..10).map(|i| 1 + 2 * i).collect();
    let mut bodies: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut ended: Vec<u32> = Vec::new();
    while ended.len() < 10 {
        let Some((h, pl)) = c.frame(Duration::from_secs(15)) else { break };
        match h.ty {
            FrameType::Settings if h.flags & 1 == 0 => c.send(FrameType::Settings, 1, 0, &[]),
            FrameType::Data => {
                bodies.entry(h.stream).or_default().extend_from_slice(&pl);
                if h.flags & 1 != 0 {
                    ended.push(h.stream);
                }
            }
            FrameType::Headers if h.flags & 1 != 0 => ended.push(h.stream),
            FrameType::Headers => {
                let _ = c.dec.decode(&pl);
            }
            _ => {}
        }
    }
    want.sort();
    ended.sort();
    assert_eq!(ended, want, "every stream ended once");
    for i in 0..10u32 {
        assert_eq!(String::from_utf8_lossy(&bodies[&(1 + 2 * i)]), format!("name=a path=/s{i}"));
    }
    // HEAD: the head only, no DATA, and the stream ends with the HEADERS frame
    c.headers(21, "HEAD", "/h", A, &[], true);
    let r = c.answer(21, true);
    assert!(r.status == 200 && r.ended && r.body.is_empty(), "{r:?}");
}

#[test]
fn a_request_body_in_several_frames_and_a_header_block_in_continuation_frames_arrive_whole() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let mut c = h2(p, &[]);
    c.headers(1, "POST", "/echo", A, &[], false);
    c.send(FrameType::Data, 0, 1, &[1u8; 10_000]);
    c.send(FrameType::Data, 0, 1, &[2u8; 10_000]);
    c.send(FrameType::Data, 1, 1, &[3u8; 5_000]);
    let r = c.answer(1, true);
    assert_eq!(String::from_utf8_lossy(&r.body), format!("len=25000 sum={}", 10_000 + 20_000 + 15_000));
    // a header block split into HEADERS (no END_HEADERS) and two CONTINUATION frames, with a long cookie
    let mut list = vec![(":method".to_string(), "GET".to_string()), (":scheme".into(), "http".into()), (":path".into(), "/split".into()), (":authority".into(), A.into()), ("x-note".into(), "n".repeat(3000)), ("cookie".into(), "a=1".into()), ("cookie".into(), "b=2".into())];
    list.push(("x-more".into(), "m".repeat(3000)));
    let block = c.enc.encode(&list);
    let (x, rest) = block.split_at(2000);
    let (y, z) = rest.split_at(2000);
    c.send(FrameType::Headers, 0x1, 3, x);
    c.send(FrameType::Continuation, 0, 3, y);
    c.send(FrameType::Continuation, 0x4, 3, z);
    let r = c.answer(3, true);
    assert_eq!(String::from_utf8_lossy(&r.body), "name=a path=/split");
    let head = saw(&a).to_lowercase();
    assert!(head.contains("cookie: a=1; b=2\r\n"), "two cookie fields are joined as HTTP/1.1 wants: {head}");
}

#[test]
fn a_cancelled_stream_stops_the_work_and_the_others_are_not_disturbed() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let mut c = h2(p, &[]);
    c.get(1, A, "/stream");
    c.get(3, A, "/quick");
    c.send(FrameType::RstStream, 0, 1, &ErrorCode::Cancel.to_u32().to_be_bytes());
    let r = c.answer(3, true);
    assert_eq!(String::from_utf8_lossy(&r.body), "name=a path=/quick");
    // the connection still works afterwards
    c.get(5, A, "/after");
    assert_eq!(String::from_utf8_lossy(&c.answer(5, true).body), "name=a path=/after");
}

#[test]
fn a_response_with_trailers_loses_them_and_a_target_that_closes_early_is_a_502() {
    let a = upstream("a");
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in dead.incoming() {
            drop(c); // accept and close without answering
        }
    });
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p), route(B, "/", dead_port, p)]);
    let mut c = h2(p, &[]);
    c.get(1, A, "/trailers");
    let r = c.answer(1, true);
    assert_eq!((r.status, String::from_utf8_lossy(&r.body).into_owned()), (200, "data".into()));
    assert!(hdr(&r, "trailer").is_none() && hdr(&r, "transfer-encoding").is_none() && hdr(&r, "connection").is_none(), "{:?}", r.headers);
    c.get(3, B, "/");
    let r = c.answer(3, true);
    assert!(r.status == 502 || r.status == 504, "{r:?}");
}

#[test]
fn bad_requests_are_refused_as_stream_errors_and_the_connection_goes_on() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let mut c = h2(p, &[]);
    // a header name with capital letters
    let mut block = c.enc.encode(&[(":method".to_string(), "GET".to_string()), (":scheme".into(), "http".into()), (":path".into(), "/".into()), (":authority".into(), A.into())]);
    block.extend_from_slice(&[0x00, 7]);
    block.extend_from_slice(b"X-Upper");
    block.extend_from_slice(&[1, b'1']);
    c.send(FrameType::Headers, 0x5, 1, &block);
    let r = c.answer(1, true);
    assert_eq!(r.reset, Some(ErrorCode::ProtocolError), "{r:?}");
    // a connection-specific header
    c.headers(3, "GET", "/", A, &[("connection", "close")], true);
    assert_eq!(c.answer(3, true).reset, Some(ErrorCode::ProtocolError));
    // TE other than trailers
    c.headers(5, "GET", "/", A, &[("te", "gzip")], true);
    assert_eq!(c.answer(5, true).reset, Some(ErrorCode::ProtocolError));
    // CONNECT is not supported
    c.headers(7, "CONNECT", "/", A, &[], true);
    assert_eq!(c.answer(7, true).reset, Some(ErrorCode::ProtocolError));
    // a path with .. is the same 400 as on HTTP/1.1
    c.get(9, A, "/a/../b");
    assert_eq!(c.answer(9, true).status, 400);
    // and a good one still works
    c.get(11, A, "/fine");
    assert_eq!(String::from_utf8_lossy(&c.answer(11, true).body), "name=a path=/fine");
}

#[test]
fn protocol_violations_end_the_connection_with_goaway_and_the_right_code() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    // an even stream number from a client
    let mut c = h2(p, &[]);
    c.get(2, A, "/");
    assert_eq!(c.answer(2, true).reset, Some(ErrorCode::ProtocolError));
    // a stream number that goes backwards
    let mut c = h2(p, &[]);
    c.get(5, A, "/x");
    let _ = c.answer(5, true);
    c.get(3, A, "/y");
    assert_eq!(c.answer(3, true).reset, Some(ErrorCode::ProtocolError));
    // a frame that is too large for the default frame size
    let mut c = h2(p, &[]);
    c.send(FrameType::Data, 0, 1, &vec![0u8; 20_000]);
    assert_eq!(c.answer(1, true).reset, Some(ErrorCode::FrameSizeError));
    // a push promise from a client
    let mut c = h2(p, &[]);
    c.send(FrameType::PushPromise, 0x4, 1, &[0, 0, 0, 2]);
    assert_eq!(c.answer(1, true).reset, Some(ErrorCode::ProtocolError));
    // a window update that overflows the connection window
    let mut c = h2(p, &[]);
    c.send(FrameType::WindowUpdate, 0, 0, &(0x7fff_ffffu32).to_be_bytes());
    assert_eq!(c.answer(1, true).reset, Some(ErrorCode::FlowControlError));
    // a wrong preface closes the connection without an answer of HTTP/2
    let mut s = connect(p);
    s.write_all(b"PRI * HTTP/2.0\r\n\r\nSX\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.is_empty() || buf.starts_with(b"HTTP/1.1 4") || buf.starts_with(b"HTTP/1.1 5"), "no HTTP/2 frames: {:?}", String::from_utf8_lossy(&buf));
}

#[test]
fn a_flood_of_resets_and_too_many_streams_are_stopped() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    // more than 32 streams open at once: the extra ones are refused, the first ones are answered
    let mut c = h2(p, &[]);
    for i in 0..40u32 {
        c.get(1 + 2 * i, A, "/stream");
    }
    let mut refused = 0;
    let mut ended = 0;
    while let Some((h, pl)) = c.frame(Duration::from_secs(5)) {
        match h.ty {
            FrameType::RstStream if ErrorCode::from_u32(u32::from_be_bytes([pl[0], pl[1], pl[2], pl[3]])) == ErrorCode::RefusedStream => refused += 1,
            FrameType::Data if h.flags & 1 != 0 => ended += 1,
            FrameType::Settings if h.flags & 1 == 0 => c.send(FrameType::Settings, 1, 0, &[]),
            _ => {}
        }
        if refused + ended >= 40 {
            break;
        }
    }
    assert_eq!(refused, 8, "the 33rd to the 40th are refused");
    assert_eq!(ended, 32, "the first 32 are answered");
    // rapid reset: open and cancel again and again
    let mut c = h2(p, &[]);
    let mut calm = false;
    for i in 0..600u32 {
        c.get(1 + 2 * i, A, "/quick");
        c.send(FrameType::RstStream, 0, 1 + 2 * i, &ErrorCode::Cancel.to_u32().to_be_bytes());
        if i % 50 == 49 {
            while let Some((h, pl)) = c.frame(Duration::from_millis(5)) {
                if h.ty == FrameType::GoAway && ErrorCode::from_u32(u32::from_be_bytes([pl[4], pl[5], pl[6], pl[7]])) == ErrorCode::EnhanceYourCalm {
                    calm = true;
                }
            }
        }
        if calm {
            break;
        }
    }
    assert!(calm, "the flood ends in GOAWAY ENHANCE_YOUR_CALM");
}

#[test]
fn ping_is_answered_idle_connections_end_and_http1_still_works_on_the_same_port() {
    let a = upstream("a");
    let p = free_port();
    let (mut o, _) = opts();
    o.idle_timeout = Duration::from_millis(600);
    let _l = Listener::start(table(vec![route(A, "/", a.port, p)]), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap();
    let mut c = h2(p, &[]);
    c.send(FrameType::Ping, 0, 0, b"12345678");
    let mut pong = false;
    while let Some((h, pl)) = c.frame(Duration::from_millis(500)) {
        if h.ty == FrameType::Ping && h.flags & 1 != 0 && pl == b"12345678" {
            pong = true;
            break;
        }
    }
    assert!(pong);
    // idle: the server says goodbye
    let mut bye = false;
    while let Some((h, _)) = c.frame(Duration::from_secs(3)) {
        if h.ty == FrameType::GoAway {
            bye = true;
            break;
        }
    }
    assert!(bye, "an idle connection ends with GOAWAY");
    // HTTP/1.1 on the same port is untouched
    assert_eq!(text(&http(p, &get(A, "/h1"))), "name=a path=/h1");
}

#[test]
fn with_http2_off_the_preface_is_not_taken_for_http2() {
    let a = upstream("a");
    let p = free_port();
    let (mut o, _) = opts();
    o.http2 = false;
    let _l = Listener::start(table(vec![route(A, "/", a.port, p)]), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap();
    let mut s = connect(p);
    s.write_all(f::PREFACE).unwrap();
    s.write_all(&f::encode_frame(FrameType::Settings, 0, 0, &[]).unwrap()).unwrap();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.starts_with(b"HTTP/1.1 4") || buf.starts_with(b"HTTP/1.1 5") || buf.is_empty(), "an HTTP/1.1 error, not HTTP/2 frames: {:?}", String::from_utf8_lossy(&buf));
    assert_eq!(text(&http(p, &get(A, "/h1"))), "name=a path=/h1");
}

#[test]
fn the_members_gate_and_tenants_of_http1_apply_to_http2_requests_too() {
    // a route that only members may open is answered by the same gate: here there is no gate set, so the route is served and logged as not enforced
    let a = upstream("a");
    let p = free_port();
    let mut members = route(B, "/", a.port, p);
    members.access = Access::Members;
    let (_l, log) = start(vec![route(A, "/", a.port, p), members]);
    let mut c = h2(p, &[]);
    c.get(1, B, "/m");
    assert_eq!(c.answer(1, true).status, 200);
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("access members (not enforced by this listener)"), "the access line of HTTP/1.1 is written for an HTTP/2 request: {log}");
}

// ------------------------------------------------------------------------------------------------------------ the real openssl

fn have(tool: &str) -> bool {
    std::process::Command::new(tool).arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `openssl s_client` against the listener (an independent TLS implementation): everything it printed.
fn s_client(port: u16, sni: &str, extra: &[&str]) -> String {
    let o = std::process::Command::new("openssl")
        .args(["s_client", "-connect", &format!("127.0.0.1:{port}"), "-servername", sni, "-showcerts"])
        .args(extra)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("openssl runs");
    format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// The first certificate in `text`, as one line without the markers and the line breaks.
fn first_cert(text: &str) -> Option<String> {
    let a = text.find("-----BEGIN CERTIFICATE-----")? + "-----BEGIN CERTIFICATE-----".len();
    let b = a + text[a..].find("-----END CERTIFICATE-----")?;
    Some(text[a..b].split_whitespace().collect())
}

#[test]
fn openssl_is_given_the_certificate_of_the_name_it_asks_for_negotiates_the_protocol_it_offers_and_gets_nothing_for_a_stranger() {
    if !have("openssl") {
        eprintln!("openssl is not installed here: this test did not run");
        return;
    }
    let a = upstream("a");
    let dir = tmp("h2openssl");
    let (chain_a, key_a) = self_signed(A).unwrap();
    let (chain_b, key_b) = self_signed(B).unwrap();
    write_cert(&dir, "acert", &chain_a, &key_a);
    write_cert(&dir, "bcert", &chain_b, &key_b);
    let p = free_port();
    let (o, _) = opts();
    let _l = Listener::start(table(vec![https_route(A, a.port, p, "acert"), https_route(B, a.port, p, "bcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    // the certificate the route names, chosen by the server name the client sends
    let (want_a, want_b) = (first_cert(&chain_a).unwrap(), first_cert(&chain_b).unwrap());
    assert_ne!(want_a, want_b);
    assert_eq!(first_cert(&s_client(p, A, &[])).as_deref(), Some(want_a.as_str()), "the certificate of the first name");
    assert_eq!(first_cert(&s_client(p, B, &[])).as_deref(), Some(want_b.as_str()), "the certificate of the second name on the same port");
    // the protocol the client offers is the one it gets
    let h2 = s_client(p, A, &["-alpn", "h2,http/1.1"]);
    assert!(h2.contains("ALPN protocol: h2") || h2.contains("Selected ALPN protocol: h2"), "{h2}");
    let h1 = s_client(p, A, &["-alpn", "http/1.1"]);
    assert!(h1.contains("http/1.1") && !h1.contains("ALPN protocol: h2"), "{h1}");
    // both TLS versions are served
    assert!(s_client(p, A, &["-tls1_2"]).contains("TLSv1.2"), "TLS 1.2");
    assert!(s_client(p, A, &["-tls1_3"]).contains("TLSv1.3"), "TLS 1.3");
    // an older version is refused
    let old = s_client(p, A, &["-tls1_1"]);
    assert!(first_cert(&old).is_none(), "no certificate for TLS 1.1: {old}");
    // a name no route has gets no certificate
    let stranger = s_client(p, "nobody.example.org", &[]);
    assert!(first_cert(&stranger).is_none(), "a stranger is given no certificate: {stranger}");
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------------------------------------------------ a real browser

fn find_chrome() -> Option<String> {
    ["google-chrome", "chromium", "chromium-browser", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
        .iter()
        .find(|c| std::process::Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
        .map(|c| c.to_string())
}

/// An upstream that serves one page whose script asks for `N` things at once, and answers each of them.
fn page_upstream(n: usize) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            std::thread::spawn(move || {
                timeout(&c);
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut w = c;
                let mut first = String::new();
                if r.read_line(&mut first).unwrap_or(0) == 0 {
                    return;
                }
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                        break;
                    }
                }
                let path = first.split(' ').nth(1).unwrap_or("/").to_string();
                let (ctype, body) = if path == "/" {
                    ("text/html", format!("<!doctype html><html><body>waiting<script>Promise.all(Array.from({{length:{n}}},(_,i)=>fetch('/p/'+i).then(r=>r.text()))).then(a=>{{document.body.textContent='GOT:'+a.join('|')}}).catch(e=>{{document.body.textContent='ERR:'+e}})</script></body></html>"))
                } else {
                    ("text/plain", format!("ok{path}"))
                };
                let _ = w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            });
        }
    });
    port
}

#[test]
fn a_real_chrome_loads_a_page_and_its_parallel_requests_over_http2_from_the_listener() {
    let Some(chrome) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let up = page_upstream(12);
    let dir = tmp("h2chrome");
    let (chain, key) = self_signed(A).unwrap();
    write_cert(&dir, "appcert", &chain, &key);
    let p = free_port();
    let (o, log) = opts();
    let _l = Listener::start(table(vec![https_route(A, up, p, "appcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let mut cmd = std::process::Command::new(&chrome);
    cmd.args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--ignore-certificate-errors", "--disable-extensions", "--virtual-time-budget=8000"])
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("--host-resolver-rules=MAP {A} 127.0.0.1"))
        .arg("--dump-dom")
        .arg(format!("https://{A}:{p}/"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // a browser that hangs must not hang the gate: it is stopped after a minute
    let mut child = cmd.spawn().expect("chrome starts");
    let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let (t_out, t_err) = (std::thread::spawn(move || { let mut b = Vec::new(); let _ = so.read_to_end(&mut b); b }), std::thread::spawn(move || { let mut b = Vec::new(); let _ = se.read_to_end(&mut b); b }));
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = std::process::Output { status: child.wait().unwrap(), stdout: t_out.join().unwrap(), stderr: t_err.join().unwrap() };
    let dom = String::from_utf8_lossy(&out.stdout).into_owned();
    let want: Vec<String> = (0..12).map(|i| format!("ok/p/{i}")).collect();
    assert!(dom.contains(&format!("GOT:{}", want.join("|"))), "the page did not get its twelve answers in order: {dom}\n{}", String::from_utf8_lossy(&out.stderr).lines().rev().take(5).collect::<Vec<_>>().join("\n"));
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("HTTP/2 connection on port") && log.contains("ALPN h2"), "Chrome spoke HTTP/2 to the listener: {log}");
    let _ = std::fs::remove_dir_all(dir);
}
