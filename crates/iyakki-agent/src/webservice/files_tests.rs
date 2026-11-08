//! The cloud profile's file and clone routes: roles, scopes, the path jail, other namespaces and other people.

use super::*;

const NS: &str = "namespace=web";

fn files(h: &H, tok: &str, q: &str) -> (u16, Value) {
    let (s, v, _) = h.call("GET", &format!("/api/v1/files?{q}"), Some(tok), None);
    (s, v)
}
fn names(v: &Value) -> Vec<String> {
    v["entries"].as_array().map(|a| a.iter().map(|e| e["name"].as_str().unwrap().to_string()).collect()).unwrap_or_default()
}
fn write(h: &H, tok: &str, body: Value) -> u16 {
    h.call("POST", "/api/v1/files/write", Some(tok), Some(body)).0
}
fn on_disk(h: &H, rel: &str) -> std::path::PathBuf {
    h.dir.join("cloud").join("files").join(rel)
}

#[test]
fn files_a_person_reads_and_writes_their_own_three_places_and_only_there() {
    let (h, _, ann, _, _) = acme_cloud();
    // namespace folder, shared folder, personal folder
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "notes/a.txt", "content": "mine"})), 200);
    assert_eq!(write(&h, &ann, json!({"scope": "shared", "namespace": "web", "path": "plan.md", "content": "ours"})), 200);
    assert_eq!(write(&h, &ann, json!({"path": "diary.txt", "content": "private"})), 200);
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/web/people/ann/notes/a.txt")).unwrap(), "mine");
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/web/shared/plan.md")).unwrap(), "ours");
    assert_eq!(std::fs::read_to_string(on_disk(&h, "personal/ann/diary.txt")).unwrap(), "private");
    let (s, v) = files(&h, &ann, &format!("{NS}&path=notes"));
    assert_eq!((s, names(&v)), (200, vec!["a.txt".to_string()]));
    let (s, v, _) = h.call("GET", "/api/v1/files/content?namespace=web&path=notes/a.txt", Some(&ann), None);
    assert_eq!((s, v["content"].as_str()), (200, Some("mine")));
    assert_eq!(files(&h, &ann, "scope=shared&namespace=web").1["entries"][0]["name"], "plan.md");
    assert_eq!(files(&h, &ann, "").1["entries"][0]["name"], "diary.txt", "no namespace: the personal folder");
    // delete
    assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": "notes"}))).0, 400, "a folder with files needs recursive");
    assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": "notes", "recursive": true}))).0, 200);
    assert!(!on_disk(&h, "ns/web/people/ann/notes").exists());
    assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": ""}))).0, 400, "the folder as a whole stays");
    assert_eq!(h.call("GET", "/api/v1/files/content?namespace=web&path=notes/a.txt", Some(&ann), None).0, 404);
    // limits
    assert_eq!(write(&h, &ann, json!({"path": "big.txt", "content": "x".repeat(600 * 1024)})), 413);
    std::fs::write(on_disk(&h, "personal/ann/bin"), [0xff, 0xfe]).unwrap();
    assert_eq!(h.call("GET", "/api/v1/files/content?path=bin", Some(&ann), None).0, 400);
}

#[test]
fn files_traversal_absolute_paths_and_links_are_refused_by_every_route() {
    let (h, _, ann, _, _) = acme_cloud();
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "ok.txt", "content": "x"})), 200);
    let secret_dir = h.dir.join("outside");
    std::fs::create_dir_all(&secret_dir).unwrap();
    std::fs::write(secret_dir.join("secret.txt"), "top secret").unwrap();
    for bad in ["../x", "../../ann/ok.txt", "/etc/passwd", "a/../../b", "ok.txt/../../../etc", "..", "/"] {
        assert_eq!(files(&h, &ann, &format!("{NS}&path={bad}")).0, 400, "list {bad}");
        assert_eq!(h.call("GET", &format!("/api/v1/files/content?{NS}&path={bad}"), Some(&ann), None).0, 400, "read {bad}");
        assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": bad, "content": "evil"})), 400, "write {bad}");
        assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": bad, "recursive": true}))).0, 400, "delete {bad}");
    }
    assert!(!h.dir.join("cloud/files/ns/web/people/x").exists() && !h.dir.join("cloud/files/b").exists() && !h.dir.join("b").exists());
    #[cfg(unix)]
    {
        let mine = on_disk(&h, "ns/web/people/ann");
        std::os::unix::fs::symlink(&secret_dir, mine.join("out")).unwrap();
        std::os::unix::fs::symlink(secret_dir.join("secret.txt"), mine.join("peek")).unwrap();
        std::os::unix::fs::symlink("/", mine.join("rootlink")).unwrap();
        for p in ["out", "out/secret.txt", "peek", "rootlink", "rootlink/etc/passwd"] {
            assert_eq!(files(&h, &ann, &format!("{NS}&path={p}")).0, 400, "list {p}");
            assert_eq!(h.call("GET", &format!("/api/v1/files/content?{NS}&path={p}"), Some(&ann), None).0, 400, "read {p}");
            assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": p, "content": "evil"})), 400, "write {p}");
            assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": p, "recursive": true}))).0, 400, "delete {p}");
        }
        assert_eq!(std::fs::read_to_string(secret_dir.join("secret.txt")).unwrap(), "top secret");
        // the listing shows the links as links, and a clone folder cannot go through one either
        let (_, v) = files(&h, &ann, NS);
        assert!(v["entries"].as_array().unwrap().iter().any(|e| e["name"] == "out" && e["kind"] == "link"));
        assert_eq!(h.call("POST", "/api/v1/git/clone", Some(&ann), Some(json!({"namespace": "web", "url": "https://example.org/a/b.git", "folder": "out"}))).0, 400);
    }
}

#[test]
fn files_another_namespace_is_a_namespace_that_is_not_there() {
    let (h, root, ann, bob, gus) = acme_cloud();
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "other", "action": "create"}))).0, 200);
    assert_eq!(write(&h, &root, json!({"scope": "shared", "namespace": "other", "path": "s.txt", "content": "other's"})), 200);
    let probes = |tok: &str, ns: &str| -> Vec<u16> {
        vec![
            files(&h, tok, &format!("scope=shared&namespace={ns}")).0,
            files(&h, tok, &format!("namespace={ns}")).0,
            h.call("GET", &format!("/api/v1/files/content?scope=shared&namespace={ns}&path=s.txt"), Some(tok), None).0,
            write(&h, tok, json!({"scope": "shared", "namespace": ns, "path": "s.txt", "content": "taken over"})),
            h.call("POST", "/api/v1/files/delete", Some(tok), Some(json!({"scope": "shared", "namespace": ns, "path": "s.txt"}))).0,
            h.call("POST", "/api/v1/git/clone", Some(tok), Some(json!({"namespace": ns, "url": "https://example.org/a/b.git"}))).0,
            h.call("GET", &format!("/api/v1/git/clones?namespace={ns}"), Some(tok), None).0,
        ]
    };
    // ann is in web, not in other; bob is in neither: both get what a missing namespace gets
    assert_eq!(probes(&ann, "other"), vec![404; 7]);
    assert_eq!(probes(&ann, "nothing"), vec![404; 7]);
    assert_eq!(probes(&bob, "web"), vec![404; 7]);
    let a = h.call("GET", "/api/v1/files?scope=shared&namespace=other", Some(&bob), None);
    let b = h.call("GET", "/api/v1/files?scope=shared&namespace=nothing", Some(&bob), None);
    assert_eq!((a.0, &a.1), (b.0, &b.1), "the same words");
    // a stranger to the tenant: refused everywhere, including the personal folder
    assert_eq!(probes(&gus, "web"), vec![403; 7]);
    assert_eq!(files(&h, &gus, "").0, 403);
    assert_eq!(write(&h, &gus, json!({"path": "x", "content": "y"})), 403);
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/other/shared/s.txt")).unwrap(), "other's", "untouched");
    // names that are not folder names never reach the disk
    for ns in ["..", "a.b", "web%2f..", "x y"] {
        assert!(matches!(files(&h, &ann, &format!("scope=shared&namespace={ns}")).0, 400 | 404), "{ns}");
    }
    assert!(!h.dir.join("cloud/files/ns/..").join("s.txt").exists());
    // a tenant admin sees the name, still cannot read (the roles say so)
    assert_eq!(files(&h, &root, "scope=shared&namespace=other").0, 200, "root administers the namespace it made");
}

#[test]
fn files_a_tenant_without_role_lists_allows_nothing() {
    let h = harness("cloud", |_| {});
    let (root, ann) = (h.root(), h.ann());
    for t in [&root, &ann] {
        assert_eq!(files(&h, t, "").0, 403);
        assert_eq!(files(&h, t, "scope=shared&namespace=web").0, 403);
        assert_eq!(write(&h, t, json!({"path": "x", "content": "y"})), 403);
        assert_eq!(h.call("POST", "/api/v1/git/clone", Some(t), Some(json!({"url": "https://example.org/a/b.git"}))).0, 403);
    }
    assert!(!h.dir.join("cloud/files").exists(), "nothing was made on the disk for a refused call");
}

#[test]
fn files_one_person_never_sees_another_persons_folders() {
    let (h, root, ann, bob, _) = acme_cloud();
    // bob joins web as an editor: same namespace, his own folder
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "editor"}))).0, 200);
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "a.txt", "content": "ann private"})), 200);
    assert_eq!(write(&h, &ann, json!({"path": "p.txt", "content": "ann personal"})), 200);
    assert_eq!(write(&h, &ann, json!({"scope": "shared", "namespace": "web", "path": "s.txt", "content": "both"})), 200);
    assert!(names(&files(&h, &bob, NS).1).is_empty(), "bob's folder is his own");
    assert!(names(&files(&h, &bob, "").1).is_empty());
    assert_eq!(names(&files(&h, &bob, "scope=shared&namespace=web").1), vec!["s.txt"], "shared is shared");
    for p in ["../ann/a.txt", "../../people/ann/a.txt", "ann/a.txt", "a.txt"] {
        let s = h.call("GET", &format!("/api/v1/files/content?{NS}&path={p}"), Some(&bob), None).0;
        assert!(s == 400 || s == 404, "{p}: {s}");
    }
    assert_eq!(h.call("GET", "/api/v1/files/content?path=../ann/p.txt", Some(&bob), None).0, 400);
    // the person cannot come from the request
    for body in [json!({"person": "ann", "namespace": "web", "path": "a.txt", "content": "forged"}), json!({"user": "ann", "path": "p.txt", "content": "forged"})] {
        assert_eq!(write(&h, &bob, body), 400);
    }
    assert_eq!(files(&h, &bob, &format!("{NS}&person=ann")).1["entries"], json!([]), "an extra query word changes nothing");
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/web/people/ann/a.txt")).unwrap(), "ann private");
    // bob writes into his own, ann does not see it
    assert_eq!(write(&h, &bob, json!({"namespace": "web", "path": "b.txt", "content": "bob"})), 200);
    assert_eq!(names(&files(&h, &ann, NS).1), vec!["a.txt"]);
    // the personal scope takes no namespace, and unknown scopes are refused
    assert_eq!(files(&h, &ann, "scope=personal&namespace=web").0, 400);
    assert_eq!(files(&h, &ann, "scope=all").0, 400);
    assert_eq!(files(&h, &ann, "scope=shared").0, 400);
}

#[test]
fn files_a_viewer_reads_but_cannot_write_delete_or_clone_and_removal_ends_access() {
    let (h, root, ann, _, _) = acme_cloud();
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "a.txt", "content": "v1"})), 200);
    assert_eq!(write(&h, &ann, json!({"scope": "shared", "namespace": "web", "path": "s.txt", "content": "v1"})), 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "ann", "role": "viewer"}))).0, 200);
    assert_eq!(files(&h, &ann, NS).0, 200);
    assert_eq!(h.call("GET", &format!("/api/v1/files/content?{NS}&path=a.txt"), Some(&ann), None).1["content"], "v1");
    assert_eq!(h.call("GET", "/api/v1/files/content?scope=shared&namespace=web&path=s.txt", Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", &format!("/api/v1/git/clones?{NS}"), Some(&ann), None).0, 200);
    for sc in ["person", "shared"] {
        assert_eq!(write(&h, &ann, json!({"scope": sc, "namespace": "web", "path": "a.txt", "content": "v2"})), 403, "write {sc}");
        assert_eq!(write(&h, &ann, json!({"scope": sc, "namespace": "web", "path": "new.txt", "content": "v2"})), 403);
        assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"scope": sc, "namespace": "web", "path": "a.txt"}))).0, 403, "delete {sc}");
        assert_eq!(h.call("POST", "/api/v1/git/clone", Some(&ann), Some(json!({"scope": sc, "namespace": "web", "url": "https://example.org/a/b.git"}))).0, 403, "clone {sc}");
    }
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/web/people/ann/a.txt")).unwrap(), "v1");
    assert!(!on_disk(&h, "ns/web/people/ann/new.txt").exists());
    // a viewer still has a personal folder outside the namespace
    assert_eq!(write(&h, &ann, json!({"path": "mine.txt", "content": "ok"})), 200);
    // the owner removes ann from the namespace: gone at once, with the answer for a missing namespace
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "ann", "role": "remove"}))).0, 200);
    assert_eq!(files(&h, &ann, NS).0, 404);
    assert_eq!(h.call("GET", &format!("/api/v1/files/content?{NS}&path=a.txt"), Some(&ann), None).0, 404);
    // and from the tenant: nothing at all, personal folder included
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "ann", "role": "remove"}))).0, 200);
    assert_eq!(files(&h, &ann, "").0, 403);
}

#[test]
fn git_clone_refuses_addresses_and_folder_names_that_are_not_safe_and_lists_checkouts() {
    let (h, _, ann, _, _) = acme_cloud();
    let clone = |body: Value| h.call("POST", "/api/v1/git/clone", Some(&ann), Some(body)).0;
    for url in [
        "file:///etc", "/etc/passwd", "ssh://git@example.org/a", "git@example.org:a/b.git", "http://example.org/a.git", "https://user:pw@example.org/a.git",
        "https://localhost/a.git", "https://127.0.0.1/a.git", "https://169.254.169.254/x", "https://db.svc/a.git", "--upload-pack=touch /tmp/x", "",
    ] {
        assert_eq!(clone(json!({"namespace": "web", "url": url})), 400, "{url}");
    }
    for folder in ["../x", "a/b", "/abs", ".hidden", "", "x y"] {
        assert_eq!(clone(json!({"namespace": "web", "url": "https://example.org/a/b.git", "folder": folder})), 400, "{folder:?}");
    }
    assert_eq!(clone(json!({"namespace": "web", "url": "https://example.org/a/b.git", "branch": "--evil"})), 400);
    assert_eq!(clone(json!({"namespace": "web", "url": "https://example.org/a/b.git", "person": "bob"})), 400, "the person cannot be named");
    // an existing folder of that name is not replaced
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "b/keep.txt", "content": "x"})), 200);
    assert_eq!(clone(json!({"namespace": "web", "url": "https://example.org/a/b.git"})), 400);
    assert_eq!(std::fs::read_to_string(on_disk(&h, "ns/web/people/ann/b/keep.txt")).unwrap(), "x");
    // checkouts are found by their .git folder
    std::fs::create_dir_all(on_disk(&h, "ns/web/people/ann/repo/.git")).unwrap();
    std::fs::write(on_disk(&h, "ns/web/people/ann/repo/.git/HEAD"), "ref: refs/heads/dev\n").unwrap();
    let (s, v, _) = h.call("GET", &format!("/api/v1/git/clones?{NS}"), Some(&ann), None);
    assert_eq!((s, v["clones"].clone()), (200, json!([{"folder": "repo", "branch": "dev"}])));
}

#[test]
fn files_routes_are_in_the_cloud_profile_only_and_in_the_openapi_document() {
    let agent = harness("agent-api", |_| {});
    let t = agent.root();
    for (m, p) in [("GET", "/api/v1/files"), ("GET", "/api/v1/files/content"), ("POST", "/api/v1/files/write"), ("POST", "/api/v1/files/delete"), ("POST", "/api/v1/git/clone"), ("GET", "/api/v1/git/clones")] {
        assert_eq!(agent.call(m, p, Some(&t), Some(json!({})).filter(|_| m == "POST")).0, 404, "{p}");
    }
    let (h, _, ann, _, _) = acme_cloud();
    let doc = h.call("GET", "/api/v1/openapi.json", Some(&ann), None).1;
    for p in ["/api/v1/files", "/api/v1/files/content", "/api/v1/files/write", "/api/v1/files/delete", "/api/v1/git/clone", "/api/v1/git/clones"] {
        assert!(doc["paths"][p].is_object(), "{p}");
    }
    // no token, no answer
    assert_eq!(h.call("GET", "/api/v1/files", None, None).0, 401);
}

// ------------------------------------------------------------------------------------------------------------ binary files

use std::sync::atomic::{AtomicU64, Ordering as AtOrd};

/// A socket stand-in: the body is read from `input`, the answer is collected in `out`.
struct Duplex {
    input: Box<dyn std::io::Read>,
    out: Vec<u8>,
}
impl std::io::Read for Duplex {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(b)
    }
}
impl std::io::Write for Duplex {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.out.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `left` bytes of 7s that are never all in memory, and the largest buffer asked for.
struct Lazy {
    left: Arc<AtomicU64>,
    biggest: Arc<AtomicU64>,
}
impl std::io::Read for Lazy {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.biggest.fetch_max(b.len() as u64, AtOrd::SeqCst);
        let n = (b.len() as u64).min(self.left.load(AtOrd::SeqCst)) as usize;
        b[..n].fill(7);
        self.left.fetch_sub(n as u64, AtOrd::SeqCst);
        Ok(n)
    }
}

fn lazy(n: u64) -> (Lazy, Arc<AtomicU64>, Arc<AtomicU64>) {
    let (left, biggest) = (Arc::new(AtomicU64::new(n)), Arc::new(AtomicU64::new(0)));
    (Lazy { left: left.clone(), biggest: biggest.clone() }, left, biggest)
}

/// One streamed request. Returns (status, header text, body).
fn stream(h: &H, method: &str, target: &str, tok: Option<&str>, len: Option<u64>, extra: &str, body: Box<dyn std::io::Read>) -> (u16, String, Vec<u8>) {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: svc.test\r\n");
    if let Some(t) = tok {
        head += &format!("Authorization: Bearer {t}\r\n");
    }
    if let Some(n) = len {
        head += &format!("Content-Length: {n}\r\n");
    }
    head += extra;
    head += "\r\n";
    assert!(crate::webservice::files::is_stream_head(head.as_bytes()));
    let mut d = Duplex { input: body, out: Vec::new() };
    assert!(crate::webservice::files::serve_stream(h.svc.shared(), &mut d, head.as_bytes(), "10.1.1.1".parse().unwrap()), "handled");
    let mut out = d.out.as_slice();
    while out.starts_with(b"HTTP/1.1 100 Continue\r\n\r\n") {
        out = &out[b"HTTP/1.1 100 Continue\r\n\r\n".len()..];
    }
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").expect("a complete head");
    let head = String::from_utf8_lossy(&out[..at]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head, out[at + 4..].to_vec())
}

fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

fn sha(b: &[u8]) -> String {
    crate::ocipush::sha256_hex(b)
}

fn tmp_files(h: &H) -> Vec<String> {
    fn walk(d: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(".iyakki-up-") {
                out.push(n);
            }
            if e.path().is_dir() {
                walk(&e.path(), out);
            }
        }
    }
    let mut v = vec![];
    walk(&h.dir.join("cloud").join("files"), &mut v);
    v
}

#[test]
fn binary_files_are_uploaded_streamed_and_downloaded_byte_for_byte() {
    let (h, _, ann, _, _) = acme_cloud();
    let data = noise(5 * 1024 * 1024 + 123); // above the service's 1 MiB body limit; every byte value occurs
    assert!(data.len() > 1 << 20 && (0..=255u8).all(|b| data.contains(&b)));
    let up = |path: &str, scope: &str, body: &[u8]| stream(&h, "PUT", &format!("/api/v1/files/upload?{NS}&scope={scope}&path={path}"), Some(&ann), Some(body.len() as u64), "Content-Type: application/octet-stream\r\n", Box::new(std::io::Cursor::new(body.to_vec())));
    let (s, head, body) = up("pics/a.bin", "person", &data);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((s, v["bytes"].as_u64(), v["sha256"].as_str()), (200, Some(data.len() as u64), Some(sha(&data).as_str())), "{head}");
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/pics/a.bin")).unwrap(), data);
    assert!(tmp_files(&h).is_empty(), "no temporary file is left");
    // shared and personal places work the same way
    assert_eq!(up("s.bin", "shared", b"\x00\xff\r\n\x00").0, 200);
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/shared/s.bin")).unwrap(), b"\x00\xff\r\n\x00");
    let (s, ..) = stream(&h, "PUT", "/api/v1/files/upload?path=mine.bin", Some(&ann), Some(3), "", Box::new(std::io::Cursor::new(b"abc".to_vec())));
    assert_eq!((s, std::fs::read(on_disk(&h, "personal/ann/mine.bin")).unwrap()), (200, b"abc".to_vec()));
    // an empty file is a file
    assert_eq!(stream(&h, "PUT", &format!("/api/v1/files/upload?{NS}&path=empty"), Some(&ann), Some(0), "", Box::new(std::io::empty())).0, 200);
    assert_eq!(std::fs::metadata(on_disk(&h, "ns/web/people/ann/empty")).unwrap().len(), 0);
    // download: exactly the bytes, with the exact length, as an attachment, with the headers every answer has
    let (s, head, body) = stream(&h, "GET", &format!("/api/v1/files/download?{NS}&path=pics/a.bin"), Some(&ann), None, "", Box::new(std::io::empty()));
    assert_eq!((s, body == data), (200, true), "{head}");
    assert!(head.contains(&format!("Content-Length: {}", data.len())) && head.contains("application/octet-stream") && head.contains("attachment; filename=\"a.bin\"") && head.contains("nosniff") && head.contains("Strict-Transport-Security"), "{head}");
    let (s, _, body) = stream(&h, "GET", &format!("/api/v1/files/download?{NS}&path=empty"), Some(&ann), None, "", Box::new(std::io::empty()));
    assert_eq!((s, body.len()), (200, 0));
    // the text routes keep their limits: the 5 MiB file is too large for them, a small binary file is not UTF-8 text
    assert_eq!(h.call("GET", &format!("/api/v1/files/content?{NS}&path=pics/a.bin"), Some(&ann), None).0, 413);
    assert_eq!(h.call("GET", &format!("/api/v1/files/content?scope=shared&{NS}&path=s.bin"), Some(&ann), None).0, 400);
    // 100-continue is answered before the body is read
    let mut d = Duplex { input: Box::new(std::io::Cursor::new(b"hello".to_vec())), out: vec![] };
    let head = format!("PUT /api/v1/files/upload?{NS}&path=c.bin HTTP/1.1\r\nAuthorization: Bearer {ann}\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n");
    assert!(crate::webservice::files::serve_stream(h.svc.shared(), &mut d, head.as_bytes(), "10.1.1.1".parse().unwrap()));
    assert!(d.out.starts_with(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK"), "{}", String::from_utf8_lossy(&d.out));
    // bytes that arrived with the head count as the start of the body, and bytes beyond Content-Length are not stored
    let mut d = Duplex { input: Box::new(std::io::Cursor::new(b"cd".to_vec())), out: vec![] };
    let head = format!("PUT /api/v1/files/upload?{NS}&path=d.bin HTTP/1.1\r\nAuthorization: Bearer {ann}\r\nContent-Length: 4\r\n\r\nab");
    assert!(crate::webservice::files::serve_stream(h.svc.shared(), &mut d, head.as_bytes(), "10.1.1.1".parse().unwrap()));
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/d.bin")).unwrap(), b"abcd");
    let head = format!("PUT /api/v1/files/upload?{NS}&path=e.bin HTTP/1.1\r\nAuthorization: Bearer {ann}\r\nContent-Length: 2\r\n\r\nabcdef");
    let mut d = Duplex { input: Box::new(std::io::empty()), out: vec![] };
    assert!(crate::webservice::files::serve_stream(h.svc.shared(), &mut d, head.as_bytes(), "10.1.1.1".parse().unwrap()));
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/e.bin")).unwrap(), b"ab");
}

#[test]
fn an_upload_is_checked_before_it_is_read_and_while_it_is_read_and_leaves_nothing_when_it_fails() {
    let (h, _, ann, _, _) = acme_cloud();
    let url = |p: &str| format!("/api/v1/files/upload?{NS}&path={p}");
    // too large: refused on the header alone, not one byte read
    let (src, left, _) = lazy(u64::MAX);
    let (s, _, body) = stream(&h, "PUT", &url("big.bin"), Some(&ann), Some(crate::cloudfiles::MAX_BINARY + 1), "", Box::new(src));
    assert_eq!(s, 413, "{}", String::from_utf8_lossy(&body));
    assert_eq!(left.load(AtOrd::SeqCst), u64::MAX, "nothing was read");
    // no length, a length that is not a number
    assert_eq!(stream(&h, "PUT", &url("n.bin"), Some(&ann), None, "", Box::new(std::io::empty())).0, 411);
    assert_eq!(stream(&h, "PUT", &url("n.bin"), Some(&ann), None, "Content-Length: many\r\n", Box::new(std::io::empty())).0, 400);
    // the body ends early: refused, and the earlier file of that name is untouched
    assert_eq!(stream(&h, "PUT", &url("keep.bin"), Some(&ann), Some(3), "", Box::new(std::io::Cursor::new(b"old".to_vec()))).0, 200);
    let (s, _, body) = stream(&h, "PUT", &url("keep.bin"), Some(&ann), Some(1000), "", Box::new(std::io::Cursor::new(vec![1u8; 10])));
    assert_eq!(s, 400);
    assert!(String::from_utf8_lossy(&body).contains("ended after 10 of 1000"), "{}", String::from_utf8_lossy(&body));
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/keep.bin")).unwrap(), b"old");
    assert!(tmp_files(&h).is_empty());
    // the checksum: right (any case) is stored, wrong is refused, malformed is refused
    let good = sha(b"payload");
    let with = |c: &str| stream(&h, "PUT", &url("sum.bin"), Some(&ann), Some(7), &format!("X-Content-SHA256: {c}\r\n"), Box::new(std::io::Cursor::new(b"payload".to_vec()))).0;
    assert_eq!((with(&good.to_uppercase()), with(&"0".repeat(64)), with("nope")), (200, 400, 400));
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/sum.bin")).unwrap(), b"payload");
    assert!(tmp_files(&h).is_empty());
    // no path, a folder in the way
    assert_eq!(stream(&h, "PUT", &format!("/api/v1/files/upload?{NS}"), Some(&ann), Some(1), "", Box::new(std::io::Cursor::new(b"x".to_vec()))).0, 400);
    assert_eq!(stream(&h, "PUT", &url("keep.bin/x"), Some(&ann), Some(1), "", Box::new(std::io::Cursor::new(b"x".to_vec()))).0, 400, "a file where a folder should be");
    // a large body is consumed in pieces of at most 64 KiB, however big it is (24 MiB here, never held whole)
    let (src, left, biggest) = lazy(24 * 1024 * 1024);
    let (s, ..) = stream(&h, "PUT", &url("huge.bin"), Some(&ann), Some(24 * 1024 * 1024), "", Box::new(src));
    assert_eq!((s, left.load(AtOrd::SeqCst), biggest.load(AtOrd::SeqCst)), (200, 0, 64 * 1024));
    assert_eq!(std::fs::metadata(on_disk(&h, "ns/web/people/ann/huge.bin")).unwrap().len(), 24 * 1024 * 1024);
    // a download of a file that is shorter than promised ends as a failure, not as a short success
    let (mut f, len) = crate::cloudfiles::open_for_download(&on_disk(&h, "ns/web/people/ann/keep.bin")).unwrap();
    assert!(crate::cloudfiles::copy_exact(&mut f, &mut Vec::new(), len + 1).is_err());
}

#[test]
fn binary_routes_follow_the_same_rights_and_the_same_jail_as_the_text_routes() {
    let (h, root, ann, bob, gus) = acme_cloud();
    // bob becomes a viewer of web: he may download, not upload
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "viewer"}))).0, 200);
    let put = |tok: Option<&str>, q: &str, body: &[u8]| stream(&h, "PUT", &format!("/api/v1/files/upload?{q}"), tok, Some(body.len() as u64), "", Box::new(std::io::Cursor::new(body.to_vec()))).0;
    let get = |tok: Option<&str>, q: &str| stream(&h, "GET", &format!("/api/v1/files/download?{q}"), tok, None, "", Box::new(std::io::empty()));
    assert_eq!(put(Some(&ann), "scope=shared&namespace=web&path=doc.bin", b"\x01\x02"), 200);
    // the viewer reads the shared file, cannot write, and cannot read ann's own folder
    assert_eq!(get(Some(&bob), "scope=shared&namespace=web&path=doc.bin").2, b"\x01\x02");
    assert_eq!(put(Some(&bob), "scope=shared&namespace=web&path=x.bin", b"x"), 403);
    assert_eq!(put(Some(&bob), "namespace=web&path=x.bin", b"x"), 403);
    assert!(!on_disk(&h, "ns/web/shared/x.bin").exists() && !on_disk(&h, "ns/web/people/bob/x.bin").exists());
    // the person is always the signed-in one: bob's own folder is empty, ann's file is not reachable by name
    assert_eq!(get(Some(&bob), "namespace=web&path=doc.bin").0, 404, "the person is the signed-in one: bob's own folder has no such file");
    // a person outside the namespace gets the answer of a namespace that is not there; a stranger to the tenant is refused
    assert_eq!(get(Some(&gus), "scope=shared&namespace=web&path=doc.bin").0, 403);
    assert_eq!(put(Some(&gus), "namespace=web&path=x.bin", b"x"), 403);
    // no token, a wrong token
    assert_eq!(put(None, "namespace=web&path=x.bin", b"x"), 401);
    assert_eq!(put(Some("nope"), "namespace=web&path=x.bin", b"x"), 401);
    assert_eq!(get(None, "namespace=web&path=x.bin").0, 401);
    // another namespace
    assert_eq!(get(Some(&ann), "scope=shared&namespace=other&path=doc.bin").0, 404);
    // the jail: parent folders, absolute paths, links (planted on disk), the folder as a whole
    for bad in ["../x", "a/../../x", "/etc/passwd", "%2e%2e/x"] {
        assert_eq!(put(Some(&ann), &format!("{NS}&path={bad}"), b"x"), 400, "upload {bad}");
        assert_eq!(get(Some(&ann), &format!("{NS}&path={bad}")).0, 400, "download {bad}");
    }
    assert_eq!(get(Some(&ann), &format!("{NS}&path=")).0, 400, "no path");
    assert!(!h.dir.join("x").exists() && !on_disk(&h, "x").exists());
    #[cfg(unix)]
    {
        let outside = h.dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        assert_eq!(put(Some(&ann), &format!("{NS}&path=ok/f.bin"), b"x"), 200);
        let dir = on_disk(&h, "ns/web/people/ann");
        std::os::unix::fs::symlink(&outside, dir.join("lnk")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), dir.join("flnk")).unwrap();
        for p in ["lnk/secret", "lnk/new.bin", "lnk", "flnk"] {
            assert_eq!(get(Some(&ann), &format!("{NS}&path={p}")).0, 400, "download {p}");
            assert_eq!(put(Some(&ann), &format!("{NS}&path={p}"), b"x"), 400, "upload {p}");
        }
        assert_eq!(std::fs::read_to_string(outside.join("secret")).unwrap(), "s");
        assert!(!outside.join("new.bin").exists());
    }
    assert_eq!(get(Some(&ann), &format!("{NS}&path=ok")).0, 400, "a folder is not a file");
    assert_eq!(get(Some(&ann), &format!("{NS}&path=gone.bin")).0, 404);
    // a field that names the person is not a thing
    assert_eq!(put(Some(&ann), &format!("{NS}&path=p.bin&person=bob"), b"x"), 200);
    assert!(on_disk(&h, "ns/web/people/ann/p.bin").exists() && !on_disk(&h, "ns/web/people/bob/p.bin").exists(), "the query's person is ignored: the folder is ann's");
}

#[test]
fn small_uploads_also_go_through_the_ordinary_pipeline_and_the_real_listener_streams_big_ones() {
    let (h, _, ann, _, _) = acme_cloud();
    // the pipeline: a body that is already in memory, any content type
    let mut r = req("PUT", &format!("/api/v1/files/upload?{NS}&path=small.bin"), Some(&ann), None);
    r.body = vec![0, 1, 2, 255];
    r.headers.insert("content-type".into(), "application/octet-stream".into());
    let (s, v, _) = h.call_req(r.clone());
    assert_eq!((s, v["bytes"].as_u64()), (200, Some(4)), "{v}");
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/small.bin")).unwrap(), [0, 1, 2, 255]);
    // a download cannot be a JSON answer: the pipeline says so (the listener never sends a download there)
    assert_eq!(h.call("GET", &format!("/api/v1/files/download?{NS}&path=small.bin"), Some(&ann), None).0, 400);
    // the agent profile has neither route
    let agent = harness("agent-api", |_| {});
    let t = agent.root();
    let head = format!("PUT /api/v1/files/upload?path=x HTTP/1.1\r\nAuthorization: Bearer {t}\r\nContent-Length: 1\r\n\r\n");
    let mut d = Duplex { input: Box::new(std::io::empty()), out: vec![] };
    assert!(!crate::webservice::files::serve_stream(agent.svc.shared(), &mut d, head.as_bytes(), "10.1.1.1".parse().unwrap()), "not handled: the ordinary pipeline answers 404");
    assert!(d.out.is_empty());
    // the real listener over TLS: 3 MiB up (above the 1 MiB body limit), the same bytes down; the text routes keep their limit
    let port = h.svc.local_addr().port();
    let data = noise(3 * 1024 * 1024 + 7);
    let hdr = |extra: &[(&str, &str)]| -> Vec<(String, String)> { [("Authorization", format!("Bearer {ann}"))].into_iter().map(|(k, v)| (k.to_string(), v)).chain(extra.iter().map(|(k, v)| (k.to_string(), v.to_string()))).collect() };
    let up = crate::devmode::client::https("127.0.0.1", port, Some(h.svc.fingerprint()), "PUT", &format!("/api/v1/files/upload?{NS}&path=big/tls.bin"), &hdr(&[("X-Content-SHA256", &sha(&data))]), &data, Duration::from_secs(60)).expect("answer");
    assert_eq!((up.status, up.json()["bytes"].as_u64()), (200, Some(data.len() as u64)), "{}", String::from_utf8_lossy(&up.body));
    assert_eq!(std::fs::read(on_disk(&h, "ns/web/people/ann/big/tls.bin")).unwrap(), data);
    let down = crate::devmode::client::https("127.0.0.1", port, Some(h.svc.fingerprint()), "GET", &format!("/api/v1/files/download?{NS}&path=big/tls.bin"), &hdr(&[]), b"", Duration::from_secs(60)).expect("answer");
    assert_eq!((down.status, down.content_type.as_str(), down.body == data), (200, "application/octet-stream", true));
    // refused early, the service closes without reading the body: the client may see the answer or only the closed connection
    let no = crate::devmode::client::https("127.0.0.1", port, Some(h.svc.fingerprint()), "PUT", &format!("/api/v1/files/upload?{NS}&path=big/no.bin"), &[], &data, Duration::from_secs(60));
    assert!(no.as_ref().map_or(true, |r| r.status == 401), "{:?}", no.map(|r| r.status));
    assert!(!on_disk(&h, "ns/web/people/ann/big/no.bin").exists());
    let json_big = crate::devmode::client::https("127.0.0.1", port, Some(h.svc.fingerprint()), "POST", "/api/v1/files/write", &hdr(&[("Content-Type", "application/json")]), &data, Duration::from_secs(60));
    assert!(json_big.as_ref().map_or(true, |r| r.status == 413), "the JSON routes keep the body limit: {:?}", json_big.map(|r| r.status));
    assert!(!on_disk(&h, "ns/web/people/ann/big/no.bin").exists());
    assert!(tmp_files(&h).is_empty());
}

#[test]
fn only_the_two_routes_are_streamed() {
    for yes in ["PUT /api/v1/files/upload?a=b HTTP/1.1\r\n", "PUT /api/v1/files/upload HTTP/1.1\r\n", "GET /api/v1/files/download?x HTTP/1.1\r\n"] {
        assert!(crate::webservice::files::is_stream_head(yes.as_bytes()), "{yes}");
    }
    for no in ["GET /api/v1/files/upload HTTP/1.1\r\n", "PUT /api/v1/files/uploads HTTP/1.1\r\n", "POST /api/v1/files/write HTTP/1.1\r\n", "PUT /api/v1/files/download HTTP/1.1\r\n", "GET /api/v1/files/downloadx HTTP/1.1\r\n", "", "PUT /api/v1/files/upload"] {
        assert!(!crate::webservice::files::is_stream_head(no.as_bytes()), "{no:?}");
    }
}

#[test]
fn files_a_folder_and_the_tenant_have_size_limits_and_a_refused_write_leaves_nothing() {
    let (h, _, ann, _, _) = acme_cloud();
    let mib = |n: usize| "x".repeat(n * 1024);
    // the defaults are limits too; then the tenant lowers them: 1 MiB per folder and 1 MiB for the whole tenant
    std::fs::create_dir_all(h.dir.join("cloud/config")).unwrap();
    std::fs::write(h.dir.join("cloud/config/tenant-config.json"), json!({"items": [{"kind": "storage.quota", "name": "default", "enabled": true, "values": {"place_mb": 1, "tenant_mb": 1}}]}).to_string()).unwrap();
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "a.txt", "content": mib(400)})), 200);
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "b.txt", "content": mib(400)})), 200);
    // 1.2 MiB in one folder: refused, and the file is not there
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "c.txt", "content": mib(400)})), 413);
    assert!(!on_disk(&h, "ns/web/people/ann/c.txt").exists());
    // replacing a file counts only the difference
    assert_eq!(write(&h, &ann, json!({"namespace": "web", "path": "a.txt", "content": mib(420)})), 200);
    // the shared folder has its own room, but the tenant as a whole is full
    let (s, v, _) = h.call("POST", "/api/v1/files/write", Some(&ann), Some(json!({"scope": "shared", "namespace": "web", "path": "p.md", "content": mib(300)})));
    assert_eq!(s, 413, "{v}");
    assert!(v.to_string().contains("tenant"), "{v}");
    assert!(!on_disk(&h, "ns/web/shared/p.md").exists());
    // an upload is refused before a byte is read
    let body = vec![7u8; 300 * 1024];
    let (s, head, _) = stream(&h, "PUT", "/api/v1/files/upload?namespace=web&path=big.bin", Some(&ann), Some(body.len() as u64), "", Box::new(std::io::Cursor::new(body)));
    assert_eq!(s, 413, "{head}");
    assert!(!on_disk(&h, "ns/web/people/ann/big.bin").exists() && tmp_files(&h).is_empty());
    // deleting makes room again
    assert_eq!(h.call("POST", "/api/v1/files/delete", Some(&ann), Some(json!({"namespace": "web", "path": "b.txt"}))).0, 200);
    assert_eq!(write(&h, &ann, json!({"scope": "shared", "namespace": "web", "path": "p.md", "content": mib(300)})), 200);
}
