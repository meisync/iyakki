//! Moved from `iyakki_shared::voiceassist_ports` into the voice leaf crate (alpha.11 AP1); the old path re-exports it.



/// What the screen asks of the worker.
#[derive(serde::Serialize, serde::Deserialize)]
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
