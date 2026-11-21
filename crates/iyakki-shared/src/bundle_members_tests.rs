use super::*;
use serde_json::json;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bundle-members-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_theme_member_becomes_theme_file_text_and_nothing_else_is_carried_over() {
    let t = theme_text(&json!({"name": "Dusk", "base": "dark", "ring": 1.5})).unwrap();
    assert!(t.contains("name = Dusk\n") && t.contains("base = dark\n") && t.contains("ring = 1.5\n"));
    for bad in [json!({"base": "dark"}), json!({"name": "x", "bg_top": ["#000000"]}), json!({"name": "a\nb"}), json!({"name": "x", "Bad-Key": "1"}), json!([1])] {
        assert!(theme_text(&bad).is_err(), "{bad}");
    }
}

#[test]
fn the_file_is_put_for_everybody_or_one_person_kept_when_another_bundle_has_the_name_and_dropped_only_when_unchanged() {
    let d = dir("put");
    let m = json!({"name": "Dusk", "base": "dark"});
    put_theme(&d, None, "dusk", &m).unwrap();
    put_theme(&d, None, "dusk", &m).unwrap();
    assert!(d.join("bundle-themes/bundle-dusk.theme").is_file());
    put_theme(&d, Some("ana"), "dusk", &m).unwrap();
    assert!(d.join("bundle-themes/ana/bundle-dusk.theme").is_file());
    assert!(put_theme(&d, None, "dusk", &json!({"name": "Other"})).unwrap_err().contains("another bundle"));
    drop_theme(&d, None, "dusk", &json!({"name": "Other"})).unwrap();
    assert!(d.join("bundle-themes/bundle-dusk.theme").is_file(), "not what the bundle wrote: left alone");
    drop_theme(&d, None, "dusk", &m).unwrap();
    drop_theme(&d, Some("ana"), "dusk", &m).unwrap();
    assert!(!d.join("bundle-themes/bundle-dusk.theme").exists() && !d.join("bundle-themes/ana/bundle-dusk.theme").exists());
    assert_eq!(theme_dir(&d, Some("../x")), d.join("bundle-themes"), "a name that starts with a dot gets no folder of its own");
    assert_eq!(theme_dir(&d, Some("..")), d.join("bundle-themes"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_template_member_follows_the_rules_of_a_template_a_person_keeps() {
    let t = view_template(&json!({"name": "Study room", "tiles": ["learn", "notes"]})).unwrap();
    assert_eq!((t.id.as_str(), t.name.as_str(), t.tiles.len()), ("bundle-study-room", "Study room", 2));
    for bad in [json!({"tiles": ["a"]}), json!({"name": "x"}), json!({"name": "x", "tiles": []}), json!({"name": "x", "tiles": [1]}), json!({"name": "x", "tiles": ["a b"]}), json!({"name": "a-name-far-too-long-for-a-view", "tiles": ["a"]}), json!({"name": "!!", "tiles": ["a"]}), json!([1])] {
        assert!(view_template(&bad).is_err(), "{bad}");
    }
}

#[test]
fn an_agent_member_is_text_with_a_name_and_a_prompt_and_nothing_that_acts() {
    let p = agent_preset(&json!({"name": "Reviewer", "prompt": "Read the diff.\nBe brief.", "summary": "Reviews", "command": "rm -rf /"})).unwrap();
    assert_eq!((p.name.as_str(), p.prompt.as_str(), p.summary.as_str()), ("Reviewer", "Read the diff.\nBe brief.", "Reviews"));
    assert_eq!(agent_preset(&json!({"name": "a", "prompt": "b"})).unwrap().summary, "");
    let long = "x".repeat(MAX_PRESET_PROMPT + 1);
    for bad in [json!({"name": "a"}), json!({"prompt": "b"}), json!({"name": "a\nb", "prompt": "b"}), json!({"name": "a", "prompt": long}), json!({"name": "a", "prompt": "b", "summary": 4}), json!("x")] {
        assert!(agent_preset(&bad).is_err(), "{bad}");
    }
}

#[test]
fn an_extension_member_is_a_line_for_the_list_with_an_optional_app_target() {
    let e = extension_line(&json!({"name": "Mail helper", "summary": "Adds a button", "target": "chat"})).unwrap();
    assert_eq!((e.name.as_str(), e.target.as_deref()), ("Mail helper", Some("chat")));
    assert_eq!(extension_line(&json!({"name": "x"})).unwrap().target, None);
    for bad in [json!({"summary": "s"}), json!({"name": "x", "target": "a b"}), json!({"name": "x", "target": 3}), json!(null)] {
        assert!(extension_line(&bad).is_err(), "{bad}");
    }
}

#[test]
fn kept_template_and_agent_copies_are_listed_for_everybody_and_the_person_and_dropped_only_when_unchanged() {
    let d = dir("kept-data");
    let t = json!({"name": "Study", "tiles": ["learn"]});
    put_data_member(&d, "template", None, "study", &t).unwrap();
    put_data_member(&d, "template", Some("ana"), "mine", &json!({"name": "Mine", "tiles": ["edit"]})).unwrap();
    put_data_member(&d, "template", None, "study", &t).unwrap();
    assert!(put_data_member(&d, "template", None, "study", &json!({"name": "Study", "tiles": ["edit"]})).unwrap_err().contains("another bundle"));
    assert!(put_data_member(&d, "template", None, "bad", &json!({"name": "x", "tiles": []})).is_err() && put_data_member(&d, "theme", None, "x", &t).is_err());
    assert_eq!(bundled_view_templates(&d, "ana").iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["bundle-study", "bundle-mine"]);
    assert_eq!(bundled_view_templates(&d, "bob").len(), 1);
    assert_eq!(bundled_view_templates(&d, "../x").len(), 1, "a name that starts with a dot has no folder of its own");
    put_data_member(&d, "agent", None, "rev", &json!({"name": "Reviewer", "prompt": "p"})).unwrap();
    assert_eq!(bundled_agent_presets(&d, "bob")[0].name, "Reviewer");
    drop_data_member(&d, "template", None, "study", &json!({"name": "Study", "tiles": ["edit"]})).unwrap();
    assert_eq!(bundled_view_templates(&d, "bob").len(), 1, "not what the bundle wrote: left alone");
    drop_data_member(&d, "template", None, "study", &t).unwrap();
    assert!(bundled_view_templates(&d, "bob").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_person_chooses_an_installed_preset_and_the_choice_ends_with_the_bundle_and_is_theirs_alone() {
    let d = dir("chosen");
    assert!(chosen_agent_preset(&d, "ana").is_none() && preset_note(&d, "ana").is_empty(), "nothing chosen by default");
    put_data_member(&d, "agent", None, "rev", &json!({"name": "Reviewer", "prompt": "Review code carefully.", "summary": "s"})).unwrap();
    put_data_member(&d, "agent", Some("bob"), "mine", &json!({"name": "Bobs", "prompt": "b"})).unwrap();
    assert!(choose_agent_preset(&d, "ana", Some("Nope")).unwrap_err().contains("no assistant preset"));
    assert!(choose_agent_preset(&d, "ana", Some("Bobs")).is_err(), "another person's preset is not on offer");
    choose_agent_preset(&d, "ana", Some("Reviewer")).unwrap();
    assert_eq!(chosen_agent_preset(&d, "ana").unwrap().name, "Reviewer");
    assert!(chosen_agent_preset(&d, "bob").is_none(), "the choice is the person's own");
    let note = preset_note(&d, "ana");
    assert!(note.contains("Reviewer") && note.contains("Review code carefully.") && note.contains("cannot give you a tool"));
    assert!(choose_agent_preset(&d, "../x", Some("Reviewer")).is_err());
    // removing the bundle's copy ends the choice; choosing none clears it
    drop_data_member(&d, "agent", None, "rev", &json!({"name": "Reviewer", "prompt": "Review code carefully.", "summary": "s"})).unwrap();
    assert!(chosen_agent_preset(&d, "ana").is_none() && preset_note(&d, "ana").is_empty());
    put_data_member(&d, "agent", None, "rev", &json!({"name": "Reviewer", "prompt": "p"})).unwrap();
    assert!(chosen_agent_preset(&d, "ana").is_some(), "the name matches again");
    choose_agent_preset(&d, "ana", None).unwrap();
    assert!(chosen_agent_preset(&d, "ana").is_none());
    let _ = std::fs::remove_dir_all(&d);
}
