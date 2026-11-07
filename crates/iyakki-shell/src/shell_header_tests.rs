//! The top bar's clock and Devices button: they fit with every other header control at four screen sizes, the clock reads
//! right in 12/24 hours, zones and daylight saving, warns about a clock that was never set, and the clicks and keys lead to
//! the right places.

use super::tests::*;
use super::*;
use iyakki_agent::clock::days_from_civil;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn at(y: i64, m: u32, d: u32, h: i64, min: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
}

fn hit_rect(sh: &Shell, label: &str) -> Option<R> {
    sh.hits().iter().find(|h| h.label == label).map(|h| h.rect)
}

fn disjoint(a: R, b: R) -> bool {
    a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5
}

/// A made-up machine for the popover: one Ethernet port with a cable.
pub(super) fn small_probe(tag: &str) -> crate::devpop::Probe {
    let d = std::env::temp_dir().join(format!("iyakki-hdr-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (rel, c) in [("sys/class/net/eth0/device/x", ""), ("sys/class/net/eth0/type", "1\n"), ("sys/class/net/eth0/flags", "0x1003\n"), ("sys/class/net/eth0/carrier", "1\n"), ("sys/class/net/eth0/address", "aa:bb:cc:00:00:01\n")] {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    }
    let roots = iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") };
    crate::devpop::Probe {
        dev: crate::apps::devices::DeviceEnv::real(),
        hw: iyakki_hwinfo::Roots { sys: roots.sys.clone(), proc: roots.proc_.clone() },
        data_dir: d.clone(),
        bt_dir: d.join("bt"),
        drivers: None,
        hostname: "iyakki-test".into(),
        volatile: false,
        screen: (1280, 720),
        wifi: Arc::new(|_| None),
        can_scan: false,
    }
    .with_roots(roots)
}

impl crate::devpop::Probe {
    fn with_roots(mut self, r: iyakki_devctl::Roots) -> Self {
        self.dev.roots = r;
        self
    }
}

fn signed_in(tag: &str) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.screen_name(), "launcher");
    sh.clock.fixed = Some(at(2026, 10, 17, 14, 5));
    sh.devprobe = Some(small_probe(tag));
    // the tests expect UTC, whatever zone the computer running them is in
    sh.env.services.settings.lock().unwrap().update(|s| s.time.zone = "UTC".into()).unwrap();
    (sh, Gfx::new(1280, 720))
}

fn resize(g: &mut Gfx, w: u32, h: u32) {
    *g = Gfx::new(w, h);
}

#[test]
fn the_launcher_bar_fits_the_clock_and_the_devices_button_at_four_sizes() {
    for who in [("root", "rootpass12"), ("gus", "guestpass1")] {
        let (mut sh, _) = shell_with_users(&format!("hdr-launch-{}", who.0));
        login(&mut sh, who.0, who.1);
        sh.clock.fixed = Some(at(2026, 10, 17, 14, 5));
        for (w, h) in SIZES {
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            let s = scale_for(w, h);
            let power = sh.session.as_ref().unwrap().policy.power;
            let bell = sh.inbox_allowed();
            let _ = (power, bell);
            let n = sh.bar_slots().count; // logout, camera, devices, clipboard (+ power, bell, panes) and the speaker and Settings
            let buttons: Vec<R> = (0..n).map(|i| header_button(w, s, i)).collect();
            for (i, b) in buttons.iter().enumerate() {
                assert!(b.x >= 0.0 && b.x + b.w <= w as f32 && b.y >= 0.0 && b.y + b.h <= 90.0 * s, "{who:?} {w}x{h} button {i} {b:?}");
                assert!(buttons[i + 1..].iter().all(|o| disjoint(*b, *o)), "{w}x{h} buttons overlap");
                assert!(b.w >= 40.0 * s && b.h >= 40.0 * s, "{w}x{h}: big enough to hit");
            }
            let clock = hit_rect(&sh, "clock").unwrap_or_else(|| panic!("{who:?} {w}x{h}: the clock is shown"));
            assert!(clock.x >= 0.0 && clock.x + clock.w <= w as f32 && clock.y >= 0.0 && clock.y + clock.h <= 90.0 * s, "{w}x{h} clock {clock:?}");
            let leftmost = buttons.last().unwrap();
            assert!(clock.x + clock.w <= leftmost.x - 40.0 * s, "{w}x{h}: the clock {clock:?} sits left of the account chip, which is left of {leftmost:?}");
            assert!(clock.x > 40.0 * s + 100.0 * s, "{w}x{h}: and right of the name IYAKKI");
            assert!(clock.h >= 30.0 * s);
        }
    }
}

#[test]
fn the_app_bar_fits_the_clock_and_the_devices_button_with_the_other_controls() {
    let (mut sh, _) = signed_in("hdr-app");
    for (w, h) in SIZES {
        let s = scale_for(w, h);
        let mut g = Gfx::new(w, h);
        sh.screen = Screen::Launcher;
        assert!(sh.open_for_demo("settings"));
        sh.render(&mut g);
        let clock = hit_rect(&sh, "clock").unwrap_or_else(|| panic!("{w}x{h}: the clock is shown in an app"));
        let rects = [("devices", dev_rect(w, s)), ("screenshot", shot_rect(w, s)), ("clipboard", clip_rect(w, s)), ("inbox", bell_rect(w, s)), ("help", help_rect(w, s)), ("back", back_rect(w, s)), ("clock", clock)];
        for (i, (na, a)) in rects.iter().enumerate() {
            assert!(a.x >= 0.0 && a.x + a.w <= w as f32 && a.y >= 0.0 && a.y + a.h <= HEADER_H * s, "{w}x{h} {na} {a:?} inside the header");
            for (nb, b) in &rects[i + 1..] {
                assert!(disjoint(*a, *b), "{w}x{h}: {na} {a:?} overlaps {nb} {b:?}");
            }
        }
        assert!(hit_rect(&sh, "devices").is_some_and(|r| r == dev_rect(w, s)), "{w}x{h}: the button is clickable");
        assert!(dev_rect(w, s).w >= 30.0 * s && dev_rect(w, s).h >= 28.0 * s, "same size as its neighbours");
        assert_eq!((dev_rect(w, s).w, dev_rect(w, s).h), (shot_rect(w, s).w, shot_rect(w, s).h));
        assert!(clock.x > 60.0 * s + 120.0 * s, "{w}x{h}: leaves room for the app's name");
    }
    // very narrow: the clock gives way instead of crowding the name and the buttons
    let (w, h) = (420u32, 300u32);
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    let s = scale_for(w, h);
    if let Some(c) = hit_rect(&sh, "clock") {
        assert!(c.x >= 60.0 * s + 120.0 * s && disjoint(c, bell_rect(w, s)), "{c:?}");
    }
}

#[test]
fn the_clock_reads_24_or_12_hours_with_or_without_seconds_in_each_persons_own_choice() {
    let (mut sh, mut g) = signed_in("hdr-fmt");
    sh.render(&mut g);
    assert_eq!(sh.clock.time_text(), "14:05");
    assert_eq!(sh.clock.date_text(), "Sat 17 Oct");
    let dir = sh.env.services.dir().to_path_buf();
    crate::uisize::save_clock(&dir, "root", true, true);
    sh.render(&mut g);
    assert_eq!(sh.clock.time_text(), "2:05:00 PM");
    crate::uisize::save_clock(&dir, "root", false, false);
    sh.render(&mut g);
    assert_eq!(sh.clock.time_text(), "14:05");
    // another person's choice is their own
    assert_eq!(crate::uisize::load_clock(&dir, "alice"), (false, false));
}

#[test]
fn the_zone_shifts_the_time_and_daylight_saving_follows_the_calendar() {
    std::env::set_var("IYAKKI_ZONEINFO", "/nonexistent"); // the built-in table, as on an image without zone data
    let (mut sh, mut g) = signed_in("hdr-zone");
    let ticks = std::cell::Cell::new(10_000u64);
    let set_zone = |sh: &mut Shell, z: &str| {
        sh.env.services.settings.lock().unwrap().update(|s| s.time.zone = z.into()).unwrap();
        ticks.set(ticks.get() + 100);
        sh.clock.refresh(ticks.get(), sh.env.services.settings.lock().map(|s| s.get().time.clone()).ok().as_ref(), None);
    };
    set_zone(&mut sh, "Asia/Kolkata");
    sh.render(&mut g);
    assert_eq!(sh.clock.time_text(), "19:35", "UTC 14:05 is 19:35 in India");
    set_zone(&mut sh, "Europe/Paris");
    sh.clock.fixed = Some(at(2026, 7, 1, 12, 0));
    assert_eq!(sh.clock.time_text(), "14:00", "summer time, UTC+2");
    sh.clock.fixed = Some(at(2026, 12, 1, 12, 0));
    assert_eq!(sh.clock.time_text(), "13:00", "winter time, UTC+1");
    sh.clock.fixed = Some(at(2026, 3, 29, 0, 59));
    assert_eq!(sh.clock.time_text(), "01:59");
    sh.clock.fixed = Some(at(2026, 3, 29, 1, 0));
    assert_eq!(sh.clock.time_text(), "03:00", "the clocks went forward");
    let tip = sh.clock.tooltip();
    assert!(tip[0].0.contains("2026") && tip[1].0.contains("Europe/Paris") && tip[1].0.contains("UTC+02:00"), "{tip:?}");
    set_zone(&mut sh, "");
    std::env::remove_var("IYAKKI_ZONEINFO");
}

#[test]
fn a_clock_that_was_never_set_says_so() {
    let (mut sh, mut g) = signed_in("hdr-wrong");
    sh.render(&mut g);
    assert!(!sh.clock.looks_wrong() && !clock_time_text(&sh.clock).ends_with('!'));
    sh.clock.fixed = Some(40 * 60 + 3600);
    sh.render(&mut g);
    assert!(sh.clock.looks_wrong());
    assert!(clock_time_text(&sh.clock).ends_with(" !"), "{}", clock_time_text(&sh.clock));
    let tip = sh.clock.tooltip();
    assert!(tip.iter().any(|(t, warn)| *warn && t.contains("looks wrong") && t.contains("clock battery")), "{tip:?}");
    // the same warning appears on the page in Settings
    assert!(hit_rect(&sh, "clock").is_some(), "the clock is still shown, with the mark");
}

#[test]
fn hovering_the_clock_shows_its_tooltip_and_clicking_or_f7_opens_the_time_page() {
    let (mut sh, mut g) = signed_in("hdr-click");
    sh.render(&mut g);
    let c = hit_rect(&sh, "clock").unwrap();
    let mid = ((c.x + c.w / 2.0) as i32, (c.y + c.h / 2.0) as i32);
    let before = g.rgba().to_vec();
    sh.handle(&Event::MouseMove(mid.0, mid.1));
    assert!(sh.take_stale(), "hovering the clock changes the picture (the tooltip)");
    sh.render(&mut g);
    assert_ne!(before, g.rgba(), "the tooltip is drawn");
    sh.handle(&Event::MouseDown(mid.0, mid.1));
    assert_eq!(sh.screen_name(), "app:settings");
    sh.render(&mut g);
    assert!(hit_rect(&sh, "12/24 hour").is_some(), "the Time page: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    // F7 does the same from the launcher
    sh.screen = Screen::Launcher;
    key(&mut sh, Key::F(7));
    assert_eq!(sh.screen_name(), "app:settings");
}

#[test]
fn the_clock_redraws_only_when_the_minute_changes() {
    let (mut sh, mut g) = signed_in("hdr-tick");
    sh.render(&mut g);
    sh.take_stale();
    sh.clock.fixed = Some(at(2026, 10, 17, 14, 5));
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    sh.take_stale();
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(!sh.take_stale(), "an idle launcher does not repaint every second");
    sh.clock.fixed = Some(at(2026, 10, 17, 14, 6));
    for _ in 0..16 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.take_stale(), "the new minute is drawn");
}

/// Tick until the popover has its first reading from the background thread.
pub(super) fn wait_for_devices(sh: &mut Shell) {
    for _ in 0..400 {
        sh.handle(&Event::Tick);
        if sh.devpop.snapshot().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the Devices popover never got its reading");
}

#[test]
fn the_devices_button_and_f8_open_and_close_the_popover_and_it_leads_to_settings() {
    let (mut sh, mut g) = signed_in("hdr-dev");
    for (w, h) in SIZES {
        resize(&mut g, w, h);
        sh.size = (w, h);
        if sh.devpop.open {
            key(&mut sh, Key::Esc);
        }
        sh.render(&mut g);
        let s = scale_for(w, h);
        let power = sh.session.as_ref().unwrap().policy.power;
        let b = header_button(w, s, if power { 3 } else { 2 });
        // the button opens it
        sh.handle(&Event::MouseDown((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
        assert!(sh.devpop.open, "{w}x{h}: the button opens the popover");
        wait_for_devices(&mut sh);
        sh.render(&mut g);
        assert!(hit_rect(&sh, "close").is_some() && sh.hits().iter().any(|h| h.label.starts_with("Ethernet")), "{w}x{h}: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
        let panel = crate::devpop::DevPop::panel(w, h, s, sh.devpop.snapshot().unwrap().items.len());
        assert!(panel.x + panel.w <= w as f32 + 0.5 && panel.y + panel.h <= h as f32 + 0.5);
        // Esc closes it; F8 opens it again and closes it again
        key(&mut sh, Key::Esc);
        assert!(!sh.devpop.open);
        key(&mut sh, Key::F(8));
        assert!(sh.devpop.open);
        key(&mut sh, Key::F(8));
        assert!(!sh.devpop.open);
        // a click outside closes it
        key(&mut sh, Key::F(8));
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(2, h as i32 - 2));
        assert!(!sh.devpop.open, "{w}x{h}: a click outside closes it");
        assert_eq!(sh.screen_name(), "launcher");
    }
    // Enter on the Ethernet row opens Settings on the Network page
    sh.size = (1280, 720);
    resize(&mut g, 1280, 720);
    key(&mut sh, Key::F(8));
    wait_for_devices(&mut sh);
    sh.render(&mut g);
    key(&mut sh, Key::Enter);
    assert_eq!(sh.screen_name(), "app:settings");
    assert!(!sh.devpop.open);
    sh.render(&mut g);
    assert!(hit_rect(&sh, "Turn on/off").is_some(), "the Network page: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
}

#[test]
fn the_devices_button_works_inside_an_app_and_driver_help_opens_the_store() {
    let (mut sh, mut g) = signed_in("hdr-dev-app");
    assert!(sh.open_for_demo("calendar"));
    sh.render(&mut g);
    let (w, h) = (1280u32, 720u32);
    let s = scale_for(w, h);
    let b = dev_rect(w, s);
    sh.handle(&Event::MouseDown((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
    assert!(sh.devpop.open, "the app header's button opens it");
    wait_for_devices(&mut sh);
    sh.render(&mut g);
    // the made-up machine has no Wi-Fi: Driver help is offered, and it opens STORE on the Drivers chip
    assert!(hit_rect(&sh, "Driver help").is_some(), "{:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    key(&mut sh, Key::Char('d'));
    assert_eq!(sh.screen_name(), "app:store");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| matches!(h.ev, Event::Tab(226)) && h.label.starts_with("Drivers")), "the Drivers chip is there");
    assert!(!sh.devpop.open);
    // not signed in: the keys do nothing
    let (mut out, _) = shell_with_users("hdr-dev-out");
    key(&mut out, Key::F(8));
    key(&mut out, Key::F(7));
    assert!(!out.devpop.open);
    assert_eq!(out.screen_name(), "login");
}

#[test]
fn the_popover_takes_its_own_keys_and_gives_the_rest_back_when_closed() {
    let (mut sh, mut g) = signed_in("hdr-keys");
    key(&mut sh, Key::F(8));
    wait_for_devices(&mut sh);
    sh.render(&mut g);
    // letters meant for the launcher (o = sign out) do nothing while the popover is open
    key(&mut sh, Key::Char('o'));
    assert_eq!(sh.screen_name(), "launcher");
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Char('o'));
    assert_eq!(sh.screen_name(), "login", "and work again once it is closed");
}

/// The MacBookPro14,1 case: the buffer is 2880x1800 but the monitor shows its top-left 2560x1600. The shell lays out for the
/// visible part, so Back, the clock and the other controls are on screen, and the rest of the buffer stays black.
#[test]
fn a_shell_for_the_visible_part_of_a_larger_buffer_keeps_back_on_screen_and_the_rest_clear() {
    let (mut sh, _) = signed_in("hdr-visible");
    let (bw, bh, vw, vh) = (2880usize, 1800usize, 2560usize, 1600usize);
    let mut mem = vec![0xABu8; bw * bh * 4];
    crate::display::clear_outside(&mut mem, bw * 4, bh, vw, vh, 4);
    // the rest of the buffer is black at once; the visible part is untouched until drawn
    assert!(mem[(vw * 4)..(bw * 4)].iter().all(|b| *b == 0), "right strip of the first row cleared");
    assert!(mem[vh * bw * 4..].iter().all(|b| *b == 0), "rows below the visible part cleared");
    assert!(mem[..vw * 4].iter().all(|b| *b == 0xAB), "visible part left alone");
    let mut fb = crate::fb::Framebuffer::in_memory_visible(&mut mem, (bw as u32, bh as u32), (vw as u32, vh as u32));
    let (w, h) = (fb.width, fb.height);
    assert_eq!((w, h), (2560, 1600));
    let mut g = Gfx::new(w, h);
    sh.screen = Screen::Launcher;
    assert!(sh.open_for_demo("settings"));
    sh.render(&mut g);
    let s = scale_for(w, h);
    for (name, r) in [("back", back_rect(w, s)), ("devices", dev_rect(w, s)), ("help", help_rect(w, s)), ("screenshot", shot_rect(w, s))] {
        assert!(r.x >= 0.0 && r.x + r.w <= vw as f32 && r.y >= 0.0 && r.y + r.h <= vh as f32, "{name} {r:?} inside the visible 2560x1600");
    }
    let clock = hit_rect(&sh, "clock").expect("clock");
    assert!(clock.x + clock.w <= vw as f32);
    let mut old = Vec::new();
    fb.present_banded(g.rgba(), &mut old).unwrap();
    // the Back button's pixels differ from the cleared black and are inside the visible rectangle
    let b = back_rect(w, s);
    let (bx, by) = ((b.x + b.w / 2.0) as usize, (b.y + b.h / 2.0) as usize);
    let at = (by * bw + bx) * 4;
    assert!(mem[at..at + 3].iter().any(|v| *v != 0), "Back is drawn at ({bx},{by})");
    assert!(mem[(vw * 4)..(bw * 4)].iter().all(|v| *v == 0), "nothing drawn right of the visible part");
    assert!(mem[vh * bw * 4..].iter().all(|v| *v == 0), "nothing drawn below it");
    // Esc leaves the app, with the pointer or without
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_name(), "launcher", "Esc is the keyboard way back");
}

#[test]
fn the_shell_feeds_the_devices_popover_tooltip_and_redraws_when_it_appears() {
    let (mut sh, mut g) = signed_in("hdr-tip");
    let (w, h) = (2560u32, 1600u32);
    resize(&mut g, w, h);
    sh.size = (w, h);
    key(&mut sh, Key::F(8));
    assert!(sh.devpop.open);
    wait_for_devices(&mut sh);
    // a made-up reading with a row whose detail the popover has to cut
    let detail = "wlan0 · a-rather-long-network-name-at-home-and-more · signal -43 dBm (excellent) · 192.168.50.76 · hardware address aa:bb:cc:dd:ee:ff · channel 36 (5 GHz) · 866 Mb/s link · connected for 3 hours";
    let item = crate::devpop::Item { title: "Wi-Fi".into(), detail: detail.into(), level: crate::devpop::Level::Ok, jump: crate::devpop::Jump::None, acts: Vec::new() };
    sh.devpop.set_snapshot(crate::devpop::Snapshot { items: vec![item], driver_help: Vec::new(), at: 9 });
    sh.render(&mut g);
    let row = sh.hits().iter().find(|x| x.label.starts_with("Wi-Fi: ")).map(|x| x.rect).expect("the row");
    let (x, y) = ((row.x + row.w * 0.3) as i32, (row.y + row.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.render(&mut g);
    let _ = sh.take_stale();
    for i in 0..crate::tooltip::POINTER_DELAY {
        sh.handle(&Event::Tick);
        if i + 1 < crate::tooltip::POINTER_DELAY {
            sh.render(&mut g);
            assert!(sh.devpop.tip_text().is_none(), "not before the delay (tick {i})");
        }
    }
    assert!(sh.take_stale(), "the shell is told to draw again with no further input");
    sh.render(&mut g);
    assert_eq!(sh.devpop.tip_text(), Some(format!("Wi-Fi\n{detail}").as_str()));
    let t = sh.devpop.tip_rect.unwrap();
    assert!(t.x >= 0.0 && t.y >= 0.0 && t.x + t.w <= w as f32 && t.y + t.h <= h as f32);
    // a move hides it and asks for a redraw
    sh.handle(&Event::MouseMove(x + 3, y));
    assert!(sh.take_stale());
    sh.render(&mut g);
    assert!(sh.devpop.tip_text().is_none());
}
