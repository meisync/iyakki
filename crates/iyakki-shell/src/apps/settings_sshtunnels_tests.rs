//! Settings -> Network -> SSH tunnels: the list, Add / Edit / Enable / Disable / Delete (asks first) / Test, the trust form for a new
//! host key, and the page drawn at four screen sizes. The client is a fake: no network, no SSH server.

use std::collections::HashMap;
use std::sync::Mutex;

use super::sshtun_page::TunForm;
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::gitkeys::{GitKeys, MemStore, Session};
use iyakki_agent::ssh::HostKey;
use iyakki_agent::sshtunnel::Ports;
use iyakki_agent::sshtunnel_mgr::{FakePorts, FakeRunner, Manager, Plan, State};

struct Rig {
    a: SettingsApp,
    cx: AppCx,
    mgr: Arc<Manager>,
    runner: Arc<FakeRunner>,
    store: Arc<MemStore>,
    keys: Arc<GitKeys>,
    shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>>,
}

fn host_key() -> HostKey {
    HostKey { pattern: String::new(), kind: "ssh-ed25519".into(), b64: iyakki_agent::story::base64_encode(b"fake-host-key-page") }
}

fn rig(tag: &str) -> Rig {
    let cx = cx_with_admin_store(tag);
    let dir = temp_dir(&format!("{tag}-tun"));
    let store = Arc::new(MemStore::default());
    let audit: iyakki_agent::gitkeys::AuditFn = Arc::new(|_, _| {});
    let keys = Arc::new(GitKeys::new(&dir, store.clone(), audit.clone(), Arc::new(Session::default())));
    keys.generate("work", "fake@example.invalid").unwrap();
    let shown: Arc<Mutex<HashMap<(String, u16), Vec<HostKey>>>> = Arc::new(Mutex::new(HashMap::new()));
    let s2 = shown.clone();
    let probe: iyakki_agent::gitjobs::Probe = Arc::new(move |h, p| s2.lock().unwrap().get(&(h.to_string(), p)).cloned().ok_or_else(|| "no host key".to_string()));
    let runner = Arc::new(FakeRunner::default());
    let ports: Arc<dyn Ports> = Arc::new(FakePorts::default());
    let mgr = Arc::new(Manager::new(&dir, keys.clone(), probe, runner.clone(), ports, audit).with_pause(Arc::new(|_| {})));
    let mut a = SettingsApp::new(&cx);
    a.tun.mgr = Some(mgr.clone());
    a.tun.now = Some(1000);
    for _ in 0..60 {
        if a.tab() == Tab::SshTunnels {
            break;
        }
        a.handle(&Event::Key(Key::Right), &cx);
    }
    assert_eq!(a.tab(), Tab::SshTunnels);
    Rig { a, cx, mgr, runner, store, keys, shown }
}

fn press(r: &mut Rig, k: Key) {
    r.a.handle(&Event::Key(k), &r.cx);
}

fn status(r: &Rig) -> String {
    r.a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(r: &mut Rig, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    r.a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &r.cx, 0);
    g
}

/// Fills the open form and submits it the way Enter would.
fn submit(r: &mut Rig, set: &[(usize, &str)], choices: &[(usize, usize)]) -> Result<String, String> {
    let Some(Modal::Form(FormKind::SshTun(k), mut f)) = r.a.modal.take() else { panic!("no tunnel form is open") };
    for (i, v) in set {
        f.set_value(*i, v);
    }
    for (i, c) in choices {
        f.set_choice(*i, *c);
    }
    r.a.tun_submit(&r.cx, &k, &mut f)
}

fn add_wiki(r: &mut Rig) -> String {
    press(r, Key::Char('a'));
    let res = submit(r, &[(0, "Wiki"), (1, "bastion.example.invalid"), (3, "alice"), (6, "wiki.example.invalid"), (7, "8080"), (8, "8123")], &[(9, 1)]);
    res.unwrap()
}

#[test]
fn the_page_is_for_administrators_under_network() {
    let r = rig("tun-tab");
    assert_eq!(r.a.tab(), Tab::SshTunnels);
    let cx = {
        let mut c = cx_with_admin_store("tun-guest");
        c.session.policy = iyakki_sec::policy::Role::Guest.template();
        c
    };
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::SshTunnels));
}

#[test]
fn add_edit_enable_disable_and_delete_after_asking() {
    let mut r = rig("tun-crud");
    let g = draw(&mut r, 1280, 800);
    for b in ["Add", "Edit", "On / Off", "Delete", "Test", "Retry", "Copy URL"] {
        assert!(g.hits.iter().any(|h| h.label == b), "{b}");
    }
    let said = add_wiki(&mut r);
    let t = r.mgr.get("t1").unwrap();
    assert_eq!((t.name.as_str(), t.local_port, t.enabled, t.key.as_str()), ("Wiki", 8123, false, "work"));
    assert!(said.contains("http://127.0.0.1:8123/") && said.contains("disabled"), "{said}");
    // a bad host is refused in words and nothing is saved
    press(&mut r, Key::Char('a'));
    let e = submit(&mut r, &[(0, "Bad"), (1, "-oProxyCommand=id"), (3, "alice"), (6, "x.example.invalid"), (7, "80")], &[(9, 1)]).unwrap_err();
    assert!(e.contains("not a host name"), "{e}");
    assert_eq!(r.mgr.tunnels().len(), 1);
    // a port that is taken is reported
    press(&mut r, Key::Char('a'));
    let e = submit(&mut r, &[(0, "Same"), (1, "bastion.example.invalid"), (3, "alice"), (6, "x.example.invalid"), (7, "80"), (8, "8123")], &[(9, 1)]).unwrap_err();
    assert!(e.contains("already used by the tunnel Wiki"), "{e}");
    // edit keeps the id and the enabled flag
    press(&mut r, Key::Char('e'));
    assert!(matches!(&r.a.modal, Some(Modal::Form(FormKind::SshTun(TunForm::Edit(Some(id))), _)) if id == "t1"));
    submit(&mut r, &[(7, "9090")], &[]).unwrap();
    assert_eq!(r.mgr.get("t1").unwrap().target_port, 9090);
    // enable: starts; the row shows it
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![host_key()]);
    r.keys.known_hosts().trust("bastion.example.invalid", 22, &[host_key()]).unwrap();
    press(&mut r, Key::Char('x'));
    assert!(status(&r).contains("is enabled"), "{}", status(&r));
    r.mgr.tick(1001);
    assert!(matches!(r.mgr.status("t1").state, State::Connected { .. }));
    let g = draw(&mut r, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Wiki"));
    // disable stops it but keeps it
    press(&mut r, Key::Char('x'));
    assert!(status(&r).contains("is disabled") && r.runner.alive() == 0 && r.mgr.tunnels().len() == 1, "{}", status(&r));
    // delete asks first: Esc keeps it, y deletes it
    press(&mut r, Key::Char('d'));
    assert!(matches!(&r.a.modal, Some(Modal::Confirm(ConfirmKind::SshTunnelDelete(id), _)) if id == "t1"));
    press(&mut r, Key::Esc);
    assert_eq!(r.mgr.tunnels().len(), 1);
    press(&mut r, Key::Char('d'));
    press(&mut r, Key::Char('y'));
    assert!(r.mgr.tunnels().is_empty(), "{}", status(&r));
    assert!(status(&r).contains("Deleted"));
}

#[test]
fn a_new_key_is_made_for_the_tunnel_and_goes_with_it() {
    let mut r = rig("tun-key");
    press(&mut r, Key::Char('a'));
    let said = submit(&mut r, &[(0, "Docs"), (1, "bastion.example.invalid"), (3, "alice"), (6, "d.example.invalid"), (7, "80")], &[(9, 0)]).unwrap();
    assert!(said.contains("public half"), "{said}");
    assert!(r.store.map.lock().unwrap().contains_key("ssh.tunnel-docs"));
    assert_eq!(r.mgr.get("t1").unwrap().local_port, 40000, "an empty port gets a free one");
    press(&mut r, Key::Char('d'));
    assert!(matches!(&r.a.modal, Some(Modal::Confirm(_, t)) if t.contains("key is deleted")));
    press(&mut r, Key::Char('y'));
    assert!(!r.store.map.lock().unwrap().contains_key("ssh.tunnel-docs"));
}

#[test]
fn test_shows_the_fingerprint_and_trust_needs_a_yes() {
    let mut r = rig("tun-trust");
    add_wiki(&mut r);
    let k = host_key();
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    press(&mut r, Key::Char('T'));
    for _ in 0..200 {
        r.a.handle(&Event::Tick, &r.cx);
        if r.a.modal.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(&r.a.modal, Some(Modal::Form(FormKind::SshTun(TunForm::Trust(h, 22)), f)) if h == "bastion.example.invalid" && f.value(0) == k.fingerprint()), "the fingerprint is shown");
    // the default answer is No
    let e = submit(&mut r, &[], &[]).unwrap_err();
    assert!(e.contains("Nothing was trusted"));
    assert!(r.keys.known_hosts().for_host("bastion.example.invalid", 22).is_empty());
}

#[test]
fn trusting_with_the_matching_fingerprint_lets_test_connect() {
    let mut r = rig("tun-trust2");
    add_wiki(&mut r);
    let k = host_key();
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![k.clone()]);
    r.a.modal = Some(Modal::Form(FormKind::SshTun(TunForm::Trust("bastion.example.invalid".into(), 22)), Form::new("t").text("f", &k.fingerprint(), "").choice("c", &["No", "Yes"], 0)));
    assert!(submit(&mut r, &[(0, "SHA256:wrong")], &[(1, 1)]).unwrap_err().contains("nothing was trusted"));
    r.a.modal = Some(Modal::Form(FormKind::SshTun(TunForm::Trust("bastion.example.invalid".into(), 22)), Form::new("t").text("f", &k.fingerprint(), "").choice("c", &["No", "Yes"], 0)));
    assert!(submit(&mut r, &[], &[(1, 1)]).unwrap().contains("trusted"));
    r.runner.plan(Plan::Connect);
    press(&mut r, Key::Char('T'));
    for _ in 0..200 {
        r.a.handle(&Event::Tick, &r.cx);
        if status(&r).contains("Connected") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(status(&r).contains("Connected") && status(&r).contains("http://127.0.0.1:8123/"), "{}", status(&r));
}

#[test]
fn the_page_draws_inside_the_window_at_the_four_sizes() {
    let mut r = rig("tun-draw");
    r.shown.lock().unwrap().insert(("bastion.example.invalid".into(), 22), vec![host_key()]);
    r.keys.known_hosts().trust("bastion.example.invalid", 22, &[host_key()]).unwrap();
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        let g = draw(&mut r, w, h);
        assert!(g.hits.iter().any(|x| x.label == "Add"), "{w}x{h}: empty list still has Add");
    }
    for i in 0..9 {
        r.mgr.save(iyakki_agent::sshtunnel::Tunnel { name: format!("T{i}"), host: "bastion.example.invalid".into(), port: 22, user: "alice".into(), key: "work".into(), local_port: 9000 + i, target_host: "x.example.invalid".into(), target_port: 80, enabled: i % 2 == 0, ..Default::default() }, false).unwrap();
    }
    r.runner.plan(Plan::Fail("Permission denied (publickey).".into()));
    r.mgr.tick(1000);
    r.mgr.tick(1001);
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        for sel in [0usize, 4, 8] {
            r.a.sel[r.a.tab] = sel;
            let g = draw(&mut r, w, h);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                let q = hit.rect;
                assert!(q.x >= -0.5 && q.y >= -0.5 && q.x + q.w <= w as f32 + 0.5 && q.y + q.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, q);
            }
            assert!(g.hits.iter().any(|x| x.label == "Delete"), "{w}x{h}");
        }
    }
}
