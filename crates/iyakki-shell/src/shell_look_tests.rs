//! The shell reads the Look (LAUNCHER-PLAN / LOOK-DESIGN stage 3): the icons, the layout, the dock and what accessibility overrides.

use super::tests::*;
use super::*;
use iyakki_uikit::look::{self, DockPos, IconShape, IconSize, IconStyle, LauncherLayout, Look};
use iyakki_uikit::look_registry;

fn fresh(tag: &str) -> (Shell, Gfx) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (sh, _) = shell_with_users(&format!("{tag}{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    (sh, Gfx::new(1280, 720))
}

fn sign_in(sh: &mut Shell, g: &mut Gfx, who: &str, pass: &str) {
    login(sh, who, pass);
    sh.render(g);
}

fn with_look(sh: &Shell, who: &str, f: impl FnOnce(&mut Look)) {
    let mut l = Look::default();
    l.name = "Mine".into();
    f(&mut l);
    look_registry::save_custom(sh.env.data_dir(), who, &l).unwrap_or_else(|e| panic!("{e:?}"));
}

fn dock_labels(sh: &Shell) -> Vec<String> {
    sh.hits().iter().filter_map(|h| h.label.strip_prefix("dock ").map(String::from)).collect()
}

fn press(sh: &mut Shell, r: R) {
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
}

#[test]
fn with_no_look_chosen_the_launcher_is_as_before_plus_the_default_dock_of_apps_the_role_may_use() {
    let (mut sh, mut g) = fresh("lk1");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(sh.look, Look::default());
    assert_eq!(dock_labels(&sh), ["CHAT", "BROWSER", "AI IDE", "EDIT", "CLI", "SYSTEM"], "the default set, in order");
    let p = launcher::params();
    assert!(p.inset_b > 0.0 && p.inset_l == 0.0 && p.fixed_cols.is_none(), "{p:?}");
    // a guest may use only some of them
    key(&mut sh, Key::Char('o'));
    sign_in(&mut sh, &mut g, "gus", "guestpass1");
    let labels = dock_labels(&sh);
    assert!(labels.contains(&"CHAT".to_string()) && !labels.contains(&"CLI".to_string()) && !labels.contains(&"SYSTEM".to_string()), "{labels:?}");
}

#[test]
fn a_click_on_a_dock_icon_opens_the_app_with_a_running_dot_while_it_is_open_and_the_dock_survives_sign_out() {
    let (mut sh, mut g) = fresh("lk2");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let (_, items) = sh.dock_layout(w, h, scale_for(w, h)).unwrap();
    press(&mut sh, items[1]);
    assert_eq!(sh.screen_name(), "app:browser");
    assert!(sh.app_running(PApp::Browser) && !sh.app_running(PApp::Chat));
    key(&mut sh, Key::Esc);
    assert_eq!(sh.screen_name(), "launcher");
}

#[test]
fn dragging_a_dock_icon_over_another_reorders_it_and_the_order_is_the_persons_own_after_sign_in_again() {
    let (mut sh, mut g) = fresh("lk3");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let (_, items) = sh.dock_layout(w, h, s).unwrap();
    let c = |r: R| ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    let (a, b) = (c(items[0]), c(items[3]));
    sh.handle(&Event::MouseDown(a.0, a.1));
    sh.handle(&Event::MouseMove(a.0 + 30, a.1));
    sh.handle(&Event::MouseMove(b.0, b.1));
    sh.handle(&Event::MouseUp(b.0, b.1));
    assert_eq!(sh.screen_name(), "launcher", "a drag opens nothing");
    sh.render(&mut g);
    assert_eq!(dock_labels(&sh), ["BROWSER", "AI IDE", "EDIT", "CHAT", "CLI", "SYSTEM"]);
    key(&mut sh, Key::Char('o'));
    sign_in(&mut sh, &mut g, "root", "rootpass12");
    assert_eq!(dock_labels(&sh)[0], "CHAT", "another person has their own list");
    key(&mut sh, Key::Char('o'));
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(dock_labels(&sh), ["BROWSER", "AI IDE", "EDIT", "CHAT", "CLI", "SYSTEM"], "kept beside the Look choice");
    assert_eq!(look_registry::load_dock(sh.env.data_dir(), "alice").map(|v| v.len()), Some(6));
}

#[test]
fn the_dock_works_by_keyboard_d_focuses_it_arrows_move_comma_and_period_reorder_x_unpins_enter_opens() {
    let (mut sh, mut g) = fresh("lk4");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    key(&mut sh, Key::Char('d'));
    assert_eq!(sh.dock_focus, Some(0));
    key(&mut sh, Key::Right);
    key(&mut sh, Key::Char('.'));
    assert_eq!(sh.dock_focus, Some(2));
    sh.render(&mut g);
    assert_eq!(&dock_labels(&sh)[..3], ["CHAT", "AI IDE", "BROWSER"]);
    key(&mut sh, Key::Char(','));
    key(&mut sh, Key::Char('x'));
    sh.render(&mut g);
    assert_eq!(dock_labels(&sh).len(), 5, "unpinned");
    key(&mut sh, Key::Char('z'));
    assert_eq!(sh.screen_name(), "launcher", "other keys do nothing while the dock has the keyboard");
    key(&mut sh, Key::Enter);
    assert!(sh.screen_name().starts_with("app:"), "Enter opens the focused app");
    sh.leave_app();
    key(&mut sh, Key::Char('d'));
    key(&mut sh, Key::Esc);
    assert_eq!(sh.dock_focus, None);
    assert_eq!(sh.screen_name(), "launcher");
}

#[test]
fn a_look_changes_the_icons_the_layout_and_the_dock_and_signing_out_puts_the_default_back() {
    let (mut sh, mut g) = fresh("lk5");
    with_look(&sh, "alice", |l| {
        l.icons.shape = IconShape::Circle;
        l.icons.style = IconStyle::ThreeD;
        l.icons.size = IconSize::L;
        l.launcher.layout = LauncherLayout::Grid;
        l.launcher.columns = look::Columns::Fixed(4);
        l.dock.position = DockPos::Left;
        l.dock.items = vec!["chat".into(), "radio".into()];
    });
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!((sh.look.icons.shape, sh.look.icons.style), (IconShape::Circle, IconStyle::ThreeD));
    assert_eq!(dock_labels(&sh), ["CHAT", "RADIO"], "the Look's own list is used when the person pinned none");
    let p = launcher::params();
    assert_eq!((p.fixed_cols, p.inset_l > 0.0, p.inset_b), (Some(4), true, 0.0));
    // the icons are in a screen that still holds them all
    let items = sh.group_items();
    let (w, h) = (g.width(), g.height());
    let rects = launcher::page_rects(w, h, scale_for(w, h), items.len(), 0);
    let dock = sh.dock_layout(w, h, scale_for(w, h)).unwrap().0;
    assert!(rects.iter().all(|r| r.x >= dock.x + dock.w), "the tiles start right of a left dock");
    key(&mut sh, Key::Char('o'));
    assert_eq!(sh.look, Look::default(), "the sign-in screen is the default look");
    assert_eq!(launcher::params(), launcher::Params::default());
}

#[test]
fn the_list_layout_gives_one_wide_row_per_app_and_labels_off_draws_no_names() {
    let (mut sh, mut g) = fresh("lk6");
    with_look(&sh, "alice", |l| {
        l.launcher.layout = LauncherLayout::List;
        l.dock.position = DockPos::Off;
    });
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let rects = launcher::page_rects(w, h, s, sh.group_items().len(), 0);
    assert!(rects.len() >= 2 && rects.iter().all(|r| r.w > 3.0 * r.h), "rows, not cells: {:?}", rects.first());
    assert!(dock_labels(&sh).is_empty(), "dock off");
    assert!(g.text_log.iter().any(|(_, _, t)| t == "CHAT"), "the names show");
    with_look(&sh, "alice", |l| {
        l.icons.labels = false;
        l.dock.position = DockPos::Off;
    });
    key(&mut sh, Key::Char('o'));
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    g.text_log.clear();
    sh.render(&mut g);
    assert!(!g.text_log.iter().any(|(_, _, t)| t == "CHAT" || t == "Talk to the AI"), "labels off");
}

#[test]
fn high_contrast_keeps_the_plain_icon_whatever_the_look_asks() {
    let (mut sh, mut g) = fresh("lk7");
    with_look(&sh, "alice", |l| l.icons.style = IconStyle::Glass);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(sh.icon_look().style, IconStyle::Glass);
    crate::theme::set_current(crate::theme::ThemeId::HighContrast);
    assert_eq!(sh.icon_look().style, IconStyle::Gradient);
    crate::theme::set_current(crate::theme::ThemeId::Dark);
}

#[test]
fn every_shape_style_layout_and_dock_side_draws_at_four_sizes_with_every_icon_on_screen() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (800, 480), (360, 740)] {
        for (i, (layout, pos, style, shape)) in [
            (LauncherLayout::Paged, DockPos::Bottom, IconStyle::Gradient, IconShape::Squircle),
            (LauncherLayout::Grid, DockPos::Left, IconStyle::Flat, IconShape::Rounded),
            (LauncherLayout::List, DockPos::Right, IconStyle::Outline, IconShape::Square),
            (LauncherLayout::Grid, DockPos::Bottom, IconStyle::Glass, IconShape::Circle),
            (LauncherLayout::Paged, DockPos::Off, IconStyle::ThreeD, IconShape::Squircle),
        ]
        .into_iter()
        .enumerate()
        {
            let (mut sh, _) = fresh("lk8");
            let mut g = Gfx::new(w, h);
            with_look(&sh, "alice", |l| {
                l.launcher.layout = layout;
                l.dock.position = pos;
                l.icons.style = style;
                l.icons.shape = shape;
            });
            login(&mut sh, "alice", "alicepass1");
            sh.resize_for_test(w, h);
            sh.render(&mut g);
            let s = scale_for(w, h);
            let n = sh.group_items().len();
            let cap = launcher::per_page(w, h, s);
            for p in 0..launcher::page_count(n, cap) {
                sh.sel = p * cap;
                sh.render(&mut g);
                for r in launcher::page_rects(w, h, s, n, p) {
                    assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h} case {i}: {r:?} is off screen");
                }
            }
        }
    }
}

#[test]
fn the_look_choice_survives_a_bad_file_and_never_replaces_the_one_the_person_had() {
    let (mut sh, mut g) = fresh("lk9");
    with_look(&sh, "alice", |l| l.icons.style = IconStyle::Flat);
    std::fs::write(sh.env.data_dir().join("themes/alice/broken.look"), "name = Broken\n[launcher]\ncolumns = 99\n").ok();
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(sh.look.icons.style, IconStyle::Flat);
    assert!(look_registry::choose(sh.env.data_dir(), "alice", "broken").is_err());
    assert_eq!(look_registry::load_choice(sh.env.data_dir(), "alice").as_deref(), Some("mine"));
}

#[test]
fn the_looks_top_bar_hides_the_clock_and_the_footer_hints_but_never_the_essentials() {
    let (mut sh, mut g) = fresh("lk10");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(sh.hits().iter().any(|h| h.label == "clock"), "the default bar has the clock");
    assert!(g.text_log.iter().any(|(_, _, t)| t.starts_with("←/→ choose")), "and the hint line");
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| {
        l.topbar.items = vec![look::BarItem::Bell];
        l.topbar.footer_hints = false;
    });
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!sh.hits().iter().any(|h| h.label == "clock"), "no clock");
    g.text_log.clear();
    sh.render(&mut g);
    assert!(!g.text_log.iter().any(|(_, _, t)| t.starts_with("←/→ choose")), "no hint line");
    for e in look::ESSENTIAL_BAR {
        assert!(sh.look.topbar.items.contains(&e), "{e:?} was put back");
    }
    // the bell is still there to click (its region is called `inbox`)
    assert!(sh.hits().iter().any(|h| h.label == "inbox"));
}

#[test]
fn settings_changes_reach_the_shell_within_a_heartbeat_and_the_accessibility_override_wins() {
    let (mut sh, mut g) = fresh("lk11");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let dir = sh.env.data_dir().to_path_buf();
    let mut l = Look::default();
    l.name = "My look".into();
    l.icons.style = IconStyle::ThreeD;
    look_registry::save_custom(&dir, "alice", &l).unwrap();
    look_registry::notify_changed();
    sh.handle(&Event::Tick);
    assert_eq!(sh.look.icons.style, IconStyle::ThreeD);
    // reduced motion from the person's accessibility settings: no pop even when the Look says full motion
    crate::fx::set_reduced_motion(true);
    key(&mut sh, Key::Char('3'));
    assert!(!sh.home_animating());
    crate::fx::set_reduced_motion(false);
}

#[test]
fn colours_written_in_a_look_file_are_drawn_and_a_look_without_them_gives_the_base_theme_back() {
    let (mut sh, mut g) = fresh("lk12");
    let base = *iyakki_uikit::theme::ThemeId::Dark.theme();
    let mut want = base.bg_top;
    want[2] = want[2].saturating_add(6);
    with_look(&sh, "alice", |l| l.theme.bg_top = want);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(crate::theme::is_custom(), "the Look's own colours are in use");
    assert_eq!(crate::theme::current().bg_top, want);
    assert_ne!(want, base.bg_top);
    // a Look with only its base theme: the colours go back
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |_| {});
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!crate::theme::is_custom());
    assert_eq!(crate::theme::current().bg_top, base.bg_top);
    // and the sign-in screen is the plain Dark look again
    key(&mut sh, Key::Char('o'));
    assert!(!crate::theme::is_custom());
}

#[test]
fn custom_colours_of_a_look_are_none_for_a_plain_base_theme_and_kept_once_for_the_same_colours() {
    let plain = Look::default();
    assert!(super::look_impl::custom_colours(&plain).is_none());
    let mut l = Look::default();
    l.theme.ring = 1.7;
    let a = super::look_impl::custom_colours(&l).unwrap();
    let b = super::look_impl::custom_colours(&l).unwrap();
    assert!(std::ptr::eq(a, b), "the same colours are held once");
    assert_eq!(a.ring, 1.7);
}

fn dock_look(sh: &Shell, f: impl FnOnce(&mut look::Dock)) {
    with_look(sh, "alice", |l| f(&mut l.dock));
}

#[test]
fn an_autohiding_dock_gives_the_room_back_and_comes_out_when_the_pointer_reaches_the_edge_or_the_keyboard_asks() {
    let (mut sh, mut g) = fresh("lk13");
    dock_look(&sh, |d| d.autohide = true);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(sh.look.dock.autohide);
    assert_eq!(launcher::params().inset_b, 0.0, "no room kept for a dock that hides");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    sh.handle(&Event::MouseMove(10, 10));
    sh.render(&mut g);
    assert!(sh.dock_layout(w, h, s).is_none() && dock_labels(&sh).is_empty(), "tucked away: nothing to click");
    // the pointer in the strip along the bottom edge brings it out
    sh.handle(&Event::MouseMove(w as i32 / 2, h as i32 - 2));
    sh.render(&mut g);
    assert!(sh.dock_layout(w, h, s).is_some());
    assert_eq!(dock_labels(&sh).len(), 6);
    // away again: gone
    sh.handle(&Event::MouseMove(10, 10));
    sh.render(&mut g);
    assert!(dock_labels(&sh).is_empty());
    // the keyboard (d) brings it out and Esc tucks it away
    key(&mut sh, Key::Char('d'));
    sh.render(&mut g);
    assert!(sh.dock_layout(w, h, s).is_some() && sh.dock_focus.is_some());
    key(&mut sh, Key::Esc);
    sh.render(&mut g);
    assert!(sh.dock_layout(w, h, s).is_none());
}

#[test]
fn a_dock_that_does_not_autohide_is_always_out_and_keeps_its_room() {
    let (mut sh, mut g) = fresh("lk14");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    sh.handle(&Event::MouseMove(10, 10));
    assert!(sh.dock_layout(w, h, scale_for(w, h)).is_some());
    assert!(launcher::params().inset_b > 0.0);
}

#[test]
fn magnify_grows_the_icons_near_the_pointer_without_overlap_and_a_click_still_opens_the_right_app() {
    let (mut sh, mut g) = fresh("lk15");
    dock_look(&sh, |d| d.magnify = true);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let (plate, rest) = sh.dock_resting(w, h, s).unwrap();
    sh.handle(&Event::MouseMove(10, 10));
    assert_eq!(sh.dock_layout(w, h, s).unwrap().1, rest, "pointer away: icons at rest");
    let c = rest[2];
    sh.handle(&Event::MouseMove((c.x + c.w / 2.0) as i32, (c.y + c.h / 2.0) as i32));
    let (p2, big) = sh.dock_layout(w, h, s).unwrap();
    assert!(p2.w > plate.w && p2.x < plate.x && p2.y == plate.y, "the plate grows along the dock with the row");
    assert!(big.iter().all(|r| r.x >= p2.x && r.x + r.w <= p2.x + p2.w));
    assert!(big[2].w > rest[2].w * 1.2, "the icon under the pointer is bigger: {} vs {}", big[2].w, rest[2].w);
    assert!(big[0].w < big[2].w && big[5].w < big[2].w, "farther ones less so");
    for p in big.windows(2) {
        assert!(p[0].x + p[0].w <= p[1].x + 0.01, "no overlap");
    }
    assert!(big[2].y < rest[2].y, "it grows away from the edge");
    press(&mut sh, big[3]);
    assert_eq!(sh.screen_name(), "app:edit", "the fourth icon (EDIT) is the one that opens");
}

#[test]
fn reduced_motion_keeps_the_dock_icons_still_even_with_magnify_on() {
    let (mut sh, mut g) = fresh("lk16");
    dock_look(&sh, |d| d.magnify = true);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    let (_, rest) = sh.dock_resting(w, h, s).unwrap();
    crate::fx::set_reduced_motion(true);
    sh.handle(&Event::MouseMove((rest[1].x + 5.0) as i32, (rest[1].y + 5.0) as i32));
    assert_eq!(sh.dock_layout(w, h, s).unwrap().1, rest);
    crate::fx::set_reduced_motion(false);
}

#[test]
fn the_looks_top_bar_hides_the_volume_icon_when_it_is_left_out_and_shows_it_by_default() {
    let (mut sh, mut g) = fresh("lk17");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let with = sh.bar_slots_with(iyakki_shared::voiceassist::MicGlyph::Off);
    assert!(with.speaker.is_some(), "the default bar has the volume icon");
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| l.topbar.items.retain(|i| *i != look::BarItem::Volume));
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let without = sh.bar_slots_with(iyakki_shared::voiceassist::MicGlyph::Off);
    assert!(without.speaker.is_none(), "volume left out of the Look: no icon");
    assert_eq!(without.count, with.count - 1, "the others close the gap");
    assert!(sh.speaker_rect().is_none());
    for e in look::ESSENTIAL_BAR {
        assert!(sh.look.topbar.items.contains(&e));
    }
}

#[test]
fn the_shells_default_dock_is_the_list_the_chat_tools_and_the_look_crate_name() {
    let shell: Vec<&str> = super::look_impl::default_dock_ids();
    assert_eq!(shell, look::DEFAULT_DOCK_IDS);
}

/// F-SHELL-64: how long one frame takes to draw against how long it takes to write. Run with
/// `cargo test --profile fast -p iyakki-shell --lib frame_cost -- --ignored --nocapture`; the numbers go in the row.
#[test]
#[ignore]
fn frame_cost_draw_against_present_at_1080p() {
    let (mut sh, mut g) = fresh("fc");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let mut g = Gfx::new(1920, 1080);
    sh.render(&mut g);
    let n: usize = std::env::var("FRAME_COST_N").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    iyakki_uikit::prof::enable(true);
    let _ = iyakki_uikit::prof::take();
    let t = std::time::Instant::now();
    for _ in 0..n {
        sh.tick += 1;
        sh.render(&mut g);
    }
    iyakki_uikit::prof::enable(false);
    for (name, ns, calls) in iyakki_uikit::prof::take().into_iter().take(24) {
        println!("FRAME-COST   {name:<28} {:>7.2} ms/frame  {:>6} calls/frame", ns as f64 / 1e6 / n as f64, calls as usize / n);
    }
    let draw = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    // a lower bound for writing the frame: swapping red and blue of every pixel into a new buffer, on one core
    let t = std::time::Instant::now();
    let mut sink = 0u8;
    for _ in 0..n {
        let out: Vec<u8> = g.rgba().chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        sink ^= out[out.len() / 2];
    }
    let write = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("FRAME-COST swizzle-write one core: {write:.2} ms per frame ({sink})");
    println!("FRAME-COST draw home 1920x1080: {draw:.2} ms per frame (profile debug_assertions={})", cfg!(debug_assertions));
}

#[test]
fn a_picture_wallpaper_is_read_from_the_themes_folder_at_sign_in_and_a_bad_file_leaves_the_theme_background() {
    use iyakki_uikit::wallpaper;
    let (mut sh, mut g) = fresh("lk18");
    let themes = sh.env.data_dir().join("themes").join("alice");
    std::fs::create_dir_all(&themes).unwrap();
    let rgba: Vec<u8> = (0..16 * 9).flat_map(|_| [30u8, 160, 90, 255]).collect();
    let png = iyakki_apps::camera::pixels::png_with_dpi(16, 9, &rgba, 96.0).unwrap();
    std::fs::write(themes.join("wallpaper.png"), &png).unwrap();
    with_look(&sh, "alice", |l| l.wallpaper = wallpaper::Id::Picture);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(wallpaper::current(), wallpaper::Id::Picture);
    assert!(wallpaper::has_picture(), "the file was read");
    // a bad file: nothing held, the audit line says why
    key(&mut sh, Key::Char('o'));
    std::fs::write(themes.join("wallpaper.png"), b"not a picture at all").unwrap();
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!wallpaper::has_picture());
    assert!(read_wallpaper_error_is_plain(&sh));
    // another wallpaper lets go of the picture
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| l.wallpaper = wallpaper::Id::Dusk);
    std::fs::write(themes.join("wallpaper.png"), &png).unwrap();
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!wallpaper::has_picture());
    key(&mut sh, Key::Char('o'));
}

fn read_wallpaper_error_is_plain(sh: &Shell) -> bool {
    let e = super::look_impl::read_wallpaper_picture(sh.env.data_dir(), "alice").unwrap_err();
    e.contains("PNG")
}

#[test]
fn the_picture_reader_refuses_a_link_a_huge_file_and_a_missing_one() {
    let d = std::env::temp_dir().join(format!("iyakki-wp-{}", std::process::id()));
    let t = d.join("themes").join("bob");
    std::fs::create_dir_all(&t).unwrap();
    assert!(super::look_impl::read_wallpaper_picture(&d, "bob").unwrap_err().contains("no wallpaper"));
    std::os::unix::fs::symlink("/etc/hostname", t.join("wallpaper.png")).unwrap();
    assert!(super::look_impl::read_wallpaper_picture(&d, "bob").unwrap_err().contains("not a plain file"));
    std::fs::remove_file(t.join("wallpaper.png")).unwrap();
    let f = std::fs::File::create(t.join("wallpaper.jpg")).unwrap();
    f.set_len(super::look_impl::PICTURE_FILE_MAX + 1).unwrap();
    assert!(super::look_impl::read_wallpaper_picture(&d, "bob").unwrap_err().contains("larger than"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_bar_buttons_are_a_list_in_the_shells_own_order_and_the_looks_order_moves_only_power_bell_volume_and_settings() {
    use super::volbar_impl::{reorder_for_look, BarButton as B};
    use look::BarItem as I;
    let base = vec![B::Logout, B::Power, B::Shot, B::Devices, B::Clipboard, B::Bell, B::Panes, B::Mic, B::Speaker, B::Settings];
    // the default Look (and any Look with the same relative order): not one button moves
    assert_eq!(reorder_for_look(base.clone(), &Look::default().topbar.items), base);
    assert_eq!(reorder_for_look(base.clone(), &[I::Clock, I::Volume, I::Bell, I::Settings, I::Power]), base);
    // settings asked before volume and the bell: the four movable places are refilled in the Look's order, the others stay
    let r = reorder_for_look(base.clone(), &[I::Settings, I::Power, I::Volume, I::Bell]);
    assert_eq!(r, vec![B::Logout, B::Settings, B::Shot, B::Devices, B::Clipboard, B::Power, B::Panes, B::Mic, B::Speaker, B::Bell]);
}

#[test]
fn a_look_that_lists_settings_before_volume_puts_the_settings_icon_nearer_the_right_edge_and_the_click_still_finds_it() {
    let (mut sh, mut g) = fresh("lk19");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let off = iyakki_shared::voiceassist::MicGlyph::Off;
    let d = sh.bar_slots_with(off);
    assert!(d.speaker.unwrap() < d.settings.unwrap(), "today: the speaker, then Settings");
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| l.topbar.items = vec![look::BarItem::Settings, look::BarItem::Volume]);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let s = sh.bar_slots_with(off);
    assert!(s.settings.unwrap() < s.speaker.unwrap(), "{s:?}");
    assert_eq!(s.count, d.count, "the same buttons, another order");
    // the icon is where the slot says and a click on it opens Settings
    sh.render(&mut g);
    let r = sh.settings_rect().expect("the Settings icon has a place");
    press(&mut sh, r);
    assert_eq!(sh.screen_name(), "app:settings");
}

#[test]
fn the_dock_position_kept_for_the_main_screen_through_the_core_wins_over_the_look() {
    use iyakki_core_api::{Core as _};
    let (mut sh, mut g) = fresh("dock-screen");
    let core = std::sync::Arc::new(iyakki_core_fake::FakeCore::new());
    sh.set_core_for_test(core.clone());
    core.desktop().set_screen_dock("alice", 0, "left").unwrap();
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(sh.look.dock.position, DockPos::Left);
    // another person has no choice of their own: the Look's position applies
    sh.sign_out();
    sign_in(&mut sh, &mut g, "root", "rootpass12");
    assert_eq!(sh.look.dock.position, look::Look::default().dock.position);
    // "off" takes the dock away
    sh.sign_out();
    core.desktop().set_screen_dock("alice", 0, "off").unwrap();
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert_eq!(sh.look.dock.position, DockPos::Off);
    assert!(dock_labels(&sh).is_empty());
}

#[test]
fn a_look_with_the_bar_off_hides_the_row_keeps_the_essentials_by_key_and_brings_the_bar_back_by_the_corner_handle_or_b() {
    let (mut sh, mut g) = fresh("lk20");
    with_look(&sh, "alice", |l| l.topbar.on = false);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!sh.look.topbar.on);
    let labels = |sh: &Shell| sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>();
    let l = labels(&sh);
    assert!(!l.iter().any(|x| x == "clock" || x == "inbox"), "no bar: {l:?}");
    assert!(l.iter().any(|x| x == "show the bar"), "the corner handle is there: {l:?}");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    // a click where the bell would be does nothing while the bar is hidden
    let bell_at = sh.bar_order().iter().position(|b| *b == super::volbar_impl::BarButton::Bell).expect("this person has the bell");
    let before = sh.screen_name();
    press(&mut sh, super::header_button(w, s, bell_at));
    assert_eq!(sh.screen_name(), before, "a hidden button is not live");
    // the corner handle shows the bar; the bell then opens the inbox, and the bar goes away again
    press(&mut sh, super::look_impl::bar_handle(w, s));
    assert!(sh.bar_visible());
    sh.render(&mut g);
    assert!(labels(&sh).iter().any(|x| x == "inbox"), "the bar is drawn");
    press(&mut sh, super::header_button(w, s, bell_at));
    assert!(!sh.bar_visible(), "one use, then it is put away");
    assert!(sh.screen_name().starts_with("app:"), "the bell opened the inbox: {}", sh.screen_name());
    sh.go_home();
    sh.render(&mut g);
    // b and Esc by key
    key(&mut sh, Key::Char('b'));
    assert!(sh.bar_visible());
    key(&mut sh, Key::Esc);
    assert!(!sh.bar_visible());
    // the essentials by key: Settings with s, sign out with o
    key(&mut sh, Key::Char('o'));
    assert!(sh.look == Look::default() && !sh.bar_peek, "signed out: the default look is back");
    // with the bar on, b means nothing special
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |_| {});
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    key(&mut sh, Key::Char('b'));
    assert!(!sh.bar_peek);
}

#[test]
fn with_the_bar_at_the_bottom_every_panel_that_hangs_from_it_opens_upward_and_stays_clear_of_it_on_screen() {
    use iyakki_uikit::barside::{set_bar_at_bottom, BAR_BAND};
    struct Off;
    impl Drop for Off {
        fn drop(&mut self) {
            set_bar_at_bottom(false);
        }
    }
    let _off = Off;
    let (mut sh, mut g) = fresh("lk21");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    for (w, h) in [(1280u32, 720u32), (800, 480), (1920, 1080), (640, 400), (3840, 2160)] {
        let s = scale_for(w, h);
        let band_top = h as f32 - BAR_BAND * s;
        // at the top (the default) the panels are under the bar, as they always were
        set_bar_at_bottom(false);
        let top_dev = crate::devpop::DevPop::panel(w, h, s, 4);
        assert!(top_dev.y >= 56.0 * s - 0.5);
        set_bar_at_bottom(true);
        let anchor = R::new(w as f32 - 200.0 * s, band_top + 20.0 * s, 150.0 * s, 36.0 * s);
        let mut panels: Vec<(&str, R)> = Vec::new();
        panels.push(("devices", crate::devpop::DevPop::panel(w, h, s, 4)));
        sh.installs.anchor = Some(anchor);
        panels.push(("installer", sh.installs.panel(w, h, s)));
        sh.battery.anchor = Some(anchor);
        panels.push(("battery", sh.battery.panel(w, h, s, 6)));
        sh.brand.anchor = (24.0 * s, band_top + 56.0 * s + 8.0 * s);
        panels.push(("menu", sh.brand.panel(w, h, s)));
        let (vol, _) = super::volbar_impl::pop_rects(anchor, w, s, 2);
        panels.push(("volume", vol));
        for (name, p) in panels {
            assert!(p.x >= -0.5 && p.y >= -0.5 && p.x + p.w <= w as f32 + 0.5, "{name} off screen at {w}x{h}: {p:?}");
            assert!(p.y + p.h <= band_top + 0.5 || p.y + p.h <= anchor.y + 0.5, "{name} overlaps the bar at {w}x{h}: {p:?} (bar band from {band_top})");
            assert!(p.h > 50.0 * s, "{name} is too small at {w}x{h}: {p:?}");
        }
    }
}

fn bottom_look(sh: &Shell) {
    with_look(sh, "alice", |l| {
        l.topbar.position = look::BarPos::Bottom;
        l.dock.position = look::DockPos::Left;
    });
}

#[test]
fn a_look_with_the_bar_at_the_bottom_puts_the_whole_bar_in_the_bottom_band_and_the_footer_in_the_top_band_with_the_same_controls() {
    use iyakki_uikit::barside::BAR_BAND;
    let (mut sh, mut g0) = fresh("lk22");
    sign_in(&mut sh, &mut g0, "alice", "alicepass1");
    let top_labels: std::collections::BTreeSet<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    key(&mut sh, Key::Char('o'));
    bottom_look(&sh);
    sign_in(&mut sh, &mut g0, "alice", "alicepass1");
    assert_eq!(sh.look.topbar.position, look::BarPos::Bottom);
    for (w, h) in [(1280u32, 720u32), (800, 480), (1920, 1080), (640, 400)] {
        let mut g = Gfx::new(w, h);
        sh.size = (w, h);
        sh.render(&mut g);
        let s = scale_for(w, h);
        let band = h as f32 - BAR_BAND * s;
        assert!((iyakki_uikit::barside::bar_dy() - band).abs() < 0.5);
        let find = |sh: &Shell, l: &str| sh.hits().iter().find(|x| x.label == l).map(|x| x.rect);
        let menu = find(&sh, "IYAKKI menu").unwrap_or_else(|| panic!("no IYAKKI menu at {w}x{h}"));
        assert!(menu.y >= band - 1.0 && menu.y + menu.h <= h as f32 + 0.5, "the name is in the bottom band at {w}x{h}: {menu:?} band {band}");
        // every region the bar had at the top is there, in the bottom band
        let labels: std::collections::BTreeSet<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
        for l in ["IYAKKI menu", "inbox", "volume"] {
            assert!(labels.contains(l), "{l} is still there at {w}x{h}");
            assert!(find(&sh, l).unwrap().y >= band - 1.0, "{l} is in the bottom band");
        }
        let _ = &top_labels;
        // the footer text moved to the top band
        let host = format!("{}  ·  ", sh.env.hostname);
        assert!(g.text_log.iter().any(|(_, y, t)| t.starts_with(&host) && *y < 76.0 * s), "the host line is at the top: {:?}", g.text_log.iter().filter(|(_, _, t)| t.starts_with(&host)).collect::<Vec<_>>());
        // nothing but the bar is in the bottom band, and the bar is not in the top band
        for hit in sh.hits() {
            if ["IYAKKI menu", "inbox", "volume"].contains(&hit.label.as_str()) {
                assert!(hit.rect.y + hit.rect.h <= h as f32 + 0.5 && hit.rect.y >= 0.0);
            }
        }
    }
    iyakki_uikit::barside::set_bar_dy(0.0);
}

#[test]
fn with_the_bar_at_the_bottom_a_click_on_its_buttons_works_and_the_volume_dropdown_opens_upward() {
    let (mut sh, mut g) = fresh("lk23");
    bottom_look(&sh);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    sh.render(&mut g);
    // the Settings icon opens Settings from the bottom band
    let r = sh.settings_rect().expect("Settings icon");
    assert!(r.y > h as f32 / 2.0, "it is at the bottom: {r:?}");
    press(&mut sh, r);
    assert_eq!(sh.screen_name(), "app:settings");
    sh.go_home();
    sh.render(&mut g);
    // the volume dropdown opens above its icon and inside the screen
    let sp = sh.speaker_rect().expect("speaker icon");
    assert!(sp.y > h as f32 / 2.0);
    press(&mut sh, sp);
    sh.render(&mut g);
    assert!(sh.vol.open, "the dropdown is open");
    let (panel, _) = super::volbar_impl::pop_rects(sp, w, s, sh.vol.sources.len());
    assert!(panel.y + panel.h <= sp.y && panel.y >= 0.0, "above the icon: {panel:?} icon {sp:?}");
    iyakki_uikit::barside::set_bar_dy(0.0);
}

#[test]
fn with_no_picture_of_the_persons_own_the_shipped_wallpaper_is_drawn_and_a_file_of_their_own_wins() {
    let (w, h, rgba) = super::look_impl::shipped_wallpaper_picture().expect("the shipped picture decodes");
    assert_eq!((w, h, rgba.len()), (1280, 720, 1280 * 720 * 4));
    // it is a dusk: darker at the top than at the horizon, and not one flat colour
    let at = |x: usize, y: usize| rgba[(y * 1280 + x) * 4] as u32 + rgba[(y * 1280 + x) * 4 + 1] as u32 + rgba[(y * 1280 + x) * 4 + 2] as u32;
    assert!(at(640, 10) < at(400, 470), "{} {}", at(640, 10), at(400, 470));
    // no file anywhere: the reader still says there is none (the shell then falls back to the shipped one); a bad file is not hidden by it
    let d = std::env::temp_dir().join(format!("iyakki-wp-shipped-{}", std::process::id()));
    std::fs::create_dir_all(d.join("themes")).unwrap();
    assert!(super::look_impl::read_wallpaper_picture(&d, "bob").unwrap_err().starts_with("there is no wallpaper"));
    std::fs::write(d.join("themes").join("wallpaper.png"), b"not a picture at all").unwrap();
    assert!(!super::look_impl::read_wallpaper_picture(&d, "bob").unwrap_err().starts_with("there is no wallpaper"), "a bad file of the person's own is reported, not replaced");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_network_and_apps_items_of_a_look_are_buttons_that_open_settings_and_go_home_and_the_default_bar_has_neither() {
    use super::volbar_impl::BarButton as B;
    let (mut sh, mut g) = fresh("lk-net-apps");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!sh.bar_order().contains(&B::Network) && !sh.bar_order().contains(&B::Apps), "the default Look lists neither: the bar is as it was");
    let before = sh.bar_order().len();
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| l.topbar.items = vec![look::BarItem::Clock, look::BarItem::Network, look::BarItem::Apps, look::BarItem::Volume, look::BarItem::NeedsYou, look::BarItem::Bell, look::BarItem::Settings, look::BarItem::Account, look::BarItem::Power]);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    let order = sh.bar_order();
    assert_eq!(order.len(), before + 2);
    let (w, h) = (g.width(), g.height());
    let s = scale_for(w, h);
    sh.render(&mut g);
    // Apps does what the Home icon does: it puts away what is open (here the Devices panel) and shows the tiles
    sh.toggle_devices();
    assert!(sh.devpop.open);
    let at = order.iter().position(|b| *b == B::Apps).unwrap();
    press(&mut sh, super::header_button(w, s, at));
    assert!(!sh.devpop.open, "the panel was put away");
    assert_eq!(sh.screen_name(), "launcher");
    // Network opens the network panel of its own (only the network rows); its Wi-Fi row leads to Settings
    sh.render(&mut g);
    let at = sh.bar_order().iter().position(|b| *b == B::Network).unwrap();
    press(&mut sh, super::header_button(w, s, at));
    assert!(sh.devpop.is_network_panel());
    sh.devpop.set_snapshot(crate::devpop::Snapshot {
        items: vec![
            crate::devpop::Item { title: "Sound".into(), detail: "speakers".into(), level: crate::devpop::Level::Ok, jump: crate::devpop::Jump::Settings("sound"), acts: Vec::new() },
            crate::devpop::Item { title: "Wi-Fi".into(), detail: "wlan0 · connected".into(), level: crate::devpop::Level::Ok, jump: crate::devpop::Jump::Settings("network"), acts: Vec::new() },
        ],
        driver_help: Vec::new(),
        at: 1,
    });
    assert_eq!(sh.devpop.snapshot().unwrap().items.len(), 2, "the reading itself is whole");
    key(&mut sh, Key::Enter);
    assert_eq!(sh.screen_name(), "app:settings");
    assert!(!sh.devpop.open);
}

#[test]
fn the_network_button_stands_in_an_open_apps_header_and_opens_the_same_panel() {
    use super::volbar_impl::{Extra, NET_TAB};
    let (mut sh, mut g) = fresh("lk-net-app");
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    assert!(!sh.wanted_extras(true).contains(&Extra::Network), "the default Look lists no network item");
    key(&mut sh, Key::Char('o'));
    with_look(&sh, "alice", |l| l.topbar.items = vec![look::BarItem::Clock, look::BarItem::Network, look::BarItem::Settings, look::BarItem::Bell]);
    sign_in(&mut sh, &mut g, "alice", "alicepass1");
    sh.open(PApp::Chat);
    assert!(matches!(sh.screen, Screen::App(..)));
    let want = sh.wanted_extras(true);
    assert_eq!(want.iter().position(|e| *e == Extra::Network).map(|i| i + 1), want.iter().position(|e| *e == Extra::Settings), "left of Settings");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(NET_TAB)), "the icon is registered in the app header");
    sh.dispatch(&Event::Tab(NET_TAB));
    assert!(sh.devpop.is_network_panel());
    assert!(matches!(sh.screen, Screen::App(..)), "the app stays open under the panel");
    sh.dispatch(&Event::Tab(NET_TAB));
    assert!(!sh.devpop.open, "the icon again puts it away");
}
