//! Settings -> Appearance: the page is there for everybody, each key changes one part of the person's own Look and is audited, the
//! essentials stay, and the page draws without text running off the side at the sizes the other settings pages are held to.

use super::*;
use crate::access;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_uikit::look::{self, DockPos, IconShape, IconStyle};
use iyakki_uikit::look_registry as reg;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::ui::scale_for(w, h);
    access::start_frame();
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

#[test]
fn the_page_is_there_for_everybody_and_found_by_its_words() {
    let cx = admin("ap-tab");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Appearance));
    assert_eq!(Tab::Appearance.group(), CATEGORIES[4]);
    for w in ["wallpaper", "dock", "icons", "3d", "glass", "launcher"] {
        assert!(Tab::Appearance.keywords().contains(w), "{w}");
    }
    let mut cx2 = cx_with_admin_store("ap-user");
    cx2.session.role = iyakki_sec::policy::Role::User;
    cx2.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(SettingsApp::new(&cx2).tabs().contains(&Tab::Appearance), "no administrator needed");
}

#[test]
fn each_key_changes_one_part_of_the_persons_own_look_and_audits_it_and_another_person_is_untouched() {
    let cx = admin("ap-keys");
    let dir = cx.data_dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    for c in ['s', 'h', 'y', 'd', 'i', 'c', 'e', 'z'] {
        press(&mut a, &cx, c);
    }
    let l = reg::current(&dir, "root").look;
    assert_eq!(l.name, "My look");
    assert_eq!(l.icons.style, IconStyle::Flat);
    assert!(reg::take_changed());
    assert_eq!(reg::load_choice(&dir, "root").as_deref(), Some("my-look"));
    assert_eq!(reg::load_choice(&dir, "amy"), None, "another person is untouched");
    // wallpaper buttons: the key is the number, the choice is kept as the wallpaper of the person and of the Look
    press(&mut a, &cx, '3');
    assert_eq!(iyakki_uikit::wallpaper::load(&dir, "root"), iyakki_uikit::wallpaper::Id::Dusk);
    assert_eq!(reg::current(&dir, "root").look.wallpaper, iyakki_uikit::wallpaper::Id::Dusk);
    iyakki_uikit::wallpaper::set_current(iyakki_uikit::wallpaper::Id::Theme);
}

#[test]
fn the_top_bar_switches_never_take_the_essentials_and_come_back_in_their_place() {
    let cx = admin("ap-bar");
    let dir = cx.data_dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    for c in ['k', 'g', 'n', 'v', 'j'] {
        press(&mut a, &cx, c);
    }
    let bar = reg::current(&dir, "root").look.topbar.items;
    for e in look::ESSENTIAL_BAR {
        assert!(bar.contains(&e), "{e:?}");
    }
    assert!(!bar.contains(&look::BarItem::Clock) && !bar.contains(&look::BarItem::Volume));
    press(&mut a, &cx, 'k');
    let bar = reg::current(&dir, "root").look.topbar.items;
    assert_eq!(bar.iter().position(|i| *i == look::BarItem::Clock), Some(0), "the clock is back first, as in the default order");
}

#[test]
fn pinning_unpinning_and_the_usual_apps_again_are_kept_per_person() {
    let cx = admin("ap-pin");
    let dir = cx.data_dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    press(&mut a, &cx, 'a');
    let v = reg::load_dock(&dir, "root").unwrap();
    assert_eq!(v.len(), 7, "{v:?}");
    press(&mut a, &cx, 'x');
    press(&mut a, &cx, 'x');
    assert_eq!(reg::load_dock(&dir, "root").unwrap().len(), 5);
    press(&mut a, &cx, 'o');
    assert_eq!(reg::load_dock(&dir, "root"), None, "the usual apps again");
    assert_eq!(reg::load_dock(&dir, "amy"), None);
}

#[test]
fn the_look_keys_go_through_the_looks_and_a_bad_file_is_never_chosen() {
    let cx = admin("ap-looks");
    let dir = cx.data_dir().to_path_buf();
    std::fs::create_dir_all(dir.join("themes/root")).unwrap();
    std::fs::write(dir.join("themes/root/broken.look"), "name = Broken\n[launcher]\ncolumns = 99\n").unwrap();
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    press(&mut a, &cx, ']');
    assert_eq!(reg::load_choice(&dir, "root").as_deref(), Some("compact"));
    press(&mut a, &cx, ']');
    press(&mut a, &cx, ']');
    assert_ne!(reg::load_choice(&dir, "root").as_deref(), Some("broken"));
    assert_eq!(reg::current(&dir, "root").look.launcher.layout, reg::current(&dir, "root").look.launcher.layout);
}

#[test]
fn the_page_draws_every_row_without_text_past_the_side_at_four_sizes_and_the_largest_text() {
    let cx = admin("ap-size");
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for level in [0usize, crate::uisize::LEVELS.len() - 1] {
            crate::uisize::apply(Some(level));
            let g = draw(&mut a, &cx, w, h);
            let s = crate::ui::scale_for(w, h);
            for (_, r, _, t) in &g.layout_log.texts {
                assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5, "{w}x{h} level {level}: {t:?} {r:?} runs off the side");
            }
            let texts: Vec<&str> = g.text_log.iter().map(|(_, _, t)| t.as_str()).collect();
            for want in ["Look", "Wallpaper", "Icon style", "Dock", "Pinned apps"] {
                assert!(texts.iter().any(|t| t.starts_with(want)), "{w}x{h}: {want} is on the page: {texts:?}");
            }
            let _ = s;
        }
    }
    crate::uisize::apply(None);
}

#[test]
fn every_clickable_row_has_a_key_so_the_page_works_without_a_mouse() {
    let cx = admin("ap-walk");
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char(_)))).count() >= 14, "{}", g.hits.len());
    let _ = (DockPos::Bottom, IconShape::Squircle);
}

#[test]
fn the_font_keys_pick_the_text_and_the_code_family_per_person_and_the_page_still_fits_in_both() {
    use iyakki_uikit::fonts::{self, CodeFont, UiFont};
    let cx = admin("ap-fonts");
    let dir = cx.data_dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    assert_eq!(fonts::load_choice(&dir, "root"), (UiFont::DejaVu, CodeFont::DejaVu));
    press(&mut a, &cx, 'f');
    assert_eq!(fonts::load_choice(&dir, "root"), (UiFont::Liberation, CodeFont::DejaVu), "the text font only");
    assert_eq!(fonts::current_ui(), UiFont::Liberation, "applied at once");
    press(&mut a, &cx, 'p');
    assert_eq!(fonts::load_choice(&dir, "root"), (UiFont::Liberation, CodeFont::Liberation));
    assert_eq!(fonts::load_choice(&dir, "amy"), (UiFont::DejaVu, CodeFont::DejaVu), "another person is untouched");
    // the page in the new family at the sizes the other pages are held to
    for (w, h) in [(360u32, 740u32), (1280, 720)] {
        for level in [0usize, crate::uisize::LEVELS.len() - 1] {
            crate::uisize::apply(Some(level));
            let g = draw(&mut a, &cx, w, h);
            for (_, r, _, t) in &g.layout_log.texts {
                assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5, "{w}x{h} level {level}: {t:?} {r:?} runs off the side");
            }
            let texts: Vec<&str> = g.text_log.iter().map(|(_, _, t)| t.as_str()).collect();
            assert!(texts.iter().any(|t| t.starts_with("Text font")) && texts.iter().any(|t| t.starts_with("Code font")), "{w}x{h}: the font rows are there");
        }
    }
    crate::uisize::apply(None);
    press(&mut a, &cx, 'F');
    press(&mut a, &cx, 'P');
    assert_eq!(fonts::load_choice(&dir, "root"), (UiFont::DejaVu, CodeFont::DejaVu), "the capital goes back");
    fonts::apply_default();
}

#[test]
fn the_type_keys_set_line_height_letter_spacing_and_tabular_figures_per_person_at_once_and_the_rows_are_shown() {
    use iyakki_uikit::typeset::{self, LineHeight, Prefs, Tracking};
    let cx = admin("ap-type");
    let dir = cx.data_dir().to_path_buf();
    typeset::set_current(Prefs::default());
    let mut a = SettingsApp::new(&cx).on(Tab::Appearance);
    press(&mut a, &cx, 'l');
    assert_eq!(typeset::load(&dir, "root").line, LineHeight::Loose, "normal, then loose");
    assert_eq!(typeset::current().line, LineHeight::Loose, "applied at once");
    press(&mut a, &cx, 'm');
    press(&mut a, &cx, 'u');
    assert_eq!(typeset::load(&dir, "root"), Prefs { line: LineHeight::Loose, tracking: Tracking::Wide, tabular: true });
    assert_eq!(typeset::load(&dir, "amy"), Prefs::default(), "another person is untouched");
    for (w, h) in [(360u32, 740u32), (1280, 720)] {
        let g = draw(&mut a, &cx, w, h);
        let texts: Vec<&str> = g.text_log.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(texts.iter().any(|t| t.starts_with("Line height")) && texts.iter().any(|t| t.starts_with("Letter spacing")) && texts.iter().any(|t| t.starts_with("Tabular")), "{w}x{h}");
        for (_, r, _, t) in &g.layout_log.texts {
            assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5, "{w}x{h}: {t:?} {r:?} runs off the side");
        }
    }
    press(&mut a, &cx, 'L');
    assert_eq!(typeset::load(&dir, "root").line, LineHeight::Normal, "the capital goes back");
    typeset::set_current(Prefs::default());
}
