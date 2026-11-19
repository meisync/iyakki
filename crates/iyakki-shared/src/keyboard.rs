//! Types moved out of `iyakki_agent::keyboard` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Choices of [`KeyboardSettings::fn_mode`] ("" leaves the keyboard as the kernel has it).
pub const FN_MODES: [&str; 4] = ["", "media", "fkeys", "auto"];

/// Limits of [`KeyboardSettings::pointer_speed_pct`] and [`KeyboardSettings::touchpad_speed_pct`]; the Settings page steps by [`POINTER_SPEED_STEP`].
pub const POINTER_SPEED_RANGE: (u32, u32) = (25, 300);

/// The scrolling choices with the defaults filled in (what the shell's scroll engine is given).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollPrefs {
    pub speed_pct: u32,
    pub wheel_natural: bool,
    pub pad_natural: bool,
    pub smooth: bool,
    pub momentum: bool,
    pub wheel_lines: u32,
}

/// Choices of [`KeyboardSettings::touchbar`].
pub const TOUCHBAR_CHOICES: [&str; 4] = ["", "off", "keys", "strip"];

use serde::{Deserialize, Serialize};

/// Choices of [`KeyboardSettings::profile`].
pub const PROFILES: &[&str] = &["", "mac", "pc"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    /// Input profile (`inputprofile`): "" (automatic: Mac when an Apple keyboard or trackpad is found, else PC), "mac" or "pc".
    pub profile: String,
    /// "" (not touched), "media" (media keys first, Fn+F-key gives F1..F12), "fkeys" (F-keys first) or "auto" (the keyboard decides).
    pub fn_mode: String,
    /// Apple ISO layout fix (swaps the keys left of Z and below Esc); None = not touched.
    pub iso_layout: Option<bool>,
    /// Swap the Option and Command keys; None = not touched.
    pub swap_opt_cmd: Option<bool>,
    /// Trackpad: a short tap clicks; None = not chosen (on).
    pub trackpad_tap: Option<bool>,
    /// Trackpad: three fingers moving drag (the left button is held while they move); None = not chosen (off).
    pub three_finger_drag: Option<bool>,
    /// Home: a sideways swipe moves to the next or previous group; None = not chosen (on).
    pub home_swipe: Option<bool>,
    /// Home swipe: how many fingers on a touchpad make the swipe (2 or 3); 0 = not chosen (3). A touch screen always uses one finger.
    pub home_swipe_fingers: u8,
    /// Trackpad: two-finger scrolling follows the fingers ("natural"); None = not chosen (on, like a phone and a Mac).
    pub trackpad_natural: Option<bool>,
    /// Scrolling speed in percent for the wheel and the trackpad (50 to 300); 0 = not chosen (100).
    pub scroll_speed_pct: u32,
    /// Pointer speed of a mouse (relative device) in percent (25 to 300); 0 = not chosen (100).
    pub pointer_speed_pct: u32,
    /// Pointer speed of a touchpad in percent (25 to 300), on top of the pad's own acceleration; 0 = not chosen (100).
    pub touchpad_speed_pct: u32,
    /// Mouse wheel: rolling towards you moves the page up ("natural"); None = not chosen (off: the classic direction).
    pub wheel_natural: Option<bool>,
    /// Scrolling eases to its target instead of jumping; None = not chosen (on).
    pub scroll_smooth: Option<bool>,
    /// A trackpad scroll keeps going and slows down after the fingers lift; None = not chosen (on).
    pub scroll_momentum: Option<bool>,
    /// Lines one mouse wheel click moves (1 to 10); 0 = not chosen (3).
    pub wheel_lines: u32,
    /// Milliseconds a key is held before it repeats (0 = the kernel's own, 250).
    pub repeat_delay_ms: u32,
    /// Repeats per second once it does (0 = the kernel's own, 30).
    pub repeat_rate: u32,
    /// The Touch Bar: "" (the default: IYAKKI's own strip where the display works, the keys otherwise), "off" (dark), "keys" (the
    /// firmware's Esc, F-keys or media keys only) or "strip" (IYAKKI's strip; keys when the display cannot be used).
    pub touchbar: String,
    /// Which strips the Touch Bar cycles through: any of "system", "recent", "ai", "app". Empty = all four.
    pub touchbar_strips: Vec<String>,
    /// Picture flips for the display (bit 0 mirror along the bar, bit 1 mirror across): for a bar that shows the picture mirrored.
    pub touchbar_flip: u8,
}

/// Limits of [`KeyboardSettings::scroll_speed_pct`] and [`KeyboardSettings::wheel_lines`], and what "not chosen" means.
pub const SCROLL_SPEED_RANGE: (u32, u32) = (50, 300);
pub const SCROLL_SPEED_DEFAULT: u32 = 100;
pub const POINTER_SPEED_DEFAULT: u32 = 100;
pub const POINTER_SPEED_STEP: u32 = 5;
pub const WHEEL_LINES_RANGE: (u32, u32) = (1, 10);
pub const WHEEL_LINES_DEFAULT: u32 = 3;


/// The strips of the Touch Bar, in the order they are cycled.
pub const TOUCHBAR_STRIPS: [&str; 4] = ["system", "recent", "ai", "app"];

/// What the Touch Bar does for a setting and a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchBarMode {
    Off,
    Keys,
    Strip,
}


impl KeyboardSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !PROFILES.contains(&self.profile.as_str()) {
            return Err("the input profile must be automatic, mac or pc".into());
        }
        if !FN_MODES.contains(&self.fn_mode.as_str()) {
            return Err("the function keys mode must be media keys first, F-keys first or automatic".into());
        }
        if self.repeat_delay_ms != 0 && !(100..=2000).contains(&self.repeat_delay_ms) {
            return Err("the key repeat delay must be between 100 and 2000 milliseconds".into());
        }
        if self.repeat_rate != 0 && !(2..=60).contains(&self.repeat_rate) {
            return Err("the key repeat rate must be between 2 and 60 per second".into());
        }
        if self.scroll_speed_pct != 0 && !(SCROLL_SPEED_RANGE.0..=SCROLL_SPEED_RANGE.1).contains(&self.scroll_speed_pct) {
            return Err("the scrolling speed must be between 50 and 300 percent".into());
        }
        for (v, what) in [(self.pointer_speed_pct, "mouse"), (self.touchpad_speed_pct, "touchpad")] {
            if v != 0 && !(POINTER_SPEED_RANGE.0..=POINTER_SPEED_RANGE.1).contains(&v) {
                return Err(format!("the {what} speed must be between 25 and 300 percent"));
            }
        }
        if self.wheel_lines != 0 && !(WHEEL_LINES_RANGE.0..=WHEEL_LINES_RANGE.1).contains(&self.wheel_lines) {
            return Err("a mouse wheel click must move between 1 and 10 lines".into());
        }
        if !TOUCHBAR_CHOICES.contains(&self.touchbar.as_str()) {
            return Err("the Touch Bar must be default, off, keys or strip".into());
        }
        if let Some(bad) = self.touchbar_strips.iter().find(|x| !TOUCHBAR_STRIPS.contains(&x.as_str())) {
            return Err(format!("unknown Touch Bar strip \"{bad}\" (system, recent, ai or app)"));
        }
        if !matches!(self.home_swipe_fingers, 0 | 2 | 3) {
            return Err("the Home swipe uses two or three fingers".into());
        }
        if self.touchbar_flip > 3 {
            return Err("the Touch Bar flip must be 0 to 3".into());
        }
        Ok(())
    }

    /// The `hid-apple` `fnmode` value for the choice (1 = media keys first, 2 = F-keys first, 3 = automatic).
    pub fn fnmode_value(&self) -> Option<u32> {
        match self.fn_mode.as_str() {
            "media" => Some(1),
            "fkeys" => Some(2),
            "auto" => Some(3),
            _ => None,
        }
    }

    /// The `applespi` `fnmode` value for the choice (1 = media keys first, 2 = F-keys first); "auto" is a hid-apple notion and leaves
    /// applespi as it is.
    pub fn applespi_fnmode_value(&self) -> Option<u32> {
        match self.fn_mode.as_str() {
            "media" => Some(1),
            "fkeys" => Some(2),
            _ => None,
        }
    }

    /// Three-finger drag with the default (off) for what was not chosen.
    pub fn three_finger_drag_on(&self) -> bool {
        self.three_finger_drag.unwrap_or(false)
    }

    /// Swiping between the groups of Home, on unless turned off.
    pub fn home_swipe_on(&self) -> bool {
        self.home_swipe.unwrap_or(true)
    }

    /// Fingers of the touchpad swipe (3 unless 2 was chosen).
    pub fn home_swipe_fingers_n(&self) -> u8 {
        if self.home_swipe_fingers == 2 { 2 } else { 3 }
    }

    /// (tap to click, natural scrolling) with the defaults for what was not chosen.
    pub fn trackpad(&self) -> (bool, bool) {
        (self.trackpad_tap.unwrap_or(true), self.trackpad_natural.unwrap_or(true))
    }

    /// The scrolling choices with the defaults for what was not chosen; a value outside its limits (a hand-edited file) is pulled
    /// back inside them.
    pub fn scroll(&self) -> ScrollPrefs {
        let pick = |v: u32, r: (u32, u32), d: u32| if v == 0 { d } else { v.clamp(r.0, r.1) };
        ScrollPrefs {
            speed_pct: pick(self.scroll_speed_pct, SCROLL_SPEED_RANGE, SCROLL_SPEED_DEFAULT),
            wheel_natural: self.wheel_natural.unwrap_or(false),
            pad_natural: self.trackpad_natural.unwrap_or(true),
            smooth: self.scroll_smooth.unwrap_or(true),
            momentum: self.scroll_momentum.unwrap_or(true),
            wheel_lines: pick(self.wheel_lines, WHEEL_LINES_RANGE, WHEEL_LINES_DEFAULT),
        }
    }

    /// (mouse, touchpad) pointer speed in percent with the default for what was not chosen; a hand-edited value outside the limits is
    /// pulled back inside them.
    pub fn pointer_speeds(&self) -> (u32, u32) {
        let pick = |v: u32| if v == 0 { POINTER_SPEED_DEFAULT } else { v.clamp(POINTER_SPEED_RANGE.0, POINTER_SPEED_RANGE.1) };
        (pick(self.pointer_speed_pct), pick(self.touchpad_speed_pct))
    }

    /// The Touch Bar `mode` value for the choice (2 = media and brightness keys, 1 = function keys; automatic leaves it).
    pub fn touchbar_value(&self) -> Option<u32> {
        if self.touchbar == "off" {
            return Some(3);
        }
        match self.fn_mode.as_str() {
            "media" => Some(2),
            "fkeys" => Some(1),
            _ => None,
        }
    }

    /// What the Touch Bar does: off, the firmware keys or IYAKKI's strip. `display_works` is whether the display could be opened; the
    /// default and "strip" fall back to the keys without it.
    pub fn touchbar_mode(&self, display_works: bool) -> TouchBarMode {
        match self.touchbar.as_str() {
            "off" => TouchBarMode::Off,
            "keys" => TouchBarMode::Keys,
            _ if display_works => TouchBarMode::Strip,
            _ => TouchBarMode::Keys,
        }
    }

    /// The strips to cycle through, in order (all when none were chosen).
    pub fn strips(&self) -> Vec<&'static str> {
        TOUCHBAR_STRIPS.into_iter().filter(|s| self.touchbar_strips.is_empty() || self.touchbar_strips.iter().any(|c| c == s)).collect()
    }

    /// (delay in ms, period in ms) when the person chose a repeat; the missing half keeps the kernel's default.
    pub fn repeat(&self) -> Option<(u32, u32)> {
        if self.repeat_delay_ms == 0 && self.repeat_rate == 0 {
            return None;
        }
        let delay = if self.repeat_delay_ms == 0 { 250 } else { self.repeat_delay_ms };
        let rate = if self.repeat_rate == 0 { 30 } else { self.repeat_rate };
        Some((delay, (1000 / rate).max(1)))
    }
}


/// What the kernel has now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleState {
    /// The `hid-apple` driver is loaded (its parameters exist).
    pub driver: bool,
    pub fnmode: Option<u32>,
    pub iso_layout: Option<bool>,
    pub swap_opt_cmd: Option<bool>,
    /// Touch Bar devices with their current mode.
    pub touchbar: Vec<(String, u32)>,
    /// The `applespi` driver (the SPI keyboard of a 2015-2017 MacBook) is loaded.
    pub spi: bool,
    pub spi_fnmode: Option<u32>,
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

pub fn pointer_defaults(profile: crate::inputprofile::Resolved) -> PointerDefaults {
    match profile {
        // two-finger scroll, tap to click, two-finger tap or Control+click is a right click, natural trackpad, classic wheel
        crate::inputprofile::Resolved::Mac => PointerDefaults { trackpad_tap: true, trackpad_natural: true, wheel_natural: false, speed_pct: 100, three_finger_drag: false },
        // as before this profile existed
        crate::inputprofile::Resolved::Pc => PointerDefaults { trackpad_tap: true, trackpad_natural: true, wheel_natural: false, speed_pct: 100, three_finger_drag: false },
    }
}

impl KeyboardSettings {
    /// Put the pointer choices back to the profile's defaults (the keyboard, repeat and Touch Bar choices are left alone).
    pub fn reset_pointer(&mut self, profile: crate::inputprofile::Resolved) {
        let d = pointer_defaults(profile);
        self.trackpad_tap = Some(d.trackpad_tap);
        self.trackpad_natural = Some(d.trackpad_natural);
        self.three_finger_drag = Some(d.three_finger_drag);
        self.wheel_natural = Some(d.wheel_natural);
        self.scroll_speed_pct = 0; // "not chosen" is 100
        self.pointer_speed_pct = 0;
        self.touchpad_speed_pct = 0;
        self.scroll_smooth = None;
        self.scroll_momentum = None;
        self.wheel_lines = 0;
        debug_assert_eq!(d.speed_pct, SCROLL_SPEED_DEFAULT);
    }
}

