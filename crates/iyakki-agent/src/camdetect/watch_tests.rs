//! The motion service against a scripted vault, a fake clock, synthetic pictures and (for the end-to-end tests) the in-house fake RTSP
//! camera on the loopback address. No real camera was used.

use super::*;
use crate::camdetect::person::{FakePersonDetector, PersonBox};
use crate::cameras::{CamSecrets, Env, NewSpec, NoH264, NoUsb};
use crate::camrec::{Mode, RecEnv, RecFeed, RecSource, Recorder, Unit};
use crate::camrec_format::{RecKind, Segment};
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use std::collections::HashMap as Map;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const JPG: &[u8] = include_bytes!("../../../iyakki-rtsp/tests/data/test-48x32.jpg");
const T0: u64 = 1_700_000_000_000; // 2023-11-14 22:13:20 UTC
const W: u32 = 160;
const H: u32 = 120;

#[derive(Default)]
struct Vault {
    items: Mutex<Map<(String, String), String>>,
    locked: AtomicBool,
}

impl CamSecrets for Vault {
    fn get(&self, owner: &str, name: &str) -> Result<zeroize::Zeroizing<String>, String> {
        if self.locked.load(Ordering::SeqCst) {
            return Err("the vault is locked".into());
        }
        self.items.lock().unwrap().get(&(owner.into(), name.into())).map(|v| zeroize::Zeroizing::new(v.clone())).ok_or_else(|| "no such secret".into())
    }
    fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), String> {
        if self.locked.load(Ordering::SeqCst) {
            return Err("locked".into());
        }
        self.items.lock().unwrap().insert((owner.into(), name.into()), value.into());
        Ok(())
    }
    fn delete(&self, owner: &str, name: &str) -> Result<(), String> {
        self.items.lock().unwrap().remove(&(owner.into(), name.into()));
        Ok(())
    }
    fn has(&self, owner: &str, name: &str) -> bool {
        self.items.lock().unwrap().contains_key(&(owner.into(), name.into()))
    }
}

#[derive(Default)]
struct CapSink {
    got: Mutex<Vec<(String, String, Notice, Option<usize>)>>,
}

impl AlertSink for CapSink {
    fn notify(&self, user: &str, camera: &str, n: &Notice, thumb: Option<&[u8]>) {
        self.got.lock().unwrap().push((user.into(), camera.into(), n.clone(), thumb.map(|t| t.len())));
    }
}

/// The "encoder" of these tests keeps the pixels exactly (so a test can look at them): `FF D8 'M' 'K' width height rgba`.
fn pack(w: u32, h: u32, d: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8, b'M', b'K'];
    v.extend(w.to_be_bytes());
    v.extend(h.to_be_bytes());
    v.extend_from_slice(d);
    v
}

fn unpack(b: &[u8]) -> (u32, u32, &[u8]) {
    assert!(b.starts_with(&[0xFF, 0xD8, b'M', b'K']), "not one of the test pictures");
    (u32::from_be_bytes(b[4..8].try_into().unwrap()), u32::from_be_bytes(b[8..12].try_into().unwrap()), &b[12..])
}

struct Rig {
    hub: Arc<Hub>,
    svc: Arc<MotionService>,
    vault: Arc<Vault>,
    sink: Arc<CapSink>,
    now: Arc<AtomicU64>,
    recording: Arc<AtomicBool>,
    thumbs: Arc<Mutex<Vec<(u32, u32, Vec<u8>)>>>,
    saved: Arc<Mutex<Vec<PathBuf>>>,
    dir: PathBuf,
    id: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.svc.shutdown();
        self.hub.stop_all();
    }
}

fn who(r: &Rig, name: &str, manage: bool) -> Who {
    Who { user: name.into(), manage, usb_ok: false, home: r.dir.join("home").join(name), may_write: Arc::new(|_| true) }
}

fn rig_at(tag: &str, address: &str, real_jpeg: bool) -> Rig {
    let dir = std::fs::canonicalize(crate::testdir::unique("camdetect", tag)).unwrap();
    let vault = Arc::new(Vault::default());
    let thumbs: Arc<Mutex<Vec<(u32, u32, Vec<u8>)>>> = Arc::new(Mutex::new(vec![]));
    let saved = Arc::new(Mutex::new(vec![]));
    let (t2, s2) = (thumbs.clone(), saved.clone());
    let hub = Arc::new(Hub::new(Env {
        data: dir.join("data"),
        secrets: vault.clone(),
        people: Arc::new(|| vec!["amy".into(), "bob".into(), "root".into(), "eve".into()]),
        usb: Arc::new(NoUsb),
        h264: Arc::new(NoH264),
        encode: Arc::new(move |w, h, d| {
            t2.lock().unwrap().push((w, h, d.to_vec()));
            if real_jpeg { JPG.to_vec() } else { pack(w, h, d) }
        }),
        after_save: Arc::new(move |p| s2.lock().unwrap().push(p.to_path_buf())),
        audit: Arc::new(|_, _, _| {}),
        backoff_ms: 30,
        idle: Duration::from_secs(5),
    }));
    let now = Arc::new(AtomicU64::new(T0));
    let recording = Arc::new(AtomicBool::new(false));
    let (n2, r2) = (now.clone(), recording.clone());
    let svc = Arc::new(MotionService::new(
        hub.clone(),
        MotionEnv {
            now_ms: Arc::new(move || n2.load(Ordering::SeqCst)),
            zone: Arc::new(Zone::utc),
            recording: Arc::new(move |_| r2.load(Ordering::SeqCst)),
            on_active: Arc::new(|_| {}),
            key_every: Duration::ZERO,
            retention_every: Duration::from_secs(3600),
        },
    ));
    let sink = Arc::new(CapSink::default());
    svc.set_sink(sink.clone());
    let mut r = Rig { hub, svc, vault, sink, now, recording, thumbs, saved, dir, id: String::new() };
    let amy = who(&r, "amy", false);
    r.id = r.hub.save(&amy, None, &NewSpec { name: "Front door".into(), address: address.into(), user: "admin".into(), allowed: vec!["bob".into()] }, Some("pw-12345")).unwrap();
    r
}

fn rig(tag: &str) -> Rig {
    rig_at(tag, "rtsp://127.0.0.1:9/live", false)
}

/// The textured scene with an optional bright rectangle `(x, y, w, h)`.
fn frame(rect: Option<(u32, u32, u32, u32)>) -> Vec<u8> {
    let mut v = vec![255u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let base = 60 + (80 * x / W) + (40 * y / H) + if ((x / 10) + (y / 10)) % 2 == 0 { 25 } else { 0 };
            let o = ((y * W + x) * 4) as usize;
            v[o] = base as u8;
            v[o + 1] = base as u8;
            v[o + 2] = base as u8;
        }
    }
    if let Some((rx, ry, rw, rh)) = rect {
        for y in ry..(ry + rh).min(H) {
            for x in rx..(rx + rw).min(W) {
                let o = ((y * W + x) * 4) as usize;
                v[o] = 250;
                v[o + 1] = 250;
                v[o + 2] = 250;
            }
        }
    }
    v
}

impl Rig {
    fn amy(&self) -> Who {
        who(self, "amy", false)
    }
    fn feed(&self, f: &[u8]) {
        self.now.fetch_add(200, Ordering::SeqCst);
        self.svc.process(&self.id, W, H, f);
    }
    fn hold(&self, f: &[u8], n: usize) {
        for _ in 0..n {
            self.feed(f);
        }
    }
    /// A rectangle walking left to right at the given height.
    fn walk(&self, y: u32, x0: u32) {
        for k in 0..16u32 {
            self.feed(&frame(Some((x0 + k * 5, y, 20, 20))));
        }
        self.hold(&frame(None), 30);
    }
    fn on(&self, mode: WatchMode) {
        self.svc.set(&self.amy(), &self.id, &Change { mode: Some(mode), ..Default::default() }).unwrap();
    }
    fn events(&self) -> Vec<Event> {
        self.svc.events(&self.amy(), &self.id, 0, u64::MAX, 100).unwrap()
    }
    fn key_hex(&self) -> String {
        self.vault.get("amy", &key_name(&self.id)).unwrap().to_string()
    }
    fn alerts(&self, a: AlertChange) {
        self.svc.set(&self.amy(), &self.id, &Change { alerts: Some(a), ..Default::default() }).unwrap();
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("data/cameras/recordings").join(&self.id).join("audit.log")).unwrap_or_default()
    }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Pt> {
    crate::camdetect::motion::rect_points(x0, y0, x1, y1)
}

#[test]
fn nothing_is_analysed_or_stored_until_the_owner_turns_it_on_and_only_the_owner_can() {
    let r = rig("rights");
    // off by default: no key, no folder, nothing taken from the picture tap
    assert!(!r.vault.has("amy", &key_name(&r.id)), "no key is made before the owner turns it on");
    assert!(!r.svc.wants(&r.id));
    r.hold(&frame(None), 10);
    assert!(r.svc.analysing_ids().is_empty() && !r.dir.join("data/cameras/events").exists());
    let st = r.svc.status(&r.amy(), &r.id).unwrap();
    assert_eq!((st.settings.mode, st.owner), (WatchMode::Off, true));
    assert!(st.state.contains("off"));
    // the person it is shared with, an administrator and a stranger cannot turn it on
    let on = Change { mode: Some(WatchMode::Always), ..Default::default() };
    assert!(r.svc.set(&who(&r, "bob", false), &r.id, &on).unwrap_err().contains("only the person who added"));
    assert!(r.svc.set(&who(&r, "root", true), &r.id, &on).unwrap_err().contains("only the person who added"));
    assert!(r.svc.set(&who(&r, "eve", false), &r.id, &on).unwrap_err().contains("no such camera"));
    assert!(r.svc.status(&who(&r, "eve", false), &r.id).is_err(), "a stranger does not learn the camera exists");
    assert!(r.log().contains("refused: not the owner"), "refusals are written to the camera's log");
    assert!(!r.vault.has("amy", &key_name(&r.id)), "a refusal made no key");
    // this computer's own camera is never watched in the background
    assert!(r.svc.set(&r.amy(), "usb:video0", &on).unwrap_err().contains("network cameras"));
    // the owner can; the person it is shared with then sees THAT it is watched, and nothing more
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Mask, "Neighbour", false, 5, &rect(0.6, 0.0, 1.0, 0.5)).unwrap();
    r.on(WatchMode::Always);
    assert_eq!(r.key_hex().len(), 64, "the camera's key is in the owner's vault, the same entry recordings use");
    let bob = r.svc.status(&who(&r, "bob", false), &r.id).unwrap();
    assert_eq!((bob.settings.mode, bob.owner), (WatchMode::Always, false));
    assert!(bob.settings.masks.is_empty() && bob.settings.zones.is_empty(), "no shapes for a viewer");
    assert!(r.svc.events(&who(&r, "bob", false), &r.id, 0, u64::MAX, 10).unwrap_err().contains("only the person who added"));
    assert!(r.svc.events(&who(&r, "root", true), &r.id, 0, u64::MAX, 10).is_err(), "an administrator is given no pictures");
    assert!(r.svc.shape_set(&who(&r, "bob", false), &r.id, ShapeKind::Mask, "x", false, 5, &rect(0.1, 0.1, 0.2, 0.2)).is_err());
    // an administrator may switch it off
    r.svc.set(&who(&r, "root", true), &r.id, &Change { mode: Some(WatchMode::Off), ..Default::default() }).unwrap();
    assert_eq!(r.svc.status(&r.amy(), &r.id).unwrap().settings.mode, WatchMode::Off);
    assert!(!r.svc.wants(&r.id));
}

#[test]
fn motion_becomes_an_event_with_zone_score_and_thumbnail_and_masked_pixels_never_reach_the_detector_or_the_thumbnail() {
    let r = rig("detect");
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Zone, "Porch", true, 5, &rect(0.0, 0.0, 1.0, 1.0)).unwrap();
    // a privacy mask over the top right; the camera sees a street there
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Mask, "Street", false, 5, &rect(0.5, 0.0, 1.0, 0.5)).unwrap();
    r.on(WatchMode::Always);
    r.hold(&frame(None), 20);
    // plenty of motion, all of it inside the mask: the detector sees a black patch and nothing moves
    r.walk(10, 85);
    assert!(r.events().is_empty(), "motion inside a privacy mask is not seen: {:?}", r.events());
    assert!(r.thumbs.lock().unwrap().is_empty(), "and no thumbnail was made");
    // motion below the mask
    r.walk(80, 10);
    let ev = r.events();
    assert_eq!(ev.len(), 1, "{ev:?}");
    let e = &ev[0];
    assert_eq!(e.zone, "Porch");
    assert!(e.score >= 1 && e.peak >= e.score && e.end_ms.is_some() && e.has_thumb);
    assert!(e.start_ms > T0 && e.bbox[1] > 0.4, "box is low in the picture: {:?}", e.bbox);
    // the thumbnail given to the encoder had the masked area black (box averages of zeros stay zero)
    let th = r.thumbs.lock().unwrap();
    assert_eq!(th.len(), 1);
    let (tw, thh, px) = (&th[0].0, &th[0].1, &th[0].2);
    assert!(*tw <= THUMB_W);
    let at = |fx: f32, fy: f32| {
        let o = (((fy * *thh as f32) as usize) * *tw as usize + (fx * *tw as f32) as usize) * 4;
        px[o]
    };
    assert_eq!(at(0.75, 0.25), 0, "the masked street is black in the thumbnail");
    assert!(at(0.25, 0.25) > 40, "the rest of the picture is not");
    // the log has the story without pictures
    let log = r.log();
    assert!(log.contains("motion.start") && log.contains("motion.end") && log.contains("motion.setting"));
}

#[test]
fn the_event_list_is_encrypted_with_the_recording_key_and_retention_removes_it() {
    let r = rig("retain");
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Zone, "Secretgarden", true, 5, &rect(0.0, 0.0, 1.0, 1.0)).unwrap();
    r.on(WatchMode::Always);
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    assert_eq!(r.events().len(), 1);
    let dir = r.dir.join("data/cameras/events").join(&r.id);
    let files: Vec<PathBuf> = walk(&dir);
    assert!(!files.is_empty() && files.iter().all(|f| f.extension().is_some_and(|x| x == "iev")));
    for f in &files {
        let raw = std::fs::read(f).unwrap();
        assert!(!raw.windows(12).any(|w| w == b"Secretgarden") && !raw.windows(4).any(|w| w == b"zone"), "no readable text in {f:?}");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let settings = std::fs::read_to_string(r.dir.join("data/cameras/motion/settings.json")).unwrap();
    assert!(!settings.contains(&r.key_hex()), "no key in the settings file");
    // the key is the camera's key: a different key finds nothing and says so
    r.vault.items.lock().unwrap().insert(("amy".into(), key_name(&r.id)), "00".repeat(32));
    assert!(r.svc.events(&r.amy(), &r.id, 0, u64::MAX, 10).unwrap_err().contains("does not open"));
    r.vault.items.lock().unwrap().remove(&("amy".into(), key_name(&r.id)));
    // retention: switched off (files closed), then 20 days later with the default of 14 days
    r.on(WatchMode::Off);
    r.now.fetch_add(20 * 86_400_000, Ordering::SeqCst);
    let (n, bytes) = r.svc.enforce();
    assert!(n >= 1 && bytes > 0, "{n} {bytes}");
    assert!(walk(&dir).is_empty());
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { v.extend(walk(&p)) } else { v.push(p) }
        }
    }
    v
}

#[test]
fn closing_the_vault_stops_analysis_and_storage_and_opening_it_starts_from_a_fresh_background() {
    let r = rig("vault");
    r.on(WatchMode::Always);
    r.hold(&frame(None), 20);
    assert!(r.svc.wants(&r.id) && r.svc.analysing_ids() == vec![r.id.clone()]);
    // sign-out: the vault closes
    r.vault.locked.store(true, Ordering::SeqCst);
    r.svc.tick();
    assert!(!r.svc.wants(&r.id), "no key, nothing is taken from the stream");
    let st = r.svc.status(&r.amy(), &r.id).unwrap();
    assert!(st.state.contains("Waiting") && st.state.contains("vault"), "{}", st.state);
    // even a picture pushed by force is dropped, and a walk stores nothing
    r.walk(60, 10);
    assert!(r.svc.analysing_ids().is_empty(), "nothing was analysed");
    assert!(r.svc.events(&r.amy(), &r.id, 0, u64::MAX, 10).is_err(), "the events cannot be opened without the key");
    let files_before = walk(&r.dir.join("data/cameras/events").join(&r.id)).len();
    // the vault opens again: the first pictures only settle the background, a changed scene is not an event
    r.vault.locked.store(false, Ordering::SeqCst);
    r.svc.tick();
    assert!(r.svc.wants(&r.id));
    let other = frame(Some((100, 90, 40, 20)));
    r.hold(&other, 30);
    assert_eq!(r.events().len(), 0, "the scene changed while nobody looked: not an event");
    assert_eq!(walk(&r.dir.join("data/cameras/events").join(&r.id)).len(), files_before, "and nothing was written meanwhile");
}

#[test]
fn watched_mode_looks_only_while_the_camera_is_on_a_screen_or_recorded() {
    let r = rig("watched");
    r.on(WatchMode::Watched);
    assert!(!r.svc.wants(&r.id), "nobody looks and nothing records");
    assert!(r.svc.status(&r.amy(), &r.id).unwrap().state.contains("not looking"));
    assert!(r.svc.always_names().is_empty(), "no red sign is needed in this mode: the picture on screen or the REC sign is the indicator");
    r.recording.store(true, Ordering::SeqCst);
    assert!(r.svc.wants(&r.id), "recording counts (the REC sign shows)");
    r.recording.store(false, Ordering::SeqCst);
    assert!(!r.svc.wants(&r.id));
    // always mode has its own red sign
    r.on(WatchMode::Always);
    assert_eq!(r.svc.always_names(), vec!["Front door".to_string()]);
    assert_eq!(super::stop_all_for(&r.dir.join("data"), "amy"), 0, "the registry knows only services made by `shared`");
    assert_eq!(r.svc.stop_all_for("nobody"), 0, "somebody else's click stops nothing");
    assert_eq!(r.svc.stop_all_for("amy"), 1);
    assert!(r.svc.always_names().is_empty() && !r.svc.wants(&r.id), "the sign's click switches it off");
}

#[test]
fn alerts_respect_the_switch_the_rate_limit_quiet_hours_and_the_thumbnail_choice() {
    let r = rig("alerts");
    r.on(WatchMode::Always);
    r.hold(&frame(None), 20);
    // alerts are off by default: an event makes no notice
    r.walk(60, 10);
    assert_eq!(r.events().len(), 1);
    assert!(r.sink.got.lock().unwrap().is_empty());
    // on, no thumbnail: one notice for the owner, no picture attached
    r.alerts(AlertChange { enabled: Some(true), max_per_hour: Some(2), group_s: Some(10), ..Default::default() });
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    {
        let got = r.sink.got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].1.as_str(), got[0].2.title.as_str()), ("amy", r.id.as_str(), "Motion at Front door"));
        assert_eq!(got[0].3, None, "the thumbnail is not attached unless the person allowed it");
    }
    // the hourly limit: more events inside the hour make no more than the limit says
    for _ in 0..4 {
        r.now.fetch_add(60_000, Ordering::SeqCst);
        r.hold(&frame(None), 20);
        r.walk(60, 10);
    }
    assert_eq!(r.events().len(), 6, "every event is still stored");
    assert!(r.sink.got.lock().unwrap().len() <= 2, "at most 2 notices in the hour: {}", r.sink.got.lock().unwrap().len());
    // thumbnail allowed, an hour later: it travels with the notice
    r.now.fetch_add(2 * 3_600_000, Ordering::SeqCst);
    r.alerts(AlertChange { thumbnail: Some(true), ..Default::default() });
    r.hold(&frame(None), 20);
    let before = r.sink.got.lock().unwrap().len();
    r.walk(60, 10);
    let got = r.sink.got.lock().unwrap();
    assert!(got.len() > before && got.last().unwrap().3.is_some(), "the thumbnail goes along when allowed");
}

#[test]
fn quiet_hours_hold_notices_and_one_summary_follows_when_they_end() {
    let r = rig("quiet");
    r.on(WatchMode::Always);
    // 22:13 UTC in the test zone: quiet from 22:00 to 23:00
    r.alerts(AlertChange { enabled: Some(true), quiet: Some("22:00-23:00".into()), ..Default::default() });
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    r.now.fetch_add(120_000, Ordering::SeqCst);
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    assert_eq!(r.events().len(), 2);
    assert!(r.sink.got.lock().unwrap().is_empty(), "nothing during quiet hours");
    r.now.fetch_add(3_600_000, Ordering::SeqCst);
    r.svc.tick();
    let got = r.sink.got.lock().unwrap();
    assert_eq!(got.len(), 1, "one summary");
    assert!(got[0].2.title.contains("2 motion events") && got[0].2.title.contains("quiet hours"), "{}", got[0].2.title);
    // a bad window is refused and changes nothing
    drop(got);
    assert!(r.svc.set(&r.amy(), &r.id, &Change { alerts: Some(AlertChange { quiet: Some("late".into()), ..Default::default() }), ..Default::default() }).is_err());
}

#[test]
fn the_person_hook_says_not_available_by_default_and_a_fake_detector_marks_events() {
    let r = rig("person");
    r.svc.set(&r.amy(), &r.id, &Change { person: Some(true), alerts: Some(AlertChange { enabled: Some(true), ..Default::default() }), ..Default::default() }).unwrap();
    r.on(WatchMode::Always);
    let st = r.svc.status(&r.amy(), &r.id).unwrap();
    assert!(st.person_note.contains("not available") && r.svc.person_availability().is_err());
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    let ev = r.events();
    assert_eq!((ev.len(), ev[0].person), (1, None), "no guess: the event is motion, not person");
    assert_eq!(r.sink.got.lock().unwrap()[0].2.title, "Motion at Front door");
    // a detector that answers
    let fake = Arc::new(FakePersonDetector::new(vec![Ok(vec![PersonBox { x: 0.1, y: 0.2, w: 0.2, h: 0.6, confidence: 0.9 }])]));
    r.svc.set_person_detector(fake.clone());
    r.now.fetch_add(3_600_000, Ordering::SeqCst);
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    let ev = r.events();
    assert_eq!(ev[0].person, Some(true));
    assert_eq!(r.sink.got.lock().unwrap().last().unwrap().2.title, "Person at Front door");
    let asked = fake.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].0 <= 480 && asked[0].2 > 0);
    // switched off: the detector is not asked
    drop(asked);
    r.svc.set(&r.amy(), &r.id, &Change { person: Some(false), ..Default::default() }).unwrap();
    r.now.fetch_add(3_600_000, Ordering::SeqCst);
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    assert_eq!(fake.asked.lock().unwrap().len(), 1);
}

#[test]
fn shapes_are_checked_limited_replaced_by_name_and_removed() {
    let r = rig("shapes");
    let amy = r.amy();
    let bow = vec![Pt::new(0.1, 0.1), Pt::new(0.9, 0.9), Pt::new(0.9, 0.1), Pt::new(0.1, 0.9)];
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Zone, "Bow", true, 5, &bow).unwrap_err().contains("crosses itself"));
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Mask, "Tiny", false, 5, &rect(0.1, 0.1, 0.1001, 0.1001)).is_err());
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Mask, "Out", false, 5, &rect(0.5, 0.5, 1.4, 0.9)).is_err());
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Zone, "  ", true, 5, &rect(0.1, 0.1, 0.5, 0.5)).is_err());
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Zone, "Ok", true, 11, &rect(0.1, 0.1, 0.5, 0.5)).is_err());
    assert!(!r.dir.join("data/cameras/motion/settings.json").exists(), "refused shapes changed nothing");
    r.svc.shape_set(&amy, &r.id, ShapeKind::Zone, "Door", true, 7, &rect(0.1, 0.1, 0.5, 0.5)).unwrap();
    r.svc.shape_set(&amy, &r.id, ShapeKind::Zone, "door", true, 3, &rect(0.2, 0.2, 0.6, 0.6)).unwrap();
    let s = r.svc.status(&amy, &r.id).unwrap().settings;
    assert_eq!((s.zones.len(), s.zones[0].sensitivity), (1, 3), "same name (any case) replaces");
    for i in 0..11 {
        r.svc.shape_set(&amy, &r.id, ShapeKind::Mask, &format!("m{i}"), false, 5, &rect(0.1, 0.1, 0.2, 0.2)).unwrap();
    }
    r.svc.shape_set(&amy, &r.id, ShapeKind::Mask, "m11", false, 5, &rect(0.1, 0.1, 0.2, 0.2)).unwrap();
    assert!(r.svc.shape_set(&amy, &r.id, ShapeKind::Mask, "m12", false, 5, &rect(0.1, 0.1, 0.2, 0.2)).unwrap_err().contains("at most 12"));
    r.svc.shape_remove(&amy, &r.id, ShapeKind::Zone, "DOOR").unwrap();
    assert!(r.svc.shape_remove(&amy, &r.id, ShapeKind::Zone, "Door").unwrap_err().contains("no zone called"));
    assert!(r.svc.status(&amy, &r.id).unwrap().settings.zones.is_empty());
    // the file holds shapes and names, 0600, and a hand-edited bad shape is dropped on load
    let f = r.dir.join("data/cameras/motion/settings.json");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    let mut file = settings::load(&r.dir.join("data"));
    file.cameras[0].masks[0].points = vec![[0.1, 0.1], [0.9, 0.9], [0.9, 0.1], [0.1, 0.9]];
    settings::save(&r.dir.join("data"), &file).unwrap();
    assert!(settings::load(&r.dir.join("data")).cameras.is_empty());
    // a damaged file: stored pictures fail closed (the whole picture is covered)
    std::fs::write(&f, b"{ not json").unwrap();
    settings::forget_cache(&r.dir.join("data"));
    let m = settings::storage_masks(&r.dir.join("data"), &r.id).unwrap();
    assert_eq!(m.len(), 1);
    assert!(geom::contains(&m[0], 0.5, 0.5) && geom::contains(&m[0], 0.01, 0.99));
}

/// A recording source that sends the 48x32 test picture.
struct JpgSource;
struct JpgFeed;
impl RecFeed for JpgFeed {
    fn next(&mut self) -> Result<Unit, crate::cameras::FeedErr> {
        std::thread::sleep(Duration::from_millis(10));
        Ok(Unit::Jpeg(JPG.to_vec()))
    }
    fn kind(&self) -> RecKind {
        RecKind::Jpeg
    }
}
impl RecSource for JpgSource {
    fn open(&self, _: &Spec) -> Result<Box<dyn RecFeed>, crate::cameras::FeedErr> {
        Ok(Box::new(JpgFeed))
    }
}

fn recorder(r: &Rig) -> Arc<Recorder> {
    let rec = Arc::new(Recorder::new(
        r.hub.clone(),
        RecEnv {
            source: Arc::new(JpgSource),
            free_space: Arc::new(|_| Some(500_000 * 1024 * 1024)),
            now_ms: Arc::new(camrec::system_ms),
            zone: Arc::new(Zone::utc),
            on_active: Arc::new(|_| {}),
            segment_ms: 100,
            retention_every: Duration::from_secs(3600),
            backoff_ms: 20,
            retry_after: Duration::from_millis(60),
        },
    ));
    rec
}

fn record_a_moment(r: &Rig, rec: &Arc<Recorder>) -> Vec<Vec<u8>> {
    let amy = r.amy();
    rec.set(&amy, &r.id, &camrec::Change { mode: Some(Mode::Continuous), ..Default::default() }).unwrap();
    let t = Instant::now();
    while rec.entries(&r.id).len() < 2 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(30));
    }
    rec.set(&amy, &r.id, &camrec::Change { mode: Some(Mode::Off), ..Default::default() }).unwrap();
    rec.shutdown();
    let key = crate::camrec_format::RecKey::from_hex(&r.key_hex()).unwrap();
    let mut frames = vec![];
    for e in rec.entries(&r.id) {
        let seg = Segment::open(&r.dir.join("data/cameras/recordings").join(&r.id).join(&e.rel), &key).unwrap();
        for i in 0..seg.len() {
            frames.push(seg.frame(i).unwrap().data);
        }
    }
    assert!(!frames.is_empty(), "something was recorded");
    frames
}

#[test]
fn masked_pixels_never_reach_the_recorder_when_the_person_asked_and_pictures_are_stored_as_received_when_not() {
    let r = rig("storage");
    let orig = iyakki_web::jpeg::decode(JPG).unwrap();
    let rec = recorder(&r);
    // 1. no mask: the camera's JPEG bytes are stored exactly as they arrived (no re-encoding)
    let frames = record_a_moment(&r, &rec);
    assert!(frames.iter().all(|f| f == JPG), "stored as received");
    // 2. a mask over the left half, not applied to storage: still as received (the mask is for detection only)
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Mask, "Left", false, 5, &rect(0.0, 0.0, 0.5, 1.0)).unwrap();
    let frames = record_a_moment(&r, &rec);
    assert!(frames.iter().all(|f| f == JPG), "without the box ticked recordings are untouched");
    // 3. "also in recordings and snapshots": the left half is black in every picture stored from now on
    let before = frames.len();
    r.svc.set(&r.amy(), &r.id, &Change { mask_storage: Some(true), ..Default::default() }).unwrap();
    let rec2 = recorder(&r);
    let frames = record_a_moment(&r, &rec2);
    let fresh = &frames[before..];
    assert!(!fresh.is_empty() && fresh.iter().all(|f| f.as_slice() != JPG), "every picture stored after the switch is masked");
    for masked in fresh {
        let (w, h, px) = unpack(masked);
        assert_eq!((w, h), (orig.width, orig.height));
        let mut left_nonblack = 0;
        let mut right_changed = 0;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let o = (y * w as usize + x) * 4;
                if x < w as usize / 2 {
                    if px[o] != 0 || px[o + 1] != 0 || px[o + 2] != 0 {
                        left_nonblack += 1;
                    }
                } else if px[o..o + 3] != orig.rgba[o..o + 3] {
                    right_changed += 1;
                }
            }
        }
        assert_eq!(left_nonblack, 0, "not one masked pixel reached the recorder");
        assert_eq!(right_changed, 0, "the rest is the camera's picture");
    }
    // 4. a picture that cannot be masked is not stored: the recorder stops with a visible message
    let bad = crate::camdetect::mask_jpeg_for_storage(&r.dir.join("data"), &r.id, vec![0xFF, 0xD8, 1, 2, 3], &*r.hub.env().encode);
    assert!(bad.unwrap_err().contains("not stored"));
}

#[test]
fn snapshots_carry_the_masks_too_when_asked_and_a_live_camera_feeds_the_detector_without_anyone_looking() {
    let cam = FakeCamera::rtsp(Config { user: Some(("admin".into(), "pw-12345".into())), digest: false, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() });
    let r = rig_at("live", &format!("rtsp://127.0.0.1:{}/live", cam.port), false);
    r.hub.set_tap(Arc::new(Tap(Arc::downgrade(&r.svc))));
    let orig = iyakki_web::jpeg::decode(JPG).unwrap();
    // always mode keeps the stream up by itself: no one calls view
    r.svc.shape_set(&r.amy(), &r.id, ShapeKind::Mask, "Left", false, 5, &rect(0.0, 0.0, 0.5, 1.0)).unwrap();
    r.svc.set(&r.amy(), &r.id, &Change { mode: Some(WatchMode::Always), mask_storage: Some(true), ..Default::default() }).unwrap();
    // the clock of this test does not move by itself, so the "recent picture" window is the fake clock's: advance it with the pictures
    let t = Instant::now();
    loop {
        r.svc.tick();
        r.now.fetch_add(100, Ordering::SeqCst);
        if r.hub.is_live(&r.id) && r.svc.analysing_ids() == vec![r.id.clone()] {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "the stream never fed the detector; live {}", r.hub.is_live(&r.id));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!r.hub.viewed_recently(&r.id), "nobody asked for a picture: the watcher alone kept it going");
    // a snapshot (the owner presses the button): the left half is black in the saved file
    let amy = r.amy();
    let path = r.hub.snapshot(&amy, &r.id).unwrap();
    // the fake camera's picture came as a JPEG: the stored file is the lossless test container
    let bytes = std::fs::read(&path).unwrap();
    let (w, h, px) = unpack(&bytes);
    assert_eq!((w, h), (orig.width, orig.height));
    let left_dirty = (0..h as usize).flat_map(|y| (0..w as usize / 2).map(move |x| (y * w as usize + x) * 4)).filter(|&o| px[o..o + 3] != [0, 0, 0]).count();
    assert_eq!(left_dirty, 0, "the snapshot has the mask applied");
    assert!(px[(w as usize / 2 + 2) * 4..(w as usize / 2 + 2) * 4 + 3] == orig.rgba[(w as usize / 2 + 2) * 4..(w as usize / 2 + 2) * 4 + 3]);
    // without the box ticked the snapshot is the camera's JPEG as it came
    r.svc.set(&amy, &r.id, &Change { mask_storage: Some(false), ..Default::default() }).unwrap();
    let path2 = r.hub.snapshot(&amy, &r.id).unwrap();
    let plain = std::fs::read(path2).unwrap();
    assert!(plain.starts_with(&[0xFF, 0xD8]) && !plain.starts_with(&[0xFF, 0xD8, b'M', b'K']), "the camera's own JPEG, not re-encoded by the mask route");
    let d = iyakki_web::jpeg::decode(&plain).unwrap();
    assert!((0..d.height as usize).flat_map(|y| (0..d.width as usize / 2).map(move |x| (y * d.width as usize + x) * 4)).any(|o| d.rgba[o..o + 3] != [0, 0, 0]), "the left half is visible when the box is not ticked");
    let _ = r.saved.lock().unwrap().len();
}

#[test]
fn the_thumbnail_of_an_event_can_be_opened_by_the_owner_and_nobody_else() {
    let r = rig_at("thumb", "rtsp://127.0.0.1:9/live", true);
    r.on(WatchMode::Always);
    r.hold(&frame(None), 20);
    r.walk(60, 10);
    let ev = r.events();
    assert_eq!(ev.len(), 1);
    let (w, h, rgba) = r.svc.thumb(&r.amy(), &r.id, ev[0].id).unwrap().expect("a thumbnail");
    assert_eq!((w, h), (48, 32));
    assert_eq!(rgba.len(), 48 * 32 * 4);
    assert!(r.svc.thumb(&r.amy(), &r.id, 12345).unwrap().is_none());
    assert!(r.svc.thumb(&who(&r, "bob", false), &r.id, ev[0].id).is_err());
    assert!(r.svc.thumb(&who(&r, "root", true), &r.id, ev[0].id).is_err());
}

#[test]
fn settings_are_checked_before_they_are_kept() {
    let r = rig("settings");
    let amy = r.amy();
    assert!(r.svc.set(&amy, &r.id, &Change { sensitivity: Some(0), ..Default::default() }).is_err());
    assert!(r.svc.set(&amy, &r.id, &Change { sensitivity: Some(11), ..Default::default() }).is_err());
    assert!(r.svc.set(&amy, &r.id, &Change { alerts: Some(AlertChange { max_per_hour: Some(0), ..Default::default() }), ..Default::default() }).is_err());
    assert!(r.svc.set(&amy, &r.id, &Change { alerts: Some(AlertChange { max_per_hour: Some(31), ..Default::default() }), ..Default::default() }).is_err());
    assert!(r.svc.set(&amy, &r.id, &Change { alerts: Some(AlertChange { group_s: Some(1), ..Default::default() }), ..Default::default() }).is_err());
    let ok = r.svc.set(&amy, &r.id, &Change { sensitivity: Some(8), alerts: Some(AlertChange { max_per_hour: Some(3), quiet: Some("22:00-06:30".into()), ..Default::default() }), ..Default::default() }).unwrap();
    assert_eq!((ok.sensitivity, ok.alerts.max_per_hour, ok.alerts.quiet.as_str(), ok.mode), (8, 3, "22:00-06:30", WatchMode::Off));
    // an empty quiet text removes the quiet hours
    let ok = r.svc.set(&amy, &r.id, &Change { alerts: Some(AlertChange { quiet: Some(String::new()), ..Default::default() }), ..Default::default() }).unwrap();
    assert!(ok.alerts.quiet.is_empty());
    // a locked vault means watching cannot be turned on (it holds the key) and nothing half-happens
    r.vault.locked.store(true, Ordering::SeqCst);
    assert!(r.svc.set(&amy, &r.id, &Change { mode: Some(WatchMode::Always), ..Default::default() }).is_err());
    r.vault.locked.store(false, Ordering::SeqCst);
    assert_eq!(r.svc.status(&amy, &r.id).unwrap().settings.mode, WatchMode::Off);
    // day ranges
    assert!(r.svc.day_range("2026-11-08").is_ok() && r.svc.day_range("yesterday").is_err());
}
