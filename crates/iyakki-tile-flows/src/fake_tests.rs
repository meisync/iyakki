//! The FLOWS tile against a fake port: no engine, no store, no chat backend, no files, no clock, no sleep (the worked example of
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10). The same screens are also proved through the real environment and the real
//! chat in `crates/iyakki-apps/src/apps/flows_tests.rs` (61 tests).

use std::sync::{Arc, Mutex};

use iyakki_flows_model::failfix::{FailBook, Failure};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use crate::fake::FakePort;
use crate::flows::{Focus, Modal, Tab};
use crate::port::{AppFlow, ChatPane, DropAction, FlowsShell, PictureFiles};
use crate::FlowsApp;

/// What the fake assistant pane was asked to do.
#[derive(Default)]
struct ChatLog {
    events: usize,
    asked: Vec<String>,
    notes: Vec<String>,
    agent_mode: bool,
    auto_ended: bool,
}

struct FakeChat {
    log: Arc<Mutex<ChatLog>>,
    running: bool,
    auto: bool,
}

impl ChatPane for FakeChat {
    fn handle(&mut self, _ev: &Event, _cx: &dyn FlowsShell) -> AppFlow {
        self.log.lock().unwrap().events += 1;
        AppFlow::Stay
    }
    fn decode_slot(&self, _ev: &Event) -> Option<Event> {
        None
    }
    fn render(&mut self, _g: &mut Gfx, _area: R, _s: f32, _cx: &dyn FlowsShell, _tick: u64) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn is_running(&self) -> bool {
        self.running
    }
    fn pending_approval(&self) -> bool {
        false
    }
    fn local_note(&mut self, _cx: &dyn FlowsShell, text: &str) {
        self.log.lock().unwrap().notes.push(text.into());
    }
    fn set_agent_mode(&mut self, on: bool) {
        self.log.lock().unwrap().agent_mode = on;
    }
    fn auto_active(&self) -> bool {
        self.auto
    }
    fn end_auto_unless_pinned(&mut self) {
        self.log.lock().unwrap().auto_ended = true;
        self.auto = false;
    }
    fn input_text(&self) -> String {
        String::new()
    }
    fn ask(&mut self, _cx: &dyn FlowsShell, text: &str) {
        self.log.lock().unwrap().asked.push(text.into());
    }
    fn prefill(&mut self, _text: &str) {}
}

/// The shell: the assistant's tools on or off, and the files a person may import by name.
struct FakeShell {
    agent: bool,
    files: Vec<(String, String)>,
}

impl FlowsShell for FakeShell {
    fn shell(&self) -> &dyn std::any::Any {
        &()
    }
    fn agent_enabled(&self) -> bool {
        self.agent
    }
    fn read_import(&self, path: &str, max: u64) -> Result<String, String> {
        let (_, text) = self.files.iter().find(|(p, _)| p == path).ok_or_else(|| "that file is outside the folders you may read".to_string())?;
        if text.len() as u64 > max {
            return Err("that file is over 2 MB".into());
        }
        Ok(text.clone())
    }
}

/// No picture files: the fake knows no artifact folder.
struct NoPictures;

impl PictureFiles for NoPictures {
    fn pixmap(&self, _data: &std::path::Path, _path: &str, _max: usize) -> Option<Arc<tiny_skia::Pixmap>> {
        None
    }
    fn bytes(&self, _data: &std::path::Path, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

struct Fx {
    app: FlowsApp,
    port: Arc<FakePort>,
    shell: FakeShell,
    chat: Arc<Mutex<ChatLog>>,
    g: Gfx,
    area: R,
    tick: u64,
}

impl Fx {
    fn new(port: Arc<FakePort>, agent: bool) -> Fx {
        let chat = Arc::new(Mutex::new(ChatLog::default()));
        let log = chat.clone();
        let app = FlowsApp::new(port.clone(), Box::new(NoPictures), Box::new(FailBook::in_memory()), |_open| Box::new(FakeChat { log, running: false, auto: false }));
        let mut f = Fx { app, port, shell: FakeShell { agent, files: vec![] }, chat, g: Gfx::new(1280, 720), area: R::new(0.0, 46.0, 1280.0, 674.0), tick: 0 };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, self.area, 1.0, &self.shell, self.tick);
    }
    fn ev(&mut self, e: Event) -> AppFlow {
        let r = self.app.handle(&e, &self.shell);
        self.draw();
        r
    }
    fn key(&mut self, k: Key) -> AppFlow {
        self.ev(Event::Key(k))
    }
    fn types(&mut self, t: &str) {
        for c in t.chars() {
            self.key(Key::Char(c));
        }
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
}

fn port() -> Arc<FakePort> {
    Arc::new(FakePort::default())
}

const FLOW_JSON: &str = r#"{"id":"imported","name":"Imported check","nodes":[],"edges":[]}"#;

#[test]
fn the_list_shows_the_flows_the_port_returns_and_an_empty_account_says_how_to_begin() {
    let f = Fx::new(port(), true);
    assert!(f.text().contains("No workflows yet"), "{}", f.text());
    let f = Fx::new(port().with_flow("check", "Site check").with_flow("backup", "Nightly backup"), true);
    let t = f.text();
    assert!(t.contains("Site check") && t.contains("Nightly backup"), "{t}");
    assert_eq!(f.app.flows.len(), 2);
}

#[test]
fn n_names_a_new_flow_and_the_port_makes_the_draft_and_the_tile_opens_it() {
    let mut f = Fx::new(port(), true);
    f.key(Key::Char('n'));
    assert!(matches!(f.app.modal, Some(Modal::Name(..))));
    f.types("Daily report");
    f.key(Key::Enter);
    let made: Vec<String> = f.port.st().flows.values().map(|x| x.name.clone()).collect();
    assert_eq!(made, ["Daily report"], "one draft, made through the port");
    assert!(f.app.cur.is_some(), "the new flow is open");
    assert!(f.app.modal.is_none());
}

#[test]
fn opening_a_flow_tells_the_shared_state_the_assistant_reads() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    let open = f.app.state();
    assert_eq!(open.lock().unwrap().flow, None);
    f.key(Key::Enter);
    assert_eq!(f.app.cur.as_deref(), Some("check"));
    assert_eq!(open.lock().unwrap().flow.as_deref(), Some("check"));
    assert_eq!(open.lock().unwrap().tab, "design");
}

#[test]
fn esc_on_the_list_leaves_the_tile_and_in_a_flow_it_goes_back_to_the_list_first() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.key(Key::Enter);
    assert_eq!(f.key(Key::Esc), AppFlow::Stay, "in a flow Esc closes the flow, not the tile");
    assert!(f.app.cur.is_none());
    assert_eq!(f.key(Key::Esc), AppFlow::Back);
}

#[test]
fn a_template_makes_a_draft_through_the_port() {
    let mut f = Fx::new(port(), true);
    f.key(Key::Char('t'));
    assert!(matches!(f.app.modal, Some(Modal::Templates(..))));
    f.key(Key::Enter);
    assert_eq!(f.port.st().flows.len(), 1, "the first template was made");
    assert!(f.app.cur.is_some());
}

#[test]
fn import_reads_the_file_through_the_shell_and_the_port_makes_the_draft() {
    let mut f = Fx::new(port(), true);
    f.shell.files.push(("Documents/check.flow.json".into(), FLOW_JSON.into()));
    f.key(Key::Char('i'));
    assert!(matches!(f.app.modal, Some(Modal::Import(..))));
    f.types("Documents/check.flow.json");
    f.key(Key::Enter);
    assert!(f.port.st().flows.contains_key("imported"), "made from the file's text");
    assert_eq!(f.app.cur.as_deref(), Some("imported"));
}

#[test]
fn an_import_the_shell_refuses_shows_its_sentence_and_makes_nothing() {
    let mut f = Fx::new(port(), true);
    f.key(Key::Char('i'));
    f.types("/etc/passwd");
    f.key(Key::Enter);
    assert!(f.port.st().flows.is_empty());
    match &f.app.modal {
        Some(Modal::Import(form)) => assert_eq!(form.error.as_deref(), Some("that file is outside the folders you may read")),
        _ => panic!("the dialog stays open with the reason"),
    }
}

#[test]
fn dropped_files_are_taken_when_json_and_the_rest_is_ignored_with_a_reason() {
    let mut f = Fx::new(port(), true);
    f.shell.files.push(("/drop/a.json".into(), FLOW_JSON.into()));
    let a = f.app.on_drop(&["/drop/a.json".into()], &f.shell);
    assert_eq!(a, DropAction::Taken);
    assert!(f.port.st().flows.contains_key("imported"));
    let b = f.app.on_drop(&["/drop/pic.png".into()], &f.shell);
    assert_eq!(b, DropAction::Ignore("FLOWS imports flow files (.json).".into()));
}

#[test]
fn deleting_asks_first_and_y_deletes_through_the_port() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.key(Key::Char('d'));
    assert!(matches!(f.app.modal, Some(Modal::Confirm(..))));
    f.key(Key::Enter);
    assert_eq!(f.port.st().flows.len(), 1, "Enter keeps it");
    f.key(Key::Char('d'));
    f.key(Key::Char('y'));
    assert!(f.port.st().flows.is_empty());
    assert!(f.app.flows.is_empty(), "the list is read again");
}

#[test]
fn export_hands_the_flow_file_to_the_host_save_slot() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.key(Key::Enter);
    f.app.export_flow();
    let (name, bytes) = f.app.take_host_save().expect("a file to save");
    assert_eq!(name, "check.flow.json");
    assert!(String::from_utf8(bytes).unwrap().contains("Site check"));
    assert!(f.app.take_host_save().is_none(), "taken once");
}

#[test]
fn ctrl_i_opens_the_assistant_and_keys_go_to_it_until_ctrl_i_or_f6_takes_them_back() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.key(Key::Ctrl('i'));
    assert!(f.app.chat_open && f.app.focus == Focus::Chat);
    f.key(Key::Char('x'));
    assert_eq!(f.chat.lock().unwrap().events, 1, "the key reached the pane");
    assert_eq!(f.port.st().flows.len(), 1, "and not the list");
    f.key(Key::F(6));
    assert_eq!(f.app.focus, Focus::Canvas);
    f.key(Key::F(6));
    assert_eq!(f.app.focus, Focus::Chat);
    f.key(Key::Ctrl('i'));
    assert!(!f.app.chat_open);
}

fn failure() -> Failure {
    Failure { run_id: "run-9".into(), flow_id: "check".into(), flow_name: "Site check".into(), version: 2, trigger: "schedule".into(), step: "fetch".into(), step_title: "Fetch".into(), reason: "timeout".into(), log_tail: vec![], input_names: vec![], needs_person: false, choice: false }
}

#[test]
fn ask_the_agent_to_fix_opens_the_assistant_with_the_failure_and_ends_auto_approval() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.app.fail = Some(failure());
    f.app.start_fix(&f.shell);
    let log = f.chat.lock().unwrap();
    assert!(log.agent_mode, "the assistant's tools are on for the thread");
    assert_eq!(log.asked.len(), 1);
    assert!(log.asked[0].starts_with("FLOW FAILURE REQUEST (fix attempt 1"), "{}", log.asked[0]);
    drop(log);
    assert!(f.app.chat_open && f.app.focus == Focus::Chat);
    assert!(f.app.status.as_ref().is_some_and(|s| s.1 && s.0.contains("attempt 1")));
}

#[test]
fn the_fix_is_refused_when_the_account_has_no_assistant_tools_or_the_step_needs_the_person() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), false);
    f.app.fail = Some(failure());
    f.app.start_fix(&f.shell);
    assert!(f.chat.lock().unwrap().asked.is_empty());
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("not enabled")));
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    f.app.fail = Some(Failure { needs_person: true, ..failure() });
    f.app.start_fix(&f.shell);
    assert!(f.chat.lock().unwrap().asked.is_empty(), "a permission only the person can give");
}

#[test]
fn the_tile_names_no_file_and_the_port_is_asked_for_the_list_again_when_the_revision_moves() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    let before = f.app.run_rev;
    f.port.st().rev += 1;
    for _ in 0..6 {
        f.ev(Event::Tick);
    }
    assert_ne!(f.app.run_rev, before, "the tile read the runs again after the port's counter moved");
    assert_eq!(f.app.tab, Tab::Design);
}

#[test]
fn the_failure_book_the_shell_gave_bounds_the_attempts_and_the_fourth_ask_says_why() {
    let mut f = Fx::new(port().with_flow("check", "Site check"), true);
    for n in 1..=3 {
        f.app.fail = Some(failure());
        f.app.start_fix(&f.shell);
        assert_eq!(f.chat.lock().unwrap().asked.len(), n);
    }
    f.app.fail = Some(failure());
    f.app.start_fix(&f.shell);
    assert_eq!(f.chat.lock().unwrap().asked.len(), 3, "no fourth ask");
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("already tried")), "{:?}", f.app.status);
    assert_eq!(f.app.book.attempts_left("check"), 0);
    f.app.book.reset("check");
    assert_eq!(f.app.book.attempts_left("check"), 3, "a success starts the count again");
}
