//! EDIT and CLI on the UI SDK next to the old screens (F-SDK-34): the old screen opens until the person chooses the SDK version, the choice
//! is the person's own, and the tile that opens keeps the old app's id (so the policy, the dock and the audit are the old app's own).

use super::tests::*;
use super::*;

fn hint(sh: &Shell) -> String {
    match &sh.screen {
        Screen::App(a, _) => a.hint(),
        _ => panic!("no app is open"),
    }
}

fn choose(sh: &Shell, user: &str, text: &str) {
    sh.cx_for_test().unwrap().core.app_data().set(user, iyakki_shared::sdkapps::APP, iyakki_shared::sdkapps::KEY, text).unwrap();
}

#[test]
fn the_old_screen_opens_until_the_person_chooses_the_sdk_version_and_the_choice_is_per_app() {
    let (mut sh, _) = shell_with_users("sdkapps-open");
    login(&mut sh, "root", "rootpass12");
    sh.open_for_test(PApp::Cli);
    assert!(!hint(&sh).starts_with("SDK version"), "nothing was chosen: {}", hint(&sh));
    sh.open_for_test(PApp::Edit);
    assert!(!hint(&sh).starts_with("SDK version"), "{}", hint(&sh));
    choose(&sh, "root", "cli");
    sh.open_for_test(PApp::Cli);
    assert!(hint(&sh).starts_with("SDK version"), "the SDK version of CLI: {}", hint(&sh));
    assert!(matches!(&sh.screen, Screen::App(a, PApp::Cli) if a.title() == "CLI"), "it is the CLI app of the same id");
    sh.open_for_test(PApp::Edit);
    assert!(!hint(&sh).starts_with("SDK version"), "EDIT was not chosen: {}", hint(&sh));
    choose(&sh, "root", "edit cli");
    sh.open_for_test(PApp::Edit);
    assert!(hint(&sh).starts_with("SDK version") && matches!(&sh.screen, Screen::App(a, PApp::Edit) if a.title() == "EDIT"));
    // taking the choice back opens the old screen again
    choose(&sh, "root", "");
    sh.open_for_test(PApp::Edit);
    assert!(!hint(&sh).starts_with("SDK version"));
}

#[test]
fn the_choice_of_one_person_does_not_change_another_persons_apps_and_the_open_is_audited_as_the_old_app() {
    let (mut sh, _) = shell_with_users("sdkapps-person");
    login(&mut sh, "root", "rootpass12");
    choose(&sh, "alice", "cli");
    sh.open_for_test(PApp::Cli);
    assert!(!hint(&sh).starts_with("SDK version"), "root did not choose it: {}", hint(&sh));
    choose(&sh, "root", "cli");
    sh.open_for_test(PApp::Cli);
    assert!(hint(&sh).starts_with("SDK version"));
    let log = sh.env.audit.recent(40).join("\n");
    assert!(log.contains("app.open cli"), "{log}");
}
