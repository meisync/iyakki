//! The events pane of SAMPLE (F-SDK-188): an app registers for events through the SDK's in-memory port, handles what waited while it was away
//! (marked late, before the first frame), reacts to live events (a note only for a live one), shows a refusal as a sentence and counts what was
//! missed. Driven through the SDK host with a clock; nothing sleeps. The core-backed port is tested where it lives (`iyakki-apps`).

use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::Phase;
use iyakki_ui_sdk::events::{ev_text, EventActions, EventsPort, MemEvents, RecordingActions};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use std::cell::RefCell;
use std::rc::Rc;

use super::*;

/// The memory port is shared with the test (a clone of the handle), as the core's is.
#[derive(Clone)]
struct Shared(Rc<RefCell<MemEvents>>);
impl EventsPort for Shared {
    fn register(&mut self, e: &str, f: &str) -> Result<(), iyakki_ui_sdk::events::EventsError> {
        self.0.borrow_mut().register(e, f)
    }
    fn unregister(&mut self, e: &str, f: &str) -> Result<(), iyakki_ui_sdk::events::EventsError> {
        self.0.borrow_mut().unregister(e, f)
    }
    fn registrations(&mut self) -> Vec<iyakki_ui_sdk::events::EventReg> {
        self.0.borrow_mut().registrations()
    }
    fn start(&mut self) -> Vec<iyakki_ui_sdk::events::AppEvent> {
        self.0.borrow_mut().start()
    }
    fn stop(&mut self) {
        self.0.borrow_mut().stop()
    }
    fn poll(&mut self, max: usize) -> Vec<iyakki_ui_sdk::events::AppEvent> {
        self.0.borrow_mut().poll(max)
    }
    fn replay(&mut self, a: u64) -> Vec<iyakki_ui_sdk::events::AppEvent> {
        self.0.borrow_mut().replay(a)
    }
    fn publish(&mut self, n: &str, p: iyakki_ui_sdk::events::EvPayload) -> Result<usize, iyakki_ui_sdk::events::EventsError> {
        self.0.borrow_mut().publish(n, p)
    }
}

#[derive(Clone, Default)]
struct Acts(Rc<RefCell<RecordingActions>>);
impl EventActions for Acts {
    fn notify(&mut self, t: &str) -> Result<(), String> {
        self.0.borrow_mut().notify(t)
    }
    fn start_flow(&mut self, n: &str) -> Result<(), String> {
        self.0.borrow_mut().start_flow(n)
    }
    fn call_tool(&mut self, t: &str, a: &str) -> Result<String, String> {
        self.0.borrow_mut().call_tool(t, a)
    }
}

fn mem() -> Shared {
    let mut m = MemEvents::new();
    m.allow("connectivity.changed");
    m.allow("session.changed");
    m.allow("settings.changed");
    m.coalescing("connectivity.changed");
    Shared(Rc::new(RefCell::new(m)))
}

fn host(w: u32, h: u32, p: &Shared, a: &Acts) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_events(Box::new(p.clone()), Box::new(a.clone())), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn tap(h: &mut Host<SampleApp>, name: &str) {
    let l = h.record();
    let (x, y) = l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { hit.name.starts_with(name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"));
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

#[test]
fn the_tab_exists_only_with_events_and_a_refusal_is_a_sentence() {
    let plain = Host::new(SampleApp::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    assert!(!plain.app.has_events());
    let (p, a) = (mem(), Acts::default());
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    assert_eq!(h.app.pane, Pane::Events);
    let t = texts(&h.record()).join("\n");
    // power.source was refused: the app does not hold that scope
    assert!(t.contains("power.source") && t.contains("permission"), "{t}");
    assert!(t.contains("Network: not heard yet"), "{t}");
}

#[test]
fn what_waited_is_handled_before_the_first_frame_marked_late_and_without_a_note() {
    let (p, a) = (mem(), Acts::default());
    // the app registered earlier and was away: events happened
    {
        let mut m = p.0.borrow_mut();
        m.register("connectivity.changed", "").unwrap();
        m.happen("connectivity.changed", ev_text(&[("state", "offline")]));
        m.happen("connectivity.changed", ev_text(&[("state", "online")]));
    }
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("Network: online"), "only the latest waited: {t}");
    assert!(t.contains("network online (late)"), "{t}");
    assert!(a.0.borrow().notes.is_empty(), "nothing rings for old news");
}

#[test]
fn a_live_event_arrives_on_the_tick_and_acts_and_missed_is_shown() {
    let (p, a) = (mem(), Acts::default());
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    p.0.borrow_mut().happen("connectivity.changed", ev_text(&[("state", "offline")]));
    h.tick(10);
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("Network: offline") && !t.contains("offline (late)"), "{t}");
    assert_eq!(a.0.borrow().notes, ["The network went away"]);
}

#[test]
fn dropped_events_arrive_as_one_missed_line() {
    let (p, a) = (mem(), Acts::default());
    {
        let mut m = p.0.borrow_mut();
        m.rate_limit("connectivity.changed", 1);
        m.register("connectivity.changed", "").unwrap();
        for s in ["online", "offline", "online"] {
            m.happen("connectivity.changed", ev_text(&[("state", s)]));
        }
    }
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("2 events were missed"), "{t}");
}

#[test]
fn the_pane_shows_the_session_and_the_last_setting_that_changed() {
    let (p, a) = (mem(), Acts::default());
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("Session: not heard yet") && t.contains("none heard yet"), "{t}");
    p.0.borrow_mut().happen("session.changed", ev_text(&[("state", "locked")]));
    p.0.borrow_mut().happen("settings.changed", ev_text(&[("setting", "time.zone")]));
    h.tick(10);
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("Session: locked") && t.contains("Last setting changed: time.zone") && t.contains("session locked"), "{t}");
}

#[test]
fn the_pane_shows_the_last_permission_change_even_one_that_came_while_it_was_closed() {
    let (p, a) = (mem(), Acts::default());
    p.0.borrow_mut().hold(&["connectivity.changed"]);
    p.0.borrow_mut().permission_change(&["camera.view"], &[], "administrator", true);
    let mut h = host(800, 480, &p, &a);
    tap(&mut h, "Events");
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("Last permission change: 1 held; +[camera.view] -[] by administrator; restart needed") && t.contains("(late)"), "{t}");
    p.0.borrow_mut().permission_change(&[], &["connectivity.changed"], "person", false);
    h.tick(10);
    let t = texts(&h.record()).join("\n");
    assert!(t.contains("-[connectivity.changed] by person"), "{t}");
}
