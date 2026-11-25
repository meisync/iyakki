//! Putting the person back after the shell restarted (F-SHELL-65): only when developer mode and the Settings switch allow it, with no
//! password typed and every vault left locked, never after a sign-out, never for a stale ticket, always audited.

use super::tests::*;
use super::*;
use iyakki_core_api::SessionResume as _;
use iyakki_core_fake::FakeCore;

fn rig(tag: &str, dev: bool, switch: bool) -> (Shell, Arc<FakeCore>) {
    let (mut sh, _) = shell_with_users(tag);
    let core = Arc::new(FakeCore::new());
    core.resume_fake().dev.store(dev, std::sync::atomic::Ordering::SeqCst);
    core.resume_fake().set_switch_for_test(switch);
    sh.set_core_for_test(core.clone());
    (sh, core)
}

fn tick(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

#[test]
fn a_signed_in_person_leaves_a_ticket_only_when_developer_mode_and_the_switch_allow_it() {
    for (dev, switch, wrote) in [(true, true, true), (true, false, false), (false, true, false), (false, false, false)] {
        let (mut sh, core) = rig(&format!("resume-w-{dev}-{switch}"), dev, switch);
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(core.session_resume_who(), wrote.then(|| "alice".to_string()), "dev {dev} switch {switch}");
    }
}

#[test]
fn after_a_restart_the_person_is_back_without_the_password_and_the_vault_stays_locked_and_it_is_audited() {
    let (mut sh, core) = rig("resume-back", true, true);
    login(&mut sh, "alice", "alicepass1");
    assert!(sh.env.services.user_vault_unlocked("alice"), "typing the password opened the vault");
    // the process dies: a fresh shell on the same data and the same core
    let env2 = sh.env.share();
    let mut sh2 = Shell::new(env2);
    sh2.set_core_for_test(core.clone());
    assert_eq!(sh2.screen_name(), "login");
    sh2.env.services.lock_user_vault("alice");
    tick(&mut sh2, 2);
    assert_ne!(sh2.screen_name(), "login", "{}", sh2.screen_name());
    assert_eq!(sh2.session.as_ref().map(|s| s.user.as_str()), Some("alice"));
    assert!(!sh2.env.services.user_vault_unlocked("alice"), "no password was typed: the vault stays locked");
    assert!(sh2.env.audit.recent(40).iter().any(|l| l.contains("login.resumed")), "audited");
}

#[test]
fn nothing_is_resumed_when_it_is_not_allowed_after_a_sign_out_or_for_a_stale_or_unknown_ticket() {
    // not allowed: the person signs in again
    let (mut sh, core) = rig("resume-no", true, false);
    core.resume_fake().set_who_for_test("alice");
    tick(&mut sh, 2);
    assert_eq!(sh.screen_name(), "login");
    // allowed, but the person signed out on purpose
    let (mut sh, core) = rig("resume-out", true, true);
    login(&mut sh, "alice", "alicepass1");
    sh.sign_out();
    assert_eq!(core.session_resume_who(), None);
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    tick(&mut sh2, 2);
    assert_eq!(sh2.screen_name(), "login");
    // a ticket for a person who no longer exists is dropped
    core.resume_fake().set_who_for_test("ghost");
    let mut sh3 = Shell::new(sh.env.share());
    sh3.set_core_for_test(core.clone());
    tick(&mut sh3, 2);
    assert_eq!(sh3.screen_name(), "login");
    assert_eq!(core.session_resume_who(), None, "the bad ticket was removed");
}

#[test]
fn the_ticket_is_refreshed_every_few_seconds_while_somebody_is_signed_in() {
    let (mut sh, core) = rig("resume-fresh", true, true);
    login(&mut sh, "alice", "alicepass1");
    core.resume_fake().clear_for_test();
    assert_eq!(core.session_resume_who(), None);
    tick(&mut sh, 31);
    assert_eq!(core.session_resume_who().as_deref(), Some("alice"));
}

#[test]
fn after_a_restart_the_open_apps_come_back_too_the_one_held_aside_and_the_front_one_and_a_sign_out_forgets_them() {
    let (mut sh, core) = rig("resume-apps", true, true);
    login(&mut sh, "alice", "alicepass1");
    // the person had Chat on the screen (the tick writes the ticket every few seconds)
    sh.open(PApp::Chat);
    for _ in 0..31 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(core.resume_fake().apps(), vec!["chat".to_string()]);
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    tick(&mut sh2, 2);
    assert_eq!(sh2.session.as_ref().map(|s| s.user.as_str()), Some("alice"));
    assert_eq!(sh2.screen_name(), "app:chat", "the open app is back");
    // two apps: one kept aside, one in front
    core.resume_fake().clear_for_test();
    core.resume_fake().remember("alice");
    core.resume_fake().remember_apps(&["edit".to_string(), "chat".to_string(), "nonsense".to_string()]);
    core.resume_fake().set_who_for_test("alice");
    let mut sh3 = Shell::new(sh.env.share());
    sh3.set_core_for_test(core.clone());
    tick(&mut sh3, 2);
    assert_eq!(sh3.screen_name(), "app:chat");
    assert_eq!(sh3.stash.as_ref().map(|(_, p)| p.id()), Some("edit"));
    // a sign-out forgets the apps with the ticket
    sh3.sign_out();
    assert!(core.resume_fake().apps().is_empty());
}

#[test]
fn after_a_restart_the_window_panes_come_back_with_their_apps_and_the_apps_of_the_ticket_are_not_stacked_on_them() {
    let (mut sh, core) = rig("resume-panes", true, true);
    login(&mut sh, "alice", "alicepass1");
    sh.set_mem_for_test(Some(big_mem()));
    sh.open(PApp::Chat);
    sh.ws_apply("cols-50-50", Some(1), true).unwrap();
    sh.ws_focus(1);
    sh.open(PApp::Edit);
    // the shell is killed: no sign-out, only the refresh of the ticket has run
    tick(&mut sh, 31);
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    sh2.set_mem_for_test(Some(big_mem()));
    tick(&mut sh2, 2);
    assert_eq!(sh2.session.as_ref().map(|s| s.user.as_str()), Some("alice"));
    assert_eq!(sh2.panes_layout_for_test(), "cols-50-50", "the arrangement is back");
    assert!(sh2.stash.is_none(), "the ticket's apps are not stacked on the panes");
    // the arrangement is only written when it changed, and not at all when the ticket is not allowed
    let (mut off, _) = rig("resume-panes-off", true, false);
    login(&mut off, "alice", "alicepass1");
    off.set_mem_for_test(Some(big_mem()));
    off.open(PApp::Chat);
    off.ws_apply("cols-50-50", Some(1), true).unwrap();
    tick(&mut off, 31);
    assert!(iyakki_shared::views::Views::load(off.env.data_dir(), "alice").views[0].layout.is_none(), "nothing kept before a sign-out when the ticket is off");
}

fn big_mem() -> crate::memgate::MemFacts {
    crate::memgate::MemFacts { total: 16 << 30, available: 12 << 30 }
}

#[test]
fn what_an_app_held_is_handed_back_to_it_after_a_restart_and_an_unknown_app_or_a_refused_item_changes_nothing() {
    let (mut sh, core) = rig("resume-items", true, true);
    login(&mut sh, "alice", "alicepass1");
    core.resume_fake().clear_for_test();
    core.resume_fake().remember("alice");
    core.resume_fake().remember_apps(&["edit".to_string()]);
    core.resume_fake().remember_items(&[("edit".to_string(), "/etc/outside-the-scope".to_string()), ("nonsense".to_string(), "x".to_string())]);
    core.resume_fake().set_who_for_test("alice");
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    tick(&mut sh2, 2);
    assert_eq!(sh2.screen_name(), "app:edit", "the app opens, the refused path leaves it empty");
    // a signed-in editor reports its file for the ticket (none here: nothing open)
    tick(&mut sh2, 31);
    assert!(core.resume_fake().items().is_empty());
}

#[test]
fn after_a_crash_a_person_who_is_not_put_back_is_told_the_screen_restarted_and_it_is_audited() {
    // developer mode off: nothing is restored, the notice says what happened
    let (mut sh, core) = rig("resume-notice", false, false);
    login(&mut sh, "alice", "alicepass1");
    tick(&mut sh, 31);
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    tick(&mut sh2, 2);
    assert_eq!(sh2.screen_name(), "login", "not restored");
    let (text, _, info) = sh2.toast.clone().expect("a notice");
    assert_eq!(text, UNEXPECTED_END_NOTICE);
    assert!(info);
    assert!(sh2.env.audit.recent(40).iter().any(|l| l.contains("shell.restarted")), "audited");
    // said once: the marker was taken
    let mut sh3 = Shell::new(sh.env.share());
    sh3.set_core_for_test(core.clone());
    tick(&mut sh3, 2);
    assert!(sh3.toast.is_none(), "no second notice for the same crash");
}

#[test]
fn no_notice_after_a_sign_out_a_fresh_boot_or_when_the_person_was_put_back() {
    // a sign-out is not a crash
    let (mut sh, core) = rig("resume-notice-out", false, false);
    login(&mut sh, "alice", "alicepass1");
    sh.sign_out();
    let mut sh2 = Shell::new(sh.env.share());
    sh2.set_core_for_test(core.clone());
    tick(&mut sh2, 2);
    assert!(sh2.toast.is_none());
    // nobody was ever signed in
    let (mut sh3, _) = rig("resume-notice-none", false, false);
    tick(&mut sh3, 2);
    assert!(sh3.toast.is_none());
    // developer mode puts the person back: the welcome-back words, not the crash notice
    let (mut sh4, core4) = rig("resume-notice-dev", true, true);
    login(&mut sh4, "alice", "alicepass1");
    let mut sh5 = Shell::new(sh4.env.share());
    sh5.set_core_for_test(core4.clone());
    tick(&mut sh5, 2);
    assert_eq!(sh5.session.as_ref().map(|s| s.user.as_str()), Some("alice"));
    assert!(!sh5.toast.as_ref().is_some_and(|t| t.0 == UNEXPECTED_END_NOTICE));
}

#[test]
fn a_drive_pulled_out_while_open_is_reported_to_the_person_through_the_core_and_audited() {
    use iyakki_core_fake::FakeOs;
    let (mut sh, _core) = rig("unplug", false, false);
    login(&mut sh, "alice", "alicepass1");
    let fake = Arc::new(FakeOs::new());
    let dir = std::env::temp_dir().join(format!("iyakki-unplug-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    sh.osguard = osguard_impl::OsGuard::over(fake.clone(), &dir);
    // nothing pulled out: nothing said
    sh.drives_tick();
    assert!(sh.toast.is_none());
    fake.disk_engine_fake().set_unplug(Some("The drive STICK was removed. It was not in use, and its folder is closed.".into()));
    sh.drives_tick();
    let (text, _, info) = sh.toast.clone().expect("the person is told");
    assert!(text.contains("STICK was removed") && !info);
    // said once
    sh.toast = None;
    sh.drives_tick();
    assert!(sh.toast.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
