//! The Chat tile in the real wasm sandbox against a scripted service, beside the native tile.
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use iyakki_tile_kit::abi::{Color, DisplayList, Event, Op};
use iyakki_tile_kit::{App, Source};
use std::sync::{Arc, Mutex};

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

/// A scripted service that records every call.
struct Svc(Mutex<Vec<(String, String, String)>>, fn(&str, &str) -> Result<String, String>);

impl ServiceBackend for Svc {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.0.lock().unwrap().push((d.into(), m.into(), j.into()));
        (self.1)(d, m)
    }
}

struct Native(Arc<Svc>);
impl Source for Native {
    fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
        ServiceBackend::call(&*self.0, d, m, j)
    }
}

fn svc(f: fn(&str, &str) -> Result<String, String>) -> Arc<Svc> {
    Arc::new(Svc(Mutex::new(vec![]), f))
}

fn runner(wasm: &[u8], name: &str, caps: &[&str], s: &Arc<Svc>) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new(name, caps), Budgets::default(), Some(s.clone() as Arc<dyn ServiceBackend>), 7).expect("the host accepts the tile");
    r.init(360, 400, 100, THEME).unwrap();
    r
}

fn native_start(app: &mut dyn App) {
    app.event(&Event::Resize { w: 360, h: 400, scale_pct: 100 });
    app.event(&THEME);
}

use iyakki_tile_chat::{ChatTile, CAPS, TILE_WASM};

fn answer(d: &str, m: &str) -> Result<String, String> {
    match (d, m) {
        ("chat", "models") => Ok(r#"{"models":[{"provider":"local","model":"tiny"}]}"#.into()),
        ("chat", "send") => Ok(r#"{"id":"r7"}"#.into()),
        ("chat", "stop") => Ok(r#"{"ok":true}"#.into()),
        ("runs", "get") => Ok(r#"{"id":"r7","status":"completed","lines":["tool now ok: noon","answer: it is noon"]}"#.into()),
        _ => Err("no".into()),
    }
}

#[test]
fn a_message_goes_out_through_chat_send_and_the_lines_come_back_through_runs_get_like_the_native_tile() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "chat", &CAPS, &s);
    let ns = svc(answer);
    let mut native = ChatTile::new(Native(ns));
    native_start(&mut native);
    assert_eq!(r.frame().unwrap(), native.frame());
    let typed = [Event::TextInput { text: "what time is it".into() }];
    r.event(&typed).unwrap();
    native.event(&typed[0]);
    assert_eq!(r.frame().unwrap(), native.frame(), "typing");
    // a real click on Send (its hit region is the button at the bottom right)
    // choose the model on the model line (a real click), then Send
    assert_eq!(r.click(100, 46).unwrap(), Some(5));
    native.event(&Event::Hit { id: 5 });
    assert_eq!(r.frame().unwrap(), native.frame(), "after choosing the model");
    let hit = r.click(330, 400 - 30 - 4 + 10).unwrap();
    assert_eq!(hit, Some(1));
    native.event(&Event::Hit { id: 1 });
    assert_eq!(r.frame().unwrap(), native.frame(), "after Send");
    let sent = s.0.lock().unwrap().iter().find(|c| c.1 == "send").unwrap().2.clone();
    assert_eq!(sent, r#"{"model":"tiny","prompt":"what time is it","provider":"local"}"#, "the chosen model goes with the message");
    assert!(texts(&r.frame().unwrap()).contains(&"answer: it is noon".to_string()));
    let seen: Vec<(String, String)> = s.0.lock().unwrap().iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert_eq!(seen[0], ("chat".to_string(), "models".to_string()));
    assert!(seen.iter().any(|c| c.1 == "send") && seen.iter().all(|c| matches!((c.0.as_str(), c.1.as_str()), ("chat", "send" | "models" | "stop") | ("runs", "get"))), "{seen:?}");
}

#[test]
fn without_the_capabilities_a_send_is_refused_and_said() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "chat", &[], &s);
    r.event(&[Event::TextInput { text: "hello".into() }]).unwrap();
    r.frame().unwrap();
    r.click(330, 400 - 30 - 4 + 10).unwrap();
    assert!(s.0.lock().unwrap().is_empty());
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t.contains("not allowed")));
}
