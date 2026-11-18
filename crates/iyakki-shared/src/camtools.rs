//! Types moved out of `iyakki_agent::camtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::{Path, PathBuf};

pub const ASK_NOTE: &str = "The camera is private: the assistant asks every time; Allow for 5 minutes is the longest it may keep using it.";

/// A country's passport or visa photo rules, as the backend's table has them.
#[derive(Clone, Debug, PartialEq)]
pub struct CountryInfo {
    pub id: String,
    pub name: String,
    pub verified: bool,
}

/// What a passport photo produced.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PassportOut {
    pub photo: PathBuf,
    pub sheet_png: Option<PathBuf>,
    pub sheet_pdf: Option<PathBuf>,
    /// A file made for online forms (pixel and file-size limits met), when the country has such rules.
    pub digital: Option<PathBuf>,
    /// The rules used, in words (size, head, background, source, checked date or "unverified").
    pub spec: String,
}

/// The shell's camera, as the tools see it.
pub trait CameraBackend: Send + Sync {
    /// "video0: HD 4MP WEBCAM" per camera.
    fn cameras(&self) -> Vec<String>;
    /// Take a photo into `dir`; the file written.
    fn photo(&self, dir: &Path) -> Result<PathBuf, String>;
    /// Take a photo with the camera whose listed name (`cameras()`) contains `device` (any case); empty: the usual one. A backend with
    /// several cameras overrides this; one with a single camera has nothing to choose (F-FLOW-29).
    fn photo_from(&self, dir: &Path, device: &str) -> Result<PathBuf, String> {
        let _ = device;
        self.photo(dir)
    }
    /// Start recording into `dir`; it stops by itself after `max_secs`. The file being written.
    fn record_start(&self, dir: &Path, max_secs: u32) -> Result<PathBuf, String>;
    /// Stop the recording: the files and the number of frames.
    fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String>;
    /// The recording going on: its file and how long it has run.
    fn recording(&self) -> Option<(PathBuf, f64)>;
    /// One settled frame as JPEG bytes, kept in memory only (nothing is written to disk). The LAB tile uses it; a backend that
    /// cannot do this says so.
    fn photo_bytes(&self) -> Result<Vec<u8>, String> {
        Err("this camera cannot give a picture without saving it".into())
    }
    /// A passport photo for `country` into `dir`; `sheet` is "4x6", "a4" or "none".
    fn passport(&self, dir: &Path, country: &str, sheet: &str) -> Result<PassportOut, String>;
    fn countries(&self) -> Vec<CountryInfo>;
}

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// The assistant's camera tools: list the cameras, take a photo, record a video (with a time limit), take a passport or
// visa photo for a country (with a print sheet and a file for online forms), and list the latest shots.
//
// The camera itself lives in the shell (the CAMERA app's V4L2 code); the shell hands a [`CameraBackend`] to
// [`CameraHub::set_backend`] when it starts, the same way it answers `screen_capture`. The tools exist only while a
// backend is there, for accounts that have the CAMERA app and may write files.
//
// A camera is private: every picture or recording **asks the person first** (`Risk::Ask`), with "Allow for 5 minutes"
// as the longest grant. While the assistant uses the camera, [`CameraHub::in_use`] says so and the shell shows a red
// "Camera in use by the assistant" sign on every screen. Pictures are saved in the person's own Pictures and Videos
// folders and nothing is sent anywhere: the tools return file paths (the chat shows a photo from its file), never the
// image data. External coding agents are not offered the tools that take pictures. Every use is an audit entry
// (`camera.agent.*`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;

pub const GRANT_SECS: u64 = 300;
pub const MAX_RECORD_SECS: u32 = 600;
pub const DEFAULT_RECORD_SECS: u32 = 30;



#[derive(Default)]
pub struct CameraHub {
    backend: Mutex<Option<Arc<dyn CameraBackend>>>,
    grants: Mutex<HashMap<String, Instant>>,
    busy: Mutex<Option<(String, String)>>,
    /// Where homes are (`/home`; tests use a temporary folder).
    home_root: Mutex<Option<PathBuf>>,
}

/// Marks the camera as used by the assistant while it lives.
pub struct Busy<'a>(&'a CameraHub);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        *self.0.busy.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl CameraHub {
    pub fn new() -> CameraHub {
        CameraHub::default()
    }

    pub fn set_backend(&self, b: Option<Arc<dyn CameraBackend>>) {
        *self.backend.lock().unwrap_or_else(|e| e.into_inner()) = b;
    }

    pub fn backend(&self) -> Option<Arc<dyn CameraBackend>> {
        self.backend.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn present(&self) -> bool {
        self.backend().is_some()
    }

    pub fn set_home_root(&self, p: &Path) {
        *self.home_root.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.to_path_buf());
    }

    pub fn home(&self, user: &str) -> PathBuf {
        self.home_root.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| PathBuf::from("/home")).join(user)
    }

    pub fn grant(&self, user: &str, secs: u64) {
        self.grants.lock().unwrap_or_else(|e| e.into_inner()).insert(user.to_string(), Instant::now() + Duration::from_secs(secs));
    }

    pub fn granted(&self, user: &str) -> bool {
        let mut g = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(user) {
            Some(t) if *t > Instant::now() => true,
            Some(_) => {
                g.remove(user);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, user: &str) {
        self.grants.lock().unwrap_or_else(|e| e.into_inner()).remove(user);
    }

    /// Mark the camera as in use by `user` for `what` until the guard is dropped (the shell shows the red sign meanwhile).
    pub fn begin(&self, user: &str, what: &str) -> Busy<'_> {
        *self.busy.lock().unwrap_or_else(|e| e.into_inner()) = Some((user.to_string(), what.to_string()));
        Busy(self)
    }

    /// What the assistant is doing with the camera right now (`None`: nothing). The shell shows it on every screen.
    pub fn in_use(&self) -> Option<String> {
        if let Some((_, what)) = self.busy.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Some(what);
        }
        self.backend().and_then(|b| b.recording()).map(|(_, secs)| format!("recording video ({}s)", secs as u64))
    }
}

pub fn is_camera_tool(name: &str) -> bool {
    matches!(name, "camera_list" | "camera_photo" | "camera_record_start" | "camera_record_stop" | "camera_passport" | "camera_recent")
}

/// The tools that take pictures: asked every time unless a 5-minute grant is running.
pub fn takes_pictures(name: &str) -> bool {
    matches!(name, "camera_photo" | "camera_record_start" | "camera_passport")
}



pub fn describe(name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    Some(match name {
        "camera_photo" => "Let the assistant take a photo with your camera? It is saved in your Pictures folder.".into(),
        "camera_record_start" => format!("Let the assistant record video with your camera for up to {} seconds? It is saved in your Videos folder.", record_secs(args)),
        "camera_passport" => format!("Let the assistant take a {} passport photo with your camera{}?", s("country").unwrap_or("?"), match s("sheet") {
            Some("none") | None => String::new(),
            Some(p) => format!(" and make a {p} print sheet"),
        }),
        "camera_record_stop" => "stop the camera recording".into(),
        "camera_list" => "list the cameras".into(),
        "camera_recent" => "list the latest camera pictures".into(),
        _ => return None,
    })
}

pub fn record_secs(args: &Value) -> u32 {
    args.get("max_seconds").and_then(Value::as_u64).map_or(DEFAULT_RECORD_SECS, |v| (v as u32).clamp(1, MAX_RECORD_SECS))
}


pub fn kb(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len()).div_ceil(1024)
}
