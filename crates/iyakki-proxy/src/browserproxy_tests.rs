//! The browser proxy setting (F-BROX-22): the text a person types, and the HTTP/1.1 path through a `CONNECT` proxy.
use crate::https::{parse_proxy, BrowserFetch};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[test]
fn the_proxy_text_is_host_port_with_an_optional_sign_in_and_anything_else_is_said_in_words() {
    let p = parse_proxy("proxy.corp.example:3128").unwrap();
    assert_eq!((p.host.as_str(), p.port, p.creds.clone()), ("proxy.corp.example", 3128, None));
    let p = parse_proxy("  http://ada:p@ss:w0rd@10.0.0.5:8080 ").unwrap();
    assert_eq!((p.host.as_str(), p.port, p.creds), ("10.0.0.5", 8080, Some(("ada".to_string(), "p@ss:w0rd".to_string()))));
    for bad in ["", "host", "host:0", "host:99999", "host:abc", "socks5://h:1", "https://h:1", "h:1/path", "a b:1", "ada@h:1", ":1", ":@h:1", "h\u{7}:1", "host:1\nHost: x"] {
        assert!(parse_proxy(bad).is_err(), "{bad:?}");
    }
    assert!(parse_proxy(&format!("{}:1", "a".repeat(400))).is_err());
}

/// A proxy that answers one `CONNECT` and pipes to `to`; the head it read is kept.
fn proxy_to(to: u16) -> (u16, Arc<Mutex<String>>, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let s2 = seen.clone();
    let h = std::thread::spawn(move || {
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
        let up = std::net::TcpStream::connect(("127.0.0.1", to)).unwrap();
        c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
        let (mut c2, mut up2) = (c.try_clone().unwrap(), up.try_clone().unwrap());
        let t = std::thread::spawn(move || {
            let _ = std::io::copy(&mut c2, &mut up2);
        });
        let (mut c3, mut up3) = (c, up);
        let _ = std::io::copy(&mut up3, &mut c3);
        let _ = t.join();
    });
    (port, seen, h)
}

#[test]
fn a_plain_page_goes_through_the_proxy_with_the_sign_in() {
    use iyakki_web::Fetch;
    // the page server: the listener stays bound until the test ends
    let page = TcpListener::bind("127.0.0.1:0").unwrap();
    let pport = page.local_addr().unwrap().port();
    let srv = std::thread::spawn(move || {
        let (mut c, _) = page.accept().unwrap();
        let mut buf = [0u8; 2048];
        let _ = c.read(&mut buf).unwrap();
        c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: close\r\n\r\nvia px").unwrap();
    });
    let (port, seen, px) = proxy_to(pport);
    let f = BrowserFetch::default().with_proxy(parse_proxy(&format!("ada:pw@127.0.0.1:{port}")).unwrap()).unwrap();
    let r = f.get(&iyakki_web::Url::parse(&format!("http://127.0.0.1:{pport}/x")).unwrap()).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"via px"[..]));
    let head = seen.lock().unwrap().clone();
    assert!(head.starts_with(&format!("CONNECT 127.0.0.1:{pport} HTTP/1.1")) && head.contains("Proxy-Authorization: Basic YWRhOnB3"), "{head}");
    srv.join().unwrap();
    px.join().unwrap();
}

#[test]
fn a_proxy_that_refuses_is_an_error_not_a_direct_connection() {
    use iyakki_web::Fetch;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = c.read(&mut buf);
        let _ = c.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n");
    });
    // the target is a bound but silent listener: if the request went around the proxy it would hang or succeed, not fail with the proxy's words
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let tport = target.local_addr().unwrap().port();
    let f = BrowserFetch::default().with_proxy(parse_proxy(&format!("127.0.0.1:{port}")).unwrap()).unwrap();
    assert!(f.get(&iyakki_web::Url::parse(&format!("http://127.0.0.1:{tport}/")).unwrap()).is_err());
    h.join().unwrap();
    drop(target);
}
