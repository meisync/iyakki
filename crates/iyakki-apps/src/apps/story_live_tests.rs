use iyakki_agent::facets_h::RealServices as _;
// Live checks of the TALES tile against a real Lemonade server on the LAN (items of the `tests` module of story.rs, included
// from there). Every test is `#[ignore = "live service"]`; run with
//
//   IYAKKI_LIVE_LEMONADE_URL=http://host:port IYAKKI_LIVE_LEMONADE_KEY_FILE=/absolute/key/file [IYAKKI_LIVE_EVIDENCE=dir]
//   cargo test -p iyakki-shell --lib story::tests::live_ -- --ignored --test-threads=1
//
// The story itself is told by the test's scripted writer; the pictures and the voice are the real server's. The key is read only
// through the `file:` reference of the production resolver; evidence is redacted before it is written.

fn live_env() -> (String, String) {
    (std::env::var("IYAKKI_LIVE_LEMONADE_URL").expect("set IYAKKI_LIVE_LEMONADE_URL"), std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").expect("set IYAKKI_LIVE_LEMONADE_KEY_FILE"))
}

fn live_note(row: &str, name: &str, text: &str) {
    let (url, key_file) = live_env();
    let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
    let mut t = text.to_string();
    for h in [key, iyakki_shared::lemonade::origin(&url).unwrap_or_default(), url.trim_end_matches('/').to_string()] {
        if !h.is_empty() {
            t = t.replace(h.as_str(), "<redacted>");
        }
    }
    eprintln!("[{row}] {name}: {}", t.lines().next().unwrap_or(""));
    if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
        let d = Path::new(&d).join(row);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(format!("{name}.txt")), t);
    }
}

/// A TALES tile whose services reach the real server through a Lemonade provider called `lemon` (key from the file, or a wrong key).
fn live_story(tag: &str, wrong_key: bool) -> (StoryApp, AppCx, PathBuf) {
    let (url, key_file) = live_env();
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make_up(tag, &t, up);
    let mut p = iyakki_shared::providers::ProviderCfg::new("lemon", iyakki_shared::providers::Kind::Lemonade);
    p.base_url = Some(url);
    p.key_ref = Some(format!("file:{key_file}"));
    cx.settings().lock().unwrap().upsert_provider(p.clone()).unwrap();
    cx.services.refresh_lemonade("lemon").expect("the typed list of the real server");
    if wrong_key {
        // the list was read with the right key (so the picture and voice models are known); every later call has the wrong one
        let w = std::env::temp_dir().join(format!("iyakki-live-wrong-{tag}"));
        std::fs::write(&w, "not-the-key-0000").unwrap();
        let mut cfg = cx.settings().lock().unwrap().get().provider("lemon").cloned().unwrap();
        cfg.key_ref = Some(format!("file:{}", w.display()));
        cx.settings().lock().unwrap().upsert_provider(cfg).unwrap();
    }
    a.refresh_tts(&cx);
    a.test_memory = Some(lots_of_memory());
    a.confirm_secs = f64::MAX;
    (a, cx, home)
}

fn live_png_size(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() > 24 && b[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).then(|| (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap())))
}

#[test]
#[ignore = "live service"]
fn live_tales_scenes_become_real_pictures_stored_beside_the_story() {
    for model in ["SD-Turbo", "SDXL-Base-1.0"] {
        let (mut a, cx, home) = live_story(&format!("live-pics-{}", model.to_lowercase()), false);
        a.opts.pic.model = Some(ModelChoice { provider: "lemon".into(), model: model.into() });
        let t0 = Instant::now();
        write_story(&mut a, &cx, "a fox");
        assert_eq!(a.pics.len(), 2, "two scenes");
        until(&mut a, &cx, "the real pictures", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
        let took = t0.elapsed();
        assert!(a.pics.iter().all(|p| p.state == PicState::Ready), "{:?}", a.pics.iter().map(|p| p.state.clone()).collect::<Vec<_>>());
        let mut lines = vec![format!("{model}: story + {} pictures in {} ms", a.pics.len(), took.as_millis())];
        let id = a.saved_id().unwrap();
        let dir = home.join(format!("Stories/{id}"));
        for (i, p) in a.pics.iter().enumerate() {
            let bytes = p.bytes.as_ref().unwrap();
            assert_eq!(p.ext, "png", "format found by its magic bytes");
            assert_eq!(p.how, format!("images via {model}"));
            let (w, h) = live_png_size(bytes).expect("a PNG");
            assert!(w >= 256 && h >= 256, "{w}x{h}");
            assert!(bytes.len() > 20_000, "a picture, not a blank: {}", bytes.len());
            let file = dir.join(format!("pics/{:02}.png", i + 1));
            assert_eq!(std::fs::read(&file).unwrap().len(), bytes.len(), "stored beside the story");
            lines.push(format!("picture {}: {w}x{h} png, {} bytes, stored as {}", i + 1, bytes.len(), file.file_name().unwrap().to_string_lossy()));
        }
        let index = std::fs::read_to_string(dir.join("pics/index.json")).unwrap();
        assert!(index.contains("image") && index.contains(model), "{index}");
        let log = std::fs::read_to_string(dir.join("pictures.log")).unwrap();
        assert!(log.contains(&format!("images via {model}")) && log.contains("RESULT: png accepted"), "{log}");
        let ev = a.pic_log_events();
        let reqs: Vec<&PicEvent> = ev.iter().filter(|e| e.kind == plk::REQUEST).collect();
        assert_eq!(reqs.len(), 2);
        assert!(reqs.iter().all(|e| e.status == Some(200) && e.model == model), "{reqs:?}");
        lines.push(format!("log events: {}; request times (ms): {:?}", ev.len(), reqs.iter().map(|e| e.ms).collect::<Vec<_>>()));
        let md = std::fs::read_to_string(dir.join("story.md")).unwrap();
        assert!(md.contains("pics/01.png") && md.contains("pics/02.png"), "{md}");
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert_layout(&g, area, "story with real pictures");
        assert!(!log.contains(&std::fs::read_to_string(live_env().1).unwrap().trim().to_string()), "the key is not in the picture log");
        live_note("F-TALES-13", &format!("tile-pictures-{model}"), &lines.join("\n"));
        if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
            let d = Path::new(&d).join("F-TALES-13");
            let _ = std::fs::copy(dir.join("pics/01.png"), d.join(format!("tile-{model}-01.png")));
            let _ = std::fs::copy(dir.join("pictures.log"), d.join(format!("tile-{model}-pictures.log")));
        }
    }
}

#[test]
#[ignore = "live service"]
fn live_tales_a_picture_model_that_is_not_on_the_server_is_not_called() {
    let (mut a, cx, _home) = live_story("live-pics-notdl", false);
    a.opts.pic.model = Some(ModelChoice { provider: "lemon".into(), model: "SDXL-Turbo".into() });
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures to stop", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
    assert!(why.contains("SDXL-Turbo is not downloaded"), "{why}");
    let ev = a.pic_log_events();
    assert!(ev.iter().any(|e| e.kind == plk::NOT_DOWNLOADED && e.decision.contains("Nothing was asked")), "{ev:?}");
    assert!(!ev.iter().any(|e| e.kind == plk::REQUEST));
    live_note("F-TALES-13", "tile-model-not-downloaded", &format!("{why}\nlog: {}", ev.iter().map(|e| e.decision.clone()).collect::<Vec<_>>().join(" | ")));
}

#[test]
#[ignore = "live service"]
fn live_tales_pictures_with_a_wrong_key_fall_back_and_say_so() {
    let (mut a, cx, home) = live_story("live-pics-wrongkey", true);
    a.opts.pic.model = Some(ModelChoice { provider: "lemon".into(), model: "SD-Turbo".into() });
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures to end", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
    let ev = a.pic_log_events();
    let req = ev.iter().find(|e| e.kind == plk::REQUEST).unwrap_or_else(|| panic!("{ev:?}"));
    assert!(req.status == Some(401) || req.error.contains("401"), "{req:?}");
    let states: Vec<String> = a.pics.iter().map(|p| format!("{:?} {} {}", p.state, p.ext, p.how)).collect();
    let log = std::fs::read_to_string(home.join(format!("Stories/{}/pictures.log", a.saved_id().unwrap()))).unwrap_or_default();
    assert!(!log.contains("not-the-key-0000"), "the wrong key is not in the log either");
    live_note("F-TALES-13", "tile-pictures-wrong-key", &format!("request status {:?}: {}\npictures: {states:?}\nnote: {:?}", req.status, req.error, a.note_text()));
}

/// A speaker that records what it was given and takes `secs` seconds over it.
fn live_recording_player(dir: &Path, secs: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join("rec.sh");
    std::fs::write(&script, format!("#!/bin/sh\ncat > '{d}'/piece.$$.wav\nsleep {secs}\necho p >> '{d}'/played.log\n", d = dir.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script.display().to_string()
}

fn live_pieces(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
#[ignore = "live service"]
fn live_tales_reads_the_story_aloud_with_kokoro_pauses_resumes_and_stops() {
    let _route = iyakki_agent::mixer::pin_command_route();
    let (url, key_file) = live_env();
    let (mut a, cx, _home) = live_story("live-read", false);
    let dir = temp_dir("live-read-eng");
    let _ = std::fs::remove_dir_all(&dir);
    let play = live_recording_player(&dir, "2");
    cx.services
        .settings()
        .lock()
        .unwrap()
        .update(|s| {
            s.voice.tts = Some(iyakki_shared::lemonade::tts_engine(&url, "kokoro-v1", None, Some(format!("file:{key_file}"))));
            s.voice.play_cmd = play.clone();
        })
        .unwrap();
    cx.services.allow_speech_server(&iyakki_shared::lemonade::api_base(&url)).unwrap();
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    assert_eq!(a.reader_status(), Status::Idle);
    let t0 = Instant::now();
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the first real piece to reach the speaker", |a| !live_pieces(&dir).is_empty() && a.reader_status() == Status::Playing);
    let first_ms = t0.elapsed().as_millis();
    let mut lines = vec![format!("first piece at the speaker after {first_ms} ms")];
    // pause, then resume at the same piece
    click(&mut a, &cx, T_PLAY);
    assert_eq!(a.reader_status(), Status::Paused);
    let unit = a.reader.snapshot().unit;
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "resumed", |a| a.reader_status() == Status::Playing);
    assert_eq!(a.reader.snapshot().unit, unit, "resume continues at the same place");
    lines.push(format!("pause and resume at unit {unit}: ok"));
    // stop forgets the place
    click(&mut a, &cx, T_STOP);
    assert_eq!(a.reader_status(), Status::Idle);
    // read the whole story to its end
    let before = live_pieces(&dir).len();
    click(&mut a, &cx, T_PLAY);
    let chunks = a.chunks.len().max(1);
    until(&mut a, &cx, "the whole story to be read", |a| matches!(a.reader_status(), Status::Done) && live_pieces(&dir).len() >= before + chunks);
    let pieces = live_pieces(&dir);
    let mut secs_total = 0.0;
    let (mut whole, mut cut) = (0, 0);
    for p in &pieces {
        let bytes = std::fs::read(p).unwrap();
        // a piece the speaker was stopped in the middle of (pause, stop) is short or empty: only the whole ones are judged
        let Ok(w) = iyakki_shared::voice::parse_wav(&bytes) else {
            cut += 1;
            continue;
        };
        whole += 1;
        assert_eq!((w.rate, w.channels), (48_000, 1), "converted for the card");
        assert!(iyakki_shared::voice::peak(&w.samples) > 1500, "audible");
        secs_total += w.samples.len() as f32 / w.rate as f32;
    }
    assert!(whole >= chunks, "{whole} whole pieces for {chunks} chunks");
    lines.push(format!("{} chunks of the story; {} pieces reached the speaker ({whole} whole, {cut} cut by pause or stop), {secs_total:.1} s of speech", chunks, pieces.len()));
    live_note("F-VOICE-17", "tile-read-aloud-kokoro", &lines.join("\n"));
}

#[test]
#[ignore = "live service"]
fn live_tales_reading_with_a_wrong_key_says_what_is_wrong() {
    let _route = iyakki_agent::mixer::pin_command_route();
    let (url, _) = live_env();
    let (mut a, cx, _home) = live_story("live-read-wrong", true);
    let dir = temp_dir("live-read-wrong-eng");
    let _ = std::fs::remove_dir_all(&dir);
    let play = live_recording_player(&dir, "1");
    let wrong = {
        let p = std::env::temp_dir().join("iyakki-live-wrong-live-read-wrong");
        format!("file:{}", p.display())
    };
    cx.services
        .settings()
        .lock()
        .unwrap()
        .update(|s| {
            s.voice.tts = Some(iyakki_shared::lemonade::tts_engine(&url, "kokoro-v1", None, Some(wrong)));
            s.voice.play_cmd = play;
        })
        .unwrap();
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the failure", |a| matches!(a.reader_status(), Status::Failed(_)));
    let Status::Failed(why) = a.reader_status() else { unreachable!() };
    assert!(why.contains("401"), "{why}");
    assert!(live_pieces(&dir).is_empty(), "nothing reached the speaker");
    live_note("F-VOICE-17", "tile-read-wrong-key", &why);
}

// ------------------------------------------------------------------ the writing model is real too (qwen3-8b)

const LIVE_TEXT_MODEL: &str = "qwen3-8b";

/// Like `live_story`, but the story, the character sheet and the scene choices are written by the real text model.
fn live_story_text(tag: &str) -> (StoryApp, AppCx, PathBuf) {
    let (a, mut cx, home) = live_story(tag, false);
    let client = cx.services.real().client_for("lemon", false).expect("a client for the real server");
    cx.chat = Arc::new(iyakki_agent::providers::ProviderChat { client, model: LIVE_TEXT_MODEL.into(), streamer: None });
    cx.settings().lock().unwrap().update(|s| s.chat = Some(ModelChoice { provider: "lemon".into(), model: LIVE_TEXT_MODEL.into() })).unwrap();
    (a, cx, home)
}

fn live_wait_long(a: &mut StoryApp, cx: &AppCx, secs: u64, what: &str, f: impl Fn(&mut StoryApp) -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        a.handle(&Event::Tick, cx);
        if f(a) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}: error {:?}, pictures {:?}, note {:?}", a.error, a.pics.iter().map(|p| p.state.clone()).collect::<Vec<_>>(), a.note);
}

/// Things that must never reach a reader: the model's thinking, a code fence, a stray JSON brace, a chat label.
fn live_leaks(text: &str) -> Vec<&'static str> {
    let mut v = Vec::new();
    let l = text.to_lowercase();
    for (needle, name) in [("<think", "think tag"), ("</think", "closing think tag"), ("```", "code fence"), ("\"title\":", "JSON key"), ("<|", "special token"), ("as an ai", "AI disclaimer"), ("here is the story", "chat preamble"), ("here's the story", "chat preamble")] {
        if l.contains(needle) {
            v.push(name);
        }
    }
    v
}

fn live_save(row: &str, name: &str, bytes: &[u8]) {
    if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
        let d = Path::new(&d).join(row);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(name), bytes);
    }
}

#[test]
#[ignore = "live service"]
fn live_tales_qwen3_writes_the_story_and_the_cartoon_composer_draws_it() {
    let mut report = Vec::new();
    for attempt in 1..=3 {
        let (mut a, cx, home) = live_story_text(&format!("live-qwen-cartoon-{attempt}"));
        for _ in 0..3 {
            click(&mut a, &cx, T_DRAWMODE);
        }
        assert_eq!(a.options().drawing, st::DrawingMode::Cartoon);
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a small fox who finds the moon in a pond");
        let t0 = Instant::now();
        key(&mut a, &cx, Key::Ctrl('n'));
        live_wait_long(&mut a, &cx, 400, "the story", |a| !a.is_writing());
        let write_ms = t0.elapsed().as_millis();
        let text = a.story_text();
        let leaks = live_leaks(&text);
        report.push(format!("attempt {attempt}: story in {write_ms} ms, title {:?}, {} blocks, {} chars, leaks {leaks:?}, error {:?}", a.story.title, a.story.blocks.len(), text.len(), a.error));
        if let Some(id) = a.saved_id() {
            if let Ok(md) = std::fs::read(home.join(format!("Stories/{id}/story.md"))) {
                live_save("F-TALES-16", &format!("story-attempt{attempt}.md"), &md);
            }
        }
        if a.error.is_some() || a.story.blocks.len() < 3 || !leaks.is_empty() || a.story.title.trim().is_empty() {
            continue;
        }
        let t1 = Instant::now();
        live_wait_long(&mut a, &cx, 600, "the cartoon pictures", |a| !a.pics.is_empty() && a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
        let draw_ms = t1.elapsed().as_millis();
        let hows: Vec<String> = a.pics.iter().map(|p| format!("{} .{}", p.how, p.ext)).collect();
        let ready = a.pics.iter().filter(|p| p.state == PicState::Ready).count();
        report.push(format!("   pictures: {ready}/{} ready in {draw_ms} ms; how: {hows:?}", a.pics.len()));
        let id = a.saved_id().unwrap();
        let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap_or_default();
        live_save("F-TALES-16", &format!("pictures-attempt{attempt}.log"), log.as_bytes());
        for (i, p) in a.pics.iter().enumerate() {
            if let Some(b) = &p.bytes {
                live_save("F-TALES-16", &format!("scene-attempt{attempt}-{}.svg", i + 1), b);
                assert!(p.ext == "png" || p.ext == "svg", "{} {:?}", p.ext, p.how);
                if p.ext == "svg" {
                    assert!(!live_leaks(&String::from_utf8_lossy(b)).iter().any(|l| *l == "think tag" || *l == "code fence"), "nothing of the model's chatter inside the picture");
                }
            }
        }
        let model_drawn = hows.iter().filter(|h| h.starts_with("cartoon (built-in)")).count();
        let sketch = hows.iter().filter(|h| h.to_lowercase().contains("sketch")).count();
        report.push(format!("   scenes drawn from the model's own JSON: {model_drawn}, honest sketch fallbacks: {sketch}"));
        live_note("F-TALES-16", "qwen3-cartoon-composer", &report.join("\n"));
        assert_eq!(ready, a.pics.len(), "every scene has a picture, by the model's JSON or by the honest fallback");
        return;
    }
    live_note("F-TALES-16", "qwen3-cartoon-composer", &report.join("\n"));
    panic!("three attempts gave no clean story and pictures:\n{}", report.join("\n"));
}

#[test]
#[ignore = "live service"]
fn live_tales_qwen3_draws_rich_svg_pictures_that_pass_the_checks_or_fall_back_honestly() {
    let mut report = Vec::new();
    for attempt in 1..=3 {
        let (mut a, cx, home) = live_story_text(&format!("live-qwen-rich-{attempt}"));
        for _ in 0..2 {
            click(&mut a, &cx, T_DRAWMODE);
        }
        assert_eq!(a.options().drawing, st::DrawingMode::Rich);
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a lighthouse keeper and a friendly whale");
        key(&mut a, &cx, Key::Ctrl('n'));
        live_wait_long(&mut a, &cx, 400, "the story", |a| !a.is_writing());
        if a.error.is_some() || a.story.blocks.len() < 2 {
            report.push(format!("attempt {attempt}: no story ({:?})", a.error));
            continue;
        }
        let t1 = Instant::now();
        live_wait_long(&mut a, &cx, 900, "the rich drawings", |a| !a.pics.is_empty() && a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
        let hows: Vec<String> = a.pics.iter().map(|p| p.how.clone()).collect();
        let id = a.saved_id().unwrap();
        let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap_or_default();
        live_save("F-TALES-15", &format!("pictures-attempt{attempt}.log"), log.as_bytes());
        for (i, p) in a.pics.iter().enumerate() {
            if let Some(b) = &p.bytes {
                live_save("F-TALES-15", &format!("scene-attempt{attempt}-{}.svg", i + 1), b);
                let s = String::from_utf8_lossy(b);
                assert!(s.contains("<svg") && !s.contains("```") && !s.contains("<think"), "{}", &s[..s.len().min(120)]);
            }
        }
        let rich = hows.iter().filter(|h| h.starts_with("rich SVG")).count();
        report.push(format!("attempt {attempt}: {} pictures in {} ms; how: {hows:?}; rich drawings accepted: {rich}", a.pics.len(), t1.elapsed().as_millis()));
        if a.pics.iter().all(|p| p.state == PicState::Ready) {
            live_note("F-TALES-15", "qwen3-rich-svg", &report.join("\n"));
            return;
        }
    }
    live_note("F-TALES-15", "qwen3-rich-svg", &report.join("\n"));
    panic!("{}", report.join("\n"));
}

// Not a live test (no server needed): the fault the live run showed, kept as a plain test beside it.
#[test]
fn drawing_rich_svg_or_cartoon_is_kept_even_when_a_picture_model_exists() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-draw-vs-images", &t);
    add_providers(&cx, &["flux"]);
    a.refresh_tts(&cx);
    assert!(!a.image_models.is_empty() || iyakki_shared::models::is_image_model("flux"), "a picture model exists");
    let auto = a.pictures_status(&cx);
    for _ in 0..3 {
        click(&mut a, &cx, T_DRAWMODE);
    }
    assert_eq!(a.options().drawing, st::DrawingMode::Cartoon);
    assert_eq!(a.pictures_status(&cx), "Pictures: cartoon (built-in)", "was {auto:?} before the choice");
    write_story(&mut a, &cx, "a fox");
    assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings), "cartoon is a drawing, not the picture model");
}

#[test]
#[ignore = "live service"]
fn live_tales_qwen3_story_with_sd_turbo_pictures_has_clean_prompts_and_a_clean_saved_story() {
    let (mut a, cx, home) = live_story_text("live-qwen-images");
    a.opts.pic.model = Some(ModelChoice { provider: "lemon".into(), model: "SD-Turbo".into() });
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a small fox who finds the moon in a pond");
    key(&mut a, &cx, Key::Ctrl('n'));
    live_wait_long(&mut a, &cx, 400, "the story", |a| !a.is_writing());
    live_wait_long(&mut a, &cx, 600, "the pictures", |a| !a.pics.is_empty() && a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
    assert!(a.pics.iter().all(|p| p.state == PicState::Ready), "{:?}", a.pics.iter().map(|p| p.state.clone()).collect::<Vec<_>>());
    let id = a.saved_id().unwrap();
    let dir = home.join(format!("Stories/{id}"));
    let md = std::fs::read_to_string(dir.join("story.md")).unwrap();
    assert!(live_leaks(&md).is_empty(), "{:?}", live_leaks(&md));
    let log = std::fs::read_to_string(dir.join("pictures.log")).unwrap();
    live_save("F-TALES-13", "qwen3-sd-turbo-pictures.log", log.as_bytes());
    live_save("F-TALES-13", "qwen3-sd-turbo-story.md", md.as_bytes());
    live_save("F-TALES-13", "qwen3-sd-turbo-01.png", &std::fs::read(dir.join("pics/01.png")).unwrap());
    let prompts: Vec<&str> = log.lines().skip_while(|l| !l.starts_with("PROMPT")).filter(|_| true).collect();
    let mut bad = Vec::new();
    let mut lines = log.lines();
    while let Some(l) = lines.next() {
        if l == "PROMPT" {
            let p = lines.next().unwrap_or("");
            if !p.trim_end().ends_with(['.', '!', '?']) || p.contains("the Gentle") || p.to_lowercase().contains("red hair") {
                bad.push(p.to_string());
            }
            assert!(p.chars().count() <= 400, "{p}");
        }
    }
    let _ = prompts;
    live_note("F-TALES-13", "qwen3-sd-turbo-tile", &format!("{} pictures; story clean; prompts with a cut sentence or a human description of the fox: {bad:?}", a.pics.len()));
    assert!(bad.is_empty(), "{bad:?}");
}

// ------------------------------------------------------------------ Save PDF after a really written story

#[test]
#[ignore = "live service"]
fn live_tales_save_pdf_after_a_qwen3_story_with_real_sd_turbo_pictures() {
    use crate::apps::pdf_live_check::{check, found_back};
    let (mut a, cx, home) = live_story_text("live-pdf-en");
    a.opts.pic.model = Some(ModelChoice { provider: "lemon".into(), model: "SD-Turbo".into() });
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a small fox who finds the moon in a pond");
    key(&mut a, &cx, Key::Ctrl('n'));
    live_wait_long(&mut a, &cx, 400, "the story", |a| !a.is_writing());
    live_wait_long(&mut a, &cx, 600, "the pictures", |a| !a.pics.is_empty() && a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
    assert!(a.pics.iter().all(|p| p.state == PicState::Ready));
    click(&mut a, &cx, T_PDF);
    let note = a.note_text().unwrap_or_default().to_string();
    let id = a.saved_id().unwrap();
    let dir = home.join(format!("Stories/{id}"));
    let bytes = std::fs::read(dir.join("story.pdf")).unwrap_or_else(|e| panic!("story.pdf beside story.md: {e}; note {note:?}"));
    assert!(dir.join("story.md").is_file());
    let pdf = check(&bytes).unwrap_or_else(|e| panic!("structure: {e}"));
    let story = std::fs::read_to_string(dir.join("story.md")).unwrap();
    let (hit, all) = found_back(&story, &pdf.text);
    live_save("F-STORY-PDF", "story.pdf", &bytes);
    live_save("F-STORY-PDF", "story.md", story.as_bytes());
    live_note("F-STORY-PDF", "tales-save-pdf-english", &format!("note: {note}\nPDF {} bytes, {} pages, {} picture(s) for {} scenes, words of the story back {hit}/{all}", pdf.bytes, pdf.pages, pdf.images, a.pics.len()));
    assert!(note.starts_with("Saved Stories/") && note.contains("story.pdf"), "{note}");
    assert!(pdf.images >= a.pics.len(), "{} pictures in the PDF for {} scenes", pdf.images, a.pics.len());
    assert!(hit * 10 >= all * 8, "words back {hit}/{all}");
}

#[test]
#[ignore = "live service"]
fn live_tales_save_pdf_of_a_spanish_story_keeps_its_accents_and_a_hindi_story_is_refused_in_words() {
    use crate::apps::pdf_live_check::check;
    // TALES offers no Greek or Russian story (see the languages of the tile); Greek and Cyrillic PDFs are proven through pdf_create
    let mut lines = Vec::new();
    for lang in ["Spanish", "Hindi"] {
        let (mut a, cx, home) = live_story_text(&format!("live-pdf-{}", lang.to_lowercase()));
        a.opts.pictures = PictureMode::Off;
        a.opts.language = lang.into();
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a small fox who finds the moon in a pond");
        key(&mut a, &cx, Key::Ctrl('n'));
        live_wait_long(&mut a, &cx, 400, "the story", |a| !a.is_writing());
        click(&mut a, &cx, T_PDF);
        let note = a.note_text().unwrap_or_default().to_string();
        let Some(id) = a.saved_id() else {
            lines.push(format!("{lang}: the story was not saved: note {note:?}, error {:?}, {} blocks", a.error, a.story.blocks.len()));
            continue;
        };
        let dir = home.join(format!("Stories/{id}"));
        match std::fs::read(dir.join("story.pdf")) {
            Ok(bytes) => {
                let pdf = check(&bytes).unwrap_or_else(|e| panic!("structure: {e}"));
                let accents = pdf.text.chars().filter(|c| "\u{e1}\u{e9}\u{ed}\u{f3}\u{fa}\u{f1}\u{bf}\u{a1}".contains(*c)).count();
                live_save("F-STORY-PDF", &format!("tales-{}.pdf", lang.to_lowercase()), &bytes);
                lines.push(format!("{lang}: {note} | {} bytes, {} pages, accented letters read back {accents}", pdf.bytes, pdf.pages));
                assert!(lang == "Spanish" && accents > 0 && !pdf.text.contains('?') || lang != "Spanish", "{lines:?}");
            }
            Err(_) => {
                lines.push(format!("{lang}: no PDF; the tile said: {note}"));
                assert!(lang == "Hindi" && note.to_lowercase().contains("cannot be written"), "{lines:?}");
            }
        }
    }
    live_note("F-STORY-PDF", "tales-spanish-hindi", &lines.join("\n"));
}
