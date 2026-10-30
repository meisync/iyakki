//! Settings -> Network -> VPN: import from the clipboard and from a file, the keys never shown or kept outside the vault, enable and
//! disable with the exact undo, mode and filters, delete after asking, keyboard and mouse, and the page drawn at four screen sizes.
//! The system layer is a fake; every key is FAKE (32 bytes of one repeated value).

use std::sync::Arc;

use super::vpn_page::VpnForm;
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::gitkeys::{MemStore, SecretStore};
use iyakki_agent::vpn_mgr::{Manager, State};
use iyakki_agent::vpn_sys::{FakeSys, Sys};

struct Rig {
    a: SettingsApp,
    cx: AppCx,
    mgr: Arc<Manager>,
    sys: Arc<FakeSys>,
    vault: Arc<MemStore>,
}

fn key(n: u8) -> String {
    iyakki_vpn::b64::encode(&[n; 32])
}

fn conf() -> String {
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.8.0.2/24\nDNS = 10.8.0.1\n\n[Peer]\nPublicKey = {}\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0, ::/0\nPersistentKeepalive = 25\n",
        key(0x11),
        key(0x22)
    )
}

fn rig(tag: &str) -> Rig {
    let cx = cx_with_admin_store(tag);
    let dir = temp_dir(&format!("{tag}-vpn"));
    let sys = Arc::new(FakeSys::new());
    sys.name("vpn.example.com", &["203.0.113.9"]);
    let vault = Arc::new(MemStore::default());
    let mgr = Arc::new(Manager::new(&dir, vault.clone(), sys.clone(), Arc::new(|_, _| {})));
    let mut a = SettingsApp::new(&cx);
    a.vpn.mgr = Some(mgr.clone());
    a.vpn.now = Some(1_000_000);
    for _ in 0..60 {
        if a.tab() == Tab::Vpn {
            break;
        }
        a.handle(&Event::Key(Key::Right), &cx);
    }
    assert_eq!(a.tab(), Tab::Vpn);
    Rig { a, cx, mgr, sys, vault }
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

fn submit(r: &mut Rig, set: &[(usize, &str)], choices: &[(usize, usize)]) -> Result<String, String> {
    let Some(Modal::Form(FormKind::Vpn(k), mut f)) = r.a.modal.take() else { panic!("no VPN form is open") };
    for (i, v) in set {
        f.set_value(*i, v);
    }
    for (i, c) in choices {
        f.set_choice(*i, *c);
    }
    r.a.vpn_submit(&r.cx, &k, &mut f)
}

fn click(r: &mut Rig, label: &str) {
    let g = draw(r, 1280, 800);
    let ev = g.hits.iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no control {label}")).ev.clone();
    r.a.handle(&ev, &r.cx);
}

fn import_clip(r: &mut Rig, name: &str) -> String {
    crate::ui::clipboard::copy(&conf());
    press(r, Key::Char('p'));
    submit(r, &[(0, name)], &[]).unwrap()
}

#[test]
fn the_page_is_for_administrators_under_network() {
    let r = rig("vpn-tab");
    assert_eq!(r.a.tab(), Tab::Vpn);
    assert!(Tab::ORDER.contains(&Tab::Vpn));
    let cx = {
        let mut c = cx_with_admin_store("vpn-guest");
        c.session.policy = iyakki_sec::policy::Role::Guest.template();
        c
    };
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Vpn));
}

#[test]
fn import_from_the_clipboard_keeps_the_keys_in_the_vault_and_off_the_screen_and_the_history() {
    let mut r = rig("vpn-clip");
    let said = import_clip(&mut r, "Office");
    assert!(said.contains("Imported") && said.contains("disabled") && said.contains("removed from the clipboard history"), "{said}");
    let p = r.mgr.profiles();
    assert_eq!((p.len(), p[0].name.as_str()), (1, "Office"));
    assert!(r.vault.names().unwrap().iter().any(|n| n.ends_with(".key")));
    assert!(!crate::ui::clipboard::history().iter().any(|h| h.contains(&key(0x11))), "the key is not kept in the copy history");
    assert!(!said.contains(&key(0x11)) && !format!("{:?}", r.mgr.rows()).contains(&key(0x11)));
    // the form never held the text, only the name
    press(&mut r, Key::Char('p'));
    crate::ui::clipboard::copy(&format!("PrivateKey = {}", key(0x44)));
    press(&mut r, Key::Char('p'));
    assert!(matches!(&r.a.modal, Some(Modal::Form(FormKind::Vpn(VpnForm::ImportClip), f)) if f.fields.len() == 1));
    // not a profile: refused in words, nothing kept, no value echoed
    let e = submit(&mut r, &[(0, "Nope")], &[]).unwrap_err();
    assert!(!e.contains(&key(0x44)), "{e}");
    assert_eq!(r.mgr.profiles().len(), 1);
}

#[test]
fn import_from_a_file_and_a_bad_file() {
    let mut r = rig("vpn-file");
    let dir = temp_dir("vpn-file-src");
    let good = dir.join("office.conf");
    std::fs::write(&good, conf()).unwrap();
    press(&mut r, Key::Char('i'));
    assert!(matches!(&r.a.modal, Some(Modal::Form(FormKind::Vpn(VpnForm::ImportFile), f)) if f.fields.len() == 2));
    let said = submit(&mut r, &[(0, "Office"), (1, &good.to_string_lossy())], &[]).unwrap();
    assert!(said.contains("Imported"));
    // a script directive is refused with its name and not its value; an empty path and a missing file are asked about
    let bad = dir.join("bad.conf");
    std::fs::write(&bad, conf().replace("[Peer]", "PostUp = /bin/sh -c evil\n[Peer]")).unwrap();
    press(&mut r, Key::Char('i'));
    let e = submit(&mut r, &[(0, "Bad"), (1, &bad.to_string_lossy())], &[]).unwrap_err();
    assert!(e.contains("PostUp") && !e.contains("evil"), "{e}");
    press(&mut r, Key::Char('i'));
    assert!(submit(&mut r, &[(0, "Empty")], &[]).unwrap_err().contains("Choose the profile file"));
    press(&mut r, Key::Char('i'));
    assert!(submit(&mut r, &[(0, "Gone"), (1, "/no/such/file.conf")], &[]).unwrap_err().contains("cannot be read"));
    assert_eq!(r.mgr.profiles().len(), 1);
}

#[test]
fn enable_disable_and_delete_with_the_keyboard_and_the_mouse() {
    let mut r = rig("vpn-life");
    import_clip(&mut r, "Office");
    let g = draw(&mut r, 1280, 800);
    for b in ["Import file", "Import clipboard", "Enable / Disable", "Mode and filters", "Delete", "Retry"] {
        assert!(g.hits.iter().any(|h| h.label == b), "{b}");
    }
    assert!(g.hits.iter().any(|h| h.label == "Office"), "the row is clickable");
    // the keyboard enables, the mouse disables
    press(&mut r, Key::Char('x'));
    assert!(status(&r).contains("is enabled"), "{}", status(&r));
    assert!(r.sys.st.lock().unwrap().links.contains("iyvpn0"));
    r.sys.st.lock().unwrap().handshake = Some(1_000_000);
    r.mgr.tick(1_000_001);
    assert!(matches!(r.mgr.status("v1").state, State::Connected { .. }));
    click(&mut r, "Enable / Disable");
    assert!(status(&r).contains("is disabled") && r.sys.clean(), "{}", status(&r));
    // delete asks first: Esc keeps it, y deletes it and empties the vault
    click(&mut r, "Delete");
    assert!(matches!(&r.a.modal, Some(Modal::Confirm(ConfirmKind::VpnDelete(id), _)) if id == "v1"));
    press(&mut r, Key::Esc);
    assert_eq!(r.mgr.profiles().len(), 1);
    press(&mut r, Key::Char('d'));
    press(&mut r, Key::Char('y'));
    assert!(r.mgr.profiles().is_empty() && r.vault.names().unwrap().is_empty(), "{}", status(&r));
    assert!(status(&r).contains("Deleted"));
    // with nothing selected the keys say what to do
    press(&mut r, Key::Char('x'));
    assert!(status(&r).contains("Choose a VPN profile"));
}

#[test]
fn a_failure_to_connect_is_shown_in_words() {
    let mut r = rig("vpn-fail");
    import_clip(&mut r, "Office");
    r.sys.fail("link add");
    press(&mut r, Key::Char('x'));
    assert!(status(&r).contains("not connected") && status(&r).contains("WireGuard module"), "{}", status(&r));
    let g = draw(&mut r, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Office"));
    r.sys.clear_failures();
    press(&mut r, Key::Char('r'));
    assert!(matches!(r.mgr.status("v1").state, State::Connecting));
}

#[test]
fn mode_and_filters_are_validated_and_saved() {
    let mut r = rig("vpn-mode");
    import_clip(&mut r, "Office");
    press(&mut r, Key::Char('m'));
    // FULL with the kill switch on
    let said = submit(&mut r, &[], &[(0, 0), (1, 1)]).unwrap();
    assert!(said.contains("kill switch on"), "{said}");
    assert!(r.mgr.get("v1").unwrap().settings.kill_switch);
    // filters in FULL mode are refused in words
    press(&mut r, Key::Char('m'));
    assert!(submit(&mut r, &[(3, "example.org")], &[(0, 0)]).unwrap_err().contains("HYBRID"));
    // a catch-all is refused when parsed
    press(&mut r, Key::Char('m'));
    assert!(submit(&mut r, &[(3, "*.com")], &[(0, 1)]).is_err());
    // HYBRID needs a rule that sends something through
    press(&mut r, Key::Char('m'));
    assert!(submit(&mut r, &[(3, "!public.corp.example.com")], &[(0, 1)]).is_err());
    press(&mut r, Key::Char('m'));
    let said = submit(&mut r, &[(3, "*.corp.example.com, 203.0.113.0/24 https://example.org/app")], &[(0, 1)]).unwrap();
    assert!(said.contains("HYBRID") && said.contains("3 filter rules"), "{said}");
    let s = r.mgr.get("v1").unwrap().settings;
    assert!(s.fail_closed && !s.kill_switch && s.filters.len() == 3);
    // the form shows the saved filters again
    press(&mut r, Key::Char('m'));
    assert!(matches!(&r.a.modal, Some(Modal::Form(_, f)) if f.value(3).contains("corp.example.com") && f.choice_idx(0) == 1));
    r.a.modal = None;
    // enabling a HYBRID profile turns the guard on first
    press(&mut r, Key::Char('x'));
    assert!(r.sys.guard_present());
    let g = draw(&mut r, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Office"));
    press(&mut r, Key::Char('x'));
    assert!(r.sys.clean());
}

#[test]
fn the_page_draws_inside_the_window_at_the_four_sizes() {
    let mut r = rig("vpn-draw");
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        let g = draw(&mut r, w, h);
        assert!(g.hits.iter().any(|x| x.label == "Import file"), "{w}x{h}: an empty list still has Import");
    }
    for i in 0..9 {
        crate::ui::clipboard::copy(&conf());
        press(&mut r, Key::Char('p'));
        submit(&mut r, &[(0, &format!("Profile {i}"))], &[]).unwrap();
    }
    r.a.sel[r.a.tab] = 0;
    press(&mut r, Key::Char('x'));
    r.sys.fail("route add");
    r.mgr.retry("v1", 1_000_000);
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
