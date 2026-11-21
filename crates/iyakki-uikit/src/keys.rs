//! Plain key types that travel inside [`crate::ui::Event`]: the Fn keys of a laptop and the Ctrl+Alt layout chords. Data only; the
//! shell decides what they do (`fnkeys.rs`, `panes.rs` re-export them under their old paths).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FnKey {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
    BrightnessAuto,
    KbdLightUp,
    KbdLightDown,
    KbdLightToggle,
    PlayPause,
    Next,
    Prev,
    Stop,
    Sleep,
    Power,
    RfKill,
    Wlan,
    Bluetooth,
    SwitchVideo,
    Print,
    Calc,
    Homepage,
    /// KEY_SCREENLOCK, and Super+L on a PC keyboard: lock the screen.
    Lock,
    /// Control+Command+S (Mac profile) or Control+Windows+S: start the screen saver now (until a key or the pointer wakes it).
    Saver,
    /// Control+Command+Q (Mac profile) or Control+Windows+Q: ask whether to sign out (a card; the person confirms).
    SignOut,
    /// Super+Alt+S: the screen reader on or off. Works at the sign-in and the lock screens too (`access.rs`).
    ScreenReader,
    /// Super+Alt+Right / Left: the keyboard walk moves to the next / previous control of the screen.
    AccessNext,
    AccessPrev,
    /// Super+Alt+Enter: press the control the keyboard walk stands on.
    AccessActivate,
    /// Super+Alt+R: say the last thing again.
    AccessRepeat,
    /// The Gemini voice assistant's start key (Super+V by default; EXPERIMENTAL): start a session, or end the open one.
    VoiceAssist,
    /// KEY_FN: the bare Fn key of a keyboard that reports it as an event of its own. Held, it cycles the Touch Bar strips (F-KBD-22).
    Function,
}

impl FnKey {
    /// Keys that go on changing while held (volume, brightness, keyboard light).
    pub fn repeats(self) -> bool {
        matches!(self, FnKey::VolumeUp | FnKey::VolumeDown | FnKey::BrightnessUp | FnKey::BrightnessDown | FnKey::KbdLightUp | FnKey::KbdLightDown)
    }
}

/// A side of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// What a Ctrl+Alt chord asks for (see `docs/MULTI-DISPLAY.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKey {
    /// Ctrl+Alt+1 to 9: apply template N.
    Apply(u8),
    /// Ctrl+Alt+arrow: snap the focused app to that side.
    Snap(Side),
    /// Ctrl+Alt+Shift+arrow: move the divider next to the focused pane.
    Nudge(Side),
    /// Ctrl+Alt+Tab (Shift: back): focus the next pane.
    Cycle { back: bool },
    /// Ctrl+Alt+M: move the focused app to the next display.
    NextDisplay,
    /// Ctrl+Alt+0: open the layout picker.
    Picker,
    /// Ctrl+Alt+Super+1 to 9: jump to desktop view N.
    View(u8),
    /// Previous or next desktop view. `mac` is the Mac-style key (Control+Command+Left/Right); otherwise the browser-style
    /// key (Ctrl+Alt+PageUp/PageDown). Which sets are on is the person's choice in Settings.
    ViewStep { back: bool, mac: bool },
    /// The previous or the next view on the person's own step keys (Ctrl+Alt plus a key of their choice, Settings -> Desktop views).
    ViewStepCustom { back: bool },
}
