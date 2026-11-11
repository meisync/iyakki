//! Recording against scripted cameras and the in-house fake RTSP camera on the loopback address. No real camera, no long run, no
//! measurement of disk speed.

use super::*;
use crate::cameras::{Env, NoH264, NoUsb, NewSpec};
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use std::sync::atomic::{AtomicU32, AtomicU64};

const JPG: &[u8] = include_bytes!("../../iyakki-rtsp/tests/data/test-48x32.jpg");
const PASSWORD: &str = "hunter2-cam-pass";

#[derive(Default)]
struct Vault {
    items: Mutex<HashMap<(String, String), String>>,
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

/// Pictures that differ from each other: the 48x32 test JPEG with a comment segment (valid, and ignored by decoders) after the start.
fn pic(i: usize) -> Vec<u8> {
    let mut v = JPG[..2].to_vec();
    let n = 10 + (i * 3) % 190;
    let c: Vec<u8> = (0..n).map(|k| ((k * 31 + i * 7) % 251) as u8).collect();
    v.extend([0xFF, 0xFE]);
    v.extend(((n + 2) as u16).to_be_bytes());
    v.extend(c);
    v.extend(&JPG[2..]);
    v
}

struct Scripted {
    interval: Duration,
    down: AtomicBool,
    opens: AtomicU32,
    h264: bool,
}

struct ScriptedFeed {
    n: usize,
    interval: Duration,
    h264: bool,
}

impl RecFeed for ScriptedFeed {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        std::thread::sleep(self.interval);
        self.n += 1;
        if self.h264 {
            let key = self.n % 5 == 1;
            let mut au = vec![0, 0, 0, 1, if key { 0x65 } else { 0x41 }];
            au.extend((0..200).map(|k| (k + self.n) as u8));
            return Ok(Unit::H264 { annex_b: au, key });
        }
        Ok(Unit::Jpeg(pic(self.n)))
    }
    fn kind(&self) -> RecKind {
        if self.h264 { RecKind::H264 } else { RecKind::Jpeg }
    }
    fn params(&self) -> Vec<u8> {
        if self.h264 { vec![0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5] } else { vec![] }
    }
}

impl RecSource for Scripted {
    fn open(&self, _: &Spec) -> Result<Box<dyn RecFeed>, FeedErr> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(FeedErr::Lost("no route to host".into()));
        }
        Ok(Box::new(ScriptedFeed { n: 0, interval: self.interval, h264: self.h264 }))
    }
}

struct Rig {
    rec: Arc<Recorder>,
    hub: Arc<Hub>,
    vault: Arc<Vault>,
    audit: Arc<Mutex<Vec<String>>>,
    active: Arc<Mutex<Vec<Vec<String>>>>,
    free_mb: Arc<AtomicU64>,
    now: Arc<AtomicU64>,
    saved: Arc<Mutex<Vec<PathBuf>>>,
    script: Arc<Scripted>,
    dir: PathBuf,
}

fn who(name: &str, manage: bool, dir: &Path) -> Who {
    let home = dir.join("home").join(name);
    Who { user: name.into(), manage, usb_ok: false, home, may_write: Arc::new(|_| true) }
}

fn rig_with(tag: &str, source: Option<Arc<dyn RecSource>>, h264: bool) -> Rig {
    let dir = std::fs::canonicalize(crate::testdir::unique("camrec", tag)).unwrap();
    let vault = Arc::new(Vault::default());
    let audit = Arc::new(Mutex::new(vec![]));
    let a2 = audit.clone();
    let saved = Arc::new(Mutex::new(vec![]));
    let s2 = saved.clone();
    let hub = Arc::new(Hub::new(Env {
        data: dir.join("data"),
        secrets: vault.clone(),
        people: Arc::new(|| vec!["amy".into(), "bob".into(), "root".into(), "eve".into()]),
        usb: Arc::new(NoUsb),
        h264: Arc::new(NoH264),
        encode: Arc::new(|_, _, _| vec![0xFF, 0xD8, 0, 0, 0]),
        after_save: Arc::new(move |p| s2.lock().unwrap().push(p.to_path_buf())),
        audit: Arc::new(move |u, e, d| a2.lock().unwrap().push(format!("{u}|{e}|{d}"))),
        backoff_ms: 30,
        idle: Duration::from_secs(5),
    }));
    let script = Arc::new(Scripted { interval: Duration::from_millis(8), down: AtomicBool::new(false), opens: AtomicU32::new(0), h264 });
    let free_mb = Arc::new(AtomicU64::new(500_000));
    let now = Arc::new(AtomicU64::new(0));
    let active = Arc::new(Mutex::new(vec![]));
    let (f2, n2, ac2) = (free_mb.clone(), now.clone(), active.clone());
    let src: Arc<dyn RecSource> = source.unwrap_or_else(|| script.clone());
    let env = RecEnv {
        source: src,
        free_space: Arc::new(move |_| Some(f2.load(Ordering::SeqCst) * MIB)),
        now_ms: Arc::new(move || {
            let n = n2.load(Ordering::SeqCst);
            if n == 0 { system_ms() } else { n }
        }),
        zone: Arc::new(Zone::utc),
        on_active: Arc::new(move |v| ac2.lock().unwrap().push(v.to_vec())),
        segment_ms: 150,
        retention_every: Duration::from_secs(3600),
        backoff_ms: 20,
        retry_after: Duration::from_millis(60),
    };
    let rec = Arc::new(Recorder::new(hub.clone(), env));
    Rig { rec, hub, vault, audit, active, free_mb, now, saved, script, dir }
}

fn rig(tag: &str) -> Rig {
    rig_with(tag, None, false)
}

impl Rig {
    fn add(&self, name: &str, owner: &str, allowed: &[&str]) -> String {
        let w = who(owner, false, &self.dir);
        self.hub.save(&w, None, &NewSpec { name: name.into(), address: "rtsp://127.0.0.1:9/live".into(), user: "admin".into(), allowed: allowed.iter().map(|s| s.to_string()).collect() }, Some(PASSWORD)).unwrap()
    }
    fn amy(&self) -> Who {
        who("amy", false, &self.dir)
    }
    fn key(&self, cam: &str) -> RecKey {
        RecKey::from_hex(&self.vault.get("amy", &key_name(cam)).unwrap()).unwrap()
    }
    /// Make the camera's key without starting a recorder (for tests that lay files down themselves).
    fn prep(&self, cam: &str) {
        let spec = self.hub.specs().into_iter().find(|s| s.id == cam).unwrap();
        self.rec.ensure_key(&spec).unwrap();
    }
    fn on(&self, cam: &str) {
        self.rec.set(&self.amy(), cam, &Change { mode: Some(Mode::Continuous), ..Default::default() }).unwrap();
    }
    fn off(&self, cam: &str) {
        self.rec.set(&self.amy(), cam, &Change { mode: Some(Mode::Off), ..Default::default() }).unwrap();
    }
    /// Write a finished segment of `n` pictures starting at `start_ms` straight into the folder, as the recorder would have.
    fn fab(&self, cam: &str, start_ms: u64, n: usize, size: usize) -> PathBuf {
        let l = Zone::utc().local((start_ms / 1000) as i64);
        let path = self.rec.cam_dir(cam).join(format!("{:04}/{:02}/{:02}/{:02}{:02}{:02}.irec", l.year, l.month, l.day, l.hour, l.minute, l.second));
        let mut w = SegmentWriter::create(&path, &self.key(cam), RecKind::Jpeg, start_ms).unwrap();
        for i in 0..n {
            let mut p = pic(i + (start_ms / 1000) as usize);
            p.resize(size.max(p.len()), 7);
            w.append(start_ms + i as u64 * 200, fmt::F_KEY, &p).unwrap();
        }
        path
    }
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

const DAY: u64 = 86_400_000;
const T0: u64 = 1_800_000_000_000; // a fixed day in 2027, UTC

// ------------------------------------------------------------------------------------------------ opt-in and the indicator

#[test]
fn nothing_records_until_the_owner_turns_it_on_and_the_sign_follows_the_recorder() {
    let r = rig("optin");
    let id = r.add("Front door", "amy", &["bob"]);
    r.rec.tick();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!r.rec.recording(&id) && r.rec.recording_names().is_empty());
    assert!(!r.rec.cam_dir(&id).join("settings.json").exists());
    assert_eq!(r.script.opens.load(Ordering::SeqCst), 0, "the camera was not even contacted");
    assert_eq!(r.rec.settings_of(&id).mode, Mode::Off);
    assert!(!r.vault.has("amy", &key_name(&id)), "no key is made before recording is switched on");

    r.on(&id);
    assert!(r.rec.recording(&id));
    assert_eq!(r.rec.recording_names(), vec!["Front door".to_string()]);
    assert!(wait_for(5, || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Recording { frames, .. } if frames > 3)));
    assert_eq!(r.active.lock().unwrap().last().unwrap(), &vec!["Front door".to_string()], "the shell's red sign was told");
    // one call stops it
    r.off(&id);
    assert!(!r.rec.recording(&id) && r.rec.recording_names().is_empty());
    assert_eq!(r.active.lock().unwrap().last().unwrap(), &Vec::<String>::new());
    let st = r.rec.status(&r.amy(), &id).unwrap();
    assert_eq!(st.state, RecState::Off);
    assert!(st.segments >= 1, "what was recorded is kept");
    assert!(r.audit.lock().unwrap().iter().any(|l| l.contains("camera.recording.start")) && r.audit.lock().unwrap().iter().any(|l| l.contains("camera.recording.stop")));
    r.rec.shutdown();
}

#[test]
fn only_the_owner_turns_recording_on_an_administrator_may_turn_it_off_and_others_cannot_touch_it() {
    let r = rig("rights");
    let id = r.add("Garage", "amy", &["bob"]);
    let on = Change { mode: Some(Mode::Continuous), ..Default::default() };
    for (name, manage) in [("bob", false), ("root", true), ("eve", false)] {
        assert!(r.rec.set(&who(name, manage, &r.dir), &id, &on).is_err(), "{name} must not turn it on");
    }
    assert_eq!(r.rec.settings_of(&id).mode, Mode::Off);
    r.on(&id);
    let off = Change { mode: Some(Mode::Off), ..Default::default() };
    assert!(r.rec.set(&who("bob", false, &r.dir), &id, &off).is_err(), "a person it is shared with cannot switch it off");
    assert!(r.rec.set(&who("eve", false, &r.dir), &id, &off).is_err());
    r.rec.set(&who("root", true, &r.dir), &id, &off).unwrap();
    assert_eq!(r.rec.settings_of(&id).mode, Mode::Off);
    // the refusals are in the camera's log
    let log = r.rec.audit(&r.amy(), &id, 50).unwrap();
    assert!(log.iter().any(|l| l.who == "bob" && !l.ok && l.ev == "recording.setting"));
    assert!(log.iter().any(|l| l.who == "root" && l.ev == "recording.setting" && l.ok), "the administrator's switch-off is logged");
    // a camera that does not exist, and a camera plugged into this computer
    assert!(r.rec.set(&r.amy(), "nope", &on).is_err());
    assert!(r.rec.set(&r.amy(), "usb:video0", &on).unwrap_err().contains("network cameras"));
    // a schedule needs a window
    assert!(r.rec.set(&r.amy(), &id, &Change { mode: Some(Mode::Schedule), ..Default::default() }).is_err());
    r.rec.shutdown();
}

#[test]
fn a_locked_vault_means_no_key_so_recording_cannot_be_turned_on_and_a_running_recorder_waits() {
    let r = rig("locked");
    let id = r.add("Porch", "amy", &[]);
    r.vault.locked.store(true, Ordering::SeqCst);
    let e = r.rec.set(&r.amy(), &id, &Change { mode: Some(Mode::Continuous), ..Default::default() }).unwrap_err();
    assert!(e.contains("vault"), "{e}");
    assert_eq!(r.rec.settings_of(&id).mode, Mode::Off);
    r.vault.locked.store(false, Ordering::SeqCst);
    r.on(&id);
    assert!(wait_for(5, || r.rec.status(&r.amy(), &id).unwrap().segments >= 1 || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Recording { frames, .. } if frames > 2)));
    // the owner signs out: the vault closes, the recorder stops writing and says why
    r.vault.locked.store(true, Ordering::SeqCst);
    assert!(wait_for(5, || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Waiting(ref m) if m.contains("vault"))), "{:?}", r.rec.status(&r.amy(), &id).unwrap().state);
    let before = r.rec.entries(&id).len();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.rec.entries(&id).len(), before, "nothing new while the vault is locked");
    // and playback is refused with the honest reason
    let e = r.rec.playback_frame(&r.amy(), &id, 0, Seek::AtOrAfter).unwrap_err();
    assert!(e.contains("vault") && e.contains("cannot be opened"), "{e}");
    r.vault.locked.store(false, Ordering::SeqCst);
    assert!(wait_for(5, || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Recording { .. })), "recording continues after the vault opens");
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ segments, encryption

#[test]
fn frames_are_stored_exactly_as_received_in_encrypted_files_in_the_day_folders() {
    let r = rig("store");
    let id = r.add("Front door", "amy", &[]);
    r.on(&id);
    assert!(wait_for(5, || r.rec.status(&r.amy(), &id).unwrap().segments >= 2));
    r.off(&id);
    let entries = r.rec.entries(&id);
    assert!(entries.len() >= 2, "segments rotate on time");
    let key = r.key(&id);
    let mut got = 0;
    for e in &entries {
        // YYYY/MM/DD/HHMMSS.irec under the camera's folder under recordings
        let parts: Vec<&str> = e.rel.split('/').collect();
        assert_eq!(parts.len(), 4, "{}", e.rel);
        assert!(parts[0].len() == 4 && parts[1].len() == 2 && parts[2].len() == 3 - 1 && parts[3].ends_with(".irec"));
        let path = r.rec.cam_dir(&id).join(&e.rel);
        assert!(path.starts_with(r.dir.join("data/cameras/recordings")));
        let seg = Segment::open(&path, &key).unwrap();
        assert_eq!(seg.len() as u32, e.frames);
        for k in 0..seg.len() {
            let f = seg.frame(k).unwrap();
            // the scripted camera's pictures are `pic(n)`; stored byte for byte, no re-encoding
            assert!(f.data.starts_with(&[0xFF, 0xD8]) && f.data.ends_with(&[0xFF, 0xD9]));
            assert!((1..400).any(|n| pic(n) == f.data), "a stored picture is not one the camera sent");
            got += 1;
        }
        // nothing readable on the disk
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(4).any(|w| w == [0xFF, 0xD8, 0xFF, 0xFE]));
    }
    assert!(got > 10);
    // the folder holds only encrypted files, an index, the log
    let names: Vec<String> = std::fs::read_dir(r.rec.cam_dir(&id)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| n.len() == 4 || n == "index.txt" || n == "audit.log"), "{names:?}");
    r.rec.shutdown();
}

#[test]
fn the_real_rtsp_client_records_a_fake_camera_and_the_pictures_decode() {
    let cam = FakeCamera::rtsp(Config { user: Some(("admin".into(), PASSWORD.into())), digest: true, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() });
    let r = rig("rtsp");
    let src: Arc<dyn RecSource> = Arc::new(NetSource(r.vault.clone()));
    let r = Rig { rec: Arc::new(Recorder::new(r.hub.clone(), RecEnv { source: src, free_space: Arc::new(|_| Some(500_000 * MIB)), now_ms: Arc::new(system_ms), zone: Arc::new(Zone::utc), on_active: Arc::new(|_| {}), segment_ms: 200, retention_every: Duration::from_secs(3600), backoff_ms: 20, retry_after: Duration::from_millis(60) })), ..r };
    let w = r.amy();
    let id = r.hub.save(&w, None, &NewSpec { name: "Fake".into(), address: format!("rtsp://127.0.0.1:{}/live", cam.port), user: "admin".into(), allowed: vec![] }, Some(PASSWORD)).unwrap();
    r.on(&id);
    assert!(wait_for(10, || r.rec.status(&w, &id).unwrap().segments >= 1));
    r.off(&id);
    let pf = r.rec.playback_frame(&w, &id, 0, Seek::AtOrAfter).unwrap().expect("a picture");
    assert_eq!((pf.width, pf.height), (48, 32));
    assert!(pf.rgba.len() == 48 * 32 * 4);
    let log = r.rec.audit(&w, &id, 100).unwrap();
    let text = serde_json::to_string(&log).unwrap();
    assert!(!text.contains(PASSWORD), "the camera's password is in the log");
    r.rec.shutdown();
}

#[test]
fn h264_is_stored_as_annex_b_with_parameter_sets_first_and_playback_says_it_needs_the_add_on() {
    let r = rig_with("h264", None, true);
    let id = r.add("Yard", "amy", &[]);
    r.on(&id);
    assert!(wait_for(5, || r.rec.status(&r.amy(), &id).unwrap().segments >= 2));
    r.off(&id);
    let e = &r.rec.entries(&id)[0];
    assert_eq!(e.kind, RecKind::H264);
    let seg = Segment::open(&r.rec.cam_dir(&id).join(&e.rel), &r.key(&id)).unwrap();
    let first = seg.frame(0).unwrap();
    assert_eq!(first.flags & fmt::F_PARAMS, fmt::F_PARAMS, "every segment starts with the parameter sets");
    assert!(first.data.starts_with(&[0, 0, 0, 1, 0x67]));
    let second = seg.frame(1).unwrap();
    assert!(second.flags & fmt::F_KEY != 0 && second.data.starts_with(&[0, 0, 0, 1, 0x65]), "a segment starts at a key frame");
    let err = r.rec.playback_frame(&r.amy(), &id, 0, Seek::AtOrAfter).unwrap_err();
    assert!(err.contains("add-on") && err.contains("export"), "{err}");
    // export works without the add-on: one Annex-B file
    let out = r.rec.export(&r.amy(), &id, 0, system_ms() + 3_600_000, true).unwrap();
    assert_eq!(out.kind, RecKind::H264);
    let bytes = std::fs::read(&out.path).unwrap();
    assert!(bytes.starts_with(&[0, 0, 0, 1, 0x67]) && bytes.windows(5).any(|w| w == [0, 0, 0, 1, 0x65]));
    assert!(out.note.contains("player"));
    r.rec.shutdown();
}

#[test]
fn losing_the_key_makes_recordings_unreadable_and_a_different_key_does_not_help() {
    let r = rig("nokey");
    let id = r.add("Hall", "amy", &[]);
    r.on(&id);
    assert!(wait_for(5, || r.rec.status(&r.amy(), &id).unwrap().segments >= 1));
    r.off(&id);
    assert!(r.rec.playback_frame(&r.amy(), &id, 0, Seek::AtOrAfter).unwrap().is_some());
    // the vault entry is gone (a lost vault)
    r.vault.delete("amy", &key_name(&id)).unwrap();
    let e = r.rec.playback_frame(&r.amy(), &id, 0, Seek::AtOrAfter);
    assert!(e.is_err());
    let e = r.rec.export(&r.amy(), &id, 0, system_ms() + 3_600_000, true).unwrap_err();
    assert!(e.contains("key") && e.contains("cannot be opened"), "{e}");
    // somebody makes a new key: still nothing opens
    r.vault.set("amy", &key_name(&id), &RecKey::generate().unwrap().to_hex()).unwrap();
    let e = r.rec.playback_frame(&r.amy(), &id, 0, Seek::AtOrAfter).unwrap_err();
    assert!(e.contains("key does not fit"), "{e}");
    assert!(KEY_NOTE.contains("cannot be opened"));
    assert!(r.rec.status(&r.amy(), &id).unwrap().key_note.contains("lose your vault"));
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ retention

#[test]
fn old_recordings_are_deleted_by_age_oldest_first_and_empty_folders_go() {
    let r = rig("age");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    r.now.store(T0, Ordering::SeqCst);
    let old = r.fab(&id, T0 - 20 * DAY, 3, 500);
    let mid = r.fab(&id, T0 - 13 * DAY, 3, 500);
    let new = r.fab(&id, T0 - DAY, 3, 500);
    r.rec.rebuild_index(&id);
    assert_eq!(r.rec.entries(&id).len(), 3);
    let rep = r.rec.enforce();
    assert!(!old.exists() && mid.exists() && new.exists(), "14 days by default: only the 20-day-old file goes");
    assert_eq!(rep.deleted.len(), 1);
    assert_eq!(rep.deleted[0].1, 1);
    assert!(!old.parent().unwrap().exists(), "the empty day folder is removed");
    r.rec.set(&r.amy(), &id, &Change { max_days: Some(3), ..Default::default() }).unwrap();
    r.rec.enforce();
    assert!(!mid.exists() && new.exists());
    assert_eq!(r.rec.entries(&id).len(), 1);
    // max_days 0 keeps everything by age
    r.rec.set(&r.amy(), &id, &Change { max_days: Some(0), ..Default::default() }).unwrap();
    r.fab(&id, T0 - 300 * DAY, 2, 100);
    r.rec.rebuild_index(&id);
    r.rec.enforce();
    assert_eq!(r.rec.entries(&id).len(), 2);
    // and the deletions are audited in the camera's log and the general one
    let log = r.rec.audit(&r.amy(), &id, 100).unwrap();
    assert!(log.iter().any(|l| l.ev == "retention.delete" && l.detail.contains("older than 14 days")));
    assert!(r.audit.lock().unwrap().iter().any(|l| l.contains("camera.retention")));
    r.rec.shutdown();
}

#[test]
fn a_camera_over_its_size_limit_and_all_cameras_over_the_total_lose_their_oldest_files_first() {
    let r = rig("size");
    let a = r.add("A", "amy", &[]);
    let b = r.add("B", "amy", &[]);
    for c in [&a, &b] {
        r.prep(c);
    }
    r.now.store(T0, Ordering::SeqCst);
    // 4 files of about 1.2 MiB for A, 2 for B
    let mut a_files = vec![];
    for i in 0..4 {
        a_files.push(r.fab(&a, T0 - (10 - i) * 3_600_000, 6, 200_000));
    }
    let b_files = [r.fab(&b, T0 - 9 * 3_600_000 - 1_800_000, 6, 200_000), r.fab(&b, T0 - 3_600_000, 6, 200_000)];
    r.rec.rebuild_index(&a);
    r.rec.rebuild_index(&b);
    r.rec.set(&r.amy(), &a, &Change { max_mb: Some(3), ..Default::default() }).unwrap();
    r.rec.enforce();
    let left: Vec<bool> = a_files.iter().map(|p| p.exists()).collect();
    assert_eq!(left, vec![false, false, true, true], "oldest first, until the camera is under 3 MB");
    assert!(b_files.iter().all(|p| p.exists()));
    // the total: allow 4 MB for everything (A has 2.4, B 2.4)
    r.rec.set_policy(&who("root", true, &r.dir), Some(4), None).unwrap();
    r.rec.enforce();
    let total: u64 = [&a, &b].iter().flat_map(|c| r.rec.entries(c)).map(|e| e.bytes).sum();
    assert!(total <= 4 * MIB, "{total}");
    assert!(!b_files[0].exists(), "the oldest file of all (B's) goes before A's newer ones");
    assert!(b_files[1].exists() || a_files[3].exists());
    // only an administrator changes the total limit
    assert!(r.rec.set_policy(&r.amy(), Some(1), None).is_err());
    // the floor cannot be set below the minimum
    assert!(r.rec.set_policy(&who("root", true, &r.dir), None, Some(10)).is_err());
    r.rec.shutdown();
}

#[test]
fn below_the_free_space_floor_recording_stops_with_a_visible_message_and_resumes_when_there_is_room() {
    let r = rig("floor");
    let id = r.add("Cam", "amy", &[]);
    r.free_mb.store(100, Ordering::SeqCst); // the default floor is 5000 MB
    r.on(&id);
    assert!(!r.rec.recording(&id), "it does not start");
    let st = r.rec.status(&r.amy(), &id).unwrap();
    assert!(matches!(&st.state, RecState::Problem(m) if m.contains("100 MB free") && m.contains("recording stopped")), "{:?}", st.state);
    assert_eq!(r.script.opens.load(Ordering::SeqCst), 0);
    assert!(r.rec.entries(&id).is_empty());
    assert!(r.rec.audit(&r.amy(), &id, 20).unwrap().iter().any(|l| l.ev == "recording.stop" && !l.ok));
    // room again
    r.free_mb.store(500_000, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    r.rec.tick();
    assert!(r.rec.recording(&id), "it starts again by itself");
    // the disk fills while recording: the worker stops itself
    r.free_mb.store(10, Ordering::SeqCst);
    // the worker sets the Problem state and then ends; the next tick takes it off the list, so wait for both
    assert!(wait_for(10, || {
        r.rec.tick();
        matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Problem(_)) && !r.rec.recording(&id)
    }), "{:?}", r.rec.status(&r.amy(), &id).unwrap().state);
    assert!(!r.rec.recording(&id));
    let n = r.rec.entries(&id).len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.rec.entries(&id).len(), n);
    r.rec.shutdown();
}

#[test]
fn a_floor_pass_deletes_nothing_of_the_owners_recordings_that_is_within_their_limits() {
    let r = rig("floor-keeps");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    r.now.store(T0, Ordering::SeqCst);
    let f = r.fab(&id, T0 - 3_600_000, 3, 1000);
    r.rec.rebuild_index(&id);
    r.free_mb.store(10, Ordering::SeqCst);
    r.rec.enforce();
    assert!(f.exists(), "a full disk stops recording; it never deletes within the limits");
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ crash safety and the index

#[test]
fn after_a_power_cut_the_earlier_files_and_the_cut_one_still_play_and_the_index_is_rebuilt() {
    let r = rig("crash");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    r.now.store(T0, Ordering::SeqCst);
    let first = r.fab(&id, T0 - 3_600_000, 5, 400);
    let second = r.fab(&id, T0 - 1_800_000, 8, 400);
    // the power goes in the middle of the last record of the second file; the index never saw the file
    let full = std::fs::read(&second).unwrap();
    std::fs::write(&second, &full[..full.len() - 100]).unwrap();
    let _ = std::fs::remove_file(r.rec.cam_dir(&id).join("index.txt"));
    // a fresh recorder (a restart) finds both
    let again = Recorder::new(r.hub.clone(), RecEnv { source: r.script.clone(), free_space: Arc::new(|_| Some(500_000 * MIB)), now_ms: Arc::new(|| T0), zone: Arc::new(Zone::utc), on_active: Arc::new(|_| {}), segment_ms: 150, retention_every: Duration::from_secs(3600), backoff_ms: 20, retry_after: Duration::from_millis(60) });
    let e = again.entries(&id);
    assert_eq!(e.len(), 2);
    assert_eq!((e[0].frames, e[1].frames), (5, 7), "the cut record is lost, nothing before it");
    assert!(first.exists());
    assert!(r.rec.cam_dir(&id).join("index.txt").exists(), "the index was written again");
    let f = again.playback_frame(&r.amy(), &id, T0 - 1_800_000, Seek::AtOrAfter).unwrap().unwrap();
    assert_eq!(f.ts_ms, T0 - 1_800_000);
    // a file with a header and no picture (power cut right at the start) is cleaned away
    let empty = r.rec.cam_dir(&id).join("2027/01/01/000000.irec");
    drop(SegmentWriter::create(&empty, &r.key(&id), RecKind::Jpeg, 1).unwrap());
    assert_eq!(again.rebuild_index(&id), 2);
    assert!(!empty.exists());
    r.rec.shutdown();
}

#[test]
fn the_index_can_be_thrown_away_and_comes_back_identical_and_a_garbled_index_is_not_trusted() {
    let r = rig("index");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    for i in 0..4 {
        r.fab(&id, T0 + i * 90_000, 4, 300);
    }
    let n = r.rec.rebuild_index(&id);
    assert_eq!(n, 4);
    let before = r.rec.entries(&id);
    let text = std::fs::read_to_string(r.rec.cam_dir(&id).join("index.txt")).unwrap();
    assert_eq!(text.lines().count(), 4);
    // a garbled index (and a line that tries to leave the folder) is ignored: the files are read again
    std::fs::write(r.rec.cam_dir(&id).join("index.txt"), "garbage\n../../etc/passwd\t1\t2\t3\t4\t1\n").unwrap();
    r.rec.idx.lock().unwrap().remove(&id);
    assert_eq!(r.rec.entries(&id), before);
    // a changed file (size differs) is read again, not trusted
    let p = r.rec.cam_dir(&id).join(&before[0].rel);
    let mut b = std::fs::read(&p).unwrap();
    b.truncate(b.len() - 50);
    std::fs::write(&p, b).unwrap();
    r.rec.idx.lock().unwrap().remove(&id);
    assert_eq!(r.rec.entries(&id)[0].frames, before[0].frames - 1);
    r.rec.shutdown();
}

#[test]
fn a_flipped_byte_in_a_stored_picture_is_reported_and_not_shown() {
    let r = rig("tamper");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    r.now.store(T0, Ordering::SeqCst);
    let p = r.fab(&id, T0, 6, 3000);
    r.rec.rebuild_index(&id);
    let mut b = std::fs::read(&p).unwrap();
    let sc = fmt::scan(&p).unwrap();
    b[sc.frames[2].offset as usize + 100] ^= 0x10;
    std::fs::write(&p, b).unwrap();
    let e = r.rec.playback_frame(&r.amy(), &id, T0 + 400, Seek::AtOrAfter).unwrap_err();
    assert!(e.contains("picture 3") && e.contains("changed or damaged"), "{e}");
    assert!(r.rec.audit(&r.amy(), &id, 20).unwrap().iter().any(|l| l.ev == "playback.error" && !l.ok));
    // the neighbours still show
    assert!(r.rec.playback_frame(&r.amy(), &id, T0 + 600, Seek::AtOrAfter).is_ok());
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ playback and export

#[test]
fn the_timeline_lists_days_and_stepping_walks_across_files() {
    let r = rig("play");
    let id = r.add("Cam", "amy", &[]);
    r.prep(&id);
    r.now.store(T0 + 3 * DAY, Ordering::SeqCst);
    // two files on one day, 1 s apart (one stretch), one a day later
    r.fab(&id, T0, 5, 800); // frames at +0 ... +800 ms
    r.fab(&id, T0 + 1_000, 5, 800);
    r.fab(&id, T0 + DAY + 10_000, 3, 800);
    r.rec.rebuild_index(&id);
    let days = r.rec.days(&r.amy(), &id).unwrap();
    assert_eq!(days.len(), 2);
    assert!(days[0].label > days[1].label, "newest first");
    assert_eq!(days[1].spans, vec![(T0, T0 + 1_800)], "two files close together are one stretch");
    assert_eq!(days[1].frames, 10);
    assert_eq!(days[1].day_start_ms % DAY, 0);
    // first picture of the first day
    let f = r.rec.playback_frame(&r.amy(), &id, days[1].day_start_ms, Seek::AtOrAfter).unwrap().unwrap();
    assert_eq!((f.ts_ms, f.prev_ms.is_none(), f.next_ms), (T0, true, Some(T0 + 200)));
    assert!(f.label.ends_with(&r.rec.clock_label(T0)) && f.label.starts_with(&days[1].label));
    // step forward through the files, then across to the next day
    let mut at = f.ts_ms;
    let mut seen = vec![at];
    while let Some(n) = r.rec.playback_frame(&r.amy(), &id, at, Seek::After).unwrap() {
        assert!(n.ts_ms > at);
        at = n.ts_ms;
        seen.push(at);
    }
    assert_eq!(seen.len(), 13);
    assert_eq!(*seen.last().unwrap(), T0 + DAY + 10_000 + 400);
    // and back
    let b = r.rec.playback_frame(&r.amy(), &id, at, Seek::Before).unwrap().unwrap();
    assert_eq!(b.ts_ms, T0 + DAY + 10_000 + 200);
    let across = r.rec.playback_frame(&r.amy(), &id, T0 + DAY + 10_000, Seek::Before).unwrap().unwrap();
    assert_eq!(across.ts_ms, T0 + 1_800, "stepping back crosses into the earlier day");
    assert!(r.rec.playback_frame(&r.amy(), &id, T0, Seek::Before).unwrap().is_none());
    // decoded pictures need real JPEGs; the fabricated ones are not, and that is reported rather than shown
    assert!(r.rec.playback_frame(&r.amy(), &id, T0, Seek::AtOrAfter).unwrap().is_some());
    r.rec.shutdown();
}

#[test]
fn only_the_owner_plays_exports_lists_or_reads_the_log_and_every_refusal_is_logged() {
    let r = rig("access");
    let id = r.add("Cam", "amy", &["bob"]);
    r.prep(&id);
    r.fab(&id, T0, 3, 500);
    r.rec.rebuild_index(&id);
    for (name, manage) in [("bob", false), ("root", true), ("eve", false)] {
        let w = who(name, manage, &r.dir);
        assert!(r.rec.days(&w, &id).is_err(), "{name}");
        assert!(r.rec.playback_frame(&w, &id, T0, Seek::AtOrAfter).is_err(), "{name}");
        assert!(r.rec.play_start(&w, &id, T0).is_err(), "{name}");
        assert!(r.rec.export(&w, &id, T0, T0 + 1000, true).is_err(), "{name}");
        assert!(r.rec.audit(&w, &id, 10).is_err(), "{name}");
        assert!(!w.home.join("Pictures").exists(), "{name}: nothing was written");
    }
    // the person it is shared with can see THAT it records, and the camera's state; not the owner's data
    let st = r.rec.status(&who("bob", false, &r.dir), &id).unwrap();
    assert!(!st.owner && st.segments == 0 && st.bytes == 0);
    // a stranger does not even learn the camera exists
    assert!(r.rec.status(&who("eve", false, &r.dir), &id).is_err());
    let log = r.rec.audit(&r.amy(), &id, 100).unwrap();
    for (n, ev) in [("bob", "playback.frame"), ("bob", "export"), ("root", "export"), ("root", "audit.read")] {
        assert!(log.iter().any(|l| l.who == n && l.ev == ev && !l.ok && l.detail.contains("not the owner")), "{n} {ev}");
    }
    assert!(!log.iter().any(|l| l.who == "eve"), "a stranger does not even get a line: the camera is not theirs to know");
    r.rec.shutdown();
}

#[test]
fn an_export_needs_a_confirmation_writes_decrypted_files_privately_and_is_logged() {
    let r = rig("export");
    let id = r.add("Front door", "amy", &[]);
    r.prep(&id);
    r.now.store(T0, Ordering::SeqCst);
    let sent: Vec<Vec<u8>> = (0..6).map(|i| pic(i + (T0 / 1000) as usize)).collect();
    r.fab(&id, T0, 6, 0);
    r.rec.rebuild_index(&id);
    let w = r.amy();
    let e = r.rec.export(&w, &id, T0, T0 + 5_000, false).unwrap_err();
    assert!(e.contains("not confirmed"));
    assert!(!w.home.join("Pictures").exists(), "no confirmation, no file");
    assert!(r.rec.export(&w, &id, T0 + 5_000, T0, true).is_err(), "end before start");
    assert!(r.rec.export(&w, &id, T0 + DAY, T0 + DAY + 5_000, true).unwrap_err().contains("nothing was recorded"));
    // frames at T0 + 0, 200 ... 1000: ask for the middle four
    let out = r.rec.export(&w, &id, T0 + 200, T0 + 800, true).unwrap();
    assert_eq!(out.frames, 4);
    assert!(out.path.starts_with(w.home.join("Pictures")) && out.path.file_name().unwrap().to_string_lossy().starts_with("camera-front-door-"));
    for (i, want) in sent[1..5].iter().enumerate() {
        let got = std::fs::read(out.path.join(format!("frame-{:06}.jpg", i + 1))).unwrap();
        assert_eq!(&got, want);
    }
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&out.path).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(out.path.join("frame-000001.jpg")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(r.saved.lock().unwrap().len(), 4, "each file was handed to the account");
    // a second export never overwrites the first
    let out2 = r.rec.export(&w, &id, T0 + 200, T0 + 800, true).unwrap();
    assert_ne!(out.path, out2.path);
    // the log says who, what range, the result
    let log = r.rec.audit(&w, &id, 50).unwrap();
    let ex: Vec<&AuditLine> = log.iter().filter(|l| l.ev == "export").collect();
    assert!(ex.iter().any(|l| l.ok && l.detail.contains("4 pictures")) && ex.iter().any(|l| !l.ok && l.detail.contains("not confirmed")));
    assert!(ex.iter().all(|l| l.who == "amy"));
    // the account's file rules decide where it may go
    let mut denied = who("amy", false, &r.dir);
    denied.may_write = Arc::new(|_| false);
    assert!(r.rec.export(&denied, &id, T0, T0 + 1000, true).unwrap_err().contains("may not save"));
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ the audit log

#[test]
fn the_cameras_log_records_views_snapshots_playback_and_recording_and_holds_no_secrets() {
    let r = rig("audit");
    let id = r.add("Front door", "amy", &["bob"]);
    let amy = r.amy();
    let bob = who("bob", false, &r.dir);
    // live views by two people, a snapshot (no stream has a picture, so it fails: the outcome is logged all the same)
    assert!(r.hub.view(&amy, &id, false).is_ok());
    assert!(r.hub.view(&bob, &id, false).is_ok());
    assert!(r.hub.snapshot(&bob, &id).is_err());
    assert!(r.hub.snapshot(&who("eve", false, &r.dir), &id).is_err());
    r.on(&id);
    assert!(wait_for(5, || r.rec.status(&amy, &id).unwrap().segments >= 1));
    r.rec.play_start(&amy, &id, system_ms()).unwrap();
    let _ = r.rec.playback_frame(&amy, &id, 0, Seek::AtOrAfter);
    r.rec.set(&amy, &id, &Change { max_days: Some(7), ..Default::default() }).unwrap();
    r.off(&id);
    r.hub.stop_all();
    let log = r.rec.audit(&amy, &id, 200).unwrap();
    let has = |who: &str, ev: &str| log.iter().any(|l| l.who == who && l.ev == ev);
    assert!(has("amy", "live.view") && has("bob", "live.view"), "who watched live");
    assert!(has("bob", "live.snapshot") && has("eve", "live.snapshot"), "snapshots, including the refused one");
    assert!(log.iter().any(|l| l.who == "eve" && l.ev == "live.snapshot" && !l.ok));
    assert!(has("amy", "playback.start") && has("amy", "recording.start") && has("amy", "recording.setting"));
    assert!(log.iter().any(|l| l.ev == "recording.stop"));
    assert!(log.windows(2).all(|w| w[0].t >= w[1].t), "newest first");
    // nothing secret anywhere in the file, in the general audit trail or in the settings
    let key = r.vault.get("amy", &key_name(&id)).unwrap().to_string();
    let mut all = std::fs::read_to_string(r.rec.cam_dir(&id).join("audit.log")).unwrap();
    all.push_str(&r.audit.lock().unwrap().join("\n"));
    all.push_str(&std::fs::read_to_string(r.rec.settings_file()).unwrap());
    all.push_str(&std::fs::read_to_string(r.rec.cam_dir(&id).join("index.txt")).unwrap_or_default());
    for secret in [PASSWORD, key.as_str()] {
        assert!(!all.contains(secret), "a secret reached a log or a setting file");
    }
    assert!(!all.contains("rtsp://admin:"), "no address with a user and password");
    // a log line with a secret-looking address is scrubbed
    log_event(&r.dir.join("data"), &id, "amy", "x", "rtsp://admin:topsecret@10.0.0.2/live", true);
    assert!(!std::fs::read_to_string(r.rec.cam_dir(&id).join("audit.log")).unwrap().contains("topsecret"));
    // the log file is private; an unsafe camera id writes nothing
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(r.rec.cam_dir(&id).join("audit.log")).unwrap().permissions().mode() & 0o777, 0o600);
    log_event(&r.dir.join("data"), "../evil", "amy", "x", "y", true);
    assert!(!r.dir.join("data/cameras/evil").exists() && !r.dir.join("data/cameras/recordings/../evil").exists());
    // hostile text in a detail cannot forge a second line
    log_event(&r.dir.join("data"), &id, "amy\nfake", "x\n{\"t\":1}", "a\r\nb", true);
    let raw = std::fs::read_to_string(r.rec.cam_dir(&id).join("audit.log")).unwrap();
    assert!(raw.lines().all(|l| serde_json::from_str::<AuditLine>(l).is_ok()));
    r.rec.shutdown();
}

#[test]
fn the_log_rolls_over_instead_of_growing_for_ever() {
    let r = rig("auditroll");
    let id = r.add("Cam", "amy", &[]);
    let data = r.dir.join("data");
    let detail = "x".repeat(290);
    for _ in 0..4000 {
        log_event(&data, &id, "amy", "live.view", &detail, true);
    }
    let dir = r.rec.cam_dir(&id);
    assert!(dir.join("audit.log.1").exists());
    assert!(std::fs::metadata(dir.join("audit.log")).unwrap().len() < 2 * AUDIT_ROTATE);
    assert!(!r.rec.audit(&r.amy(), &id, 10_000).unwrap().is_empty());
    r.rec.shutdown();
}

// ------------------------------------------------------------------------------------------------ schedule, restart, removal

#[test]
fn windows_parse_print_and_match_the_local_time_including_past_midnight() {
    let w = Window::parse("mon-fri 08:00-18:00").unwrap();
    assert_eq!(w.text(), "weekdays 08:00-18:00");
    let z = Zone::utc();
    let at = |y, m, d, hh, mm| z.local(z.local_to_unix(y, m, d, hh, mm, 0));
    // 2027-01-04 is a Monday
    assert!(w.contains(&at(2027, 1, 4, 8, 0)) && w.contains(&at(2027, 1, 4, 17, 59)));
    assert!(!w.contains(&at(2027, 1, 4, 7, 59)) && !w.contains(&at(2027, 1, 4, 18, 0)));
    assert!(!w.contains(&at(2027, 1, 9, 10, 0)), "Saturday");
    let night = Window::parse("daily 22:00-06:00").unwrap();
    assert!(night.contains(&at(2027, 1, 4, 23, 0)) && night.contains(&at(2027, 1, 5, 5, 59)) && !night.contains(&at(2027, 1, 5, 6, 0)) && !night.contains(&at(2027, 1, 5, 12, 0)));
    let fri = Window::parse("fri 22:00-02:00").unwrap();
    assert!(fri.contains(&at(2027, 1, 8, 23, 0)) && fri.contains(&at(2027, 1, 9, 1, 0)), "Friday night runs into Saturday");
    assert!(!fri.contains(&at(2027, 1, 10, 1, 0)) && !fri.contains(&at(2027, 1, 7, 23, 0)));
    assert!(Window::parse("sat,sun 00:00-24:00").unwrap().contains(&at(2027, 1, 10, 23, 59)));
    assert_eq!(Window::parse("weekends 00:00-24:00").unwrap().days, 0x60);
    assert_eq!(Window::parse("sat-mon 09:00-10:00").unwrap().days, 0b1100001);
    for bad in ["", "mon", "mon 08:00", "mon 08:00-08:00", "funday 08:00-09:00", "mon 25:00-26:00", "mon 08:61-09:00", "mon-fri 8-9", "weekdays 24:01-02:00"] {
        assert!(Window::parse(bad).is_err(), "{bad}");
    }
    assert!(Window::parse(&format!("rtsp://a:secretpw@h {}", "9".repeat(10))).unwrap_err().len() < 200);
}

#[test]
fn a_scheduled_camera_records_inside_its_window_only() {
    let r = rig("schedule");
    let id = r.add("Cam", "amy", &[]);
    let z = Zone::utc();
    let ms = |hh, mm| z.local_to_unix(2027, 1, 4, hh, mm, 0) as u64 * 1000; // a Monday
    r.now.store(ms(12, 0), Ordering::SeqCst);
    r.rec.set(&r.amy(), &id, &Change { mode: Some(Mode::Schedule), windows: Some(vec![Window::parse("mon-fri 08:00-10:00").unwrap()]), ..Default::default() }).unwrap();
    assert!(!r.rec.recording(&id), "noon is outside the window");
    assert!(matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Waiting(ref m) if m.contains("schedule")));
    assert_eq!(r.script.opens.load(Ordering::SeqCst), 0);
    r.now.store(ms(9, 0), Ordering::SeqCst);
    r.rec.tick();
    assert!(r.rec.recording(&id), "inside the window");
    assert!(wait_for(5, || r.script.opens.load(Ordering::SeqCst) >= 1));
    r.now.store(ms(10, 0), Ordering::SeqCst);
    r.rec.tick();
    assert!(!r.rec.recording(&id), "the window ended");
    assert!(r.rec.audit(&r.amy(), &id, 50).unwrap().iter().any(|l| l.ev == "recording.stop" && l.detail.contains("schedule window ended")));
    r.rec.shutdown();
}

#[test]
fn a_restart_picks_the_switched_on_cameras_up_again_by_itself_and_settings_survive() {
    let r = rig("restart");
    let id = r.add("Cam", "amy", &[]);
    r.rec.set(&r.amy(), &id, &Change { mode: Some(Mode::Continuous), max_days: Some(5), max_mb: Some(123), ..Default::default() }).unwrap();
    r.rec.shutdown();
    assert!(!r.rec.recording(&id));
    // a new recorder on the same folder (the shell started again)
    let again = Arc::new(Recorder::new(r.hub.clone(), RecEnv { source: r.script.clone(), free_space: Arc::new(|_| Some(500_000 * MIB)), now_ms: Arc::new(system_ms), zone: Arc::new(Zone::utc), on_active: Arc::new(|_| {}), segment_ms: 150, retention_every: Duration::from_secs(3600), backoff_ms: 20, retry_after: Duration::from_millis(60) }));
    again.tick();
    assert!(again.recording(&id), "recording that is switched on resumes");
    let s = again.settings_of(&id);
    assert_eq!((s.mode, s.max_days, s.max_mb), (Mode::Continuous, 5, 123));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(again.settings_file()).unwrap().permissions().mode() & 0o777, 0o600);
    again.shutdown();
}

#[test]
fn a_lost_camera_is_tried_again_and_says_so_and_a_removed_camera_stops_recording_but_keeps_its_files() {
    let r = rig("lost");
    let id = r.add("Cam", "amy", &[]);
    r.script.down.store(true, Ordering::SeqCst);
    r.on(&id);
    assert!(wait_for(5, || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Waiting(ref m) if m.contains("cannot be reached") && m.contains("no route"))));
    assert!(wait_for(5, || r.script.opens.load(Ordering::SeqCst) >= 3), "it keeps trying");
    r.script.down.store(false, Ordering::SeqCst);
    assert!(wait_for(10, || matches!(r.rec.status(&r.amy(), &id).unwrap().state, RecState::Recording { .. })));
    assert!(wait_for(5, || r.rec.status(&r.amy(), &id).unwrap().segments >= 1));
    let files = r.rec.entries(&id).len();
    assert!(r.rec.recording(&id));
    r.hub.remove(&r.amy(), &id).unwrap();
    r.rec.tick();
    assert!(!r.rec.recording(&id) && r.rec.recording_names().is_empty());
    assert!(r.rec.entries(&id).len() >= files, "the files are kept");
    r.rec.shutdown();
}

#[test]
fn stop_all_for_a_person_is_the_red_signs_one_click() {
    let r = rig("stopall");
    let a = r.add("A", "amy", &[]);
    let b = r.add("B", "amy", &[]);
    r.on(&a);
    r.on(&b);
    assert_eq!(r.rec.recording_names().len(), 2);
    assert_eq!(r.rec.stop_all_for("bob"), 0, "somebody else's click stops nothing of amy's");
    assert_eq!(r.rec.recording_names().len(), 2);
    assert_eq!(r.rec.stop_all_for("amy"), 2);
    assert!(r.rec.recording_names().is_empty());
    assert_eq!(r.rec.settings_of(&a).mode, Mode::Off);
    r.rec.shutdown();
}
