use super::*;

fn sdk() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-ui-sdk")
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-newapp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn names_are_checked_and_the_words_made_from_them_are_the_ones_the_templates_use() {
    for bad in ["", "a", "Notes", "9lives", "my_app", "my--app", "app-", "x".repeat(31).as_str(), "sample", "chat", "a b", "../x"] {
        assert!(names(bad).is_err(), "{bad:?} must be refused");
    }
    let n = names("my-notes").unwrap();
    assert_eq!((n.id.as_str(), n.title.as_str(), n.ty.as_str(), n.crate_name.as_str(), n.module.as_str()), ("my-notes", "MY NOTES", "MyNotesApp", "iyakki-app-my-notes", "iyakki_app_my_notes"));
}

#[test]
fn the_generated_crate_has_every_file_no_placeholder_left_and_depends_on_the_sdk_alone() {
    let d = scratch("files");
    let (root, written, in_repo) = generate("my-notes", Some(&d), Some(&sdk())).unwrap();
    assert!(!in_repo);
    assert_eq!(written, ["Cargo.toml", "app.toml", "src/lib.rs", "src/ports.rs", "tests/golden.rs", "tests/starter.rs", "README.md", "examples/preview.rs"]);
    for rel in &written {
        let t = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(!t.contains("{{"), "{rel} still has a placeholder");
        assert!(!t.contains("/home/") || rel == "Cargo.toml", "{rel}");
    }
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let deps: Vec<&str> = cargo.lines().skip_while(|l| *l != "[dependencies]").skip(1).take_while(|l| !l.starts_with('[') && !l.is_empty()).collect();
    assert_eq!(deps.len(), 1, "{deps:?}");
    assert!(deps[0].starts_with("iyakki-ui-sdk = { path = "), "{deps:?}");
    assert!(cargo.contains("[workspace]"), "outside the workspace it is its own project");
    let manifest = std::fs::read_to_string(root.join("app.toml")).unwrap();
    assert!(manifest.contains("id = \"my-notes\"") && manifest.contains("ports = [\"storage\"]") && manifest.contains("display_list = 1"));
    // a second run changes nothing and says so
    let before = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let e = generate("my-notes", Some(&d), Some(&sdk())).err().unwrap();
    assert!(e.contains("exists already"), "{e}");
    assert_eq!(std::fs::read_to_string(root.join("src/lib.rs")).unwrap(), before);
    // outside the workspace the SDK must be named
    assert!(generate("other", Some(&d), None).err().unwrap().contains("--sdk"));
    assert!(generate("other", Some(&d), Some(Path::new("/nonexistent"))).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn inside_the_workspace_the_crate_uses_the_workspace_package_fields_and_a_relative_sdk_path() {
    let d = scratch("repo");
    std::fs::create_dir_all(d.join("iyakki-ui-sdk")).unwrap();
    std::fs::write(d.join("iyakki-ui-sdk/Cargo.toml"), "[package]\nname = \"iyakki-ui-sdk\"\n").unwrap();
    let (root, _, in_repo) = generate("calc", Some(&d), None).unwrap();
    assert!(in_repo);
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("version.workspace = true") && cargo.contains("path = \"../iyakki-ui-sdk\"") && !cargo.contains("[workspace]"), "{cargo}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_command_prints_what_to_do_next_and_refuses_what_it_should() {
    assert_eq!(cli(&[]), 2);
    assert_eq!(cli(&["Bad Name".into()]), 1);
    let d = scratch("cli");
    let a = |n: &str| vec![n.to_string(), "--dir".into(), d.to_string_lossy().to_string(), "--sdk".into(), sdk().to_string_lossy().to_string()];
    assert_eq!(cli(&a("hello")), 0);
    assert!(d.join("iyakki-app-hello/src/lib.rs").is_file());
    assert_eq!(cli(&a("hello")), 1, "it does not overwrite");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn with_tile_the_crate_also_builds_as_a_wasm_tile_and_says_how() {
    let d = scratch("tile");
    let (root, written, _) = generate_with("tiled", Some(&d), Some(&sdk()), true).unwrap();
    assert_eq!(written, ["Cargo.toml", "app.toml", "src/lib.rs", "src/ports.rs", "tests/golden.rs", "tests/starter.rs", "src/tile.rs", "README.md", "examples/preview.rs"]);
    for rel in &written {
        assert!(!std::fs::read_to_string(root.join(rel)).unwrap().contains("{{"), "{rel} still has a placeholder");
    }
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("crate-type = [\"rlib\", \"cdylib\"]") && cargo.contains("iyakki-sdk-tile = { path = "), "{cargo}");
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("#[cfg(target_arch = \"wasm32\")]\nmod tile;"));
    assert!(std::fs::read_to_string(root.join("src/tile.rs")).unwrap().contains("export_sdk_tile!(crate::TiledApp::with_storage)"));
    let m = std::fs::read_to_string(root.join("app.toml")).unwrap();
    assert!(m.contains("tile_abi = 1") && m.contains("tile_caps = [\"storage\", \"dock\"]"));
    // without --tile there is none of it
    let (plain, written, _) = generate("plain", Some(&d), Some(&sdk())).unwrap();
    assert!(!written.iter().any(|w| w == "src/tile.rs") && !std::fs::read_to_string(plain.join("src/lib.rs")).unwrap().contains("mod tile"));
    assert!(!std::fs::read_to_string(plain.join("app.toml")).unwrap().contains("tile_abi"));
    // the sdk-tile crate must be there
    let lone = scratch("lone");
    std::fs::create_dir_all(lone.join("iyakki-ui-sdk")).unwrap();
    std::fs::write(lone.join("iyakki-ui-sdk/Cargo.toml"), "[package]\nname = \"iyakki-ui-sdk\"\n").unwrap();
    assert!(generate_with("x1", Some(&lone), None, true).err().unwrap().contains("iyakki-sdk-tile"));
    // the command says how to build the tile
    let a = vec!["cli-tile".to_string(), "--tile".into(), "--dir".into(), d.to_string_lossy().to_string(), "--sdk".into(), sdk().to_string_lossy().to_string()];
    assert_eq!(cli(&a), 0);
    assert!(d.join("iyakki-app-cli-tile/src/tile.rs").is_file());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&lone);
}

/// The real thing for a tile: a generated app made with `--tile` is built for wasm32 with cargo (offline), loaded by the real tile host
/// runner with only the capabilities its manifest names, drawn, tapped, and its count kept through the scripted core. Ignored by default.
#[test]
#[ignore = "builds a generated crate with cargo for wasm32"]
fn a_generated_tile_app_builds_for_wasm32_and_runs_in_the_tile_host() {
    use iyakki_tile_abi::{Color, Event, Op as TOp};
    use iyakki_tile_host::{Budgets, ServiceBackend, TileManifest, TileRunner};
    use std::sync::{Arc, Mutex};
    let sysroot = std::process::Command::new("rustc").args(["--print", "sysroot"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if !Path::new(&sysroot).join("lib/rustlib/wasm32-unknown-unknown/lib").is_dir() {
        eprintln!("skipped: the wasm32-unknown-unknown target is not installed");
        return;
    }
    let d = scratch("tilebuild");
    let (root, _, _) = generate_with("round-tile", Some(&d), Some(&sdk()), true).unwrap();
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"), root.join("Cargo.lock")).unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| root.join("target").to_string_lossy().to_string());
    let o = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--lib", "--offline", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTFLAGS")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let wasm = std::fs::read(Path::new(&target).join("wasm32-unknown-unknown/release/iyakki_app_round_tile.wasm")).expect("the tile");
    struct Core(Mutex<Vec<(String, String, String)>>);
    impl ServiceBackend for Core {
        fn call(&self, d: &str, m: &str, j: &str) -> Result<String, String> {
            self.0.lock().unwrap().push((d.into(), m.into(), j.into()));
            match (d, m) {
                ("storage", "get") => Ok("\"41\"".into()),
                ("storage", "set") => Ok("null".into()),
                ("dock", _) => Ok("[]".into()),
                _ => Err("no".into()),
            }
        }
    }
    let core = Arc::new(Core(Mutex::new(Vec::new())));
    let theme = Event::Theme { dark: false, accent: Color(0, 0, 0, 255), background: Color(255, 255, 255, 255), foreground: Color(0, 0, 0, 255) };
    let mut r = TileRunner::load(&wasm, TileManifest::new("round-tile", &["storage", "dock"]), Budgets::default(), Some(core.clone() as Arc<dyn ServiceBackend>), 3).expect("the host accepts the generated tile");
    r.init(360, 740, 100, theme).unwrap();
    let f = r.frame().unwrap();
    let texts = |l: &iyakki_tile_abi::DisplayList| l.ops.iter().filter_map(|o| if let TOp::Text { text, .. } = o { Some(text.clone()) } else { None }).collect::<Vec<_>>();
    assert!(texts(&f).iter().any(|t| t == "Pressed 41 times"), "the count came from the core: {:?}", texts(&f));
    let hit = f.ops.iter().find_map(|o| if let TOp::Hit { id, r } = o { Some((*id, r.x + r.w / 2, r.y + r.h / 2)) } else { None }).unwrap();
    r.click(hit.1, hit.2).unwrap();
    assert!(texts(&r.frame().unwrap()).iter().any(|t| t == "Pressed 42 times"));
    assert!(core.0.lock().unwrap().iter().any(|c| c.0 == "storage" && c.1 == "set" && c.2.contains("42")), "{:?}", core.0.lock().unwrap());
    let _ = std::fs::remove_dir_all(&d);
}

/// The real thing: the generated app is built and tested with cargo (offline, the shared target folder of this checkout). Slow, so it is
/// ignored by default; run it with `cargo test -p iyakki-agent --lib newapp -- --ignored`.
#[test]
#[ignore = "builds a generated crate with cargo"]
fn the_generated_app_compiles_writes_its_golden_files_and_passes_its_own_tests() {
    let d = scratch("build");
    let (root, _, _) = generate("round-trip", Some(&d), Some(&sdk())).unwrap();
    // the workspace lock file gives the standalone project the same versions, so nothing is fetched
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"), root.join("Cargo.lock")).unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| root.join("target").to_string_lossy().to_string());
    let run = |write: bool| {
        let mut c = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        c.args(["test", "--offline", "--manifest-path"]).arg(root.join("Cargo.toml")).env("CARGO_TARGET_DIR", &target);
        if write {
            c.env("IYAKKI_GOLDEN_WRITE", "1");
        }
        c.output().unwrap()
    };
    // the first run writes the golden files itself (and passes); the lint test is one of the four in golden.rs, the starter counter test is the one in starter.rs
    let o = run(false);
    let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success() && text.contains("4 passed") && text.contains("1 passed") && root.join("tests/golden/phone.txt").is_file(), "{text}");
    let o = run(true);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(root.join("tests/golden/phone.txt").is_file() && root.join("tests/golden/desktop.txt").is_file());
    let o = run(false);
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success() && text.contains("4 passed") && text.contains("1 passed"), "{text}\n{}", String::from_utf8_lossy(&o.stderr));
    // a change of the app's drawing is caught by the golden test
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap().replace("Pressed {} times", "Tapped {} times");
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    let o = run(false);
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(!o.status.success() && text.contains("frame changed"), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_sdk_path_is_relative_inside_or_beside_the_checkout_and_absolute_elsewhere() {
    let d = scratch("relpath");
    let sdk = d.join("co/crates/iyakki-ui-sdk");
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::write(sdk.join("Cargo.toml"), "[package]\nname = \"iyakki-ui-sdk\"\n").unwrap();
    let dep = |root: &Path| std::fs::read_to_string(root.join("Cargo.toml")).unwrap().lines().find(|l| l.starts_with("iyakki-ui-sdk")).unwrap().to_string();
    let (inside, _, _) = generate("one", Some(&d.join("co/apps")), Some(&sdk)).unwrap();
    assert!(dep(&inside).contains("path = \"../../crates/iyakki-ui-sdk\""), "{}", dep(&inside));
    let (beside, _, _) = generate("two", Some(&d), Some(&sdk)).unwrap();
    assert!(dep(&beside).contains("path = \"../co/crates/iyakki-ui-sdk\""), "{}", dep(&beside));
    // a project folder with its own repository, beside the checkout
    std::fs::create_dir_all(d.join("billing/.git")).unwrap();
    let (nested, _, _) = generate("four", Some(&d.join("billing")), Some(&sdk)).unwrap();
    assert!(dep(&nested).contains("path = \"../../co/crates/iyakki-ui-sdk\""), "{}", dep(&nested));
    let other = scratch("relpath-other");
    let (far, _, _) = generate("three", Some(&other.join("x/y")), Some(&sdk)).unwrap();
    assert!(dep(&far).contains(&format!("path = \"{}\"", sdk.canonicalize().unwrap().display())), "{}", dep(&far));
    assert_eq!(relative(Path::new("/a/b/c"), Path::new("/a/d/e")), "../../d/e");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&other);
}
