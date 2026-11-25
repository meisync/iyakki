//! The TALES tile on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no `Services`, no session. The
//! story library is the real store in a folder of its own, the chat model, the speech engines of the computer, the picture models, the
//! agents and the audit log are in-memory fakes, and the time is a manual clock (`iyakki_testkit::ManualClock`), so a saved story carries
//! the date the test chose. What a thread of the tile does is waited for by looking at the tile, never by sleeping for a guess.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use iyakki_story_model::DirectVoice;
use iyakki_voice_model::hosttts::HostTtsApi;
use iyakki_voice_model::hostengines::{Env, Report};
use iyakki_voice_model::voicehelper::HelperState;
use iyakki_testkit::ManualClock;

use super::*;
use crate::port::TalesWorker;

/// A fresh folder for one test (the process id and the tag keep tests apart; the folder is left for the system's temp cleaning).
fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tales-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A model that streams one fixed story in small pieces and answers every other request with a fixed line.
struct Teller {
    text: String,
    asked: Mutex<Vec<String>>,
}

impl ChatBackend for Teller {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        self.asked.lock().unwrap().push(h[0].content.clone());
        Ok("A small fox sits by a quiet river.".into())
    }
    fn complete_tools_stream(&self, h: &[Message], _tools: &[iyakki_chat_model::ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<iyakki_chat_model::ModelTurn, String> {
        self.asked.lock().unwrap().push(h[0].content.clone());
        let chars: Vec<char> = self.text.chars().collect();
        for piece in chars.chunks(17) {
            let p: String = piece.iter().collect();
            if !on_text(&p) {
                break;
            }
        }
        Ok(iyakki_chat_model::ModelTurn { text: self.text.clone(), calls: vec![] })
    }
}

#[derive(Default)]
struct FakeTts {
    enabled: bool,
    scanning: Mutex<bool>,
    ensured: AtomicUsize,
    rechecked: AtomicUsize,
}

impl HostTtsApi for FakeTts {
    fn is_enabled(&self) -> bool {
        self.enabled
    }
    fn configure(&self, _env: Env) {}
    fn set_opener(&self, _opener: Option<Vec<String>>) {}
    fn ensure(&self) {
        self.ensured.fetch_add(1, Ordering::SeqCst);
    }
    fn recheck(&self) {
        self.rechecked.fetch_add(1, Ordering::SeqCst);
        *self.scanning.lock().unwrap() = true;
    }
    fn report(&self) -> Option<std::sync::Arc<Report>> {
        None
    }
    fn scanning(&self) -> bool {
        *self.scanning.lock().unwrap()
    }
    fn scan_now(&self) -> std::sync::Arc<Report> {
        std::sync::Arc::new(Report::empty())
    }
    fn helper_open(&self, _hv: std::sync::Arc<HostVoice>, _opener: Option<&[String]>) -> Result<String, String> {
        Err("no browser here".into())
    }
    fn helper_state(&self) -> Option<HelperState> {
        None
    }
    fn helper_stop(&self) {}
    fn auto_engine(&self, _lang: Option<&str>) -> Option<SpeechEngine> {
        None
    }
}

/// Everything the tile asks for, answered in memory.
struct FakePort {
    home: PathBuf,
    policy: iyakki_sec::policy::Policy,
    clock: ManualClock,
    teller: Arc<Teller>,
    tts: Arc<FakeTts>,
    audit: Arc<Mutex<Vec<(String, String)>>>,
    admin: bool,
    commands: bool,
    pdf_asked: Mutex<Vec<(usize, i64)>>,
    pulled: Mutex<Vec<(String, String)>>,
    engines_set: Mutex<Vec<String>>,
    images: Vec<(String, String)>,
    link: Option<(usize, String)>,
    wants_pdf: bool,
}

impl FakePort {
    fn new(tag: &str, story: &str) -> FakePort {
        let dir = temp_dir(tag);
        let home = std::fs::canonicalize({
            std::fs::create_dir_all(dir.join("home")).unwrap();
            dir.join("home")
        })
        .unwrap();
        let mut policy = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::User, &Default::default());
        policy.fs_read = vec![home.to_string_lossy().into_owned()];
        policy.fs_write = vec![home.to_string_lossy().into_owned()];
        FakePort {
            home,
            policy,
            clock: ManualClock::new(1_700_000_000_000),
            teller: Arc::new(Teller { text: story.into(), asked: Mutex::new(vec![]) }),
            tts: Arc::new(FakeTts::default()),
            audit: Arc::new(Mutex::new(vec![])),
            admin: false,
            commands: false,
            pdf_asked: Mutex::new(vec![]),
            pulled: Mutex::new(vec![]),
            engines_set: Mutex::new(vec![]),
            images: vec![],
            link: None,
            wants_pdf: false,
        }
    }
    fn audited(&self) -> Vec<String> {
        self.audit.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
    }
}

struct FakeWorker {
    clock: ManualClock,
}

impl TalesWorker for FakeWorker {
    fn provider_is_lemonade(&self, _provider: &str) -> bool {
        false
    }
    fn lemonade_live_model(&self, _provider: &str, _model: &str) -> Result<Option<iyakki_provider_kind::lemonade::Model>, String> {
        Ok(None)
    }
    fn lemonade_health(&self, _provider: &str) -> Result<(String, Vec<String>), String> {
        Ok(("ok".into(), vec![]))
    }
    fn generate_image(&self, _model: Option<(&str, &str)>, _prompt: &str, _size: &str, _params: &iyakki_provider_kind::ImageParams) -> crate::port::ImageOut {
        (Err("the fake draws nothing".into()), Default::default())
    }
    fn start_agent(&self, _req: AgentRunReq) -> Box<dyn iyakki_story_kit::RunFeed> {
        struct Failing;
        impl iyakki_story_kit::RunFeed for Failing {
            fn recv(&self, _wait: Duration) -> Result<iyakki_story_kit::RunMsg, std::sync::mpsc::RecvTimeoutError> {
                Ok(iyakki_story_kit::RunMsg::Failed("the fake runs no agent".into()))
            }
            fn cancel(&self) {}
            fn deny(&self, _id: &str) {}
        }
        Box::new(Failing)
    }
    fn fetch_link(&self, _url: &str, _allow_video: bool) -> Result<crate::port::LinkedPic, String> {
        let _ = self.clock.now_secs();
        Err("the fake fetches nothing".into())
    }
}

struct NoSink;
impl iyakki_story_kit::Sink for NoSink {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn iyakki_story_kit::Playing>, String> {
        Err("the fake plays nothing".into())
    }
}

impl TalesPort for FakePort {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn may_run_commands(&self) -> bool {
        self.commands
    }
    fn voice_allowed(&self) -> bool {
        true
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn audit_fn(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let log = self.audit.clone();
        Arc::new(move |a, s| log.lock().unwrap().push((a.to_string(), s.to_string())))
    }
    fn usage_fn(&self) -> Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync> {
        Arc::new(|_, _, _, _| {})
    }
    fn clock(&self) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        {
            let c = self.clock.clone();
            Arc::new(move || c.now_secs())
        }
    }
    fn data_dir(&self) -> PathBuf {
        self.home.parent().unwrap().to_path_buf()
    }
    fn rewording(&self) -> Arc<dyn Fn(&str) -> String + Send + Sync> {
        Arc::new(|m| m.to_string())
    }
    fn memory(&self) -> Option<iyakki_sysinfo_model::Memory> {
        None
    }
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader> {
        Arc::new(iyakki_story_model::NoReader)
    }
    fn story_store(&self) -> StoryStore {
        let roots = iyakki_filescope::FileRoots { home: self.home.clone(), roots: vec![self.home.clone()], mounts: vec![] };
        StoryStore::open_with(&self.policy, "tester", self.home.clone(), roots)
    }
    fn story_pdf(&self, _story: &Story, _opts: &StoryOptions, pics: &[iyakki_story_kit::StoredPic], now: i64) -> Result<iyakki_story_kit::StoryPdf, String> {
        self.pdf_asked.lock().unwrap().push((pics.len(), now));
        Ok(iyakki_story_kit::StoryPdf { bytes: b"%PDF-fake".to_vec(), pages: 1, pictures: 0, left_out: 0 })
    }
    fn asks_for_pdf(&self, _text: &str) -> bool {
        self.wants_pdf
    }
    fn asks_for_link(&self, _text: &str, _scenes: usize) -> Option<(usize, String)> {
        self.link.clone()
    }
    fn chat(&self) -> Arc<dyn ChatBackend> {
        self.teller.clone()
    }
    fn chat_backend_for(&self, _choice: ModelChoice) -> Arc<dyn ChatBackend> {
        self.teller.clone()
    }
    fn chat_choice(&self) -> Option<ModelChoice> {
        None
    }
    fn text_models(&self) -> Vec<(String, String)> {
        vec![("local".into(), "tiny".into())]
    }
    fn quick_engines(&self, _path: &str, _voice_dirs: &[PathBuf]) -> Vec<st::QuickEngine> {
        vec![]
    }
    fn set_speech_engine(&self, engine: SpeechEngine) -> Result<(), String> {
        self.engines_set.lock().unwrap().push(format!("{engine:?}"));
        Ok(())
    }
    fn voice_settings(&self) -> iyakki_voice_model::voice::VoiceSettings {
        Default::default()
    }
    fn model_list(&self, _plain: Vec<(String, String)>, _top: Option<model_picker::TopRow>, _agents: bool, _current: Current, _title: &str, _hint: &str) -> Option<ModelList> {
        None
    }
    fn model_draw(&self, _g: &mut Gfx, _area: R, _scale: f32, _list: &mut ModelList) {}
    fn model_remember_custom(&self, _agent: &str, _model: &str) {}
    fn agent_entry(&self, _id: &str) -> Option<AgentEntry> {
        None
    }
    fn agent_known(&self, _id: &str) -> bool {
        false
    }
    fn agent_label(&self, _id: &str) -> Option<String> {
        None
    }
    fn agent_advisor(&self, _id: &str) -> bool {
        false
    }
    fn agent_check(&self, _id: &str) -> Result<(), String> {
        Err("the fake has no agents".into())
    }
    fn agent_model_label(&self, _id: &str, model: &str) -> String {
        model.to_string()
    }
    fn agent_key_ref(&self, _id: &str) -> String {
        String::new()
    }
    fn resolve_secret(&self, key_ref: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Err(format!("no secret {key_ref} in the fake vault"))
    }
    fn scene_agent_chat(&self, _req: &AgentRunReq) -> Option<Arc<dyn ChatBackend>> {
        None
    }
    fn image_models(&self) -> Vec<(String, String)> {
        self.images.clone()
    }
    fn picture_choices(&self, _found: &[(String, String)]) -> Vec<iyakki_story_kit::storyillus::Cand> {
        vec![]
    }
    fn cpu_only(&self) -> bool {
        true
    }
    fn gpu_lemonade(&self) -> Option<String> {
        None
    }
    fn lemonade_images_missing(&self) -> Vec<iyakki_provider_kind::lemonade::Model> {
        vec![]
    }
    fn lemonade_model(&self, _provider: &str, _model: &str) -> Option<iyakki_provider_kind::lemonade::Model> {
        None
    }
    fn provider_is_lemonade(&self, _provider: &str) -> bool {
        false
    }
    fn pull_status(&self) -> Vec<iyakki_provider_kind::PullStatus> {
        vec![]
    }
    fn pull_model(&self, provider: &str, model: &str) -> Result<(), String> {
        self.pulled.lock().unwrap().push((provider.to_string(), model.to_string()));
        Ok(())
    }
    fn host_tts(&self) -> Arc<dyn HostTtsApi> {
        self.tts.clone()
    }
    fn direct_voice(&self, _engine: &SpeechEngine) -> Option<Arc<dyn DirectVoice>> {
        None
    }
    fn speaks_directly(&self, _engine: &SpeechEngine) -> bool {
        false
    }
    fn can_save_audio(&self, _engine: &SpeechEngine) -> bool {
        true
    }
    fn player_sink(&self, _configured: &str, _path: &str) -> Arc<dyn iyakki_story_kit::Sink> {
        Arc::new(NoSink)
    }
    fn reading_synth(&self, _opts: SpeakOpts) -> Arc<dyn iyakki_story_kit::Synth> {
        Arc::new(iyakki_story_kit::PrebuiltSynth::new(&[]))
    }
    fn worker(&self) -> Arc<dyn TalesWorker> {
        Arc::new(FakeWorker { clock: self.clock.clone() })
    }
}

const STORY: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nShe walked home with a quiet heart, and she slept well.\n";

/// Run the tile's heartbeat until `done` holds (the writer thread's messages arrive on a channel).
fn until(app: &mut StoryApp, port: &FakePort, what: &str, done: impl Fn(&StoryApp) -> bool) {
    iyakki_testkit::wait::wait_until(Duration::from_secs(30), || {
        app.poll(port);
        let _ = what;
        done(app)
    });
}

#[test]
fn a_new_tile_reads_the_library_and_the_speech_engines_through_the_port() {
    let port = FakePort::new("tales-port-new", STORY);
    let app = StoryApp::with_port(&port);
    assert!(app.library_ids().is_empty(), "an empty library to begin with");
    assert!(matches!(app.tts_status(), TtsStatus::NoEngine), "{:?}", app.tts_status());
    assert!(port.tts.ensured.load(Ordering::SeqCst) >= 1, "the tile asks the computer's engines to be looked for");
    assert!(app.image_models.is_empty(), "the fake has no picture model");
}

#[test]
fn a_story_is_written_by_the_ports_chat_model_and_saved_with_the_ports_clock() {
    let port = FakePort::new("tales-port-write", STORY);
    let mut app = StoryApp::with_port(&port);
    app.prefs.auto_save = true;
    app.idea.set("a small fox who is afraid of the dark");
    app.submit(&port);
    assert!(app.is_writing(), "the model is asked at once");
    until(&mut app, &port, "the story to be written", |a| !a.is_writing());
    assert!(app.story_text().contains("Brave Little Fox"), "{:?}", app.story_text());
    let asked = port.teller.asked.lock().unwrap().clone();
    assert!(asked.iter().any(|m| m.contains("IYAKKI Tales")), "the port's chat model got the request: {asked:?}");
    let ids = app.store.list();
    assert_eq!(ids.len(), 1, "the story is saved ({:?})", app.error_text());
    let saved = app.store.list();
    assert_eq!(saved[0].created, 1_700_000_000, "the date is the manual clock's, not the wall clock's");
    let audited = port.audited();
    assert!(audited.contains(&"story.create".to_string()) && audited.contains(&"story.save".to_string()), "{audited:?}");
}

#[test]
fn the_pdf_is_made_by_the_port_at_the_ports_time_and_saved_beside_the_story() {
    let port = FakePort::new("tales-port-pdf", STORY);
    let mut app = StoryApp::with_port(&port);
    app.prefs.auto_save = true;
    app.idea.set("a fox");
    app.submit(&port);
    until(&mut app, &port, "the story", |a| !a.is_writing() && a.saved_id().is_some());
    // the pictures are drawn by a background thread: let it finish, so the number the PDF gets is the number drawn
    until(&mut app, &port, "the drawings", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))));
    let drawn = app.pics.iter().filter(|p| p.state == PicState::Ready).count();
    port.clock.advance(Duration::from_secs(86_400));
    app.save_pdf(&port, None);
    assert_eq!(*port.pdf_asked.lock().unwrap(), vec![(drawn, 1_700_086_400)], "asked once, at the manual clock's time, with the pictures that were drawn");
    let id = app.saved_id().expect("saved");
    assert_eq!(std::fs::read(app.store.story_dir(&id).unwrap().join("story.pdf")).unwrap(), b"%PDF-fake");
    assert!(port.audited().contains(&"story.pdf".to_string()));
}

#[test]
fn a_message_that_asks_for_a_pdf_or_a_picture_link_is_recognised_by_the_port_not_by_the_tile() {
    let mut port = FakePort::new("tales-port-ask", STORY);
    let mut app = StoryApp::with_port(&port);
    app.prefs.auto_save = true;
    app.idea.set("a fox");
    app.submit(&port);
    until(&mut app, &port, "the story", |a| !a.is_writing() && a.saved_id().is_some());
    // the port says the next message asks for a link: the tile only asks the person first
    port.link = Some((1, "https://example.org/fox.png".into()));
    app.idea.set("use https://example.org/fox.png for picture 2");
    app.submit(&port);
    assert!(matches!(app.modal, Some(Modal::FetchLink(1, ref u)) if u == "https://example.org/fox.png"), "{}", app.modal.is_some());
    assert!(!app.is_writing(), "no model call is made for it");
    app.modal = None;
    // and for a PDF the same writer as the button runs
    port.link = None;
    port.wants_pdf = true;
    app.idea.set("make it a PDF");
    app.submit(&port);
    assert_eq!(port.pdf_asked.lock().unwrap().len(), 1);
    assert!(app.note_text().is_some_and(|n| n.contains("story.pdf")), "{:?}", app.note_text());
}

#[test]
fn choosing_a_model_keeps_the_choice_and_writes_one_audit_line_through_the_port() {
    let port = FakePort::new("tales-port-model", STORY);
    let mut app = StoryApp::with_port(&port);
    app.choose_model(model_picker::Pick::Provider("local".into(), "tiny".into()), false, &port);
    assert_eq!(app.prefs.model.as_ref().map(|m| (m.provider.as_str(), m.model.as_str())), Some(("local", "tiny")));
    assert_eq!(app.store.load_prefs().model.map(|m| m.model), Some("tiny".to_string()), "kept in the library's own prefs file");
    assert_eq!(*port.audit.lock().unwrap(), vec![("story.model".to_string(), "local tiny".to_string())]);
    app.choose_model(model_picker::Pick::Top, false, &port);
    assert!(app.prefs.model.is_none());
    assert_eq!(port.audit.lock().unwrap().last().unwrap(), &("story.model".to_string(), "same as chat".to_string()));
}

#[test]
fn an_agent_is_refused_in_plain_words_when_the_account_may_not_run_commands() {
    let port = FakePort::new("tales-port-agent", STORY);
    let mut app = StoryApp::with_port(&port);
    // the account may not run commands: the tile asks the port and says so, and keeps the old choice
    app.choose_agent("claude", String::new(), false, &port);
    assert!(app.prefs.agent.is_none());
    // an agent the settings do not know is not offered a run either
    app.prefs.agent = Some("claude".into());
    let r = app.agent_request(&port, &[]);
    assert!(r.is_err_and(|e| e.contains("not set up any more")), "the fake has no agent configured");
}

#[test]
fn the_one_click_voice_goes_to_the_ports_settings_and_is_audited() {
    let mut port = FakePort::new("tales-port-quick", STORY);
    port.admin = true;
    let mut app = StoryApp::with_port(&port);
    app.quick = vec![st::QuickEngine { label: "Use espeak-ng".into(), engine: SpeechEngine::Host { engine: "espeak-ng".into(), program: None, voice: None } }];
    app.apply_quick(0, &port);
    assert_eq!(port.engines_set.lock().unwrap().len(), 1, "the engine was set through the port");
    assert!(port.audit.lock().unwrap().contains(&("story.engine".to_string(), "Use espeak-ng".to_string())));
    assert!(app.note_text().is_some_and(|n| n.contains("is now the voice")), "{:?}", app.note_text());
}

#[test]
fn turning_reading_on_asks_the_ports_engines_and_goes_to_settings_when_they_are_off() {
    let port = FakePort::new("tales-port-turnon", STORY);
    let mut app = StoryApp::with_port(&port);
    // the computer's engine detection is off (a desktop without it): Settings -> Voice is where it is set up
    app.turn_on_reading(&port);
    assert_eq!(app.take_request(), Some(("settings".into(), "voice".into())));
    assert!(port.audited().contains(&"story.turn_on_reading".to_string()));
    assert_eq!(port.tts.rechecked.load(Ordering::SeqCst), 0, "nothing was scanned");
    // with detection on, the tile rechecks and waits for the scan to end
    let mut port = FakePort::new("tales-port-turnon2", STORY);
    port.tts = Arc::new(FakeTts { enabled: true, ..Default::default() });
    let mut app = StoryApp::with_port(&port);
    app.turn_on_reading(&port);
    assert_eq!(port.tts.rechecked.load(Ordering::SeqCst), 1);
    app.poll(&port);
    assert!(app.turning_on, "still scanning: the tile waits");
    *port.tts.scanning.lock().unwrap() = false;
    app.poll(&port);
    assert!(!app.turning_on, "the scan ended");
    assert_eq!(app.take_request(), Some(("settings".into(), "voice".into())), "no engine was found");
}

#[test]
fn a_picture_model_that_is_not_on_the_server_is_installed_through_the_port() {
    let port = FakePort::new("tales-port-install", STORY);
    let mut app = StoryApp::with_port(&port);
    app.install = Some(Install { provider: "lemonade".into(), model: "sd-turbo".into(), size: "2 GB".into(), started: false });
    app.start_install(&port);
    assert_eq!(*port.pulled.lock().unwrap(), vec![("lemonade".to_string(), "sd-turbo".to_string())]);
    assert!(app.install.as_ref().is_some_and(|i| i.started));
    assert!(port.audited().contains(&"story.picture_install".to_string()));
    // the log line of it carries the manual clock's stamp
    assert!(app.pic_log_events().iter().any(|e| e.ts == 1_700_000_000), "{:?}", app.pic_log_events().iter().map(|e| e.ts).collect::<Vec<_>>());
}
