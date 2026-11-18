//! The assistant's run loop over a real socket (F-ASSIST-11, F-ASSIST-19, F-ASSIST-22, F-ASSIST-27): the real `ProviderChat` with the
//! real streaming HTTP client talks to a small model server of this file on the loopback address, which speaks the wire format of
//! the OpenAI-compatible endpoint (what Ollama and Lemonade serve: server-sent events) and of Anthropic's messages endpoint. The
//! run loop (`start_with`) sits on top, so what is checked is what the server received and what the person would see, with no fake
//! between the chat call and the wire. No real provider, key or model is involved.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{start_with, Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::assistrun::Assist;
use crate::chat::{Message, ToolSpec};
use crate::providers::{Kind, ProviderCfg, ProviderChat, ProviderClient};
use crate::toolblock::ReminderCtx;
use crate::vault::Secrets;
use iyakki_proxy::Upstream;

#[derive(Clone, Debug)]
pub(crate) struct Req {
    pub(crate) path: String,
    pub(crate) body: String,
}

fn read_req(s: &mut TcpStream) -> Option<Req> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let path = head.lines().next()?.split(' ').nth(1)?.to_string();
    let len = head.lines().filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok()).unwrap_or(0);
    while buf.len() < head_end + len {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Some(Req { path, body: String::from_utf8_lossy(&buf[head_end..(head_end + len).min(buf.len())]).into_owned() })
}

/// A model server: `answer(n, request, stream)` is called for the n-th request and writes the reply however slowly it likes.
pub(crate) fn serve(answer: impl Fn(usize, &Req, &mut TcpStream) + Send + Sync + 'static) -> (u16, Arc<Mutex<Vec<Req>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let (log, answer) = (seen.clone(), Arc::new(answer));
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let (log, answer) = (log.clone(), answer.clone());
            std::thread::spawn(move || {
                let Some(r) = read_req(&mut s) else { return };
                let n = {
                    let mut g = log.lock().unwrap();
                    g.push(r.clone());
                    g.len() - 1
                };
                answer(n, &r, &mut s);
            });
        }
    });
    (port, seen)
}

pub(crate) fn sse_head(s: &mut TcpStream) {
    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n");
    let _ = s.flush();
}

/// One server-sent event; false when the client has gone away.
pub(crate) fn event(s: &mut TcpStream, name: Option<&str>, data: &str) -> bool {
    let text = match name {
        Some(n) => format!("event: {n}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    };
    s.write_all(text.as_bytes()).and_then(|_| s.flush()).is_ok()
}

pub(crate) fn delta(text: &str) -> String {
    json!({"choices": [{"delta": {"content": text}}]}).to_string()
}

pub(crate) fn backend(kind: Kind, port: u16, model: &str) -> Arc<ProviderChat> {
    std::env::set_var("IYAKKI_TEST_WIRE_KEY", "wire-test-key-0123456789");
    let mut cfg = ProviderCfg::new("wire", kind);
    cfg.base_url = Some(format!("http://127.0.0.1:{port}/v1"));
    cfg.key_ref = Some("env:IYAKKI_TEST_WIRE_KEY".into());
    let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    let client = ProviderClient::new(cfg, Secrets::new(None, up.clone()), up);
    Arc::new(ProviderChat { client, model: model.into(), streamer: Some(iyakki_proxy::https::download_streamer()) })
}

struct Now {
    ran: Mutex<Vec<String>>,
}

impl Tools for Now {
    fn defs(&self) -> Vec<ToolDef> {
        vec![ToolDef { spec: ToolSpec { name: "now".into(), description: "the time".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false }]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{name} {args}"));
        Ok("12:00 noon".into())
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

fn assist() -> Assist {
    Assist::standard(ReminderCtx { agent_on: true, browser_policy: "interactive".into(), engine_installed: true, screen_available: false, loaded: None })
}

fn drive(h: &RunHandle, mut each: impl FnMut(&RunEvent)) -> Vec<RunEvent> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(e) = h.events.recv_timeout(Duration::from_millis(20)) {
            if let RunEvent::Approval { id, .. } = &e {
                h.decide(id, Verdict::Approve);
            }
            each(&e);
            let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
            out.push(e);
            if done {
                break;
            }
        }
    }
    out
}

fn final_text(e: &[RunEvent]) -> String {
    e.iter().rev().find_map(|x| if let RunEvent::Final { text } = x { Some(text.clone()) } else { None }).unwrap_or_else(|| panic!("no final answer in {e:?}"))
}

fn start(b: Arc<ProviderChat>, t: Arc<Now>, ask: &str, auto: bool) -> RunHandle {
    start_with(b, t, vec![Message::system("SYS"), Message::user(ask)], RunLimits { auto, ..RunLimits::default() }, assist())
}

fn tool_call_events(s: &mut TcpStream, id: &str, name: &str) {
    event(s, None, &json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "function": {"name": name, "arguments": ""}}]}}]}).to_string());
    event(s, None, &json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}}]}).to_string());
    event(s, None, "[DONE]");
}

#[test]
fn an_answer_streams_over_the_wire_as_it_is_written_with_the_thinking_taken_out_and_the_request_keeps_the_model_loaded() {
    let finished_ms = Arc::new(AtomicU64::new(0));
    let t0 = Instant::now();
    let fin = finished_ms.clone();
    let (port, seen) = serve(move |_, _, s| {
        sse_head(s);
        event(s, None, &json!({"choices": [{"delta": {"role": "assistant", "reasoning": "hmm"}}]}).to_string());
        event(s, None, &delta("<thi"));
        event(s, None, &delta("nk>secret plan</think>It is "));
        std::thread::sleep(Duration::from_millis(600));
        event(s, None, &delta("noon "));
        std::thread::sleep(Duration::from_millis(300));
        event(s, None, &delta("exactly."));
        // stored before the closing events: the client could otherwise see the final answer and read the time before the server thread stored it
        fin.store(t0.elapsed().as_millis() as u64, Ordering::SeqCst);
        event(s, None, &json!({"choices": [], "usage": {"prompt_tokens": 40, "completion_tokens": 6}}).to_string());
        event(s, None, "[DONE]");
    });
    let tools = Arc::new(Now { ran: Mutex::new(vec![]) });
    let h = start(backend(Kind::Ollama, port, "qwen3:8b"), tools, "what time is it", false);
    let mut first_delta_ms = None;
    let ev = drive(&h, |e| {
        if matches!(e, RunEvent::Delta { text } if !text.is_empty()) && first_delta_ms.is_none() {
            first_delta_ms = Some(t0.elapsed().as_millis() as u64);
        }
    });
    assert_eq!(final_text(&ev), "It is noon exactly.", "{ev:?}");
    let first = first_delta_ms.expect("the answer was shown while it was written");
    let done = finished_ms.load(Ordering::SeqCst);
    assert!(done >= 800 && first + 500 < done, "the first words ({first} ms) came well before the server finished ({done} ms)");
    let shown: String = ev.iter().filter_map(|e| if let RunEvent::Delta { text } = e { Some(text.clone()) } else { None }).collect();
    assert!(!shown.contains("secret plan") && !shown.contains("think>"), "thinking is never shown as the answer: {shown}");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Thinking { .. })), "thinking is shown as activity");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].path, "/v1/chat/completions");
    let b: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!((b["stream"].clone(), b["stream_options"]["include_usage"].clone(), b["keep_alive"].clone()), (json!(true), json!(true), json!("30m")), "{b}");
    assert!(b["tools"].as_array().is_some_and(|t| t.iter().any(|x| x["function"]["name"] == "now")), "{b}");
}

#[test]
fn anthropic_events_over_the_wire_stream_text_and_assemble_a_tool_call_that_the_run_then_answers() {
    let (port, seen) = serve(|n, _, s| {
        sse_head(s);
        if n == 0 {
            event(s, Some("message_start"), r#"{"type":"message_start","message":{"usage":{"input_tokens":55,"output_tokens":1}}}"#);
            event(s, Some("content_block_start"), r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#);
            event(s, Some("content_block_delta"), r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me "}}"#);
            std::thread::sleep(Duration::from_millis(200));
            event(s, Some("content_block_delta"), r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"look."}}"#);
            event(s, Some("content_block_start"), r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_1","name":"now","input":{}}}"#);
            event(s, Some("content_block_delta"), r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{"}}"#);
            event(s, Some("content_block_delta"), r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"}"}}"#);
            event(s, Some("message_delta"), r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":31}}"#);
        } else {
            event(s, Some("content_block_start"), r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#);
            event(s, Some("content_block_delta"), r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"It is 12:00 noon."}}"#);
            event(s, Some("message_delta"), r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#);
        }
        event(s, Some("message_stop"), r#"{"type":"message_stop"}"#);
    });
    let tools = Arc::new(Now { ran: Mutex::new(vec![]) });
    let h = start(backend(Kind::Anthropic, port, "claude-x"), tools.clone(), "what time is it", false);
    let ev = drive(&h, |_| {});
    assert_eq!(final_text(&ev), "It is 12:00 noon.", "{ev:?}");
    assert_eq!(tools.ran.lock().unwrap().len(), 1, "the tool the stream assembled from its fragments ran once");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/v1/messages");
    let b: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(b["stream"], json!(true));
    assert!(seen[1].body.contains("12:00 noon") && seen[1].body.contains("tool_result"), "the tool result went back to the model in Anthropic's shape");
}

#[test]
fn a_server_that_only_thinks_is_abandoned_over_the_wire_and_asked_again_once_with_a_smaller_context() {
    let first_gone_ms = Arc::new(AtomicU64::new(0));
    let (g, t0) = (first_gone_ms.clone(), Instant::now());
    let (port, seen) = serve(move |n, _, s| {
        sse_head(s);
        if n == 0 {
            // thinking tokens for as long as the client listens
            for i in 0..2000 {
                if !event(s, None, &json!({"choices": [{"delta": {"reasoning": format!("thought {i} ")}}]}).to_string()) {
                    g.store(t0.elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        } else {
            event(s, None, &delta("Here is the answer."));
            event(s, None, "[DONE]");
        }
    });
    let mut a = assist();
    a.think_limit = Duration::from_millis(400);
    a.silence_limit = Duration::from_millis(800);
    let mut history = vec![Message::system("SYS")];
    for i in 0..10 {
        history.push(Message::user(format!("question {i}")));
        history.push(Message::assistant(format!("answer {i}")));
    }
    history.push(Message::user("the real question"));
    let tools = Arc::new(Now { ran: Mutex::new(vec![]) });
    let h = start_with(backend(Kind::Ollama, port, "qwen3:8b"), tools, history, RunLimits::default(), a);
    let ev = drive(&h, |_| {});
    assert_eq!(final_text(&ev), "Here is the answer.", "{ev:?}");
    let notes: Vec<String> = ev.iter().filter_map(|e| if let RunEvent::Note { text } = e { Some(text.clone()) } else { None }).collect();
    assert!(notes.iter().any(|x| x.contains("watchdog") && x.contains("thinking for")), "{notes:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one retry only");
    let (b0, b1): (Value, Value) = (serde_json::from_str(&seen[0].body).unwrap(), serde_json::from_str(&seen[1].body).unwrap());
    assert!(b1["messages"].as_array().unwrap().len() < b0["messages"].as_array().unwrap().len(), "a smaller context on the retry");
    assert!(seen[1].body.contains("thought for too long"), "the plain instruction comes with the retry");
    std::thread::sleep(Duration::from_millis(500));
    let gone = first_gone_ms.load(Ordering::SeqCst);
    assert!(gone > 0 && gone < 4000, "the abandoned connection was closed, the server saw it at {gone} ms");
}

#[test]
fn a_call_printed_over_many_lines_in_a_streamed_answer_is_run_and_never_shown_as_the_answer() {
    let (port, seen) = serve(|n, _, s| {
        sse_head(s);
        if n == 0 {
            for piece in ["Checking.\n```json\n{\n  \"name\": \"now\",\n", "  \"arguments\": {}\n}\n```"] {
                event(s, None, &delta(piece));
                std::thread::sleep(Duration::from_millis(50));
            }
        } else {
            event(s, None, &delta("It is 12:00 noon."));
        }
        event(s, None, "[DONE]");
    });
    let tools = Arc::new(Now { ran: Mutex::new(vec![]) });
    let h = start(backend(Kind::Ollama, port, "small-model"), tools.clone(), "what time is it", false);
    let ev = drive(&h, |_| {});
    assert_eq!(final_text(&ev), "It is 12:00 noon.", "{ev:?}");
    assert_eq!(tools.ran.lock().unwrap().len(), 1, "the printed call ran");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Final { text } if text.contains("\"name\""))), "never the final answer");
    assert!(seen.lock().unwrap()[1].body.contains("12:00 noon"), "its result went back to the model");
}

#[test]
fn in_auto_mode_the_wire_request_says_not_to_ask_and_a_permission_question_is_sent_back_once() {
    let ask = "The service is deployed. Would you like me to check if the service is responding or help with anything else?";
    let (port, seen) = serve(move |n, _, s| {
        sse_head(s);
        match n {
            0 | 2 => tool_call_events(s, &format!("c{n}"), "now"),
            1 => {
                event(s, None, &delta(ask));
                event(s, None, "[DONE]");
            }
            _ => {
                event(s, None, &delta("It answers on port 3000."));
                event(s, None, "[DONE]");
            }
        }
    });
    let tools = Arc::new(Now { ran: Mutex::new(vec![]) });
    let h = start(backend(Kind::Ollama, port, "qwen3:8b"), tools.clone(), "deploy the app", true);
    let ev = drive(&h, |_| {});
    assert_eq!(final_text(&ev), "It answers on port 3000.", "{ev:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    assert!(seen[0].body.contains("Approvals are automatic: do not ask whether to proceed"), "the rule reaches the wire");
    assert!(seen[2].body.contains("Do it now; do not ask"), "the question was sent back once");
    assert_eq!(tools.ran.lock().unwrap().len(), 2);
    // in ask mode the same question stands and nothing is said about automatic approvals
    drop(seen);
    let (port, seen) = serve(move |n, _, s| {
        sse_head(s);
        if n == 0 {
            tool_call_events(s, "c0", "now");
        } else {
            event(s, None, &delta(ask));
            event(s, None, "[DONE]");
        }
    });
    let h = start(backend(Kind::Ollama, port, "qwen3:8b"), Arc::new(Now { ran: Mutex::new(vec![]) }), "deploy the app", false);
    let ev = drive(&h, |_| {});
    assert_eq!(final_text(&ev), ask);
    assert!(!seen.lock().unwrap()[0].body.contains("Approvals are automatic"));
}

// ------------------------------------------------------------------------------------------------ Autopilot and workspace history over the wire

enum Turn {
    Say(String),
    Calls(Vec<(&'static str, &'static str, Value)>),
}

fn say(t: &str) -> Turn {
    Turn::Say(t.into())
}

/// A server that answers the n-th request with the n-th turn of the script (and says so when the script is used up).
fn serve_script(script: Vec<Turn>) -> (u16, Arc<Mutex<Vec<Req>>>) {
    serve(move |n, _, s| {
        sse_head(s);
        match script.get(n) {
            Some(Turn::Say(t)) => {
                event(s, None, &delta(t));
            }
            Some(Turn::Calls(cs)) => {
                for (i, (id, name, args)) in cs.iter().enumerate() {
                    event(s, None, &json!({"choices": [{"delta": {"tool_calls": [{"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}}]}}]}).to_string());
                }
            }
            None => {
                event(s, None, &delta("script used up"));
            }
        }
        event(s, None, "[DONE]");
    })
}

struct Gen {
    names: Vec<&'static str>,
    ran: Mutex<Vec<String>>,
    out: Box<dyn Fn(&str) -> String + Send + Sync>,
}

impl Tools for Gen {
    fn defs(&self) -> Vec<ToolDef> {
        self.names
            .iter()
            .map(|n| {
                let props = match *n {
                    "ask_user" => json!({"question": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}),
                    "write_file" | "read_file" => json!({"path": {"type": "string"}}),
                    _ => json!({}),
                };
                ToolDef { spec: ToolSpec { name: (*n).into(), description: format!("{n} does a thing"), schema: json!({"type": "object", "properties": props}) }, risk: Risk::Safe, untrusted: false }
            })
            .collect()
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{name} {args}"));
        Ok((self.out)(name))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

fn root(tag: &str) -> std::path::PathBuf {
    let r = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-wire-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(&r).unwrap();
    r
}

fn project(root: &std::path::Path, names: Vec<&'static str>, out: impl Fn(&str) -> String + Send + Sync + 'static) -> Arc<crate::boardtools::ProjectTools> {
    let inner = Arc::new(Gen { names, ran: Mutex::new(vec![]), out: Box::new(out) });
    Arc::new(crate::boardtools::ProjectTools::new(inner, root, "Web", &root.join("FEATURES.md")))
}

fn autopilot_over(b: Arc<ProviderChat>, t: Arc<crate::boardtools::ProjectTools>, c: crate::autopilot::Context) -> Vec<RunEvent> {
    let h = crate::autopilot::start(b, t, vec![Message::system("SYS"), Message::user("go")], RunLimits { max_steps: 20, max_tool_calls: 100, ..RunLimits::default() }, assist(), c);
    drive(&h, |_| {})
}

fn add_rows(titles: &[&'static str]) -> Turn {
    Turn::Calls(titles.iter().enumerate().map(|(i, t)| (["a0", "a1", "a2"][i], "board_add", json!({"prefix": "APP", "title": t}))).collect())
}

fn deploy_script() -> Vec<Turn> {
    vec![
        add_rows(&["Deploy the app and check that it responds"]),
        say("Planned."),
        Turn::Calls(vec![("a", "k8s_apply", json!({"manifest_path": "deploy/web.yaml"}))]),
        Turn::Calls(vec![("s", "deploy_status", json!({}))]),
        say("Applied and healthy."),
    ]
}

const DEPLOY_TOOLS: [&str; 7] = ["write_file", "deploy_status", "k8s_apply", "k8s_status", "k8s_teardown", "deploy_build", "ask_user"];

#[test]
fn the_autopilot_offers_the_teardown_once_per_chat_after_a_verified_deploy_and_asks_nothing_when_the_goal_is_done_over_the_wire() {
    let r = root("tear");
    let t = project(&r, DEPLOY_TOOLS.to_vec(), |n| match n {
        "deploy_status" => "Deployment web: 1/1 ready, Healthy".into(),
        "k8s_status" => "namespace: test-cla-dev (exists)".into(),
        o => format!("{o} ok"),
    });
    let ctx = |session: &str| {
        let mut c = crate::autopilot::Context::new(&r, &r.join("FEATURES.md"), "deploy the app");
        c.session = session.to_string();
        c
    };
    let user_of = |seen: &[Req]| -> Vec<String> { seen.iter().map(|q| q.body.clone()).collect() };
    // the first chat: the final check carries the offer and the question is shown once with its options
    let mut s = deploy_script();
    s.push(Turn::Calls(vec![("q", "ask_user", json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]}))]));
    let (port, seen) = serve_script(s);
    let ev = autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t.clone(), ctx("s_one"));
    let text = final_text(&ev);
    assert!(text.starts_with("Tear down the dev environment?\n\n1. Keep it running\n2. Stop it\n3. Remove all\n\nAutopilot finished"), "{text}; requests {:?}", seen.lock().unwrap().iter().map(|q| { let v: Value = serde_json::from_str(&q.body).unwrap_or_default(); let m = v["messages"].as_array().and_then(|a| a.last()).cloned().unwrap_or_default(); format!("{}: {}", m["role"], m["content"].as_str().unwrap_or("").chars().take(90).collect::<String>()) }).collect::<Vec<_>>());
    assert_eq!(text.matches("Tear down the dev environment?").count(), 1);
    assert!(user_of(&seen.lock().unwrap()).iter().any(|b| b.contains("AUTOPILOT final check") && b.contains("Tear down the dev environment?")), "the offer reached the wire");
    assert_eq!(crate::projectstate::noted(&r, "teardown_offered").as_deref(), Some("s_one"));
    // the same chat again: no second offer, a closing question is sent back, the summary is all that is shown
    let _ = std::fs::remove_file(r.join("FEATURES.md"));
    let mut s2 = deploy_script();
    s2.push(say("Redeployed; everything is healthy.\n\nWould you like me to tear it down?"));
    s2.push(say("Redeployed; everything is healthy."));
    let (port, seen) = serve_script(s2);
    let ev = autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t.clone(), ctx("s_one"));
    let text = final_text(&ev);
    assert!(text.contains("Redeployed; everything is healthy.") && !text.contains("Would you like"), "{text}");
    let bodies = user_of(&seen.lock().unwrap());
    assert!(bodies.iter().all(|b| !b.contains("Tear down the dev environment?")) && bodies.iter().any(|b| b.contains("a summary only. Do not ask anything")), "no offer the second time");
    assert!(bodies.iter().any(|b| b.contains("SYSTEM NOTE: the goal is complete. Write the summary only")), "the closing question was sent back once");
    // another chat is offered again
    let _ = std::fs::remove_file(r.join("FEATURES.md"));
    let mut s3 = deploy_script();
    s3.push(say("Healthy."));
    let (port, seen) = serve_script(s3);
    autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t, ctx("s_two"));
    assert!(user_of(&seen.lock().unwrap()).iter().any(|b| b.contains("AUTOPILOT final check") && b.contains("Tear down the dev environment?")));
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn after_remove_all_the_namespace_is_checked_gone_and_the_run_ends_with_one_summary_and_no_question_over_the_wire() {
    let r = root("removeall");
    let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let g = gone.clone();
    let t = project(&r, DEPLOY_TOOLS.to_vec(), move |n| match n {
        "k8s_teardown" => {
            g.store(true, Ordering::SeqCst);
            "Removed the namespace test-cla-dev and everything in it.".into()
        }
        "k8s_status" if g.load(Ordering::SeqCst) => "namespace: test-cla-dev does not exist yet (it is created on the first approved k8s_apply)".into(),
        "k8s_status" => "namespace: test-cla-dev (exists)".into(),
        o => format!("{o} ok"),
    });
    let c = crate::autopilot::Context::new(&r, &r.join("FEATURES.md"), "Remove all");
    let (port, seen) = serve_script(vec![
        Turn::Calls(vec![("t", "k8s_teardown", json!({}))]),
        say("All resources in test-cla-dev were removed.\n\nWould you like me to:\n1. Keep it running\n2. Stop it\n3. Remove all\n\nPlease choose an option."),
        Turn::Calls(vec![("q", "ask_user", json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]}))]),
        say("All resources in test-cla-dev were removed and the namespace is gone."),
    ]);
    let ev = autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t, c);
    let text = final_text(&ev);
    assert!(text.starts_with("All resources in test-cla-dev were removed and the namespace is gone.") && text.contains("Autopilot finished after 1 steps: 1 of 1 rows done (verified)"), "{text}");
    assert!(!text.contains("Would you like") && !text.contains("Please choose"), "{text}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Model { text, .. } if text.contains("Would you like"))), "the question is not shown in the chat either");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::ToolResult { name, ok: false, text, .. } if name == "ask_user" && text.contains("the goal is complete"))), "ask_user was refused");
    assert_eq!(seen.lock().unwrap().len(), 4, "nothing is asked after the namespace was seen gone");
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn what_a_step_learned_is_recalled_in_a_later_request_and_in_a_new_session_over_the_wire() {
    use crate::history::{History, Scope};
    let store = Arc::new(Mutex::new(crate::kb::KnowledgeBase::in_memory(crate::kb::default_embedder())));
    let r = root("hist");
    let t = project(&r, vec!["read_file", "write_file", "deploy_status"], |n| if n == "read_file" { "registry address is 192.168.1.68:30500, namespace test2-dev".into() } else { format!("{n} ok") });
    let hist = History::new(store.clone(), "alice", "Web", Some("s_one"));
    let mut c = crate::autopilot::Context::new(&r, &r.join("FEATURES.md"), "ship the registry change");
    c.history = Some(hist);
    let (port, seen) = serve_script(vec![
        add_rows(&["Check the registry address", "Add a Dockerfile"]),
        say("Planned."),
        Turn::Calls(vec![("r", "read_file", json!({"path": "notes.txt"}))]),
        say("Noted the registry address."),
        Turn::Calls(vec![("w", "write_file", json!({"path": "Dockerfile"}))]),
        say("Wrote it."),
        say("Checked."),
        say("never"),
    ]);
    let ev = autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t, c);
    assert!(!ev.is_empty());
    let bodies: Vec<String> = seen.lock().unwrap().iter().map(|q| q.body.clone()).collect();
    assert!(bodies.iter().any(|b| b.contains("Earlier in this workspace (recalled") && b.contains("192.168.1.68:30500")), "a later request on the wire carried the recalled fact");
    assert!(bodies.iter().any(|b| b.contains("Running summary: Goal: ship the registry change")), "the running summary reached the wire");
    assert!(bodies.iter().all(|b| b.matches("Running summary:").count() <= 1), "never appended twice");
    // the fact is in the workspace history: another session finds it with workspace scope only, and a new session starts from the summary
    let other = History::new(store.clone(), "alice", "Web", Some("s_two"));
    assert!(other.search("registry address", Scope::Session, None, 3).iter().all(|s| !s.text.contains("192.168.1.68")));
    assert!(other.search("registry address", Scope::Workspace, None, 3).iter().any(|s| s.text.contains("192.168.1.68:30500")));
    let r2 = root("hist2");
    let t2 = project(&r2, vec!["read_file"], |n| format!("{n} ok"));
    let mut c2 = crate::autopilot::Context::new(&r2, &r2.join("FEATURES.md"), "continue the shop work");
    c2.history = Some(other.clone());
    let (port, seen2) = serve_script(vec![say("ok"), say("ok")]);
    autopilot_over(backend(Kind::Ollama, port, "qwen3:8b"), t2, c2);
    assert!(seen2.lock().unwrap()[0].body.contains("a summary of a previous session") && seen2.lock().unwrap()[0].body.contains("Goal: ship the registry change"), "the new session starts from the earlier summary");
    other.record("tool", "", "deployed with password: hunter2hunter2X9");
    assert!(!other.search("deployed password", Scope::Workspace, None, 5).iter().any(|s| s.text.contains("hunter2")), "secrets are never indexed");
    let _ = std::fs::remove_dir_all(&r);
    let _ = std::fs::remove_dir_all(&r2);
}
