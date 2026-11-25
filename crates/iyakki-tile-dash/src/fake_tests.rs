//! The DASH tile against fakes: a scripted data server (`iyakki_dash::connector::fake`), a fake assistant pane and a fake port for the person's files.
//! No shell, no account, no chat app, no sleep: the workers are waited for with `settle_for_test` (a blocking read of what they send). The same
//! screens are also proved with the real chat app and the shell's context in `crates/iyakki-apps/src/apps/dash_tests.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_dash::connector::fake::FakeUp;
use iyakki_dash::connector::SourceConfig;
use iyakki_dash::model::*;
use iyakki_dash::tools::Env;
use iyakki_uikit::ctxmenu::Item;
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use crate::app::*;
use crate::port::{ChatPane, DashPort};

const NOW: i64 = 1_792_069_200_000;
/// Click ids of the fake pane's slot.
const SLOT_BASE: usize = 600_000;

/// What the fake pane saw and what it says.
#[derive(Default)]
struct ChatLog {
    events: Vec<String>,
    ticks: usize,
    typed: String,
    running: bool,
    approval: bool,
    request: Option<(String, String)>,
    back_on_esc: bool,
}

struct FakeChat(Arc<Mutex<ChatLog>>);

impl ChatPane for FakeChat {
    fn handle(&mut self, ev: &Event, _cx: &dyn DashPort) -> Flow {
        let mut l = self.0.lock().unwrap();
        match ev {
            Event::Tick => l.ticks += 1,
            Event::Key(Key::Char(c)) => l.typed.push(*c),
            Event::Key(Key::Esc) if l.back_on_esc => return Flow::Back,
            other => l.events.push(format!("{other:?}")),
        }
        Flow::Stay
    }
    fn decode_slot(&self, ev: &Event) -> Option<Event> {
        match ev {
            Event::Tab(n) if *n >= SLOT_BASE => Some(Event::Tab(n - SLOT_BASE)),
            Event::Sub(10, k) => Some(Event::Key(k.clone())),
            _ => None,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &dyn DashPort, _tick: u64) {
        g.hit(R::new(area.x, area.y, area.w, 20.0 * s), Event::Tab(SLOT_BASE + 1), "fake chat button");
    }
    fn wants_redraw(&mut self) -> bool {
        false
    }
    fn is_running(&self) -> bool {
        self.0.lock().unwrap().running
    }
    fn pending_approval(&self) -> bool {
        self.0.lock().unwrap().approval
    }
    fn prefill(&mut self, text: &str) {
        self.0.lock().unwrap().typed = text.to_string();
    }
    fn input_text(&self) -> String {
        self.0.lock().unwrap().typed.clone()
    }
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<Item> {
        vec![Item::new("link.open", "Open link", "")]
    }
    fn context_pick(&mut self, id: &str, _cx: &dyn DashPort) -> Flow {
        self.0.lock().unwrap().events.push(format!("pick {id}"));
        Flow::Stay
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.lock().unwrap().request.take()
    }
}

/// The person's files, in memory.
#[derive(Default)]
struct FakeFiles {
    reads: Mutex<Vec<String>>,
    writes: Mutex<Vec<(String, usize)>>,
    file: Mutex<Option<Result<String, String>>>,
    offer_host_save: Mutex<bool>,
    refuse_write: Mutex<Option<String>>,
}

struct FakePort(Arc<FakeFiles>);

impl DashPort for FakePort {
    fn shell(&self) -> &dyn std::any::Any {
        &()
    }
    fn read_dashboard_file(&self, _env: &Env, path: &str, _max: u64) -> Result<String, String> {
        self.0.reads.lock().unwrap().push(path.to_string());
        self.0.file.lock().unwrap().clone().unwrap_or_else(|| Err("you may not read that file".into()))
    }
    fn write_export(&self, _env: &Env, name: &str, bytes: &[u8]) -> Result<bool, String> {
        if let Some(e) = self.0.refuse_write.lock().unwrap().clone() {
            return Err(e);
        }
        self.0.writes.lock().unwrap().push((name.to_string(), bytes.len()));
        Ok(*self.0.offer_host_save.lock().unwrap())
    }
}

/// A folder of its own that goes away with the test.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("iyakki-tile-dash-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    app: DashApp,
    port: FakePort,
    files: Arc<FakeFiles>,
    chat: Arc<Mutex<ChatLog>>,
    up: Arc<FakeUp>,
    _dir: Dir,
    g: Gfx,
    tick: u64,
}

fn matrix(n: usize) -> String {
    let series: Vec<String> = (0..n)
        .map(|k| {
            let pts: Vec<String> = (0..60).map(|i| format!("[{},\"{}\"]", 1_792_065_600 + i * 60, 40.0 + 20.0 * ((i as f64) / 9.0 + k as f64).sin())).collect();
            format!(r#"{{"metric":{{"instance":"node-{k}:9100"}},"values":[{}]}}"#, pts.join(","))
        })
        .collect();
    format!(r#"{{"status":"success","data":{{"resultType":"matrix","result":[{}]}}}}"#, series.join(","))
}

fn fx() -> Fx {
    let dir = Dir::new();
    let up = FakeUp::new();
    up.on("query_range", 200, &matrix(2)).on("/api/v1/query?", 200, r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"instance":"a"},"value":[1792069200,"42.5"]}]}}"#);
    let (env, _audit) = Env::for_test("amy", &dir.0, &up, &[("svc.prom", "x")], NOW);
    let mut c = SourceConfig { id: "prom".into(), kind: "prometheus".into(), name: "Prod".into(), ..Default::default() };
    c.values.insert("url".into(), "http://prom.internal:9090".into());
    env.sources().save(c).unwrap();
    let chat = Arc::new(Mutex::new(ChatLog::default()));
    let c2 = chat.clone();
    let app = DashApp::new(Arc::new(env), move |_state| Box::new(FakeChat(c2)));
    let files = Arc::new(FakeFiles::default());
    Fx { app, port: FakePort(files.clone()), files, chat, up, _dir: dir, g: Gfx::new(1280, 720), tick: 0 }
}

impl Fx {
    fn draw_at(&mut self, w: u32, h: u32) {
        self.g = Gfx::new(w, h);
        self.g.clear(rgb(8, 10, 22));
        let s = iyakki_uikit::ui::scale_for(w, h);
        let hh = 46.0 * s;
        self.tick += 1;
        self.app.render(&mut self.g, R::new(0.0, hh, w as f32, h as f32 - hh), s, &self.port, self.tick);
    }
    fn draw(&mut self) {
        self.draw_at(1280, 720)
    }
    fn ev(&mut self, e: Event) -> Flow {
        let f = self.app.handle(&e, &self.port);
        self.draw();
        f
    }
    fn key(&mut self, k: Key) -> Flow {
        self.ev(Event::Key(k))
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
}

fn board_of(titles: &[(&str, Visual, &str)]) -> Dashboard {
    let mut d = Dashboard::new("", "Checkout");
    for (t, v, q) in titles {
        d.add_panel(Panel::new("", t, "prom", *v, q, Grid::new(0, 0, 12, 8))).unwrap();
    }
    d
}

fn loaded() -> Fx {
    let mut f = fx();
    f.app.open_dashboard_for_test(board_of(&[("CPU by node", Visual::Line, "rate(cpu[5m])"), ("Load", Visual::Stat, "node_load1")]));
    f.app.settle_for_test();
    f.draw();
    f
}

#[test]
fn the_panels_ask_the_scripted_server_and_draw_what_comes_back() {
    let f = loaded();
    assert!(f.app.has_frame_for_test("p1") && f.app.has_frame_for_test("p2"));
    assert!(f.text().contains("CPU by node") && f.text().contains("Load"), "{}", f.text());
    let urls = f.up.urls();
    assert!(urls.iter().any(|u| u.contains("query_range")), "{urls:?}");
}

#[test]
fn ask_ai_opens_the_pane_focuses_it_and_prefills_the_question_about_the_panel() {
    let mut f = loaded();
    f.app.set_chat_for_test(false);
    f.key(Key::Down);
    f.key(Key::Char('i'));
    assert!(f.app.chat_open_for_test() && f.app.chat_focused_for_test());
    let t = f.app.chat_input_for_test();
    assert!(t.contains("Explain panel p1") && t.contains("CPU by node"), "{t}");
    assert_eq!(f.chat.lock().unwrap().typed, t);
}

#[test]
fn keys_go_only_to_the_pane_that_owns_the_keyboard() {
    let mut f = loaded();
    f.app.set_chat_for_test(true);
    for c in "hello".chars() {
        f.key(Key::Char(c));
    }
    assert_eq!(f.app.chat_input_for_test(), "hello");
    assert!(!f.app.in_editor_for_test(), "no shortcut of the board fired");
    f.key(Key::F(6));
    assert!(!f.app.chat_focused_for_test());
    f.key(Key::Char('a'));
    assert_eq!(f.app.chat_input_for_test(), "hello");
    assert!(f.app.in_editor_for_test());
}

#[test]
fn a_click_in_the_pane_slot_is_decoded_by_the_pane_and_takes_the_focus() {
    let mut f = loaded();
    f.app.set_chat_for_test(true);
    f.key(Key::F(6));
    assert!(!f.app.chat_focused_for_test());
    f.ev(Event::Tab(SLOT_BASE + 7));
    assert!(f.app.chat_focused_for_test());
    assert!(f.chat.lock().unwrap().events.contains(&"Tab(7)".to_string()), "{:?}", f.chat.lock().unwrap().events);
}

#[test]
fn a_pane_that_asks_to_leave_hands_the_keyboard_back_to_the_board_and_the_tile_stays() {
    let mut f = loaded();
    f.app.set_chat_for_test(true);
    f.chat.lock().unwrap().back_on_esc = true;
    f.key(Key::Esc);
    assert!(!f.app.chat_focused_for_test());
}

#[test]
fn every_tick_reaches_the_pane_and_a_running_assistant_keeps_the_tile_redrawing() {
    let mut f = loaded();
    f.app.set_chat_for_test(true);
    let before = f.chat.lock().unwrap().ticks;
    f.ev(Event::Tick);
    assert_eq!(f.chat.lock().unwrap().ticks, before + 1);
    f.chat.lock().unwrap().running = true;
    for _ in 0..3 {
        f.ev(Event::Tick);
    }
    assert!(f.app.wants_redraw());
}

#[test]
fn a_link_the_pane_wants_opened_is_handed_to_the_shell_once() {
    let mut f = loaded();
    f.chat.lock().unwrap().request = Some(("browser".into(), "https://example.org".into()));
    assert_eq!(f.app.take_request(), Some(("browser".to_string(), "https://example.org".to_string())));
    assert_eq!(f.app.take_request(), None);
}

#[test]
fn the_panel_menu_is_the_tiles_and_the_pane_menu_is_the_panes() {
    let mut f = loaded();
    f.app.set_chat_for_test(true);
    f.draw();
    let c = f.app.chat_rect_for_test().expect("the pane is open");
    let items = f.app.context_items(c.x as i32 + 5, c.y as i32 + 5);
    assert_eq!(items.len(), 1);
    f.app.set_chat_for_test(false);
    f.draw();
    let (idx, r) = f.app.panel_rects_for_test().first().cloned().unwrap();
    let items = f.app.context_items(r.x as i32 + 4, r.y as i32 + 4);
    assert!(items.iter().any(|i| i.id == "dash.copy") && items.iter().any(|i| i.id == "dash.ask"), "panel {idx}");
    assert_eq!(f.app.context_pick("link.open", &f.port), Flow::Stay);
}

#[test]
fn exporting_a_panel_goes_through_the_port_and_offers_the_host_save_only_when_the_port_says_so() {
    let mut f = loaded();
    f.key(Key::Down);
    f.app.context_pick("dash.export", &f.port);
    f.draw();
    let w = f.files.writes.lock().unwrap().clone();
    assert_eq!(w.len(), 1);
    assert!(w[0].0.starts_with("cpu-by-node-") && w[0].0.ends_with(".csv") && w[0].1 > 10, "{w:?}");
    assert!(f.app.take_host_save().is_none());
    *f.files.offer_host_save.lock().unwrap() = true;
    f.app.context_pick("dash.export", &f.port);
    let (name, bytes) = f.app.take_host_save().expect("offered");
    assert!(name.ends_with(".csv") && !bytes.is_empty());
}

#[test]
fn a_refused_export_says_why() {
    let mut f = loaded();
    f.key(Key::Down);
    *f.files.refuse_write.lock().unwrap() = Some("the folder is full".into());
    f.app.context_pick("dash.export", &f.port);
    f.draw();
    assert!(f.text().contains("Could not write the file: the folder is full"), "{}", f.text());
    assert!(f.files.writes.lock().unwrap().is_empty());
}

#[test]
fn importing_reads_the_file_through_the_port_and_a_refusal_is_shown_in_the_forms_words() {
    let mut f = fx();
    f.app.go_for_test("list");
    f.draw();
    f.key(Key::Char('i'));
    for c in "~/Dashboards/a.json".chars() {
        f.key(Key::Char(c));
    }
    f.key(Key::Enter);
    assert_eq!(f.files.reads.lock().unwrap().clone(), ["~/Dashboards/a.json"]);
    assert!(f.text().contains("you may not read that file"), "{}", f.text());
}

#[test]
fn a_file_that_is_not_a_dashboard_is_refused_by_the_engine_and_nothing_is_opened() {
    let mut f = fx();
    *f.files.file.lock().unwrap() = Some(Ok("this is not json".into()));
    f.app.go_for_test("list");
    f.draw();
    f.key(Key::Char('i'));
    for c in "x.json".chars() {
        f.key(Key::Char(c));
    }
    f.key(Key::Enter);
    assert_eq!(f.files.reads.lock().unwrap().len(), 1);
    assert!(!f.app.on_screen_for_test("board"), "no dashboard was opened");
}

#[test]
fn every_screen_fits_the_four_sizes_with_the_pane_open_and_closed() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1366, 768), (2560, 1600)] {
        for chat in [false, true] {
            let mut f = loaded();
            for screen in ["board", "list", "sources", "board"] {
                f.app.go_for_test(screen);
                f.app.set_chat_for_test(chat);
                f.draw_at(w, h);
                for hit in &f.g.hits {
                    let r = hit.rect;
                    assert!(r.x + r.w >= -1.0 && r.x <= w as f32 + 1.0 && r.y + r.h >= -1.0 && r.y <= h as f32 + 1.0, "{screen} {w}x{h} chat={chat}: `{}` {r:?} is outside the window", hit.label);
                }
            }
        }
    }
}
