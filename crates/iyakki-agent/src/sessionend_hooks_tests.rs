//! Conformance of the end-of-session hooks (D-SESSION-END): every registered kind stops on a sign-out, with fakes and no sleeps.

use crate::media::{Launcher, MediaPlayer, Running};
use crate::services::Services;
use crate::sessionend_hooks::{register_all, register_with, Owners, Stops, REGISTERED};
use crate::storykit::ReaderGroup;
use iyakki_proxy::{StreamHead, Streamer, Upstream, UpstreamResponse};
use iyakki_shared::sessionend::{Ctx, SessionEnd, Why};
use iyakki_shared::story::{Chunk, Playing, Sink, Status, Synth};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Dead;
impl Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// A station that never answers: the radio stays in a non-idle state until it is stopped.
struct Refuse;
impl Streamer for Refuse {
    fn open(&self, _: &str, _: &[(String, String)]) -> Result<StreamHead, String> {
        Err("connection refused".into())
    }
}

struct FakeRun(Arc<AtomicBool>);
impl Running for FakeRun {
    fn alive(&mut self) -> bool {
        !self.0.load(Ordering::SeqCst)
    }
    fn stop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct FakeLauncher(Mutex<Vec<Arc<AtomicBool>>>);
impl Launcher for FakeLauncher {
    fn launch(&self, _: &str) -> Result<Box<dyn Running>, String> {
        let f = Arc::new(AtomicBool::new(false));
        self.0.lock().unwrap().push(f.clone());
        Ok(Box::new(FakeRun(f)))
    }
}

struct Wav;
impl Synth for Wav {
    fn synth(&self, _: &str) -> Result<Vec<u8>, String> {
        Ok(vec![0; 64])
    }
}
struct Loud;
struct LoudPlaying(bool);
impl Playing for LoudPlaying {
    fn is_playing(&mut self) -> bool {
        !self.0
    }
    fn stop(&mut self) {
        self.0 = true;
    }
}
impl Sink for Loud {
    fn play(&self, _: &[u8]) -> Result<Box<dyn Playing>, String> {
        Ok(Box::new(LoudPlaying(false)))
    }
}

fn svc(tag: &str) -> Arc<Services> {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-sessend-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap()
}

fn ctx(user: &str, why: Why) -> Ctx {
    Ctx { user: user.into(), why, sound_owner: true }
}

/// Wait until `what` holds (a wall-clock deadline, no fixed sleep).
fn until(what: impl Fn() -> bool) {
    let t0 = std::time::Instant::now();
    while !what() {
        assert!(t0.elapsed() < std::time::Duration::from_secs(60), "never got there");
        std::thread::yield_now();
    }
}

#[test]
fn every_registered_kind_stops_on_a_sign_out_and_only_then() {
    let svc = svc("conf");
    let launcher = Arc::new(FakeLauncher(Mutex::new(Vec::new())));
    let player = Arc::new(MediaPlayer::new(launcher.clone()));
    let readers = Arc::new(ReaderGroup::default());
    let reg = SessionEnd::new();
    register_with(&svc, &reg, player.clone(), readers.clone());
    let names: Vec<(String, _)> = reg.registered();
    let want: Vec<(String, _)> = REGISTERED.iter().map(|(n, k)| (n.to_string(), *k)).collect();
    assert_eq!(names, want, "REGISTERED lists exactly what register_with registers");

    // nothing runs: nothing stops and nothing is audited as a stop
    let log = Mutex::new(Vec::new());
    let rep = reg.end(&ctx("maya", Why::SignOut), &|a, d| log.lock().unwrap().push(format!("{a} {d}")));
    assert!(rep.stopped.is_empty(), "{:?}", rep.stopped);
    assert_eq!(*log.lock().unwrap(), vec![format!("session.end.count sign-out 0 of {}", REGISTERED.len())]);

    // the radio (a station that refuses is still not idle), the player and its queue, a reading, the two grants
    svc.radio.set_streamer(Arc::new(Refuse));
    svc.radio.play(&crate::radio::Station::new("Test FM", "http://station.invalid/x"), Arc::new(|_| true), "cat");
    player.play("maya", "song", "/music/song.mp3").unwrap();
    player.set_queue("maya", vec!["/music/a.mp3".into()], 0, true);
    let reader = readers.reader();
    reader.start(vec![Chunk { unit: 0, start: 0, end: 5, text: "Hello".into() }], 0, true, "v", Arc::new(Wav), Arc::new(Loud));
    until(|| matches!(reader.status(), Status::Playing));
    svc.camera.grant("maya", 300);
    svc.screen.grant("maya", 300);

    let rep = reg.end(&ctx("maya", Why::SignOut), &|_, _| {});
    let mut got = rep.stopped.clone();
    got.sort();
    // the first five are the sound and grants of lane SO1: this test starts those; the others have their own tests below
    let mut all: Vec<String> = REGISTERED.iter().take(5).map(|(n, _)| n.to_string()).collect();
    all.sort();
    assert_eq!(got, all, "every registered kind stopped something");
    assert!(matches!(svc.radio.state(), iyakki_tile_api::radio::RadioState::Idle));
    assert!(launcher.0.lock().unwrap().iter().all(|f| f.load(Ordering::SeqCst)));
    assert!(player.queue_state().items.is_empty());
    assert_eq!(reader.status(), Status::Idle);
    assert!(!svc.camera.granted("maya") && !svc.screen.granted("maya"));

    // a second sign-out has nothing left to stop
    assert!(reg.end(&ctx("maya", Why::SignOut), &|_, _| {}).stopped.is_empty());
}

#[test]
fn another_persons_player_and_a_non_owner_radio_are_left_alone_and_a_lock_keeps_the_sound() {
    let svc = svc("other");
    let launcher = Arc::new(FakeLauncher(Mutex::new(Vec::new())));
    let player = Arc::new(MediaPlayer::new(launcher.clone()));
    let reg = SessionEnd::new();
    register_with(&svc, &reg, player.clone(), Arc::new(ReaderGroup::default()));
    svc.radio.set_streamer(Arc::new(Refuse));
    svc.radio.play(&crate::radio::Station::new("Test FM", "http://station.invalid/x"), Arc::new(|_| true), "cat");
    player.play("sam", "song", "/music/song.mp3").unwrap();
    svc.camera.grant("maya", 300);

    // maya signs out but the sound is sam's: sam's player and the radio go on, maya's camera grant ends
    let c = Ctx { user: "maya".into(), why: Why::SignOut, sound_owner: false };
    assert_eq!(reg.end(&c, &|_, _| {}).stopped, vec!["camera-grant"]);
    assert!(!matches!(svc.radio.state(), iyakki_tile_api::radio::RadioState::Idle));
    assert!(launcher.0.lock().unwrap().iter().all(|f| !f.load(Ordering::SeqCst)));

    // a lock runs only the capture hooks: the radio and the player play on
    svc.screen.grant("sam", 300);
    let rep = reg.end(&ctx("sam", Why::Lock), &|_, _| {});
    assert_eq!(rep.stopped, vec!["screen-grant"]);
    assert!(!matches!(svc.radio.state(), iyakki_tile_api::radio::RadioState::Idle));

    // the sound changes owner from sam: sam's sound sources end
    let rep = reg.end(&ctx("sam", Why::SoundSwitch), &|_, _| {});
    assert_eq!(rep.stopped, vec!["radio", "media-player"]);
}

/// Port-forwards that only count: what stop_all stopped.
struct Fwds(Mutex<usize>);
impl iyakki_core_api::portforward::PortForwards for Fwds {
    fn start(&self, _: &iyakki_core_api::portforward::Request) -> Result<iyakki_core_api::portforward::Info, String> {
        Err("not here".into())
    }
    fn list(&self) -> Vec<iyakki_core_api::portforward::Info> {
        Vec::new()
    }
    fn stop(&self, _: u32) -> bool {
        false
    }
    fn stop_all(&self) -> usize {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct FakeWin(Arc<AtomicBool>);
impl iyakki_shared::linuxwin::Proc for FakeWin {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn alive(&mut self) -> bool {
        !self.0.load(Ordering::SeqCst)
    }
    fn kill(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct AnyOne;
impl iyakki_core_api::PolicySource for AnyOne {
    fn policy_of(&self, _: &str) -> Option<iyakki_sec::policy::Policy> {
        Some(iyakki_sec::policy::Role::Admin.template())
    }
}

fn owners(forwards: Arc<Fwds>) -> (Owners, &'static iyakki_shared::sessionend::LiveSet<crate::terminalfacet::Terminals>) {
    let live: &'static iyakki_shared::sessionend::LiveSet<crate::terminalfacet::Terminals> = Box::leak(Box::new(iyakki_shared::sessionend::LiveSet::new()));
    (Owners { stops: Stops::private(), forwards, terminals: live }, live)
}

fn cloud_on(svc: &Arc<Services>) {
    svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_cloud_stream = true).unwrap();
}

fn running_job(svc: &Arc<Services>, user: &str, workspace: &str, label: &str) -> String {
    let work = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let spec = crate::jobs::Spec { label: label.into(), cmd: crate::jobs::Cmd::Shell("sleep 300".into()), cwd: work, workspace: workspace.into() };
    svc.jobs.start(user, &spec, &|_| Ok(())).unwrap().id
}

#[test]
fn every_sound_capture_and_process_kind_stops_on_a_sign_out_with_its_count_and_a_suspend_stops_sound_and_capture_only() {
    let svc = svc("kinds");
    let player = Arc::new(MediaPlayer::new(Arc::new(FakeLauncher(Mutex::new(Vec::new())))));
    let readers = Arc::new(ReaderGroup::default());
    let reg = SessionEnd::new();
    let fwds = Arc::new(Fwds(Mutex::new(2)));
    let (own, live) = owners(fwds.clone());
    register_all(&svc, &reg, player, readers, &own);
    let want: Vec<(String, _)> = REGISTERED.iter().map(|(n, k)| (n.to_string(), *k)).collect();
    assert_eq!(reg.registered(), want, "REGISTERED lists exactly what register_all registers");
    assert!(REGISTERED.iter().any(|(_, k)| *k == iyakki_shared::sessionend::Kind::Process), "the process kinds are in the list");
    let s = own.stops;

    // sound: a speech player, the assistant's speaker thread and a cloud answer
    let play = crate::voice::command_for(&["sleep".to_string(), "300".to_string()], None).stdin(std::process::Stdio::null()).spawn().unwrap();
    let mut player = crate::voice::Player::from_speech_in(s.players, play, Vec::new(), None, std::time::Duration::from_secs(600));
    let say = s.assistant_speaker.enter(|| {});
    let cloud = s.cloud_speaker.enter(|| {});
    // capture: a microphone process, a browser page with its microphone, camera and sound on, the cloud listener
    let mut rec = crate::voice::Recorder::start_in(s.recorders, "sleep 300", 5).unwrap();
    iyakki_shared::browsermedia::isolate_for_this_thread();
    let page = iyakki_shared::browsermedia::BrowserMedia::silent(iyakki_shared::browsermedia::Policy::allow_all());
    iyakki_shared::browsermedia::register("kinds-maya", page.clone());
    page.set(iyakki_shared::browsermedia::Kind::Mic, true).unwrap();
    page.set_audio_out(true);
    cloud_on(&svc);
    assert!(svc.cloud_voice_offer_at("127.0.0.1:0").is_ok());
    assert!(rec.is_alive() && player.is_playing());
    // processes: a terminal, forwards (2), a Linux window, an IDE run, a lab job
    let terms = Arc::new(crate::terminalfacet::Terminals::new(svc.clone()));
    terms.set_policies(Arc::new(AnyOne));
    live.add(&terms);
    use iyakki_core_api::terminal::Terminal;
    let t1 = terms.open("kinds-maya", "sample").unwrap().id;
    let other = terms.open("kinds-bob", "sample").unwrap().id;
    let mut win = iyakki_shared::linuxwin::Window::around(Box::new(FakeWin(Arc::new(AtomicBool::new(false)))), "xterm", Arc::new(|_, _| {}));
    win.enter(s.windows);
    let ide = running_job(&svc, "kinds-maya", "proj", "build");
    let lab = running_job(&svc, "kinds-maya", "", "chat job");
    let install = running_job(&svc, "kinds-maya", iyakki_shared::quickinstall::WORKSPACE, "install");

    // a suspend: sound and capture, not the processes
    let rep = reg.end(&Ctx { user: String::new(), why: Why::Suspend, sound_owner: true }, &|_, _| {});
    let mut got = rep.stopped.clone();
    got.sort();
    let mut want_stopped = vec!["voice-players", "voice-assistant-speaker", "cloud-speaker", "voice-recorders", "cloud-audio-listener", "browser-capture"].into_iter().map(String::from).collect::<Vec<_>>();
    want_stopped.sort();
    assert_eq!(got, want_stopped, "a suspend stops sound and capture only");
    assert!(!rec.is_alive() && !player.is_playing() && say.stopped() && cloud.stopped() && !page.is_on(iyakki_shared::browsermedia::Kind::Mic));
    assert_eq!(svc.jobs.running_count("kinds-maya"), 3, "the jobs went on");
    assert!(page.sources().audio_out && *fwds.0.lock().unwrap() == 2 && win.alive());

    // a sign-out: the processes too, each with its count in the audit
    let log = Mutex::new(Vec::new());
    let rep = reg.end(&ctx("kinds-maya", Why::SignOut), &|a, d| log.lock().unwrap().push(format!("{a} {d}")));
    let log = log.lock().unwrap().clone();
    for line in ["session.end terminals 1", "session.end port-forwards 2", "session.end linux-windows 1", "session.end ide-task-runs 1", "session.end lab-jobs 1", "session.end browser-audio"] {
        assert!(log.contains(&line.to_string()), "{line} in {log:?}");
    }
    assert_eq!(rep.ran, REGISTERED.len());
    // together with the first test (the five of lane SO1) every registered kind has been walked
    let mut covered: Vec<&str> = REGISTERED.iter().take(5).map(|(n, _)| *n).collect();
    covered.extend(want_stopped.iter().map(String::as_str));
    covered.extend(["terminals", "port-forwards", "linux-windows", "ide-task-runs", "lab-jobs", "browser-audio"]);
    covered.sort();
    let mut every: Vec<&str> = REGISTERED.iter().map(|(n, _)| *n).collect();
    every.sort();
    assert_eq!(covered, every, "a new kind needs a line in the conformance test");
    assert!(terms.run("kinds-maya", &t1, "help").is_err(), "the person's terminal is closed");
    assert!(terms.run("kinds-bob", &other, "help").is_ok(), "another person's terminal stays");
    assert!(!win.alive() && !page.sources().audio_out);
    let state = |id: &str| svc.jobs.get("kinds-maya", id).unwrap().state;
    assert_eq!((state(&ide), state(&lab)), (crate::jobs::State::Stopped, crate::jobs::State::Stopped));
    assert_eq!(state(&install), crate::jobs::State::Running, "an install of the system goes on");
    let _ = svc.jobs.stop("kinds-maya", &install, std::time::Duration::from_millis(200));
    iyakki_shared::browsermedia::unregister("kinds-maya");

    // a second sign-out has nothing left
    assert!(reg.end(&ctx("kinds-maya", Why::SignOut), &|_, _| {}).stopped.is_empty());
    // the player and recorder of this test are gone with their processes
    drop((player, rec));
}

#[test]
fn a_job_marked_to_go_on_survives_the_sign_out_and_a_lock_runs_no_process_hook() {
    let svc = svc("keep");
    let reg = SessionEnd::new();
    let (own, _) = owners(Arc::new(Fwds(Mutex::new(0))));
    register_all(&svc, &reg, Arc::new(MediaPlayer::new(Arc::new(FakeLauncher(Mutex::new(Vec::new()))))), Arc::new(ReaderGroup::default()), &own);
    let kept = running_job(&svc, "keep-ann", "proj", "server");
    let plain = running_job(&svc, "keep-ann", "proj", "build");
    svc.jobs.set_keep("keep-ann", &kept, true).unwrap();
    assert!(reg.end(&ctx("keep-ann", Why::Lock), &|_, _| {}).stopped.is_empty(), "a lock stops no job");
    assert_eq!(svc.jobs.running_count("keep-ann"), 2);
    let rep = reg.end(&ctx("keep-ann", Why::SignOut), &|_, _| {});
    assert_eq!((rep.stopped, rep.counts), (vec!["ide-task-runs".to_string()], vec![1]));
    assert!(svc.jobs.get("keep-ann", &kept).unwrap().running() && !svc.jobs.get("keep-ann", &plain).unwrap().running());
    let _ = svc.jobs.stop("keep-ann", &kept, std::time::Duration::from_millis(200));
}

#[test]
fn another_persons_sound_owner_rule_applies_to_the_voice_players_and_a_sound_switch_runs_sound_only() {
    let svc = svc("switch");
    let reg = SessionEnd::new();
    let (own, _) = owners(Arc::new(Fwds(Mutex::new(0))));
    register_all(&svc, &reg, Arc::new(MediaPlayer::new(Arc::new(FakeLauncher(Mutex::new(Vec::new()))))), Arc::new(ReaderGroup::default()), &own);
    let say = own.stops.assistant_speaker.enter(|| {});
    // not the owner of the sound: its speakers go on; the owner's switch ends them
    let c = Ctx { user: "x".into(), why: Why::SignOut, sound_owner: false };
    assert!(!reg.end(&c, &|_, _| {}).stopped.contains(&"voice-assistant-speaker".to_string()) && !say.stopped());
    let rep = reg.end(&ctx("x", Why::SoundSwitch), &|_, _| {});
    assert!(rep.stopped.contains(&"voice-assistant-speaker".to_string()) && !rep.stopped.contains(&"voice-recorders".to_string()), "{:?}", rep.stopped);
    // the microphone of the voice desk goes at a lock
    let mut rec = crate::voice::Recorder::start_in(own.stops.recorders, "sleep 300", 5).unwrap();
    assert!(rec.is_alive());
    assert!(reg.end(&ctx("x", Why::Lock), &|_, _| {}).stopped.contains(&"voice-recorders".to_string()));
    assert!(!rec.is_alive());
}

struct Held {
    started: std::sync::mpsc::Sender<()>,
    stopped: Arc<AtomicBool>,
}
impl Sink for Held {
    fn play(&self, _: &[u8]) -> Result<Box<dyn Playing>, String> {
        let _ = self.started.send(());
        Ok(Box::new(HeldPlaying(self.stopped.clone())))
    }
}
struct HeldPlaying(Arc<AtomicBool>);
impl Playing for HeldPlaying {
    fn is_playing(&mut self) -> bool {
        !self.0.load(Ordering::SeqCst)
    }
    fn stop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn the_assistants_sentence_is_cut_while_it_plays_when_the_session_ends() {
    let set: &'static iyakki_shared::sessionend::StopSet = Box::leak(Box::new(iyakki_shared::sessionend::StopSet::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let (busy, stop) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
    let ended = set.enter(|| {});
    let (b2, s2, st2) = (busy.clone(), stop.clone(), stopped.clone());
    let t = std::thread::spawn(move || crate::voiceassist_ports::speak_sentence(&Wav, &Held { started: tx, stopped: st2 }, "hello", false, &b2, &s2, &ended));
    rx.recv().unwrap();
    assert!(busy.load(Ordering::SeqCst) && !stopped.load(Ordering::SeqCst), "it is playing");
    assert_eq!(set.stop_all(), 1);
    t.join().unwrap();
    assert!(stopped.load(Ordering::SeqCst) && !busy.load(Ordering::SeqCst), "the sound was cut and the speaker is free");
    // a sentence started after the end is not played at all
    let (tx2, rx2) = std::sync::mpsc::channel();
    let ended2 = set.enter(|| {});
    set.stop_all();
    let busy2 = AtomicBool::new(true);
    crate::voiceassist_ports::speak_sentence(&Wav, &Held { started: tx2, stopped: Arc::new(AtomicBool::new(false)) }, "again", false, &busy2, &AtomicBool::new(false), &ended2);
    assert!(rx2.try_recv().is_err() && !busy2.load(Ordering::SeqCst));
}

#[test]
fn the_cloud_audio_listener_ends_with_the_session_and_a_new_offer_starts_a_new_one() {
    let svc = svc("cloud");
    cloud_on(&svc);
    assert!(!svc.end_cloud_voice(), "nothing listens yet");
    svc.cloud_voice_offer_at("127.0.0.1:0").unwrap();
    let l = svc.cloud_voice.lock().unwrap().clone().unwrap();
    assert!(!l.ended());
    assert!(svc.end_cloud_voice());
    assert!(l.ended() && svc.cloud_voice.lock().unwrap().is_none());
    assert!(!svc.end_cloud_voice());
    svc.cloud_voice_offer_at("127.0.0.1:0").unwrap();
    assert!(svc.end_cloud_voice(), "a later offer listens again, and ends again");
}
