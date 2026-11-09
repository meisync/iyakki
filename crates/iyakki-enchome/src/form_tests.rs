use super::form::*;
use super::tests::*;
use super::*;

fn type_all(f: &mut Form, s: &str) {
    for c in s.chars() {
        f.type_text(&c.to_string());
    }
}

fn cx<'a>(r: &'a Rig, user: &'a str, pw: &'a dyn Fn(&str) -> bool) -> Ctx<'a> {
    Ctx { env: &r.env, user, verify_signin: pw }
}

fn recovery_of(f: &Form) -> String {
    match &f.flow {
        Flow::ShowRecovery { text, .. } => text.to_string(),
        _ => panic!("the recovery card is not showing: {:?}", f.notice),
    }
}

#[test]
fn creating_shows_the_key_once_and_it_counts_as_saved_only_when_typed_back() {
    let r = rig("form-create");
    r.home("amy");
    let ok = |p: &str| p == "amy-pass-1";
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::Create, &c);
    type_all(&mut f, "not-my-password");
    f.submit(&c);
    assert!(f.notice.as_deref().unwrap().contains("not your sign-in password"));
    assert!(!r.env.exists("amy"), "nothing is made for a wrong password");
    f.cancel();
    f.start(Purpose::Create, &c);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    let key = recovery_of(&f);
    assert!(r.env.status("amy").state == State::Open && !r.env.meta("amy").recovery_confirmed);
    // pressing the button with nothing or a wrong key typed does not count as saved
    f.submit(&c);
    assert!(matches!(f.flow, Flow::ShowRecovery { .. }) && !r.env.meta("amy").recovery_confirmed);
    type_all(&mut f, &key[..key.len() - 1]);
    f.submit(&c);
    assert!(!r.env.meta("amy").recovery_confirmed);
    type_all(&mut f, &key);
    f.submit(&c);
    assert!(r.env.meta("amy").recovery_confirmed && matches!(f.flow, Flow::Done));
    // no message ever holds the password or the key
    let n = f.notice.unwrap();
    assert!(!n.contains(&key) && !n.contains("amy-pass-1"));
}

#[test]
fn backspace_edits_the_masked_field_and_cancel_forgets_it() {
    let r = rig("form-edit");
    r.home("amy");
    let ok = |p: &str| p == "ab";
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::Create, &c);
    type_all(&mut f, "abx");
    f.backspace();
    f.submit(&c);
    assert!(matches!(f.flow, Flow::ShowRecovery { .. }));
    f.cancel();
    assert!(f.is_idle());
}

#[test]
fn unlock_now_with_the_password_and_a_wrong_one_says_the_same_generic_thing() {
    let r = rig("form-unlock");
    r.made("amy", "amy-pass-1");
    r.env.lock("amy").unwrap();
    let ok = |_: &str| true;
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::Unlock, &c);
    type_all(&mut f, "wrong");
    f.submit(&c);
    assert_eq!(f.notice.as_deref(), Some("The password or recovery key is wrong."));
    r.clock.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    assert_eq!(r.env.status("amy").state, State::Open);
    let mut g = Form::default();
    g.start(Purpose::Unlock, &c);
    assert!(g.is_idle() && g.notice.unwrap().contains("already open"));
}

#[test]
fn the_recovery_key_route_checks_typing_then_sets_the_current_password() {
    let r = rig("form-rec");
    let rk = r.made("amy", "old-pass-1");
    r.env.lock("amy").unwrap();
    r.env.note_admin_reset("amy");
    let ok = |p: &str| p == "reset-by-admin-1";
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::UnlockWithRecovery, &c);
    type_all(&mut f, "AAAA-BBBB");
    f.submit(&c);
    assert!(f.notice.as_deref().unwrap().contains("not written correctly"));
    f.cancel();
    f.start(Purpose::UnlockWithRecovery, &c);
    type_all(&mut f, &rk);
    f.submit(&c);
    assert!(matches!(f.flow, Flow::Ask { stage: 1, .. }));
    type_all(&mut f, "wrong-signin");
    f.submit(&c);
    assert!(f.notice.as_deref().unwrap().contains("not your sign-in password"));
    type_all(&mut f, "reset-by-admin-1");
    f.submit(&c);
    assert_eq!(r.env.status("amy").state, State::Open);
    assert!(!r.env.status("amy").meta.sealed_by_reset);
    r.env.lock("amy").unwrap();
    r.clock.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
    r.env.unlock("amy", &Secret::password("reset-by-admin-1")).unwrap();
}

#[test]
fn changing_the_recovery_key_and_the_admin_slot_go_through_the_password() {
    let r = rig("form-slots");
    let old = r.made("amy", "amy-pass-1");
    let ok = |p: &str| p == "amy-pass-1";
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::ChangeRecovery, &c);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    let new = recovery_of(&f);
    assert_ne!(old, new);
    type_all(&mut f, &new);
    f.submit(&c);
    assert!(r.env.meta("amy").recovery_confirmed);
    // admin slot: person's password, then the administrator's
    f.start(Purpose::AdminSlot { enable: true }, &c);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    assert!(matches!(f.flow, Flow::Ask { stage: 1, .. }));
    type_all(&mut f, "the-admin-secret");
    f.submit(&c);
    assert!(r.env.status("amy").has_admin_slot());
    assert!(f.notice.as_deref().unwrap().contains("can open and read your home"));
    f.start(Purpose::AdminSlot { enable: false }, &c);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    assert!(!r.env.status("amy").has_admin_slot());
}

#[test]
fn start_refuses_what_makes_no_sense() {
    let r = rig("form-refuse");
    let ok = |_: &str| true;
    let c = cx(&r, "amy", &ok);
    for p in [Purpose::Unlock, Purpose::ChangeRecovery, Purpose::AdminSlot { enable: true }, Purpose::UnlockWithRecovery] {
        let mut f = Form::default();
        f.start(p, &c);
        assert!(f.is_idle() && f.notice.unwrap().contains("make one first"));
    }
    r.made("amy", "amy-pass-1");
    for p in [Purpose::Create, Purpose::Migrate] {
        let mut f = Form::default();
        f.start(p, &c);
        assert!(f.is_idle() && f.notice.unwrap().contains("already has"));
    }
}

#[test]
fn the_migration_form_shows_the_plan_first_and_only_then_moves() {
    let r = rig("form-migrate");
    let h = r.home("amy");
    std::fs::write(h.join("a.txt"), b"alpha").unwrap();
    let ok = |p: &str| p == "amy-pass-1";
    let c = cx(&r, "amy", &ok);
    let mut f = Form::default();
    f.start(Purpose::Migrate, &c);
    type_all(&mut f, "amy-pass-1");
    f.submit(&c);
    assert!(matches!(f.flow, Flow::MigratePlan { .. }));
    assert!(f.notice.as_deref().unwrap().contains("COPIED"));
    assert!(!r.env.exists("amy"), "the plan changes nothing");
    f.submit(&c);
    let key = recovery_of(&f);
    assert_eq!(std::fs::read(h.join("a.txt")).unwrap(), b"alpha");
    assert!(r.env.plain_copy_path("amy").is_dir());
    type_all(&mut f, &key);
    f.submit(&c);
    assert!(r.env.meta("amy").recovery_confirmed);
}

#[test]
fn requests_from_the_assistant_are_one_word_taken_once_and_never_carry_a_secret() {
    let d = std::env::temp_dir().join(format!("iyakki-enchome-req-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(take_request(&d, "amy"), None);
    for r in [Request::Create, Request::Unlock, Request::ChangeRecovery, Request::AdminSlotOn, Request::AdminSlotOff, Request::Migrate] {
        leave_request(&d, "amy", r);
        assert_eq!(take_request(&d, "amy"), Some(r));
        assert_eq!(take_request(&d, "amy"), None);
    }
    std::fs::write(d.join("enchome-request-amy"), "create; rm -rf /").unwrap();
    assert_eq!(take_request(&d, "amy"), None, "anything but a known word is ignored");
    leave_request(&d, "../../etc/x", Request::Create);
    assert!(std::fs::read_dir(&d).unwrap().flatten().all(|e| e.path().parent() == Some(d.as_path())));
    let _ = std::fs::remove_dir_all(d);
}
