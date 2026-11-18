use super::*;
use crate::gfx::Gfx;

/// The choice is thread-local and the tests share threads: put it back whatever happens.
struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        apply_default();
    }
}

fn drawn(text: &str, face: Face) -> (Vec<u8>, f32) {
    let mut g = Gfx::new(400, 40);
    g.clear([0, 0, 0, 255]);
    let w = g.text(4.0, 4.0, 20.0, [255, 255, 255, 255], text, face);
    (g.rgba().to_vec(), w)
}

#[test]
fn dejavu_is_the_default_and_a_choice_changes_the_shapes_and_back_again_gives_the_same_pixels() {
    let _r = Reset;
    assert_eq!((current_ui(), current_code()), (UiFont::DejaVu, CodeFont::DejaVu));
    let (a, wa) = drawn("Settings and files", Face::Sans);
    set_ui(UiFont::Liberation);
    let (b, wb) = drawn("Settings and files", Face::Sans);
    assert_ne!(a, b, "another family draws other shapes");
    assert!(wb < wa, "Liberation Sans is narrower than DejaVu Sans: {wb} {wa}");
    // the code font is its own choice: the sans text did not move when only the code font changed
    set_ui(UiFont::DejaVu);
    set_code(CodeFont::Liberation);
    assert_eq!(drawn("Settings and files", Face::Sans).0, a, "a code font choice leaves the interface text alone");
    assert_ne!(drawn("let x = 1;", Face::Mono).0, {
        set_code(CodeFont::DejaVu);
        drawn("let x = 1;", Face::Mono).0
    });
    assert_eq!(drawn("Settings and files", Face::Sans).0, a, "back to the default gives the first picture exactly");
}

#[test]
fn a_frame_buffer_that_drew_in_one_face_draws_in_the_new_one_after_a_choice() {
    let _r = Reset;
    let mut g = Gfx::new(400, 40);
    g.text(4.0, 4.0, 20.0, [255, 255, 255, 255], "Hello", Face::Bold);
    let first = g.rgba().to_vec();
    set_ui(UiFont::Liberation);
    g.clear([0, 0, 0, 255]);
    let mut fresh = Gfx::new(400, 40);
    fresh.clear([0, 0, 0, 255]);
    g.text(4.0, 4.0, 20.0, [255, 255, 255, 255], "Hello", Face::Bold);
    fresh.text(4.0, 4.0, 20.0, [255, 255, 255, 255], "Hello", Face::Bold);
    assert_eq!(g.rgba(), fresh.rgba(), "the old buffer's letter cache was dropped");
    assert_ne!(g.rgba(), &first[..]);
}

#[test]
fn a_letter_the_chosen_face_lacks_is_drawn_from_dejavu_and_never_as_an_empty_box() {
    let _r = Reset;
    let lack = ['✓', '→', '⚠', '●'];
    set_ui(UiFont::Liberation);
    set_code(CodeFont::Liberation);
    for face in [Face::Sans, Face::Bold, Face::Mono] {
        for c in lack {
            let f = font_for(face, c);
            assert_ne!(f.lookup_glyph_index(c), 0, "{c} in {face:?} has a glyph");
        }
    }
    // at least one of them really is missing from Liberation (the test would prove nothing otherwise)
    assert!(lack.iter().any(|c| font(Face::Sans).lookup_glyph_index(*c) == 0));
    let (px, w) = drawn("✓ done", Face::Sans);
    assert!(w > 20.0 && px.iter().step_by(4).any(|v| *v > 0));
}

#[test]
fn every_letter_of_english_text_has_a_glyph_in_every_family() {
    let text = "The quick brown fox jumps over the lazy dog 0123456789 .,:;!?()[]{}<>/\\|-_=+*&^%$#@~`'\" … · — ← → ✓";
    for face in [Face::Sans, Face::Bold, Face::Mono] {
        for c in text.chars() {
            assert_ne!(font_for(face, c).lookup_glyph_index(c), 0, "{c:?} in {face:?}");
        }
    }
}

#[test]
fn the_choice_is_kept_per_person_beside_the_other_settings_and_unknown_values_mean_the_default() {
    let dir = std::env::temp_dir().join(format!("iyakki-fonts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(load_choice(&dir, "alice"), (UiFont::DejaVu, CodeFont::DejaVu));
    std::fs::write(dir.join("ui-alice.json"), r#"{"wallpaper":"dusk","theme":"light"}"#).unwrap();
    save_choice(&dir, "alice", UiFont::Liberation, CodeFont::Liberation);
    assert_eq!(load_choice(&dir, "alice"), (UiFont::Liberation, CodeFont::Liberation));
    assert_eq!(load_choice(&dir, "bob"), (UiFont::DejaVu, CodeFont::DejaVu), "another person is unaffected");
    let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ui-alice.json")).unwrap()).unwrap();
    assert_eq!((kept["wallpaper"].as_str(), kept["theme"].as_str()), (Some("dusk"), Some("light")), "the other settings stayed");
    std::fs::write(dir.join("ui-carol.json"), r#"{"font_ui":"comic","font_code":7}"#).unwrap();
    assert_eq!(load_choice(&dir, "carol"), (UiFont::DejaVu, CodeFont::DejaVu));
    std::fs::write(dir.join("ui-dave.json"), "not json").unwrap();
    assert_eq!(load_choice(&dir, "dave"), (UiFont::DejaVu, CodeFont::DejaVu));
    assert_eq!(UiFont::from_key(" Liberation "), Some(UiFont::Liberation));
    assert_eq!(CodeFont::from_key("nope"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_family_is_permissively_licensed_and_listed_in_the_legal_files() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = std::fs::read_to_string(root.join("legal/assets.txt")).unwrap();
    let notices = std::fs::read_to_string(root.join("legal/THIRD-PARTY-NOTICES.md")).unwrap();
    for lic in UI_ALL.iter().map(|f| f.licence()).chain(CODE_ALL.iter().map(|f| f.licence())) {
        assert!(lic.contains("Open Font License") || lic.contains("DejaVu"), "{lic}");
    }
    for f in ["LiberationSans-Regular.ttf", "LiberationSans-Bold.ttf", "LiberationMono-Regular.ttf"] {
        assert!(root.join("crates/iyakki-shell/assets").join(f).is_file(), "{f} is bundled");
    }
    assert!(assets.contains("assets/Liberation*.ttf | OFL-1.1"));
    assert!(notices.contains("Liberation") && root.join("crates/iyakki-shell/assets/LIBERATION-LICENSE.txt").is_file());
}
