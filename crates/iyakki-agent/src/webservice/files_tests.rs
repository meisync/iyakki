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
