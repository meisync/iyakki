//! Settings → Time and Settings → Host name: who may change what, the zone picker, the 12/24-hour choice that is each person's own,
//! asking a time server (a fake one on this computer, never the network) with the rule about stepping back, host name rules and
//! saving that keeps the other keys of `system.conf`, the read-only window, and geometry at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use std::sync::Mutex;

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

fn plain_user(tag: &str) -> AppCx {
    let mut cx = admin(tag);
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    key(a, cx, Key::Char(c));
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| press(a, cx, c));
}
fn clear_field(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..70 {
        key(a, cx, Key::Backspace);
    }
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}
fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    for _ in 0..40 {
        if a.tab() == t {
            return;
        }
        key(a, cx, Key::Right);
    }
    panic!("no page {t:?}");
}
fn overlaps(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

/// A fake time server on this computer answering with a clock `skew` seconds ahead of ours.
fn fake_server(skew: f64) -> String {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            if n < 48 {
                continue;
            }
            let mut r = [0u8; 48];
            r[0] = 0x24;
            r[1] = 2;
            r[24..32].copy_from_slice(&buf[40..48]);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() + skew + 2_208_988_800.0;
            let mut t = [0u8; 8];
            t[..4].copy_from_slice(&(now.floor() as u64 as u32).to_be_bytes());
            t[4..].copy_from_slice(&(((now - now.floor()) * 4_294_967_296.0) as u64 as u32).to_be_bytes());
            r[32..40].copy_from_slice(&t);
            r[40..48].copy_from_slice(&t);
            let _ = sock.send_to(&r, from);
        }
    });
    format!("127.0.0.1:{}", addr.port())
}

type Calls = Arc<Mutex<Vec<f64>>>;
fn time_app(tag: &str, writable: bool, servers: Option<Vec<String>>) -> (AppCx, SettingsApp, Calls) {
    let cx = admin(tag);
    let calls: Calls = Default::default();
    let c2 = calls.clone();
    let mut a = SettingsApp::new(&cx).with_time(writable, Arc::new(move |t| {
        c2.lock().unwrap().push(t);
        Ok(())
    }), servers);
    goto(&mut a, &cx, Tab::Time);
    (cx, a, calls)
}

#[test]
fn the_time_page_is_for_everyone_and_the_host_name_page_for_administrators() {
    let cx = admin("tm-tabs");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Time) && a.tabs().contains(&Tab::HostName));
    assert_eq!((Tab::Time.group(), Tab::HostName.group()), ("System", "System"));
    let u = plain_user("tm-tabs-user");
    let b = SettingsApp::new(&u);
    assert!(b.tabs().contains(&Tab::Time), "everyone can see the time and choose their clock face");
    assert!(!b.tabs().contains(&Tab::HostName), "only administrators see the host name page");
}

#[test]
fn a_person_chooses_12_hours_and_seconds_for_themselves_only() {
    let (cx, mut a, _) = time_app("tm-face", true, None);
    let dir = cx.services.dir().to_path_buf();
    press(&mut a, &cx, 'h');
    assert_eq!(crate::uisize::load_clock(&dir, "root"), (true, false));
    assert!(status(&a).contains("12-hour"));
    press(&mut a, &cx, 's');
    assert_eq!(crate::uisize::load_clock(&dir, "root"), (true, true));
    press(&mut a, &cx, 'h');
    assert_eq!(crate::uisize::load_clock(&dir, "root"), (false, true));
    assert_eq!(crate::uisize::load_clock(&dir, "amy"), (false, false), "somebody else is unchanged");
    // an ordinary person may do it too, and nothing else on the page
    let u = plain_user("tm-face-user");
    let mut b = SettingsApp::new(&u);
    goto(&mut b, &u, Tab::Time);
    press(&mut b, &u, 'h');
    assert_eq!(crate::uisize::load_clock(u.services.dir(), "amy"), (true, false));
    press(&mut b, &u, 'n');
    assert!(status(&b).contains("administrators"), "{}", status(&b));
    assert!(!cx.services.settings.lock().unwrap().get().time.no_ntp);
}

#[test]
fn an_administrator_picks_a_zone_from_the_list_and_the_clock_follows() {
    std::env::set_var("IYAKKI_ZONEINFO", "/nonexistent");
    let (cx, mut a, _) = time_app("tm-zone", true, None);
    press(&mut a, &cx, 'z');
    assert!(a.has_modal());
    typ(&mut a, &cx, "paris");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal());
    assert_eq!(cx.services.settings.lock().unwrap().get().time.zone, "Europe/Paris");
    assert!(status(&a).contains("Europe/Paris"));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Time zone"));
    assert!(a.time_rows(&cx).iter().any(|r| r.0 == "Time zone" && r.1 == "Europe/Paris"));
    assert!(a.time_rows(&cx).iter().any(|r| r.0 == "Zones known" && r.1.contains("built-in list")), "the page says where the list comes from");
    press(&mut a, &cx, 'u');
    assert_eq!(cx.services.settings.lock().unwrap().get().time.zone, "");
    // a person without the right is told, and nothing changes
    let u = plain_user("tm-zone-user");
    let mut b = SettingsApp::new(&u);
    goto(&mut b, &u, Tab::Time);
    press(&mut b, &u, 'z');
    assert!(!b.has_modal() && status(&b).contains("administrators"));
    std::env::remove_var("IYAKKI_ZONEINFO");
}

#[test]
fn the_time_server_and_network_time_switch_are_checked_and_saved() {
    let (cx, mut a, _) = time_app("tm-ntp", true, None);
    press(&mut a, &cx, 'c');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "bad host!");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a name with a space and a bang keeps the form open");
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "ntp.example.org");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal());
    assert_eq!(cx.services.settings.lock().unwrap().get().time.ntp_server, "ntp.example.org");
    press(&mut a, &cx, 'n');
    assert!(cx.services.settings.lock().unwrap().get().time.no_ntp);
    assert!(a.time_rows(&cx).iter().any(|r| r.0 == "Network time" && r.1 == "off"));
    press(&mut a, &cx, 'n');
    assert!(!cx.services.settings.lock().unwrap().get().time.no_ntp);
    press(&mut a, &cx, 'c');
    clear_field(&mut a, &cx);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.services.settings.lock().unwrap().get().time.ntp_server, "", "empty means the usual servers");
}

#[test]
fn asking_the_server_sets_the_clock_but_never_goes_back_far_without_a_yes() {
    // 20 s behind: set
    let (cx, mut a, calls) = time_app("tm-sync", true, Some(vec![fake_server(20.0)]));
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Clock set"), "{}", status(&a));
    assert_eq!(calls.lock().unwrap().len(), 1);
    // 5 minutes ahead: left alone, and the page says how to go back
    let (cx, mut a, calls) = time_app("tm-sync-back", true, Some(vec![fake_server(-300.0)]));
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).contains("not changed") && status(&a).contains("Set the clock to the network time"), "{}", status(&a));
    assert!(calls.lock().unwrap().is_empty(), "nothing was set");
    assert!(a.time_rows(&cx).iter().any(|r| r.2 && r.1.contains("not changed")), "shown in amber on the page");
    press(&mut a, &cx, 'b');
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Clock set"), "{}", status(&a));
    assert_eq!(calls.lock().unwrap().len(), 1, "now it went back, because the administrator said so");
    // nobody home
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (cx, mut a, _) = time_app("tm-sync-none", true, Some(vec![format!("127.0.0.1:{}", silent.local_addr().unwrap().port())]));
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Could not reach"), "{}", status(&a));
}

#[test]
fn a_desktop_window_changes_nothing_of_the_machines_time() {
    let (cx, mut a, calls) = time_app("tm-window", false, Some(vec![fake_server(50.0)]));
    for c in ['z', 'u', 'n', 'c', 'y', 'b'] {
        press(&mut a, &cx, c);
        assert!(!a.has_modal() && !a.is_busy(), "{c}");
        assert!(status(&a).contains("follows the clock"), "{c}: {}", status(&a));
    }
    assert!(calls.lock().unwrap().is_empty());
    let t = cx.services.settings.lock().unwrap().get().time.clone();
    assert_eq!((t.zone.as_str(), t.no_ntp, t.ntp_server.as_str()), ("", false, ""));
    press(&mut a, &cx, 'h');
    assert!(crate::uisize::load_clock(cx.services.dir(), "root").0, "the clock face is still the person's own");
}

#[test]
fn both_pages_fit_at_four_sizes_without_overlapping_buttons() {
    for tab in [Tab::Time, Tab::HostName] {
        let cx = admin(&format!("tm-geo-{tab:?}"));
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx, tab);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = draw(&mut a, &cx, w, h);
            let buttons: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char(_))) && h.rect.h < 80.0).collect();
            assert!(buttons.len() >= 3, "{tab:?} {w}x{h}: buttons");
            for (i, b) in buttons.iter().enumerate() {
                assert!(b.rect.x >= 0.0 && b.rect.x + b.rect.w <= w as f32 && b.rect.y >= 0.0 && b.rect.y + b.rect.h <= h as f32, "{tab:?} {w}x{h} {} {:?}", b.label, b.rect);
                assert!(buttons[i + 1..].iter().all(|o| !overlaps(b.rect, o.rect)), "{tab:?} {w}x{h}: {} overlaps another button", b.label);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ host name

type Names = Arc<Mutex<Vec<String>>>;
fn host_app(tag: &str, cmdline: &str, writable: bool) -> (AppCx, SettingsApp, std::path::PathBuf, Names) {
    let cx = admin(tag);
    let root = temp_dir(&format!("{tag}-hn"));
    std::fs::write(root.join("system.conf"), "zram=2G\nswappiness=90\nsecret-future-key=1\n").unwrap();
    let names: Names = Default::default();
    let n2 = names.clone();
    let mut a = SettingsApp::new(&cx).with_host_name(root.join("system.conf"), root.join("hostname"), cmdline, writable, Arc::new(move |n, p| {
        n2.lock().unwrap().push(n.to_string());
        std::fs::write(p, format!("{n}\n")).map_err(|e| e.to_string())
    }));
    goto(&mut a, &cx, Tab::HostName);
    (cx, a, root, names)
}

#[test]
fn a_bad_name_is_refused_with_a_reason_and_a_good_one_is_applied_and_saved() {
    let (cx, mut a, root, names) = host_app("hn-edit", "", true);
    press(&mut a, &cx, 'e');
    assert!(a.has_modal());
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "Mac mini");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "capitals and spaces keep the form open with an explanation");
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "kitchen-mac");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal());
    assert!(status(&a).contains("kitchen-mac is ready"), "{}", status(&a));
    assert!(names.lock().unwrap().is_empty(), "typing does not change anything yet");
    press(&mut a, &cx, 'y');
    assert_eq!(names.lock().unwrap().as_slice(), ["kitchen-mac"]);
    assert_eq!(std::fs::read_to_string(root.join("hostname")).unwrap(), "kitchen-mac\n");
    let conf = std::fs::read_to_string(root.join("system.conf")).unwrap();
    assert!(conf.contains("hostname=kitchen-mac") && conf.contains("zram=2G") && conf.contains("swappiness=90") && conf.contains("secret-future-key=1"), "{conf}");
    assert!(status(&a).contains("kept for the next boots"), "{}", status(&a));
}

#[test]
fn save_only_keeps_the_name_for_the_next_boot_and_apply_failures_are_said() {
    let (cx, mut a, root, names) = host_app("hn-save", "", true);
    press(&mut a, &cx, 'e');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "den");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'v');
    assert!(names.lock().unwrap().is_empty(), "saving does not change the running system");
    assert!(std::fs::read_to_string(root.join("system.conf")).unwrap().contains("hostname=den"));
    assert!(status(&a).contains("next boot"));
    // nothing typed: Apply now has nothing to apply
    let (cx2, mut b, _, _) = host_app("hn-none", "", true);
    press(&mut b, &cx2, 'y');
    assert!(status(&b).contains("Press e"), "{}", status(&b));
}

#[test]
fn the_boot_option_the_window_and_ordinary_people_cannot_change_it() {
    let (cx, mut a, root, names) = host_app("hn-lock", "quiet iyakki.hostname=lab-7", true);
    press(&mut a, &cx, 'e');
    assert!(!a.has_modal() && status(&a).contains("iyakki.hostname=lab-7"), "{}", status(&a));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Change name"));
    let (cx, mut w, _, names2) = host_app("hn-window", "", false);
    press(&mut w, &cx, 'e');
    assert!(!w.has_modal() && status(&w).contains("changes nothing there"), "{}", status(&w));
    press(&mut w, &cx, 'y');
    assert!(names.lock().unwrap().is_empty() && names2.lock().unwrap().is_empty());
    assert!(!std::fs::read_to_string(root.join("system.conf")).unwrap().contains("hostname="));
}

#[test]
fn the_overview_names_the_host() {
    let cx = admin("hn-overview");
    let mut sys = crate::apps::system::SystemApp::new(&cx);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    sys.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 0);
    assert!(!crate::hostname::current("iyakki").is_empty());
}

fn zone_of(cx: &AppCx) -> iyakki_agent::settings::TimeSettings {
    cx.services.settings.lock().unwrap().get().time.clone()
}

#[test]
fn detect_automatically_is_opt_in_applies_the_detected_zone_and_a_hand_picked_zone_switches_it_off() {
    let (cx, a, _) = time_app("tm-auto", true, None);
    let asked = Arc::new(Mutex::new(0));
    let n = asked.clone();
    let answer = Arc::new(Mutex::new(Ok::<(String, &'static str), String>(("Asia/Kolkata".into(), "ipapi.co"))));
    let ans = answer.clone();
    let mut a = a.with_zone_detector(Arc::new(move || {
        *n.lock().unwrap() += 1;
        ans.lock().unwrap().clone()
    }));
    // off by default: nothing is asked, and Detect now says how to switch it on
    assert!(!zone_of(&cx).zone_auto);
    press(&mut a, &cx, 'd');
    assert!(status(&a).contains("Switch on Detect automatically first"), "{}", status(&a));
    assert_eq!(*asked.lock().unwrap(), 0, "no lookup while it is off");
    let rows: Vec<String> = a.time_rows(&cx).into_iter().map(|r| format!("{}: {}", r.0, r.1)).collect();
    assert!(rows.iter().any(|r| r.starts_with("Automatic zone: off")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("public IP address") && r.contains("ipapi.co")), "the label says what is sent: {rows:?}");
    // on: looked up at once and applied
    press(&mut a, &cx, 'a');
    assert_eq!(*asked.lock().unwrap(), 1);
    let t = zone_of(&cx);
    assert!(t.zone_auto && t.zone == "Asia/Kolkata" && t.zone_note == "ipapi.co: Asia/Kolkata", "{t:?}");
    assert!(status(&a).contains("Asia/Kolkata"));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.time") && l.contains("zone=auto")));
    // a failed lookup keeps the zone and says why
    *answer.lock().unwrap() = Err("Could not detect the time zone (ipapi.co: offline)".into());
    press(&mut a, &cx, 'd');
    assert_eq!(zone_of(&cx).zone, "Asia/Kolkata");
    assert!(zone_of(&cx).zone_note.starts_with("Could not"));
    // choosing a zone by hand makes it fixed
    a.time_pick(&cx, "Europe/Paris");
    let t = zone_of(&cx);
    assert!(!t.zone_auto && t.zone == "Europe/Paris", "{t:?}");
    // the window on someone's computer changes nothing
    let (cx2, b, _) = time_app("tm-auto-ro", false, None);
    let mut b = b.with_zone_detector(Arc::new(|| Ok(("Asia/Tokyo".into(), "ipapi.co"))));
    press(&mut b, &cx2, 'a');
    assert!(!zone_of(&cx2).zone_auto);
}

