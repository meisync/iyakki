//! Cloud tenants in the shell (`shell_tenant.rs`, `cloudsession.rs`): the mode choice, adding a tenant with a trust dialog, a changed
//! fingerprint, a refused sign-in, two tenants open at once, input only to the shown one, closing one, nothing of a tenant on Personal,
//! the strip at the standard sizes, and the remembered list on disk. The network is a fake: these tests do not touch the real client.

use super::shell_tenant::Panel;
use super::tests::*;
use super::*;
use crate::cloudsession::{FakeDesktop, FakeNet};
use iyakki_agent::cloudclient::{Input, Pin, Status};
use iyakki_agent::cloudtenants::{self as ct, Tenants};

const SIZES: [(u32, u32); 5] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480), (360, 740)];
const GREEN: [u8; 4] = [20, 120, 60, 255];

fn net() -> Arc<FakeNet> {
    let n = Arc::new(FakeNet::default());
    n.accounts.lock().unwrap().push(("alice".into(), "alicepass1".into()));
    n
}

fn shell(tag: &str, n: &Arc<FakeNet>) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    sh.set_tenant_net_for_test(n.clone());
    sh
}

/// Remember tenants (name, address, pin byte) the way an earlier run would have left them, and make the fake network serve them.
fn seed(sh: &Shell, n: &FakeNet, list: &[(&str, &str, u8)]) {
    let mut t = Tenants::default();
    for (name, addr, pin) in list {
        t.add(name, addr, Pin([*pin; 32])).unwrap();
        n.serve(addr, *pin);
    }
    t.save(&ct::path_in(sh.env.services.dir())).unwrap();
}

fn pump_until(sh: &mut Shell, what: &str, f: impl Fn(&Shell) -> bool) {
    for _ in 0..400 {
        sh.handle(&Event::Tick);
        if f(sh) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("gave up waiting for: {what} (dialog: {})", sh.tenant_dialog());
}

fn frame(sh: &mut Shell, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    g
}

/// Draw twice: the first frame shows the on-screen keyboard (the sign-in field has the focus), the second puts the strip above it.
fn settle(sh: &mut Shell, g: &mut Gfx) {
    sh.render(g);
    sh.render(g);
}

fn has_label(sh: &Shell, label: &str) -> bool {
    sh.hits().iter().any(|h| h.label == label)
}

fn count_colour(g: &Gfx, c: [u8; 4]) -> usize {
    g.pm.data().chunks_exact(4).filter(|p| p[..3] == c[..3]).count()
}

/// Sign in to a remembered tenant through the dialogs.
fn open_tenant(sh: &mut Shell, g: &mut Gfx, name: &str, account: &str, password: &str) {
    settle(sh, g);
    if sh.tenant_dialog() == "closed" {
        click(sh, g, "cloud");
    }
    click(sh, g, &format!("tenant row {name}"));
    pump_until(sh, "the sign-in dialog", |s| s.tenant_dialog() == "signin");
    if let Panel::Creds { fields, focus, .. } = &mut sh.tenants.panel {
        fields[0].set(account);
        *focus = 1;
    }
    typ(sh, password);
    key(sh, Key::Enter);
    pump_until(sh, "the tenant to open", |s| s.open_tenants().iter().any(|n| n == name));
}

#[test]
fn the_sign_in_screen_offers_this_machine_and_cloud_and_the_form_is_unchanged() {
    let n = net();
    let mut sh = shell("tn-mode", &n);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(has_label(&sh, "this machine") && has_label(&sh, "cloud"), "both modes are offered");
    assert!(has_label(&sh, "Sign in"), "the personal form is still there");
    assert_eq!(sh.tenant_dialog(), "closed");
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    assert_eq!(sh.tenant_dialog(), "pick");
    sh.render(&mut g);
    assert!(has_label(&sh, "Add a tenant"));
    // back to this machine: the personal sign-in works as before
    click(&mut sh, &mut g, "this machine");
    assert_eq!(sh.tenant_dialog(), "closed");
    login(&mut sh, "alice", "alicepass1");
    assert_eq!(sh.session().unwrap().user, "alice");
}

#[test]
fn the_first_run_setup_screen_has_no_mode_choice() {
    let (e, _) = env("tn-setup");
    let mut sh = Shell::new(e);
    assert_eq!(sh.screen_name(), "setup");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(!has_label(&sh, "cloud") && !has_label(&sh, "this machine"));
}

#[test]
fn adding_a_tenant_asks_the_person_to_trust_the_fingerprint_and_remembers_it() {
    let n = net();
    n.serve("acme.example.org:443", 0xab);
    let mut sh = shell("tn-add", &n);
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "Add a tenant");
    assert_eq!(sh.tenant_dialog(), "add");
    // the fields take typed text (the same events the on-screen keyboard sends)
    sh.render(&mut g);
    typ(&mut sh, "acme.example.org");
    key(&mut sh, Key::Tab);
    typ(&mut sh, "Acme");
    key(&mut sh, Key::Enter);
    pump_until(&mut sh, "the trust dialog", |s| s.tenant_dialog() == "trust");
    // nothing is remembered before the person accepts
    assert!(sh.remembered_tenants().is_empty());
    assert!(!ct::path_in(sh.env.services.dir()).exists());
    sh.render(&mut g);
    let Panel::Trust { shown, .. } = &sh.tenants.panel else { panic!("trust dialog") };
    assert_eq!(shown.pin.display(), Pin([0xab; 32]).display());
    assert!(has_label(&sh, "Trust this certificate"));
    // Enter does not accept
    key(&mut sh, Key::Enter);
    assert_eq!(sh.tenant_dialog(), "trust");
    click(&mut sh, &mut g, "Trust this certificate");
    assert_eq!(sh.tenant_dialog(), "signin");
    assert_eq!(sh.remembered_tenants(), vec![("Acme".to_string(), "acme.example.org:443".to_string())]);
    let saved = Tenants::load(&ct::path_in(sh.env.services.dir()));
    assert_eq!(saved.get("Acme").unwrap().pin, Pin([0xab; 32]));
}

#[test]
fn declining_the_certificate_remembers_nothing() {
    let n = net();
    n.serve("acme.example.org:443", 7);
    let mut sh = shell("tn-decline", &n);
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "Add a tenant");
    sh.render(&mut g);
    typ(&mut sh, "acme.example.org");
    key(&mut sh, Key::Tab);
    typ(&mut sh, "Acme");
    key(&mut sh, Key::Enter);
    pump_until(&mut sh, "the trust dialog", |s| s.tenant_dialog() == "trust");
    click(&mut sh, &mut g, "Cancel");
    assert_ne!(sh.tenant_dialog(), "trust");
    assert!(sh.remembered_tenants().is_empty());
}

#[test]
fn a_bad_address_or_an_unreachable_server_is_said_in_words() {
    let n = net();
    let mut sh = shell("tn-bad", &n);
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "Add a tenant");
    sh.render(&mut g);
    typ(&mut sh, "not a host");
    key(&mut sh, Key::Tab);
    typ(&mut sh, "X");
    key(&mut sh, Key::Enter);
    assert!(sh.tenant_dialog_error().unwrap().contains("address"), "{:?}", sh.tenant_dialog_error());
    // a good address nobody answers at
    if let Panel::Add { fields, focus, .. } = &mut sh.tenants.panel {
        fields[0].set("nobody.example.org");
        *focus = 1;
    }
    key(&mut sh, Key::Enter);
    pump_until(&mut sh, "the error", |s| s.tenant_dialog_error().is_some_and(|e| e.contains("Could not reach")));
    assert_eq!(sh.tenant_dialog(), "add");
}

#[test]
fn a_changed_fingerprint_is_a_hard_warning_and_nothing_is_sent() {
    let n = net();
    let mut sh = shell("tn-changed", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    n.serve("acme.example.org:443", 2); // the server now shows another certificate
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "tenant row Acme");
    pump_until(&mut sh, "the warning", |s| s.tenant_dialog() == "changed");
    sh.render(&mut g);
    assert!(!has_label(&sh, "Trust this certificate"), "a changed certificate is never offered for acceptance");
    assert!(!has_label(&sh, "Sign in"), "and no password is asked for");
    key(&mut sh, Key::Char('y'));
    key(&mut sh, Key::Enter);
    assert_ne!(sh.tenant_dialog(), "signin");
    assert!(n.made.lock().unwrap().is_empty(), "no connection was made");
    assert_eq!(Tenants::load(&ct::path_in(sh.env.services.dir())).get("Acme").unwrap().pin, Pin([1; 32]), "the old pin is kept");
}

#[test]
fn a_changed_fingerprint_can_only_be_replaced_by_forgetting_the_tenant() {
    let n = net();
    let mut sh = shell("tn-forget", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    n.serve("acme.example.org:443", 2);
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "tenant row Acme");
    pump_until(&mut sh, "the warning", |s| s.tenant_dialog() == "changed");
    click(&mut sh, &mut g, "Forget this tenant");
    assert!(sh.remembered_tenants().is_empty());
    assert!(Tenants::load(&ct::path_in(sh.env.services.dir())).list().is_empty());
}

#[test]
fn a_refused_sign_in_is_shown_and_the_password_is_cleared() {
    let n = net();
    let mut sh = shell("tn-refused", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    settle(&mut sh, &mut g);
    click(&mut sh, &mut g, "cloud");
    click(&mut sh, &mut g, "tenant row Acme");
    pump_until(&mut sh, "the sign-in dialog", |s| s.tenant_dialog() == "signin");
    typ(&mut sh, "alice");
    key(&mut sh, Key::Tab);
    typ(&mut sh, "wrong-password");
    key(&mut sh, Key::Enter);
    pump_until(&mut sh, "the refusal", |s| s.tenant_dialog_error().is_some());
    assert!(sh.tenant_dialog_error().unwrap().contains("refused"));
    let Panel::Creds { fields, .. } = &sh.tenants.panel else { panic!("still on the sign-in dialog") };
    assert!(fields[1].value.is_empty(), "the wrong password is not kept");
    assert!(sh.open_tenants().is_empty());
}

#[test]
fn two_tenants_stay_open_together_and_switching_keeps_both_alive() {
    let n = net();
    let mut sh = shell("tn-two", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("Beta", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    assert_eq!(sh.shown_tenant().as_deref(), Some("Acme"));
    open_tenant(&mut sh, &mut g, "Beta", "alice", "alicepass1");
    assert_eq!(sh.open_tenants(), vec!["Acme", "Beta"]);
    assert_eq!(sh.shown_tenant().as_deref(), Some("Beta"));
    sh.render(&mut g);
    // the chips: this machine, each tenant with a live dot
    assert!(has_label(&sh, "this machine"));
    assert!(has_label(&sh, "tenant Acme (live)") && has_label(&sh, "tenant Beta (live)"));
    click(&mut sh, &mut g, "tenant Acme (live)");
    assert_eq!(sh.shown_tenant().as_deref(), Some("Acme"));
    click(&mut sh, &mut g, "this machine");
    assert_eq!(sh.shown_tenant(), None);
    click(&mut sh, &mut g, "tenant Beta (live)");
    assert_eq!(sh.shown_tenant().as_deref(), Some("Beta"));
    for a in ["acme.example.org:443", "beta.example.org:443"] {
        let d = n.desktop_of(a).unwrap();
        assert!(!d.is_closed(), "{a} stays alive while another view is shown");
    }
    // the remembered account is kept for next time, never the password
    assert_eq!(Tenants::load(&ct::path_in(sh.env.services.dir())).get("Acme").unwrap().last_account, "alice");
}

#[test]
fn the_cycle_key_goes_through_personal_and_every_tenant() {
    let n = net();
    let mut sh = shell("tn-cycle", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("Beta", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    open_tenant(&mut sh, &mut g, "Beta", "alice", "alicepass1");
    let mut seen = Vec::new();
    for _ in 0..3 {
        key(&mut sh, Key::Ctrl('J'));
        seen.push(sh.shown_tenant());
    }
    assert_eq!(seen, vec![None, Some("Acme".to_string()), Some("Beta".to_string())]);
}

#[test]
fn keys_mouse_and_paste_go_only_to_the_shown_tenant() {
    let n = net();
    let mut sh = shell("tn-input", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("Beta", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    open_tenant(&mut sh, &mut g, "Beta", "alice", "alicepass1");
    let (acme, beta) = (n.desktop_of("acme.example.org:443").unwrap(), n.desktop_of("beta.example.org:443").unwrap());
    let before = (acme.sent().len(), beta.sent().len());
    click(&mut sh, &mut g, "tenant Acme (live)");
    sh.render(&mut g);
    typ(&mut sh, "hi");
    key(&mut sh, Key::Enter);
    key(&mut sh, Key::Ctrl('c'));
    let a = sh.tenants.area;
    let (cx, cy) = ((a.x + a.w / 2.0) as i32, (a.y + a.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(cx, cy));
    sh.handle(&Event::MouseDown(cx, cy));
    sh.handle(&Event::MouseUp(cx, cy));
    sh.handle(&Event::Paste("pasted".into()));
    let sent = acme.sent();
    assert!(sent.contains(&Input::Text("h".into())) && sent.contains(&Input::Text("i".into())));
    assert!(sent.contains(&Input::Key { name: "Enter".into(), ctrl: false, shift: false, alt: false }));
    assert!(sent.contains(&Input::Key { name: "c".into(), ctrl: true, shift: false, alt: false }));
    assert!(sent.iter().any(|i| matches!(i, Input::MouseDown { button: 0, .. })) && sent.iter().any(|i| matches!(i, Input::MouseUp { .. })));
    assert!(sent.contains(&Input::Paste("pasted".into())));
    assert_eq!(beta.sent().len(), before.1, "the other tenant got nothing");
    // and on Personal nobody gets anything
    click(&mut sh, &mut g, "this machine");
    let (na, nb) = (acme.sent().len(), beta.sent().len());
    sh.handle(&Event::MouseMove(cx, cy));
    key(&mut sh, Key::Char('x'));
    assert_eq!((acme.sent().len(), beta.sent().len()), (na, nb));
    assert!(na > before.0);
}

#[test]
fn a_click_on_the_picture_lands_at_the_matching_point_of_the_tenants_screen() {
    let n = net();
    let mut sh = shell("tn-map", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    sh.render(&mut g);
    let d = sh.tenants.dest;
    let acme = n.desktop_of("acme.example.org:443").unwrap();
    // the picture is 64x40: the top-left pixel of the drawn picture is (0, 0), the middle is about (32, 20)
    sh.handle(&Event::MouseDown(d.0 as i32 + 1, d.1 as i32 + 1));
    sh.handle(&Event::MouseUp(d.0 as i32 + 1, d.1 as i32 + 1));
    sh.handle(&Event::MouseDown((d.0 + d.2 / 2.0) as i32, (d.1 + d.3 / 2.0) as i32));
    let downs: Vec<(i32, i32)> = acme.sent().iter().filter_map(|i| if let Input::MouseDown { x, y, .. } = i { Some((*x, *y)) } else { None }).collect();
    assert_eq!(downs[0], (0, 0));
    assert!((downs[1].0 - 32).abs() <= 1 && (downs[1].1 - 20).abs() <= 1, "{downs:?}");
    // the picture keeps its 64:40 shape
    assert!((d.2 / d.3 - 1.6).abs() < 0.05, "{d:?}");
}

#[test]
fn closing_one_tenant_leaves_the_other_open_and_signs_out_of_the_closed_one() {
    let n = net();
    let mut sh = shell("tn-close", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("Beta", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    open_tenant(&mut sh, &mut g, "Beta", "alice", "alicepass1");
    click(&mut sh, &mut g, "close tenant"); // Beta is shown
    assert_eq!(sh.open_tenants(), vec!["Acme"]);
    assert_eq!(sh.shown_tenant(), None, "Personal is shown after closing");
    assert!(n.desktop_of("beta.example.org:443").unwrap().is_closed());
    assert!(!n.desktop_of("acme.example.org:443").unwrap().is_closed());
    sh.render(&mut g);
    assert!(has_label(&sh, "tenant Acme (live)") && !has_label(&sh, "tenant Beta (live)"));
}

#[test]
fn the_person_signing_out_ends_every_tenant_session() {
    let n = net();
    let mut sh = shell("tn-signout", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    click(&mut sh, &mut g, "this machine");
    login(&mut sh, "alice", "alicepass1");
    assert!(sh.session().is_some());
    assert_eq!(sh.open_tenants(), vec!["Acme"], "signing in on this machine keeps the tenant");
    sh.sign_out();
    assert!(sh.open_tenants().is_empty());
    assert!(n.desktop_of("acme.example.org:443").unwrap().is_closed());
}

#[test]
fn personal_shows_nothing_of_a_tenant_but_the_chip_name_and_dot() {
    let n = net();
    let mut sh = shell("tn-private", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    let shown = frame(&mut sh, 1280, 720);
    assert!(count_colour(&shown, GREEN) > 1000, "the tenant's picture is on screen while it is shown");
    click(&mut sh, &mut g, "this machine");
    let personal = frame(&mut sh, 1280, 720);
    assert_eq!(count_colour(&personal, GREEN), 0, "no pixel of the tenant's picture on Personal");
    // what the hits reveal is the chip only
    for h in sh.hits() {
        assert!(!h.label.contains("subject"), "{}", h.label);
    }
    assert!(has_label(&sh, "tenant Acme (live)"));
    // a tenant that is not live shows a red or amber dot, still only the name
    n.desktop_of("acme.example.org:443").unwrap().set_status(Status::Reconnecting);
    sh.handle(&Event::Tick);
    sh.render(&mut g);
    assert!(has_label(&sh, "tenant Acme (reconnecting)"));
    n.desktop_of("acme.example.org:443").unwrap().set_status(Status::Closed("gone".into()));
    sh.render(&mut g);
    assert!(has_label(&sh, "tenant Acme (closed)"));
}

#[test]
fn the_on_screen_keyboard_is_offered_over_a_live_tenant_picture() {
    let n = net();
    let mut sh = shell("tn-osk", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    sh.render(&mut g);
    assert!(sh.fields.iter().any(|f| f.focused), "the picture counts as a text target, so the keyboard button appears");
    click(&mut sh, &mut g, "this machine");
    sh.render(&mut g);
    assert!(!sh.fields.iter().any(|f| f.id == 9520), "not on Personal");
}

#[test]
fn the_strip_dialogs_and_picture_fit_the_screen_at_every_standard_size() {
    let n = net();
    let mut sh = shell("tn-sizes", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("A fairly long tenant name indeed", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    open_tenant(&mut sh, &mut g, "A fairly long tenant name indeed", "alice", "alicepass1");
    for (w, h) in SIZES {
        // the shown tenant
        frame(&mut sh, w, h);
        let strip: Vec<R> = sh.tenants.rects.iter().map(|(r, _)| *r).collect();
        assert!(!strip.is_empty());
        for (i, r) in strip.iter().enumerate() {
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: chip {r:?}");
            for o in &strip[i + 1..] {
                assert!(r.x + r.w <= o.x || o.x + o.w <= r.x || r.y + r.h <= o.y || o.y + o.h <= r.y, "{w}x{h}: chips overlap {r:?} {o:?}");
            }
        }
        let a = sh.tenants.area;
        let d = sh.tenants.dest;
        assert!(a.x >= 0.0 && a.y >= 0.0 && a.x + a.w <= w as f32 && a.y + a.h <= h as f32, "{w}x{h}: area {a:?}");
        assert!(d.0 >= a.x && d.1 >= a.y && d.0 + d.2 <= a.x + a.w + 0.5 && d.1 + d.3 <= a.y + a.h + 0.5, "{w}x{h}: picture {d:?} in {a:?}");
        assert!(strip.iter().all(|r| r.y >= a.y + a.h - 0.5), "{w}x{h}: the strip is below the picture");
        // each dialog
        sh.tenants.hub.show(None);
        for dialog in ["pick", "add", "signin", "trust", "changed"] {
            sh.tenants.panel = match dialog {
                "pick" => Panel::Pick { sel: 0, error: Some("Could not reach the server (a long explanation of what went wrong and what to try next).".into()) },
                "add" => Panel::Add { fields: vec![TextField::new("Address", false), TextField::new("Name", false)], focus: 0, error: None },
                "signin" => Shell::tenant_creds_panel(ct::Tenant { name: "Acme".into(), addr: "acme.example.org:443".into(), pin: Pin([1; 32]), last_account: "alice".into() }),
                "trust" => Panel::Trust { name: "Acme".into(), addr: "acme.example.org:443".into(), shown: iyakki_agent::cloudclient::Presented { pin: Pin([0xcd; 32]), subject: "acme.example.org".into(), not_after: 1_900_000_000 } },
                _ => Panel::Changed { name: "Acme".into(), old: Pin([1; 32]), new: Pin([2; 32]) },
            };
            frame(&mut sh, w, h);
            let btns: Vec<R> = sh.tenants.rects.iter().map(|(r, _)| *r).collect();
            for r in &btns {
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h} {dialog}: {r:?} is off the screen");
            }
        }
        sh.tenants.panel = Panel::Closed;
        sh.tenants.hub.show(Some(1));
    }
}

#[test]
fn the_sign_in_screen_chips_keep_clear_of_the_card_at_every_size() {
    let n = net();
    let mut sh = shell("tn-chips", &n);
    for (w, h) in SIZES {
        frame(&mut sh, w, h);
        let chips: Vec<R> = sh.tenants.rects.iter().map(|(r, _)| *r).collect();
        assert_eq!(chips.len(), 2, "{w}x{h}: This machine and Cloud");
        let card = auth_card(w, h, scale_for(w, h), 2);
        for r in chips {
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: {r:?}");
            assert!(r.y >= card.y + card.h - 1.0, "{w}x{h}: {r:?} overlaps the card {card:?}");
        }
    }
}

#[test]
fn the_remembered_list_survives_a_restart_and_holds_no_password() {
    let n = net();
    n.serve("acme.example.org:443", 9);
    let mut sh = shell("tn-persist", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 9)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    let path = ct::path_in(sh.env.services.dir());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("alicepass1"), "the password is never written");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    // a new process reads the same list
    let again = Tenants::load(&path);
    assert_eq!(again.list().len(), 1);
    assert_eq!(again.get("Acme").unwrap().last_account, "alice");
    assert_eq!(again.get("Acme").unwrap().pin, Pin([9; 32]));
}

#[test]
fn a_dropped_link_shows_on_the_dot_and_the_picture_says_so() {
    let n = net();
    let mut sh = shell("tn-drop", &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    let d: FakeDesktop = n.desktop_of("acme.example.org:443").unwrap();
    d.set_status(Status::Reconnecting);
    sh.take_stale();
    sh.handle(&Event::Tick);
    assert!(sh.take_stale(), "a change of the dot asks for a new frame");
    sh.render(&mut g);
    assert!(has_label(&sh, "tenant Acme (reconnecting)"));
    // the window's size is told to the tenant once it has settled
    for _ in 0..30 {
        sh.handle(&Event::Tick);
    }
    assert!(d.sent().iter().any(|i| matches!(i, Input::Size { .. })));
}

// ---------------------------------------------------------------------------- the menu entry and the layout (2026-11-03)

/// Sign in to the machine and keep a tenant open (opened on the sign-in screen, as the owner would).
fn signed_in_with_tenant(tag: &str) -> (Shell, Arc<FakeNet>, Gfx) {
    let n = net();
    let mut sh = shell(tag, &n);
    seed(&sh, &n, &[("Acme", "acme.example.org:443", 1), ("Beta", "beta.example.org:443", 2)]);
    let mut g = Gfx::new(1280, 720);
    open_tenant(&mut sh, &mut g, "Acme", "alice", "alicepass1");
    click(&mut sh, &mut g, "this machine");
    login(&mut sh, "alice", "alicepass1");
    assert!(sh.session().is_some());
    (sh, n, g)
}

#[test]
fn the_iyaki_menu_has_a_cloud_entry_that_lists_switches_and_adds_by_mouse() {
    let (mut sh, _n, mut g) = signed_in_with_tenant("tn-menu");
    sh.tenants.hub.show(None);
    sh.render(&mut g);
    click(&mut sh, &mut g, "IYAKKI menu");
    assert!(has_label(&sh, "Cloud") || { sh.render(&mut g); has_label(&sh, "Cloud") }, "the menu lists Cloud");
    click(&mut sh, &mut g, "Cloud");
    assert_eq!(sh.tenant_dialog(), "pick");
    sh.render(&mut g);
    // the tenant signed in to is listed with its state; the remembered one is listed to sign in to
    assert!(has_label(&sh, "switch to Acme (live)"), "{:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert!(has_label(&sh, "tenant row Beta") && has_label(&sh, "Add a tenant") && has_label(&sh, "Personal"));
    // switch
    click(&mut sh, &mut g, "switch to Acme (live)");
    assert_eq!(sh.tenant_dialog(), "closed");
    assert_eq!(sh.shown_tenant().as_deref(), Some("Acme"));
    // from a shown tenant the menu still opens (the tenant does not swallow the click) and Add opens the connect dialog
    click(&mut sh, &mut g, "IYAKKI menu");
    click(&mut sh, &mut g, "Cloud");
    assert_eq!(sh.tenant_dialog(), "pick");
    click(&mut sh, &mut g, "Add a tenant");
    assert_eq!(sh.tenant_dialog(), "add");
    click(&mut sh, &mut g, "Back");
    click(&mut sh, &mut g, "Personal");
    assert_eq!(sh.shown_tenant(), None);
}

#[test]
fn the_cloud_entry_is_reachable_by_keyboard_and_a_tenant_never_gets_the_menu_keys() {
    let (mut sh, n, mut g) = signed_in_with_tenant("tn-menu-keys");
    sh.tenants.hub.show(None);
    sh.render(&mut g);
    let acme = n.desktop_of("acme.example.org:443").unwrap();
    let before = acme.sent().len();
    key(&mut sh, Key::F(10));
    assert!(sh.brand.open, "F10 opens the IYAKKI menu");
    key(&mut sh, Key::Char('c'));
    assert_eq!(sh.tenant_dialog(), "pick");
    assert!(!sh.brand.open);
    // Down and Enter act on the dialog's rows: the first row is the open tenant (Acme), then Beta
    key(&mut sh, Key::Down);
    key(&mut sh, Key::Up);
    key(&mut sh, Key::Enter);
    assert_eq!(sh.tenant_dialog(), "closed");
    assert_eq!(sh.shown_tenant().as_deref(), Some("Acme"));
    // now the tenant is shown: the menu keys are still the shell's
    sh.render(&mut g);
    key(&mut sh, Key::F(10));
    assert!(sh.brand.open, "F10 opens the IYAKKI menu even over a tenant");
    key(&mut sh, Key::Esc);
    assert!(!sh.brand.open);
    assert_eq!(acme.sent().len(), before, "F10, c, Down, Up, Enter and Esc went to the shell, not to the tenant");
    // Esc closes the dialog
    key(&mut sh, Key::F(10));
    key(&mut sh, Key::Char('c'));
    key(&mut sh, Key::Esc);
    assert_eq!(sh.tenant_dialog(), "closed");
}

#[test]
fn the_cloud_menu_entry_is_in_the_menu_at_every_size_and_is_not_cut_off() {
    let (mut sh, _n, _g) = signed_in_with_tenant("tn-menu-size");
    for (w, h) in [(1280, 960), (360, 740), (1280, 720), (1024, 600)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        key(&mut sh, Key::F(10));
        sh.render(&mut g);
        let hit = sh.hits().iter().find(|x| x.label == "Cloud").unwrap_or_else(|| panic!("{w}x{h}: no Cloud entry")).clone();
        let r = hit.rect;
        assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5 && r.h >= 30.0, "{w}x{h}: {r:?}");
        key(&mut sh, Key::F(10));
    }
}

fn overlaps_r(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

#[test]
fn the_tenant_strip_covers_no_other_control_and_no_control_covers_it() {
    let (mut sh, _n, _g) = signed_in_with_tenant("tn-layout");
    // the on-screen keyboard's button floats over the corner of the picture, as it does over every screen; it is checked against the chips
    let own = |l: &str| l.starts_with("tenant ") || l == "close tenant" || l == "personal" || l == "this machine" || l == "cloud" || l.starts_with("switch to");
    for (w, h) in [(1280, 960), (360, 740), (1280, 720), (1920, 1080), (1024, 600), (800, 480)] {
        let mut g = Gfx::new(w, h);
        for screen in ["launcher", "settings"] {
            sh.tenants.hub.show(None);
            if screen == "settings" {
                sh.open(PApp::Settings);
            } else {
                sh.leave_app();
            }
            // Personal: no strip at all, so nothing of the app or the launcher is covered down to the last row of pixels
            frame_into(&mut sh, &mut g);
            assert!(sh.tenants.rects.is_empty(), "{w}x{h} {screen}: no chips on Personal");
            assert!(!sh.hits().iter().any(|x| x.label.starts_with("tenant ")), "{w}x{h} {screen}: nothing of a tenant on Personal");
            // a tenant shown: the strip is the bottom row and the picture stops above it; the top bar is intact
            sh.tenants.hub.show(Some(0));
            frame_into(&mut sh, &mut g);
            let a = sh.tenants.area;
            let strip: Vec<R> = sh.tenants.rects.iter().map(|(r, _)| *r).collect();
            assert!(!strip.is_empty(), "{w}x{h} {screen}");
            for r in &strip {
                assert!(r.y >= a.y + a.h - 0.5 && r.y + r.h <= h as f32 + 0.5 && r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5, "{w}x{h} {screen}: chip {r:?} area {a:?}");
            }
            for hit in sh.hits() {
                if own(&hit.label) {
                    continue;
                }
                if hit.label == "keyboard" {
                    for r in &strip {
                        assert!(!overlaps_r(hit.rect, *r), "{w}x{h} {screen}: the keyboard button {:?} meets the chip {r:?}", hit.rect);
                    }
                    continue;
                }
                // a control either lies wholly above the tenant's area (the top bar) or is the tenant's own picture
                let covered_by_area = overlaps_r(hit.rect, a) && hit.label != "tenant desktop";
                assert!(!covered_by_area, "{w}x{h} {screen}: `{}` {:?} is under the tenant's area {a:?}", hit.label, hit.rect);
                for r in &strip {
                    assert!(!overlaps_r(hit.rect, *r), "{w}x{h} {screen}: `{}` {:?} meets the strip {r:?}", hit.label, hit.rect);
                }
            }
            // the top bar's controls are still there to click (the name, sign out)
            assert!(has_label(&sh, "IYAKKI menu") || screen == "settings", "{w}x{h} {screen}: the name stays clickable");
            // the dialog's cards do not meet the strip either
            sh.tenants.panel = Panel::Pick { sel: 0, error: None };
            frame_into(&mut sh, &mut g);
            let strip: Vec<R> = sh.tenants.rects.iter().filter(|(_, a)| matches!(a, super::shell_tenant::Act::Machine | super::shell_tenant::Act::Chip(_) | super::shell_tenant::Act::CloseTenant)).map(|(r, _)| *r).collect();
            let cards: Vec<R> = sh.tenants.rects.iter().filter(|(_, a)| matches!(a, super::shell_tenant::Act::Pick(_) | super::shell_tenant::Act::New | super::shell_tenant::Act::Back)).map(|(r, _)| *r).collect();
            assert!(!cards.is_empty());
            for c in &cards {
                for r in &strip {
                    assert!(!overlaps_r(*c, *r), "{w}x{h} {screen}: dialog control {c:?} meets the strip {r:?}");
                }
            }
            sh.tenants.panel = Panel::Closed;
        }
    }
}

fn frame_into(sh: &mut Shell, g: &mut Gfx) {
    // a frame at this graphics size (the shell reads the size from the canvas)
    sh.render(g);
    sh.render(g);
}
