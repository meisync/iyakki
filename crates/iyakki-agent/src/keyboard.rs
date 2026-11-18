//! Settings -> Keyboard and function keys: what is kept in the settings file and how it reaches the kernel.
//!
//! Apple keyboards on Linux (`hid-apple`) have a module parameter `fnmode` that decides what the top row sends: media and
//! brightness keys by default (Fn+F-key gives F1 to F12), or the F-keys by default. It, `iso_layout` and `swap_opt_cmd` are
//! writable at run time under `/sys/module/hid_apple/parameters/`. The Touch Bar of a MacBook Pro 2019 (`hid-appletb-kbd`)
//! has a `mode` file per device with the same choice. The built-in keyboard of a 2015-2017 MacBook (no USB: it sits on an SPI bus)
//! is driven by `applespi`, whose parameters `fnmode` (0 off, 1 media keys first, 2 F-keys first; it has no automatic mode) and
//! `iso_layout` are writable the same way under `/sys/module/applespi/parameters/`; its trackpad is the second input device of
//! the same driver (tap to click and scrolling are IYAKKI's own, `iyakki-apps/src/touchpad.rs`). Every path hangs off a `sys` root, so tests use a fake directory.
//! Nothing here runs a program.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
pub use iyakki_shared::keyboard::*;


/// The `hid-appletb-kbd` mode the shell asks for while IYAKKI draws the strip (3 = the firmware draws nothing), or None to leave the
/// chosen one. Written by the shell's Touch Bar thread, read by [`apply_apple`] so the ten-second settings sync does not bring the
/// firmware's keys back over the strip.
static TOUCHBAR_OVERRIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

pub fn set_touchbar_override(mode: Option<u32>) {
    TOUCHBAR_OVERRIDE.store(mode.unwrap_or(u32::MAX), std::sync::atomic::Ordering::Relaxed);
}

pub fn touchbar_override() -> Option<u32> {
    Some(TOUCHBAR_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed)).filter(|v| *v != u32::MAX)
}

pub fn apple_param(sys: &Path, name: &str) -> PathBuf {
    sys.join("module/hid_apple/parameters").join(name)
}

/// A parameter of `hid-appletb-kbd`, the Touch Bar's keyboard driver (it also dims and switches off the bar's backlight).
pub fn touchbar_param(sys: &Path, name: &str) -> PathBuf {
    sys.join("module/hid_appletb_kbd/parameters").join(name)
}

/// The kernel's defaults dim the Touch Bar after 60 s without input and switch it off 15 s later, so the keys vanish (a touch still
/// works) while a video plays or a page is read. IYAKKI asks for 15 minutes to dim and 30 more to switch off.
pub const TOUCHBAR_DIM_SECS: u32 = 900;
pub const TOUCHBAR_OFF_SECS: u32 = 1800;

/// A parameter of `applespi`, the SPI keyboard driver of the 2015-2017 MacBooks.
pub fn applespi_param(sys: &Path, name: &str) -> PathBuf {
    sys.join("module/applespi/parameters").join(name)
}

/// The `mode` files of the Touch Bar keyboard devices (none without the driver or the hardware).
pub fn touchbar_mode_files(sys: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(sys.join("bus/hid/drivers/hid-appletb-kbd")) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("mode")).filter(|p| p.is_file()).collect();
    v.sort();
    v
}

/// The strip stopped (failed, turned off): put the firmware's keys back at once, in the chosen mode or the kernel's default
/// (2, media keys). Returns the files written.
pub fn restore_touchbar_mode(sys: &Path, k: &KeyboardSettings) -> usize {
    let v = k.touchbar_value().filter(|v| *v != 3 || k.touchbar == "off").unwrap_or(2).to_string();
    let mut n = 0;
    for p in touchbar_mode_files(sys) {
        if read_trim(&p).as_deref() != Some(v.as_str()) && std::fs::write(&p, &v).is_ok() {
            n += 1;
        }
    }
    n
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_bool(p: &Path) -> Option<bool> {
    // kernel bool parameters read back as Y or N (uint ones as numbers)
    read_trim(p).and_then(|s| match s.as_str() {
        "Y" | "y" | "1" => Some(true),
        "N" | "n" | "0" => Some(false),
        _ => None,
    })
}

pub fn apple_state(sys: &Path) -> AppleState {
    let fnp = apple_param(sys, "fnmode");
    AppleState {
        driver: fnp.is_file(),
        fnmode: read_trim(&fnp).and_then(|s| s.parse().ok()),
        iso_layout: read_bool(&apple_param(sys, "iso_layout")),
        swap_opt_cmd: read_bool(&apple_param(sys, "swap_opt_cmd")),
        spi: applespi_param(sys, "fnmode").is_file(),
        spi_fnmode: read_trim(&applespi_param(sys, "fnmode")).and_then(|s| s.parse().ok()),
        touchbar: touchbar_mode_files(sys).into_iter().filter_map(|p| Some((p.parent()?.file_name()?.to_string_lossy().into_owned(), read_trim(&p)?.parse().ok()?))).collect(),
    }
}

fn put(p: &Path, v: &str, what: &str, done: &mut Vec<String>) -> Result<(), String> {
    if read_trim(p).as_deref() == Some(v) {
        return Ok(());
    }
    std::fs::write(p, v).map_err(|e| format!("could not set {what}: {e}"))?;
    done.push(format!("{what} = {v}"));
    Ok(())
}

/// Write the chosen values to the kernel where its files exist; returns what was changed (empty when everything already
/// matched or there is no Apple keyboard driver). Cheap enough to call every few seconds so a driver loaded later is caught.
pub fn apply_apple(sys: &Path, k: &KeyboardSettings) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let fnp = apple_param(sys, "fnmode");
    if let (true, Some(v)) = (fnp.is_file(), k.fnmode_value()) {
        put(&fnp, &v.to_string(), "hid_apple fnmode", &mut done)?;
    }
    for (name, want) in [("iso_layout", k.iso_layout), ("swap_opt_cmd", k.swap_opt_cmd)] {
        let p = apple_param(sys, name);
        if let (true, Some(b)) = (p.is_file(), want) {
            // bool parameters read back as Y/N; compare as booleans
            if read_bool(&p) != Some(b) {
                std::fs::write(&p, if b { "1" } else { "0" }).map_err(|e| format!("could not set {name}: {e}"))?;
                done.push(format!("hid_apple {name} = {}", b as u8));
            }
        }
    }
    // applespi: the same two settings as hid_apple (a MacBook has this one, an external Apple keyboard the other)
    let sp = applespi_param(sys, "fnmode");
    if let (true, Some(v)) = (sp.is_file(), k.applespi_fnmode_value()) {
        put(&sp, &v.to_string(), "applespi fnmode", &mut done)?;
    }
    let sp = applespi_param(sys, "iso_layout");
    if let (true, Some(b)) = (sp.is_file(), k.iso_layout) {
        if read_bool(&sp) != Some(b) {
            std::fs::write(&sp, if b { "1" } else { "0" }).map_err(|e| format!("could not set applespi iso_layout: {e}"))?;
            done.push(format!("applespi iso_layout = {}", b as u8));
        }
    }
    if let Some(v) = touchbar_override().or(k.touchbar_value()) {
        for p in touchbar_mode_files(sys) {
            put(&p, &v.to_string(), "Touch Bar mode", &mut done)?;
        }
    } else if k.touchbar != "off" {
        // no mode chosen and the strip is not drawing: a bar left dark (mode 3: a strip that stopped, or a driver that came back after a
        // configuration change) gets the kernel's default keys again. Any other mode, Fn held included, is left alone.
        for p in touchbar_mode_files(sys) {
            if read_trim(&p).as_deref() == Some("3") {
                put(&p, "2", "Touch Bar mode", &mut done)?;
            }
        }
    }
    // a failing write here (older driver, read-only parameter) is not worth stopping the other settings for.
    // The driver armed its first dim (60 s) when it loaded, before these timers could be changed, so a bar that is still dark after
    // the automatic dimming is switched off is lit again: with `autodim` off nothing in the kernel darkens it any more.
    let autodim = touchbar_param(sys, "autodim");
    if autodim.is_file() {
        let _ = put(&autodim, "N", "Touch Bar autodim", &mut done);
        let bl = sys.join("class/backlight/appletb_backlight");
        if let (Some(0), Some(max)) = (read_trim(&bl.join("brightness")).and_then(|s| s.parse::<u32>().ok()), read_trim(&bl.join("max_brightness")).and_then(|s| s.parse::<u32>().ok())) {
            if max > 0 && std::fs::write(bl.join("brightness"), max.to_string()).is_ok() {
                done.push(format!("Touch Bar backlight = {max}"));
            }
        }
    }
    for (name, secs) in [("dim_timeout", TOUCHBAR_DIM_SECS), ("idle_timeout", TOUCHBAR_OFF_SECS)] {
        let p = touchbar_param(sys, name);
        if p.is_file() {
            let _ = put(&p, &secs.to_string(), &format!("Touch Bar {name}"), &mut done);
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-kbd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_apple(d: &Path) {
        let p = d.join("module/hid_apple/parameters");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("fnmode"), "3\n").unwrap();
        std::fs::write(p.join("iso_layout"), "N\n").unwrap();
        std::fs::write(p.join("swap_opt_cmd"), "N\n").unwrap();
        let tb = d.join("bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005");
        std::fs::create_dir_all(&tb).unwrap();
        std::fs::write(tb.join("mode"), "2\n").unwrap();
    }

    #[test]
    fn the_touchbar_backlight_timers_are_lengthened_where_the_driver_has_them_and_left_alone_elsewhere() {
        let d = tmp("tbtimers");
        assert!(apply_apple(&d, &KeyboardSettings::default()).unwrap().is_empty(), "no driver, nothing written");
        let p = touchbar_param(&d, "dim_timeout");
        let q = touchbar_param(&d, "idle_timeout");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "60\n").unwrap();
        std::fs::write(&q, "15\n").unwrap();
        let done = apply_apple(&d, &KeyboardSettings::default()).unwrap();
        assert_eq!(done, vec!["Touch Bar dim_timeout = 900", "Touch Bar idle_timeout = 1800"]);
        // the driver's automatic dimming is switched off, and a bar it already darkened is lit again
        let a = touchbar_param(&d, "autodim");
        std::fs::write(&a, "Y\n").unwrap();
        let bl = d.join("class/backlight/appletb_backlight");
        std::fs::create_dir_all(&bl).unwrap();
        std::fs::write(bl.join("brightness"), "0\n").unwrap();
        std::fs::write(bl.join("max_brightness"), "2\n").unwrap();
        let done = apply_apple(&d, &KeyboardSettings::default()).unwrap();
        assert_eq!(done, vec!["Touch Bar autodim = N", "Touch Bar backlight = 2"]);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "N");
        assert_eq!(std::fs::read_to_string(bl.join("brightness")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "900");
        assert!(apply_apple(&d, &KeyboardSettings::default()).unwrap().is_empty(), "written again only when it differs");
    }

    #[test]
    fn pointer_speeds_have_defaults_limits_and_old_files_load() {
        assert_eq!(KeyboardSettings::default().pointer_speeds(), (100, 100));
        let k = KeyboardSettings { pointer_speed_pct: 150, touchpad_speed_pct: 25, ..Default::default() };
        assert!(k.validate().is_ok());
        assert_eq!(k.pointer_speeds(), (150, 25));
        for (v, ok) in [(0, true), (24, false), (25, true), (300, true), (301, false)] {
            assert_eq!(KeyboardSettings { pointer_speed_pct: v, ..Default::default() }.validate().is_ok(), ok, "mouse {v}");
            assert_eq!(KeyboardSettings { touchpad_speed_pct: v, ..Default::default() }.validate().is_ok(), ok, "touchpad {v}");
        }
        assert_eq!(KeyboardSettings { pointer_speed_pct: 9000, touchpad_speed_pct: 3, ..Default::default() }.pointer_speeds(), (300, 25));
        let old: KeyboardSettings = serde_json::from_str("{\"fn_mode\":\"media\"}").unwrap();
        assert_eq!(old.pointer_speeds(), (100, 100), "a file written before the setting existed");
    }

    #[test]
    fn the_choices_are_validated_and_map_to_kernel_values() {
        let mut k = KeyboardSettings::default();
        assert!(k.validate().is_ok());
        assert_eq!((k.fnmode_value(), k.touchbar_value(), k.repeat()), (None, None, None), "nothing chosen touches nothing");
        for (m, fnv, tb) in [("media", 1, Some(2)), ("fkeys", 2, Some(1)), ("auto", 3, None)] {
            k.fn_mode = m.into();
            assert!(k.validate().is_ok());
            assert_eq!((k.fnmode_value(), k.touchbar_value()), (Some(fnv), tb), "{m}");
        }
        k.fn_mode = "bogus".into();
        assert!(k.validate().is_err());
        k.fn_mode.clear();
        k.repeat_delay_ms = 50;
        assert!(k.validate().is_err());
        k.repeat_delay_ms = 400;
        k.repeat_rate = 100;
        assert!(k.validate().is_err());
        k.repeat_rate = 25;
        assert!(k.validate().is_ok());
        assert_eq!(k.repeat(), Some((400, 40)));
        k.repeat_delay_ms = 0;
        assert_eq!(k.repeat(), Some((250, 40)), "the half left alone keeps the kernel default");
    }

    #[test]
    fn the_fnmode_is_written_to_a_fake_hid_apple_and_only_when_it_differs() {
        let d = tmp("fn");
        fake_apple(&d);
        let st = apple_state(&d);
        assert!(st.driver);
        assert_eq!((st.fnmode, st.iso_layout, st.swap_opt_cmd), (Some(3), Some(false), Some(false)));
        assert_eq!(st.touchbar, vec![("0003:05AC:8302.0005".to_string(), 2)]);
        let k = KeyboardSettings { fn_mode: "fkeys".into(), iso_layout: Some(true), swap_opt_cmd: Some(false), ..Default::default() };
        let done = apply_apple(&d, &k).unwrap();
        assert_eq!(done, vec!["hid_apple fnmode = 2", "hid_apple iso_layout = 1", "Touch Bar mode = 1"]);
        assert_eq!(std::fs::read_to_string(apple_param(&d, "fnmode")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(apple_param(&d, "iso_layout")).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(apple_param(&d, "swap_opt_cmd")).unwrap(), "N\n", "unchanged value not rewritten");
        assert!(apply_apple(&d, &k).unwrap().is_empty(), "the second call has nothing to do");
        // media keys first
        let k = KeyboardSettings { fn_mode: "media".into(), ..Default::default() };
        apply_apple(&d, &k).unwrap();
        assert_eq!(std::fs::read_to_string(apple_param(&d, "fnmode")).unwrap(), "1");
        // nothing chosen: nothing written
        assert!(apply_apple(&d, &KeyboardSettings::default()).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fake_applespi(d: &Path) {
        let p = d.join("module/applespi/parameters");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("fnmode"), "1\n").unwrap();
        std::fs::write(p.join("iso_layout"), "N\n").unwrap();
    }

    #[test]
    fn the_fnmode_and_iso_layout_reach_a_fake_applespi_like_hid_apple() {
        let d = tmp("spi");
        fake_applespi(&d);
        let st = apple_state(&d);
        assert!(st.spi && !st.driver, "applespi alone: the MacBook's own keyboard");
        assert_eq!(st.spi_fnmode, Some(1));
        let k = KeyboardSettings { fn_mode: "fkeys".into(), iso_layout: Some(true), ..Default::default() };
        assert_eq!(apply_apple(&d, &k).unwrap(), vec!["applespi fnmode = 2", "applespi iso_layout = 1"]);
        assert_eq!(std::fs::read_to_string(applespi_param(&d, "fnmode")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(applespi_param(&d, "iso_layout")).unwrap(), "1");
        assert!(apply_apple(&d, &k).unwrap().is_empty(), "nothing to do the second time");
        let media = KeyboardSettings { fn_mode: "media".into(), ..Default::default() };
        assert_eq!(apply_apple(&d, &media).unwrap(), vec!["applespi fnmode = 1"]);
        // applespi has no automatic mode: "auto" leaves it alone
        let auto = KeyboardSettings { fn_mode: "auto".into(), ..Default::default() };
        assert!(apply_apple(&d, &auto).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(applespi_param(&d, "fnmode")).unwrap(), "1");
        // both drivers present (a MacBook with an external Apple keyboard): both are written
        fake_apple(&d);
        let done = apply_apple(&d, &KeyboardSettings { fn_mode: "fkeys".into(), ..Default::default() }).unwrap();
        assert!(done.contains(&"hid_apple fnmode = 2".to_string()) && done.contains(&"applespi fnmode = 2".to_string()), "{done:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_trackpad_choices_default_to_tap_on_and_natural_scrolling() {
        let mut k = KeyboardSettings::default();
        assert_eq!(k.trackpad(), (true, true));
        k.trackpad_tap = Some(false);
        k.trackpad_natural = Some(false);
        assert_eq!(k.trackpad(), (false, false));
        assert_eq!(serde_json::from_str::<KeyboardSettings>("{\"trackpad_tap\":false}").unwrap().trackpad(), (false, true));
    }

    #[test]
    fn the_scroll_choices_have_defaults_limits_and_survive_a_round_trip() {
        let d = KeyboardSettings::default().scroll();
        assert_eq!(d, ScrollPrefs { speed_pct: 100, wheel_natural: false, pad_natural: true, smooth: true, momentum: true, wheel_lines: 3 });
        let k = KeyboardSettings { scroll_speed_pct: 250, wheel_natural: Some(true), scroll_smooth: Some(false), scroll_momentum: Some(false), wheel_lines: 5, trackpad_natural: Some(false), ..Default::default() };
        assert_eq!(k.scroll(), ScrollPrefs { speed_pct: 250, wheel_natural: true, pad_natural: false, smooth: false, momentum: false, wheel_lines: 5 });
        let j = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<KeyboardSettings>(&j).unwrap(), k);
        assert!(k.validate().is_ok());
        for (speed, ok) in [(49, false), (50, true), (300, true), (301, false), (0, true)] {
            assert_eq!(KeyboardSettings { scroll_speed_pct: speed, ..Default::default() }.validate().is_ok(), ok, "speed {speed}");
        }
        for (lines, ok) in [(0, true), (1, true), (10, true), (11, false)] {
            assert_eq!(KeyboardSettings { wheel_lines: lines, ..Default::default() }.validate().is_ok(), ok, "lines {lines}");
        }
        // a hand-edited file outside the limits is pulled back inside them
        assert_eq!(KeyboardSettings { scroll_speed_pct: 9000, ..Default::default() }.scroll().speed_pct, 300);
        assert_eq!(KeyboardSettings { scroll_speed_pct: 5, ..Default::default() }.scroll().speed_pct, 50);
        assert_eq!(KeyboardSettings { wheel_lines: 99, ..Default::default() }.scroll().wheel_lines, 10);
    }

    #[test]
    fn a_machine_without_the_apple_driver_is_left_alone() {
        let d = tmp("none");
        let k = KeyboardSettings { fn_mode: "media".into(), iso_layout: Some(true), ..Default::default() };
        assert_eq!(apply_apple(&d, &k), Ok(vec![]));
        assert_eq!(apple_state(&d), AppleState::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_settings_survive_a_round_trip_and_old_files_without_them_load() {
        let k = KeyboardSettings { fn_mode: "fkeys".into(), iso_layout: Some(true), swap_opt_cmd: None, trackpad_tap: Some(false), trackpad_natural: Some(true), repeat_delay_ms: 300, repeat_rate: 20, ..Default::default() };
        let j = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<KeyboardSettings>(&j).unwrap(), k);
        assert_eq!(serde_json::from_str::<KeyboardSettings>("{}").unwrap(), KeyboardSettings::default());
    }

    #[test]
    fn the_touch_bar_choice_picks_the_mode_the_strips_and_the_firmware_key_mode() {
        let d = KeyboardSettings::default();
        assert_eq!(d.touchbar_mode(true), TouchBarMode::Strip, "the default is the strip where the display works");
        assert_eq!(d.touchbar_mode(false), TouchBarMode::Keys, "and the keys where it does not");
        let k = |t: &str| KeyboardSettings { touchbar: t.into(), ..Default::default() };
        assert_eq!((k("off").touchbar_mode(true), k("keys").touchbar_mode(true), k("strip").touchbar_mode(false)), (TouchBarMode::Off, TouchBarMode::Keys, TouchBarMode::Keys));
        assert_eq!(d.strips(), ["system", "recent", "ai", "app"]);
        let two = KeyboardSettings { touchbar_strips: vec!["ai".into(), "system".into()], ..Default::default() };
        assert_eq!(two.strips(), ["system", "ai"], "always in the cycle order");
        assert_eq!(k("off").touchbar_value(), Some(3), "off: the firmware draws nothing either");
        assert_eq!(KeyboardSettings { fn_mode: "fkeys".into(), ..Default::default() }.touchbar_value(), Some(1));
        assert!(k("sideways").validate().is_err());
        assert!(KeyboardSettings { touchbar_strips: vec!["x".into()], ..Default::default() }.validate().is_err());
        assert!(KeyboardSettings { touchbar_flip: 4, ..Default::default() }.validate().is_err());
        assert!(KeyboardSettings { touchbar: "strip".into(), touchbar_strips: vec!["recent".into()], touchbar_flip: 3, ..Default::default() }.validate().is_ok());
    }

    #[test]
    fn while_the_strip_is_drawn_the_sync_keeps_the_firmware_keys_off() {
        let d = tmp("tbo");
        let m = d.join("bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("mode"), "2\n").unwrap();
        let k = KeyboardSettings { fn_mode: "media".into(), ..Default::default() };
        set_touchbar_override(Some(3));
        let done = apply_apple(&d, &k).unwrap();
        set_touchbar_override(None);
        assert_eq!(std::fs::read_to_string(m.join("mode")).unwrap().trim(), "3");
        assert!(done.iter().any(|x| x.contains("Touch Bar mode = 3")));
        // the strip gave up: the chosen key mode comes back
        apply_apple(&d, &k).unwrap();
        assert_eq!(std::fs::read_to_string(m.join("mode")).unwrap().trim(), "2");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn when_the_strip_stops_the_firmware_keys_come_back_in_the_chosen_or_default_mode() {
        let d = tmp("tbr");
        let m = d.join("bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("mode"), "3\n").unwrap();
        assert_eq!(restore_touchbar_mode(&d, &KeyboardSettings::default()), 1);
        assert_eq!(std::fs::read_to_string(m.join("mode")).unwrap().trim(), "2", "the kernel's default: media keys");
        assert_eq!(restore_touchbar_mode(&d, &KeyboardSettings::default()), 0, "already right: no write");
        restore_touchbar_mode(&d, &KeyboardSettings { fn_mode: "fkeys".into(), ..Default::default() });
        assert_eq!(std::fs::read_to_string(m.join("mode")).unwrap().trim(), "1");
        restore_touchbar_mode(&d, &KeyboardSettings { touchbar: "off".into(), ..Default::default() });
        assert_eq!(std::fs::read_to_string(m.join("mode")).unwrap().trim(), "3", "off stays dark");
        assert_eq!(restore_touchbar_mode(&d.join("nothing"), &KeyboardSettings::default()), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_mode_file_is_found_again_after_the_driver_rebinds_under_a_new_name() {
        let d = tmp("tbn");
        let base = d.join("bus/hid/drivers/hid-appletb-kbd");
        std::fs::create_dir_all(base.join("0003:05AC:8302.0005")).unwrap();
        std::fs::write(base.join("0003:05AC:8302.0005/mode"), "3\n").unwrap();
        assert_eq!(restore_touchbar_mode(&d, &KeyboardSettings::default()), 1);
        // a configuration switch: the old HID device is gone, the driver binds a new one (a new number) which starts in its own default
        std::fs::remove_dir_all(base.join("0003:05AC:8302.0005")).unwrap();
        std::fs::create_dir_all(base.join("0003:05AC:8302.0009")).unwrap();
        std::fs::write(base.join("0003:05AC:8302.0009/mode"), "3\n").unwrap();
        assert_eq!(restore_touchbar_mode(&d, &KeyboardSettings::default()), 1);
        assert_eq!(std::fs::read_to_string(base.join("0003:05AC:8302.0009/mode")).unwrap().trim(), "2");
        // the ordinary sync does the same without being asked (mode 3 with nothing chosen), and leaves a held Fn (mode 1) and "off" alone
        std::fs::write(base.join("0003:05AC:8302.0009/mode"), "3\n").unwrap();
        apply_apple(&d, &KeyboardSettings::default()).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("0003:05AC:8302.0009/mode")).unwrap().trim(), "2");
        std::fs::write(base.join("0003:05AC:8302.0009/mode"), "1\n").unwrap();
        apply_apple(&d, &KeyboardSettings::default()).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("0003:05AC:8302.0009/mode")).unwrap().trim(), "1");
        std::fs::write(base.join("0003:05AC:8302.0009/mode"), "3\n").unwrap();
        apply_apple(&d, &KeyboardSettings { touchbar: "off".into(), ..Default::default() }).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("0003:05AC:8302.0009/mode")).unwrap().trim(), "3");
        let _ = std::fs::remove_dir_all(&d);
    }
}
