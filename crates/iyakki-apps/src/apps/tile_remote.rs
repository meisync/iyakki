//! A tenant's native tile (F-CLOUD-76, stage 2 of docs/development/CLOUD-MULTITENANT-PLAN.md): the tile runs on the tenant's side; this
//! screen asks the tenant's web service for its display list, draws it with the LOCAL renderer (not a scaled picture of the tenant's
//! screen) and turns clicks and typing into the events the service takes. Where the frames come from is [`RemoteFrames`] (the shell
//! gives it the signed-in tenant connection; tests give a fake), so this file never opens a connection or sees a password.
//!
//! The network never blocks the screen: one request is in flight on a worker thread; events typed meanwhile wait and go in the next
//! request (at most 128 in one). A tenant that fails or answers nonsense stops the tile with the reason in words; Ctrl+R starts it again.

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use iyakki_tile_abi::{Color, DisplayList};
use iyakki_tile_host::export::from_json;
use iyakki_tile_host::render::render_with;
use iyakki_tile_host::ImageTable;
use serde_json::{json, Value};
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

use super::{App, AppCx, Flow};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

const ACCENT: [u8; 4] = [120, 200, 255, 255];
const AREA: usize = 7400;
const MIN_SIDE: i32 = 64;
const MAX_SIDE: i32 = 1024;
const BG: Color = Color(24, 26, 34, 255);
/// The service takes at most this many events in one request.
pub const MAX_EVENTS: usize = 128;

/// Where a tenant's tile frames come from.
pub trait RemoteFrames: Send + Sync {
    /// Send `events` (the service's JSON events) to tile `id` and return `(redraw, frame)`: the display list as JSON.
    fn frame(&self, id: &str, w: i32, h: i32, reset: bool, events: &[Value]) -> Result<(bool, Value), String>;
}

struct Req {
    w: i32,
    h: i32,
    reset: bool,
    events: Vec<Value>,
}

type Resp = Result<(bool, DisplayList, i32, i32), String>;

pub struct RemoteTileApp {
    title: &'static str,
    tenant: String,
    tx: Sender<Req>,
    rx: Receiver<Resp>,
    busy: bool,
    first: bool,
    queued: Vec<Value>,
    size: (i32, i32),
    list: Option<DisplayList>,
    picture: Option<Pixmap>,
    err: Option<String>,
    dirty: bool,
    ticks: u64,
    wants_tick: bool,
    src: Arc<dyn RemoteFrames>,
    id: String,
    surface: R,
    scale: f32,
}

impl RemoteTileApp {
    /// `tenant` is the tenant's name (shown in the header), `id` the tile on the tenant's side, `ticks` whether it wants the clock.
    pub fn new(tenant: &str, id: &str, title: &str, ticks: bool, src: Arc<dyn RemoteFrames>) -> RemoteTileApp {
        let (tx, rx, ) = Self::spawn(id, src.clone());
        RemoteTileApp { title: super::tile::intern(title), tenant: tenant.into(), tx, rx, busy: false, first: true, queued: Vec::new(), size: (0, 0), list: None, picture: None, err: None, dirty: true, ticks: 0, wants_tick: ticks, src, id: id.into(), surface: R::new(0.0, 0.0, 0.0, 0.0), scale: 1.0 }
    }

    fn spawn(id: &str, src: Arc<dyn RemoteFrames>) -> (Sender<Req>, Receiver<Resp>) {
        let (tx, rx_req) = channel::<Req>();
        let (tx_resp, rx) = channel::<Resp>();
        let id = id.to_string();
        std::thread::spawn(move || {
            // ends when the app is dropped (the request channel closes)
            while let Ok(r) = rx_req.recv() {
                let out = src.frame(&id, r.w, r.h, r.reset, &r.events).and_then(|(redraw, v)| from_json(&v).map(|(l, w, h)| (redraw, l, w, h)));
                if tx_resp.send(out).is_err() {
                    break;
                }
            }
        });
        (tx, rx)
    }

    fn restart(&mut self) {
        let (tx, rx) = Self::spawn(&self.id, self.src.clone());
        self.tx = tx;
        self.rx = rx;
        self.busy = false;
        self.first = true;
        self.queued.clear();
        self.err = None;
        self.list = None;
        self.picture = None;
        self.size = (0, 0);
        self.dirty = true;
    }

    fn push(&mut self, e: Value) {
        if self.err.is_none() {
            self.queued.push(e);
        }
    }

    /// Take an answer when one is there; start the next request when none is in flight and there is something to say.
    fn pump(&mut self) {
        if self.busy {
            match self.rx.try_recv() {
                Ok(r) => self.apply(r),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.busy = false;
                    self.err = Some("the connection to the tenant's tiles ended".into());
                }
            }
        }
        self.send_next();
    }

    fn apply(&mut self, r: Resp) {
        self.busy = false;
        match r {
            Ok((redraw, list, ..)) => {
                if redraw || self.list.is_none() {
                    let (w, h) = self.size;
                    let img = render_with(&list, w, h, BG, &ImageTable::new());
                    self.picture = IntSize::from_wh(w as u32, h as u32).and_then(|sz| Pixmap::from_vec(img.rgba, sz));
                    self.list = Some(list);
                    self.dirty = true;
                }
            }
            Err(e) => {
                self.err = Some(e);
                self.dirty = true;
            }
        }
    }

    fn send_next(&mut self) {
        if self.busy || self.err.is_some() || self.size == (0, 0) || (!self.first && self.queued.is_empty()) {
            return;
        }
        let n = self.queued.len().min(MAX_EVENTS);
        let events: Vec<Value> = self.queued.drain(..n).collect();
        let req = Req { w: self.size.0, h: self.size.1, reset: self.first, events };
        self.first = false;
        if self.tx.send(req).is_ok() {
            self.busy = true;
        } else {
            self.err = Some("the connection to the tenant's tiles ended".into());
        }
    }

    /// The words the tile drew (tests).
    pub fn drawn_text(&self) -> Vec<String> {
        self.list.as_ref().map(|l| l.ops.iter().filter_map(|o| if let iyakki_tile_abi::Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()).unwrap_or_default()
    }

    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// Block until the request in flight has been answered (tests: no timing, the worker thread answers or the test fails by hanging
    /// on a fake that never does).
    #[cfg(test)]
    pub(super) fn settle(&mut self) {
        loop {
            self.pump();
            if !self.busy {
                // one more round when events came in while it was busy
                if self.queued.is_empty() || self.err.is_some() {
                    return;
                }
                continue;
            }
            match self.rx.recv() {
                Ok(r) => self.apply(r),
                Err(_) => {
                    self.busy = false;
                    return;
                }
            }
        }
    }
}

impl App for RemoteTileApp {
    fn title(&self) -> &'static str {
        self.title
    }
    fn icon(&self) -> Icon {
        Icon::Mesh
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "A tile of your cloud tenant: click and type in it · Ctrl+R start it again · Esc back".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Ctrl('r')) => self.restart(),
            Event::Tick => {
                self.ticks += 1;
                if self.wants_tick && self.ticks % 3 == 0 {
                    self.push(json!({"type": "tick", "dt_ms": 100}));
                }
                self.pump();
            }
            Event::AreaAt(AREA, x, y) => {
                let (lx, ly) = ((*x / self.scale) as i32, (*y / self.scale) as i32);
                self.push(json!({"type": "pointer_down", "x": lx, "y": ly, "button": 0}));
                self.push(json!({"type": "pointer_up", "x": lx, "y": ly, "button": 0}));
            }
            Event::Key(Key::Char(c)) => self.push(json!({"type": "text", "text": c.to_string()})),
            Event::Key(Key::Backspace) => {
                self.push(json!({"type": "key", "code": 8, "mods": 0, "down": true}));
                self.push(json!({"type": "key", "code": 8, "mods": 0, "down": false}));
            }
            Event::Key(Key::Enter) => {
                self.push(json!({"type": "key", "code": 13, "mods": 0, "down": true}));
                self.push(json!({"type": "key", "code": 13, "mods": 0, "down": false}));
            }
            Event::Paste(t) => self.push(json!({"type": "text", "text": t.chars().take(256).collect::<String>()})),
            _ => {}
        }
        Flow::Stay
    }
    fn wants_redraw(&mut self) -> bool {
        self.pump();
        std::mem::take(&mut self.dirty) || self.busy
    }
    fn render(&mut self, g: &mut Gfx, a: R, s: f32, _cx: &AppCx, _tick: u64) {
        let pad = 12.0 * s;
        let size = 13.0 * s;
        let head = format!("Cloud tile · {}", self.tenant);
        let head = g.fit(size, &head, Face::Sans, a.w - 2.0 * pad);
        g.text(a.x + pad, a.y + 6.0 * s, size, MUTED, &head, Face::Sans);
        let top = a.y + 8.0 * s + g.line_height(size, Face::Sans);
        let area = R::new(a.x + pad, top, a.w - 2.0 * pad, (a.y + a.h - top - pad).max(0.0));
        if let Some(why) = self.err.clone() {
            let mut y = area.y + 8.0 * s;
            for line in g.wrap(15.0 * s, &why, Face::Sans, area.w) {
                g.text(area.x, y, 15.0 * s, ERR, &line, Face::Sans);
                y += g.line_height(15.0 * s, Face::Sans);
            }
            g.text(area.x, y + 8.0 * s, 13.0 * s, DIM, "Ctrl+R starts it again.", Face::Sans);
            return;
        }
        let (w, h) = (((area.w / s) as i32).clamp(MIN_SIDE, MAX_SIDE), ((area.h / s) as i32).clamp(MIN_SIDE, MAX_SIDE));
        if self.size != (w, h) {
            if self.size != (0, 0) {
                self.push(json!({"type": "resize", "w": w, "h": h, "scale_pct": 100}));
            }
            self.size = (w, h);
        }
        self.pump();
        self.scale = s;
        self.surface = R::new(area.x, area.y, w as f32 * s, h as f32 * s);
        if let Some(pm) = &self.picture {
            let paint = PixmapPaint { quality: FilterQuality::Nearest, ..PixmapPaint::default() };
            g.pm_mut().draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(area.x, area.y).pre_scale(s, s), None);
            g.hit(self.surface, Event::Area(AREA), "Cloud tile");
        } else {
            g.text(area.x, area.y, 15.0 * s, TEXT, "Asking the tenant for the tile...", Face::Sans);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use std::sync::Mutex;

    /// A tenant that draws the number of clicks it was sent and records every request.
    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<(String, i32, i32, bool, Vec<Value>)>>,
        clicks: Mutex<u32>,
        fail: Mutex<Option<String>>,
    }
    impl RemoteFrames for Fake {
        fn frame(&self, id: &str, w: i32, h: i32, reset: bool, events: &[Value]) -> Result<(bool, Value), String> {
            self.log.lock().unwrap().push((id.into(), w, h, reset, events.to_vec()));
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            let mut c = self.clicks.lock().unwrap();
            *c += events.iter().filter(|e| e["type"] == "pointer_up").count() as u32;
            Ok((true, json!({"abi": 1, "width": w, "height": h, "ops": [
                {"op": "rect", "x": 0, "y": 0, "w": w, "h": h, "color": "#101820ff"},
                {"op": "text", "x": 4, "y": 4, "face": 0, "size": 14, "width": 50, "height": 16, "color": "#ffffffff", "text": format!("clicks {}", *c)},
                {"op": "hit", "id": 1, "x": 0, "y": 0, "w": w, "h": h}
            ]})))
        }
    }

    fn draw(a: &mut RemoteTileApp, cx: &AppCx) -> Gfx {
        let mut g = Gfx::new(800, 600);
        a.render(&mut g, R::new(0.0, 0.0, 800.0, 600.0), 1.0, cx, 0);
        g
    }

    #[test]
    fn the_tile_asks_the_tenant_draws_its_list_locally_and_sends_clicks_and_typing_as_events() {
        let cx = cx_with_admin_store("remote-tile");
        let fake = Arc::new(Fake::default());
        let mut a = RemoteTileApp::new("acme", "counter", "Counter", false, fake.clone());
        draw(&mut a, &cx);
        a.settle();
        assert_eq!(a.drawn_text(), ["clicks 0"]);
        {
            let log = fake.log.lock().unwrap();
            assert_eq!((log[0].0.as_str(), log[0].3, log[0].4.len()), ("counter", true, 0), "the first request starts the tile and carries no events");
            assert!(log[0].1 >= MIN_SIDE && log[0].2 >= MIN_SIDE);
        }
        let g = draw(&mut a, &cx);
        assert!(g.hits.iter().any(|h| h.label == "Cloud tile"));
        let (surface, scale) = (a.surface, a.scale);
        a.handle(&Event::AreaAt(AREA, 10.0 * scale, 12.0 * scale), &cx);
        a.handle(&Event::Key(Key::Char('x')), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        a.settle();
        assert_eq!(a.drawn_text(), ["clicks 1"]);
        let log = fake.log.lock().unwrap();
        let sent: Vec<Value> = log[1..].iter().flat_map(|l| l.4.clone()).collect();
        let kinds: Vec<&str> = sent.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["pointer_down", "pointer_up", "text", "key", "key"]);
        assert_eq!((sent[0]["x"].as_i64(), sent[0]["y"].as_i64()), (Some(10), Some(12)));
        assert!(surface.w > 0.0);
    }

    #[test]
    fn at_most_128_events_go_in_one_request_and_a_tick_goes_only_to_a_tile_that_wants_it() {
        let cx = cx_with_admin_store("remote-tile2");
        let fake = Arc::new(Fake::default());
        let mut a = RemoteTileApp::new("acme", "t", "T", true, fake.clone());
        draw(&mut a, &cx);
        a.settle();
        for _ in 0..200 {
            a.push(json!({"type": "text", "text": "a"}));
        }
        a.settle();
        assert!(fake.log.lock().unwrap().iter().all(|l| l.4.len() <= MAX_EVENTS));
        assert_eq!(fake.log.lock().unwrap().iter().map(|l| l.4.len()).sum::<usize>(), 200);
        let before = fake.log.lock().unwrap().len();
        for _ in 0..3 {
            a.handle(&Event::Tick, &cx);
        }
        a.settle();
        assert!(fake.log.lock().unwrap()[before..].iter().any(|l| l.4.iter().any(|e| e["type"] == "tick")));
        let quiet = Arc::new(Fake::default());
        let mut b = RemoteTileApp::new("acme", "t", "T", false, quiet.clone());
        draw(&mut b, &cx);
        b.settle();
        for _ in 0..9 {
            b.handle(&Event::Tick, &cx);
        }
        b.settle();
        assert!(quiet.log.lock().unwrap().iter().all(|l| l.4.is_empty()), "no tick for a tile that did not ask");
    }

    #[test]
    fn a_failing_tenant_or_a_nonsense_answer_stops_the_tile_in_words_and_ctrl_r_starts_it_again() {
        let cx = cx_with_admin_store("remote-tile3");
        let fake = Arc::new(Fake::default());
        *fake.fail.lock().unwrap() = Some("the tenant signed you out: sign in again".into());
        let mut a = RemoteTileApp::new("acme", "t", "T", false, fake.clone());
        draw(&mut a, &cx);
        a.settle();
        assert_eq!(a.error(), Some("the tenant signed you out: sign in again"));
        let g = draw(&mut a, &cx);
        assert!(g.hits.iter().all(|h| h.label != "Cloud tile"));
        *fake.fail.lock().unwrap() = None;
        a.handle(&Event::Key(Key::Ctrl('r')), &cx);
        assert!(a.error().is_none());
        draw(&mut a, &cx);
        a.settle();
        assert_eq!(a.drawn_text(), ["clicks 0"]);
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        // a frame with an operation this version does not know is refused as a whole
        struct Odd;
        impl RemoteFrames for Odd {
            fn frame(&self, _: &str, w: i32, h: i32, _: bool, _: &[Value]) -> Result<(bool, Value), String> {
                Ok((true, json!({"width": w, "height": h, "ops": [{"op": "script"}]})))
            }
        }
        let mut b = RemoteTileApp::new("acme", "t", "T", false, Arc::new(Odd));
        draw(&mut b, &cx);
        b.settle();
        assert!(b.error().unwrap().contains("does not know"), "{:?}", b.error());
    }
}
