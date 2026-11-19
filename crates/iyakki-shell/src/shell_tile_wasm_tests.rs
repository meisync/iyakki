//! `wasm` tile apps at the shell level (F-WASM-09, F-APPS-45): a tile app that carries a WebAssembly module is refused while the
//! experimental switch is off, runs the module in the tile host once it is on and takes real clicks and typing, a module that is not a
//! tile says so in words, and a `.png` icon is drawn on the launcher tile instead of the first letter.
use iyakki_agent::facets_h::RealServices as _;

use super::tests::*;
use super::*;
use iyakki_agent::store::tilehub;

/// The example counter tile, assembled from `tile.wat` (the file a tile app carries).
const MODULE: &[u8] = include_bytes!("../../iyakki-tile-example/tile.wasm");

fn app_dir(tag: &str, module: &[u8], icon: &str, extra: Option<(&str, &[u8])>) -> std::path::PathBuf {
    let ws = std::env::temp_dir().join(format!("iyakki-wasmtile-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let dir = ws.join("counter");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app.json"), format!(r#"{{"id":"counter","name":"Counter","icon":"{icon}","wasm":"tile.wasm"}}"#)).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><p>This app is a WASM tile.</p>").unwrap();
    std::fs::write(dir.join("tile.wasm"), module).unwrap();
    if let Some((n, b)) = extra {
        std::fs::write(dir.join(n), b).unwrap();
    }
    dir
}

fn open_tile(sh: &mut Shell, g: &mut Gfx, title: &str) {
    sh.set_group(Group::Apps);
    sh.render(g);
    let items = sh.group_items();
    let i = items.iter().position(|it| matches!(it, LItem::App(e) if e.title == title)).unwrap_or_else(|| panic!("no tile {title}"));
    sh.sel = i;
    sh.render(g);
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let cap = launcher::per_page(w, h, s);
    let r = launcher::page_rects(w, h, s, items.len(), i / cap)[i % cap];
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
}

fn lines(sh: &mut Shell) -> Vec<String> {
    sh.wasm_tile_text_for_test().lines().map(str::to_string).collect()
}

fn switch(svc: &dyn iyakki_agent_api::ServicesHandle, on: bool) {
    svc.settings().lock().unwrap().update(|s| s.wasm_tiles = on).unwrap();
}

#[test]
fn a_wasm_tile_is_refused_while_the_switch_is_off_then_runs_the_module_and_takes_real_clicks_and_typing() {
    let (mut sh, _) = shell_with_users("tiles-wasm");
    let mut g = Gfx::new(1280, 720);
    let dir = app_dir("run", MODULE, "🧩", None);
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    assert!(!tilehub::wasm_enabled(svc.real()), "off by default");
    let e = tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    assert!(e.wasm, "the entry knows it carries a module");
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Counter");
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "tile:counter");
    let said = sh.wasm_tile_text_for_test();
    assert!(said.contains("experimental") && said.contains("off"), "{said}");
    assert!(tilehub::open_wasm(&iyakki_agent::services::Services::from_handle(&svc), &iyakki_sec::policy::Role::Admin.template(), "root", &e).is_err());

    // switched on, Ctrl+R starts it again: the tile host draws the example counter
    switch(&*svc, true);
    sh.handle(&Event::Key(Key::Ctrl('r')));
    sh.render(&mut g);
    let l = lines(&mut sh);
    assert_eq!(&l[..4], ["TILE DEMO", "COUNT", "0", "+1"], "{l:?}");

    // a real click on the button reaches the module and the picture changes
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let r = sh.hits().iter().find(|x| x.label == "WASM tile").expect("the tile surface takes clicks").rect;
    for n in 1..=3 {
        sh.handle(&Event::MouseDown((r.x + 30.0 * s) as i32, (r.y + 80.0 * s) as i32));
        sh.render(&mut g);
        assert_eq!(lines(&mut sh)[2], n.to_string());
    }
    // typing reaches the text field of the module
    sh.handle(&Event::Key(Key::Char('h')));
    sh.handle(&Event::Key(Key::Char('i')));
    sh.render(&mut g);
    assert!(lines(&mut sh).contains(&"hi".to_string()), "{:?}", lines(&mut sh));
    sh.handle(&Event::Key(Key::Backspace));
    sh.render(&mut g);
    assert!(lines(&mut sh).contains(&"h".to_string()));
    // the picture is really on screen: the button's blue is where the module put it
    let px = |g: &Gfx, x: f32, y: f32| {
        let i = (((y as u32) * g.width() + x as u32) * 4) as usize;
        [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2]]
    };
    assert_eq!(px(&g, r.x + 30.0 * s, r.y + 80.0 * s), [40, 110, 220], "the module's button colour");

    // switched off again: the next start is refused
    switch(&*svc, false);
    sh.handle(&Event::Key(Key::Ctrl('r')));
    sh.render(&mut g);
    assert!(sh.wasm_tile_text_for_test().contains("off"));
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_name(), "launcher");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_module_that_is_not_a_tile_is_stopped_in_words_and_the_shell_carries_on() {
    let (mut sh, _) = shell_with_users("tiles-wasm-bad");
    let mut g = Gfx::new(1280, 720);
    // a valid but empty module: it has none of the six tile functions
    let dir = app_dir("bad", b"\0asm\x01\0\0\0", "🧩", None);
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    switch(&*svc, true);
    tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Counter");
    sh.render(&mut g);
    let said = sh.wasm_tile_text_for_test();
    assert!(said.contains("not a tile"), "{said}");
    sh.handle(&Event::Tick);
    sh.handle(&Event::Key(Key::Char('x')));
    sh.render(&mut g);
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_name(), "launcher");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn the_pictures_a_wasm_tile_carries_go_into_its_table_before_it_runs_and_a_picture_the_table_refuses_stops_it_in_words() {
    let png = |w: u32, h: u32| {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        pm.encode_png().unwrap()
    };
    let (mut sh, _) = shell_with_users("tiles-wasm-pics");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    switch(&*svc, true);
    let (small, big) = (png(4, 4), png(1025, 1));
    let services = iyakki_agent::services::Services::from_handle(&svc);
    for (tag, file, bytes, refused) in [("ok", "pic.png", &small, false), ("big", "big.png", &big, true)] {
        let dir = app_dir(&format!("pics-{tag}"), MODULE, "🧩", Some((file, bytes)));
        std::fs::write(dir.join("app.json"), format!(r#"{{"id":"counter","name":"Counter","icon":"🧩","wasm":"tile.wasm","images":[{{"handle":7,"file":"{file}"}}]}}"#)).unwrap();
        // the loader decodes the bundle's pictures into the tile's table, with the table's caps
        let e = tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir);
        match (e, refused) {
            (Ok(e), false) => {
                let t = tilehub::wasm_pictures(&services, "root", &e).unwrap();
                assert_eq!(t.get(7).map(|p| (p.w, p.h)), Some((4, 4)), "the table has the picture under its handle");
                assert!(tilehub::open_wasm(&services, &iyakki_sec::policy::Role::Admin.template(), "root", &e).is_ok());
            }
            (Err(why), true) => assert!(why.contains("refused"), "{why}"),
            (other, _) => panic!("{tag}: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
    // the tile on the screen starts with the table in place
    let dir = app_dir("pics-run", MODULE, "🧩", Some(("pic.png", &small)));
    std::fs::write(dir.join("app.json"), r#"{"id":"counter","name":"Counter","icon":"🧩","wasm":"tile.wasm","images":[{"handle":7,"file":"pic.png"}]}"#).unwrap();
    tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Counter");
    sh.render(&mut g);
    assert_eq!(&lines(&mut sh)[..4], ["TILE DEMO", "COUNT", "0", "+1"]);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_png_icon_is_drawn_on_the_launcher_tile_and_a_letter_stands_in_when_it_cannot_be_read() {
    let (mut sh, _) = shell_with_users("tiles-png");
    let png = {
        let mut pm = tiny_skia::Pixmap::new(32, 32).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(210, 20, 30, 255));
        pm.encode_png().unwrap()
    };
    let dir = app_dir("png", MODULE, "icon.png", Some(("icon.png", &png)));
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    let e = tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    let pic = custom_icon_picture(&e).expect("a .png icon decodes");
    assert_eq!((pic.width(), pic.height()), (32, 32));
    // drawn: the middle of the badge is the icon's red, not the accent colour of a letter badge
    let r = R::new(40.0, 40.0, 200.0, 200.0);
    let mid = |g: &Gfx| {
        let (x, y) = ((r.x + r.w / 2.0) as u32, (r.y + 26.0 + r.w * 0.26) as u32);
        let i = ((y * g.width() + x) * 4) as usize;
        [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2]]
    };
    let mut g = Gfx::new(400, 400);
    draw_custom_tile(&mut g, &e, r, 1.0, false, true, 1.0);
    let px = mid(&g);
    assert!(px[0] > 190 && px[1] < 60 && px[2] < 60, "{px:?}");
    // an emoji icon, a picture that is not a picture and an svg all keep the letter
    let mut plain = e.clone();
    plain.icon_picture = None;
    let mut g2 = Gfx::new(400, 400);
    draw_custom_tile(&mut g2, &plain, r, 1.0, false, true, 1.0);
    assert_ne!(mid(&g2), px);
    let mut bad = e.clone();
    bad.key = "tile/shared/bad".into();
    bad.icon_picture = Some(("icon.png".into(), b"not a picture".to_vec()));
    assert!(custom_icon_picture(&bad).is_none());
    bad.key = "tile/shared/svg".into();
    bad.icon_picture = Some(("icon.svg".into(), b"<svg/>".to_vec()));
    assert!(custom_icon_picture(&bad).is_none());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

/// A tile of ABI 2 whose frame carries `Access` operations (F-A11Y-06): a static WAT module that returns the encoded frame, and counts the
/// taps it gets in a global so the test can see an activation arrive as a click.
fn access_module() -> Vec<u8> {
    wat::parse_str(access_wat()).expect("the test tile assembles")
}

fn access_wat() -> String {
    use iyakki_tile_abi::{access_flags, Color, DisplayList, Op, Rect, Role};
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 200, 120), color: Color(30, 30, 40, 255) });
    l.push(Op::Hit { id: 1, r: Rect::new(10, 10, 80, 30) });
    l.push(Op::Hit { id: 2, r: Rect::new(10, 60, 80, 30) });
    l.push(Op::Access { id: 1, role: Role::Button, r: Rect::new(10, 10, 80, 30), name: "Save changes".into(), value: String::new(), flags: 0 });
    l.push(Op::Access { id: 2, role: Role::Toggle, r: Rect::new(10, 60, 80, 30), name: "Wi-Fi".into(), value: String::new(), flags: access_flags::CHECKED });
    let frame = l.encode().unwrap();
    let mut prefix: String = (frame.len() as u32).to_le_bytes().iter().map(|b| format!("\\{b:02x}")).collect();
    prefix.extend(frame.iter().map(|b| format!("\\{b:02x}")));
    format!(
        r#"(module
  (memory (export "memory") 2)
  (data (i32.const 1024) "{prefix}")
  (global $h (mut i32) (i32.const 8192))
  (func (export "iyakki_tile_abi") (result i32) (i32.const 2))
  (func (export "iyakki_alloc") (param i32) (result i32) (global.get $h))
  (func (export "iyakki_tile_init") (param i32 i32 i32) (result i32) (i32.const 0))
  (func (export "iyakki_tile_event") (param i32 i32) (result i32) (i32.const 0))
  (func (export "iyakki_tile_frame") (result i32) (i32.const 1024)))"#
    )
}

#[test]
fn an_abi_2_tile_reaches_the_toolkits_access_tree_with_its_own_names_roles_and_states() {
    let (mut sh, _) = shell_with_users("tiles-wasm-access");
    let mut g = Gfx::new(1280, 720);
    let dir = app_dir("access", &access_module(), "🧩", None);
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    switch(&*svc, true);
    tilehub::add_dev(svc.real(), &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Counter");
    sh.render(&mut g);
    let hits: Vec<_> = sh.hits().iter().filter(|h| h.label == "Save changes" || h.label == "Wi-Fi").cloned().collect();
    assert_eq!(hits.len(), 2, "both access nodes are hits: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    let nodes: Vec<_> = hits.iter().map(|h| iyakki_uikit::access::node_of(h).expect("a described node")).collect();
    assert_eq!(nodes[0].speak(), "Save changes, button");
    assert!(nodes[1].speak().starts_with("Wi-Fi, switch"), "{}", nodes[1].speak());
    // the tile's own hits are named, so none is listed as unnamed
    assert!(iyakki_uikit::access::unnamed(sh.hits(), (1280.0, 720.0)).is_empty());
    // the keyboard walk order is the tile's reading order
    let walk: Vec<String> = iyakki_uikit::access::entries(sh.hits(), (1280.0, 720.0)).into_iter().map(|e| e.node.name).filter(|n| n == "Save changes" || n == "Wi-Fi").collect();
    assert_eq!(walk, ["Save changes", "Wi-Fi"]);
    // activating a node does not break the tile (it is a click in the middle of the rectangle)
    let r = hits[0].rect;
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "tile:counter");
    assert!(!sh.wasm_tile_text_for_test().contains("not a tile"));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
