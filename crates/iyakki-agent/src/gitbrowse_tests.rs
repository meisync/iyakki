//! Browsing against a scripted host (`FakeGitHttp`) and a scripted git (`FakeRunner`): no network, no real repository.

use super::*;
use crate::gitconn::{FakeGitHttp, NetErr};
use crate::wsexec::{FakeRunner, Out};
use serde_json::json;
use std::sync::Mutex;

const TOK: &str = "tok_FAKEFAKEFAKEFAKE1234";

fn conn(kind: Kind, url: &str) -> Connection {
    Connection { id: "c".into(), name: "C".into(), kind, url: url.into(), ..Default::default() }
}

fn allow() -> Egress {
    Arc::new(|_| Ok(()))
}

fn browser(kind: Kind, url: &str, http: &Arc<FakeGitHttp>) -> Browser {
    Browser::new(http.clone(), conn(kind, url), TOK, None, allow()).with_clock(Arc::new(|| 1_800_000_000))
}

fn b64(s: &[u8]) -> String {
    crate::sshkey::b64(s)
}

fn page_of(n: usize, kind: Kind) -> String {
    let items: Vec<Value> = (0..n)
        .map(|i| match kind {
            Kind::GitLab => json!({"path_with_namespace": format!("team/r{i}"), "visibility": "private", "http_url_to_repo": format!("https://gl.example/team/r{i}.git"), "ssh_url_to_repo": format!("git@gl.example:team/r{i}.git"), "web_url": format!("https://gl.example/team/r{i}")}),
            _ => json!({"full_name": format!("team/r{i}"), "private": true, "clone_url": format!("https://git.example.org/team/r{i}.git"), "ssh_url": format!("git@git.example.org:team/r{i}.git"), "html_url": format!("https://git.example.org/team/r{i}")}),
        })
        .collect();
    Value::Array(items).to_string()
}

// ------------------------------------------------------------------ repositories

#[test]
fn a_gitea_repository_list_is_read_with_everything_the_host_gives() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("GET https://git.example.org/api/v1/user/repos?limit=50&page=1", 200, &[("Link", "<https://git.example.org/api/v1/user/repos?page=2>; rel=\"next\"")], &json!([
        {"full_name":"ops/infra","description":"Cluster files","default_branch":"main","private":true,"updated_at":"2026-10-22T10:00:00Z","stars_count":3,"size":2048,"clone_url":"https://git.example.org/ops/infra.git","ssh_url":"git@git.example.org:ops/infra.git","html_url":"https://git.example.org/ops/infra","archived":false,"fork":false},
        {"full_name":"ops/with-cred","clone_url":"https://me:pw@git.example.org/ops/x.git"}
    ]).to_string());
    let p = browser(Kind::Gitea, "https://git.example.org", &http).repos(1).unwrap();
    assert_eq!(p.items.len(), 1, "an address holding a credential is dropped");
    let r = &p.items[0];
    assert_eq!((r.full_name.as_str(), r.default_branch.as_str(), r.private, r.stars, r.size_kb, r.updated), ("ops/infra", "main", true, Some(3), Some(2048), 1_792_663_200));
    assert!(p.more, "the Link header says there is a next page");
    let q = &http.requests()[0];
    assert_eq!((q.method.as_str(), q.timeout_s, q.token.as_deref(), q.style), ("GET", TIMEOUT_S, Some(TOK), gitconn::Style::Token));
}

#[test]
fn github_and_gitlab_lists_are_read_and_paged() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("GET https://api.github.com/user/repos?per_page=50&sort=updated&page=2", 200, &[], &json!([
        {"full_name":"octo/hello","description":null,"default_branch":"trunk","private":false,"pushed_at":"2026-10-20T08:30:00+02:00","stargazers_count":120,"size":10,"clone_url":"https://github.com/octo/hello.git","ssh_url":"git@github.com:octo/hello.git","html_url":"https://github.com/octo/hello","license":{"spdx_id":"MIT","name":"MIT License"}}
    ]).to_string());
    let p = browser(Kind::GitHub, "https://github.com", &http).repos(2).unwrap();
    let r = &p.items[0];
    assert_eq!((r.full_name.as_str(), r.private, r.stars, r.license.as_str(), r.description.as_str()), ("octo/hello", false, Some(120), "MIT", ""));
    assert_eq!(r.updated, ts_of("2026-10-20T06:30:00Z"), "an offset is applied");
    assert!(!p.more, "fewer than a page and no Link header: that was all");
    let h2 = Arc::new(FakeGitHttp::default());
    h2.answer("projects?membership=true&per_page=50&order_by=last_activity_at&page=1", 200, &[("X-Next-Page", "2")], &page_of(50, Kind::GitLab));
    let p = browser(Kind::GitLab, "https://gl.example", &h2).repos(1).unwrap();
    assert_eq!(p.items.len(), 50);
    assert!(p.more, "x-next-page");
    assert_eq!(p.items[3].full_name, "team/r3");
    assert!(p.items[3].private && p.items[3].https_url.ends_with("r3.git"));
    assert_eq!(h2.requests()[0].style, gitconn::Style::Bearer);
    // no paging headers at all: a full page means there may be more
    let h3 = Arc::new(FakeGitHttp::default());
    h3.answer("page=1", 200, &[], &page_of(50, Kind::Gitea));
    assert!(browser(Kind::Gitea, "https://git.example.org", &h3).repos(1).unwrap().more);
}

#[test]
fn another_kind_of_host_has_no_api_to_browse() {
    let http = Arc::new(FakeGitHttp::default());
    let e = browser(Kind::Other, "https://git.example.org", &http).repos(1).unwrap_err();
    assert!(matches!(e, BrowseErr::Persistent(ref t) if t.contains("clone the repository")), "{e:?}");
    assert!(http.requests().is_empty());
}

// ------------------------------------------------------------------ failures

#[test]
fn failures_are_told_apart_and_nothing_secret_is_in_their_words() {
    let http = Arc::new(FakeGitHttp::default());
    let b = browser(Kind::Gitea, "https://git.example.org", &http);
    http.fail("page=1", NetErr::Unreachable(format!("could not connect, token {TOK}")));
    let e = b.repos(1).unwrap_err();
    assert!(matches!(e, BrowseErr::Transient(_)) && !e.text().contains(TOK) && e.verdict() == crate::k8sretry::Verdict::Transient, "{e:?}");
    for (status, persistent) in [(401u16, true), (403, true), (404, true), (500, false), (502, false), (503, false)] {
        let h = Arc::new(FakeGitHttp::default());
        h.answer("page=1", status, &[], &format!("{{\"message\":\"nope {TOK}\"}}"));
        let e = browser(Kind::Gitea, "https://git.example.org", &h).repos(1).unwrap_err();
        assert_eq!(matches!(e, BrowseErr::Persistent(_)), persistent, "{status}: {e:?}");
        assert!(!e.text().contains(TOK), "{status}");
    }
    let h = Arc::new(FakeGitHttp::default());
    h.fail("page=1", NetErr::Tls(format!("self signed certificate {TOK}")));
    let e = browser(Kind::Gitea, "https://git.example.org", &h).repos(1).unwrap_err();
    assert!(matches!(e, BrowseErr::Persistent(ref t) if t.contains("certificate") && !t.contains(TOK)), "{e:?}");
    // the answer is not what the kind of host should say
    let h = Arc::new(FakeGitHttp::default());
    h.answer("page=1", 200, &[], "<html>login</html>");
    assert!(matches!(browser(Kind::Gitea, "https://git.example.org", &h).repos(1), Err(BrowseErr::Persistent(t)) if t.contains("Gitea")));
}

#[test]
fn a_rate_limit_is_waited_for_by_the_hosts_own_numbers() {
    let cases: Vec<(u16, Vec<(&str, &str)>, &str, u64)> = vec![
        (429, vec![("Retry-After", "17")], "", 17),
        (429, vec![], "", DEFAULT_WAIT_S),
        (403, vec![("X-RateLimit-Remaining", "0"), ("X-RateLimit-Reset", "1800000042")], "", 42),
        (403, vec![], "API rate limit exceeded for user", DEFAULT_WAIT_S),
        (429, vec![("Retry-After", "999999")], "", MAX_WAIT_S),
        (429, vec![("Retry-After", "0")], "", 1),
    ];
    for (status, headers, body, want) in cases {
        let h = Arc::new(FakeGitHttp::default());
        h.answer("page=1", status, &headers, body);
        let e = browser(Kind::GitHub, "https://github.com", &h).repos(1).unwrap_err();
        assert_eq!(e, BrowseErr::Rate(want), "{status} {headers:?}");
        assert!(e.text().contains(&format!("retry in {want} s")) && e.verdict() == crate::k8sretry::Verdict::Transient);
    }
    // a plain 403 is a refusal, not a rate limit
    let h = Arc::new(FakeGitHttp::default());
    h.answer("page=1", 403, &[], "forbidden");
    assert!(matches!(browser(Kind::GitHub, "https://github.com", &h).repos(1), Err(BrowseErr::Persistent(_))));
}

#[test]
fn the_network_policy_is_asked_before_every_request() {
    let http = Arc::new(FakeGitHttp::default());
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = asked.clone();
    let deny: Egress = Arc::new(move |h| {
        a2.lock().unwrap().push(h.to_string());
        Err(format!("Your account may not reach {h}"))
    });
    let b = Browser::new(http.clone(), conn(Kind::Gitea, "https://git.example.org:3000"), TOK, None, deny);
    assert!(matches!(b.repos(1), Err(BrowseErr::Persistent(t)) if t.contains("may not reach git.example.org")));
    assert!(b.repo("a/b").dir("main", "").is_err() && b.repo("a/b").commits("main").is_err());
    assert!(http.requests().is_empty(), "nothing was sent");
    assert_eq!(asked.lock().unwrap().len(), 3);
}

// ------------------------------------------------------------------ files

#[test]
fn directories_are_listed_lazily_folders_first_for_each_kind() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("/api/v1/repos/ops/infra/contents/docs?ref=main", 200, &[], &json!([{"name":"b.md","path":"docs/b.md","type":"file","size":10},{"name":"sub","path":"docs/sub","type":"dir","size":0},{"name":"A.md","path":"docs/A.md","type":"file","size":5},{"name":"l","path":"docs/l","type":"symlink","size":1}]).to_string());
    http.answer("/api/v1/repos/ops/infra/contents?ref=dev%2Fx", 200, &[], &json!([{"name":"README.md","path":"README.md","type":"file","size":99}]).to_string());
    let r = browser(Kind::Gitea, "https://git.example.org", &http).repo("ops/infra");
    let d = r.dir("main", "docs").unwrap();
    assert_eq!(d.entries.iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), vec![("sub", EntryKind::Dir), ("A.md", EntryKind::File), ("b.md", EntryKind::File), ("l", EntryKind::Link)]);
    assert_eq!(r.dir("dev/x", "").unwrap().entries[0].path, "README.md");
    assert_eq!(find_readme(&r.dir("dev/x", "").unwrap()).unwrap().name, "README.md");
    // only the directory asked for is requested
    assert_eq!(http.requests().len(), 3, "one request for each folder opened, never the whole tree");
    let g = Arc::new(FakeGitHttp::default());
    g.answer("/api/v4/projects/team%2Fapp/repository/tree?per_page=100&ref=main&path=src%2Futil", 200, &[], &json!([{"id":"1","name":"x.rs","type":"blob","path":"src/util/x.rs"},{"id":"2","name":"m","type":"tree","path":"src/util/m"},{"id":"3","name":"sm","type":"commit","path":"src/util/sm"}]).to_string());
    let d = browser(Kind::GitLab, "https://gl.example", &g).repo("team/app").dir("main", "src/util").unwrap();
    assert_eq!(d.entries.iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), vec![("m", EntryKind::Dir), ("sm", EntryKind::Submodule), ("x.rs", EntryKind::File)]);
    let h = Arc::new(FakeGitHttp::default());
    h.answer("/repos/octo/hello/contents?ref=main", 200, &[], "[]");
    assert!(browser(Kind::GitHub, "https://github.com", &h).repo("octo/hello").dir("main", "").unwrap().entries.is_empty());
}

#[test]
fn a_huge_directory_is_cut_and_says_so() {
    let http = Arc::new(FakeGitHttp::default());
    let items: Vec<Value> = (0..(DIR_MAX + 40)).map(|i| json!({"name":format!("f{i:04}"),"path":format!("f{i:04}"),"type":"file","size":1})).collect();
    http.answer("/contents", 200, &[], &Value::Array(items).to_string());
    let d = browser(Kind::Gitea, "https://git.example.org", &http).repo("a/b").dir("", "").unwrap();
    assert_eq!((d.entries.len(), d.cut), (DIR_MAX, true));
}

#[test]
fn files_come_back_as_bytes_in_each_kind_and_the_limits_hold() {
    let http = Arc::new(FakeGitHttp::default());
    let body = |s: &[u8]| json!({"type":"file","encoding":"base64","size":s.len(),"content":b64(s)}).to_string();
    http.answer("/repos/ops/infra/contents/src/a.txt?ref=main", 200, &[], &body(b"hello\nworld\n"));
    let r = browser(Kind::Gitea, "https://git.example.org", &http).repo("ops/infra");
    let f = r.file("main", "src/a.txt", 0).unwrap();
    assert_eq!((f.bytes.as_slice(), f.size, f.too_big), (&b"hello\nworld\n"[..], 12, false));
    assert_eq!(classify("a.txt", &f.bytes), Content::Text("hello\nworld\n".into()));
    // a file the listing says is large is not asked for at all
    let n = http.requests().len();
    let big = r.file("main", "src/big.bin", FILE_MAX + 1).unwrap();
    assert!(big.too_big && big.bytes.is_empty() && big.size == FILE_MAX + 1);
    assert_eq!(http.requests().len(), n);
    // the host says the size only in its answer
    http.answer("/contents/src/late.txt", 200, &[], &json!({"type":"file","encoding":"base64","size":FILE_MAX + 5,"content":""}).to_string());
    assert!(r.file("main", "src/late.txt", 0).unwrap().too_big);
    // GitHub leaves big files out of the answer
    let gh = Arc::new(FakeGitHttp::default());
    gh.answer("/contents/x.bin", 200, &[], &json!({"type":"file","encoding":"none","size":2_000_000,"content":""}).to_string());
    assert!(browser(Kind::GitHub, "https://github.com", &gh).repo("o/r").file("main", "x.bin", 0).unwrap().too_big);
    // a request that overflows the byte limit is the same answer
    let ov = Arc::new(FakeGitHttp::default());
    ov.fail("/contents/y.bin", NetErr::Other(TOO_BIG.into()));
    assert!(browser(Kind::Gitea, "https://git.example.org", &ov).repo("o/r").file("main", "y.bin", 0).unwrap().too_big);
    // GitLab: the path is one encoded piece
    let gl = Arc::new(FakeGitHttp::default());
    gl.answer("/projects/team%2Fapp/repository/files/dir%2Fa%20b.txt?ref=main", 200, &[], &body(b"x"));
    assert_eq!(browser(Kind::GitLab, "https://gl.example", &gl).repo("team/app").file("main", "dir/a b.txt", 0).unwrap().bytes, b"x");
    // a folder is not a file
    http.answer("/contents/docs?ref=main", 200, &[], "[]");
    assert!(r.file("main", "docs", 0).is_err());
}

#[test]
fn what_a_file_is_is_told_from_its_bytes() {
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1];
    assert_eq!(classify("pic.png", &png), Content::Image);
    assert_eq!(classify("noext", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]), Content::Image);
    assert_eq!(classify("a.rs", "fn main() {}\n".as_bytes()), Content::Text("fn main() {}\n".into()));
    assert_eq!(classify("a.rs", "ünï\n".as_bytes()), Content::Text("ünï\n".into()));
    assert_eq!(classify("a.zip", b"PK\x03\x04abc"), Content::Binary("ZIP archive".into()));
    assert_eq!(classify("x", b"\x7fELF\x02\x01\x00\x00"), Content::Binary("ELF program".into()));
    assert_eq!(classify("data", &[0, 1, 2, 3, 255]), Content::Binary("binary data".into()));
    assert_eq!(classify("a.txt", &[b'a', 0xff, 0xfe, b'b']), Content::Binary("binary data".into()), "not UTF-8");
    assert!(is_readme("README.md") && is_readme("readme") && is_readme("Readme.rst") && !is_readme("readme_old.txt") && !is_readme("docs"));
    let d = Dir { entries: ["README.rst", "README.md", "readme"].iter().map(|n| TreeEntry { name: n.to_string(), path: n.to_string(), kind: EntryKind::File, size: 1 }).collect(), cut: false };
    assert_eq!(find_readme(&d).unwrap().name, "README.md", "Markdown first");
}

// ------------------------------------------------------------------ commits and diffs

#[test]
fn commits_are_read_for_each_kind() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("/api/v1/repos/ops/infra/commits?limit=50&stat=false&verification=false&files=false&sha=main", 200, &[], &json!([
        {"sha":"abcdef0123456789","commit":{"message":"Fix the thing\n\nLonger words.","author":{"name":"Ada","date":"2026-10-22T10:00:00Z"}}}
    ]).to_string());
    let c = browser(Kind::Gitea, "https://git.example.org", &http).repo("ops/infra").commits("main").unwrap();
    assert_eq!((c[0].short().as_str(), c[0].subject().as_str(), c[0].author.as_str(), c[0].when), ("abcdef0", "Fix the thing", "Ada", 1_792_663_200));
    assert!(c[0].message.contains("Longer words."));
    let gl = Arc::new(FakeGitHttp::default());
    gl.answer("/repository/commits?per_page=50&ref_name=main", 200, &[], &json!([{"id":"fedcba9876","title":"T","message":"T\n\nbody","author_name":"Bo","authored_date":"2026-10-22T10:00:00.000+00:00"}]).to_string());
    let c = browser(Kind::GitLab, "https://gl.example", &gl).repo("team/app").commits("main").unwrap();
    assert_eq!((c[0].author.as_str(), c[0].when), ("Bo", 1_792_663_200));
    let gh = Arc::new(FakeGitHttp::default());
    let many: Vec<Value> = (0..80).map(|i| json!({"sha":format!("{i:040x}"),"commit":{"message":"m","author":{"name":"x","date":"2026-10-22T10:00:00Z"}}})).collect();
    gh.answer("/commits?per_page=50&sha=main", 200, &[], &Value::Array(many).to_string());
    assert_eq!(browser(Kind::GitHub, "https://github.com", &gh).repo("o/r").commits("main").unwrap().len(), COMMITS, "never more than 50");
}

const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    old();\n+    new();\n }\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..333\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/old name.txt b/new name.txt\nsimilarity index 90%\nrename from old name.txt\nrename to new name.txt\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";

#[test]
fn a_unified_diff_is_split_by_file_with_counts_and_line_kinds() {
    let d = parse_unified(DIFF);
    assert!(!d.too_large);
    let f: Vec<(&str, char, u32, u32)> = d.files.iter().map(|f| (f.name(), f.status, f.added, f.removed)).collect();
    assert_eq!(f, vec![("src/a.rs", 'M', 1, 1), ("new.txt", 'A', 2, 0), ("gone.txt", 'D', 0, 1), ("new name.txt", 'R', 0, 0), ("img.png", 'M', 0, 0)]);
    assert!(d.files[4].binary && !d.files[0].binary);
    let kinds: Vec<LineKind> = d.files[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, vec![LineKind::Meta, LineKind::Meta, LineKind::Meta, LineKind::Meta, LineKind::Hunk, LineKind::Ctx, LineKind::Del, LineKind::Add, LineKind::Ctx]);
    // a removed line that looks like a header is still a removed line
    let d = parse_unified("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,1 @@\n--- a/not a header\n ctx\n");
    assert_eq!((d.files[0].removed, d.files[0].added), (1, 0));
}

#[test]
fn a_diff_over_the_limit_keeps_the_file_list_and_drops_the_lines() {
    let mut t = String::from("diff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -1 +1 @@\n");
    while t.len() <= DIFF_MAX {
        t.push_str("+xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    }
    t.push_str("diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-a\n+b\n");
    let d = parse_unified(&t);
    assert!(d.too_large);
    assert_eq!(d.files.iter().map(|f| f.name()).collect::<Vec<_>>(), vec!["big.txt", "b.txt"]);
    assert!(d.files.iter().all(|f| f.lines.is_empty()) && d.files[1].added == 1);
}

#[test]
fn one_commits_diff_is_asked_the_way_each_host_wants() {
    let gitea = Arc::new(FakeGitHttp::default());
    gitea.answer("/git/commits/abc123.diff", 404, &[], "");
    gitea.answer("/repos/ops/infra/commits/abc123.diff", 200, &[], DIFF);
    let d = browser(Kind::Gitea, "https://git.example.org", &gitea).repo("ops/infra").diff("abc123").unwrap();
    assert_eq!(d.files.len(), 5, "the second address is tried when the first is not there");
    assert!(gitea.lines().iter().all(|l| l.starts_with("GET ")));
    let gh = Arc::new(FakeGitHttp::default());
    gh.answer("/repos/o/r/commits/abc123", 200, &[], DIFF);
    assert_eq!(browser(Kind::GitHub, "https://github.com", &gh).repo("o/r").diff("abc123").unwrap().files.len(), 5);
    assert_eq!(gh.requests()[0].accept.as_deref(), Some("application/vnd.github.diff"));
    let gl = Arc::new(FakeGitHttp::default());
    gl.answer("/projects/team%2Fapp/repository/commits/abc123/diff", 200, &[], &json!([
        {"old_path":"a.rs","new_path":"a.rs","new_file":false,"deleted_file":false,"renamed_file":false,"diff":"@@ -1 +1 @@\n-a\n+b\n"},
        {"old_path":"n.txt","new_path":"n.txt","new_file":true,"deleted_file":false,"renamed_file":false,"diff":"@@ -0,0 +1 @@\n+n"},
        {"old_path":"i.png","new_path":"i.png","new_file":false,"deleted_file":false,"renamed_file":false,"diff":""}
    ]).to_string());
    let d = browser(Kind::GitLab, "https://gl.example", &gl).repo("team/app").diff("abc123").unwrap();
    assert_eq!(d.files.iter().map(|f| (f.name(), f.status, f.added, f.removed, f.binary)).collect::<Vec<_>>(), vec![("a.rs", 'M', 1, 1, false), ("n.txt", 'A', 1, 0, false), ("i.png", 'M', 0, 0, true)]);
    // too large for the byte limit: the fallback, not an error
    let big = Arc::new(FakeGitHttp::default());
    big.fail("/commits/abc123", NetErr::Other(TOK.len().to_string()));
    big.fail("/commits/abc123", NetErr::Other(TOO_BIG.into()));
    assert!(matches!(browser(Kind::GitHub, "https://github.com", &big).repo("o/r").diff("abc123"), Err(_)));
    let big = Arc::new(FakeGitHttp::default());
    big.fail("/commits/abc123", NetErr::Other(TOO_BIG.into()));
    let d = browser(Kind::GitHub, "https://github.com", &big).repo("o/r").diff("abc123").unwrap();
    assert!(d.too_large && d.files.is_empty());
    // only a commit id is accepted
    assert!(browser(Kind::GitHub, "https://github.com", &big).repo("o/r").diff("main; rm -rf").is_err());
}

// ------------------------------------------------------------------ branches, tags, issues

#[test]
fn branches_and_tags_mark_the_default_and_come_in_a_fixed_order() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("/repos/ops/infra/branches?limit=50", 200, &[], &json!([{"name":"zeta","commit":{"id":"a1","message":"Z\nbody","timestamp":"2026-10-22T10:00:00Z"}},{"name":"main","commit":{"id":"b2","message":"M","timestamp":"2026-10-22T11:00:00Z"}}]).to_string());
    http.answer("/repos/ops/infra/tags?limit=50", 200, &[], &json!([{"name":"v1","commit":{"sha":"c3"}}]).to_string());
    let r = browser(Kind::Gitea, "https://git.example.org", &http).repo("ops/infra").with_default("main");
    let b = r.branches().unwrap();
    assert_eq!(b.iter().map(|x| (x.name.as_str(), x.default, x.subject.as_str())).collect::<Vec<_>>(), vec![("main", true, "M"), ("zeta", false, "Z")]);
    let t = r.tags().unwrap();
    assert_eq!((t[0].name.as_str(), t[0].sha.as_str(), t[0].tag), ("v1", "c3", true));
    let gl = Arc::new(FakeGitHttp::default());
    gl.answer("/repository/branches?per_page=50", 200, &[], &json!([{"name":"dev","default":false,"commit":{"id":"1","title":"t","committed_date":"2026-10-22T10:00:00Z"}},{"name":"trunk","default":true,"commit":{"id":"2","title":"u"}}]).to_string());
    let b = browser(Kind::GitLab, "https://gl.example", &gl).repo("team/app").branches().unwrap();
    assert_eq!(b[0].name, "trunk");
}

#[test]
fn issues_and_pull_requests_are_read_only_lists_for_each_kind() {
    let gitea = Arc::new(FakeGitHttp::default());
    gitea.answer("/repos/ops/infra/issues?type=issues&state=all&limit=50", 200, &[], &json!([{"number":4,"title":"Broken","state":"open","user":{"login":"ada"},"updated_at":"2026-10-22T10:00:00Z","labels":[{"name":"bug"},{"name":"p1"}],"body":"It breaks.","comments":2}]).to_string());
    gitea.answer("/repos/ops/infra/issues?type=pulls&state=all&limit=50", 200, &[], &json!([{"number":5,"title":"Fix","state":"closed","merged":true,"user":{"login":"bo"},"updated_at":"2026-10-22T10:00:00Z","labels":[],"body":"b"}]).to_string());
    let r = browser(Kind::Gitea, "https://git.example.org", &gitea).repo("ops/infra");
    let i = r.issues(false).unwrap();
    assert_eq!((i[0].number, i[0].title.as_str(), i[0].state.as_str(), i[0].author.as_str(), i[0].labels.clone(), i[0].comments, i[0].pr), (4, "Broken", "open", "ada", vec!["bug".to_string(), "p1".to_string()], 2, false));
    let p = r.issues(true).unwrap();
    assert!(p[0].pr && p[0].state == "merged");
    let gh = Arc::new(FakeGitHttp::default());
    gh.answer("/repos/o/r/issues?state=all&per_page=50&sort=updated", 200, &[], &json!([{"number":1,"title":"I","state":"open","user":{"login":"a"},"updated_at":"2026-10-22T10:00:00Z","labels":[]},{"number":2,"title":"PR in the issue list","state":"open","user":{"login":"a"},"updated_at":"2026-10-22T10:00:00Z","labels":[],"pull_request":{}}]).to_string());
    gh.answer("/repos/o/r/pulls?state=all&per_page=50&sort=updated", 200, &[], &json!([{"number":2,"title":"PR","state":"open","user":{"login":"a"},"updated_at":"2026-10-22T10:00:00Z"}]).to_string());
    let r = browser(Kind::GitHub, "https://github.com", &gh).repo("o/r");
    assert_eq!(r.issues(false).unwrap().iter().map(|i| i.number).collect::<Vec<_>>(), vec![1], "pull requests are not issues");
    assert_eq!(r.issues(true).unwrap()[0].number, 2);
    let gl = Arc::new(FakeGitHttp::default());
    gl.answer("/projects/team%2Fapp/merge_requests?state=all&per_page=50&order_by=updated_at", 200, &[], &json!([{"iid":9,"title":"MR","state":"opened","author":{"username":"cy"},"updated_at":"2026-10-22T10:00:00Z","labels":["x"],"description":"d"}]).to_string());
    let m = browser(Kind::GitLab, "https://gl.example", &gl).repo("team/app").issues(true).unwrap();
    assert_eq!((m[0].number, m[0].state.as_str(), m[0].author.as_str(), m[0].body.as_str()), (9, "open", "cy", "d"));
    // nothing but GET was ever sent
    for h in [&gitea, &gh, &gl] {
        assert!(h.requests().iter().all(|q| q.method == "GET" && q.body.is_none()));
    }
}

#[test]
fn refs_and_paths_that_could_become_options_are_refused_before_any_request() {
    let http = Arc::new(FakeGitHttp::default());
    let r = browser(Kind::Gitea, "https://git.example.org", &http).repo("a/b");
    for bad in ["--output=x", "a b", "a;b", "a\nb", "x`y`"] {
        assert!(r.dir(bad, "").is_err() && r.commits(bad).is_err(), "{bad:?}");
    }
    for bad in ["../etc", "a/../b", "/abs", "-x", "a\0b"] {
        assert!(r.dir("main", bad).is_err() && r.file("main", bad, 0).is_err(), "{bad:?}");
    }
    assert!(http.requests().is_empty());
}

#[test]
fn the_token_is_only_ever_in_the_header_not_in_an_address() {
    let http = Arc::new(FakeGitHttp::default());
    http.answer("page=1", 200, &[], "[]");
    http.answer("/contents", 404, &[], "");
    let b = browser(Kind::Gitea, "https://git.example.org", &http);
    let _ = b.repos(1);
    let _ = b.repo("a/b").dir("main", "src");
    for q in http.requests() {
        assert!(!q.url.contains(TOK), "{}", q.url);
        assert_eq!(q.token.as_deref(), Some(TOK));
    }
    // no token at all (a public repository): nothing is sent as one
    let h = Arc::new(FakeGitHttp::default());
    h.answer("page=1", 200, &[], "[]");
    Browser::new(h.clone(), conn(Kind::Gitea, "https://git.example.org"), "", None, allow()).repos(1).unwrap();
    assert!(h.requests()[0].token.is_none());
}

// ------------------------------------------------------------------ helpers

#[test]
fn clone_addresses_are_matched_whatever_their_spelling() {
    let same = ["https://git.example.org:3000/Team/App.git", "git@git.example.org:team/app.git", "ssh://git@git.example.org:2222/team/app", "https://me@git.example.org/team/app/", "HTTP://GIT.EXAMPLE.ORG/team/app.git"];
    for s in same {
        assert_eq!(remote_key(s), "git.example.org/team/app", "{s}");
    }
    assert_ne!(remote_key("https://git.example.org/team/other.git"), remote_key(same[0]));
    assert_ne!(remote_key("https://github.com/team/app.git"), remote_key(same[0]));
}

#[test]
fn times_are_read_with_offsets_and_fractions() {
    assert_eq!(ts_of("2026-10-22T10:00:00Z"), 1_792_663_200);
    assert_eq!(ts_of("2026-10-22T12:00:00+02:00"), 1_792_663_200);
    assert_eq!(ts_of("2026-10-22T05:00:00-0500"), 1_792_663_200);
    assert_eq!(ts_of("2026-10-22T10:00:00.123456Z"), 1_792_663_200);
    assert_eq!(ts_of("2026-10-22T10:00:00.5+00:00"), 1_792_663_200);
    assert_eq!(ts_of("nonsense"), 0);
    assert_eq!(ts_of(""), 0);
}

// ------------------------------------------------------------------ a clone on this computer

fn git_fake() -> Arc<FakeRunner> {
    Arc::new(FakeRunner::new(&["git"]))
}

fn local(run: &Arc<FakeRunner>) -> LocalRepo {
    LocalRepo::new(run.clone(), Path::new("/data/Repositories/app"), Path::new("/nonexistent-tmp"))
}

#[test]
fn a_local_clone_is_read_with_git_only_and_read_only_commands() {
    let run = git_fake();
    run.answer("ls-tree -l -z main -- src/", "100644 blob aaa      12\tsrc/lib.rs\0040000 tree bbb       -\tsrc/util\0120000 blob ccc       4\tsrc/link\0160000 commit ddd       -\tsrc/sm\0");
    run.answer("ls-tree -l -z HEAD", "100644 blob aaa      99\tREADME.md\0");
    let l = local(&run);
    let d = l.dir("main", "src").unwrap();
    assert_eq!(d.entries.iter().map(|e| (e.name.as_str(), e.kind, e.size)).collect::<Vec<_>>(), vec![("util", EntryKind::Dir, 0), ("lib.rs", EntryKind::File, 12), ("link", EntryKind::Link, 4), ("sm", EntryKind::Submodule, 0)]);
    assert_eq!(l.dir("", "").unwrap().entries[0].path, "README.md");
    run.answer("show main:src/lib.rs", "pub fn a() {}\n");
    let f = l.file("main", "src/lib.rs", 14).unwrap();
    assert_eq!(f.bytes, b"pub fn a() {}\n");
    run.answer("log -n50", "aaaaaaa1111\u{1f}Ada\u{1f}1792663200\u{1f}First line\n\nmore\n\u{1e}\nbbbbbbb2222\u{1f}Bo\u{1f}1792663100\u{1f}Second\n\u{1e}\n");
    let c = l.commits("main").unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].subject().as_str(), c[0].author.as_str(), c[0].when, c[1].short().as_str()), ("First line", "Ada", 1_792_663_200, "bbbbbbb"));
    assert!(c[0].message.contains("more"));
    run.answer("show --format= --patch --no-color --no-ext-diff -M abc123 --", DIFF);
    assert_eq!(l.diff("abc123").unwrap().files.len(), 5);
    run.answer("symbolic-ref --short -q refs/remotes/origin/HEAD", "origin/main\n");
    run.answer("refs/heads", "dev\u{1f}1111\u{1f}Dev work\u{1f}1792663000\nmain\u{1f}2222\u{1f}Main work\u{1f}1792663100\n");
    run.answer("refs/tags", "v2\u{1f}3333\u{1f}Release two\u{1f}1792663200\n");
    let b = l.branches().unwrap();
    assert_eq!(b.iter().map(|x| (x.name.as_str(), x.default)).collect::<Vec<_>>(), vec![("main", true), ("dev", false)]);
    assert_eq!(l.tags().unwrap()[0].name, "v2");
    assert!(matches!(l.issues(false), Err(BrowseErr::Persistent(t)) if t.contains("host")));
    run.answer("status --porcelain=v2 --branch", "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n");
    let st = l.status().unwrap();
    assert_eq!((st.branch.as_str(), st.ahead, st.behind, st.changed, st.untracked, st.dirty()), ("main", 2, 1, 1, 1, true));
    // every call was git, with a fixed environment, and none could change the clone
    let calls = run.calls();
    assert!(calls.len() > 5);
    for c in &calls {
        assert_eq!(c.argv[0], "git");
        let line = c.line();
        let sub = c.argv.iter().skip_while(|a| *a != "-C").nth(2).map(String::as_str).unwrap_or("");
        assert!(["ls-tree", "show", "log", "for-each-ref", "cat-file", "symbolic-ref", "remote", "status"].contains(&sub), "a command that could change the clone: {line}");
        assert!(c.env.iter().any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == "0"), "{line}");
        assert_eq!(c.cwd, "/data/Repositories/app");
    }
}

#[test]
fn a_local_file_over_the_limit_is_not_read_and_a_failure_is_in_plain_words() {
    let run = git_fake();
    let l = local(&run);
    run.answer("cat-file -s HEAD:big.bin", &format!("{}\n", FILE_MAX + 10));
    let f = l.file("", "big.bin", 0).unwrap();
    assert!(f.too_big && f.size == FILE_MAX + 10);
    assert!(!run.lines().iter().any(|x| x.contains("show ")), "the content was never asked for");
    run.fail("ls-tree -l -z nope", 128, "fatal: Not a valid object name nope");
    assert!(matches!(l.dir("nope", ""), Err(BrowseErr::Persistent(t)) if t.contains("Not a valid object name")));
    let missing = Arc::new(FakeRunner::new(&[]));
    assert!(local(&missing).dir("main", "").is_err());
    // refs and paths that could be options
    assert!(l.dir("--exec=x", "").is_err() && l.dir("main", "../x").is_err() && l.file("main", "", 0).is_err() && l.diff("-p").is_err());
}

/// A runner that writes the picture `checkout-index --prefix=<dir>/` was asked for: a local picture is read through a temporary index.
struct PicRunner {
    calls: Mutex<Vec<String>>,
    png: Vec<u8>,
}

impl Runner for PicRunner {
    fn run(&self, _cwd: &Path, argv: &[String], env: &[(String, String)], _t: Duration, _cap: usize, _s: Option<crate::wsexec::Sink>) -> Out {
        let line = argv.join(" ");
        self.calls.lock().unwrap().push(format!("{line} | {}", env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")));
        if line.contains("show ") {
            // what git prints for a picture, read lossily
            return Out { code: Some(0), stdout: String::from_utf8_lossy(&self.png).into_owned(), ..Default::default() };
        }
        if let Some(p) = argv.iter().find_map(|a| a.strip_prefix("--prefix=")) {
            let target = Path::new(p).join(argv.last().unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, &self.png).unwrap();
        }
        Out { code: Some(0), ..Default::default() }
    }
    fn has(&self, p: &str) -> bool {
        p == "git"
    }
}

#[test]
fn a_local_picture_is_read_through_a_temporary_index_and_the_scratch_folder_is_removed() {
    let png: Vec<u8> = [&[0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R'][..], &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]].concat();
    let tmp = std::env::temp_dir().join(format!("gb-pic-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let run = Arc::new(PicRunner { calls: Mutex::new(vec![]), png: png.clone() });
    let l = LocalRepo::new(run.clone(), Path::new("/data/Repositories/app"), &tmp);
    let f = l.file("main", "img/logo.png", 20).unwrap();
    assert_eq!(f.bytes, png);
    assert_eq!(classify("logo.png", &f.bytes), Content::Image);
    let calls = run.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.contains("read-tree main") && c.contains("GIT_INDEX_FILE=")), "{calls:?}");
    assert!(calls.iter().any(|c| c.contains("checkout-index --prefix=") && c.contains("-- img/logo.png")), "{calls:?}");
    assert!(std::fs::read_dir(&tmp).unwrap().next().is_none(), "nothing is left in the scratch folder");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn clones_are_found_by_their_git_folder_and_matched_by_remote() {
    let root = std::env::temp_dir().join(format!("gb-find-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["app/.git", "tools/.git", "notes", "elsewhere/.git"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let extra = root.join("elsewhere");
    let run = git_fake();
    run.answer("remote get-url origin", "https://me:SECRET1234@git.example.org/team/app.git\n");
    run.answer("symbolic-ref --short -q HEAD", "main\n");
    let found = find_clones(&(run.clone() as Arc<dyn Runner>), &root, &[extra.clone(), root.join("app")]);
    assert_eq!(found.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["app", "elsewhere", "tools"], "not `notes` (no .git), and `app` once");
    assert_eq!(found[0].branch, "main");
    assert_eq!(found[0].key().as_deref(), Some("git.example.org/team/app"));
    assert!(!found[0].remote.as_deref().unwrap().contains("SECRET1234"), "a credential in the address is never kept");
    let _ = std::fs::remove_dir_all(&root);
}

/// A runner that reads the private curl config while it exists and answers a cut-off body.
struct CutOff {
    config: Mutex<String>,
    cap: Mutex<usize>,
}

impl Runner for CutOff {
    fn run(&self, _cwd: &Path, argv: &[String], _env: &[(String, String)], _t: Duration, cap: usize, _s: Option<crate::wsexec::Sink>) -> Out {
        *self.cap.lock().unwrap() = cap;
        *self.config.lock().unwrap() = std::fs::read_to_string(&argv[2]).unwrap_or_default();
        Out { code: Some(0), stdout: "HTTP/1.1 200 OK\r\n\r\nthe first part of a very long diff".into(), truncated: true, ..Default::default() }
    }
    fn has(&self, p: &str) -> bool {
        p == "curl"
    }
}

#[test]
fn curl_asks_for_the_accept_type_and_time_limit_of_the_request_and_says_when_the_answer_was_cut() {
    let d = std::env::temp_dir().join(format!("gb-curl-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let run = Arc::new(CutOff { config: Mutex::new(String::new()), cap: Mutex::new(0) });
    let h = gitconn::CurlGit { run: run.clone(), tmp: d.clone() };
    let e = Browser::new(Arc::new(h), conn(Kind::GitHub, "https://github.com"), TOK, None, allow()).repo("o/r").diff("abc123").unwrap();
    assert!(e.too_large, "a cut-off answer is the too-large fallback, not an error about the network");
    let cfg = run.config.lock().unwrap().clone();
    assert!(cfg.contains("Accept: application/vnd.github.diff") && cfg.contains("max-time = 15") && cfg.contains("request = \"GET\""), "{cfg}");
    assert!(cfg.contains(TOK), "the token is in the private config file ...");
    assert!(*run.cap.lock().unwrap() > DIFF_MAX, "... and the answer limit is the request's own");
    assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "... which is removed afterwards");
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ the working copy (AI IDE Git tab)

const PORCELAIN: &str = "# branch.oid 1234\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\n1 A. N... 000000 100644 100644 000 ccc new.txt\n1 D. N... 100644 000000 000000 ddd 000 old.txt\n2 R. N... 100644 100644 100644 eee fff R100 moved.rs\tbefore.rs\nu UU N... 100644 100644 100644 100644 a b c conflict.rs\n? notes.md\n";

#[test]
fn working_copy_changes_are_parsed_with_status_and_staging() {
    let v = parse_changes(PORCELAIN);
    assert_eq!(v.len(), 6);
    let by = |p: &str| v.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("{p}"));
    assert_eq!((by("src/a b.rs").kind, by("src/a b.rs").staged, by("src/a b.rs").unstaged), (ChangeKind::Modified, false, true), "a path with a space stays whole");
    assert_eq!((by("new.txt").kind, by("new.txt").staged), (ChangeKind::Added, true));
    assert_eq!(by("old.txt").kind, ChangeKind::Deleted);
    assert_eq!((by("moved.rs").kind, by("moved.rs").from.as_str()), (ChangeKind::Renamed, "before.rs"));
    assert_eq!(by("conflict.rs").kind, ChangeKind::Conflicted);
    assert_eq!((by("notes.md").kind.letter(), by("notes.md").kind.word()), ('?', "new, not tracked"));
    assert!(v.windows(2).all(|w| w[0].path <= w[1].path), "sorted by path");
    assert!(parse_changes("# branch.head main\n").is_empty());
}

#[test]
fn working_reads_status_once_and_diff_head_reads_one_file_without_writing() {
    let run = Arc::new(FakeRunner::new(&["git"]));
    run.answer("status --porcelain=v2", PORCELAIN);
    run.answer("diff HEAD", "diff --git a/new.txt b/new.txt\nindex 1..2 100644\n--- a/new.txt\n+++ b/new.txt\n@@ -1 +1,2 @@\n a\n+b\n");
    let l = local(&run);
    let w = l.working().unwrap();
    assert_eq!((w.status.branch.as_str(), w.status.ahead, w.status.behind, w.changes.len(), w.more), ("main", 2, 1, 6, 0));
    let d = l.diff_head("new.txt").unwrap();
    assert_eq!((d.files.len(), d.files[0].added), (1, 1));
    assert!(l.diff_head("../etc/passwd").is_err() && l.diff_head("").is_err());
    for line in run.lines() {
        assert!(!line.contains(" commit") && !line.contains(" push") && !line.contains(" checkout") && !line.contains(" pull"), "{line}");
        assert!(line.contains("GIT") || line.starts_with("git"), "{line}");
    }
    let bad = Arc::new(FakeRunner::new(&["git"]));
    bad.fail("status", 128, "fatal: not a git repository (or any of the parent directories): .git");
    assert!(local(&bad).working().unwrap_err().text().contains("not a git repository"));
}
