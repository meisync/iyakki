//! Input profile (Settings -> Keyboard): Automatic, Mac or PC. Pure logic only (no device, no file, no program), so it is tested here
//! and the shell (`iyakki-shell::inputprofile`, `input.rs`) only feeds it evdev codes and turns the answers into its own keys.
//!
//! * [`detect`] reads the text of `/proc/bus/input/devices` (the shell reads the file) and says whether an Apple keyboard or trackpad
//!   is present; [`resolve`] turns the choice ("" automatic, "mac", "pc") plus that into Mac or PC.
//! * Mac: the Command key is the shortcut key where the PC's Ctrl is ([`map_chord`]); Option moves by word; Control stays Control.
//! * The hid_apple `swap_opt_cmd` option swaps the two keys inside the kernel. In the Mac profile the keys are named by what is
//!   printed on them ([`command_code_is`]), so the swap changes nothing there and the two never swap twice; in the PC profile the swap
//!   is the kernel's alone (the key under the thumb sends Alt, as before).

use crate::keyboard::KeyboardSettings;
pub use iyakki_shared::inputprofile::*;
pub use iyakki_shared::inputprofile::{Found, Held, Mapped, NavDir, Out, Resolved};
pub use iyakki_shared::keyboard::{pointer_defaults, PointerDefaults};








// evdev codes









// ------------------------------------------------------------------ chords









// ------------------------------------------------------------------ pointer defaults


#[cfg(test)]
mod tests {
    use super::*;

    const USB_APPLE_MAGIC: &str = "I: Bus=0003 Vendor=05ac Product=0267 Version=0111\nN: Name=\"Apple Inc. Magic Keyboard\"\nP: Phys=usb-0000:00:14.0-1/input0\nH: Handlers=sysrq kbd event3 leds\nB: PROP=0\n\nI: Bus=0003 Vendor=05ac Product=0267 Version=0111\nN: Name=\"Apple Inc. Magic Keyboard Consumer Control\"\nH: Handlers=kbd event4\n\n";
    const T2: &str = "I: Bus=0003 Vendor=05ac Product=027b Version=0111\nN: Name=\"Apple Inc. Apple Internal Keyboard / Trackpad\"\nH: Handlers=sysrq kbd event2 leds mouse0\n\nI: Bus=0003 Vendor=05ac Product=8102 Version=0111\nN: Name=\"Apple Inc. Touch Bar Backlight\"\nH: Handlers=kbd event5\n\nI: Bus=0003 Vendor=05ac Product=8302 Version=0111\nN: Name=\"Apple Inc. Apple Touch Bar\"\nH: Handlers=kbd event6\n\n";
    const SPI: &str = "I: Bus=001d Vendor=05ac Product=0273 Version=0000\nN: Name=\"Apple SPI Keyboard\"\nH: Handlers=sysrq kbd event3 leds\n\nI: Bus=001d Vendor=05ac Product=0273 Version=0000\nN: Name=\"Apple SPI Touchpad\"\nH: Handlers=mouse1 event4\n\n";
    const GENERIC: &str = "I: Bus=0003 Vendor=046d Product=c31c Version=0110\nN: Name=\"Logitech USB Keyboard\"\nH: Handlers=sysrq kbd event3 leds\n\nI: Bus=0003 Vendor=046d Product=c077 Version=0111\nN: Name=\"Logitech USB Optical Mouse\"\nH: Handlers=mouse0 event4\n\n";
    const APPLE_MOUSE_AND_TOUCHBAR_ONLY: &str = "I: Bus=0003 Vendor=05ac Product=8302 Version=0111\nN: Name=\"Apple Inc. Apple Touch Bar\"\nH: Handlers=kbd event6\n\nI: Bus=0005 Vendor=004c Product=0269 Version=0001\nN: Name=\"Magic Mouse\"\nH: Handlers=mouse2 event7\n\n";
    const BT_MAGIC_TRACKPAD: &str = "I: Bus=0005 Vendor=004c Product=0265 Version=0001\nN: Name=\"Magic Trackpad 2\"\nH: Handlers=mouse2 event7\n\n";

    #[test]
    fn apple_keyboards_and_trackpads_are_found_from_the_kernels_device_list() {
        assert_eq!(detect(USB_APPLE_MAGIC), Found { keyboard: true, trackpad: false });
        assert_eq!(detect(T2), Found { keyboard: true, trackpad: true }, "one T2 device is both; the Touch Bar adds nothing");
        assert_eq!(detect(SPI), Found { keyboard: true, trackpad: true });
        assert_eq!(detect(GENERIC), Found::default());
        assert_eq!(detect(APPLE_MOUSE_AND_TOUCHBAR_ONLY), Found::default(), "a Magic Mouse and the Touch Bar are no keyboard or trackpad");
        assert_eq!(detect(BT_MAGIC_TRACKPAD), Found { keyboard: false, trackpad: true });
        assert_eq!(detect(""), Found::default());
        assert_eq!(detect(&format!("{GENERIC}{T2}")), Found { keyboard: true, trackpad: true }, "an Apple keyboard beside a PC one");
    }

    #[test]
    fn automatic_follows_the_machine_and_a_choice_wins() {
        let apple = Found { keyboard: true, trackpad: false };
        let pad = Found { keyboard: false, trackpad: true };
        let none = Found::default();
        for (choice, found, want) in [
            ("", apple, Resolved::Mac),
            ("", pad, Resolved::Mac),
            ("", none, Resolved::Pc),
            ("mac", none, Resolved::Mac),
            ("pc", apple, Resolved::Pc),
            ("pc", pad, Resolved::Pc),
            ("typo", apple, Resolved::Mac),
            ("typo", none, Resolved::Pc),
        ] {
            assert_eq!(resolve(choice, found), want, "{choice:?} {found:?}");
        }
        assert_eq!((next_choice(""), next_choice("mac"), next_choice("pc"), next_choice("?")), ("mac", "pc", "", ""));
        assert_eq!((label(""), label("mac"), label("pc")), ("Automatic", "Mac", "PC"));
    }

    #[test]
    fn the_profile_swap_and_physical_key_matrix_never_swaps_twice() {
        // (profile, swap, physical key) -> logical key
        for (profile, swap, phys, want) in [
            (Resolved::Mac, false, Physical::Cmd, Modifier::Command),
            (Resolved::Mac, false, Physical::Opt, Modifier::Option),
            (Resolved::Mac, true, Physical::Cmd, Modifier::Command),
            (Resolved::Mac, true, Physical::Opt, Modifier::Option),
            (Resolved::Pc, false, Physical::Cmd, Modifier::Super),
            (Resolved::Pc, false, Physical::Opt, Modifier::Alt),
            (Resolved::Pc, true, Physical::Cmd, Modifier::Alt),
            (Resolved::Pc, true, Physical::Opt, Modifier::Super),
        ] {
            let code = evdev_for(phys, swap);
            assert_eq!(logical_modifier(profile, swap, code), Some(want), "{profile:?} swap {swap} {phys:?}");
        }
        // the right-hand keys behave like the left
        assert_eq!(logical_modifier(Resolved::Mac, false, KEY_RIGHTMETA), Some(Modifier::Command));
        assert_eq!(logical_modifier(Resolved::Mac, false, KEY_RIGHTALT), Some(Modifier::Option));
        assert_eq!(logical_modifier(Resolved::Mac, true, KEY_RIGHTALT), Some(Modifier::Command));
        assert_eq!(logical_modifier(Resolved::Pc, false, 30), None, "other keys are no modifiers");
        // the swap reaches only an Apple keyboard
        assert!(swap_in_effect(Some(true), true));
        assert!(!swap_in_effect(Some(true), false) && !swap_in_effect(Some(false), true) && !swap_in_effect(None, true));
        // a PC keyboard with the swap saved: Windows key is Command in the Mac profile
        let sw = swap_in_effect(Some(true), false);
        assert_eq!(logical_modifier(Resolved::Mac, sw, KEY_LEFTMETA), Some(Modifier::Command));
    }

    fn mac(command: bool, option: bool, shift: bool, base: Base) -> Mapped {
        map_chord(Resolved::Mac, Held { command, option, control: false, shift }, base)
    }
    fn ctrl(c: char) -> Mapped {
        Mapped::Keys(vec![Out::Ctrl(c)])
    }

    #[test]
    fn command_letters_are_the_shortcuts_the_pcs_ctrl_letters_are() {
        for c in ['c', 'x', 'v', 'a', 'z', 'f', 's', 'w', 't', 'n', 'l', 'r'] {
            assert_eq!(mac(true, false, false, Base::Letter(c)), ctrl(c), "Command+{c}");
        }
        assert_eq!(mac(true, false, true, Base::Letter('z')), ctrl('Z'), "Command+Shift+Z is redo");
        assert_eq!(mac(true, false, true, Base::Letter('v')), ctrl('V'));
        assert_eq!(mac(true, false, false, Base::Letter('C')), ctrl('c'), "caps lock does not matter");
        assert_eq!(mac(true, false, false, Base::Letter('q')), Mapped::Ignore, "Command+Q does not quit");
        assert_eq!(map_chord(Resolved::Mac, Held { command: true, control: true, ..Default::default() }, Base::Letter('q')), Mapped::Ignore, "Control+Command+Q: the shell asks to sign out before this table is asked; if it ever got here nothing must quit");
    }

    #[test]
    fn command_and_option_move_the_cursor_like_a_mac() {
        let nav = |dir, shift, ctrl| Mapped::Keys(vec![Out::Nav { dir, shift, ctrl }]);
        assert_eq!(mac(true, false, false, Base::Left), nav(NavDir::Home, false, false));
        assert_eq!(mac(true, false, false, Base::Right), nav(NavDir::End, false, false));
        assert_eq!(mac(true, false, true, Base::Left), nav(NavDir::Home, true, false), "with Shift it selects");
        assert_eq!(mac(true, false, false, Base::Up), nav(NavDir::Home, false, true));
        assert_eq!(mac(true, false, false, Base::Down), nav(NavDir::End, false, true));
        assert_eq!(mac(false, true, false, Base::Left), nav(NavDir::Left, false, true), "Option+Left is a word jump");
        assert_eq!(mac(false, true, false, Base::Right), nav(NavDir::Right, false, true));
        assert_eq!(mac(false, true, true, Base::Right), nav(NavDir::Right, true, true));
        assert_eq!(mac(false, true, false, Base::Backspace), Mapped::Keys(vec![Out::Nav { dir: NavDir::Left, shift: true, ctrl: true }, Out::Backspace]));
        assert_eq!(mac(true, false, false, Base::Backspace), Mapped::Keys(vec![Out::Nav { dir: NavDir::Home, shift: true, ctrl: false }, Out::Backspace]));
        assert_eq!(mac(true, false, false, Base::Tab), Mapped::Keys(vec![Out::NextPane { back: false }]));
        assert_eq!(mac(true, false, true, Base::Tab), Mapped::Keys(vec![Out::NextPane { back: true }]));
    }

    #[test]
    fn the_pc_profile_and_plain_keys_are_untouched() {
        for base in [Base::Letter('c'), Base::Left, Base::Backspace, Base::Tab, Base::Other] {
            for h in [Held { command: true, ..Default::default() }, Held { option: true, ..Default::default() }, Held { control: true, ..Default::default() }] {
                assert_eq!(map_chord(Resolved::Pc, h, base), Mapped::Pass, "{h:?} {base:?}");
            }
        }
        assert_eq!(mac(false, false, false, Base::Letter('c')), Mapped::Pass, "plain c");
        assert_eq!(map_chord(Resolved::Mac, Held { control: true, ..Default::default() }, Base::Letter('c')), Mapped::Pass, "Control+C stays Control+C on a Mac");
        assert_eq!(mac(true, false, false, Base::Other), Mapped::Pass);
        assert_eq!(mac(false, true, false, Base::Up), Mapped::Pass);
    }

    #[test]
    fn the_live_row_names_the_keys_and_what_they_do() {
        let cmd = Held { command: true, ..Default::default() };
        assert_eq!(physical_name(Resolved::Mac, cmd, "C"), "Command+C");
        assert_eq!(meaning(Resolved::Mac, cmd, Base::Letter('c')), Some("Copy"));
        assert_eq!(meaning(Resolved::Mac, Held { command: true, shift: true, ..Default::default() }, Base::Letter('z')), Some("Redo"));
        assert_eq!(physical_name(Resolved::Mac, Held { command: true, shift: true, ..Default::default() }, "Z"), "Command+Shift+Z");
        assert_eq!(meaning(Resolved::Mac, Held { option: true, ..Default::default() }, Base::Left), Some("Word left"));
        let ctl = Held { control: true, ..Default::default() };
        assert_eq!(physical_name(Resolved::Pc, ctl, "V"), "Control+V");
        assert_eq!(meaning(Resolved::Pc, ctl, Base::Letter('v')), Some("Paste"));
        assert_eq!(meaning(Resolved::Pc, Held::default(), Base::Letter('v')), None);
        assert_eq!(physical_name(Resolved::Pc, Held { command: true, ..Default::default() }, "E"), "Windows+E");
        assert_eq!(scroll_words(-120, true, true), "Scrolled content up (natural, trackpad)");
        assert_eq!(scroll_words(120, false, false), "Scrolled content down (traditional, mouse wheel)");
    }

    #[test]
    fn the_pointer_defaults_and_the_reset() {
        let m = pointer_defaults(Resolved::Mac);
        assert_eq!((m.trackpad_tap, m.trackpad_natural, m.wheel_natural, m.speed_pct, m.three_finger_drag), (true, true, false, 100, false));
        assert_eq!(pointer_defaults(Resolved::Pc).trackpad_natural, KeyboardSettings::default().trackpad().1, "the PC profile starts where it always did");
        let mut k = KeyboardSettings { trackpad_natural: Some(false), wheel_natural: Some(true), scroll_speed_pct: 250, wheel_lines: 8, scroll_smooth: Some(false), repeat_rate: 20, swap_opt_cmd: Some(true), profile: "mac".into(), ..Default::default() };
        k.reset_pointer(Resolved::Mac);
        assert_eq!(k.trackpad(), (true, true));
        let s = k.scroll();
        assert_eq!((s.speed_pct, s.wheel_natural, s.pad_natural, s.smooth, s.wheel_lines), (100, false, true, true, 3));
        assert_eq!((k.repeat_rate, k.swap_opt_cmd, k.profile.as_str()), (20, Some(true), "mac"), "keyboard choices are not touched");
        assert!(k.validate().is_ok());
    }

    #[test]
    fn old_settings_files_load_with_automatic_and_the_profile_round_trips() {
        let old: KeyboardSettings = serde_json::from_str("{\"fn_mode\":\"media\",\"swap_opt_cmd\":true,\"trackpad_natural\":false}").unwrap();
        assert_eq!(old.profile, "");
        assert_eq!((old.swap_opt_cmd, old.trackpad_natural), (Some(true), Some(false)));
        assert!(old.validate().is_ok());
        let k = KeyboardSettings { profile: "mac".into(), ..old };
        let j = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<KeyboardSettings>(&j).unwrap(), k);
        assert!(KeyboardSettings { profile: "linux".into(), ..Default::default() }.validate().is_err());
        for p in PROFILES {
            assert!(KeyboardSettings { profile: (*p).into(), ..Default::default() }.validate().is_ok());
        }
    }
}
