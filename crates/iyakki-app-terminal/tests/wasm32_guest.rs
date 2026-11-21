//! CLI built as a real WASM tile (src/guest.rs, `--features tile`) and run in the real sandbox through the tile host: the tile declares tile ABI 3,
//! so the host sends it a pinch and a line typed in the tile runs through the core's terminal and the text grows (F-SDK-34, D-SDK-34-01 (a)); the same pinch is dropped before a tile that declared an older
//! ABI. The test builds the guest with the installed wasm32 target (offline) and says why it skipped when that target is not installed.
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

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
        None => (std::env::temp_dir().join(format!("iyakki-app-terminal-wasm-{}", std::process::id())), true),
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "-p", "iyakki-app-terminal", "--features", "tile"])
        .env("CARGO_TARGET_DIR", &dir)
        .env_remove("RUSTFLAGS")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo starts");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::fs::read(dir.join("wasm32-unknown-unknown/release/iyakki_app_terminal.wasm")).expect("the wasm file");
    if own {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Some(bytes)
}

/// A scripted core: a terminal that answers `pwd` and asks before `mkdir`, and a key-value store. It records every call.
#[derive(Default)]
struct Core {
    log: Mutex<Vec<String>>,
}

impl ServiceBackend for Core {
    fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("{d}.{m} {j}"));
        let a: serde_json::Value = serde_json::from_str(j).map_err(|_| "bad json".to_string())?;
        match (d, m) {
            ("terminal", "open") => Ok(r#"{"prompt":"iyakki> ","banner":"IYAKKI command line"}"#.into()),
            ("terminal", "run") if a["line"] == "pwd" => Ok(r#"{"text":"/home/ada"}"#.into()),
            ("terminal", "run") => Ok(r#"{"text":"","question":{"call":"q1","summary":"Make a folder?"}}"#.into()),
            ("terminal", "decide") => Ok(r#"{"text":"made"}"#.into()),
            ("terminal", "close") => Ok("{}".into()),
            ("storage", "get") => Ok("null".into()),
            ("storage", "set") => Ok("null".into()),
            ("dock", _) => Ok("{}".into()),
            _ => Err("no such call".into()),
        }
    }
}

fn runner(wasm: &[u8], core: &Arc<Core>) -> TileRunner {
    let mut r = TileRunner::load(wasm, TileManifest::new("cli", &["terminal", "storage", "dock"]), Budgets::default(), Some(core.clone() as Arc<dyn ServiceBackend>), 7).expect("the host accepts the tile");
    r.init(360, 740, 100, THEME).unwrap();
    r
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

fn size_of(l: &DisplayList, word: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Text { text, size, .. } = o { (text == word).then_some(*size) } else { None }).unwrap_or_else(|| panic!("no text {word}"))
}

#[test]
fn cli_as_a_real_wasm_tile_runs_a_line_through_the_core_asks_before_a_change_and_declares_abi_3() {
    let Some(wasm) = build_once() else { return };
    assert!(wasm.len() < 4_000_000, "the tile is {} bytes", wasm.len());
    let core = Arc::new(Core::default());
    let mut w = runner(&wasm, &core);
    assert!(w.takes_app_events(), "the tile declares tile ABI 3 (it asks for pinches)");
    w.event(&[Event::Tick { dt_ms: 100 }]).unwrap();
    assert!(core.log.lock().unwrap().iter().any(|l| l.starts_with("terminal.open")), "the first heartbeat opens the session");
    w.event(&[Event::TextInput { text: "pwd".into() }, Event::Key { code: 13, mods: 0, down: true }]).unwrap();
    assert!(texts(&w.frame().unwrap()).iter().any(|t| t.contains("/home/ada")), "{:?}", texts(&w.frame().unwrap()));
    // a line that changes something waits for the person: Allow is a control of the tile
    w.event(&[Event::TextInput { text: "mkdir x".into() }, Event::Key { code: 13, mods: 0, down: true }]).unwrap();
    let f = w.frame().unwrap();
    assert!(texts(&f).iter().any(|t| t.contains("Allow")), "{:?}", texts(&f));
    assert!(!core.log.lock().unwrap().iter().any(|l| l.starts_with("terminal.decide")), "nothing is decided before the person answers");
    w.event(&[Event::Key { code: 121, mods: 0, down: true }]).unwrap();
    assert!(core.log.lock().unwrap().iter().any(|l| l.starts_with("terminal.decide") && l.contains("\"approve\":true")), "{:?}", core.log.lock().unwrap());
    // a pinch makes the text bigger
    let before = size_of(&w.frame().unwrap(), "A+");
    w.event(&[Event::Pinch { scale_milli: 2000, x: 100, y: 100 }]).unwrap();
    assert!(size_of(&w.frame().unwrap(), "A+") > before);
    w.event(&[Event::PinchEnd { scale_milli: 2000 }]).unwrap();
    assert!(core.log.lock().unwrap().iter().any(|l| l.starts_with("storage.set") && l.contains("2000")), "the size is kept through the core");
}
