//! The assistant pane of SAMPLE: the embedded `ChatView` over a scripted port, driven through the SDK host the way the shell drives it.

use std::cell::RefCell;
use std::rc::Rc;

use iyakki_ui_sdk::assistant::{ScriptedAssistant, SharedAssistant, Turn};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

use super::*;

type Handle = Rc<RefCell<ScriptedAssistant>>;

fn host(w: u32, h: u32, script: Vec<Turn>) -> (Host<SampleApp>, Handle) {
    let (port, handle) = SharedAssistant::new(script);
    (Host::new(SampleApp::new().with_assistant(Box::new(port)), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default())), handle)
}

fn center_of(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn tap(h: &mut Host<SampleApp>, name: &str) {
    let l = h.record();
    let (x, y) = center_of(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn type_text(h: &mut Host<SampleApp>, s: &str) {
    for c in s.chars() {
        h.event(&UiEvent::Key(UiKey::Char(c)));
    }
}

/// Ask the assistant something through the keyboard: `?` focuses its field, Enter sends.
fn ask(h: &mut Host<SampleApp>, s: &str) {
    h.event(&UiEvent::Key(UiKey::Char('?')));
    type_text(h, s);
    h.event(&UiEvent::Key(UiKey::Enter));
}

fn tick(h: &mut Host<SampleApp>) {
    h.event(&UiEvent::Tick);
}

fn add_call(text: &str) -> Turn {
    Turn::call("Adding it.", "add_note", &format!("{{\"text\":\"{text}\"}}"))
}

#[test]
fn a_wide_pane_shows_the_assistant_in_the_side_and_a_button_swaps_it_for_the_screen_panel() {
    let (mut h, _) = host(1280, 720, vec![]);
    let l = h.record();
    assert!(l.texts().contains(&"Ask the assistant") && !l.texts().contains(&"This screen"), "{:?}", l.texts());
    assert!(l.a11y().iter().any(|n| n.name == "Send"));
    tap(&mut h, "Screen panel");
    let l = h.record();
    assert!(l.texts().contains(&"This screen") && !l.texts().contains(&"Ask the assistant"));
    // an app without an assistant is the old app
    let plain = Host::new(SampleApp::new(), Look::default(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default())).record();
    assert!(plain.texts().contains(&"This screen"));
}

#[test]
fn the_assistant_adds_a_note_through_the_app_tool_only_after_the_person_allows_it() {
    let (mut h, handle) = host(1280, 720, vec![add_call("buy milk"), Turn::say("I added buy milk.")]);
    let before = h.app.items.len();
    ask(&mut h, "please add buy milk");
    tick(&mut h);
    assert_eq!(h.app.items.len(), before, "nothing changes while the card waits");
    assert!(h.app.chat().unwrap().pending_approval().is_some());
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.starts_with("Allow:")), "the card says what will happen: {:?}", l.texts());
    tap(&mut h, "Allow");
    tick(&mut h);
    assert_eq!(h.app.items.len(), before + 1);
    assert_eq!(h.app.items.last().map(|i| i.text.as_str()), Some("buy milk"));
    let t = h.app.chat().unwrap().transcript();
    assert!(t.iter().any(|l| l.starts_with("Tool add_note done: Added \"buy milk\"")), "{t:?}");
    assert_eq!(t.last().map(String::as_str), Some("Assistant: I added buy milk."));
    assert_eq!(handle.borrow().audit, vec!["assistant.open notes", "assistant.tool add_note approved", "assistant.tool add_note ran ok"]);
}

#[test]
fn a_denied_tool_leaves_the_notes_alone_and_the_keyboard_can_decide_too() {
    let (mut h, _) = host(1280, 720, vec![add_call("secret"), Turn::say("Okay, not added."), add_call("kept"), Turn::say("Added.")]);
    let n = h.app.items.len();
    ask(&mut h, "add secret");
    tap(&mut h, "Deny");
    tick(&mut h);
    assert_eq!(h.app.items.len(), n);
    assert!(h.app.chat().unwrap().transcript().iter().any(|l| l.contains("the person did not allow it")));
    // Esc leaves the field; with a card showing, y allows
    ask(&mut h, "add kept");
    h.event(&UiEvent::Key(UiKey::Esc));
    h.event(&UiEvent::Key(UiKey::Char('y')));
    tick(&mut h);
    assert_eq!(h.app.items.last().map(|i| i.text.as_str()), Some("kept"));
}

#[test]
fn a_read_only_tool_runs_without_a_card_and_the_assistant_sees_the_list_and_the_selected_note() {
    let (mut h, handle) = host(1280, 720, vec![Turn::call("", "list_notes", "{}"), Turn::say("You have two notes.")]);
    ask(&mut h, "what is on my list?");
    assert!(h.app.chat().unwrap().pending_approval().is_none());
    tick(&mut h);
    let t = h.app.chat().unwrap().transcript().join("\n");
    assert!(t.contains("1 [ ] Try the wallpaper picker") && t.contains("2 [ ] Resize the window"), "{t}");
    let seen = handle.borrow().seen.clone();
    assert!(seen[0].contains("Document Notes: 1 [ ] Try the wallpaper picker\n2 [ ] Resize the window") && seen[0].contains("Selection Selected note: 1 Try the wallpaper picker"), "{seen:?}");
    // selecting another note changes what the next message is shown
    h.app.chat();
    h.event(&UiEvent::Key(UiKey::Esc));
    h.event(&UiEvent::Key(UiKey::Down));
    handle.borrow_mut().push_script(vec![Turn::say("ok")]);
    ask(&mut h, "and this one?");
    assert!(handle.borrow().seen[1].contains("Selected note: 2 Resize the window"), "{:?}", handle.borrow().seen);
}

#[test]
fn bad_tool_arguments_are_told_to_the_model_not_applied() {
    let (mut h, _) = host(
        1280,
        720,
        vec![
            Turn::call("", "add_note", "{\"text\":5}"),
            Turn::call("", "add_note", "{\"text\":\"   \"}"),
            Turn::call("", "check_note", "{\"number\":99}"),
            Turn::call("", "add_note", "not json"),
            Turn::say("I could not do that."),
        ],
    );
    let n = h.app.items.len();
    ask(&mut h, "do the things");
    for _ in 0..4 {
        tap(&mut h, "Allow");
        tick(&mut h);
    }
    assert_eq!(h.app.items.len(), n);
    assert_eq!(h.app.items.iter().filter(|i| i.done).count(), 0);
    let t = h.app.chat().unwrap().transcript().join("\n");
    for want in ["add_note needs `text`", "the note is empty", "there is no note 99", "not a JSON object"] {
        assert!(t.contains(want), "{want} missing in {t}");
    }
}

#[test]
fn the_assistant_checks_a_note_off_and_the_change_is_kept_through_the_storage_port() {
    let store = Rc::new(RefCell::new(iyakki_ui_sdk::ports::MemStorage::default()));
    struct Shared(Rc<RefCell<iyakki_ui_sdk::ports::MemStorage>>);
    impl StoragePort for Shared {
        fn get(&mut self, k: &str) -> Option<String> {
            self.0.borrow_mut().get(k)
        }
        fn set(&mut self, k: &str, v: &str) -> Result<(), iyakki_ui_sdk::ports::PortError> {
            self.0.borrow_mut().set(k, v)
        }
        fn remove(&mut self, k: &str) -> Result<(), iyakki_ui_sdk::ports::PortError> {
            self.0.borrow_mut().remove(k)
        }
        fn keys(&mut self) -> Vec<String> {
            self.0.borrow_mut().keys()
        }
    }
    let (port, _) = SharedAssistant::new(vec![Turn::call("", "check_note", "{\"number\":2}"), Turn::say("Done.")]);
    let app = SampleApp::with_storage(Box::new(Shared(store.clone()))).with_assistant(Box::new(port));
    let mut h = Host::new(app, Look::default(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    ask(&mut h, "check off the second note");
    tap(&mut h, "Allow");
    tick(&mut h);
    assert!(h.app.items[1].done);
    assert!(store.borrow().map.get(NOTES_KEY).unwrap().contains("1 Resize the window"));
}

#[test]
fn a_phone_pane_has_the_notes_first_and_the_assistant_one_button_away() {
    let (mut h, _) = host(360, 740, vec![Turn::say("hello")]);
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "Assistant") && !l.texts().contains(&"Ask the assistant"));
    tap(&mut h, "Assistant");
    let l = h.record();
    assert!(l.texts().contains(&"Ask the assistant") && l.a11y().iter().any(|n| n.name == "Back to notes"));
    ask(&mut h, "hi");
    tick(&mut h);
    assert!(h.app.chat().unwrap().transcript().iter().any(|t| t == "Assistant: hello"));
    tap(&mut h, "Back to notes");
    assert!(h.record().a11y().iter().any(|n| n.name == "Clear done (0)"));
}

#[test]
fn a_host_with_no_assistant_says_so_in_the_pane_and_the_notes_still_work() {
    let app = SampleApp::new().with_assistant(Box::new(iyakki_ui_sdk::assistant::NoAssistant));
    let mut h = Host::new(app, Look::default(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    ask(&mut h, "hello?");
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.contains("not available")), "{:?}", l.texts());
    assert!(h.app.open_count() >= 2);
}

#[test]
fn every_control_of_the_pane_has_a_name_and_stays_inside_at_all_sizes_and_text_sizes() {
    for (w, hh, touch) in [(360u32, 740u32, true), (800, 480, false), (1280, 720, false)] {
        for scale in [1.0f32, 3.0] {
            let (mut h, _) = host(w, hh, vec![add_call("a note that is long enough to wrap in the narrow pane of the phone")]);
            h.touch = touch;
            h.text_scale = scale;
            if w < 600 {
                tap(&mut h, "Assistant");
            }
            ask(&mut h, "add a long note please, with a message long enough to wrap");
            let l = h.record();
            for n in l.a11y() {
                assert!(!n.name.is_empty(), "a control without a name at {w}x{hh}");
                assert!(n.rect.x >= -0.5 && n.rect.y >= -0.5 && n.rect.right() <= w as f32 + 0.5 && n.rect.bottom() <= hh as f32 + 0.5, "{} sticks out at {w}x{hh} x{scale}: {:?}", n.name, n.rect);
            }
        }
    }
}
