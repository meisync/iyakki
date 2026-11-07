use super::shell_tzask::{self, due};
use super::tests::*;
use super::*;
use iyakki_agent::settings::TimeSettings;

#[test]
fn the_question_is_for_an_administrator_with_a_network_who_was_never_asked_and_chose_nothing() {
    let t = TimeSettings::default();
    assert!(due(&t, true, true));
    assert!(!due(&t, false, true), "only an administrator may change the system's zone");
    assert!(!due(&t, true, false), "not before there is a network");
    assert!(!due(&TimeSettings { zone_asked: true, ..t.clone() }, true, true), "asked once, never again");
    assert!(!due(&TimeSettings { zone_auto: true, ..t.clone() }, true, true), "already automatic");
    assert!(!due(&TimeSettings { zone: "Europe/Paris".into(), ..t.clone() }, true, true), "a zone was chosen by hand");
}

fn signed_admin(tag: &str) -> (Shell, Gfx) {
    let (mut sh, _host) = shell_with_users(tag);
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    typ(&mut sh, "root");
    click_field(&mut sh, &mut g, 1);
    typ(&mut sh, "rootpass12");
    click(&mut sh, &mut g, "Sign in");
    (sh, g)
}

fn time_of(sh: &Shell) -> TimeSettings {
    sh.env.services.settings.lock().unwrap().get().time.clone()
}

#[test]
fn each_answer_is_remembered_and_utc_fixes_the_zone_without_asking_any_service() {
    for (id, label) in [(shell_tzask::UTC, "utc"), (shell_tzask::PICK, "pick"), (shell_tzask::LATER, "later")] {
        let (mut sh, mut g) = signed_admin(&format!("tzask-{label}"));
        sh.tzask.open = true;
        sh.render(&mut g);
        for name in ["Detect automatically", "Use UTC", "Choose in Settings", "Not now"] {
            assert!(sh.hits().iter().any(|h| h.label == name), "{name} is a button");
        }
        sh.handle(&Event::Tab(id));
        assert!(!sh.tzask.open, "the card closes");
        let t = time_of(&sh);
        assert!(t.zone_asked, "{label}: it is not asked again");
        assert!(!t.zone_auto, "{label}: nothing was switched on by the answer");
        assert_eq!(t.zone == "UTC", id == shell_tzask::UTC, "{label}: {t:?}");
    }
}

#[test]
fn detect_automatically_switches_the_opt_in_on_only_when_pressed() {
    let (mut sh, mut g) = signed_admin("tzask-auto");
    assert!(!time_of(&sh).zone_auto, "off until the person says yes");
    sh.tzask.open = true;
    sh.render(&mut g);
    sh.handle(&Event::Tab(shell_tzask::AUTO));
    let t = time_of(&sh);
    assert!(t.zone_asked && t.zone_auto);
}
