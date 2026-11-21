//! The device side of My IYAKKI's sound stream in cloud mode (F-VOICE-38, F-VOICE-61). One small crate for what both the agent and the
//! connector program need: the frame format and its jitter buffer ([`audiostream`]), the pinned TLS link ([`audiotls`]), the consent gate
//! ([`gate`]), the queue that turns microphone sound into frames ([`uplink`]), the microphone loop ([`device`]) and the certificate offer
//! that travels inside the authenticated session ([`certoffer`]). Nothing here knows an account, a setting or a file: the microphone and the
//! link are traits, the gate is a plain value.

pub mod audiostream;
pub mod audiotls;
pub mod certoffer;
pub mod device;
pub mod gate;
pub mod uplink;

/// The microphone as the stream and the wake listener see it: open, read what arrived since the last read, close.
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
