//! The shell and the media keys: volume with the display, brightness, the keys never reaching the focused app (the web page
//! included), held-key repeat, radio keys that wait for the kernel, the power card that never acts alone, and the saved keyboard
//! settings reaching a fake `hid-apple`.

use super::tests::*;
use super::*;
use crate::fnkeys::{FnKey, OsdKind};
use std::path::Path;

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn fake_sys(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-fnsh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn signed_in(tag: &str) -> (Shell, std::path::PathBuf) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let sys = fake_sys(tag);
    sh.fnk.sys = sys.clone();
    sh.fnk.sound_probe = Some(true);
    (sh, sys)
}

fn press(sh: &mut Shell, k: FnKey) {
    sh.handle(&Event::Tick); // a press in a heartbeat of its own
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(k, true));
    sh.handle(&Event::Fn(k, false));
}

fn osd(sh: &Shell) -> Option<crate::fnkeys::Osd> {
    sh.fnk.osd.clone()
}

fn tick(sh: &mut Shell, n: u32) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

#[test]
fn volume_keys_change_the_settings_show_the_display_and_mute_remembers_the_level() {
    let (mut sh, _) = signed_in("fn-vol");
    let svc = sh.env.services.clone();
    svc.update_sound(|s| s.volume_pct = 50).unwrap();
    press(&mut sh, FnKey::VolumeUp);
    assert_eq!(svc.sound_settings().volume_pct, 55);
    let o = osd(&sh).expect("the display shows");
    assert_eq!((o.kind, o.text.as_str(), o.muted), (OsdKind::Volume, "55%", false));
    assert!(sh.take_stale(), "a redraw is asked for");
    press(&mut sh, FnKey::VolumeDown);
    press(&mut sh, FnKey::VolumeDown);
    assert_eq!(svc.sound_settings().volume_pct, 45);
    press(&mut sh, FnKey::VolumeMute);
    let s = svc.sound_settings();
    assert!(s.muted && s.volume_pct == 45, "muted, level kept: {s:?}");
    assert_eq!(osd(&sh).unwrap().text, "Muted");
    press(&mut sh, FnKey::VolumeMute);
    assert_eq!((svc.sound_settings().muted, svc.sound_settings().volume_pct), (false, 45));
    // the top bar's Devices popover and Settings -> Sound read the same settings: a change made there shows on the next key
    svc.update_sound(|s| s.volume_pct = 80).unwrap();
    press(&mut sh, FnKey::VolumeUp);
    assert_eq!(svc.sound_settings().volume_pct, 85);
    // the saved file has it (a new Services over the same directory would read it back)
    assert_eq!(svc.settings.lock().unwrap().get().sound.volume_pct, 85);
    // the display goes away by itself after about 1.5 s
    tick(&mut sh, 60);
    assert!(osd(&sh).is_none());
}

#[test]
fn a_held_volume_key_repeats_until_released_and_a_press_in_the_same_beat_counts_once() {
    let (mut sh, _) = signed_in("fn-hold");
    let svc = sh.env.services.clone();
    svc.update_sound(|s| s.volume_pct = 10).unwrap();
    sh.handle(&Event::Tick);
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(FnKey::VolumeUp, true));
    sh.handle(&Event::Fn(FnKey::VolumeUp, true)); // a second device reporting the same press
    assert_eq!(svc.sound_settings().volume_pct, 15, "once");
    tick(&mut sh, 70);
    let held = svc.sound_settings().volume_pct;
    assert!(held >= 35 && held <= 100, "it went on while held: {held}");
    sh.handle(&Event::Fn(FnKey::VolumeUp, false));
    tick(&mut sh, 40);
    assert_eq!(svc.sound_settings().volume_pct, held, "released: it stops");
    // keys that are not steps do not repeat
    sh.handle(&Event::Fn(FnKey::VolumeMute, true));
    tick(&mut sh, 70);
    assert!(svc.sound_settings().muted, "mute acted once");
}

#[test]
fn without_a_sound_output_the_key_says_so_and_changes_nothing() {
    let (mut sh, _) = signed_in("fn-nosound");
    sh.fnk.sound_probe = Some(false);
    let before = sh.env.services.sound_settings();
    press(&mut sh, FnKey::VolumeUp);
    assert_eq!(osd(&sh).unwrap().text, "No sound output");
    assert_eq!(sh.env.services.sound_settings(), before);
}

#[test]
fn brightness_keys_write_the_backlight_show_the_display_and_say_when_there_is_none() {
    let (mut sh, sys) = signed_in("fn-bri");
    press(&mut sh, FnKey::BrightnessUp);
    assert_eq!(osd(&sh).unwrap().text, "No brightness control on this display");
    put(&sys, "class/backlight/intel_backlight/brightness", "30000\n");
    put(&sys, "class/backlight/intel_backlight/max_brightness", "96000\n");
    press(&mut sh, FnKey::BrightnessUp);
    let o = osd(&sh).unwrap();
    assert_eq!(o.kind, OsdKind::Brightness);
    let v: u32 = std::fs::read_to_string(sys.join("class/backlight/intel_backlight/brightness")).unwrap().trim().parse().unwrap();
    assert!(v > 30000, "{v}");
    press(&mut sh, FnKey::BrightnessDown);
    let w: u32 = std::fs::read_to_string(sys.join("class/backlight/intel_backlight/brightness")).unwrap().trim().parse().unwrap();
    assert!(w < v, "{w} < {v}");
    // the keyboard light
    press(&mut sh, FnKey::KbdLightUp);
    assert_eq!(osd(&sh).unwrap().text, "No keyboard backlight on this computer");
    put(&sys, "class/leds/smc::kbd_backlight/brightness", "100\n");
    put(&sys, "class/leds/smc::kbd_backlight/max_brightness", "255\n");
    press(&mut sh, FnKey::KbdLightToggle);
    assert_eq!(osd(&sh).unwrap().text, "Off");
    press(&mut sh, FnKey::KbdLightToggle);
    assert_eq!(std::fs::read_to_string(sys.join("class/leds/smc::kbd_backlight/brightness")).unwrap().trim(), "100");
    // the other notices
    press(&mut sh, FnKey::SwitchVideo);
    assert!(osd(&sh).unwrap().text.contains("Display mode"));
    press(&mut sh, FnKey::MicMute);
    assert!(osd(&sh).unwrap().text.contains("not available yet"));
}

struct Rec(std::sync::Arc<std::sync::Mutex<Vec<Event>>>);
impl App for Rec {
    fn title(&self) -> &'static str {
        "Rec"
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        rgb(0, 255, 0)
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            self.0.lock().unwrap().push(ev.clone());
        }
        Flow::Stay
    }
    fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, _tick: u64) {}
}

#[test]
fn a_media_key_never_reaches_the_focused_app_or_the_web_page() {
    for (tag, pa) in [("fn-leak-chat", PApp::Chat), ("fn-leak-web", PApp::Browser)] {
        let (mut sh, _) = signed_in(tag);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        sh.screen = Screen::App(Box::new(Rec(seen.clone())), pa);
        for k in crate::fnkeys::KEYS.iter().map(|i| i.key).filter(|k| !matches!(k, FnKey::Power | FnKey::Sleep | FnKey::Print | FnKey::Homepage | FnKey::Lock)) {
            press(&mut sh, k);
        }
        sh.handle(&Event::Key(Key::Char('v'))); // an ordinary key still arrives
        let got = seen.lock().unwrap().clone();
        assert_eq!(got, vec![Event::Key(Key::Char('v'))], "{tag}: only the typed key arrived: {got:?}");
        assert!(matches!(sh.screen, Screen::App(..)), "the app keeps the screen (no focus change)");
    }
}

#[test]
fn the_display_is_drawn_without_stealing_focus_and_the_screen_stays_the_same() {
    let (mut sh, _) = signed_in("fn-osd");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        let before = (sh.screen_name(), sh.hits().len());
        press(&mut sh, FnKey::VolumeUp);
        sh.render(&mut g);
        assert_eq!((sh.screen_name(), sh.hits().len()), before, "{w}x{h}: same screen, same click regions");
        let s = scale_for(w, h);
        let r = crate::fnkeys::osd_rect(w as f32, h as f32, s, 40.0 * s, true);
        let p = g.pm.pixel((r.x + 40.0 * s) as u32, (r.y + r.h / 2.0) as u32).unwrap();
        assert!(p.blue() > 20, "{w}x{h}: the display is on the picture");
    }
}

#[test]
fn the_fn_power_card_sleeps_on_s_only_when_possible() {
    let (mut sh, host) = shell_with_users("fn-sleep");
    login(&mut sh, "root", "rootpass12");
    press(&mut sh, FnKey::Power);
    sh.handle(&Event::Key(Key::Char('s')));
    assert!(!sh.fnk.power_card && *host.slept.lock().unwrap() == 0 && sh.session.is_some(), "not possible: nothing happens");
    *host.sleepable.lock().unwrap() = true;
    press(&mut sh, FnKey::Power);
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(*host.slept.lock().unwrap(), 0);
    press(&mut sh, FnKey::Power);
    sh.handle(&Event::Key(Key::Char('s')));
    assert_eq!(*host.slept.lock().unwrap(), 1);
    assert!(sh.session.is_some() && sh.is_locked(), "locked (the apps stay open) before sleeping");
}

#[test]
fn the_power_card_opens_on_the_key_and_acts_only_on_a_choice() {
    let (mut sh, host) = {
        let (mut sh, host) = shell_with_users("fn-power");
        login(&mut sh, "root", "rootpass12");
        (sh, host)
    };
    let mut g = Gfx::new(1280, 720);
    press(&mut sh, FnKey::Power);
    assert!(sh.fnk.power_card);
    assert!(host.powered_off.lock().unwrap().is_none(), "nothing happens by itself");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Key(Key::Char('r'))), "Restart is a button");
    // a stray key closes it without acting
    sh.handle(&Event::Key(Key::Char('x')));
    assert!(!sh.fnk.power_card && host.powered_off.lock().unwrap().is_none());
    // the key again closes it, Esc closes it
    press(&mut sh, FnKey::Power);
    assert!(sh.fnk.power_card);
    press(&mut sh, FnKey::Sleep);
    assert!(!sh.fnk.power_card);
    press(&mut sh, FnKey::Power);
    sh.handle(&Event::Key(Key::Esc));
    assert!(!sh.fnk.power_card && host.powered_off.lock().unwrap().is_none());
    // a choice acts, and is in the audit log
    press(&mut sh, FnKey::Power);
    sh.handle(&Event::Key(Key::Char('r')));
    assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()));
    // an account without the power permission, at the machine's own screen, may power off (the login screen has no password either:
    // someone at the machine can pull the plug)
    let (mut sh2, host2) = shell_with_users("fn-power2");
    login(&mut sh2, "root", "rootpass12");
    sh2.session.as_mut().unwrap().policy.power = false;
    press(&mut sh2, FnKey::Power);
    assert!(sh2.fnk.power_card, "a plain user at the machine gets the power card");
    sh2.handle(&Event::Key(Key::Esc));
    assert!(!sh2.fnk.power_card && host2.powered_off.lock().unwrap().is_none());
    // a remote (browser or cloud) session without the permission is told so
    sh2.set_remote(true);
    press(&mut sh2, FnKey::Power);
    assert!(!sh2.fnk.power_card);
    assert!(osd(&sh2).unwrap().text.contains("may not"));
    assert!(host2.powered_off.lock().unwrap().is_none());
    // before sign-in: the login screen's own power card (`shell_loginbar_tests.rs`), not the signed-in one
    let (mut sh3, _) = shell_with_users("fn-power3");
    press(&mut sh3, FnKey::Power);
    assert!(!sh3.fnk.power_card && sh3.lbar.card.is_some());
}

#[test]
fn media_keys_answer_for_the_radio_and_say_when_nothing_plays() {
    let (mut sh, _) = signed_in("fn-media");
    press(&mut sh, FnKey::PlayPause);
    assert_eq!(osd(&sh).unwrap().text, "Nothing is playing");
    press(&mut sh, FnKey::Stop);
    assert_eq!(osd(&sh).unwrap().text, "Nothing is playing");
    press(&mut sh, FnKey::Next);
    assert_eq!(osd(&sh).unwrap().text, "Nothing is playing");
}

#[test]
fn the_wifi_key_waits_for_the_kernel_and_never_switches_twice() {
    let (mut sh, sys) = signed_in("fn-rf");
    let rf = |soft: &str| {
        put(&sys, "class/rfkill/rfkill0/type", "wlan\n");
        put(&sys, "class/rfkill/rfkill0/name", "phy0\n");
        put(&sys, "class/rfkill/rfkill0/soft", soft);
        put(&sys, "class/rfkill/rfkill0/hard", "0\n");
    };
    let soft = || std::fs::read_to_string(sys.join("class/rfkill/rfkill0/soft")).unwrap().trim().to_string();
    rf("0\n");
    // nothing else switches: the shell does, after a moment
    press(&mut sh, FnKey::Wlan);
    assert_eq!(soft(), "0", "not at once");
    tick(&mut sh, 6);
    assert_eq!(soft(), "1");
    assert_eq!(osd(&sh).unwrap().text, "WiFi off");
    // the kernel (rfkill-input) switched it by itself between the key and the check: only the notice
    press(&mut sh, FnKey::Wlan);
    put(&sys, "class/rfkill/rfkill0/soft", "0\n");
    tick(&mut sh, 6);
    assert_eq!(soft(), "0", "left as the kernel set it");
    assert_eq!(osd(&sh).unwrap().text, "WiFi on");
}

#[test]
fn the_saved_keyboard_settings_reach_a_fake_hid_apple_and_survive_a_restart() {
    let (mut sh, sys) = signed_in("fn-kbd");
    put(&sys, "module/hid_apple/parameters/fnmode", "3\n");
    sh.env.services.settings.lock().unwrap().update(|s| s.keyboard.fn_mode = "fkeys".into()).unwrap();
    tick(&mut sh, 5); // the first sync happens a few heartbeats after start
    assert_eq!(std::fs::read_to_string(sys.join("module/hid_apple/parameters/fnmode")).unwrap(), "2");
    // the driver loads later: caught within ten seconds
    let (mut sh2, sys2) = signed_in("fn-kbd2");
    sh2.env.services.settings.lock().unwrap().update(|s| s.keyboard.fn_mode = "media".into()).unwrap();
    tick(&mut sh2, 5);
    put(&sys2, "module/hid_apple/parameters/fnmode", "3\n");
    tick(&mut sh2, 310);
    assert_eq!(std::fs::read_to_string(sys2.join("module/hid_apple/parameters/fnmode")).unwrap(), "1");
    // an invalid saved value is refused by the store
    assert!(sh2.env.services.settings.lock().unwrap().update(|s| s.keyboard.fn_mode = "bogus".into()).is_err());
}

#[test]
fn every_volume_entry_point_changes_the_one_master_and_the_views_read_it_back() {
    use iyakki_agent::softvol::{Plan, Softvol};
    let (mut sh, _) = signed_in("fn-master");
    let svc = sh.env.services.clone();
    // the control the master ends up on: one fake softvol, fed what the selector feeds it (the master's gain)
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let d = fake_sys("fn-master-conf");
    let sv = Softvol::new(d.join("asound.conf"), Mixer(calls.clone()));
    let plan = Plan::Soft { card: 0, device: 0 };
    let sync = || sv.sync(Some(&plan), svc.sound_settings().gain());
    let last = || calls.lock().unwrap().last().cloned().unwrap_or_default();
    svc.master_set(50).unwrap();
    // 1. keyboard
    press(&mut sh, FnKey::VolumeUp);
    assert_eq!(svc.master().percent, 55);
    assert_eq!(svc.radio.volume(), 55, "the radio's slider is a view of the master");
    sync();
    assert!(last().ends_with("sset IYAKKI 55%"), "{}", last());
    // 2. a Settings slider / the radio's bars set the master, never a second level
    svc.master_set(70).unwrap();
    assert_eq!((svc.radio.volume(), svc.master().percent), (70, 70));
    svc.radio.set_volume(30);
    assert_eq!((svc.master().percent, svc.sound_settings().volume_pct), (30, 30), "the radio slider moved the master");
    sync();
    assert!(last().ends_with("sset IYAKKI 30%"), "{}", last());
    // 3. mute is one flag: popover, key and the app buttons toggle the same one, the level is kept
    assert!(svc.master_toggle_mute().unwrap().muted);
    sync();
    assert!(last().ends_with("sset IYAKKI 0%"), "{}", last());
    assert_eq!(svc.radio.volume(), 30, "the radio still shows the level it will come back to");
    assert_eq!(svc.sound_settings().gain(), 0, "WAV players are silent");
    // a volume-up while muted unmutes, from the key, the radio slider or a nudge
    press(&mut sh, FnKey::VolumeUp);
    assert_eq!((svc.master().muted, svc.master().percent), (false, 35));
    svc.master_toggle_mute().unwrap();
    svc.radio.set_volume(40);
    assert_eq!((svc.master().muted, svc.master().percent), (false, 40));
    svc.master_toggle_mute().unwrap();
    press(&mut sh, FnKey::VolumeMute);
    assert!(!svc.master().muted, "the key toggles the same flag");
    svc.master_toggle_mute().unwrap();
    svc.master_nudge(5).unwrap();
    assert!(!svc.master().muted);
    sync();
    assert!(last().ends_with("sset IYAKKI 45%"), "{}", last());
    // 4. one master change counter for every view to watch; the saved file has the value
    let g = svc.master().generation;
    svc.master_nudge(-5).unwrap();
    assert!(svc.master().generation > g);
    assert_eq!(svc.settings.lock().unwrap().get().sound.volume_pct, 40);
    let _ = std::fs::remove_dir_all(&d);
}

struct Mixer(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
impl iyakki_agent::softvol::Runner for Mixer {
    fn amixer(&self, args: &[String]) -> Result<String, String> {
        self.0.lock().unwrap().push(args.join(" "));
        Ok(String::new())
    }
    fn prime(&self, _pcm: &str) -> Result<(), String> {
        Ok(())
    }
}
