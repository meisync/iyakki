//! CHECK-UP: the tile itself lives in the `iyakki-tile-checkup` crate (it does not depend on the agent or the shared crate). This file is the
//! shell's side of it: the tile's port over the machine's inventory, the audio and camera readers and the core (`checkup_port.rs`), and the
//! adapter that makes the tile an `App` of the shell. The steps and their verdicts stay in `iyakki_shared::checkup` (docs/CHECKUP.md).

use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

use iyakki_shared::checkup::Inventory;
use iyakki_tile_checkup as tile;

use super::checkup_port::{Bridge, CheckupPort, CxPort};
pub use super::checkup_port::Parts;
use super::{App, AppCx, Flow};
use crate::camera::probe::Shot;
use crate::gfx::Icon;
use crate::ui::{Event, R};

pub use tile::{Active, ACCENT, A_LIST, T_AGAIN, T_AGAIN_TONE, T_DISMISS, T_DOCTOR, T_FIX, T_NO, T_ROW_DOCTOR, T_RUN, T_SHARE, T_SKIP, T_STOP, T_TARGET, T_YES};

/// The port the tile uses between frames, where the shell hands over no context (`wants_redraw`): it keeps what the report needs (the
/// person's data folder and the cluster services). A step is never started from it, so the readers that need the context answer nothing.
struct Held {
    dir: std::path::PathBuf,
    k3s: Arc<dyn iyakki_agent_api::DevCluster>,
    inv: Inventory,
}

impl CheckupPort for Held {
    fn inventory(&self) -> Inventory {
        self.inv.clone()
    }
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String> {
        Err("The tone needs the shell's context.".into())
    }
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send> {
        Box::new(|| Shot { heard: iyakki_shared::checkup::CamHeard::NoCamera("The camera needs the shell's context.".into()), picture: None })
    }
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        Default::default()
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
    fn k3s(&self) -> Arc<dyn iyakki_agent_api::DevCluster> {
        self.k3s.clone()
    }
    fn audit(&self, _action: &str, _subject: &str) {}
}

pub struct CheckupApp {
    pub(super) tile: tile::CheckupTile,
    parts: Parts,
    dir: std::path::PathBuf,
    k3s: Arc<dyn iyakki_agent_api::DevCluster>,
    /// The machine's last reading; the tile asks the port to judge steps and never sees it.
    inv: RefCell<Inventory>,
}

impl CheckupApp {
    pub fn new(cx: &AppCx) -> CheckupApp {
        CheckupApp::with_parts(cx, Parts::real())
    }

    pub fn with_parts(cx: &AppCx, parts: Parts) -> CheckupApp {
        CheckupApp::with_port(&CxPort(cx), parts)
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn CheckupPort, parts: Parts) -> CheckupApp {
        let inv = RefCell::new(Inventory::default());
        let t = tile::CheckupTile::new(&Bridge { port, parts: &parts, inv: &inv });
        CheckupApp { tile: t, parts, dir: port.data_dir(), k3s: port.k3s(), inv }
    }

    #[cfg(test)]
    fn bridge<'a>(&'a self, port: &'a dyn CheckupPort) -> Bridge<'a> {
        Bridge { port, parts: &self.parts, inv: &self.inv }
    }

    // the tile's inner steps, on a port of the caller's choosing (the tests drive them without a rendered frame)
    #[cfg(test)]
    pub(super) fn begin(&mut self, i: usize, port: &dyn CheckupPort) {
        let b = Bridge { port, parts: &self.parts, inv: &self.inv };
        self.tile.begin(i, &b);
    }
    #[cfg(test)]
    pub(super) fn answer(&mut self, said: iyakki_tile_api::checkup::Said, port: &dyn CheckupPort) {
        let b = Bridge { port, parts: &self.parts, inv: &self.inv };
        self.tile.answer(said, &b);
    }
    #[cfg(test)]
    pub(super) fn do_fix(&mut self, port: &dyn CheckupPort) {
        let b = Bridge { port, parts: &self.parts, inv: &self.inv };
        self.tile.do_fix(&b);
    }
    #[cfg(test)]
    pub(super) fn end_active(&mut self, port: &dyn CheckupPort) {
        let b = Bridge { port, parts: &self.parts, inv: &self.inv };
        self.tile.end_active(&b);
    }
    #[cfg(test)]
    pub(super) fn keep_report(&self, port: &dyn CheckupPort) {
        self.tile.keep_report(&self.bridge(port));
    }
}

impl Drop for CheckupApp {
    fn drop(&mut self) {
        // the tile is closed: a recording or a camera picture in progress is dropped and the sign goes out
        if self.tile.release() {
            (self.parts.sign)(0);
        }
    }
}

impl App for CheckupApp {
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
        match self.tile.handle(ev, &Bridge { port: &port, parts: &self.parts, inv: &self.inv }) {
            tile::Answer::Stay => Flow::Stay,
            tile::Answer::Back => Flow::Back,
        }
    }
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.tile.scroll_px(px)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.tile.take_request()
    }
    fn keeps_screen_on(&self) -> bool {
        self.tile.keeps_screen_on()
    }
    fn wants_redraw(&mut self) -> bool {
        // the tile polls with the shell's context only to read the machine; the real parts need none, and a test's parts are closures
        let port = Held { dir: self.dir.clone(), k3s: self.k3s.clone(), inv: self.inv.borrow().clone() };
        let b = Bridge { port: &port, parts: &self.parts, inv: &self.inv };
        self.tile.wants_redraw(&b)
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let port = CxPort(cx);
        self.tile.render(g, area, s, &Bridge { port: &port, parts: &self.parts, inv: &self.inv }, tick);
    }
}
