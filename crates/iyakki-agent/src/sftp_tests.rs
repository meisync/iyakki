//! Tests for the SFTP server: the packet codec with hostile input, the scope-confined file layer (path escapes, links), the session, the
//! manager (keys, limits, firewall rules, expiry), the chat tool, and a real `sftp` client over loopback when this computer has one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::Tools;
use crate::sftp::*;
use crate::sftp_fs::*;
use crate::sftp_proto::*;
use crate::sftp_session::Session;

// ------------------------------------------------------------------ packet builders

fn s(x: &str) -> Vec<u8> {
    let mut o = (x.len() as u32).to_be_bytes().to_vec();
    o.extend_from_slice(x.as_bytes());
    o
}
fn b(x: &[u8]) -> Vec<u8> {
    let mut o = (x.len() as u32).to_be_bytes().to_vec();
    o.extend_from_slice(x);
    o
}
fn pk(t: u8, id: u32, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut o = vec![t];
    o.extend_from_slice(&id.to_be_bytes());
    for p in parts {
        o.extend_from_slice(p);
    }
    o
}
fn no_attrs() -> Vec<u8> {
    vec![0, 0, 0, 0]
}
fn u32b(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
fn u64b(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
fn code_of(p: &[u8]) -> u32 {
    assert_eq!(p[0], FXP_STATUS, "{p:?}");
    u32::from_be_bytes(p[5..9].try_into().unwrap())
}
fn handle_of(p: &[u8]) -> Vec<u8> {
    assert_eq!(p[0], FXP_HANDLE, "{p:?}");
    let n = u32::from_be_bytes(p[5..9].try_into().unwrap()) as usize;
    p[9..9 + n].to_vec()
}

// ------------------------------------------------------------------ the codec

#[test]
fn every_request_type_is_parsed() {
    let h = b(&[0, 0, 0, 1]);
    let cases: Vec<(Vec<u8>, Req)> = vec![
        (pk(FXP_OPEN, 1, &[s("/a"), u32b(OPEN_READ), no_attrs()]), Req::Open { id: 1, path: "/a".into(), flags: OPEN_READ, attrs: Attrs::default() }),
        (pk(FXP_CLOSE, 2, &[h.clone()]), Req::Close { id: 2, handle: vec![0, 0, 0, 1] }),
        (pk(FXP_READ, 3, &[h.clone(), u64b(10), u32b(20)]), Req::Read { id: 3, handle: vec![0, 0, 0, 1], offset: 10, len: 20 }),
        (pk(FXP_WRITE, 4, &[h.clone(), u64b(5), b(b"xy")]), Req::Write { id: 4, handle: vec![0, 0, 0, 1], offset: 5, data: b"xy".to_vec() }),
        (pk(FXP_LSTAT, 5, &[s("l")]), Req::Lstat { id: 5, path: "l".into() }),
        (pk(FXP_FSTAT, 6, &[h.clone()]), Req::Fstat { id: 6, handle: vec![0, 0, 0, 1] }),
        (pk(FXP_SETSTAT, 7, &[s("p"), [u32b(4), u32b(0o600)].concat()]), Req::Setstat { id: 7, path: "p".into(), attrs: Attrs { perms: Some(0o600), ..Default::default() } }),
        (pk(FXP_FSETSTAT, 8, &[h.clone(), [u32b(1), u64b(9)].concat()]), Req::Fsetstat { id: 8, handle: vec![0, 0, 0, 1], attrs: Attrs { size: Some(9), ..Default::default() } }),
        (pk(FXP_OPENDIR, 9, &[s("/")]), Req::Opendir { id: 9, path: "/".into() }),
        (pk(FXP_READDIR, 10, &[h.clone()]), Req::Readdir { id: 10, handle: vec![0, 0, 0, 1] }),
        (pk(FXP_REMOVE, 11, &[s("r")]), Req::Remove { id: 11, path: "r".into() }),
        (pk(FXP_MKDIR, 12, &[s("m"), no_attrs()]), Req::Mkdir { id: 12, path: "m".into(), attrs: Attrs::default() }),
        (pk(FXP_RMDIR, 13, &[s("d")]), Req::Rmdir { id: 13, path: "d".into() }),
        (pk(FXP_REALPATH, 14, &[s(".")]), Req::Realpath { id: 14, path: ".".into() }),
        (pk(FXP_STAT, 15, &[s("t")]), Req::Stat { id: 15, path: "t".into() }),
        (pk(FXP_RENAME, 16, &[s("a"), s("b")]), Req::Rename { id: 16, from: "a".into(), to: "b".into() }),
        (pk(FXP_READLINK, 17, &[s("k")]), Req::Readlink { id: 17, path: "k".into() }),
        (pk(FXP_SYMLINK, 18, &[s("a"), s("b")]), Req::Symlink { id: 18 }),
        (pk(FXP_EXTENDED, 19, &[s("x@y")]), Req::Extended { id: 19, name: "x@y".into() }),
    ];
    for (bytes, want) in cases {
        let got = parse_request(&bytes).unwrap();
        assert_eq!(got, want);
        assert!(got.id().is_some());
    }
    assert_eq!(parse_request(&[FXP_INIT, 0, 0, 0, 3]).unwrap(), Req::Init { version: 3 });
}

#[test]
fn every_prefix_of_every_request_is_an_error_not_a_panic() {
    let packets = [
        pk(FXP_OPEN, 1, &[s("/some/path"), u32b(OPEN_WRITE | OPEN_CREAT), [u32b(0x8000_000f), u64b(1), u32b(1), u32b(2), u32b(0o644), u32b(3), u32b(4), u32b(1), s("k"), s("v")].concat()]),
        pk(FXP_WRITE, 4, &[b(&[0, 0, 0, 1]), u64b(5), b(b"hello")]),
        pk(FXP_RENAME, 16, &[s("a"), s("b")]),
    ];
    for p in packets {
        assert!(parse_request(&p).is_ok());
        for n in 0..p.len() {
            assert!(parse_request(&p[..n]).is_err(), "prefix {n} of {}", p.len());
        }
    }
}

#[test]
fn lying_lengths_and_hostile_fields_are_refused() {
    // a string that claims 4 GB, then 3 bytes
    let mut p = vec![FXP_OPENDIR, 0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff];
    p.extend_from_slice(b"abc");
    assert!(matches!(parse_request(&p), Err(ProtoError::TooLong)));
    // a string longer than the rest of the packet
    let p = [vec![FXP_OPENDIR, 0, 0, 0, 1], u32b(50), b"abc".to_vec()].concat();
    assert_eq!(parse_request(&p), Err(ProtoError::Truncated));
    // invalid UTF-8 and NUL in a path
    assert_eq!(parse_request(&pk(FXP_STAT, 1, &[b(&[0xff, 0xfe])])), Err(ProtoError::BadText));
    assert_eq!(parse_request(&pk(FXP_STAT, 1, &[s("a\0b")])), Err(ProtoError::BadText));
    // a path over 4096 bytes
    assert_eq!(parse_request(&pk(FXP_STAT, 1, &[s(&"a".repeat(MAX_PATH + 1))])), Err(ProtoError::TooLong));
    // a handle over 256 bytes
    assert_eq!(parse_request(&pk(FXP_CLOSE, 1, &[b(&[0u8; 300])])), Err(ProtoError::TooLong));
    // a million extended attribute pairs
    let attrs = [u32b(0x8000_0000), u32b(1_000_000)].concat();
    assert_eq!(parse_request(&pk(FXP_MKDIR, 1, &[s("x"), attrs])), Err(ProtoError::TooLong));
    // unknown type
    assert_eq!(parse_request(&pk(77, 1, &[])), Err(ProtoError::UnknownType(77)));
    assert!(parse_request(&[]).is_err());
    // a payload above the maximum is refused whole
    assert_eq!(parse_request(&vec![FXP_WRITE; MAX_PACKET + 1]), Err(ProtoError::TooLong));
}

#[test]
fn frame_lengths_are_checked_before_anything_is_waited_for() {
    assert!(frame_len([0, 0, 0, 0]).is_err());
    assert!(frame_len([0xff, 0xff, 0xff, 0xff]).is_err());
    assert_eq!(frame_len([0, 4, 0, 0]), Ok(MAX_PACKET));
    assert!(frame_len([0, 4, 0, 1]).is_err());
    let mut f = Framer::default();
    f.push(&[0xff, 0xff, 0xff, 0xff]);
    assert_eq!(f.next(), Err(ProtoError::TooLong), "a hostile length fails at once, with 4 bytes buffered");
    // packets that arrive in pieces, and two in one go
    let mut f = Framer::default();
    let a = frame(&[1, 2, 3]);
    let c = frame(&[9; 10]);
    let all = [a.clone(), c.clone()].concat();
    for chunk in all.chunks(3) {
        f.push(chunk);
    }
    assert_eq!(f.next().unwrap().unwrap(), vec![1, 2, 3]);
    assert_eq!(f.next().unwrap().unwrap(), vec![9; 10]);
    assert_eq!(f.next().unwrap(), None);
    assert_eq!(f.buffered(), 0);
}

#[test]
fn answers_have_the_version_3_layout() {
    assert_eq!(version_packet(), vec![FXP_VERSION, 0, 0, 0, 3]);
    let st = status_packet(7, FX_EOF, "End");
    assert_eq!(&st[..9], &[FXP_STATUS, 0, 0, 0, 7, 0, 0, 0, 1]);
    assert_eq!(code_of(&st), FX_EOF);
    let a = attrs_packet(1, &Attrs { size: Some(3), uid_gid: Some((1, 2)), perms: Some(0o100644), times: Some((5, 6)) });
    assert_eq!(a.len(), 1 + 4 + 4 + 8 + 8 + 4 + 8);
    assert_eq!(&a[5..9], &15u32.to_be_bytes());
    let n = name_packet(2, &[("f".into(), "long".into(), Attrs::default())]);
    assert_eq!(n[0], FXP_NAME);
    assert_eq!(&n[5..9], &1u32.to_be_bytes());
    assert_eq!(data_packet(3, b"ab")[0], FXP_DATA);
    assert_eq!(handle_of(&handle_packet(4, &[1, 2])), vec![1, 2]);
}

#[test]
fn the_ls_line_and_dates_are_right() {
    assert_eq!(civil_for_test(0), (1970, 1, 1));
    assert_eq!(civil_for_test(20_000), (2024, 10, 4));
    let l = long_name("f.txt", &Attrs { size: Some(12), uid_gid: Some((1000, 1000)), perms: Some(0o100644), times: Some((0, 86400 * 20_000)) });
    assert!(l.starts_with("-rw-r--r-- 1 1000"), "{l}");
    assert!(l.ends_with("Oct  4 2024 f.txt"), "{l}");
    assert!(long_name("d", &Attrs { perms: Some(0o040755), ..Default::default() }).starts_with("drwxr-xr-x"));
}

// ------------------------------------------------------------------ the confined files

struct World {
    root: PathBuf,
    home: PathBuf,
    shared: PathBuf,
    outside: PathBuf,
}

fn world(tag: &str) -> (World, Jail) {
    let root = std::fs::canonicalize(crate::testdir::unique("sftp", tag)).unwrap();
    let (home, shared, outside) = (root.join("home/amy"), root.join("shared"), root.join("outside"));
    for d in [&home, &shared, &outside, &home.join("sub")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(home.join("a.txt"), "alpha").unwrap();
    std::fs::write(home.join("sub/deep.txt"), "deep").unwrap();
    std::fs::write(shared.join("pub.txt"), "public").unwrap();
    std::fs::write(outside.join("secret.txt"), "TOP SECRET").unwrap();
    std::fs::create_dir_all(outside.join("dir")).unwrap();
    use std::os::unix::fs::symlink;
    symlink(&outside, home.join("link_out")).unwrap();
    symlink(outside.join("secret.txt"), home.join("file_link")).unwrap();
    symlink(home.join("sub"), home.join("link_in")).unwrap();
    symlink(home.join("nowhere"), home.join("dangling")).unwrap();
    symlink("/etc", home.join("link_etc")).unwrap();
    symlink("../../outside", home.join("rel_out")).unwrap();
    let (h, sh) = (home.clone(), shared.clone());
    let read: Rule = Arc::new(move |p: &Path| p.starts_with(&h) || p.starts_with(&sh));
    let h2 = home.clone();
    let write: Rule = Arc::new(move |p: &Path| p.starts_with(&h2));
    let jail = Jail::new(home.clone(), vec![shared.clone()], read, write);
    (World { root, home, shared, outside }, jail)
}

fn names(j: &Jail, v: &str) -> Vec<String> {
    j.list(v).unwrap().into_iter().map(|e| e.0).collect()
}

#[test]
fn paths_cannot_leave_the_tree_in_any_disguise() {
    let (w, j) = world("escape");
    let secret = w.outside.join("secret.txt");
    for v in [
        "/../outside/secret.txt",
        "../../outside/secret.txt",
        "/../../../../../etc/passwd",
        "//etc/passwd",
        "/etc/passwd",
        "etc/passwd",
        &secret.display().to_string(),
        "/link_out/secret.txt",
        "/link_out/../secret.txt",
        "/file_link",
        "/rel_out/secret.txt",
        "/link_etc/passwd",
        "/./sub/../link_out/secret.txt",
        "/sub/../../outside/secret.txt",
        "/a.txt/../../outside/secret.txt",
        "/dangling",
        "\\..\\..\\outside\\secret.txt",
        "/+shared/../outside/secret.txt",
        "/+shared/../../outside/secret.txt",
        "/+nosuch/x",
        "/a\0.txt",
    ] {
        match j.open(v, OPEN_READ, &Attrs::default()) {
            Ok((mut f, virt)) => {
                // allowed only if it stayed inside and is not the secret
                let mut text = String::new();
                std::io::Read::read_to_string(&mut f, &mut text).unwrap();
                assert!(!text.contains("SECRET"), "{v} -> {virt} leaked");
                assert!(!virt.contains(".."), "{virt}");
            }
            Err(e) => assert!(matches!(e, FsErr::NotFound | FsErr::Denied | FsErr::Failure(_)), "{v}: {e:?}"),
        }
    }
    // stat, list and realpath cannot show the outside either
    assert_eq!(j.stat("/link_out", true), Err(FsErr::NotFound));
    assert_eq!(j.stat("/link_out/dir", true), Err(FsErr::NotFound));
    assert!(matches!(j.list("/link_out"), Err(FsErr::NotFound)));
    assert!(matches!(j.list("/../outside"), Err(FsErr::NotFound)));
    // `..` at the top stays at the top, as in a chroot
    assert_eq!(j.realpath("/../../..").unwrap(), "/");
    assert_eq!(j.realpath("/sub/../a.txt").unwrap(), "/a.txt");
    assert_eq!(j.realpath("").unwrap(), "/");
    assert_eq!(j.realpath("/sub/new.txt").unwrap(), "/sub/new.txt", "a name that does not exist yet is fine");
    assert!(j.realpath("/nope/new.txt").is_err());
}

#[test]
fn links_leading_out_are_hidden_and_links_inside_work() {
    let (_w, j) = world("links");
    let n = names(&j, "/");
    for hidden in ["link_out", "file_link", "link_etc", "rel_out", "dangling"] {
        assert!(!n.contains(&hidden.to_string()), "{hidden} must not be listed: {n:?}");
    }
    for shown in ["a.txt", "sub", "link_in", "+shared"] {
        assert!(n.contains(&shown.to_string()), "{shown} in {n:?}");
    }
    assert_eq!(j.stat("/file_link", false), Err(FsErr::NotFound), "even an lstat does not show a link that leads out");
    assert_eq!(j.stat("/dangling", false), Err(FsErr::NotFound));
    assert!(j.stat("/link_in", false).is_ok());
    assert_eq!(names(&j, "/link_in"), vec!["deep.txt".to_string()]);
    let (mut f, v) = j.open("/link_in/deep.txt", OPEN_READ, &Attrs::default()).unwrap();
    assert_eq!(v, "/link_in/deep.txt");
    let mut t = String::new();
    std::io::Read::read_to_string(&mut f, &mut t).unwrap();
    assert_eq!(t, "deep");
}

#[test]
fn nothing_is_created_through_a_link_that_leads_out() {
    let (w, j) = world("create");
    let attrs = Attrs::default();
    for v in ["/link_out/new.txt", "/rel_out/new.txt", "/link_etc/iyakki-test", "/dangling", "/file_link"] {
        let r = j.open(v, OPEN_WRITE | OPEN_CREAT, &attrs);
        assert!(r.is_err(), "{v}");
    }
    assert!(j.mkdir("/link_out/d", &attrs).is_err());
    assert!(!w.outside.join("new.txt").exists());
    assert!(!w.outside.join("d").exists());
    assert!(w.outside.join("secret.txt").exists(), "the target of a link is never touched");
    assert!(!Path::new("/etc/iyakki-test").exists());
}

#[test]
fn removing_a_link_removes_the_link_and_not_its_target() {
    let (w, j) = world("rmlink");
    // a link inside that points inside
    assert_eq!(j.remove("/link_in").unwrap(), "/link_in");
    assert!(w.home.join("sub/deep.txt").exists());
    assert!(!w.home.join("link_in").exists());
    // a link that points outside can be removed by its owner (the link only), but not followed
    let r = j.remove("/file_link");
    assert!(r.is_ok(), "{r:?}");
    assert!(w.outside.join("secret.txt").exists());
}

#[test]
fn writing_needs_the_write_rule_and_the_other_roots_are_read_only() {
    let (w, j) = world("rw");
    let a = Attrs::default();
    // read the shared root
    let (mut f, v) = j.open("/+shared/pub.txt", OPEN_READ, &a).unwrap();
    assert_eq!(v, "/+shared/pub.txt");
    let mut t = String::new();
    std::io::Read::read_to_string(&mut f, &mut t).unwrap();
    assert_eq!(t, "public");
    assert_eq!(names(&j, "/+shared"), vec!["pub.txt".to_string()]);
    // but not change it
    assert_eq!(j.open("/+shared/pub.txt", OPEN_WRITE, &a).err(), Some(FsErr::Denied));
    assert_eq!(j.open("/+shared/new.txt", OPEN_WRITE | OPEN_CREAT, &a).err(), Some(FsErr::Denied));
    assert_eq!(j.mkdir("/+shared/d", &a), Err(FsErr::Denied));
    assert_eq!(j.remove("/+shared/pub.txt"), Err(FsErr::Denied));
    assert_eq!(j.rename("/+shared/pub.txt", "/moved.txt"), Err(FsErr::Denied));
    assert_eq!(j.rename("/a.txt", "/+shared/in.txt"), Err(FsErr::Denied));
    assert_eq!(j.setstat("/+shared/pub.txt", &Attrs { perms: Some(0o777), ..Default::default() }), Err(FsErr::Denied));
    assert!(w.shared.join("pub.txt").exists() && !w.shared.join("new.txt").exists());
    // the roots themselves are never removed, renamed or changed
    for v in ["/", "/+shared", "/."] {
        assert!(j.rmdir(v).is_err() && j.remove(v).is_err(), "{v}");
        assert!(j.rename(v, "/x").is_err());
        assert!(j.setstat(v, &Attrs { perms: Some(0o700), ..Default::default() }).is_err());
    }
    assert!(w.home.exists() && w.shared.exists());
}

#[test]
fn files_and_folders_can_be_made_changed_and_removed_inside() {
    let (w, j) = world("ops");
    let a = Attrs::default();
    {
        let (f, v) = j.open("/new.txt", OPEN_WRITE | OPEN_CREAT | OPEN_EXCL, &Attrs { perms: Some(0o600), ..Default::default() }).unwrap();
        assert_eq!(v, "/new.txt");
        write_at(&f, 0, b"hello").unwrap();
        write_at(&f, 5, b" world").unwrap();
    }
    assert_eq!(std::fs::read_to_string(w.home.join("new.txt")).unwrap(), "hello world");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(w.home.join("new.txt")).unwrap().permissions().mode() & 0o777 & !0o022, 0o600 & !0o022);
    assert_eq!(j.open("/new.txt", OPEN_WRITE | OPEN_CREAT | OPEN_EXCL, &a).err(), Some(FsErr::Exists));
    let (f, _) = j.open("/new.txt", OPEN_READ, &a).unwrap();
    assert_eq!(read_at(&f, 6, 100).unwrap(), b"world");
    assert_eq!(read_at(&f, 100, 10).unwrap(), b"");
    assert_eq!(read_at(&f, 0, u32::MAX).unwrap().len(), 11, "a huge length is cut to what exists");
    assert!(read_at(&f, u64::MAX, 1).is_err());
    assert!(write_at(&f, u64::MAX - 2, b"abcdef").is_err());
    // truncate on open, append
    let (f, _) = j.open("/new.txt", OPEN_WRITE | OPEN_TRUNC, &a).unwrap();
    assert_eq!(f.metadata().unwrap().len(), 0);
    let (f, _) = j.open("/new.txt", OPEN_APPEND, &a).unwrap();
    write_at(&f, 0, b"x").unwrap();
    write_at(&f, 0, b"y").unwrap();
    assert_eq!(std::fs::read_to_string(w.home.join("new.txt")).unwrap(), "xy");
    // folders
    assert_eq!(j.mkdir("/d1", &a).unwrap(), "/d1");
    assert_eq!(j.mkdir("/d1", &a), Err(FsErr::Exists));
    assert_eq!(j.mkdir("/no/such/d2", &a), Err(FsErr::NotFound), "no folder is made with missing parents");
    assert!(j.rmdir("/sub").is_err(), "not empty");
    assert!(j.remove("/d1").is_err(), "remove is for files");
    assert!(j.rmdir("/new.txt").is_err());
    assert_eq!(j.rmdir("/d1").unwrap(), "/d1");
    // rename never replaces
    assert!(j.rename("/new.txt", "/a.txt").is_err());
    assert_eq!(j.rename("/new.txt", "/renamed.txt").unwrap(), ("/new.txt".into(), "/renamed.txt".into()));
    assert!(j.rename("/sub", "/sub/inside").is_err(), "a folder does not move into itself");
    assert!(j.rename("/renamed.txt", "/link_out/x").is_err());
    assert!(j.rename("/missing", "/x").is_err());
    // setstat
    j.setstat("/renamed.txt", &Attrs { perms: Some(0o4755), size: Some(1), times: Some((1000, 2000)), ..Default::default() }).unwrap();
    let m = std::fs::metadata(w.home.join("renamed.txt")).unwrap();
    assert_eq!(m.permissions().mode() & 0o7777, 0o755, "the set-user-id bit is dropped");
    assert_eq!(m.len(), 1);
    assert_eq!(std::os::unix::fs::MetadataExt::mtime(&m), 2000);
    assert_eq!(j.remove("/renamed.txt").unwrap(), "/renamed.txt");
    assert_eq!(j.remove("/renamed.txt"), Err(FsErr::NotFound));
}

#[test]
fn only_regular_files_open_and_a_folder_is_not_a_file() {
    let (w, j) = world("kinds");
    let a = Attrs::default();
    assert!(j.open("/sub", OPEN_READ, &a).is_err());
    assert!(j.open("/", OPEN_READ, &a).is_err());
    assert!(j.open("/a.txt", 0, &a).is_err(), "neither reading nor writing");
    assert!(j.list("/a.txt").is_err());
    if std::process::Command::new("mkfifo").arg(w.home.join("pipe")).status().is_ok_and(|s| s.success()) {
        let t = std::time::Instant::now();
        assert!(j.open("/pipe", OPEN_READ, &a).is_err(), "a FIFO is not offered");
        assert!(t.elapsed().as_secs() < 5, "and opening it does not block");
    }
}

#[test]
fn hostile_names_and_huge_listings_are_handled() {
    let (w, j) = world("names");
    for name in ["new\nline", "tab\there", "ünï.txt", "emoji-\u{1F600}", ".hidden", "a b c"] {
        std::fs::write(w.home.join(name), "x").unwrap();
    }
    let n = names(&j, "/");
    for name in ["new\nline", "ünï.txt", ".hidden", "a b c"] {
        assert!(n.contains(&name.to_string()), "{name}");
    }
    assert!(j.open("/ünï.txt", OPEN_READ, &Attrs::default()).is_ok());
    assert!(j.open(&format!("/{}", "x".repeat(300)), OPEN_READ, &Attrs::default()).is_err());
    assert!(j.open(&format!("/{}", "x/".repeat(2000)), OPEN_READ, &Attrs::default()).is_err());
}

#[test]
fn a_person_without_read_rights_sees_nothing() {
    let (w, _) = world("norights");
    let none: Rule = Arc::new(|_: &Path| false);
    let j = Jail::new(w.home.clone(), vec![], none.clone(), none);
    assert!(j.list("/").is_err());
    assert!(j.open("/a.txt", OPEN_READ, &Attrs::default()).is_err());
    assert!(j.stat("/", true).is_err());
}

// ------------------------------------------------------------------ the session

type Log = Arc<Mutex<Vec<String>>>;

fn session(j: Jail) -> (Session, Log) {
    let log: Log = Arc::default();
    let l2 = log.clone();
    (Session::new(j, Box::new(move |e, d| l2.lock().unwrap().push(format!("{e} {d}")))), log)
}

fn started(j: Jail) -> (Session, Log) {
    let (mut s, log) = session(j);
    let r = s.handle(&[FXP_INIT, 0, 0, 0, 3]);
    assert_eq!(r, vec![version_packet()]);
    (s, log)
}

fn one(s: &mut Session, p: Vec<u8>) -> Vec<u8> {
    let mut r = s.handle(&p);
    assert_eq!(r.len(), 1, "{r:?}");
    r.remove(0)
}

#[test]
fn a_whole_upload_download_and_listing_conversation() {
    let (w, j) = world("conv");
    let (mut s, log) = started(j);
    // upload
    let h = handle_of(&one(&mut s, pk(FXP_OPEN, 1, &[s_("/up.bin"), u32b(OPEN_WRITE | OPEN_CREAT | OPEN_TRUNC), no_attrs()])));
    assert_eq!(code_of(&one(&mut s, pk(FXP_WRITE, 2, &[b(&h), u64b(0), b(b"SECRET-DATA-1")]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_WRITE, 3, &[b(&h), u64b(13), b(b"-2")]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_CLOSE, 4, &[b(&h)]))), FX_OK);
    assert_eq!(std::fs::read(w.home.join("up.bin")).unwrap(), b"SECRET-DATA-1-2");
    // download in two reads, then EOF
    let h = handle_of(&one(&mut s, pk(FXP_OPEN, 5, &[s_("/up.bin"), u32b(OPEN_READ), no_attrs()])));
    let d = one(&mut s, pk(FXP_READ, 6, &[b(&h), u64b(0), u32b(10)]));
    assert_eq!((d[0], &d[9..]), (FXP_DATA, b"SECRET-DAT".as_slice()));
    let d = one(&mut s, pk(FXP_READ, 7, &[b(&h), u64b(10), u32b(1000)]));
    assert_eq!(&d[9..], b"A-1-2");
    assert_eq!(code_of(&one(&mut s, pk(FXP_READ, 8, &[b(&h), u64b(15), u32b(10)]))), FX_EOF);
    // fstat
    let a = one(&mut s, pk(FXP_FSTAT, 9, &[b(&h)]));
    assert_eq!(a[0], FXP_ATTRS);
    // writing to a read handle is refused
    assert_eq!(code_of(&one(&mut s, pk(FXP_WRITE, 10, &[b(&h), u64b(0), b(b"x")]))), FX_PERMISSION_DENIED);
    assert_eq!(code_of(&one(&mut s, pk(FXP_CLOSE, 11, &[b(&h)]))), FX_OK);
    // the handle is gone
    assert_eq!(code_of(&one(&mut s, pk(FXP_READ, 12, &[b(&h), u64b(0), u32b(1)]))), FX_FAILURE);
    assert_eq!(code_of(&one(&mut s, pk(FXP_CLOSE, 13, &[b(&h)]))), FX_FAILURE);
    // listing: one NAME packet, then EOF
    let dh = handle_of(&one(&mut s, pk(FXP_OPENDIR, 14, &[s_("/")])));
    let n = one(&mut s, pk(FXP_READDIR, 15, &[b(&dh)]));
    assert_eq!(n[0], FXP_NAME);
    assert_eq!(code_of(&one(&mut s, pk(FXP_READDIR, 16, &[b(&dh)]))), FX_EOF);
    // names
    assert_eq!(code_of(&one(&mut s, pk(FXP_MKDIR, 17, &[s_("/made"), no_attrs()]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_RENAME, 18, &[s_("/made"), s_("/made2")]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_RMDIR, 19, &[s_("/made2")]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_REMOVE, 20, &[s_("/up.bin")]))), FX_OK);
    assert_eq!(code_of(&one(&mut s, pk(FXP_REMOVE, 21, &[s_("/up.bin")]))), FX_NO_SUCH_FILE);
    let rp = one(&mut s, pk(FXP_REALPATH, 22, &[s_(".")]));
    assert_eq!(rp[0], FXP_NAME);
    assert_eq!(one(&mut s, pk(FXP_STAT, 23, &[s_("/a.txt")]))[0], FXP_ATTRS);
    assert_eq!(code_of(&one(&mut s, pk(FXP_STAT, 24, &[s_("/link_out")]))), FX_NO_SUCH_FILE);
    assert_eq!(code_of(&one(&mut s, pk(FXP_READLINK, 25, &[s_("/link_in")]))), FX_OP_UNSUPPORTED);
    assert_eq!(code_of(&one(&mut s, pk(FXP_SYMLINK, 26, &[s_("/x"), s_("/etc")]))), FX_OP_UNSUPPORTED);
    assert_eq!(code_of(&one(&mut s, pk(FXP_EXTENDED, 27, &[s_("posix-rename@openssh.com")]))), FX_OP_UNSUPPORTED);
    assert!(!w.home.join("x").exists());
    // the audit trail names what changed and never what was written
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("sftp.open create /up.bin") && log.contains("sftp.mkdir /made") && log.contains("sftp.rename /made -> /made2") && log.contains("sftp.remove /up.bin"), "{log}");
    assert!(!log.contains("SECRET-DATA"), "file contents must not reach the audit log: {log}");
}

fn s_(x: &str) -> Vec<u8> {
    s(x)
}

#[test]
fn the_session_ends_on_a_broken_stream() {
    let (_w, j) = world("broken");
    // a request before INIT
    let (mut s1, _) = session(j);
    assert!(s1.handle(&pk(FXP_STAT, 1, &[s_("/")])).is_empty() && s1.closed);
    let (_w, j) = world("broken2");
    let (mut s2, log) = started(j);
    assert!(s2.feed(&[0xff, 0xff, 0xff, 0xff]).is_empty() && s2.closed, "a 4 GB packet length ends it");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("sftp.bad_stream")));
    assert!(s2.feed(&frame(&[FXP_INIT, 0, 0, 0, 3])).is_empty(), "nothing is answered after that");
    let (_w, j) = world("broken3");
    let (mut s3, _) = started(j);
    let mut bad = pk(FXP_OPEN, 1, &[s_("/a")]);
    bad.truncate(8);
    assert!(s3.feed(&frame(&bad)).is_empty() && s3.closed, "a cut-short request ends it");
    let (_w, j) = world("broken4");
    let (mut s4, _) = started(j);
    assert!(s4.handle(&[FXP_INIT, 0, 0, 0, 3]).is_empty() && s4.closed, "a second INIT ends it");
    // an unknown type with an id gets an answer, one without ends it
    let (_w, j) = world("broken5");
    let (mut s5, _) = started(j);
    assert_eq!(code_of(&one(&mut s5, pk(77, 9, &[]))), FX_OP_UNSUPPORTED);
    assert!(!s5.closed);
    assert!(s5.handle(&[77]).is_empty() && s5.closed);
}

#[test]
fn handles_are_limited_and_a_listing_comes_in_pages() {
    let (w, j) = world("limits");
    for i in 0..250 {
        std::fs::write(w.home.join(format!("f{i:03}")), "").unwrap();
    }
    let (mut s, _) = started(j);
    let mut hs = Vec::new();
    for i in 0..crate::sftp_session::MAX_HANDLES {
        hs.push(handle_of(&one(&mut s, pk(FXP_OPEN, i as u32, &[s_("/a.txt"), u32b(OPEN_READ), no_attrs()]))));
    }
    assert_eq!(code_of(&one(&mut s, pk(FXP_OPEN, 999, &[s_("/a.txt"), u32b(OPEN_READ), no_attrs()]))), FX_FAILURE, "the 65th is refused");
    assert_eq!(code_of(&one(&mut s, pk(FXP_CLOSE, 1000, &[b(&hs[0])]))), FX_OK);
    assert_eq!(one(&mut s, pk(FXP_OPEN, 1001, &[s_("/a.txt"), u32b(OPEN_READ), no_attrs()]))[0], FXP_HANDLE);
    for (i, h) in hs.iter().enumerate().skip(1) {
        one(&mut s, pk(FXP_CLOSE, 2000 + i as u32, &[b(h)]));
    }
    let dh = handle_of(&one(&mut s, pk(FXP_OPENDIR, 1, &[s_("/")])));
    let mut total = 0;
    loop {
        let r = one(&mut s, pk(FXP_READDIR, 2, &[b(&dh)]));
        if r[0] == FXP_STATUS {
            assert_eq!(code_of(&r), FX_EOF);
            break;
        }
        let n = u32::from_be_bytes(r[5..9].try_into().unwrap()) as usize;
        assert!(n <= 100 && r.len() < MAX_PACKET);
        total += n;
    }
    assert!(total >= 250);
    // a READ never returns more than 64 KiB
    std::fs::write(w.home.join("big"), vec![7u8; 300_000]).unwrap();
    let h = handle_of(&one(&mut s, pk(FXP_OPEN, 3, &[s_("/big"), u32b(OPEN_READ), no_attrs()])));
    let d = one(&mut s, pk(FXP_READ, 4, &[b(&h), u64b(0), u32b(u32::MAX)]));
    assert_eq!(d.len(), 9 + 65536);
}

// ------------------------------------------------------------------ keys, limits, firewall, manager

fn tmp(tag: &str) -> PathBuf {
    std::fs::canonicalize(crate::testdir::unique("sftpmgr", tag)).unwrap()
}

fn key_line(seed: u8, comment: &str) -> String {
    crate::sshkey::KeyPair::from_seed([seed; 32], comment).public_line()
}

#[test]
fn authorised_keys_are_checked_stored_privately_and_removed() {
    let d = tmp("keys");
    for bad in ["", "ssh-rsa AAAAB3Nza amy", "ssh-ed25519 !!!notbase64 x", "ssh-ed25519 AAAA", &format!("{}\nssh-ed25519 x", key_line(1, "a")), "-----BEGIN OPENSSH PRIVATE KEY-----", &"ssh-ed25519 ".repeat(100)] {
        assert!(parse_key_line(bad).is_err(), "{bad}");
    }
    let fp = add_key(&d, "amy", &key_line(1, "laptop")).unwrap();
    assert!(fp.starts_with("SHA256:"));
    assert_eq!(add_key(&d, "amy", &key_line(1, "again")).unwrap(), fp, "the same key twice is one key");
    add_key(&d, "amy", &key_line(2, "phone")).unwrap();
    assert_eq!(user_keys(&d, "amy").len(), 2);
    assert!(user_keys(&d, "bob").is_empty());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(d.join("sftp/keys/amy.pub")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(add_key(&d, "../evil", &key_line(3, "x")).is_err() && add_key(&d, "A b", &key_line(3, "x")).is_err());
    assert_eq!(remove_key(&d, "amy", &fp).unwrap(), 1);
    assert!(remove_key(&d, "amy", &fp).is_err());
    assert_eq!(user_keys(&d, "amy").len(), 1);
    for i in 0..MAX_KEYS_PER_USER as u8 + 2 {
        let _ = add_key(&d, "cap", &key_line(10 + i, "k"));
    }
    assert_eq!(user_keys(&d, "cap").len(), MAX_KEYS_PER_USER);
    // the host key is made once and kept
    let h1 = host_key(&d).unwrap().fingerprint();
    assert_eq!(host_key(&d).unwrap().fingerprint(), h1);
    assert_eq!(std::fs::metadata(d.join("sftp/host_key")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn many_wrong_passwords_block_an_address_for_a_while() {
    let mut l = Limiter::default();
    let ip: std::net::IpAddr = "192.168.1.5".parse().unwrap();
    let other: std::net::IpAddr = "192.168.1.6".parse().unwrap();
    for _ in 0..9 {
        l.failed(ip, 1000);
    }
    assert!(!l.blocked(ip, 1000));
    l.failed(ip, 1001);
    assert!(l.blocked(ip, 1002) && !l.blocked(other, 1002));
    assert!(l.blocked(ip, 1000 + 299) && !l.blocked(ip, 1001 + 300));
    // failures spread over more than the window never add up
    let mut l = Limiter::default();
    for i in 0..30 {
        l.failed(ip, 5000 + i * 400);
    }
    assert!(!l.blocked(ip, 5000 + 30 * 400));
}

#[test]
fn firewall_rules_are_for_the_local_network_with_an_expiry_and_replace_each_other() {
    let base = iyakki_fw::model::Config::default();
    let on = with_rules(base.clone(), Some((2222, 5000)));
    let ours: Vec<_> = on.rules.iter().filter(|r| r.comment == RULE_COMMENT).collect();
    assert_eq!(ours.len(), 4);
    for r in &ours {
        assert_eq!((r.ports.as_str(), r.proto, r.expires, r.kind), ("2222", iyakki_fw::model::Proto::Tcp, 5000, iyakki_fw::model::Kind::Cidr));
        assert!(LOCAL_NETWORKS.contains(&r.value.as_str()), "{}", r.value);
        iyakki_fw::validate::check_rule(r).unwrap();
    }
    assert!(on.only_opens_from(&base), "opening needs no confirmation step");
    let again = with_rules(on.clone(), Some((2323, 0)));
    assert_eq!(again.rules.iter().filter(|r| r.comment == RULE_COMMENT).count(), 4, "no pile-up");
    assert!(again.rules.iter().filter(|r| r.comment == RULE_COMMENT).all(|r| r.ports == "2323" && r.expires == 0));
    let off = with_rules(on, None);
    assert_eq!((off.rules, off.presets, off.enabled), (base.rules, base.presets, base.enabled), "closing leaves nothing behind");
}

struct FakeFw(Mutex<Vec<String>>, bool);
impl Firewall for FakeFw {
    fn open(&self, port: u16, expires: i64) -> Result<(), String> {
        if self.1 {
            return Err("refused".into());
        }
        self.0.lock().unwrap().push(format!("open {} {}", port != 0, expires != 0));
        Ok(())
    }
    fn close(&self) -> Result<(), String> {
        self.0.lock().unwrap().push("close".into());
        Ok(())
    }
}

struct FakeAuth {
    policy: iyakki_sec::policy::Policy,
    tries: Mutex<u32>,
}
impl iyakki_core_api::service::Authenticator for FakeAuth {
    fn login(&self, name: &str, password: &str) -> iyakki_core_api::error::ApiResult<iyakki_core_api::service::Principal> {
        *self.tries.lock().unwrap() += 1;
        if name == "amy" && password == "right-password-9" {
            Ok(iyakki_core_api::service::Principal { user: name.into(), role: "user".into(), admin: false, policy: self.policy.clone() })
        } else {
            Err(iyakki_core_api::error::ApiError::Denied("invalid username or password".into()))
        }
    }
    fn lookup(&self, user: &str) -> Option<iyakki_core_api::service::Principal> {
        (user == "amy").then(|| iyakki_core_api::service::Principal { user: user.into(), role: "user".into(), admin: false, policy: self.policy.clone() })
    }
}

struct Rig {
    sftp: Arc<Sftp>,
    fw: Arc<FakeFw>,
    audit: Log,
    w: World,
    data: PathBuf,
    auth: Arc<FakeAuth>,
}

fn rig(tag: &str) -> Rig {
    let (w, _j) = world(tag);
    let data = tmp(&format!("{tag}-data"));
    let mut policy = iyakki_sec::policy::Role::User.template();
    policy.fs_read = vec![w.home.display().to_string(), w.shared.display().to_string()];
    policy.fs_write = vec![w.home.display().to_string()];
    let auth = Arc::new(FakeAuth { policy, tries: Mutex::new(0) });
    let audit: Log = Arc::default();
    let a2 = audit.clone();
    let fw = Arc::new(FakeFw(Mutex::new(Vec::new()), false));
    let sftp = Arc::new(Sftp::new(&data, auth.clone(), Arc::new(move |e, d| a2.lock().unwrap().push(format!("{e} {d}"))), fw.clone()).with_home(w.home.clone()));
    Rig { sftp, fw, audit, w, data, auth }
}

#[test]
fn it_is_off_until_enabled_and_enabling_needs_a_way_to_log_in() {
    let r = rig("manager");
    let st = r.sftp.status();
    assert!(!st.running && st.config.users.is_empty() && !st.config.password_login);
    assert!(r.sftp.enable("amy").unwrap_err().contains("no account is allowed"));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = 0;
        Ok(())
    })
    .unwrap();
    assert!(r.sftp.enable("amy").unwrap_err().contains("nobody could log in"));
    assert!(r.fw.0.lock().unwrap().is_empty(), "nothing was opened");
    assert!(r.sftp.configure(|c| { c.port = 80; Ok(()) }).is_err() && r.sftp.configure(|c| { c.port = 8100; Ok(()) }).is_err());
    assert!(r.sftp.configure(|c| { c.users.push("A b".into()); Ok(()) }).is_err());
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    assert!(st.running && st.port != 0 && st.by == "amy");
    assert_eq!(r.fw.0.lock().unwrap().as_slice(), ["open true true".to_string()], "one firewall opening, with an expiry");
    assert!(r.sftp.enable("amy").is_err(), "twice is refused");
    // the port really answers and speaks SSH
    let mut c = std::net::TcpStream::connect(("127.0.0.1", st.port)).unwrap();
    let mut line = [0u8; 8];
    std::io::Read::read_exact(&mut c, &mut line).unwrap();
    assert_eq!(&line, b"SSH-2.0-");
    drop(c);
    assert!(r.sftp.disable("amy"));
    assert_eq!(r.fw.0.lock().unwrap().last().unwrap(), "close");
    assert!(std::net::TcpStream::connect(("127.0.0.1", st.port)).is_err(), "closed again");
    assert!(!r.sftp.disable("amy"));
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("sftp.enable by amy") && a.contains("sftp.disable by amy"), "{a}");
}

#[test]
fn a_firewall_that_refuses_leaves_nothing_listening() {
    let mut r = rig("fwfail");
    let fw = Arc::new(FakeFw(Mutex::new(Vec::new()), true));
    let a2 = r.audit.clone();
    r.sftp = Arc::new(Sftp::new(&r.data, r.auth.clone(), Arc::new(move |e, d| a2.lock().unwrap().push(format!("{e} {d}"))), fw).with_home(r.w.home.clone()));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = 0;
        Ok(())
    })
    .unwrap();
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    assert!(r.sftp.enable("amy").unwrap_err().contains("firewall"));
    assert!(!r.sftp.status().running);
}

#[test]
fn the_gate_checks_the_allow_list_keys_and_passwords_and_never_logs_secrets() {
    let r = rig("gate");
    let limiter: Arc<Mutex<Limiter>> = Arc::default();
    let mut cfg = Config { users: vec!["amy".into()], ..Config::default() };
    let gate = |cfg: &Config| RealGate { dir: r.data.clone(), cfg: cfg.clone(), auth: r.auth.clone(), audit: Arc::new(|_, _| {}), limiter: limiter.clone(), home: Some(r.w.home.clone()) };
    use crate::ssh_server::Gate;
    let ip: std::net::IpAddr = "192.168.1.9".parse().unwrap();
    let k = crate::sshkey::KeyPair::from_seed([1; 32], "k");
    assert!(!gate(&cfg).key_ok("amy", &k.public_blob(), ip), "no key yet");
    add_key(&r.data, "amy", &k.public_line()).unwrap();
    assert!(gate(&cfg).key_ok("amy", &k.public_blob(), ip));
    assert!(!gate(&cfg).key_ok("amy", &crate::sshkey::KeyPair::from_seed([2; 32], "o").public_blob(), ip));
    assert!(!gate(&cfg).key_ok("bob", &k.public_blob(), ip), "not on the list");
    // passwords: off by default
    assert!(!gate(&cfg).password_ok("amy", "right-password-9", ip));
    assert_eq!(*r.auth.tries.lock().unwrap(), 0, "with passwords off the account code is not even asked");
    cfg.password_login = true;
    assert!(gate(&cfg).password_ok("amy", "right-password-9", ip));
    assert!(!gate(&cfg).password_ok("amy", "wrong", ip));
    assert!(!gate(&cfg).password_ok("bob", "right-password-9", ip), "an account off the list never gets in, even with a good password");
    for _ in 0..12 {
        gate(&cfg).password_ok("amy", "wrong", ip);
    }
    let before = *r.auth.tries.lock().unwrap();
    assert!(!gate(&cfg).password_ok("amy", "right-password-9", ip), "the address is blocked, even for the right password");
    assert_eq!(*r.auth.tries.lock().unwrap(), before, "and the account code is not asked while blocked");
    assert!(gate(&cfg).password_ok("amy", "right-password-9", "192.168.1.10".parse().unwrap()), "another address is not affected");
    // sessions: the account's rules decide
    let s = gate(&cfg).session("amy", ip);
    assert!(s.is_some());
    assert!(gate(&cfg).session("bob", ip).is_none());
}

#[test]
fn the_jail_of_an_account_follows_its_file_rules_and_hides_the_data_folder() {
    let r = rig("jailfor");
    let policy = r.auth.policy.clone();
    let j = jail_for(&policy, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.list("/").unwrap().iter().any(|e| e.0 == "a.txt"));
    assert!(j.open("/+shared/pub.txt", OPEN_READ, &Attrs::default()).is_ok());
    assert!(j.open("/+shared/pub.txt", OPEN_WRITE, &Attrs::default()).is_err(), "the policy gives read only there");
    assert!(j.open("/link_out/secret.txt", OPEN_READ, &Attrs::default()).is_err());
    // an account with no file rules sees an empty, unwritable home
    let mut none = policy.clone();
    none.fs_read.clear();
    none.fs_write.clear();
    let j = jail_for(&none, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.list("/").is_err() && j.open("/a.txt", OPEN_READ, &Attrs::default()).is_err());
    // a read-only account cannot write in its home
    let mut ro = policy;
    ro.fs_write.clear();
    let j = jail_for(&ro, "amy", &r.data, Some(r.w.home.clone()));
    assert!(j.open("/a.txt", OPEN_READ, &Attrs::default()).is_ok());
    assert_eq!(j.open("/z.txt", OPEN_WRITE | OPEN_CREAT, &Attrs::default()).err(), Some(FsErr::Denied));
    assert_eq!(j.remove("/a.txt"), Err(FsErr::Denied));
}

// ------------------------------------------------------------------ the chat tool

#[test]
fn the_chat_tool_reads_freely_and_asks_for_every_change() {
    use crate::sftptools::*;
    let r = rig("tool");
    let t = SftpTools::new(Arc::new(crate::assign::tests_support::NoTools), r.sftp.clone(), "amy");
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("sftp_status"), Some(crate::agent::Risk::Safe));
    assert_eq!(risk("sftp_set"), Some(crate::agent::Risk::Ask));
    assert!(!t.pre_approved("sftp_set") && !t.approved_always("sftp_set") && t.grant_blocker("sftp_set", &json!({})).is_some());
    let st = t.call("sftp_status", &json!({})).unwrap();
    assert!(st.contains("SFTP server: off") && st.contains("Host key: SHA256:") && st.contains("no account is allowed"), "{st}");
    // enabling without a login way is refused with the reason, at the card
    let e = json!({"action": "enable"});
    assert!(t.refusal("sftp_set", &e).unwrap().contains("no account is allowed"));
    t.call("sftp_set", &json!({"action": "allow_user", "user": "amy"})).unwrap();
    assert!(t.refusal("sftp_set", &e).unwrap().contains("nobody could log in"));
    // keys: public lines pass, private keys and passwords never
    let line = key_line(4, "me@laptop");
    assert!(t.preview("sftp_set", &json!({"action": "add_key", "key": line})).unwrap().contains("SHA256:"));
    assert!(t.refusal("sftp_set", &json!({"action": "add_key", "key": "-----BEGIN OPENSSH PRIVATE KEY-----\nabc"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "password_login", "value": "on", "password": "x"})).unwrap().contains("never send"));
    assert!(t.refusal("sftp_set", &json!({"action": "add_key", "key": "ssh-rsa AAAA x"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "allow_user", "user": "../x"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "set_port", "port": 80})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "set_expiry", "expires": "forever"})).is_some());
    assert!(t.refusal("sftp_set", &json!({"action": "nope"})).is_some());
    t.call("sftp_set", &json!({"action": "add_key", "key": line})).unwrap();
    t.call("sftp_set", &json!({"action": "set_expiry", "expires": "1 hour"})).unwrap();
    t.call("sftp_set", &json!({"action": "set_port", "port": 2299})).unwrap();
    assert_eq!(r.sftp.config().port, 2299);
    // the card for enabling says who, what, from where and until when
    let card = t.preview("sftp_set", &e).unwrap();
    for needle in ["LOCAL NETWORK ONLY", "amy", "port 2299", "1 hour", "public keys only", "own file rules"] {
        assert!(card.contains(needle) || card.to_lowercase().contains(&needle.to_lowercase()), "{needle} in {card}");
    }
    let pw = t.preview("sftp_set", &json!({"action": "password_login", "value": "on"})).unwrap();
    assert!(pw.contains("ACCOUNT PASSWORD") && pw.contains("never stored"));
    // really enabling (port 0 for the test)
    r.sftp.configure(|c| {
        c.port = 0;
        Ok(())
    })
    .unwrap();
    let out = t.call("sftp_set", &e).unwrap();
    assert!(out.contains("SFTP server is ON") && out.contains("Local network only"), "{out}");
    assert!(t.call("sftp_status", &json!({})).unwrap().contains("SFTP server: ON"));
    assert!(t.refusal("sftp_set", &e).unwrap().contains("already running"));
    assert!(t.call("sftp_set", &json!({"action": "disable"})).unwrap().contains("turned off"));
    assert!(!r.sftp.status().running);
    assert!(t.call("sftp_set", &json!({"action": "remove_key", "fingerprint": "SHA256:nothing"})).is_err());
    // the prompt rule never asks for secrets
    assert!(PROMPT_RULE.contains("Never ask for or type a password or private key"));
}

#[test]
fn it_stops_by_itself_when_the_time_is_up_and_closes_the_firewall() {
    let r = rig("expiry");
    r.sftp
        .configure(|c| {
            c.users.push("amy".into());
            c.port = 0;
            c.expiry_secs = 1;
            Ok(())
        })
        .unwrap();
    add_key(&r.data, "amy", &key_line(1, "k")).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    assert!(st.running && st.until > 0);
    assert!(status_lines(&r.data)[0].contains("ON at port"));
    std::thread::sleep(std::time::Duration::from_millis(2600));
    let st = r.sftp.status();
    assert!(!st.running, "the time is up");
    assert!(std::net::TcpStream::connect(("127.0.0.1", st.port)).is_err() || st.port == 0);
    assert_eq!(r.fw.0.lock().unwrap().last().unwrap(), "close", "the firewall opening is removed");
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("sftp.disable by expiry")));
    assert!(status_lines(&r.data)[0].contains("off"));
    // never: no end time
    r.sftp.configure(|c| {
        c.expiry_secs = 0;
        Ok(())
    })
    .unwrap();
    r.sftp.enable("amy").unwrap();
    assert_eq!(r.sftp.status().until, 0);
    r.sftp.disable("test");
    let lines = status_lines(&r.data).join("\n");
    assert!(lines.contains("keys only") && lines.contains("amy") && !lines.to_lowercase().contains("password:"), "{lines}");
}

// ------------------------------------------------------------------ a real client over loopback

fn have(p: &str) -> bool {
    std::process::Command::new(p).arg("-V").output().is_ok() || std::process::Command::new(p).arg("-?").output().is_ok()
}

fn run_sftp(r: &Rig, port: u16, extra: &[&str], envs: &[(&str, &str)], script: &str) -> (String, String, bool) {
    use std::io::Write;
    let mut cmd = std::process::Command::new("sftp");
    cmd.args(["-P", &port.to_string(), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=10"]).args(extra).arg("amy@127.0.0.1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(&r.w.root).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

/// A real OpenSSH `sftp` against our server: key login, an upload and a download, escapes, a refused unknown key, a password login and a
/// wrong password. Skipped (with a note) when this computer has no `sftp` or `ssh-keygen`.
#[test]
fn a_real_sftp_client_works_over_loopback_and_cannot_escape() {
    if !(have("ssh") && have("ssh-keygen") && std::process::Command::new("sftp").arg("-?").output().is_ok()) {
        eprintln!("no sftp client on this computer: the loopback test is skipped");
        return;
    }
    let r = rig("real");
    let keydir = tmp("real-keys");
    let gen = |name: &str| {
        let f = keydir.join(name);
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"]).arg(&f).status().unwrap().success());
        f
    };
    let (good, stranger) = (gen("good"), gen("stranger"));
    let publine = std::fs::read_to_string(keydir.join("good.pub")).unwrap();
    r.sftp
        .configure(|c| {
            c.users.push("amy".into());
            c.port = 0;
            c.password_login = true;
            Ok(())
        })
        .unwrap();
    add_key(&r.data, "amy", &publine).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    let port = st.port;
    let id = |f: &PathBuf| ["-i".to_string(), f.display().to_string(), "-o".into(), "IdentitiesOnly=yes".into(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "PreferredAuthentications=publickey".into()];
    let good_args = id(&good);
    let good_args: Vec<&str> = good_args.iter().map(String::as_str).collect();

    std::fs::write(r.w.root.join("local-up.txt"), "uploaded by a real client\n").unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(r.w.root.join("local-big.bin"), &big).unwrap();
    let script = "pwd\nls\nput local-up.txt up.txt\nput local-big.bin big.bin\nget up.txt got.txt\nget big.bin got-big.bin\nmkdir newdir\nrename up.txt newdir/moved.txt\nls newdir\n\
                  cd /\nget /../../../outside/secret.txt leak1.txt\nget ../../outside/secret.txt leak2.txt\nget /link_out/secret.txt leak3.txt\nget /file_link leak4.txt\nls /link_out\nls /../outside\nput local-up.txt /link_out/planted.txt\nget /etc/passwd leak5.txt\nls /+shared\nget /+shared/pub.txt got-pub.txt\nput local-up.txt /+shared/nope.txt\nrm /a.txt\nrmdir newdir\nrm newdir/moved.txt\nrmdir newdir\nbye\n";
    let (out, err, _ok) = run_sftp(&r, port, &good_args, &[], script);
    eprintln!("--- sftp output ---\n{out}\n--- stderr ---\n{err}");
    assert!(out.contains("Remote working directory: /"), "{out}");
    assert!(out.contains("a.txt") && out.contains("+shared"), "{out}");
    assert_eq!(std::fs::read_to_string(r.w.root.join("got.txt")).unwrap(), "uploaded by a real client\n");
    assert_eq!(std::fs::read(r.w.root.join("got-big.bin")).unwrap(), big, "a 3 MB file crosses the encrypted channel intact");
    assert_eq!(std::fs::read(r.w.home.join("big.bin")).unwrap(), big);
    assert_eq!(std::fs::read_to_string(r.w.root.join("got-pub.txt")).unwrap(), "public");
    for leak in ["leak1.txt", "leak2.txt", "leak3.txt", "leak4.txt", "leak5.txt"] {
        assert!(!r.w.root.join(leak).exists(), "{leak} must not exist: nothing outside the scope can be fetched");
    }
    assert!(!r.w.outside.join("planted.txt").exists(), "nothing is written outside");
    assert!(!r.w.shared.join("nope.txt").exists(), "the read-only root stays read-only");
    assert!(!r.w.home.join("a.txt").exists(), "rm inside the scope works");
    assert!(!r.w.home.join("newdir").exists());
    assert_eq!(std::fs::read_to_string(r.w.outside.join("secret.txt")).unwrap(), "TOP SECRET");
    // refused: a key the server does not know
    let sid = id(&stranger);
    let sid: Vec<&str> = sid.iter().map(String::as_str).collect();
    let (_o, e, ok) = run_sftp(&r, port, &sid, &[], "ls\n");
    assert!(!ok && e.contains("Permission denied"), "{e}");
    // password login through the account code, via an askpass helper (the password is never on a command line)
    let ask = keydir.join("askpass.sh");
    std::fs::write(&ask, "#!/bin/sh\necho right-password-9\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ask, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let pw_args = ["-o", "PreferredAuthentications=password", "-o", "PubkeyAuthentication=no", "-o", "NumberOfPasswordPrompts=1"];
    let envs = [("SSH_ASKPASS", ask.to_str().unwrap()), ("SSH_ASKPASS_REQUIRE", "force"), ("DISPLAY", "none")];
    let (o, e, _) = run_sftp(&r, port, &pw_args, &envs, "ls\nbye\n");
    assert!(o.contains("+shared"), "password login should list the home: {o} {e}");
    std::fs::write(&ask, "#!/bin/sh\necho wrong-password\n").unwrap();
    let (o, e, ok) = run_sftp(&r, port, &pw_args, &envs, "ls\nbye\n");
    assert!(!ok && !o.contains("+shared") && e.contains("Permission denied"), "{o} {e}");
    // the audit trail has the logins and file changes, and no password
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("sftp.login amy from 127.0.0.1") && a.contains("sftp.login_failed amy") && a.contains("sftp.open create /up.txt") && a.contains("sftp.rename /up.txt -> /newdir/moved.txt"), "{a}");
    assert!(!a.contains("right-password-9") && !a.contains("wrong-password") && !a.contains("TOP SECRET"), "{a}");
    r.sftp.disable("test");
}

/// Not a test: starts the server on a high port for a minute so a person can try `ssh -vvv` or `sftp` by hand
/// (`IYAKKI_SFTP_DEBUG_PORT=2299 cargo test -p iyakki-agent --lib sftp_by_hand -- --ignored --nocapture`).
#[test]
#[ignore]
fn sftp_by_hand() {
    let port: u16 = std::env::var("IYAKKI_SFTP_DEBUG_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(2299);
    let r = rig("hand");
    let line = std::env::var("IYAKKI_SFTP_DEBUG_KEY").ok().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_else(|| key_line(1, "k"));
    r.sftp.configure(|c| {
        c.users.push("amy".into());
        c.port = port;
        c.password_login = true;
        Ok(())
    })
    .unwrap();
    add_key(&r.data, "amy", &line).unwrap();
    let st = r.sftp.enable("amy").unwrap();
    eprintln!("listening on {} home {}", st.port, r.w.home.display());
    std::thread::sleep(std::time::Duration::from_secs(150));
    r.sftp.disable("hand");
    eprintln!("{}", r.audit.lock().unwrap().join("\n"));
}
