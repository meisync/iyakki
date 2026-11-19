//! The assistant and camera facets over a real loopback socket (F-SDK-85, F-SDK-166): `SocketCore` against the reference server serving the
//! core fakes. What is checked: the whole turn of the assistant with its approvals, events pushed instead of polled, the scopes that gate a
//! yes, a connection reaching only what it opened, and a camera picture cut into chunks and put back together.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_core_api::assistant::{AppToolSpec, AssistEvent, SessionSpec, ToolApproval};
use iyakki_core_api::cameras::{FrameFormat, ViewPhase, ViewSpec};
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::Core;
use iyakki_core_fake::{FakeAssistant, FakeCam, FakeCameras, FakeTurn};
use iyakki_ui_client::client::{Connection, Remote};
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::socketcore::SocketCore;
use iyakki_ui_client::*;

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    assistant: Arc<FakeAssistant>,
    cameras: Arc<FakeCameras>,
}

fn machine(scopes: &[&str], with_domains: bool) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let (assistant, cameras) = (Arc::new(FakeAssistant::new()), Arc::new(FakeCameras::new()));
    let (s2, a2, c2) = (stop.clone(), assistant.clone(), cameras.clone());
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    let thread = std::thread::spawn(move || {
        let refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let served = Served {
            hello: settings_hello("m", RateLimit { calls: 100_000, per_ms: 1000 }),
            settings: None,
            vault: None,
            flows: None,
            assistant: with_domains.then(|| a2 as Arc<dyn iyakki_core_api::Assistant>),
            cameras: with_domains.then(|| c2 as Arc<dyn iyakki_core_api::Cameras>),
            rpc: None,
            appevents: None,
            ctx: CallContext::remote("amy", Transport::LocalSocket, &refs),
        };
        serve(l, Arc::new(served), s2).unwrap();
    });
    Machine { addr, stop, thread: Some(thread), assistant, cameras }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn core(m: &Machine) -> SocketCore {
    SocketCore::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(20)).unwrap())
}

const ALL: [&str; 4] = ["assistant:use", "assistant:approve", "cameras:view", "cameras:consent"];

fn spec() -> SessionSpec {
    SessionSpec {
        conversation: "main".into(),
        title: "Notes".into(),
        tools: vec![AppToolSpec { name: "add_note".into(), description: "adds a note".into(), schema: "{\"type\":\"object\"}".into(), approval: ToolApproval::Ask }],
        ..Default::default()
    }
}

#[test]
fn a_whole_assistant_turn_runs_over_the_socket_and_its_events_are_pushed() {
    let m = machine(&ALL, true);
    m.assistant.script(vec![FakeTurn::call("Adding.", "add_note", r#"{"text":"eggs"}"#), FakeTurn::say("Done.")]);
    let c = core(&m);
    let a = c.assistant();
    let sid = a.open("ignored", "sample", &spec()).unwrap().id;
    c.socket_assistant().subscribe(&sid).unwrap();
    a.send("ignored", &sid, "add eggs").unwrap();
    let mut seen: Vec<AssistEvent> = Vec::new();
    let mut approved = false;
    while !seen.last().is_some_and(AssistEvent::ends_turn) {
        for e in c.socket_assistant().wait_events(&sid).unwrap() {
            match &e {
                AssistEvent::Approval { call, .. } => {
                    a.decide("ignored", &sid, call, true).unwrap();
                    approved = true;
                }
                AssistEvent::ToolCall { call, .. } => a.answer("ignored", &sid, call, true, "added").unwrap(),
                _ => {}
            }
            seen.push(e);
        }
    }
    assert!(approved, "the card came through the push: {seen:?}");
    assert!(seen.iter().any(|e| matches!(e, AssistEvent::ToolCall { tool, .. } if tool == "add_note")), "{seen:?}");
    assert!(matches!(seen.last(), Some(AssistEvent::Final { text }) if text == "Done."), "{seen:?}");
    assert!(a.history("ignored", &sid).iter().any(|h| h.text == "Done."));
    assert!(a.poll("ignored", &sid).is_empty(), "nothing is left to ask for once the events are pushed");
    a.close("ignored", &sid);
    assert!(a.send("ignored", &sid, "x").is_err(), "a closed session is gone");
}

#[test]
fn a_yes_needs_its_own_scope_and_a_no_does_not() {
    let m = machine(&["assistant:use", "cameras:view"], true);
    m.assistant.script(vec![FakeTurn::call("Adding.", "add_note", "{}")]);
    let c = core(&m);
    let a = c.assistant();
    let sid = a.open("u", "sample", &spec()).unwrap().id;
    a.send("u", &sid, "add").unwrap();
    // the fake core queues its events at once; poll (not subscribed) hands them out
    let call = a.poll("u", &sid).into_iter().find_map(|e| if let AssistEvent::Approval { call, .. } = e { Some(call) } else { None }).expect("a card");
    let e = a.decide("u", &sid, &call, true).unwrap_err();
    assert!(e.contains("assistant:approve"), "{e}");
    a.decide("u", &sid, &call, false).unwrap();
}

#[test]
fn a_connection_reaches_only_the_sessions_it_opened_and_a_domain_that_is_not_served_says_so() {
    let m = machine(&ALL, true);
    let (one, two) = (core(&m), core(&m));
    let sid = one.assistant().open("u", "sample", &spec()).unwrap().id;
    let e = two.assistant().send("u", &sid, "hello").unwrap_err();
    assert_eq!(e, "no such assistant session");
    assert!(two.assistant().poll("u", &sid).is_empty());
    // no scope at all: the domain is refused in words; a machine without the domains does not know them
    let none = machine(&[], true);
    assert!(core(&none).assistant().open("u", "sample", &spec()).unwrap_err().contains("assistant:use"));
    let bare = machine(&ALL, false);
    assert!(core(&bare).assistant().open("u", "sample", &spec()).unwrap_err().contains("does not serve assistant"));
    assert!(core(&bare).cameras().open("u", "sample", &ViewSpec { cameras: vec!["a".into()], ..Default::default() }).unwrap_err().contains("does not serve cameras"));
}

#[test]
fn a_camera_view_asks_the_person_and_a_big_picture_comes_in_chunks_that_fit_together() {
    let m = machine(&ALL, true);
    m.cameras.add(FakeCam::new("front", "Front door", "amy").with_ptz());
    // 200 x 100 RGBA is 80 000 bytes: two chunks of at most 40 KiB
    let pic: Vec<u8> = (0..80_000u32).map(|i| (i % 251) as u8).collect();
    m.cameras.push_frame("front", 200, 100, pic.clone());
    let c = core(&m);
    let cams = c.cameras();
    assert_eq!(cams.list("u", "sample")[0].name, "Front door");
    let v = cams.open("u", "sample", &ViewSpec { cameras: vec!["front".into()], ..Default::default() }).unwrap();
    assert!(matches!(cams.state("u", &v.id).unwrap().phase, ViewPhase::AwaitingConsent { .. }));
    assert!(cams.frame("u", &v.id, "front", 0, FrameFormat::Rgba).unwrap().is_none(), "nothing before the card");
    assert_eq!(m.cameras.pulled("front"), 0);
    let call = cams.events("u", &v.id).into_iter().find_map(|e| if let iyakki_core_api::cameras::CamEvent::Approval { call, .. } = e { Some(call) } else { None }).expect("a card");
    cams.decide("u", &v.id, &call, true).unwrap();
    let f = cams.frame("u", &v.id, "front", 0, FrameFormat::Rgba).unwrap().expect("a picture");
    assert_eq!((f.width, f.height, f.data.len()), (200, 100, 80_000));
    assert_eq!(*f.data, pic, "the chunks fit together");
    assert!(cams.frame("u", &v.id, "front", f.seq, FrameFormat::Rgba).unwrap().is_none(), "nothing newer");
    let snap = cams.snapshot("u", &v.id, "front").unwrap();
    assert!(!snap.is_empty());
    cams.close("u", &v.id);
    assert!(cams.state("u", &v.id).is_err(), "closed");
}

#[test]
fn the_consent_scope_gates_a_yes_and_ending_the_connection_closes_what_it_opened() {
    let m = machine(&["cameras:view"], true);
    m.cameras.add(FakeCam::new("front", "Front door", "amy"));
    let r = Remote::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(20)).unwrap());
    let cams = iyakki_ui_client::facets::SocketCameras::new(r.connection());
    use iyakki_core_api::Cameras;
    let v = cams.open("u", "sample", &ViewSpec { cameras: vec!["front".into()], ..Default::default() }).unwrap();
    let call = cams.events("u", &v.id).into_iter().find_map(|e| if let iyakki_core_api::cameras::CamEvent::Approval { call, .. } = e { Some(call) } else { None }).unwrap();
    assert!(cams.decide("u", &v.id, &call, true).unwrap_err().contains("cameras:consent"));
    assert_eq!(m.cameras.open_views("amy"), 1);
    drop(cams);
    drop(r);
    // the connection is gone: the machine closes the view (the server thread notices the end of the stream; wait for it by asking the fake)
    let end = std::time::Instant::now() + Duration::from_secs(30);
    while m.cameras.open_views("amy") != 0 && std::time::Instant::now() < end {
        std::thread::yield_now();
    }
    assert_eq!(m.cameras.open_views("amy"), 0, "a gone UI leaves no view open");
}
