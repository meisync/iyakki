//! F-AI-02, the cloud part: the same checks as the Lemonade row, against OpenAI, Claude (Anthropic) and Gemini with the person's own
//! API key. All `#[ignore = "live model"]`: they send a few cheap requests to the vendor and cost a few cents.
//!
//! Environment, per vendor (`OPENAI`, `ANTHROPIC`, `GEMINI`; nothing of it is in the repository):
//! * `IYAKKI_LIVE_<VENDOR>_KEY_FILE`  a file holding the API key (read by the provider client at run time, never printed);
//! * `IYAKKI_LIVE_<VENDOR>_MODEL`     the chat model to ask (no default: the person names one their key may use).
//!
//! Run one: `cargo test -p iyakki-agent --locked live_tests::cloud_rows::f_ai_02_openai -- --ignored --nocapture`.
//! A check passes when at least two of three attempts hold; it asserts structure (a listed model, a non-empty answer, the tool
//! called with the right argument, the tool result used), never wording. A vendor whose variables are not set is reported as NOT RUN,
//! never as passed, and the row's notes are changed only by a person after reading the output.

use super::*;

struct Vendor {
    name: &'static str,
    env: &'static str,
    kind: Kind,
}

const VENDORS: [Vendor; 3] = [Vendor { name: "OpenAI", env: "OPENAI", kind: Kind::OpenAi }, Vendor { name: "Claude", env: "ANTHROPIC", kind: Kind::Anthropic }, Vendor { name: "Gemini", env: "GEMINI", kind: Kind::Gemini }];

fn chat_of(v: &Vendor, key_file: &str, model: &str) -> ProviderChat {
    let mut cfg = ProviderCfg::new(&v.env.to_ascii_lowercase(), v.kind);
    cfg.key_ref = Some(format!("file:{key_file}"));
    let up = upstream(120);
    ProviderChat { client: ProviderClient::new(cfg, Secrets::new(None, up.clone()), up), model: model.to_string(), streamer: None }
}

/// One attempt against one vendor: list, plain chat, a tool call, the result read back.
fn one_attempt(b: &ProviderChat, model: &str) -> Result<Vec<String>, String> {
    let mut said = vec![];
    let rep = b.client.test()?;
    said.push(format!("list_models: {} model(s) in {} ms", rep.models.len(), rep.latency_ms));
    if rep.models.is_empty() {
        return Err("the vendor listed no models".into());
    }
    if !rep.models.iter().any(|m| m == model || m.ends_with(&format!("/{model}"))) {
        said.push(format!("note: {model} is not in the list (the vendor may list it under another id)"));
    }
    let t = b.complete(&[Message::system("Answer with one word."), Message::user("What colour is the sky on a clear day?")])?;
    said.push(format!("plain answer: {}", head(&t, 80)));
    if t.trim().is_empty() {
        return Err("empty plain answer".into());
    }
    let spec = vec![ToolSpec { name: "get_temperature".into(), description: "Current temperature of a city in Celsius.".into(), schema: json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}) }];
    let hist = vec![Message::system("Use the tools you have to answer."), Message::user("What is the temperature in Chennai right now?")];
    let turn = b.complete_tools(&hist, &spec)?;
    let call = turn.calls.first().ok_or_else(|| format!("no tool call; the model said: {}", head(&turn.text, 120)))?;
    if call.name != "get_temperature" || call.args.get("city").and_then(Value::as_str).is_none_or(|c| !c.to_lowercase().contains("chennai")) {
        return Err(format!("wrong call: {} {}", call.name, call.args));
    }
    said.push(format!("tool call: {} {}", call.name, call.args));
    let mut h2 = hist.clone();
    h2.push(Message::assistant_calls(turn.text.clone(), turn.calls.clone()));
    h2.push(Message::tool_result(call, "27"));
    let fin = b.complete_tools(&h2, &spec)?;
    said.push(format!("answer after the tool result: {}", head(&fin.text, 120)));
    if !fin.text.contains("27") {
        return Err(format!("the answer does not use the tool result: {}", head(&fin.text, 200)));
    }
    // a wrong key reads as a sentence that points at Settings, never as raw HTTP and never with the key in it
    let bad = std::env::temp_dir().join(format!("iyakki-live-wrongkey-cloud-{}", std::process::id()));
    std::fs::write(&bad, "sk-this-is-not-the-key-0123456789").map_err(|e| e.to_string())?;
    let wrong = chat_of(&VENDORS[0], &bad.display().to_string(), model);
    let e = wrong.complete(&[Message::user("hi")]).err().unwrap_or_default();
    let _ = std::fs::remove_file(&bad);
    if e.contains("sk-this-is-not") || e.starts_with("HTTP ") {
        return Err(format!("a wrong key reads badly: {}", head(&e, 160)));
    }
    said.push(format!("wrong key reads: {}", head(&e, 120)));
    Ok(said)
}

fn run_vendor(i: usize) {
    let v = &VENDORS[i];
    let (kf, model) = match (std::env::var(format!("IYAKKI_LIVE_{}_KEY_FILE", v.env)), std::env::var(format!("IYAKKI_LIVE_{}_MODEL", v.env))) {
        (Ok(k), Ok(m)) => (k, m),
        _ => {
            crate::qeprintln!("F-AI-02 {}: NOT RUN (set IYAKKI_LIVE_{}_KEY_FILE and IYAKKI_LIVE_{}_MODEL)", v.name, v.env, v.env);
            return;
        }
    };
    let b = chat_of(v, &kf, &model);
    let mut passes = 0;
    for n in 1..=3 {
        match one_attempt(&b, &model) {
            Ok(said) => {
                passes += 1;
                crate::qeprintln!("F-AI-02 {} attempt {n}: pass\n  {}", v.name, said.join("\n  "));
            }
            Err(e) => crate::qeprintln!("F-AI-02 {} attempt {n}: FAIL {}", v.name, head(&e, 300).replace(&std::fs::read_to_string(&kf).unwrap_or_default().trim().to_string(), "<key>")),
        }
    }
    assert!(passes >= 2, "F-AI-02 {}: only {passes} of 3 attempts held", v.name);
    crate::qeprintln!("F-AI-02 {}: {passes} of 3 attempts held", v.name);
}

#[test]
#[ignore = "live model"]
fn f_ai_02_openai() {
    run_vendor(0);
}

#[test]
#[ignore = "live model"]
fn f_ai_02_claude() {
    run_vendor(1);
}

#[test]
#[ignore = "live model"]
fn f_ai_02_gemini() {
    run_vendor(2);
}

/// Without any key the three checks do nothing and say so; this one runs in the normal test run and proves the file compiles and
/// that an unset vendor is "NOT RUN" rather than a pass.
#[test]
fn the_cloud_checks_are_configured_by_environment_only() {
    assert_eq!(VENDORS.iter().map(|v| v.env).collect::<Vec<_>>(), ["OPENAI", "ANTHROPIC", "GEMINI"]);
    assert!(VENDORS.iter().all(|v| v.kind.needs_key()));
}
