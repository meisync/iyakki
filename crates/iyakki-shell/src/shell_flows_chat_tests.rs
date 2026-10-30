//! The chat in and after FLOWS, driven through the real shell (tile, keys, ticks, the chat's real request to a scripted model):
//! the open flow and its newest failure reach the assistant on every path (the embedded pane, the card's "Ask the agent to
//! fix", the ordinary CHAT tile after leaving FLOWS), the chip shows it and detaches it, and the sticky context ends at delete
//! and sign-out. What the model is shown is read from the request it received.

use super::tests::*;
use super::*;
use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, Role as MsgRole, ToolSpec};
use iyakki_agent::flows::{build_flow, FlowEnv};
use serde_json::json;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

type Seen = Arc<StdMutex<Vec<(Vec<Message>, Vec<String>)>>>;

struct Cap(Seen);
impl ChatBackend for Cap {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no plain chat".into())
    }
    fn complete_tools(&self, m: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.0.lock().unwrap().push((m.to_vec(), t.iter().map(|x| x.name.clone()).collect()));
        Ok(ModelTurn { text: "ok".into(), calls: vec![] })
    }
}

/// A shell whose model records each request, signed in as the administrator, with one failed flow made through the same
/// environment the FLOWS tile uses. Returns (shell, requests, flow id, failed run id, the input value that must never show).
fn rig(tag: &str) -> (Shell, Seen, String, String, String, std::sync::MutexGuard<'static, ()>) {
    // the tests share one home folder (HOME is per process) and the list opens its first flow: they run one at a time
    static ONE_AT_A_TIME: StdMutex<()> = StdMutex::new(());
    let guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    crate::apps::tests::isolated_home();
    let seen: Seen = Arc::new(StdMutex::new(vec![]));
    let (mut e, _h) = env(tag);
    e.chat = Arc::new(Cap(seen.clone()));
    {
        let mut st = e.store.lock().unwrap();
        st.create_first_admin("root", "rootpass12").unwrap();
        let admin = st.authenticate("root", "rootpass12").unwrap();
        // one account per test: the tests share the process, and an account's flows live in its home
        st.create_user(&admin, tag, "fcpass12345", iyakki_sec::policy::Role::Admin).unwrap();
    }
    let mut sh = Shell::new(e);
    login(&mut sh, tag, "fcpass12345");
    let policy = sh.session.as_ref().unwrap().policy.clone();
    let env = FlowEnv::open(&sh.env.services, tag, &policy).unwrap();
    for f in env.summaries().iter().filter(|f| f.id.starts_with("00-fc-")) {
        let _ = env.delete_flow(&f.id);
    }
    let token = format!("{}{}", "ghp_", "Z9y8X7w6V5u4T3s2R1q0P9o8N7m6L5k4J3i2");
    let spec = json!({"name": format!("00 fc {tag}"), "inputs": [{"name": "api", "type": "text", "required": false}], "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 1}}], "edges": [{"from": "start", "to": "call"}]});
    let flow = env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
    env.store.publish(&flow.id, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let mut req = iyakki_flow::StartRequest::new(&flow.id).trigger("manual", "me");
    req.inputs = json!({"api": token});
    let run = env.engine.start_run(req).unwrap().run_id;
    env.engine.wait_for(&run, Duration::from_secs(20), |r| r.status.is_terminal()).unwrap();
    (sh, seen, flow.id, run, token, guard)
}

fn tick_until(sh: &mut Shell, what: &str, mut done: impl FnMut(&Shell) -> bool) {
    for _ in 0..800 {
        sh.handle(&Event::Tick);
        if done(sh) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn type_and_send(sh: &mut Shell, text: &str) {
    typ(sh, text);
    key(sh, Key::Enter);
}

fn answered(seen: &Seen, n: usize) -> impl FnMut(&Shell) -> bool + '_ {
    move |_| seen.lock().unwrap().len() >= n
}

fn system_of(seen: &Seen, i: usize) -> (String, Vec<String>, String) {
    let (msgs, tools) = seen.lock().unwrap()[i].clone();
    let sys = msgs.iter().filter(|m| m.role == MsgRole::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    let all = msgs.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    (sys, tools, all)
}

fn flows_first(sys: &str, tools: &[String], flow: &str, run: &str) {
    let name = flow.replace('-', " ");
    assert!(sys.contains("You work in FLOWS") && sys.contains("a FLOW (flow_create)"), "flows-first prompt: {sys}");
    assert!(sys.contains(&format!("OPEN ON SCREEN: flow `{flow}` \"{name}\", published as version 1")), "the open flow: {sys}");
    assert!(sys.contains("node call [http]") && sys.contains("inputs (names only): api"), "steps and input names: {sys}");
    assert!(sys.contains(&format!("NEWEST FAILED RUN {run}")) && sys.contains("failed step `call` (Call the site) [http], attempt 1 of 1"), "the failure: {sys}");
    assert!(tools.iter().take_while(|t| t.starts_with("flow_")).count() >= 10, "flow tools lead: {tools:?}");
    assert!(!tools.iter().any(|t| t == "app_scaffold" || t == "tile_scaffold"), "no app creation: {tools:?}");
}

fn open_flows_and_flow(sh: &mut Shell) {
    assert!(sh.open_for_demo("flows"), "{}", sh.screen_name());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    key(sh, Key::Enter);
    sh.render(&mut g);
}

#[test]
fn the_chat_key_in_flows_gives_the_assistant_the_open_flow_and_its_failure() {
    let (mut sh, seen, flow, run, token, _guard) = rig("fckey");
    open_flows_and_flow(&mut sh);
    key(&mut sh, Key::Ctrl('i'));
    type_and_send(&mut sh, "why is it red");
    tick_until(&mut sh, "the model's request", answered(&seen, 1));
    let (sys, tools, all) = system_of(&seen, 0);
    flows_first(&sys, &tools, &flow, &run);
    assert!(!all.contains(&token), "no input value reaches the model");
    // the conversation is this flow's own thread
    let key_ = iyakki_agent::flows::chat_key_for("fckey", Some(&flow));
    for _ in 0..300 {
        sh.handle(&Event::Tick);
        if !sh.env.services.sessions.list(&key_).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!sh.env.services.sessions.list(&key_).is_empty(), "the thread is keyed by the flow id");
}

#[test]
fn the_failure_cards_button_opens_the_chat_with_the_context_and_the_prepared_request() {
    let (mut sh, seen, flow, run, token, _guard) = rig("fccard");
    open_flows_and_flow(&mut sh);
    // the card's button (its key F): the same call the click makes
    key(&mut sh, Key::Char('F'));
    tick_until(&mut sh, "the model's request", answered(&seen, 1));
    let (sys, tools, all) = system_of(&seen, 0);
    flows_first(&sys, &tools, &flow, &run);
    assert!(all.contains("FLOW FAILURE REQUEST (fix attempt 1 of 3)") && all.contains("step `call`") && all.contains(&run), "the prepared request: {all}");
    assert!(!all.contains(&token));
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "chat pane"), "the assistant is open beside the flow");
}

#[test]
fn leaving_flows_keeps_the_flow_as_context_in_the_chat_tile_with_a_chip_that_detaches_it() {
    let (mut sh, seen, flow, run, token, _guard) = rig("fcsticky");
    open_flows_and_flow(&mut sh);
    // Esc closes the flow, Esc again leaves FLOWS
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Esc);
    assert_eq!(sh.screen_name(), "launcher");
    assert!(sh.open_for_demo("chat"), "{}", sh.screen_name());
    sh.handle(&Event::Tick);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label.contains("Detach")), "the chip with its X is shown");
    type_and_send(&mut sh, "fix the flow please");
    tick_until(&mut sh, "the model's request", answered(&seen, 1));
    let (sys, tools, all) = system_of(&seen, 0);
    flows_first(&sys, &tools, &flow, &run);
    assert!(!all.contains(&token));
    // X detaches: the next question has no flow context, and the flow is forgotten
    for _ in 0..300 {
        sh.handle(&Event::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let x = sh.hits().iter().find(|h| h.label.contains("Detach")).unwrap().ev.clone();
    sh.handle(&x);
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label.contains("Detach")), "the chip is gone");
    assert!(!iyakki_agent::flows::last_flow_recorded(&sh.env.services, "fcsticky"));
    type_and_send(&mut sh, "hello there");
    tick_until(&mut sh, "the second request", answered(&seen, 2));
    let (sys2, tools2, _) = system_of(&seen, 1);
    assert!(!sys2.contains("You work in FLOWS") && !sys2.contains("OPEN ON SCREEN") && !tools2.iter().any(|t| t == "flow_list"), "{sys2}");
}

#[test]
fn the_sticky_flow_ends_when_the_flow_is_deleted_or_the_person_signs_out() {
    let (mut sh, seen, flow, _run, _t, _guard) = rig("fcend");
    open_flows_and_flow(&mut sh);
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Esc);
    let policy = sh.session.as_ref().unwrap().policy.clone();
    let env = FlowEnv::open(&sh.env.services, "fcend", &policy).unwrap();
    assert!(iyakki_agent::flows::last_flow_recorded(&sh.env.services, "fcend"));
    // delete: the chat tile no longer has it
    env.delete_flow(&flow).unwrap();
    assert!(sh.open_for_demo("chat"));
    sh.handle(&Event::Tick);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label.contains("Detach")));
    type_and_send(&mut sh, "hello");
    tick_until(&mut sh, "the request", answered(&seen, 1));
    assert!(!system_of(&seen, 0).0.contains("OPEN ON SCREEN"));
    // sign-out clears a recorded flow
    iyakki_agent::flows::set_last_flow(&sh.env.services, "fcend", "gone-soon", "Gone soon");
    assert!(iyakki_agent::flows::last_flow_recorded(&sh.env.services, "fcend"));
    sh.screen = Screen::Launcher;
    key(&mut sh, Key::Char('o'));
    assert!(!iyakki_agent::flows::last_flow_recorded(&sh.env.services, "fcend"), "sign-out forgets the flow");
}
