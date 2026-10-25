//! Media and Fn keys: the key map, the step curves, the backlight and radio writers on a fake sysfs, the volume change through a
//! fake mixer, the key events of the input layer and the on-screen display at four screen sizes.

use super::*;
use crate::input::{InputState, Raw, EV_KEY};
use crate::ui::Event;
use std::collections::HashSet;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-fnk-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap().trim().to_string()
}

pub(crate) fn light(root: &Path, class: &str, name: &str, value: u32, max: u32) {
    put(root, &format!("class/{class}/{name}/brightness"), &format!("{value}\n"));
    put(root, &format!("class/{class}/{name}/max_brightness"), &format!("{max}\n"));
}

#[test]
fn every_key_code_maps_to_one_action_and_is_not_also_typed() {
    let mut seen = HashSet::new();
    for i in KEYS {
        assert!(seen.insert(i.code), "{} is listed twice", i.name);
        assert_eq!(fn_key(i.code), Some(i.key), "{}", i.name);
        assert!(!i.action.is_empty() && i.name.starts_with("KEY_"));
        assert!(crate::input::translate(i.code, false, false).is_none(), "{} must not also be typed as a character", i.name);
    }
    // the table of the specification, code by code (linux/input-event-codes.h)
    let want: [(u16, FnKey); 27] = [
        (115, FnKey::VolumeUp), (114, FnKey::VolumeDown), (113, FnKey::VolumeMute), (248, FnKey::MicMute),
        (225, FnKey::BrightnessUp), (224, FnKey::BrightnessDown), (244, FnKey::BrightnessAuto),
        (230, FnKey::KbdLightUp), (229, FnKey::KbdLightDown), (228, FnKey::KbdLightToggle),
        (164, FnKey::PlayPause), (200, FnKey::PlayPause), (201, FnKey::PlayPause), (207, FnKey::PlayPause),
        (163, FnKey::Next), (165, FnKey::Prev), (166, FnKey::Stop),
        (142, FnKey::Sleep), (205, FnKey::Sleep), (116, FnKey::Power),
        (247, FnKey::RfKill), (238, FnKey::Wlan), (237, FnKey::Bluetooth),
        (227, FnKey::SwitchVideo), (210, FnKey::Print), (140, FnKey::Calc), (172, FnKey::Homepage),
    ];
    assert_eq!(KEYS.len(), want.len());
    for (c, k) in want {
        assert_eq!(fn_key(c), Some(k), "code {c}");
    }
    assert_eq!(fn_key(30), None, "KEY_A is not an Fn key");
    assert!(describe_code(225).contains("KEY_BRIGHTNESSUP") && describe_code(999).contains("does not use"));
}

#[test]
fn the_input_layer_turns_the_keys_into_events_and_leaves_typing_alone() {
    let mut st = InputState::new(800, 600);
    let f = |st: &mut InputState, code: u16, value: i32| st.feed(Raw { typ: EV_KEY, code, value }, None);
    assert_eq!(f(&mut st, 115, 1), vec![Event::Fn(FnKey::VolumeUp, true)]);
    assert!(f(&mut st, 115, 2).is_empty(), "the kernel's repeat is ignored: the shell repeats by itself");
    assert_eq!(f(&mut st, 115, 0), vec![Event::Fn(FnKey::VolumeUp, false)]);
    assert_eq!(f(&mut st, 116, 1), vec![Event::Fn(FnKey::Power, true)]);
    assert_eq!(f(&mut st, 30, 1), vec![Event::Key(crate::ui::Key::Char('a'))], "typing is unchanged");
    f(&mut st, 255, 1); // an unknown code is remembered for the key-code list in Settings
    assert!(seen().contains(&255));
    assert!(describe_code(255).contains("255"));
}

#[test]
fn only_the_step_keys_repeat() {
    for k in [FnKey::VolumeUp, FnKey::VolumeDown, FnKey::BrightnessUp, FnKey::BrightnessDown, FnKey::KbdLightUp, FnKey::KbdLightDown] {
        assert!(k.repeats(), "{k:?}");
    }
    for k in [FnKey::VolumeMute, FnKey::Power, FnKey::PlayPause, FnKey::Wlan, FnKey::KbdLightToggle, FnKey::Print] {
        assert!(!k.repeats(), "{k:?} must act once per press");
    }
}

#[test]
fn the_brightness_curve_has_about_16_even_steps_and_never_reaches_zero() {
    for max in [1u32, 2, 7, 15, 100, 255, 937, 7500, 96000, 400000] {
        let l = brightness_levels(max);
        assert!(!l.is_empty() && l.len() <= STEPS, "max {max}: {l:?}");
        assert!(l.windows(2).all(|w| w[0] < w[1]), "max {max} strictly rising: {l:?}");
        assert!(l[0] >= 1, "max {max}: the lowest step is never 0");
        assert_eq!(*l.last().unwrap(), max, "max {max}: the top step is full");
    }
    let l = brightness_levels(96000);
    assert_eq!(l.len(), STEPS);
    assert!(l[0] <= 96000 / 40, "the lowest step is dim: {}", l[0]);
    // a square law: the steps are small near the bottom, large near the top
    assert!(l[1] - l[0] < l[15] - l[14] && l[2] - l[1] < l[15] - l[14]);
    assert_eq!(brightness_levels(0), Vec::<u32>::new());
    // stepping stops at both ends and finds a place from an odd value
    assert_eq!(step_level(&l, l[15], true), l[15]);
    assert_eq!(step_level(&l, l[0], false), l[0]);
    assert_eq!(step_level(&l, 0, true), l[0]);
    assert_eq!(step_level(&l, 1, false), l[0], "from below the lowest step it stays at the lowest, never 0");
    assert_eq!(step_level(&l, (l[4] + l[5]) / 2, true), l[5]);
    assert_eq!(step_level(&l, (l[4] + l[5]) / 2, false), l[4]);
    // 16 presses go from the bottom to the top
    let mut v = l[0];
    for _ in 0..15 {
        v = step_level(&l, v, true);
    }
    assert_eq!(v, l[15]);
}

#[test]
fn the_keyboard_light_steps_include_off() {
    assert_eq!(kbd_levels(3), vec![0, 1, 2, 3]);
    assert_eq!(kbd_levels(255), vec![0, 31, 63, 95, 127, 159, 191, 223, 255]);
    assert_eq!(kbd_levels(1), vec![0, 1]);
    assert!(kbd_levels(0).is_empty());
}

#[test]
fn the_panel_backlight_is_found_ranked_and_written_on_a_fake_sysfs() {
    let d = tmp("panel");
    assert_eq!(adjust_panel(&d, true), Err(NO_PANEL.to_string()), "no backlight: said so");
    light(&d, "backlight", "acpi_video0", 5, 10);
    light(&d, "backlight", "intel_backlight", 48000, 96000);
    light(&d, "backlight", "appletb_backlight", 1, 2);
    let names: Vec<String> = panel_backlights(&d).into_iter().map(|l| l.name).collect();
    assert_eq!(names, vec!["intel_backlight", "acpi_video0"], "the panel's own driver first; the Touch Bar's backlight is not the screen");
    assert_eq!(all_backlights(&d).len(), 3);
    let up = adjust_panel(&d, true).unwrap();
    assert_eq!((up.name.as_str(), up.at_end), ("intel_backlight", false));
    let v: u32 = read(&d, "class/backlight/intel_backlight/brightness").parse().unwrap();
    assert!(v > 48000 && v <= 96000, "{v}");
    assert_eq!(read(&d, "class/backlight/acpi_video0/brightness"), "5", "only one backlight is written");
    assert_eq!(read(&d, "class/backlight/appletb_backlight/brightness"), "1");
    // down to the very bottom: never 0
    for _ in 0..40 {
        adjust_panel(&d, false).unwrap();
    }
    let low: u32 = read(&d, "class/backlight/intel_backlight/brightness").parse().unwrap();
    assert!(low >= 1 && low <= 96000 / 40, "{low}");
    assert!(adjust_panel(&d, false).unwrap().at_end);
    for _ in 0..40 {
        adjust_panel(&d, true).unwrap();
    }
    assert_eq!(read(&d, "class/backlight/intel_backlight/brightness"), "96000");
    assert!(adjust_panel(&d, true).unwrap().at_end);
    // Mac panels
    let m = tmp("panel-mac");
    light(&m, "backlight", "gmux_backlight", 100, 1023);
    light(&m, "backlight", "apple-panel-bl", 100, 255);
    light(&m, "backlight", "amdgpu_bl0", 100, 255);
    assert_eq!(panel_backlights(&m)[0].name, "amdgpu_bl0");
    assert_eq!(panel_backlights(&m)[1].name, "apple-panel-bl");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&m);
}

#[test]
fn a_backlight_that_cannot_be_written_says_why() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root writes anywhere: nothing to show
    }
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("ro");
    light(&d, "backlight", "intel_backlight", 10, 100);
    let f = d.join("class/backlight/intel_backlight/brightness");
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
    let e = adjust_panel(&d, true).unwrap_err();
    assert!(e.contains("needs root"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_keyboard_light_steps_and_toggles_and_remembers_its_level() {
    let d = tmp("kbd");
    let mut last = 0;
    assert_eq!(adjust_kbd(&d, KbdOp::Up, &mut last), Err(NO_KBD.to_string()));
    light(&d, "leds", "smc::kbd_backlight", 64, 255);
    light(&d, "leds", "input3::capslock", 0, 1);
    assert_eq!(kbd_backlights(&d).len(), 1, "other LEDs are not keyboard lights");
    let c = adjust_kbd(&d, KbdOp::Up, &mut last).unwrap();
    assert_eq!(c.value, 95);
    assert_eq!(read(&d, "class/leds/smc::kbd_backlight/brightness"), "95");
    assert_eq!(adjust_kbd(&d, KbdOp::Toggle, &mut last).unwrap().value, 0);
    assert_eq!(read(&d, "class/leds/smc::kbd_backlight/brightness"), "0");
    assert_eq!(last, 95);
    assert_eq!(adjust_kbd(&d, KbdOp::Toggle, &mut last).unwrap().value, 95, "back to where it was");
    for _ in 0..12 {
        adjust_kbd(&d, KbdOp::Down, &mut last).unwrap();
    }
    assert_eq!(read(&d, "class/leds/smc::kbd_backlight/brightness"), "0", "the light can go off by stepping down");
    assert!(adjust_kbd(&d, KbdOp::Down, &mut last).unwrap().at_end);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_volume_steps_by_five_and_mute_keeps_the_level() {
    let mut s = SoundSettings { volume_pct: 50, ..Default::default() };
    volume_apply(&mut s, VolOp::Up);
    assert_eq!((s.volume_pct, s.muted), (55, false));
    volume_apply(&mut s, VolOp::Down);
    volume_apply(&mut s, VolOp::Down);
    assert_eq!(s.volume_pct, 45);
    volume_apply(&mut s, VolOp::Mute);
    assert_eq!((s.volume_pct, s.muted, s.gain()), (45, true, 0), "muted: the level is kept, the gain is 0");
    volume_apply(&mut s, VolOp::Mute);
    assert_eq!((s.volume_pct, s.muted), (45, false), "unmuted at the same level");
    volume_apply(&mut s, VolOp::Mute);
    volume_apply(&mut s, VolOp::Up);
    assert_eq!((s.volume_pct, s.muted), (50, false), "a step while muted unmutes");
    // off the grid: snap
    s.volume_pct = 37;
    volume_apply(&mut s, VolOp::Up);
    assert_eq!(s.volume_pct, 40);
    s.volume_pct = 37;
    volume_apply(&mut s, VolOp::Down);
    assert_eq!(s.volume_pct, 35);
    // the ends
    s.volume_pct = 98;
    volume_apply(&mut s, VolOp::Up);
    volume_apply(&mut s, VolOp::Up);
    assert_eq!(s.volume_pct, 100);
    s.volume_pct = 3;
    volume_apply(&mut s, VolOp::Down);
    volume_apply(&mut s, VolOp::Down);
    assert_eq!(s.volume_pct, 0);
    s.muted = true;
    volume_apply(&mut s, VolOp::Mute);
    assert_eq!((s.volume_pct, s.muted), (UNMUTE_LEVEL, false), "unmuting a volume of 0 does not stay silent");
    assert!(s.validate().is_ok());
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

#[test]
fn a_volume_key_reaches_the_software_volume_control_of_a_fake_mixer() {
    use iyakki_agent::softvol::{Plan, Softvol};
    let d = tmp("mixer");
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sv = Softvol::new(d.join("asound.conf"), Mixer(calls.clone()));
    let plan = Plan::Soft { card: 1, device: 0 };
    let mut s = SoundSettings { volume_pct: 60, ..Default::default() };
    let level = |c: &std::sync::Arc<std::sync::Mutex<Vec<String>>>| c.lock().unwrap().last().cloned().unwrap_or_default();
    assert_eq!(sv.sync(Some(&plan), s.gain()).soft_card, Some(1));
    assert!(level(&calls).ends_with("sset IYAKKI 60%"), "{}", level(&calls));
    volume_apply(&mut s, VolOp::Up);
    sv.sync(Some(&plan), s.gain());
    assert!(level(&calls).ends_with("sset IYAKKI 65%"), "{}", level(&calls));
    volume_apply(&mut s, VolOp::Mute);
    sv.sync(Some(&plan), s.gain());
    assert!(level(&calls).ends_with("sset IYAKKI 0%"), "muted sets the control to 0: {}", level(&calls));
    volume_apply(&mut s, VolOp::Mute);
    sv.sync(Some(&plan), s.gain());
    assert!(level(&calls).ends_with("sset IYAKKI 65%"), "unmuted at the remembered level: {}", level(&calls));
    let _ = std::fs::remove_dir_all(&d);
}

fn rfkill(root: &Path, id: &str, kind: &str, soft: u8, hard: u8) {
    put(root, &format!("class/rfkill/{id}/type"), &format!("{kind}\n"));
    put(root, &format!("class/rfkill/{id}/name"), &format!("{id}\n"));
    put(root, &format!("class/rfkill/{id}/soft"), &format!("{soft}\n"));
    put(root, &format!("class/rfkill/{id}/hard"), &format!("{hard}\n"));
}

#[test]
fn wifi_bluetooth_and_airplane_keys_switch_radios_with_a_notice() {
    let d = tmp("rf");
    assert_eq!(toggle_radio(&d, Radio::Wifi).unwrap(), "No WiFi adapter");
    rfkill(&d, "rfkill0", "wlan", 0, 0);
    rfkill(&d, "rfkill1", "bluetooth", 0, 0);
    assert_eq!(toggle_radio(&d, Radio::Wifi).unwrap(), "WiFi off");
    assert_eq!(read(&d, "class/rfkill/rfkill0/soft"), "1");
    assert_eq!(read(&d, "class/rfkill/rfkill1/soft"), "0", "Bluetooth is untouched");
    assert_eq!(toggle_radio(&d, Radio::Wifi).unwrap(), "WiFi on");
    assert_eq!(toggle_radio(&d, Radio::Bluetooth).unwrap(), "Bluetooth off");
    assert_eq!(toggle_radio(&d, Radio::All).unwrap(), "Airplane mode on", "one radio was still on: everything goes off");
    assert_eq!(read(&d, "class/rfkill/rfkill0/soft"), "1");
    assert_eq!(toggle_radio(&d, Radio::All).unwrap(), "Airplane mode off");
    assert_eq!(read(&d, "class/rfkill/rfkill1/soft"), "0");
    // a hardware switch cannot be undone from here
    rfkill(&d, "rfkill0", "wlan", 1, 1);
    assert_eq!(toggle_radio(&d, Radio::Wifi).unwrap(), "WiFi is off by a hardware switch");
    assert_eq!(read(&d, "class/rfkill/rfkill0/soft"), "1");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_display_fits_and_is_drawn_at_the_four_usual_sizes_without_taking_input() {
    let sizes: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];
    let shown = [
        Osd::volume(&SoundSettings { volume_pct: 65, ..Default::default() }, 10),
        Osd::volume(&SoundSettings { volume_pct: 65, muted: true, ..Default::default() }, 10),
        Osd { kind: OsdKind::Brightness, text: "100%".into(), fraction: Some(1.0), muted: false, until: 10 },
        Osd { kind: OsdKind::KbdLight, text: "Off".into(), fraction: Some(0.0), muted: false, until: 10 },
        Osd::notice("Display mode key: IYAKKI uses the screen as it is", 10),
        Osd::notice("No sound output", 10),
    ];
    for (w, h) in sizes {
        let s = crate::shell::scale_for(w, h);
        for o in &shown {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            draw_osd(&mut g, w as f32, h as f32, s, o);
            let tw = g.text_width(17.0 * s, &o.text, Face::Bold);
            let r = osd_rect(w as f32, h as f32, s, tw, o.fraction.is_some());
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y >= 0.0 && r.y + r.h <= h as f32 - 90.0 * s + 1.0, "{w}x{h} {o:?}: {r:?} inside the screen and above the toast line");
            assert!((r.x + r.w / 2.0 - w as f32 / 2.0).abs() < 1.0, "bottom centre");
            assert!(g.hits.is_empty(), "the display registers no click region: it never takes focus");
            // painted: something other than black inside the pill, nothing outside the screen's centre column far above it
            let px = |x: f32, y: f32| g.pm.pixel(x as u32, y as u32).map(|p| (p.red(), p.green(), p.blue())).unwrap();
            assert_ne!(px(r.x + r.w / 2.0, r.y + 2.0 * s), (0, 0, 0), "{w}x{h}");
            assert_eq!(px(w as f32 / 2.0, 10.0), (0, 0, 0), "{w}x{h}: nothing drawn at the top");
        }
    }
    // the bar's filled length follows the level
    let (w, h, s) = (1280u32, 720u32, 1.0f32);
    let fill = |pct: u32| {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        draw_osd(&mut g, w as f32, h as f32, s, &Osd::volume(&SoundSettings { volume_pct: pct, ..Default::default() }, 1));
        let r = osd_rect(w as f32, h as f32, s, 0.0, true);
        let y = r.y + r.h / 2.0;
        (0..r.w as u32).filter(|dx| g.pm.pixel(r.x as u32 + dx, y as u32).map(|p| p.blue() > 200 && p.red() < 160).unwrap_or(false)).count()
    };
    assert!(fill(90) > fill(30) + 40, "{} vs {}", fill(90), fill(30));
}
