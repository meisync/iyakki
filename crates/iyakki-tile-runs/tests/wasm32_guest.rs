//! The Runs tile built for wasm32 and driven through the host runner with a scripted `runs` service: the committed `tile.wasm` and a
//! fresh build must both draw what the native tile draws, ask only for `runs.list` and `runs.get`, and be refused anything else.
//! The fresh build uses the installed `wasm32-unknown-unknown` target offline and is skipped (with a printed reason) when it is missing.
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use iyakki_tile_runs::{RunsTile, Source, CAPS, TILE_WASM};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

struct Svc(Mutex<Vec<(String, String, String)>>);

impl ServiceBackend for Svc {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.0.lock().unwrap().push((d.into(), m.into(), j.into()));
        match (d, m) {
            ("runs", "list") => Ok(r#"{"runs":[{"id":"r1","status":"running","prompt":"build the thing"},{"id":"r0","status":"done","prompt":"say hello"}]}"#.into()),
            ("runs", "get") => Ok(r#"{"id":"r1","status":"running","lines":["you: build the thing","tool web_fetch ok: 12 bytes"]}"#.into()),
            _ => Err("no".into()),
        }
    }
}

struct Native(Arc<Svc>);
impl Source for Native {
    fn call(&mut self, m: &str, j: &str) -> Result<String, String> {
        ServiceBackend::call(&*self.0, "runs", m, j)
    }
}

fn target_installed() -> bool {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    sysroot.is_some_and(|s| PathBuf::from(s).join("lib/rustlib/wasm32-unknown-unknown/lib").is_dir())
}

fn fresh_build() -> Option<Vec<u8>> {
    if !target_installed() {
        eprintln!("skipped: the wasm32-unknown-unknown target is not installed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-tile-runs-wasm-{}", std::process::id()));
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "-p", "iyakki-tile-runs"])
        .env("CARGO_TARGET_DIR", &dir)
        .env_remove("RUSTFLAGS")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo starts");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("wasm32-unknown-unknown/release/iyakki_tile_runs.wasm")).expect("the wasm file");
    let _ = std::fs::remove_dir_all(&dir);
    Some(bytes)
}

fn runner(wasm: &[u8], svc: &Arc<Svc>) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new("runs", &CAPS), Budgets::default(), Some(svc.clone() as Arc<dyn ServiceBackend>), 7).expect("the host accepts the tile");
    r.init(360, 300, 100, THEME).unwrap();
    r
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn drive(wasm: &[u8]) {
    let svc = Arc::new(Svc(Mutex::new(vec![])));
    let mut r = runner(wasm, &svc);
    let native_svc = Arc::new(Svc(Mutex::new(vec![])));
    let mut native = RunsTile::new(Native(native_svc.clone()));
    native.event(&Event::Resize { w: 360, h: 300, scale_pct: 100 });
    native.event(&THEME);
    assert_eq!(r.frame().unwrap(), native.frame(), "the first picture");
    let x = texts(&r.frame().unwrap());
    assert!(x.contains(&"build the thing".to_string()) && x.contains(&"say hello".to_string()), "{x:?}");
    // open the first row with a real click, read its lines
    let row = r.click(50, 40 + 10).unwrap();
    assert_eq!(row, Some(100));
    native.event(&Event::Hit { id: 100 });
    assert_eq!(r.frame().unwrap(), native.frame(), "after opening a run");
    assert!(texts(&r.frame().unwrap()).contains(&"tool web_fetch ok: 12 bytes".to_string()));
    // a tick of three seconds reads it again
    r.event(&[Event::Tick { dt_ms: 3000 }]).unwrap();
    native.event(&Event::Tick { dt_ms: 3000 });
    assert_eq!(r.frame().unwrap(), native.frame());
    let seen: Vec<(String, String)> = svc.0.lock().unwrap().iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert!(seen.iter().all(|(d, m)| d == "runs" && (m == "list" || m == "get")), "{seen:?}");
    assert!(seen.iter().filter(|c| c.1 == "get").count() >= 2);
}

#[test]
fn the_committed_tile_wasm_draws_what_the_native_tile_draws_and_asks_only_for_runs() {
    drive(TILE_WASM);
}

#[test]
fn a_fresh_wasm32_build_does_too() {
    if let Some(w) = fresh_build() {
        drive(&w);
    }
}

#[test]
fn without_the_capabilities_every_call_is_refused_and_the_tile_says_so() {
    let svc = Arc::new(Svc(Mutex::new(vec![])));
    let mut r = TileRunner::load(TILE_WASM, TileManifest::new("runs", &[]), Budgets::default(), Some(svc.clone() as Arc<dyn ServiceBackend>), 7).unwrap();
    r.init(360, 300, 100, THEME).unwrap();
    assert!(svc.0.lock().unwrap().is_empty(), "nothing reached the service");
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t.contains("not allowed")));
}
