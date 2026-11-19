//! Types moved out of `iyakki_agent::voiceassist_ports` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the screen asks of the worker.
pub enum Cmd {
    Tap,
    Stop,
    /// A tap or a key on the approval card.
    Approve(String, bool),
    /// A tap or a key on "did you say ...".
    Confirm(bool),
    /// The waker's text for a piece of work that ended: asked of the agent and answered aloud when the assistant is free.
    Wake(String),
    Quit,
}

/// The settings the worker follows (read about four times a second, so a change from the chat tool or Settings acts at once).
#[derive(Clone, Copy, Debug)]
pub struct Live {
    pub muted: bool,
    pub follow_ms: u64,
    pub pause_ms: u64,
    pub spoken_approvals: bool,
}
