//! Per-model request options and the provider's own model list, with a fake HTTP client (no network, made-up keys).

use std::sync::{Arc, Mutex};

use iyakki_proxy::{Upstream, UpstreamResponse};
use serde_json::{json, Value};

use crate::chat::Message;
use crate::modelopts::*;
use crate::providers::{reported_reasoning_tokens, reported_usage, thinking_only, Kind, ProviderCfg, ProviderClient, THINKING_ONLY_MESSAGE};
use crate::vault::Secrets;

const NIM_MODEL: &str = "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning";
const FAKE_KEY: &str = "nvapi-FAKEFAKEFAKEFAKE0123456789made-up";

struct Fake {
    calls: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
    reply: Mutex<(u16, String)>,
}
impl Upstream for Fake {
    fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.calls.lock().unwrap().push((m.into(), url.into(), h.to_vec(), String::from_utf8_lossy(b).into()));
        let (status, body) = self.reply.lock().unwrap().clone();
        Ok(UpstreamResponse { status, body: body.into_bytes() })
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-modelopts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn client(id: &str, kind: Kind, dir: Option<&std::path::Path>, status: u16, body: &str) -> (ProviderClient, Arc<Fake>) {
    std::env::set_var("IYAKKI_TEST_MODELOPTS_KEY", FAKE_KEY);
    let mut cfg = ProviderCfg::new(id, kind);
    cfg.key_ref = Some("env:IYAKKI_TEST_MODELOPTS_KEY".into());
    let f = Arc::new(Fake { calls: Default::default(), reply: Mutex::new((status, body.into())) });
    let c = ProviderClient::new(cfg, Secrets::new(None, f.clone()), f.clone());
    (if let Some(d) = dir { c.with_options_dir(d) } else { c }, f)
}

fn ask() -> Vec<Message> {
    vec![Message::user("Reply with the single word OK.")]
}

fn full_options() -> ModelOptions {
    ModelOptions { max_tokens: Some(65536), temperature: Some(0.6), top_p: Some(0.95), reasoning_budget: Some(16384) }
}

const OK_REPLY: &str = r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}],"usage":{"prompt_tokens":12,"completion_tokens":300,"completion_tokens_details":{"reasoning_tokens":290}}}"#;

// -------------------------------------------------------------------------------- validation

#[test]
fn the_option_validation_table() {
    let ok = |f: &str, t: &str| {
        let mut o = ModelOptions::default();
        o.set_field(f, t).unwrap_or_else(|e| panic!("{f}={t}: {e}"));
        o
    };
    let bad = |f: &str, t: &str| {
        let mut o = ModelOptions { max_tokens: Some(5), ..Default::default() };
        let e = o.set_field(f, t).expect_err(&format!("{f}={t} should be refused"));
        assert_eq!(o.max_tokens, Some(5), "nothing changes when refused");
        e
    };
    assert_eq!(ok("max_tokens", "1").max_tokens, Some(1));
    assert_eq!(ok("max_tokens", "1000000").max_tokens, Some(1_000_000));
    for t in ["0", "1000001", "-3", "1.5", "abc"] {
        assert!(bad("max_tokens", t).contains("max_tokens"), "{t}");
    }
    assert_eq!(ok("temperature", "0").temperature, Some(0.0));
    assert_eq!(ok("temperature", "2").temperature, Some(2.0));
    assert_eq!(ok("temperature", "0.6").temperature, Some(0.6));
    for t in ["-0.1", "2.1", "NaN", "inf", "hot"] {
        assert!(bad("temperature", t).contains("temperature"), "{t}");
    }
    assert_eq!(ok("top_p", "1").top_p, Some(1.0));
    assert_eq!(ok("top_p", "0.95").top_p, Some(0.95));
    for t in ["1.01", "-1", "x"] {
        assert!(bad("top_p", t).contains("top_p"), "{t}");
    }
    assert_eq!(ok("reasoning_budget", "0").reasoning_budget, Some(0), "0 is off, and is remembered as off");
    assert_eq!(ok("reasoning_budget", "16384").reasoning_budget, Some(16384));
    for t in ["-1", "1000001", "1e3"] {
        assert!(bad("reasoning_budget", t).contains("reasoning_budget"), "{t}");
    }
    assert!(bad("colour", "1").contains("unknown option"));
    let mut o = full_options();
    o.set_field("top_p", "").unwrap();
    o.set_field("temperature", "none").unwrap();
    assert_eq!((o.top_p, o.temperature), (None, None), "empty clears");
    assert_eq!(full_options().show("temperature"), "0.6");
    assert_eq!(ModelOptions::default().show("top_p"), "not set");
    assert_eq!(ModelOptions { reasoning_budget: Some(0), ..Default::default() }.show("reasoning_budget"), "0 (off)");
    assert_eq!(ModelOptions::default().summary(), "nothing set");
}

// -------------------------------------------------------------------------------- the request body

#[test]
fn the_nvidia_request_body_matches_the_owners_call_exactly() {
    let d = tmp("golden");
    set(&d, "nvidia", NIM_MODEL, &full_options()).unwrap();
    let (c, f) = client("nvidia", Kind::Nvidia, Some(&d), 200, OK_REPLY);
    let t = c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    assert_eq!(t.text, "OK");
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls[0].0, "POST");
    assert_eq!(calls[0].1, "https://integrate.api.nvidia.com/v1/chat/completions");
    assert!(calls[0].2.iter().any(|(k, v)| k == "Authorization" && v == &format!("Bearer {FAKE_KEY}")));
    let sent: Value = serde_json::from_str(&calls[0].3).unwrap();
    let golden = json!({
        "model": NIM_MODEL,
        "messages": [{"role": "user", "content": "Reply with the single word OK."}],
        "stream": false,
        "max_tokens": 65536,
        "reasoning_budget": 16384,
        "temperature": 0.6,
        "top_p": 0.95
    });
    assert_eq!(sent, golden);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn options_are_sent_only_when_set_and_the_reasoning_budget_only_to_providers_that_take_it() {
    let d = tmp("onlyset");
    // nothing saved: the body has none of them
    let (c, f) = client("nvidia", Kind::Nvidia, Some(&d), 200, OK_REPLY);
    c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[0].3).unwrap();
    for k in FIELDS {
        assert!(b.get(k).is_none(), "{k} must not be sent when unset: {b}");
    }
    // a budget of 0 is off: not sent; temperature alone is sent alone
    set(&d, "nvidia", NIM_MODEL, &ModelOptions { temperature: Some(0.2), reasoning_budget: Some(0), ..Default::default() }).unwrap();
    c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[1].3).unwrap();
    assert_eq!(b["temperature"], json!(0.2));
    assert!(b.get("reasoning_budget").is_none() && b.get("max_tokens").is_none() && b.get("top_p").is_none(), "{b}");
    // another provider with the same numbers: the budget stays home
    set(&d, "openai", "gpt-4o", &full_options()).unwrap();
    let (c, f) = client("openai", Kind::OpenAi, Some(&d), 200, OK_REPLY);
    c.chat_turn("gpt-4o", &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[0].3).unwrap();
    assert_eq!((b["max_tokens"].clone(), b["top_p"].clone()), (json!(65536), json!(0.95)));
    assert!(b.get("reasoning_budget").is_none(), "{b}");
    assert_eq!(used_by(Kind::OpenAi, "reasoning_budget"), Err("not used by this provider"));
    assert_eq!(used_by(Kind::Anthropic, "reasoning_budget"), Err("not used by this provider"));
    assert!(used_by(Kind::Nvidia, "reasoning_budget").is_ok());
    // options belong to one model: another model of the same provider gets none
    c.chat_turn("gpt-4o-mini", &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[1].3).unwrap();
    assert!(b.get("max_tokens").is_none(), "{b}");
    // Anthropic: its max_tokens is replaced, temperature added, no budget
    set(&d, "claude", "claude-sonnet-4-5", &full_options()).unwrap();
    let (c, f) = client("claude", Kind::Anthropic, Some(&d), 200, r#"{"content":[{"type":"text","text":"OK"}]}"#);
    c.chat_turn("claude-sonnet-4-5", &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[0].3).unwrap();
    assert_eq!((b["max_tokens"].clone(), b["temperature"].clone()), (json!(65536), json!(0.6)));
    assert!(b.get("reasoning_budget").is_none() && b.get("thinking").is_none(), "{b}");
    // Gemini: nothing, the call is the same as before
    let (c, f) = client("gem", Kind::Gemini, Some(&d), 200, r#"{"candidates":[{"content":{"parts":[{"text":"OK"}]}}]}"#);
    set(&d, "gem", "gemini-2.0-flash", &full_options()).unwrap();
    c.chat_turn("gemini-2.0-flash", &ask(), &[]).unwrap();
    let b = f.calls.lock().unwrap()[0].3.clone();
    assert!(!b.contains("max_tokens") && !b.contains("temperature") && !b.contains("top_p"), "{b}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_hand_edited_wild_value_is_not_sent() {
    let d = tmp("wild");
    std::fs::write(d.join(FILE), r#"{"schema":1,"models":{"nvidia/m":{"temperature":9,"max_tokens":0}}}"#).unwrap();
    assert!(get(&d, "nvidia", "m").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

// -------------------------------------------------------------------------------- the file

#[test]
fn damaged_or_newer_files_are_moved_aside_or_left_alone_and_never_overwritten() {
    SCHEMA.check().unwrap();
    let d = tmp("file");
    let f = d.join(FILE);
    std::fs::write(&f, "{ half").unwrap();
    assert!(get(&d, "nvidia", "m").is_empty());
    let moved: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
    assert_eq!(moved.len(), 1, "the damaged bytes were moved aside");
    assert_eq!(std::fs::read_to_string(moved[0].path()).unwrap(), "{ half");
    set(&d, "nvidia", "m", &ModelOptions { top_p: Some(0.5), ..Default::default() }).unwrap();
    assert_eq!(get(&d, "nvidia", "m").top_p, Some(0.5));
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(on_disk["schema"], json!(1));
    // a newer release's file: read, never rewritten
    let newer = r#"{"schema": 9, "models": {"nvidia/m": {"top_p": 0.25}}}"#;
    std::fs::write(&f, newer).unwrap();
    assert_eq!(get(&d, "nvidia", "m").top_p, Some(0.25));
    assert!(read_only(&d).is_some());
    assert!(set(&d, "nvidia", "m", &full_options()).is_err());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), newer);
    // an option removed to nothing removes the entry
    std::fs::remove_file(&f).unwrap();
    set(&d, "a", "m", &full_options()).unwrap();
    set(&d, "a", "m", &ModelOptions::default()).unwrap();
    assert!(all(&d).is_empty());
    assert!(set(&d, "Bad Id", "m", &full_options()).is_err() && set(&d, "a", "bad model!", &full_options()).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

// -------------------------------------------------------------------------------- reasoning replies

#[test]
fn reasoning_content_is_never_the_answer_and_the_reasoning_tokens_are_counted() {
    let (c, _) = client("nvidia", Kind::Nvidia, None, 200, r#"{"choices":[{"message":{"role":"assistant","content":"OK","reasoning_content":"The user wants one word, so I will say OK because..."}}],"usage":{"prompt_tokens":12,"completion_tokens":300,"completion_tokens_details":{"reasoning_tokens":290}}}"#);
    let t = c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    assert_eq!(t.text, "OK");
    assert!(!thinking_only());
    assert_eq!(reported_usage(), Some((12, 300)), "the completion count already holds the thinking tokens");
    assert_eq!(reported_reasoning_tokens(), Some(290));
    // `reasoning` (Ollama, vLLM) is the same
    let (c, _) = client("nvidia", Kind::Nvidia, None, 200, r#"{"choices":[{"message":{"content":"OK","reasoning":"thinking out loud"}}]}"#);
    assert_eq!(c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap().text, "OK");
    assert_eq!(reported_reasoning_tokens(), None, "cleared at the start of every call");
}

#[test]
fn a_reply_with_only_reasoning_ends_with_a_plain_message_not_the_thinking() {
    for field in ["reasoning_content", "reasoning"] {
        let body = format!(r#"{{"choices":[{{"message":{{"role":"assistant","content":null,"{field}":"secret chain of thought"}}}}]}}"#);
        let (c, _) = client("nvidia", Kind::Nvidia, None, 200, &body);
        let t = c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
        assert!(t.text.is_empty() && t.calls.is_empty(), "{field}: the thinking is not the answer: {t:?}");
        assert!(thinking_only());
        let e = c.chat(NIM_MODEL, &ask()).unwrap_err();
        assert_eq!(e, THINKING_ONLY_MESSAGE);
        assert!(!e.contains("secret chain"));
    }
}

#[test]
fn a_streamed_reply_with_only_reasoning_is_not_shown_either() {
    struct Sse(String);
    impl iyakki_proxy::Streamer for Sse {
        fn open(&self, _: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
            Err("no GET".into())
        }
        fn post(&self, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
            Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(self.0.clone().into_bytes())) })
        }
    }
    let (c, _) = client("nvidia", Kind::Nvidia, None, 200, "{}");
    let body = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"secret chain of thought\"}}]}\n\ndata: [DONE]\n\n";
    let mut shown = String::new();
    let turn = c.chat_turn_stream(NIM_MODEL, &ask(), &[], &Sse(body.into()), &mut |p| {
        shown.push_str(p);
        true
    }).unwrap();
    assert!(turn.text.is_empty() && shown.is_empty(), "{turn:?} {shown:?}");
    assert!(thinking_only());
}

// -------------------------------------------------------------------------------- the provider's list

const NIM_LIST: &str = r#"{"object":"list","data":[{"id":"nvidia/nemotron-3-nano-omni-30b-a3b-reasoning","object":"model"},{"id":"meta/llama-3.3-70b-instruct","object":"model"},{"id":"meta/llama-3.3-70b-instruct"},{"id":"bad name!"}]}"#;

#[test]
fn the_provider_list_is_parsed_sorted_cached_and_asked_for_with_the_key_only_in_a_header() {
    forget_cached("nimlist");
    let (c, f) = client("nimlist", Kind::Nvidia, None, 200, NIM_LIST);
    let ids = fetch_models(&c).unwrap();
    assert_eq!(ids, vec!["meta/llama-3.3-70b-instruct", NIM_MODEL], "sorted, de-duplicated, only usable names");
    let calls = f.calls.lock().unwrap();
    assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), ("GET", "https://integrate.api.nvidia.com/v1/models"));
    assert!(calls[0].2.iter().any(|(k, v)| k == "Authorization" && v.contains(FAKE_KEY)) && !calls[0].1.contains("nvapi") && !calls[0].3.contains("nvapi"));
    assert_eq!(cached_models("nimlist"), Some(ids));
    // a model the list does not hold is said so in plain words; one it holds is not
    assert_eq!(not_offered("nimlist", "nvidia/meta/llama2-70b").unwrap(), "The model nvidia/meta/llama2-70b is not offered by this provider for this key. Pick one from the list.");
    assert!(not_offered("nimlist", NIM_MODEL).is_none());
    assert!(not_offered("never-fetched", "anything").is_none(), "no list, no claim");
    forget_cached("nimlist");
    assert!(cached_models("nimlist").is_none());
}

#[test]
fn list_failures_are_in_plain_words_and_never_cached() {
    forget_cached("nimerr");
    let (c, _) = client("nimerr", Kind::Nvidia, None, 401, r#"{"error":{"message":"Incorrect API key provided: nvapi-FAKEFAKEFAKEFAKE0123456789made-up"}}"#);
    let e = fetch_models(&c).unwrap_err();
    assert!(e.starts_with("nimerr refused the key") && e.contains("Settings -> Models"), "{e}");
    assert!(!e.contains("FAKEFAKE"), "{e}");
    let (c, _) = client("nimerr", Kind::Compat, None, 404, "404 page not found");
    let mut cfg = c.cfg.clone();
    cfg.base_url = Some("http://box.example:8000/v1".into());
    let fake = Arc::new(Fake { calls: Default::default(), reply: Mutex::new((404, "404 page not found".into())) });
    let c = ProviderClient::new(cfg, Secrets::new(None, fake.clone()), fake);
    let e = fetch_models(&c).unwrap_err();
    assert!(e.contains("did not find that address"), "{e}");
    assert!(cached_models("nimerr").is_none());
    let (c, _) = client("nimerr", Kind::Nvidia, None, 200, r#"{"unexpected":true}"#);
    assert!(fetch_models(&c).is_err());
}

#[test]
fn a_light_one_shot_call_asks_nvidia_for_a_small_thinking_budget_and_other_providers_for_nothing_new() {
    let d = tmp("minimal");
    set(&d, "nvidia", NIM_MODEL, &full_options()).unwrap();
    let (c, f) = client("nvidia", Kind::Nvidia, Some(&d), 200, OK_REPLY);
    crate::providers::set_thread_reasoning(Some(crate::dictfix::REASONING));
    c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    crate::providers::set_thread_reasoning(None);
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[0].3).unwrap();
    assert_eq!(b["reasoning_budget"], json!(MINIMAL_BUDGET), "the person's 16384 is capped for this call: {b}");
    // without the hint the person's own budget is sent, as before
    c.chat_turn(NIM_MODEL, &ask(), &[]).unwrap();
    let b: Value = serde_json::from_str(&f.calls.lock().unwrap()[1].3).unwrap();
    assert_eq!(b["reasoning_budget"], json!(16384));
    // another provider is sent no thinking field because of the hint
    let (c2, f2) = client("openai", Kind::OpenAi, Some(&d), 200, OK_REPLY);
    crate::providers::set_thread_reasoning(Some(crate::dictfix::REASONING));
    c2.chat_turn("gpt-4o", &ask(), &[]).unwrap();
    crate::providers::set_thread_reasoning(None);
    let b: Value = serde_json::from_str(&f2.calls.lock().unwrap()[0].3).unwrap();
    assert!(b.get("reasoning_budget").is_none() && b.get("reasoning_effort").is_none(), "{b}");
    let _ = std::fs::remove_dir_all(&d);
}
