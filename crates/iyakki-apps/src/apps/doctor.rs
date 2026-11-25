//! DOCTOR: the tile itself lives in the `iyakki-tile-doctor` crate (it does not depend on the agent or the shared crate). This file is the
//! shell's side of it: the tile's port over the machine's facts, the support bundle and the core (`doctor_port.rs`, `doctor_facts.rs`), the
//! "Manual tests" tab that the shell owns (`doctor_manual*.rs`, handed to the tile through its pane trait), and the adapter that makes the
//! tile an `App` of the shell. The rules and their facts stay in `iyakki_shared::doctor`, the bundle in `iyakki_shared::diagshare`.

use iyakki_tile_doctor as tile;

pub use super::doctor_manual::ManualTab;
use super::doctor_manual::Out;
pub use super::doctor_port::Parts;
use super::doctor_port::{Bridge, CxPort, DoctorPort, Held, State};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use tile::{ACCENT, A_LIST, A_PREVIEW, T_CLOSE, T_COPY, T_DISMISS, T_FIX, T_RECHECK, T_SAVE, T_SHARE, T_TAB_FINDINGS, T_TAB_MANUAL};

/// The manual tab over the shell's port: what the tile sees of it.
struct Pane<'a> {
    tab: &'a mut Option<ManualTab>,
    port: &'a dyn DoctorPort,
    now: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl tile::ManualPane for Pane<'_> {
    fn is_open(&self) -> bool {
        self.tab.is_some()
    }
    fn open(&mut self) {
        if self.tab.is_none() {
            *self.tab = Some(ManualTab::open(self.port, self.now.clone()));
        }
    }
    fn handle(&mut self, ev: &Event) -> tile::PaneOut {
        match self.tab.as_mut().map(|m| m.handle(ev, self.port)) {
            Some(Out::Leave) => tile::PaneOut::Leave,
            _ => tile::PaneOut::Stay,
        }
    }
    fn draw(&mut self, g: &mut Gfx, area: R, s: f32) {
        if let Some(m) = self.tab.as_mut() {
            m.draw(g, area, s);
        }
    }
    fn wheel_px(&mut self, px: i32) -> bool {
        self.tab.as_mut().is_some_and(|m| m.main.wheel_px(px) || m.nav.wheel_px(px))
    }
    fn take_dirty(&mut self) -> bool {
        self.tab.as_mut().is_some_and(|m| std::mem::take(&mut m.dirty))
    }
}

pub struct DoctorApp {
    pub(super) tile: tile::DoctorTile,
    pub(super) parts: Parts,
    st: State,
    /// The "Manual tests" tab (made when it is first opened); the tile only knows whether it is the one showing.
    pub(super) manual: Option<ManualTab>,
}

impl DoctorApp {
    pub fn new(cx: &AppCx) -> DoctorApp {
        DoctorApp::with_parts(cx, Parts::real())
    }

    pub fn with_parts(cx: &AppCx, parts: Parts) -> DoctorApp {
        DoctorApp::with_port(&CxPort(cx), parts)
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn DoctorPort, parts: Parts) -> DoctorApp {
        let st = State::default();
        let t = tile::DoctorTile::new(&Bridge { port, parts: &parts, st: &st });
        DoctorApp { tile: t, parts, st, manual: None }
    }

    // the tile's inner steps on a port of the caller's choosing (the tests drive them without a rendered frame)
    #[cfg(test)]
    pub(super) fn do_fix(&mut self, i: usize, port: &dyn DoctorPort) {
        self.tile.do_fix(i, &Bridge { port, parts: &self.parts, st: &self.st });
    }
    #[cfg(test)]
    pub(super) fn open_share(&mut self, port: &dyn DoctorPort) {
        self.tile.open_share(&Bridge { port, parts: &self.parts, st: &self.st });
    }
    #[cfg(test)]
    pub(super) fn open_manual(&mut self, port: &dyn DoctorPort) {
        let mut pane = Pane { tab: &mut self.manual, port, now: self.parts.now.clone() };
        self.tile.open_manual(&Bridge { port, parts: &self.parts, st: &self.st }, &mut pane);
    }
    #[cfg(test)]
    pub(super) fn open_item_inner(&mut self, item: &str, port: &dyn DoctorPort) {
        let mut pane = Pane { tab: &mut self.manual, port, now: self.parts.now.clone() };
        self.tile.open_item(item, &Bridge { port, parts: &self.parts, st: &self.st }, &mut pane);
    }
    /// One event for the manual tab, on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn handle_manual(&mut self, ev: &Event, port: &dyn DoctorPort) {
        let mut pane = Pane { tab: &mut self.manual, port, now: self.parts.now.clone() };
        let _ = self.tile.handle(ev, &Bridge { port, parts: &self.parts, st: &self.st }, &mut pane);
    }
    #[cfg(test)]
    pub(super) fn save_to(&mut self, i: usize, port: &dyn DoctorPort) {
        self.tile.save_to(i, &Bridge { port, parts: &self.parts, st: &self.st });
    }
}

impl App for DoctorApp {
    fn title(&self) -> &'static str {
        self.tile.title()
    }
    fn page(&self) -> String {
        self.tile.page()
    }
    fn icon(&self) -> Icon {
        self.tile.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.tile.accent()
    }
    fn hint(&self) -> String {
        self.tile.hint()
    }
    fn help(&self) -> Vec<String> {
        self.tile.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let port = CxPort(cx);
        let mut pane = Pane { tab: &mut self.manual, port: &port, now: self.parts.now.clone() };
        match self.tile.handle(ev, &Bridge { port: &port, parts: &self.parts, st: &self.st }, &mut pane) {
            tile::Answer::Stay => Flow::Stay,
            tile::Answer::Back => Flow::Back,
        }
    }
    fn scroll_px(&mut self, px: i32, cx: &AppCx) -> bool {
        let port = CxPort(cx);
        let mut pane = Pane { tab: &mut self.manual, port: &port, now: self.parts.now.clone() };
        self.tile.scroll_px(px, &mut pane)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.tile.take_request()
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        let port = CxPort(cx);
        let mut pane = Pane { tab: &mut self.manual, port: &port, now: self.parts.now.clone() };
        self.tile.open_item(item, &Bridge { port: &port, parts: &self.parts, st: &self.st }, &mut pane);
    }
    fn wants_redraw(&mut self) -> bool {
        // between frames the shell hands over no context: the slow answer only touches the state, the manual tab only its dirty flag
        let held = Held { admin: self.tile.admin };
        let mut pane = Pane { tab: &mut self.manual, port: &held, now: self.parts.now.clone() };
        self.tile.wants_redraw(&Bridge { port: &held, parts: &self.parts, st: &self.st }, &mut pane)
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let port = CxPort(cx);
        let mut pane = Pane { tab: &mut self.manual, port: &port, now: self.parts.now.clone() };
        self.tile.render(g, area, s, &Bridge { port: &port, parts: &self.parts, st: &self.st }, &mut pane, tick);
    }
}
