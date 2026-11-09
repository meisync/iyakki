use super::*;
use std::io::Read;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

fn tmp(tag: &str) -> PathBuf {
    std::fs::canonicalize(crate::testdir::unique("rf", tag)).unwrap()
}

fn stop() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + i / 251) % 251) as u8).collect()
}

// ====================================================================== the manager, with an in-memory connector

#[derive(Default)]
struct MemSecrets(Mutex<BTreeMap<String, String>>);

impl Secrets for MemSecrets {
    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
    fn get(&self, n: &str) -> Result<Zeroizing<String>, String> {
        self.0.lock().unwrap().get(n).map(|v| Zeroizing::new(v.clone())).ok_or_else(|| format!("`{n}` is not in the vault"))
    }
    fn set(&self, n: &str, v: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(n.into(), v.into());
        Ok(())
    }
    fn delete(&self, n: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(n);
        Ok(())
    }
}

#[derive(Default)]
struct MemServer {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    downloads: Vec<(String, u64)>,
    uploads: Vec<(String, u64)>,
    /// What identity the server shows.
    identity: String,
    connects: u32,
    /// The first progress call signals `started` and waits for `go`.
    gate: Option<(std::sync::mpsc::Sender<()>, Arc<Mutex<std::sync::mpsc::Receiver<()>>>)>,
}

struct MemConnector(Arc<Mutex<MemServer>>);

struct MemFs(Arc<Mutex<MemServer>>);

impl Connector for MemConnector {
    fn connect(&self, _c: &Conn, cred: &Cred, pin: Option<&str>, _: Arc<AtomicBool>) -> Result<Box<dyn RemoteFs>, ConnectErr> {
        let mut s = self.0.lock().unwrap();
        if pin != Some(s.identity.as_str()) {
            return Err(ConnectErr::Untrusted { kind: "ssh-ed25519".into(), fingerprint: s.identity.clone() });
        }
        if let Cred::Password(p) = cred {
            if p.as_str() != "pw-1" {
                return Err(ConnectErr::Auth("the server did not accept the login".into()));
            }
        }
        s.connects += 1;
        Ok(Box::new(MemFs(self.0.clone())))
    }
}

impl RemoteFs for MemFs {
    fn list(&mut self, path: &str) -> Result<(String, Vec<Entry>), String> {
        let s = self.0.lock().unwrap();
        let p = clean_path(path);
        if p != "/" && !s.dirs.contains(&p) {
            return Err("no such file or folder".into());
        }
        let base = if p == "/" { String::new() } else { p.clone() };
        let mut v = Vec::new();
        for d in s.dirs.iter().filter(|d| d.rsplit_once('/').is_some_and(|(a, n)| a == base && !n.is_empty())) {
            v.push(Entry { name: d.rsplit('/').next().unwrap().into(), is_dir: true, is_link: false, size: 0, modified: 0, perms: String::new() });
        }
        for (f, c) in s.files.iter().filter(|(f, _)| f.rsplit_once('/').is_some_and(|(a, _)| a == base)) {
            v.push(Entry { name: f.rsplit('/').next().unwrap().into(), is_dir: false, is_link: false, size: c.len() as u64, modified: 0, perms: String::new() });
        }
        Ok((p, v))
    }
    fn mkdir(&mut self, p: &str) -> Result<(), String> {
        self.0.lock().unwrap().dirs.insert(clean_path(p));
        Ok(())
    }
    fn rename(&mut self, a: &str, b: &str) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        if s.files.contains_key(&clean_path(b)) {
            return Err("the server refused (file exists)".into());
        }
        let c = s.files.remove(&clean_path(a)).ok_or("no such file or folder")?;
        s.files.insert(clean_path(b), c);
        Ok(())
    }
    fn remove_file(&mut self, p: &str) -> Result<(), String> {
        self.0.lock().unwrap().files.remove(&clean_path(p)).map(|_| ()).ok_or_else(|| "no such file or folder".into())
    }
    fn remove_dir(&mut self, p: &str) -> Result<(), String> {
        self.0.lock().unwrap().dirs.remove(&clean_path(p)).then_some(()).ok_or_else(|| "no such file or folder".into())
    }
    fn size(&mut self, p: &str) -> Result<Option<u64>, String> {
        Ok(self.0.lock().unwrap().files.get(&clean_path(p)).map(|c| c.len() as u64))
    }
    fn download(&mut self, remote: &str, offset: u64, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        let (data, gate) = {
            let mut s = self.0.lock().unwrap();
            s.downloads.push((remote.into(), offset));
            (s.files.get(&clean_path(remote)).cloned().ok_or("no such file or folder")?, s.gate.take())
        };
        for (i, chunk) in data[offset as usize..].chunks(1000).enumerate() {
            out.write_all(chunk).map_err(|e| e.to_string())?;
            if i == 0 {
                if let Some((started, go)) = &gate {
                    let _ = started.send(());
                    let _ = go.lock().unwrap().recv_timeout(Duration::from_secs(5));
                }
            }
            if !progress(chunk.len() as u64) {
                return Err("cancelled".into());
            }
        }
        Ok(())
    }
    fn upload(&mut self, remote: &str, offset: u64, src: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        let mut all = Vec::new();
        src.read_to_end(&mut all).map_err(|e| e.to_string())?;
        let mut s = self.0.lock().unwrap();
        s.uploads.push((remote.into(), offset));
        let f = s.files.entry(clean_path(remote)).or_default();
        f.truncate(offset as usize);
        f.extend_from_slice(&all);
        drop(s);
        if !progress(all.len() as u64) {
            return Err("cancelled".into());
        }
        Ok(())
    }
}

struct Rig {
    m: Arc<Manager>,
    srv: Arc<Mutex<MemServer>>,
    secrets: Arc<MemSecrets>,
    data: PathBuf,
    dl: PathBuf,
    audit: Arc<Mutex<Vec<String>>>,
}

fn rig(tag: &str) -> Rig {
    let data = tmp(tag);
    let dl = data.join("Downloads");
    std::fs::create_dir_all(&dl).unwrap();
    let srv = Arc::new(Mutex::new(MemServer { identity: "SHA256:real".into(), ..Default::default() }));
    srv.lock().unwrap().dirs.insert("/docs".into());
    srv.lock().unwrap().files.insert("/docs/a.bin".into(), pattern(10_500));
    srv.lock().unwrap().files.insert("/top.txt".into(), b"top".to_vec());
    let secrets = Arc::new(MemSecrets::default());
    let home = data.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let h2 = home.clone();
    let local: LocalCheck = Arc::new(move |p: &str, write: bool| {
        let pb = PathBuf::from(p);
        let probe = if write { pb.parent().map(|x| x.to_path_buf()).unwrap_or_default() } else { pb.clone() };
        let real = std::fs::canonicalize(&probe).map_err(|_| "not found or not allowed".to_string())?;
        if !real.starts_with(&h2) && !real.starts_with(std::fs::canonicalize(h2.parent().unwrap().join("Downloads")).unwrap_or_default()) {
            return Err("outside the folders your account may use".into());
        }
        Ok(if write { real.join(pb.file_name().ok_or("no name")?) } else { real })
    });
    let audit: Arc<Mutex<Vec<String>>> = Arc::default();
    let a2 = audit.clone();
    let m = Arc::new(Manager::new(&data, "amy", Arc::new(MemConnector(srv.clone())), secrets.clone(), local, Arc::new(move |e, d| a2.lock().unwrap().push(format!("{e} {d}"))), dl.to_str().unwrap()));
    Rig { m, srv, secrets, data, dl, audit }
}

fn conn(name: &str) -> Conn {
    Conn { name: name.into(), host: "nas.lan".into(), port: 22, user: "amy".into(), proto: Proto::Sftp, auth: Auth::Password, ..Conn::default() }
}

fn wait_finished(m: &Manager, id: u64) -> TransferInfo {
    let t = Instant::now();
    loop {
        if let Some(i) = m.transfers().into_iter().find(|t| t.id == id && t.state.finished()) {
            return i;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "transfer did not finish: {:?}", m.transfers());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open_ready(r: &Rig) -> u64 {
    r.m.save(&conn("nas"), Some("pw-1"), None).unwrap();
    match r.m.open("nas").unwrap() {
        Opened::Unknown(c) => r.m.trust("nas", &c.fingerprint).unwrap(),
        o => panic!("{o:?}"),
    }
    match r.m.open("nas").unwrap() {
        Opened::Ready(id) => id,
        o => panic!("{o:?}"),
    }
}

#[test]
fn a_saved_connection_keeps_its_password_in_the_vault_only() {
    let r = rig("m1");
    r.m.save(&conn("My NAS"), Some("pw-secret-9"), None).unwrap();
    let file = std::fs::read_to_string(r.data.join("remotefiles/amy/connections.json")).unwrap();
    assert!(file.contains("My NAS") && file.contains("nas.lan") && !file.contains("pw-secret-9"), "{file}");
    assert!(r.secrets.0.lock().unwrap().values().any(|v| v == "pw-secret-9"));
    assert!(r.secrets.names().contains(&"rf.my-nas".to_string()));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(r.data.join("remotefiles/amy/connections.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(r.m.connections()[0].has_password);
    assert!(!r.audit.lock().unwrap().iter().any(|l| l.contains("pw-secret-9")));
    // editing without a password keeps it; renaming moves it; an empty password removes it
    r.m.save(&Conn { host: "other.lan".into(), ..conn("My NAS") }, None, Some("My NAS")).unwrap();
    assert!(r.m.connection("My NAS").unwrap().has_password);
    r.m.save(&conn("Renamed"), None, Some("My NAS")).unwrap();
    assert!(r.m.connection("Renamed").unwrap().has_password && r.m.connection("My NAS").is_none());
    assert_eq!(r.secrets.0.lock().unwrap().get("rf.renamed").map(String::as_str), Some("pw-secret-9"));
    assert!(!r.secrets.names().contains(&"rf.my-nas".to_string()));
    r.m.save(&conn("Renamed"), Some(""), Some("Renamed")).unwrap();
    assert!(!r.m.connection("Renamed").unwrap().has_password);
    // removing removes the secret too
    r.m.save(&conn("Renamed"), Some("again-pw"), Some("Renamed")).unwrap();
    r.m.remove("Renamed").unwrap();
    assert!(r.secrets.names().is_empty() && r.m.connections().is_empty());
    assert!(r.m.remove("Renamed").is_err());
}

#[test]
fn connections_are_validated_plain_ftp_needs_the_warning_and_names_cannot_collide() {
    let r = rig("m2");
    let ftp = Conn { proto: Proto::Ftp, port: 21, ..conn("old") };
    assert!(r.m.save(&ftp, Some("x"), None).unwrap_err().contains("unencrypted"));
    r.m.save(&Conn { plain_ok: true, ..ftp.clone() }, Some("x"), None).unwrap();
    assert!(r.m.connection("old").unwrap().plain_ok);
    // the flag means nothing on a protocol that is encrypted
    r.m.save(&Conn { plain_ok: true, ..conn("safe") }, Some("x"), None).unwrap();
    assert!(!r.m.connection("safe").unwrap().plain_ok);
    for bad in [Conn { host: "bad host;rm".into(), ..conn("a") }, Conn { port: 0, ..conn("a") }, Conn { user: String::new(), ..conn("a") }, Conn { name: "../x".into(), ..conn("a") }, Conn { auth: Auth::Key, key: "id_rsa".into(), ..conn("a") }, Conn { proto: Proto::Ftps, auth: Auth::Key, key: "ssh.k".into(), ..conn("a") }] {
        assert!(r.m.save(&bad, None, None).is_err(), "{bad:?}");
    }
    assert!(r.m.save(&conn("OLD"), None, None).is_err(), "same name, other case");
    assert!(r.m.save(&conn("o l d"), None, None).is_ok());
    assert!(r.m.save(&conn("o-l-d"), None, None).is_err(), "would share the vault slot of `o l d`");
    assert!(r.m.save(&conn("x"), Some("line\nbreak"), None).is_err());
}

#[test]
fn a_new_identity_is_shown_a_changed_one_is_refused_and_only_the_shown_fingerprint_can_be_trusted() {
    let r = rig("m3");
    r.m.save(&conn("nas"), Some("pw-1"), None).unwrap();
    let Opened::Unknown(c) = r.m.open("nas").unwrap() else { panic!("expected a card") };
    assert_eq!((c.fingerprint.as_str(), c.pinned.clone(), c.host.as_str(), c.port), ("SHA256:real", None, "nas.lan", 22));
    assert_eq!(r.srv.lock().unwrap().connects, 0, "nothing connected yet");
    assert!(r.m.trust("nas", "SHA256:something-else").is_err(), "only the shown identity can be trusted");
    assert!(r.m.trust("ghost", "SHA256:real").is_err());
    r.m.trust("nas", "SHA256:real").unwrap();
    assert!(matches!(r.m.open("nas").unwrap(), Opened::Ready(_)));
    let pins = std::fs::read_to_string(r.data.join("remotefiles/amy/hostpins.json")).unwrap();
    assert!(pins.contains("sftp:nas.lan:22") && pins.contains("SHA256:real"), "{pins}");
    // the server changes its identity
    r.srv.lock().unwrap().identity = "SHA256:evil".into();
    let Opened::Changed(c) = r.m.open("nas").unwrap() else { panic!("expected a refusal") };
    assert_eq!((c.fingerprint.as_str(), c.pinned.as_deref()), ("SHA256:evil", Some("SHA256:real")));
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("remotefiles.identity_changed")));
    // trusting is possible only after the deliberate forget, and then it is a first contact again
    assert!(r.m.trust("nas", "SHA256:real").is_err());
    assert!(r.m.trust("nas", "SHA256:evil").unwrap_err().contains("forget"), "a changed identity needs the old pin forgotten first");
    assert_eq!(std::fs::read_to_string(r.data.join("remotefiles/amy/hostpins.json")).unwrap().matches("SHA256:real").count(), 1, "the pin is untouched");
    r.m.forget_host("nas").unwrap();
    assert!(matches!(r.m.open("nas").unwrap(), Opened::Unknown(c) if c.pinned.is_none()));
    assert!(r.m.forget_host("nas").is_err(), "nothing pinned any more");
    // a wrong password is an error, not a card
    r.m.trust("nas", "SHA256:evil").unwrap();
    r.m.save(&conn("nas"), Some("wrong"), Some("nas")).unwrap();
    assert!(r.m.open("nas").unwrap_err().contains("did not accept"));
    r.m.save(&conn("nas"), Some(""), Some("nas")).unwrap();
    assert!(r.m.open("nas").unwrap_err().contains("no password is stored"));
}

#[test]
fn browsing_makes_renames_and_deletes_with_the_root_protected_and_bad_paths_refused() {
    let r = rig("m4");
    let s = open_ready(&r);
    let (p, e) = r.m.list(s, "/").unwrap();
    assert_eq!((p.as_str(), e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>()), ("/", vec!["docs", "top.txt"]));
    r.m.mkdir(s, "/docs/new").unwrap();
    r.m.rename(s, "/top.txt", "/top2.txt").unwrap();
    r.m.delete(s, "/top2.txt", false).unwrap();
    r.m.delete(s, "/docs/new", true).unwrap();
    assert!(r.m.delete(s, "/", true).is_err() && r.m.delete(s, "/a/..", true).is_err(), "never the root");
    assert!(r.m.list(s, "/x\r\nDELE /").is_err() && r.m.list(s, "").is_err() && r.m.list(s, "a\0b").is_err());
    assert!(r.m.list(s, "/nowhere").is_err());
    r.m.close(s);
    assert_eq!(r.m.list(s, "/").unwrap_err(), "that connection is closed");
    let log = r.audit.lock().unwrap().join("\n");
    assert!(log.contains("remotefiles.delete /top2.txt") && log.contains("remotefiles.mkdir /docs/new") && log.contains("remotefiles.open"), "{log}");
    // jobs for the tile: a ticket, then the answer once
    let s = match r.m.open("nas").unwrap() {
        Opened::Ready(id) => id,
        o => panic!("{o:?}"),
    };
    let t = r.m.submit(Job::List(s, "/docs".into()));
    let t0 = Instant::now();
    let got = loop {
        if let Some(x) = r.m.take(t) {
            break x;
        }
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(&got, JobResult::Listing(p, e) if p == "/docs" && e.len() == 1), "{got:?}");
    assert!(matches!(r.m.take(t), Some(JobResult::Failed(_))), "a ticket is answered once");
}

#[test]
fn a_download_goes_to_a_part_file_and_is_renamed_when_complete() {
    let r = rig("m5");
    let s = open_ready(&r);
    let target = r.dl.join("a.bin");
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: false }).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!((i.state, i.done, i.total, i.resumed), (State::Done, 10_500, 10_500, false), "{i:?}");
    assert_eq!(std::fs::read(&target).unwrap(), pattern(10_500));
    assert!(!r.dl.join("a.bin.part").exists());
    // it will not replace without being told
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: false }).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!(i.state, State::Failed);
    assert!(i.note.contains("already exists"), "{}", i.note);
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: true }).unwrap();
    assert_eq!(wait_finished(&r.m, id).state, State::Done);
    // an unknown remote file fails clearly
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/none".into(), local: r.dl.join("none").to_str().unwrap().into(), overwrite: false }).unwrap();
    let i = wait_finished(&r.m, id);
    assert!(i.state == State::Failed && i.note.contains("not on the server"), "{i:?}");
    assert!(!r.dl.join("none").exists() && !r.dl.join("none.part").exists());
}

#[test]
fn a_stopped_download_resumes_from_the_part_file_and_cancel_keeps_it() {
    let r = rig("m6");
    let s = open_ready(&r);
    let target = r.dl.join("resume.bin");
    // an earlier attempt left 4000 bytes
    std::fs::write(r.dl.join("resume.bin.part"), &pattern(10_500)[..4000]).unwrap();
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: false }).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!((i.state, i.resumed), (State::Done, true), "{i:?}");
    assert_eq!(std::fs::read(&target).unwrap(), pattern(10_500), "the continued file is complete and right");
    assert_eq!(r.srv.lock().unwrap().downloads.last().unwrap().1, 4000, "the server was asked from byte 4000");
    // a .part that is longer than the file is not trusted
    std::fs::remove_file(&target).unwrap();
    std::fs::write(r.dl.join("resume.bin.part"), vec![1u8; 20_000]).unwrap();
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: false }).unwrap();
    assert_eq!(wait_finished(&r.m, id).state, State::Done);
    assert_eq!(std::fs::read(&target).unwrap(), pattern(10_500));
    assert_eq!(r.srv.lock().unwrap().downloads.last().unwrap().1, 0);
    // cancel in the middle: the part stays, the final file does not exist, and retry continues
    std::fs::remove_file(&target).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    r.srv.lock().unwrap().gate = Some((tx, Arc::new(Mutex::new(go_rx))));
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: target.to_str().unwrap().into(), overwrite: false }).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    r.m.cancel_transfer(id);
    go_tx.send(()).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!(i.state, State::Cancelled, "{i:?}");
    assert!(!target.exists());
    let part = std::fs::metadata(r.dl.join("resume.bin.part")).unwrap().len();
    assert!(part > 0 && part < 10_500, "{part}");
    let again = r.m.retry_transfer(id).unwrap();
    let i = wait_finished(&r.m, again);
    assert_eq!((i.state, i.resumed), (State::Done, true), "{i:?}");
    assert_eq!(std::fs::read(&target).unwrap(), pattern(10_500));
    assert!(r.m.retry_transfer(again).is_err(), "only a stopped transfer is retried");
    r.m.clear_finished();
    assert!(r.m.transfers().is_empty());
}

#[test]
fn uploads_go_to_a_part_name_resume_and_replace_only_when_told() {
    let r = rig("m7");
    let s = open_ready(&r);
    let src = r.data.join("home/up.bin");
    std::fs::write(&src, pattern(9000)).unwrap();
    let req = |ow| TransferReq { session: s, download: false, remote: "/docs/up.bin".into(), local: src.to_str().unwrap().into(), overwrite: ow };
    let id = r.m.start_transfer(&req(false)).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!((i.state, i.done, i.total), (State::Done, 9000, 9000), "{i:?}");
    assert_eq!(r.srv.lock().unwrap().files["/docs/up.bin"], pattern(9000));
    assert!(!r.srv.lock().unwrap().files.contains_key("/docs/up.bin.part"));
    assert_eq!(r.srv.lock().unwrap().uploads.last().unwrap(), &("/docs/up.bin.part".to_string(), 0));
    // the same name again: refused unless told
    let id = r.m.start_transfer(&req(false)).unwrap();
    let i = wait_finished(&r.m, id);
    assert!(i.state == State::Failed && i.note.contains("already on the server"), "{i:?}");
    // told: the old one is replaced
    std::fs::write(&src, pattern(9500)).unwrap();
    let id = r.m.start_transfer(&req(true)).unwrap();
    assert_eq!(wait_finished(&r.m, id).state, State::Done);
    assert_eq!(r.srv.lock().unwrap().files["/docs/up.bin"], pattern(9500));
    // a partial upload on the server is continued
    r.srv.lock().unwrap().files.remove("/docs/up.bin");
    r.srv.lock().unwrap().files.insert("/docs/up.bin.part".into(), pattern(9500)[..3000].to_vec());
    let id = r.m.start_transfer(&req(false)).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!((i.state, i.resumed), (State::Done, true), "{i:?}");
    assert_eq!(r.srv.lock().unwrap().uploads.last().unwrap().1, 3000);
    assert_eq!(r.srv.lock().unwrap().files["/docs/up.bin"], pattern(9500));
}

#[test]
fn local_paths_obey_the_file_rules_and_links_are_not_written_through() {
    let r = rig("m8");
    let s = open_ready(&r);
    let outside = tmp("m8-out");
    std::fs::write(outside.join("secret"), "s").unwrap();
    // downloading into a folder the account may not use, uploading a file it may not read
    let e = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/top.txt".into(), local: outside.join("t.txt").to_str().unwrap().into(), overwrite: true }).unwrap_err();
    assert!(e.contains("outside"), "{e}");
    let e = r.m.start_transfer(&TransferReq { session: s, download: false, remote: "/s".into(), local: outside.join("secret").to_str().unwrap().into(), overwrite: false }).unwrap_err();
    assert!(e.contains("outside"), "{e}");
    // a link at the target is not written through
    std::os::unix::fs::symlink(outside.join("victim"), r.dl.join("link.txt")).unwrap();
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/top.txt".into(), local: r.dl.join("link.txt").to_str().unwrap().into(), overwrite: true }).unwrap();
    let i = wait_finished(&r.m, id);
    assert_eq!(i.state, State::Failed);
    assert!(!outside.join("victim").exists(), "nothing was written through the link: {i:?}");
    // a .part that is a link is refused too
    std::os::unix::fs::symlink(outside.join("victim2"), r.dl.join("p.txt.part")).unwrap();
    let id = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/top.txt".into(), local: r.dl.join("p.txt").to_str().unwrap().into(), overwrite: false }).unwrap();
    assert_eq!(wait_finished(&r.m, id).state, State::Failed);
    assert!(!outside.join("victim2").exists());
    // a closed session cannot start anything; a path with a line break is refused
    r.m.close(s);
    assert!(r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/top.txt".into(), local: "x".into(), overwrite: false }).is_err());
}

#[test]
fn at_most_two_transfers_run_at_once_and_the_rest_wait() {
    let r = rig("m9");
    let s = open_ready(&r);
    let (tx, rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    r.srv.lock().unwrap().gate = Some((tx, Arc::new(Mutex::new(go_rx))));
    let ids: Vec<u64> = (0..4).map(|i| r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: r.dl.join(format!("f{i}.bin")).to_str().unwrap().into(), overwrite: false }).unwrap()).collect();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let states = || r.m.transfers().iter().map(|t| t.state).collect::<Vec<_>>();
    let running = states().iter().filter(|s| **s == State::Running).count();
    let queued = states().iter().filter(|s| **s == State::Queued).count();
    assert_eq!((running, queued), (2, 2), "{:?}", states());
    go_tx.send(()).unwrap();
    for id in ids {
        assert_eq!(wait_finished(&r.m, id).state, State::Done);
    }
    // a queued transfer can be cancelled before it starts
    let (tx, rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    r.srv.lock().unwrap().gate = Some((tx, Arc::new(Mutex::new(go_rx))));
    let a = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: r.dl.join("g0.bin").to_str().unwrap().into(), overwrite: false }).unwrap();
    let b = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: r.dl.join("g1.bin").to_str().unwrap().into(), overwrite: false }).unwrap();
    let c = r.m.start_transfer(&TransferReq { session: s, download: true, remote: "/docs/a.bin".into(), local: r.dl.join("g2.bin").to_str().unwrap().into(), overwrite: false }).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    r.m.cancel_transfer(c);
    go_tx.send(()).unwrap();
    assert_eq!(wait_finished(&r.m, c).state, State::Cancelled);
    assert_eq!(wait_finished(&r.m, a).state, State::Done);
    assert_eq!(wait_finished(&r.m, b).state, State::Done);
    assert!(!r.dl.join("g2.bin").exists());
}

// ====================================================================== the assistant's tools

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
        Err("no".into())
    }
    fn describe(&self, n: &str, _: &serde_json::Value) -> String {
        n.into()
    }
}

use crate::agent::{Risk, ToolDef, Tools};
use crate::remotefilestools::RemoteTools;
use serde_json::json;

#[test]
fn the_chat_tools_list_freely_ask_to_act_and_cannot_trust_a_server_or_carry_a_password() {
    let r = rig("t1");
    let tools = RemoteTools::new(Arc::new(NoTools), r.m.clone());
    let defs = tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    for n in ["remote_connections", "remote_list", "remote_transfers", "remote_close"] {
        assert_eq!(risk(n), Risk::Safe, "{n}");
    }
    for n in ["remote_connect", "remote_transfer", "remote_change", "remote_save"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    assert!(defs.iter().find(|d| d.spec.name == "remote_list").unwrap().untrusted, "names from a server are untrusted text");
    assert!(!defs.iter().any(|d| d.spec.name.contains("trust") || d.spec.name.contains("forget")), "the assistant cannot trust a server");
    // saving: no password, no plain ftp
    assert!(tools.call("remote_save", &json!({"name": "n", "host": "h.lan", "user": "u", "protocol": "sftp", "password": "x"})).is_err());
    assert!(tools.refusal("remote_save", &json!({"name": "n", "host": "h.lan", "user": "u", "protocol": "ftp"})).is_some());
    assert!(tools.refusal("remote_save", &json!({"name": "n", "host": "h.lan", "user": "u", "protocol": "sftp", "key": "-----BEGIN OPENSSH PRIVATE KEY-----"})).is_some());
    let out = tools.call("remote_save", &json!({"name": "box", "host": "Box.LAN", "user": "u", "protocol": "ftps"})).unwrap();
    assert!(out.contains("password in the REMOTE FILES tile") || out.contains("enters the password"), "{out}");
    assert!(tools.call("remote_connections", &json!({})).unwrap().contains("box: FTPS u@box.lan:21, no password stored"));
    // connect: an unknown identity is returned as a card text, not trusted
    r.m.save(&conn("nas"), Some("pw-1"), None).unwrap();
    let out = tools.call("remote_connect", &json!({"name": "nas"})).unwrap();
    assert!(out.contains("NOT connected") && out.contains("SHA256:real") && out.contains("REMOTE FILES tile"), "{out}");
    assert!(tools.preview("remote_connect", &json!({"name": "nas"})).unwrap().contains("nas.lan"));
    r.m.trust("nas", "SHA256:real").unwrap();
    let out = tools.call("remote_connect", &json!({"name": "nas"})).unwrap();
    assert!(out.contains("Connected: session"), "{out}");
    let s = r.m.sessions()[0].0;
    // the changed identity is a refusal
    r.srv.lock().unwrap().identity = "SHA256:evil".into();
    assert!(tools.call("remote_connect", &json!({"name": "nas"})).unwrap().contains("REFUSED"));
    r.srv.lock().unwrap().identity = "SHA256:real".into();
    // listing escapes control characters of a hostile name
    r.srv.lock().unwrap().files.insert("/evil\u{1b}[2J name".into(), vec![1]);
    let out = tools.call("remote_list", &json!({"session": s, "path": "/"})).unwrap();
    assert!(out.contains("docs") && !out.contains('\u{1b}'), "{out:?}");
    // change: the delete card names the path and cannot be granted for good
    let del = json!({"session": s, "action": "delete", "path": "/top.txt"});
    assert!(tools.preview("remote_change", &del).unwrap().contains("DELETE") && tools.grant_blocker("remote_change", &del).is_some());
    assert!(tools.grant_blocker("remote_change", &json!({"session": s, "action": "mkdir", "path": "/x"})).is_none());
    assert!(tools.call("remote_change", &del).is_ok());
    assert!(tools.call("remote_change", &json!({"session": s, "action": "delete", "path": "/"})).is_err());
    assert!(tools.refusal("remote_change", &json!({"session": s, "action": "delete", "path": "/a\nb"})).is_some());
    // transfer
    let out = tools.call("remote_transfer", &json!({"session": s, "direction": "download", "remote": "/docs/a.bin", "local": r.dl.join("t.bin").to_str().unwrap()})).unwrap();
    assert!(out.contains("queued"));
    let id = r.m.transfers()[0].id;
    wait_finished(&r.m, id);
    assert!(r.dl.join("t.bin").exists());
    assert!(tools.call("remote_transfers", &json!({"clear": true})).unwrap().contains("No transfers"));
    assert!(tools.call("remote_close", &json!({"session": s})).unwrap().contains("Closed"));
    assert!(tools.call("remote_list", &json!({"session": s})).is_err());
    assert!(tools.refusal("remote_transfer", &json!({"session": 1, "direction": "sideways", "remote": "a", "local": "b"})).is_some());
    // other tools pass through
    assert_eq!(tools.call("other", &json!({})), Err("no".into()));
}
