//! The embedded browser in the client (F-SDK-64): the frame a core's browser session gives is the SDK display list, and the client engine's
//! renderer draws that same text as it is, with the glyphs the page supplies. Nothing is converted between the core and the client.

use iyakki_client_engine::uidraw::UiSurface;
use iyakki_core_api::browsersession::{EnginePref, Navigate, OpenSpec, PageDefaults, Palette};
use iyakki_core_api::BrowserSessions;
use iyakki_core_fake::{FakeBrowser, FakePage};

fn spec() -> OpenSpec {
    OpenSpec { width: 240.0, height: 160.0, text_scale: 1.0, engine: EnginePref::InHouse, palette: Palette::default(), defaults: PageDefaults::default() }
}

#[test]
fn the_frame_of_a_core_browser_session_is_drawn_by_the_client_engine_as_it_is_and_its_text_and_link_come_through() {
    let core = FakeBrowser::new();
    core.add_page("https://example.org/", FakePage::new("Example page", &["Hello from the core"]).link("Next page", "https://example.org/next"));
    let id = core.open("amy", "client", &spec()).unwrap();
    core.navigate("amy", "client", id, &Navigate::Url("https://example.org/".into())).unwrap();
    let frame = core.frame("amy", "client", id, 0).unwrap().expect("a frame for a page that is open");
    let bg = Palette::default().bg;

    let mut surface = UiSurface::default();
    surface.load(&frame.list, 1.0).expect("the client engine reads the core's frame without a conversion");
    let (px, w, h, version) = surface.framebuffer();
    assert_eq!((w, h, version), (240, 160, 1));
    assert_eq!(&px[..4], &[bg[0], bg[1], bg[2], bg[3]], "the page background is the palette the view asked for");
    // the page's text is handed to the page (this module has no fonts) with its own place and colour; the link is a hit region with its name
    let said: Vec<&str> = surface.texts().iter().map(|t| t.text.as_str()).collect();
    assert!(said.contains(&"Example page") && said.contains(&"Hello from the core") && said.contains(&"Next page"), "{said:?}");
    let link = surface.texts().iter().find(|t| t.text == "Next page").unwrap();
    assert_eq!(surface.action_at(link.x + 2.0, link.y + 2.0), Some(1), "a tap on the link is the action the core's `activate` takes");
    // the same frame again is the same picture; a newer generation draws again
    let again = core.frame("amy", "client", id, frame.generation).unwrap();
    assert!(again.is_none(), "nothing new until the page changes");
}
