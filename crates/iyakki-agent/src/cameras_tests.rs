//! The camera hub against scripted cameras on the loopback address (`iyakki_rtsp::fakeserver`) and fake USB cameras. No real camera
//! was used.

use super::*;
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use std::collections::HashMap as Map;

const JPG: &[u8] = include_bytes!("../../iyakki-rtsp/tests/data/test-48x32.jpg");

#[derive(Default)]
struct Vault {
    items: Mutex<Map<(String, String), String>>,
    locked: AtomicBool,
}

impl CamSecrets for Vault {
    fn get(&self, owner: &str, name: &str) -> Result<Zeroizing<String>, String> {
        if self.locked.load(Ordering::SeqCst) {
            return Err("the vault is locked".into());
        }
        self.items.lock().unwrap().get(&(owner.into(), name.into())).map(|v| Zeroizing::new(v.clone())).ok_or_else(|| "no such secret".into())
    }
    fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), String> {
        if self.locked.load(Ordering::SeqCst) {
            return Err("could not save the camera's password in your vault: locked".into());
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

struct Rig {
    hub: Hub,
    vault: Arc<Vault>,
    audit: Arc<Mutex<Vec<String>>>,
    saved: Arc<Mutex<Vec<PathBuf>>>,
    dir: PathBuf,
}

struct FakeUsb {
    opened: Arc<Mutex<u32>>,
    busy: bool,
}
struct SolidFeed(u8);
impl Feed for SolidFeed {
    fn next(&mut self) -> Result<Picture, FeedErr> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(Picture::Rgba { width: 8, height: 4, data: [self.0, 100, 50, 255].repeat(32) })
    }
}
impl UsbProvider for FakeUsb {
    fn list(&self) -> Vec<UsbCam> {
        vec![UsbCam { id: "video0".into(), name: "HD Webcam".into(), builtin: false }]
    }
    fn open(&self, _: &str) -> Result<Box<dyn Feed>, FeedErr> {
        *self.opened.lock().unwrap() += 1;
        if self.busy { Err(FeedErr::Busy("the camera is used by another app".into())) } else { Ok(Box::new(SolidFeed(200))) }
    }
}

struct FakeDecoder;
impl H264Decoder for FakeDecoder {
    fn decode(&mut self, au: &AccessUnit) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        assert!(au.is_keyframe());
        Ok(Some((4, 2, [9, 9, 9, 255].repeat(8))))
    }
}
struct WithH264;
impl H264Support for WithH264 {
    fn decoder(&self) -> Option<Box<dyn H264Decoder>> {
        Some(Box::new(FakeDecoder))
    }
    fn hint(&self) -> String {
        String::new()
    }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-cam-{tag}-{}-{}", std::process::id(), crate::clock::now_unix()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig_with(tag: &str, usb: Arc<dyn UsbProvider>, h264: Arc<dyn H264Support>, idle_ms: u64) -> Rig {
    let dir = tmp(tag);
    let vault = Arc::new(Vault::default());
    let audit = Arc::new(Mutex::new(vec![]));
    let saved = Arc::new(Mutex::new(vec![]));
    let (a2, s2) = (audit.clone(), saved.clone());
    let env = Env {
        data: dir.clone(),
        secrets: vault.clone(),
        people: Arc::new(|| vec!["amy".into(), "bob".into(), "root".into()]),
        usb,
        h264,
        encode: Arc::new(|w, h, d| {
            let mut v = vec![0xFF, 0xD8, b'E', b'N', b'C'];
            v.extend_from_slice(&w.to_be_bytes());
            v.extend_from_slice(&h.to_be_bytes());
            v.push(d[0]);
            v
        }),
        after_save: Arc::new(move |p| s2.lock().unwrap().push(p.to_path_buf())),
        audit: Arc::new(move |u, e, d| a2.lock().unwrap().push(format!("{u}|{e}|{d}"))),
        backoff_ms: 30,
        idle: Duration::from_millis(idle_ms),
    };
    Rig { hub: Hub::new(env), vault, audit, saved, dir }
}

fn rig(tag: &str) -> Rig {
    rig_with(tag, Arc::new(NoUsb), Arc::new(NoH264), 4000)
}

fn who(r: &Rig, user: &str, admin: bool) -> Who {
    let home = r.dir.join("home").join(user);
    Who { user: user.into(), manage: admin, usb_ok: true, home, may_write: Arc::new(|_| true) }
}

fn cfg(user: Option<(&str, &str)>, digest: bool) -> Config {
    Config { user: user.map(|(u, p)| (u.to_string(), p.to_string())), digest, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() }
}

fn new_cam(name: &str, address: &str, user: &str, allowed: &[&str]) -> NewSpec {
    NewSpec { name: name.into(), address: address.into(), user: user.into(), allowed: allowed.iter().map(|s| s.to_string()).collect() }
}

fn wait_frame(r: &Rig, w: &Who, id: &str, big: bool) -> Frame {
    let t = Instant::now();
    loop {
        if let Some(f) = r.hub.view(w, id, big).unwrap() {
            return f;
        }
        assert!(t.elapsed() < Duration::from_secs(8), "no picture; status {:?}", r.hub.cameras(w).iter().find(|c| c.id == id).map(|c| c.status.clone()));
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_status(r: &Rig, w: &Who, id: &str, pred: impl Fn(&Status) -> bool) -> Status {
    let t = Instant::now();
    loop {
        let _ = r.hub.view(w, id, false);
        if let Some(c) = r.hub.cameras(w).into_iter().find(|c| c.id == id) {
            if pred(&c.status) {
                return c.status;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(8), "status never matched: {:?}", r.hub.cameras(w).iter().find(|c| c.id == id).map(|c| c.status.clone()));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_camera_saved_with_credentials_in_its_address_keeps_the_password_only_in_the_vault() {
    let r = rig("save");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Front door", "rtsp://admin:hunter2@192.168.1.20:554/stream1", "", &["bob"]), None).unwrap();
    let file = std::fs::read_to_string(r.dir.join("cameras/cameras.json")).unwrap();
    assert!(!file.contains("hunter2") && !file.contains("admin:"), "no password and no user:password address in the record: {file}");
    assert!(file.contains("\"address\": \"rtsp://192.168.1.20:554/stream1\"") && file.contains("\"user\": \"admin\""));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(r.dir.join("cameras/cameras.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(&*r.vault.get("amy", &format!("cam.{id}")).unwrap(), "hunter2");
    let v = &r.hub.cameras(&amy)[0];
    assert_eq!((v.name.as_str(), v.address.as_str(), v.user.as_str(), v.has_password, v.manage), ("Front door", "rtsp://192.168.1.20:554/stream1", "admin", true, true));
    assert!(r.audit.lock().unwrap().iter().all(|l| !l.contains("hunter2") && !l.contains("admin:")), "{:?}", r.audit.lock().unwrap());
    // an edit that leaves the password empty keeps it; an explicit empty one removes it
    r.hub.save(&amy, Some(&id), &new_cam("Front door", "rtsp://192.168.1.20:554/stream1", "admin", &["bob"]), None).unwrap();
    assert!(r.vault.has("amy", &format!("cam.{id}")));
    r.hub.save(&amy, Some(&id), &new_cam("Front door", "rtsp://192.168.1.20:554/stream1", "admin", &["bob"]), Some("")).unwrap();
    assert!(!r.vault.has("amy", &format!("cam.{id}")));
}

#[test]
fn bad_input_is_refused_in_words_and_never_echoes_a_password() {
    let r = rig("bad");
    let amy = who(&r, "amy", false);
    let e = r.hub.save(&amy, None, &new_cam("x", "rtsp://u:pw-secret@host:99999/x", "", &[]), None).unwrap_err();
    assert!(!e.contains("pw-secret"), "{e}");
    assert!(r.hub.save(&amy, None, &new_cam("  ", "rtsp://h/x", "", &[]), None).unwrap_err().contains("name"));
    assert!(r.hub.save(&amy, None, &new_cam("c", "ftp://h/x", "", &[]), None).is_err());
    assert!(r.hub.save(&amy, None, &new_cam("c", "https://h/x", "", &[]), None).unwrap_err().contains("not supported"));
    assert!(r.hub.save(&amy, None, &new_cam("c", "rtsp://h/x", "", &["nobody"]), None).unwrap_err().contains("no account"));
    r.hub.save(&amy, None, &new_cam("Door", "rtsp://h/x", "", &[]), None).unwrap();
    assert!(r.hub.save(&amy, None, &new_cam("door", "rtsp://h/y", "", &[]), None).unwrap_err().contains("already exists"));
    // a locked vault saves nothing at all
    r.vault.locked.store(true, Ordering::SeqCst);
    assert!(r.hub.save(&amy, None, &new_cam("Garage", "rtsp://h/g", "u", &[]), Some("pw")).is_err());
    assert_eq!(r.hub.specs().len(), 1, "the failed save left no half-saved camera");
}

#[test]
fn only_the_owner_and_the_people_on_the_list_may_watch_and_administrators_manage_without_watching() {
    let cam = FakeCamera::rtsp(cfg(None, true));
    let r = rig("access");
    let (amy, bob, root) = (who(&r, "amy", false), who(&r, "bob", false), who(&r, "root", true));
    let id = r.hub.save(&amy, None, &new_cam("Yard", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    assert!(r.hub.cameras(&bob).is_empty(), "bob does not even see it");
    assert!(r.hub.view(&bob, &id, false).unwrap_err().contains("not on its list"));
    assert!(r.hub.snapshot(&bob, &id).is_err());
    assert!(r.hub.remove(&bob, &id).is_err());
    assert!(r.hub.resolve(&bob, "Yard").is_err());
    // the administrator sees the record and may manage it, but the stream is not shown to them
    let rv = r.hub.cameras(&root);
    assert_eq!(rv.len(), 1);
    assert!(rv[0].manage);
    assert!(r.hub.view(&root, &id, false).is_err(), "managing is not watching");
    assert_eq!(r.hub.running(), 0, "a refused view starts no stream");
    // amy gives bob access
    r.hub.save(&amy, Some(&id), &new_cam("Yard", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &["bob"]), None).unwrap();
    assert_eq!(r.hub.cameras(&bob).len(), 1);
    assert!(!r.hub.cameras(&bob)[0].manage, "watching is not managing");
    assert!(r.hub.save(&bob, Some(&id), &new_cam("Yard", "rtsp://h/x", "", &[]), None).is_err());
    wait_frame(&r, &bob, &id, false);
    // and takes it away again
    r.hub.save(&amy, Some(&id), &new_cam("Yard", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    assert!(r.hub.cameras(&bob).is_empty());
    assert!(r.hub.view(&bob, &id, false).is_err());
    // only the owner changes the password
    assert!(r.hub.save(&root, Some(&id), &new_cam("Yard", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), Some("pw")).unwrap_err().contains("only the person who added"));
    r.hub.stop_all();
}

#[test]
fn a_digest_camera_goes_live_with_the_password_from_the_vault_and_pictures_are_shrunk() {
    let cam = FakeCamera::rtsp(cfg(Some(("admin", "s3cret")), true));
    let r = rig("live");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Hall", &format!("rtsp://admin:s3cret@127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    let f = wait_frame(&r, &amy, &id, false);
    assert_eq!((f.width, f.height), (48, 32));
    assert_eq!(f.rgba.len(), 48 * 32 * 4);
    assert_eq!(wait_status(&r, &amy, &id, |s| *s == Status::Live), Status::Live);
    let v = &r.hub.cameras(&amy)[0];
    assert!(v.fps >= 0.0 && v.size == (48, 32));
    assert!(cam.requests().iter().all(|l| !l.contains("s3cret")));
    // seq advances with new pictures, at the capped rate
    let first = f.seq;
    std::thread::sleep(Duration::from_millis(500));
    let later = r.hub.view(&amy, &id, false).unwrap().unwrap();
    assert!(later.seq > first && later.seq <= first + 5, "{} -> {} in half a second (cap {SMALL_FPS}/s)", first, later.seq);
    let big = wait_frame(&r, &amy, &id, true);
    assert_eq!(big.width, 48);
    r.hub.stop_all();
}

#[test]
fn wrong_password_locked_vault_and_missing_camera_each_say_what_is_wrong() {
    let cam = FakeCamera::rtsp(cfg(Some(("admin", "right")), true));
    let r = rig("wrong");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Hall", &format!("rtsp://admin:wrong@127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    let s = wait_status(&r, &amy, &id, |s| matches!(s, Status::Lost(_)));
    let Status::Lost(m) = s else { unreachable!() };
    assert!(m.contains("refused the user name or password") && !m.contains("wrong\""), "{m}");
    r.hub.stop_all();
    // the owner's vault is locked: the stream cannot read the password
    let id2 = r.hub.save(&amy, None, &new_cam("Hall2", &format!("rtsp://admin:right@127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    r.vault.locked.store(true, Ordering::SeqCst);
    let s = wait_status(&r, &amy, &id2, |s| matches!(s, Status::NeedsPassword(_)));
    assert!(s.detail_text().contains("vault"));
    r.vault.locked.store(false, Ordering::SeqCst);
    r.hub.stop_all();
    // nothing listens there
    let dead = r.hub.save(&amy, None, &new_cam("Dead", "rtsp://127.0.0.1:1/x", "", &[]), None).unwrap();
    let s = wait_status(&r, &amy, &dead, |s| matches!(s, Status::Lost(_)));
    assert!(s.detail_text().contains("cannot reach"));
    r.hub.stop_all();
}

trait Detail {
    fn detail_text(&self) -> String;
}
impl Detail for Status {
    fn detail_text(&self) -> String {
        match self {
            Status::Lost(m) | Status::NeedsAddon(m) | Status::Busy(m) | Status::NeedsPassword(m) => m.clone(),
            _ => String::new(),
        }
    }
}

#[test]
fn a_dropped_stream_is_tried_again_with_a_pause() {
    let cam = FakeCamera::rtsp(Config { max_frames: Some(3), ..cfg(None, true) });
    let r = rig("retry");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Flaky", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    let t = Instant::now();
    while cam.streams.load(Ordering::SeqCst) < 2 {
        let _ = r.hub.view(&amy, &id, false);
        assert!(t.elapsed() < Duration::from_secs(10), "never reconnected");
        std::thread::sleep(Duration::from_millis(20));
    }
    r.hub.stop_all();
}

#[test]
fn a_stream_stops_when_nobody_looks_and_the_connection_is_released() {
    let cam = FakeCamera::rtsp(cfg(None, true));
    let r = rig_with("idle", Arc::new(NoUsb), Arc::new(NoH264), 300);
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Idle", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    wait_frame(&r, &amy, &id, false);
    assert_eq!(r.hub.running(), 1);
    let t = Instant::now();
    while r.hub.running() > 0 {
        assert!(t.elapsed() < Duration::from_secs(5), "the stream kept running with nobody looking");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn h264_needs_the_addon_unless_a_decoder_is_plugged_in() {
    let cam = FakeCamera::rtsp(Config { h264: true, ..cfg(None, true) });
    let r = rig("h264");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("H264", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    let s = wait_status(&r, &amy, &id, |s| matches!(s, Status::NeedsAddon(_)));
    assert!(s.detail_text().contains("add-on") && s.detail_text().contains("Motion-JPEG"));
    assert!(r.hub.view(&amy, &id, false).unwrap().is_none());
    r.hub.stop_all();
    let r2 = rig_with("h264b", Arc::new(NoUsb), Arc::new(WithH264), 4000);
    let id = r2.hub.save(&amy, None, &new_cam("H264", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    let f = wait_frame(&r2, &amy, &id, false);
    assert_eq!((f.width, f.height), (4, 2));
    r2.hub.stop_all();
}

#[test]
fn http_cameras_work_too() {
    let cam = FakeCamera::http(cfg(Some(("u", "p")), false));
    let r = rig("http");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Web", &format!("http://u:p@127.0.0.1:{}/video.mjpg", cam.port), "", &[]), None).unwrap();
    wait_frame(&r, &amy, &id, false);
    let id2 = r.hub.save(&amy, None, &new_cam("Snap", &format!("http://127.0.0.1:{}/snapshot.jpg", cam.port), "u", &[]), Some("p")).unwrap();
    wait_frame(&r, &amy, &id2, false);
    r.hub.stop_all();
}

#[test]
fn a_snapshot_is_one_jpeg_in_the_persons_pictures_and_asks_the_write_rule() {
    let cam = FakeCamera::rtsp(cfg(None, true));
    let r = rig("snap");
    let amy = who(&r, "amy", false);
    let id = r.hub.save(&amy, None, &new_cam("Front door", &format!("rtsp://127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    assert!(r.hub.snapshot(&amy, &id).unwrap_err().contains("not shown a picture yet"));
    wait_frame(&r, &amy, &id, false);
    let p = r.hub.snapshot(&amy, &id).unwrap();
    assert!(p.starts_with(r.dir.join("home/amy/Pictures")) && p.file_name().unwrap().to_string_lossy().starts_with("camera-front-door-"));
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    assert!(iyakki_web::jpeg::decode(&bytes).is_ok(), "the saved file is a picture that decodes");
    assert_eq!(r.saved.lock().unwrap().as_slice(), &[p.clone()], "the shell is told so it can give the file to the account");
    let p2 = r.hub.snapshot(&amy, &id).unwrap();
    assert_ne!(p, p2, "nothing is overwritten");
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("amy|camera.snapshot|Front door")));
    let mut denied = amy.clone();
    denied.may_write = Arc::new(|_| false);
    assert!(r.hub.snapshot(&denied, &id).unwrap_err().contains("may not save"));
    r.hub.stop_all();
}

#[test]
fn usb_cameras_are_listed_first_open_only_while_looked_at_and_snapshot_through_the_encoder() {
    let opened = Arc::new(Mutex::new(0));
    let r = rig_with("usb", Arc::new(FakeUsb { opened: opened.clone(), busy: false }), Arc::new(NoH264), 300);
    let amy = who(&r, "amy", false);
    let list = r.hub.cameras(&amy);
    assert_eq!((list[0].id.as_str(), list[0].kind, list[0].name.as_str()), ("usb:video0", Kind::Usb, "HD Webcam"));
    assert_eq!(*opened.lock().unwrap(), 0, "listing a camera does not open it");
    assert_eq!(r.hub.running(), 0);
    let f = wait_frame(&r, &amy, "usb:video0", false);
    assert_eq!((f.width, f.height), (8, 4));
    let p = r.hub.snapshot(&amy, "usb:video0").unwrap();
    let b = std::fs::read(&p).unwrap();
    assert_eq!(&b[2..5], b"ENC", "an RGBA picture goes through the encoder");
    let t = Instant::now();
    while r.hub.running() > 0 {
        assert!(t.elapsed() < Duration::from_secs(5), "the USB camera stayed open with nobody looking");
        std::thread::sleep(Duration::from_millis(50));
    }
    // an account without camera rights sees none and cannot open it
    let mut nope = amy.clone();
    nope.usb_ok = false;
    assert!(r.hub.cameras(&nope).is_empty());
    assert!(r.hub.view(&nope, "usb:video0", false).is_err());
    assert!(r.hub.view(&amy, "usb:nothere", false).is_err());
}

#[test]
fn a_busy_usb_camera_says_so() {
    let r = rig_with("busy", Arc::new(FakeUsb { opened: Arc::new(Mutex::new(0)), busy: true }), Arc::new(NoH264), 2000);
    let amy = who(&r, "amy", false);
    let s = wait_status(&r, &amy, "usb:video0", |s| matches!(s, Status::Busy(_)));
    assert!(s.detail_text().contains("another app"));
    r.hub.stop_all();
}

#[test]
fn removing_a_camera_deletes_its_password_and_stops_its_stream() {
    let cam = FakeCamera::rtsp(cfg(Some(("u", "p")), true));
    let r = rig("remove");
    let (amy, root) = (who(&r, "amy", false), who(&r, "root", true));
    let id = r.hub.save(&amy, None, &new_cam("Gone", &format!("rtsp://u:p@127.0.0.1:{}/live", cam.port), "", &[]), None).unwrap();
    wait_frame(&r, &amy, &id, false);
    assert!(r.vault.has("amy", &format!("cam.{id}")));
    r.hub.remove(&root, &id).unwrap();
    assert!(r.hub.cameras(&amy).is_empty() && !r.vault.has("amy", &format!("cam.{id}")));
    assert_eq!(r.hub.running(), 0);
    assert!(r.hub.remove(&amy, &id).is_err());
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("root|camera.remove|Gone")));
}

#[test]
fn names_resolve_and_ambiguity_is_refused() {
    let r = rig("resolve");
    let (amy, bob) = (who(&r, "amy", false), who(&r, "bob", false));
    r.hub.save(&amy, None, &new_cam("Door", "rtsp://h/a", "", &["bob"]), None).unwrap();
    r.hub.save(&bob, None, &new_cam("door", "rtsp://h/b", "", &[]), None).unwrap();
    assert!(r.hub.resolve(&bob, "Door").unwrap_err().contains("more than one"));
    let id = r.hub.cameras(&bob)[0].id.clone();
    assert_eq!(r.hub.resolve(&bob, &id).unwrap().id, id);
    assert!(r.hub.resolve(&amy, "door").is_ok());
    assert!(r.hub.resolve(&amy, "nope").is_err());
}

#[test]
fn shrinking_averages_whole_blocks_and_leaves_small_pictures_alone() {
    let px: Vec<u8> = (0..4 * 4).flat_map(|i| [(i * 10) as u8, 0, 0, 255]).collect();
    assert_eq!(shrink(4, 4, &px, 8), (4, 4, px.clone()));
    let (w, h, out) = shrink(4, 4, &px, 2);
    assert_eq!((w, h, out.len()), (2, 2, 16));
    assert_eq!(out[0], ((0 + 10 + 40 + 50) / 4) as u8);
    // a picture whose data is shorter than its size says is returned as is, not read out of bounds
    let _ = shrink(100, 100, &[1, 2, 3], 10);
}
