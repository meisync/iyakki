//! The store server over HTTPS (F-BSHARE-05): a real listener on 127.0.0.1, the in-house rustls identity, a rustls client that trusts
//! only the server's own certificate. Nothing outside a temporary folder.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::server::*;
use crate::pkg::{new_signing_key, signing_key_hex};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};

const READ: &str = "read-token-for-tls-tests-0123456789";
const ADMIN: &str = "admin-token-for-tls-tests-012345678";

struct Up {
    port: u16,
    stop: Arc<AtomicBool>,
    dir: std::path::PathBuf,
    cert_pem: Vec<u8>,
    fingerprint: String,
}

impl Drop for Up {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(tag: &str) -> Up {
    let dir = std::env::temp_dir().join(format!("iyakki-store-tls-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let id = crate::webservice::tls::self_signed(&dir.join("tls"), &["127.0.0.1".to_string()]).unwrap();
    let mut cfg = Config::new(&dir.join("store"));
    cfg.read_token_hash = Some(hash_token(READ));
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    cfg.disk_margin = 1 << 20;
    cfg.reads_per_min = 10_000;
    let srv = Arc::new(StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, cfg2) = (stop.clone(), id.config.clone());
    std::thread::spawn(move || super::server_http::serve_tls(srv, l, s2, cfg2));
    Up { port, stop, cert_pem: std::fs::read(dir.join("tls/cert.pem")).unwrap(), fingerprint: id.fingerprint, dir }
}

fn https(port: u16, trust_pem: &[u8], request: &str) -> Result<String, String> {
    https_bytes(port, trust_pem, request.as_bytes()).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// One HTTPS request that trusts exactly `trust_pem`; returns the whole answer.
fn https_bytes(port: u16, trust_pem: &[u8], request: &[u8]) -> Result<Vec<u8>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(trust_pem) {
        roots.add(c.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.with_root_certificates(roots).with_no_client_auth();
    let conn = rustls::ClientConnection::new(Arc::new(cfg), ServerName::try_from("127.0.0.1").unwrap().to_owned()).map_err(|e| e.to_string())?;
    let tcp = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut t = rustls::StreamOwned::new(conn, tcp);
    t.write_all(request).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let _ = t.read_to_end(&mut out);
    if out.is_empty() {
        return Err("no answer (the handshake failed)".into());
    }
    Ok(out)
}

#[test]
fn the_store_answers_over_https_to_a_client_that_trusts_its_certificate_and_reads_still_need_the_token() {
    let u = start("ok");
    assert!(u.fingerprint.len() == 95, "the fingerprint is printed as AA:BB:...");
    let ok = https(u.port, &u.cert_pem, "GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("ok\n"), "{ok}");
    let no = https(u.port, &u.cert_pem, "GET /v1/components.json HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    assert!(no.starts_with("HTTP/1.1 401"), "{no}");
    let yes = https(u.port, &u.cert_pem, &format!("GET /v1/components.json HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {READ}\r\nConnection: close\r\n\r\n")).unwrap();
    assert!(!yes.starts_with("HTTP/1.1 401"), "{yes}");
}

#[test]
fn another_certificate_is_not_trusted_and_plain_http_gets_nothing_useful() {
    let u = start("other");
    let stranger = start("stranger");
    assert!(https(u.port, &stranger.cert_pem, "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").is_err(), "a client trusting another certificate refuses this server");
    let mut s = TcpStream::connect(("127.0.0.1", u.port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    assert!(!String::from_utf8_lossy(&out).contains("200 OK"), "plain HTTP on the TLS port is not served");
}

#[test]
fn a_blob_goes_up_and_comes_back_over_https_byte_for_byte() {
    use sha2::{Digest, Sha256};
    let u = start("blob");
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    let sha = crate::pkg::hex(&Sha256::digest(&data));
    let mut put = format!("PUT /v1/blobs/{sha}?component=build HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {ADMIN}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len()).into_bytes();
    put.extend_from_slice(&data);
    let ans = https_bytes(u.port, &u.cert_pem, &put).unwrap();
    let head = String::from_utf8_lossy(&ans[..ans.len().min(40)]).into_owned();
    assert!(head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.1 201"), "{head}");
    let get = https_bytes(u.port, &u.cert_pem, format!("GET /v1/blobs/{sha} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {READ}\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
    let split = get.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert!(String::from_utf8_lossy(&get[..12]).starts_with("HTTP/1.1 200"), "{}", String::from_utf8_lossy(&get[..60]));
    assert_eq!(&get[split..], &data[..], "the same bytes come back through TLS");
    let no = https(u.port, &u.cert_pem, "PUT /v1/blobs/0000000000000000000000000000000000000000000000000000000000000000 HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nabc").unwrap();
    assert!(no.starts_with("HTTP/1.1 401"), "an upload without a publish token is refused over TLS too: {no}");
}
