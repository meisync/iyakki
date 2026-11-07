//! `iyakki-connect` against a fake tenant over real TLS on the loopback: each refusal (changed pin, bad certificate, wrong password)
//! and the pin file's rules (mode 600, trust on first use, nothing replaced silently).

use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_connect::{pins::Store, run, Ctx, Fail};
use iyakki_link::cloudclient::pin_of;

struct Fake {
    addr: String,
    der: Vec<u8>,
    /// Requests that reached the application layer (a request line was read).
    requests: Arc<AtomicUsize>,
}

fn cert(not_after_year: Option<i32>) -> (Vec<u8>, rustls::ServerConfig) {
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    if let Some(y) = not_after_year {
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(y, 1, 1);
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let c = params.self_signed(&key).unwrap();
    let der = c.der().to_vec();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![c.der().clone()], rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap())
        .unwrap();
    (der, cfg)
}

/// A tenant that answers every login with 401.
fn tenant(expired: bool) -> Fake {
    let (der, cfg) = cert(expired.then_some(2001));
    let cfg = Arc::new(cfg);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    let requests = Arc::new(AtomicUsize::new(0));
    let n = requests.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (cfg, n) = (cfg.clone(), n.clone());
            std::thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(cfg).unwrap(), s);
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(k) => buf.extend_from_slice(&chunk[..k]),
                    }
                }
                n.fetch_add(1, Ordering::SeqCst);
                let body = r#"{"error":"wrong account or password"}"#;
                let _ = write!(tls, "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = tls.flush();
            });
        }
    });
    Fake { addr, der, requests }
}

/// A server that does not speak TLS at all.
fn garbage() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\nthis is not TLS");
        }
    });
    addr
}

fn ctx(dir: &std::path::Path) -> Ctx {
    Ctx { store: Store::new(dir), timeout: Duration::from_secs(5), interactive: false }
}

fn go(c: &Ctx, input: &str, args: &[&str]) -> (Result<(), Fail>, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let r = run(&a, c, &mut Cursor::new(input.to_string()), &mut out);
    (r, String::from_utf8(out).unwrap())
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn probe_shows_names_validity_and_fingerprint() {
    let t = tenant(false);
    let d = dir("probe");
    let (r, out) = go(&ctx(&d), "", &["probe", &t.addr]);
    r.unwrap();
    assert!(out.contains("localhost") && out.contains("valid from:") && out.contains("valid to:"), "{out}");
    assert!(out.contains(&pin_of(&t.der).display()), "{out}");
    assert!(!d.exists(), "probe stores nothing");
    assert_eq!(t.requests.load(Ordering::SeqCst), 0, "probe sends no application data");
}

#[test]
fn an_expired_certificate_is_shown_with_a_warning() {
    let t = tenant(true);
    let (r, out) = go(&ctx(&dir("expired")), "", &["probe", &t.addr]);
    r.unwrap();
    assert!(out.contains("WARNING: expired"), "{out}");
}

#[test]
fn pin_is_trust_on_first_use_and_the_file_is_private() {
    let t = tenant(false);
    let d = dir("tofu");
    let c = ctx(&d);
    let (r, _) = go(&c, "", &["pin", &t.addr]);
    assert!(matches!(r, Err(Fail::Error(m)) if m.contains("--yes")), "without a terminal or --yes nothing is trusted");
    assert!(c.store.get(&t.addr).unwrap().is_none());
    go(&c, "", &["pin", &t.addr, "--yes"]).0.unwrap();
    assert_eq!(c.store.get(&t.addr).unwrap(), Some(pin_of(&t.der)));
    assert_eq!(std::fs::metadata(d.join("pins")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
    let (r, out) = go(&c, "", &["pin", &t.addr, "--yes"]);
    r.unwrap();
    assert!(out.contains("already pinned"), "{out}");
    let (_, list) = go(&c, "", &["pins"]);
    assert!(list.contains(&pin_of(&t.der).display()));
    go(&c, "", &["unpin", &t.addr]).0.unwrap();
    assert!(c.store.get(&t.addr).unwrap().is_none());
    let mut ic = ctx(&d);
    ic.interactive = true;
    go(&ic, "n\n", &["pin", &t.addr]).0.unwrap_err();
    go(&ic, "y\n", &["pin", &t.addr]).0.unwrap();
    assert!(ic.store.get(&t.addr).unwrap().is_some());
}

#[test]
fn a_changed_certificate_is_a_hard_stop_and_the_pin_is_kept() {
    let (old, new) = (tenant(false), tenant(false));
    let d = dir("changed");
    let c = ctx(&d);
    // the address now shows `new`, but the pin on file is `old`'s certificate
    c.store.set(&new.addr, pin_of(&old.der)).unwrap();
    let (r, _) = go(&c, "", &["pin", &new.addr, "--yes"]);
    let Err(f @ Fail::PinChanged(_)) = r else { panic!("{r:?}") };
    assert_eq!(f.status(), 2);
    assert!(f.text().contains("changed") && f.text().contains(&pin_of(&old.der).display()) && f.text().contains(&pin_of(&new.der).display()));
    assert_eq!(c.store.get(&new.addr).unwrap(), Some(pin_of(&old.der)), "never replaced silently");
    let (r, _) = go(&c, "hunter2\n", &["login", &new.addr, "alice"]);
    assert!(matches!(r, Err(Fail::PinChanged(_))), "{r:?}");
    assert_eq!(new.requests.load(Ordering::SeqCst), 0, "the password was not sent");
}

#[test]
fn a_bad_certificate_or_a_server_without_tls_is_refused_and_nothing_is_pinned() {
    let g = garbage();
    let d = dir("bad");
    let c = ctx(&d);
    for args in [vec!["probe", g.as_str()], vec!["pin", g.as_str(), "--yes"]] {
        let (r, _) = go(&c, "", &args);
        assert!(matches!(r, Err(Fail::Error(m)) if m.contains("secure connection")), "{args:?}");
    }
    assert!(c.store.get(&g).unwrap().is_none());
    // nothing listens
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    drop(l);
    assert!(go(&c, "", &["probe", &dead]).0.is_err());
    // login needs a pin first
    let (r, _) = go(&c, "pw\n", &["login", &g, "alice"]);
    assert!(matches!(r, Err(Fail::Error(m)) if m.contains("not pinned")));
}

#[test]
fn a_wrong_password_is_refused_with_the_servers_words() {
    let t = tenant(false);
    let c = ctx(&dir("password"));
    go(&c, "", &["pin", &t.addr, "--yes"]).0.unwrap();
    let (r, _) = go(&c, "wrong\n", &["login", &t.addr, "alice"]);
    let Err(Fail::Error(m)) = r else { panic!("{r:?}") };
    assert!(m.contains("refused") && m.contains("wrong account or password"), "{m}");
    assert_eq!(t.requests.load(Ordering::SeqCst), 1);
    let (r, _) = go(&c, "\n", &["login", &t.addr, "alice"]);
    assert!(matches!(r, Err(Fail::Error(m)) if m.contains("no password")), "an empty password is not even sent");
    assert_eq!(t.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn bad_arguments_print_usage() {
    let c = ctx(&dir("usage"));
    assert!(matches!(go(&c, "", &[]).0, Err(Fail::Error(m)) if m.contains("usage")));
    assert!(matches!(go(&c, "", &["probe"]).0, Err(Fail::Error(m)) if m.contains("usage")));
    assert!(matches!(go(&c, "", &["login", "h", "u", "extra-password"]).0, Err(Fail::Error(m)) if m.contains("usage")), "a password in the arguments is not accepted");
}

#[test]
fn dates_are_written_in_utc() {
    assert_eq!(iyakki_connect::date(0), "1970-01-01 00:00 UTC");
    assert_eq!(iyakki_connect::date(1_798_761_600 + 3660), "2027-01-01 01:01 UTC");
}
