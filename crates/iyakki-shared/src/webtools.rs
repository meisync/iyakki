//! Types moved out of `iyakki_agent::webtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// How much of a page the browser keeps for the assistant.
pub const SNAPSHOT_CHARS: usize = 40_000;

/// What the BROWSER app shows right now, as text (kept per person, dropped at sign-out).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageSnapshot {
    pub url: String,
    pub title: String,
    /// The readable text of the page (up to [`SNAPSHOT_CHARS`]); empty for a page shown by the full engine.
    pub text: String,
    /// What the person has selected on the page, if anything.
    pub selection: String,
    /// When it was taken (seconds since the epoch).
    pub time: i64,
    /// The page is drawn by the embedded full engine, whose text this system cannot read.
    pub full_engine: bool,
}
