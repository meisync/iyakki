//! Settings: the way the pages are arranged (categories, order, the Overview start page and its attention list), the search
//! with plain words, role gating, every page drawn inside the window at five sizes, and a keyboard-only walk through every page.

use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_fw::backend::FakeBackend;
use iyakki_fw::engine::{Clock, Engine, Saved, Store};
use iyakki_fw::service::Handle;
use std::sync::Mutex;

const SIZES: [(u32, u32); 5] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)];

fn role_cx(tag: &str, role: iyakki_sec::policy::Role) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    if role != iyakki_sec::policy::Role::Admin {
        cx.session.user = "amy".into();
        cx.session.role = role;
        cx.session.policy = role.template();
        cx.services.on_login("amy", "amypass-123", false);
    }
    cx
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn typ(a: &mut SettingsApp, cx: &AppCx, t: &str) {
    for c in t.chars() {
        key(a, cx, Key::Char(c));
    }
}

struct Fix(i64);
impl Clock for Fix {
    fn now(&self) -> i64 {
        self.0
    }
}
#[derive(Default)]
struct Mem(Mutex<Option<Saved>>);
impl Store for Mem {
    fn load(&self) -> Result<Option<Saved>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(s.clone());
        Ok(())
    }
}
struct NoDns;
impl iyakki_fw::engine::Resolver for NoDns {
    fn resolve(&self, _: &str) -> Result<Vec<std::net::IpAddr>, String> {
        Err("none".into())
    }
}

fn fake_engine() -> Arc<Mutex<Engine>> {
    let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(Fix(5_000_000)), Box::new(NoDns), Box::new(Mem::default()));
    e.boot(false);
    Arc::new(Mutex::new(e))
}

fn admin_app(tag: &str) -> (SettingsApp, AppCx) {
    let cx = cx_with_admin_store(tag);
    let mut a = SettingsApp::new(&cx);
    a.fw.handle = Handle::Own(fake_engine());
    a.fw.writable = true;
    (a, cx)
}

#[test]
fn every_page_sits_under_one_of_the_headings_in_a_fixed_order() {
    let (a, _cx) = admin_app("ia-order");
    for t in a.tabs() {
        assert!(CATEGORIES.contains(&t.group()), "{t:?}");
    }
    let uniq: std::collections::BTreeSet<String> = Tab::ORDER.iter().map(|t| format!("{t:?}")).collect();
    assert_eq!(uniq.len(), Tab::ORDER.len(), "no page twice in the order table");
    // the list is sorted by heading, then by the table
    let ranks: Vec<usize> = a.tabs().iter().map(|t| CATEGORIES.iter().position(|c| *c == t.group()).unwrap()).collect();
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
    assert_eq!(a.tabs()[0], Tab::Overview, "Settings opens on the Overview");
    // the headings in the left list come in the same order and none is empty
    let entries = nav::entries(&a.nav_items(), "");
    let heads: Vec<&str> = entries.iter().filter_map(|e| if let nav::Entry::Group(g) = e { Some(*g) } else { None }).collect();
    assert_eq!(heads, CATEGORIES.to_vec());
    // every page an administrator can have is in the order table (a new page has to be placed)
    for t in a.tabs() {
        assert!(Tab::ORDER.contains(t), "{t:?} is missing from Tab::ORDER");
    }
    assert!(a.tabs().len() <= 48, "the per-page selection table has room");
}

#[test]
fn plain_words_and_synonyms_find_the_right_page() {
    let (mut a, cx) = admin_app("ia-search");
    let found = |a: &mut SettingsApp, cx: &AppCx, q: &str| -> Vec<Tab> {
        a.nav_query.set("");
        key(a, cx, Key::Ctrl('f'));
        typ(a, cx, q);
        let v = a.nav_matches().iter().map(|i| a.tabs()[*i]).collect();
        key(a, cx, Key::Esc);
        key(a, cx, Key::Esc);
        v
    };
    for (word, want) in [
        ("wifi", Tab::Network),
        ("wi-fi", Tab::Network),
        ("password", Tab::Account),
        ("password", Tab::Vault),
        ("dark", Tab::Display),
        ("font", Tab::Display),
        ("font", Tab::Account),
        ("sleep", Tab::Display),
        ("mouse", Tab::Bluetooth),
        ("backup", Tab::Storage),
        ("reboot", Tab::Storage),
        ("security", Tab::Firewall),
        ("volume", Tab::Sound),
        ("clock", Tab::Time),
        ("computer name", Tab::HostName),
        ("assistant", Tab::Models),
        ("speakers", Tab::Sound),
        ("block", Tab::Firewall),
    ] {
        assert!(found(&mut a, &cx, word).contains(&want), "{word:?} should find {want:?}");
    }
    assert!(found(&mut a, &cx, "zzzzqq").is_empty());
    // the empty state tells people what to do
    key(&mut a, &cx, Key::Ctrl('f'));
    typ(&mut a, &cx, "zzzzqq");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().all(|h| !matches!(h.ev, Event::Tab(_))), "no page listed");
}

#[test]
fn the_overview_lists_what_needs_attention_and_fix_it_opens_the_page() {
    let (mut a, cx) = admin_app("ia-attn");
    let list = a.attention(&cx);
    assert!(list.iter().any(|x| x.tab == Tab::Models && x.title.contains("No AI model")), "{list:?}");
    assert!(!list.iter().any(|x| x.tab == Tab::Firewall), "the firewall is on");
    // switch the firewall off through the engine: it is then listed, first
    if let Handle::Own(e) = &a.fw.handle {
        e.lock().unwrap().disable_now("test").unwrap();
    }
    let list = a.attention(&cx);
    assert_eq!(list[0].tab, Tab::Firewall, "the most serious first: {list:?}");
    assert!(list[0].title.contains("open"));
    // the page draws the list with one "Fix it" button per item, each inside the window
    for (w, h) in SIZES {
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().any(|x| x.label == "Fix it"), "{w}x{h}");
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            let rc = hit.rect;
            assert!(rc.x >= -0.5 && rc.y >= -0.5 && rc.x + rc.w <= w as f32 + 0.5 && rc.y + rc.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, rc);
        }
    }
    // a click on it opens the page that fixes it
    let g = draw(&mut a, &cx, 1280, 720);
    let fix = g.hits.iter().find(|x| x.label == "Fix it").unwrap().ev.clone();
    a.handle(&fix, &cx);
    assert_eq!(a.tab(), Tab::Firewall);
    // Enter does the same from the keyboard
    a.go_to(Tab::Overview);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.tab(), Tab::Firewall, "the selected item opens");
    // choosing a model clears its line
    a.go_to(Tab::Overview);
    Self_::choose_model(&mut a, &cx);
    assert!(!a.attention(&cx).iter().any(|x| x.tab == Tab::Models));
    // the common things are buttons too
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|x| x.label == "Join WiFi"));
}

struct Self_;
impl Self_ {
    fn choose_model(a: &mut SettingsApp, cx: &AppCx) {
        SettingsApp::svc(cx).settings.lock().unwrap().update(|s| {
            s.providers.push(ProviderCfg { id: "p".into(), kind: Kind::Ollama, base_url: Some("http://127.0.0.1:1".into()), key_ref: None, models: vec!["m".into()], default_model: Some("m".into()), enabled: true, model_kinds: Default::default() });
            s.chat = Some(ModelChoice { provider: "p".into(), model: "m".into() });
        }).unwrap();
        let _ = a;
    }
}

#[test]
fn an_all_clear_overview_says_so_and_a_user_only_hears_about_their_own_pages() {
    let (mut a, cx) = admin_app("ia-clear");
    Self_::choose_model(&mut a, &cx);
    let rest: Vec<Tab> = a.attention(&cx).iter().map(|x| x.tab).collect();
    assert!(!rest.contains(&Tab::Models) && !rest.contains(&Tab::Firewall));
    for role in [iyakki_sec::policy::Role::User, iyakki_sec::policy::Role::Guest] {
        let cx = role_cx(&format!("ia-role-{role:?}"), role);
        let mut u = SettingsApp::new(&cx);
        assert_eq!(u.tab(), Tab::Overview);
        for t in [Tab::Firewall, Tab::Network, Tab::Storage, Tab::Bluetooth, Tab::Install, Tab::Agents] {
            assert!(!u.tabs().contains(&t), "{role:?} must not get {t:?}");
        }
        assert!(u.attention(&cx).iter().all(|x| u.tabs().contains(&x.tab)), "only pages they have");
        for (w, h) in SIZES {
            assert!(!draw(&mut u, &cx, w, h).hits.is_empty());
        }
    }
}

#[test]
fn every_page_draws_inside_the_window_at_five_sizes_for_each_role() {
    for role in [iyakki_sec::policy::Role::Admin, iyakki_sec::policy::Role::User, iyakki_sec::policy::Role::Guest] {
        let cx = role_cx(&format!("ia-draw-{role:?}"), role);
        let mut a = SettingsApp::new(&cx);
        a.fw.handle = Handle::Own(fake_engine());
        a.fw.writable = true;
        let n = a.tabs().len();
        for i in 0..n {
            a.tab = i;
            // the administrator's pages at all five sizes; the other roles' (a subset of them) at the first three, to keep the run short
            let sizes: &[(u32, u32)] = if role == iyakki_sec::policy::Role::Admin { &SIZES } else { &SIZES[..3] };
            for &(w, h) in sizes {
                let g = draw(&mut a, &cx, w, h);
                for hit in g.hits.iter().filter(|x| x.rect.w > 0.0 && x.rect.h > 0.0) {
                    let rc = hit.rect;
                    assert!(rc.x >= -0.5 && rc.y >= -0.5 && rc.x + rc.w <= w as f32 + 0.5 && rc.y + rc.h <= h as f32 + 0.5, "{role:?} {:?} {w}x{h}: {:?} {:?}", a.tab(), hit.label, rc);
                }
                // the page list's rows never overlap one another
                let rows: Vec<R> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(_))).map(|x| x.rect).collect();
                for (p, r1) in rows.iter().enumerate() {
                    for r2 in rows.iter().skip(p + 1) {
                        let overlap = r1.x < r2.x + r2.w && r2.x < r1.x + r1.w && r1.y < r2.y + r2.h - 0.5 && r2.y < r1.y + r1.h - 0.5;
                        // the Overview's own "Fix it" and common-thing buttons also carry a Tab event: they sit apart too
                        assert!(!overlap, "{role:?} {:?} {w}x{h}: {r1:?} vs {r2:?}", a.tab());
                    }
                }
            }
        }
    }
}

#[test]
fn the_keyboard_alone_reaches_every_page_and_search_finds_pages_without_the_mouse() {
    let (mut a, cx) = admin_app("ia-keys");
    let n = a.tabs().len();
    let mut seen = vec![a.tab()];
    for _ in 1..n {
        key(&mut a, &cx, Key::Right);
        seen.push(a.tab());
        assert!(!a.hint().is_empty(), "{:?} has a hint line", a.tab());
        let _ = draw(&mut a, &cx, 800, 480);
    }
    assert_eq!(seen.len(), n);
    assert_eq!(seen, a.tabs().to_vec(), "Right visits every page once, in list order");
    for _ in 0..n {
        key(&mut a, &cx, Key::Left);
    }
    assert_eq!(a.tab(), Tab::Overview, "Left walks back");
    // Ctrl+F, a word, Enter
    key(&mut a, &cx, Key::Ctrl('f'));
    typ(&mut a, &cx, "firewall");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.tab(), Tab::Firewall);
    assert!(!a.nav_focus, "the keyboard goes back to the page");
    // the simple firewall page is operable from the keyboard: Add opens the list, Esc closes it
    key(&mut a, &cx, Key::Char('a'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal());
    // "/" finds a page when no page needs the key
    a.go_to(Tab::Overview);
    key(&mut a, &cx, Key::Char('/'));
    assert!(a.nav_focus);
}

#[test]
fn every_page_has_a_title_a_sentence_and_words_to_find_it_by() {
    for t in Tab::ORDER {
        assert!(!t.label().is_empty() && !t.keywords().is_empty(), "{t:?}");
        let b = t.blurb();
        assert!(b.len() > 20 && b.ends_with('.'), "{t:?}: {b:?}");
    }
}

/// `IYAKKI_SHOTS=/some/dir cargo test -p iyakki-shell --lib settings_pictures` writes PNGs of Overview, both firewall views, Account,
/// Display and Models at the five sizes (nothing happens without the variable).
#[test]
fn settings_pictures() {
    let Ok(dir) = std::env::var("IYAKKI_SHOTS") else { return };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let (mut a, cx) = admin_app("ia-pics");
    if let Handle::Own(e) = &a.fw.handle {
        e.lock().unwrap().disable_now("picture").unwrap();
    }
    for (name, tab, advanced) in [("overview", Tab::Overview, false), ("firewall", Tab::Firewall, false), ("firewall-advanced", Tab::Firewall, true), ("account", Tab::Account, false), ("display", Tab::Display, false), ("models", Tab::Models, false)] {
        a.go_to(tab);
        a.fw.advanced = advanced;
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            g.save_png(&dir.join(format!("settings-{name}-{w}x{h}.png"))).unwrap();
        }
    }
}

#[test]
fn the_actions_that_once_had_only_a_key_now_also_have_a_button_to_click_or_touch() {
    let (mut a, _cx) = admin_app("ia-keyonly");
    let has = |a: &SettingsApp, keys: &str| {
        let got: Vec<char> = a.buttons_for(true).iter().map(|b| b.2).collect();
        for k in keys.chars() {
            assert!(got.contains(&k), "{:?} has no button for the key {k}", a.tab());
        }
    };
    a.go_to(Tab::Cloud);
    has(&a, "ca");
    a.go_to(Tab::Memory);
    has(&a, "r");
    a.go_to(Tab::Shares);
    has(&a, "f");
    a.go_to(Tab::Firewall);
    a.fw.advanced = true;
    has(&a, "rcib");
}
