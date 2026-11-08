//! The input profile as the shell uses it (Settings -> Keyboard -> Input profile): the resolved profile (Mac or PC), whether the
//! Apple driver's Option/Command swap really applies, the Apple devices found, and the "What you will see" rows (the last key and
//! the last scroll). The decisions are pure and live in `iyakki_agent::inputprofile`; this file keeps the current state for the
//! input thread and turns the agent's answers into the shell's own keys.
//!
//! Only [`refresh`] reads the kernel's device list (the shell calls it from its settings sync, never a tile).
//! Like `scrolleng`, the state is shared with the input thread in the program and is per test thread in tests.

use crate::panes;
use crate::ui::{Event, Key, Nav};
use iyakki_agent::inputprofile::{self as ip, Found, Held, Mapped, NavDir, Out, Resolved};
use iyakki_agent::keyboard::KeyboardSettings;

#[derive(Clone, Debug)]
struct State {
    resolved: Resolved,
    swap: bool,
    found: Found,
    choice: String,
    last_key: String,
    /// (delta of the last scroll step, natural, trackpad)
    last_scroll: Option<(i32, bool, bool)>,
}

impl State {
    const fn new() -> State {
        State { resolved: Resolved::Pc, swap: false, found: Found { keyboard: false, trackpad: false }, choice: String::new(), last_key: String::new(), last_scroll: None }
    }
}

#[cfg(not(test))]
static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State::new());
#[cfg(test)]
thread_local! {
    static STATE: std::cell::RefCell<State> = const { std::cell::RefCell::new(State::new()) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&mut STATE.lock().unwrap_or_else(|p| p.into_inner()))
    }
    #[cfg(test)]
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// The settings (and what was found) become the live state. The saved swap counts only where an Apple keyboard is present.
fn set(k: &KeyboardSettings, found: Found) {
    with(|s| {
        s.found = found;
        s.choice = k.profile.clone();
        s.resolved = ip::resolve(&k.profile, found);
        s.swap = ip::swap_in_effect(k.swap_opt_cmd, found.keyboard);
    });
}

/// Use new settings with the devices found last time (the Settings page calls this, so a change is live at once).
pub fn apply(k: &KeyboardSettings) {
    set(k, with(|s| s.found));
}

/// Look at the kernel's device list again, then apply. Called from the shell's settings sync.
pub fn refresh(k: &KeyboardSettings) {
    let text = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    set(k, ip::detect(&text));
}

/// Same with the list given (tests, fixtures).
pub fn refresh_from(k: &KeyboardSettings, devices: &str) {
    set(k, ip::detect(devices));
}

pub fn resolved() -> Resolved {
    with(|s| s.resolved)
}
pub fn swap_in_effect() -> bool {
    with(|s| s.swap)
}
pub fn found() -> Found {
    with(|s| s.found)
}

pub fn note_key(text: String) {
    with(|s| s.last_key = text);
}

/// Remember the direction of a scroll step for the live row.
pub fn note_scroll(delta: i32, pad: bool) {
    if delta == 0 {
        return;
    }
    let c = crate::scrolleng::config();
    with(|s| s.last_scroll = Some((delta, if pad { c.pad_natural } else { c.wheel_natural }, pad)));
}

/// The two live rows: (last key, last scroll), each with a hint when nothing happened yet.
pub fn live_rows() -> (String, String) {
    with(|s| {
        let key = if s.last_key.is_empty() { "press a key, for example Command+C or Ctrl+C".to_string() } else { s.last_key.clone() };
        let scroll = match s.last_scroll {
            Some((d, natural, pad)) => ip::scroll_words(d, natural, pad),
            None => "scroll with two fingers or the wheel".to_string(),
        };
        (key, scroll)
    })
}

/// The line that says which profile is in use and why.
pub fn status_line() -> String {
    with(|s| {
        let p = if s.resolved == Resolved::Mac { "Mac" } else { "PC" };
        let why = match (s.choice.as_str(), s.found.keyboard || s.found.trackpad) {
            ("", true) => "Automatic: an Apple keyboard or trackpad was found",
            ("", false) => "Automatic: no Apple keyboard or trackpad found",
            _ => "chosen by you",
        };
        format!("{p} ({why}); Option/Command swap {}", if s.swap { "in effect" } else { "not in effect" })
    })
}

// ------------------------------------------------------------------ the input thread's side

/// The base key of an evdev code for the chord table.
fn base_of(code: u16) -> ip::Base {
    match crate::input::translate(code, false, false) {
        Some(Key::Char(c)) if c.is_ascii_alphabetic() => ip::Base::Letter(c.to_ascii_lowercase()),
        Some(Key::Left) => ip::Base::Left,
        Some(Key::Right) => ip::Base::Right,
        Some(Key::Up) => ip::Base::Up,
        Some(Key::Down) => ip::Base::Down,
        Some(Key::Backspace) => ip::Base::Backspace,
        Some(Key::Tab) => ip::Base::Tab,
        _ => ip::Base::Other,
    }
}

fn key_name(code: u16) -> String {
    match crate::input::translate(code, false, false) {
        Some(Key::Char(c)) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(Key::Char(' ')) => "Space".into(),
        Some(Key::Char(c)) => c.to_string(),
        Some(other) => format!("{other:?}"),
        None => format!("key {code}"),
    }
}

/// What the shell does for a press of `code` with these modifiers: `Some(events)` when the profile takes the key (possibly no events at
/// all: Command+Q), `None` when the key is handled the usual way. Also fills the "last key" row for a press (`first`).
pub fn on_press(code: u16, held: Held, first: bool) -> Option<Vec<Event>> {
    let profile = resolved();
    let base = base_of(code);
    let mapped = ip::map_chord(profile, held, base);
    if first && (held.command || held.option || held.control) {
        if let Some(m) = ip::meaning(profile, held, base) {
            note_key(format!("{} -> {m}", ip::physical_name(profile, held, &key_name(code))));
        } else if !matches!(mapped, Mapped::Pass) {
            note_key(format!("{} -> {}", ip::physical_name(profile, held, &key_name(code)), "a shortcut"));
        }
    } else if first && crate::input::translate(code, false, false).is_some() {
        note_key(match crate::input::translate(code, held.shift, false) {
            Some(Key::Char(c)) => format!("{} -> types \"{c}\"", ip::physical_name(profile, held, &key_name(code))),
            _ => format!("{} -> {}", ip::physical_name(profile, held, &key_name(code)), key_name(code)),
        });
    }
    match mapped {
        Mapped::Pass => None,
        Mapped::Ignore => Some(Vec::new()),
        Mapped::Keys(v) => Some(v.into_iter().filter_map(event_of).collect()),
    }
}

fn event_of(o: Out) -> Option<Event> {
    Some(match o {
        Out::Ctrl(c) => Event::Key(Key::Ctrl(c)),
        Out::Backspace => Event::Key(Key::Backspace),
        Out::Nav { dir, shift, ctrl } => {
            let nav = match dir {
                NavDir::Left => Nav::Left,
                NavDir::Right => Nav::Right,
                NavDir::Home => Nav::Home,
                NavDir::End => Nav::End,
            };
            if !shift && !ctrl {
                Event::Key(match dir {
                    NavDir::Left => Key::Left,
                    NavDir::Right => Key::Right,
                    NavDir::Home => Key::Home,
                    NavDir::End => Key::End,
                })
            } else {
                Event::Key(Key::Nav { nav, shift, ctrl })
            }
        }
        Out::NextPane { back } => Event::Layout(panes::chord_from_evdev(15, back)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::inputprofile::{evdev_for, Physical};

    fn mac_setup() {
        let k = KeyboardSettings { profile: "mac".into(), ..Default::default() };
        refresh_from(&k, "");
    }
    fn cmd() -> Held {
        Held { command: true, ..Default::default() }
    }

    #[test]
    fn the_mac_profile_turns_command_c_v_into_the_shells_copy_and_paste_and_control_c_stays_control() {
        mac_setup();
        assert_eq!(on_press(46, cmd(), true), Some(vec![Event::Key(Key::Ctrl('c'))])); // KEY_C
        assert_eq!(live_rows().0, "Command+C -> Copy");
        assert_eq!(on_press(47, Held { command: true, shift: true, ..Default::default() }, true), Some(vec![Event::Key(Key::Ctrl('V'))]));
        assert_eq!(on_press(46, Held { control: true, ..Default::default() }, true), None, "Control+C is the usual Control+C");
        assert_eq!(live_rows().0, "Control+C -> Copy");
        assert_eq!(on_press(16, cmd(), true), Some(vec![]), "Command+Q is dropped");
        assert!(live_rows().0.starts_with("Command+Q -> Quit (ignored"));
        assert_eq!(on_press(105, cmd(), true), Some(vec![Event::Key(Key::Home)]));
        assert_eq!(on_press(105, Held { option: true, ..Default::default() }, true), Some(vec![Event::Key(Key::Nav { nav: Nav::Left, shift: false, ctrl: true })]));
        assert_eq!(on_press(14, Held { option: true, ..Default::default() }, true), Some(vec![Event::Key(Key::Nav { nav: Nav::Left, shift: true, ctrl: true }), Event::Key(Key::Backspace)]));
        assert!(matches!(on_press(15, cmd(), true).as_deref(), Some([Event::Layout(_)])), "Command+Tab switches panes");
    }

    #[test]
    fn the_pc_profile_changes_nothing_and_a_plain_key_is_described() {
        refresh_from(&KeyboardSettings { profile: "pc".into(), ..Default::default() }, "");
        for held in [cmd(), Held { option: true, ..Default::default() }, Held { control: true, ..Default::default() }] {
            assert_eq!(on_press(46, held, true), None);
        }
        on_press(30, Held::default(), true);
        assert_eq!(live_rows().0, "A -> types \"a\"");
    }

    #[test]
    fn automatic_uses_the_found_devices_and_the_swap_counts_only_on_an_apple_keyboard() {
        let t2 = "I: Bus=0003 Vendor=05ac Product=027b Version=0111\nN: Name=\"Apple Inc. Apple Internal Keyboard / Trackpad\"\nH: Handlers=kbd event2\n\n";
        let generic = "I: Bus=0003 Vendor=046d Product=c31c Version=0110\nN: Name=\"Logitech USB Keyboard\"\nH: Handlers=kbd event3\n\n";
        let k = KeyboardSettings { swap_opt_cmd: Some(true), ..Default::default() };
        refresh_from(&k, t2);
        assert_eq!((resolved(), swap_in_effect()), (Resolved::Mac, true));
        assert!(status_line().contains("Automatic: an Apple keyboard"), "{}", status_line());
        refresh_from(&k, generic);
        assert_eq!((resolved(), swap_in_effect()), (Resolved::Pc, false));
        assert!(status_line().contains("no Apple keyboard"));
        // the page applies a new choice at once, with the devices found last time
        apply(&KeyboardSettings { profile: "mac".into(), ..k.clone() });
        assert_eq!(resolved(), Resolved::Mac);
        // with the swap in effect the physical Command key (sent as Alt by the driver) is still Command
        refresh_from(&KeyboardSettings { profile: "mac".into(), swap_opt_cmd: Some(true), ..Default::default() }, t2);
        assert_eq!(ip::logical_modifier(resolved(), swap_in_effect(), evdev_for(Physical::Cmd, true)), Some(ip::Modifier::Command));
    }

    #[test]
    fn the_last_scroll_row_follows_the_direction_and_the_setting() {
        crate::scrolleng::request_config(crate::fnkeys::scroll_config(KeyboardSettings::default().scroll()));
        assert_eq!(live_rows().1, "scroll with two fingers or the wheel");
        note_scroll(-40, true);
        assert_eq!(live_rows().1, "Scrolled content up (natural, trackpad)");
        note_scroll(40, false);
        assert_eq!(live_rows().1, "Scrolled content down (traditional, mouse wheel)");
        note_scroll(0, false);
        assert_eq!(live_rows().1, "Scrolled content down (traditional, mouse wheel)", "no movement changes nothing");
    }
}
