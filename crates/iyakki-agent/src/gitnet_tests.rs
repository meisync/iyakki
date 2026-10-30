//! The client against real local sockets (no fake runner, no curl).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use super::*;

const TOK: &str = "tok_FAKEFAKEFAKEFAKE9876";

fn req(url: &str) -> Req {
    Req { method: "GET".into(), url: url.into(), token: Some(TOK.into()), style: Style::Token, body: None, cert: None, accept: None, max_bytes: 0, timeout_s: 5 }
}

/// Serves one connection with `answer(request_text)`; returns the address and the request seen.
fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, std::sync::mpsc::Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut buf = vec![0u8; 8192];
        let n = s.read(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = s.write_all(&answer(&text));
        let _ = tx.send(text);
    });
    (format!("http://127.0.0.1:{}", addr.port()), rx)
}

fn ok(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Thing: Yes\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn a_get_sends_the_token_in_the_header_only_and_reads_status_headers_and_body() {
    let (base, rx) = serve(|_| ok(r#"{"login":"octo"}"#));
    let r = NetGit::default().send(&req(&format!("{base}/api/v1/user?x=1"))).unwrap();
    assert_eq!((r.status, r.body.as_str(), r.header("x-thing")), (200, r#"{"login":"octo"}"#, Some("Yes")));
    let seen = rx.recv().unwrap();
    assert!(seen.starts_with("GET /api/v1/user?x=1 HTTP/1.1\r\n"), "{seen}");
    assert!(seen.contains(&format!("Authorization: token {TOK}\r\n")) && seen.contains("Accept: application/json"));
}

#[test]
fn a_bearer_post_with_a_body_and_a_chunked_answer() {
    let (base, rx) = serve(|_| b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n".to_vec());
    let mut q = req(&format!("{base}/x"));
    q.method = "POST".into();
    q.style = Style::Bearer;
    q.body = Some(r#"{"a":1}"#.into());
    let r = NetGit::default().send(&q).unwrap();
    assert_eq!((r.status, r.body.as_str()), (201, "hello world"));
    let seen = rx.recv().unwrap();
    assert!(seen.contains(&format!("Authorization: Bearer {TOK}")) && seen.contains("Content-Length: 7") && seen.ends_with(r#"{"a":1}"#), "{seen}");
}

#[test]
fn an_answer_over_the_limit_is_too_big_and_a_closed_port_is_unreachable_without_the_token() {
    let (base, _rx) = serve(|_| ok(&"x".repeat(5000)));
    let mut q = req(&format!("{base}/big"));
    q.max_bytes = 1000;
    assert_eq!(NetGit::default().send(&q), Err(NetErr::Other(TOO_BIG.into())));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    match NetGit::default().send(&req(&format!("http://127.0.0.1:{port}/"))) {
        Err(NetErr::Unreachable(m)) => assert!(!m.contains(TOK), "{m}"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn a_silent_server_times_out_and_odd_addresses_are_refused() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let mut q = req(&format!("http://127.0.0.1:{port}/"));
    q.timeout_s = 1;
    let t = std::time::Instant::now();
    assert!(matches!(NetGit::default().send(&q), Err(NetErr::Unreachable(_))));
    assert!(t.elapsed() < Duration::from_secs(5));
    for bad in ["ftp://x/", "http://user:pw@127.0.0.1/", "http://127.0.0.1/a b"] {
        assert!(matches!(NetGit::default().send(&req(bad)), Err(NetErr::Other(_))), "{bad}");
    }
}

// ---- https with a certificate made here

fn tls_server() -> (u16, Vec<u8>) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let der = ck.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(der.clone())], key).unwrap();
    let cfg = Arc::new(cfg);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten().take(6) {
            let mut conn = rustls::ServerConnection::new(cfg.clone()).unwrap();
            let mut s = s;
            let mut tls = rustls::Stream::new(&mut conn, &mut s);
            let mut b = [0u8; 4096];
            if tls.read(&mut b).is_ok_and(|n| n > 0) {
                let _ = tls.write_all(&ok("{}"));
                let _ = tls.flush();
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    (port, der)
}

#[test]
fn https_with_an_unknown_certificate_is_refused_the_certificate_can_be_read_and_the_pinned_one_works() {
    let (port, der) = tls_server();
    let url = format!("https://localhost:{port}/api/v1/user");
    let h = NetGit::default();
    // not trusted by the system: a certificate error (or no CA list at all), never an answer
    assert!(matches!(h.send(&req(&url)), Err(NetErr::Tls(_))));
    // what the server shows, for the person to compare
    let c = h.peer_cert(&format!("https://localhost:{port}")).unwrap();
    let want = cert_info(&{
        let b = crate::wsproto::base64(&der);
        format!("-----BEGIN CERTIFICATE-----\n{b}\n-----END CERTIFICATE-----\n")
    })
    .unwrap();
    assert_eq!(c.fingerprint, want.fingerprint);
    // pinned: the same file works, another certificate does not
    let dir = std::env::temp_dir().join(format!("gitnet-{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ok.pem"), &c.pem).unwrap();
    let mut q = req(&url);
    q.cert = Some(dir.join("ok.pem"));
    assert_eq!(h.send(&q).unwrap().status, 200);
    let other = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    std::fs::write(dir.join("other.pem"), other.cert.pem()).unwrap();
    q.cert = Some(dir.join("other.pem"));
    assert!(matches!(h.send(&q), Err(NetErr::Tls(_))));
    let _ = std::fs::remove_dir_all(&dir);
}
