//! A client's display-list mode in the shell (F-SDK-29): the checks of opening an app, the frames, the input lines and the audit.

use super::tests::*;
use super::*;

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path_buf()).unwrap_or_default()
}

#[test]
fn an_sdk_app_opens_for_a_signed_in_person_gives_frames_and_takes_input_lines_and_is_audited() {
    let (mut sh, _h) = shell_with_users("listapp-ok");
    assert_eq!(sh.list_app_open("sample", 800, 600).unwrap_err().code, "denied", "nobody is signed in");
    login(&mut sh, "root", "rootpass12");
    let first = sh.list_app_open("sample", 800, 600).expect("the first frame");
    assert!(first.starts_with("iyakki-display-list"), "{}", &first[..first.len().min(60)]);
    assert_eq!(sh.list_app_id(), Some("sample"));
    // a key reaches the app and the next frame comes back; the app is still open
    let (next, left) = sh.list_app_input("key tab\nresize 640 480").expect("a frame");
    assert!(next.starts_with("iyakki-display-list") && !left);
    assert_eq!(sh.list_app_id(), Some("sample"));
    assert!(sh.list_app_close() && !sh.list_app_close(), "closing twice is harmless");
    let a = audit_text(&sh);
    assert!(a.contains("client.app.open") && a.contains("client.app.close"), "{a}");
    // the tile on the shell's own screen was never touched
    assert!(!matches!(sh.screen, Screen::App(..)), "the client's app is not the shell's screen");
}

#[test]
fn the_refusals_have_stable_words_and_a_bad_batch_applies_nothing() {
    let (mut sh, _h) = shell_with_users("listapp-refuse");
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.list_app_open("chat", 800, 600).unwrap_err().code, "not_found", "only SDK apps can be shown");
    assert_eq!(sh.list_app_open("nope", 800, 600).unwrap_err().code, "not_found");
    assert_eq!(sh.list_app_open("sample", 10, 600).unwrap_err().code, "bad_request");
    assert_eq!(sh.list_app_open("sample", 800, 9000).unwrap_err().code, "bad_request");
    assert_eq!(sh.list_app_input("key enter").unwrap_err().code, "not_open");
    sh.list_app_open("sample", 800, 600).unwrap();
    let e = sh.list_app_input("key enter\nthis is not a line").unwrap_err();
    assert_eq!(e.code, "bad_request");
    assert!(e.message.contains("line 2"), "{}", e.message);
    assert_eq!(sh.list_app_id(), Some("sample"), "a bad batch does not close the app");
}

#[test]
fn a_role_without_the_app_is_refused_and_the_refusal_is_audited() {
    let (mut sh, _h) = shell_with_users("listapp-role");
    login(&mut sh, "gus", "guestpass1");
    let e = sh.list_app_open("sample", 800, 600).unwrap_err();
    assert_eq!(e.code, "denied");
    assert!(e.message.contains("guest"), "{}", e.message);
    assert!(sh.list_app_id().is_none());
    assert!(audit_text(&sh).contains("client.app.denied"));
}

#[test]
fn leaving_the_app_closes_it_and_a_second_open_replaces_the_first() {
    let (mut sh, _h) = shell_with_users("listapp-leave");
    login(&mut sh, "root", "rootpass12");
    sh.list_app_open("markdown", 800, 600).unwrap();
    assert_eq!(sh.list_app_id(), Some("markdown"));
    sh.list_app_open("sample", 800, 600).unwrap();
    assert_eq!(sh.list_app_id(), Some("sample"), "one app at a time");
    assert!(audit_text(&sh).matches("client.app.close").count() >= 1, "the replaced app was closed and audited");
    let (_, left) = sh.list_app_input("key esc").unwrap();
    assert!(left, "Esc on the home page of the app is Back");
    assert!(sh.list_app_id().is_none(), "the person left: the app is closed");
}
