//! A `wasm` tile app (F-WASM-09, F-APPS-45): a tile app whose `app.json` names a WebAssembly module (`"wasm": "tile.wasm"`). The module
//! runs in the `iyakki-tile-host` sandbox (fuel, time and memory budgets, no tools, no files) and draws the display list the tile ABI
//! defines (docs/development/TILE-ABI.md); this screen turns clicks and typing into tile events and paints the list with the host's
//! reference renderer. It starts only while Settings -> Developer mode -> WASM tiles (experimental) is on; otherwise it says why.
//!
//! The reference renderer draws rectangles, outlines and a small built-in font: enough for the example tile, not a full UI toolkit.

use iyakki_agent::store::tilehub::{self, TileEntry};
use iyakki_tile_abi::{Color, DisplayList, Event as TileEvent};
use iyakki_tile_host::render::render;
use iyakki_tile_host::TileRunner;
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

use super::{App, AppCx, Flow};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

const ACCENT: [u8; 4] = [190, 170, 255, 255];
const AREA: usize = 7300;
/// The tile's surface in logical pixels is the free area, kept inside these limits.
const MIN_SIDE: i32 = 64;
const MAX_SIDE: i32 = 1024;
const BG: Color = Color(24, 26, 34, 255);

pub struct WasmTileApp {
    entry: TileEntry,
    user: String,
    runner: Option<TileRunner>,
    started: bool,
    err: Option<String>,
    size: (i32, i32),
    picture: Option<Pixmap>,
    list: Option<DisplayList>,
    dirty: bool,
    ticks: u64,
    /// Where the last frame put the tile's surface (tests, and to turn a click into tile coordinates).
    surface: R,
    scale: f32,
}

impl WasmTileApp {
    pub fn new(entry: TileEntry, cx: &AppCx) -> WasmTileApp {
        let mut a = WasmTileApp { user: cx.session.user.clone(), entry, runner: None, started: false, err: None, size: (0, 0), picture: None, list: None, dirty: true, ticks: 0, surface: R::new(0.0, 0.0, 0.0, 0.0), scale: 1.0 };
        a.load(cx);
        a
    }

    fn load(&mut self, cx: &AppCx) {
        self.err = None;
        self.runner = None;
        self.started = false;
        self.picture = None;
        self.list = None;
        self.dirty = true;
        match tilehub::open_wasm(&cx.services, &cx.session.policy, &self.user, &self.entry) {
            Ok(r) => self.runner = Some(r),
            Err(e) => self.err = Some(e),
        }
        cx.audit.record(&self.user, "tile.open", &format!("{} {} (wasm{})", self.entry.name, self.entry.version, if self.err.is_some() { ", refused" } else { "" }));
    }

    /// The tile stopped (budget, crash, bad picture): its words for the person, and no more events reach it.
    fn stop(&mut self, why: impl std::fmt::Display) {
        self.err = Some(why.to_string());
        self.runner = None;
    }

    fn theme() -> TileEvent {
        TileEvent::Theme { dark: true, accent: Color(190, 170, 255, 255), background: BG, foreground: Color(230, 232, 240, 255) }
    }

    /// Send events to the tile; a tile that fails is stopped.
    fn send(&mut self, events: &[TileEvent]) {
        let Some(r) = self.runner.as_mut() else { return };
        match r.event(events) {
            Ok(true) => self.dirty = true,
            Ok(false) => {}
            Err(e) => self.stop(e),
        }
    }

    /// Make sure the tile knows the surface size, then paint the newest frame into a picture.
    fn prepare(&mut self, w: i32, h: i32) {
        let Some(r) = self.runner.as_mut() else { return };
        if !self.started {
            if let Err(e) = r.init(w, h, 100, Self::theme()) {
                return self.stop(e);
            }
            self.started = true;
            self.size = (w, h);
            self.dirty = true;
        } else if self.size != (w, h) {
            self.size = (w, h);
            self.send(&[TileEvent::Resize { w, h, scale_pct: 100 }]);
            self.dirty = true;
        }
        if !self.dirty {
            return;
        }
        let Some(r) = self.runner.as_mut() else { return };
        match r.frame() {
            Ok(list) => {
                let img = render(&list, w, h, BG);
                self.picture = IntSize::from_wh(w as u32, h as u32).and_then(|sz| Pixmap::from_vec(img.rgba, sz));
                self.list = Some(list);
                self.dirty = false;
            }
            Err(e) => self.stop(e),
        }
    }

    /// The words the tile drew (tests).
    pub fn drawn_text(&self) -> Vec<String> {
        self.list.as_ref().map(|l| l.ops.iter().filter_map(|o| if let iyakki_tile_abi::Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()).unwrap_or_default()
    }

    /// Why the tile does not run, if it does not (tests).
    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// The tile's surface as drawn last: screen rectangle and scale (tests).
    pub fn surface(&self) -> (R, f32) {
        (self.surface, self.scale)
    }
}

impl App for WasmTileApp {
    fn title(&self) -> &'static str {
        super::tile::intern(&self.entry.title)
    }

    fn icon(&self) -> Icon {
        Icon::Store
    }

    fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    fn hint(&self) -> String {
        "WASM tile (experimental): click and type in the tile · Ctrl+R start it again · Esc back".into()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Ctrl('r')) => self.load(cx),
            Event::Tick => {
                self.ticks += 1;
                if self.ticks % 3 == 0 && self.started {
                    self.send(&[TileEvent::Tick { dt_ms: 100 }]);
                }
            }
            Event::AreaAt(AREA, x, y) if self.runner.is_some() && self.started => {
                let (lx, ly) = ((*x / self.scale) as i32, (*y / self.scale) as i32);
                match self.runner.as_mut().map(|r| r.click(lx, ly)) {
                    Some(Ok(_)) => self.dirty = true,
                    Some(Err(e)) => self.stop(e),
                    None => {}
                }
            }
            Event::Key(Key::Char(c)) if self.started => self.send(&[TileEvent::TextInput { text: c.to_string() }]),
            Event::Key(Key::Backspace) if self.started => self.send(&[TileEvent::Key { code: 8, mods: 0, down: true }]),
            Event::Key(Key::Enter) if self.started => self.send(&[TileEvent::Key { code: 13, mods: 0, down: true }]),
            Event::Paste(t) if self.started => self.send(&[TileEvent::TextInput { text: t.chars().take(256).collect() }]),
            _ => {}
        }
        Flow::Stay
    }

    fn wants_redraw(&mut self) -> bool {
        self.dirty
    }

    #[cfg(test)]
    fn wasm_tile_text(&mut self) -> String {
        match &self.err {
            Some(e) => e.clone(),
            None => self.drawn_text().join("\n"),
        }
    }

    fn render(&mut self, g: &mut Gfx, a: R, s: f32, _cx: &AppCx, _tick: u64) {
        let pad = 12.0 * s;
        let size = 13.0 * s;
        let head = format!("WASM tile (experimental) · {}", self.entry.detail_line());
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
        self.prepare(w, h);
        self.scale = s;
        self.surface = R::new(area.x, area.y, w as f32 * s, h as f32 * s);
        if let Some(pm) = &self.picture {
            let paint = PixmapPaint { quality: FilterQuality::Nearest, ..PixmapPaint::default() };
            g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(area.x, area.y).pre_scale(s, s), None);
            g.hit(self.surface, Event::Area(AREA), "WASM tile");
        } else if self.err.is_none() {
            g.text(area.x, area.y, 15.0 * s, TEXT, "Starting the tile...", Face::Sans);
        }
        if let Some(why) = self.err.clone() {
            g.text(area.x, area.y + 8.0 * s, 15.0 * s, ERR, &why, Face::Sans);
        }
    }
}
