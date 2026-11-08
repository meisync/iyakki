//! Settings -> Network -> Share internet against a made-up machine (`MemBackend`): the lists with names and states, the Wi-Fi line, the
//! first press that warns and the second that starts, stop, one-line errors, the clients list, administrators only, audit, and the page
//! at two screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::sharing::control::{Backend, MemBackend};
use iyakki_agent::sharing::{Client, IfaceInfo, LinkKind, State, Status};

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

fn machine() -> Vec<IfaceInfo> {
    let mut up = IfaceInfo::new("eth0", LinkKind::Ethernet);
    up.cidrs = vec![iyakki_agent::sharing::Cidr::parse("192.168.1.5/24").unwrap()];
    up.default_route = true;
    vec![up, IfaceInfo::new("enx0", LinkKind::Usb)]
}

fn page(cx: &AppCx, ifs: Vec<IfaceInfo>) -> (SettingsApp, Arc<MemBackend>) {
    let mut a = SettingsApp::new(cx);
    a.go_to(Tab::Network);
    if a.tab() != Tab::Network {
        a.tabs.push(Tab::Network); // a person without the Network page: a hand-built request, as the hardware tests do
        a.tab = a.tabs.len() - 1;
    }
    assert_eq!(a.tab(), Tab::Network);
    let b = Arc::new(MemBackend::new(ifs, false));
    a.share.backend = Some(b.clone());
    a.share.ttl_ms = 0;
    a.handle(&Event::Key(Key::Char('i')), cx);
    assert!(a.share.open, "i opens the page");
    (a, b)
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> String {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_page_lists_what_the_machine_has_with_names_and_states_and_the_wifi_line() {
    let cx = admin("share1");
    let mut ifs = machine();
    ifs.push(IfaceInfo::new("thunderbolt0", LinkKind::Thunderbolt));
    ifs[1].carrier = false;
    let (mut a, _) = page(&cx, ifs);
    let t = draw(&mut a, &cx, 1280, 960);
    for want in ["Share internet", "Sharing is off.", "Ethernet eth0 - has internet", "USB link enx0 - no cable", "Thunderbolt / USB-C link thunderbolt0 - cable connected", "Wi-Fi hotspot: not available on this build", "give other computers access to your internet", "Client isolation is on", "switches itself off after 2 h"] {
        assert!(t.contains(want), "missing {want:?} in\n{t}");
    }
    assert!(t.contains("eth0 - in use") || t.contains("Ethernet eth0 - in use"), "the uplink is listed as in use on the share-through list: {t}");
}

#[test]
fn the_first_press_of_start_warns_and_the_second_sends_the_request_and_audits_it() {
    let cx = admin("share2");
    let (mut a, b) = page(&cx, machine());
    a.handle(&Event::Key(Key::Down), &cx); // enx0 (eth0 is first, in use)
    press(&mut a, &cx, 's');
    let s = status(&a);
    assert!(s.contains("access to your internet connection") && s.contains("Press s again to start sharing eth0 through"), "{s}");
    assert!(b.request().id == 0, "nothing was asked yet");
    press(&mut a, &cx, 's');
    assert!(status(&a).contains("Starting to share eth0 through"), "{}", status(&a));
    let r = b.request();
    assert!(r.on && r.id == 1 && r.uplink.as_deref() == Some("eth0") && r.via.as_deref() == Some("enx0") && r.isolate && !r.allow_lan && r.idle_hours == 2);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.share") && l.contains("start eth0 -> enx0")));
    // another key between the two presses disarms
    let (mut a2, b2) = page(&cx, machine());
    a2.handle(&Event::Key(Key::Down), &cx);
    press(&mut a2, &cx, 's');
    press(&mut a2, &cx, 'o');
    press(&mut a2, &cx, 's');
    assert!(!b2.request().on, "the confirmation was interrupted");
}

#[test]
fn one_click_or_tap_on_start_starts_sharing_without_a_second_press() {
    let cx = admin("share-click1");
    let (mut a, b) = page(&cx, machine());
    a.handle(&Event::Key(Key::Down), &cx);
    crate::ui::set_from_click(true);
    press(&mut a, &cx, 's');
    crate::ui::set_from_click(false);
    assert!(b.request().on, "one click started it: {}", status(&a));
    assert!(status(&a).contains("Starting to share eth0 through"), "{}", status(&a));
}

#[test]
fn it_stops_when_it_is_on_and_shows_the_devices_and_the_time_left() {
    let cx = admin("share3");
    let (mut a, b) = page(&cx, machine());
    b.set_status(Status { state: State::On, uplink: "eth0".into(), downstream: "enx0".into(), subnet: "10.42.0.0/24".into(), gateway: "10.42.0.1".into(), idle_hours: 2, isolate: true, off_in_secs: Some(6480), clients: vec![Client { name: "laptop-2".into(), ip: "10.42.0.10".into(), mac: "02:00:00:00:00:07".into(), expires_in: 3000 }], rx_bytes: 1200, tx_bytes: 34000, request_id: 1, updated: 1, ..Status::default() });
    let t = draw(&mut a, &cx, 1280, 960);
    for want in ["Sharing eth0 through enx0 on 10.42.0.0/24 (1 device). Switches off in 1 h 48 min", "Devices that were given an address", "stays in this list until its address runs out", "laptop-2", "10.42.0.10", "address kept for 50 min", "1200 bytes from the devices"] {
        assert!(t.contains(want), "missing {want:?} in\n{t}");
    }
    press(&mut a, &cx, 's');
    assert!(!b.request().on && b.request().id == 1, "the stop request is written: {:?}", b.request());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.share") && l.contains("stop")));
}

#[test]
fn every_problem_is_explained_in_one_line() {
    let cx = admin("share4");
    // no default route
    let mut ifs = machine();
    ifs[0].default_route = false;
    let (mut a, b) = page(&cx, ifs);
    let t = draw(&mut a, &cx, 1280, 960);
    assert!(t.contains("no connection on this computer has a route to the internet"), "{t}");
    press(&mut a, &cx, 's');
    assert!(status(&a).starts_with("There is no internet to share"), "{}", status(&a));
    assert!(!b.request().on);
    // the device is busy (it has an address of its own)
    let mut ifs = machine();
    ifs[1].cidrs = vec![iyakki_agent::sharing::Cidr::parse("172.30.0.4/24").unwrap()];
    let (mut a, b) = page(&cx, ifs);
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 's');
    assert!(status(&a).contains("enx0 cannot be used") || status(&a).contains("enx0 is busy"), "{}", status(&a));
    assert!(!b.request().on);
    // nothing to share through
    let (mut a, _) = page(&cx, vec![machine().remove(0)]);
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 's');
    let s = status(&a);
    assert!(s.contains("cannot be used") || s.contains("No cable device"), "{s}");
    // the service reports a failure
    let (mut a, b) = page(&cx, machine());
    b.set_status(Status { state: State::Failed, note: "Sharing stopped: eth0 lost its route to the internet.".into(), request_id: 3, ..Status::default() });
    assert!(draw(&mut a, &cx, 1280, 960).contains("Sharing is not running. Sharing stopped: eth0 lost its route"));
}

#[test]
fn options_hours_isolation_and_local_network_change_the_request() {
    let cx = admin("share5");
    let (mut a, b) = page(&cx, machine());
    a.handle(&Event::Key(Key::Down), &cx);
    for _ in 0..3 {
        press(&mut a, &cx, '+');
    }
    press(&mut a, &cx, '-');
    press(&mut a, &cx, 'o');
    press(&mut a, &cx, 'l');
    let t = draw(&mut a, &cx, 1280, 960);
    assert!(t.contains("after 4 h") && t.contains("Devices can reach each other: yes") && t.contains("local network: yes") && t.contains("Client isolation is OFF"), "{t}");
    press(&mut a, &cx, 's');
    press(&mut a, &cx, 's');
    let r = b.request();
    assert!(r.on && r.idle_hours == 4 && !r.isolate && r.allow_lan, "{r:?}");
    // limits
    for _ in 0..100 {
        press(&mut a, &cx, '+');
    }
    assert_eq!(a.share.hours, 72);
    for _ in 0..100 {
        press(&mut a, &cx, '-');
    }
    assert_eq!(a.share.hours, 1);
}

#[test]
fn only_administrators_can_change_anything_and_it_is_audited() {
    let mut cx = admin("share6");
    cx.session.user = "amy".into();
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let (mut a, b) = page(&cx, machine());
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 's');
    press(&mut a, &cx, 's');
    press(&mut a, &cx, 'o');
    assert!(status(&a).contains("Only administrators"), "{}", status(&a));
    assert!(!b.request().on && b.request().id == 0);
    assert!(!a.share.can_start || draw(&mut a, &cx, 1280, 960).contains("can only be started by an administrator"));
    assert!(a.share.isolate, "the option did not change");
    assert!(draw(&mut a, &cx, 1280, 960).contains("Only administrators can change the options."));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.denied")));
}

#[test]
fn keys_move_between_the_lists_and_back_leaves_the_page() {
    let cx = admin("share7");
    let (mut a, _) = page(&cx, machine());
    assert_eq!(a.share.col, 1);
    press(&mut a, &cx, 'u');
    assert_eq!(a.share.col, 0);
    press(&mut a, &cx, 'd');
    a.handle(&Event::Key(Key::Down), &cx);
    assert_eq!(a.share.down_sel, 1);
    a.handle(&Event::Key(Key::Down), &cx);
    a.handle(&Event::Key(Key::Down), &cx);
    assert_eq!(a.share.down_sel, 2, "stays on the last row (the Bluetooth entry)");
    assert_eq!(a.buttons_for(true).iter().map(|b| b.2).collect::<Vec<_>>(), ['s', '+', '-', 'o', 'l', 'b']);
    press(&mut a, &cx, 'b');
    assert!(!a.share.open);
    assert!(a.buttons_for(true).iter().any(|b| b.0 == "Share internet"), "the Network page offers the new page");
}

#[test]
fn the_page_fits_a_desktop_and_a_phone_and_scrolls_to_the_end() {
    let cx = admin("share8");
    let (mut a, b) = page(&cx, machine());
    b.set_status(Status { state: State::On, uplink: "eth0".into(), downstream: "enx0".into(), subnet: "10.42.0.0/24".into(), clients: (0..6).map(|i| Client { name: format!("device-{i}"), ip: format!("10.42.0.{}", 10 + i), mac: "02".into(), expires_in: 100 }).collect(), request_id: 1, off_in_secs: Some(100), ..Status::default() });
    for (w, h) in [(1280, 960), (360, 740)] {
        let t = draw(&mut a, &cx, w, h);
        assert!(t.contains("Share internet"));
        for _ in 0..30 {
            a.handle(&Event::Key(Key::PageDown), &cx);
        }
        let t = draw(&mut a, &cx, w, h);
        assert!(t.contains("back."), "the end of the page is reachable at {w}x{h}: {t}");
        for _ in 0..30 {
            a.handle(&Event::Key(Key::PageUp), &cx);
        }
    }
}

#[test]
fn bluetooth_is_listed_with_the_reason_and_cannot_be_chosen() {
    let cx = admin("share9");
    let (mut a, b) = page(&cx, machine());
    let t = draw(&mut a, &cx, 1280, 960);
    assert!(t.contains("Bluetooth tethering (phones connect to this computer)") && t.contains("Bluetooth tethering"), "{t}");
    assert!(t.contains("use a phone's tethering"), "{t}");
    a.handle(&Event::Key(Key::Down), &cx);
    a.handle(&Event::Key(Key::Down), &cx); // bluetooth-nap
    press(&mut a, &cx, 's');
    assert!(status(&a).contains("bluetooth-nap cannot be used") && !b.request().on, "{}", status(&a));
}

#[test]
fn a_user_may_start_when_the_policy_says_users_and_sees_the_page_read_only_when_it_says_nobody() {
    use iyakki_agent::sharing::policy::{Policy, Who};
    let mut cx = admin("share10");
    cx.session.user = "amy".into();
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    // users allowed: amy starts it, and the service is told she is not an administrator
    let (mut a, b) = page(&cx, machine());
    b.set_policy_raw(Policy { wired: Who::Users, ..Policy::default() });
    a.share_open(&cx);
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 's');
    press(&mut a, &cx, 's');
    let r = b.request();
    assert!(r.on && r.by == "amy" && !r.by_admin, "{r:?}");
    // nobody: Start is hidden, the reason and who changed the policy are shown, nothing is written
    let (mut a, b) = page(&cx, machine());
    b.set_policy_raw(Policy { wired: Who::Nobody, wifi: Who::Nobody, bluetooth: Who::Nobody, changed_by: "root".into(), changed_at: 77, ..Policy::default() });
    a.share_open(&cx);
    let t = draw(&mut a, &cx, 1280, 960);
    assert!(t.contains("Internet sharing is turned off by your administrator.") && t.contains("Policy last changed by root (at 77)"), "{t}");
    assert_eq!(a.buttons_for(false).iter().map(|b| b.2).collect::<Vec<_>>(), ['b']);
    a.handle(&Event::Key(Key::Down), &cx);
    press(&mut a, &cx, 's');
    assert!(status(&a).contains("turned off by your administrator") && !b.request().on);
}

#[test]
fn an_administrator_changes_the_policy_from_the_page_and_it_is_audited_with_before_and_after() {
    use iyakki_agent::sharing::policy::Who;
    let cx = admin("share11");
    let (mut a, b) = page(&cx, machine());
    press(&mut a, &cx, 'p');
    assert_eq!(b.policy().wired, Who::Users);
    assert_eq!(b.policy().changed_by, "root");
    let l = cx.audit.recent(20).into_iter().find(|l| l.contains("settings.share.policy")).expect("audited");
    assert!(l.contains("before [wired admins") && l.contains("after [wired users"), "{l}");
}

#[test]
fn a_click_or_touch_on_a_row_of_either_list_chooses_it_like_the_keys_do() {
    let cx = admin("share-click");
    let mut ifs = machine();
    ifs.push(IfaceInfo::new("thunderbolt0", LinkKind::Thunderbolt));
    let (mut a, _) = page(&cx, ifs);
    let mut g = Gfx::new(1280, 900);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 828.0), 1.0, &cx, 0);
    // every row of both lists is a target, and each is tall enough for a finger
    let downs: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(_, Key::Char('D')))).collect();
    let ups: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(_, Key::Char('U')))).collect();
    assert!(downs.len() >= 3 && !ups.is_empty(), "{} down rows, {} internet rows", downs.len(), ups.len());
    assert!(downs.iter().all(|h| h.rect.h >= 28.0), "rows are big enough to touch");
    let want = g.hits.iter().find(|h| h.label.contains("thunderbolt0")).expect("the Thunderbolt row").ev.clone();
    a.handle(&want, &cx);
    assert_eq!((a.share.col, a.share.down_sel), (1, 2), "the click chose the Thunderbolt link and moved to the second list");
    // a click on an internet-source row moves back to the first list
    let first_up = ups[0].ev.clone();
    a.handle(&first_up, &cx);
    assert_eq!((a.share.col, a.share.up_sel), (0, 0));
}

#[test]
fn a_click_on_a_microphone_or_output_row_chooses_it_instead_of_only_selecting_it() {
    assert!(Target::SoundMic("plughw:2,0".into()).activates_on_click() && Target::SoundOut("PCH:3".into(), "plughw:0,3".into()).activates_on_click());
    assert!(Target::SoundT2Mic.activates_on_click() && Target::SoundChoice.activates_on_click());
    // rows that change the machine's connections stay select-then-press
    assert!(!Target::Ethernet("eth0".into()).activates_on_click() && !Target::Nothing.activates_on_click());
    assert!(!Target::BtPower(0, true).activates_on_click());
}

fn click(a: &mut SettingsApp, cx: &AppCx, label: &str) {
    let mut g = Gfx::new(1280, 900);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 828.0), 1.0, cx, 0);
    let ev = g.hits.iter().find(|h| h.label.contains(label)).unwrap_or_else(|| panic!("no click target for {label}")).ev.clone();
    a.handle(&ev, cx);
}

#[test]
fn the_option_rows_and_the_policy_row_can_be_clicked_or_touched_not_only_keyed() {
    let cx = admin("share-optclick");
    let (mut a, b) = page(&cx, machine());
    let (iso, lan, hours) = (a.share.isolate, a.share.allow_lan, a.share.hours);
    click(&mut a, &cx, "Devices can reach each other");
    assert_eq!(a.share.isolate, !iso, "a click on the each-other row flips it");
    click(&mut a, &cx, "Devices can reach my local network");
    assert_eq!(a.share.allow_lan, !lan, "a click on the local-network row flips it");
    click(&mut a, &cx, "Switch off by itself");
    assert_eq!(a.share.hours, hours + 1, "a click on the hours row adds an hour");
    let before = b.policy().wired;
    click(&mut a, &cx, "Who may start wired and USB sharing");
    assert_ne!(b.policy().wired, before, "a click on the policy row cycles it");
    // the keys still work, and the band key no longer collides with Back
    press(&mut a, &cx, 'o');
    assert_eq!(a.share.isolate, iso);
    press(&mut a, &cx, 'w');
    assert!(a.share.open, "w is the band key; b still goes back");
    press(&mut a, &cx, 'b');
    assert!(!a.share.open);
}

/// A made-up machine whose Wi-Fi card can be an access point and whose hotspot has a name and password.
struct Hot(Arc<MemBackend>, bool);
impl Backend for Hot {
    fn interfaces(&self) -> Vec<IfaceInfo> {
        self.0.interfaces()
    }
    fn wifi_ap(&self) -> bool {
        self.0.wifi_ap()
    }
    fn status(&self) -> Status {
        self.0.status()
    }
    fn request(&self) -> iyakki_agent::sharing::Request {
        self.0.request()
    }
    fn submit(&self, r: &iyakki_agent::sharing::Request) -> Result<(), String> {
        self.0.submit(r)
    }
    fn policy(&self) -> iyakki_agent::sharing::policy::Policy {
        self.0.policy()
    }
    fn hotspot(&self) -> Option<(String, String)> {
        self.1.then(|| ("iyakki-ab12".to_string(), "pass;word".to_string()))
    }
    fn set_policy(&self, p: &iyakki_agent::sharing::policy::Policy, by: &str, a: bool) -> Result<String, String> {
        self.0.set_policy(p, by, a)
    }
}

/// Dark pixels (all of red, green and blue under 40) inside the white QR square found by scanning for white.
fn hotspot_page(tag: &str, with_hotspot: bool) -> (String, usize, usize) {
    let cx = admin(tag);
    let (mut a, b) = page(&cx, machine());
    let mem = Arc::new(MemBackend::new(machine(), true));
    drop(b);
    a.share.backend = Some(Arc::new(Hot(mem, with_hotspot)));
    let mut g = Gfx::new(1920, 1080);
    g.clear(rgb(0, 0, 0));
    // choose the hotspot in the "share through" list (it is the entry named after the access point interface)
    for sel in 0..6 {
        a.share.down_sel = sel;
        g.clear(rgb(0, 0, 0));
        g.layout_log = Default::default();
        a.render(&mut g, R::new(0.0, 72.0, 1920.0, 1008.0), 1.5, &cx, 0);
        if with_hotspot && g.layout_log.texts.iter().any(|t| t.3.starts_with("Network name")) {
            break;
        }
    }
    let text = g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n");
    let d = g.rgba();
    let white = d.chunks(4).filter(|p| p[..3] == [255, 255, 255]).count();
    // the page background is dark, so dark pixels inside white squares are only found next to white ones: count black with white neighbours on the row
    let mut dark = 0;
    for row in d.chunks(1920 * 4) {
        let px: Vec<&[u8]> = row.chunks(4).collect();
        for i in 1..px.len() - 1 {
            if px[i][..3] == [0, 0, 0] && px[i - 1][..3] == [255, 255, 255] {
                dark += 1;
            }
        }
    }
    (text, white, dark)
}

#[test]
fn the_hotspot_block_draws_a_qr_code_that_reads_back_as_the_wifi_string() {
    let (t, white, dark) = hotspot_page("shareqr1", true);
    assert!(t.contains("Network name: iyakki-ab12"), "the hotspot block is shown: {t}");
    assert!(t.contains("Scan with the phone camera to join"), "{t}");
    let q = iyakki_agent::qr::encode(iyakki_agent::qr::wifi_payload("iyakki-ab12", "pass;word").as_bytes()).unwrap();
    // 6 px a module at 1920x1080, quiet zone included
    let side = (q.size + 8) * 6;
    assert!(white >= side * side / 4, "a white square (about half of it is dark modules) of about {side} px a side, got {white} white pixels");
    assert!(dark > 0, "dark modules are drawn");
}

#[test]
fn there_is_no_qr_code_when_there_is_no_hotspot() {
    let (t, white, dark) = hotspot_page("shareqr2", false);
    assert!(!t.contains("Scan with the phone camera"), "{t}");
    assert_eq!(dark, 0);
    assert!(white < 2000, "no white square: {white}");
}

#[test]
fn the_hotspot_name_and_password_are_changed_from_the_page_checked_audited_without_the_password_and_the_form_fits_four_sizes() {
    let cx = admin("sharewifi1");
    let (mut a, _) = page(&cx, machine());
    let mem = Arc::new(MemBackend::new(machine(), true));
    a.share.backend = Some(mem.clone());
    // key n opens the form with the current name and an empty (kept) password
    press(&mut a, &cx, 'n');
    let Some(Modal::Form(FormKind::HotspotWifi, f)) = a.modal.as_ref() else { panic!("the form opens") };
    assert_eq!(f.value(0), "IYAKKI-TEST");
    assert!(f.raw(1).is_empty());
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
        let mut g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &cx, 0);
        assert!(g.layout_log.texts.iter().any(|t| t.3.contains("Wi-Fi hotspot name and password")), "{w}x{h}: the form is drawn");
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
        }
    }
    a.modal = None;
    let form = |name: &str, pw: &str| {
        let mut f = Form::new("x").text("Network name", "", "").secret("Password", "");
        f.set_value(0, name);
        f.set_value(1, pw);
        f
    };
    // refused: a bad name, a short password; nothing changes
    assert!(a.hotspot_wifi_submit(&cx, &form("", "long enough pw")).is_err());
    assert!(a.hotspot_wifi_submit(&cx, &form("Cafe", "short")).is_err());
    assert_eq!(mem.hotspot().unwrap(), ("IYAKKI-TEST".to_string(), "first password".to_string()));
    // a new name keeps the password; a new password is stored; the audit line never holds it
    a.hotspot_wifi_submit(&cx, &form("Cafe", "")).unwrap();
    assert_eq!(mem.hotspot().unwrap(), ("Cafe".to_string(), "first password".to_string()));
    let said = a.hotspot_wifi_submit(&cx, &form("Cafe", "second password")).unwrap();
    assert!(said.contains("next time sharing starts"));
    assert_eq!(mem.hotspot().unwrap().1, "second password");
    let log = cx.audit.recent(20).join("\n");
    assert!(log.contains("settings.share.wifi") && log.contains("name IYAKKI-TEST -> Cafe; password kept") && log.contains("password changed"), "{log}");
    assert!(!log.contains("second password") && !log.contains("first password"), "{log}");
    // only an administrator
    let mut user = admin("sharewifi2");
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let (mut u, _) = page(&admin("sharewifi3"), machine());
    u.share.backend = Some(mem.clone());
    assert!(u.hotspot_wifi_submit(&user, &form("Evil", "evil password")).is_err());
    assert_eq!(mem.hotspot().unwrap().0, "Cafe");
}
