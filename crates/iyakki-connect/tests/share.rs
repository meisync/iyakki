//! `--share DIR`: every way out of the shared folder that was thought of, at the level of the folder (`Share`) and through the page's
//! routes (`/fs/...`), plus the overwrite question, read-only, and a race that swaps a folder for a link while it is being read.

mod common;

use common::*;
use iyakki_connect::serve::{Options, Server};
use iyakki_connect::share::{Share, ShareError, MAX_FILE};
use iyakki_link::rawlink::{Auth, FilesApi};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

struct Dirs {
    base: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// `base/root` is shared; `base/outside` holds a secret that must never be reachable.
fn dirs(tag: &str) -> Dirs {
    let base = std::env::temp_dir().join(format!("iyakki-share-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, outside) = (base.join("root"), base.join("outside"));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("a.txt"), "hello").unwrap();
    std::fs::write(root.join("sub/b.txt"), "inner").unwrap();
    std::fs::write(outside.join("secret.txt"), "SECRET").unwrap();
    symlink(outside.join("secret.txt"), root.join("link_file")).unwrap();
    symlink(&outside, root.join("link_dir")).unwrap();
    symlink(root.join("sub"), root.join("inner_link")).unwrap();
    symlink("../../outside", root.join("sub/up")).unwrap();
    Dirs { base, root, outside }
}

fn refused<T: std::fmt::Debug>(r: Result<T, ShareError>) -> bool {
    matches!(r, Err(ShareError::Refused(_)))
}

#[test]
fn plain_files_and_folders_work() {
    let d = dirs("plain");
    let s = Share::new(&d.root, false).unwrap();
    assert_eq!(s.read("a.txt").unwrap(), b"hello");
    assert_eq!(s.read("sub/b.txt").unwrap(), b"inner");
    let names: Vec<_> = s.list("").unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
    assert!(names.contains(&("a.txt".into(), "file")) && names.contains(&("sub".into(), "dir")), "{names:?}");
    assert!(names.contains(&("link_dir".into(), "link")), "links are shown as links");
    assert_eq!(s.list("sub").unwrap().iter().filter(|e| e.kind == "file").count(), 1);
    assert_eq!(s.read("nothing.txt"), Err(ShareError::NotFound));
    assert_eq!(s.exists("a.txt"), Ok(true));
    assert_eq!(s.exists("sub/none"), Ok(false));
}

#[test]
fn dot_dot_in_any_form_is_refused() {
    let d = dirs("dotdot");
    let s = Share::new(&d.root, true).unwrap();
    for p in ["../outside/secret.txt", "sub/../../outside/secret.txt", "..", "../", "sub/..", "./a.txt", "sub/./b.txt", "a.txt/..", "sub//b.txt", "sub/", "/", "sub/../a.txt", "...//outside"] {
        // "..." and its like are plain names; the rest are refused
        if p.starts_with("...") {
            assert!(matches!(s.read(p), Err(ShareError::Refused(_) | ShareError::NotFound)), "{p}");
            continue;
        }
        assert!(refused(s.read(p)), "read {p:?}");
        assert!(refused(s.list(p)), "list {p:?}");
        assert!(refused(s.write(p, b"x", true)), "write {p:?}");
        assert!(refused(s.exists(p)), "exists {p:?}");
    }
    assert_eq!(std::fs::read(d.outside.join("secret.txt")).unwrap(), b"SECRET");
}

#[test]
fn absolute_paths_are_refused() {
    let d = dirs("abs");
    let s = Share::new(&d.root, true).unwrap();
    let abs_secret = d.outside.join("secret.txt").to_string_lossy().into_owned();
    for p in ["/etc/passwd", "//etc/passwd", &abs_secret, &format!("{}/a.txt", d.root.display()), "/"] {
        assert!(refused(s.read(p)), "read {p:?}");
        assert!(refused(s.list(p)), "list {p:?}");
        assert!(refused(s.write(p, b"x", true)), "write {p:?}");
    }
    assert!(!Path::new("/etc/passwd.iyakki").exists());
}

#[test]
fn encoding_and_character_tricks_do_not_escape() {
    let d = dirs("enc");
    let s = Share::new(&d.root, true).unwrap();
    // characters that are control, NUL, backslash: refused outright
    for p in ["a.txt\0", "..\\outside\\secret.txt", "sub\\..\\a.txt", "a\nb", "a\rb", "\u{7}", "sub/\u{1b}[31m", "..\t"] {
        assert!(refused(s.read(p)), "{p:?}");
        assert!(refused(s.write(p, b"x", true)), "{p:?}");
    }
    // percent signs, full-width dots, a trailing dot or space: only plain file names that do not exist (the page server decodes once;
    // what is left is a name, never a traversal)
    for p in ["%2e%2e/outside/secret.txt", "..%2foutside%2fsecret.txt", "%2e%2e", "\u{ff0e}\u{ff0e}/outside/secret.txt", "\u{2025}/x", "a.txt ", "a.txt.", "A.TXT", "sub/B.TXT"] {
        let r = s.read(p);
        assert!(matches!(r, Err(ShareError::NotFound | ShareError::Refused(_))), "{p:?}: {r:?}");
    }
    // the reserved temporary prefix cannot be named
    assert!(refused(s.read(".iyakki-part-0123")));
    assert!(refused(s.write(".iyakki-part-0123", b"x", true)));
    // very long names and paths
    assert!(refused(s.read(&"a".repeat(300))));
    assert!(refused(s.read(&"a/".repeat(600))));
    // the empty file name for a write
    assert!(refused(s.write("", b"x", true)));
}

#[test]
fn links_are_never_followed_in_or_out() {
    let d = dirs("links");
    let s = Share::new(&d.root, true).unwrap();
    assert!(refused(s.read("link_file")), "a link to a file outside");
    assert!(refused(s.read("link_dir/secret.txt")), "through a link to a folder outside");
    assert!(refused(s.list("link_dir")), "listing a link to a folder outside");
    assert!(refused(s.read("sub/up/outside/secret.txt")), "a link up out of a sub-folder");
    assert!(refused(s.read("inner_link/b.txt")), "even a link that stays inside is not followed");
    assert!(refused(s.write("link_dir/new.txt", b"x", true)), "writing through a link to a folder");
    assert!(refused(s.write("link_file", b"overwritten", true)), "writing onto a link");
    assert!(!d.outside.join("new.txt").exists());
    assert_eq!(std::fs::read(d.outside.join("secret.txt")).unwrap(), b"SECRET", "the file behind the link is untouched");
    assert!(std::fs::symlink_metadata(d.root.join("link_file")).unwrap().file_type().is_symlink(), "and the link is still a link");
}

#[test]
fn only_regular_files_are_read() {
    let d = dirs("kinds");
    let s = Share::new(&d.root, false).unwrap();
    let fifo = std::ffi::CString::new(d.root.join("pipe").to_string_lossy().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(refused(s.read("pipe")), "a named pipe would hang a reader");
    assert!(refused(s.read("sub")), "a folder is not a file");
    assert!(refused(s.list("a.txt")), "a file is not a folder");
}

#[test]
fn read_only_means_nothing_is_written() {
    let d = dirs("ro");
    let s = Share::new(&d.root, false).unwrap();
    assert_eq!(s.write("new.txt", b"x", true), Err(ShareError::ReadOnly));
    assert_eq!(s.write("a.txt", b"x", true), Err(ShareError::ReadOnly));
    assert!(!d.root.join("new.txt").exists());
    assert_eq!(std::fs::read(d.root.join("a.txt")).unwrap(), b"hello");
}

#[test]
fn writing_asks_before_replacing_and_never_leaves_half_files_or_loose_modes() {
    let d = dirs("write");
    let s = Share::new(&d.root, true).unwrap();
    assert_eq!(s.write("new.txt", b"first", false), Ok(()));
    assert_eq!(std::fs::read(d.root.join("new.txt")).unwrap(), b"first");
    assert_eq!(std::fs::metadata(d.root.join("new.txt")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(s.write("new.txt", b"second", false), Err(ShareError::Exists), "the default is no");
    assert_eq!(std::fs::read(d.root.join("new.txt")).unwrap(), b"first", "and nothing changed");
    assert_eq!(s.write("new.txt", b"second", true), Ok(()));
    assert_eq!(std::fs::read(d.root.join("new.txt")).unwrap(), b"second");
    assert_eq!(s.write("sub/c.txt", b"in a folder", false), Ok(()));
    assert_eq!(s.write("nofolder/c.txt", b"x", false), Err(ShareError::NotFound), "folders are not made behind the person's back");
    assert!(refused(s.write("sub", b"x", true)), "a folder is never replaced");
    let left: Vec<_> = std::fs::read_dir(&d.root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(".iyakki-part")).collect();
    assert!(left.is_empty(), "no temporary file is left: {left:?}");
    assert!(s.list("").unwrap().iter().all(|e| !e.name.starts_with(".iyakki-part")));
}

#[test]
fn the_size_limit_is_a_refusal_not_a_cut() {
    let d = dirs("size");
    let s = Share::new(&d.root, true).unwrap();
    assert_eq!(s.write("big.bin", &vec![0u8; MAX_FILE as usize + 1], true), Err(ShareError::TooLarge));
    assert!(!d.root.join("big.bin").exists());
    // a sparse file over the limit is refused from its size, before a byte is read
    let f = std::fs::File::create(d.root.join("sparse")).unwrap();
    f.set_len(MAX_FILE + 1).unwrap();
    assert_eq!(Share::new(&d.root, false).unwrap().read("sparse"), Err(ShareError::TooLarge));
}

#[test]
fn the_root_itself_is_chosen_with_care() {
    let d = dirs("root");
    assert!(Share::new(Path::new("/"), false).is_err(), "not the whole file system");
    assert!(Share::new(&d.root.join("a.txt"), false).is_err(), "a file is not a folder");
    assert!(Share::new(&d.root.join("missing"), false).is_err());
    // a link to a folder is the person's own choice: it is followed once, here
    let via = d.base.join("via");
    symlink(&d.root, &via).unwrap();
    assert_eq!(Share::new(&via, false).unwrap().root(), std::fs::canonicalize(&d.root).unwrap());
}

/// The race: while files are read and written through `swap/`, `swap` is switched between a real folder and a link to the folder
/// with the secret. A path checked and then opened by name would sooner or later follow the link; this one never does.
#[test]
fn swapping_a_folder_for_a_link_while_it_is_used_never_reaches_the_secret() {
    let d = dirs("race");
    std::fs::create_dir_all(d.root.join("real")).unwrap();
    std::fs::write(d.root.join("real/x.txt"), "INSIDE").unwrap();
    std::fs::write(d.outside.join("x.txt"), "SECRET").unwrap();
    symlink(&d.outside, d.root.join("lnk")).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (root, s2) = (d.root.clone(), stop.clone());
    let swapper = std::thread::spawn(move || {
        let mut n = 0u64;
        while !s2.load(std::sync::atomic::Ordering::Relaxed) {
            let (a, b) = if n % 2 == 0 { ("real", "lnk") } else { ("lnk", "real") };
            // swap <- a, then back; the other one takes its turn
            if std::fs::rename(root.join(a), root.join("swap")).is_ok() {
                std::thread::yield_now();
                let _ = std::fs::rename(root.join("swap"), root.join(a));
            }
            let _ = b;
            n += 1;
        }
    });
    let s = Share::new(&d.root, true).unwrap();
    let t0 = std::time::Instant::now();
    let (mut reads_ok, mut refused_n, mut notfound) = (0u64, 0u64, 0u64);
    while t0.elapsed() < std::time::Duration::from_millis(2500) {
        match s.read("swap/x.txt") {
            Ok(b) => {
                assert_eq!(b, b"INSIDE", "a read through the swapped name returned the secret");
                reads_ok += 1;
            }
            Err(ShareError::Refused(_)) => refused_n += 1,
            Err(ShareError::NotFound) => notfound += 1,
            Err(e) => panic!("{e:?}"),
        }
        let _ = s.write("swap/y.txt", b"written", true);
        let _ = s.list("swap");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    swapper.join().unwrap();
    assert!(!d.outside.join("y.txt").exists(), "nothing was written into the outside folder");
    assert_eq!(std::fs::read(d.outside.join("x.txt")).unwrap(), b"SECRET");
    eprintln!("race: {reads_ok} reads inside, {refused_n} refused as a link, {notfound} not found while swapped");
    assert!(reads_ok + refused_n + notfound > 100, "the loop really ran");
}

// ------------------------------------------------------------------------------------------------------------- through the page

fn server(d: &Dirs, r: &Remote, write: bool) -> (Server, String, String) {
    let files = FilesApi::login(&r.target, USER, PASSWORD).unwrap();
    let s = Server::start(Options {
        target: r.target.clone(),
        auth: Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new(PASSWORD.into()) },
        share: Some(Share::new(&d.root, write).unwrap()),
        files: Some(files),
        port: 0,
    })
    .unwrap();
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let (_, head, _) = get(s.port(), &format!("/?t={token}"), None, "");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let cfg: serde_json::Value = serde_json::from_slice(&get(s.port(), "/api/config", None, &format!("Cookie: {cookie}\r\n")).2).unwrap();
    let csrf = cfg["csrf"].as_str().unwrap().to_string();
    (s, cookie, csrf)
}

fn api(s: &Server, cookie: &str, csrf: &str, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    let p = s.port();
    let raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nCookie: {cookie}\r\nOrigin: http://127.0.0.1:{p}\r\nX-Csrf: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let (code, _, b) = http(p, &raw);
    (code, serde_json::from_slice(&b).unwrap_or_default())
}

#[test]
fn the_pages_routes_refuse_every_escape_and_say_no_more_than_needed() {
    let d = dirs("http");
    let r = start_remote();
    let (s, cookie, csrf) = server(&d, &r, true);
    let list = |q: &str| api(&s, &cookie, &csrf, "GET", &format!("/fs/local/list?path={q}"), "");
    assert_eq!(list("").0, 200);
    assert_eq!(list("sub").0, 200);
    for q in ["..", "%2e%2e", "..%2foutside", "%2e%2e%2foutside", "%2e%2e/outside", "sub%2f..%2f..", "/etc", "%2fetc", "%2Fetc%2Fpasswd", "..%5coutside", "a%00b", "%00", "sub%2f", "link_dir", "inner_link", "sub%2fup", "%c0%ae%c0%ae%2foutside"] {
        let (code, body) = list(q);
        assert!(code == 400 || code == 403, "list {q}: {code} {body}");
        assert!(!body.to_string().contains("secret.txt"), "{q}: {body}");
    }
    // sending a file: only what is inside, never through a link; the secret never reaches the tenant
    let send = |local: &str, remote: &str, replace: bool| api(&s, &cookie, &csrf, "POST", "/fs/send", &serde_json::json!({"local": local, "remote": remote, "replace": replace}).to_string());
    for local in ["../outside/secret.txt", "/etc/passwd", "link_file", "link_dir/secret.txt", "%2e%2e/outside/secret.txt", "sub/../../outside/secret.txt"] {
        let (code, body) = send(local, "stolen.txt", true);
        assert!(code == 403 || code == 404, "send {local}: {code} {body}");
    }
    assert!(r.files.lock().unwrap().is_empty(), "nothing reached the tenant: {:?}", r.files.lock().unwrap().keys().collect::<Vec<_>>());
    // the tenant side's paths are checked here too, before any request
    for remote in ["../x", "/abs", "a/../b", "a//b"] {
        let (code, _) = send("a.txt", remote, true);
        assert_eq!(code, 403, "remote {remote}");
    }
    // saving: names that escape are refused, nothing is written outside
    r.files.lock().unwrap().insert("doc.txt".into(), b"from the tenant".to_vec());
    let fetch = |remote: &str, local: &str, replace: bool| api(&s, &cookie, &csrf, "POST", "/fs/fetch", &serde_json::json!({"remote": remote, "local": local, "replace": replace}).to_string());
    for local in ["../outside/doc.txt", "/tmp/doc.txt", "link_dir/doc.txt", "link_file", "sub/up/outside/doc.txt"] {
        let (code, body) = fetch("doc.txt", local, true);
        assert!(code == 403 || code == 404, "fetch to {local}: {code} {body}");
    }
    assert!(!d.outside.join("doc.txt").exists());
    assert_eq!(std::fs::read(d.outside.join("secret.txt")).unwrap(), b"SECRET");
}

#[test]
fn replacing_asks_first_in_both_directions_and_the_default_is_no() {
    let d = dirs("ask");
    let r = start_remote();
    let (s, cookie, csrf) = server(&d, &r, true);
    let send = |replace: bool| api(&s, &cookie, &csrf, "POST", "/fs/send", &serde_json::json!({"local": "a.txt", "remote": "a.txt", "replace": replace}).to_string());
    assert_eq!(send(false).0, 200, "a new name goes");
    assert_eq!(r.files.lock().unwrap()["a.txt"], b"hello");
    std::fs::write(d.root.join("a.txt"), "changed").unwrap();
    let (code, body) = send(false);
    assert_eq!((code, body["exists"].as_bool()), (409, Some(true)), "the tenant already has it: ask");
    assert_eq!(r.files.lock().unwrap()["a.txt"], b"hello", "nothing was replaced");
    assert_eq!(send(true).0, 200, "after a yes");
    assert_eq!(r.files.lock().unwrap()["a.txt"], b"changed");
    // the other direction
    r.files.lock().unwrap().insert("doc.txt".into(), b"v1".to_vec());
    let fetch = |replace: bool| api(&s, &cookie, &csrf, "POST", "/fs/fetch", &serde_json::json!({"remote": "doc.txt", "local": "doc.txt", "replace": replace}).to_string());
    assert_eq!(fetch(false).0, 200);
    assert_eq!(std::fs::read(d.root.join("doc.txt")).unwrap(), b"v1");
    r.files.lock().unwrap().insert("doc.txt".into(), b"v2".to_vec());
    let (code, body) = fetch(false);
    assert_eq!((code, body["exists"].as_bool()), (409, Some(true)));
    assert_eq!(std::fs::read(d.root.join("doc.txt")).unwrap(), b"v1", "kept");
    assert_eq!(fetch(true).0, 200);
    assert_eq!(std::fs::read(d.root.join("doc.txt")).unwrap(), b"v2");
}

#[test]
fn a_read_only_share_can_send_but_never_receive() {
    let d = dirs("ro-http");
    let r = start_remote();
    let (s, cookie, csrf) = server(&d, &r, false);
    let cfg: serde_json::Value = serde_json::from_slice(&get(s.port(), "/api/config", None, &format!("Cookie: {cookie}\r\n")).2).unwrap();
    assert_eq!((cfg["share"].as_bool(), cfg["write"].as_bool()), (Some(true), Some(false)));
    r.files.lock().unwrap().insert("doc.txt".into(), b"v1".to_vec());
    let (code, body) = api(&s, &cookie, &csrf, "POST", "/fs/fetch", r#"{"remote":"doc.txt","local":"doc.txt","replace":true}"#);
    assert_eq!(code, 403, "{body}");
    assert!(body["error"].as_str().unwrap().contains("read-only"));
    assert!(!d.root.join("doc.txt").exists());
    assert_eq!(api(&s, &cookie, &csrf, "POST", "/fs/send", r#"{"local":"a.txt","remote":"sent.txt"}"#).0, 200);
    assert_eq!(r.files.lock().unwrap()["sent.txt"], b"hello");
    let (code, body) = api(&s, &cookie, &csrf, "GET", "/fs/remote/list?path=", "");
    assert_eq!(code, 200, "{body}");
    assert!(body["entries"].as_array().unwrap().iter().any(|e| e["name"] == "sent.txt"));
}

#[test]
fn without_share_the_file_routes_do_not_exist() {
    let r = start_remote();
    let s = Server::start(Options { target: r.target.clone(), auth: Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new(PASSWORD.into()) }, share: None, files: None, port: 0 }).unwrap();
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let (_, head, _) = get(s.port(), &format!("/?t={token}"), None, "");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let cfg: serde_json::Value = serde_json::from_slice(&get(s.port(), "/api/config", None, &format!("Cookie: {cookie}\r\n")).2).unwrap();
    let csrf = cfg["csrf"].as_str().unwrap().to_string();
    assert_eq!(cfg["share"].as_bool(), Some(false));
    assert_eq!(api(&s, &cookie, &csrf, "GET", "/fs/local/list?path=", "").0, 404);
    assert_eq!(api(&s, &cookie, &csrf, "POST", "/fs/send", r#"{"local":"a","remote":"b"}"#).0, 404);
}
