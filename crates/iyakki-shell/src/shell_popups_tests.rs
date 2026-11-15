//! The top bar's popups (speaker dropdown, devices popover, battery popup, IYAKKI menu, installs list): a touch or click outside
//! closes each one, on an empty spot and on a tile alike; a press on a speaker button keeps its dropdown open and acts; Esc closes it.
//! (The clock tooltip is hover-only and the inbox is a screen, so neither has anything to close.)

use super::battery_tests::{laptop, signed};
use super::tests::*;
use super::*;

const SIZES: [(u32, u32); 3] = [(1280, 720), (1024, 600), (800, 480)];

#[derive(Clone, Copy, Debug)]
enum Pop {
    Vol,
    Dev,
    Battery,
    Brand,
    Installs,
}

fn is_open(sh: &Shell, p: Pop) -> bool {
    match p {
        Pop::Vol => sh.vol.open,
        Pop::Dev => sh.devpop.open,
        Pop::Battery => sh.battery.open,
        Pop::Brand => sh.brand.open,
        Pop::Installs => sh.installs.open,
    }
}

fn open(sh: &mut Shell, g: &mut Gfx, p: Pop) {
    sh.screen = Screen::Launcher;
    sh.render(g);
    match p {
        Pop::Vol => {
            sh.vol.open = true;
            sh.vol.kb = true;
        }
        Pop::Dev => sh.handle(&Event::Key(Key::F(8))),
        Pop::Battery => sh.handle(&Event::Key(Key::F(4))),
        Pop::Brand => sh.handle(&Event::Key(Key::F(10))),
        Pop::Installs => sh.toggle_installs(),
    }
    sh.render(g);
    assert!(is_open(sh, p), "{p:?} opens");
}

fn press(sh: &mut Shell, g: &mut Gfx, x: i32, y: i32) {
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.render(g);
}

/// The centre of the lowest tile (leftmost, or rightmost) of the closed home screen, away from the popup that will open.
fn tile_spot(sh: &mut Shell, g: &mut Gfx, w: u32, right: bool) -> (i32, i32) {
    sh.screen = Screen::Launcher;
    sh.render(g);
    let h = sh.hits().iter().filter(|h| h.rect.w > 40.0 && h.rect.h > 40.0 && h.rect.w < w as f32 / 2.0 && h.rect.y > 60.0 && h.label != "backdrop").max_by(|a, b| (a.rect.y, if right { a.rect.x } else { -a.rect.x }).partial_cmp(&(b.rect.y, if right { b.rect.x } else { -b.rect.x })).unwrap()).expect("a tile").rect;
    ((h.x + h.w / 2.0) as i32, (h.y + h.h / 2.0) as i32)
}

#[test]
fn every_top_bar_popup_closes_on_a_press_outside_and_on_esc() {
    let d = laptop("popups", 64, "Charging", true);
    let (mut sh, _) = signed("popups", Some(&d));
    sh.fnk.sound_probe = Some(true);
    for (w, h) in SIZES {
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        for p in [Pop::Vol, Pop::Dev, Pop::Battery, Pop::Brand, Pop::Installs] {
            // Esc
            open(&mut sh, &mut g, p);
            sh.handle(&Event::Key(Key::Esc));
            assert!(!is_open(&sh, p), "{w}x{h} {p:?}: Esc closes it");
            assert_eq!(sh.screen_name(), "launcher", "{w}x{h} {p:?}: and stays signed in");
            // an empty spot (the bottom-right corner)
            open(&mut sh, &mut g, p);
            press(&mut sh, &mut g, w as i32 - 2, h as i32 - 2);
            assert!(!is_open(&sh, p), "{w}x{h} {p:?}: a press on an empty spot closes it");
            // a tile
            let spot = tile_spot(&mut sh, &mut g, w, matches!(p, Pop::Brand));
            open(&mut sh, &mut g, p);
            press(&mut sh, &mut g, spot.0, spot.1);
            assert!(!is_open(&sh, p), "{w}x{h} {p:?}: a press on a tile at {spot:?} closes it; hits {:?}", sh.hits().iter().map(|h| (h.label.clone(), h.rect)).collect::<Vec<_>>());
            sh.screen = Screen::Launcher;
            sh.render(&mut g);
        }
    }
}

#[test]
fn a_press_on_a_speaker_button_keeps_the_dropdown_open_and_acts() {
    let (mut sh, _) = shell_with_users("popups-vol");
    login(&mut sh, "root", "rootpass12");
    sh.fnk.sound_probe = Some(true);
    let mut g = Gfx::new(1280, 720);
    sh.env.services.update_sound(|s| {
        s.volume_pct = 40;
        s.muted = false;
    })
    .unwrap();
    open(&mut sh, &mut g, Pop::Vol);
    let btn = sh.hits().iter().rev().find(|h| h.label == "volume louder").map(|h| h.rect).expect("a Louder button");
    press(&mut sh, &mut g, (btn.x + btn.w / 2.0) as i32, (btn.y + btn.h / 2.0) as i32);
    assert!(sh.vol.open, "the dropdown stays open");
    assert!(sh.env.services.master().percent > 40, "and Louder raised the volume");
}

#[test]
fn a_press_on_an_app_s_own_region_closes_the_speaker_dropdown() {
    let (mut sh, _) = shell_with_users("popups-app");
    login(&mut sh, "root", "rootpass12");
    sh.fnk.sound_probe = Some(true);
    let mut g = Gfx::new(1280, 720);
    for id in ["settings", "chat", "dash", "flows", "data", "system"] {
        sh.screen = Screen::Launcher;
        sh.stash = None;
        assert!(sh.open_for_demo(id), "{id} opens");
        sh.render(&mut g);
        sh.vol.open = true;
        sh.vol.kb = true;
        sh.render(&mut g);
        let icon = sh.speaker_rect().expect("the speaker");
        let panel = super::volbar_impl::pop_rects(icon, 1280, scale_for(1280, 720), sh.vol.sources.len()).0;
        // the biggest region of the app that lies clear of the bar and the dropdown
        let spot = sh.hits().iter().filter(|h| h.rect.y > 90.0 && h.rect.w > 20.0 && h.rect.h > 10.0 && h.label != "backdrop").map(|h| ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32)).find(|&(x, y)| !panel.contains(x, y));
        let Some((x, y)) = spot else { continue };
        let before = sh.screen_name();
        sh.handle(&Event::MouseDown(x, y));
        sh.handle(&Event::MouseUp(x, y));
        assert!(!sh.vol.open, "{id}: a press at ({x}, {y}) on the app closes the dropdown (screen {before} -> {})", sh.screen_name());
        sh.screen = Screen::Launcher;
    }
}
