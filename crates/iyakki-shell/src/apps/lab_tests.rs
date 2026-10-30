//! LAB with fakes: no network, no real model, no camera, no screen, no ffmpeg. A fake picture backend answers with a tiny PNG, a
//! fake vision backend streams a short answer, a fake frame source returns flat colours.

use super::*;
use crate::apps::chatpic::fixtures::png;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use crate::gfx::Gfx;
use crate::ui::R;
use iyakki_agent::lab::{CallTrace, VideoInfo};
use iyakki_agent::providers::{Kind, ProviderCfg};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

// ------------------------------------------------------------------------------------------------ the fakes

#[derive(Default)]
struct FakePic {
    calls: Mutex<Vec<PictureRequest>>,
    /// Models that fail with a server error.
    fail: Mutex<Vec<String>>,
    /// While true, a call waits (so a test can cancel it).
    hold: AtomicBool,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl PictureBackend for FakePic {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        self.calls.lock().unwrap().push(r.clone());
        let t0 = Instant::now();
        while self.hold.load(Ordering::SeqCst) && t0.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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

struct World {
    cx: AppCx,
    pic: Arc<FakePic>,
    vis: Arc<FakeVis>,
    home: PathBuf,
    camera_calls: Arc<AtomicUsize>,
}

static PORT: AtomicUsize = AtomicUsize::new(0);

fn lemonade_list() -> serde_json::Value {
    serde_json::json!({"data": [
        {"id": "Qwen3-0.6B-GGUF", "recipe": "llamacpp", "size": 0.38, "downloaded": true, "labels": []},
        {"id": "Phi-4-Multimodal-GGUF", "recipe": "llamacpp", "size": 3.1, "downloaded": true, "labels": ["vision"]},
        {"id": "SD-Turbo", "recipe": "sd-cpp", "size": 2.6, "downloaded": true, "labels": ["image"], "image_defaults": {"steps": 4, "width": 512, "height": 512}},
        {"id": "SDXL-Base-1.0", "recipe": "sd-cpp", "size": 6.9, "downloaded": true, "labels": ["image"], "image_defaults": {"steps": 20, "width": 1024, "height": 1024}}
    ]})
}

fn add_lemonade(cx: &AppCx, id: &str, base: &str) {
    let mut p = ProviderCfg::new(id, Kind::Lemonade);
    p.base_url = Some(base.into());
    cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(base), &iyakki_agent::lemonade::parse_models(&lemonade_list(), &[]));
}

/// A session with a local Lemonade (`home`), one on another computer (`gpu`) and a plain provider (`oa`).
fn world(tag: &str, with_models: bool) -> World {
    let mut cx = cx_with_admin_store(tag);
    let home = temp_dir(&format!("{tag}-home"));
    cx.session.user = "root".into();
    if with_models {
        let n = PORT.fetch_add(3, Ordering::SeqCst) as u32 + std::process::id() % 1000 * 10;
        add_lemonade(&cx, "home", &format!("http://127.0.0.1:{}", 21000 + n));
        add_lemonade(&cx, "gpu", &format!("http://192.168.1.50:{}", 22000 + n));
        let mut oa = ProviderCfg::new("oa", Kind::Compat);
        oa.base_url = Some("http://127.0.0.1:13999/v1".into());
        oa.models = vec!["gpt-4o".into(), "dall-e-3".into(), "text-only".into(), "text-embedding-3-small".into()];
        cx.services.settings.lock().unwrap().upsert_provider(oa).unwrap();
    }
    World { cx, pic: Arc::new(FakePic::default()), vis: Arc::new(FakeVis::default()), home, camera_calls: Arc::new(AtomicUsize::new(0)) }
}

fn rig(w: &World, ffmpeg: bool) -> Rig {
    let calls = w.camera_calls.clone();
    let shot = png(40, 30, [10, 120, 60]);
    Rig {
        picture: w.pic.clone(),
        vision: w.vis.clone(),
        frames: Arc::new(FakeFrames { ok: ffmpeg }),
        camera: Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(shot.clone())
        }),
        has_camera: Arc::new(|| true),
        screen: Arc::new(|| Ok((64, 48, vec![200; 64 * 48 * 4]))),
        home: w.home.clone(),
        roots_home: Some(w.home.clone()),
    }
}

fn app(w: &World) -> LabApp {
    LabApp::with_rig(&w.cx, rig(w, true))
}

fn pump(a: &mut LabApp, cx: &AppCx) {
    let t = Instant::now();
    loop {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() || t.elapsed().as_secs() > 8 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    assert!(!a.is_busy(), "the job did not finish");
}

fn draw(a: &mut LabApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(crate::gfx::rgb(9, 11, 20));
    g.clear_hits();
    a.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), crate::shell::scale_for(w, h).min(1.0), cx, 0);
    g
}

/// Click the control with this label on the drawn screen, the way the mouse does.
fn click(a: &mut LabApp, cx: &AppCx, label: &str) {
    let g = draw(a, cx, 1300, 800);
    let hit = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control called {label:?}: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
    let ev = hit.ev.clone();
    a.handle(&ev, cx);
}

fn type_prompt(a: &mut LabApp, cx: &AppCx, t: &str) {
    a.handle(&Event::Paste(t.to_string()), cx);
}

fn choose(a: &mut LabApp, cx: &AppCx, control: &str, model: &str) {
    click(a, cx, control);
    let Some(Modal::Pick { picker, .. }) = &a.modal else { panic!("no list opened") };
    let i = picker.visible().iter().position(|l| l.starts_with(model)).unwrap_or_else(|| panic!("{model} not in {:?}", picker.visible()));
    a.handle(&Event::Row(i), cx);
    assert!(a.modal.is_none());
}

// ------------------------------------------------------------------------------------------------ registration

#[test]
fn the_tile_is_in_work_for_everyone_but_guests_and_works_in_a_cloud_session() {
    use iyakki_sec::policy::{App as PApp, Role};
    let tiles = crate::shell::tiles_for_test();
    assert_eq!(tiles.iter().filter(|(a, _)| *a == PApp::Lab).count(), 1);
    assert!(tiles.contains(&(PApp::Lab, "LAB")));
    assert_eq!(crate::launcher::Group::of(PApp::Lab), Some(crate::launcher::Group::Home)); // the home screen groups changed (F-SHELL-22, F-SHELL-54): LAB sits in Ask & Create
    assert_eq!((PApp::Lab.id(), PApp::Lab.label().as_str()), ("lab", "LAB"));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Lab), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Lab));
    assert_eq!(iyakki_agent::cloudmode::app_class(PApp::Lab), iyakki_agent::cloudmode::Class::Remote);
    assert_eq!(crate::memgate::class_of(PApp::Lab), crate::memgate::AppClass::Heavy);
    let w = world("lab-reg", true);
    let a = app(&w);
    assert_eq!((a.title(), a.tab()), ("Lab", Tab::Picture));
    assert!(a.help().len() >= 6 && a.hint().contains("F3 Vision"));
}

// ------------------------------------------------------------------------------------------------ the lists

#[test]
fn each_tab_lists_the_models_that_fit_it_and_the_choice_prefills_the_size() {
    let w = world("lab-lists", true);
    let mut a = app(&w);
    let pics: Vec<String> = a.cat.pictures.iter().map(|m| format!("{}/{}", m.provider, m.model)).collect();
    assert_eq!(pics, ["home/SD-Turbo", "home/SDXL-Base-1.0", "oa/dall-e-3", "gpu/SD-Turbo", "gpu/SDXL-Base-1.0"], "this computer first");
    let vis: Vec<String> = a.cat.vision.iter().map(|m| m.model.clone()).collect();
    assert!(vis.contains(&"Phi-4-Multimodal-GGUF".to_string()) && vis.contains(&"gpt-4o".to_string()) && !vis.contains(&"text-only".to_string()), "{vis:?}");
    assert_eq!(a.pic_a, Some(("home".into(), "SD-Turbo".into())));
    assert_eq!((a.width.value.as_str(), a.height.value.as_str()), ("512", "512"), "the model's own size");
    assert_eq!(a.steps.value, "4", "the model's own steps in the box");
    assert!(a.steps_text().contains("model's own 4"), "{}", a.steps_text());
    choose(&mut a, &w.cx, "Model", "SDXL-Base-1.0 \u{b7} on home");
    assert_eq!((a.width.value.as_str(), a.height.value.as_str()), ("1024", "1024"));
    assert_eq!(a.steps.value, "20");
    assert!(a.steps_text().contains("model's own 20"));
    choose(&mut a, &w.cx, "Model", "dall-e-3");
    assert_eq!(a.pic_a, Some(("oa".into(), "dall-e-3".into())));
    assert!(a.steps_text().contains("this server chooses its own") && a.steps.value.is_empty(), "a server that does not say gets no made-up number");
    // vision: "any chat model" adds the unmarked ones, with a warning
    a.handle(&Event::Key(Key::F(3)), &w.cx);
    assert_eq!(a.tab(), Tab::Vision);
    assert!(a.cat.vision_choices(a.any_chat).iter().all(|m| m.sees == Sees::Yes));
    click(&mut a, &w.cx, "any chat model");
    assert!(a.any_chat && a.cat.vision_choices(true).iter().any(|m| m.model == "text-only" && m.sees == Sees::Unknown));
    choose(&mut a, &w.cx, "Model", "text-only");
    assert!(a.vis_model(Slot::Vis).unwrap().label().contains("may not see pictures"));
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "any chat model"));
}

#[test]
fn with_no_models_the_tabs_say_what_to_do_and_nothing_is_sent() {
    let w = world("lab-empty", false);
    let mut a = app(&w);
    assert!(a.cat.pictures.is_empty() && a.cat.vision.is_empty());
    for tab in [Tab::Picture, Tab::Vision, Tab::Video] {
        a.switch(tab);
        let g = draw(&mut a, &w.cx, 1000, 700);
        assert!(g.hits.iter().any(|h| h.label == "Refresh models"), "{tab:?}");
    }
    a.switch(Tab::Picture);
    type_prompt(&mut a, &w.cx, "a fox");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    let (t, ok) = a.status_text().unwrap();
    assert!(!ok && t.contains("No picture model is available") && t.contains("Settings"), "{t}");
    assert!(w.pic.calls.lock().unwrap().is_empty() && !a.is_busy() && a.modal.is_none());
    assert_eq!(lab::video_line(&a.cat), lab::NO_VIDEO_MODEL);
}

// ------------------------------------------------------------------------------------------------ pictures

#[test]
fn a_picture_is_made_shown_logged_kept_and_saved_only_when_asked() {
    let w = world("lab-pic", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a red kite over a beach");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.is_busy());
    assert!(a.progress_text().unwrap().contains("Drawing"), "a progress line while it runs");
    pump(&mut a, &w.cx);
    assert_eq!(a.history.len(), 1);
    let it = &a.history[0];
    assert_eq!((it.provider.as_str(), it.model.as_str(), it.ext, it.ms), ("home", "SD-Turbo", "png", 1234));
    assert_eq!((it.asked, it.size, it.steps), ((512, 512), (64, 64), Some(4)), "asked size, size it really has, steps the server used");
    assert!(it.seed > 0 && it.prompt == "a red kite over a beach" && !it.bytes.is_empty());
    let req = w.pic.calls.lock().unwrap()[0].clone();
    assert_eq!((req.width, req.height, req.seed, req.negative.as_deref(), req.steps), (512, 512, it.seed, None, Some(4)));
    assert_eq!(a.shown.len(), 1);
    let g = draw(&mut a, &w.cx, 1300, 800);
    for l in ["Generate", "Save picture", "Copy prompt", "history"] {
        assert!(g.hits.iter().any(|h| h.label == l), "{l}");
    }
    // the log line: time, provider, model, request summary, status, duration, bytes
    let log = a.log_text();
    assert!(log.contains("picture") && log.contains("home / SD-Turbo") && log.contains("512x512 seed") && log.contains("a red kite") && log.contains("HTTP 200") && log.contains("1.2 s"), "{log}");
    assert!(!a.home_has_pictures(), "nothing is written until Save");
    // Save puts it in Pictures (and nowhere else)
    click(&mut a, &w.cx, "Save picture");
    let (t, ok) = a.status_text().unwrap();
    assert!(ok && t.starts_with("Saved as "), "{t}");
    let files: Vec<_> = std::fs::read_dir(w.home.join("Pictures")).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    assert_eq!(std::fs::read(files[0].path()).unwrap(), a.history[0].bytes);
    // Copy prompt
    click(&mut a, &w.cx, "Copy prompt");
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("a red kite over a beach"));
}

impl LabApp {
    fn home_has_pictures(&self) -> bool {
        self.rig.home.join("Pictures").exists()
    }
}

#[test]
fn size_and_seed_are_bounded_and_a_negative_prompt_goes_only_where_the_server_takes_it() {
    let w = world("lab-fields", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a lighthouse");
    a.width.set("99999");
    a.height.set("7");
    a.seed.set("42");
    a.steps.set("9999");
    a.negative.set("blurry");
    assert!(a.pic_model(Slot::PicA).unwrap().takes_negative());
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    let r = w.pic.calls.lock().unwrap()[0].clone();
    assert_eq!((r.width, r.height, r.seed, r.negative.as_deref(), r.steps), (2048, 64, 42, Some("blurry"), Some(100)), "bounded to 64..2048 and 1..100, the seed as typed");
    assert_eq!(a.steps.value, "100", "the box shows what was sent");
    assert_eq!((a.width.value.as_str(), a.height.value.as_str()), ("2048", "64"), "the boxes show what was sent");
    assert!(a.log_text().contains("+negative") && a.log_text().contains("seed 42"));
    // a provider that does not take one is not sent one, and the box is not offered
    choose(&mut a, &w.cx, "Model", "dall-e-3");
    assert!(!a.pic_model(Slot::PicA).unwrap().takes_negative());
    assert!(!a.order().contains(&Focus::Negative));
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    let second = w.pic.calls.lock().unwrap()[1].clone();
    assert_eq!((second.negative, second.steps), (None, None), "a server that takes neither is sent neither");
    assert!(!a.order().contains(&Focus::Steps));
    // New seed and Reuse seed
    click(&mut a, &w.cx, "New");
    assert!(a.seed.value.parse::<u64>().unwrap() != 42);
    click(&mut a, &w.cx, "Reuse seed");
    assert_eq!(a.seed.value, a.history.last().unwrap().seed.to_string());
    // an empty prompt is said, not sent
    a.prompt.set("  ");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.status_text().unwrap().0.contains("Type what the picture should show") && w.pic.calls.lock().unwrap().len() == 2);
}

#[test]
fn the_styles_of_tales_are_added_to_the_prompt() {
    let w = world("lab-style", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox");
    click(&mut a, &w.cx, "watercolor");
    assert_eq!(a.prompt.text, "a fox, a soft watercolor painting");
    assert_eq!(a.focus, Focus::Prompt);
}

#[test]
fn cancel_stops_waiting_and_the_late_answer_is_dropped() {
    let w = world("lab-cancel", true);
    w.pic.hold.store(true, Ordering::SeqCst);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    let t = Instant::now();
    while w.pic.calls.lock().unwrap().is_empty() && t.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    assert!(a.is_busy());
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "Cancel"));
    a.handle(&Event::Key(Key::Esc), &w.cx);
    assert!(!a.is_busy(), "the screen is free at once");
    let (t, ok) = a.status_text().unwrap();
    assert!(ok && t.contains("Cancelled") && t.contains("may still finish on the server"), "{t}");
    assert!(a.log_text().contains("stopped by the person") && a.log_text().contains("not recalled"), "{}", a.log_text());
    w.pic.hold.store(false, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(80));
    a.handle(&Event::Tick, &w.cx);
    assert!(a.history.is_empty() && a.shown.is_empty(), "the dropped answer does not appear");
    // the next run works
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    assert_eq!(a.history.len(), 1);
}

#[test]
fn compare_runs_the_same_prompt_on_two_models_one_after_the_other_in_order() {
    let w = world("lab-compare", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a cat");
    a.seed.set("5");
    choose(&mut a, &w.cx, "Compare with", "SDXL-Base-1.0 \u{b7} on home");
    click(&mut a, &w.cx, "Compare (one after the other)");
    pump(&mut a, &w.cx);
    let calls = w.pic.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().map(|c| c.model.as_str()).collect::<Vec<_>>(), ["SD-Turbo", "SDXL-Base-1.0"], "A first, then B");
    assert!(calls.iter().all(|c| c.prompt == "a cat" && c.seed == 5), "the same prompt and seed");
    assert_eq!(w.pic.max_in_flight.load(Ordering::SeqCst), 1, "never at the same time");
    let ids: Vec<u64> = a.shown.iter().filter_map(|s| if let Shown::Item(i) = s { Some(*i) } else { None }).collect();
    assert_eq!(ids.len(), 2);
    assert_eq!((a.item(ids[0]).unwrap().model.as_str(), a.item(ids[1]).unwrap().model.as_str()), ("SD-Turbo", "SDXL-Base-1.0"));
    assert_eq!(a.log.len(), 2, "one log line each");
    draw(&mut a, &w.cx, 1300, 800);
    // two different models are needed
    a.pic_b = a.pic_a.clone();
    click(&mut a, &w.cx, "Compare (one after the other)");
    assert!(a.status_text().unwrap().0.contains("two different models"));
    // when the second fails the first stays and the failure is shown with its reason
    a.pic_b = Some(("home".into(), "SDXL-Base-1.0".into()));
    w.pic.fail.lock().unwrap().push("SDXL-Base-1.0".into());
    click(&mut a, &w.cx, "Compare (one after the other)");
    pump(&mut a, &w.cx);
    assert!(matches!(a.shown[0], Shown::Item(_)) && matches!(&a.shown[1], Shown::Failed { error, .. } if error.contains("out of memory")));
    assert!(a.log_text().contains("HTTP 500") && a.log_text().contains("ERROR"));
    draw(&mut a, &w.cx, 1300, 800);
}

#[test]
fn the_history_keeps_at_most_sixteen_and_a_memory_budget() {
    let w = world("lab-caps", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a dot");
    for _ in 0..lab::HISTORY_MAX_ITEMS + 3 {
        a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
        pump(&mut a, &w.cx);
    }
    assert_eq!(a.history.len(), lab::HISTORY_MAX_ITEMS);
    assert_eq!(a.history.last().unwrap().id, (lab::HISTORY_MAX_ITEMS + 3) as u64, "the newest is kept");
    // a result that weighs a lot pushes the older ones out
    let pm = Arc::new(Pixmap::new(8, 8).unwrap());
    let mk = |id: u64, mb: usize| Item { id, provider: "p".into(), model: "m".into(), remote: false, prompt: String::new(), negative: String::new(), seed: 1, asked: (8, 8), size: (8, 8), steps: None, ms: 1, bytes: vec![0; mb << 20], ext: "png", view: pm.clone(), thumb: pm.clone() };
    a.history = vec![mk(1, 20), mk(2, 20), mk(3, 20)];
    a.enforce_caps();
    assert_eq!(a.history.iter().map(|i| i.id).collect::<Vec<_>>(), [2, 3], "60 MB is over the 48 MB budget: the oldest goes");
    // the list rows pick a result to look at
    a.history = vec![mk(1, 1), mk(2, 1)];
    a.shown.clear();
    a.handle(&Event::Tab(T_HIST0), &w.cx);
    assert!(matches!(a.shown[0], Shown::Item(2)), "the first row is the newest");
    a.handle(&Event::Tab(T_HIST0 + 1), &w.cx);
    assert!(matches!(a.shown[0], Shown::Item(1)));
    draw(&mut a, &w.cx, 1300, 800);
}

#[test]
fn a_provider_on_another_computer_is_asked_about_once_per_session() {
    let w = world("lab-remote", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox");
    choose(&mut a, &w.cx, "Model", "SD-Turbo \u{b7} on gpu");
    let m = a.pic_model(Slot::PicA).unwrap().clone();
    assert!(m.remote && m.runs_on() == "Runs on gpu (remote, the network)");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(matches!(&a.modal, Some(Modal::Consent { providers, .. }) if providers[0].0 == "gpu"), "asked first");
    assert!(w.pic.calls.lock().unwrap().is_empty(), "nothing is sent while the card is open");
    draw(&mut a, &w.cx, 1000, 700);
    a.handle(&Event::Tab(T_NO), &w.cx);
    assert!(a.modal.is_none() && w.pic.calls.lock().unwrap().is_empty());
    assert!(a.status_text().unwrap().0.contains("Nothing was sent"));
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    a.handle(&Event::Key(Key::Char('y')), &w.cx);
    pump(&mut a, &w.cx);
    assert_eq!(w.pic.calls.lock().unwrap().len(), 1);
    // the second run on the same provider does not ask again
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.modal.is_none());
    pump(&mut a, &w.cx);
    assert_eq!(w.pic.calls.lock().unwrap().len(), 2);
    assert!(a.history.iter().all(|i| i.remote));
    // a local model never asks; a new session asks again
    choose(&mut a, &w.cx, "Model", "SD-Turbo \u{b7} on home");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.modal.is_none());
    pump(&mut a, &w.cx);
    let mut b = app(&w);
    type_prompt(&mut b, &w.cx, "a fox");
    choose(&mut b, &w.cx, "Model", "SD-Turbo \u{b7} on gpu");
    b.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(matches!(b.modal, Some(Modal::Consent { .. })), "approvals are not kept past the session");
    assert!(w.cx.services.settings.lock().unwrap().get().voice.remote_ok.is_empty(), "and are not written to the settings");
    // compare with a remote second model asks about it before anything runs
    let mut c = app(&w);
    type_prompt(&mut c, &w.cx, "a fox");
    choose(&mut c, &w.cx, "Compare with", "SD-Turbo \u{b7} on gpu");
    click(&mut c, &w.cx, "Compare (one after the other)");
    assert!(matches!(c.modal, Some(Modal::Consent { .. })));
    c.handle(&Event::Tab(T_YES), &w.cx);
    pump(&mut c, &w.cx);
    assert_eq!(c.history.len(), 2);
}

// ------------------------------------------------------------------------------------------------ vision

fn write_png(w: &World, name: &str, wd: u32, ht: u32) -> String {
    let p = w.home.join(name);
    std::fs::write(&p, png(wd, ht, [200, 30, 30])).unwrap();
    p.display().to_string()
}

fn open_file(a: &mut LabApp, w: &World, path: &str) {
    a.path.set(path);
    a.focus = Focus::Path;
    a.handle(&Event::Key(Key::Enter), &w.cx);
    pump(a, &w.cx);
}

#[test]
fn a_picture_from_a_file_is_shrunk_to_a_bounded_size_and_the_size_sent_is_shown() {
    let w = world("lab-file", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "small.png", 200, 100));
    let p = a.input.clone().unwrap();
    assert_eq!((p.w, p.h, p.scaled()), (200, 100, false));
    assert!(p.sent_line().starts_with("Sending 200x100") && !p.sent_line().contains("smaller"), "{}", p.sent_line());
    open_file(&mut a, &w, &write_png(&w, "big.png", 3000, 1500));
    let p = a.input.clone().unwrap();
    assert_eq!((p.w, p.h, p.from, p.scaled()), (1024, 512, (3000, 1500), true), "neither side over 1024");
    assert!(p.sent_line().contains("was 3000x1500") && p.sent_line().contains("made smaller"), "{}", p.sent_line());
    assert!(p.png.len() <= lab::VISION_MAX_BYTES);
    assert!(a.log_text().contains("downscaled from 3000x1500"), "{}", a.log_text());
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "Ask"));
    // a file that is not a picture, one that is missing, one over the limit
    std::fs::write(w.home.join("notes.txt"), "hello").unwrap();
    open_file(&mut a, &w, &w.home.join("notes.txt").display().to_string());
    assert!(a.status_text().unwrap().0.contains("not a picture IYAKKI can read"), "{:?}", a.status_text());
    open_file(&mut a, &w, &w.home.join("missing.png").display().to_string());
    assert!(!a.status_text().unwrap().1);
    let huge = std::fs::File::create(w.home.join("huge.png")).unwrap();
    huge.set_len(lab::MAX_INPUT_FILE_BYTES + 1).unwrap();
    open_file(&mut a, &w, &w.home.join("huge.png").display().to_string());
    assert!(a.status_text().unwrap().0.contains("too large to open") && a.status_text().unwrap().0.contains("25 MB"), "{:?}", a.status_text());
    // the picture that was ready stays
    assert!(a.input.is_some());
    // pixels over the cap are refused from the header, before decoding
    let mut bomb = png(2, 2, [0, 0, 0]);
    bomb[16..20].copy_from_slice(&20000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&20000u32.to_be_bytes());
    assert!(lab_pic::prepare(&bomb, "x").unwrap_err().contains("64 megapixels"));
    assert_eq!(lab_pic::header_size(&png(7, 5, [1, 2, 3])), Some((7, 5)));
}

#[test]
fn the_camera_the_screen_and_the_clipboard_give_a_picture_and_the_private_ones_ask_first() {
    let w = world("lab-src", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    // the camera asks every time; no is final
    click(&mut a, &w.cx, "Camera");
    assert!(matches!(a.modal, Some(Modal::Source(Src::Camera))));
    draw(&mut a, &w.cx, 1000, 700);
    a.handle(&Event::Tab(T_NO), &w.cx);
    assert!(a.modal.is_none() && w.camera_calls.load(Ordering::SeqCst) == 0 && a.input.is_none());
    click(&mut a, &w.cx, "Camera");
    a.handle(&Event::Key(Key::Char('y')), &w.cx);
    pump(&mut a, &w.cx);
    assert_eq!(w.camera_calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.input.as_ref().unwrap().source, "the camera");
    // the screen asks too
    click(&mut a, &w.cx, "Screenshot");
    assert!(matches!(a.modal, Some(Modal::Source(Src::Screen))));
    a.handle(&Event::Tab(T_YES), &w.cx);
    pump(&mut a, &w.cx);
    let p = a.input.as_ref().unwrap();
    assert_eq!((p.source.as_str(), p.w, p.h), ("the screen", 64, 48));
    // the clipboard picture
    click(&mut a, &w.cx, "Clipboard picture");
    pump(&mut a, &w.cx);
    crate::ui::clipboard::clear();
    click(&mut a, &w.cx, "Clipboard picture");
    assert!(a.status_text().unwrap().0.contains("holds no picture"), "{:?}", a.status_text());
    crate::ui::clipboard::copy_image(30, 20, vec![180; 30 * 20 * 4]);
    click(&mut a, &w.cx, "Clipboard picture");
    pump(&mut a, &w.cx);
    assert_eq!(a.input.as_ref().unwrap().source, "the clipboard");
    let log = a.log_text();
    assert!(log.contains("/ camera") && log.contains("/ screen") && log.contains("/ clipboard"), "{log}");
    // no camera: the button is off and says why
    let mut b = LabApp::with_rig(&w.cx, Rig { has_camera: Arc::new(|| false), ..rig(&w, true) });
    b.switch(Tab::Vision);
    let g = draw(&mut b, &w.cx, 1300, 800);
    assert!(!g.hits.iter().any(|h| h.label == "Camera"));
    b.handle(&Event::Key(Key::Char('k')), &w.cx);
    assert!(b.modal.is_none() && b.status_text().unwrap().0.contains("No camera"));
}

#[test]
fn a_question_is_answered_as_it_streams_with_time_and_tokens_and_the_log_says_so() {
    let w = world("lab-ask", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    // nothing to ask about yet
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.status_text().unwrap().0.contains("Choose a picture first"));
    open_file(&mut a, &w, &write_png(&w, "red.png", 64, 64));
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    assert!(a.status_text().unwrap().0.contains("Type a question"));
    click(&mut a, &w.cx, "Describe this picture");
    assert_eq!(a.question.text, "Describe this picture.");
    for (i, (label, text)) in QUICK.iter().enumerate() {
        a.handle(&Event::Tab(T_QUICK0 + i), &w.cx);
        assert_eq!(a.question.text, *text, "{label}");
    }
    a.handle(&Event::Tab(T_QUICK0), &w.cx);
    click(&mut a, &w.cx, "Ask");
    assert!(a.is_busy());
    pump(&mut a, &w.cx);
    let ans = &a.answers[0];
    assert!(ans.done && ans.error.is_none() && ans.text.starts_with("A red square."), "{}", ans.text);
    assert_eq!((ans.tokens, ans.ms, ans.streamed), (Some((10, 3)), 800, true));
    let sent = w.vis.calls.lock().unwrap()[0].clone();
    assert_eq!((sent.images.len(), sent.question.as_str()), (1, "Describe this picture."));
    assert!(sent.images[0].starts_with(&[0x89, b'P', b'N', b'G']));
    let log = a.log_text();
    assert!(log.contains("vision") && log.contains("home / Phi-4-Multimodal-GGUF") && log.contains("1 picture") && log.contains("HTTP 200") && log.contains("10 tokens in, 3 out"), "{log}");
    draw(&mut a, &w.cx, 1300, 800);
    click(&mut a, &w.cx, "Copy answer");
    assert!(crate::ui::clipboard::latest().unwrap().contains("A red square."));
}

#[test]
fn a_model_that_cannot_take_pictures_says_so_and_nothing_is_faked() {
    let w = world("lab-cannot", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "red.png", 64, 64));
    a.question.set("What is this?");
    // a chat model not marked as vision: tried, the server refuses, the message says what happened
    click(&mut a, &w.cx, "any chat model");
    choose(&mut a, &w.cx, "Model", "text-only");
    click(&mut a, &w.cx, "Ask");
    pump(&mut a, &w.cx);
    let e = a.answers[0].error.clone().unwrap();
    assert!(e.starts_with("text-only cannot take pictures") && e.contains("does not support image input"), "{e}");
    assert!(a.answers[0].text.is_empty(), "no made-up answer");
    assert!(a.log_text().contains("HTTP 400") && a.log_text().contains("ERROR"), "{}", a.log_text());
    draw(&mut a, &w.cx, 1300, 800);
    // a model that can never see pictures is refused before sending (here a picture model offered as a chat model)
    let before = w.vis.calls.lock().unwrap().len();
    let mut dalle = a.cat.pictures.iter().find(|m| m.model == "dall-e-3").unwrap().clone();
    dalle.sees = Sees::Unknown;
    a.cat.chat_other.push(dalle);
    a.vis_a = Some(("oa".into(), "dall-e-3".into()));
    a.run_vision(&w.cx, false);
    assert!(a.status_text().unwrap().0.contains("draws pictures from text"), "{:?}", a.status_text());
    assert_eq!(w.vis.calls.lock().unwrap().len(), before, "nothing was sent");
    assert!(a.log_text().contains("not sent") && a.log_text().contains("refused before sending"));
}

#[test]
fn two_vision_models_answer_the_same_picture_one_after_the_other() {
    let w = world("lab-vcompare", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "red.png", 64, 64));
    a.question.set("What is this?");
    choose(&mut a, &w.cx, "Compare with", "gpt-4o");
    click(&mut a, &w.cx, "Compare two models");
    pump(&mut a, &w.cx);
    let calls = w.vis.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].images == calls[1].images && calls[0].question == calls[1].question, "the same picture and question");
    assert_eq!((a.answers.len(), a.answers[0].label.as_str()), (2, "Phi-4-Multimodal-GGUF on home"));
    assert!(a.answers[1].text.contains("gpt-4o") && a.answers.iter().all(|x| x.done));
    assert_eq!(a.log.len(), 3, "the picture and one line for each model");
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "Compare two models"));
    draw(&mut a, &w.cx, 700, 600);
}

#[test]
fn a_remote_vision_model_asks_before_the_picture_leaves() {
    let w = world("lab-vremote", true);
    let mut a = app(&w);
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "red.png", 64, 64));
    a.question.set("What is this?");
    choose(&mut a, &w.cx, "Model", "Phi-4-Multimodal-GGUF \u{b7} on gpu");
    click(&mut a, &w.cx, "Ask");
    assert!(matches!(a.modal, Some(Modal::Consent { then: Then::Vision { .. }, .. })));
    assert!(w.vis.calls.lock().unwrap().is_empty());
    draw(&mut a, &w.cx, 1000, 700);
    a.handle(&Event::Tab(T_YES), &w.cx);
    pump(&mut a, &w.cx);
    assert_eq!(w.vis.calls.lock().unwrap().len(), 1);
}

// ------------------------------------------------------------------------------------------------ video

fn video_file(w: &World) -> String {
    let p = w.home.join("clip.mp4");
    std::fs::write(&p, b"not really a video").unwrap();
    p.display().to_string()
}

#[test]
fn a_video_is_sampled_into_frames_with_times_and_asked_about() {
    let w = world("lab-video", true);
    let mut a = app(&w);
    a.switch(Tab::Video);
    assert!(a.ffmpeg.is_ok());
    a.vpath.set(&video_file(&w));
    a.question.set("What happens?");
    // the number of frames is 4 to 16
    for _ in 0..30 {
        a.handle(&Event::Tab(T_FRAMES_UP), &w.cx);
    }
    assert_eq!(a.nframes, 16);
    for _ in 0..30 {
        a.handle(&Event::Tab(T_FRAMES_DN), &w.cx);
    }
    assert_eq!(a.nframes, 4);
    click(&mut a, &w.cx, "Take frames and ask");
    pump(&mut a, &w.cx);
    assert_eq!(a.frames.len(), 4);
    assert_eq!(a.frames.iter().map(|f| f.time).collect::<Vec<_>>(), [5.0, 15.0, 25.0, 35.0], "evenly spaced over 40 s");
    let sent = w.vis.calls.lock().unwrap()[0].clone();
    assert_eq!(sent.images.len(), 4);
    assert!(sent.question.starts_with("These 4 pictures are frames from one video, in order, taken at 0:05, 0:15, 0:25, 0:35.") && sent.question.ends_with("What happens?"), "{}", sent.question);
    assert!(a.answers[0].done && a.answers[0].text.starts_with("A red square."));
    let log = a.log_text();
    assert!(log.contains("video") && log.contains("4 frame(s) from clip.mp4") && log.contains("ffmpeg"), "{log}");
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "Take frames and ask"));
    // every k seconds
    click(&mut a, &w.cx, "evenly spaced");
    assert!(!a.even);
    a.every.set("10");
    a.nframes = 5;
    click(&mut a, &w.cx, "Take frames and ask");
    pump(&mut a, &w.cx);
    assert_eq!(a.frames.iter().map(|f| f.time).collect::<Vec<_>>(), [0.0, 10.0, 20.0, 30.0], "a frame every 10 s while the video lasts: 0, 10, 20, 30");
    draw(&mut a, &w.cx, 700, 600);
    draw(&mut a, &w.cx, 400, 700);
}

#[test]
fn without_ffmpeg_the_tab_says_what_is_missing_and_how_to_get_it_without_a_dialog() {
    let w = world("lab-noffmpeg", true);
    let mut a = LabApp::with_rig(&w.cx, rig(&w, false));
    a.switch(Tab::Video);
    let e = a.ffmpeg.clone().unwrap_err();
    assert!(e.contains("ffmpeg is not installed") && e.contains("Store") && e.contains("does not ship it"), "{e}");
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.label == "Open Media tools setup"), "a button to the setup, not an error dialog");
    a.vpath.set(&video_file(&w));
    a.question.set("What happens?");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    let (t, ok) = a.status_text().unwrap();
    assert!(!ok && t.contains("ffmpeg is not installed"), "{t}");
    assert!(a.modal.is_none() && !a.is_busy() && w.vis.calls.lock().unwrap().is_empty(), "nothing was sent");
    click(&mut a, &w.cx, "Open Media tools setup");
    assert_eq!(a.take_request(), Some(("quickinstall".into(), "ffmpeg".into())));
}

#[test]
fn video_generation_is_offered_only_when_a_provider_lists_a_video_model() {
    let w = world("lab-vgen", true);
    let mut a = app(&w);
    a.switch(Tab::Video);
    assert!(a.cat.video.is_empty());
    assert_eq!(lab::video_line(&a.cat), "No video generation model is available from your providers. Models are installed in Settings \u{2192} Models (Lemonade or Ollama); none of them lists a video model today.");
    draw(&mut a, &w.cx, 1300, 800);
    // a server that lists one: said plainly, and no request exists to send
    let list = serde_json::json!({"data": [{"id": "Wan-Video-1", "recipe": "wan-video", "size": 9.0, "downloaded": true, "labels": ["video"]}]});
    let base = format!("http://127.0.0.1:{}", 23000 + std::process::id() % 1000);
    let mut p = ProviderCfg::new("vid", Kind::Lemonade);
    p.base_url = Some(base.clone());
    w.cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(&base), &iyakki_agent::lemonade::parse_models(&list, &[]));
    a.reload(&w.cx);
    assert_eq!(a.cat.video.len(), 1);
    let line = lab::video_line(&a.cat);
    assert!(line.contains("Wan-Video-1") && line.contains("nothing is faked"), "{line}");
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(!g.hits.iter().any(|h| h.label.to_lowercase().contains("generate video")), "no fake button");
}

// ------------------------------------------------------------------------------------------------ log, keys, drawing

#[test]
fn the_log_copies_clears_and_holds_no_secrets() {
    let w = world("lab-log", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    let text = a.log_text();
    assert_eq!(text.lines().count(), 1);
    for secret in ["Bearer", "Authorization", "api_key", "sk-"] {
        assert!(!text.contains(secret), "{text}");
    }
    click(&mut a, &w.cx, "Copy log");
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(text.as_str()));
    click(&mut a, &w.cx, "Clear log");
    assert!(a.log.is_empty() && a.log_text().is_empty() && a.logv.lines.is_empty());
    click(&mut a, &w.cx, "Copy log");
    assert!(a.status_text().unwrap().0.contains("log is empty"));
    // the panel is on every tab
    for tab in [Tab::Picture, Tab::Vision, Tab::Video] {
        a.switch(tab);
        let g = draw(&mut a, &w.cx, 1000, 700);
        assert!(g.hits.iter().any(|h| h.label == "Copy log") && g.hits.iter().any(|h| h.label == "Clear log"), "{tab:?}");
    }
}

#[test]
fn save_is_refused_where_the_account_may_not_write_and_nothing_is_left_behind() {
    let mut w = world("lab-save", true);
    w.cx.session.policy.fs_write = vec![];
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    click(&mut a, &w.cx, "Save picture");
    let (t, ok) = a.status_text().unwrap();
    assert!(!ok, "{t}");
    assert!(!w.home.join("Pictures").exists() || std::fs::read_dir(w.home.join("Pictures")).unwrap().count() == 0);
    // with nothing made there is nothing to save
    let w2 = world("lab-save2", true);
    let mut b = app(&w2);
    b.handle(&Event::Key(Key::Char('s')), &w2.cx);
    b.focus = Focus::None;
    b.handle(&Event::Key(Key::Char('s')), &w2.cx);
    assert!(b.status_text().unwrap().0.contains("no picture to save"));
}

#[test]
fn keys_and_mouse_both_drive_it() {
    let w = world("lab-keys", true);
    let mut a = app(&w);
    // the tabs by key and by click
    a.handle(&Event::Key(Key::F(3)), &w.cx);
    assert_eq!(a.tab(), Tab::Vision);
    a.handle(&Event::Key(Key::F(4)), &w.cx);
    assert_eq!(a.tab(), Tab::Video);
    click(&mut a, &w.cx, "PICTURE");
    assert_eq!(a.tab(), Tab::Picture);
    // Tab moves between the boxes and Shift+Tab back
    a.focus = Focus::Prompt;
    a.handle(&Event::Key(Key::Tab), &w.cx);
    assert_eq!(a.focus, Focus::Negative, "SD-Turbo's server takes a negative prompt");
    a.handle(&Event::Key(Key::BackTab), &w.cx);
    assert_eq!(a.focus, Focus::Prompt);
    // typing, the single-key shortcuts when no box has the keyboard
    for c in "a boat".chars() {
        a.handle(&Event::Key(Key::Char(c)), &w.cx);
    }
    assert_eq!(a.prompt.text, "a boat");
    a.handle(&Event::Key(Key::Esc), &w.cx);
    assert_eq!(a.focus, Focus::None);
    a.handle(&Event::Key(Key::Char('g')), &w.cx);
    pump(&mut a, &w.cx);
    assert_eq!(a.history.len(), 1);
    a.handle(&Event::Key(Key::Char('v')), &w.cx);
    assert_eq!(a.tab(), Tab::Vision);
    a.handle(&Event::Key(Key::Char('p')), &w.cx);
    // a press in the prompt box and in a field focuses it
    let g = draw(&mut a, &w.cx, 1300, 800);
    assert!(g.hits.iter().any(|h| h.ev == Event::Area(PROMPT_AREA)) && g.hits.iter().any(|h| h.ev == Event::Focus(F_SEED)));
    a.handle(&Event::FocusAt(F_SEED, 5.0), &w.cx);
    assert_eq!(a.focus, Focus::Seed);
    for c in "123".chars() {
        a.handle(&Event::Key(Key::Char(c)), &w.cx);
    }
    assert_eq!(a.seed.value, "123");
    a.handle(&Event::AreaAt(PROMPT_AREA, 20.0, 10.0), &w.cx);
    assert_eq!(a.focus, Focus::Prompt);
    // Esc with nothing running and nothing focused goes back
    a.handle(&Event::Key(Key::Esc), &w.cx);
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &w.cx), Flow::Back));
    // the wheel scrolls the controls when they do not fit
    let mut g = Gfx::new(500, 360);
    a.render(&mut g, R::new(0.0, 46.0, 500.0, 314.0), 1.0, &w.cx, 0);
    assert!(a.col_max > 0.0, "a small window scrolls its controls");
    a.handle(&Event::Scroll(-3), &w.cx);
    assert!(a.col_scroll > 0.0);
}

#[test]
fn every_tab_draws_at_phone_laptop_and_large_sizes_in_every_state() {
    let w = world("lab-draw", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a fox in the snow, with a long description that wraps over more than one line of the prompt box");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "r.png", 90, 60));
    a.question.set("What is this?");
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    a.switch(Tab::Video);
    a.vpath.set(&video_file(&w));
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    for tab in [Tab::Picture, Tab::Vision, Tab::Video] {
        a.switch(tab);
        for (wd, ht) in [(360u32, 740u32), (800, 480), (1024, 600), (1920, 1080)] {
            let g = draw(&mut a, &w.cx, wd, ht);
            assert!(g.hits.iter().any(|h| h.label == "Copy log"), "{tab:?} {wd}x{ht}");
        }
    }
    // the cards
    a.switch(Tab::Picture);
    a.modal = Some(Modal::Consent { providers: vec![("gpu".into(), "the network")], then: Then::Picture { compare: false } });
    draw(&mut a, &w.cx, 800, 480);
    a.modal = Some(Modal::Source(Src::Screen));
    draw(&mut a, &w.cx, 800, 480);
    a.modal = None;
    click(&mut a, &w.cx, "Model");
    draw(&mut a, &w.cx, 800, 480);
    a.handle(&Event::Key(Key::Esc), &w.cx);
    assert!(a.modal.is_none());
    a.handle(&Event::Tab(T_BROWSE), &w.cx);
    a.switch(Tab::Vision);
    a.handle(&Event::Tab(T_BROWSE), &w.cx);
    assert!(matches!(a.modal, Some(Modal::File { video: false, .. })));
    draw(&mut a, &w.cx, 900, 600);
    a.handle(&Event::Key(Key::Esc), &w.cx);
    assert!(a.modal.is_none());
}

/// `IYAKKI_SHOT=/path cargo test -p iyakki-shell --lib lab_picture -- --ignored` writes pictures of the three tabs.
#[test]
#[ignore]
fn lab_picture() {
    let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
    let w = world("lab-shot", true);
    let mut a = app(&w);
    type_prompt(&mut a, &w.cx, "a lighthouse on a cliff at dusk, watercolor");
    choose(&mut a, &w.cx, "Compare with", "SDXL-Base-1.0 \u{b7} on home");
    click(&mut a, &w.cx, "Compare (one after the other)");
    pump(&mut a, &w.cx);
    for (i, (wd, ht)) in [(1280u32, 800u32), (390, 780)].into_iter().enumerate() {
        let g = draw(&mut a, &w.cx, wd, ht);
        g.save_png(std::path::Path::new(&format!("{out}-picture-{i}.png"))).unwrap();
    }
    a.switch(Tab::Vision);
    open_file(&mut a, &w, &write_png(&w, "red.png", 640, 400));
    click(&mut a, &w.cx, "Describe this picture");
    click(&mut a, &w.cx, "Compare two models");
    pump(&mut a, &w.cx);
    draw(&mut a, &w.cx, 1280, 800).save_png(std::path::Path::new(&format!("{out}-vision.png"))).unwrap();
    a.switch(Tab::Video);
    a.vpath.set(&video_file(&w));
    a.handle(&Event::Key(Key::Ctrl('\n')), &w.cx);
    pump(&mut a, &w.cx);
    draw(&mut a, &w.cx, 1280, 800).save_png(std::path::Path::new(&format!("{out}-video.png"))).unwrap();
}
