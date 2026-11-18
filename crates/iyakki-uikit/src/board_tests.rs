use super::*;

fn keys(b: &Board) -> Vec<&'static str> {
    b.slots.iter().map(|s| s.kind.key()).collect()
}

#[test]
fn the_default_board_is_what_needs_a_person_then_the_apps_the_machine_the_runs_and_the_network() {
    let b = Board::default();
    assert_eq!(keys(&b), ["attention", "apps", "vitals", "runs", "network"]);
    assert_eq!(b.slots[0].size, Size::Full);
    assert_eq!(b.missing().iter().map(|k| k.key()).collect::<Vec<_>>(), ["approvals", "tokens"]);
}

#[test]
fn removing_moving_resizing_and_adding_keep_every_kind_once() {
    let mut b = Board::default();
    assert_eq!(b.remove(1).map(|s| s.kind), Some(Kind::Apps));
    assert_eq!(keys(&b), ["attention", "vitals", "runs", "network"]);
    assert_eq!(b.remove(9), None);
    assert_eq!(b.move_to(3, 0), Some(0));
    assert_eq!(keys(&b), ["network", "attention", "vitals", "runs"]);
    assert_eq!(b.move_to(0, 99), Some(3), "past the end is the end");
    assert_eq!(keys(&b), ["attention", "vitals", "runs", "network"]);
    assert_eq!(b.move_to(7, 0), None);
    assert!(b.set_size(1, Size::Full) && b.slots[1].size == Size::Full && !b.set_size(9, Size::Full));
    assert!(b.add(Kind::Tokens) && !b.add(Kind::Tokens), "a kind is never there twice");
    assert!(b.add(Kind::Approvals) && b.add(Kind::Apps));
    assert_eq!(b.slots.len(), MAX);
    assert!(b.missing().is_empty());
    b.remove(0);
    assert!(b.add(Kind::Attention), "a place is free again");
    assert_eq!(b.slots.last().map(|s| (s.kind, s.size)), Some((Kind::Attention, Size::Full)));
}

#[test]
fn the_text_round_trips_and_unknown_words_and_repeats_are_dropped_not_trusted() {
    let b = Board::default();
    assert_eq!(Board::from_text(&b.to_text()), Some(b));
    let odd = Board::from_text("apps:full, bogus:half, apps:half, vitals:wide, runs").unwrap();
    assert_eq!(odd.slots.iter().map(|s| (s.kind.key(), s.size.key())).collect::<Vec<_>>(), [("apps", "full"), ("runs", "half")]);
    assert_eq!(Board::from_text("nonsense"), None);
    assert_eq!(Board::from_text(""), None);
}

#[test]
fn the_board_is_kept_per_person_beside_the_other_settings_and_a_bad_file_gives_the_default() {
    let dir = std::env::temp_dir().join(format!("iyakki-board-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(load(&dir, "alice"), Board::default());
    std::fs::write(dir.join("ui-alice.json"), r#"{"wallpaper":"dusk"}"#).unwrap();
    let mut b = Board::default();
    b.remove(0);
    b.add(Kind::Tokens);
    save(&dir, "alice", &b);
    assert_eq!(load(&dir, "alice"), b);
    assert_eq!(load(&dir, "bob"), Board::default(), "another person has their own");
    let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ui-alice.json")).unwrap()).unwrap();
    assert_eq!(kept["wallpaper"].as_str(), Some("dusk"), "the other settings stayed");
    // an empty board is a choice too
    save(&dir, "alice", &Board { slots: vec![] });
    assert!(load(&dir, "alice").slots.is_empty());
    std::fs::write(dir.join("ui-carol.json"), r#"{"board":"zzz"}"#).unwrap();
    assert_eq!(load(&dir, "carol"), Board::default());
    std::fs::write(dir.join("ui-dave.json"), "not json").unwrap();
    assert_eq!(load(&dir, "dave"), Board::default());
    let _ = std::fs::remove_dir_all(&dir);
}
