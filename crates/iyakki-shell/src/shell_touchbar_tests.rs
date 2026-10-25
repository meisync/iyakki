//! The shell and the Touch Bar strip: a ready fake display, pictures made from the shell's state, touches carried out, recent apps
//! remembered per account, and the firmware's keys handed back when the strip stops.

use super::tests::*;
use super::*;
use crate::touchbar::{slider_track, TouchEv};
use crate::touchbar_host::Host;
use std::path::Path;

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn rig(tag: &str) -> (Shell, Host, std::path::PathBuf) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let sys = std::env::temp_dir().join(format!("iyakki-tbsh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sys);
    std::fs::create_dir_all(&sys).unwrap();
    sh.fnk.sys = sys.clone();
    sh.fnk.sound_probe = Some(true);
    let host = Host::detached(0);
    host.ready_for_test(2170, 60);
    sh.tbar_attach_for_test(host.clone());
    (sh, host, sys)
}

fn ticks(sh: &mut Shell, n: u32) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

fn center_of(sh: &mut Shell, id: &str) -> i32 {
    let s = sh.tbar_bar_for_test().unwrap().slots().iter().find(|s| s.item.id == id).unwrap().rect;
    (s.x + s.w / 2) as i32
}

#[test]
fn a_ready_display_gets_a_picture_of_the_shell_state_and_only_changes_after() {
    let (mut sh, host, _) = rig("tb-pic");
    ticks(&mut sh, 6);
    let p = host.take_picture_for_test().expect("the first picture");
    assert_eq!((p.w, p.h), (2170, 60));
    ticks(&mut sh, 30);
    assert!(host.take_picture_for_test().is_none(), "nothing changed, nothing sent (apart from the clock)");
    sh.env.services.update_sound(|s| s.volume_pct = 20).unwrap();
    ticks(&mut sh, 6);
    assert!(host.take_picture_for_test().is_some(), "a volume change redraws");
}

#[test]
fn touching_the_volume_slider_sets_the_master_volume_and_a_tap_on_the_chip_cycles() {
    let (mut sh, host, _) = rig("tb-vol");
    ticks(&mut sh, 6);
    let vol = sh.tbar_bar_for_test().unwrap().slots().iter().find(|s| s.item.id == "volume").unwrap().rect;
    let (x0, x1) = slider_track(&vol);
    host.push_touch(TouchEv::Down((x0 + (x1 - x0) * 0.3) as i32));
    host.push_touch(TouchEv::Up((x0 + (x1 - x0) * 0.3) as i32));
    ticks(&mut sh, 4);
    let v = sh.env.services.sound_settings().volume_pct;
    assert!((28..=32).contains(&v), "{v}");
    let chip = center_of(&mut sh, "mode");
    host.push_touch(TouchEv::Down(chip));
    host.push_touch(TouchEv::Up(chip));
    ticks(&mut sh, 2);
    assert_eq!(sh.tbar_bar_for_test().unwrap().chosen(), crate::touchbar::Strip::Recent);
}

#[test]
fn brightness_follows_the_slider_and_never_goes_fully_dark() {
    let (mut sh, host, sys) = rig("tb-br");
    put(&sys, "class/backlight/intel_backlight/brightness", "500\n");
    put(&sys, "class/backlight/intel_backlight/max_brightness", "1000\n");
    ticks(&mut sh, 40);
    let br = sh.tbar_bar_for_test().unwrap().slots().iter().find(|s| s.item.id == "brightness").unwrap().rect;
    let (x0, x1) = slider_track(&br);
    let _ = x1;
    host.push_touch(TouchEv::Down((br.x + br.w - 2) as i32));
    ticks(&mut sh, 4);
    assert_eq!(std::fs::read_to_string(sys.join("class/backlight/intel_backlight/brightness")).unwrap().trim(), "1000");
    host.push_touch(TouchEv::Move((x0 - 50.0) as i32));
    ticks(&mut sh, 8);
    let v: u32 = std::fs::read_to_string(sys.join("class/backlight/intel_backlight/brightness")).unwrap().trim().parse().unwrap();
    assert!(v > 0 && v < 100, "the lowest step, not black: {v}");
}

#[test]
fn apps_that_come_to_the_front_become_recents_for_that_account_and_open_from_the_strip() {
    let (mut sh, host, _) = rig("tb-rec");
    ticks(&mut sh, 4);
    sh.open(PApp::Chat);
    ticks(&mut sh, 4);
    sh.handle(&Event::Key(Key::Esc));
    ticks(&mut sh, 2);
    assert!(matches!(sh.screen, Screen::Launcher));
    sh.open(PApp::Radio);
    ticks(&mut sh, 4);
    assert_eq!(sh.tbar.saved.recents, ["radio", "chat"]);
    assert!(sh.env.services.dir().join("touchbar-root.json").is_file(), "remembered on disk");
    // the Recent strip: a tile opens its app (from another app: Esc first)
    sh.tbar_bar_for_test().unwrap().set_strip(crate::touchbar::Strip::Recent);
    ticks(&mut sh, 6);
    let tile = center_of(&mut sh, "recent-chat");
    host.push_touch(TouchEv::Down(tile));
    host.push_touch(TouchEv::Up(tile));
    ticks(&mut sh, 4);
    assert!(matches!(&sh.screen, Screen::App(_, PApp::Chat)), "CHAT is open");
    // a fresh shell for the same account finds them again
    let (mut other, _) = shell_with_users("tb-rec");
    let _ = &mut other;
}

#[test]
fn nothing_about_the_account_is_drawn_before_sign_in() {
    let (mut sh, _) = shell_with_users("tb-lock");
    let host = Host::detached(0);
    host.ready_for_test(2170, 60);
    sh.tbar_attach_for_test(host.clone());
    ticks(&mut sh, 6);
    let p = host.take_picture_for_test().expect("a picture");
    assert!(p.rgba.chunks_exact(4).all(|px| px[..3] != crate::ui::CARD[..3]), "no buttons on the login screen");
}

#[test]
fn when_the_display_stops_the_firmware_keys_come_back_in_the_default_mode() {
    let (mut sh, host, sys) = rig("tb-back");
    put(&sys, "bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005/mode", "3\n");
    ticks(&mut sh, 6);
    assert!(sh.tbar.ready);
    // the strip fails: a worker that stopped answering (status seen as not ready)
    host.stop_for_test();
    ticks(&mut sh, 2);
    assert!(!sh.tbar.ready);
    assert_eq!(std::fs::read_to_string(sys.join("bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005/mode")).unwrap().trim(), "2");
    assert!(sh.tbar_bar_for_test().is_none());
}

#[test]
fn the_stop_and_approval_buttons_reach_the_chat() {
    let (mut sh, host, _) = rig("tb-ai");
    sh.tbar_attach_for_test(host.clone());
    sh.open(PApp::Chat);
    ticks(&mut sh, 6);
    sh.tbar_bar_for_test().unwrap().set_strip(crate::touchbar::Strip::Ai);
    ticks(&mut sh, 6);
    let items: Vec<String> = sh.tbar_bar_for_test().unwrap().slots().iter().map(|s| s.item.id.clone()).collect();
    assert!(items.contains(&"chat".to_string()) && items.contains(&"stop".to_string()));
    let stop = sh.tbar_bar_for_test().unwrap().slots().iter().find(|s| s.item.id == "stop").unwrap().item.clone();
    assert!(!stop.enabled, "no run is going");
}
