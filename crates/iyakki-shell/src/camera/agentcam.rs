//! The camera as the assistant's tools see it ([`iyakki_agent::camtools::CameraBackend`]): the shell registers one
//! [`AgentCamera`] at start. Each photo opens the first camera, lets the exposure settle for a few frames, keeps one
//! frame and closes the camera again; a recording keeps it open until it is stopped or its time limit is reached. When the
//! CAMERA app holds the camera, the device is busy and the tool says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::camtools::{CameraBackend, CountryInfo, PassportOut};

use super::live::Live;
use super::passport::{self, Paper};
use super::{specs, CamDevice, Opener, Presence, RawFrame};

/// Frames dropped before a picture is kept (auto exposure and white balance settle).
const SETTLE_FRAMES: u64 = 6;
const FRAME_WAIT: Duration = Duration::from_secs(6);

struct Rec {
    live: Live,
    path: PathBuf,
    started: Instant,
    generation: u64,
}

pub struct AgentCamera {
    presence: Mutex<Presence>,
    opener: Arc<dyn Opener>,
    rec: Arc<Mutex<Option<Rec>>>,
    generation: Mutex<u64>,
    settle: u64,
    /// Use the camera of the web browser the person is in when there is one (the real wiring; off for tests of the machine's cameras).
    follow_browser: bool,
}

impl AgentCamera {
    pub fn system() -> AgentCamera {
        AgentCamera::with(Presence::system(), Arc::new(super::v4l2::V4l2Opener)).following_the_browser()
    }

    /// The browser's camera comes first whenever a web page offers one.
    pub fn following_the_browser(mut self) -> AgentCamera {
        self.follow_browser = true;
        self
    }

    pub fn with(presence: Presence, opener: Arc<dyn Opener>) -> AgentCamera {
        AgentCamera { presence: Mutex::new(presence), opener, rec: Arc::new(Mutex::new(None)), generation: Mutex::new(0), settle: SETTLE_FRAMES, follow_browser: false }
    }

    /// Keep the first frame (tests).
    pub fn without_settling(mut self) -> AgentCamera {
        self.settle = 1;
        self
    }

    /// The camera of the web browser the person is in, when there is one (same rule as the CAMERA app: it comes first).
    fn browser(&self) -> Option<Arc<iyakki_agent::browsermedia::BrowserMedia>> {
        if self.follow_browser { iyakki_agent::browsersource::browser_camera_for_desktop() } else { None }
    }

    fn first_camera(&self) -> Result<CamDevice, String> {
        if self.browser().is_some() {
            return Presence::browser().cameras().into_iter().next().ok_or_else(|| "no camera".to_string());
        }
        let mut p = self.presence.lock().unwrap_or_else(|e| e.into_inner());
        p.scan();
        p.cameras().into_iter().next().ok_or_else(|| "no camera is connected (plug in a USB camera)".to_string())
    }

    fn open(&self) -> Result<Live, String> {
        if self.rec.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return Err("the camera is recording; stop the recording first".into());
        }
        let dev = self.first_camera()?;
        let browser = self.browser().map(|m| Arc::new(super::browsercam::BrowserOpener::new(m)) as Arc<dyn Opener>);
        let o = browser.as_ref().unwrap_or(&self.opener).open(&dev, None).map_err(|e| if e.contains("busy") || e.contains("EBUSY") || e.contains("Device or resource busy") { "the camera is in use (is the CAMERA app open? close it and try again)".to_string() } else { e })?;
        Ok(Live::start(o.cap))
    }

    /// One settled frame from a freshly opened camera.
    fn grab(&self) -> Result<Arc<RawFrame>, String> {
        let live = self.open()?;
        let t = Instant::now();
        loop {
            if let Some(e) = live.error() {
                return Err(e);
            }
            if live.frames() >= self.settle {
                if let Some(f) = live.latest() {
                    return Ok(f);
                }
            }
            if t.elapsed() > FRAME_WAIT {
                return Err("the camera sent no picture".into());
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

impl CameraBackend for AgentCamera {
    fn cameras(&self) -> Vec<String> {
        if self.browser().is_some() {
            return Presence::browser().cameras().into_iter().map(|c| format!("{}: {}", c.node, c.name)).collect();
        }
        let mut p = self.presence.lock().unwrap_or_else(|e| e.into_inner());
        p.scan();
        p.cameras().into_iter().map(|c| format!("{}: {}", c.node, c.name)).collect()
    }

    fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
        let f = self.grab()?;
        let jpeg = f.to_jpeg()?;
        let path = super::unique(dir, &format!("photo-{}", super::stamp()), "jpg");
        let written = std::fs::write(&path, jpeg).map_err(|e| format!("could not save the photo: {e}"));
        super::own::give_saved(&path);
        written?;
        Ok(path)
    }

    fn photo_bytes(&self) -> Result<Vec<u8>, String> {
        self.grab()?.to_jpeg()
    }

    fn record_start(&self, dir: &Path, max_secs: u32) -> Result<PathBuf, String> {
        let live = self.open()?;
        let path = super::unique(dir, &format!("video-{}", super::stamp()), "avi");
        let started = live.start_recording(&path);
        super::own::give_saved(&path);
        started?;
        let generation = {
            let mut g = self.generation.lock().unwrap_or_else(|e| e.into_inner());
            *g += 1;
            *g
        };
        *self.rec.lock().unwrap_or_else(|e| e.into_inner()) = Some(Rec { live, path: path.clone(), started: Instant::now(), generation });
        // the time limit: stop this recording (not a later one) when it is reached
        let rec = self.rec.clone();
        std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(max_secs as u64);
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let mut g = rec.lock().unwrap_or_else(|e| e.into_inner());
                match g.as_ref() {
                    Some(r) if r.generation == generation => {
                        if Instant::now() >= end || r.live.error().is_some() {
                            if let Some(r) = g.take() {
                                if let Some(Ok((files, _))) = r.live.stop_recording() {
                                    files.iter().for_each(|f| super::own::give_saved(f));
                                }
                            }
                            return;
                        }
                    }
                    _ => return,
                }
            }
        });
        Ok(path)
    }

    fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String> {
        let r = self.rec.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or("nothing is being recorded")?;
        let done = r.live.stop_recording().ok_or("nothing is being recorded")?;
        if let Ok((files, _)) = &done {
            files.iter().for_each(|f| super::own::give_saved(f));
        }
        done
    }

    fn recording(&self) -> Option<(PathBuf, f64)> {
        self.rec.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| (r.path.clone(), r.started.elapsed().as_secs_f64()))
    }

    fn passport(&self, dir: &Path, country: &str, sheet: &str) -> Result<PassportOut, String> {
        let s = *specs::spec(country).ok_or_else(|| format!("unknown country {country}"))?;
        let paper = match sheet {
            "4x6" => Some(Paper::Photo4x6),
            "a4" => Some(Paper::A4),
            _ => None,
        };
        let f = self.grab()?;
        let (fw, fh, rgba) = f.to_rgba()?;
        // the head is assumed in the middle of the picture, a little above centre, as the app's frame starts
        let crop = passport::crop_rect(&s, fw, fh, 0.8, 0.5, 0.5);
        let saved = passport::save(dir, &s, fw, fh, &rgba, crop, paper, &super::stamp())?;
        for f in [Some(&saved.photo), saved.sheet_png.as_ref(), saved.sheet_pdf.as_ref(), saved.digital.as_ref()].into_iter().flatten() {
            super::own::give_saved(f);
        }
        let mut spec = s.describe();
        if saved.low_res {
            spec.push_str(". The camera picture was small for this size, so the photo was enlarged");
        }
        Ok(PassportOut { photo: saved.photo, sheet_png: saved.sheet_png, sheet_pdf: saved.sheet_pdf, digital: saved.digital, spec })
    }

    fn countries(&self) -> Vec<CountryInfo> {
        specs::SPECS.iter().filter(|s| s.id != specs::CUSTOM_ID).map(|s| CountryInfo { id: s.id.into(), name: s.name.into(), verified: s.verified }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::detect::tests::FakeMachine;
    use crate::camera::fake::FakeOpener;

    #[test]
    fn the_assistants_camera_takes_photos_records_with_a_limit_and_makes_passport_photos() {
        let m = FakeMachine::new("agentcam");
        let d = m.dir().join("out");
        std::fs::create_dir_all(&d).unwrap();
        let cam = AgentCamera::with(m.presence(), Arc::new(FakeOpener::default())).without_settling();
        assert!(cam.cameras().is_empty());
        assert!(cam.photo(&d).unwrap_err().contains("no camera"));
        m.plug_webcam(0, "HD 4MP WEBCAM");
        assert_eq!(cam.cameras(), vec!["video0: HD 4MP WEBCAM"]);
        let p = cam.photo(&d).unwrap();
        let b = crate::camera::jpeg::decode(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!((b.width, b.height), (640, 360));
        // a recording stops by itself at its limit
        let v = cam.record_start(&d, 1).unwrap();
        assert!(cam.recording().is_some());
        assert!(cam.photo(&d).unwrap_err().contains("recording"), "one user of the camera at a time");
        for _ in 0..60 {
            if cam.recording().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(cam.recording().is_none(), "the time limit stopped it");
        let info = crate::camera::avi::parse(&std::fs::read(&v).unwrap()).unwrap();
        assert!(info.frames > 0);
        assert!(cam.record_stop().is_err());
        // stopped by hand
        cam.record_start(&d, 600).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let (files, _) = cam.record_stop().unwrap();
        assert!(crate::camera::avi::parse(&std::fs::read(&files[0]).unwrap()).is_ok());
        // a passport photo for a country with online rules: print photo, online file, sheet, and the rules used
        let out = cam.passport(&d, "us-visa", "4x6").unwrap();
        assert!(out.photo.exists() && out.digital.as_ref().unwrap().exists() && out.sheet_pdf.as_ref().unwrap().exists());
        assert!(out.spec.contains("US visa") && out.spec.contains("UNVERIFIED") && out.spec.contains("enlarged"), "{}", out.spec);
        let out = cam.passport(&d, "uk-passport", "none").unwrap();
        assert!(out.sheet_png.is_none() && out.spec.contains("gov.uk"));
        assert!(cam.passport(&d, "atlantis", "none").is_err());
        assert!(cam.countries().iter().any(|c| c.id == "in-passport" && c.verified));
        assert!(!cam.countries().iter().any(|c| c.id == "custom"));
    }

    #[test]
    fn the_assistants_camera_is_the_browsers_when_a_page_offers_it_and_asks_the_page_first() {
        use iyakki_agent::browsermedia::{BrowserMedia, Kind, Policy, CH_CAMERA};
        let dir = std::env::temp_dir().join(format!("iyakki-agentcam-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = BrowserMedia::silent(Policy::allow_all());
        iyakki_agent::browsermedia::register("agentcam-browser-user", m.clone());
        // a minimal JPEG: SOI, SOF0 of 320x240, a start-of-scan header with a little data, end marker
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8];
        jpeg.extend(240u16.to_be_bytes());
        jpeg.extend(320u16.to_be_bytes());
        jpeg.extend([1, 1, 0x11, 0]);
        jpeg.extend([0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 0x3F, 0, 0x12, 0x34, 0xFF, 0xD9]);
        let cam = AgentCamera::with(Presence::none(), Arc::new(FakeOpener::default())).without_settling().following_the_browser();
        assert_eq!(cam.cameras(), vec!["browser: Browser camera"], "listed as the Browser camera");
        // the person clicks Camera in the page and frames arrive
        m.set(Kind::Camera, true).unwrap();
        let feeder = {
            let (m, jpeg) = (m.clone(), jpeg.clone());
            std::thread::spawn(move || {
                for _ in 0..40 {
                    let mut msg = vec![CH_CAMERA];
                    msg.extend_from_slice(&jpeg);
                    m.binary(&msg);
                    std::thread::sleep(Duration::from_millis(160));
                }
            })
        };
        let bytes = cam.photo_bytes();
        feeder.join().unwrap();
        iyakki_agent::browsermedia::unregister("agentcam-browser-user");
        assert!(bytes.is_ok(), "{bytes:?}");
        // a camera tool that does not follow the browser keeps to the machine
        assert!(AgentCamera::with(Presence::none(), Arc::new(FakeOpener::default())).cameras().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
