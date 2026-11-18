//! One input model for every target: pointer (with an id, so several fingers), wheel, keys, text, the actions of hit regions, resize and
//! the heartbeat. A mobile view is the same events with touch pointers; [`crate::gesture`] turns raw pointers into taps, long presses,
//! drags and swipes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    /// Which finger or device (0 for a mouse).
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub phase: Phase,
    /// Milliseconds on any steady clock; only differences are used.
    pub t_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Tap { x: f32, y: f32 },
    LongPress { x: f32, y: f32 },
    DragStart { x: f32, y: f32 },
    /// The distance moved since the drag began.
    Drag { dx: f32, dy: f32 },
    DragEnd { dx: f32, dy: f32 },
    Swipe(Dir),
    /// Two fingers: the distance between them now over the distance when the second one came down (1.0 = unchanged), and the place
    /// half way between them.
    Pinch { scale: f32, x: f32, y: f32 },
    /// A finger of the pinch lifted; the last scale.
    PinchEnd { scale: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKey {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Esc,
    Tab,
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Pointer(Pointer),
    Gesture(Gesture),
    /// Positive = towards older content.
    Wheel(i32),
    Key(UiKey),
    /// Text for the focused field (typed, pasted or composed by an input method).
    Text(String),
    /// A hit region was used (a click, a tap, Enter on a focused control).
    Action(u32),
    /// A drag that began on one hit region ended on another: the action of the region it was picked up from and the action of the region
    /// it was let go over (drag and drop; made by the host from the drag gestures and the last frame's hit regions, never sent by a client).
    Drop { from: u32, to: u32 },
    /// The pane changed size, screen or view: ask `cx` again.
    Resized,
    /// About 30 times a second.
    Tick,
}
