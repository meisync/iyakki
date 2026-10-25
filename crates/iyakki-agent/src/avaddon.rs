//! The optional audio and video add-on for the full web engine: H.264 video and AAC audio in the BROWSER tile (TikTok and the like).
//!
//! **Nothing third-party is shipped.** The image carries only GStreamer's `gst-libav` plugin, an LGPL glue library that decodes
//! through FFmpeg's libraries; it sits in [`PLUGIN_DIR`], which GStreamer does not scan, so it is inactive. The FFmpeg libraries
//! (LGPL, shared) are downloaded from their vendor when the person asks (Store -> Setup, the same installer as the radio's audio
//! add-on) into [`lib_dir`]. When all of them are there, [`engine_env`] adds the plugin folder, the library folder and a separate
//! plugin registry to the web engine's environment; when they are not, nothing changes and the engine behaves as it always did.
//!
//! The libraries must carry the sonames `gst-libav` was linked against (a different FFmpeg major version does not load). The image
//! build writes them to [`ABI_FILE`] (`web-post-build.sh`); the add-on's manifest names the download that provides them.
//! H.264 and AAC are patent-licensed formats: decoding them is the person's own choice after their own download
//! (docs/FULL-BROWSER.md, Codec policy).

use std::path::{Path, PathBuf};

/// The inactive GStreamer plugin folder of the image (`libgstlibav.so` only); not on GStreamer's default search path.
pub const PLUGIN_DIR: &str = "/usr/lib/iyakki-av/gstreamer-1.0";
/// The sonames the image's `gst-libav` needs, one per line (`#` starts a comment); written by `web-post-build.sh`.
pub const ABI_FILE: &str = "/usr/share/iyakki/av-addon-abi";
/// A folder to use instead of the default add-on library folder (tests, a person's own build of the libraries).
pub const LIB_DIR_ENV: &str = "IYAKKI_AV_ADDON_DIR";
/// The Store -> Setup item that downloads the libraries. SEAM: this is the id of the radio side's add-on entry (FFmpeg shared
/// libraries, LGPL); change it here if the installer names it differently.
pub const INSTALL_ITEM: &str = "ffmpeg-libs";
/// Sonames of the FFmpeg version Buildroot builds today (6.1), used only when the image has no [`ABI_FILE`] (a development run).
pub const DEFAULT_SONAMES: [&str; 7] = ["libavcodec.so.60", "libavformat.so.60", "libavutil.so.58", "libavfilter.so.9", "libswresample.so.4", "libswscale.so.7", "libpostproc.so.57"];

/// What the person is told above a page whose video needs the add-on (the shell draws it with three buttons).
pub const BAR_NEEDS: &str = "This video needs the audio and video add-on. Install it (your click, downloaded from the vendor, LGPL, licence shown).";
/// The add-on is installed but the engine still cannot play the page's media.
pub const BAR_BROKEN: &str = "The audio and video add-on is installed, but this page's video still cannot be played with it.";
/// This image was built without the bridge (`WEB_AV_ADDON` off).
pub const BAR_UNAVAILABLE: &str = "This video needs H.264 or AAC, which this image does not include.";

/// Where the add-on's libraries live: `IYAKKI_AV_ADDON_DIR`, else `<data>/addons/ffmpeg/lib`.
pub fn lib_dir(data: &Path) -> PathBuf {
    std::env::var_os(LIB_DIR_ENV).map(PathBuf::from).unwrap_or_else(|| data.join("addons").join("ffmpeg").join("lib"))
}

/// The data folder of a browser profile folder (`<data>/web-profile/<user>`).
pub fn data_of_profile(profile: &Path) -> Option<&Path> {
    profile.parent().and_then(Path::parent)
}

/// The sonames listed in an [`ABI_FILE`] text; the default set when it names none.
pub fn required_sonames(abi_text: &str) -> Vec<String> {
    let v: Vec<String> = abi_text
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty() && l.starts_with("lib") && l.contains(".so") && l.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')))
        .map(str::to_string)
        .collect();
    if v.is_empty() {
        DEFAULT_SONAMES.iter().map(|s| s.to_string()).collect()
    } else {
        v
    }
}

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

/// What the add-on looks like on this machine: the plugin folder, the libraries folder and the sonames the plugin needs.
pub fn detect_at(plugin_dir: &Path, lib_dir: &Path, sonames: &[String]) -> Addon {
    if !plugin_dir.join("libgstlibav.so").is_file() {
        return Addon::NotInImage;
    }
    let missing: Vec<String> = sonames.iter().filter(|s| !lib_dir.join(s).is_file()).cloned().collect();
    if missing.is_empty() {
        Addon::Ready
    } else if missing.len() == sonames.len() {
        Addon::Absent
    } else {
        Addon::Incomplete(missing)
    }
}

/// [`detect_at`] with this image's places.
pub fn detect(data: &Path) -> Addon {
    let abi = std::fs::read_to_string(ABI_FILE).unwrap_or_default();
    detect_at(Path::new(PLUGIN_DIR), &lib_dir(data), &required_sonames(&abi))
}

/// A short name for the libraries folder's contents (file names and sizes), so a changed add-on gets a new plugin registry.
fn fingerprint(lib_dir: &Path) -> String {
    let mut items: Vec<(String, u64)> = std::fs::read_dir(lib_dir)
        .map(|d| d.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?.len()))).collect())
        .unwrap_or_default();
    items.sort();
    fnv(&format!("{items:?}"))
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

/// The environment variables to add to (or replace in) the web engine's when the add-on is ready; empty otherwise, so without the
/// add-on the engine's environment is exactly what it was. `machine` tells machines apart (the host name): a data stick that moves
/// from one computer to another gets a registry of its own on each, because GStreamer's registry caches what a machine can load.
pub fn engine_env(addon: &Addon, plugin_dir: &Path, lib_dir: &Path, profile: &Path, machine: &str) -> Vec<(String, String)> {
    if *addon != Addon::Ready {
        return Vec::new();
    }
    let registry = profile.join("cache").join(format!("gst-registry-av-{}.bin", fnv(&format!("{machine}{}", fingerprint(lib_dir)))));
    vec![
        // the inactive plugin folder, scanned only now
        ("GST_PLUGIN_PATH_1_0".into(), plugin_dir.to_string_lossy().into_owned()),
        // where the plugin finds libavcodec and the rest (a folder holding only the add-on's libraries)
        ("LD_LIBRARY_PATH".into(), lib_dir.to_string_lossy().into_owned()),
        // a registry of its own, so the plugin list without the add-on is never mixed up with the one with it
        ("GST_REGISTRY_1_0".into(), registry.to_string_lossy().into_owned()),
        // the add-on's decoders win over any other H.264 / AAC decoder (default ranks are 256)
        ("GST_PLUGIN_FEATURE_RANK".into(), "avdec_h264:300,avdec_aac:300,avdec_mp3:300".into()),
    ]
}

/// Put `extra` into `env`, replacing a variable of the same name.
pub fn merge_env(env: &mut Vec<(String, String)>, extra: Vec<(String, String)>) {
    for (k, v) in extra {
        env.retain(|(a, _)| *a != k);
        env.push((k, v));
    }
}

/// The host name, for [`engine_env`].
pub fn machine_name() -> String {
    std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default()
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

/// The text of the bar for this add-on state (when the add-on is ready the page's trouble is another one: [`BAR_BROKEN`]).
pub fn bar_text(addon: &Addon) -> &'static str {
    match addon {
        Addon::NotInImage => BAR_UNAVAILABLE,
        Addon::Absent | Addon::Incomplete(_) => BAR_NEEDS,
        Addon::Ready => BAR_BROKEN,
    }
}

/// The page-side script of the media probe (`dist/webview/wpe-helper/media-probe.js`, built into the helper).
pub const PROBE_JS: &str = include_str!("../../../dist/webview/wpe-helper/media-probe.js");
