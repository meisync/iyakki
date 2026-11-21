//! Settings -> Remote access -> Connect a client (experimental): off by default and labelled so, administrators only, audited without the
//! link, a one-time link that can be redeemed once, the device list, revoking, and the page fits at two sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::clientmedia::device_holds;
use iyakki_shared::clientlink as cl;
use cl::DeviceKey;

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn page(cx: &AppCx) -> SettingsApp {
    let mut a = SettingsApp::new(cx);
    a.go_to(Tab::Remote);
    a
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn texts(g: &Gfx) -> String {
    g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n")
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn it_is_off_by_default_labelled_experimental_and_switches_on_and_off_with_an_audit_line() {
    let cx = admin("connect1");
    let mut a = page(&cx);
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("Connect a client (EXPERIMENTAL)") && t.contains("Client connections: off"), "{t}");
    assert!(!cl::enabled(cx.data_dir()));
    press(&mut a, &cx, 'l');
    assert!(status(&a).contains("Switch client connections on first"), "{}", status(&a));
    assert_eq!(cl::open_tokens(cx.data_dir(), now()), 0, "no link while it is off");
    press(&mut a, &cx, 'k');
    assert!(cl::enabled(cx.data_dir()));
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("Client connections: on (experimental)"));
    press(&mut a, &cx, 'k');
    assert!(!cl::enabled(cx.data_dir()));
    let log = cx.audit.recent(20).join("\n");
    assert!(log.contains("settings.client_endpoint") && log.contains("on") && log.contains("off"), "{log}");
}

#[test]
fn an_ordinary_account_changes_nothing() {
    let mut cx = admin("connect2");
    cx.session.user = "amy".into();
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = page(&cx);
    for c in ['k', 'l', 'x', '1'] {
        press(&mut a, &cx, c);
    }
    assert!(!cl::enabled(cx.data_dir()));
    assert_eq!(cl::open_tokens(cx.data_dir(), now()), 0);
}

#[test]
fn the_one_time_link_is_shown_as_text_works_once_and_is_not_in_the_audit_log() {
    let cx = admin("connect3");
    let mut a = page(&cx);
    press(&mut a, &cx, 'k');
    press(&mut a, &cx, 'l');
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    let url = t.lines().find(|l| l.contains("/connect#")).unwrap_or_else(|| panic!("no link on the page: {t}")).to_string();
    assert!(url.starts_with("https://"), "{url}");
    let token = url.split_once('#').unwrap().1.to_string();
    assert_eq!(token.len(), 64);
    assert!(t.contains("works once"));
    let log = cx.audit.recent(50).join("\n");
    assert!(log.contains("settings.client_link") && !log.contains(&token), "the audit holds the fact, never the link: {log}");
    assert_eq!(cl::redeem_token(cx.data_dir(), &token, now()), Ok("root".to_string()));
    assert_eq!(cl::redeem_token(cx.data_dir(), &token, now()), Err(cl::TokenError::Used));
    // switching off takes the link off the page
    press(&mut a, &cx, 'k');
    assert!(!texts(&draw(&mut a, &cx, 1280, 960)).contains("/connect#"));
}

#[test]
fn devices_are_listed_with_last_seen_and_revoked_by_number() {
    let cx = admin("connect4");
    let mut a = page(&cx);
    press(&mut a, &cx, 'k');
    let d = cx.data_dir().to_path_buf();
    let n = now();
    let one = cl::enrol_device(&d, "Maya's laptop", &DeviceKey::from_seed([1; 32]).public_hex(), "root", n - 7200).unwrap();
    cl::touch_device(&d, &one.id, n - 300);
    cl::enrol_device(&d, "Phone", &DeviceKey::from_seed([2; 32]).public_hex(), "root", n).unwrap();
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("Maya's laptop") && t.contains("last seen 5 min ago"), "{t}");
    assert!(t.contains("Phone") && t.contains("last seen never"), "{t}");
    // the list is newest first: 1 is the phone, 2 the laptop; a digit alone does nothing
    press(&mut a, &cx, '2');
    assert!(cl::get_device(&d, &one.id).unwrap().active());
    press(&mut a, &cx, 'x');
    assert!(status(&a).contains("Revoke which device"));
    press(&mut a, &cx, '2');
    assert!(!cl::get_device(&d, &one.id).unwrap().active());
    assert!(status(&a).contains("Maya's laptop is revoked"));
    // another key cancels the arming
    press(&mut a, &cx, 'x');
    press(&mut a, &cx, 'q');
    press(&mut a, &cx, '1');
    assert_eq!(cl::list_devices(&d).iter().filter(|x| x.active()).count(), 1);
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("revoked"), "{t}");
    let log = cx.audit.recent(50).join("\n");
    assert!(log.contains("settings.client_revoke") && log.contains(&one.id), "{log}");
    // a number with no device
    press(&mut a, &cx, 'x');
    press(&mut a, &cx, '9');
    assert!(status(&a).contains("no device with that number"));
}

#[test]
fn the_page_fits_at_two_sizes_with_a_link_and_devices_and_the_qr_code_is_drawn() {
    let cx = admin("connect5");
    let mut a = page(&cx);
    press(&mut a, &cx, 'k');
    press(&mut a, &cx, 'l');
    cl::enrol_device(cx.data_dir(), "Maya's laptop", &DeviceKey::from_seed([3; 32]).public_hex(), "root", now()).unwrap();
    for (w, h) in [(1280u32, 960u32), (360, 740)] {
        for end in [false, true] {
            a.handle(&Event::Key(if end { Key::End } else { Key::Home }), &cx);
            let g = draw(&mut a, &cx, w, h);
            for (_, rect, clip, text) in &g.layout_log.texts {
                let right = clip.map_or(rect.x + rect.w, |c| (rect.x + rect.w).min(c.x + c.w));
                assert!(rect.x >= 0.0 && right <= w as f32 + 0.5, "{w}x{h}: {text:?} at {rect:?} leaves the screen sideways");
            }
            for x in g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char('k' | 'l')))) {
                let r = x.rect;
                assert!(r.w > 0.0 && r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {} {r:?} outside the screen", x.label);
            }
        }
    }
    let g = draw(&mut a, &cx, 1280, 960);
    assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('l'))), "the new-link button is reachable by touch");
}

#[test]
fn ago_says_it_in_words() {
    assert_eq!((ago(1000, 0), ago(1000, 990), ago(1000, 400), ago(100_000, 90_000), ago(1_000_000, 1)), ("never".to_string(), "just now".to_string(), "10 min ago".to_string(), "2 h ago".to_string(), "11 days ago".to_string()));
}

#[test]
fn a_devices_permissions_are_switched_one_by_one_audited_and_count_at_the_next_request() {
    let cx = admin("connect-perm");
    let mut a = page(&cx);
    press(&mut a, &cx, 'k');
    let d = cx.data_dir().to_path_buf();
    let dev = cl::enrol_device(&d, "Maya's laptop", &DeviceKey::from_seed([9; 32]).public_hex(), "root", now()).unwrap();
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(!t.contains("audio.out"), "nothing shown until a device is picked: {t}");
    // d then the number opens the device's page
    press(&mut a, &cx, '1');
    assert!(device_holds(&d, &dev.id).is_empty(), "a digit alone does nothing");
    press(&mut a, &cx, 'd');
    assert!(status(&a).contains("Permissions of which device"));
    press(&mut a, &cx, '1');
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    for p in ["audio.out", "clipboard.client", "camera.view", "client.camera.offer", "client.microphone.offer"] {
        assert!(t.contains(&format!("{p}: off")), "{p}: {t}");
    }
    assert!(t.contains("still ask on this machine at every use"), "{t}");
    // the buttons are reachable by touch too
    let g = draw(&mut a, &cx, 1280, 960);
    for c in ['1', '2', '3', '4', '5', 'd'] {
        assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char(c))), "no button for {c}");
    }
    // switch the camera offer and the sound on, the sound off again
    press(&mut a, &cx, '4');
    press(&mut a, &cx, '1');
    assert_eq!(device_holds(&d, &dev.id), vec!["speaker.play", "client.camera.offer"]);
    assert!(status(&a).contains("may now ask to"));
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("audio.out: on") && t.contains("client.camera.offer: on") && t.contains("camera.view: off"), "{t}");
    press(&mut a, &cx, '1');
    assert_eq!(device_holds(&d, &dev.id), vec!["client.camera.offer"]);
    assert!(status(&a).contains("no longer"));
    // the broker agrees at its next call, and a hard ask still asks (no card here: no_card)
    let (broker, _log) = iyakki_shared::accessbroker::local(&d, None);
    let caller = iyakki_core_api::broker::Caller { app: "client".into(), kind: iyakki_core_api::broker::CallerKind::Client { device_id: dev.id.clone() }, person: "root".into(), device: "Maya's laptop".into() };
    let call = |cap| iyakki_core_api::broker::Call { caller: &caller, capability: cap, unit: None, label: "x" };
    assert_eq!(broker.decide(&call("speaker.play")), Err(iyakki_core_api::broker::Denial::Revoked));
    assert_eq!(broker.decide(&call("client.camera.offer")), Err(iyakki_core_api::broker::Denial::NoCard), "on is only allowed to ask");
    let log = cx.audit.recent(50).join("\n");
    assert!(log.contains("settings.client_permission") && log.contains("client.camera.offer on Maya's laptop") && log.contains("audio.out off"), "{log}");
    // d goes back; the page has no secrets in the audit
    press(&mut a, &cx, 'd');
    assert!(!texts(&draw(&mut a, &cx, 1280, 960)).contains("audio.out:"));
    // a number with no device, and a non-administrator cannot
    press(&mut a, &cx, 'd');
    press(&mut a, &cx, '8');
    assert!(status(&a).contains("no device with that number"));
}

#[test]
fn a_devices_role_is_shown_stepped_with_r_audited_and_sets_every_switch_at_once() {
    let cx = admin("connect-role");
    let mut a = page(&cx);
    press(&mut a, &cx, 'k');
    let d = cx.data_dir().to_path_buf();
    let dev = cl::enrol_device(&d, "Maya's laptop", &DeviceKey::from_seed([8; 32]).public_hex(), "root", now()).unwrap();
    press(&mut a, &cx, 'd');
    press(&mut a, &cx, '1');
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("Role of Maya's laptop: viewer") && t.contains("Press r for the next role"), "{t}");
    press(&mut a, &cx, 'r');
    assert_eq!(device_holds(&d, &dev.id), vec!["speaker.play"], "listener: sound");
    assert!(status(&a).contains("role listener"), "{}", status(&a));
    press(&mut a, &cx, 'r');
    press(&mut a, &cx, 'r');
    let full = iyakki_shared::clientmedia::DEVICE_ROLES.iter().find(|r| r.0 == "full").expect("the full role").2;
    assert_eq!(device_holds(&d, &dev.id).len(), full.len(), "full: every switch");
    assert_eq!(full.len(), iyakki_agent::clientmedia::GRANTABLE.len(), "the full role holds every grantable switch");
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("Role of Maya's laptop: full"));
    press(&mut a, &cx, 'r');
    assert!(device_holds(&d, &dev.id).is_empty(), "back to viewer: the other switches are taken away");
    // a switch set by hand makes the role custom and the next press starts at the first role again
    press(&mut a, &cx, '4');
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("Role of Maya's laptop: custom"));
    press(&mut a, &cx, 'r');
    assert!(device_holds(&d, &dev.id).is_empty() && status(&a).contains("role viewer"));
    assert!(cx.audit.recent(50).join("\n").contains("settings.client_role listener Maya's laptop"));
}
