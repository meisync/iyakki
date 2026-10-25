//! Settings → Models with AMD's Lemonade Server, against a fake Lemonade on loopback: Detect, the Auto-detect type of the
//! provider form, the models by type with their quick actions (chat, embeddings, TALES pictures, reading voice,
//! speech-to-text), Load / Pull behind an approval card naming the size, and the page's geometry at four screen sizes.

use super::*;
use crate::apps::lemonade_fake::{serve, FakeLemonade};
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::lemonade;
use super::lemonade_page::LemPick;

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
    let a = SettingsApp::new(&cx).on(Tab::Models);
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
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
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
    let mut b = SettingsApp::new(&user).on(Tab::Models);
    click_chip(&mut b, &user, "SD-Turbo");
    assert!(b.modal.is_none() && status(&b).contains("Only an administrator"), "{}", status(&b));
}

#[test]
fn the_add_form_auto_detects_lemonade_and_offers_it_as_a_type() {
    let fake = serve();
    let cx = admin("lem5");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
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
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
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

// ---------------------------------------------------------------------------------------- a big catalog: scrolling and search

/// `n` catalog models of every type (none downloaded) plus the installed `kokoro-v1` (tts) and `Qwen3-0.6B-GGUF` (chat).
fn big_catalog(n: usize) -> Vec<lemonade::Model> {
    use lemonade::ModelType as T;
    let kinds = [T::Chat, T::Chat, T::Chat, T::Vision, T::Embeddings, T::Reranking, T::Image, T::SpeechToText, T::TextToSpeech];
    let mk = |id: String, kind: T, downloaded: bool, labels: &[&str]| lemonade::Model { id, kind, recipe: String::new(), checkpoint: format!("org/{}", kind.key()), labels: labels.iter().map(|s| s.to_string()).collect(), downloaded, loaded: false, suggested: false, size_gb: Some(0.337 + kind as u8 as f64), image_defaults: None };
    let mut v: Vec<lemonade::Model> = (0..n.saturating_sub(2)).map(|i| mk(format!("cat-{:03}-{}", i, kinds[i % kinds.len()].key()), kinds[i % kinds.len()], false, if i % 7 == 0 { &["reasoning"] } else { &[] })).collect();
    v.push(mk("Qwen3-0.6B-GGUF".into(), T::Chat, true, &[]));
    v.push(mk("kokoro-v1".into(), T::TextToSpeech, true, &["tts", "speech"]));
    v
}

fn with_catalog(tag: &str, n: usize) -> (AppCx, SettingsApp, FakeLemonade) {
    let (cx, a, fake) = with_lemonade(tag);
    let base = cx.services.settings.lock().unwrap().get().provider("lemonade-local").unwrap().base().unwrap();
    lemonade::remember(&lemonade::api_base(&base), &big_catalog(n));
    (cx, a, fake)
}

fn chip_labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(_, Key::Char(c)) if SettingsApp::lem_chip_index(0, c).is_some())).map(|h| h.label.clone()).collect()
}

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

#[test]
fn a_117_model_catalog_shows_the_installed_voice_first_and_every_section_is_reachable_by_scrolling_at_four_sizes() {
    let (cx, mut a, _fake) = with_catalog("lem-big", 117);
    for (w, h) in SIZES {
        a.lem_scroll = 0.0;
        a.lem_query.set("");
        let g = draw(&mut a, &cx, w, h);
        // the installed voice is a visible chip with a click region, without scrolling
        let kokoro = g.hits.iter().find(|x| x.label == "kokoro-v1").unwrap_or_else(|| panic!("kokoro-v1 visible at {w}x{h}: {:?}", chip_labels(&g)));
        assert!(kokoro.rect.w > 5.0 && kokoro.rect.h > 5.0 && kokoro.rect.y + kokoro.rect.h <= h as f32 + 0.5, "{w}x{h}");
        assert!(a.lem_max > 0.0, "117 models do not fit at {w}x{h}");
        // every model is reachable by scrolling, and no region lies outside the list's area or the screen
        let mut seen = std::collections::BTreeSet::new();
        let mut last = -1.0;
        for _ in 0..400 {
            let g = draw(&mut a, &cx, w, h);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{} off screen at {w}x{h}: {r:?}", hit.label);
            }
            let top = a.lem_rect.y;
            for hit in g.hits.iter().filter(|x| matches!(x.ev, Event::Sub(_, Key::Char(c)) if SettingsApp::lem_chip_index(0, c).is_some())) {
                assert!(hit.rect.y >= top, "chip {} above the list at {w}x{h}", hit.label);
                seen.insert(hit.label.clone());
            }
            if a.lem_scroll >= a.lem_max || a.lem_scroll == last {
                break;
            }
            last = a.lem_scroll;
            a.handle(&Event::Key(Key::PageDown), &cx);
        }
        assert_eq!(seen.len(), 117, "every model reached at {w}x{h}");
        // the wheel and PageUp go back up
        a.handle(&Event::MouseMove((a.lem_rect.x + 20.0) as i32, (a.lem_rect.y + a.lem_rect.h / 2.0) as i32), &cx);
        let before = a.lem_scroll;
        a.handle(&Event::Scroll(3), &cx);
        assert!(a.lem_scroll < before, "wheel up scrolls up");
        a.handle(&Event::Key(Key::Home), &cx);
        assert_eq!(a.lem_scroll, 0.0);
    }
}

#[test]
fn the_header_counts_installed_and_available_and_the_provider_card_never_says_0_models() {
    let (cx, mut a, _fake) = with_catalog("lem-count", 117);
    let prov = cx.services.settings.lock().unwrap().get().provider("lemonade-local").cloned().unwrap();
    assert_eq!(SettingsApp::models_summary(&cx, &prov), "2 installed · 115 available");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "kokoro-v1"));
    // no list yet: asks to read it; other providers say what they hold
    let none = ProviderCfg::new("lemonade-new", Kind::Lemonade);
    assert_eq!(SettingsApp::models_summary(&cx, &none), "models not read yet: press Test or All models");
    let mut o = ProviderCfg::new("oll", Kind::Ollama);
    assert_eq!(SettingsApp::models_summary(&cx, &o), "no chat models yet (press Models)");
    o.models = vec!["a".into(), "b".into()];
    assert_eq!(SettingsApp::models_summary(&cx, &o), "2 chat models saved");
}

#[test]
fn a_server_with_no_chat_model_installed_says_so_in_the_chat_section() {
    let (cx, mut a, _fake) = with_catalog("lem-nochat", 40);
    let base = cx.services.settings.lock().unwrap().get().provider("lemonade-local").unwrap().base().unwrap();
    let mut l = big_catalog(40);
    l.retain(|m| m.id != "Qwen3-0.6B-GGUF");
    lemonade::remember(&lemonade::api_base(&base), &l);
    let mut g = draw(&mut a, &cx, 1920, 1080);
    let mut found = false;
    for _ in 0..30 {
        found |= a.has_text("no chat model installed: pick one below to pull");
        a.handle(&Event::Key(Key::PageDown), &cx);
        g = draw(&mut a, &cx, 1920, 1080);
    }
    assert!(found, "the chat section explains itself");
}

#[test]
fn search_filters_by_name_type_and_label_counts_what_is_shown_and_esc_clears_it() {
    let (cx, mut a, _fake) = with_catalog("lem-search", 117);
    let labels = |a: &mut SettingsApp| {
        let mut all = std::collections::BTreeSet::new();
        a.lem_scroll = 0.0;
        for _ in 0..300 {
            let g = draw(a, &cx, 1920, 1080);
            all.extend(chip_labels(&g));
            if a.lem_scroll >= a.lem_max {
                break;
            }
            a.handle(&Event::Key(Key::PageDown), &cx);
        }
        all
    };
    // '/' focuses the box; typing filters live
    a.handle(&Event::Key(Key::Char('/')), &cx);
    assert!(a.lem_search);
    a.lem_scroll = 100.0;
    typ(&mut a, &cx, "KOKO");
    assert_eq!(a.lem_scroll, 0.0, "typing resets the scroll");
    assert_eq!(labels(&mut a), ["kokoro-v1".to_string()].into_iter().collect(), "by name, any case");
    // by type word
    a.lem_query.set("tts");
    let tts = labels(&mut a);
    assert!(tts.contains("kokoro-v1") && tts.iter().all(|m| m.ends_with("-tts") || m == "kokoro-v1") && tts.len() > 5, "{tts:?}");
    // by a label of the model
    a.lem_query.set("reasoning");
    let r = labels(&mut a);
    assert!(!r.is_empty() && r.iter().all(|m| m.starts_with("cat-")), "{r:?}");
    // the header counts what matches (the button bar now reserves every row it needs, so the list may show a few chips fewer than match)
    let _ = draw(&mut a, &cx, 1920, 1080);
    let shown: usize = a.lem_texts.iter().find_map(|t| t.split(" of 117 shown").next().and_then(|n| n.trim().parse().ok())).unwrap_or(0);
    assert!(shown >= r.len() && shown > 0 && shown < 117, "count in the header: {shown} for {} visible chips in {:?}", r.len(), a.lem_texts);
    // nothing matches
    a.lem_query.set("zzyzx");
    let g = draw(&mut a, &cx, 1920, 1080);
    assert!(a.has_text("no model matches “zzyzx”") && chip_labels(&g).is_empty());
    // Esc clears the text first, then leaves the box
    key(&mut a, &cx, Key::Esc);
    assert!(a.lem_query.value.is_empty() && a.lem_search);
    key(&mut a, &cx, Key::Esc);
    assert!(!a.lem_search);
    assert_eq!(labels(&mut a).len(), 117);
}

#[test]
fn a_click_on_a_filtered_chip_acts_on_that_model_and_a_catalog_past_255_models_stays_clickable() {
    let (cx, mut a, _fake) = with_catalog("lem-click", 300);
    a.lem_query.set("cat-290");
    let g = draw(&mut a, &cx, 1280, 720);
    let hit = g.hits.iter().find(|h| h.label.starts_with("cat-290")).expect("the filtered chip");
    let ev = hit.ev.clone();
    assert!(matches!(ev, Event::Sub(_, Key::Char(c)) if c != 'L'), "index above 255 uses the extended id: {ev:?}");
    let id = hit.label.clone();
    a.handle(&ev, &cx);
    let Some(Modal::Pick(PickKind::Lemonade(LemPick::Action(_, model)), p)) = &a.modal else { panic!("actions for the clicked model ({})", status(&a)) };
    assert_eq!(model, &id);
    assert!(p.title.starts_with(&id));
    key(&mut a, &cx, Key::Esc);
    // the installed voice in the same catalog: Use as reading voice
    a.lem_query.set("kokoro");
    click_chip_at(&mut a, &cx, "kokoro-v1");
    let Some(Modal::Pick(_, p)) = &a.modal else { panic!("picker") };
    assert!(p.title.starts_with("kokoro-v1 (Text-to-speech, available)"), "{}", p.title);
    pick(&mut a, &cx, "voice");
    assert!(matches!(a.modal, Some(Modal::Pick(PickKind::Lemonade(LemPick::Voice(..)), _))));
}

fn click_chip_at(a: &mut SettingsApp, cx: &AppCx, model: &str) {
    let g = draw(a, cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.label == model).unwrap_or_else(|| panic!("no chip for {model}")).ev.clone();
    a.handle(&ev, cx);
}

impl SettingsApp {
    fn has_text(&self, t: &str) -> bool {
        self.lem_texts.iter().any(|x| x.contains(t))
    }
}

// ------------------------------------------------------------------ Omni collections: Use all parts

/// A Lemonade provider whose list holds the Omni collection (`sd_installed`: whether SD-Turbo is on the server).
fn with_omni(tag: &str, sd_installed: bool) -> (AppCx, SettingsApp, FakeLemonade) {
    let (cx, a, fake) = with_lemonade(tag);
    let v = serde_json::json!({"data": [
        {"id": "LMX-Omni-5.5B-Lite", "recipe": "collection.omni", "labels": ["chat"], "size": 9.3, "downloaded": true, "system_prompt": "p", "components": ["Qwen3.5-4B-MTP-GGUF", "SD-Turbo", "Whisper-Tiny", "kokoro-v1"]},
        {"id": "Qwen3.5-4B-MTP-GGUF", "recipe": "llamacpp", "labels": ["vision"], "size": 3.4, "downloaded": true},
        {"id": "SD-Turbo", "recipe": "sd-cpp", "labels": ["image"], "size": 2.6, "downloaded": sd_installed},
        {"id": "Whisper-Tiny", "recipe": "whispercpp", "labels": ["transcription"], "size": 0.075, "downloaded": true},
        {"id": "kokoro-v1", "recipe": "kokoro", "labels": ["tts"], "size": 0.354, "downloaded": true},
        {"id": "Qwen3-0.6B-GGUF", "recipe": "llamacpp", "size": 0.38, "downloaded": true, "labels": ["reasoning"]}
    ]});
    lemonade::remember(&fake.base(), &lemonade::parse_models(&v, &[]));
    lemonade::remember_collections(&fake.base(), &lemonade::parse_collections(&v));
    (cx, a, fake)
}

fn picker_items(a: &SettingsApp) -> Vec<String> {
    match &a.modal {
        Some(Modal::Pick(_, p)) => p.visible().into_iter().cloned().collect(),
        _ => vec![],
    }
}

fn picture_model(cx: &AppCx) -> Option<String> {
    st::StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir()).load_prefs().picture_model.map(|m| m.model)
}

#[test]
fn a_collection_shows_as_an_omni_collection_with_its_parts_and_use_all_parts_sets_all_four_and_undo_restores() {
    let (cx, mut a, _fake) = with_omni("omni1", true);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "LMX-Omni-5.5B-Lite"), "the collection has a chip");
    click_chip(&mut a, &cx, "LMX-Omni-5.5B-Lite");
    let items = picker_items(&a);
    for want in ["chat+vision Qwen3.5-4B-MTP-GGUF · installed", "pictures SD-Turbo · installed", "speech in Whisper-Tiny · installed", "speech out kokoro-v1 · installed"] {
        assert!(items.iter().any(|i| i.trim() == want), "{want} in {items:?}");
    }
    assert!(items.iter().any(|i| i == "Use all parts…") && !items.iter().any(|i| i.starts_with("Install missing")) && !items.iter().any(|i| i.starts_with("Undo")), "{items:?}");
    let before_chat = cx.services.settings.lock().unwrap().get().chat.clone();
    let start = (cx.services.settings.lock().unwrap().get().voice.tts.clone(), picture_model(&cx));
    pick(&mut a, &cx, "Use all");
    let Some(Modal::Confirm(ConfirmKind::Lemonade(_), text)) = &a.modal else { panic!("a review card") };
    assert!(text.contains("lemonade-local") && text.contains("127.0.0.1") && text.contains("this computer") && text.contains("speech out kokoro-v1") && text.contains("Undo"), "{text}");
    key(&mut a, &cx, Key::Esc);
    assert_eq!(cx.services.settings.lock().unwrap().get().chat, before_chat, "nothing changes without a yes");
    click_chip(&mut a, &cx, "LMX-Omni-5.5B-Lite");
    pick(&mut a, &cx, "Use all");
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("chat Qwen3.5-4B-MTP-GGUF") && status(&a).contains("pictures SD-Turbo"), "{}", status(&a));
    {
        let s = cx.services.settings.lock().unwrap();
        let g = s.get();
        assert_eq!(g.chat.as_ref().map(|c| (c.provider.as_str(), c.model.as_str())), Some(("lemonade-local", "Qwen3.5-4B-MTP-GGUF")));
        assert!(g.voice.stt.is_some() && g.voice.tts.is_some());
    }
    assert_eq!(picture_model(&cx).as_deref(), Some("SD-Turbo"));
    assert!(cx.audit.recent(30).join("\n").contains("settings.lemonade.use_all_parts"));
    // Undo is offered on the collection and puts the old choices back
    click_chip(&mut a, &cx, "LMX-Omni-5.5B-Lite");
    assert!(picker_items(&a).iter().any(|i| i == "Undo Use all parts"));
    pick(&mut a, &cx, "Undo");
    assert_eq!(cx.services.settings.lock().unwrap().get().chat, before_chat);
    assert_eq!((cx.services.settings.lock().unwrap().get().voice.tts.clone(), picture_model(&cx)), start);
}

#[test]
fn a_missing_part_shows_in_the_list_blocks_use_all_parts_and_offers_to_install_with_the_size() {
    let (cx, mut a, fake) = with_omni("omni2", false);
    click_chip(&mut a, &cx, "LMX-Omni-5.5B-Lite");
    let items = picker_items(&a);
    assert!(items.iter().any(|i| i.trim() == "pictures SD-Turbo · missing (2.6 GB)"), "{items:?}");
    assert!(items.iter().any(|i| i.starts_with("Install missing parts (2.6 GB)")), "{items:?}");
    pick(&mut a, &cx, "Use all");
    let Some(Modal::Confirm(ConfirmKind::Lemonade(_), text)) = &a.modal else { panic!("an install card") };
    assert!(text.contains("SD-Turbo") && text.contains("2.6 GB") && text.contains("127.0.0.1"), "{text}");
    let start = (cx.services.settings.lock().unwrap().get().voice.tts.clone(), picture_model(&cx));
    key(&mut a, &cx, Key::Char('y'));
    for _ in 0..400 {
        if cx.services.pull_status().iter().any(|p| p.done || p.error.is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fake.bodies("/api/v1/pull")[0].contains("\"model_name\":\"LMX-Omni-5.5B-Lite\""));
    assert_eq!((cx.services.settings.lock().unwrap().get().voice.tts.clone(), picture_model(&cx)), start, "nothing is set while a part is missing");
}

#[test]
fn only_an_administrator_can_use_all_parts_and_a_plain_model_has_no_such_button() {
    let (cx, mut a, _fake) = with_omni("omni3", true);
    let tts = cx.services.settings.lock().unwrap().get().voice.tts.clone();
    click_chip(&mut a, &cx, "SD-Turbo");
    assert!(!picker_items(&a).iter().any(|i| i.contains("Use all parts") || i.contains("Install missing")), "{:?}", picker_items(&a));
    key(&mut a, &cx, Key::Esc);
    let mut user = cx.clone();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = SettingsApp::new(&user).on(Tab::Models);
    click_chip(&mut b, &user, "LMX-Omni-5.5B-Lite");
    assert!(b.modal.is_none() && status(&b).contains("Only an administrator"), "{}", status(&b));
    assert_eq!(cx.services.settings.lock().unwrap().get().voice.tts, tts);
}
