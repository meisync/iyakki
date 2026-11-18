//! Types moved out of `iyakki_agent::avaddon` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.


use std::path::Path;

/// The Store -> Setup item that downloads the libraries. SEAM: this is the id of the radio side's add-on entry (FFmpeg shared
/// libraries, LGPL); change it here if the installer names it differently.
pub const INSTALL_ITEM: &str = "ffmpeg-libs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Addon {
    /// This image has no bridge plugin: nothing to install can help.
    NotInImage,
    /// The bridge is there; the person has not installed the libraries.
    Absent,
    /// Some of the libraries are there (these are missing).
    Incomplete(Vec<String>),
    /// All of them are there: the engine is given the decoders.
    Ready,
}

/// What a page was missing: the helper's `MediaUnsupported` word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Video,
    Audio,
    Both,
}

impl Need {
    pub fn parse(word: &str) -> Option<Need> {
        match word.trim() {
            "video" => Some(Need::Video),
            "audio" => Some(Need::Audio),
            "both" => Some(Need::Both),
            _ => None,
        }
    }
}

/// What the person is told above a page whose video needs the add-on (the shell draws it with three buttons).
pub const BAR_NEEDS: &str = "This video needs the audio and video add-on. IYAKKI does not provide it: an administrator installs it from a file in /local/packages, a mirror, or an installed ffmpeg (licence shown first).";
/// The add-on is installed but the engine still cannot play the page's media.
pub const BAR_BROKEN: &str = "The audio and video add-on is installed, but this page's video still cannot be played with it.";
/// This image was built without the bridge (`WEB_AV_ADDON` off).
pub const BAR_UNAVAILABLE: &str = "This video needs H.264 or AAC, which this image does not include. IYAKKI does not provide the codecs: an administrator can put the add-on file in /local/packages or set a mirror (Store -> Setup -> Browser audio and video).";

/// The data folder of a browser profile folder (`<data>/web-profile/<user>`).
pub fn data_of_profile(profile: &Path) -> Option<&Path> {
    profile.parent().and_then(Path::parent)
}

/// The text of the bar for this add-on state (when the add-on is ready the page's trouble is another one: [`BAR_BROKEN`]).
pub fn bar_text(addon: &Addon) -> &'static str {
    match addon {
        Addon::NotInImage => BAR_UNAVAILABLE,
        Addon::Absent | Addon::Incomplete(_) => BAR_NEEDS,
        Addon::Ready => BAR_BROKEN,
    }
}

/// What Settings -> Browser and the chat say about the add-on: a short state and what to do about it. The installing itself (the review,
/// the licence and patent confirmation, the build) is in Store -> Setup -> Browser audio and video and asks an administrator.
pub fn settings_text(addon: &Addon) -> (String, String) {
    match addon {
        Addon::NotInImage => ("Audio and video add-on: not part of this image".into(), "This image has no bridge for H.264 and AAC, so pages that need them cannot play sound or pictures of that kind. Everything else plays.".into()),
        Addon::Absent => ("Audio and video add-on: not installed".into(), "Pages with H.264 video or AAC sound need it. An administrator installs it in Store, Setup, Browser audio and video, after reading what is inside and its licence. IYAKKI does not ship or host these libraries.".into()),
        Addon::Incomplete(missing) => (format!("Audio and video add-on: incomplete ({} of its libraries are missing)", missing.len()), "It does not work until it is installed again in Store, Setup, Browser audio and video. Missing: ".to_string() + &missing.iter().take(6).cloned().collect::<Vec<_>>().join(", ") + "."),
        Addon::Ready => ("Audio and video add-on: installed".into(), "The full browser plays H.264 video and AAC or MP3 sound. An administrator can take it away in Store, Setup, Browser audio and video.".into()),
    }
}
