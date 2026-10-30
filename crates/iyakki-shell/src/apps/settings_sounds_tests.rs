//! Settings -> System sounds: the master switch (on by default), the level (-12 dB by default), a switch and a Preview for
//! each cue, who may change them, that the old settings load, that a preview goes through the play command, and the page
//! at four sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::syssounds::{self as ss, Cue};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto(a: &mut SettingsApp, _cx: &AppCx, t: Tab) {
    a.go_to(t);
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
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn sounds(cx: &AppCx) -> ss::SystemSounds {
    cx.services.system_sounds()
}

#[test]
fn the_page_sits_next_to_sound_for_administrators_and_starts_on_with_a_quiet_level() {
    let cx = admin("sysnd-tab");
    let a = SettingsApp::new(&cx);
    let t = a.tabs().to_vec();
    let i = t.iter().position(|x| *x == Tab::Sound).unwrap();
    assert_eq!(t[i + 1], Tab::SystemSounds);
    assert_eq!((Tab::SystemSounds.label(), Tab::SystemSounds.group()), ("System sounds", "Display & sound"));
    assert_eq!(Tab::SystemSounds.cloud_page(), Some("sound"), "hidden in cloud mode with the Sound page");
    let d = sounds(&cx);
    assert!(d.enabled && d.offset_db == -12 && Cue::ALL.iter().all(|c| d.get(*c)));
    // an ordinary user has no hardware pages at all
    let mut u = admin("sysnd-user");
    u.session.role = iyakki_sec::policy::Role::User;
    u.session.policy = iyakki_sec::policy::Role::User.template();
    u.session.user = "alice".into();
    assert!(!SettingsApp::new(&u).tabs().contains(&Tab::SystemSounds));
}

#[test]
fn the_switches_the_level_and_the_slider_are_saved_and_audited() {
    let cx = admin("sysnd-set");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SystemSounds);
    // row 0: the master switch
    key(&mut a, &cx, Key::Enter);
    assert!(!sounds(&cx).enabled && status(&a) == "System sounds are off.", "{}", status(&a));
    key(&mut a, &cx, Key::Char(' '));
    assert!(sounds(&cx).enabled);
    // the level: + and - move it by 3 dB, within -30..0
    key(&mut a, &cx, Key::Char('+'));
    assert_eq!(sounds(&cx).offset_db, -9);
    for _ in 0..20 {
        key(&mut a, &cx, Key::Char('+'));
    }
    assert_eq!(sounds(&cx).offset_db, 0, "never louder than IYAKKI's volume");
    for _ in 0..20 {
        key(&mut a, &cx, Key::Char('-'));
    }
    assert_eq!(sounds(&cx).offset_db, -30);
    // a click on the slider track
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "slider"));
    let w = a.slider_w.iter().find(|(i, _)| *i == SLIDER_SYSND).unwrap().1;
    a.handle(&Event::AreaAt(SLIDER_SYSND, w / 2.0, 0.0), &cx);
    assert_eq!(sounds(&cx).offset_db, -15);
    a.handle(&Event::AreaAt(SLIDER_SYSND, w, 0.0), &cx);
    assert_eq!(sounds(&cx).offset_db, 0);
    // the cues: rows 2 to 5 in the order login, logout, notification, job done
    for (i, c) in Cue::ALL.iter().enumerate() {
        a.handle(&Event::Row(2 + i), &cx);
        key(&mut a, &cx, Key::Enter);
        assert!(!sounds(&cx).get(*c), "{c:?} off");
        key(&mut a, &cx, Key::Enter);
        assert!(sounds(&cx).get(*c), "{c:?} on again");
    }
    // clicks on the switches
    let g = draw(&mut a, &cx, 1280, 720);
    let hit = g.hits.iter().find(|h| h.label == "Signing out").unwrap().ev.clone();
    a.handle(&hit, &cx);
    assert!(!sounds(&cx).logout && sounds(&cx).login);
    let log = std::fs::read_to_string(cx.audit.path()).unwrap();
    assert!(log.contains("settings.sysnd") && log.contains("settings.sysnd.level"), "{log}");
    // kept: a new settings store reads the same
    let d = sounds(&cx);
    assert_eq!(cx.services.settings.lock().unwrap().get().sounds, d);
}

#[test]
fn only_administrators_change_them() {
    let mut cx = admin("sysnd-deny");
    cx.session.policy.manage_settings = false;
    let mut a = SettingsApp::new(&cx);
    a.tabs = vec![Tab::SystemSounds]; // as if reached some other way
    a.tab = 0;
    let before = sounds(&cx);
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Char('+'));
    a.handle(&Event::AreaAt(SLIDER_SYSND, 1.0, 0.0), &cx);
    assert_eq!(sounds(&cx), before);
    assert!(status(&a).contains("administrator"), "{}", status(&a));
}

#[test]
fn preview_plays_the_cue_through_the_play_command_and_says_so() {
    let cx = admin("sysnd-preview");
    // the cue goes through the configured play command; the mixer (on by default) would take it instead
    cx.services.update_sound(|s| s.mixer = false).unwrap();
    let out = temp_dir("sysnd-preview-out").join("played.wav");
    cx.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = format!("sh -c 'cat > {}'", out.display())).unwrap();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SystemSounds);
    // the Preview button of the "Signing out" row
    let g = draw(&mut a, &cx, 1280, 720);
    let previews: Vec<_> = g.hits.iter().filter(|h| h.label == "Preview" && matches!(h.ev, Event::Sub(_, Key::Char('P')))).collect();
    assert_eq!(previews.len(), 4, "a Preview for each cue");
    let ev = previews[1].ev.clone();
    a.handle(&ev, &cx);
    settle(&mut a, &cx);
    assert_eq!(status(&a), "Played the signing out sound.");
    let wav = std::fs::read(&out).unwrap();
    let parsed = iyakki_agent::voice::parse_wav(&wav).unwrap();
    assert_eq!(parsed.rate, 44_100);
    assert_eq!(parsed.samples.len(), ss::render(Cue::Logout, -12).len(), "the logout cue at the level of the settings");
    // with the master switch off the preview still plays (the person asked to hear it)
    key(&mut a, &cx, Key::Home);
    a.sel[a.tab] = 0;
    key(&mut a, &cx, Key::Enter);
    assert!(!sounds(&cx).enabled);
    key(&mut a, &cx, Key::Char('p'));
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Played the"), "{}", status(&a));
    // muted: it says so instead of playing
    cx.services.update_sound(|s| s.muted = true).unwrap();
    key(&mut a, &cx, Key::Char('p'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("muted"), "{}", status(&a));
    // a play command that fails is reported
    cx.services.update_sound(|s| s.muted = false).unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.voice.play_cmd = "sh -c 'exit 3'".into()).unwrap();
    key(&mut a, &cx, Key::Char('p'));
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("The sound did not play"), "{}", status(&a));
}

#[test]
fn the_page_is_on_screen_at_four_sizes_and_says_when_sound_is_muted() {
    let cx = admin("sysnd-draw");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SystemSounds);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for sel in 0..sysnd::ROWS {
            a.sel[a.tab] = sel;
            let g = draw(&mut a, &cx, w, h);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{} is off screen at {w}x{h} (row {sel}): {r:?}", hit.label);
            }
            // the selected row is on the page
            let want = match sel {
                0 => "System sounds",
                1 => "level",
                n => Cue::ALL[n - 2].label(),
            };
            assert!(g.hits.iter().any(|h| h.label == want), "row {sel} ({want}) is shown at {w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
    }
    cx.services.update_sound(|s| s.muted = true).unwrap();
    assert!(!draw(&mut a, &cx, 1280, 720).hits.is_empty());
}

#[test]
fn a_cloud_machine_without_sound_does_not_list_the_page() {
    let cx = admin("sysnd-cloud");
    let c = iyakki_agent::cloudmode::cloud_mode(iyakki_agent::cloudmode::Setting::Auto, &iyakki_agent::cloudmode::Facts::bare());
    let a = SettingsApp::new(&cx).with_cloud(c);
    assert!(!a.tabs().contains(&Tab::SystemSounds) && !a.tabs().contains(&Tab::Sound));
}

/// Where each row's switch or card is drawn, by label.
fn spots(g: &Gfx) -> Vec<(String, i32)> {
    g.hits.iter().map(|h| (h.label.clone(), h.rect.y as i32)).collect()
}

#[test]
fn a_click_on_a_row_near_the_bottom_leaves_the_list_where_it_is() {
    let cx = admin("sysnd-click");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SystemSounds);
    // a window that shows only some rows; a key scrolls the choice into view
    let (w, h) = (800u32, 260u32);
    for _ in 0..4 {
        key(&mut a, &cx, Key::Down);
        draw(&mut a, &cx, w, h);
    }
    let before = draw(&mut a, &cx, w, h);
    let top = a.win[a.tab].top();
    assert!(top > 0, "the window is small enough to scroll");
    // click the last row that is drawn
    let last = before.hits.iter().filter_map(|x| match x.ev { Event::Row(i) => Some(i), Event::Sub(i, _) => Some(i as usize), _ => None }).max().unwrap();
    a.handle(&Event::Row(last), &cx);
    assert_eq!(a.sel(), last);
    let after = draw(&mut a, &cx, w, h);
    assert_eq!(a.win[a.tab].top(), top, "a click does not scroll the list");
    for (label, y) in spots(&before) {
        assert!(spots(&after).contains(&(label.clone(), y)), "{label} moved after the click");
    }
}

#[test]
fn the_wheel_scrolls_the_rows_and_not_the_choice_and_a_key_comes_back_to_the_choice() {
    let cx = admin("sysnd-wheel");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SystemSounds);
    let (w, h) = (800u32, 260u32);
    draw(&mut a, &cx, w, h);
    assert_eq!(a.win[a.tab].top(), 0);
    a.handle(&Event::Scroll(-1), &cx);
    assert!(a.win[a.tab].top() > 0, "the wheel scrolled the rows");
    assert_eq!(a.sel(), 0, "the choice stayed");
    draw(&mut a, &cx, w, h);
    key(&mut a, &cx, Key::Down);
    draw(&mut a, &cx, w, h);
    let t = a.win[a.tab].top();
    assert!(t <= a.sel(), "the key showed the choice (top {t}, choice {})", a.sel());
}
