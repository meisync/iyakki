//! Tests of the FLOWS chat: the flows-first context (only in FLOWS, no app creation), the form for missing input (validation, keys,
//! mouse, masked secret that never reaches the transcript), and one conversation per workflow.

use super::tests::{ask, render_with, run_until};
use super::*;
use crate::apps::tests::cx_with_backend;
use iyakki_agent::chat::{ModelTurn, ToolSpec};
use iyakki_agent::flows::{FlowEnv, Open, SharedOpen};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A model that follows a script and keeps what it was shown: (messages, tool names) of each request.
struct Cap {
    turns: Mutex<std::collections::VecDeque<ModelTurn>>,
    seen: Arc<Mutex<Vec<(Vec<Message>, Vec<String>)>>>,
}
impl ChatBackend for Cap {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no plain chat".into())
    }
    fn complete_tools(&self, m: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push((m.to_vec(), t.iter().map(|x| x.name.clone()).collect()));
        self.turns.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
    }
}

type Seen = Arc<Mutex<Vec<(Vec<Message>, Vec<String>)>>>;

fn turn(text: &str, calls: Vec<(&str, &str, serde_json::Value)>) -> ModelTurn {
    ModelTurn { text: text.into(), calls: calls.into_iter().map(|(id, n, a)| ToolCall { id: id.into(), name: n.into(), args: a }).collect() }
}

fn rig(tag: &str, turns: Vec<ModelTurn>, flows: bool) -> (ChatApp, AppCx, Seen, SharedOpen, Arc<FlowEnv>) {
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let (mut cx, _d) = cx_with_backend(tag, Cap { turns: Mutex::new(turns.into()), seen: seen.clone() });
    if !cx.session.policy.apps.contains(&iyakki_sec::policy::App::Flows) {
        cx.session.policy.apps.insert(iyakki_sec::policy::App::Flows);
    }
    let home = std::env::temp_dir().join(format!("iyakki-chatform-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let env = Arc::new(FlowEnv::build_in(&cx.services, &cx.session.user, &cx.session.policy, false, &home));
    let open: SharedOpen = Arc::new(Mutex::new(Open { tab: "design".into(), ..Default::default() }));
    let mut app = ChatApp::new();
    if flows {
        app = app.with_flows(FlowsLink { env: env.clone(), open: open.clone() });
    }
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    (app, cx, seen, open, env)
}

fn form_turn() -> ModelTurn {
    turn("", vec![("f1", "ask_form", json!({"title": "Inputs for backup", "fields": [
        {"name": "host", "label": "Host", "kind": "text", "required": true},
        {"name": "token", "label": "API token", "kind": "secret", "required": true},
        {"name": "mode", "label": "Mode", "kind": "choice", "choices": ["full", "fast"], "required": true, "default": "full"}
    ]}))])
}

fn type_text(app: &mut ChatApp, cx: &AppCx, t: &str) {
    for c in t.chars() {
        app.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn last_user(app: &ChatApp) -> String {
    app.messages().iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

#[test]
fn the_flows_context_is_only_in_flows_leads_with_flow_tools_and_offers_no_app_creation() {
    let (mut app, cx, seen, open, env) = rig("ffctx", vec![turn("ok", vec![])], true);
    iyakki_agent::flows::tools::FlowTools::context(&env, &open);
    open.lock().unwrap().flow = Some("nightly".into());
    ask(&mut app, &cx, "make a backup");
    run_until(&mut app, &cx, |a| !a.is_running());
    let (msgs, tools) = seen.lock().unwrap()[0].clone();
    let sys = msgs.iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(sys.contains("You work in FLOWS") && sys.contains("a FLOW (flow_create)") && sys.contains("never an IYAKKI tile app"), "{sys}");
    assert!(tools.iter().any(|t| t == "flow_list") && tools.iter().any(|t| t == "ask_form"));
    assert!(tools.iter().take_while(|t| t.starts_with("flow_")).count() >= 10, "flow tools lead: {tools:?}");
    for t in ["app_scaffold", "tile_scaffold", "tile_pack", "tile_ship", "tile_validate"] {
        assert!(!tools.iter().any(|x| x == t), "{t} must not be offered in FLOWS");
    }
    // the ordinary chat has none of it
    let (mut plain, cx2, seen2, _o, _e) = rig("ffctx2", vec![turn("ok", vec![])], false);
    ask(&mut plain, &cx2, "hello");
    run_until(&mut plain, &cx2, |a| !a.is_running());
    let (msgs, tools) = seen2.lock().unwrap()[0].clone();
    assert!(!msgs.iter().any(|m| m.content.contains("You work in FLOWS")));
    assert!(!tools.iter().any(|t| t == "ask_form"));
}

#[test]
fn a_form_for_missing_input_validates_takes_keys_masks_the_secret_and_the_secret_never_reaches_the_transcript() {
    let (mut app, cx, _seen, _o, _e) = rig("ffform", vec![form_turn(), turn("Running it now.", vec![])], true);
    ask(&mut app, &cx, "run the backup");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(app.form_open(), "error: {:?}", app.error_text());
    assert_eq!(app.form_labels(), vec!["Host *", "API token *", "Mode *"]);
    // the conversation says what is asked, with no values
    assert!(app.messages().last().unwrap().content.contains("Inputs for backup"));
    // Ctrl+S with a required field empty: an error, the form stays, the first empty field has the keyboard
    app.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(app.form_open());
    assert!(app.form_error().unwrap_or_default().contains("required"), "the error is shown");
    let _ = render_with(&mut app, &cx, 1280, 720);
    // type the host, Tab to the secret and type it, Enter submits
    type_text(&mut app, &cx, "db.local");
    app.handle(&Event::Key(Key::Enter), &cx); // Enter walks to the next field
    assert!(app.form_open());
    type_text(&mut app, &cx, "TOPSECRET-123");
    app.handle(&Event::Key(Key::Tab), &cx);
    assert!(app.form_open());
    let _ = render_with(&mut app, &cx, 1280, 720);
    app.handle(&Event::Key(Key::Enter), &cx);
    if app.form_open() {
        // a vault that is locked in this test rig keeps the form open with a clear message and nothing sent
        assert!(app.form_error().unwrap_or_default().contains("vault"));
        assert!(!app.messages().iter().any(|m| m.content.contains("TOPSECRET")));
        return;
    }
    let sent = last_user(&app);
    assert!(sent.contains("host: db.local") && sent.contains("mode: full") && sent.contains("{{secret.flow.token}}"), "{sent}");
    assert!(app.messages().iter().all(|m| !m.content.contains("TOPSECRET-123")), "no secret in the transcript");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert_eq!(app.messages().last().unwrap().content, "Running it now.");
}

#[test]
fn escape_cancels_the_form_with_a_plain_cancelled_result() {
    let (mut app, cx, _s, _o, _e) = rig("ffcancel", vec![form_turn(), turn("Okay, not running it.", vec![])], true);
    ask(&mut app, &cx, "run the backup");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(app.form_open());
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(!app.form_open());
    assert_eq!(last_user(&app), "Form cancelled.");
}

#[test]
fn the_form_draws_without_overlap_at_the_usual_sizes_and_its_buttons_work_with_the_mouse() {
    let (mut app, cx, _s, _o, _e) = rig("ffgeom", vec![form_turn(), turn("done", vec![])], true);
    ask(&mut app, &cx, "run the backup");
    run_until(&mut app, &cx, |a| !a.is_running());
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let g = render_with(&mut app, &cx, w, h);
        for (i, a) in g.hits.iter().enumerate() {
            assert!(a.rect.x >= -0.5 && a.rect.y >= -0.5 && a.rect.x + a.rect.w <= w as f32 + 0.5 && a.rect.y + a.rect.h <= h as f32 + 0.5, "{w}x{h}: {} {:?} outside", a.label, a.rect);
            for b in g.hits.iter().skip(i + 1) {
                let o = a.rect.x < b.rect.x + b.rect.w - 0.5 && b.rect.x < a.rect.x + a.rect.w - 0.5 && a.rect.y < b.rect.y + b.rect.h - 0.5 && b.rect.y < a.rect.y + a.rect.h - 0.5;
                if o {
                    // a hit area inside a bigger one (the dim backdrop, the dialog) is allowed; two controls side by side are not
                    let inside = |x: R, y: R| x.x >= y.x - 0.5 && x.y >= y.y - 0.5 && x.x + x.w <= y.x + y.w + 0.5 && x.y + x.h <= y.y + y.h + 0.5;
                    assert!(inside(a.rect, b.rect) || inside(b.rect, a.rect), "{w}x{h}: {} and {} overlap", a.label, b.label);
                }
            }
        }
    }
    // a click on the Cancel button cancels
    let g = render_with(&mut app, &cx, 1280, 720);
    let cancel = g.hits.iter().find(|h| h.label.to_lowercase().contains("cancel")).map(|h| h.ev.clone());
    let ev = cancel.expect("a Cancel button");
    app.handle(&ev, &cx);
    assert!(!app.form_open());
    assert_eq!(last_user(&app), "Form cancelled.");
}

#[test]
fn each_workflow_has_its_own_conversation_and_nothing_leaks_between_them() {
    let (mut app, cx, seen, open, _e) = rig("ffthread", vec![turn("About alpha.", vec![]), turn("About beta.", vec![]), turn("Back on alpha.", vec![])], true);
    open.lock().unwrap().flow = Some("alpha".into());
    ask(&mut app, &cx, "alpha question");
    run_until(&mut app, &cx, |a| !a.is_running());
    // switching to another workflow shows an empty conversation for it
    open.lock().unwrap().flow = Some("beta".into());
    app.handle(&Event::Tick, &cx);
    assert!(app.messages().iter().all(|m| !m.content.contains("alpha question")));
    ask(&mut app, &cx, "beta question");
    run_until(&mut app, &cx, |a| !a.is_running());
    let beta_req = seen.lock().unwrap()[1].0.clone();
    assert!(!beta_req.iter().any(|m| m.content.contains("alpha question") || m.content.contains("About alpha.")), "beta's request carries nothing of alpha's thread");
    // back to alpha: its own thread returns
    open.lock().unwrap().flow = Some("alpha".into());
    app.handle(&Event::Tick, &cx);
    assert!(app.messages().iter().any(|m| m.content.contains("alpha question")));
    assert!(app.messages().iter().all(|m| !m.content.contains("beta question")));
    // no flow open: yet another thread
    open.lock().unwrap().flow = None;
    app.handle(&Event::Tick, &cx);
    assert!(app.messages().iter().all(|m| !m.content.contains("alpha question") && !m.content.contains("beta question")));
}

// ------------------------------------------------------------------------------------------------ the flow still attached to the CHAT tile

fn make_flow(env: &FlowEnv, name: &str) -> String {
    let spec = json!({"name": name, "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]});
    env.create_draft(iyakki_agent::flows::build_flow(&spec).unwrap(), "user:root").unwrap().id
}

#[test]
fn the_plain_chat_has_the_last_open_flow_as_context_with_a_chip_until_it_is_detached() {
    crate::apps::tests::isolated_home();
    let (mut app, mut cx, seen, _open, _env) = rig("ffsticky", vec![turn("ok", vec![]), turn("ok", vec![])], false);
    cx.session.user = "sticky-chip-user".into();
    assert!(app.flow_chip().is_none(), "nothing recorded: no chip, no flows context");
    // the chat opens the account's own environment (the one FLOWS uses)
    let own = FlowEnv::open(&cx.services, &cx.session.user, &cx.session.policy).unwrap();
    let own_id = make_flow(&own, "Nightly backup");
    iyakki_agent::flows::set_last_flow(&cx.services, &cx.session.user, &own_id, "Nightly backup");
    app.handle(&Event::Tick, &cx);
    assert_eq!(app.flow_chip().as_deref(), Some("Flow: Nightly backup"));
    let _ = render_with(&mut app, &cx, 1280, 720);
    ask(&mut app, &cx, "add a step");
    run_until(&mut app, &cx, |a| !a.is_running());
    let (msgs, tools) = seen.lock().unwrap()[0].clone();
    let sys = msgs.iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(sys.contains("You work in FLOWS") && sys.contains(&format!("OPEN ON SCREEN: flow `{own_id}` \"Nightly backup\"")), "{sys}");
    assert!(tools.iter().take_while(|t| t.starts_with("flow_")).count() >= 10, "{tools:?}");
    // the thread is the flow's own
    assert!(!cx.services.sessions.list(&iyakki_agent::flows::chat_key_for(&cx.session.user, Some(&own_id))).is_empty());
    // the X lets it go: no chip, nothing recorded, and the next request is the ordinary chat
    app.handle(&Event::Tab(super::FLOW_CHIP_X), &cx);
    assert!(app.flow_chip().is_none() && !iyakki_agent::flows::last_flow_recorded(&cx.services, &cx.session.user));
    ask(&mut app, &cx, "hello");
    run_until(&mut app, &cx, |a| !a.is_running());
    let (msgs, tools) = seen.lock().unwrap()[1].clone();
    assert!(!msgs.iter().any(|m| m.content.contains("You work in FLOWS")) && !tools.iter().any(|t| t == "flow_list"));
}

#[test]
fn the_chip_says_when_the_last_run_failed_and_the_context_is_logged_without_secrets() {
    crate::apps::tests::isolated_home();
    let (mut app, mut cx, seen, _open, _env) = rig("ffchip", vec![turn("ok", vec![])], false);
    cx.session.user = "sticky-fail-user".into();
    let env = FlowEnv::open(&cx.services, &cx.session.user, &cx.session.policy).unwrap();
    let spec = json!({"name": "Red flow", "nodes": [{"id": "call", "kind": "http", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 1}}], "edges": [{"from": "start", "to": "call"}]});
    let f = env.create_draft(iyakki_agent::flows::build_flow(&spec).unwrap(), "user:root").unwrap();
    env.store.publish(&f.id, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let run = env.engine.start_run(iyakki_flow::StartRequest::new(&f.id).trigger("manual", "me")).unwrap().run_id;
    env.engine.wait_for(&run, std::time::Duration::from_secs(20), |r| r.status.is_terminal()).unwrap();
    iyakki_agent::flows::set_last_flow(&cx.services, &cx.session.user, &f.id, "Red flow");
    app.handle(&Event::Tick, &cx);
    assert_eq!(app.flow_chip().as_deref(), Some("Flow: Red flow (last run failed)"));
    let lines = test_log::capture(|| {
        ask(&mut app, &cx, "what happened");
        run_until(&mut app, &cx, |a| !a.is_running());
    });
    let (msgs, _) = seen.lock().unwrap()[0].clone();
    let sys = msgs.iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(sys.contains(&format!("NEWEST FAILED RUN {run}")) && sys.contains("failed step `call`"), "{sys}");
    if let Some(lines) = lines {
        let want = format!("chat: flows context flow={} name=Red flow failed_run={run}", f.id);
        assert!(lines.iter().any(|l| l == &want), "{lines:?}");
    }
}

/// Captures `log::info!` lines while a closure runs, when this test binary could install the logger (None when another test did).
mod test_log {
    use std::sync::Mutex;
    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    struct Cap;
    impl log::Log for Cap {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            LINES.lock().unwrap().push(format!("{}", r.args()));
        }
        fn flush(&self) {}
    }
    pub fn capture(f: impl FnOnce()) -> Option<Vec<String>> {
        let ok = *INSTALLED.get_or_init(|| {
            let ok = log::set_logger(&Cap).is_ok();
            log::set_max_level(log::LevelFilter::Info);
            ok
        });
        f();
        ok.then(|| LINES.lock().unwrap().clone())
    }
}
