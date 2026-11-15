//! The assistant rows (F-ASSIST-*): the run loop, the guards, the board and Autopilot, with a real model.

use super::*;

fn web_tools() -> Arc<Reg> {
    let defs = vec![
        tdef("now", "The current date and time.", json!({}), &[], Risk::Safe),
        tdef("weather", "Current weather of a place.", json!({"place": {"type": "string"}}), &[], Risk::Safe),
        tdef("web_search", "Search the web and return result titles and snippets.", json!({"query": {"type": "string"}}), &["query"], Risk::Safe),
    ];
    Reg::new(defs, Box::new(|n, a| match n {
        "weather" => Ok(format!("{}: 31 C, clear sky, humidity 70%", a.get("place").and_then(Value::as_str).unwrap_or("Chennai"))),
        "now" => Ok("2026-11-12 10:15 IST".into()),
        _ => Ok("1. Chennai weather today - 31 C and clear".into()),
    }))
}

#[test]
#[ignore = "live model"]
fn f_assist_10_a_lookup_uses_the_tool_without_refusing_or_asking_permission() {
    attempts("F-ASSIST-10", "with the reminder and the guards on, a real model answers a read-only lookup through a tool, never refuses or asks permission, and ends with an answer", |rec, note| {
        let t = web_tools();
        let ev = run_chat(rec, t.clone(), crate::toolblock::COMPACT_PROMPT, "What is the weather in Chennai right now?", false, 120, |_| Some(Verdict::Approve));
        let text = final_of(&ev)?;
        note.say(format!("tools run: {:?}; notes: {:?}; answer: {}", t.names(), notes_of(&ev), head(&text, 160)));
        if ev.iter().any(|e| matches!(e, RunEvent::Approval { .. })) {
            return Err("a read-only lookup asked for approval".into());
        }
        if !(t.count("weather") + t.count("web_search") >= 1) {
            return Err("no lookup tool was called".into());
        }
        if !text.contains("31") {
            return Err(format!("the answer does not carry the tool's result: {}", head(&text, 200)));
        }
        let l = text.to_lowercase();
        if l.contains("i cannot access") || l.contains("i can't access") || l.contains("would you like me to") {
            return Err(format!("a refusal or a permission question reached the person: {}", head(&text, 200)));
        }
        Ok(())
    });
}
