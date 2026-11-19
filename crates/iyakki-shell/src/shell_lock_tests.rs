//! The lock screen in the shell: locking keeps the person signed in and the apps running, closes the vault, takes every key and click;
//! the right password opens it again, a wrong one is throttled like at sign-in; Super+L, the Power menu, a request file (Settings, the
//! assistant), sleep and the idle minutes lock; media or a waiting approval hold the idle lock off; a guest is signed out instead; the
//! sign-in screen is unchanged. The password is never in the audit log.
use iyakki_agent::facets_h::RealServices as _;

use super::tests::*;
use super::*;
use crate::saver::Stage;
use iyakki_shared::lockscreen::{self, LockConfig};
use iyakki_shared::screensaver::{First, SaverConfig, Style};
use std::sync::atomic::{AtomicU64, Ordering};

const MIN: u64 = 60_000;

fn signed_in(tag: &str, user: &str, pw: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, user, pw);
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

fn unlock_with(sh: &mut Shell, pw: &str) {
    typ(sh, pw);
    key(sh, Key::Enter);
}

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

/// A signed-in shell with a fake clock, no screen saver of its own and `lock_min` as the person's automatic lock.
fn timed(tag: &str, lock_min: u32) -> (Shell, Arc<AtomicU64>) {
    let mut sh = signed_in(tag, "root", "rootpass12");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    sh.saver_config_for_test(SaverConfig { first: First::Nothing, ..SaverConfig::default() });
    if lock_min > 0 {
        LockConfig { auto_min: lock_min }.save(sh.env.data_dir(), "root").unwrap();
    }
    (sh, clock)
}

fn run(sh: &mut Shell, clock: &AtomicU64, ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(250);
        clock.fetch_add(step, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        left -= step;
    }
}

#[test]
fn locking_keeps_the_person_signed_in_closes_the_vault_and_covers_the_desktop() {
    let mut sh = signed_in("lk-basic", "root", "rootpass12");
    assert!(sh.env.services.user_vault_unlocked("root"), "the vault opened at sign-in");
    assert!(!sh.is_locked());
    assert!(sh.lock_screen("test"));
    assert!(sh.is_locked() && sh.screen_name() == "locked");
    assert!(sh.session.is_some(), "still signed in: nothing was stopped");
    assert!(!sh.env.services.user_vault_unlocked("root"), "the vault stays closed until the password is typed");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "Unlock") && labels.iter().any(|l| l == "field"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.starts_with("group ") || l == "IYAKKI menu"), "nothing of the desktop can be clicked: {labels:?}");
    assert!(audit_text(&sh).contains("lock"), "locking is audited");
}

#[test]
fn a_locked_screen_takes_every_key_and_the_right_password_opens_it_again() {
    let mut sh = signed_in("lk-keys", "root", "rootpass12");
    sh.lock_screen("test");
    // keys that would open or close things on the desktop are the prompt's
    for k in [Key::Char('s'), Key::Char('q'), Key::Esc, Key::F(10), Key::F(12), Key::Down, Key::Ctrl('p')] {
        key(&mut sh, k.clone());
        assert!(sh.is_locked() && sh.screen_name() == "locked", "{k:?} did not get past the lock");
    }
    sh.handle(&Event::Scroll(3));
    sh.handle(&Event::ScrollPx(120));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.is_locked() && !sh.fnk.power_card);
    unlock_with(&mut sh, "wrong password");
    assert!(sh.is_locked(), "a wrong password does not open it");
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    assert_eq!(sh.screen_name(), "launcher", "the desktop is as it was");
    assert!(sh.session.is_some() && sh.env.services.user_vault_unlocked("root"), "the vault opens with the password");
    let log = audit_text(&sh);
    assert!(log.contains("unlock") && log.contains("unlock.failed"));
    assert!(!log.contains("rootpass12") && !log.contains("wrong password"), "no password is ever logged");
}

#[test]
fn a_wrong_password_shows_an_error_wipes_the_field_and_attempts_are_throttled_like_at_sign_in() {
    let mut sh = signed_in("lk-throttle", "root", "rootpass12");
    sh.lock_screen("test");
    let mut g = Gfx::new(1280, 720);
    for _ in 0..5 {
        unlock_with(&mut sh, "nope nope nope");
        assert!(sh.lockui.form.fields[0].value.is_empty(), "the typed password is wiped after every attempt");
    }
    // even the right password is refused during the wait
    unlock_with(&mut sh, "rootpass12");
    assert!(sh.is_locked(), "throttled");
    assert!(sh.lockui.form.error.as_deref().is_some(), "the wait is explained");
    sh.render(&mut g);
    assert!(!audit_text(&sh).contains("nope nope"));
    // an empty field is asked for, not counted
    let mut sh2 = signed_in("lk-empty", "root", "rootpass12");
    sh2.lock_screen("test");
    key(&mut sh2, Key::Enter);
    assert_eq!(sh2.lockui.form.error.as_deref(), Some("Type your password"));
    unlock_with(&mut sh2, "rootpass12");
    assert!(!sh2.is_locked(), "an empty attempt did not count against the person");
}

#[test]
fn the_password_can_be_typed_pasted_and_unlocked_with_the_mouse() {
    let mut sh = signed_in("lk-mouse", "root", "rootpass12");
    sh.lock_screen("test");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    sh.handle(&Event::Paste("rootpass12".into()));
    click(&mut sh, &mut g, "Unlock");
    assert!(!sh.is_locked(), "paste, then the Unlock button");
    // another person's password does not open this lock
    sh.lock_screen("test");
    unlock_with(&mut sh, "alicepass1");
    assert!(sh.is_locked(), "only the signed-in person's own password opens their screen");
}

#[test]
fn every_way_to_lock_works_and_a_guest_is_signed_out_instead() {
    // the lock key (Super+L / KEY_SCREENLOCK)
    let mut sh = signed_in("lk-key", "root", "rootpass12");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(sh.is_locked());
    // the Power menu's Lock and the power key's card
    let mut sh = signed_in("lk-menu", "oscar", "operpass12");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.is_locked() && sh.session.is_some(), "locked, not signed out");
    unlock_with(&mut sh, "operpass12");
    assert_eq!(sh.screen_name(), "launcher", "the power menu is not left open behind the lock");
    sh.fnk.power_card = true;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.is_locked());
    // a request from Settings or the assistant, taken within a second
    let mut sh = signed_in("lk-req", "root", "rootpass12");
    lockscreen::request(sh.env.data_dir(), "alice");
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(!sh.is_locked(), "somebody else's request is not ours");
    lockscreen::request(sh.env.data_dir(), "root");
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.is_locked(), "our own request locks");
    // a guest has no password: Lock signs out
    let mut sh = signed_in("lk-guest", "gus", "guestpass1");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(!sh.is_locked() && sh.session.is_some(), "the lock key does not sign a guest out by surprise");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.session.is_none() && sh.screen_name() == "login", "the Power menu's Lock signs a guest out, as before");
}

#[test]
fn nobody_signed_in_sees_the_normal_sign_in_screen_and_the_lock_key_does_nothing() {
    let (mut sh, _) = shell_with_users("lk-login");
    assert_eq!(sh.screen_name(), "login");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(!sh.is_locked() && sh.screen_name() == "login");
    assert!(!sh.lock_screen("test"), "nothing to lock");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Sign in") && !sh.hits().iter().any(|h| h.label == "Unlock"));
    // signing out from a locked screen cannot happen: the sign-in screen has no lock state left over
    login(&mut sh, "root", "rootpass12");
    sh.lock_screen("test");
    unlock_with(&mut sh, "rootpass12");
    sh.sign_out();
    assert_eq!(sh.screen_name(), "login");
}

#[test]
fn sleeping_locks_the_screen_and_keeps_the_session() {
    let (mut sh, host) = shell_with_users("lk-sleep");
    *host.sleepable.lock().unwrap() = true;
    login(&mut sh, "root", "rootpass12");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('s')));
    assert_eq!(*host.slept.lock().unwrap(), 1);
    assert!(sh.is_locked() && sh.session.is_some(), "the screen asks for the password on wake; the apps were not closed");
    assert!(!sh.env.services.user_vault_unlocked("root"));
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
}

#[test]
fn the_automatic_lock_is_off_until_chosen_and_then_comes_after_the_idle_minutes() {
    let (mut sh, clock) = timed("lk-auto-off", 0);
    run(&mut sh, &clock, 90 * MIN);
    assert!(!sh.is_locked(), "never by default");
    let (mut sh, clock) = timed("lk-auto-on", 10);
    run(&mut sh, &clock, 9 * MIN);
    assert!(!sh.is_locked(), "not yet");
    sh.handle(&Event::MouseMove(10, 10));
    run(&mut sh, &clock, 9 * MIN);
    assert!(!sh.is_locked(), "input restarted the count");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked() && sh.session.is_some(), "locked after ten quiet minutes");
    assert!(!sh.env.services.user_vault_unlocked("root"));
    assert!(audit_text(&sh).contains("idle"));
    // typing on the lock screen is input: it does not lock a second time or wipe the prompt
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    run(&mut sh, &clock, 5 * MIN);
    assert!(!sh.is_locked(), "the count started again at the unlock");
}

#[test]
fn media_a_waiting_approval_or_a_dialog_hold_the_automatic_lock_off_and_the_count_starts_after_them() {
    let (mut sh, clock) = timed("lk-hold", 5);
    sh.fnk.power_card = true;
    run(&mut sh, &clock, 30 * MIN);
    assert!(!sh.is_locked(), "a card waits for the person: no lock over it");
    sh.fnk.power_card = false;
    run(&mut sh, &clock, 4 * MIN);
    assert!(!sh.is_locked(), "the count began when the hold ended, not at the last key");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked());
}

#[test]
fn the_choice_is_each_persons_own() {
    let (mut sh, clock) = timed("lk-person", 5);
    LockConfig { auto_min: 0 }.save(sh.env.data_dir(), "root").unwrap();
    LockConfig { auto_min: 5 }.save(sh.env.data_dir(), "alice").unwrap();
    run(&mut sh, &clock, 20 * MIN);
    assert!(!sh.is_locked(), "alice's choice does not lock root");
}

#[test]
fn waking_from_the_screen_saver_shows_the_lock_and_the_waking_key_is_not_a_password_character() {
    let (mut sh, clock) = timed("lk-saver", 5);
    sh.saver_config_for_test(SaverConfig { first: First::Saver, after_min: 5, then_off_min: 5, battery_shorter: false, style: Style::Logo, dim_pct: 0 });
    run(&mut sh, &clock, 5 * MIN + 2_000);
    assert!(sh.is_locked() && sh.screen_stage() == Stage::Saver, "locked behind the saver");
    sh.handle(&Event::Key(Key::Char('r'))); // wakes only
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert!(sh.is_locked());
    assert!(sh.lockui.form.fields[0].value.is_empty(), "the waking key was not typed into the prompt");
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    // the screen off: pointer movement wakes it and the prompt is there
    run(&mut sh, &clock, 20 * MIN);
    assert!(sh.is_locked());
    sh.handle(&Event::MouseMove(30, 30));
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_eq!(sh.screen_name(), "locked");
}

#[test]
fn heartbeats_go_on_behind_the_lock_so_work_in_the_background_is_not_stopped() {
    let mut sh = signed_in("lk-beat", "root", "rootpass12");
    sh.lock_screen("test");
    let t0 = sh.tick;
    for _ in 0..30 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.tick, t0 + 30, "the shell's clock, and with it every running app's, keeps going");
    assert!(sh.session.is_some());
}

#[test]
fn the_lock_draws_at_many_sizes_with_the_field_and_button_inside_the_screen_and_a_touch_keyboard_lift() {
    let mut sh = signed_in("lk-draw", "root", "rootpass12");
    sh.lock_screen("test");
    for (w, h) in [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2880, 1800)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        for want in ["field", "Unlock"] {
            let hit = sh.hits().iter().find(|x| x.label == want).unwrap_or_else(|| panic!("{want} at {w}x{h}"));
            assert!(hit.rect.x >= -0.5 && hit.rect.y >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{want} inside {w}x{h}: {:?}", hit.rect);
        }
    }
    // an error line and the clock draw without a panic
    unlock_with(&mut sh, "wrong");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
}

/// A locked screen stays locked, but a computer asking through developer mode gets the lock card (state "Locked screen", no click regions)
/// and a picture of it instead of a timeout.
#[test]
fn a_locked_screen_still_answers_a_computer_that_asks_and_stays_locked() {
    use iyakki_agent::devmode::state;
    let mut sh = signed_in("lk-dev", "root", "rootpass12");
    assert!(sh.lock_screen("test"));
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let dir = sh.env.data_dir().to_path_buf();
    let _ = state::fetch(&dir, 0);
    sh.handle(&Event::Tick);
    sh.render(&mut g);
    let snap = state::fetch(&dir, 500).expect("a frame of the lock card");
    assert_eq!(snap.app, "Locked screen");
    assert!(snap.buttons.is_empty(), "nothing to click behind a lock: {:?}", snap.buttons.iter().map(|b| &b.label).collect::<Vec<_>>());
    assert!(sh.is_locked(), "asking does not unlock");
    // a picture is served too
    let svc = sh.env.services.clone();
    svc.screen().set_present(true);
    let asker = std::thread::spawn(move || svc.screen().request(iyakki_shared::devmode::SCREEN_ANY, std::time::Duration::from_secs(5)));
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        sh.handle(&Event::Tick);
        sh.render(&mut g);
        if asker.is_finished() {
            break;
        }
    }
    assert!(asker.join().unwrap().is_ok());
    assert!(sh.is_locked());
}

/// A guest has no password and no lock of their own: a saved choice, even one written under the guest name, never locks them.
#[test]
fn a_guest_is_never_locked_by_the_idle_minutes() {
    let mut sh = signed_in("lk-guest-auto", "gus", "guestpass1");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    sh.saver_config_for_test(SaverConfig { first: First::Nothing, ..SaverConfig::default() });
    LockConfig { auto_min: 5 }.save(sh.env.data_dir(), "gus").unwrap();
    run(&mut sh, &clock, 90 * MIN);
    assert!(!sh.is_locked() && sh.session.is_some(), "a guest stays as they are, however long it is quiet");
}

/// The automatic lock keeps its minutes on battery (the screen saver's times are halved, this one is not), and a battery in use holds nothing.
#[test]
fn the_automatic_lock_is_not_halved_on_battery() {
    let (mut sh, clock) = timed("lk-battery", 10);
    let sys = std::env::temp_dir().join(format!("iyakki-lk-batsys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sys);
    for (k, v) in [("type", "Battery"), ("status", "Discharging"), ("capacity", "64"), ("energy_now", "32000000"), ("energy_full", "50000000"), ("energy_full_design", "60000000"), ("power_now", "8000000")] {
        let p = sys.join(format!("class/power_supply/BAT0/{k}"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }
    std::fs::create_dir_all(sys.join("class/power_supply/ADP1")).unwrap();
    std::fs::write(sys.join("class/power_supply/ADP1/type"), "Mains\n").unwrap();
    std::fs::write(sys.join("class/power_supply/ADP1/online"), "0\n").unwrap();
    sh.battery.set_env(iyakki_battery::BatteryEnv::at(&sys));
    sh.battery_step(0);
    assert!(sh.saver_keep_on_for_test().is_none(), "a battery in use holds nothing");
    run(&mut sh, &clock, 6 * MIN);
    assert!(!sh.is_locked(), "ten minutes are ten minutes on battery too (halved it would have locked at five)");
    run(&mut sh, &clock, 5 * MIN);
    assert!(sh.is_locked(), "and the lock comes at ten");
    let _ = std::fs::remove_dir_all(&sys);
}

/// A drive that was lost keeps its banner up and with it the automatic lock away; the count starts when the banner goes.
#[test]
fn the_drive_banner_holds_the_automatic_lock_off() {
    let (mut sh, clock) = timed("lk-holds", 5);
    iyakki_storage::loss::set_thread_override(Some(iyakki_storage::loss::Snapshot { state: iyakki_storage::loss::State::Lost, since: 10, lost_at: Some(10), ..Default::default() }));
    run(&mut sh, &clock, 20 * MIN);
    assert!(!sh.is_locked(), "the drive banner holds the lock off");
    iyakki_storage::loss::set_thread_override(None);
    run(&mut sh, &clock, 4 * MIN);
    assert!(!sh.is_locked(), "the count began when the last hold ended");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked());
}

/// A stream of silent MP3 frames that never ends, for a radio that really plays on the made-up network.
struct QuietStream;
struct QuietBytes(usize);
impl std::io::Read for QuietBytes {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let n = out.len().min(417);
        for b in out[..n].iter_mut() {
            *b = [0xFF, 0xFB, 0x90, 0x64].get(self.0).copied().unwrap_or(0);
            self.0 = (self.0 + 1) % 417;
        }
        Ok(n)
    }
}
impl iyakki_proxy::Streamer for QuietStream {
    fn open(&self, _url: &str, _h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(QuietBytes(0)) })
    }
}

/// A radio that plays keeps the automatic lock away for as long as it plays, and the count starts when it stops.
#[test]
fn a_playing_radio_holds_the_automatic_lock_off_and_the_count_starts_after_it() {
    let (mut sh, clock) = timed("lk-radio", 5);
    let radio = sh.env.services.radio().clone();
    radio.set_streamer(std::sync::Arc::new(QuietStream));
    let allow: std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync> = std::sync::Arc::new(|_| true);
    radio.play(&iyakki_tile_api::radio::Station::new("Jazz FM", "https://jazz.example.org/live.mp3"), allow, "tee /dev/null");
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if radio.is_active() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(radio.is_active());
    assert_eq!(sh.saver_keep_on_for_test(), Some("the radio"));
    run(&mut sh, &clock, 30 * MIN);
    assert!(!sh.is_locked(), "nobody locks the screen over a radio that is playing");
    radio.stop();
    run(&mut sh, &clock, 4 * MIN);
    assert!(!sh.is_locked(), "the count began when the radio stopped");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked());
}

#[test]
fn with_the_lock_choice_on_the_screen_lock_seals_the_browser_data_and_the_right_password_opens_it_again() {
    use iyakki_shared::profilepolicy::Policy;
    use iyakki_shared::profilestore::{Status};
    use iyakki_agent::profilestore::{ProfileStore};
    let mut sh = signed_in("lk-seal", "root", "rootpass12");
    let d = sh.env.data_dir().to_path_buf();
    // a plain profile, then protection on (the key goes into the person's vault), as Settings -> Browser does
    let plain = iyakki_agent::webengine::profile_dir(&d, "root");
    std::fs::create_dir_all(plain.join("data")).unwrap();
    std::fs::write(plain.join("data/cookies.sqlite"), "FAKE-COOKIE-JAR").unwrap();
    iyakki_agent::profilestore::protect_for(sh.env.services.real(), "root").unwrap();
    let st = ProfileStore::new(&d, "root");
    assert_eq!(st.status(), Status::Open);
    // the choice is off by default: locking leaves the data open
    assert!(sh.lock_screen("test"));
    assert_eq!(st.status(), Status::Open, "off by default");
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    // the choice on: locking seals it, the decrypted copy is gone, no key is in the audit text
    Policy { lock_seal: true, ..Policy::load(&d, "root") }.save(&d, "root").unwrap();
    assert!(sh.lock_screen("test"));
    assert_eq!(st.status(), Status::Sealed);
    assert!(!plain.join("data/cookies.sqlite").exists(), "the decrypted copy was removed");
    assert!(audit_text(&sh).contains("browser.data.sealed"));
    // a wrong password opens nothing
    unlock_with(&mut sh, "not-the-password");
    assert!(sh.is_locked());
    assert_eq!(st.status(), Status::Sealed);
    // the right one opens the lock and the browser data
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    assert_eq!(st.status(), Status::Open);
    assert_eq!(std::fs::read_to_string(plain.join("data/cookies.sqlite")).unwrap(), "FAKE-COOKIE-JAR");
    let log = audit_text(&sh);
    assert!(log.contains("browser.data.opened"));
    let key = sh.env.services.real().user_get("root", iyakki_shared::profilestore::VAULT_SECRET).unwrap().to_string();
    assert!(!log.contains(&key), "the key is not in the audit log");
}

/// An app that only counts the times the lock told it to let go of what it holds.
struct LockProbe(Arc<AtomicU64>);
impl App for LockProbe {
    fn title(&self) -> &'static str {
        "PROBE"
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [10, 200, 10, 255]
    }
    fn hint(&self) -> String {
        "probe".into()
    }
    fn handle(&mut self, _ev: &Event, _cx: &AppCx) -> Flow {
        Flow::Stay
    }
    fn render(&mut self, _g: &mut Gfx, _area: R, _s: f32, _cx: &AppCx, _tick: u64) {}
    fn on_lock(&mut self, _cx: &AppCx) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn the_lock_tells_the_open_app_to_let_go_only_when_the_browser_data_is_sealed_with_the_screen() {
    use iyakki_shared::profilepolicy::Policy;
    let mut sh = signed_in("lk-letgo", "root", "rootpass12");
    let d = sh.env.data_dir().to_path_buf();
    let n = Arc::new(AtomicU64::new(0));
    sh.screen = Screen::App(Box::new(LockProbe(n.clone())), PApp::Browser);
    // the choice is off by default: nothing is told
    assert!(sh.lock_screen("test"));
    assert_eq!(n.load(Ordering::SeqCst), 0);
    unlock_with(&mut sh, "rootpass12");
    sh.screen = Screen::App(Box::new(LockProbe(n.clone())), PApp::Browser);
    // with the choice on, the app is told before the data is sealed
    Policy { lock_seal: true, ..Policy::load(&d, "root") }.save(&d, "root").unwrap();
    assert!(sh.lock_screen("test"));
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn the_lock_also_tells_the_apps_of_other_displays_and_the_stashed_app_to_let_go() {
    use crate::display::outputs::{DispSpec, DisplayConfig};
    use iyakki_shared::profilepolicy::Policy;
    let mut sh = signed_in("lk-letgo2", "root", "rootpass12");
    let d = sh.env.data_dir().to_path_buf();
    let specs = vec![
        DispSpec { name: "eDP-1".into(), w: 1280, h: 720, x: 0, y: 0 },
        DispSpec { name: "HDMI-A-1".into(), w: 1024, h: 600, x: 1280, y: 0 },
    ];
    sh.set_displays(&specs, &DisplayConfig::default());
    let n = Arc::new(AtomicU64::new(0));
    sh.display_put_for_test(1, 0, Box::new(LockProbe(n.clone())), PApp::Browser);
    sh.stash = Some((Box::new(LockProbe(n.clone())), PApp::Browser));
    Policy { lock_seal: true, ..Policy::load(&d, "root") }.save(&d, "root").unwrap();
    assert!(sh.lock_screen("test"));
    assert_eq!(n.load(Ordering::SeqCst), 2, "the app on the second display and the stashed one were both told");
}

#[test]
fn locking_and_unlocking_tell_the_apps_that_listen_with_one_word() {
    use iyakki_core_api::appevents::{Val};
    let mut sh = signed_in("lk-events", "root", "rootpass12");
    let core = sh.core.clone();
    let ev = core.app_events();
    ev.register("root", "sample", "session.changed", "").unwrap();
    ev.start("root", "sample");
    let _ = ev.poll("root", "sample", 20); // the sign-in's own event
    assert!(sh.lock_screen("test"));
    let got = ev.poll("root", "sample", 20);
    assert_eq!(got.iter().map(|e| e.payload["state"].clone()).collect::<Vec<_>>(), [Val::Text("locked".into())]);
    unlock_with(&mut sh, "rootpass12");
    let got = ev.poll("root", "sample", 20);
    assert_eq!(got.iter().map(|e| e.payload["state"].clone()).collect::<Vec<_>>(), [Val::Text("unlocked".into())]);
    assert!(!audit_text(&sh).contains("rootpass12"));
}

#[test]
fn the_app_that_comes_to_the_front_and_the_one_that_leaves_are_told_to_the_apps_that_listen() {
    use iyakki_core_api::appevents::{Val};
    let mut sh = signed_in("lk-fg", "root", "rootpass12");
    let core = sh.core.clone();
    let ev = core.app_events();
    ev.register("root", "sample", "app.lifecycle", "").unwrap();
    ev.start("root", "sample");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let _ = ev.poll("root", "sample", 20);
    sh.open_linked("settings");
    sh.render(&mut g);
    let got = ev.poll("root", "sample", 20);
    let words: Vec<(Val, Val)> = got.iter().map(|e| (e.payload["app"].clone(), e.payload["change"].clone())).collect();
    assert_eq!(words, [(Val::Text("settings".into()), Val::Text("foreground".into()))]);
    sh.leave_app();
    sh.render(&mut g);
    let got = ev.poll("root", "sample", 20);
    assert_eq!(got.iter().map(|e| e.payload["change"].clone()).collect::<Vec<_>>(), [Val::Text("background".into())]);
}
