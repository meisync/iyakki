//! Tests for the SFTP protocol code: the packet codec with hostile input, the scope-confined file layer (path escapes, links) and the
//! session. (The manager, gate, firewall and chat-tool tests stay in the agent: `iyakki-agent/src/sftp_tests.rs`.)

use std::path::Path;
use std::sync::Arc;

use crate::sftp_fs::*;
use crate::sftp_proto::*;
use crate::sftp_session::Session;
use crate::testworld::{world, Log};

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

