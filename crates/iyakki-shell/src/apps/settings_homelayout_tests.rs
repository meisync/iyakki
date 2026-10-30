//! Settings -> Home layout: groups made, renamed, put in order and deleted, tiles moved and put back, Reset layout, the checks, the
//! built-in groups' protection, who sees it (everyone, for their own layout) and the page at four sizes.

use super::*;
use super::homelayout_page::HlRow;
use iyakki_sec::policy::Role;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::layout::Layout;

fn user(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto(a: &mut SettingsApp, _cx: &AppCx) {
    a.go_to(Tab::HomeLayout);
}
fn layout(cx: &AppCx) -> Layout {
    Layout::load(cx.services.dir(), &cx.session.user)
}
/// Move the cursor to the row whose first column says `name`.
fn select(a: &mut SettingsApp, cx: &AppCx, name: &str) {
    let rows = SettingsApp::hl_rows(cx);
    let i = rows.iter().position(|r| matches!(r, HlRow::Group { name: n, .. } | HlRow::Tile { title: n, .. } if n == name)).unwrap_or_else(|| panic!("no row {name}"));
    a.sel[a.tab] = i;
}

#[test]
fn everyone_has_the_page_and_it_makes_renames_orders_and_deletes_groups() {
    let cx = user("hl1");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx);
    let rows = SettingsApp::hl_rows(&cx);
    assert!(matches!(&rows[0], HlRow::Group { name, builtin: true, .. } if name == "Ask & Create"));
    assert_eq!(rows.iter().filter(|r| matches!(r, HlRow::Group { .. })).count(), 6);
    // new group, with the checks
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "work");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a built-in name is refused and the form stays");
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "Games");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal() && status(&a).contains("Games"), "{}", status(&a));
    assert_eq!(layout(&cx).groups.len(), 1);
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "GAMES");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a duplicate in another case is refused");
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "Music");
    key(&mut a, &cx, Key::Enter);
    // order
    select(&mut a, &cx, "Music");
    key(&mut a, &cx, Key::Char('['));
    assert_eq!(layout(&cx).groups.iter().map(|g| g.label.clone()).collect::<Vec<_>>(), vec!["Music", "Games"]);
    // rename
    select(&mut a, &cx, "Games");
    key(&mut a, &cx, Key::Char('r'));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "Play");
    key(&mut a, &cx, Key::Enter);
    assert!(layout(&cx).groups.iter().any(|g| g.label == "Play"));
    // built-in groups cannot be renamed, deleted or ordered
    for (k, what) in [('r', "names"), ('x', "deleted"), ('[', "stay")] {
        select(&mut a, &cx, "Build");
        key(&mut a, &cx, Key::Char(k));
        assert!(!a.has_modal(), "{k}");
        assert!(status(&a).contains(what), "{k}: {}", status(&a));
    }
    // delete: a card says how many tiles go back, y confirms
    select(&mut a, &cx, "CHAT");
    key(&mut a, &cx, Key::Char('m'));
    assert!(a.has_modal(), "a list of groups");
    typ(&mut a, &cx, "");
    // choose "Play" from the list: down to it, Enter
    for _ in 0..8 {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(layout(&cx).placed.get("sys:chat").map(|g| g.starts_with('c')), Some(true), "{:?} {}", layout(&cx).placed, status(&a));
    select(&mut a, &cx, "Play");
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Char('n'));
    assert_eq!(layout(&cx).groups.len(), 2, "any other key cancels");
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("1 tile went back"), "{}", status(&a));
    assert_eq!(layout(&cx).groups.len(), 1);
    assert!(layout(&cx).placed.is_empty());
}

#[test]
fn tiles_are_moved_put_back_and_reset_and_a_plain_user_has_the_page_for_their_own_layout() {
    let mut cx = user("hl2");
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    cx.session.user = "alice".into();
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::HomeLayout), "not only administrators");
    goto(&mut a, &cx);
    assert!(!SettingsApp::hl_rows(&cx).iter().any(|r| matches!(r, HlRow::Tile { title, .. } if title == "USERS")), "a tile the person cannot see is not listed");
    select(&mut a, &cx, "CHAT");
    key(&mut a, &cx, Key::Char('m'));
    key(&mut a, &cx, Key::Enter);
    let l = layout(&cx);
    assert!(l.placed.contains_key("sys:chat"), "{:?}", l.placed);
    assert!(SettingsApp::hl_rows(&cx).iter().any(|r| matches!(r, HlRow::Tile { title, moved: true, .. } if title == "CHAT")));
    key(&mut a, &cx, Key::Char('p'));
    assert!(layout(&cx).placed.is_empty());
    // reset
    key(&mut a, &cx, Key::Char('m'));
    key(&mut a, &cx, Key::Enter);
    assert!(!layout(&cx).placed.is_empty());
    key(&mut a, &cx, Key::Char('z'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Char('y'));
    assert!(layout(&cx).placed.is_empty() && status(&a).contains("back"));
    // it is this person's file only
    assert!(Layout::load(cx.services.dir(), "root").is_default());
}

#[test]
fn the_page_draws_at_four_sizes() {
    let cx = user("hl3");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let mut a = SettingsApp::new(&cx);
        goto(&mut a, &cx);
        let mut g = Gfx::new(w, h);
        let s = crate::shell::scale_for(w, h);
        a.sel[a.tab] = 30;
        a.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), s, &cx, 0);
        assert!(g.hits.iter().all(|hh| hh.rect.x + hh.rect.w <= w as f32 + 1.0), "{w}x{h}");
    }
}
