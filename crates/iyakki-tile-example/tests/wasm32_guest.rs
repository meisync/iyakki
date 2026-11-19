//! The Rust counter built for wasm32 (src/guest.rs) and driven through the host runner, side by side with the hand-written WAT tile and the
//! native `Counter`: the same events must give the same pictures. The test builds the guest with the installed `wasm32-unknown-unknown`
//! target into its own target directory (offline, nothing is downloaded) and skips with a printed reason when the target is not installed.
use iyakki_tile_abi::{Color, Event};
use iyakki_tile_example::{Counter, TILE_WAT};
use iyakki_tile_host::{Budgets, TileManifest, TileRunner};
use std::path::PathBuf;
use std::process::Command;

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

fn target_installed() -> bool {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    sysroot.is_some_and(|s| PathBuf::from(s).join("lib/rustlib/wasm32-unknown-unknown/lib").is_dir())
}

/// The built `.wasm` (built once for both tests), or None (with the reason printed) when this machine cannot build it.
fn build() -> Option<Vec<u8>> {
    static BUILT: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    BUILT.get_or_init(build_once).clone()
}

fn build_once() -> Option<Vec<u8>> {
    if !target_installed() {
        eprintln!("skipped: the wasm32-unknown-unknown target is not installed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-tile-example-wasm-{}", std::process::id()));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "-p", "iyakki-tile-example"])
        .env("CARGO_TARGET_DIR", &dir)
        .env_remove("RUSTFLAGS")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo starts");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("wasm32-unknown-unknown/release/iyakki_tile_example.wasm")).expect("the wasm file");
    let _ = std::fs::remove_dir_all(&dir);
    Some(bytes)
}

fn runner(wasm: &[u8]) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new("counter", &[]), Budgets::default(), None, 7).expect("the host accepts the tile");
    r.init(320, 200, 100, THEME).unwrap();
    r
}

#[test]
fn the_rust_tile_built_for_wasm32_draws_what_the_wat_tile_and_the_native_counter_draw() {
    let Some(wasm) = build() else { return };
    let mut rust = runner(&wasm);
    let mut wat = runner(TILE_WAT.as_bytes());
    let mut native = Counter::new();
    assert_eq!(rust.frame().unwrap(), wat.frame().unwrap());
    assert_eq!(rust.frame().unwrap(), native.frame());
    // a click on the button, typing, a backspace, the field hit and a resize
    assert_eq!(rust.click(30, 80).unwrap(), Some(1));
    wat.click(30, 80).unwrap();
    native.event(&Event::Hit { id: 1 });
    let batch = [Event::TextInput { text: "hello".into() }, Event::Key { code: 8, mods: 0, down: true }];
    rust.event(&batch).unwrap();
    wat.event(&batch).unwrap();
    for e in &batch {
        native.event(e);
    }
    assert_eq!(rust.frame().unwrap(), native.frame());
    assert_eq!(rust.frame().unwrap(), wat.frame().unwrap());
    assert_eq!(rust.click(30, 130).unwrap(), Some(2));
    let resize = [Event::Resize { w: 640, h: 360, scale_pct: 200 }];
    assert!(rust.event(&resize).unwrap());
    native.event(&Event::Hit { id: 2 });
    native.event(&resize[0]);
    assert_eq!(rust.frame().unwrap(), native.frame());
}

#[test]
fn the_rust_tile_stays_inside_the_budgets_and_ignores_events_it_does_not_use() {
    let Some(wasm) = build() else { return };
    let mut r = runner(&wasm);
    for _ in 0..200 {
        r.click(30, 80).unwrap();
        let _ = r.frame().unwrap();
    }
    assert!(!r.event(&[Event::Tick { dt_ms: 16 }]).unwrap(), "a tick changes nothing");
    assert!(r.frame().unwrap().ops.len() >= 9);
}
