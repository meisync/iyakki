//! The Settings controls that used to be reachable only through the chat (owner rule: both): the browser data choices, the order of found
//! passages and Graph RAG, the description of camera alerts, and the user theme files on the theme card. Real app, keys and clicks (the
//! hit events a click sends), the audit lines, the page layout at the five usual sizes, and the same result as the chat tool.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_shared::agent::Tools as _;
use iyakki_shared::profilepolicy::Policy;

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

const SIZES: [(u32, u32); 5] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)];

/// Every key the page offers somewhere along its scroll (the Browser page scrolls), each hit inside the window sideways.
fn keys_along_the_page(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> std::collections::BTreeSet<char> {
    let mut found = std::collections::BTreeSet::new();
    a.bh.scroll = 0.0;
    for _ in 0..40 {
        let g = draw(a, cx, w, h);
        for hit in &g.hits {
            if let Event::Key(Key::Char(c)) = hit.ev {
                let r = hit.rect;
                if r.y >= 72.0 && r.y + r.h <= h as f32 + 0.5 {
                    assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5, "{w}x{h}: {c} {r:?}");
                    found.insert(c);
                }
            }
        }
        if a.bh.scroll >= a.bh.max - 0.5 {
            break;
        }
        a.bh.scroll = (a.bh.scroll + 150.0).min(a.bh.max);
    }
    found
}

#[test]
fn the_browser_data_choices_are_keys_and_buttons_audited_and_the_same_as_the_chat_tool_sets() {
    let cx = admin("bchoice1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let dir = cx.data_dir().to_path_buf();
    assert_eq!(Policy::load(&dir, "root"), Policy::default());
    press(&mut a, &cx, 'Z');
    assert_eq!(Policy::load(&dir, "root").checkpoint_minutes, 30, "10 -> 30");
    for want in [60, 0, 5, 10] {
        press(&mut a, &cx, 'Z');
        assert_eq!(Policy::load(&dir, "root").checkpoint_minutes, want);
    }
    press(&mut a, &cx, 'L');
    press(&mut a, &cx, 'Y');
    let p = Policy::load(&dir, "root");
    assert!(p.lock_seal && p.clear_cookies_on_exit);
    // the site lists through the forms: a bad name is refused with the reason, nothing is saved
    press(&mut a, &cx, 'B');
    let Some(Modal::Form(FormKind::BrowserSites(false), _)) = &a.modal else { panic!("the form for the never-kept sites") };
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!a.has_modal(), "Esc closes the form");
    let bad = Form::new("x").text("Sites", "not a site", "");
    assert!(a.browser_sites_submit(&cx, false, &bad).unwrap_err().contains("not a site name"));
    let good = Form::new("x").text("Sites", "Shop.Example.com, example.org\nshop.example.com", "");
    assert!(a.browser_sites_submit(&cx, false, &good).unwrap().contains("2 site"));
    let good2 = Form::new("x").text("Sites", "bank.example.com", "");
    a.browser_sites_submit(&cx, true, &good2).unwrap();
    let p = Policy::load(&dir, "root");
    assert_eq!((p.never_persist.clone(), p.clear_sites_on_exit.clone()), (vec!["example.org".to_string(), "shop.example.com".to_string()], vec!["bank.example.com".to_string()]));
    assert!(cx.audit.recent(40).iter().filter(|l| l.contains("settings.browser.protection")).count() >= 7);
    assert!(!cx.audit.recent(40).iter().any(|l| l.contains("shop.example.com")), "the audit holds no site names");
    // the page shows it, and the chat tool says the same
    assert!(keys_along_the_page(&mut a, &cx, 1280, 720).contains(&'Z'));
    let (cx2, t) = chat_rig(&cx);
    let said = t.call("browser_protection_choices", &serde_json::json!({})).unwrap();
    assert!(said.contains(&p.summary()), "{said}");
    // and the other way round: what the chat sets is what the page then shows
    t.call("browser_protection_choices_set", &serde_json::json!({"lock_seal": false, "checkpoint_minutes": 0, "never_persist": ["a.example.com"]})).unwrap();
    let after = Policy::load(&dir, "root");
    assert!(!after.lock_seal && after.checkpoint_minutes == 0 && after.never_persist == ["a.example.com"] && after.clear_cookies_on_exit);
    press(&mut a, &cx, 'L');
    assert!(Policy::load(&dir, "root").lock_seal, "the page keeps working from what the chat left");
    let _ = cx2;
}

/// The chat tools of the browser data pages on the same data folder (the real  entries).
fn chat_rig(cx0: &AppCx) -> (AppCx, iyakki_shared::settingstools::SettingsTools) {
    use iyakki_shared::agent::{ToolDef};
    use iyakki_shared::agent::{Tools};
    use iyakki_shared::settingstools::{SettingsTools};
    use iyakki_agent::settingstools::{Env};
    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
            name.to_string()
        }
    }
    let cx = cx0.clone();
    let dir = cx.data_dir().to_path_buf();
    let settings = Arc::new(std::sync::Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    let env = Env { dir: dir.clone(), user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(|_, _| {}) };
    let mut entries = crate::apps::chat_browser_data::entries(&crate::apps::chat_port::CxChat(&cx), &env);
    entries.extend(crate::apps::chat_camvision::entries(&crate::apps::chat_port::CxChat(&cx), &env));
    let t = SettingsTools::new(Arc::new(NoTools), true, entries);
    (cx, t)
}

#[test]
fn the_browser_page_offers_every_new_key_at_the_five_sizes_and_each_button_sends_its_key() {
    let cx = admin("bchoice2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    for (w, h) in SIZES {
        let found = keys_along_the_page(&mut a, &cx, w, h);
        for k in ['Z', 'L', 'Y', 'B', 'D', 'P'] {
            assert!(found.contains(&k), "{w}x{h}: {k} is reachable by touch; found {found:?}");
        }
    }
    // a click is the key its hit carries
    a.bh.scroll = 0.0;
    for _ in 0..40 {
        let g = draw(&mut a, &cx, 1280, 720);
        if let Some(h) = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char('L')) && h.rect.y > 72.0 && h.rect.y + h.rect.h < 720.0) {
            let ev = h.ev.clone();
            a.handle(&ev, &cx);
            assert!(Policy::load(cx.data_dir(), "root").lock_seal, "the click on the switch changed the choice");
            return;
        }
        a.bh.scroll = (a.bh.scroll + 150.0).min(a.bh.max);
    }
    panic!("the switch was never on screen");
}

#[test]
fn the_documents_page_has_the_order_and_graph_controls_for_an_administrator_only_and_the_chat_agrees() {
    use iyakki_agent::ragrerank::Mode;
    let cx = admin("ragorder1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Rag);
    let dir = cx.data_dir().to_path_buf();
    assert_eq!(Mode::load(&dir), Mode::Off);
    assert!(!iyakki_agent::ragraph::is_on(&dir));
    press(&mut a, &cx, 'o');
    assert_eq!(Mode::load(&dir), Mode::Lexical);
    press(&mut a, &cx, 'o');
    assert_eq!(Mode::load(&dir), Mode::Off, "o goes round: off, lexical, off");
    press(&mut a, &cx, 'g');
    assert!(iyakki_agent::ragraph::is_on(&dir));
    press(&mut a, &cx, 'O');
    let Some(Modal::Form(FormKind::RagRerankModel, _)) = &a.modal else { panic!("the model form") };
    let f = Form::new("x").text("Model name", "bge-reranker-v2-m3-GGUF", "");
    assert!(a.rag_rerank_submit(&cx, &f).unwrap().contains("model bge-reranker"));
    assert_eq!(Mode::load(&dir).text(), "model bge-reranker-v2-m3-GGUF");
    assert!(a.rag_rerank_submit(&cx, &Form::new("x").text("Model name", "bad name!", "")).is_err());
    assert!(a.rag_rerank_submit(&cx, &Form::new("x").text("Model name", "", "")).unwrap().contains("off"));
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("settings.rag") && l.contains("graph on")));
    // the layout: both keys are buttons on every size, inside the window
    a.handle(&Event::Key(Key::Esc), &cx);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        for c in ['o', 'O', 'g'] {
            let hit = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char(c)));
            let Some(hit) = hit else { panic!("{w}x{h}: {c} has no button; buttons: {:?}", g.hits.iter().map(|x| (x.label.clone(), x.rect.x as i32, x.rect.y as i32, x.rect.w as i32)).collect::<Vec<_>>()) };
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y >= 0.0 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {c} {r:?}");
        }
    }
    // an ordinary account sees the state and has no buttons; its keys do nothing
    let mut cx2 = cx.clone();
    cx2.session.user = "bea".into();
    cx2.session.role = iyakki_sec::policy::Role::User;
    cx2.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = SettingsApp::new(&cx2);
    b.go_to(Tab::Rag);
    let g = draw(&mut b, &cx2, 1280, 720);
    assert!(!g.hits.iter().any(|x| matches!(x.ev, Event::Key(Key::Char('o' | 'O')))), "no order buttons for an ordinary account");
    press(&mut b, &cx2, 'o');
    press(&mut b, &cx2, 'g');
    assert_eq!(Mode::load(&dir), Mode::Off);
}

#[test]
fn camera_alert_descriptions_are_a_switch_on_the_notifications_page_with_the_same_file_the_chat_uses() {
    let cx = admin("camvis1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Notifications);
    let dir = cx.data_dir().to_path_buf();
    assert!(!iyakki_shared::camvision::Setting::load(&dir, "root").on, "off until chosen");
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        let hit = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('v')));
        let r = hit.unwrap_or_else(|| panic!("{w}x{h}: no switch")).rect;
        assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
    }
    press(&mut a, &cx, 'v');
    assert!(iyakki_shared::camvision::Setting::load(&dir, "root").on);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.cameras.vision") && l.contains("on")));
    let (_cx2, t) = chat_rig(&cx);
    let said = t.call("camera_vision_status", &serde_json::json!({})).unwrap();
    assert!(said.contains("are on"), "the chat sees what the page set: {said} {:?}", std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>());
    t.call("camera_vision_set", &serde_json::json!({"on": false})).unwrap();
    press(&mut a, &cx, 'v');
    assert!(iyakki_shared::camvision::Setting::load(&dir, "root").on, "and the page works from what the chat left");
}

#[test]
fn a_theme_file_is_a_button_on_the_theme_card_applies_at_once_and_is_kept_and_a_bad_one_is_not_offered() {
    use crate::theme::{current_id, is_custom, load_choice, ThemeId};
    let cx = admin("theme-user1");
    let dir = cx.data_dir().to_path_buf();
    std::fs::create_dir_all(dir.join("themes")).unwrap();
    std::fs::write(dir.join("themes/midnight-blue.theme"), "name = Midnight blue\nbase = dark\nbg_top = #0b1020\n").unwrap();
    std::fs::write(dir.join("themes/dull.theme"), "name = Dull\ntext = #111111\n").unwrap();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Display);
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        let labels: Vec<String> = g.hits.iter().map(|x| x.label.clone()).collect();
        assert!(labels.iter().any(|l| l.contains("Midnight blue")), "{w}x{h}: the file is a button: {labels:?}");
        assert!(!labels.iter().any(|l| l.contains("Dull")), "a file that cannot be used is not offered");
        for hit in g.hits.iter().filter(|x| x.label.contains("Midnight") || x.label.contains("High contrast")) {
            assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{w}x{h}: {:?}", hit.rect);
        }
    }
    let g = draw(&mut a, &cx, 1280, 720);
    let click = g.hits.iter().find(|x| x.label.contains("Midnight blue")).map(|x| x.ev.clone()).unwrap();
    a.handle(&click, &cx);
    assert!(is_custom(), "the theme is on screen at once");
    assert_eq!(crate::theme::themed(ThemeId::Dark.theme().bg_top), [0x0b, 0x10, 0x20, 255]);
    assert_eq!(load_choice(&dir, "root").as_deref(), Some("user:midnight-blue"));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.display.theme") && l.contains("midnight-blue")));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|x| x.label == "\u{2713} Midnight blue"), "the chosen one carries the tick");
    // t goes on round to the first built-in one again, a built-in click takes the file away
    press(&mut a, &cx, 't');
    assert_eq!(current_id(), ThemeId::Dark);
    assert!(!is_custom(), "t after the last theme goes back to Dark");
    press(&mut a, &cx, '4');
    assert!(is_custom());
    press(&mut a, &cx, '2');
    assert!(!is_custom() && current_id() == ThemeId::Light);
    assert_eq!(load_choice(&dir, "root").as_deref(), Some("light"));
    press(&mut a, &cx, '9');
    assert!(status(&a).contains("no theme"), "a number with no theme says so: {}", status(&a));
    // the chat tool reads what the page chose
    crate::theme::apply(None);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn echo_cancellation_is_a_row_on_the_voice_page_by_key_and_touch_for_an_administrator_and_the_chat_agrees() {
    let cx = admin("echo1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Voice);
    assert!(!cx.voice_settings().echo_cancel, "off until chosen");
    for (w, h) in SIZES {
        // the Voice page scrolls: the row is reachable somewhere along it
        a.vo_scroll = 0.0;
        let mut seen = None;
        for _ in 0..40 {
            let g = draw(&mut a, &cx, w, h);
            if let Some(x) = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('E')) && x.rect.y >= 72.0 && x.rect.y + x.rect.h <= h as f32 + 0.5) {
                seen = Some(x.rect);
                break;
            }
            if a.vo_scroll >= a.vo_max - 0.5 {
                break;
            }
            a.vo_scroll = (a.vo_scroll + 120.0).min(a.vo_max);
        }
        let r = seen.unwrap_or_else(|| panic!("{w}x{h}: no row"));
        assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5, "{w}x{h}: {r:?}");
    }
    a.vo_scroll = 0.0;
    let mut ev = None;
    for _ in 0..40 {
        let g = draw(&mut a, &cx, 1280, 720);
        if let Some(x) = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('E'))) {
            ev = Some(x.ev.clone());
            break;
        }
        a.vo_scroll = (a.vo_scroll + 120.0).min(a.vo_max);
    }
    let ev = ev.expect("a hit for the row");
    a.handle(&ev, &cx);
    assert!(cx.voice_settings().echo_cancel);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("echo-cancel-on")));
    press(&mut a, &cx, 'E');
    assert!(!cx.voice_settings().echo_cancel);
}

#[test]
fn the_chat_installs_lists_and_removes_a_site_app_and_the_list_says_what_the_site_lacks() {
    let cx = admin("siteapp-chat1");
    let (cx, t) = chat_rig(&cx);
    assert!(t.call("site_apps_list", &serde_json::json!({})).unwrap().contains("No site"));
    assert!(t.call("browser_install_site", &serde_json::json!({"url": "http://plain.example/"})).is_err(), "only https");
    let said = t.call("browser_install_site", &serde_json::json!({"url": "https://www.netflix.com/browse", "title": "Netflix", "name": "Films"})).unwrap();
    assert!(said.contains("Films") && said.contains("content decryption module"), "{said}");
    let listed = t.call("site_apps_list", &serde_json::json!({})).unwrap();
    assert!(listed.contains("https://www.netflix.com") && listed.contains("NOTE"), "{listed}");
    let id = crate::apps::site_apps::load(&cx).apps[0].id.clone();
    let prof = crate::apps::site_apps::profile_dir(cx.data_dir(), "root", &id);
    std::fs::create_dir_all(&prof).unwrap();
    std::fs::write(prof.join("cookie"), b"x").unwrap();
    let said = t.call("site_app_remove", &serde_json::json!({"name": "films"})).unwrap();
    assert!(said.contains("sign-ins are deleted") && !prof.exists(), "{said}");
    assert!(t.call("site_app_remove", &serde_json::json!({"name": "films"})).is_err());
}

#[test]
fn the_chat_opens_a_site_app_by_name_and_the_shell_gets_one_request_for_this_person_only() {
    let cx = admin("siteapp-open1");
    let (cx, t) = chat_rig(&cx);
    let defs = t.defs();
    assert_eq!(defs.iter().find(|d| d.spec.name == "site_app_open").expect("the tool exists").risk, iyakki_agent::agent::Risk::Ask, "opening a site asks first");
    assert!(t.call("site_app_open", &serde_json::json!({"name": "films"})).unwrap_err().contains("site_apps_list"), "no such site app");
    assert!(crate::apps::download_open::take_chat(&cx.session.user).is_none(), "a refused call queues nothing");
    t.call("browser_install_site", &serde_json::json!({"url": "https://www.example.org/start", "title": "Example", "name": "Films"})).unwrap();
    let id = crate::apps::site_apps::load(&cx).apps[0].id.clone();
    let said = t.call("site_app_open", &serde_json::json!({"name": "films"})).unwrap();
    assert!(said.contains("Films"), "{said}");
    assert!(crate::apps::download_open::take_chat("someone-else").is_none());
    let (tile, item) = crate::apps::download_open::take_chat(&cx.session.user).expect("queued");
    assert_eq!(tile, "browser");
    assert_eq!(crate::apps::download_open::item_for(&tile, &item), format!("site:{id}"), "the browser's own open-item hand-over");
    assert!(crate::apps::download_open::take_chat(&cx.session.user).is_none(), "taken once");
    assert!(t.describe("site_app_open", &serde_json::json!({"name": "Films"})).contains("Films"));
}
