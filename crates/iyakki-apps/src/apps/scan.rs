//! NET SCAN: the tile itself lives in the `iyakki-tile-scan` crate (it does not depend on the agent, the shared crate or the scanner). This file is
//! the shell's side of it: the adapter that makes the tile an `App` of the shell, over the real port in `scan_port.rs` (the core's permission-checked
//! scanner, the account's host policy, the new-device watch, the audit line) and the settings writes in `scan_models.rs`. Model servers (Ollama
//! and others) can be added to Settings → Models in one step, with an API key going into the system vault; scans stay on private addresses and
//! every scan is written to the audit log.

use super::scan_port::{Bridge, CxPort, ScanPort};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_scan::{is_lemonade, Answer, FormKind, Mode, ACCENT};
use iyakki_tile_scan as tile;

pub struct ScanApp {
    tile: tile::ScanTile,
}

impl ScanApp {
    pub fn new(cx: &AppCx) -> ScanApp {
        ScanApp::with_port(&CxPort(cx))
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn ScanPort) -> ScanApp {
        ScanApp { tile: tile::ScanTile::new(&Bridge(port)) }
    }

    /// Hand the tile the answers that came in (the tests drive this without a frame).
    #[cfg(test)]
    pub(super) fn poll(&mut self, port: &dyn ScanPort) {
        self.tile.poll(&Bridge(port));
    }

    /// Start the chosen scan on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn start(&mut self, port: &dyn ScanPort) {
        self.tile.start(&Bridge(port));
    }

    /// An event of the devices view on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn handle_devices(&mut self, ev: &Event, port: &dyn ScanPort) -> Flow {
        match self.tile.handle_devices(ev, &Bridge(port)) {
            Answer::Stay => Flow::Stay,
            Answer::Back => Flow::Back,
        }
    }
}

#[cfg(test)]
impl std::ops::Deref for ScanApp {
    type Target = tile::ScanTile;
    fn deref(&self) -> &tile::ScanTile {
        &self.tile
    }
}

#[cfg(test)]
impl std::ops::DerefMut for ScanApp {
    fn deref_mut(&mut self) -> &mut tile::ScanTile {
        &mut self.tile
    }
}

impl App for ScanApp {
    fn title(&self) -> &'static str {
        self.tile.title()
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
    fn wants_redraw(&mut self) -> bool {
        self.tile.wants_redraw()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.tile.handle(ev, &Bridge(&CxPort(cx))) {
            Answer::Stay => Flow::Stay,
            Answer::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, &Bridge(&CxPort(cx)), tick)
    }
}
