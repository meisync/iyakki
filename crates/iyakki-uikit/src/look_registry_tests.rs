//! The Look list and the per-account choice (F-LOOK-05).

use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-look-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("themes").join("ann")).unwrap();
    d
}

const MIDNIGHT: &str = include_str!("../../../docs/examples/looks/midnight-glass.look");
const PAPER: &str = include_str!("../../../docs/examples/looks/paper-list.look");

#[test]
fn the_list_is_the_built_ins_then_the_persons_files_then_the_shared_ones() {
    let d = dir("list");
    std::fs::write(d.join("themes/ann/midnight-glass.look"), MIDNIGHT).unwrap();
    std::fs::write(d.join("themes/paper-list.look"), PAPER).unwrap();
    std::fs::write(d.join("themes/old.theme"), "name = Old\nbase = light\n").unwrap();
    std::fs::write(d.join("themes/broken.look"), "name = Broken\n[launcher]\ncolumns = 99\n").unwrap();
    std::fs::write(d.join("themes/default.look"), PAPER).unwrap();
    std::fs::write(d.join("themes/ann/paper-list.look"), "name = Ann paper\n").unwrap();
    let keys: Vec<(String, bool)> = available(&d, "ann").into_iter().map(|e| (e.key, e.result.is_ok())).collect();
    let k = |s: &str| keys.iter().position(|(x, _)| x == s).unwrap_or_else(|| panic!("{s} missing in {keys:?}"));
    assert_eq!((k("default"), k("compact"), k("classic")), (0, 1, 2));
    assert!(k("midnight-glass") > 2 && k("old") > 2);
    assert!(keys[k("old")].1, "a v1 theme file is a Look");
    assert!(!keys[k("broken")].1, "a bad file is listed, with its reasons");
    assert!(!keys[k("default (file)")].1, "a file cannot take a built-in name");
    // the person's own file shadows the shared one with the same key
    assert_eq!(load(&d, "ann", "paper-list").unwrap().look.name, "Ann paper");
    assert_eq!(load(&d, "bob", "paper-list").unwrap().look.name, "Paper list");
    let e = load(&d, "ann", "broken").unwrap_err();
    assert_eq!(e[0].line, 3);
    assert!(load(&d, "ann", "nope").unwrap_err()[0].message.contains("no Look"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn big_files_and_links_are_refused_like_theme_files() {
    let d = dir("caps");
    std::fs::write(d.join("themes/huge.look"), format!("name = Huge\n# {}\n", "x".repeat(9000))).unwrap();
    assert!(load(&d, "ann", "huge").unwrap_err()[0].message.contains("bigger"));
    #[cfg(unix)]
    {
        std::fs::write(d.join("real.txt"), PAPER).unwrap();
        std::os::unix::fs::symlink(d.join("real.txt"), d.join("themes/linked.look")).unwrap();
        assert!(load(&d, "ann", "linked").unwrap_err()[0].message.contains("plain file"));
    }
    for i in 0..40 {
        std::fs::write(d.join(format!("themes/f{i:02}.look")), PAPER).unwrap();
    }
    assert!(available(&d, "ann").len() <= 3 + crate::skins::SHIPPED.len() + usertheme::MAX_FILES * 2 * 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_choice_is_kept_beside_the_theme_and_an_old_file_keeps_working() {
    let d = dir("choice");
    // an old file without the field: no choice, the default Look, and nothing was written by looking
    let old = r#"{"theme":"light","wallpaper":"dusk","size":2}"#;
    std::fs::write(d.join("ui-ann.json"), old).unwrap();
    assert_eq!(load_choice(&d, "ann"), None);
    assert_eq!(current(&d, "ann").look, Look::default());
    assert_eq!(std::fs::read_to_string(d.join("ui-ann.json")).unwrap(), old);
    // the person chooses: the field is added and the rest stays
    std::fs::write(d.join("themes/ann/midnight-glass.look"), MIDNIGHT).unwrap();
    let a = choose(&d, "ann", "midnight-glass").unwrap();
    assert_eq!(a.look.name, "Midnight glass");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("ui-ann.json")).unwrap()).unwrap();
    assert_eq!((v["look"].as_str(), v["theme"].as_str(), v["wallpaper"].as_str(), v["size"].as_i64()), (Some("midnight-glass"), Some("light"), Some("dusk"), Some(2)));
    assert_eq!(crate::theme::load(&d, "ann"), crate::theme::ThemeId::Light, "the theme reader is not disturbed");
    assert_eq!(current(&d, "ann").look.name, "Midnight glass");
    // a built-in can be chosen too; a bad or missing one changes nothing
    assert_eq!(choose(&d, "ann", "compact").unwrap().look.name, "Compact");
    std::fs::write(d.join("themes/ann/bad.look"), "name = Bad\n[dock]\ndock_size = 1\n").unwrap();
    assert!(choose(&d, "ann", "bad").is_err());
    assert!(choose(&d, "ann", "missing").is_err());
    assert_eq!(load_choice(&d, "ann").as_deref(), Some("compact"));
    // when the chosen file is later damaged or removed the person is not left without a Look
    std::fs::write(d.join("themes/ann/midnight-glass.look"), "garbage").unwrap();
    save_choice(&d, "ann", "midnight-glass");
    assert_eq!(current(&d, "ann").look, Look::default());
    // a file that is not JSON is replaced by a fresh one that holds the choice
    std::fs::write(d.join("ui-bob.json"), "not json").unwrap();
    save_choice(&d, "bob", "classic");
    assert_eq!(load_choice(&d, "bob").as_deref(), Some("classic"));
    // names that try to leave the folder cannot
    save_choice(&d, "../evil", "classic");
    assert!(d.join("ui-..evil.json").exists() && !d.parent().unwrap().join("ui-evil.json").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pinned_dock_apps_are_kept_per_person_beside_the_choice_and_bad_names_are_dropped() {
    let d = dir("dock");
    assert_eq!(load_dock(&d, "ann"), None);
    save_choice(&d, "ann", "compact");
    save_dock(&d, "ann", &["chat".into(), "browser".into(), "../evil".into(), "".into(), "edit".into()]);
    assert_eq!(load_dock(&d, "ann"), Some(vec!["chat".to_string(), "browser".to_string(), "edit".to_string()]));
    assert_eq!(load_choice(&d, "ann").as_deref(), Some("compact"), "the Look choice is left alone");
    assert_eq!(load_dock(&d, "bob"), None, "per person");
}

#[test]
fn an_edited_look_is_saved_as_the_persons_file_and_chosen_and_a_built_in_name_is_refused() {
    let d = dir("custom");
    let mut l = Look::default();
    l.name = "My desk".into();
    l.icons.style = look::IconStyle::ThreeD;
    l.dock.items = vec!["chat".into(), "ide".into()];
    let a = save_custom(&d, "ann", &l).unwrap();
    assert_eq!(a.look.key(), "my-desk");
    assert!(d.join("themes/ann/my-desk.look").exists());
    assert_eq!(load_choice(&d, "ann").as_deref(), Some("my-desk"));
    let back = current(&d, "ann").look;
    assert_eq!((back.icons.style, back.dock.items.clone()), (look::IconStyle::ThreeD, vec!["chat".to_string(), "ide".to_string()]));
    let mut b = Look::default();
    b.name = "Compact".into();
    assert!(save_custom(&d, "ann", &b).is_err());
}
