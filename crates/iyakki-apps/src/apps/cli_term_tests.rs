//! The CLI tile's interactive session: the keyboard path (every key to the program, F10 closes, paste, scroll back), the screen drawn at
//! the four screen sizes, the end of the program, and the refusals before a login starts. The program is a fake link; no process runs.
use iyakki_agent::facets_h::RealServices as _;

use std::sync::{Arc, Mutex};

use iyakki_vt::{Link, LinkState, Terminal};

use super::term::{map_key, palette, TermView};
use super::*;
use crate::apps::tests::cx_with_backend;

#[derive(Default)]
struct Shared {
    incoming: Vec<u8>,
    written: Vec<u8>,
    size: (u16, u16),
    closed: bool,
    exit: Option<i32>,
    hung_up: bool,
}

struct Fake(Arc<Mutex<Shared>>);
impl Link for Fake {
    fn read(&mut self, out: &mut Vec<u8>) -> LinkState {
        let mut s = self.0.lock().unwrap();
        out.append(&mut s.incoming);
        if s.closed { LinkState::Closed } else { LinkState::Open }
    }
    fn write(&mut self, b: &[u8]) {
        self.0.lock().unwrap().written.extend_from_slice(b);
    }
    fn resize(&mut self, c: u16, r: u16) {
        self.0.lock().unwrap().size = (c, r);
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.0.lock().unwrap().exit
    }
    fn close(&mut self) {
        let mut s = self.0.lock().unwrap();
        s.closed = true;
        s.hung_up = true;
    }
}

fn view() -> (TermView, Arc<Mutex<Shared>>) {
    let sh = Arc::new(Mutex::new(Shared::default()));
    let term: Terminal<Box<dyn Link>> = Terminal::new(Box::new(Fake(sh.clone())) as Box<dyn Link>, 80, 24);
    (TermView::new(term, "ssh box", None), sh)
}

fn app_with_session(tag: &str) -> (CliApp, AppCx, Arc<Mutex<Shared>>) {
    let (cx, _d) = cx_with_backend(tag, iyakki_shared::chat::Unconfigured);
    let mut app = CliApp::new(&cx);
    let (v, sh) = view();
    app.attach_session(v);
    (app, cx, sh)
}

fn take(sh: &Arc<Mutex<Shared>>) -> Vec<u8> {
    std::mem::take(&mut sh.lock().unwrap().written)
}

fn draw(app: &mut CliApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
    g
}

#[test]
fn tile_keys_become_the_keys_a_terminal_program_expects() {
    use crate::ui::Nav;
    let m = |k: Key| map_key(&k);
    assert_eq!(m(Key::Char('a')).unwrap().0, iyakki_vt::Key::Char('a'));
    let (k, mods) = m(Key::Ctrl('c')).unwrap();
    assert_eq!((k, mods.ctrl, mods.shift), (iyakki_vt::Key::Char('c'), true, false));
    assert!(m(Key::Ctrl('C')).unwrap().1.shift, "an uppercase letter means Shift was held too");
    assert_eq!(m(Key::Esc).unwrap().0, iyakki_vt::Key::Esc);
    assert_eq!(m(Key::BackTab).unwrap().0, iyakki_vt::Key::BackTab);
    assert_eq!(m(Key::F(5)).unwrap().0, iyakki_vt::Key::F(5));
    let (k, mods) = m(Key::Nav { nav: Nav::Left, shift: false, ctrl: true }).unwrap();
    assert_eq!((k, mods.ctrl), (iyakki_vt::Key::Left, true));
    assert_eq!(palette(1), rgb(205, 49, 49));
    assert_eq!(palette(16), rgb(0, 0, 0));
    assert_eq!(palette(231), rgb(255, 255, 255));
    assert_eq!(palette(232), rgb(8, 8, 8));
    assert_eq!(palette(255), rgb(238, 238, 238));
}

#[test]
fn while_a_session_runs_every_key_goes_to_the_program_and_f10_hangs_it_up() {
    let (mut app, cx, sh) = app_with_session("term-keys");
    for ev in [Event::Key(Key::Char('l')), Event::Key(Key::Char('s')), Event::Key(Key::Enter), Event::Key(Key::Esc), Event::Key(Key::Tab), Event::Key(Key::Up), Event::Key(Key::Ctrl('c')), Event::Key(Key::Backspace)] {
        assert!(matches!(app.handle(&ev, &cx), Flow::Stay), "the tile does not close or switch tabs: {ev:?}");
    }
    assert_eq!(take(&sh), b"ls\r\x1b\t\x1b[A\x03\x7f".to_vec(), "Esc and Tab are the program's, not the tile's");
    assert_eq!(app.tab(), Tab::Terminal);
    app.handle(&Event::Paste("echo hi\nsecond".into()), &cx);
    assert_eq!(take(&sh), b"echo hi\rsecond".to_vec());
    // F10 closes: the program is hung up and the tile is back
    assert!(!sh.lock().unwrap().hung_up);
    app.handle(&Event::Key(Key::F(10)), &cx);
    assert!(sh.lock().unwrap().hung_up && app.sess.is_none());
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("ssh.logout")));
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back), "after the session Esc leaves the tile again");
}

#[test]
fn output_is_read_on_the_heartbeat_asks_for_a_redraw_and_scrolls_back_with_the_wheel_and_shift_page_up() {
    use crate::ui::Nav;
    let (mut app, cx, sh) = app_with_session("term-out");
    let _ = app.wants_redraw();
    sh.lock().unwrap().incoming.extend((0..60).flat_map(|i| format!("output line {i}\r\n").into_bytes()));
    app.handle(&Event::Tick, &cx);
    assert!(app.wants_redraw(), "new output asks for a draw");
    assert!(!app.wants_redraw(), "once");
    let g = draw(&mut app, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Close (F10)"));
    let before = app.sess.as_mut().unwrap().terminal().back;
    assert_eq!(before, 0);
    app.handle(&Event::Scroll(2), &cx);
    assert_eq!(app.sess.as_mut().unwrap().terminal().back, 6, "the wheel scrolls three lines a notch");
    app.handle(&Event::Key(Key::Nav { nav: Nav::PageUp, shift: true, ctrl: false }), &cx);
    assert!(app.sess.as_mut().unwrap().terminal().back > 6);
    assert!(take(&sh).is_empty(), "scrolling sends nothing to the program");
    app.handle(&Event::Key(Key::Char('q')), &cx);
    assert_eq!(app.sess.as_mut().unwrap().terminal().back, 0, "typing returns to the live screen");
    // the program writes while the view is scrolled: it comes back to the end
    app.handle(&Event::Scroll(2), &cx);
    sh.lock().unwrap().incoming.extend_from_slice(b"more\r\n");
    app.handle(&Event::Tick, &cx);
    assert_eq!(app.sess.as_mut().unwrap().terminal().back, 0);
}

#[test]
fn the_screen_fits_every_window_size_resizes_the_program_and_keeps_its_buttons_inside() {
    let mut seen = Vec::new();
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let (mut app, cx, sh) = app_with_session(&format!("term-size-{w}"));
        sh.lock().unwrap().incoming.extend_from_slice(b"\x1b[1;31mred\x1b[0m plain \x1b[7mreverse\x1b[0m\r\n$ ");
        app.handle(&Event::Tick, &cx);
        let g = draw(&mut app, &cx, w, h);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {} is outside: {r:?}", hit.label);
        }
        let (cols, rows) = app.sess.as_ref().unwrap().grid;
        assert!(cols >= 40 && rows >= 8, "{w}x{h}: a usable grid, got {cols}x{rows}");
        assert_eq!(sh.lock().unwrap().size, (cols as u16, rows as u16), "{w}x{h}: the program was told the size");
        assert!(g.hits.iter().all(|h| h.label != "Connections"), "the tabs are not drawn over a session");
        seen.push((cols, rows));
    }
    assert!(seen[1].0 > seen[0].0 && seen[0].0 > seen[3].0, "a bigger window gives more columns: {seen:?}");
}

#[test]
fn when_the_program_ends_the_screen_stays_and_enter_closes_the_view() {
    let (mut app, cx, sh) = app_with_session("term-end");
    sh.lock().unwrap().incoming.extend_from_slice(b"Connection to box closed.\r\n");
    {
        let mut s = sh.lock().unwrap();
        s.closed = true;
        s.exit = Some(255);
    }
    app.handle(&Event::Tick, &cx);
    let g = draw(&mut app, &cx, 1024, 600);
    assert!(g.hits.iter().any(|h| h.label == "Close (F10)"));
    assert_eq!(app.sess.as_mut().unwrap().ended(), Some(255));
    assert!(take(&sh).is_empty());
    // keys no longer go anywhere; only Enter, Esc or F10 close the view
    assert!(matches!(app.handle(&Event::Key(Key::Char('x')), &cx), Flow::Stay));
    assert!(app.sess.is_some() && take(&sh).is_empty());
    app.handle(&Event::Key(Key::Enter), &cx);
    assert!(app.sess.is_none());
}

#[test]
fn a_login_is_refused_before_anything_starts_when_the_host_key_is_not_trusted_or_the_account_may_not() {
    let (cx, _d) = cx_with_backend("term-refuse", iyakki_shared::chat::Unconfigured);
    cx.services.on_login(&cx.session.user, "pass-word-12", true);
    let mut app = CliApp::new(&cx);
    app.handle(&Event::Tab(2), &cx);
    app.handle(&Event::Key(Key::Char('l')), &cx);
    assert!(app.sess.is_none() && app.conns.login.is_none(), "no connection yet");
    app.handle(&Event::Key(Key::Char('n')), &cx);
    for (i, v) in ["box", "10.0.0.5", "22", "deploy"].iter().enumerate() {
        if i > 0 {
            app.handle(&Event::Key(Key::Tab), &cx);
        }
        for c in v.chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
    }
    app.handle(&Event::Tab(conn::SAVE), &cx);
    app.handle(&Event::Key(Key::Char('l')), &cx);
    assert!(app.sess.is_none() && app.conns.login.is_none(), "nothing started");
    let g = draw(&mut app, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Log in (l)"), "the button is there");
    // the status line says why (the same line the page shows)
    assert!(app.conns.status_text().unwrap_or_default().contains("not trusted yet"), "{:?}", app.conns.status_text());
}

/// A real login on a real pseudo-terminal with a stand-in `ssh` program (Linux only): the tile finds the trusted host, starts the client
/// through the same rules as Connect (keys only: `BatchMode=yes`), shows what it prints, sends what is typed and shows how it ended.
#[cfg(unix)]
#[test]
fn a_login_starts_the_client_on_a_terminal_shows_its_output_sends_the_typing_and_ends_with_its_exit_code() {
    use iyakki_agent::ssh::{Connection, Env, HostKey};
    use std::os::unix::fs::PermissionsExt;
    let (cx, _d) = cx_with_backend("term-real", iyakki_shared::chat::Unconfigured);
    cx.services.on_login(&cx.session.user, "pass-word-12", true);
    let dir = std::env::temp_dir().join(format!("iyakki-termreal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let script = dir.join("bin/ssh");
    std::fs::write(&script, format!("#!/bin/sh\nPATH=/usr/bin:/bin; export PATH\nif [ \"$1\" = \"-V\" ]; then echo \"OpenSSH_9.6p1 Ubuntu-3, OpenSSL 3.0.13\" >&2; exit 0; fi\necho \"ARGS: $*\" > {log}\necho \"TERM=$TERM\"\nstty size\necho READY\nread line\necho got:$line\nexit 3\n", log = dir.join("args").display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.real().clis.set_search_path(Some(&dir.join("bin").display().to_string()));
    let env = Env::for_services(&iyakki_agent::services::Services::from_handle(&cx.services), &cx.session.user);
    env.connections().save(Connection { name: "box".into(), host: "10.0.0.5".into(), port: 22, user: "deploy".into(), key: String::new(), command: String::new() }, None).unwrap();
    env.known_hosts().trust("10.0.0.5", 22, &[HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: "AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H".into() }]).unwrap();
    let mut app = CliApp::new(&cx);
    app.handle(&Event::Tab(2), &cx);
    app.handle(&Event::Key(Key::Char('l')), &cx);
    assert!(app.sess.is_some(), "the login started: {:?}", app.conns.status_text());
    let screen = |app: &mut CliApp| app.sess.as_mut().unwrap().terminal().screen.text();
    let mut ok = false;
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        // (no drawing yet: the first draw fits the terminal to the tile, and the program must have read its size, which it prints before READY, first)
        app.handle(&Event::Tick, &cx);
        if screen(&mut app).contains("READY") {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ok, "the client's output reached the screen: {}", screen(&mut app));
    let _ = draw(&mut app, &cx, 1280, 720);
    let text = screen(&mut app);
    assert!(text.contains("TERM=xterm-256color"), "{text}");
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    assert!(args.contains("BatchMode=yes") && args.contains("StrictHostKeyChecking=yes") && args.contains("deploy@10.0.0.5"), "{args}");
    let (cols, rows) = app.sess.as_ref().unwrap().grid;
    for c in "hi".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    let mut ended = None;
    for _ in 0..300 {
        app.handle(&Event::Tick, &cx);
        if let Some(c) = app.sess.as_mut().unwrap().ended() {
            ended = Some(c);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let text = screen(&mut app);
    assert!(text.contains("got:hi"), "what was typed reached the program: {text}");
    assert!(text.contains("30 100"), "the program started on a terminal of the first size, 30 rows by 100 columns: {text}");
    assert!((cols, rows) != (100, 30), "and the first draw fitted the terminal to the tile ({cols}x{rows})");
    assert_eq!(ended, Some(3), "{text}");
    app.handle(&Event::Key(Key::Enter), &cx);
    assert!(app.sess.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The assistant's `ssh_login` (after the person approved the card) reaches the tile through the queue: the tile checks again, opens the
/// session with a terminal on a stand-in `ssh`, and a host whose key is not trusted opens nothing and says why.
#[cfg(unix)]
#[test]
fn a_login_the_assistant_asked_for_opens_in_the_tile_after_the_same_checks_as_the_l_key() {
    use iyakki_agent::ssh::{Connection, Env, HostKey};
    use std::os::unix::fs::PermissionsExt;
    let (cx, _d) = cx_with_backend("term-asked", iyakki_shared::chat::Unconfigured);
    cx.services.on_login(&cx.session.user, "pass-word-12", true);
    let dir = std::env::temp_dir().join(format!("iyakki-termasked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let script = dir.join("bin/ssh");
    std::fs::write(&script, format!("#!/bin/sh\nPATH=/usr/bin:/bin; export PATH\nif [ \"$1\" = \"-V\" ]; then echo \"OpenSSH_9.6p1 Ubuntu-3, OpenSSL 3.0.13\" >&2; exit 0; fi\necho \"ARGS: $*\" > {log}\necho ASKED-READY\nread line\nexit 0\n", log = dir.join("args").display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.real().clis.set_search_path(Some(&dir.join("bin").display().to_string()));
    let env = Env::for_services(&iyakki_agent::services::Services::from_handle(&cx.services), &cx.session.user);
    for (n, h) in [("box", "10.0.0.5"), ("fresh", "10.0.0.6")] {
        env.connections().save(Connection { name: n.into(), host: h.into(), port: 22, user: "deploy".into(), key: String::new(), command: String::new() }, None).unwrap();
    }
    env.known_hosts().trust("10.0.0.5", 22, &[HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: "AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H".into() }]).unwrap();
    let mut app = CliApp::new(&cx);
    // an untrusted host: nothing opens, the tile says why, the request is used up
    iyakki_agent::sshsessiontools::request_login(cx.data_dir(), &cx.session.user, "fresh");
    app.handle(&Event::Tick, &cx);
    assert!(app.sess.is_none() && app.conns.status_text().unwrap_or_default().contains("not trusted yet"), "{:?}", app.conns.status_text());
    assert!(!iyakki_agent::sshsessiontools::pending(cx.data_dir(), &cx.session.user));
    // a trusted one opens on a terminal and runs the client with a terminal asked for and keys only
    iyakki_agent::sshsessiontools::request_login(cx.data_dir(), &cx.session.user, "box");
    app.handle(&Event::Tick, &cx);
    assert!(app.sess.is_some(), "{:?}", app.conns.status_text());
    let mut ok = false;
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        app.handle(&Event::Tick, &cx);
        if app.sess.as_mut().unwrap().terminal().screen.text().contains("ASKED-READY") {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ok, "the client's output reached the screen");
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    assert!(args.contains("BatchMode=yes") && args.contains(" -t ") && args.contains("deploy@10.0.0.5") && !args.trim_end().ends_with(" true"), "a login asks for a terminal and does not run `true`: {args}");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("ssh.login")));
    app.sess.as_mut().unwrap().close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved connection becomes the arguments of an interactive login (a terminal asked for, no `true`) or of a quick check.
#[test]
fn a_login_asks_for_a_terminal_and_a_check_runs_true() {
    use iyakki_agent::ssh::Connection;
    let c = Connection { name: "box".into(), host: "10.0.0.5".into(), port: 2222, user: "deploy".into(), key: "ssh.deploy".into(), command: String::new() };
    assert_eq!(c.login_args(), ["-p", "2222", "-i", "uvault:ssh.deploy", "-t", "deploy@10.0.0.5"]);
    assert_eq!(c.args().last().map(String::as_str), Some("true"));
    let d = Connection { command: "tmux attach".into(), ..c };
    assert_eq!(d.login_args(), ["-p", "2222", "-i", "uvault:ssh.deploy", "-t", "deploy@10.0.0.5", "tmux", "attach"]);
}
