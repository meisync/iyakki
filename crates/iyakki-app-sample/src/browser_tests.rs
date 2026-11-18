//! The browser pane of SAMPLE (F-SDK-61): two embedded `BrowserView`s over the SDK's in-memory port. The core-backed port and the real
//! engine are tested where they live (`iyakki-apps` `sdk_browser`, `iyakki-core-browser`).

use iyakki_ui_sdk::browser::{BrowserPort, MemBrowser};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

use super::*;

fn site() -> impl FnMut() -> Box<dyn BrowserPort> {
    || {
        Box::new(
            MemBrowser::default()
                .with_page("https://a.org/", "Alpha", &["alpha page text", "second alpha line"], &[("To beta", "https://b.org/")])
                .with_page("https://b.org/", "Beta", &["beta page text"], &[]),
        )
    }
}

fn host(w: u32, h: u32) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_browser(site()), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn tap(h: &mut Host<SampleApp>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn type_text(h: &mut Host<SampleApp>, s: &str) {
    for c in s.chars() {
        h.event(&UiEvent::Key(UiKey::Char(c)));
    }
}

fn open_pane(h: &mut Host<SampleApp>) {
    h.record();
    tap(h, "Browser");
    assert_eq!(h.app.pane, Pane::Browser);
}

#[test]
fn an_app_without_a_browser_has_no_browser_tab() {
    let mut h = Host::new(SampleApp::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    let l = h.record();
    assert!(!l.a11y().iter().any(|n| n.name == "Browser"));
    assert!(!h.app.has_browser());
}

#[test]
fn typing_an_address_opens_the_page_inside_the_pane_and_its_link_is_a_named_control() {
    let mut h = host(800, 480);
    open_pane(&mut h);
    tap(&mut h, "Address or search");
    type_text(&mut h, "a.org");
    h.event(&UiEvent::Key(UiKey::Enter));
    let l = h.record();
    assert!(l.texts().contains(&"alpha page text"), "{:?}", l.texts());
    assert!(l.a11y().iter().any(|n| n.name == "To beta" && n.role == Role::Link), "the page's link is a named link of the pane");
    let (x, y) = centre(&l, "To beta");
    let pane = (0.0..800.0, 0.0..480.0);
    assert!(pane.0.contains(&x) && pane.1.contains(&y));
    // the address follows the page and the tab is named by its title
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "View 1: Alpha"), "{:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>());
    // tap the link: beta, then Back returns to alpha
    tap(&mut h, "To beta");
    assert!(h.record().texts().contains(&"beta page text"));
    assert_eq!(h.app.tabs[0].addr, "https://b.org/");
    tap(&mut h, "Back");
    assert!(h.record().texts().contains(&"alpha page text"));
    tap(&mut h, "Fwd");
    assert!(h.record().texts().contains(&"beta page text"));
}

#[test]
fn the_two_views_keep_their_own_pages_and_the_wheel_and_find_go_to_the_current_one() {
    let mut h = host(1280, 720);
    open_pane(&mut h);
    h.app.browse("a.org");
    h.record();
    tap(&mut h, "View 2");
    h.app.browse("b.org");
    let l = h.record();
    assert!(l.texts().contains(&"beta page text") && !l.texts().contains(&"alpha page text"));
    tap(&mut h, "View 1: Alpha");
    assert!(h.record().texts().contains(&"alpha page text"), "view 1 kept its page");
    // find in page: the count is said under the page
    tap(&mut h, "Find in page");
    type_text(&mut h, "alpha");
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.starts_with("1 of 2 found")), "{:?}", l.texts());
    assert_eq!(h.app.current_view().unwrap().find_state().matches, 2);
}

#[test]
fn a_refused_address_is_said_under_the_page_and_escape_goes_back_to_the_launcher() {
    let mut h = host(800, 480);
    open_pane(&mut h);
    tap(&mut h, "Address or search");
    type_text(&mut h, "javascript:alert(1)");
    h.event(&UiEvent::Key(UiKey::Enter));
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.contains("only http and https")), "{:?}", l.texts());
    // Escape leaves the page focus, then the address field, then the app
    h.event(&UiEvent::Key(UiKey::Esc));
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn the_pane_draws_inside_the_pane_with_named_controls_at_every_size_and_text_size() {
    for (w, hh, touch) in [(360u32, 740u32, true), (800, 480, false), (1280, 720, false)] {
        for text_scale in [1.0f32, 3.0] {
            let mut h = host(w, hh);
            h.touch = touch;
            h.text_scale = text_scale;
            open_pane(&mut h);
            h.app.browse("a.org");
            let l = h.record();
            for op in &l.ops {
                if let Op::Hit(r, hit) = op {
                    assert!(!hit.name.is_empty(), "an unnamed control");
                    assert!(r.x >= -0.01 && r.right() <= w as f32 + 0.01, "{} sticks out sideways at {w}x{hh} x{text_scale}: {r:?}", hit.name);
                    if touch && hit.role == Role::Button {
                        assert!(r.h >= 44.0, "{} is too small to touch ({r:?})", hit.name);
                    }
                }
            }
        }
    }
}
