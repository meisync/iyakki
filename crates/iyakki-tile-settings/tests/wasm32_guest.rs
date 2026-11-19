//! The Settings tile in the real wasm sandbox against a scripted service, beside the native tile: same pictures, one kind of call.
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

use iyakki_tile_settings::{SettingsTile, CAPS, TILE_WASM};

fn answer(d: &str, m: &str) -> Result<String, String> {
    match (d, m) {
        ("settings", "get") => Ok(r#"{"user":"ann","role":"admin","admin":true,"models":[{"provider":"local","model":"tiny"}],"web":{"on":true,"port":8443,"status":"on, port 8443"},"tools":31}"#.into()),
        _ => Err("no".into()),
    }
}

#[test]
fn the_committed_wasm_draws_what_the_native_tile_draws_and_asks_only_for_settings_get() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "settings", &CAPS, &s);
    let ns = svc(answer);
    let mut native = SettingsTile::new(Native(ns));
    native_start(&mut native);
    assert_eq!(r.frame().unwrap(), native.frame());
    assert!(texts(&r.frame().unwrap()).contains(&"ann (admin)".to_string()));
    r.event(&[Event::Tick { dt_ms: 10_000 }]).unwrap();
    native.event(&Event::Tick { dt_ms: 10_000 });
    assert_eq!(r.frame().unwrap(), native.frame());
    assert!(s.0.lock().unwrap().iter().all(|c| c.0 == "settings" && c.1 == "get"));
    assert!(s.0.lock().unwrap().len() >= 2);
}

#[test]
fn without_the_capability_nothing_reaches_the_service() {
    let s = svc(answer);
    let mut r = runner(TILE_WASM, "settings", &[], &s);
    assert!(s.0.lock().unwrap().is_empty());
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t.contains("not allowed")));
}
