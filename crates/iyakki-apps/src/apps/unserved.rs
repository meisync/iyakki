//! An app whose core object this program is not given (F-SPLIT-03, D-SPLIT-03-QUIET): the UI over the core's socket cannot build the Flows panel, the
//! Tales store, the app store or the radio list from remote objects yet, because the core hands them out as live objects of its own, not as data. The
//! launcher still shows the tile; opening it shows this screen, which says what is missing and leaves with Esc, instead of the program stopping.

use super::{App, AppCx, Flow};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, Key, R, TEXT};
use iyakki_sec::policy::App as PApp;

pub struct UnservedApp {
    title: &'static str,
    icon: Icon,
    accent: [u8; 4],
    what: &'static str,
}

impl UnservedApp {
    pub fn new(title: &'static str, icon: Icon, accent: [u8; 4], what: &'static str) -> UnservedApp {
        UnservedApp { title, icon, accent, what }
    }
}

/// What the core does not serve to this program for `app`, as a sentence, or `None` when the app can be built from what the core serves. A core
/// that serves everything in the process (the agent linked in) answers `None` for every app.
pub fn missing(app: PApp, cx: &AppCx) -> Option<&'static str> {
    let user = cx.session.user.as_str();
    match app {
        PApp::Flows if cx.services.flows_jobs(user, &cx.session.policy).is_none() => Some("The flows engine is the core's own object and is not served over the UI socket yet."),
        PApp::Tales if cx.b5().story_store(&cx.session.policy, user, cx.data_dir()).is_none() => Some("The story store is the core's own object and is not served over the UI socket yet."),
        PApp::Lab if cx.b5().lab_backends().is_none() => Some("The picture and vision models are the core's own objects and are not served over the UI socket yet."),
        PApp::Store if cx.b4().store().is_none() => Some("The app store is the core's own object and is not served over the UI socket yet."),
        PApp::Radio if cx.b5().radio_stations(user).is_none() => Some("The radio stations are the core's own object and are not served over the UI socket yet."),
        PApp::Media if cx.b5().media_queue(user).is_none() => Some("The download queue is the core's own object and is not served over the UI socket yet."),
        _ => None,
    }
}

impl App for UnservedApp {
    fn title(&self) -> &'static str {
        self.title
    }
    fn icon(&self) -> Icon {
        self.icon
    }
    fn accent(&self) -> [u8; 4] {
        self.accent
    }
    fn is_unserved(&self) -> bool {
        true
    }
    fn hint(&self) -> String {
        "Esc back".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) => Flow::Back,
            _ => Flow::Stay,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        crate::fx::empty_state(g, area, s, self.icon, &format!("{} is not available in this UI program yet", self.title), self.what, self.accent);
        g.text(area.x + 24.0 * s, area.y + area.h - 28.0 * s, 13.0 * s, TEXT, "The same app works when the shell runs inside the core (iyakki-ui --in-process).", Face::Sans);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    #[test]
    fn the_screen_says_what_is_missing_draws_and_leaves_with_esc() {
        let cx = cx_with_admin_store("unserved1");
        let mut app = UnservedApp::new("FLOWS", Icon::Flows, [200, 120, 40, 255], "The flows engine is not served.");
        assert_eq!(app.title(), "FLOWS");
        let mut g = Gfx::new(800, 480);
        app.render(&mut g, R::new(0.0, 0.0, 800.0, 480.0), 1.0, &cx, 0);
        let px = g.grab((0, 0, 800, 480));
        let first = [px[0], px[1], px[2]];
        assert!(px.chunks(4).any(|p| [p[0], p[1], p[2]] != first), "something was drawn");
        assert!(matches!(app.handle(&Event::Key(Key::Char('x')), &cx), Flow::Stay));
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        assert!(matches!(app.handle(&Event::Key(Key::Char('q')), &cx), Flow::Back));
    }

    #[test]
    fn a_core_that_serves_the_objects_misses_nothing() {
        // the test services are the real ones: every object the apps ask for is served
        let cx = cx_with_admin_store("unserved2");
        for app in [PApp::Flows, PApp::Tales, PApp::Lab, PApp::Store, PApp::Radio, PApp::Media, PApp::Chat] {
            assert_eq!(missing(app, &cx), None, "{app:?} is built from what the real core serves");
        }
    }
}
