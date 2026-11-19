//! The model router in the chat: the tools are offered, a run needs the person's card, and a run the person approved is not refused by the
//! core for lack of a gesture even though the tool runs on another thread than the key press (F-ROUTER-06). The core here is the REAL guarded
//! one (the broker, the access log file and the gesture card) over a fake machine; the model is a script, and the providers are a closed port.

use super::tests::{agent_cx, ask, run_until, turn};
use super::*;
use iyakki_core_api::broker::{Caller, CallerKind};
use iyakki_core_api::guarded::{GestureCard, GuardedOs};
use iyakki_shared::providers::ProviderCfg;
use iyakki_agent::routertools::Backend as _;
use serde_json::json;

fn tool_results(app: &ChatApp) -> Vec<String> {
    app.messages().iter().filter(|m| m.role == iyakki_shared::chat::Role::Tool).map(|m| m.content.clone()).collect()
}

fn setup(tag: &str) -> (AppCx, Arc<GestureCard>, Arc<iyakki_agent::routertools::FileBackend>) {
    let call = |id: &str, goal: &str| turn("", vec![(id, "router_run", json!({"goal": goal, "plan": {"steps": [{"id": "a", "task": "do a"}]}}))]);
    let (mut cx, _h) = agent_cx(tag, |_| vec![call("r1", "first"), turn("Started.", vec![]), call("r2", "second"), turn("Refused.", vec![])]);
    let card = Arc::new(GestureCard::new(std::time::Duration::from_secs(1)));
    let (broker, _log) = iyakki_shared::accessbroker::local(cx.data_dir(), Some(card.clone()));
    let caller = Caller { app: "chat".into(), kind: CallerKind::Builtin, person: cx.session.user.clone(), device: "this".into() };
    cx.os = Arc::new(GuardedOs::new(Arc::new(iyakki_core_fake::FakeOs::new()), broker, caller));
    let mut p = ProviderCfg::new("lan", iyakki_shared::providers::Kind::Compat);
    p.base_url = Some("http://127.0.0.1:9/v1".into());
    p.models = vec!["big-70b".into(), "small-7b".into()];
    cx.settings().lock().unwrap().upsert_provider(p).unwrap();
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let be = Arc::new(iyakki_agent::routertools::FileBackend::for_services(iyakki_agent::services::Services::from_handle(&cx.services.clone()), &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d))));
    be.save_config(&iyakki_agent::router::RouterConfig { enabled: true, ..Default::default() }).unwrap();
    (cx, card, be)
}

#[test]
fn a_run_the_person_approved_in_the_chat_starts_on_the_real_guarded_core_and_one_nobody_pressed_for_is_refused() {
    let (cx, card, be) = setup("rtr-chat");
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    // the tools and the rule are in front of the model
    ask(&mut app, &cx, "start a router run");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    assert_eq!(app.pending_approval().map(|p| p.1.clone()).as_deref(), Some("router_run"), "a card for the run, even though Auto would run safe tools");
    // the person's press, handled the way the shell handles one: inside the gesture card's dispatch
    card.dispatch(|| app.handle(&Event::Key(Key::Ctrl('y')), &cx));
    run_until(&mut app, &cx, |a| !a.is_running());
    let results = tool_results(&app);
    assert!(results.iter().any(|r| r.contains("has started")), "refused for lack of a gesture? {results:?}");
    be.slot().join();
    assert!(cx.audit.recent(200).iter().any(|l| l.contains("router.run.permit")));
    // a second run whose card was answered with no press behind it (nothing in the dispatch) is refused by the core
    ask(&mut app, &cx, "and another");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    app.handle(&Event::Key(Key::Ctrl('y')), &cx);
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(tool_results(&app).iter().any(|r| r.contains("no_card")), "{:?}", tool_results(&app));
    be.slot().join();
}

#[test]
fn the_router_tools_are_offered_with_the_rule_and_the_selection_words_find_them() {
    let (cx, _card, _be) = setup("rtr-offer");
    let _ = &cx;
    assert!(iyakki_shared::routertools::PROMPT_RULE.contains("router_run") && iyakki_shared::routertools::PROMPT_RULE.contains("asks the person every time"));
    assert_eq!(iyakki_shared::toolblock::group_of("router_run"), iyakki_shared::toolblock::Group::Machine);
    assert_eq!(iyakki_shared::toolblock::group_of("router_status"), iyakki_shared::toolblock::Group::Machine);
    assert!(iyakki_shared::toolblock::intent_groups("start a model router run with a planner model").contains(&iyakki_shared::toolblock::Group::Machine));
}
