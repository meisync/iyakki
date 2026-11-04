//! Settings -> Remote access: the button "Serve this desktop to browsers" is off by default, refuses without an administrator password,
//! is audited, shows the addresses, the certificate fingerprint and what it exposes, takes a port, reaches keyboard and touch, and
//! the page fits at 1280x960 and 360x740 (scrolling to the end).

use super::remote_page::RemoteForm;
use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::webaccess as wa;

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
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
    assert_eq!(a.tab(), Tab::Remote);
    a
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
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

fn fake_certificate(cx: &AppCx) {
    let d = cx.services.dir().join("tls");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("cert.pem"), "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n").unwrap();
}

#[test]
fn it_is_off_by_default_turns_on_with_the_exposure_said_and_off_again_and_is_audited() {
    let cx = admin("remote1");
    let mut a = page(&cx);
    assert!(!cx.services.web_access_on() && cx.services.web_access_port() == 8443);
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    assert!(t.contains("Off: nothing listens"), "{t}");
    assert!(!t.contains("https://"), "no address is offered while it is off: {t}");
    a.handle(&Event::Key(Key::Char('e')), &cx);
    assert!(cx.services.web_access_on());
    let s = status(&a);
    assert!(s.contains("Anyone on this network who knows a sign-in can use this desktop") && s.contains("machine's accounts") && s.contains(".local:8443"), "{s}");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.web_access") && l.contains("on (port 8443, HTTPS)")));
    fake_certificate(&cx);
    let t = texts(&draw(&mut a, &cx, 1280, 960));
    let label = iyakki_agent::devname::label(cx.services.dir());
    assert!(t.contains(&format!("https://{label}.local:8443")), "{t}");
    assert!(t.contains("Certificate fingerprint (SHA-256)") && t.contains("What this exposes"), "{t}");
    let fp = iyakki_agent::devmode::cert_fingerprint(cx.services.dir()).unwrap();
    let rows: Vec<String> = fp.split(':').collect::<Vec<_>>().chunks(8).map(|c| c.join(":")).collect();
    assert!(rows.len() == 4 && rows.iter().all(|r| t.contains(r.as_str())), "the whole fingerprint of the certificate is drawn: {t}");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(!cx.services.web_access_on());
    assert!(status(&a).contains("No longer serving"), "{}", status(&a));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.web_access") && l.contains("off (port 8443")));
}

#[test]
fn it_refuses_to_turn_on_without_an_administrator_with_a_password_and_for_ordinary_accounts() {
    let mut cx = admin("remote2");
    cx.store = Arc::new(std::sync::Mutex::new(crate::accounts::UserStore::open(&temp_dir("remote2-users").join("users.json")).unwrap()));
    let mut a = page(&cx);
    a.handle(&Event::Key(Key::Char('e')), &cx);
    assert!(!cx.services.web_access_on());
    assert!(status(&a).contains("no administrator account with a password"), "{}", status(&a));
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("cannot be switched on"));
    let mut user = admin("remote3");
    user.session.user = "amy".into();
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = SettingsApp::new(&user);
    b.handle_remote(&Key::Char('e'), &user);
    assert!(!user.services.web_access_on(), "an ordinary account cannot change it");
}

#[test]
fn the_port_is_asked_checked_and_kept() {
    let cx = admin("remote4");
    let mut a = page(&cx);
    a.handle(&Event::Key(Key::Char('p')), &cx);
    assert!(a.has_modal(), "the port question opens");
    let mut f = Form::new("Port").text("Port", "8443", "");
    for (bad, why) in [("80", "1024"), ("abc", "number"), ("70000", "number")] {
        f.set_value(0, bad);
        let e = a.remote_submit(&cx, &RemoteForm::Port, &mut f).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert_eq!(cx.services.web_access_port(), 8443);
    f.set_value(0, "9443");
    assert!(a.remote_submit(&cx, &RemoteForm::Port, &mut f).unwrap().contains("9443"));
    assert_eq!(cx.services.web_access_port(), 9443);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.web_access.port") && l.contains("9443")));
    f.set_value(0, "");
    a.remote_submit(&cx, &RemoteForm::Port, &mut f).unwrap();
    assert_eq!(cx.services.web_access_port(), wa::DEFAULT_PORT, "empty: the default");
}

#[test]
fn the_page_says_when_it_was_set_at_boot_and_when_the_port_is_in_use() {
    let cx = admin("remote5");
    let mut a = page(&cx);
    a.handle(&Event::Key(Key::Char('e')), &cx);
    wa::write_status(cx.services.dir(), &wa::Status { listening: true, port: 8443, boot: true, error: String::new() });
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("on (set at boot), port 8443"));
    wa::write_status(cx.services.dir(), &wa::Status { listening: false, port: 8443, boot: false, error: "port 8443 is already in use by another program".into() });
    assert!(texts(&draw(&mut a, &cx, 1280, 960)).contains("not listening: port 8443 is already in use by another program"));
}

#[test]
fn the_button_is_reached_by_keyboard_and_touch_and_the_page_fits_at_two_sizes() {
    let cx = admin("remote6");
    fake_certificate(&cx);
    let mut a = page(&cx);
    for on in [false, true] {
        if on {
            a.handle(&Event::Key(Key::Char('e')), &cx);
        }
        for (w, h) in [(1280u32, 960u32), (360, 740)] {
            for end in [false, true] {
                a.handle(&Event::Key(if end { Key::End } else { Key::Home }), &cx);
                let g = draw(&mut a, &cx, w, h);
                let b = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('e'))).unwrap_or_else(|| panic!("{w}x{h}: no button for e"));
                assert!(b.label.contains(if on { "Stop serving" } else { "Serve this desktop to browsers" }), "{w}x{h}: {}", b.label);
                for x in g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char('e' | 'p')))) {
                    let r = x.rect;
                    assert!(r.w > 0.0 && r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {} {r:?} outside the screen", x.label);
                }
                for (_, rect, clip, text) in &g.layout_log.texts {
                    let right = clip.map_or(rect.x + rect.w, |c| (rect.x + rect.w).min(c.x + c.w));
                    assert!(rect.x >= 0.0 && right <= w as f32 + 0.5, "{w}x{h}: {text:?} at {rect:?} leaves the screen sideways");
                }
                if end {
                    // scrolled to the end, the last sentence is inside the page
                    let last = g.layout_log.texts.iter().find(|t| t.3.contains("Turn it off when you are done") || t.3.contains("done. The firewall")).unwrap_or_else(|| panic!("{w}x{h}: the last sentence is reachable by scrolling"));
                    assert!(last.1.y + last.1.h <= h as f32 + 0.5, "{w}x{h}: {:?}", last.1);
                }
            }
        }
    }
    // touch: the button is a hit area that sends the key, like every button bar of Settings
    let g = draw(&mut a, &cx, 360, 740);
    let b = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('e'))).unwrap();
    assert!(b.rect.w >= 100.0 && b.rect.h > 0.0, "{:?}", b.rect);
}
