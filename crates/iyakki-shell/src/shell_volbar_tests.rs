//! The home screen's top bar: the Settings icon (no Settings tile on the grid) and the speaker icon on IYAKKI's one master volume:
//! its states, click, wheel, drag, hold and the keyboard, the missing sound output, and the bar at four screen sizes.

use super::tests::*;
use super::volbar_impl::{pct_at, pop_rects, Speaker};
use super::*;
use crate::fnkeys::OsdKind;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1024, 600), (800, 480), (480, 600)];

fn centre(r: R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

fn signed(tag: &str, who: (&str, &str)) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, who.0, who.1);
    sh.fnk.sound_probe = Some(true);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    (sh, g)
}

fn vol(sh: &Shell) -> (u32, bool) {
    let m = sh.env.services.master();
    (m.percent, m.muted)
}

fn set_vol(sh: &Shell, pct: u32, muted: bool) {
    sh.env.services.update_sound(|s| {
        s.volume_pct = pct;
        s.muted = muted;
    })
    .unwrap();
}

/// Press, optionally move, and let go, as a mouse does.
fn click_at(sh: &mut Shell, g: &mut Gfx, r: R) {
    let (x, y) = centre(r);
    sh.render(g);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.render(g);
}

fn icon(sh: &Shell) -> R {
    sh.speaker_rect().expect("the speaker is in the bar")
}

#[test]
fn settings_is_not_a_tile_but_an_icon_in_the_top_bar_that_opens_it() {
    let (mut sh, mut g) = signed("vb-settings", ("root", "rootpass12"));
    assert!(!sh.visible_tiles().iter().any(|t| t.app == PApp::Settings), "no Settings tile in the visible tiles");
    for gr in sh.groups() {
        sh.set_group(gr);
        assert!(!sh.group_items().iter().any(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Settings)), "nor in any group");
    }
    assert!(!layout_tiles(true, &iyakki_agent::cloudmode::CloudMode::desktop()).iter().any(|t| t.1 == "SETTINGS"), "nor in Settings -> Home layout");
    let r = sh.settings_rect().expect("a Settings icon");
    click_at(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "app:settings", "a click opens Settings");
    key(&mut sh, Key::Esc);
    assert_eq!(sh.screen_name(), "launcher");
    // the keyboard: the key s, F7 (the clock) and the IYAKKI menu (F10 then s) all work too
    key(&mut sh, Key::Char('s'));
    assert_eq!(sh.screen_name(), "app:settings");
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::F(10));
    key(&mut sh, Key::Char('s'));
    assert_eq!(sh.screen_name(), "app:settings");
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::F(7));
    assert_eq!(sh.screen_name(), "app:settings");
    // the launcher, the number keys and the search paths still reach the other apps
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Char('1'));
    assert_eq!(sh.screen_name(), "app:chat");
    assert!(sh.open_for_demo("settings"), "the chat agent and the IDE open it by name");
}

#[test]
fn the_settings_icon_has_the_permissions_of_the_tile() {
    let (mut sh, mut g) = signed("vb-guest", ("gus", "guestpass1"));
    assert!(!sh.session.as_ref().unwrap().policy.allows_app(PApp::Settings));
    let r = sh.settings_rect().expect("the icon is shown, dimmed");
    click_at(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "launcher", "a guest stays on the home screen");
    assert!(sh.toast_text().is_some_and(|t| t.contains("can't open SETTINGS")), "{:?}", sh.toast_text());
    key(&mut sh, Key::Char('s'));
    assert_eq!(sh.screen_name(), "launcher");
    let audit = std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default();
    assert!(audit.contains("app.denied"), "the refusal is audited like the tile's");
}

#[test]
fn the_speaker_shows_muted_low_medium_high_and_no_output() {
    let (mut sh, mut g) = signed("vb-states", ("root", "rootpass12"));
    for (pct, muted, want) in [(0, false, Speaker::Muted), (20, true, Speaker::Muted), (1, false, Speaker::Low), (33, false, Speaker::Low), (34, false, Speaker::Medium), (66, false, Speaker::Medium), (67, false, Speaker::High), (100, false, Speaker::High)] {
        set_vol(&sh, pct, muted);
        assert_eq!(sh.speaker_state(), want, "{pct}% muted={muted}");
        sh.render(&mut g);
    }
    sh.fnk.sound_probe = Some(false);
    assert_eq!(sh.speaker_state(), Speaker::NoOutput);
    // each state draws a different picture
    let mut pics = Vec::new();
    for st in [Speaker::NoOutput, Speaker::Muted, Speaker::Low, Speaker::Medium, Speaker::High] {
        let mut gi = Gfx::new(64, 64);
        gi.clear(rgb(0, 0, 0));
        gi.icon(st.icon(), 32.0, 32.0, 48.0, rgb(255, 255, 255));
        pics.push(gi.pm.data().to_vec());
    }
    for i in 0..pics.len() {
        for j in i + 1..pics.len() {
            assert_ne!(pics[i], pics[j], "states {i} and {j} look different");
        }
    }
    // the tip says the percent, or why there is no sound
    let s = SoundSettings { volume_pct: 40, ..Default::default() };
    assert_eq!(Speaker::Medium.tip(&s)[0].0, "Volume 40%");
    assert!(Speaker::Muted.tip(&s)[0].0.contains("Muted") && Speaker::Muted.tip(&s)[0].0.contains("40%"));
    assert_eq!(Speaker::NoOutput.tip(&s)[0].0, "No sound output");
}

use iyakki_agent::audioout::SoundSettings;

#[test]
fn a_click_mutes_and_unmutes_keeping_the_level_like_the_mute_key() {
    let (mut sh, mut g) = signed("vb-click", ("root", "rootpass12"));
    set_vol(&sh, 60, false);
    let r = icon(&sh);
    click_at(&mut sh, &mut g, r);
    assert_eq!(vol(&sh), (60, true), "muted, the level kept");
    assert_eq!(sh.speaker_state(), Speaker::Muted);
    let o = sh.fnk.osd.clone().expect("the same bar the keys show");
    assert_eq!((o.kind, o.muted), (OsdKind::Volume, true));
    click_at(&mut sh, &mut g, r);
    assert_eq!(vol(&sh), (60, false), "unmuted at the same level");
    // the same result as the mute key
    sh.handle(&Event::Tick);
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeMute, true));
    assert_eq!(vol(&sh), (60, true));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeMute, false));
    assert!(!sh.volume_slider_open(), "a plain click shows no slider");
    assert_eq!(sh.screen_name(), "launcher");
}

#[test]
fn the_wheel_over_the_speaker_changes_the_one_master_volume_in_5_percent_steps() {
    let (mut sh, mut g) = signed("vb-wheel", ("root", "rootpass12"));
    set_vol(&sh, 50, false);
    let (x, y) = centre(icon(&sh));
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::Scroll(1));
    assert_eq!(vol(&sh), (55, false), "up is louder");
    sh.handle(&Event::Scroll(-1));
    sh.handle(&Event::Scroll(-1));
    assert_eq!(vol(&sh), (45, false));
    sh.handle(&Event::Scroll(2));
    assert_eq!(vol(&sh), (55, false), "two notches, two steps");
    // a step also unmutes, like the keys
    set_vol(&sh, 55, true);
    sh.handle(&Event::Scroll(1));
    assert_eq!(vol(&sh), (60, false));
    let o = sh.fnk.osd.clone().expect("the bar shows");
    assert_eq!((o.kind, o.text.as_str()), (OsdKind::Volume, "60%"));
    // limits
    set_vol(&sh, 98, false);
    sh.handle(&Event::Scroll(3));
    assert_eq!(vol(&sh).0, 100);
    set_vol(&sh, 3, false);
    sh.handle(&Event::Scroll(-3));
    assert_eq!(vol(&sh).0, 0);
    // away from the icon the wheel still turns the pages and leaves the volume alone
    set_vol(&sh, 50, false);
    sh.handle(&Event::MouseMove(640, 400));
    sh.render(&mut g);
    sh.handle(&Event::Scroll(1));
    assert_eq!(vol(&sh), (50, false));
    // the keys move the same value from where the wheel left it
    sh.handle(&Event::Tick);
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VolumeUp, true));
    assert_eq!(vol(&sh).0, 55);
    assert_eq!(sh.env.services.sound_settings().volume_pct, 55, "there is one volume");
}

#[test]
fn dragging_from_the_speaker_sets_the_volume_along_the_slider() {
    let (mut sh, mut g) = signed("vb-drag", ("root", "rootpass12"));
    set_vol(&sh, 30, false);
    let r = icon(&sh);
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let (_, track) = pop_rects(r, w, s);
    let (x, y) = centre(r);
    sh.render(&mut g);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    assert_eq!(vol(&sh), (30, false), "pressing alone changes nothing");
    // down to the middle of the slider
    let mid = (track.y + track.h / 2.0) as i32;
    sh.handle(&Event::MouseMove(x, mid));
    assert!(sh.volume_slider_open(), "moving with the button down shows the slider");
    let want = pct_at(track, mid as f32);
    assert_eq!(vol(&sh), (want, false));
    assert!((45..=55).contains(&want), "the middle is about half: {want}");
    // and to the top: 100, to below the bottom: 0
    sh.handle(&Event::MouseMove(x, track.y as i32 - 30));
    assert_eq!(vol(&sh).0, 100);
    sh.handle(&Event::MouseMove(x, (track.y + track.h) as i32 + 30));
    assert_eq!(vol(&sh).0, 0);
    sh.handle(&Event::MouseMove(x, (track.y + track.h * 0.25) as i32));
    let q = vol(&sh).0;
    assert!((70..=80).contains(&q), "a quarter from the top is about 75: {q}");
    sh.handle(&Event::MouseUp(x, mid));
    assert!(!sh.volume_slider_open(), "the slider goes when the button is let go");
    assert_eq!(vol(&sh), (q, false), "and the volume stays; the drag was not a click");
    assert!(sh.fnk.osd.as_ref().is_some_and(|o| o.kind == OsdKind::Volume));
}

#[test]
fn holding_the_speaker_shows_the_slider_and_clicking_in_it_sets_the_volume() {
    let (mut sh, mut g) = signed("vb-hold", ("root", "rootpass12"));
    set_vol(&sh, 40, false);
    let r = icon(&sh);
    let (x, y) = centre(r);
    sh.render(&mut g);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    for _ in 0..12 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.volume_slider_open(), "held for a third of a second");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "volume slider"));
    sh.handle(&Event::MouseUp(x, y));
    assert_eq!(vol(&sh), (40, false), "holding is not a click: nothing was muted");
    // a quick click does not flash the slider
    click_at(&mut sh, &mut g, r);
    assert!(!sh.volume_slider_open());
    assert_eq!(vol(&sh), (40, true));
}

#[test]
fn the_keyboard_opens_the_slider_with_v_and_changes_mutes_and_closes_it() {
    let (mut sh, mut g) = signed("vb-kbd", ("root", "rootpass12"));
    set_vol(&sh, 50, false);
    key(&mut sh, Key::Char('v'));
    assert!(sh.volume_slider_open());
    sh.render(&mut g);
    key(&mut sh, Key::Up);
    assert_eq!(vol(&sh), (55, false));
    key(&mut sh, Key::Down);
    key(&mut sh, Key::Down);
    assert_eq!(vol(&sh), (45, false));
    key(&mut sh, Key::Char('+'));
    key(&mut sh, Key::Char('-'));
    key(&mut sh, Key::Char('-'));
    assert_eq!(vol(&sh).0, 40);
    key(&mut sh, Key::Char('m'));
    assert_eq!(vol(&sh), (40, true), "m mutes here, it does not move a tile");
    key(&mut sh, Key::Enter);
    assert_eq!(vol(&sh), (40, false), "Enter unmutes");
    key(&mut sh, Key::End);
    assert_eq!(vol(&sh).0, 100);
    key(&mut sh, Key::Home);
    assert_eq!(vol(&sh).0, 0);
    key(&mut sh, Key::Esc);
    assert!(!sh.volume_slider_open());
    assert_eq!(sh.screen_name(), "launcher", "Esc closed the slider, it did not sign out");
    assert!(sh.session.is_some());
    // v again opens and closes
    key(&mut sh, Key::Char('v'));
    key(&mut sh, Key::Char('v'));
    assert!(!sh.volume_slider_open());
    // a click elsewhere closes it and still acts: here, opening the Settings icon's app
    key(&mut sh, Key::Char('v'));
    sh.render(&mut g);
    let sr = sh.settings_rect().unwrap();
    click_at(&mut sh, &mut g, sr);
    assert!(!sh.volume_slider_open());
    assert_eq!(sh.screen_name(), "app:settings");
}

#[test]
fn with_no_sound_output_the_speaker_is_crossed_and_says_why_and_the_volume_stays() {
    let (mut sh, mut g) = signed("vb-none", ("root", "rootpass12"));
    set_vol(&sh, 50, false);
    sh.fnk.sound_probe = Some(false);
    sh.render(&mut g);
    assert_eq!(sh.speaker_state(), Speaker::NoOutput);
    let r = icon(&sh);
    click_at(&mut sh, &mut g, r);
    assert_eq!(vol(&sh), (50, false), "nothing is muted");
    let o = sh.fnk.osd.clone().expect("a notice");
    assert_eq!((o.kind, o.text.as_str()), (OsdKind::Notice, "No sound output"));
    sh.fnk.osd = None;
    let (x, y) = centre(r);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::Scroll(1));
    assert_eq!(vol(&sh), (50, false), "the wheel too");
    assert_eq!(sh.fnk.osd.as_ref().map(|o| o.text.as_str()), Some("No sound output"));
    key(&mut sh, Key::Char('v'));
    assert!(!sh.volume_slider_open(), "no slider for nothing to change");
    // hovering draws the reason
    sh.handle(&Event::MouseMove(x, y));
    sh.render(&mut g);
}

#[test]
fn the_volume_in_the_devices_popover_and_settings_follow_the_speaker() {
    let (mut sh, mut g) = signed("vb-one", ("root", "rootpass12"));
    set_vol(&sh, 50, false);
    let (x, y) = centre(icon(&sh));
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::Scroll(1));
    // one value: the services' master, what Settings -> Sound reads and what the Devices popover shows
    assert_eq!(sh.env.services.master().percent, 55);
    assert_eq!(sh.env.services.sound_settings().volume_pct, 55);
    let r = icon(&sh);
    click_at(&mut sh, &mut g, r);
    assert!(sh.env.services.master().muted && sh.env.services.sound_settings().muted);
    // the Devices popover reads the same master volume (devpop::sound_volume_view takes it from the settings)
    assert!(sh.devpop.wants_refresh_idle(), "and is told to read again");
}

#[test]
fn the_bar_icons_fit_without_overlap_at_four_screen_sizes_and_for_every_role() {
    for who in [("root", "rootpass12"), ("gus", "guestpass1")] {
        let (mut sh, _) = shell_with_users(&format!("vb-fit-{}", who.0));
        login(&mut sh, who.0, who.1);
        sh.fnk.sound_probe = Some(true);
        for (w, h) in SIZES {
            sh.resize_for_test(w, h);
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            let s = scale_for(w, h);
            let slots = sh.bar_slots();
            let rects: Vec<R> = (0..slots.count).map(|i| header_button(w, s, i)).collect();
            assert_eq!(slots.speaker.map(|i| rects[i]), sh.speaker_rect());
            assert_eq!(slots.settings.map(|i| rects[i]), sh.settings_rect());
            for (i, a) in rects.iter().enumerate() {
                assert!(a.x >= 0.0 && a.x + a.w <= w as f32 && a.y >= 0.0 && a.y + a.h <= 90.0 * s, "{who:?} {w}x{h}: icon {i} {a:?} inside the screen");
                assert!(a.w >= 24.0 && a.h >= 24.0, "{w}x{h}: tappable {a:?}");
                assert!(rects[i + 1..].iter().all(|b| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5), "{who:?} {w}x{h}: icons {i} overlap");
            }
            // the name on the left stays clear of the icons, and so does the clock when it is shown
            let name_w = g.text_width(30.0 * s, "IYAKKI", Face::Bold);
            let leftmost = rects.iter().map(|r| r.x).fold(f32::MAX, f32::min);
            assert!(40.0 * s + name_w + 12.0 * s <= leftmost, "{who:?} {w}x{h}: the name {name_w} reaches the icons at {leftmost}");
            if let Some(c) = sh.hits().iter().find(|x| x.label == "clock").map(|x| x.rect) {
                assert!(c.x + c.w <= leftmost, "{who:?} {w}x{h}: the clock {c:?} is left of the icons at {leftmost}");
            }
            // the slider panel is inside the screen, below the icon, and its track is inside the panel
            if let Some(sp) = sh.speaker_rect() {
                let (panel, track) = pop_rects(sp, w, s);
                assert!(panel.x >= 0.0 && panel.x + panel.w <= w as f32 && panel.y > sp.y + sp.h && panel.y + panel.h <= h as f32, "{who:?} {w}x{h}: panel {panel:?}");
                assert!(track.x >= panel.x && track.x + track.w <= panel.x + panel.w && track.y >= panel.y && track.y + track.h <= panel.y + panel.h);
                assert!(track.h > 40.0 * s * 0.6, "a track long enough to drag along");
                assert!(sp.contains(centre(sp).0, centre(sp).1));
                assert_eq!(pct_at(track, track.y + track.h), 0);
                assert_eq!(pct_at(track, track.y), 100);
                // open it and draw: the controls still fit
                sh.vol.open = true;
                sh.vol.kb = true;
                sh.render(&mut g);
                assert!(sh.hits().iter().any(|x| x.label == "volume slider" && x.rect == panel));
                sh.vol.open = false;
                sh.vol.kb = false;
            }
            // every tile of every group still fits the grid below the bar
            for gr in sh.groups() {
                sh.set_group(gr);
                sh.render(&mut g);
            }
        }
    }
}

#[test]
fn cloud_mode_hides_the_speaker_but_keeps_settings() {
    let (mut sh, mut g) = signed("vb-cloud", ("root", "rootpass12"));
    assert!(sh.bar_slots().speaker.is_some());
    let mut c = iyakki_agent::cloudmode::CloudMode::desktop();
    c.fn_keys_off = true;
    sh.set_cloud_for_test(c);
    sh.render(&mut g);
    let slots = sh.bar_slots();
    assert!(slots.speaker.is_none() && slots.settings.is_some(), "{slots:?}");
    key(&mut sh, Key::Char('v'));
    assert!(!sh.volume_slider_open());
}
