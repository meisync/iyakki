//! The IYAKKI menu and the installer indicator through the shell: the name opens the menu, keys and clicks work and close it,
//! Quick install starts detached jobs (a stand-in script, never the real `iyakki`, no network), the indicator appears beside the
//! clock in the launcher and in an app's header without covering anything, and it opens the list of installs with live logs.

use super::tests::*;
use super::*;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn hit(sh: &Shell, label: &str) -> Option<R> {
    sh.hits().iter().find(|h| h.label == label).map(|h| h.rect)
}

/// The installer indicator's box, whatever it says.
fn indicator(sh: &Shell) -> Option<R> {
    sh.hits().iter().find(|h| h.ev == Event::Tab(crate::installpop::INDICATOR)).map(|h| h.rect)
}

fn center(r: &R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

fn press(sh: &mut Shell, r: R) {
    let (x, y) = center(&r);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
}

/// A stand-in for `iyakki quick-install`: prints steps like the real one, then waits a little.
fn fake_launch(tag: &str, secs: &str) -> iyakki_agent::quickinstall::Launch {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("iyakki-brand-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("fake-iyakki");
    std::fs::write(&p, format!("#!/bin/sh\necho \"[1/2] installing $2\"\nsleep {secs}\necho \"[2/2] done $2\"\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    iyakki_agent::quickinstall::Launch { exe: p, data: d.join("data") }
}

fn stop_all(sh: &Shell) {
    let user = sh.session.as_ref().unwrap().user.clone();
    for j in sh.env.services.jobs.list(&user) {
        let _ = sh.env.services.jobs.stop(&user, &j.id, std::time::Duration::from_millis(500));
    }
}

#[test]
fn the_name_in_the_top_bar_opens_the_menu_and_esc_a_click_outside_and_f10_close_it() {
    let (mut sh, _) = shell_with_users("brand-open");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    let name = hit(&sh, "IYAKKI menu").expect("the name is clickable");
    press(&mut sh, name);
    assert!(sh.brand.open);
    sh.render(&mut g);
    for e in sh.brand.shown() {
        assert!(hit(&sh, e.1).is_some(), "entry {}", e.1);
    }
    assert!(hit(&sh, "IYAKKI menu").is_none(), "while open only the menu takes clicks");
    // keys: Down, Enter opens a page, Esc back, Esc closes
    key(&mut sh, Key::Down);
    key(&mut sh, Key::Enter);
    assert_eq!(sh.brand.page, crate::brandmenu::Page::Install);
    assert!(!sh.brand.presence.is_empty(), "opening Quick install read what is installed");
    key(&mut sh, Key::Esc);
    assert!(sh.brand.open && sh.brand.page == crate::brandmenu::Page::Menu);
    key(&mut sh, Key::Esc);
    assert!(!sh.brand.open);
    assert_eq!(sh.screen_name(), "launcher", "Esc closed the menu, it did not sign out");
    // a click outside closes it
    key(&mut sh, Key::F(10));
    sh.render(&mut g);
    assert!(sh.brand.open);
    sh.handle(&Event::MouseDown(1200, 700));
    assert!(!sh.brand.open);
    // F10 opens it inside an app too, under the header
    assert!(sh.open_for_demo("chat"));
    key(&mut sh, Key::F(10));
    sh.render(&mut g);
    assert!(sh.brand.open && sh.brand.panel(1280, 720, scale_for(1280, 720)).y >= HEADER_H * scale_for(1280, 720));
    key(&mut sh, Key::F(10));
    assert!(!sh.brand.open);
    assert_eq!(sh.screen_name(), "app:chat");
}

#[test]
fn quick_install_starts_jobs_the_indicator_shows_them_beside_the_clock_and_opens_their_logs() {
    let (mut sh, _) = shell_with_users("brand-install");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.quick_launch = Some(fake_launch("install", "15"));
    sh.render(&mut g);
    assert!(indicator(&sh).is_none(), "no indicator while nothing installs");
    key(&mut sh, Key::F(10));
    key(&mut sh, Key::Char('q'));
    // Claude Code (its runtime, Node.js, first when the machine has none)
    let row = iyakki_agent::quickinstall::Item::ALL.iter().position(|i| *i == iyakki_agent::quickinstall::Item::ClaudeCode).unwrap();
    sh.handle(&Event::Tab(crate::brandmenu::IROW + row));
    key(&mut sh, Key::Enter);
    let note = sh.brand.note.clone().unwrap();
    assert!(note.1 && note.0.contains("Claude Code ("), "the job says what it installs (after Node.js when that is missing): {note:?}");
    assert!(sh.env.audit.recent(5).iter().any(|l| l.contains("quickinstall.start") && l.contains("claude-code")));
    // a second start of the same thing is refused while it runs
    key(&mut sh, Key::Enter);
    assert!(sh.brand.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("being installed already")), "{:?}", sh.brand.note);
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Esc);
    // the indicator, beside the clock, apart from it and from the name
    sh.render(&mut g);
    let ind = indicator(&sh).expect("the indicator");
    assert!(sh.hits().iter().any(|h| h.label.contains("install") && h.label.contains("running")), "it says what it is");
    let clock = hit(&sh, "clock").expect("the clock");
    assert!(ind.x + ind.w <= clock.x + 0.5, "left of the clock: {ind:?} {clock:?}");
    assert!((ind.y + ind.h / 2.0 - (clock.y + clock.h / 2.0)).abs() < 12.0, "on the clock's line");
    // it animates: frames are asked for while the job runs
    let mut due = false;
    for _ in 0..12 {
        sh.handle(&Event::Tick);
        due |= sh.take_stale();
    }
    assert!(due, "the ring turns");
    // a click opens the list of installs; Log shows the live output; Esc closes the log, then the list
    press(&mut sh, ind);
    assert!(sh.installs.open);
    sh.render(&mut g);
    let t0 = std::time::Instant::now();
    while !sh.installs.jobs().first().is_some_and(|j| j.last_line.contains("installing")) {
        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "the job never printed");
        std::thread::sleep(std::time::Duration::from_millis(100));
        sh.handle(&Event::Tick);
        if let Some(cx) = sh.cx() {
            sh.installs.poll(&cx, true);
        }
    }
    sh.render(&mut g);
    let log = hit(&sh, "Log").expect("a Log button on the row");
    press(&mut sh, log);
    assert!(sh.installs.log_open());
    sh.render(&mut g);
    assert!(hit(&sh, "Close").is_some(), "the log view is drawn");
    key(&mut sh, Key::Esc);
    assert!(sh.installs.open && !sh.installs.log_open(), "Esc closes the log first");
    key(&mut sh, Key::Esc);
    assert!(!sh.installs.open);
    // inside an app the indicator sits left of the header's clock
    assert!(sh.open_for_demo("chat"));
    sh.render(&mut g);
    let ind = indicator(&sh).expect("the indicator in the app header");
    let clock = hit(&sh, "clock").expect("the header clock");
    assert!(ind.x + ind.w <= clock.x + 0.5 && ind.y + ind.h <= HEADER_H * scale_for(1280, 720) + 0.5);
    stop_all(&sh);
}

#[test]
fn the_indicator_fits_with_every_header_control_at_four_sizes() {
    let (mut sh, _) = shell_with_users("brand-fit");
    login(&mut sh, "root", "rootpass12");
    sh.quick_launch = Some(fake_launch("fit", "20"));
    key(&mut sh, Key::F(10));
    key(&mut sh, Key::Char('q'));
    let row = iyakki_agent::quickinstall::Item::ALL.iter().position(|i| *i == iyakki_agent::quickinstall::Item::Copilot).unwrap();
    sh.handle(&Event::Tab(crate::brandmenu::IROW + row));
    key(&mut sh, Key::Char('i'));
    key(&mut sh, Key::F(10));
    if let Some(cx) = sh.cx() {
        sh.installs.poll(&cx, true);
    }
    for app in ["launcher", "chat"] {
        if app == "chat" {
            assert!(sh.open_for_demo("chat"));
        }
        for (w, h) in SIZES {
            let mut g = Gfx::new(w, h);
            sh.resize_for_test(w, h);
            sh.render(&mut g);
            let Some(ind) = indicator(&sh) else {
                assert!(w <= 1024, "{app} {w}x{h}: only a narrow screen may hide the indicator");
                continue;
            };
            assert!(ind.x >= 0.0 && ind.x + ind.w <= w as f32, "{app} {w}x{h}");
            for other in sh.hits().iter().filter(|x| x.ev != Event::Tab(crate::installpop::INDICATOR) && x.rect.y < 80.0 * scale_for(w, h) && x.rect.h < 80.0 * scale_for(w, h)) {
                let r = other.rect;
                let apart = ind.x + ind.w <= r.x + 0.5 || r.x + r.w <= ind.x + 0.5 || ind.y + ind.h <= r.y + 0.5 || r.y + r.h <= ind.y + 0.5;
                assert!(apart, "{app} {w}x{h}: the indicator covers {} {r:?} ({ind:?})", other.label);
            }
        }
    }
    stop_all(&sh);
}

#[test]
fn an_ordinary_account_cannot_quick_install_and_the_media_page_checks_the_file() {
    let (mut sh, _) = shell_with_users("brand-user");
    login(&mut sh, "alice", "alicepass1");
    sh.quick_launch = Some(fake_launch("user", "1"));
    key(&mut sh, Key::F(10));
    key(&mut sh, Key::Char('q'));
    key(&mut sh, Key::Enter);
    assert!(sh.brand.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")), "{:?}", sh.brand.note);
    assert!(iyakki_agent::quickinstall::install_jobs(&sh.env.services.jobs, "alice").is_empty());
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Char('m'));
    assert_eq!(sh.brand.page, crate::brandmenu::Page::Media);
    for c in "/no/such/file.mp4".chars() {
        key(&mut sh, Key::Char(c));
    }
    key(&mut sh, Key::Enter);
    assert!(sh.brand.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("No such file")), "{:?}", sh.brand.note);
    // the Mac page asks the Devices reader and leads to STORE -> Drivers
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Char('d'));
    assert_eq!(sh.brand.page, crate::brandmenu::Page::Mac);
    key(&mut sh, Key::Enter);
    assert!(!sh.brand.open, "Enter left for STORE \u{2192} Drivers");
}

#[test]
fn the_clock_is_visible_in_an_app_header_not_painted_over_by_the_header_band() {
    let (mut sh, _) = shell_with_users("brand-clock");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    assert!(sh.open_for_demo("chat"));
    sh.render(&mut g);
    let c = hit(&sh, "clock").expect("the header clock");
    assert_eq!(sh.hits().iter().filter(|h| h.label == "clock").count(), 1, "one click region");
    let mut bright = 0;
    for y in c.y as u32..(c.y + c.h) as u32 {
        for x in c.x as u32..(c.x + c.w) as u32 {
            let o = ((y * g.width() + x) * 4) as usize;
            let p = &g.rgba()[o..o + 3];
            if p.iter().all(|v| *v > 180) {
                bright += 1;
            }
        }
    }
    assert!(bright > 30, "the time's text is drawn on top: {bright} bright pixels");
}
