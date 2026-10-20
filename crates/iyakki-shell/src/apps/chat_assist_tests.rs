//! The chat with the assistant's safety net: the capability reminder is never shown or saved, a wrong "I cannot" is retried and
//! shown as a step, the answer streams in, the footer says where the time went, a long chat gets a rolling summary, and
//! `screen_capture` asks in the chat.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::tests::{agent_cx, ask, render, run_until, turn};
use super::*;
use crate::apps::tests::cx_with_backend;
use iyakki_agent::chat::{ModelTurn, ToolSpec};
use serde_json::json;

/// A scripted model that remembers every request and can stream.
struct Rec {
    turns: Mutex<VecDeque<ModelTurn>>,
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
    pieces: Vec<(u64, &'static str)>,
}

impl ChatBackend for Rec {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no plain chat".into())
    }
    fn complete_tools(&self, h: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push(h.to_vec());
        self.turns.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
    }
    fn complete_tools_stream(&self, h: &[Message], t: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        if self.pieces.is_empty() {
            return self.complete_tools(h, t);
        }
        self.seen.lock().unwrap().push(h.to_vec());
        let mut text = String::new();
        for (pause, p) in &self.pieces {
            std::thread::sleep(Duration::from_millis(*pause));
            text.push_str(p);
            on_text(p);
        }
        Ok(ModelTurn { text, calls: vec![] })
    }
}

fn rec(tag: &str, turns: Vec<ModelTurn>, pieces: Vec<(u64, &'static str)>) -> (AppCx, std::path::PathBuf, Arc<Mutex<Vec<Vec<Message>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-chat-assist-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let (mut cx, _d) = cx_with_backend(tag, Rec { turns: Mutex::new(turns.into()), seen: seen.clone(), pieces });
    cx.session.policy.fs_read = vec![home.display().to_string()];
    cx.session.policy.fs_write = vec![home.display().to_string()];
    (cx, home, seen)
}

fn agent(cx: &AppCx) -> ChatApp {
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), cx);
    app
}

#[test]
fn a_wrong_i_cannot_is_retried_before_it_is_shown_and_the_reminder_is_never_saved() {
    let (cx, home, seen) = rec(
        "deny",
        vec![
            turn("I cannot access your files or folders. My capabilities are confined to the tools described earlier.", vec![]),
            turn("", vec![("c1", "list_dir", json!({"path": "."}))]),
            turn("Your folder has notes.txt.", vec![]),
        ],
        vec![],
    );
    std::fs::write(home.join("notes.txt"), "x").unwrap();
    let mut app = agent(&cx);
    ask(&mut app, &cx, "show me the files in my folder");
    run_until(&mut app, &cx, |a| !a.is_running());
    let msgs = app.messages();
    assert_eq!(msgs.last().unwrap().content, "Your folder has notes.txt.");
    assert!(!msgs.iter().any(|m| m.content.contains("cannot access your files")), "the refusal was never shown: {msgs:?}");
    let step = msgs.iter().find(|m| m.tool_calls.iter().any(|c| c.name == "assistant_check")).expect("the retry is a step in the conversation");
    assert!(!step.tool_calls.is_empty());
    assert!(msgs.iter().any(|m| m.role == Role::Tool && m.content.contains("tools can")), "{msgs:?}");
    // the model was reminded, near the end of its request; the chat never kept the reminder
    let first = seen.lock().unwrap()[0].clone();
    assert!(first.last().unwrap().content.contains("[TOOLS AVAILABLE NOW]") && first.last().unwrap().content.contains("list_dir"));
    assert!(app.messages().iter().all(|m| !m.content.contains("TOOLS AVAILABLE NOW")), "not in the transcript");
    let saved = cx.services.sessions.load("tester", app.session_id().unwrap()).unwrap();
    assert!(saved.messages.iter().all(|m| !m.content.contains("TOOLS AVAILABLE NOW") && !m.content.contains("SYSTEM NOTE")), "not in the saved chat");
    // the step is drawn
    let g = render(&mut app, &cx);
    assert!(g.hits.iter().any(|h| h.label == "text area"));
    // the run record has it
    let (log, _) = cx.services.runs.load("tester", &cx.services.runs.list("tester")[0].id).unwrap();
    assert!(log.events.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("list_dir"))));
    assert!(log.events.iter().any(|e| matches!(e, RunEvent::Timing { .. })));
    assert!(!log.events.iter().any(|e| matches!(e, RunEvent::Delta { .. })), "partial text is not recorded");
}

#[test]
fn old_refusals_in_the_chat_are_not_sent_back_to_the_model_but_stay_in_the_saved_chat() {
    let (cx, home, seen) = rec("hygiene", vec![turn("Your folder has notes.txt.", vec![])], vec![]);
    std::fs::write(home.join("notes.txt"), "x").unwrap();
    let mut app = agent(&cx);
    app.msgs.push(Message::user("list my files"));
    app.msgs.push(Message::assistant("I cannot access external websites or your files."));
    ask(&mut app, &cx, "list my files again");
    run_until(&mut app, &cx, |a| !a.is_running());
    let sent = seen.lock().unwrap()[0].clone();
    assert!(!sent.iter().any(|m| m.content.contains("cannot access external")), "the model does not see its old refusal: {sent:?}");
    assert!(sent.iter().any(|m| m.content.contains("wrongly said this could not be done")));
    assert!(app.messages().iter().any(|m| m.content.contains("cannot access external")), "the transcript keeps it");
}

#[test]
fn the_answer_appears_while_it_is_written_and_the_footer_says_where_the_time_went() {
    let (cx, _home, _seen) = rec("stream", vec![], vec![(250, "Chennai is "), (400, "31 degrees "), (300, "today.")]);
    let mut app = agent(&cx);
    ask(&mut app, &cx, "weather in chennai");
    run_until(&mut app, &cx, |a| a.live.contains("31"));
    assert!(app.is_running(), "still writing");
    assert!(!app.messages().iter().any(|m| m.content.contains("31 degrees")), "not in the chat until it is final");
    let g = render(&mut app, &cx);
    assert!(g.hits.iter().any(|h| h.label == "text area"));
    assert!(block_texts(&app).iter().any(|t| t.contains("Chennai is 31")), "the answer so far is drawn in the transcript while it is written: {:?}", block_texts(&app));
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(app.live.is_empty());
    assert_eq!(app.messages().last().unwrap().content, "Chennai is 31 degrees today.");
    let t = app.timing_text();
    assert!(t.starts_with("first token in ") && t.contains(", total "), "{t}");
    let g = render(&mut app, &cx);
    let _ = g; // the footer is drawn with the model name; its text is checked through timing_text
    let meta = cx.services.runs.list("tester").remove(0);
    assert!(meta.total_ms >= meta.model_ms && meta.first_token_ms > 0 && meta.overhead_ms < 200, "the run record keeps the breakdown: {meta:?}");
    let (tok, text, total, model, tools, ours, prep, _think) = app.timing.unwrap();
    assert!(text >= 200 && total >= text && total >= model + tools && ours < 200 && tok <= text, "{:?}", app.timing);
    let _ = prep;
}

#[test]
fn starting_a_run_does_not_wait_for_the_context() {
    let (cx, _home, _seen) = rec("fast", vec![turn("ok", vec![])], vec![]);
    // a long chat, so building the context has real work to do
    let mut app = agent(&cx);
    for i in 0..60 {
        app.msgs.push(Message::user(format!("question {i}")));
        app.msgs.push(Message::assistant(format!("answer {i}")));
    }
    let t = std::time::Instant::now();
    ask(&mut app, &cx, "one more");
    assert!(t.elapsed() < Duration::from_millis(60), "sending took {:?} on the UI thread", t.elapsed());
    run_until(&mut app, &cx, |a| !a.is_running());
}

#[test]
fn a_long_chat_gets_a_rolling_summary_in_the_session_and_in_the_request() {
    let (cx, _home, seen) = rec("summary", vec![turn("ok", vec![])], vec![]);
    let mut app = agent(&cx);
    app.ensure_session(&cx);
    for i in 0..30 {
        app.msgs.push(Message::user(format!("question about topic {i}")));
        app.msgs.push(Message::assistant(format!("answer about topic {i}")));
    }
    ask(&mut app, &cx, "and now?");
    run_until(&mut app, &cx, |a| !a.is_running());
    let sys = seen.lock().unwrap()[0][0].content.clone();
    assert!(sys.contains("Summary of the earlier part of this conversation") && sys.contains("topic 1"), "{sys}");
    assert!(app.session.as_ref().unwrap().summary.contains("topic"), "stored in the session");
    // a short chat has none
    let (cx2, _h, seen2) = rec("nosummary", vec![turn("ok", vec![])], vec![]);
    let mut app2 = agent(&cx2);
    ask(&mut app2, &cx2, "hello");
    run_until(&mut app2, &cx2, |a| !a.is_running());
    assert!(!seen2.lock().unwrap()[0][0].content.contains("Summary of the earlier part"));
}

mod screen {
    use super::*;
    use iyakki_agent::screencap::Frame;

    #[test]
    fn the_assistant_asks_in_the_chat_every_time_and_the_picture_appears() {
        let (cx, _home) = agent_cx("scr", |_| vec![turn("", vec![("s1", "screen_capture", json!({}))]), turn("", vec![("s2", "screen_capture", json!({}))]), turn("Here are two pictures of your screen.", vec![])]);
        cx.services.screen.set_present(true);
        let audit = cx.audit.clone();
        cx.services.browser.set_audit(Arc::new(move |u: &str, e: &str, d: &str| audit.record(u, e, d)));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (svc, s2) = (cx.services.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s2.load(std::sync::atomic::Ordering::SeqCst) {
                if svc.screen.wanted("tester") {
                    svc.screen.serve("tester", || Some(Frame { w: 8, h: 4, rgba: vec![120; 8 * 4 * 4] }));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let mut app = ChatApp::new();
        app.handle(&Event::Key(Key::Ctrl('g')), &cx);
        ask(&mut app, &cx, "take a screenshot of my screen");
        run_until(&mut app, &cx, |a| a.pending_approval().is_some());
        let (_, tool, summary) = app.pending_approval().unwrap().clone();
        assert_eq!(tool, "screen_capture");
        assert_eq!(iyakki_agent::grants::split_card(&summary).question, "Let the assistant take a picture of your IYAKKI screen? It may show private information.");
        let g = render(&mut app, &cx);
        for want in ["Allow once (Ctrl+Y)", "Allow for 5 minutes (Ctrl+U)", "Deny (Ctrl+N)"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        assert!(!g.hits.iter().any(|h| h.label.starts_with("Always allow")));
        // "Allow for 5 minutes": the second capture in the same run is not asked
        let u = g.hits.iter().find(|h| h.label == "Allow for 5 minutes (Ctrl+U)").unwrap().ev.clone();
        app.handle(&u, &cx);
        let mut asked_again = false;
        run_until(&mut app, &cx, |a| {
            asked_again |= a.pending_approval().is_some();
            !a.is_running()
        });
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!asked_again, "covered by the 5 minutes");
        let pics: Vec<&Message> = app.messages().iter().filter(|m| m.role == Role::Tool && m.content.contains("IMAGE: ")).collect();
        assert_eq!(pics.len(), 2, "{:?}", app.messages());
        let path = pics[0].content.rsplit("IMAGE: ").next().unwrap().trim().to_string();
        assert!(std::fs::read(&path).unwrap().starts_with(&[0x89, b'P', b'N', b'G']), "a PNG in the chat's own folder: {path}");
        assert!(cx.audit.recent(40).iter().any(|l| l.contains("screen.capture")), "audited: {:?}", cx.audit.recent(40));
        cx.services.screen.revoke("tester");
    }

    #[test]
    fn a_denied_capture_takes_nothing() {
        let (cx, home) = agent_cx("scrdeny", |_| vec![turn("", vec![("s1", "screen_capture", json!({}))]), turn("OK, I did not.", vec![])]);
        cx.services.screen.set_present(true);
        let mut app = ChatApp::new();
        app.handle(&Event::Key(Key::Ctrl('g')), &cx);
        ask(&mut app, &cx, "screenshot please");
        run_until(&mut app, &cx, |a| a.pending_approval().is_some());
        app.handle(&Event::Key(Key::Ctrl('n')), &cx);
        run_until(&mut app, &cx, |a| !a.is_running());
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        assert!(!cx.services.screen.wanted("tester"));
    }
}

#[test]
fn a_plain_chat_streams_too_and_a_backend_that_cannot_still_works() {
    // streaming
    let (cx, _home, _seen) = rec("plainstream", vec![], vec![(150, "Hello "), (400, "there, "), (300, "friend.")]);
    let mut app = ChatApp::new();
    ask(&mut app, &cx, "hi");
    run_until(&mut app, &cx, |a| a.live.contains("Hello"));
    assert!(app.is_waiting() && !app.messages().iter().any(|m| m.content.contains("Hello")), "partial text is shown but not yet in the chat");
    run_until(&mut app, &cx, |a| !a.is_waiting());
    assert_eq!(app.messages().last().unwrap().content, "Hello there, friend.");
    assert!(app.live.is_empty());
    // a backend with only `complete`
    struct Plain;
    impl ChatBackend for Plain {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Ok("one piece".into())
        }
    }
    let (cx2, _) = cx_with_backend("plainplain", Plain);
    let mut app2 = ChatApp::new();
    ask(&mut app2, &cx2, "hi");
    run_until(&mut app2, &cx2, |a| !a.is_waiting());
    assert_eq!(app2.messages().last().unwrap().content, "one piece");
    // errors still reach the person
    struct Fail;
    impl ChatBackend for Fail {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("HTTP 500: broken".into())
        }
    }
    let (cx3, _) = cx_with_backend("plainfail", Fail);
    let mut app3 = ChatApp::new();
    ask(&mut app3, &cx3, "hi");
    run_until(&mut app3, &cx3, |a| !a.is_waiting());
    assert_eq!(app3.error.as_deref(), Some("HTTP 500: broken"));
}

/// A model that thinks for a while (reporting its thinking as the provider code does), then answers.
struct Thinker {
    think: Vec<(u64, &'static str)>,
    answer: Vec<(u64, &'static str)>,
}

impl ChatBackend for Thinker {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no plain chat".into())
    }
    fn complete_tools_stream(&self, _: &[Message], _: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        for (pause, t) in &self.think {
            std::thread::sleep(Duration::from_millis(*pause));
            iyakki_agent::providers::report_thinking(t);
            on_text("");
        }
        let mut text = String::new();
        for (pause, t) in &self.answer {
            std::thread::sleep(Duration::from_millis(*pause));
            text.push_str(t);
            on_text(t);
        }
        Ok(ModelTurn { text, calls: vec![] })
    }
}

fn thinker(tag: &str) -> AppCx {
    let (mut cx, _d) = cx_with_backend(tag, Thinker { think: vec![(100, "Let me work this out. "), (1200, "17 times 20 is 340. "), (1200, "Plus 17 times 3 is 51. ")], answer: vec![(100, "It is "), (200, "391.")] });
    let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-chat-think-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    cx.session.policy.fs_read = vec![home.display().to_string()];
    cx.session.policy.fs_write = vec![home.display().to_string()];
    cx
}

fn block_texts(app: &ChatApp) -> Vec<String> {
    app.view.layouts().iter().flat_map(|l| l.texts()).collect()
}

fn check_thinking_flow(app: &mut ChatApp, cx: &AppCx, busy: impl Fn(&ChatApp) -> bool) {
    run_until(app, cx, |a| !a.thinking.is_empty());
    assert!(busy(app), "still working");
    render(app, cx);
    assert!(block_texts(app).iter().any(|t| t.starts_with("Thinking")), "a Thinking block is on the screen from the first moment: {:?}", block_texts(app));
    assert!(app.messages().iter().all(|m| !m.content.contains("Let me work this out")), "the thinking is not part of the chat");
    // after a couple of seconds the block counts them
    run_until(app, cx, |a| a.waiting_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(2100)) || !busy(a));
    render(app, cx);
    assert!(block_texts(app).iter().any(|t| t.starts_with("Thinking… ") && t.ends_with(" s")), "{:?}", block_texts(app));
    // the transcript block opens to the thinking
    assert!(app.thinking.contains("17 times 20"), "{}", app.thinking);
}

#[test]
fn agent_mode_shows_a_thinking_block_while_the_model_thinks_then_the_answer() {
    let cx = thinker("thinkagent");
    let mut app = agent(&cx);
    ask(&mut app, &cx, "what is 17*23?");
    check_thinking_flow(&mut app, &cx, |a| a.is_running());
    run_until(&mut app, &cx, |a| !a.is_running());
    assert_eq!(app.messages().last().unwrap().content, "It is 391.");
    assert!(app.thinking.is_empty() && app.live.is_empty(), "the thinking block is gone once the answer is final");
    render(&mut app, &cx);
    assert!(block_texts(&app).iter().any(|t| t.contains("391")), "the final answer renders: {:?}", block_texts(&app));
    assert!(app.timing_text().starts_with("thought "), "the footer says how long it thought: {}", app.timing_text());
    assert!(app.error.is_none());
}

#[test]
fn plain_chat_shows_the_thinking_block_too_and_then_the_answer() {
    let cx = thinker("thinkplain");
    let mut app = ChatApp::new();
    ask(&mut app, &cx, "what is 17*23?");
    check_thinking_flow(&mut app, &cx, |a| a.is_waiting());
    run_until(&mut app, &cx, |a| !a.is_waiting());
    assert_eq!(app.messages().last().unwrap().content, "It is 391.");
    assert!(app.thinking.is_empty() && app.live.is_empty());
    render(&mut app, &cx);
    assert!(block_texts(&app).iter().any(|t| t.contains("391")));
}

#[test]
fn an_empty_answer_is_an_error_message_in_both_modes() {
    struct Empty;
    impl ChatBackend for Empty {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Ok(String::new())
        }
        fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
            Ok(ModelTurn::default())
        }
    }
    let (mut cx, _) = cx_with_backend("emptyans", Empty);
    cx.session.policy.fs_read = vec!["/tmp".into()];
    let mut plain = ChatApp::new();
    ask(&mut plain, &cx, "hi");
    run_until(&mut plain, &cx, |a| !a.is_waiting());
    assert!(plain.error.as_deref().is_some_and(|e| e.contains("empty answer")), "{:?}", plain.error);
    let mut ag = agent(&cx);
    ask(&mut ag, &cx, "hi");
    run_until(&mut ag, &cx, |a| !a.is_running());
    assert!(ag.error.as_deref().is_some_and(|e| e.contains("empty answer")), "{:?}", ag.error);
}
