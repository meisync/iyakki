//! The VOICE tile in the tile tables (counts, group, roles, cloud mode, opening) and the system sounds the shell plays: login,
//! logout (which never holds sign-out back), a new notification and a finished job. Nothing plays for real: cues go to a recording
//! fake output (`cues::fake`).
use iyakki_agent::facets_h::RealServices as _;

use super::tests::*;
use super::*;
use crate::cues::fake;
use iyakki_cloudmode as cloudmode;
use iyakki_cloudmode::{Facts, Setting};
use iyakki_shared::jobs::{Cmd, Spec};
use iyakki_agent::syssounds::{self as ss, Cue};
use iyakki_sec::policy::Role;
use std::time::{Duration, Instant};

/// The size of the WAV a cue is played as (at the default level).
fn wav_len(c: Cue) -> usize {
    44 + 2 * ss::render(c, ss::DEFAULT_OFFSET_DB).len()
}

fn signed_in(tag: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    sh
}

/// Wait (up to 3 s) until the fake output has played `n` sounds.
fn played(n: usize) -> Vec<usize> {
    let t = Instant::now();
    loop {
        let p = fake::played();
        if p.len() >= n || t.elapsed() > Duration::from_secs(3) {
            return p;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_voice_tile_is_one_tile_in_home_for_everyone_but_guests() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Voice).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::Voice).unwrap();
    assert_eq!((t.label, t.sub, t.group), ("VOICE", "Read, dictate", Some(Group::Home)));
    assert_eq!((PApp::Voice.id(), PApp::Voice.label().as_str()), ("voice", "VOICE"));
    assert_eq!(Group::of(PApp::Voice), Some(Group::Home));
    assert_eq!(TILES.len(), PApp::ALL.len() - 1, "every app but the inbox has a tile");
    assert_eq!(PApp::ALL.len(), 42, "FIREWALL, LEARN, CHECK-UP, DOCTOR, GIT, LAB, DISKS, LINUX WINDOW, VOICE, K8S, DEV TOOLS, AUTH, REMOTE FILES, CAMERAS, PDF, SAMPLE and MARKDOWN are the newest tiles");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Voice), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Voice));
    assert!(layout_tiles(true, &cloudmode::CloudMode::desktop()).iter().any(|t| t.1 == "VOICE" && t.2 == "home"));
}

#[test]
fn the_voice_tile_opens_for_a_user_not_for_a_guest_and_is_hidden_in_cloud_mode_without_sound() {
    let (mut sh, _) = shell_with_users("voice-tile");
    login(&mut sh, "alice", "alicepass1");
    assert!(sh.visible_tiles().iter().any(|t| t.label == "VOICE"));
    sh.open(PApp::Voice);
    assert_eq!(sh.screen_name(), "app:voice");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "READ ALOUD") && sh.hits().iter().any(|h| h.label == "DICTATE"));
    key(&mut sh, Key::Esc); // leaves the box
    key(&mut sh, Key::Esc); // and goes back
    assert_eq!(sh.screen_name(), "launcher");
    // cloud mode: hidden when there is no sound output, kept when the machine has one
    sh.set_cloud_for_test(cloudmode::cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() }));
    assert!(!sh.visible_tiles().iter().any(|t| t.label == "VOICE"));
    sh.open(PApp::Voice);
    assert_eq!(sh.screen_name(), "launcher", "nothing opens a hidden app");
    sh.set_cloud_for_test(cloudmode::cloud_mode(Setting::Auto, &Facts { display: false, ..Facts::full() }));
    assert!(sh.visible_tiles().iter().any(|t| t.label == "VOICE"), "hardware that exists stays");
    assert_eq!(cloudmode::app_class(PApp::Voice), cloudmode::Class::Hardware(cloudmode::Need::Audio));
    // guests have no VOICE at all
    let (mut sh, _) = shell_with_users("voice-guest");
    login(&mut sh, "gus", "guestpass1");
    sh.open(PApp::Voice);
    assert_eq!(sh.screen_name(), "launcher");
}

#[test]
fn quick_install_opens_from_the_voice_tile_without_leaving_it() {
    let mut sh = signed_in("voice-qi");
    sh.open(PApp::Voice);
    // the app asks for Quick install: the Store opens (on Setup), the app is kept aside and comes back with Back
    sh.open_linked_from_app("quickinstall");
    assert!(!sh.brand.open);
    assert_eq!(sh.screen_name(), "app:store", "the Store shows Setup");
    sh.leave_app();
    assert_eq!(sh.screen_name(), "app:voice", "Back returns to the app");
}

#[test]
fn signing_in_plays_the_login_sound_and_signing_out_the_logout_sound() {
    fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-basic");
    assert!(fake::played().is_empty(), "the login form is silent");
    login(&mut sh, "root", "rootpass12");
    assert_eq!(played(1), vec![wav_len(Cue::Login)]);
    key(&mut sh, Key::Esc); // sign out
    assert_eq!(sh.screen_name(), "login");
    assert_eq!(played(2), vec![wav_len(Cue::Login), wav_len(Cue::Logout)]);
    // the same lengths mean different sounds
    assert_ne!(ss::render(Cue::Login, -12), ss::render(Cue::Logout, -12));
}

#[test]
fn a_slow_output_never_holds_sign_out_back_and_a_quick_one_finishes_before_the_screen_changes() {
    // an output that takes three seconds: sign-out is done within the bound
    let slow = fake::reset(3000);
    let mut sh = signed_in("cue-slow");
    let t = Instant::now();
    key(&mut sh, Key::Esc);
    let took = t.elapsed();
    assert_eq!(sh.screen_name(), "login");
    assert!(took < ss::LOGOUT_WAIT + Duration::from_millis(500), "sign-out took {took:?}");
    assert!(took >= Duration::from_millis(600), "it did wait for the sound: {took:?}");
    assert!(slow.played.lock().unwrap().len() <= 1, "the slow output was left to finish by itself");
    // an output that takes 100 ms: the sound has been played when the login form appears
    let quick = fake::reset(100);
    let mut sh = signed_in("cue-quick");
    quick.played.lock().unwrap().clear();
    let t = Instant::now();
    key(&mut sh, Key::Esc);
    assert!(t.elapsed() < Duration::from_millis(600), "{:?}", t.elapsed());
    assert!(quick.played.lock().unwrap().contains(&wav_len(Cue::Logout)), "the logout sound was over before the screen changed");
}

#[test]
fn nothing_plays_when_the_switch_is_off_muted_without_an_output_or_in_cloud_mode() {
    // the master switch
    let out = fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-off");
    sh.services_for_demo().update_system_sounds(|s| s.enabled = false).unwrap();
    login(&mut sh, "root", "rootpass12");
    key(&mut sh, Key::Esc);
    std::thread::sleep(Duration::from_millis(100));
    assert!(out.played.lock().unwrap().is_empty());
    // one cue off, the others on
    let out = fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-one");
    sh.services_for_demo().update_system_sounds(|s| s.logout = false).unwrap();
    login(&mut sh, "root", "rootpass12");
    key(&mut sh, Key::Esc);
    assert_eq!(played(1), vec![wav_len(Cue::Login)]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(out.played.lock().unwrap().len(), 1, "no logout sound");
    // muted in Settings -> Sound
    let out = fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-muted");
    sh.services_for_demo().update_sound(|s| s.muted = true).unwrap();
    login(&mut sh, "root", "rootpass12");
    std::thread::sleep(Duration::from_millis(100));
    assert!(out.played.lock().unwrap().is_empty());
    // no output at all
    let out = fake::reset(0);
    *out.ready.lock().unwrap() = Err("no sound output".into());
    let (mut sh, _) = shell_with_users("cue-nooutput");
    login(&mut sh, "root", "rootpass12");
    key(&mut sh, Key::Esc);
    std::thread::sleep(Duration::from_millis(100));
    assert!(out.played.lock().unwrap().is_empty());
    // cloud mode without sound hardware
    let out = fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-cloud");
    sh.set_cloud_for_test(cloudmode::cloud_mode(Setting::Auto, &Facts::bare()));
    login(&mut sh, "root", "rootpass12");
    std::thread::sleep(Duration::from_millis(100));
    assert!(out.played.lock().unwrap().is_empty());
}

#[test]
fn a_new_notification_chimes_once_and_what_was_unread_at_sign_in_does_not() {
    let out = fake::reset(0);
    let (mut sh, _) = shell_with_users("cue-notify");
    let svc = sh.services_for_demo().clone();
    svc.real().notify("root", iyakki_shared::inbox::Kind::Warning, "Waiting for you", "from before", None).unwrap();
    login(&mut sh, "root", "rootpass12");
    assert_eq!(played(1), vec![wav_len(Cue::Login)]);
    sh.refresh_badge();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(out.played.lock().unwrap().len(), 1, "an unread note from before is not new");
    svc.real().notify("root", iyakki_shared::inbox::Kind::Warning, "A new one", "just now", None).unwrap();
    sh.refresh_badge();
    assert_eq!(played(2), vec![wav_len(Cue::Login), wav_len(Cue::Notify)]);
    sh.refresh_badge();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(out.played.lock().unwrap().len(), 2, "the same count chimes once");
}

#[test]
fn a_finished_job_has_the_done_sound_and_not_the_notification_chime_as_well() {
    let out = fake::reset(0);
    let mut sh = signed_in("cue-job");
    assert_eq!(played(1).len(), 1);
    let svc = sh.services_for_demo().clone();
    let dir = std::env::temp_dir();
    let id = svc.jobs().start("root", &Spec { label: "quick".into(), cmd: Cmd::Shell("true".into()), cwd: dir, workspace: String::new() }, &|_: &std::path::Path| Ok(())).unwrap().id;
    let t = Instant::now();
    while svc.jobs().list("root").iter().any(|j| j.id == id && j.running()) && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    sh.announce_finished_jobs();
    let p = played(2);
    assert_eq!(p, vec![wav_len(Cue::Login), wav_len(Cue::JobDone)], "{p:?}");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(out.played.lock().unwrap().len(), 2, "one sound for the batch");
}

/// A hook of this test only (a process-wide registry is shared by every shell test, so the probe has a name of its own).
fn probe(name: &'static str, kind: iyakki_shared::sessionend::Kind) -> Arc<std::sync::atomic::AtomicUsize> {
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    iyakki_shared::sessionend::global().register(name, kind, move |c| {
        if c.user == "root" {
            n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return true;
        }
        false
    });
    n
}

#[test]
fn a_sign_out_ends_the_session_sources_before_the_screen_changes_and_the_logout_cue_still_plays() {
    use std::sync::atomic::Ordering;
    let quick = fake::reset(100);
    let mut sh = signed_in("end-order");
    quick.played.lock().unwrap().clear();
    let sound = probe("test-probe-order-sound", iyakki_shared::sessionend::Kind::Sound);
    let cap = probe("test-probe-order-capture", iyakki_shared::sessionend::Kind::Capture);
    key(&mut sh, Key::Esc);
    assert_eq!(sh.screen_name(), "login");
    assert!(sound.load(Ordering::SeqCst) >= 1 && cap.load(Ordering::SeqCst) >= 1, "both kinds ended");
    assert!(quick.played.lock().unwrap().contains(&wav_len(Cue::Logout)), "the logout cue still played");
    let audit = std::fs::read_to_string(sh.env.audit.path_buf()).unwrap_or_default();
    let at = |needle: &str| audit.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} not in {audit}"));
    assert!(at("\"session.end\",\"detail\":\"test-probe-order-sound\"") < at("\"event\":\"logout\""), "the sources stop before the session is closed");
    assert!(at("\"session.end.count\",\"detail\":\"sign-out") < at("\"event\":\"logout\""));
    iyakki_shared::sessionend::global().unregister("test-probe-order-sound");
    iyakki_shared::sessionend::global().unregister("test-probe-order-capture");
}

#[test]
fn a_lock_ends_capture_only_and_a_change_of_sound_owner_ends_the_previous_persons_sound_only() {
    use iyakki_shared::sessionend::Why;
    use std::sync::atomic::Ordering;
    let mut sh = signed_in("end-lock");
    let sound = probe("test-probe-lock-sound", iyakki_shared::sessionend::Kind::Sound);
    let cap = probe("test-probe-lock-capture", iyakki_shared::sessionend::Kind::Capture);
    let (s0, c0) = (sound.load(Ordering::SeqCst), cap.load(Ordering::SeqCst));
    assert!(sh.lock_screen("test"));
    assert_eq!(sound.load(Ordering::SeqCst), s0, "playback goes on behind a lock");
    assert!(cap.load(Ordering::SeqCst) > c0, "capture stops with the lock");
    let c1 = cap.load(Ordering::SeqCst);
    sh.end_session("root", Why::SoundSwitch, true);
    assert!(sound.load(Ordering::SeqCst) > s0, "the previous owner's sound ends when the sound changes hands");
    assert_eq!(cap.load(Ordering::SeqCst), c1, "a switch of the sound does not touch capture");
    iyakki_shared::sessionend::global().unregister("test-probe-lock-sound");
    iyakki_shared::sessionend::global().unregister("test-probe-lock-capture");
}

/// A probe that remembers why it was run (other tests sign out as the same person at the same time, so a test looks at the reasons seen).
fn why_probe(name: &'static str, kind: iyakki_shared::sessionend::Kind) -> Arc<Mutex<Vec<iyakki_shared::sessionend::Why>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    iyakki_shared::sessionend::global().register(name, kind, move |c| {
        s2.lock().unwrap().push(c.why);
        false
    });
    seen
}

#[test]
fn before_the_machine_sleeps_capture_and_sound_stop_and_a_process_goes_on() {
    use iyakki_shared::sessionend::{Kind, Why};
    let mut sh = signed_in("end-sleep");
    let sound = why_probe("test-probe-sleep-sound", Kind::Sound);
    let cap = why_probe("test-probe-sleep-capture", Kind::Capture);
    let proc = why_probe("test-probe-sleep-process", Kind::Process);
    sh.sleep_now("root", "test");
    let (s, c, p) = (sound.lock().unwrap().clone(), cap.lock().unwrap().clone(), proc.lock().unwrap().clone());
    assert!(s.contains(&Why::Suspend), "the sound stops before the machine sleeps: {s:?}");
    assert!(!s.contains(&Why::Lock), "a lock alone keeps the sound: {s:?}");
    let (lock_at, suspend_at) = (c.iter().position(|w| *w == Why::Lock), c.iter().position(|w| *w == Why::Suspend));
    assert!(lock_at.is_some() && suspend_at.is_some() && lock_at < suspend_at, "capture stops at the lock and again at the suspend: {c:?}");
    assert!(!p.iter().any(|w| matches!(w, Why::Lock | Why::Suspend)), "terminals, forwards and jobs go on through a sleep: {p:?}");
    let audit = std::fs::read_to_string(sh.env.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("\"event\":\"session.end.count\",\"detail\":\"suspend "), "the suspend is audited with its count");
    for n in ["test-probe-sleep-sound", "test-probe-sleep-capture", "test-probe-sleep-process"] {
        iyakki_shared::sessionend::global().unregister(n);
    }
}
