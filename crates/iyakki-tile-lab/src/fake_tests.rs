//! The LAB tile on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no `Services`, no session, no
//! settings. The port hands the tile a fake rig (a fake picture backend that answers with a tiny PNG, a fake vision backend that streams a short
//! answer, a fake frame source with flat colours, a PNG-only codec), a catalogue written by hand, a vision check, the folders it may read and
//! write, and an audit list. A job is waited for by polling the tile on a wall-clock budget, never by sleeping a fixed time.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use iyakki_testkit::wait_until;
use std::sync::{Arc, Mutex};

use iyakki_lab_model::{CallTrace, Catalogue, FrameSource, ImageTrace, Kind, LabModel, PictureBackend, PictureRequest, Sees, VideoInfo, VisionAnswer, VisionBackend, VisionRequest};
use iyakki_lab_model as lab;
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{Event, Key, R};

use crate::app::*;
use crate::pic::PictureCodec;
use crate::port::LabPort;

/// A fresh empty folder for one test (the tag is unique per test).
fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tile-lab-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
    pm.encode_png().unwrap()
}

/// The decoders of the tests: PNG only.
struct PngCodec;

impl PictureCodec for PngCodec {
    fn is_picture(&self, bytes: &[u8]) -> bool {
        bytes.starts_with(&[0x89, b'P', b'N', b'G'])
    }
    fn decode(&self, bytes: &[u8]) -> Result<tiny_skia::Pixmap, String> {
        tiny_skia::Pixmap::decode_png(bytes).map_err(|e| e.to_string())
    }
}

// ------------------------------------------------------------------------------------------------ the fakes

// ------------------------------------------------------------------------------------------------ the fakes

#[derive(Default)]
struct FakePic {
    calls: Mutex<Vec<PictureRequest>>,
    /// Models that fail with a server error.
    fail: Mutex<Vec<String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl PictureBackend for FakePic {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        self.calls.lock().unwrap().push(r.clone());
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let mut trace = ImageTrace { provider: r.provider.clone(), model: r.model.clone(), width: r.width, height: r.height, steps: Some(4), seed: Some(r.seed), ms: 1234, ..Default::default() };
        if self.fail.lock().unwrap().contains(&r.model) {
            trace.status = Some(500);
            trace.error = "out of memory".into();
            return (Err("HTTP 500: out of memory".into()), trace);
        }
        let bytes = png(r.width.min(64), r.height.min(64), [(r.model.len() * 20 % 255) as u8, 90, 200]);
        trace.steps = r.steps.or(trace.steps);
        trace.status = Some(200);
        trace.image_bytes = bytes.len() as u64;
        (Ok((bytes, "png")), trace)
    }
}

#[derive(Default)]
struct FakeVis {
    calls: Mutex<Vec<VisionRequest>>,
}

impl VisionBackend for FakeVis {
    fn ask(&self, r: &VisionRequest, _cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace) {
        self.calls.lock().unwrap().push(r.clone());
        let mut t = CallTrace { ms: 800, request_bytes: r.images.iter().map(|i| i.len() as u64).sum(), ..Default::default() };
        if r.model == "text-only" {
            let e = "HTTP 400: this model does not support image input".to_string();
            t.status = Some(400);
            t.error = e.clone();
            return (Err(lab::cannot_see_message(&r.model, &e).unwrap()), t);
        }
        on_text("A red ");
        on_text("square.");
        t.status = Some(200);
        t.response_bytes = 13;
        (Ok(VisionAnswer { text: format!("A red square. ({})", r.model), tokens: Some((10, 3)), streamed: true }), t)
    }
}

struct FakeFrames {
    ok: bool,
}

impl FrameSource for FakeFrames {
    fn available(&self) -> Result<(), String> {
        if self.ok { Ok(()) } else { Err(lab::NO_FFMPEG.to_string()) }
    }
    fn info(&self, _f: &str) -> Result<VideoInfo, String> {
        self.available().map(|_| VideoInfo { duration: 40.0, size: (320, 180) })
    }
    fn frame_rgba(&self, _f: &str, t: f64, size: (u32, u32), _c: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        Ok(vec![(t as u8).wrapping_mul(6); size.0 as usize * size.1 as usize * 4])
    }
}

struct FakePort {
    pic: Arc<FakePic>,
    vis: Arc<FakeVis>,
    home: PathBuf,
    log: RefCell<Vec<String>>,
    saved: RefCell<Vec<(PathBuf, usize)>>,
    reads: RefCell<Vec<String>>,
    refreshed: Arc<Mutex<Vec<String>>>,
    lemonade: Vec<String>,
    /// Every path the port is asked to read is refused with this.
    refuse_read: Option<String>,
}

fn model(provider: &str, model: &str, remote: bool, sees: Sees) -> LabModel {
    LabModel { provider: provider.into(), kind: Kind::Compat, model: model.into(), remote, place: if remote { "the internet" } else { "this computer" }, size_gb: None, defaults: None, loaded: false, sees, why: String::new() }
}

impl FakePort {
    fn new(tag: &str) -> FakePort {
        FakePort { pic: Arc::new(FakePic::default()), vis: Arc::new(FakeVis::default()), home: temp_dir(tag), log: RefCell::new(vec![]), saved: RefCell::new(vec![]), reads: RefCell::new(vec![]), refreshed: Arc::new(Mutex::new(vec![])), lemonade: vec!["home".into()], refuse_read: None }
    }
    fn logged(&self, w: &str) -> bool {
        self.log.borrow().iter().any(|l| l.contains(w))
    }
}

impl LabPort for FakePort {
    fn rig(&self) -> Rig {
        Rig {
            picture: self.pic.clone(),
            vision: self.vis.clone(),
            codec: Arc::new(PngCodec),
            frames: Arc::new(FakeFrames { ok: true }),
            camera: Arc::new(|| Ok(png(40, 30, [10, 120, 60]))),
            has_camera: Arc::new(|| true),
            screen: Arc::new(|| Ok((64, 48, vec![200; 64 * 48 * 4]))),
            home: self.home.clone(),
            roots_home: None,
        }
    }
    fn catalogue(&self) -> Catalogue {
        Catalogue {
            pictures: vec![model("home", "SD-Turbo", false, Sees::Yes), model("oa", "dall-e-3", true, Sees::Yes)],
            vision: vec![model("home", "Phi-vision", false, Sees::Yes), model("oa", "gpt-4o", true, Sees::Yes)],
            chat_other: vec![model("oa", "text-only", true, Sees::Unknown)],
            video: vec![],
            pictures_not_downloaded: 0,
        }
    }
    fn check_vision(&self, _c: &Catalogue, _provider: &str, model: &str) -> Result<Sees, String> {
        if model == "text-only" {
            Err("text-only reads words only and does not take pictures.".into())
        } else {
            Ok(Sees::Yes)
        }
    }
    fn read_path(&self, _home: Option<PathBuf>, input: &str) -> Result<PathBuf, String> {
        self.reads.borrow_mut().push(input.to_string());
        match &self.refuse_read {
            Some(e) => Err(e.clone()),
            None => Ok(self.home.join(input.trim())),
        }
    }
    fn save_file(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        self.saved.borrow_mut().push((dir.to_path_buf(), bytes.len()));
        Ok(dir.join(name))
    }
    fn lemonade_ids(&self) -> Vec<String> {
        self.lemonade.clone()
    }
    fn refresh_lemonade(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let seen = self.refreshed.clone();
        Arc::new(move |id: &str| seen.lock().unwrap().push(id.to_string()))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.borrow_mut().push(format!("{action} {subject}"));
    }
}

fn settle(a: &mut LabApp, p: &FakePort) {
    wait_until(std::time::Duration::from_secs(20), || {
        a.handle_with(&Event::Tick, p);
        !a.is_busy()
    });
}

fn frame(a: &mut LabApp, p: &FakePort, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(iyakki_uikit::gfx::rgb(9, 11, 20));
    a.render_with(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), iyakki_uikit::ui::scale_for(w, h).min(1.0), p, 0);
    g
}

fn press(a: &mut LabApp, p: &FakePort, g: &Gfx, label: &str) {
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control `{label}`"));
    a.handle_with(&h.ev.clone(), p);
}

#[test]
fn a_picture_on_this_computer_is_drawn_through_the_rig_audited_and_saved_through_the_port() {
    let p = FakePort::new("labfk1");
    let mut a = LabApp::with_port(&p);
    assert_eq!(a.pic_a, Some(("home".into(), "SD-Turbo".into())), "the first model of the catalogue the port gave");
    a.handle_with(&Event::Paste("a fox".into()), &p);
    a.handle_with(&Event::Key(Key::Ctrl('\n')), &p);
    assert!(a.modal.is_none(), "a model on this computer needs no consent");
    settle(&mut a, &p);
    assert_eq!(p.pic.calls.lock().unwrap().len(), 1);
    assert!(p.logged("lab.picture SD-Turbo on home"), "{:?}", p.log.borrow());
    let g = frame(&mut a, &p, 1300, 800);
    press(&mut a, &p, &g, "Save picture");
    let (dir, n) = p.saved.borrow()[0].clone();
    assert_eq!(dir, p.home.join("Pictures"));
    assert!(n > 0);
    assert!(p.logged("lab.picture.save") && a.status_text().unwrap().0.starts_with("Saved as"), "{:?}", a.status_text());
}

#[test]
fn a_remote_model_asks_first_and_the_yes_is_audited_before_anything_is_sent() {
    let p = FakePort::new("labfk2");
    let mut a = LabApp::with_port(&p);
    a.pic_a = Some(("oa".into(), "dall-e-3".into()));
    a.handle_with(&Event::Paste("a fox".into()), &p);
    a.handle_with(&Event::Key(Key::Ctrl('\n')), &p);
    assert!(a.modal.is_some(), "the person is asked before a prompt leaves the computer");
    assert!(p.pic.calls.lock().unwrap().is_empty());
    let g = frame(&mut a, &p, 1300, 800);
    press(&mut a, &p, &g, "Yes, send");
    assert!(p.logged("lab.remote_ok oa"));
    settle(&mut a, &p);
    assert_eq!(p.pic.calls.lock().unwrap().len(), 1);
    // asked once for the session: the next one goes straight
    a.handle_with(&Event::Key(Key::Ctrl('\n')), &p);
    assert!(a.modal.is_none());
    settle(&mut a, &p);
    assert_eq!(p.pic.calls.lock().unwrap().len(), 2);
}

#[test]
fn a_file_is_read_through_the_port_and_a_refusal_is_shown_and_nothing_is_sent() {
    let mut p = FakePort::new("labfk3");
    std::fs::write(p.home.join("red.png"), png(64, 64, [200, 20, 20])).unwrap();
    let mut a = LabApp::with_port(&p);
    a.handle_with(&Event::Key(Key::F(3)), &p);
    a.path.set("red.png");
    a.focus = Focus::Path;
    a.handle_with(&Event::Key(Key::Enter), &p);
    settle(&mut a, &p);
    assert_eq!(p.reads.borrow().as_slice(), ["red.png"]);
    assert!(a.input.is_some(), "{:?}", a.status_text());
    assert!(p.logged("lab.open"));
    // the port refuses the next path
    p.refuse_read = Some("That folder is outside what you may read.".into());
    a.input = None;
    a.path.set("/etc/shadow");
    a.focus = Focus::Path;
    a.handle_with(&Event::Key(Key::Enter), &p);
    settle(&mut a, &p);
    assert!(a.input.is_none());
    assert_eq!(a.status_text().map(|s| s.1), Some(false));
    assert!(a.status_text().unwrap().0.contains("outside"));
    assert!(p.vis.calls.lock().unwrap().is_empty());
}

#[test]
fn the_vision_check_of_the_port_refuses_a_model_before_anything_is_sent() {
    let p = FakePort::new("labfk4");
    std::fs::write(p.home.join("red.png"), png(64, 64, [200, 20, 20])).unwrap();
    let mut a = LabApp::with_port(&p);
    a.handle_with(&Event::Key(Key::F(3)), &p);
    a.path.set("red.png");
    a.focus = Focus::Path;
    a.handle_with(&Event::Key(Key::Enter), &p);
    settle(&mut a, &p);
    a.cat.chat_other = vec![model("oa", "text-only", true, Sees::Unknown)];
    a.vis_a = Some(("oa".into(), "text-only".into()));
    a.question.set("What is this?");
    a.run_vision(&p, false);
    assert!(a.status_text().unwrap().0.contains("does not take pictures"), "{:?}", a.status_text());
    assert!(p.vis.calls.lock().unwrap().is_empty(), "nothing was sent");
    assert!(!p.logged("lab.vision"), "a refused question is not logged as asked");
    assert!(a.log_text().contains("refused before sending"));
    // a model the port accepts is asked and audited
    a.vis_a = Some(("home".into(), "Phi-vision".into()));
    a.run_vision(&p, false);
    settle(&mut a, &p);
    assert_eq!(p.vis.calls.lock().unwrap().len(), 1);
    assert!(p.logged("lab.vision Phi-vision on home"));
}

#[test]
fn refreshing_the_lists_reads_each_lemonade_server_through_the_port_and_without_one_says_so() {
    let mut p = FakePort::new("labfk5");
    let mut a = LabApp::with_port(&p);
    let g = frame(&mut a, &p, 1300, 800);
    press(&mut a, &p, &g, "Refresh models");
    settle(&mut a, &p);
    assert_eq!(p.refreshed.lock().unwrap().as_slice(), ["home"]);
    p.lemonade.clear();
    let g = frame(&mut a, &p, 1300, 800);
    press(&mut a, &p, &g, "Refresh models");
    assert!(a.status_text().unwrap().0.contains("no Lemonade server"), "{:?}", a.status_text());
    assert_eq!(p.refreshed.lock().unwrap().len(), 1);
}

#[test]
fn every_tab_draws_at_the_sizes_on_a_catalogue_the_port_gave() {
    let p = FakePort::new("labfk6");
    let mut a = LabApp::with_port(&p);
    for (w, h) in [(1300u32, 800u32), (800, 600), (480, 400)] {
        for tab in [Tab::Picture, Tab::Vision, Tab::Video] {
            a.switch(tab);
            let g = frame(&mut a, &p, w, h);
            assert!(g.hits.iter().any(|x| x.label == "Refresh models" || !x.label.is_empty()), "{tab:?} at {w}x{h}");
        }
    }
}
