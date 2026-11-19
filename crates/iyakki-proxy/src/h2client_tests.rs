use super::*;
use crate::hpack::{Decoder, Encoder};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

fn read_frame<S: Read>(s: &mut S) -> Option<(u8, u8, u32, Vec<u8>)> {
    let mut h = [0u8; 9];
    s.read_exact(&mut h).ok()?;
    let n = (h[0] as usize) << 16 | (h[1] as usize) << 8 | h[2] as usize;
    let mut p = vec![0u8; n];
    s.read_exact(&mut p).ok()?;
    Some((h[3], h[4], u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff, p))
}

static SILENT_PINGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Rq {
    id: u32,
    method: String,
    path: String,
    body: Vec<u8>,
    ended: bool,
    answered: bool,
}

fn answer<S: Write>(s: &mut S, enc: &mut Encoder, id: u32, ctype: &str, body: &[u8]) {
    let block = enc.encode(&[(":status".into(), "200".into()), ("content-type".into(), ctype.into()), ("content-length".into(), body.len().to_string())]);
    let mut o = Vec::new();
    crate::h2::frame(&mut o, 1, 0x4, id, &block);
    let chunks: Vec<&[u8]> = body.chunks(16_384).collect();
    for (i, c) in chunks.iter().enumerate() {
        crate::h2::frame(&mut o, 0, if i + 1 == chunks.len() { 1 } else { 0 }, id, c);
    }
    if chunks.is_empty() {
        crate::h2::frame(&mut o, 0, 1, id, &[]);
    }
    s.write_all(&o).unwrap();
    s.flush().unwrap();
}

/// A small HTTP/2 server written with the codecs of this crate: answers by path, and answers `/b` before `/a` whatever the order of arrival.
fn serve_conn<S: Read + Write>(mut s: S) {
    let mut pre = [0u8; 24];
    if s.read_exact(&mut pre).is_err() || pre != *crate::h2::PREFACE {
        return;
    }
    let mut o = Vec::new();
    crate::h2::frame(&mut o, 4, 0, 0, &[]);
    s.write_all(&o).unwrap();
    s.flush().unwrap();
    let (mut dec, mut enc) = (Decoder::new(4096), Encoder::default());
    let mut reqs: Vec<Rq> = Vec::new();
    let (mut held_a, mut b_done) = (None::<u32>, false);
    while let Some((kind, flags, id, p)) = read_frame(&mut s) {
        match kind {
            4 if flags & 1 == 0 => {
                let mut o = Vec::new();
                crate::h2::frame(&mut o, 4, 1, 0, &[]);
                s.write_all(&o).unwrap();
            }
            1 => {
                let hs = dec.decode(&p).unwrap();
                let get = |n: &str| hs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_default();
                reqs.push(Rq { id, method: get(":method"), path: get(":path"), body: Vec::new(), ended: flags & 1 != 0, answered: false });
            }
            0 => {
                if let Some(r) = reqs.iter_mut().find(|r| r.id == id) {
                    r.body.extend_from_slice(&p);
                    r.ended = flags & 1 != 0;
                }
            }
            // a PING is answered with the same eight bytes (unless the test server is told to stay silent)
            6 if flags & 1 == 0 && !SILENT_PINGS.load(Ordering::SeqCst) => {
                let mut o = Vec::new();
                crate::h2::frame(&mut o, 6, 1, 0, &p);
                s.write_all(&o).unwrap();
                s.flush().unwrap();
            }
            _ => {}
        }
        for i in 0..reqs.len() {
            if !reqs[i].ended || reqs[i].answered {
                continue;
            }
            reqs[i].answered = true;
            let (id, path) = (reqs[i].id, reqs[i].path.clone());
            match path.as_str() {
                "/a" => {
                    if b_done {
                        answer(&mut s, &mut enc, id, "text/plain", b"ay");
                    } else {
                        held_a = Some(id);
                    }
                }
                "/b" => {
                    answer(&mut s, &mut enc, id, "text/plain", b"bee");
                    b_done = true;
                    if let Some(a) = held_a.take() {
                        answer(&mut s, &mut enc, a, "text/plain", b"ay");
                    }
                }
                "/misdirected" => {
                    let block = enc.encode(&[(":status".into(), "421".into()), ("content-length".into(), "0".into())]);
                    let mut o = Vec::new();
                    crate::h2::frame(&mut o, 1, 0x5, id, &block);
                    s.write_all(&o).unwrap();
                }
                "/big" => answer(&mut s, &mut enc, id, "text/plain", &vec![b'x'; 300_000]),
                "/media" => answer(&mut s, &mut enc, id, "audio/mpeg", &vec![b'm'; 900_000]),
                "/post" => answer(&mut s, &mut enc, id, "text/plain", format!("{} got {} bytes", reqs[i].method, reqs[i].body.len()).as_bytes()),
                "/bye" => {
                    answer(&mut s, &mut enc, id, "text/plain", b"bye");
                    let mut o = Vec::new();
                    let mut pl = id.to_be_bytes().to_vec();
                    pl.extend_from_slice(&0u32.to_be_bytes());
                    crate::h2::frame(&mut o, 7, 0, 0, &pl);
                    s.write_all(&o).unwrap();
                    // the server keeps the socket open (closing at once could reset it and lose the answer); it takes no more streams
                }
                _ => answer(&mut s, &mut enc, id, "text/plain", b"hello over h2"),
            }
        }
        let _ = s.flush();
    }
}

/// A plain TCP server; its address.
fn serve_plain() -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let (port, n2) = (l.local_addr().unwrap().port(), n.clone());
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            n2.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || serve_conn(s));
        }
    });
    (port, n)
}

fn plain_dialer(port: u16, dials: Arc<AtomicUsize>) -> Dialer {
    Arc::new(move |_h: &str, _p: u16| {
        dials.fetch_add(1, Ordering::SeqCst);
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        s.set_nodelay(true).ok();
        Ok(Dialed::H2(Box::new(s)))
    })
}

fn get(path: &str) -> H2Request {
    H2Request { method: "GET".into(), authority: "example.test".into(), path: path.into(), headers: vec![("accept".into(), "*/*".into())], body: Vec::new(), max_body: 12 << 20, media_cap: 100_000 }
}

fn ok(p: Pooled) -> H2Response {
    match p {
        Pooled::Done(Ok(r)) => r,
        Pooled::Done(Err(e)) => panic!("failed: {e}"),
        Pooled::Http1 => panic!("went to HTTP/1.1"),
    }
}

const T: Duration = Duration::from_secs(30);

#[test]
fn a_get_and_a_post_go_over_one_connection_and_a_big_answer_arrives_whole() {
    let (port, accepted) = serve_plain();
    let dials = Arc::new(AtomicUsize::new(0));
    let pool = H2Pool::new(plain_dialer(port, dials.clone()));
    let r = ok(pool.fetch("example.test", 443, &get("/hello"), T));
    assert_eq!((r.status, r.body.as_slice()), (200, &b"hello over h2"[..]));
    let mut post = get("/post");
    post.method = "POST".into();
    post.body = vec![7u8; 50_000];
    let r = ok(pool.fetch("example.test", 443, &post, T));
    assert_eq!(r.body, b"POST got 50000 bytes");
    let r = ok(pool.fetch("example.test", 443, &get("/big"), T));
    assert_eq!(r.body.len(), 300_000);
    assert!(r.body.iter().all(|&b| b == b'x'));
    assert_eq!(dials.load(Ordering::SeqCst), 1, "all three used the one connection");
    assert_eq!(pool.connections(), 1);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn two_requests_at_once_share_one_connection_and_the_later_one_may_answer_first() {
    let (port, accepted) = serve_plain();
    let pool = Arc::new(H2Pool::new(plain_dialer(port, Arc::new(AtomicUsize::new(0)))));
    let (p1, p2) = (pool.clone(), pool.clone());
    // warm the connection so both requests certainly share it
    ok(pool.fetch("example.test", 443, &get("/hello"), T));
    let a = std::thread::spawn(move || ok(p1.fetch("example.test", 443, &get("/a"), T)));
    let b = std::thread::spawn(move || ok(p2.fetch("example.test", 443, &get("/b"), T)));
    assert_eq!(b.join().unwrap().body, b"bee");
    assert_eq!(a.join().unwrap().body, b"ay");
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "one connection carried both");
}

#[test]
fn a_host_that_does_not_speak_h2_is_remembered_and_not_dialled_again() {
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = dials.clone();
    let pool = H2Pool::new(Arc::new(move |_: &str, _: u16| {
        d2.fetch_add(1, Ordering::SeqCst);
        Ok(Dialed::NotH2)
    }));
    for _ in 0..3 {
        assert!(matches!(pool.fetch("old.test", 443, &get("/"), T), Pooled::Http1));
    }
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    assert_eq!(pool.connections(), 0);
    // a host that cannot be reached is also handed to the HTTP/1.1 path, which reports the real error
    let down = H2Pool::new(Arc::new(|_: &str, _: u16| Err("refused".to_string())));
    assert!(matches!(down.fetch("x.test", 443, &get("/"), T), Pooled::Http1));
}

#[test]
fn after_a_goodbye_the_next_request_opens_a_new_connection() {
    let (port, accepted) = serve_plain();
    let dials = Arc::new(AtomicUsize::new(0));
    let pool = H2Pool::new(plain_dialer(port, dials.clone()));
    assert_eq!(ok(pool.fetch("example.test", 443, &get("/bye"), T)).body, b"bye");
    // the server closed after saying goodbye; the next request must not be lost
    assert_eq!(ok(pool.fetch("example.test", 443, &get("/hello"), T)).body, b"hello over h2");
    assert_eq!(dials.load(Ordering::SeqCst), 2);
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[test]
fn a_song_is_cut_at_the_cap_and_a_page_over_the_limit_fails() {
    let (port, _) = serve_plain();
    let pool = H2Pool::new(plain_dialer(port, Arc::new(AtomicUsize::new(0))));
    let r = ok(pool.fetch("example.test", 443, &get("/media"), T));
    assert_eq!(r.body.len(), 100_000, "only the start of the song is kept");
    let mut small = get("/big");
    small.max_body = 100_000;
    match pool.fetch("example.test", 443, &small, T) {
        Pooled::Done(Err(e)) => assert!(e.contains("larger"), "{e}"),
        _ => panic!("the limit should fail the request"),
    }
    // the connection is still good for the next request
    assert_eq!(ok(pool.fetch("example.test", 443, &get("/hello"), T)).status, 200);
}

#[test]
fn a_server_that_hangs_up_before_answering_gives_the_request_back_for_http1() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            drop(s);
        }
    });
    let pool = H2Pool::new(plain_dialer(port, Arc::new(AtomicUsize::new(0))));
    assert!(matches!(pool.fetch("example.test", 443, &get("/"), T), Pooled::Http1));
    let mut post = get("/");
    post.method = "POST".into();
    assert!(matches!(pool.fetch("example.test", 443, &post, T), Pooled::Done(Err(_))), "a POST is not sent twice behind the caller's back");
}

#[test]
fn urls_are_taken_apart_and_the_tls_hello_offers_h2_first() {
    assert_eq!(split_https("https://example.com/a/b?x=1#f"), Some(("example.com".into(), 443, "example.com".into(), "/a/b?x=1".into())));
    assert_eq!(split_https("https://example.com:8443"), Some(("example.com".into(), 8443, "example.com:8443".into(), "/".into())));
    assert_eq!(split_https("https://example.com?q=1"), Some(("example.com".into(), 443, "example.com".into(), "/?q=1".into())));
    assert_eq!(split_https("https://[::1]:9000/x"), Some(("::1".into(), 9000, "[::1]:9000".into(), "/x".into())));
    assert_eq!(split_https("http://example.com/"), None);
    assert_eq!(split_https("https://user@example.com/"), None);
    assert_eq!(split_https("https:///x"), None);
    assert_eq!(alpn_offer(), vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[cfg(feature = "https")]
mod over_tls {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use rustls::pki_types::CertificateDer;

    /// A TLS server on localhost that offers `alpn`; every accepted connection speaks HTTP/2 when it was chosen.
    fn serve_tls(alpn: &[&[u8]]) -> (u16, rustls::RootCertStore) {
        let mut cp = CertificateParams::new(vec![]).unwrap();
        cp.distinguished_name.push(DnType::CommonName, "Test Root");
        cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ck = KeyPair::generate().unwrap();
        let ca = cp.self_signed(&ck).unwrap();
        let mut lp = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        lp.distinguished_name.push(DnType::CommonName, "localhost");
        let lk = KeyPair::generate().unwrap();
        let leaf = lp.signed_by(&lk, &ca, &ck).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(leaf.der().to_vec())], rustls::pki_types::PrivateKeyDer::try_from(lk.serialize_der()).unwrap())
            .unwrap();
        cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    let mut c = rustls::ServerConnection::new(cfg).unwrap();
                    let mut s = s;
                    while c.is_handshaking() {
                        if c.complete_io(&mut s).is_err() {
                            return;
                        }
                    }
                    if c.alpn_protocol() == Some(b"h2".as_slice()) {
                        serve_conn(rustls::StreamOwned::new(c, s));
                    }
                });
            }
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(ca.der().to_vec())).unwrap();
        (port, roots)
    }

    #[test]
    fn the_real_dialer_negotiates_h2_with_alpn_and_carries_requests() {
        let (port, roots) = serve_tls(&[b"h2", b"http/1.1"]);
        let dial = tls_dialer(roots, Duration::from_secs(10));
        let pool = H2Pool::new(dial);
        let r = ok(pool.fetch("localhost", port, &get("/hello"), T));
        assert_eq!(r.body, b"hello over h2");
        assert_eq!(ok(pool.fetch("localhost", port, &get("/big"), T)).body.len(), 300_000);
        assert_eq!(pool.connections(), 1);
    }

    #[test]
    fn a_server_that_chooses_http11_or_does_not_negotiate_is_left_to_http11() {
        let (port, roots) = serve_tls(&[b"http/1.1"]);
        let pool = H2Pool::new(tls_dialer(roots, Duration::from_secs(10)));
        assert!(matches!(pool.fetch("localhost", port, &get("/hello"), T), Pooled::Http1));
        let (port, roots) = serve_tls(&[]);
        let pool = H2Pool::new(tls_dialer(roots, Duration::from_secs(10)));
        assert!(matches!(pool.fetch("localhost", port, &get("/hello"), T), Pooled::Http1));
    }

    #[test]
    fn a_certificate_that_is_not_trusted_is_not_used_for_h2() {
        let (port, _roots) = serve_tls(&[b"h2"]);
        let pool = H2Pool::new(tls_dialer(rustls::RootCertStore::empty(), Duration::from_secs(10)));
        assert!(matches!(pool.fetch("localhost", port, &get("/hello"), T), Pooled::Http1), "the handshake fails, nothing is sent");
        assert_eq!(pool.connections(), 0);
    }

    #[test]
    fn the_browser_transport_answers_over_h2_with_headers_and_gzip_left_to_the_caller() {
        use iyakki_web::Fetch;
        let (port, roots) = serve_tls(&[b"h2"]);
        let f = crate::https::BrowserFetch::default().with_h2_dialer(tls_dialer(roots, Duration::from_secs(10)));
        let url = iyakki_web::Url::parse(&format!("https://localhost:{port}/hello")).unwrap();
        let r = f.get(&url).unwrap();
        assert_eq!((r.status, r.body.as_slice(), r.content_type.as_str()), (200, &b"hello over h2"[..], "text/plain"));
        assert!(r.headers.iter().any(|(k, v)| k == "content-length" && v == "13"));
        // a video is cut to the start the way the HTTP/1.1 path does
        let m = f.get(&iyakki_web::Url::parse(&format!("https://localhost:{port}/media")).unwrap()).unwrap();
        assert_eq!(m.body.len(), 512 * 1024);
        assert_eq!(m.content_length, Some(900_000));
        assert_eq!(f.h2_connections(), 1);
    }
}

/// Against real servers on the internet (run by hand: `cargo test -p iyakki-proxy --lib live_h2 -- --ignored`).
#[cfg(feature = "https")]
#[test]
#[ignore = "needs the internet"]
fn live_h2_real_servers_answer_over_one_multiplexed_connection() {
    use iyakki_web::Fetch;
    let f = std::sync::Arc::new(crate::https::BrowserFetch::default());
    for host in ["www.google.com", "www.cloudflare.com", "www.wikipedia.org"] {
        let url = iyakki_web::Url::parse(&format!("https://{host}/")).unwrap();
        let r = f.get(&url).unwrap_or_else(|e| panic!("{host}: {e}"));
        assert!(r.status == 200 || (300..400).contains(&r.status), "{host}: {}", r.status);
        assert!(!r.body.is_empty() || r.location.is_some(), "{host}: empty");
        assert!(!r.headers.is_empty());
        eprintln!("{host}: status {} body {} bytes, {} h2 connection(s)", r.status, r.body.len(), f.h2_connections());
    }
    assert!(f.h2_connections() >= 1, "at least one of the hosts must have spoken h2");
    // several requests at once to one host share one connection
    let before = f.h2_connections();
    let hs: Vec<_> = ["/", "/robots.txt", "/sitemap.xml", "/favicon.ico"]
        .iter()
        .map(|p| {
            let f = f.clone();
            let url = iyakki_web::Url::parse(&format!("https://www.wikipedia.org{p}")).unwrap();
            std::thread::spawn(move || f.get(&url).map(|r| r.status))
        })
        .collect();
    for h in hs {
        let st = h.join().unwrap().unwrap();
        assert!(st < 500);
    }
    assert!(f.h2_connections() <= before + 1, "four requests, no more than one new connection");
}

#[test]
fn an_idle_connection_is_asked_with_a_ping_and_a_silent_one_is_dropped_before_a_request_is_lost_on_it() {
    let _one = PING_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    SILENT_PINGS.store(false, Ordering::SeqCst);
    let (port, accepted) = serve_plain();
    let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let c = H2Connection::open(Box::new(s)).unwrap();
    assert!(c.ping_check(Duration::from_secs(5)), "a live server answers");
    assert!(c.ping_check(Duration::from_secs(5)), "again, with another token");
    assert!(c.idle_for() < Duration::from_secs(5), "the answer counts as activity");
    // the same connection still carries a request
    let r = c.fetch(&get("/hello"), Instant::now() + T).unwrap();
    assert_eq!(r.body, b"hello over h2");
    // a server that never answers: the check fails inside its wait and the connection is broken, so the pool dials a new one
    SILENT_PINGS.store(true, Ordering::SeqCst);
    let t0 = Instant::now();
    assert!(!c.ping_check(Duration::from_millis(300)));
    assert!(t0.elapsed() < Duration::from_secs(3) && c.is_broken());
    assert!(!c.ping_check(Duration::from_millis(100)), "a broken connection is not alive");
    SILENT_PINGS.store(false, Ordering::SeqCst);
    assert!(accepted.load(Ordering::SeqCst) >= 1);
}

static PING_LOCK: Mutex<()> = Mutex::new(());

#[cfg(feature = "https")]
mod coalescing {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use rustls::pki_types::CertificateDer;
    use std::net::IpAddr;

    /// A TLS h2 server on localhost whose certificate names `names`; `misdirect` answers every request for `/` + that host with 421 is not needed: the test server answers by path.
    fn serve_names(names: &[&str]) -> (u16, rustls::RootCertStore, Arc<AtomicUsize>) {
        let mut cp = CertificateParams::new(vec![]).unwrap();
        cp.distinguished_name.push(DnType::CommonName, "Test Root");
        cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ck = KeyPair::generate().unwrap();
        let ca = cp.self_signed(&ck).unwrap();
        let mut lp = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        lp.distinguished_name.push(DnType::CommonName, names[0]);
        let lk = KeyPair::generate().unwrap();
        let leaf = lp.signed_by(&lk, &ca, &ck).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut cfg = rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(leaf.der().to_vec())], rustls::pki_types::PrivateKeyDer::try_from(lk.serialize_der()).unwrap()).unwrap();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let a2 = accepted.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                a2.fetch_add(1, Ordering::SeqCst);
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    let mut c = rustls::ServerConnection::new(cfg).unwrap();
                    let mut s = s;
                    while c.is_handshaking() {
                        if c.complete_io(&mut s).is_err() {
                            return;
                        }
                    }
                    serve_conn(rustls::StreamOwned::new(c, s));
                });
            }
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(ca.der().to_vec())).unwrap();
        (port, roots, accepted)
    }

    fn counting(inner: Dialer, n: Arc<AtomicUsize>) -> Dialer {
        Arc::new(move |h: &str, p: u16| {
            n.fetch_add(1, Ordering::SeqCst);
            inner(h, p)
        })
    }

    fn lo() -> Resolver {
        Arc::new(|_, _| vec![IpAddr::from([127, 0, 0, 1])])
    }

    #[test]
    fn a_second_host_with_the_same_address_and_a_certificate_that_names_it_rides_on_the_first_connection() {
        let (port, roots, _) = serve_names(&["localhost", "alias.test"]);
        let dials = Arc::new(AtomicUsize::new(0));
        let pool = H2Pool::new(counting(tls_dialer(roots, Duration::from_secs(10)), dials.clone())).with_coalescing(lo());
        assert_eq!(ok(pool.fetch("localhost", port, &get("/hello"), T)).body, b"hello over h2");
        let mut req = get("/hello");
        req.authority = format!("alias.test:{port}");
        assert_eq!(ok(pool.fetch("alias.test", port, &req, T)).body, b"hello over h2");
        assert_eq!((dials.load(Ordering::SeqCst), pool.connections(), pool.coalesced_hosts()), (1, 1, 1), "one dial, one connection, two hosts");
        // the next request for the second host uses the remembered connection
        assert_eq!(ok(pool.fetch("alias.test", port, &req, T)).body, b"hello over h2");
        assert_eq!(dials.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_host_the_certificate_does_not_name_or_that_has_another_address_gets_its_own_connection() {
        let (port, roots, _) = serve_names(&["localhost"]);
        let dials = Arc::new(AtomicUsize::new(0));
        let pool = H2Pool::new(counting(tls_dialer(roots.clone(), Duration::from_secs(10)), dials.clone())).with_coalescing(lo());
        ok(pool.fetch("localhost", port, &get("/hello"), T));
        // not named: the dial is made (and fails the certificate check here, which is the answer Http1), nothing is shared
        assert!(matches!(pool.fetch("other.test", port, &get("/hello"), T), Pooled::Http1));
        assert_eq!((dials.load(Ordering::SeqCst), pool.coalesced_hosts()), (2, 0));
        // named, but the host resolves elsewhere
        let (port, roots, _) = serve_names(&["localhost", "alias.test"]);
        let elsewhere: Resolver = Arc::new(|h, _| if h == "alias.test" { vec![IpAddr::from([192, 0, 2, 7])] } else { vec![IpAddr::from([127, 0, 0, 1])] });
        let dials = Arc::new(AtomicUsize::new(0));
        let pool = H2Pool::new(counting(tls_dialer(roots, Duration::from_secs(10)), dials.clone())).with_coalescing(elsewhere);
        ok(pool.fetch("localhost", port, &get("/hello"), T));
        let _ = pool.fetch("alias.test", port, &get("/hello"), T);
        assert_eq!(pool.coalesced_hosts(), 0);
        assert_eq!(dials.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_421_over_a_shared_connection_ends_the_sharing_for_that_host() {
        let (port, roots, _) = serve_names(&["localhost", "alias.test"]);
        let dials = Arc::new(AtomicUsize::new(0));
        let pool = H2Pool::new(counting(tls_dialer(roots, Duration::from_secs(10)), dials.clone())).with_coalescing(lo());
        ok(pool.fetch("localhost", port, &get("/hello"), T));
        let mut req = get("/misdirected");
        req.authority = format!("alias.test:{port}");
        // the shared connection says 421: the host is sent the other way (its own dial; the name does not resolve here, so HTTP/1.1)
        assert!(matches!(pool.fetch("alias.test", port, &req, T), Pooled::Http1));
        assert_eq!(pool.coalesced_hosts(), 0);
        let before = dials.load(Ordering::SeqCst);
        assert!(matches!(pool.fetch("alias.test", port, &get("/hello"), T), Pooled::Http1));
        assert!(dials.load(Ordering::SeqCst) > before, "it dials for itself and is not shared again");
    }

    #[test]
    fn without_coalescing_every_host_dials_for_itself() {
        let (port, roots, _) = serve_names(&["localhost", "alias.test"]);
        let dials = Arc::new(AtomicUsize::new(0));
        let pool = H2Pool::new(counting(tls_dialer(roots, Duration::from_secs(10)), dials.clone()));
        ok(pool.fetch("localhost", port, &get("/hello"), T));
        let _ = pool.fetch("alias.test", port, &get("/hello"), T);
        assert_eq!(dials.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_dialer_through_a_proxy_asks_for_a_tunnel_and_then_speaks_tls_and_h2_end_to_end() {
        let (port, roots, _) = serve_names(&["localhost"]);
        // a tiny proxy: reads the CONNECT head, answers 200 and pipes both ways to the real server
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let pport = l.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(String::new()));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if c.read(&mut b).unwrap_or(0) == 0 {
                    return;
                }
                head.push(b[0]);
            }
            *s2.lock().unwrap() = String::from_utf8_lossy(&head).into_owned();
            let up = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let (mut c2, mut up2) = (c.try_clone().unwrap(), up.try_clone().unwrap());
            let t = std::thread::spawn(move || {
                let _ = std::io::copy(&mut c2, &mut up2);
            });
            let (mut c3, mut up3) = (c, up);
            let _ = std::io::copy(&mut up3, &mut c3);
            let _ = t.join();
        });
        let dial = tls_dialer_via(roots, Duration::from_secs(10), Some(Proxy { host: "127.0.0.1".into(), port: pport, creds: Some(("ada".into(), "pw".into())) }));
        let pool = H2Pool::new(dial);
        assert_eq!(ok(pool.fetch("localhost", port, &get("/hello"), T)).body, b"hello over h2");
        let head = seen.lock().unwrap().clone();
        assert!(head.starts_with(&format!("CONNECT localhost:{port} HTTP/1.1")) && head.contains("Proxy-Authorization: Basic YWRhOnB3"), "{head}");
    }
}
