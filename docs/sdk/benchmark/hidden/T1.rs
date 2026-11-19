// HIDDEN test of task T1: never shown to the model. Copied to tests/hidden_t1.rs by grade.py.
use iyakki_app_bench_t1::{BenchT1App, ID};
use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::ports::{PortError, StoragePort};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Shared(Rc<RefCell<BTreeMap<String, String>>>);
impl StoragePort for Shared {
    fn get(&mut self, k: &str) -> Option<String> { self.0.borrow().get(k).cloned() }
    fn set(&mut self, k: &str, v: &str) -> Result<(), PortError> { self.0.borrow_mut().insert(k.into(), v.into()); Ok(()) }
    fn remove(&mut self, k: &str) -> Result<(), PortError> { self.0.borrow_mut().remove(k); Ok(()) }
    fn keys(&mut self) -> Vec<String> { self.0.borrow().keys().cloned().collect() }
}
struct Denying;
impl StoragePort for Denying {
    fn get(&mut self, _: &str) -> Option<String> { None }
    fn set(&mut self, _: &str, _: &str) -> Result<(), PortError> { Err(PortError::Denied) }
    fn remove(&mut self, _: &str) -> Result<(), PortError> { Ok(()) }
    fn keys(&mut self) -> Vec<String> { vec![] }
}

fn host(app: BenchT1App) -> Host<BenchT1App> {
    Host::new(app, Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn has(l: &DisplayList, name: &str) -> bool {
    l.ops.iter().any(|o| matches!(o, Op::Hit(_, h) if h.name == name))
}
fn tap(h: &mut Host<BenchT1App>, name: &str) {
    let l = h.record();
    let (x, y) = l.ops.iter().find_map(|o| if let Op::Hit(r, hh) = o { (hh.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"));
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}
fn ticks(h: &mut Host<BenchT1App>, n: usize) {
    for _ in 0..n { h.event(&UiEvent::Tick); }
}
fn say(h: &mut Host<BenchT1App>, text: &str) {
    tap(h, "Assistant");
    ticks(h, 2);
    tap(h, "Ask the assistant");
    h.event(&UiEvent::Text(text.into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    ticks(h, 6);
}

#[test]
fn state_survives_a_restart_through_the_port() {
    let s = Shared::default();
    let mut h = host(BenchT1App::with_storage(Box::new(s.clone())));
    tap(&mut h, "More");
    tap(&mut h, "Notes");
    tap(&mut h, "Note");
    h.event(&UiEvent::Text("alpha".into()));
    tap(&mut h, "Add");
    tap(&mut h, "Compact rows");
    let again = BenchT1App::with_storage(Box::new(s));
    assert_eq!(again.count(), 1);
    assert_eq!(again.notes(), vec!["alpha".to_string()]);
    assert!(again.compact());
}

#[test]
fn the_chat_tool_that_changes_the_switch_waits_for_the_person() {
    let s = Shared::default();
    let (port, _script) = SharedAssistant::new(vec![Turn::call("", "compact_set", "{\"on\":true}"), Turn::say("Done.")]);
    let mut h = host(BenchT1App::with_ports(Box::new(s.clone()), Box::new(port)));
    say(&mut h, "turn compact rows on");
    assert!(!h.app.compact(), "nothing changes before the person allows it");
    assert!(has(&h.record(), "Allow"), "the approval card shows Allow");
    tap(&mut h, "Allow");
    ticks(&mut h, 6);
    assert!(h.app.compact(), "after Allow the switch is on");
    assert_eq!(s.0.borrow().get("compact").map(String::as_str), Some("true"), "and it is kept");
}

#[test]
fn deny_leaves_the_switch_alone_and_the_read_tool_needs_no_card() {
    let (port, _s) = SharedAssistant::new(vec![Turn::call("", "compact_set", "{\"on\":true}"), Turn::say("Not done.")]);
    let mut h = host(BenchT1App::with_ports(Box::new(Shared::default()), Box::new(port)));
    say(&mut h, "turn compact rows on");
    tap(&mut h, "Deny");
    ticks(&mut h, 6);
    assert!(!h.app.compact());
    let (port, _s) = SharedAssistant::new(vec![Turn::call("", "compact_status", "{}"), Turn::say("It is off.")]);
    let mut h = host(BenchT1App::with_ports(Box::new(Shared::default()), Box::new(port)));
    say(&mut h, "are rows compact");
    assert!(!has(&h.record(), "Allow"), "a read asks nothing");
}

#[test]
fn the_screen_switch_and_the_tool_share_one_state() {
    let (port, _s) = SharedAssistant::new(vec![Turn::call("", "compact_set", "{\"on\":false}"), Turn::say("Done.")]);
    let mut h = host(BenchT1App::with_ports(Box::new(Shared::default()), Box::new(port)));
    tap(&mut h, "Notes");
    tap(&mut h, "Compact rows");
    assert!(h.app.compact());
    say(&mut h, "turn compact rows off");
    tap(&mut h, "Allow");
    ticks(&mut h, 6);
    assert!(!h.app.compact());
}

#[test]
fn limits_and_refusals_are_handled_without_a_crash() {
    let mut h = host(BenchT1App::new());
    tap(&mut h, "Notes");
    for i in 0..101 {
        tap(&mut h, "Note");
        h.event(&UiEvent::Text(format!("n{i}")));
        tap(&mut h, "Add");
    }
    assert_eq!(h.app.notes().len(), 100, "at most 100 notes");
    let mut d = host(BenchT1App::with_storage(Box::new(Denying)));
    tap(&mut d, "More");
    tap(&mut d, "Notes");
    let _ = d.record();
}
