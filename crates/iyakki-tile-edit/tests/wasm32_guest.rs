//! The Edit tile in the real wasm sandbox against a scripted `files` service, beside the native tile: the same events give the same pictures,
//! a file is opened by name and a save is asked again until the scripted person has answered.
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use iyakki_tile_kit::abi::{Color, DisplayList, Event, Op};
use iyakki_tile_kit::{App, Source};
use std::sync::{Arc, Mutex};

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

/// A scripted service that records every call.
struct Svc(Mutex<Vec<(String, String, String)>>, fn(&str, &str, usize) -> Result<String, String>);

impl ServiceBackend for Svc {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        let mut l = self.0.lock().unwrap();
        let before = l.iter().filter(|c| c.0 == d && c.1 == m).count();
        l.push((d.into(), m.into(), j.into()));
        (self.1)(d, m, before)
    }
}

struct Native(Arc<Svc>);
impl Source for Native {
    fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
        ServiceBackend::call(&*self.0, d, m, j)
    }
}

fn svc(f: fn(&str, &str, usize) -> Result<String, String>) -> Arc<Svc> {
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


use iyakki_tile_edit::{EditTile, CAPS, TILE_WASM};

/// `files.read` answers a small file; `files.write` says "asked" twice and then "ok" (the person answered), as the service does.
fn answer(d: &str, m: &str, before: usize) -> Result<String, String> {
    match (d, m) {
        ("files", "read") => Ok(r#"{"ok":true,"text":"let a = 1;\nlet b = 2;\n"}"#.into()),
        ("files", "write") => {
            Ok(if before < 2 { r#"{"ok":false,"asked":true}"#.into() } else { r#"{"ok":true,"text":"wrote it"}"#.into() })
        }
        _ => Err("no".into()),
    }
}

#[test]
fn the_same_typing_gives_the_same_pictures_as_the_native_tile_and_a_file_is_opened_and_saved_after_the_answer() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "edit", &CAPS, &s);
    let ns = svc(answer);
    let mut native = EditTile::new(Native(ns));
    native_start(&mut native);
    assert_eq!(r.frame().unwrap(), native.frame());
    let script = vec![
        Event::Hit { id: 6 },
        Event::TextInput { text: "/home/ann/a.rs".into() },
        Event::Key { code: 13, mods: 0, down: true },
        Event::Hit { id: 1000 },
        Event::TextInput { text: "// edited".into() },
        Event::Key { code: 's' as u32, mods: 2, down: true },
        Event::Tick { dt_ms: 2000 },
        Event::Tick { dt_ms: 2000 },
        Event::Key { code: 'z' as u32, mods: 2, down: true },
    ];
    for e in &script {
        r.event(std::slice::from_ref(e)).unwrap();
        native.event(e);
        assert_eq!(r.frame().unwrap(), native.frame(), "after {e:?}");
    }
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t.starts_with("Saved /home/ann/a.rs")), "{:?}", texts(&r.frame().unwrap()));
    let seen: Vec<(String, String)> = s.0.lock().unwrap().iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert_eq!(seen.iter().filter(|c| c.1 == "write").count(), 3, "asked, asked again, saved");
    assert!(seen.iter().all(|c| c.0 == "files" && (c.1 == "read" || c.1 == "write")), "{seen:?}");
    // a real click on the Open button
    // (the button is found by its hit id: its place depends on the tile's width, KI-990)
    let f = r.frame().unwrap();
    let open = f.ops.iter().find_map(|o| if let Op::Hit { id: 4, r } = o { Some(*r) } else { None }).expect("the Open button is drawn");
    assert!(r.click(open.x + open.w / 2, open.y + open.h / 2).unwrap().is_some());
}

#[test]
fn without_the_capabilities_a_file_cannot_be_opened_and_the_tile_says_so() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "edit", &[], &s);
    r.event(&[Event::Hit { id: 6 }]).unwrap();
    r.frame().unwrap();
    r.event(&[Event::TextInput { text: "/a".into() }, Event::Key { code: 13, mods: 0, down: true }]).unwrap();
    assert!(s.0.lock().unwrap().is_empty());
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t.contains("not allowed")));
}
