//! The camera tools of the assistant over the services (the hub and its backend trait are `iyakki_shared::camtools`).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;
use iyakki_shared::camtools::{kb, record_secs};

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

fn folder(svc: &Services, p: &Policy, user: &str, name: &str) -> Result<PathBuf, String> {
    let d = svc.camera.home(user).join(name);
    if !p.allows_write(user, &d.display().to_string()) {
        return Err(format!("your account may not save in {}", d.display()));
    }
    std::fs::create_dir_all(&d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
    Ok(d)
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
            let z = iyakki_clock::Zone::system();
            Ok(found.into_iter().take(limit).map(|(t, f)| {
                let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                format!("{} · {} KB · {}", f.display(), kb(&f), iyakki_clock::format(&z, secs, "%Y-%m-%d %H:%M"))
            }).collect::<Vec<_>>().join("\n"))
        }
        _ => Err(format!("unknown camera tool {name}")),
    }
}

/// Take the picture, keep it in the chat's folder and answer in the form the chat draws under the call.
pub fn capture(svc: &crate::services::Services, p: &Policy, user: &str, cwd: &std::path::Path) -> Result<String, String> {
    if p.fs_write.is_empty() {
        return Err("your account may not save files, so the picture cannot be kept for the chat".into());
    }
    let f = svc.screen.request(user, Duration::from_millis(svc.screen.wait_ms.load(Ordering::SeqCst)))?;
    if f.w == 0 || f.h == 0 || f.rgba.len() != f.w as usize * f.h as usize * 4 {
        return Err("the screen picture was empty or damaged; try again".into());
    }
    let png = iyakki_shared::browserjobs::png::encode_rgba(f.w, f.h, &f.rgba);
    let stamp = iyakki_clock::format(&iyakki_clock::Zone::system(), iyakki_clock::now_unix(), "%Y%m%d-%H%M%S");
    for n in 0..1000 {
        let file = if n == 0 { format!("screen-{stamp}.png") } else { format!("screen-{stamp}-{}.png", n + 1) };
        let path = crate::scope::resolve(cwd, &file)?;
        crate::scope::check_write(p, user, &path)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut out) => {
                std::io::Write::write_all(&mut out, &png).map_err(|e| e.to_string())?;
                svc.browser.audit(user, "screen.capture", &format!("{}x{}", f.w, f.h));
                return Ok(format!("Your IYAKKI screen ({}x{}) is shown in the chat (saved as {}, {} KB). It was captured as it was; nothing was hidden.\nIMAGE: {}", f.w, f.h, path.display(), png.len().div_ceil(1024), path.display()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not find a free file name".into())
}

