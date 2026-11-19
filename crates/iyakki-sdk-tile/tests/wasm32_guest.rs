//! The SAMPLE app built as a real WASM tile (src/guest.rs, wasm32-unknown-unknown) and run in the real sandbox through the tile host
//! runner, with a scripted core behind the tile's `call`: the same pictures as the native adapter, notes kept through `storage`, the
//! manifest's capability enforced by the host (a tile without `storage` reaches nothing and still works), the frame inside the budgets.
//! The test builds the guest with the installed wasm32 target (offline) and skips with a printed reason when it is not installed.
use iyakki_app_sample::SampleApp;
use iyakki_sdk_tile::ports::{CoreDock, CoreStorage, Shared};
use iyakki_sdk_tile::SdkTile;
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use iyakki_tile_kit::{App, Source};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const THEME: Event = Event::Theme { dark: false, accent: Color(40, 110, 220, 255), background: Color(245, 245, 245, 255), foreground: Color(20, 20, 20, 255) };

fn target_installed() -> bool {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    sysroot.is_some_and(|s| PathBuf::from(s).join("lib/rustlib/wasm32-unknown-unknown/lib").is_dir())
}

fn build() -> Option<Vec<u8>> {
    static BUILT: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    BUILT.get_or_init(build_once).clone()
}

fn build_once() -> Option<Vec<u8>> {
    if !target_installed() {
        eprintln!("skipped: the wasm32-unknown-unknown target is not installed");
        return None;
    }
    let (dir, own) = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(d) => (PathBuf::from(d), false),
        None => (std::env::temp_dir().join(format!("iyakki-sdk-tile-wasm-{}", std::process::id())), true),
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "-p", "iyakki-sdk-tile", "--features", "sample-guest"])
        .env("CARGO_TARGET_DIR", &dir)
        .env_remove("RUSTFLAGS")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo starts");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("wasm32-unknown-unknown/release/iyakki_sdk_tile.wasm")).expect("the wasm file");
    if own {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Some(bytes)
}

/// The core as the tile host sees it: `ServiceBackend`, recording every call that reaches it.
#[derive(Default)]
struct Core {
    store: Mutex<BTreeMap<String, String>>,
    calls: Mutex<Vec<(String, String)>>,
}

impl Core {
    fn answer(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push((d.into(), m.into()));
        let a: serde_json::Value = serde_json::from_str(j).map_err(|_| "bad json".to_string())?;
        let k = a["k"].as_str().unwrap_or("").to_string();
        match (d, m) {
            ("storage", "get") => Ok(self.store.lock().unwrap().get(&k).map_or("null".into(), |v| serde_json::to_string(v).unwrap())),
            ("storage", "set") => {
                self.store.lock().unwrap().insert(k, a["v"].as_str().unwrap_or("").to_string());
                Ok("null".into())
            }
            ("dock", "pinned") => Ok("[]".into()),
            ("dock", "status") => Ok("{}".into()),
            _ => Err("no such call".into()),
        }
    }
}

impl ServiceBackend for Core {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.answer(d, m, j)
    }
}

struct Native(Arc<Core>);
impl Source for Native {
    fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.0.answer(d, m, j)
    }
}

fn runner(wasm: &[u8], caps: &[&str], core: &Arc<Core>) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new("sample", caps), Budgets::default(), Some(core.clone() as Arc<dyn ServiceBackend>), 7).expect("the host accepts the tile");
    r.init(360, 740, 100, THEME).unwrap();
    r
}

fn native(core: &Arc<Core>) -> SdkTile<SampleApp> {
    let door = Shared::new(Native(core.clone()));
    let mut t = SdkTile::new(SampleApp::with_storage(Box::new(CoreStorage::new(door.clone()))), Box::new(CoreDock::new(door)));
    t.event(&Event::Resize { w: 360, h: 740, scale_pct: 100 });
    t.event(&THEME);
    t
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn centre(t: &SdkTile<SampleApp>, name: &str) -> (i32, i32) {
    let n = t.sdk_frame().a11y().into_iter().find(|n| n.name == name).unwrap_or_else(|| panic!("no control {name}"));
    ((n.rect.x + n.rect.w / 2.0) as i32, (n.rect.y + n.rect.h / 2.0) as i32)
}

#[test]
fn the_sample_app_as_a_real_wasm_tile_draws_like_the_native_adapter_keeps_its_notes_through_the_core_and_is_held_to_its_capabilities() {
    let Some(wasm) = build() else { return };
    // no import but the tile ABI's own
    assert!(wasm.len() < 1_500_000, "the tile is {} bytes", wasm.len());
    let core_w = Arc::new(Core::default());
    let core_n = Arc::new(Core::default());
    let mut w = runner(&wasm, &["storage", "dock"], &core_w);
    let mut n = native(&core_n);
    let fw = w.frame().unwrap();
    assert_eq!(fw, n.frame(), "the first frame is the same picture");
    assert!(texts(&fw).iter().any(|t| t == "Sample") && fw.ops.iter().filter(|o| matches!(o, Op::Hit { .. })).count() >= 5);
    let stats = w.last_frame().map(|_| ()).is_some();
    assert!(stats);

    // a real tap through the host (down, up, hit), typing, Enter
    let (x, y) = centre(&n, "New note");
    let id = w.click(x, y).unwrap();
    assert!(id.is_some(), "the field is a hit region");
    n.event(&Event::PointerDown { x, y, button: 0 });
    n.event(&Event::PointerUp { x, y, button: 0 });
    n.event(&Event::Hit { id: id.unwrap() });
    assert_eq!(w.frame().unwrap(), n.frame(), "after the tap");
    let batch = [Event::TextInput { text: "feed the cat".into() }, Event::Key { code: 13, mods: 0, down: true }];
    w.event(&batch).unwrap();
    for e in &batch {
        n.event(e);
    }
    let fw = w.frame().unwrap();
    assert_eq!(fw, n.frame(), "after typing a note");
    assert!(texts(&fw).iter().any(|t| t == "[ ] feed the cat"), "{:?}", texts(&fw));
    let kept = core_w.store.lock().unwrap().get("notes").cloned().unwrap_or_default();
    assert!(kept.contains("0 feed the cat"), "the core holds the note: {kept:?}");
    assert_eq!(kept, core_n.store.lock().unwrap().get("notes").cloned().unwrap_or_default(), "the wasm tile and the native adapter kept the same text");

    // a restart: a new tile instance reads the note back through storage.get
    let mut again = runner(&wasm, &["storage", "dock"], &core_w);
    assert!(texts(&again.frame().unwrap()).iter().any(|t| t == "[ ] feed the cat"));

    // a resize to the wide layout through the host, and a draw inside the budgets
    w.event(&[Event::Resize { w: 1000, h: 600, scale_pct: 100 }]).unwrap();
    let wide = w.frame().unwrap();
    assert!(texts(&wide).iter().any(|t| t == "This screen"), "the wide layout has its side panel");

    // a manifest without storage: the host refuses the calls before they reach the core, the tile still works
    let bare = Arc::new(Core::default());
    let mut r = runner(&wasm, &["dock"], &bare);
    let (x, y) = centre(&n, "New note");
    r.frame().unwrap();
    r.click(x, y).unwrap();
    r.event(&[Event::TextInput { text: "lost".into() }, Event::Key { code: 13, mods: 0, down: true }]).unwrap();
    let f = r.frame().unwrap();
    assert!(texts(&f).iter().any(|t| t == "[ ] lost"), "the note is in memory");
    assert!(!bare.calls.lock().unwrap().iter().any(|c| c.0 == "storage"), "no storage call reached the core: {:?}", bare.calls.lock().unwrap());
    assert!(bare.store.lock().unwrap().is_empty());
    // and a manifest with neither reaches nothing at all
    let none = Arc::new(Core::default());
    let mut r = runner(&wasm, &[], &none);
    r.frame().unwrap();
    assert!(none.calls.lock().unwrap().is_empty(), "{:?}", none.calls.lock().unwrap());
}
