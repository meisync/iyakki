use super::*;

const GOLDEN: &str = include_str!("../../tests/golden/wizard_v1.json");
const FUTURE: &str = include_str!("../../tests/golden/wizard_future.json");

fn parse(t: &str) -> Vec<WizardSpec> {
    serde_json::from_str(t).unwrap()
}

fn caps(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn err_of(t: &str, declared: &[&str]) -> String {
    validate_wizards(&parse(t), declared).unwrap_err()
}

#[test]
fn the_golden_wizards_parse_validate_and_round_trip_without_loss() {
    let w = parse(GOLDEN);
    assert_eq!(w.len(), 4);
    validate_wizards(&w, &["net.connect"]).unwrap();
    for x in &w {
        x.check_runnable().unwrap();
    }
    assert_eq!(w[0].trigger.kind(), TriggerKind::OnInstall);
    assert_eq!(w[1].trigger.kind(), TriggerKind::OnUpdate);
    assert!(w[1].trigger.matches_update_from("1.4.0"));
    assert!(!w[1].trigger.matches_update_from("2.0.0"));
    assert!(!w[0].trigger.matches_update_from("1.0.0"));
    let before: Value = serde_json::from_str(GOLDEN).unwrap();
    let after = serde_json::to_value(&w).unwrap();
    assert_eq!(before, after, "writing the wizards back loses nothing");
}

#[test]
fn unknown_fields_steps_fields_rules_and_checks_are_kept_and_refused_at_run_time_not_at_parse_time() {
    let w = parse(FUTURE);
    validate_wizards(&w, &[]).unwrap();
    assert_eq!(serde_json::to_value(&w).unwrap(), serde_json::from_str::<Value>(FUTURE).unwrap(), "unknown things are written back as they were");
    assert!(w[0].extra.contains_key("future_hint") && w[0].trigger.extra.contains_key("when_idle"));
    assert!(matches!(w[0].steps[1], Step::Unknown(_)));
    let e = w[0].check_runnable().unwrap_err();
    assert!(e.contains("field kind") && e.contains("time-of-day"), "{e}");
    // each unknown thing alone is refused with its own words
    let mut x = w[0].clone();
    if let Step::Form(f) = &mut x.steps[0] {
        f.fields[0].kind = "text".into();
    }
    assert!(x.check_runnable().unwrap_err().contains("rule \"before\""));
    if let Step::Form(f) = &mut x.steps[0] {
        f.fields[0].rules.clear();
    }
    assert!(x.check_runnable().unwrap_err().contains("step kind \"ask-the-model\""));
    x.steps.remove(1);
    assert!(x.check_runnable().unwrap_err().contains("check \"gpu.present\""));
    x.steps.remove(1);
    x.check_runnable().unwrap();
    let mut t = w[0].clone();
    t.trigger.on = "on_wake".into();
    assert!(t.check_runnable().unwrap_err().contains("trigger"));
}

#[test]
fn a_newer_wizard_version_is_kept_and_refused_with_the_words_needs_a_newer_iyakki() {
    let mut w = parse(GOLDEN);
    w[0].wizard_version = 2;
    w[0].steps = vec![Step::Unknown(serde_json::json!({"kind": "holo", "id": "x"}))];
    validate_wizards(&w, &[]).unwrap();
    assert!(w[0].check_runnable().unwrap_err().contains("Needs a newer IYAKKI"));
}

#[test]
fn a_wizard_without_wizard_version_is_version_one() {
    let w = parse(r#"[{"id":"a","trigger":{"on":"reconfigure"},"steps":[{"kind":"summary","id":"s","text":"hi"}]}]"#);
    assert_eq!(w[0].wizard_version, 1);
    validate_wizards(&w, &[]).unwrap();
}

#[test]
fn limits_and_bad_shapes_are_refused_with_plain_words() {
    let step = |n: usize| (0..n).map(|i| format!(r#"{{"kind":"check","id":"c{i}","check":"capability.held","param":"notify"}}"#)).collect::<Vec<_>>().join(",");
    let w = |body: &str| format!(r#"[{{"id":"w","trigger":{{"on":"on_install"}},"steps":[{body}]}}]"#);
    assert!(err_of(&w(&step(13)), &[]).contains("1 to 12 steps"));
    assert!(err_of(&w(""), &[]).contains("1 to 12 steps"));
    assert!(err_of(&w(&format!("{},{}", step(1), step(1))), &[]).contains("used twice"));
    let fields = (0..13).map(|i| format!(r#"{{"id":"f{i}","label":"L","kind":"text"}}"#)).collect::<Vec<_>>().join(",");
    assert!(err_of(&w(&format!(r#"{{"kind":"form","id":"f","fields":[{fields}]}}"#)), &[]).contains("1 to 12 fields"));
    let long = "x".repeat(81);
    assert!(err_of(&w(&format!(r#"{{"kind":"form","id":"f","fields":[{{"id":"a","label":"{long}","kind":"text"}}]}}"#)), &[]).contains("label"));
    assert!(err_of(&w(r#"{"kind":"summary","id":"s","text":"x"},{"kind":"check","id":"c","check":"capability.held","param":"notify"}"#), &[]).contains("last step"));
    let many: Vec<String> = (0..9).map(|i| format!(r#"{{"id":"w{i}","trigger":{{"on":"reconfigure"}},"steps":[{{"kind":"summary","id":"s","text":"x"}}]}}"#)).collect();
    assert!(err_of(&format!("[{}]", many.join(",")), &[]).contains("at most 8"));
}

#[test]
fn field_rules_secrets_choices_and_paths_are_checked() {
    let f = |field: &str| format!(r#"[{{"id":"w","trigger":{{"on":"on_install"}},"steps":[{{"kind":"form","id":"f","fields":[{field}]}}]}}]"#);
    assert!(err_of(&f(r#"{"id":"t","label":"Token","kind":"secret","default":"abc"}"#), &[]).contains("no default"));
    assert!(err_of(&f(r#"{"id":"c","label":"C","kind":"choice"}"#), &[]).contains("lists its choices"));
    assert!(err_of(&f(r#"{"id":"c","label":"C","kind":"choice","choices":[{"value":"a","label":"A"}],"default":"b"}"#), &[]).contains("not one of"));
    assert!(err_of(&f(r#"{"id":"c","label":"C","kind":"choice","choices":[{"value":"a","label":"A"},{"value":"a","label":"B"}]}"#), &[]).contains("same value"));
    assert!(err_of(&f(r#"{"id":"p","label":"P","kind":"path","default":"/a/../b"}"#), &[]).contains("absolute"));
    assert!(err_of(&f(r#"{"id":"p","label":"P","kind":"path","default":"relative"}"#), &[]).contains("absolute"));
    assert!(err_of(&f(r#"{"id":"b","label":"B","kind":"toggle","default":"yes"}"#), &[]).contains("true or false"));
    assert!(err_of(&f(r#"{"id":"a","label":"A","kind":"text","rules":[{"rule":"min_len"}]}"#), &[]).contains("takes a number"));
    assert!(err_of(&f(r#"{"id":"a","label":"A","kind":"text","rules":[{"rule":"max_len","value":99999}]}"#), &[]).contains("takes a number"));
    assert!(err_of(&f(r#"{"id":"a","label":"A","kind":"text","rules":[{"rule":"one_of","value":[]}]}"#), &[]).contains("one_of"));
    assert!(err_of(&f(r#"{"id":"a","label":"A","kind":"text"},{"id":"a","label":"B","kind":"text"}"#), &[]).contains("used twice"));
    let rules = (0..7).map(|_| r#"{"rule":"no_whitespace"}"#).collect::<Vec<_>>().join(",");
    assert!(err_of(&f(&format!(r#"{{"id":"a","label":"A","kind":"text","rules":[{rules}]}}"#)), &[]).contains("at most 6 rules"));
}

#[test]
fn checks_may_use_only_declared_capabilities_and_migrations_stay_inside_the_app() {
    let w = |step: &str| format!(r#"[{{"id":"w","trigger":{{"on":"on_update","from":"<2"}},"steps":[{step}]}}]"#);
    let reach = r#"{"kind":"check","id":"c","check":"net.reach","param":"example.com:443"}"#;
    assert!(err_of(&w(reach), &[]).contains("does not declare"));
    validate_wizards(&parse(&w(reach)), &["net.connect"]).unwrap();
    assert!(err_of(&w(r#"{"kind":"check","id":"c","check":"net.reach","param":"*"}"#), &["net.connect"]).contains("network address"));
    assert!(err_of(&w(r#"{"kind":"check","id":"c","check":"net.reach"}"#), &["net.connect"]).contains("needs a param"));
    assert!(err_of(&w(r#"{"kind":"check","id":"c","check":"capability.held","param":"teleport"}"#), &[]).contains("not a capability"));
    assert!(err_of(&w(r#"{"kind":"migration","id":"m","migration":"../x"}"#), &[]).contains("inside the app"));
    assert!(err_of(&w(r#"{"kind":"migration","id":"m","migration":"/etc/passwd"}"#), &[]).contains("inside the app"));
    assert!(err_of(&w(r#"{"kind":"migration","id":"m","migration":"m.json","from":"one"}"#), &[]).contains("from is a version"));
    assert!(err_of(r#"[{"id":"w","trigger":{"on":"on_install","from":"<2"},"steps":[{"kind":"summary","id":"s","text":"x"}]}]"#, &[]).contains("on_update only"));
    assert!(err_of(r#"[{"id":"w","trigger":{"on":"on_update","from":"soon"},"steps":[{"kind":"summary","id":"s","text":"x"}]}]"#, &[]).contains("trigger.from"));
}

#[test]
fn a_known_step_with_a_wrong_body_is_a_parse_error_naming_the_kind() {
    let e = serde_json::from_str::<Vec<WizardSpec>>(r#"[{"id":"w","trigger":{"on":"on_install"},"steps":[{"kind":"form","id":"f"}]}]"#).unwrap_err().to_string();
    assert!(e.contains("form step"), "{e}");
    let e = serde_json::from_str::<Vec<WizardSpec>>(r#"[{"id":"w","trigger":{"on":"on_install"},"steps":[{"id":"f"}]}]"#).unwrap_err().to_string();
    assert!(e.contains("needs a \"kind\""), "{e}");
    let _ = caps(&[]);
}
