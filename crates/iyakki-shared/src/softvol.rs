//! Types moved out of `iyakki_agent::softvol` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The mixer control softvol adds to the card (`amixer -c C sset IYAKKI 30%`; `iyakki-devctl` knows it). Its own name, not
/// `Master`: on a PC the HDMI device shares its card with an analog codec whose `Master Playback Volume` must stay apart,
/// and without a `Playback Volume` suffix it is never mistaken for a hardware control (see [`HW_CONTROLS`]).
pub const CONTROL: &str = "IYAKKI";

pub use iyakki_core_api::soundtypes::MAX_BOOST_DB;
