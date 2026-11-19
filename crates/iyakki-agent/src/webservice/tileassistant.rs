//! The `assistant` domain of a browser tile (F-SDK-84): what the UI SDK's `AssistantPort` asks of a tile host, mapped onto the core's embedded
//! assistant facet (`iyakki_core_api::Assistant`) for the signed-in person and the tile's own id. The core owns the model, the history and the
//! limits; the tile hands it its own tools and context and runs those tools itself when the core says so (`ToolCall`, then `answer`).
//!
//! A tile reaches only the sessions it opened itself (the host keeps the list), whatever session names it makes up. A yes to a tool's card
//! (`decide` with `approve` true) is the person's: the web service asks them on a card in their page first (`TileBackend::assistant_call`),
//! the same rule as a page's download in the `browser` domain.
//!
//! Calls (arguments are a JSON object): `models`, `roles`, `open {conversation, title, tools, context, model, role}` gives `{id, resumed}`,
//! `set_context {session, context}`, `choose {session, model, role}`, `send {session, text}`, `poll {session}` gives the events
//! (`AssistEvent`, tagged by `type`), `decide {session, call, approve}`, `answer {session, call, ok, text}`, `cancel {session}`,
//! `history {session}` gives `[{role, text}]`, `close {session}`.

use iyakki_core_api::assistant::{AppToolSpec, AssistEvent, Assistant, ContextItem, HistoryRole, SessionSpec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq)]
pub enum Req {
    Models,
    Roles,
    Open(SessionSpec),
    SetContext(String, Vec<ContextItem>),
    Choose(String, Option<String>, Option<String>),
    Send(String, String),
    Poll(String),
    /// `approve` true is the person's yes: the web service asks them first.
    Decide(String, String, bool),
    Answer(String, String, bool, String),
    Cancel(String),
    History(String),
    Close(String),
}

fn text<'a>(v: &'a Value, k: &str, max: usize) -> Result<&'a str, String> {
    match v.get(k).and_then(Value::as_str) {
        Some(s) if s.len() <= max && !s.contains('\0') => Ok(s),
        _ => Err(format!("\"{k}\" is text of at most {max} bytes")),
    }
}

fn session(v: &Value) -> Result<String, String> {
    Ok(text(v, "session", 64)?.to_string())
}

fn opt_text(v: &Value, k: &str) -> Result<Option<String>, String> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => text(v, k, 64).map(|s| Some(s.to_string())),
    }
}

fn context(v: &Value) -> Result<Vec<ContextItem>, String> {
    match v.get("context") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(c) => serde_json::from_value(c.clone()).map_err(|_| "\"context\" is a list of {kind, name, text}".to_string()),
    }
}

pub fn plan(method: &str, args_json: &str) -> Result<Req, String> {
    let v: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !v.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    Ok(match method {
        "models" => Req::Models,
        "roles" => Req::Roles,
        "open" => {
            let tools: Vec<AppToolSpec> = match v.get("tools") {
                None | Some(Value::Null) => Vec::new(),
                Some(t) => serde_json::from_value(t.clone()).map_err(|_| "\"tools\" is a list of {name, description, schema, approval}".to_string())?,
            };
            Req::Open(SessionSpec { conversation: text(&v, "conversation", 200)?.to_string(), title: text(&v, "title", 200).unwrap_or("").to_string(), tools, context: context(&v)?, model: opt_text(&v, "model")?, role: opt_text(&v, "role")? })
        }
        "set_context" => Req::SetContext(session(&v)?, context(&v)?),
        "choose" => Req::Choose(session(&v)?, opt_text(&v, "model")?, opt_text(&v, "role")?),
        "send" => Req::Send(session(&v)?, text(&v, "text", iyakki_core_api::assistant::MAX_MESSAGE)?.to_string()),
        "poll" => Req::Poll(session(&v)?),
        "decide" => Req::Decide(session(&v)?, text(&v, "call", 64)?.to_string(), v.get("approve").and_then(Value::as_bool).ok_or("\"approve\" is true or false")?),
        "answer" => Req::Answer(session(&v)?, text(&v, "call", 64)?.to_string(), v.get("ok").and_then(Value::as_bool).ok_or("\"ok\" is true or false")?, text(&v, "text", 64 * 1024)?.to_string()),
        "cancel" => Req::Cancel(session(&v)?),
        "history" => Req::History(session(&v)?),
        "close" => Req::Close(session(&v)?),
        _ => return Err("that is not a call this tile may make".into()),
    })
}

/// What the host remembers for one tile: the sessions it opened, and the cards it was shown (call id to the words of the card).
#[derive(Default)]
pub struct Own {
    pub sessions: BTreeSet<String>,
    pub cards: BTreeMap<String, String>,
}

impl Req {
    /// The session this request names, if any.
    pub fn session(&self) -> Option<&str> {
        match self {
            Req::SetContext(s, _) | Req::Choose(s, _, _) | Req::Send(s, _) | Req::Poll(s) | Req::Decide(s, _, _) | Req::Answer(s, _, _, _) | Req::Cancel(s) | Req::History(s) | Req::Close(s) => Some(s),
            _ => None,
        }
    }
}

/// The words of the person's card for a yes (the model's and the app's text are data: cut and cleaned).
pub fn card_words(tile: &str, summary: &str) -> String {
    let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    format!("Let the assistant of {} do this: {}", clean(tile, 40), clean(summary, 160))
}

fn choices(c: Vec<iyakki_core_api::assistant::Choice>) -> String {
    Value::Array(c.into_iter().map(|c| json!({"id": c.id, "label": c.label})).collect()).to_string()
}

/// Run a planned request. A session the tile did not open is "no such assistant session", the core's own sentence. `Decide` with `approve`
/// true must have been approved by the person before this is called; this function does not ask.
pub fn run(a: &dyn Assistant, user: &str, app: &str, own: &mut Own, req: &Req) -> Result<String, String> {
    if let Some(s) = req.session() {
        if !own.sessions.contains(s) {
            return Err("no such assistant session".into());
        }
    }
    let ok = || json!({"ok": true}).to_string();
    Ok(match req {
        Req::Models => choices(a.models(user)),
        Req::Roles => choices(a.roles(user)),
        Req::Open(spec) => {
            let info = a.open(user, app, spec)?;
            own.sessions.insert(info.id.clone());
            json!({"id": info.id, "resumed": info.resumed}).to_string()
        }
        Req::SetContext(s, c) => {
            a.set_context(user, s, c)?;
            ok()
        }
        Req::Choose(s, m, r) => {
            a.choose(user, s, m.as_deref(), r.as_deref())?;
            ok()
        }
        Req::Send(s, t) => {
            a.send(user, s, t)?;
            ok()
        }
        Req::Poll(s) => {
            let events = a.poll(user, s);
            for e in &events {
                if let AssistEvent::Approval { call, tool, summary, .. } = e {
                    if own.cards.len() < 64 {
                        own.cards.insert(call.clone(), card_words(app, &format!("{tool}: {summary}")));
                    }
                }
            }
            serde_json::to_string(&events).map_err(|_| "the events were not readable".to_string())?
        }
        Req::Decide(s, call, approve) => {
            a.decide(user, s, call, *approve)?;
            own.cards.remove(call);
            ok()
        }
        Req::Answer(s, call, good, t) => {
            a.answer(user, s, call, *good, t)?;
            ok()
        }
        Req::Cancel(s) => {
            a.cancel(user, s)?;
            ok()
        }
        Req::History(s) => Value::Array(
            a.history(user, s).into_iter().map(|h| json!({"role": match h.role { HistoryRole::User => "user", HistoryRole::Assistant => "assistant", HistoryRole::Tool => "tool"}, "text": h.text})).collect(),
        )
        .to_string(),
        Req::Close(s) => {
            a.close(user, s);
            own.sessions.remove(s);
            ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::NoAssistant;

    #[test]
    fn open_reads_tools_and_context_and_refuses_the_wrong_shapes() {
        let Req::Open(s) = plan("open", r#"{"conversation":"main","title":"Notes","tools":[{"name":"add_note","description":"adds","schema":"{}","approval":"ask"}],"context":[{"kind":"document","name":"doc","text":"x"}],"role":"concise"}"#).unwrap() else { panic!() };
        assert_eq!((s.conversation.as_str(), s.tools.len(), s.context.len(), s.role.as_deref()), ("main", 1, 1, Some("concise")));
        for bad in ["{}", r#"{"conversation":5}"#, r#"{"conversation":"a","tools":3}"#, r#"{"conversation":"a","tools":[{"name":"x"}]}"#, r#"{"conversation":"a","context":[1]}"#, "[1]", "nope", r#"{"conversation":"a","model":9}"#] {
            assert!(plan("open", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_other_calls_are_checked_and_unknown_ones_are_not_calls() {
        assert_eq!(plan("send", r#"{"session":"as-1","text":"hi"}"#).unwrap(), Req::Send("as-1".into(), "hi".into()));
        assert_eq!(plan("decide", r#"{"session":"s","call":"c","approve":false}"#).unwrap(), Req::Decide("s".into(), "c".into(), false));
        assert_eq!(plan("answer", r#"{"session":"s","call":"c","ok":true,"text":"done"}"#).unwrap(), Req::Answer("s".into(), "c".into(), true, "done".into()));
        assert_eq!(plan("poll", r#"{"session":"s"}"#).unwrap(), Req::Poll("s".into()));
        for (m, a) in [("send", r#"{"session":"s"}"#), ("decide", r#"{"session":"s","call":"c"}"#), ("answer", r#"{"session":"s","call":"c","text":""}"#), ("poll", "{}"), ("history", r#"{"session":7}"#), ("shell", "{}"), ("set_enabled", "{}"), ("send", &format!(r#"{{"session":"s","text":"{}"}}"#, "x".repeat(9000)))] {
            assert!(plan(m, a).is_err(), "{m} {a}");
        }
    }

    #[test]
    fn a_core_with_no_assistant_answers_with_its_sentence_and_a_made_up_session_is_not_the_tiles() {
        let mut own = Own::default();
        let e = run(&NoAssistant, "u", "tile", &mut own, &plan("open", r#"{"conversation":"main"}"#).unwrap()).unwrap_err();
        assert!(e.contains("no embedded assistant"), "{e}");
        let e = run(&NoAssistant, "u", "tile", &mut own, &Req::Poll("as-1".into())).unwrap_err();
        assert_eq!(e, "no such assistant session");
        assert_eq!(run(&NoAssistant, "u", "tile", &mut own, &Req::Models).unwrap(), "[]");
    }

    #[test]
    fn the_card_words_are_cleaned_and_short() {
        let w = card_words("tile\u{7}", &format!("add_note: {}", "x".repeat(400)));
        assert!(w.starts_with("Let the assistant of tile do this: add_note: xxx") && w.len() < 260 && !w.contains('\u{7}'), "{w}");
    }

    #[test]
    fn a_tile_runs_a_whole_turn_through_the_core_for_its_own_sessions_only() {
        use iyakki_core_fake::{FakeAssistant, FakeTurn};
        let core = FakeAssistant::new();
        core.script(vec![FakeTurn::call("Adding.", "add_note", r#"{"text":"eggs"}"#), FakeTurn::say("Done.")]);
        let mut own = Own::default();
        let call = |own: &mut Own, who: &str, m: &str, a: &str| plan(m, a).and_then(|r| run(&core, who, "tile-a", own, &r));
        let open = r#"{"conversation":"main","title":"Notes","tools":[{"name":"add_note","description":"adds a note","schema":"{\"type\":\"object\"}","approval":"ask"}]}"#;
        let id: Value = serde_json::from_str(&call(&mut own, "alice", "open", open).unwrap()).unwrap();
        let sid = id["id"].as_str().unwrap().to_string();
        assert_eq!(id["resumed"], false);
        call(&mut own, "alice", "send", &format!(r#"{{"session":"{sid}","text":"add eggs"}}"#)).unwrap();
        let ev: Vec<Value> = serde_json::from_str(&call(&mut own, "alice", "poll", &format!(r#"{{"session":"{sid}"}}"#)).unwrap()).unwrap();
        let card = ev.iter().find(|e| e["type"] == "approval").expect("the core asks first");
        assert!(own.cards.contains_key(card["call"].as_str().unwrap()), "the host remembers the words of the card");
        let c = card["call"].as_str().unwrap();
        call(&mut own, "alice", "decide", &format!(r#"{{"session":"{sid}","call":"{c}","approve":true}}"#)).unwrap();
        assert!(own.cards.is_empty());
        let ev: Vec<Value> = serde_json::from_str(&call(&mut own, "alice", "poll", &format!(r#"{{"session":"{sid}"}}"#)).unwrap()).unwrap();
        assert!(ev.iter().any(|e| e["type"] == "tool_call" && e["tool"] == "add_note"), "{ev:?}");
        call(&mut own, "alice", "answer", &format!(r#"{{"session":"{sid}","call":"{c}","ok":true,"text":"added"}}"#)).unwrap();
        let ev: Vec<Value> = serde_json::from_str(&call(&mut own, "alice", "poll", &format!(r#"{{"session":"{sid}"}}"#)).unwrap()).unwrap();
        assert!(ev.iter().any(|e| e["type"] == "final" && e["text"] == "Done."), "{ev:?}");
        let h: Vec<Value> = serde_json::from_str(&call(&mut own, "alice", "history", &format!(r#"{{"session":"{sid}"}}"#)).unwrap()).unwrap();
        assert!(h.iter().any(|x| x["role"] == "user"));
        // another person, or another tile (its own list), does not reach the session
        assert_eq!(call(&mut own, "bob", "poll", &format!(r#"{{"session":"{sid}"}}"#)).unwrap(), "[]", "another person is told nothing");
        assert!(call(&mut own, "bob", "send", &format!(r#"{{"session":"{sid}","text":"x"}}"#)).is_err());
        let mut other = Own::default();
        assert_eq!(call(&mut other, "alice", "poll", &format!(r#"{{"session":"{sid}"}}"#)).unwrap_err(), "no such assistant session");
        call(&mut own, "alice", "close", &format!(r#"{{"session":"{sid}"}}"#)).unwrap();
        assert!(call(&mut own, "alice", "send", &format!(r#"{{"session":"{sid}","text":"x"}}"#)).is_err(), "closed sessions leave the tile's list");
    }
}
