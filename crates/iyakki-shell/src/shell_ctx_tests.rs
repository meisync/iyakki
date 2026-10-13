//! The right-click context menu at the shell level: where it opens, what it offers over a text field, how it is driven by
//! keys and the mouse, and that its entries go through the existing clipboard plumbing.

use super::tests::*;
use super::*;
use crate::ctxmenu::{COPY, CUT, HISTORY, PASTE, SELECT_ALL};

fn ids(sh: &Shell) -> Vec<String> {
    sh.context_menu().expect("the menu is open").items.iter().map(|i| i.id.clone()).collect()
}

fn enabled(sh: &Shell, id: &str) -> bool {
    let m = sh.context_menu().expect("the menu is open");
    m.is_enabled(m.index_of(id).unwrap_or_else(|| panic!("no entry {id}")))
}

/// Click the entry with this id in the open menu.
fn pick(sh: &mut Shell, g: &mut Gfx, id: &str) {
    sh.render(g);
    let m = sh.context_menu().expect("the menu is open");
    let r = m.item_rect(m.index_of(id).unwrap_or_else(|| panic!("no entry {id}"))).unwrap();
    let (x, y) = (r.x as i32 + 10, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
}

fn field_centre(sh: &mut Shell, g: &mut Gfx, id: usize) -> (i32, i32) {
    sh.render(g);
    let h = sh.hits().iter().rev().find(|h| h.label == "field" && h.ev == Event::Focus(id)).unwrap_or_else(|| panic!("no field {id}")).clone();
    ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32)
}

fn field_text(sh: &Shell, i: usize) -> String {
    match &sh.screen {
        Screen::Login(f) => f.fields[i].value.clone(),
        _ => panic!("not the login screen"),
    }
}

#[test]
fn a_right_click_opens_a_menu_at_the_pointer_inside_the_screen_and_it_closes_on_escape() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("ctx1");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(5, 5));
    sh.render(&mut g);
    let m = sh.context_menu().expect("a right click opens the menu, not the history");
    assert!(!sh.clipboard_picker_open());
    assert_eq!((m.rect.x, m.rect.y), (5.0, 5.0), "its corner is at the pointer");
    assert_eq!(ids(&sh), vec![HISTORY], "away from any text only the history is offered");
    sh.handle(&Event::Key(Key::Esc));
    assert!(sh.context_menu().is_none());
    // near the bottom right corner it moves back inside
    sh.handle(&Event::MouseRight(1279, 719));
    let r = sh.context_menu().unwrap().rect;
    assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 1280.0 && r.y + r.h <= 720.0, "{r:?}");
    sh.render(&mut g);
    // a second right click moves it
    sh.handle(&Event::MouseRight(300, 200));
    assert_eq!((sh.context_menu().unwrap().rect.x, sh.context_menu().unwrap().rect.y), (300.0, 200.0));
    // the system clipboard was asked for, so Paste can become usable
    assert!(sh.take_host_paste_request());
}

#[test]
fn over_a_text_field_it_offers_cut_copy_paste_and_select_all_enabled_by_what_applies() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("ctx2");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    typ(&mut sh, "hello");
    let (x, y) = field_centre(&mut sh, &mut g, 0);
    sh.handle(&Event::MouseRight(x, y));
    assert_eq!(ids(&sh), vec![CUT, COPY, PASTE, SELECT_ALL, "", HISTORY]);
    let labels: Vec<String> = sh.context_menu().unwrap().items.iter().map(|i| i.shortcut.clone()).collect();
    assert_eq!(&labels[..4], ["Ctrl+X", "Ctrl+C", "Ctrl+V", "Ctrl+A"]);
    assert!(!enabled(&sh, CUT) && !enabled(&sh, COPY), "nothing is selected");
    assert!(!enabled(&sh, PASTE), "the clipboard is empty");
    assert!(enabled(&sh, SELECT_ALL) && enabled(&sh, HISTORY));
    // Select all
    pick(&mut sh, &mut g, SELECT_ALL);
    assert!(sh.context_menu().is_none(), "choosing closes the menu");
    match &sh.screen {
        Screen::Login(f) => assert!(f.fields[0].is_all_selected()),
        _ => unreachable!(),
    }
    // now Cut and Copy are usable; Copy puts it on the clipboard (and the system one)
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(x, y));
    assert!(enabled(&sh, CUT) && enabled(&sh, COPY));
    pick(&mut sh, &mut g, COPY);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("hello"));
    assert_eq!(crate::ui::clipboard::take_host_copy().as_deref(), Some("hello"));
    assert_eq!(field_text(&sh, 0), "hello", "Copy leaves the text");
    // Cut removes it, and Paste (now usable) puts it back
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(x, y));
    assert!(enabled(&sh, PASTE));
    pick(&mut sh, &mut g, CUT);
    assert_eq!(field_text(&sh, 0), "");
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(x, y));
    assert!(!enabled(&sh, SELECT_ALL), "an empty field has nothing to select");
    pick(&mut sh, &mut g, PASTE);
    assert_eq!(field_text(&sh, 0), "hello");
}

#[test]
fn a_right_click_on_another_field_focuses_it_and_a_password_cannot_be_copied() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("ctx3");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    typ(&mut sh, "alice");
    let (x, y) = field_centre(&mut sh, &mut g, 1);
    sh.handle(&Event::MouseRight(x, y));
    assert!(matches!(&sh.screen, Screen::Login(f) if f.focus == 1), "the field under the pointer gets the focus");
    sh.handle(&Event::Key(Key::Esc));
    typ(&mut sh, "secret-pass");
    sh.render(&mut g);
    key(&mut sh, Key::Ctrl('a'));
    sh.handle(&Event::MouseRight(x, y));
    assert!(!enabled(&sh, COPY) && !enabled(&sh, CUT), "a password is never copied, even when selected");
    assert!(enabled(&sh, SELECT_ALL));
}

#[test]
fn the_keyboard_drives_the_menu_and_clipboard_history_opens_the_picker() {
    crate::ui::clipboard::clear();
    crate::ui::clipboard::remember("older");
    let (mut sh, _) = shell_with_users("ctx4");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    typ(&mut sh, "abc");
    let (x, y) = field_centre(&mut sh, &mut g, 0);
    sh.handle(&Event::MouseRight(x, y));
    // Cut and Copy are off, so the first usable entry (Paste) is highlighted; Down goes on, wrapping past the history
    let sel = |sh: &Shell| sh.context_menu().unwrap().items[sh.context_menu().unwrap().sel].id.clone();
    assert_eq!(sel(&sh), PASTE);
    key(&mut sh, Key::Down);
    assert_eq!(sel(&sh), SELECT_ALL);
    key(&mut sh, Key::Down);
    assert_eq!(sel(&sh), HISTORY, "the separator is skipped");
    key(&mut sh, Key::Down);
    assert_eq!(sel(&sh), PASTE);
    key(&mut sh, Key::Up);
    assert_eq!(sel(&sh), HISTORY);
    // Enter on Paste puts the latest copy in the focused field
    key(&mut sh, Key::Down);
    key(&mut sh, Key::Enter);
    assert_eq!(field_text(&sh, 0), "abcolder");
    // Clipboard history…
    sh.handle(&Event::MouseRight(x, y));
    pick(&mut sh, &mut g, HISTORY);
    assert!(sh.context_menu().is_none() && sh.clipboard_picker_open(), "the history picker opens");
    key(&mut sh, Key::Esc);
    assert!(!sh.clipboard_picker_open());
    // Ctrl+Shift+V still opens it directly
    key(&mut sh, Key::Ctrl('V'));
    assert!(sh.clipboard_picker_open());
}

#[test]
fn a_click_elsewhere_a_scroll_or_a_key_closes_it_and_a_disabled_entry_does_nothing() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("ctx5");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    let (x, y) = field_centre(&mut sh, &mut g, 0);
    sh.handle(&Event::MouseRight(x, y));
    // Copy is disabled (no selection): clicking it leaves the menu open and copies nothing
    pick(&mut sh, &mut g, COPY);
    assert!(sh.context_menu().is_some());
    assert!(crate::ui::clipboard::latest().is_none());
    // a click on the Sign in button closes the menu and is swallowed (it does not press the button)
    sh.render(&mut g);
    let b = sh.hits().iter().find(|h| h.label == "Sign in").expect("button").rect;
    sh.handle(&Event::MouseRight(x, y));
    sh.handle(&Event::MouseDown((b.x + 5.0) as i32, (b.y + 5.0) as i32));
    assert!(sh.context_menu().is_none());
    assert_eq!(sh.screen_name(), "login");
    assert!(!matches!(&sh.screen, Screen::Login(f) if f.error.is_some()), "nothing was submitted");
    // a scroll closes it
    sh.handle(&Event::MouseRight(x, y));
    sh.handle(&Event::Scroll(1));
    assert!(sh.context_menu().is_none());
    // another key closes it and still does what it would have done
    sh.handle(&Event::MouseRight(x, y));
    typ(&mut sh, "z");
    assert!(sh.context_menu().is_none());
    assert_eq!(field_text(&sh, 0), "z");
    // and so do heartbeats not (they leave it open)
    sh.handle(&Event::MouseRight(x, y));
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.context_menu().is_some());
}

#[test]
fn an_app_adds_its_own_entries_the_shell_carries_them_out_and_the_history_stays_last() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("ctx6");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.open(PApp::Cli);
    assert!(sh.screen_name().starts_with("app"), "{}", sh.screen_name());
    for c in "whoami".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    sh.render(&mut g);
    let area = sh.hits().iter().find(|h| h.label == "text area").expect("the terminal output takes presses").rect;
    sh.handle(&Event::MouseRight(area.x as i32 + 30, area.y as i32 + 10));
    assert_eq!(ids(&sh), vec![COPY, SELECT_ALL, "", HISTORY]);
    assert!(!enabled(&sh, COPY), "nothing is selected yet");
    pick(&mut sh, &mut g, SELECT_ALL);
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(area.x as i32 + 30, area.y as i32 + 10));
    assert!(enabled(&sh, COPY));
    pick(&mut sh, &mut g, COPY);
    let copied = crate::ui::clipboard::latest().expect("copied");
    assert!(copied.contains("whoami"), "{copied:?}");
    assert_eq!(crate::ui::clipboard::take_host_copy().as_deref(), Some(copied.as_str()));
}
