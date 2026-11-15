// The VOICE tile against a real Lemonade server on the LAN (included at the end of voice_tests.rs). Ignored tests: they need
// IYAKKI_LIVE_LEMONADE_URL and IYAKKI_LIVE_LEMONADE_KEY_FILE (see story_live_tests.rs) and run with
//   cargo test -p iyakki-shell --lib voice::tests::live_ -- --ignored --test-threads=1
// The speaker and the microphone are the fakes of this file; the speech, the transcript, the consent card and the key are real.
// For dictation the "microphone" plays back speech that kokoro-v1 made, so the transcript can be compared with the words said.

fn live_env() -> (String, String) {
    (std::env::var("IYAKKI_LIVE_LEMONADE_URL").expect("set IYAKKI_LIVE_LEMONADE_URL"), std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").expect("set IYAKKI_LIVE_LEMONADE_KEY_FILE"))
}

fn live_note(row: &str, name: &str, text: &str) {
    let (url, key_file) = live_env();
    let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
    let mut t = text.to_string();
    for h in [key, iyakki_agent::lemonade::origin(&url).unwrap_or_default(), url.trim_end_matches('/').to_string()] {
        if !h.is_empty() {
            t = t.replace(h.as_str(), "<redacted>");
        }
    }
    eprintln!("[{row}] {name}: {}", t.lines().next().unwrap_or(""));
    if let Ok(d) = std::env::var("IYAKKI_LIVE_EVIDENCE") {
        let d = std::path::Path::new(&d).join(row);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(format!("{name}.txt")), t);
    }
}

/// An admin whose only model server is the real one, named `lemon` (key from the file, or a wrong one after the list was read).
fn live_bench(tag: &str, wrong_key: bool, hold_ms: u64) -> Bench {
    let (url, key_file) = live_env();
    let fake = serve(); // unused: only here so the Bench type is the one of the other tests
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    let up: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(120)));
    cx.services = services_in(&d, up);
    cx.services.on_login("root", "rootpass12", true);
    let mut p = ProviderCfg::new("lemon", Kind::Lemonade);
    p.base_url = Some(url);
    p.key_ref = Some(format!("file:{key_file}"));
    cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    cx.services.refresh_lemonade("lemon").expect("the typed list of the real server");
    if wrong_key {
        let w = std::env::temp_dir().join(format!("iyakki-live-wrong-{tag}"));
        std::fs::write(&w, "not-the-key-0000").unwrap();
        let mut cfg = cx.services.settings.lock().unwrap().get().provider("lemon").cloned().unwrap();
        cfg.key_ref = Some(format!("file:{}", w.display()));
        cx.services.settings.lock().unwrap().upsert_provider(cfg).unwrap();
    }
    Bench { cx, fake, sink: Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(hold_ms) }), recs: recs(vec![usb()], "Generic PC") }
}

fn live_wait(a: &mut VoiceApp, cx: &AppCx, secs: u64, what: impl Fn(&VoiceApp) -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        tick(a, cx);
        if what(a) {
            return;
        }
    }
    panic!("gave up waiting: status {:?}, reader {:?}", status(a), a.reader.status());
}

fn live_words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

#[test]
#[ignore = "live service"]
fn live_voice_tile_asks_first_then_reads_pasted_text_with_kokoro() {
    let b = live_bench("live-voice-read", false, 0);
    let mut a = b.app();
    let i = a.voices.iter().position(|v| v.id.starts_with("lemonade:lemon:kokoro-v1:")).expect("the real server's Kokoro voice is listed");
    a.voice_i = i;
    let text = "Hello there. This is a test of reading aloud. The fox went home.";
    a.handle(&Event::Paste(text.into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    // a server on another machine: the card comes first and nothing is sent until the person says yes
    assert!(a.modal.is_some() && !a.is_reading(), "the consent card comes first");
    click(&mut a, &b.cx, T_NO);
    assert!(b.cx.services.voice_settings().remote_ok.is_empty());
    click(&mut a, &b.cx, T_PLAY);
    key(&mut a, &b.cx, Key::Char('y'));
    assert_eq!(b.cx.services.voice_settings().remote_ok.len(), 1);
    let t0 = Instant::now();
    live_wait(&mut a, &b.cx, 120, |a| !a.is_reading() && a.job.is_none());
    assert_eq!(status(&a), "Finished reading.");
    let played = b.sink.played.lock().unwrap();
    assert_eq!(played.len(), 3, "one sound for each sentence");
    let mut secs = Vec::new();
    for w in played.iter() {
        let p = iyakki_agent::voice::parse_wav(w).expect("a WAV the speaker can take");
        assert!(iyakki_agent::voice::peak(&p.samples) > 1500);
        secs.push(p.samples.len() as f32 / (p.rate as f32 * p.channels as f32));
    }
    let log = std::fs::read_to_string(b.cx.audit.path()).unwrap();
    assert!(log.contains("voice.read") && log.contains("voice.remote_ok") && !log.contains("Hello there"), "the audit names the voice, never the text: {log}");
    assert!(!log.contains(&std::fs::read_to_string(live_env().1).unwrap().trim().to_string()), "no key in the audit");
    live_note("F-VOICE-17", "voice-tile-read-aloud", &format!("consent card shown first, No sent nothing, Yes kept; 3 sentences read in {} ms; sounds {:?} s each; audit has voice.read and voice.remote_ok", t0.elapsed().as_millis(), secs));
}

#[test]
#[ignore = "live service"]
fn live_voice_tile_pause_resume_stop_with_real_speech() {
    let b = live_bench("live-voice-pause", false, 2500);
    let mut a = b.app();
    a.voice_i = a.voices.iter().position(|v| v.id.starts_with("lemonade:lemon:kokoro-v1:")).unwrap();
    b.cx.services.allow_speech_server(&iyakki_agent::lemonade::api_base(&live_env().0)).unwrap();
    a.handle(&Event::Paste("First sentence here. Second one follows. Third ends it.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    live_wait(&mut a, &b.cx, 60, |a| a.reader.status() == Status::Playing && a.text.selection().is_some());
    assert_eq!(a.text.selection(), Some((0, 20)), "the first sentence is lit");
    click(&mut a, &b.cx, T_PAUSE);
    assert_eq!(a.reader.status(), Status::Paused);
    click(&mut a, &b.cx, T_PAUSE);
    live_wait(&mut a, &b.cx, 30, |a| a.reader.status() == Status::Playing);
    click(&mut a, &b.cx, T_STOP);
    assert!(!a.is_reading() && a.text.selection().is_none() && a.job.is_none());
    live_note("F-VOICE-17", "voice-tile-cut-resume", "first sentence lit, pause froze it, resume went on, stop cleared the light and the job (real kokoro speech, fake speaker holding 2.5 s a sentence)");
}

#[test]
#[ignore = "live service"]
fn live_voice_tile_dictation_gets_a_transcript_from_whisper_tiny() {
    let said = "The quick brown fox jumps over the lazy dog.";
    let (url, key_file) = live_env();
    // the "microphone": what kokoro-v1 says, as 16 kHz mono (made through the same engine call the tile uses)
    let up = iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(120));
    let secrets = iyakki_agent::vault::Secrets::new(None, Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(5))));
    let engine = iyakki_agent::lemonade::tts_engine(&url, "kokoro-v1", None, Some(format!("file:{key_file}")));
    let wav = iyakki_agent::voice::synthesize(&engine, said, &secrets, &up).expect("kokoro made the speech");
    let pcm = iyakki_agent::voice::to_mono_16k(&iyakki_agent::voice::parse_wav(&wav).unwrap());
    let mut last = String::new();
    for attempt in 1..=3 {
        let mut b = live_bench(&format!("live-voice-dictate-{attempt}"), false, 0);
        b.recs = Arc::new(FakeRecs { mics: vec![usb()], product: "Generic PC".into(), cfg: CapCfg { pcm: pcm.clone(), ..cap_cfg() }, started: Mutex::new(vec![]) });
        let mut a = b.app();
        key(&mut a, &b.cx, Key::F(3));
        assert_eq!(a.area(), Area::Dictate);
        click(&mut a, &b.cx, T_REC);
        // the server is on another machine: the card comes first, nothing recorded or sent until yes
        assert!(a.modal.is_some() && b.recs.started.lock().unwrap().is_empty());
        click(&mut a, &b.cx, T_YES);
        assert!(a.is_recording());
        for _ in 0..5 {
            tick(&mut a, &b.cx);
        }
        let t0 = Instant::now();
        click(&mut a, &b.cx, T_REC_STOP);
        live_wait(&mut a, &b.cx, 60, |a| !a.transcribing);
        last = a.transcript.text.clone();
        let heard = live_words(&last);
        let want = live_words(said);
        let ok = want.iter().filter(|w| heard.contains(w)).count();
        if ok * 10 >= want.len() * 7 {
            assert!(status(&a).starts_with("Transcript ready"), "{}", status(&a));
            live_note("F-VOICE-18", "voice-tile-dictation", &format!("attempt {attempt}: consent card, record, stop; transcript in {} ms: {last:?} ({ok}/{} words of {said:?})", t0.elapsed().as_millis(), want.len()));
            return;
        }
        eprintln!("attempt {attempt}: heard {last:?}");
    }
    panic!("Whisper-Tiny did not hear the words in 3 attempts; last {last:?}");
}

#[test]
#[ignore = "live service"]
fn live_voice_tile_wrong_key_is_said_in_plain_words_and_the_key_is_not_shown() {
    let b = live_bench("live-voice-wrongkey", true, 0);
    let mut a = b.app();
    a.voice_i = a.voices.iter().position(|v| v.id.starts_with("lemonade:lemon:kokoro-v1:")).unwrap();
    b.cx.services.allow_speech_server(&iyakki_agent::lemonade::api_base(&live_env().0)).unwrap();
    a.handle(&Event::Paste("Nobody may hear this.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    live_wait(&mut a, &b.cx, 30, |a| a.status.as_ref().is_some_and(|s| !s.1));
    let s = status(&a);
    assert!(s.contains("API key") && !s.contains("not-the-key-0000"), "{s}");
    assert!(b.sink.played.lock().unwrap().is_empty());
    live_note("F-VOICE-17", "voice-tile-wrong-key", &s);
}

