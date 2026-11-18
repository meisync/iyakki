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
