//! The Home, Settings and speaker icons on every top bar (home screen, every app's header, the workspace bar of windowed and
//! multi-display layouts), their actions, their order and collapse at five screen sizes, and the DEV sign on every bar.

use super::tests::*;
use super::volbar_impl::Extra;
use super::*;
use crate::display::outputs::{DispSpec, DisplayConfig};
use crate::memgate::{MemFacts, GIB};
use crate::panes::LayoutKey;

const SIZES: [(u32, u32); 5] = [(800, 480), (1024, 600), (1280, 720), (1920, 1080), (2560, 1600)];
const APPS: [&str; 6] = ["settings", "chat", "dash", "flows", "data", "system"];

fn signed(tag: &str, who: (&str, &str)) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, who.0, who.1);
    sh.fnk.sound_probe = Some(true);
    sh
}

fn label(sh: &Shell, l: &str) -> Option<R> {
    sh.hits().iter().rev().find(|h| h.label == l).map(|h| h.rect)
}

fn centre(r: R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

fn click(sh: &mut Shell, g: &mut Gfx, r: R) {
    let (x, y) = centre(r);
    sh.render(g);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.render(g);
}

fn disjoint(a: R, b: R) -> bool {
    a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5
}

fn open_app(sh: &mut Shell, id: &str) {
    sh.screen = Screen::Launcher;
    sh.stash = None;
    assert!(sh.open_for_demo(id), "{id} opens");
}

#[test]
fn every_app_header_has_home_gear_and_speaker_in_order_without_overlap_at_five_sizes() {
    let mut sh = signed("tb-all", ("root", "rootpass12"));
    for id in APPS {
        for (w, h) in SIZES {
            let mut g = Gfx::new(w, h);
            let s = scale_for(w, h);
            open_app(&mut sh, id);
            sh.render(&mut g);
            let (gear, spk, home) = (label(&sh, "settings"), label(&sh, "volume"), label(&sh, "home"));
            let (gear, spk, home) = (gear.unwrap_or_else(|| panic!("{id} {w}x{h}: gear")), spk.unwrap_or_else(|| panic!("{id} {w}x{h}: speaker")), home.unwrap_or_else(|| panic!("{id} {w}x{h}: home")));
            assert!(gear.x < spk.x && spk.x < home.x, "{id} {w}x{h}: order gear, speaker, home");
            let clock = label(&sh, "clock");
            let mut all = vec![("gear", gear), ("speaker", spk), ("home", home), ("devices", dev_rect(w, s)), ("screenshot", shot_rect(w, s)), ("clipboard", clip_rect(w, s)), ("inbox", bell_rect(w, s)), ("help", help_rect(w, s)), ("back", back_rect(w, s))];
            if let Some(c) = clock {
                all.push(("clock", c));
            }
            if panes_impl::layout_slot() {
                all.push(("layout", panes_impl::layout_btn_rect(w, s)));
            }
            for (i, (na, a)) in all.iter().enumerate() {
                assert!(a.x >= 0.0 && a.x + a.w <= w as f32 && a.y >= 0.0 && a.y + a.h <= HEADER_H * s, "{id} {w}x{h} {na} {a:?} inside the header");
                for (nb, b) in &all[i + 1..] {
                    assert!(disjoint(*a, *b), "{id} {w}x{h}: {na} {a:?} overlaps {nb} {b:?}");
                }
            }
            for (na, r) in [("gear", gear), ("speaker", spk), ("home", home)] {
                assert!(r.w >= 30.0 * s && r.h >= 28.0 * s, "{id} {w}x{h}: {na} big enough to hit {r:?}");
            }
        }
    }
}

#[test]
fn narrow_headers_drop_the_gear_first_then_the_speaker_and_keep_home() {
    let mut sh = signed("tb-narrow", ("root", "rootpass12"));
    let mut seen = Vec::new();
    for w in [900u32, 700, 600, 520, 480, 420] {
        let h = 600;
        let mut g = Gfx::new(w, h);
        open_app(&mut sh, "chat");
        sh.render(&mut g);
        let s = scale_for(w, h);
        let (gear, spk, home) = (label(&sh, "settings"), label(&sh, "volume"), label(&sh, "home"));
        assert!(home.is_some(), "{w}: Home is kept at every width");
        assert!(gear.is_none() || spk.is_some(), "{w}: the gear never stays without the speaker");
        if let Some(c) = label(&sh, "clock") {
            for r in [gear, spk, home].into_iter().flatten() {
                assert!(disjoint(c, r), "{w}: clock {c:?} overlaps {r:?}");
            }
        }
        for r in [gear, spk, home].into_iter().flatten() {
            assert!(disjoint(r, bell_rect(w, s)) && disjoint(r, back_rect(w, s)), "{w}: {r:?}");
        }
        seen.push((w, gear.is_some(), spk.is_some()));
    }
    assert!(seen.first().unwrap().1 && seen.first().unwrap().2, "all three at 900: {seen:?}");
    assert!(seen.iter().any(|(_, g, sp)| !g && *sp) || seen.iter().all(|(_, g, sp)| g == sp), "{seen:?}");
    assert!(!seen.last().unwrap().1, "no gear at 420: {seen:?}");
}

#[test]
fn the_home_icon_and_f11_return_to_the_tiles_from_any_app_and_from_an_app_kept_aside() {
    let mut sh = signed("tb-home", ("root", "rootpass12"));
    let mut g = Gfx::new(1280, 720);
    for id in APPS {
        open_app(&mut sh, id);
        sh.render(&mut g);
        let r = label(&sh, "home").unwrap();
        click(&mut sh, &mut g, r);
        assert_eq!(sh.screen_name(), "launcher", "{id}: the icon leads home");
        open_app(&mut sh, id);
        sh.handle(&Event::Key(Key::F(11)));
        assert_eq!(sh.screen_name(), "launcher", "{id}: F11 too");
    }
    // the inbox opened from an app keeps that app aside; Home leaves both
    open_app(&mut sh, "chat");
    sh.handle(&Event::Key(Key::F(9)));
    assert!(sh.stash.is_some());
    sh.render(&mut g);
    let r = label(&sh, "home").unwrap();
    click(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "launcher");
    assert!(sh.stash.is_none());
    // on the tiles there is no Home icon (nothing to do), the gear and speaker are there
    sh.render(&mut g);
    assert!(label(&sh, "home").is_none() && sh.settings_rect().is_some() && sh.speaker_rect().is_some());
    assert!(sh.home_rect().is_none());
    // a popover that is open closes
    open_app(&mut sh, "chat");
    sh.handle(&Event::Key(Key::F(8)));
    assert!(sh.devpop.open);
    sh.handle(&Event::Key(Key::F(11)));
    assert!(!sh.devpop.open);
}

#[test]
fn the_grid_icon_is_the_layout_picker_and_home_is_a_separate_icon() {
    let mut sh = signed("tb-grid", ("root", "rootpass12"));
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    let mut g = Gfx::new(1280, 720);
    open_app(&mut sh, "chat");
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(labels.iter().filter(|l| *l == "home").count() == 1 && labels.iter().filter(|l| *l == "layout").count() <= 1, "{labels:?}");
    if let Some(l) = label(&sh, "layout") {
        click(&mut sh, &mut g, l);
        assert!(sh.panes_picker_open_for_test(), "the grid opens the layout picker");
        assert!(sh.screen_name().starts_with("app:"), "it does not go home");
    }
}

#[test]
fn the_gear_opens_settings_keeping_the_app_aside_and_stays_put_inside_settings() {
    let mut sh = signed("tb-gear", ("root", "rootpass12"));
    let mut g = Gfx::new(1280, 720);
    open_app(&mut sh, "chat");
    sh.render(&mut g);
    let r = label(&sh, "settings").unwrap();
    click(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "app:settings");
    assert!(sh.stash.as_ref().is_some_and(|(_, p)| *p == PApp::Chat), "the chat is kept aside, still running");
    // the gear in Settings changes nothing
    let r = label(&sh, "settings").unwrap();
    click(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "app:settings");
    assert!(sh.stash.is_some());
    // Back returns to the chat
    sh.leave_app();
    assert_eq!(sh.screen_name(), "app:chat");
    // from the tiles the gear still works
    sh.leave_app();
    sh.render(&mut g);
    let r = sh.settings_rect().unwrap();
    click(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), "app:settings");
}

#[test]
fn a_role_without_settings_gets_the_refusal_from_the_gear_in_an_app_and_keeps_its_app() {
    let mut sh = signed("tb-guest", ("gus", "guestpass1"));
    let mut g = Gfx::new(1280, 720);
    let allowed = APPS.iter().find(|id| *id != &"settings" && {
        sh.screen = Screen::Launcher;
        sh.open_for_demo(id) && sh.screen_name().starts_with("app:")
    });
    let Some(id) = allowed else { return };
    open_app(&mut sh, id);
    sh.render(&mut g);
    let before = sh.screen_name();
    let r = label(&sh, "settings").expect("the gear is shown, dimmed");
    click(&mut sh, &mut g, r);
    assert_eq!(sh.screen_name(), before, "still in the app");
    assert!(sh.stash.is_none());
}

#[test]
fn the_speaker_in_an_app_mutes_follows_the_wheel_and_drags_one_master_volume() {
    let mut sh = signed("tb-spk", ("root", "rootpass12"));
    sh.env.services.update_sound(|s| {
        s.volume_pct = 40;
        s.muted = false;
    })
    .unwrap();
    let mut g = Gfx::new(1280, 720);
    open_app(&mut sh, "chat");
    sh.render(&mut g);
    let r = label(&sh, "volume").unwrap();
    click(&mut sh, &mut g, r);
    assert!(sh.env.services.master().muted, "a click mutes");
    click(&mut sh, &mut g, r);
    assert!(!sh.env.services.master().muted && sh.env.services.master().percent == 40, "and unmutes keeping the level");
    let (x, y) = centre(r);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::Scroll(2));
    assert_eq!(sh.env.services.master().percent, 50, "the wheel: 5 % a notch");
    // press, move down the slider, let go
    sh.render(&mut g);
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseMove(x, y + 30));
    sh.render(&mut g);
    assert!(sh.volume_slider_open(), "dragging shows the slider");
    sh.handle(&Event::MouseUp(x, y + 30));
    assert!(sh.env.services.master().percent != 50);
    assert!(sh.screen_name() == "app:chat", "the app is untouched");
}

#[test]
fn cloud_mode_hides_what_does_not_apply_on_every_bar() {
    let mut sh = signed("tb-cloud", ("root", "rootpass12"));
    let mut g = Gfx::new(1280, 720);
    let mut c = iyakki_agent::cloudmode::CloudMode::desktop();
    c.fn_keys_off = true;
    sh.set_cloud_for_test(c);
    open_app(&mut sh, "chat");
    sh.render(&mut g);
    assert!(label(&sh, "volume").is_none() && label(&sh, "settings").is_some() && label(&sh, "home").is_some());
    assert_eq!(sh.wanted_extras(true), vec![Extra::Settings, Extra::Home]);
    let mut c = iyakki_agent::cloudmode::CloudMode::desktop();
    c.hidden_apps = vec![PApp::Settings];
    sh.set_cloud_for_test(c);
    sh.render(&mut g);
    assert!(label(&sh, "settings").is_none() && label(&sh, "volume").is_some() && label(&sh, "home").is_some());
}

fn dev_on(sh: &mut Shell, on: bool) {
    sh.dev_sign = on.then(|| "DEV".to_string());
}

fn has_dev(hits: &[Hit]) -> bool {
    hits.iter().any(|h| h.label == "developer mode on")
}

#[test]
fn the_dev_sign_shows_on_every_bar_when_developer_mode_is_on_and_not_when_it_is_off() {
    // single screen: the home screen and an app
    let mut sh = signed("tb-dev1", ("root", "rootpass12"));
    let mut g = Gfx::new(1280, 720);
    for on in [true, false] {
        dev_on(&mut sh, on);
        sh.screen = Screen::Launcher;
        sh.render(&mut g);
        assert_eq!(has_dev(sh.hits()), on, "home screen, dev {on}");
        open_app(&mut sh, "chat");
        sh.render(&mut g);
        assert_eq!(has_dev(sh.hits()), on, "app header, dev {on}");
    }
    // windowed layout on one display
    let mut sh = signed("tb-dev2", ("alice", "alicepass1"));
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    sh.handle(&Event::Layout(LayoutKey::Apply(2)));
    sh.render(&mut g);
    assert!(sh.panes_focus_for_test().is_some(), "the workspace is on");
    for on in [true, false] {
        dev_on(&mut sh, on);
        sh.render(&mut g);
        assert_eq!(has_dev(sh.hits()), on, "windowed, dev {on}");
        for l in ["settings", "volume"] {
            assert!(label(&sh, l).is_some(), "windowed bar has {l}");
        }
    }
    // two displays
    let (mut sh, _) = shell_with_users("tb-dev3");
    let (mut g0, mut g1) = (Gfx::new(1280, 720), Gfx::new(1024, 600));
    sh.render(&mut g0);
    login(&mut sh, "alice", "alicepass1");
    sh.fnk.sound_probe = Some(true);
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    let specs = vec![DispSpec { name: "eDP-1".into(), w: 1280, h: 720, x: 0, y: 0 }, DispSpec { name: "HDMI-A-1".into(), w: 1024, h: 600, x: 1280, y: 0 }];
    sh.set_displays(&specs, &DisplayConfig::default());
    for on in [true, false] {
        dev_on(&mut sh, on);
        sh.render_display(1, &mut g1);
        sh.render_display(0, &mut g0);
        for k in 0..2 {
            let hits = sh.display_hits_for_test(k);
            assert_eq!(has_dev(&hits), on, "display {k}, dev {on}");
            for l in ["settings", "volume", "clock", "inbox"] {
                assert!(hits.iter().any(|h| h.label == l), "display {k} bar has {l}");
            }
        }
    }
}

#[test]
fn the_workspace_bar_icons_do_not_overlap_the_buttons_and_home_goes_back_in_the_focused_pane() {
    let mut sh = signed("tb-ws", ("alice", "alicepass1"));
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        sh.screen = Screen::Launcher;
        sh.render(&mut g);
        sh.handle(&Event::Layout(LayoutKey::Apply(2)));
        sh.render(&mut g);
        let hits: Vec<Hit> = sh.hits().to_vec();
        let sc = scale_for(w, h);
        let bar: Vec<&Hit> = hits.iter().filter(|x| x.rect.y < 40.0 * sc && x.rect.h < 40.0 * sc).collect();
        for (i, a) in bar.iter().enumerate() {
            for b in &bar[i + 1..] {
                assert!(disjoint(a.rect, b.rect), "{w}x{h}: {} {:?} overlaps {} {:?}", a.label, a.rect, b.label, b.rect);
            }
        }
        sh.handle(&Event::Layout(LayoutKey::Apply(1)));
    }
}
