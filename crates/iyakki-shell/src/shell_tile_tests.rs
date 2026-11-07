//! Custom apps (tile apps) at the shell level: they appear in the launcher, open in their own tile and take real clicks, show the standard
//! bar, are removed through the lifecycle layer with the data choice, dev apps are marked and reload, and nothing overlaps at four sizes.

use super::tests::*;
use super::*;
use iyakki_agent::pkg::{build_generic, new_signing_key, public_key_hex};
use iyakki_agent::store::install::{Actor, Confirm};
use iyakki_agent::store::manifest::Meta;
use iyakki_agent::store::tile::Bundle;
use iyakki_agent::store::Store;
use iyakki_agent::skills::Scope;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn dist(p: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist").join(p)
}

/// Install a sample tile app as `root` for everyone (or for `root` alone), signed by a key the device trusts.
fn install(sh: &Shell, sample: &str, shared: bool) {
    let svc = sh.services_for_demo().clone();
    let key = new_signing_key().unwrap();
    svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
    let b = Bundle::from_dir(&dist(&format!("samples/tiles/{sample}"))).unwrap();
    let m = Meta { title: b.manifest.name.clone(), summary: "A sample".into(), permissions: b.manifest.caps.permissions(), publisher: iyakki_agent::store::manifest::Publisher { id: "acme".into(), name: "Acme".into(), url: String::new() }, ..Default::default() };
    let bytes = build_generic("tile", sample, "1.0.0", "MIT", &b.to_payload(), Some(&m.to_json()), None, Some(&key)).unwrap().into_bytes();
    let store = Store::for_services(&svc);
    let actor = Actor { user: "root".into(), allowed: true, can_own: true, can_shared: true, admin: true };
    let scope = if shared { Scope::Shared } else { Scope::User("root".into()) };
    let r = store.review(&svc, &actor, bytes, None, "file", false, scope, false, &[]);
    store.install(&svc, &actor, &r, Confirm::all()).unwrap_or_else(|e| panic!("{e}: {}", r.summary()));
}

fn click_label(sh: &mut Shell, g: &mut Gfx, label: &str) {
    sh.render(g);
    let r = sh.hits().iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no click region {label:?}: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>())).rect;
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
}

/// Click a custom app's tile in the launcher (tiles take clicks by their rectangles, as the system tiles do).
fn open_tile(sh: &mut Shell, g: &mut Gfx, title: &str) {
    sh.set_group(Group::Apps);
    sh.render(g);
    let items = sh.group_items();
    let i = items.iter().position(|it| matches!(it, LItem::App(e) if e.title == title)).unwrap_or_else(|| panic!("no tile {title}"));
    // show the page the tile is on (Work holds the system tiles too)
    sh.sel = i;
    sh.render(g);
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let cap = launcher::per_page(w, h, s);
    let r = launcher::page_rects(w, h, s, items.len(), i / cap)[i % cap];
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
}

#[test]
fn installed_tile_apps_get_their_own_tiles_in_my_apps_and_each_person_sees_only_theirs() {
    let (mut sh, _) = shell_with_users("tiles-launch");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.groups(), Group::CHIPS[..5].to_vec(), "five sections, no Apps tab while nothing is installed");
    install(&sh, "calculator", true);
    install(&sh, "notes", false);
    sh.refresh_custom();
    // apps that declare no section land in Apps
    sh.set_group(Group::Apps);
    assert_eq!(sh.group_custom().iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Calculator", "Notes and to-dos"]);
    sh.render(&mut g);
    assert_eq!(sh.group_items().len(), sh.group_tiles().len() + 2);
    sh.set_group(Group::Home);
    assert!(sh.group_custom().is_empty());
    // alice sees the shared one, not root's own
    sh.sign_out();
    login(&mut sh, "alice", "alicepass1");
    sh.refresh_custom();
    assert_eq!(sh.custom.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["calculator"]);
    // a guest has no browser, so no custom apps either
    sh.sign_out();
    login(&mut sh, "gus", "guestpass1");
    sh.refresh_custom();
    assert!(sh.custom.is_empty() == !iyakki_agent::store::tilehub::allowed(&iyakki_sec::policy::Role::Guest.template()));
}

#[test]
fn a_tile_app_opens_full_screen_takes_real_clicks_and_back_returns_to_the_launcher() {
    let (mut sh, _) = shell_with_users("tiles-open");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    install(&sh, "calculator", true);
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Calculator");
    assert_eq!(sh.screen_name(), "tile:calculator");
    sh.render(&mut g);
    // 7 + 8 = by clicking the drawn keys with the mouse
    for k in ["7", "+", "8", "="] {
        click_label(&mut sh, &mut g, k);
    }
    sh.render(&mut g);
    let text = match &mut sh.screen {
        Screen::App(_, _) => sh.tile_text_for_test(),
        _ => panic!("still in the app"),
    };
    assert!(text.contains("15"), "{text}");
    // the keyboard works too: Tab to a control and Enter presses it
    sh.handle(&Event::Key(Key::Tab));
    // Back (the header's own button) returns to the launcher with the tile still there
    let s = scale_for(1280, 720);
    let b = back_rect(1280, s);
    sh.handle(&Event::MouseDown((b.x + 5.0) as i32, (b.y + 5.0) as i32));
    assert_eq!(sh.screen_name(), "launcher");
    assert_eq!(sh.custom_open, None);
}

#[test]
fn the_app_bar_shows_the_custom_app_mark_and_info_lists_what_it_may_do() {
    let (mut sh, _) = shell_with_users("tiles-info");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    install(&sh, "notes", true);
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Notes and to-dos");
    click_label(&mut sh, &mut g, "Info");
    sh.render(&mut g);
    let text = sh.tile_card_text_for_test();
    assert!(text.contains("custom app") && text.contains("acme") && text.contains("keep its own saved data") && text.contains("never use your vault"), "{text}");
    sh.handle(&Event::Key(Key::Esc));
    assert!(sh.tile_card_text_for_test().is_empty(), "Esc closes the panel");
    assert_eq!(sh.screen_name(), "tile:notes", "Esc closed the panel, not the app");
}

#[test]
fn removing_a_tile_app_asks_about_its_data_through_the_lifecycle_layer() {
    let (mut sh, _) = shell_with_users("tiles-remove");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    install(&sh, "notes", true);
    sh.refresh_custom();
    open_tile(&mut sh, &mut g, "Notes and to-dos");
    // the app saves something
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    let text = sh.tile_text_for_test();
    let field = sh.hits().iter().find(|h| h.label == "field").unwrap_or_else(|| panic!("no field: {labels:?} text={text}")).rect;
    sh.handle(&Event::MouseDown((field.x + 8.0) as i32, (field.y + field.h / 2.0) as i32));
    for c in "Buy milk".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    click_label(&mut sh, &mut g, "Add");
    let st = iyakki_agent::store::tile::storage_file(&Store::for_services(sh.services_for_demo()), &Scope::Shared, "notes", "root");
    assert!(std::fs::read_to_string(&st).unwrap().contains("Buy milk"), "saved through the bridge");
    click_label(&mut sh, &mut g, "Remove");
    sh.render(&mut g);
    let t = sh.tile_card_text_for_test();
    assert!(t.contains("KEPT unless you choose") && t.contains("launcher"), "{t}");
    sh.handle(&Event::Key(Key::Char('y')));
    assert_eq!(sh.screen_name(), "launcher");
    sh.refresh_custom();
    assert!(sh.custom.is_empty());
    assert!(st.exists(), "kept by default");
}

#[test]
fn a_dev_app_is_added_by_an_administrator_only_marked_unsigned_and_reloads_when_its_files_change() {
    let (mut sh, _) = shell_with_users("tiles-dev");
    let mut g = Gfx::new(1280, 720);
    let ws = std::env::temp_dir().join(format!("iyakki-devtile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    iyakki_agent::store::author::scaffold("tile", "clock-dev", &ws, "").unwrap();
    let dir = ws.join("clock-dev");
    login(&mut sh, "alice", "alicepass1");
    let svc = sh.services_for_demo().clone();
    let user_policy = iyakki_sec::policy::Role::User.template();
    assert!(iyakki_agent::store::tilehub::add_dev(&svc, &user_policy, "alice", &dir).unwrap_err().contains("administrator"));
    sh.sign_out();
    login(&mut sh, "root", "rootpass12");
    let e = iyakki_agent::store::tilehub::add_dev(&svc, &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    assert!(e.dev && e.trust == "unsigned dev app");
    sh.refresh_custom();
    sh.set_group(Group::Apps);
    sh.render(&mut g);
    open_tile(&mut sh, &mut g, "Clock dev");
    sh.render(&mut g);
    assert!(sh.tile_text_for_test().contains("Press the button"), "{}", sh.tile_text_for_test());
    click_label(&mut sh, &mut g, "Info");
    assert!(sh.tile_card_text_for_test().contains("UNSIGNED DEV APP"));
    sh.handle(&Event::Key(Key::Esc));
    // edit the file: the tile reloads by itself
    std::thread::sleep(std::time::Duration::from_millis(20));
    let js = std::fs::read_to_string(dir.join("app.js")).unwrap().replace("Press the button.", "Press it now.");
    std::fs::write(dir.join("app.js"), js).unwrap();
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap().replace("Press the button.", "Press it now.");
    std::fs::write(dir.join("index.html"), html).unwrap();
    for _ in 0..64 {
        sh.handle(&Event::Tick);
    }
    sh.render(&mut g);
    assert!(sh.tile_text_for_test().contains("Press it now"), "{}", sh.tile_text_for_test());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn nothing_overlaps_or_leaves_the_screen_for_the_launcher_tile_the_app_bar_and_the_panels_at_four_sizes() {
    let (mut sh, _) = shell_with_users("tiles-geom");
    login(&mut sh, "root", "rootpass12");
    install(&sh, "calculator", true);
    install(&sh, "notes", true);
    install(&sh, "converter", true);
    sh.refresh_custom();
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        sh.resize_for_test(w, h);
        let s = scale_for(w, h);
        // the launcher, section Apps: the chips fit and the tiles are inside the screen and apart
        sh.leave_app();
        sh.set_group(Group::Apps);
        sh.render(&mut g);
        let gs = sh.groups();
        let labels: Vec<&str> = gs.iter().map(|x| x.label()).collect();
        let chips = launcher::chip_rects(s, &labels);
        assert!(chips.last().unwrap().x + chips.last().unwrap().w <= w as f32 - 20.0 * s, "{w}x{h} chips {chips:?}");
        let items = sh.group_items();
        assert_eq!(sh.group_custom().len(), 3);
        let rects = launcher::page_rects(w, h, s, items.len(), 0);
        for (i, a) in rects.iter().enumerate() {
            assert!(a.x >= 0.0 && a.x + a.w <= w as f32 && a.y + a.h <= h as f32, "{w}x{h} {a:?}");
            assert!(rects[i + 1..].iter().all(|b| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y || b.y + b.h <= a.y), "{w}x{h} overlap");
        }
        // the app: bar buttons inside the screen, not on top of each other; the info panel and the remove panel inside the screen
        open_tile(&mut sh, &mut g, "Calculator");
        sh.render(&mut g);
        let bar = sh.tile_bar_for_test();
        assert_eq!(bar.len(), 3);
        for (name, r) in &bar {
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y >= 0.0 && r.y + r.h <= h as f32 * 0.2, "{w}x{h} {name} {r:?}");
        }
        for (i, (_, a)) in bar.iter().enumerate() {
            assert!(bar[i + 1..].iter().all(|(_, b)| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5), "{w}x{h} bar buttons overlap");
        }
        // the Back button of the header is not covered by the bar
        let back = back_rect(w, s);
        assert!(bar.iter().all(|(_, r)| r.y >= back.y + back.h - 2.0 || r.x + r.w <= back.x || r.x >= back.x + back.w));
        for button in ["Info", "Remove"] {
            click_label(&mut sh, &mut g, button);
            sh.render(&mut g);
            let (p, btns) = sh.tile_card_geometry_for_test().expect("a panel is open");
            assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= w as f32 && p.y + p.h <= h as f32, "{w}x{h} {button} panel {p:?}");
            assert!(!btns.is_empty());
            for b in &btns {
                assert!(b.x >= p.x && b.x + b.w <= p.x + p.w + 0.5 && b.y >= p.y && b.y + b.h <= p.y + p.h + 0.5, "{w}x{h} {button} button {b:?} outside {p:?}");
            }
            sh.handle(&Event::Key(Key::Esc));
        }
    }
}

#[test]
fn the_store_tile_says_how_many_updates_the_last_check_found_at_four_sizes() {
    use iyakki_agent::store::updatecheck as chk;
    let (mut sh, _) = shell_with_users("store-update-chip");
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    let upd = |id: &str| chk::Update { kind: chk::Kind::App, id: id.into(), title: id.into(), from: "1.0.0".into(), to: "1.1.0".into(), source: "Stick".into(), new_permissions: vec![], note: String::new() };
    for (n, want) in [(0usize, "Apps and skills"), (1, "1 update"), (2, "2 updates")] {
        chk::Report { checked_at: 1, updates: (0..n).map(|i| upd(&format!("skill:a{i}"))).collect(), ..Default::default() }.save(svc.dir()).unwrap();
        for (w, h) in SIZES {
            let mut g = Gfx::new(w, h);
            sh.resize_for_test(w, h);
            let s = scale_for(w, h);
            sh.leave_app();
            sh.set_group(Group::System);
            sh.refresh_badge();
            sh.render(&mut g);
            let items = sh.group_items();
            let i = items.iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Store)).expect("the STORE tile");
            sh.sel = i;
            sh.render(&mut g);
            let cap = launcher::per_page(w, h, s);
            let tile = launcher::page_rects(w, h, s, items.len(), i / cap)[i % cap];
            let hit = g.layout_log.texts.iter().rev().find(|(_, _, _, t)| t == want).unwrap_or_else(|| panic!("{w}x{h}: {want:?} not drawn for {n} update(s)"));
            let r = hit.1;
            assert!(r.x >= tile.x - 1.0 && r.x + r.w <= tile.x + tile.w + 1.0 && r.x >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: {want:?} {r:?} leaves its tile {tile:?}");
        }
    }
    let _ = std::fs::remove_file(svc.dir().join(chk::FILE));
}
