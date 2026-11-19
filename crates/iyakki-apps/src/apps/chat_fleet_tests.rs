//! The fleet in the chat: the tools are in front of the model, a change needs the person's card, and one the person approved is not refused by
//! the core for lack of a gesture even though the tool runs on another thread (F-FLEET-07, F-FLEET-09). The core is the REAL guarded one (the
//! broker, the access log file and the gesture card) over a fake machine; the model is a script.

use super::tests::{agent_cx, ask, run_until, turn};
use super::*;
use iyakki_agent::fleet_hub::Hub;
use iyakki_core_api::broker::{Caller, CallerKind};
use iyakki_core_api::guarded::{GestureCard, GuardedOs};
use serde_json::json;

fn tool_results(app: &ChatApp) -> Vec<String> {
    app.messages().iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.clone()).collect()
}

fn setup(tag: &str, script: impl FnOnce(&std::path::Path) -> Vec<iyakki_shared::chat::ModelTurn>) -> (AppCx, Arc<GestureCard>, Arc<Hub>) {
    let (mut cx, _h) = agent_cx(tag, script);
    let card = Arc::new(GestureCard::new(std::time::Duration::from_secs(1)));
    let (broker, _log) = iyakki_shared::accessbroker::local(cx.data_dir(), Some(card.clone()));
    let caller = Caller { app: "chat".into(), kind: CallerKind::Builtin, person: cx.session.user.clone(), device: "this".into() };
    cx.os = Arc::new(GuardedOs::new(Arc::new(iyakki_core_fake::FakeOs::new()), broker, caller));
    let hub = Hub::open(cx.data_dir()).unwrap();
    hub.set_bind("127.0.0.1:0");
    (cx, card, hub)
}

#[test]
fn a_fleet_the_person_approved_in_the_chat_is_made_on_the_real_guarded_core_and_one_nobody_pressed_for_is_refused() {
    let call = |id: &str, n: &str| turn("", vec![(id, "fleet_pair", json!({"action": "create", "name": n, "my_name": "alpha"}))]);
    let (cx, card, hub) = setup("flt-chat", move |_: &std::path::Path| vec![call("f1", "one"), turn("Made.", vec![]), call("f2", "two"), turn("Refused.", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "make a fleet");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    assert_eq!(app.pending_approval().map(|p| p.1.clone()).as_deref(), Some("fleet_pair"), "a card, even in Auto mode");
    assert!(hub.status().fleets.is_empty(), "nothing before the press");
    card.dispatch(|| app.handle(&Event::Key(Key::Ctrl('y')), &cx));
    run_until(&mut app, &cx, |a| !a.is_running());
    let results = tool_results(&app);
    assert!(results.iter().any(|r| r.contains("Created the fleet one")), "refused for lack of a gesture? {results:?}");
    assert_eq!(hub.status().fleets.len(), 1);
    assert!(cx.audit.recent(200).iter().any(|l| l.contains("fleet.create")));
    // a second one whose card was answered with no press behind it is refused by the core
    ask(&mut app, &cx, "and another");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    app.handle(&Event::Key(Key::Ctrl('y')), &cx);
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(tool_results(&app).iter().any(|r| r.contains("no_card")), "{:?}", tool_results(&app));
    assert_eq!(hub.status().fleets.len(), 1, "the refused one was not made");
    hub.stop_node();
}

#[test]
fn the_fleet_status_runs_without_a_card_and_the_tools_the_rule_and_the_selection_words_are_in_place() {
    let (cx, _card, hub) = setup("flt-status", |_: &std::path::Path| vec![turn("", vec![("s1", "fleet_status", json!({}))]), turn("Looked.", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "is this computer in a fleet");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(app.pending_approval().is_none(), "a read asks nothing");
    assert!(tool_results(&app).iter().any(|r| r.contains("in no fleet")), "{:?}", tool_results(&app));
    assert!(cx.services.fleet_prompt_rule().contains("fleet_assign"));
    assert_eq!(iyakki_shared::toolblock::group_of("fleet_assign"), iyakki_shared::toolblock::Group::Machine);
    let _ = hub;
}
