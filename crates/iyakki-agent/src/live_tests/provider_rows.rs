//! F-CHAT-24 (provider failures in plain words, with the real server) and F-AI-02 (the Lemonade / OpenAI-compatible part).

use super::*;
use crate::agent::RunLimits;

fn wrong_key_file(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("iyakki-live-wrongkey-{}-{tag}", std::process::id()));
    std::fs::write(&p, "sk-this-is-not-the-key-0123456789").unwrap();
    p
}

/// A run of the loop whose model is `client`/`model`: how the failure reads to the person.
fn failure_text(client: ProviderClient, model: &str) -> Result<String, String> {
    let b = Arc::new(ProviderChat { client, model: model.to_string(), streamer: None });
    let reg = Reg::new(vec![], Box::new(|_, _| Ok(String::new())));
    let h = start_with(b, reg, vec![Message::system("be brief"), Message::user("say hello")], RunLimits::default(), Assist::off());
    let ev = drive(&h, 60, |_| Some(Verdict::Approve));
    match ev.last() {
        Some(RunEvent::Failed { error }) => Ok(error.clone()),
        other => Err(format!("expected a failed run, got {other:?}")),
    }
}

#[test]
#[ignore = "live model"]
fn f_chat_24_failures_with_the_real_server_read_in_plain_words() {
    attempts("F-CHAT-24", "wrong key, wrong model name and a wrong port each end a run with a plain sentence, not raw HTTP", |_, note| {
        // wrong key
        let kf = wrong_key_file("a");
        let c = client_with(None, Some(Some(&format!("file:{}", kf.display()))));
        let key_msg = failure_text(c, &model_name())?;
        note.say(format!("wrong key -> {}", head(&key_msg, 300)));
        let _ = std::fs::remove_file(&kf);
        // wrong model name
        let model_msg = failure_text(client_with(None, None), "no-such-model-qwerty")?;
        note.say(format!("wrong model -> {}", head(&model_msg, 300)));
        // server trouble simulated with a port nothing listens on
        let url = need("IYAKKI_LIVE_LEMONADE_URL");
        let host = iyakki_sec::url_host(&url).ok_or("no host")?;
        let dead = format!("http://{host}:9");
        let port_msg = failure_text(client_with(Some(&dead), None), &model_name())?;
        note.say(format!("wrong port -> {}", head(&port_msg, 300)));
        let raw = |m: &str| m.starts_with("HTTP ") || m.contains("Error {") || m.contains("os error");
        // The key check depends on the server enforcing keys; say what it did.
        if key_msg.contains("refused the key") && key_msg.contains("Settings") {
            note.say("the server enforces keys: the refusal points at Settings");
        } else {
            return Err(format!("a wrong key was not met with the key sentence: {key_msg}"));
        }
        if raw(&model_msg) {
            return Err(format!("the wrong model name reads as raw HTTP: {model_msg}"));
        }
        if !(model_msg.to_lowercase().contains("model") && (model_msg.contains("Settings") || model_msg.to_lowercase().contains("pull"))) {
            return Err(format!("the wrong model name does not say what to do: {model_msg}"));
        }
        if !(port_msg.starts_with("Could not reach") || port_msg.contains("took too long")) {
            return Err(format!("the wrong port does not read as a reach/time-out sentence: {port_msg}"));
        }
        for m in [&key_msg, &model_msg, &port_msg] {
            if m.contains("sk-this-is-not") {
                return Err("the wrong key leaked into the message".into());
            }
        }
        Ok(())
    });
}

#[test]
#[ignore = "live model"]
fn f_ai_02_the_lemonade_provider_lists_chats_calls_a_tool_and_streams() {
    attempts("F-AI-02", "Lemonade / OpenAI-compatible only: list models, a plain chat, a tool call, the tool result read back, the health line. OpenAI, Claude and Gemini were NOT tried.", |rec, note| {
        let c = client_with(None, None);
        let rep = c.test()?;
        note.say(format!("list_models: {} model(s) in {} ms", rep.models.len(), rep.latency_ms));
        let model = model_name();
        if !rep.models.iter().any(|m| *m == model) {
            return Err(format!("the model {model} is not in the list"));
        }
        match c.lemonade_health() {
            Ok((_, loaded)) => note.say(format!("health answered; {} model(s) loaded", loaded.len())),
            Err(e) => note.say(format!("health call failed: {}", head(&e, 100))),
        }
        // plain chat
        let t = rec.complete(&[Message::system("Answer with one word."), Message::user("What colour is the sky on a clear day?")])?;
        note.say(format!("plain answer: {}", head(&t, 80)));
        if t.trim().is_empty() {
            return Err("empty plain answer".into());
        }
        // a tool call, then the result read back
        let spec = vec![ToolSpec { name: "get_temperature".into(), description: "Current temperature of a city in Celsius.".into(), schema: json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}) }];
        let hist = vec![Message::system("Use the tools you have to answer."), Message::user("What is the temperature in Chennai right now?")];
        let turn = rec.complete_tools(&hist, &spec)?;
        let call = turn.calls.first().ok_or_else(|| format!("no tool call; the model said: {}", head(&turn.text, 120)))?;
        if call.name != "get_temperature" || call.args.get("city").and_then(Value::as_str).is_none_or(|c| !c.to_lowercase().contains("chennai")) {
            return Err(format!("wrong call: {} {}", call.name, call.args));
        }
        note.say(format!("tool call: {} {}", call.name, call.args));
        let mut h2 = hist.clone();
        h2.push(Message::assistant_calls(turn.text.clone(), turn.calls.clone()));
        h2.push(Message::tool_result(call, "27"));
        let fin = rec.complete_tools(&h2, &spec)?;
        note.say(format!("answer after the tool result: {}", head(&fin.text, 120)));
        if !fin.text.contains("27") {
            return Err(format!("the answer does not use the tool result: {}", head(&fin.text, 200)));
        }
        Ok(())
    });
}
