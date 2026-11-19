//! The provider client over a real socket (F-AI-08, F-AI-12, F-AI-13, F-AI-14): `ProviderClient` with the real HTTP client
//! (`UreqUpstream`) talks to a small OpenAI-style server of this file on the loopback address. Nothing is faked between the chat call and
//! the wire, so what the tests check is what a server really receives (the method, path, key header and JSON body) and how a real reply
//! with token counts, reasoning text, a long model list or an error is read. No real provider, key or model is involved.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::https::UreqUpstream;
use iyakki_proxy::Upstream;
use serde_json::{json, Value};

use crate::chat::Message;
use crate::modelopts::{set, ModelOptions};
use crate::providers::{reported_reasoning_tokens, reported_usage, thinking_only, Kind, ProviderCfg, ProviderClient, THINKING_ONLY_MESSAGE};
use crate::vault::Secrets;

const KEY: &str = "nvapi-LOOPBACKmadeup0123456789";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

/// A server that answers every request with `reply(&request)` (status, body) and remembers what it was sent.
fn serve(reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let (log, reply) = (seen.clone(), Arc::new(reply));
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (log, reply) = (log.clone(), reply.clone());
            std::thread::spawn(move || handle(s, &log, reply.as_ref()));
        }
    });
    (port, seen)
}

fn handle(mut s: TcpStream, log: &Mutex<Vec<Seen>>, reply: &dyn Fn(&Seen) -> (u16, String)) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    while buf.len() < head_end + len {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = String::from_utf8_lossy(&buf[head_end..(head_end + len).min(buf.len())]).into_owned();
    let seen = Seen { method, path, headers, body };
    log.lock().unwrap().push(seen.clone());
    let (status, out) = reply(&seen);
    let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).as_bytes());
    let _ = s.flush();
}

fn client(kind: Kind, port: u16, dir: Option<&std::path::Path>) -> ProviderClient {
    std::env::set_var("IYAKKI_TEST_LOOPBACK_KEY", KEY);
    let mut cfg = ProviderCfg::new("loop", kind);
    cfg.base_url = Some(format!("http://127.0.0.1:{port}/v1"));
    cfg.key_ref = Some("env:IYAKKI_TEST_LOOPBACK_KEY".into());
    let up: Arc<dyn Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(10)));
    let c = ProviderClient::new(cfg, Secrets::new(None, up.clone()), up);
    if let Some(d) = dir { c.with_options_dir(d) } else { c }
}

fn ask() -> Vec<Message> {
    vec![Message::user("Reply with the single word OK.")]
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-provloop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_real_request_carries_the_key_the_path_and_the_options_and_the_servers_token_counts_are_read() {
    let d = tmp("opts");
    set(&d, "loop", "m1", &ModelOptions { max_tokens: Some(4096), temperature: Some(0.6), top_p: Some(0.95), reasoning_budget: Some(2048) }).unwrap();
    let (port, seen) = serve(|_| (200, r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}],"usage":{"prompt_tokens":12,"completion_tokens":300,"completion_tokens_details":{"reasoning_tokens":290}}}"#.into()));
    let c = client(Kind::Nvidia, port, Some(&d));
    let t = c.chat_turn("m1", &ask(), &[]).unwrap();
    assert_eq!(t.text, "OK");
    assert_eq!(reported_usage(), Some((12, 300)), "the counts come from the server's own usage block");
    assert_eq!(reported_reasoning_tokens(), Some(290));
    let s = seen.lock().unwrap();
    assert_eq!((s[0].method.as_str(), s[0].path.as_str()), ("POST", "/v1/chat/completions"));
    assert_eq!(s[0].header("authorization"), Some(format!("Bearer {KEY}").as_str()), "the key travels in the header and nowhere else");
    assert!(!s[0].path.contains(KEY) && !s[0].body.contains(KEY));
    let sent: Value = serde_json::from_str(&s[0].body).unwrap();
    assert_eq!(sent["model"], "m1");
    assert_eq!((sent["max_tokens"].clone(), sent["temperature"].clone(), sent["top_p"].clone(), sent["reasoning_budget"].clone()), (json!(4096), json!(0.6), json!(0.95), json!(2048)));
    drop(s);
    // another provider with the same numbers saved: options are per provider and model, and a model with none sends none
    let c2 = client(Kind::Nvidia, port, Some(&d));
    c2.chat_turn("other-model", &ask(), &[]).unwrap();
    let last: Value = serde_json::from_str(&seen.lock().unwrap().last().unwrap().body).unwrap();
    assert!(last.get("max_tokens").is_none() && last.get("temperature").is_none() && last.get("reasoning_budget").is_none(), "{last}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_server_that_does_not_count_leaves_the_counts_unknown_and_an_error_reads_as_words() {
    let (port, _) = serve(|r| if r.body.contains("boom") { (503, r#"{"error":{"message":"overloaded"}}"#.into()) } else { (200, r#"{"choices":[{"message":{"content":"hi"}}]}"#.into()) });
    let c = client(Kind::Compat, port, None);
    assert_eq!(c.chat("m", &ask()).unwrap(), "hi");
    assert_eq!(reported_usage(), None, "nothing is claimed that the server did not say");
    let e = c.chat("m", &[Message::user("boom")]).unwrap_err();
    assert!(e.contains("503") || e.to_lowercase().contains("overloaded"), "{e}");
    assert!(!e.contains(KEY), "the key is never in an error");
}

#[test]
fn reasoning_text_from_a_real_reply_is_never_the_answer() {
    for field in ["reasoning_content", "reasoning"] {
        // with an answer: the answer wins, the thinking is dropped
        let both = format!(r#"{{"choices":[{{"message":{{"content":"OK","{field}":"chain of thought {field}"}}}}],"usage":{{"prompt_tokens":5,"completion_tokens":50,"completion_tokens_details":{{"reasoning_tokens":40}}}}}}"#);
        let (port, _) = serve(move |_| (200, both.clone()));
        let c = client(Kind::Nvidia, port, None);
        let t = c.chat_turn("m", &ask(), &[]).unwrap();
        assert_eq!(t.text, "OK");
        assert!(!thinking_only());
        assert_eq!(reported_reasoning_tokens(), Some(40));
        // with nothing but thinking: a plain message, never the thinking
        let only = format!(r#"{{"choices":[{{"message":{{"role":"assistant","content":null,"{field}":"secret chain of thought"}}}}]}}"#);
        let (port, _) = serve(move |_| (200, only.clone()));
        let c = client(Kind::Nvidia, port, None);
        let e = c.chat("m", &ask()).unwrap_err();
        assert_eq!(e, THINKING_ONLY_MESSAGE);
        assert!(!e.contains("secret chain"));
    }
}

#[test]
fn the_model_list_is_read_from_a_real_server_even_when_it_is_very_long_and_a_wrong_key_is_told() {
    let _shared = iyakki_testkit::lock(iyakki_testkit::names::ENV); // flaky: the process-wide state below is shared with the other tests of this run
    let ids: Vec<Value> = (0..1500).map(|i| json!({"id": format!("vendor/model-{i:04}"), "object": "model"})).collect();
    let list = json!({"object": "list", "data": ids}).to_string();
    let (port, seen) = serve(move |r| if r.header("authorization") == Some(format!("Bearer {KEY}").as_str()) { (200, list.clone()) } else { (401, r#"{"error":"bad key"}"#.into()) });
    let c = client(Kind::Nvidia, port, None);
    let models = c.list_models().unwrap();
    assert_eq!(models.len(), 1500);
    assert!(models.iter().any(|m| m == "vendor/model-0000") && models.iter().any(|m| m == "vendor/model-1499"));
    assert_eq!(seen.lock().unwrap()[0].method, "GET");
    assert_eq!(seen.lock().unwrap()[0].path, "/v1/models");
    // a wrong key: the server says 401 and the client says so without printing any key
    std::env::set_var("IYAKKI_TEST_LOOPBACK_KEY", "nvapi-WRONGWRONG");
    let mut cfg = ProviderCfg::new("loop", Kind::Nvidia);
    cfg.base_url = Some(format!("http://127.0.0.1:{port}/v1"));
    cfg.key_ref = Some("env:IYAKKI_TEST_LOOPBACK_KEY".into());
    let up: Arc<dyn Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(10)));
    let bad = ProviderClient::new(cfg, Secrets::new(None, up.clone()), up);
    let e = bad.list_models().unwrap_err();
    assert!(e.contains("401") || e.to_lowercase().contains("key"), "{e}");
    assert!(!e.contains("WRONGWRONG"));
    std::env::set_var("IYAKKI_TEST_LOOPBACK_KEY", KEY);
}
