//! EDIT built as a real WASM tile (src/guest.rs, `--features tile`) and run in the real sandbox through the tile host: the tile declares tile ABI 3,
//! so the host sends it a pinch and the text grows (F-SDK-34, D-SDK-34-01 (a)); the same pinch is dropped before a tile that declared an older
//! ABI. The test builds the guest with the installed wasm32 target (offline) and says why it skipped when that target is not installed.
use iyakki_app_edit::EditApp;
use iyakki_sdk_tile::ports::{CoreDock, CoreFiles, CoreStorage, Shared};
use iyakki_sdk_tile::SdkTile;
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use iyakki_tile_kit::{App, Source};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

fn target_installed() -> bool {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    sysroot.is_some_and(|s| PathBuf::from(s).join("lib/rustlib/wasm32-unknown-unknown/lib").is_dir())
}

fn build_once() -> Option<Vec<u8>> {
    if !target_installed() {
        eprintln!("skipped: the wasm32-unknown-unknown target is not installed");
        return None;
    }
    let (dir, own) = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(d) => (PathBuf::from(d), false),
        None => (std::env::temp_dir().join(format!("iyakki-app-edit-wasm-{}", std::process::id())), true),
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "-p", "iyakki-app-edit", "--features", "tile"])
        .env("CARGO_TARGET_DIR", &dir)
        .env_remove("RUSTFLAGS")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo starts");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("wasm32-unknown-unknown/release/iyakki_app_edit.wasm")).expect("the wasm file");
    if own {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Some(bytes)
}

/// A core that has no files, no storage and nothing else: every call is refused, which the app must live with.
struct NoCore;
impl ServiceBackend for NoCore {
    fn call(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("no such call".into())
    }
}
impl Source for NoCore {
    fn call(&mut self, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("no such call".into())
    }
}

fn runner(wasm: &[u8]) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new("edit", &["storage", "dock", "home"]), Budgets::default(), Some(Arc::new(NoCore) as Arc<dyn ServiceBackend>), 7).expect("the host accepts the tile");
    r.init(360, 740, 100, THEME).unwrap();
    r
}

fn text_size(l: &DisplayList, word: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Text { text, size, .. } = o { (text == word).then_some(*size) } else { None }).unwrap_or_else(|| panic!("no text {word}"))
}

#[test]
fn edit_as_a_real_wasm_tile_declares_abi_3_and_a_pinch_makes_the_text_bigger() {
    let Some(wasm) = build_once() else { return };
    assert!(wasm.len() < 4_000_000, "the tile is {} bytes", wasm.len());
    let mut w = runner(&wasm);
    assert!(w.takes_app_events(), "the tile declares tile ABI 3 (it asks for pinches)");
    let before = text_size(&w.frame().unwrap(), "A-");
    w.event(&[Event::Pinch { scale_milli: 2000, x: 100, y: 100 }]).unwrap();
    let during = text_size(&w.frame().unwrap(), "A+");
    assert!(during > before, "the text grew while the fingers were apart: {before} -> {during}");
    w.event(&[Event::PinchEnd { scale_milli: 2000 }]).unwrap();
    let after = text_size(&w.frame().unwrap(), "A-");
    assert!(after > before, "and stays big: {before} -> {after}");
    // a pinch back brings it down again, from the size the last pinch ended at
    w.event(&[Event::Pinch { scale_milli: 500, x: 100, y: 100 }]).unwrap();
    assert!(text_size(&w.frame().unwrap(), "A-") < after);
}

#[test]
fn the_native_adapter_and_the_wasm_tile_answer_a_pinch_alike() {
    let Some(wasm) = build_once() else { return };
    let mut w = runner(&wasm);
    let door = Shared::new(NoCore);
    let mut n = SdkTile::new(EditApp::new(Box::new(CoreFiles::new(door.clone()))).with_storage(Box::new(CoreStorage::new(door.clone()))), Box::new(CoreDock::new(door)));
    n.event(&Event::Resize { w: 360, h: 740, scale_pct: 100 });
    n.event(&THEME);
    assert_eq!(w.frame().unwrap().ops.len() > 0, n.frame().ops.len() > 0);
    for e in [Event::Pinch { scale_milli: 1600, x: 50, y: 60 }, Event::PinchEnd { scale_milli: 1600 }] {
        w.event(&[e.clone()]).unwrap();
        n.event(&e);
    }
    assert_eq!(text_size(&w.frame().unwrap(), "A-"), text_size(&n.frame(), "A-"));
    assert_eq!(n.with_app(|a| a.zoom()), 1600);
}
