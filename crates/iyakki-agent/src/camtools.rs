//! The assistant's camera tools: list the cameras, take a photo, record a video (with a time limit), take a passport or
//! visa photo for a country (with a print sheet and a file for online forms), and list the latest shots.
//!
//! The camera itself lives in the shell (the CAMERA app's V4L2 code); the shell hands a [`CameraBackend`] to
//! [`CameraHub::set_backend`] when it starts, the same way it answers `screen_capture`. The tools exist only while a
//! backend is there, for accounts that have the CAMERA app and may write files.
//!
//! A camera is private: every picture or recording **asks the person first** (`Risk::Ask`), with "Allow for 5 minutes"
//! as the longest grant. While the assistant uses the camera, [`CameraHub::in_use`] says so and the shell shows a red
//! "Camera in use by the assistant" sign on every screen. Pictures are saved in the person's own Pictures and Videos
//! folders and nothing is sent anywhere: the tools return file paths (the chat shows a photo from its file), never the
//! image data. External coding agents are not offered the tools that take pictures. Every use is an audit entry
//! (`camera.agent.*`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;

pub const GRANT_SECS: u64 = 300;
pub const MAX_RECORD_SECS: u32 = 600;
pub const DEFAULT_RECORD_SECS: u32 = 30;
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
    /// Start recording into `dir`; it stops by itself after `max_secs`. The file being written.
    fn record_start(&self, dir: &Path, max_secs: u32) -> Result<PathBuf, String>;
    /// Stop the recording: the files and the number of frames.
    fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String>;
    /// The recording going on: its file and how long it has run.
    fn recording(&self) -> Option<(PathBuf, f64)>;
    /// A passport photo for `country` into `dir`; `sheet` is "4x6", "a4" or "none".
    fn passport(&self, dir: &Path, country: &str, sheet: &str) -> Result<PassportOut, String>;
    fn countries(&self) -> Vec<CountryInfo>;
}

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

    fn home(&self, user: &str) -> PathBuf {
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

    fn begin(&self, user: &str, what: &str) -> Busy<'_> {
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

pub fn offered(svc: &Services, p: &Policy) -> bool {
    p.apps.contains(&App::Camera) && !p.fs_write.is_empty() && svc.camera.present()
}

pub fn defs(svc: &Services, p: &Policy) -> Vec<ToolDef> {
    if !offered(svc, p) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let tool = |name: &str, desc: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false };
    let countries: Vec<String> = svc.camera.backend().map(|b| b.countries().into_iter().map(|c| c.id).collect()).unwrap_or_default();
    vec![
        tool("camera_list", "List the USB cameras connected to this computer.", obj(json!({}), &[]), Risk::Safe),
        tool("camera_photo", "Take a photo with the USB camera and save it as a JPEG in the person's Pictures folder; the chat shows it. Asks the person first (the camera is private). The photo stays on this computer.", obj(json!({}), &[]), Risk::Ask),
        tool("camera_record_start", "Start recording a video (picture only, no sound) into the person's Videos folder as an MJPEG AVI. It stops by itself after `max_seconds` (default 30, at most 600); camera_record_stop stops it sooner. Asks the person first.", obj(json!({"max_seconds": {"type": "integer"}}), &[]), Risk::Ask),
        tool("camera_record_stop", "Stop the video recording the assistant started; returns the file.", obj(json!({}), &[]), Risk::Safe),
        tool(
            "camera_passport",
            "Take a passport or visa photo for a country: the picture is cropped to that country's size at 300 dpi, a print sheet with several copies is made (`sheet`: 4x6, a4 or none) and, where the country has rules for online forms, a file within its pixel and file-size limits. Returns the files and the rules used (with their official source; rules that could not be confirmed are marked unverified). The person should stand before a plain light wall, facing the camera. Asks the person first.",
            obj(json!({"country": {"type": "string", "enum": countries}, "sheet": {"type": "string", "enum": ["4x6", "a4", "none"]}}), &["country"]),
            Risk::Ask,
        ),
        tool("camera_recent", "List the newest photos, passport photos and videos taken with the camera (file names, sizes, times; no image data). `limit` default 5.", obj(json!({"limit": {"type": "integer"}}), &[]), Risk::Safe),
    ]
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

fn record_secs(args: &Value) -> u32 {
    args.get("max_seconds").and_then(Value::as_u64).map_or(DEFAULT_RECORD_SECS, |v| (v as u32).clamp(1, MAX_RECORD_SECS))
}

fn folder(svc: &Services, p: &Policy, user: &str, name: &str) -> Result<PathBuf, String> {
    let d = svc.camera.home(user).join(name);
    if !p.allows_write(user, &d.display().to_string()) {
        return Err(format!("your account may not save in {}", d.display()));
    }
    std::fs::create_dir_all(&d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
    Ok(d)
}

fn kb(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len()).div_ceil(1024)
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, user: &str) -> Result<String, String> {
    if !p.apps.contains(&App::Camera) {
        return Err("your account may not use the camera".into());
    }
    let b = svc.camera.backend().ok_or("there is no camera service here (the IYAKKI shell is not running)")?;
    let audit = |what: &str, detail: &str| svc.browser.audit(user, what, detail);
    match name {
        "camera_list" => {
            let c = b.cameras();
            Ok(if c.is_empty() { "No camera is connected. A USB camera can be plugged in at any time.".into() } else { format!("{} camera(s):\n{}", c.len(), c.join("\n")) })
        }
        "camera_photo" => {
            let dir = folder(svc, p, user, "Pictures")?;
            let _busy = svc.camera.begin(user, "taking a photo");
            let path = b.photo(&dir)?;
            audit("camera.agent.photo", &path.display().to_string());
            Ok(format!("Took a photo: {} ({} KB). It is only on this computer.\nIMAGE: {}", path.display(), kb(&path), path.display()))
        }
        "camera_record_start" => {
            if let Some((f, _)) = b.recording() {
                return Err(format!("already recording into {}; use camera_record_stop", f.display()));
            }
            let secs = record_secs(args);
            let dir = folder(svc, p, user, "Videos")?;
            let path = b.record_start(&dir, secs)?;
            audit("camera.agent.record", &format!("{} ({secs}s max)", path.display()));
            Ok(format!("Recording to {} (picture only, no sound). It stops by itself after {secs} seconds; camera_record_stop stops it sooner. A red sign shows the camera is in use.", path.display()))
        }
        "camera_record_stop" => {
            if b.recording().is_none() {
                return Err("nothing is being recorded".into());
            }
            let (files, frames) = b.record_stop()?;
            let list: Vec<String> = files.iter().map(|f| format!("{} ({} KB)", f.display(), kb(f))).collect();
            audit("camera.agent.record.stop", &list.join(", "));
            Ok(format!("Recording stopped: {frames} frames in {}.", list.join(", ")))
        }
        "camera_passport" => {
            let country = args.get("country").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
            let sheet = args.get("sheet").and_then(Value::as_str).unwrap_or("4x6").trim().to_lowercase();
            if !matches!(sheet.as_str(), "4x6" | "a4" | "none") {
                return Err("sheet must be 4x6, a4 or none".into());
            }
            let known = b.countries();
            if !known.iter().any(|c| c.id == country) {
                return Err(format!("unknown country “{country}”; choose one of: {}", known.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(", ")));
            }
            let dir = folder(svc, p, user, "Pictures")?;
            let _busy = svc.camera.begin(user, "taking a passport photo");
            let out = b.passport(&dir, &country, &sheet)?;
            audit("camera.agent.passport", &format!("{country}: {}", out.photo.display()));
            let mut lines = vec![format!("Passport photo: {} ({} KB)", out.photo.display(), kb(&out.photo))];
            for (label, f) in [("Print sheet (PNG)", &out.sheet_png), ("Print sheet (PDF, print at 100%)", &out.sheet_pdf), ("For online forms", &out.digital)] {
                if let Some(f) = f {
                    lines.push(format!("{label}: {} ({} KB)", f.display(), kb(f)));
                }
            }
            lines.push(format!("Rules used: {}", out.spec));
            lines.push("Check the photo against the rules before using it; nothing was sent anywhere.".into());
            lines.push(format!("IMAGE: {}", out.photo.display()));
            Ok(lines.join("\n"))
        }
        "camera_recent" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(5).clamp(1, 50) as usize;
            let home = svc.camera.home(user);
            let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
            for sub in ["Pictures", "Videos"] {
                let d = home.join(sub);
                if !p.allows_read(user, &d.display().to_string()) {
                    continue;
                }
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    if ["photo-", "passport-", "video-"].iter().any(|pre| n.starts_with(pre)) {
                        found.push((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()));
                    }
                }
            }
            found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
            if found.is_empty() {
                return Ok("No camera pictures or videos yet.".into());
            }
            let z = crate::clock::Zone::system();
            Ok(found.into_iter().take(limit).map(|(t, f)| {
                let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                format!("{} · {} KB · {}", f.display(), kb(&f), crate::clock::format(&z, secs, "%Y-%m-%d %H:%M"))
            }).collect::<Vec<_>>().join("\n"))
        }
        _ => Err(format!("unknown camera tool {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    /// A camera that writes small files and remembers what it was asked.
    #[derive(Default)]
    pub struct FakeCam {
        pub rec: Mutex<Option<(PathBuf, Instant)>>,
        pub asked: Mutex<Vec<String>>,
    }

    impl CameraBackend for FakeCam {
        fn cameras(&self) -> Vec<String> {
            vec!["video0: HD 4MP WEBCAM".into()]
        }
        fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
            self.asked.lock().unwrap().push("photo".into());
            let p = dir.join(format!("photo-{}.jpg", self.asked.lock().unwrap().len()));
            std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
            Ok(p)
        }
        fn record_start(&self, dir: &Path, max_secs: u32) -> Result<PathBuf, String> {
            self.asked.lock().unwrap().push(format!("record {max_secs}"));
            let p = dir.join("video-1.avi");
            std::fs::write(&p, b"RIFF").unwrap();
            *self.rec.lock().unwrap() = Some((p.clone(), Instant::now()));
            Ok(p)
        }
        fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String> {
            let (p, _) = self.rec.lock().unwrap().take().ok_or("not recording")?;
            Ok((vec![p], 42))
        }
        fn recording(&self) -> Option<(PathBuf, f64)> {
            self.rec.lock().unwrap().as_ref().map(|(p, t)| (p.clone(), t.elapsed().as_secs_f64()))
        }
        fn passport(&self, dir: &Path, country: &str, sheet: &str) -> Result<PassportOut, String> {
            self.asked.lock().unwrap().push(format!("passport {country} {sheet}"));
            let photo = dir.join(format!("passport-{country}.png"));
            std::fs::write(&photo, b"png").unwrap();
            let sheet_png = (sheet != "none").then(|| dir.join("sheet.png"));
            Ok(PassportOut { photo, sheet_png, sheet_pdf: None, digital: None, spec: format!("{country}: 2×2 in") })
        }
        fn countries(&self) -> Vec<CountryInfo> {
            vec![CountryInfo { id: "us-passport".into(), name: "USA passport".into(), verified: true }]
        }
    }

    fn setup(tag: &str) -> (Arc<Services>, Policy, PathBuf, Arc<FakeCam>) {
        let d = std::env::temp_dir().join(format!("iyakki-camtools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("svc")).unwrap();
        struct Off;
        impl iyakki_proxy::Upstream for Off {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let svc = Services::open(&d.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        svc.camera.set_home_root(&d.join("home"));
        let mut p = Role::User.template();
        p.fs_write = vec![d.join("home").display().to_string()];
        p.fs_read = p.fs_write.clone();
        (svc, p, d, Arc::new(FakeCam::default()))
    }

    #[test]
    fn the_tools_exist_only_with_a_camera_service_and_the_app_and_picture_taking_asks() {
        let (svc, mut p, d, cam) = setup("defs");
        assert!(defs(&svc, &p).is_empty(), "no shell camera service: no tools");
        svc.camera.set_backend(Some(cam.clone()));
        let names: Vec<(String, Risk)> = defs(&svc, &p).into_iter().map(|d| (d.spec.name, d.risk)).collect();
        assert_eq!(names.len(), 6);
        for (n, r) in &names {
            assert!(is_camera_tool(n));
            assert_eq!(*r == Risk::Ask, takes_pictures(n), "{n}");
        }
        assert!(describe("camera_record_start", &json!({"max_seconds": 9999})).unwrap().contains("600 seconds"));
        assert!(describe("camera_passport", &json!({"country": "us-passport", "sheet": "a4"})).unwrap().contains("a4 print sheet"));
        // external coding agents get only the harmless ones
        let ext: Vec<String> = defs(&svc, &p).into_iter().filter(crate::extagent::bridge::offered).map(|d| d.spec.name).collect();
        assert!(!ext.iter().any(|n| takes_pictures(n)), "{ext:?}");
        p.apps.remove(&App::Camera);
        assert!(defs(&svc, &p).is_empty());
        assert!(call("camera_list", &json!({}), &svc, &p, "amy").is_err());
        assert!(!Role::Guest.template().apps.contains(&App::Camera));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn photo_recording_passport_and_recent_work_in_the_persons_folders_and_mark_the_camera_in_use() {
        let (svc, p, d, cam) = setup("calls");
        svc.camera.set_backend(Some(cam.clone()));
        assert!(call("camera_list", &json!({}), &svc, &p, "amy").unwrap().contains("HD 4MP WEBCAM"));
        let r = call("camera_photo", &json!({}), &svc, &p, "amy").unwrap();
        assert!(r.contains(&d.join("home/amy/Pictures").display().to_string()) && r.contains("IMAGE: "), "{r}");
        assert!(svc.camera.in_use().is_none(), "the sign goes away when the photo is taken");
        let r = call("camera_record_start", &json!({"max_seconds": 5000}), &svc, &p, "amy").unwrap();
        assert!(r.contains("600 seconds") && r.contains("no sound"), "{r}");
        assert_eq!(cam.asked.lock().unwrap().last().unwrap(), "record 600");
        assert!(svc.camera.in_use().unwrap().contains("recording"), "the sign shows while recording");
        assert!(call("camera_record_start", &json!({}), &svc, &p, "amy").unwrap_err().contains("already recording"));
        let r = call("camera_record_stop", &json!({}), &svc, &p, "amy").unwrap();
        assert!(r.contains("42 frames") && r.contains("video-1.avi"), "{r}");
        assert!(svc.camera.in_use().is_none());
        assert!(call("camera_record_stop", &json!({}), &svc, &p, "amy").is_err());
        let e = call("camera_passport", &json!({"country": "atlantis"}), &svc, &p, "amy").unwrap_err();
        assert!(e.contains("us-passport"), "{e}");
        let r = call("camera_passport", &json!({"country": "US-Passport", "sheet": "a4"}), &svc, &p, "amy").unwrap();
        assert!(r.contains("passport-us-passport.png") && r.contains("Print sheet") && r.contains("Rules used: us-passport") && r.contains("nothing was sent"), "{r}");
        let r = call("camera_recent", &json!({"limit": 10}), &svc, &p, "amy").unwrap();
        assert!(r.contains("photo-1.jpg") && r.contains("video-1.avi") && r.contains("passport-us-passport.png"), "{r}");
        assert!(!r.contains("sheet.png"));
        // no write access: nothing is taken
        let mut q = p.clone();
        q.fs_write.clear();
        assert!(call("camera_photo", &json!({}), &svc, &q, "amy").unwrap_err().contains("may not save"));
        // grants last five minutes per person
        assert!(!svc.camera.granted("amy"));
        svc.camera.grant("amy", GRANT_SECS);
        assert!(svc.camera.granted("amy") && !svc.camera.granted("bob"));
        svc.camera.grant("amy", 0);
        assert!(!svc.camera.granted("amy"));
        let _ = std::fs::remove_dir_all(d);
    }
}
