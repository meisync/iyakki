// HIDDEN test of task T2. Copied to tests/hidden_t2.rs by grade.py.
use iyakki_app_bench_t2::{BenchT2App, ID};
use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::files::MemFiles;
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

fn host(app: BenchT2App) -> Host<BenchT2App> {
    Host::new(app, Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn has(l: &DisplayList, name: &str) -> bool {
    l.ops.iter().any(|o| matches!(o, Op::Hit(_, h) if h.name == name))
}
fn tap(h: &mut Host<BenchT2App>, name: &str) {
    let l = h.record();
    let (x, y) = l.ops.iter().find_map(|o| if let Op::Hit(r, hh) = o { (hh.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"));
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}
fn ticks(h: &mut Host<BenchT2App>, n: usize) {
    for _ in 0..n { h.event(&UiEvent::Tick); }
}
fn setup(script: Vec<Turn>) -> (Host<BenchT2App>, std::rc::Rc<std::cell::RefCell<iyakki_ui_sdk::assistant::ScriptedAssistant>>) {
    let mut f = MemFiles::new();
    f.put("a.txt", "hello world");
    let (port, handle) = SharedAssistant::new(script);
    let mut h = host(BenchT2App::with_ports(Box::new(f), Box::new(port)));
    h.event(&UiEvent::Tick);
    h.event(&UiEvent::Key(UiKey::Enter)); // open a.txt in the Files pane
    ticks(&mut h, 2);
    (h, handle)
}
fn say(h: &mut Host<BenchT2App>, text: &str) {
    tap(h, "Assistant");
    ticks(h, 2);
    tap(h, "Ask the assistant");
    h.event(&UiEvent::Text(text.into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    ticks(h, 6);
}

#[test]
fn the_assistant_is_told_which_file_is_open() {
    let (mut h, handle) = setup(vec![Turn::say("Ok.")]);
    say(&mut h, "what is open?");
    let seen = handle.borrow().seen.join("\n");
    assert!(seen.contains("a.txt") && seen.contains("hello world"), "the model must see the open file as context: {seen}");
}

#[test]
fn reading_the_open_file_asks_nothing_and_closing_it_waits_for_the_person() {
    let (mut h, _s) = setup(vec![Turn::call("", "read_open_file", "{}"), Turn::say("Read it.")]);
    say(&mut h, "read it");
    assert!(!has(&h.record(), "Allow"), "a read tool needs no card");
    let (mut h, _s) = setup(vec![Turn::call("", "close_file", "{}"), Turn::say("Closed.")]);
    say(&mut h, "close it");
    assert_eq!(h.app.opened().as_deref(), Some("a.txt"), "not closed before the person allows it");
    assert!(has(&h.record(), "Allow"));
    tap(&mut h, "Allow");
    ticks(&mut h, 6);
    assert_eq!(h.app.opened(), None);
    assert!(h.record().texts().contains(&"No file open"));
}

#[test]
fn deny_keeps_the_file_open() {
    let (mut h, _s) = setup(vec![Turn::call("", "close_file", "{}"), Turn::say("Not closed.")]);
    say(&mut h, "close it");
    tap(&mut h, "Deny");
    ticks(&mut h, 6);
    assert_eq!(h.app.opened().as_deref(), Some("a.txt"));
}

#[test]
fn a_refusing_files_port_is_shown_in_words_and_does_not_crash() {
    let mut h = host(BenchT2App::with_ports(Box::new(MemFiles::unavailable()), Box::new(iyakki_ui_sdk::assistant::NoAssistant)));
    ticks(&mut h, 3);
    let texts = h.record().texts().join(" ");
    assert!(!texts.is_empty());
    tap(&mut h, "Assistant");
    ticks(&mut h, 3);
}
