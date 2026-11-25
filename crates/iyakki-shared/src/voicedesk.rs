//! The voice desk's types and rules live in `iyakki-voice-model` (re-exported here); the three functions that reach the capture formats
//! the agent learned, the model list of the drivers and the storage words stay in this file.

pub use iyakki_voice_model::voicedesk::*;

/// The record command aimed at one microphone (see [`record_cmd_for_with`]; the device strings come from [`crate::micpcm`]).
pub fn record_cmd_for(base: &str, mic: Option<&Mic>) -> String {
    record_cmd_for_with(base, mic, &|id| crate::micpcm::pcm_for(id))
}

/// What to tell the person when no microphone was found.
pub fn no_mic_text(product: &str) -> String {
    no_mic_text_t2(product, false)
}

/// [`no_mic_text`], knowing that a T2 sound card's microphones are switched off.
pub fn no_mic_text_t2(product: &str, t2_hidden: bool) -> String {
    no_mic_text_with(product, t2_hidden, iyakki_drivers::cs8409::model_applies(product))
}

/// A storage error in words a person can act on (a function so tiles ask core instead of using the storage crate).
pub fn reword_storage_error(msg: &str) -> String {
    iyakki_storage::loss::reword(msg)
}
