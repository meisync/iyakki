//! The GIT tile's host reader (`Browser`, `RemoteRepo`) over the real in-house HTTP client (`NetGit`) against a small real HTTP server
//! on 127.0.0.1 that speaks the Gitea REST API: paged repository list, folders and files (base64), commits, branches, tags, the token
//! header, 401, 404, 429 with Retry-After and 500. No fake transport.

#[allow(unused_imports)]
use iyakki_shared::gitbrowse::*;
#[allow(unused_imports)]
#[allow(unused_imports)]
use crate::gitconn;
#[allow(unused_imports)]
use iyakki_shared::gitconn::{GitHttp, NetErr, Req, Resp, TOO_BIG};
#[allow(unused_imports)]
use iyakki_shared::wsexec::Runner;
#[allow(unused_imports)]
use serde_json::Value;
#[allow(unused_imports)]
use zeroize::Zeroizing;
#[allow(unused_imports)]
use std::{path::{Path, PathBuf}, time::Duration};
use crate::gitconn::{Connection, Kind};
use crate::gitnet::NetGit;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

fn server(h: impl Fn(&str, &str, Option<&str>) -> (u16, Vec<(String, String)>, String) + Send + Sync + 'static) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let log: Log = Arc::default();
    let (lg, h) = (log.clone(), Arc::new(h));
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (lg, h) = (lg.clone(), h.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let mut p = line.split_whitespace();
                let (m, t) = (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string());
                let (mut auth, mut len) = (None, 0usize);
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = l.trim().split_once(':') {
                        if k.eq_ignore_ascii_case("authorization") {
                            auth = Some(v.trim().to_string());
                        }
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut b = vec![0u8; len];
                let _ = r.read_exact(&mut b);
                let (code, headers, body) = h(&m, &t, auth.as_deref());
                lg.lock().unwrap().push((m, t, auth));
                let mut out = format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let mut s = s;
                let _ = s.write_all(out.as_bytes());
                let _ = s.write_all(body.as_bytes());
            });
        }
    });
    (url, log)
}

fn browser(url: &str, token: &str) -> Browser {
    let conn = Connection { id: "c".into(), name: "Local".into(), kind: Kind::Gitea, url: url.into(), ..Default::default() };
    Browser::new(Arc::new(NetGit::default()), conn, token, None, Arc::new(|_| Ok(())))
}

fn b64(s: &[u8]) -> String {
    crate::sshkey::b64(s)
}

#[test]
fn the_gitea_reader_works_over_the_real_client_against_a_real_loopback_host() {
    let (url, log) = server(|_m, t, auth| {
        let none = |c: u16, b: &str| (c, vec![], b.to_string());
        if auth != Some("token tok_LOOPBACK_1234567890") && !t.contains("/public") {
            return none(401, r#"{"message":"token is required"}"#);
        }
        let p = t.split('?').next().unwrap();
        match p {
            "/api/v1/user/repos" if t.contains("page=1") => (200, vec![("Link".into(), "<http://x/api/v1/user/repos?page=2>; rel=\"next\"".into())], r#"[{"full_name":"me/app","description":"An app","private":true,"default_branch":"main","clone_url":"http://x/me/app.git","updated_at":"2026-11-01T10:00:00Z","stars_count":3}]"#.into()),
            "/api/v1/user/repos" => none(200, r#"[{"full_name":"me/lib","description":"","private":false,"default_branch":"dev","clone_url":"http://x/me/lib.git","updated_at":"2026-10-01T10:00:00Z"}]"#),
            "/api/v1/repos/me/app/contents" => none(200, r#"[{"name":"src","path":"src","type":"dir","size":0},{"name":"README.md","path":"README.md","type":"file","size":8}]"#),
            "/api/v1/repos/me/app/contents/README.md" => none(200, &format!(r#"{{"name":"README.md","path":"README.md","type":"file","size":8,"encoding":"base64","content":"{}"}}"#, b64(b"# Hello\n"))),
            "/api/v1/repos/me/app/commits" => none(200, r#"[{"sha":"abc1234def","commit":{"message":"First\n\nbody","author":{"name":"Ada","date":"2026-11-01T10:00:00Z"}}}]"#),
            "/api/v1/repos/me/app/branches" => none(200, r#"[{"name":"main","commit":{"id":"abc1234def"}},{"name":"dev","commit":{"id":"fff"}}]"#),
            "/api/v1/repos/me/app/tags" => none(200, r#"[{"name":"v1","commit":{"sha":"abc1234def"}}]"#),
            "/api/v1/repos/me/limited/contents" => (429, vec![("Retry-After".into(), "7".into())], "{}".into()),
            "/api/v1/repos/me/broken/contents" => none(500, "oops"),
            _ => none(404, r#"{"message":"not found"}"#),
        }
    });
    let b = browser(&url, "tok_LOOPBACK_1234567890");
    let page = b.repos(1).unwrap();
    assert_eq!((page.items.len(), page.more, page.items[0].full_name.as_str(), page.items[0].private), (1, true, "me/app", true));
    let page2 = b.repos(2).unwrap();
    assert_eq!((page2.items[0].full_name.as_str(), page2.more), ("me/lib", false));
    let app = b.repo("me/app").with_default("main");
    let dir = app.dir("main", "").unwrap();
    assert_eq!(dir.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["src", "README.md"], "folders first");
    let f = app.file("main", "README.md", 8).unwrap();
    assert_eq!(f.bytes, b"# Hello\n");
    assert_eq!(app.commits("main").unwrap()[0].subject(), "First");
    assert_eq!(app.branches().unwrap().len(), 2);
    assert_eq!(app.tags().unwrap()[0].name, "v1");
    // failures: a rate limit says how long to wait, a 500 is transient (retried by the tile), a 404 is persistent
    assert!(matches!(b.repo("me/limited").dir("", ""), Err(BrowseErr::Rate(7))));
    assert!(matches!(b.repo("me/broken").dir("", ""), Err(BrowseErr::Transient(_))));
    assert!(matches!(b.repo("me/none").dir("", ""), Err(BrowseErr::Persistent(_))));
    // paths that could become options never leave the machine
    let before = log.lock().unwrap().len();
    assert!(app.file("main", "--output=x", 0).is_err() && app.dir("-x", "").is_err());
    assert_eq!(log.lock().unwrap().len(), before);
    // the token went in the header on every call, never in an address
    let seen = log.lock().unwrap().clone();
    assert!(seen.iter().all(|(_, t, a)| a.as_deref() == Some("token tok_LOOPBACK_1234567890") && !t.contains("tok_LOOPBACK")), "{seen:?}");
    // a wrong token is a plain 401 sentence that does not echo the token
    let bad = browser(&url, "tok_WRONG_0000000000000000").repos(1).unwrap_err();
    match bad {
        BrowseErr::Persistent(m) => assert!(!m.contains("tok_WRONG"), "{m}"),
        o => panic!("{o:?}"),
    }
    // a host that is not there is transient (the tile keeps the last list and retries)
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    assert!(matches!(browser(&dead, "t").repos(1), Err(BrowseErr::Transient(_))));
}
