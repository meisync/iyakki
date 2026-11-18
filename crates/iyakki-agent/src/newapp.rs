//! `iyakki new-app NAME [--dir DIR] [--sdk PATH] [--tile]`: makes a new app crate written against the UI SDK (docs/development/UI-SDK.md): the crate,
//! its manifest (`app.toml`), a storage port stub, and a golden test. It only writes the files of the new folder; it never overwrites a
//! folder that exists, never runs a program, and the generated crate depends on `iyakki-ui-sdk` and nothing else from IYAKKI (its own test
//! scans for that, for colour literals and for machine access).
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage: iyakki new-app NAME [--dir DIR] [--sdk PATH] [--tile]
  NAME   lower-case letters, digits and '-', 2 to 30 characters, starting with a letter (the crate is iyakki-app-NAME)
  --dir  the folder the crate folder is made in (default: ./crates when it holds iyakki-ui-sdk, else the current folder)
  --sdk  the folder of the iyakki-ui-sdk crate; needed outside the IYAKKI workspace, where the crate is made as its own project. The path in Cargo.toml is relative when the new crate is inside the checkout of the SDK or beside it, absolute otherwise (edit it for a build host: docs/sdk/troubleshooting.md)
  --tile also make the app build as a WASM tile (src/tile.rs, iyakki-sdk-tile next to the SDK): the storage and dock ports then go through the core";

/// The names a new app may not take: ids the system already uses.
const RESERVED: [&str; 8] = ["sample", "settings", "store", "chat", "home", "shell", "core", "system"];

/// Why a name is refused, or the words made from it.
pub struct Names {
    pub id: String,
    pub title: String,
    pub ty: String,
    pub crate_name: String,
    pub module: String,
}

pub fn names(name: &str) -> Result<Names, String> {
    let ok = (2..=30).contains(&name.len()) && name.starts_with(|c: char| c.is_ascii_lowercase()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !name.ends_with('-') && !name.contains("--");
    if !ok {
        return Err(format!("{name:?} is not a good app name: use lower-case letters, digits and single '-', 2 to 30 characters, starting with a letter"));
    }
    if RESERVED.contains(&name) {
        return Err(format!("{name:?} is the id of an app that already exists; pick another name"));
    }
    let words: Vec<&str> = name.split('-').collect();
    let ty: String = words.iter().map(|w| w[..1].to_ascii_uppercase() + &w[1..]).collect::<String>() + "App";
    Ok(Names { id: name.to_string(), title: words.join(" ").to_ascii_uppercase(), ty, crate_name: format!("iyakki-app-{name}"), module: format!("iyakki_app_{}", name.replace('-', "_")) })
}

fn fill(t: &str, n: &Names) -> String {
    t.replace("{{ID}}", &n.id).replace("{{TITLE}}", &n.title).replace("{{TYPE}}", &n.ty).replace("{{CRATE}}", &n.crate_name).replace("{{MODULE}}", &n.module)
}

const MANIFEST: &str = r#"# The manifest of the app (read by the store and the shell; the install card is made from it).
id = "{{ID}}"
title = "{{TITLE}}"
version = "0.1.0"
# the SDK and the display list version this app is written against (additive changes only; a newer list is refused by name)
ui_sdk = 1
display_list = 1
# the smallest pane the app lays out for (logical units); below it the host shows the app scaled, never cut
min_width = 280
min_height = 320
# the ports the app asks for. The app never touches a file, a socket or a device itself: the core decides each call, asks the person where a
# call changes something, and writes it to the log. An empty list means the app asks for nothing.
ports = ["storage"]
"#;

const PORTS: &str = r#"//! The ports of {{TITLE}}: what the app may ask the machine for. The app never reaches a file itself (the core rule): it is given a port, and
//! the core checks the permission, asks the person where a call changes something and writes the log. The SDK's storage port is used as it
//! is; the shell adapter and the WASM tile give the app the core-backed one, and a test or the browser preview gets [`MemStorage`], which
//! forgets everything at exit.

pub use iyakki_ui_sdk::ports::{MemStorage, PortError, StoragePort};

/// What to tell the person when the port refused.
pub fn sentence(e: PortError) -> String {
    match e {
        PortError::BadKey => "that name is not allowed".into(),
        PortError::TooBig => "that is too much to keep".into(),
        PortError::Denied => "the core did not allow that".into(),
    }
}
"#;

/// `src/tile.rs` of an app made with `--tile`: the wasm32 build as a WASM tile (tile ABI 1) with the core's storage and dock.
const TILE: &str = r#"//! {{TITLE}} as a WASM tile: `cargo build --release --lib --target wasm32-unknown-unknown` gives `target/wasm32-unknown-unknown/release/{{MODULE}}.wasm`,
//! the six exports of tile ABI 1 over this app. Its storage and its dock go through the tile's one door (`call`); the host checks the
//! manifest's `ports` before anything is forwarded to the core, so an app that did not declare `storage` in app.toml keeps nothing.

iyakki_sdk_tile::export_sdk_tile!(crate::{{TYPE}}::with_storage);
"#;

const LIB: &str = r#"//! {{TITLE}}: a new app written against the IYAKKI UI SDK (docs/development/UI-SDK.md). It draws only through `Canvas`, takes its colours
//! from the Look, lays itself out from the pane's size class and reaches the machine only through the ports in [`ports`].

use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::desktop::DockStatus;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

pub mod ports;
use ports::{MemStorage, StoragePort};
{{TILEMOD}}

/// The app id the dock, the launcher and the policy use (the same as `id` in app.toml).
pub const ID: &str = "{{ID}}";
const A_MORE: u32 = 1;
const A_RESET: u32 = 2;
const COUNT_KEY: &str = "count";

pub struct {{TYPE}} {
    count: u32,
    note: String,
    storage: Box<dyn StoragePort>,
}

impl Default for {{TYPE}} {
    fn default() -> Self {
        {{TYPE}}::new()
    }
}

impl {{TYPE}} {
    pub fn new() -> {{TYPE}} {
        {{TYPE}}::with_storage(Box::new(MemStorage::default()))
    }

    /// The app over the storage the host gives it.
    pub fn with_storage(mut storage: Box<dyn StoragePort>) -> {{TYPE}} {
        let count = storage.get(COUNT_KEY).and_then(|s| s.parse().ok()).unwrap_or(0);
        {{TYPE}} { count, note: String::new(), storage }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    fn set(&mut self, n: u32) {
        self.count = n;
        self.note = match self.storage.set(COUNT_KEY, &n.to_string()) {
            Ok(()) => String::new(),
            Err(e) => ports::sentence(e),
        };
    }
}

impl UiApp for {{TYPE}} {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> String {
        "{{TITLE}}".into()
    }

    fn dock_status(&self) -> Option<DockStatus> {
        Some(DockStatus::new(if self.count > 0 { Some(self.count) } else { None }, &format!("{} so far", self.count)))
    }

    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(A_MORE) | UiEvent::Key(UiKey::Enter) | UiEvent::Key(UiKey::Char(' ')) => self.set(self.count.saturating_add(1)),
            UiEvent::Action(A_RESET) | UiEvent::Key(UiKey::Backspace) => self.set(0),
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }

    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        let m = cx.metrics();
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let width = (w - 2.0 * g).max(0.0);
        // sizes and gaps are steps of the skin's scales (TypeRole, Space), never numbers of your own: the skin and the person's text size apply
        let ts = m.size(TypeRole::Heading);
        let body = m.size(TypeRole::Large);
        c.text(g, g, ts, th.text, "{{TITLE}}", TextStyle::Bold);
        let line = fit(c, body, &format!("Pressed {} times", self.count), TextStyle::Regular, width);
        c.text(g, g + ts * 1.5, body, th.text, &line, TextStyle::Regular);
        let t = m.min_target();
        let y = g + ts * 1.5 + body * 1.8;
        let gap = m.gap(Space::Md);
        let bw = ((width - gap) / 2.0).max(0.0);
        widgets::button(c, th, &m, Rect::new(g, y, bw, t), "More", A_MORE, true);
        widgets::button(c, th, &m, Rect::new(g + bw + gap, y, bw, t), "Reset", A_RESET, false);
        if !self.note.is_empty() {
            let small = m.size(TypeRole::Body);
            let note = fit(c, small, &self.note, TextStyle::Regular, width);
            c.text(g, y + t + gap, small, th.muted, &note, TextStyle::Regular);
        }
    }
}
"#;

/// `examples/preview.rs`: the app in a browser tab on 127.0.0.1 (`cargo run --example preview`).
const PREVIEW: &str = r#"//! {{TITLE}} in a browser tab: `cargo run --example preview` prints an address on 127.0.0.1; open it. The page draws the app's display list
//! and sends clicks, keys and text back. The app keeps its notes in memory here (a real device gives it the core's ports).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use {{MODULE}}::{{{TYPE}}, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::wire::Session;

fn main() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("cannot listen on 127.0.0.1");
    println!("http://127.0.0.1:{}/", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    let host = Host::new({{TYPE}}::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    if let Err(e) = iyakki_ui_preview::serve(listener, Session::new(host), Arc::new(AtomicBool::new(false))) {
        eprintln!("{e}");
    }
}
"#;

/// `README.md` of the new app: the commands, in the order a new person needs them.
const README: &str = r#"# {{TITLE}}

An IYAKKI app written against the UI SDK (`iyakki-ui-sdk`). It depends on that crate and nothing else from IYAKKI.

| Do this | With |
|---|---|
| Run the tests (the first run writes the golden files in `tests/golden`: read and commit them) | `cargo test` |
| Change the look on purpose, then accept the new golden files | delete `tests/golden` and run `cargo test`, or `IYAKKI_GOLDEN_WRITE=1 cargo test` |
| See it in a browser tab | `cargo run --example preview`, then open the address it prints |
| Build it as a WASM tile (only if made with `--tile`) | `cargo build --release --lib --target wasm32-unknown-unknown` |

Rules the tests keep: draw only through `Canvas`; colours from `cx.colors()`; sizes, gaps and radii from `cx.metrics()` (`m.size`, `m.gap`, `m.radius`, `m.line`), never a number of your own; every control has a name; every target is at least `m.min_target()`; the machine is reached through ports only.
Guide: docs/sdk/README.md in the IYAKKI repository.
"#;

/// `tests/starter.rs`: the behaviour of the STARTER counter only. Delete or rewrite this file when you change the app.
const STARTER: &str = r#"//! Tests of the STARTER counter of {{TITLE}} (a tap on More, Enter, Backspace, Esc and the count kept through `with_storage`).
//! They describe the app `iyakki new-app` made, not your app: when you change the app, rewrite this file for what it does or delete it.
//! (`app-check` points here when one of these fails.) The checks in golden.rs (names, bounds, touch size, source rules) are the ones to keep.

use {{MODULE}}::ports::{MemStorage, StoragePort};
use {{MODULE}}::{{{TYPE}}, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

fn host(w: u32, h: u32) -> Host<{{TYPE}}> {
    Host::new({{TYPE}}::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}

#[test]
fn a_tap_a_key_and_the_storage_port_work() {
    let mut h = host(360, 740);
    let l = h.record();
    let (x, y) = centre(&l, "More");
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
    assert_eq!(h.app.count(), 1, "a tap on More");
    h.event(&UiEvent::Key(UiKey::Enter));
    assert_eq!(h.app.count(), 2, "Enter");
    assert!(h.record().texts().contains(&"Pressed 2 times"));
    h.event(&UiEvent::Key(UiKey::Backspace));
    assert_eq!(h.app.count(), 0, "Backspace resets");
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), iyakki_ui_sdk::app::Flow::Back);
    // the count comes back from the port, and a refusal of the port is shown, not hidden
    let mut s = MemStorage::default();
    s.set("count", "41").unwrap();
    let mut app = {{TYPE}}::with_storage(Box::new(s));
    assert_eq!(app.count(), 41);
    let mut h = Host::new(std::mem::take(&mut app), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    h.event(&UiEvent::Key(UiKey::Enter));
    assert_eq!(h.app.count(), 42);
}
"#;

const TEST: &str = r#"//! The golden test of {{TITLE}}, and the checks every SDK app should keep: names, bounds, touch targets, input, the source rules.
//! The golden files hold the display list of the first frame at three sizes. After a change you meant to make, delete tests/golden
//! and run `cargo test` once (or `IYAKKI_GOLDEN_WRITE=1 cargo test`; with a remote cargo wrapper `./run.sh app-check APP --update-golden`), then read the diff before committing it.

use std::path::Path;

use {{MODULE}}::{{{TYPE}}, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::look::Look;

const SIZES: [(&str, u32, u32); 3] = [("phone", 360, 740), ("tablet", 800, 480), ("desktop", 1280, 720)];

fn host(w: u32, h: u32) -> Host<{{TYPE}}> {
    Host::new({{TYPE}}::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

#[test]
fn the_first_frame_matches_its_golden_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for (name, w, h) in SIZES {
        let got = host(w, h).record().to_text();
        let file = dir.join(format!("{name}.txt"));
        // `app-check --update-golden` puts the marker file tests/golden/.dump next to the sources (a remote cargo wrapper copies it with them) and
        // reads the frames from this output, so the golden files come back even when cargo ran on another machine
        if dir.join(".dump").exists() {
            println!("GOLDEN-BEGIN {name}\n{got}\nGOLDEN-END {name}");
            continue;
        }
        if std::env::var("IYAKKI_GOLDEN_WRITE").is_ok_and(|v| v == "1") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, &got).unwrap();
            continue;
        }
        let Ok(want) = std::fs::read_to_string(&file) else {
            // the first run makes the golden file; read it and commit it (a later run compares against it)
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, &got).unwrap();
            eprintln!("wrote {}: commit it. If cargo runs on another machine (a build wrapper) the file stays there: bring it back with `./run.sh app-check APP --update-golden`", file.display());
            continue;
        };
        assert_eq!(got, want, "the {name} frame changed; if you meant it, delete tests/golden, run `cargo test` and read the diff");
    }
}

#[test]
fn every_control_has_a_name_sits_inside_the_pane_and_is_big_enough_to_touch_at_every_size_and_text_size() {
    for (_, w, h) in SIZES {
        for scale in [1.0, 3.0] {
            let mut hst = host(w, h);
            hst.touch = true;
            hst.text_scale = scale;
            let l = hst.record();
            let nodes = l.a11y();
            assert!(nodes.len() >= 2, "{w}x{h}: the controls are named");
            for n in nodes {
                assert!(!n.name.is_empty(), "WHAT: a control has no name (at {w}x{h}, text x{scale}).\nFIX: a screen reader, a browser client and these tests find a control by its name. Give the widget a label (`widgets::button(c, th, &m, r, \"More\", A_MORE, true)`) or a non-empty `name` in the `Hit` you pass to `Canvas::hit`. Icon-only buttons need a name too.");
                assert!(n.rect.x >= 0.0 && n.rect.y >= 0.0 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= h as f32 + 0.5, "WHAT: `{}` leaves the pane at {w}x{h}, text x{scale}.\nFIX: lay out from `c.size()` and `m.gutter()`, not from constants; shorten text with `fit(c, size, text, style, max_w)`; at text 3x let content scroll or wrap. Before: `Rect::new(g, y, 400.0, h)`. After: `Rect::new(g, y, w - 2.0 * g, t)`.", n.name);
                assert!(n.rect.h >= 44.0 - 0.5, "WHAT: `{}` is under 44 units high on touch (at {w}x{h}, text x{scale}).\nFIX: use the widgets (they enforce it) or make the rectangle at least `m.min_target()` high. Before: `Rect::new(x, y, w, 30.0)`. After: `Rect::new(x, y, w, m.min_target())`.", n.name);
            }
        }
    }
}

#[test]
fn no_styling_numbers_in_the_drawing_code() {
    // a number of 3.0 or more in drawing code is almost always a size, gap or radius: ask the skin (m.size, m.gap, m.radius, m.line) instead
    for file in ["src/lib.rs", "src/ports.rs"] {
        let code = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        let bad = iyakki_ui_sdk::lint::style_literals(&code);
        assert!(bad.is_empty(), "{file} has styling numbers; use m.size(TypeRole::..), m.gap(Space::..), m.radius(Radius::..), m.line(Line::..):\n{}", bad.join("\n"));
    }
}

#[test]
fn the_source_keeps_the_sdk_rules() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in ["src/lib.rs", "src/ports.rs", "src/tile.rs"] {
        let Ok(code) = std::fs::read_to_string(dir.join(file)) else { continue };
        for bad in ["rgb(", "rgba(", "std::fs", "std::net", "std::process", "iyakki_uikit", "iyakki_agent", "iyakki_shell", "\"/proc", "\"/sys", "\"/dev"] {
            assert!(!code.contains(bad), "WHAT: {file} contains `{bad}`.\nFIX: colours come from `cx.colors()` (not `rgb(`); files, network and processes go through a port the host hands in (not `std::fs`, `std::net`, `std::process`, `/proc`, `/sys`, `/dev`); an app never names `iyakki_uikit`, `iyakki_agent` or `iyakki_shell`. Before: `std::fs::read_to_string(p)`. After: `self.files.read_text(p, MAX)` on a `FilesPort`.");
        }
    }
    let m = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let mut in_deps = false;
    for l in m.lines() {
        if l.starts_with('[') {
            in_deps = l.trim() == "[dependencies]";
        } else if in_deps && l.starts_with("iyakki-") {
            assert!(l.starts_with("iyakki-ui-sdk") || l.starts_with("iyakki-sdk-tile"), "WHAT: Cargo.toml [dependencies] has `{l}`.\nFIX: an app depends on `iyakki-ui-sdk` (and `iyakki-sdk-tile` for a tile) and no other IYAKKI crate. Delete the line; the SDK re-exports `look`, `theme` and `wallpaper`. If you need more from the machine, that is a new port: stop and ask.");
        }
    }
    let manifest = std::fs::read_to_string(dir.join("app.toml")).unwrap();
    assert!(manifest.contains(&format!("id = \"{ID}\"")), "WHAT: app.toml `id` differs from `ID` in src/lib.rs.\nFIX: make both the same text (`id = \"{ID}\"`).");
}
"#;

/// The files of a new app: path inside the crate folder and text. `in_repo` uses the workspace's version, edition and licence. `tile` is the
/// path of the `iyakki-sdk-tile` crate when the app is also to build as a WASM tile (`--tile`).
pub fn files(n: &Names, sdk_path: &str, in_repo: bool, tile: Option<&str>, preview: Option<&str>) -> Vec<(String, String)> {
    let head = if in_repo { "version.workspace = true\nedition.workspace = true\nlicense.workspace = true\n".to_string() } else { "version = \"0.1.0\"\nedition = \"2021\"\nlicense = \"Apache-2.0\"\n".to_string() };
    let tail = if in_repo { "" } else { "\n[workspace]\n" };
    let lib_section = if tile.is_some() { "\n[lib]\n# cdylib: the wasm32 build is the WASM tile (src/tile.rs); on the host the crate is an ordinary library for the tests.\ncrate-type = [\"rlib\", \"cdylib\"]\n" } else { "" };
    let tile_dep = tile.map(|p| format!("iyakki-sdk-tile = {{ path = \"{p}\" }}\n")).unwrap_or_default();
    let dev = preview.map(|p| format!("\n[dev-dependencies]\n# only for `cargo run --example preview` (the app in a browser tab); the app itself does not use it\niyakki-ui-preview = {{ path = \"{p}\" }}\n")).unwrap_or_default();
    let cargo = format!("[package]\nname = \"{}\"\n{head}{lib_section}\n# An app written with the UI SDK (docs/development/UI-SDK.md): it depends on iyakki-ui-sdk and nothing else from IYAKKI{}.\n\n[dependencies]\niyakki-ui-sdk = {{ path = \"{sdk_path}\" }}\n{tile_dep}{dev}{tail}", n.crate_name, if tile.is_some() { " but iyakki-sdk-tile, which builds it as a WASM tile" } else { "" });
    let tilemod = if tile.is_some() { "\n#[cfg(target_arch = \"wasm32\")]\nmod tile;\n" } else { "" };
    let mut manifest = fill(MANIFEST, n);
    if tile.is_some() {
        manifest.push_str("# built as a WASM tile too: the tile ABI version and the capabilities the tile host checks before anything reaches the core\ntile_abi = 1\ntile_caps = [\"storage\", \"dock\"]\n");
    }
    let mut v = vec![
        ("Cargo.toml".into(), cargo),
        ("app.toml".into(), manifest),
        ("src/lib.rs".into(), fill(LIB, n).replace("{{TILEMOD}}", tilemod)),
        ("src/ports.rs".into(), fill(PORTS, n)),
        ("tests/golden.rs".into(), fill(TEST, n)),
        ("tests/starter.rs".into(), fill(STARTER, n)),
    ];
    if tile.is_some() {
        v.push(("src/tile.rs".into(), fill(TILE, n)));
    }
    v.push(("README.md".into(), fill(README, n)));
    if preview.is_some() {
        v.push(("examples/preview.rs".into(), fill(PREVIEW, n)));
    }
    v
}

/// `to` as a path relative to the folder `from` (both absolute, lexically clean).
pub fn relative(from: &Path, to: &Path) -> String {
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(t.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".into(); f.len() - common];
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// An absolute path without `.` and `..` (the folder need not exist).
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Make the crate. Returns the folder and the files written; refuses a folder that exists.
pub fn generate(name: &str, dir: Option<&Path>, sdk: Option<&Path>) -> Result<(PathBuf, Vec<String>, bool), String> {
    generate_with(name, dir, sdk, false)
}

/// As [`generate`]; with `tile` the app also builds as a WASM tile (`src/tile.rs`, `iyakki-sdk-tile` as a second dependency).
pub fn generate_with(name: &str, dir: Option<&Path>, sdk: Option<&Path>, tile: bool) -> Result<(PathBuf, Vec<String>, bool), String> {
    let n = names(name)?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let default_in_repo = cwd.join("crates/iyakki-ui-sdk/Cargo.toml").is_file();
    let base: PathBuf = match dir {
        Some(d) => d.to_path_buf(),
        None if default_in_repo => cwd.join("crates"),
        None => cwd.clone(),
    };
    let in_repo = sdk.is_none() && base.join("iyakki-ui-sdk/Cargo.toml").is_file();
    let sdk_path = match (sdk, in_repo) {
        (Some(p), _) => p.canonicalize().map_err(|_| format!("{}: there is no iyakki-ui-sdk crate there", p.display()))?.to_string_lossy().to_string(),
        (None, true) => "../iyakki-ui-sdk".to_string(),
        (None, false) => return Err("outside the IYAKKI workspace give the folder of the iyakki-ui-sdk crate with --sdk PATH".into()),
    };
    if sdk.is_some() && !Path::new(&sdk_path).join("Cargo.toml").is_file() {
        return Err(format!("{sdk_path}: there is no Cargo.toml (is it the iyakki-ui-sdk crate?)"));
    }
    let tile_path = if tile {
        let p = if in_repo { "../iyakki-sdk-tile".to_string() } else { Path::new(&sdk_path).parent().map(|d| d.join("iyakki-sdk-tile")).unwrap_or_default().to_string_lossy().to_string() };
        let at = if in_repo { base.join("iyakki-sdk-tile") } else { PathBuf::from(&p) };
        if !at.join("Cargo.toml").is_file() {
            return Err(format!("{}: there is no iyakki-sdk-tile crate next to the SDK (needed for --tile)", at.display()));
        }
        Some(p)
    } else {
        None
    };
    let root = base.join(&n.crate_name);
    if root.exists() {
        return Err(format!("{} exists already; nothing was written", root.display()));
    }
    // Outside the workspace the SDK path is made relative to the new crate when the crate is inside the same checkout (the folder that holds
    // `crates/iyakki-ui-sdk`) or at most three folders below the folder that holds the checkout (a crate beside the checkout, or in a project folder with its own repository beside it: the layout of a remote build host that copies each folder next to the others): the project then builds on another machine or a remote build host that has the checkout. Elsewhere it stays absolute.
    let abs_root = clean(&if root.is_absolute() { root.clone() } else { cwd.join(&root) });
    let checkout = Path::new(&sdk_path).parent().and_then(|c| c.parent()).map(Path::to_path_buf);
    let inside = !in_repo && checkout.as_ref().is_some_and(|top| abs_root.starts_with(top) || top.parent().is_some_and(|up| abs_root.starts_with(up) && abs_root.components().count() - up.components().count() <= 3));
    let show = |p: &Path| if inside { relative(&abs_root, p) } else { p.to_string_lossy().to_string() };
    let preview_path = if in_repo { Some("../iyakki-ui-preview".to_string()) } else { Path::new(&sdk_path).parent().map(|d| d.join("iyakki-ui-preview")).filter(|d| d.join("Cargo.toml").is_file()).map(|d| show(&d)) };
    let tile_path = tile_path.map(|t| if in_repo { t } else { show(Path::new(&t)) });
    let sdk_shown = if in_repo { sdk_path.clone() } else { show(Path::new(&sdk_path)) };
    let list = files(&n, &sdk_shown, in_repo, tile_path.as_deref(), preview_path.as_deref());
    let mut written = Vec::new();
    for (rel, text) in &list {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(&root)).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        written.push(rel.clone());
    }
    Ok((root, written, in_repo))
}

/// `iyakki new-app ...`
pub fn cli(args: &[String]) -> i32 {
    let flag = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let name = args.iter().enumerate().find(|(i, a)| !a.starts_with("--") && (*i == 0 || !matches!(args[*i - 1].as_str(), "--dir" | "--sdk"))).map(|(_, a)| a.clone());
    let Some(name) = name else {
        crate::qeprintln!("{USAGE}");
        return 2;
    };
    let tile = args.iter().any(|a| a == "--tile");
    match generate_with(&name, flag("--dir").as_deref(), flag("--sdk").as_deref(), tile) {
        Ok((root, written, in_repo)) => {
            crate::qprintln!("Made {} ({}).", root.display(), written.join(", "));
            if tile {
                crate::qprintln!("As a WASM tile: `cargo build --release --lib --target wasm32-unknown-unknown` (needs `rustup target add wasm32-unknown-unknown`), then ship the .wasm with the manifest's tile_abi and tile_caps.");
            }
            if !in_repo {
                crate::qprintln!("The SDK is named in Cargo.toml by a path relative to the new crate when the crate is inside the checkout or beside it, otherwise by an absolute path on this machine (on another machine or a build host, edit that one line).");
            }
            if in_repo {
                crate::qprintln!("Next: add \"crates/iyakki-app-{name}\" to the workspace members, a line `iyakki-app-{name} apps` to dist/scripts/crate-layers.txt and a row to FEATURES.md; then `cargo test -p iyakki-app-{name}` (the first run writes the golden files: read and commit them).");
            } else {
                crate::qprintln!("Next, in that folder: `cargo test` (the first run writes the golden files in tests/golden: read and commit them), then `cargo run --example preview` to see the app in a browser tab. README.md lists the commands.");
            }
            0
        }
        Err(e) => {
            crate::qeprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
#[path = "newapp_tests.rs"]
mod tests;
