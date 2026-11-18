//! Tests of the camera tools (they need the services).

use super::camkit::*;
use iyakki_shared::camtools::*;
use std::path::{Path, PathBuf};
use crate::agent::Risk;
use super::camkit::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};
use crate::services::Services;

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
