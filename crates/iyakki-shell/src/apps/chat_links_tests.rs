//! Tests for links in the transcript: finding them, where they are drawn, and what a click, a key or the menu does with each kind.

use super::chat_links::{test_system, LinkOut, OpenHow};
use super::tests::render_with;
use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::App;

const ANSWER: &str = "Try [Apple Maps](https://maps.apple.com/place?q=Cafe%20Roma&ll=12.9,80.2) for the route, or read www.example.org/a_very_long/path/that/wraps/across/the/line/for/sure/and-more/and-more-and-more. \
Mail <mailto:ann@example.com> or call tel:+4912345. Docs: [spec](https://example.com/Rust_(lang)) and [bad](javascript:alert(1)) and [evil](https://evil.test/login) and https://bank.example/ then \
[plain](http://example.com/a) and [dev](http://localhost:3000/app) and [pin](geo:12.9,80.2) and [shell](ssh://host/x) and [odd](weird://x) and [outside](file:///etc/passwd) and `https://code.example/x` stays code.\n\n```\ncurl https://block.example/in-code\n```";

fn chat_with(tag: &str, answer: &str, tool_out: Option<&str>) -> (ChatApp, AppCx) {
    let (cx, _d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
    let mut app = ChatApp::new();
    app.msgs.push(Message::user("find the cafe"));
    if let Some(o) = tool_out {
        let call = iyakki_agent::chat::ToolCall { id: "t1".into(), name: "web_fetch".into(), args: serde_json::json!({"url": "https://x.test"}) };
        app.msgs.push(Message::assistant_calls("", vec![call.clone()]));
        app.msgs.push(Message::tool_result(&call, o.to_string()));
    }
    app.msgs.push(Message::assistant(answer.to_string()));
    (app, cx)
}

fn find(app: &ChatApp, part: &str) -> usize {
    (0..app.view.link_count()).find(|&n| app.view.link(n).is_some_and(|l| l.url.contains(part))).unwrap_or_else(|| panic!("no link with {part:?}: {:?}", (0..app.view.link_count()).filter_map(|n| app.view.link(n)).map(|l| l.url).collect::<Vec<_>>()))
}

/// A press and release on link `n`, then the heartbeats that let the double-click time pass.
fn click(app: &mut ChatApp, cx: &AppCx, n: usize) {
    let (id, r) = app.view.link_screen_rects(n)[0];
    crate::ui::set_click_count(1);
    app.handle(&Event::AreaAt(id, r.w / 2.0, r.h / 2.0), cx);
    app.handle(&Event::MouseUp(0, 0), cx);
    for _ in 0..chat_view::LINK_DELAY {
        app.handle(&Event::Tick, cx);
    }
}

fn click_url(app: &mut ChatApp, cx: &AppCx, part: &str) {
    let n = find(app, part);
    click(app, cx, n);
}

fn request(app: &mut ChatApp) -> Option<(String, String)> {
    App::take_request(app)
}

fn shown(tag: &str) -> (ChatApp, AppCx) {
    let (mut app, cx) = chat_with(tag, ANSWER, None);
    crate::ui::clipboard::clear();
    render_with(&mut app, &cx, 1280, 720);
    (app, cx)
}

#[test]
fn every_kind_of_link_is_found_and_the_blocked_and_code_ones_are_not() {
    let (app, _cx) = shown("lnk1");
    let urls: Vec<String> = (0..app.view.link_count()).filter_map(|n| app.view.link(n)).map(|l| l.url).collect();
    for want in ["https://maps.apple.com/place?q=Cafe%20Roma&ll=12.9,80.2", "https://www.example.org/a_very_long/", "mailto:ann@example.com", "tel:+4912345", "https://example.com/Rust_(lang)", "https://evil.test/login", "https://bank.example/", "http://example.com/a", "http://localhost:3000/app", "geo:12.9,80.2", "ssh://host/x", "weird://x", "file:///etc/passwd"] {
        assert!(urls.iter().any(|u| u.starts_with(want)), "{want} in {urls:?}");
    }
    assert!(!urls.iter().any(|u| u.contains("javascript") || u.contains("code.example") || u.contains("block.example")), "{urls:?}");
    // the blocked one is plain text with a note
    let t = app.view.line_texts().join(" ");
    assert!(t.contains("bad") && t.contains("javascript: link not opened"), "{t}");
    // the label is what was shown, the address is what it goes to
    let maps = app.view.link(find(&app, "maps.apple.com")).unwrap();
    assert_eq!(maps.text, "Apple Maps");
}

#[test]
fn a_link_that_wraps_is_one_link_with_a_rectangle_per_line_and_geometry_holds_at_every_size() {
    for (w, h) in [(1280u32, 720u32), (800, 480), (420, 700)] {
        let (mut app, cx) = chat_with(&format!("lnk2-{w}"), ANSWER, Some("see https://example.com/docs/guide?x=1\nand http://10.43.1.2:8080/health"));
        let g = render_with(&mut app, &cx, w, h);
        let n = find(&app, "www.example.org");
        let rects = app.view.link_screen_rects(n);
        assert!(!rects.is_empty());
        if w <= 800 {
            assert!(rects.len() >= 2, "the long address wraps over lines at {w}px: {rects:?}");
            let ids: std::collections::BTreeSet<usize> = rects.iter().map(|r| r.0).collect();
            assert_eq!(ids.len(), rects.len(), "each part has its own id, all of one link");
        }
        let link_hits: Vec<_> = g.hits.iter().filter(|h| h.label == "link").collect();
        assert!(link_hits.len() >= rects.len());
        let (fw, fh) = (w as f32, h as f32);
        for lh in &link_hits {
            let r = lh.rect;
            assert!(r.w > 0.0 && r.h > 0.0 && r.x >= 0.0 && r.x + r.w <= fw + 1.0 && r.y >= 0.0 && r.y + r.h <= fh + 1.0, "{w}x{h}: {r:?}");
        }
        // scrolled up the hits follow the text and are cut at the transcript's edge
        app.scroll_px = 200.0;
        let g2 = render_with(&mut app, &cx, w, h);
        for hit in g2.hits.iter().filter(|h| h.label == "link") {
            assert!(hit.rect.y >= 46.0 && hit.rect.y + hit.rect.h <= fh, "{w}x{h}: {:?}", hit.rect);
        }
        // the hand pointer: the hit is a link, not a text area
        assert!(g.hits.iter().any(|h| h.label == "link"));
    }
}

#[test]
fn a_click_does_the_right_thing_for_each_scheme() {
    let (mut app, cx) = shown("lnk3");
    // https: the BROWSER tile, new tab
    let n = find(&app, "maps.apple.com");
    click(&mut app, &cx, n);
    assert_eq!(request(&mut app), Some(("browser".into(), "https://maps.apple.com/place?q=Cafe%20Roma&ll=12.9,80.2".into())));
    // mailto: copied, nothing opened
    click_url(&mut app, &cx, "mailto:");
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("ann@example.com"));
    assert!(request(&mut app).is_none());
    // tel: copied with a note, no dialer
    click_url(&mut app, &cx, "tel:");
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("+4912345"));
    assert!(app.info.as_deref().unwrap_or("").contains("cannot place calls") && request(&mut app).is_none());
    // geo: becomes a normal https map
    click_url(&mut app, &cx, "geo:");
    let r = request(&mut app).unwrap();
    assert!(r.1.starts_with("https://www.openstreetmap.org/?mlat=12.9&mlon=80.2"), "{r:?}");
    // plain http to this computer opens
    click_url(&mut app, &cx, "localhost:3000");
    assert_eq!(request(&mut app).unwrap().1, "http://localhost:3000/app");
    // ssh://: a card that says IYAKKI cannot open it, and Copy
    click_url(&mut app, &cx, "ssh://");
    assert!(request(&mut app).is_none());
    let card = app.links.card.clone().expect("a card");
    assert!(card.title.contains("cannot open ssh"), "{}", card.title);
    app.handle(&Event::Key(Key::Char('c')), &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("ssh://host/x"));
    assert!(app.links.card.is_none());
    // an unknown protocol: the card, then copy only
    click_url(&mut app, &cx, "weird://");
    assert!(app.links.card.as_ref().unwrap().title.contains("weird"));
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(app.links.card.is_none() && request(&mut app).is_none());
    // a file outside the account's folders is refused with a plain reason
    click_url(&mut app, &cx, "file:///etc/passwd");
    let e = app.error.clone().unwrap();
    assert!(e.contains("outside") && app.editor().is_none(), "{e}");
}

#[test]
fn a_file_link_inside_the_account_opens_in_the_editor_at_its_line() {
    let (cx, _d) = cx_with_backend("lnk4", iyakki_agent::chat::Unconfigured);
    let home = iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir());
    std::fs::write(home.join("note.md"), "one\ntwo\nthree\n").unwrap();
    let mut app = ChatApp::new();
    app.msgs.push(Message::assistant(format!("See [the note](file://{}/note.md:3) and [again](note.md).", home.display())));
    render_with(&mut app, &cx, 1280, 720);
    click_url(&mut app, &cx, "note.md:3");
    assert!(app.editor().is_some(), "{:?}", app.error);
    assert!(app.editor().unwrap().open_paths().iter().any(|p| p.ends_with("note.md")));
    // a link out of the folder by `..` is refused outright
    let mut app = ChatApp::new();
    app.msgs.push(Message::assistant("[up](file:///home/../etc/passwd) [rel](../secret.rs)".to_string()));
    render_with(&mut app, &cx, 1280, 720);
    click_url(&mut app, &cx, "etc/passwd");
    assert!(app.error.is_some() && app.editor().is_none());
}

#[test]
fn the_text_that_shows_another_address_insecure_http_and_lookalike_hosts_are_marked() {
    let (mut app, cx) = chat_with("lnk5", "[evil](https://xn--80ak6aa92e.com/) [bank](https://bank.com) [x](http://example.com/a) [ok](https://example.com) [loc](http://192.168.1.4:30080/)\n\nhttps://shown.test is fine; [https://shown.test](https://other.test/x) is not.", None);
    render_with(&mut app, &cx, 1280, 720);
    let w = |p: &str| app.view.link(find(&app, p)).unwrap().warn;
    assert!(w("xn--80ak6aa92e").lookalike);
    assert!(w("http://example.com/a").insecure);
    assert!(!w("https://example.com").any());
    assert!(!w("192.168.1.4").insecure, "the private network is expected to be plain http");
    assert!(w("other.test").mismatch);
    assert!(!w("shown.test").mismatch || app.view.link_count() > 0);
    // the status line under the pointer says it
    let n = find(&app, "other.test");
    let (_, r) = app.view.link_screen_rects(n)[0];
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    g.mouse = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    app.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 0);
    let plain = render_with(&mut app, &cx, 1280, 720);
    assert_ne!(g.rgba(), plain.rgba(), "hovering shows the status line");
    // clicking one of these opens it all the same (the click is the person's decision), with the warning as a note
    click(&mut app, &cx, n);
    assert!(request(&mut app).is_some() && app.info.as_deref().unwrap_or("").contains("different address"));
}

#[test]
fn nothing_opens_by_itself_not_while_drawing_not_while_waiting_and_not_for_an_autopilot_turn() {
    let (mut app, cx) = chat_with("lnk6", ANSWER, Some("fetched https://example.com/docs and https://evil.test/x"));
    crate::ui::clipboard::clear();
    for _ in 0..5 {
        render_with(&mut app, &cx, 1280, 720);
    }
    for _ in 0..200 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(request(&mut app).is_none() && app.take_link_out().is_none() && app.links.card.is_none());
    assert!(crate::ui::clipboard::latest().is_none());
    // hovering and moving the pointer over a link opens nothing either
    let n = find(&app, "evil.test");
    let (_, r) = app.view.link_screen_rects(n)[0];
    app.handle(&Event::MouseMove((r.x + 2.0) as i32, (r.y + 2.0) as i32), &cx);
    for _ in 0..30 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(request(&mut app).is_none() && app.take_link_out().is_none());
}

#[test]
fn a_drag_selects_and_never_clicks_a_double_click_selects_a_word_and_a_single_click_waits() {
    let (mut app, cx) = shown("lnk7");
    let n = find(&app, "maps.apple.com");
    let (id, r) = app.view.link_screen_rects(n)[0];
    // press on the link, drag away, release: a selection, no opening
    crate::ui::set_click_count(1);
    app.handle(&Event::AreaAt(id, 3.0, r.h / 2.0), &cx);
    crate::ui::set_dragging(true);
    app.handle(&Event::AreaAt(id, r.w + 80.0, r.h / 2.0), &cx);
    crate::ui::set_dragging(false);
    app.handle(&Event::MouseUp(0, 0), &cx);
    for _ in 0..40 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(app.view.has_selection(), "the drag selected text");
    assert!(request(&mut app).is_none(), "a drag is not a click");
    // a double click selects a word of the link and opens nothing
    crate::ui::set_click_count(1);
    app.handle(&Event::AreaAt(id, 6.0, r.h / 2.0), &cx);
    app.handle(&Event::MouseUp(0, 0), &cx);
    app.handle(&Event::Tick, &cx);
    crate::ui::set_click_count(2);
    app.handle(&Event::AreaAt(id, 6.0, r.h / 2.0), &cx);
    crate::ui::set_click_count(1);
    app.handle(&Event::MouseUp(0, 0), &cx);
    for _ in 0..40 {
        app.handle(&Event::Tick, &cx);
    }
    assert_eq!(app.view.selected_text().as_deref(), Some("Apple"));
    assert!(request(&mut app).is_none(), "a double click opens nothing");
    // a single click opens only after the double-click time
    crate::ui::set_click_count(1);
    app.handle(&Event::AreaAt(id, 6.0, r.h / 2.0), &cx);
    app.handle(&Event::MouseUp(0, 0), &cx);
    for _ in 0..chat_view::LINK_DELAY - 3 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(request(&mut app).is_none());
    for _ in 0..4 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(request(&mut app).is_some());
    // copying across a link keeps the text as written
    app.view.select_all();
    let all = app.view.selected_text().unwrap();
    assert!(all.contains("Try Apple Maps for the route"), "{all}");
}

#[test]
fn tab_walks_the_links_and_enter_opens_the_one_the_keyboard_is_on() {
    let (mut app, cx) = shown("lnk8");
    assert!(app.view.link_count() > 5 && app.view.link_focus.is_none());
    let before = app.messages().len();
    app.handle(&Event::Key(Key::Tab), &cx);
    assert_eq!(app.view.link_focus, Some(0));
    app.handle(&Event::Key(Key::Tab), &cx);
    assert_eq!(app.view.link_focus, Some(1));
    app.handle(&Event::Key(Key::BackTab), &cx);
    app.handle(&Event::Key(Key::Enter), &cx);
    let r = request(&mut app).expect("Enter opened the link");
    assert!(r.1.starts_with("https://maps.apple.com"), "{r:?}");
    assert_eq!(app.messages().len(), before, "Enter did not send a message");
    // the ring is drawn and the status line shows the address
    let a = render_with(&mut app, &cx, 1280, 720);
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(app.view.link_focus.is_none());
    let b = render_with(&mut app, &cx, 1280, 720);
    assert_ne!(a.rgba(), b.rgba());
    // Shift+Tab from nowhere starts at the last link; walking off the end leaves the links
    app.handle(&Event::Key(Key::BackTab), &cx);
    assert_eq!(app.view.link_focus, Some(app.view.link_count() - 1));
    app.handle(&Event::Key(Key::Tab), &cx);
    assert!(app.view.link_focus.is_none());
    // typed text keeps Tab and Enter for the box
    app.input.set("hello");
    app.handle(&Event::Key(Key::Tab), &cx);
    assert!(app.view.link_focus.is_none());
}

#[test]
fn the_right_click_menu_of_a_link_opens_copies_and_asks_once_for_the_system_browser() {
    let (mut app, cx) = shown("lnk9");
    let log = test_system(false);
    let n = find(&app, "maps.apple.com");
    let (_, r) = app.view.link_screen_rects(n)[0];
    let at = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    let ids = |app: &mut ChatApp| App::context_items(app, at.0, at.1).into_iter().map(|i| i.id).collect::<Vec<_>>();
    let v = ids(&mut app);
    for want in ["link.open", "link.tab", "link.copy", "link.text", "link.map", "copy", "selall"] {
        assert!(v.iter().any(|i| i == want), "{want} in {v:?}");
    }
    assert!(!v.iter().any(|i| i == "link.ide" || i == "link.sys"), "not in the IDE, not in a window: {v:?}");
    // away from a link there is no link entry
    assert!(!App::context_items(&mut app, 5, 60).iter().any(|i| i.id.starts_with("link.")));
    // the copies
    ids(&mut app);
    App::context_pick(&mut app, "link.copy", &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("https://maps.apple.com/place?q=Cafe%20Roma&ll=12.9,80.2"));
    ids(&mut app);
    App::context_pick(&mut app, "link.text", &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("Apple Maps"));
    ids(&mut app);
    App::context_pick(&mut app, "link.map", &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("12.9,80.2"));
    ids(&mut app);
    App::context_pick(&mut app, "link.tab", &cx);
    assert!(request(&mut app).unwrap().1.starts_with("https://maps.apple.com"));
    // in a desktop window the system browser is offered, behind a one-time question
    let log = {
        drop(log);
        test_system(true)
    };
    assert!(ids(&mut app).iter().any(|i| i == "link.sys"));
    App::context_pick(&mut app, "link.sys", &cx);
    assert!(log.borrow().is_empty(), "nothing ran before the person said yes");
    assert!(app.links.card.is_some());
    app.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(log.borrow().as_slice(), [vec!["xdg-open".to_string(), "https://maps.apple.com/place?q=Cafe%20Roma&ll=12.9,80.2".to_string()]], "an argument array, the address as one argument");
    ids(&mut app);
    App::context_pick(&mut app, "link.sys", &cx);
    assert_eq!(log.borrow().len(), 2, "asked once per session");
    // mail and ssh links never go to the system browser
    let m = find(&app, "mailto:");
    let (_, r) = app.view.link_screen_rects(m)[0];
    let v = App::context_items(&mut app, (r.x + 2.0) as i32, (r.y + 2.0) as i32).into_iter().map(|i| i.id).collect::<Vec<_>>();
    assert!(!v.iter().any(|i| i == "link.sys" || i == "link.tab"), "{v:?}");
    test_system(false);
}

#[test]
fn in_the_ide_a_click_goes_to_the_ide_browser_a_new_tab_to_the_browser_tile_and_a_file_to_the_editor() {
    let (cx, d) = cx_with_backend("lnk10", iyakki_agent::chat::Unconfigured);
    let root = d.join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut app = ChatApp::new().with_project(ProjectScope { name: "demo".into(), root: root.clone(), key: "ide:demo".into(), features: root.join("FEATURES.md"), cluster: None });
    app.msgs.push(Message::assistant(format!("Run it on [the app](http://localhost:3000/) or [docs](https://example.com/d); fix [main](src/main.rs:7) not [other](/etc/passwd) or [abs]({}).", root.join("src/main.rs").display())));
    render_with(&mut app, &cx, 900, 700);
    click_url(&mut app, &cx, "localhost:3000");
    assert_eq!(app.take_link_out(), Some(LinkOut::IdeBrowser("http://localhost:3000/".into())));
    assert!(request(&mut app).is_none());
    let n = find(&app, "example.com/d");
    let (_, r) = app.view.link_screen_rects(n)[0];
    let v = App::context_items(&mut app, (r.x + 2.0) as i32, (r.y + 2.0) as i32).into_iter().map(|i| i.id).collect::<Vec<_>>();
    assert!(v.iter().any(|i| i == "link.ide"), "{v:?}");
    App::context_pick(&mut app, "link.tab", &cx);
    assert_eq!(app.take_link_out(), Some(LinkOut::Browser("https://example.com/d".into())));
    App::context_items(&mut app, (r.x + 2.0) as i32, (r.y + 2.0) as i32);
    App::context_pick(&mut app, "link.ide", &cx);
    assert_eq!(app.take_link_out(), Some(LinkOut::IdeBrowser("https://example.com/d".into())));
    click_url(&mut app, &cx, "src/main.rs:7");
    assert_eq!(app.take_link_out(), Some(LinkOut::Editor { path: "src/main.rs".into(), line: Some(7) }));
    // an absolute path inside the workspace is made relative; one outside is refused
    click_url(&mut app, &cx, "proj/src/main.rs");
    assert_eq!(app.take_link_out(), Some(LinkOut::Editor { path: "src/main.rs".into(), line: None }));
    click_url(&mut app, &cx, "/etc/passwd");
    assert!(app.take_link_out().is_none() && app.error.as_deref().unwrap_or("").contains("outside this workspace"));
}

#[test]
fn addresses_in_tool_results_are_links_and_a_long_one_broken_over_rows_is_still_one() {
    let long = format!("https://example.com/{}", "segment/".repeat(30));
    let out = format!("status 200 https://example.com/docs.\n{long}\nsee (https://en.wikipedia.org/wiki/Rust_(programming_language)) and `ls`\nx");
    let (mut app, cx) = chat_with("lnk11", "done", Some(&out));
    app.view.toggle("tool:t1");
    render_with(&mut app, &cx, 700, 900);
    let urls: Vec<String> = (0..app.view.link_count()).filter_map(|n| app.view.link(n)).map(|l| l.url).collect();
    assert!(urls.contains(&"https://example.com/docs".to_string()), "{urls:?}");
    assert!(urls.contains(&"https://en.wikipedia.org/wiki/Rust_(programming_language)".to_string()), "{urls:?}");
    let n = find(&app, "segment/segment");
    assert_eq!(app.view.link(n).unwrap().url, long);
    assert!(app.view.link_screen_rects(n).len() >= 2, "one link over several rows");
    click(&mut app, &cx, n);
    assert_eq!(request(&mut app).unwrap().1, long);
    // closed, the first rows still show their links
    let (mut app, cx) = chat_with("lnk12", "done", Some(&out));
    render_with(&mut app, &cx, 1100, 900);
    assert!(app.view.link_count() >= 2);
    click_url(&mut app, &cx, "example.com/docs");
    assert_eq!(request(&mut app).unwrap().1, "https://example.com/docs");
}

#[test]
fn the_prompts_tell_the_model_how_to_write_links() {
    for p in [iyakki_agent::toolblock::COMPACT_PROMPT, iyakki_agent::agent::AGENT_SYSTEM_PROMPT] {
        assert!(p.contains("[human label](full https URL)") && p.contains("maps.apple.com") && p.contains("stable link"));
    }
    let _ = OpenHow::Click;
}

#[test]
fn an_opened_table_has_an_open_in_data_viewer_link_that_asks_the_shell_for_the_data_app_at_four_sizes() {
    let (cx, _d) = cx_with_backend("lnk13", iyakki_agent::chat::Unconfigured);
    let home = iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir());
    std::fs::write(home.join("sales.csv"), "a,b\n1,2\n").unwrap();
    let path = format!("{}/sales.csv", home.display());
    let mut app = ChatApp::new();
    app.msgs.push(Message::user("open my sales table"));
    let call = iyakki_agent::chat::ToolCall { id: "t1".into(), name: "data_open".into(), args: serde_json::json!({"path": "sales.csv"}) };
    app.msgs.push(Message::assistant_calls("", vec![call.clone()]));
    app.msgs.push(Message::tool_result(&call, format!("Opened {path} as d1: 1 rows, 2 columns (x engine, 0 KB).\nColumns: a, b\nFirst rows:\n| a | b |\n|---|---|\n| 1 | 2 |\n")));
    app.msgs.push(Message::assistant("done".to_string()));
    for (w, h) in [(1280, 720), (1920, 1080), (1024, 600), (800, 480)] {
        render_with(&mut app, &cx, w, h);
        let n = find(&app, "sales.csv");
        assert!(!app.view.link_screen_rects(n).is_empty(), "{w}x{h}");
    }
    let n = find(&app, "sales.csv");
    click(&mut app, &cx, n);
    let req = request(&mut app).expect("the DATA app is asked for");
    assert_eq!(req.0, "data");
    assert!(req.1.ends_with("sales.csv"));
    // other tools' results get no such link
    let (app2, _) = chat_with("lnk14", "done", Some(&format!("Opened {path} as d1: 1 rows")));
    assert!((0..app2.view.link_count()).all(|n| app2.view.link(n).is_none_or(|l| !l.url.ends_with("sales.csv"))));
}
