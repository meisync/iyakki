//! A `wasm` tile app (F-WASM-09, F-APPS-45): a tile app whose `app.json` names a WebAssembly module (`"wasm": "tile.wasm"`). The module
//! runs in the `iyakki-tile-host` sandbox (fuel, time and memory budgets, no tools, no files) and draws the display list the tile ABI
//! defines (docs/development/TILE-ABI.md); this screen turns clicks and typing into tile events and paints the list with the host's
//! reference renderer. It starts only while Settings -> Developer mode -> WASM tiles (experimental) is on; otherwise it says why.
//!
//! The reference renderer draws rectangles, outlines and a small built-in font: enough for the example tile, not a full UI toolkit.

use iyakki_shared::store_tilehub::TileEntry;
use iyakki_tile_abi::{Color, DisplayList, Event as TileEvent};
use iyakki_tile_host::render::render_with;
use iyakki_tile_host::ImageTable;
use iyakki_agent_api::WasmTileRt;
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

use super::{App, AppCx, Flow};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

const ACCENT: [u8; 4] = [190, 170, 255, 255];
const AREA: usize = 7300;
/// The button that shows the screen reader review.
const REVIEW_TAB: usize = 7299;
/// Access nodes are areas `AREA + 1 ..= AREA + ACCESS_MAX`.
const ACCESS_MAX: usize = 256;
/// The tile's surface in logical pixels is the free area, kept inside these limits.
const MIN_SIDE: i32 = 64;
const MAX_SIDE: i32 = 1024;
const BG: Color = Color(24, 26, 34, 255);

pub struct WasmTileApp {
    entry: TileEntry,
    user: String,
    runner: Option<Box<dyn WasmTileRt>>,
    started: bool,
    err: Option<String>,
    size: (i32, i32),
    picture: Option<Pixmap>,
    /// The pictures the tile's bundle carries, by handle (F-WASM-16), drawn by the same renderer path as the web page's.
    images: ImageTable,
    list: Option<DisplayList>,
    dirty: bool,
    ticks: u64,
    /// Where the last frame put the tile's surface (tests, and to turn a click into tile coordinates).
    surface: R,
    scale: f32,
    /// The first-use card of the SDK bridge that waits for the person (the request id and the card): the tile asks again after the answer.
    card: Option<(u32, super::card::Card)>,
    /// The screen reader review of the tile's picture is shown instead of the tile (Ctrl+B or its button).
    review: bool,
    /// The app the tile asked the dock to open (`dock.open`): the shell takes it once with `take_jump` (F-SDK-34, D-SDK-34-03).
    jump: Option<(String, String)>,
}

impl WasmTileApp {
    pub fn new(entry: TileEntry, cx: &AppCx) -> WasmTileApp {
        let mut a = WasmTileApp { user: cx.session.user.clone(), entry, runner: None, started: false, err: None, size: (0, 0), picture: None, images: ImageTable::new(), list: None, dirty: true, ticks: 0, surface: R::new(0.0, 0.0, 0.0, 0.0), scale: 1.0, card: None, review: false, jump: None };
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
        self.images = ImageTable::new();
        // the pictures first: a picture the table refuses stops the tile before it runs, with the table's words
        match super::store::store_of(cx).tile_pictures(&cx.session.policy, &self.user, &self.entry) {
            Ok(t) => self.images = t,
            Err(e) => self.err = Some(e),
        }
        if self.err.is_none() {
            match super::store::store_of(cx).tile_open_wasm(&cx.session.policy, &self.user, &self.entry) {
                Ok(r) => self.runner = Some(r),
                Err(e) => self.err = Some(e),
            }
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
                let img = render_with(&list, w, h, BG, &self.images);
                self.picture = IntSize::from_wh(w as u32, h as u32).and_then(|sz| Pixmap::from_vec(img.rgba, sz));
                self.list = Some(list);
                self.dirty = false;
            }
            Err(e) => self.stop(e),
        }
    }

    /// The tile's own access tree (ABI 2 `Access` operations, F-A11Y-06): each node becomes a hit with the tile's name, role and state
    /// in the toolkit's tree, in the tile's reading order, so the screen reader and the keyboard walk treat it like a native control.
    /// Activating it clicks the middle of its rectangle, as a pointer would. A tile of ABI 1 gives none and keeps the one labelled hit.
    fn register_access(&self, g: &mut Gfx, ox: f32, oy: f32, s: f32) {
        use crate::access::{describe, Meta, Role, State};
        use iyakki_tile_abi::{access_flags as f, Role as TRole};
        for (i, n) in self.list.iter().flat_map(|l| l.access_tree()).take(ACCESS_MAX).enumerate() {
            let rect = R::new(ox + n.rect.x as f32 * s, oy + n.rect.y as f32 * s, n.rect.w as f32 * s, n.rect.h as f32 * s);
            let role = match n.role {
                TRole::Button => Role::Button,
                TRole::Field => Role::Field,
                TRole::ListItem => Role::ListItem,
                TRole::Toggle => Role::Toggle,
                TRole::Tab => Role::Tab,
                TRole::Heading => Role::Heading,
                TRole::Slider => Role::Slider,
                TRole::Link => Role::Link,
                TRole::Text => Role::Spoken,
            };
            let mut st = State::default().selected(n.flags & f::SELECTED != 0).disabled(n.flags & f::DISABLED != 0);
            if matches!(n.role, TRole::Toggle) || n.flags & f::CHECKED != 0 {
                st = st.checked(n.flags & f::CHECKED != 0);
            }
            if n.flags & f::EXPANDED != 0 {
                st = st.expanded(true);
            }
            if !n.value.is_empty() && n.flags & f::SECRET == 0 {
                st = st.value(n.value.clone());
            }
            let ev = Event::Area(AREA + 1 + i);
            describe(&n.name, &ev, Meta::new(role, st));
            g.hit(rect, ev, &n.name);
        }
    }

    /// The middle of the `i`th access node in surface pixels (what an activation clicks), if there is one.
    fn access_centre(&self, i: usize) -> Option<(i32, i32)> {
        let n = self.list.as_ref()?.access_tree().into_iter().nth(i)?;
        Some((n.rect.x + n.rect.w / 2, n.rect.y + n.rect.h / 2))
    }

    /// A `dock.open` the tile made since the last event: kept for the shell (`take_jump`) and said in the audit log. The newest wins.
    fn take_dock_open(&mut self, cx: &AppCx) {
        if let Some(app) = self.runner.as_ref().and_then(|r| r.take_dock_open()) {
            cx.audit.record(&self.user, "desktop.dock.open", &format!("{app} (asked by {})", self.entry.name));
            self.jump = Some((app, String::new()));
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
        "WASM tile (experimental): click and type in the tile · Ctrl+B screen reader review · Ctrl+R start it again · Esc back".into()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        // a card of the SDK bridge waits for the person: only a key or a press on its buttons answers it, and nothing else reaches the tile
        if let Some((id, card)) = self.card.as_mut() {
            let id = *id;
            match card.handle(ev) {
                Some(Key::Char('y')) => self.answer_card(id, true, cx),
                Some(Key::Esc) => self.answer_card(id, false, cx),
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Ctrl('r')) => self.load(cx),
            Event::Key(Key::Ctrl('b')) | Event::Tab(REVIEW_TAB) => {
                self.review = !self.review;
                self.dirty = true;
            }
            Event::Tick => {
                self.ticks += 1;
                self.look_for_card(cx);
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
            Event::AreaAt(a, ..) if *a > AREA && *a <= AREA + ACCESS_MAX && self.runner.is_some() && self.started => {
                if let Some((lx, ly)) = self.access_centre(*a - AREA - 1) {
                    match self.runner.as_mut().map(|r| r.click(lx, ly)) {
                        Some(Ok(_)) => self.dirty = true,
                        Some(Err(e)) => self.stop(e),
                        None => {}
                    }
                }
            }
            Event::Key(Key::Char(c)) if self.started => self.send(&[TileEvent::TextInput { text: c.to_string() }]),
            Event::Key(Key::Backspace) if self.started => self.send(&[TileEvent::Key { code: 8, mods: 0, down: true }]),
            Event::Key(Key::Enter) if self.started => self.send(&[TileEvent::Key { code: 13, mods: 0, down: true }]),
            // a pinch goes only to a tile that declared ABI 3 (D-SDK-34-01); older tiles never hear it, and the Ctrl+plus key of the gesture reaches no tile
            Event::Pinch { .. } | Event::PinchEnd { .. } if self.started => {
                let abi3 = self.runner.as_ref().is_some_and(|r| r.takes_app_events());
                if let Some(t) = pinch_for_tile(ev, abi3, self.surface, self.scale) {
                    self.send(&[t]);
                }
            }
            Event::Paste(t) if self.started => self.send(&[TileEvent::TextInput { text: t.chars().take(256).collect() }]),
            _ => {}
        }
        self.take_dock_open(cx);
        Flow::Stay
    }

    fn take_jump(&mut self) -> Option<(String, String)> {
        self.jump.take()
    }

    fn wants_redraw(&mut self) -> bool {
        self.dirty
    }

    #[cfg(any(test, feature = "testing"))]
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
        // the button that shows the review; the review replaces the tile's picture while it is on
        let label = if self.review { "Back to the tile (Ctrl+B)" } else { "Screen reader review (Ctrl+B)" };
        let bw = g.text_width(size, label, Face::Sans) + 2.0 * pad;
        g.button(a.x + a.w - bw - pad, a.y + 4.0 * s, size, label, ACCENT, Event::Tab(REVIEW_TAB));
        if self.review {
            self.draw_review(g, area, s);
            return;
        }
        self.scale = s;
        self.surface = R::new(area.x, area.y, w as f32 * s, h as f32 * s);
        if let Some(pm) = &self.picture {
            let paint = PixmapPaint { quality: FilterQuality::Nearest, ..PixmapPaint::default() };
            g.pm_mut().draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(area.x, area.y).pre_scale(s, s), None);
            g.hit(self.surface, Event::Area(AREA), "WASM tile");
            self.register_access(g, area.x, area.y, s);
        } else if self.err.is_none() {
            g.text(area.x, area.y, 15.0 * s, TEXT, "Starting the tile...", Face::Sans);
        }
        if let Some(why) = self.err.clone() {
            g.text(area.x, area.y + 8.0 * s, 15.0 * s, ERR, &why, Face::Sans);
        }
        if let Some((_, card)) = self.card.as_mut() {
            card.render(g, a, s);
        }
    }
}

impl WasmTileApp {
    /// The drawn review (F-A11Y-06): what a screen reader cannot name in the picture the tile shows now, and when the store turns strict.
    fn draw_review(&self, g: &mut Gfx, area: R, s: f32) {
        let mut y = area.y + 8.0 * s;
        g.text(area.x, y, 18.0 * s, TEXT, "Screen reader review", Face::Bold);
        y += g.line_height(18.0 * s, Face::Bold) + 8.0 * s;
        let Some(list) = &self.list else {
            g.text(area.x, y, 14.0 * s, MUTED, "The tile has not drawn a picture yet.", Face::Sans);
            return;
        };
        let lint = iyakki_tile_host::lint::lint_of_list(list);
        let today = iyakki_shared::release::civil_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        let review = lint.review(iyakki_tile_host::lint::STRICT_FROM, &today);
        for line in review.screen_lines(&lint) {
            for l in g.wrap(14.0 * s, &line, Face::Sans, area.w) {
                g.text(area.x, y, 14.0 * s, if review.warning.is_empty() { TEXT } else { ERR }, &l, Face::Sans);
                y += g.line_height(14.0 * s, Face::Sans) + 2.0 * s;
            }
            y += 6.0 * s;
        }
    }

    /// Take the oldest request of the SDK bridge that waits for the person and make its card; a kind this screen cannot show is answered no.
    fn look_for_card(&mut self, cx: &AppCx) {
        if self.card.is_some() {
            return;
        }
        let Some(r) = self.runner.as_ref() else { return };
        for (id, kind, args) in r.requests() {
            if matches!(kind.as_str(), "sdk.approve" | "sdk.card") {
                let args: serde_json::Value = serde_json::from_str(&args).unwrap_or_default();
                self.card = Some((id, super::tile::sdk_card(&format!("“{}”", self.entry.title), &kind, &args)));
                self.dirty = true;
                return;
            }
            // the other requests of a page (clipboard, files, the assistant) have no card here: a WebAssembly tile gets a plain no
            let _ = r.answer(id, false);
            cx.audit.record(&self.user, "tile.sdk.deny", &format!("{} was refused {kind} (no card for it on this screen)", self.entry.name));
        }
    }

    fn answer_card(&mut self, id: u32, allow: bool, cx: &AppCx) {
        if let Some(r) = self.runner.as_ref() {
            let _ = r.answer(id, allow);
        }
        cx.audit.record(&self.user, if allow { "tile.sdk.allow" } else { "tile.sdk.deny" }, &format!("{} {}", self.entry.name, if allow { "was allowed what it asked" } else { "was refused what it asked" }));
        self.card = None;
        self.dirty = true;
    }
}

/// The tile event for a pinch of the shell, in the tile's own pixels; none for a tile that did not declare ABI 3 or for any other event.
fn pinch_for_tile(ev: &Event, tile_takes_abi3: bool, surface: R, scale: f32) -> Option<TileEvent> {
    if !tile_takes_abi3 {
        return None;
    }
    match ev {
        Event::Pinch { scale_milli, x, y } => Some(TileEvent::Pinch { scale_milli: *scale_milli, x: ((*x as f32 - surface.x) / scale) as i32, y: ((*y as f32 - surface.y) / scale) as i32 }),
        Event::PinchEnd { scale_milli } => Some(TileEvent::PinchEnd { scale_milli: *scale_milli }),
        _ => None,
    }
}

#[cfg(test)]
mod pinch_tests {
    use super::*;

    #[test]
    fn a_pinch_reaches_only_a_tile_of_abi_3_in_the_tiles_own_pixels() {
        let surface = R::new(100.0, 50.0, 400.0, 300.0);
        let p = Event::Pinch { scale_milli: 1500, x: 300, y: 150 };
        assert_eq!(pinch_for_tile(&p, true, surface, 2.0), Some(TileEvent::Pinch { scale_milli: 1500, x: 100, y: 50 }));
        assert_eq!(pinch_for_tile(&Event::PinchEnd { scale_milli: 1500 }, true, surface, 2.0), Some(TileEvent::PinchEnd { scale_milli: 1500 }));
        assert_eq!(pinch_for_tile(&p, false, surface, 2.0), None, "an older tile hears nothing");
        assert_eq!(pinch_for_tile(&Event::Tick, true, surface, 2.0), None);
    }
}

#[cfg(test)]
mod card_tests {
    use super::super::tile::sdk_card;
    use serde_json::json;

    #[test]
    fn the_first_use_cards_say_what_the_app_asks_and_have_a_yes_and_a_no() {
        let c = sdk_card("“Notes”", "sdk.approve", &json!({"ns": "llm", "model": "small", "where": "this computer", "leaves": false}));
        assert!(c.title.contains("AI model") && c.lines.iter().any(|l| l.0.contains("nothing leaves it")), "{:?}", c.lines);
        assert_eq!(c.actions.len(), 2);
        let s = sdk_card("“Notes”", "sdk.approve", &json!({"ns": "speech", "where": "stays on this computer"}));
        assert!(s.title.contains("read aloud"));
        let k = sdk_card("“Notes”", "sdk.card", &json!({"title": "Show a page", "lines": ["It opens example.org"], "everyTime": true}));
        assert_eq!(k.title, "Show a page");
        assert!(k.lines.iter().any(|l| l.0.contains("every time")));
    }
}
