//! Sound hardware as the tiles need it (UI/OS split step 6, the Audio slice, F-ARCH-06): which cards there are and the playback volume of a card.
//! Types and the trait only; `iyakki-core-local` reads `/proc/asound` and runs the mixer, `iyakki-core-fake` answers from canned data.

use crate::error::ApiResult;
use serde::{Deserialize, Serialize};

/// A sound card from `/proc/asound/cards`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundCard {
    pub index: u32,
    pub name: String,
    pub long_name: String,
}

/// The playback volume of a card: which mixer control carries it, the level in percent and whether it is muted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackVolume {
    pub control: String,
    pub percent: u32,
    pub muted: bool,
}

pub trait Audio: Send + Sync {
    fn sound_cards(&self) -> ApiResult<Vec<SoundCard>>;
    /// The playback volume of `card`; `Unsupported` when the card has no volume control.
    fn volume(&self, card: u32) -> ApiResult<PlaybackVolume>;
    /// Set the level (0 to 100) and give back what the mixer says afterwards.
    fn set_volume(&self, card: u32, percent: u32) -> ApiResult<PlaybackVolume>;
    /// Move the level by `delta` percent points, or toggle the mute when `delta` is `None`.
    fn change_volume(&self, card: u32, delta: Option<i32>) -> ApiResult<PlaybackVolume>;
}
