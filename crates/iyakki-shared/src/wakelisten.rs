//! The wake phrase listener's plain ports (moved out of `iyakki_agent::wakelisten`, F-SPLIT-03).

pub const SENSITIVITY: [&str; 3] = ["low", "normal", "high"];

/// The microphone as the listener sees it: open, read what arrived since the last read, close. One trait for the wake listener, the
/// stream to a cloud machine and the connector program (`iyakki-audiolink`).
pub use iyakki_audiolink::Mic;
