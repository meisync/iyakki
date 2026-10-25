use super::*;
use crate::gitkeys::{MemStore, Session};
use crate::wsexec::FakeRunner;

const FAKE: &str = "tok_FAKEFAKEFAKEFAKE1234";

fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("gc-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Fx {
    dir: PathBuf,
    store: Arc<MemStore>,
    log: Arc<Mutex<Vec<String>>>,
    conns: Conns,
}

fn fx(tag: &str) -> Fx {
    let dir = temp(tag);
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let conns = Conns::new(&dir, store.clone(), Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}"))));
    Fx { dir, store, log, conns }
}

fn conn(kind: Kind, name: &str, url: &str) -> Connection {
    Connection { name: name.into(), kind, url: url.into(), ..Default::default() }
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn adding_a_connection_keeps_the_token_in_the_vault_only_and_the_first_one_is_the_default() {
    let f = fx("add");
    let a = f.conns.add(conn(Kind::Gitea, "Home Gitea", "https://git.example.org:3000/"), Some(FAKE)).unwrap();
    assert_eq!((a.id.as_str(), a.url.as_str(), a.token_vault.as_str(), a.default), ("home-gitea", "https://git.example.org:3000", "git.conn.home-gitea.token", true));
    assert_eq!(f.store.get("git.conn.home-gitea.token").unwrap().as_str(), FAKE);
    let b = f.conns.add(conn(Kind::GitHub, "Work", "https://github.com"), Some("ghp_FAKEFAKEFAKEFAKEFAKE")).unwrap();
    assert!(!b.default && f.conns.default_connection().unwrap().id == "home-gitea");
    f.conns.set_default(&b.id).unwrap();
    assert_eq!(f.conns.default_connection().unwrap().id, "work");
    assert_eq!(f.conns.list().iter().filter(|c| c.default).count(), 1);
    // no file holds a token; the audit has names only
    for p in walk(&f.dir) {
        let t = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(!t.contains("FAKEFAKE"), "token in {}", p.display());
    }
    let log = f.log.lock().unwrap().join("\n");
    assert!(log.contains("git.connect.add Home Gitea git.example.org") && log.contains("git.connect.default Work github.com") && !log.contains("FAKEFAKE"), "{log}");
    // refusals
    assert!(f.conns.add(conn(Kind::Gitea, "home gitea", "https://x.example"), Some(FAKE)).unwrap_err().contains("already"));
    assert!(f.conns.add(conn(Kind::Gitea, "C", "ftp://x"), Some(FAKE)).unwrap_err().contains("https://"));
    assert!(f.conns.add(conn(Kind::Gitea, "C", "https://me:pw@x.example"), Some(FAKE)).unwrap_err().contains("password"));
    assert!(f.conns.add(conn(Kind::Gitea, "C", "https://x.example"), None).unwrap_err().contains("token"));
    assert!(f.conns.add(Connection { method: Method::Ssh, ..conn(Kind::Gitea, "S", "https://x.example") }, None).unwrap_err().contains("SSH key"));
    assert!(f.conns.add(conn(Kind::Gitea, "C", "https://x.example"), Some("has space")).unwrap_err().contains("spaces"));
    // removing deletes our token entry
    f.conns.remove("home-gitea").unwrap();
    assert!(f.store.get("git.conn.home-gitea.token").is_err());
    assert!(f.log.lock().unwrap().join("\n").contains("git.connect.remove Home Gitea git.example.org"));
}

#[test]
fn replacing_a_token_and_changing_the_address_drop_what_no_longer_applies() {
    let f = fx("edit");
    let mut c = f.conns.add(conn(Kind::Gitea, "G", "https://a.example"), Some(FAKE)).unwrap();
    f.conns.record_test(&c.id, &TestReport { ok: true, account: "octo".into(), ..TestReport::blank(&c) }).unwrap();
    c = f.conns.get(&c.id).unwrap();
    assert_eq!(c.account, "octo");
    f.conns.update(&c, Some("tok_REPLACEDREPLACED12")).unwrap();
    assert_eq!(f.store.get("git.conn.g.token").unwrap().as_str(), "tok_REPLACEDREPLACED12");
    assert_eq!(f.conns.get("g").unwrap().account, "octo", "same server: the account stays");
    let mut moved = f.conns.get("g").unwrap();
    moved.url = "https://b.example".into();
    f.conns.update(&moved, None).unwrap();
    let m = f.conns.get("g").unwrap();
    assert!(m.account.is_empty() && m.last_test.is_none());
    assert!(f.conns.set_identity("g", "Octo <x>", "").is_err() && f.conns.set_identity("g", "Octo", "not-an-email").is_err());
    f.conns.set_identity("g", "Octo Cat", "octo@example.org").unwrap();
    assert_eq!(f.conns.get("g").unwrap().identity_email, "octo@example.org");
}

fn user_json(login: &str) -> String {
    format!(r#"{{"login":"{login}","username":"{login}","name":"Octo Cat","email":"octo@example.org"}}"#)
}

#[test]
fn a_gitea_test_uses_the_host_api_and_says_what_it_could_not_check() {
    let f = fx("tgitea");
    let c = f.conns.add(conn(Kind::Gitea, "G", "https://git.example.org"), Some(FAKE)).unwrap();
    let h = FakeGitHttp::default();
    h.answer("GET https://git.example.org/api/v1/user/repos", 200, &[], "[]").answer("GET https://git.example.org/api/v1/user", 200, &[], &user_json("octo"));
    let r = test_connection(&h, &c, FAKE, None);
    assert!(r.ok && r.account == "octo" && r.email == "octo@example.org" && r.can_read == Some(true) && r.can_write.is_none(), "{r:?}");
    assert!(r.notes.iter().any(|n| n.contains("does not say what a token may write")));
    assert_eq!(r.tls, Tls::Verified);
    let req = &h.requests()[0];
    assert_eq!((req.style, req.token.as_deref()), (Style::Token, Some(FAKE)));
    assert!(r.summary().contains("Signed in as octo"));
    // 401
    let h = FakeGitHttp::default();
    h.answer("/api/v1/user", 401, &[], r#"{"message":"token is invalid"}"#);
    let r = test_connection(&h, &c, FAKE, None);
    assert!(!r.ok && r.error.as_deref().unwrap().contains("refused (401)"));
    // 403 on the account: a missing scope
    let h = FakeGitHttp::default();
    h.answer("/api/v1/user", 403, &[], "{}");
    assert!(test_connection(&h, &c, FAKE, None).error.unwrap().contains("read:user"));
    // reading repositories forbidden
    let h = FakeGitHttp::default();
    h.answer("/api/v1/user/repos", 403, &[], "{}").answer("/api/v1/user", 200, &[], &user_json("octo"));
    let r = test_connection(&h, &c, FAKE, None);
    assert!(!r.ok && r.can_read == Some(false) && r.error.unwrap().contains("read:repository"));
    // not a Gitea
    let h = FakeGitHttp::default();
    h.answer("/api/v1/user", 200, &[], "<html>hi</html>");
    assert!(test_connection(&h, &c, FAKE, None).error.unwrap().contains("check the address"));
}

#[test]
fn unreachable_and_self_signed_servers_are_told_in_plain_words_and_the_token_is_scrubbed() {
    let f = fx("tnet");
    let c = f.conns.add(conn(Kind::Gitea, "G", "https://git.example.org"), Some(FAKE)).unwrap();
    let h = FakeGitHttp::default();
    h.fail("GET", NetErr::Unreachable(format!("could not resolve host (Authorization: token {FAKE})")));
    let r = test_connection(&h, &c, FAKE, None);
    let e = r.error.unwrap();
    assert!(e.starts_with("The server did not answer") && !e.contains("FAKEFAKE"), "{e}");
    let h = FakeGitHttp::default();
    h.fail("GET", NetErr::Tls("self-signed certificate".into()));
    let r = test_connection(&h, &c, FAKE, None);
    assert_eq!(r.tls, Tls::Untrusted);
    assert!(r.error.unwrap().contains("certificate is not trusted"));
    // the person compares the fingerprint and trusts it: only for this connection, and only the certificate file is kept
    let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
    let info = cert_info(pem).unwrap();
    assert_eq!(info.fingerprint, "SHA256:03:90:58:C6:F2:C0:CB:49:2C:53:3B:0A:4D:14:EF:77:CC:0F:78:AB:CC:CE:D5:28:7D:84:A1:A2:01:1C:FB:81");
    assert!(info.fingerprint.starts_with("SHA256:") && info.fingerprint.len() == 7 + 32 * 3 - 1);
    f.conns.trust_cert(&c.id, &info).unwrap();
    let c2 = f.conns.get(&c.id).unwrap();
    let cert = f.conns.pinned_cert(&c2).unwrap();
    assert_eq!(std::fs::read_to_string(&cert).unwrap(), pem);
    let h = FakeGitHttp::default();
    h.answer("/api/v1/user/repos", 200, &[], "[]").answer("/api/v1/user", 200, &[], &user_json("octo"));
    let r = test_connection(&h, &c2, FAKE, Some(&cert));
    assert_eq!(r.tls, Tls::Pinned);
    assert_eq!(h.requests()[0].cert.as_ref(), Some(&cert));
    assert!(f.log.lock().unwrap().join("\n").contains("git.connect.trust G git.example.org SHA256:"));
    // another connection does not get that trust
    let other = f.conns.add(conn(Kind::Gitea, "H", "https://other.example.org"), Some(FAKE)).unwrap();
    assert!(f.conns.pinned_cert(&other).is_none());
    // curl's output is read: headers, status, body
    let r = parse_curl("HTTP/1.1 200 OK\r\nX-OAuth-Scopes: repo, user\r\n\r\n{\"login\":\"o\"}\n200").unwrap();
    assert_eq!((r.status, r.header("x-oauth-scopes"), r.body.as_str()), (200, Some("repo, user"), "{\"login\":\"o\"}"));
    let r = parse_curl("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nA: b\r\n\r\n{}\n201").unwrap();
    assert_eq!(r.status, 201);
}

#[test]
fn a_curl_request_keeps_the_token_out_of_the_command_line_and_pins_a_certificate_file() {
    let d = temp("curl");
    let run = Arc::new(FakeRunner::new(&["curl"]));
    run.answer("curl -K", "HTTP/1.1 200 OK\r\n\r\n{\"login\":\"o\"}\n200");
    let h = CurlGit { run: run.clone(), tmp: d.clone() };
    let r = h.send(&Req { method: "GET".into(), url: "https://git.example.org/api/v1/user".into(), token: Some(FAKE.into()), style: Style::Token, body: None, cert: Some(d.join("c.pem")) }).unwrap();
    assert_eq!(r.status, 200);
    let calls = run.lines();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].contains("FAKEFAKE") && calls[0].starts_with("curl -K "), "{}", calls[0]);
    assert!(walk(&d).is_empty(), "the private config file is removed");
    // a certificate problem is a Tls error, a refused connection is not
    let run = Arc::new(FakeRunner::new(&["curl"]));
    run.fail("curl", 60, &format!("curl: (60) SSL certificate problem: self-signed certificate ({FAKE})"));
    let h = CurlGit { run, tmp: d.clone() };
    let e = h.send(&Req { method: "GET".into(), url: "https://x.example/".into(), token: Some(FAKE.into()), style: Style::Bearer, body: None, cert: None }).unwrap_err();
    assert!(matches!(&e, NetErr::Tls(t) if !t.contains("FAKEFAKE")), "{e:?}");
    let run = Arc::new(FakeRunner::new(&["curl"]));
    run.fail("curl", 7, "curl: (7) Failed to connect");
    let h = CurlGit { run, tmp: d };
    assert!(matches!(h.send(&Req { method: "GET".into(), url: "https://x.example/".into(), token: None, style: Style::Bearer, body: None, cert: None }), Err(NetErr::Unreachable(_))));
    // the certificate shown for comparison
    let run = Arc::new(FakeRunner::new(&["curl"]));
    run.answer("--certinfo", "Cert:-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n");
    let h = CurlGit { run: run.clone(), tmp: temp("curl2") };
    assert!(h.peer_cert("https://x.example").unwrap().fingerprint.starts_with("SHA256:"));
    assert!(h.peer_cert("http://x.example").is_err());
}

#[test]
fn github_and_gitlab_scopes_are_checked_where_the_api_tells() {
    let f = fx("scopes");
    let gh = f.conns.add(conn(Kind::GitHub, "GH", "https://github.com"), Some("ghp_FAKEFAKEFAKEFAKEFAKE")).unwrap();
    let h = FakeGitHttp::default();
    h.answer("GET https://api.github.com/user", 200, &[("X-OAuth-Scopes", "repo, read:org")], &user_json("octo"));
    let r = test_connection(&h, &gh, "ghp_FAKEFAKEFAKEFAKEFAKE", None);
    assert!(r.ok && r.can_write == Some(true) && r.summary().contains("can read and write"), "{r:?}");
    assert_eq!(h.requests()[0].style, Style::Bearer);
    // a classic token without repo
    let h = FakeGitHttp::default();
    h.answer("/user", 200, &[("X-OAuth-Scopes", "read:user")], &user_json("octo"));
    let r = test_connection(&h, &gh, "x", None);
    assert!(!r.ok && r.can_write == Some(false) && r.error.unwrap().contains("cannot write repositories (needs the repo scope)"));
    // a fine-grained token: no header, so it says so
    let h = FakeGitHttp::default();
    h.answer("/user", 200, &[], &user_json("octo"));
    let r = test_connection(&h, &gh, "x", None);
    assert!(r.ok && r.can_write.is_none() && r.notes[0].contains("fine-grained"));
    // GitLab: scopes of the token itself
    let gl = f.conns.add(conn(Kind::GitLab, "GL", "https://gitlab.example.org"), Some("glpat-FAKEFAKEFAKEFAKE")).unwrap();
    let h = FakeGitHttp::default();
    h.answer("/api/v4/personal_access_tokens/self", 200, &[], r#"{"scopes":["read_api","read_repository"]}"#).answer("/api/v4/user", 200, &[], &user_json("octo"));
    let r = test_connection(&h, &gl, "x", None);
    assert!(!r.ok && r.can_read == Some(true) && r.error.unwrap().contains("needs the api scope"));
    let h = FakeGitHttp::default();
    h.answer("/api/v4/personal_access_tokens/self", 200, &[], r#"{"scopes":["api"]}"#).answer("/api/v4/user", 200, &[], &user_json("octo"));
    assert!(test_connection(&h, &gl, "x", None).ok);
    // GitHub Enterprise has its own API path
    assert_eq!(api_base(Kind::GitHub, "https://ghe.example.org"), "https://ghe.example.org/api/v3");
    // another kind of host: nothing to ask, said plainly
    let o = f.conns.add(conn(Kind::Other, "O", "https://git.example.org"), Some("x1234")).unwrap();
    let r = test_connection(&FakeGitHttp::default(), &o, "x", None);
    assert!(r.ok && r.notes[0].contains("no account API"));
}

#[test]
fn creating_and_listing_repositories_use_each_host_api() {
    let f = fx("repos");
    let g = f.conns.add(conn(Kind::Gitea, "G", "https://git.example.org"), Some(FAKE)).unwrap();
    let h = FakeGitHttp::default();
    h.answer("POST https://git.example.org/api/v1/user/repos", 201, &[], r#"{"full_name":"octo/my-app","clone_url":"https://git.example.org/octo/my-app.git","ssh_url":"git@git.example.org:octo/my-app.git","html_url":"https://git.example.org/octo/my-app","private":true}"#);
    let info = create_repo(&f.conns, &h, &g, "my-app", true, "A project").unwrap();
    assert_eq!((info.full_name.as_str(), info.private), ("octo/my-app", true));
    let body: serde_json::Value = serde_json::from_str(h.requests()[0].body.as_ref().unwrap()).unwrap();
    assert_eq!((body["name"].as_str(), body["private"].as_bool(), body["auto_init"].as_bool()), (Some("my-app"), Some(true), Some(false)));
    let log = f.log.lock().unwrap().join("\n");
    assert!(log.contains("git.repo.create git.example.org octo/my-app") && !log.contains("FAKEFAKE"));
    // the other kinds
    let (p, b) = create_body(Kind::GitLab, "my-app", true, "");
    assert!(p == "/projects" && b.contains(r#""visibility":"private""#));
    assert_eq!(create_body(Kind::GitHub, "x", false, "").0, "/user/repos");
    // errors in plain words
    for (status, want) in [(401, "refused (401)"), (403, "cannot write repositories (needs write:repository)"), (409, "already exists")] {
        let h = FakeGitHttp::default();
        h.answer("POST", status, &[], r#"{"message":"x"}"#);
        let e = create_repo(&f.conns, &h, &g, "my-app", true, "").unwrap_err();
        assert!(e.contains(want), "{status}: {e}");
    }
    assert!(create_repo(&f.conns, &FakeGitHttp::default(), &g, "bad name", true, "").unwrap_err().contains("not a repository name"));
    assert!(create_repo(&f.conns, &FakeGitHttp::default(), &g, "ok", true, "").unwrap_err().contains("did not answer"));
    // list
    let h = FakeGitHttp::default();
    h.answer("GET https://git.example.org/api/v1/user/repos?limit=50", 200, &[], r#"[{"full_name":"octo/a","clone_url":"https://git.example.org/octo/a.git","ssh_url":"git@git.example.org:octo/a.git","html_url":"x","private":false},{"full_name":"octo/leak","clone_url":"https://octo:pw@git.example.org/octo/leak.git","ssh_url":"","html_url":"","private":true}]"#);
    let l = list_repos(&h, &g, FAKE, None).unwrap();
    assert_eq!(l.len(), 1, "an address with a credential in it is dropped");
    assert!(!l[0].private);
    let gl = f.conns.add(conn(Kind::GitLab, "GL", "https://gitlab.example.org"), Some("glpat-FAKEFAKEFAKEFAKE")).unwrap();
    let h = FakeGitHttp::default();
    h.answer("/api/v4/projects?membership=true", 200, &[], r#"[{"path_with_namespace":"octo/p","http_url_to_repo":"https://gitlab.example.org/octo/p.git","ssh_url_to_repo":"git@gitlab.example.org:octo/p.git","web_url":"w","visibility":"private"}]"#);
    assert_eq!(list_repos(&h, &gl, "x", None).unwrap()[0].full_name, "octo/p");
    assert!(list_repos(&h, &f.conns.add(conn(Kind::Other, "O", "https://o.example"), Some("x1234")).unwrap(), "x", None).is_err());
}

#[test]
fn where_the_data_goes_is_labelled() {
    assert_eq!(placement_of("https://github.com", false), Placement::Internet);
    assert_eq!(placement_of("http://192.168.1.20:3000", false), Placement::Lan);
    assert_eq!(placement_of("https://gitea.local", false), Placement::Lan);
    assert_eq!(placement_of("http://gitea:3000", false), Placement::Lan);
    assert_eq!(placement_of("https://8.8.8.8", false), Placement::Internet);
    assert_eq!(placement_of("http://10.0.0.5:30300", true), Placement::Cluster);
    assert_eq!(Placement::Lan.label(), "on your local network");
    assert_eq!(host_of("https://User@Git.Example.org:3000/x"), "git.example.org");
}

#[test]
fn the_cluster_gitea_is_listed_with_its_own_token_entry_and_removing_the_connection_keeps_that_entry() {
    use crate::platform::installed::{self, Installed};
    let f = fx("cluster");
    assert!(cluster_gitea(&f.dir, "alice").is_empty(), "none installed");
    let rec = Installed {
        id: "gitea-iyakki-gitea".into(),
        service: "gitea".into(),
        title: "Gitea".into(),
        kind: "git".into(),
        urls: vec![crate::platform::UrlInfo { label: "Gitea".into(), url: "http://10.0.0.5:30300".into(), kind: "web".into() }],
        vault: vec!["svc.gitea-iyakki-gitea.token".into()],
        ..Default::default()
    };
    installed::save(&f.dir, "alice", &[rec]).unwrap();
    let l = cluster_gitea(&f.dir, "alice");
    assert_eq!((l[0].url.as_str(), l[0].token_vault.as_str(), l[0].problem.as_str()), ("http://10.0.0.5:30300", "svc.gitea-iyakki-gitea.token", ""));
    assert!(cluster_gitea(&f.dir, "bob").is_empty());
    f.store.set("svc.gitea-iyakki-gitea.token", "tok_CLUSTERFAKEFAKEFAKE1").unwrap();
    let c = f.conns.add(Connection { cluster: l[0].instance.clone(), token_vault: l[0].token_vault.clone(), ..conn(Kind::Gitea, "My cluster", &l[0].url) }, None).unwrap();
    assert_eq!(c.placement(), Placement::Cluster);
    assert_eq!(f.conns.token(&c).unwrap().as_str(), "tok_CLUSTERFAKEFAKEFAKE1");
    assert!(!f.store.names().unwrap().iter().any(|n| n.starts_with("git.conn.")), "the token is not copied");
    f.conns.remove(&c.id).unwrap();
    assert!(f.store.get("svc.gitea-iyakki-gitea.token").is_ok(), "the platform's token stays");
}

#[test]
fn the_workspace_git_side_is_read_and_changed_through_the_runner() {
    let d = temp("ws");
    let run = FakeRunner::new(&["git"]);
    run.answer("rev-parse --show-toplevel", &format!("{}\n", d.display())).answer("symbolic-ref --short HEAD", "main\n").answer("remote get-url origin", "https://git.example.org/octo/app.git\n").answer("rev-list --count HEAD", "3\n").answer("status --porcelain", " M a\n?? b\n");
    let s = ws_git(&run, &d);
    assert_eq!(s, WsGit { is_repo: true, branch: "main".into(), origin: Some("https://git.example.org/octo/app.git".into()), commits: 3, changed: 2 });
    // inside another repository is not "a repository of its own"
    let run2 = FakeRunner::new(&["git"]);
    run2.answer("rev-parse --show-toplevel", "/somewhere/else\n");
    assert!(!ws_git(&run2, &d).is_repo);
    // set origin: add, same is a no-op, other needs replace, a credential is refused
    let run3 = FakeRunner::new(&["git"]);
    run3.fail("remote get-url origin", 2, "error: No such remote").answer("remote add origin", "");
    set_origin(&run3, &d, "https://git.example.org/octo/app.git", false).unwrap();
    assert!(run3.lines().iter().any(|l| l.ends_with("remote add origin https://git.example.org/octo/app.git")));
    assert!(set_origin(&run3, &d, "https://octo:pw@git.example.org/octo/app.git", false).is_err());
    set_origin(&run, &d, "https://git.example.org/octo/app.git", false).unwrap();
    assert!(set_origin(&run, &d, "https://git.example.org/octo/other.git", false).unwrap_err().contains("already has an origin"));
    run.answer("remote set-url origin", "");
    set_origin(&run, &d, "https://git.example.org/octo/other.git", true).unwrap();
    assert!(run.lines().iter().any(|l| l.ends_with("remote set-url origin https://git.example.org/octo/other.git")));
    // remove, identity (only where unset) and the first commit
    run.answer("remote remove origin", "");
    remove_origin(&run, &d).unwrap();
    let c = Connection { identity_name: "Octo".into(), identity_email: "o@example.org".into(), ..conn(Kind::Gitea, "G", "https://x.example") };
    let run4 = FakeRunner::new(&["git"]);
    run4.fail("config --local --get", 1, "").answer("config --local", "").answer("add -A", "").answer("commit -q", "");
    first_commit(&run4, &d, &c).unwrap();
    let l = run4.lines().join("\n");
    assert!(l.contains("config --local user.name Octo") && l.contains("config --local user.email o@example.org") && l.contains("add -A") && l.contains("commit -q -m"), "{l}");
    let run5 = FakeRunner::new(&["git"]);
    run5.answer("config --local --get", "Already\n").answer("add -A", "").fail("commit", 128, "Author identity unknown");
    assert!(first_commit(&run5, &d, &c).unwrap_err().contains("who you are"));
    assert!(!run5.lines().iter().any(|l| l.contains("config --local user.name")), "an identity the person set is kept");
}
