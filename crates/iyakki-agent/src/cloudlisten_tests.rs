//! Tests of the domain listener. Loopback sockets only: no network, no name resolution, no privileged ports.

use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cloudroute::{self_signed, Access, CertRef, Route, RouteTable, Scheme, Target};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};

type Resp = (u16, Vec<(String, String)>, Vec<u8>);

// ------------------------------------------------------------------------------------------------------------ helpers

fn free_port() -> u16 {
    // below the range the system hands out for bind(0) and outgoing connections, so no other test can take it between two starts
    crate::testdir::free_port()
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-listen-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

type Log = Arc<Mutex<Vec<String>>>;

fn opts() -> (Options, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let o = Options { bind: "127.0.0.1".parse().unwrap(), redirect_port: free_port(), log: Arc::new(move |s| l2.lock().unwrap().push(s.to_string())), ..Options::default() };
    (o, log)
}

fn route(host: &str, prefix: &str, target: u16, listen: u16) -> Route {
    Route { host: host.into(), path_prefix: prefix.into(), target: Target::Local { port: target }, scheme: Scheme::Http, listen: Some(listen), websocket: true, redirect_http: false, cert: None, access: Access::Public, client_ca: None }
}

fn https_route(host: &str, target: u16, listen: u16, cert: &str) -> Route {
    Route { scheme: Scheme::Https, cert: Some(CertRef::Vault(cert.into())), ..route(host, "/", target, listen) }
}

fn table(routes: Vec<Route>) -> RouteTable {
    RouteTable { routes, ingress_class: None }
}

fn start(routes: Vec<Route>) -> (Listener, Log) {
    let (o, log) = opts();
    (Listener::start(table(routes), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap(), log)
}

fn timeout(s: &TcpStream) {
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    timeout(&s);
    s
}

/// The upstream test server: answers one request per connection (the listener sends `Connection: close`) and keeps every head it saw.
struct Up {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

fn upstream(name: &'static str) -> Up {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(c) = c else { continue };
            let s = s2.clone();
            std::thread::spawn(move || up_conn(name, c, s));
        }
    });
    Up { port, seen }
}

fn up_conn(name: &str, c: TcpStream, seen: Arc<Mutex<Vec<String>>>) {
    timeout(&c);
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut w = c;
    let mut head = String::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 {
            return;
        }
        head.push_str(&l);
        if l == "\r\n" {
            break;
        }
    }
    seen.lock().unwrap().push(head.clone());
    let path = head.lines().next().unwrap().split(' ').nth(1).unwrap().to_string();
    let lower = head.to_lowercase();
    let mut body = Vec::new();
    if lower.contains("transfer-encoding: chunked") {
        loop {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            let n = usize::from_str_radix(l.trim(), 16).unwrap();
            let mut buf = vec![0u8; n + 2];
            r.read_exact(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
    } else if let Some(l) = lower.lines().find_map(|l| l.strip_prefix("content-length: ")) {
        body = vec![0u8; l.trim().parse().unwrap()];
        r.read_exact(&mut body).unwrap();
    }
    let reply = |w: &mut TcpStream, b: &[u8]| {
        w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).as_bytes()).unwrap();
        w.write_all(b).unwrap();
    };
    match path.as_str() {
        "/echo" => {
            let sum: u64 = body.iter().map(|b| *b as u64).sum();
            reply(&mut w, format!("len={} sum={sum}", body.len()).as_bytes());
        }
        "/chunked" => {
            w.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nalpha\r\n4\r\nbeta\r\n5\r\ngamma\r\n0\r\n\r\n").unwrap();
        }
        "/big" => {
            let b: Vec<u8> = (0..3_000_000usize).map(|i| (i % 251) as u8).collect();
            reply(&mut w, &b);
        }
        "/stream" => {
            w.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\none\r\n").unwrap();
            w.flush().unwrap();
            std::thread::sleep(Duration::from_millis(800));
            w.write_all(b"3\r\ntwo\r\n0\r\n\r\n").unwrap();
        }
        "/ws" if lower.contains("upgrade: websocket") => {
            w.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nwelcome").unwrap();
            let mut buf = [0u8; 4096];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || w.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
        _ => reply(&mut w, format!("name={name} path={path}").as_bytes()),
    }
}

fn read_response<R: BufRead>(r: &mut R) -> Resp {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let code: u16 = line.split(' ').nth(1).unwrap_or_else(|| panic!("no status line: {line:?}")).parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" {
            break;
        }
        let (n, v) = l.trim_end().split_once(':').unwrap();
        headers.push((n.to_ascii_lowercase(), v.trim().to_string()));
    }
    let h = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if h("transfer-encoding").as_deref() == Some("chunked") {
        loop {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            let n = usize::from_str_radix(l.trim(), 16).unwrap();
            let mut buf = vec![0u8; n + 2];
            r.read_exact(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
    } else if let Some(n) = h("content-length") {
        body = vec![0u8; n.parse().unwrap()];
        r.read_exact(&mut body).unwrap();
    }
    (code, headers, body)
}

fn get(host: &str, path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")
}

/// One request on its own connection.
fn http(port: u16, req: &str) -> Resp {
    let mut s = connect(port);
    s.write_all(req.as_bytes()).unwrap();
    read_response(&mut BufReader::new(s))
}

fn text(r: &Resp) -> String {
    String::from_utf8_lossy(&r.2).to_string()
}

fn header<'a>(r: &'a Resp, n: &str) -> Option<&'a str> {
    r.1.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
}

fn write_cert(dir: &std::path::Path, name: &str, chain: &str, key: &str) {
    std::fs::write(dir.join(format!("{name}.crt")), chain).unwrap();
    std::fs::write(dir.join(format!("{name}.key")), key).unwrap();
}

fn der(pem: &str) -> Vec<u8> {
    CertificateDer::from_pem_slice(pem.as_bytes()).unwrap().as_ref().to_vec()
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn trusting(pems: &[&str], versions: &[&'static rustls::SupportedProtocolVersion]) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for p in pems {
        roots.add(CertificateDer::from_pem_slice(p.as_bytes()).unwrap()).unwrap();
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider()).with_protocol_versions(versions).unwrap().with_root_certificates(roots).with_no_client_auth();
    cfg.resumption = rustls::client::Resumption::disabled(); // a resumed session would show the old certificate
    Arc::new(cfg)
}

type TlsReader = BufReader<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

/// Connect, finish the handshake, and return the stream with the certificate the server presented.
fn tls_connect(port: u16, sni: &str, cfg: &Arc<rustls::ClientConfig>) -> Result<(TlsReader, Vec<u8>, rustls::ProtocolVersion), String> {
    let mut tcp = connect(port);
    let mut conn = rustls::ClientConnection::new(cfg.clone(), ServerName::try_from(sni.to_string()).unwrap()).unwrap();
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(|e| e.to_string())?;
    }
    let cert = conn.peer_certificates().unwrap()[0].as_ref().to_vec();
    let v = conn.protocol_version().unwrap();
    Ok((BufReader::new(rustls::StreamOwned::new(conn, tcp)), cert, v))
}

fn tls_get(port: u16, sni: &str, cfg: &Arc<rustls::ClientConfig>, host: &str, path: &str) -> Result<(Resp, Vec<u8>), String> {
    let (mut s, cert, _) = tls_connect(port, sni, cfg)?;
    s.get_mut().write_all(get(host, path).as_bytes()).map_err(|e| e.to_string())?;
    Ok((read_response(&mut s), cert))
}

fn saw(u: &Up) -> String {
    u.seen.lock().unwrap().join("---\n")
}

const A: &str = "app.example.org";
const B: &str = "shop.example.org";

// ------------------------------------------------------------------------------------------------------------ routing

#[test]
fn requests_are_routed_by_host_and_path_to_two_upstreams_with_forwarding_headers() {
    let (a, b) = (upstream("a"), upstream("b"));
    let p = free_port();
    let mut members = route("other.example.org", "/", b.port, p);
    members.access = Access::Members;
    let (l, log) = start(vec![route(A, "/", a.port, p), route(A, "/api", b.port, p), members]);
    assert_eq!(text(&http(p, &get(A, "/"))), "name=a path=/");
    assert_eq!(text(&http(p, &get(A, "/api/v1/x?q=1"))), "name=b path=/api/v1/x?q=1", "the longest path prefix wins and the target is passed as sent");
    assert_eq!(text(&http(p, &get("OTHER.example.org", "/z"))), "name=b path=/z");
    assert_eq!(text(&http(p, &get(&format!("{A}:{p}"), "/p"))), "name=a path=/p", "a port in the Host header is ignored for matching");
    // headers: hop-by-hop dropped, forwarding headers set by the listener
    let req = format!("GET /h HTTP/1.1\r\nHost: {A}\r\nX-Forwarded-For: 6.6.6.6\r\nForwarded: for=6.6.6.6\r\nConnection: keep-alive, X-Secret\r\nX-Secret: s\r\nKeep-Alive: 5\r\nX-Custom: kept\r\n\r\n");
    assert_eq!(http(p, &req).0, 200);
    let head = saw(&a).to_lowercase();
    assert!(head.contains("x-forwarded-for: 127.0.0.1\r\n") && !head.contains("6.6.6.6"), "{head}");
    assert!(head.contains("x-forwarded-proto: http\r\n") && head.contains(&format!("x-forwarded-host: {A}\r\n")), "{head}");
    assert!(head.contains("x-custom: kept") && !head.contains("x-secret") && !head.contains("keep-alive") && head.contains("connection: close"), "{head}");
    // the log shows the access of the route; members is not pretended to be enforced
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("access public"), "{log}");
    assert!(log.contains("access members (not enforced by this listener)"), "{log}");
    drop(l);
}

#[test]
fn a_keep_alive_connection_routes_each_request_on_its_own() {
    let (a, b) = (upstream("a"), upstream("b"));
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p), route(A, "/api", b.port, p), route(B, "/", b.port, p)]);
    let mut s = connect(p);
    let mut r = BufReader::new(s.try_clone().unwrap());
    for (host, path, want) in [(A, "/one", "name=a path=/one"), (A, "/api/two", "name=b path=/api/two"), (B, "/three", "name=b path=/three"), (A, "/four", "name=a path=/four")] {
        s.write_all(get(host, path).as_bytes()).unwrap();
        let resp = read_response(&mut r);
        assert_eq!(text(&resp), want);
        assert_ne!(header(&resp, "connection"), Some("close"), "the connection is kept");
    }
    // a 404 on the same connection keeps it too
    s.write_all(get("nobody.example.org", "/").as_bytes()).unwrap();
    assert_eq!(read_response(&mut r).0, 404);
    s.write_all(get(A, "/five").as_bytes()).unwrap();
    assert_eq!(text(&read_response(&mut r)), "name=a path=/five");
}

#[test]
fn bodies_stream_through_in_both_directions_with_length_and_chunked_framing() {
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    // a large request body with Content-Length
    let body: Vec<u8> = (0..5_000_000usize).map(|i| (i % 253) as u8).collect();
    let sum: u64 = body.iter().map(|b| *b as u64).sum();
    let mut s = connect(p);
    s.write_all(format!("POST /echo HTTP/1.1\r\nHost: {A}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).unwrap();
    s.write_all(&body).unwrap();
    assert_eq!(text(&read_response(&mut BufReader::new(s))), format!("len=5000000 sum={sum}"));
    // a chunked request body (with a chunk extension, which is dropped)
    let mut s = connect(p);
    s.write_all(format!("POST /echo HTTP/1.1\r\nHost: {A}\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\n\r\n").as_bytes()).unwrap();
    let sum: u64 = b"hello world".iter().map(|b| *b as u64).sum();
    assert_eq!(text(&read_response(&mut BufReader::new(s))), format!("len=11 sum={sum}"));
    // a chunked and a large response
    assert_eq!(text(&http(p, &get(A, "/chunked"))), "alphabetagamma");
    let big = http(p, &get(A, "/big"));
    assert_eq!(big.2.len(), 3_000_000);
    assert!(big.2.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    // streaming: the first chunk reaches the client while the target still holds back the second (it waits 800 ms)
    let mut s = connect(p);
    let t0 = Instant::now();
    s.write_all(get(A, "/stream").as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut l = String::new();
    while l != "\r\n" {
        l.clear();
        r.read_line(&mut l).unwrap();
    }
    let mut size = String::new();
    r.read_line(&mut size).unwrap();
    let mut data = [0u8; 5];
    r.read_exact(&mut data).unwrap();
    assert_eq!((size.trim(), &data[..3]), ("3", &b"one"[..]));
    assert!(t0.elapsed() < Duration::from_millis(600), "the first chunk waited for the whole response: {:?}", t0.elapsed());
}

#[test]
fn an_upgrade_becomes_a_two_way_byte_tunnel_and_a_route_can_refuse_it() {
    let a = upstream("a");
    let b = upstream("b");
    let p = free_port();
    let mut nows = route("nows.example.org", "/", b.port, p);
    nows.websocket = false;
    let (_l, _) = start(vec![route(A, "/", a.port, p), nows]);
    let mut s = connect(p);
    s.write_all(format!("GET /ws HTTP/1.1\r\nHost: {A}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: x\r\n\r\n").as_bytes()).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut first = String::new();
    r.read_line(&mut first).unwrap();
    assert!(first.starts_with("HTTP/1.1 101"), "{first}");
    let mut l = String::new();
    while l != "\r\n" {
        l.clear();
        r.read_line(&mut l).unwrap();
    }
    let mut got = [0u8; 7];
    r.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"welcome", "bytes the target sent right after the 101 reach the client");
    s.write_all(b"ping").unwrap();
    let mut got = [0u8; 4];
    r.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"ping");
    let big: Vec<u8> = (0..200_000usize).map(|i| (i % 249) as u8).collect();
    let mut w = s.try_clone().unwrap();
    let big2 = big.clone();
    let t = std::thread::spawn(move || w.write_all(&big2).unwrap());
    let mut back = vec![0u8; big.len()];
    r.read_exact(&mut back).unwrap();
    t.join().unwrap();
    assert_eq!(back, big, "200 KB went out and came back unchanged");
    // websocket=false: refused, the target is not contacted
    let resp = http(p, &format!("GET /ws HTTP/1.1\r\nHost: nows.example.org\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n"));
    assert_eq!(resp.0, 403);
    assert!(b.seen.lock().unwrap().is_empty());
    // a plain request to that route still works
    assert_eq!(http(p, &get("nows.example.org", "/")).0, 200);
}

#[test]
fn unknown_unreachable_malformed_and_oversized_requests_get_clear_answers() {
    let a = upstream("a");
    let dead = free_port();
    let (p, q) = (free_port(), free_port());
    let mut svc = route("svc.example.org", "/", 1, p);
    svc.target = Target::Service { name: "web".into(), port: 80 };
    let (_l, log) = start(vec![route(A, "/", a.port, p), route("down.example.org", "/", dead, p), svc, route("slow.example.org", "/", a.port, q)]);
    assert_eq!(http(p, &get("nobody.example.org", "/")).0, 404);
    let down = http(p, &get("down.example.org", "/"));
    assert_eq!(down.0, 502);
    assert!(text(&down).contains(&format!("127.0.0.1:{dead}")), "{}", text(&down));
    let svc = http(p, &get("svc.example.org", "/"));
    assert_eq!((svc.0, text(&svc).contains("Kubernetes Service")), (502, true));
    assert!(log.lock().unwrap().iter().any(|l| l.contains("svc.example.org") && l.contains("NOT reachable from a local listener")));
    assert_eq!(http(p, "GET / HTTP/1.0\r\n\r\n").0, 505);
    assert_eq!(http(p, "GET / HTTP/1.1\r\n\r\n").0, 400, "no Host");
    assert_eq!(http(p, &format!("GET /a/../b HTTP/1.1\r\nHost: {A}\r\n\r\n")).0, 400, "a path with .. is refused");
    assert_eq!(http(p, &format!("POST / HTTP/1.1\r\nHost: {A}\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n")).0, 400, "both framings");
    assert_eq!(http(p, &format!("CONNECT x:1 HTTP/1.1\r\nHost: {A}\r\n\r\n")).0, 405);
    // the head limit is 16 KB
    let big = http(p, &format!("GET / HTTP/1.1\r\nHost: {A}\r\nX-Big: {}\r\n\r\n", "a".repeat(17 * 1024)));
    assert_eq!(big.0, 431);
    assert!(a.seen.lock().unwrap().iter().all(|h| !h.contains("X-Big")), "the oversized head never reached the target");
    let ok = http(p, &format!("GET / HTTP/1.1\r\nHost: {A}\r\nX-Big: {}\r\n\r\n", "a".repeat(15 * 1024)));
    assert_eq!(ok.0, 200, "a head under the limit passes");
}

#[test]
fn a_client_that_is_too_slow_is_answered_408_and_the_connection_count_is_capped() {
    let a = upstream("a");
    let (p, q) = (free_port(), free_port());
    let (mut o, _) = opts();
    o.head_timeout = Duration::from_millis(300);
    o.max_conns = 1;
    let l = Listener::start(table(vec![route(A, "/", a.port, p), route(B, "/", a.port, q)]), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap();
    // slow head
    let mut s = connect(p);
    s.write_all(b"GET / HT").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 408"), "{out:?}");
    // cap: one idle connection holds the only slot, a second is closed without an answer
    let hold = connect(q);
    std::thread::sleep(Duration::from_millis(100));
    let mut second = connect(q);
    let _ = second.write_all(get(B, "/").as_bytes());
    let mut buf = [0u8; 64];
    let n = second.read(&mut buf).unwrap_or(0);
    assert_eq!(n, 0, "over the cap: closed without an answer");
    drop(hold);
    let t0 = Instant::now();
    loop {
        let mut s = connect(q);
        let _ = s.write_all(get(B, "/").as_bytes());
        let mut r = BufReader::new(s);
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) > 0 {
            assert!(line.contains("200"), "{line}");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "the slot was never freed");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(l);
}

// ------------------------------------------------------------------------------------------------------------ https

#[test]
fn https_ends_tls_with_the_route_certificate_and_tunnels_upgrades_too() {
    let a = upstream("a");
    let dir = tmp("https");
    let (chain, key) = self_signed(A).unwrap();
    write_cert(&dir, "appcert", &chain, &key);
    let p = free_port();
    let (o, _) = opts();
    let mut only = https_route(A, a.port, p, "appcert");
    only.path_prefix = "/only".into();
    let l = Listener::start(table(vec![only, https_route("two.example.org", a.port, p, "appcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    // "two.example.org" is not covered by the certificate: refused, but the listener serves the other host
    assert!(l.refused().iter().any(|(h, why)| h == "two.example.org" && why.contains("covers")), "{:?}", l.refused());
    let cfg = trusting(&[&chain], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    let (resp, cert) = tls_get(p, A, &cfg, A, "/only/x").unwrap();
    assert_eq!(text(&resp), "name=a path=/only/x");
    assert_eq!(cert, der(&chain));
    assert!(saw(&a).to_lowercase().contains("x-forwarded-proto: https"));
    // TLS 1.2 is accepted
    let (mut s12, _, v) = tls_connect(p, A, &trusting(&[&chain], &[&rustls::version::TLS12])).unwrap();
    assert_eq!(v, rustls::ProtocolVersion::TLSv1_2);
    s12.get_mut().write_all(get(A, "/only").as_bytes()).unwrap();
    assert_eq!(read_response(&mut s12).0, 200);
    // 421: a Host no route matches, a path no route has, and a Host that is not the name of the connection
    assert_eq!(tls_get(p, A, &cfg, A, "/elsewhere").unwrap().0 .0, 421);
    assert_eq!(tls_get(p, A, &cfg, "two.example.org", "/only").unwrap().0 .0, 421);
    assert_eq!(tls_get(p, A, &cfg, "other.example.org", "/only").unwrap().0 .0, 421);
    // an Upgrade through TLS
    let (mut s, _, _) = tls_connect(p, A, &cfg).unwrap();
    s.get_mut().write_all(format!("GET /only HTTP/1.1\r\nHost: {A}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).unwrap();
    // the target answers /only with a normal 200 (it upgrades /ws only): the tunnel path is tried on /ws below
    assert_eq!(read_response(&mut s).0, 200);
    drop(l);
    let l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), opts().0).unwrap();
    let (mut s, _, _) = tls_connect(p, A, &cfg).unwrap();
    s.get_mut().write_all(format!("GET /ws HTTP/1.1\r\nHost: {A}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).unwrap();
    let mut first = String::new();
    s.read_line(&mut first).unwrap();
    assert!(first.starts_with("HTTP/1.1 101"), "{first}");
    let mut line = String::new();
    while line != "\r\n" {
        line.clear();
        s.read_line(&mut line).unwrap();
    }
    let mut got = [0u8; 7];
    s.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"welcome");
    s.get_mut().write_all(b"over tls").unwrap();
    let mut got = [0u8; 8];
    s.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"over tls");
    drop(l);
}

#[test]
fn sni_chooses_between_two_certificates_on_one_port() {
    let (a, b) = (upstream("a"), upstream("b"));
    let dir = tmp("sni");
    let (ca, ka) = self_signed(A).unwrap();
    let (cb, kb) = self_signed(B).unwrap();
    write_cert(&dir, "a", &ca, &ka);
    write_cert(&dir, "b", &cb, &kb);
    let p = free_port();
    let (o, _) = opts();
    let _l = Listener::start(table(vec![https_route(A, a.port, p, "a"), https_route(B, b.port, p, "b")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let cfg = trusting(&[&ca, &cb], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    let (r1, c1) = tls_get(p, A, &cfg, A, "/").unwrap();
    let (r2, c2) = tls_get(p, B, &cfg, B, "/").unwrap();
    assert_eq!((text(&r1), c1), ("name=a path=/".to_string(), der(&ca)));
    assert_eq!((text(&r2), c2), ("name=b path=/".to_string(), der(&cb)));
    assert!(tls_connect(p, "nope.example.org", &cfg).is_err(), "a name without a certificate gets no handshake");
}

#[test]
fn a_certificate_can_be_replaced_without_a_restart() {
    let a = upstream("a");
    let dir = tmp("hot");
    let (c1, k1) = self_signed(A).unwrap();
    write_cert(&dir, "app", &c1, &k1);
    let p = free_port();
    let (o, log) = opts();
    let l = Listener::start(table(vec![https_route(A, a.port, p, "app")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let (c2, k2) = self_signed(A).unwrap();
    let cfg = trusting(&[&c1, &c2], &[&rustls::version::TLS13]);
    assert_eq!(tls_get(p, A, &cfg, A, "/").unwrap().1, der(&c1));
    write_cert(&dir, "app", &c2, &k2);
    assert_eq!(tls_get(p, A, &cfg, A, "/").unwrap().1, der(&c1), "nothing changes until the reload");
    let lines = l.reload_certs();
    assert!(lines.iter().any(|x| x.contains(A) && x.contains("valid")), "{lines:?}");
    assert_eq!(tls_get(p, A, &cfg, A, "/").unwrap().1, der(&c2), "a new connection uses the new certificate");
    // a broken replacement does not take the host down: the valid one stays, and the log says so
    write_cert(&dir, "app", "garbage", &k2);
    let lines = l.reload_certs();
    assert!(lines.iter().any(|x| x.contains("the previous one stays")), "{lines:?}");
    assert_eq!(tls_get(p, A, &cfg, A, "/").unwrap().1, der(&c2));
    assert!(l.refused().is_empty());
    assert!(!log.lock().unwrap().join("\n").contains("PRIVATE KEY"), "keys are never logged");
}

fn expired(host: &str) -> (String, String) {
    let mut p = rcgen::CertificateParams::new(vec![host.to_string()]).unwrap();
    p.not_before = rcgen::date_time_ymd(2000, 1, 1);
    p.not_after = rcgen::date_time_ymd(2001, 1, 1);
    let kp = rcgen::KeyPair::generate().unwrap();
    let c = p.self_signed(&kp).unwrap();
    (c.pem(), kp.serialize_pem())
}

#[test]
fn certificates_that_are_invalid_for_the_host_expired_or_with_another_key_are_refused() {
    let a = upstream("a");
    let dir = tmp("bad");
    let p = free_port();
    let (good, good_key) = self_signed(A).unwrap();
    write_cert(&dir, "good", &good, &good_key);
    let (wrong, wrong_key) = self_signed("elsewhere.example.org").unwrap();
    write_cert(&dir, "wrong", &wrong, &wrong_key);
    let (old, old_key) = expired("old.example.org");
    write_cert(&dir, "old", &old, &old_key);
    let (mix_chain, _) = self_signed("mix.example.org").unwrap();
    let (_, other_key) = self_signed("mix.example.org").unwrap();
    write_cert(&dir, "mix", &mix_chain, &other_key);
    let (o, log) = opts();
    let l = Listener::start(
        table(vec![
            https_route(A, a.port, p, "good"),
            https_route("wrong.example.org", a.port, p, "wrong"),
            https_route("old.example.org", a.port, p, "old"),
            https_route("mix.example.org", a.port, p, "mix"),
            https_route("missing.example.org", a.port, p, "absent"),
        ]),
        Arc::new(FileCerts::new(&dir)),
        Arc::new(NoGate),
        o,
    )
    .unwrap();
    let why = |h: &str| l.refused().into_iter().find(|(x, _)| x == h).map(|(_, w)| w).unwrap_or_default();
    assert!(why("wrong.example.org").contains("covers elsewhere.example.org and not wrong.example.org"), "{}", why("wrong.example.org"));
    assert!(why("old.example.org").contains("expired"), "{}", why("old.example.org"));
    assert!(why("mix.example.org").contains("does not belong"), "{}", why("mix.example.org"));
    assert!(why("missing.example.org").contains("no absent.crt"), "{}", why("missing.example.org"));
    assert_eq!(l.refused().len(), 4);
    let cfg = trusting(&[&good, &wrong, &old, &mix_chain], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    for h in ["wrong.example.org", "old.example.org", "mix.example.org", "missing.example.org"] {
        assert!(tls_connect(p, h, &cfg).is_err(), "{h} must not be served");
    }
    assert_eq!(tls_get(p, A, &cfg, A, "/").unwrap().0 .0, 200, "the valid host is served");
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("refused") && log.contains("old.example.org"), "{log}");
    for secret in [&good_key, &wrong_key, &old_key, &other_key] {
        let body = secret.lines().nth(1).unwrap();
        assert!(!log.contains(body) && !log.contains("PRIVATE KEY"), "a key was logged");
    }
}

#[test]
fn passthrough_reaches_a_tls_target_untouched_and_shares_a_port_with_https() {
    let a = upstream("a");
    // a TLS target with its own certificate
    let pass = "pass.example.org";
    let (pc, pk) = self_signed(pass).unwrap();
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(pk.as_bytes()).unwrap();
    let scfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from_pem_slice(pc.as_bytes()).unwrap()], key)
            .unwrap(),
    );
    let tl = TcpListener::bind("127.0.0.1:0").unwrap();
    let tport = tl.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in tl.incoming() {
            let Ok(tcp) = c else { continue };
            timeout(&tcp);
            let conn = rustls::ServerConnection::new(scfg.clone()).unwrap();
            let mut s = BufReader::new(rustls::StreamOwned::new(conn, tcp));
            let mut l = String::new();
            while l != "\r\n" {
                l.clear();
                if s.read_line(&mut l).unwrap_or(0) == 0 {
                    break;
                }
            }
            let _ = s.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\ntls-upstream");
        }
    });
    let dir = tmp("pass");
    let (ac, ak) = self_signed(A).unwrap();
    write_cert(&dir, "a", &ac, &ak);
    let p = free_port();
    let mut pr = route(pass, "/", tport, p);
    pr.scheme = Scheme::Passthrough;
    let (o, _) = opts();
    let _l = Listener::start(table(vec![pr, https_route(A, a.port, p, "a")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let cfg = trusting(&[&pc, &ac], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    let (resp, cert) = tls_get(p, pass, &cfg, pass, "/").unwrap();
    assert_eq!(text(&resp), "tls-upstream");
    assert_eq!(cert, der(&pc), "the certificate is the target's own: TLS was not terminated by the listener");
    let (resp, cert) = tls_get(p, A, &cfg, A, "/").unwrap();
    assert_eq!((text(&resp), cert), ("name=a path=/".to_string(), der(&ac)), "the https route on the same port is still terminated here");
}

#[test]
fn the_client_hello_reader_finds_the_server_name() {
    let cfg = trusting(&[], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    let hello = |name: &str| {
        let mut c = rustls::ClientConnection::new(cfg.clone(), ServerName::try_from(name.to_string()).unwrap()).unwrap();
        let mut v = Vec::new();
        c.write_tls(&mut v).unwrap();
        v
    };
    let h = hello("App.Example.org");
    assert_eq!(client_hello_sni(&h), Ok(Hello::Sni(Some("app.example.org".into()))));
    assert_eq!(client_hello_sni(&h[..3]), Ok(Hello::Incomplete));
    assert_eq!(client_hello_sni(&h[..h.len() - 1]), Ok(Hello::Incomplete));
    assert_eq!(client_hello_sni(&[]), Ok(Hello::Incomplete));
    assert_eq!(client_hello_sni(&hello("127.0.0.1")), Ok(Hello::Sni(None)), "an address is sent without a name");
    assert!(client_hello_sni(b"GET / HTTP/1.1\r\n").is_err());
    let mut cut = h.clone();
    cut[5] = 2; // not a ClientHello
    assert!(client_hello_sni(&cut).is_err());
}

// ------------------------------------------------------------------------------------------------------------ redirect, ports

#[test]
fn plain_http_is_redirected_to_https_when_the_route_asks() {
    let a = upstream("a");
    let dir = tmp("redir");
    let (c, k) = self_signed(A).unwrap();
    write_cert(&dir, "a", &c, &k);
    let (p, hp) = (free_port(), free_port());
    let (mut o, _) = opts();
    o.redirect_port = hp;
    let mut r = https_route(A, a.port, p, "a");
    r.redirect_http = true;
    let l = Listener::start(table(vec![r]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    assert!(l.ports().contains(&hp) && l.ports().contains(&p));
    let resp = http(hp, &get(A, "/x/y?z=1"));
    assert_eq!(resp.0, 308);
    assert_eq!(header(&resp, "location"), Some(format!("https://{A}:{p}/x/y?z=1").as_str()));
    assert_eq!(http(hp, &get("nobody.example.org", "/")).0, 404, "other hosts are not redirected");
    assert!(a.seen.lock().unwrap().is_empty(), "the target was not contacted");
}

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);

impl PortGate for Rec {
    fn open(&self, port: u16) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("open {port}"));
        Ok(())
    }
    fn close(&self, port: u16) {
        self.0.lock().unwrap().push(format!("close {port}"));
    }
}

#[test]
fn the_port_gate_hears_each_port_once_when_it_gains_its_first_route_and_loses_its_last() {
    let a = upstream("a");
    let mut ports = [free_port(), free_port(), free_port()];
    ports.sort_unstable();
    let [p1, p2, p3] = ports;
    let gate = Arc::new(Rec::default());
    let ev = || gate.0.lock().unwrap().clone();
    let (o, _) = opts();
    let l = Listener::start(table(vec![route(A, "/", a.port, p1), route(A, "/api", a.port, p1), route(B, "/", a.port, p2)]), Arc::new(FileCerts::new(tmp("none"))), gate.clone(), o).unwrap();
    assert_eq!(ev(), vec![format!("open {p1}"), format!("open {p2}")], "two routes on p1 open it once");
    // p1 loses one of two routes: still open; p2 loses its last: closed; p3 is new: opened
    l.update(table(vec![route(A, "/", a.port, p1), route("new.example.org", "/", a.port, p3)])).unwrap();
    assert_eq!(ev()[2..].to_vec(), vec![format!("open {p3}"), format!("close {p2}")]);
    assert_eq!(l.ports(), vec![p1, p3]);
    assert!(crate::testdir::port_is_closed(p2), "the closed port no longer accepts");
    assert_eq!(text(&http(p3, &get("new.example.org", "/"))), "name=a path=/");
    assert_eq!(http(p1, &get(B, "/")).0, 404, "the removed route is gone from the running listener");
    drop(l);
    let e = ev();
    assert_eq!(e[4..].to_vec(), vec![format!("close {p1}"), format!("close {p3}")]);
    assert_eq!(e.iter().filter(|x| x.starts_with("open")).count(), 3);
    assert_eq!(e.iter().filter(|x| x.starts_with("close")).count(), 3);
}

#[test]
fn a_port_that_cannot_be_bound_is_an_error_and_nothing_else_is_used() {
    let a = upstream("a");
    let mut ports = [free_port(), free_port()];
    ports.sort_unstable();
    let [free, taken] = ports;
    let _hold = TcpListener::bind(("127.0.0.1", taken)).unwrap();
    let gate = Arc::new(Rec::default());
    let (o, _) = opts();
    let e = Listener::start(table(vec![route(A, "/", a.port, free), route(B, "/", a.port, taken)]), Arc::new(FileCerts::new(tmp("none"))), gate.clone(), o).err().expect("must fail");
    assert!(e.contains(&format!("cannot listen on port {taken}")) && e.contains("no other port is used"), "{e}");
    assert_eq!(gate.0.lock().unwrap().clone(), vec![format!("open {free}"), format!("close {free}")], "the port bound before the failure is released again");
    assert!(crate::testdir::port_is_closed(free), "the released port is closed");
    // a table that mixes plain and tls routes on one port is refused before anything is bound
    let dir = tmp("mix");
    let (c, k) = self_signed(A).unwrap();
    write_cert(&dir, "a", &c, &k);
    let (o, _) = opts();
    let e = Listener::start(table(vec![route(B, "/", a.port, free), https_route(A, a.port, free, "a")]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).err().expect("must fail");
    assert!(e.contains("both plain http and tls"), "{e}");
}

#[test]
fn file_certs_find_a_certificate_by_name_or_by_host() {
    let dir = tmp("files");
    let (c, k) = self_signed(A).unwrap();
    write_cert(&dir, "named", &c, &k);
    write_cert(&dir, B, &c, &k);
    let f = FileCerts::new(&dir);
    assert_eq!(f.load(A, &CertRef::Vault("named".into())).unwrap().0, c);
    assert_eq!(f.load(B, &CertRef::Secret("other".into())).unwrap().1, k, "falls back to HOST.crt");
    let e = f.load("none.example.org", &CertRef::Vault("nothing".into())).unwrap_err();
    assert!(e.contains("no nothing.crt and nothing.key"), "{e}");
}

// ------------------------------------------------------------------------------------------------------------ vault-backed certificates

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct FakeVault(Arc<Mutex<HashMap<String, Result<Vec<u8>, String>>>>);

impl FakeVault {
    fn put(&self, name: &str, v: &str) {
        self.0.lock().unwrap().insert(name.into(), Ok(v.as_bytes().to_vec()));
    }
    fn put_cert(&self, name: &str, chain: &str, key: &str) {
        self.put(&format!("{name}-chain"), chain);
        self.put(&format!("{name}-key"), key);
    }
    fn fail(&self, name: &str, why: &str) {
        self.0.lock().unwrap().insert(name.into(), Err(why.into()));
    }
}

impl vault::SecretReader for FakeVault {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().get(name).cloned().unwrap_or_else(|| Err("404: no such secret".into()))
    }
}

fn vault_source(v: &FakeVault) -> Arc<vault::VaultCerts> {
    Arc::new(vault::VaultCerts::new(Box::new(v.clone())))
}

fn served_cert(p: u16, host: &str, chain: &str) -> Vec<u8> {
    tls_connect(p, host, &trusting(&[chain], &[&rustls::version::TLS13])).map(|(_, c, _)| c).unwrap_or_default()
}

#[test]
fn a_certificate_in_the_vault_is_served_replaced_on_rotation_and_the_last_good_one_stays_on_failure() {
    let a = upstream("a");
    let (chain1, key1) = self_signed(A).unwrap();
    let (chain2, key2) = self_signed(A).unwrap();
    let vault = FakeVault::default();
    vault.put_cert("appcert", &chain1, &key1);
    let p = free_port();
    let (o, log) = opts();
    let l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), vault_source(&vault), Arc::new(NoGate), o).unwrap();
    assert!(l.refused().is_empty(), "{:?}", l.refused());
    assert_eq!(served_cert(p, A, &chain1), der(&chain1));
    let cfg = trusting(&[&chain1], &[&rustls::version::TLS13]);
    assert_eq!(text(&tls_get(p, A, &cfg, A, "/x").unwrap().0), "name=a path=/x");

    // rotation: the vault now holds the second pair; a reload serves it to new connections
    vault.put_cert("appcert", &chain2, &key2);
    assert!(l.reload_certs().iter().any(|x| x.starts_with(&format!("certificate {A}: valid"))));
    assert_eq!(served_cert(p, A, &chain2), der(&chain2));

    // the vault is down: the second certificate stays
    vault.fail("appcert-chain", "503: vault unavailable");
    let lines = l.reload_certs().join("\n");
    assert!(lines.contains("the previous one stays") && lines.contains("vault unavailable"), "{lines}");
    assert_eq!(served_cert(p, A, &chain2), der(&chain2));

    // a half-rotated pair (new chain, old key) is refused, the old pair stays
    vault.put("appcert-chain", &chain1);
    vault.put("appcert-key", &key2);
    assert!(l.reload_certs().join("\n").contains("does not belong"));
    assert_eq!(served_cert(p, A, &chain2), der(&chain2));
    assert!(l.refused().is_empty());
    drop(log);
}

#[test]
fn nothing_from_the_vault_secrets_reaches_a_log_line_or_a_refusal_reason() {
    let a = upstream("a");
    let (chain, key) = self_signed(A).unwrap();
    let vault = FakeVault::default();
    // a key that is not a key but contains words that must never be printed
    vault.put_cert("appcert", &chain, "-----BEGIN PRIVATE KEY-----\nTOPSECRETKEYMATERIAL\n-----END PRIVATE KEY-----\n");
    let p = free_port();
    let (o, log) = opts();
    let l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), vault_source(&vault), Arc::new(NoGate), o).unwrap();
    assert_eq!(l.refused().len(), 1, "the host is not served over https");
    let mut everything = log.lock().unwrap().join("\n");
    everything.push_str(&format!("{:?}", l.refused()));
    everything.push_str(&l.reload_certs().join("\n"));
    assert!(!everything.contains("TOPSECRETKEYMATERIAL") && !everything.contains("BEGIN PRIVATE KEY"), "{everything}");
    // a good key whose bytes are never echoed either
    vault.put_cert("appcert", &chain, &key);
    l.reload_certs();
    let body = key.lines().nth(1).unwrap().to_string();
    assert!(!log.lock().unwrap().join("\n").contains(&body));
    // a vault error carrying the secret's name is fine; a secret that is not text is named, not shown
    vault.0.lock().unwrap().insert("appcert-key".into(), Ok(vec![0xff, 0xfe, 0x00]));
    let lines = l.reload_certs().join("\n");
    assert!(lines.contains("not text"), "{lines}");
}

#[test]
fn the_vault_source_refuses_kubernetes_secret_refs_missing_secrets_and_bad_names() {
    let vault = FakeVault::default();
    let src = vault_source(&vault);
    assert!(src.load(A, &CertRef::Secret("x".into())).unwrap_err().contains("not in the vault"));
    assert!(src.load(A, &CertRef::Vault("nothing".into())).unwrap_err().contains("nothing-chain"));
    assert!(src.load(A, &CertRef::Vault("../etc".into())).unwrap_err().contains("not a vault name"));
    assert!(src.load_client_ca("a b").unwrap_err().contains("not a vault name"));
    assert!(src.load_client_ca("missing").unwrap_err().contains("missing"));
}

#[test]
fn a_timer_reloads_the_vault_certificate_by_itself_and_stops_with_the_listener() {
    let a = upstream("a");
    let (chain1, key1) = self_signed(A).unwrap();
    let (chain2, key2) = self_signed(A).unwrap();
    let vault = FakeVault::default();
    vault.put_cert("appcert", &chain1, &key1);
    let p = free_port();
    let (mut o, _) = opts();
    o.cert_reload = Some(Duration::from_millis(100));
    let l = Listener::start(table(vec![https_route(A, a.port, p, "appcert")]), vault_source(&vault), Arc::new(NoGate), o).unwrap();
    assert_eq!(served_cert(p, A, &chain1), der(&chain1));
    vault.put_cert("appcert", &chain2, &key2);
    let until = Instant::now() + Duration::from_secs(10);
    while served_cert(p, A, &chain2) != der(&chain2) {
        assert!(Instant::now() < until, "the rotated certificate was never served");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(l);
}

// ------------------------------------------------------------------------------------------------------------ mutual TLS

struct Ca {
    pem: String,
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
}

fn make_ca(cn: &str) -> Ca {
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.distinguished_name.push(rcgen::DnType::CommonName, cn);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    Ca { pem: cert.pem(), cert, key }
}

/// A client certificate signed by `ca`; `expired` makes one that ended in 2001.
fn client_identity(ca: &Ca, cn: &str, expired: bool) -> (Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>) {
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    p.distinguished_name.push(rcgen::DnType::CommonName, cn);
    if expired {
        p.not_before = rcgen::date_time_ymd(2000, 1, 1);
        p.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.signed_by(&key, &ca.cert, &ca.key).unwrap();
    (vec![cert.der().clone()], rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into())
}

fn with_client(server_pems: &[&str], id: Option<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>)>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for p in server_pems {
        roots.add(CertificateDer::from_pem_slice(p.as_bytes()).unwrap()).unwrap();
    }
    let b = rustls::ClientConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12]).unwrap().with_root_certificates(roots);
    let mut cfg = match id {
        Some((c, k)) => b.with_client_auth_cert(c, k).unwrap(),
        None => b.with_no_client_auth(),
    };
    cfg.resumption = rustls::client::Resumption::disabled();
    Arc::new(cfg)
}

/// The status of `GET /` or the reason the connection did not get that far (a refused handshake).
fn status_of(port: u16, host: &str, cfg: &Arc<rustls::ClientConfig>) -> Result<u16, String> {
    let (mut s, _, _) = tls_connect(port, host, cfg)?;
    s.get_mut().write_all(get(host, "/").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    s.read_line(&mut line).map_err(|e| e.to_string())?;
    line.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("no answer ({line:?})"))
}

const M1: &str = "secure.example.org";
const M2: &str = "other.example.org";
const PLAIN: &str = "open.example.org";
const NOCA: &str = "broken.example.org";

struct Mtls {
    l: Listener,
    p: u16,
    dir: PathBuf,
    up: Up,
    server: HashMap<&'static str, String>,
    log: Log,
}

fn mtls_listener(ca1: &Ca, ca2: &Ca) -> Mtls {
    let up = upstream("secure");
    let dir = tmp("mtls");
    let mut server = HashMap::new();
    for h in [M1, M2, PLAIN, NOCA] {
        let (chain, key) = self_signed(h).unwrap();
        write_cert(&dir, &h.replace('.', "-"), &chain, &key);
        server.insert(h, chain);
    }
    std::fs::write(dir.join("ca1.ca"), &ca1.pem).unwrap();
    std::fs::write(dir.join("ca2.ca"), &ca2.pem).unwrap();
    let p = free_port();
    let (mut o, log) = opts();
    o.client_ca.insert(M1.into(), "ca1".into());
    o.client_ca.insert(M2.into(), "ca2".into());
    o.client_ca.insert(NOCA.into(), "nothere".into());
    let routes = [M1, M2, PLAIN, NOCA].iter().map(|h| https_route(h, up.port, p, &h.replace('.', "-"))).collect();
    let l = Listener::start(table(routes), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    Mtls { l, p, dir, up, server, log }
}

impl Mtls {
    fn status(&self, host: &'static str, id: Option<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>)>) -> Result<u16, String> {
        status_of(self.p, host, &with_client(&[&self.server[host]], id))
    }
}

#[test]
fn a_mutual_tls_route_accepts_only_a_certificate_of_its_own_ca() {
    let (ca1, ca2, rogue) = (make_ca("ca one"), make_ca("ca two"), make_ca("rogue"));
    let m = mtls_listener(&ca1, &ca2);
    // the right certificate
    assert_eq!(m.status(M1, Some(client_identity(&ca1, "alice", false))), Ok(200));
    assert_eq!(m.status(M2, Some(client_identity(&ca2, "bob", false))), Ok(200));
    let reached = m.up.seen.lock().unwrap().len();
    assert_eq!(reached, 2);
    // no certificate: 403 and the target is not contacted
    assert_eq!(m.status(M1, None), Ok(403));
    // a certificate of the other route's CA (which the handshake trusts) is still refused on this route
    assert_eq!(m.status(M1, Some(client_identity(&ca2, "bob", false))), Ok(403));
    assert_eq!(m.status(M2, Some(client_identity(&ca1, "alice", false))), Ok(403));
    // a certificate of a CA nobody configured: refused in the handshake or with 403, never 200
    assert_ne!(m.status(M1, Some(client_identity(&rogue, "mallory", false))), Ok(200));
    // an expired certificate of the right CA
    assert_ne!(m.status(M1, Some(client_identity(&ca1, "alice", true))), Ok(200));
    assert_eq!(m.up.seen.lock().unwrap().len(), reached, "none of the refused requests reached the target");
    // a route with no client CA does not ask for a certificate
    assert_eq!(m.status(PLAIN, None), Ok(200));
    // a route whose CA cannot be loaded turns every client away, even one with a certificate
    assert_eq!(m.status(NOCA, Some(client_identity(&ca1, "alice", false))), Ok(503));
    assert_eq!(m.status(NOCA, None), Ok(503));
    let log = m.log.lock().unwrap().join("\n");
    assert!(log.contains("client certificate refused") && log.contains("client CA broken.example.org: refused"), "{log}");
}

#[test]
fn a_client_ca_is_replaced_without_a_restart_and_a_bad_replacement_changes_nothing() {
    let (ca1, ca2) = (make_ca("ca one"), make_ca("ca two"));
    let m = mtls_listener(&ca1, &ca2);
    assert_eq!(m.status(M1, Some(client_identity(&ca1, "alice", false))), Ok(200));
    // garbage in the CA file: the previous CA stays
    std::fs::write(m.dir.join("ca1.ca"), "not a certificate").unwrap();
    let lines = m.l.reload_certs().join("\n");
    assert!(lines.contains("client CA secure.example.org: new CA refused"), "{lines}");
    assert_eq!(m.status(M1, Some(client_identity(&ca1, "alice", false))), Ok(200));
    // a real replacement: the old CA's clients are out, the new CA's are in
    let ca3 = make_ca("ca three");
    std::fs::write(m.dir.join("ca1.ca"), &ca3.pem).unwrap();
    m.l.reload_certs();
    assert_ne!(m.status(M1, Some(client_identity(&ca1, "alice", false))), Ok(200));
    assert_eq!(m.status(M1, Some(client_identity(&ca3, "carol", false))), Ok(200));
}

// ------------------------------------------------------------------------------------------------------------ pods and services

struct FakeProc {
    alive: Arc<AtomicBool>,
    killed: Arc<AtomicBool>,
    _tx: Option<std::sync::mpsc::Sender<String>>,
}

impl pod::Proc for FakeProc {
    fn alive(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    fn kill(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.killed.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Ready,
    Silent,
    Ends,
    Fail,
}

struct FakeKubectl {
    mode: Arc<Mutex<Mode>>,
    spawns: Arc<AtomicUsize>,
    procs: Arc<Mutex<Vec<(Arc<AtomicBool>, Arc<AtomicBool>)>>>,
    args: Arc<Mutex<Vec<Vec<String>>>>,
}

impl FakeKubectl {
    fn new() -> FakeKubectl {
        FakeKubectl { mode: Arc::new(Mutex::new(Mode::Ready)), spawns: Arc::new(AtomicUsize::new(0)), procs: Arc::new(Mutex::new(Vec::new())), args: Arc::new(Mutex::new(Vec::new())) }
    }
    /// A spawn function whose "forward" is the given local port.
    fn spawn(&self, local: u16) -> pod::Spawn {
        let (mode, spawns, procs, args) = (self.mode.clone(), self.spawns.clone(), self.procs.clone(), self.args.clone());
        Box::new(move |a: &[String]| {
            spawns.fetch_add(1, Ordering::SeqCst);
            args.lock().unwrap().push(a.to_vec());
            let m = *mode.lock().unwrap();
            if m == Mode::Fail {
                return Err("kubectl could not be started (no such file): install kubectl".into());
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let (alive, killed) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
            procs.lock().unwrap().push((alive.clone(), killed.clone()));
            let mut keep = None;
            match m {
                Mode::Ready => {
                    tx.send("Handling connection".into()).ok();
                    tx.send(format!("Forwarding from 127.0.0.1:{local} -> 80")).unwrap();
                    keep = Some(tx);
                }
                Mode::Silent => keep = Some(tx),
                _ => {}
            }
            Ok((Box::new(FakeProc { alive, killed, _tx: keep }) as Box<dyn pod::Proc>, rx))
        })
    }
}

fn svc_route(host: &str, name: &str, listen: u16) -> Route {
    Route { target: Target::Service { name: name.into(), port: 80 }, ..route(host, "/", 1, listen) }
}

fn with_dialer(routes: Vec<Route>, d: Arc<dyn ServiceDialer>) -> (Listener, Log) {
    let (mut o, log) = opts();
    o.dialer = Some(d);
    (Listener::start(table(routes), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap(), log)
}

#[test]
fn a_service_route_goes_through_a_port_forward_that_is_started_once_reused_and_restarted_when_it_dies() {
    let a = upstream("pod");
    let k = FakeKubectl::new();
    let pf = Arc::new(pod::PortForward::with_spawn("acme", k.spawn(a.port), Duration::from_secs(5)).unwrap());
    let p = free_port();
    let (l, log) = with_dialer(vec![svc_route(A, "web", p)], pf.clone());
    assert_eq!(text(&http(p, &get(A, "/one"))), "name=pod path=/one");
    assert_eq!(text(&http(p, &get(A, "/two"))), "name=pod path=/two");
    assert_eq!(k.spawns.load(Ordering::SeqCst), 1, "one forward serves both requests");
    assert_eq!(pf.running(), 1);
    assert_eq!(k.args.lock().unwrap()[0], ["port-forward", "--address=127.0.0.1", "--namespace=acme", "svc/web", ":80"]);
    // the forward dies: the next request starts a new one
    k.procs.lock().unwrap()[0].0.store(false, Ordering::SeqCst);
    assert_eq!(text(&http(p, &get(A, "/three"))), "name=pod path=/three");
    assert_eq!(k.spawns.load(Ordering::SeqCst), 2);
    assert!(log.lock().unwrap().iter().any(|l| l.contains("-> service web:80")), "{:?}", log.lock().unwrap());
    // dropping the dialer ends the forwards
    drop(l);
    drop(pf);
    assert!(k.procs.lock().unwrap().last().unwrap().1.load(Ordering::SeqCst), "the forward was killed with its owner");
}

#[test]
fn a_port_forward_that_is_not_ready_ends_early_or_cannot_start_answers_502_and_is_cleaned_up() {
    let a = upstream("pod");
    let k = FakeKubectl::new();
    let pf = Arc::new(pod::PortForward::with_spawn("acme", k.spawn(a.port), Duration::from_millis(300)).unwrap());
    let p = free_port();
    let (_l, log) = with_dialer(vec![svc_route(A, "web", p)], pf.clone());
    // never says it is ready: timeout, the process is killed
    *k.mode.lock().unwrap() = Mode::Silent;
    let t = Instant::now();
    assert_eq!(http(p, &get(A, "/")).0, 502);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(k.procs.lock().unwrap()[0].1.load(Ordering::SeqCst), "killed");
    assert_eq!(pf.running(), 0);
    // ends at once (no pod behind the service)
    *k.mode.lock().unwrap() = Mode::Ends;
    assert_eq!(http(p, &get(A, "/")).0, 502);
    // cannot be started at all
    *k.mode.lock().unwrap() = Mode::Fail;
    assert_eq!(http(p, &get(A, "/")).0, 502);
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("not ready within") && log.contains("ended before it was ready") && log.contains("install kubectl"), "{log}");
    // the forward is up but the port behind it is closed: 502, and the dead forward is dropped
    let dead = free_port();
    let k2 = FakeKubectl::new();
    let pf2 = Arc::new(pod::PortForward::with_spawn("acme", k2.spawn(dead), Duration::from_secs(2)).unwrap());
    let p2 = free_port();
    let (_l2, _) = with_dialer(vec![svc_route(A, "web", p2)], pf2.clone());
    assert_eq!(http(p2, &get(A, "/")).0, 502);
    assert_eq!(pf2.running(), 0);
}

#[test]
fn only_plain_service_names_and_ports_are_dialled_and_kubectl_flags_are_checked() {
    let k = FakeKubectl::new();
    let pf = pod::PortForward::with_spawn("acme", k.spawn(1), Duration::from_millis(100)).unwrap();
    for (n, port) in [("Bad Name", 80), ("web;rm", 80), ("--all", 80), ("", 80), ("web", 0)] {
        assert!(pf.dial(n, port, Duration::from_millis(100)).is_err(), "{n:?}");
    }
    assert_eq!(k.spawns.load(Ordering::SeqCst), 0, "nothing was started for a bad name");
    assert!(pod::PortForward::with_spawn("Bad NS", k.spawn(1), Duration::from_secs(1)).is_err());
    assert!(pod::PortForward::new("acme", Some("--evil"), None).is_err());
    assert!(pod::PortForward::new("acme", None, Some("")).is_err());
    assert!(pod::PortForward::new("acme", Some("/x/kubeconfig"), Some("ctx")).is_ok());
    assert_eq!(pod::forwarding_port("Forwarding from 127.0.0.1:41234 -> 80"), Some(41234));
    assert_eq!(pod::forwarding_port("Forwarding from [::1]:41234 -> 80"), None);
    assert_eq!(pod::forwarding_port("Forwarding from 127.0.0.1:0 -> 80"), None);
    assert_eq!(pod::forwarding_port("Handling connection for 41234"), None);
}

#[test]
fn a_service_is_reached_by_its_cluster_dns_name_and_a_slow_resolver_cannot_hold_a_request() {
    let a = upstream("dns");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (asked2, port) = (asked.clone(), a.port);
    let dns = pod::ClusterDns::new("acme", "svc.cluster.local").unwrap().with_resolver(Box::new(move |n, p| {
        asked2.lock().unwrap().push((n.to_string(), p));
        Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
    }));
    assert_eq!(dns.fqdn("web"), "web.acme.svc.cluster.local");
    let p = free_port();
    let (_l, _) = with_dialer(vec![svc_route(A, "web", p)], Arc::new(dns));
    assert_eq!(text(&http(p, &get(A, "/d"))), "name=dns path=/d");
    assert_eq!(asked.lock().unwrap()[0], ("web.acme.svc.cluster.local".to_string(), 80));
    // a resolver that does not answer in time, and one that finds nothing
    let slow = pod::ClusterDns::new("acme", "svc.cluster.local").unwrap().with_resolver(Box::new(|_, _| {
        std::thread::sleep(Duration::from_millis(1500));
        Ok(vec![])
    }));
    let t = Instant::now();
    let e = slow.dial("web", 80, Duration::from_millis(150)).unwrap_err();
    assert!(e.to_string().contains("did not resolve"), "{e}");
    assert!(t.elapsed() < Duration::from_millis(1400), "returned at the timeout, not when the resolver finished: {:?}", t.elapsed());
    let none = pod::ClusterDns::new("acme", "svc.cluster.local").unwrap().with_resolver(Box::new(|_, _| Ok(vec![])));
    assert!(none.dial("web", 80, Duration::from_millis(150)).is_err());
    assert!(none.dial("../x", 80, Duration::from_millis(150)).is_err());
    assert!(pod::ClusterDns::new("Bad NS", "svc.cluster.local").is_err());
}

#[test]
fn a_service_route_without_a_dialer_still_answers_502_and_a_service_can_be_a_passthrough_target() {
    let p = free_port();
    let (_l, _) = start(vec![svc_route(A, "web", p)]);
    let r = http(p, &get(A, "/"));
    assert_eq!((r.0, text(&r).contains("Kubernetes Service")), (502, true));
}

// ------------------------------------------------------------------------------------------------------------ the firewall gate

type Calls = Arc<Mutex<Vec<Vec<u16>>>>;

fn recording_gate(reach: gate::GateReach, fail_on: Option<u16>) -> (Arc<gate::FirewallGate>, Calls) {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let c2 = calls.clone();
    let g = gate::FirewallGate::with_apply(
        reach,
        Box::new(move |ports: &BTreeSet<u16>, _| {
            if fail_on.is_some_and(|f| ports.contains(&f)) {
                return Err("the firewall refused".into());
            }
            c2.lock().unwrap().push(ports.iter().copied().collect());
            Ok(())
        }),
    );
    (Arc::new(g), calls)
}

#[test]
fn the_firewall_gate_opens_a_port_with_its_first_route_and_closes_it_with_its_last() {
    let a = upstream("a");
    let (p1, p2) = (free_port(), free_port());
    let (g, calls) = recording_gate(gate::GateReach::LocalNetwork, None);
    let (o, _) = opts();
    let l = Listener::start(table(vec![route(A, "/", a.port, p1), route(B, "/", a.port, p2)]), Arc::new(FileCerts::new(tmp("none"))), g.clone(), o).unwrap();
    let mut both = vec![p1, p2];
    both.sort_unstable();
    assert_eq!(g.open_ports(), both);
    // a second route on an open port opens nothing new; dropping a port's last route closes it
    l.update(table(vec![route(A, "/", a.port, p1), route(A, "/x", a.port, p1)])).unwrap();
    assert_eq!(g.open_ports(), vec![p1]);
    assert_eq!(calls.lock().unwrap().last().unwrap(), &vec![p1]);
    drop(l);
    assert!(g.open_ports().is_empty(), "stopping the listener closes everything");
    assert_eq!(calls.lock().unwrap().last().unwrap(), &Vec::<u16>::new());
}

#[test]
fn a_firewall_that_refuses_stops_the_listener_from_starting_and_holds_no_port() {
    let a = upstream("a");
    let (p1, p2) = (free_port(), free_port());
    let (g, _) = recording_gate(gate::GateReach::LocalNetwork, Some(p2));
    let (o, _) = opts();
    let e = Listener::start(table(vec![route(A, "/", a.port, p1), route(B, "/", a.port, p2)]), Arc::new(FileCerts::new(tmp("none"))), g.clone(), o).err().unwrap();
    assert!(e.contains("the firewall could not be opened") && e.contains("the firewall refused"), "{e}");
    assert!(g.open_ports().len() <= 1 && !g.open_ports().contains(&p2));
    assert!(TcpListener::bind(("127.0.0.1", p2)).is_ok(), "the refused port is not held");
    assert!(g.open(0).is_err(), "port 0 is not a port");
}

#[test]
fn the_dry_run_of_the_gate_shows_the_rules_for_the_ports_without_touching_the_firewall() {
    use iyakki_fw::ruleset::Ctx;
    let base = iyakki_fw::Config::default();
    let local = gate::FirewallGate::dry_run(&base, &[443, 8443], gate::GateReach::LocalNetwork, &Ctx::default()).unwrap();
    assert!(local.contains("th dport 443") && local.contains("th dport 8443") && local.contains("ip saddr 192.168.0.0/16"), "{local}");
    assert!(!local.contains("0.0.0.0/0"), "{local}");
    let any = gate::FirewallGate::dry_run(&base, &[443], gate::GateReach::Anywhere, &Ctx::default()).unwrap();
    assert!(any.contains("th dport 443") && any.contains("0.0.0.0/0"), "{any}");
    let none = gate::FirewallGate::dry_run(&base, &[], gate::GateReach::Anywhere, &Ctx::default()).unwrap();
    assert!(!none.contains("domain listener"), "{none}");
}

// ------------------------------------------------------------------------------------------------------------ wiring: `cloud listen` flags -> setup

const KEYHEX: &str = "0101010101010101010101010101010101010101010101010101010101010101";

struct GateLog {
    calls: Calls,
    gate: Arc<gate::FirewallGate>,
}

struct Rig {
    vault: FakeVault,
    gate: Option<Result<GateLog, String>>,
    dialer: Option<Arc<dyn ServiceDialer>>,
    gate_asked: Arc<Mutex<Vec<(gate::GateReach, bool)>>>,
}

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

/// Fakes for every dependency of the setup: the vault, the key file, the environment, the firewall and the pod dialer.
fn deps(r: &Rig) -> setup::Deps {
    let v = r.vault.clone();
    let gate_pick: Result<Arc<dyn PortGate>, String> = match &r.gate {
        Some(Ok(g)) => Ok(g.gate.clone() as Arc<dyn PortGate>),
        Some(Err(e)) => Err(e.clone()),
        None => Err("no firewall in this test".into()),
    };
    let asked = r.gate_asked.clone();
    let dialer = r.dialer.clone();
    setup::Deps {
        env: Box::new(|_| None),
        read_file: Box::new(|p| if p == "member.key" { Ok(KEYHEX.into()) } else { Err(format!("{p}: no such file")) }),
        vault: Box::new(move |_client, _scope| Box::new(v.clone())),
        gate: Box::new(move |reach, start| {
            asked.lock().unwrap().push((reach, start));
            gate_pick.clone()
        }),
        port_forward: Box::new(move |_ns, _kc, _ctx| dialer.clone().ok_or_else(|| "no kubectl".to_string())),
    }
}

fn rig_with_gate() -> Rig {
    let (g, calls) = recording_gate(gate::GateReach::LocalNetwork, None);
    Rig { vault: FakeVault::default(), gate: Some(Ok(GateLog { calls, gate: g })), dialer: None, gate_asked: Arc::default() }
}

const VAULT_FLAGS: &str = "--cert-source vault --vault https://vault.internal:8445 --vault-fingerprint ab:cd --tenant acme --vault-user bob --vault-key member.key";

fn started(a: &str, routes: Vec<Route>, r: &Rig) -> Result<Listener, String> {
    let t = table(routes);
    let mut s = setup::build(&args(a), &t, &deps(r))?;
    s.options.log = Arc::new(|_| {});
    Listener::start(t, s.source, s.gate, s.options)
}

#[test]
fn the_vault_source_serves_a_vault_certificate_and_writes_no_key_to_disk() {
    let up = upstream("a");
    let p = free_port();
    let (chain, key) = self_signed(A).unwrap();
    let r = rig_with_gate();
    r.vault.put_cert("appcert", &chain, &key);
    let l = started(&format!("{VAULT_FLAGS} --bind 127.0.0.1 --reach local"), vec![https_route(A, up.port, p, "appcert")], &r).unwrap();
    let (resp, cert) = tls_get(p, A, &trusting(&[&chain], &[&rustls::version::TLS13]), A, "/").unwrap();
    assert_eq!((resp.0, cert), (200, der(&chain)));
    assert!(r.gate_asked.lock().unwrap().iter().all(|(reach, start)| *reach == gate::GateReach::LocalNetwork && !*start));
    drop(l);
}

#[test]
fn a_vault_that_is_down_serves_no_https_and_never_falls_back_to_files() {
    let up = upstream("a");
    let p = free_port();
    let r = rig_with_gate();
    r.vault.fail("appcert-chain", "the vault did not answer");
    let l = started(&format!("{VAULT_FLAGS} --bind 127.0.0.1"), vec![https_route(A, up.port, p, "appcert")], &r).unwrap();
    assert_eq!(l.refused().len(), 1, "{:?}", l.refused());
    let (chain, _) = self_signed(A).unwrap();
    assert!(tls_get(p, A, &trusting(&[&chain], &[&rustls::version::TLS13]), A, "/").is_err(), "the handshake fails: nothing is served");
    assert!(up.seen.lock().unwrap().is_empty());
    // a file source mixed in is a configuration error, not a fallback
    let t = table(vec![https_route(A, up.port, p, "appcert")]);
    let e = setup::build(&args(&format!("{VAULT_FLAGS} --cert-dir /somewhere --bind 127.0.0.1")), &t, &deps(&r)).err().unwrap();
    assert!(e.contains("one place only"), "{e}");
}

#[test]
fn the_vault_source_needs_its_address_user_key_and_scope_and_names_what_is_missing() {
    let r = rig_with_gate();
    let t = table(vec![https_route(A, 1, 8443, "c")]);
    let bad = |a: &str| setup::build(&args(a), &t, &deps(&r)).err().unwrap_or_else(|| panic!("accepted: {a}"));
    assert!(bad("--cert-source vault --bind 127.0.0.1").contains("--vault URL"));
    assert!(bad("--cert-source vault --vault http://plain --tenant t --vault-user u --vault-key member.key").contains("not usable"));
    assert!(bad("--cert-source vault --vault https://v:1 --tenant t --vault-key member.key").contains("--vault-user"));
    assert!(bad("--cert-source vault --vault https://v:1 --tenant t --vault-user u").contains("--vault-key"));
    assert!(bad("--cert-source vault --vault https://v:1 --tenant t --vault-user u --vault-key nothere").contains("no such file"));
    assert!(bad("--cert-source vault --vault https://v:1 --tenant t --vault-user u --vault-key member.key --vault-scope bogus").contains("--vault-scope"));
    assert!(bad("--cert-source vault --vault https://v:1 --vault-user u --vault-key member.key").contains("IYAKKI_TENANT"));
    assert!(bad("--cert-source nowhere").contains("file or vault"));
}

fn vault_mtls_rig(ca1: &Ca, ca2: &Ca, hosts: &[&str]) -> (Rig, HashMap<String, String>) {
    let r = rig_with_gate();
    let mut chains = HashMap::new();
    for h in hosts {
        let (chain, key) = self_signed(h).unwrap();
        r.vault.put_cert(&h.replace('.', "-"), &chain, &key);
        chains.insert(h.to_string(), chain);
    }
    r.vault.put("ca-one", &ca1.pem);
    r.vault.put("ca-two", &ca2.pem);
    (r, chains)
}

#[test]
fn mutual_tls_from_flags_takes_the_client_ca_from_the_vault_and_refuses_bad_and_wrong_tenant_certificates() {
    let (ca1, ca2) = (make_ca("tenant one"), make_ca("tenant two"));
    let (r, chains) = vault_mtls_rig(&ca1, &ca2, &[M1, M2]);
    let up = upstream("t");
    let p = free_port();
    let routes = vec![https_route(M1, up.port, p, &M1.replace('.', "-")), https_route(M2, up.port, p, &M2.replace('.', "-"))];
    let flags = format!("{VAULT_FLAGS} --bind 127.0.0.1 --client-ca {M1}=ca-one --client-ca {M2}=ca-two --route-tenant {M1}=one --route-tenant {M2}=two");
    let l = started(&flags, routes, &r).unwrap();
    let cfg = |host: &str, id| with_client(&[&chains[host]], id);
    assert_eq!(status_of(p, M1, &cfg(M1, Some(client_identity(&ca1, "alice", false)))), Ok(200));
    assert_eq!(status_of(p, M2, &cfg(M2, Some(client_identity(&ca2, "bob", false)))), Ok(200));
    let reached = up.seen.lock().unwrap().len();
    // no certificate, a certificate of the other tenant's CA, an expired one, a certificate of an unknown CA
    assert_eq!(status_of(p, M1, &cfg(M1, None)), Ok(403));
    assert_eq!(status_of(p, M1, &cfg(M1, Some(client_identity(&ca2, "bob", false)))), Ok(403), "wrong tenant");
    assert_eq!(status_of(p, M2, &cfg(M2, Some(client_identity(&ca1, "alice", false)))), Ok(403), "wrong tenant");
    assert_ne!(status_of(p, M1, &cfg(M1, Some(client_identity(&ca1, "alice", true)))), Ok(200));
    assert_ne!(status_of(p, M1, &cfg(M1, Some(client_identity(&make_ca("rogue"), "mallory", false)))), Ok(200));
    assert_eq!(up.seen.lock().unwrap().len(), reached, "no refused request reached the target");
    assert_eq!((l.tenant_open("one"), l.tenant_open("two")), (0, 0), "refused requests hold no tenant slot");
}

#[test]
fn a_client_ca_that_the_vault_cannot_give_turns_every_client_away() {
    let (ca1, ca2) = (make_ca("tenant one"), make_ca("tenant two"));
    let (r, chains) = vault_mtls_rig(&ca1, &ca2, &[M1]);
    r.vault.fail("ca-one", "the vault did not answer");
    let up = upstream("t");
    let p = free_port();
    let l = started(&format!("{VAULT_FLAGS} --bind 127.0.0.1 --client-ca {M1}=ca-one"), vec![https_route(M1, up.port, p, &M1.replace('.', "-"))], &r).unwrap();
    assert_eq!(status_of(p, M1, &with_client(&[&chains[M1]], Some(client_identity(&ca1, "alice", false)))), Ok(503));
    assert!(up.seen.lock().unwrap().is_empty());
    drop(l);
}

#[test]
fn a_client_ca_on_a_route_that_is_not_https_is_a_configuration_error() {
    let r = rig_with_gate();
    let t = table(vec![route(A, "/", 1, 8080), https_route(B, 1, 8443, "c")]);
    for a in [format!("--client-ca {A}=ca --bind 127.0.0.1"), "--client-ca nothere.example.org=ca".to_string(), format!("--client-ca {B}")] {
        assert!(setup::build(&args(&a), &t, &deps(&r)).is_err(), "{a}");
    }
    assert!(setup::build(&args(&format!("--client-ca {B}=ca --client-ca {B}=other --bind 127.0.0.1")), &t, &deps(&r)).is_err(), "twice");
}

#[test]
fn a_service_route_is_reached_through_the_dialer_the_flags_choose() {
    struct ToPort(u16, Mutex<Vec<String>>);
    impl ServiceDialer for ToPort {
        fn dial(&self, name: &str, port: u16, _t: Duration) -> io::Result<TcpStream> {
            self.1.lock().unwrap().push(format!("{name}:{port}"));
            TcpStream::connect(("127.0.0.1", self.0))
        }
    }
    let up = upstream("pod");
    let d = Arc::new(ToPort(up.port, Mutex::new(Vec::new())));
    let mut r = rig_with_gate();
    r.dialer = Some(d.clone());
    let p = free_port();
    let l = started("--bind 127.0.0.1 --service-dialer port-forward --namespace iyakki-acme", vec![svc_route(A, "web", p)], &r).unwrap();
    assert_eq!(http(p, &get(A, "/x")).0, 200);
    assert_eq!(d.1.lock().unwrap().as_slice(), ["web:80"]);
    drop(l);
    // the same route without a dialer is a 502, and the setup says so
    let t = table(vec![svc_route(A, "web", free_port())]);
    let s = setup::build(&args("--bind 127.0.0.1"), &t, &deps(&r)).unwrap();
    assert!(s.options.dialer.is_none() && s.notes.iter().any(|n| n.contains("502")));
    // a dialer needs its namespace, and a kubectl that is missing is an error
    assert!(setup::build(&args("--bind 127.0.0.1 --service-dialer port-forward"), &t, &deps(&r)).is_err());
    assert!(setup::build(&args("--bind 127.0.0.1 --service-dialer cluster-dns"), &t, &deps(&r)).is_err());
    assert!(setup::build(&args("--bind 127.0.0.1 --service-dialer cluster-dns --namespace iyakki-acme"), &t, &deps(&r)).unwrap().options.dialer.is_some());
    assert!(setup::build(&args("--bind 127.0.0.1 --service-dialer teleport"), &t, &deps(&r)).is_err());
    let none = Rig { dialer: None, ..rig_with_gate() };
    assert!(setup::build(&args("--bind 127.0.0.1 --service-dialer port-forward --namespace iyakki-acme"), &t, &deps(&none)).is_err());
}

#[test]
fn without_a_working_firewall_the_listener_does_not_open_any_port() {
    let up = upstream("a");
    let p = free_port();
    let t = table(vec![route(A, "/", up.port, p)]);
    // the firewall is unavailable: no default fallback to "no gate", even on a loopback bind
    let r = Rig { gate: Some(Err("the kernel firewall is not available".into())), ..rig_with_gate() };
    let e = setup::build(&args("--bind 127.0.0.1"), &t, &deps(&r)).err().unwrap();
    assert!(e.contains("does not open") && e.contains("not available"), "{e}");
    assert!(TcpListener::bind(("127.0.0.1", p)).is_ok(), "no port was bound");
    // --no-firewall is for loopback only
    for bind in ["0.0.0.0", "192.168.1.5", "::"] {
        let e = setup::build(&args(&format!("--no-firewall --bind {bind}")), &t, &deps(&r)).err().unwrap();
        assert!(e.contains("loopback"), "{bind}: {e}");
    }
    assert!(setup::build(&args("--no-firewall"), &t, &deps(&r)).is_err(), "the default bind is every interface");
    r.gate_asked.lock().unwrap().clear();
    let s = setup::build(&args("--no-firewall --bind 127.0.0.1"), &t, &deps(&r)).unwrap();
    assert!(s.notes.iter().any(|n| n.contains("firewall is not driven")));
    assert!(r.gate_asked.lock().unwrap().is_empty(), "--no-firewall never asks for the firewall");
}

#[test]
fn a_gate_that_refuses_one_port_stops_the_listener_and_the_reach_and_start_flags_reach_the_gate() {
    let up = upstream("a");
    let (p1, p2) = (free_port(), free_port());
    let (g, _) = recording_gate(gate::GateReach::Anywhere, Some(p2));
    let r = Rig { gate: Some(Ok(GateLog { calls: Arc::default(), gate: g.clone() })), ..rig_with_gate() };
    let e = started("--bind 127.0.0.1 --reach anywhere --firewall-start", vec![route(A, "/", up.port, p1), route(B, "/", up.port, p2)], &r).err().unwrap();
    assert!(e.contains("the firewall could not be opened"), "{e}");
    assert!(TcpListener::bind(("127.0.0.1", p2)).is_ok());
    assert_eq!(r.gate_asked.lock().unwrap().as_slice(), [(gate::GateReach::Anywhere, true)]);
    assert!(setup::build(&args("--bind 127.0.0.1 --reach everyone"), &table(vec![]), &deps(&r)).is_err());
    // with a working gate the ports are opened and closed with the listener
    let r = rig_with_gate();
    let calls = r.gate.as_ref().unwrap().as_ref().ok().unwrap().calls.clone();
    let l = started("--bind 127.0.0.1", vec![route(A, "/", up.port, p1)], &r).unwrap();
    assert_eq!(calls.lock().unwrap().last().unwrap(), &vec![p1]);
    drop(l);
    assert_eq!(calls.lock().unwrap().last().unwrap(), &Vec::<u16>::new());
}

/// An upstream that holds each request until `release` is sent.
fn holding_upstream() -> (u16, std::sync::mpsc::Sender<()>, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = Arc::new(Mutex::new(rx));
    let held = Arc::new(AtomicUsize::new(0));
    let h2 = held.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(c) = c else { continue };
            let (rx, h) = (rx.clone(), h2.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut w = c;
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                        break;
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let _ = rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
                let _ = w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nheld");
            });
        }
    });
    (port, tx, held)
}

#[test]
fn each_tenant_has_its_own_limit_and_a_route_without_a_tenant_is_refused() {
    let (hold_port, release, held) = holding_upstream();
    let fast = upstream("fast");
    let p = free_port();
    let r = rig_with_gate();
    let routes = vec![route(A, "/", hold_port, p), route(B, "/", fast.port, p)];
    let l = started(&format!("--bind 127.0.0.1 --route-tenant {A}=acme --route-tenant {B}=globex --tenant-limit 1"), routes.clone(), &r).unwrap();
    // acme's first request is held open at its upstream
    let first = std::thread::spawn(move || http(p, &get(A, "/")).0);
    let t0 = Instant::now();
    while held.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(l.tenant_open("acme"), 1);
    // its second is turned away, the other tenant is not affected
    let r2 = http(p, &get(A, "/"));
    assert_eq!(r2.0, 503, "{}", text(&r2));
    assert_eq!(held.load(Ordering::SeqCst), 1, "the second request never reached the target");
    assert_eq!(http(p, &get(B, "/")).0, 200);
    release.send(()).unwrap();
    assert_eq!(first.join().unwrap(), 200);
    let t0 = Instant::now();
    while l.tenant_open("acme") != 0 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!((l.tenant_open("acme"), l.tenant_open("globex")), (0, 0), "slots are released");
    drop(l);
    // every route needs a tenant: from the flags, and in the listener itself (also on update)
    let e = started(&format!("--bind 127.0.0.1 --route-tenant {A}=acme"), routes.clone(), &r).err().unwrap();
    assert!(e.contains(B) && e.contains("tenant"), "{e}");
    assert!(setup::build(&args(&format!("--bind 127.0.0.1 --route-tenant nothere.example.org=x")), &table(routes.clone()), &deps(&r)).is_err());
    assert!(setup::build(&args(&format!("--bind 127.0.0.1 --route-tenant {A}=a --tenant-limit 0")), &table(routes.clone()), &deps(&r)).is_err());
    assert!(setup::build(&args("--bind 127.0.0.1 --tenant-limit 5"), &table(routes.clone()), &deps(&r)).is_err());
    let (mut o, _) = opts();
    o.tenants.insert(A.into(), "acme".into());
    let l = Listener::start(table(vec![route(A, "/", fast.port, free_port())]), Arc::new(FileCerts::new(tmp("none"))), Arc::new(NoGate), o).unwrap();
    assert!(l.update(table(vec![route(A, "/", fast.port, free_port()), route(B, "/", fast.port, free_port())])).is_err(), "a new route without a tenant is refused and nothing changes");
    assert_eq!(l.ports().len(), 1);
}

#[test]
fn a_listener_without_tenants_behaves_as_before_and_the_help_text_lists_the_flags() {
    let up = upstream("a");
    let p = free_port();
    let r = rig_with_gate();
    let l = started("--bind 127.0.0.1", vec![route(A, "/", up.port, p)], &r).unwrap();
    assert_eq!(http(p, &get(A, "/")).0, 200);
    assert_eq!(l.tenant_open("acme"), 0);
    for f in ["--cert-source", "--vault-key", "--client-ca", "--service-dialer", "--reach", "--no-firewall", "--route-tenant", "--tenant-limit", "does NOT open"] {
        assert!(setup::USAGE.contains(f), "{f}");
    }
    // the command prints its help
    let mut said = Vec::new();
    crate::cloudcli::run(&args("listen --help"), &mut |l| said.push(l.to_string())).unwrap();
    assert!(said.join("\n").contains("--cert-source"));
}

#[test]
fn a_client_hello_split_over_several_tls_records_gives_the_same_server_name() {
    let cfg = trusting(&[], &[&rustls::version::TLS13, &rustls::version::TLS12]);
    let mut c = rustls::ClientConnection::new(cfg, ServerName::try_from("split.example.org".to_string()).unwrap()).unwrap();
    let mut whole = Vec::new();
    c.write_tls(&mut whole).unwrap();
    assert_eq!(client_hello_sni(&whole), Ok(Hello::Sni(Some("split.example.org".into()))));
    let hs = &whole[5..];
    let records = |sizes: &[usize]| {
        let mut out = Vec::new();
        let mut rest = hs;
        for n in sizes {
            let (a, b) = rest.split_at((*n).min(rest.len()));
            out.extend_from_slice(&[0x16, 3, 1, (a.len() >> 8) as u8, a.len() as u8]);
            out.extend_from_slice(a);
            rest = b;
        }
        if !rest.is_empty() {
            out.extend_from_slice(&[0x16, 3, 1, (rest.len() >> 8) as u8, rest.len() as u8]);
            out.extend_from_slice(rest);
        }
        out
    };
    // two records, three, and a split inside the 4-byte handshake header
    for sizes in [vec![10], vec![40, 40], vec![2], vec![1, 1, 1, 1]] {
        let v = records(&sizes);
        assert_eq!(client_hello_sni(&v), Ok(Hello::Sni(Some("split.example.org".into()))), "{sizes:?}");
        // every cut of the stream before its end is "need more", never a wrong answer
        for cut in [0, 3, 5, 6, v.len() / 2, v.len() - 1] {
            assert_eq!(client_hello_sni(&v[..cut]), Ok(Hello::Incomplete), "{sizes:?} cut {cut}");
        }
    }
    // another kind of record in the middle, an empty record, a huge hello: refused
    let mut mixed = records(&[20]);
    mixed[0] = 0x17;
    let first_len = 5 + 20;
    mixed[first_len] = 0x17;
    assert!(client_hello_sni(&mixed).is_err());
    let mut other = records(&[20]);
    other[25] = 0x15; // the second record is an alert, the hello is not complete
    assert!(client_hello_sni(&other).is_err());
    assert!(client_hello_sni(&[0x16, 3, 1, 0, 0]).is_err(), "an empty record");
    let mut big = vec![0x16, 3, 1, 0x20, 0x00, 1, 0xff, 0xff, 0xff];
    big.extend(std::iter::repeat(0u8).take(0x2000 - 4));
    assert_eq!(client_hello_sni(&big), Err("ClientHello too large"));
    // a stream of full records that never completes a hello stops at the limit
    let mut endless = Vec::new();
    for i in 0..4 {
        endless.extend_from_slice(&[0x16, 3, 1, 0x40, 0x00]);
        if i == 0 {
            endless.extend_from_slice(&[1, 0x00, 0x7f, 0xff]);
            endless.extend(std::iter::repeat(0u8).take(0x4000 - 4));
        } else {
            endless.extend(std::iter::repeat(0u8).take(0x4000));
        }
    }
    assert!(client_hello_sni(&endless).is_ok(), "a 32 KiB hello is still within the limit: it is incomplete or parsed, not looped on");
}

#[test]
fn many_idle_tunnels_sleep_instead_of_polling_and_still_carry_bytes_both_ways_at_once() {
    use std::sync::atomic::Ordering;
    let a = upstream("a");
    let p = free_port();
    let (_l, _) = start(vec![route(A, "/", a.port, p)]);
    let open = || {
        let mut s = connect(p);
        s.write_all(format!("GET /ws HTTP/1.1\r\nHost: {A}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: x\r\n\r\n").as_bytes()).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("HTTP/1.1 101"), "{line}");
        while line != "\r\n" {
            line.clear();
            r.read_line(&mut line).unwrap();
        }
        let mut w = [0u8; 7];
        r.read_exact(&mut w).unwrap();
        (s, r)
    };
    let mut conns: Vec<_> = (0..60).map(|_| open()).collect();
    std::thread::sleep(Duration::from_millis(300)); // let every tunnel settle into its wait
    let before = TUNNEL_PASSES.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1500));
    let idle_passes = TUNNEL_PASSES.load(Ordering::Relaxed) - before;
    // polling every 10 ms would be about 60 x 150 = 9000 passes in this time; sleeping gives about one a second each (60 to 120)
    assert!(idle_passes < 600, "idle tunnels still look at their sockets {idle_passes} times in 1.5 s");
    // and every one of them wakes at once when bytes arrive, in both directions together
    let t0 = Instant::now();
    for (i, (s, _)) in conns.iter_mut().enumerate() {
        s.write_all(format!("msg{i:03}").as_bytes()).unwrap();
    }
    for (i, (_, r)) in conns.iter_mut().enumerate() {
        let mut got = [0u8; 6];
        r.read_exact(&mut got).unwrap();
        assert_eq!(&got, format!("msg{i:03}").as_bytes());
    }
    assert!(t0.elapsed() < Duration::from_secs(3), "60 tunnels answered in {:?}", t0.elapsed());
}

#[test]
fn a_client_ca_named_in_the_route_file_guards_the_whole_host_and_the_target_learns_which_certificate_came() {
    use sha2::{Digest, Sha256};
    let ca = make_ca("ca one");
    let up = upstream("secure");
    let dir = tmp("mtls-file");
    let (chain, key) = self_signed(M1).unwrap();
    write_cert(&dir, "secure", &chain, &key);
    std::fs::write(dir.join("ca1.ca"), &ca.pem).unwrap();
    let p = free_port();
    let mut guarded = https_route(M1, up.port, p, "secure");
    guarded.client_ca = Some("ca1".into());
    let mut open_path = https_route(M1, up.port, p, "secure");
    open_path.path_prefix = "/open".into(); // no client_ca of its own: the HOST is guarded, so this path is too
    let (o, _) = opts();
    let _l = Listener::start(table(vec![guarded, open_path]), Arc::new(FileCerts::new(&dir)), Arc::new(NoGate), o).unwrap();
    let call = |path: &str, id: Option<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>)>, extra: &str| -> Result<u16, String> {
        let cfg = with_client(&[&chain], id);
        let (mut s, _, _) = tls_connect(p, M1, &cfg)?;
        s.get_mut().write_all(format!("GET {path} HTTP/1.1\r\nHost: {M1}\r\n{extra}Connection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        s.read_line(&mut line).map_err(|e| e.to_string())?;
        line.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("no answer ({line:?})"))
    };
    assert_eq!(call("/", None, ""), Ok(403));
    assert_eq!(call("/open", None, ""), Ok(403), "another path of a guarded host is guarded too");
    assert!(saw(&up).is_empty(), "nothing reached the target");
    let (certs, k) = client_identity(&ca, "alice", false);
    let want: String = Sha256::digest(certs[0].as_ref()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(call("/open", Some((certs.clone(), k.clone_key())), "X-Client-Cert-Sha256: forged\r\n"), Ok(200));
    let seen = saw(&up).to_lowercase();
    assert!(seen.contains(&format!("x-client-cert-sha256: {want}")), "{seen}");
    assert!(!seen.contains("forged"), "a header the client made up is dropped: {seen}");
}

#[test]
fn a_route_file_names_a_client_ca_only_on_an_https_route_with_a_plain_name() {
    let mut r = route(A, "/", 8080, 8443);
    r.client_ca = Some("ca1".into());
    assert!(RouteTable { routes: vec![r.clone()], ingress_class: None }.check(&[]).unwrap_err().contains("https route"));
    let mut h = https_route(A, 8080, 8443, "appcert");
    h.client_ca = Some("../x".into());
    assert!(RouteTable { routes: vec![h.clone()], ingress_class: None }.check(&[]).unwrap_err().contains("client CA"));
    h.client_ca = Some("ca1".into());
    assert!(RouteTable { routes: vec![h.clone()], ingress_class: None }.check(&[]).is_ok());
    // it round-trips through the route file and is absent from a file that does not use it
    let text = serde_json::to_string(&RouteTable { routes: vec![h], ingress_class: None }).unwrap();
    assert!(text.contains("\"client_ca\":\"ca1\""));
    assert!(!serde_json::to_string(&RouteTable { routes: vec![https_route(A, 8080, 8443, "appcert")], ingress_class: None }).unwrap().contains("client_ca"));
}
