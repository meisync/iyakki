//! Tests of the LINUX WINDOW tile: key mapping, geometry at the usual sizes and in panes, the whole start/draw/input/resize/stop path
//! against a fake sandbox (a fake spawner that serves RFB on the real socket path) and a fake `/proc`. No network, no namespaces.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::linuxapps::{LinuxApps, Sandbox};
use iyakki_agent::linuxwin::{self as lw, Proc, Spawn};
use iyakki_agent::wsexec::FakeRunner;
use iyakki_rfb::keysyms as ks;

use super::linuxwin::*;
use super::tests::cx_with_admin_store;
use super::{App, AppCx, Flow};
use crate::gfx::Gfx;
use crate::ui::{Event, Key, Nav, R};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-lwtile-{tag}-{}", std::process::id()));
    let _ = iyakki_agent::rootfs::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn make_la(data: &Path, network: bool) -> LinuxApps {
    let mut la = LinuxApps::new(data, "root", Path::new("/usr/bin/iyakki"), Arc::new(FakeRunner::new(&[])));
    la.assume(false, Some(50 << 30));
    let own = la.dir_of(lw::SANDBOX);
    let root = la.rootfs_of(lw::SANDBOX);
    for sub in ["usr/bin", "usr/local/bin", "usr/share/applications", "home/user", "mnt", "tmp"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::write(root.join("usr/bin/Xvnc"), "x").unwrap();
    std::fs::write(root.join(lw::SCRIPT_REL), lw::session_script()).unwrap();
    std::fs::write(root.join("usr/share/applications/xterm.desktop"), "[Desktop Entry]\nType=Application\nName=XTerm\nExec=xterm\n").unwrap();
    let s = Sandbox { name: lw::SANDBOX.into(), owner: "root".into(), release: lw::RELEASE.into(), created: 1, network, mounts: vec![], id_base: 0, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: "t".into(), source: "t".into(), arch: "x86_64".into() };
    std::fs::write(own.join("sandbox.json"), serde_json::to_string(&s).unwrap()).unwrap();
    la
}

// ------------------------------------------------------------------------------------------------ the fake display server

#[derive(Default)]
struct Server {
    /// Everything the client sent after the handshake, as messages `(type, bytes)`.
    msgs: Mutex<Vec<(u8, Vec<u8>)>>,
}

fn rect_hdr(x: u16, y: u16, w: u16, h: u16, enc: i32) -> Vec<u8> {
    let mut v = Vec::new();
    for n in [x, y, w, h] {
        v.extend_from_slice(&n.to_be_bytes());
    }
    v.extend_from_slice(&enc.to_be_bytes());
    v
}

fn ext_size(reason: u16, w: u16, h: u16) -> Vec<u8> {
    let mut r = rect_hdr(reason, 0, w, h, -308);
    r.extend_from_slice(&[1, 0, 0, 0]);
    r.extend_from_slice(&7u32.to_be_bytes());
    r.extend_from_slice(&[0, 0, 0, 0]);
    r.extend_from_slice(&w.to_be_bytes());
    r.extend_from_slice(&h.to_be_bytes());
    r.extend_from_slice(&0u32.to_be_bytes());
    r
}

fn red_square() -> Vec<u8> {
    let mut r = rect_hdr(0, 0, 8, 8, 0);
    for _ in 0..64 {
        r.extend_from_slice(&[0, 0, 255, 0]);
    }
    r
}

fn serve(listener: UnixListener, srv: Arc<Server>, w: u16, h: u16, alive: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok((mut c, _)) = listener.accept() else { return };
        let mut send = |c: &mut std::os::unix::net::UnixStream, b: &[u8]| c.write_all(b).is_ok();
        send(&mut c, b"RFB 003.008\n");
        let mut buf = [0u8; 12];
        if c.read_exact(&mut buf).is_err() {
            return;
        }
        send(&mut c, &[1, 1]);
        let mut one = [0u8; 1];
        let _ = c.read_exact(&mut one);
        send(&mut c, &0u32.to_be_bytes());
        let _ = c.read_exact(&mut one);
        let mut init = Vec::new();
        init.extend_from_slice(&w.to_be_bytes());
        init.extend_from_slice(&h.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        init.extend_from_slice(&4u32.to_be_bytes());
        init.extend_from_slice(b"fake");
        send(&mut c, &init);
        // the client's first messages: SetPixelFormat, SetEncodings, an update request
        let mut first = vec![0u8; 20];
        if c.read_exact(&mut first).is_err() {
            return;
        }
        let mut hd = [0u8; 4];
        let _ = c.read_exact(&mut hd);
        let n = u16::from_be_bytes([hd[2], hd[3]]) as usize;
        let mut enc = vec![0u8; n * 4];
        let _ = c.read_exact(&mut enc);
        let mut req = [0u8; 10];
        let _ = c.read_exact(&mut req);
        // announce the resize ability and draw a red square
        let mut upd = vec![0, 0, 0, 2];
        upd.extend(ext_size(0, w, h));
        upd.extend(red_square());
        send(&mut c, &upd);
        loop {
            let mut t = [0u8; 1];
            if c.read_exact(&mut t).is_err() {
                break;
            }
            let len = match t[0] {
                3 => 9,
                4 => 7,
                5 => 5,
                6 => 7, // then the text
                251 => 7,
                _ => 0,
            };
            let mut body = vec![0u8; len];
            if c.read_exact(&mut body).is_err() {
                break;
            }
            match t[0] {
                6 => {
                    let l = u32::from_be_bytes([body[3], body[4], body[5], body[6]]) as usize;
                    let mut text = vec![0u8; l];
                    let _ = c.read_exact(&mut text);
                    body.extend(text);
                }
                251 => {
                    let ns = body[5] as usize;
                    let mut screens = vec![0u8; ns * 16];
                    let _ = c.read_exact(&mut screens);
                    let (nw, nh) = (u16::from_be_bytes([body[1], body[2]]), u16::from_be_bytes([body[3], body[4]]));
                    body.extend(screens);
                    let mut u = vec![0, 0, 0, 1];
                    u.extend(ext_size(1, nw, nh));
                    send(&mut c, &u);
                }
                _ => {}
            }
            srv.msgs.lock().unwrap().push((t[0], body));
        }
        alive.store(false, Ordering::SeqCst);
    });
}

struct FakeProc {
    alive: Arc<AtomicBool>,
    killed: Arc<AtomicBool>,
}
impl Proc for FakeProc {
    fn pid(&self) -> Option<u32> {
        Some(4242)
    }
    fn alive(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    fn kill(&mut self) {
        self.killed.store(true, Ordering::SeqCst);
    }
}

struct FakeSpawn {
    srv: Arc<Server>,
    killed: Arc<AtomicBool>,
    argv: Mutex<Vec<String>>,
    size: (u16, u16),
}
impl Spawn for FakeSpawn {
    fn spawn(&self, argv: &[String], _cwd: &Path, log: &Path) -> Result<Box<dyn Proc>, String> {
        *self.argv.lock().unwrap() = argv.to_vec();
        std::fs::write(log, "").unwrap();
        let i = argv.iter().position(|a| a == "--bind-rw").ok_or("no run folder")?;
        let sock = Path::new(&argv[i + 1]).join("display.sock");
        let _ = std::fs::remove_file(&sock);
        let l = UnixListener::bind(&sock).map_err(|e| e.to_string())?;
        let alive = Arc::new(AtomicBool::new(true));
        serve(l, self.srv.clone(), self.size.0, self.size.1, alive.clone());
        Ok(Box::new(FakeProc { alive, killed: self.killed.clone() }))
    }
}

struct Rigged {
    app: LinuxWinApp,
    cx: AppCx,
    srv: Arc<Server>,
    killed: Arc<AtomicBool>,
    spawn: Arc<FakeSpawn>,
    dir: PathBuf,
}

fn fake_proc_tree(root: &Path) {
    let mk = |pid: u32, ppid: u32, ticks: u64, rss: u64| {
        let d = root.join(pid.to_string());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("stat"), format!("{pid} (p) S {ppid} 1 1 0 -1 0 0 0 0 0 {ticks} 0 0 0 20 0 1 0 1 1 1\n")).unwrap();
        std::fs::write(d.join("status"), format!("VmRSS:\t{rss} kB\n")).unwrap();
    };
    mk(4242, 1, 10, 2048);
    mk(4243, 4242, 50, 51_200);
}

fn rig(tag: &str, network: bool, size: (u16, u16)) -> Rigged {
    let cx = cx_with_admin_store(tag);
    let dir = tmp(tag);
    make_la(&dir, network);
    let proc_root = dir.join("proc");
    fake_proc_tree(&proc_root);
    let srv = Arc::new(Server::default());
    let killed = Arc::new(AtomicBool::new(false));
    let spawn = Arc::new(FakeSpawn { srv: srv.clone(), killed: killed.clone(), argv: Mutex::new(vec![]), size });
    let d2 = dir.clone();
    let rig = Rig {
        la: Arc::new(move || {
            let mut la = LinuxApps::new(&d2, "root", Path::new("/usr/bin/iyakki"), Arc::new(FakeRunner::new(&[])));
            la.assume(false, Some(50 << 30));
            Ok(la)
        }),
        spawn: spawn.clone(),
        proc_root,
    };
    let app = LinuxWinApp::with_rig(&cx, rig);
    Rigged { app, cx, srv, killed, spawn, dir }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn draw(a: &mut LinuxWinApp, cx: &AppCx, w: u32, h: u32, area: R) -> Gfx {
    let mut g = Gfx::new(w, h);
    a.render(&mut g, area, 1.0, cx, 0);
    g
}

fn start_running(r: &mut Rigged, area: R) -> Gfx {
    // one draw first, so the tile knows its area (the start asks the display for that size)
    let _ = draw(&mut r.app, &r.cx, area.x as u32 + area.w as u32, area.y as u32 + area.h as u32, area);
    r.app.handle(&Event::Tab(1), &r.cx);
    assert_eq!(r.app.phase, Phase::Starting);
    wait_for("running", || {
        r.app.handle(&Event::Tick, &r.cx);
        r.app.running()
    });
    wait_for("the picture", || {
        r.app.handle(&Event::Tick, &r.cx);
        r.app.frame_size() != (0, 0)
    });
    draw(&mut r.app, &r.cx, area.x as u32 + area.w as u32, area.y as u32 + area.h as u32, area)
}

fn px(g: &Gfx, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * g.width() + x) * 4) as usize;
    let d = g.rgba();
    [d[i], d[i + 1], d[i + 2]]
}

// ------------------------------------------------------------------------------------------------ pure parts

#[test]
fn keys_become_keysym_sequences_with_modifiers_around_the_key() {
    assert_eq!(key_syms(&Key::Char('a')), [(true, 0x61), (false, 0x61)]);
    assert_eq!(key_syms(&Key::Char('A')), [(true, 0x41), (false, 0x41)], "the display's keymap adds Shift itself");
    assert_eq!(key_syms(&Key::Char('é')), [(true, 0xe9), (false, 0xe9)]);
    assert_eq!(key_syms(&Key::Char('€')), [(true, 0x0100_20ac), (false, 0x0100_20ac)]);
    assert_eq!(key_syms(&Key::Ctrl('c')), [(true, ks::CONTROL_L), (true, 0x63), (false, 0x63), (false, ks::CONTROL_L)]);
    assert_eq!(key_syms(&Key::Ctrl('C')), [(true, ks::CONTROL_L), (true, ks::SHIFT_L), (true, 0x63), (false, 0x63), (false, ks::SHIFT_L), (false, ks::CONTROL_L)]);
    assert_eq!(key_syms(&Key::Enter)[0], (true, 0xff0d));
    assert_eq!(key_syms(&Key::BackTab), [(true, ks::SHIFT_L), (true, 0xfe20), (false, 0xfe20), (false, ks::SHIFT_L)]);
    assert_eq!(key_syms(&Key::Nav { nav: Nav::Right, shift: true, ctrl: true }), [(true, ks::CONTROL_L), (true, ks::SHIFT_L), (true, 0xff53), (false, 0xff53), (false, ks::SHIFT_L), (false, ks::CONTROL_L)]);
    assert_eq!(key_syms(&Key::F(5))[0].1, 0xffc2);
    assert_eq!(key_syms(&Key::F(99))[0].1, ks::function_key(12));
    for (k, s) in [(Key::Left, 0xff51), (Key::Up, 0xff52), (Key::Right, 0xff53), (Key::Down, 0xff54), (Key::Home, 0xff50), (Key::End, 0xff57), (Key::PageUp, 0xff55), (Key::PageDown, 0xff56), (Key::Esc, 0xff1b), (Key::Tab, 0xff09), (Key::Backspace, 0xff08), (Key::Delete, 0xffff)] {
        assert_eq!(key_syms(&k), [(true, s), (false, s)], "{k:?}");
    }
    // a repeat is the same sequence sent again: every press is balanced by a release
    let v = key_syms(&Key::Ctrl('v'));
    assert_eq!(v.iter().filter(|e| e.0).count(), v.iter().filter(|e| !e.0).count());
    assert_eq!(paste_keys(), key_syms(&Key::Ctrl('v')));
}

#[test]
fn the_layout_never_goes_negative_and_the_wanted_size_stays_in_bounds() {
    for (w, h) in [(360.0, 740.0), (800.0, 480.0), (1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1600.0), (0.0, 0.0), (10.0, 5.0), (-5.0, -5.0), (f32::NAN, 100.0), (100000.0, 100000.0)] {
        for s in [0.5f32, 1.0, 2.0, 4.0] {
            let area = R::new(0.0, 0.0, w, h);
            let (strip, view) = layout(area, s);
            assert!(strip.h >= 0.0 && view.h >= 0.0 && strip.w >= 0.0 && view.w >= 0.0, "{w}x{h} s{s}");
            assert!(strip.h + view.h <= area.h.max(0.0) + 0.01 || area.h.is_nan());
            let (ww, wh) = wanted_size(view);
            assert!((lw::MIN_SIZE..=lw::MAX_SIZE).contains(&ww) && (lw::MIN_SIZE..=lw::MAX_SIZE).contains(&wh), "{ww}x{wh}");
        }
    }
}

#[test]
fn a_press_is_a_hit_only_inside_the_area_also_in_a_pane() {
    let view = R::new(400.0, 100.0, 300.0, 200.0);
    assert_eq!(to_remote(view, 400, 100), Some((0, 0)));
    assert_eq!(to_remote(view, 699, 299), Some((299, 199)));
    assert_eq!(to_remote(view, 700, 150), None);
    assert_eq!(to_remote(view, 399, 150), None);
    assert_eq!(to_remote(view, 500, 99), None);
    assert_eq!(to_remote(view, 500, 300), None);
    assert_eq!(to_remote(R::new(0.0, 0.0, 0.0, 0.0), 0, 0), None);
}

#[test]
fn memory_is_said_in_megabytes_or_gigabytes() {
    assert_eq!(mem_text(2048), "2 MB");
    assert_eq!(mem_text(300 * 1024), "300 MB");
    assert_eq!(mem_text(3 * 1024 * 1024), "3.0 GB");
}

// ------------------------------------------------------------------------------------------------ the whole path with fakes

#[test]
fn start_draw_input_clipboard_resize_and_stop_against_a_fake_sandbox() {
    let mut r = rig("lw-full", false, (640, 400));
    assert_eq!(r.app.phase, Phase::Pick);
    assert_eq!(r.app.programs.len(), 1);
    let area = R::new(0.0, 0.0, 800.0, 480.0);
    let g = start_running(&mut r, area);
    // the sandbox command: the session script with the size of the area (minus the strip), and the program
    let argv = r.spawn.argv.lock().unwrap().clone();
    let dd = argv.iter().position(|a| a == "--").unwrap();
    assert_eq!(argv[dd + 1], lw::SCRIPT_IN);
    assert_eq!((argv[dd + 2].as_str(), argv[dd + 3].as_str(), argv[dd + 4].as_str()), ("800", "452", "xterm"), "the area below the 28 pixel strip");
    // the picture is drawn at the top left of the area below the strip, the rest of the area is dark
    let (_, view) = layout(area, 1.0);
    assert_eq!(px(&g, 2, view.y as u32 + 2), [255, 0, 0], "the red square");
    assert_eq!(px(&g, 100, view.y as u32 + 100), [0, 0, 0], "the black screen");
    assert_eq!(px(&g, 700, view.y as u32 + 400), [14, 16, 24], "outside the 640x400 screen");
    let hit = g.hits.iter().find(|h| h.label == "linux window").expect("the area is a hit region");
    assert_eq!(hit.ev, Event::Area(AREA_WIN));
    assert!(hit.rect.x >= 0.0 && hit.rect.y >= area.y && hit.rect.x + hit.rect.w <= 800.0 && hit.rect.y + hit.rect.h <= 480.0 + 0.01);
    for label in ["Stop", "Copy from window", "Paste into window"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label}");
    }

    // keyboard, pointer, wheel
    r.app.handle(&Event::Key(Key::Char('x')), &r.cx);
    r.app.handle(&Event::Key(Key::Ctrl('c')), &r.cx);
    r.app.handle(&Event::AreaAt(AREA_WIN, 10.0, 20.0), &r.cx);
    r.app.handle(&Event::MouseUp(10, (view.y + 20.0) as i32), &r.cx);
    r.app.handle(&Event::MouseMove(30, (view.y + 40.0) as i32), &r.cx);
    r.app.handle(&Event::MouseRight(31, (view.y + 41.0) as i32), &r.cx);
    r.app.handle(&Event::Scroll(2), &r.cx);
    r.app.handle(&Event::Scroll(-1), &r.cx);
    wait_for("input arrives", || r.srv.msgs.lock().unwrap().len() >= 20);
    let msgs = r.srv.msgs.lock().unwrap().clone();
    let keys: Vec<(bool, u32)> = msgs.iter().filter(|m| m.0 == 4).map(|m| (m.1[0] == 1, u32::from_be_bytes([m.1[3], m.1[4], m.1[5], m.1[6]]))).collect();
    assert_eq!(&keys[..2], &[(true, 0x78), (false, 0x78)]);
    assert_eq!(&keys[2..6], &[(true, ks::CONTROL_L), (true, 0x63), (false, 0x63), (false, ks::CONTROL_L)]);
    let ptr: Vec<(u8, u16, u16)> = msgs.iter().filter(|m| m.0 == 5).map(|m| (m.1[0], u16::from_be_bytes([m.1[1], m.1[2]]), u16::from_be_bytes([m.1[3], m.1[4]]))).collect();
    assert_eq!(&ptr[..3], &[(0, 10, 20), (1, 10, 20), (0, 10, 20)], "move, press, release at the position inside the screen");
    assert!(ptr.contains(&(0, 30, 40)), "a plain move");
    assert!(ptr.windows(3).any(|w| w == [(0, 31, 41), (4, 31, 41), (0, 31, 41)]), "a right click: {ptr:?}");
    assert!(ptr.iter().filter(|p| p.0 == 8).count() == 2 && ptr.iter().filter(|p| p.0 == 16).count() == 1, "wheel up twice, down once: {ptr:?}");

    // the clipboard bridge is explicit: nothing is sent until the button is pressed
    assert!(!r.srv.msgs.lock().unwrap().iter().any(|m| m.0 == 6));
    crate::ui::clipboard::copy("hello\u{7} world");
    r.app.handle(&Event::Tab(5), &r.cx);
    wait_for("the text arrives", || r.srv.msgs.lock().unwrap().iter().any(|m| m.0 == 6));
    let cut = r.srv.msgs.lock().unwrap().iter().find(|m| m.0 == 6).unwrap().1.clone();
    assert_eq!(&cut[7..], b"hello world", "typed text only: the control character is dropped");

    // resize: a different area, after it stayed so for a moment the display is asked for that size
    let area2 = R::new(0.0, 0.0, 1000.0, 600.0);
    let t0 = Instant::now();
    loop {
        let _ = draw(&mut r.app, &r.cx, 1000, 600, area2);
        r.app.handle(&Event::Tick, &r.cx);
        if r.srv.msgs.lock().unwrap().iter().any(|m| m.0 == 251) {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "no resize request");
        std::thread::sleep(Duration::from_millis(20));
    }
    let req = r.srv.msgs.lock().unwrap().iter().find(|m| m.0 == 251).unwrap().1.clone();
    assert_eq!((u16::from_be_bytes([req[1], req[2]]), u16::from_be_bytes([req[3], req[4]])), (1000, 572));
    wait_for("the new picture size", || {
        r.app.handle(&Event::Tick, &r.cx);
        r.app.frame_size() == (1000, 572)
    });
    let g2 = draw(&mut r.app, &r.cx, 1000, 600, area2);
    assert_eq!(px(&g2, 5, 28 + 5), [0, 0, 0], "the picture was replaced by the new, black one");

    // cpu and memory of the sandbox's processes, from the fake /proc
    wait_for("usage", || {
        r.app.handle(&Event::Tick, &r.cx);
        r.app.usage.procs == 2
    });
    assert_eq!(r.app.usage.rss_kb, 2048 + 51_200);
    assert_eq!(r.app.peak_kb, 53_248);

    // stop kills the sandbox's process and goes back to the list
    r.app.handle(&Event::Tab(3), &r.cx);
    assert!(r.killed.load(Ordering::SeqCst));
    assert_eq!(r.app.phase, Phase::Pick);
    assert!(!r.dir.join("linuxapps/root/gui/run/display.sock").exists());
    let log = std::fs::read_to_string(r.dir.join("linuxapps/audit.log")).unwrap();
    assert!(log.contains("window.start") && log.contains("window.stop"), "{log}");
    let _ = iyakki_agent::rootfs::remove_tree(&r.dir);
}

#[test]
fn closing_the_tile_stops_the_sandbox() {
    let mut r = rig("lw-drop", false, (320, 200));
    let _ = start_running(&mut r, R::new(0.0, 0.0, 800.0, 480.0));
    let killed = r.killed.clone();
    let dir = r.dir.clone();
    drop(r.app);
    assert!(killed.load(Ordering::SeqCst), "dropping the app kills the process; its pid namespace goes with it");
    let _ = iyakki_agent::rootfs::remove_tree(&dir);
}

#[test]
fn closing_the_tile_while_it_starts_stops_it_too() {
    let mut r = rig("lw-early", false, (320, 200));
    let _ = draw(&mut r.app, &r.cx, 800, 480, R::new(0.0, 0.0, 800.0, 480.0));
    r.app.handle(&Event::Tab(1), &r.cx);
    assert_eq!(r.app.phase, Phase::Starting);
    let killed = r.killed.clone();
    let dir = r.dir.clone();
    drop(r.app);
    wait_for("the start thread cleans up", || killed.load(Ordering::SeqCst));
    let _ = iyakki_agent::rootfs::remove_tree(&dir);
}

#[test]
fn the_tile_draws_at_the_usual_sizes_and_in_panes_without_panicking() {
    let mut r = rig("lw-sizes", false, (400, 300));
    let base = draw(&mut r.app, &r.cx, 800, 480, R::new(0.0, 0.0, 800.0, 480.0));
    assert!(base.hits.iter().any(|h| h.label == "program"), "the list of programs");
    let _ = start_running(&mut r, R::new(0.0, 0.0, 800.0, 480.0));
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (100, 50), (1, 1)] {
        for pane in [R::new(0.0, 0.0, w as f32, h as f32), R::new(w as f32 / 2.0, 0.0, w as f32 / 2.0, h as f32), R::new(0.0, h as f32 / 2.0, w as f32, h as f32 / 2.0), R::new(-30.0, -10.0, w as f32 + 60.0, h as f32 + 20.0)] {
            let g = draw(&mut r.app, &r.cx, w, h, pane);
            if let Some(hit) = g.hits.iter().find(|h| h.label == "linux window") {
                assert!(hit.rect.w >= 0.0 && hit.rect.h >= 0.0 && hit.rect.y >= pane.y - 0.01);
                assert!(hit.rect.x + hit.rect.w <= pane.x + pane.w + 0.01 && hit.rect.y + hit.rect.h <= pane.y + pane.h + 0.01, "{w}x{h} {pane:?} {:?}", hit.rect);
            }
            // pointer and wheel at odd places never panic
            r.app.handle(&Event::MouseMove(-5, -5), &r.cx);
            r.app.handle(&Event::MouseMove(w as i32 + 50, h as i32 + 50), &r.cx);
            r.app.handle(&Event::AreaAt(AREA_WIN, 99999.0, -4.0), &r.cx);
            r.app.handle(&Event::MouseUp(-1, 99999), &r.cx);
            r.app.handle(&Event::Scroll(i32::MAX), &r.cx);
            r.app.handle(&Event::Scroll(i32::MIN), &r.cx);
        }
    }
    r.app.stop();
    let _ = iyakki_agent::rootfs::remove_tree(&r.dir);
}

#[test]
fn the_panel_draws_in_every_phase_at_the_usual_sizes() {
    let cx = cx_with_admin_store("lw-panel");
    for (name, make) in [("empty", 0), ("installed", 1)] {
        let dir = tmp(&format!("lw-panel-{name}"));
        if make == 1 {
            make_la(&dir, false);
        }
        let d2 = dir.clone();
        let rg = Rig { la: Arc::new(move || Ok(LinuxApps::new(&d2, "root", Path::new("/usr/bin/iyakki"), Arc::new(FakeRunner::new(&[]))))), spawn: Arc::new(lw::System), proc_root: dir.join("proc") };
        let mut a = LinuxWinApp::with_rig(&cx, rg);
        assert_eq!(a.phase, if make == 1 { Phase::Pick } else { Phase::NotInstalled });
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
            let g = draw(&mut a, &cx, w, h, R::new(0.0, 0.0, w as f32, h as f32));
            if make == 0 {
                assert!(g.hits.iter().any(|x| x.label == "Install Linux windows"), "an administrator can install: {w}x{h}");
            } else {
                assert!(g.hits.iter().any(|x| x.label == "Start"), "{w}x{h}");
            }
            for hit in &g.hits {
                assert!(hit.rect.x >= -0.01 && hit.rect.y >= -0.01 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{} at {w}x{h}: {:?}", hit.label, hit.rect);
            }
        }
        a.phase = Phase::Ended("test".into());
        let g = draw(&mut a, &cx, 360, 740, R::new(0.0, 0.0, 360.0, 740.0));
        assert!(g.hits.iter().any(|x| x.label == "Back to the list"));
        let _ = iyakki_agent::rootfs::remove_tree(&dir);
    }
}

#[test]
fn only_the_full_command_line_permission_may_run_it_and_a_networked_sandbox_needs_all_hosts() {
    let mut cx = cx_with_admin_store("lw-gate");
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Restricted;
    let dir = tmp("lw-gate");
    make_la(&dir, false);
    let d2 = dir.clone();
    let mk = move || Rig { la: { let d = d2.clone(); Arc::new(move || Ok(LinuxApps::new(&d, "root", Path::new("/usr/bin/iyakki"), Arc::new(FakeRunner::new(&[]))))) }, spawn: Arc::new(lw::System), proc_root: "/nonexistent".into() };
    let mut a = LinuxWinApp::with_rig(&cx, mk());
    assert!(matches!(a.phase, Phase::NoAccess(_)));
    a.handle(&Event::Tab(1), &cx);
    assert!(matches!(a.phase, Phase::NoAccess(_)), "no start");
    let g = draw(&mut a, &cx, 800, 480, R::new(0.0, 0.0, 800.0, 480.0));
    assert!(!g.hits.iter().any(|h| h.label == "Start"));
    // full permission but not every host, sandbox network on
    let mut cx2 = cx_with_admin_store("lw-gate2");
    cx2.session.policy.net_hosts = vec!["example.org".into()];
    make_la(&dir, true);
    let mut b = LinuxWinApp::with_rig(&cx2, mk());
    assert_eq!(b.phase, Phase::Pick);
    b.handle(&Event::Tab(1), &cx2);
    assert_eq!(b.phase, Phase::Pick, "refused");
    assert!(b.note.as_ref().is_some_and(|n| n.0.contains("any server") && !n.1), "{:?}", b.note);
    let _ = iyakki_agent::rootfs::remove_tree(&dir);
}

#[test]
fn a_display_that_cannot_start_ends_with_the_reason() {
    struct Dies;
    impl Spawn for Dies {
        fn spawn(&self, _a: &[String], _c: &Path, log: &Path) -> Result<Box<dyn Proc>, String> {
            std::fs::write(log, "Xvnc: cannot open display\n").unwrap();
            struct P;
            impl Proc for P {
                fn pid(&self) -> Option<u32> {
                    Some(1)
                }
                fn alive(&mut self) -> bool {
                    false
                }
                fn kill(&mut self) {}
            }
            Ok(Box::new(P))
        }
    }
    let cx = cx_with_admin_store("lw-dies");
    let dir = tmp("lw-dies");
    make_la(&dir, false);
    let d2 = dir.clone();
    let rg = Rig { la: Arc::new(move || Ok(LinuxApps::new(&d2, "root", Path::new("/usr/bin/iyakki"), Arc::new(FakeRunner::new(&[]))))), spawn: Arc::new(Dies), proc_root: dir.join("proc") };
    let mut a = LinuxWinApp::with_rig(&cx, rg);
    a.handle(&Event::Tab(1), &cx);
    wait_for("the end", || {
        a.handle(&Event::Tick, &cx);
        matches!(a.phase, Phase::Ended(_))
    });
    match &a.phase {
        Phase::Ended(m) => assert!(m.contains("cannot open display"), "{m}"),
        _ => unreachable!(),
    }
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    let _ = iyakki_agent::rootfs::remove_tree(&dir);
}

#[test]
fn the_tile_is_on_the_grid_for_operators_and_admins_only() {
    use iyakki_sec::policy::{App as PApp, Role};
    assert!(Role::Admin.template().apps.contains(&PApp::LinuxWin));
    assert!(Role::Operator.template().apps.contains(&PApp::LinuxWin));
    assert!(!Role::User.template().apps.contains(&PApp::LinuxWin));
    assert!(!Role::Guest.template().apps.contains(&PApp::LinuxWin));
    assert_eq!(PApp::LinuxWin.label(), "LINUX WINDOW");
    assert!(crate::shell::tiles_for_test().iter().any(|(a, l)| *a == PApp::LinuxWin && *l == "LINUX WINDOW"));
}
