//! The home screen's pages (LAUNCHER-PLAN slice 1): six pages, the keys, the dots and arrows, the motion and its off switch, the machine
//! page, and the colours in every theme.

use super::tests::*;
use super::*;
use crate::appicon;

fn home() -> (Shell, Gfx) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (mut sh, _) = shell_with_users(&format!("home{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    (sh, g)
}

fn click_at(sh: &mut Shell, r: R) {
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
}

#[test]
fn the_home_screen_has_the_machine_page_then_one_page_per_group() {
    let (sh, _) = home();
    assert_eq!(sh.home_pages(), 1 + sh.groups().len());
    assert_eq!(sh.home_pages(), 6, "the machine and the five built-in groups");
    assert_eq!(sh.home_page(), 1, "a signed-in person starts on their remembered group (Ask & Create by default), not on the machine page");
}

#[test]
fn the_keys_one_to_six_turn_to_the_pages_and_left_right_carry_on_past_the_ends() {
    let (mut sh, mut g) = home();
    for (d, want) in [('1', 0usize), ('2', 1), ('3', 2), ('4', 3), ('5', 4), ('6', 5)] {
        key(&mut sh, Key::Char(d));
        sh.render(&mut g);
        assert_eq!(sh.home_page(), want, "key {d}");
        assert_eq!(sh.machine, want == 0);
    }
    key(&mut sh, Key::Char('7'));
    assert_eq!(sh.home_page(), 5, "no seventh page: the key does nothing");
    // on the machine page Right goes to the first group; Left on it does nothing
    key(&mut sh, Key::Char('1'));
    key(&mut sh, Key::Left);
    assert_eq!(sh.home_page(), 0);
    key(&mut sh, Key::Right);
    assert_eq!(sh.home_page(), 1);
    // Left on the first icon of a group goes back a page, Right on the last goes on
    key(&mut sh, Key::Left);
    assert_eq!(sh.home_page(), 0);
    key(&mut sh, Key::Char('3'));
    let n = sh.group_items().len();
    for _ in 0..n - 1 {
        key(&mut sh, Key::Right);
    }
    assert_eq!((sh.home_page(), sh.sel), (2, n - 1), "Right walks the icons first");
    key(&mut sh, Key::Right);
    assert_eq!(sh.home_page(), 3);
    // the last page has no page after it
    key(&mut sh, Key::Char('6'));
    let n = sh.group_items().len();
    sh.sel = n - 1;
    key(&mut sh, Key::Right);
    assert_eq!((sh.home_page(), sh.sel), (5, n - 1));
}

#[test]
fn the_machine_page_has_no_icons_and_nothing_on_it_opens_by_enter_or_crashes() {
    let (mut sh, mut g) = home();
    key(&mut sh, Key::Char('1'));
    for k in [Key::Enter, Key::Up, Key::Down, Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Left] {
        sh.handle(&Event::Key(k));
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "launcher");
        assert_eq!(sh.home_page(), 0);
    }
    sh.handle(&Event::Scroll(3));
    sh.handle(&Event::MouseMove(500, 400));
    sh.handle(&Event::MouseDown(500, 400));
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "launcher");
    assert!(g.text_log.iter().any(|(_, _, t)| t == "The machine"), "the page says what it is");
    assert!(g.text_log.iter().any(|(_, _, t)| t == "NEEDS A PERSON") && g.text_log.iter().any(|(_, _, t)| t == "APPS"));
    // the group summary lists every group with its count
    assert!(g.text_log.iter().any(|(_, _, t)| t == "Build"), "{:?}", g.text_log.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>());
}

#[test]
fn the_machine_page_names_what_needs_a_person_from_what_the_apps_say_through_the_core() {
    use iyakki_core_api::{AppStatus, StatusKind};
    let (mut sh, mut g) = home();
    key(&mut sh, Key::Char('1'));
    // the core's word, scripted: approvals waiting, updates to install, a station that plays (which needs nobody)
    let fake = Arc::new(iyakki_core_fake::FakeCore::new());
    fake.statuses_fake().set(vec![
        ("chat".into(), AppStatus { kind: StatusKind::Attention, line: "1 waiting for you".into(), count: 1, needs: Some(("Approvals waiting".into(), "1 waiting for you".into())) }),
        ("store".into(), AppStatus { kind: StatusKind::Setup, line: "3 updates".into(), count: 3, needs: Some(("Updates".into(), "3 waiting in STORE".into())) }),
        ("radio".into(), AppStatus { kind: StatusKind::Running, line: "Playing Test FM".into(), count: 0, needs: None }),
    ]);
    sh.core = fake.clone();
    sh.refresh_badge();
    sh.badge = Badge { unread: 2, approvals: 1, overdue: 0, other: 0 };
    let rows = sh.machine_attention();
    let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, ["Approvals waiting", "Inbox", "Updates"], "the first the core named, the inbox, then the rest; the playing radio needs nobody");
    assert!(rows[2].1.contains("3 waiting in STORE"));
    sh.render(&mut g);
    assert!(g.text_log.iter().any(|(_, _, t)| t == "Approvals waiting"));
    // nothing said, no unread: an honest empty state
    fake.statuses_fake().set(vec![]);
    sh.refresh_badge();
    sh.badge = Badge::default();
    g.text_log.clear();
    sh.render(&mut g);
    assert!(g.text_log.iter().any(|(_, _, t)| t.starts_with("Nothing needs you")), "an honest empty state");
}

#[test]
fn an_icon_says_what_its_app_says_in_its_line_chip_and_badge_and_an_app_that_says_nothing_keeps_its_static_line() {
    use iyakki_core_api::{AppStatus, StatusKind};
    let (mut sh, mut g) = home();
    let fake = Arc::new(iyakki_core_fake::FakeCore::new());
    fake.statuses_fake().set(vec![
        ("chat".into(), AppStatus { kind: StatusKind::Attention, line: "2 waiting for you".into(), count: 2, needs: Some(("Approvals waiting".into(), "2 waiting for you".into())) }),
        ("radio".into(), AppStatus { kind: StatusKind::Running, line: "Playing Test FM".into(), count: 0, needs: None }),
    ]);
    sh.core = fake;
    sh.refresh_badge();
    assert_eq!(sh.app_state(PApp::Chat, true, false), (appicon::AppState::Attention, 2));
    assert_eq!(sh.app_state(PApp::Radio, true, false), (appicon::AppState::Running, 0));
    assert_eq!(sh.app_state(PApp::Rag, true, false), (appicon::AppState::Ready, 0));
    assert_eq!(sh.tile_sub(PApp::Chat, "Talk to the AI"), "2 waiting for you");
    assert_eq!(sh.tile_sub(PApp::Rag, "Ask your files"), "Ask your files", "nothing said: the static line, no made-up figure");
    // the line and the chip reach the screen (this person's home group holds CHAT)
    key(&mut sh, Key::Char('2'));
    g.text_log.clear();
    sh.render(&mut g);
    let said: Vec<&String> = g.text_log.iter().map(|x| &x.2).collect();
    assert!(said.iter().any(|t| t.as_str() == "2 waiting for you"), "the line: {said:?}");
    assert!(said.iter().any(|t| t.as_str() == "ATTENTION"), "the chip: {said:?}");
    assert!(!said.iter().any(|t| t.as_str() == "Talk to the AI"), "the static line gave way");
}

#[test]
fn the_page_dots_and_the_arrows_turn_the_page_by_mouse_and_touch() {
    let (mut sh, mut g) = home();
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let dots = launcher::page_dots(w, h, s, sh.home_pages());
    click_at(&mut sh, dots[0]);
    assert_eq!(sh.home_page(), 0);
    click_at(&mut sh, dots[4]);
    assert_eq!(sh.home_page(), 4);
    sh.render(&mut g);
    let (prev, next) = launcher::edge_chevrons(w, h, s);
    click_at(&mut sh, prev);
    assert_eq!(sh.home_page(), 3);
    click_at(&mut sh, next);
    click_at(&mut sh, next);
    assert_eq!(sh.home_page(), 5);
    click_at(&mut sh, next);
    assert_eq!(sh.home_page(), 5, "no arrow past the last page");
    // the first page has no previous arrow
    click_at(&mut sh, dots[0]);
    click_at(&mut sh, prev);
    assert_eq!(sh.home_page(), 0);
    // both are registered as regions with words for the screen reader and the keyboard walk
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|x| x.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "go to The machine") && labels.iter().any(|l| l == "go to Build"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "show next group") && !labels.iter().any(|l| l == "show previous group"), "{labels:?}");
}

#[test]
fn every_app_in_every_group_is_reachable_by_key_and_the_chips_still_switch_pages() {
    let (mut sh, mut g) = home();
    let mut seen = 0;
    for p in 1..sh.home_pages() {
        key(&mut sh, Key::Char(char::from_digit(p as u32 + 1, 10).unwrap()));
        sh.render(&mut g);
        let n = sh.group_items().len();
        let mut at = vec![sh.sel];
        for _ in 1..n {
            key(&mut sh, Key::Right);
            if sh.home_page() != p {
                break;
            }
            at.push(sh.sel);
        }
        assert_eq!(at.len(), n, "page {p}: Right visits every icon");
        seen += n;
    }
    assert!(seen >= 30, "{seen} icons over the groups");
    // a chip click is a page change too
    key(&mut sh, Key::Char('1'));
    sh.render(&mut g);
    let row = sh.chip_row();
    click_at(&mut sh, row.chips[2].1);
    assert_eq!(sh.home_page(), 3);
    assert!(!sh.machine);
}

#[test]
fn the_pop_runs_about_a_second_after_a_page_change_and_reduced_motion_removes_it() {
    let (mut sh, mut g) = home();
    let was = crate::fx::reduced_motion();
    crate::fx::set_reduced_motion(false);
    sh.render(&mut g);
    sh.take_stale();
    assert!(!sh.home_animating(), "nothing moves before a page change");
    key(&mut sh, Key::Char('3'));
    assert!(sh.home_animating() && !sh.home_is_calm());
    assert_eq!(sh.redraw_interval_ms(), 100);
    // it asks for a frame per heartbeat while it runs, then settles
    let mut frames = 0;
    for _ in 0..80 {
        sh.handle(&Event::Tick);
        if sh.take_stale() {
            frames += 1;
            sh.render(&mut g);
        }
    }
    assert!(frames >= 15 && frames < 60, "{frames} frames while it ran");
    assert!(!sh.home_animating(), "done after a couple of seconds");
    assert!(sh.home_is_calm() && sh.redraw_interval_ms() == 500, "an idle Home is back to the slow safety net");
    // reduced motion: no frames asked for, nothing animating, the icons at their final size
    crate::fx::set_reduced_motion(true);
    key(&mut sh, Key::Char('4'));
    assert!(!sh.home_animating());
    sh.take_stale();
    sh.handle(&Event::Tick);
    assert!(!sh.take_stale());
    assert!(sh.home_is_calm());
    crate::fx::set_reduced_motion(was);
}

#[test]
fn an_idle_home_draws_no_faster_than_every_500_ms_after_the_new_pages_settle() {
    let (mut sh, mut g) = home();
    crate::fx::set_reduced_motion(false);
    key(&mut sh, Key::Char('2'));
    for _ in 0..90 {
        sh.handle(&Event::Tick);
        sh.take_stale();
    }
    sh.render(&mut g);
    sh.take_stale();
    let (mut frames, mut since) = (0u32, 0u64);
    for _ in 0..300 {
        sh.handle(&Event::Tick);
        since += 33;
        let dirty = sh.take_stale();
        if dirty || since >= sh.redraw_interval_ms() {
            frames += 1;
            since = 0;
            sh.render(&mut g);
        }
    }
    assert!(frames <= 25, "{frames} frames in 10 s of an idle Home");
}

#[test]
fn the_page_is_drawn_in_every_theme_with_readable_text_on_the_island() {
    use crate::theme::ThemeId;
    for id in crate::theme::ALL {
        crate::theme::set_current(id);
        let (mut sh, mut g) = home();
        crate::fx::set_reduced_motion(true);
        for page in ['1', '2', '3', '5'] {
            key(&mut sh, Key::Char(page));
            sh.render(&mut g);
            assert!(g.text_log.iter().filter(|(_, _, t)| !t.is_empty()).count() > 6, "{id:?} page {page} draws text");
        }
        // the ink the page uses over the island's own fill (the island is the theme's card colour, see-through, over the background)
        let t = crate::theme::current();
        let island = crate::fx::over(t.bg_top, crate::fx::with_alpha(t.card, appicon::island_alpha()));
        for (name, ink) in [("TEXT", t.text), ("MUTED", t.muted), ("DIM", t.dim)] {
            let c = crate::fx::contrast(ink, island);
            assert!(c >= 4.5, "{id:?}: {name} on the island is {c:.2}:1");
        }
        crate::theme::set_current(ThemeId::Dark);
    }
    crate::fx::set_reduced_motion(false);
}

#[test]
fn a_locked_app_shows_the_lock_and_the_chip_and_keeps_its_icon_colour() {
    let (sh, _) = home();
    // a person's role that does not allow an app: Locked, with the chip text; state never reaches the body's colour (appicon test)
    let (st, n) = sh.app_state(PApp::Chat, false, false);
    assert_eq!((st, n), (appicon::AppState::Locked, 0));
    assert_eq!(st.chip_text(), Some("LOCKED"));
    assert_eq!(sh.app_state(PApp::Radio, true, false).0, appicon::AppState::Ready, "an app that says nothing has no chip");
    assert_eq!(sh.app_state(PApp::Camera, true, true).0, appicon::AppState::Setup);
}

#[test]
fn the_island_is_drawn_over_a_wallpaper_blurred_once_and_high_contrast_keeps_it_plain() {
    use iyakki_uikit::blur;
    let (mut sh, mut g) = home();
    iyakki_uikit::wallpaper::set_current(iyakki_uikit::wallpaper::Id::Dusk);
    sh.render(&mut g);
    let b0 = blur::builds();
    for _ in 0..4 {
        sh.render(&mut g);
    }
    assert!(blur::builds() <= b0 + 1, "frames after the first reuse the picture");
    let after_first = blur::builds();
    for _ in 0..4 {
        sh.render(&mut g);
    }
    assert_eq!(blur::builds(), after_first, "four more frames built nothing");
    // High contrast draws a solid island: it never asks for the frost
    crate::theme::set_current(crate::theme::ThemeId::HighContrast);
    let hc = blur::builds();
    sh.render(&mut g);
    sh.render(&mut g);
    let _ = hc; // the background itself is rebuilt for the new theme; the island must not have drawn the picture
    let (w, h) = (g.width() as usize, g.height() as usize);
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2]]
    };
    let isl = launcher::island_rect(w as u32, h as u32, scale_for(w as u32, h as u32), sh.group_items().len());
    let (a, b) = (px((isl.x + 6.0) as usize, (isl.y + isl.h / 2.0) as usize), px((isl.x + 14.0) as usize, (isl.y + isl.h / 2.0) as usize));
    assert_eq!(a, b, "a solid island has no wallpaper showing through");
    crate::theme::set_current(crate::theme::ThemeId::Dark);
    iyakki_uikit::wallpaper::set_current(iyakki_uikit::wallpaper::Id::Theme);
}

#[test]
fn the_fonts_a_person_chose_are_applied_at_their_sign_in_and_the_default_again_at_the_sign_in_screen() {
    use iyakki_uikit::fonts::{self, CodeFont, UiFont};
    let (mut sh, _) = shell_with_users("fonts");
    let dir = sh.services_for_demo().dir().to_path_buf();
    fonts::save_choice(&dir, "alice", UiFont::Liberation, CodeFont::Liberation);
    login(&mut sh, "root", "rootpass12");
    assert_eq!((fonts::current_ui(), fonts::current_code()), (UiFont::DejaVu, CodeFont::DejaVu), "root chose nothing");
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    assert_eq!((fonts::current_ui(), fonts::current_code()), (UiFont::Liberation, CodeFont::Liberation));
    // the home screen draws with it (the same frame in the other family differs) and nothing breaks
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let with = g.rgba().to_vec();
    fonts::apply_default();
    sh.render(&mut g);
    assert_ne!(with, g.rgba().to_vec(), "the shapes changed");
    fonts::set_ui(UiFont::Liberation);
    key(&mut sh, Key::Char('o'));
    assert_eq!((fonts::current_ui(), fonts::current_code()), (UiFont::DejaVu, CodeFont::DejaVu), "the sign-in screen is always the default");
}
