//! Types moved out of `iyakki_agent::avbuild` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The image's copy of the bridge repository's map and recipe.
pub const BRIDGE_DIR: &str = "/usr/share/iyakki/av-addon/bridge";

/// The title of the Store's confirm step for this route (the Store recognises the route by it).
pub const CONFIRM_TITLE: &str = "Browser audio and video: build it here (k3s), read, then confirm";

/// What the person is told when k3s is not there.
pub const NO_K3S_TEXT: &str = "Building here needs k3s, which is not installed or not running. Install and start k3s in Store -> Setup, or use the file route (put ffmpeg-libs-<version>-linux-musl-<arch>.tar.gz in /local/packages) or the mirror route.";
