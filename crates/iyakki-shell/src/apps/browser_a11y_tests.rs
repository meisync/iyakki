//! Accessibility of the Browser tile, driven with the keyboard only and a fake voice that records what it was asked to say.
//! Three fixture pages (a shop, a sign-in form, an article) stand for the kinds of page the built-in engine shows; every link,
//! button and field of each is reached with Tab and activated with Enter or Space. Nothing here is heard by a person: see
//! docs/ACCESSIBILITY.md for what is and is not verified.
use super::a11y::{os_palette, PageSpeech};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_agent::browsera11y as cfg;
use iyakki_web::a11y::Role as WRole;
use iyakki_web::fetch::{Fetch, Response};
use std::collections::HashMap;
use std::sync::Mutex;

const SHOP: &str = include_str!("../../../iyakki-web/src/a11y_fixtures/shop.html");
const SIGNIN: &str = include_str!("../../../iyakki-web/src/a11y_fixtures/signin.html");
const ARTICLE: &str = include_str!("../../../iyakki-web/src/a11y_fixtures/article.html");

struct Site {
    pages: Mutex<HashMap<String, Response>>,
}

impl Fetch for Site {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let key = url.without_fragment().to_string();
        // every other address is a small page, so a followed link has somewhere to go
        Ok(self.pages.lock().unwrap().get(&key).cloned().unwrap_or_else(|| html("<html><head><title>Elsewhere</title></head><body><h1>Elsewhere</h1><p>Another page.</p></body></html>")))
    }
}

fn html(body: &str) -> Response {
    Response { status: 200, content_type: "text/html".into(), body: body.as_bytes().to_vec(), ..Response::default() }
}

fn cx_for(tag: &str, pages: Vec<(&str, &str)>) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    cx.web = Arc::new(Site { pages: Mutex::new(pages.into_iter().map(|(u, b)| (u.to_string(), html(b))).collect()) });
    cx
}

fn settle(a: &mut BrowserApp, cx: &AppCx) {
    for _ in 0..24_000 {
        a.handle(&Event::Tick, cx);
        if !a.is_loading() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the page never finished loading");
}

fn key(a: &mut BrowserApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn frame(a: &mut BrowserApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0), cx, 0);
    g
}

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<String>>>);

impl PageSpeech for Rec {
    fn focus(&mut self, t: &str) {
        self.0.lock().unwrap().push(format!("F:{t}"));
    }
    fn notice(&mut self, t: &str) {
        self.0.lock().unwrap().push(format!("N:{t}"));
    }
}

impl Rec {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn eff() -> cfg::Effective {
    cfg::Effective { browse_mode: false, forced_colors: false, min_font_px: 0, freeze_animation: false, screen_reader: true, reduce_motion: false, big_ring: false, os_high_contrast: false }
}

/// A browser on `html` at https://a.org/, the fake voice on, the first frame drawn and what was said at load taken away.
fn open(tag: &str, body: &str, e: cfg::Effective) -> (BrowserApp, AppCx, Rec) {
    let cx = cx_for(tag, vec![("https://a.org/", body)]);
    let mut a = BrowserApp::new(&cx);
    let rec = Rec::default();
    a.a11y_pin(e, Box::new(rec.clone()));
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let _ = frame(&mut a, &cx, 1280, 720);
    rec.take();
    (a, cx, rec)
}

fn spoken(a: &mut BrowserApp, cx: &AppCx, rec: &Rec) -> Vec<String> {
    let _ = frame(a, cx, 1280, 720);
    rec.take()
}

fn name_of(a: &BrowserApp, node: usize) -> String {
    a.a11y_tree().unwrap().nodes[node].name.clone()
}

// ------------------------------------------------------------------------------------------------------ speech on load

#[test]
fn a_page_is_announced_when_it_appears_with_its_title_and_counts_and_its_alerts() {
    let cx = cx_for("a11y-load", vec![("https://a.org/", SIGNIN)]);
    let mut a = BrowserApp::new(&cx);
    let rec = Rec::default();
    a.a11y_pin(eff(), Box::new(rec.clone()));
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.iter().any(|s| s.starts_with("N:Sign in. 1 links, 1 headings, 9 form fields")), "{said:?}");
    assert!(said.iter().any(|s| s == "N:Alert. Please fix the two problems below."), "{said:?}");
    // the assistant's facts: a small file, never the address or the text
    let file = cfg::read_page_summary(cx.services.dir(), &cx.session.user).unwrap();
    assert!(file.starts_with("a.org \"Sign in\":") && file.contains("1 fields marked invalid"), "{file}");
    assert!(!file.contains("https://"));
}

#[test]
fn nothing_is_spoken_when_the_screen_reader_is_off() {
    let (mut a, cx, rec) = open("a11y-off", SHOP, cfg::Effective { screen_reader: false, ..eff() });
    key(&mut a, &cx, Key::Tab);
    assert!(spoken(&mut a, &cx, &rec).is_empty());
}

// ------------------------------------------------------------------------------------------------- keyboard-only walks

/// Tab through the whole page, say-checking every stop, and activate each stop with the keys a person would use.
fn walk(tag: &str, body: &str) -> (BrowserApp, AppCx, Rec, Vec<usize>) {
    let (mut a, cx, rec) = open(tag, body, eff());
    let tree = a.a11y_tree().unwrap();
    let order = iyakki_web::a11y_nav::tab_order(&tree);
    assert!(!order.is_empty());
    let mut visited = Vec::new();
    for want in &order {
        key(&mut a, &cx, Key::Tab);
        let said = spoken(&mut a, &cx, &rec);
        let at = a.a11y_here(&tree).expect("Tab lands on a node");
        assert_eq!(at, *want, "Tab visits the page's tab order");
        let n = &tree.nodes[at];
        assert!(n.link.is_some() || n.control.is_some() || matches!(n.role, WRole::Button), "a stop is something that can be pressed: {n:?}");
        // the first words of what is said are the node's own sentence (the live value may follow)
        let first = said.iter().rev().find(|s| s.starts_with("F:")).unwrap_or_else(|| panic!("nothing said at {n:?}: {said:?}"));
        assert!(first.starts_with(&format!("F:{}", tree.nodes[at].name)) || tree.nodes[at].name.is_empty(), "{first} for {n:?}");
        visited.push(at);
    }
    assert_eq!(visited, order);
    // Tab once more wraps to the first
    key(&mut a, &cx, Key::Tab);
    assert_eq!(a.a11y_here(&tree), Some(order[0]));
    (a, cx, rec, order)
}

#[test]
fn the_shop_page_every_link_and_button_is_reached_with_tab_and_pressed_with_enter() {
    let (mut a, cx, rec, order) = walk("a11y-shop", SHOP);
    let tree = a.a11y_tree().unwrap();
    // the skip link is the first stop: Enter moves to the main part and says so
    key(&mut a, &cx, Key::Enter);
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.iter().any(|s| s == "F:Main landmark"), "{said:?}");
    assert_eq!(tree.nodes[a.a11y.cursor.unwrap()].role, WRole::Landmark(iyakki_web::a11y::Landmark::Main));
    // every ordinary link is followed by Enter
    let mut followed = 0;
    for &n in order.iter().filter(|&&n| tree.nodes[n].role == WRole::Link && !tree.nodes[n].href.starts_with('#')) {
        a.a11y_goto(n, None);
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert_eq!(a.current_url().map(|u| u.path.clone()), Some(tree.nodes[n].href.clone()), "Enter followed {:?}", tree.nodes[n]);
        followed += 1;
        key(&mut a, &cx, Key::Left);
        settle(&mut a, &cx);
        let _ = frame(&mut a, &cx, 1280, 720);
    }
    assert_eq!(followed, 5, "Home, Shoes, Bags, Deal of the day, About us");
    // the buttons: Search sends the form (a GET to /s), the other has nothing to send and says so
    let search = order.iter().copied().find(|&n| tree.nodes[n].name == "Search" && tree.nodes[n].role == WRole::Button).unwrap();
    a.a11y_goto(search, None);
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert_eq!(a.current_url().map(|u| u.path.clone()), Some("/s".into()));
    assert!(spoken(&mut a, &cx, &rec).iter().any(|s| s.starts_with("N:Elsewhere.")), "the new page is announced");
}

#[test]
fn the_sign_in_page_every_field_is_reached_filled_and_pressed_by_keyboard_alone() {
    let (mut a, cx, rec, order) = walk("a11y-signin", SIGNIN);
    let tree = a.a11y_tree().unwrap();
    let by = |name: &str| order.iter().copied().find(|&n| tree.nodes[n].name == name).unwrap_or_else(|| panic!("no stop named {name}"));
    // a text field takes the typing, the live value is said when the focus comes back
    a.a11y_goto(by("Email"), None);
    for c in "ann@example.org".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    let ci = tree.nodes[by("Email")].control.unwrap();
    assert_eq!(a.control(ci).unwrap().1.value, "ann@example.org");
    key(&mut a, &cx, Key::Esc);
    // a password field is typed into and is never read back
    a.a11y_goto(by("Password"), None);
    for c in "correct horse".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.iter().all(|s| !s.contains("correct horse")), "{said:?}");
    key(&mut a, &cx, Key::Esc);
    // a check box: Space toggles it and the new state is said
    a.a11y_goto(by("Remember me"), None);
    rec.take();
    key(&mut a, &cx, Key::Char(' '));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Remember me, checkbox, not checked"]);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Remember me, checkbox, checked"]);
    // a drop-down: arrows choose, the choice is said
    a.a11y_goto(by("Country"), None);
    rec.take();
    key(&mut a, &cx, Key::Down);
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Country, combo box, Japan"]);
    key(&mut a, &cx, Key::Enter);
    assert!(a.popup.is_some(), "Enter opens the list");
    key(&mut a, &cx, Key::Esc);
    // the buttons and the link
    a.a11y_goto(by("Clear"), None);
    key(&mut a, &cx, Key::Enter);
    assert!(a.status_text().unwrap().contains("JavaScript"), "{:?}", a.status_text());
    a.a11y_goto(by("Forgot your password?"), None);
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert_eq!(a.current_url().map(|u| u.path.clone()), Some("/forgot".into()));
}

#[test]
fn arrow_keys_move_inside_a_group_of_radio_buttons_and_say_where() {
    let (mut a, cx, rec) = open("a11y-radio", SIGNIN, eff());
    let tree = a.a11y_tree().unwrap();
    let free = tree.nodes.iter().position(|n| n.role == WRole::Radio && n.name == "Free").unwrap();
    a.a11y_goto(free, None);
    rec.take();
    key(&mut a, &cx, Key::Down);
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Pro, radio button, selected, 2 of 3"]);
    let state = |name: &str| {
        let n = tree.nodes.iter().position(|n| n.role == WRole::Radio && n.name == name).unwrap();
        a.control(tree.nodes[n].control.unwrap()).unwrap().1.checked
    };
    assert!(state("Pro") && !state("Free"), "choosing one clears the others");
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Right);
    assert_eq!(a.a11y_here(&tree).map(|n| name_of(&a, n)), Some("Free".into()), "wraps round");
    key(&mut a, &cx, Key::Up);
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:Team, radio button, selected, 3 of 3".into()));
}

#[test]
fn the_article_every_link_is_reached_and_followed_by_keyboard() {
    let (mut a, cx, _rec, order) = walk("a11y-article", ARTICLE);
    let tree = a.a11y_tree().unwrap();
    let links: Vec<usize> = order.iter().copied().filter(|&n| tree.nodes[n].role == WRole::Link).collect();
    assert_eq!(links.len(), 5);
    for &n in links.iter().filter(|&&n| !tree.nodes[n].href.starts_with('#')) {
        a.a11y_goto(n, None);
        let before = a.current_url().cloned();
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        let want = tree.nodes[n].href.clone();
        if want.starts_with('/') {
            assert_eq!(a.current_url().map(|u| u.path.clone()), Some(want.clone()));
            key(&mut a, &cx, Key::Left);
            settle(&mut a, &cx);
            let _ = frame(&mut a, &cx, 1280, 720);
        } else {
            // another site: the browser took the press (it loaded it, or refused it for the account's host policy and said so)
            assert!(a.current_url().cloned() != before || a.failed.is_some() || a.status_text().is_some(), "Enter on {want} did something");
            if a.current_url().cloned() != before {
                key(&mut a, &cx, Key::Left);
                settle(&mut a, &cx);
                let _ = frame(&mut a, &cx, 1280, 720);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ browse mode

#[test]
fn browse_mode_quick_keys_jump_by_kind_in_both_directions_and_say_what_they_found() {
    let (mut a, cx, rec) = open("a11y-quick", ARTICLE, eff());
    // off: H is the home page key, nothing is said
    assert!(!a.a11y_browse());
    key(&mut a, &cx, Key::Char('v'));
    assert!(a.a11y_browse());
    assert!(spoken(&mut a, &cx, &rec).iter().any(|s| s.starts_with("F:Browse mode on")));
    key(&mut a, &cx, Key::Char('h'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 1, How tides work"]);
    key(&mut a, &cx, Key::Char('h'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 2, Two bulges"]);
    key(&mut a, &cx, Key::Char('3'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 3, Why it matters"]);
    key(&mut a, &cx, Key::Char('H'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 2, Spring and neap"]);
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:A reference, link".into()), "next link after the cursor in page order");
    // the focus is the real one, so Enter follows it
    assert!(a.focus_link.is_some());
    key(&mut a, &cx, Key::Char('r'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Wrapped to the start. Sections, navigation landmark"], "no landmark after the last link: wraps and says so");
    key(&mut a, &cx, Key::Char('t'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["N:No tables on this page"]);
    key(&mut a, &cx, Key::Char('2'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 2, Two bulges"]);
    key(&mut a, &cx, Key::Char('2'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Heading level 2, Spring and neap"]);
    // off again: the letters are the browser's own keys
    key(&mut a, &cx, Key::Char('v'));
    assert!(!a.a11y_browse());
}

#[test]
fn quick_keys_find_fields_buttons_tables_and_landmarks_on_the_shop_page() {
    let (mut a, cx, rec) = open("a11y-quick2", SHOP, cfg::Effective { browse_mode: true, ..eff() });
    key(&mut a, &cx, Key::Char('t'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Sizes and prices, Table, 3 rows, 2 columns"]);
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Wrapped to the start. Search the shop, edit text"], "the only field is above the table");
    assert!(a.focus_ctl.is_some(), "the field has the real keyboard focus");
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('b'));
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:Search, button".into()));
    key(&mut a, &cx, Key::Char('B'));
    key(&mut a, &cx, Key::Char('R'));
    assert!(spoken(&mut a, &cx, &rec).last().unwrap().ends_with("landmark"));
    key(&mut a, &cx, Key::Char('?'));
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.last().unwrap().starts_with("N:") && said.last().unwrap().contains("links") && said.last().unwrap().contains("1 images without a description"), "{said:?}");
}

#[test]
fn skip_to_main_uses_the_pages_skip_link_or_its_main_landmark() {
    let (mut a, cx, rec) = open("a11y-skip", SHOP, eff());
    key(&mut a, &cx, Key::Char('m'));
    assert_eq!(spoken(&mut a, &cx, &rec), ["F:Main landmark"]);
    let none = "<p>hello</p><a href=x>x</a>";
    let (mut b, cx2, rec2) = open("a11y-skip2", none, eff());
    key(&mut b, &cx2, Key::Char('m'));
    assert_eq!(spoken(&mut b, &cx2, &rec2), ["N:This page has no main part to skip to"]);
}

// ------------------------------------------------------------------------------------------------------------- the lists

#[test]
fn the_links_and_headings_lists_open_filter_by_typing_and_go_to_the_choice() {
    let (mut a, cx, rec) = open("a11y-rotor", ARTICLE, eff());
    key(&mut a, &cx, Key::Char('K'));
    let said = spoken(&mut a, &cx, &rec);
    assert!(said[0].starts_with("F:Links list, 5 items. Type to search"), "{said:?}");
    assert!(a.a11y.rotor.is_some());
    key(&mut a, &cx, Key::Char('r'));
    key(&mut a, &cx, Key::Char('e'));
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:Read about the moon, 1 of 2".into()));
    key(&mut a, &cx, Key::Down);
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:A reference, 2 of 2".into()));
    key(&mut a, &cx, Key::Enter);
    assert!(a.a11y.rotor.is_none());
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:A reference, link".into()));
    assert!(a.focus_link.is_some(), "the choice has the real focus, Enter would follow it");
    // headings: typing narrows, Escape closes without moving
    key(&mut a, &cx, Key::Char('J'));
    rec.take();
    for c in "spr".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:Spring and neap, 1 of 1".into()));
    key(&mut a, &cx, Key::Char('x'));
    assert_eq!(spoken(&mut a, &cx, &rec).last().cloned(), Some("F:No matches".into()));
    key(&mut a, &cx, Key::Esc);
    assert!(a.a11y.rotor.is_none());
    assert_eq!(spoken(&mut a, &cx, &rec), ["N:List closed"]);
}

#[test]
fn the_list_is_drawn_inside_the_screen_at_every_size_and_its_rows_can_be_clicked() {
    for (w, h) in [(2560u32, 1600u32), (1920, 1080), (1366, 768), (1280, 720), (800, 480)] {
        let (mut a, cx, _rec) = open(&format!("a11y-rotor-{w}"), ARTICLE, eff());
        key(&mut a, &cx, Key::Char('K'));
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
        a.render(&mut g, area, s, &cx, 0);
        let rows: Vec<_> = g.hits.iter().filter(|hit| matches!(hit.ev, Event::Tab(n) if (8000..8300).contains(&n))).collect();
        assert_eq!(rows.len(), 5, "{w}x{h}: a row for every link");
        for r in rows {
            assert!(r.rect.x >= 0.0 && r.rect.y >= area.y && r.rect.x + r.rect.w <= w as f32 && r.rect.y + r.rect.h <= h as f32, "{w}x{h}: {:?}", r.rect);
        }
        // a click on the third row goes to the third link
        a.handle(&Event::Tab(8002), &cx);
        assert!(a.a11y.rotor.is_none() && a.focus_link.is_some());
    }
}

// ------------------------------------------------------------------------------------------------------------- reading

#[test]
fn read_page_says_the_article_top_to_bottom_pauses_resumes_and_stops() {
    let long = "The moon pulls on the sea and the pull is a little stronger on the side of the Earth that faces the moon. ";
    let page = format!("<html><head><title>Tides</title></head><body><nav><a href=\"/x\">Menu item</a></nav><h1>How tides work</h1><p>{long}{long}</p><p>{long}Second paragraph ends here.</p><img src=a.png alt=\"Diagram of bulges\"><p>{long}{long}</p></body></html>");
    let (mut a, cx, rec) = open("a11y-read", &page, eff());
    key(&mut a, &cx, Key::Char('A'));
    a.a11y_flush(0);
    let said = rec.take();
    assert!(said[0].starts_with("F:Reading."), "{said:?}");
    assert_eq!(said[1], "N:Heading level 1, How tides work");
    // the next paragraph waits for the voice: a few ticks later nothing new, then it comes
    a.a11y_flush(5);
    assert!(rec.take().is_empty());
    a.a11y_flush(400);
    let said = rec.take();
    assert_eq!(said.len(), 1);
    assert!(said[0].starts_with("N:The moon pulls on the sea"), "{said:?}");
    // pause: nothing more, however long
    key(&mut a, &cx, Key::Char('A'));
    a.a11y_flush(500);
    assert_eq!(rec.take(), ["F:Paused. A goes on, Escape stops."]);
    a.a11y_flush(9000);
    assert!(rec.take().is_empty());
    key(&mut a, &cx, Key::Char('A'));
    a.a11y_flush(9001);
    let said = rec.take();
    assert_eq!(said[0], "F:Reading on.");
    assert!(said[1].starts_with("N:The moon") && said[1].contains("Second paragraph ends here"), "{said:?}");
    // Escape stops it and does not leave the browser
    let flow = a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(flow, Flow::Stay));
    a.a11y_flush(9002);
    assert_eq!(rec.take(), ["F:Stopped reading"]);
    assert!(a.a11y.reading.is_none());
    // to the end: "End of page"
    key(&mut a, &cx, Key::Char('A'));
    for t in (0..40_000).step_by(300) {
        a.a11y_flush(10_000 + t);
    }
    let said = rec.take();
    assert!(said.iter().any(|s| s == "N:Image, Diagram of bulges") || said.iter().any(|s| s.contains("The moon")), "{said:?}");
    assert_eq!(said.last().unwrap(), "N:End of page");
}

#[test]
fn read_whole_page_goes_through_the_menus_landmarks_images_and_tables_in_order() {
    let (mut a, cx, rec) = open("a11y-readall", SHOP, eff());
    key(&mut a, &cx, Key::Char('W'));
    for t in (0..60_000).step_by(100) {
        a.a11y_flush(t);
    }
    let said = rec.take();
    let pos = |needle: &str| said.iter().position(|s| s.contains(needle)).unwrap_or_else(|| panic!("{needle} not said in {said:?}"));
    assert!(pos("Skip to main content") < pos("Primary, navigation landmark"));
    assert!(pos("Primary, navigation landmark") < pos("Heading level 2, Pricing"));
    assert!(pos("Image, Red running shoe") < pos("Sizes and prices"));
    assert!(pos("Row 3 of 3. Size: 42, Price: $55") < pos("Offers, complementary landmark"));
    assert!(said.iter().all(|s| !s.contains("secret one") && !s.contains("decor two") && !s.contains("spacer")));
}

// -------------------------------------------------------------------------------------------------------------- forms

#[test]
fn a_form_that_breaks_the_pages_own_rules_is_not_sent_and_the_field_is_named() {
    let (mut a, cx, rec) = open("a11y-validate", SIGNIN, eff());
    let tree = a.a11y_tree().unwrap();
    let email = tree.nodes.iter().position(|n| n.name == "Email").unwrap();
    let pw = tree.nodes.iter().position(|n| n.role == WRole::PasswordField).unwrap();
    // the password field alone (the email is empty): Enter does not send, the email field takes the focus
    a.a11y_goto(pw, None);
    for c in "longenough".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    rec.take();
    key(&mut a, &cx, Key::Enter);
    assert!(!a.is_loading(), "nothing was sent");
    assert_eq!(a.a11y_here(&tree), Some(email));
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.iter().any(|s| s.starts_with("F:Email is required. Email, edit text, required, invalid")), "{said:?}");
    assert!(a.status_text().unwrap().contains("The form was not sent"));
    // an address without an at sign
    for c in "ann".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(!a.is_loading());
    assert!(spoken(&mut a, &cx, &rec).iter().any(|s| s.contains("needs an email address with an at sign")));
    // a good one goes
    for c in "@b.org".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert!(a.is_loading(), "the form was sent");
    assert!(a.a11y.submit_pending);
}

#[test]
fn a_page_that_comes_back_with_errors_puts_the_focus_on_the_first_one_and_counts_them() {
    let cx = cx_for("a11y-back", vec![("https://a.org/", SIGNIN)]);
    let mut a = BrowserApp::new(&cx);
    let rec = Rec::default();
    a.a11y_pin(eff(), Box::new(rec.clone()));
    a.a11y.submit_pending = true;
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let said = spoken(&mut a, &cx, &rec);
    assert!(said.iter().any(|s| s == "N:The form has 1 field with errors"), "{said:?}");
    assert!(said.iter().any(|s| s.starts_with("F:Email, edit text, required, invalid, Enter an email address with an at sign.")), "{said:?}");
    assert!(a.focus_ctl.is_some());
    assert!(!a.a11y.submit_pending);
}

// ------------------------------------------------------------------------------------------------------------- the look

fn text_prims(a: &BrowserApp) -> Vec<(String, f32, [u8; 4])> {
    a.layout.as_ref().unwrap().prims.iter().filter_map(|p| if let Kind::Text { text, size, color, .. } = &p.kind { Some((text.clone(), *size, *color)) } else { None }).collect()
}

const PALE: &str = "<html><head><title>Pale</title><style>body{background:#fafafa;color:#bbbbbb} a{color:#9999ff} .t{background:#ddeeff;color:#ccdd99}</style></head><body><h1>Pale page</h1><p class=t>Washed out words</p><p><a href=\"/seen\">visited link</a> and <a href=\"/new\">new link</a></p></body></html>";

#[test]
fn forced_colours_use_the_os_palette_with_aa_contrast_and_tell_visited_links_apart() {
    let pal = os_palette();
    pal.check().expect("the OS High contrast palette passes WCAG AA");
    assert!(pal.lowest_text_ratio() >= 7.0, "{}", pal.lowest_text_ratio());
    // without the option the page keeps its own pale ground (the page's own text colours are lifted by the layout when they are too faint)
    let (plain, _cx, _) = open("a11y-pale", PALE, eff());
    assert_eq!(plain.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]));
    // with it: black ground, every text colour at least 7 to 1, and a visited link has its own colour
    let cx = cx_for("a11y-forced", vec![("https://a.org/", PALE)]);
    let mut a = BrowserApp::new(&cx);
    a.a11y_pin(cfg::Effective { forced_colors: true, ..eff() }, Box::new(Rec::default()));
    a.data.history.push(browser_data::Visit { url: "https://a.org/seen".into(), title: "seen".into(), visits: 1, last: 0 });
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let _ = frame(&mut a, &cx, 1280, 720);
    assert_eq!(a.layout.as_ref().unwrap().canvas, Some(pal.bg));
    for (text, _, color) in text_prims(&a) {
        let r = iyakki_web::style::contrast(color, pal.bg);
        assert!(r >= 7.0, "{text:?} is {r:.1} to 1");
    }
    let colour = |w: &str| text_prims(&a).into_iter().find(|(t, _, _)| t.contains(w)).map(|(_, _, c)| c).unwrap();
    assert_eq!(colour("visited"), pal.visited);
    assert_eq!(colour("new"), pal.link);
    assert_eq!(colour("Washed"), pal.fg);
    // the setting changes with the files: turning it off lays the page out again
    a.a11y_set_effective(eff());
    assert!(a.layout.is_none());
    let _ = frame(&mut a, &cx, 1280, 720);
    assert_eq!(a.layout.as_ref().unwrap().canvas, Some([0xfa, 0xfa, 0xfa, 255]));
}

#[test]
fn the_minimum_font_size_makes_small_text_bigger_and_the_os_text_size_and_zoom_still_apply() {
    let page = "<html><body><p>plain words</p></body></html>";
    let (a0, _cx0, _) = open("a11y-font0", page, eff());
    let size = |a: &BrowserApp| text_prims(a).into_iter().find(|(t, _, _)| t.contains("plain")).map(|(_, s, _)| s).unwrap();
    let base = size(&a0);
    let (a1, _cx1, _) = open("a11y-font1", page, cfg::Effective { min_font_px: 32, ..eff() });
    assert!(size(&a1) > base * 1.8, "{} against {}", size(&a1), base);
    // the whole tile scales with Settings -> Display (the scale argument), so a bigger size means bigger page text
    let (mut a2, cx2, _) = open("a11y-font2", page, eff());
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    a2.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.8, &cx2, 0);
    assert!(size(&a2) > base * 1.5, "the tile's scale reaches the page text");
}

#[test]
fn reduce_motion_and_more_contrast_reach_the_pages_media_queries() {
    let page = "<html><head><style>p{color:#0000ff} @media (prefers-reduced-motion: reduce){p{color:#ff0000}} @media (prefers-contrast: more){p{font-weight:bold}}</style></head><body><p>moving words</p></body></html>";
    let red = |a: &BrowserApp| text_prims(a).into_iter().find(|(t, _, _)| t.contains("moving")).map(|(_, _, c)| c[0] > c[2]).unwrap();
    let (calm, _c1, _) = open("a11y-calm", page, cfg::Effective { freeze_animation: true, ..eff() });
    assert!(red(&calm), "the reduced-motion rules apply");
    let (normal, _c2, _) = open("a11y-normal", page, eff());
    assert!(!red(&normal), "and only then");
}

#[test]
fn the_cursor_ring_sits_on_the_heading_and_the_big_ring_option_thickens_it() {
    let (mut a, cx, _rec) = open("a11y-ring", ARTICLE, eff());
    key(&mut a, &cx, Key::Char('v'));
    key(&mut a, &cx, Key::Char('2'));
    let tree = a.a11y_tree().unwrap();
    let cur = a.a11y.cursor.unwrap();
    assert_eq!(tree.nodes[cur].name, "Two bulges");
    let r = a.a11y_locate(&tree, cur).expect("the heading is found on the drawn page");
    let (_, y_of_heading, _) = {
        let p = a.layout.as_ref().unwrap().prims.iter().find(|p| matches!(&p.kind, Kind::Text { text, .. } if text.contains("Two"))).unwrap();
        ((), p.y, ())
    };
    assert!((r.y - y_of_heading).abs() < 2.0, "{r:?} against {y_of_heading}");
    // a second heading of the same words further down is found as the second one
    let twice = "<h1>Intro</h1><p>words</p><h2>Same</h2><p>filler filler filler</p><h2>Same</h2><p>end</p>";
    let (mut b, cx2, _) = open("a11y-twice", twice, cfg::Effective { browse_mode: true, ..eff() });
    key(&mut b, &cx2, Key::Char('2'));
    key(&mut b, &cx2, Key::Char('2'));
    let t2 = b.a11y_tree().unwrap();
    let first = t2.nodes.iter().position(|n| n.name == "Same").unwrap();
    let second = t2.nodes.iter().rposition(|n| n.name == "Same").unwrap();
    assert_eq!(b.a11y.cursor, Some(second));
    assert!(b.a11y_locate(&t2, second).unwrap().y > b.a11y_locate(&t2, first).unwrap().y + 10.0);
    assert_eq!(a.a11y_ring().1, 1.0);
    a.a11y_set_effective(cfg::Effective { big_ring: true, ..eff() });
    assert_eq!(a.a11y_ring().1, 2.0);
    a.a11y_set_effective(cfg::Effective { forced_colors: true, ..eff() });
    assert_eq!(a.a11y_ring().0, os_palette().focus);
    // drawing never fails at any size
    for (w, h) in [(2560, 1600), (800, 480)] {
        let _ = frame(&mut a, &cx, w, h);
    }
}

#[test]
fn the_help_lists_the_accessibility_keys() {
    let (a, _cx, _) = open("a11y-help", SHOP, eff());
    assert!(a.help().iter().any(|l| l.contains("browse mode") && l.contains("headings list")));
}

#[test]
fn the_reader_view_has_no_tree_so_tab_walks_the_old_way_and_reading_still_works() {
    let long = "A long sentence that belongs to the article and goes on for a while so that it is clearly text and not a menu entry at all. ";
    let page = format!("<html><head><title>Story</title></head><body><h1>Story</h1><p>{long}{long} <a href=\"/a\">one</a></p><p>{long}{long}</p></body></html>");
    let (mut b, cx, rec) = open("a11y-readerview", &page, eff());
    key(&mut b, &cx, Key::Char('R'));
    let _ = frame(&mut b, &cx, 1280, 720);
    assert!(b.a11y_tree().is_none(), "the reader view is made from blocks, not from the page's tree");
    key(&mut b, &cx, Key::Tab);
    assert!(b.focus_link.is_some() || b.focus_ctl.is_none(), "the old walk still lands on the link");
    key(&mut b, &cx, Key::Char('A'));
    b.a11y_flush(0);
    let said = rec.take();
    assert!(said.iter().any(|s| s.starts_with("N:Heading level 1, Story")), "{said:?}");
}
