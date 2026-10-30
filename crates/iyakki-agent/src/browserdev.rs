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

use std::collections::BTreeSet;
use std::path::Path;

pub const CAM_CHOICE: &str = "iyakki-browser-cam";
pub const CAM_HIDDEN: &str = "iyakki-browser-cam-hidden";
pub const MIC_HIDDEN: &str = "iyakki-browser-mic-hidden";

/// A camera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cam {
    /// The engine's name for it (the V4L2 card name, ends trimmed).
    pub id: String,
    pub label: String,
}

/// The capture cameras of this computer, in the order of their `/dev/video` numbers, each name once. `sys` is the root of `/sys`.
/// A camera has several nodes (a second one carries metadata): only index 0 is a picture node.
pub fn list_cameras(sys: &Path) -> Vec<Cam> {
    let mut nodes: Vec<(u32, String)> = Vec::new();
    for e in std::fs::read_dir(sys.join("class/video4linux")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(n) = name.strip_prefix("video").and_then(|n| n.parse::<u32>().ok()) else { continue };
        let read = |f: &str| std::fs::read_to_string(e.path().join(f)).map(|t| t.trim_end_matches('\n').to_string()).ok();
        if read("index").is_some_and(|i| i.trim() != "0") {
            continue;
        }
        if let Some(card) = read("name").map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
            nodes.push((n, card));
        }
    }
    nodes.sort();
    let mut out: Vec<Cam> = Vec::new();
    for (_, card) in nodes {
        if !out.iter().any(|c| c.id == card) {
            let label = card.trim_end_matches(':').trim().to_string();
            out.push(Cam { id: card, label });
        }
    }
    out
}

/// What the person decided about devices, per person (`browser-devices-<user>.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicePrefs {
    /// The default camera (a [`Cam::id`]); empty: automatic.
    pub cam: String,
    pub hidden_cams: BTreeSet<String>,
    /// Hidden microphones (ALSA ids such as `plughw:1,0`).
    pub hidden_mics: BTreeSet<String>,
}

const MAX_ITEMS: usize = 64;

fn clean_id(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t.len() <= 120 && !t.chars().any(|c| c.is_control())).then(|| t.to_string())
}

impl DevicePrefs {
    pub fn to_json(&self) -> String {
        serde_json::json!({ "cam": self.cam, "hidden_cams": self.hidden_cams, "hidden_mics": self.hidden_mics }).to_string()
    }

    /// Damaged or missing text gives the defaults (nothing hidden, automatic).
    pub fn from_json(text: &str) -> DevicePrefs {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return DevicePrefs::default() };
        let set = |k: &str| -> BTreeSet<String> { v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().and_then(clean_id)).take(MAX_ITEMS).collect() };
        DevicePrefs { cam: v["cam"].as_str().and_then(clean_id).unwrap_or_default(), hidden_cams: set("hidden_cams"), hidden_mics: set("hidden_mics") }
    }

    pub fn load(path: &Path) -> DevicePrefs {
        std::fs::read_to_string(path).map(|t| DevicePrefs::from_json(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, path)
    }

    /// Mark a camera "don't use". The default camera is cleared when it is the one hidden.
    pub fn hide_cam(&mut self, id: &str) {
        if let Some(i) = clean_id(id) {
            if self.hidden_cams.len() < MAX_ITEMS {
                self.hidden_cams.insert(i.clone());
            }
            if self.cam == i {
                self.cam.clear();
            }
        }
    }

    pub fn hide_mic(&mut self, id: &str) {
        if let Some(i) = clean_id(id) {
            if self.hidden_mics.len() < MAX_ITEMS {
                self.hidden_mics.insert(i);
            }
        }
    }

    pub fn show_cam(&mut self, id: &str) {
        self.hidden_cams.remove(id);
    }

    pub fn show_mic(&mut self, id: &str) {
        self.hidden_mics.remove(id);
    }

    /// The cameras a page may be given, and the one to use when the person made no choice for the site.
    pub fn usable_cams<'a>(&self, all: &'a [Cam]) -> Vec<&'a Cam> {
        all.iter().filter(|c| !self.hidden_cams.contains(&c.id)).collect()
    }

    /// The camera to use: `wanted` (a site's remembered camera) when it is plugged in and not hidden, else the default, else the first.
    pub fn pick_cam<'a>(&self, all: &'a [Cam], wanted: &str) -> Option<&'a Cam> {
        let usable = self.usable_cams(all);
        [wanted, self.cam.as_str()].into_iter().filter(|w| !w.is_empty()).find_map(|w| usable.iter().copied().find(|c| c.id == w)).or_else(|| usable.first().copied())
    }

    /// The hidden devices as sentences for Settings: `(kind, id, label)`; the label is the id when the device is not plugged in.
    pub fn hidden_rows(&self, cams: &[Cam], mics: &[crate::voicedesk::Mic]) -> Vec<(&'static str, String, String)> {
        let mut rows = Vec::new();
        for id in &self.hidden_cams {
            rows.push(("camera", id.clone(), cams.iter().find(|c| &c.id == id).map_or_else(|| id.trim().trim_end_matches(':').trim().to_string(), |c| c.label.clone())));
        }
        for id in &self.hidden_mics {
            rows.push(("microphone", id.clone(), mics.iter().find(|m| &m.id == id).map_or_else(|| id.clone(), |m| m.label.clone())));
        }
        rows
    }
}

/// Hand one value to the plugins in the shared folder (`file`); an empty value removes the file (automatic).
pub fn share_value(dir: &Path, file: &str, value: &str) {
    let f = dir.join(file);
    match clean_id(value) {
        None => {
            let _ = std::fs::remove_file(&f);
        }
        Some(v) => write_public(&f, &format!("{v}\n")),
    }
}

/// Hand a list of ids to the plugins, one per line; an empty list removes the file.
pub fn share_list(dir: &Path, file: &str, ids: &BTreeSet<String>) {
    let f = dir.join(file);
    if ids.is_empty() {
        let _ = std::fs::remove_file(&f);
    } else {
        write_public(&f, &ids.iter().map(|i| format!("{i}\n")).collect::<String>());
    }
}

fn write_public(f: &Path, text: &str) {
    let _ = std::fs::write(f, text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o644));
    }
}

/// Everything the plugins need for one request from a site: the hidden lists, and the camera and microphone to use first (a site's
/// remembered devices, else the defaults). Called before the shell answers a request with an allow.
pub fn share_all(dir: &Path, prefs: &DevicePrefs, cam: &str, mic: &str) {
    share_list(dir, CAM_HIDDEN, &prefs.hidden_cams);
    share_list(dir, MIC_HIDDEN, &prefs.hidden_mics);
    share_value(dir, CAM_CHOICE, cam);
    share_value(dir, crate::voicedesk::BROWSER_MIC_CHOICE, mic);
}

#[cfg(test)]
#[path = "browserdev_tests.rs"]
mod tests;

/// Where the kernel's device tables live (a function so tiles ask core instead of naming a kernel path themselves).
pub fn sys_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/sys")
}
