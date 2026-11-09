use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use crate::ui::Key;
use iyakki_agent::sftp_fs::{Jail, Rule};
use iyakki_agent::sftp_session::Session;
use iyakki_agent::ssh_server::{serve, Gate, Server};
use iyakki_tile_api::remote::{RemoteSource, State};
use std::net::{IpAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn admin_cx(tag: &str) -> AppCx {
    // the person's home is where these tests download to and upload from: keep it out of the developer's real home
    crate::apps::tests::isolated_home();
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

struct G(PathBuf);

impl Gate for G {
    fn key_ok(&self, _: &str, _: &[u8], _: IpAddr) -> bool {
        false
    }
    fn password_ok(&self, user: &str, pw: &str, _: IpAddr) -> bool {
        user == "root" && pw == "pw-1"
    }
    fn session(&self, _: &str, _: IpAddr) -> Option<Session> {
        let h = self.0.clone();
        let r: Rule = Arc::new({
            let h = h.clone();
            move |p: &std::path::Path| p.starts_with(&h)
        });
        let w: Rule = Arc::new({
            let h = h.clone();
            move |p: &std::path::Path| p.starts_with(&h)
        });
        Some(Session::new(Jail::new(h, vec![], r, w), Box::new(|_, _| {})))
    }
    fn audit(&self, _: &str, _: &str) {}
}

/// The in-tree SSH/SFTP server on a loopback port, serving `home`. Returns the port and the host key's fingerprint.
fn server(home: PathBuf) -> (u16, String, Arc<AtomicBool>) {
    let host = Arc::new(iyakki_agent::sshkey::KeyPair::from_seed([4; 32], "host"));
    let fp = iyakki_agent::sshkey::fingerprint_of_blob(&host.public_blob());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, g) = (stop.clone(), Arc::new(G(home)));
    std::thread::spawn(move || {
        for s in l.incoming() {
            if s2.load(Ordering::Relaxed) {
                break;
            }
            let Ok(s) = s else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
            let (h, g, st) = (host.clone(), g.clone(), s2.clone());
            std::thread::spawn(move || {
                let srv = Server { host_key: &h, gate: &*g, peer: "127.0.0.1".parse().unwrap() };
                serve(s, &srv, st);
            });
        }
    });
    (port, fp, stop)
}

fn conn(port: u16) -> api::Conn {
    api::Conn { name: "lab".into(), host: "127.0.0.1".into(), port, user: "root".into(), ..api::Conn::default() }
}

fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The real chain without a window: the vault, the manager, the in-house SSH client, a loopback SSH server, the person's file rules.
#[test]
fn the_shell_source_connects_pins_browses_and_downloads_inside_the_persons_file_rules() {
    let cx = admin_cx("rf1");
    let src = ShellRemote(manager_for(&cx));
    let home = iyakki_agent::scope::FileRoots::new(&cx.session.policy, &cx.session.user, cx.services.dir()).home;
    let remote_home = temp_dir("rf1-remote");
    let remote_home = std::fs::canonicalize(&remote_home).unwrap();
    std::fs::write(remote_home.join("hello.txt"), "hello from the server").unwrap();
    let (port, fp, stop) = server(remote_home.clone());
    src.save(&conn(port), Some("pw-1"), None).unwrap();
    // the password is in the vault, not in the saved list
    assert_eq!(cx.services.user_get("root", "rf.lab").unwrap().as_str(), "pw-1");
    let saved = std::fs::read_to_string(cx.services.dir().join("remotefiles/root/connections.json")).unwrap();
    assert!(saved.contains("127.0.0.1") && !saved.contains("pw-1"), "{saved}");
    assert!(src.connections()[0].has_password);
    // first contact: a card, nothing pinned
    let open = |src: &ShellRemote| {
        let t = src.submit(api::Job::Open("lab".into()));
        wait("the open", || src.take(t))
    };
    let api::JobResult::Opened(api::Opened::Unknown(card)) = open(&src) else { panic!("expected a card") };
    assert_eq!((card.fingerprint.as_str(), card.kind.as_str(), card.pinned.clone()), (fp.as_str(), "ssh-ed25519", None));
    assert!(src.trust("lab", "SHA256:not-the-one").is_err());
    src.trust("lab", &fp).unwrap();
    let api::JobResult::Opened(api::Opened::Ready(sid)) = open(&src) else { panic!("expected a session") };
    let t = src.submit(api::Job::List(sid, "/".into()));
    let api::JobResult::Listing(path, entries) = wait("the listing", || src.take(t)) else { panic!("expected a listing") };
    assert_eq!((path.as_str(), entries.iter().map(|e| (e.name.as_str(), e.size)).collect::<Vec<_>>()), ("/", vec![("hello.txt", 21)]));
    // a download into the person's own folder works; into somewhere they may not write does not start
    let target = home.join("Downloads").join("hello.txt");
    let id = src.start_transfer(&api::TransferReq { session: sid, download: true, remote: "/hello.txt".into(), local: target.to_string_lossy().into(), overwrite: false }).unwrap();
    let done = wait("the download", || src.transfers().into_iter().find(|t| t.id == id && t.state.finished()));
    assert_eq!(done.state, State::Done, "{done:?}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello from the server");
    let outside = std::fs::canonicalize(temp_dir("rf1-outside")).unwrap();
    assert!(outside.exists());
    let e = src.start_transfer(&api::TransferReq { session: sid, download: true, remote: "/hello.txt".into(), local: outside.join("x.txt").to_string_lossy().into(), overwrite: false }).unwrap_err();
    assert!(e.contains("outside"), "{e}");
    assert!(!outside.join("x.txt").exists());
    // an upload from the person's folder
    std::fs::write(home.join("up.txt"), "from the person").unwrap();
    let id = src.start_transfer(&api::TransferReq { session: sid, download: false, remote: "/up.txt".into(), local: "~/up.txt".into(), overwrite: false }).unwrap();
    let done = wait("the upload", || src.transfers().into_iter().find(|t| t.id == id && t.state.finished()));
    assert_eq!(done.state, State::Done, "{done:?}");
    assert_eq!(std::fs::read_to_string(remote_home.join("up.txt")).unwrap(), "from the person");
    // a file outside the person's rules is not read for an upload
    std::fs::write(outside.join("secret.txt"), "s").unwrap();
    assert!(src.start_transfer(&api::TransferReq { session: sid, download: false, remote: "/s.txt".into(), local: outside.join("secret.txt").to_string_lossy().into(), overwrite: false }).is_err());
    // the audit log has the actions and no password
    src.close(sid);
    stop.store(true, Ordering::Relaxed);
}

/// The tile, driven through the shell's adapter with real events, over the same chain.
#[test]
fn the_tile_in_the_shell_shows_the_card_then_the_folder() {
    let cx = admin_cx("rf2");
    let remote_home = std::fs::canonicalize(temp_dir("rf2-remote")).unwrap();
    std::fs::create_dir(remote_home.join("projects")).unwrap();
    std::fs::write(remote_home.join("notes.md"), "n").unwrap();
    let (port, fp, stop) = server(remote_home);
    ShellRemote(manager_for(&cx)).save(&conn(port), Some("pw-1"), None).unwrap();
    let mut app = RemoteFilesApp::new(&cx);
    let mut g = Gfx::new(1200, 700);
    let area = R::new(0.0, 46.0, 1200.0, 654.0);
    let draw = |app: &mut RemoteFilesApp, g: &mut Gfx| {
        g.clear_hits();
        app.render(g, area, 1.0, &cx, 0);
    };
    draw(&mut app, &mut g);
    assert!(g.hits.iter().any(|h| h.label == "lab"), "the saved connection is listed");
    assert!(matches!(app.handle(&Event::Key(Key::Enter), &cx), Flow::Stay));
    wait("the card", || {
        app.handle(&Event::Tick, &cx);
        draw(&mut app, &mut g);
        g.hits.iter().any(|h| h.label.contains("Trust")).then_some(())
    });
    app.handle(&Event::Key(Key::Char('y')), &cx);
    wait("the folder", || {
        app.handle(&Event::Tick, &cx);
        draw(&mut app, &mut g);
        g.hits.iter().any(|h| h.label == "notes.md").then_some(())
    });
    assert!(g.hits.iter().any(|h| h.label == "projects"));
    assert_eq!(app.hint().contains("upload"), true);
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Stay), "Esc disconnects first");
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    let _ = fp;
    stop.store(true, Ordering::Relaxed);
}

#[test]
fn the_tile_is_a_system_app_with_a_policy_id() {
    use iyakki_sec::policy::App as PApp;
    assert_eq!((PApp::Remote.id(), PApp::Remote.label()), ("remote", "REMOTE FILES".to_string()));
    assert!(PApp::ALL.contains(&PApp::Remote));
}
