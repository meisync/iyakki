//! The VOICE tile on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no `Services`, no session, no agent.
//! The recorder, the dictation desk, the wording corrector, the wake counts and the audit log are in-memory fakes, the files are a folder of the
//! test's own. The old tests of the voice files run through the real port.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_story_model::DirectVoice;
use iyakki_voice_model::facets::{MineWorker, NoSpeechKit, PendingWaker, SpeechKit, VoiceDeskFacet};
use iyakki_voice_model::dictfix::{self, Fixed, Reason, Target};
use iyakki_voice_model::voice::{SpeechEngine, VoiceSettings};
use iyakki_voice_model::voicedesk::{Capture, Mic, Recorders};

use super::*;
use crate::voice_mine::{T_MINE_FALSE, T_MINE_MUTE, T_MINE_OFF, T_MINE_ON, T_MINE_STREAM_YES};
use crate::port::{Fixer, VoicePort};

/// A fresh folder for one test (the process id and the tag keep tests apart).
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-voice-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Cap;

impl Capture for Cap {
    fn level(&self) -> f32 {
        0.5
    }
    fn elapsed(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn is_full(&self) -> bool {
        false
    }
    fn is_alive(&mut self) -> bool {
        true
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        Ok(iyakki_voice_model::voice::tone(300.0, 1200))
    }
}

#[derive(Default)]
struct Recs {
    started: AtomicUsize,
}

impl Recorders for Recs {
    fn mics(&self) -> Vec<Mic> {
        vec![Mic { id: "plughw:1,0".into(), label: "USB Audio (USB)".into(), usb: true }]
    }
    fn product(&self) -> String {
        "IYAKKI test".into()
    }
    fn start(&self, _cmd: &str, _max_secs: usize) -> Result<Box<dyn Capture>, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Cap))
    }
}

/// The dictation desk: one speech engine and a fixed transcript.
struct Desk {
    engine: Option<SpeechEngine>,
    said: String,
}

impl VoiceDeskFacet for Desk {
    fn stt_engine(&self) -> Option<SpeechEngine> {
        self.engine.clone()
    }
    fn transcribe(&self, _engine: &SpeechEngine, _pcm: &[i16]) -> Result<String, String> {
        Ok(self.said.clone())
    }
}

struct Fake {
    voice_allowed: bool,
    admin: bool,
    settings: Mutex<VoiceSettings>,
    audit: Mutex<Vec<(String, String)>>,
    desk: Arc<Desk>,
    target: Mutex<Target>,
    fix_enabled: AtomicBool,
    marks: Mutex<Vec<String>>,
    stream: Mutex<Vec<bool>>,
    session_request: AtomicBool,
    home: std::path::PathBuf,
    mic: Mutex<String>,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        Fake {
            voice_allowed: true,
            admin: true,
            settings: Mutex::new(VoiceSettings::default()),
            audit: Mutex::new(vec![]),
            desk: Arc::new(Desk { engine: Some(SpeechEngine::Command { command: "stt {wav}".into() }), said: "hello world".into() }),
            target: Mutex::new(Target::NoModel),
            fix_enabled: AtomicBool::new(false),
            marks: Mutex::new(vec![]),
            stream: Mutex::new(vec![]),
            session_request: AtomicBool::new(false),
            home: std::fs::canonicalize(temp_dir(tag)).unwrap(),
            mic: Mutex::new(String::new()),
        }
    }
    fn audited(&self, action: &str) -> Vec<String> {
        self.audit.lock().unwrap().iter().filter(|a| a.0 == action).map(|a| a.1.clone()).collect()
    }
}

impl VoicePort for Fake {
    fn tile_recording(&self) -> Arc<dyn Fn(bool) + Send + Sync> {
        Arc::new(|_| {})
    }
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader> {
        Arc::new(iyakki_story_model::NoReader)
    }
    fn speaks_directly(&self, _engine: &SpeechEngine) -> bool {
        false
    }
    fn give_saved(&self, _path: &std::path::Path) {}
    fn stamp(&self) -> String {
        "20231114-221320".into()
    }
    fn no_mic_text(&self, product: &str, t2_hidden: bool) -> String {
        iyakki_voice_model::voicedesk::no_mic_text_with(product, t2_hidden, false)
    }
    fn record_cmd_for(&self, base: &str, mic: Option<&Mic>) -> String {
        iyakki_voice_model::voicedesk::record_cmd_for_with(base, mic, &|_| None)
    }
    fn reword_storage_error(&self, msg: &str) -> String {
        msg.to_string()
    }
    fn user(&self) -> String {
        "tester".into()
    }
    fn voice_allowed(&self) -> bool {
        self.voice_allowed
    }
    fn manage_settings(&self) -> bool {
        self.admin
    }
    fn allows_write(&self, path: &str) -> bool {
        std::path::Path::new(path).starts_with(&self.home)
    }
    fn record(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push((action.into(), detail.into()));
    }
    fn voice_settings(&self) -> VoiceSettings {
        self.settings.lock().unwrap().clone()
    }
    fn update_voice(&self, f: &mut dyn FnMut(&mut VoiceSettings)) -> Result<(), String> {
        f(&mut self.settings.lock().unwrap());
        Ok(())
    }
    fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        *self.mic.lock().unwrap() = id.into();
        Ok(())
    }
    fn voicedesk(&self) -> Arc<dyn VoiceDeskFacet> {
        self.desk.clone()
    }
    fn direct_voice(&self, _engine: &SpeechEngine) -> Option<Arc<dyn DirectVoice>> {
        None
    }
    fn player_command(&self, configured: &str, _path: &str) -> Option<String> {
        Some(configured.to_string())
    }
    fn proc_root(&self) -> std::path::PathBuf {
        "/nonexistent-proc".into()
    }
    fn dict_target(&self) -> Target {
        self.target.lock().unwrap().clone()
    }
    fn dict_set_enabled(&self, on: bool) -> Result<(), String> {
        self.fix_enabled.store(on, Ordering::SeqCst);
        self.update_voice(&mut |v| v.fix_wording = on)
    }
    fn dict_fixer(&self) -> Fixer {
        Arc::new(|raw, _ctx| Fixed { text: raw.to_uppercase(), raw: raw.to_string(), changed: true, kept_because: None, needs_consent: None, reason: Reason::Changed })
    }
    fn speech_kit(&self) -> Arc<dyn SpeechKit> {
        Arc::new(NoSpeechKit)
    }
    fn allow_speech_server(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }
    fn home_root(&self) -> std::path::PathBuf {
        self.home.clone()
    }
    fn file_roots(&self) -> iyakki_filescope::FileRoots {
        iyakki_filescope::FileRoots { home: self.home.clone(), roots: vec![self.home.clone()], mounts: vec![] }
    }
    fn check_read(&self, path: &std::path::Path) -> Result<(), String> {
        if path.starts_with(&self.home) {
            Ok(())
        } else {
            Err("outside the allowed folders".into())
        }
    }
    fn take_session_request(&self) -> bool {
        self.session_request.swap(false, Ordering::SeqCst)
    }
    fn wake_mark(&self, kind: &str) -> Result<(), String> {
        self.marks.lock().unwrap().push(kind.into());
        Ok(())
    }
    fn wake_stats_line(&self) -> String {
        format!("{} marks", self.marks.lock().unwrap().len())
    }
    fn cloud_stream_sync(&self) {}
    fn cloud_stream_press(&self, yes: bool) {
        self.stream.lock().unwrap().push(yes);
    }
    fn cloud_stream_card(&self) -> Option<String> {
        None
    }
    fn pending_waker(&self) -> Option<Box<dyn PendingWaker>> {
        None
    }
    fn spawn_assistant(&self, _recorders: Arc<dyn Recorders>) -> Option<Box<dyn MineWorker>> {
        None
    }
}

fn app(f: &Fake) -> (VoiceApp, Arc<Recs>) {
    let recs = Arc::new(Recs::default());
    let rig = Rig { recorders: recs.clone(), sink: None, output: Arc::new(|| Ok(())) };
    (VoiceApp::with_port(f, rig), recs)
}

fn status(a: &VoiceApp) -> String {
    a.status_text().map(|s| s.0.to_string()).unwrap_or_default()
}

#[test]
fn without_the_voice_right_a_read_is_refused_and_audited() {
    let mut f = Fake::new("vf-denied");
    f.voice_allowed = false;
    let (mut a, _) = app(&f);
    assert!(!a.allowed(&f, "read"));
    assert_eq!(f.audited("voice.denied"), vec!["read".to_string()]);
    assert_eq!(status(&a), "Voice is not enabled for your account.");
}

#[test]
fn keep_listening_and_the_wording_fix_switch_through_the_port_and_are_audited() {
    let f = Fake::new("vf-switch");
    let (mut a, _) = app(&f);
    a.click(T_KEEP, &f);
    assert!(f.voice_settings().keep_listening);
    assert_eq!(f.audited("settings.voice.keep_listening"), vec!["on".to_string()]);
    a.click(T_FIX, &f);
    assert!(f.fix_enabled.load(Ordering::SeqCst));
    assert_eq!(f.audited(dictfix::AUDIT), vec!["on".to_string()]);
    assert!(status(&a).contains("no chat model is set up"), "no model: the tile says it does nothing yet");
    *f.target.lock().unwrap() = Target::NoModel;
    a.click(T_FIX, &f);
    assert!(!f.fix_enabled.load(Ordering::SeqCst));
    assert!(status(&a).starts_with("Correct my wording is off"));
}

#[test]
fn dictation_goes_through_the_desk_and_lands_in_the_box() {
    let f = Fake::new("vf-dictate");
    let (mut a, recs) = app(&f);
    a.start_record(&f);
    assert_eq!(recs.started.load(Ordering::SeqCst), 1, "the microphone opens on the person's press, through the rig");
    assert!(f.audited("voice.record").len() == 1);
    a.stop_record(&f, true);
    iyakki_testkit::wait_until(Duration::from_secs(10), || {
        a.on_tick(&f);
        a.transcript.text.contains("hello world")
    });
    assert_eq!(a.transcript.text, "hello world", "the fix is off: the transcript is exactly what was heard");
}

#[test]
fn without_a_speech_engine_dictation_says_so() {
    let mut f = Fake::new("vf-noengine");
    f.desk = Arc::new(Desk { engine: None, said: String::new() });
    let (mut a, _) = app(&f);
    a.start_record(&f);
    a.stop_record(&f, true);
    assert!(status(&a).contains("No speech-to-text is set up") || a.transcript.text.is_empty());
}

#[test]
fn a_chunk_is_tidied_by_the_fixer_the_port_hands_out() {
    let f = Fake::new("vf-fix");
    let (mut a, _) = app(&f);
    a.add_chunk(&f, "hello there".into(), true);
    assert_eq!(a.transcript.text, "hello there", "the raw words are in the box at once");
    iyakki_testkit::wait_until(Duration::from_secs(10), || {
        a.fix_tick(&f);
        a.transcript.text == "HELLO THERE"
    });
    assert_eq!(f.audited(dictfix::AUDIT), vec![Reason::Changed.label().to_string()]);
}

#[test]
fn the_wake_counts_and_the_cloud_card_answer_go_to_the_port_and_the_log() {
    let f = Fake::new("vf-wake");
    let (mut a, _) = app(&f);
    a.switch(Area::Mine, &f);
    a.click(T_MINE_FALSE, &f);
    assert_eq!(*f.marks.lock().unwrap(), vec!["false_wake".to_string()]);
    assert_eq!(f.audited("voice.wake"), vec!["marked false-wake".to_string()]);
    a.click(T_MINE_STREAM_YES, &f);
    assert_eq!(*f.stream.lock().unwrap(), vec![true]);
    assert_eq!(f.audited("voice.cloud_stream"), vec!["card: allowed for this session".to_string()]);
}

#[test]
fn my_iyakki_is_off_by_default_and_only_an_administrator_turns_it_on() {
    let mut f = Fake::new("vf-mine");
    f.admin = false;
    let (mut a, _) = app(&f);
    a.switch(Area::Mine, &f);
    assert!(!f.voice_settings().my_on, "off by default");
    a.click(T_MINE_ON, &f);
    assert!(!f.voice_settings().my_on);
    assert!(a.mine.note.contains("Only an administrator"));
    let mut g = Fake::new("vf-mine2");
    g.admin = true;
    let (mut b, _) = app(&g);
    b.switch(Area::Mine, &g);
    b.click(T_MINE_ON, &g);
    assert!(g.voice_settings().my_on);
    assert_eq!(g.audited("voice.assistant"), vec!["on".to_string()]);
    b.click(T_MINE_MUTE, &g);
    assert!(g.voice_settings().my_muted);
    b.click(T_MINE_OFF, &g);
    assert!(!g.voice_settings().my_on);
}

#[test]
fn a_file_is_read_only_inside_the_folders_the_port_names() {
    let f = Fake::new("vf-file");
    std::fs::write(f.home.join("note.txt"), "read me aloud").unwrap();
    let (mut a, _) = app(&f);
    a.click(T_LOAD, &f);
    a.path.set("note.txt");
    a.key(&Key::Enter, &f);
    assert_eq!(a.text.text, "read me aloud");
    assert_eq!(f.audited("voice.load").len(), 1);
    a.click(T_LOAD, &f);
    a.path.set("/etc/hostname");
    a.key(&Key::Enter, &f);
    assert_eq!(a.text.text, "read me aloud");
    assert!(f.audited("voice.load").len() == 1, "nothing outside the roots is read or logged as read");
}

#[test]
fn a_wake_request_opens_the_assistant_tab_only_when_it_is_on() {
    let f = Fake::new("vf-session");
    let (mut a, _) = app(&f);
    f.session_request.store(true, Ordering::SeqCst);
    a.mine_tick(&f);
    assert_ne!(a.area(), Area::Mine, "off: the request changes nothing");
    f.update_voice(&mut |v| v.my_on = true).unwrap();
    f.session_request.store(true, Ordering::SeqCst);
    a.mine_tick(&f);
    assert_eq!(a.area(), Area::Mine);
}
