//! Bluetooth audio as the core serves it (F-ARCH-06): the receiving side (a phone or laptop plays through this computer's speakers) and the sending
//! side (this computer's sound goes to headphones or a speaker). The views are those of `iyakki-devctl`; choosing the device, the card text and the
//! change go through [`BtAudio`]: the device has `iyakki_core_local::LocalBtAudio`, a test has `iyakki_core_fake::FakeBtAudio`.
//!
//! Reads (`views`, the paired sinks, the Sound page line) are `system.read`. Every change (connect, disconnect, open pairing, start or stop
//! sending, the test tone) needs `system.bluetooth.audio`, and the card that always came first still does: [`BtAudio::plan`] gives its text
//! (or the plain reason it cannot be done) before anything is written, and the number to compare while pairing is answered by the person on
//! the screen, never through this trait.

pub use iyakki_devctl::a2dp::{AudioDeviceView, AudioState, AudioView};
pub use iyakki_devctl::a2dp_send::SendView;

/// What can be changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtAudioAction {
    /// Connect a paired phone or laptop so its sound plays here (`device` is a name or an address; empty when only one is paired).
    Connect { device: String },
    /// The same, for a page that cannot wait: it asks the phone to connect and returns at once, the view shows how it went (the address is a paired phone's).
    ConnectAt { address: String },
    Disconnect,
    /// Open pairing for phones for two minutes, or close it.
    Pairing { on: bool },
    /// Send this computer's sound to paired headphones or a speaker.
    SendStart { device: String, whole_computer: bool, mute_speakers: bool },
    SendStop,
    /// Play a two second test tone through the headphones that are being sent to.
    SendTone,
}

impl BtAudioAction {
    /// The chat tool that does this.
    pub fn tool(&self) -> &'static str {
        match self {
            BtAudioAction::Connect { .. } | BtAudioAction::ConnectAt { .. } => "bluetooth_audio_connect",
            BtAudioAction::Disconnect => "bluetooth_audio_disconnect",
            BtAudioAction::Pairing { .. } => "bluetooth_audio_pairing",
            BtAudioAction::SendStart { .. } => "bluetooth_audio_send_start",
            BtAudioAction::SendStop => "bluetooth_audio_send_stop",
            BtAudioAction::SendTone => "bluetooth_audio_send_tone",
        }
    }
}

pub const NOT_HERE: &str = "Bluetooth audio is not available here";

pub trait BtAudio: Send + Sync {
    /// Who plays through this computer, the codec, packets lost, or why nothing can.
    fn receive_view(&self) -> AudioView {
        AudioView { why_not: NOT_HERE.into(), ..Default::default() }
    }
    /// Whether sound goes to headphones or a speaker, to which, and if not, why.
    fn send_view(&self) -> SendView {
        SendView { why: NOT_HERE.into(), ..Default::default() }
    }
    /// The paired headphones and speakers as (address, name), for the Sound page's choice.
    fn sinks(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// One line for the Sound page: where the whole computer's sound goes, or why not.
    fn sound_line(&self) -> String {
        self.send_view().line()
    }
    /// The headphones remembered from the last start, as (address, mute speakers).
    fn remembered(&self) -> Option<(String, bool)> {
        None
    }
    /// The card text of a change, or the plain reason it cannot be done (no such device, nothing connected). Writes nothing.
    fn plan(&self, _action: &BtAudioAction) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
    /// Carry the change out after the card was answered; the answer is the sentence to show.
    fn apply(&self, _action: &BtAudioAction) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
    /// Sign-out: the keys of a connected phone or headset change nothing until somebody signs in again.
    fn clear_remote(&self) {}
}

/// The handle of a machine without Bluetooth audio.
pub struct NoBtAudio;

impl BtAudio for NoBtAudio {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_without_bluetooth_audio_says_so_everywhere_and_changes_nothing() {
        let n = NoBtAudio;
        assert_eq!(n.receive_view().why_not, NOT_HERE);
        assert!(!n.receive_view().ready);
        assert_eq!(n.send_view().why, NOT_HERE);
        assert!(n.sinks().is_empty() && n.remembered().is_none());
        for a in [BtAudioAction::Disconnect, BtAudioAction::SendStop, BtAudioAction::SendTone, BtAudioAction::Pairing { on: true }, BtAudioAction::Connect { device: String::new() }] {
            assert_eq!(n.plan(&a).unwrap_err(), NOT_HERE);
            assert_eq!(n.apply(&a).unwrap_err(), NOT_HERE);
        }
        n.clear_remote();
    }

    #[test]
    fn every_change_names_its_chat_tool() {
        let names: Vec<_> = [BtAudioAction::Disconnect, BtAudioAction::SendStop, BtAudioAction::SendTone, BtAudioAction::Pairing { on: false }, BtAudioAction::Connect { device: "x".into() }, BtAudioAction::SendStart { device: String::new(), whole_computer: true, mute_speakers: true }].iter().map(BtAudioAction::tool).collect();
        assert!(names.iter().all(|n| n.starts_with("bluetooth_audio_")) && names.len() == 6);
    }
}
