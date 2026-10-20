use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// An in-memory registry v2: blobs by digest, manifests by `repo:tag`, a log of every request.
#[derive(Default)]
pub struct FakeRegistry {
    pub blobs: Mutex<HashMap<String, Vec<u8>>>,
    pub manifests: Mutex<HashMap<String, Vec<u8>>>,
    pub log: Mutex<Vec<String>>,
    /// Answer every upload start with this status (a failure to test).
    pub upload_status: Mutex<Option<u16>>,
    pub down: Mutex<bool>,
}

impl RegistryHttp for FakeRegistry {
    fn request(&self, method: &str, host: &str, path: &str, _h: &[(String, String)], body: &[u8]) -> Result<Reply, String> {
        self.log.lock().unwrap().push(format!("{method} {path}"));
        if *self.down.lock().unwrap() {
            return Err(format!("cannot connect to {host}: connection refused"));
        }
        let r = |status: u16, body: &str| Ok(Reply { status, headers: vec![], body: body.as_bytes().to_vec() });
        if path == "/v2/" {
            return r(200, "{}");
        }
        if let Some(rest) = path.strip_prefix("/v2/_catalog") {
            let _ = rest;
            let repos: std::collections::BTreeSet<String> = self.manifests.lock().unwrap().keys().filter_map(|k| k.rsplit_once(':').map(|x| x.0.to_string())).collect();
            return r(200, &serde_json::json!({ "repositories": repos }).to_string());
        }
        if let Some(repo) = path.strip_prefix("/v2/").and_then(|p| p.strip_suffix("/tags/list")) {
            let tags: Vec<String> = self.manifests.lock().unwrap().keys().filter_map(|k| k.strip_prefix(&format!("{repo}:")).map(str::to_string)).collect();
            return r(200, &serde_json::json!({ "name": repo, "tags": tags }).to_string());
        }
        if method == "HEAD" && path.contains("/blobs/sha256:") {
            let d = &path[path.find("sha256:").unwrap()..];
            return r(if self.blobs.lock().unwrap().contains_key(d) { 200 } else { 404 }, "");
        }
        if method == "POST" && path.ends_with("/blobs/uploads/") {
            if let Some(s) = *self.upload_status.lock().unwrap() {
                return r(s, "denied");
            }
            let repo = path.trim_start_matches("/v2/").trim_end_matches("/blobs/uploads/");
            return Ok(Reply { status: 202, headers: vec![("location".into(), format!("http://{host}/v2/{repo}/blobs/uploads/uuid-1?_state=abc"))], body: vec![] });
        }
        if method == "PUT" && path.contains("/blobs/uploads/") {
            let d = path.split("digest=").nth(1).unwrap().to_string();
            if format!("sha256:{}", sha256_hex(body)) != d {
                return r(400, "digest invalid");
            }
            self.blobs.lock().unwrap().insert(d, body.to_vec());
            return r(201, "");
        }
        if method == "PUT" && path.contains("/manifests/") {
            let rest = path.trim_start_matches("/v2/");
            let (repo, tag) = rest.split_once("/manifests/").unwrap();
            self.manifests.lock().unwrap().insert(format!("{repo}:{tag}"), body.to_vec());
            return r(201, "");
        }
        r(404, "not found")
    }
}

fn spec(bytes: &[u8]) -> ImageSpec<'_> {
    ImageSpec { file: "iyakki", contents: bytes, arch: "arm64", user: "10001:10001", args: vec!["store-server".into(), "--data".into(), "/data".into()], port: 8080 }
}

#[test]
fn the_tar_has_one_file_with_a_valid_header() {
    let t = tar_one_file("iyakki", b"hello").unwrap();
    assert_eq!(t.len(), 512 + 512 + 1024);
    assert_eq!(&t[..6], b"iyakki");
    assert_eq!(&t[257..262], b"ustar");
    assert_eq!(t[156], b'0');
    // the checksum is the sum of the header with the checksum field read as spaces
    let stored = u32::from_str_radix(std::str::from_utf8(&t[148..154]).unwrap(), 8).unwrap();
    let mut h = t[..512].to_vec();
    h[148..156].copy_from_slice(b"        ");
    assert_eq!(stored, h.iter().map(|b| *b as u32).sum::<u32>());
    assert_eq!(std::str::from_utf8(&t[124..135]).unwrap(), "00000000005");
    assert_eq!(&t[512..517], b"hello");
    assert!(t[517..].iter().all(|b| *b == 0));
    assert!(tar_one_file("a/b", b"x").is_err() && tar_one_file("", b"x").is_err());
}

#[test]
fn the_image_is_reproducible_and_describes_the_program() {
    let a = build_image(&spec(b"program")).unwrap();
    let b = build_image(&spec(b"program")).unwrap();
    assert_eq!(a.manifest, b.manifest);
    assert_eq!(a.layer_digest, format!("sha256:{}", sha256_hex(&a.layer)));
    let cfg: serde_json::Value = serde_json::from_slice(&a.config).unwrap();
    assert_eq!(cfg["architecture"], "arm64");
    assert_eq!(cfg["os"], "linux");
    assert_eq!(cfg["config"]["User"], "10001:10001");
    assert_eq!(cfg["config"]["Entrypoint"], serde_json::json!(["/iyakki", "store-server", "--data", "/data"]));
    assert!(cfg["config"]["ExposedPorts"]["8080/tcp"].is_object());
    assert_eq!(cfg["rootfs"]["diff_ids"][0], a.layer_digest.as_str());
    let m: serde_json::Value = serde_json::from_slice(&a.manifest).unwrap();
    assert_eq!(m["schemaVersion"], 2);
    assert_eq!(m["mediaType"], MEDIA_MANIFEST);
    assert_eq!(m["config"]["digest"], a.config_digest.as_str());
    assert_eq!(m["layers"][0]["mediaType"], MEDIA_LAYER);
    assert_eq!(m["layers"][0]["size"], a.layer.len());
    // a different program is a different layer
    assert_ne!(build_image(&spec(b"other")).unwrap().layer_digest, a.layer_digest);
    assert!(build_image(&ImageSpec { arch: "riscv64", ..spec(b"x") }).is_err());
    assert!(build_image(&spec(b"")).is_err());
}

#[test]
fn pushing_sends_the_blobs_then_the_manifest_and_skips_what_the_registry_has() {
    let reg = FakeRegistry::default();
    let img = build_image(&spec(b"program bytes")).unwrap();
    let mut said = Vec::new();
    let digest = push(&reg, "localhost:30500", "iyakki-platform/iyakki-store-server", "1.0.0", &img, &mut |l| said.push(l.to_string())).unwrap();
    assert_eq!(digest, format!("sha256:{}", sha256_hex(&img.manifest)));
    assert_eq!(reg.blobs.lock().unwrap().get(&img.layer_digest), Some(&img.layer));
    assert_eq!(reg.blobs.lock().unwrap().get(&img.config_digest), Some(&img.config));
    assert_eq!(reg.manifests.lock().unwrap().get("iyakki-platform/iyakki-store-server:1.0.0"), Some(&img.manifest));
    let log = reg.log.lock().unwrap().clone();
    assert!(log.first().unwrap() == "GET /v2/" && log.last().unwrap().starts_with("PUT /v2/iyakki-platform/iyakki-store-server/manifests/1.0.0"), "{log:?}");
    assert!(said.iter().any(|l| l.contains("sending the image layer")) && said.iter().any(|l| l.contains("sending the manifest")), "{said:?}");
    // the absolute Location of the upload is reduced to its path
    assert!(log.iter().any(|l| l.starts_with("PUT /v2/iyakki-platform/iyakki-store-server/blobs/uploads/uuid-1?_state=abc&digest=sha256:")), "{log:?}");
    // pushing again sends no blob
    reg.log.lock().unwrap().clear();
    let mut said2 = Vec::new();
    push(&reg, "localhost:30500", "iyakki-platform/iyakki-store-server", "1.0.0", &img, &mut |l| said2.push(l.to_string())).unwrap();
    assert!(!reg.log.lock().unwrap().iter().any(|l| l.starts_with("POST ")), "{:?}", reg.log.lock().unwrap());
    assert!(said2.iter().any(|l| l.contains("in the registry already")), "{said2:?}");
}

#[test]
fn push_failures_are_said_plainly() {
    let img = build_image(&spec(b"x")).unwrap();
    let reg = FakeRegistry::default();
    *reg.down.lock().unwrap() = true;
    let e = push(&reg, "localhost:30500", "a/b", "1", &img, &mut |_| {}).unwrap_err();
    assert!(e.contains("cannot connect to localhost:30500"), "{e}");
    let reg = FakeRegistry::default();
    *reg.upload_status.lock().unwrap() = Some(403);
    let e = push(&reg, "localhost:30500", "a/b", "1", &img, &mut |_| {}).unwrap_err();
    assert!(e.contains("starting the upload failed at localhost:30500: HTTP 403") && e.contains("sign-in"), "{e}");
    assert!(push(&FakeRegistry::default(), "h:1", "Bad Repo", "1", &img, &mut |_| {}).unwrap_err().contains("not a repository name"));
    assert!(push(&FakeRegistry::default(), "h:1", "a/b", "-x", &img, &mut |_| {}).unwrap_err().contains("not an image tag"));
}

#[test]
fn the_catalog_counts_repositories_and_tags() {
    let reg = FakeRegistry::default();
    let img = build_image(&spec(b"x")).unwrap();
    push(&reg, "h:1", "iyakki-platform/store", "1.0.0", &img, &mut |_| {}).unwrap();
    push(&reg, "h:1", "iyakki-platform/store", "1.0.1", &img, &mut |_| {}).unwrap();
    push(&reg, "h:1", "other/app", "x", &img, &mut |_| {}).unwrap();
    let c = catalog(&reg, "h:1").unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c.iter().map(|(_, t)| t.len()).sum::<usize>(), 3);
    assert!(answers(&reg, "h:1"));
    *reg.down.lock().unwrap() = true;
    assert!(!answers(&reg, "h:1"));
}

#[test]
fn replies_are_parsed_with_length_chunks_or_close() {
    let r = parse_reply(b"HTTP/1.1 202 Accepted\r\nLocation: /v2/x/blobs/uploads/1\r\nContent-Length: 2\r\n\r\nokEXTRA", false).unwrap();
    assert_eq!((r.status, r.header("location"), r.body.as_slice()), (202, Some("/v2/x/blobs/uploads/1"), &b"ok"[..]));
    let r = parse_reply(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n", false).unwrap();
    assert_eq!(r.body, b"abcde");
    let r = parse_reply(b"HTTP/1.1 200 OK\r\n\r\nuntil close", false).unwrap();
    assert_eq!(r.body, b"until close");
    let r = parse_reply(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n", true).unwrap();
    assert!(r.body.is_empty());
    assert!(parse_reply(b"garbage", false).is_err());
    assert_eq!(upload_url("/v2/a/blobs/uploads/1", "sha256:ab"), "/v2/a/blobs/uploads/1?digest=sha256:ab");
    assert_eq!(upload_url("http://h:1/v2/a/blobs/uploads/1?x=1", "sha256:ab"), "/v2/a/blobs/uploads/1?x=1&digest=sha256:ab");
}

/// The real client against a throwaway server on the loopback address of this machine (no other network).
#[test]
fn the_real_client_talks_http_to_a_local_listener() {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") || !got.ends_with(b"PAYLOAD") {
            let n = s.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        s.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 4\r\n\r\ndone").unwrap();
        String::from_utf8_lossy(&got).into_owned()
    });
    let r = TcpHttp::default().request("PUT", &addr, "/v2/x/manifests/1", &[("Content-Type".into(), "text/plain".into())], b"PAYLOAD").unwrap();
    assert_eq!((r.status, r.body.as_slice()), (201, &b"done"[..]));
    let seen = t.join().unwrap();
    assert!(seen.starts_with("PUT /v2/x/manifests/1 HTTP/1.1\r\n") && seen.contains("Content-Length: 7\r\n") && seen.contains("Content-Type: text/plain\r\n"), "{seen}");
    // nothing listens on a closed port
    let gone = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = gone.local_addr().unwrap().to_string();
    drop(gone);
    assert!(TcpHttp::default().request("GET", &a, "/v2/", &[], &[]).unwrap_err().contains("cannot connect"));
    assert!(TcpHttp::default().request("GET", "a b", "/v2/", &[], &[]).is_err());
}
