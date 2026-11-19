//! Skins (F-SDK-122): the shipped skins pass the validator, the registry lists and chooses them, and an app's accent is corrected
//! when its contrast fails.

use super::*;
use crate::look_registry;
use crate::theme::{ThemeId, ALL};

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-skins-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("themes")).unwrap();
    d
}

#[test]
fn every_shipped_skin_passes_the_validator_and_differs_from_the_plain_skin() {
    let all = shipped();
    assert_eq!(all.len(), SHIPPED.len());
    let mut keys: Vec<&str> = all.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), SHIPPED.len(), "skin keys are unique");
    let plain = Look::default();
    for (k, r) in &all {
        let a = r.as_ref().unwrap_or_else(|p| panic!("{k}: {}", look::problems_text(p)));
        assert!(a.fixes.is_empty(), "{k} needed fixes: {:?}", a.fixes);
        assert_eq!(a.look.key(), *k, "the Look name makes the file's key");
        assert_ne!(a.look.surface, plain.surface, "{k} restyles more than colour");
        assert!(!Look::builtin().iter().any(|b| b.key() == *k), "{k} is not a built-in key");
        // the text it writes back reads as the same skin
        assert_eq!(Look::from_text(&a.look.to_text()).unwrap(), a.look);
    }
    // the four skins cover the words: a pale one, a glass one, sharp corners, reduced motion
    let looks: Vec<Look> = all.into_iter().map(|(_, r)| r.unwrap().look).collect();
    assert!(looks.iter().any(|l| l.surface.glass) && looks.iter().any(|l| l.motion == look::Motion::Reduced));
    assert!(looks.iter().any(|l| l.surface.shape == look::Shape::Sharp) && looks.iter().any(|l| l.theme.id == ThemeId::Light));
}

#[test]
fn the_validator_refuses_what_the_look_rules_refuse_and_what_the_widgets_cannot_read() {
    assert!(validate("name = X\n[effects]\nblur = 99\n").is_err());
    // an accent the same as the card cannot be seen, and a label on a pale accent cannot be read
    let p = validate("[look]\nname = Faint\n[colours]\nbase = dark\naccent = #303644\n").unwrap_err();
    assert!(p.iter().any(|x| x.message.contains("accent")), "{p:?}");
    let p = validate("[look]\nname = Pale accent\n[colours]\nbase = light\naccent = #f0f0a0\n").unwrap_err();
    assert!(p.iter().any(|x| x.message.contains("accent")), "{p:?}");
    assert!(validate("[look]\nname = Fine\n[colours]\nbase = light\n[shape]\nshape = sharp\n").is_ok());
}

#[test]
fn the_registry_lists_the_shipped_skins_after_the_built_in_looks_and_a_file_cannot_take_their_name() {
    let d = dir("registry");
    std::fs::write(d.join("themes/calm.look"), "[look]\nname = Calm\n").unwrap();
    let keys: Vec<(String, bool)> = look_registry::available(&d, "ann").into_iter().map(|e| (e.key, e.builtin)).collect();
    let pos = |k: &str| keys.iter().position(|(x, _)| x == k).unwrap_or_else(|| panic!("{k} missing: {keys:?}"));
    assert_eq!((pos("default"), pos("compact"), pos("classic")), (0, 1, 2));
    for (k, _) in SHIPPED {
        assert!(pos(k) > 2 && keys[pos(k)].1, "{k} is listed as shipped");
    }
    assert!(keys.iter().any(|(k, b)| k == "calm (file)" && !b), "a file named like a skin is listed with its reason");
    assert!(look_registry::save_custom(&d, "ann", &Look { name: "Paper".into(), ..Look::default() }).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn choosing_a_skin_is_kept_for_that_person_and_read_back_while_another_keeps_theirs() {
    let d = dir("choose");
    let a = look_registry::choose(&d, "ann", "glass-night").unwrap();
    assert!(a.look.surface.glass);
    assert_eq!(look_registry::current(&d, "ann").look.name, "Glass night");
    assert_eq!(look_registry::current(&d, "bob").look.name, "Default", "bob never chose");
    look_registry::choose(&d, "bob", "paper").unwrap();
    assert_eq!(look_registry::current(&d, "ann").look.name, "Glass night");
    assert_eq!(look_registry::current(&d, "bob").look.name, "Paper");
    // the shell hears about the change and reads it again, no restart
    look_registry::notify_changed();
    assert!(look_registry::take_changed() && !look_registry::take_changed());
    assert!(look_registry::choose(&d, "ann", "nope").is_err());
    assert_eq!(look_registry::current(&d, "ann").look.name, "Glass night", "a refused choice keeps the one before");
    let _ = std::fs::remove_dir_all(&d);
}

const WISHES: [[u8; 4]; 8] = [
    [255, 255, 0, 255],   // pure yellow
    [255, 0, 0, 255],     // pure red
    [255, 255, 255, 255], // white
    [0, 0, 0, 255],       // black
    [20, 20, 30, 255],    // near the dark ground
    [240, 240, 250, 255], // near the light ground
    [0, 160, 90, 255],    // a green
    [120, 60, 200, 128],  // a purple with alpha
];

#[test]
fn an_accent_that_fails_the_contrast_is_corrected_and_one_that_passes_is_kept() {
    let mut kept = 0;
    let mut corrected = 0;
    for id in ALL {
        let t = id.theme();
        for w in WISHES {
            let a = fit_accent(t, w);
            assert!(accent_ok(t, a.colour), "{:?} on {}: {:?}", w, id.key(), a);
            assert_eq!(a.colour[3], 255);
            if accent_ok(t, [w[0], w[1], w[2], 255]) {
                assert!(!a.adjusted && a.colour[..3] == w[..3], "a passing accent is kept: {w:?} on {}", id.key());
                kept += 1;
            } else {
                assert!(a.adjusted, "a failing accent is reported as adjusted");
                // WCAG AA numbers, spelled out: the label 4.5:1, the control against its grounds 3:1
                assert!(contrast(label_on(t, a.colour), a.colour) >= 4.5 && contrast(a.colour, t.card) >= 3.0 && contrast(a.colour, t.bg_top) >= 3.0);
                corrected += 1;
            }
        }
    }
    assert!(kept > 0 && corrected > 0, "the wishes cover both outcomes (kept {kept}, corrected {corrected})");
}

#[test]
fn the_fallback_is_the_themes_own_accent_when_nothing_on_the_way_passes() {
    // a theme where no mix passes: the card and the page are both the middle grey, so no accent is 3:1 against them but the theme's own
    let mut t = *ThemeId::Dark.theme();
    t.card = [128, 128, 128, 255];
    t.bg_top = [128, 128, 128, 255];
    t.accent = [128, 128, 128, 255];
    let a = fit_accent(&t, [200, 30, 30, 255]);
    assert_eq!(a.colour, t.accent);
    assert!(a.adjusted);
}

#[test]
fn what_an_app_may_ask_about_the_skin_in_use() {
    let paper = shipped().into_iter().find(|(k, _)| k == "paper").unwrap().1.unwrap().look;
    let s = SkinInfo::of(&paper);
    assert_eq!((s.key.as_str(), s.name.as_str(), s.base, s.dark, s.glass), ("paper", "Paper", "light", false, false));
    assert_eq!((s.shape, s.edge, s.spacing, s.reduced_motion), ("round", "thin", "airy", false));
    let d = SkinInfo::of(&Look::default());
    assert!(d.dark && d.key == "default" && d.base == "dark");
}

#[test]
fn a_label_on_the_accent_always_reads_even_on_the_light_theme_whose_text_is_dark_on_a_deep_blue() {
    for id in ALL {
        let t = id.theme();
        for fill in [t.accent, [250, 250, 150, 255], [20, 20, 30, 255], [120, 120, 120, 255], [255, 0, 0, 255]] {
            let l = label_on(t, fill);
            assert!(contrast(l, fill) >= 4.5, "{} on {fill:?}: {:.2}", id.key(), contrast(l, fill));
        }
    }
    // the theme's own text is kept when it reads (on a fill as dark as the card), and the audit finding: on the plain Dark accent it does not
    let d = ThemeId::Dark.theme();
    assert_eq!(label_on(d, d.card), d.text);
    assert!(contrast(d.text, d.accent) < 4.5, "Dark text on the Dark accent was below AA before label_on: {:.2}", contrast(d.text, d.accent));
}
