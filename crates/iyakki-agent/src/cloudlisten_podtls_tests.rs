//! Tests of the TLS link between the listener and the pod (`cloudlisten_podtls.rs`). Real TLS on both ends over loopback sockets: the
//! listener (`Listener`) dials a `PodServer` that stands in front of a plain test application; certificates come from a memory store
//! behind the same `SecretReader` the vault source uses (the real vault service is not involved).

use super::*;
use crate::cloudlisten::podtls as pt;
use crate::cloudlisten::podtls::{Leaf, Material, PodOptions, PodServer, Rotate, SecretNames, SecretWriter};
use crate::cloudlisten::vault::SecretReader;
use std::collections::HashMap;

const NAME: &str = "app";
const HOST: &str = "app.pod.test";
const FAR_PAST: (pt::Ymd, pt::Ymd) = ((2000, 1, 1), (2001, 1, 1));

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[derive(Clone, Default)]
struct Mem(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl Mem {
    fn set(&self, n: &str, v: &str) {
        self.0.lock().unwrap().insert(n.into(), v.as_bytes().to_vec());
    }
    fn text(&self, n: &str) -> String {
        String::from_utf8(self.0.lock().unwrap().get(n).cloned().unwrap()).unwrap()
    }
    fn copy(&self) -> Mem {
        Mem(Arc::new(Mutex::new(self.0.lock().unwrap().clone())))
    }
}

impl SecretReader for Mem {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().get(name).cloned().ok_or_else(|| "404: no such secret".to_string())
    }
}

impl SecretWriter for Mem {
    fn write(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(name.into(), value.to_vec());
        Ok(())
    }
}

/// A reader that remembers what it was asked for.
struct Spy(Mem, Arc<Mutex<Vec<String>>>);

impl SecretReader for Spy {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.1.lock().unwrap().push(name.into());
        self.0.read(name)
    }
}

fn provisioned() -> Mem {
    let m = Mem::default();
    pt::provision(&m, NAME, unix_now(), pt::LEAF_DAYS).unwrap();
    m
}

fn names() -> SecretNames {
    SecretNames::of(NAME)
}

fn source(m: &Mem) -> Arc<vault::VaultCerts> {
    Arc::new(vault::VaultCerts::new(Box::new(m.clone())))
}

fn server_material(m: &Mem) -> Material {
    vault::VaultCerts::new(Box::new(m.clone())).load_pod_server(NAME).unwrap()
}

/// A plain application behind a pod sidecar.
struct Pod {
    server: PodServer,
    app: Up,
    log: Log,
}

fn pod(m: &Mem) -> Pod {
    let app = upstream("pod");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let mut o = PodOptions::new(app.port);
    o.log = Arc::new(move |s| l2.lock().unwrap().push(s.to_string()));
    o.handshake_timeout = Duration::from_secs(3);
    let server = PodServer::start("127.0.0.1:0".parse().unwrap(), &server_material(m), o).unwrap();
    Pod { server, app, log }
}

/// The listener with one plain route to `target` that wants TLS to the pod. Returns it and the port it listens on.
fn listener(m: &Mem, target: u16) -> (Listener, u16, Log) {
    let (mut o, log) = opts();
    o.pod_tls.insert(HOST.into(), NAME.into());
    o.connect_timeout = Duration::from_millis(700);
    let p = free_port();
    let l = Listener::start(table(vec![route(HOST, "/", target, p)]), source(m), Arc::new(NoGate), o).unwrap();
    (l, p, log)
}

fn ask(p: u16, path: &str) -> Resp {
    http(p, &get(HOST, path))
}

fn wait_for(log: &Log, needle: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        if log.lock().unwrap().iter().any(|l| l.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn pem_ders(p: &str) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_slice_iter(p.as_bytes()).collect::<Result<_, _>>().unwrap()
}

fn key_der(p: &str) -> rustls::pki_types::PrivateKeyDer<'static> {
    rustls::pki_types::PrivateKeyDer::from_pem_slice(p.as_bytes()).unwrap()
}

/// What a client other than the listener does to the pod: TLS with (or without) a client certificate, then a request. Returns what came
/// back; an empty string with an error is "the pod gave nothing".
fn raw_client(pod: &Pod, ca_pem: &str, name: &str, id: Option<(&str, &str)>) -> Result<String, String> {
    let mut roots = rustls::RootCertStore::empty();
    for c in pem_ders(ca_pem) {
        roots.add(c).unwrap();
    }
    let prov = Arc::new(rustls::crypto::ring::default_provider());
    let b = rustls::ClientConfig::builder_with_provider(prov).with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12]).unwrap().with_root_certificates(roots);
    let cfg = match id {
        Some((c, k)) => b.with_client_auth_cert(pem_ders(c), key_der(k)).unwrap(),
        None => b.with_no_client_auth(),
    };
    let tcp = TcpStream::connect(pod.server.addr()).map_err(|e| e.to_string())?;
    timeout(&tcp);
    let conn = rustls::ClientConnection::new(Arc::new(cfg), ServerName::try_from(pt::server_name(name)).unwrap()).map_err(|e| e.to_string())?;
    let mut s = rustls::StreamOwned::new(conn, tcp);
    let sent = s.write_all(b"GET /secret HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").and_then(|_| s.flush());
    let mut got = Vec::new();
    let read = s.read_to_end(&mut got);
    match (sent, read) {
        (_, Ok(_)) if !got.is_empty() => Ok(String::from_utf8_lossy(&got).to_string()),
        (Err(e), _) | (_, Err(e)) => Err(e.to_string()),
        _ => Err("closed without an answer".into()),
    }
}

// ------------------------------------------------------------------------------------------------------------ the good path

#[test]
fn a_request_reaches_the_pod_over_mutual_tls_with_every_kind_of_body() {
    let m = provisioned();
    let p = pod(&m);
    let (_l, port, llog) = listener(&m, p.server.addr().port());
    let r = ask(port, "/hello");
    assert_eq!((r.0, text(&r).as_str()), (200, "name=pod path=/hello"));
    let seen = p.app.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("X-Forwarded-For: 127.0.0.1"), "{}", seen[0]);
    // a chunked answer, a 3 MB answer and a 1 MB request body
    assert_eq!(text(&ask(port, "/chunked")), "alphabetagamma");
    let big = ask(port, "/big");
    assert_eq!(big.2.len(), 3_000_000);
    assert!(big.2.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8), "3 MB through two TLS hops arrive unchanged");
    let body = vec![7u8; 1_000_000];
    let mut s = connect(port);
    s.write_all(format!("POST /echo HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).unwrap();
    s.write_all(&body).unwrap();
    assert_eq!(text(&read_response(&mut BufReader::new(s))), format!("len=1000000 sum={}", 7_000_000));
    assert!(llog.lock().unwrap().iter().any(|l| l.contains("pod TLS app.pod.test: client certificate valid")), "{:?}", llog.lock().unwrap());
}

#[test]
fn an_upgrade_runs_through_the_tls_link_as_a_two_way_tunnel() {
    let m = provisioned();
    let p = pod(&m);
    let (_l, port, _) = listener(&m, p.server.addr().port());
    let mut s = connect(port);
    s.write_all(format!("GET /ws HTTP/1.1\r\nHost: {HOST}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: x\r\n\r\n").as_bytes()).unwrap();
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
    assert_eq!(&got, b"welcome");
    let big: Vec<u8> = (0..100_000usize).map(|i| (i % 249) as u8).collect();
    let (mut w, big2) = (s.try_clone().unwrap(), big.clone());
    let t = std::thread::spawn(move || w.write_all(&big2).unwrap());
    let mut back = vec![0u8; big.len()];
    r.read_exact(&mut back).unwrap();
    t.join().unwrap();
    assert_eq!(back, big);
}

// ------------------------------------------------------------------------------------------------------------ what is refused

#[test]
fn a_listener_certificate_of_another_ca_is_refused_by_the_pod_and_the_application_sees_nothing() {
    let m = provisioned();
    let p = pod(&m);
    let evil = pt::create_ca(NAME, unix_now(), 30).unwrap();
    let w = pt::issue(&evil, NAME, Leaf::Client, unix_now(), 30).unwrap();
    let wrong = m.copy();
    wrong.set(&names().client_chain, &w.0);
    wrong.set(&names().client_key, &w.1);
    let (_l, port, _) = listener(&wrong, p.server.addr().port());
    assert_eq!(ask(port, "/x").0, 502);
    assert!(wait_for(&p.log, "refused"), "{:?}", p.log.lock().unwrap());
    assert!(p.app.seen.lock().unwrap().is_empty(), "the application never saw a request");
}

#[test]
fn a_client_with_no_certificate_or_a_foreign_one_never_reaches_the_application() {
    let m = provisioned();
    let p = pod(&m);
    let ca = m.text(&names().ca);
    let none = raw_client(&p, &ca, NAME, None);
    assert!(none.is_err(), "no client certificate: {none:?}");
    let evil = pt::create_ca(NAME, unix_now(), 30).unwrap();
    let (c, k) = pt::issue(&evil, NAME, Leaf::Client, unix_now(), 30).unwrap();
    let foreign = raw_client(&p, &ca, NAME, Some((&c, &k)));
    assert!(foreign.is_err(), "a certificate of another CA: {foreign:?}");
    assert!(p.app.seen.lock().unwrap().is_empty());
    // the right certificate does get through (the raw client is a fair test)
    let ok = raw_client(&p, &ca, NAME, Some((&m.text(&names().client_chain), &m.text(&names().client_key)))).unwrap();
    assert!(ok.contains("path=/secret"), "{ok}");
}

#[test]
fn an_expired_client_certificate_is_refused_by_the_pod_and_by_the_listener_itself() {
    let m = provisioned();
    let p = pod(&m);
    let ca = pt::RouteCa { cert_pem: m.text(&names().ca), key_pem: m.text(&names().ca_key) };
    let (c, k) = pt::issue_for(&ca, NAME, Leaf::Client, &pt::server_name(NAME), FAR_PAST.0, FAR_PAST.1).unwrap();
    let r = raw_client(&p, &ca.cert_pem, NAME, Some((&c, &k)));
    assert!(r.is_err(), "the pod refuses an expired client certificate: {r:?}");
    assert!(p.app.seen.lock().unwrap().is_empty());
    // the listener does not even load its own expired certificate: fail closed, nothing is sent in plain
    let old = m.copy();
    old.set(&names().client_chain, &c);
    old.set(&names().client_key, &k);
    let (_l, port, llog) = listener(&old, p.server.addr().port());
    assert_eq!(ask(port, "/x").0, 502);
    assert!(llog.lock().unwrap().iter().any(|l| l.contains("has expired") && l.contains("not sent in plain")), "{:?}", llog.lock().unwrap());
    assert!(p.app.seen.lock().unwrap().is_empty());
}

#[test]
fn a_pod_with_an_expired_certificate_is_refused_by_the_listener_and_cannot_even_start_as_ours() {
    let m = provisioned();
    let ca = pt::RouteCa { cert_pem: m.text(&names().ca), key_pem: m.text(&names().ca_key) };
    let (c, k) = pt::issue_for(&ca, NAME, Leaf::Server, &pt::server_name(NAME), FAR_PAST.0, FAR_PAST.1).unwrap();
    let mat = Material { ca: ca.cert_pem.clone(), chain: c.clone(), key: k.clone() };
    assert!(PodServer::start("127.0.0.1:0".parse().unwrap(), &mat, PodOptions::new(1)).unwrap_err().contains("expired"));
    // a pod of someone else, with the right CA but an expired certificate, built with rustls directly
    let seen = fake_tls_pod(&c, &k, &ca.cert_pem);
    let (_l, port, llog) = listener(&m, seen.0);
    assert_eq!(ask(port, "/x").0, 502);
    assert!(llog.lock().unwrap().iter().any(|l| l.contains("TLS to the pod failed")), "{:?}", llog.lock().unwrap());
    assert!(seen.1.lock().unwrap().is_empty(), "no request reached that pod");
}

#[test]
fn a_pod_certificate_for_another_name_is_refused_by_the_listener() {
    let m = provisioned();
    let ca = pt::RouteCa { cert_pem: m.text(&names().ca), key_pem: m.text(&names().ca_key) };
    let (c, k) = pt::issue_for(&ca, NAME, Leaf::Server, "other.pod.iyakki.internal", (2020, 1, 1), (2040, 1, 1)).unwrap();
    let wrong = m.copy();
    wrong.set(&names().server_chain, &c);
    wrong.set(&names().server_key, &k);
    let p = pod(&wrong);
    let (_l, port, llog) = listener(&m, p.server.addr().port());
    assert_eq!(ask(port, "/x").0, 502);
    assert!(llog.lock().unwrap().iter().any(|l| l.contains("TLS to the pod failed")), "{:?}", llog.lock().unwrap());
    assert!(p.app.seen.lock().unwrap().is_empty());
}

#[test]
fn plain_http_is_never_spoken_either_way() {
    let m = provisioned();
    let p = pod(&m);
    // 1. a plain client talking to the pod gets no answer and the application sees nothing
    let mut s = connect(p.server.addr().port());
    s.write_all(get("x", "/secret").as_bytes()).unwrap();
    let mut got = Vec::new();
    let _ = s.read_to_end(&mut got);
    assert!(!String::from_utf8_lossy(&got).contains("HTTP/1."), "{:?}", String::from_utf8_lossy(&got));
    assert!(p.app.seen.lock().unwrap().is_empty());
    // 2. the listener against a target that only speaks plain HTTP: 502, and that target never got an HTTP request
    let plain = upstream("plain");
    let (_l, port, _) = listener(&m, plain.port);
    assert_eq!(ask(port, "/secret").0, 502);
    assert!(plain.seen.lock().unwrap().is_empty(), "the request was not sent in plain: {:?}", plain.seen.lock().unwrap());
    // 3. no certificate at all for the link: also 502, nothing sent
    let (_l2, port2, llog) = listener(&Mem::default(), plain.port);
    assert_eq!(ask(port2, "/secret").0, 502);
    assert!(plain.seen.lock().unwrap().is_empty());
    assert!(llog.lock().unwrap().iter().any(|l| l.contains("no usable certificate")), "{:?}", llog.lock().unwrap());
}

/// A TLS server built with rustls directly (a pod that is not ours): returns its port and the heads it was asked for.
fn fake_tls_pod(chain: &str, key: &str, ca: &str) -> (u16, Arc<Mutex<Vec<String>>>) {
    let prov = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for c in pem_ders(ca) {
        roots.add(c).unwrap();
    }
    let v = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), prov.clone()).build().unwrap();
    let cfg = Arc::new(rustls::ServerConfig::builder_with_provider(prov).with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_client_cert_verifier(v).with_single_cert(pem_ders(chain), key_der(key)).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut tcp) = c else { continue };
            timeout(&tcp);
            let mut conn = rustls::ServerConnection::new(cfg.clone()).unwrap();
            if conn.complete_io(&mut tcp).is_err() {
                continue;
            }
            let mut s = rustls::StreamOwned::new(conn, tcp);
            let mut b = [0u8; 2048];
            if let Ok(n) = s.read(&mut b) {
                if n > 0 {
                    s2.lock().unwrap().push(String::from_utf8_lossy(&b[..n]).to_string());
                }
            }
        }
    });
    (port, seen)
}

// ------------------------------------------------------------------------------------------------------------ rotation and the vault

#[test]
fn new_certificates_reach_both_ends_without_a_restart_and_a_bad_replacement_changes_nothing() {
    let m = provisioned();
    let p = pod(&m);
    let (l, port, llog) = listener(&m, p.server.addr().port());
    assert_eq!(ask(port, "/a").0, 200);
    let before = m.text(&names().client_chain);
    pt::rotate(&m, &m, NAME, Rotate::Leaves, unix_now(), 30).unwrap();
    assert_ne!(before, m.text(&names().client_chain), "a new listener certificate was written");
    l.reload_certs();
    p.server.replace(&server_material(&m)).unwrap();
    assert_eq!(ask(port, "/b").0, 200, "the new pair works");
    // a half-rotated pair (a certificate with the wrong key) and garbage: refused, the last good one stays
    let good_key = m.text(&names().client_key);
    m.set(&names().client_key, &m.text(&names().server_key));
    let lines = l.reload_certs();
    assert!(lines.iter().any(|x| x.contains("pod TLS") && x.contains("the previous one stays")), "{lines:?}");
    assert_eq!(ask(port, "/c").0, 200);
    m.set(&names().client_key, &good_key);
    m.set(&names().server_key, "garbage");
    assert!(p.server.replace(&server_material(&m)).is_err());
    assert_eq!(ask(port, "/d").0, 200, "the pod keeps its last good configuration");
    let all = llog.lock().unwrap().join("\n") + &p.log.lock().unwrap().join("\n");
    assert!(!all.contains("PRIVATE KEY") && !all.contains("garbage"), "no key text in a log line");
}

#[test]
fn a_new_ca_overlaps_the_old_one_and_the_old_ca_stops_working_when_it_is_dropped() {
    let m = provisioned();
    let old = m.copy();
    let p = pod(&m);
    let (l, port, _) = listener(&m, p.server.addr().port());
    assert_eq!(ask(port, "/a").0, 200);
    pt::rotate(&m, &m, NAME, Rotate::CaOverlap, unix_now(), 30).unwrap();
    // the old certificates still work against the bundle (the ends have not reloaded yet)
    assert_eq!(ask(port, "/b").0, 200);
    let bundle = m.text(&names().ca);
    assert_eq!(bundle.matches("BEGIN CERTIFICATE").count(), 2, "old and new CA are both trusted during the overlap");
    l.reload_certs();
    p.server.replace(&server_material(&m)).unwrap();
    assert_eq!(ask(port, "/c").0, 200, "new certificates of the new CA");
    // an end that still holds an old-CA certificate is accepted during the overlap, refused after drop-old
    let ca_old = old.text(&names().ca);
    let (oc, ok) = (old.text(&names().client_chain), old.text(&names().client_key));
    assert!(raw_client_with_roots(&p, &bundle, &oc, &ok).is_ok(), "overlap");
    pt::rotate(&m, &m, NAME, Rotate::DropOldCa, unix_now(), 30).unwrap();
    assert_eq!(m.text(&names().ca).matches("BEGIN CERTIFICATE").count(), 1);
    l.reload_certs();
    p.server.replace(&server_material(&m)).unwrap();
    assert_eq!(ask(port, "/d").0, 200);
    assert!(raw_client_with_roots(&p, &m.text(&names().ca), &oc, &ok).is_err(), "the old CA is no longer trusted");
    assert!(!ca_old.is_empty());
    assert_eq!(p.app.seen.lock().unwrap().iter().filter(|h| h.contains("/secret")).count(), 1, "only the overlap request got through, not the one after drop-old");
}

fn raw_client_with_roots(p: &Pod, roots: &str, chain: &str, key: &str) -> Result<String, String> {
    raw_client(p, roots, NAME, Some((chain, key)))
}

#[test]
fn the_listener_and_the_pod_read_only_their_own_secrets_never_the_ca_key() {
    let m = provisioned();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let src = vault::VaultCerts::new(Box::new(Spy(m.clone(), seen.clone())));
    src.load_pod_client(NAME).unwrap();
    let asked: Vec<String> = std::mem::take(&mut *seen.lock().unwrap());
    let n = names();
    assert_eq!(asked, vec![n.ca.clone(), n.client_chain.clone(), n.client_key.clone()], "the listener reads the CA certificate and its own pair");
    src.load_pod_server(NAME).unwrap();
    let asked: Vec<String> = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(asked, vec![n.ca.clone(), n.server_chain.clone(), n.server_key.clone()]);
    assert!(src.load_pod_client("../x").is_err());
    // the files source has the same layout and the same refusals
    let d = tmp("podfiles");
    for (f, v) in [("app.pod-ca", n.ca.as_str()), ("app.pod-client.crt", n.client_chain.as_str()), ("app.pod-client.key", n.client_key.as_str())] {
        std::fs::write(d.join(f), m.text(v)).unwrap();
    }
    let files = FileCerts::new(&d);
    assert_eq!(files.load_pod_client(NAME).unwrap().ca, m.text(&n.ca));
    assert!(files.load_pod_server(NAME).is_err(), "no server files: not made up");
    // the CA key is only read when rotating, by the control side
    pt::rotate(&Mem::default(), &Spy(m.clone(), seen.clone()), NAME, Rotate::Leaves, unix_now(), 30).unwrap();
    assert!(seen.lock().unwrap().contains(&n.ca_key));
}

#[test]
fn a_certificate_made_for_a_route_is_valid_for_one_name_and_one_use() {
    let m = provisioned();
    let n = names();
    let der = |p: String| pem_ders(&p).remove(0);
    let server = crate::cloudroute::read_cert(der(m.text(&n.server_chain)).as_ref()).unwrap();
    assert_eq!(server.names, vec![pt::server_name(NAME)]);
    let client = crate::cloudroute::read_cert(der(m.text(&n.client_chain)).as_ref()).unwrap();
    assert!(client.names.is_empty(), "the listener's certificate names no host");
    assert!(server.not_after - unix_now() < 91 * 86400 && server.not_after - unix_now() > 88 * 86400, "90 days");
    // a server certificate cannot be used as a client certificate and the other way round
    let ca = m.text(&n.ca);
    let p = pod(&m);
    let as_client = raw_client(&p, &ca, NAME, Some((&m.text(&n.server_chain), &m.text(&n.server_key))));
    assert!(as_client.is_err(), "the pod's own certificate is not a client certificate: {as_client:?}");
}

// ------------------------------------------------------------------------------------------------------------ the flags and the command

#[test]
fn the_pod_tls_flag_is_checked_against_the_routes_and_reaches_the_options() {
    let r = rig_with_gate();
    let mut pass = https_route("pass.example.org", 1, 8443, "c");
    pass.scheme = Scheme::Passthrough;
    let t = table(vec![route("plain.example.org", "/", 1, 8080), pass]);
    let ok = setup::build(&args("--pod-tls plain.example.org=app --bind 127.0.0.1 --no-firewall"), &t, &deps(&r)).unwrap();
    assert_eq!(ok.options.pod_tls.get("plain.example.org").map(String::as_str), Some("app"));
    assert!(ok.notes.iter().any(|n| n.contains("never in plain")));
    for a in ["--pod-tls pass.example.org=app", "--pod-tls nothere.example.org=app", "--pod-tls plain.example.org", "--pod-tls plain.example.org=a --pod-tls plain.example.org=b", "--pod-tls plain.example.org=../x"] {
        assert!(setup::build(&args(&format!("{a} --bind 127.0.0.1 --no-firewall")), &t, &deps(&r)).is_err(), "{a}");
    }
}

#[test]
fn the_command_checks_its_arguments_and_the_usage_names_every_flag() {
    let mut out = |_: &str| {};
    assert!(pt::cli(&[], &mut out).unwrap_err().contains("usage"));
    assert!(pt::cli(&args("create --name app --leaf-days 0"), &mut out).is_err());
    assert!(pt::cli(&args("serve --name app --listen nonsense --backend-port 1"), &mut out).is_err());
    assert!(pt::cli(&args("rotate --name app --what sideways"), &mut out).is_err());
    for f in ["create", "rotate", "serve", "--leaf-days", "--backend-port", "drop-old"] {
        assert!(pt::USAGE.contains(f), "{f}");
    }
    assert!(setup::USAGE.contains("--pod-tls"));
    assert_eq!(pt::ymd(0), (1970, 1, 1));
    assert_eq!(pt::ymd(1_000_000_000), (2001, 9, 9));
    assert_eq!(pt::ymd(1_709_164_800), (2024, 2, 29));
}

// ------------------------------------------------------------------------------------------------------------ the real vault service

fn vault_reader(url: &str, fp: &str, user: &str, seed: u8, scope: crate::cloudvaultd::Scope) -> vault::Reader {
    let t = crate::cloudvaultlink::VaultTarget::parse(url, Some(fp), Some("acme")).unwrap();
    vault::Reader { client: t.client(user, crate::cloudkeys::MemberKeys::from_seed([seed; 32])).unwrap(), scope }
}

#[test]
fn the_vault_reader_writes_and_reads_the_pod_link_over_the_real_vault_service_and_a_stranger_cannot() {
    use crate::cloudvaultd::{Principal, Scope};
    let d = crate::testdir::unique("podtls", "vault");
    {
        let tdir = d.join("tenants").join("acme");
        let mut r = crate::cloudroles::Roles::new("alice").unwrap();
        r.add_tenant_member("alice", "bob", crate::cloudroles::TenantRole::Member).unwrap();
        crate::cloudroles::save(&crate::cloudroles::path(&tdir), &r).unwrap();
        let st = crate::cloudvaultd::Store::open(&d).unwrap();
        for (u, n) in [("alice", 1u8), ("bob", 2)] {
            st.enroll(&Principal::Member { tenant: "acme".into(), user: u.into() }, &crate::cloudkeys::MemberKeys::from_seed([n; 32]).public()).unwrap();
        }
    }
    let run = crate::cloudvaultd::start("127.0.0.1:0".parse().unwrap(), &d, None).unwrap();
    let url = format!("https://127.0.0.1:{}", run.local_addr().port());
    let alice = vault_reader(&url, &run.fingerprint, "alice", 1, Scope::Tenant);
    alice.client.create_scope(&Scope::Tenant).unwrap();
    // writing through Reader::write is client.put: the six secrets arrive, sealed on this side
    pt::provision(&alice, NAME, unix_now(), 30).unwrap();
    let mut have = pt::PodVault::names(&alice).unwrap();
    have.sort();
    let n = names();
    let mut want = vec![n.ca.clone(), n.ca_key.clone(), n.client_chain.clone(), n.client_key.clone(), n.server_chain.clone(), n.server_key.clone()];
    want.sort();
    assert_eq!(have, want);
    assert!(pt::status(&alice, None, unix_now()).unwrap().contains("app: CA valid for"), "{}", pt::status(&alice, None, unix_now()).unwrap());
    // a member who was not given the scope's key reads and writes nothing, and the error holds no secret text
    let bob = vault_reader(&url, &run.fingerprint, "bob", 2, Scope::Tenant);
    let e = pt::read_material(&bob, NAME, "client").unwrap_err();
    assert!(!e.contains("PRIVATE") && !e.contains("BEGIN"), "{e}");
    let e = SecretWriter::write(&bob, &n.ca, b"x").unwrap_err();
    assert!(e.contains("could not") || !e.is_empty());
    assert_eq!(String::from_utf8(alice.read(&n.ca).unwrap()).unwrap().matches("BEGIN CERTIFICATE").count(), 1, "bob's refused write changed nothing");
    // the pod link works end to end with certificates that only ever travelled through the vault service
    let pod_end = Mem::default();
    for s in [&n.ca, &n.server_chain, &n.server_key] {
        pod_end.set(s, &String::from_utf8(alice.read(s).unwrap()).unwrap());
    }
    let p = pod(&pod_end);
    let (mut o, _log) = opts();
    o.pod_tls.insert(HOST.into(), NAME.into());
    let port = free_port();
    let src = Arc::new(vault::VaultCerts::new(Box::new(vault_reader(&url, &run.fingerprint, "alice", 1, Scope::Tenant))));
    let l = Listener::start(table(vec![route(HOST, "/", p.server.addr().port(), port)]), src, Arc::new(NoGate), o).unwrap();
    assert_eq!(ask(port, "/via-vault").0, 200);
    // a rotation written through the real service reaches the listener at its next reload
    pt::rotate(&alice, &alice, NAME, Rotate::Leaves, unix_now(), 30).unwrap();
    for s in [&n.server_chain, &n.server_key] {
        pod_end.set(s, &String::from_utf8(alice.read(s).unwrap()).unwrap());
    }
    l.reload_certs();
    p.server.replace(&server_material(&pod_end)).unwrap();
    assert_eq!(ask(port, "/after-rotation").0, 200);
    run.stop();
}

#[test]
fn the_spec_command_writes_the_sidecar_and_service_patches_for_the_route_of_a_host() {
    let d = tmp("spec");
    let f = d.join("routes.json");
    std::fs::write(&f, r#"{"routes":[{"host":"shop.example.org","target":{"service":{"name":"shop-web","port":8080}},"scheme":"http"}]}"#).unwrap();
    let mut out = Vec::new();
    pt::cli(&args(&format!("spec --name shop --routes {} --host shop.example.org --image reg.example.org/iyakki:1", f.display())), &mut |l| out.push(l.to_string())).unwrap();
    let all = out.join("\n");
    assert!(all.contains("# kubectl patch deployment shop-web") && all.contains("\"--backend-port\", \"8080\"") && all.contains("secretName: shop-pod-tls"), "{all}");
    assert!(all.contains("---\n# kubectl patch service shop-web") && all.contains("- port: 8080\n      targetPort: 8443"), "{all}");
    assert!(pt::cli(&args(&format!("spec --name shop --routes {} --host other.example.org --image x", f.display())), &mut |_| {}).is_err());
    // the Secret command prints no key: without --apply it needs the vault and says what it would do; here the vault is not given
    assert!(pt::cli(&args("secret --name shop --namespace team-a"), &mut |_| {}).is_err());
}
