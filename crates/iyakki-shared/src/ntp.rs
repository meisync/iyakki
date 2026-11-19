//! Types moved out of `iyakki_agent::ntp` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the last attempt did, for Settings -> Time and the clock's tooltip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Unix time of the last answer from a server (0 = never).
    pub last_sync: i64,
    pub server: String,
    /// Our clock's error at that time, seconds (positive = we were behind).
    pub offset: f64,
    /// A plain sentence for the person.
    pub message: String,
    /// The clock is this far ahead of the network time and was left alone until the person agrees (seconds, positive).
    pub pending_back: Option<f64>,
}

/// A time server name or address the settings accept (the agent's `ntp::valid_server` is the same rule).
pub fn valid_server(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}
