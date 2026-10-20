// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from ide_tests.rs); the marker keeps the BOUNDARY check from counting it.
//! Tests of the AI IDE's Browser tab (docs/AI-IDE.md): its place in the tab strip, geometry, who owns the keyboard, click routing,
//! "Open in browser" from the cluster view, the assistant's tools with the fake engine helper (takeover, crash recovery) and the
//! remembered page. (A child module of `ide_tests`, which has the helpers.)

use super::*;
use crate::apps::component::{route, Component, EmbedMode, EmbedParams, HostEvent};
use crate::apps::ide_browser::{self as ib, IdeBrowser};
use crate::apps::web_agent;

const APP: &str = "http://192.168.1.69:30929/attendence";

fn fake_helper() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let p = dir.join("iyakki-fake-webview");
    if !p.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = std::process::Command::new(cargo).args(["build", "-q", "-p", "iyakki-webview", "--bin", "iyakki-fake-webview"]).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok && p.is_file(), "could not build the fake helper");
    }
    p
}

/// A world whose person has a name of their own: the assistant's tab is one per person, and tests run side by side.
fn bworld(tag: &str) -> World {
    let mut w = world(tag);
    // the fake helper stands in for the installed engine
    iyakki_agent::browsertools::assume_engine(true);
    w.cx.session.user = format!("ibt-{tag}");
    w.cx.session.policy.browser_agent = iyakki_sec::policy::BrowserAgent::Interactive;
    w.cx.session.policy.net_hosts = vec!["*".into()];
    w.cx.services.browser.set_wait(Duration::from_secs(20));
    w
}

fn pump(app: &mut IdeApp, cx: &AppCx) {
    web_agent::tick(&cx.services, &cx.audit, &cx.session);
    app.handle(&Event::Tick, cx);
}

fn pump_until(app: &mut IdeApp, cx: &AppCx, what: &str, mut f: impl FnMut(&mut IdeApp) -> bool) {
    for i in 0..1500 {
        web_agent::tick(&cx.services, &cx.audit, &cx.session);
        app.handle(&Event::Tick, cx);
        // drawing is slow in a debug build; the engine only needs a frame now and then
        if i % 40 == 0 {
            render(app, cx, 400, 300);
        }
        if f(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for: {what}; page_view {:?} problem {:?} tab {:?}", web_agent::page_view(&cx.session.user), web_agent::problem(&cx.session.user), app.ws[0].main_tab);
}

fn bhits(g: &Gfx) -> Vec<&crate::ui::Hit> {
    g.hits.iter().filter(|h| route::is_slot(&h.ev, route::BROWSER)).collect()
}

fn setup(tag: &str) -> (World, IdeApp) {
    let w = bworld(tag);
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    (w, app)
}

// ------------------------------------------------------------------------------------------------ the tab

#[test]
fn browser_is_a_main_tab_between_board_and_k8s_with_a_globe_and_a_shortcut() {
    assert_eq!(MainTab::ALL, [MainTab::Editor, MainTab::Board, MainTab::Tasks, MainTab::Notes, MainTab::Browser, MainTab::K8s]);
    assert_eq!(MainTab::ALL.iter().map(|t| t.label()).collect::<Vec<_>>(), ["Editor", "Board", "Tasks", "Context", "Browser", "K8s"]);
    assert_eq!(super::super::ide_ui::main_glyph(MainTab::Browser), crate::fx::Glyph::Globe);
    let (w, mut app) = setup("tab");
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600)] {
        let g = render(&mut app, &w.cx, wd, ht);
        let tabs: Vec<(&str, f32)> = (0..6).filter_map(|i| g.hits.iter().find(|h| h.ev == Event::Tab(T_MAIN + i)).map(|h| (h.label.as_str(), h.rect.x))).collect();
        assert_eq!(tabs.iter().map(|t| t.0.split(' ').next().unwrap()).collect::<Vec<_>>(), ["Editor", "Board", "Tasks", "Context", "Browser", "K8s"], "{wd}x{ht}");
        assert!(tabs.windows(2).all(|p| p[0].1 < p[1].1), "left to right in that order: {tabs:?}");
    }
    // clicking the tab shows it; Ctrl+Shift+O toggles it and Esc-less: the second press goes back to the editor
    let g = render(&mut app, &w.cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.label == "Browser" && matches!(h.ev, Event::Tab(_))).unwrap().ev.clone();
    app.handle(&ev, &w.cx);
    assert_eq!(cur(&app).main_tab, MainTab::Browser);
    app.show_main(MainTab::Editor);
    key(&mut app, &w.cx, Key::Ctrl('O'));
    assert_eq!(cur(&app).main_tab, MainTab::Browser);
    assert_eq!(cur(&app).focus, Pane::Main);
    key(&mut app, &w.cx, Key::Ctrl('O'));
    assert_eq!(cur(&app).main_tab, MainTab::Editor);
}

#[test]
fn there_is_one_browser_per_workspace_and_opening_again_reuses_it() {
    let (w, mut app) = setup("single");
    app.open_browser(Some("http://localhost:3000/"), &w.cx);
    let first = cur(&app).browser.as_ref().map(|b| b as *const IdeBrowser as usize).unwrap();
    app.open_browser(Some("http://localhost:3001/"), &w.cx);
    app.show_main(MainTab::Browser);
    let again = cur(&app).browser.as_ref().map(|b| b as *const IdeBrowser as usize).unwrap();
    assert_eq!(first, again, "the same browser surface");
    assert_eq!(cur(&app).browser.as_ref().unwrap().address_text(), "http://localhost:3001/");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert_eq!(g.hits.iter().filter(|h| h.label == "Browser" && matches!(h.ev, Event::Tab(_))).count(), 1, "one Browser tab, no second surface in the bottom panel");
    assert_eq!(g.hits.iter().filter(|h| h.label == "Reload").count(), 1, "one toolbar");
}

#[test]
fn an_account_without_the_browser_app_gets_a_note_and_no_page() {
    let (mut w, mut app) = setup("noapp");
    w.cx.session.policy.apps.remove(&PApp::Browser);
    app.open_browser(Some(APP), &w.cx);
    assert!(cur(&app).browser.is_none());
    assert!(app.notice.as_ref().is_some_and(|n| n.0.contains("may not use the browser")));
    app.show_main(MainTab::Browser);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(bhits(&g).is_empty());
}

// ------------------------------------------------------------------------------------------------ geometry

#[test]
fn the_toolbar_parts_never_overlap_or_leave_the_rectangle() {
    for s in [0.7f32, 1.0, 1.5, 2.0] {
        for w in [300.0f32, 420.0, 600.0, 900.0, 1500.0] {
            for chips in [false, true] {
                for notes in [None, Some(70.0 * s)] {
                    let rect = R::new(10.0, 20.0, w * s, 400.0 * s);
                    let p = ib::parts(rect, s, 90.0 * s, notes, chips);
                    let mut row = vec![p.back, p.forward, p.reload, p.addr, p.mode, p.external];
                    row.extend(p.notes);
                    for r in &row {
                        assert!(r.x >= rect.x - 0.01 && r.x + r.w <= rect.x + rect.w + 0.01, "{s} {w}: {r:?} inside {rect:?}");
                        assert!(r.y >= p.toolbar.y - 0.01 && r.y + r.h <= p.toolbar.y + p.toolbar.h + 0.01);
                    }
                    for (i, a) in row.iter().enumerate() {
                        for b in &row[i + 1..] {
                            assert!(a.x + a.w <= b.x + 0.01 || b.x + b.w <= a.x + 0.01, "{s} {w}: {a:?} overlaps {b:?}");
                        }
                    }
                    assert!(p.addr.w >= 20.0);
                    assert!(p.body.y >= p.toolbar.y + p.toolbar.h - 0.01 && p.body.y + p.body.h <= rect.y + rect.h + 0.01);
                    if let Some(c) = p.chips {
                        assert!(c.y >= p.toolbar.y + p.toolbar.h - 0.01 && p.body.y >= c.y + c.h - 0.01);
                    }
                }
            }
        }
    }
}

#[test]
fn drawn_in_the_ide_the_browser_stays_inside_the_main_pane_at_every_size_and_density() {
    use super::super::ide_chrome::Density;
    let (w, mut app) = setup("geom");
    app.open_browser(None, &w.cx);
    cur_mut(&mut app).browser.as_mut().unwrap().set_chips(vec![ib::Chip { label: "web · 192.168.1.50:80".into(), url: "http://192.168.1.50:80".into() }, ib::Chip { label: "localhost:3000".into(), url: "http://localhost:3000/".into() }]);
    for d in [Density::Comfortable, Density::Compact, Density::Dense] {
        app.set_density(d);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600)] {
            let g = render(&mut app, &w.cx, wd, ht);
            let main = app.layout.as_ref().unwrap().main;
            let mine = bhits(&g);
            assert!(mine.len() >= 6, "{d:?} {wd}x{ht}: toolbar buttons are there: {:?}", mine.iter().map(|h| &h.label).collect::<Vec<_>>());
            for h in &mine {
                assert!(h.rect.x >= main.x - 1.0 && h.rect.y >= main.y - 1.0 && h.rect.x + h.rect.w <= main.x + main.w + 1.0 && h.rect.y + h.rect.h <= main.y + main.h + 1.0, "{d:?} {wd}x{ht}: {} {:?} outside {main:?}", h.label, h.rect);
            }
            // two controls of the toolbar never share space
            let tool: Vec<_> = mine.iter().filter(|h| ["Back", "Forward", "Reload", "Open in the Browser tile"].contains(&h.label.as_str()) || h.label.contains("engine") || h.label.starts_with("Reader")).collect();
            for (i, a) in tool.iter().enumerate() {
                for b in &tool[i + 1..] {
                    assert!(a.rect.x + a.rect.w <= b.rect.x + 0.5 || b.rect.x + b.rect.w <= a.rect.x + 0.5 || a.rect.y + a.rect.h <= b.rect.y + 0.5 || b.rect.y + b.rect.h <= a.rect.y + 0.5, "{d:?} {wd}x{ht}: {} overlaps {}", a.label, b.label);
                }
            }
            assert!(route::duplicate_ids(&g.hits).is_empty(), "{d:?} {wd}x{ht}: {:?}", route::duplicate_ids(&g.hits));
        }
    }
}

#[test]
fn the_browsers_click_ids_live_in_their_own_range_and_never_collide_with_the_ides() {
    let (w, mut app) = setup("ids");
    app.open_browser(None, &w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    let range = route::base(route::BROWSER)..route::base(route::BROWSER + 1);
    let id = |e: &Event| match e {
        Event::Tab(n) | Event::Row(n) | Event::Focus(n) | Event::Area(n) | Event::FocusAt(n, _) | Event::AreaAt(n, ..) => Some(*n),
        _ => None,
    };
    let inside: Vec<_> = g.hits.iter().filter(|h| id(&h.ev).is_some_and(|n| range.contains(&n))).collect();
    assert!(inside.len() >= 3, "{:?}", inside.iter().map(|h| &h.label).collect::<Vec<_>>());
    // everything else that was drawn (editor excluded: it is hidden) stays out of that range
    app.show_main(MainTab::Editor);
    let g2 = render(&mut app, &w.cx, 1280, 720);
    assert!(g2.hits.iter().all(|h| !id(&h.ev).is_some_and(|n| range.contains(&n))), "no other pane uses the browser's ids");
    // the editor's, the chat's and the tree's ids still decode as theirs
    for h in &g2.hits {
        if let Some((_, _)) = decode(&h.ev) {
            assert!(route::decode_slot(&h.ev, route::BROWSER).is_none(), "{:?}", h.ev);
        }
    }
    assert!(route::duplicate_ids(&g.hits).is_empty());
}

// ------------------------------------------------------------------------------------------------ the keyboard

#[test]
fn the_address_field_owns_text_the_page_owns_keys_and_the_editor_and_composer_are_untouched() {
    let (w, mut app) = setup("keys");
    app.open_file("a.py", None, &w.cx);
    let editor_before = cur(&app).editor.text();
    app.open_browser(None, &w.cx);
    assert_eq!(cur(&app).owner(), Owner::BrowserAddr, "an empty browser starts in its address field");
    assert!(Owner::BrowserAddr.is_text() && !Owner::BrowserPage.is_text());
    crate::ui::take_carets();
    render_hovering(&mut app, &w.cx, 1280, 720, 1, (5, 5));
    assert_eq!(crate::ui::take_carets(), 1, "exactly one caret: the address field's");
    typ(&mut app, &w.cx, "localhost:8080/x");
    assert_eq!(cur(&app).browser.as_ref().unwrap().address_text(), "localhost:8080/x");
    assert_eq!(cur(&app).editor.text(), editor_before, "typing in the address never reaches the editor");
    assert!(cur(&app).chat.input_text().is_empty());
    // a paste goes to the address, not to the composer
    app.handle(&Event::Paste("?q=1".into()), &w.cx);
    assert!(cur(&app).browser.as_ref().unwrap().address_text().ends_with("?q=1"));
    assert!(cur(&app).chat.input_text().is_empty());
    // Esc leaves the field (the page is the owner: no caret), Esc again hands the keyboard to the tree
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).owner(), Owner::BrowserPage);
    crate::ui::take_carets();
    render_hovering(&mut app, &w.cx, 1280, 720, 1, (5, 5));
    assert_eq!(crate::ui::take_carets(), 0, "the page is a pane owner: no IDE caret");
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).focus, Pane::Tree);
    // with the keyboard on the composer, typing never goes to the browser
    app.focus_pane(Pane::Chat);
    typ(&mut app, &w.cx, "hello");
    assert_eq!(cur(&app).chat.input_text(), "hello");
    assert!(!cur(&app).browser.as_ref().unwrap().address_text().contains("hello"));
    app.handle(&Event::Paste("pasted".into()), &w.cx);
    assert!(!cur(&app).browser.as_ref().unwrap().address_text().contains("pasted"), "a paste goes to the owner only");
    // Ctrl+L from the page goes to the address
    app.show_main(MainTab::Browser);
    key(&mut app, &w.cx, Key::Ctrl('l'));
    assert_eq!(cur(&app).owner(), Owner::BrowserAddr);
}

#[test]
fn enter_in_the_address_field_navigates_and_a_bad_address_is_refused_with_a_reason() {
    let (w, mut app) = setup("addr");
    app.open_browser(None, &w.cx);
    typ(&mut app, &w.cx, "javascript:alert(1)");
    key(&mut app, &w.cx, Key::Enter);
    assert!(cur(&app).browser.as_ref().unwrap().note().is_some_and(|n| n.contains("http")), "{:?}", cur(&app).browser.as_ref().unwrap().note());
    // an address of a host the account may not open
    let (mut w2, mut app2) = setup("addr2");
    w2.cx.session.policy.net_hosts = vec!["only.example".into()];
    app2.open_browser(None, &w2.cx);
    typ(&mut app2, &w2.cx, "blocked.example/x");
    key(&mut app2, &w2.cx, Key::Enter);
    assert!(cur(&app2).browser.as_ref().unwrap().note().is_some_and(|n| n.contains("may not open")), "{:?}", cur(&app2).browser.as_ref().unwrap().note());
}

// ------------------------------------------------------------------------------------------------ routing in rectangle coordinates

#[test]
fn clicks_on_the_toolbar_reach_the_browser_and_clicks_elsewhere_do_not() {
    let (w, mut app) = setup("route");
    app.open_file("a.py", None, &w.cx);
    app.open_browser(None, &w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    // the address field: its centre is a click on it
    let addr = g.hits.iter().find(|h| matches!(h.ev, Event::Focus(n) if n == route::base(route::BROWSER) + ib::FOCUS_ADDR)).expect("the address field registers a click region");
    assert!(app.layout.as_ref().unwrap().main.contains((addr.rect.x + 2.0) as i32, (addr.rect.y + 2.0) as i32), "inside the main pane");
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).owner(), Owner::BrowserPage);
    app.handle(&Event::FocusAt(route::base(route::BROWSER) + ib::FOCUS_ADDR, 5.0), &w.cx);
    assert_eq!(cur(&app).owner(), Owner::BrowserAddr);
    // the mode button toggles Reader and Full (no engine here: it says so instead of switching)
    let mode = g.hits.iter().find(|h| h.label.contains("engine") || h.label.starts_with("Reader")).unwrap().ev.clone();
    app.handle(&mode, &w.cx);
    assert!(cur(&app).browser.as_ref().unwrap().note().is_some_and(|n| n.contains("not installed") || n.contains("engine")), "{:?}", cur(&app).browser.as_ref().unwrap().note());
    // the editor's own ids never reach the browser
    let before = cur(&app).browser.as_ref().unwrap().address_text().to_string();
    app.handle(&Event::Tab(2 * STRIDE + 7), &w.cx);
    app.handle(&Event::Row(3 * STRIDE + 1), &w.cx);
    assert_eq!(cur(&app).browser.as_ref().unwrap().address_text(), before);
    assert_ne!(cur(&app).focus, Pane::Bottom);
    // an open-externally click asks the shell for the BROWSER tile on that page
    app.open_browser(Some("http://localhost:4000/"), &w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    let ext = g.hits.iter().find(|h| h.label == "Open in the Browser tile").unwrap().ev.clone();
    cur_mut(&mut app).browser.as_mut().unwrap().set_chips(vec![]);
    app.handle(&ext, &w.cx);
    ticks(&mut app, &w.cx, 3);
    let req = app.take_request();
    assert!(req.as_ref().is_some_and(|r| r.0 == "browser"), "{req:?}");
}

#[test]
fn the_address_buttons_come_from_the_clusters_services_dev_servers_and_the_last_page() {
    let c = ib::chips_for(Some("http://192.168.1.69:30929/attendence"), &[("web".into(), "http://192.168.1.50:80".into())], &["http://localhost:3000/".into(), "http://192.168.1.50:80/".into()]);
    let urls: Vec<&str> = c.iter().map(|c| c.url.as_str()).collect();
    assert_eq!(urls, ["http://192.168.1.50:80", "http://localhost:3000/", "http://192.168.1.69:30929/attendence"], "each address once, services first");
    assert_eq!(c[0].label, "web · 192.168.1.50:80");
    assert!(c[2].label.starts_with("last: "));
    let many: Vec<(String, String)> = (0..20).map(|i| (format!("s{i}"), format!("http://10.0.0.{i}:80"))).collect();
    assert_eq!(ib::chips_for(None, &many, &[]).len(), 8, "at most eight");
}

// ------------------------------------------------------------------------------------------------ open from the cluster view

#[test]
fn open_in_browser_on_a_services_row_shows_its_address_in_the_browser_tab() {
    let _k = fake_cluster();
    let (w, mut app) = setup("k8sopen");
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    // the chips come from the Services (the LoadBalancer "web" has an outside address; "api" is cluster-internal)
    app.show_main(MainTab::Browser);
    ticks(&mut app, &w.cx, 3);
    assert!(cur(&app).browser.is_some() || { render(&mut app, &w.cx, 1280, 720); true });
    ticks(&mut app, &w.cx, 3);
    let chips: Vec<String> = cur(&app).browser.as_ref().unwrap().chips().iter().map(|c| c.url.clone()).collect();
    assert!(chips.iter().any(|u| u == "http://192.168.1.50:80"), "{chips:?}");
    // the K8s view: Tab to Services, o on the selected row
    app.show_main(MainTab::K8s);
    key(&mut app, &w.cx, Key::Tab);
    assert_eq!(cur(&app).k8s.as_ref().unwrap().kind, super::super::ide_k8s::Kind::Services);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Open in browser  o"), "the K8s toolbar offers it");
    key(&mut app, &w.cx, Key::Char('o'));
    ticks(&mut app, &w.cx, 3);
    assert_eq!(cur(&app).main_tab, MainTab::Browser, "{:?}", cur(&app).k8s.as_ref().unwrap().status);
    assert_eq!(cur(&app).browser.as_ref().unwrap().address_text(), "http://192.168.1.50:80");
    // a Service with no outside address says why and opens nothing
    app.show_main(MainTab::K8s);
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Char('o'));
    ticks(&mut app, &w.cx, 3);
    assert_eq!(cur(&app).main_tab, MainTab::K8s);
    assert!(cur(&app).k8s.as_ref().unwrap().status.as_ref().is_some_and(|s| s.contains("no address outside")), "{:?}", cur(&app).k8s.as_ref().unwrap().status);
    // on another kind of row it explains what to select
    key(&mut app, &w.cx, Key::BackTab);
    key(&mut app, &w.cx, Key::Char('o'));
    assert!(cur(&app).k8s.as_ref().unwrap().status.as_ref().is_some_and(|s| s.contains("Select a Service")));
}

// ------------------------------------------------------------------------------------------------ remembered page

#[test]
fn the_last_page_is_kept_in_project_json_without_a_query_and_restored_next_time() {
    let (w, mut app) = setup("persist");
    let root = cur(&app).root();
    let helper = fake_helper();
    web_agent::use_helper_for(&w.cx.session.user, helper);
    app.open_browser(Some("http://192.168.1.69:30929/attendence?token=abc#top"), &w.cx);
    assert_eq!(cur(&app).browser.as_ref().unwrap().mode(), EmbedMode::Full);
    pump_until(&mut app, &w.cx, "the page to arrive", |a| a.ws[0].cfg.preview.url == APP);
    let text = std::fs::read_to_string(workspace::ProjectConfig::path(&root)).unwrap();
    assert!(text.contains("\"preview\"") && text.contains(APP) && !text.contains("token") && !text.contains("#top"), "{text}");
    assert_eq!(workspace::ProjectConfig::load(&root).unwrap().preview.url, APP);
    // a new session shows it again on the first use of the tab
    drop(app);
    let mut app2 = IdeApp::new(&w.cx);
    assert_eq!(cur(&app2).cfg.preview.url, APP);
    app2.show_main(MainTab::Browser);
    render(&mut app2, &w.cx, 1280, 720);
    assert_eq!(cur(&app2).browser.as_ref().unwrap().address_text(), APP);
    // a workspace that has no preview does not grow a "preview" entry
    let d2 = project(&w, "beta", &[]);
    open(&mut app2, &w.cx, "Beta", &d2);
    cur_mut(&mut app2).cfg.save(&d2).unwrap();
    assert!(!std::fs::read_to_string(workspace::ProjectConfig::path(&d2)).unwrap().contains("preview"));
    // only web addresses are kept
    assert_eq!(ib::persistable("javascript:alert(1)"), None);
    assert_eq!(ib::persistable("http://u:p@h/x"), None);
    assert_eq!(ib::persistable("https://h/a?x=1"), Some("https://h/a".into()));
}

// ------------------------------------------------------------------------------------------------ the assistant, with the fake helper

fn ask(app: &mut IdeApp, cx: &AppCx, tool: &str, args: serde_json::Value) {
    cur_mut(app).chat.ask(cx, &format!("do:{tool}|{args}"));
}

#[test]
fn the_assistant_opens_the_page_in_the_ide_and_the_person_watches_and_can_take_over() {
    let _k = fake_cluster();
    let (w, mut app) = setup("ai");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, fake_helper());
    // the workspace's cluster is known (its Service "web" is at 192.168.1.50): that address is the workspace's own
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    app.show_main(MainTab::Editor);
    app.focus_pane(Pane::Chat);
    pump(&mut app, &w.cx);
    ask(&mut app, &w.cx, "ide_browser_open", serde_json::json!({"url": "192.168.1.50/attendence"}));
    let mut asked = 0;
    pump_until(&mut app, &w.cx, "the Browser tab to show the page", |a| {
        asked += w.cx.services.browser.consent.pending(&user).len();
        a.ws[0].main_tab == MainTab::Browser && web_agent::page_view(&user).is_some_and(|v| v.0 == "http://192.168.1.50/attendence")
    });
    assert_eq!(asked, 0, "the workspace's own address needs no look question");
    // the keyboard stayed with the composer: the page did not take it
    assert_eq!(cur(&app).focus, Pane::Chat);
    assert_eq!(cur(&app).owner(), Owner::Composer);
    assert_eq!(cur(&app).browser.as_ref().unwrap().mode(), EmbedMode::Full);
    // the banner says the assistant is controlling the page, with Stop and Take over
    w.cx.services.browser.note_doing(&user, "click button \"Send\"", "192.168.1.69");
    let g = render(&mut app, &w.cx, 1280, 720);
    let labels: Vec<&str> = bhits(&g).iter().map(|h| h.label.as_str()).collect();
    assert!(labels.contains(&"Stop") && labels.contains(&"Take over"), "{labels:?}");
    assert!(!labels.contains(&"My tabs"), "no way out of the embedded view: the IDE has its own tabs");
    // the person clicks on the page: the assistant is paused, and its next step is told so
    let area = bhits(&g).iter().find(|h| matches!(h.ev, Event::Area(_) | Event::AreaAt(..))).expect("the page area").ev.clone();
    let area = match area {
        Event::Area(n) | Event::AreaAt(n, ..) => Event::AreaAt(n, 20.0, 20.0),
        e => e,
    };
    app.handle(&area, &w.cx);
    assert!(w.cx.services.browser.control_view(&user).taken_over, "a click by the person takes over");
    assert_eq!(cur(&app).owner(), Owner::BrowserPage, "the page owns the keyboard now");
    assert!(w.cx.services.browser.check_control(&user).is_err());
    // typing goes to the page and nowhere else
    let composer = cur(&app).chat.input_text().to_string();
    key(&mut app, &w.cx, Key::Char('x'));
    assert_eq!(cur(&app).chat.input_text(), composer);
    // Resume from the banner lets the assistant go on
    let g = render(&mut app, &w.cx, 1280, 720);
    let resume = bhits(&g).iter().find(|h| h.label == "Resume").expect("Resume is offered").ev.clone();
    app.handle(&resume, &w.cx);
    assert!(!w.cx.services.browser.control_view(&user).taken_over);
    pump_until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
}

#[test]
fn a_readonly_view_ignores_the_persons_clicks_and_keys_on_the_page() {
    let (w, mut app) = setup("readonly");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, fake_helper());
    app.open_browser(None, &w.cx);
    let b = cur_mut(&mut app).browser.as_mut().unwrap();
    b.set_params(&serde_json::json!({"readonly": true, "mode": "full"}), &w.cx).unwrap();
    assert!(b.readonly());
    let before = w.cx.services.browser.jobs.pending_for(&user);
    b.focus_address_for_test(false);
    assert!(b.handle(&Event::Key(Key::Char('a')), &w.cx), "swallowed, not passed on");
    assert!(b.handle(&Event::AreaAt(web_agent::AREA_AGENT, 3.0, 3.0), &w.cx));
    assert_eq!(w.cx.services.browser.jobs.pending_for(&user), before);
    assert_eq!(b.params()["readonly"], true);
    assert!(b.set_params(&serde_json::json!({"mode": "pdf"}), &w.cx).is_err());
}

#[test]
fn a_crashed_engine_shows_a_notice_with_reload_and_the_ide_keeps_working() {
    let (w, mut app) = setup("crash");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, fake_helper());
    app.open_browser(Some("http://192.168.1.69:30929/attendence"), &w.cx);
    pump_until(&mut app, &w.cx, "the first page", |_a| web_agent::page_view(&user).is_some_and(|v| v.0.starts_with("http://192.168.1.69")));
    // the helper dies while a script runs
    let jobs = w.cx.services.browser.jobs.clone();
    let u2 = user.clone();
    let h = std::thread::spawn(move || jobs.call(&u2, iyakki_agent::browserjobs::Op::Eval { js: "EXIT".into(), timeout_ms: 5000 }, Duration::from_secs(30)));
    pump_until(&mut app, &w.cx, "the crash to be noticed", |a| a.ws[0].browser.as_ref().unwrap().crashed().is_some());
    let _ = h.join();
    let g = render(&mut app, &w.cx, 1280, 720);
    let labels: Vec<&str> = bhits(&g).iter().map(|h| h.label.as_str()).collect();
    assert!(labels.contains(&"Reload"), "{labels:?}");
    assert!(cur(&app).browser.as_ref().unwrap().crashed().is_some_and(|c| c.contains("browser engine stopped")));
    // the rest of the IDE is fine: the editor takes keys
    app.open_file("a.py", None, &w.cx);
    app.show_main(MainTab::Editor);
    app.focus_pane(Pane::Main);
    key(&mut app, &w.cx, Key::Char('z'));
    assert!(cur(&app).editor.text().contains('z'));
    // Reload starts a fresh engine on the same page
    app.show_main(MainTab::Browser);
    let reload = bhits(&render(&mut app, &w.cx, 1280, 720)).iter().find(|h| h.label == "Reload").unwrap().ev.clone();
    app.handle(&reload, &w.cx);
    pump_until(&mut app, &w.cx, "the engine to come back", |a| a.ws[0].browser.as_ref().unwrap().crashed().is_none() && web_agent::page_view(&user).is_some_and(|v| v.0.starts_with("http://192.168.1.69")));
}

#[test]
fn the_reader_hands_a_page_that_needs_the_engine_to_the_one_shared_engine() {
    let (w, mut app) = setup("handoff");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, fake_helper());
    app.open_browser(None, &w.cx);
    {
        let b = cur_mut(&mut app).browser.as_mut().unwrap();
        b.set_mode(EmbedMode::Reader, &w.cx);
        assert_eq!(b.mode(), EmbedMode::Reader);
        b.set_mode(EmbedMode::Full, &w.cx);
        assert_eq!(b.mode(), EmbedMode::Full);
        b.set_mode(EmbedMode::Reader, &w.cx);
    }
    // the assistant starts acting while the reader is showing: the view switches to the engine's page
    w.cx.services.browser.note_doing(&user, "open", "192.168.1.69");
    pump_until(&mut app, &w.cx, "the view to follow the assistant", |a| a.ws[0].browser.as_ref().unwrap().mode() == EmbedMode::Full);
    // and the stand-alone browser tile never starts a second engine of its own while embedded
    assert!(!cur(&app).browser.as_ref().unwrap().note().is_some_and(|n| n.contains("second")));
}

#[test]
fn host_events_report_navigation_loading_and_close() {
    let (w, app) = setup("events");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, fake_helper());
    let mut b = IdeBrowser::new(EmbedParams { url: Some(APP.into()), mode: EmbedMode::Full, ..Default::default() }, "Alpha", &w.cx);
    let mut got: Vec<HostEvent> = Vec::new();
    for _ in 0..2000 {
        web_agent::tick(&w.cx.services, &w.cx.audit, &w.cx.session);
        b.tick(&w.cx);
        got.extend(b.drain_events());
        if got.iter().any(|e| matches!(e, HostEvent::Navigated { url, .. } if url == APP)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(got.iter().any(|e| matches!(e, HostEvent::Navigated { url, title } if url == APP && title.contains("fake page"))), "{got:?}");
    assert_eq!(b.kind(), "browser");
    assert!(b.title().contains("fake page"));
    let p = b.params();
    assert_eq!((p["url"].as_str(), p["mode"].as_str()), (Some(APP), Some("full")));
    b.close(&w.cx);
    b.close(&w.cx);
    assert_eq!(b.drain_events(), vec![HostEvent::Closed], "close is idempotent");
    drop(app);
}

// ------------------------------------------------------------------------------------------------ live (real engine)

struct NoTools;
impl iyakki_agent::agent::Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

/// Needs a display and the WebKitGTK helper of the checkout: `xvfb-run -a cargo test -p iyakki-shell --lib -- --ignored live_ide_browser`.
/// Serves a page with a small `python3` http.server, shows it in the IDE's Browser tab with the real engine, runs `ide_browser_check` on
/// it and writes a picture of the IDE to `$IYAKKI_SHOTS` (default: the temp folder).
#[test]
#[ignore = "needs a display (xvfb-run) and WebKitGTK"]
fn live_ide_browser_shows_a_local_page_and_the_check_tool_passes() {
    use iyakki_agent::agent::Tools;
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/host-helper.py");
    assert!(helper.is_file(), "{helper:?}");
    let (w, mut app) = setup("live");
    let user = w.cx.session.user.clone();
    web_agent::use_helper_for(&user, helper);
    let site = w.base.join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("attendence"), "<!doctype html><html><head><title>Attendance</title></head><body><h1>Attendance sheet</h1><label>Name <input id=n></label> <button id=b onclick=\"document.title='Marked'\">Mark present</button></body></html>").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    // python's own http.server, answering every path with the page as text/html (like an app does)
    let code = "import http.server,sys\nbody=open(sys.argv[2],'rb').read()\nclass H(http.server.BaseHTTPRequestHandler):\n  def do_GET(self):\n    self.send_response(200); self.send_header('Content-Type','text/html; charset=utf-8'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)\n  def log_message(self,*a): pass\nhttp.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),H).serve_forever()\n";
    let mut srv = std::process::Command::new("python3").args(["-c", code, &port.to_string(), &site.join("attendence").display().to_string()]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let url = format!("http://localhost:{port}/attendence");
    app.open_browser(Some(&url), &w.cx);
    let started = std::time::Instant::now();
    loop {
        web_agent::tick(&w.cx.services, &w.cx.audit, &w.cx.session);
        app.handle(&Event::Tick, &w.cx);
        if web_agent::page_view(&user).is_some_and(|v| v.1.contains("Attendance")) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the page never arrived: {:?} {:?}", web_agent::page_view(&user), web_agent::problem(&user));
        std::thread::sleep(Duration::from_millis(20));
    }
    // the tool, on a worker thread (it waits for the engine, which this thread services)
    let tools = Arc::new(iyakki_agent::idebrowser::IdeBrowserTools::new(Arc::new(NoTools), w.cx.services.clone(), w.cx.session.policy.clone(), &user, "Alpha", &w.base.join("alpha")));
    let t2 = tools.clone();
    let url2 = url.clone();
    let h = std::thread::spawn(move || t2.call("ide_browser_check", &serde_json::json!({"url": url2, "expect_text": "Attendance sheet", "expect_status": 200})));
    while !h.is_finished() {
        web_agent::tick(&w.cx.services, &w.cx.audit, &w.cx.session);
        app.handle(&Event::Tick, &w.cx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let r = h.join().unwrap().unwrap();
    eprintln!("{r}");
    assert!(r.starts_with("CHECK PASS") && r.contains("status: 200"), "{r}");
    // let a few frames arrive, then draw the IDE with the real page in it
    for _ in 0..100 {
        web_agent::tick(&w.cx.services, &w.cx.audit, &w.cx.session);
        app.handle(&Event::Tick, &w.cx);
        std::thread::sleep(Duration::from_millis(20));
    }
    let g = render(&mut app, &w.cx, 1280, 720);
    let out = std::env::var("IYAKKI_SHOTS").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    std::fs::create_dir_all(&out).unwrap();
    let png = iyakki_agent::browserjobs::png::encode_rgba(g.width(), g.height(), g.rgba());
    std::fs::write(out.join("ide-browser-live.png"), png).unwrap();
    let _ = srv.kill();
    let _ = srv.wait();
}
