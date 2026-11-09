//! Tests for the SFTP / SSH / FTP client codecs (hostile input, loopback servers). The connection manager and the vault tests are in
//! the agent: `iyakki-agent/src/remotefiles_tests.rs`.

use crate::remotefs::{clean_path, RemoteFs};
use crate::rf_ftp::{self, parse_list_line, parse_mlsd_line};
use crate::rf_sftp::{self, parse_response, plain_name, ProtoError, Resp, SftpFs};
use crate::rf_ssh::{self, SshErr};
use crate::sftp_fs::{Jail, Rule};
use crate::sftp_session::Session;
use crate::ssh_server::{serve, Gate, Server};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    std::fs::canonicalize(crate::testdir::unique("rf", tag)).unwrap()
}

fn stop() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + i / 251) % 251) as u8).collect()
}

// ====================================================================== the SFTP codec

fn be(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
fn st(b: &[u8]) -> Vec<u8> {
    let mut o = be(b.len() as u32);
    o.extend_from_slice(b);
    o
}

fn name_packet(id: u32, names: &[&str]) -> Vec<u8> {
    let mut p = vec![104];
    p.extend(be(id));
    p.extend(be(names.len() as u32));
    for n in names {
        p.extend(st(n.as_bytes()));
        p.extend(st(b"-rw-r--r-- 1 x x 1 Jan 1 00:00 f"));
        p.extend(be(0));
    }
    p
}

#[test]
fn answers_are_parsed_and_every_cut_short_prefix_is_an_error_not_a_panic() {
    let mut status = vec![101];
    status.extend(be(7));
    status.extend(be(3));
    status.extend(st(b"nope\x1b[31m"));
    status.extend(st(b""));
    assert_eq!(parse_response(&status), Ok(Resp::Status { id: 7, code: 3, msg: "nope[31m".into() }), "control characters are stripped from messages");
    let mut data = vec![103];
    data.extend(be(9));
    data.extend(st(b"hello"));
    assert_eq!(parse_response(&data), Ok(Resp::Data { id: 9, data: b"hello".to_vec() }));
    let mut attrs = vec![105];
    attrs.extend(be(1));
    attrs.extend(be(1 | 4 | 8));
    attrs.extend(5u64.to_be_bytes());
    attrs.extend(be(0o100644));
    attrs.extend(be(0));
    attrs.extend(be(1_700_000_000));
    match parse_response(&attrs).unwrap() {
        Resp::Attrs { attrs, .. } => assert_eq!((attrs.size, attrs.perms, attrs.mtime), (Some(5), Some(0o100644), Some(1_700_000_000))),
        r => panic!("{r:?}"),
    }
    for full in [&status, &data, &attrs, &name_packet(1, &["a", "b"])] {
        for n in 0..full.len() {
            let _ = parse_response(&full[..n]);
        }
    }
    assert_eq!(parse_response(&[]), Err(ProtoError::Truncated));
    assert_eq!(parse_response(&[77, 0, 0, 0, 1]), Err(ProtoError::Unknown(77)));
}

#[test]
fn lying_lengths_and_counts_are_refused_before_anything_is_allocated() {
    // a string of 4 GB in a 9-byte packet
    let mut p = vec![103];
    p.extend(be(1));
    p.extend(be(u32::MAX));
    assert!(matches!(parse_response(&p), Err(ProtoError::TooLong) | Err(ProtoError::Truncated)));
    // a million names promised, none present
    let mut p = vec![104];
    p.extend(be(1));
    p.extend(be(1_000_000));
    assert_eq!(parse_response(&p), Err(ProtoError::TooLong));
    // a plausible count but the names are cut off
    let mut p = vec![104];
    p.extend(be(1));
    p.extend(be(3));
    p.extend(st(b"a"));
    assert!(parse_response(&p).is_err());
    // an empty handle, an enormous handle
    let mut p = vec![102];
    p.extend(be(1));
    p.extend(st(b""));
    assert!(parse_response(&p).is_err());
    let mut p = vec![102];
    p.extend(be(1));
    p.extend(st(&[1u8; 300]));
    assert!(parse_response(&p).is_err());
    // too many extended attributes
    let mut p = vec![105];
    p.extend(be(1));
    p.extend(be(0x8000_0000));
    p.extend(be(1000));
    assert_eq!(parse_response(&p), Err(ProtoError::TooLong));
    // a packet over the maximum
    assert_eq!(parse_response(&vec![103; rf_sftp::MAX_PACKET + 1]), Err(ProtoError::TooLong));
}

/// A stream with scripted answers; what the client wrote is kept.
struct Script {
    input: std::io::Cursor<Vec<u8>>,
    out: Arc<Mutex<Vec<u8>>>,
}

impl Read for Script {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(b)
    }
}
impl Write for Script {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.out.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn framed(p: Vec<u8>) -> Vec<u8> {
    let mut o = be(p.len() as u32);
    o.extend(p);
    o
}

fn status_p(id: u32, code: u32) -> Vec<u8> {
    let mut p = vec![101];
    p.extend(be(id));
    p.extend(be(code));
    p.extend(st(b""));
    p.extend(st(b""));
    p
}

fn handle_p(id: u32, h: &[u8]) -> Vec<u8> {
    let mut p = vec![102];
    p.extend(be(id));
    p.extend(st(h));
    p
}

fn scripted(packets: Vec<Vec<u8>>) -> Result<SftpFs<Script>, String> {
    let mut all = framed({
        let mut v = vec![2];
        v.extend(be(3));
        v
    });
    for p in packets {
        all.extend(framed(p));
    }
    SftpFs::start(Script { input: std::io::Cursor::new(all), out: Arc::default() })
}

#[test]
fn a_hostile_listing_cannot_smuggle_paths_in_as_names() {
    // ids: realpath 2, opendir 3, readdir 4, readdir 5, close 6
    let bad = ["..", ".", "a/b", "../../etc/passwd", "", "x\0y", "ok.txt", "dir"];
    let mut real = vec![104];
    real.extend(be(2));
    real.extend(be(1));
    real.extend(st(b"/home/amy"));
    real.extend(st(b""));
    real.extend(be(0));
    let mut fs = scripted(vec![real, handle_p(3, b"h"), name_packet(4, &bad), status_p(5, 1), status_p(6, 0)]).unwrap();
    let (path, entries) = fs.list("~").unwrap();
    assert_eq!(path, "/home/amy");
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["ok.txt", "dir"]);
    assert!(!plain_name("a/b") && !plain_name("..") && plain_name("a b.txt"));
}

#[test]
fn a_server_that_answers_out_of_order_or_oversize_is_refused() {
    // the answer to request 2 carries id 99
    let r = scripted(vec![status_p(99, 0)]).unwrap().mkdir("/x");
    assert!(r.unwrap_err().contains("different request"));
    // a data block longer than the 32 KiB asked for
    let mut big = vec![103];
    big.extend(be(3));
    big.extend(st(&vec![0u8; 40_000]));
    let mut fs = scripted(vec![handle_p(2, b"h"), big]).unwrap();
    let mut sink = Vec::new();
    let e = fs.download("/f", 0, &mut sink, &mut |_| true).unwrap_err();
    assert!(e.contains("more data than was asked"), "{e}");
    assert!(sink.is_empty());
    // a packet length over the maximum
    let mut all = framed({
        let mut v = vec![2];
        v.extend(be(3));
        v
    });
    all.extend(be(0x7fff_ffff));
    let mut fs = SftpFs::start(Script { input: std::io::Cursor::new(all), out: Arc::default() }).unwrap();
    assert!(fs.mkdir("/x").unwrap_err().contains("not allowed"));
    // an old server
    let all = framed({
        let mut v = vec![2];
        v.extend(be(2));
        v
    });
    assert!(SftpFs::start(Script { input: std::io::Cursor::new(all), out: Arc::default() }).err().unwrap().contains("version 3"));
}

// ====================================================================== SFTP over the in-tree SSH server

struct TGate {
    home: PathBuf,
    key: Option<Vec<u8>>,
    logins: AtomicU32,
    attempts: AtomicU32,
}

impl Gate for TGate {
    fn key_ok(&self, user: &str, blob: &[u8], _: IpAddr) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        user == "amy" && self.key.as_deref() == Some(blob)
    }
    fn password_ok(&self, user: &str, password: &str, _: IpAddr) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        user == "amy" && password == "right-pw-1"
    }
    fn session(&self, _: &str, _: IpAddr) -> Option<Session> {
        self.logins.fetch_add(1, Ordering::Relaxed);
        let h = self.home.clone();
        let (r, w): (Rule, Rule) = (Arc::new({
            let h = h.clone();
            move |p: &std::path::Path| p.starts_with(&h)
        }), Arc::new({
            let h = h.clone();
            move |p: &std::path::Path| p.starts_with(&h)
        }));
        Some(Session::new(Jail::new(h, vec![], r, w), Box::new(|_, _| {})))
    }
    fn audit(&self, _: &str, _: &str) {}
}

struct SshRig {
    port: u16,
    gate: Arc<TGate>,
    host: Arc<crate::sshkey::KeyPair>,
    stop: Arc<AtomicBool>,
    home: PathBuf,
}

impl Drop for SshRig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn ssh_rig(tag: &str, client_key: Option<&crate::sshkey::KeyPair>) -> SshRig {
    let home = tmp(tag);
    let gate = Arc::new(TGate { home: home.clone(), key: client_key.map(|k| k.public_blob()), logins: AtomicU32::new(0), attempts: AtomicU32::new(0) });
    let host = Arc::new(crate::sshkey::KeyPair::from_seed([9; 32], "host"));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = stop();
    let (g2, h2, s2) = (gate.clone(), host.clone(), stop.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            if s2.load(Ordering::Relaxed) {
                break;
            }
            let Ok(s) = s else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
            let (g, h, st) = (g2.clone(), h2.clone(), s2.clone());
            std::thread::spawn(move || {
                let srv = Server { host_key: &h, gate: &*g, peer: "127.0.0.1".parse().unwrap() };
                serve(s, &srv, st);
            });
        }
    });
    SshRig { port, gate, host, stop, home }
}

fn tcp_to(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    s
}

fn sftp_to(rig: &SshRig, password: Option<&str>, key: Option<&crate::sshkey::KeyPair>) -> SftpFs<rf_ssh::Ssh<TcpStream>> {
    let fp = crate::sshkey::fingerprint_of_blob(&rig.host.public_blob());
    let ssh = rf_ssh::connect(tcp_to(rig.port), "amy", password, key, &|_, f| f == fp, stop()).unwrap_or_else(|e| panic!("{e}"));
    SftpFs::start(ssh).unwrap()
}

#[test]
fn sftp_over_the_in_tree_ssh_server_lists_makes_moves_and_removes() {
    let rig = ssh_rig("ssh1", None);
    std::fs::write(rig.home.join("a.txt"), "alpha").unwrap();
    std::fs::create_dir(rig.home.join("sub")).unwrap();
    let mut fs = sftp_to(&rig, Some("right-pw-1"), None);
    let (path, mut entries) = fs.list("/").unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(path, "/");
    assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect::<Vec<_>>(), vec![("a.txt", false, 5), ("sub", true, entries[1].size)]);
    assert!(entries[0].modified > 0 && entries[0].perms.len() == 9);
    fs.mkdir("/made").unwrap();
    assert!(rig.home.join("made").is_dir());
    assert!(fs.mkdir("/made").is_err(), "a second mkdir says no");
    fs.rename("/a.txt", "/b.txt").unwrap();
    assert!(rig.home.join("b.txt").exists() && !rig.home.join("a.txt").exists());
    assert_eq!(fs.size("/b.txt").unwrap(), Some(5));
    assert_eq!(fs.size("/nothing").unwrap(), None);
    fs.remove_file("/b.txt").unwrap();
    fs.remove_dir("/made").unwrap();
    assert!(fs.list("/nope").is_err());
    // the server's own scope rules still apply: nothing above the root
    let (p, _) = fs.list("/../../..").unwrap();
    assert_eq!(p, "/");
    assert_eq!(rig.gate.logins.load(Ordering::Relaxed), 1);
}

#[test]
fn sftp_moves_a_file_larger_than_the_window_and_resumes_in_both_directions() {
    let rig = ssh_rig("ssh2", None);
    let big = pattern(5 * 1024 * 1024 + 123);
    let src = rig.home.join("local-src.bin");
    std::fs::write(&src, &big).unwrap();
    let mut fs = sftp_to(&rig, Some("right-pw-1"), None);
    // upload 1 MiB, then "resume" the rest
    let mut f = std::fs::File::open(&src).unwrap();
    let mut first = vec![0u8; 1024 * 1024];
    f.read_exact(&mut first).unwrap();
    let mut seen = 0u64;
    fs.upload("/up.part", 0, &mut &first[..], &mut |n| {
        seen += n;
        true
    })
    .unwrap();
    assert_eq!(seen, first.len() as u64);
    assert_eq!(fs.size("/up.part").unwrap(), Some(1024 * 1024));
    let mut rest = f;
    let mut seen2 = 0u64;
    fs.upload("/up.part", 1024 * 1024, &mut rest, &mut |n| {
        seen2 += n;
        true
    })
    .unwrap();
    assert_eq!(seen + seen2, big.len() as u64);
    fs.rename("/up.part", "/up.bin").unwrap();
    assert_eq!(std::fs::read(rig.home.join("up.bin")).unwrap(), big);
    // download whole, and from an offset
    let mut got = Vec::new();
    fs.download("/up.bin", 0, &mut got, &mut |_| true).unwrap();
    assert_eq!(got, big);
    let mut tail = Vec::new();
    fs.download("/up.bin", 2_000_000, &mut tail, &mut |_| true).unwrap();
    assert_eq!(tail, big[2_000_000..]);
    // cancelling stops it
    let mut n = 0;
    let e = fs.download("/up.bin", 0, &mut Vec::new(), &mut |_| {
        n += 1;
        n < 3
    })
    .unwrap_err();
    assert_eq!(e, "cancelled");
}

#[test]
fn a_host_key_that_is_not_pinned_ends_the_connection_before_any_login() {
    let rig = ssh_rig("ssh3", None);
    let seen = Mutex::new(String::new());
    let r = rf_ssh::connect(tcp_to(rig.port), "amy", Some("right-pw-1"), None, &|kind, fp| {
        *seen.lock().unwrap() = format!("{kind} {fp}");
        false
    }, stop());
    match r {
        Err(SshErr::HostKey { kind, fingerprint }) => {
            assert_eq!(kind, "ssh-ed25519");
            assert_eq!(fingerprint, crate::sshkey::fingerprint_of_blob(&rig.host.public_blob()));
            assert!(seen.lock().unwrap().contains(&fingerprint));
        }
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("connected to an unpinned server"),
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.gate.attempts.load(Ordering::Relaxed), 0, "no password was sent");
    // a wrong pin is the same
    let r = rf_ssh::connect(tcp_to(rig.port), "amy", Some("right-pw-1"), None, &|_, f| f == "SHA256:other", stop());
    assert!(matches!(r, Err(SshErr::HostKey { .. })));
    assert_eq!(rig.gate.attempts.load(Ordering::Relaxed), 0);
}

#[test]
fn key_login_works_and_a_wrong_password_or_key_is_an_auth_error() {
    let k = crate::sshkey::KeyPair::from_seed([5; 32], "me");
    let rig = ssh_rig("ssh4", Some(&k));
    let mut fs = sftp_to(&rig, None, Some(&k));
    assert!(fs.list("/").is_ok());
    let fp = crate::sshkey::fingerprint_of_blob(&rig.host.public_blob());
    let ok = |_: &str, f: &str| f == fp;
    let e = rf_ssh::connect(tcp_to(rig.port), "amy", Some("wrong"), None, &ok, stop()).err().unwrap();
    assert!(matches!(e, SshErr::Auth(_)), "{e}");
    let other = crate::sshkey::KeyPair::from_seed([6; 32], "other");
    let e = rf_ssh::connect(tcp_to(rig.port), "amy", None, Some(&other), &ok, stop()).err().unwrap();
    assert!(matches!(e, SshErr::Auth(_)), "{e}");
    let e = rf_ssh::connect(tcp_to(rig.port), "amy", None, None, &ok, stop()).err().unwrap();
    assert!(matches!(e, SshErr::Auth(_)));
}

/// A server that says something wrong after the greeting, then keeps the line open for a while.
fn rogue(first: Vec<u8>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let _ = s.write_all(&first);
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    port
}

#[test]
fn a_hostile_ssh_server_gets_an_error_not_a_hang_or_a_crash() {
    let nohost = |_: &str, _: &str| true;
    // not SSH at all
    let e = rf_ssh::connect(tcp_to(rogue(b"HTTP/1.1 400 Bad Request\r\n\r\n".to_vec())), "u", Some("p"), None, &nohost, stop()).err().unwrap();
    assert!(e.to_string().contains("SSH") || e.to_string().contains("closed") || e.to_string().contains("in time") || e.to_string().contains("reset"), "{e}");
    // SSH 1
    let e = rf_ssh::connect(tcp_to(rogue(b"SSH-1.5-old\r\n".to_vec())), "u", Some("p"), None, &nohost, stop()).err().unwrap();
    assert!(e.to_string().contains("SSH 2"), "{e}");
    // a greeting line without an end
    let e = rf_ssh::connect(tcp_to(rogue(vec![b'A'; 5000])), "u", Some("p"), None, &nohost, stop()).err().unwrap();
    assert!(e.to_string().contains("too long") || e.to_string().contains("no SSH greeting"), "{e}");
    // a good greeting, then a packet that claims to be 4 GB
    let mut v = b"SSH-2.0-rogue\r\n".to_vec();
    v.extend(0xffff_fff0u32.to_be_bytes());
    v.extend([0u8; 16]);
    let e = rf_ssh::connect(tcp_to(rogue(v)), "u", Some("p"), None, &nohost, stop()).err().unwrap();
    assert!(e.to_string().contains("size that is not allowed"), "{e}");
    // a good greeting, then a tiny packet with nonsense padding
    let mut v = b"SSH-2.0-rogue\r\n".to_vec();
    v.extend(12u32.to_be_bytes());
    v.extend([200u8; 12]);
    let e = rf_ssh::connect(tcp_to(rogue(v)), "u", Some("p"), None, &nohost, stop()).err().unwrap();
    assert!(e.to_string().contains("padding") || e.to_string().contains("kex") || e.to_string().contains("key exchange"), "{e}");
    // the stop switch ends a connect that waits
    let s = stop();
    let s2 = s.clone();
    let port = rogue(Vec::new());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        s2.store(true, Ordering::Relaxed);
    });
    let t = Instant::now();
    let e = rf_ssh::connect(tcp_to(port), "u", Some("p"), None, &nohost, s).err().unwrap();
    assert!(e.to_string().contains("cancelled") || e.to_string().contains("closed"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(5));
}

// ====================================================================== FTP and FTPS against a fake server

#[test]
fn listings_are_read_in_both_formats_and_paths_in_names_are_dropped() {
    let now = 1_760_000_000; // 2025-10
    let e = parse_list_line("drwxr-xr-x    2 ftp      ftp          4096 Jan 05 12:30 my docs", now).unwrap();
    assert_eq!((e.name.as_str(), e.is_dir, e.size, e.perms.as_str()), ("my docs", true, 0, "rwxr-xr-x"));
    assert!(e.modified > 1_700_000_000 && e.modified < now, "a past date this year: {}", e.modified);
    let e = parse_list_line("-rw-r--r--    1 0        0         1234567 Mar 03  2019 old.iso", now).unwrap();
    assert_eq!((e.name.as_str(), e.size, e.modified), ("old.iso", 1_234_567, 1_551_571_200));
    let e = parse_list_line("lrwxrwxrwx    1 0        0               7 Feb 02 10:00 current -> releases/5", now).unwrap();
    assert_eq!((e.name.as_str(), e.is_link), ("current", true));
    // December seen in January belongs to last year
    let e = parse_list_line("-rw-r--r-- 1 u g 5 Dec 31 23:00 y.txt", 1_767_225_600 + 3 * 86_400).unwrap();
    assert!(e.modified < 1_767_225_600);
    for bad in ["total 12", "", "garbage", "-rw-r--r-- 1 u g 5 Jan 01 10:00 ../../x", "-rw-r--r-- 1 u g 5 Jan 01 10:00 a/b", "-rw-r--r-- 1 u g notasize Jan 01 10:00 f", "-rw-r--r-- 1 u g 5 Foo 01 10:00 f", "drwxr-xr-x 1 u g 5 Jan 01 10:00 ..", "-rw-r--r-- 1 u g 5 Jan 01 10:00 ."] {
        assert!(parse_list_line(bad, now).is_none(), "{bad}");
    }
    let e = parse_mlsd_line("type=file;size=42;modify=20240102030405;UNIX.mode=0640; a file.txt").unwrap();
    assert_eq!((e.name.as_str(), e.size, e.is_dir, e.perms.as_str()), ("a file.txt", 42, false, "rw-r-----"));
    assert_eq!(e.modified, 1_704_164_645);
    assert!(parse_mlsd_line("type=dir;modify=20240102030405; sub").unwrap().is_dir);
    for bad in ["type=cdir; .", "type=pdir; ..", "type=file;size=1; ../../x", "type=file;size=1; a/b", "type=file;size=x; f", "nofacts", "type=file;size=1;"] {
        assert!(parse_mlsd_line(bad).is_none(), "{bad}");
    }
}

#[derive(Clone, Default)]
struct Opts {
    mlsd: bool,
    evil_pasv: bool,
    evil_listing: bool,
    tls: Option<Arc<rustls::ServerConfig>>,
    no_stor_resume: bool,
    huge_line: bool,
}

#[derive(Default)]
struct FtpWorld {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    log: Vec<String>,
    logins: u32,
}

trait Duplex: Read + Write + Send {}
impl<T: Read + Write + Send> Duplex for T {}

fn ftp_line(s: &mut dyn Duplex, buf: &mut Vec<u8>) -> Option<String> {
    loop {
        if let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let l: Vec<u8> = buf.drain(..=i).collect();
            return Some(String::from_utf8_lossy(&l).trim_end().to_string());
        }
        let mut c = [0u8; 512];
        match s.read(&mut c) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&c[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return None,
        }
    }
}

fn reply(s: &mut dyn Duplex, text: &str) {
    let _ = s.write_all(format!("{text}\r\n").as_bytes());
    let _ = s.flush();
}

fn server_tls(cfg: &Arc<rustls::ServerConfig>, tcp: TcpStream) -> Box<dyn Duplex> {
    let conn = rustls::ServerConnection::new(cfg.clone()).unwrap();
    Box::new(rustls::StreamOwned::new(conn, tcp))
}

fn ftp_session(tcp: TcpStream, o: Opts, w: Arc<Mutex<FtpWorld>>) {
    let _ = tcp.set_read_timeout(Some(Duration::from_millis(100)));
    let mut ctl: Box<dyn Duplex> = Box::new(tcp.try_clone().unwrap());
    let mut buf = Vec::new();
    let mut cwd = "/".to_string();
    let mut rest = 0usize;
    let mut listener: Option<TcpListener> = None;
    let mut rnfr = String::new();
    let mut prot_p = false;
    let abs = |cwd: &str, a: &str| if a.starts_with('/') { clean_path(a) } else { clean_path(&format!("{cwd}/{a}")) };
    reply(&mut *ctl, if o.huge_line { "220 hi" } else { "220 fake ftp" });
    if o.huge_line {
        let _ = ctl.write_all(&vec![b'x'; 10_000]);
        return;
    }
    while let Some(line) = ftp_line(&mut *ctl, &mut buf) {
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        w.lock().unwrap().log.push(if cmd == "PASS" { "PASS ****".into() } else { line.clone() });
        match cmd.to_ascii_uppercase().as_str() {
            "AUTH" if o.tls.is_some() => {
                reply(&mut *ctl, "234 go ahead");
                ctl = server_tls(o.tls.as_ref().unwrap(), tcp.try_clone().unwrap());
                buf.clear();
            }
            "AUTH" => reply(&mut *ctl, "502 no tls"),
            "PBSZ" => reply(&mut *ctl, "200 ok"),
            "PROT" => {
                prot_p = arg == "P";
                reply(&mut *ctl, "200 ok");
            }
            "USER" => reply(&mut *ctl, "331 password please"),
            "PASS" => {
                if arg == "hunter2" {
                    w.lock().unwrap().logins += 1;
                    reply(&mut *ctl, "230 welcome");
                } else {
                    reply(&mut *ctl, "530 no");
                }
            }
            "OPTS" | "TYPE" => reply(&mut *ctl, "200 ok"),
            "PWD" => reply(&mut *ctl, &format!("257 \"{cwd}\" is the current folder")),
            "CWD" => {
                let p = abs(&cwd, arg);
                if p == "/" || w.lock().unwrap().dirs.contains(&p) {
                    cwd = p;
                    reply(&mut *ctl, "250 ok");
                } else {
                    reply(&mut *ctl, "550 no such folder");
                }
            }
            "EPSV" | "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = l.local_addr().unwrap().port();
                listener = Some(l);
                if cmd.eq_ignore_ascii_case("EPSV") && !o.evil_pasv {
                    reply(&mut *ctl, &format!("229 Entering Extended Passive Mode (|||{port}|)"));
                } else if cmd.eq_ignore_ascii_case("EPSV") {
                    reply(&mut *ctl, "502 use PASV");
                } else {
                    let ip = if o.evil_pasv { "10,9,9,9" } else { "127,0,0,1" };
                    reply(&mut *ctl, &format!("227 Entering Passive Mode ({ip},{},{})", port / 256, port % 256));
                }
            }
            "MLSD" | "LIST" | "RETR" | "STOR" | "APPE" => {
                let up = cmd.to_ascii_uppercase();
                if up == "MLSD" && !o.mlsd {
                    reply(&mut *ctl, "500 unknown command");
                    continue;
                }
                let target = abs(&cwd, arg);
                let (payload, is_up): (Vec<u8>, bool) = match up.as_str() {
                    "MLSD" | "LIST" => {
                        let wd = w.lock().unwrap();
                        let mut out = String::new();
                        let base = if cwd == "/" { String::new() } else { cwd.clone() };
                        if o.evil_listing {
                            if up == "LIST" {
                                for n in ["-rw-r--r-- 1 u g 5 Jan 01 10:00 ../../etc/passwd", "-rw-r--r-- 1 u g 5 Jan 01 10:00 a/b", "-rw-r--r-- 1 u g 5 Jan 01 10:00 fine.txt"] {
                                    out.push_str(&format!("{n}\r\n"));
                                }
                            } else {
                                out.push_str("type=file;size=5; ../../etc/passwd\r\ntype=file;size=5; a/b\r\ntype=file;size=5; fine.txt\r\n");
                            }
                        } else {
                            for d in wd.dirs.iter().filter(|d| d.rsplit_once('/').is_some_and(|(p, n)| p == base && !n.is_empty())) {
                                let n = d.rsplit('/').next().unwrap();
                                out.push_str(&if up == "MLSD" { format!("type=dir;modify=20240102030405; {n}\r\n") } else { format!("drwxr-xr-x 2 u g 4096 Jan 02 03:04 {n}\r\n") });
                            }
                            for (f, c) in wd.files.iter().filter(|(f, _)| f.rsplit_once('/').is_some_and(|(p, _)| p == base)) {
                                let n = f.rsplit('/').next().unwrap();
                                out.push_str(&if up == "MLSD" { format!("type=file;size={};modify=20240102030405; {n}\r\n", c.len()) } else { format!("-rw-r--r-- 1 u g {} Jan 02 03:04 {n}\r\n", c.len()) });
                            }
                        }
                        (out.into_bytes(), false)
                    }
                    "RETR" => match w.lock().unwrap().files.get(&target) {
                        Some(c) => (c[rest.min(c.len())..].to_vec(), false),
                        None => {
                            reply(&mut *ctl, "550 not found");
                            continue;
                        }
                    },
                    _ => (Vec::new(), true),
                };
                let Some(l) = listener.take() else {
                    reply(&mut *ctl, "425 no data connection");
                    continue;
                };
                reply(&mut *ctl, "150 here it comes");
                let (d, _) = l.accept().unwrap();
                let _ = d.set_read_timeout(Some(Duration::from_millis(100)));
                let mut ds: Box<dyn Duplex> = if prot_p { server_tls(o.tls.as_ref().unwrap(), d) } else { Box::new(d) };
                if is_up {
                    let mut got = Vec::new();
                    let mut c = [0u8; 8192];
                    loop {
                        match ds.read(&mut c) {
                            Ok(0) => break,
                            Ok(n) => got.extend_from_slice(&c[..n]),
                            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                            Err(_) => break,
                        }
                    }
                    let mut wd = w.lock().unwrap();
                    if up == "APPE" {
                        wd.files.entry(target).or_default().extend_from_slice(&got);
                    } else {
                        wd.files.insert(target, got);
                    }
                } else {
                    let _ = ds.write_all(&payload);
                    let _ = ds.flush();
                }
                drop(ds);
                rest = 0;
                reply(&mut *ctl, "226 done");
            }
            "REST" => {
                rest = arg.parse().unwrap_or(0);
                reply(&mut *ctl, if o.no_stor_resume { "502 no" } else { "350 ok" });
            }
            "SIZE" => match w.lock().unwrap().files.get(&abs(&cwd, arg)) {
                Some(c) => reply(&mut *ctl, &format!("213 {}", c.len())),
                None => reply(&mut *ctl, "550 not found"),
            },
            "MKD" => {
                w.lock().unwrap().dirs.insert(abs(&cwd, arg));
                reply(&mut *ctl, &format!("257 \"{}\" created", abs(&cwd, arg)));
            }
            "DELE" => reply(&mut *ctl, if w.lock().unwrap().files.remove(&abs(&cwd, arg)).is_some() { "250 gone" } else { "550 no" }),
            "RMD" => reply(&mut *ctl, if w.lock().unwrap().dirs.remove(&abs(&cwd, arg)) { "250 gone" } else { "550 no" }),
            "RNFR" => {
                rnfr = abs(&cwd, arg);
                reply(&mut *ctl, "350 ok");
            }
            "RNTO" => {
                let mut wd = w.lock().unwrap();
                if let Some(c) = wd.files.remove(&rnfr) {
                    wd.files.insert(abs(&cwd, arg), c);
                    reply(&mut *ctl, "250 ok");
                } else {
                    reply(&mut *ctl, "550 no");
                }
            }
            "QUIT" => {
                reply(&mut *ctl, "221 bye");
                return;
            }
            _ => reply(&mut *ctl, "502 not implemented"),
        }
    }
}

struct FtpRig {
    port: u16,
    world: Arc<Mutex<FtpWorld>>,
    cert_fp: String,
}

fn tls_server_config() -> (Arc<rustls::ServerConfig>, String) {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let (chain, key) = { let c = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap(); (c.cert.pem(), c.key_pair.serialize_pem()) };
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain.as_bytes()).collect::<Result<_, _>>().unwrap();
    let fp = rf_ftp::cert_fingerprint(certs[0].as_ref());
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(certs, key).unwrap();
    (Arc::new(cfg), fp)
}

fn ftp_rig(o: Opts) -> FtpRig {
    let world = Arc::new(Mutex::new(FtpWorld::default()));
    world.lock().unwrap().dirs.insert("/pub".into());
    world.lock().unwrap().files.insert("/pub/readme.txt".into(), b"read me".to_vec());
    world.lock().unwrap().files.insert("/top.bin".into(), pattern(300_000));
    let cert_fp = o.tls.as_ref().map(|_| String::new()).unwrap_or_default();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let w2 = world.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            let (o, w) = (o.clone(), w2.clone());
            std::thread::spawn(move || ftp_session(s, o, w));
        }
    });
    FtpRig { port, world, cert_fp }
}

fn ftp_to(rig: &FtpRig, tls: bool, pin: &str) -> Result<rf_ftp::FtpFs, rf_ftp::FtpErr> {
    rf_ftp::connect("127.0.0.1", rig.port, "amy", "hunter2", tls, &|fp| pin.is_empty() || fp == pin, stop())
}

#[test]
fn plain_ftp_lists_moves_resumes_and_removes_with_mlsd_or_list() {
    for mlsd in [true, false] {
        let rig = ftp_rig(Opts { mlsd, ..Opts::default() });
        let mut fs = ftp_to(&rig, false, "").unwrap_or_else(|e| panic!("{e}"));
        let (p, mut e) = fs.list("/").unwrap();
        e.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(p, "/");
        assert_eq!(e.iter().map(|x| (x.name.as_str(), x.is_dir, x.size)).collect::<Vec<_>>(), vec![("pub", true, 0), ("top.bin", false, 300_000)], "mlsd={mlsd}");
        let (p, e) = fs.list("/pub").unwrap();
        assert_eq!((p.as_str(), e.len(), e[0].name.as_str()), ("/pub", 1, "readme.txt"));
        assert!(fs.list("/nope").is_err());
        let mut got = Vec::new();
        fs.download("/top.bin", 0, &mut got, &mut |_| true).unwrap();
        assert_eq!(got, pattern(300_000));
        let mut tail = Vec::new();
        fs.download("/top.bin", 100_000, &mut tail, &mut |_| true).unwrap();
        assert_eq!(tail, pattern(300_000)[100_000..]);
        // upload, then resume with APPE
        let data = pattern(70_000);
        fs.upload("/up.part", 0, &mut &data[..40_000], &mut |_| true).unwrap();
        assert_eq!(fs.size("/up.part").unwrap(), Some(40_000));
        fs.upload("/up.part", 40_000, &mut &data[40_000..], &mut |_| true).unwrap();
        fs.rename("/up.part", "/up.bin").unwrap();
        assert_eq!(rig.world.lock().unwrap().files["/up.bin"], data);
        fs.mkdir("/newdir").unwrap();
        assert!(rig.world.lock().unwrap().dirs.contains("/newdir"));
        fs.remove_dir("/newdir").unwrap();
        fs.remove_file("/up.bin").unwrap();
        assert_eq!(fs.size("/up.bin").unwrap(), None);
        // cancel
        let mut n = 0;
        let e = fs.download("/top.bin", 0, &mut Vec::new(), &mut |_| {
            n += 1;
            n < 2
        })
        .unwrap_err();
        assert_eq!(e, "cancelled");
    }
}

#[test]
fn a_hostile_ftp_server_cannot_redirect_the_data_connection_or_smuggle_paths() {
    // PASV names another address: it is ignored, the control peer is used (EPSV is refused so PASV is tried)
    let rig = ftp_rig(Opts { evil_pasv: true, mlsd: false, ..Opts::default() });
    let mut fs = ftp_to(&rig, false, "").unwrap();
    let (_, e) = fs.list("/pub").unwrap();
    assert_eq!(e.len(), 1, "the data connection went to the control address although PASV said 10.9.9.9");
    // listings with path tricks
    for mlsd in [true, false] {
        let rig = ftp_rig(Opts { evil_listing: true, mlsd, ..Opts::default() });
        let mut fs = ftp_to(&rig, false, "").unwrap();
        let (_, e) = fs.list("/").unwrap();
        assert_eq!(e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["fine.txt"], "mlsd={mlsd}");
    }
    // a reply line far over the limit
    let rig = ftp_rig(Opts { huge_line: true, ..Opts::default() });
    let e = ftp_to(&rig, false, "").err().unwrap();
    assert!(e.to_string().contains("too long") || e.to_string().contains("closed") || e.to_string().contains("code"), "{e}");
    // a path with a line break never reaches the wire
    let rig = ftp_rig(Opts::default());
    let mut fs = ftp_to(&rig, false, "").unwrap();
    assert!(fs.list("/pub\r\nDELE /top.bin").is_err());
    assert!(fs.remove_file("/x\nDELE /top.bin").is_err());
    assert!(rig.world.lock().unwrap().files.contains_key("/top.bin"));
    assert!(!rig.world.lock().unwrap().log.iter().any(|l| l.starts_with("DELE")));
    // a wrong password
    let e = rf_ftp::connect("127.0.0.1", rig.port, "amy", "wrong", false, &|_| true, stop()).err().unwrap();
    assert!(matches!(e, rf_ftp::FtpErr::Auth(_)), "{e}");
    // the password never appears in the server's log in clear text (the test server masks it) and is sent once
    assert!(rig.world.lock().unwrap().log.iter().filter(|l| l.starts_with("PASS")).count() >= 2);
}

#[test]
fn ftps_pins_the_certificate_before_sending_the_password_and_encrypts_the_data() {
    let (cfg, fp) = tls_server_config();
    let rig = ftp_rig(Opts { tls: Some(cfg), mlsd: true, ..Opts::default() });
    let _ = &rig.cert_fp;
    // nothing pinned: refused, with the fingerprint to show, and no USER/PASS on the wire
    match ftp_to_strict(&rig, "") {
        Err(rf_ftp::FtpErr::Cert { fingerprint }) => assert_eq!(fingerprint, fp),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("connected without a pin"),
    }
    assert!(!rig.world.lock().unwrap().log.iter().any(|l| l.starts_with("USER") || l.starts_with("PASS")), "{:?}", rig.world.lock().unwrap().log);
    // another pin: the same
    assert!(matches!(ftp_to_strict(&rig, "SHA256:other"), Err(rf_ftp::FtpErr::Cert { .. })));
    assert_eq!(rig.world.lock().unwrap().logins, 0);
    // the right pin
    let mut fs = ftp_to_strict(&rig, &fp).unwrap_or_else(|e| panic!("{e}"));
    let (_, e) = fs.list("/pub").unwrap();
    assert_eq!(e[0].name, "readme.txt");
    let mut got = Vec::new();
    fs.download("/pub/readme.txt", 0, &mut got, &mut |_| true).unwrap();
    assert_eq!(got, b"read me");
    let data = pattern(50_000);
    fs.upload("/enc.bin", 0, &mut &data[..], &mut |_| true).unwrap();
    assert_eq!(rig.world.lock().unwrap().files["/enc.bin"], data);
    let log = rig.world.lock().unwrap().log.clone();
    let pos = |p: &str| log.iter().position(|l| l.starts_with(p)).unwrap();
    assert!(pos("AUTH TLS") < pos("USER") && pos("PROT P") < pos("USER"), "{log:?}");
    // a server without TLS is not used for FTPS
    let plain = ftp_rig(Opts::default());
    let e = ftp_to_strict(&plain, "").err().unwrap();
    assert!(e.to_string().contains("does not offer FTPS"), "{e}");
}

fn ftp_to_strict(rig: &FtpRig, pin: &str) -> Result<rf_ftp::FtpFs, rf_ftp::FtpErr> {
    rf_ftp::connect("localhost", rig.port, "amy", "hunter2", true, &|fp| fp == pin, stop())
}

