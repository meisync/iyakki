//! The shared build store on the server side: the read token, the signed build list (checks, pinned key, serial), retention 1 with the
//! old build kept until the purge, the store cap. In process (no sockets, no programs, nothing outside a temporary folder).

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

use super::buildshare as bs;
use super::server::*;
use crate::pkg::{new_signing_key, public_key_hex, signing_key_hex};

const ADMIN: &str = "admin-token-for-tests-0123456789";
const READ: &str = "read-token-for-tests-012345678901";

struct T {
    srv: StoreServer,
    dir: PathBuf,
    ukey: SigningKey,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(tag: &str, tweak: impl Fn(&mut Config, &SigningKey)) -> T {
    let dir = std::env::temp_dir().join(format!("iyakki-builds-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ukey = SigningKey::from_bytes(&[7u8; 32]);
    let mut cfg = Config::new(&dir.join("store"));
    cfg.admin_token_hash = Some(hash_token(ADMIN));
    cfg.read_token_hash = Some(hash_token(READ));
    cfg.writes_per_min = 10_000;
    cfg.reads_per_min = 10_000;
    cfg.disk_margin = 1 << 20;
    tweak(&mut cfg, &ukey);
    let srv = StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap();
    T { srv, dir, ukey }
}

fn req(method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Req {
    let mut headers = HashMap::new();
    if let Some(t) = token {
        headers.insert("authorization".to_string(), format!("Bearer {t}"));
    }
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let query = q.split('&').filter_map(|x| x.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Req { method: method.into(), path: p.into(), query, headers, body: body.to_vec() }
}

fn sha(b: &[u8]) -> String {
    crate::pkg::hex(&Sha256::digest(b))
}

impl T {
    fn call(&self, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Resp {
        self.srv.handle(&req(method, path, token, body), "10.0.0.1")
    }

    fn blob(&self, b: &[u8]) -> bs::BFile {
        let h = sha(b);
        let r = self.call("PUT", &format!("/v1/blobs/{h}?component=build"), Some(ADMIN), b);
        assert!(r.status == 200 || r.status == 201, "{}", r.text());
        bs::BFile { path: format!("boot/{}", &h[..6]), sha256: h, size: b.len() as u64 }
    }

    fn has_blob(&self, h: &str) -> bool {
        self.srv.blob_get(&req("GET", &format!("/v1/blobs/{h}"), Some(READ), b""), "10.0.0.1").status == 200
    }

    fn list_with(&self, old: Option<&bs::List>, id: &str, files: Vec<bs::BFile>, keep: usize) -> (bs::List, String) {
        let b = bs::Build { variant: "pc-x86_64".into(), channel: "dev".into(), build_id: id.into(), version: "0.1.0".into(), pack_variant: "pc".into(), commit: "c0ffee".into(), published: 1, seq: 0, total: 0, files, needs: vec![] };
        let (l, _) = bs::add_build(old, "test-builds", b, keep, 1000).unwrap();
        let doc = bs::build_doc(&l, &self.ukey);
        (l, doc)
    }

    fn put_list(&self, doc: &str) -> Resp {
        self.call("PUT", "/v1/builds.json", Some(ADMIN), doc.as_bytes())
    }
}

#[test]
fn reads_need_the_read_token_and_writes_need_a_publish_token() {
    let t = rig("tokens", |_, _| {});
    let f = t.blob(b"hello build");
    let (_, doc) = t.list_with(None, "a", vec![f.clone()], 1);
    assert_eq!(t.call("PUT", "/v1/builds.json", Some(READ), doc.as_bytes()).status, 401, "the read token cannot publish");
    assert_eq!(t.call("PUT", "/v1/builds.json", None, doc.as_bytes()).status, 401);
    assert_eq!(t.put_list(&doc).status, 200);
    for path in ["/v1/builds.json", "/v1/components.json"] {
        assert_eq!(t.call("GET", path, None, b"").status, 401, "{path} without a token");
        assert_eq!(t.call("GET", path, Some("wrong-token-wrong-token-1234"), b"").status, 401, "{path} with a wrong token");
        assert_eq!(t.call("GET", path, Some(READ), b"").status, 200, "{path} with the read token");
        assert_eq!(t.call("GET", path, Some(ADMIN), b"").status, 200, "{path} with the publish token");
    }
    let blob = |tok: Option<&str>| t.srv.blob_get(&req("GET", &format!("/v1/blobs/{}", f.sha256), tok, b""), "10.0.0.1").status;
    assert_eq!((blob(None), blob(Some("nope-nope-nope-nope-nope-1")), blob(Some(READ)), blob(Some(ADMIN))), (401, 401, 200, 200));
    assert_eq!(t.call("GET", "/healthz", None, b"").status, 200, "health stays open");
}

#[test]
fn a_store_without_a_read_token_stays_open_for_reads() {
    let t = rig("open", |c, _| c.read_token_hash = None);
    assert_eq!(t.call("GET", "/v1/components.json", None, b"").status, 200);
}

#[test]
fn the_list_is_checked_before_it_is_taken() {
    let t = rig("checks", |c, k| c.builds_key = Some(public_key_hex(k)));
    let f = t.blob(b"one");
    let (l1, doc1) = t.list_with(None, "a", vec![f.clone()], 1);
    // a list signed by another key is refused (the store pins the update key)
    let other = SigningKey::from_bytes(&[9u8; 32]);
    assert_eq!(t.put_list(&bs::build_doc(&l1, &other)).status, 400);
    // a list naming a blob the store does not have is refused, and says so
    let ghost = bs::BFile { path: "boot/ghost".into(), sha256: sha(b"ghost"), size: 5 };
    let (_, bad) = t.list_with(None, "g", vec![f.clone(), ghost], 1);
    let r = t.put_list(&bad);
    assert_eq!(r.status, 409);
    assert!(r.text().contains("not in the store"), "{}", r.text());
    // a blob of the wrong size is as good as missing
    let wrong = bs::BFile { size: f.size + 1, ..f.clone() };
    let (_, bad) = t.list_with(None, "w", vec![wrong], 1);
    assert_eq!(t.put_list(&bad).status, 409);
    // garbage, and a changed byte, are refused
    assert_eq!(t.put_list("{}").status, 400);
    assert_eq!(t.put_list(&doc1.replace("\"dev\"", "\"zzz\"")).status, 400);
    assert_eq!(t.put_list(&doc1).status, 200);
    // an older (or the same) serial is not taken back
    assert_eq!(t.put_list(&doc1).status, 409);
}

#[test]
fn keep_one_the_old_build_stays_fetchable_until_the_purge_and_shared_blobs_stay_after_it() {
    let t = rig("keep1", |_, _| {});
    let (root_a, shared, root_b) = (t.blob(b"root image A"), t.blob(b"kernel shared"), t.blob(b"root image B"));
    let (l1, doc1) = t.list_with(None, "a", vec![root_a.clone(), shared.clone()], 1);
    assert_eq!(t.put_list(&doc1).status, 200);
    // publish B: its blobs are up first, then the list
    let (l2, doc2) = t.list_with(Some(&l1), "b", vec![root_b.clone(), shared.clone()], 1);
    assert_eq!(l2.builds.len(), 1, "keep 1");
    assert_eq!(t.put_list(&doc2).status, 200);
    let latest = bs::verify(&t.call("GET", "/v1/builds.json", Some(READ), b"").body, &public_key_hex(&t.ukey)).unwrap();
    assert_eq!(bs::latest(&latest, "pc-x86_64", "dev").unwrap().build_id, "b", "latest is the new build at once");
    // DURING: a device that started on A can still finish
    assert!(t.has_blob(&root_a.sha256) && t.has_blob(&shared.sha256) && t.has_blob(&root_b.sha256));
    // AFTER the purge: A's unique blob is gone, the shared one and B's stay
    let r = t.call("POST", "/v1/builds/purge", Some(ADMIN), b"");
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains("\"removed_blobs\":1"), "{}", r.text());
    assert!(!t.has_blob(&root_a.sha256), "the old build's own blob is gone");
    assert!(t.has_blob(&shared.sha256) && t.has_blob(&root_b.sha256));
    assert_eq!(t.call("POST", "/v1/builds/purge", Some(READ), b"").status, 401);
}

#[test]
fn a_republished_identical_build_loses_nothing_in_the_purge() {
    let t = rig("same", |_, _| {});
    let f = t.blob(b"same root");
    let (l1, d1) = t.list_with(None, "a", vec![f.clone()], 1);
    t.put_list(&d1);
    let (_, d2) = t.list_with(Some(&l1), "b", vec![f.clone()], 1);
    t.put_list(&d2);
    t.call("POST", "/v1/builds/purge", Some(ADMIN), b"");
    assert!(t.has_blob(&f.sha256));
}

#[test]
fn gc_reports_the_disk_and_removes_only_old_unreferenced_blobs() {
    let t = rig("gc", |_, _| {});
    let (kept, stray) = (t.blob(b"referenced"), t.blob(b"nobody refers to me"));
    let (_, doc) = t.list_with(None, "a", vec![kept.clone()], 1);
    t.put_list(&doc);
    let dry = t.call("POST", "/v1/builds/gc?grace=0", Some(ADMIN), b"");
    let v: serde_json::Value = serde_json::from_slice(&dry.body).unwrap();
    assert_eq!((v["blobs"].as_u64(), v["unreferenced"].as_u64(), v["removed"].as_u64()), (Some(2), Some(1), Some(0)), "a dry run removes nothing");
    assert!(t.has_blob(&stray.sha256));
    // the default grace keeps a fresh upload that is waiting for its list
    t.call("POST", "/v1/builds/gc?apply=1", Some(ADMIN), b"");
    assert!(t.has_blob(&stray.sha256));
    t.call("POST", "/v1/builds/gc?apply=1&grace=0", Some(ADMIN), b"");
    assert!(!t.has_blob(&stray.sha256) && t.has_blob(&kept.sha256));
}

#[test]
fn the_store_cap_refuses_an_upload_that_would_go_over_it() {
    let t = rig("cap", |c, _| c.store_cap = 2 << 20);
    let a = vec![1u8; 1 << 20];
    let r = t.call("PUT", &format!("/v1/blobs/{}?component=build", sha(&a)), Some(ADMIN), &a);
    assert_eq!(r.status, 201, "{}", r.text());
    let b = vec![2u8; (1 << 20) + 4096];
    let r = t.call("PUT", &format!("/v1/blobs/{}?component=build", sha(&b)), Some(ADMIN), &b);
    assert_eq!(r.status, 507, "{}", r.text());
    assert!(r.text().contains("store is full"));
}

const ANN: &str = "ann-own-token-for-tests-0123456";
const BOB: &str = "bob-own-token-for-tests-0123456";

#[test]
fn each_person_reads_with_their_own_token_and_one_is_revoked_alone() {
    let both = parse_person_tokens(&format!("# the team\nann {ANN}\nbob {BOB}  # second\n")).unwrap();
    assert_eq!(both.len(), 2);
    let t = rig("persons", |c, _| {
        c.read_token_hash = None;
        c.person_read_tokens = both.clone();
    });
    let f = t.blob(b"person build");
    let (_, doc) = t.list_with(None, "a", vec![f.clone()], 1);
    assert_eq!(t.put_list(&doc).status, 200);
    assert_eq!(t.call("GET", "/v1/builds.json", None, b"").status, 401, "a store with person tokens is not open");
    assert_eq!(t.call("GET", "/v1/builds.json", Some(READ), b"").status, 401, "a token nobody has");
    for tok in [ANN, BOB] {
        assert_eq!(t.call("GET", "/v1/builds.json", Some(tok), b"").status, 200);
    }
    assert_eq!(t.call("PUT", "/v1/builds.json", Some(ANN), doc.as_bytes()).status, 401, "a person's read token cannot publish");
    let log = t.srv.audit_lines().join("\n");
    assert!(log.contains(" ann builds.read") && log.contains(" bob builds.read"), "{log}");
    assert!(!log.contains(ANN) && !log.contains(BOB), "no token in the log");
    // Bob's line is taken out: only Bob loses access
    let t2 = rig("persons2", |c, _| {
        c.read_token_hash = None;
        c.person_read_tokens = parse_person_tokens(&format!("ann {ANN}\n")).unwrap();
    });
    assert_eq!((t2.call("GET", "/v1/builds.json", Some(ANN), b"").status, t2.call("GET", "/v1/builds.json", Some(BOB), b"").status), (404, 401), "Ann still reads (nothing published here: 404), Bob is refused");
}

#[test]
fn the_shared_token_and_person_tokens_work_side_by_side_and_the_file_is_checked() {
    let t = rig("both", |c, _| c.person_read_tokens = parse_person_tokens(&format!("ann {ANN}\n")).unwrap());
    assert_eq!((t.call("GET", "/v1/components.json", Some(READ), b"").status, t.call("GET", "/v1/components.json", Some(ANN), b"").status), (200, 200));
    for bad in ["ann short", "ann", "a/b 0123456789012345678901234", &format!("ann {ANN}\nann {BOB}"), &format!("ann {ANN} extra")] {
        assert!(parse_person_tokens(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_blob_read_with_a_persons_own_token_names_the_person_in_the_audit_log_and_other_readers_are_not_named() {
    let t = rig("blobaudit", |c, _| c.person_read_tokens = parse_person_tokens(&format!("ann {ANN}\nbob {BOB}\n")).unwrap());
    let f = t.blob(b"audited bytes");
    let get = |tok: &str, range: Option<&str>| {
        let mut r = req("GET", &format!("/v1/blobs/{}", f.sha256), Some(tok), b"");
        if let Some(x) = range {
            r.headers.insert("range".into(), x.into());
        }
        t.srv.blob_get(&r, "10.0.0.1").status
    };
    assert_eq!(get(ANN, None), 200);
    assert_eq!(get(BOB, Some("bytes=0-3")), 206);
    assert_eq!(get(BOB, Some("bytes=4-6")), 206, "a resumed piece is served");
    assert_eq!(get(READ, None), 200, "the shared token");
    let log = t.srv.audit_lines();
    let reads: Vec<&String> = log.iter().filter(|l| l.contains("blob.read")).collect();
    assert_eq!(reads.len(), 2, "{log:?}");
    assert!(reads.iter().any(|l| l.contains(" ann blob.read")) && reads.iter().any(|l| l.contains(" bob blob.read")), "{reads:?}");
    assert!(reads.iter().all(|l| l.contains(&f.sha256)));
    let all = log.join("\n");
    assert!(!all.contains(ANN) && !all.contains(BOB) && !all.contains(READ), "no token in the log");
}
