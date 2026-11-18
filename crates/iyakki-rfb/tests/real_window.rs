//! The real run of Linux windows (ignored: needs the network, user namespaces and the `iyakki` program). It lives in this crate's tests
//! because it needs both the sandbox (`iyakki-agent`) and the RFB client.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::linuxapps::LinuxApps;
use iyakki_agent::linuxwin as lw;

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(20), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn chars_keys(c: char) -> Vec<(bool, u32)> {
    let s = iyakki_rfb::keysyms::from_char(c);
    vec![(true, s), (false, s)]
}

/// A REAL run: downloads Debian 12, installs the display packages with apt in a sandbox as a normal user, starts xterm in a window and
/// connects the RFB client to the real Xvnc. Needs the network, a kernel with user namespaces and the `iyakki` program
/// (`IYAKKI_EXE`, for example `target/debug/iyakki`). Everything it makes is deleted at the end.
/// `IYAKKI_EXE=... TMPDIR=/tmp/iyakki-test/rt-lw cargo test -p iyakki-rfb --test real_window real_window::real -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn real_linux_window_runs_xterm_and_the_client_sees_it() {
    let exe = PathBuf::from(std::env::var("IYAKKI_EXE").expect("IYAKKI_EXE"));
    let base = std::env::var("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    // LW_DATA: a data folder whose sandbox `gui` was prepared earlier (kept); otherwise a new one that is deleted at the end
    let keep = std::env::var("LW_DATA").ok().map(PathBuf::from);
    let data = keep.clone().unwrap_or_else(|| base.join(format!("lw-real-{}", std::process::id())));
    std::fs::create_dir_all(&data).unwrap();
    let la = LinuxApps::new(&data, "dev", &exe, Arc::new(iyakki_agent::wsexec::System));
    let st = iyakki_agent::iyakki_proxy::https::UreqStreamer::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sink: iyakki_agent::wsexec::Sink = Arc::new(Mutex::new(String::new()));
        let line = lw::install(&la, &st, Some(sink.clone()), &mut |l| println!("{l}")).unwrap_or_else(|e| panic!("install: {e}\n{}", sink.lock().unwrap().chars().rev().take(3000).collect::<String>().chars().rev().collect::<String>()));
        println!("{line}");
        let progs = lw::programs(&la.rootfs_of(lw::SANDBOX));
        println!("programs: {:?}", progs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>());
        let xterm = progs.iter().find(|p| p.id == "xterm" || p.id == "debian-xterm").expect("xterm in the launcher").clone();
        let xterm = match std::env::var("LW_SH") {
            // debugging: run this shell line (inside the sandbox) instead of xterm; its output goes to session.log
            Ok(line) => lw::Program { id: "debug".into(), name: "debug".into(), exec: vec!["sh".into(), "-c".into(), line] },
            Err(_) => xterm,
        };
        let mut win = lw::Window::start(&la, &lw::System, &xterm, 800, 500).unwrap();
        win.wait_socket(Duration::from_secs(30), &std::thread::sleep).unwrap_or_else(|e| panic!("{e}"));
        let link = iyakki_rfb::Link::connect_unix(&win.socket(), Duration::from_secs(10)).unwrap();
        wait_for("rfb ready", || link.with(|s| s.ready()));
        println!("screen {}x{} name {}", link.with(|s| s.fb().w), link.with(|s| s.fb().h), link.with(|s| s.name().to_string()));
        assert_eq!(link.with(|s| (s.fb().w, s.fb().h)), (800, 500));
        // some pixel of the xterm window is not black
        wait_for("a picture", || {
            link.with(|s| {
                s.take_events();
                s.fb().px.iter().any(|p| *p != 0)
            })
        });
        // resize through ExtendedDesktopSize
        wait_for("resize ability", || link.with(|s| {
            s.take_events();
            s.can_resize()
        }));
        assert!(link.with(|s| s.request_resize(640, 400)));
        wait_for("resized", || link.with(|s| {
            s.take_events();
            (s.fb().w, s.fb().h) == (640, 400)
        }));
        // type into the xterm: the screen changes
        let before: u64 = link.with(|s| s.fb().px.iter().map(|p| *p as u64).sum());
        for c in "echo hello".chars() {
            for (down, sym) in chars_keys(c) {
                link.with(|s| s.key_event(down, sym));
            }
        }
        wait_for("the screen changed after typing", || link.with(|s| {
            s.take_events();
            s.fb().px.iter().map(|p| *p as u64).sum::<u64>() != before
        }));
        let u = lw::usage_of(Path::new("/proc"), win.pid().unwrap());
        println!("usage: {} processes, {} MB, {} ticks", u.procs, u.rss_kb / 1024, u.ticks);
        assert!(u.procs >= 3, "Xvnc, the window manager and xterm run below the started process");
        let pid = win.pid().unwrap();
        drop(link);
        win.stop();
        std::thread::sleep(Duration::from_millis(500));
        let left = lw::usage_of(Path::new("/proc"), pid);
        assert_eq!(left.procs, 0, "nothing of the sandbox is left");
    }));
    if keep.is_none() {
        let _ = iyakki_agent::rootfs::remove_tree(&data);
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
