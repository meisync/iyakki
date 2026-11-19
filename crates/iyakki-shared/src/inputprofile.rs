//! Types moved out of `iyakki_agent::inputprofile` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The profile in use after Automatic is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    Mac,
    Pc,
}

/// Which Apple input devices the kernel lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub keyboard: bool,
    pub trackpad: bool,
}

/// Which way a navigation key goes (the shell's `Nav`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDir {
    Left,
    Right,
    Home,
    End,
}

/// One key the shell must deliver, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ctrl+letter (an uppercase letter means Shift is held too).
    Ctrl(char),
    Nav { dir: NavDir, shift: bool, ctrl: bool },
    Backspace,
    /// The pane-switch chord of Ctrl+Alt+Tab (Shift goes back).
    NextPane { back: bool },
}

/// What a chord does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mapped {
    /// Not a Mac chord: the key is handled as it always was.
    Pass,
    /// The keys to deliver instead of the press, in order.
    Keys(Vec<Out>),
    /// The press is dropped on purpose (Command+Q: nothing quits).
    Ignore,
}

/// The modifiers held when a key goes down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub command: bool,
    pub option: bool,
    pub control: bool,
    pub shift: bool,
}



/// The choices of `KeyboardSettings::profile`: "" is Automatic.
pub const PROFILES: &[&str] = &["", "mac", "pc"];

/// What the person sees named on the page.
pub fn label(choice: &str) -> &'static str {
    match choice {
        "mac" => "Mac",
        "pc" => "PC",
        _ => "Automatic",
    }
}

/// The next choice when the button is pressed (Automatic, Mac, PC, Automatic ...).
pub fn next_choice(choice: &str) -> &'static str {
    match choice {
        "" => "mac",
        "mac" => "pc",
        _ => "",
    }
}

/// Mac or PC. Automatic follows what was found; an unknown word is Automatic too (a hand-edited file).
pub fn resolve(choice: &str, found: Found) -> Resolved {
    match choice {
        "mac" => Resolved::Mac,
        "pc" => Resolved::Pc,
        _ if found.keyboard || found.trackpad => Resolved::Mac,
        _ => Resolved::Pc,
    }
}

/// Look through the text of `/proc/bus/input/devices` for Apple keyboards and trackpads (USB, Bluetooth, the T2 and SPI MacBooks).
/// The Touch Bar, mice and other Apple things do not count.
pub fn detect(devices: &str) -> Found {
    let mut f = Found::default();
    for block in devices.split("\n\n") {
        let (mut name, mut vendor) = (String::new(), String::new());
        for line in block.lines() {
            if let Some(n) = line.strip_prefix("N: Name=") {
                name = n.trim().trim_matches('"').to_lowercase();
            } else if let Some(i) = line.strip_prefix("I: ") {
                vendor = i.split_whitespace().find_map(|w| w.strip_prefix("Vendor=")).unwrap_or("").to_lowercase();
            }
        }
        let apple = vendor == "05ac" || vendor == "004c" || name.contains("apple") || name.contains("magic keyboard") || name.contains("magic trackpad");
        let skip = name.contains("touch bar") || name.contains("ibridge") || name.contains("display") || name.contains("backlight");
        if !apple || skip {
            continue;
        }
        if name.contains("keyboard") {
            f.keyboard = true;
        }
        if name.contains("trackpad") || name.contains("touchpad") {
            f.trackpad = true;
        }
    }
    f
}

pub const KEY_LEFTALT: u16 = 56;

pub const KEY_RIGHTALT: u16 = 100;

pub const KEY_LEFTMETA: u16 = 125;

pub const KEY_RIGHTMETA: u16 = 126;

/// What a modifier key is called in the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Command,
    Option,
    /// The PC's Win/Super key (PC profile only).
    Super,
    Alt,
}

/// The physical key an Apple keyboard has at a place; `Cmd` is the key next to the space bar on the Command side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Physical {
    Cmd,
    Opt,
}

/// The evdev code the kernel sends for a physical Apple key: Command is Meta and Option is Alt, unless `hid_apple` swaps them
/// (`swap` must be true only when the swap is on AND the key is on an Apple keyboard).
pub fn evdev_for(key: Physical, swap: bool) -> u16 {
    match (key, swap) {
        (Physical::Cmd, false) | (Physical::Opt, true) => KEY_LEFTMETA,
        (Physical::Cmd, true) | (Physical::Opt, false) => KEY_LEFTALT,
    }
}

/// True for the keys that are Alt or Meta (either side).
pub fn is_alt(code: u16) -> bool {
    code == KEY_LEFTALT || code == KEY_RIGHTALT
}

pub fn is_meta(code: u16) -> bool {
    code == KEY_LEFTMETA || code == KEY_RIGHTMETA
}

/// Does this evdev code mean the Command key in the Mac profile? Mac names keys by their print, so the answer undoes the kernel swap.
pub fn command_code_is(code: u16, swap: bool) -> bool {
    if swap {
        is_alt(code)
    } else {
        is_meta(code)
    }
}

/// Does this evdev code mean the Option key in the Mac profile?
pub fn option_code_is(code: u16, swap: bool) -> bool {
    if swap {
        is_meta(code)
    } else {
        is_alt(code)
    }
}

/// The logical modifier for an evdev Alt/Meta code (None for any other code): the matrix of profile x swap x key.
pub fn logical_modifier(profile: Resolved, swap: bool, code: u16) -> Option<Modifier> {
    if !is_alt(code) && !is_meta(code) {
        return None;
    }
    Some(match profile {
        Resolved::Mac if command_code_is(code, swap) => Modifier::Command,
        Resolved::Mac => Modifier::Option,
        Resolved::Pc if is_meta(code) => Modifier::Super,
        Resolved::Pc => Modifier::Alt,
    })
}

/// Whether the saved swap really reaches the keyboard being used: the option exists only in the Apple driver.
pub fn swap_in_effect(setting: Option<bool>, apple_keyboard: bool) -> bool {
    setting == Some(true) && apple_keyboard
}

/// The key pressed with the modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Letter(char),
    Left,
    Right,
    Up,
    Down,
    Backspace,
    Tab,
    Other,
}

/// The Mac profile's chords. In the PC profile (or with no Command or Option held) everything is `Pass`.
pub fn map_chord(profile: Resolved, h: Held, base: Base) -> Mapped {
    if profile != Resolved::Mac || !(h.command || h.option) {
        return Mapped::Pass;
    }
    if h.command {
        return match base {
            // Command+Q is dropped. Control+Command+Q is the sign-out chord, but the shell takes it (as a sign-out card) before this table is asked, so
            // this arm only ever sees Command+Q alone or a Control+Command+Q the shell did not take
            Base::Letter('q') => Mapped::Ignore,
            Base::Letter(c) if c.is_ascii_alphabetic() => {
                // Command+Shift+Z is redo, as Ctrl+Shift+Z is
                Mapped::Keys(vec![Out::Ctrl(if h.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })])
            }
            Base::Left => Mapped::Keys(vec![Out::Nav { dir: NavDir::Home, shift: h.shift, ctrl: false }]),
            Base::Right => Mapped::Keys(vec![Out::Nav { dir: NavDir::End, shift: h.shift, ctrl: false }]),
            Base::Up => Mapped::Keys(vec![Out::Nav { dir: NavDir::Home, shift: h.shift, ctrl: true }]),
            Base::Down => Mapped::Keys(vec![Out::Nav { dir: NavDir::End, shift: h.shift, ctrl: true }]),
            // select to the start of the line, then delete the selection
            Base::Backspace => Mapped::Keys(vec![Out::Nav { dir: NavDir::Home, shift: true, ctrl: false }, Out::Backspace]),
            Base::Tab => Mapped::Keys(vec![Out::NextPane { back: h.shift }]),
            _ => Mapped::Pass,
        };
    }
    match base {
        Base::Left => Mapped::Keys(vec![Out::Nav { dir: NavDir::Left, shift: h.shift, ctrl: true }]),
        Base::Right => Mapped::Keys(vec![Out::Nav { dir: NavDir::Right, shift: h.shift, ctrl: true }]),
        Base::Backspace => Mapped::Keys(vec![Out::Nav { dir: NavDir::Left, shift: true, ctrl: true }, Out::Backspace]),
        _ => Mapped::Pass,
    }
}

/// The name of a shortcut as the live row shows it: "Command+C" (or "Control+C" in the PC profile).
pub fn physical_name(profile: Resolved, h: Held, key: &str) -> String {
    let mut s = String::new();
    let (cmd_word, opt_word) = match profile {
        Resolved::Mac => ("Command", "Option"),
        Resolved::Pc => ("Windows", "Alt"),
    };
    for (on, w) in [(h.control, "Control"), (h.option, opt_word), (h.command, cmd_word), (h.shift, "Shift")] {
        if on {
            s.push_str(w);
            s.push('+');
        }
    }
    s.push_str(key);
    s
}

/// What a chord means in words, for the live row; None when it is just typing.
pub fn meaning(profile: Resolved, h: Held, base: Base) -> Option<&'static str> {
    let by_letter = |c: char, shift: bool| -> Option<&'static str> {
        Some(match (c.to_ascii_lowercase(), shift) {
            ('c', _) => "Copy",
            ('x', _) => "Cut",
            ('v', _) => "Paste",
            ('a', _) => "Select all",
            ('z', false) => "Undo",
            ('z', true) => "Redo",
            ('y', _) => "Redo",
            ('f', _) => "Find",
            ('s', _) => "Save",
            ('w', _) => "Close tab",
            ('t', _) => "New tab",
            ('n', _) => "New",
            ('l', _) => "Address / go to line",
            ('r', _) => "Reload / run",
            ('q', _) => "Quit (ignored: IYAKKI never quits on a key)",
            _ => return None,
        })
    };
    match (profile, base) {
        (Resolved::Mac, _) if h.command => match base {
            Base::Letter(c) => by_letter(c, h.shift),
            Base::Left => Some("Start of line"),
            Base::Right => Some("End of line"),
            Base::Up => Some("Top of the document"),
            Base::Down => Some("Bottom of the document"),
            Base::Backspace => Some("Delete to the start of the line"),
            Base::Tab => Some("Next pane"),
            _ => None,
        },
        (Resolved::Mac, _) if h.option => match base {
            Base::Left => Some("Word left"),
            Base::Right => Some("Word right"),
            Base::Backspace => Some("Delete the word before"),
            _ => None,
        },
        (_, Base::Letter(c)) if h.control => by_letter(c, h.shift),
        (_, Base::Left) if h.control => Some("Word left"),
        (_, Base::Right) if h.control => Some("Word right"),
        _ => None,
    }
}

/// The pointer choices a profile starts from (the "Reset to profile defaults" button).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerDefaults {
    pub trackpad_tap: bool,
    pub trackpad_natural: bool,
    pub wheel_natural: bool,
    pub speed_pct: u32,
    /// Three-finger drag is built (Settings, Keyboard, key J) and is off in both profiles until the person turns it on.
    pub three_finger_drag: bool,
}

pub fn pointer_defaults(profile: Resolved) -> PointerDefaults {
    match profile {
        // two-finger scroll, tap to click, two-finger tap or Control+click is a right click, natural trackpad, classic wheel
        Resolved::Mac => PointerDefaults { trackpad_tap: true, trackpad_natural: true, wheel_natural: false, speed_pct: 100, three_finger_drag: false },
        // as before this profile existed
        Resolved::Pc => PointerDefaults { trackpad_tap: true, trackpad_natural: true, wheel_natural: false, speed_pct: 100, three_finger_drag: false },
    }
}

/// "Scrolled content up (natural)" for the live row. `delta` is the shell's: positive = towards older content (the page moves down).
/// `natural` says which setting applied; `pad` says trackpad or wheel.
pub fn scroll_words(delta: i32, natural: bool, pad: bool) -> String {
    // positive = towards older content = the page slides down the screen
    let content = if delta > 0 { "down" } else { "up" };
    format!("Scrolled content {content} ({}, {})", if natural { "natural" } else { "traditional" }, if pad { "trackpad" } else { "mouse wheel" })
}
