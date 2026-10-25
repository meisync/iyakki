//! Tests of shared folders: the request logic is called directly with an injected peer address and clock; the socket tests use
//! 127.0.0.1 on ephemeral ports only. Files live in short temp folders that are removed afterwards.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::sharedfolder::*;
use crate::sharedsite::{self, Env, Req, Site};

static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = PathBuf::from(format!("/tmp/iyakki-test/rt-share/t{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(std::fs::canonicalize(&p).unwrap())
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _t: Tmp,
    root: PathBuf,
    outside: PathBuf,
    clock: Arc<AtomicI64>,
    log: Arc<Mutex<Vec<String>>>,
    free: Arc<AtomicI64>,
}

fn fixture() -> Fx {
    let t = Tmp::new();
    let root = t.0.join("share");
    let outside = t.0.join("outside");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("a.txt"), "hello world\n").unwrap();
    std::fs::write(root.join("sub/b.md"), "# b").unwrap();
    std::fs::write(root.join(".secret"), "dot").unwrap();
    std::fs::create_dir_all(root.join(".hid")).unwrap();
    std::fs::write(root.join(".hid/x.txt"), "hidden dir").unwrap();
    std::fs::write(root.join("page.html"), "<script>alert(1)</script>").unwrap();
    std::fs::write(root.join("pic.svg"), "<svg onload=alert(1)/>").unwrap();
    std::fs::write(root.join("100%25.txt"), "literal percent").unwrap();
    std::fs::write(outside.join("secret.txt"), "TOP SECRET").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link_out.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link_dir")).unwrap();
    std::os::unix::fs::symlink(root.join("a.txt"), root.join("link_in.txt")).unwrap();
    std::os::unix::fs::symlink(root.join(".hid"), root.join("link_hid")).unwrap();
    Fx { _t: t, root, outside, clock: Arc::new(AtomicI64::new(1_800_000_000)), log: Arc::new(Mutex::new(vec![])), free: Arc::new(AtomicI64::new(1 << 40)) }
}

impl Fx {
    fn cfg(&self) -> ShareCfg {
        ShareCfg { id: "id1".into(), name: "demo".into(), folder: self.root.to_string_lossy().into(), access: Access::Open, bind: "127.0.0.1".into(), max_upload_mb: 1, ..Default::default() }
    }
    fn env(&self) -> Env {
        let (c, l, f) = (self.clock.clone(), self.log.clone(), self.free.clone());
        Env { clock: Arc::new(move || c.load(Ordering::SeqCst)), audit: Arc::new(move |e, d| l.lock().unwrap().push(format!("{e} {d}"))), deny: vec![], free_space: Arc::new(move |_| Some(f.load(Ordering::SeqCst) as u64)) }
    }
    fn site(&self, f: impl FnOnce(&mut ShareCfg)) -> Site {
        let mut c = self.cfg();
        f(&mut c);
        Site::new(c, self.env()).unwrap()
    }
}

struct Resp {
    status: u16,
    head: String,
    body: Vec<u8>,
}
impl Resp {
    fn h(&self, k: &str) -> Option<String> {
        self.head.lines().find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string()))
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

const LAN: &str = "192.168.7.20";

fn run(site: &Site, req: Req, peer: &str) -> Resp {
    let head_only = req.method == "HEAD";
    let r = site.respond(&req.with("host", "127.0.0.1:1"), peer.parse::<IpAddr>().unwrap());
    let raw = r.to_vec(head_only);
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..at]).into_owned();
    Resp { status: r.status, head, body: raw[at + 4..].to_vec() }
}
fn get(site: &Site, path: &str) -> Resp {
    run(site, Req::get(path), LAN)
}

// ------------------------------------------------------------------ paths

#[test]
fn listing_and_download_work() {
    let f = fixture();
    let s = f.site(|_| {});
    let r = get(&s, "/");
    assert_eq!(r.status, 200);
    let t = r.text();
    assert!(t.contains("a.txt") && t.contains("sub/") && t.contains("11 B") || t.contains("12 B"), "{t}");
    assert!(t.find("sub/").unwrap() < t.find("a.txt").unwrap(), "folders first");
    let d = get(&s, "/a.txt");
    assert_eq!((d.status, d.text().as_str()), (200, "hello world\n"));
    assert_eq!(d.h("content-type").unwrap(), "text/plain; charset=utf-8");
    assert_eq!(get(&s, "/sub").status, 301);
    assert_eq!(get(&s, "/sub").h("location").unwrap(), "/sub/");
    assert!(get(&s, "/sub/").text().contains("b.md"));
    assert_eq!(get(&s, "/link_in.txt").text(), "hello world\n", "a link that stays inside is fine");
}

#[test]
fn path_traversal_is_refused_in_every_disguise() {
    let f = fixture();
    let s = f.site(|_| {});
    for p in ["/../outside/secret.txt", "/sub/../../outside/secret.txt", "/%2e%2e/outside/secret.txt", "/%2E%2E%2foutside/secret.txt", "/sub/%2e%2e/%2e%2e/outside/secret.txt", "/..%5coutside", "/sub%5c..%5c..%5coutside", "/a.txt%00.png", "/%00", "/..%00/", "/%2e%2e", "/....//....//etc/passwd"] {
        let r = get(&s, p);
        assert!(matches!(r.status, 400 | 404), "{p} -> {}", r.status);
        assert!(!r.text().contains("TOP SECRET") && !r.text().contains("root:"), "{p}");
    }
    // double encoding stays a literal name: it is not "..", and no file has that name
    assert_eq!(get(&s, "/%252e%252e/outside/secret.txt").status, 404);
    // ... and a file really named with a percent sign is reachable only by its own name
    assert_eq!(get(&s, "/100%2525.txt").text(), "literal percent");
    // a raw backslash or NUL in the request line is refused by the parser
    assert!(matches!(sharedsite::parse_head(b"GET /a\\b HTTP/1.1\r\n\r\n"), sharedsite::Head::Bad(400, _)));
    assert!(matches!(sharedsite::parse_head(b"GET /a\0b HTTP/1.1\r\n\r\n"), sharedsite::Head::Bad(400, _)));
    // overlong
    let long = format!("/{}", "a".repeat(5000));
    assert_eq!(get(&s, &long).status, 400);
    let deep = "/x".repeat(100);
    assert_eq!(get(&s, &deep).status, 400);
    let longname = format!("/{}", "n".repeat(300));
    assert_eq!(get(&s, &longname).status, 400);
}

#[test]
fn symlinks_pointing_out_are_not_followed_or_listed() {
    let f = fixture();
    let s = f.site(|c| c.show_hidden = true);
    assert_eq!(get(&s, "/link_out.txt").status, 404);
    assert_eq!(get(&s, "/link_dir/secret.txt").status, 404);
    assert_eq!(get(&s, "/link_dir/").status, 404);
    let t = get(&s, "/").text();
    assert!(!t.contains("link_out") && !t.contains("link_dir"), "{t}");
    assert!(t.contains("link_in.txt"));
    // a symlink swapped in after the share started is caught too
    std::fs::remove_file(f.root.join("sub/b.md")).unwrap();
    std::os::unix::fs::symlink(f.outside.join("secret.txt"), f.root.join("sub/b.md")).unwrap();
    assert_eq!(get(&s, "/sub/b.md").status, 404);
}

#[test]
fn dotfiles_are_hidden_unless_switched_on() {
    let f = fixture();
    let s = f.site(|_| {});
    assert_eq!(get(&s, "/.secret").status, 404);
    assert_eq!(get(&s, "/.hid/x.txt").status, 404);
    assert_eq!(get(&s, "/link_hid/x.txt").status, 404, "a link into a hidden folder is hidden too");
    assert!(!get(&s, "/").text().contains(".secret"));
    let s2 = f.site(|c| c.show_hidden = true);
    assert_eq!(get(&s2, "/.secret").text(), "dot");
    assert!(get(&s2, "/").text().contains(".secret"));
    // a website never shows dotfiles, whatever the switch says
    let s3 = f.site(|c| {
        c.mode = Mode::Website;
        c.show_hidden = true;
    });
    assert_eq!(get(&s3, "/.secret").status, 404);
}

#[test]
fn the_deny_list_is_never_served() {
    let f = fixture();
    std::fs::create_dir_all(f.root.join("vault")).unwrap();
    std::fs::write(f.root.join("vault/system.afv"), "keys").unwrap();
    let mut env = f.env();
    env.deny = vec![f.root.join("vault")];
    let s = Site::new(f.cfg(), env).unwrap();
    assert_eq!(get(&s, "/vault/system.afv").status, 404);
    assert!(!get(&s, "/").text().contains("vault"));
}

// ------------------------------------------------------------------ http features

#[test]
fn range_requests() {
    let f = fixture();
    let s = f.site(|_| {});
    let r = run(&s, Req::get("/a.txt").with("range", "bytes=0-4"), LAN);
    assert_eq!((r.status, r.text().as_str()), (206, "hello"));
    assert_eq!(r.h("content-range").unwrap(), "bytes 0-4/12");
    assert_eq!(r.h("accept-ranges").unwrap(), "bytes");
    let r = run(&s, Req::get("/a.txt").with("range", "bytes=-6"), LAN);
    assert_eq!((r.status, r.text().as_str()), (206, "world\n"), "suffix");
    let r = run(&s, Req::get("/a.txt").with("range", "bytes=6-"), LAN);
    assert_eq!(r.text(), "world\n");
    let r = run(&s, Req::get("/a.txt").with("range", "bytes=0-9999"), LAN);
    assert_eq!((r.status, r.body.len()), (206, 12), "an end past the file is cut");
    for bad in ["bytes=50-60", "bytes=9-3", "bytes=-0", "bytes=a-b"] {
        let r = run(&s, Req::get("/a.txt").with("range", bad), LAN);
        assert_eq!(r.status, 416, "{bad}");
        assert_eq!(r.h("content-range").unwrap(), "bytes */12");
    }
    // several ranges, or a unit that is not bytes: the whole file
    assert_eq!(run(&s, Req::get("/a.txt").with("range", "bytes=0-1,3-4"), LAN).status, 200);
    assert_eq!(run(&s, Req::get("/a.txt").with("range", "items=0-1"), LAN).status, 200);
    // If-Range with another validator gives the whole file
    assert_eq!(run(&s, Req::get("/a.txt").with("range", "bytes=0-1").with("if-range", "\"nope\""), LAN).status, 200);
}

#[test]
fn head_etag_and_not_modified() {
    let f = fixture();
    let s = f.site(|_| {});
    let h = run(&s, Req::get("/a.txt").method("HEAD"), LAN);
    assert_eq!((h.status, h.body.len()), (200, 0));
    assert_eq!(h.h("content-length").unwrap(), "12");
    let (etag, lm) = (h.h("etag").unwrap(), h.h("last-modified").unwrap());
    assert!(lm.ends_with(" GMT") && etag.starts_with('"'));
    let r = run(&s, Req::get("/a.txt").with("if-none-match", &etag), LAN);
    assert_eq!((r.status, r.body.len()), (304, 0));
    assert_eq!(run(&s, Req::get("/a.txt").with("if-none-match", &format!("W/{etag}, \"x\"")), LAN).status, 304);
    assert_eq!(run(&s, Req::get("/a.txt").with("if-modified-since", &lm), LAN).status, 304);
    assert_eq!(run(&s, Req::get("/a.txt").with("if-none-match", "\"other\""), LAN).status, 200);
    assert_eq!(http_date_check(), "Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(sharedsite::http_date(1_800_000_000), "Fri, 15 Jan 2027 08:00:00 GMT");
}
fn http_date_check() -> String {
    sharedsite::http_date(0)
}

#[test]
fn mime_table() {
    for (n, m) in [("a.html", "text/html; charset=utf-8"), ("a.PNG", "image/png"), ("x.mp4", "video/mp4"), ("x.webm", "video/webm"), ("x.mp3", "audio/mpeg"), ("a.pdf", "application/pdf"), ("a.js", "text/javascript; charset=utf-8"), ("a.json", "application/json; charset=utf-8"), ("a.wasm", "application/wasm"), ("noext", "application/octet-stream"), ("a.weird", "application/octet-stream"), ("x.woff2", "font/woff2")] {
        assert_eq!(sharedsite::mime_for(n), m, "{n}");
    }
    assert!(sharedsite::is_active_content("a.HTML") && sharedsite::is_active_content("x.svg") && !sharedsite::is_active_content("a.png"));
}

#[test]
fn methods_and_headers_of_a_listing_share() {
    let f = fixture();
    let s = f.site(|_| {});
    let r = get(&s, "/");
    assert_eq!(r.h("x-content-type-options").unwrap(), "nosniff");
    let csp = r.h("content-security-policy").unwrap();
    assert!(csp.contains("default-src 'none'") && !csp.contains("script-src") && csp.contains("frame-ancestors 'none'"));
    assert!(r.h("set-cookie").is_none());
    assert_eq!(run(&s, Req::get("/").method("DELETE"), LAN).status, 405);
    assert_eq!(run(&s, Req::get("/").method("PUT").body(b"x"), LAN).status, 405, "uploads are off");
    // page-like files are downloads, never shown, in a listing share
    for n in ["/page.html", "/pic.svg"] {
        let r = get(&s, n);
        assert_eq!(r.status, 200);
        assert!(r.h("content-disposition").unwrap().starts_with("attachment"), "{n}");
        assert_eq!(r.h("content-type").unwrap(), "application/octet-stream");
        assert_eq!(r.h("x-content-type-options").unwrap(), "nosniff");
        assert!(r.h("content-security-policy").unwrap().contains("sandbox"));
    }
    assert!(get(&s, "/a.txt").h("content-disposition").is_none());
}

#[test]
fn website_mode_serves_index_and_runs_on_its_own_origin() {
    let f = fixture();
    std::fs::write(f.root.join("index.html"), "<h1>site</h1><script src=app.js></script>").unwrap();
    std::fs::write(f.root.join("sub/index.html"), "sub index").unwrap();
    std::fs::create_dir_all(f.root.join("empty")).unwrap();
    let s = f.site(|c| c.mode = Mode::Website);
    let r = get(&s, "/");
    assert_eq!(r.status, 200);
    assert!(r.text().contains("<h1>site</h1>"));
    assert_eq!(r.h("content-type").unwrap(), "text/html; charset=utf-8");
    assert!(r.h("content-disposition").is_none() && r.h("content-security-policy").is_none(), "scripts run in a website");
    assert_eq!(r.h("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(get(&s, "/sub/").text(), "sub index");
    assert_eq!(get(&s, "/empty/").status, 404, "no listing in a website");
    assert_eq!(run(&s, Req::get("/new.txt").method("PUT").body(b"x"), LAN).status, 405);
}

#[test]
fn hostile_file_names_are_escaped_in_the_listing() {
    let f = fixture();
    for n in ["<img src=x onerror=alert(1)>.txt", "q\"uote'.txt", "a&b.txt", "\u{202e}rtl\u{202c}.txt", "ünï-日本.txt", "tab\tname.txt"] {
        std::fs::write(f.root.join(n), "x").unwrap();
    }
    let s = f.site(|_| {});
    let t = get(&s, "/").text();
    assert!(!t.contains("<img"), "{t}");
    assert!(t.contains("&lt;img src=x onerror=alert(1)&gt;") && t.contains("q&quot;uote&#39;.txt") && t.contains("a&amp;b.txt") && t.contains("ünï-日本.txt"));
    assert!(t.contains("<bdi>"), "names are isolated for mixed direction");
    // the link of such a name works
    let link = format!("/{}", sharedsite::pct_encode("<img src=x onerror=alert(1)>.txt"));
    assert_eq!(get(&s, &link).status, 200);
    assert!(t.contains(&sharedsite::pct_encode("<img src=x onerror=alert(1)>.txt")));
    assert_eq!(sharedsite::esc("<>&\"'"), "&lt;&gt;&amp;&quot;&#39;");
}

// ------------------------------------------------------------------ upload

fn multipart_body(files: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let b = "XBOUNDARYX";
    let mut v = Vec::new();
    for (n, d) in files {
        v.extend_from_slice(format!("--{b}\r\nContent-Disposition: form-data; name=\"f\"; filename=\"{n}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        v.extend_from_slice(d);
        v.extend_from_slice(b"\r\n");
    }
    v.extend_from_slice(format!("--{b}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={b}"), v)
}

#[test]
fn uploads_obey_the_switches_limits_and_never_overwrite() {
    let f = fixture();
    let s = f.site(|c| c.upload = true);
    // PUT
    let r = run(&s, Req::get("/sub/new.txt").method("PUT").body(b"fresh"), LAN);
    assert_eq!(r.status, 201);
    assert_eq!(std::fs::read(f.root.join("sub/new.txt")).unwrap(), b"fresh");
    // never overwrites without the flag
    let r = run(&s, Req::get("/sub/new.txt").method("PUT").body(b"other"), LAN);
    assert_eq!(r.status, 409);
    assert_eq!(std::fs::read(f.root.join("sub/new.txt")).unwrap(), b"fresh");
    // too big (limit is 1 MB)
    let big = vec![0u8; 1024 * 1024 + 1];
    assert_eq!(run(&s, Req::get("/big.bin").method("PUT").body(&big), LAN).status, 413);
    assert!(!f.root.join("big.bin").exists());
    // bad names and places
    for p in ["/.dot", "/..%2fx", "/sub/%2e%2e/%2e%2e/outside/x", "/nodir/x.txt", "/link_dir/x.txt", "/link_out.txt"] {
        let r = run(&s, Req::get(p).method("PUT").body(b"x"), LAN);
        assert!(matches!(r.status, 400 | 404 | 409), "{p} -> {}", r.status);
    }
    assert!(!f.outside.join("x.txt").exists() && !f.outside.join("x").exists());
    assert_eq!(std::fs::read(f.outside.join("secret.txt")).unwrap(), b"TOP SECRET", "a link out is not written through");
    // multipart, several files; a name with a path keeps only its last part
    let (ct, body) = multipart_body(&[("one.txt", b"1"), ("../../evil.txt", b"2")]);
    let r = run(&s, Req::get("/").method("POST").with("content-type", &ct).body(&body), LAN);
    assert_eq!(r.status, 303);
    assert_eq!(std::fs::read(f.root.join("one.txt")).unwrap(), b"1");
    assert_eq!(std::fs::read(f.root.join("evil.txt")).unwrap(), b"2");
    assert!(!f.root.parent().unwrap().join("evil.txt").exists());
    let (ct, body) = multipart_body(&[("one.txt", b"again")]);
    assert_eq!(run(&s, Req::get("/").method("POST").with("content-type", &ct).body(&body), LAN).status, 409);
    // folders need their own switch
    let r = run(&s, Req::get("/").method("POST").with("content-type", "application/x-www-form-urlencoded").body(b"mkdir=docs"), LAN);
    assert_eq!(r.status, 405);
    let s2 = f.site(|c| {
        c.upload = true;
        c.mkdir = true;
        c.overwrite = true;
    });
    let r = run(&s2, Req::get("/").method("POST").with("content-type", "application/x-www-form-urlencoded").body(b"mkdir=docs+2"), LAN);
    assert_eq!(r.status, 303);
    assert!(f.root.join("docs 2").is_dir());
    for bad in ["mkdir=..", "mkdir=.x", "mkdir=a%2Fb", "mkdir=", "mkdir=docs+2"] {
        let r = run(&s2, Req::get("/").method("POST").with("content-type", "application/x-www-form-urlencoded").body(bad.as_bytes()), LAN);
        assert!(matches!(r.status, 400 | 409), "{bad} {}", r.status);
    }
    // overwrite on: replaced
    let (ct, body) = multipart_body(&[("one.txt", b"replaced")]);
    assert_eq!(run(&s2, Req::get("/").method("POST").with("content-type", &ct).body(&body), LAN).status, 303);
    assert_eq!(std::fs::read(f.root.join("one.txt")).unwrap(), b"replaced");
    // the listing offers the forms only when switched on
    assert!(get(&s2, "/").text().contains("type=file") && !get(&s, "/").text().contains("name=mkdir"));
    assert!(!f.site(|_| {}).respond(&Req::get("/"), LAN.parse().unwrap()).to_vec(false).windows(9).any(|w| w == b"type=file"));
    let log = f.log.lock().unwrap().join("\n");
    assert!(log.contains("PUT /sub/new.txt -> 201"), "{log}");
}

#[test]
fn uploads_check_disk_space() {
    let f = fixture();
    let s = f.site(|c| c.upload = true);
    f.free.store(1024, Ordering::SeqCst);
    assert_eq!(run(&s, Req::get("/x.txt").method("PUT").body(b"data"), LAN).status, 507, "the reserve is kept free");
    f.free.store((sharedsite::DISK_RESERVE + 3) as i64, Ordering::SeqCst);
    assert_eq!(run(&s, Req::get("/x.txt").method("PUT").body(b"data"), LAN).status, 507);
    f.free.store((sharedsite::DISK_RESERVE + 4) as i64, Ordering::SeqCst);
    assert_eq!(run(&s, Req::get("/x.txt").method("PUT").body(b"data"), LAN).status, 201);
}

// ------------------------------------------------------------------ access modes

#[test]
fn token_mode_checks_the_secret_and_a_new_token_revokes() {
    let f = fixture();
    let tok = random_hex(16);
    assert_eq!(tok.len(), 32);
    let s = f.site(|c| {
        c.access = Access::Token;
        c.token = tok.clone();
    });
    assert_eq!(get(&s, "/").status, 404, "no secret");
    assert_eq!(get(&s, "/a.txt").status, 404);
    assert_eq!(get(&s, &format!("/s/{}/", "0".repeat(32))).status, 404);
    assert_eq!(get(&s, &format!("/s/{}", &tok[..31])).status, 404);
    let r = get(&s, &format!("/s/{tok}/"));
    assert_eq!(r.status, 200);
    assert!(r.text().contains(&format!("href=\"/s/{tok}/a.txt\"")), "links keep the secret prefix");
    assert_eq!(get(&s, &format!("/s/{tok}/a.txt")).text(), "hello world\n");
    assert_eq!(get(&s, &format!("/s/{tok}/sub")).h("location").unwrap(), format!("/s/{tok}/sub/"));
    assert_eq!(get(&s, &format!("/s/{tok}/../outside/secret.txt")).status, 400);
    // the secret is not written to the log
    let log = f.log.lock().unwrap().join("\n");
    assert!(!log.contains(&tok), "{log}");
    assert!(log.contains("share=demo ip=192.168.7.20 GET /a.txt -> 200"), "{log}");
    // guessing is slowed down: after 10 wrong tries the address is told to wait
    for _ in 0..10 {
        get(&s, "/s/wrong/");
    }
    assert_eq!(get(&s, &format!("/s/{tok}/")).status, 429);
    f.clock.fetch_add(61, Ordering::SeqCst);
    assert_eq!(get(&s, &format!("/s/{tok}/")).status, 200);
}

#[test]
fn password_mode_and_nothing_secret_in_logs() {
    let f = fixture();
    let hash = hash_password("correct horse").unwrap();
    assert!(hash.starts_with("$argon2id$") && !hash.contains("correct horse"));
    assert!(verify_password("correct horse", &hash) && !verify_password("wrong", &hash));
    let s = f.site(|c| {
        c.access = Access::Password;
        c.pass_hash = hash.clone();
    });
    let basic = |u: &str, p: &str| {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let d = format!("{u}:{p}").into_bytes();
        let mut o = String::from("Basic ");
        for c in d.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    o.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    o.push('=');
                }
            }
        }
        o
    };
    let r = get(&s, "/");
    assert_eq!(r.status, 401);
    assert!(r.h("www-authenticate").unwrap().starts_with("Basic"));
    assert_eq!(run(&s, Req::get("/").with("authorization", &basic("x", "wrong")), LAN).status, 401);
    assert_eq!(run(&s, Req::get("/").with("authorization", "Basic !!!"), LAN).status, 401);
    let ok = run(&s, Req::get("/a.txt").with("authorization", &basic("anyone", "correct horse")), LAN);
    assert_eq!((ok.status, ok.text().as_str()), (200, "hello world\n"));
    let log = f.log.lock().unwrap().join("\n");
    assert!(!log.contains("correct horse") && !log.contains("wrong") && !log.contains("Basic"), "{log}");
    assert!(log.contains("401"));
}

#[test]
fn open_mode_is_for_the_local_network_only() {
    let f = fixture();
    let s = f.site(|_| {});
    for ok in ["192.168.1.5", "10.0.0.8", "172.16.4.4", "169.254.3.3", "127.0.0.1", "100.64.1.2", "fe80::1", "fd12::5", "::1", "::ffff:192.168.1.9"] {
        assert_eq!(run(&s, Req::get("/a.txt"), ok).status, 200, "{ok}");
    }
    for bad in ["8.8.8.8", "203.0.113.9", "2001:db8::1", "::ffff:8.8.8.8", "172.32.0.1"] {
        assert_eq!(run(&s, Req::get("/a.txt"), bad).status, 403, "{bad}");
    }
}

#[test]
fn this_machine_only_refuses_other_addresses_and_foreign_host_names() {
    let f = fixture();
    let s = f.site(|c| c.access = Access::Local);
    assert_eq!(run(&s, Req::get("/"), LAN).status, 403);
    assert_eq!(run(&s, Req::get("/"), "10.1.1.1").status, 403);
    assert_eq!(run(&s, Req::get("/"), "127.0.0.1").status, 200);
    assert_eq!(run(&s, Req::get("/"), "::1").status, 200);
    // a web page on another site cannot reach it through a name that points here (DNS rebinding)
    let r = s.respond(&Req::get("/").with("host", "evil.example:8081"), "127.0.0.1".parse().unwrap());
    assert_eq!(r.status, 403);
    let r = s.respond(&Req::get("/").with("host", "localhost:8081"), "127.0.0.1".parse().unwrap());
    assert_eq!(r.status, 200);
    assert_eq!(sharedsite::bind_addr(&ShareCfg { access: Access::Local, bind: "all".into(), port: 9000, ..Default::default() }).unwrap().ip().to_string(), "127.0.0.1");
}

#[test]
fn rate_limit_per_client_and_expiry_with_a_fake_clock() {
    let f = fixture();
    let s = f.site(|c| c.expires_at = 1_800_000_100);
    for _ in 0..300 {
        assert_eq!(get(&s, "/a.txt").status, 200);
    }
    assert_eq!(get(&s, "/a.txt").status, 429);
    assert_eq!(run(&s, Req::get("/a.txt"), "192.168.7.21").status, 200, "another client is not slowed down");
    f.clock.fetch_add(11, Ordering::SeqCst);
    assert_eq!(get(&s, "/a.txt").status, 200);
    f.clock.store(1_800_000_100, Ordering::SeqCst);
    assert_eq!(get(&s, "/a.txt").status, 410, "the share has ended");
}

// ------------------------------------------------------------------ sockets (loopback only)

fn talk(addr: std::net::SocketAddr, req: &[u8]) -> String {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    c.write_all(req).unwrap();
    let mut out = Vec::new();
    let _ = c.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn local_cfg(f: &Fx) -> ShareCfg {
    let mut c = f.cfg();
    c.access = Access::Local;
    c.port = 0;
    c
}

#[test]
fn a_real_server_answers_on_loopback_and_stops_cleanly() {
    let f = fixture();
    let mut c = local_cfg(&f);
    c.upload = true;
    let mut run = sharedsite::start(c, f.env()).unwrap();
    assert!(run.addr.ip().is_loopback());
    let r = talk(run.addr, b"GET /a.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 200 OK") && r.ends_with("hello world\n"), "{r}");
    let r = talk(run.addr, b"GET /a.txt HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=6-\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 206") && r.ends_with("world\n"));
    let r = talk(run.addr, b"HEAD /a.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(r.contains("Content-Length: 12") && r.ends_with("\r\n\r\n"));
    // an upload through the socket
    let r = talk(run.addr, b"PUT /up.txt HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 5\r\n\r\nhello");
    assert!(r.starts_with("HTTP/1.1 201"), "{r}");
    assert_eq!(std::fs::read(f.root.join("up.txt")).unwrap(), b"hello");
    let r = talk(run.addr, b"PUT /up2.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 411"));
    let r = talk(run.addr, b"PUT /up3.txt HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 99999999\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 413"));
    assert_eq!(run.site.clients_served(), 1);
    let addr = run.addr;
    run.stop();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(), "the port is closed");
}

#[test]
fn header_size_and_slow_clients_are_limited() {
    let f = fixture();
    let run = sharedsite::start(local_cfg(&f), f.env()).unwrap();
    let big = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nX: {}\r\n\r\n", "a".repeat(20 * 1024));
    let r = talk(run.addr, big.as_bytes());
    assert!(r.starts_with("HTTP/1.1 431"), "{}", &r[..r.len().min(40)]);
    assert!(talk(run.addr, b"GARBAGE\r\n\r\n").starts_with("HTTP/1.1 400"));
    assert!(talk(run.addr, b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").starts_with("HTTP/1.1 501"));
    // slow loris: a client that never finishes its headers is cut off with 408 after 15 seconds
    let t0 = std::time::Instant::now();
    let mut c = TcpStream::connect(run.addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    c.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n").unwrap();
    let mut out = String::new();
    let _ = c.read_to_string(&mut out);
    let took = t0.elapsed();
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");
    assert!(took >= Duration::from_secs(14) && took < Duration::from_secs(20), "{took:?}");
}

#[test]
fn connections_are_capped() {
    let f = fixture();
    let run = sharedsite::start(local_cfg(&f), f.env()).unwrap();
    let mut held = Vec::new();
    for _ in 0..sharedsite::MAX_CONNECTIONS {
        let mut c = TcpStream::connect(run.addr).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\n").unwrap(); // never finished
        held.push(c);
    }
    std::thread::sleep(Duration::from_millis(300));
    let r = talk(run.addr, b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    drop(held);
}

// ------------------------------------------------------------------ the manager

fn manager(f: &Fx, allow: bool) -> Shares {
    let a: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |_| allow);
    let lan: Arc<dyn Fn() -> Vec<String> + Send + Sync> = Arc::new(|| vec!["192.168.7.20".into(), "10.9.9.9".into()]);
    Shares::open(&f.root.parent().unwrap().join("cfg/shares.json"), "alice", f.env(), a, lan)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn draft(f: &Fx) -> Draft {
    Draft { port: free_port(), name: "Photos".into(), folder: f.root.to_string_lossy().into(), access: Access::Local, expiry_secs: 3600, max_upload_mb: 5, ..Default::default() }
}

#[test]
fn forbidden_and_foreign_folders_are_refused() {
    let f = fixture();
    let m = manager(&f, true);
    for p in ["/", "/etc", "/etc/ssh", "/proc", "/sys", "/dev", "/boot", "/home", "/root", "relative/dir", "", "/does/not/exist", "/etc/passwd"] {
        let mut d = draft(&f);
        d.folder = p.into();
        assert!(m.create(&d).is_err(), "{p}");
    }
    // a file is not a folder
    let mut d = draft(&f);
    d.folder = f.root.join("a.txt").to_string_lossy().into();
    assert!(m.create(&d).is_err());
    // not writable by the person (another user's home, a read-only scope)
    let m2 = manager(&f, false);
    assert!(m2.create(&draft(&f)).unwrap_err().contains("write"));
    // the data folder of the system
    let mut env = f.env();
    env.deny = vec![f.root.join("sub")];
    let a: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| true);
    let m3 = Shares::open(&f.root.parent().unwrap().join("c3/shares.json"), "alice", env, a, Arc::new(Vec::new));
    let mut d = draft(&f);
    d.folder = f.root.join("sub").to_string_lossy().into();
    assert!(m3.create(&d).unwrap_err().contains("private"));
    // a folder reached through a symlink is judged by where it really is
    let mut d = draft(&f);
    d.folder = "/tmp/iyakki-test/rt-share/../../../etc".into();
    assert!(m.create(&d).is_err());
}

#[test]
fn creating_a_share_is_safe_by_default() {
    let f = fixture();
    let m = manager(&f, true);
    let mut d = draft(&f);
    d.access = Access::Open;
    assert!(m.create(&d).unwrap_err().contains("everyone on the network"), "open needs the confirmation");
    d.confirm_open = true;
    let id = m.create(&d).unwrap();
    let c = m.get(&id).unwrap();
    assert!(!c.upload && !c.mkdir && !c.overwrite && !c.show_hidden, "read-only by default");
    assert_eq!(c.port, FIRST_PORT.max(c.port));
    assert_eq!(m.running_count(), 0, "off until started");
    // names, duplicates, passwords, expiry
    let mut e = draft(&f);
    e.name = "bad/name".into();
    assert!(m.create(&e).is_err());
    e.name = "photos".into();
    e.access = Access::Local;
    assert!(m.create(&e).unwrap_err().contains("name"));
    e.name = "pw".into();
    e.access = Access::Password;
    assert!(m.create(&e).is_err(), "a password is needed");
    e.password = "short".into();
    assert!(m.create(&e).is_err());
    e.password = "long enough pw".into();
    e.expiry_secs = 5;
    assert!(m.create(&e).is_err());
    e.expiry_secs = 0;
    let pid = m.create(&e).unwrap();
    let pc = m.get(&pid).unwrap();
    assert!(pc.pass_hash.starts_with("$argon2id$") && pc.token.is_empty());
    let saved = std::fs::read_to_string(m.file()).unwrap();
    assert!(!saved.contains("long enough pw"), "the password itself is never stored");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(m.file()).unwrap().permissions().mode() & 0o777, 0o600);
    // website mode cannot upload
    let mut w = draft(&f);
    w.name = "site".into();
    w.mode = Mode::Website;
    w.upload = true;
    w.show_hidden = true;
    let wid = m.create(&w).unwrap();
    let wc = m.get(&wid).unwrap();
    assert!(!wc.upload && !wc.show_hidden);
    // the list survives a restart of the agent
    let again = manager(&f, true);
    assert_eq!(again.list().len(), 3);
}

#[test]
fn start_stop_expiry_stop_all_and_restore() {
    let f = fixture();
    let m = manager(&f, true);
    let mut d = draft(&f);
    let a = m.create(&d).unwrap();
    d.name = "Second".into();
    d.port = free_port();
    d.keep_running = true;
    let b = m.create(&d).unwrap();
    m.start(&a).unwrap();
    m.start(&b).unwrap();
    let ia = m.list().into_iter().find(|i| i.cfg.id == a).unwrap();
    assert!(ia.running && ia.urls[0].starts_with("http://127.0.0.1:") && ia.left == Some(3600));
    let addr = m.addr_of(&a).unwrap();
    assert!(talk(addr, b"GET /a.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").starts_with("HTTP/1.1 200"));
    // the port stays the same after stop and start
    let port = m.get(&a).unwrap().port;
    m.stop(&a);
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
    m.start(&a).unwrap();
    assert_eq!(m.get(&a).unwrap().port, port);
    // expiry
    f.clock.fetch_add(3601, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(), "the port closes by itself when the time is up");
    m.tick();
    assert_eq!(m.running_count(), 0, "both ended");
    m.start(&a).unwrap();
    m.start(&b).unwrap();
    assert_eq!(m.running_count(), 2);
    m.stop_all();
    assert_eq!(m.running_count(), 0);
    // restore at sign-in starts only the "keep running" share
    m.restore();
    assert_eq!(m.running_count(), 1);
    assert!(m.list().into_iter().find(|i| i.cfg.id == b).unwrap().running);
    m.stop_all();
    // delete
    m.delete(&a).unwrap();
    assert_eq!(m.list().len(), 1);
    // the whole registry stops on sign-out and on "stop all"
    let log = f.log.lock().unwrap().join("\n");
    assert!(log.contains("share.stop_all") && log.contains("share.expired") && log.contains("share.start"));
}

#[test]
fn a_new_token_revokes_the_old_link() {
    let f = fixture();
    let m = manager(&f, true);
    let mut d = draft(&f);
    d.access = Access::Token;
    d.bind = "127.0.0.1".into();
    let id = m.create(&d).unwrap();
    m.start(&id).unwrap();
    let old = m.get(&id).unwrap().token;
    let addr = m.addr_of(&id).unwrap();
    // loopback requests carry a 127.0.0.1 peer, which the open/token modes accept
    assert!(talk(addr, format!("GET /s/{old}/a.txt HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).starts_with("HTTP/1.1 200"));
    m.new_token(&id).unwrap();
    let new = m.get(&id).unwrap().token;
    assert_ne!(old, new);
    let addr = m.addr_of(&id).unwrap();
    assert!(talk(addr, format!("GET /s/{old}/a.txt HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).starts_with("HTTP/1.1 404"));
    assert!(talk(addr, format!("GET /s/{new}/a.txt HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).starts_with("HTTP/1.1 200"));
    let url = &m.list()[0].urls[0];
    assert!(url.contains(&format!("/s/{new}/")));
    m.stop_all();
}

#[test]
fn addresses_shown_for_a_share() {
    let f = fixture();
    let m = manager(&f, true);
    let mut d = draft(&f);
    d.password = "long enough pw".into();
    let id = m.create(&d).unwrap();
    let u = m.list().into_iter().find(|i| i.cfg.id == id).unwrap().urls;
    assert_eq!(u.len(), 1, "a local share shows its one address");
    let mut d2 = draft(&f);
    d2.name = "Wide".into();
    d2.access = Access::Password;
    d2.password = "long enough pw".into();
    d2.bind = "all".into();
    d2.port = 0;
    let id2 = m.create(&d2).unwrap();
    let u = m.list().into_iter().find(|i| i.cfg.id == id2).unwrap().urls;
    assert_eq!(u.len(), 2, "every address of the machine: {u:?}");
    assert!(u.iter().all(|x| x.starts_with("http://") && x.ends_with('/')));
    let mut d3 = draft(&f);
    d3.name = "Bad bind".into();
    d3.access = Access::Password;
    d3.password = "long enough pw".into();
    d3.bind = "203.0.113.5".into();
    d3.port = 0;
    assert!(m.create(&d3).is_err());
    let _ = Path::new("/");
}
