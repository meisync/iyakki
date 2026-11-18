use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::wallpaper;

use super::*;

fn host(w: u32, h: u32) -> Host<SampleApp> {
    Host::new(SampleApp::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn tap(h: &mut Host<SampleApp>, x: f32, y: f32) {
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn center_of(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn type_text(h: &mut Host<SampleApp>, s: &str) {
    for c in s.chars() {
        h.event(&UiEvent::Key(UiKey::Char(c)));
    }
}

#[test]
fn a_phone_pane_gets_one_column_and_a_wide_pane_gets_a_side_panel() {
    let mut phone = host(360, 740);
    let l = phone.record();
    assert!(l.texts().contains(&"Sample") && !l.texts().contains(&"This screen"), "{:?}", l.texts());
    assert!(l.a11y().iter().any(|n| n.name == "Pin to dock"), "the pin button sits under the list on a phone");
    assert!(l.texts().iter().any(|t| t.starts_with("compact pane, 360 x 740")));
    let mut wide = host(1280, 720);
    let l = wide.record();
    assert!(l.texts().contains(&"This screen") && l.texts().contains(&"Wallpaper"));
    assert!(l.texts().iter().any(|t| t.starts_with("expanded pane, 1280 x 720")));
    // resizing the same pane changes the layout (the size class follows the pane)
    wide.resize(400.0, 700.0);
    assert!(!wide.record().texts().contains(&"This screen"));
}

#[test]
fn typing_a_note_and_pressing_enter_adds_it_and_a_tap_checks_it_off() {
    let mut h = host(800, 480);
    let l = h.record();
    let (x, y) = center_of(&l, "New note");
    tap(&mut h, x, y);
    assert!(h.app.focused, "a tap on the field focuses it");
    type_text(&mut h, "buy milk");
    h.event(&UiEvent::Key(UiKey::Enter));
    assert_eq!(h.app.items.last().map(|i| i.text.as_str()), Some("buy milk"));
    assert_eq!(h.app.input, "");
    h.event(&UiEvent::Key(UiKey::Esc));
    assert!(!h.app.focused);
    let l = h.record();
    let (x, y) = center_of(&l, "[ ] buy milk");
    tap(&mut h, x, y);
    assert!(h.app.items.last().unwrap().done, "a tap on the row toggles it");
    assert_eq!(h.app.open_count(), 2);
    // clear done
    let l = h.record();
    let (x, y) = center_of(&l, "Clear done (1)");
    tap(&mut h, x, y);
    assert_eq!(h.app.items.len(), 2);
    // Escape with nothing focused leaves the app
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn pasted_text_and_control_characters_are_handled_and_the_limits_hold() {
    let mut a = SampleApp::new();
    let mut h = host(800, 480);
    h.app.focused = true;
    h.event(&UiEvent::Text("a\u{7}b\nc".into()));
    assert_eq!(h.app.input, "abc");
    h.event(&UiEvent::Text("x".repeat(500)));
    assert_eq!(h.app.input.chars().count(), MAX_LEN);
    a.input = "   ".into();
    a.add();
    assert_eq!(a.items.len(), 2, "a blank note is not added");
    for i in 0..MAX_ITEMS + 5 {
        a.input = format!("n{i}");
        a.add();
    }
    assert_eq!(a.items.len(), MAX_ITEMS);
}

#[test]
fn the_dock_hears_the_open_count_and_the_pin_button_goes_through_the_dock_port() {
    let mut h = host(800, 480);
    h.event(&UiEvent::Tick);
    let l = h.record();
    let (x, y) = center_of(&l, "Pin to dock");
    tap(&mut h, x, y);
    assert_eq!(h.dock.pinned(), vec![ID.to_string()]);
    assert_eq!(h.app.note, "Pinned to the dock");
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "Unpin from dock"));
    let (x, y) = center_of(&l, "Unpin from dock");
    tap(&mut h, x, y);
    assert!(h.dock.pinned().is_empty());
    // a dock that refuses is reported, not hidden
    let mut refused = Host::new(SampleApp::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::default()), Box::new(MemWallpaper::default()));
    let l = refused.record();
    let (x, y) = center_of(&l, "Pin to dock");
    tap(&mut refused, x, y);
    assert_eq!(refused.app.note, "The dock did not allow that");
}

#[test]
fn the_wallpaper_picker_sets_the_wallpaper_of_this_screen_through_the_port() {
    let mut h = host(1280, 720);
    let l = h.record();
    let (x, y) = center_of(&l, "Wallpaper Meadow");
    tap(&mut h, x, y);
    assert_eq!(h.wallpaper.get(0), wallpaper::Id::Meadow);
}

#[test]
fn every_size_and_text_size_draws_inside_the_pane_with_named_controls() {
    for (w, h, touch) in [(360u32, 740u32, true), (800, 480, false), (1280, 720, false)] {
        for text_scale in [1.0f32, 3.0] {
            let mut host = host(w, h);
            host.touch = touch;
            host.text_scale = text_scale;
            let l = host.record();
            for op in &l.ops {
                if let Op::Hit(r, hit) = op {
                    assert!(!hit.name.is_empty(), "an unnamed control");
                    assert!(r.x >= -0.01 && r.right() <= w as f32 + 0.01, "{} sticks out sideways at {w}x{h} x{text_scale}: {r:?}", hit.name);
                    if touch {
                        assert!(r.h >= 44.0, "{} is too small to touch ({r:?})", hit.name);
                    }
                }
            }
            assert!(l.texts().contains(&"Sample"));
        }
    }
}

#[test]
fn arrow_keys_wheel_and_delete_move_through_the_list() {
    let mut h = host(800, 480);
    h.event(&UiEvent::Key(UiKey::Down));
    assert_eq!(h.app.selected, 1);
    h.event(&UiEvent::Key(UiKey::Down));
    assert_eq!(h.app.selected, 1, "stops at the last row");
    h.event(&UiEvent::Wheel(1));
    assert_eq!(h.app.selected, 0);
    h.event(&UiEvent::Key(UiKey::Delete));
    assert_eq!(h.app.items.len(), 1);
    h.event(&UiEvent::Key(UiKey::Char(' ')));
    assert!(h.app.items[0].done);
    assert_eq!(h.app.title(), "SAMPLE");
}

/// The SDK rule for new apps: colours come from the Look. No colour literal, no machine access, and no IYAKKI crate but the SDK.
#[test]
fn the_app_uses_only_the_sdk_and_no_colour_of_its_own() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let code = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    let code = code.split("#[cfg(test)]").next().unwrap();
    for bad in ["rgb(", "rgba(", "[0, 0, 0,", "255, 255, 255", "std::fs", "std::net", "std::process", "iyakki_uikit", "iyakki_agent", "iyakki_shell", "\"/proc", "\"/sys", "\"/dev"] {
        assert!(!code.contains(bad), "lib.rs contains {bad}");
    }
    let has_colour_literal = code.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| {
        let mut nums = 0;
        for part in l.split(|c: char| !c.is_ascii_digit()) {
            if part.parse::<u16>().is_ok_and(|n| n > 31) && l.contains('[') && l.contains(',') && !l.contains("0.") {
                nums += 1;
            }
        }
        nums >= 3 && l.contains("[") && !l.contains("MAX")
    });
    assert!(!has_colour_literal, "a colour literal in lib.rs");
    let m = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let mut in_deps = false;
    for l in m.lines() {
        if l.starts_with('[') {
            in_deps = l.trim() == "[dependencies]";
        } else if in_deps && l.starts_with("iyakki-") {
            assert!(l.starts_with("iyakki-ui-sdk"), "a dependency besides the SDK: {l}");
        }
    }
}

#[test]
fn the_keyboard_is_wanted_exactly_while_the_field_has_focus() {
    let mut h = host(360, 740);
    assert!(!h.wants_keyboard());
    let l = h.record();
    let (x, y) = center_of(&l, "New note");
    tap(&mut h, x, y);
    assert!(h.wants_keyboard());
    h.event(&UiEvent::Key(UiKey::Esc));
    assert!(!h.wants_keyboard());
}

#[test]
fn notes_given_a_storage_port_come_back_and_every_change_is_saved() {
    use iyakki_ui_sdk::ports::{MemStorage, PortError, StoragePort};
    use std::cell::RefCell;
    use std::rc::Rc;
    // a port that shows what was kept to the test
    struct Shared(Rc<RefCell<MemStorage>>);
    impl StoragePort for Shared {
        fn get(&mut self, k: &str) -> Option<String> {
            self.0.borrow_mut().get(k)
        }
        fn set(&mut self, k: &str, v: &str) -> Result<(), PortError> {
            self.0.borrow_mut().set(k, v)
        }
        fn remove(&mut self, k: &str) -> Result<(), PortError> {
            self.0.borrow_mut().remove(k)
        }
        fn keys(&mut self) -> Vec<String> {
            self.0.borrow_mut().keys()
        }
    }
    let kept = Rc::new(RefCell::new(MemStorage::default()));
    let mk = |kept: &Rc<RefCell<MemStorage>>| Host::new(SampleApp::with_storage(Box::new(Shared(kept.clone()))), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    let mut h = mk(&kept);
    h.app.focused = true;
    type_text(&mut h, "milk");
    h.event(&UiEvent::Key(UiKey::Enter));
    h.event(&UiEvent::Key(UiKey::Esc));
    h.event(&UiEvent::Key(UiKey::Enter)); // toggles the selected (new) note
    assert_eq!(kept.borrow().map.get(NOTES_KEY).map(String::as_str), Some("0 Try the wallpaper picker\n0 Resize the window\n1 milk\n"));
    let h2 = mk(&kept);
    assert_eq!(h2.app.items.len(), 3);
    assert!(h2.app.items[2].done && h2.app.items[2].text == "milk");
    // damaged storage is read as far as it makes sense and never crashes
    kept.borrow_mut().map.insert(NOTES_KEY.into(), "garbage without a space\n1 ok\n\u{7}0 control\n".into());
    let h3 = mk(&kept);
    assert_eq!(h3.app.items.len(), 1, "lines that are not `0 text` or `1 text` are dropped");
    assert!(h3.app.items[0].done && h3.app.items[0].text == "ok");
    // a port that refuses says so under the pin button instead of failing silently
    struct Full;
    impl StoragePort for Full {
        fn get(&mut self, _: &str) -> Option<String> {
            None
        }
        fn set(&mut self, _: &str, _: &str) -> Result<(), PortError> {
            Err(PortError::TooBig)
        }
        fn remove(&mut self, _: &str) -> Result<(), PortError> {
            Ok(())
        }
        fn keys(&mut self) -> Vec<String> {
            Vec::new()
        }
    }
    let mut a = SampleApp::with_storage(Box::new(Full));
    a.input = "x".into();
    a.add();
    assert!(a.note.contains("could not be saved"));
}

/// F-SDK-121: an SDK app writes no styling numbers either: gaps, sizes and radii come from the skin's tokens.
#[test]
fn the_app_has_no_styling_numbers_of_its_own() {
    let bad = iyakki_ui_sdk::lint::style_literals(include_str!("lib.rs"));
    assert!(bad.is_empty(), "use m.gap, m.size, m.radius, m.line:\n{}", bad.join("\n"));
}

fn files_host(w: u32, h: u32) -> Host<SampleApp> {
    let mut f = iyakki_ui_sdk::files::MemFiles::new();
    f.put("hello.txt", "Hello from a file\nsecond line");
    f.put("Docs/a.txt", "a");
    Host::new(SampleApp::new().with_files(Box::new(f)), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

#[test]
fn the_files_pane_shows_the_home_folder_opens_a_text_file_and_gives_the_app_its_first_line() {
    let mut h = files_host(800, 480);
    assert!(h.app.has_files() && !h.app.files_open());
    let l = h.record();
    let (x, y) = center_of(&l, "Files");
    tap(&mut h, x, y);
    assert!(h.app.files_open());
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "Notes"), "the button now goes back");
    assert_eq!(h.app.files().unwrap().names(), ["Docs/", "hello.txt"]);
    let (x, y) = center_of(&l, "hello.txt");
    tap(&mut h, x, y);
    assert_eq!(h.app.files().unwrap().preview().map(|p| p.0.as_str()), Some("hello.txt"));
    assert_eq!(h.app.note, "hello.txt: Hello from a file");
    // Esc closes the text, Esc again closes the pane, a third goes back out of the app
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(h.app.files().unwrap().preview().is_none() && h.app.files_open());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(!h.app.files_open());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn the_key_f_opens_the_pane_only_when_the_app_has_a_files_port_and_the_pane_draws_in_every_size() {
    let mut plain = host(800, 480);
    plain.event(&UiEvent::Key(UiKey::Char('f')));
    assert!(!plain.app.files_open());
    let l = plain.record();
    assert!(!l.a11y().iter().any(|n| n.name == "Files"), "no button without the port");
    for (w, hgt) in [(320, 480), (800, 480), (1600, 900)] {
        let mut h = files_host(w, hgt);
        h.event(&UiEvent::Key(UiKey::Char('f')));
        assert!(h.app.files_open());
        h.event(&UiEvent::Key(UiKey::Down));
        h.event(&UiEvent::Key(UiKey::Enter));
        let l = h.record();
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= hgt as f32 + 0.5, "{} outside at {w}x{hgt}", hit.name);
            }
        }
    }
}

fn term_host(w: u32, h: u32) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_terminal(Box::new(iyakki_ui_sdk::terminal::MemTerminal::new())), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

#[test]
fn the_terminal_pane_runs_a_read_only_line_at_once_and_a_change_only_after_the_person_allows() {
    let mut h = term_host(800, 480);
    assert!(h.app.has_terminal() && !h.app.terminal_open());
    let l = h.record();
    let (x, y) = center_of(&l, "Terminal");
    tap(&mut h, x, y);
    assert!(h.app.terminal_open());
    for c in "echo hi".chars() {
        h.event(&UiEvent::Key(UiKey::Char(c)));
    }
    h.event(&UiEvent::Key(UiKey::Enter));
    assert!(h.app.terminal().unwrap().transcript().contains(&"hi".to_string()));
    for c in "mkdir docs".chars() {
        h.event(&UiEvent::Text(c.to_string()));
    }
    h.event(&UiEvent::Key(UiKey::Enter));
    assert!(h.app.terminal().unwrap().pending_question().is_some());
    let l = h.record();
    let (x, y) = center_of(&l, "Allow");
    tap(&mut h, x, y);
    assert!(h.app.terminal().unwrap().transcript().contains(&"done: mkdir docs".to_string()));
    // Esc leaves the field first, then the pane, then the app
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(h.app.terminal_open());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(!h.app.terminal_open());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn the_terminal_and_files_panes_exclude_each_other_and_draw_inside_the_pane_at_every_size() {
    let mut f = iyakki_ui_sdk::files::MemFiles::new();
    f.put("a.txt", "x");
    for (w, hgt) in [(320, 480), (800, 480), (1600, 900)] {
        let mut h = Host::new(SampleApp::new().with_files(Box::new(f_clone(&f))).with_terminal(Box::new(iyakki_ui_sdk::terminal::MemTerminal::new())), Look::default(), Arrangement::single(w, hgt), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
        h.event(&UiEvent::Key(UiKey::Char('f')));
        assert!(h.app.files_open() && !h.app.terminal_open());
        h.event(&UiEvent::Key(UiKey::Esc));
        h.event(&UiEvent::Key(UiKey::Char('t')));
        assert!(h.app.terminal_open() && !h.app.files_open());
        let l = h.record();
        let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Run") && names.iter().any(|n| n == "Files"), "{names:?}");
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= hgt as f32 + 0.5, "{} outside at {w}x{hgt}", hit.name);
            }
        }
    }
}

fn f_clone(src: &iyakki_ui_sdk::files::MemFiles) -> iyakki_ui_sdk::files::MemFiles {
    let _ = src;
    let mut f = iyakki_ui_sdk::files::MemFiles::new();
    f.put("a.txt", "x");
    f
}

#[test]
fn the_editor_pane_edits_the_notes_as_text_and_save_turns_the_lines_back_into_notes() {
    let mut h = ed_host(800, 480);
    assert!(!h.app.editor_open());
    let l = h.record();
    let (x, y) = center_of(&l, "Edit");
    tap(&mut h, x, y);
    assert!(h.app.editor_open());
    assert_eq!(h.app.editor().unwrap().text(), "Try the wallpaper picker\nResize the window\n");
    // tap the first line, type at its end, check off nothing, add a done note on a new line
    let l = h.record();
    let (x, y) = center_of(&l, "line 1");
    tap(&mut h, x, y);
    assert!(h.app.wants_keyboard(), "the editor has the focus: the host shows the keyboard");
    h.event(&UiEvent::Text(" now".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    h.event(&UiEvent::Text("[x] a done one".into()));
    let l = h.record();
    let (x, y) = center_of(&l, "Save");
    tap(&mut h, x, y);
    let notes: Vec<(String, bool)> = h.app.items.iter().map(|i| (i.text.clone(), i.done)).collect();
    assert_eq!(notes, [("Try the wallpaper picker now".to_string(), false), ("a done one".to_string(), true), ("Resize the window".to_string(), false)]);
    assert_eq!(h.app.note, "Notes saved from the editor");
    assert!(!h.app.editor().unwrap().dirty());
    // Esc gives the focus back, Esc again leaves the pane
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(h.app.editor_open());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(!h.app.editor_open());
}

#[test]
fn the_editor_pane_draws_inside_every_size_with_named_controls() {
    for (w, hgt) in [(320, 480), (800, 480), (1600, 900)] {
        let mut h = ed_host(w, hgt);
        h.event(&UiEvent::Key(UiKey::Char('e')));
        assert!(h.app.editor_open());
        let l = h.record();
        let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Undo") && names.iter().any(|n| n == "Save"), "{names:?}");
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= hgt as f32 + 0.5, "{} outside at {w}x{hgt}", hit.name);
            }
        }
    }
}

fn ed_host(w: u32, h: u32) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_editor(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

#[test]
fn the_editors_assistant_is_given_the_text_and_its_reply_becomes_a_proposal_the_person_reviews() {
    use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
    let reply = "I will mark it.\n<<<<<<< SEARCH\nResize the window\n=======\nResize the window twice\n>>>>>>> REPLACE";
    let (port, handle) = SharedAssistant::new(vec![Turn::say(reply)]);
    let mut h = Host::new(SampleApp::new().with_editor().with_editor_assistant(Box::new(port)), Look::default(), Arrangement::single(1000, 560), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    let l = h.record();
    let (x, y) = center_of(&l, "Edit");
    tap(&mut h, x, y);
    assert!(h.app.editor_open());
    // the assistant is beside the text on a wide pane
    let l = h.record();
    let (x, y) = center_of(&l, "Ask the assistant");
    tap(&mut h, x, y);
    h.event(&UiEvent::Text("make the second note say twice".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    // the model was shown how to propose a change, the document and (no selection yet)
    let seen = handle.borrow().seen.join("\n");
    assert!(seen.contains("SEARCH") && seen.contains("Resize the window") && seen.contains("How to change the text"), "the model was told how to propose a change and shown the text: {seen}");
    assert!(h.app.editor().unwrap().reviewing(), "the reply became a proposal");
    assert_eq!(h.app.editor().unwrap().text(), "Try the wallpaper picker\nResize the window\n", "nothing changed before the person decides");
    let l = h.record();
    let (x, y) = center_of(&l, "Apply");
    tap(&mut h, x, y);
    assert_eq!(h.app.editor().unwrap().text(), "Try the wallpaper picker\nResize the window twice\n");
    let l = h.record();
    let (x, y) = center_of(&l, "Save");
    tap(&mut h, x, y);
    assert_eq!(h.app.items[1].text, "Resize the window twice");
}

#[test]
fn a_reply_with_no_usable_edit_is_said_not_applied_and_a_narrow_pane_switches_between_text_and_assistant() {
    use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
    let (port, _h) = SharedAssistant::new(vec![Turn::say("<<<<<<< SEARCH\nnot in the text\n=======\nx\n>>>>>>> REPLACE")]);
    let mut h = Host::new(SampleApp::new().with_editor().with_editor_assistant(Box::new(port)), Look::default(), Arrangement::single(360, 560), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    let l = h.record();
    let (x, y) = center_of(&l, "Edit");
    tap(&mut h, x, y);
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "Assistant") && !l.a11y().iter().any(|n| n.name == "Ask the assistant"), "narrow: the text first, a button to the assistant");
    let (x, y) = center_of(&l, "Assistant");
    tap(&mut h, x, y);
    let l = h.record();
    let (x, y) = center_of(&l, "Ask the assistant");
    tap(&mut h, x, y);
    h.event(&UiEvent::Text("change it".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    assert!(!h.app.editor().unwrap().reviewing());
    assert!(h.app.note.contains("could not be used"), "{}", h.app.note);
    let l = h.record();
    let (x, y) = center_of(&l, "Editor");
    tap(&mut h, x, y);
    assert!(h.record().a11y().iter().any(|n| n.name == "Undo"));
}

#[test]
fn a_file_opened_in_the_files_pane_is_edited_and_its_save_waits_for_the_persons_yes_in_the_files_pane() {
    let mut f = iyakki_ui_sdk::files::MemFiles::new();
    f.put("todo.txt", "buy bread");
    let mut h = Host::new(SampleApp::new().with_files(Box::new(f)).with_editor(), Look::default(), Arrangement::single(1000, 600), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    let l = h.record();
    let (x, y) = center_of(&l, "Files");
    tap(&mut h, x, y);
    let l = h.record();
    let (x, y) = center_of(&l, "todo.txt");
    tap(&mut h, x, y);
    assert!(h.app.files().unwrap().preview().is_some());
    // `e` puts the opened file in the editor
    h.event(&UiEvent::Key(UiKey::Char('e')));
    assert!(h.app.editor_open());
    assert_eq!(h.app.editor().unwrap().text(), "buy bread");
    let l = h.record();
    let (x, y) = center_of(&l, "line 1");
    tap(&mut h, x, y);
    h.event(&UiEvent::Text(" and milk".into()));
    let l = h.record();
    let (x, y) = center_of(&l, "Save");
    tap(&mut h, x, y);
    // the question is in the Files pane; nothing is saved yet
    assert!(h.app.files_open());
    assert!(h.app.files().unwrap().pending_question().unwrap().summary.contains("Save `todo.txt`"));
    assert_eq!(h.app.items.len(), 2, "the notes were not touched");
    let l = h.record();
    let (x, y) = center_of(&l, "Allow");
    tap(&mut h, x, y);
    assert_eq!(h.app.files().unwrap().note(), Some("Saved `todo.txt`."));
    assert!(!h.app.editor().unwrap().dirty(), "the editor knows it is saved");
    // reading the file again shows the new text
    let l = h.record();
    let (x, y) = center_of(&l, "todo.txt");
    tap(&mut h, x, y);
    assert_eq!(h.app.files().unwrap().preview().map(|p| p.1.as_str()), Some("buy bread and milk"));
}
