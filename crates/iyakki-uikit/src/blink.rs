//! The blinking caret and its permission. Moved out of the shell's `apps` module so a tile crate can draw a caret.

/// The blinking caret: on for half a second, off for half, and never while the pane being drawn does not own the keyboard or
/// the window has lost the system focus (see [`with_caret`]). Every text input and the editor take their caret from here.
pub fn blink(tick: u64) -> bool {
    crate::ui::input_focus_allowed() && (tick / 15) % 2 == 0
}

/// Whether anything that marks "typing goes here" (caret, current-line wash, bracket box) may be drawn now.
pub fn caret_allowed() -> bool {
    crate::ui::input_focus_allowed()
}

/// Draw with the caret allowed (`on`) or hidden: the AI IDE draws the editor and the chat side by side and only the pane
/// that has the keyboard may show a caret, so a blinking caret always means "typing goes here".
pub fn with_caret<T>(on: bool, f: impl FnOnce() -> T) -> T {
    crate::ui::with_input_focus(on, f)
}

