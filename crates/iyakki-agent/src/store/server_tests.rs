//! Tests of the private store server: request handling in process, and a few rounds over a real loopback socket.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use super::catalog::{sha256_hex, Catalog, CatalogSigner};
use super::server::*;
use super::trust::Roots;
use crate::pkg::{build_skill, new_signing_key, public_key_hex, resign, signing_key_hex};

const NOW: i64 = 1_800_000_000;
const ADMIN: &str = "admin-token-for-tests-0123456789";

struct Fx {
    srv: Arc<StoreServer>,
    dir: std::path::PathBuf,
    clock: Arc<AtomicI64>,
    ptoken: String,
}

fn fx(tag: &str, tweak: impl Fn(&mut Config)) -> Fx {
    let dir = std::env::temp_dir().join(format!("iyakki-srv-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::new(&dir);
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    tweak(&mut cfg);
    let clock = Arc::new(AtomicI64::new(NOW));
    let c2 = clock.clone();
    let key = new_signing_key().unwrap();
    let srv = Arc::new(StoreServer::open(cfg, &signing_key_hex(&key)).unwrap().with_clock(Arc::new(move || c2.load(Ordering::SeqCst))));
    let ptoken = srv.create_publisher("acme", "Acme Ltd").unwrap();
    Fx { srv, dir, clock, ptoken }
}

fn req(method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Req {
    let mut headers = HashMap::new();
    if let Some(t) = token {
        headers.insert("authorization".to_string(), format!("Bearer {t}"));
    }
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let query = q.split('&').filter_map(|x| x.split_once('=')).map(|(k, v)| (k.to_string(), v.replace('+', " "))).collect();
    Req { method: method.into(), path: p.into(), query, headers, body: body.to_vec() }
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Test skill {name}\ntriggers: test\n---\nDo the thing.\n")
}

fn skill(name: &str, version: &str, key: &ed25519_dalek::SigningKey) -> String {
    let t = build_skill(&skill_md(name), version, "MIT", Some(key)).unwrap();
    resign(&t, Some(&format!(r#"{{"title":"{name}","summary":"A test skill"}}"#)), None, key).unwrap()
}

fn publish(f: &Fx, token: &str, pkg: &str) -> Resp {
    f.srv.handle(&req("POST", "/v1/publish", Some(token), pkg.as_bytes()), "10.0.0.1")
}

fn json(r: &Resp) -> serde_json::Value {
    serde_json::from_slice(&r.body).unwrap_or_default()
}

fn catalog_of(f: &Fx) -> (Catalog, super::catalog::CatalogCheck) {
    let r = f.srv.handle(&req("GET", "/catalog.json", None, b""), "10.0.0.2");
    assert_eq!(r.status, 200);
    Catalog::parse(&r.text(), &Roots::default(), &[], NOW, true).unwrap()
}

#[test]
fn a_good_package_is_stored_listed_and_the_catalog_verifies_with_the_device_code() {
    let f = fx("good", |_| {});
    let k = new_signing_key().unwrap();
    let r = publish(&f, &f.ptoken, &skill("meeting-notes", "1.0.0", &k));
    assert_eq!(r.status, 200, "{}", r.text());
    let v = json(&r);
    assert_eq!((v["name"].as_str(), v["trust"].as_str()), (Some("meeting-notes"), Some("Community")), "an unknown key is Community");
    // the catalog parses with the device-side code, signed by the store key, and names the publisher
    let (c, check) = catalog_of(&f);
    assert!(matches!(check.signer, CatalogSigner::Unknown(_)), "{:?}", check.signer);
    assert_eq!(c.key, f.srv.catalog_key());
    let e = c.find("meeting-notes").unwrap();
    assert_eq!((e.meta.publisher.id.as_str(), e.kind.as_str(), e.signer.clone()), ("acme", "skill", public_key_hex(&k)));
    // a pinned catalog key makes the device trust the catalog, and the package is served by its hash
    let pkg = f.srv.handle(&req("GET", &format!("/{}", e.url), None, b""), "10.0.0.2");
    assert_eq!(pkg.status, 200);
    assert_eq!((sha256_hex(&pkg.body), pkg.body.len() as u64), (e.sha256.clone(), e.size));
    assert!(crate::pkg::parse(&pkg.text()).is_ok());
    let h = f.srv.handle(&req("GET", &format!("/{}", e.url), None, b""), "10.0.0.2").headers.iter().find(|h| h.0 == "Cache-Control").unwrap().1.clone();
    assert!(h.contains("immutable"));
    let info = json(&f.srv.handle(&req("GET", "/v1/info", None, b""), "10.0.0.2"));
    assert_eq!(info["key"].as_str().unwrap(), f.srv.catalog_key());
    assert_eq!(info["fingerprint"].as_str().unwrap(), fingerprint(&f.srv.catalog_key()));
    assert_eq!(fingerprint(&f.srv.catalog_key()).split(' ').count(), 8);
    assert_eq!(f.srv.handle(&req("GET", "/healthz", None, b""), "x").status, 200);
}

#[test]
fn a_key_the_administrator_trusts_makes_the_package_trusted_and_require_trusted_refuses_strangers() {
    let f = fx("trusted", |_| {});
    let k = new_signing_key().unwrap();
    std::fs::write(f.dir.join("trusted-keys.txt"), format!("{} Acme release key\n", public_key_hex(&k))).unwrap();
    let r = publish(&f, &f.ptoken, &skill("trusted-one", "1.0.0", &k));
    assert_eq!(json(&r)["trust"], "Trusted", "{}", r.text());
    let strict = fx("strict", |c| c.allow_community = false);
    let stranger = new_signing_key().unwrap();
    let r = publish(&strict, &strict.ptoken, &skill("stranger", "1.0.0", &stranger));
    assert_eq!(r.status, 422);
    assert!(r.text().contains("trusts"), "{}", r.text());
}

#[test]
fn bad_signatures_tampered_hashes_unsigned_and_wrong_kinds_are_refused() {
    let f = fx("bad", |_| {});
    let k = new_signing_key().unwrap();
    let good = skill("some-skill", "1.0.0", &k);
    // a flipped signature
    let sig = good.lines().find(|l| l.starts_with("signature: ")).unwrap().to_string();
    let flipped = format!("signature: {}{}", &sig[11..sig.len() - 1], if sig.ends_with('0') { '1' } else { '0' });
    let r = publish(&f, &f.ptoken, &good.replace(&sig, &flipped));
    assert_eq!(r.status, 422, "{}", r.text());
    assert!(r.text().contains("signature"));
    // payload changed after signing: the hash no longer matches
    let r = publish(&f, &f.ptoken, &good.replace("Do the thing.", "Do the other thing."));
    assert_eq!(r.status, 422);
    assert!(r.text().contains("hash"), "{}", r.text());
    // unsigned
    let unsigned = build_skill(&skill_md("unsigned-one"), "1.0.0", "MIT", None).unwrap();
    let r = publish(&f, &f.ptoken, &unsigned);
    assert_eq!(r.status, 422);
    assert!(r.text().contains("not signed"));
    // garbage
    assert_eq!(publish(&f, &f.ptoken, "hello").status, 422);
    // a kind the store does not take
    let cli = crate::pkg::build_generic("theme", "sunset", "1.0.0", "MIT", "{}", None, None, Some(&k)).unwrap();
    assert_eq!(publish(&f, &f.ptoken, &cli).status, 200, "themes are fine");
    let narrow = fx("kinds", |c| c.kinds = vec!["skill".into()]);
    assert_eq!(publish(&narrow, &narrow.ptoken, &cli).status, 422);
    assert!(catalog_of(&f).0.find("some-skill").is_none());
}

#[test]
fn versions_only_go_up_names_belong_to_their_first_publisher_and_look_alikes_are_refused() {
    let f = fx("names", |_| {});
    let bob = f.srv.create_publisher("bob", "Bob").unwrap();
    let k = new_signing_key().unwrap();
    assert_eq!(publish(&f, &f.ptoken, &skill("report-writer", "1.2.0", &k)).status, 200);
    let same = publish(&f, &f.ptoken, &skill("report-writer", "1.2.0", &k));
    assert_eq!(same.status, 409);
    assert!(same.text().contains("versions only go up"));
    assert_eq!(publish(&f, &f.ptoken, &skill("report-writer", "1.1.9", &k)).status, 409);
    assert_eq!(publish(&f, &f.ptoken, &skill("report-writer", "1.10.0", &k)).status, 200, "1.10 is newer than 1.2");
    // another publisher cannot take the name, nor a look-alike
    assert_eq!(publish(&f, &bob, &skill("report-writer", "9.0.0", &k)).status, 403);
    let r = publish(&f, &bob, &skill("report-wr1ter", "1.0.0", &k));
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.text().contains("too close"));
    // the catalog lists the highest version only
    let (c, _) = catalog_of(&f);
    assert_eq!(c.entries.len(), 1);
    assert_eq!(c.find("report-writer").unwrap().version, "1.10.0");
    // store information may not name somebody else as the publisher
    let spoof = resign(&build_skill(&skill_md("spoofed"), "1.0.0", "MIT", Some(&k)).unwrap(), Some(r#"{"publisher":{"id":"bob","name":"Bob"}}"#), None, &k).unwrap();
    assert_eq!(publish(&f, &f.ptoken, &spoof).status, 403);
}

#[test]
fn oversize_bodies_secrets_and_bad_tokens_are_refused() {
    let f = fx("limits", |c| c.max_body = 1024);
    let k = new_signing_key().unwrap();
    let big = format!("{}{}", skill("fat-skill", "1.0.0", &k), "x".repeat(2000));
    let r = publish(&f, &f.ptoken, &big);
    assert_eq!(r.status, 413);
    // tokens
    let small = skill("small-skill", "1.0.0", &k);
    assert_eq!(f.srv.handle(&req("POST", "/v1/publish", None, small.as_bytes()), "1.1.1.1").status, 401);
    assert_eq!(publish(&f, "iyk_wrong", &small).status, 401);
    assert_eq!(publish(&f, &f.ptoken, &small).status, 200);
    // a token sent as a query parameter is not a token
    assert_eq!(f.srv.handle(&req("POST", &format!("/v1/publish?token={}", f.ptoken), None, small.as_bytes()), "1.1.1.1").status, 401);
    // secrets in the payload
    let leaky_md = "---\nname: leaky\ndescription: Leaky\ntriggers: x\n---\nUse the token ghp_abcdefghijklmnopqrstuvwxyz0123456789 to call it.\n";
    let leaky = resign(&build_skill(leaky_md, "1.0.0", "MIT", Some(&k)).unwrap(), None, None, &k).unwrap();
    let r = publish(&f, &f.ptoken, &leaky);
    assert_eq!(r.status, 422);
    assert!(r.text().contains("access token"), "{}", r.text());
    assert!(!r.text().contains("ghp_abc"), "the secret is never echoed");
}

#[test]
fn failed_sign_ins_and_floods_of_changes_are_rate_limited_per_minute() {
    let f = fx("rate", |c| {
        c.fails_per_min = 3;
        c.writes_per_min = 5;
    });
    let k = new_signing_key().unwrap();
    let p = skill("one-skill", "1.0.0", &k);
    let codes: Vec<u16> = (0..5).map(|_| f.srv.handle(&req("POST", "/v1/publish", Some("nope"), p.as_bytes()), "6.6.6.6").status).collect();
    assert_eq!(codes[..3], [401, 401, 401]);
    assert_eq!(codes[3], 429);
    // the guesser is locked out even with the right token; another address is not
    assert_eq!(f.srv.handle(&req("POST", "/v1/publish", Some(&f.ptoken), p.as_bytes()), "6.6.6.6").status, 429);
    assert_eq!(f.srv.handle(&req("POST", "/v1/publish", Some(&f.ptoken), p.as_bytes()), "7.7.7.7").status, 200);
    // after a minute it is fine again
    f.clock.fetch_add(61, Ordering::SeqCst);
    assert_eq!(f.srv.handle(&req("POST", "/v1/publish", Some(&f.ptoken), skill("two-skill", "1.0.0", &k).as_bytes()), "6.6.6.6").status, 200);
    // too many changes
    let mut last = 0;
    for i in 0..8 {
        last = f.srv.handle(&req("POST", "/v1/publish", Some(&f.ptoken), skill(&format!("flood-{i}"), "1.0.0", &k).as_bytes()), "8.8.8.8").status;
    }
    assert_eq!(last, 429);
    assert!(std::fs::read_to_string(f.dir.join("audit.log")).unwrap().contains("rate-limit"));
}

#[test]
fn yanking_removes_the_package_from_the_catalog_and_signs_the_list_and_the_version_stays_burnt() {
    let f = fx("yank", |_| {});
    let k = new_signing_key().unwrap();
    publish(&f, &f.ptoken, &skill("old-thing", "1.0.0", &k));
    publish(&f, &f.ptoken, &skill("old-thing", "1.1.0", &k));
    let serial_before = catalog_of(&f).0.serial;
    // only the administrator may yank
    assert_eq!(f.srv.handle(&req("DELETE", "/v1/packages/old-thing/1.1.0", Some(&f.ptoken), b""), "9.9.9.9").status, 401);
    let r = f.srv.handle(&req("DELETE", "/v1/packages/old-thing/1.1.0?reason=bad+build", Some(ADMIN), b""), "9.9.9.9");
    assert_eq!(r.status, 200, "{}", r.text());
    let (c, _) = catalog_of(&f);
    assert!(c.serial > serial_before, "the catalog was re-signed with a higher serial");
    assert_eq!(c.find("old-thing").unwrap().version, "1.0.0", "the catalog falls back to the previous version");
    // the signed yanked list
    let y: serde_json::Value = serde_json::from_slice(&f.srv.handle(&req("GET", "/yanked.json", None, b""), "x").body).unwrap();
    assert!(super::trust::verify_doc("yanked", &y, y["key"].as_str().unwrap(), y["sig"].as_str().unwrap()));
    assert_eq!(y["key"].as_str().unwrap(), f.srv.catalog_key());
    assert_eq!(y["packages"][0]["version"], "1.1.0");
    assert_eq!(y["packages"][0]["reason"], "bad build");
    // that version cannot come back
    assert_eq!(publish(&f, &f.ptoken, &skill("old-thing", "1.1.0", &k)).status, 409);
    // yank everything
    assert_eq!(f.srv.handle(&req("DELETE", "/v1/packages/old-thing", Some(ADMIN), b""), "9.9.9.9").status, 200);
    assert!(catalog_of(&f).0.entries.is_empty());
    assert_eq!(f.srv.handle(&req("DELETE", "/v1/packages/old-thing", Some(ADMIN), b""), "9.9.9.9").status, 404);
    assert_eq!(f.srv.handle(&req("DELETE", "/v1/packages/..%2fx", Some(ADMIN), b""), "9.9.9.9").status, 400);
    let log = std::fs::read_to_string(f.dir.join("audit.log")).unwrap();
    assert!(log.contains(" yank ") && log.contains("old-thing"), "{log}");
}

#[test]
fn concurrent_publishes_are_serialised_and_every_one_lands_in_the_catalog() {
    let f = fx("conc", |c| c.writes_per_min = 1000);
    let k = Arc::new(new_signing_key().unwrap());
    let ts: Vec<_> = (0..12)
        .map(|i| {
            let (s, t, k) = (f.srv.clone(), f.ptoken.clone(), k.clone());
            std::thread::spawn(move || s.handle(&req("POST", "/v1/publish", Some(&t), skill(&format!("par-skill-{i:02}x"), "1.0.0", &k).as_bytes()), "5.5.5.5").status)
        })
        .collect();
    let codes: Vec<u16> = ts.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(codes.iter().all(|c| *c == 200), "{codes:?}");
    let (c, _) = catalog_of(&f);
    assert_eq!(c.entries.len(), 12);
    // the initial serial plus one per change, no gaps and no repeats
    assert_eq!(c.serial, 13);
}

#[test]
fn package_paths_cannot_climb_and_the_home_page_escapes_everything() {
    let f = fx("paths", |_| {});
    let k = new_signing_key().unwrap();
    std::fs::write(f.dir.join("secret.txt"), "top secret").unwrap();
    for p in ["/packages/../secret.txt", "/packages/..%2fsecret.txt", "/packages/secret.txt", "/packages/", "/packages/%2e%2e/secret.txt", "/packages/ABCDEF.iyp"] {
        let r = f.srv.handle(&req("GET", p, None, b""), "1.2.3.4");
        assert_eq!(r.status, 404, "{p}");
        assert!(!r.text().contains("top secret"));
    }
    assert_eq!(f.srv.handle(&req("GET", &format!("/packages/{}.iyp", "0".repeat(64)), None, b""), "1").status, 404);
    // a name with a slash or dots never gets past the package parser
    let evil = build_skill(&skill_md("fine-name"), "1.0.0", "MIT", Some(&k)).unwrap().replace("name: fine-name", "name: ../../etc/passwd");
    assert_eq!(publish(&f, &f.ptoken, &evil).status, 422);
    // the home page: no script, HTML-escaped metadata, trust badge
    let t = resign(&build_skill(&skill_md("pretty-skill"), "1.0.0", "MIT", Some(&k)).unwrap(), Some(r#"{"title":"<script>alert(1)</script>","summary":"a & b"}"#), None, &k).unwrap();
    assert_eq!(publish(&f, &f.ptoken, &t).status, 200);
    let home = f.srv.handle(&req("GET", "/", None, b""), "1");
    let h = home.text();
    assert!(h.contains("&lt;script&gt;") && !h.contains("<script>") && h.contains("Community") && h.contains("a &amp; b"), "{h}");
    assert!(home.headers.iter().any(|x| x.0 == "Content-Security-Policy"));
    // CORS is off: no such header ever, and no OPTIONS
    assert!(!home.headers.iter().any(|x| x.0.starts_with("Access-Control")));
    assert_eq!(f.srv.handle(&req("OPTIONS", "/catalog.json", None, b""), "1").status, 405);
}

#[test]
fn read_only_mode_and_publisher_onboarding() {
    let f = fx("ro", |c| c.read_only = true);
    let k = new_signing_key().unwrap();
    assert_eq!(publish(&f, &f.ptoken, &skill("ro-skill", "1.0.0", &k)).status, 403);
    assert_eq!(f.srv.handle(&req("DELETE", "/v1/packages/ro-skill", Some(ADMIN), b""), "1").status, 403);
    assert_eq!(f.srv.handle(&req("GET", "/catalog.json", None, b""), "1").status, 200);
    // the admin API: only the admin adds publishers; the token is shown once and only its hash is kept
    let f = fx("onboard", |_| {});
    assert_eq!(f.srv.handle(&req("POST", "/v1/publishers", Some(&f.ptoken), br#"{"id":"carol"}"#), "1").status, 401);
    let r = f.srv.handle(&req("POST", "/v1/publishers", Some(ADMIN), br#"{"id":"carol","name":"Carol"}"#), "1");
    assert_eq!(r.status, 201, "{}", r.text());
    let tok = json(&r)["token"].as_str().unwrap().to_string();
    assert_eq!(publish(&f, &tok, &skill("carols-skill", "1.0.0", &new_signing_key().unwrap())).status, 200);
    let stored = std::fs::read_to_string(f.dir.join("publishers.json")).unwrap();
    assert!(!stored.contains(&tok) && stored.contains(&hash_token(&tok)));
    assert_eq!(f.srv.handle(&req("POST", "/v1/publishers", Some(ADMIN), br#"{"id":"carol"}"#), "1").status, 400, "twice");
    assert_eq!(f.srv.handle(&req("POST", "/v1/publishers", Some(ADMIN), br#"{"id":"admin"}"#), "1").status, 400);
    let list = json(&f.srv.handle(&req("GET", "/v1/publishers", Some(ADMIN), b""), "1"));
    assert_eq!(list["publishers"].as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains("token_hash"));
}

#[test]
fn multipart_bodies_work_and_the_audit_log_names_who_did_what() {
    let f = fx("multi", |_| {});
    let k = new_signing_key().unwrap();
    let p = skill("multi-skill", "1.0.0", &k);
    let body = format!("--XX\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi\r\n--XX\r\nContent-Disposition: form-data; name=\"package\"; filename=\"a.iyp\"\r\nContent-Type: text/plain\r\n\r\n{p}\r\n--XX--\r\n");
    let mut r = req("POST", "/v1/publish", Some(&f.ptoken), body.as_bytes());
    r.headers.insert("content-type".into(), "multipart/form-data; boundary=XX".into());
    assert_eq!(f.srv.handle(&r, "3.3.3.3").status, 200);
    publish(&f, "bad", &p);
    let log = std::fs::read_to_string(f.dir.join("audit.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(lines.iter().any(|l| l.contains("3.3.3.3") && l.contains(" acme ") && l.contains("publish") && l.contains("multi-skill 1.0.0")), "{log}");
    assert!(lines.iter().any(|l| l.contains(" auth ") && l.contains("401")), "{log}");
    assert!(!log.contains(&f.ptoken) && !log.contains("bad "), "tokens never reach the log");
}

#[test]
fn over_a_real_loopback_socket_the_server_answers_limits_and_publishes() {
    let f = fx("sock", |c| c.max_body = 2048);
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (srv, st) = (f.srv.clone(), stop.clone());
    let th = std::thread::spawn(move || super::server_http::serve(srv, l, st));
    let talk = |raw: Vec<u8>| -> String {
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        s.write_all(&raw).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    };
    let get = |p: &str| talk(format!("GET {p} HTTP/1.1\r\nHost: x\r\n\r\n").into_bytes());
    assert!(get("/healthz").starts_with("HTTP/1.1 200 OK") && get("/healthz").ends_with("ok\n"));
    assert!(get("/nothing").starts_with("HTTP/1.1 404"));
    assert!(get("/packages/../secret").starts_with("HTTP/1.1 400"));
    assert!(get("/catalog.json").contains("\"sig\""));
    assert!(!get("/").to_lowercase().contains("access-control"));
    // publish
    let k = new_signing_key().unwrap();
    let p = skill("sock-skill", "1.0.0", &k);
    let post = |tok: &str, body: &str| talk(format!("POST /v1/publish HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {tok}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes());
    let r = post(&f.ptoken, &p);
    assert!(r.starts_with("HTTP/1.1 200") && r.contains("\"sock-skill\""), "{r}");
    assert!(post("wrong", &p).starts_with("HTTP/1.1 401"));
    // a declared body that is too large is refused before it is read; chunked is refused; garbage is a 400
    let r = talk(b"POST /v1/publish HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999\r\n\r\n".to_vec());
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    assert!(talk(b"POST /v1/publish HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec()).starts_with("HTTP/1.1 400"));
    assert!(talk(b"BROKEN\r\n\r\n".to_vec()).starts_with("HTTP/1.1 400"));
    assert!(talk(vec![b'a'; 20_000]).starts_with("HTTP/1.1 431"));
    // the catalog the socket served verifies
    let c = get("/catalog.json");
    let body = c.split("\r\n\r\n").nth(1).unwrap();
    let (cat, _) = Catalog::parse(body, &Roots::default(), &[], NOW, true).unwrap();
    assert!(cat.find("sock-skill").is_some());
    stop.store(true, Ordering::SeqCst);
    th.join().unwrap();
}
