//! Tests that need the whole Shell and the apps together. They lived in `apps/` before the apps split (alpha.4); an app
//! crate cannot see the shell's tile table, the launcher groups or the shell's drawing, so these checks are made here.

use crate::launcher::Group;
use crate::shell::{draw_capture_sign, tiles_for_test};
use crate::ui::{Event, Key};
use iyakki_sec::policy::App as PApp;

fn tiles_of(app: PApp) -> Vec<&'static str> {
    tiles_for_test().into_iter().filter(|t| t.0 == app).map(|t| t.1).collect()
}

#[test]
fn the_tiles_of_dash_flows_lab_and_linux_window_are_registered_once_in_their_groups() {
    assert_eq!(tiles_of(PApp::Dash), vec!["DASH"]);
    assert_eq!(Group::of(PApp::Dash), Some(Group::Data));
    assert_eq!(tiles_of(PApp::Flows), vec!["FLOWS"]);
    assert_eq!(Group::of(PApp::Flows), Some(Group::Work));
    assert_eq!(tiles_of(PApp::Lab), vec!["LAB"]);
    assert_eq!(Group::of(PApp::Lab), Some(Group::Home)); // the home screen groups changed (F-SHELL-22, F-SHELL-54): LAB sits in Ask & Create
    assert!(tiles_of(PApp::LinuxWin).contains(&"LINUX WINDOW"));
}

#[test]
fn the_ide_tales_and_store_tiles_are_in_their_home_screen_groups() {
    assert_eq!(Group::of(PApp::Ide), Some(Group::Work));
    assert!(Group::Work.holds(PApp::Ide));
    assert_eq!(Group::of(PApp::Tales), Some(Group::Home));
    assert_eq!(Group::of(PApp::Store), Some(Group::System));
}

#[test]
fn the_capture_sign_is_drawn_inside_the_screen_with_the_kinds_in_words() {
    let mut g = crate::gfx::Gfx::new(1280, 720);
    let r = draw_capture_sign(&mut g, 720, 1.0, 3, false);
    assert!(r.x >= 0.0 && r.y + r.h <= 720.0 && r.x + r.w <= 1280.0, "inside the screen");
    let shown = g.text_log.iter().map(|t| t.2.as_str()).collect::<Vec<_>>().join("\n");
    assert!(shown.contains("A page is using your camera and microphone. Click to stop."), "{shown}");
}

#[test]
fn the_shell_hands_the_device_popover_every_event_and_redraws_when_the_tooltip_changes() {
    let shell = include_str!("shell.rs");
    assert!(shell.contains("self.devpop.tip_event(ev)") && shell.contains("self.devpop.tip_changed()"));
}

#[test]
fn a_real_mouse_drag_through_the_shell_selects_chat_text_and_ctrl_c_copies_it() {
    use crate::shell::tests::{login, shell_with_users};
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("chatmouse");
    login(&mut sh, "alice", "alicepass1");
    let app = crate::apps::chat::ChatApp::with_transcript_for_test();
    sh.show_app_for_test(Box::new(app), iyakki_sec::policy::App::Chat);
    let mut g = crate::gfx::Gfx::new(1280, 900);
    sh.render(&mut g);
    let area = sh.hits().iter().find(|h| h.label == "text area").expect("the transcript area").rect;
    // press inside the first message, drag down into the answer, release
    let (x0, y0) = ((area.x + 70.0) as i32, (area.y + 30.0) as i32);
    sh.handle(&Event::MouseMove(x0, y0));
    sh.handle(&Event::MouseDown(x0, y0));
    for step in 1..=8 {
        let (x, y) = (x0 + step * 20, y0 + step * 25);
        sh.handle(&Event::MouseMove(x, y));
        sh.render(&mut g);
    }
    sh.handle(&Event::MouseUp(x0 + 160, y0 + 200));
    sh.handle(&Event::Key(Key::Ctrl('c')));
    let copied = crate::ui::clipboard::latest();
    assert!(copied.as_deref().is_some_and(|t| t.chars().count() > 20), "the dragged text was copied: {copied:?}");
}

#[test]
fn the_model_picker_takes_typing_keys_and_the_wheel_through_the_shell_like_a_real_keyboard_and_mouse() {
    use crate::shell::tests::{login, shell_with_users};
    use iyakki_shared::providers::{Kind, ProviderCfg};
    let (mut sh, _) = shell_with_users("pickershell");
    login(&mut sh, "alice", "alicepass1");
    {
        let svc = sh.services_for_demo().clone();
        let mut st = svc.settings().lock().unwrap();
        let mut a = ProviderCfg::new("ollama", Kind::Ollama);
        a.models = (0..40).map(|i| format!("llama-{i:02}")).collect();
        st.upsert_provider(a).unwrap();
        let mut b = ProviderCfg::new("lab", Kind::Compat);
        b.base_url = Some("http://192.168.1.9:8000/v1".into());
        b.models = vec!["nemotron-mini".into(), "qwen3".into()];
        st.upsert_provider(b).unwrap();
    }
    sh.show_app_for_test(Box::new(crate::apps::chat::ChatApp::with_transcript_for_test()), iyakki_sec::policy::App::Chat);
    let mut g = crate::gfx::Gfx::new(1280, 720);
    sh.render(&mut g);
    // Ctrl+P opens the dialog: providers as headings, the model rows, a scroll bar
    sh.handle(&Event::Key(Key::Ctrl('p')));
    let mut g = crate::gfx::Gfx::new(1280, 720);
    sh.render(&mut g);
    let hits: Vec<_> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(hits.iter().any(|l| l == "provider") && hits.iter().any(|l| l == "model"), "{hits:?}");
    assert!(hits.iter().any(|l| l == "scroll bar"), "a long list shows its bar: {hits:?}");
    let rows = |sh: &crate::shell::Shell| sh.hits().iter().filter(|h| h.label == "model").count();
    let first_row = |sh: &crate::shell::Shell| sh.hits().iter().find(|h| h.label == "model").map(|h| h.ev.clone());
    // the wheel (the pointer over the dialog) scrolls it
    let before = first_row(&sh);
    sh.handle(&Event::MouseMove(640, 400));
    sh.handle(&Event::Scroll(-2));
    sh.render(&mut g);
    assert_ne!(first_row(&sh), before, "the wheel moved the list");
    // typing: every printable key arrives as Key::Char
    for c in "nemo".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.render(&mut g);
    assert!(g.text_log.iter().any(|t| t.2.contains("Search models: nemo")), "the query is shown");
    assert_eq!(rows(&sh), 1, "one match");
    sh.handle(&Event::Key(Key::Backspace));
    sh.render(&mut g);
    assert!(g.text_log.iter().any(|t| t.2.contains("Search models: nem")));
    // a pasted word works as well
    sh.handle(&Event::Paste("o".into()));
    sh.handle(&Event::Key(Key::Down));
    sh.handle(&Event::Key(Key::PageDown));
    sh.handle(&Event::Key(Key::Home));
    sh.render(&mut g);
    assert_eq!(rows(&sh), 1);
    // Esc clears the query first and closes the dialog second
    sh.handle(&Event::Key(Key::Esc));
    let mut g = crate::gfx::Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(rows(&sh) > 1 && !g.text_log.iter().any(|t| t.2.contains("Search:")));
    sh.handle(&Event::Key(Key::Esc));
    let mut g2 = crate::gfx::Gfx::new(1280, 720);
    sh.render(&mut g2);
    assert!(!sh.hits().iter().any(|h| h.label == "model"), "closed");
}
