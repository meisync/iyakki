//! Tests of the DASH tile: geometry at the four reference sizes with the chat open and closed, the click ids, the panel
//! workflow (data arrives from a fake server, zoom by dragging, legend, editor, undo), sources and secrets, persistence, the
//! assistant sharing the open dashboard, and the tile's registration.

use std::sync::{Arc, Mutex};

use iyakki_agent::dash::connector::{Deps, NetOptions, SecretResolver, Transport, Zeroizing};
use iyakki_agent::dash::model::*;
use iyakki_agent::dash::tools::Env;
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::wsexec::FakeRunner;

use super::dash::*;
use super::tests::{cx_with_admin_store, temp_dir};
use super::{App, AppCx};
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};

const SECRET: &str = "S3CRET-NEVER-SHOWN";
const NOW: i64 = 1_792_069_200_000;

struct Up {
    rules: Vec<(String, u16, String)>,
    seen: Mutex<Vec<String>>,
}

impl Upstream for Up {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        let line = format!("{method} {url} {}", String::from_utf8_lossy(body));
        self.seen.lock().unwrap().push(format!("{line} || {:?}", headers.iter().map(|h| h.0.clone()).collect::<Vec<_>>()));
        for (k, status, body) in &self.rules {
            if line.contains(k.as_str()) {
                if *status == 0 {
                    return Err(body.clone());
                }
                return Ok(UpstreamResponse { status: *status, body: body.clone().into_bytes() });
            }
        }
        Ok(UpstreamResponse { status: 404, body: format!("no rule for {line}").into_bytes() })
    }
}

struct Tr(Arc<Up>);
impl Transport for Tr {
    fn upstream(&self, _: &NetOptions) -> Result<Arc<dyn Upstream>, String> {
        Ok(self.0.clone())
    }
}

struct Sec;
impl SecretResolver for Sec {
    fn resolve(&self, r: &str) -> Result<Zeroizing<String>, String> {
        if r == "uvault:svc.prom" { Ok(Zeroizing::new(SECRET.into())) } else { Err("no such secret".into()) }
    }
}

/// A Prometheus matrix with `n` series of 120 points.
fn matrix(n: usize) -> String {
    let series: Vec<String> = (0..n)
        .map(|k| {
            let pts: Vec<String> = (0..120).map(|i| format!("[{},\"{}\"]", 1_792_065_600 + i * 30, 40.0 + 25.0 * ((i as f64) / 12.0 + k as f64).sin() + if (80..84).contains(&i) { 30.0 } else { 0.0 })).collect();
            format!(r#"{{"metric":{{"instance":"node-{k}:9100"}},"values":[{}]}}"#, pts.join(","))
        })
        .collect();
    format!(r#"{{"status":"success","data":{{"resultType":"matrix","result":[{}]}}}}"#, series.join(","))
}

fn up() -> Arc<Up> {
    Arc::new(Up {
        rules: vec![
            ("http://localhost:".into(), 0, "Connection refused (os error 111)".into()),
            ("query_range".into(), 200, matrix(3)),
            ("/api/v1/query?".into(), 200, r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"instance":"a"},"value":[1792069200,"42.5"]}]}}"#.into()),
            ("label/__name__/values".into(), 200, r#"{"status":"success","data":["node_load1","up","node_cpu_seconds_total"]}"#.into()),
            ("buildinfo".into(), 200, r#"{"status":"success","data":{"version":"2.53.0"}}"#.into()),
        ],
        seen: Mutex::new(vec![]),
    })
}

struct Fixture {
    app: DashApp,
    cx: AppCx,
    up: Arc<Up>,
    env: Arc<Env>,
    /// What was written to the vault (name, value) and every line the tile put in the audit log.
    vault: Arc<Mutex<Vec<(String, String)>>>,
    audit: Arc<Mutex<Vec<String>>>,
}

fn fixture(tag: &str) -> Fixture {
    fixture_c(tag, Arc::new(iyakki_agent::dash::cluster::NoCluster), true)
}

fn fixture_c(tag: &str, cluster: Arc<dyn iyakki_agent::dash::cluster::ClusterApi>, admin: bool) -> Fixture {
    let cx = cx_with_admin_store(tag);
    let dir = temp_dir(&format!("dash-{tag}"));
    let up = up();
    let vault: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![("svc.prom".into(), SECRET.into()), ("svc.other".into(), "other-secret-value".into())]));
    let audit: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let (v1, v2, a1) = (vault.clone(), vault.clone(), audit.clone());
    let env = Arc::new(Env {
        user: cx.session.user.clone(),
        policy: if admin { cx.session.policy.clone() } else { iyakki_sec::policy::Role::User.template() },
        data_dir: dir.clone(),
        home: dir.join("home"),
        deps: Deps { transport: Arc::new(Tr(up.clone())), secrets: Arc::new(Sec), runner: Arc::new(FakeRunner::new(&[])), path_ok: Arc::new(|_| true), work_dir: dir.clone(), cluster },
        now_ms: Arc::new(|| NOW),
        vault_names: Arc::new(move || v1.lock().unwrap().iter().map(|x| x.0.clone()).collect()),
        vault_set: Arc::new(move |n, v| {
            v2.lock().unwrap().push((n.to_string(), v.to_string()));
            Ok(())
        }),
        audit: Arc::new(move |k, d| a1.lock().unwrap().push(format!("{k} {d}"))),
        cache: iyakki_agent::dash::multi::FetchCache::new(),
        people: Arc::new(iyakki_agent::dash::tools::AnyPeople),
    });
    let mut c = iyakki_agent::dash::connector::SourceConfig { id: "prom".into(), kind: "prometheus".into(), name: "Prod".into(), ..Default::default() };
    c.values.insert("url".into(), "http://prom.internal:9090".into());
    c.values.insert("auth".into(), "bearer".into());
    c.secrets.insert("token".into(), "uvault:svc.prom".into());
    env.sources().save(c).unwrap();
    let app = DashApp::with_env(&cx, env.clone());
    Fixture { app, cx, up, env, vault, audit }
}

fn area(w: u32, h: u32) -> (R, f32) {
    let s = crate::shell::scale_for(w, h);
    let hh = 46.0 * s;
    (R::new(0.0, hh, w as f32, h as f32 - hh), s)
}

fn draw(f: &mut Fixture, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    crate::fx::background(&mut g);
    let (a, s) = area(w, h);
    f.app.render(&mut g, a, s, &f.cx, 7);
    g
}

fn tick(f: &mut Fixture, n: usize) {
    for _ in 0..n {
        f.app.handle(&Event::Tick, &f.cx);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Tick until every panel has been answered.
fn settle(f: &mut Fixture) {
    for _ in 0..400 {
        tick(f, 1);
        let st = f.app.state();
        let g = st.lock().unwrap();
        if !g.dashboard.panels.is_empty() && g.dashboard.panels.iter().filter(|p| p.visual != Visual::Text).all(|p| g.results.contains_key(&p.id)) {
            return;
        }
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the panels never got their data");
}

fn click(f: &mut Fixture, label: &str) {
    let g = draw(f, 1280, 720);
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing labelled `{label}`: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>())).clone();
    let (x, y) = ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32);
    f.app.handle(&Event::MouseMove(x, y), &f.cx);
    f.app.handle(&h.ev.clone(), &f.cx);
}

fn key(f: &mut Fixture, k: Key) {
    f.app.handle(&Event::Key(k), &f.cx);
}

fn types(f: &mut Fixture, t: &str) {
    for c in t.chars() {
        key(f, Key::Char(c));
    }
}

/// A board with four panels of different visuals, loaded.
fn board(tag: &str) -> Fixture {
    let mut f = fixture(tag);
    let d = {
        let mut d = Dashboard::new("", "Checkout");
        d.add_panel(Panel::new("", "CPU by node", "prom", Visual::Area, "rate(cpu[5m])", Grid::new(0, 0, 12, 8)).unit(Unit::Percent).legend_format("{{instance}}")).unwrap();
        d.add_panel(Panel::new("", "Load", "prom", Visual::Stat, "node_load1", Grid::new(0, 0, 6, 5)).unit(Unit::Number)).unwrap();
        d.add_panel(Panel::new("", "Hosts", "prom", Visual::Table, "up", Grid::new(0, 0, 12, 7))).unwrap();
        d.add_panel(Panel::new("", "Note", "", Visual::Text, "", Grid::new(0, 0, 6, 4))).unwrap();
        d.panels[3].options.text = "# Runbook\n- check the pods\n- page the owner".into();
        d
    };
    f.app.open_dashboard_for_test(d);
    settle(&mut f);
    f
}

#[test]
fn the_tile_is_registered_once_in_the_work_group_and_roles_follow_policy() {
    use iyakki_sec::policy::{App as PApp, Role};
    assert_eq!(PApp::Dash.id(), "dash");
    assert_eq!(PApp::ALL.iter().filter(|a| **a == PApp::Dash).count(), 1);
    let t: Vec<_> = crate::shell::tiles_for_test().into_iter().filter(|t| t.0 == PApp::Dash).collect();
    assert_eq!(t.len(), 1, "one tile");
    assert_eq!(t[0].1, "DASH");
    assert_eq!(crate::launcher::Group::of(PApp::Dash), Some(crate::launcher::Group::Data));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Dash), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Dash));
}

#[test]
fn every_screen_fits_at_the_four_sizes_with_the_chat_open_and_closed() {
    let mut f = board("geo");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for chat in [false, true] {
            for screen in ["board", "list", "sources", "editor", "zoom"] {
                f.app.go_for_test(screen);
                f.app.set_chat_for_test(chat);
                let g = draw(&mut f, w, h);
                let (a, s) = area(w, h);
                let ctx = format!("{screen} {w}x{h} chat={chat}");
                // every click region lies inside the window
                for hit in &g.hits {
                    if hit.label == "backdrop" || hit.label == "slot-mark" {
                        continue;
                    }
                    let r = hit.rect;
                    assert!(r.x >= -1.0 && r.y >= a.y - s * 40.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{ctx}: `{}` at {r:?}", hit.label);
                }
                // click ids are never reused for different rectangles
                let dup = super::component::route::duplicate_ids(&g.hits);
                assert!(dup.is_empty(), "{ctx}: {dup:?}");
                // the board's panels do not overlap each other or the chat
                let panels = f.app.panel_rects_for_test();
                for (i, (_, a1)) in panels.iter().enumerate() {
                    for (_, b1) in panels.iter().skip(i + 1) {
                        assert!(a1.x + a1.w <= b1.x + 0.5 || b1.x + b1.w <= a1.x + 0.5 || a1.y + a1.h <= b1.y + 0.5 || b1.y + b1.h <= a1.y + 0.5, "{ctx}: panels overlap {a1:?} {b1:?}");
                    }
                }
                if let Some(c) = f.app.chat_rect_for_test() {
                    // main controls (ids below the chat slot) never sit under the chat pane
                    let in_slot = |ev: &Event| match ev { Event::Tab(n) | Event::Row(n) | Event::Focus(n) | Event::Area(n) => *n >= 6_000_000, _ => true };
                    for hit in g.hits.iter().filter(|h| h.label != "backdrop" && h.label != "slot-mark" && !in_slot(&h.ev) && h.label != "chat pane" && h.label != "Hide") {
                        let r = hit.rect;
                        let overlap = r.x < c.x + c.w - 0.5 && c.x < r.x + r.w - 0.5 && r.y < c.y + c.h - 0.5 && c.y < r.y + r.h - 0.5;
                        assert!(!overlap, "{ctx}: `{}` {r:?} is under the chat {c:?}", hit.label);
                    }
                    if w == 800 {
                        assert_eq!(c.w, a.w, "{ctx}: in a small window the chat takes the whole area");
                    } else {
                        assert!(c.w < a.w * 0.5, "{ctx}: beside the board when there is room");
                    }
                }
            }
        }
    }
}

#[test]
fn buttons_of_one_toolbar_never_overlap() {
    let mut f = board("tb");
    let labels = ["Dashboards", "Zoom out", "Refresh", "+ Panel", "Sources", "Undo", "Redo", "Save", "Export", "AI", "5m", "15m", "1h", "6h", "24h", "7d"];
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        f.app.go_for_test("board");
        f.app.set_chat_for_test(false);
        let g = draw(&mut f, w, h);
        let hits: Vec<_> = g.hits.iter().filter(|h| labels.contains(&h.label.as_str())).collect();
        assert!(hits.len() >= 10, "{w}x{h}: {}", hits.len());
        for (i, a) in hits.iter().enumerate() {
            for b in hits.iter().skip(i + 1) {
                let (p, q) = (a.rect, b.rect);
                let ov = p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                assert!(!ov, "{w}x{h}: `{}` and `{}` overlap", a.label, b.label);
            }
        }
    }
}

#[test]
fn data_arrives_from_the_server_and_the_secret_travels_only_in_a_header() {
    let f = board("data");
    let seen = f.up.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|l| l.contains("query_range") && l.contains("step=")), "{seen:?}");
    assert!(seen.iter().all(|l| !l.contains(SECRET)), "the secret is never in a URL or body");
    assert!(seen.iter().any(|l| l.contains("\"Authorization\"")), "it is sent as a header");
    let st = f.app.state();
    let g = st.lock().unwrap();
    assert!(g.results.get("p1").is_some_and(|r| r.summary.contains("3 series")), "{:?}", g.results.get("p1"));
    assert!(g.results.get("p4").is_none(), "a text panel asks nothing");
}

#[test]
fn the_board_draws_charts_and_can_be_looked_at() {
    let mut f = board("png");
    f.app.set_chat_for_test(false);
    let g = draw(&mut f, 1280, 720);
    // not an empty screen: plenty of coloured pixels from the chart lines
    let px = g.rgba();
    let lit = px.chunks(4).filter(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) > 300).count();
    assert!(lit > 4000, "{lit}");
    if let Ok(dir) = std::env::var("IYAKKI_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        g.save_png(&std::path::Path::new(&dir).join("dash-board.png")).unwrap();
        f.app.set_chat_for_test(true);
        draw(&mut f, 1280, 720).save_png(&std::path::Path::new(&dir).join("dash-board-chat.png")).unwrap();
        f.app.go_for_test("list");
        draw(&mut f, 1280, 720).save_png(&std::path::Path::new(&dir).join("dash-list.png")).unwrap();
        f.app.go_for_test("sources");
        draw(&mut f, 1280, 720).save_png(&std::path::Path::new(&dir).join("dash-sources.png")).unwrap();
        f.app.go_for_test("editor");
        draw(&mut f, 1280, 720).save_png(&std::path::Path::new(&dir).join("dash-editor.png")).unwrap();
        f.app.go_for_test("zoom");
        draw(&mut f, 1280, 720).save_png(&std::path::Path::new(&dir).join("dash-zoom.png")).unwrap();
        f.app.set_chat_for_test(false);
        f.app.go_for_test("board");
        let (a, s) = area(800, 480);
        let mut g2 = Gfx::new(800, 480);
        crate::fx::background(&mut g2);
        f.app.render(&mut g2, a, s, &f.cx, 3);
        g2.save_png(&std::path::Path::new(&dir).join("dash-board-800.png")).unwrap();
    }
}

#[test]
fn dragging_across_a_chart_zooms_the_range_and_undo_brings_it_back() {
    let mut f = board("zoom");
    f.app.set_chat_for_test(false);
    let g = draw(&mut f, 1280, 720);
    let plot = g.hits.iter().find(|h| h.label == "chart").expect("a chart is on the board").rect;
    let (x0, y) = ((plot.x + plot.w * 0.25) as i32, (plot.y + plot.h / 2.0) as i32);
    let x1 = (plot.x + plot.w * 0.5) as i32;
    f.app.handle(&Event::MouseMove(x0, y), &f.cx);
    f.app.handle(&Event::AreaAt(400_000, x0 as f32 - plot.x, y as f32 - plot.y), &f.cx);
    f.app.handle(&Event::MouseMove(x1, y), &f.cx);
    f.app.handle(&Event::MouseUp(x1, y), &f.cx);
    let t = f.app.state().lock().unwrap().dashboard.time.clone();
    assert!(t.from.parse::<i64>().is_ok() && t.to.parse::<i64>().is_ok(), "{t:?}");
    let (a, b): (i64, i64) = (t.from.parse().unwrap(), t.to.parse().unwrap());
    let span = b - a;
    assert!(span > 0 && span < 3_600_000 / 3, "about a quarter of an hour of the hour: {span}");
    key(&mut f, Key::Ctrl('z'));
    assert_eq!(f.app.state().lock().unwrap().dashboard.time.from, "now-1h");
    // a tiny drag is a click, not a zoom
    f.app.handle(&Event::AreaAt(400_000, x0 as f32 - plot.x, 10.0), &f.cx);
    f.app.handle(&Event::MouseMove(x0 + 2, y), &f.cx);
    f.app.handle(&Event::MouseUp(x0 + 2, y), &f.cx);
    assert_eq!(f.app.state().lock().unwrap().dashboard.time.from, "now-1h");
}

#[test]
fn clicking_a_legend_entry_hides_that_line_without_changing_the_dashboard() {
    let mut f = board("legend");
    f.app.set_chat_for_test(false);
    let rev = f.app.state().lock().unwrap().rev;
    click(&mut f, "legend node-1:9100");
    assert_eq!(f.app.state().lock().unwrap().rev, rev, "hiding a line is a view, not an edit");
    assert_eq!(f.app.hidden_for_test("p1"), vec!["node-1:9100".to_string()]);
    click(&mut f, "legend node-1:9100");
    assert!(f.app.hidden_for_test("p1").is_empty());
}

#[test]
fn panels_move_resize_and_the_keyboard_works_too() {
    let mut f = board("move");
    f.app.set_chat_for_test(false);
    key(&mut f, Key::Down);
    let sel = f.app.state().lock().unwrap().selected.clone();
    assert!(sel.is_some(), "an arrow selects a panel");
    let before = f.app.state().lock().unwrap().dashboard.panel(sel.as_ref().unwrap()).unwrap().grid;
    key(&mut f, Key::Nav { nav: crate::ui::Nav::Right, shift: true, ctrl: false });
    let after = f.app.state().lock().unwrap().dashboard.panel(sel.as_ref().unwrap()).unwrap().grid;
    assert!(after != before, "Shift+arrow moves it: {before:?} -> {after:?}");
    key(&mut f, Key::Nav { nav: crate::ui::Nav::Down, shift: false, ctrl: true });
    assert_eq!(f.app.state().lock().unwrap().dashboard.panel(sel.as_ref().unwrap()).unwrap().grid.h, after.h + 1);
    // a drag on the title bar moves it by whole cells
    let g = draw(&mut f, 1280, 720);
    let hh = g.hits.iter().rev().find(|h| h.label.starts_with("move ")).unwrap().clone();
    let (x, y) = ((hh.rect.x + 30.0) as i32, (hh.rect.y + 6.0) as i32);
    f.app.handle(&Event::MouseMove(x, y), &f.cx);
    f.app.handle(&hh.ev.clone(), &f.cx);
    f.app.handle(&Event::MouseMove(x + 120, y + 60), &f.cx);
    draw(&mut f, 1280, 720);
    f.app.handle(&Event::MouseUp(x + 120, y + 60), &f.cx);
    let d = f.app.state().lock().unwrap().dashboard.clone();
    for (i, a) in d.panels.iter().enumerate() {
        for b in d.panels.iter().skip(i + 1) {
            assert!(!a.grid.overlaps(&b.grid), "no overlap after moves");
        }
    }
    // delete and undo
    let n = d.panels.len();
    key(&mut f, Key::Delete);
    assert_eq!(f.app.state().lock().unwrap().dashboard.panels.len(), n - 1);
    key(&mut f, Key::Ctrl('z'));
    assert_eq!(f.app.state().lock().unwrap().dashboard.panels.len(), n);
}

#[test]
fn the_editor_previews_and_applies_a_new_panel_and_undo_removes_it() {
    let mut f = board("editor");
    f.app.set_chat_for_test(false);
    let n = f.app.state().lock().unwrap().dashboard.panels.len();
    click(&mut f, "+ Panel");
    f.app.go_for_test("editor-check");
    // title, then the query
    types(&mut f, "Memory");
    key(&mut f, Key::Tab);
    key(&mut f, Key::Tab);
    key(&mut f, Key::Tab);
    types(&mut f, "node_memory_free");
    key(&mut f, Key::Ctrl('r'));
    for _ in 0..300 {
        tick(&mut f, 1);
        if f.app.preview_ready_for_test() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(f.app.preview_ready_for_test(), "the preview came back");
    key(&mut f, Key::Ctrl('s'));
    let st = f.app.state();
    let g = st.lock().unwrap();
    assert_eq!(g.dashboard.panels.len(), n + 1);
    let p = g.dashboard.panels.last().unwrap();
    assert_eq!((p.title.as_str(), p.query.text.as_str(), p.source.as_str()), ("Memory", "node_memory_free", "prom"));
    assert_eq!(g.highlight.as_deref().or(g.selected.as_deref()), Some(p.id.as_str()));
    drop(g);
    key(&mut f, Key::Ctrl('z'));
    assert_eq!(st.lock().unwrap().dashboard.panels.len(), n);
}

#[test]
fn typing_goes_only_to_the_pane_that_owns_the_keyboard() {
    let mut f = board("focus");
    f.app.set_chat_for_test(true);
    // the chat is open and focused: letters go to its composer
    assert!(f.app.chat_focused_for_test());
    types(&mut f, "hello");
    assert_eq!(f.app.chat_input_for_test(), "hello");
    assert_eq!(f.app.state().lock().unwrap().dashboard.panels.len(), 4, "keys did not reach the board (no shortcut fired)");
    // F6 gives the keyboard to the board: 'a' now opens the editor instead of reaching the chat
    key(&mut f, Key::F(6));
    assert!(!f.app.chat_focused_for_test());
    key(&mut f, Key::Char('a'));
    assert_eq!(f.app.chat_input_for_test(), "hello");
    assert!(f.app.in_editor_for_test());
    // in the editor, typing reaches the title only, not the chat
    types(&mut f, "X");
    assert_eq!(f.app.chat_input_for_test(), "hello");
    assert!(f.app.editor_title_for_test().ends_with('X'));
}

#[test]
fn ask_ai_prefills_the_chat_with_the_panel_and_the_moment() {
    let mut f = board("ask");
    f.app.set_chat_for_test(false);
    key(&mut f, Key::Down);
    key(&mut f, Key::Char('i'));
    assert!(f.app.chat_open_for_test() && f.app.chat_focused_for_test());
    let t = f.app.chat_input_for_test();
    assert!(t.contains("Explain panel p1") && t.contains("CPU by node"), "{t}");
    assert_eq!(f.app.state().lock().unwrap().selected.as_deref(), Some("p1"));
}

#[test]
fn the_assistant_edits_the_open_dashboard_and_the_screen_follows_and_highlights() {
    let mut f = board("assist");
    f.app.set_chat_for_test(false);
    let st = f.app.state();
    let r = f.env.call("dash_panel_add", &serde_json::json!({"title": "Memory", "source": "prom", "query": "node_memory_free", "visual": "line"}), Some(&st)).unwrap();
    assert!(r.contains("Added panel p5"), "{r}");
    draw(&mut f, 1280, 720);
    assert_eq!(f.app.panel_rects_for_test().len(), 5, "all five panels are drawn");
    assert!(f.app.flashing_for_test("p5"));
    settle_all(&mut f);
    assert!(st.lock().unwrap().results.contains_key("p5"), "the new panel asked its source");
    // the person undoes it with Ctrl+Z
    key(&mut f, Key::Ctrl('z'));
    assert_eq!(st.lock().unwrap().dashboard.panels.len(), 4);
}

fn settle_all(f: &mut Fixture) {
    for _ in 0..300 {
        tick(f, 1);
        let st = f.app.state();
        let g = st.lock().unwrap();
        if g.dashboard.panels.iter().filter(|p| p.visual != Visual::Text).all(|p| g.results.contains_key(&p.id)) {
            return;
        }
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    panic!("not settled");
}

#[test]
fn saving_lists_and_reopening_a_dashboard_round_trips_through_the_folder() {
    let mut f = board("save");
    key(&mut f, Key::Ctrl('s'));
    let dir = f.env.home.join("Dashboards");
    assert!(dir.join("checkout.json").is_file(), "{:?}", std::fs::read_dir(&dir).map(|d| d.count()));
    let raw = std::fs::read_to_string(dir.join("checkout.json")).unwrap();
    assert!(!raw.contains(SECRET) && !raw.contains("uvault") && !raw.contains("prom.internal"), "a dashboard file holds no credentials and no addresses");
    f.app.go_for_test("list");
    let mut fresh = DashApp::with_env(&f.cx, f.env.clone());
    let mut g = Gfx::new(1280, 720);
    let (a, s) = area(1280, 720);
    fresh.render(&mut g, a, s, &f.cx, 1);
    assert!(g.hits.iter().any(|h| h.label == "row"));
    fresh.handle(&Event::Key(Key::Enter), &f.cx);
    assert_eq!(fresh.state().lock().unwrap().dashboard.panels.len(), 4);
    // leaving with unsaved changes asks first
    fresh.handle(&Event::Key(Key::Down), &f.cx);
    fresh.handle(&Event::Key(Key::Delete), &f.cx);
    fresh.handle(&Event::Key(Key::Esc), &f.cx);
    assert!(fresh.modal_open_for_test(), "unsaved changes are not dropped silently");
}

#[test]
fn a_template_needs_its_kind_of_source_and_opens_bound_to_it() {
    let mut f = fixture("tpl");
    f.app.go_for_test("list");
    // the first template is Prometheus node_exporter
    let n = f.app.list_len_for_test();
    f.app.select_list_for_test(n);
    key(&mut f, Key::Enter);
    assert_eq!(f.app.state().lock().unwrap().dashboard.panels[0].source, "prom");
    assert!(f.app.state().lock().unwrap().dashboard.panels.len() >= 6);
    // a template whose source kind is missing sends the person to Sources with a sentence
    f.app.go_for_test("list");
    f.app.select_list_for_test(n + 3);
    key(&mut f, Key::Enter);
    assert!(f.app.status_for_test().contains("needs a ClickHouse source"), "{}", f.app.status_for_test());
    assert!(f.app.on_screen_for_test("sources"));
}

#[test]
fn the_add_source_form_stores_references_never_secrets_and_lists_only_vault_names() {
    let mut f = fixture("src");
    f.app.go_for_test("sources");
    key(&mut f, Key::Char('a'));
    // the kinds picker: choose Elasticsearch
    types(&mut f, "Elasticsearch");
    key(&mut f, Key::Enter);
    let form = f.app.source_form_labels_for_test();
    assert!(form.iter().any(|l| l == "Name") && form.iter().any(|l| l.contains("Address")), "{form:?}");
    assert!(!form.iter().any(|l| l.to_lowercase().contains("password") || l.to_lowercase().contains("token")), "secret fields appear only for the sign-in that needs them: {form:?}");
    f.app.fill_source_form_for_test(&[("Name", "Logs"), ("Address", "http://es.internal:9200")]);
    key(&mut f, Key::Ctrl('s'));
    let all = f.env.sources().list();
    let es = all.iter().find(|s| s.kind == "elasticsearch").expect("saved");
    assert_eq!(es.value("url"), "http://es.internal:9200");
    assert!(es.secrets.is_empty());
    // editing offers vault names, and the choice is stored as a reference
    f.app.go_for_test("sources");
    f.app.select_source_for_test("prom");
    key(&mut f, Key::Char('e'));
    let choices = f.app.source_form_choices_for_test();
    assert!(choices.iter().any(|c| c.contains(&"uvault:svc.prom".to_string())) && choices.iter().all(|c| c.iter().all(|o| !o.contains(SECRET))), "{choices:?}");
    key(&mut f, Key::Esc);
    let raw = std::fs::read_to_string(f.env.data_dir.join("dash").join(format!("sources-{}.json", f.env.user))).unwrap();
    assert!(raw.contains("uvault:svc.prom") && !raw.contains(SECRET));
}

#[test]
fn testing_a_source_reports_in_the_row_and_errors_explain_the_fix() {
    let mut f = fixture("test");
    f.app.go_for_test("sources");
    key(&mut f, Key::Char('t'));
    for _ in 0..200 {
        tick(&mut f, 1);
        if f.app.source_status_for_test("prom").is_some_and(|s| s.contains("Prometheus 2.53.0")) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    panic!("no test result: {:?}", f.app.source_status_for_test("prom"));
}

#[test]
fn an_error_is_shown_inside_the_panel_not_as_a_popup() {
    let mut f = fixture("err");
    let mut d = Dashboard::new("", "Broken");
    d.add_panel(Panel::new("", "Bad", "nope", Visual::Line, "up", Grid::new(0, 0, 12, 8))).unwrap();
    f.app.open_dashboard_for_test(d);
    settle(&mut f);
    assert!(!f.app.modal_open_for_test());
    let st = f.app.state();
    let g = st.lock().unwrap();
    let e = g.results.get("p1").and_then(|r| r.error.clone()).unwrap();
    assert!(e.contains("there is no source `nope`"), "{e}");
    drop(g);
    let img = draw(&mut f, 1280, 720);
    assert!(img.hits.iter().any(|h| h.label.starts_with("panel Bad")));
}

#[test]
fn leaving_the_tile_cancels_what_is_still_running() {
    let f = board("cancel");
    let token = f.app.cancel_token_for_test();
    assert!(!token.is_cancelled());
    drop(f.app);
    assert!(token.is_cancelled());
}

#[test]
fn the_context_menu_copies_the_panel_data_as_csv() {
    let mut f = board("ctx");
    f.app.set_chat_for_test(false);
    draw(&mut f, 1280, 720);
    let (_, r) = f.app.panel_rects_for_test()[0];
    let items = f.app.context_items((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert!(ids.contains(&"dash.copy") && ids.contains(&"dash.ask") && ids.contains(&"dash.edit"), "{ids:?}");
    f.app.context_pick("dash.copy", &f.cx);
    let clip = crate::ui::clipboard::latest().unwrap_or_default();
    assert!(clip.starts_with("time,node-0:9100") || clip.starts_with("time,"), "{}", &clip[..clip.len().min(60)]);
}

#[test]
fn a_table_sorts_pages_and_copies_a_cell() {
    let mut f = board("table");
    f.app.go_for_test("zoom");
    draw(&mut f, 1280, 720);
    key(&mut f, Key::Down);
    key(&mut f, Key::Char('c'));
    assert!(crate::ui::clipboard::latest().is_some());
    let g = draw(&mut f, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("sort ")), "headers sort");
}

fn mixed_panel(title: &str) -> Panel {
    let mut p = Panel::new("", title, "prom", Visual::Line, "cpu", Grid::new(0, 0, 12, 8)).legend_format("{{instance}}");
    p.queries.push(QueryDef { source: "es".into(), query: PanelQuery { text: "{\"aggs\":{}}".into(), target: Some("logs-*".into()), axis: Axis::Right, alias: "errors".into(), unit: Some(Unit::Number), ..Default::default() } });
    p.expressions = vec!["C = B / A * 100".into(), "align nearest 60s".into()];
    p
}

fn with_es(f: &Fixture) {
    let mut es = iyakki_agent::dash::connector::SourceConfig { id: "es".into(), kind: "elasticsearch".into(), name: "Logs".into(), ..Default::default() };
    es.values.insert("url".into(), "http://es.internal:9200".into());
    f.env.sources().save(es).unwrap();
}

#[test]
fn a_failing_source_leaves_the_panel_with_the_other_sources_data_and_a_badge_and_identical_queries_share_a_fetch() {
    let mut f = fixture("partial");
    with_es(&f);
    f.app.reload_for_test();
    let mut d = Dashboard::new("", "Mixed");
    d.add_panel(mixed_panel("CPU and errors")).unwrap();
    d.add_panel(Panel::new("", "CPU again", "prom", Visual::Area, "cpu", Grid::new(0, 0, 12, 8))).unwrap();
    f.app.open_dashboard_for_test(d);
    settle(&mut f);
    // the Elasticsearch part has no answer (404): the Prometheus data is still there, the failure is named
    let q = f.app.qerrors_for_test("p1");
    assert_eq!(q.len(), 2, "{q:?}");
    assert!(q[0].starts_with("B (es):"), "{q:?}");
    assert!(q[1].starts_with("expression C:"), "the line that needs B says so: {q:?}");
    assert!(f.app.has_frame_for_test("p1"), "the panel still has data");
    assert!(f.app.qerrors_for_test("p2").is_empty());
    let prom_fetches = f.up.seen.lock().unwrap().iter().filter(|l| l.contains("query_range")).count();
    assert_eq!(prom_fetches, 1, "the same question of two panels is one request: {:?}", f.up.seen.lock().unwrap());
    // no popup: the failure is inside the panel
    assert!(!f.app.modal_open_for_test());
    let g = draw(&mut f, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("panel CPU and errors")));
    // the assistant's context mentions the extra query and the problem
    let ctx = iyakki_agent::dash::tools::DashTools::context(&f.env, &f.app.state());
    assert!(ctx.contains("+1 query on Logs, 2 expression(s)") && ctx.contains("B (es)"), "{ctx}");
}

#[test]
fn the_multi_query_editor_fits_at_the_four_sizes_with_its_tabs() {
    let mut f = fixture("edtabs");
    with_es(&f);
    f.app.reload_for_test();
    let mut d = Dashboard::new("", "Mixed");
    d.add_panel(mixed_panel("CPU and errors")).unwrap();
    f.app.open_dashboard_for_test(d);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for chat in [false, true] {
            for (page, expr, cur) in [(0usize, false, 0usize), (1, false, 1), (1, true, 0), (2, false, 0)] {
                f.app.go_for_test("editor");
                f.app.set_chat_for_test(chat);
                f.app.set_editor_view_for_test(page, expr, cur);
                let g = draw(&mut f, w, h);
                let ctx = format!("{w}x{h} chat={chat} page={page} expr={expr} cur={cur}");
                for hit in g.hits.iter().filter(|x| x.label != "backdrop" && x.label != "slot-mark") {
                    let r = hit.rect;
                    assert!(r.x >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{ctx}: `{}` {r:?}", hit.label);
                }
                let dup = super::component::route::duplicate_ids(&g.hits);
                assert!(dup.is_empty(), "{ctx}: {dup:?}");
                // the pills of the tab rows never overlap each other
                let tab_hit = |x: &crate::ui::Hit| matches!(x.ev, Event::Tab(n) if (300..316).contains(&n) || n == 183);
                let tabs: Vec<_> = g.hits.iter().filter(|x| tab_hit(x) && x.rect.w > 0.0).collect();
                if !(chat && w < 1000) && (page == 1 || (wide(w, chat) && page == 0)) {
                    assert!(tabs.len() >= if expr { 4 } else { 5 }, "{ctx}: the query tabs are drawn ({})", tabs.len());
                }
                for (i, a) in tabs.iter().enumerate() {
                    for b in tabs.iter().skip(i + 1) {
                        let (p, q) = (a.rect, b.rect);
                        let ov = p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                        assert!(!ov, "{ctx}: `{}` and `{}` overlap", a.label, b.label);
                    }
                }
            }
        }
    }
}

fn wide(w: u32, chat: bool) -> bool {
    let (_, s) = area(w, 720);
    let main = if chat && w >= 1000 { w as f32 * 0.68 } else { w as f32 };
    !(chat && w < 1000) && main / s >= 1000.0
}

#[test]
fn a_typed_secret_goes_to_the_vault_and_only_the_reference_is_saved_and_nothing_logs_it() {
    const TYPED: &str = "typed-by-hand-PW-8842";
    let mut f = fixture("newsecret");
    f.app.go_for_test("sources");
    key(&mut f, Key::Char('a'));
    types(&mut f, "Prometheus");
    key(&mut f, Key::Enter);
    f.app.fill_source_form_for_test(&[("Name", "Edge"), ("Address", "http://edge.internal:9090")]);
    f.app.pick_source_choice_for_test("Sign-in", "bearer");
    assert!(f.app.source_form_labels_for_test().iter().any(|l| l.starts_with("Token / API key")), "{:?}", f.app.source_form_labels_for_test());
    // no field of the form takes a plain-text secret
    assert!(f.app.source_form_kinds_for_test().iter().all(|k| k != "secret"), "{:?}", f.app.source_form_kinds_for_test());
    // choosing "create a new vault entry" opens the small dialog
    f.app.focus_source_field_for_test("Token / API key");
    for _ in 0..3 {
        key(&mut f, Key::Right);
    }
    assert!(f.app.new_secret_open_for_test(), "the create-entry dialog is open");
    key(&mut f, Key::Enter);
    types(&mut f, TYPED);
    key(&mut f, Key::Ctrl('s'));
    assert!(!f.app.new_secret_open_for_test());
    assert_eq!(f.vault.lock().unwrap().last().cloned(), Some(("svc.prometheus".to_string(), TYPED.to_string())), "the value went to the vault");
    // back in the source form with the reference chosen; save it
    key(&mut f, Key::Ctrl('s'));
    let raw = std::fs::read_to_string(f.env.data_dir.join("dash").join(format!("sources-{}.json", f.env.user))).unwrap();
    assert!(raw.contains("uvault:svc.prometheus") && !raw.contains(TYPED), "{raw}");
    let saved = f.env.sources().list().into_iter().find(|s| s.name == "Edge").unwrap();
    assert_eq!(saved.secrets["token"], "uvault:svc.prometheus");
    assert!(saved.values.get("token").is_none() && saved.values.get("auth").map(String::as_str) == Some("bearer"), "{saved:?}");
    // the audit log has the names only
    let audit = f.audit.lock().unwrap().join("\n");
    assert!(audit.contains("dash.vault_entry_created svc.prometheus") && audit.contains("dash.source_saved"), "{audit}");
    assert!(!audit.contains(TYPED) && !audit.contains(SECRET), "{audit}");
    // the typed text is in no field of any dialog that is still open
    assert!(!f.app.any_form_value_contains_for_test(TYPED));
}

#[test]
fn a_locked_vault_is_explained_in_the_create_dialog_and_nothing_is_saved() {
    let mut f = fixture("locked");
    let env2 = {
        let mut e = (*f.env).clone();
        e.vault_set = Arc::new(|_, _| Err("vault is locked".into()));
        Arc::new(e)
    };
    f.app = DashApp::with_env(&f.cx, env2);
    f.app.go_for_test("sources");
    key(&mut f, Key::Char('a'));
    types(&mut f, "Prometheus");
    key(&mut f, Key::Enter);
    f.app.pick_source_choice_for_test("Sign-in", "bearer");
    f.app.focus_source_field_for_test("Token / API key");
    for _ in 0..3 {
        key(&mut f, Key::Right);
    }
    key(&mut f, Key::Enter);
    types(&mut f, "whatever-typed-1");
    key(&mut f, Key::Ctrl('s'));
    assert!(f.app.new_secret_open_for_test(), "the dialog stays open");
    assert!(f.app.new_secret_error_for_test().contains("vault is locked"), "{}", f.app.new_secret_error_for_test());
    assert!(!f.app.any_form_value_contains_for_test("whatever-typed-1"), "the typed text was wiped");
}

#[test]
fn the_audit_log_and_the_assistants_context_never_hold_the_secret() {
    let mut f = board("auditscan");
    f.app.set_chat_for_test(false);
    f.app.go_for_test("sources");
    key(&mut f, Key::Char('t'));
    for _ in 0..100 {
        tick(&mut f, 1);
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let r = f.env.call("dash_query", &serde_json::json!({"source": "prom", "query": "up"}), Some(&f.app.state()));
    let all = format!("{:?}\n{}\n{}", r, f.audit.lock().unwrap().join("\n"), iyakki_agent::dash::tools::DashTools::context(&f.env, &f.app.state()));
    assert!(!all.contains(SECRET), "{all}");
    let seen = f.up.seen.lock().unwrap().join("\n");
    assert!(!seen.contains(SECRET), "never in a URL or a body");
}

// ---------------------------------------------------------------------------------------------------------- unreachable sources and the repair

use iyakki_agent::dash::cluster::{ClusterApi, SvcInfo};

/// A cluster that holds exactly these Services.
struct FixedCluster(Vec<SvcInfo>);
impl ClusterApi for FixedCluster {
    fn service(&self, ns: &str, name: &str) -> Result<SvcInfo, DashError> {
        self.0.iter().find(|s| s.ns == ns && s.name == name).cloned().ok_or_else(|| DashError::config(format!("no Service {name} in {ns}")))
    }
    fn services(&self) -> Result<Vec<SvcInfo>, DashError> {
        Ok(self.0.clone())
    }
    fn host_listens(&self, _: u16) -> bool {
        false
    }
}

use iyakki_agent::dash::connector::DashError;

fn prom_svc() -> SvcInfo {
    SvcInfo { ns: "iyakki-monitoring".into(), name: "prometheus".into(), kind: "ClusterIP".into(), cluster_ip: "10.43.4.4".into(), ports: vec![("http".into(), 9090)], app: "prometheus".into(), ready: Some(true) }
}

fn local_source(f: &Fixture, id: &str, kind: &str, name: &str, url: &str) {
    let mut c = iyakki_agent::dash::connector::SourceConfig { id: id.into(), kind: kind.into(), name: name.into(), ..Default::default() };
    c.values.insert("url".into(), url.into());
    f.env.sources().save(c).unwrap();
}

/// The owner's Mac: Prometheus is in the cluster, Elasticsearch and ClickHouse are not.
fn broken_board(tag: &str, admin: bool) -> Fixture {
    let mut f = fixture_c(tag, Arc::new(FixedCluster(vec![prom_svc()])), admin);
    let mut c = f.env.sources().get("prom").unwrap();
    c.values.insert("url".into(), "http://localhost:9090".into());
    f.env.sources().save(c).unwrap();
    f.app.reload_for_test();
    let mut d = Dashboard::new("", "Node");
    for (i, t) in ["CPU", "Memory", "Disk"].iter().enumerate() {
        d.add_panel(Panel::new("", t, "prom", Visual::Line, "up", Grid::new(0, i as i32 * 8, 12, 8))).unwrap();
    }
    f.app.open_dashboard_for_test(d);
    settle(&mut f);
    f
}

/// Draw until a button with this label shows up (the cluster is asked in the background).
fn wait_for_label(f: &mut Fixture, label: &str) -> Gfx {
    for _ in 0..300 {
        tick(f, 1);
        let g = draw(f, 1280, 720);
        if g.hits.iter().any(|h| h.label == label) {
            return g;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no `{label}` button");
}

#[test]
fn panels_that_fail_the_same_way_get_one_banner_and_the_repair_waits_for_the_click() {
    let mut f = broken_board("banner", true);
    let g = wait_for_label(&mut f, "Use cluster address");
    // one banner for the source, and its button and `Open Sources`
    let banner_buttons = g.hits.iter().filter(|h| h.ev == Event::Tab(id::REPAIR)).count();
    assert_eq!(banner_buttons, 1);
    assert!(g.hits.iter().any(|h| h.label == "Open Sources"));
    // nothing changed by itself
    let before = f.env.sources().get("prom").unwrap();
    assert_eq!(before.value("url"), "http://localhost:9090");
    assert!(!f.audit.lock().unwrap().iter().any(|l| l.contains("source_repaired")));
    // the click rewrites the address, keeps the rest, audits, and the panels load again
    f.app.handle(&Event::Tab(id::REPAIR), &f.cx);
    for _ in 0..300 {
        tick(&mut f, 1);
        draw(&mut f, 1280, 720);
        if f.env.sources().get("prom").unwrap().value("url").starts_with("cluster://") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let after = f.env.sources().get("prom").unwrap();
    assert_eq!(after.value("url"), "cluster://iyakki-monitoring/prometheus:9090");
    let mut expect = before.clone();
    expect.values.insert("url".into(), after.value("url").into());
    assert_eq!(after, expect, "name, sign-in and secrets are kept");
    assert_eq!(f.audit.lock().unwrap().iter().filter(|l| l.starts_with("dash.source_repaired")).count(), 1);
    for _ in 0..200 {
        tick(&mut f, 1);
        if (0..3).all(|i| f.app.has_frame_for_test(&format!("p{}", i + 1))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!((0..3).all(|i| f.app.has_frame_for_test(&format!("p{}", i + 1))), "the panels refreshed through the cluster address");
    let g = draw(&mut f, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Open Sources"), "the banner is gone");
}

#[test]
fn a_person_who_is_not_an_administrator_cannot_repair_and_nothing_changes() {
    let mut f = broken_board("nonadmin", false);
    f.app.handle(&Event::Tab(id::REPAIR), &f.cx);
    f.app.handle(&Event::Tab(id::REPAIR_P), &f.cx);
    for _ in 0..60 {
        tick(&mut f, 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(f.env.sources().get("prom").unwrap().value("url"), "http://localhost:9090");
    assert!(!f.audit.lock().unwrap().iter().any(|l| l.contains("source_repaired")));
    let st = f.app.status_for_test();
    assert!(st.contains("administrator"), "{st}");
}

#[test]
fn the_banner_and_the_short_panel_state_fit_at_the_four_sizes() {
    let mut f = broken_board("sizes", true);
    wait_for_label(&mut f, "Use cluster address");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for chat in [false, true] {
            f.app.set_chat_for_test(chat);
            let g = draw(&mut f, w, h);
            let (a, s) = area(w, h);
            let ctx = format!("{w}x{h} chat={chat}");
            for hit in &g.hits {
                if hit.label == "backdrop" || hit.label == "slot-mark" {
                    continue;
                }
                let r = hit.rect;
                assert!(r.x >= -1.0 && r.y >= a.y - s * 40.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{ctx}: `{}` at {r:?}", hit.label);
            }
            assert!(super::component::route::duplicate_ids(&g.hits).is_empty(), "{ctx}");
            if w == 800 && chat {
                continue; // the chat takes the whole window: the board is not drawn
            }
            let banner = g.hits.iter().find(|h| h.ev == Event::Tab(id::REPAIR)).unwrap_or_else(|| panic!("{ctx}: the banner button is missing"));
            // the board's panels start below the banner
            for (_, r) in f.app.panel_rects_for_test() {
                let overlap = r.x < banner.rect.x + banner.rect.w && banner.rect.x < r.x + r.w && r.y < banner.rect.y + banner.rect.h && banner.rect.y < r.y + r.h;
                assert!(!overlap, "{ctx}: a panel {r:?} is under the banner button {:?}", banner.rect);
            }
        }
    }
}

#[test]
fn the_sources_page_offers_install_remove_fix_all_and_remove_duplicate_for_the_owners_sources() {
    let mut f = broken_board("sources", true);
    local_source(&f, "elasticsearch", "elasticsearch", "Elasticsearch", "http://localhost:9200");
    local_source(&f, "clickhouse", "clickhouse", "ClickHouse", "http://localhost:8123");
    f.app.go_for_test("sources");
    f.app.reload_for_test();
    wait_for_label(&mut f, "Fix all");
    let sel = |f: &mut Fixture, id: &str| {
        let i = f.env.sources().list().iter().position(|s| s.id == id).unwrap();
        f.app.handle(&Event::Row(id::ROW_SRC + i), &f.cx);
    };
    // Elasticsearch is not in the cluster: install it from Setup
    sel(&mut f, "elasticsearch");
    wait_for_label(&mut f, "Install Elasticsearch (Store -> Setup)");
    f.app.handle(&Event::Tab(id::S_REPAIR), &f.cx);
    assert_eq!(f.app.take_request(), Some(("store".into(), "setup:elasticsearch".into())));
    assert_eq!(f.env.sources().get("elasticsearch").unwrap().value("url"), "http://localhost:9200", "nothing changed");
    // ClickHouse has its installer item too now: the same Install, not a bare Remove
    sel(&mut f, "clickhouse");
    wait_for_label(&mut f, "Install ClickHouse (Store -> Setup)");
    f.app.handle(&Event::Tab(id::S_REPAIR), &f.cx);
    assert_eq!(f.app.take_request(), Some(("store".into(), "setup:clickhouse".into())));
    assert!(f.env.sources().get("clickhouse").is_some());
    // Fix all points only the sources whose Service exists at the cluster; the other two keep their own choice
    f.app.handle(&Event::Tab(id::S_FIXALL), &f.cx);
    for _ in 0..200 {
        tick(&mut f, 1);
        draw(&mut f, 1280, 720);
        if f.env.sources().get("prom").unwrap().value("url").starts_with("cluster://") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(f.env.sources().get("prom").unwrap().value("url"), "cluster://iyakki-monitoring/prometheus:9090");
    assert_eq!(f.env.sources().get("elasticsearch").unwrap().value("url"), "http://localhost:9200");
    assert_eq!(f.env.sources().get("clickhouse").unwrap().value("url"), "http://localhost:8123");
    // a second source with the same address (the shared Cluster Prometheus) is offered for removal
    local_source(&f, "prometheus-cluster", "prometheus", "Cluster Prometheus", "cluster://iyakki-monitoring/prometheus:9090");
    f.app.reload_for_test();
    sel(&mut f, "prom");
    let g = draw(&mut f, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Remove duplicate"));
}

/// A board of three panels on a localhost ClickHouse that is not installed anywhere.
fn not_installed_board(tag: &str) -> Fixture {
    let mut f = fixture_c(tag, Arc::new(FixedCluster(vec![prom_svc()])), true);
    local_source(&f, "clickhouse", "clickhouse", "ClickHouse", "http://localhost:8123");
    f.app.reload_for_test();
    let mut d = Dashboard::new("", "Events");
    for (i, t) in ["Rows", "Errors", "Latency"].iter().enumerate() {
        d.add_panel(Panel::new("", t, "clickhouse", Visual::Line, "select 1", Grid::new(0, i as i32 * 8, 12, 8))).unwrap();
    }
    f.app.open_dashboard_for_test(d);
    settle(&mut f);
    f
}

#[test]
fn a_source_for_a_service_that_is_not_installed_says_so_with_install_and_remove_and_the_panels_add_no_buttons() {
    let mut f = not_installed_board("notinst");
    let g = wait_for_label(&mut f, "Install");
    // one banner with its own two buttons plus Open Sources; the title says plainly what is missing
    assert!(g.hits.iter().any(|h| h.label == "Remove this source" && h.ev == Event::Tab(id::REMOVE)));
    assert!(g.hits.iter().any(|h| h.label == "Open Sources"));
    assert_eq!(g.hits.iter().filter(|h| h.label == "Install").count(), 1, "one Install, not one per panel");
    assert!(g.text_log.iter().any(|t| t.2.contains("ClickHouse is not installed")), "the banner names it: {:?}", g.text_log);
    assert!(!g.text_log.iter().any(|t| t.2.contains("Source ClickHouse is not reachable")), "no 'not reachable' wording");
    // the panels carry no repair button of their own (no id in the REPAIR_P range, no second Install)
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if (id::REPAIR_P..id::REPAIR_P + 100).contains(&n))));
    assert!(!g.hits.iter().any(|h| h.label == "Use cluster address"));
    // Install opens Store -> Setup at the ClickHouse item; nothing else changes
    f.app.handle(&Event::Tab(id::REPAIR), &f.cx);
    assert_eq!(f.app.take_request(), Some(("store".into(), "setup:clickhouse".into())));
    assert!(f.env.sources().get("clickhouse").is_some());
    // Remove this source asks first, and the answer removes it
    f.app.handle(&Event::Tab(id::REMOVE), &f.cx);
    assert!(f.app.modal_open_for_test());
    key(&mut f, Key::Esc);
    assert!(f.env.sources().get("clickhouse").is_some(), "cancel keeps it");
}

#[test]
fn the_not_installed_banner_fits_at_the_four_sizes() {
    let mut f = not_installed_board("notinst-sizes");
    wait_for_label(&mut f, "Install");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut f, w, h);
        for hit in g.hits.iter().filter(|h| matches!(h.label.as_str(), "Install" | "Remove this source" | "Open Sources")) {
            let r = hit.rect;
            assert!(r.x >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{w}x{h}: `{}` at {r:?}", hit.label);
        }
        assert!(super::component::route::duplicate_ids(&g.hits).is_empty(), "{w}x{h}");
    }
}

#[test]
fn the_list_shares_the_cluster_dashboard_only_after_a_plain_confirmation_and_shows_with_whom() {
    let mut f = fixture("share");
    f.env.sources().save(iyakki_agent::dash::seed::default_source()).unwrap();
    let t = iyakki_agent::dash::templates::find("k3s-local").unwrap();
    f.env.dashboards().save(&t.instantiate("kube-local", "k3s-local")).unwrap();
    f.app.reload_for_test();
    f.app.select_list_for_test(0);
    // h asks for the names, then for a confirmation; Esc at the confirmation shares nothing
    key(&mut f, Key::Char('h'));
    assert!(f.app.modal_open_for_test());
    types(&mut f, "bob, carol");
    key(&mut f, Key::Enter);
    assert!(f.app.modal_open_for_test(), "the confirmation is showing");
    key(&mut f, Key::Esc);
    assert!(!f.app.modal_open_for_test() && f.env.shared_with("k3s-local").is_empty());
    assert!(f.audit.lock().unwrap().iter().all(|l| !l.starts_with("dash.shared")));
    // again, and confirm
    key(&mut f, Key::Char('h'));
    types(&mut f, "bob:operator, carol");
    key(&mut f, Key::Enter);
    key(&mut f, Key::Char('y'));
    use iyakki_agent::dash::share::Grant;
    assert_eq!(f.env.shared_with("k3s-local"), [Grant::operator("bob"), Grant::viewer("carol")]);
    assert!(f.app.status_for_test().contains("Shared with bob (operator), carol (viewer)"), "{}", f.app.status_for_test());
    assert!(f.audit.lock().unwrap().iter().any(|l| l == "dash.shared k3s-local: bob (operator), carol (viewer)"));
    // the list says with whom
    let g = draw(&mut f, 1280, 720);
    let _ = g;
    assert_eq!(f.app.shared_text_for_test("k3s-local"), "bob (operator), carol (viewer)");
    // an empty list stops sharing, after a confirmation too
    f.app.select_list_for_test(0);
    key(&mut f, Key::Char('h'));
    for _ in 0..30 {
        key(&mut f, Key::Backspace);
    }
    key(&mut f, Key::Enter);
    key(&mut f, Key::Char('y'));
    assert!(f.env.shared_with("k3s-local").is_empty());
    assert!(f.audit.lock().unwrap().iter().any(|l| l == "dash.unshared k3s-local: bob, carol"));
}

#[test]
fn an_account_that_is_not_an_administrator_cannot_share_and_a_viewer_cannot_save_or_export() {
    let mut f = fixture_c("share-user", Arc::new(iyakki_agent::dash::cluster::NoCluster), false);
    let mut d = Dashboard::new("notes", "Notes");
    d.add_panel(Panel::new("t", "T", "", Visual::Text, "", Grid::new(0, 0, 6, 4))).unwrap();
    f.env.dashboards().save(&d).unwrap();
    f.app.reload_for_test();
    f.app.select_list_for_test(0);
    key(&mut f, Key::Char('h'));
    types(&mut f, "bob");
    key(&mut f, Key::Enter);
    key(&mut f, Key::Char('y'));
    assert!(f.app.status_for_test().contains("only an administrator"), "{}", f.app.status_for_test());
    assert!(f.env.shared_with("notes").is_empty());
    // a dashboard open as a viewer cannot be saved or exported
    let mut shared = Dashboard::new("share:amy/k3s-local", "Cluster");
    shared.add_panel(Panel::new("t", "T", "", Visual::Text, "", Grid::new(0, 0, 6, 4))).unwrap();
    f.app.open_dashboard_for_test(shared);
    key(&mut f, Key::Ctrl('s'));
    assert!(f.app.status_for_test().contains("not shared with you"), "{}", f.app.status_for_test());
    assert!(f.env.dashboards().list().iter().all(|e| e.id == "notes"));
}

#[test]
fn the_dialog_refuses_an_unknown_role_and_a_viewer_row_says_what_you_are() {
    let mut f = fixture("share-role");
    f.env.sources().save(iyakki_agent::dash::seed::default_source()).unwrap();
    let t = iyakki_agent::dash::templates::find("k3s-local").unwrap();
    f.env.dashboards().save(&t.instantiate("kube-local", "k3s-local")).unwrap();
    f.app.reload_for_test();
    f.app.select_list_for_test(0);
    key(&mut f, Key::Char('h'));
    types(&mut f, "bob:boss");
    key(&mut f, Key::Enter);
    assert!(f.app.modal_open_for_test(), "the form stays open with the error");
    assert!(f.env.shared_with("k3s-local").is_empty());
    key(&mut f, Key::Esc);
    assert!(!f.app.modal_open_for_test());
}
