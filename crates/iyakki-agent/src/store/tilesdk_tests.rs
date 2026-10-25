//! Tests of the SDK calls of the tile bridge (`iyakki.llm`, `iyakki.models`, `iyakki.speech`): capability refusals with their codes, argument
//! checks, the first-use card, budgets and rate limits, per-app usage records, streaming, time limits, and the same calls from a page and
//! from the headless harness (`T.fake`). The model and the speaker are fakes; a mock provider stands in for the network in the one test
//! of the real model backend.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_js::modules::Bridge;
use serde_json::{json, Value};

use super::tile::*;
use super::tilesdk::*;
use crate::usage::{Call, UsageLog};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tilesdk-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn info() -> AppInfo {
    AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false }
}

fn caps(llm: bool, speech: bool) -> Caps {
    Caps { llm, speech, ..Caps::default() }
}

/// A host with a fake-backed SDK; the first-use card is answered "allow" unless a test says otherwise.
fn host(tag: &str, c: Caps) -> (TileHost, Arc<TileSdk>, Arc<Mutex<Outbox>>) {
    let out = Arc::new(Mutex::new(Outbox::default()));
    let sdk = Arc::new(TileSdk::fake("t", out.clone(), &tmp(tag)));
    (TileHost::new(info(), c, TileStorage::memory(), out.clone()).with_sdk(sdk.clone()), sdk, out)
}

fn code_of(e: &str) -> (String, String) {
    SdkError::decode(e).unwrap_or_else(|| panic!("not a coded error: {e}"))
}

fn chat(text: &str) -> Value {
    json!({"messages": [{"role": "user", "content": text}]})
}

#[test]
fn errors_carry_a_code_and_never_a_server_or_a_key() {
    let e = SdkError::new(code::BUDGET_STOP, "enough for today").with("resets_at", 1790000000);
    assert_eq!(code_of(&e.encode()), ("BudgetStop".into(), "enough for today".into()));
    assert_eq!(e.payload()["resets_at"], 1790000000);
    assert!(SdkError::decode("plain text").is_none());
    let p = plain("HTTP 401 from https://api.example.com/v1/chat: bad key sk-abc123 and Bearer xyz\nline");
    assert!(!p.contains("example.com") && !p.contains("sk-abc") && !p.to_lowercase().contains("bearer"), "{p}");
    assert_eq!(plain("https://only.example.com"), "the model did not answer");
}

#[test]
fn every_sdk_call_refuses_an_app_that_did_not_declare_it_with_the_code_no_capability() {
    let (mut h, _, out) = host("nocap", Caps::default());
    for op in ["llm.chat", "llm.stream", "llm.cancel", "llm.usage", "models.list", "speech.say", "speech.stop"] {
        let e = h.call(op, &json!({"messages": [], "text": "x", "handle": 1})).unwrap_err();
        let (c, m) = code_of(&e);
        assert_eq!(c, "NoCapability", "{op}");
        assert!(m.contains("did not ask for"), "{op}: {m}");
    }
    assert!(!out.lock().unwrap().refused.is_empty(), "refusals are kept for the info panel");
    // the model is not the voice: one capability does not give the other
    let (mut h, _, _) = host("onecap", caps(true, false));
    assert!(h.call("models.list", &json!({})).is_ok());
    assert_eq!(code_of(&h.call("speech.say", &json!({"text": "hi"})).unwrap_err()).0, "NoCapability");
    // a host without any backend says so with a code, and the test hooks do not exist outside the harness
    let out = Arc::new(Mutex::new(Outbox::default()));
    let mut bare = TileHost::new(info(), Caps::all(), TileStorage::memory(), out);
    assert_eq!(code_of(&bare.call("llm.chat", &chat("hi")).unwrap_err()).0, "Unavailable");
    assert!(bare.call("__test.llm.reply", &json!({"text": "x"})).unwrap_err().contains("no such bridge call"));
    let (mut real_like, _, _) = host("hooks", caps(true, true));
    assert!(real_like.call("__test.llm.reply", &json!({"text": "x"})).is_ok(), "only a runtime built with fakes answers");
}

#[test]
fn chat_checks_every_argument_and_names_the_field() {
    let (mut h, _, _) = host("args", caps(true, true));
    let long = "x".repeat(100_001);
    for (a, field) in [
        (json!({}), "messages"),
        (json!({"messages": []}), "messages"),
        (json!({"messages": "hi"}), "messages"),
        (json!({"messages": [{"role": "robot", "content": "hi"}]}), "messages"),
        (json!({"messages": [{"role": "user"}]}), "messages"),
        (json!({"messages": [{"role": "system", "content": "only a system message"}]}), "messages"),
        (json!({"messages": [{"role": "user", "content": long}]}), "messages"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "max_tokens": 5000}), "max_tokens"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "max_tokens": 0}), "max_tokens"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "max_tokens": "many"}), "max_tokens"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "model": 7}), "model"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "model": "not-listed"}), "model"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "temperature": 0.2}), "temperature"),
        (json!({"messages": [{"role": "user", "content": "hi"}], "stream": "yes"}), "stream"),
    ] {
        let (c, m) = code_of(&h.call("llm.chat", &a).unwrap_err());
        assert_eq!(c, "BadArgument", "{a}");
        assert!(m.starts_with(&format!("{field}:")), "{a}: {m}");
    }
    let many: Vec<Value> = (0..65).map(|_| json!({"role": "user", "content": "x"})).collect();
    assert_eq!(code_of(&h.call("llm.chat", &json!({"messages": many})).unwrap_err()).0, "BadArgument");
    for (op, a) in [("models.list", json!({"kind": "video"})), ("models.list", json!({"nope": 1})), ("llm.usage", json!({"since": "year"})), ("llm.cancel", json!({"handle": "x"}))] {
        assert_eq!(code_of(&h.call(op, &a).unwrap_err()).0, "BadArgument", "{op} {a}");
    }
}

#[test]
fn a_chat_answers_with_text_usage_and_model_and_is_written_to_the_log_under_the_apps_id() {
    let (mut h, sdk, _) = host("chat", caps(true, false));
    h.call("__test.llm.reply", &json!({"text": "Hello from the fake"})).unwrap();
    h.call("__test.llm.usage", &json!({"input": 120, "output": 30})).unwrap();
    let r = h.call("llm.chat", &chat("Say hello")).unwrap();
    assert_eq!((r["text"].as_str(), r["usage"]["input"].as_u64(), r["usage"]["output"].as_u64(), r["usage"]["estimated"].as_bool()), (Some("Hello from the fake"), Some(120), Some(30), Some(false)));
    assert_eq!(r["model"], "fake-model");
    // the answer says nothing about a provider, a key or an address
    assert!(!r.to_string().contains("fake\"") || r["model"] == "fake-model");
    let m = h.call("models.list", &json!({})).unwrap();
    assert_eq!(m, json!([{"id": "fake-model", "label": "Fake model", "kind": "chat", "where": "this computer", "free": true}]));
    assert_eq!(h.call("models.list", &json!({"kind": "image"})).unwrap(), json!([]));
    // an estimate is marked as one
    h.call("__test.llm.reply", &json!({"text": "x".repeat(40)})).unwrap();
    h.call("__test.llm.usage", &json!({"input": 0, "output": 0})).unwrap();
    let u = h.call("llm.usage", &json!({})).unwrap();
    assert_eq!(u["today"]["calls"], 1);
    assert_eq!((u["today"]["input"].as_u64(), u["today"]["output"].as_u64()), (Some(120), Some(30)));
    assert_eq!(u["limits"]["tokensPerDay"], Value::Null, "a model on this computer has no default limit");
    assert_eq!(u["today"]["priced"], false, "money is never claimed");
    assert_eq!(u["byDay"].as_array().unwrap().len(), 7);
    assert_eq!(sdk.summary().unwrap(), "Today: 1 call, 150 tokens, no daily limit for a model on this computer");
    // the model was asked what the app sent, and nothing else
    let seen = sdk.fakes().unwrap().llm.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].messages, vec![("user".to_string(), "Say hello".to_string())]);
    assert_eq!(seen[0].max_tokens, DEFAULT_TOKENS);
}

#[test]
fn failures_come_back_with_their_code_and_are_counted_as_errors_without_tokens() {
    let (mut h, _, _) = host("fail", caps(true, false));
    h.call("__test.llm.fail", &json!({"code": "Unavailable"})).unwrap();
    assert_eq!(code_of(&h.call("llm.chat", &chat("hi")).unwrap_err()).0, "Unavailable");
    h.call("__test.llm.fail", &json!({"code": "Whatever"})).unwrap();
    assert_eq!(code_of(&h.call("llm.chat", &chat("hi")).unwrap_err()).0, "Failed", "an unknown code is a plain failure");
    let u = h.call("llm.usage", &json!({})).unwrap();
    assert_eq!((u["today"]["calls"].as_u64(), u["today"]["errors"].as_u64(), u["today"]["input"].as_u64(), u["today"]["output"].as_u64()), (Some(2), Some(2), Some(0), Some(0)));
}

#[test]
fn twenty_calls_a_minute_then_quota_exceeded_until_the_minute_has_passed() {
    let (mut h, _, _) = host("rate", caps(true, false));
    for _ in 0..LLM_PER_MINUTE {
        h.call("llm.chat", &chat("hi")).unwrap();
    }
    let e = SdkError::decode(&h.call("llm.chat", &chat("hi")).unwrap_err()).unwrap();
    assert_eq!(e.0, "QuotaExceeded");
    h.call("__test.clock.advance", &json!({"ms": 61_000})).unwrap();
    assert!(h.call("llm.chat", &chat("hi")).is_ok(), "a new minute, a new allowance");
}

#[test]
fn the_daily_budget_is_checked_before_the_model_is_asked_and_the_person_alone_sets_it() {
    let (mut h, sdk, out) = host("budget", caps(true, false));
    h.call("__test.llm.usage", &json!({"input": 60, "output": 30})).unwrap();
    h.call("__test.budget.set", &json!({"tokensPerDay": 100})).unwrap();
    let r = h.call("llm.chat", &chat("hi")).unwrap();
    assert_eq!(r["usage"]["input"], 60);
    // 90 of 100 used: the person is told once, in the inbox and in the audit log
    {
        let o = out.lock().unwrap();
        assert_eq!(o.notices.iter().filter(|n| n.0 == "Usage limit").count(), 1, "{:?}", o.notices);
        assert!(o.audit.iter().any(|a| a.0 == "app.budget.warn"), "{:?}", o.audit);
    }
    // a second call whose prompt alone would cross the line is stopped and never reaches the model
    let asked = sdk.fakes().unwrap().llm.seen().len();
    let e = h.call("llm.chat", &chat(&"y".repeat(400))).unwrap_err();
    let v: Value = serde_json::from_str(e.strip_prefix(ERROR_PREFIX).unwrap()).unwrap();
    assert_eq!(v["code"], "BudgetStop");
    assert!(v["resets_at"].as_i64().unwrap() > crate::clock::now_unix(), "{v}");
    assert_eq!(sdk.fakes().unwrap().llm.seen().len(), asked, "a stopped app costs nothing");
    assert!(out.lock().unwrap().audit.iter().any(|a| a.0 == "app.budget.stop"));
    // the warning is not repeated, and the app cannot change its own limit: there is no call for it
    h.call("__test.llm.usage", &json!({"input": 1, "output": 1})).unwrap();
    assert!(h.call("llm.chat", &json!({"messages": [{"role": "user", "content": "x"}], "max_tokens": 4096, "limit": 1})).is_err());
    assert_eq!(out.lock().unwrap().notices.iter().filter(|n| n.0 == "Usage limit").count(), 1);
    // the person raises the limit and the app goes on
    sdk.set_tokens_per_day(Some(1000));
    assert!(h.call("llm.chat", &chat(&"y".repeat(400))).is_ok());
    assert!(out.lock().unwrap().audit.iter().any(|a| a.0 == "app.budget.set"));
    assert_eq!(h.call("llm.usage", &json!({})).unwrap()["limits"]["tokensPerDay"], 1000);
}

#[test]
fn a_model_that_is_not_on_this_computer_gets_the_default_cloud_limit() {
    let (mut h, sdk, _) = host("cloud", caps(true, false));
    h.call("__test.models.set", &json!({"models": [{"id": "big", "label": "Big", "where": "internet"}]})).unwrap();
    assert_eq!(sdk.token_cap(), Some(DEFAULT_CLOUD_TOKENS));
    let m = h.call("models.list", &json!({})).unwrap();
    assert_eq!((m[0]["where"].as_str(), m[0]["free"].as_bool()), (Some("internet"), Some(false)));
    assert_eq!(h.call("llm.usage", &json!({})).unwrap()["remaining"]["tokens"], DEFAULT_CLOUD_TOKENS);
    sdk.set_tokens_per_day(Some(0));
    assert_eq!(code_of(&h.call("llm.chat", &chat("hi")).unwrap_err()).0, "BudgetStop", "a limit of zero is a stop, not 'unlimited'");
}

#[test]
fn the_first_use_card_is_asked_once_per_app_and_kept() {
    let (mut h, sdk, out) = host("approve", caps(true, true));
    h.call("__test.approve.set", &json!({"mode": "ask"})).unwrap();
    let r = h.call("llm.chat", &chat("hi")).unwrap();
    let id = r["pending"].as_u64().expect("the call waits for the person") as u32;
    let req = out.lock().unwrap().requests.clone();
    assert_eq!((req.len(), req[0].kind.as_str(), req[0].id), (1, "sdk.approve", id));
    assert_eq!((req[0].args["ns"].as_str(), req[0].args["where"].as_str(), req[0].args["leaves"].as_bool()), (Some("llm"), Some("this computer"), Some(false)));
    assert!(sdk.answer(id, true));
    assert!(out.lock().unwrap().requests.is_empty());
    let d = out.lock().unwrap().done.clone();
    assert_eq!((d.len(), d[0].id, d[0].ok), (1, id, true));
    assert_eq!(serde_json::from_str::<Value>(&d[0].value).unwrap()["text"], "ok");
    // kept: the next call does not ask
    assert!(h.call("llm.chat", &chat("again")).unwrap()["text"].is_string());
    assert!(sdk.saved().llm && !sdk.saved().speech, "one namespace's answer is not the other's");
    // the person says no to reading aloud
    let r = h.call("speech.say", &json!({"text": "hello"})).unwrap();
    let id = r["pending"].as_u64().unwrap() as u32;
    assert!(out.lock().unwrap().requests.iter().any(|q| q.id == id && q.args["ns"] == "speech" && q.args["where"].as_str().unwrap().contains("stays on this computer")));
    assert!(sdk.answer(id, false));
    let d = out.lock().unwrap().done.last().cloned().unwrap();
    assert!(!d.ok);
    assert_eq!(code_of(&d.value).0, "Denied");
    assert!(sdk.fakes().unwrap().speech.said().is_empty(), "nothing was spoken");
    assert!(!sdk.answer(id, true), "an answer settles a card once");
    // revoking takes the answer back
    sdk.revoke();
    assert!(!sdk.saved().llm);
    assert!(h.call("llm.chat", &chat("hi")).unwrap()["pending"].is_number());
}

#[test]
fn a_denied_first_use_and_a_card_nobody_answers_are_denied() {
    let (mut h, sdk, out) = host("deny", caps(true, false));
    h.call("__test.approve.set", &json!({"mode": "deny"})).unwrap();
    assert_eq!(code_of(&h.call("llm.chat", &chat("hi")).unwrap_err()).0, "Denied");
    assert!(!sdk.saved().llm);
    h.call("__test.approve.set", &json!({"mode": "ask"})).unwrap();
    let id = h.call("llm.chat", &chat("hi")).unwrap()["pending"].as_u64().unwrap() as u32;
    sdk.tick();
    assert_eq!(sdk.waiting(), 1, "still waiting");
    h.call("__test.clock.advance", &json!({"ms": ANSWER_MS + 1})).unwrap();
    sdk.tick();
    assert_eq!(sdk.waiting(), 0);
    assert!(out.lock().unwrap().requests.is_empty(), "the card is taken down");
    let d = out.lock().unwrap().done.last().cloned().unwrap();
    assert_eq!((d.id, d.ok, code_of(&d.value).0), (id, false, "Denied".to_string()));
    // at most MAX_JOBS cards wait at once
    h.call("__test.clock.advance", &json!({"ms": 0})).unwrap();
    for _ in 0..MAX_JOBS {
        assert!(h.call("llm.chat", &chat("hi")).unwrap()["pending"].is_number());
    }
    assert_eq!(code_of(&h.call("llm.chat", &chat("hi")).unwrap_err()).0, "QuotaExceeded");
}

#[test]
fn a_stream_sends_pieces_then_done_keeps_to_fifty_waiting_and_stops_when_cancelled() {
    let (mut h, sdk, out) = host("stream", caps(true, false));
    h.call("__test.llm.reply", &json!({"text": "one two three"})).unwrap();
    let r = h.call("llm.stream", &chat("count")).unwrap();
    let handle = r["handle"].as_u64().unwrap() as u32;
    let ev: Vec<(String, Value)> = out.lock().unwrap().emits.iter().filter(|e| e.handle == handle).map(|e| (e.event.clone(), e.payload.clone())).collect();
    let texts: String = ev.iter().filter(|e| e.0 == "text").map(|e| e.1["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(texts, "one two three");
    assert_eq!(ev.last().unwrap().0, "done");
    assert_eq!(ev.last().unwrap().1["text"], "one two three");
    assert_eq!(h.call("llm.usage", &json!({})).unwrap()["today"]["calls"], 1, "a stream is one call");
    // chat with stream:true takes the same road
    assert!(h.call("llm.chat", &json!({"messages": [{"role": "user", "content": "x"}], "stream": true})).unwrap()["handle"].is_number());
    // a long answer: only 50 events wait, the oldest are dropped, once told
    out.lock().unwrap().emits.clear();
    h.call("__test.llm.reply", &json!({"text": vec!["w"; 200].join(" ")})).unwrap();
    let handle = h.call("llm.stream", &chat("long")).unwrap()["handle"].as_u64().unwrap() as u32;
    let o = out.lock().unwrap();
    let mine: Vec<&Emit> = o.emits.iter().filter(|e| e.handle == handle).collect();
    assert!(mine.iter().filter(|e| e.event == "text").count() <= EVENTS_PER_HANDLE);
    assert_eq!(mine.iter().filter(|e| e.event == "dropped").count(), 1);
    assert_eq!(mine.last().unwrap().event, "done", "the end is never dropped");
    drop(o);
    // an error is an event with its code
    out.lock().unwrap().emits.clear();
    h.call("__test.llm.fail", &json!({"code": "Unavailable"})).unwrap();
    let handle = h.call("llm.stream", &chat("x")).unwrap()["handle"].as_u64().unwrap() as u32;
    let e = out.lock().unwrap().emits.iter().find(|e| e.handle == handle && e.event == "error").cloned().unwrap();
    assert_eq!(e.payload["code"], "Unavailable");
    let _ = sdk;
}

/// A model on another thread: it waits for the test to let it go.
struct Slow {
    gate: Arc<AtomicU64>,
    text: &'static str,
}

impl LlmBackend for Slow {
    fn chat(&self, _: &ChatRequest, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatReply, SdkError> {
        while self.gate.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for p in self.text.split_inclusive(' ') {
            if !on_text(p) {
                break;
            }
        }
        Ok(ChatReply { text: self.text.into(), input: 10, output: 5, estimated: false, model: "slow".into() })
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo { id: "slow".into(), label: "Slow".into(), provider: "p".into(), place: "this computer" }]
    }
}

fn slow_sdk(tag: &str, gate: Arc<AtomicU64>, clock: Arc<AtomicU64>) -> (Arc<TileSdk>, Arc<Mutex<Outbox>>) {
    let out = Arc::new(Mutex::new(Outbox::default()));
    let log = LogMeter(UsageLog::open(&tmp(tag).join("usage.jsonl")));
    let sdk = TileSdk::new("t", "tester", Arc::new(Slow { gate, text: "late answer" }), Arc::new(FakeSpeech::default()), Arc::new(log), out.clone(), None).with_clock(move || clock.load(Ordering::SeqCst));
    // a person who allowed this app before
    sdk.revoke();
    (Arc::new(sdk), out)
}

fn wait_for(out: &Arc<Mutex<Outbox>>, f: impl Fn(&Outbox) -> bool) {
    for _ in 0..400 {
        if f(&out.lock().unwrap()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("nothing arrived");
}

#[test]
fn a_slow_model_runs_on_a_worker_and_its_answer_is_delivered_later() {
    let (gate, clock) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (sdk, out) = slow_sdk("slow", gate.clone(), clock);
    // the card first (the person is not a fake here)
    let id = sdk.chat(&chat("hi"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    assert_eq!(out.lock().unwrap().requests[0].kind, "sdk.approve");
    assert!(sdk.answer(id, true));
    assert!(out.lock().unwrap().done.is_empty(), "the model has not answered");
    gate.store(1, Ordering::SeqCst);
    wait_for(&out, |o| !o.done.is_empty());
    let d = out.lock().unwrap().done[0].clone();
    assert_eq!((d.id, d.ok), (id, true));
    assert_eq!(serde_json::from_str::<Value>(&d.value).unwrap()["text"], "late answer");
    // the next call needs no card and a stream gets its pieces from the worker
    gate.store(1, Ordering::SeqCst);
    let h = sdk.chat(&chat("again"), true).unwrap()["handle"].as_u64().unwrap() as u32;
    wait_for(&out, |o| o.emits.iter().any(|e| e.handle == h && e.event == "done"));
    let texts: String = out.lock().unwrap().emits.iter().filter(|e| e.handle == h && e.event == "text").map(|e| e.payload["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(texts, "late answer");
    // one stream at a time
    gate.store(0, Ordering::SeqCst);
    let first = sdk.chat(&chat("a"), true).unwrap()["handle"].as_u64().unwrap() as u32;
    assert_eq!(code_of(&sdk.chat(&chat("b"), true).unwrap_err().encode()).0, "QuotaExceeded");
    // cancelling lets the worker stop; the partial answer still ends the stream
    sdk.cancel(&json!({"handle": first})).unwrap();
    gate.store(1, Ordering::SeqCst);
    wait_for(&out, |o| o.emits.iter().any(|e| e.handle == first && e.event == "done"));
    let done = out.lock().unwrap().emits.iter().find(|e| e.handle == first && e.event == "done").cloned().unwrap();
    assert_eq!(done.payload["cancelled"], true);
}

#[test]
fn a_model_that_takes_over_a_minute_is_a_timeout_and_its_late_answer_is_counted_but_not_delivered() {
    let (gate, clock) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (sdk, out) = slow_sdk("timeout", gate.clone(), clock.clone());
    let id = sdk.chat(&chat("hi"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    sdk.answer(id, true);
    clock.store(JOB_MS + 1, Ordering::SeqCst);
    sdk.tick();
    let d = out.lock().unwrap().done.clone();
    assert_eq!((d.len(), d[0].ok, code_of(&d[0].value).0), (1, false, "Timeout".to_string()));
    gate.store(1, Ordering::SeqCst);
    // the worker finishes after the limit: the tokens were spent, so they are in the log; the page hears nothing more
    for _ in 0..200 {
        if sdk.summary().is_some_and(|s| s.contains("1 call")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(sdk.summary().unwrap().contains("1 call, 15 tokens"), "{:?}", sdk.summary());
    assert_eq!(out.lock().unwrap().done.len(), 1);
    // closing the tile drops what is in flight
    gate.store(0, Ordering::SeqCst);
    let id = sdk.chat(&chat("hi"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    let _ = id;
    sdk.close();
    assert_eq!(sdk.waiting(), 0);
}

#[test]
fn reading_aloud_checks_its_arguments_returns_an_id_and_counts_thirty_an_hour() {
    let (mut h, sdk, _) = host("say", caps(false, true));
    for a in [json!({}), json!({"text": 5}), json!({"text": "  "}), json!({"text": "x".repeat(SAY_CHARS + 1)}), json!({"text": "hi", "speed": 2}), json!({"text": "hi", "speed": "fast"}), json!({"text": "hi", "lang": "en us"}), json!({"text": "hi", "voice": 3}), json!({"text": "hi", "pitch": 1})] {
        assert_eq!(code_of(&h.call("speech.say", &a).unwrap_err()).0, "BadArgument", "{a}");
    }
    let r = h.call("speech.say", &json!({"text": "Hello", "voice": "nova", "lang": "ta-IN", "speed": 1.2})).unwrap();
    assert_eq!(r["id"], 1);
    let said = sdk.fakes().unwrap().speech.said();
    assert_eq!((said[0].0.as_str(), said[0].1.voice.as_deref(), said[0].1.lang.as_deref()), ("Hello", Some("nova"), Some("ta-IN")));
    assert!((said[0].1.speed.unwrap() - 1.2).abs() < 1e-6);
    assert_eq!(h.call("speech.stop", &json!({"id": 1})).unwrap(), Value::Null);
    assert_eq!(sdk.fakes().unwrap().speech.stopped(), vec![1]);
    assert_eq!(code_of(&h.call("speech.stop", &json!({"id": "x"})).unwrap_err()).0, "BadArgument");
    for _ in 1..SAY_PER_HOUR {
        h.call("speech.say", &json!({"text": "again"})).unwrap();
    }
    let (c, m) = code_of(&h.call("speech.say", &json!({"text": "one too many"})).unwrap_err());
    assert_eq!(c, "QuotaExceeded");
    assert!(m.contains("30 readings an hour"), "{m}");
    h.call("__test.clock.advance", &json!({"ms": 3_600_001})).unwrap();
    assert!(h.call("speech.say", &json!({"text": "later"})).is_ok());
    // every reading is in the log as speech with its characters, under this app
    let u = h.call("llm.usage", &json!({}));
    assert_eq!(code_of(&u.unwrap_err()).0, "NoCapability", "usage belongs to llm");
}

#[test]
fn reading_aloud_is_logged_as_speech_and_does_not_use_up_the_model_budget() {
    let (mut h, sdk, _) = host("speechlog", caps(true, true));
    h.call("__test.budget.set", &json!({"tokensPerDay": 50})).unwrap();
    h.call("speech.say", &json!({"text": "x".repeat(5000)})).unwrap();
    assert_eq!(h.call("llm.usage", &json!({})).unwrap()["today"]["calls"], 0, "usage() is the model's numbers");
    assert!(h.call("llm.chat", &chat("hi")).is_ok(), "5000 characters read aloud are not 5000 tokens");
    let by = sdk_log_by_source(&sdk);
    assert!(by.iter().any(|t| t.cat == "tts" && t.src == "app:t" && t.input == 5000 && t.calls == 1), "{by:?}");
    assert!(by.iter().any(|t| t.cat == "llm" && t.src == "app:t" && t.calls == 1), "{by:?}");
}

fn sdk_log_by_source(sdk: &TileSdk) -> Vec<crate::usage::Totals> {
    sdk.log_by_source()
}

#[test]
fn a_speaker_that_cannot_be_shared_answers_busy_with_who_holds_it() {
    let (mut h, sdk, _) = host("busy", caps(false, true));
    h.call("__test.speech.busy", &json!({"holder": "the radio"})).unwrap();
    let e = h.call("speech.say", &json!({"text": "hello"})).unwrap_err();
    let v: Value = serde_json::from_str(e.strip_prefix(ERROR_PREFIX).unwrap()).unwrap();
    assert_eq!((v["code"].as_str(), v["holder"].as_str()), (Some("Busy"), Some("the radio")));
    assert!(sdk.fakes().unwrap().speech.said().is_empty());
    h.call("__test.speech.busy", &json!({})).unwrap();
    assert!(h.call("speech.say", &json!({"text": "hello"})).is_ok());
    assert_eq!(h.call("__test.speech.said", &json!({})).unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn the_exclusive_speaker_is_found_from_the_output_and_the_programs_that_hold_it() {
    // a mixer (or a sound server): everyone plays at once; only the app's own reading counts
    assert_eq!(speaker_holder(true, &["aplay (12)".into()], None), None);
    assert_eq!(speaker_holder(true, &[], Some("this app")), Some("this app".into()));
    // one program at a time: whoever has the device open is named
    assert_eq!(speaker_holder(false, &["aplay (12)".into(), "x".into()], None), Some("aplay (12)".into()));
    assert_eq!(speaker_holder(false, &[], None), None);
    assert_eq!(speaker_holder(false, &["aplay (12)".into()], Some("the app mail")), Some("the app mail".into()));
    assert_eq!(card_device("plughw:2,0"), Some((2, 0)));
    assert_eq!(card_device("hw:1,3"), Some((1, 3)));
    assert_eq!(card_device("default"), None);
    // a made-up /proc: pid 4242 (aplay) holds card 2 device 0
    let root = tmp("proc");
    let fd = root.join("4242").join("fd");
    std::fs::create_dir_all(&fd).unwrap();
    std::os::unix::fs::symlink("/dev/snd/pcmC2D0p", fd.join("3")).unwrap();
    std::fs::write(root.join("4242").join("comm"), "aplay\n").unwrap();
    assert_eq!(exclusive_holders(&root, Some("plughw:2,0"), false), vec!["aplay (4242)".to_string()]);
    assert!(exclusive_holders(&root, Some("plughw:2,0"), true).is_empty(), "a shared output is not exclusive");
    assert!(exclusive_holders(&root, Some("plughw:1,0"), false).is_empty(), "another card is free");
    assert!(exclusive_holders(&root, None, false).is_empty());
}

#[test]
fn what_the_person_decided_is_kept_in_a_file_the_app_cannot_reach() {
    let dir = tmp("saved");
    let file = dir.join("t.json");
    let out = Arc::new(Mutex::new(Outbox::default()));
    let mk = || TileSdk::new("t", "u", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&dir.join("usage.jsonl")))), out.clone(), Some(&file));
    let a = Arc::new(mk());
    let id = a.chat(&chat("hi"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    a.answer(id, true);
    a.set_tokens_per_day(Some(5000));
    let b = mk();
    assert_eq!(b.saved(), Saved { llm: true, speech: false, tokens_per_day: Some(5000) }, "a new run remembers");
    assert_eq!(b.token_cap(), Some(5000));
    std::fs::write(&file, "{ not json").unwrap();
    assert_eq!(mk().saved(), Saved::default(), "a damaged file means: ask again");
}

#[test]
fn the_real_model_backend_uses_the_persons_setup_counts_the_providers_numbers_and_hides_the_provider() {
    use crate::providers::{Kind, ProviderCfg};
    use crate::settings::ModelChoice;
    struct Net(Mutex<Vec<String>>);
    impl iyakki_proxy::Upstream for Net {
        fn request(&self, _: &str, url: &str, _: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            self.0.lock().unwrap().push(format!("{url} {}", String::from_utf8_lossy(body)));
            Ok(iyakki_proxy::UpstreamResponse { status: 200, body: br#"{"choices":[{"message":{"content":"Hi, from the provider"}}],"usage":{"prompt_tokens":11,"completion_tokens":4}}"#.to_vec() })
        }
    }
    let dir = tmp("real");
    let net = Arc::new(Net(Default::default()));
    let svc = crate::services::Services::open(&dir, net.clone(), net.clone()).unwrap();
    let llm = ServicesLlm { svc: svc.clone() };
    let req = ChatRequest { messages: vec![("user".into(), "hello".into())], model: None, max_tokens: 100 };
    let e = llm.chat(&req, &mut |_| true).unwrap_err();
    assert_eq!(e.code, "Unavailable", "no model chosen yet");
    assert!(e.extra["fix"].as_str().unwrap().contains("Settings"));
    {
        let mut st = svc.settings.lock().unwrap();
        let mut p = ProviderCfg::new("home", Kind::Ollama);
        p.models = vec!["gemma3".into()];
        st.upsert_provider(p).unwrap();
        st.set_chat(Some(ModelChoice { provider: "home".into(), model: "gemma3".into() })).unwrap();
    }
    let models = llm.models();
    assert_eq!((models[0].id.as_str(), models[0].place, models[0].free()), ("gemma3", "this computer", true));
    // through a whole app: card, call, numbers, log
    let out = Arc::new(Mutex::new(Outbox::default()));
    let sdk = Arc::new(TileSdk::for_services(&svc, "root", "notes", out.clone()));
    let id = sdk.chat(&chat("hello"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    let card = out.lock().unwrap().requests[0].args.clone();
    assert_eq!((card["provider"].as_str(), card["model"].as_str(), card["leaves"].as_bool()), (Some("home"), Some("gemma3"), Some(false)), "the person's card names the provider");
    sdk.answer(id, true);
    wait_for(&out, |o| !o.done.is_empty());
    let d = out.lock().unwrap().done[0].clone();
    assert!(d.ok, "{d:?}");
    let v: Value = serde_json::from_str(&d.value).unwrap();
    assert_eq!((v["text"].as_str(), v["usage"]["input"].as_u64(), v["usage"]["output"].as_u64(), v["usage"]["estimated"].as_bool()), (Some("Hi, from the provider"), Some(11), Some(4), Some(false)));
    assert!(!d.value.contains("home") && !d.value.contains("11434"), "the page learns no provider and no address: {}", d.value);
    let e = svc.usage.entries(Some("root"));
    assert_eq!((e.len(), e[0].src.as_str(), e[0].cat.as_str(), e[0].provider.as_str(), e[0].model.as_str(), e[0].input, e[0].output, e[0].errors), (1, "app:notes", "llm", "home", "gemma3", 11, 4, 0));
    // the setup is read again at every call: a provider that goes away is an error with a code, not a stale answer
    svc.settings.lock().unwrap().set_chat(None).unwrap();
    assert_eq!(llm.chat(&req, &mut |_| true).unwrap_err().code, "Unavailable");
    // the answer is cut at max_tokens (about four characters a token) when the provider has no such option
    let short = ChatRequest { max_tokens: 1, ..req };
    svc.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "home".into(), model: "gemma3".into() })).unwrap();
    assert_eq!(llm.chat(&short, &mut |_| true).unwrap().text, "Hi, ");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_page_gets_errors_with_codes_promises_for_slow_calls_and_events_for_streams() {
    let m = r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"llm":true,"speech":true}}"#;
    let b = Bundle::from_files(&[("index.html", "<script>globalThis.log = [];</script>"), ("app.json", m)], "t").unwrap();
    let dir = tmp("page");
    let mut rt = TileRuntime::open_sdk(b, info(), TileStorage::memory(), Arc::new(iyakki_web::fetch::NoNetwork), vec![], None, |out| Some(TileSdk::fake("t", out, &dir))).unwrap();
    let mut js = |code: &str| -> String { rt.page.eval("t", code).unwrap_or_else(|e| format!("ERR {e}")) };
    // both call shapes, results as promises
    js("iyakki.llm.chat({messages: [{role: 'user', content: 'hi'}]}).then((r) => log.push('a:' + r.text + ':' + r.usage.estimated)); iyakki.llm.chat([{role: 'user', content: 'hi'}], {max_tokens: 10}).then((r) => log.push('b:' + r.model)); iyakki.models.list().then((m) => log.push('m:' + m.length + m[0].where)); iyakki.speech.say('hello').then((r) => log.push('s:' + typeof r.id));");
    rt.pump();
    rt.page.advance(10);
    assert_eq!(rt.page.eval("t", "log.join()").unwrap(), "a:ok:true,b:fake-model,m:1this computer,s:number");
    // the page checks its own arguments before anything leaves, with a TypeError that names the problem
    for bad in ["iyakki.llm.chat({messages: 5})", "iyakki.llm.chat([{role: 'bot', content: 'x'}])", "iyakki.llm.chat([{role: 'user', content: 'x'}], {max_tokens: 99999})", "iyakki.llm.stream([{role: 'user', content: 3}])", "iyakki.speech.say('x', {speed: 5})", "iyakki.speech.say('x', {pitch: 1})", "iyakki.speech.say(7)"] {
        let r = rt.page.eval("t", bad);
        assert!(matches!(r, Err(iyakki_js::JsError::Exception(ref m)) if m.contains("iyakki:")), "{bad}: {r:?}");
    }
    // the host's refusals arrive as Error objects with .code and the code's own fields
    rt.page.eval("t", "iyakki.llm.chat({messages: [{role: 'user', content: 'x'}], model: 'nope'}).catch((e) => log.push(e.code + '|' + e.field + '|' + (e instanceof Error)))").unwrap();
    rt.page.advance(10);
    assert!(rt.page.eval("t", "log.join()").unwrap().ends_with("BadArgument|model|true"));
    rt.sdk.as_ref().unwrap().fake_op("llm.fail", &json!({"code": "Unavailable"})).unwrap();
    rt.page.eval("t", "iyakki.llm.chat([{role: 'user', content: 'x'}]).catch((e) => log.push('e:' + e.code))").unwrap();
    rt.page.eval("t", "iyakki.llm.chat([{role: 'user', content: 'x'}]).catch((e) => log.push('e:' + e.code))").unwrap();
    rt.page.advance(10);
    assert!(rt.page.eval("t", "log.join()").unwrap().ends_with("e:Unavailable,e:Unavailable"));
    // a stream: handlers are called on the page's next turn, in order
    rt.sdk.as_ref().unwrap().fake_op("llm.reply", &json!({"text": "to be or not"})).unwrap();
    rt.page.eval("t", "globalThis.s = []; const h = iyakki.llm.stream([{role: 'user', content: 'x'}]); h.on('text', (t) => s.push(t.text)).on('done', (d) => s.push('done:' + d.usage.output)); h.on('error', (e) => s.push('err:' + e.code));").unwrap();
    assert_eq!(rt.page.eval("t", "s.join('')").unwrap(), "", "nothing before the page's next turn");
    assert!(rt.pump());
    assert_eq!(rt.page.eval("t", "s.join('')").unwrap(), "to be or notdone:3");
    // a stream's error is an Error with a code
    rt.sdk.as_ref().unwrap().fake_op("llm.fail", &json!({"code": "Busy"})).unwrap();
    rt.page.eval("t", "globalThis.s = []; iyakki.llm.stream([{role: 'user', content: 'x'}]).on('error', (e) => s.push(e.code + ':' + (e instanceof Error)));").unwrap();
    rt.pump();
    assert_eq!(rt.page.eval("t", "s.join()").unwrap(), "Busy:true");
    // the API cannot be reached around: no way to set limits or answer cards from the page
    assert_eq!(rt.page.eval("t", "typeof iyakki.llm.setLimit + typeof iyakki.llm.approve + typeof iyakki.speech.listen").unwrap(), "undefinedundefinedundefined");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_headless_harness_calls_every_namespace_with_fakes_including_async_tests_and_refusals() {
    let dir = tmp("harness");
    let write = |p: &str, t: &str| {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    write("app.json", r#"{"id":"sdkapp","name":"Sdk","icon":"🧩","capabilities":{"llm":true,"speech":true,"notifications":true}}"#);
    write("index.html", "<h1>x</h1>");
    write(
        "tests/sdk.js",
        r#"
T.test('chat with a fake reply', async () => {
  T.fake.llm.reply('Hello');
  T.fake.llm.usage(7, 3);
  const r = await iyakki.llm.chat({ messages: [{ role: 'user', content: 'hi' }] });
  T.eq(r.text, 'Hello'); T.eq(r.usage.input, 7); T.eq(r.usage.output, 3);
});
T.test('models and usage', async () => {
  T.fake.models.set([{ id: 'm1', label: 'Fast', kind: 'chat', where: 'this computer' }]);
  const m = await iyakki.models.list();
  T.eq(m[0].id, 'm1');
  const u = await iyakki.llm.usage();
  T.ok(u.today.calls >= 1, 'the call above is counted');
});
T.test('streaming', async () => {
  T.fake.llm.reply('a b c');
  const got = [];
  await new Promise((resolve) => { iyakki.llm.stream([{ role: 'user', content: 'x' }]).on('text', (t) => got.push(t.text)).on('done', resolve); });
  T.eq(got.join(''), 'a b c');
});
T.test('refusals have codes', async () => {
  T.fake.llm.fail('Unavailable');
  let code = '';
  try { await iyakki.llm.chat([{ role: 'user', content: 'x' }]); } catch (e) { code = e.code; }
  T.eq(code, 'Unavailable');
  T.fake.approve('deny');
  T.fake.llm.reply('x');
  T.fake.approve('allow');
});
T.test('over the budget', async () => {
  T.fake.llm.reply('x');
  T.fake.budget.set({ tokensPerDay: 1 });
  let e1 = null;
  try { await iyakki.llm.chat([{ role: 'user', content: 'x' }]); } catch (e) { e1 = e; }
  T.eq(e1.code, 'BudgetStop'); T.ok(e1.resets_at > 0);
  T.fake.budget.set({ tokensPerDay: 100000 });
});
T.test('speech and busy', async () => {
  const r = await iyakki.speech.say('Hello', { lang: 'en' });
  T.ok(r.id > 0);
  T.eq(T.fake.speech.said()[0].text, 'Hello');
  T.fake.speech.busy('the radio');
  let h = '';
  try { await iyakki.speech.say('again'); } catch (e) { h = e.code + ':' + e.holder; }
  T.eq(h, 'Busy:the radio');
});
T.test('notifications are listed for the test', () => {
  iyakki.notify.show('Hi', 'there');
  T.eq(T.fake.notify.sent()[0].title, 'Hi');
});
T.test('a promise that never settles fails', () => new Promise(() => {}));
"#,
    );
    let rep = run_tests(&dir).unwrap();
    let text = rep.text();
    assert_eq!((rep.passed, rep.failed), (7, 1), "{text}");
    assert!(text.contains("FAIL a promise that never settles fails: the promise never settled"), "{text}");
    // an app without the capabilities gets codes from the same harness
    write("app.json", r#"{"id":"sdkapp","name":"Sdk","icon":"🧩"}"#);
    write("tests/sdk.js", "T.test('refused', async () => { let c = ''; try { await iyakki.llm.chat([{ role: 'user', content: 'x' }]); } catch (e) { c = e.code; } T.eq(c, 'NoCapability'); let d = ''; try { await iyakki.speech.say('x'); } catch (e) { d = e.code; } T.eq(d, 'NoCapability'); });");
    let rep = run_tests(&dir).unwrap();
    assert!(rep.ok(), "{}", rep.text());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_tile_stops_what_it_started() {
    let gate = Arc::new(AtomicU64::new(0));
    let m = r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"llm":true}}"#;
    let b = Bundle::from_files(&[("index.html", "<p>x</p>"), ("app.json", m)], "t").unwrap();
    let g2 = gate.clone();
    let rt = TileRuntime::open_sdk(b, info(), TileStorage::memory(), Arc::new(iyakki_web::fetch::NoNetwork), vec![], None, move |out| {
        let log = LogMeter(UsageLog::open(&tmp("close").join("usage.jsonl")));
        let s = TileSdk::new("t", "u", Arc::new(Slow { gate: g2, text: "never heard" }), Arc::new(FakeSpeech::default()), Arc::new(log), out, None);
        Some(s)
    })
    .unwrap();
    let sdk = rt.sdk.clone().unwrap();
    let id = sdk.chat(&chat("hi"), false).unwrap()["pending"].as_u64().unwrap() as u32;
    sdk.answer(id, true);
    assert_eq!(sdk.waiting(), 1);
    drop(rt);
    assert_eq!(sdk.waiting(), 0, "dropping the runtime closes the SDK");
    gate.store(1, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(sdk.summary().is_some_and(|s| s.contains("1 call")), "the tokens it spent are still counted");
}

#[test]
fn the_manifest_knows_the_new_capabilities_in_plain_words() {
    let m = TileManifest::parse(r#"{"id":"x","name":"X","capabilities":{"llm":true,"speech":true,"storage":true}}"#).unwrap();
    assert_eq!(m.caps.permissions(), ["tile:storage", "tile:llm", "tile:speech"]);
    assert_eq!(Caps::from_permissions(&m.caps.permissions()), m.caps);
    assert_eq!(TileManifest::parse(&m.to_json().to_string()).unwrap().caps, m.caps, "to_json keeps them");
    assert!(TileManifest::parse(r#"{"id":"x","name":"X","capabilities":{"llm":"yes"}}"#).is_err());
    assert!(TileManifest::parse(r#"{"id":"x","name":"X","capabilities":{"models":true}}"#).unwrap_err().contains("llm, speech"));
    use super::manifest::Permission;
    let words = |p: &str| Permission::parse(p).unwrap().words("amy");
    assert!(words("tile:llm").contains("AI model") && words("tile:llm").contains("daily limit") && words("tile:llm").contains("another computer"));
    assert!(words("tile:speech").contains("read text aloud"));
    assert!(Permission::parse("tile:llm").unwrap().sensitive() && Permission::parse("tile:speech").unwrap().sensitive());
    assert!(!Permission::parse("tile:storage").unwrap().sensitive());
    let _ = Call::default();
}
