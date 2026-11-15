//! User theme files (F-SHELL-62): the format, the readability rules, the folders, the choice kept per person, and the built-in themes
//! staying as they were.

use super::*;
use crate::theme::{self, ThemeId};

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        theme::set_custom(None);
        theme::set_current(ThemeId::Dark);
    }
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-usertheme-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const MIDNIGHT: &str = "# a calm blue\nname = Midnight blue\nbase = dark\nbg_top = #0b1020\nbg_bottom = #05070f\ncard = #131a30\ntext = #f2f4ff\nring = 1.5\n";

#[test]
fn a_theme_file_is_read_with_its_base_for_what_it_does_not_say() {
    let u = parse(MIDNIGHT).unwrap();
    assert_eq!(u.name, "Midnight blue");
    assert_eq!(u.theme.bg_top, [0x0b, 0x10, 0x20, 255]);
    assert_eq!(u.theme.text, [0xf2, 0xf4, 0xff, 255]);
    assert_eq!(u.theme.ring, 1.5);
    assert_eq!(u.theme.accent, ThemeId::Dark.theme().accent, "not written: the base's colour");
    assert_eq!(u.theme.id, ThemeId::Dark);
    let light = parse("name = Paper\nbase = light\nbg_top = #fdfcf8\n").unwrap();
    assert_eq!((light.theme.id, light.theme.text), (ThemeId::Light, ThemeId::Light.theme().text));
    assert_eq!(key_of("Midnight blue!"), "midnight-blue");
}

#[test]
fn a_file_that_cannot_be_read_is_refused_with_its_line_and_the_reason() {
    for (text, want) in [
        ("bg_top = #000000\n", "needs a `name"),
        ("name = x\nbg_top #000000\n", "line 2"),
        ("name = x\nbg_top = red\n", "#rrggbb"),
        ("name = x\nbg_top = #12345\n", "#rrggbb"),
        ("name = x\nsparkle = #000000\n", "not a colour"),
        ("name = x\nbase = neon\n", "base is dark"),
        ("name = x\nring = 9\n", "ring is a number"),
        ("name = bad/name\n", "the name is"),
        ("name = x\ntext = #101010\n", "`text` on"),
        ("name = x\nborder = #0b0b14\ncard = #0b0b14\n", "border"),
    ] {
        let e = parse(text).unwrap_err();
        assert!(e.contains(want), "{text:?}: {e}");
    }
    // the built-in themes pass their own rules
    for id in theme::ALL {
        assert!(check(id.theme()).is_ok() || id == ThemeId::Dark || id == ThemeId::Light, "{}", id.name());
    }
}

#[test]
fn themes_come_from_the_persons_folder_then_the_shared_one_and_a_bad_file_is_listed_not_hidden() {
    let d = dir("folders");
    std::fs::create_dir_all(d.join("themes/ann")).unwrap();
    std::fs::write(d.join("themes/ann/midnight-blue.theme"), MIDNIGHT).unwrap();
    std::fs::write(d.join("themes/midnight-blue.theme"), MIDNIGHT.replace("Midnight blue", "Shared midnight")).unwrap();
    std::fs::write(d.join("themes/paper.theme"), "name = Paper\nbase = light\n").unwrap();
    std::fs::write(d.join("themes/broken.theme"), "name = Broken\ntext = #111111\n").unwrap();
    std::fs::write(d.join("themes/big.theme"), "#".repeat(5000)).unwrap();
    std::fs::write(d.join("themes/notes.txt"), "not a theme").unwrap();
    let a = available(&d, "ann");
    let keys: Vec<&str> = a.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["midnight-blue", "big", "broken", "paper"], "own first, shadowing the shared one of the same key: {keys:?}");
    assert_eq!(a[0].1.as_ref().unwrap().name, "Midnight blue", "ann's own, not the shared one");
    assert!(a[1].1.as_ref().unwrap_err().contains("bigger than") && a[2].1.as_ref().unwrap_err().contains("`text` on"));
    // another person sees the shared ones only
    let b = available(&d, "bob");
    assert_eq!(b.iter().find(|(k, _)| k == "midnight-blue").unwrap().1.as_ref().unwrap().name, "Shared midnight");
    // a person name that tries to leave the folder looks only in the shared one
    assert_eq!(folders(&d, "../x").len(), 1, "a name that starts with a dot gets no folder of its own");
    assert!(folders(&d, "..").iter().all(|p| !p.to_string_lossy().contains("..")));
}

#[test]
fn choosing_a_user_theme_draws_with_it_keeps_the_choice_and_a_bad_one_changes_nothing() {
    let _r = Reset;
    let d = dir("choose");
    std::fs::create_dir_all(d.join("themes")).unwrap();
    std::fs::write(d.join("themes/midnight-blue.theme"), MIDNIGHT).unwrap();
    std::fs::write(d.join("themes/broken.theme"), "name = Broken\ntext = #111111\n").unwrap();
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), ThemeId::Dark.theme().bg_top, "Dark is untouched to begin with");
    assert_eq!(choose(&d, "ann", "midnight-blue").unwrap(), "Midnight blue");
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), [0x0b, 0x10, 0x20, 255], "the person's colour is on screen");
    assert_eq!(theme::themed(ThemeId::Dark.theme().accent), ThemeId::Dark.theme().accent, "what the file does not say is the base's");
    assert_eq!(theme::ring(), 1.5);
    // refused: the active theme stays
    assert!(choose(&d, "ann", "broken").unwrap_err().contains("`text` on"));
    assert!(choose(&d, "ann", "nope").unwrap_err().contains("no theme file"));
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), [0x0b, 0x10, 0x20, 255]);
    // the choice is kept per person and applied at the next sign-in; another person is not affected
    assert_eq!(theme::load_choice(&d, "ann").as_deref(), Some("user:midnight-blue"));
    theme::apply(None);
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), ThemeId::Dark.theme().bg_top, "the sign-in screen is always Dark");
    theme::apply_for(&d, "ann");
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), [0x0b, 0x10, 0x20, 255]);
    theme::apply_for(&d, "bob");
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), ThemeId::Dark.theme().bg_top);
    // choosing a built-in theme takes the custom one away
    theme::save(&d, "ann", ThemeId::Light);
    theme::apply_for(&d, "ann");
    assert_eq!(theme::current_id(), ThemeId::Light);
    assert_ne!(theme::themed(ThemeId::Dark.theme().bg_top), [0x0b, 0x10, 0x20, 255]);
    // a theme file that was removed or broke since: back to the saved built-in (Dark when none), never a half theme
    theme::save_choice(&d, "ann", "user:gone");
    theme::apply_for(&d, "ann");
    assert_eq!(theme::current_id(), ThemeId::Dark);
    assert_eq!(theme::themed(ThemeId::Dark.theme().bg_top), ThemeId::Dark.theme().bg_top);
}

#[test]
fn the_same_theme_is_kept_once_and_the_cap_is_a_plain_refusal() {
    let a = leak(&parse(MIDNIGHT).unwrap()).unwrap();
    let b = leak(&parse(MIDNIGHT).unwrap()).unwrap();
    assert!(std::ptr::eq(a, b));
}
