//! SYSTEM: the dev cluster's health row and its log (a stand-in `k3s`, no real cluster).

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::k3sdev::{Machine, Svc};
use iyakki_sec::policy::Role;
use std::sync::Arc;

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    cx.services = services_in(&temp_dir(&format!("{tag}-svc")), Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

/// Installed by hand: the state file, a stand-in program that stays up and answers `kubectl`, and a log.
const STAND_IN: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  echo \"time=now level=info msg=started\"\n  echo \"time=now level=error msg=etcd slow\"\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nkubectl) case \"$*\" in *kubeletVersion*) echo v1.31.1+k3s1;; *\"get pods\"*) printf 'kube-system a 1/1 Running 0 1m\\nkube-system b 1/1 Running 0 1m\\ndefault c 0/1 Pending 0 1m\\n';; *) echo True;; esac;;\nesac\n";

fn install_by_hand(cx: &AppCx) {
    use std::os::unix::fs::PermissionsExt;
    let k = &cx.services.k3sdev;
    let l = k.layout();
    std::fs::create_dir_all(l.root.join("bin")).unwrap();
    std::fs::write(l.bin(), STAND_IN).unwrap();
    std::fs::set_permissions(l.bin(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(l.state(), "{\"backend\":\"native\",\"version\":\"v1.31.1+k3s1\",\"autostart\":true}").unwrap();
    let m = Machine { k3s_yaml: l.root.join("nope/k3s.yaml"), lib_dir: l.root.join("nope/lib"), bins: vec![], units: vec![], proc_dir: l.root.join("nope/proc") };
    k.assume(true, true, false, false, m, Duration::from_millis(10), Duration::from_millis(20));
    k.assume_connected(None);
}

fn frame(a: &mut SystemApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, cx, 0);
    g
}

fn roots(tag: &str) -> SysRoots {
    let d = temp_dir(&format!("{tag}-sys"));
    let (proc, sys) = (d.join("proc"), d.join("sys"));
    std::fs::create_dir_all(proc.join("net")).unwrap();
    std::fs::create_dir_all(&sys).unwrap();
    std::fs::write(proc.join("stat"), "cpu  100 0 0 800 0 0 0 0 0 0\ncpu0 100 0 0 800 0 0 0 0 0 0\n").unwrap();
    std::fs::write(proc.join("meminfo"), "MemTotal: 8000000 kB\nMemAvailable: 6000000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n").unwrap();
    std::fs::write(proc.join("loadavg"), "0.5 0.4 0.3 1/200 9\n").unwrap();
    std::fs::write(proc.join("uptime"), "100.0 1.0\n").unwrap();
    std::fs::write(proc.join("net/dev"), "h\nh\n  eth0: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n").unwrap();
    std::fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
    SysRoots { proc, sys, passwd: d.join("passwd") }
}

fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..1500 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

#[test]
fn the_overview_shows_the_clusters_state_and_nothing_when_none_is_installed() {
    let cx = admin("sysc1");
    let mut a = SystemApp::with_roots(roots("sysc1"), temp_dir("sysc1-data"));
    a.refresh(&cx);
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(93)), "no cluster, no row, no button");
    a.handle(&Event::Key(Key::Char('l')), &cx);
    assert!(a.cluster_log.is_none());

    install_by_hand(&cx);
    let k = cx.services.k3sdev.clone();
    // installed but stopped: the row says so and the log can be read (it is empty)
    let h = k.health();
    assert_eq!((h.installed, h.state.as_str(), h.autostart), (true, "stopped", true));
    for (w, hh) in [(1280u32, 720u32), (1024, 600)] {
        let g = frame(&mut a, &cx, w, hh);
        let area = R::new(0.0, 72.0, w as f32, hh as f32 - 72.0);
        let b = g.hits.iter().find(|h| h.ev == Event::Tab(93)).unwrap_or_else(|| panic!("{w}x{hh}: View logs"));
        assert!(b.rect.x >= area.x && b.rect.y >= area.y && b.rect.x + b.rect.w <= area.x + area.w + 0.5 && b.rect.y + b.rect.h <= area.y + area.h + 0.5, "{w}x{hh}: {:?} outside {area:?}", b.rect);
        for o in g.hits.iter().filter(|o| o.ev != b.ev && !matches!(o.ev, Event::Tab(0..=3))) {
            let (x, y) = (b.rect.x.max(o.rect.x), b.rect.y.max(o.rect.y));
            assert!(!(x < (b.rect.x + b.rect.w).min(o.rect.x + o.rect.w) - 0.5 && y < (b.rect.y + b.rect.h).min(o.rect.y + o.rect.h) - 0.5), "{w}x{hh}: View logs covers {:?}", o.ev);
        }
    }

    // running: node Ready, the pods counted, the data folder measured
    k.start("root").unwrap();
    assert!(wait_for(|| k.state() == Svc::Running), "{:?}", k.state());
    let h = k.refresh_health();
    assert_eq!((h.state.as_str(), h.version.as_str(), h.node_ready, h.pods), ("running", "v1.31.1+k3s1", Some(true), Some((2, 3))), "{h:?}");
    assert!(h.data_bytes.is_some() && h.disk_pct.is_some());
    assert_eq!(h.line(), "running · v1.31.1+k3s1 · node Ready · 2/3 pods");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(93)));

    // View logs: the tail of the service log, selectable text; Esc closes
    a.handle(&Event::Key(Key::Char('l')), &cx);
    let l = a.cluster_log.as_ref().expect("the log is open");
    let text: Vec<&str> = l.lines.iter().map(|x| x.0.as_str()).collect();
    assert!(text.iter().any(|t| t.contains("msg=started")) && text.iter().any(|t| t.contains("etcd slow")), "{text:?}");
    assert!(l.lines.iter().any(|x| x.0.contains("etcd slow") && x.1 == ERR), "errors are marked");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(92)), "a Close button");
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Area(_))), "the text can be selected with the mouse");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.cluster_log.is_none());
    // the same through the button
    a.handle(&Event::Tab(93), &cx);
    assert!(a.cluster_log.is_some());
    a.handle(&Event::Tab(92), &cx);
    assert!(a.cluster_log.is_none());
    assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("devcluster.logs"));

    // an ordinary account sees the state but has no log button and cannot open the log
    let mut user = cx.clone();
    user.session.role = Role::User;
    user.session.policy = Role::User.template();
    let mut b = SystemApp::with_roots(roots("sysc1u"), temp_dir("sysc1u-data"));
    b.refresh(&user);
    let g = frame(&mut b, &user, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(93)));
    b.handle(&Event::Key(Key::Char('l')), &user);
    assert!(b.cluster_log.is_none());
    k.stop("root").unwrap();
}

#[test]
fn a_failed_cluster_shows_its_reason_and_raises_an_alert() {
    let cx = admin("sysc2");
    install_by_hand(&cx);
    let k = cx.services.k3sdev.clone();
    std::fs::write(k.layout().bin(), "#!/bin/sh\necho 'boom: cannot bind 6443'\nexit 3\n").unwrap();
    k.start("root").unwrap();
    assert!(wait_for(|| matches!(k.state(), Svc::Failed(_))), "{:?}", k.state());
    let h = k.health();
    assert_eq!(h.state, "failed");
    assert!(h.failed_reason.contains("boom: cannot bind 6443") && h.last_restart_reason.contains("boom"), "{h:?}");
    let mut a = SystemApp::with_roots(roots("sysc2"), temp_dir("sysc2-data"));
    a.refresh(&cx);
    let g = frame(&mut a, &cx, 1024, 600);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(93)));
    // the failure is also an alert for the administrators' INBOX
    let alerts = k.take_alerts();
    assert!(alerts.iter().any(|a| a.title_body().0.contains("stopped and was restarted") && a.title_body().1.contains("boom")), "{alerts:?}");
}
