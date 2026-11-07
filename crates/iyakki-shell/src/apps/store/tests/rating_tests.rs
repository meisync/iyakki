//! Tests of "Your rating" on a package's page: stars and a private comment per account, reached by mouse, touch (the same clicks)
//! and keyboard, drawn at the four screen sizes, kept when the package updates, forgotten only on request, and never leaving the
//! computer. The file itself is tested in iyakki-agent (`store::ratings`).

use super::*;
use iyakki_agent::store::ratings::{Ratings, MAX_COMMENT};

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

fn rating_of(cx: &AppCx, a: &StoreApp, name: &str) -> Option<iyakki_agent::store::ratings::Rating> {
    Ratings::load(a.store.dir(), &cx.session.user).get(&format!("tool:{name}")).cloned()
}

fn audit_log(tag: &str) -> String {
    std::fs::read_to_string(iyakki_agent::testdir::find("iyakki-shell", tag).join("audit.log")).unwrap_or_default()
}

/// Installed page of `shout`, open on its page.
fn open_installed(tag: &str) -> (AppCx, StoreApp, SigningKey, std::path::PathBuf) {
    let (cx, k, d) = world(tag);
    publish(&d, 1, &k, &[tool("shout", "1.0.0", &["net:example.com"], &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    install_cat(&mut a, &cx, "shout");
    a.switch(Section::Installed);
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(a.mode, Mode::Detail) && a.idetail.is_some());
    (cx, a, k, d)
}

#[test]
fn the_page_shows_your_stars_and_comment_at_the_four_sizes_and_the_mouse_and_keyboard_reach_them() {
    let (cx, mut a, _k, _d) = open_installed("rt-inst");
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        let l = labels(&g);
        for n in 1..=5 {
            assert!(l.iter().any(|x| x.starts_with(&format!("Rate {n} star"))), "{w}x{h}: {l:?}");
        }
        assert!(l.contains(&"Add a comment (w)".to_string()) && !l.contains(&"Forget my rating (f)".to_string()), "{w}x{h}: nothing to forget yet");
        all_inside(&g, w, h, "the page with the rating row");
        no_overlap(&g, w, h, |e| matches!(e, Event::Tab(240..=249) | Event::Tab(5701..=5711) | Event::Tab(307)), "the buttons and stars");
    }
    // mouse: a click on the 4th star
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(rating::TAB_STAR + 4));
    assert_eq!(rating_of(&cx, &a, "shout").unwrap().stars, 4);
    assert!(a.status_text().unwrap().contains("4 stars"), "{:?}", a.status_text());
    // keyboard: 2 replaces it, the same number again takes the stars away
    key(&mut a, &cx, Key::Char('2'));
    assert_eq!(rating_of(&cx, &a, "shout").unwrap().stars, 2);
    key(&mut a, &cx, Key::Char('2'));
    assert!(rating_of(&cx, &a, "shout").is_none());
    key(&mut a, &cx, Key::Char('5'));
    // a comment by keyboard: w opens the field, typing fills it, Enter saves and returns to the page
    key(&mut a, &cx, Key::Char('w'));
    assert!(matches!(a.mode, Mode::Edit(_)));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(labels(&g).contains(&"Save (Enter)".to_string()), "{w}x{h}");
        all_inside(&g, w, h, "the comment dialog");
    }
    typed(&mut a, &cx, "Quick and tidy");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(a.mode, Mode::Detail), "back on the page");
    let r = rating_of(&cx, &a, "shout").unwrap();
    assert_eq!((r.stars, r.comment.as_str(), r.version.as_str()), (5, "Quick and tidy", "1.0.0"));
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        let l = labels(&g);
        assert!(l.contains(&"Edit comment (w)".to_string()) && l.contains(&"Forget my rating (f)".to_string()), "{w}x{h}: {l:?}");
        all_inside(&g, w, h, "the page with a rating and a comment");
        no_overlap(&g, w, h, |e| matches!(e, Event::Tab(240..=249) | Event::Tab(5701..=5711) | Event::Tab(307)), "the buttons and stars with a comment");
    }
    // mouse: Edit comment opens the dialog with the old text; Esc leaves it unchanged; an empty text removes just the comment
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(rating::TAB_WRITE));
    let Mode::Edit(st) = &a.mode else { panic!("the dialog") };
    assert_eq!(st.field.value, "Quick and tidy");
    key(&mut a, &cx, Key::Esc);
    assert!(matches!(a.mode, Mode::Detail) && rating_of(&cx, &a, "shout").unwrap().comment == "Quick and tidy");
    key(&mut a, &cx, Key::Char('w'));
    for _ in 0..20 {
        key(&mut a, &cx, Key::Backspace);
    }
    key(&mut a, &cx, Key::Enter);
    let r = rating_of(&cx, &a, "shout").unwrap();
    assert_eq!((r.stars, r.comment.as_str()), (5, ""));
    // forget: by mouse, and the keyboard says there is nothing left
    click_ev(&mut a, &cx, 1280, 720, Event::Tab(rating::TAB_FORGET));
    assert!(rating_of(&cx, &a, "shout").is_none() && a.status_text().unwrap().contains("forgotten"));
    key(&mut a, &cx, Key::Char('f'));
    assert!(a.status_text().unwrap().contains("no rating"), "{:?}", a.status_text());
}

#[test]
fn a_comment_with_a_password_or_over_500_characters_is_refused_in_plain_words_and_nothing_is_stored_or_logged() {
    let (cx, mut a, _k, _d) = open_installed("rt-guard");
    key(&mut a, &cx, Key::Char('w'));
    typed(&mut a, &cx, "my password: hunter2hunter2 works");
    key(&mut a, &cx, Key::Enter);
    let say = a.status_text().unwrap();
    assert!(say.contains("credential") && !say.contains("hunter2"), "{say}");
    assert!(rating_of(&cx, &a, "shout").is_none());
    // too long
    key(&mut a, &cx, Key::Char('w'));
    if let Mode::Edit(st) = &mut a.mode {
        st.field.max = 2000;
        st.field.set(&"word ".repeat(MAX_COMMENT));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(a.status_text().unwrap().contains("at most 500"), "{:?}", a.status_text());
    assert!(rating_of(&cx, &a, "shout").is_none());
    // exactly 500 is fine
    key(&mut a, &cx, Key::Char('w'));
    if let Mode::Edit(st) = &mut a.mode {
        st.field.max = 2000;
        st.field.set(&format!("{}word!", "word ".repeat(99)));
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(rating_of(&cx, &a, "shout").unwrap().comment.len(), MAX_COMMENT);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        all_inside(&g, w, h, "a long comment on the page");
    }
    // the audit line names the package and never holds the comment
    key(&mut a, &cx, Key::Char('3'));
    key(&mut a, &cx, Key::Char('f'));
    let log = audit_log("rt-guard");
    assert!(log.contains("store.rate-comment") && log.contains("\"store.rate\"") && log.contains("store.rate-forget") && log.contains("shout"), "{log}");
    assert!(!log.contains("hunter2") && !log.contains("word word"), "no comment text in the audit");
}

#[test]
fn the_rating_stays_when_the_package_updates_and_in_the_next_session_and_belongs_to_one_account() {
    let (cx, mut a, k, d) = open_installed("rt-upd");
    key(&mut a, &cx, Key::Char('4'));
    key(&mut a, &cx, Key::Char('w'));
    typed(&mut a, &cx, "Works well");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Esc);
    publish(&d, 2, &k, &[tool("shout", "1.1.0", &["net:example.com"], &k)]);
    a.reload(&cx);
    select(&mut a, "shout");
    key(&mut a, &cx, Key::Char('u'));
    wait_job(&mut a, &cx);
    if matches!(a.mode, Mode::Review(_)) {
        key(&mut a, &cx, Key::Char('y'));
        wait_job(&mut a, &cx);
    }
    assert_eq!(version_of(&cx, "tool", "shout"), "1.1.0", "{:?}", a.status_text());
    let r = rating_of(&cx, &a, "shout").unwrap();
    assert_eq!((r.stars, r.comment.as_str()), (4, "Works well"), "the update did not touch the rating");
    // a new session of the tile reads it back, and shows it on the page
    let mut b = StoreApp::new(&cx);
    b.switch(Section::Installed);
    select(&mut b, "shout");
    key(&mut b, &cx, Key::Enter);
    let g = draw(&mut b, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Forget my rating (f)".to_string()));
    // another account has none of it
    assert!(Ratings::load(b.store.dir(), "someone-else").is_empty());
    // the file is in the store folder as the person's own, not shared
    assert!(Ratings::path_of(b.store.dir(), &cx.session.user).exists());
}

#[test]
fn discover_pages_have_the_row_too_and_the_keys_do_nothing_elsewhere() {
    let (cx, k, d) = world("rt-disc");
    publish(&d, 1, &k, &[tool("shout", "1.0.0", &["net:example.com"], &k)]);
    let mut a = StoreApp::new(&cx);
    add_folder(&mut a, &cx, &d);
    cat_on(&mut a, &cx);
    // on the list, 3 and f are not for ratings
    key(&mut a, &cx, Key::Char('3'));
    assert!(rating_of(&cx, &a, "shout").is_none());
    assert!(a.items.iter().any(|i| i.entry.name == "shout"), "the stick's package is listed");
    a.detail = Some(("tool".into(), "shout".into()));
    a.mode = Mode::Detail;
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(labels(&g).contains(&"Add a comment (w)".to_string()), "{w}x{h}: {:?}", labels(&g));
        all_inside(&g, w, h, "the Discover page with the rating row");
        no_overlap(&g, w, h, |e| matches!(e, Event::Tab(300..=307) | Event::Tab(5701..=5711) | Event::Tab(205)), "the buttons and stars");
    }
    click_ev(&mut a, &cx, 1024, 600, Event::Tab(rating::TAB_STAR + 3));
    assert_eq!(rating_of(&cx, &a, "shout").map(|r| r.stars), Some(3), "{:?}: a package can be rated from its page before it is installed", a.status_text());
    key(&mut a, &cx, Key::Char('w'));
    typed(&mut a, &cx, "Looks useful");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(a.mode, Mode::Detail) && a.detail.is_some(), "the page stays open after the dialog");
    assert_eq!(rating_of(&cx, &a, "shout").unwrap().comment, "Looks useful");
}
