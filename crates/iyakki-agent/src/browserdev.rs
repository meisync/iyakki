//! Cameras and microphones as the web pages in the Browser see them: which camera is the default, which devices the person marked
//! "don't use" (a defective microphone, a camera with a cover on), and the files that hand this to the engine's two GStreamer plugins
//! (`dist/webview/wpe-helper/camshare.c`, `micshare.c`).
//!
//! * A **hidden** device is not offered to any page (`enumerateDevices` never lists it) and is never picked as the default. The person
//!   hides one with Delete in the chooser of the camera or microphone question and brings it back in Settings -> Browser.
//! * The **default camera** is the one the person chose in the camera question (empty: automatic, the first one found). The default
//!   microphone is the voice settings' microphone (the Voice tile shows the same one), see [`crate::voicedesk`].
//! * A camera is named the way the engine names it: the Video4Linux card name (`/sys/class/video4linux/video*/name`), for example
//!   `FaceTime HD Camera (Built-in):`, with spaces at the ends removed on both sides (the plugin trims the engine's name before it
//!   compares). Two identical cameras share a name; they are listed once.
//! * The shared files sit in the folder both sides use ([`crate::voicedesk::mic_share_dir`]); the plugins read them when a page asks and
//!   when the device list changes. Nothing here opens a device.


pub use iyakki_shared::browserdev::*;

#[cfg(test)]
#[path = "browserdev_tests.rs"]
mod tests;

/// Where the kernel's device tables live (a function so tiles ask core instead of naming a kernel path themselves).
pub fn sys_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/sys")
}

#[cfg(test)]
use std::path::Path;
