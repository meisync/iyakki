//! Settings → Models with AMD's Lemonade Server, against a fake Lemonade on loopback: Detect, the Auto-detect type of the
//! provider form, the models by type with their quick actions (chat, embeddings, TALES pictures, reading voice,
//! speech-to-text), Load / Pull behind an approval card naming the size, and the page's geometry at four screen sizes.

use super::*;
use crate::apps::lemonade_fake::{serve, FakeLemonade};
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::lemonade;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    let up: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    cx.services = services_in(&d, up);
    cx.services.set_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| key(a, cx, Key::Char(c)));
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..1000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn options(f: &Form) -> Vec<String> {
    match &f.fields[1].kind {
        super::super::widgets::FieldKind::Choice(o) => o.clone(),
        _ => vec![],
    }
}
fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A Lemonade provider added the way Detect adds it, with its typed list read, selected on the Models page.
fn with_lemonade(tag: &str) -> (AppCx, SettingsApp, FakeLemonade) {
    let fake = serve();
    let cx = admin(tag);
    cx.services.use_local_lemonade(&fake.base(), Some("2026.42.1"));
    cx.services.refresh_lemonade("lemonade-local").unwrap();
    let a = SettingsApp::new(&cx);
    (cx, a, fake)
}

/// Click the chip of `model` in the Lemonade panel.
fn click_chip(a: &mut SettingsApp, cx: &AppCx, model: &str) {
    let g = draw(a, cx, 1280, 720);
    let hit = g.hits.iter().find(|h| h.label == model && matches!(h.ev, Event::Sub(_, Key::Char('L')))).unwrap_or_else(|| panic!("no chip for {model}"));
    let ev = hit.ev.clone();
    a.handle(&ev, cx);
}

fn pick(a: &mut SettingsApp, cx: &AppCx, filter: &str) {
    assert!(matches!(a.modal, Some(Modal::Pick(..))), "a picker is open ({})", status(a));
    typ(a, cx, filter);
    key(a, cx, Key::Enter);
}

#[test]
fn detect_finds_lemonade_on_this_machine_adds_it_once_and_never_touches_a_provider_already_there() {
    let fake = serve();
    let cx = admin("lem1");
    let mut a = SettingsApp::new(&cx);
    lemonade::set_local_ports_for_tests(Some((unused_port(), vec![unused_port(), fake.port])));
    key(&mut a, &cx, Key::Char('f'));
    settle(&mut a, &cx);
    assert_eq!(status(&a), format!("Lemonade found at {} (version 2026.42.1); added to Models as lemonade-local.", fake.base()));
    let p = cx.services.settings.lock().unwrap().get().provider("lemonade-local").cloned().unwrap();
    assert_eq!((p.kind, p.models.clone()), (Kind::Lemonade, vec!["Qwen3-0.6B-GGUF".to_string(), "Phi-4-Multimodal-GGUF".to_string()]));
    assert_eq!(p.model_kinds.get("SD-Turbo").map(String::as_str), Some("image"));
    // Detect again: nothing new, the person's edits stay
    cx.services.settings.lock().unwrap().update(|s| s.providers[0].default_model = Some("Qwen3-0.6B-GGUF".into())).unwrap();
    key(&mut a, &cx, Key::Char('f'));
    settle(&mut a, &cx);
    lemonade::set_local_ports_for_tests(None);
    assert!(status(&a).ends_with("already in Models as lemonade-local."), "{}", status(&a));
    let ps = cx.services.settings.lock().unwrap().get().providers.clone();
    assert_eq!((ps.len(), ps[0].default_model.as_deref()), (1, Some("Qwen3-0.6B-GGUF")));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.provider.detect")));
}

#[test]
fn the_models_page_shows_lemonade_models_by_type_and_every_chip_and_button_is_on_screen_at_four_sizes() {
    let (cx, mut a, _fake) = with_lemonade("lem2");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{} off screen at {w}x{h}: {r:?}", hit.label);
        }
        let chips: Vec<&str> = g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(_, Key::Char('L')))).map(|h| h.label.as_str()).collect();
        // the full list is always one button away
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('l'))), "All models at {w}x{h}");
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('f'))), "Detect at {w}x{h}");
        if h >= 720 {
            for m in ["Qwen3-0.6B-GGUF", "Phi-4-Multimodal-GGUF", "nomic-embed-text-v1-GGUF", "SD-Turbo", "SDXL-Base-1.0", "Whisper-Base", "kokoro-v1"] {
                assert!(chips.contains(&m), "{m} at {w}x{h}: {chips:?}");
            }
            // chips of one section never overlap each other
            let rects: Vec<R> = g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(_, Key::Char('L')))).map(|h| h.rect).collect();
            for (i, r) in rects.iter().enumerate() {
                for q in &rects[i + 1..] {
                    assert!(r.x + r.w <= q.x + 0.5 || q.x + q.w <= r.x + 0.5 || r.y + r.h <= q.y + 0.5 || q.y + q.h <= r.y + 0.5, "overlap at {w}x{h}");
                }
            }
        }
    }
    // the picker holds every model with its type, state and size
    key(&mut a, &cx, Key::Char('l'));
    let Some(Modal::Pick(PickKind::Lemonade(_), p)) = &a.modal else { panic!("model picker") };
    assert!(p.title.contains("lemonade-local"));
    key(&mut a, &cx, Key::Esc);
}

#[test]
fn quick_actions_make_a_model_the_chat_model_embeddings_tales_picture_model_reading_voice_and_speech_to_text() {
    let (cx, mut a, fake) = with_lemonade("lem3");
    click_chip(&mut a, &cx, "Qwen3-0.6B-GGUF");
    pick(&mut a, &cx, "chat");
    assert_eq!(cx.services.settings.lock().unwrap().get().chat.clone().map(|c| c.model), Some("Qwen3-0.6B-GGUF".into()));
    click_chip(&mut a, &cx, "nomic-embed-text-v1-GGUF");
    pick(&mut a, &cx, "embeddings");
    assert_eq!(cx.services.settings.lock().unwrap().get().embedding.clone().map(|c| c.model), Some("nomic-embed-text-v1-GGUF".into()));
    // TALES pictures: the choice is the person's own, and the picture path draws with it
    click_chip(&mut a, &cx, "SD-Turbo");
    pick(&mut a, &cx, "TALES");
    assert!(status(&a).contains("TALES pictures are drawn with SD-Turbo"), "{}", status(&a));
    let prefs = st::StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir()).load_prefs();
    let found = cx.services.image_models();
    let plan = st::plan_model(prefs.picture_model.as_ref(), &found, "picture model");
    let st::ModelPlan::Chosen(c) = plan else { panic!("the chosen picture model is used: {plan:?}") };
    let (_, model, bytes, _) = cx.services.generate_image(Some((&c.provider, &c.model)), "a fox", "1024x1024").unwrap();
    assert_eq!(model, "SD-Turbo");
    assert_eq!(iyakki_agent::scene::reply::validate(bytes, false, "the picture model").unwrap().ext, "png");
    assert!(fake.bodies("/api/v1/images/generations")[0].contains("\"size\":\"512x512\""));
    // the reading voice, with a Kokoro voice
    click_chip(&mut a, &cx, "kokoro-v1");
    pick(&mut a, &cx, "voice");
    pick(&mut a, &cx, "af_heart");
    let tts = cx.services.settings.lock().unwrap().get().voice.tts.clone();
    assert_eq!(tts, Some(SpeechEngine::Http { url: format!("{}/audio/speech", fake.base()), model: "kokoro-v1".into(), key_ref: None, voice: Some("af_heart".into()), language: None }));
    let wav = cx.services.synthesize_text("Once upon a time.").unwrap();
    assert!(wav.starts_with(b"RIFF"));
    // speech-to-text
    click_chip(&mut a, &cx, "Whisper-Base");
    pick(&mut a, &cx, "speech");
    assert_eq!(cx.services.transcribe_samples(&voice::tone(300.0, 300)).unwrap(), "hello lemonade");
    let log = cx.audit.recent(40).join("\n");
    assert!(log.contains("settings.chat_model") && log.contains("story.picture_model") && log.contains("tts lemonade") && log.contains("stt lemonade"), "{log}");
}

#[test]
fn pull_and_load_ask_first_on_a_card_naming_the_model_and_its_size() {
    let (cx, mut a, fake) = with_lemonade("lem4");
    // not downloaded: the only action is Pull, and it asks first
    click_chip(&mut a, &cx, "SDXL-Base-1.0");
    pick(&mut a, &cx, "Pull");
    let Some(Modal::Confirm(ConfirmKind::Lemonade(_), text)) = &a.modal else { panic!("an approval card") };
    assert!(text.contains("SDXL-Base-1.0") && text.contains("6.9 GB") && text.contains("127.0.0.1"), "{text}");
    key(&mut a, &cx, Key::Esc);
    assert!(fake.bodies("/api/v1/pull").is_empty(), "nothing is downloaded without a yes");
    click_chip(&mut a, &cx, "SDXL-Base-1.0");
    pick(&mut a, &cx, "Pull");
    key(&mut a, &cx, Key::Char('y'));
    for _ in 0..400 {
        if cx.services.pull_status().iter().any(|p| p.done || p.error.is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cx.services.pull_status()[0].done, "{:?}", cx.services.pull_status());
    assert!(fake.bodies("/api/v1/pull")[0].contains("\"model_name\":\"SDXL-Base-1.0\""));
    // load asks too
    click_chip(&mut a, &cx, "SD-Turbo");
    pick(&mut a, &cx, "Load");
    let Some(Modal::Confirm(_, text)) = &a.modal else { panic!("an approval card") };
    assert!(text.contains("SD-Turbo") && text.contains("2.6 GB"), "{text}");
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert_eq!(status(&a), "Loaded model: SD-Turbo");
    let log = cx.audit.recent(30).join("\n");
    assert!(log.contains("settings.lemonade.pull") && log.contains("settings.lemonade.load"));
    // someone who may not change settings sees the models but cannot act on them
    let mut user = cx.clone();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = SettingsApp::new(&user);
    click_chip(&mut b, &user, "SD-Turbo");
    assert!(b.modal.is_none() && status(&b).contains("Only an administrator"), "{}", status(&b));
}

#[test]
fn the_add_form_auto_detects_lemonade_and_offers_it_as_a_type() {
    let fake = serve();
    let cx = admin("lem5");
    let mut a = SettingsApp::new(&cx);
    key(&mut a, &cx, Key::Char('a'));
    let Some(Modal::Form(_, f)) = &mut a.modal else { panic!("the provider form") };
    let opts = options(f);
    assert!(opts.iter().any(|o| o == "Lemonade") && opts.last().is_some_and(|o| o.starts_with("Auto-detect")), "{opts:?}");
    f.set_value(0, "amd-box");
    f.fields[1].choice = opts.len() - 1;
    f.set_value(2, &format!("http://127.0.0.1:{}", fake.port));
    key(&mut a, &cx, Key::Ctrl('s'));
    settle(&mut a, &cx);
    assert!(status(&a).starts_with(&format!("Lemonade found at {} (version 2026.42.1); saved as amd-box.", fake.base())), "{}", status(&a));
    let p = cx.services.settings.lock().unwrap().get().provider("amd-box").cloned().unwrap();
    assert_eq!((p.kind, p.base_url.as_deref()), (Kind::Lemonade, Some(fake.base().as_str())));
    assert_eq!(p.model_kinds.get("kokoro-v1").map(String::as_str), Some("tts"));
    assert_eq!(Kind::Lemonade.default_base(), Some("http://localhost:13305/api/v1"));
    // an address where nothing answers says so
    key(&mut a, &cx, Key::Char('a'));
    let Some(Modal::Form(_, f)) = &mut a.modal else { panic!("the provider form") };
    f.set_value(0, "nobody");
    f.fields[1].choice = options(f).len() - 1;
    f.set_value(2, &format!("http://127.0.0.1:{}", unused_port()));
    key(&mut a, &cx, Key::Ctrl('s'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("no Ollama, Lemonade or OpenAI-compatible server answered"), "{}", status(&a));
    assert!(cx.services.settings.lock().unwrap().get().provider("nobody").is_none());
}

#[test]
fn a_lemonade_on_another_machine_is_labelled_and_reads_or_listens_only_after_a_yes_for_that_server() {
    // a server on the internet (a documentation address: nothing is ever sent to it here; choosing a voice makes no request)
    let cx = admin("lem-remote");
    let mut p = ProviderCfg::new("far-lemon", Kind::Lemonade);
    p.base_url = Some("http://192.0.2.10:13305/api/v1".into());
    p.model_kinds.insert("kokoro-v1".into(), "tts".into());
    p.model_kinds.insert("Whisper-Base".into(), "stt".into());
    cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    let mut a = SettingsApp::new(&cx);
    // the panel's heading says where it is ("Lemonade on 192.0.2.10 (on the internet)")
    assert_eq!(iyakki_agent::speechtime::locality("http://192.0.2.10:13305/api/v1").label(), "on the internet");
    click_chip(&mut a, &cx, "kokoro-v1");
    pick(&mut a, &cx, "voice");
    pick(&mut a, &cx, "af_heart");
    let Some(Modal::Confirm(ConfirmKind::Lemonade(_), text)) = &a.modal else { panic!("a card asking first ({})", status(&a)) };
    assert!(text.contains("on the internet") && text.contains("192.0.2.10") && text.contains("sends the text"), "{text}");
    key(&mut a, &cx, Key::Esc);
    assert!(cx.services.voice_settings().tts.is_none() && cx.services.voice_settings().remote_ok.is_empty(), "no yes, nothing set");
    assert!(cx.services.lemonade_auto_voice(None).is_none(), "never used by itself either");
    click_chip(&mut a, &cx, "kokoro-v1");
    pick(&mut a, &cx, "voice");
    pick(&mut a, &cx, "af_heart");
    key(&mut a, &cx, Key::Char('y'));
    assert!(matches!(cx.services.voice_settings().tts, Some(SpeechEngine::Http { ref model, .. }) if model == "kokoro-v1"), "{}", status(&a));
    assert_eq!(cx.services.voice_settings().remote_ok, vec!["http://192.0.2.10:13305".to_string()]);
    // the same server again: no second card
    click_chip(&mut a, &cx, "Whisper-Base");
    pick(&mut a, &cx, "speech");
    assert!(a.modal.is_none(), "asked once per server");
    assert!(matches!(cx.services.voice_settings().stt, Some(SpeechEngine::Http { ref model, .. }) if model == "Whisper-Base"));
    assert!(cx.audit.recent(20).join("\n").contains("settings.voice.remote_ok"));
}
