//! Tests of the assistant's run loop: the capability reminder, the denial guard, repair, the plain-text tool protocol, lazy
//! groups, loops, timeouts, parallel calls, streaming and the latency breakdown. The model is a script; the tools are fakes.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{start_with, Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::assistrun::Assist;
use crate::chat::{ChatBackend, Message, ModelTurn, Role, ToolCall, ToolSpec};
use crate::toolblock::ReminderCtx;

// ------------------------------------------------------------------------------------------------ fakes

struct Scripted {
    turns: Mutex<VecDeque<Result<ModelTurn, String>>>,
    /// Each request: the messages and the names of the tools offered.
    seen: Mutex<Vec<(Vec<Message>, Vec<String>)>>,
    /// Pieces of the answer to stream, with a pause before each (empty = no streaming).
    pieces: Vec<(u64, &'static str)>,
    first_delay: u64,
}

impl Scripted {
    fn new(turns: Vec<Result<ModelTurn, String>>) -> Arc<Scripted> {
        Arc::new(Scripted { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]), pieces: vec![], first_delay: 0 })
    }
    fn streaming(turns: Vec<Result<ModelTurn, String>>, first_delay: u64, pieces: Vec<(u64, &'static str)>) -> Arc<Scripted> {
        Arc::new(Scripted { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]), pieces, first_delay })
    }
    fn requests(&self) -> Vec<(Vec<Message>, Vec<String>)> {
        self.seen.lock().unwrap().clone()
    }
}

impl ChatBackend for Scripted {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("plain chat unused".into())
    }
    fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push((h.to_vec(), t.iter().map(|s| s.name.clone()).collect()));
        if self.first_delay > 0 {
            std::thread::sleep(Duration::from_millis(self.first_delay));
        }
        self.turns.lock().unwrap().pop_front().unwrap_or(Err("script exhausted".into()))
    }
    fn complete_tools_stream(&self, h: &[Message], t: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        if self.pieces.is_empty() {
            return self.complete_tools(h, t);
        }
        self.seen.lock().unwrap().push((h.to_vec(), t.iter().map(|s| s.name.clone()).collect()));
        std::thread::sleep(Duration::from_millis(self.first_delay));
        let mut text = String::new();
        for (pause, piece) in &self.pieces {
            std::thread::sleep(Duration::from_millis(*pause));
            text.push_str(piece);
            if !on_text(piece) {
                break;
            }
        }
        Ok(ModelTurn { text, calls: vec![] })
    }
}

type Handler = Box<dyn Fn(&str, &Value) -> Result<String, String> + Send + Sync>;

struct Reg {
    defs: Vec<ToolDef>,
    delay: Duration,
    ran: Mutex<Vec<String>>,
    out: Handler,
}

fn def(name: &str, props: Value, required: &[&str], risk: Risk) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: format!("{name} does a thing"), schema: json!({"type": "object", "properties": props, "required": required}) }, risk, untrusted: false }
}

fn q() -> Value {
    json!({"query": {"type": "string"}})
}

fn registry(names: &[&str]) -> Vec<ToolDef> {
    names
        .iter()
        .map(|n| match *n {
            "weather" => def(n, json!({"place": {"type": "string"}}), &[], Risk::Safe),
            "stock_quote" => def(n, json!({"symbol": {"type": "string"}}), &["symbol"], Risk::Safe),
            "web_search" | "web_research" | "radio_play" | "radio_search" => def(n, q(), &["query"], Risk::Safe),
            "web_fetch" | "browser_open" => def(n, json!({"url": {"type": "string"}}), &["url"], Risk::Safe),
            "write_file" | "k8s_delete" => def(n, json!({"path": {"type": "string"}}), &["path"], Risk::Ask),
            "run_command" => def(n, json!({"program": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}}), &["program"], Risk::Ask),
            other => def(other, json!({}), &[], Risk::Safe),
        })
        .collect()
}

impl Reg {
    fn new(names: &[&str]) -> Arc<Reg> {
        Arc::new(Reg { defs: registry(names), delay: Duration::ZERO, ran: Mutex::new(vec![]), out: Box::new(|n, a| Ok(format!("{n} result for {a}"))) })
    }
    fn with(names: &[&str], delay_ms: u64, out: Handler) -> Arc<Reg> {
        Arc::new(Reg { defs: registry(names), delay: Duration::from_millis(delay_ms), ran: Mutex::new(vec![]), out })
    }
}

impl Tools for Reg {
    fn defs(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{name} {args}"));
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        (self.out)(name, args)
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        format!("{name} {args}")
    }
}

fn ctx() -> ReminderCtx {
    ReminderCtx { agent_on: true, browser_policy: "interactive".into(), engine_installed: true, screen_available: false, loaded: None }
}

fn std_assist() -> Assist {
    Assist::standard(ctx())
}

fn say(t: &str) -> Result<ModelTurn, String> {
    Ok(ModelTurn { text: t.into(), calls: vec![] })
}

fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: name.into(), args }
}

fn calls(c: Vec<ToolCall>) -> Result<ModelTurn, String> {
    Ok(ModelTurn { text: String::new(), calls: c })
}

fn run(b: Arc<Scripted>, t: Arc<Reg>, ask: &str, assist: Assist) -> Vec<RunEvent> {
    let h = start_with(b, t, vec![Message::system("SYS"), Message::user(ask)], RunLimits::default(), assist);
    drive(&h, |_| Some(Verdict::Approve))
}

fn drive(h: &RunHandle, mut verdict: impl FnMut(&str) -> Option<Verdict>) -> Vec<RunEvent> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match h.events.recv_timeout(Duration::from_millis(50)) {
            Ok(e) => {
                if let RunEvent::Approval { id, name, .. } = &e {
                    if let Some(v) = verdict(name) {
                        h.decide(id, v);
                    }
                }
                let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
                out.push(e);
                if done {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    out
}

fn notes(e: &[RunEvent]) -> Vec<String> {
    e.iter().filter_map(|x| if let RunEvent::Note { text } = x { Some(text.clone()) } else { None }).collect()
}

fn final_text(e: &[RunEvent]) -> String {
    e.iter().rev().find_map(|x| if let RunEvent::Final { text } = x { Some(text.clone()) } else { None }).unwrap_or_else(|| panic!("no final answer in {e:?}"))
}

fn results(e: &[RunEvent]) -> Vec<(String, bool, String)> {
    e.iter().filter_map(|x| if let RunEvent::ToolResult { name, ok, text, .. } = x { Some((name.clone(), *ok, text.clone())) } else { None }).collect()
}

const WEB: &[&str] = &["now", "weather", "stock_quote", "web_search", "web_fetch", "web_research", "read_file"];

// ------------------------------------------------------------------------------------------------ the reminder

#[test]
fn the_reminder_reaches_the_model_near_the_end_and_is_never_shown_or_saved() {
    let b = Scripted::new(vec![say("Chennai is 31 C.")]);
    let ev = run(b.clone(), Reg::new(WEB), "weather in chennai", std_assist());
    let (req, tools) = &b.requests()[0];
    let last_user = req.iter().rev().find(|m| m.role == Role::User).unwrap();
    assert!(last_user.content.starts_with("weather in chennai\n\n[TOOLS AVAILABLE NOW]"), "{}", last_user.content);
    assert!(last_user.content.contains("weather, ") || last_user.content.contains("weather"), "{}", last_user.content);
    assert!(last_user.content.contains("outdated if a tool above does it"));
    assert_eq!(req.last().unwrap().content, last_user.content, "it is the last thing the model reads");
    assert!(!req[0].content.contains("TOOLS AVAILABLE"), "the system message stays stable");
    assert!(tools.contains(&"weather".to_string()));
    // nothing of it in what the person sees or what is recorded
    let json = serde_json::to_string(&ev).unwrap();
    assert!(!json.contains("TOOLS AVAILABLE NOW"), "{json}");
    assert_eq!(final_text(&ev), "Chennai is 31 C.");
    // with the safety net off the request is exactly what was given
    let b2 = Scripted::new(vec![say("x")]);
    run(b2.clone(), Reg::new(WEB), "weather in chennai", Assist::off());
    assert_eq!(b2.requests()[0].0.last().unwrap().content, "weather in chennai");
}

// ------------------------------------------------------------------------------------------------ the denial guard

#[test]
fn a_denial_that_contradicts_the_tools_is_retried_once_before_anything_is_shown() {
    let denial = "I cannot access external websites or services. My capabilities are confined to the tools described earlier.";
    let b = Scripted::new(vec![say(denial), calls(vec![call("c1", "weather", json!({"place": "Chennai"}))]), say("Chennai is 31 C [1].")]);
    let t = Reg::new(WEB);
    let ev = run(b.clone(), t.clone(), "what is the weather in chennai", std_assist());
    assert_eq!(final_text(&ev), "Chennai is 31 C [1].");
    let shown: String = ev.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    assert!(!shown.contains("cannot access external"), "the refusal was never shown: {shown}");
    assert!(notes(&ev)[0].contains("weather"), "{:?}", notes(&ev));
    let reqs = b.requests();
    assert_eq!(reqs.len(), 3);
    let retry = &reqs[1].0;
    assert!(retry.iter().any(|m| m.content.contains("cannot access external")), "the model sees its own refusal…");
    assert!(retry.last().unwrap().content.starts_with("SYSTEM NOTE") && retry.last().unwrap().content.contains("weather"), "…and the note that names the tool");
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    assert!(!reqs[2].0.iter().any(|m| m.content.contains("cannot access external")), "the retry's scaffolding is not kept");
}

#[test]
fn a_second_refusal_is_shown_with_a_note_and_there_is_only_one_retry() {
    let denial = "I cannot take screenshots of my own interface.";
    let b = Scripted::new(vec![say(denial), say(denial)]);
    let ev = run(b.clone(), Reg::new(&["screen_capture", "now"]), "take a screenshot of my screen", std_assist());
    assert_eq!(final_text(&ev), denial);
    assert_eq!(b.requests().len(), 2);
    let n = notes(&ev);
    assert_eq!(n.len(), 2, "{n:?}");
    assert!(n[1].contains("Ctrl+P") && n[1].contains("Ctrl+N"), "{n:?}");
}

#[test]
fn a_true_refusal_is_left_alone() {
    // browser control is off: only browser_status exists, and the model says exactly that
    let honest = "I cannot open it in your browser: browser control is off for your account.";
    let b = Scripted::new(vec![say(honest)]);
    let ev = run(b.clone(), Reg::new(&["browser_status", "now"]), "open gemini in my browser", std_assist());
    assert_eq!(final_text(&ev), honest);
    assert_eq!(b.requests().len(), 1);
    assert!(notes(&ev).is_empty());
    // and with the guard off nothing is retried even for a wrong refusal
    let b = Scripted::new(vec![say("I cannot access external websites.")]);
    let ev = run(b.clone(), Reg::new(WEB), "weather in chennai", Assist::off());
    assert_eq!(b.requests().len(), 1);
    assert!(final_text(&ev).starts_with("I cannot"));
}

#[test]
fn asking_permission_for_a_read_only_lookup_is_sent_back_once() {
    let b = Scripted::new(vec![say("I can help with that. Would you like me to fetch the latest price of envx?"), calls(vec![call("c", "stock_quote", json!({"symbol": "ENVX"}))]), say("ENVX 42.1 [1]")]);
    let ev = run(b.clone(), Reg::new(WEB), "what is the stock price of envx", std_assist());
    assert_eq!(final_text(&ev), "ENVX 42.1 [1]");
    assert!(notes(&ev)[0].contains("permission"), "{:?}", notes(&ev));
    assert!(b.requests()[1].0.last().unwrap().content.contains("do not ask permission"));
    // a write is still asked
    let b = Scripted::new(vec![say("Would you like me to send this email to your boss?")]);
    let ev = run(b.clone(), Reg::new(WEB), "write to my boss", std_assist());
    assert_eq!(b.requests().len(), 1);
    assert!(final_text(&ev).contains("send this email"));
}

// ------------------------------------------------------------------------------------------------ repair

#[test]
fn malformed_calls_are_repaired_before_they_run() {
    let b = Scripted::new(vec![
        calls(vec![call("a", "Web.Search", json!("{'query': 'nifty news',}")), call("b", "stock_quote", json!({"Symbol": 5})), call("c", "weather", json!({"_raw_arguments": "{place: 'Chennai'"}))]),
        say("done"),
    ]);
    let t = Reg::new(WEB);
    let ev = run(b, t.clone(), "news", std_assist());
    assert_eq!(final_text(&ev), "done");
    let ran = t.ran.lock().unwrap().clone();
    assert!(ran.contains(&"web_search {\"query\":\"nifty news\"}".to_string()), "{ran:?}");
    assert!(ran.contains(&"stock_quote {\"symbol\":\"5\"}".to_string()), "{ran:?}");
    assert!(ran.contains(&"weather {\"place\":\"Chennai\"}".to_string()), "{ran:?}");
    assert!(results(&ev).iter().all(|r| r.1), "{:?}", results(&ev));
}

#[test]
fn bad_calls_get_an_error_that_says_what_to_fix() {
    let b = Scripted::new(vec![calls(vec![call("a", "stock_quote", json!({})), call("b", "no_such_tool", json!({}))]), say("sorry")]);
    let ev = run(b.clone(), Reg::new(WEB), "price", std_assist());
    let r = results(&ev);
    assert!(!r[0].1 && r[0].2.contains("missing required argument `symbol`") && r[0].2.contains("stock_quote(symbol: string*)"), "{r:?}");
    assert!(!r[1].1 && r[1].2.contains("unknown tool `no_such_tool`") && r[1].2.contains("Tools that exist:") && r[1].2.contains("weather"), "{r:?}");
    let fed_back = &b.requests()[1].0;
    assert!(fed_back.iter().any(|m| m.role == Role::Tool && m.content.contains("Valid form")));
}

#[test]
fn calls_printed_as_text_run_even_on_a_model_with_native_tools() {
    let text = "Let me check.\n```json\n{\"name\": \"weather\", \"arguments\": {\"place\": \"Chennai\"}}\n```";
    let b = Scripted::new(vec![say(text), say("31 C")]);
    let t = Reg::new(WEB);
    let ev = run(b, t.clone(), "weather in chennai", std_assist());
    assert_eq!(final_text(&ev), "31 C");
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Model { text, calls } if text == "Let me check." && calls.len() == 1)));
}

#[test]
fn a_model_without_tool_support_switches_to_the_text_protocol() {
    let b = Scripted::new(vec![
        Err("HTTP 400: registry.ollama.ai/library/deepseek-r1:1.5b does not support tools".into()),
        say("```tool\n{\"name\": \"weather\", \"arguments\": {\"place\": \"Chennai\"}}\n```"),
        say("It is 31 C in Chennai."),
    ]);
    let t = Reg::new(WEB);
    let ev = run(b.clone(), t.clone(), "weather in chennai", std_assist());
    assert_eq!(final_text(&ev), "It is 31 C in Chennai.");
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    assert!(notes(&ev)[0].contains("plain-text tool protocol"), "{:?}", notes(&ev));
    let reqs = b.requests();
    assert_eq!(reqs.len(), 3);
    assert!(!reqs[0].1.is_empty() && reqs[1].1.is_empty() && reqs[2].1.is_empty(), "tools are not sent natively any more");
    assert!(reqs[1].0[0].content.contains("TOOL PROTOCOL") && reqs[1].0[0].content.contains("weather("), "{}", reqs[1].0[0].content);
    // the result comes back as a user message, and the call as the block the model wrote
    let last = &reqs[2].0;
    assert!(last.iter().all(|m| m.role != Role::Tool && m.tool_calls.is_empty()));
    assert!(last.iter().any(|m| m.role == Role::Assistant && m.content.contains("```tool")));
    assert!(last.iter().any(|m| m.role == Role::User && m.content.starts_with("Result of weather:")));
}

#[test]
fn a_failing_provider_is_reported_not_swallowed() {
    let b = Scripted::new(vec![Err("HTTP 500: model crashed".into())]);
    let ev = run(b, Reg::new(WEB), "hi", std_assist());
    assert!(matches!(ev.last(), Some(RunEvent::Failed { error }) if error.contains("model crashed")), "{ev:?}");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Timing { .. })), "the breakdown is there even for a failed run");
}

#[test]
fn a_blank_reply_is_asked_for_again_once_and_after_tool_work_it_is_answered_from_the_result() {
    // first turn blank: one more try, then the answer
    let b = Scripted::new(vec![say(""), say("here you go")]);
    let ev = run(b.clone(), Reg::new(WEB), "hi", std_assist());
    assert_eq!(final_text(&ev), "here you go", "{ev:?}");
    assert!(notes(&ev).iter().any(|n| n.contains("empty reply")), "{:?}", notes(&ev));
    assert!(b.requests()[1].0.last().unwrap().content.contains("last reply was empty"));
    // blank right after a tool result: the retry points at the result; the answer is shown, no error
    let b = Scripted::new(vec![calls(vec![call("c1", "weather", json!({"place": "Chennai"}))]), say(""), say("It is 31 degrees.")]);
    let ev = run(b.clone(), Reg::new(WEB), "weather?", std_assist());
    assert_eq!(final_text(&ev), "It is 31 degrees.", "{ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Failed { .. })), "{ev:?}");
    assert!(b.requests()[2].0.last().unwrap().content.contains("tool results above"));
    // blank twice after tool work: the last tool output is shown instead of an error under the input
    let b = Scripted::new(vec![calls(vec![call("c1", "weather", json!({"place": "Chennai"}))]), say(""), say("  ")]);
    let ev = run(b, Reg::new(WEB), "weather?", std_assist());
    assert!(final_text(&ev).contains("weather result for"), "{ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Failed { .. })), "{ev:?}");
    // text that came with tool calls, then a blank turn: still no error
    let b = Scripted::new(vec![Ok(ModelTurn { text: "Checking.".into(), calls: vec![call("c1", "weather", json!({}))] }), say(""), say("")]);
    let ev = run(b, Reg::new(WEB), "weather?", std_assist());
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Failed { .. })), "{ev:?}");
    // nothing at all, twice, and no tool work: the error stays
    let b = Scripted::new(vec![say(""), say("")]);
    let ev = run(b, Reg::new(WEB), "hi", std_assist());
    assert!(matches!(ev.last(), Some(RunEvent::Failed { error }) if error.contains("empty answer")), "{ev:?}");
}

#[test]
fn a_call_that_policy_refuses_is_answered_with_the_reason_and_nobody_is_asked() {
    struct Refuse(Mutex<Vec<String>>);
    impl Tools for Refuse {
        fn defs(&self) -> Vec<ToolDef> {
            vec![def("run_command", json!({"program": {"type": "string"}, "args": {"type": "array"}}), &["program"], Risk::Ask)]
        }
        fn call(&self, name: &str, args: &Value) -> Result<String, String> {
            self.0.lock().unwrap().push(format!("{name} {args}"));
            Ok("ran".into())
        }
        fn describe(&self, name: &str, _a: &Value) -> String {
            name.into()
        }
        fn refusal(&self, _n: &str, a: &Value) -> Option<String> {
            (a["program"] == "k3s").then(|| "A cluster is already running and connected. IYAKKI will not start another one here.".to_string())
        }
    }
    let t = Arc::new(Refuse(Mutex::new(vec![])));
    let b = Scripted::new(vec![calls(vec![call("c1", "run_command", json!({"program": "k3s", "args": ["server"]}))]), calls(vec![call("c2", "run_command", json!({"program": "kubectl", "args": ["get", "pods"]}))]), say("used kubectl")]);
    let h = start_with(b.clone(), t.clone(), vec![Message::system("SYS"), Message::user("connect to the cluster")], RunLimits::default(), std_assist());
    let mut asked = vec![];
    let ev = drive(&h, |n| {
        asked.push(n.to_string());
        Some(Verdict::Approve)
    });
    assert_eq!(final_text(&ev), "used kubectl", "{ev:?}");
    assert_eq!(asked, vec!["run_command"], "only the allowed call asked; the refused one did not");
    assert_eq!(t.0.lock().unwrap().len(), 1, "the refused call never ran");
    let r = results(&ev);
    assert!(!r[0].1 && r[0].2.contains("will not start another one"), "{r:?}");
    assert!(b.requests()[1].0.iter().any(|m| m.content.contains("will not start another one")), "the model sees the reason");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Decision { approved: false, by, .. } if by == "policy")));
}

// ------------------------------------------------------------------------------------------------ loops, limits, stop

#[test]
fn the_same_call_with_the_same_answer_three_times_is_broken_and_the_model_must_answer() {
    let same = || calls(vec![call("x", "web_search", json!({"query": "q"}))]);
    let b = Scripted::new(vec![same(), same(), same(), say("Nothing more to find.")]);
    let ev = run(b.clone(), Reg::new(WEB), "look", std_assist());
    assert_eq!(final_text(&ev), "Nothing more to find.");
    let r = results(&ev);
    assert!(r[2].2.starts_with("LOOP:") && !r[2].1, "{r:?}");
    let reqs = b.requests();
    assert!(reqs[3].1.is_empty(), "the answer round offers no tools");
    assert!(reqs[3].0.last().unwrap().content.contains("answer now"));
}

#[test]
fn a_model_that_ignores_the_loop_warning_is_stopped_with_a_message() {
    let same = || calls(vec![call("x", "web_search", json!({"query": "q"}))]);
    let b = Scripted::new((0..8).map(|_| same()).collect());
    let ev = run(b, Reg::new(WEB), "look", std_assist());
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("kept calling web_search")), "{ev:?}");
}

#[test]
fn a_page_that_changes_is_not_a_loop() {
    let same = || calls(vec![call("x", "web_search", json!({"query": "q"}))]);
    let b = Scripted::new(vec![same(), same(), same(), same(), say("ok")]);
    let n = std::sync::atomic::AtomicUsize::new(0);
    let t = Reg::with(WEB, 0, Box::new(move |_, _| Ok(format!("result {}", n.fetch_add(1, Ordering::SeqCst)))));
    let ev = run(b, t, "look", std_assist());
    assert_eq!(final_text(&ev), "ok");
}

#[test]
fn a_stuck_tool_times_out_with_an_error_the_model_can_use() {
    let mut a = std_assist();
    a.tool_timeout_cap = Some(Duration::from_millis(300));
    let b = Scripted::new(vec![calls(vec![call("s", "web_fetch", json!({"url": "https://x"}))]), say("That site is not answering; here is what I know.")]);
    let t = Reg::with(WEB, 5000, Box::new(|_, _| Ok("never".into())));
    let t0 = Instant::now();
    let ev = run(b, t, "fetch", a);
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    let r = results(&ev);
    assert!(!r[0].1 && r[0].2.contains("did not finish within"), "{r:?}");
    assert!(final_text(&ev).contains("not answering"));
}

#[test]
fn stop_answers_at_once_while_a_tool_is_stuck_and_while_the_model_is_thinking() {
    let b = Scripted::new(vec![calls(vec![call("s", "web_fetch", json!({"url": "https://x"}))])]);
    let t = Reg::with(WEB, 10_000, Box::new(|_, _| Ok("never".into())));
    let h = start_with(b, t, vec![Message::system("S"), Message::user("fetch")], RunLimits::default(), std_assist());
    std::thread::sleep(Duration::from_millis(300));
    let t0 = Instant::now();
    h.cancel();
    let ev = drive(&h, |_| None);
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("stopped by the user")), "{ev:?}");
    // the model call itself
    let slow = Scripted::streaming(vec![say("x")], 10_000, vec![]);
    let h = start_with(slow, Reg::new(WEB), vec![Message::system("S"), Message::user("hi")], RunLimits::default(), std_assist());
    std::thread::sleep(Duration::from_millis(200));
    let t0 = Instant::now();
    h.cancel();
    let ev = drive(&h, |_| None);
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { .. })), "{ev:?}");
}

#[test]
fn tool_results_are_cut_to_about_a_thousand_five_hundred_tokens() {
    let b = Scripted::new(vec![calls(vec![call("a", "web_fetch", json!({"url": "https://x"}))]), say("ok")]);
    let t = Reg::with(WEB, 0, Box::new(|_, _| Ok("word ".repeat(20_000))));
    let ev = run(b, t, "fetch", std_assist());
    let r = results(&ev);
    assert!(r[0].2.chars().count() < 6100 && r[0].2.contains("truncated"), "{}", r[0].2.len());
}

#[test]
fn approvals_still_work_with_the_safety_net_and_a_denial_is_final() {
    let b = Scripted::new(vec![calls(vec![call("w", "write_file", json!({"path": "/tmp/x"}))]), say("I could not write it.")]);
    let t = Reg::new(&["write_file", "weather"]);
    let h = start_with(b, t.clone(), vec![Message::system("S"), Message::user("write")], RunLimits::default(), std_assist());
    let ev = drive(&h, |_| Some(Verdict::Deny));
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Approval { name, .. } if name == "write_file")));
    assert!(t.ran.lock().unwrap().is_empty());
    assert!(results(&ev)[0].2.contains("denied"));
}

// ------------------------------------------------------------------------------------------------ lazy groups

#[test]
fn only_the_groups_a_message_suggests_are_offered_and_more_tools_loads_the_rest() {
    let all = &["now", "weather", "web_search", "browser_open", "browser_status", "radio_play", "radio_search", "calendar_add"];
    let b = Scripted::new(vec![calls(vec![call("m", "more_tools", json!({"group": "radio"}))]), calls(vec![call("r", "radio_play", json!({"query": "tamil"}))]), say("Playing.")]);
    let t = Reg::new(all);
    let ev = run(b.clone(), t.clone(), "weather in chennai", std_assist());
    let reqs = b.requests();
    assert!(reqs[0].1.contains(&"weather".to_string()) && reqs[0].1.contains(&"more_tools".to_string()));
    assert!(!reqs[0].1.iter().any(|n| n.starts_with("browser_") || n.starts_with("radio_") || n.starts_with("calendar_")), "{:?}", reqs[0].1);
    assert!(reqs[0].0.last().unwrap().content.contains("radio: radio_play, radio_search (not loaded yet: call more_tools)"), "{}", reqs[0].0.last().unwrap().content);
    assert!(reqs[1].1.contains(&"radio_play".to_string()), "loaded by more_tools");
    assert_eq!(final_text(&ev), "Playing.");
    assert!(results(&ev)[0].2.contains("radio tools are available"));
    // a message that suggests radio gets it at once
    let b = Scripted::new(vec![say("ok")]);
    run(b.clone(), t, "play a tamil fm", std_assist());
    assert!(b.requests()[0].1.contains(&"radio_play".to_string()));
}

#[test]
fn calling_a_tool_that_was_not_offered_loads_its_group_and_runs() {
    let b = Scripted::new(vec![calls(vec![call("c", "calendar_add", json!({}))]), say("added")]);
    let t = Reg::new(&["now", "weather", "calendar_add"]);
    let ev = run(b.clone(), t.clone(), "weather", std_assist());
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    assert_eq!(final_text(&ev), "added");
    assert!(b.requests()[1].1.contains(&"calendar_add".to_string()));
}

// ------------------------------------------------------------------------------------------------ parallel, streaming, timing

#[test]
fn independent_read_only_calls_run_at_the_same_time_and_results_keep_their_order() {
    let b = Scripted::new(vec![
        calls(vec![call("1", "web_fetch", json!({"url": "https://a"})), call("2", "web_fetch", json!({"url": "https://b"})), call("3", "weather", json!({"place": "Chennai"})), call("4", "web_search", json!({"query": "x"}))]),
        say("done"),
    ]);
    let t = Reg::with(WEB, 400, Box::new(|n, a| Ok(format!("{n}:{a}"))));
    let t0 = Instant::now();
    let ev = run(b.clone(), t, "go", std_assist());
    let took = t0.elapsed();
    assert!(took < Duration::from_millis(1000), "four 400 ms tools should overlap, took {took:?}");
    let r = results(&ev);
    assert_eq!(r.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["web_fetch", "web_fetch", "weather", "web_search"]);
    assert!(r[0].2.contains("https://a") && r[1].2.contains("https://b"));
    let second = b.requests();
    let tool_msgs: Vec<&Message> = second[1].0.iter().filter(|m| m.role == Role::Tool).collect();
    assert_eq!(tool_msgs.iter().map(|m| m.tool_call_id.clone().unwrap()).collect::<Vec<_>>(), vec!["1", "2", "3", "4"]);
    // sequentially (safety net off) the same calls take the sum
    let b = Scripted::new(vec![calls(vec![call("1", "web_fetch", json!({"url": "https://a"})), call("2", "web_fetch", json!({"url": "https://b"}))]), say("done")]);
    let t = Reg::with(WEB, 300, Box::new(|n, a| Ok(format!("{n}:{a}"))));
    let t0 = Instant::now();
    run(b, t, "go", Assist::off());
    assert!(t0.elapsed() >= Duration::from_millis(600));
}

#[test]
fn calls_that_change_things_never_run_in_parallel() {
    assert!(crate::assistrun::parallel_safe("web_fetch") && crate::assistrun::parallel_safe("stock_quote"));
    for n in ["write_file", "media_play", "radio_play", "browser_click", "calendar_add", "run_command", "vault_request", "pause_media"] {
        assert!(!crate::assistrun::parallel_safe(n), "{n}");
    }
}

#[test]
fn the_answer_streams_while_it_is_written_and_the_first_text_is_early() {
    let b = Scripted::streaming(vec![say("")], 150, vec![(0, "Chennai "), (200, "is "), (200, "31 "), (200, "degrees.")]);
    let ev = run(b, Reg::new(WEB), "weather", std_assist());
    let deltas: Vec<&String> = ev.iter().filter_map(|e| if let RunEvent::Delta { text } = e { Some(text) } else { None }).collect();
    assert!(deltas.len() >= 3, "{deltas:?}");
    assert!(deltas.windows(2).all(|w| w[1].starts_with(w[0].as_str())), "each update holds the text so far: {deltas:?}");
    let timing = ev.iter().find_map(|e| if let RunEvent::Timing { first_text_ms, total_ms, .. } = e { Some((*first_text_ms, *total_ms)) } else { None }).unwrap();
    assert!(timing.0 >= 100 && timing.0 < 600 && timing.1 > timing.0 + 300, "{timing:?}");
    assert_eq!(final_text(&ev), "Chennai is 31 degrees.");
}

#[test]
fn text_that_starts_like_a_refusal_is_held_back_from_the_live_view() {
    let b = Scripted::streaming(vec![say("")], 0, vec![(0, "I cannot access external websites"), (150, " or services. My capabilities are limited."), (0, "")]);
    let ev = run(b, Reg::new(WEB), "weather in chennai", std_assist());
    let first_note = ev.iter().position(|e| matches!(e, RunEvent::Note { .. })).expect("the guard stepped in");
    assert!(!ev[..first_note].iter().any(|e| matches!(e, RunEvent::Delta { .. })), "nothing of the refusal was shown before the retry: {ev:?}");
}

#[test]
fn the_latency_breakdown_adds_up_and_our_own_overhead_is_small() {
    let b = Scripted::new(vec![calls(vec![call("a", "web_fetch", json!({"url": "https://a"}))]), say("done")]);
    let t = Reg::with(WEB, 120, Box::new(|_, _| Ok("page".into())));
    let ev = run(b, t, "go", std_assist());
    let (first, total, model, tools, overhead, pre, rounds) = ev.iter().find_map(|e| if let RunEvent::Timing { first_token_ms, total_ms, model_ms, tools_ms, overhead_ms, pre_request_ms, rounds, .. } = e { Some((*first_token_ms, *total_ms, *model_ms, *tools_ms, *overhead_ms, *pre_request_ms, *rounds)) } else { None }).unwrap();
    assert_eq!(rounds, 2);
    assert!(tools >= 100 && tools < 400, "tools {tools}");
    assert!(total >= model + tools && total - model - tools == overhead, "{total} {model} {tools} {overhead}");
    assert!(overhead < 400 && pre < 250 && first <= total, "overhead {overhead} ms, before the first request {pre} ms");
    // Timing comes right before the end of the run
    let n = ev.len();
    assert!(matches!(ev[n - 2], RunEvent::Timing { .. }) && matches!(ev[n - 1], RunEvent::Final { .. }));
    // without the safety net no extra events appear
    let b = Scripted::new(vec![say("hi")]);
    let ev = run(b, Reg::new(WEB), "hi", Assist::off());
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Timing { .. } | RunEvent::Delta { .. } | RunEvent::Note { .. })));
}

// ------------------------------------------------------------------------------------------------ screen_capture

mod screen {
    use super::*;
    use crate::agent::UserTools;
    use crate::screencap::Frame;
    use crate::services::Services;
    use iyakki_sec::policy::Role as PolicyRole;

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn rig(tag: &str, with_screen: bool) -> (Arc<UserTools>, Arc<Services>, std::path::PathBuf) {
        let dir = std::fs::canonicalize({
            let d = std::env::temp_dir().join(format!("iyakki-screen-{}-{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("home")).unwrap();
            d
        })
        .unwrap();
        let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        let mut policy = PolicyRole::User.template();
        let home = dir.join("home");
        policy.fs_read = vec![home.display().to_string()];
        policy.fs_write = vec![home.display().to_string()];
        svc.screen.set_present(with_screen);
        let tools = UserTools::new("alice", policy, svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(&home);
        (Arc::new(tools), svc, home)
    }

    /// Stands in for the shell: answers capture requests with a small picture.
    fn shell(svc: Arc<Services>) -> Arc<std::sync::atomic::AtomicBool> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                if svc.screen.wanted("alice") {
                    svc.screen.serve("alice", || Some(Frame { w: 4, h: 2, rgba: vec![200; 4 * 2 * 4] }));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        stop
    }

    #[test]
    fn the_tool_is_offered_only_when_a_screen_exists_and_asks_every_time() {
        let (t, svc, _) = rig("offer", false);
        assert!(!t.defs().iter().any(|d| d.spec.name == "screen_capture"));
        svc.screen.set_present(true);
        let d = t.defs().into_iter().find(|d| d.spec.name == "screen_capture").expect("offered with a screen");
        assert_eq!(d.risk, Risk::Ask);
        assert_eq!(t.describe("screen_capture", &json!({})), "Let the assistant take a picture of your IYAKKI screen? It may show private information.");
    }

    #[test]
    fn an_approved_capture_shows_the_picture_in_the_chat_and_is_audited() {
        let (t, svc, home) = rig("capture", true);
        let log: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
        let l2 = log.clone();
        svc.browser.set_audit(Arc::new(move |_: &str, e: &str, d: &str| l2.lock().unwrap().push((e.to_string(), d.to_string()))));
        let stop = shell(svc.clone());
        let b = Scripted::new(vec![calls(vec![call("s", "screen_capture", json!({}))]), say("Here is your screen.")]);
        let h = start_with(b, t.clone(), vec![Message::system("S"), Message::user("take a screenshot")], RunLimits::default(), Assist::standard(ReminderCtx { screen_available: true, ..ctx() }));
        let mut asked = Vec::new();
        let ev = drive(&h, |name| {
            asked.push(name.to_string());
            Some(Verdict::Approve)
        });
        stop.store(true, Ordering::SeqCst);
        assert_eq!(asked, vec!["screen_capture"], "the person was asked");
        let r = results(&ev);
        assert!(r[0].1 && r[0].2.contains("IMAGE: ") && r[0].2.contains("screen-"), "{r:?}");
        let path = r[0].2.rsplit("IMAGE: ").next().unwrap().trim().to_string();
        assert!(path.starts_with(&home.display().to_string()));
        let png = std::fs::read(&path).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(log.lock().unwrap().iter().any(|(e, d)| e == "screen.capture" && d == "4x2"), "{:?}", log.lock().unwrap());
        // denied: nothing is captured and nothing is saved
        let (t2, svc2, home2) = rig("denied", true);
        let stop = shell(svc2);
        let b = Scripted::new(vec![calls(vec![call("s", "screen_capture", json!({}))]), say("I did not take it.")]);
        let h = start_with(b, t2, vec![Message::system("S"), Message::user("take a screenshot")], RunLimits::default(), std_assist());
        let ev = drive(&h, |_| Some(Verdict::Deny));
        stop.store(true, Ordering::SeqCst);
        assert!(results(&ev)[0].2.contains("denied"));
        assert_eq!(std::fs::read_dir(&home2).unwrap().count(), 0);
    }

    #[test]
    fn allow_for_five_minutes_skips_the_question_for_that_long_only() {
        let (t, svc, _) = rig("grant", true);
        let stop = shell(svc.clone());
        let b = Scripted::new(vec![
            calls(vec![call("s1", "screen_capture", json!({}))]),
            calls(vec![call("s2", "screen_capture", json!({}))]),
            say("two pictures"),
        ]);
        let h = start_with(b, t.clone(), vec![Message::system("S"), Message::user("screenshots")], RunLimits::default(), std_assist());
        let mut questions = 0;
        let ev = drive(&h, |_| {
            questions += 1;
            Some(Verdict::Always)
        });
        assert_eq!(questions, 1, "asked once; the second capture is covered by the 5 minutes");
        assert!(ev.iter().any(|e| matches!(e, RunEvent::Decision { by, .. } if by == "grant")), "{ev:?}");
        assert_eq!(results(&ev).iter().filter(|r| r.1).count(), 2);
        assert!(svc.screen.granted("alice"));
        // when the time is up it asks again
        svc.screen.revoke("alice");
        assert!(!t.pre_approved("screen_capture"));
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn without_a_shell_answering_the_error_is_clear() {
        let (t, svc, _) = rig("nobody", true);
        svc.screen.set_wait_ms(150);
        let e = t.call("screen_capture", &json!({}));
        assert!(e.unwrap_err().contains("did not answer"), "a screen that never answers is reported");
    }
}

// ------------------------------------------------------------------------------------------------ thinking models
//
// The streams below were recorded from a real Ollama server (qwen3:8b and deepseek-r1:latest, `/v1/chat/completions`,
// `stream: true`); only the middle of the long thinking phase was cut out. They drive the real provider parser and the real
// run loop.

mod thinking {
    use super::*;
    use crate::providers::{ProviderCfg, ProviderClient};
    use crate::vault::Secrets;
    use std::io::Read;

    const QWEN_THINKS_THEN_ANSWERS: &str = include_str!("fixtures/qwen3_thinking_then_content.sse");
    const QWEN_THINKS_THEN_NOTHING: &str = include_str!("fixtures/qwen3_thinking_then_tool_call.sse");
    const DEEPSEEK_REASONING_FIELD: &str = include_str!("fixtures/deepseek_r1_reasoning_field.sse");
    const NO_THINKING_400: &str = r#"{"error":{"message":"\"qwen2.5-coder:7b\" does not support thinking","type":"invalid_request_error","param":null,"code":null}}"#;

    /// Bytes arrive in small pieces with a pause, like a server that is still writing.
    struct Trickle {
        data: std::io::Cursor<Vec<u8>>,
        pause: Duration,
    }
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.pause);
            let n = buf.len().min(900);
            self.data.read(&mut buf[..n])
        }
    }

    struct Seq {
        bodies: Mutex<VecDeque<(u16, String)>>,
        sent: Mutex<Vec<Value>>,
    }
    impl iyakki_proxy::Streamer for Seq {
        fn open(&self, _: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
            Err("no GET".into())
        }
        fn post(&self, _: &str, _: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
            self.sent.lock().unwrap().push(serde_json::from_slice(body).unwrap());
            let (status, text) = self.bodies.lock().unwrap().pop_front().ok_or("no more recorded answers")?;
            Ok(iyakki_proxy::StreamHead { status, headers: vec![], body: Box::new(Trickle { data: std::io::Cursor::new(text.into_bytes()), pause: Duration::from_millis(12) }) })
        }
    }

    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    /// The real provider code over recorded answers.
    struct Replay {
        client: ProviderClient,
        model: &'static str,
        st: Arc<Seq>,
    }
    impl ChatBackend for Replay {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("plain chat unused".into())
        }
        fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
            self.client.chat_turn(self.model, h, t)
        }
        fn complete_tools_stream(&self, h: &[Message], t: &[ToolSpec], on: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
            self.client.chat_turn_stream(self.model, h, t, self.st.as_ref(), on)
        }
    }

    fn replay(model: &'static str, bodies: Vec<(u16, String)>) -> (Arc<Replay>, Arc<Seq>) {
        let st = Arc::new(Seq { bodies: Mutex::new(bodies.into()), sent: Mutex::new(vec![]) });
        let cfg: ProviderCfg = serde_json::from_value(json!({"id": "p", "kind": "ollama", "base_url": "http://ollama.test:11434"})).unwrap();
        let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(Dead);
        (Arc::new(Replay { client: ProviderClient::new(cfg, Secrets::new(None, up.clone()), up), model, st: st.clone() }), st)
    }

    fn run_replay(b: Arc<Replay>, ask: &str, assist: Assist) -> Vec<RunEvent> {
        let h = start_with(b, Reg::new(WEB), vec![Message::system("SYS"), Message::user(ask)], RunLimits::default(), assist);
        drive(&h, |_| Some(Verdict::Approve))
    }

    fn assist(fast: bool) -> Assist {
        let mut a = std_assist();
        a.fast_reasoning = fast;
        a
    }

    fn thinking_events(e: &[RunEvent]) -> Vec<(String, u64)> {
        e.iter().filter_map(|x| if let RunEvent::Thinking { text, secs } = x { Some((text.clone(), *secs)) } else { None }).collect()
    }

    #[test]
    fn a_model_that_thinks_for_a_long_time_shows_its_thinking_and_then_the_answer() {
        let (b, st) = replay("qwen3:8b", vec![(200, QWEN_THINKS_THEN_ANSWERS.into())]);
        let ev = run_replay(b, "What is 17*23? Answer briefly.", assist(false));
        let th = thinking_events(&ev);
        assert!(!th.is_empty(), "the thinking reaches the screen: {ev:?}");
        assert!(th[0].0.starts_with("Okay") || th.last().unwrap().0.contains("17"), "{:?}", th.last());
        assert!(th.windows(2).all(|w| w[1].0.starts_with(&w[0].0)), "each update holds the thinking so far");
        let first_think = ev.iter().position(|e| matches!(e, RunEvent::Thinking { .. })).unwrap();
        let first_text = ev.iter().position(|e| matches!(e, RunEvent::Delta { .. })).expect("the answer streams");
        assert!(first_think < first_text, "thinking comes first");
        assert!(final_text(&ev).contains("391"), "{}", final_text(&ev));
        assert!(!final_text(&ev).contains("Okay, the user"), "the thinking is not part of the answer");
        let (think_ms, total) = ev.iter().find_map(|e| if let RunEvent::Timing { thinking_ms, total_ms, .. } = e { Some((*thinking_ms, *total_ms)) } else { None }).unwrap();
        assert!(think_ms >= 50 && think_ms <= total, "thinking_ms {think_ms}, total {total}");
        let sent = st.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["stream"], json!(true));
        assert!(sent[0].get("reasoning_effort").is_none(), "thinking was left on");
        // thinking is never recorded or sent back
        let json = serde_json::to_string(&ev.iter().filter(|e| !matches!(e, RunEvent::Thinking { .. })).collect::<Vec<_>>()).unwrap();
        assert!(!json.contains("Okay, the user"), "{json}");
    }

    #[test]
    fn thinking_that_ends_without_an_answer_is_asked_again_without_thinking() {
        // the real stream: qwen3:8b with a tool offered thought about the call and then wrote nothing at all
        let (b, st) = replay("qwen3:8b", vec![(200, QWEN_THINKS_THEN_NOTHING.into()), (200, QWEN_THINKS_THEN_ANSWERS.replace("reasoning", "ignored")), (200, String::new())]);
        let ev = run_replay(b, "what is the weather in chennai", assist(false));
        assert!(notes(&ev).iter().any(|n| n.contains("finished thinking without answering")), "{:?}", notes(&ev));
        assert!(final_text(&ev).contains("391"), "the second try answered: {}", final_text(&ev));
        let sent = st.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].get("reasoning_effort").is_none());
        assert_eq!(sent[1]["reasoning_effort"], json!("none"), "the retry turns thinking off");
        assert!(!ev.iter().any(|e| matches!(e, RunEvent::Failed { .. })));
    }

    #[test]
    fn an_answer_that_exists_only_as_thinking_is_shown_with_a_note_not_as_empty() {
        let (b, _) = replay("qwen3:8b", vec![(200, QWEN_THINKS_THEN_NOTHING.into())]);
        let ev = run_replay(b, "what is the weather in chennai", assist(true));
        assert!(final_text(&ev).starts_with("Okay, the user is asking about the weather in Chennai"), "{}", final_text(&ev));
        assert!(notes(&ev).iter().any(|n| n.contains("only its thinking")), "{:?}", notes(&ev));
    }

    #[test]
    fn deepseek_thinking_in_the_reasoning_field_and_inline_think_tags_both_work() {
        // as recorded: the thinking arrives in `reasoning`
        let (b, _) = replay("deepseek-r1:latest", vec![(200, DEEPSEEK_REASONING_FIELD.into())]);
        let ev = run_replay(b, "What is 17*23? Answer briefly.", assist(false));
        assert!(!thinking_events(&ev).is_empty());
        assert!(final_text(&ev).contains("391"), "{}", final_text(&ev));
        // the same stream with the thinking written inline between <think> tags (older servers and some models do this)
        let mut inline = String::new();
        let mut open = false;
        for l in DEEPSEEK_REASONING_FIELD.lines().filter(|l| l.starts_with("data: {")) {
            let mut v: Value = serde_json::from_str(&l[5..]).unwrap();
            let Some(d) = v.pointer_mut("/choices/0/delta") else {
                inline.push_str(&format!("{l}\n\n"));
                continue;
            };
            let r = d.get("reasoning").and_then(Value::as_str).unwrap_or("").to_string();
            let c = d.get("content").and_then(Value::as_str).unwrap_or("").to_string();
            let mut content = String::new();
            if !r.is_empty() {
                if !open {
                    content.push_str("<think>");
                    open = true;
                }
                content.push_str(&r);
            }
            if !c.is_empty() {
                if open {
                    content.push_str("</think>");
                    open = false;
                }
                content.push_str(&c);
            }
            d.as_object_mut().unwrap().remove("reasoning");
            d["content"] = json!(content);
            inline.push_str(&format!("data: {v}\n\n"));
        }
        inline.push_str("data: [DONE]\n\n");
        let (b, _) = replay("deepseek-r1:latest", vec![(200, inline)]);
        let ev = run_replay(b, "What is 17*23? Answer briefly.", assist(false));
        let th = thinking_events(&ev);
        assert!(!th.is_empty() && th.last().unwrap().0.len() > 20, "the thinking between the tags is shown as thinking: {th:?}");
        assert!(!final_text(&ev).contains("<think>") && !final_text(&ev).contains("</think>") && final_text(&ev).contains("391"), "{}", final_text(&ev));
        assert!(!ev.iter().any(|e| matches!(e, RunEvent::Delta { text } if text.contains("think>"))), "no tag ever reaches the live view");
    }

    #[test]
    fn a_model_that_cannot_think_gets_the_switch_taken_away_and_the_question_asked_again() {
        let (b, st) = replay("qwen2.5-coder:7b", vec![(400, NO_THINKING_400.into()), (200, QWEN_THINKS_THEN_ANSWERS.replace("reasoning", "ignored"))]);
        let ev = run_replay(b.clone(), "What is 17*23? Answer briefly.", assist(true));
        assert!(final_text(&ev).contains("391"), "{ev:?}");
        {
            let sent = st.sent.lock().unwrap();
            assert_eq!(sent.len(), 2, "one refusal, one retry");
            assert_eq!(sent[0]["reasoning_effort"], json!("none"));
            assert!(sent[1].get("reasoning_effort").is_none(), "the retry has no thinking switch");
        }
        // and it is remembered: the next question does not even try
        st.bodies.lock().unwrap().push_back((200, QWEN_THINKS_THEN_ANSWERS.replace("reasoning", "ignored")));
        let ev = run_replay(b, "again", assist(true));
        assert!(final_text(&ev).contains("391"));
        assert!(st.sent.lock().unwrap()[2].get("reasoning_effort").is_none());
        assert!(!ev.iter().any(|e| matches!(e, RunEvent::Failed { .. })));
    }

    #[test]
    fn a_server_error_and_an_empty_stream_are_messages_in_the_chat_not_silence() {
        let (b, _) = replay("qwen3:8b", vec![(500, r#"{"error":{"message":"model runner has unexpectedly stopped"}}"#.into())]);
        let ev = run_replay(b, "hi", assist(false));
        assert!(matches!(ev.last(), Some(RunEvent::Failed { error }) if error.contains("model runner has unexpectedly stopped")), "{ev:?}");
        let (b, _) = replay("qwen3:8b", vec![(200, "data: [DONE]\n\n".into()), (200, "data: [DONE]\n\n".into())]);
        let ev = run_replay(b, "hi", assist(false));
        assert!(matches!(ev.last(), Some(RunEvent::Failed { error }) if error.contains("empty answer")), "{ev:?}");
    }

    #[test]
    fn stop_works_while_the_model_is_thinking() {
        let (b, _) = replay("qwen3:8b", vec![(200, QWEN_THINKS_THEN_ANSWERS.into())]);
        let h = start_with(b, Reg::new(WEB), vec![Message::system("S"), Message::user("q")], RunLimits::default(), assist(false));
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        h.cancel();
        let ev = drive(&h, |_| None);
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { .. })), "{ev:?}");
    }
}

// ------------------------------------------------------------------------------------------------ approval mode Auto

#[test]
fn auto_mode_runs_writes_without_asking_but_destructive_secret_and_outside_calls_still_ask() {
    let ask = |n: &str| def(n, json!({}), &[], Risk::Ask);
    let reg = || Arc::new(Reg { defs: vec![ask("write_file"), ask("deploy_build"), ask("k8s_delete"), ask("run_command"), ask("screen_capture"), ask("mcp__x__y")], delay: Duration::ZERO, ran: Mutex::new(vec![]), out: Box::new(|n, _| Ok(format!("{n} ok"))) });
    let script = || {
        Scripted::new(vec![
            calls(vec![call("a", "write_file", json!({"path": "app.js"})), call("b", "deploy_build", json!({})), call("c", "run_command", json!({"program": "npm", "args": ["install"]}))]),
            calls(vec![call("d", "k8s_delete", json!({"kind": "pods", "name": "x"})), call("e", "run_command", json!({"program": "rm", "args": ["-rf", "node_modules"]})), call("f", "run_command", json!({"program": "cat", "args": ["/etc/passwd"]})), call("g", "screen_capture", json!({})), call("h", "mcp__x__y", json!({}))]),
            say("done"),
        ])
    };
    // Auto
    let t = reg();
    let mut asked = vec![];
    let limits = RunLimits { auto: true, ..Default::default() };
    let h = start_with(script(), t.clone(), vec![Message::system("SYS"), Message::user("go")], limits, std_assist());
    let ev = drive(&h, |n| {
        asked.push(n.to_string());
        Some(Verdict::Deny)
    });
    assert_eq!(final_text(&ev), "done", "{ev:?}");
    assert_eq!(asked, vec!["k8s_delete", "run_command", "run_command", "screen_capture", "mcp__x__y"], "only the hard limits ask");
    let by: Vec<String> = ev.iter().filter_map(|e| if let RunEvent::Decision { id, approved: true, by, .. } = e { Some(format!("{id}:{by}")) } else { None }).collect();
    assert_eq!(by, vec!["a:auto-approved", "b:auto-approved", "c:auto-approved"], "the audit row says auto");
    assert_eq!(t.ran.lock().unwrap().len(), 3, "denied calls never ran");
    // Ask (default): everything asks
    let mut asked = vec![];
    let h = start_with(script(), reg(), vec![Message::system("SYS"), Message::user("go")], RunLimits::default(), std_assist());
    drive(&h, |n| {
        asked.push(n.to_string());
        Some(Verdict::Deny)
    });
    assert_eq!(asked.len(), 8);
}

#[test]
fn hard_ask_lists_exactly_what_auto_never_runs() {
    use crate::agent::hard_ask;
    assert!(hard_ask("write_file", &json!({})).is_none() && hard_ask("k8s_apply", &json!({})).is_none() && hard_ask("deploy_load", &json!({})).is_none());
    assert!(hard_ask("k8s_delete", &json!({})).is_some() && hard_ask("vault_request", &json!({})).is_some());
    assert!(hard_ask("run_command", &json!({"program": "kubectl", "args": ["delete", "ns", "x"]})).is_some());
    assert!(hard_ask("run_command", &json!({"program": "podman", "args": ["system", "prune"]})).is_some());
    assert!(hard_ask("run_command", &json!({"program": "ls", "args": ["../other"]})).is_some());
    assert!(hard_ask("run_command", &json!({"program": "node", "args": ["app.js"]})).is_none());
    assert!(hard_ask("http_request", &json!({"method": "DELETE"})).is_some() && hard_ask("http_request", &json!({"method": "GET"})).is_none());
}

// ------------------------------------------------------------------------------------------------ the stall guard

const STALL: &str = "It seems there is an issue with the directory structure. Let me try a different approach to ensure the directory exists before writing the file.";

fn failing_writer() -> Arc<Reg> {
    // the first write fails like the user's did, later ones work
    let n = Mutex::new(0);
    Reg::with(&["write_file", "now"], 0, Box::new(move |_, _| {
        let mut n = n.lock().unwrap();
        *n += 1;
        if *n == 1 { Err("cannot write /w/deploy/registry.yaml: /w/deploy is a file, not a folder".into()) } else { Ok("wrote 5 bytes".into()) }
    }))
}

fn nudged(b: &Scripted) -> usize {
    b.requests().iter().filter(|(m, _)| m.last().is_some_and(|x| x.content.starts_with("SYSTEM NOTE: you said you would act"))).count()
}

#[test]
fn a_stall_after_a_failed_tool_is_nudged_once_and_recovers_into_a_tool_call() {
    let w = |id: &str| calls(vec![call(id, "write_file", json!({"path": "deploy/registry.yaml"}))]);
    let b = Scripted::new(vec![w("c1"), say(STALL), w("c2"), say("Wrote the manifest.")]);
    let t = failing_writer();
    let ev = run(b.clone(), t.clone(), "write the registry manifest", std_assist());
    assert_eq!(final_text(&ev), "Wrote the manifest.");
    assert_eq!(t.ran.lock().unwrap().len(), 2, "the nudge led to a second call");
    assert_eq!(nudged(&b), 1);
    assert!(notes(&ev).iter().any(|n| n.contains("continuing")), "{:?}", notes(&ev));
    // the nudge is request-only: never a user message in the saved/shown events
    let shown: String = ev.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    assert!(!shown.contains("you said you would act"), "{shown}");
}

#[test]
fn a_genuine_answer_or_a_question_is_not_nudged() {
    for text in ["Wrote the manifest. Everything is done.", "Which folder should the manifest go in?", "I cannot write there: /w/deploy is a file. I need you to rename it."] {
        let b = Scripted::new(vec![calls(vec![call("c1", "write_file", json!({"path": "x"}))]), say(text)]);
        let ev = run(b.clone(), failing_writer(), "write it", std_assist());
        assert_eq!(final_text(&ev), text);
        assert_eq!(b.requests().len(), 2, "{text}");
    }
}

#[test]
fn at_most_two_nudges_per_run() {
    let b = Scripted::new(vec![calls(vec![call("c1", "write_file", json!({"path": "x"}))]), say(STALL), say(STALL), say(STALL), say("never")]);
    let ev = run(b.clone(), failing_writer(), "write it", std_assist());
    assert_eq!(nudged(&b), 2);
    assert_eq!(b.requests().len(), 4);
    assert_eq!(final_text(&ev), STALL, "the third stall is shown as the answer");
}

#[test]
fn no_nudge_after_a_denial_and_none_when_the_guard_is_off() {
    let b = Scripted::new(vec![calls(vec![call("c1", "write_file", json!({"path": "x"}))]), say(STALL)]);
    let h = start_with(b.clone(), failing_writer(), vec![Message::system("SYS"), Message::user("write it")], RunLimits::default(), std_assist());
    let ev = drive(&h, |_| Some(Verdict::Deny));
    assert_eq!(final_text(&ev), STALL);
    assert_eq!(nudged(&b), 0);
    let b = Scripted::new(vec![calls(vec![call("c1", "write_file", json!({"path": "x"}))]), say(STALL)]);
    run(b.clone(), failing_writer(), "write it", Assist::off());
    assert_eq!(b.requests().len(), 2);
}

#[test]
fn no_nudge_while_the_same_call_keeps_failing() {
    let w = |id: &str| calls(vec![call(id, "write_file", json!({"path": "deploy/x"}))]);
    let t = Reg::with(&["write_file"], 0, Box::new(|_, _| Err("cannot write: /w/deploy is a file, not a folder".into())));
    let b = Scripted::new(vec![w("c1"), w("c2"), w("c3"), say(STALL)]);
    let ev = run(b.clone(), t, "write it", std_assist());
    assert_eq!(nudged(&b), 0, "three identical failures: the model must explain, not be pushed again");
    assert!(final_text(&ev).contains("different approach"));
}

// ------------------------------------------------------------------------------------------------ the board in a workspace chat

#[test]
fn a_workspace_chat_plans_a_build_on_the_board_which_is_created_by_the_first_row() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-board-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let features = root.join("FEATURES.md");
    assert!(!features.exists());
    let inner = Reg::new(&["read_file", "write_file", "list_dir", "run_command", "now"]);
    let tools = Arc::new(crate::boardtools::ProjectTools::new(inner.clone(), &root, "Web", &features));
    let add = |id: &str, title: &str| calls(vec![call(id, "board_add", json!({"prefix": "APP", "title": title}))]);
    let b = Scripted::new(vec![
        add("a1", "Write server.js with an HTTP server on port 3000"),
        add("a2", "Add a Dockerfile"),
        calls(vec![call("w1", "write_file", json!({"path": "server.js"}))]),
        calls(vec![call("u1", "board_update", json!({"id": "F-APP-01", "status": "partial", "comment": "written, not run yet"}))]),
        say("server.js is written; the board shows what is left."),
        say("The Dockerfile row stays planned; the board has it."),
    ]);
    let h = start_with(b.clone(), tools, vec![Message::system(format!("SYS{}", crate::boardtools::workspace_prompt("Web", &root, "FEATURES.md"))), Message::user("create a simple nodejs webserver")], RunLimits::default(), std_assist());
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(final_text(&ev).contains("board"), "{ev:?}");
    // the board tools were offered to the model in the first request, with lazy groups on
    let (req, offered) = &b.requests()[0];
    assert!(offered.contains(&"board_add".to_string()) && offered.contains(&"board_update".to_string()) && offered.contains(&"board_list".to_string()), "{offered:?}");
    assert!(req[0].content.contains("FIRST call board_add") && req[0].content.contains("Never mark a row done unless"), "the prompt tells it to plan on the board");
    let text = std::fs::read_to_string(&features).expect("the first board_add created the file");
    assert!(text.contains("| ID | Feature | Status | Tests | Started | Finished | Notes |"), "{text}");
    assert!(text.contains("F-APP-01") && text.contains("F-APP-02") && text.contains("Dockerfile"), "{text}");
    assert!(text.lines().any(|l| l.contains("F-APP-01") && l.contains("partial") && l.contains("written, not run yet")), "{text}");
    assert!(text.lines().any(|l| l.contains("F-APP-02") && l.contains("planned")), "{text}");
    assert!(text.contains("planned 1") && text.contains("partial 1") && text.contains("total 2"), "totals kept in step: {text}");
    // reading a workspace without a board says how to create it
    let none = root.join("other");
    std::fs::create_dir_all(&none).unwrap();
    let t2 = crate::boardtools::ProjectTools::new(inner, &none, "Other", &none.join("FEATURES.md"));
    assert!(Tools::call(&t2, "board_list", &json!({})).unwrap_err().contains("board_add"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ask_user_ends_the_turn_with_the_question_and_its_numbered_options_and_the_model_is_not_called_again() {
    let reg = Arc::new(Reg {
        defs: vec![def("ask_user", json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}), &["question"], Risk::Safe)],
        delay: Duration::ZERO,
        ran: Mutex::new(vec![]),
        out: Box::new(|_, _| Ok("shown".into())),
    });
    let b = Scripted::new(vec![calls(vec![call("a1", "ask_user", json!({"question": "Which registry?", "options": ["Docker Hub", "GHCR"]}))]), say("this must never be asked for")]);
    let ev = run(b.clone(), reg.clone(), "deploy my app", std_assist());
    assert_eq!(final_text(&ev), "Which registry?\n\n1. Docker Hub\n2. GHCR");
    assert_eq!(b.requests().len(), 1, "no second model call after the question");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::ToolResult { name, ok: true, .. } if name == "ask_user")));
    assert_eq!(reg.ran.lock().unwrap().len(), 1);
    // no approval was needed (a Safe tool)
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Approval { .. })));
}

// ------------------------------------------------------------------------------------------------ the board tracker

const NODE_TOOLS: &[&str] = &["read_file", "write_file", "deploy_build", "deploy_load", "k8s_apply", "deploy_status", "run_command"];

fn board_world(tag: &str, load_fails: bool) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<Reg>, Arc<Mutex<Vec<String>>>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-track-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // a row the person wrote by hand
    std::fs::write(root.join("FEATURES.md"), format!("{}| H-01 | Build the docs by hand | planned | — |  |  | mine |\n", crate::featureboard::EMPTY_BOARD)).unwrap();
    let inner = Reg::with(NODE_TOOLS, 0, Box::new(move |n, _| match n {
        "deploy_load" if load_fails => Err("push to 192.168.1.5:30500 failed: connection refused\nsecond line".into()),
        "deploy_status" => Ok("Deployment web: 1/1 ready, Healthy, images localhost/web:abc\nPod web-1: Running ready true restarts 0".into()),
        other => Ok(format!("{other} ok")),
    }));
    let audit = Arc::new(Mutex::new(Vec::new()));
    let a2 = audit.clone();
    let tools = Arc::new(crate::boardtools::ProjectTools::new(inner.clone(), &root, "Web", &root.join("FEATURES.md")).with_audit(Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k}: {d}")))));
    (root, tools, inner, audit)
}

fn row_status(root: &std::path::Path, id: &str) -> (String, String) {
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    let f = b.get(id).unwrap_or_else(|| panic!("no row {id}"));
    (f.status.clone(), f.notes.clone())
}

fn node_plan() -> Vec<Result<ModelTurn, String>> {
    let add = |n: usize, t: &str| calls(vec![call(&format!("a{n}"), "board_add", json!({"prefix": "APP", "title": t}))]);
    vec![
        add(1, "Write server.js with an HTTP server on port 3000"),
        add(2, "Add a Dockerfile"),
        add(3, "Build the container image"),
        add(4, "Load the image into the registry"),
        add(5, "Deploy the manifest to k3s"),
        add(6, "Verify the deployment is running"),
        calls(vec![call("w1", "write_file", json!({"path": "server.js"}))]),
        calls(vec![call("w2", "write_file", json!({"path": "Dockerfile"}))]),
        calls(vec![call("b", "deploy_build", json!({"dir": ".", "image": "web"}))]),
        calls(vec![call("l", "deploy_load", json!({"image": "localhost/web:abc"}))]),
        calls(vec![call("k", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        calls(vec![call("s", "deploy_status", json!({"name": "web"}))]),
    ]
}

fn run_board(b: Arc<Scripted>, tools: Arc<crate::boardtools::ProjectTools>) -> Vec<RunEvent> {
    let h = start_with(b, tools, vec![Message::system("SYS"), Message::user("create a simple nodejs webserver and deploy it")], RunLimits { max_steps: 40, max_tool_calls: 40, ..RunLimits::default() }, std_assist());
    drive(&h, |_| Some(Verdict::Approve))
}

#[test]
fn the_tracker_moves_the_rows_the_assistant_planned_without_the_model_updating_them() {
    let (root, tools, _inner, audit) = board_world("node", false);
    let mut turns = node_plan();
    turns.push(say("Done: server.js, the Dockerfile and the deployment are in place."));
    let ev = run_board(Scripted::new(turns), tools);
    assert!(final_text(&ev).starts_with("Done"));
    for n in 1..=6 {
        let (st, notes) = row_status(&root, &format!("F-APP-0{n}"));
        assert_eq!(st, "done", "F-APP-0{n}: {notes}");
    }
    assert_eq!(row_status(&root, "H-01"), ("planned".to_string(), "mine".to_string()), "a row the person wrote is never touched");
    let text = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
    let f = crate::featureboard::Board::parse(&text).get("F-APP-03").unwrap();
    assert!(!f.started.is_empty() && !f.finished.is_empty(), "times are set: {text}");
    assert!(text.contains("done 6") && text.contains("planned 1"), "totals follow: {text}");
    let a = audit.lock().unwrap().join("\n");
    assert!(a.contains("board.auto: F-APP-03 planned -> partial (deploy_build)") && a.contains("F-APP-06"), "{a}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("checking the board"))), "nothing was left open");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_load_leaves_the_row_partial_with_a_note_and_later_rows_wait() {
    let (root, tools, _inner, _a) = board_world("fail", true);
    let mut turns = node_plan();
    turns.truncate(10); // up to and including the failed deploy_load
    turns.push(say("The load failed, so I stopped here."));
    run_board(Scripted::new(turns), tools);
    assert_eq!(row_status(&root, "F-APP-03").0, "done");
    let (st, notes) = row_status(&root, "F-APP-04");
    assert_eq!(st, "partial");
    assert!(notes.contains("auto: deploy_load failed: push to 192.168.1.5:30500 failed: connection refused") && !notes.contains("second line"), "{notes}");
    assert_eq!(row_status(&root, "F-APP-05").0, "planned");
    assert_eq!(row_status(&root, "F-APP-06").0, "planned");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_progress_claim_with_rows_the_tools_could_not_settle_is_sent_back_once() {
    let (root, tools, _inner, _a) = board_world("nudge", false);
    let b = Scripted::new(vec![
        calls(vec![call("a1", "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"}))]),
        say("Everything is done and working."),
        calls(vec![call("u1", "board_update", json!({"id": "F-APP-01", "status": "partial", "comment": "text changed, not checked in a browser yet"}))]),
        say("The row stays partial until it is checked."),
    ]);
    let ev = run_board(b.clone(), tools);
    assert_eq!(row_status(&root, "F-APP-01").0, "partial");
    assert!(b.requests()[2].0.last().unwrap().content.contains("F-APP-01 (planned)"), "the model is told which rows are open");
    assert!(notes(&ev).iter().any(|n| n.contains("checking the board")));
    // only once: a second claim with rows still open is accepted
    let (root2, tools2, _i, _a2) = board_world("nudge2", false);
    let b = Scripted::new(vec![
        calls(vec![call("a1", "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"}))]),
        say("Everything is done."),
        say("Still all done."),
    ]);
    let ev = run_board(b.clone(), tools2);
    assert_eq!(final_text(&ev), "Still all done.");
    assert_eq!(b.requests().len(), 3);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

// ------------------------------------------------------------------------------------------------ auto mode: no asking permission

fn run_auto(b: Arc<Scripted>, t: Arc<Reg>, auto: bool) -> Vec<RunEvent> {
    let h = start_with(b, t, vec![Message::system("SYS"), Message::user("deploy the app")], RunLimits { auto, ..RunLimits::default() }, std_assist());
    drive(&h, |_| Some(Verdict::Approve))
}

#[test]
fn in_auto_mode_the_model_is_told_not_to_ask_and_a_permission_question_is_sent_back_once() {
    let ask = "The service is deployed. Would you like me to check if the service is responding or help with anything else?";
    let b = Scripted::new(vec![calls(vec![call("c1", "now", json!({}))]), say(ask), calls(vec![call("c2", "now", json!({}))]), say("It answers on port 3000.")]);
    let ev = run_auto(b.clone(), Reg::new(&["now"]), true);
    assert_eq!(final_text(&ev), "It answers on port 3000.");
    let reqs = b.requests();
    assert!(reqs[0].0[0].content.contains("Approvals are automatic: do not ask whether to proceed"), "the rule reaches the system message");
    assert!(reqs[2].0.last().unwrap().content.contains("Do it now; do not ask"), "{:?}", reqs[2].0.last());
    // Ask mode: no rule, and the question stands (the person answers with the chips)
    let b = Scripted::new(vec![calls(vec![call("c1", "now", json!({}))]), say(ask)]);
    let ev = run_auto(b.clone(), Reg::new(&["now"]), false);
    assert_eq!(final_text(&ev), ask);
    assert!(!b.requests()[0].0[0].content.contains("Approvals are automatic"));
    // a real choice or a missing detail in auto mode is left to the person
    let b = Scripted::new(vec![calls(vec![call("c1", "now", json!({}))]), say("Which port should it listen on?")]);
    let ev = run_auto(b.clone(), Reg::new(&["now"]), true);
    assert_eq!(final_text(&ev), "Which port should it listen on?");
    assert_eq!(b.requests().len(), 2);
    // it counts in the cap of two
    let b = Scripted::new(vec![calls(vec![call("c1", "now", json!({}))]), say(ask), say(ask), say(ask)]);
    run_auto(b.clone(), Reg::new(&["now"]), true);
    assert_eq!(b.requests().len(), 4);
}

// ------------------------------------------------------------------------------------------------ autopilot

mod autopilot_tests {
    use super::*;
    use crate::autopilot::{self, Config, Context, Decision, Rows, State, StepEnd};

    fn world(tag: &str, build_fails: bool) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<Mutex<Vec<String>>>) {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-ap-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let inner = Reg::with(&["read_file", "write_file", "deploy_build", "deploy_status", "k8s_delete"], 0, Box::new(move |n, _| match n {
            "deploy_build" if build_fails => Err("build failed: no space left".into()),
            "deploy_status" => Ok("Deployment web: 1/1 ready, Healthy".into()),
            o => Ok(format!("{o} ok")),
        }));
        let audit = Arc::new(Mutex::new(Vec::new()));
        let a2 = audit.clone();
        let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")).with_audit(Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k}: {d}")))));
        (root, t, audit)
    }

    fn ctx(root: &std::path::Path, audit: &Arc<Mutex<Vec<String>>>) -> Context {
        let mut c = Context::new(root, &root.join("FEATURES.md"), "create a simple nodejs webserver and deploy it");
        let a = audit.clone();
        c.audit = Some(Arc::new(move |k, d| a.lock().unwrap().push(format!("{k}: {d}"))));
        c
    }

    fn go(b: Arc<Scripted>, t: Arc<crate::boardtools::ProjectTools>, c: Context, verdict: impl FnMut(&str) -> Option<Verdict>) -> Vec<RunEvent> {
        let h = autopilot::start(b, t, vec![Message::system("SYS"), Message::user("create a simple nodejs webserver and deploy it")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
        drive(&h, verdict)
    }

    fn adds(titles: &[&str]) -> Result<ModelTurn, String> {
        calls(titles.iter().enumerate().map(|(i, t)| call(&format!("a{i}"), "board_add", json!({"prefix": "APP", "title": t}))).collect())
    }

    fn user_messages(b: &Scripted) -> Vec<String> {
        b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect()
    }

    #[test]
    fn a_multi_step_goal_completes_without_the_person_typing_anything() {
        let (root, t, audit) = world("full", false);
        let b = Scripted::new(vec![
            adds(&["Write server.js with an HTTP server", "Add a Dockerfile", "Build the container image"]),
            say("Planned three steps."),
            calls(vec![call("w1", "write_file", json!({"path": "server.js"}))]),
            say("Wrote server.js."),
            calls(vec![call("w2", "write_file", json!({"path": "Dockerfile"}))]),
            say("Wrote the Dockerfile."),
            calls(vec![call("b1", "deploy_build", json!({"dir": ".", "image": "web"}))]),
            say("Built the image."),
            calls(vec![call("s1", "deploy_status", json!({"name": "web"}))]),
            say("Verified: the deployment is ready. Done: three rows. Blocked: none. You: nothing."),
        ]);
        let ev = go(b.clone(), t, ctx(&root, &audit), |_| Some(Verdict::Approve));
        let text = final_text(&ev);
        assert!(text.contains("Verified: the deployment is ready") && text.contains("Autopilot finished after 5 steps: 3 of 3 rows done"), "{text}");
        assert!(text.contains("Files changed: server.js, Dockerfile") && text.contains("Commands run: deploy_build"), "{text}");
        assert!(notes(&ev).iter().any(|n| n.starts_with("autopilot: step 2 · row 1 of 3 · F-APP-01")) && notes(&ev).iter().any(|n| n.contains("verifying")), "{:?}", notes(&ev));
        let users = user_messages(&b);
        assert!(users[2].starts_with("AUTOPILOT step 2: 0/3 rows done. Work on F-APP-01"), "{}", users[2]);
        assert!(!users[2].contains("F-APP-02"), "only the current row is sent, not the whole board");
        assert!(users.iter().filter(|u| u.contains("create a simple nodejs")).count() == 0 || users[0].contains("AUTOPILOT goal"), "the person typed one message only");
        let b = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
        assert!(b.contains("done 3"), "{b}");
        let a = audit.lock().unwrap().join("\n");
        assert!(a.contains("autopilot.start: goal: create a simple nodejs") && a.contains("autopilot.step") && a.contains("autopilot.stop: finished"), "{a}");
        assert!(std::fs::read_to_string(root.join(".iyakki/autopilot.json")).unwrap().contains("\"finished\""));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_row_that_keeps_failing_is_retried_differently_then_blocked_and_the_run_moves_on() {
        let (root, t, audit) = world("retry", true);
        let build = |id: &str| calls(vec![call(id, "deploy_build", json!({"dir": ".", "image": "web"}))]);
        let b = Scripted::new(vec![
            adds(&["Build the container image", "Add a Dockerfile"]),
            say("Planned."),
            build("b1"),
            say("It failed."),
            build("b2"),
            say("It failed again."),
            build("b3"),
            say("It failed a third time."),
            calls(vec![call("w1", "write_file", json!({"path": "Dockerfile"}))]),
            say("Wrote the Dockerfile."),
            say("Summary: build blocked (disk full), Dockerfile done. You: free disk space."),
        ]);
        let ev = go(b.clone(), t, ctx(&root, &audit), |_| Some(Verdict::Approve));
        let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
        let r1 = board.get("F-APP-01").unwrap();
        assert_eq!(r1.status, "blocked", "{r1:?}");
        assert!(r1.notes.contains("autopilot: not finished after 3 attempts"), "{}", r1.notes);
        assert_eq!(board.get("F-APP-02").unwrap().status, "done");
        let users = user_messages(&b);
        assert!(users[4].contains("Attempt 2 of 3") && users[6].contains("Attempt 3 of 3"), "{users:?}");
        assert!(notes(&ev).iter().any(|n| n.contains("F-APP-01 blocked")), "{:?}", notes(&ev));
        assert!(final_text(&ev).contains("1 blocked"), "{}", final_text(&ev));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hard_ask_call_waits_for_the_person_then_goes_on_and_a_refusal_blocks_the_row() {
        for approve in [true, false] {
            let (root, t, audit) = world(if approve { "ask-yes" } else { "ask-no" }, false);
            let mut step2 = vec![call("d1", "k8s_delete", json!({"path": "old"}))];
            if approve {
                step2.push(call("w1", "write_file", json!({"path": "Dockerfile"})));
            }
            let b = Scripted::new(vec![adds(&["Add a Dockerfile"]), say("Planned."), calls(step2), say("Handled the old deployment."), say("Summary.")]);
            let ev = go(b, t, ctx(&root, &audit), |name| {
                assert_eq!(name, "k8s_delete", "only the call that always asks is put to the person");
                Some(if approve { Verdict::Approve } else { Verdict::Deny })
            });
            assert!(notes(&ev).iter().any(|n| n.contains("needs you")), "{:?}", notes(&ev));
            assert!(ev.iter().any(|e| matches!(e, RunEvent::Decision { name, by, approved, .. } if name == "k8s_delete" && by == "user" && *approved == approve)), "{ev:?}");
            let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
            if approve {
                assert_eq!(board.get("F-APP-01").unwrap().status, "done");
            } else {
                assert_eq!(board.get("F-APP-01").unwrap().status, "blocked");
                assert!(board.get("F-APP-01").unwrap().notes.contains("did not approve"));
            }
            assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{:?}", ev.last());
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn budgets_and_no_progress_stop_the_run_and_it_can_be_resumed() {
        // steps
        let (root, t, audit) = world("steps", false);
        let mut c = ctx(&root, &audit);
        c.cfg.max_steps = 2;
        let b = Scripted::new(vec![adds(&["Polish the welcome text"]), say("Planned."), say("Working on it."), say("never")]);
        let ev = go(b.clone(), t, c, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("step budget")), "{:?}", ev.last());
        assert_eq!(b.requests().len(), 3);
        // tokens
        let (root2, t2, a2) = world("tokens", false);
        let mut c = ctx(&root2, &a2);
        c.cfg.max_tokens = 1;
        let b = Scripted::new(vec![adds(&["Polish the welcome text"]), say("Planned."), say("never")]);
        let ev = go(b, t2, c, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("token budget")), "{:?}", ev.last());
        // no progress: the row cannot be tracked and the board never changes
        let (root3, t3, a3) = world("noprog", false);
        let b = Scripted::new(vec![adds(&["Polish the welcome text"]), say("Planned."), say("Looking."), say("Still looking."), say("Looking more."), say("never")]);
        let mut c = ctx(&root3, &a3);
        c.cfg.retries = 10;
        let ev = go(b, t3, c, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("no progress in 3 steps")), "{:?}", ev.last());
        // the state says stopped, and Resume goes on from there with fresh budgets
        let saved = State::load(&root3.join(".iyakki/autopilot.json")).unwrap();
        assert_eq!(saved.status, "stopped");
        let b = Scripted::new(vec![say("Looked again; the text is fine."), say("Final check passed.")]);
        let (_r, t4, _a4) = world("noprog-keep", false);
        let _ = t4;
        let tools = Arc::new(crate::boardtools::ProjectTools::new(Reg::new(&["read_file"]), &root3, "Web", &root3.join("FEATURES.md")));
        let mut c = ctx(&root3, &a3);
        c.resume = true;
        c.cfg.retries = 10;
        let ev = go(b, tools, c, |_| Some(Verdict::Approve));
        assert!(notes(&ev).iter().any(|n| n.starts_with("autopilot resumed")), "{:?}", notes(&ev));
        for r in [root, root2, root3] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    #[test]
    fn typing_pauses_autopilot_and_resume_continues_a_restarted_window_sees_the_saved_place() {
        let (root, t, audit) = world("pause", false);
        let b = Scripted::streaming(vec![say("slow")], 400, vec![]);
        let h = autopilot::start(b, t.clone(), vec![Message::system("SYS"), Message::user("goal")], RunLimits::default(), std_assist(), ctx(&root, &audit));
        std::thread::sleep(Duration::from_millis(100));
        h.pause();
        let ev = drive(&h, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("paused by the person") && reason.contains("Resume")), "{:?}", ev.last());
        let saved = State::load(&root.join(".iyakki/autopilot.json")).expect("saved");
        assert_eq!((saved.status.as_str(), saved.goal.as_str()), ("paused", "create a simple nodejs webserver and deploy it"));
        assert!(audit.lock().unwrap().iter().any(|a| a.starts_with("autopilot.pause")));
        // a new window: Resume
        let b = Scripted::new(vec![adds(&["Add a Dockerfile"]), say("Planned."), calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]), say("Done."), say("Verified.")]);
        let mut c = ctx(&root, &audit);
        c.resume = true;
        let ev = go(b, t, c, |_| Some(Verdict::Approve));
        assert!(notes(&ev).iter().any(|n| n.starts_with("autopilot resumed")), "{:?}", notes(&ev));
        assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{:?}", ev.last());
        // Stop is not Resume-able by accident: cancel ends it and says so
        let (root2, t2, a2) = world("stop", false);
        let b = Scripted::streaming(vec![say("slow")], 400, vec![]);
        let h = autopilot::start(b, t2, vec![Message::system("SYS"), Message::user("goal")], RunLimits::default(), std_assist(), ctx(&root2, &a2));
        std::thread::sleep(Duration::from_millis(100));
        h.cancel();
        let ev = drive(&h, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("stopped by the person")), "{:?}", ev.last());
        for r in [root, root2] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    #[test]
    fn a_read_only_workspace_refuses_and_the_state_machine_enforces_time_and_tokens() {
        let (root, t, audit) = world("ro", false);
        let mut c = ctx(&root, &audit);
        c.read_only = true;
        let b = Scripted::new(vec![say("never")]);
        let ev = go(b.clone(), t, c, |_| Some(Verdict::Approve));
        assert!(matches!(ev.last(), Some(RunEvent::Failed { error }) if error.contains("read-only")), "{:?}", ev.last());
        assert!(b.requests().is_empty());
        let cfg = Config::default();
        let rows = Rows { open: vec![("F-1".into(), "x".into(), "planned".into())], total: 1, done: 0, blocked: 0, claimed: 0, sig: "a".into(), ..Default::default() };
        let mut s = State::new("g", 1000);
        assert!(matches!(s.after_step(&cfg, &StepEnd::Final, &rows, 10, 1000 + 31 * 60, false), Decision::Stop(w) if w.contains("time budget")));
        let mut s = State::new("g", 1000);
        let cfg2 = Config { max_tokens: 100, ..Config::default() };
        assert!(matches!(s.after_step(&cfg2, &StepEnd::Final, &rows, 150, 1001, false), Decision::Stop(w) if w.contains("token budget")));
        let mut s = State::new("g", 1000);
        assert!(matches!(s.after_step(&cfg, &StepEnd::Failed("down".into()), &rows, 0, 1001, false), Decision::Pause(_)));
        // the state survives a restart as JSON
        let mut s = State::new("deploy it", 5);
        s.tries.insert("F-1".into(), 2);
        s.status = "paused".into();
        assert_eq!(State::from_json(&s.to_json()), Some(s));
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ------------------------------------------------------------------------------------------------ done needs proof

fn proof_world(tag: &str) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-proof-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let inner = Reg::with(&["read_file", "write_file", "k8s_status", "deploy_build", "run_command"], 0, Box::new(|n, _| match n {
        "k8s_status" => Ok("Kubernetes cluster connected\nnamespace: test2-dev (exists)\nlocal k3s: yes".into()),
        o => Ok(format!("{o} ok")),
    }));
    (root.clone(), Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md"))))
}

fn board_call(t: &crate::boardtools::ProjectTools, name: &str, args: Value) -> Result<String, String> {
    Tools::call(t, name, &args)
}

#[test]
fn the_model_cannot_set_a_row_to_done_without_a_verified_check() {
    let (root, t) = proof_world("refuse");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Create namespace test2-dev"})).unwrap();
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"})).unwrap();
    // the screenshot: done is claimed although the step was blocked, with a Tests-style note. Nothing is verified, so it is not done
    for (n, args) in [json!({"id": "F-APP-01", "status": "done", "comment": "blocked, cluster-scoped"}), json!({"id": "F-APP-02", "status": "done", "comment": "tests: all passed"})].into_iter().enumerate() {
        Tools::begin_step(&*t);
        let r = board_call(&t, "board_update", args).unwrap();
        assert!(r.contains("is parked as review") && r.contains("needs your OK") && r.contains("Continue with the next row"), "{n}: {r}");
        assert!(Tools::should_end_run(&*t).is_some(), "{n}: the step ends so the next row starts");
    }
    assert_eq!((row_status(&root, "F-APP-01").0, row_status(&root, "F-APP-02").0), ("review".to_string(), "review".to_string()));
    assert!(!crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    assert!(Tools::preview(&*t, "board_update", &json!({"id": "F-APP-01", "status": "done"})).is_some_and(|p| p.contains("would be refused") || p.contains("F-APP-01")), "the approval preview exists");
    // partial and blocked, with a reason, are fine
    Tools::begin_step(&*t);
    assert!(board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "blocked", "comment": "cluster-scoped, cannot be created"})).is_ok());
    // a passing test run is a verified check for the rows that name a test
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Run the unit tests"})).unwrap();
    board_call(&t, "run_command", json!({"program": "npm", "args": ["test"]})).unwrap();
    assert_eq!(row_status(&root, "F-APP-03").0, "done", "the harness itself marks it, verified");
    // a row the harness proved may be set again by the model
    Tools::begin_step(&*t);
    board_call(&t, "board_update", json!({"id": "F-APP-03", "status": "partial", "comment": "rerunning"})).unwrap();
    assert!(board_call(&t, "board_update", json!({"id": "F-APP-03", "status": "done"})).unwrap().starts_with("updated"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_row_blocked_as_unnecessary_is_done_only_when_the_namespace_is_seen_to_exist() {
    let (root, t) = proof_world("ns");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Create namespace test2-dev"})).unwrap();
    board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "blocked", "comment": "cluster-scoped, cannot be created"})).unwrap();
    // other successes do not revive a blocked row
    board_call(&t, "deploy_build", json!({"dir": ".", "image": "web"})).unwrap();
    assert_eq!(row_status(&root, "F-APP-01").0, "blocked");
    // the namespace exists (it is created by the first apply): the verified path marks it done
    board_call(&t, "k8s_status", json!({})).unwrap();
    let (st, notes) = row_status(&root, "F-APP-01");
    assert_eq!(st, "done", "{notes}");
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rows_marked_done_without_proof_are_claimed_not_counted_and_the_summary_says_so() {
    let (root, t) = proof_world("claimed");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"})).unwrap();
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Add a Dockerfile"})).unwrap();
    board_call(&t, "write_file", json!({"path": "Dockerfile"})).unwrap();
    // somebody forces the first row to done in the file
    let f = root.join("FEATURES.md");
    let text = std::fs::read_to_string(&f).unwrap().replacen("| planned |", "| done |", 1);
    std::fs::write(&f, text).unwrap();
    let rows = crate::autopilot::Rows::read(&root, &f);
    assert_eq!((rows.done, rows.claimed, rows.total), (1, 1, 2), "only the verified one counts");
    // and the autopilot summary says it
    let (root2, t2, audit) = (root.clone(), t.clone(), Arc::new(Mutex::new(Vec::new())));
    let mut c = crate::autopilot::Context::new(&root2, &f, "polish and containerise");
    let a = audit.clone();
    c.audit = Some(Arc::new(move |k, d| a.lock().unwrap().push(format!("{k}: {d}"))));
    let b = Scripted::new(vec![say("Planned."), say("All checked.")]);
    let h = crate::autopilot::start(b, t2, vec![Message::system("SYS"), Message::user("go")], RunLimits::default(), std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let text = final_text(&ev);
    assert!(text.contains("1 of 2 rows done (verified)") && text.contains("1 marked done without a check (claimed, not counted)"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_same_board_update_twice_writes_once_and_the_second_says_already_set() {
    let (root, t) = proof_world("dup");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"})).unwrap();
    let args = json!({"id": "F-APP-01", "status": "partial", "comment": "started"});
    let first = board_call(&t, "board_update", args.clone()).unwrap();
    assert!(first.starts_with("updated F-APP-01"), "{first}");
    let after_first = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
    let mtime = std::fs::metadata(root.join("FEATURES.md")).unwrap().modified().unwrap();
    let second = board_call(&t, "board_update", args).unwrap();
    assert!(second.starts_with("already set") && second.contains("nothing was written"), "{second}");
    assert_eq!(std::fs::read_to_string(root.join("FEATURES.md")).unwrap(), after_first);
    assert_eq!(std::fs::metadata(root.join("FEATURES.md")).unwrap().modified().unwrap(), mtime, "no write happened");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ claims of done, pending rows

fn vworld(tag: &str, ready: bool) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<Mutex<Vec<String>>>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-verify-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let inner = Reg::with(&["read_file", "write_file", "deploy_status"], 0, Box::new(move |n, _| match n {
        "deploy_status" if ready => Ok("Deployment web: 1/1 ready, Healthy".into()),
        "deploy_status" => Ok("Deployment web: 0/1 ready, Unhealthy\nPod web-1: Pending ImagePullBackOff".into()),
        o => Ok(format!("{o} ok")),
    }));
    let audit = Arc::new(Mutex::new(Vec::new()));
    let a2 = audit.clone();
    let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")).with_audit(Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k}: {d}")))));
    (root, t, audit)
}

#[test]
fn claiming_done_runs_the_check_itself_and_a_pass_marks_the_row_done() {
    let (root, t, audit) = vworld("claim-ok", true);
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Check that the app responds"})).unwrap();
    let r = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "Deployment completed. Application is now ready for use."})).unwrap();
    assert!(r.starts_with("updated F-APP-01"), "{r}");
    assert_eq!(row_status(&root, "F-APP-01").0, "done");
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    assert!(audit.lock().unwrap().iter().any(|a| a.contains("verified by the harness before done")), "{:?}", audit.lock().unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_claim_with_a_failing_check_ends_the_step_with_a_diagnosis_for_the_next_one_and_only_real_fix_attempts_count() {
    let (root, t, _a) = vworld("claim-fail", false);
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Check that the app responds"})).unwrap();
    let e1 = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "it works"})).unwrap_err();
    assert!(e1.contains("the check failed") && e1.chars().count() < 300 && e1.lines().count() == 1, "short, one line, no dump: {e1}");
    assert_eq!(row_status(&root, "F-APP-01").0, "planned", "the row is unchanged");
    assert!(Tools::should_end_run(&*t).is_some_and(|m| m.contains("diagnosis")), "the step ends at the first failed check");
    let (id, diag) = Tools::take_diagnosis(&*t).expect("the diagnosis is handed over once");
    assert!(id == "F-APP-01" && diag.contains("ImagePullBackOff") && diag.contains("0/1 ready"), "{diag}");
    assert!(Tools::take_diagnosis(&*t).is_none());
    // nothing changed between failures: stalls, not attempts, and only the third parks the row
    for n in 0..2 {
        Tools::begin_step(&*t);
        assert!(board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": format!("again {n}")})).is_err(), "stall {n}");
        assert!(Tools::should_end_run(&*t).is_some());
    }
    Tools::begin_step(&*t);
    let r = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "once more"})).unwrap();
    assert!(r.contains("parked as blocked") && r.chars().count() < 300, "{r}");
    let (st, notes) = row_status(&root, "F-APP-01");
    assert_eq!(st, "blocked");
    assert!(notes.contains("still failed after 3 fix attempts") && notes.contains("ImagePullBackOff"), "the note carries the diagnosis: {notes}");
    // real fix attempts (something changed in between) count the same way
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Check the second app responds"})).unwrap();
    board_call(&t, "board_update", json!({"id": "F-APP-02", "status": "partial", "comment": "needs F-APP-01 first"})).unwrap();
    for n in 0..4 {
        Tools::begin_step(&*t);
        let r = board_call(&t, "board_update", json!({"id": "F-APP-02", "status": "done", "comment": format!("fix {n}")}));
        if n < 3 {
            assert!(r.is_err(), "{n}: {r:?}");
            board_call(&t, "write_file", json!({"path": format!("fix{n}.yaml")})).unwrap();
        } else {
            assert!(r.unwrap().contains("parked as pending"), "the fourth failure is the third failed fix");
        }
    }
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    let f = b.get("F-APP-02").unwrap();
    assert_eq!((f.status.as_str(), f.depends().as_str()), ("pending", "F-APP-01"), "{f:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_row_with_no_automatic_check_goes_to_review_at_once_and_is_not_asked_about_again() {
    let (root, t, _a) = vworld("claim-review", true);
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"})).unwrap();
    let args = json!({"id": "F-APP-01", "status": "done", "comment": "Deployment completed. Application is now ready for use."});
    let r = board_call(&t, "board_update", args.clone()).unwrap();
    assert!(r.contains("parked as review") && r.contains("needs your OK"), "{r}");
    assert_eq!(row_status(&root, "F-APP-01").0, "review");
    assert!(Tools::should_end_run(&*t).is_some(), "the step ends");
    assert!(Tools::should_end_run(&*t).is_none(), "and the signal is given once");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn pending_rows_wait_for_their_dependency_and_autopilot_comes_back_to_them() {
    let (root, t, audit) = vworld("pending", true);
    let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "ship it");
    let a = audit.clone();
    c.audit = Some(Arc::new(move |k, d| a.lock().unwrap().push(format!("{k}: {d}"))));
    let b = Scripted::new(vec![
        adds_rows(&["Check that the app responds", "Add a Dockerfile"]),
        say("Planned."),
        calls(vec![call("p", "board_update", json!({"id": "F-APP-01", "status": "pending", "depends_on": "F-APP-02", "comment": "needs the Dockerfile first"}))]),
        say("Waiting on the Dockerfile."),
        calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]),
        say("Wrote it."),
        calls(vec![call("s", "deploy_status", json!({"name": "web"}))]),
        say("The app is ready."),
        say("Checked; all good."),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("ship it")], RunLimits { max_steps: 20, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let n = notes(&ev);
    assert!(n.iter().any(|x| x.contains("autopilot: step 3/2") || x.contains("F-APP-02")), "{n:?}");
    assert!(n.iter().any(|x| x.contains("back to F-APP-01") && x.contains("F-APP-02 is done")), "pending row revisited once its dependency is done: {n:?}");
    assert_eq!((row_status(&root, "F-APP-01").0, row_status(&root, "F-APP-02").0), ("done".to_string(), "done".to_string()));
    assert!(final_text(&ev).contains("2 of 2 rows done (verified)"), "{}", final_text(&ev));
    let _ = std::fs::remove_dir_all(&root);
    // a row that stays pending is listed in the summary with what it waits for
    let (root, t, _a) = vworld("pending2", true);
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "ship it");
    let pend = |id: &str| calls(vec![call(id, "board_update", json!({"id": "F-APP-01", "status": "pending", "depends_on": "the registry", "comment": "no registry yet"}))]);
    let b = Scripted::new(vec![adds_rows(&["Polish the welcome text"]), say("Planned."), pend("p1"), say("Waiting."), pend("p2"), say("Still waiting."), say("Summary: waiting for the registry.")]);
    let h = crate::autopilot::start(b, t, vec![Message::system("SYS"), Message::user("ship it")], RunLimits { max_steps: 20, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let text = final_text(&ev);
    assert!(text.contains("Pending: F-APP-01 (waits for the registry)"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

fn adds_rows(titles: &[&str]) -> Result<ModelTurn, String> {
    calls(titles.iter().enumerate().map(|(i, t)| call(&format!("a{i}"), "board_add", json!({"prefix": "APP", "title": t}))).collect())
}

// ------------------------------------------------------------------------------------------------ the thinking watchdog

/// A model that thinks for ever on its first `stall` calls (a stream of thinking tokens and keep-alives, nothing visible), then answers.
struct Stalling {
    stall: usize,
    calls: Mutex<usize>,
    seen: Mutex<Vec<Vec<Message>>>,
    stopped_reading_after: Mutex<Option<Duration>>,
}

impl Stalling {
    fn new(stall: usize) -> Arc<Stalling> {
        Arc::new(Stalling { stall, calls: Mutex::new(0), seen: Mutex::new(vec![]), stopped_reading_after: Mutex::new(None) })
    }
}

impl ChatBackend for Stalling {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("unused".into())
    }
    fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.complete_tools_stream(h, t, &mut |_| true)
    }
    fn complete_tools_stream(&self, h: &[Message], _t: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push(h.to_vec());
        let n = {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            *c
        };
        if n > self.stall {
            return Ok(ModelTurn { text: "Here is the answer.".into(), calls: vec![] });
        }
        let t0 = Instant::now();
        for _ in 0..2000 {
            crate::providers::report_thinking("hmm, let me think about this for a long time. ");
            if !on_text("") {
                *self.stopped_reading_after.lock().unwrap() = Some(t0.elapsed());
                return Err("read abandoned".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("never answered".into())
    }
}

fn watchdog_assist() -> Assist {
    let mut a = std_assist();
    a.think_limit = Duration::from_millis(300);
    a.silence_limit = Duration::from_millis(600);
    a
}

fn history_of(n: usize) -> Vec<Message> {
    let mut h = vec![Message::system("SYS")];
    for i in 0..n {
        h.push(Message::user(format!("question {i}")));
        h.push(Message::assistant(format!("answer {i}")));
    }
    h.push(Message::user("the real question"));
    h
}

#[test]
fn a_model_that_only_thinks_is_abandoned_and_retried_once_with_shorter_thinking_and_a_smaller_context() {
    let b = Stalling::new(1);
    let t0 = Instant::now();
    let h = start_with(b.clone(), Reg::new(&["now"]), history_of(10), RunLimits::default(), watchdog_assist());
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert_eq!(final_text(&ev), "Here is the answer.");
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    let n = notes(&ev);
    assert!(n.iter().any(|x| x.contains("watchdog") && x.contains("thinking for") && x.contains("shorter thinking")), "{n:?}");
    let seen = b.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].len() < seen[0].len(), "a smaller context: {} vs {}", seen[1].len(), seen[0].len());
    assert!(seen[1].last().unwrap().content.contains("thought for too long") && seen[1][0].content == "SYS", "the system message stays, the plain instruction comes last");
    assert!(seen[1].iter().any(|m| m.content.starts_with("the real question")));
    std::thread::sleep(Duration::from_millis(300));
    assert!(b.stopped_reading_after.lock().unwrap().is_some_and(|d| d < Duration::from_secs(2)), "the abandoned stream stops being read at the next token");
}

#[test]
fn a_second_stall_stops_the_run_with_a_clear_message_and_keeps_the_thinking() {
    let b = Stalling::new(2);
    let h = start_with(b.clone(), Reg::new(&["now"]), history_of(2), RunLimits::default(), watchdog_assist());
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("watchdog") && reason.contains("twice") && reason.contains("another model")), "{:?}", ev.last());
    assert_eq!(b.seen.lock().unwrap().len(), 2, "one retry only");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Thinking { text, .. } if text.contains("let me think"))), "the partial thinking was shown");
}

#[test]
fn stop_interrupts_a_thinking_model_within_a_second() {
    let b = Stalling::new(5);
    let mut a = std_assist();
    a.think_limit = Duration::from_secs(60);
    let h = start_with(b.clone(), Reg::new(&["now"]), history_of(1), RunLimits::default(), a);
    std::thread::sleep(Duration::from_millis(300));
    let t0 = Instant::now();
    h.cancel();
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("stopped by the user")), "{:?}", ev.last());
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    std::thread::sleep(Duration::from_millis(200));
    assert!(b.stopped_reading_after.lock().unwrap().is_some(), "the helper stopped reading too");
}

#[test]
fn the_default_limits_are_sixty_and_one_hundred_twenty_seconds() {
    let a = Assist::standard(ctx());
    assert_eq!((a.think_limit, a.silence_limit), (Duration::from_secs(60), Duration::from_secs(120)));
    assert_eq!(crate::settings::Settings::default().think_limit_secs, 0, "0 means the default");
}

#[test]
fn a_message_typed_while_paused_becomes_part_of_the_next_step_and_replans() {
    let (root, t, audit) = vworld("interject", true);
    let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "build the app");
    let a = audit.clone();
    c.audit = Some(Arc::new(move |k, d| a.lock().unwrap().push(format!("{k}: {d}"))));
    let slow = Scripted::streaming(vec![say("slow")], 400, vec![]);
    let h = crate::autopilot::start(slow, t.clone(), vec![Message::system("SYS"), Message::user("build the app")], RunLimits::default(), std_assist(), c.clone());
    std::thread::sleep(Duration::from_millis(100));
    h.pause();
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("paused")), "{:?}", ev.last());
    // the person's message is not a new goal: the saved goal stays and the message is part of the next step
    let mut c2 = c;
    c2.resume = true;
    c2.interjection = Some("going in loop, fix the text and use port 8080".into());
    let b = Scripted::new(vec![say("ok"), say("ok again")]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("going in loop, fix the text and use port 8080")], RunLimits::default(), std_assist(), c2);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let first = b.requests()[0].0.iter().rev().find(|m| m.role == Role::User).unwrap().content.clone();
    assert!(first.contains("the person interrupted and says: \"going in loop, fix the text and use port 8080\"") && first.contains("Re-plan"), "{first}");
    assert!(notes(&ev).iter().any(|n| n.contains("re-planning for your message")), "{:?}", notes(&ev));
    assert!(!notes(&ev).iter().any(|n| n.starts_with("autopilot on:")), "not announced as a new goal");
    assert_eq!(crate::autopilot::State::load(&root.join(".iyakki/autopilot.json")).unwrap().goal, "build the app");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ autopilot context, tokens, history

mod autopilot_context_tests {
    use super::*;
    use crate::autopilot::{self, Context};
    use crate::history::{History, Scope};

    fn kb() -> Arc<Mutex<crate::kb::KnowledgeBase>> {
        Arc::new(Mutex::new(crate::kb::KnowledgeBase::in_memory(crate::kb::default_embedder())))
    }

    fn world(tag: &str, handler: Handler) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>) {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-apc-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let inner = Reg::with(&["read_file", "write_file", "deploy_status", "k8s_logs"], 0, handler);
        (root.clone(), Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md"))))
    }

    fn plain(_: &str, _: &Value) -> Result<String, String> {
        Ok("ok".into())
    }

    fn run(b: Arc<Scripted>, t: Arc<crate::boardtools::ProjectTools>, c: Context, assist: Assist) -> Vec<RunEvent> {
        let h = autopilot::start(b, t, vec![Message::system("SYS"), Message::user("build and deploy the shop")], RunLimits { max_steps: 20, max_tool_calls: 400, ..RunLimits::default() }, assist, c);
        drive(&h, |_| Some(Verdict::Approve))
    }

    /// A chat that has been going on for a long time: what a normal run would resend with every request.
    fn long_chat() -> crate::assistrun::Prepare {
        Arc::new(|_defs| {
            let mut h = vec![Message::system("SYS (the system prompt of the chat)")];
            for i in 0..150 {
                h.push(Message::user(format!("earlier question {i}: {}", "about the shop and its deployment ".repeat(8))));
                h.push(Message::assistant(format!("earlier answer {i}: {}", "the shop service was configured like this ".repeat(8))));
            }
            h.push(Message::user("build and deploy the shop"));
            h
        })
    }

    /// 16 steps of a plan whose rows the tools cannot prove: every step reads a bulky file and adds a row.
    fn long_script() -> Vec<Result<ModelTurn, String>> {
        let mut v = vec![calls(vec![call("a0", "board_add", json!({"prefix": "APP", "title": "Task 0"}))]), say("Planned.")];
        for k in 1..16 {
            v.push(calls(vec![call(&format!("r{k}"), "read_file", json!({"path": format!("log{k}.txt")}))]));
            v.push(calls(vec![call(&format!("a{k}"), "board_add", json!({"prefix": "APP", "title": format!("Task {k}")}))]));
            v.push(say(&format!("Looked at part {k}.")));
        }
        v.push(say("never"));
        v
    }

    fn step_request_tokens(b: &Scripted, step: usize) -> Vec<u64> {
        b.requests().iter().filter(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).is_some_and(|x| x.content.starts_with(&format!("AUTOPILOT step {step}:")))).map(|(m, _)| crate::agent::estimate_tokens(m)).collect()
    }

    #[test]
    fn the_context_of_step_sixteen_stays_small_where_the_old_way_resends_the_whole_chat() {
        let big = |_: &str, _: &Value| -> Result<String, String> { Ok("log line of some tool output\\n".repeat(2000)) };
        let measure = |tag: &str, compact: bool| {
            let (root, t) = world(tag, Box::new(big));
            let mut c = Context::new(&root, &root.join("FEATURES.md"), "build and deploy the shop");
            c.compact = compact;
            c.cfg.max_steps = 16;
            c.cfg.retries = 100;
            c.cfg.max_tokens = 100_000_000;
            let mut a = std_assist();
            a.prepare = Some(long_chat());
            let b = Scripted::new(long_script());
            let ev = run(b.clone(), t, c, a);
            let _ = std::fs::remove_dir_all(&root);
            (b, ev)
        };
        let (b, ev) = measure("compact", true);
        let sizes: Vec<u64> = (2..=16).map(|s| step_request_tokens(&b, s).into_iter().max().unwrap_or(0)).collect();
        assert!(sizes.iter().all(|s| *s > 0), "every step from 2 to 16 ran: {sizes:?} {:?} {:?}", ev.last(), notes(&ev).last());
        assert!(sizes[14] < 6_000, "step 16 sends under 6k tokens: {sizes:?}");
        assert!(sizes[14] < sizes[0] * 3, "and it does not grow with the number of steps: {sizes:?}");
        assert!(notes(&ev).iter().any(|n| n.contains("step 16") && n.contains("k tok last step")), "{:?}", notes(&ev));
        let (old, _) = measure("old", false);
        let before = (2..=16).map(|s| step_request_tokens(&old, s).into_iter().max().unwrap_or(0)).collect::<Vec<_>>();
        assert!(before[14] > sizes[14] * 3, "before {} vs after {}", before[14], sizes[14]);
    }

    #[test]
    fn only_new_tokens_count_and_a_budget_ending_is_a_checkpoint_not_a_failure() {
        // new tokens: the same run, counted both ways
        let usage = |new: bool| {
            let t = Reg::with(&["read_file"], 0, Box::new(|_, _| Ok("some file text ".repeat(300))));
            let mut a = std_assist();
            a.count_new_tokens = new;
            let turns: Vec<_> = (0..6).map(|i| calls(vec![call(&format!("c{i}"), "read_file", json!({}))])).chain([say("done reading")]).collect();
            let h = start_with(Scripted::new(turns), t, vec![Message::system("SYS"), Message::user("read it")], RunLimits::default(), a);
            let ev = drive(&h, |_| Some(Verdict::Approve));
            ev.iter().rev().find_map(|e| if let RunEvent::Usage { tokens, .. } = e { Some(*tokens) } else { None }).unwrap()
        };
        let (full, new) = (usage(false), usage(true));
        assert!(new * 2 < full, "the resent history is not counted again: new {new} vs full {full}");
        // a token budget ending writes a checkpoint to the chat and the state, and Resume starts with fresh budgets
        let (root, t) = world("budget", Box::new(plain));
        let mut c = Context::new(&root, &root.join("FEATURES.md"), "build and deploy the shop");
        c.cfg.max_tokens = 1;
        let a = std_assist();
        let b = Scripted::new(vec![calls(vec![call("a0", "board_add", json!({"prefix": "APP", "title": "Add a Dockerfile"}))]), say("Planned.")]);
        let ev = run(b, t.clone(), c.clone(), a.clone());
        assert!(ev.iter().any(|e| matches!(e, RunEvent::Model { text, .. } if text.contains("reached its token budget") && text.contains("Nothing is lost") && text.contains("Resume (fresh context)"))), "{ev:?}");
        assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("token budget")));
        let saved = autopilot::State::load(&root.join(".iyakki/autopilot.json")).unwrap();
        assert_eq!((saved.status.as_str(), saved.step), ("stopped", 1));
        assert!(saved.summary.contains("Goal: build and deploy the shop") && saved.summary.len() <= autopilot::SUMMARY_CAP);
        let mut c2 = c;
        c2.resume = true;
        c2.cfg.max_tokens = 0;
        let b = Scripted::new(vec![calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]), say("Wrote it."), say("Checked.")]);
        let ev = run(b, t, c2, a);
        assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{:?}", ev.last());
        assert!(autopilot::State::load(&root.join(".iyakki/autopilot.json")).unwrap().base_step >= 1, "budgets restarted from the resumed step");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tool_that_loops_does_not_stop_autopilot_the_harness_verifies_or_blocks_the_row_and_goes_on() {
        for (ready, want) in [(true, "done"), (false, "blocked")] {
            let (root, t) = world(if ready { "loop-ok" } else { "loop-bad" }, Box::new(move |n, _| match n {
                "k8s_logs" => Ok("(the log is empty)".into()),
                "deploy_status" if ready => Ok("Deployment web: 1/1 ready, Healthy".into()),
                "deploy_status" => Ok("Deployment web: 0/1 ready, Unhealthy".into()),
                o => Ok(format!("{o} ok")),
            }));
            let c = Context::new(&root, &root.join("FEATURES.md"), "check the app");
            let looped = calls((0..6).map(|i| call(&format!("l{i}"), "k8s_logs", json!({"pod": "web-1"}))).collect());
            let b = Scripted::new(vec![calls(vec![call("a", "board_add", json!({"prefix": "APP", "title": "Check that the app responds"}))]), say("Planned."), looped, say("Stopped by the loop guard."), say("Summary.")]);
            let ev = run(b, t, c, std_assist());
            let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
            assert_eq!(board.get("F-APP-01").unwrap().status, want, "{:?}", notes(&ev));
            assert!(notes(&ev).iter().any(|n| n.contains("a tool kept repeating itself")), "{:?}", notes(&ev));
            assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "autopilot went on to the end instead of stopping: {:?}", ev.last());
            let _ = std::fs::remove_dir_all(&root);
        }
        // the LOOP message itself points to the alternative
        let t = Reg::with(&["k8s_logs"], 0, Box::new(|_, _| Ok("(the log is empty)".into())));
        let turns = vec![calls((0..3).map(|i| call(&format!("l{i}"), "k8s_logs", json!({"pod": "web-1"}))).collect()), say("ok")];
        let ev = run_plain(Scripted::new(turns), t);
        assert!(results(&ev).iter().any(|(_, _, t)| t.contains("LOOP:") && t.contains("deploy_status") && t.contains("reachable at")), "{:?}", results(&ev));
    }

    fn run_plain(b: Arc<Scripted>, t: Arc<Reg>) -> Vec<RunEvent> {
        let h = start_with(b, t, vec![Message::system("SYS"), Message::user("look at the logs")], RunLimits::default(), std_assist());
        drive(&h, |_| Some(Verdict::Approve))
    }

    #[test]
    fn what_a_step_learned_is_recalled_later_and_by_a_new_session_and_history_search_reads_it() {
        let store = kb();
        let (root, t) = world("hist", Box::new(|n, _| match n {
            "read_file" => Ok("registry address is 192.168.1.68:30500, namespace test2-dev".into()),
            o => Ok(format!("{o} ok")),
        }));
        let hist = History::new(store.clone(), "alice", "Web", Some("s_one"));
        let mut c = Context::new(&root, &root.join("FEATURES.md"), "ship the registry change");
        c.history = Some(hist.clone());
        let mut script = vec![adds_rows(&["Check the registry address", "Add a Dockerfile"]), say("Planned."), calls(vec![call("r", "read_file", json!({"path": "notes.txt"}))]), say("Noted the registry address.")];
        script.push(calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]));
        script.push(say("Wrote it."));
        script.push(say("Checked."));
        script.push(say("never"));
        let b = Scripted::new(script);
        let ev = run(b.clone(), t.clone(), c, std_assist());
        assert!(!ev.is_empty());
        // later steps were offered the recalled fact for the current row
        let users: Vec<String> = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect();
        assert!(users.iter().any(|u| u.contains("Earlier in this workspace (recalled") && u.contains("192.168.1.68:30500")), "{users:?}");
        assert!(users.iter().any(|u| u.contains("Running summary: Goal: ship the registry change")), "the running summary is pasted, replaced each step");
        assert!(users.iter().all(|u| u.matches("Running summary:").count() <= 1), "never appended twice");
        // history_search reads it, with scope rules
        let tools = crate::boardtools::ProjectTools::new(Reg::new(&["read_file"]), &root, "Web", &root.join("FEATURES.md")).with_history(hist);
        assert!(Tools::defs(&tools).iter().any(|d| d.spec.name == "history_search" && d.risk == Risk::Safe));
        let r = Tools::call(&tools, "history_search", &json!({"query": "registry address", "scope": "session"})).unwrap();
        assert!(r.contains("192.168.1.68:30500"), "{r}");
        let other = History::new(store.clone(), "alice", "Web", Some("s_two"));
        let t2 = crate::boardtools::ProjectTools::new(Reg::new(&["read_file"]), &root, "Web", &root.join("FEATURES.md")).with_history(other.clone());
        assert_eq!(Tools::call(&t2, "history_search", &json!({"query": "registry address", "scope": "session"})).unwrap(), "No matching history.");
        assert!(Tools::call(&t2, "history_search", &json!({"query": "registry address", "scope": "workspace"})).unwrap().contains("192.168.1.68:30500"));
        assert!(!Tools::defs(&crate::boardtools::ProjectTools::new(Reg::new(&["read_file"]), &root, "Web", &root.join("FEATURES.md"))).iter().any(|d| d.spec.name == "history_search"), "off without a history");
        // a new session of the same workspace starts with the earlier summary
        assert!(other.latest("summary").is_some_and(|s| s.text.contains("Session summary") || s.text.contains("Goal:")));
        let (root2, t3) = world("hist2", Box::new(plain));
        let mut c = Context::new(&root2, &root2.join("FEATURES.md"), "continue the shop work");
        c.history = Some(other.clone());
        let b = Scripted::new(vec![say("ok"), say("ok")]);
        run(b.clone(), t3, c, std_assist());
        let first = b.requests()[0].0.iter().rev().find(|m| m.role == Role::User).unwrap().content.clone();
        assert!(first.contains("a summary of a previous session") && first.contains("Goal: ship the registry change"), "{first}");
        // secrets are never in it
        other.record("tool", "", "deployed with password: hunter2hunter2X9");
        assert!(!other.search("deployed password", Scope::Workspace, None, 5).iter().any(|s| s.text.contains("hunter2")));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    #[test]
    fn the_running_summary_is_replaced_and_capped() {
        let mut s = autopilot::State::new("a goal", 0);
        let rows = autopilot::Rows { total: 3, done: 1, done_ids: vec!["F-1".into()], ..Default::default() };
        let titles: std::collections::BTreeMap<String, String> = [("F-1".to_string(), "Write server.js".to_string())].into();
        let files: Vec<String> = (0..500).map(|i| format!("some/very/long/path/to/file{i}.js")).collect();
        s.update_summary(&rows, &titles, &files, &["npm test".into()], &"error ".repeat(500), &"answer ".repeat(500));
        assert!(s.summary.chars().count() <= autopilot::SUMMARY_CAP && s.summary.contains("Done: F-1 Write server.js") && s.summary.contains("Last error:"), "{}", s.summary);
        let first = s.summary.clone();
        s.update_summary(&rows, &titles, &files[..2], &[], "", "short");
        assert_ne!(first, s.summary);
        assert!(s.summary.matches("Goal:").count() == 1, "replaced, not appended");
    }
}

// ------------------------------------------------------------------------------------------------ hard limits per step

fn cworld(tag: &str) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<Reg>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-caps-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let inner = Reg::with(&["read_file", "write_file", "deploy_status", "run_command"], 0, Box::new(|n, _| match n {
        "deploy_status" => Ok("Deployment web: 1/1 ready, Healthy".into()),
        o => Ok(format!("{o} ok")),
    }));
    (root.clone(), Arc::new(crate::boardtools::ProjectTools::new(inner.clone(), &root, "Web", &root.join("FEATURES.md"))), inner)
}

#[test]
fn a_code_row_is_verified_by_its_files_and_never_by_deploy_status() {
    let (root, t, inner) = cworld("code");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Create MP3 Player and Picture Viewer"})).unwrap();
    // nothing was written yet: the check fails briefly
    let e = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "done"})).unwrap_err();
    assert!(e.contains("no file was written for this row yet"), "{e}");
    // files exist and parse: the harness verifies and the row is done
    std::fs::write(root.join("player.html"), "<html><audio></audio></html>").unwrap();
    std::fs::write(root.join("server.js"), "console.log('hi')").unwrap();
    board_call(&t, "write_file", json!({"path": "player.html"})).unwrap();
    board_call(&t, "write_file", json!({"path": "server.js"})).unwrap();
    let r = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "done"})).unwrap();
    assert!(r.starts_with("updated F-APP-01"), "{r}");
    assert_eq!(row_status(&root, "F-APP-01").0, "done");
    let ran = inner.ran.lock().unwrap().join("\n");
    assert!(!ran.contains("deploy_status"), "a code row is never checked with deploy_status: {ran}");
    assert!(ran.contains("node") && ran.contains("--check"), "the .js file was syntax-checked: {ran}");
    // an empty file, a broken JSON file, or a missing named file fails
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Write config.json and data.json for the app"})).unwrap();
    std::fs::write(root.join("config.json"), "{ not json").unwrap();
    std::fs::write(root.join("data.json"), "").unwrap();
    Tools::begin_step(&*t);
    let e = board_call(&t, "board_update", json!({"id": "F-APP-02", "status": "done", "comment": "done"})).unwrap_err();
    assert!(e.contains("config.json is not valid JSON"), "{e}");
    std::fs::write(root.join("config.json"), "{\"a\": 1}").unwrap();
    // something changed (a file written): the next claim runs again, and now the empty file is named
    board_call(&t, "write_file", json!({"path": "notes.txt"})).unwrap();
    Tools::begin_step(&*t);
    let e = board_call(&t, "board_update", json!({"id": "F-APP-02", "status": "done", "comment": "done"})).unwrap_err();
    assert!(e.contains("data.json is empty"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_model_that_claims_done_eighty_times_is_stopped_after_one_call_and_gets_the_diagnosis_in_the_next_step() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-caps-{}-eighty", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let r2 = root.clone();
    let inner = Reg::with(&["read_file", "write_file", "deploy_status", "run_command"], 0, Box::new(move |n, a| {
        if n == "write_file" {
            // the fake tool really writes, so the harness check can see the file
            let p = a.get("path").and_then(Value::as_str).unwrap_or("x.txt");
            std::fs::write(r2.join(p), "<html><audio></audio></html>").unwrap();
        }
        Ok(format!("{n} ok"))
    }));
    let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")));
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "work on the features tasks");
    let claims = calls((0..80).map(|i| call(&format!("u{i}"), "board_update", json!({"id": "F-APP-01", "status": "done", "comment": format!("The check failed {i} times, so everything is ready for use, attempt {i}")}))).collect());
    let b = Scripted::new(vec![
        adds_rows(&["Create MP3 Player and Picture Viewer", "Add a Dockerfile"]),
        say("Planned."),
        claims,
        calls(vec![call("w", "write_file", json!({"path": "player.html"}))]),
        say("Wrote player.html."),
        calls(vec![call("d", "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "the page exists"}))]),
        say("Looked fine."),
        calls(vec![call("w2", "write_file", json!({"path": "Dockerfile"}))]),
        say("Wrote the Dockerfile."),
        say("Summary of the run."),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("work on the features tasks")], RunLimits { max_steps: 20, max_tool_calls: 400, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let updates: Vec<_> = results(&ev).into_iter().filter(|(n, _, _)| n == "board_update").collect();
    assert_eq!(updates.iter().filter(|(_, ok, _)| !ok).count(), 1, "the 80 claims ran once (it ran 80 times before): {updates:?}");
    assert!(updates.iter().all(|(_, _, text)| text.chars().count() <= 300), "board results stay short");
    // the next step carried the diagnosis and the instruction to make the fix
    let users: Vec<String> = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect();
    let with_diag = users.iter().find(|u| u.contains("The check for F-APP-01 FAILED. Diagnosis:")).expect("the diagnosis step");
    assert!(with_diag.contains("no file was written") && with_diag.contains("Do not describe a fix: make it with tools"), "{with_diag}");
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!((board.get("F-APP-01").unwrap().status.as_str(), board.get("F-APP-02").unwrap().status.as_str()), ("done", "done"), "fixed and verified, then the next row");
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{:?}", ev.last());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_fourth_board_update_in_one_step_is_not_run_and_parks_the_row() {
    let (root, t, _i) = cworld("cap4");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Polish the welcome text"})).unwrap();
    Tools::begin_step(&*t);
    for i in 0..3 {
        board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "partial", "comment": format!("working {i}")})).unwrap();
    }
    assert!(Tools::should_end_run(&*t).is_none());
    let r = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "partial", "comment": "working 3"})).unwrap();
    assert!(r.contains("parked as review") && r.contains("too often"), "{r}");
    assert!(Tools::should_end_run(&*t).is_some());
    assert_eq!(row_status(&root, "F-APP-01").0, "review");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_goal_about_many_board_rows_asks_which_ones_and_then_adopts_them_and_picks_actionable_rows_first() {
    let (root, t, _i) = cworld("many");
    // 20 rows the person wrote, the second depending on the first
    let mut text = crate::featureboard::EMPTY_BOARD.replace("| Started | Finished | Notes |", "| Phase | Started | Finished | Depends | Notes |").replace("|---|---|---|---|---|---|---|", "|---|---|---|---|---|---|---|---|---|");
    for i in 1..=20 {
        text.push_str(&format!("| F-X-{i:02} | Add feature {i} | planned | — | {} | | | {} | |\n", if i % 2 == 0 { "P1" } else { "P2" }, if i == 1 { "F-X-02".to_string() } else { String::new() }));
    }
    std::fs::write(root.join("FEATURES.md"), text).unwrap();
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "work on the features tasks please");
    let h = crate::autopilot::start(Scripted::new(vec![say("never")]), t.clone(), vec![Message::system("SYS"), Message::user("work on the features tasks please")], RunLimits::default(), std_assist(), c.clone());
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let q = final_text(&ev);
    assert!(q.starts_with("The board has 20 open rows. Which should I work through?") && q.contains("1. Work through all 20 rows") && q.contains("2. Only the 20 planned rows") && q.contains("Only phase P"), "{q}");
    let saved = crate::autopilot::State::load(&root.join(".iyakki/autopilot.json")).unwrap();
    assert!(saved.awaiting_scope && saved.status == "paused");
    // the answer (a click on a chip) arrives as the next message: only phase P1
    let mut c2 = c;
    c2.resume = true;
    c2.interjection = Some("Only phase P1 (10 rows)".into());
    let b = Scripted::new(vec![say("Looked at it."), say("Looked again."), say("Looked once more."), say("never")]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("Only phase P1")], RunLimits::default(), std_assist(), c2);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(notes(&ev).iter().any(|n| n.contains("working through 10 rows already on the board")), "{:?}", notes(&ev));
    let first = b.requests()[0].0.iter().rev().find(|m| m.role == Role::User).unwrap().content.clone();
    assert!(first.contains("row 1 of 10") || first.contains("F-X-02"), "the actionable row comes first: F-X-01 waits for F-X-02, so F-X-02 is picked: {first}");
    assert!(!first.contains("F-X-03"), "only phase P1 rows (the even ones) were adopted: {first}");
    assert!(notes(&ev).iter().any(|n| n.contains("row ") && n.contains(" of 10")), "{:?}", notes(&ev));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tests_and_files_can_be_listed_linked_and_a_row_whose_listed_tests_passed_counts_as_verified() {
    let (root, t, _i) = cworld("tests");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Create the cart", "tests": ["cart::tests::adds", "cart::tests::removes"], "files": ["src/cart.js"]})).unwrap();
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    let f = b.get("F-APP-01").unwrap();
    assert_eq!((f.tests.clone(), f.files.clone()), (vec!["cart::tests::adds".to_string(), "cart::tests::removes".to_string()], vec!["src/cart.js".to_string()]));
    // link more tests (kept, no duplicates), refused for an unknown row, shown as a diff before approval
    let pv = Tools::preview(&*t, "board_link_tests", &json!({"id": "F-APP-01", "tests": ["cart::tests::adds", "cart::tests::totals"]})).unwrap();
    assert!(pv.contains("+ 3 tests"), "{pv}");
    board_call(&t, "board_link_tests", json!({"id": "F-APP-01", "tests": "cart::tests::adds, cart::tests::totals"})).unwrap();
    assert_eq!(crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap()).get("F-APP-01").unwrap().test_count(), 3);
    assert!(board_call(&t, "board_link_tests", json!({"id": "F-NOPE", "tests": ["x"]})).unwrap_err().contains("no feature"));
    assert!(Tools::defs(&*t).iter().any(|d| d.spec.name == "board_link_tests" && d.risk == Risk::Ask), "linking asks like other board writes");
    // a failing listed test is not a verification; a passing run of all of them is (no source file needed)
    crate::testresults::save(&root, &crate::testresults::Results { ran_at: 1, tests: [("cart::tests::adds", crate::testresults::Outcome::Pass), ("cart::tests::removes", crate::testresults::Outcome::Fail), ("cart::tests::totals", crate::testresults::Outcome::Pass)].map(|(k, v)| (k.to_string(), v)).into() });
    Tools::begin_step(&*t);
    assert!(board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "done"})).is_err(), "no file exists and a listed test fails");
    crate::testresults::save(&root, &crate::testresults::Results { ran_at: 2, tests: [("cart::tests::adds", crate::testresults::Outcome::Pass), ("cart::tests::removes", crate::testresults::Outcome::Pass), ("cart::tests::totals", crate::testresults::Outcome::Pass)].map(|(k, v)| (k.to_string(), v)).into() });
    let r = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "done"})).unwrap();
    assert!(r.starts_with("updated F-APP-01"), "{r}");
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    assert!(crate::boardtools::workspace_prompt("Web", &root, "FEATURES.md").contains("list them in the row (`tests`"), "the prompt rule");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ honest summaries, fixing instead of advising

#[test]
fn the_summary_lists_only_what_was_executed_and_labels_advice_as_not_run() {
    let (root, t, _i) = cworld("honest");
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "add a dockerfile");
    let b = Scripted::new(vec![
        adds_rows(&["Add a Dockerfile"]),
        say("Planned."),
        // a delete the person refuses, a write that runs
        calls(vec![call("k", "run_command", json!({"program": "kubectl", "args": ["delete", "deployment", "old-one"]})), call("w", "write_file", json!({"path": "Dockerfile"}))]),
        say("Wrote it."),
        say("Summary: the Dockerfile is in place. To clean up you could run:\n$ kubectl delete deployment student-attendance-deployment -n test3-dev\nand then `docker rmi old-image`."),
    ]);
    let h = crate::autopilot::start(b, t, vec![Message::system("SYS"), Message::user("add a dockerfile")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |name| Some(if name == "run_command" { Verdict::Deny } else { Verdict::Approve }));
    let text = final_text(&ev);
    assert!(text.contains("Files changed: Dockerfile"), "{text}");
    assert!(!text.contains("Commands run"), "the refused kubectl was never run, and the prose is not scraped: {text}");
    assert!(text.contains("Suggested, not run: kubectl delete deployment student-attendance-deployment -n test3-dev; docker rmi old-image"), "{text}");
    assert!(crate::autopilot::suggested_commands("Run kubectl get pods", &["kubectl get pods".to_string()]).is_empty(), "a command that did run is not 'suggested'");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_check_hands_the_model_the_diagnosis_and_advice_without_action_is_pushed_to_act_until_the_row_is_verified() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-fixloop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fixed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let f2 = fixed.clone();
    let inner = Reg::with(&["read_file", "write_file", "deploy_build", "deploy_load", "k8s_apply", "deploy_status", "run_command"], 0, Box::new(move |n, _| match n {
        "write_file" => {
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok("write_file ok".into())
        }
        "deploy_status" if f2.load(std::sync::atomic::Ordering::SeqCst) => Ok("Deployment web: 1/1 ready, Healthy".into()),
        "deploy_status" => Ok("Deployment web: 0/1 ready, Unhealthy\nPod web-1: CreateContainerConfigError\nPod web-2: ImagePullBackOff (made-up image)\nDiagnosis: the manifest says runAsNonRoot: true but the image runs as root (its Dockerfile has no USER). Fix: add USER 10001 to the Dockerfile, rebuild, deploy_load and k8s_apply.\nNext: change the Dockerfile and redeploy.".into()),
        o => Ok(format!("{o} ok")),
    }));
    let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")));
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "deploy the app");
    let b = Scripted::new(vec![
        adds_rows(&["Deploy the app and check that it responds"]),
        say("Planned."),
        // step 2: apply works, the status is unhealthy, the model claims done: the check fails and the step ends
        calls(vec![call("a", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        calls(vec![call("s", "deploy_status", json!({"name": "web"}))]),
        calls(vec![call("c1", "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "applied"}))]),
        // step 3 starts with the diagnosis; the model first only gives advice
        say("You should add USER 10001 to the Dockerfile and rebuild the image, then run kubectl apply again."),
        // the harness pushes it to act: it fixes the Dockerfile, rebuilds, redeploys and claims done again
        calls(vec![call("w", "write_file", json!({"path": "Dockerfile"})), call("b", "deploy_build", json!({"dir": ".", "image": "web"})), call("l", "deploy_load", json!({"image": "web"})), call("a2", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        calls(vec![call("c2", "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "fixed the Dockerfile and redeployed"}))]),
        say("Fixed it."),
        say("Checked."),
        say("never"),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("deploy the app")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let users: Vec<String> = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect();
    let diag = users.iter().find(|u| u.contains("The check for F-APP-01 FAILED. Diagnosis:")).expect("the next step carries the diagnosis");
    assert!(diag.contains("CreateContainerConfigError") && diag.contains("ImagePullBackOff") && diag.contains("runAsNonRoot") && diag.contains("Do not describe a fix"), "{diag}");
    assert!(users.iter().any(|u| u.starts_with("SYSTEM NOTE: Autopilot is working and nothing has been done yet in this step") || u.starts_with("SYSTEM NOTE: you have the tools to do this")), "advice only is nudged to act: {users:?}");
    assert!(notes(&ev).iter().any(|n| n.contains("continuing")), "{:?}", notes(&ev));
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!(board.get("F-APP-01").unwrap().status, "done", "{:?}", board.get("F-APP-01"));
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"), "verified by the real check, not claimed");
    let text = final_text(&ev);
    assert!(text.contains("1 of 1 rows done (verified)") && text.contains("Files changed: Dockerfile") && text.contains("deploy_build; deploy_load; k8s_apply"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
    // the same model that never acts is stopped with the diagnosis in the blocked note, not finished quietly
    let (root2, t2, _i) = cworld("noact");
    let mut c2 = crate::autopilot::Context::new(&root2, &root2.join("FEATURES.md"), "deploy the app");
    c2.cfg.max_no_progress = 10;
    let advice = |n: u32| say(&format!("You should change the manifest and run kubectl apply (attempt {n})."));
    let b2 = Scripted::new(vec![adds_rows(&["Deploy the app"]), say("Planned."), advice(1), advice(2), advice(3), advice(4), advice(5), advice(6), advice(7), advice(8), say("Summary.")]);
    let ev = {
        let h = crate::autopilot::start(b2, t2, vec![Message::system("SYS"), Message::user("deploy the app")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c2);
        drive(&h, |_| Some(Verdict::Approve))
    };
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root2.join("FEATURES.md")).unwrap());
    let f = board.get("F-APP-01").unwrap();
    assert_eq!(f.status, "blocked", "{:?} {:?}", f, ev.last());
    assert!(f.notes.contains("kept describing fixes instead of making them"), "{}", f.notes);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn a_pretty_printed_more_tools_call_in_the_answer_is_run_and_never_shown_as_the_answer() {
    let text = "{\n  \"name\": \"more_tools\",\n  \"arguments\": {\n    \"group\": \"system\"\n  }\n}";
    let b = Scripted::new(vec![say(text), say("The system tools are loaded.")]);
    let ev = run(b.clone(), Reg::new(&["now", "run_command", "list_clis"]), "which command line tools exist", std_assist());
    assert_eq!(final_text(&ev), "The system tools are loaded.", "{ev:?}");
    assert!(results(&ev).iter().any(|(n, ok, _)| n == "more_tools" && *ok), "the call was executed: {:?}", results(&ev));
    let shown: String = ev.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    assert!(!shown.contains("\\\"group\\\": \\\"system\\\"") || results(&ev).iter().any(|(n, _, _)| n == "more_tools"));
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Final { text } if text.contains("more_tools"))), "never the final answer");
}

#[test]
fn the_same_title_is_one_row_and_sentences_are_acceptance_criteria_not_tests() {
    let (root, t, _i) = cworld("dedupe");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Deploy backend service", "tests": ["Backend deployment should be created successfully.", "deploy::tests::creates_the_deployment"], "acceptance": ["The pod is Running"]})).unwrap();
    let r = board_call(&t, "board_add", json!({"prefix": "APP", "title": "deploy Backend  service!", "tests": ["deploy::tests::rollout_is_ready"]})).unwrap();
    assert!(r.starts_with("already exists as F-APP-01") && r.contains("no second row"), "{r}");
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!(b.features().len(), 1, "one row, not two");
    let f = b.get("F-APP-01").unwrap();
    assert_eq!(f.tests, vec!["deploy::tests::creates_the_deployment".to_string(), "deploy::tests::rollout_is_ready".to_string()], "merged into the existing row");
    assert_eq!((f.test_count(), f.criteria.len()), (2, 2), "{f:?}");
    assert!(f.criteria.contains(&"Backend deployment should be created successfully.".to_string()) && f.criteria.contains(&"The pod is Running".to_string()));
    assert!(Tools::preview(&*t, "board_add", &json!({"prefix": "APP", "title": "Deploy backend service"})).unwrap().contains("already exists as F-APP-01"));
    // a sentence given as a test is filed as a criterion, a real name stays a test
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Add login", "tests": ["Users can log in with a password.", "tests/test_login.py::test_ok", "TestLogin"]})).unwrap();
    let f = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap()).get("F-APP-02").unwrap();
    assert_eq!((f.tests.clone(), f.criteria.clone()), (vec!["tests/test_login.py::test_ok".to_string(), "TestLogin".to_string()], vec!["Users can log in with a password.".to_string()]));
    for (s, want) in [("crate::mod::test_name", true), ("tests/a.py::test_b", true), ("TestAdd", true), ("cart.test.js", true), ("test_cart", true), ("cart adds an item", true), ("Backend deployment should be created successfully.", false), ("The pod is Running", false), ("Users can log in.", false), ("", false)] {
        assert_eq!(crate::featureboard::is_test_name(s), want, "{s:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_stale_manifest_and_a_root_image_are_fixed_by_editing_both_and_redeploying_after_the_model_first_only_describes() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let written = Arc::new(Mutex::new(std::collections::BTreeSet::<String>::new()));
    let w2 = written.clone();
    let inner = Reg::with(&["read_file", "write_file", "deploy_build", "deploy_load", "k8s_apply", "deploy_status", "run_command"], 0, Box::new(move |n, a| match n {
        "write_file" => {
            w2.lock().unwrap().insert(a.get("path").and_then(Value::as_str).unwrap_or("").to_string());
            Ok("write_file ok".into())
        }
        // healthy only when both the Dockerfile and the manifest were changed
        "deploy_status" if w2.lock().unwrap().contains("Dockerfile") && w2.lock().unwrap().contains("deploy/web.yaml") => Ok("Deployment web: 1/1 ready, Healthy".into()),
        "deploy_status" => Ok("Deployment web: 0/2 ready, Unhealthy\\nPod web-a: CreateContainerConfigError (runAsNonRoot with a root image)\\nPod web-b: ImagePullBackOff (image student-attendance-image does not exist)\\nDiagnosis: the Dockerfile has no USER and the manifest still points at a stale image name.\\nNext: add USER 10001 to the Dockerfile, fix the image in deploy/web.yaml, rebuild, deploy_load and k8s_apply.".into()),
        o => Ok(format!("{o} ok")),
    }));
    let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")));
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "deploy the app");
    let advice = |n: u32| say(&format!("You should add a USER line to the Dockerfile and correct the image name in the manifest, then run kubectl apply ({n})."));
    let b = Scripted::new(vec![
        adds_rows(&["Deploy the app and check that it responds"]),
        say("Planned."),
        calls(vec![call("c1", "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "applied"}))]),
        advice(1),
        // nudged, it only describes again; the second nudge is spent, the step ends without a change
        advice(2),
        advice(3),
        // the next step carries the diagnosis again; now it acts: both files, rebuild, redeploy, claim
        calls(vec![call("w1", "write_file", json!({"path": "Dockerfile"})), call("w2", "write_file", json!({"path": "deploy/web.yaml"})), call("b", "deploy_build", json!({"dir": ".", "image": "web"})), call("l", "deploy_load", json!({"image": "web"})), call("a", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        calls(vec![call("c2", "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "fixed both"}))]),
        say("Fixed it."),
        say("Checked."),
        say("never"),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("deploy the app")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!(board.get("F-APP-01").unwrap().status, "done", "{:?} {:?}", board.get("F-APP-01"), ev.last());
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"));
    let users: Vec<String> = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect();
    assert!(users.iter().filter(|u| u.contains("Diagnosis:") && u.contains("CreateContainerConfigError") && u.contains("ImagePullBackOff")).count() >= 2, "the diagnosis was handed over again after a step that only described");
    let text = final_text(&ev);
    assert!(text.contains("1 of 1 rows done (verified)") && text.contains("Files changed: Dockerfile, deploy/web.yaml") && text.contains("deploy_build; deploy_load; k8s_apply"), "{text}");
    assert!(text.contains("Suggested, not run: kubectl apply") || !text.contains("kubectl"), "advice is never listed as run: {text}");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ never ask the person to edit or run

const DELEGATING_REPLY: &str = "Please, open the Dockerfile and replace it:\n```\nFROM node:14 AS build\nWORKDIR /app\n```\nAfter that, run the deploy_build command again. Try the following command: deploy_build(dir=\".\", image=\"web\")";

#[test]
fn asking_the_person_to_edit_or_run_is_sent_back_once_in_ask_and_auto_mode_and_the_model_then_edits_the_file() {
    for auto in [false, true] {
        let b = Scripted::new(vec![
            say(DELEGATING_REPLY),
            calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]),
            say("I rewrote the first line of the Dockerfile and the test build can run again."),
        ]);
        let t = Reg::new(&["write_file", "deploy_build", "run_command", "now"]);
        let ev = run_auto(b.clone(), t.clone(), auto);
        assert_eq!(final_text(&ev), "I rewrote the first line of the Dockerfile and the test build can run again.", "auto={auto}");
        assert!(b.requests()[1].0.last().unwrap().content.contains("Do it yourself now with the tools; do not ask the person to edit or run anything"), "auto={auto}");
        assert!(t.ran.lock().unwrap().iter().any(|r| r.starts_with("write_file")), "the file was edited by a tool call");
        assert!(!ev.iter().any(|e| matches!(e, RunEvent::Final { text } if text.contains("Please, open the Dockerfile"))), "the request to the person is not the answer");
    }
    // at most two nudges, then the answer stands
    let b = Scripted::new(vec![say(DELEGATING_REPLY), say(DELEGATING_REPLY), say(DELEGATING_REPLY), say("never")]);
    run_auto(b.clone(), Reg::new(&["write_file", "deploy_build", "now"]), false);
    assert_eq!(b.requests().len(), 3);
}

#[test]
fn summaries_explanations_and_real_questions_are_not_nudged() {
    for text in [
        "I replaced line 1 of the Dockerfile with FROM node:18 and rebuilt the image; the test build now passes.",
        "The Dockerfile had no USER line, so the pod could not start. I added USER 10001 and redeployed.",
        "Which Node version should the Dockerfile use, 18 or 20?",
        "The service is reachable at http://192.168.1.5:30080.",
        "Summary: 2 of 3 rows done, one blocked because the registry is unreachable.",
    ] {
        let b = Scripted::new(vec![say(text)]);
        let ev = run_auto(b.clone(), Reg::new(&["write_file", "deploy_build", "run_command"]), false);
        assert_eq!(final_text(&ev), text);
        assert_eq!(b.requests().len(), 1, "{text}");
    }
    // a chat without file or command tools has nothing to do itself, so its advice stands
    let b = Scripted::new(vec![say(DELEGATING_REPLY)]);
    run_auto(b.clone(), Reg::new(&["now", "weather"]), false);
    assert_eq!(b.requests().len(), 1);
}

#[test]
fn a_failed_build_quotes_the_failing_step_and_the_numbered_dockerfile_and_autopilot_carries_it_to_the_next_step() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-buildfail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Dockerfile"), "FROM node:14 AS\nWORKDIR /app\nCOPY . .\n").unwrap();
    let inner = Reg::with(&["write_file", "deploy_build", "deploy_load", "k8s_apply", "read_file", "deploy_status"], 0, Box::new(|n, _| match n {
        "deploy_build" => Err("TEST BUILD FAILED\ndockerfile parse error on line 1: FROM requires either one or three arguments".into()),
        o => Ok(format!("{o} ok")),
    }));
    let t = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")));
    let e = Tools::call(&*t, "deploy_build", &json!({"dir": ".", "image": "web", "target": "test"})).unwrap_err();
    assert!(e.contains("Failing step: dockerfile parse error on line 1: FROM requires either one or three arguments"), "{e}");
    assert!(e.contains("1: FROM node:14 AS") && e.contains("2: WORKDIR /app") && e.contains("3: COPY . ."), "{e}");
    assert!(e.contains("Do not ask the person to edit anything"));
    // in Autopilot the next step opens with it, and the step requests carry the rule and the skills block of the prepared context
    let skills: crate::assistrun::Prepare = Arc::new(|_| vec![Message::system("SYS with SKILLBLOCK-MARKER the best practices"), Message::user("old chat")]);
    let mut a = std_assist();
    a.prepare = Some(skills);
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "build the app");
    let b = Scripted::new(vec![
        adds_rows(&["Write the app Dockerfile"]),
        say("Planned."),
        calls(vec![call("b", "deploy_build", json!({"dir": ".", "image": "web", "target": "test"}))]),
        say("The build failed."),
        calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]),
        say("Rewrote it."),
        say("Checked."),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("build the app")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, a, c);
    let _ = drive(&h, |_| Some(Verdict::Approve));
    let reqs = b.requests();
    assert!(reqs.iter().all(|(m, _)| m[0].content.contains("SKILLBLOCK-MARKER")), "the skills block is in every compact step request");
    let users: Vec<String> = reqs.iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).collect();
    assert!(users.iter().any(|u| u.contains("never ask me to open, edit, replace, create or save a file")), "the step header carries the rule");
    assert!(users.iter().any(|u| u.contains("The build FAILED. Failing step: dockerfile parse error on line 1") && u.contains("1: FROM node:14 AS")), "the next step quotes the real failure and the numbered Dockerfile: {users:?}");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ bugs (B- rows) with captured evidence

const CRASH_STATUS: &str = "Deployment web: 0/1 ready, Unhealthy\nPod web-7d9f8-abcde: CrashLoopBackOff\nDiagnosis: the container exits at start; see its logs.";

fn bug_world(tag: &str) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<std::sync::atomic::AtomicBool>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-bugs-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Dockerfile"), "FROM node:18\n").unwrap();
    let healthy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h2 = healthy.clone();
    let inner = Reg::with(&["read_file", "write_file", "deploy_build", "deploy_status", "k8s_apply"], 0, Box::new(move |n, _| match n {
        "deploy_status" if h2.load(std::sync::atomic::Ordering::SeqCst) => Ok("Deployment web: 1/1 ready, Healthy".into()),
        "deploy_status" => Ok(CRASH_STATUS.into()),
        "deploy_build" => Err("TEST BUILD FAILED\ndockerfile parse error on line 1: FROM requires either one or three arguments\npassword: hunter2hunter2X9".into()),
        o => Ok(format!("{o} ok")),
    }));
    (root.clone(), Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md"))), healthy)
}

#[test]
fn a_pod_that_cannot_start_becomes_one_bug_with_its_evidence_and_the_fix_resolves_it_with_the_passing_run() {
    let (root, t, healthy) = bug_world("auto");
    board_call(&t, "board_add", json!({"prefix": "APP", "title": "Deploy the app and check that it responds"})).unwrap();
    let r1 = board_call(&t, "deploy_status", json!({})).unwrap();
    assert!(r1.contains("Bug B-APP-01: the logs of every container are in .iyakki/bugs/B-APP-01/"), "{r1}");
    let r2 = board_call(&t, "deploy_status", json!({})).unwrap();
    assert!(r2.contains("Bug B-APP-01"), "the same failure is the same bug: {r2}");
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!(b.features().iter().filter(|f| f.is_bug()).count(), 1, "one bug for two sightings");
    let bug = b.get("B-APP-01").unwrap();
    assert!(bug.title.contains("CrashLoopBackOff") && bug.severity == "high" && bug.logs == vec![".iyakki/bugs/B-APP-01/".to_string()] && bug.notes.contains("seen 2 times"), "{bug:?}");
    let dir = root.join(".iyakki/bugs/B-APP-01");
    assert!(dir.join("failing-output.txt").is_file() && dir.join("summary.md").is_file() && dir.join("seen-2.txt").is_file());
    // the harness' own failed check on the feature row links to the same bug and the diagnosis says where the logs are
    let e = board_call(&t, "board_update", json!({"id": "F-APP-01", "status": "done", "comment": "applied"})).unwrap_err();
    assert!(e.contains("the check failed"), "{e}");
    let (id, diag) = Tools::take_diagnosis(&*t).unwrap();
    assert_eq!(id, "F-APP-01");
    assert!(diag.contains("Bug B-APP-01: logs of every container are in .iyakki/bugs/B-APP-01/"), "{diag}");
    assert_eq!(crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap()).features().iter().filter(|f| f.is_bug()).count(), 1);
    // the tracker never moves a bug by tool success, and the model cannot set one done without a check
    board_call(&t, "write_file", json!({"path": "Dockerfile"})).unwrap();
    assert_eq!(row_status(&root, "B-APP-01").0, "planned");
    // fixed: the next healthy status resolves it, keeps the passing run and sets the finish time
    healthy.store(true, std::sync::atomic::Ordering::SeqCst);
    board_call(&t, "deploy_status", json!({})).unwrap();
    let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    let bug = b.get("B-APP-01").unwrap();
    assert_eq!(bug.status, "done");
    assert!(!bug.finished.is_empty(), "{bug:?}");
    assert!(std::fs::read_to_string(dir.join("passing.log")).unwrap().contains("1/1 ready"));
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("B-APP-01"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_local_build_is_the_evidence_of_a_bug_report_without_any_pod_and_secrets_stay_out() {
    let (root, t, _h) = bug_world("local");
    let e = board_call(&t, "deploy_build", json!({"dir": ".", "image": "web", "target": "test"})).unwrap_err();
    assert!(e.contains("Failing step: dockerfile parse error on line 1"));
    let r = board_call(&t, "bug_report", json!({"title": "the test build fails on the first line", "severity": "med", "repro": "deploy_build target=test"})).unwrap();
    assert!(r.starts_with("B-APP-01 created"), "{r}");
    let dir = root.join(".iyakki/bugs/B-APP-01");
    let out = std::fs::read_to_string(dir.join("build-output.log")).unwrap();
    assert!(out.contains("dockerfile parse error on line 1") && !out.contains("hunter2"), "{out}");
    assert!(dir.join("summary.md").is_file());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_autopilot_summary_lists_open_bugs_apart_from_the_features() {
    let (root, t, _h) = bug_world("summary");
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "ship it");
    let b = Scripted::new(vec![
        adds_rows(&["Add a Dockerfile"]),
        say("Planned."),
        calls(vec![call("s", "deploy_status", json!({}))]),
        calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]),
        say("Wrote the Dockerfile."),
        say("Summary."),
    ]);
    let h = crate::autopilot::start(b, t, vec![Message::system("SYS"), Message::user("ship it")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let text = final_text(&ev);
    assert!(text.contains("1 of 1 rows done (verified)"), "bugs are not plan rows: {text}");
    assert!(text.contains("Open bugs (B-): B-APP-01"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ no run starts from the beginning

fn state_of(root: &std::path::Path) -> String {
    crate::projectstate::build(&crate::projectstate::Input { name: "Web", root, features: &root.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: None, history: None })
}

#[test]
fn a_second_run_in_the_workspace_starts_from_the_state_the_first_left_and_does_not_add_what_exists() {
    let (root, t, _i) = cworld("second");
    std::fs::write(root.join("package.json"), "{}").unwrap();
    // the first run: plans two rows and writes the first file
    let b1 = Scripted::new(vec![
        adds_rows(&["Write server.js with an HTTP server", "Add a Dockerfile"]),
        calls(vec![call("w", "write_file", json!({"path": "server.js"}))]),
        say("Wrote server.js."),
    ]);
    let h = start_with(b1, t.clone(), vec![Message::system(format!("SYS\\n\\n{}", state_of(&root))), Message::user("create a node web server")], RunLimits::default(), std_assist());
    assert!(matches!(drive(&h, |_| Some(Verdict::Approve)).last(), Some(RunEvent::Final { .. })));
    std::fs::write(root.join("server.js"), "console.log(1)").unwrap();
    // a new chat of the same workspace: its first request carries the state read from the disk just now
    let b2 = Scripted::new(vec![
        adds_rows(&["write server.js with an HTTP server!", "Add a Dockerfile"]),
        say("Continuing with the Dockerfile row."),
    ]);
    let state = state_of(&root);
    let h = start_with(b2.clone(), t, vec![Message::system(format!("SYS\\n\\n{state}")), Message::user("continue")], RunLimits::default(), std_assist());
    let ev = drive(&h, |_| Some(Verdict::Approve));
    let sys = &b2.requests()[0].0[0].content;
    assert!(sys.contains("PROJECT STATE") && sys.contains("Stack: JavaScript") && sys.contains("npm test"), "{sys}");
    assert!(sys.contains("2 rows, 1 done") && sys.contains("F-APP-02 [planned] Add a Dockerfile") && !sys.contains("F-APP-01 ["), "the done row is only counted, the unfinished one is listed: {sys}");
    assert!(sys.contains("server.js") && sys.contains("Do not recreate files or rows that exist"), "{sys}");
    // the model repeats its plan out of habit: no second copy of any row
    assert!(results(&ev).iter().filter(|(n, _, t)| n == "board_add" && t.contains("already exists as")).count() == 2, "{:?}", results(&ev));
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    assert_eq!(board.features().len(), 2, "no row was added twice");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_autopilot_goal_to_continue_adopts_the_rows_that_exist_and_every_step_carries_the_project_state() {
    let (root, t, _i) = cworld("continue");
    std::fs::write(root.join("go.mod"), "module x").unwrap();
    let c0 = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "build it");
    let b1 = Scripted::new(vec![adds_rows(&["Write main.go", "Add a Dockerfile"]), say("Planned."), calls(vec![call("w", "write_file", json!({"path": "main.go"}))]), say("Wrote main.go."), calls(vec![call("b", "board_update", json!({"id": "F-APP-02", "status": "partial", "comment": "started"}))]), say("Paused here.")]);
    let h = crate::autopilot::start(b1, t.clone(), vec![Message::system("SYS"), Message::user("build it")], RunLimits { max_steps: 3, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c0);
    let _ = drive(&h, |_| Some(Verdict::Approve));
    // later, a new run with the goal "continue": no new plan, the next unfinished row is picked and said
    let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "continue");
    let r2 = root.clone();
    c.state = Some(Arc::new(move || state_of(&r2)));
    let b2 = Scripted::new(vec![calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]), say("Wrote the Dockerfile."), say("Checked.")]);
    let h = crate::autopilot::start(b2.clone(), t, vec![Message::system("SYS"), Message::user("continue")], RunLimits { max_steps: 20, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(notes(&ev).iter().any(|n| n.contains("the board already has 2 rows (1 done); continuing with the next one")), "{:?}", notes(&ev));
    let first = b2.requests()[0].0.iter().rev().find(|m| m.role == Role::User).unwrap().content.clone();
    assert!(first.contains("The board already has rows from earlier work: do not plan again") && first.contains("F-APP-02") && !first.contains("Plan it first"), "{first}");
    assert!(first.contains("PROJECT STATE") && first.contains("Stack: Go") && first.contains("go test ./..."), "{first}");
    assert!(!results(&ev).iter().any(|(n, _, _)| n == "board_add"), "no row was added");
    assert_eq!(crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap()).features().len(), 2);
    assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{:?}", ev.last());
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ no questions after the goal is done

const REMOVED_PROSE: &str = "All resources in test-cla-dev were removed.\n\nWould you like me to:\n1. Keep it running\n2. Stop it\n3. Remove all\n\nPlease choose an option.";

fn teardown_world(tag: &str) -> (std::path::PathBuf, Arc<crate::boardtools::ProjectTools>, Arc<std::sync::atomic::AtomicBool>, Arc<Reg>) {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-tear-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let g2 = gone.clone();
    let mut inner_defs = registry(&["write_file", "deploy_status", "k8s_apply", "k8s_status", "k8s_teardown", "deploy_build", "deploy_load"]);
    inner_defs.push(def("ask_user", json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}), &["question"], Risk::Safe));
    let inner = Arc::new(Reg {
        defs: inner_defs,
        delay: Duration::ZERO,
        ran: Mutex::new(vec![]),
        out: Box::new(move |n, _| match n {
            "k8s_teardown" => {
                g2.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok("Removed the namespace test-cla-dev and everything in it.".into())
            }
            "k8s_status" if g2.load(std::sync::atomic::Ordering::SeqCst) => Ok("namespace: test-cla-dev does not exist yet (it is created on the first approved k8s_apply)".into()),
            "k8s_status" => Ok("namespace: test-cla-dev (exists)".into()),
            "deploy_status" => Ok("Deployment web: 1/1 ready, Healthy".into()),
            o => Ok(format!("{o} ok")),
        }),
    });
    (root.clone(), Arc::new(crate::boardtools::ProjectTools::new(inner.clone(), &root, "Web", &root.join("FEATURES.md"))), gone, inner)
}

fn go_ap(b: Arc<Scripted>, t: Arc<crate::boardtools::ProjectTools>, c: crate::autopilot::Context) -> Vec<RunEvent> {
    let h = crate::autopilot::start(b, t, vec![Message::system("SYS"), Message::user("go")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, std_assist(), c);
    drive(&h, |_| Some(Verdict::Approve))
}

#[test]
fn remove_all_tears_down_checks_the_namespace_is_gone_and_ends_with_one_summary_and_no_question() {
    let (root, t, _gone, inner) = teardown_world("removeall");
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "Remove all");
    // the model (as in the transcript) removes everything, then asks in prose, and would ask again with ask_user
    let b = Scripted::new(vec![
        calls(vec![call("t", "k8s_teardown", json!({}))]),
        say(REMOVED_PROSE),
        // sent back once ("the goal is complete"), it still tries ask_user, which is refused, and then writes the summary
        calls(vec![call("q", "ask_user", json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]}))]),
        say("All resources in test-cla-dev were removed and the namespace is gone."),
    ]);
    let ev = go_ap(b.clone(), t, c);
    let text = final_text(&ev);
    assert!(text.starts_with("All resources in test-cla-dev were removed and the namespace is gone.") && text.contains("Autopilot finished after 1 steps: 1 of 1 rows done (verified)"), "{text}");
    assert!(!text.contains("Would you like") && !text.contains("Please choose") && !text.contains("Keep it running"), "{text}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Model { text, .. } if text.contains("Would you like") || text.contains("Please choose"))), "the question is not shown in the chat either");
    assert!(!results(&ev).iter().any(|(n, ok, _)| n == "ask_user" && *ok), "ask_user never produced a question");
    assert!(results(&ev).iter().any(|(n, ok, text)| n == "ask_user" && !*ok && text.contains("the goal is complete")), "and it was refused");
    assert_eq!(inner.ran.lock().unwrap().iter().filter(|r| r.starts_with("k8s_teardown")).count(), 1);
    assert_eq!(b.requests().len(), 4, "teardown, the prose (sent back once), the refused ask_user, the summary; nothing after the namespace was seen gone");
    let board = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
    let row = board.get("F-APP-01").unwrap();
    assert_eq!(row.status, "done");
    assert!(crate::boardtrack::Tracker::new(&root, &root.join("FEATURES.md")).is_verified("F-APP-01"), "verified by the namespace being gone");
    assert!(crate::autopilot::goal_is_teardown("tear down the dev environment") && crate::autopilot::goal_is_teardown("clean up") && crate::autopilot::goal_is_teardown("remove everything") && !crate::autopilot::goal_is_teardown("deploy the app"));
    let _ = std::fs::remove_dir_all(&root);
}

fn deploy_script() -> Vec<Result<ModelTurn, String>> {
    vec![
        adds_rows(&["Deploy the app and check that it responds"]),
        say("Planned."),
        calls(vec![call("a", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        calls(vec![call("s", "deploy_status", json!({}))]),
        say("Applied and healthy."),
    ]
}

#[test]
fn the_teardown_offer_comes_after_a_verified_deploy_and_only_once_per_chat() {
    let (root, t, _gone, _inner) = teardown_world("offer");
    let ctx = |session: &str| {
        let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "deploy the app");
        c.session = session.to_string();
        c
    };
    let mut s = deploy_script();
    s.push(calls(vec![call("q", "ask_user", json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]}))]));
    let b = Scripted::new(s);
    let ev = go_ap(b.clone(), t.clone(), ctx("s_one"));
    let last_user = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).filter(|u| u.contains("AUTOPILOT final check")).last().unwrap();
    assert!(last_user.contains("Tear down the dev environment?") && last_user.contains("Keep it running, Stop it, Remove all"), "the offer is part of the final check: {last_user}");
    assert!(final_text(&ev).starts_with("Tear down the dev environment?\n\n1. Keep it running\n2. Stop it\n3. Remove all\n\nAutopilot finished"), "one question with its options, once: {}", final_text(&ev));
    assert_eq!(final_text(&ev).matches("Tear down the dev environment?").count(), 1);
    assert_eq!(crate::projectstate::noted(&root, "teardown_offered").as_deref(), Some("s_one"));
    // the same chat deploys again: no second offer, the final message is a summary, and a question is refused
    let _ = std::fs::remove_file(root.join("FEATURES.md"));
    let mut s2 = deploy_script();
    s2.push(say("Redeployed; everything is healthy.\n\nWould you like me to tear it down?"));
    s2.push(say("Redeployed; everything is healthy."));
    let b2 = Scripted::new(s2);
    let ev = go_ap(b2.clone(), t.clone(), ctx("s_one"));
    let final_check = b2.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).filter(|u| u.contains("AUTOPILOT final check")).last().unwrap();
    assert!(!final_check.contains("Tear down the dev environment?") && final_check.contains("a summary only. Do not ask anything"), "{final_check}");
    assert!(b2.requests().iter().any(|(m, _)| m.last().is_some_and(|x| x.content.starts_with("SYSTEM NOTE: the goal is complete. Write the summary only"))), "a closing question is sent back once");
    let text = final_text(&ev);
    assert!(text.contains("Redeployed; everything is healthy.") && !text.contains("Would you like"), "{text}");
    // another chat is offered again
    let _ = std::fs::remove_file(root.join("FEATURES.md"));
    let mut s3 = deploy_script();
    s3.push(say("Healthy."));
    let b3 = Scripted::new(s3);
    go_ap(b3.clone(), t, ctx("s_two"));
    assert!(b3.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).any(|u| u.contains("AUTOPILOT final check") && u.contains("Tear down the dev environment?")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn no_offer_when_nothing_was_deployed_or_the_namespace_is_gone() {
    // nothing was deployed (a file was written): the final message is a summary only
    let (root, t, _g, _i) = teardown_world("nodeploy");
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "add a dockerfile");
    let b = Scripted::new(vec![adds_rows(&["Add a Dockerfile"]), say("Planned."), calls(vec![call("w", "write_file", json!({"path": "Dockerfile"}))]), say("Wrote it."), say("Done: the Dockerfile exists.")]);
    go_ap(b.clone(), t, c);
    let fc = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).filter(|u| u.contains("AUTOPILOT final check")).last().unwrap();
    assert!(!fc.contains("Tear down the dev environment?") && fc.contains("summary only"), "{fc}");
    // deployed, but the namespace no longer exists
    let (root2, t2, gone, _i) = teardown_world("gone");
    gone.store(true, std::sync::atomic::Ordering::SeqCst);
    let c = crate::autopilot::Context::new(&root2, &root2.join("FEATURES.md"), "deploy the app");
    let mut s = deploy_script();
    s.push(say("Healthy."));
    let b = Scripted::new(s);
    go_ap(b.clone(), t2, c);
    let fc = b.requests().iter().filter_map(|(m, _)| m.iter().rev().find(|x| x.role == Role::User).map(|x| x.content.clone())).filter(|u| u.contains("AUTOPILOT final check")).last().unwrap();
    assert!(!fc.contains("Tear down the dev environment?"), "{fc}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

#[test]
fn prose_that_repeats_the_question_ask_user_asks_is_dropped_so_it_is_asked_once() {
    let reg = Arc::new(Reg {
        defs: vec![def("ask_user", json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}), &["question"], Risk::Safe)],
        delay: Duration::ZERO,
        ran: Mutex::new(vec![]),
        out: Box::new(|_, _| Ok("shown".into())),
    });
    let b = Scripted::new(vec![Ok(ModelTurn { text: REMOVED_PROSE.into(), calls: vec![call("q", "ask_user", json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]}))] })]);
    let ev = run(b, reg, "tidy up", std_assist());
    let shown: Vec<&String> = ev.iter().filter_map(|e| if let RunEvent::Model { text, .. } = e { Some(text) } else { None }).collect();
    assert!(shown.iter().all(|t| !t.contains("Keep it running") && !t.contains("Please choose")), "the prose lost the repeated question: {shown:?}");
    assert!(shown.iter().any(|t| t.contains("All resources in test-cla-dev were removed.")), "the summary stays: {shown:?}");
    assert_eq!(final_text(&ev), "Tear down the dev environment?\n\n1. Keep it running\n2. Stop it\n3. Remove all");
    // prose that says something else stays
    let reg2 = Arc::new(Reg { defs: vec![def("ask_user", json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}), &["question"], Risk::Safe)], delay: Duration::ZERO, ran: Mutex::new(vec![]), out: Box::new(|_, _| Ok("shown".into())) });
    let b = Scripted::new(vec![Ok(ModelTurn { text: "I found two registries on this machine.".into(), calls: vec![call("q", "ask_user", json!({"question": "Which registry?", "options": ["Docker Hub", "GHCR"]}))] })]);
    let ev = run(b, reg2, "deploy", std_assist());
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Model { text, .. } if text.contains("two registries"))));
}

// ------------------------------------------------------------------------------------------------ dependencies between rows

#[test]
fn board_tools_set_dependencies_refuse_unknown_ids_cycles_and_early_starts_and_report_the_ready_rows() {
    let (root, t) = proof_world("deps");
    let bu = |args: Value| {
        Tools::begin_step(&*t);
        board_call(&t, "board_update", args)
    };
    for title in ["Write the server", "Write the client", "Write the docs"] {
        board_call(&t, "board_add", json!({"prefix": "APP", "title": title})).unwrap();
    }
    bu(json!({"id": "F-APP-02", "depends": ["F-APP-01"]})).unwrap();
    let md = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
    assert!(md.contains("Depends") && md.lines().any(|l| l.contains("F-APP-02") && l.contains("F-APP-01")), "a Depends column of its own: {md}");
    let e = bu(json!({"id": "F-APP-03", "depends": ["F-NOPE-9"]})).unwrap_err();
    assert!(e.contains("F-NOPE-9 is not a row"), "{e}");
    let e = bu(json!({"id": "F-APP-01", "depends": "F-APP-02"})).unwrap_err();
    assert!(e.contains("cycle") && e.contains("F-APP-01 -> F-APP-02 -> F-APP-01"), "the path is named: {e}");
    assert!(bu(json!({"id": "F-APP-03", "depends": ["F-APP-03"]})).unwrap_err().contains("itself"));
    assert!(!std::fs::read_to_string(root.join("FEATURES.md")).unwrap().contains("F-APP-03 | Write the docs | planned | | F-APP"), "refused changes write nothing");
    // listing and the planner
    let l = board_call(&t, "board_list", json!({})).unwrap();
    assert!(l.lines().any(|x| x.starts_with("F-APP-02") && x.contains("waits: F-APP-01")), "{l}");
    let d = board_call(&t, "board_depends", json!({})).unwrap();
    assert!(d.contains("Ready to start now (2): F-APP-01, F-APP-03") && d.contains("F-APP-02 waits for F-APP-01"), "{d}");
    assert!(board_call(&t, "board_depends", json!({"id": "F-APP-01"})).unwrap().contains("Unblocks: F-APP-02"));
    // a row is not started before what it waits for is done, unless the person allows it (the approval shows a warning)
    let e = bu(json!({"id": "F-APP-02", "status": "partial", "comment": "starting"})).unwrap_err();
    assert!(e.contains("F-APP-02 waits for F-APP-01") && e.contains("override_deps"), "{e}");
    let args = json!({"id": "F-APP-02", "status": "partial", "comment": "starting", "override_deps": true});
    assert!(Tools::preview(&*t, "board_update", &args).unwrap().contains("WARNING: starting a row whose dependencies are not done"));
    bu(args).unwrap();
    bu(json!({"id": "F-APP-03", "status": "partial", "comment": "free row"})).unwrap();
    let names: Vec<String> = Tools::defs(&*t).into_iter().map(|d| d.spec.name).collect();
    assert!(names.contains(&"board_depends".to_string()) && names.contains(&"note_add".to_string()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn autopilot_starts_the_ready_row_even_when_a_waiting_row_comes_first_and_stops_when_only_waiting_rows_are_left() {
    let (root, t, _i) = cworld("readyrow");
    let board = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 2 · blocked 0 · deprecated 0 · total 2<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Depends | Notes |\n|---|---|---|---|---|---|\n| F-APP-01 | Write the client | planned |  | F-APP-02 |  |\n| F-APP-02 | Write the server | planned |  |  |  |\n";
    std::fs::write(root.join("FEATURES.md"), board).unwrap();
    let c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "work through the board rows");
    let b = Scripted::new(vec![calls(vec![call("w", "write_file", json!({"path": "server.js"}))]), say("Wrote the server.")]);
    let h = crate::autopilot::start(b.clone(), t.clone(), vec![Message::system("SYS"), Message::user("work through the board rows")], RunLimits { max_steps: 2, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c);
    let _ = drive(&h, |_| Some(Verdict::Approve));
    let first = b.requests()[0].0.iter().rev().find(|m| m.role == Role::User).unwrap().content.clone();
    assert!(first.contains("Work on F-APP-02") && !first.contains("Work on F-APP-01"), "the client waits for the server: {first}");
    // only a waiting row is left (its dependency is blocked): there is no next row to start
    let mut rows = crate::autopilot::Rows::default();
    rows.open = vec![("F-APP-01".into(), "client".into(), "planned".into())];
    rows.waiting.insert("F-APP-01".into(), vec!["F-APP-02".into()]);
    assert!(rows.next_row().is_none());
    rows.waiting.clear();
    assert_eq!(rows.next_row().unwrap().0, "F-APP-01");
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ project notes

#[test]
fn the_notes_open_every_state_block_pinned_first_and_the_cut_that_keeps_it_short_never_touches_them() {
    let (root, _t, _i) = cworld("notesblock");
    let me = crate::notes::Who::Person("Casey".into());
    crate::notes::edit(&root, |n| {
        n.add(crate::notes::NewNote::new("Newest idea", "recent", crate::notes::Kind::Decision), &me, 500)?;
        n.add(crate::notes::NewNote { pinned: true, ..crate::notes::NewNote::new("Never touch the migrations folder", "A past incident.", crate::notes::Kind::Constraint) }, &me, 100)?;
        Ok(())
    })
    .unwrap();
    // a board big enough that the state block is cut
    let mut md = String::from("# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 60 · blocked 0 · deprecated 0 · total 60<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Notes |\n|---|---|---|---|---|\n");
    for i in 1..=60 {
        md.push_str(&format!("| F-APP-{i:02} | {} | planned |  |  |\n", "a long title that takes room ".repeat(2)));
    }
    std::fs::write(root.join("FEATURES.md"), md).unwrap();
    let s = state_of(&root);
    let (head, pin, newest, stack) = (s.find("PROJECT STATE").unwrap(), s.find("Never touch the migrations").unwrap(), s.find("Newest idea").unwrap(), s.find("Stack:").unwrap());
    assert!(head < pin && pin < newest && newest < stack, "the notes sit right after the first line, pinned first: {}", &s[..400.min(s.len())]);
    assert!(s.contains("PROJECT CONTEXT (2 active, 1 pinned") && s.contains("A past incident."));
    assert!(s.contains("[… cut to fit …]") || s.len() < 6000, "the rest of the block is what gets cut");
    // another workspace's notes are not read: a task's tree has its own (empty) folder, the notes come from the workspace root
    let tree = root.join("elsewhere");
    std::fs::create_dir_all(&tree).unwrap();
    let inp = crate::projectstate::Input { name: "Web", root: &tree, features: &tree.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: None, history: None };
    assert!(!crate::projectstate::build(&inp).contains("PROJECT CONTEXT"));
    assert!(crate::projectstate::build_with_notes(&inp, &root).contains("Never touch the migrations"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_note_written_in_one_autopilot_step_applies_to_the_next_and_the_compacted_context_still_carries_every_note() {
    let (root, t, _i) = cworld("notestep");
    let me = crate::notes::Who::Person("Casey".into());
    crate::notes::edit(&root, |n| n.add(crate::notes::NewNote { pinned: true, ..crate::notes::NewNote::new("Use port 8081", "8080 is the registry.", crate::notes::Kind::Gotcha) }, &me, 100).map(|_| ())).unwrap();
    let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "build it");
    assert!(c.compact, "the context is rebuilt from the state each step, not resent");
    let r2 = root.clone();
    c.state = Some(Arc::new(move || state_of(&r2)));
    let b = Scripted::new(vec![
        adds_rows(&["Write server.js", "Write the readme"]),
        calls(vec![call("n", "note_add", json!({"title": "Server entry is server.js", "text": "Keep it the single entry point.", "kind": "decision"}))]),
        calls(vec![call("w", "write_file", json!({"path": "server.js"}))]),
        say("Wrote server.js."),
        calls(vec![call("w2", "write_file", json!({"path": "README.md"}))]),
        say("Wrote the readme."),
        say("Checked."),
    ]);
    let h = crate::autopilot::start(b.clone(), t, vec![Message::system("SYS"), Message::user("build it")], RunLimits { max_steps: 20, max_tool_calls: 50, ..RunLimits::default() }, std_assist(), c);
    let ev = drive(&h, |_| Some(Verdict::Approve));
    assert!(results(&ev).iter().any(|(n, ok, text)| n == "note_add" && *ok && text.contains("added N-002")), "{:?}", results(&ev));
    let reqs = b.requests();
    let steps: Vec<String> = reqs.iter().map(|r| r.0.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()).collect();
    // every request carries the person's pinned note at the top of the state block, whatever was compacted
    assert!(steps.iter().all(|s| s.contains("PROJECT CONTEXT") && s.contains("Use port 8081")), "{steps:?}");
    // the agent's own note appears from the next step on, labelled as an agent's note, not as an instruction
    let later = steps.iter().skip(2).find(|s| s.contains("Server entry is server.js")).expect("the note applies to a later step");
    assert!(later.contains("context written earlier by agents") && later.find("Use port 8081").unwrap() < later.find("Server entry is server.js").unwrap(), "{later}");
    assert!(!steps[0].contains("Server entry is server.js"));
    // the file shows it for review
    let (n, _) = crate::notes::load(&root);
    assert!(n.get("N-002").unwrap().is_new_by_agent());
    // the chat's Autopilot is an agent of the workspace too: named, with its steps and the note it added, done at the end
    for _ in 0..200 {
        if crate::assign::load(&root).get("autopilot:run").is_some_and(|a| a.state != crate::assign::AState::Working) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let d = crate::assign::load(&root);
    let a = d.get("autopilot:run").expect("registered");
    assert_eq!((a.name.as_str(), a.kind.as_str(), a.notes.clone()), ("Agent 1", "autopilot", vec!["N-002".to_string()]));
    assert!(!a.state.active(), "when the run is over the agent is not shown as working: {:?} {}", a.state, a.last);
    assert!(a.steps.iter().any(|s| s.kind == crate::assign::StepKind::Plan) && a.steps.iter().any(|s| s.kind == crate::assign::StepKind::File && s.text == "wrote server.js"), "{:?}", a.steps);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_row_that_keeps_failing_leaves_one_note_for_the_next_agent_and_not_a_second_for_the_same_incident() {
    let (root, _t, _i) = cworld("failnote");
    let a = crate::notes::auto_note(&root, "Autopilot", crate::notes::NewNote::new("F-APP-01 keeps failing", "not finished after 3 attempts; last diagnosis: BackOff", crate::notes::Kind::Gotcha));
    assert!(a.is_some());
    assert!(crate::notes::auto_note(&root, "Autopilot", crate::notes::NewNote::new("F-APP-01 keeps failing", "again", crate::notes::Kind::Gotcha)).is_none(), "deduplicated by title");
    assert!(crate::notes::auto_note(&root, "Autopilot", crate::notes::NewNote::new("F-APP-01 keeps failing", "x", crate::notes::Kind::Gotcha)).is_none(), "the same row is the same incident");
    assert!(crate::notes::auto_note(&root, "Autopilot", crate::notes::NewNote::new("F-APP-02 keeps failing", "x", crate::notes::Kind::Gotcha)).is_some(), "another row is another incident");
    let (n, _) = crate::notes::load(&root);
    assert_eq!((n.notes.len(), n.notes[0].author.as_str(), n.notes[0].by_agent), (2, "Autopilot", true));
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ agents on one workspace

#[test]
fn two_agents_writing_one_file_are_flagged_and_the_one_the_person_queued_waits_while_notes_carry_the_agents_name() {
    let (root, _t, inner) = cworld("agents2");
    let (ka, kb) = ("task-a", "task-b");
    crate::assign::update(&root, |d| {
        d.register(ka, "task", "Claude Code (sonnet)", &["F-APP-01".to_string()], ka, 1);
        d.register(kb, "task", "built-in", &["F-APP-02".to_string()], kb, 1);
    });
    let mk = |key: &str, name: &str| crate::boardtools::ProjectTools::new(inner.clone(), &root, "Web", &root.join("FEATURES.md")).with_notes(name, &root).with_agent(key);
    let (a, b) = (mk(ka, "Agent 1"), mk(kb, "Agent 2"));
    board_call(&a, "write_file", json!({"path": "src/lib.rs", "content": "a"})).unwrap();
    board_call(&b, "write_file", json!({"path": "docs/x.md", "content": "b"})).unwrap();
    assert!(crate::assign::load(&root).live_conflicts().is_empty());
    // the second agent touches the first one's file: it goes on (the default), and the conflict is on record for the badge
    board_call(&b, "write_file", json!({"path": "src/lib.rs", "content": "b"})).unwrap();
    let d = crate::assign::load(&root);
    assert_eq!(d.live_conflicts().len(), 1);
    assert_eq!((d.conflicts[0].holder_name.as_str(), d.conflicts[0].other_name.as_str()), ("Agent 1", "Agent 2"));
    assert!(d.get(kb).unwrap().steps.iter().any(|s| s.text.contains("also being changed by Agent 1")));
    // the person chooses to queue the second agent: its next write of that file is refused with the reason, other files are fine
    crate::assign::update(&root, |d| d.resolve_conflict("src/lib.rs", kb, crate::assign::Choice::Queue));
    let e = board_call(&b, "write_file", json!({"path": "src/lib.rs", "content": "c"})).unwrap_err();
    assert!(e.contains("being changed by Agent 1") && e.contains("asked to wait"), "{e}");
    board_call(&b, "write_file", json!({"path": "docs/y.md", "content": "c"})).unwrap();
    board_call(&a, "write_file", json!({"path": "src/lib.rs", "content": "d"})).unwrap();
    // timeline and notes
    board_call(&a, "note_add", json!({"title": "lib.rs is generated", "text": "edit the template", "kind": "gotcha"})).unwrap();
    let d = crate::assign::load(&root);
    let steps = &d.get(ka).unwrap().steps;
    assert!(steps.iter().any(|s| s.kind == crate::assign::StepKind::File && s.text == "wrote src/lib.rs"));
    assert_eq!(d.get(ka).unwrap().notes, ["N-001"]);
    assert_eq!(crate::notes::load(&root).0.get("N-001").unwrap().author, "Agent 1");
    // the holder finishes: the queue is over
    crate::assign::update(&root, |d| d.set_state(ka, crate::assign::AState::Done, "", 9));
    board_call(&b, "write_file", json!({"path": "src/lib.rs", "content": "e"})).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------------------------------------ the live approval mode

mod live_mode {
    use super::*;
    use crate::livemode::{LiveMode, Mode};

    /// Events of a run until it ends; `on` sees each one and may answer it or switch the mode.
    fn watch(h: &RunHandle, mut on: impl FnMut(&RunEvent, &RunHandle)) -> Vec<RunEvent> {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(e) = h.events.recv_timeout(Duration::from_millis(20)) {
                on(&e, h);
                let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
                out.push(e);
                if done {
                    return out;
                }
            }
        }
        panic!("the run did not end: {out:?}");
    }

    fn decided(ev: &[RunEvent]) -> Vec<String> {
        ev.iter().filter_map(|e| if let RunEvent::Decision { id, approved, by, .. } = e { Some(format!("{id}:{}:{by}", if *approved { "yes" } else { "no" })) } else { None }).collect()
    }

    #[test]
    fn a_waiting_card_is_approved_by_switching_to_auto_and_an_always_ask_card_keeps_waiting() {
        let t = Reg::new(&["write_file", "k8s_delete"]);
        let b = Scripted::new(vec![calls(vec![call("a", "write_file", json!({"path": "app.js"}))]), calls(vec![call("b", "k8s_delete", json!({"path": "pods/x"}))]), say("done")]);
        let cell = LiveMode::new(Mode::Ask);
        let limits = RunLimits { live: Some(cell.clone()), ..Default::default() };
        let h = start_with(b, t.clone(), vec![Message::system("SYS"), Message::user("go")], limits, std_assist());
        let mut hard_waited = false;
        let ev = watch(&h, |e, h| {
            if let RunEvent::Approval { id, name, .. } = e {
                if name == "write_file" {
                    // the person switches the chip instead of answering
                    assert!(cell.set(Mode::Auto));
                } else {
                    // already in Auto: an always-ask call still waits for the person
                    std::thread::sleep(Duration::from_millis(400));
                    hard_waited = h.events.try_recv().is_err();
                    h.decide(id, Verdict::Deny);
                }
            }
        });
        assert_eq!(final_text(&ev), "done");
        assert_eq!(decided(&ev), vec!["a:yes:mode-switch", "b:no:user"], "{ev:?}");
        assert!(hard_waited, "the k8s_delete card stayed open after the switch");
        assert!(notes(&ev).iter().any(|n| n == "write_file: approved by switching to Auto-approve safe"), "{:?}", notes(&ev));
        assert_eq!(t.ran.lock().unwrap().len(), 1, "only the write ran");
    }

    #[test]
    fn switching_back_to_ask_first_lets_the_running_tool_finish_and_the_next_risky_call_asks() {
        let t = Reg::with(&["write_file"], 300, Box::new(|n, _| Ok(format!("{n} ok"))));
        let b = Scripted::new(vec![calls(vec![call("a", "write_file", json!({"path": "one.js"}))]), calls(vec![call("b", "write_file", json!({"path": "two.js"}))]), say("done")]);
        let cell = LiveMode::new(Mode::Auto);
        let h = start_with(b, t.clone(), vec![Message::system("SYS"), Message::user("go")], RunLimits { live: Some(cell.clone()), ..Default::default() }, std_assist());
        let mut asked = vec![];
        let ev = watch(&h, |e, h| match e {
            // the first write was auto-approved and is running now: the person switches to Ask first
            RunEvent::Decision { id, .. } if id == "a" => {
                cell.set(Mode::Ask);
            }
            RunEvent::Approval { id, name, .. } => {
                asked.push(name.clone());
                h.decide(id, Verdict::Approve);
            }
            _ => {}
        });
        assert_eq!(final_text(&ev), "done");
        assert_eq!(decided(&ev), vec!["a:yes:auto-approved", "b:yes:user"]);
        assert_eq!(asked, vec!["write_file"], "the second write asked");
        assert!(results(&ev).iter().all(|r| r.1), "the running write finished: {:?}", results(&ev));
        assert_eq!(t.ran.lock().unwrap().len(), 2);
    }

    /// Project tools with a detached job that keeps running.
    struct WithJob(Arc<crate::boardtools::ProjectTools>);
    impl Tools for WithJob {
        fn defs(&self) -> Vec<ToolDef> {
            self.0.defs()
        }
        fn call(&self, n: &str, a: &Value) -> Result<String, String> {
            Tools::call(self.0.as_ref(), n, a)
        }
        fn describe(&self, n: &str, a: &Value) -> String {
            self.0.describe(n, a)
        }
        fn unresolved_work(&self) -> Vec<String> {
            self.0.unresolved_work()
        }
        fn begin_step(&self) {
            self.0.begin_step()
        }
        fn should_end_run(&self) -> Option<String> {
            self.0.should_end_run()
        }
        fn jobs_running(&self) -> Vec<String> {
            vec!["image build (j1)".into()]
        }
    }

    #[test]
    fn autopilot_switched_off_mid_step_finishes_the_step_then_stops_with_a_summary_and_leaves_the_job_running() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-live-ap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let inner = Reg::with(&["read_file", "write_file"], 0, Box::new(|n, _| Ok(format!("{n} ok"))));
        let pt = Arc::new(crate::boardtools::ProjectTools::new(inner, &root, "Web", &root.join("FEATURES.md")));
        let audit = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut c = crate::autopilot::Context::new(&root, &root.join("FEATURES.md"), "build the shop");
        let a = audit.clone();
        c.audit = Some(Arc::new(move |k, d| a.lock().unwrap().push(format!("{k}: {d}"))));
        // every model call takes 300 ms, so the switch lands inside the first step
        let b = Scripted::streaming(vec![calls(vec![call("a0", "board_add", json!({"prefix": "APP", "title": "Write the shop page"})), call("a1", "board_add", json!({"prefix": "APP", "title": "Add a Dockerfile"}))]), say("Planned two rows."), say("never")], 300, vec![]);
        let cell = LiveMode::new(Mode::Autopilot);
        let t0 = Instant::now();
        let h = crate::autopilot::start(b.clone(), Arc::new(WithJob(pt)), vec![Message::system("SYS"), Message::user("build the shop")], RunLimits { live: Some(cell.clone()), ..Default::default() }, std_assist(), c);
        std::thread::sleep(Duration::from_millis(100));
        cell.set(Mode::Ask);
        let ev = watch(&h, |e, h| {
            if let RunEvent::Approval { id, .. } = e {
                // Ask first now: the rest of the step asks, and the person allows it
                h.decide(id, Verdict::Approve);
            }
        });
        let text = final_text(&ev);
        assert!(text.contains("Autopilot switched off: stopped after step 1") && text.contains("Still running (detached, see the Jobs strip): image build (j1)") && text.contains("Resume"), "{text}");
        assert_eq!(b.requests().len(), 2, "the step finished, no next step began");
        assert!(t0.elapsed() < Duration::from_secs(10), "it did not wait for the job");
        let board = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
        assert!(board.contains("Write the shop page") && board.contains("Add a Dockerfile"), "the step's board work is whole: {board}");
        let saved = crate::autopilot::State::load(&root.join(".iyakki/autopilot.json")).unwrap();
        assert_eq!((saved.status.as_str(), saved.reason.as_str()), ("paused", "Autopilot switched off by the person"));
        assert!(audit.lock().unwrap().iter().any(|l| l.starts_with("autopilot.pause: switched off at step 1")), "{:?}", audit.lock().unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_bridge_gate_reads_the_live_mode_for_every_permission_prompt_and_while_a_card_waits() {
        use crate::extagent::bridge::{Correlator, Decisions, Gate};
        let ws = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-live-gate-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let (etx, erx) = std::sync::mpsc::channel();
        let (dtx, drx) = std::sync::mpsc::channel();
        let mut gate = Gate::new(Reg::new(&["read_file"]), false, etx, Decisions::new(drx), Arc::new(Correlator::default()), Arc::new(std::sync::atomic::AtomicBool::new(false)), Duration::from_secs(20), "Claude Code", Arc::new(|_, _| {}), &ws, 12_000);
        let cell = LiveMode::new(Mode::Ask);
        gate.live = Some(cell.clone());
        gate.allow_auto = true;
        let gate = Arc::new(gate);
        let prompt = |id: &str| json!({"tool_name": "Write", "input": {"file_path": ws.join("a.txt").display().to_string(), "content": "x"}, "tool_use_id": id});
        let ask_in_thread = |p: Value| {
            let g = gate.clone();
            std::thread::spawn(move || g.approve(&p))
        };
        let next = |want: &str| -> RunEvent {
            let t = Instant::now();
            loop {
                let e = erx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("no {want} event"));
                let hit = match &e {
                    RunEvent::Approval { .. } => want == "approval",
                    RunEvent::Decision { .. } => want == "decision",
                    RunEvent::Note { .. } => want == "note",
                    _ => false,
                };
                if hit {
                    return e;
                }
                assert!(t.elapsed() < Duration::from_secs(5));
            }
        };
        // Ask first: the first prompt is a card the person answers
        let th = ask_in_thread(prompt("p1"));
        assert!(matches!(next("approval"), RunEvent::Approval { id, .. } if id == "p1"));
        dtx.send(("p1".into(), Verdict::Approve)).unwrap();
        assert_eq!(th.join().unwrap()["behavior"], "allow");
        assert!(matches!(next("decision"), RunEvent::Decision { by, .. } if by == "user"));
        // switched to Auto-approve safe between two prompts: the next one runs without a card
        cell.set(Mode::Auto);
        assert_eq!(gate.approve(&prompt("p2"))["behavior"], "allow");
        assert!(matches!(next("decision"), RunEvent::Decision { id, by, .. } if id == "p2" && by == "auto-approved"));
        // back to Ask first: the next prompt asks; switching to Auto while its card waits approves it
        cell.set(Mode::Ask);
        let th = ask_in_thread(prompt("p3"));
        assert!(matches!(next("approval"), RunEvent::Approval { id, .. } if id == "p3"));
        cell.set(Mode::Auto);
        assert_eq!(th.join().unwrap()["behavior"], "allow");
        assert!(matches!(next("note"), RunEvent::Note { text } if text.contains("Write: approved by switching to Auto-approve safe")));
        assert!(matches!(next("decision"), RunEvent::Decision { id, by, .. } if id == "p3" && by == "mode-switch"));
        // a destructive shell command always asks, whatever the mode
        let th = ask_in_thread(json!({"tool_name": "Bash", "input": {"command": "rm -rf build"}, "tool_use_id": "p4"}));
        assert!(matches!(next("approval"), RunEvent::Approval { id, .. } if id == "p4"));
        std::thread::sleep(Duration::from_millis(300));
        dtx.send(("p4".into(), Verdict::Deny)).unwrap();
        assert_eq!(th.join().unwrap()["behavior"], "deny");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn an_agent_without_allow_auto_keeps_asking_after_a_switch_and_says_why_once() {
        use crate::extagent::bridge::{Correlator, Decisions, Gate};
        let ws = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-live-gate2-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let (etx, erx) = std::sync::mpsc::channel();
        let (_dtx, drx) = std::sync::mpsc::channel::<(String, Verdict)>();
        let mut gate = Gate::new(Reg::new(&["read_file"]), false, etx, Decisions::new(drx), Arc::new(Correlator::default()), Arc::new(std::sync::atomic::AtomicBool::new(false)), Duration::from_millis(200), "Copilot", Arc::new(|_, _| {}), &ws, 12_000);
        gate.live = Some(LiveMode::new(Mode::Auto));
        gate.allow_auto = false;
        let p = json!({"tool_name": "Write", "input": {"file_path": ws.join("b.txt").display().to_string(), "content": "x"}});
        assert_eq!(gate.approve(&p)["behavior"], "deny", "nobody answered the card: it timed out");
        assert_eq!(gate.approve(&p)["behavior"], "deny");
        let ev: Vec<RunEvent> = erx.try_iter().collect();
        assert_eq!(ev.iter().filter(|e| matches!(e, RunEvent::Approval { .. })).count(), 2, "both asked");
        assert_eq!(ev.iter().filter(|e| matches!(e, RunEvent::Note { text } if text.contains("Auto is off for Copilot"))).count(), 1, "said once");
        let _ = std::fs::remove_dir_all(&ws);
    }
}

// ------------------------------------------------------------------------------------------------ camera tools

mod camera {
    use super::*;
    use crate::agent::UserTools;
    use crate::camtools::{CameraBackend, CountryInfo, PassportOut};
    use crate::services::Services;
    use iyakki_sec::policy::Role as PolicyRole;
    use std::path::{Path, PathBuf};

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    #[derive(Default)]
    struct Cam(Mutex<u32>);
    impl CameraBackend for Cam {
        fn cameras(&self) -> Vec<String> {
            vec!["video0: test".into()]
        }
        fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
            let mut n = self.0.lock().unwrap();
            *n += 1;
            let p = dir.join(format!("photo-{n}.jpg"));
            std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xD9]).map_err(|e| e.to_string())?;
            Ok(p)
        }
        fn record_start(&self, _: &Path, _: u32) -> Result<PathBuf, String> {
            Err("no".into())
        }
        fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String> {
            Err("no".into())
        }
        fn recording(&self) -> Option<(PathBuf, f64)> {
            None
        }
        fn passport(&self, _: &Path, _: &str, _: &str) -> Result<PassportOut, String> {
            Err("no".into())
        }
        fn countries(&self) -> Vec<CountryInfo> {
            vec![]
        }
    }

    fn rig(tag: &str) -> (Arc<UserTools>, Arc<Services>, std::path::PathBuf) {
        let dir = std::fs::canonicalize({
            let d = std::env::temp_dir().join(format!("iyakki-camrun-{}-{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("home")).unwrap();
            d
        })
        .unwrap();
        let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        svc.camera.set_home_root(&dir.join("home"));
        svc.camera.set_backend(Some(Arc::new(Cam::default())));
        let mut policy = PolicyRole::User.template();
        let home = dir.join("home");
        policy.fs_read = vec![home.display().to_string()];
        policy.fs_write = vec![home.display().to_string()];
        let tools = UserTools::new("alice", policy, svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(&home);
        (Arc::new(tools), svc, home)
    }

    #[test]
    fn a_photo_by_the_assistant_asks_first_and_allow_for_five_minutes_is_the_longest_grant() {
        let (t, svc, home) = rig("ask");
        assert!(crate::agent::hard_ask("camera_photo", &json!({})).is_some(), "never auto-approved, never a session grant");
        assert!(crate::agent::hard_ask("camera_list", &json!({})).is_none());
        let b = Scripted::new(vec![calls(vec![call("c1", "camera_photo", json!({}))]), calls(vec![call("c2", "camera_photo", json!({}))]), say("two photos")]);
        let h = start_with(b, t.clone(), vec![Message::system("S"), Message::user("take my photo")], RunLimits::default(), std_assist());
        let mut asked = Vec::new();
        let ev = drive(&h, |name| {
            asked.push(name.to_string());
            Some(Verdict::Always)
        });
        assert_eq!(asked, vec!["camera_photo"], "asked once; the second photo falls in the 5 minutes");
        let r = results(&ev);
        assert_eq!(r.iter().filter(|r| r.1 && r.2.contains("IMAGE: ")).count(), 2, "{r:?}");
        assert!(svc.camera.granted("alice"));
        assert_eq!(std::fs::read_dir(home.join("alice/Pictures")).unwrap().count(), 2);
        svc.camera.revoke("alice");
        assert!(!t.pre_approved("camera_photo"));
        // denied: no photo
        let (t2, _, home2) = rig("deny");
        let b = Scripted::new(vec![calls(vec![call("c", "camera_photo", json!({}))]), say("ok")]);
        let h = start_with(b, t2, vec![Message::system("S"), Message::user("photo")], RunLimits::default(), std_assist());
        let ev = drive(&h, |_| Some(Verdict::Deny));
        assert!(results(&ev)[0].2.contains("denied"));
        assert!(!home2.join("alice/Pictures").exists());
    }
}
