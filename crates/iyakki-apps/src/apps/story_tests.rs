// The tests of the TALES tile. The whole file is compiled only under `#[cfg(test)]` (declared in mod.rs), so the boundary scan counts it as test code.
use super::*;
use crate::apps::tests::{cx_with_backend, services_in, temp_dir, Offline};
use iyakki_shared::chat::{ChatBackend, Message, ModelTurn, Role as ChatRole, ToolSpec};
use iyakki_shared::voice::{encode_wav, tone, SpeechEngine};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const STORY: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said an owl. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept well.\n";

/// A model that records what it was asked and streams a story in small pieces.
struct Teller {
    seen: Mutex<Vec<(Vec<Message>, usize)>>,
    text: String,
    fail: Option<String>,
    delay_ms: u64,
    /// What the model answers when asked for a drawing.
    svg: Mutex<String>,
    /// What it answers when asked to choose a scene for the built-in cartoon.
    scene: Mutex<String>,
    /// When set, every drawing request fails with this.
    draw_error: Mutex<Option<String>>,
    /// Stories to answer with, one per request (then `text` again).
    queue: Mutex<Vec<String>>,
}

const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 400 240'><defs><linearGradient id='sky' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#223366'/><stop offset='1' stop-color='#88aadd'/></linearGradient></defs><rect width='400' height='240' fill='url(#sky)'/><circle cx='300' cy='60' r='28' fill='#ffeeaa'/><ellipse cx='120' cy='190' rx='90' ry='40' fill='#336644'/></svg>";

const SCENE: &str = "{\"setting\":\"meadow\",\"time\":\"day\",\"mood\":\"happy\",\"objects\":[{\"kind\":\"hero\",\"x\":30},{\"kind\":\"tree\",\"x\":85,\"size\":\"large\"}]}";

impl Teller {
    fn new(text: &str) -> Arc<Teller> {
        Arc::new(Teller { seen: Mutex::new(vec![]), text: text.into(), fail: None, delay_ms: 0, svg: Mutex::new(SVG.into()), scene: Mutex::new(SCENE.into()), draw_error: Mutex::new(None), queue: Mutex::new(vec![]) })
    }
    fn with(text: &str, fail: Option<&str>, delay_ms: u64) -> Arc<Teller> {
        Arc::new(Teller { seen: Mutex::new(vec![]), text: text.into(), fail: fail.map(str::to_string), delay_ms, svg: Mutex::new(SVG.into()), scene: Mutex::new(SCENE.into()), draw_error: Mutex::new(None), queue: Mutex::new(vec![]) })
    }
    /// The next requests for a story are answered with these texts, in order.
    fn queue(&self, texts: &[&str]) {
        *self.queue.lock().unwrap() = texts.iter().map(|t| t.to_string()).collect();
    }
    /// The requests that asked for a story.
    fn story_calls(&self) -> Vec<(Vec<Message>, usize)> {
        self.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("IYAKKI Tales")).cloned().collect()
    }
    /// The requests that asked for a drawing.
    fn drawing_calls(&self) -> Vec<(Vec<Message>, usize)> {
        self.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("small illustration") || c.0[0].content.contains("children's-book illustrator")).cloned().collect()
    }
}

struct Shared(Arc<Teller>);
impl ChatBackend for Shared {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        self.0.seen.lock().unwrap().push((h.to_vec(), 0));
        if h[0].content.contains("picture brief") {
            return Ok("A small fox sits by a quiet river as the moon rises.".into());
        }
        if h[0].content.contains("where and when a story happens") {
            return Ok("a river meadow at dusk, soft blue light".into());
        }
        if h[0].content.contains("small illustration") || h[0].content.contains("children's-book illustrator") {
            if let Some(e) = self.0.draw_error.lock().unwrap().clone() {
                return Err(e);
            }
            return Ok(if h[0].content.contains("scene composer") { self.0.scene.lock().unwrap().clone() } else { self.0.svg.lock().unwrap().clone() });
        }
        self.0.fail.clone().map_or(Ok(self.0.text.clone()), Err)
    }
    fn complete_tools_stream(&self, h: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.0.seen.lock().unwrap().push((h.to_vec(), tools.len()));
        if let Some(e) = &self.0.fail {
            return Err(e.clone());
        }
        let full = {
            let mut q = self.0.queue.lock().unwrap();
            if q.is_empty() { self.0.text.clone() } else { q.remove(0) }
        };
        let chars: Vec<char> = full.chars().collect();
        let mut sent = String::new();
        for piece in chars.chunks(23) {
            let p: String = piece.iter().collect();
            if self.0.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.0.delay_ms));
            }
            sent.push_str(&p);
            if !on_text(&p) {
                break;
            }
        }
        Ok(ModelTurn { text: sent, calls: vec![] })
    }
}

fn make(tag: &str, teller: &Arc<Teller>) -> (StoryApp, AppCx, PathBuf) {
    make_up(tag, teller, Arc::new(Offline))
}

fn make_up(tag: &str, teller: &Arc<Teller>, up: Arc<dyn iyakki_proxy::Upstream>) -> (StoryApp, AppCx, PathBuf) {
    let (mut cx, d) = cx_with_backend(tag, Shared(teller.clone()));
    cx.services = services_in(&temp_dir(&format!("{tag}-svc")), up);
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(home).unwrap();
    cx.session.policy.fs_read = vec![home.to_string_lossy().into_owned()];
    cx.session.policy.fs_write = vec![home.to_string_lossy().into_owned()];
    let mut app = StoryApp::new(&cx);
    app.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
    app.prefs = app.store.load_prefs();
    (app, cx, home)
}

fn typ(a: &mut StoryApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        a.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn key(a: &mut StoryApp, cx: &AppCx, k: Key) -> Flow {
    a.handle(&Event::Key(k), cx)
}

fn click(a: &mut StoryApp, cx: &AppCx, code: usize) {
    a.handle(&Event::Tab(code), cx);
}

/// Click the control named `label` of the last drawing.
fn click_hit(a: &mut StoryApp, cx: &AppCx, g: &Gfx, label: &str) {
    let ev = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control named {label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>())).ev.clone();
    a.handle(&ev, cx);
}

/// Run the heartbeat until `f` holds (or panic after ~8 s).
fn until(a: &mut StoryApp, cx: &AppCx, what: &str, f: impl Fn(&mut StoryApp) -> bool) {
    // a wall-clock wait: what is waited for runs processes, whose speed depends on the load of the machine
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        a.handle(&Event::Tick, cx);
        if f(a) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for: {what}; story page {:?}, error {:?}, reader {:?}, pictures {:?}, note {:?}", a.page_name(), a.error, a.reader.status(), a.pics.iter().map(|p| p.state.clone()).collect::<Vec<_>>(), a.note);
}

fn write_story(a: &mut StoryApp, cx: &AppCx, topic: &str) {
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), cx);
    typ(a, cx, topic);
    key(a, cx, Key::Ctrl('n'));
    until(a, cx, "the story to be written", |a| !a.is_writing());
}

fn draw(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, tick: u64) -> (Gfx, R) {
    let s = crate::ui::scale_for(w, h);
    draw_at(a, cx, w, h, s, tick)
}

fn draw_at(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, s: f32, tick: u64) -> (Gfx, R) {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    let top = 46.0 * s;
    let area = R::new(0.0, top, w as f32, h as f32 - top);
    a.render(&mut g, area, s, cx, tick);
    (g, area)
}

/// A speech engine that is a script: it logs what it was asked to say, then prints a short WAV (or fails when `fail` exists).
struct Engine {
    dir: PathBuf,
}
impl Engine {
    fn install(cx: &AppCx, dir: &Path, play: &str) -> Engine {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("tone.wav"), encode_wav(&tone(440.0, 120), 16_000)).unwrap();
        let script = dir.join("say.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat >> '{d}/said.log'\necho >> '{d}/said.log'\nif [ -f '{d}/fail' ]; then echo 'engine exploded' >&2; exit 3; fi\ncat '{d}/tone.wav'\n", d = dir.display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        cx.services
            .settings()
            .lock()
            .unwrap()
            .update(|s| {
                s.voice.tts = Some(SpeechEngine::Command { command: script.display().to_string() });
                s.voice.play_cmd = play.to_string();
            })
            .unwrap();
        Engine { dir: dir.to_path_buf() }
    }
    fn said(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("said.log")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
    }
}

fn logging_player(dir: &Path) -> String {
    logging_player_for(dir, "0.12")
}

/// A speaker that "plays" for `secs` seconds and then logs one line.
fn logging_player_for(dir: &Path, secs: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    format!("sh -c \"cat >/dev/null; sleep {secs}; echo p >> {}/played.log\"", dir.display())
}

/// A speaker that plays piece N (N pieces have been played before it) once the file `go.N` exists in `dir`.
fn gated_player(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join("gated.sh");
    std::fs::write(&script, format!("#!/bin/sh\ncat >/dev/null\nn=$(cat '{d}/played.log' 2>/dev/null | wc -l | tr -d ' ')\nwhile [ ! -f '{d}/go.'$n ]; do sleep 0.02; done\necho p >> '{d}/played.log'\n", d = dir.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script.display().to_string()
}

fn played(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("played.log")).map_or(0, |t| t.lines().count())
}

// ------------------------------------------------------------------ writing

#[test]
fn a_topic_with_options_is_written_by_the_model_without_tools_and_the_story_is_saved() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-write", &t);
    assert_eq!((a.page_name(), a.focus_name()), ("story", "idea"), "ready to type");
    // pick options by clicking chips
    click(&mut a, &cx, T_AUDIENCE + 1);
    click(&mut a, &cx, T_GENRE + 3);
    click(&mut a, &cx, T_LENGTH + 1);
    click(&mut a, &cx, T_LANG + 2);
    click(&mut a, &cx, T_TONE + 3);
    click(&mut a, &cx, T_SUGGEST); // an idea from the list fills the topic
    assert_eq!(a.options().topic, SUGGESTIONS[0]);
    assert_eq!(a.focus_name(), "idea");
    // an empty topic is refused with a hint, and nothing is asked of the model
    a.idea.set("");
    key(&mut a, &cx, Key::Ctrl('n'));
    assert!(a.note_text().unwrap().contains("Type a topic"), "{:?}", a.note_text());
    assert!(t.seen.lock().unwrap().is_empty(), "nothing was asked of the model");
    write_story(&mut a, &cx, "a fox and the moon with Mala and her goat");
    assert_eq!(a.page_name(), "story");
    let seen = t.story_calls();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, 0, "no tools are ever offered");
    let (sys, user) = (&seen[0].0[0], &seen[0].0[1]);
    assert_eq!((sys.role, user.role), (ChatRole::System, ChatRole::User));
    assert!(sys.content.contains("SAFETY (teenagers)") && sys.content.contains("YOU HAVE NO TOOLS") && sys.content.contains("in Hindi"), "{}", sys.content);
    assert!(user.content.contains("a fox and the moon") && user.content.contains("Mala and her goat") && user.content.contains("science-fiction") == false);
    assert!(sys.content.contains("science-fiction"), "the genre chip reached the prompt");
    assert!(a.story_text().contains("Once upon a time") && a.story_text().starts_with("The Brave Little Fox"));
    // it was saved in the account's folder, with the options, and listed
    let id = a.saved_id().expect("saved");
    assert_eq!(id, "the-brave-little-fox");
    let md = std::fs::read_to_string(home.join("Stories/the-brave-little-fox/story.md")).unwrap();
    assert!(md.starts_with("---\ntitle: The Brave Little Fox") && md.contains("audience: teen") && md.contains("language: Hindi") && md.contains("# The Brave Little Fox"), "{md}");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("story.create") && log.contains("teen scifi medium Hindi"), "{log}");
    assert!(!log.contains("a fox and the moon"), "the topic text is not logged");
    // the choices are remembered for the next visit, the topic is not
    assert_eq!(a.store.load_prefs().options.audience, Audience::Teen);
    assert_eq!(a.store.load_prefs().options.language, "Hindi");
    assert!(a.store.load_prefs().options.topic.is_empty(), "the topic itself is not kept");
}

#[test]
fn the_story_appears_as_it_is_written_paragraph_by_paragraph() {
    let t = Teller::with(STORY, None, 60);
    let (mut a, cx, _) = make("tales-stream", &t);
    let _ = draw(&mut a, &cx, 1280, 720, 0); // the first draw loads the fonts: not while the clock runs
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a fox");
    key(&mut a, &cx, Key::Ctrl('n'));
    assert!(a.is_writing());
    let (mut max_blocks, mut saw_partial, mut counts) = (0, false, vec![]);
    for _ in 0..600u64 {
        a.handle(&Event::Tick, &cx);
        if a.story.blocks.len() > max_blocks {
            max_blocks = a.story.blocks.len();
            counts.push(a.story.blocks.len());
        }
        saw_partial |= !a.story.partial.is_empty();
        if !a.is_writing() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!a.is_writing());
    assert!(saw_partial, "a paragraph in progress is shown before it is finished");
    assert_eq!(counts, vec![1, 2, 3], "the paragraphs were completed one after another: {counts:?}");
    assert_eq!(a.story.title, "The Brave Little Fox");
    assert!(a.saved_id().is_some());
}

#[test]
fn the_paragraph_in_progress_is_drawn_after_the_finished_ones_with_moving_dots() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-partial", &t);
    a.story = st::parse_story("# The Fox\n\nOne finished paragraph here.\n\nHalf a sente", false);
    assert_eq!((a.story.blocks.len(), a.story.partial.as_str()), (1, "Half a sente"));
    let (_tx, rx) = channel();
    a.gen = Some(Gen { rx, stop: Arc::new(AtomicBool::new(false)) });
    a.go(Page::Story, &cx);
    let (g1, area) = draw(&mut a, &cx, 1280, 720, 0);
    let (g2, _) = draw(&mut a, &cx, 1280, 720, 10);
    assert_ne!(g1.rgba(), g2.rgba(), "the dots move");
    assert_layout(&g1, area, "story page while writing");
    assert!(g1.hits.iter().any(|h| h.label == "Stop writing"), "and the person can stop it");
    a.gen = None;
}

#[test]
fn a_failing_model_says_why_and_a_story_stopped_early_is_still_kept() {
    let t = Teller::with(STORY, Some("No chat model configured. Open Settings → Models and add a provider."), 0);
    let (mut a, cx, _) = make("tales-fail", &t);
    write_story(&mut a, &cx, "a fox");
    assert!(a.error_text().unwrap().contains("Settings → Models"), "{:?}", a.error_text());
    assert!(a.saved_id().is_none() && a.library_ids().is_empty());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Send"), "the composer is there to try again");
    // stopped by the person after the first paragraph: what was written is kept
    let t2 = Teller::with(STORY, None, 15);
    let (mut b, cx2, home) = make("tales-stop", &t2);
    b.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx2);
    typ(&mut b, &cx2, "a fox");
    key(&mut b, &cx2, Key::Ctrl('n'));
    until(&mut b, &cx2, "a paragraph", |a| !a.story.blocks.is_empty());
    click(&mut b, &cx2, T_STOP_WRITING);
    until(&mut b, &cx2, "the end", |a| !a.is_writing());
    let id = b.saved_id().expect("kept");
    assert!(b.note_text().unwrap().contains("Stopped early"), "{:?}", b.note_text());
    let md = std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap();
    assert!(md.contains("Once upon a time") && !md.contains("slept well"), "{md}");
}

// ------------------------------------------------------------------ reading aloud

#[test]
fn without_a_speech_engine_the_story_is_still_written_and_the_page_says_how_to_turn_reading_on() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-notts", &t);
    assert_eq!(*a.tts_status(), TtsStatus::NoEngine);
    let help = st::status_text(a.tts_status());
    assert!(help.contains("Settings → Voice") && help.contains("/v1/audio/speech") && help.contains("piper"), "{help}");
    write_story(&mut a, &cx, "a fox");
    assert!(a.saved_id().is_some(), "written and saved as text");
    assert!(a.story_text().contains("slept well"));
    click(&mut a, &cx, T_PLAY);
    assert_eq!(a.note_text(), Some(help.as_str()), "pressing play explains it");
    assert_eq!(a.reader_status(), Status::Idle);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Play"), "the player is drawn all the same");
    // an account that may not use the speaker is told that instead
    let mut cx2 = cx.clone();
    cx2.session.policy.voice = false;
    a.refresh_tts(&cx2);
    assert_eq!(*a.tts_status(), TtsStatus::NotAllowed);
    assert!(st::status_text(a.tts_status()).contains("not enabled for your account"));
    // a missing speaker command
    let engine_dir = temp_dir("tales-notts-eng");
    let _e = Engine::install(&cx, &engine_dir, "nosuchplayer-xyz -q");
    a.refresh_tts(&cx);
    assert_eq!(*a.tts_status(), TtsStatus::NoPlayer("nosuchplayer-xyz".into()));
}

#[test]
fn one_click_turns_on_a_local_piper_when_it_is_installed() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-quick", &t);
    // nothing to offer on this machine's settings: the page says so instead of showing a button
    a.quick.clear();
    a.quick.push(st::QuickEngine { label: "Use piper (en_US-test)".into(), engine: SpeechEngine::Command { command: "piper --model /v/en.onnx --output_file {out}".into() } });
    click(&mut a, &cx, T_QUICK);
    assert!(matches!(cx.voice_settings().tts, Some(SpeechEngine::Command { ref command }) if command.starts_with("piper")), "{:?}", cx.voice_settings().tts);
    assert!(a.note_text().unwrap().contains("piper (en_US-test)"), "{:?}", a.note_text());
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("story.engine"));
    // the button is only drawn for administrators
    let mut c2 = cx.clone();
    c2.session.policy.manage_settings = false;
    cx.settings().lock().unwrap().update(|s| s.voice.tts = None).unwrap();
    a.refresh_tts(&c2);
    assert!(a.quick.is_empty(), "no one-click buttons for someone who cannot change Settings");
}

#[test]
fn the_story_is_spoken_in_order_while_it_is_written_and_the_page_follows_along() {
    let _route = iyakki_agent::mixer::pin_command_route(); // these play through the speaker command
    let t = Teller::with(STORY, None, 15);
    let (mut a, cx, _) = make("tales-read", &t);
    let _ = draw(&mut a, &cx, 1280, 720, 0);
    let dir = temp_dir("tales-read-eng");
    // a speaker that holds each piece until the test has looked at it (the file go.N lets piece N end): what the page shows
    // while a piece plays is looked at while it plays, however slow or fast the machine is
    let player = gated_player(&dir.join("p"));
    let eng = Engine::install(&cx, &dir, &player);
    a.refresh_tts(&cx);
    assert!(matches!(a.tts_status(), TtsStatus::Ready { .. }));
    assert!(a.prefs.read_while_writing, "reading while writing is on by default");
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a fox");
    key(&mut a, &cx, Key::Ctrl('n'));
    // reading starts before the model has finished
    until(&mut a, &cx, "reading to start", |a| a.live && matches!(a.reader_status(), Status::Playing | Status::Buffering | Status::Done));
    let early = a.is_writing();
    let mut highlighted = std::collections::BTreeSet::new();
    let mut released = std::collections::BTreeSet::new();
    // the wait is for the reading to end (a state), with a wall-clock limit that only matters when something is wrong
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut tick = 0u64;
    loop {
        assert!(Instant::now() < deadline, "the reading did not end: {:?}, writing {}", a.reader_status(), a.is_writing());
        a.handle(&Event::Tick, &cx);
        let snap = a.reader.snapshot();
        if matches!(snap.status, Status::Playing) && !released.contains(&snap.chunk) {
            // this piece plays now and is held: the page lights its paragraph and offers Pause
            highlighted.insert(snap.unit);
            let (g, _) = draw(&mut a, &cx, 1280, 720, tick);
            assert_eq!(a.reader_status(), Status::Playing, "held, so still playing");
            assert!(g.hits.iter().any(|h| h.label == "Pause"), "the play button turns into pause while it plays");
            released.insert(snap.chunk);
            std::fs::write(dir.join("p").join(format!("go.{}", snap.chunk)), "").unwrap();
        }
        if a.reader_status() == Status::Done && !a.is_writing() {
            break;
        }
        tick += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(a.reader_status(), Status::Done);
    assert!(early || a.chunks.len() > 3, "(started while writing: {early})");
    // the engine was asked for the sentences in story order, each once, title first
    let said = eng.said();
    assert_eq!(said[0], "The Brave Little Fox");
    let spoken = said.join(" ");
    let (i1, i2, i3) = (spoken.find("Once upon a time").unwrap(), spoken.find("One night").unwrap(), spoken.find("She walked home").unwrap());
    assert!(i1 < i2 && i2 < i3, "in order: {said:?}");
    assert_eq!(said.len(), a.chunks.len(), "one request per piece");
    assert_eq!(played(&dir.join("p")), a.chunks.len(), "and every piece was played");
    assert!(highlighted.len() >= 2, "the highlight moved along the paragraphs: {highlighted:?}");
    assert_eq!(a.reader.snapshot().fraction, 1.0);
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("story.read"));
}

// ------------------------------------------------------------------ the word being said

/// A speaker whose voice reports the word it is on (like the browser voice's boundary events); it never ends by itself.
struct WordSink {
    at: Arc<Mutex<Option<(usize, usize)>>>,
}
struct WordChild {
    at: Arc<Mutex<Option<(usize, usize)>>>,
    until: Instant,
}
impl st::Playing for WordChild {
    fn is_playing(&mut self) -> bool {
        Instant::now() < self.until
    }
    fn stop(&mut self) {}
    fn spoken_word(&mut self) -> Option<(usize, usize)> {
        *self.at.lock().unwrap()
    }
}
impl st::Sink for WordSink {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn st::Playing>, String> {
        Ok(Box::new(WordChild { at: self.at.clone(), until: Instant::now() + Duration::from_secs(3600) }))
    }
}
struct ToneSynth;
impl st::Synth for ToneSynth {
    fn synth(&self, _text: &str) -> Result<Vec<u8>, String> {
        // long, so that the timing model moves slowly and the voice's own word is what the page shows
        Ok(encode_wav(&tone(440.0, 60_000), 16_000))
    }
}

/// A written story whose chunk `chunk` is being read, with the voice on word `word` (None: no word reported yet).
/// The time of the reader in these tests: it only moves when a test moves it (milliseconds).
type Moved = Arc<std::sync::atomic::AtomicU32>;

fn reading_app(tag: &str, chunk: usize, word: Option<usize>) -> (StoryApp, AppCx, Arc<Mutex<Option<(usize, usize)>>>, Moved) {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make(tag, &t);
    // the word timing runs on a clock the test holds: how long the machine takes to draw or to start processes cannot
    // move the word that is lit
    let time: Moved = Arc::new(std::sync::atomic::AtomicU32::new(1000));
    let t2 = time.clone();
    a.reader.set_time_source(Arc::new(move || t2.load(std::sync::atomic::Ordering::SeqCst) as f32));
    write_story(&mut a, &cx, "a fox");
    a.chunks = st::chunks_for(&a.story.units(), 0, st::MAX_CHUNK_CHARS);
    a.chunked_units = a.story.units().len();
    let at = Arc::new(Mutex::new(None));
    a.reader.start(a.chunks.clone(), chunk, false, "t", Arc::new(ToneSynth), Arc::new(WordSink { at: at.clone() }));
    a.live = true;
    a.manual_unit = None;
    until(&mut a, &cx, "playing", |a| a.reader_status() == Status::Playing);
    if let Some(k) = word {
        let n = iyakki_wordtime::words_of(&a.chunks[chunk].text).len();
        *at.lock().unwrap() = Some((k.min(n - 1), n));
        until(&mut a, &cx, "an exact word", |a| a.reader.snapshot().word_exact);
    }
    (a, cx, at, time)
}

fn word_text(a: &StoryApp) -> String {
    let snap = a.reader.snapshot();
    let (u, (x, y)) = (snap.unit, snap.word.expect("a word"));
    a.story.units()[u].chars().skip(x).take(y - x).collect()
}

#[test]
fn the_sentence_and_the_word_being_said_are_lit_inside_the_text_at_every_screen_size() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        // the chunk of the second paragraph (several sentences, wrapping on a small screen), each of its words in turn
        let (mut a, cx, at, _time) = reading_app(&format!("tales-word-{w}"), 2, Some(0));
        let n = iyakki_wordtime::words_of(&a.chunks[2].text).len();
        assert!(n > 12, "{n}");
        let mut lines_seen = std::collections::BTreeSet::new();
        for k in [0, 5, 10, n - 1] {
            *at.lock().unwrap() = Some((k, n));
            let (c0, sp) = (a.chunks[2].start, iyakki_wordtime::words_of(&a.chunks[2].text)[k]);
            until(&mut a, &cx, "the word", |a| a.reader.snapshot().word == Some((c0 + sp.start, c0 + sp.end)));
            let want = {
                let c = &a.chunks[2];
                let sp = iyakki_wordtime::words_of(&c.text)[k];
                c.text.chars().skip(sp.start).take(sp.end - sp.start).collect::<String>()
            };
            assert_eq!(word_text(&a), want, "{w}x{h} word {k}");
            // a few frames, so that the scroll follows and the easing settles
            let mut g = draw(&mut a, &cx, w, h, 0).0;
            for t in 1..10 {
                g = draw(&mut a, &cx, w, h, t).0;
            }
            let s = crate::ui::scale_for(w, h);
            let word = a.word_rect.unwrap_or_else(|| panic!("{w}x{h}: word {k} ({want}) was not drawn"));
            let area = a.text_area;
            assert!(word.x >= area.x - 4.0 * s && word.x + word.w <= area.x + area.w + 4.0 * s, "{w}x{h} word {k}: {word:?} not inside {area:?}");
            assert!(word.y >= area.y - 2.0 && word.y + word.h <= area.y + area.h + 2.0, "{w}x{h} word {k}: {word:?} not inside {area:?} (the page follows the word)");
            assert!(!a.sentence_rects.is_empty());
            assert!(a.sentence_rects.iter().any(|r| word.x >= r.x - 0.6 && word.x + word.w <= r.x + r.w + 0.6 && (word.y - r.y).abs() < 0.6), "{w}x{h} word {k}: not inside the sentence {:?} / {word:?}", a.sentence_rects);
            // a solid gold fill (not a wash) at the left edge of the highlight, where no glyph is drawn (the right edge when the
            // word starts the line: the left padding of the fill is outside the text area, which clips it)
            let (lx, rx) = (word.x + 1.5 * s, word.x + word.w - 1.5 * s);
            let px = g.pm_mut().pixel((if lx >= area.x + 0.5 { lx } else { rx }) as u32, (word.y + word.h / 2.0) as u32).expect("pixel");
            assert!(px.red() > 200 && px.green() > 170 && px.blue() > 80, "{w}x{h} word {k}: {px:?}");
            lines_seen.insert((word.y * 10.0) as i32);
        }
        assert!(lines_seen.len() >= 2, "{w}x{h}: the word moved over several lines {lines_seen:?}");
        a.reader.stop();
    }
}

#[test]
fn the_highlight_setting_chooses_sentence_and_word_sentence_only_or_nothing() {
    use iyakki_shared::voice::Highlight;
    let (mut a, cx, _at, _time) = reading_app("tales-hl-setting", 2, Some(2));
    let set = |h: Highlight| cx.settings().lock().unwrap().update(|s| s.voice.highlight = h).unwrap();
    for t in 0..10 {
        let _ = draw(&mut a, &cx, 1280, 720, t);
    }
    assert!(a.word_rect.is_some() && !a.sentence_rects.is_empty(), "the default is sentence and word");
    set(Highlight::Sentence);
    let _ = draw(&mut a, &cx, 1280, 720, 2);
    assert!(a.word_rect.is_none() && !a.sentence_rects.is_empty(), "sentence only");
    set(Highlight::Off);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 3);
    assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "off");
    // and still reading, with the page following along
    assert_eq!(a.reader_status(), Status::Playing);
    assert!(g.hits.iter().any(|h| h.label == "Pause"));
    a.reader.stop();
    set(Highlight::SentenceAndWord);
    let _ = draw(&mut a, &cx, 1280, 720, 4);
    assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "nothing is lit when nothing is read");
}

#[test]
fn pausing_freezes_the_word_on_the_page_and_stopping_clears_it() {
    let (mut a, cx, _at, time) = reading_app("tales-hl-pause", 2, Some(5));
    for t in 0..25 {
        let _ = draw(&mut a, &cx, 1280, 720, t);
    }
    let word = a.reader.snapshot().word;
    assert!(a.word_rect.is_some());
    click(&mut a, &cx, T_PLAY);
    assert_eq!(a.reader_status(), Status::Paused);
    assert_eq!(a.reader.snapshot().word, word, "the word stays the one that was lit");
    // (the fill eases from the word before it, so let it arrive)
    for t in 100..115 {
        let _ = draw(&mut a, &cx, 1280, 720, t);
    }
    let lit = a.word_rect.expect("lit");
    for t in 2..8 {
        // time passes (the real clock is not what moves the word): a paused word stays where it is
        time.fetch_add(30_000, std::sync::atomic::Ordering::SeqCst);
        let _ = draw(&mut a, &cx, 1280, 720, t);
        assert_eq!(a.word_rect, Some(lit), "the paused word does not move");
    }
    click(&mut a, &cx, T_STOP);
    let _ = draw(&mut a, &cx, 1280, 720, 9);
    assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "stop clears the highlight");
}

#[test]
fn frames_are_asked_for_only_while_the_story_is_being_read() {
    let (mut a, cx, _at, _time) = reading_app("tales-frames", 2, Some(1));
    let _ = a.wants_redraw();
    let mut n = 0;
    for _ in 0..30 {
        a.handle(&Event::Tick, &cx);
        if a.wants_redraw() {
            n += 1;
        }
    }
    assert!((8..=12).contains(&n), "about every third heartbeat while reading, got {n}");
    a.reader.pause();
    for _ in 0..40 {
        // (the level bars settle for a moment)
        a.handle(&Event::Tick, &cx);
        let _ = a.wants_redraw();
    }
    let idle = (0..30).filter(|_| {
        a.handle(&Event::Tick, &cx);
        a.wants_redraw()
    });
    assert_eq!(idle.count(), 0, "paused: nothing moves, no frames");
    a.reader.stop();
    for _ in 0..40 {
        a.handle(&Event::Tick, &cx);
        let _ = a.wants_redraw();
    }
    let idle = (0..30).filter(|_| {
        a.handle(&Event::Tick, &cx);
        a.wants_redraw()
    });
    assert_eq!(idle.count(), 0, "stopped: no frames");
}

/// The screen point of character `pos` of unit `unit` in the last drawing.
fn point_of(a: &StoryApp, g: &mut Gfx, s: f32, unit: usize, pos: usize) -> (f32, f32) {
    let l = (0..a.line_map.len()).rev().find(|i| a.line_map[*i].0 == unit && a.line_map[*i].1 <= pos).expect("line");
    let start = a.line_map[l].1;
    let text: String = a.lay[unit].lines.iter().find(|(_, st0)| *st0 == start).map(|(t, _)| t.clone()).unwrap();
    let prefix: String = text.chars().take(pos - start).collect();
    let x = a.text_area.x + g.text_width(16.0 * s, &prefix, Face::Sans) + 3.0 * s;
    (x, a.line_tops[l] + g.line_height(16.0 * s, Face::Sans) / 2.0)
}

#[test]
fn a_click_on_a_sentence_reads_from_that_sentence_but_a_double_click_or_a_drag_does_not() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-click-read", &t);
    let dir = temp_dir("tales-click-eng");
    let eng = Engine::install(&cx, &dir, &logging_player_for(&dir.join("p"), "0.2"));
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    // the end of the story is in view (the first paragraph has a picture slot above it)
    a.tscroll = 1.0e6;
    let _ = draw(&mut a, &cx, 1280, 720, 0);
    let (mut g, _) = draw(&mut a, &cx, 1280, 720, 1);
    let s = crate::ui::scale_for(1280, 720);
    // the third sentence of the second paragraph
    let unit = a.story.units().iter().position(|u| u.contains("The fox listened carefully.")).unwrap();
    let pos = a.story.units()[unit].chars().count() - "The fox listened carefully.".chars().count();
    let before = a.chunks.len();
    assert_eq!(a.reader_status(), Status::Idle);
    let (x, y) = point_of(&a, &mut g, s, unit, pos + 2);
    let press = |a: &mut StoryApp, cx: &AppCx, x: f32, y: f32, clicks: u8, drag: bool| {
        crate::ui::set_click_count(clicks);
        crate::ui::set_dragging(drag);
        a.handle(&Event::AreaAt(TEXT_PANE, x - a.text_area.x, y - a.text_area.y), cx);
        crate::ui::set_dragging(false);
        crate::ui::set_click_count(1);
    };
    let release = |a: &mut StoryApp, cx: &AppCx| {
        a.handle(&Event::MouseUp(0, 0), cx);
    };
    // a double click (the word is selected) never starts reading
    press(&mut a, &cx, x, y, 2, false);
    release(&mut a, &cx);
    for _ in 0..40 {
        a.handle(&Event::Tick, &cx);
    }
    assert_eq!(a.reader_status(), Status::Idle);
    // neither does a drag
    press(&mut a, &cx, x, y, 1, false);
    press(&mut a, &cx, x + 30.0, y, 1, true);
    release(&mut a, &cx);
    for _ in 0..40 {
        a.handle(&Event::Tick, &cx);
    }
    assert_eq!(a.reader_status(), Status::Idle);
    a.pane.clear();
    // a plain click waits out the double-click time, then reads from the start of the sentence it is in
    press(&mut a, &cx, x, y, 1, false);
    release(&mut a, &cx);
    for _ in 0..5 {
        a.handle(&Event::Tick, &cx);
    }
    assert_eq!(a.reader_status(), Status::Idle, "still waiting for a possible second click");
    for _ in 0..20 {
        a.handle(&Event::Tick, &cx);
    }
    until(&mut a, &cx, "reading from the click", |a| matches!(a.reader_status(), Status::Playing | Status::Buffering | Status::Done));
    let snap = a.reader.snapshot();
    assert_eq!(snap.unit, unit);
    assert_eq!(snap.range.0, pos, "the piece starts at the clicked sentence");
    assert!(a.chunks.len() > before.max(1) - 1);
    until(&mut a, &cx, "the engine to be asked", |_| !eng.said().is_empty());
    assert_eq!(eng.said()[0], "The fox listened carefully.", "the first thing said is the clicked sentence: {:?}", eng.said());
    a.reader.stop();
}

/// Whether a process with this id exists (signal 0 asks the kernel; there is no /proc on a Mac: KI-664).
fn process_exists(n: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(n as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[test]
fn play_pause_stop_and_replay_end_the_speaker_process() {
    let _route = iyakki_agent::mixer::pin_command_route(); // these play through the speaker command
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-ctl", &t);
    let dir = temp_dir("tales-ctl-eng");
    let pid = dir.join("pid");
    let _eng = Engine::install(&cx, &dir, &format!("sh -c \"echo $$ > {}; cat >/dev/null; sleep 30\"", pid.display()));
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    assert_eq!(a.reader_status(), Status::Idle, "nothing is read until asked");
    let alive = |p: &Path| -> Option<u32> {
        let n: u32 = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
        process_exists(n).then_some(n)
    };
    let gone = |a: &mut StoryApp, cx: &AppCx, n: u32| until(a, cx, "the speaker process to end", |_| !process_exists(n));
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the first piece to play", |a| alive(&pid).is_some() && a.reader_status() == Status::Playing);
    let first = alive(&pid).unwrap();
    // pause ends the process; resume starts a new one for the same piece
    click(&mut a, &cx, T_PLAY);
    gone(&mut a, &cx, first);
    assert_eq!(a.reader_status(), Status::Paused);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Play"));
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "a new process", |a| alive(&pid).is_some_and(|n| n != first) && a.reader_status() == Status::Playing);
    let second = alive(&pid).unwrap();
    // stop kills it and the position is forgotten
    click(&mut a, &cx, T_STOP);
    gone(&mut a, &cx, second);
    assert_eq!(a.reader_status(), Status::Idle);
    // next part, then replay from the start, then leaving the tile ends the sound too
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "playing", |_| alive(&pid).is_some_and(|n| n != second));
    let third = alive(&pid).unwrap();
    click(&mut a, &cx, T_NEXT);
    gone(&mut a, &cx, third);
    until(&mut a, &cx, "the next part", |a| a.reader.snapshot().unit > 0 && alive(&pid).is_some_and(|n| n != third));
    assert!(a.reader.snapshot().unit >= 1);
    click(&mut a, &cx, T_REPLAY);
    until(&mut a, &cx, "replay from the start", |a| a.reader.snapshot().chunk == 0 && a.reader_status() == Status::Playing);
    let last = alive(&pid);
    drop(a);
    if let Some(n) = last {
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            if !process_exists(n) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the speaker process {n} survived leaving the tile");
    }
}

#[test]
fn a_speech_engine_that_fails_is_reported_on_the_page() {
    let _route = iyakki_agent::mixer::pin_command_route(); // these play through the speaker command
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-err", &t);
    let dir = temp_dir("tales-err-eng");
    let _eng = Engine::install(&cx, &dir, &logging_player(&dir.join("p")));
    std::fs::write(dir.join("fail"), "").unwrap();
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the failure", |a| matches!(a.reader_status(), Status::Failed(_)));
    let e = a.error_text().unwrap().to_string();
    assert!(e.contains("Could not make the speech for part 1") && e.contains("engine exploded"), "{e}");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Play"), "retry is one click away");
    assert_eq!(played(&dir.join("p")), 0);
    // fixing the engine and pressing replay works
    std::fs::remove_file(dir.join("fail")).unwrap();
    click(&mut a, &cx, T_REPLAY);
    until(&mut a, &cx, "done", |a| a.reader_status() == Status::Done);
    assert!(played(&dir.join("p")) > 0);
}

#[test]
fn voice_and_speed_are_chosen_from_the_engine_and_remembered() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-voice", &t);
    cx.services
        .settings()
        .lock()
        .unwrap()
        .update(|s| {
            s.voice.tts = Some(SpeechEngine::Http { url: "https://api.openai.com/v1/audio/speech".into(), model: "tts-1".into(), key_ref: None, voice: Some("nova".into()), language: None });
            s.voice.play_cmd = "sh -c true".into();
        })
        .unwrap();
    a.refresh_tts(&cx);
    assert_eq!(a.voices[0], "nova");
    assert!(a.voices.len() >= 6 && a.takes_speed);
    click(&mut a, &cx, T_VOICE);
    assert_eq!(a.prefs.voice.as_deref(), Some("alloy"));
    click(&mut a, &cx, T_SPEED);
    assert!((a.prefs.speed - 1.1).abs() < 0.001);
    assert_eq!(a.store.load_prefs().voice.as_deref(), Some("alloy"), "remembered for the next visit");
    assert!((a.store.load_prefs().speed - 1.1).abs() < 0.001);
    write_story_nofocus(&mut a, &cx);
    assert!(a.saved_id().is_some());
    // a command engine without {voice} or {speed} has neither chip
    cx.settings().lock().unwrap().update(|s| s.voice.tts = Some(SpeechEngine::Command { command: "piper --output_file {out}".into() })).unwrap();
    a.refresh_tts(&cx);
    assert!(a.voices.is_empty() && !a.takes_speed);
}

fn write_story_nofocus(a: &mut StoryApp, cx: &AppCx) {
    a.open_form(cx);
    a.idea.set("a fox");
    a.sync_fields();
    a.start_create(cx);
    until(a, cx, "the story", |a| !a.is_writing());
}

// ------------------------------------------------------------------ audio files and the library

#[test]
fn audio_is_saved_next_to_the_story_and_heard_again_later_without_a_speech_engine() {
    let _route = iyakki_agent::mixer::pin_command_route(); // these play through the speaker command
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-audio", &t);
    let dir = temp_dir("tales-audio-eng");
    let eng = Engine::install(&cx, &dir, &logging_player(&dir.join("p")));
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    let id = a.saved_id().unwrap();
    click(&mut a, &cx, T_SAVE_AUDIO);
    assert!(a.audio_job.is_some());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Cancel audio")), "progress and a cancel button while it is made");
    until(&mut a, &cx, "the audio file", |a| a.audio_job.is_none());
    assert!(a.note_text().unwrap().starts_with("Audio saved: "), "{:?}", a.note_text());
    let wav = home.join(format!("Stories/{id}/audio.wav"));
    assert!(wav.is_file() && std::fs::read(&wav).unwrap().starts_with(b"RIFF"));
    assert!(home.join(format!("Stories/{id}/audio.json")).is_file());
    assert_eq!(eng.said().len(), a.chunks.len(), "every piece was made once");
    // share the text and the audio through the host-files save dialog
    click(&mut a, &cx, T_EXPORT);
    key(&mut a, &cx, Key::Down); // the second line of the menu: the text
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().unwrap();
    assert_eq!(name, format!("{id}.md"));
    assert!(String::from_utf8(bytes).unwrap().contains("# The Brave Little Fox"));
    click(&mut a, &cx, T_EXPORT);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().unwrap();
    assert_eq!((name.as_str(), &bytes[..4]), ((format!("{id}.wav")).as_str(), &b"RIFF"[..]));
    assert!(a.take_host_save().is_none(), "taken once");
    // later: no engine at all, the saved sound is played piece by piece with the page following
    cx.settings().lock().unwrap().update(|s| s.voice.tts = None).unwrap();
    let mut b = StoryApp::new(&cx);
    b.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
    b.go(Page::Library, &cx);
    assert_eq!(*b.tts_status(), TtsStatus::NoEngine);
    assert_eq!(b.library_ids(), vec![id.clone()]);
    let before = eng.said().len();
    click(&mut b, &cx, T_LIB_LISTEN);
    assert_eq!(b.page_name(), "story");
    assert!(b.saved_audio.is_some() && b.can_read());
    until(&mut b, &cx, "the saved audio to play out", |b| b.reader_status() == Status::Done);
    assert_eq!(played(&dir.join("p")), b.chunks.len(), "every piece of the saved audio was played, once");
    assert_eq!(eng.said().len(), before, "the engine was not asked again");
    let (g, _) = draw(&mut b, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Play"));
}

#[test]
fn the_library_lists_searches_opens_favourites_and_deletes_with_a_question_first() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-lib", &t);
    write_story(&mut a, &cx, "a fox");
    let first = a.saved_id().unwrap();
    a.open_form(&cx);
    a.idea.set("owls");
    a.sync_fields();
    a.start_create(&cx);
    until(&mut a, &cx, "the second story", |a| !a.is_writing());
    let second = a.saved_id().unwrap();
    assert_ne!(first, second, "the same title does not overwrite");
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(a.page_name(), "library");
    assert_eq!(a.library_ids().len(), 2);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Listen") && g.hits.iter().any(|h| h.label == "Delete"), "the chosen row has its actions");
    // search
    key(&mut a, &cx, Key::Char('/'));
    assert_eq!(a.focus_name(), "search");
    typ(&mut a, &cx, "zebra");
    assert!(a.library_ids().is_empty());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().all(|h| h.label != "Listen"));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "owl");
    assert_eq!(a.library_ids().len(), 2, "the word is in the story text");
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.focus_name(), "none");
    // favourite
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('f'));
    let chosen = a.library_ids()[1].clone();
    assert!(a.store.load(&chosen).unwrap().entry.favourite);
    // delete asks first; anything but y keeps it
    key(&mut a, &cx, Key::Char('d'));
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Cancel"), "a confirmation dialog is up");
    key(&mut a, &cx, Key::Char('n'));
    assert_eq!(a.library_ids().len(), 2, "n did not delete");
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(a.library_ids().len(), 1);
    assert!(!home.join(format!("Stories/{chosen}")).exists());
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("story.delete"));
    // open: the saved text is on the page and the list survives a new visit
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.page_name(), "story");
    assert!(a.story_text().contains("Once upon a time"));
    let b = {
        let mut b = StoryApp::new(&cx);
        b.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
        b.go(Page::Library, &cx);
        b
    };
    assert_eq!(b.library_ids().len(), 1);
}

#[test]
fn a_story_cannot_be_saved_outside_what_the_account_may_write_and_is_still_shown() {
    let t = Teller::new(STORY);
    let (mut a, mut cx, home) = make("tales-scope", &t);
    cx.session.policy.fs_write.clear();
    a.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
    write_story(&mut a, &cx, "a fox");
    assert!(a.story_text().contains("slept well"), "the story is on the page");
    assert!(a.saved_id().is_none());
    assert!(a.note_text().unwrap().contains("could not be saved") && a.note_text().unwrap().contains("may not save"), "{:?}", a.note_text());
    assert!(!home.join("Stories").exists());
    click(&mut a, &cx, T_SAVE_AUDIO);
    assert!(a.audio_job.is_none());
}

// ------------------------------------------------------------------ selection, focus, drawing

#[test]
fn the_story_text_can_be_selected_and_copied() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-copy", &t);
    write_story(&mut a, &cx, "a fox");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    let pane = g.hits.iter().find(|h| h.label == "text area").expect("the text is selectable").rect;
    crate::ui::set_click_count(3);
    crate::ui::set_dragging(false);
    a.handle(&Event::AreaAt(TEXT_PANE, 20.0, 6.0), &cx); // a triple click on the first line
    crate::ui::set_click_count(1);
    let _ = draw(&mut a, &cx, 1280, 720, 1);
    key(&mut a, &cx, Key::Ctrl('c'));
    let copied = crate::ui::clipboard::latest().unwrap_or_default();
    assert!(copied.starts_with("Once upon a time a small fox"), "{copied:?}");
    assert!(!copied.contains("One night"), "just the paragraph: {copied:?}");
    // select all and copy the lot (the title is the hero, the paragraphs are the text)
    key(&mut a, &cx, Key::Ctrl('a'));
    let _ = draw(&mut a, &cx, 1280, 720, 2);
    key(&mut a, &cx, Key::Ctrl('c'));
    let all = crate::ui::clipboard::latest().unwrap_or_default();
    assert!(all.contains("Once upon a time") && all.contains("slept well"), "{all:?}");
    // the right-click menu offers Copy over the text
    let items = a.context_items((pane.x + 10.0) as i32, (pane.y + 10.0) as i32);
    assert!(items.iter().any(|i| i.label == "Copy"));
    // a press in the text takes the keyboard away from a field
    a.focus = Focus::Search;
    a.handle(&Event::AreaAt(TEXT_PANE, 5.0, 5.0), &cx);
    assert_eq!(a.focus_name(), "none");
}

#[test]
fn exactly_one_thing_owns_the_keyboard_and_shows_a_caret() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-focus", &t);
    let carets = |a: &mut StoryApp| {
        let mut best = 0;
        for tick in [0u64, 16] {
            crate::ui::take_carets();
            crate::ui::take_focus_rings();
            let _ = draw(a, &cx, 1280, 720, tick);
            let (c, r) = (crate::ui::take_carets(), crate::ui::take_focus_rings());
            assert!(c <= 1 && r <= 1, "{c} carets and {r} focus frames on the {} page", a.page_name());
            best = best.max(c);
        }
        best
    };
    assert_eq!(a.focus_name(), "idea");
    assert_eq!(carets(&mut a), 1, "the topic field");
    key(&mut a, &cx, Key::Tab);
    assert_eq!(a.focus_name(), "none", "with Options closed there is only the idea box");
    assert_eq!(carets(&mut a), 0, "no field, no caret");
    key(&mut a, &cx, Key::BackTab);
    assert_eq!(a.focus_name(), "idea");
    // with Options open the keyboard still has the idea box and "no field" only
    click(&mut a, &cx, T_OPTIONS_MENU);
    key(&mut a, &cx, Key::Tab);
    assert_eq!(a.focus_name(), "none");
    key(&mut a, &cx, Key::BackTab);
    assert_eq!(a.focus_name(), "idea");
    key(&mut a, &cx, Key::Esc);
    assert_eq!((a.menu, a.focus_name()), (None, "idea"), "Esc closes Options and the keyboard goes back to the idea box");
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.focus_name(), "none", "the next Esc leaves the field");
    assert!(matches!(key(&mut a, &cx, Key::Esc), Flow::Back), "the second leaves the tile");
    // typing goes to the owner only: a letter with no field is a shortcut, not text
    a.go(Page::Story, &cx);
    assert_eq!(a.focus_name(), "none");
    assert_eq!(carets(&mut a), 0);
    a.go(Page::Library, &cx);
    a.handle(&Event::FocusAt(SEARCH_FIELD, 4.0), &cx);
    assert_eq!(carets(&mut a), 1, "the search field");
    a.open_form(&cx);
    a.idea.set("");
    typ(&mut a, &cx, "hi");
    assert_eq!(a.idea.text, "hi");
    key(&mut a, &cx, Key::Esc);
    typ(&mut a, &cx, "xyz");
    assert_eq!(a.idea.text, "hi", "with no field focused, letters are not typed into one");
    // a click on a chip takes the keyboard from the field; an idea chip puts the caret back in the topic
    a.set_focus(Focus::Idea);
    click(&mut a, &cx, T_GENRE);
    assert_eq!(a.focus_name(), "none");
    click(&mut a, &cx, T_SUGGEST + 1);
    assert_eq!(a.focus_name(), "idea");
    // a pasted text goes into the focused field only
    a.handle(&Event::Paste(" pasted".into()), &cx);
    assert!(a.idea.text.ends_with(" pasted") || a.idea.text.contains("pasted"));
}

/// No two click regions overlap (except a row and the buttons on it), and all are inside the window.
fn assert_layout(g: &Gfx, area: R, what: &str) {
    let (w, h) = (g.width() as f32, g.height() as f32);
    for hit in &g.hits {
        let r = hit.rect;
        assert!(r.x >= -0.5 && r.y >= area.y - 0.5 && r.x + r.w <= w + 0.5 && r.y + r.h <= h + 0.5, "{what}: {:?} is outside the window", (&hit.label, r.x, r.y, r.w, r.h));
        // a button cut by the edge of a scrolled area (a picture's button, a block of the conversation) may show only a sliver
        let cut = matches!(hit.label.as_str(), "Draw again" | "Try again" | "toggle");
        assert!(cut || (r.w >= 8.0 && r.h >= 8.0), "{what}: {:?} is too small to hit", (&hit.label, r.w, r.h));
    }
    for (i, a) in g.hits.iter().enumerate() {
        for b in &g.hits[i + 1..] {
            if matches!(a.ev, Event::Row(_)) || matches!(b.ev, Event::Row(_)) {
                continue;
            }
            // buttons sit on the pictures, which sit in the text area
            if a.label == "text area" || b.label == "text area" {
                continue;
            }
            // the clipboard button belongs to the field it sits in
            if (a.label == "field" && b.label == "clipboard") || (a.label == "clipboard" && b.label == "field") {
                continue;
            }
            let (ra, rb) = (a.rect, b.rect);
            let ox = (ra.x + ra.w).min(rb.x + rb.w) - ra.x.max(rb.x);
            let oy = (ra.y + ra.h).min(rb.y + rb.h) - ra.y.max(rb.y);
            assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} and {:?} overlap ({ox:.0} x {oy:.0})", (&a.label, ra.x, ra.y, ra.w, ra.h), (&b.label, rb.x, rb.y, rb.w, rb.h));
        }
    }
}

fn colours(g: &Gfx) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in g.rgba().chunks_exact(4).step_by(37) {
        seen.insert([px[0], px[1], px[2]]);
    }
    seen.len()
}

#[test]
fn every_page_fits_at_the_screen_sizes_with_no_overlap() {
    let _route = iyakki_agent::mixer::pin_command_route(); // these play through the speaker command
    let long = STORY.replace("\n\nShe walked", &"\n\nAnother long paragraph about the river, the reeds and the night wind that went on for quite a while without stopping.".repeat(8));
    let t = Teller::new(&long);
    let (mut a, cx, _) = make("tales-geo", &t);
    let dir = temp_dir("tales-geo-eng");
    let _eng = Engine::install(&cx, &dir, "sh -c \"cat >/dev/null; sleep 30\"");
    a.refresh_tts(&cx);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    assert!(a.pics.len() >= 3, "the long story has pictures: {}", a.pics.len());
    // a second and a third story for the list
    for topic in ["owls", "a robot"] {
        a.open_form(&cx);
        a.idea.set(topic);
        a.sync_fields();
        a.start_create(&cx);
        until(&mut a, &cx, "a story", |a| !a.is_writing());
        until(&mut a, &cx, "its pictures", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    }
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for text_scale in [1.0f32, 1.35] {
            let s = crate::ui::scale_for(w, h) * text_scale;
            let wide = w as f32 >= WIDE_PX;
            // the story with its composer below it; narrow, the composer is a toggle
            for pane_open in if wide { vec![true] } else { vec![false, true] } {
                a.go(Page::Story, &cx);
                a.pane_open = pane_open;
                click(&mut a, &cx, T_PLAY);
                until(&mut a, &cx, "playing", |a| a.reader_status() == Status::Playing);
                for tick in [0u64, 7] {
                    let (g, area) = draw_at(&mut a, &cx, w, h, s, tick);
                    let what = format!("story page at {w}x{h} (text x{text_scale}, composer open {pane_open})");
                    assert_layout(&g, area, &what);
                    assert!(colours(&g) > 20, "{what} is not blank");
                    for needed in ["Pause", "Stop", "Previous part", "Next part", "progress", "text area"] {
                        assert!(g.hits.iter().any(|h| h.label == needed), "{what}: the {needed} control is there");
                    }
                    // the text area stays above the player
                    let text = g.hits.iter().find(|h| h.label == "text area").unwrap().rect;
                    let bar = g.hits.iter().find(|h| h.label == "progress").unwrap().rect;
                    assert!(text.y + text.h <= bar.y, "{what}: text ends above the player");
                    assert!(g.hits.iter().any(|h| h.label == "Save story" || h.label == "Saved ✓"), "{what}: the toolbar has the save button");
                }
                let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
                if pane_open {
                    for needed in ["idea", "Send", "Kids", "Teens", "Adults", "Kind of story", "Tone", "Options ▾"] {
                        assert!(g.hits.iter().any(|h| h.label == *needed), "the {needed} control of the composer is there at {w}x{h} x{text_scale}");
                    }
                    assert!(g.hits.iter().any(|h| h.label.starts_with("Model: ") && h.label.ends_with(" ▾")), "the Model chip at {w}x{h}");
                    assert!(!g.hits.iter().any(|h| h.label.starts_with("Agent: ")), "no separate Agent chip at {w}x{h}");
                    // one group of buttons only: no length, language, tone or picture chips on the panel
                    for gone in ["Medium · 5 min", "Hindi", "Mysterious", "Drawings (SVG)", "Read while it is written", "Save automatically"] {
                        assert!(!g.hits.iter().any(|h| h.label == gone), "{gone} is behind Options at {w}x{h}");
                    }
                    // the composer is under the story (or beside it when wide), in view, and Send is inside the window
                    let idea = g.hits.iter().find(|h| h.label == "idea").unwrap().rect;
                    let send = g.hits.iter().find(|h| h.label == "Send").unwrap().rect;
                    let text = g.hits.iter().find(|h| h.label == "text area").unwrap().rect;
                    assert!((idea.x >= text.x + text.w || idea.y >= text.y + text.h) && idea.y >= area.y && send.y + send.h <= h as f32, "{w}x{h}: idea {idea:?} send {send:?} text {text:?} area {area:?} open {pane_open}");
                    assert!(idea.x + idea.w <= send.x, "Send is beside the box at {w}x{h}");
                    // fonts: the chips use the same sizes as the chat's controls
                    assert!(idea.h >= 38.0 * s * 0.99, "the box is a readable height at {w}x{h}");
                }
                click(&mut a, &cx, T_STOP);
            }
            a.go(Page::Library, &cx);
            for tick in [0u64, 7] {
                let (g, area) = draw_at(&mut a, &cx, w, h, s, tick);
                assert_layout(&g, area, &format!("library at {w}x{h} (text x{text_scale})"));
            }
        }
    }
    // the toggle on a small screen: Hide the idea box / Idea & options
    a.go(Page::Story, &cx);
    a.pane_open = true;
    let (g, _) = draw(&mut a, &cx, 800, 480, 0);
    assert!(g.hits.iter().any(|h| h.label == "Hide the idea box"), "the composer can be hidden");
    a.pane_open = false;
    let (g, _) = draw(&mut a, &cx, 800, 480, 0);
    assert!(g.hits.iter().any(|h| h.label == "Idea & options"), "and shown again");
    assert!(!g.hits.iter().any(|h| h.label == "Send"));
    click(&mut a, &cx, T_OPTIONS);
    assert!(a.pane_open && a.focus_name() == "idea");
    a.stop_reading();
}

#[test]
fn the_options_popover_holds_the_rest_scrolls_and_fits_at_every_size() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-opts", &t);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for text_scale in [1.0f32, 1.35] {
            let s = crate::ui::scale_for(w, h) * text_scale;
            a.go(Page::Story, &cx);
            a.pane_open = true;
            a.menu = None;
            let (g, _) = draw_at(&mut a, &cx, w, h, s, 0);
            click_hit(&mut a, &cx, &g, "Options ▾");
            assert_eq!(a.menu, Some(Menu::Options));
            a.cscroll = 0.0;
            let mut seen: std::collections::HashSet<String> = Default::default();
            for step in 0..30 {
                a.cscroll = step as f32 * 60.0 * s;
                let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
                let panel = g.hits.iter().find(|x| x.label == "options").expect("the popover").rect;
                assert!(panel.x >= area.x && panel.y >= area.y && panel.x + panel.w <= w as f32 && panel.y + panel.h <= h as f32, "popover inside the window at {w}x{h} x{text_scale}: {panel:?}");
                assert!(g.hits.iter().any(|x| x.label == "Close"), "Close is in view");
                for x in &g.hits {
                    seen.insert(x.label.clone());
                }
                if a.cscroll >= a.cmax {
                    break;
                }
            }
            for needed in ["Medium · 5 min", "Language", "Pictures off", "Read while it is written", "Save automatically"] {
                assert!(seen.contains(needed), "{needed} is reachable in Options at {w}x{h} x{text_scale}");
            }
            // what the owner took away is not there: no language chips, no characters box, no picture chips or style chips
            for gone in ["field", "Hindi", "Auto", "Drawings (SVG)", "Image model", "Storybook watercolor", "Comic", "Pixel art", "Night sky"] {
                assert!(!seen.contains(gone), "{gone} is not a control of Options at {w}x{h}");
            }
            assert!(!seen.contains("Picture model: automatic"), "no picture model chip while no image model is set up");
            // Esc closes it
            key(&mut a, &cx, Key::Esc);
            assert_eq!(a.menu, None);
            assert_eq!(a.focus_name(), "idea");
        }
    }
    // the one language control: a drop-down in Options whose list opens over it and returns to Options
    a.go(Page::Story, &cx);
    a.menu = None;
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Options ▾");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Language");
    assert_eq!(a.menu, Some(Menu::Language));
    let (g, area) = draw(&mut a, &cx, 1280, 720, 1);
    for l in ["English", "Tamil", "Hindi", "Japanese"] {
        let r = g.hits.iter().rev().find(|x| x.label == l).unwrap_or_else(|| panic!("{l} in the list")).rect;
        assert!(r.y >= area.y - 1.0 && r.y + r.h <= 721.0, "{l} inside the window: {r:?}");
    }
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.options().language, "Tamil");
    assert_eq!(a.menu, Some(Menu::Options), "the choice closes the list and Options is there again");
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.menu, None);
    assert!(a.explicit[H_LANG]);
    assert_eq!(a.store.load_prefs().options.language, "Tamil", "remembered");
    a.opts.language = "English".into();
    a.explicit[H_LANG] = false;
    // a click outside closes it, a choice inside keeps it open and is remembered and explicit
    a.go(Page::Story, &cx);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Options ▾");
    click(&mut a, &cx, T_LENGTH + 2);
    assert_eq!(a.menu, Some(Menu::Options));
    assert_eq!(a.options().length, Length::Long);
    assert!(a.explicit[H_LEN]);
    click(&mut a, &cx, T_MENU_CLOSE);
    assert_eq!(a.menu, None);
    key(&mut a, &cx, Key::Ctrl('o'));
    assert_eq!(a.menu, Some(Menu::Options), "Ctrl+O opens it");
    key(&mut a, &cx, Key::Ctrl('o'));
    assert_eq!(a.menu, None);
}

#[test]
fn the_cover_is_made_by_the_shell_from_the_title() {
    let mut g = Gfx::new(300, 300);
    g.clear(rgb(9, 11, 20));
    cover(&mut g, R::new(20.0, 20.0, 200.0, 200.0), 1.0, "The Brave Little Fox");
    let a = g.rgba().to_vec();
    let px = |g: &Gfx, x: usize, y: usize| {
        let o = (y * 300 + x) * 4;
        [g.rgba()[o], g.rgba()[o + 1], g.rgba()[o + 2]]
    };
    assert_ne!(px(&g, 40, 40), px(&g, 200, 200), "a gradient, not a flat fill");
    assert_eq!(px(&g, 21, 21), [12, 14, 19], "the corners are rounded off");
    let mut g2 = Gfx::new(300, 300);
    g2.clear(rgb(9, 11, 20));
    cover(&mut g2, R::new(20.0, 20.0, 200.0, 200.0), 1.0, "A Totally Different Tale");
    assert_ne!(a, g2.rgba().to_vec(), "another title, another cover");
    // tiny covers (library rows) show an initial instead of an unreadable title
    let mut g3 = Gfx::new(100, 100);
    cover(&mut g3, R::new(10.0, 10.0, 40.0, 40.0), 0.6, "Zebra");
    assert!(colours(&g3) > 5);
}

#[test]
fn the_tile_is_in_the_media_group_for_roles_that_have_apps_and_the_model_has_no_tools() {
    use iyakki_sec::policy::{App as PApp, Role};
    assert!(Role::Admin.template().apps.contains(&PApp::Tales) && Role::Operator.template().apps.contains(&PApp::Tales) && Role::User.template().apps.contains(&PApp::Tales));
    assert!(!Role::Guest.template().apps.contains(&PApp::Tales));
    assert_eq!(PApp::Tales.label(), "TALES");
    let t = Teller::new(STORY);
    let (a, _, _) = make("tales-meta", &t);
    assert_eq!((a.title(), a.icon(), a.accent()), ("Tales", Icon::Tales, ACCENT));
    assert!(a.help().iter().any(|l| l.contains("Space")) && a.hint().contains("Enter send") && a.hint().contains("Ctrl+P model or agent"));
}

// ------------------------------------------------------------------ the form, pictures and saving

fn press(a: &mut StoryApp, cx: &AppCx, x: f32, y: f32) {
    a.handle(&Event::AreaAt(IDEA_AREA, x, y), cx);
}

#[test]
fn the_idea_box_has_several_lines_grows_selects_and_places_the_caret() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-idea", &t);
    assert_eq!(a.focus_name(), "idea", "ready to type");
    typ(&mut a, &cx, "first line");
    key(&mut a, &cx, Key::Ctrl('j'));
    typ(&mut a, &cx, "second line");
    assert_eq!(a.idea.text, "first line\nsecond line", "Ctrl+J makes a new line (Enter sends)");
    assert_eq!(a.options().topic, a.idea.text, "the options follow the box");
    let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
    key(&mut a, &cx, Key::Up);
    typ(&mut a, &cx, "X");
    assert_eq!(a.idea.text, "first lineX\nsecond line", "Up keeps the column, clipped to the shorter line");
    key(&mut a, &cx, Key::Home);
    typ(&mut a, &cx, ">");
    key(&mut a, &cx, Key::End);
    key(&mut a, &cx, Key::Backspace);
    assert_eq!(a.idea.text, ">first line\nsecond line");
    // select all and replace
    key(&mut a, &cx, Key::Ctrl('a'));
    assert_eq!(a.idea.selected_text().as_deref(), Some(">first line\nsecond line"));
    key(&mut a, &cx, Key::Ctrl('c'));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(">first line\nsecond line"));
    typ(&mut a, &cx, "new");
    assert_eq!(a.idea.text, "new");
    // paste keeps line breaks; the length is capped
    a.handle(&Event::Paste("\nand\r\nmore".into()), &cx);
    assert_eq!(a.idea.text, "new\nand\nmore");
    a.idea.set(&"x".repeat(st::MAX_TOPIC + 500));
    assert_eq!(a.idea.len(), st::MAX_TOPIC);
    assert!(!a.idea.key(&Key::Char('y')), "no room for more");
    // three lines at least, growing with the text (up to ten lines in the tall panel)
    a.idea.set("short");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    let small = g.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
    a.idea.set(&"a longer idea that wraps over a few lines of the box ".repeat(6));
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    let big = g.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
    assert!(big > small, "the box grew: {small} to {big}");
    let (g2, _) = {
        a.idea.set(&"w ".repeat(150));
        draw(&mut a, &cx, 1280, 720, 0)
    };
    let capped = g2.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
    assert!(a.idea.rows() <= 10 && capped <= big * 1.0 + 10.0 * 30.0, "at most ten lines");
    // a press places the caret, a drag selects, a triple click selects all
    a.idea.set("one\ntwo words\nthree");
    let _ = draw(&mut a, &cx, 1280, 720, 0);
    let (pad, lh) = a.idea_geo.unwrap();
    press(&mut a, &cx, pad, pad + lh * 0.5);
    assert_eq!(a.idea.cur, 0);
    press(&mut a, &cx, pad + 5000.0, pad + lh * 1.5);
    assert_eq!(a.idea.cur, "one\ntwo words".chars().count(), "the end of the second line");
    crate::ui::set_dragging(true);
    press(&mut a, &cx, pad + 5000.0, pad + lh * 2.5);
    crate::ui::set_dragging(false);
    assert_eq!(a.idea.selected_text().as_deref(), Some("\nthree"));
    crate::ui::set_click_count(3);
    press(&mut a, &cx, pad + 2.0, pad + 2.0);
    crate::ui::set_click_count(1);
    assert_eq!(a.idea.selected_text().as_deref(), Some("one\ntwo words\nthree"));
    // cut puts it on the clipboard and removes it
    key(&mut a, &cx, Key::Ctrl('x'));
    assert!(a.idea.text.is_empty() && crate::ui::clipboard::latest().as_deref() == Some("one\ntwo words\nthree"));
}

#[test]
fn each_scene_gets_a_drawing_made_in_the_background_and_kept_with_the_story() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-pics", &t);
    write_story(&mut a, &cx, "a fox");
    assert_eq!(a.pics.len(), 2, "three paragraphs, two scenes");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    // the model was asked for each, with no tools, with the style, the safety words for children and the scene
    let calls = t.drawing_calls();
    assert_eq!(calls.len(), 2);
    for (msgs, tools) in &calls {
        assert_eq!(*tools, 0);
        assert!(msgs[0].content.contains(PictureStyle::Watercolor.phrase()) && msgs[0].content.contains("small children") && msgs[0].content.contains("YOU HAVE NO TOOLS"));
        assert!(msgs[1].content.contains("The Brave Little Fox"));
    }
    assert!(calls[0].0[1].content.contains("Once upon a time") && calls[1].0[1].content.contains("She walked home"));
    // each is placed before the paragraph it shows
    assert_eq!(a.pics.iter().map(|p| p.scene.place_before).collect::<Vec<_>>(), vec![1, 2], "the first paragraph, then the other two");
    assert!(a.pics.iter().all(|p| p.ext == "svg" && p.bytes.as_ref().is_some_and(|b| b.starts_with(b"<svg")) && p.prompt.contains("watercolor")));
    // drawn in the page, and the page lays out the same story around them
    let before = a.pic_cache.rasterised;
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(a.pic_cache.rasterised > before, "the picture was drawn");
    assert_layout(&g, area, "story with pictures");
    let (_, _) = draw(&mut a, &cx, 1280, 720, 1);
    assert_eq!(a.pic_cache.rasterised, a.pic_cache.rasterised.max(before + 1));
    // kept in the story folder with the prompt, and the text refers to them
    let id = a.saved_id().unwrap();
    let dir = home.join(format!("Stories/{id}"));
    assert!(dir.join("pics/01.svg").is_file() && dir.join("pics/02.svg").is_file());
    let index = std::fs::read_to_string(dir.join("pics/index.json")).unwrap();
    assert!(index.contains("watercolor") && index.contains("SVG drawing") && index.contains("Once upon a time"), "{index}");
    let md = std::fs::read_to_string(dir.join("story.md")).unwrap();
    assert!(md.contains("![The Brave Little Fox, part 1](pics/01.svg)") && md.contains("![The Brave Little Fox, part 2](pics/02.svg)"), "{md}");
    assert!(md.find("pics/01.svg").unwrap() < md.find("Once upon a time").unwrap());
    assert_eq!(a.store.load(&id).unwrap().entry.pictures, 2);
    // one picture again: only that one changes
    let first = a.pics[0].bytes.clone();
    let second = a.pics[1].bytes.clone();
    *t.scene.lock().unwrap() = SCENE.replace("\"x\":85", "\"x\":15").replace("meadow", "snow");
    click(&mut a, &cx, T_PIC_REDO + 1);
    assert!(matches!(a.pics[1].state, PicState::Queued | PicState::Working));
    until(&mut a, &cx, "the new drawing", |a| a.pics[1].state == PicState::Ready && a.pics[1].bytes != second);
    assert_eq!(a.pics[0].bytes, first, "the other picture is untouched");
    assert_eq!(std::fs::read(dir.join("pics/02.svg")).unwrap(), a.pics[1].bytes.as_ref().unwrap().to_vec(), "and the saved one follows");
    // what was asked and answered is kept beside the story, with the character sheet every picture used
    let log = std::fs::read_to_string(dir.join("pictures.log")).unwrap();
    assert!(log.contains("Picture 1") && log.contains("REQUEST") && log.contains("REPLY") && log.contains("scene composer"), "{log}");
    let sheet = std::fs::read_to_string(dir.join("character.json")).unwrap();
    assert!(sheet.contains("\"gender\"") && a.sheet.is_some(), "{sheet}");
    assert!(a.pics.iter().all(|p| p.how == "cartoon (built-in)"), "the status says what drew it: {:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
    assert_eq!(t.drawing_calls().len(), 3);
}

#[test]
fn a_drawing_that_cannot_be_used_leaves_a_placeholder_and_can_be_tried_again() {
    let t = Teller::new(STORY);
    *t.draw_error.lock().unwrap() = Some("connection refused".into());
    let (mut a, cx, _) = make("tales-badpic", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the failures", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    assert_eq!(t.drawing_calls().len(), 6, "each picture: the scene twice, then the plain drawing");
    let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
    assert!(why.contains("connection refused"), "{why}");
    // the story is all there, the placeholder says why, and the way to try again is one click
    assert!(a.story_text().contains("slept well"));
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert_layout(&g, area, "placeholders");
    // the pictures are in the text, which scrolls: the way to try again is there
    let mut found = false;
    for k in 0..40 {
        a.tscroll = k as f32 * 25.0;
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        found |= g.hits.iter().any(|h| h.label == "Try again");
    }
    assert!(found, "Try again is reachable by scrolling the story");
    a.tscroll = 0.0;
    // a model that cooperates now: one retry
    *t.draw_error.lock().unwrap() = None;
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the retry", |a| a.pics[0].state == PicState::Ready);
    assert!(matches!(a.pics[1].state, PicState::Failed(_)), "the other one still waits for its turn");
    // pictures that are switched off cost nothing
    let t2 = Teller::new(STORY);
    let (mut b, cx2, home) = make("tales-nopic", &t2);
    click(&mut b, &cx2, T_PICMODE);
    assert_eq!(b.options().pictures, PictureMode::Off);
    write_story(&mut b, &cx2, "a fox");
    assert!(b.pics.is_empty() && t2.drawing_calls().is_empty());
    let md = std::fs::read_to_string(home.join(format!("Stories/{}/story.md", b.saved_id().unwrap()))).unwrap();
    assert!(!md.contains("!["));
}

#[test]
fn save_story_is_a_clear_button_with_a_saved_state_and_an_automatic_option() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-save", &t);
    assert!(a.prefs.auto_save, "saving by itself is the default");
    click(&mut a, &cx, T_AUTOSAVE);
    assert!(!a.prefs.auto_save);
    assert!(!a.store.load_prefs().auto_save, "and the choice is remembered");
    write_story(&mut a, &cx, "a fox");
    assert!(a.saved_id().is_none() && a.store.list().is_empty(), "nothing is saved unasked");
    let _ = &home;
    assert!(a.note_text().unwrap().contains("not saved yet"), "{:?}", a.note_text());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Save story") && !g.hits.iter().any(|h| h.label == "Saved ✓"));
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    // the button: text and the pictures that are ready go into Stories/<title>/
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().expect("saved");
    let dir = home.join(format!("Stories/{id}"));
    assert!(dir.join("story.md").is_file() && dir.join("pics/01.svg").is_file(), "text and pictures");
    assert!(a.note_text().unwrap().contains(&format!("Saved in Stories/{id}/")) && a.note_text().unwrap().contains("2 pictures"), "{:?}", a.note_text());
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Saved ✓") && !g.hits.iter().any(|h| h.label == "Save story"), "the state shows");
    assert_layout(&g, area, "saved");
    // it is in Listen again, with its pictures
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(a.library_ids(), vec![id.clone()]);
    assert_eq!(a.lib[0].pictures, 2);
    // switching saving on saves a story that is waiting
    let t2 = Teller::new(STORY);
    let (mut b, cx2, home2) = make("tales-save2", &t2);
    click(&mut b, &cx2, T_AUTOSAVE);
    write_story(&mut b, &cx2, "a fox");
    assert!(b.saved_id().is_none());
    click(&mut b, &cx2, T_AUTOSAVE);
    assert!(b.saved_id().is_some() && home2.join("Stories").is_dir(), "switching it on saves what is waiting");
}

#[test]
fn save_story_keeps_the_audio_too_when_it_has_been_spoken() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-saveaudio", &t);
    let dir = temp_dir("tales-saveaudio-eng");
    let _eng = Engine::install(&cx, &dir, &logging_player_for(&dir.join("p"), "0.02"));
    a.refresh_tts(&cx);
    a.prefs.auto_save = false;
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "all of it spoken", |a| a.reader_status() == Status::Done);
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    assert!(home.join(format!("Stories/{id}/audio.wav")).is_file() && home.join(format!("Stories/{id}/audio.json")).is_file());
    assert!(a.note_text().unwrap().contains("audio"), "{:?}", a.note_text());
    assert!(a.store.load(&id).unwrap().entry.has_audio);
}

#[test]
fn save_pdf_writes_story_pdf_beside_story_md_with_the_text_and_pictures() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-pdf", &t);
    a.prefs.auto_save = false;
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    assert!(a.saved_id().is_none());
    click(&mut a, &cx, T_PDF);
    let id = a.saved_id().expect("the story is saved first, so the PDF has a folder");
    let dir = home.join(format!("Stories/{id}"));
    assert!(dir.join("story.md").is_file(), "the text is beside it");
    let bytes = std::fs::read(dir.join("story.pdf")).expect("story.pdf");
    let d = iyakki_pdf::Document::open(bytes.clone()).unwrap();
    let text: String = (0..d.page_count()).map(|i| d.page_text(i).unwrap()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("The Brave Little Fox") && text.contains("Once upon a time a small fox lived near the river."), "{text}");
    assert_eq!(bytes.windows(15).filter(|w| *w == b"/Subtype /Image").count(), 2, "both illustrations are in the file");
    let note = a.note_text().unwrap();
    assert!(note.contains(&format!("Saved Stories/{id}/story.pdf")) && note.contains("2 pictures") && !note.contains("replaced"), "{note}");
    // the keyboard does the same, and the earlier copy is replaced, said so
    key(&mut a, &cx, Key::Char('d'));
    assert!(a.note_text().unwrap().contains("the earlier copy was replaced"), "{:?}", a.note_text());
    assert!(!dir.join("story.pdf.part").exists());
    // it goes with the story
    a.go(Page::Library, &cx);
    assert_eq!(a.library_ids(), vec![id.clone()]);
    let _ = a.store.delete(&id);
    assert!(!dir.exists());
}

#[test]
fn save_pdf_is_reachable_at_the_four_standard_sizes_by_button_and_keyboard() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-pdf-sizes", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    let mut button_seen = Vec::new();
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for text_scale in [1.0f32, 1.35] {
            let s = crate::ui::scale_for(w, h) * text_scale;
            a.go(Page::Story, &cx);
            a.pane_open = false;
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
            let what = format!("story page at {w}x{h} (text x{text_scale})");
            assert_layout(&g, area, &what);
            if let Some(btn) = g.hits.iter().find(|h| h.label == "Save PDF") {
                button_seen.push((w, h, text_scale));
                assert!(btn.rect.x >= area.x && btn.rect.x + btn.rect.w <= area.x + area.w + 1.0, "{what}: the button is inside the tile");
                // pressing the drawn button writes the file
                a.handle(&Event::Tab(T_PDF), &cx);
                assert!(a.note_text().unwrap().contains("story.pdf"), "{what}: {:?}", a.note_text());
            } else {
                // the toolbar drops what does not fit; the key always works and the help line names it
                key(&mut a, &cx, Key::Char('d'));
                assert!(a.note_text().unwrap().contains("story.pdf"), "{what}: {:?}", a.note_text());
            }
            let (g, _) = draw_at(&mut a, &cx, w, h, s, 1);
            assert!(g.hits.iter().any(|h| h.label == "Saved ✓" || h.label == "Save story"), "{what}: still drawn after the export");
        }
    }
    assert!(button_seen.iter().any(|(w, _, _)| *w >= 1280), "the button is on the toolbar on the wide sizes: {button_seen:?}");
    let id = a.saved_id().unwrap();
    assert!(home.join(format!("Stories/{id}/story.pdf")).is_file());
}

#[test]
fn the_conversation_can_ask_for_the_pdf_and_it_is_a_step_and_not_a_correction() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-pdf-chat", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    let asked_before = t.seen.lock().unwrap().len();
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "please make it a PDF");
    key(&mut a, &cx, Key::Ctrl('n'));
    assert_eq!(t.seen.lock().unwrap().len(), asked_before, "the model was not asked to rewrite the story");
    let id = a.saved_id().expect("saved");
    assert!(home.join(format!("Stories/{id}/story.pdf")).is_file());
    assert!(a.convo.iter().any(|m| m.tool_calls.iter().any(|c| c.name == "PDF")), "a PDF step is on the timeline");
    assert!(a.convo.iter().any(|m| m.content.contains("Saved Stories/")), "and its result");
    assert!(a.idea.text.is_empty());
    // a message that only mentions a pdf is a correction as before
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "the fox reads a pdf in the library");
    key(&mut a, &cx, Key::Ctrl('n'));
    until(&mut a, &cx, "the revision", |a| !a.is_writing());
    assert!(t.seen.lock().unwrap().len() > asked_before, "this one went to the model");
}

#[test]
fn a_story_in_another_script_is_refused_in_plain_words_and_no_file_is_made() {
    let t = Teller::new("# \u{4e2d}\u{6587}\n\n\u{4e2d}\u{6587} \u{4e2d}\u{6587} \u{4e2d}\u{6587}.\n");
    let (mut a, cx, home) = make("tales-pdf-tamil", &t);
    a.prefs.auto_save = false;
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PDF);
    let note = a.note_text().unwrap();
    assert!(note.contains("cannot be written in a PDF yet") && note.contains("Nothing was saved"), "{note}");
    let id = a.saved_id().unwrap();
    assert!(!home.join(format!("Stories/{id}/story.pdf")).exists());
}

#[test]
fn export_makes_one_html_page_with_the_pictures_inside_or_the_text() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-export", &t);
    a.prefs.auto_save = false;
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    // an unsaved story can be exported too: the page is made from what is on screen
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Export…"));
    click(&mut a, &cx, T_EXPORT);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Page with pictures")), "the menu is up");
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().expect("handed to the host-files save dialog");
    assert_eq!(name, "the-brave-little-fox.html");
    let html = String::from_utf8(bytes).unwrap();
    assert!(html.starts_with("<!doctype html>") && html.contains("<h1>The Brave Little Fox</h1>"));
    assert_eq!(html.matches("data:image/svg+xml;base64,").count(), 2, "both pictures are inside the file");
    assert!(!html.contains("<script") && !html.contains("src=\"http") && !html.contains("<link"), "self-contained, no script");
    assert!(html.find("<img").unwrap() < html.find("Once upon a time").unwrap());
    // Esc closes the menu without exporting
    click(&mut a, &cx, T_EXPORT);
    key(&mut a, &cx, Key::Esc);
    assert!(a.take_host_save().is_none() && a.modal.is_none());
    // a saved story from Listen again exports the same way, from what is on disk
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    a.go(Page::Library, &cx);
    click(&mut a, &cx, T_LIB_EXPORT);
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().unwrap();
    assert_eq!(name, format!("{id}.html"));
    let html = String::from_utf8(bytes).unwrap();
    assert_eq!(html.matches("data:image/svg+xml;base64,").count(), 2);
    // the text
    click(&mut a, &cx, T_LIB_EXPORT);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    let (name, bytes) = a.take_host_save().unwrap();
    assert_eq!(name, format!("{id}.md"));
    assert!(String::from_utf8(bytes).unwrap().contains("![The Brave Little Fox, part 1](pics/01.svg)"), "the Markdown refers to the pictures");
}

#[test]
fn a_saved_story_opens_with_its_pictures_where_they_were() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-reopen", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    let id = a.saved_id().unwrap();
    let places: Vec<usize> = a.pics.iter().map(|p| p.scene.place_before).collect();
    a.go(Page::Library, &cx);
    click(&mut a, &cx, T_LIB_OPEN);
    assert_eq!(a.page_name(), "story");
    assert_eq!(a.saved_id().as_deref(), Some(id.as_str()));
    assert_eq!(a.pics.len(), 2);
    assert!(a.pics.iter().all(|p| p.state == PicState::Ready));
    assert_eq!(a.pics.iter().map(|p| p.scene.place_before).collect::<Vec<_>>(), places, "in the same places");
    assert!(a.pics[0].prompt.contains("watercolor"));
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert_layout(&g, area, "a saved story with pictures");
    assert!(a.pic_cache.rasterised >= 1);
}

// ------------------------------------------------------------------ the model

fn add_providers(cx: &AppCx, models: &[&str]) {
    let mut a = iyakki_shared::providers::ProviderCfg::new("home", iyakki_shared::providers::Kind::Ollama);
    a.models = models.iter().map(|m| m.to_string()).collect();
    let mut b = iyakki_shared::providers::ProviderCfg::new("off", iyakki_shared::providers::Kind::Ollama);
    b.models = vec!["hidden".into()];
    b.enabled = false;
    let mut st = cx.settings().lock().unwrap();
    st.upsert_provider(a).unwrap();
    st.upsert_provider(b).unwrap();
}

/// Draw the form scrolled step by step until a hit whose label starts with `prefix` shows; its label.
fn find_in_form(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, prefix: &str) -> Option<String> {
    a.pane_open = true;
    a.menu = Some(Menu::Options);
    for step in 0..30 {
        a.cscroll = step as f32 * 80.0;
        let (g, _) = draw(a, cx, w, h, 1);
        if let Some(x) = g.hits.iter().find(|x| x.label.starts_with(prefix)) {
            return Some(x.label.clone());
        }
    }
    None
}

/// Make the tile use `other` for any chosen model and remember which (provider, model) pairs were asked for.
fn record_choices(a: &mut StoryApp, other: &Arc<Teller>) -> Arc<Mutex<Vec<(String, String)>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (log, other) = (asked.clone(), other.clone());
    a.test_backend = Some(Arc::new(move |c: &ModelChoice| {
        log.lock().unwrap().push((c.provider.clone(), c.model.clone()));
        Arc::new(Shared(other.clone())) as Arc<dyn ChatBackend>
    }));
    asked
}

#[test]
fn the_model_chosen_in_tales_writes_the_story_without_tools_and_is_remembered_and_chat_is_unchanged() {
    let chat_model = Teller::new("# Wrong\n\nThe chat model must not be asked.");
    let tales_model = Teller::new(STORY);
    let (mut a, mut cx, home) = make("tales-model", &chat_model);
    cx.session.policy.manage_settings = false; // not an administrator: may still choose here
    add_providers(&cx, &["llama3", "qwen3"]);
    cx.settings().lock().unwrap().set_chat(Some(ModelChoice { provider: "home".into(), model: "llama3".into() })).unwrap();
    let asked = record_choices(&mut a, &tales_model);
    // by default: same as chat, and the chip says so
    assert_eq!(a.model_label(&cx).0, "same as chat (llama3 · home)");
    // Ctrl+P: the default line, then the models of the enabled providers only
    key(&mut a, &cx, Key::Ctrl('p'));
    let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
    assert_eq!(m.list.items[1..3], [("home".to_string(), "llama3".to_string()), ("home".to_string(), "qwen3".to_string())], "the same list as the chat's, behind the first line");
    assert_eq!(m.list.rows[0], crate::apps::model_picker::MRow::Top);
    assert_eq!(m.list.sel, 0, "same as chat is highlighted");
    key(&mut a, &cx, Key::Down); // the provider's heading
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_none());
    assert_eq!(a.prefs.model, Some(ModelChoice { provider: "home".into(), model: "qwen3".into() }));
    assert_eq!(a.store.load_prefs().model, a.prefs.model, "remembered for the next visit");
    let chat = cx.settings().lock().unwrap().get().chat.clone().unwrap();
    assert_eq!(chat.model, "llama3", "the chat model was not changed by a non-administrator");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("story.model") && l.contains("qwen3")));
    // the story is written by the chosen model, with no tools, and the page says which
    write_story(&mut a, &cx, "a fox");
    assert_eq!(asked.lock().unwrap().first(), Some(&("home".to_string(), "qwen3".to_string())));
    assert_eq!(chat_model.story_calls().len(), 0, "the chat model was not asked");
    let calls = tales_model.story_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, 0, "still no tools");
    assert_eq!(a.run_model, "qwen3 · home");
    let (_tx, rx) = channel();
    a.gen = Some(Gen { rx, stop: Arc::new(AtomicBool::new(false)) });
    assert_eq!(a.status_line(&a.reader.snapshot()).0, "Writing the story with qwen3 · home…");
    a.gen = None;
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("story.create") && l.contains("model home qwen3")));
    // a new visit finds the choice again
    let mut b = StoryApp::new(&cx);
    b.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
    b.prefs = b.store.load_prefs();
    assert_eq!(b.model_label(&cx).0, "qwen3 · home");
    // going back to "same as chat" is one more choice
    key(&mut a, &cx, Key::Ctrl('p'));
    key(&mut a, &cx, Key::Home);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.prefs.model, None);
    assert_eq!(a.store.load_prefs().model, None);
}

#[test]
fn a_choice_whose_provider_was_removed_falls_back_to_the_chat_model_with_a_note_and_works_again_when_it_returns() {
    let chat_model = Teller::new(STORY);
    let tales_model = Teller::new("# Wrong\n\nNot this one.");
    let (mut a, cx, _) = make("tales-gone", &chat_model);
    add_providers(&cx, &["llama3", "qwen3"]);
    let mut base = iyakki_shared::providers::ProviderCfg::new("base", iyakki_shared::providers::Kind::Ollama);
    base.models = vec!["small".into()];
    cx.settings().lock().unwrap().upsert_provider(base).unwrap();
    cx.settings().lock().unwrap().set_chat(Some(ModelChoice { provider: "base".into(), model: "small".into() })).unwrap();
    let asked = record_choices(&mut a, &tales_model);
    a.prefs.model = Some(ModelChoice { provider: "home".into(), model: "qwen3".into() });
    cx.settings().lock().unwrap().remove_provider("home").unwrap_or(());
    let (label, note) = a.model_label(&cx);
    assert!(label.starts_with("same as chat"), "{label}");
    assert!(note.as_deref().is_some_and(|n| n.contains("qwen3") && n.contains("not available")), "{note:?}");
    // the page shows the note, and the story is written by the chat model
    assert_eq!(a.writer_line(&cx), "model: same as chat (small · base)");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Model: same as chat ▾"));
    write_story(&mut a, &cx, "a fox");
    assert!(asked.lock().unwrap().is_empty(), "no backend was made for the missing model");
    assert_eq!(chat_model.story_calls().len(), 1);
    assert_eq!(a.prefs.model.as_ref().map(|c| c.model.as_str()), Some("qwen3"), "the choice is kept, not erased");
    // the provider comes back: the choice is used again
    add_providers(&cx, &["llama3", "qwen3"]);
    assert_eq!(a.model_label(&cx), ("qwen3 · home".to_string(), None));
}

#[test]
fn the_picture_model_is_chosen_separately_and_only_offered_for_a_picture_model() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-picmodel", &t);
    // with no picture model anywhere there is nothing to choose: no chip in Options
    a.opts.pictures = PictureMode::Auto;
    a.image_models.clear();
    assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model"), None, "not offered when images are not available");
    // with one it is offered, unless the pictures are off
    a.image_models = vec![("home".into(), "flux".into())];
    assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model").as_deref(), Some("Picture model: automatic"));
    a.opts.pictures = PictureMode::Off;
    assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model"), None, "pictures off: no picture model to choose");
    a.opts.pictures = PictureMode::Auto;
    // with no picture model anywhere the chooser says so and stays closed
    a.image_models.clear();
    click(&mut a, &cx, T_PICMODEL);
    assert!(a.modal.is_none() || matches!(a.modal, Some(Modal::Model(_))));
    // with one, it is chosen, remembered and handed to the picture request
    a.modal = None;
    a.image_models = vec![("home".into(), "flux".into()), ("home".into(), "sdxl".into())];
    add_providers(&cx, &["flux", "sdxl"]);
    click(&mut a, &cx, T_PICMODEL);
    {
        let Some(Modal::Model(m)) = &a.modal else { panic!("the picture chooser is open") };
        assert!(m.picture && m.list.title == "Choose the picture model", "{}", m.list.title);
        assert_eq!(m.list.items.len(), 3, "Automatic and the two picture models; no agents");
        assert_eq!(m.list.sel, 0);
    }
    key(&mut a, &cx, Key::Down); // the provider's heading
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.prefs.picture_model, Some(ModelChoice { provider: "home".into(), model: "sdxl".into() }));
    assert_eq!(a.store.load_prefs().picture_model, a.prefs.picture_model);
    assert_eq!(a.picture_plan(&cx), ModelPlan::Chosen(ModelChoice { provider: "home".into(), model: "sdxl".into() }));
    assert_eq!(a.picture_label(&cx).0, "sdxl · home");
}

#[test]
fn the_model_chooser_fits_and_works_by_click_and_key_at_every_screen_size() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-pickgeo", &t);
    let names: Vec<String> = (0..40).map(|i| format!("model-{i:02}-with-a-fairly-long-name:latest")).collect();
    add_providers(&cx, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let _asked = record_choices(&mut a, &t);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for text_scale in [1.0f32, 1.35] {
            let s = crate::ui::scale_for(w, h) * text_scale;
            a.go(Page::Story, &cx);
            a.pane_open = true;
            // the composer with the Model chip: Send is always in view
            let (g, _) = draw_at(&mut a, &cx, w, h, s, 1);
            assert!(g.hits.iter().any(|x| x.label == "Send"), "Send in view at {w}x{h} x{text_scale}");
            assert!(g.hits.iter().any(|x| x.label.starts_with("Model: ")), "the Model chip at {w}x{h} x{text_scale}");
            // the chooser: opened by Ctrl+P, every visible line inside the screen, the highlighted line in view
            a.cscroll = 0.0;
            a.focus = Focus::None;
            key(&mut a, &cx, Key::Ctrl('p'));
            assert!(matches!(a.modal, Some(Modal::Model(_))), "Ctrl+P opens at {w}x{h}");
            for step in [Key::Down, Key::PageDown, Key::End, Key::PageUp, Key::Home, Key::Down] {
                key(&mut a, &cx, step);
                let (g, area) = draw_at(&mut a, &cx, w, h, s, 2);
                assert_layout(&g, area, &format!("chooser at {w}x{h} x{text_scale}"));
                let rows: Vec<_> = g.hits.iter().filter(|x| x.label == "model").collect();
                assert!(rows.len() >= 3, "{} lines at {w}x{h}", rows.len());
                let Some(Modal::Model(m)) = &a.modal else { panic!() };
                assert!(rows.iter().any(|r| matches!(r.ev, Event::Row(n) if n == m.list.sel)), "the highlighted line is in view at {w}x{h} (sel {})", m.list.sel);
                assert!(!g.hits.iter().any(|x| x.label == "Send"), "the chooser covers the composer");
                for r in &rows {
                    assert!(r.rect.y >= area.y && r.rect.y + r.rect.h <= area.y + area.h, "line inside the window at {w}x{h}: {:?}", r.rect);
                }
            }
            // a click on a line chooses it
            let (g, _) = draw_at(&mut a, &cx, w, h, s, 3);
            let target = g.hits.iter().filter(|x| x.label == "model").filter_map(|x| if let Event::Row(n) = x.ev { Some(n) } else { None }).last().unwrap();
            a.handle(&Event::Row(target), &cx);
            assert!(a.modal.is_none());
            assert_eq!(a.prefs.model.as_ref().map(|c| c.model.clone()), Some(names[target - 1].clone()), "line {target} at {w}x{h}");
            // Esc closes without changing; the chip's click opens it too
            click(&mut a, &cx, T_MODEL);
            assert!(matches!(a.modal, Some(Modal::Model(_))));
            key(&mut a, &cx, Key::Home);
            key(&mut a, &cx, Key::Esc);
            assert!(a.modal.is_none());
            assert_eq!(a.prefs.model.as_ref().map(|c| c.model.clone()), Some(names[target - 1].clone()), "Esc changes nothing");
            a.prefs.model = None;
        }
    }
}

/// The row of `(agent, model)` / provider model in the open chooser.
fn pick_row(a: &StoryApp, f: impl Fn(&crate::apps::model_picker::MRow, &(String, String)) -> bool) -> usize {
    let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
    m.list.rows.iter().zip(&m.list.items).position(|(r, i)| f(r, i)).expect("the row is in the list")
}

#[test]
fn one_chooser_picks_a_provider_model_an_agent_model_a_custom_id_or_same_as_chat_and_replaces_the_other() {
    use crate::apps::model_picker::MRow;
    let t = Teller::new(STORY);
    let (mut a, mut cx, _) = make("tales-onechoice", &t);
    cx.session.policy.manage_settings = true;
    add_providers(&cx, &["qwen3:8b", "llama3"]);
    let dir = temp_dir("tales-onechoice-bin");
    set_agents(&cx, Some(&fake_claude(&dir, false)), None);
    assert_eq!(a.chip_text(&cx), "Model: same as chat");
    // a provider's model, found by its row; the chip says model and provider
    key(&mut a, &cx, Key::Ctrl('p'));
    let r = pick_row(&a, |_, i| i.1 == "qwen3:8b");
    a.handle(&Event::Row(r), &cx);
    assert!(a.modal.is_none());
    assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
    assert_eq!(a.choice(&cx), Choice::Provider(ModelChoice { provider: "home".into(), model: "qwen3:8b".into() }));
    // an agent's model: the agent replaces the provider's model as the one choice (the model is kept as the fallback)
    key(&mut a, &cx, Key::Ctrl('p'));
    let r = pick_row(&a, |r, _| matches!(r, MRow::Model(ag, mi) if ag == "claude-code" && mi.id == "sonnet"));
    a.handle(&Event::Row(r), &cx);
    assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
    assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()));
    // the list opens on it, and the agent's header keeps the model
    key(&mut a, &cx, Key::Ctrl('p'));
    let Some(Modal::Model(m)) = &a.modal else { panic!() };
    assert_eq!(m.list.sel, r);
    let head = pick_row(&a, |r, _| matches!(r, MRow::Head(id) if id == "claude-code"));
    a.handle(&Event::Row(head), &cx);
    assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()), "the header keeps the model");
    // a custom id typed in the list is refused with shell characters, then used and remembered for the agent
    key(&mut a, &cx, Key::Ctrl('g'));
    let c = pick_row(&a, |r, _| matches!(r, MRow::Custom(id) if id == "claude-code"));
    a.handle(&Event::Row(c), &cx);
    for ch in "x; ls".chars() {
        key(&mut a, &cx, Key::Char(ch));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(a.modal, Some(Modal::Model(_))) && a.note_text().is_some_and(|n| n.contains("letters, digits")), "{:?}", a.note_text());
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    for ch in "claude-opus-5-5".chars() {
        key(&mut a, &cx, Key::Char(ch));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_none());
    assert_eq!(a.prefs.agent_model, "claude-opus-5-5");
    assert_eq!(a.chip_text(&cx), "Model: claude-opus-5-5 · Claude Code");
    assert_eq!(cx.settings().lock().unwrap().get().agents.claude.custom_models, ["claude-opus-5-5"]);
    assert_eq!(a.store.load_prefs().agent_model, "claude-opus-5-5", "remembered");
    // the story keeps its no-tools rule and the page says who writes
    assert!(a.writer_line(&cx).contains("no tools"), "{}", a.writer_line(&cx));
    // the first line, "Same as chat", ends both
    key(&mut a, &cx, Key::Ctrl('p'));
    key(&mut a, &cx, Key::Home);
    key(&mut a, &cx, Key::Enter);
    assert_eq!((a.prefs.agent.clone(), a.prefs.model.clone(), a.prefs.agent_model.clone()), (None, None, String::new()));
    assert_eq!(a.chip_text(&cx), "Model: same as chat");
    // a provider's model also ends an agent choice
    a.prefs.agent = Some("claude-code".into());
    key(&mut a, &cx, Key::Ctrl('p'));
    let r = pick_row(&a, |_, i| i.1 == "llama3");
    a.handle(&Event::Row(r), &cx);
    assert_eq!((a.prefs.agent.clone(), a.chip_text(&cx)), (None, "Model: llama3 · home".to_string()));
}

#[test]
fn old_preferences_with_a_model_and_an_agent_become_one_choice() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-migrate", &t);
    add_providers(&cx, &["qwen3:8b"]);
    let old = r#"{"model":{"provider":"home","model":"qwen3:8b"},"agent":"claude-code","agent_model":"sonnet"}"#;
    a.prefs = serde_json::from_str(old).unwrap();
    assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()), "the agent answered before, so it is the choice");
    assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
    let old = r#"{"model":{"provider":"home","model":"qwen3:8b"}}"#;
    a.prefs = serde_json::from_str(old).unwrap();
    assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
    a.prefs = serde_json::from_str("{}").unwrap();
    assert_eq!(a.chip_text(&cx), "Model: same as chat");
    // an agent that is not configured any more falls to the model that was chosen too
    a.prefs = serde_json::from_str(r#"{"model":{"provider":"home","model":"qwen3:8b"},"agent":"custom:gone"}"#).unwrap();
    assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
}

// ------------------------------------------------------------------ browser voice

/// A page that answers like a browser: the voices at once, then each piece starts, lasts a moment and ends.
/// While `held` is true a piece that has been started does not end (however long it has talked): the test decides when the page
/// says it is done, not a wall-clock time that a loaded machine can overrun between two steps of the test.
fn browser_page(hv: &Arc<HostVoice>, talk_ms: u64, held: Arc<AtomicBool>) -> (Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
    use iyakki_shared::hostvoice::{FromClient, Front, ToClient, VoiceInfo};
    hv.connect(Front::Browser);
    let (said, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
    let (h, sd, st) = (hv.clone(), said.clone(), stop.clone());
    std::thread::spawn(move || {
        let mut cur: Option<(u64, std::time::Instant)> = None;
        while !st.load(Ordering::Relaxed) {
            for m in h.take_outgoing() {
                match m {
                    ToClient::ListVoices => h.event(FromClient::Voices {
                        supported: true,
                        voices: vec![VoiceInfo { name: "Lekha".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "Samantha".into(), lang: "en-US".into(), local: true }, VoiceInfo { name: "Daniel".into(), lang: "en-GB".into(), local: true }],
                    }),
                    ToClient::Speak { id, text, .. } => {
                        sd.lock().unwrap().push(text);
                        h.event(FromClient::Started { id });
                        cur = Some((id, std::time::Instant::now()));
                    }
                    ToClient::Cancel { id } => {
                        // as the real page: only the utterance named (or whatever is said when none is named)
                        if id.is_none() || cur.is_some_and(|c| Some(c.0) == id) {
                            cur = None;
                        }
                    }
                    _ => {}
                }
            }
            if let Some((id, t0)) = cur {
                if t0.elapsed() >= Duration::from_millis(talk_ms) && !held.load(Ordering::SeqCst) {
                    h.event(FromClient::Ended { id });
                    cur = None;
                }
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    });
    (said, stop)
}

fn hit_ev(g: &Gfx, label: &str) -> Event {
    g.hits.iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no control named {label}")).ev.clone()
}

#[test]
fn with_a_browser_connected_and_nothing_else_set_up_the_story_is_read_by_the_browser_at_every_screen_size() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-bv", &t);
    let hv = HostVoice::current();
    let held = Arc::new(AtomicBool::new(false));
    let (said, stop) = browser_page(&hv, 1200, held.clone());
    until(&mut a, &cx, "the page's answer", |_| hv.is_ready());
    a.refresh_tts(&cx);
    assert!(a.browser_voice && matches!(a.tts, TtsStatus::Ready { .. }), "{:?}", a.tts);
    assert!(st::status_text(&a.tts).contains("browser voice"));
    assert_eq!(a.voices, vec!["".to_string(), "Samantha".into(), "Daniel".into()], "automatic first, then the voices of the story's language (English)");
    assert!(a.takes_speed);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        a.go(Page::Story, &cx);
        a.pane_open = false;
        let s = crate::ui::scale_for(w, h);
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
        assert_layout(&g, area, &format!("browser voice story page at {w}x{h}"));
        for needed in ["Play", "Save audio"] {
            assert!(g.hits.iter().any(|x| x.label == needed), "{needed} at {w}x{h}");
        }
        // Save audio explains itself instead of making a file
        a.note = None;
        a.handle(&hit_ev(&g, "Save audio"), &cx);
        assert!(a.audio_job.is_none(), "no audio job");
        assert!(a.note_text().unwrap().contains("Save audio is not available with the browser voice"), "{:?}", a.note_text());
        // Play by its button: the pieces reach the page in order and the player follows
        said.lock().unwrap().clear();
        a.reader.stop();
        a.live = false;
        held.store(true, Ordering::SeqCst);
        a.handle(&hit_ev(&g, "Play"), &cx);
        until(&mut a, &cx, "reading", |a| a.reader_status() == Status::Playing);
        let (g2, area) = draw_at(&mut a, &cx, w, h, s, 3);
        assert_layout(&g2, area, &format!("browser voice player at {w}x{h}"));
        for needed in ["Pause", "Stop", "progress"] {
            assert!(g2.hits.iter().any(|x| x.label == needed), "{needed} while reading at {w}x{h}: {:?} status {:?}", g2.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>(), a.reader_status());
        }
        let first = a.chunks[0].text.clone();
        assert_eq!(said.lock().unwrap().first(), Some(&first), "the first piece goes to the page first");
        a.handle(&hit_ev(&g2, "Pause"), &cx);
        assert_eq!(a.reader_status(), Status::Paused);
        a.handle(&Event::Tab(T_PLAY), &cx);
        held.store(false, Ordering::SeqCst);
        // jump to the last piece with the progress bar's own path; it is said and the reading ends
        let last = a.chunks.len() - 1;
        a.seek(last, &cx);
        until(&mut a, &cx, "done", |a| a.reader_status() == Status::Done);
        assert_eq!(said.lock().unwrap().last(), Some(&a.chunks[last].text));
    }
    // the voice chip cycles through automatic and the browser's voices, and the choice is what is sent
    a.go(Page::Story, &cx);
    assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Voice: ").as_deref(), Some("Voice: automatic"));
    a.handle(&Event::Tab(T_VOICE), &cx);
    assert_eq!(a.prefs.voice.as_deref(), Some("Samantha"));
    a.reader.stop();
    said.lock().unwrap().clear();
    a.start_reading(0, &cx);
    until(&mut a, &cx, "a piece said", |_| !said.lock().unwrap().is_empty());
    a.reader.stop();
    stop.store(true, Ordering::Relaxed);
}

#[test]
fn on_the_console_the_story_says_how_to_get_the_browser_voice_and_an_explicit_choice_explains_itself() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-bv-console", &t);
    a.refresh_tts(&cx);
    assert_eq!(a.tts, TtsStatus::NoEngine);
    assert!(!a.browser_voice);
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PLAY);
    let note = a.note_text().unwrap().to_string();
    assert!(note.contains("browser voice") && note.contains("iyakki web"), "{note}");
    assert!(note.find("browser voice").unwrap() < note.find("Settings → Voice").unwrap(), "the browser voice is offered first: {note}");
    // chosen in Settings, on a screen that cannot speak
    cx.settings().lock().unwrap().update(|s| s.voice.tts = Some(SpeechEngine::Browser)).unwrap();
    a.refresh_tts(&cx);
    assert!(matches!(a.tts, TtsStatus::BrowserUnavailable(_)), "{:?}", a.tts);
    click(&mut a, &cx, T_PLAY);
    assert!(a.note_text().unwrap().contains("Browser voice is chosen") && a.note_text().unwrap().contains("console"), "{:?}", a.note_text());
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        a.pane_open = true;
        let s = crate::ui::scale_for(w, h);
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
        assert_layout(&g, area, &format!("unavailable browser voice at {w}x{h}"));
        assert!(g.hits.iter().any(|x| x.label == "Send"));
    }
    // the voice permission still rules
    let mut cx2 = cx;
    cx2.session.policy.voice = false;
    a.refresh_tts(&cx2);
    assert_eq!(a.tts, TtsStatus::NotAllowed);
}

// ------------------------------------------------------------------ the composer: audience, kind, tone, suggestions

#[test]
fn who_is_it_for_is_the_only_button_group_and_is_remembered() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-aud", &t);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Teens");
    assert_eq!(a.options().audience, Audience::Teen);
    assert_eq!(a.store.load_prefs().options.audience, Audience::Teen, "remembered at once");
    key(&mut a, &cx, Key::Esc);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
    click_hit(&mut a, &cx, &g, "Adults");
    write_story(&mut a, &cx, "a fox");
    let sys = &t.story_calls()[0].0[0].content;
    assert!(sys.contains("SAFETY (adults)"), "the audience reached the prompt");
    // a new visit finds it again
    let mut b = StoryApp::new(&cx);
    b.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), a.store.dir().parent().unwrap().to_path_buf()).unwrap();
    b.prefs = b.store.load_prefs();
    assert_eq!(b.prefs.options.audience, Audience::Adult);
    // the other groups are not on the panel
    let (g, _) = draw(&mut a, &cx, 1280, 720, 2);
    let panel_buttons: Vec<&str> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_GENRE..T_GENRE + 6).contains(&n) || (T_LENGTH..T_LENGTH + 3).contains(&n) || (T_LANG..T_LANG + 14).contains(&n) || (T_TONE..T_TONE + 5).contains(&n) || n == T_PICMODE || n == T_READ_WHILE || n == T_AUTOSAVE)).map(|h| h.label.as_str()).collect();
    assert!(panel_buttons.is_empty(), "{panel_buttons:?}");
}

#[test]
fn kind_and_tone_are_drop_downs_that_open_a_list_by_click_and_by_key() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-drop", &t);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = crate::ui::scale_for(w, h);
        a.menu = None;
        let (g, _) = draw_at(&mut a, &cx, w, h, s, 0);
        assert!(g.hits.iter().any(|x| x.label == "Kind of story") && g.hits.iter().any(|x| x.label == "Tone"));
        click_hit(&mut a, &cx, &g, "Kind of story");
        assert_eq!(a.menu, Some(Menu::Kind));
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
        for gname in ["Bedtime", "Adventure", "Fantasy", "Sci-fi", "Moral", "Funny"] {
            let r = g.hits.iter().rev().find(|x| x.label == gname).unwrap_or_else(|| panic!("{gname} at {w}x{h}")).rect;
            assert!(r.y >= area.y - 1.0 && r.y + r.h <= h as f32 + 1.0 && r.x >= 0.0 && r.x + r.w <= w as f32 + 1.0, "{gname} inside the window at {w}x{h}: {r:?}");
        }
        click_hit(&mut a, &cx, &g, "Sci-fi");
        assert_eq!((a.options().genre, a.menu, a.focus_name()), (Genre::SciFi, None, "idea"));
        a.opts.genre = Genre::Bedtime;
    }
    // by key: open, Down, Enter; Esc leaves it as it was
    a.explicit = [false; 4];
    click(&mut a, &cx, T_TONE_MENU);
    assert_eq!(a.menu, Some(Menu::Tone));
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert_eq!((a.options().tone, a.menu), (Tone::ALL[2], None));
    assert!(a.explicit[H_TONE]);
    assert_eq!(a.store.load_prefs().options.tone, Tone::ALL[2], "remembered");
    click(&mut a, &cx, T_KIND_MENU);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Esc);
    assert_eq!((a.options().genre, a.menu), (Genre::Bedtime, None));
    // the list shows what the idea text suggested while nothing was chosen
    a.explicit = [false; 4];
    a.opts.tone = Tone::Calm;
    a.idea.set("an exciting story about a kite");
    a.sync_fields();
    assert_eq!(a.shown_options().tone, Tone::Exciting);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|x| x.label == "Exciting ×" || x.label == "Tone: Exciting ×"), "the suggestion is a chip under the box");
}

#[test]
fn the_idea_text_suggests_length_language_kind_and_tone_as_chips_and_what_is_chosen_wins() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-hints", &t);
    a.idea.set("a 5-minute funny story in Tamil about a fox, in comic style");
    a.sync_fields();
    let names: Vec<_> = a.hint_chips().iter().map(|c| (c.name, c.value.clone(), c.applied)).collect();
    assert_eq!(names, vec![("Length", "Medium · 5 min".to_string(), true), ("Language", "Tamil".to_string(), true), ("Kind", "Funny".to_string(), true), ("Picture style", "Comic".to_string(), true)]);
    // the options themselves are untouched; the chips show what was understood
    assert_eq!((a.options().length, a.options().language.as_str(), a.options().genre), (Length::Short, "English", Genre::Bedtime));
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert_layout(&g, area, "composer with chips");
    for label in ["Length: Medium · 5 min ×", "Language: Tamil ×", "Kind: Funny ×", "Picture style: Comic ×", "Pictures: cartoon (built-in)"] {
        assert!(g.hits.iter().any(|x| x.label == label), "{label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    }
    // a chip is removable: that suggestion is not used
    click_hit(&mut a, &cx, &g, "Language: Tamil ×");
    assert_eq!(a.shown_options().language, "English");
    assert!(a.hint_chips().iter().all(|c| c.name != "Language"));
    assert_eq!(a.focus_name(), "idea");
    // an explicit choice always wins, and the chip says so
    click(&mut a, &cx, T_GENRE);
    let chips = a.hint_chips();
    let kind = chips.iter().find(|c| c.name == "Kind").unwrap();
    assert!(!kind.applied && kind.chosen == "Bedtime");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
    assert!(g.hits.iter().any(|x| x.label == "\"funny\" ignored, Kind is Bedtime ×"));
    // Send uses the suggestions that stand, and nothing of them is remembered
    a.set_focus(Focus::Idea);
    key(&mut a, &cx, Key::Enter);
    until(&mut a, &cx, "the story", |a| !a.is_writing());
    let calls = t.story_calls();
    let (sys, user) = (&calls[0].0[0].content, &calls[0].0[1].content);
    assert!(sys.contains("A bedtime story") && !sys.contains("A funny story"), "the explicit kind won");
    assert!(sys.contains("About 750 words (between 600 and 900)"), "the five minutes reached the prompt: {sys}");
    assert!(user.contains("medium") && user.contains("in English"), "{user}");
    assert_eq!(a.story_opts.length, Length::Medium);
    assert_eq!(a.story_opts.style, PictureStyle::Comic, "the style named in the text");
    assert!(a.idea.text.is_empty(), "the box is empty after Send, like the chat's");
    assert_eq!(a.options().length, Length::Short, "a suggestion is not a choice");
    let saved = a.store.load_prefs().options;
    assert_eq!((saved.length, saved.language.as_str(), saved.genre), (Length::Short, "English", Genre::Bedtime));
    // the words come back as a suggestion when they change
    a.new_story(&cx);
    a.idea.set("a story in Hindi");
    a.sync_fields();
    assert_eq!(a.hint_chips().iter().map(|c| c.value.clone()).collect::<Vec<_>>(), vec!["Hindi".to_string()]);
    // an option chosen in Options beats the text, too
    click(&mut a, &cx, T_LANG + 3);
    assert!(!a.hint_chips()[0].applied);
    assert_eq!(a.shown_options().language, "Telugu");
    // nothing is guessed from words that only look like options
    a.idea.set("a fox in the long grass who loves warm soup");
    a.sync_fields();
    assert!(a.hint_chips().is_empty());
}

// ------------------------------------------------------------------ pictures: Auto

#[test]
fn pictures_are_automatic_images_first_svg_when_there_is_no_image_model_and_off_is_the_one_switch() {
    // no picture model: SVG drawings with the story model
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-auto-svg", &t);
    assert_eq!(a.options().pictures, PictureMode::Auto, "automatic is the default");
    assert_eq!(a.pictures_status(&cx), "Pictures: cartoon (built-in)");
    write_story(&mut a, &cx, "a fox");
    assert_eq!(a.story_opts.pictures, PictureMode::Drawings);
    assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings));
    until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    assert!(!t.drawing_calls().is_empty());
    assert_eq!(a.options().pictures, PictureMode::Auto, "automatic stays the remembered choice");
    assert_eq!(a.story_opts.style, PictureStyle::Watercolor, "a kids' bedtime story is a storybook watercolor");
    // a picture model exists: images have priority, named on the chip
    let t2 = Teller::new(STORY);
    let (mut b, cx2, _) = make("tales-auto-img", &t2);
    add_providers(&cx2, &["llama3", "flux"]);
    b.refresh_tts(&cx2);
    assert_eq!(b.pictures_status(&cx2), "Pictures: images via flux");
    let (g, _) = draw(&mut b, &cx2, 1280, 720, 0);
    assert!(g.hits.iter().any(|x| x.label == "Pictures: images via flux"), "the chip near the composer says it");
    write_story(&mut b, &cx2, "a fox");
    assert_eq!(b.story_opts.pictures, PictureMode::ImageModel);
    assert!(cx2.audit.recent(50).iter().any(|l| l.contains("story.picture") && l.contains("image scene 1")), "images were asked for first");
    assert!(find_in_form(&mut b, &cx2, 1280, 720, "Picture model").is_some(), "the picture model chip is in Options because images are available");
    // the story kind picks the style by itself, and the idea text can name another
    let t3 = Teller::new(STORY);
    let (mut c, cx3, _) = make("tales-style", &t3);
    click(&mut c, &cx3, T_GENRE + 3); // sci-fi
    write_story(&mut c, &cx3, "a robot");
    assert_eq!(c.story_opts.style, PictureStyle::Pixel);
    let t4 = Teller::new(STORY);
    let (mut d, cx4, _) = make("tales-style2", &t4);
    write_story(&mut d, &cx4, "a robot in pixel art");
    assert_eq!(d.story_opts.style, PictureStyle::Pixel, "named in the idea");
    // the one switch: pictures off, with or without a picture model
    let t5 = Teller::new(STORY);
    let (mut e, cx5, _) = make("tales-auto-off", &t5);
    add_providers(&cx5, &["flux"]);
    e.refresh_tts(&cx5);
    click(&mut e, &cx5, T_PICMODE);
    assert_eq!(e.pictures_status(&cx5), "Pictures: off");
    assert_eq!(e.store.load_prefs().options.pictures, PictureMode::Off, "remembered");
    write_story(&mut e, &cx5, "a fox");
    assert!(e.pics.is_empty() && t5.drawing_calls().is_empty());
    click(&mut e, &cx5, T_PICMODE);
    assert_eq!(e.options().pictures, PictureMode::Auto);
    // preferences saved by older versions (an explicit picture choice, a characters note) are brought along
    let mut o = StoryOptions { pictures: PictureMode::ImageModel, characters: "Old note".into(), ..Default::default() };
    normalize_options(&mut o);
    assert_eq!((o.pictures, o.characters.as_str()), (PictureMode::Auto, ""));
    let mut off = StoryOptions { pictures: PictureMode::Off, ..Default::default() };
    normalize_options(&mut off);
    assert_eq!(off.pictures, PictureMode::Off, "off stays off");
}

/// A rich drawing a strong model might make: a gradient sky and a few dozen shapes.
fn rich_svg() -> String {
    format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 800 480'><defs><linearGradient id='s' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#ff9966'/><stop offset='1' stop-color='#3366cc'/></linearGradient></defs><rect width='800' height='480' fill='url(#s)'/>{}</svg>", (0..24).map(|i| format!("<circle cx='{}' cy='{}' r='{}' fill='#{:02x}{:02x}40'/>", 30 + i * 31, 100 + (i * 53) % 300, 12 + i, i * 10, 255 - i * 9)).collect::<String>())
}

#[test]
fn the_pictures_are_drawn_by_the_chosen_writer_first_rich_for_a_strong_one_and_the_own_model_takes_over_when_it_fails() {
    let own = Teller::new(STORY);
    let agent = Teller::new(STORY);
    *agent.svg.lock().unwrap() = rich_svg();
    let (mut a, cx, home) = make("tales-route", &own);
    let (ta, to) = (agent.clone(), own.clone());
    a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", all_pictures_ready);
    assert_eq!(agent.drawing_calls().len(), 2, "the agent that was chosen drew both pictures");
    assert_eq!(own.drawing_calls().len(), 0, "the small model drew nothing");
    let ask = &agent.drawing_calls()[0].0;
    assert!(ask[0].content.contains("children's-book illustrator") && ask[0].content.contains("0 0 800 480"), "a real art-direction brief");
    assert!(ask[1].content.contains("DRAW THIS MOMENT") && ask[1].content.contains("Once upon a time") && ask[1].content.contains("The Brave Little Fox") && ask[1].content.contains("CHARACTER SHEET"), "the whole story, the moment and the sheet: {}", ask[1].content);
    assert!(a.pics.iter().all(|p| p.how == "rich SVG drawing by Claude Code"), "{:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
    let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.as_str()).collect();
    assert!(said.iter().any(|r| r.contains("rich SVG drawing by Claude Code")), "{said:?}");
    // the person's words about one picture reach the writer
    say(&mut a, &cx, "redraw picture 2: make the sky pink");
    until(&mut a, &cx, "the new picture", all_pictures_ready);
    assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"), "the feedback is in the request");
    // draw again keeps the feedback
    click(&mut a, &cx, T_PIC_REDO + 1);
    until(&mut a, &cx, "again", all_pictures_ready);
    assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"));
    // the agent goes away: the writing model takes over with the built-in cartoon, said honestly
    *agent.draw_error.lock().unwrap() = Some("agent offline".into());
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the fallback", all_pictures_ready);
    assert_eq!(a.pics[0].how, "cartoon (built-in)");
    assert!(own.drawing_calls().len() == 1 && own.drawing_calls()[0].0[0].content.contains("scene composer"));
    let id = a.saved_id().unwrap();
    let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap();
    assert!(log.contains("rich drawing, try 1") && log.contains("DRAW THIS MOMENT") && log.contains("RESULT: used"), "{log}");
}

#[test]
fn a_drawing_the_model_gets_wrong_becomes_a_built_in_sketch_that_says_so_and_the_log_shows_why() {
    let t = Teller::new(STORY);
    *t.svg.lock().unwrap() = "<svg><rect width='10' height='10'/></svg>".into();
    *t.scene.lock().unwrap() = "I would draw a fox".into();
    let (mut a, cx, home) = make("tales-sketch", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the sketches", all_pictures_ready);
    assert!(a.pics.iter().all(|p| p.how == "built-in sketch" && p.ext == "svg" && p.bytes.as_ref().unwrap().len() > 3000), "{:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
    let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.as_str()).collect();
    assert!(said.iter().all(|r| !r.contains("an SVG drawing")) && said.iter().any(|r| r.contains("Built-in sketch: the model's drawing could not be used")), "{said:?}");
    let id = a.saved_id().unwrap();
    let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap();
    for need in ["I would draw a fox", "not used", "plain drawing", "built-in sketch from the words", "REQUEST", "REPLY"] {
        assert!(log.contains(need), "{need}: {log}");
    }
}

#[test]
fn an_image_from_the_writer_is_kept_as_it_is_with_a_manifest_shown_in_the_slot_and_exported() {
    let png = crate::apps::chatpic::fixtures::png(64, 40, [200, 60, 40]);
    let own = Teller::new(STORY);
    let agent = Teller::new(STORY);
    *agent.svg.lock().unwrap() = format!("Here you are: ![the scene](data:image/png;base64,{})", st::base64_encode(&png));
    let (mut a, cx, home) = make("tales-imgreply", &own);
    let (ta, to) = (agent.clone(), own.clone());
    a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    assert!(a.pics.iter().all(|p| p.ext == "png" && p.how == "image by Claude Code" && p.bytes.as_ref().unwrap().starts_with(&[0x89, b'P'])), "{:?}", a.pics.iter().map(|p| (&p.ext, &p.how)).collect::<Vec<_>>());
    // drawn in the slot, in the frame of the tile
    let before = a.pic_cache.rasterised;
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(a.pic_cache.rasterised > before, "the PNG was decoded and drawn");
    assert_layout(&g, area, "story with a png picture");
    // kept beside the story with a manifest: kind, what made it, the prompt and the time
    let id = a.saved_id().unwrap();
    let dir = home.join(format!("Stories/{id}/pics"));
    assert!(dir.join("01.png").is_file() && dir.join("02.png").is_file());
    let manifest = std::fs::read_to_string(dir.join("index.json")).unwrap();
    for need in ["\"kind\": \"image\"", "image by Claude Code", "\"saved_at\"", "\"prompt\""] {
        assert!(manifest.contains(need), "{need}: {manifest}");
    }
    assert!(std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap().contains("(pics/01.png)"));
    // the person's words about one picture work for an image too
    say(&mut a, &cx, "redraw picture 2: make the sky pink");
    until(&mut a, &cx, "again", all_pictures_ready);
    assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"));
    // export carries the images
    a.prefs.auto_save = false;
    click(&mut a, &cx, T_EXPORT);
    let _ = draw(&mut a, &cx, 1280, 720, 0);
    key(&mut a, &cx, Key::Enter);
    let (_, bytes) = a.take_host_save().expect("export");
    assert_eq!(String::from_utf8(bytes).unwrap().matches("data:image/png;base64,").count(), 2);
}

#[test]
fn a_vp8_webm_clip_shows_its_first_picture_as_the_poster_and_the_slot_keeps_its_label() {
    let webp = include_bytes!("../../../iyakki-web/tests/data/webp_lossy.webp");
    let at = webp.windows(4).position(|w| w == b"VP8 ").unwrap();
    let n = u32::from_le_bytes([webp[at + 4], webp[at + 5], webp[at + 6], webp[at + 7]]) as usize;
    let webm = iyakki_web::webm::one_frame_clip_for_test(&webp[at + 8..at + 8 + n]);
    let reply = format!("data:video/webm;base64,{}", st::base64_encode(&webm));
    let own = Teller::new(STORY);
    let agent = Teller::new(STORY);
    *agent.svg.lock().unwrap() = reply;
    let (mut a, cx, _home) = make("tales-webm-poster", &own);
    let (ta, to) = (agent.clone(), own.clone());
    a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
    click(&mut a, &cx, T_VIDEO);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the clips", all_pictures_ready);
    assert!(a.pics.iter().all(|p| p.ext == "webm"), "{:?}", a.pics.iter().map(|p| p.ext.clone()).collect::<Vec<_>>());
    assert!(a.pic_scaled.is_empty());
    let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
    assert_layout(&g, area, "story with a poster frame");
    assert!(!a.pic_scaled.is_empty(), "the poster frame was decoded and kept for the slot");
    assert!(g.text_log.iter().any(|(_, _, t)| t.contains("Video clip")), "the label stays");
    // a clip whose first picture cannot be decoded keeps the card
    assert!(iyakki_web::webm::poster(b"\x1a\x45\xdf\xa3").is_err());
}

#[test]
fn a_video_clip_needs_the_option_and_is_kept_and_exported_when_allowed_and_a_link_is_never_fetched() {
    let mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom-and-some-more-bytes-of-a-tiny-fake-clip".to_vec();
    let reply = format!("data:video/mp4;base64,{}", st::base64_encode(&mp4));
    let make_app = |tag: &str, allow: bool| {
        let own = Teller::new(STORY);
        let agent = Teller::new(STORY);
        *agent.svg.lock().unwrap() = reply.clone();
        let (mut a, cx, home) = make(tag, &own);
        let (ta, to) = (agent.clone(), own.clone());
        a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
        assert!(!a.options().video, "off by default");
        if allow {
            click(&mut a, &cx, T_VIDEO);
            assert!(a.options().video);
        }
        (a, cx, home)
    };
    // off: the clip is refused with the reason and the built-in cartoon is drawn
    let (mut a, cx, _) = make_app("tales-video-off", false);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    assert!(a.pics.iter().all(|p| p.ext == "svg" && p.how == "cartoon (built-in)"));
    let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.as_str()).collect();
    assert!(said.iter().any(|r| r.contains("switched off")), "{said:?}");
    // on: kept as mp4 beside the story, the slot says so, the export plays it
    let (mut b, cx2, home) = make_app("tales-video-on", true);
    write_story(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the clips", all_pictures_ready);
    assert!(b.pics.iter().all(|p| p.ext == "mp4" && p.how == "video clip by Claude Code"));
    let (g, area) = draw(&mut b, &cx2, 1280, 720, 0);
    assert_layout(&g, area, "story with a video clip");
    assert!(b.pic_scaled.is_empty(), "an mp4 clip has no poster frame: the card stays");
    let id = b.saved_id().unwrap();
    assert_eq!(std::fs::read(home.join(format!("Stories/{id}/pics/01.mp4"))).unwrap(), mp4);
    assert!(std::fs::read_to_string(home.join(format!("Stories/{id}/pics/index.json"))).unwrap().contains("\"kind\": \"video\""));
    b.prefs.auto_save = false;
    click(&mut b, &cx2, T_EXPORT);
    let _ = draw(&mut b, &cx2, 1280, 720, 0);
    key(&mut b, &cx2, Key::Enter);
    let html = String::from_utf8(b.take_host_save().unwrap().1).unwrap();
    assert!(html.matches("<video muted loop controls").count() == 2 && html.contains("data:video/mp4;base64,"));
    // a reply that only links to a picture on the web is not fetched; the host is named in the log
    let own = Teller::new(STORY);
    let agent = Teller::new(STORY);
    *agent.svg.lock().unwrap() = "![scene](https://cdn.example.org/x/scene.png)".into();
    let (mut c, cx3, home3) = make("tales-link", &own);
    let (ta, to) = (agent.clone(), own.clone());
    c.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
    write_story(&mut c, &cx3, "a fox");
    until(&mut c, &cx3, "the pictures", all_pictures_ready);
    assert!(c.pics.iter().all(|p| p.ext == "svg"), "nothing was fetched");
    let id = c.saved_id().unwrap();
    let log = std::fs::read_to_string(home3.join(format!("Stories/{id}/pictures.log"))).unwrap();
    assert!(log.contains("cdn.example.org") && log.contains("not fetched without your approval"), "{log}");
}

/// A web that serves one picture and remembers every address it was asked for.
struct OnePicture(Vec<u8>, Mutex<Vec<String>>);
impl iyakki_web::Fetch for OnePicture {
    fn get(&self, url: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
        self.1.lock().unwrap().push(url.to_string());
        if url.to_string() == "https://cdn.example.org/x/scene.png" {
            Ok(iyakki_web::Response { status: 200, content_type: "image/png".into(), body: self.0.clone(), ..Default::default() })
        } else {
            Err("no route".into())
        }
    }
}

#[test]
fn a_link_in_the_writers_reply_is_offered_and_fetched_only_after_the_person_says_yes_and_a_message_with_an_address_asks_the_same_question() {
    let own = Teller::new(STORY);
    let agent = Teller::new(STORY);
    *agent.svg.lock().unwrap() = "![scene](https://cdn.example.org/x/scene.png)".into();
    let (mut a, cx0, _) = make("tales-link-ok", &own);
    let web = Arc::new(OnePicture(tiny_png(), Mutex::new(vec![])));
    let mut cx = cx0.clone();
    cx.web = web.clone();
    let (ta, to) = (agent.clone(), own.clone());
    a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    assert!(a.pics.iter().all(|p| p.ext == "svg" && p.links == vec!["https://cdn.example.org/x/scene.png".to_string()]), "the built-in picture, and the link offered");
    assert!(web.1.lock().unwrap().is_empty(), "nothing was fetched by itself");
    // the button names the site; a click asks; any other key than y keeps things as they are
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_LINK) && h.label.contains("cdn.example.org")), "one button names the site");
    click(&mut a, &cx, T_LINK);
    assert!(matches!(&a.modal, Some(Modal::FetchLink(0, u)) if u.contains("cdn.example.org")));
    let _ = draw(&mut a, &cx, 1280, 720, 0);
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.modal.is_none() && web.1.lock().unwrap().is_empty(), "no: nothing is fetched");
    // yes: fetched in the background, judged, and shown as the scene's picture
    click(&mut a, &cx, T_LINK);
    key(&mut a, &cx, Key::Char('y'));
    until(&mut a, &cx, "the linked picture", |a| a.pics[0].how.contains("you approved the link"));
    assert_eq!((a.pics[0].ext.as_str(), web.1.lock().unwrap().clone()), ("png", vec!["https://cdn.example.org/x/scene.png".to_string()]));
    assert!(a.pics[0].links.is_empty(), "the offer is used up");
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("story.picture.link") && l.contains("cdn.example.org")));
    // a link that gives no picture leaves the scene as it was and says why
    let before = a.pics[1].bytes.clone();
    a.pics[1].links = vec!["https://cdn.example.org/gone.png".to_string()];
    click(&mut a, &cx, T_LINK + 3);
    key(&mut a, &cx, Key::Char('y'));
    until(&mut a, &cx, "the failure", |a| a.note.as_ref().is_some_and(|n| n.0.contains("could not be used")));
    assert_eq!(a.pics[1].bytes, before);
    assert_eq!(a.pics[1].state, PicState::Ready);
    // an address in a message of the conversation asks the same question
    a.idea.set("use https://cdn.example.org/x/scene.png for picture 2");
    a.submit(&cx);
    assert!(matches!(&a.modal, Some(Modal::FetchLink(1, _))), "picture 2 is index 1");
    key(&mut a, &cx, Key::Esc);
    assert!(a.modal.is_none());
}

#[test]
fn the_drawing_option_cycles_and_shows_in_options_and_on_the_chip() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-drawmode", &t);
    assert_eq!(a.options().drawing, st::DrawingMode::Auto);
    assert!(find_in_form(&mut a, &cx, 1280, 720, "Drawing: Auto").is_some());
    click(&mut a, &cx, T_DRAWMODE);
    assert_eq!(a.options().drawing, st::DrawingMode::Image);
    click(&mut a, &cx, T_DRAWMODE);
    assert_eq!(a.options().drawing, st::DrawingMode::Rich);
    assert!(a.pictures_status(&cx).starts_with("Pictures: rich SVG by"), "{}", a.pictures_status(&cx));
    click(&mut a, &cx, T_DRAWMODE);
    assert_eq!(a.options().drawing, st::DrawingMode::Cartoon);
    assert_eq!(a.pictures_status(&cx), "Pictures: cartoon (built-in)");
    assert_eq!(a.store.load_prefs().options.drawing, st::DrawingMode::Cartoon, "remembered");
    click(&mut a, &cx, T_DRAWMODE);
    assert_eq!(a.options().drawing, st::DrawingMode::Auto);
    // pictures switched off hide the choice
    click(&mut a, &cx, T_PICMODE);
    assert!(find_in_form(&mut a, &cx, 1280, 720, "Drawing: Auto").is_none());
}

#[test]
fn when_automatic_images_fail_the_rest_are_drawn_as_svg_and_it_is_said_once() {
    let t = Teller::new(&STORY.replace("\n\nShe walked", &"\n\nAnother long paragraph about the river, the reeds and the night wind that went on for quite a while.".repeat(8)));
    let (mut a, cx, _) = make("tales-auto-fall", &t);
    add_providers(&cx, &["flux"]);
    write_story(&mut a, &cx, "a fox");
    assert!(a.pics.len() >= 3, "{} scenes", a.pics.len());
    assert!(cx.audit.recent(50).iter().any(|l| l.contains("story.picture") && l.contains("image scene 1")), "images came first");
    until(&mut a, &cx, "every scene done", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
    assert!(!a.pic_auto, "the fall-back happened once");
    assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings), "the scenes were drawn as SVG");
    assert_eq!(t.drawing_calls().len(), a.pics.len());
    let note = a.note_text().unwrap_or_default().to_string();
    assert!(note.contains("Image generation did not work") && note.contains("drawn as SVG"), "{note}");
    assert_eq!(note.matches("Image generation").count(), 1, "said once");
    // the conversation says so too: the failed step, then the new steps
    let results: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.as_str()).collect();
    assert!(results.iter().any(|r| r.starts_with("Failed: image generation did not work")), "{results:?}");
    assert!(results.iter().filter(|r| r.contains("cartoon (built-in)")).count() >= 3, "{results:?}");
}

// ------------------------------------------------------------------ a conversation: corrections, versions, steering

/// STORY with the owl named: only the middle paragraph changed.
const REV: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said a wise owl named Beeru. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept well.\n";
/// REV with a new ending: the last paragraph changed.
const REV2: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said a wise owl named Beeru. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept soundly until the morning.\n";

/// Type a message in the panel's box and send it with Enter.
fn say(a: &mut StoryApp, cx: &AppCx, text: &str) {
    a.set_focus(Focus::Idea);
    a.idea.set(text);
    a.sync_fields();
    key(a, cx, Key::Enter);
}

fn all_pictures_ready(a: &mut StoryApp) -> bool {
    a.pics.iter().all(|p| p.state == PicState::Ready)
}

// ------------------------------------------------------------------ illustrations by the picture model

/// A tiny real PNG (8x8).
fn tiny_png() -> Vec<u8> {
    let hex = "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc0000001c74455874436f6d6d656e7400736563726574206c6f636174696f6e206461746154e84f540000002a49444154789c63d4883ab1e0510203034382dc026406e3091b0d4c5106060626aca209720b1807d22800fc713f4fb3c21d830000000049454e44ae426082";
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect()
}

/// A picture server that is never reached over a network: it answers the image calls with a tiny PNG (or an error), after
/// an optional delay, and remembers what it was asked.
struct FakeImages {
    asked: Mutex<Vec<String>>,
    delay_ms: u64,
    fail: bool,
}

impl FakeImages {
    fn new(delay_ms: u64, fail: bool) -> Arc<FakeImages> {
        Arc::new(FakeImages { asked: Mutex::new(vec![]), delay_ms, fail })
    }
    fn count(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

struct ImgUp(Arc<FakeImages>);

impl iyakki_proxy::Upstream for ImgUp {
    fn request(&self, _m: &str, u: &str, _h: &[(String, String)], b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        let this = &self.0;
        if !(u.ends_with("/api/generate") || u.ends_with("/images/generations")) {
            return Err("offline".into());
        }
        this.asked.lock().unwrap().push(String::from_utf8_lossy(b).into_owned());
        if this.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(this.delay_ms));
        }
        if this.fail {
            return Ok(iyakki_proxy::UpstreamResponse { status: 500, body: b"{\"error\":\"the picture model fell over\"}".to_vec() });
        }
        let body = serde_json::json!({ "images": [st::base64_encode(&tiny_png())] }).to_string().into_bytes();
        Ok(iyakki_proxy::UpstreamResponse { status: 200, body })
    }
}

fn with_images(tag: &str, delay_ms: u64, fail: bool) -> (StoryApp, AppCx, PathBuf, Arc<Teller>, Arc<FakeImages>) {
    let t = Teller::new(STORY);
    let fake = FakeImages::new(delay_ms, fail);
    let (mut a, cx, home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
    add_providers(&cx, &["llama3", "flux"]);
    a.refresh_tts(&cx);
    (a, cx, home, t, fake)
}

fn lots_of_memory() -> iyakki_core_api::sysinfo::Memory {
    iyakki_core_api::sysinfo::Memory { total: 32 << 30, available: 24 << 30, swap_total: 0, swap_free: 0 }
}

#[test]
fn tales_illustrations_are_offered_with_a_plain_reason_and_no_error_when_there_is_no_picture_model() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-illus-none", &t);
    assert_eq!(a.readiness(&cx), Readiness::NoModel);
    let why = illus::reason(&a.readiness(&cx)).unwrap();
    assert!(why.contains("No picture model is selected") && why.contains("Settings") && why.contains("Models"), "{why}");
    // the option is on the page (it cycles) and the story is written with drawings and no error
    assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Pictures: every chapter").as_deref(), Some("Pictures: every chapter"));
    write_story(&mut a, &cx, "a fox");
    assert!(a.error.is_none() && !a.story.is_empty());
    assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings), "drawings, not the picture model");
    // a Lemonade picture model that is not downloaded is offered as Install first, not as a failure
    let mut l = iyakki_shared::providers::ProviderCfg::new("lemon", iyakki_shared::providers::Kind::Lemonade);
    l.base_url = Some("http://lemonade-tales-test.invalid:8000".into());
    cx.settings().lock().unwrap().upsert_provider(l).unwrap();
    let m = iyakki_shared::lemonade::Model { id: "SDXL-Base-1.0".into(), kind: iyakki_shared::lemonade::ModelType::Image, recipe: "sd-cpp".into(), checkpoint: String::new(), labels: vec!["image".into()], downloaded: false, loaded: false, suggested: false, size_gb: Some(6.9), image_defaults: None };
    iyakki_shared::lemonade::remember(&iyakki_shared::lemonade::api_base("http://lemonade-tales-test.invalid:8000"), &[m]);
    let r = a.readiness(&cx);
    assert_eq!(r, Readiness::NotDownloaded { model: "SDXL-Base-1.0".into(), size: "6.9 GB".into() });
    assert!(illus::reason(&r).unwrap().contains("Install first"));
}

#[test]
fn tales_illustrations_cover_only_every_chapter_and_ask_me_each_time_with_the_prompt_notes_and_metadata_kept() {
    // every chapter (the default): a picture per scene by the picture model
    let (mut a, cx, _home, t, fake) = with_images("tales-illus-all", 0, false);
    a.test_memory = Some(lots_of_memory());
    assert_eq!(a.opts.coverage, Coverage::Chapters);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    assert_eq!(a.pics.len(), 2);
    assert_eq!(fake.count(), 2, "one request per picture, one at a time");
    let asked = fake.asked.lock().unwrap().join("\n");
    assert!(asked.contains(PictureStyle::Watercolor.phrase()), "the one style line is in every prompt: {asked}");
    assert!(asked.contains("A small fox sits by a quiet river") && asked.contains("Setting: a river meadow at dusk"), "the brief from the story model and the setting note: {asked}");
    assert_eq!(a.setting.as_deref(), Some("a river meadow at dusk, soft blue light"), "the note is made once per story");
    assert_eq!(t.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("where and when a story happens")).count(), 1);
    assert!(a.pics.iter().all(|p| p.size == "8x8" && p.how == "images via flux"));
    // kept with the story: the file, its prompt, the model and the size, and the setting note
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    let kept = a.store.load_pictures(&id);
    assert_eq!(kept.len(), 2);
    assert_eq!((kept[0].size.as_str(), kept[0].source.as_str(), kept[0].mode.as_str()), ("8x8", "images via flux", "image"));
    assert!(kept[0].prompt.contains("river meadow") && kept[0].bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    assert_eq!(a.store.load_setting(&id).as_deref(), Some("a river meadow at dusk, soft blue light"));
    // cover only: exactly one picture
    let (mut b, cx2, _h, _t, fake2) = with_images("tales-illus-cover", 0, false);
    b.test_memory = Some(lots_of_memory());
    click(&mut b, &cx2, T_COVERAGE);
    assert_eq!(b.opts.coverage, Coverage::Cover);
    assert_eq!(b.store.load_prefs().options.coverage, Coverage::Cover, "remembered");
    write_story(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the cover", all_pictures_ready);
    assert_eq!((b.pics.len(), fake2.count()), (1, 1));
    // ask me each time: nothing is drawn until the person says so; the text is there meanwhile
    let (mut c, cx3, _h, _t, fake3) = with_images("tales-illus-ask", 0, false);
    c.test_memory = Some(lots_of_memory());
    click(&mut c, &cx3, T_COVERAGE);
    click(&mut c, &cx3, T_COVERAGE);
    assert_eq!(c.opts.coverage, Coverage::Ask);
    write_story(&mut c, &cx3, "a fox");
    assert!(c.held && !c.story.is_empty() && fake3.count() == 0 && c.pics.len() == 2);
    for _ in 0..20 {
        c.handle(&Event::Tick, &cx3);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fake3.count(), 0, "still waiting for the yes");
    let (g, _) = draw(&mut c, &cx3, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Make the pictures") && g.hits.iter().any(|h| h.label == "No pictures"));
    click(&mut c, &cx3, T_PICS_GO);
    until(&mut c, &cx3, "the pictures after the yes", all_pictures_ready);
    assert_eq!(fake3.count(), 2);
    // and a no keeps the text and draws nothing
    let (mut d, cx4, _h, _t, fake4) = with_images("tales-illus-no", 0, false);
    click(&mut d, &cx4, T_COVERAGE);
    click(&mut d, &cx4, T_COVERAGE);
    write_story(&mut d, &cx4, "a fox");
    let words = d.story.words();
    click(&mut d, &cx4, T_PICS_SKIP);
    assert!(!d.held && d.pics.is_empty() && fake4.count() == 0 && d.story.words() == words);
}


// ------------------------------------------------------------------ picture look, quality, model, seed and the picture log

fn img_model(id: &str, gb: f64, w: u32, steps: u32) -> iyakki_shared::lemonade::Model {
    iyakki_shared::lemonade::Model { id: id.into(), kind: iyakki_shared::lemonade::ModelType::Image, recipe: "sd-cpp".into(), checkpoint: String::new(), labels: vec!["image".into()], downloaded: true, loaded: false, suggested: false, size_gb: Some(gb), image_defaults: Some(iyakki_shared::lemonade::ImageDefaults { width: Some(w), height: Some(w), steps: Some(steps) }) }
}

/// A Lemonade server on this computer with SD-Turbo and SDXL, and one on another computer with FLUX.
fn with_two_lemonades(tag: &str, delay_ms: u64, fail: bool) -> (StoryApp, AppCx, PathBuf, Arc<FakeImages>) {
    let t = Teller::new(STORY);
    let fake = FakeImages::new(delay_ms, fail);
    let (mut a, cx, home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
    let (local, remote) = (format!("http://127.0.0.1:{}", 20000 + hash(tag) % 20000), format!("http://{tag}.gpu-box.invalid:13305"));
    for (id, base) in [("home", &local), ("gpu", &remote)] {
        let mut l = iyakki_shared::providers::ProviderCfg::new(id, iyakki_shared::providers::Kind::Lemonade);
        l.base_url = Some(base.clone());
        cx.settings().lock().unwrap().upsert_provider(l).unwrap();
    }
    iyakki_shared::lemonade::remember(&iyakki_shared::lemonade::api_base(&local), &[img_model("SD-Turbo", 2.0, 512, 4), img_model("SDXL-Base-1.0", 6.9, 1024, 20)]);
    iyakki_shared::lemonade::remember(&iyakki_shared::lemonade::api_base(&remote), &[img_model("FLUX-Dev", 23.0, 1024, 28)]);
    a.refresh_tts(&cx);
    a.test_memory = Some(lots_of_memory());
    a.confirm_secs = f64::MAX;
    (a, cx, home, fake)
}

fn bodies(f: &FakeImages) -> Vec<serde_json::Value> {
    f.asked.lock().unwrap().iter().map(|b| serde_json::from_str(b).unwrap()).collect()
}

#[test]
fn tales_picture_style_quality_model_and_seed_are_options_kept_per_story_and_as_default() {
    let (mut a, cx, _home, fake) = with_two_lemonades("tales-pic-opts", 0, false);
    assert_eq!(cx.services.gpu_lemonade().as_deref(), Some("gpu"), "a Lemonade server that is not this computer is the GPU one");
    assert_eq!(a.pic_now(), illus::PictureOpts::default(), "the old defaults");
    // the chips are on the page, with the provider named, the model's own size and steps and the plain advice
    let l = find_in_form(&mut a, &cx, 1920, 1080, "Style \u{2611}: storybook illustration").unwrap();
    assert!(!l.is_empty());
    for want in ["Quality: as selected", "Model for this story: automatic", "Same look for all pictures: off", "Picture log (g)"] {
        assert!(find_in_form(&mut a, &cx, 1920, 1080, want).is_some(), "{want}");
    }
    let (g, _) = draw(&mut a, &cx, 1920, 1080, 1);
    assert!(g.hits.len() > 5);
    // the styles are ticked one by one and remembered as the default; the last one cannot be unticked
    click(&mut a, &cx, T_STYLE + 1);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Storybook, illus::Look::Watercolor]);
    click(&mut a, &cx, T_STYLE);
    click(&mut a, &cx, T_STYLE + 1);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Watercolor], "the last style stays ticked");
    click(&mut a, &cx, T_REALISTIC);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo], "one style ticked: Realistic replaces it");
    click(&mut a, &cx, T_REALISTIC);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Watercolor], "unticking brings the styles back");
    click(&mut a, &cx, T_STYLE + 2);
    click(&mut a, &cx, T_STYLE + 1);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo]);
    assert_eq!(a.store.load_prefs().options.pic.styles_list(), vec![illus::Look::Photo]);
    click(&mut a, &cx, T_QUALITY);
    click(&mut a, &cx, T_QUALITY);
    click(&mut a, &cx, T_QUALITY);
    assert_eq!(a.opts.pic.quality, illus::Quality::Best);
    let r = a.resolved_for(&a.pic_now(), &cx).unwrap();
    assert_eq!((r.provider.as_str(), r.model.as_str(), r.steps, r.width), ("gpu", "FLUX-Dev", Some(35), 1024), "Best picks the largest model, here on the other computer, with more steps");
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let sent = bodies(&fake);
    assert_eq!(sent.len(), 2);
    for b in &sent {
        assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str()), (Some("FLUX-Dev"), Some(35), Some("1024x1024")), "the steps of Quality Best inside what the model has");
        assert_eq!(b["cfg_scale"].as_f64(), Some(6.0), "photo style asks a model that is not Turbo for guidance");
        let prompt = b["prompt"].as_str().unwrap();
        assert!(prompt.starts_with("a realistic photograph, shot on a 50mm lens") && prompt.contains("A small fox sits by a quiet river") && prompt.chars().count() <= illus::Family::Long.cap(), "{prompt}");
        assert!(prompt.contains("Avoid: cartoon, illustration") && b.get("negative_prompt").is_none(), "the images API is not known to take a negative prompt: {prompt}");
    }
    assert_ne!(sent[0]["seed"], sent[1]["seed"], "a new random seed for every picture");
    // saved with the story: its own file, back when it is opened, whatever the defaults are now
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    let kept = a.store.load_picture_opts(&id).unwrap();
    assert_eq!((kept.styles_list(), kept.quality), (vec![illus::Look::Photo], illus::Quality::Best));
    click(&mut a, &cx, T_STYLE + 3);
    click(&mut a, &cx, T_QUALITY);
    assert!(a.story_opts.pic.styles_list().contains(&illus::Look::Cinematic) && a.store.load_picture_opts(&id).unwrap().styles_list().contains(&illus::Look::Cinematic), "changing it on the page changes the story's choice too");
    a.opts.pic = illus::PictureOpts::default();
    let e = a.store.list().into_iter().find(|e| e.id == id).unwrap();
    assert!(a.options_of(&e).pic.styles_list().contains(&illus::Look::Cinematic), "opened later, the story keeps its own styles");
    // a model for this story: a model of any provider, named with its provider; wins over Quality
    let (mut b, cx2, _h, fake2) = with_two_lemonades("tales-pic-model", 0, false);
    click(&mut b, &cx2, T_QUALITY);
    assert_eq!(b.resolved_for(&b.pic_now(), &cx2).unwrap().model, "SD-Turbo");
    click(&mut b, &cx2, T_STORYMODEL);
    let first = b.opts.pic.model.clone().unwrap();
    assert!(b.found_images(&cx2).contains(&(first.provider.clone(), first.model.clone())));
    while b.opts.pic.model.as_ref().map(|m| m.model.as_str()) != Some("FLUX-Dev") {
        click(&mut b, &cx2, T_STORYMODEL);
    }
    assert_eq!(find_in_form(&mut b, &cx2, 1920, 1080, "Model for this story: FLUX-Dev \u{b7} gpu").is_some(), true, "the provider is shown");
    assert_eq!(b.resolved_for(&b.pic_now(), &cx2).unwrap().why, "chosen for this story");
    click(&mut b, &cx2, T_STORYMODEL);
    assert!(b.opts.pic.model.is_none(), "after the last model it is automatic again");
    b.opts.pic.model = Some(ModelChoice { provider: "gpu".into(), model: "FLUX-Dev".into() });
    write_story(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the pictures", all_pictures_ready);
    assert!(bodies(&fake2).iter().all(|x| x["model"] == "FLUX-Dev"));
    // keep the same look: one seed for the whole story, kept with it
    let (mut c, cx3, _h, fake3) = with_two_lemonades("tales-pic-seed", 0, false);
    click(&mut c, &cx3, T_KEEPLOOK);
    assert!(c.opts.pic.keep_look && c.opts.pic.seed.is_none());
    write_story(&mut c, &cx3, "a fox");
    until(&mut c, &cx3, "the pictures", all_pictures_ready);
    let seeds: Vec<_> = bodies(&fake3).iter().map(|x| x["seed"].clone()).collect();
    assert!(seeds.len() == 2 && seeds[0] == seeds[1] && seeds[0].is_u64(), "{seeds:?}");
    let id = c.saved_id().unwrap();
    assert_eq!(c.store.load_picture_opts(&id).unwrap().seed, seeds[0].as_u64(), "the story keeps its seed");
    assert!(c.opts.pic.seed.is_none(), "the default never holds a seed");
}

#[test]
fn tales_picture_log_keeps_every_request_skip_and_fallback_in_order_without_secrets() {
    let (mut a, cx, _home, _fake) = with_two_lemonades("tales-piclog", 0, false);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let id = a.saved_id().unwrap();
    let ev = a.store.load_picture_log(&id);
    assert_eq!(ev.len(), 2, "{ev:?}");
    let e = &ev[0];
    assert_eq!((e.kind.as_str(), e.story.as_str(), e.picture, e.attempt), ("request", "The Brave Little Fox", 1, 1));
    assert_eq!((e.provider.as_str(), e.model.as_str(), e.size.as_str(), e.steps, e.status, e.format.as_str()), ("home", "SD-Turbo", "512x512", Some(4), Some(200), "png"), "{e:?}");
    assert!(e.seed.is_some() && e.bytes > 0 && !e.prompt.is_empty() && e.negative.contains("extra fingers") && e.verbose.is_none(), "verbose is off by default: {e:?}");
    assert_eq!(e.negative_sent, "not sent", "no room for the avoid-words in a short SD-Turbo prompt");
    assert!(e.prompt.chars().count() <= illus::Family::Turbo.cap(), "SD-Turbo gets a short prompt: {}", e.prompt);
    assert_eq!(ev[1].picture, 2);
    // Try again is a new attempt of the same picture, logged after the others
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the redraw", |a| a.pics[0].state == PicState::Ready && a.store.load_picture_log(&id).len() == 3);
    assert_eq!(a.store.load_picture_log(&id)[2].attempt, 2);
    // the view: opened by the button and by g, newest at the bottom, copy gives the whole text
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Picture log (g)");
    assert!(matches!(a.modal, Some(Modal::PicLog(_))));
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    for l in ["Copy log", "Clear log", "Verbose: off", "Close"] {
        assert!(g.hits.iter().any(|h| h.label == l), "{l}");
    }
    click(&mut a, &cx, T_LOG_COPY);
    let all = piclog::log_text(&a.pic_log_events());
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(all.as_str()), "Copy log copies the whole text");
    assert!(all.contains("SD-Turbo on home") && all.contains("HTTP 200") && all.contains("attempt 2 (retries 1)"));
    // verbose is a switch, off by default, kept
    click(&mut a, &cx, T_LOG_VERBOSE);
    assert!(a.pic_now().verbose && a.store.load_picture_opts(&id).unwrap().verbose);
    click(&mut a, &cx, T_LOG_VERBOSE);
    assert!(!a.pic_now().verbose);
    // scrolling: many events, the latest at the bottom, keys and the wheel move it, it stays inside the log
    let many: Vec<PicEvent> = (0..80).map(|n| piclog::decision_event(1_792_000_000 + n, plk::SKIPPED, "S", 1, "A", &format!("skipped number {n}"))).collect();
    a.log_pic_events(many);
    let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
    let (total, rows, top) = if let Some(Modal::PicLog(v)) = &a.modal { (v.total.get(), v.rows.get(), v.top.get()) } else { unreachable!() };
    assert!(total > rows * 2 && top == total - rows, "it opens at the latest events: {total} {rows} {top}");
    key(&mut a, &cx, Key::Up);
    key(&mut a, &cx, Key::PageUp);
    let now = if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 };
    assert_eq!(now, top - 1 - (rows - 1));
    a.handle(&Event::Scroll(-2), &cx);
    let up = if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 };
    assert_eq!(up, now + 6, "the wheel scrolls the log (down), not the story");
    key(&mut a, &cx, Key::Home);
    let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert_eq!(if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 9 }, 0);
    key(&mut a, &cx, Key::End);
    key(&mut a, &cx, Key::PageDown);
    assert_eq!(if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 }, total - rows);
    // clear asks twice, then the log is empty on disk too
    click(&mut a, &cx, T_LOG_CLEAR);
    assert!(!a.store.load_picture_log(&id).is_empty(), "one press only asks");
    click(&mut a, &cx, T_LOG_CLEAR);
    assert!(a.store.load_picture_log(&id).is_empty() && a.pic_log_events().is_empty());
    key(&mut a, &cx, Key::Esc);
    assert!(a.modal.is_none());
    key(&mut a, &cx, Key::Char('g'));
    assert!(matches!(a.modal, Some(Modal::PicLog(_))), "g opens it");
}

#[test]
fn tales_picture_log_names_the_server_error_the_svg_fallback_the_memory_skip_and_the_cancel() {
    // the picture model fails: its status and text are logged, and the rest of the story falls back to SVG, said once
    let (mut a, cx, _home, _fake) = with_two_lemonades("tales-piclog-fail", 0, true);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", all_pictures_ready);
    let id = a.saved_id().unwrap();
    let ev = a.store.load_picture_log(&id);
    let req = ev.iter().find(|e| e.kind == plk::REQUEST).unwrap();
    assert_eq!(req.status, Some(500));
    assert!(req.error.contains("fell over") && req.format.is_empty(), "{req:?}");
    let fb = ev.iter().find(|e| e.kind == plk::SVG_FALLBACK).unwrap();
    assert!(fb.svg_fallback.contains("fell over"), "{fb:?}");
    assert!(ev.iter().position(|e| e.kind == plk::REQUEST) < ev.iter().position(|e| e.kind == plk::SVG_FALLBACK), "in order");
    // low memory: nothing is asked, and the decision is logged
    let (mut b, cx2, _h, fake2) = with_two_lemonades("tales-piclog-mem", 0, false);
    b.test_memory = Some(iyakki_core_api::sysinfo::Memory { total: 8 << 30, available: 200 << 20, swap_total: 0, swap_free: 0 });
    write_story(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the pictures to wait", |b| b.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    assert_eq!(fake2.count(), 0);
    let ev = b.pic_log_events();
    assert!(ev.iter().any(|e| e.kind == plk::SKIPPED && e.decision.starts_with("skipped: free memory low") && e.decision.contains("200 MB free")), "{ev:?}");
    // cancelled while being drawn: the late answer is still logged
    let (mut c, cx3, _h, fake3) = with_two_lemonades("tales-piclog-cancel", 300, false);
    write_story(&mut c, &cx3, "a fox");
    until(&mut c, &cx3, "the first request", |_| fake3.count() >= 1);
    click(&mut c, &cx3, T_PICS_CANCEL);
    assert!(c.pic_log_events().iter().any(|e| e.decision.starts_with("cancelled")));
    until(&mut c, &cx3, "the late answer in the log", |c| c.pic_log_events().iter().any(|e| e.kind == plk::REQUEST));
    // a story that is not saved yet keeps the events in memory and writes them when it is saved
    let (mut d, cx4, _h, _f) = with_two_lemonades("tales-piclog-unsaved", 0, false);
    d.prefs.auto_save = false;
    write_story(&mut d, &cx4, "a fox");
    until(&mut d, &cx4, "the pictures", all_pictures_ready);
    assert!(d.saved_id().is_none() && d.pic_pending.len() == 2);
    click(&mut d, &cx4, T_SAVE);
    let id = d.saved_id().unwrap();
    assert!(d.pic_pending.is_empty() && d.store.load_picture_log(&id).len() == 2);
}


/// A Lemonade server that is never reached over a network: its model list says whether SDXL is downloaded (a download
/// makes it so), its health lists a busy model, and its picture call is slow when asked to be.
struct LiveUp {
    sdxl_downloaded: std::sync::atomic::AtomicBool,
    delay_ms: u64,
    /// `delay_ms == HELD`: the picture call answers only after [`LiveUp::release`] (a gate, not a time a loaded machine can overrun)
    released: std::sync::atomic::AtomicBool,
    gens: AtomicUsize,
    health_calls: AtomicUsize,
    pulls: AtomicUsize,
}

const HELD: u64 = u64::MAX;

impl LiveUp {
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    fn new(downloaded: bool, delay_ms: u64) -> Arc<LiveUp> {
        Arc::new(LiveUp { sdxl_downloaded: std::sync::atomic::AtomicBool::new(downloaded), delay_ms, released: std::sync::atomic::AtomicBool::new(false), gens: AtomicUsize::new(0), health_calls: AtomicUsize::new(0), pulls: AtomicUsize::new(0) })
    }
}

struct LiveNet(Arc<LiveUp>);

impl iyakki_proxy::Upstream for LiveNet {
    fn request(&self, _m: &str, u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        let t = &self.0;
        let ok = |v: serde_json::Value| Ok(iyakki_proxy::UpstreamResponse { status: 200, body: v.to_string().into_bytes() });
        if u.ends_with("/models?show_all=true") {
            let dl = t.sdxl_downloaded.load(Ordering::SeqCst);
            return ok(serde_json::json!({"data": [
                {"id": "SD-Turbo", "recipe": "sd-cpp", "labels": ["image"], "size": 2.0, "downloaded": true, "image_defaults": {"steps": 4, "width": 512, "height": 512}},
                {"id": "SDXL-Base-1.0", "recipe": "sd-cpp", "labels": ["image"], "size": 6.94, "downloaded": dl, "image_defaults": {"steps": 20, "width": 1024, "height": 1024}}
            ]}));
        }
        if u.ends_with("/health") {
            t.health_calls.fetch_add(1, Ordering::SeqCst);
            return ok(serde_json::json!({"status": "ok", "all_models_loaded": [{"model_name": "kokoro-v1"}]}));
        }
        if u.ends_with("/images/generations") {
            t.gens.fetch_add(1, Ordering::SeqCst);
            if t.delay_ms == HELD {
                iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || t.released.load(Ordering::SeqCst));
            } else {
                std::thread::sleep(Duration::from_millis(t.delay_ms));
            }
            return ok(serde_json::json!({ "data": [{"b64_json": st::base64_encode(&tiny_png())}] }));
        }
        Err("offline".into())
    }
}

impl iyakki_proxy::Streamer for LiveNet {
    fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        Err("not used".into())
    }
    fn post(&self, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
        self.0.pulls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        self.0.sdxl_downloaded.store(true, Ordering::SeqCst);
        let body = "data: {\"bytes_downloaded\": 100, \"bytes_total\": 200, \"file\": \"sdxl.gguf\"}\nevent: complete\ndata: {}\n";
        Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(body.as_bytes().to_vec())) })
    }
}

/// An app whose only Lemonade server says SDXL is downloaded in its saved list (stale) and the live truth is `downloaded`;
/// SDXL is chosen for the story.
fn with_live_lemonade(tag: &str, downloaded: bool, delay_ms: u64) -> (StoryApp, AppCx, Arc<LiveUp>) {
    let t = Teller::new(STORY);
    let live = LiveUp::new(downloaded, delay_ms);
    let (mut a, cx, _home) = make_up(tag, &t, Arc::new(LiveNet(live.clone())));
    cx.set_streamer(Arc::new(LiveNet(live.clone())));
    let base = format!("http://127.0.0.1:{}", 20000 + hash(tag) % 20000);
    let mut l = iyakki_shared::providers::ProviderCfg::new("home", iyakki_shared::providers::Kind::Lemonade);
    l.base_url = Some(base.clone());
    cx.settings().lock().unwrap().upsert_provider(l).unwrap();
    iyakki_shared::lemonade::remember(&iyakki_shared::lemonade::api_base(&base), &[img_model("SD-Turbo", 2.0, 512, 4), img_model("SDXL-Base-1.0", 6.94, 1024, 20)]);
    a.refresh_tts(&cx);
    a.test_memory = Some(lots_of_memory());
    a.confirm_secs = f64::MAX;
    a.opts.pic.model = Some(ModelChoice { provider: "home".into(), model: "SDXL-Base-1.0".into() });
    (a, cx, live)
}

#[test]
fn tales_picture_that_is_not_downloaded_is_not_called_and_offers_install() {
    let (mut a, cx, live) = with_live_lemonade("tales-notdl", false, 0);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures to stop", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))) && a.install.is_some());
    assert_eq!(live.gens.load(Ordering::SeqCst), 0, "the picture call was never made");
    let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
    assert!(why.contains("SDXL-Base-1.0 is not downloaded on home (6.9 GB)"), "{why}");
    assert!(a.pics.iter().all(|p| p.mode == PictureMode::ImageModel), "no fall back to SVG for this");
    let ev = a.pic_log_events();
    assert!(ev.iter().any(|e| e.kind == plk::NOT_DOWNLOADED && e.model == "SDXL-Base-1.0" && e.decision.contains("Nothing was asked")), "{ev:?}");
    assert!(!ev.iter().any(|e| e.kind == plk::REQUEST || e.kind == plk::SVG_FALLBACK));
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Install SDXL-Base-1.0 (6.9 GB)"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    // the story's model list says which models are installed, and their size
    let list = cx.ai().picture_choices(&a.found_images(&cx)).iter().map(|c| c.line()).collect::<Vec<_>>().join("|");
    assert!(list.contains("SD-Turbo on home: installed, 2.0 GB, 512x512, 4 steps") && list.contains("SDXL-Base-1.0 on home: not downloaded, 6.9 GB, 1024x1024, 20 steps"), "{list}");
    assert!(find_in_form(&mut a, &cx, 1920, 1080, "Model for this story: SDXL-Base-1.0 \u{b7} home (not downloaded, 6.9 GB)").is_some());
}

#[test]
fn tales_picture_install_pulls_with_progress_and_then_draws_the_waiting_pictures() {
    let (mut a, cx, live) = with_live_lemonade("tales-pull", false, 0);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the offer", |a| a.install.is_some() && a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    click(&mut a, &cx, T_PIC_INSTALL);
    assert!(a.install.as_ref().is_some_and(|i| i.started));
    until(&mut a, &cx, "the pictures after the download", all_pictures_ready);
    assert_eq!(live.pulls.load(Ordering::SeqCst), 1);
    assert_eq!(live.gens.load(Ordering::SeqCst), 2, "drawn after the pull, one call each");
    assert!(a.install.is_none());
    let ev = a.pic_log_events();
    let at = |f: &dyn Fn(&PicEvent) -> bool| ev.iter().position(|e| f(e)).unwrap_or_else(|| panic!("missing in {ev:?}"));
    let (nd, start, fin, req) = (at(&|e| e.kind == plk::NOT_DOWNLOADED), at(&|e| e.decision.starts_with("pull started")), at(&|e| e.decision == "pull finished"), at(&|e| e.kind == plk::REQUEST));
    assert!(nd < start && start < fin && fin < req, "in order: {nd} {start} {fin} {req}");
    assert!(ev.iter().filter(|e| e.kind == plk::REQUEST).all(|e| e.model == "SDXL-Base-1.0" && e.status == Some(200)), "{ev:?}");
}

#[test]
fn tales_picture_that_is_slow_shows_elapsed_time_and_the_server_state_and_logs_the_wait() {
    let (mut a, cx, live) = with_live_lemonade("tales-slow", true, HELD);
    a.test_timing = Some((Duration::from_secs(60), Duration::from_millis(40), Duration::from_millis(100)));
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "a wait line with the server state", |a| a.wait_text.as_ref().is_some_and(|n| n.contains("so far") && n.contains("server: loaded models: kokoro-v1")));
    let n = a.wait_text.clone().unwrap();
    assert!(n.contains("Drawing picture 1") && n.contains("The model is loading or generating; on a CPU this can take minutes") && n.contains("SDXL-Base-1.0 is not loaded yet") && n.contains("Cancel pictures"), "{n}");
    assert!(live.health_calls.load(Ordering::SeqCst) >= 1, "the health is polled while it waits");
    // the first "waiting" line can be written before the server's first answer has arrived (a slow health call): the line that
    // carries the server state is the one that proves the state reaches the log, and the log repeats the line, so wait for it
    until(&mut a, &cx, "a wait line with the server state in the picture log", |a| a.pic_log_events().iter().any(|e| e.kind == plk::WAITING && e.decision.contains("kokoro-v1")));
    live.release();
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let ev = a.pic_log_events();
    let w = ev.iter().find(|e| e.kind == plk::WAITING && e.decision.contains("kokoro-v1")).unwrap_or_else(|| panic!("{ev:?}"));
    assert!(ev.iter().filter(|e| e.kind == plk::WAITING).all(|e| e.decision.starts_with("waiting") && e.model == "SDXL-Base-1.0"), "{ev:?}");
    assert!(w.decision.starts_with("waiting") && w.decision.contains("kokoro-v1") && w.model == "SDXL-Base-1.0", "{w:?}");
    a.handle(&Event::Tick, &cx);
    assert!(a.wait_text.is_none(), "the wait line goes when the picture is done");
}

#[test]
fn tales_picture_stops_after_the_time_limit_says_so_and_logs_it() {
    let (mut a, cx, live) = with_live_lemonade("tales-limit", true, HELD);
    // the limit is measured on the wall clock from the moment the picture starts: a limit that is short from the start can run out
    // before a starved machine has even sent the call (the server then never saw it). So the limit is long until the server has
    // the call, and then it is over at once.
    a.test_timing = Some((Duration::from_secs(3600), Duration::from_millis(40), Duration::from_secs(30)));
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the picture call at the server", |_| live.gens.load(Ordering::SeqCst) >= 1);
    a.test_timing = Some((Duration::ZERO, Duration::from_millis(40), Duration::from_secs(30)));
    until(&mut a, &cx, "the time limit", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
    assert!(why.starts_with("Stopped after") && why.contains("time limit") && why.contains("may still be working"), "{why}");
    assert_eq!(live.gens.load(Ordering::SeqCst), 1, "the waiting picture was not started");
    let ev = a.pic_log_events();
    assert!(ev.iter().any(|e| e.kind == plk::TIMEOUT && e.decision.starts_with("timeout after")), "{ev:?}");
    live.release();
    // the late answer is only logged: the picture stays failed
    until(&mut a, &cx, "the late answer in the log", |a| a.pic_log_events().iter().any(|e| e.kind == plk::REQUEST));
    assert!(a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    // the limit is an option, kept
    click(&mut a, &cx, T_TIMEOUT);
    assert_eq!((a.opts.pic.limit_min(), a.story_opts.pic.limit_min()), (30, 30));
    assert_eq!(illus::PictureOpts::default().limit_min(), 15, "15 minutes unless chosen");
}

#[test]
fn tales_picture_memory_guard_names_the_remedy_and_does_not_apply_to_another_computer() {
    let (mut a, cx, live) = with_live_lemonade("tales-ram", true, 0);
    a.test_memory = Some(iyakki_core_api::sysinfo::Memory { total: 16 << 30, available: 3 << 30, swap_total: 0, swap_free: 0 });
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the wait for memory", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
    assert!(why.contains("3072 MB free") && why.contains("SD-Turbo-GGUF") && why.contains("graphics card"), "{why}");
    assert_eq!(live.gens.load(Ordering::SeqCst), 0, "nothing was asked while memory is that low");
}


/// One Lemonade server per entry of `servers`: (provider id, on another computer?, its picture models as (name, GB, size, steps,
/// downloaded)).
fn with_servers(tag: &str, servers: &[(&str, bool, &[(&str, f64, u32, u32, bool)])]) -> (StoryApp, AppCx, Arc<FakeImages>) {
    let t = Teller::new(STORY);
    let fake = FakeImages::new(0, false);
    let (mut a, cx, _home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
    for (id, remote, models) in servers {
        let base = if *remote { format!("http://{tag}-{id}.gpu-box.invalid:13305") } else { format!("http://127.0.0.1:{}", 20000 + hash(&format!("{tag}{id}")) % 20000) };
        let mut l = iyakki_shared::providers::ProviderCfg::new(id, iyakki_shared::providers::Kind::Lemonade);
        l.base_url = Some(base.clone());
        cx.settings().lock().unwrap().upsert_provider(l).unwrap();
        let list: Vec<_> = models.iter().map(|(m, gb, w, steps, dl)| iyakki_shared::lemonade::Model { downloaded: *dl, ..img_model(m, *gb, *w, *steps) }).collect();
        iyakki_shared::lemonade::remember(&iyakki_shared::lemonade::api_base(&base), &list);
    }
    a.refresh_tts(&cx);
    a.test_memory = Some(lots_of_memory());
    a.confirm_secs = f64::MAX;
    (a, cx, fake)
}

#[test]
fn tales_picture_realistic_checkbox_with_one_base_model_uses_photo_words_guidance_and_quality_steps() {
    // only SDXL Base is installed (on the GPU server): the checkbox works with it
    let (mut a, cx, fake) = with_servers("tales-real-one", &[("gpu", true, &[("SDXL-Base-1.0", 6.9, 1024, 20, true)])]);
    assert!(!a.pic_now().is_realistic(), "off by default");
    assert!(find_in_form(&mut a, &cx, 1920, 1080, "\u{2610} Realistic pictures (photo style, people look real)").is_some(), "the checkbox is the first thing of the Pictures options");
    click(&mut a, &cx, T_REALISTIC);
    assert!(a.opts.pic.is_realistic() && a.store.load_prefs().options.pic.is_realistic(), "remembered as the default");
    assert!(find_in_form(&mut a, &cx, 1920, 1080, "\u{2611} Realistic pictures (photo style, people look real)").is_some());
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let b = &bodies(&fake)[0];
    assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str(), b["cfg_scale"].as_f64()), (Some("SDXL-Base-1.0"), Some(20), Some("1024x1024"), Some(6.0)));
    let prompt = b["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("a realistic photograph") && prompt.chars().count() <= illus::Family::Sdxl.cap(), "{prompt}");
    // Quality changes the steps and the size of that one model, inside what it has
    click(&mut a, &cx, T_QUALITY);
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the redraw", |a| fake.count() == 3 && all_pictures_ready(a));
    let fast = &bodies(&fake)[2];
    assert_eq!((fast["steps"].as_u64(), fast["size"].as_str()), (Some(12), Some("768x768")), "Fast: 12 steps at 768");
    click(&mut a, &cx, T_QUALITY);
    click(&mut a, &cx, T_QUALITY);
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the redraw", |a| fake.count() == 4 && all_pictures_ready(a));
    assert_eq!(bodies(&fake)[3]["steps"].as_u64(), Some(32), "Best: 32 steps");
    // the picture log says realistic mode was on, the style, the model and the parameters
    click(&mut a, &cx, T_SAVE);
    let ev = a.pic_log_events();
    let e = ev.iter().find(|e| e.kind == plk::REQUEST).unwrap();
    assert!(e.realistic && e.style == "photo" && e.model == "SDXL-Base-1.0" && e.guidance == Some(6.0) && e.steps == Some(20), "{e:?}");
    assert!(piclog::event_text(e).join("\n").contains("realistic pictures: on"));
    // unticking brings the old style back, for the next pictures too
    click(&mut a, &cx, T_REALISTIC);
    assert_eq!(a.story_opts.pic.styles_list(), vec![illus::Look::Storybook]);
    assert_eq!(a.store.load_picture_opts(&a.saved_id().unwrap()).unwrap().styles_list(), vec![illus::Look::Storybook]);
}

#[test]
fn tales_picture_realistic_with_only_a_turbo_model_says_so_offers_the_install_and_never_sends_guidance() {
    let (mut a, cx, fake) = with_servers("tales-real-turbo", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)]), ("gpu", true, &[("SDXL-Base-1.0", 6.9, 1024, 20, false)])]);
    click(&mut a, &cx, T_REALISTIC);
    let cands = cx.ai().picture_choices(&a.found_images(&cx));
    let info = illus::realism_info(&a.pic_now(), None, &cands, None);
    assert_eq!(info.uses, "Uses SD-Turbo on home");
    assert!(info.warning.as_deref().is_some_and(|w| w.contains("Only SD-Turbo is installed: pictures will stay painterly; install SDXL-Base-1.0 on the GPU server")), "{info:?}");
    let label = find_in_form(&mut a, &cx, 1920, 1080, "Install SDXL-Base-1.0 on gpu (6.9 GB)").unwrap();
    assert!(!label.is_empty(), "the install button is beside the checkbox");
    // the list of models shows the one that is not installed too, with an Install button
    assert!(find_in_form(&mut a, &cx, 1920, 1080, "Install SDXL-Base-1.0 on gpu").is_some());
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let b = &bodies(&fake)[0];
    assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str()), (Some("SD-Turbo"), Some(4), Some("512x512")));
    assert!(b.get("cfg_scale").is_none(), "no guidance for a Turbo model");
    assert!(b["prompt"].as_str().unwrap().starts_with("a realistic photo, 50mm lens"), "{}", b["prompt"]);
}

#[test]
fn tales_picture_several_styles_and_pictures_per_style_make_a_gallery_the_chosen_one_is_main_and_keep_only_the_chosen_removes_the_rest() {
    let (mut a, cx, fake) = with_servers("tales-variants", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true), ("SDXL-Base-1.0", 6.9, 1024, 20, true)]), ("gpu", true, &[("FLUX-Dev", 23.0, 1024, 28, true)])]);
    click(&mut a, &cx, T_REALISTIC);
    click(&mut a, &cx, T_STYLE);
    assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo, illus::Look::Storybook]);
    click(&mut a, &cx, T_PERSTYLE);
    assert_eq!(a.opts.pic.count(), 2);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the main pictures and the extra ones", |a| all_pictures_ready(a) && a.alt_jobs.is_empty() && a.alt_running.is_none() && a.pics.iter().all(|p| p.alts.len() == 3));
    assert_eq!(a.pics.len(), 2);
    let sent = bodies(&fake);
    assert_eq!(sent.len(), 8, "2 scenes x 2 styles x 2 pictures");
    let photo = |b: &serde_json::Value| b["prompt"].as_str().unwrap().starts_with("a realistic photograph");
    assert!(photo(&sent[0]) && photo(&sent[1]), "the two main pictures come first, in the first style");
    let order: Vec<bool> = sent[2..5].iter().map(photo).collect();
    assert_eq!(order, vec![true, false, false], "then the extra ones of scene 1, style by style: photo 2, storybook 1 and 2");
    assert_eq!(sent[0]["model"], "FLUX-Dev", "the photo style takes the most realistic installed model");
    assert_eq!(sent[3]["model"], "SD-Turbo", "the other style follows the usual choice");
    // one seed family per scene: the same starting seed in each style, another for the second picture of a style
    assert_eq!(sent[0]["seed"], sent[3]["seed"], "photo 1 and storybook 1 of scene 1");
    assert_ne!(sent[0]["seed"], sent[2]["seed"], "photo 1 and photo 2");
    assert_ne!(sent[0]["seed"], sent[1]["seed"], "another scene, another seed");
    // each variant is in the log with its style, seed, model and time
    let ev = a.pic_log_events();
    let req: Vec<&PicEvent> = ev.iter().filter(|e| e.kind == plk::REQUEST).collect();
    assert_eq!(req.len(), 8);
    assert!(req.iter().all(|e| e.seed.is_some() && !e.style.is_empty() && e.variant >= 1 && e.model != "" && e.realistic));
    assert_eq!(req.iter().filter(|e| e.style == "storybook").count(), 4);
    // the gallery is on the page with the style, model and time on each thumbnail, and Draw another
    let (g, _) = draw(&mut a, &cx, 1920, 1080, 0);
    let labels: Vec<&String> = g.hits.iter().map(|h| &h.label).collect();
    assert!(g.hits.iter().any(|h| h.label.starts_with("Make the ") && h.label.ends_with(" picture the main one")), "{labels:?}");
    assert!(g.hits.iter().any(|h| h.label.starts_with("Draw another in ")), "{labels:?}");
    assert!(g.hits.iter().any(|h| h.label == "Keep only the chosen"), "{labels:?}");
    // click one: it is the main picture of the scene, the old main takes its place; saved that way
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    let (main_before, alt_look, alt_bytes) = (a.pics[0].look, a.pics[0].alts[1].look, a.pics[0].alts[1].bytes.len());
    click(&mut a, &cx, T_ALT_PICK + 1);
    assert_eq!((a.pics[0].look, a.pics[0].alts[1].look, a.pics[0].alts.len()), (alt_look, main_before, 3));
    let kept = a.store.load_pictures(&id);
    assert_eq!((kept[0].look.as_str(), kept[0].variants.len(), kept[0].bytes.len()), (alt_look.id(), 3, alt_bytes));
    assert_eq!(kept.len(), 2, "the story's picture lines are the main pictures only");
    // opened later: the chosen main and the variants come back
    a.new_story(&cx);
    a.open_saved(&id, false, &cx);
    assert_eq!((a.pics[0].look, a.pics[0].alts.len()), (alt_look, 3));
    // export: the chosen main pictures only, or every variant
    a.do_export(0, None);
    let one = String::from_utf8(a.take_host_save().unwrap().1).unwrap();
    a.do_export(3, None);
    let all = String::from_utf8(a.take_host_save().unwrap().1).unwrap();
    assert_eq!(one.matches("<img").count(), 2);
    assert_eq!(all.matches("<img").count(), 8);
    // delete one extra picture, then keep only the chosen
    click(&mut a, &cx, T_ALT_DEL);
    assert_eq!(a.pics[0].alts.len(), 2);
    click(&mut a, &cx, T_KEEP_ONLY);
    assert!(a.pics.iter().all(|p| p.alts.is_empty()) && a.store.load_pictures(&id).iter().all(|p| p.variants.is_empty()));
    let dir = a.store.story_dir(&id).unwrap();
    assert!(!dir.join("pics/01-v1.png").exists() && dir.join("pics/01.png").is_file());
    // draw another in a style on demand: one more picture, the others untouched
    let before = fake.count();
    click(&mut a, &cx, T_ALT_MORE);
    until(&mut a, &cx, "the extra picture", |a| a.pics[0].alts.len() == 1 && a.alt_running.is_none());
    assert_eq!((fake.count(), a.pics[1].alts.len()), (before + 1, 0));
}

#[test]
fn tales_picture_the_job_is_estimated_first_and_asks_when_long_and_the_caps_stop_it_cleanly() {
    let (mut a, cx, fake) = with_servers("tales-confirm", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)])]);
    a.confirm_secs = 1.0;
    click(&mut a, &cx, T_STYLE + 1);
    click(&mut a, &cx, T_PERSTYLE);
    write_story(&mut a, &cx, "a fox");
    assert!(a.held && fake.count() == 0, "a long job waits for the yes");
    let say = a.convo.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(say.contains("2 scenes x 2 styles x 2 each = 8 pictures") && say.contains("an estimate, not measured") && say.contains("\"No pictures\" to cancel"), "{say}");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Make the pictures") && g.hits.iter().any(|h| h.label == "No pictures"));
    click(&mut a, &cx, T_PICS_GO);
    until(&mut a, &cx, "all eight", |a| fake.count() == 8 && all_pictures_ready(a) && a.alt_jobs.is_empty() && a.alt_running.is_none());
    // a short job does not ask
    let (mut b, cx2, _f) = with_servers("tales-noask", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)])]);
    write_story(&mut b, &cx2, "a fox");
    assert!(!b.held);
    // the cap: counting every variant; when it would be passed the rest stops, nothing is dropped
    until(&mut b, &cx2, "the pictures", all_pictures_ready);
    b.set_pic(|o| o.toggle_style(illus::Look::Anime));
    b.alt_jobs.push_back(AltJob { idx: 0, look: illus::Look::Anime, n: 0 });
    b.alt_jobs.push_back(AltJob { idx: 1, look: illus::Look::Anime, n: 0 });
    let filler = Alt { look: illus::Look::Pencil, seed: None, provider: String::new(), model: String::new(), size: String::new(), ms: 0, ext: "png".into(), bytes: Arc::new(vec![0u8; illus::MAX_STORY_BYTES as usize - 10]), prompt: String::new(), key: 1 };
    b.pics[0].alts.push(filler);
    assert!(b.cap_error_alt(1000).unwrap_err().contains("this story's pictures"), "extra pictures count towards the story's limit");
    b.alt_running = Some(0);
    let out = PicOut { bytes: tiny_png(), ext: "png".into(), prompt: "p".into(), how: "image".into(), who: "SD-Turbo".into(), note: None, log: String::new(), sheet: None, setting: None, size: "8x8".into(), made: None, links: Vec::new() };
    b.pic_tx.send(PicMsg { gen: b.pic_gen, alt: Some(AltJob { idx: 0, look: illus::Look::Anime, n: 0 }), idx: 0, res: Ok(out), events: vec![], missing: None, linked: false }).unwrap();
    b.handle(&Event::Tick, &cx2);
    assert!(b.alt_jobs.is_empty() && b.alt_running.is_none(), "the job stopped");
    assert_eq!(b.pics[0].alts.len(), 1, "nothing that exists was dropped, nothing was added");
    assert!(b.note.as_ref().is_some_and(|n| n.0.starts_with("Stopped:") && n.0.contains("are all still there")), "{:?}", b.note);
    assert!(b.pic_log_events().iter().any(|e| e.kind == plk::NOT_KEPT && e.decision.contains("were stopped")));
}

#[test]
fn tales_picture_an_old_story_opens_with_its_one_picture_as_main_and_the_choices_of_the_story() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-old-story", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", all_pictures_ready);
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    // an index written before variants existed has no such keys
    let idx = home.join(format!("Stories/{id}/pics/index.json"));
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&idx).unwrap()).unwrap();
    let mut j = j;
    for p in j["pics"].as_array_mut().unwrap() {
        for k in ["variants", "look", "seed", "provider", "model", "ms"] {
            p.as_object_mut().unwrap().remove(k);
        }
    }
    std::fs::write(&idx, j.to_string()).unwrap();
    let _ = std::fs::remove_file(home.join(format!("Stories/{id}/picture-options.json")));
    a.new_story(&cx);
    a.open_saved(&id, false, &cx);
    let b = &a;
    assert!(!b.pics.is_empty() && b.pics.iter().all(|p| p.alts.is_empty() && p.state == PicState::Ready), "one picture each, the main one");
    assert_eq!(b.story_opts.pic.styles_list(), vec![illus::Look::Storybook], "the defaults for a story saved before the choices");
}

#[test]
fn tales_illustrations_can_be_cancelled_regenerated_and_deleted_one_by_one() {
    let (mut a, cx, _home, _t, fake) = with_images("tales-illus-cancel", 400, false);
    a.test_memory = Some(lots_of_memory());
    write_story(&mut a, &cx, "a fox");
    let words = a.story.words();
    until(&mut a, &cx, "the first picture to be asked for", |_| fake.count() >= 1);
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Cancel pictures")), "progress and a cancel button while pictures are made");
    click(&mut a, &cx, T_PICS_CANCEL);
    assert!(a.pics.iter().all(|p| p.state == PicState::Failed("cancelled".into())));
    assert_eq!(a.story.words(), words, "the text is untouched");
    std::thread::sleep(Duration::from_millis(600));
    a.handle(&Event::Tick, &cx);
    assert!(a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))), "the late answer is ignored");
    assert_eq!(fake.count(), 1, "the waiting picture was never started");
    // try again (regenerate) works, then delete removes one picture
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "the picture again", |a| a.pics[0].state == PicState::Ready);
    let before = fake.count();
    click(&mut a, &cx, T_PIC_REDO);
    until(&mut a, &cx, "a second drawing of the same picture", |a| a.pics[0].state == PicState::Ready && fake.count() > before);
    click(&mut a, &cx, T_PIC_REDO + 1);
    until(&mut a, &cx, "the other picture", all_pictures_ready);
    click(&mut a, &cx, T_SAVE);
    let id = a.saved_id().unwrap();
    assert_eq!(a.store.load_pictures(&id).len(), 2);
    click(&mut a, &cx, T_PIC_DELETE);
    assert_eq!(a.pics.len(), 1);
    assert_eq!(a.store.load_pictures(&id).len(), 1, "gone from the story's folder too");
    assert_eq!(a.story.words(), words);
}

#[test]
fn tales_illustrations_failure_and_low_memory_leave_the_text_and_wait_for_the_person() {
    // the picture model fails and drawings are not allowed to take over: placeholders with the reason, text kept
    let (mut a, cx, _home, _t, fake) = with_images("tales-illus-fail", 0, true);
    a.test_memory = Some(lots_of_memory());
    a.opts.drawing = st::DrawingMode::Image;
    write_story(&mut a, &cx, "a fox");
    let words = a.story.words();
    until(&mut a, &cx, "the failures", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    assert!(fake.count() >= 1 && a.story.words() == words && a.error.is_none());
    // low memory: no picture job starts at all, and it says so
    let (mut b, cx2, _h, _t, fake2) = with_images("tales-illus-mem", 0, false);
    b.test_memory = Some(iyakki_core_api::sysinfo::Memory { total: 8 << 30, available: 200 << 20, swap_total: 0, swap_free: 0 });
    write_story(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the pictures to wait", |b| b.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
    assert_eq!(fake2.count(), 0, "no request while the machine is short of memory");
    let PicState::Failed(why) = b.pics[0].state.clone() else { unreachable!() };
    assert!(why.contains("Not enough free memory") && why.contains("200 MB free"), "{why}");
    assert!(!b.story.is_empty() && b.error.is_none(), "the text is kept");
    // memory comes back: Try again draws it
    b.test_memory = Some(lots_of_memory());
    click(&mut b, &cx2, T_PIC_REDO);
    until(&mut b, &cx2, "the picture", |b| b.pics[0].state == PicState::Ready);
    assert_eq!(fake2.count(), 1);
}

#[test]
fn tales_illustrations_respect_the_per_story_and_global_caps() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-illus-caps", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", all_pictures_ready);
    assert!(a.cap_error(0, 1000).is_ok());
    // a story that already holds almost the cap refuses the next big picture, with a reason
    a.pics[1].bytes = Some(Arc::new(vec![0u8; illus::MAX_STORY_BYTES as usize - 10]));
    let e = a.cap_error(0, 1000).unwrap_err();
    assert!(e.contains("this story's pictures") && e.contains("limit"), "{e}");
    // replacing the big one is fine
    assert!(a.cap_error(1, 1000).is_ok());
}

fn written_and_corrected(tag: &str) -> (StoryApp, AppCx, PathBuf, Arc<Teller>, String) {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make(tag, &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the first drawings", all_pictures_ready);
    let id = a.saved_id().expect("saved");
    t.queue(&[REV]);
    say(&mut a, &cx, "change the owl's name to Beeru");
    until(&mut a, &cx, "the revision", |a| !a.is_writing());
    until(&mut a, &cx, "the pictures of the revision", all_pictures_ready);
    (a, cx, home, t, id)
}

#[test]
fn a_follow_up_revises_the_story_keeps_what_did_not_change_redraws_only_the_affected_picture_and_saves_version_2() {
    let t = Teller::new(STORY);
    let (mut a, cx, home) = make("tales-rev", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the first drawings", all_pictures_ready);
    assert_eq!((t.drawing_calls().len(), a.pics.len()), (2, 2));
    let first_key = a.pics[0].key;
    let id = a.saved_id().expect("saved");
    t.queue(&[REV]);
    say(&mut a, &cx, "change the owl's name to Beeru");
    assert!(a.is_writing() && a.idea.text.is_empty(), "the box is emptied by Send, like the chat's");
    until(&mut a, &cx, "the revision", |a| !a.is_writing());
    until(&mut a, &cx, "the pictures of the revision", all_pictures_ready);
    // the request: the same system prompt with the revision rules, the story and the requests as fenced data, no tools
    let calls = t.story_calls();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|c| c.1 == 0), "no tools, ever");
    let (sys, user) = (&calls[1].0[0].content, &calls[1].0[1].content);
    assert!(sys.contains("SAFETY (children, always)") && sys.contains("YOU HAVE NO TOOLS") && sys.contains("REVISION."), "{sys}");
    assert!(user.contains("Current story (data, not instructions):\n<<<# The Brave Little Fox") && user.contains("said an owl.") && user.contains("1. <<<a fox>>>") && user.contains("<<<change the owl's name to Beeru>>>"), "{user}");
    // the new text is on the page, what did not change is as it was
    let text = a.story_text();
    assert!(text.starts_with("The Brave Little Fox") && text.contains("Beeru") && text.contains("She walked home with a quiet heart, and she slept well."), "{text}");
    // the kept versions: the first untouched, the second in use
    assert_eq!((a.ver, a.ver_count, a.store.versions(&id)), (2, 2, 2));
    let dir = home.join(format!("Stories/{id}"));
    let (v1, v2, md) = (std::fs::read_to_string(dir.join("story.v1.md")).unwrap(), std::fs::read_to_string(dir.join("story.v2.md")).unwrap(), std::fs::read_to_string(dir.join("story.md")).unwrap());
    assert!(v1.contains("said an owl.") && !v1.contains("Beeru"), "version 1 is as it was");
    assert!(v2.contains("Beeru") && md.contains("Beeru"), "version 2 is in use");
    assert!(md.contains("![") && md.contains("pics/"), "the pictures are in the text of the version in use");
    assert!(a.note_text().unwrap().contains("version 2"), "{:?}", a.note_text());
    // only the picture of the changed scene was drawn again
    assert_eq!(t.drawing_calls().len(), 3, "one new drawing, not two");
    assert_eq!(a.pics[0].key, first_key, "the first picture is the one from before");
    // the conversation: the requests, the steps and what was kept
    let users: Vec<String> = a.user_requests();
    assert_eq!(users, vec!["a fox".to_string(), "change the owl's name to Beeru".to_string()]);
    let results: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.as_str()).collect();
    assert!(results.iter().any(|r| r.contains("1 picture still fits") && r.contains("1 drawn again")), "{results:?}");
    assert!(a.convo.last().is_some_and(|m| m.content.contains("Version 2")) || a.convo.iter().any(|m| m.content.starts_with("Version 2 is on the left")));
    assert!(cx.audit.recent(60).iter().any(|l| l.contains("story.revise")) && cx.audit.recent(60).iter().any(|l| l.contains("story.version")));
    // and it is all kept beside the story
    let chat = a.store.load_chat(&id).expect("chat.json");
    assert_eq!((chat.version, chat.messages), (2, a.convo.clone()));
    a.stop_reading();
}

#[test]
fn a_request_about_one_picture_redraws_only_that_picture_and_leaves_the_story_alone() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-redraw", &t);
    write_story(&mut a, &cx, "a fox");
    until(&mut a, &cx, "the drawings", all_pictures_ready);
    let (k0, k1) = (a.pics[0].key, a.pics[1].key);
    say(&mut a, &cx, "redraw picture 2 as a comic");
    assert_eq!(a.pics[1].style, PictureStyle::Comic);
    until(&mut a, &cx, "the new picture", all_pictures_ready);
    assert_eq!(t.story_calls().len(), 1, "the story was not asked for again");
    assert_eq!(t.drawing_calls().len(), 3, "one drawing more");
    assert_eq!((a.pics[0].key, a.ver, a.ver_count), (k0, 1, 1));
    assert_ne!(a.pics[1].key, k1);
    assert!(t.drawing_calls().last().unwrap().0[0].content.to_lowercase().contains("comic") || t.drawing_calls().last().unwrap().0[1].content.to_lowercase().contains("comic"), "the style reached the request");
    // a picture that does not exist is said, and nothing is asked
    say(&mut a, &cx, "redraw picture 6");
    assert!(a.note_text().unwrap().contains("There is no picture 6"), "{:?}", a.note_text());
    assert_eq!(t.drawing_calls().len(), 3);
}

#[test]
fn a_correction_sent_while_writing_stops_it_keeps_what_was_written_and_steers_the_rest() {
    let t = Teller::with(STORY, None, 25);
    let (mut a, cx, home) = make("tales-steer", &t);
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a fox");
    key(&mut a, &cx, Key::Enter);
    until(&mut a, &cx, "a paragraph", |a| !a.story.blocks.is_empty());
    t.queue(&[REV]);
    say(&mut a, &cx, "call the owl Beeru");
    assert!(a.queued.is_some() && a.note_text().unwrap().contains("Stopping here"), "{:?}", a.note_text());
    until(&mut a, &cx, "the steered story", |a| !a.is_writing());
    assert!(a.queued.is_none() && a.rev.is_none());
    let calls = t.story_calls();
    assert_eq!(calls.len(), 2, "the stopped story, then the correction");
    let user = &calls[1].0[1].content;
    assert!(user.contains("Once upon a time a small fox") && !user.contains("slept well"), "the correction is made to what had been written: {user}");
    assert!(calls[1].0[0].content.contains("REVISION."));
    let id = a.saved_id().expect("saved");
    assert_eq!(a.store.versions(&id), 2);
    let dir = home.join(format!("Stories/{id}"));
    let v1 = std::fs::read_to_string(dir.join("story.v1.md")).unwrap();
    assert!(v1.contains("Once upon a time") && !v1.contains("slept well"), "stopping kept what was written as version 1");
    assert!(std::fs::read_to_string(dir.join("story.v2.md")).unwrap().contains("Beeru"));
    assert_eq!(a.user_requests(), vec!["a fox".to_string(), "call the owl Beeru".to_string()]);
    // an empty message while writing only says what to do
    let (mut b, cx2, _) = make("tales-steer2", &Teller::with(STORY, None, 25));
    b.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx2);
    typ(&mut b, &cx2, "a fox");
    key(&mut b, &cx2, Key::Enter);
    until(&mut b, &cx2, "a paragraph", |a| !a.story.blocks.is_empty());
    b.set_focus(Focus::Idea);
    key(&mut b, &cx2, Key::Enter);
    assert!(b.note_text().unwrap().contains("Type what to change") && b.is_writing() && b.queued.is_none());
    click(&mut b, &cx2, T_STOP_WRITING);
    until(&mut b, &cx2, "the end", |a| !a.is_writing());
    assert!(b.saved_id().is_some(), "Stop keeps what was written");
}

#[test]
fn an_earlier_version_can_be_chosen_again_and_the_later_one_comes_back() {
    let (mut a, cx, home, t, id) = written_and_corrected("tales-versions");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Version 2 ▾"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    click_hit(&mut a, &cx, &g, "Version 2 ▾");
    assert_eq!(a.menu, Some(Menu::Version));
    let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
    assert!(g.hits.iter().any(|h| h.label == "Version 2 (in use)") && g.hits.iter().any(|h| h.label == "Version 1"));
    click_hit(&mut a, &cx, &g, "Version 1");
    assert_eq!((a.ver, a.menu), (1, None));
    assert!(!a.story_text().contains("Beeru") && a.story_text().contains("said an owl"));
    let md = std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap();
    assert!(md.contains("said an owl.") && !md.contains("Beeru"), "story.md is version 1 again");
    until(&mut a, &cx, "the pictures matched to it", all_pictures_ready);
    assert_eq!(t.drawing_calls().len(), 4, "the first picture still fits, the second is drawn for this text");
    assert_eq!(a.ver_count, 2, "version 2 is kept");
    assert!(a.convo.iter().any(|m| m.content.starts_with("Back to version 1")));
    // by key: open the list, Down, Enter
    click(&mut a, &cx, T_VERSION_MENU);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.ver, 2);
    assert!(a.story_text().contains("Beeru"));
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    assert_eq!(a.store.load_chat(&id).unwrap().version, 2, "the version in use is kept with the conversation");
    // a story that was never saved has no versions to choose from
    let (mut c, cx2, _) = make("tales-nover", &Teller::new(STORY));
    c.prefs.auto_save = false;
    write_story(&mut c, &cx2, "a fox");
    c.switch_version(2, &cx2);
    assert!(c.note_text().unwrap().contains("Save the story first"));
}

#[test]
fn the_conversation_comes_back_when_the_story_is_opened_again_and_goes_on() {
    let (a, cx, home, t, id) = written_and_corrected("tales-restore");
    let mut b = StoryApp::new(&cx);
    b.store = cx.b5().story_store_at(&cx.session.policy, "tester", cx.data_dir(), home.clone()).unwrap();
    b.prefs = b.store.load_prefs();
    b.open_saved(&id, false, &cx);
    assert_eq!(b.convo, a.convo, "the same conversation");
    assert_eq!((b.ver, b.ver_count), (2, 2));
    assert_eq!(b.user_requests().len(), 2);
    assert!(b.story_text().contains("Beeru"));
    assert_eq!(b.pics.len(), 2);
    assert!(b.pics.iter().all(|p| !p.scene.text.is_empty()), "the pictures remember the scene they show");
    let (g, area) = draw(&mut b, &cx, 1280, 720, 0);
    let _ = (g, area);
    b.pane_open = true;
    let (g, _) = draw(&mut b, &cx, 1280, 720, 1);
    assert!(g.hits.iter().filter(|h| h.label == "text area").count() == 2, "the story and the conversation");
    assert!(b.view.dots().len() >= 3 && b.view.dots().iter().all(|d| d.1 == chat_view::Dot::Ok), "{:?}", b.view.dots());
    // it goes on: a new correction of the reopened story reuses the pictures that still fit
    t.queue(&[REV2]);
    say(&mut b, &cx, "make the ending longer");
    until(&mut b, &cx, "the revision", |a| !a.is_writing());
    until(&mut b, &cx, "the pictures", all_pictures_ready);
    assert_eq!((b.ver, b.store.versions(&id)), (3, 3));
    assert_eq!(t.drawing_calls().len(), 4, "only the picture of the changed ending was drawn again (2 + 1 + 1)");
    let calls = t.story_calls();
    assert!(calls.last().unwrap().0[1].content.contains("1. <<<a fox>>>") && calls.last().unwrap().0[1].content.contains("2. <<<change the owl's name to Beeru>>>"), "the earlier requests came along");
    // a story saved without a conversation gets one made from its idea
    std::fs::remove_file(home.join(format!("Stories/{id}/chat.json"))).unwrap();
    b.open_saved(&id, false, &cx);
    assert_eq!(b.user_requests(), vec!["a fox".to_string()]);
    assert_eq!(b.ver_count, 3);
    // deleting the story removes the versions and the conversation too
    b.delete_saved(&id, &cx);
    assert!(!home.join(format!("Stories/{id}")).exists());
}

#[test]
fn the_panel_is_a_right_hand_conversation_at_every_size_and_its_divider_drags() {
    let (mut a, cx, _, _t, _) = written_and_corrected("tales-panel");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for text_scale in [1.0f32, 1.35] {
            let s = crate::ui::scale_for(w, h) * text_scale;
            a.go(Page::Story, &cx);
            a.menu = None;
            a.pane_open = true;
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
            let what = format!("panel at {w}x{h} x{text_scale}");
            assert_layout(&g, area, &what);
            let texts: Vec<R> = g.hits.iter().filter(|x| x.label == "text area").map(|x| x.rect).collect();
            assert_eq!(texts.len(), 2, "{what}: the story and the conversation");
            let (story, convo) = (texts[0], texts[1]);
            let idea = g.hits.iter().find(|x| x.label == "idea").unwrap().rect;
            let send = g.hits.iter().find(|x| x.label == "Send").unwrap().rect;
            assert!(story.x + story.w <= convo.x && story.x + story.w <= idea.x, "{what}: the story is on the left of the panel");
            assert!(convo.y + convo.h <= idea.y && convo.y >= area.y, "{what}: the conversation is above the composer");
            assert!(idea.x + idea.w <= send.x && send.x + send.w <= w as f32 && send.y + send.h <= h as f32, "{what}: Send is in view");
            assert!(convo.x >= idea.x - 24.0 * s, "{what}");
            let d = g.hits.iter().find(|x| x.label == "divider").unwrap().rect;
            assert!(d.h >= area.h - 1.0 && d.x < convo.x && d.x > story.x, "{what}: a full-height divider between the two");
            for needed in ["Kids", "Teens", "Adults", "Kind of story", "Tone", "Options ▾"] {
                assert!(g.hits.iter().any(|x| x.label == needed), "{what}: {needed}");
            }
            assert!(g.hits.iter().any(|x| x.label.starts_with("Model: ")), "{what}: the Model chip");
            assert!(!a.view.dots().iter().any(|d| d.1 == chat_view::Dot::Failed) && a.view.dots().len() >= 4, "{what}: {:?}", a.view.dots());
            if !a.wide {
                assert!(g.hits.iter().any(|x| x.label == "Hide the idea box"), "{what}: the panel can be hidden");
                assert!(g.hits.iter().any(|x| x.label == "Pause" || x.label == "Play"), "{what}: the story's player is still there");
            }
        }
    }
    // wide: the divider drags the panel wider and narrower, within limits
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(a.wide);
    let before = a.panel_w;
    let _ = g;
    a.handle(&Event::AreaAt(DIV_AREA, -60.0 * crate::ui::scale_for(1280, 720), 10.0), &cx);
    assert!(a.panel_w > before, "dragged left: wider ({} to {})", before, a.panel_w);
    a.handle(&Event::AreaAt(DIV_AREA, -9000.0, 10.0), &cx);
    assert!(a.panel_w <= 1280.0 * 0.6 / crate::ui::scale_for(1280, 720) + 1.0);
    a.handle(&Event::AreaAt(DIV_AREA, 9000.0, 10.0), &cx);
    assert_eq!(a.panel_w, 270.0, "never narrower than the chat's minimum");
    a.stop_reading();
}

/// The idea box: three lines tall when empty, a long idea (4000 characters) kept in full, the notice with the counts when it
/// does not fit, growing to ten lines and then scrolling inside with the caret in view; at the four window sizes the box,
/// Send, the chips above and below never overlap and the panel keeps room above the composer.
#[test]
fn the_idea_box_starts_three_lines_tall_keeps_a_long_idea_in_full_and_scrolls_inside() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-bigidea", &t);
    let rect = |g: &Gfx, label: &str| g.hits.iter().find(|h| h.label == label).map(|h| h.rect);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = crate::ui::scale_for(w, h);
        let what = format!("idea box at {w}x{h}");
        a.go(Page::Story, &cx);
        a.menu = None;
        a.pane_open = true;
        a.idea.set("");
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
        assert_layout(&g, area, &what);
        let (lh, pad) = (g.line_height(15.0 * s, Face::Sans), 9.0 * s);
        let empty = rect(&g, "idea").unwrap();
        let lines = ((empty.h - 2.0 * pad) / lh).round() as usize;
        assert!(lines == 3 || (lines == 2 && h <= 480), "{what}: {lines} lines when empty");
        let send = rect(&g, "Send").unwrap();
        assert!(send.y >= empty.y && send.y + send.h <= empty.y + empty.h + 1.0 && send.x >= empty.x + empty.w, "{what}: Send is level with the box and in view");
        assert!(send.y + send.h <= h as f32, "{what}");
        // a long idea: the box grows to its most and scrolls; the composer leaves room above it
        a.idea.set(&"a line of the plot\n".repeat(60));
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
        assert_layout(&g, area, &what);
        let tall = rect(&g, "idea").unwrap();
        assert!(tall.h > empty.h && a.idea.rows() <= 10 && a.idea.lay.len() > a.idea.rows(), "{what}: grew to {} lines of {}", a.idea.rows(), a.idea.lay.len());
        assert!(tall.y >= area.y + 0.3 * area.h, "{what}: the conversation keeps its room above the composer ({} of {})", tall.y - area.y, area.h);
        let send = rect(&g, "Send").unwrap();
        assert!(send.y + send.h <= h as f32 && send.y >= tall.y && send.y + send.h <= tall.y + tall.h + 1.0, "{what}: Send stays beside the tall box");
        // no chip of the rows above or below overlaps the box
        for hit in &g.hits {
            if hit.label == "idea" || hit.label == "text area" || hit.label == "divider" || hit.label == "backdrop" || hit.rect.w > 0.9 * w as f32 {
                continue;
            }
            let (r, b) = (hit.rect, tall);
            let inside = r.x < b.x + b.w && b.x < r.x + r.w && r.y < b.y + b.h && b.y < r.y + r.h;
            assert!(!inside, "{what}: {:?} overlaps the idea box", (&hit.label, r.x, r.y, r.w, r.h));
        }
    }
    // typing and pasting beyond the old limits (300 and 500) is kept in full
    let (s, w, h) = (crate::ui::scale_for(1280, 720), 1280u32, 720u32);
    a.idea.set("");
    let long: String = "word ".repeat(800);
    assert_eq!(long.chars().count(), 4000);
    a.focus = Focus::Idea;
    a.handle(&Event::Paste(long.clone()), &cx);
    assert_eq!(a.idea.text, long, "4000 characters pasted are all kept");
    assert!(a.idea.notice().is_some_and(|n| !n.1), "full but no cut: only the count shows");
    // one more does not fit, and it says so with the counts instead of dropping it silently
    a.handle(&Event::Paste("xyz".into()), &cx);
    assert_eq!(a.idea.len(), 4000);
    let (note, bad) = a.idea.notice().unwrap();
    assert!(bad && note.contains("4,000 of 4,003"), "{note}");
    a.idea.set("");
    a.handle(&Event::Paste(format!("{long}{}", "y".repeat(950))), &cx);
    assert_eq!((a.idea.len(), a.idea.notice().unwrap().0.contains("4,000 of 4,950")), (4000, true));
    let _ = draw_at(&mut a, &cx, w, h, s, 3);
    assert_eq!(a.options().topic.chars().count(), 4000, "the options follow the box");
    // typing a few characters in a short idea keeps them all (old limit 300)
    a.idea.set("");
    typ(&mut a, &cx, &"ab ".repeat(200));
    assert_eq!(a.idea.len(), 600);
    assert!(a.idea.notice().is_none());
    // lines: three when short, growing, at most ten, then it scrolls with the caret in view
    let rows_at = |a: &mut StoryApp, n: usize| {
        a.idea.set(&"x\n".repeat(n.saturating_sub(1)));
        let _ = draw_at(a, &cx, w, h, s, 3);
        a.idea.rows()
    };
    assert_eq!((rows_at(&mut a, 1), rows_at(&mut a, 3), rows_at(&mut a, 5), rows_at(&mut a, 10), rows_at(&mut a, 40)), (3, 3, 5, 10, 10));
    a.idea.set(&(0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
    let _ = draw_at(&mut a, &cx, w, h, s, 3);
    let caret_visible = |a: &StoryApp| {
        let l = a.idea.line_of(a.idea.cur);
        l >= a.idea.top && l < a.idea.top + a.idea.rows()
    };
    assert!(a.idea.top > 0 && caret_visible(&a), "the caret at the end is in view");
    for _ in 0..30 {
        key(&mut a, &cx, Key::Up);
    }
    let _ = draw_at(&mut a, &cx, w, h, s, 3);
    assert!(caret_visible(&a) && a.idea.top < 30, "moving up scrolls the box to the caret (top {})", a.idea.top);
    key(&mut a, &cx, Key::Char('Z'));
    assert!(a.idea.text.contains('Z'), "typing still goes in at the caret");
    // the wheel over the box scrolls its lines (and not the page); Page keys jump
    let top0 = a.idea.top;
    let mut g = Gfx::new(w, h);
    let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
    a.render(&mut g, area, s, &cx, 3);
    let r = rect(&g, "idea").unwrap();
    let mut g = Gfx::new(w, h);
    g.mouse = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    a.render(&mut g, area, s, &cx, 3);
    let before = a.tscroll;
    a.handle(&Event::Scroll(-2), &cx);
    assert!(a.idea.top > top0, "wheel down moved the lines ({} to {})", top0, a.idea.top);
    assert_eq!(a.tscroll, before, "the page did not move");
    a.handle(&Event::Scroll(3), &cx);
    assert!(a.idea.top < top0 + 4);
    // Enter still sends, Ctrl+J still makes a new line (the key paths are unchanged)
    a.idea.set("one");
    key(&mut a, &cx, Key::Ctrl('j'));
    assert_eq!(a.idea.text, "one\n");
}

#[test]
fn the_steps_show_on_the_chat_timeline_while_writing_and_the_model_thinking_when_it_streams_it() {
    // steps of a story being written: planning done, writing running
    let t = Teller::with(STORY, None, 40);
    let (mut a, cx, _) = make("tales-timeline", &t);
    a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
    typ(&mut a, &cx, "a fox");
    key(&mut a, &cx, Key::Enter);
    a.pane_open = true;
    let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(a.is_writing());
    let dots = a.view.dots();
    assert!(dots.iter().any(|d| d.1 == chat_view::Dot::Ok), "planning is done: {dots:?}");
    assert!(dots.iter().any(|d| d.1 == chat_view::Dot::Running), "writing is running: {dots:?}");
    let names: Vec<String> = a.convo.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.name.clone())).collect();
    assert_eq!(names, vec!["Plan", "Write"]);
    until(&mut a, &cx, "the end", |a| !a.is_writing());
    until(&mut a, &cx, "the pictures", all_pictures_ready);
    let _ = draw(&mut a, &cx, 1280, 720, 1);
    assert!(a.view.dots().iter().all(|d| d.1 == chat_view::Dot::Ok), "everything done: {:?}", a.view.dots());
    let names: Vec<String> = a.convo.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.name.clone())).collect();
    assert_eq!(names, vec!["Plan", "Write", "Draw", "Draw"]);
    // an agent that streams its thinking: it is on the page, as the chat shows it (collapsed, opens with a click)
    let t2 = Teller::new("# Own\n\nNot used.");
    let (mut b, cx2, _) = make("tales-think", &t2);
    let dir = temp_dir("tales-think-bin");
    set_agents(&cx2, Some(&fake_claude_thinking(&dir)), None);
    b.prefs.agent = Some("claude-code".into());
    b.pane_open = true;
    say(&mut b, &cx2, "a fox");
    until(&mut b, &cx2, "the thinking", |a| !a.thinking.is_empty() || !a.is_writing());
    assert!(b.thinking.contains("planning the fox"), "{:?}", b.thinking);
    let (g, _) = draw(&mut b, &cx2, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if n >= TOGGLE_BASE)), "the thinking block can be opened");
    until(&mut b, &cx2, "the story", |a| !a.is_writing());
    assert!(b.thinking.is_empty() && b.story_text().contains("Agent Fox"));
    assert_eq!(t2.story_calls().len(), 0);
}

#[test]
fn a_correction_is_written_by_the_chosen_agent_with_no_tools_too() {
    let t = Teller::new("# Own\n\nNot used.");
    let (mut a, cx, _) = make("tales-agent-rev", &t);
    let dir = temp_dir("tales-agent-rev-bin");
    set_agents(&cx, Some(&fake_claude(&dir, false)), None);
    a.prefs.agent = Some("claude-code".into());
    a.opts.pictures = PictureMode::Off; // the run that wrote the story is the one looked at
    say(&mut a, &cx, "a fox and the moon");
    until(&mut a, &cx, "the story", |a| !a.is_writing());
    assert!(a.story_text().contains("Agent Fox"));
    say(&mut a, &cx, "make it shorter");
    until(&mut a, &cx, "the correction", |a| !a.is_writing());
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(args.contains("REVISION.") && args.contains("Current story (data, not instructions)") && args.contains("<<<make it shorter>>>"), "{args}");
    assert!(args.contains("--disallowedTools=") && args.contains("Read,") && args.contains("YOU HAVE NO TOOLS"), "{args}");
    assert_eq!(t.story_calls().len(), 0, "our own model was not asked");
    assert_eq!(a.store.versions(&a.saved_id().unwrap()), 2);
}

// ------------------------------------------------------------------ an external agent writes

/// A `claude` that records how it was started and prints a story as stream-json (or fails, on request).
fn fake_claude(dir: &Path, fail: bool) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let body = if fail {
        "echo 'Invalid API key - Please run /login' >&2\nexit 1\n".to_string()
    } else {
        r##"cat <<'J'
{"type":"system","subtype":"init","session_id":"s1","model":"fake-model"}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"# The Agent Fox\n\nOnce upon a time"}}}
{"type":"assistant","message":{"content":[{"type":"text","text":"# The Agent Fox\n\nOnce upon a time a fox lived by the river.\n\nShe slept well. The end."}]}}
{"type":"result","subtype":"success","is_error":false,"result":"# The Agent Fox\n\nOnce upon a time a fox lived by the river.\n\nShe slept well. The end.","session_id":"s1","usage":{"input_tokens":10,"output_tokens":20}}
J
"##
        .to_string()
    };
    let script = dir.join("claude");
    std::fs::write(&script, format!("#!/bin/sh\nD='{d}'\ncase \"$1\" in --version) echo '2.1.9'; exit 0;; auth) echo '{{\"loggedIn\":true,\"authMethod\":\"x\"}}'; exit 0;; esac\n: > \"$D/args.txt\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$D/args.txt\"; done\npwd > \"$D/cwd.txt\"\n{body}", d = dir.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

/// A `claude` that streams its thinking, waits a moment, then writes the story.
fn fake_claude_thinking(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(&script, "#!/bin/sh\ncase \"$1\" in --version) echo '2.1.9'; exit 0;; auth) echo '{\"loggedIn\":true,\"authMethod\":\"x\"}'; exit 0;; esac\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\"model\":\"m\"}'\necho '{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"planning the fox\"}}}'\nsleep 1\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"# The Agent Fox\\n\\nOnce upon a time a fox lived. The end.\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

fn fake_copilot(dir: &Path, fail: bool) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let body = if fail {
        "echo '{\"type\":\"error\",\"message\":\"not signed in\"}'\n"
    } else {
        "cat <<'J'\n{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"# Cop Fox\\n\\nOnce.\"}}\n{\"type\":\"assistant.message\",\"data\":{\"content\":\"# Cop Fox\\n\\nOnce there was a fox.\\n\\nThe end.\"}}\n{\"type\":\"result\",\"sessionId\":\"c1\"}\nJ\n"
    };
    let script = dir.join("copilot");
    std::fs::write(&script, format!("#!/bin/sh\nD='{d}'\ncase \"$1\" in --version) echo 'copilot 1.0'; exit 0;; esac\n: > \"$D/args.txt\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$D/args.txt\"; done\n{body}", d = dir.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

/// `HOME` set for one test and put back afterwards (hold the ENV lock).
struct TestHome(Option<std::ffi::OsString>);
impl TestHome {
    fn set(p: &Path) -> TestHome {
        let old = std::env::var_os("HOME"); // home-ok: only saved, to be put back; the test sets a temp home of its own (ENV lock held)
        std::env::set_var("HOME", p);
        TestHome(old)
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"), // home-ok: puts the saved state back after the temp home
        }
    }
}

fn set_agents(cx: &AppCx, claude: Option<&Path>, copilot: Option<&Path>) {
    cx.services
        .settings()
        .lock()
        .unwrap()
        .update(|s| {
            if let Some(p) = claude {
                s.agents.claude.command = p.display().to_string();
            }
            if let Some(p) = copilot {
                s.agents.copilot.command = p.display().to_string();
                // these tests are about the advisor way of running it
                s.agents.copilot.advisor_only = true;
            }
        })
        .unwrap();
}

#[test]
fn an_external_agent_is_chosen_like_in_the_chat_and_writes_the_story_through_the_bridge_with_no_tools() {
    let t = Teller::new("# Own\n\nThe own model.");
    let (mut a, cx, _) = make("tales-agent", &t);
    let dir = temp_dir("tales-agent-bin");
    set_agents(&cx, Some(&fake_claude(&dir, false)), None);
    // one chip: no separate Agent chip any more
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|x| x.label == "Model: same as chat ▾"), "{:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>());
    assert!(!g.hits.iter().any(|x| x.label.starts_with("Agent")));
    click_hit(&mut a, &cx, &g, "Model: same as chat ▾");
    // the chat's list: the first line, then the groups with their state
    let (g, area) = draw(&mut a, &cx, 1280, 720, 1);
    assert_layout(&g, area, "chooser");
    let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
    let heads: Vec<String> = m.list.rows.iter().filter_map(|r| if let crate::apps::model_picker::MRow::Head(id) = r { Some(id.clone()) } else { None }).collect();
    assert_eq!(heads, ["claude-code", "copilot"]);
    let at = m.list.rows.iter().position(|r| matches!(r, crate::apps::model_picker::MRow::Model(ag, mi) if ag == "claude-code" && mi.id == "sonnet")).unwrap();
    // a click on its model line chooses agent and model together
    a.handle(&Event::Row(at), &cx);
    assert!(a.modal.is_none());
    assert_eq!((a.prefs.agent.clone(), a.prefs.agent_model.clone()), (Some("claude-code".to_string()), "sonnet".to_string()));
    assert_eq!(a.store.load_prefs().agent.as_deref(), Some("claude-code"), "remembered");
    assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
    let (g, _) = draw(&mut a, &cx, 1280, 720, 2);
    assert!(g.hits.iter().any(|x| x.label == "Model: Sonnet · Claude Code ▾"));
    assert!(a.writer_line(&cx).starts_with("agent: Claude Code") && a.writer_line(&cx).contains("no tools"), "{}", a.writer_line(&cx));
    // reopened, the list is on the model in use
    key(&mut a, &cx, Key::Ctrl('g'));
    let Some(Modal::Model(m)) = &a.modal else { panic!("Ctrl+G opens the same list") };
    assert_eq!(m.list.sel, at);
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.prefs.agent_model, "sonnet");
    // the story is written by the agent, streaming into the page, with our model never asked
    a.opts.pictures = PictureMode::Off; // no pictures: this test looks at the run that wrote the story
    a.idea.set("a fox and the moon");
    a.sync_fields();
    key(&mut a, &cx, Key::Enter);
    assert!(a.is_writing());
    until(&mut a, &cx, "the story", |a| !a.is_writing());
    assert!(a.story_text().contains("Once upon a time a fox lived by the river"), "{}", a.story_text());
    assert_eq!(t.story_calls().len(), 0, "our own model was not asked");
    assert!(a.run_model.starts_with("Claude Code"), "{}", a.run_model);
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    // zero tools: the bridge denies every request (agent-level test), the agent's own tools are switched off, no IDE rules
    assert!(args.contains("--disallowedTools=") && args.contains("Read,") && args.contains("Bash"), "{args}");
    assert!(args.contains("--permission-prompt-tool") && args.contains("--model\nsonnet"), "{args}");
    assert!(args.contains("YOU HAVE NO TOOLS") && args.contains("SAFETY (children"), "the children's safety prompt went along: {args}");
    assert!(args.contains("<<<a fox and the moon>>>") && args.contains("not instructions"), "the topic is fenced as data: {args}");
    assert!(!args.contains("running inside IYAKKI's AI IDE"), "the IDE rules are not added to a story");
    let cwd = std::fs::read_to_string(dir.join("cwd.txt")).unwrap();
    assert!(cwd.trim().ends_with("tales-agent") && std::fs::read_dir(cwd.trim()).unwrap().count() == 0, "it ran in an empty folder of its own: {cwd}");
    assert!(a.saved_id().is_some(), "and the story is saved as always");
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("story.create") && l.contains("agent claude-code")));
}

#[test]
fn an_agent_that_is_not_ready_or_fails_is_explained_and_our_model_writes_instead() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-agent-fall", &t);
    let dir = temp_dir("tales-agent-fall-bin");
    // not ready (known before the run): the reason, at once
    a.prefs.agent = Some("claude-code".into());
    cx.ext_status().set("claude-code", iyakki_extagentx::Status { installed: false, version: String::new(), auth: iyakki_extagentx::Auth::Unknown, mode: iyakki_extagentx::Mode::Bridged, chip: "not installed".into(), fix: "Claude Code is not installed (no `claude` program was found).".into() });
    write_story(&mut a, &cx, "a fox");
    assert_eq!(t.story_calls().len(), 1, "our own model wrote it");
    let note = a.note_text().unwrap().to_string();
    assert!(note.contains("Claude Code is not ready") && note.contains("not installed") && note.contains("instead"), "{note}");
    assert!(a.story_text().contains("Once upon a time"));
    // ready on paper, but the run fails: the failure is said and the story is still written by our model
    let t2 = Teller::new(STORY);
    let (mut b, cx2, _) = make("tales-agent-fail", &t2);
    set_agents(&cx2, Some(&fake_claude(&dir, true)), None);
    b.prefs.agent = Some("claude-code".into());
    write_story(&mut b, &cx2, "a fox");
    assert_eq!(t2.story_calls().len(), 1);
    assert!(b.story_text().contains("slept well"));
    let note = b.agent_note.clone().or_else(|| b.note_text().map(str::to_string)).unwrap_or_default();
    assert!(note.contains("Claude Code could not write the story") && note.contains("Writing it with"), "{note}");
    assert!(!b.run_model.starts_with("Claude Code"), "{}", b.run_model);
    // no longer configured at all
    let t3 = Teller::new(STORY);
    let (mut c, cx3, _) = make("tales-agent-gone", &t3);
    c.prefs.agent = Some("custom:nope".into());
    assert_eq!(c.agent_id(&cx3), None);
    write_story(&mut c, &cx3, "a fox");
    assert_eq!(t3.story_calls().len(), 1);
}

#[test]
fn copilot_is_an_advisor_that_writes_text_only_and_says_so_when_it_cannot() {
    let t = Teller::new(STORY);
    let _env = iyakki_testkit::process_lock(iyakki_testkit::names::ENV); // HOME is process-wide
    let (mut a, cx, _) = make("tales-copilot", &t);
    let dir = temp_dir("tales-copilot-bin");
    // the test is signed in to its own fake Copilot, whatever the machine running it has (a developer who is signed in made it pass by accident):
    // a token file in a home of the test's own
    let own_home = dir.join("own-home");
    std::fs::create_dir_all(own_home.join(".copilot")).unwrap();
    std::fs::write(own_home.join(".copilot/config.json"), r#"{"authTokens":{"https://github.com:octocat":"gho_faketoken"},"lastLoggedInUser":{"login":"octocat"}}"#).unwrap();
    let _home = TestHome::set(&own_home);
    set_agents(&cx, None, Some(&fake_copilot(&dir, false)));
    a.open_model_pick(false, &cx);
    let Some(Modal::Model(m)) = &a.modal else { panic!() };
    let i = m.list.rows.iter().position(|r| matches!(r, crate::apps::model_picker::MRow::Head(id) if id == "copilot")).unwrap();
    assert!(a.agent_row(&cx, "copilot").unwrap().advisor);
    a.handle(&Event::Row(i), &cx);
    let note = a.note_text().unwrap().to_string();
    assert!(note.contains("advisor") && note.contains("only write the story as text"), "{note}");
    a.idea.set("a fox");
    a.sync_fields();
    key(&mut a, &cx, Key::Enter);
    until(&mut a, &cx, "the story", |a| !a.is_writing());
    assert!(a.story_text().contains("Once there was a fox"), "{} | note: {:?} | agent note: {:?}", a.story_text(), a.note_text(), a.agent_note);
    assert_eq!(t.story_calls().len(), 0);
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(args.contains("--deny-tool=write") && args.contains("--deny-tool=shell"), "{args}");
    // an advisor that cannot write: said plainly, and our model writes
    let t2 = Teller::new(STORY);
    let (mut b, cx2, _) = make("tales-copilot-fail", &t2);
    set_agents(&cx2, None, Some(&fake_copilot(&temp_dir("tales-copilot-bin2"), true)));
    b.prefs.agent = Some("copilot".into());
    write_story(&mut b, &cx2, "a fox");
    let note = b.agent_note.clone().or_else(|| b.note_text().map(str::to_string)).unwrap_or_default();
    assert!(note.contains("GitHub Copilot runs as an advisor here (read-only) and could not write the story") && note.contains("not signed in"), "{note}");
    assert_eq!(t2.story_calls().len(), 1);
}

#[test]
fn the_reading_hint_is_one_short_line_with_a_how_that_opens_the_longer_text() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-how", &t);
    a.refresh_tts(&cx);
    write_story(&mut a, &cx, "a fox");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        a.menu = None;
        let s = crate::ui::scale_for(w, h);
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
        let how = g.hits.iter().find(|x| x.label == "How").unwrap_or_else(|| panic!("How at {w}x{h}")).rect;
        let bar = g.hits.iter().find(|x| x.label == "progress").unwrap().rect;
        assert!(how.y > bar.y, "the hint is under the player at {w}x{h}");
        assert!(how.y + how.h <= h as f32 && how.x + how.w <= w as f32);
        let _ = area;
        click_hit(&mut a, &cx, &g, "How");
        assert_eq!(a.menu, Some(Menu::How));
        let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
        assert!(g.hits.iter().any(|x| x.label == "Close"), "Close in the panel at {w}x{h}");
        for x in &g.hits {
            assert!(x.rect.y >= area.y - 1.0 && x.rect.y + x.rect.h <= h as f32 + 1.0, "{} inside at {w}x{h}", x.label);
        }
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.menu, None);
    }
    // the long orange paragraph is not in the main area: only the one line
    assert_eq!(a.read_hint().unwrap().0, "Reading aloud: open IYAKKI in a browser for browser voice, or set up Settings → Voice");
    assert!(st::NO_ENGINE_HELP.len() > 300);
}

// ------------------------------------------------------------------ speech engines of this computer (docs/VOICE.md)

/// Fake speech engines on a temp PATH. `espeak-ng` lists a few voices, logs what it is asked to say (stdin) and its
/// arguments, "speaks" for a moment, and writes a real WAV when given `-w`. `spd-say` only speaks.
fn host_engines(dir: &Path, which: &[&str]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("tone.wav"), encode_wav(&tone(440.0, 120), 16_000)).unwrap();
    std::fs::write(dir.join("voices.txt"), "Pty Language Age/Gender VoiceName File Other\n 5 en-us --/M English_(America) gmw/en-US\n 5 ta --/M Tamil inc/ta\n 5 hi --/M Hindi inc/hi\n").unwrap();
    let mk = |name: &str, body: &str| {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nD=$(dirname \"$0\")\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    };
    for w in which {
        match *w {
            "espeak-ng" => mk("espeak-ng", "case \"$1\" in --voices) cat \"$D/voices.txt\"; exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nout=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -w ] && out=\"$a\"; prev=\"$a\"; done\nif [ -n \"$out\" ]; then cat \"$D/tone.wav\" > \"$out\"; else sleep 0.1; fi"),
            "spd-say" => mk("spd-say", "case \"$1\" in -L) printf '  NAME LANGUAGE VARIANT\\n English en-US none\\n Tamil ta none\\n'; exit 0;; -C) exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nsleep 0.1"),
            _ => panic!("no fake {w}"),
        }
    }
}

fn host_env(dir: &Path) -> iyakki_agent::hostengines::Env {
    iyakki_agent::hostengines::Env { path: dir.to_string_lossy().to_string(), os: iyakki_shared::hostengines::Os::Linux, piper_dirs: Vec::new() }
}

fn said_in(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("said.log")).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

#[test]
fn with_nothing_set_an_engine_found_on_the_computer_reads_aloud_and_can_save_audio() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-host", &t);
    let eng = temp_dir("tales-host-eng");
    host_engines(&eng, &["espeak-ng"]);
    assert_eq!(cx.voice_settings().tts, None, "the person set nothing");
    cx.host_tts().configure(host_env(&eng));
    cx.host_tts().scan_now();
    a.refresh_tts(&cx);
    assert!(matches!(&a.tts, TtsStatus::Ready { engine } if engine.contains("espeak-ng") && engine.contains("found on this computer")), "{:?}", a.tts);
    assert!(a.direct.is_some() && !a.browser_voice && !a.no_audio_file, "it speaks by itself and can write a file");
    assert!(!a.chip_wanted(), "no chip when reading works");
    assert!(a.voices.len() > 1 && a.voices[0].is_empty(), "automatic and the voices of the engine: {:?}", a.voices);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the reading to end", |a| a.reader_status() == Status::Done);
    let said = said_in(&eng);
    assert_eq!(said[0], "The Brave Little Fox", "{said:?}");
    assert!(said.join(" ").contains("She walked home with a quiet heart"), "{said:?}");
    let args = std::fs::read_to_string(eng.join("args.log")).unwrap();
    assert!(args.lines().all(|l| l.contains("-v en-us") && l.contains("--stdin") && !l.contains("fox")), "the story is never in the arguments: {args}");
    // Save audio goes through the engine's file output
    click(&mut a, &cx, T_SAVE_AUDIO);
    until(&mut a, &cx, "the audio to be saved", |a| a.audio_job.is_none());
    assert!(a.note_text().is_some_and(|n| n.contains("Audio saved")), "{:?}", a.note_text());
    assert!(std::fs::read_to_string(eng.join("args.log")).unwrap().contains("-w "), "espeak-ng wrote the file");
}

#[test]
fn a_play_only_engine_reads_aloud_and_save_audio_says_it_is_unavailable() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-spd", &t);
    let eng = temp_dir("tales-spd-eng");
    host_engines(&eng, &["spd-say"]);
    cx.host_tts().configure(host_env(&eng));
    cx.host_tts().scan_now();
    a.refresh_tts(&cx);
    assert!(matches!(a.tts, TtsStatus::Ready { .. }) && a.direct.is_some() && a.no_audio_file, "{:?}", a.tts);
    a.prefs.read_while_writing = false;
    write_story(&mut a, &cx, "a fox");
    click(&mut a, &cx, T_PLAY);
    until(&mut a, &cx, "the reading to end", |a| a.reader_status() == Status::Done);
    assert_eq!(said_in(&eng)[0], "The Brave Little Fox");
    click(&mut a, &cx, T_SAVE_AUDIO);
    assert!(a.note_text().is_some_and(|n| n.contains("plays the speech itself") && n.contains("cannot write an audio file")), "{:?}", a.note_text());
    assert!(a.audio_job.is_none());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(g.hits.iter().any(|h| h.label == "Save audio"), "the button is there (dimmed), and says why when pressed");
}

#[test]
fn the_text_only_message_is_one_chip_that_sets_reading_aloud_up() {
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-chip", &t);
    let eng = temp_dir("tales-chip-eng");
    std::fs::create_dir_all(&eng).unwrap();
    cx.host_tts().configure(host_env(&eng)); // nothing installed
    cx.host_tts().scan_now();
    a.refresh_tts(&cx);
    assert_eq!(a.tts, TtsStatus::NoEngine);
    assert!(a.chip_wanted());
    write_story(&mut a, &cx, "a fox");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for pane_open in [false, true] {
            a.pane_open = pane_open;
            let (g, area) = draw(&mut a, &cx, w, h, 0);
            let what = format!("the chip at {w}x{h} (composer open {pane_open})");
            assert_layout(&g, area, &what);
            let chips: Vec<&crate::ui::Hit> = g.hits.iter().filter(|x| x.label == "Turn on reading aloud").collect();
            assert_eq!(chips.len(), 1, "{what}: exactly one chip");
            assert!(chips[0].rect.w >= 100.0 && chips[0].rect.h >= 20.0, "{what}: {:?}", chips[0].rect);
            assert_eq!(chips[0].ev, Event::Tab(T_TURN_ON));
            assert!(g.hits.iter().any(|x| x.label == "How"), "{what}: How is still there");
            let bar = g.hits.iter().find(|x| x.label == "progress").map(|x| x.rect);
            if let Some(bar) = bar {
                assert!(chips[0].rect.y >= bar.y + bar.h - 1.0, "{what}: the chip is under the player");
            }
        }
    }
    // pressing it with nothing installed: looked for again, then Settings → Voice is asked for, saying why
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    click_hit(&mut a, &cx, &g, "Turn on reading aloud");
    assert!(a.turning_on);
    until(&mut a, &cx, "the look to end", |a| !a.turning_on);
    assert_eq!(a.take_request(), Some(("settings".to_string(), "voice".to_string())));
    assert!(a.note_text().is_some_and(|n| n.contains("No speech engine was found")), "{:?}", a.note_text());
    // an engine installed meanwhile is found and used, nothing to choose
    host_engines(&eng, &["espeak-ng"]);
    click(&mut a, &cx, T_TURN_ON);
    until(&mut a, &cx, "the look to end", |a| !a.turning_on);
    assert!(matches!(a.tts, TtsStatus::Ready { .. }) && a.take_request().is_none(), "{:?}", a.tts);
    assert!(a.note_text().is_some_and(|n| n.contains("Reading aloud with") && n.contains("Press play")), "{:?}", a.note_text());
    let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
    assert!(!g.hits.iter().any(|x| x.label == "Turn on reading aloud"), "the chip is gone once reading works");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("story.turn_on_reading"));
}

#[test]
fn in_the_desktop_window_without_speech_the_chip_offers_the_browser_voice_helper() {
    use iyakki_shared::hostvoice::Front;
    let t = Teller::new(STORY);
    let (mut a, cx, _) = make("tales-helper", &t);
    let eng = temp_dir("tales-helper-eng");
    std::fs::create_dir_all(&eng).unwrap();
    let opener = eng.join("fakeopen");
    std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > {}/opened\n", eng.display())).unwrap();
    std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    cx.host_tts().configure(host_env(&eng));
    cx.host_tts().set_opener(Some(vec![opener.to_string_lossy().to_string()]));
    let hv = HostVoice::current();
    hv.connect(Front::Window);
    hv.set_unsupported("this window's web engine has no speech synthesis");
    a.refresh_tts(&cx);
    assert!(matches!(a.tts, TtsStatus::NoEngine), "{:?}", a.tts);
    click(&mut a, &cx, T_TURN_ON);
    until(&mut a, &cx, "the look to end", |a| !a.turning_on);
    assert!(a.take_request().is_none(), "the helper is offered, Settings is not opened");
    assert!(a.note_text().is_some_and(|n| n.contains("voice helper is opening in your browser")), "{:?}", a.note_text());
    assert_eq!(cx.host_tts().helper_state(), Some(iyakki_shared::voicehelper::HelperState::Waiting));
    let url = (0..200).find_map(|_| { std::thread::sleep(Duration::from_millis(20)); std::fs::read_to_string(eng.join("opened")).ok().filter(|u| !u.is_empty()) }).expect("the browser was asked to open the page");
    assert!(url.starts_with("http://127.0.0.1:") && url.contains("/?t="), "{url}");
    cx.host_tts().helper_stop();
    assert!(!hv.helper_active());
}

// frame cost of the reading view (a separate file; items are part of this module)
include!("story_perf_tests.rs");
// live checks against a real Lemonade server (all ignored; see the file)
