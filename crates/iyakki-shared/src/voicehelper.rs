//! Types moved out of `iyakki_agent::voicehelper` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperState {
    /// Listening; the browser has not connected yet.
    Waiting,
    Connected,
    /// Over, and why.
    Closed(String),
}
