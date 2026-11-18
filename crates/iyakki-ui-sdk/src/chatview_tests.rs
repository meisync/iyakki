use crate::assistant::*;
use crate::chatview::*;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::size::Metrics;

fn notes_tool() -> ToolSpec {
    ToolSpec::new("add_note", "Add a note to the list", "{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}},\"required\":[\"text\"]}", true)
}

fn peek_tool() -> ToolSpec {
    ToolSpec::new("count_notes", "How many notes there are", "{\"type\":\"object\"}", false)
}

fn cfg() -> SessionConfig {
    SessionConfig { conversation: "main".into(), title: "Notes".into(), tools: vec![notes_tool(), peek_tool()], ..Default::default() }
}

fn view(script: Vec<Turn>) -> (ChatView, std::rc::Rc<std::cell::RefCell<ScriptedAssistant>>) {
    let (port, handle) = SharedAssistant::new(script);
    (ChatView::new(Box::new(port), cfg(), 500), handle)
}

fn draw(v: &mut ChatView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

#[test]
fn a_plain_answer_streams_into_the_transcript() {
    let (mut v, _) = view(vec![Turn::say("Hello there")]);
    assert!(v.send("hi"));
    assert_eq!(v.transcript(), vec!["You: hi".to_string(), "Assistant: Hello there".to_string()]);
    assert!(!v.busy(), "the turn ended with the final answer");
    assert!(!v.send("   "), "an empty message is not sent");
}

#[test]
fn an_app_tool_that_asks_waits_for_the_person_then_runs_in_the_app_and_the_model_sees_the_result() {
    let (mut v, h) = view(vec![Turn::call("I will add it.", "add_note", "{\"text\":\"milk\"}"), Turn::say("Added milk.")]);
    v.send("add milk to my notes");
    assert!(v.busy());
    let a = v.pending_approval().expect("a card is waiting").clone();
    assert_eq!(a.tool, "add_note");
    assert!(v.pump().is_empty(), "nothing runs before the person decides");
    // the card is on screen with two named buttons
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    let allow = action_named(&l, "Allow");
    action_named(&l, "Deny");
    assert!(v.action(allow));
    let calls = v.pump();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].tool.as_str(), calls[0].args.as_str()), ("add_note", "{\"text\":\"milk\"}"));
    v.answer(&calls[0], Ok("Added \"milk\"".into()));
    assert!(!v.busy());
    let t = v.transcript().join("\n");
    assert!(t.contains("Tool add_note done: Added \"milk\"") && t.ends_with("Assistant: Added milk."), "{t}");
    assert_eq!(h.borrow().audit, vec!["assistant.open main", "assistant.tool add_note approved", "assistant.tool add_note ran ok"]);
}

#[test]
fn a_denied_tool_never_reaches_the_app() {
    let (mut v, h) = view(vec![Turn::call("", "add_note", "{\"text\":\"x\"}"), Turn::say("Understood, I did not add it.")]);
    v.send("add x");
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Deny"));
    assert!(v.pump().is_empty());
    assert!(v.transcript().join("\n").contains("Tool add_note failed: the person did not allow it"));
    assert!(h.borrow().audit.iter().any(|l| l == "assistant.tool add_note denied"));
    assert!(!v.busy());
}

#[test]
fn a_safe_tool_runs_without_a_card_and_y_and_n_decide_only_while_a_card_shows() {
    let (mut v, _) = view(vec![Turn::call("", "count_notes", "{}"), Turn::say("You have 2 notes.")]);
    v.send("how many notes?");
    assert!(v.pending_approval().is_none(), "a read-only tool asks nothing");
    let calls = v.pump();
    assert_eq!(calls.len(), 1, "the call is ready for the app at once");
    v.answer(&calls[0], Ok("2 notes".into()));
    assert!(v.transcript().last().unwrap().ends_with("You have 2 notes."));
    let (mut v2, _) = view(vec![Turn::call("", "add_note", "{\"text\":\"a\"}"), Turn::say("ok")]);
    assert!(!v2.key(UiKey::Char('y')), "no card: y is not the view's");
    v2.send("add a");
    assert!(v2.key(UiKey::Char('n')), "a card is showing: n denies");
    assert!(v2.pending_approval().is_none());
}

#[test]
fn tick_hands_the_app_its_tool_calls_and_does_nothing_before_the_session_is_open() {
    let (mut v, _) = view(vec![Turn::call("", "count_notes", "{}"), Turn::say("Two.")]);
    assert!(v.tick().is_empty(), "nothing is open yet");
    v.send("count");
    let calls = v.tick();
    assert_eq!(calls.len(), 1);
    assert!(v.tick().is_empty(), "a call is handed out once");
}

#[test]
fn the_context_the_app_attaches_is_what_the_model_is_shown_and_it_can_change() {
    let (mut v, h) = view(vec![Turn::say("a"), Turn::say("b")]);
    v.set_context(vec![ContextItem::new(ContextKind::Document, "Shopping", "milk, eggs")]);
    v.send("what is on my list?");
    v.set_context(vec![ContextItem::new(ContextKind::Selection, "Selected note", "eggs")]);
    v.send("and this?");
    let seen = h.borrow().seen.clone();
    assert!(seen[0].contains("Document Shopping: milk, eggs"), "{seen:?}");
    assert!(seen[1].contains("Selection Selected note: eggs") && !seen[1].contains("milk"), "{seen:?}");
}

#[test]
fn model_and_role_are_chosen_through_the_port_and_a_refusal_is_a_sentence_in_the_view() {
    let (mut v, h) = view(vec![Turn::say("a")]);
    v.send("hi");
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Model: default"));
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Role: default"));
    v.send("again");
    assert!(h.borrow().seen[1].starts_with("model=scripted role=assistant"), "{:?}", h.borrow().seen);
    let (mut bad, hb) = view(vec![]);
    hb.borrow_mut().refuse = Some("The assistant needs a model: open Settings, Models.".into());
    assert!(!bad.send("hi"));
    assert_eq!(bad.error(), Some("The assistant needs a model: open Settings, Models."));
    let l = draw(&mut bad, 800.0, 480.0, false, 1.0);
    assert!(l.texts().iter().any(|t| t.contains("needs a model")));
}

#[test]
fn stop_cancels_a_waiting_turn_and_the_same_conversation_comes_back_with_its_history() {
    let (mut v, h) = view(vec![Turn::call("", "add_note", "{\"text\":\"x\"}")]);
    v.send("add x");
    assert!(v.busy());
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Stop"));
    assert!(!v.busy() && v.pending_approval().is_none());
    assert!(v.transcript().last().unwrap().contains("stopped by the person"));
    // a second view over the same port and the same conversation name gets the history back
    h.borrow_mut().push_script(vec![Turn::say("fine")]);
    let mut again = ChatView::new(Box::new(SharedAssistant(h.clone())), cfg(), 500);
    again.send("one more");
    assert!(again.transcript().iter().any(|l| l == "You: add x"), "{:?}", again.transcript());
}

#[test]
fn typing_in_the_field_sends_with_enter_and_keys_are_the_fields_only_while_it_has_focus() {
    let (mut v, _) = view(vec![Turn::say("pong")]);
    assert!(!v.key(UiKey::Char('a')), "not focused: the app keeps its keys");
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Ask the assistant"));
    assert!(v.wants_keyboard());
    for c in "ping".chars() {
        assert!(v.key(UiKey::Char(c)));
    }
    assert!(v.text(" pong"));
    assert_eq!(v.input, "ping pong");
    v.key(UiKey::Enter);
    assert_eq!(v.input, "");
    assert!(v.transcript().contains(&"You: ping pong".to_string()));
    assert!(v.key(UiKey::Esc) && !v.wants_keyboard());
}

#[test]
fn the_view_stays_inside_its_rectangle_with_named_targets_at_every_size_and_text_size() {
    for (w, h, touch) in [(360.0, 740.0, true), (800.0, 480.0, false), (1280.0, 720.0, false)] {
        for scale in [1.0f32, 3.0] {
            let (mut v, _) = view(vec![Turn::call("Adding it now, this is a long sentence so that the card has to wrap onto more than one line of text", "add_note", "{\"text\":\"a very long note text that goes on and on\"}")]);
            v.send("add a very long note with many many words in it so that the transcript wraps around the narrow pane");
            let l = draw(&mut v, w, h, touch, scale);
            let area = Rect::new(0.0, 0.0, w, h);
            for n in l.a11y() {
                assert!(n.rect.x >= -0.5 && n.rect.y >= -0.5 && n.rect.right() <= area.right() + 0.5 && n.rect.bottom() <= area.bottom() + 0.5, "{} sticks out at {w}x{h} x{scale}: {:?}", n.name, n.rect);
                if touch {
                    assert!(n.rect.h >= 44.0 - 0.1, "{} is smaller than a touch target at {w}x{h}", n.name);
                }
            }
            let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
            for want in ["Ask the assistant", "Stop"] {
                assert!(names.iter().any(|n| n == want), "{want} missing at {w}x{h} x{scale}: {names:?}");
            }
            // the newest lines are the ones in view (at 3x text a narrow pane shows only the last few)
            assert!(l.texts().len() > 4, "the transcript is drawn at {w}x{h} x{scale}: {:?}", l.texts());
            if scale == 1.0 && w > 700.0 {
                assert!(l.texts().iter().any(|t| t.starts_with("You:")), "the message is drawn at {w}x{h}");
            }
        }
    }
}

#[test]
fn wrapping_cuts_a_long_word_and_keeps_every_line_inside_the_width() {
    let mut c = ListCanvas::new(100.0, 100.0);
    let lines = wrap(&mut c, 14.0, crate::canvas::TextStyle::Regular, "a verylongwordwithoutanyspacesatall and short\nnext", 90.0);
    assert!(lines.len() >= 3);
    for l in &lines {
        use crate::canvas::Canvas;
        assert!(c.measure(14.0, l, crate::canvas::TextStyle::Regular) <= 90.0 + 0.1, "{l}");
    }
    assert_eq!(lines.last().unwrap(), "next");
}
