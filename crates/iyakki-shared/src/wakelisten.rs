//! The wake phrase listener's plain ports (moved out of `iyakki_agent::wakelisten`, F-SPLIT-03).

pub const SENSITIVITY: [&str; 3] = ["low", "normal", "high"];

/// The microphone as the listener sees it: open, read what arrived since the last read, close.
pub trait Mic: Send {
    fn open(&mut self) -> Result<(), String>;
    fn is_open(&self) -> bool;
    /// 16 kHz mono samples since the last call (nothing is kept here).
    fn drain(&mut self) -> Vec<i16>;
    /// The capture is still running.
    fn alive(&mut self) -> bool;
    /// Close the microphone and drop anything unread.
    fn close(&mut self);
}
