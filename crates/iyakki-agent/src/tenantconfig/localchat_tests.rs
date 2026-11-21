//! Tests of the chat backend of a team's local model host (`localchat.rs`). A fake server on a loopback socket speaks real HTTP to the real
//! transport (`ureq`); the entry keeps its allowed internal address, and the test transport only points that address at the fake.

use super::*;
use crate::chat::{ChatBackend, Message};
use crate::tenantconfig::localchat::{self, TeamLocalChat, Transport};
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const TOKEN: &str = "local-token-xyz";
const ENTRY_BASE: &str = "http://10.0.5.7:11434";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    delay_ms: u64,
    extra: Vec<(&'static str, String)>,
}

fn ok(text: &str) -> Reply {
    Reply { status: 200, body: json!({"choices": [{"message": {"role": "assistant", "content": text}}]}).to_string().into_bytes(), delay_ms: 0, extra: vec![] }
}

fn status(code: u16, body: &str) -> Reply {
    Reply { status: code, body: body.as_bytes().to_vec(), delay_ms: 0, extra: vec![] }
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn fake(responder: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let (s2, r) = (seen.clone(), Arc::new(responder));
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(c) = c else { continue };
            // a socket from a listener may inherit its mode on some systems: ours blocks
            let _ = c.set_nonblocking(false);
            let (s3, r2) = (s2.clone(), r.clone());
            std::thread::spawn(move || serve(c, &s3, &*r2));
        }
    });
    Fake { port, seen }
}

fn serve(mut c: TcpStream, seen: &Mutex<Vec<Seen>>, responder: &dyn Fn(&Seen) -> Reply) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let first = lines.next().unwrap_or("");
    let (method, path) = (first.split(' ').next().unwrap_or("").to_string(), first.split(' ').nth(1).unwrap_or("").to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    let s = Seen { method, path, headers, body: String::from_utf8_lossy(&body).to_string() };
    seen.lock().unwrap().push(s.clone());
    let r = responder(&s);
    std::thread::sleep(Duration::from_millis(r.delay_ms));
    let mut out = format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
    for (k, v) in &r.extra {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = c.write_all(out.as_bytes()).and_then(|_| c.write_all(&r.body));
}

/// The real transport, with the entry's address pointed at the fake.
struct Rewrite {
    inner: Arc<dyn Upstream>,
    to: String,
}

impl Upstream for Rewrite {
    fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.inner.request(m, &url.replacen(ENTRY_BASE, &self.to, 1), h, b)
    }
}

fn to(port: u16) -> Transport {
    let real = localchat::real_transport();
    Arc::new(move |t| Arc::new(Rewrite { inner: real(t), to: format!("http://127.0.0.1:{port}") }))
}

fn secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn host(tag: &str, extra: Value, with_token: bool) -> T {
    let x = t(tag);
    x.put("adam", "integration.policy", &[], json!({"enabled": true, "values": {"internal_hosts": ["10.0.5.0/24"]}})).unwrap();
    let mut b = local_body(extra);
    if with_token {
        b["secrets"] = json!({"token": TOKEN});
    }
    x.put("mia", "models.local", &["web", "gpu"], b).unwrap();
    x
}

fn ask(x: &T, user: &str, model: &str, tr: Transport, history: &[Message]) -> Result<String, String> {
    let env = x.env();
    let clock = secs;
    let c = TeamLocalChat { env: &env, team: "web", name: "gpu", user, user_rank: 1, model, clock: &clock, transport: tr, streamer: None };
    c.complete(history)
}

fn q(text: &str) -> Vec<Message> {
    vec![Message::system("be brief"), Message::user(text)]
}

// ------------------------------------------------------------------------------------------------------------ the request

#[test]
fn tcfg_local_chat_sends_one_non_streamed_openai_request_with_the_token_and_returns_the_text() {
    let x = host("lc-shape", json!({"api_style": "openai_compatible"}), true);
    let f = fake(|_| ok("hello from the gpu"));
    assert_eq!(ask(&x, "max", "llama3", to(f.port), &q("what is 2+2?")), Ok("hello from the gpu".to_string()));
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let s = &seen[0];
    assert_eq!((s.method.as_str(), s.path.as_str()), ("POST", "/v1/chat/completions"));
    assert_eq!(s.header("authorization"), Some(format!("Bearer {TOKEN}").as_str()));
    assert!(s.header("content-type").unwrap().starts_with("application/json"));
    let b = s.json();
    assert_eq!(b["model"], "llama3");
    assert_eq!(b["stream"], false, "streaming is off");
    assert!(b.get("tools").is_none(), "no tools are offered");
    assert_eq!(b["messages"], json!([{"role": "system", "content": "be brief"}, {"role": "user", "content": "what is 2+2?"}]));
    // the token came from the vault for a named action and nowhere else
    assert!(x.vault.uses.lock().unwrap().iter().all(|(a, _)| a == "local-model-call") && x.vault.uses.lock().unwrap().len() == 1);
    let audit = x.rec.all();
    assert!(audit.contains("tenant.local.answer") && audit.contains("ok=true"), "{audit}");
    assert!(!audit.contains(TOKEN) && !audit.contains("what is 2+2") && !audit.contains("hello from the gpu") && !audit.contains("10.0.5.7"), "no token, question, answer or address in the audit: {audit}");
}

#[test]
fn tcfg_local_chat_in_ollama_style_uses_the_openai_endpoint_of_ollama_and_a_host_without_a_token_gets_no_header() {
    let x = host("lc-ollama", json!({}), false);
    let f = fake(|_| ok("ollama says hi"));
    assert_eq!(ask(&x, "max", "llama3", to(f.port), &q("hi")), Ok("ollama says hi".to_string()));
    let s = f.seen.lock().unwrap()[0].clone();
    assert_eq!(s.path, "/v1/chat/completions");
    assert_eq!(s.json()["stream"], false);
    assert!(s.header("authorization").is_none(), "an entry without a token sends no Authorization header");
    assert!(x.vault.uses.lock().unwrap().is_empty());
}

#[test]
fn tcfg_local_chat_is_gated_before_anything_is_sent_and_a_plain_question_offers_no_tools() {
    let x = host("lc-gate", json!({}), true);
    let f = fake(|_| ok("never"));
    let r = ask(&x, "max", "other-model", to(f.port), &q("hi"));
    assert!(r.unwrap_err().contains("not allowed"), "model not on the list");
    assert!(f.seen.lock().unwrap().iter().all(|s| s.json()["model"] == "llama3"), "only an allowed model ever reached the host");
    let before = f.seen.lock().unwrap().len();
    let env = x.env();
    let clock = secs;
    let c = TeamLocalChat { env: &env, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(f.port), streamer: None };
    let tools = [crate::chat::ToolSpec { name: "t".into(), description: "d".into(), schema: json!({"type": "object"}) }];
    assert!(c.complete_tools(&q("hi"), &tools).is_ok_and(|t| t.text == "never"), "tool turns go through the same gate");
    assert_eq!(f.seen.lock().unwrap().len(), before + 1);
    assert!(f.seen.lock().unwrap().last().unwrap().json().get("tools").is_some());
}

#[test]
fn tcfg_local_chat_holds_the_fair_share_slot_while_the_host_answers() {
    let x = Arc::new(host("lc-slot", json!({"max_concurrent_team": 1, "max_concurrent_per_person": 1, "queue_timeout_secs": 1}), true));
    // the host answers the first call only when the test lets it (a gate, not a delay: a loaded machine cannot make the answer come "too early")
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let g2 = gate.clone();
    let f = fake(move |_| {
        iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || g2.load(std::sync::atomic::Ordering::SeqCst));
        ok("slow")
    });
    let x2 = x.clone();
    let port = f.port;
    let first = std::thread::spawn(move || ask(&x2, "max", "llama3", to(port), &q("one")));
    iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || !f.seen.lock().unwrap().is_empty());
    let second = ask(&x, "vic", "llama3", to(f.port), &q("two"));
    gate.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(second, Err("the queue timed out".to_string()), "the first call still holds the only slot");
    assert_eq!(first.join().unwrap(), Ok("slow".to_string()));
    assert_eq!(f.seen.lock().unwrap().len(), 1, "the second call never reached the host");
}

// ------------------------------------------------------------------------------------------------------------ errors

#[test]
fn tcfg_local_chat_maps_401_404_429_and_500_to_plain_sentences_without_the_token_or_the_address() {
    let x = host("lc-errors", json!({"api_style": "openai_compatible"}), true);
    for (code, body, want) in [
        (401, format!("{{\"error\":{{\"message\":\"bad key {TOKEN}\"}}}}"), "refused the team's token"),
        (403, "forbidden".to_string(), "refused the team's token"),
        (404, "{\"error\":\"model 'llama3' not found\"}".to_string(), "does not know this model or API path"),
        (429, "slow down".to_string(), "busy"),
        (500, format!("{{\"error\":\"boom {TOKEN} at 10.0.5.7\"}}"), "the host failed (HTTP 500)"),
        (400, "model at 10.0.5.7:11434 is loading".to_string(), "refused the request (HTTP 400): model at [host] is loading"),
        (503, "overloaded".to_string(), "the host failed (HTTP 503)"),
        (400, "bad request".to_string(), "refused the request (HTTP 400)"),
    ] {
        let f = fake(move |_| status(code, &body));
        let e = ask(&x, "max", "llama3", to(f.port), &q("hi")).unwrap_err();
        assert!(e.contains(want), "{code}: {e}");
        assert!(!e.contains(TOKEN) && !e.contains("10.0.5.7") && !e.contains("127.0.0.1"), "{code}: {e}");
    }
    let audit = x.rec.all();
    assert!(audit.contains("tenant.local.failed") && !audit.contains(TOKEN) && !audit.contains("10.0.5.7"), "{audit}");
}

#[test]
fn tcfg_local_chat_says_so_when_the_host_is_not_there_is_too_slow_or_answers_something_else() {
    let x = host("lc-down", json!({"max_request_secs": 1}), true);
    // nothing listens
    let closed = crate::testdir::free_port();
    let e = ask(&x, "max", "llama3", to(closed), &q("hi")).unwrap_err();
    assert_eq!(e, "the local model host could not be reached");
    // the whole call may not take longer than max_request_secs
    let slow = fake(|_| Reply { delay_ms: 3000, ..ok("late") });
    // (the error itself shows the call was cut at the limit: had it waited for the answer, it would have returned "late")
    let e = ask(&x, "max", "llama3", to(slow.port), &q("hi")).unwrap_err();
    assert!(e.contains("did not answer within 1 seconds"), "{e}");
    // an answer of another shape (a streamed or native reply to a non-streamed call)
    let odd = fake(|_| status(200, "data: {\"x\":1}\n\n"));
    assert!(ask(&x, "max", "llama3", to(odd.port), &q("hi")).unwrap_err().contains("not in the shape"));
    let empty = fake(|_| status(200, "{\"choices\":[{\"message\":{\"content\":\"\"}}]}"));
    assert!(ask(&x, "max", "llama3", to(empty.port), &q("hi")).is_err());
}

// ------------------------------------------------------------------------------------------------------------ size caps

#[test]
fn tcfg_local_chat_refuses_a_request_over_max_request_kb_before_sending_and_an_answer_over_the_cap() {
    let x = host("lc-size", json!({"max_request_kb": 1}), true);
    let f = fake(|_| ok("fine"));
    let e = ask(&x, "max", "llama3", to(f.port), &q(&"x".repeat(5000))).unwrap_err();
    assert!(e.contains("larger than this host takes (1 KB)"), "{e}");
    assert!(f.seen.lock().unwrap().is_empty(), "nothing was sent");
    assert_eq!(ask(&x, "max", "llama3", to(f.port), &q("short")), Ok("fine".to_string()), "a request under the cap goes through");
    // an answer larger than the cap is not read into a reply
    let big = fake(|_| status(200, &format!("{{\"choices\":[{{\"message\":{{\"content\":\"{}\"}}}}]}}", "a".repeat(localchat::MAX_RESPONSE_BYTES + 10))));
    let e = ask(&x, "max", "llama3", to(big.port), &q("hi")).unwrap_err();
    assert!(e.contains("larger than 4 MB"), "{}", &e[..e.len().min(200)]);
}

// ------------------------------------------------------------------------------------------------------------ the transport itself

fn target<'a>(base: &'a str, style: &'a str, token: Option<&'a str>) -> runtime::LocalTarget<'a> {
    runtime::LocalTarget { base, style, token, max_request_bytes: 64 * 1024, max_secs: 5 }
}

#[test]
fn tcfg_local_chat_over_the_real_transport_does_not_follow_a_redirect_with_the_token() {
    let other = fake(|_| ok("elsewhere"));
    let loc = format!("http://127.0.0.1:{}/v1/chat/completions", other.port);
    let f = fake(move |_| Reply { status: 302, body: vec![], delay_ms: 0, extra: vec![("Location", loc.clone())] });
    let base = format!("http://127.0.0.1:{}", f.port);
    let e = localchat::answer(&target(&base, "openai_compatible", Some(TOKEN)), "llama3", &q("hi"), &localchat::real_transport()).unwrap_err();
    assert!(e.contains("HTTP 302"), "{e}");
    assert!(other.seen.lock().unwrap().is_empty(), "the redirect target never saw the request or the token");
    // a good call straight against the fake
    let g = fake(|_| ok("direct"));
    let base = format!("http://127.0.0.1:{}", g.port);
    assert_eq!(localchat::answer(&target(&base, "openai_compatible", None), "llama3", &q("hi"), &localchat::real_transport()), Ok("direct".to_string()));
    assert!(g.seen.lock().unwrap().iter().all(|s| s.path == "/v1/chat/completions"), "{:?}", g.seen.lock().unwrap());
}

#[test]
fn tcfg_local_chat_refuses_an_unusable_address_style_model_or_empty_question() {
    let tr = localchat::real_transport();
    let base = "http://127.0.0.1:1";
    for (b, style, model, hist) in [
        ("http://user:pw@127.0.0.1:1", "ollama", "m", q("hi")),
        ("http://127.0.0.1:1/some/path", "ollama", "m", q("hi")),
        ("ftp://127.0.0.1:1", "ollama", "m", q("hi")),
        (base, "grpc", "m", q("hi")),
        (base, "ollama", "bad model!", q("hi")),
        (base, "ollama", "m", vec![]),
    ] {
        assert!(localchat::answer(&target(b, style, None), model, &hist, &tr).is_err(), "{b} {style} {model}");
    }
}

#[test]
fn tcfg_local_chat_never_repeats_a_token_in_any_error_text() {
    for e in ["HTTP 500: oops local-token-xyz", "connection refused local-token-xyz", "HTTP 401: local-token-xyz", "something timed out for local-token-xyz", "TOO_LARGE_REQUEST 7"] {
        let s = localchat::explain(e, Some(TOKEN), 30, &[]);
        assert!(!s.contains(TOKEN), "{e} -> {s}");
    }
    assert_eq!(localchat::explain("dns error: failed to lookup address information", None, 30, &[]), "the local model host could not be reached");
}

// ------------------------------------------------------------------------------------------------------------ tool calls and streaming

struct RewriteStream {
    inner: Arc<dyn iyakki_proxy::Streamer>,
    to: String,
}

impl iyakki_proxy::Streamer for RewriteStream {
    fn open(&self, url: &str, h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        self.inner.open(&url.replacen(ENTRY_BASE, &self.to, 1), h)
    }
    fn post(&self, url: &str, h: &[(String, String)], b: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
        self.inner.post(&url.replacen(ENTRY_BASE, &self.to, 1), h, b)
    }
}

fn to_stream(port: u16) -> localchat::StreamTransport {
    let real = localchat::real_streamer();
    Arc::new(move |t| Arc::new(RewriteStream { inner: real(t), to: format!("http://127.0.0.1:{port}") }))
}

fn tool() -> crate::chat::ToolSpec {
    crate::chat::ToolSpec { name: "weather".into(), description: "look up the weather".into(), schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}) }
}

fn sse(parts: &[Value]) -> Reply {
    let mut s = String::new();
    for p in parts {
        s.push_str(&format!("data: {p}\n\n"));
    }
    s.push_str("data: [DONE]\n\n");
    Reply { status: 200, body: s.into_bytes(), delay_ms: 0, extra: vec![] }
}

#[test]
fn tcfg_local_chat_returns_the_tool_calls_of_the_host_through_the_same_gate_and_token() {
    let x = host("lc-tools", json!({"api_style": "openai_compatible"}), true);
    let f = fake(|_| Reply { status: 200, body: json!({"choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Oslo\"}"}}]}}]}).to_string().into_bytes(), delay_ms: 0, extra: vec![] });
    let env = x.env();
    let clock = secs;
    let c = TeamLocalChat { env: &env, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(f.port), streamer: None };
    let turn = c.complete_tools(&q("weather in Oslo?"), &[tool()]).unwrap();
    assert_eq!(turn.calls.len(), 1);
    assert_eq!((turn.calls[0].name.as_str(), &turn.calls[0].args), ("weather", &json!({"city": "Oslo"})));
    let s = f.seen.lock().unwrap()[0].clone();
    assert_eq!(s.header("authorization"), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(s.json()["tools"][0]["function"]["name"], "weather");
    let audit = x.rec.all();
    assert!(audit.contains("tool_calls=1") && !audit.contains(TOKEN) && !audit.contains("Oslo"), "{audit}");
    // a model that is not on the list never reaches the host
    let c2 = TeamLocalChat { model: "other-model", ..c };
    assert!(c2.complete_tools(&q("hi"), &[tool()]).unwrap_err().contains("not allowed"));
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}

#[test]
fn tcfg_local_chat_streams_pieces_with_the_token_and_stops_when_asked_and_keeps_the_caps() {
    let x = host("lc-stream", json!({"api_style": "openai_compatible"}), true);
    let f = fake(|_| sse(&[json!({"choices": [{"delta": {"content": "Hel"}}]}), json!({"choices": [{"delta": {"content": "lo"}}]}), json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})]));
    let env = x.env();
    let clock = secs;
    let c = TeamLocalChat { env: &env, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(f.port), streamer: Some(to_stream(f.port)) };
    let mut pieces = Vec::new();
    let turn = c
        .complete_tools_stream(&q("hi"), &[], &mut |p| {
            pieces.push(p.to_string());
            true
        })
        .unwrap();
    assert_eq!(turn.text, "Hello");
    assert!(pieces.concat() == "Hello" && pieces.len() >= 2, "{pieces:?}");
    let s = f.seen.lock().unwrap()[0].clone();
    assert_eq!(s.json()["stream"], true);
    assert_eq!(s.header("authorization"), Some(format!("Bearer {TOKEN}").as_str()));
    // the request cap holds for a stream too, and nothing is sent
    let small = host("lc-stream-small", json!({"api_style": "openai_compatible", "max_request_kb": 1}), true);
    let env2 = small.env();
    let g = fake(|_| sse(&[]));
    let c3 = TeamLocalChat { env: &env2, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(g.port), streamer: Some(to_stream(g.port)) };
    let e = c3.complete_tools_stream(&q(&"x".repeat(5000)), &[], &mut |_| true).unwrap_err();
    assert!(e.contains("larger than this host takes (1 KB)"), "{e}");
    assert!(g.seen.lock().unwrap().is_empty());
    // an HTTP error of the host is a plain sentence without the token
    let h = fake(|_| status(401, "no"));
    let c4 = TeamLocalChat { env: &env, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(h.port), streamer: Some(to_stream(h.port)) };
    let e = c4.complete_tools_stream(&q("hi"), &[], &mut |_| true).unwrap_err();
    assert!(e.contains("refused the team's token") && !e.contains(TOKEN), "{e}");
}

#[test]
fn tcfg_local_chat_without_a_streamer_answers_in_one_piece() {
    let x = host("lc-nostream", json!({"api_style": "openai_compatible"}), true);
    let f = fake(|_| ok("whole"));
    let env = x.env();
    let clock = secs;
    let c = TeamLocalChat { env: &env, team: "web", name: "gpu", user: "max", user_rank: 1, model: "llama3", clock: &clock, transport: to(f.port), streamer: None };
    let mut n = 0;
    let t = c.complete_tools_stream(&q("hi"), &[tool()], &mut |_| { n += 1; true }).unwrap();
    assert_eq!(t.text, "whole");
    assert_eq!(f.seen.lock().unwrap()[0].json()["stream"], false);
    assert_eq!(n, 0);
}

#[test]
fn tcfg_a_team_host_is_published_as_a_cluster_service_by_address_or_name_and_bad_names_are_refused() {
    use crate::tenantconfig::hostservice::{host_service, service_yaml, HostService};
    let x = host("lc-svc", json!({}), false);
    let y = host_service(&x.env(), "web", "gpu").unwrap();
    assert!(y.contains("name: llm-gpu\n  namespace: web") && y.contains("kind: Endpoints") && y.contains("- ip: \"10.0.5.7\"") && y.contains("port: 11434"), "{y}");
    assert!(!y.contains("selector"), "an outside host has no selector");
    let h = |host: &str| HostService { namespace: "web".into(), name: "gpu".into(), host: host.into(), port: 8080 };
    let n = service_yaml(&h("GPU.lan.example.com")).unwrap();
    assert!(n.contains("type: ExternalName") && n.contains("externalName: gpu.lan.example.com") && !n.contains("Endpoints"), "{n}");
    assert!(service_yaml(&h("2001:db8::7")).unwrap().contains("- ip: \"2001:db8::7\""));
    for bad in ["bad host", "a\nkind: Secret", "-x.example.com", ""] {
        assert!(service_yaml(&h(bad)).is_err(), "{bad:?}");
    }
    assert!(service_yaml(&HostService { name: "Gpu\nx".into(), ..h("10.0.0.1") }).is_err());
    assert!(service_yaml(&HostService { port: 0, ..h("10.0.0.1") }).is_err());
    assert!(host_service(&x.env(), "web", "nope").is_err());
}

// ------------------------------------------------------------------------------------------------------------ team providers and the token count

const PROVIDER_BASE: &str = "https://llm.corp.example";
const PROVIDER_KEY: &str = "key-for-corp-llm-123456";

struct RewriteProvider {
    inner: Arc<dyn Upstream>,
    to: String,
}

impl Upstream for RewriteProvider {
    fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.inner.request(m, &url.replacen(PROVIDER_BASE, &self.to, 1), h, b)
    }
}

fn to_provider(port: u16) -> Transport {
    let real = localchat::real_transport();
    Arc::new(move |t| Arc::new(RewriteProvider { inner: real(t), to: format!("http://127.0.0.1:{port}") }))
}

fn provider_host(tag: &str, extra: Value) -> T {
    let x = t(tag);
    let mut v = json!({"api": "openai", "base_url": PROVIDER_BASE, "allowed_models": ["m1"], "terms_ack": true});
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    x.put("mia", "models.provider", &["web", "corp"], json!({"enabled": true, "values": v, "secrets": {"api_key": PROVIDER_KEY}})).unwrap();
    x
}

fn used(text: &str, prompt: u64, completion: u64) -> Reply {
    Reply { status: 200, body: json!({"choices": [{"message": {"role": "assistant", "content": text}}], "usage": {"prompt_tokens": prompt, "completion_tokens": completion}}).to_string().into_bytes(), delay_ms: 0, extra: vec![] }
}

#[test]
fn tcfg_a_team_provider_answers_with_its_key_counts_tokens_and_is_refused_over_the_daily_token_limit() {
    let x = provider_host("tp-tokens", json!({"daily_tokens_per_person": 20}));
    let f = fake(|_| used("pong", 8, 5));
    let env = x.env();
    let clock = secs;
    let c = |user: &'static str| localchat::TeamProviderChat { env: &env, team: "web", name: "corp", user, user_rank: 1, model: "m1", clock: &clock, transport: to_provider(f.port) };
    assert_eq!(c("max").complete(&q("ping")), Ok("pong".to_string()));
    let s = f.seen.lock().unwrap()[0].clone();
    assert_eq!(s.path, "/chat/completions");
    assert_eq!(s.header("authorization"), Some(format!("Bearer {PROVIDER_KEY}").as_str()));
    let now = clock();
    assert_eq!(crate::tenantconfig::store::Store::new(env.dir).tokens_today("models.provider", "web:corp", "max", now), 13, "the provider's own count");
    assert_eq!(c("max").complete(&q("ping")), Ok("pong".to_string()), "13 of 20 used: still allowed");
    let e = c("max").complete(&q("ping")).unwrap_err();
    assert!(e.contains("daily token limit"), "{e}");
    assert_eq!(f.seen.lock().unwrap().len(), 2, "the refused request never reached the provider");
    assert_eq!(c("vic").complete(&q("ping")), Ok("pong".to_string()), "counts are per person");
    let audit = x.rec.all();
    assert!(audit.contains("tenant.provider.tokens") && audit.contains("added=13") && !audit.contains(PROVIDER_KEY) && !audit.contains("ping"), "{audit}");
}

#[test]
fn tcfg_a_team_provider_returns_tool_calls_gates_the_model_and_hides_the_key_in_errors() {
    let x = provider_host("tp-tools", json!({}));
    let f = fake(|s| if s.json()["messages"].to_string().contains("boom") { status(500, "oops key-for-corp-llm-123456") } else { Reply { status: 200, body: json!({"choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "weather", "arguments": "{}"}}]}}]}).to_string().into_bytes(), delay_ms: 0, extra: vec![] } });
    let env = x.env();
    let clock = secs;
    let c = localchat::TeamProviderChat { env: &env, team: "web", name: "corp", user: "max", user_rank: 1, model: "m1", clock: &clock, transport: to_provider(f.port) };
    assert_eq!(c.complete_tools(&q("weather?"), &[tool()]).unwrap().calls[0].name, "weather");
    let e = c.complete(&q("boom")).unwrap_err();
    assert!(e.contains("HTTP 500") && !e.contains(PROVIDER_KEY) && !e.contains("llm.corp.example") && !e.contains("local model host"), "{e}");
    let other = localchat::TeamProviderChat { model: "m2", ..c };
    assert!(other.complete(&q("hi")).unwrap_err().contains("not allowed"));
    assert_eq!(f.seen.lock().unwrap().len(), 2);
    let a = provider_host("tp-anthropic", json!({"api": "anthropic"}));
    let e2 = a.env();
    let c2 = localchat::TeamProviderChat { env: &e2, team: "web", name: "corp", user: "max", user_rank: 1, model: "m1", clock: &clock, transport: to_provider(f.port) };
    assert!(c2.complete(&q("hi")).unwrap_err().contains("Anthropic"));
}

#[test]
fn tcfg_an_owned_team_backend_serves_a_run_through_the_same_gate_and_audit() {
    use crate::tenantconfig::localchat::{OwnedTeamChat, TeamKind};
    let x = host("lc-owned", json!({}), true);
    let f = fake(|_| ok("from the owned backend"));
    let make = |user: &str| OwnedTeamChat {
        kind: TeamKind::Local,
        dir: x.dir.clone(),
        vault: x.vault.clone(),
        audit: x.rec.clone(),
        team: "web".into(),
        name: "gpu".into(),
        user: user.into(),
        user_rank: 1,
        model: "llama3".into(),
        transport: to(f.port),
        streamer: None,
        clock: Arc::new(secs),
    };
    let b = make("max");
    assert_eq!(b.provider_name(), "gpu");
    assert_eq!(b.complete(&q("hi")), Ok("from the owned backend".to_string()));
    let audit = x.rec.all();
    assert!(audit.contains("tenant.local.answer") && audit.contains("ok=true"), "{audit}");
    // a model that is not on the host's list is refused by the gate on the call, with nothing sent
    let n = f.seen.lock().unwrap().len();
    let mut other = make("max");
    other.model = "not-listed".into();
    assert!(other.complete(&q("hi")).is_err());
    assert_eq!(f.seen.lock().unwrap().len(), n, "a refused call sends nothing");
}
