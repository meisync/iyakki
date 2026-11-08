//! System components: the streamed blobs and the signed list on the server (in process and over a loopback socket), the retention plan, the
//! device's plan, download and hand-over, and the command line. No network beyond servers started here; nothing is applied to anything.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::compmanifest::{self as cm, Delta, Entry, Range, Record};
use super::components::{self as comps, Installed, PinnedStore, Release, Via};
use super::server::*;
use crate::pkg::{new_signing_key, public_key_hex, signing_key_hex};

const ADMIN: &str = "admin-token-for-tests-0123456789";

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-comp-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Pseudo-random bytes that are the same for the same seed.
struct Gen {
    state: u64,
    left: u64,
    max_read: usize,
    reads: u64,
}

impl Gen {
    fn new(seed: u64, n: u64) -> Gen {
        Gen { state: seed | 1, left: n, max_read: 0, reads: 0 }
    }
}

impl Read for Gen {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.max_read = self.max_read.max(buf.len());
        self.reads += 1;
        let n = (buf.len() as u64).min(self.left) as usize;
        for b in &mut buf[..n] {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            *b = (self.state >> 24) as u8;
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn bytes(seed: u64, n: u64) -> Vec<u8> {
    let mut v = vec![];
    Gen::new(seed, n).read_to_end(&mut v).unwrap();
    v
}

fn sha(b: &[u8]) -> String {
    crate::devmode::update::sha256_hex(b)
}

fn sha_of_stream(r: &mut dyn Read) -> String {
    use sha2::{Digest, Sha256};
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop {
        let n = r.read(&mut buf).unwrap();
        if n == 0 {
            return crate::pkg::hex(&h.finalize());
        }
        h.update(&buf[..n]);
    }
}

struct Rig {
    srv: Arc<StoreServer>,
    dir: PathBuf,
    ptoken: String,
    other: String,
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    th: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(tag: &str, tweak: impl Fn(&mut Config)) -> Rig {
    let d = dir(tag);
    let mut cfg = Config::new(&d.join("store"));
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    cfg.writes_per_min = 10_000;
    cfg.disk_margin = 1 << 20;
    tweak(&mut cfg);
    let key = new_signing_key().unwrap();
    let srv = Arc::new(StoreServer::open(cfg, &signing_key_hex(&key)).unwrap());
    let ptoken = srv.create_publisher("acme", "Acme Ltd").unwrap();
    let other = srv.create_publisher("other", "Other Ltd").unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (srv.clone(), stop.clone());
    let th = std::thread::spawn(move || super::server_http::serve(s2, l, st2));
    Rig { srv, dir: d, ptoken, other, addr, stop, th: Some(th) }
}

impl Rig {
    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn store(&self) -> PinnedStore {
        PinnedStore { name: "Test store".into(), base: self.base(), pin: self.srv.catalog_key() }
    }

    fn data(&self) -> PathBuf {
        self.dir.join("store")
    }
}

fn req(method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Req {
    let mut headers = std::collections::HashMap::new();
    if let Some(t) = token {
        headers.insert("authorization".to_string(), format!("Bearer {t}"));
    }
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let query = q.split('&').filter_map(|x| x.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Req { method: method.into(), path: p.into(), query, headers, body: body.to_vec() }
}

fn json(r: &Resp) -> serde_json::Value {
    serde_json::from_slice(&r.body).unwrap_or_default()
}

/// A whole small blob through `handle`.
fn put_blob(r: &Rig, token: &str, comp: &str, b: &[u8]) -> (String, Resp) {
    let h = sha(b);
    let resp = r.srv.handle(&req("PUT", &format!("/v1/blobs/{h}?component={comp}"), Some(token), b), "10.0.0.1");
    (h, resp)
}

fn entry_json(comp: &str, ver: &str, ch: &str, b: &[u8], requires: &[(&str, &str)], deltas: &[Delta]) -> serde_json::Value {
    serde_json::json!({"component": comp, "version": ver, "sha256": sha(b), "size": b.len(), "requires": requires.iter().cloned().collect::<BTreeMap<_, _>>(), "channel": ch, "built_from": "abc123", "delta": deltas})
}

fn post_entry(r: &Rig, token: &str, e: &serde_json::Value) -> Resp {
    r.srv.handle(&req("POST", "/v1/components", Some(token), e.to_string().as_bytes()), "10.0.0.1")
}

/// Upload and list in one go; the entry's result.
fn publish(r: &Rig, token: &str, comp: &str, ver: &str, ch: &str, b: &[u8], requires: &[(&str, &str)], deltas: &[Delta]) -> Resp {
    let (_, up) = put_blob(r, token, comp, b);
    assert!(up.status == 201 || up.status == 200, "{}", up.text());
    post_entry(r, token, &entry_json(comp, ver, ch, b, requires, deltas))
}

// ---------------------------------------------------------------------------------------- raw HTTP for the streaming routes

struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn raw(addr: std::net::SocketAddr, head: &str, body: Option<&mut dyn Read>) -> Answer {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(30))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    if let Some(b) = body {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = b.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            if s.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    }
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").expect("an HTTP answer");
    let head = String::from_utf8_lossy(&out[..at]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Answer { status, headers, body: out[at + 4..].to_vec() }
}

fn put_head(path: &str, token: &str, len: u64, extra: &str) -> String {
    format!("PUT {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {len}\r\n{extra}\r\n")
}

// ---------------------------------------------------------------------------------------- server: blobs

#[test]
fn a_50_mib_blob_streams_through_a_small_buffer_and_is_verified() {
    let r = rig("stream50", |_| {});
    let n: u64 = 50 << 20;
    let want = sha_of_stream(&mut Gen::new(7, n));
    let mut g = Gen::new(7, n);
    let resp = r.srv.blob_put(&req("PUT", &format!("/v1/blobs/{want}?component=base"), Some(&r.ptoken), b""), n, &mut g, "10.0.0.1");
    assert_eq!(resp.status, 201, "{}", resp.text());
    // the server never asked the reader for more than its buffer, and read every byte
    assert!(g.max_read <= BLOB_BUF, "the largest read was {} bytes", g.max_read);
    assert_eq!(g.left, 0);
    assert!(g.reads >= n / BLOB_BUF as u64);
    let f = r.data().join("blobs").join(&want);
    assert_eq!(std::fs::metadata(&f).unwrap().len(), n);
    assert!(!r.data().join("incoming").join(format!("{want}.part")).exists(), "no part file is left");
    // an existing blob is not stored twice
    let again = r.srv.blob_put(&req("PUT", &format!("/v1/blobs/{want}?component=base"), Some(&r.ptoken), b""), 5, &mut &b"hello"[..], "10.0.0.1");
    assert_eq!((again.status, json(&again)["existing"].as_bool()), (200, Some(true)));
}

#[test]
fn over_a_socket_a_20_mib_blob_goes_up_and_comes_down_in_ranges_with_immutable_headers() {
    let r = rig("socket", |c| c.max_body = 2048);
    let n: u64 = 20 << 20;
    let want = sha_of_stream(&mut Gen::new(11, n));
    let a = raw(r.addr, &put_head(&format!("/v1/blobs/{want}?component=core"), &r.ptoken, n, ""), Some(&mut Gen::new(11, n)));
    assert_eq!(a.status, 201, "{}", String::from_utf8_lossy(&a.body));
    // the package path keeps its small cap: the same size to /v1/publish is refused before it is read
    let big = raw(r.addr, "POST /v1/publish HTTP/1.1\r\nHost: x\r\nContent-Length: 20971520\r\n\r\n", None);
    assert_eq!(big.status, 413);
    // whole download
    let g = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\n\r\n"), None);
    assert_eq!(g.status, 200);
    assert_eq!(g.body.len() as u64, n);
    assert_eq!(sha(&g.body), want);
    assert_eq!(g.headers["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!((g.headers["accept-ranges"].as_str(), g.headers["etag"].as_str()), ("bytes", format!("\"{want}\"").as_str()));
    // a range, an open range, a suffix
    let all = g.body;
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes=100-199\r\n\r\n"), None);
    assert_eq!((p.status, p.headers["content-range"].as_str()), (206, format!("bytes 100-199/{n}").as_str()));
    assert_eq!(p.body, &all[100..200]);
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes={}-\r\n\r\n", n - 10), None);
    assert_eq!((p.status, p.body.as_slice()), (206, &all[all.len() - 10..]));
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes=-5\r\n\r\n"), None);
    assert_eq!((p.status, p.body.as_slice()), (206, &all[all.len() - 5..]));
    // beyond the end: 416 with the size; If-Range that does not match: the whole file
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes={n}-\r\n\r\n"), None);
    assert_eq!((p.status, p.headers["content-range"].as_str()), (416, format!("bytes */{n}").as_str()));
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes=0-9\r\nIf-Range: \"other\"\r\n\r\n"), None);
    assert_eq!((p.status, p.body.len() as u64), (200, n));
    let p = raw(r.addr, &format!("GET /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\nRange: bytes=0-9\r\nIf-Range: \"{want}\"\r\n\r\n"), None);
    assert_eq!(p.status, 206);
    // HEAD: the length, no body; unknown blobs and bad names
    let h = raw(r.addr, &format!("HEAD /v1/blobs/{want} HTTP/1.1\r\nHost: x\r\n\r\n"), None);
    assert_eq!((h.status, h.headers["content-length"].as_str(), h.body.len()), (200, n.to_string().as_str(), 0));
    assert_eq!(raw(r.addr, &format!("GET /v1/blobs/{} HTTP/1.1\r\nHost: x\r\n\r\n", "0".repeat(64)), None).status, 404);
    assert_eq!(raw(r.addr, "GET /v1/blobs/not-a-hash HTTP/1.1\r\nHost: x\r\n\r\n", None).status, 404);
}

#[test]
fn a_wrong_hash_is_refused_and_nothing_is_kept() {
    let r = rig("wronghash", |_| {});
    let data = bytes(3, 300_000);
    let lie = sha(b"something else");
    let resp = r.srv.blob_put(&req("PUT", &format!("/v1/blobs/{lie}?component=kernel"), Some(&r.ptoken), b""), data.len() as u64, &mut &data[..], "10.0.0.1");
    assert_eq!(resp.status, 422, "{}", resp.text());
    assert!(!r.data().join("blobs").join(&lie).exists());
    assert!(!r.data().join("incoming").join(format!("{lie}.part")).exists());
    // the same through the piecewise route: the check happens when the last piece arrives
    let h = r.srv.handle(&{ let mut q = req("PUT", &format!("/v1/blobs/{lie}?component=kernel"), Some(&r.ptoken), &data[..100_000]); q.headers.insert("content-range".into(), format!("bytes 0-99999/{}", data.len())); q }, "10.0.0.1");
    assert_eq!((h.status, json(&h)["have"].as_u64()), (200, Some(100_000)));
    let h = r.srv.handle(&{ let mut q = req("PUT", &format!("/v1/blobs/{lie}?component=kernel"), Some(&r.ptoken), &data[100_000..]); q.headers.insert("content-range".into(), format!("bytes 100000-{}/{}", data.len() - 1, data.len())); q }, "10.0.0.1");
    assert_eq!(h.status, 422);
    assert!(!r.data().join("blobs").join(&lie).exists() && !r.data().join("incoming").join(format!("{lie}.part")).exists());
}

#[test]
fn an_upload_resumes_from_what_the_store_holds() {
    let r = rig("resume", |_| {});
    let data = bytes(5, 1_000_000);
    let h = sha(&data);
    let path = format!("/v1/blobs/{h}?component=base");
    // the first 300000 bytes arrive, then the connection ends 100000 bytes into the next piece
    let piece = |from: usize, to: usize| {
        let mut q = req("PUT", &path, Some(&r.ptoken), b"");
        q.headers.insert("content-range".into(), format!("bytes {from}-{}/{}", to - 1, data.len()));
        r.srv.blob_put(&q, (to - from) as u64, &mut &data[from..to], "10.0.0.1")
    };
    assert_eq!(json(&piece(0, 300_000))["have"], 300_000);
    // a piece that claims 200000 bytes but ends after 100000
    let mut q = req("PUT", &path, Some(&r.ptoken), b"");
    q.headers.insert("content-range".into(), "bytes 300000-499999/1000000".into());
    let cut = r.srv.blob_put(&q, 200_000, &mut &data[300_000..400_000], "10.0.0.1");
    assert_eq!((cut.status, json(&cut)["have"].as_u64()), (400, Some(400_000)));
    // the status question says the same; a piece from the wrong place is told where to continue
    let st = r.srv.blob_get(&req("GET", &format!("/v1/blobs/{h}/part"), Some(&r.ptoken), b""), "10.0.0.1");
    assert!(String::from_utf8_lossy(match &st.body { Body::Bytes(b) => b, _ => panic!() }).contains("400000"));
    let wrong = piece(500_000, 600_000);
    assert_eq!((wrong.status, json(&wrong)["have"].as_u64()), (409, Some(400_000)));
    assert_eq!(json(&piece(400_000, 700_000))["have"], 700_000);
    let done = piece(700_000, 1_000_000);
    assert_eq!(done.status, 201, "{}", done.text());
    assert_eq!(std::fs::read(r.data().join("blobs").join(&h)).unwrap(), data);
    // the status of a finished blob
    let st = r.srv.blob_get(&req("GET", &format!("/v1/blobs/{h}/part"), Some(&r.ptoken), b""), "10.0.0.1");
    assert!(String::from_utf8_lossy(match &st.body { Body::Bytes(b) => b, _ => panic!() }).contains("\"exists\":true"));
}

#[test]
fn blob_limits_disk_space_and_who_may_upload() {
    let d = dir("limits");
    let free = Arc::new(AtomicU64::new(u64::MAX));
    let f2 = free.clone();
    let mut cfg = Config::new(&d.join("store"));
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    cfg.component_caps.insert("kernel".into(), 1 << 20);
    cfg.disk_margin = 1 << 20;
    let srv = StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap().with_free_space(Arc::new(move |_| Some(f2.load(Ordering::SeqCst))));
    let ptoken = srv.create_publisher("acme", "Acme").unwrap();
    let put = |tok: Option<&str>, comp: &str, b: &[u8]| srv.handle(&req("PUT", &format!("/v1/blobs/{}?component={comp}", sha(b)), tok, b), "10.0.0.1");
    let big = bytes(1, (1 << 20) + 1);
    let r = put(Some(&ptoken), "kernel", &big);
    assert_eq!(r.status, 413, "the kernel cap is 1 MiB");
    assert!(r.text().contains("limited to 1 MiB"), "{}", r.text());
    assert_eq!(put(Some(&ptoken), "base", &big).status, 201, "the other kinds use the default cap");
    assert_eq!(put(None, "base", b"x").status, 401);
    assert_eq!(put(Some("wrong"), "base", b"x").status, 401);
    assert_eq!(put(Some(ADMIN), "nonsense", b"x").status, 400, "the component must be one the store takes");
    assert_eq!(srv.handle(&req("PUT", "/v1/blobs/zz?component=base", Some(ADMIN), b"x"), "10.0.0.1").status, 400);
    free.store(5 << 20, Ordering::SeqCst);
    let data = bytes(2, 6 << 20);
    assert_eq!(put(Some(ADMIN), "base", &data).status, 507, "6 MiB does not fit in 5 MiB of free disk");
    assert!(!d.join("store/blobs").join(sha(&data)).exists());
    free.store(100 << 20, Ordering::SeqCst);
    assert_eq!(put(Some(ADMIN), "base", &data).status, 201);
    // a configured default cap applies to every component
    let mut small = Config::new(&d.join("small"));
    small.admin_token_hash = Some(hash_token(ADMIN));
    small.max_component = 1000;
    let small = StoreServer::open(small, &signing_key_hex(&new_signing_key().unwrap())).unwrap();
    assert_eq!(small.handle(&req("PUT", &format!("/v1/blobs/{}?component=core", sha(&[1u8; 1001])), Some(ADMIN), &[1u8; 1001]), "10.0.0.1").status, 413);
    // a read-only store takes nothing
    let mut ro = Config::new(&d.join("ro"));
    ro.admin_token_hash = Some(hash_token(ADMIN));
    ro.read_only = true;
    let ro = StoreServer::open(ro, &signing_key_hex(&new_signing_key().unwrap())).unwrap();
    assert_eq!(ro.handle(&req("PUT", &format!("/v1/blobs/{}?component=base", sha(b"x")), Some(ADMIN), b"x"), "10.0.0.1").status, 403);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------------------- server: the signed list

#[test]
fn the_list_is_signed_and_a_device_accepts_it_only_with_the_pinned_key() {
    let r = rig("signed", |_| {});
    let k1 = bytes(1, 4000);
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.5", "stable", &k1, &[], &[]).status, 200);
    let get = r.srv.handle(&req("GET", "/v1/components.json", None, b""), "10.0.0.1");
    assert_eq!((get.status, get.content_type), (200, "application/json"));
    let doc = cm::verify(&get.body, &r.srv.catalog_key()).unwrap();
    assert_eq!(doc.entries.len(), 1);
    let e = &doc.entries[0];
    assert_eq!((e.component.as_str(), e.version.as_str(), e.sha256.as_str(), e.size, e.install_path.as_str(), e.built_from.as_str()), ("kernel", "6.12.5", sha(&k1).as_str(), 4000, "boot/bzImage", "abc123"));
    // another key, no key, a changed byte, no signature, a different document: refused
    assert!(cm::verify(&get.body, &public_key_hex(&new_signing_key().unwrap())).unwrap_err().contains("not signed by the key you confirmed"));
    assert!(cm::verify(&get.body, "").is_err());
    let text = String::from_utf8_lossy(&get.body).into_owned();
    assert!(cm::verify(text.replace("6.12.5", "9.9.9").as_bytes(), &r.srv.catalog_key()).is_err());
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v.as_object_mut().unwrap().remove("sig");
    assert!(cm::verify(v.to_string().as_bytes(), &r.srv.catalog_key()).is_err());
    assert!(cm::verify(br#"{"components":[]}"#, &r.srv.catalog_key()).is_err());
    // the catalog key signs the catalog and this list for different purposes: the catalog is not a component list
    let cat = r.srv.handle(&req("GET", "/catalog.json", None, b""), "10.0.0.1");
    assert!(cm::verify(&cat.body, &r.srv.catalog_key()).is_err());
    // nobody writes through the GET route
    assert_eq!(r.srv.handle(&req("PUT", "/v1/components.json", Some(ADMIN), b"{}"), "10.0.0.1").status, 405);
}

#[test]
fn publishing_rules_versions_only_go_up_names_have_owners_and_requires_must_parse() {
    let r = rig("rules", |_| {});
    let k = |n: u64| bytes(100 + n, 5000 + n);
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.5", "nightly", &k(1), &[("core", ">=1100")], &[]).status, 200);
    // the same or an older version in the same channel: refused; another channel is its own line
    let again = publish(&r, &r.ptoken, "kernel", "6.12.5", "nightly", &k(2), &[], &[]);
    assert_eq!(again.status, 409, "{}", again.text());
    assert!(again.text().contains("versions only go up"));
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.4", "nightly", &k(3), &[], &[]).status, 409);
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.4", "beta", &k(3), &[], &[]).status, 200);
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.10", "nightly", &k(4), &[], &[]).status, 200, "6.12.10 is newer than 6.12.5");
    // a component name belongs to the first publisher
    let theirs = publish(&r, &r.other, "kernel", "7.0.0", "nightly", &k(5), &[], &[]);
    assert_eq!(theirs.status, 403, "{}", theirs.text());
    assert!(theirs.text().contains("belongs to another publisher"));
    // the entry must describe a blob that is on the store, with the same size
    let ghost = bytes(9, 777);
    let e = entry_json("boot", "1", "nightly", &ghost, &[], &[]);
    let no = post_entry(&r, &r.ptoken, &e);
    assert_eq!(no.status, 422);
    assert!(no.text().contains("PUT /v1/blobs/"));
    put_blob(&r, &r.ptoken, "boot", &ghost);
    let mut wrong = e.clone();
    wrong["size"] = 778.into();
    assert_eq!(post_entry(&r, &r.ptoken, &wrong).status, 422);
    // ranges that do not parse, a name this store does not take, no install path for a kernel layer, a delta from nothing
    let mut bad = e.clone();
    bad["requires"] = serde_json::json!({"kernel": "banana"});
    let b = post_entry(&r, &r.ptoken, &bad);
    assert_eq!(b.status, 422);
    assert!(b.text().contains("requires kernel"), "{}", b.text());
    bad["requires"] = serde_json::json!({"boot": ">=1"});
    assert_eq!(post_entry(&r, &r.ptoken, &bad).status, 422, "a component cannot require itself");
    let mut other = e.clone();
    other["component"] = "gadget".into();
    assert_eq!(post_entry(&r, &r.ptoken, &other).status, 422);
    let layer = bytes(10, 600);
    put_blob(&r, &r.ptoken, "kernel-layer", &layer);
    assert!(post_entry(&r, &r.ptoken, &entry_json("kernel-layer", "1", "nightly", &layer, &[], &[])).text().contains("install_path"));
    let mut nodelta = e.clone();
    nodelta["delta"] = serde_json::json!([{"from_sha256": sha(b"nothing"), "sha256": sha(&ghost), "size": ghost.len()}]);
    assert_eq!(post_entry(&r, &r.ptoken, &nodelta).status, 422);
    // a good one
    assert_eq!(post_entry(&r, &r.ptoken, &e).status, 200);
    // no token, wrong token
    assert_eq!(r.srv.handle(&req("POST", "/v1/components", None, e.to_string().as_bytes()), "10.0.0.1").status, 401);
}

#[test]
fn a_withdrawn_version_leaves_the_list_stays_in_the_notice_and_cannot_come_back() {
    let r = rig("yank", |_| {});
    let a = bytes(21, 3000);
    assert_eq!(publish(&r, &r.ptoken, "core", "1100", "stable", &a, &[], &[]).status, 200);
    assert_eq!(publish(&r, &r.ptoken, "core", "1101", "stable", &bytes(22, 3000), &[], &[]).status, 200);
    let del = |tok: &str, path: &str| r.srv.handle(&req("DELETE", path, Some(tok), b""), "10.0.0.1");
    assert_eq!(del(&r.other, "/v1/components/core/1101?reason=bad").status, 403, "another publisher cannot withdraw it");
    assert_eq!(del("nope", "/v1/components/core/1101").status, 401);
    assert_eq!(del(&r.ptoken, "/v1/components/core/9999").status, 404);
    assert_eq!(del(&r.ptoken, "/v1/components/core/1101?reason=crashes+at+start").status, 200);
    let doc = cm::verify(&r.srv.handle(&req("GET", "/v1/components.json", None, b""), "10.0.0.1").body, &r.srv.catalog_key()).unwrap();
    assert_eq!(doc.entries.iter().map(|e| e.version.as_str()).collect::<Vec<_>>(), ["1100"]);
    assert_eq!((doc.yanked.len(), doc.yanked[0].version.as_str()), (1, "1101"));
    assert!(doc.latest("core", "stable").unwrap().version == "1100");
    // the version number cannot be reused for other bytes
    assert_eq!(publish(&r, &r.ptoken, "core", "1101", "stable", &bytes(23, 3000), &[], &[]).status, 409);
    // the administrator can withdraw everything of a component
    assert_eq!(del(ADMIN, "/v1/components/core").status, 200);
    let doc = cm::verify(&r.srv.handle(&req("GET", "/v1/components.json", None, b""), "10.0.0.1").body, &r.srv.catalog_key()).unwrap();
    assert!(doc.entries.is_empty() && doc.yanked.len() == 2);
}

// ---------------------------------------------------------------------------------------- retention

fn rec(comp: &str, ch: &str, ver: &str, n: u64, deltas: &[u64]) -> Record {
    let h = |n: u64| format!("{n:064x}");
    Record { entry: Entry { component: comp.into(), version: ver.into(), sha256: h(n), size: 100, requires: BTreeMap::new(), channel: ch.into(), built_from: String::new(), delta: deltas.iter().map(|f| Delta { from_sha256: h(*f), sha256: h(n * 1000 + f), size: 10 }).collect(), install_path: String::new(), added: 0 }, owner: "acme".into(), yanked: None, purged: false }
}

#[test]
fn retention_keeps_the_newest_n_with_the_deltas_between_them_and_says_what_goes() {
    let mut recs = vec![rec("kernel", "nightly", "1", 1, &[]), rec("kernel", "nightly", "2", 2, &[1]), rec("kernel", "nightly", "3", 3, &[2, 1]), rec("kernel", "nightly", "4", 4, &[3, 2]), rec("kernel", "nightly", "5", 5, &[4, 3])];
    recs.push(rec("core", "nightly", "10", 50, &[]));
    let p = cm::plan_retention(&recs, 2);
    // 5 and 4 stay, 3 2 1 go; 5 keeps its delta from 4 and loses the one from 3; 4 loses both (from 3 and 2)
    assert_eq!(p.drop_entries.iter().map(|d| d.2.as_str()).collect::<Vec<_>>(), ["3", "2", "1"]);
    let mut dd: Vec<_> = p.drop_deltas.iter().map(|d| (d.1.as_str(), d.2.clone())).collect();
    dd.sort();
    assert_eq!(dd, [("4", format!("{:064x}", 4 * 1000 + 3)), ("4", format!("{:064x}", 4 * 1000 + 2)), ("5", format!("{:064x}", 5 * 1000 + 3))].iter().map(|(a, b)| (*a, b.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());
    // blobs: the three full artifacts that go, the three deltas that belong to them (1 + 2), and the three deltas dropped from kept entries
    assert_eq!(p.remove_blobs.len(), 3 + 3 + 3, "{:?}", p.remove_blobs);
    assert!(p.remove_blobs.contains(&format!("{:064x}", 3)) && !p.remove_blobs.contains(&format!("{:064x}", 5)) && !p.remove_blobs.contains(&format!("{:064x}", 5004)));
    assert_eq!(p.bytes, 3 * 100 + 6 * 10, "full artifacts count 100, deltas 10");
    // applying the plan to the records leaves exactly what the plan promised
    let after = cm::apply_retention(&recs, &p);
    assert_eq!(after.iter().filter(|r| r.entry.component == "kernel").map(|r| r.entry.version.as_str()).collect::<Vec<_>>(), ["4", "5"]);
    assert_eq!(after.iter().find(|r| r.entry.version == "5").unwrap().entry.delta.len(), 1);
    assert!(after.iter().find(|r| r.entry.version == "4").unwrap().entry.delta.is_empty());
    assert!(cm::plan_retention(&after, 2).is_empty(), "a second run has nothing to do");
    // keep more than there is: nothing goes
    assert!(cm::plan_retention(&recs, 9).is_empty());
}

#[test]
fn retention_withdrawn_entries_lose_their_files_not_their_notice_and_a_shared_blob_stays() {
    let mut recs = vec![rec("base", "nightly", "1", 1, &[]), rec("base", "nightly", "2", 2, &[]), rec("base", "nightly", "3", 3, &[])];
    recs[0].yanked = Some("bad".into());
    // beta carries a copy of version 3 (a promotion): same bytes, another channel; nightly keeps only 3
    let mut promoted = rec("base", "beta", "3", 3, &[]);
    promoted.entry.version = "3".into();
    recs.push(promoted);
    let p = cm::plan_retention(&recs, 1);
    assert_eq!(p.purge_yanked.iter().map(|d| d.2.as_str()).collect::<Vec<_>>(), ["1"]);
    assert_eq!(p.drop_entries.iter().map(|d| d.2.as_str()).collect::<Vec<_>>(), ["2"]);
    assert!(p.remove_blobs.contains(&format!("{:064x}", 1)) && p.remove_blobs.contains(&format!("{:064x}", 2)) && !p.remove_blobs.contains(&format!("{:064x}", 3)));
    let after = cm::apply_retention(&recs, &p);
    assert!(after.iter().any(|r| r.entry.version == "1" && r.yanked.is_some() && r.purged), "the notice stays");
    let key = new_signing_key().unwrap();
    let doc = cm::verify(cm::build_doc("s", 5, 1, &after, &key).as_bytes(), &public_key_hex(&key)).unwrap();
    assert_eq!(doc.yanked.len(), 1, "devices keep refusing the withdrawn version");
}

#[test]
fn the_retention_route_is_the_administrators_asks_before_it_removes_and_clears_orphans() {
    let d = dir("retroute");
    let mut cfg = Config::new(&d.join("store"));
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    cfg.writes_per_min = 10_000;
    cfg.disk_margin = 1 << 20;
    let srv = StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap();
    let tok = srv.create_publisher("acme", "Acme").unwrap();
    let r = |m: &str, p: &str, t: &str| srv.handle(&req(m, p, Some(t), b""), "10.0.0.1");
    let mut blobs = vec![];
    for (i, v) in ["1", "2", "3", "4"].iter().enumerate() {
        let b = bytes(300 + i as u64, 2000 + i as u64);
        let h = sha(&b);
        assert_eq!(srv.handle(&req("PUT", &format!("/v1/blobs/{h}?component=core"), Some(&tok), &b), "10.0.0.1").status, 201);
        let e = entry_json("core", v, "stable", &b, &[], &[]);
        assert_eq!(srv.handle(&req("POST", "/v1/components", Some(&tok), e.to_string().as_bytes()), "10.0.0.1").status, 200);
        blobs.push(h);
    }
    // an upload nobody listed: new, so it stays; an old one goes
    let (fresh, old) = (bytes(400, 100), bytes(401, 101));
    for b in [&fresh, &old] {
        srv.handle(&req("PUT", &format!("/v1/blobs/{}?component=core", sha(b)), Some(&tok), b), "10.0.0.1");
    }
    let oldf = std::fs::File::options().write(true).open(d.join("store/blobs").join(sha(&old))).unwrap();
    oldf.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400)).unwrap();
    drop(oldf);
    assert_eq!(r("POST", "/v1/components/retention?keep=2", &tok).status, 403, "a publisher cannot run it");
    assert_eq!(r("POST", "/v1/components/retention?keep=0", ADMIN).status, 400);
    let dry = json(&r("POST", "/v1/components/retention?keep=2", ADMIN));
    assert_eq!((dry["applied"].as_bool(), dry["drop_entries"].as_array().unwrap().len(), dry["orphans"].as_u64()), (Some(false), 2, Some(1)));
    assert!(d.join("store/blobs").join(&blobs[0]).exists(), "a dry run removes nothing");
    let done = json(&r("POST", "/v1/components/retention?keep=2&apply=1", ADMIN));
    assert_eq!((done["applied"].as_bool(), done["removed"].as_u64()), (Some(true), Some(3)));
    assert!(!d.join("store/blobs").join(&blobs[0]).exists() && !d.join("store/blobs").join(&blobs[1]).exists());
    assert!(d.join("store/blobs").join(&blobs[3]).exists() && d.join("store/blobs").join(sha(&fresh)).exists() && !d.join("store/blobs").join(sha(&old)).exists());
    let doc = cm::verify(&srv.handle(&req("GET", "/v1/components.json", None, b""), "10.0.0.1").body, &srv.catalog_key()).unwrap();
    assert_eq!(doc.entries.iter().map(|e| e.version.as_str()).collect::<Vec<_>>(), ["3", "4"]);
    let again = json(&r("POST", "/v1/components/retention?keep=2&apply=1", ADMIN));
    assert_eq!(again["removed"].as_u64(), Some(0));
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------------------- ranges

#[test]
fn ranges_parse_and_match_by_version() {
    let m = |r: &str, v: &str| Range::parse(r).unwrap().matches(v);
    assert!(m(">=6.12", "6.12.5") && m(">=6.12", "6.12") && !m(">=6.12", "6.11.9"));
    assert!(m("6.12..6.13", "6.12.5") && !m("6.12..6.13", "6.13") && !m("6.12..6.13", "6.11"));
    assert!(m(">=1100, <1200", "1107") && !m(">=1100, <1200", "1200") && m("<2", "1.9"));
    assert!(m("1107", "1107") && !m("1107", "1108") && m("=1107", "1107") && m("*", "anything-1") && m(">1", "2"));
    for bad in ["", "banana", ">=", "6.12..", ">=a.b", "<>1", &"1".repeat(80)] {
        assert!(Range::parse(bad).is_err(), "{bad:?}");
    }
    assert!(cm::valid_version("0.1.0-alpha.3") && cm::valid_version("1107") && !cm::valid_version("v1") && !cm::valid_version("1..2") && !cm::valid_version(""));
    assert!(cm::valid_install_path("boot/bzImage") && cm::valid_install_path("/usr/bin/iyakki") && !cm::valid_install_path("/etc/../x") && !cm::valid_install_path("home/x") && !cm::valid_install_path("boot/../x"));
}

// ---------------------------------------------------------------------------------------- the device's plan

fn h(n: u64) -> String {
    format!("{n:064x}")
}

fn ent(comp: &str, ver: &str, ch: &str, n: u64, size: u64, requires: &[(&str, &str)], deltas: &[(u64, u64)]) -> Entry {
    Entry { component: comp.into(), version: ver.into(), sha256: h(n), size, requires: requires.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), channel: ch.into(), built_from: String::new(), delta: deltas.iter().map(|(f, s)| Delta { from_sha256: h(*f), sha256: h(n * 1000 + f), size: *s }).collect(), install_path: String::new(), added: 0 }
}

fn doc_of(entries: Vec<Entry>, yanked: &[(&str, u64)]) -> cm::Doc {
    let key = new_signing_key().unwrap();
    let mut recs: Vec<Record> = entries.into_iter().map(|entry| Record { entry, owner: "acme".into(), yanked: None, purged: false }).collect();
    for (c, n) in yanked {
        let mut r = Record { entry: ent(c, "0", "stable", *n, 10, &[], &[]), owner: "acme".into(), yanked: Some("it crashed".into()), purged: false };
        r.entry.version = "0".into();
        recs.push(r);
    }
    cm::verify(cm::build_doc("s", 7, 1, &recs, &key).as_bytes(), &public_key_hex(&key)).unwrap()
}

fn installed(v: &[(&str, &str, u64)]) -> Installed {
    let mut i = Installed::default();
    for (c, ver, n) in v {
        i.versions.insert(c.to_string(), ver.to_string());
        i.shas.insert(c.to_string(), h(*n));
    }
    i
}

#[test]
fn a_delta_is_preferred_when_it_starts_from_the_installed_file_otherwise_the_whole_file() {
    let dd = dir("planpref");
    let doc = doc_of(vec![ent("core", "1108", "stable", 8, 80_000_000, &[], &[(7, 3_000_000), (6, 90_000_000)])], &[]);
    let file = dd.join("core-old");
    std::fs::write(&file, b"x").unwrap();
    let mut inst = installed(&[("core", "1107", 7)]);
    inst.files.insert("core".into(), file.clone());
    let p = comps::plan(&doc, "S", "stable", &inst, None).unwrap();
    assert_eq!((p.steps[0].via, p.steps[0].download, p.steps[0].blob_sha256.clone()), (Via::Delta, 3_000_000, h(8007)));
    assert_eq!(p.steps[0].full_download, 80_000_000);
    // installed file is another version than any delta starts from: whole file
    let mut inst2 = installed(&[("core", "1105", 5)]);
    inst2.files.insert("core".into(), file.clone());
    assert_eq!(comps::plan(&doc, "S", "stable", &inst2, None).unwrap().steps[0].via, Via::Full);
    // a delta that is not smaller is not used
    let doc2 = doc_of(vec![ent("core", "1108", "stable", 8, 80_000_000, &[], &[(6, 90_000_000)])], &[]);
    let mut inst3 = installed(&[("core", "1106", 6)]);
    inst3.files.insert("core".into(), file);
    assert_eq!(comps::plan(&doc2, "S", "stable", &inst3, None).unwrap().steps[0].via, Via::Full);
    // the installed file is not there: whole file
    let inst4 = installed(&[("core", "1107", 7)]);
    assert_eq!(comps::plan(&doc, "S", "stable", &inst4, None).unwrap().steps[0].via, Via::Full);
    // up to date, newer than the store, other channel
    let up = comps::plan(&doc, "S", "stable", &installed(&[("core", "1108", 8)]), None).unwrap();
    assert!(up.steps.is_empty() && up.current == ["core 1108"]);
    assert!(comps::plan(&doc, "S", "stable", &installed(&[("core", "1200", 9)]), None).unwrap().steps.is_empty());
    assert!(comps::plan(&doc, "S", "beta", &inst4, None).unwrap().steps.is_empty());
    // a list older than the one seen before is refused (rollback of the store's list)
    assert!(comps::plan(&doc, "S", "stable", &inst4, Some(8)).unwrap_err().contains("older component list"));
    assert!(comps::plan(&doc, "S", "stable", &inst4, Some(7)).is_ok());
    let _ = std::fs::remove_dir_all(&dd);
}

#[test]
fn requires_are_checked_against_what_will_be_installed_and_the_refusal_names_the_part_that_goes_first() {
    // core needs a newer kernel; the kernel is in the store: both are planned, kernel first
    let doc = doc_of(vec![ent("kernel", "6.12.5", "stable", 2, 15_000_000, &[], &[]), ent("core", "1108", "stable", 8, 80_000_000, &[("kernel", ">=6.12")], &[])], &[]);
    let inst = installed(&[("kernel", "6.10.2", 1), ("core", "1107", 7)]);
    let p = comps::plan(&doc, "S", "stable", &inst, None).unwrap();
    assert!(p.blocked.is_empty());
    assert_eq!(p.steps.iter().map(|s| s.component.as_str()).collect::<Vec<_>>(), ["kernel", "core"]);
    // the kernel is not in the store: core is not offered, and the message says what to do first
    let doc = doc_of(vec![ent("core", "1108", "stable", 8, 80_000_000, &[("kernel", ">=6.12")], &[])], &[]);
    let p = comps::plan(&doc, "S", "stable", &inst, None).unwrap();
    assert!(p.steps.is_empty());
    let why = &p.blocked[0].why;
    assert!(why.contains("core 1108 needs kernel >=6.12 but this device has kernel 6.10.2") && why.contains("update kernel first"), "{why}");
    let text = comps::plan_text(&p);
    assert!(text.contains("Not offered: core 1108 needs kernel") && text.contains("Nothing has been downloaded or installed"), "{text}");
    // the offered kernel is itself not enough
    let doc = doc_of(vec![ent("kernel", "6.11.0", "stable", 2, 15_000_000, &[], &[]), ent("core", "1108", "stable", 8, 80_000_000, &[("kernel", ">=6.12")], &[])], &[]);
    let p = comps::plan(&doc, "S", "stable", &inst, None).unwrap();
    assert_eq!(p.steps.iter().map(|s| s.component.as_str()).collect::<Vec<_>>(), ["kernel"]);
    assert!(p.blocked[0].why.contains("kernel 6.11.0") || p.blocked[0].why.contains("has kernel 6.11.0"), "{}", p.blocked[0].why);
    // a chain: base needs core >= 2000, which is blocked, so base is blocked too
    let doc = doc_of(vec![ent("core", "2000", "stable", 20, 80_000_000, &[("kernel", ">=7")], &[]), ent("base", "40", "stable", 4, 400_000_000, &[("core", ">=2000")], &[])], &[]);
    let p = comps::plan(&doc, "S", "stable", &inst, None).unwrap();
    assert!(p.steps.is_empty() && p.blocked.len() == 2, "{:?}", p.blocked);
    assert!(p.blocked.iter().any(|b| b.component == "base" && b.why.contains("core")));
    // a fact that is not a component: it must match exactly
    let doc = doc_of(vec![ent("kernel-layer", "9", "stable", 9, 1_000_000, &[("abi", "tile-abi 2")], &[])], &[]);
    let mut i2 = installed(&[]);
    assert!(comps::plan(&doc, "S", "stable", &i2, None).unwrap().blocked[0].why.contains("abi"));
    i2.extras.insert("abi".into(), "tile-abi 2".into());
    assert_eq!(comps::plan(&doc, "S", "stable", &i2, None).unwrap().steps.len(), 1);
}

#[test]
fn the_plan_text_says_what_changes_how_big_and_whether_a_restart_is_needed() {
    let doc = doc_of(vec![ent("kernel", "6.12.5", "stable", 2, 15_000_000, &[], &[]), ent("core", "1108", "stable", 8, 80_000_000, &[], &[(7, 3_000_000)]), ent("base", "40", "stable", 4, 400_000_000, &[], &[])], &[("boot", 99)]);
    let dd = dir("plantext");
    let f = dd.join("c");
    std::fs::write(&f, b"x").unwrap();
    let mut inst = installed(&[("core", "1107", 7), ("kernel", "6.10.2", 1), ("base", "40", 4)]);
    inst.files.insert("core".into(), f);
    let t = comps::plan_text(&comps::plan(&doc, "My store", "stable", &inst, None).unwrap());
    assert!(t.contains("kernel 6.10.2 -> 6.12.5: the whole file, 14.3 MB; the machine restarts to use it"), "{t}");
    assert!(t.contains("core 1107 -> 1108: only the difference, 2.9 MB (the whole file is 76.3 MB); IYAKKI's services restart, not the machine"), "{t}");
    assert!(t.contains("Up to date: base 40") && t.contains("A restart of the machine is needed") && t.contains("Nothing has been downloaded or installed"), "{t}");
    // a withdrawn installed version is said
    let mut inst2 = installed(&[("boot", "5", 99)]);
    inst2.versions.insert("boot".into(), "5".into());
    let doc2 = doc_of(vec![ent("boot", "6", "stable", 6, 1_000_000, &[], &[])], &[("boot", 99)]);
    assert!(comps::plan_text(&comps::plan(&doc2, "S", "stable", &inst2, None).unwrap()).contains("was withdrawn by the store: it crashed"));
    let _ = std::fs::remove_dir_all(&dd);
}

// ---------------------------------------------------------------------------------------- download, delta, resume

/// A streamer that cuts the first body after `cut` bytes and remembers the headers of every request.
struct Cut {
    inner: Arc<dyn iyakki_proxy::Streamer>,
    cut: AtomicU64,
    seen: Mutex<Vec<Vec<(String, String)>>>,
}

struct CutBody {
    inner: Box<dyn Read + Send>,
    left: u64,
}

impl Read for CutBody {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "network problem: the connection ended unexpectedly"));
        }
        let n = b.len().min(self.left as usize);
        let k = self.inner.read(&mut b[..n])?;
        self.left -= k as u64;
        Ok(k)
    }
}

impl iyakki_proxy::Streamer for Cut {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        self.seen.lock().unwrap().push(headers.to_vec());
        let mut head = self.inner.open(url, headers)?;
        let cut = self.cut.swap(0, Ordering::SeqCst);
        if cut > 0 && head.status < 300 {
            head.body = Box::new(CutBody { inner: head.body, left: cut });
        }
        Ok(head)
    }
}

fn streamer() -> Arc<dyn iyakki_proxy::Streamer> {
    Arc::new(iyakki_proxy::https::UreqStreamer::default())
}

fn cli_publish(r: &Rig, comp: &str, ver: &str, file: &Path, extra: &[&str]) -> Result<String, String> {
    let var = format!("IYAKKI_TEST_COMP_TOKEN_{}", r.addr.port());
    std::env::set_var(&var, &r.ptoken);
    let mut a: Vec<String> = ["component", "publish", "--server", &r.base(), "--token-env", &var, "--component", comp, "--version", ver, "--file", &file.display().to_string(), "--channel", "stable"].iter().map(|s| s.to_string()).collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    super::compcli::run(&a)
}

#[test]
fn the_publish_command_uploads_in_pieces_makes_a_delta_and_the_device_downloads_resumes_and_applies_it() {
    let r = rig("e2e", |_| {});
    let w = dir("e2e-work");
    // version 1 of the core, then version 2 with a few changed regions
    let v1 = bytes(77, 1_200_000);
    let mut v2 = v1.clone();
    v2[300_000..400_000].copy_from_slice(&bytes(78, 100_000));
    v2.extend_from_slice(b"appended");
    let (f1, f2) = (w.join("core-1107"), w.join("core-1108"));
    std::fs::write(&f1, &v1).unwrap();
    std::fs::write(&f2, &v2).unwrap();
    let out = cli_publish(&r, "core", "1107", &f1, &["--requires", "kernel=>=6.12", "--built-from", "main@abc"]).unwrap();
    assert!(out.contains("published core 1107 to channel stable"), "{out}");
    // pieces of 512 KiB through the library entry point (the command uses 32 MiB): 1.2 MB goes up as three requests
    let up = iyakki_proxy::https::UreqUpstream::default();
    let req2 = super::compcli::PublishReq { component: "core".into(), version: "1108".into(), channel: "stable".into(), file: f2.clone(), requires: BTreeMap::new(), built_from: "main@def".into(), install_path: String::new(), release: None, delta_from: vec![f1.clone()] };
    let mut log = vec![];
    let msg = super::compcli::publish_with_chunk(&up, &r.base(), &r.ptoken, &req2, 512 << 10, &mut |s| log.push(s.to_string())).unwrap();
    assert!(msg.contains("published core 1108"), "{msg}");
    assert!(log.iter().any(|l| l.contains("delta from")) && log.iter().filter(|l| l.contains(" of ")).count() >= 3, "{log:?}");
    // the same bytes as another version: the blobs are already there, so nothing goes up again
    log.clear();
    super::compcli::publish_with_chunk(&up, &r.base(), &r.ptoken, &super::compcli::PublishReq { version: "1109".into(), ..req2.clone() }, 512 << 10, &mut |s| log.push(s.to_string())).unwrap();
    assert!(log.iter().filter(|l| l.contains("already on the store")).count() == 2 && !log.iter().any(|l| l.contains(" of ")), "{log:?}");
    // the listing
    let listing = super::compcli::run(&["component".into(), "list".into(), "--server".into(), r.base(), "--key".into(), r.srv.catalog_key()]).unwrap();
    assert!(listing.contains("signature checked") && listing.contains("core") && listing.contains("1108") && listing.contains("deltas from"), "{listing}");
    assert!(super::compcli::run(&["component".into(), "list".into(), "--server".into(), r.base()]).unwrap().contains("NOT VERIFIED"));
    // the device has 1107 installed
    let store = r.store();
    let doc = comps::fetch_doc(&iyakki_proxy::https::UreqUpstream::default(), &store).unwrap();
    let mut inst = Installed::default();
    inst.versions.insert("core".into(), "1107".into());
    inst.shas.insert("core".into(), sha(&v1));
    inst.files.insert("core".into(), f1.clone());
    inst.extras.insert("x".into(), "y".into());
    let plan = comps::plan(&doc, &store.name, "stable", &inst, None).unwrap();
    assert_eq!((plan.steps.len(), plan.steps[0].via), (1, Via::Delta), "{}", comps::plan_text(&plan));
    assert!(plan.steps[0].download < 200_000, "the difference is small: {}", plan.steps[0].download);
    // wrong key: the list is refused before anything else
    let bad = PinnedStore { pin: public_key_hex(&new_signing_key().unwrap()), ..store.clone() };
    assert!(comps::fetch_doc(&iyakki_proxy::https::UreqUpstream::default(), &bad).unwrap_err().contains("not signed by the key you confirmed"));
    // stage: the delta is cut after 20000 bytes, the second try asks for the rest with Range
    let cut = Arc::new(Cut { inner: streamer(), cut: AtomicU64::new(20_000), seen: Mutex::new(vec![]) });
    let mut shown = vec![];
    let staged = comps::stage(&plan, &store.base, cut.as_ref(), &inst, &w.join("staging"), 2, &mut |c, a, b| shown.push((c.to_string(), a, b)), &|| false).unwrap();
    let seen = cut.seen.lock().unwrap().clone();
    assert!(seen[0].iter().all(|(k, _)| k != "Range") && seen[1].iter().any(|(k, v)| k == "Range" && v == "bytes=20000-"), "{seen:?}");
    assert_eq!(std::fs::read(&staged.items[0].file).unwrap(), v2, "the file made from the difference is the new core");
    assert!(staged.items[0].note.contains("made from the installed core"), "{}", staged.items[0].note);
    assert!(shown.last().map(|s| s.1 == s.2).unwrap_or(false));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_corrupted_blob_a_lying_delta_and_a_missing_blob_are_all_refused() {
    let r = rig("corrupt", |_| {});
    let w = dir("corrupt-work");
    let v1 = bytes(88, 400_000);
    let mut v2 = v1.clone();
    v2[1000] ^= 1;
    let (f1, f2) = (w.join("a"), w.join("b"));
    std::fs::write(&f1, &v1).unwrap();
    std::fs::write(&f2, &v2).unwrap();
    cli_publish(&r, "base", "1", &f1, &[]).unwrap();
    cli_publish(&r, "base", "2", &f2, &["--delta-from", &f1.display().to_string()]).unwrap();
    let store = r.store();
    let doc = comps::fetch_doc(&iyakki_proxy::https::UreqUpstream::default(), &store).unwrap();
    // the device believes it has version 1, but the file on its disk is something else: the delta cannot be applied, the whole file is fetched
    let wrong = w.join("wrong");
    std::fs::write(&wrong, bytes(1, 400_000)).unwrap();
    let mut inst = Installed::default();
    inst.versions.insert("base".into(), "1".into());
    inst.shas.insert("base".into(), sha(&v1));
    inst.files.insert("base".into(), wrong);
    let plan = comps::plan(&doc, &store.name, "stable", &inst, None).unwrap();
    assert_eq!(plan.steps[0].via, Via::Delta);
    let staged = comps::stage(&plan, &store.base, streamer().as_ref(), &inst, &w.join("s1"), 0, &mut |_, _, _| {}, &|| false).unwrap();
    assert_eq!(std::fs::read(&staged.items[0].file).unwrap(), v2);
    assert!(staged.items[0].note.contains("could not be used") && staged.items[0].note.contains("not the version this delta was made against"), "{}", staged.items[0].note);
    // a changed byte on the store's disk: the download is refused, no final file is left
    let target = r.data().join("blobs").join(sha(&v2));
    let mut damaged = std::fs::read(&target).unwrap();
    damaged[5] ^= 0xff;
    std::fs::write(&target, damaged).unwrap();
    let inst2 = Installed::default();
    let plan2 = comps::plan(&doc, &store.name, "stable", &inst2, None).unwrap();
    let e = comps::stage(&plan2, &store.base, streamer().as_ref(), &inst2, &w.join("s2"), 0, &mut |_, _, _| {}, &|| false).unwrap_err();
    assert!(e.contains("does not match its SHA-256"), "{e}");
    assert!(!w.join("s2").join(sha(&v2)).exists() && !w.join("s2").join(format!("{}.part", sha(&v2))).exists());
    // the blob is gone from the store (cleaned up): said plainly
    std::fs::remove_file(&target).unwrap();
    let e = comps::stage(&plan2, &store.base, streamer().as_ref(), &inst2, &w.join("s3"), 0, &mut |_, _, _| {}, &|| false).unwrap_err();
    assert!(e.contains("no longer has this file"), "{e}");
    // cancelling keeps what was fetched
    let e = comps::stage(&plan2, &store.base, streamer().as_ref(), &inst2, &w.join("s4"), 0, &mut |_, _, _| {}, &|| true).unwrap_err();
    assert!(e.contains("cancelled") || e.contains("no longer has"), "{e}");
    let _ = std::fs::remove_dir_all(&w);
}

// ---------------------------------------------------------------------------------------- hand-over

fn staged_item(comp: &str, ver: &str, install: &str, file: &Path) -> comps::StagedItem {
    let b = std::fs::read(file).unwrap();
    comps::StagedItem { step: comps::Step { component: comp.into(), from: None, to: ver.into(), via: Via::Full, blob_sha256: sha(&b), download: b.len() as u64, sha256: sha(&b), size: b.len() as u64, full_download: b.len() as u64, install_path: install.into(), restart: cm::restart_of(comp), requires: BTreeMap::new() }, file: file.to_path_buf(), note: String::new() }
}

#[test]
fn a_boot_only_update_carries_the_running_system_image_not_the_stale_copy_on_the_boot_partition() {
    // a lean install: the boot partition still holds the root image of the FIRST install, the running one lives on the data volume. A payload
    // made from the boot partition alone would switch the next start back to the old image (seen on the Mac mini, 2026-11-08).
    let w = dir("handover-root");
    let rest = w.join("rest");
    for (p, n) in [("boot/bzImage", 1u64), ("boot/grub/grub.cfg", 2), ("boot/initrd-lean", 3), ("boot/root.sfs", 4)] {
        std::fs::create_dir_all(rest.join(p).parent().unwrap()).unwrap();
        std::fs::write(rest.join(p), bytes(n, 50_000)).unwrap();
    }
    let active = w.join("active-root.sfs");
    std::fs::write(&active, bytes(99, 50_000)).unwrap();
    let new_initrd = w.join("i");
    std::fs::write(&new_initrd, bytes(70, 20_000)).unwrap();
    let staged = comps::Staged { dir: w.join("st"), items: vec![staged_item("boot", "1107.15", "boot/initrd-lean", &new_initrd)] };
    let rel = Release { version: "0.1.0".into(), variant: "pc".into() };
    let ho = comps::handover_with_root(&staged, &w.join("out"), &rel, Some(&rest), Some(&active), &Installed::default()).unwrap();
    let pay = ho.payload.unwrap();
    assert_eq!(std::fs::read(pay.join("boot/root.sfs")).unwrap(), std::fs::read(&active).unwrap(), "the payload carries the stale root image");
    assert_ne!(std::fs::read(pay.join("boot/root.sfs")).unwrap(), std::fs::read(rest.join("boot/root.sfs")).unwrap());
    assert_eq!(std::fs::read(pay.join("boot/initrd-lean")).unwrap(), std::fs::read(&new_initrd).unwrap());
    let m = crate::devmode::update::read_manifest(&pay).unwrap();
    crate::devmode::update::verify_payload(&pay, &m).unwrap();
    // the stale file on the boot partition was not touched
    assert_eq!(std::fs::read(rest.join("boot/root.sfs")).unwrap(), bytes(4, 50_000));
    // a base part in the same update brings its own image: it is not replaced
    let new_base = w.join("b");
    std::fs::write(&new_base, bytes(71, 50_000)).unwrap();
    let staged2 = comps::Staged { dir: w.join("st2"), items: vec![staged_item("base", "1107.15", "boot/root.sfs", &new_base)] };
    let ho2 = comps::handover_with_root(&staged2, &w.join("out2"), &rel, Some(&rest), Some(&active), &Installed::default()).unwrap();
    assert_eq!(std::fs::read(ho2.payload.unwrap().join("boot/root.sfs")).unwrap(), std::fs::read(&new_base).unwrap());
}

#[test]
fn verified_boot_parts_become_an_update_payload_the_update_machinery_accepts_and_core_becomes_a_patch_pack() {
    let w = dir("handover");
    // what is installed on the boot partition: kernel, menu, start RAM disk, root image
    let rest = w.join("rest");
    for (p, n) in [("boot/bzImage", 1u64), ("boot/grub/grub.cfg", 2), ("boot/initrd-lean", 3), ("boot/root.sfs", 4)] {
        std::fs::create_dir_all(rest.join(p).parent().unwrap()).unwrap();
        std::fs::write(rest.join(p), bytes(n, 50_000)).unwrap();
    }
    let new_kernel = w.join("k");
    std::fs::write(&new_kernel, bytes(50, 60_000)).unwrap();
    let old_core = w.join("core-old");
    let mut core = bytes(60, 300_000);
    std::fs::write(&old_core, &core).unwrap();
    core[100] ^= 1;
    let new_core = w.join("core-new");
    std::fs::write(&new_core, &core).unwrap();
    let staged = comps::Staged { dir: w.join("st"), items: vec![staged_item("kernel", "6.12.5", "boot/bzImage", &new_kernel), staged_item("core", "1108", "/usr/bin/iyakki", &new_core)] };
    let mut inst = Installed::default();
    inst.files.insert("core".into(), old_core);
    let rel = Release { version: "0.1.0".into(), variant: "pc".into() };
    let ho = comps::handover(&staged, &w.join("out"), &rel, Some(&rest), &inst).unwrap();
    // the payload: the new kernel, the rest from the installed boot files, a manifest the Update centre reads and verifies
    let pay = ho.payload.clone().unwrap();
    let m = crate::devmode::update::read_manifest(&pay).unwrap();
    crate::devmode::update::verify_payload(&pay, &m).unwrap();
    assert_eq!((m.version.as_str(), m.variant.as_str()), ("0.1.0", "pc"));
    assert_eq!(m.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["boot/bzImage", "boot/grub/grub.cfg", "boot/initrd-lean", "boot/root.sfs"]);
    assert_eq!(std::fs::read(pay.join("boot/bzImage")).unwrap(), std::fs::read(&new_kernel).unwrap());
    assert_eq!(std::fs::read(pay.join("boot/root.sfs")).unwrap(), std::fs::read(rest.join("boot/root.sfs")).unwrap());
    let found = crate::devmode::centre::read_pack(&pay, None).unwrap();
    assert_eq!(found.manifest.files.len(), 4);
    // the patch pack: one replace of the program, carried as a small delta against the installed file
    let pack = crate::devmode::patch::decode(&std::fs::read(ho.patch_pack.clone().unwrap()).unwrap()).unwrap();
    assert_eq!((pack.manifest.ops.len(), pack.manifest.ops[0].path.as_str()), (1, "/usr/bin/iyakki"));
    assert!(pack.manifest.ops[0].delta.is_some() && pack.manifest.ops[0].size < 5_000, "{}", pack.manifest.ops[0].size);
    assert!(pack.manifest.needs_reboot());
    let text = comps::staged_text(&staged, &ho);
    assert!(text.contains("Update payload for IYAKKI 0.1.0") && text.contains("Patch pack") && text.contains("Nothing was applied"), "{text}");
    // without the installed boot files the payload would be missing parts: refused with the next step
    let e = comps::handover(&staged, &w.join("out2"), &rel, None, &inst).unwrap_err();
    assert!(e.contains("--rest-from"), "{e}");
    // a kernel layer with no install path cannot be handed over
    let mut layer = staged_item("kernel-layer", "1", "", &new_kernel);
    layer.step.install_path.clear();
    let e = comps::handover(&comps::Staged { dir: w.join("st"), items: vec![layer] }, &w.join("out3"), &rel, None, &inst).unwrap_err();
    assert!(e.contains("no install path"), "{e}");
    let _ = std::fs::remove_dir_all(&w);
}

// ---------------------------------------------------------------------------------------- the command line

#[test]
fn the_command_line_plans_stages_lists_withdraws_and_cleans_up() {
    let r = rig("cli", |_| {});
    let w = dir("cli-work");
    let f = w.join("kernel");
    std::fs::write(&f, bytes(31, 80_000)).unwrap();
    // no token in the environment: told how
    let e = super::compcli::run(&["component".into(), "publish".into(), "--server".into(), r.base(), "--token-env".into(), "IYAKKI_TEST_NO_SUCH_COMP_TOKEN".into(), "--component".into(), "kernel".into(), "--version".into(), "6.12.5".into(), "--file".into(), f.display().to_string()]).unwrap_err();
    assert!(e.contains("not set") && e.contains("never put the token on the command line"), "{e}");
    assert!(super::compcli::run(&["component".into(), "publish".into(), "--token".into(), "x".into()]).is_err(), "there is no token flag");
    assert!(cli_publish(&r, "kernel", "6.12.5", &f, &["--requires", "badformat"]).unwrap_err().contains("name=range"));
    assert!(cli_publish(&r, "kernel", "6.12.5", &f, &["--requires", "core=banana"]).unwrap_err().contains("requires core"));
    cli_publish(&r, "kernel", "6.12.5", &f, &[]).unwrap();
    let wrong_token = super::compcli::publish_component(&iyakki_proxy::https::UreqUpstream::default(), &r.base(), "iyk_wrong", &super::compcli::PublishReq { component: "kernel".into(), version: "6.12.6".into(), channel: "stable".into(), file: f.clone(), requires: BTreeMap::new(), built_from: String::new(), install_path: String::new(), release: None, delta_from: vec![] }, &mut |_| {}).unwrap_err();
    assert!(wrong_token.contains("did not accept the token"), "{wrong_token}");
    // `plan` on the device side: from the store, with the pinned key; nothing installed on record
    let args = |extra: &[&str]| -> Vec<String> { ["component", "plan", "--server", &r.base(), "--key", &r.srv.catalog_key()].iter().chain(extra.iter()).map(|s| s.to_string()).collect() };
    let t = super::compcli::run(&args(&[])).unwrap();
    assert!(t.contains("kernel (not on record) -> 6.12.5") && t.contains("Nothing has been downloaded or installed"), "{t}");
    assert!(super::compcli::run(&args(&["--channel", "beta"])).unwrap().contains("Nothing to update"));
    let mut bad_key = args(&[]);
    bad_key[5] = public_key_hex(&new_signing_key().unwrap());
    assert!(super::compcli::run(&bad_key).unwrap_err().contains("not signed by the key you confirmed"));
    // plan with --stage: downloads, checks, and says it applied nothing; the boot files it completes come from --rest-from
    let rest = w.join("rest");
    for p in ["boot/grub/grub.cfg", "boot/initrd-lean", "boot/root.sfs"] {
        std::fs::create_dir_all(rest.join(p).parent().unwrap()).unwrap();
        std::fs::write(rest.join(p), p.as_bytes()).unwrap();
    }
    let out = w.join("out");
    let t = super::compcli::run(&args(&["--stage", &out.display().to_string(), "--rest-from", &rest.display().to_string(), "--version", "0.2.0", "--variant", "pc"])).unwrap();
    assert!(t.contains("Verified and staged") && t.contains("Update payload for IYAKKI 0.2.0") && t.contains("Nothing was applied"), "{t}");
    assert!(out.join("update-0.2.0/update.json").is_file() && out.join("update-0.2.0/boot/bzImage").is_file());
    // withdraw and clean up
    let var = format!("IYAKKI_TEST_COMP_TOKEN_{}", r.addr.port());
    std::env::set_var(&var, ADMIN);
    let line = |v: &[&str]| -> Vec<String> { ["component"].iter().chain(v.iter()).map(|s| s.to_string()).collect() };
    let y = super::compcli::run(&line(&["yank", "--server", &r.base(), "--token-env", &var, "--component", "kernel", "--version", "6.12.5", "--reason", "boots to a black screen"])).unwrap();
    assert!(y.contains("withdrawn 1 version"), "{y}");
    assert!(super::compcli::run(&line(&["list", "--server", &r.base(), "--key", &r.srv.catalog_key()])).unwrap().contains("withdrawn: kernel 6.12.5 (stable): boots to a black screen"));
    let dry = super::compcli::run(&line(&["retention", "--server", &r.base(), "--token-env", &var, "--keep", "2"])).unwrap();
    assert!(dry.contains("would be removed") && dry.contains("files removed (withdrawal notice stays): kernel 6.12.5") && dry.contains("add --apply"), "{dry}");
    let done = super::compcli::run(&line(&["retention", "--server", &r.base(), "--token-env", &var, "--keep", "2", "--apply"])).unwrap();
    assert!(done.contains("were removed"), "{done}");
    assert!(!r.data().join("blobs").join(sha(&std::fs::read(&f).unwrap())).exists());
    // a clear-text address that is not a private one is refused before the token is sent
    let e = super::compcli::run(&line(&["yank", "--server", "http://store.example.com", "--token-env", &var, "--component", "kernel"])).unwrap_err();
    assert!(e.contains("unencrypted"), "{e}");
    assert!(super::compcli::run(&["frobnicate".into()]).is_err() && super::compcli::run(&line(&["nothing"])).unwrap_err().contains("unknown command"));
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn the_installed_record_round_trips_and_a_damaged_one_reads_as_empty() {
    let d = dir("installed");
    assert_eq!(Installed::load(&d), Installed::default());
    let mut i = Installed::default();
    i.versions.insert("core".into(), "1107".into());
    i.shas.insert("core".into(), h(7));
    i.save(&d).unwrap();
    assert_eq!(Installed::load(&d), i);
    std::fs::write(d.join(comps::INSTALLED_FILE), b"{broken").unwrap();
    assert_eq!(Installed::load(&d), Installed::default());
    let _ = std::fs::remove_dir_all(&d);
}


// ---------------------------------------------------------------------------------------- the release label

#[test]
fn the_signed_list_carries_the_release_label_and_it_only_goes_up() {
    let r = rig("release", |_| {});
    let k1 = bytes(1, 4000);
    // no label yet: the list has none
    assert_eq!(publish(&r, &r.ptoken, "kernel", "6.12.5", "stable", &k1, &[], &[]).status, 200);
    let get = |r: &Rig| r.srv.handle(&req("GET", "/v1/components.json", None, b""), "10.0.0.1");
    assert!(cm::verify(&get(&r).body, &r.srv.catalog_key()).unwrap().release.is_none());
    // a publish that names a release puts it in the signed list
    let k2 = bytes(2, 4100);
    let (_, up) = put_blob(&r, &r.ptoken, "kernel", &k2);
    assert!(up.status == 200 || up.status == 201);
    let mut e = entry_json("kernel", "6.12.6", "stable", &k2, &[], &[]);
    e["release"] = serde_json::json!({"version": "0.1.0-alpha.3", "variant": "pc"});
    assert_eq!(post_entry(&r, &r.ptoken, &e).status, 200);
    let body = get(&r).body;
    let doc = cm::verify(&body, &r.srv.catalog_key()).unwrap();
    assert_eq!(doc.release, Some(cm::ReleaseLabel { version: "0.1.0-alpha.3".into(), variant: "pc".into() }));
    // the label is signed: changing it breaks the signature
    let text = String::from_utf8_lossy(&body).into_owned();
    assert!(cm::verify(text.replace("0.1.0-alpha.3", "0.1.0-alpha.9").as_bytes(), &r.srv.catalog_key()).is_err());
    assert!(cm::verify(text.replace("\"pc\"", "\"mac\"").as_bytes(), &r.srv.catalog_key()).is_err());
    // an older release is refused, a bad one is not accepted, a later one replaces it
    let k3 = bytes(3, 4200);
    let (_, up) = put_blob(&r, &r.ptoken, "kernel", &k3);
    assert!(up.status == 200 || up.status == 201);
    let mut e3 = entry_json("kernel", "6.12.7", "stable", &k3, &[], &[]);
    e3["release"] = serde_json::json!({"version": "0.1.0-alpha.2", "variant": "pc"});
    let older = post_entry(&r, &r.ptoken, &e3);
    assert_eq!(older.status, 409, "{}", older.text());
    e3["release"] = serde_json::json!({"version": "not a version", "variant": "pc"});
    assert_eq!(post_entry(&r, &r.ptoken, &e3).status, 422);
    e3["release"] = serde_json::json!({"version": "0.1.0-alpha.4", "variant": "p c"});
    assert_eq!(post_entry(&r, &r.ptoken, &e3).status, 422);
    e3["release"] = serde_json::json!({"version": "0.1.0-alpha.4", "variant": "pc"});
    assert_eq!(post_entry(&r, &r.ptoken, &e3).status, 200);
    assert_eq!(cm::verify(&get(&r).body, &r.srv.catalog_key()).unwrap().release.unwrap().version, "0.1.0-alpha.4");
}

#[test]
fn the_staged_payload_is_labelled_with_the_signed_release_unless_flags_override() {
    let r = rig("release-cli", |_| {});
    let w = dir("release-cli-work");
    let f = w.join("kernel");
    std::fs::write(&f, bytes(31, 80_000)).unwrap();
    // publishing with only one of the two flags is refused
    assert!(cli_publish(&r, "kernel", "6.12.5", &f, &["--release", "0.2.1"]).unwrap_err().contains("go together"));
    cli_publish(&r, "kernel", "6.12.5", &f, &["--release", "0.2.1", "--release-variant", "pc"]).unwrap();
    let args = |extra: &[&str]| -> Vec<String> { ["component", "plan", "--server", &r.base(), "--key", &r.srv.catalog_key()].iter().chain(extra.iter()).map(|s| s.to_string()).collect() };
    let rest = w.join("rest");
    for p in ["boot/grub/grub.cfg", "boot/initrd-lean", "boot/root.sfs"] {
        std::fs::create_dir_all(rest.join(p).parent().unwrap()).unwrap();
        std::fs::write(rest.join(p), p.as_bytes()).unwrap();
    }
    // no --version/--variant: the signed label is used
    let out = w.join("out");
    let t = super::compcli::run(&args(&["--stage", &out.display().to_string(), "--rest-from", &rest.display().to_string()])).unwrap();
    assert!(t.contains("Update payload for IYAKKI 0.2.1 (pc)"), "{t}");
    assert!(out.join("update-0.2.1/update.json").is_file());
    // the flags override it
    let out2 = w.join("out2");
    let t = super::compcli::run(&args(&["--stage", &out2.display().to_string(), "--rest-from", &rest.display().to_string(), "--version", "0.2.2", "--variant", "mac"])).unwrap();
    assert!(t.contains("IYAKKI 0.2.2 (mac)"), "{t}");
    // a store with no label, and no flags: told what to do
    let r2 = rig("release-none", |_| {});
    cli_publish(&r2, "kernel", "6.12.5", &f, &[]).unwrap();
    let args2: Vec<String> = ["component", "plan", "--server", &r2.base(), "--key", &r2.srv.catalog_key(), "--stage", &w.join("out3").display().to_string(), "--rest-from", &rest.display().to_string()].iter().map(|s| s.to_string()).collect();
    assert!(super::compcli::run(&args2).unwrap_err().contains("carries no release label"));
    let _ = std::fs::remove_dir_all(&w);
}
