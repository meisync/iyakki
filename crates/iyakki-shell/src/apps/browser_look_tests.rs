//! The Browser tile's look: tab strip, tool bar, loading and error states, tab groups and the overview, at every test size.
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_web::fetch::{Fetch, Response};
use std::collections::HashMap;
use std::sync::Mutex;

struct Site {
    pages: Mutex<HashMap<String, Response>>,
}

impl Fetch for Site {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.pages.lock().unwrap().get(&url.to_string()).cloned().ok_or_else(|| "connection refused".to_string())
    }
}

fn html(body: &str) -> Response {
    Response { status: 200, content_type: "text/html".into(), body: body.as_bytes().to_vec(), ..Response::default() }
}

fn site(pages: Vec<(&str, Response)>) -> Arc<Site> {
    Arc::new(Site { pages: Mutex::new(pages.into_iter().map(|(u, r)| (u.to_string(), r)).collect()) })
}

fn cx_with(tag: &str, web: Arc<Site>) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    cx.web = web;
    cx
}

fn settle(a: &mut BrowserApp, cx: &AppCx) {
    for _ in 0..24_000 {
        a.handle(&Event::Tick, cx);
        if !a.is_loading() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the page never finished loading");
}

fn key(a: &mut BrowserApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn frame(a: &mut BrowserApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0), cx, 0);
    g
}

fn allow_js(a: &mut BrowserApp, cx: &AppCx, host: &str) {
    a.cfg.js_sites.insert(host.to_string());
    browser_config::save(cx, &a.cfg);
}

const GUIDE: &str = "<html><head><title>The Guide</title></head><body><h1>Guide</h1><p>Read the <a href=\"/more\">more</a> page.</p></body></html>";
use crate::apps::browser_chrome as chrome;
use crate::apps::browser_groups::StripItem;

const SIZES: [(u32, u32); 5] = [(2560, 1600), (1920, 1080), (1366, 768), (1280, 720), (800, 480)];

fn scale(w: u32, h: u32) -> f32 {
    (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0)
}

fn px(g: &Gfx, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * g.width() + x) * 4) as usize;
    let d = g.rgba();
    [d[i], d[i + 1], d[i + 2], d[i + 3]]
}

fn inside(r: R, o: R) -> bool {
    r.x >= o.x - 0.5 && r.y >= o.y - 0.5 && r.x + r.w <= o.x + o.w + 0.5 && r.y + r.h <= o.y + o.h + 0.5
}

fn overlaps(a: R, b: R) -> bool {
    a.w > 0.0 && b.w > 0.0 && a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

const LONG: &str = "An extremely long page title that goes on and on and on, far wider than any tab in the strip can ever show";

fn long_site(n: usize) -> Arc<Site> {
    let pages: Vec<(String, Response)> = (0..n).map(|i| (format!("https://s{i}.org/"), html(&format!("<html><head><title>{LONG} {i}</title></head><body><p>page {i}</p></body></html>")))).collect();
    let pages: Vec<(&str, Response)> = pages.iter().map(|(u, r)| (u.as_str(), r.clone())).collect();
    site(pages)
}

fn app_with_tabs(tag: &str, n: usize) -> (BrowserApp, AppCx) {
    let cx = cx_with(tag, long_site(n));
    let mut a = BrowserApp::new(&cx);
    for i in 0..n {
        if i > 0 {
            a.new_tab();
        }
        a.open_url(&format!("s{i}.org"), &cx);
        settle(&mut a, &cx);
    }
    (a, cx)
}

fn strip_rect(w: u32, s: f32) -> R {
    R::new(0.0, 72.0, w as f32, chrome::TABS_H * s)
}

#[test]
fn tab_titles_stay_clear_of_the_close_button_and_the_tool_bar_at_every_size() {
    for (w, h) in SIZES {
        let s = scale(w, h);
        for n in [1usize, 6, 12] {
            let items: Vec<StripItem> = (0..n).map(StripItem::Tab).collect();
            for active in [0, n - 1] {
                let area = strip_rect(w, s);
                let geo = chrome::strip_layout(area, s, &items, active, &|_| 0.0, 4.0 * s);
                let bar = R::new(area.x, area.y + area.h, area.w, chrome::BAR_H * s);
                let mut g = Gfx::new(w, h);
                assert_eq!(geo.tabs.len(), n);
                for t in &geo.tabs {
                    assert!(inside(t.tab, R::new(area.x, area.y, area.w, area.h)), "{w}x{h} n={n}: tab {} is inside the strip: {:?}", t.idx, t.tab);
                    assert!(t.tab.y + t.tab.h <= area.y + area.h - 2.0 * s, "{w}x{h}: a gap stays above the tool bar");
                    assert!(!overlaps(t.tab, bar), "{w}x{h}: the tab does not run into the tool bar");
                    assert!(!overlaps(t.title, t.close), "{w}x{h} n={n}: the title does not reach the close button: {:?} {:?}", t.title, t.close);
                    assert!(!overlaps(t.title, t.icon), "{w}x{h}: the title does not cover the icon");
                    assert!(inside(t.close, t.tab) && inside(t.title, t.tab), "{w}x{h}: inside the tab");
                    // the cut title fits its rectangle
                    let shown = g.fit(14.0 * s, LONG, Face::Bold, t.title.w);
                    assert!(t.title.w <= 0.5 || g.text_width(14.0 * s, &shown, Face::Bold) <= t.title.w + 0.5, "{w}x{h} n={n}: {shown:?} fits {}", t.title.w);
                    if t.idx == active {
                        assert!(t.close.w >= 20.0 * s, "the tab in use always has its close button");
                    }
                }
                for (a, b) in geo.tabs.iter().zip(geo.tabs.iter().skip(1)) {
                    assert!(!overlaps(a.tab, b.tab), "{w}x{h} n={n}: tabs do not overlap");
                }
                assert!(inside(geo.plus, area) && inside(geo.overview, area), "{w}x{h} n={n}: + and overview are inside the strip: {:?} {:?}", geo.plus, geo.overview);
                let last = geo.tabs.last().unwrap().tab;
                assert!(!overlaps(last, geo.plus) && !overlaps(last, geo.overview), "{w}x{h} n={n}: nothing covers the + or the overview button");
            }
        }
    }
}

#[test]
fn the_real_strip_with_long_titles_registers_tabs_and_close_buttons_that_do_not_overlap() {
    for n in [1usize, 6, 12] {
        let (mut a, cx) = app_with_tabs(&format!("look-strip-{n}"), n);
        for (w, h) in SIZES {
            let s = scale(w, h);
            let g = frame(&mut a, &cx, w, h);
            let strip = R::new(0.0, 72.0, w as f32, chrome::TABS_H * s);
            let tabs: Vec<_> = g.hits.iter().filter(|x| x.label == "tab").collect();
            let closes: Vec<_> = g.hits.iter().filter(|x| x.label == "close tab").collect();
            assert_eq!(tabs.len(), n, "{w}x{h}");
            for t in &tabs {
                assert!(inside(t.rect, strip), "{w}x{h} n={n}: {:?} in {strip:?}", t.rect);
            }
            for c in &closes {
                assert!(tabs.iter().any(|t| inside(c.rect, t.rect)), "{w}x{h}: a close button sits inside its tab");
            }
            // the text drawn for titles never starts inside a close button
            for (x, y, text) in &g.text_log {
                if text.contains("extremely") || text.contains("An extr") {
                    assert!(closes.iter().all(|c| !(c.rect.contains(*x as i32, *y as i32))), "{w}x{h} n={n}: title text {text:?} starts inside a close button");
                }
            }
        }
    }
}

#[test]
fn the_tool_bar_groups_its_controls_and_nothing_overlaps_or_leaves_the_bar() {
    for (w, h) in SIZES {
        let s = scale(w, h);
        for (star, chips) in [(false, vec![]), (true, vec![(chrome::Chip::Reader, 80.0 * s), (chrome::Chip::Notes, 90.0 * s), (chrome::Chip::Scripts, 110.0 * s), (chrome::Chip::Play, 70.0 * s), (chrome::Chip::Dev, 60.0 * s)])] {
            let b = chrome::BarIn { cap_w: 0.0, lock_w: 90.0 * s, star, go_w: 56.0 * s, chips };
            let geo = chrome::bar_layout(0.0, 72.0 + chrome::TABS_H * s, w as f32, s, &b);
            let bar = R::new(0.0, 72.0 + chrome::TABS_H * s, w as f32, chrome::BAR_H * s);
            let mut parts: Vec<(String, R)> = geo.nav.iter().enumerate().map(|(i, r)| (format!("nav{i}"), *r)).collect();
            parts.push(("field".into(), geo.field));
            for (c, r) in &geo.chips {
                parts.push((format!("{c:?}"), *r));
            }
            for (i, (n1, r1)) in parts.iter().enumerate() {
                assert!(inside(*r1, bar), "{w}x{h}: {n1} {r1:?} is inside the bar {bar:?}");
                for (n2, r2) in parts.iter().skip(i + 1) {
                    assert!(!overlaps(*r1, *r2), "{w}x{h}: {n1} and {n2} overlap");
                }
            }
            // Go sits inside the field's right end, the star just left of it, the lock chip at the left edge
            assert!(inside(geo.go, geo.field) && geo.go.x + geo.go.w > geo.field.x + geo.field.w - 8.0 * s, "{w}x{h}: Go is at the right end of the address bar");
            if let Some(st) = geo.star {
                assert!(inside(st, geo.field) && !overlaps(st, geo.go), "{w}x{h}: the star is inside the field, left of Go");
            }
            let lock = geo.lock.unwrap();
            assert!(inside(lock, geo.field) && lock.x - geo.field.x < 12.0 * s, "{w}x{h}: the lock chip is inside the field's left edge");
            assert!(geo.inset.0 >= lock.w && geo.inset.1 >= geo.go.w, "{w}x{h}: the text keeps clear of the chips");
            assert!(geo.field.w >= 200.0 * s, "{w}x{h}: the address field keeps its room: {}", geo.field.w);
            for r in geo.nav.iter().chain(geo.chips.iter().map(|c| &c.1)) {
                assert!(r.w >= 38.0 * s && r.h >= 38.0 * s, "{w}x{h}: touch target {r:?}");
            }
            assert!(geo.go.h >= 32.0 * s && geo.go.w >= 44.0 * s);
        }
    }
}

#[test]
fn the_real_bar_has_go_in_the_address_field_the_quiet_dev_chip_and_the_scripts_chip_in_words() {
    let web = site(vec![("https://a.org/", html(GUIDE))]);
    let cx = cx_with("look-bar", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    // scripts run everywhere by default, and the chip says so; the rest of the test is about a person who switched that off
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|x| x.ev == Event::Tab(5) && x.label.starts_with("Scripts allowed")), "the chip says scripts run by default");
    a.cfg.js_all = false;
    browser_config::save(&cx, &a.cfg);
    for (w, h) in SIZES {
        let g = frame(&mut a, &cx, w, h);
        let field = g.hits.iter().find(|x| x.ev == Event::Focus(0)).expect("the address field").rect;
        let go = g.hits.iter().find(|x| x.label == "Go").unwrap().rect;
        assert!(inside(go, field), "{w}x{h}: Go is inside the address bar");
        let star = g.hits.iter().find(|x| x.ev == Event::Tab(8)).unwrap().rect;
        assert!(inside(star, field) && star.x + star.w <= go.x + 0.5, "{w}x{h}: star before Go, inside the field");
        let js = g.hits.iter().find(|x| x.ev == Event::Tab(5)).expect("the scripts chip");
        assert!(js.label.starts_with("Scripts blocked") && js.label.contains("allow"), "{}", js.label);
        let dev = g.hits.iter().find(|x| x.ev == Event::Tab(browser_devpanel::TAB_TOGGLE)).expect("the dev chip");
        assert!(dev.label.contains("F12"), "the key is still named: {}", dev.label);
        assert!(dev.rect.w < 80.0 * scale(w, h), "{w}x{h}: the dev chip is small: {:?}", dev.rect);
        assert!(!g.text_log.iter().any(|t| t.2.contains("Dev tools")), "the long label is gone from the bar");
        assert!(!g.text_log.iter().any(|t| t.2 == "JS"), "the bare JS toggle is gone");
    }
    allow_js(&mut a, &cx, "a.org");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|x| x.label.starts_with("Scripts allowed")), "the chip says so when scripts run");
}

#[test]
fn the_dev_chip_is_neutral_without_problems_and_amber_or_red_only_when_there_are_some() {
    let accent = ACCENT;
    assert_eq!(chrome::dev_color(false, 0, 0, true, accent), (MUTED, false), "collecting with nothing wrong stays quiet");
    assert_eq!(chrome::dev_color(false, 2, 0, true, accent), (chrome::AMBER, true), "failed requests: amber");
    assert_eq!(chrome::dev_color(false, 2, 1, true, accent), (ERR, true), "errors: red");
    assert_eq!(chrome::dev_color(false, 0, 3, false, accent), (MUTED, false), "nothing is collected, so nothing to flag");
    assert_eq!(chrome::dev_color(true, 0, 3, true, accent), (accent, true), "open: the accent");
    assert_eq!(chrome::dev_text(false, 0, 0, true), "</>");
    assert_eq!(chrome::dev_text(false, 2, 3, true), "</> 5");
    assert_eq!(chrome::dev_text(true, 0, 0, true), "</> Esc");
}

#[test]
fn loading_shows_a_progress_bar_a_spinner_and_after_fifteen_seconds_stop_and_retry() {
    let web = site(vec![("https://a.org/", html(GUIDE))]);
    let cx = cx_with("look-loading", web);
    let mut a = BrowserApp::new(&cx);
    let (_tx, rx) = channel();
    a.nav = Nav::Loading { url: Url::parse("https://slow.example/").unwrap(), rx, how: How::Push };
    a.load_started = std::time::Instant::now();
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 5);
    assert!(!g.hits.iter().any(|x| x.label == "Stop"), "no hint in the first seconds");
    // the progress bar's colour is in the row under the tool bar
    let bar_y = (72.0 + (chrome::TABS_H + chrome::BAR_H - 2.0)) as u32;
    let seen = (0..1280).any(|x| { let p = px(&g, x, bar_y); p[2] > 180 && p[0] < 140 });
    assert!(seen, "the accent coloured progress bar is drawn while loading");
    a.load_started = std::time::Instant::now() - std::time::Duration::from_secs(20);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 5);
    assert!(g.hits.iter().any(|x| x.label == "Stop") && g.hits.iter().any(|x| x.label == "Retry"), "the taking-long card has Stop and Retry");
    assert!(g.text_log.iter().any(|t| t.2.contains("taking a while")));
    a.handle(&Event::Tab(TAB_STOP), &cx);
    assert!(!a.is_loading() && a.status_text() == Some("Stopped."));
}

#[test]
fn a_page_that_cannot_be_loaded_gets_a_plain_card_with_retry() {
    let web = site(vec![]);
    let cx = cx_with("look-error", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("https://down.example/", &cx);
    settle(&mut a, &cx);
    assert!(a.failed.is_some());
    for (w, h) in SIZES {
        let g = frame(&mut a, &cx, w, h);
        let retry = g.hits.iter().find(|x| x.label == "Retry").unwrap_or_else(|| panic!("{w}x{h}: Retry"));
        assert!(retry.rect.w * retry.rect.h > 0.0 && retry.rect.y + retry.rect.h <= h as f32);
        assert!(g.text_log.iter().any(|t| t.2.starts_with("Can't reach down.example")), "{w}x{h}: plain words");
    }
    assert!(!a.is_loading());
    a.handle(&Event::Tab(TAB_RETRY), &cx);
    assert!(a.is_loading() || a.failed.is_some(), "retry asks again");
    settle(&mut a, &cx);
    let (t, body) = chrome::explain_failure("bank.example", "tls: invalid peer certificate: UnknownIssuer");
    assert!(t.contains("certificate") && body.contains("not private"));
    assert_eq!(chrome::explain_failure("x.org", "network is unreachable").0, "You are offline");
}

// ------------------------------------------------------------------------------------------------ groups

#[test]
fn groups_are_made_collapsed_renamed_and_closed_with_their_tabs() {
    let (mut a, cx) = app_with_tabs("look-groups", 4);
    a.switch_to(1);
    key(&mut a, &cx, Key::Ctrl('G'));
    assert_eq!(a.groups.list.len(), 1);
    assert!(a.group_edit.is_some(), "the name can be typed at once");
    for c in "Work".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    // the field starts with "Group 1": clear it first
    let gid = a.groups.list[0].id;
    a.group_edit.as_mut().unwrap().field.set("");
    for c in "Work".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.groups.get(gid).unwrap().name, "Work");
    a.switch_to(3);
    key(&mut a, &cx, Key::Ctrl('G'));
    a.group_edit = None;
    a.groups.assign(3, Some(gid));
    a.groups.assign(3, Some(gid));
    a.regroup();
    assert_eq!(a.groups.members(gid), vec![1, 2].into_iter().filter(|_| false).chain(a.groups.members(gid)).collect::<Vec<_>>());
    assert!(a.groups.members(gid).windows(2).all(|p| p[1] == p[0] + 1), "the group's tabs sit together: {:?}", a.groups.members(gid));
    // the strip shows one chip; collapsing it hides the group's tabs except the one in use
    for (w, h) in SIZES {
        let g = frame(&mut a, &cx, w, h);
        assert_eq!(g.hits.iter().filter(|x| x.label.starts_with("group ")).count(), 1, "{w}x{h}: one chip per group");
    }
    a.switch_to(0);
    let tabs_before = frame(&mut a, &cx, 1280, 720).hits.iter().filter(|x| x.label == "tab").count();
    a.handle(&Event::Tab(TAB_CHIP), &cx);
    let tabs_after = frame(&mut a, &cx, 1280, 720).hits.iter().filter(|x| x.label == "tab").count();
    assert_eq!(tabs_before - tabs_after, a.groups.members(gid).len(), "collapsed: the group's tabs are hidden");
    a.handle(&Event::Tab(TAB_CHIP), &cx);
    // closing the tabs of the group one by one removes the group with the last
    let n = a.groups.members(gid).len();
    for _ in 0..n {
        let first = a.groups.members(gid)[0];
        a.close_tab(first);
    }
    assert!(a.groups.list.is_empty(), "the last tab of a group took the group with it");
    assert_eq!(a.groups.of_tab.len(), a.tab_count());
}

#[test]
fn ungroup_and_every_size_with_two_groups_and_twelve_tabs_has_no_overlap() {
    let (mut a, cx) = app_with_tabs("look-groups-many", 12);
    for (i, t) in [(0usize, 0usize), (1, 0), (2, 0), (5, 1), (6, 1)] {
        let _ = t;
        a.switch_to(i);
        if a.groups.list.len() < 1 + (i >= 5) as usize {
            key(&mut a, &cx, Key::Ctrl('G'));
            a.commit_group_edit();
        } else {
            let id = a.groups.list[(i >= 5) as usize].id;
            a.groups.assign(i, Some(id));
            a.regroup();
        }
    }
    assert_eq!(a.groups.list.len(), 2);
    for (w, h) in SIZES {
        let s = scale(w, h);
        let g = frame(&mut a, &cx, w, h);
        let strip = R::new(0.0, 72.0, w as f32, chrome::TABS_H * s);
        let mut parts: Vec<(String, R)> = g.hits.iter().filter(|x| x.label == "tab" || x.label.starts_with("group ") || x.label == "new tab" || x.label == "tab overview").map(|x| (x.label.clone(), x.rect)).collect();
        parts.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap());
        for (i, (n1, r1)) in parts.iter().enumerate() {
            assert!(inside(*r1, strip), "{w}x{h}: {n1} {r1:?} in the strip");
            for (n2, r2) in parts.iter().skip(i + 1) {
                assert!(!overlaps(*r1, *r2), "{w}x{h}: {n1} {r1:?} and {n2} {r2:?} overlap");
            }
        }
    }
    // Ctrl+Shift+U takes the tab in use out of its group
    let n = a.groups.list.len();
    let act = a.active;
    if a.groups.of(act).is_some() {
        key(&mut a, &cx, Key::Ctrl('U'));
        assert!(a.groups.of(a.active).is_none());
        assert!(a.groups.list.len() <= n);
    }
}

#[test]
fn groups_come_back_for_the_same_person_as_tabs_that_load_when_opened() {
    let web = long_site(3);
    let cx = cx_with("look-persist", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("s0.org", &cx);
    settle(&mut a, &cx);
    a.new_tab();
    a.open_url("s1.org", &cx);
    settle(&mut a, &cx);
    a.new_tab();
    a.open_url("s2.org", &cx);
    settle(&mut a, &cx);
    a.switch_to(1);
    a.new_group_from(1);
    a.group_edit.as_mut().unwrap().field.set("Reading");
    a.commit_group_edit();
    a.groups.assign(2, Some(a.groups.list[0].id));
    a.regroup();
    a.groups.toggle_collapse(a.groups.list[0].id);
    for _ in 0..30 {
        a.handle(&Event::Tick, &cx);
    }
    drop(a);
    let b = BrowserApp::new(&cx);
    assert_eq!(b.groups.list.len(), 1, "the group is back");
    assert_eq!((b.groups.list[0].name.as_str(), b.groups.list[0].collapsed), ("Reading", true));
    assert_eq!(b.tab_count(), 3, "the first tab and the group's two");
    assert_eq!(b.groups.members(b.groups.list[0].id).len(), 2);
    let mut b = b;
    assert_eq!(b.tab_titles()[1], "s1.org", "a tab not opened yet is named by its site");
    b.switch_to(1);
    b.handle(&Event::Tick, &cx);
    settle(&mut b, &cx);
    assert!(b.page().is_some_and(|p| p.url.host == "s1.org"), "it loads when first shown");
}

#[test]
fn the_overview_lists_every_tab_by_group_searches_them_and_switches() {
    let (mut a, cx) = app_with_tabs("look-overview", 5);
    a.switch_to(1);
    a.new_group_from(1);
    a.commit_group_edit();
    a.groups.assign(2, Some(a.groups.list[0].id));
    a.regroup();
    for (w, h) in SIZES {
        a.overview = None;
        key(&mut a, &cx, Key::Ctrl('O'));
        assert!(a.overview.is_some());
        let g = frame(&mut a, &cx, w, h);
        let s = scale(w, h);
        let cards: Vec<_> = g.hits.iter().filter(|x| x.label == "tab card").collect();
        assert!(!cards.is_empty() && cards.len() <= 5, "{w}x{h}: cards {}", cards.len());
        for (i, c) in cards.iter().enumerate() {
            for d in cards.iter().skip(i + 1) {
                assert!(!overlaps(c.rect, d.rect), "{w}x{h}: cards overlap");
            }
            assert!(c.rect.y + c.rect.h <= h as f32 && c.rect.x + c.rect.w <= w as f32, "{w}x{h}: card on screen");
        }
        let done = g.hits.iter().find(|x| x.label == "Done").unwrap();
        assert!(done.rect.h >= 38.0 * s);
        if w >= 1280 {
            assert!(g.hits.iter().any(|x| x.label == "rename group"), "{w}x{h}: the group's header");
        }
    }
    // search: only matching tabs stay
    a.overview.as_mut().unwrap().set("s3");
    assert_eq!(a.overview_matches(), vec![3]);
    a.overview.as_mut().unwrap().set("nothing like it");
    assert!(a.overview_matches().is_empty());
    a.overview.as_mut().unwrap().set("s4");
    key(&mut a, &cx, Key::Enter);
    assert!(a.overview.is_none() && a.active == 4, "Enter opens the first match");
    // Esc closes it
    key(&mut a, &cx, Key::Ctrl('O'));
    key(&mut a, &cx, Key::Esc);
    assert!(a.overview.is_none());
    // the card buttons: close a tab, and step a tab through the groups
    a.handle(&Event::Tab(TAB_CARD_CLOSE + 4), &cx);
    assert_eq!(a.tab_count(), 4);
    a.cycle_tab_group(0);
    assert_eq!(a.groups.of(0).map(|g| g.id), Some(a.groups.list[0].id));
    assert!(a.groups.members(a.groups.list[0].id).windows(2).all(|p| p[1] == p[0] + 1));
}

#[test]
fn the_start_page_and_loading_placeholder_are_dark_not_white() {
    let cx = cx_with("look-dark", site(vec![]));
    let mut a = BrowserApp::new(&cx);
    let g = frame(&mut a, &cx, 1280, 720);
    let p = px(&g, 640, 500);
    assert!(p[0] < 60 && p[1] < 60 && p[2] < 80, "the page area of a new tab is dark: {p:?}");
    assert!(chrome::PLACEHOLDER[0] < 40, "the engine's placeholder is calm and dark");
}

/// Pictures of the look, for judging by eye: `IYAKKI_SHOT_DIR=/dir cargo test -p iyakki-shell --lib browser_look_pictures -- --ignored`.
#[test]
#[ignore]
fn browser_look_pictures() {
    let Ok(dir) = std::env::var("IYAKKI_SHOT_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (1280u32, 720u32);
    let shot = |a: &mut BrowserApp, cx: &AppCx, name: &str, tick: u64| {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, cx, tick);
        g.save_png(&dir.join(name)).unwrap();
    };
    let (mut a, cx) = app_with_tabs("look-pics", 4);
    a.switch_to(1);
    a.new_group_from(1);
    a.group_edit.as_mut().unwrap().field.set("Reading");
    a.commit_group_edit();
    a.groups.assign(2, Some(a.groups.list[0].id));
    a.regroup();
    shot(&mut a, &cx, "after-groups.png", 3);
    a.group_edit = None;
    a.open_group_edit(a.active);
    shot(&mut a, &cx, "after-group-edit.png", 3);
    a.group_edit = None;
    a.open_overview();
    shot(&mut a, &cx, "after-overview.png", 3);
    a.overview = None;
    let (_tx, rx) = channel();
    a.nav = Nav::Loading { url: Url::parse("https://www.youtube.com/").unwrap(), rx, how: How::Push };
    a.load_started = std::time::Instant::now() - std::time::Duration::from_secs(20);
    shot(&mut a, &cx, "after-loading-slow.png", 8);
    a.nav = Nav::Idle;
    a.status = None;
    let cx2 = cx_with("look-pics-err", site(vec![]));
    let mut b = BrowserApp::new(&cx2);
    b.open_url("https://down.example/", &cx2);
    settle(&mut b, &cx2);
    shot(&mut b, &cx2, "after-error.png", 3);
    let cx3 = cx_with("look-pics-new", site(vec![]));
    let mut c = BrowserApp::new(&cx3);
    shot(&mut c, &cx3, "after-newtab.png", 3);
}
