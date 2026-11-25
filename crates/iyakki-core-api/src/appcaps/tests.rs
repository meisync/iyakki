use super::*;

fn req(name: &str, why: &str, param: Option<&str>) -> CapabilityRequest {
    CapabilityRequest { name: name.into(), why: why.into(), param: param.map(str::to_string), extra: Default::default() }
}

#[test]
fn the_catalogue_has_the_design_names_and_hard_asks() {
    let names: Vec<&str> = CATALOGUE.iter().map(|d| d.name).collect();
    for n in ["camera.view", "camera.record", "microphone.listen", "speaker.play", "bluetooth.use", "usb.use", "files.read", "files.write", "net.connect", "net.listen", "screen.capture", "input.inject", "notify", "notify.receive", "schedule.use", "web.request.headers", "web.content.script", "web.user.agent", "clipboard.client", "files.client", "client.camera.offer", "client.microphone.offer"] {
        assert!(names.contains(&n), "{n}");
    }
    assert_eq!(names.len(), 66);
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 66, "names are unique");
    let hard: Vec<&str> = CATALOGUE.iter().filter(|d| d.hard_ask).map(|d| d.name).collect();
    assert_eq!(hard, ["camera.record", "microphone.listen", "net.listen", "screen.capture", "input.inject", "clipboard.client", "files.client", "client.camera.offer", "client.microphone.offer", "system.memory.set", "net.scan", "system.disk.install", "system.hostname.set", "router.use", "router.strategy.set", "k8s.portforward", "fleet.join", "fleet.manage", "fleet.assign", "fleet.receive", "tools.request"]);
    assert!(is_hard_ask("not.a.capability"), "unknown fails closed");
    assert!(!is_hard_ask("notify.receive") && !is_hard_ask("schedule.use"));
    assert_eq!(CATALOGUE_VERSION, 0);
}

#[test]
fn a_manifest_list_parses_from_json_and_unknown_fields_are_kept_and_reported() {
    let ok: Vec<CapabilityRequest> = serde_json::from_str(r#"[{"name":"camera.view","why":"show the live view"},{"name":"net.connect","why":"fetch weather","param":"api.example.com:443"}]"#).unwrap();
    assert_eq!(ok.len(), 2);
    assert!(validate_requests(&ok).is_ok());
    // finding F-1: a field of a newer manifest is kept and reported; an unknown NAME is still refused
    let newer: Vec<CapabilityRequest> = serde_json::from_str(r#"[{"name":"notify","why":"x","grant":true}]"#).unwrap();
    assert!(validate_requests(&newer).is_ok());
    assert_eq!(unknown_request_fields(&newer), vec!["notify: grant".to_string()]);
    assert_eq!(serde_json::to_value(&newer).unwrap()[0]["grant"], true);
    assert!(validate_requests(&serde_json::from_str::<Vec<CapabilityRequest>>(r#"[{"name":"nope","why":"x","grant":true}]"#).unwrap()).is_err());
}

#[test]
fn an_unknown_capability_is_refused_and_the_message_lists_the_names() {
    let e = validate_requests(&[req("camera.peek", "x", None)]).unwrap_err();
    assert_eq!(e, CapError::Unknown("camera.peek".into()));
    assert!(e.to_string().contains("camera.view"));
}

#[test]
fn a_reason_is_required_parameters_must_fit_and_duplicates_are_refused() {
    assert!(matches!(validate_requests(&[req("notify", "  ", None)]), Err(CapError::NoReason(_))));
    assert!(matches!(validate_requests(&[req("notify", &"x".repeat(201), None)]), Err(CapError::NoReason(_))));
    assert!(matches!(validate_requests(&[req("speaker.play", "x", Some("left"))]), Err(CapError::BadParam { .. })));
    assert!(matches!(validate_requests(&[req("usb.use", "x", Some("12345:1"))]), Err(CapError::BadParam { .. })));
    assert!(validate_requests(&[req("usb.use", "x", Some("05c8:0445"))]).is_ok());
    assert!(matches!(validate_requests(&[req("files.read", "x", Some("../etc"))]), Err(CapError::BadParam { .. })));
    assert!(matches!(validate_requests(&[req("files.read", "x", Some("relative"))]), Err(CapError::BadParam { .. })));
    assert!(validate_requests(&[req("files.read", "x", Some("/home/a/docs"))]).is_ok());
    assert!(matches!(validate_requests(&[req("net.connect", "x", Some("*.example.com"))]), Err(CapError::BadParam { .. })));
    assert!(matches!(validate_requests(&[req("net.listen", "x", Some("0"))]), Err(CapError::BadParam { .. })));
    assert!(validate_requests(&[req("net.listen", "x", Some("8080"))]).is_ok());
    assert!(matches!(validate_requests(&[req("notify", "x", None), req("notify", "y", None)]), Err(CapError::Duplicate(_))));
    assert!(validate_requests(&[req("camera.view", "a", Some("front")), req("camera.view", "b", Some("back"))]).is_ok());
    let many: Vec<_> = (0..25).map(|i| req("camera.view", "x", Some(&format!("c{i}")))).collect();
    assert_eq!(validate_requests(&many), Err(CapError::TooMany));
}

#[test]
fn notify_receive_and_schedule_use_take_their_own_parameters() {
    assert!(validate_requests(&[req("notify.receive", "show new mail", Some("source=mail min=high")), req("schedule.use", "daily summary", Some("every 1d"))]).is_ok());
    assert!(!lookup("notify.receive").unwrap().hard_ask);
}

#[test]
fn a_first_install_asks_for_all_of_it_and_nothing_asks_nothing() {
    let r = [req("camera.view", "live view", None), req("camera.record", "save clips", None)];
    assert_eq!(plan_ask(None, &[]), AskPlan::Nothing);
    assert_eq!(plan_ask(None, &r), AskPlan::All(r.to_vec()));
}

#[test]
fn an_update_asks_only_for_more_and_not_for_the_same_or_less() {
    let v1 = [req("camera.view", "live view", None), req("notify", "tell me", None)];
    let a = record_approval("alice", "watcher", "1.0.0", "aa", 100, &v1);
    assert_eq!(plan_ask(Some(&a), &v1), AskPlan::Nothing);
    assert_eq!(plan_ask(Some(&a), &v1[..1]), AskPlan::Nothing, "less");
    let v2 = [req("camera.view", "live view", None), req("microphone.listen", "sound", None)];
    assert_eq!(plan_ask(Some(&a), &v2), AskPlan::More(vec![v2[1].clone()]));
    // a held "any camera" covers a named one; a named one does not cover "any"
    assert_eq!(plan_ask(Some(&a), &[req("camera.view", "x", Some("front"))]), AskPlan::Nothing);
    let named = record_approval("alice", "watcher", "1.0.0", "aa", 100, &[req("camera.view", "x", Some("front"))]);
    assert!(matches!(plan_ask(Some(&named), &[req("camera.view", "x", None)]), AskPlan::More(_)));
    assert!(matches!(plan_ask(Some(&named), &[req("camera.view", "x", Some("back"))]), AskPlan::More(_)));
}

#[test]
fn the_card_marks_new_lines_and_hard_asks() {
    let held = [Grant { name: "camera.view".into(), param: None }];
    let l = card_lines(&[req("camera.view", "live view", Some("front")), req("camera.record", "clips", None)], &held);
    assert!(!l[0].is_new && l[0].words.contains("(front)") && l[0].words.contains("live view"));
    assert!(l[1].is_new && l[1].hard_ask);
}

#[test]
fn the_record_says_who_what_when_and_for_which_version_and_revoking_works() {
    let mut book = MemoryBook::default();
    let a = record_approval("alice", "watcher", "1.2.0", "abc123", 1_700_000_000, &[req("camera.view", "x", Some("front")), req("notify", "y", None)]);
    book.put("tile/user/alice/watcher", a);
    let mut a = book.get("tile/user/alice/watcher").unwrap();
    assert_eq!((a.who.as_str(), a.app_version.as_str(), a.app_sha256.as_str(), a.at), ("alice", "1.2.0", "abc123", 1_700_000_000));
    assert!(a.holds("camera.view", Some("front")) && a.holds("notify", None));
    assert!(!a.holds("camera.view", Some("back")) && !a.holds("microphone.listen", None));
    let line = a.audit_detail();
    assert!(line.contains("alice") && line.contains("1.2.0") && line.contains("abc123") && line.contains("camera.view[front]"));
    a.revoked.push(Grant { name: "notify".into(), param: None });
    assert!(!a.holds("notify", None), "a revoke applies at the next call");
    assert!(matches!(plan_ask(Some(&a), &[req("notify", "y", None)]), AskPlan::More(_)), "asking for a revoked one asks again");
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(serde_json::from_str::<Approval>(&json).unwrap(), a);
    book.remove("tile/user/alice/watcher");
    assert!(book.get("tile/user/alice/watcher").is_none());
}

#[test]
fn built_in_apps_hold_the_defaults_by_a_static_table_and_others_never() {
    assert!(builtin_holds("anything", "notify") && builtin_holds("anything", "speaker.play") && builtin_holds("cameras", "camera.view"));
    assert!(builtin_holds("settings", "bluetooth.use"));
    assert!(!builtin_holds("browser", "bluetooth.use"), "settings app only");
    assert!(!builtin_holds("browser", "usb.use") && !builtin_holds("settings", "screen.capture") && !builtin_holds("settings", "input.inject"));
    assert!(!builtin_holds("settings", "net.connect"), "role policy decides");
    assert!(!builtin_holds("settings", "no.such"));
    for (app, caps) in BUILTIN_TABLE {
        for c in *caps {
            assert!(lookup(c).is_some(), "{app} lists unknown {c}");
        }
    }
}

#[test]
fn the_older_object_form_maps_device_flags_and_hosts_and_says_why_the_rest_has_no_name() {
    let m = map_legacy(&["storage", "notifications", "speech", "files", "clipboard", "agent", "llm", "mystery"], &["api.example.com".to_string()]);
    let names: Vec<(&str, Option<&str>)> = m.requests.iter().map(|r| (r.name.as_str(), r.param.as_deref())).collect();
    assert_eq!(names, [("notify", None), ("speaker.play", None), ("net.connect", Some("api.example.com"))]);
    assert!(validate_requests(&m.requests).is_ok(), "what it makes is a valid manifest list");
    let un: Vec<&str> = m.unmapped.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(un, ["storage", "files", "clipboard", "agent", "llm", "mystery"]);
    assert!(m.unmapped.iter().all(|(_, why)| !why.is_empty()));
    assert_eq!(map_legacy(&[], &[]), LegacyMap::default());
}

#[test]
fn a_grant_with_an_end_is_held_until_its_second_and_a_bad_end_is_ignored() {
    let mut extra = serde_json::Map::new();
    extra.insert("ends".into(), serde_json::json!({"camera.view": 100, "events.power": "soon"}));
    let a = Approval { who: "pat".into(), app: "n".into(), app_version: "1".into(), app_sha256: String::new(), at: 0, grants: vec![Grant { name: "camera.view".into(), param: None }, Grant { name: "events.power".into(), param: None }], revoked: vec![], extra };
    assert!(a.holds_at("camera.view", None, 99));
    assert!(!a.holds_at("camera.view", None, 100));
    assert!(a.holds_at("events.power", None, 10_000), "a value that is no number is no end");
    assert_eq!(a.ends().len(), 1);
}

#[test]
fn a_limit_only_shortens_a_held_grant_and_can_be_taken_off_again() {
    let mut a = Approval { who: "pat".into(), app: "n".into(), app_version: "1".into(), app_sha256: String::new(), at: 0, grants: vec![Grant { name: "camera.view".into(), param: None }], revoked: vec![], extra: Default::default() };
    assert!(!a.set_end("notify", Some(500), 10), "a capability that is not held gets no end");
    assert!(!a.set_end("camera.view", Some(10), 10), "an end that is not in the future is refused");
    assert!(a.set_end("camera.view", Some(500), 10));
    assert!(a.holds_at("camera.view", None, 499) && !a.holds_at("camera.view", None, 500));
    assert!(a.set_end("camera.view", None, 10));
    assert!(a.extra.is_empty() && a.holds_at("camera.view", None, 10_000), "no empty `ends` is left behind");
    a.revoke("camera.view");
    assert!(!a.set_end("camera.view", Some(500), 10), "a revoked grant cannot be limited");
}
