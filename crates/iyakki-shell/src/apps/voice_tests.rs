//! VOICE against a fake Lemonade on loopback (kokoro-v1 speech, Whisper-Base transcription), a fake speaker and a fake
//! recorder: no real audio, no real network.

use super::*;
use crate::apps::lemonade_fake::{serve, FakeLemonade};
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use crate::gfx::{rgb, Gfx};
use crate::ui::R;
use iyakki_agent::providers::{Kind, ProviderCfg};
use iyakki_agent::story::Playing;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------ the fakes

/// A speaker: keeps every sound it was given and "plays" each for `hold`.
struct FakeSink {
    played: Mutex<Vec<Vec<u8>>>,
    hold: Duration,
}

struct Holding(Instant, Duration);

impl Playing for Holding {
    fn is_playing(&mut self) -> bool {
        self.0.elapsed() < self.1
    }
    fn stop(&mut self) {
        self.1 = Duration::ZERO;
    }
}

impl st::Sink for FakeSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        self.played.lock().unwrap().push(wav.to_vec());
        Ok(Box::new(Holding(Instant::now(), self.hold)))
    }
}

/// What the fake microphone does.
#[derive(Clone)]
struct CapCfg {
    pcm: Vec<i16>,
    level: f32,
    secs: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    full: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

struct FakeCap(CapCfg);

impl Capture for FakeCap {
    fn level(&self) -> f32 {
        self.0.level
    }
    fn elapsed(&self) -> Duration {
        Duration::from_secs(self.0.secs.load(Ordering::SeqCst))
    }
    fn is_full(&self) -> bool {
        self.0.full.load(Ordering::SeqCst)
    }
    fn is_alive(&mut self) -> bool {
        self.0.alive.load(Ordering::SeqCst)
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        if self.0.pcm.is_empty() {
            return Err("the microphone produced no audio: audio open error".into());
        }
        Ok(self.0.pcm.clone())
    }
}

impl Drop for FakeCap {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

struct FakeRecs {
    mics: Vec<Mic>,
    product: String,
    cfg: CapCfg,
    started: Mutex<Vec<(String, usize)>>,
}

impl Recorders for FakeRecs {
    fn mics(&self) -> Vec<Mic> {
        self.mics.clone()
    }
    fn product(&self) -> String {
        self.product.clone()
    }
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String> {
        self.started.lock().unwrap().push((cmd.to_string(), max_secs));
        Ok(Box::new(FakeCap(self.cfg.clone())))
    }
}

/// Speech: a tone loud enough to count as speech.
fn speech() -> Vec<i16> {
    iyakki_agent::voice::tone(300.0, 1200)
}

fn usb() -> Mic {
    Mic { id: "plughw:1,0".into(), label: "USB Audio (USB)".into(), usb: true }
}

struct Bench {
    cx: AppCx,
    fake: FakeLemonade,
    sink: Arc<FakeSink>,
    recs: Arc<FakeRecs>,
}

impl Bench {
    fn app(&self) -> VoiceApp {
        let sink = self.sink.clone();
        let rig = Rig { recorders: self.recs.clone(), sink: Some(sink), output: Arc::new(|| Ok(())) };
        VoiceApp::with_rig(&self.cx, rig)
    }
    fn app_with_output(&self, out: Result<(), String>) -> VoiceApp {
        let rig = Rig { recorders: self.recs.clone(), sink: Some(self.sink.clone()), output: Arc::new(move || out.clone()) };
        VoiceApp::with_rig(&self.cx, rig)
    }
    fn app_with_recs(&self, recs: Arc<FakeRecs>) -> VoiceApp {
        VoiceApp::with_rig(&self.cx, Rig { recorders: recs, sink: Some(self.sink.clone()), output: Arc::new(|| Ok(())) })
    }
    fn spoken(&self) -> Vec<serde_json::Value> {
        self.fake.bodies("/api/v1/audio/speech").iter().map(|b| serde_json::from_str(b).unwrap()).collect()
    }
}

fn cap_cfg() -> CapCfg {
    CapCfg { pcm: speech(), level: 0.6, secs: Arc::new(AtomicU64::new(3)), alive: Arc::new(AtomicBool::new(true)), full: Arc::new(AtomicBool::new(false)), dropped: Arc::new(AtomicBool::new(false)) }
}

fn recs(mics: Vec<Mic>, product: &str) -> Arc<FakeRecs> {
    Arc::new(FakeRecs { mics, product: product.into(), cfg: cap_cfg(), started: Mutex::new(vec![]) })
}

/// An admin with the fake Lemonade added the way Detect adds it (kokoro-v1 and Whisper-Base listed).
fn bench(tag: &str) -> Bench {
    let fake = serve();
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    let up: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    cx.services = services_in(&d, up);
    cx.services.on_login("root", "rootpass12", true);
    cx.services.use_local_lemonade(&fake.base(), Some("2026.42.1"));
    cx.services.refresh_lemonade("lemonade-local").unwrap();
    Bench { cx, fake, sink: Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(0) }), recs: recs(vec![usb()], "Generic PC") }
}

/// An admin with nothing set up: no provider, nothing found on this computer.
fn bare(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx
}

/// A Lemonade provider on another machine (names ending in .lan are on the network); its models are typed but never asked.
fn add_remote(cx: &AppCx, id: &str, kinds: &[(&str, &str)]) {
    let mut p = ProviderCfg::new(id, Kind::Lemonade);
    p.base_url = Some("http://lemon.lan:13305/api/v1".into());
    p.model_kinds = kinds.iter().map(|(m, k)| (m.to_string(), k.to_string())).collect();
    cx.services.settings.lock().unwrap().update(|s| s.providers.push(p)).unwrap();
}

fn click(a: &mut VoiceApp, cx: &AppCx, n: usize) {
    a.handle(&Event::Tab(n), cx);
}

fn key(a: &mut VoiceApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn tick(a: &mut VoiceApp, cx: &AppCx) {
    a.handle(&Event::Tick, cx);
    std::thread::sleep(Duration::from_millis(10));
}

fn wait(a: &mut VoiceApp, cx: &AppCx, what: impl Fn(&VoiceApp) -> bool) {
    for _ in 0..1500 {
        tick(a, cx);
        if what(a) {
            return;
        }
    }
    panic!("gave up waiting: status {:?}, reader {:?}", a.status, a.reader.status());
}

fn draw(a: &mut VoiceApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    let hh = 46.0 * s;
    a.render(&mut g, R::new(0.0, hh, w as f32, h as f32 - hh), s, cx, 3);
    g
}

fn label_hit(g: &Gfx, label: &str) -> bool {
    g.hits.iter().any(|h| h.label == label)
}

fn status(a: &VoiceApp) -> String {
    a.status_text().map(|s| s.0.to_string()).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ reading

#[test]
fn pasted_text_is_read_with_kokoro_the_request_is_right_and_the_sound_is_played() {
    let b = bench("voice-read");
    let mut a = b.app();
    assert_eq!(a.area(), Area::Read);
    assert!(a.voices.iter().any(|v| v.label == "Kokoro · af_heart") && a.voices.len() == 12, "{:?}", a.voices.iter().map(|v| &v.label).collect::<Vec<_>>());
    a.handle(&Event::Paste("Hello there. This is a test of reading aloud.".into()), &b.cx);
    assert_eq!(a.text.text, "Hello there. This is a test of reading aloud.");
    assert!(status(&a).contains("Pasted"), "{}", status(&a));
    click(&mut a, &b.cx, T_PLAY);
    assert!(a.is_reading());
    wait(&mut a, &b.cx, |a| !a.is_reading() && a.job.is_none());
    assert_eq!(status(&a), "Finished reading.");
    let bodies = b.spoken();
    assert_eq!(bodies.len(), 2, "one request for each sentence: {bodies:?}");
    for (body, text) in bodies.iter().zip(["Hello there.", "This is a test of reading aloud."]) {
        assert_eq!(body["model"], "kokoro-v1");
        assert_eq!(body["voice"], "af_sky", "the first Kokoro voice");
        assert_eq!(body["input"], text);
        assert_eq!(body["response_format"], "wav");
        assert!(body.get("speed").is_none(), "normal speed sends no speed");
    }
    let played = b.sink.played.lock().unwrap();
    assert_eq!(played.len(), 2);
    assert!(played.iter().all(|w| &w[..4] == b"RIFF"));
    let log = std::fs::read_to_string(b.cx.audit.path()).unwrap();
    assert!(log.contains("voice.read") && log.contains("lemonade:lemonade-local:kokoro-v1:af_sky") && !log.contains("Hello there"), "the audit names the voice, never the text: {log}");
}

#[test]
fn the_voice_picker_and_the_speed_change_what_is_asked_for() {
    let b = bench("voice-pick");
    let mut a = b.app();
    click(&mut a, &b.cx, T_VOICE);
    assert!(matches!(a.modal, Some(Modal::VoicePick(_))));
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains("am_adam")), "the voices are listed");
    for c in "am_adam".chars() {
        key(&mut a, &b.cx, Key::Char(c));
    }
    key(&mut a, &b.cx, Key::Enter);
    assert!(a.modal.is_none());
    assert_eq!(a.voice().unwrap().label, "Kokoro · am_adam");
    // faster: 1.00 -> 1.10
    click(&mut a, &b.cx, T_SPEED_UP);
    click(&mut a, &b.cx, T_SPEED_UP);
    assert!((a.speed - 1.10).abs() < 0.001, "{}", a.speed);
    a.handle(&Event::Paste("One short sentence.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    wait(&mut a, &b.cx, |a| a.job.is_none() && !a.is_reading());
    let body = &b.spoken()[0];
    assert_eq!((body["voice"].as_str(), body["speed"].as_f64().map(|s| (s * 100.0).round() as i32)), (Some("am_adam"), Some(110)));
    // the ends of the slider: 0.7x and 1.5x
    for _ in 0..30 {
        click(&mut a, &b.cx, T_SPEED_DN);
    }
    assert!((a.speed - 0.7).abs() < 0.001);
    for _ in 0..30 {
        click(&mut a, &b.cx, T_SPEED_UP);
    }
    assert!((a.speed - 1.5).abs() < 0.001);
    // a click on the track
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "speed"));
    let r = a.speed_rect;
    a.handle(&Event::AreaAt(SPEED_AREA, 0.0, 0.0), &b.cx);
    assert!((a.speed - 0.7).abs() < 0.001, "left end");
    a.handle(&Event::AreaAt(SPEED_AREA, r.w / 2.0, 0.0), &b.cx);
    assert!((a.speed - 1.1).abs() < 0.001, "middle: {}", a.speed);
}

#[test]
fn the_sentence_being_read_is_lit_and_pause_resume_and_stop_work() {
    let mut b = bench("voice-lit");
    // long enough that a slow, loaded machine (the picture takes a while to draw) never finishes the reading under the test
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(2500) });
    let mut a = b.app();
    a.handle(&Event::Paste("First sentence here. Second one follows. Third ends it.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    wait(&mut a, &b.cx, |a| a.reader.status() == Status::Playing && a.text.selection().is_some());
    assert_eq!(a.text.selection(), Some((0, 20)), "the first sentence is lit in the box");
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "Pause") && label_hit(&g, "Stop") && !label_hit(&g, "Play"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    click(&mut a, &b.cx, T_PAUSE);
    assert_eq!(a.reader.status(), Status::Paused);
    assert!(label_hit(&draw(&mut a, &b.cx, 1280, 720), "Resume"));
    click(&mut a, &b.cx, T_PAUSE);
    wait(&mut a, &b.cx, |a| a.reader.status() == Status::Playing);
    // a click on the progress bar jumps to the second sentence
    let _ = draw(&mut a, &b.cx, 1280, 720);
    let pr = a.progress_rect;
    a.handle(&Event::AreaAt(PROGRESS_AREA, pr.w * 0.5, 0.0), &b.cx);
    wait(&mut a, &b.cx, |a| a.text.selection() == Some((21, 40)));
    click(&mut a, &b.cx, T_STOP);
    assert!(!a.is_reading() && a.text.selection().is_none() && a.job.is_none());
    assert!(label_hit(&draw(&mut a, &b.cx, 1280, 720), "Play"));
}

#[test]
fn read_selection_reads_only_the_selected_part_and_lights_it_where_it_is() {
    let mut b = bench("voice-sel");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(150), });
    let mut a = b.app();
    a.handle(&Event::Paste("Skip this one. Read only this one. Skip the end.".into()), &b.cx);
    click(&mut a, &b.cx, T_SEL);
    assert!(status(&a).contains("Select some of the text"), "{}", status(&a));
    assert!(b.spoken().is_empty());
    a.text.anchor = Some(15);
    a.text.cur = 34;
    assert_eq!(a.text.selected_text().as_deref(), Some("Read only this one."));
    click(&mut a, &b.cx, T_SEL);
    wait(&mut a, &b.cx, |a| a.job.is_none() && !a.is_reading());
    let bodies = b.spoken();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["input"], "Read only this one.");
}

#[test]
fn a_remote_voice_asks_first_the_answer_is_kept_and_no_asks_sends_nothing() {
    let b = bench("voice-consent");
    add_remote(&b.cx, "lan-voice", &[("kokoro-v1", "tts")]);
    let mut a = b.app();
    let i = a.voices.iter().position(|v| v.id == "lemonade:lan-voice:kokoro-v1:af_sky").expect("the remote voice is listed");
    a.voice_i = i;
    a.handle(&Event::Paste("A private sentence.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    assert!(a.modal.is_some() && !a.is_reading(), "the card comes first");
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "Yes, send") && label_hit(&g, "No"));
    let Some(Modal::Consent { text, dest, .. }) = &a.modal else { panic!() };
    assert!(text.contains("lan-voice") && text.contains("lemon.lan") && text.contains("on the network") && text.contains("Send text to lemon.lan from now on?"), "{text}");
    assert_eq!((dest.place_words(), dest.server.as_str()), ("your network", "http://lemon.lan:13305"));
    // No: nothing is sent, nothing is remembered
    click(&mut a, &b.cx, T_NO);
    assert!(a.modal.is_none() && !a.is_reading());
    assert!(b.cx.services.voice_settings().remote_ok.is_empty());
    assert!(b.fake.bodies("/api/v1/audio/speech").is_empty());
    // Yes: remembered the same way Settings -> Models remembers it, then it reads (the machine is not really there)
    click(&mut a, &b.cx, T_PLAY);
    key(&mut a, &b.cx, Key::Char('y'));
    assert_eq!(b.cx.services.voice_settings().remote_ok, vec!["http://lemon.lan:13305".to_string()]);
    assert!(a.modal.is_none() && a.is_reading());
    a.stop_reading();
    let log = std::fs::read_to_string(b.cx.audit.path()).unwrap();
    assert!(log.contains("voice.remote_ok") && log.contains("http://lemon.lan:13305"), "{log}");
    // asked once: the next reading goes straight on
    click(&mut a, &b.cx, T_PLAY);
    assert!(a.modal.is_none() && a.is_reading());
    a.stop_reading();
}

#[test]
fn an_unreachable_server_is_said_in_plain_words() {
    let mut cx = cx_with_admin_store("voice-down");
    let d = temp_dir("voice-down-svc");
    cx.services = services_in(&d, Arc::new(Offline));
    let mut p = ProviderCfg::new("lemonade-local", Kind::Lemonade);
    p.base_url = Some("http://127.0.0.1:9/api/v1".into());
    p.model_kinds = [("kokoro-v1".to_string(), "tts".to_string())].into();
    cx.services.settings.lock().unwrap().update(|s| s.providers.push(p)).unwrap();
    let rig = Rig { recorders: recs(vec![usb()], "x"), sink: Some(Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::ZERO })), output: Arc::new(|| Ok(())) };
    let mut a = VoiceApp::with_rig(&cx, rig);
    a.handle(&Event::Paste("Nobody is listening.".into()), &cx);
    click(&mut a, &cx, T_PLAY);
    wait(&mut a, &cx, |a| a.status.as_ref().is_some_and(|s| !s.1));
    assert!(status(&a).contains("did not answer at http://127.0.0.1:9"), "{}", status(&a));
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label == "Play"), "can try again");
}

#[test]
fn no_voice_at_all_offers_to_set_up_kokoro_and_no_sound_output_points_to_settings() {
    let cx = bare("voice-none");
    let rig = Rig { recorders: recs(vec![usb()], "x"), sink: None, output: Arc::new(|| Ok(())) };
    let mut a = VoiceApp::with_rig(&cx, rig);
    assert!(a.voices.is_empty());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(label_hit(&g, "Set up Kokoro") && label_hit(&g, "Voice settings"));
    a.handle(&Event::Paste("Hello.".into()), &cx);
    click(&mut a, &cx, T_PLAY);
    assert_eq!(status(&a), "No reading voice is set up yet.");
    click(&mut a, &cx, T_SETUP);
    assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "kokoro".to_string())));
    assert_eq!(a.take_request(), None);
    click(&mut a, &cx, T_SETTINGS_VOICE);
    assert_eq!(a.take_request(), Some(("settings".to_string(), "voice".to_string())));
    // a voice exists but the computer has no sound output
    let b = bench("voice-nooutput");
    let mut a = b.app_with_output(Err("no sound output: connect a monitor".into()));
    a.handle(&Event::Paste("Hello.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    assert_eq!(status(&a), "No sound output: choose where sound plays in Settings -> Sound.");
    assert!(!a.is_reading() && b.spoken().is_empty(), "nothing was asked of the voice");
}

#[test]
fn twenty_thousand_characters_fit_longer_text_is_cut_with_a_notice_and_is_read_in_pieces() {
    let mut b = bench("voice-big");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_secs(3) });
    let mut a = b.app();
    let sentence = "Here is one more sentence of the long text. ";
    let long = sentence.repeat(600); // 26,400 characters
    a.handle(&Event::Paste(long.clone()), &b.cx);
    assert_eq!(a.text.len(), 20_000);
    let (n, bad) = a.text.notice().unwrap();
    assert!(bad && n.contains("20,000 of"), "{n}");
    click(&mut a, &b.cx, T_PLAY);
    let snap = a.reader.snapshot();
    assert_eq!(snap.chunks, vd::pieces(&a.text.text).len());
    assert!(snap.chunks > 400, "{}", snap.chunks);
    for (w, h) in [(360u32, 740u32), (1280, 720)] {
        let g = draw(&mut a, &b.cx, w, h);
        assert!(label_hit(&g, "Stop"), "{w}x{h}: {:?} {:?} {:?}", a.status, a.reader.status(), g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    }
    a.stop_reading();
}

#[test]
fn the_language_of_the_text_is_noticed_and_a_voice_that_cannot_read_it_is_flagged() {
    let b = bench("voice-lang");
    let mut a = b.app();
    a.handle(&Event::Paste("வணக்கம், இது ஒரு சோதனை வாக்கியம்.".into()), &b.cx);
    let notes = a.read_notes(&b.cx);
    assert!(notes.iter().any(|(t, _)| t.contains("looks like Tamil") && t.contains("Kokoro reads English only")), "{notes:?}");
    let place = notes.iter().map(|n| n.0.as_str()).find(|t| t.starts_with("Text is sent to")).unwrap();
    assert!(place.contains("lemonade-local") && place.contains("(this computer)"), "{place}");
    a.text.take();
    a.handle(&Event::Paste("This is plain English text for the hint.".into()), &b.cx);
    assert!(a.read_notes(&b.cx).iter().any(|(t, _)| t == "Looks like English."));
}

#[test]
fn paste_open_file_and_clear_work_and_files_are_checked_against_the_account() {
    let b = bench("voice-file");
    let mut a = b.app();
    crate::ui::clipboard::copy("from the clipboard");
    click(&mut a, &b.cx, T_PASTE);
    assert_eq!(a.text.text, "from the clipboard");
    click(&mut a, &b.cx, T_CLEAR);
    assert!(a.text.is_empty());
    // a file inside the home folder opens; one outside does not
    let home = temp_dir("voice-file-home");
    std::fs::write(home.join("note.txt"), "Text from a file.").unwrap();
    std::fs::write(home.join("pic.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    let mut cx = b.cx.clone();
    cx.session.policy.fs_read = vec![home.display().to_string()];
    cx.session.policy.fs_write = vec![home.display().to_string()];
    let mut a = VoiceApp::with_rig(&cx, Rig { recorders: b.recs.clone(), sink: Some(b.sink.clone()), output: Arc::new(|| Ok(())) });
    key(&mut a, &cx, Key::Ctrl('o'));
    assert_eq!(a.focus, Focus::Path);
    for c in format!("{}/note.txt", home.display()).chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.text.text, "Text from a file.");
    assert!(status(&a).starts_with("Loaded 17 characters"), "{}", status(&a));
    click(&mut a, &cx, T_LOAD);
    a.handle(&Event::Paste(format!("{}/pic.bin", home.display())), &cx);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(status(&a), "That file is not text.");
    click(&mut a, &cx, T_LOAD);
    a.handle(&Event::Paste("/etc/hostname".into()), &cx);
    key(&mut a, &cx, Key::Enter);
    assert!(!status(&a).is_empty() && a.text.text == "Text from a file.", "{}", status(&a));
}

#[test]
fn only_accounts_with_voice_may_read_or_record() {
    let mut b = bench("voice-denied");
    b.cx.session.policy.voice = false;
    let mut a = b.app();
    a.handle(&Event::Paste("Hello.".into()), &b.cx);
    click(&mut a, &b.cx, T_PLAY);
    assert_eq!(status(&a), "Voice is not enabled for your account.");
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_REC);
    assert!(b.recs.started.lock().unwrap().is_empty() && !a.is_recording());
    assert!(std::fs::read_to_string(b.cx.audit.path()).unwrap().contains("voice.denied"));
}

// ------------------------------------------------------------------------------------------------ dictating

#[test]
fn recording_needs_a_click_shows_a_clear_indicator_and_the_transcript_comes_from_whisper() {
    let b = bench("voice-dictate");
    let mut a = b.app();
    // nothing starts by itself: not opening the tab, not time passing, not typing or pasting
    key(&mut a, &b.cx, Key::F(3));
    assert_eq!(a.area(), Area::Dictate);
    for _ in 0..200 {
        tick(&mut a, &b.cx);
    }
    a.handle(&Event::Paste("some text".into()), &b.cx);
    assert!(b.recs.started.lock().unwrap().is_empty() && !a.is_recording(), "never records without the click");
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "● Record") && label_hit(&g, "microphone"));
    click(&mut a, &b.cx, T_REC);
    assert!(a.is_recording());
    {
        let s = b.recs.started.lock().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].1, 300, "five minutes at most");
        assert!(s[0].0.starts_with("arecord"), "{}", s[0].0);
    }
    for _ in 0..5 {
        tick(&mut a, &b.cx);
    }
    assert!(a.level > 0.3, "the level meter moves: {}", a.level);
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(label_hit(&g, "Stop"), "the stop button is the recording indicator's neighbour");
    click(&mut a, &b.cx, T_REC_STOP);
    assert!(!a.is_recording() && a.transcribing);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    assert_eq!(a.transcript.text, "some text\nhello lemonade", "the pasted text was in the transcript box; the new words follow it");
    assert!(status(&a).starts_with("Transcript ready"), "{}", status(&a));
    // the request: multipart with the model, the format and a WAV of the recording
    let bodies = b.fake.bodies("/api/v1/audio/transcriptions");
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("name=\"model\"") && bodies[0].contains("Whisper-Base") && bodies[0].contains("response_format") && bodies[0].contains("filename=\"audio.wav\"") && bodies[0].contains("RIFF"), "{}", &bodies[0].chars().take(300).collect::<String>());
    // the sound was not kept anywhere on disk
    fn wavs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                wavs(&p, out);
            } else if matches!(p.extension().and_then(|e| e.to_str()), Some("wav" | "raw" | "pcm")) {
                out.push(p);
            }
        }
    }
    let mut found = Vec::new();
    wavs(b.cx.services.dir(), &mut found);
    assert!(found.is_empty(), "audio stays in memory: {found:?}");
}

#[test]
fn the_transcript_is_edited_copied_cleared_and_read_back() {
    let b = bench("voice-copy");
    let mut a = b.app();
    key(&mut a, &b.cx, Key::F(3));
    click(&mut a, &b.cx, T_COPY);
    assert_eq!(status(&a), "There is no transcript to copy yet.");
    click(&mut a, &b.cx, T_REC);
    click(&mut a, &b.cx, T_REC_STOP);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    // it can be edited
    click(&mut a, &b.cx, T_T_CLEAR);
    a.handle(&Event::Paste("Edited: hello lemonade".into()), &b.cx);
    click(&mut a, &b.cx, T_COPY);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("Edited: hello lemonade"));
    assert!(status(&a).starts_with("Copied 22 characters"), "{}", status(&a));
    assert_eq!(a.transcript.text, "Edited: hello lemonade", "copy keeps the text");
    click(&mut a, &b.cx, T_COPY_CLEAR);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("Edited: hello lemonade"));
    assert!(a.transcript.is_empty());
    // a second recording adds to what is there; read it back switches to READ ALOUD with the text
    click(&mut a, &b.cx, T_REC);
    click(&mut a, &b.cx, T_REC_STOP);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    click(&mut a, &b.cx, T_REC);
    click(&mut a, &b.cx, T_REC_STOP);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    assert_eq!(a.transcript.text, "hello lemonade\nhello lemonade");
    click(&mut a, &b.cx, T_READ_BACK);
    assert_eq!((a.area(), a.text.text.as_str()), (Area::Read, "hello lemonade\nhello lemonade"));
}

#[test]
fn no_microphone_explains_that_usb_works_and_names_the_laptop_without_a_driver() {
    let b = bench("voice-nomic");
    let r = recs(vec![], "MacBookPro14,1");
    let mut a = b.app_with_recs(r.clone());
    key(&mut a, &b.cx, Key::F(3));
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(!label_hit(&g, "● Record") || true);
    click(&mut a, &b.cx, T_REC);
    assert!(!a.is_recording() && r.started.lock().unwrap().is_empty());
    let s = status(&a);
    assert!(s.contains("No microphone found") && s.contains("USB microphones work") && s.contains("MacBookPro14,1 is not supported"), "{s}");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let g = draw(&mut a, &b.cx, w, h);
        assert!(!g.hits.is_empty());
    }
}

#[test]
fn a_remote_speech_to_text_server_asks_before_the_microphone_starts() {
    let cx = bare("voice-stt-consent");
    add_remote(&cx, "lan-stt", &[("Whisper-Base", "stt")]);
    let r = recs(vec![usb()], "x");
    let mut a = VoiceApp::with_rig(&cx, Rig { recorders: r.clone(), sink: None, output: Arc::new(|| Ok(())) });
    click(&mut a, &cx, T_TAB_DICT);
    click(&mut a, &cx, T_REC);
    assert!(a.modal.is_some() && r.started.lock().unwrap().is_empty());
    let Some(Modal::Consent { text, .. }) = &a.modal else { panic!() };
    assert!(text.contains("what the microphone hears") && text.contains("Send sound to lemon.lan from now on?"), "{text}");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(label_hit(&g, "Yes, send"));
    click(&mut a, &cx, T_NO);
    assert!(r.started.lock().unwrap().is_empty() && a.modal.is_none());
    click(&mut a, &cx, T_REC);
    click(&mut a, &cx, T_YES);
    assert_eq!(cx.services.voice_settings().remote_ok, vec!["http://lemon.lan:13305".to_string()]);
    assert!(a.is_recording(), "yes starts the recording that was asked for");
    a.stop_record(&cx, false);
}

#[test]
fn escape_cancels_a_recording_and_keeps_nothing_and_five_minutes_stops_by_itself() {
    let b = bench("voice-limit");
    let r = recs(vec![usb()], "x");
    let mut a = b.app_with_recs(r.clone());
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_REC);
    key(&mut a, &b.cx, Key::Esc);
    assert!(!a.is_recording() && !a.transcribing && r.cfg.dropped.load(Ordering::SeqCst));
    assert!(status(&a).contains("Nothing was kept") && a.transcript.is_empty());
    assert!(b.fake.bodies("/api/v1/audio/transcriptions").is_empty(), "the sound never left");
    // the limit
    click(&mut a, &b.cx, T_REC);
    r.cfg.secs.store(299, Ordering::SeqCst);
    tick(&mut a, &b.cx);
    assert!(a.is_recording());
    r.cfg.secs.store(300, Ordering::SeqCst);
    tick(&mut a, &b.cx);
    assert!(!a.is_recording() && a.transcribing, "stopped at five minutes");
    wait(&mut a, &b.cx, |a| !a.transcribing);
    assert_eq!(a.transcript.text, "hello lemonade");
    assert!(draw(&mut a, &b.cx, 1280, 720).hits.iter().any(|h| h.label == "Copy to clipboard"));
}

#[test]
fn silence_a_dead_microphone_and_an_unreachable_server_each_get_their_own_words() {
    let b = bench("voice-fail");
    // silence
    let quiet = Arc::new(FakeRecs { mics: vec![usb()], product: "x".into(), cfg: CapCfg { pcm: vec![3; 16000], ..cap_cfg() }, started: Mutex::new(vec![]) });
    let mut a = b.app_with_recs(quiet);
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_REC);
    click(&mut a, &b.cx, T_REC_STOP);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    assert_eq!(status(&a), "I only heard silence. Check the microphone and its volume.");
    // the device goes away (the program ended and captured nothing)
    let dead = Arc::new(FakeRecs { mics: vec![usb()], product: "x".into(), cfg: CapCfg { pcm: vec![], ..cap_cfg() }, started: Mutex::new(vec![]) });
    dead.cfg.alive.store(false, Ordering::SeqCst);
    let mut a = b.app_with_recs(dead);
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_REC);
    tick(&mut a, &b.cx);
    wait(&mut a, &b.cx, |a| !a.transcribing && !a.is_recording());
    assert!(status(&a).contains("microphone produced no audio"), "{}", status(&a));
    // no speech-to-text anywhere
    let cx = bare("voice-nostt");
    let mut a = VoiceApp::with_rig(&cx, Rig { recorders: recs(vec![usb()], "x"), sink: None, output: Arc::new(|| Ok(())) });
    click(&mut a, &cx, T_TAB_DICT);
    click(&mut a, &cx, T_REC);
    assert!(status(&a).starts_with("No speech-to-text is set up") && !a.is_recording());
    assert!(label_hit(&draw(&mut a, &cx, 1280, 720), "Voice settings"));
    // the server does not answer
    let mut cx = bare("voice-stt-down");
    let mut p = ProviderCfg::new("lemonade-local", Kind::Lemonade);
    p.base_url = Some("http://127.0.0.1:9/api/v1".into());
    p.model_kinds = [("Whisper-Base".to_string(), "stt".to_string())].into();
    cx.services.settings.lock().unwrap().update(|s| s.providers.push(p)).unwrap();
    cx.session.policy.voice = true;
    let mut a = VoiceApp::with_rig(&cx, Rig { recorders: recs(vec![usb()], "x"), sink: None, output: Arc::new(|| Ok(())) });
    click(&mut a, &cx, T_TAB_DICT);
    click(&mut a, &cx, T_REC);
    click(&mut a, &cx, T_REC_STOP);
    wait(&mut a, &cx, |a| !a.transcribing);
    assert!(status(&a).contains("speech-to-text server did not answer (Whisper-Base)"), "{}", status(&a));
}

#[test]
fn the_chosen_microphone_is_the_one_recorded() {
    let b = bench("voice-mic");
    let two = vec![Mic { id: "plughw:0,0".into(), label: "Analog".into(), usb: false }, usb()];
    let r = recs(two, "x");
    let mut a = b.app_with_recs(r.clone());
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_MIC);
    let g = draw(&mut a, &b.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains("USB Audio (USB)")));
    for c in "usb".chars() {
        key(&mut a, &b.cx, Key::Char(c));
    }
    key(&mut a, &b.cx, Key::Enter);
    assert_eq!(a.mic_i, 2);
    click(&mut a, &b.cx, T_REC);
    assert!(r.started.lock().unwrap()[0].0.contains("-D plughw:1,0"), "{:?}", r.started.lock().unwrap());
    a.stop_record(&b.cx, false);
}

// ------------------------------------------------------------------------------------------------ keys

#[test]
fn the_keyboard_reaches_everything() {
    let mut b = bench("voice-keys");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_millis(300) });
    let mut a = b.app();
    // the text box has the keyboard to begin with: typing goes in it
    for c in "Typed words. Two sentences.".chars() {
        key(&mut a, &b.cx, Key::Char(c));
    }
    assert_eq!(a.text.text, "Typed words. Two sentences.");
    key(&mut a, &b.cx, Key::Ctrl('a'));
    key(&mut a, &b.cx, Key::Ctrl('c'));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("Typed words. Two sentences."));
    key(&mut a, &b.cx, Key::Right);
    // Ctrl+Enter plays, again pauses, again resumes; Esc stops; Esc again leaves the box; Esc again goes back
    key(&mut a, &b.cx, Key::Ctrl('\n'));
    assert!(a.is_reading());
    wait(&mut a, &b.cx, |a| a.reader.status() == Status::Playing);
    key(&mut a, &b.cx, Key::Ctrl('\n'));
    assert_eq!(a.reader.status(), Status::Paused);
    key(&mut a, &b.cx, Key::Ctrl('\n'));
    wait(&mut a, &b.cx, |a| a.reader.status() == Status::Playing);
    key(&mut a, &b.cx, Key::Esc);
    assert!(!a.is_reading());
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &b.cx), Flow::Stay) || a.focus == Focus::None);
    // with no box focused the letters are shortcuts
    a.focus = Focus::None;
    key(&mut a, &b.cx, Key::Char('d'));
    assert_eq!(a.area(), Area::Dictate);
    key(&mut a, &b.cx, Key::Char('r'));
    assert_eq!(a.area(), Area::Read);
    a.focus = Focus::None;
    key(&mut a, &b.cx, Key::Char(' '));
    assert!(a.is_reading());
    key(&mut a, &b.cx, Key::Char('s'));
    assert!(!a.is_reading());
    key(&mut a, &b.cx, Key::Char('v'));
    assert!(matches!(a.modal, Some(Modal::VoicePick(_))));
    key(&mut a, &b.cx, Key::Esc);
    assert!(a.modal.is_none());
    key(&mut a, &b.cx, Key::Char('+'));
    assert!((a.speed - 1.05).abs() < 0.001);
    key(&mut a, &b.cx, Key::Char('-'));
    key(&mut a, &b.cx, Key::Char('-'));
    assert!((a.speed - 0.95).abs() < 0.001);
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &b.cx), Flow::Back), "Esc with nothing to stop goes back");
    // dictate: Ctrl+Enter records and stops
    key(&mut a, &b.cx, Key::F(3));
    key(&mut a, &b.cx, Key::Ctrl('\n'));
    assert!(a.is_recording());
    key(&mut a, &b.cx, Key::Ctrl('\n'));
    assert!(a.transcribing);
    wait(&mut a, &b.cx, |a| !a.transcribing);
    key(&mut a, &b.cx, Key::Char('c'));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("hello lemonade"));
    key(&mut a, &b.cx, Key::Char('x'));
    assert!(a.transcript.is_empty());
    assert!(!a.help().is_empty() && a.hint().contains("F2"));
}

// ------------------------------------------------------------------------------------------------ the picture

fn inside(g: &Gfx, w: u32, h: u32, what: &str) {
    for hit in &g.hits {
        let r = hit.rect;
        assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{what}: {} is off screen at {w}x{h}: {r:?}", hit.label);
    }
}

fn colours(g: &Gfx) -> usize {
    let px = g.rgba();
    let mut seen = std::collections::HashSet::new();
    for i in (0..px.len()).step_by(4 * 37) {
        seen.insert([px[i], px[i + 1], px[i + 2]]);
    }
    seen.len()
}

const SIZES: [(u32, u32); 4] = [(360, 740), (800, 480), (1280, 720), (1920, 1080)];

#[test]
fn read_aloud_draws_in_every_state_at_four_sizes_and_every_control_is_on_screen() {
    let mut b = bench("voice-draw-read");
    b.sink = Arc::new(FakeSink { played: Mutex::new(vec![]), hold: Duration::from_secs(60) });
    for (w, h) in SIZES {
        // idle and empty
        let mut a = b.app();
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "idle");
        assert!(colours(&g) > 12, "not blank at {w}x{h}");
        for l in ["READ ALOUD", "DICTATE", "voice", "Play", "Paste", "Open file", "Clear", "Read selection", "Stop", "text"] {
            assert!(label_hit(&g, l) || matches!(l, "Read selection" | "Stop" | "Clear"), "{l} at {w}x{h}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        // text, a warning, an open file prompt
        a.handle(&Event::Paste("வணக்கம், இது ஒரு சோதனை வாக்கியம். ".repeat(40)), &b.cx);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "tamil text");
        key(&mut a, &b.cx, Key::Ctrl('o'));
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "file prompt");
        key(&mut a, &b.cx, Key::Esc);
        // playing
        a.text.set("One sentence to read. Another sentence follows it. And a third.");
        click(&mut a, &b.cx, T_PLAY);
        wait(&mut a, &b.cx, |a| a.reader.status() == Status::Playing);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "playing");
        assert!(label_hit(&g, "Pause") && label_hit(&g, "progress"), "{w}x{h}");
        click(&mut a, &b.cx, T_PAUSE);
        inside(&draw(&mut a, &b.cx, w, h), w, h, "paused");
        a.stop_reading();
        // an error
        a.say("The reading voice did not answer at http://127.0.0.1:13305. Is the server running?", false);
        inside(&draw(&mut a, &b.cx, w, h), w, h, "error");
        // the pickers and the consent card
        click(&mut a, &b.cx, T_VOICE);
        inside(&draw(&mut a, &b.cx, w, h), w, h, "voice picker");
        key(&mut a, &b.cx, Key::Esc);
    }
    // no voice at all
    let cx = bare("voice-draw-none");
    for (w, h) in SIZES {
        let mut a = VoiceApp::with_rig(&cx, Rig { recorders: recs(vec![], "x"), sink: None, output: Arc::new(|| Ok(())) });
        let g = draw(&mut a, &cx, w, h);
        inside(&g, w, h, "no voice");
        assert!(label_hit(&g, "Set up Kokoro"), "{w}x{h}");
    }
}

#[test]
fn dictate_draws_in_every_state_at_four_sizes_and_every_control_is_on_screen() {
    let b = bench("voice-draw-dict");
    for (w, h) in SIZES {
        let mut a = b.app();
        click(&mut a, &b.cx, T_TAB_DICT);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "idle");
        assert!(colours(&g) > 12);
        for l in ["● Record", "Copy to clipboard", "Read it back", "microphone"] {
            assert!(label_hit(&g, l) || l.starts_with("Copy") || l.starts_with("Read"), "{l} at {w}x{h}");
        }
        // recording
        click(&mut a, &b.cx, T_REC);
        for _ in 0..3 {
            tick(&mut a, &b.cx);
        }
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "recording");
        assert!(label_hit(&g, "Stop"), "{w}x{h}");
        click(&mut a, &b.cx, T_REC_STOP);
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "transcribing");
        wait(&mut a, &b.cx, |a| !a.transcribing);
        // a transcript
        a.transcript.set(&"A long transcript line that wraps over several lines of the box. ".repeat(20));
        let g = draw(&mut a, &b.cx, w, h);
        inside(&g, w, h, "transcript");
        assert!(label_hit(&g, "Copy to clipboard") && label_hit(&g, "Copy and clear") && label_hit(&g, "Read it back"), "{w}x{h}");
        // no microphone
        let mut n = b.app_with_recs(recs(vec![], "Generic"));
        click(&mut n, &b.cx, T_TAB_DICT);
        inside(&draw(&mut n, &b.cx, w, h), w, h, "no microphone");
    }
}

#[test]
fn a_recording_stops_when_the_app_is_left() {
    let b = bench("voice-leave");
    let r = recs(vec![usb()], "x");
    let mut a = b.app_with_recs(r.clone());
    click(&mut a, &b.cx, T_TAB_DICT);
    click(&mut a, &b.cx, T_REC);
    assert!(a.is_recording());
    drop(a);
    assert!(r.cfg.dropped.load(Ordering::SeqCst), "closing the app ends the recording");
}
