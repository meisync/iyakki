//! The IDE for apps for IYAKKI: the New form's tile-app default and the target it records, the header chip, Change target on the
//! owner's calc workspace, Run / Add tile, and the whole Publish flow against the real store server on a loopback socket
//! (checks, version, key, token, review card, result, install back from the store), with the geometry of every new dialog at four sizes.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_agent::store::catalog::HttpFetch;
use iyakki_agent::store::install::Actor;
use iyakki_agent::store::server::{hash_token, Config, StoreServer};
use iyakki_agent::store::{keys, tilehub, Store};
use iyakki_agent::target::{self as tg, Target as AppTarget};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};

use super::*;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn modal_card(app: &IdeApp) -> Option<&super::super::ide_tile::CardModal> {
    match &app.modal {
        Some(Modal::Card(c)) => Some(c),
        _ => None,
    }
}

fn card_text(app: &IdeApp) -> String {
    modal_card(app).map(|c| format!("{}\n{}", c.card.title, c.card.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n"))).unwrap_or_default()
}

#[test]
fn the_new_form_offers_the_tile_app_first_records_the_target_and_starts_a_tile_project() {
    let w = world("tile-new");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(NEW_APP_CHOICES[0], "IYAKKI tile app (a tile in the launcher)");
    key(&mut app, &w.cx, Key::Ctrl('N'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    assert_eq!(f.choice_idx(0), 0, "the tile app is the default");
    form_fits(&mut app, &w.cx, "new app form");
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(1, "calc");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    for f in ["index.html", "app.js", "style.css", "app.json", "iyakki-app.json", "README.md", "tests/app.test.js"] {
        assert!(d.join("calc").join(f).is_file(), "{f}");
    }
    assert_eq!(tg::read(&d), Some(AppTarget::IyakkiTile));
    assert_eq!(cur(&app).cfg.target, "iyakki-tile");
    let said = cur(&app).chat.input_text();
    assert!(said.contains("iyakki-tile-author") && said.contains("no server, no Dockerfile, no k3s") && said.contains("tile_install_dev"), "{said}");
    assert_eq!(iyakki_agent::projectstate::detect_stack(&d, "").pinned_skills("hello"), vec!["iyakki-app-author", "iyakki-tile-author"]);
    // a service on k3s: only the target is recorded, no files
    key(&mut app, &w.cx, Key::Ctrl('N'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_choice(0, 1);
    f.set_value(1, "orders");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert_eq!(tg::read(&d), Some(AppTarget::K3sService));
    assert!(!d.join("orders").exists());
}

#[test]
fn the_header_shows_the_target_chip_and_the_tile_buttons_and_the_chip_opens_the_target_form() {
    let w = world("tile-chip");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "change the target"), "a workspace without a target says so and the chip is there");
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    ticks(&mut app, &w.cx, 31);
    assert_eq!(cur(&app).cfg.target, "iyakki-tile", "the chip follows the project file");
    for (wd, ht) in SIZES {
        let g = render(&mut app, &w.cx, wd, ht);
        let chip = g.hits.iter().find(|h| h.label == "change the target");
        let band = g.hits.iter().find(|h| h.label == "project chooser").map(|h| h.rect).unwrap();
        if wd < 1000 {
            // too narrow for the chip: the target is under ⋯, Change target…
            assert!(chip.is_none() || chip.is_some_and(|c| c.rect.x + c.rect.w <= wd as f32), "{wd}x{ht}");
            continue_header_checks(&g, wd, ht);
            continue;
        }
        assert!(chip.is_some_and(|c| c.rect.x >= 0.0 && c.rect.x + c.rect.w <= wd as f32 && (c.rect.y - band.y).abs() < 8.0 && c.rect.h <= band.h + 2.0), "{wd}x{ht}: {:?} vs {band:?}", chip.map(|c| c.rect));
        continue_header_checks(&g, wd, ht);
    }
    let g = render(&mut app, &w.cx, 1920, 1080);
    for label in ["Add tile", "Publish"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label} is a button on a wide header");
    }
    // the overflow menu has the rest at every width
    let _ = render(&mut app, &w.cx, 800, 480);
    assert!(app.top_overflow.iter().any(|o| o.0.starts_with("Change target")) && app.top_overflow.iter().any(|o| o.0.starts_with("Validate and test app")));
    app.handle(&Event::Tab(T_TOP + 16), &w.cx);
    assert!(matches!(&app.modal, Some(Modal::Form(FormKind::Target, _))));
}

const CALC_BOARD: &str = "<!-- summary:start -->done 0 · partial 0 · planned 3 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-APP-01 | Calculator web app titled calc (UI + /api/calc) | P1 | planned | P1 | 0% | — |  |\n| F-DEPLOY-01 | Deploy calc to k3s (calc-dev) and verify | P1 | planned | P1 | 0% | — |  |\n| F-TEST-01 | Integration tests as a Job after rollout | P1 | planned | P1 | 0% | — |  |\n";

#[test]
fn change_target_on_the_owners_calc_workspace_shows_the_plan_converts_and_offers_only_the_teardown() {
    let w = world("tile-change");
    let d = project(&w, "calc", &[("server.js", "require('http').createServer(()=>{}).listen(8080)"), ("Dockerfile", "FROM node:20\n"), ("package.json", "{}")]);
    std::fs::write(d.join("FEATURES.md"), CALC_BOARD).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "calc", &d);
    app.open_target_form();
    let Some(Modal::Form(FormKind::Target, f)) = &mut app.modal else { panic!() };
    f.set_choice(0, 0);
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &w.cx, wd, ht);
    }
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let t = card_text(&app);
    assert!(t.contains("Change target to IYAKKI tile app") && t.contains("server.js") && t.contains("Dockerfile") && t.contains("F-DEPLOY-01") && t.contains("F-TEST-01") && t.contains("calc-dev") && t.contains("asks first"), "{t}");
    assert!(!t.contains("F-APP-01"), "the user-interface row stays: {t}");
    assert_eq!(tg::read(&d), None, "nothing changes before the person says yes");
    // the plan card fits at every size
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &w.cx, wd, ht);
        let c = modal_card(&app).unwrap();
        let (p, area_h) = (c.card.last_panel, ht as f32);
        assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= wd as f32 && p.y + p.h <= area_h, "{wd}x{ht} {p:?}");
        assert!(c.card.last_buttons.iter().all(|b| b.x >= p.x && b.x + b.w <= p.x + p.w + 0.5 && b.y + b.h <= p.y + p.h + 0.5), "{wd}x{ht}");
    }
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    assert_eq!(tg::read(&d), Some(AppTarget::IyakkiTile));
    assert_eq!(tg::teardown_pending(&d).as_deref(), Some("calc-dev"), "the old namespace may be removed, nothing else");
    assert!(d.join("server.js").exists(), "the files change only through the assistant's own approved tools");
    assert!(cur(&app).chat.is_running() || cur(&app).chat.error_text().is_some() || cur(&app).chat.input_text().is_empty(), "the assistant was asked to convert");
    // a workspace that is already a tile app: choosing the same target does nothing
    app.open_target_form();
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none());
}

#[test]
fn run_opens_the_tile_app_as_an_unsigned_dev_app_and_add_tile_needs_an_administrator() {
    let w = world("tile-run");
    let d = project(&w, "alpha", &[]);
    iyakki_agent::store::author::scaffold("tile", "clock", &d, "").unwrap();
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(app.project_dir(), Some(d.join("clock")));
    key(&mut app, &w.cx, Key::F(5));
    let (what, _) = app.request.take().expect("Run asks the shell to open the app");
    assert_eq!(what, format!("tile-preview:{}", d.join("clock").display()));
    // a service workspace keeps the old Run
    tg::write(&d, "Alpha", AppTarget::K3sService).unwrap();
    ticks(&mut app, &w.cx, 31);
    key(&mut app, &w.cx, Key::F(5));
    assert!(app.request.is_none());
    // Add tile
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    app.add_tile(&w.cx);
    let n = app.notice.clone().unwrap();
    assert!(n.0.contains("unsigned dev app") == n.1 || n.0.contains("administrator"), "{n:?}");
    let mut plain = w.cx.clone();
    plain.session.policy = iyakki_sec::policy::Role::User.template();
    plain.session.policy.fs_write = vec![w.base.display().to_string()];
    plain.session.policy.fs_read = vec![w.base.display().to_string()];
    app.notice = None;
    app.add_tile(&plain);
    assert!(app.notice.as_ref().unwrap().0.contains("administrator"), "{:?}", app.notice);
}

// ------------------------------------------------------------------------------------------------------------ publish

struct Loop;
impl Upstream for Loop {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        let rest = url.strip_prefix("http://127.0.0.1:").ok_or_else(|| format!("test transport: only the loopback ({url})"))?;
        let (port, path) = rest.split_once('/').map_or((rest, String::new()), |(p, r)| (p, format!("/{r}")));
        let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let mut req = format!("{method} {} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n", if path.is_empty() { "/" } else { &path }, body.len());
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let at = out.windows(4).position(|w| w == b"\r\n\r\n").ok_or("bad reply")?;
        let status = String::from_utf8_lossy(&out[..at]).split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        Ok(UpstreamResponse { status, body: out[at + 4..].to_vec() })
    }
}

struct StoreWorld {
    addr: String,
    stop: Arc<AtomicBool>,
    th: Option<std::thread::JoinHandle<()>>,
    ptoken: String,
    cx: AppCx,
    srv: Arc<StoreServer>,
}

impl Drop for StoreWorld {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.addr);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

fn store_world(w: &World, tag: &str) -> StoreWorld {
    let dir = w.base.join(format!("server-{tag}"));
    let mut cfg = Config::new(&dir);
    cfg.writes_per_min = 1000;
    cfg.admin_token_hash = Some(hash_token("admin-token-for-tests-0123456789"));
    let srv = Arc::new(StoreServer::open(cfg, &iyakki_agent::pkg::signing_key_hex(&iyakki_agent::pkg::new_signing_key().unwrap())).unwrap());
    let ptoken = srv.create_publisher("acme", "Acme").unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (srv.clone(), stop.clone());
    let th = Some(std::thread::spawn(move || iyakki_agent::store::server_http::serve(s2, l, st2)));
    // the device talks to it over the loopback
    let svc = iyakki_agent::services::Services::open(&w.base.join(format!("device-{tag}")), Arc::new(Loop), Arc::new(Loop)).unwrap();
    let mut cx = w.cx.clone();
    cx.services = svc;
    cx.services.on_login(&cx.session.user, "a-long-test-passphrase", true);
    let fetch = HttpFetch(Arc::new(Loop));
    let store = Store::for_services(&cx.services);
    let admin = Actor::from_policy(&cx.session.user, &cx.session.policy);
    let probe = store.probe_private(&fetch, &format!("http://{addr}")).unwrap();
    let id = store.add_private_source(&admin, &fetch, "Office store", &format!("http://{addr}"), &probe.fingerprint).unwrap();
    store.set_publish_target(&admin, &id).unwrap();
    StoreWorld { addr, stop, th, ptoken, cx, srv }
}

fn card_fits(app: &IdeApp, wd: u32, ht: u32, what: &str) {
    let c = modal_card(app).unwrap_or_else(|| panic!("{what}: a card is open"));
    let p = c.card.last_panel;
    assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= wd as f32 && p.y + p.h <= ht as f32, "{what} {wd}x{ht} panel {p:?}");
    assert!(!c.card.last_buttons.is_empty());
    for (i, b) in c.card.last_buttons.iter().enumerate() {
        assert!(b.x >= p.x && b.x + b.w <= p.x + p.w + 0.5 && b.y >= p.y && b.y + b.h <= p.y + p.h + 0.5, "{what} {wd}x{ht} button {b:?} outside {p:?}");
        assert!(c.card.last_buttons[i + 1..].iter().all(|o| b.x + b.w <= o.x + 0.5 || o.x + o.w <= b.x + 0.5 || b.y + b.h <= o.y + 0.5 || o.y + o.h <= b.y + 0.5), "{what} {wd}x{ht} buttons overlap");
    }
}

fn form_fits(app: &mut IdeApp, cx: &AppCx, what: &str) {
    for (wd, ht) in SIZES {
        let g = render(app, cx, wd, ht);
        // everything the form registered as clickable is inside the window
        for h in g.hits.iter().filter(|h| h.label != "backdrop") {
            assert!(h.rect.x >= -0.5 && h.rect.y >= -0.5 && h.rect.x + h.rect.w <= wd as f32 + 0.5 && h.rect.y + h.rect.h <= ht as f32 + 0.5, "{what} {wd}x{ht}: {} {:?}", h.label, h.rect);
        }
    }
}

#[test]
fn publish_from_the_ide_end_to_end_with_the_real_store_server_and_a_tile_that_comes_back_installed() {
    let w = world("tile-pub");
    let sw = store_world(&w, "pub");
    let cx = sw.cx.clone();
    let d = project(&w, "alpha", &[]);
    iyakki_agent::store::author::scaffold("tile", "office-clock", &d, "").unwrap();
    let app_dir = d.join("office-clock");
    std::fs::write(app_dir.join("CHANGELOG.md"), "# Changelog\n\n## 0.1.0 - first\n- first\n").unwrap();
    let t = std::fs::read_to_string(app_dir.join("iyakki-app.json")).unwrap().replace("A tile app for the IYAKKI launcher.", "A clock for the office wall.");
    std::fs::write(app_dir.join("iyakki-app.json"), t).unwrap();
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    let mut app = IdeApp::new(&cx);
    open(&mut app, &cx, "Alpha", &d);

    // 1. a project that fails its checks is not offered the form
    let good_js = std::fs::read_to_string(app_dir.join("app.js")).unwrap();
    std::fs::write(app_dir.join("app.js"), "eval('1');").unwrap();
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let t = card_text(&app);
    assert!(t.contains("Publish: fix these first") && t.contains("eval and new Function"), "{t}");
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &cx, wd, ht);
        card_fits(&app, wd, ht, "fix first");
    }
    key(&mut app, &cx, Key::Esc);
    std::fs::write(app_dir.join("app.js"), good_js).unwrap();

    // 2. the form: the store (the default target first), the version, the change log and the key
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let Some(Modal::Form(FormKind::Publish, f)) = &app.modal else { panic!("the publish form: {:?}", card_text(&app)) };
    assert!(f.title.contains("office-clock 0.1.0"));
    let store_field = &f.fields[0];
    let iyakki_shell_choices = match &store_field.kind {
        crate::apps::widgets::FieldKind::Choice(o) => o.clone(),
        _ => panic!(),
    };
    assert!(iyakki_shell_choices[0].starts_with("★ Office store") && iyakki_shell_choices[0].contains(&sw.addr) && iyakki_shell_choices[0].contains("key "), "{iyakki_shell_choices:?}");
    assert!(iyakki_shell_choices.iter().any(|o| o.starts_with("Deploy a new private store")) && iyakki_shell_choices.iter().any(|o| o.starts_with("Export package file")));
    form_fits(&mut app, &cx, "publish form");

    // 3. submit with no key yet: one is made, the token is missing, the masked dialog asks for it
    key(&mut app, &cx, Key::Ctrl('s'));
    assert!(keys::list(&cx.services, "tester").len() == 1, "the key was made");
    let Some(Modal::Form(FormKind::PublishToken, f)) = &mut app.modal else { panic!("the token dialog: {:?}", card_text(&app)) };
    assert!(matches!(f.fields[0].kind, crate::apps::widgets::FieldKind::Secret));
    form_fits(&mut app, &cx, "token form");
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(0, &sw.ptoken);
    // the token is on screen only as dots
    let g = render(&mut app, &cx, 1280, 720);
    let _ = g;
    key(&mut app, &cx, Key::Ctrl('s'));
    assert!(iyakki_agent::store::publishflow::token_present(&cx.services, "tester", &Store::for_services(&cx.services).config().sources[0].id));

    // 4. the review card: server (http warning), package, permissions, trust (new key), files, the admin-key button
    let t = card_text(&app);
    assert!(t.contains("Publish office-clock 0.1.0 to Office store?") && t.contains(&format!("Server: http://{}", sw.addr)) && t.contains("plain http") && t.contains("office-clock 0.1.0 (tile)"), "{t}");
    assert!(t.contains("saved data") && t.contains("Community") && t.contains("is new to Office store") && t.contains("index.html") && t.contains("app.js"), "{t}");
    assert!(!t.contains(&sw.ptoken), "the token never appears");
    let labels: Vec<String> = modal_card(&app).unwrap().card.actions.iter().map(|a| a.label.clone()).collect();
    assert_eq!(labels, vec!["Approve and publish", "Copy key for the store admin", "Cancel"]);
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &cx, wd, ht);
        card_fits(&app, wd, ht, "review card");
    }
    // nothing was sent yet
    assert!(sw.srv.audit_lines().iter().all(|l| !l.contains(" publish ")));
    key(&mut app, &cx, Key::Char('k'));
    assert!(modal_card(&app).is_some(), "copying the key keeps the card");
    assert!(crate::ui::clipboard::latest().is_some_and(|c| c.ends_with(" publisher")), "{:?}", crate::ui::clipboard::latest());

    // 5. approve: published, with the withdraw note; the header says so
    key(&mut app, &cx, Key::Char('y'));
    let t = card_text(&app);
    assert!(t.contains("Published") && t.contains("office-clock 0.1.0 is listed in Office store as Community") && t.contains("does NOT remove copies already installed"), "{t}");
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &cx, wd, ht);
        card_fits(&app, wd, ht, "result card");
    }
    assert!(sw.srv.audit_lines().iter().any(|l| l.contains("office-clock 0.1.0")));
    assert!(!sw.srv.audit_lines().join("\n").contains(&sw.ptoken));
    ticks(&mut app, &cx, 31);
    assert_eq!(app.last_hint.as_deref(), Some("Published: 0.1.0 \u{2192} Office store"));
    let g = render(&mut app, &cx, 1920, 1080);
    let _ = g;
    let log = std::fs::read_to_string(app_dir.join(".iyakki/publish.json")).unwrap();
    assert!(log.contains("0.1.0") && !log.contains(&sw.ptoken), "{log}");

    // 6. install it back from the store on this device: the normal review, then the tile appears and is opened
    key(&mut app, &cx, Key::Char('i'));
    let t = card_text(&app);
    assert!(t.contains("Install office-clock from the store?") && t.contains("Office clock"), "{t}");
    key(&mut app, &cx, Key::Char('y'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    assert_eq!(app.request.take().map(|r| r.0), Some("tile-open:office-clock".to_string()));
    let tiles = tilehub::installed(&cx.services, &cx.session.policy, "tester");
    assert_eq!(tiles.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["office-clock"]);

    // 7. publishing the same version again: refused with the way out, and after a bump the header shows the update and the key is no longer new
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let Some(Modal::Form(FormKind::Publish, f)) = &mut app.modal else { panic!() };
    f.set_choice(1, 0);
    key(&mut app, &cx, Key::Ctrl('s'));
    let t = card_text(&app);
    assert!(t.contains("Not ready to publish") && t.contains("not newer than 0.1.0") && t.contains("versions only go up") && t.contains("0.1.1"), "{t}");
    key(&mut app, &cx, Key::Esc);
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let Some(Modal::Form(FormKind::Publish, f)) = &mut app.modal else { panic!() };
    assert!(f.fields[1].label.contains("the store has 0.1.0"), "{}", f.fields[1].label);
    assert_eq!(f.choice_idx(1), 1, "a patch is suggested when the store already has this version");
    // a bump needs its change log line
    key(&mut app, &cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("stays in the form") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("change log")), "{:?}", f.error);
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(2, "dimmer at night");
    f.set_choice(3, 0);
    key(&mut app, &cx, Key::Ctrl('s'));
    let t = card_text(&app);
    assert!(t.contains("Publish office-clock 0.1.1 to Office store?") && !t.contains("is new to Office store") && t.contains("The store has 0.1.0 now; this is higher"), "{t}");
    assert!(std::fs::read_to_string(app_dir.join("CHANGELOG.md")).unwrap().contains("## 0.1.1") && std::fs::read_to_string(app_dir.join("app.json")).unwrap().contains("0.1.1"));
    key(&mut app, &cx, Key::Esc);
    ticks(&mut app, &cx, 31);
    assert!(app.last_hint.as_deref().unwrap().contains("update available"), "{:?}", app.last_hint);
}

#[test]
fn the_export_choice_writes_a_signed_package_file_and_a_new_store_opens_the_platform_wizard() {
    let w = world("tile-export");
    let sw = store_world(&w, "exp");
    let cx = sw.cx.clone();
    let d = project(&w, "alpha", &[]);
    iyakki_agent::store::author::scaffold("tile", "office-clock", &d, "").unwrap();
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    let mut app = IdeApp::new(&cx);
    open(&mut app, &cx, "Alpha", &d);
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let Some(Modal::Form(FormKind::Publish, f)) = &mut app.modal else { panic!() };
    let n = match &f.fields[0].kind {
        crate::apps::widgets::FieldKind::Choice(o) => o.len(),
        _ => 0,
    };
    f.set_choice(0, n - 1);
    key(&mut app, &cx, Key::Ctrl('s'));
    assert!(card_text(&app).contains("Package file written"), "{}", card_text(&app));
    let out = d.join("office-clock/office-clock-0.1.0.iyp");
    let pk = iyakki_agent::pkg::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert!(pk.manifest.signer.is_some() && pk.manifest.kind == "tile");
    assert!(sw.srv.audit_lines().iter().all(|l| !l.contains(" publish ")), "nothing was sent to any store");
    key(&mut app, &cx, Key::Esc);
    // deploy a new store: the wizard, then back
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let Some(Modal::Form(FormKind::Publish, f)) = &mut app.modal else { panic!() };
    f.set_choice(0, n - 2);
    key(&mut app, &cx, Key::Ctrl('s'));
    assert_eq!(app.request.take(), Some(("settings".to_string(), "platform:private-store".to_string())));
    assert!(app.notice.as_ref().unwrap().0.contains("press Publish again"));
}

/// Every header button is on the screen and none sits on another.
fn continue_header_checks(g: &Gfx, wd: u32, ht: u32) {
    let top: Vec<(String, R)> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_TOP..T_TOP + 100).contains(&n)) && h.rect.y < 70.0).map(|h| (h.label.clone(), h.rect)).collect();
    for (i, (la, a)) in top.iter().enumerate() {
        assert!(a.x >= 0.0 && a.x + a.w <= wd as f32 + 0.5, "{wd}x{ht} {la} {a:?}");
        assert!(top[i + 1..].iter().all(|(_, b)| a.x + a.w <= b.x + 1.0 || b.x + b.w <= a.x + 1.0 || a.y + a.h <= b.y || b.y + b.h <= a.y), "{wd}x{ht} overlap {la} {a:?} with {:?}", top[i + 1..].iter().map(|(l, b)| (l.clone(), *b)).collect::<Vec<_>>());
    }
}

#[test]
fn publish_offers_this_device_in_developer_mode_and_installs_through_the_same_path_as_iyakki_dev() {
    let w = world("tile-devmode");
    let cx = w.cx.clone();
    let d = project(&w, "alpha", &[]);
    iyakki_agent::store::author::scaffold("tile", "desk-clock", &d, "").unwrap();
    let app_dir = d.join("desk-clock");
    std::fs::write(app_dir.join("CHANGELOG.md"), "# Changelog\n\n## 0.1.0 - first\n- first\n").unwrap();
    let t = std::fs::read_to_string(app_dir.join("iyakki-app.json")).unwrap().replace("A tile app for the IYAKKI launcher.", "A clock for the desk.");
    std::fs::write(app_dir.join("iyakki-app.json"), t).unwrap();
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    let mut app = IdeApp::new(&cx);
    open(&mut app, &cx, "Alpha", &d);
    let choices = |app: &IdeApp| -> Vec<String> {
        let Some(Modal::Form(FormKind::Publish, f)) = &app.modal else { panic!("the publish form: {:?}", card_text(app)) };
        match &f.fields[0].kind {
            crate::apps::widgets::FieldKind::Choice(o) => o.clone(),
            _ => panic!(),
        }
    };
    // developer mode off: no such target
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    assert!(!choices(&app).iter().any(|o| o.contains("developer mode")), "{:?}", choices(&app));
    key(&mut app, &cx, Key::Esc);
    // on: "This device (developer mode)" sits with the stores
    iyakki_agent::devmode::DevMode::open(cx.services.dir()).enable(&iyakki_sec::policy::Role::Admin.template(), "root", 0).unwrap();
    app.handle(&Event::Tab(T_TOP + 14), &cx);
    let opts = choices(&app);
    let i = opts.iter().position(|o| o == "This device (developer mode)").unwrap_or_else(|| panic!("{opts:?}"));
    form_fits(&mut app, &cx, "publish form with developer targets");
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_choice(0, i);
    key(&mut app, &cx, Key::Ctrl('s'));
    let t = card_text(&app);
    assert!(t.contains("Installed in developer mode") && t.contains("tile:desk-clock 0.1.0 installed on this device"), "{t}");
    let store = Store::for_services(&cx.services);
    let rec = store.records().into_values().find(|r| r.name == "desk-clock").expect("installed by the STORE pipeline");
    assert_eq!((rec.source.as_str(), rec.level.as_str()), ("developer:this-device-ide", "developer"));
    assert!(tilehub::installed(&cx.services, &cx.session.policy, &cx.session.user).iter().any(|e| e.name == "desk-clock" && e.is_developer()), "a tile labelled Developer (local)");
    assert_eq!(app.request.as_ref().map(|r| r.0.as_str()), Some("tile-open:desk-clock"), "it opens straight away");
    for (wd, ht) in SIZES {
        let _ = render(&mut app, &cx, wd, ht);
        card_fits(&app, wd, ht, "developer install card");
    }
    let acts = iyakki_agent::devmode::DevMode::open(cx.services.dir()).activity(10);
    assert!(acts.iter().any(|a| a.action == "dev.install" && a.ok && a.who == "this-device-ide"), "logged like a remote install");
}
